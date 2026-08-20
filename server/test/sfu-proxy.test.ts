import { env, runInDurableObject, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import type { Participant } from "../src/protocol";
import { HEARTBEAT_TIMEOUT_MS, type Room } from "../src/room";
import type { SfuCredentials } from "../src/sfu";
import { TEST_SFU, type ProxyEcho } from "./fake-realtime";

/**
 * The `/rooms/:code/sfu/*` proxy: the only way a client can negotiate tracks,
 * because the app secret that signs those calls stays server-side (DR-2).
 *
 * Routing and secret injection are exercised through the real Worker; the
 * authorisation matrix is exercised inside the Durable Object, where a test
 * can hand out session ids of its own choosing.
 */

const BASE = "https://goodvoice.test";

const PUBLISH = {
  sessionDescription: { type: "offer", sdp: "v=0\r\n" },
  tracks: [{ location: "local", mid: "0", trackName: "mic" }],
};

interface JoinResponse {
  self: string;
  participants: Participant[];
  sfu: SfuCredentials;
}

interface EchoResponse {
  echo: ProxyEcho;
}

function room(code: string) {
  return env.ROOM.get(env.ROOM.idFromName(code));
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

function negotiate(
  code: string,
  participant: string,
  operation: string,
  body: unknown,
  method = "POST",
): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/sfu/${operation}?p=${participant}`, {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** Calls the proxy on a Room instance directly, bypassing the Worker router. */
function negotiateInRoom(
  instance: Room,
  participant: string,
  body: unknown,
  operation = "tracks/new",
  method = "POST",
): Promise<Response> {
  return instance.fetch(
    new Request(`https://room/sfu/${operation}?p=${participant}`, {
      method,
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

describe("POST /rooms/:code/sfu/tracks/new", () => {
  it("signs the call and forwards it on the caller's own session", async () => {
    const ana = await join("sfu-publish", "ana");

    const response = await negotiate(
      "sfu-publish",
      ana.self,
      "tracks/new",
      PUBLISH,
    );

    expect(response.status).toBe(200);
    const { echo } = (await response.json()) as EchoResponse;
    expect(echo).toEqual({
      method: "POST",
      session: TEST_SFU.sessionId,
      operation: "tracks/new",
      body: PUBLISH,
    });
  });

  it("never hands the app secret to the client", async () => {
    const ana = await join("sfu-secret", "ana");

    const raw = await (
      await negotiate("sfu-secret", ana.self, "tracks/new", PUBLISH)
    ).text();

    expect(raw).not.toContain(TEST_SFU.appSecret);
  });

  it("answers CORS preflight for the PUT operations", async () => {
    const response = await SELF.fetch(
      `${BASE}/rooms/sfu-cors/sfu/renegotiate`,
      {
        method: "OPTIONS",
      },
    );

    expect(response.status).toBe(204);
    expect(response.headers.get("access-control-allow-methods")).toContain(
      "PUT",
    );
  });
});

describe("SFU proxy authorisation", () => {
  it("lets a participant pull a track from someone else in the room", async () => {
    await runInDurableObject(room("sfu-peer"), async (instance: Room) => {
      const ana = instance.join("ana").self;
      const bruno = instance.join("bruno").self;
      instance.attachSession(ana, "session-ana");
      instance.attachSession(bruno, "session-bruno");

      const response = await negotiateInRoom(instance, ana, {
        tracks: [
          { location: "remote", sessionId: "session-bruno", trackName: "mic" },
        ],
      });

      expect(response.status).toBe(200);
      const { echo } = (await response.json()) as EchoResponse;
      // Signed as ana, pulling bruno: the session in the path is always the
      // caller's, the one in the body is whose media they want.
      expect(echo.session).toBe("session-ana");
    });
  });

  it("rejects a track that names a session outside the room", async () => {
    await runInDurableObject(room("sfu-outsider"), async (instance: Room) => {
      const ana = instance.join("ana").self;
      instance.attachSession(ana, "session-ana");

      const response = await negotiateInRoom(instance, ana, {
        tracks: [
          {
            location: "remote",
            sessionId: "session-stranger",
            trackName: "mic",
          },
        ],
      });

      expect(response.status).toBe(400);
      expect(await response.json()).toMatchObject({ error: "bad_request" });
    });
  });

  it("rejects a participant who never joined", async () => {
    const response = await negotiate(
      "sfu-unknown",
      crypto.randomUUID(),
      "tracks/new",
      PUBLISH,
    );

    expect(response.status).toBe(404);
    expect(await response.json()).toMatchObject({
      error: "unknown_participant",
    });
  });

  it("rejects a call with no participant id", async () => {
    const response = await SELF.fetch(`${BASE}/rooms/sfu-noid/sfu/tracks/new`, {
      method: "POST",
      body: JSON.stringify(PUBLISH),
    });

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });

  it("rejects a participant whose session was never created", async () => {
    await runInDurableObject(
      room("sfu-sessionless"),
      async (instance: Room) => {
        const ana = instance.join("ana").self;

        const response = await negotiateInRoom(instance, ana, PUBLISH);

        expect(response.status).toBe(502);
        expect(await response.json()).toMatchObject({
          error: "sfu_unavailable",
        });
      },
    );
  });
});

describe("SFU proxy surface", () => {
  it("forwards renegotiate and tracks/close as PUT", async () => {
    const ana = await join("sfu-methods", "ana");

    for (const operation of ["renegotiate", "tracks/close"]) {
      const response = await negotiate(
        "sfu-methods",
        ana.self,
        operation,
        { sessionDescription: { type: "answer", sdp: "v=0\r\n" } },
        "PUT",
      );

      expect(response.status).toBe(200);
      const { echo } = (await response.json()) as EchoResponse;
      expect(echo).toMatchObject({ method: "PUT", operation });
    }
  });

  it("refuses an operation that is not on the allowlist", async () => {
    const ana = await join("sfu-allowlist", "ana");

    for (const operation of ["sessions/new", "tracks", "tracks/new/extra"]) {
      const response = await negotiate(
        "sfu-allowlist",
        ana.self,
        operation,
        PUBLISH,
      );

      expect(response.status).toBe(400);
      expect(await response.json()).toMatchObject({ error: "bad_request" });
    }
  });

  it("cannot be walked out of the room with a relative path", async () => {
    const ana = await join("sfu-traversal", "ana");

    // `..` is resolved by the URL parser long before routing, so this asks for
    // `/rooms/sfu-traversal/apps` — a route that does not exist.
    const response = await negotiate(
      "sfu-traversal",
      ana.self,
      "../apps",
      PUBLISH,
    );

    expect(response.status).toBe(404);
  });

  it("refuses the wrong method for an allowed operation", async () => {
    const ana = await join("sfu-method-mismatch", "ana");

    const response = await negotiate(
      "sfu-method-mismatch",
      ana.self,
      "tracks/new",
      PUBLISH,
      "PUT",
    );

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });

  it("refuses a body that is not JSON", async () => {
    const ana = await join("sfu-nonjson", "ana");

    const response = await SELF.fetch(
      `${BASE}/rooms/sfu-nonjson/sfu/tracks/new?p=${ana.self}`,
      { method: "POST", body: "not json" },
    );

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
  });

  it("counts a negotiation as proof of life", async () => {
    await runInDurableObject(room("sfu-liveness"), async (instance: Room) => {
      const ana = instance.join("ana").self;
      instance.attachSession(ana, "session-ana");
      // Backdate them past the timeout: only the proxy call can save them.
      instance.touch(ana, 0);

      await negotiateInRoom(instance, ana, PUBLISH);

      expect(instance.sweep(HEARTBEAT_TIMEOUT_MS + 1)).toBe(0);
      expect(instance.roster()).toHaveLength(1);
    });
  });
});
