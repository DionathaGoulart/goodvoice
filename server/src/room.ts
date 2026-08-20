import { DurableObject } from "cloudflare:workers";

import type { Env } from "./env";
import {
  clientMessageSchema,
  displayNameSchema,
  joinRequestSchema,
  MAX_PARTICIPANTS,
  RoomError,
  type ClientMessage,
  type Participant,
  type ServerMessage,
} from "./protocol";
import { createSfuSession } from "./sfu";

/** A participant plus the bits that never leave the server. */
interface Member extends Participant {
  socket: WebSocket | null;
}

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
          return Response.json({ ...joined, sfu });
        } catch (error) {
          this.leave(joined.self);
          throw error;
        }
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
    this.#members.set(id, {
      id,
      name: parsed.data,
      joinedAt: Date.now(),
      muted: false,
      deafened: false,
      sharing: false,
      socket: null,
    });

    this.#broadcastRoster();
    return { self: id, participants: this.roster() };
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
      .map(({ socket: _socket, ...participant }) => participant);
  }

  // --- internals ----------------------------------------------------------

  #onMessage(participantId: string, raw: unknown): void {
    const member = this.#members.get(participantId);
    if (!member) {
      return;
    }

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

  /** Last one out turns off the lights. */
  #reset(): void {
    this.#members.clear();
  }
}
