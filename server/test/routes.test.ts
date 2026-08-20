import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { MAX_PARTICIPANTS, type Participant } from "../src/protocol";
import type { SfuCredentials } from "../src/sfu";
import { TEST_SFU } from "./fake-realtime";

const BASE = "https://goodvoice.test";

interface JoinResponse {
  self: string;
  participants: Participant[];
  sfu: SfuCredentials;
}

async function join(code: string, name: string): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/join`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
}

async function joinOk(code: string, name: string): Promise<JoinResponse> {
  const response = await join(code, name);
  expect(response.status).toBe(200);
  return (await response.json()) as JoinResponse;
}

function openSocket(code: string, participant: string): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/ws?p=${participant}`, {
    headers: { upgrade: "websocket" },
  });
}

/** Resolves with the next message the socket receives, or rejects on timeout. */
function nextMessage(socket: WebSocket): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no message")), 2000);
    socket.addEventListener(
      "message",
      (event) => {
        clearTimeout(timer);
        resolve(JSON.parse(event.data as string));
      },
      { once: true },
    );
  });
}

describe("GET /health", () => {
  it("answers ok", async () => {
    const response = await SELF.fetch(`${BASE}/health`);

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ ok: true });
    expect(response.headers.get("access-control-allow-origin")).toBe("*");
  });
});

describe("routing", () => {
  it("404s anything that is not health or a room", async () => {
    for (const path of ["/", "/rooms", "/rooms/abcd", "/rooms/abcd/join/x"]) {
      expect((await SELF.fetch(`${BASE}${path}`)).status).toBe(404);
    }
  });

  it("404s a room code that fails validation", async () => {
    expect((await join("no", "ana")).status).toBe(404);
    expect((await join("has space", "ana")).status).toBe(404);
  });

  it("404s the wrong method on a real route", async () => {
    expect((await SELF.fetch(`${BASE}/rooms/lobby/join`)).status).toBe(404);
  });

  it("answers CORS preflight", async () => {
    const response = await SELF.fetch(`${BASE}/rooms/lobby/join`, {
      method: "OPTIONS",
    });

    expect(response.status).toBe(204);
    expect(response.headers.get("access-control-allow-origin")).toBe("*");
    expect(response.headers.get("access-control-allow-methods")).toContain(
      "POST",
    );
  });

  it("treats room codes case-insensitively", async () => {
    await joinOk("SquadRoom", "ana");
    const second = await joinOk("squadroom", "bruno");

    expect(second.participants.map((p) => p.name)).toEqual(["ana", "bruno"]);
  });
});

describe("POST /rooms/:code/join", () => {
  it("returns the participant id and roster", async () => {
    const body = await joinOk("join-basic", "ana");

    expect(body.self).toMatch(/^[0-9a-f-]{36}$/);
    expect(body.participants).toHaveLength(1);
    expect(body.participants[0]?.name).toBe("ana");
  });

  it("rejects a malformed body with a typed error", async () => {
    const response = await SELF.fetch(`${BASE}/rooms/join-bad/join`, {
      method: "POST",
      body: "not json",
    });

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });

  it("hands back SFU session and ICE credentials", async () => {
    const body = await joinOk("join-sfu", "ana");

    expect(body.sfu.sessionId).toBe(TEST_SFU.sessionId);
    expect(body.sfu.iceServers).toEqual([
      {
        urls: [TEST_SFU.turnUrl],
        username: "test-user",
        credential: "test-credential",
      },
    ]);
    expect(body.sfu.maxAudioBitrate).toBe(32_000);
    expect(body.sfu.maxVideoBitrate).toBe(2_500_000);
  });

  it("never leaks the app secret to the client", async () => {
    const raw = await (await join("join-secret", "ana")).text();

    expect(raw).not.toContain(TEST_SFU.appSecret);
    expect(raw).not.toContain(TEST_SFU.turnKeyToken);
  });

  it("rejects the ninth participant with 409 room_full", async () => {
    for (let i = 0; i < MAX_PARTICIPANTS; i += 1) {
      await joinOk("join-full", `p${i}`);
    }

    const response = await join("join-full", "one-too-many");

    expect(response.status).toBe(409);
    expect(await response.json()).toMatchObject({ error: "room_full" });
  });
});

describe("GET /rooms/:code/ws", () => {
  it("upgrades a joined participant and greets them", async () => {
    const { self } = await joinOk("ws-basic", "ana");

    const response = await openSocket("ws-basic", self);
    expect(response.status).toBe(101);

    const socket = response.webSocket!;
    socket.accept();
    await expect(nextMessage(socket)).resolves.toMatchObject({
      type: "welcome",
      self,
    });
    socket.close();
  });

  it("pushes a roster update when someone else joins", async () => {
    const ana = await joinOk("ws-roster", "ana");
    const socket = (await openSocket("ws-roster", ana.self)).webSocket!;
    socket.accept();
    await nextMessage(socket); // welcome

    const pushed = nextMessage(socket);
    await joinOk("ws-roster", "bruno");

    await expect(pushed).resolves.toMatchObject({
      type: "roster",
      participants: [{ name: "ana" }, { name: "bruno" }],
    });
    socket.close();
  });

  it("rejects an id that never joined", async () => {
    const response = await openSocket("ws-unknown", crypto.randomUUID());

    expect(response.status).toBe(404);
    expect(await response.json()).toMatchObject({
      error: "unknown_participant",
    });
  });

  it("rejects a plain GET with no upgrade header", async () => {
    const { self } = await joinOk("ws-plain", "ana");

    const response = await SELF.fetch(`${BASE}/rooms/ws-plain/ws?p=${self}`);

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });

  it("rejects an upgrade with no participant id", async () => {
    const response = await SELF.fetch(`${BASE}/rooms/ws-noid/ws`, {
      headers: { upgrade: "websocket" },
    });

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });
});
