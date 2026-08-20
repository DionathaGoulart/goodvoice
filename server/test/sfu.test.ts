import { describe, expect, it, vi } from "vitest";

import type { Env } from "../src/env";
import { RoomError } from "../src/protocol";
import { createSfuSession, STUN_ONLY, TURN_TTL_SECONDS } from "../src/sfu";

/**
 * These are unit tests of the credential exchange itself: every case injects
 * its own `fetch`, so no network and no Miniflare outbound service is involved.
 */

const CONFIGURED = {
  CALLS_APP_ID: "app-1",
  CALLS_APP_SECRET: "secret-1",
  TURN_KEY_ID: "turn-1",
  TURN_KEY_API_TOKEN: "turn-token-1",
} as Env;

function responder(
  routes: Record<string, () => Response>,
): typeof fetch & { calls: Request[] } {
  const calls: Request[] = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = new Request(input as RequestInfo, init);
    calls.push(request);
    const path = new URL(request.url).pathname;
    const route = routes[path];
    return route ? route() : new Response("no route", { status: 404 });
  }) as typeof fetch & { calls: Request[] };
  impl.calls = calls;
  return impl;
}

const SESSION_PATH = "/v1/apps/app-1/sessions/new";
const TURN_PATH = "/v1/turn/keys/turn-1/credentials/generate-ice-servers";

const okSession = () => Response.json({ sessionId: "sess-1" });
const okTurn = () =>
  Response.json({
    iceServers: { urls: ["turn:example:3478"], username: "u", credential: "c" },
  });

describe("createSfuSession", () => {
  it("returns the session id and TURN credentials", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: okTurn,
    });

    const credentials = await createSfuSession(CONFIGURED, fetchImpl);

    expect(credentials.sessionId).toBe("sess-1");
    expect(credentials.iceServers).toEqual([
      { urls: ["turn:example:3478"], username: "u", credential: "c" },
    ]);
  });

  it("authenticates with the app secret and the TURN token separately", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: okTurn,
    });

    await createSfuSession(CONFIGURED, fetchImpl);

    const [session, turn] = fetchImpl.calls;
    expect(session?.headers.get("authorization")).toBe("Bearer secret-1");
    expect(session?.method).toBe("POST");
    expect(turn?.headers.get("authorization")).toBe("Bearer turn-token-1");
    expect(await turn?.json()).toEqual({ ttl: TURN_TTL_SECONDS });
  });

  it("accepts an iceServers array as well as a single object", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: () =>
        Response.json({ iceServers: [{ urls: "turn:a" }, { urls: "turn:b" }] }),
    });

    const credentials = await createSfuSession(CONFIGURED, fetchImpl);

    expect(credentials.iceServers).toHaveLength(2);
  });

  it("reads the bitrate caps from the environment", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: okTurn,
    });

    const credentials = await createSfuSession(
      {
        ...CONFIGURED,
        MAX_AUDIO_BITRATE: "24000",
        MAX_VIDEO_BITRATE: "900000",
      },
      fetchImpl,
    );

    expect(credentials.maxAudioBitrate).toBe(24_000);
    expect(credentials.maxVideoBitrate).toBe(900_000);
  });

  it("falls back to defaults when the caps are missing or junk", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: okTurn,
    });

    const credentials = await createSfuSession(
      { ...CONFIGURED, MAX_AUDIO_BITRATE: "not-a-number" },
      fetchImpl,
    );

    expect(credentials.maxAudioBitrate).toBe(32_000);
    expect(credentials.maxVideoBitrate).toBe(2_500_000);
  });
});

describe("createSfuSession without TURN", () => {
  it("uses plain STUN when no TURN key is configured", async () => {
    const fetchImpl = responder({ [SESSION_PATH]: okSession });

    const credentials = await createSfuSession(
      {
        CALLS_APP_ID: "app-1",
        CALLS_APP_SECRET: "secret-1",
      } as Env,
      fetchImpl,
    );

    expect(credentials.iceServers).toEqual(STUN_ONLY);
    expect(fetchImpl.calls).toHaveLength(1);
  });

  it("degrades to STUN rather than failing when TURN errors", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fetchImpl = responder({
      [SESSION_PATH]: okSession,
      [TURN_PATH]: () => new Response("nope", { status: 500 }),
    });

    const credentials = await createSfuSession(CONFIGURED, fetchImpl);

    expect(credentials.sessionId).toBe("sess-1");
    expect(credentials.iceServers).toEqual(STUN_ONLY);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});

describe("createSfuSession failures", () => {
  it("refuses to run unconfigured", async () => {
    const fetchImpl = responder({});

    await expect(createSfuSession({} as Env, fetchImpl)).rejects.toMatchObject({
      code: "sfu_unavailable",
    });
    expect(fetchImpl.calls).toHaveLength(0);
  });

  it("surfaces a rejected session as sfu_unavailable", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: () => new Response("denied", { status: 403 }),
    });

    const error = await createSfuSession(CONFIGURED, fetchImpl).catch(
      (e: unknown) => e,
    );

    expect(error).toBeInstanceOf(RoomError);
    expect((error as RoomError).status).toBe(502);
    expect((error as RoomError).message).toContain("403");
  });

  it("surfaces a network failure as sfu_unavailable", async () => {
    const fetchImpl = (() =>
      Promise.reject(new Error("connection reset"))) as typeof fetch;

    await expect(createSfuSession(CONFIGURED, fetchImpl)).rejects.toMatchObject(
      { code: "sfu_unavailable" },
    );
  });

  it("rejects a session response with no sessionId", async () => {
    const fetchImpl = responder({
      [SESSION_PATH]: () => Response.json({ nope: true }),
    });

    await expect(createSfuSession(CONFIGURED, fetchImpl)).rejects.toMatchObject(
      { code: "sfu_unavailable" },
    );
  });
});
