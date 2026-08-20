import { DurableObject } from "cloudflare:workers";

import type { Env } from "./env";
import {
  clientMessageSchema,
  displayNameSchema,
  joinRequestSchema,
  MAX_PARTICIPANTS,
  RoomError,
  sfuProxyBodySchema,
  type ClientMessage,
  type Participant,
  type ServerMessage,
} from "./protocol";
import {
  createSfuSession,
  isSfuOperation,
  proxySfuRequest,
  sfuOperationMethod,
} from "./sfu";

/** A participant plus the bits that never leave the server. */
interface Member extends Participant {
  socket: WebSocket | null;
  /** Last time we heard anything at all from them. */
  lastSeen: number;
  /**
   * The Realtime session created for them at join. Null between taking the
   * roster slot and the credential exchange coming back — and for participants
   * created straight through {@link Room.join} in tests.
   */
  sessionId: string | null;
}

/**
 * How long a participant may stay silent before the room drops them. A client
 * that is alive but idle still sends a heartbeat well inside this window.
 */
export const HEARTBEAT_TIMEOUT_MS = 30_000;

/** How often the alarm re-checks. Fires only while the room has members. */
const SWEEP_INTERVAL_MS = 10_000;

/** What `POST /rooms/:code/join` answers with. */
export interface JoinResult {
  self: string;
  participants: Participant[];
}

/**
 * One Durable Object per room code. State is in-memory only: rooms are
 * ephemeral and the storage API is never touched (prd.md §7). When the last
 * participant leaves, the map is emptied and the object is indistinguishable
 * from a room that never existed.
 */
export class Room extends DurableObject<Env> {
  #members = new Map<string, Member>();

  /**
   * Alarm bookkeeping runs off the request path, so it has to be serialised:
   * otherwise a `setAlarm` started by a join can land *after* the
   * `deleteAlarm` of the room emptying, resurrecting a timer for a room that
   * no longer exists.
   */
  #alarmWork: Promise<unknown> = Promise.resolve();

  /**
   * The Worker forwards `/rooms/:code/{join,ws}` here as `/join` and `/ws`.
   * Errors are answered rather than thrown: a `Response` crosses the Durable
   * Object boundary intact, while a custom error class loses its fields.
   */
  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);

    try {
      if (url.pathname === "/join") {
        const body = joinRequestSchema.safeParse(
          await request.json().catch(() => null),
        );
        if (!body.success) {
          throw new RoomError(
            "bad_request",
            "expected a JSON body with a name",
          );
        }

        // Take the slot first so a full room never burns an SFU session, and
        // give it back if the SFU turns us down — a participant with no
        // session can neither speak nor be heard.
        const joined = this.join(body.data.name);
        try {
          const sfu = await createSfuSession(this.env);
          this.attachSession(joined.self, sfu.sessionId);
          return Response.json({ ...joined, sfu });
        } catch (error) {
          this.leave(joined.self);
          throw error;
        }
      }

      if (url.pathname.startsWith("/sfu/")) {
        return await this.#proxySfu(url, request);
      }

      if (url.pathname === "/ws") {
        if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
          throw new RoomError("bad_request", "expected a WebSocket upgrade");
        }
        const participant = url.searchParams.get("p");
        if (!participant) {
          throw new RoomError("bad_request", "missing participant id");
        }
        return this.openSocket(participant);
      }
    } catch (error) {
      if (error instanceof RoomError) {
        return error.toResponse();
      }
      throw error;
    }

    return new Response("not found", { status: 404 });
  }

  // --- roster API, also called directly by unit tests ---------------------

  /**
   * Registers a participant and returns the roster as they will see it.
   *
   * @throws {RoomError} `room_full` past {@link MAX_PARTICIPANTS}.
   */
  join(rawName: string): JoinResult {
    const parsed = displayNameSchema.safeParse(rawName);
    if (!parsed.success) {
      throw new RoomError("bad_request", "invalid display name");
    }

    if (this.#members.size >= MAX_PARTICIPANTS) {
      throw new RoomError(
        "room_full",
        `room is full (${MAX_PARTICIPANTS} participants)`,
      );
    }

    const id = crypto.randomUUID();
    const now = Date.now();
    this.#members.set(id, {
      id,
      name: parsed.data,
      joinedAt: now,
      muted: false,
      deafened: false,
      sharing: false,
      socket: null,
      lastSeen: now,
      sessionId: null,
    });

    // The clock starts here, not at the WebSocket: a client that joins and
    // never connects is swept out like any other silent participant.
    this.#queueAlarmWork(() => this.#scheduleSweep());
    this.#broadcastRoster();
    return { self: id, participants: this.roster() };
  }

  /**
   * Binds the Realtime session the credential exchange just created to the
   * participant it was created for. Nothing else may negotiate on it: this
   * mapping is what the SFU proxy authorises against.
   */
  attachSession(participantId: string, sessionId: string): void {
    const member = this.#members.get(participantId);
    if (member) {
      member.sessionId = sessionId;
    }
  }

  /**
   * Upgrades a connection for an already-joined participant.
   *
   * @throws {RoomError} `unknown_participant` if the id was never issued here.
   */
  openSocket(participantId: string): Response {
    const member = this.#members.get(participantId);
    if (!member) {
      throw new RoomError("unknown_participant", "join the room first");
    }

    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    server.accept();

    // A reconnect on the same id replaces the old socket rather than
    // duplicating the participant.
    member.socket?.close(1000, "replaced by a newer connection");
    member.socket = server;
    member.lastSeen = Date.now();

    server.addEventListener("message", (event) => {
      this.#onMessage(participantId, event.data);
    });
    const drop = () => this.leave(participantId);
    server.addEventListener("close", drop);
    server.addEventListener("error", drop);

    // No broadcast: connecting does not change the roster. Everyone already
    // learned about this participant when `join()` registered them.
    this.#send(server, {
      type: "welcome",
      self: participantId,
      participants: this.roster(),
    });

    return new Response(null, { status: 101, webSocket: client });
  }

  /** Removes a participant. Emptying the room wipes every trace of it. */
  leave(participantId: string): void {
    const member = this.#members.get(participantId);
    if (!member) {
      return;
    }

    this.#members.delete(participantId);
    member.socket?.close(1000, "left the room");

    if (this.#members.size === 0) {
      this.#reset();
      return;
    }

    this.#broadcastRoster();
  }

  /** The roster, ordered oldest-first so the UI list is stable. */
  roster(): Participant[] {
    return [...this.#members.values()]
      .sort((a, b) => a.joinedAt - b.joinedAt)
      .map(
        ({
          socket: _socket,
          lastSeen: _lastSeen,
          sessionId: _sessionId,
          ...participant
        }) => participant,
      );
  }

  /** Records that we just heard from a participant. Unknown ids are ignored. */
  touch(participantId: string, now: number = Date.now()): void {
    const member = this.#members.get(participantId);
    if (member) {
      member.lastSeen = now;
    }
  }

  /**
   * Evicts everyone who has gone quiet for longer than
   * {@link HEARTBEAT_TIMEOUT_MS}. `now` is a parameter so tests can look into
   * the future without waiting for it.
   *
   * @returns how many participants were evicted.
   */
  sweep(now: number = Date.now()): number {
    const stale = [...this.#members.values()].filter(
      (member) => now - member.lastSeen > HEARTBEAT_TIMEOUT_MS,
    );

    for (const member of stale) {
      member.socket?.close(1001, "heartbeat timeout");
      this.#members.delete(member.id);
    }

    if (stale.length === 0) {
      return 0;
    }

    if (this.#members.size === 0) {
      this.#reset();
    } else {
      this.#broadcastRoster();
    }
    return stale.length;
  }

  /**
   * Backstop for the case where every socket dies without firing a close
   * event — the room would otherwise sit there holding a phantom roster.
   */
  override async alarm(): Promise<void> {
    this.sweep();
    this.#queueAlarmWork(() => this.#scheduleSweep());
    await this.#alarmWork;
  }

  // --- internals ----------------------------------------------------------

  /**
   * `/rooms/:code/sfu/<operation>` — track negotiation, signed with the app
   * secret on the way out (DR-2). The room decides two things the SFU cannot:
   * that the caller owns the session being negotiated, and that any session
   * they pull from is one of their roommates'.
   */
  async #proxySfu(url: URL, request: Request): Promise<Response> {
    const operation = url.pathname.slice("/sfu/".length);
    if (!isSfuOperation(operation)) {
      throw new RoomError(
        "bad_request",
        `unsupported SFU operation: ${operation}`,
      );
    }
    if (request.method !== sfuOperationMethod(operation)) {
      throw new RoomError(
        "bad_request",
        `${operation} expects ${sfuOperationMethod(operation)}`,
      );
    }

    const participantId = url.searchParams.get("p");
    if (!participantId) {
      throw new RoomError("bad_request", "missing participant id");
    }

    const member = this.#members.get(participantId);
    if (!member) {
      throw new RoomError("unknown_participant", "join the room first");
    }
    if (!member.sessionId) {
      throw new RoomError(
        "sfu_unavailable",
        "this participant has no Realtime session",
      );
    }

    const body = await request.text();
    this.#assertTracksStayInTheRoom(body);

    // Negotiating a track proves the client is alive just as well as a
    // heartbeat does, and a long negotiation must not age them out.
    this.touch(participantId);

    return proxySfuRequest(this.env, member.sessionId, operation, body);
  }

  /**
   * Rejects a body that pulls from a session belonging to no one here. The
   * session id of a stranger is not secret enough to be a capability — without
   * this check, learning one would be enough to subscribe to their audio from
   * outside their room.
   */
  #assertTracksStayInTheRoom(body: string): void {
    const parsed = sfuProxyBodySchema.safeParse(
      ((): unknown => {
        try {
          return JSON.parse(body);
        } catch {
          return null;
        }
      })(),
    );
    if (!parsed.success) {
      throw new RoomError("bad_request", "expected a JSON body");
    }

    const inRoom = new Set(
      [...this.#members.values()]
        .map((member) => member.sessionId)
        .filter((sessionId): sessionId is string => sessionId !== null),
    );

    for (const track of parsed.data.tracks ?? []) {
      if (track.sessionId !== undefined && !inRoom.has(track.sessionId)) {
        throw new RoomError(
          "bad_request",
          "track refers to a session outside this room",
        );
      }
    }
  }

  #queueAlarmWork(task: () => Promise<unknown>): void {
    this.#alarmWork = this.#alarmWork.then(task, task);
  }

  /**
   * Room state stays in memory; the alarm is the one piece of storage the room
   * uses, and it holds no room data — only the fact that a sweep is due.
   */
  async #scheduleSweep(): Promise<void> {
    if (this.#members.size === 0) {
      return;
    }
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + SWEEP_INTERVAL_MS);
    }
  }

  #onMessage(participantId: string, raw: unknown): void {
    const member = this.#members.get(participantId);
    if (!member) {
      return;
    }

    // Any traffic at all proves the participant is alive, valid or not.
    this.touch(participantId);

    let message: ClientMessage;
    try {
      message = clientMessageSchema.parse(
        JSON.parse(typeof raw === "string" ? raw : ""),
      );
    } catch {
      if (member.socket) {
        this.#send(member.socket, {
          type: "error",
          code: "bad_request",
          message: "unparseable message",
        });
      }
      return;
    }

    switch (message.type) {
      case "heartbeat":
        // `lastSeen` above was the whole point; the roster is unchanged.
        return;
      case "mute":
        member.muted = message.muted;
        break;
      case "deafen":
        member.deafened = message.deafened;
        break;
      case "share":
        if (!this.#applyShare(member, message.sharing)) {
          return;
        }
        break;
      case "leave":
        this.leave(participantId);
        return;
    }

    this.#broadcastRoster();
  }

  /**
   * One sharer at a time (prd.md §8). Returns false when the request was
   * rejected, in which case the caller must not broadcast.
   */
  #applyShare(member: Member, sharing: boolean): boolean {
    if (sharing) {
      const other = [...this.#members.values()].find(
        (m) => m.sharing && m.id !== member.id,
      );
      if (other) {
        if (member.socket) {
          this.#send(member.socket, {
            type: "error",
            code: "already_sharing",
            message: `${other.name} is already sharing`,
          });
        }
        return false;
      }
    }
    member.sharing = sharing;
    return true;
  }

  #broadcastRoster(): void {
    const message: ServerMessage = {
      type: "roster",
      participants: this.roster(),
    };
    for (const member of this.#members.values()) {
      if (member.socket) {
        this.#send(member.socket, message);
      }
    }
  }

  #send(socket: WebSocket, message: ServerMessage): void {
    try {
      socket.send(JSON.stringify(message));
    } catch {
      // A socket that died between the roster snapshot and this send is
      // handled by its own close event; nothing to do here.
    }
  }

  /** Last one out turns off the lights: no roster, no pending alarm. */
  #reset(): void {
    this.#members.clear();
    this.#queueAlarmWork(() => this.ctx.storage.deleteAlarm());
  }
}
