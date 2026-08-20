import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import type { Participant } from "../src/protocol";

/**
 * Mute and deafen for task 3.2.
 *
 * Both are decided on the client — mute stops packets at the encoder, deafen
 * stops playback — so what the room owns is the *other* half: telling everyone
 * else. A roster that does not carry the flag leaves seven people watching a
 * silent participant with no way to tell talking from muted.
 */

const BASE = "https://goodvoice.test";

interface JoinResponse {
  self: string;
  participants: Participant[];
}

async function join(code: string, name: string): Promise<JoinResponse> {
  const response = await SELF.fetch(`${BASE}/rooms/${code}/join`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
  expect(response.status).toBe(200);
  return (await response.json()) as JoinResponse;
}

/** An accepted socket with its `welcome` already consumed. */
async function connect(code: string, participant: string): Promise<WebSocket> {
  const response = await SELF.fetch(
    `${BASE}/rooms/${code}/ws?p=${participant}`,
    { headers: { upgrade: "websocket" } },
  );
  expect(response.status).toBe(101);

  const socket = response.webSocket!;
  socket.accept();
  await expect(nextMessage(socket)).resolves.toMatchObject({ type: "welcome" });
  return socket;
}

function nextMessage(socket: WebSocket): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no message")), 2000);
    socket.addEventListener(
      "message",
      (event) => {
        clearTimeout(timer);
        resolve(JSON.parse(event.data as string) as Record<string, unknown>);
      },
      { once: true },
    );
  });
}

/** The next roster push, as a map of name → participant. */
async function nextRoster(
  socket: WebSocket,
): Promise<Record<string, Participant>> {
  const message = await nextMessage(socket);
  expect(message.type).toBe("roster");

  const participants = message.participants as Participant[];
  return Object.fromEntries(participants.map((p) => [p.name, p]));
}

/**
 * Two participants, both connected, both quiet — the state every test below
 * starts from. Sockets are opened after both joins so no roster push from the
 * setup is left queued to be mistaken for the one under test.
 */
async function pair(code: string) {
  const ana = await join(code, "ana");
  const bruno = await join(code, "bruno");

  return {
    ana: { ...ana, socket: await connect(code, ana.self) },
    bruno: { ...bruno, socket: await connect(code, bruno.self) },
  };
}

describe("mute", () => {
  it("shows up on everyone else's roster", async () => {
    const { ana, bruno } = await pair("presence-mute");

    const pushed = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: true }));

    const roster = await pushed;
    expect(roster.ana?.muted).toBe(true);
    // One participant's mute is not the room's: nobody else moved.
    expect(roster.bruno?.muted).toBe(false);

    ana.socket.close();
    bruno.socket.close();
  });

  it("comes back off again", async () => {
    const { ana, bruno } = await pair("presence-unmute");

    const muted = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: true }));
    expect((await muted).ana?.muted).toBe(true);

    const unmuted = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: false }));
    expect((await unmuted).ana?.muted).toBe(false);

    ana.socket.close();
    bruno.socket.close();
  });
});

describe("deafen", () => {
  it("shows up on everyone else's roster", async () => {
    const { ana, bruno } = await pair("presence-deafen");

    const pushed = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "deafen", deafened: true }));

    const roster = await pushed;
    expect(roster.ana?.deafened).toBe(true);
    // Deafen is not mute: someone who cannot hear you can still talk.
    expect(roster.ana?.muted).toBe(false);

    ana.socket.close();
    bruno.socket.close();
  });

  it("stacks with mute rather than replacing it", async () => {
    const { ana, bruno } = await pair("presence-both");

    const muted = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: true }));
    await muted;

    const deafened = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "deafen", deafened: true }));

    const roster = await deafened;
    expect(roster.ana).toMatchObject({ muted: true, deafened: true });

    ana.socket.close();
    bruno.socket.close();
  });
});

describe("the flags a client sets", () => {
  it("are already there for someone who joins later", async () => {
    // A newcomer must not have to wait for the next change to learn who is
    // muted: the join answer carries the room as it stands.
    const code = "presence-latecomer";
    const { ana, bruno } = await pair(code);

    const muted = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: true }));
    await muted;

    const carla = await join(code, "carla");
    const seen = Object.fromEntries(
      carla.participants.map((p) => [p.name, p]),
    ) as Record<string, Participant>;

    expect(seen.ana?.muted).toBe(true);
    expect(seen.bruno?.muted).toBe(false);

    ana.socket.close();
    bruno.socket.close();
  });

  it("are not moved by a message the room cannot parse", async () => {
    const { ana, bruno } = await pair("presence-garbage");

    const muted = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "mute", muted: true }));
    await muted;

    // The sender is told; the room is not disturbed.
    const answered = nextMessage(ana.socket);
    ana.socket.send("not json at all");
    expect(await answered).toMatchObject({
      type: "error",
      code: "bad_request",
    });

    const carla = nextRoster(bruno.socket);
    await join("presence-garbage", "carla");
    expect((await carla).ana?.muted).toBe(true);

    ana.socket.close();
    bruno.socket.close();
  });
});

describe("leave", () => {
  it("takes the participant off everyone's roster at once", async () => {
    const { ana, bruno } = await pair("presence-leave");

    const pushed = nextRoster(bruno.socket);
    ana.socket.send(JSON.stringify({ type: "leave" }));

    const roster = await pushed;
    expect(Object.keys(roster)).toEqual(["bruno"]);

    bruno.socket.close();
  });
});
