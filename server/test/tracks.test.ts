import { env, runInDurableObject, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import type { Participant } from "../src/protocol";
import type { Room } from "../src/room";
import type { SfuCredentials } from "../src/sfu";
import { MID_REFUSED, MID_REJECTED, TEST_SFU } from "./fake-realtime";

/**
 * Track signalling for task 2.4: how a participant learns where to pull their
 * roommates' media from.
 *
 * There is no "I published a track" message. The room reads publishes out of
 * the SFU proxy it already signs, so a client cannot announce a track it never
 * published, and cannot publish one and forget to announce it. What peers see
 * is the roster, which carries each participant's session id and live tracks.
 */

const BASE = "https://goodvoice.test";

const OFFER = { sessionDescription: { type: "offer", sdp: "v=0\r\n" } };

interface JoinResponse {
  self: string;
  participants: Participant[];
  sfu: SfuCredentials;
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

function publish(
  code: string,
  participant: string,
  tracks: Record<string, unknown>[],
): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/sfu/tracks/new?p=${participant}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ ...OFFER, tracks }),
  });
}

function close(
  code: string,
  participant: string,
  tracks: Record<string, unknown>[],
): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/sfu/tracks/close?p=${participant}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ tracks, force: false }),
  });
}

const mic = (mid = "0") => [{ location: "local", mid, trackName: "mic" }];
const screen = (mid = "1") => [{ location: "local", mid, trackName: "screen" }];

/** The roster as the room itself holds it, with no extra join to read it. */
async function roster(code: string): Promise<Participant[]> {
  return runInDurableObject(room(code), (instance: Room) => instance.roster());
}

function openSocket(code: string, participant: string): Promise<Response> {
  return SELF.fetch(`${BASE}/rooms/${code}/ws?p=${participant}`, {
    headers: { upgrade: "websocket" },
  });
}

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

/** The next `count` messages, collected from one listener so none is missed. */
function collectMessages(socket: WebSocket, count: number): Promise<unknown[]> {
  return new Promise((resolve, reject) => {
    const seen: unknown[] = [];
    const timer = setTimeout(
      () => reject(new Error(`only saw ${seen.length} of ${count} messages`)),
      2000,
    );
    const onMessage = (event: MessageEvent) => {
      seen.push(JSON.parse(event.data as string));
      if (seen.length === count) {
        clearTimeout(timer);
        socket.removeEventListener("message", onMessage);
        resolve(seen);
      }
    };
    socket.addEventListener("message", onMessage);
  });
}

describe("publishing a track", () => {
  it("puts it on the roster with the kind its name implies", async () => {
    const ana = await join("track-mic", "ana");

    expect((await publish("track-mic", ana.self, mic())).status).toBe(200);

    expect(await roster("track-mic")).toEqual([
      expect.objectContaining({
        name: "ana",
        sessionId: TEST_SFU.sessionId,
        tracks: [{ name: "mic", kind: "audio" }],
      }),
    ]);
  });

  it("gives a joiner their own session id in their roster entry", async () => {
    const ana = await join("track-self", "ana");

    expect(ana.participants[0]).toMatchObject({
      name: "ana",
      sessionId: ana.sfu.sessionId,
    });
  });

  it("pushes the session id to peers as soon as it exists", async () => {
    const ana = await join("track-address", "ana");
    const socket = (await openSocket("track-address", ana.self)).webSocket!;
    socket.accept();
    await nextMessage(socket); // welcome

    // Bruno is announced by `join()` before his session exists, so both pushes
    // land inside the one call: the roster peers end up holding must carry it.
    const pushes = collectMessages(socket, 2);
    await join("track-address", "bruno");

    const [, addressed] = await pushes;
    expect(addressed).toMatchObject({
      type: "roster",
      participants: [
        { name: "ana", sessionId: TEST_SFU.sessionId },
        { name: "bruno", sessionId: TEST_SFU.sessionId },
      ],
    });
    socket.close();
  });

  it("pushes the new track to everyone already in the room", async () => {
    const ana = await join("track-push", "ana");
    const bruno = await join("track-push", "bruno");

    const socket = (await openSocket("track-push", bruno.self)).webSocket!;
    socket.accept();
    await nextMessage(socket); // welcome

    const pushed = nextMessage(socket);
    await publish("track-push", ana.self, mic());

    await expect(pushed).resolves.toMatchObject({
      type: "roster",
      participants: [
        { name: "ana", tracks: [{ name: "mic", kind: "audio" }] },
        { name: "bruno", tracks: [] },
      ],
    });
    socket.close();
  });

  it("tells a late joiner what is already being published", async () => {
    const ana = await join("track-late", "ana");
    await publish("track-late", ana.self, mic());

    const bruno = await join("track-late", "bruno");

    // The join response is enough to start pulling: session id and track name
    // are the whole address.
    expect(bruno.participants[0]).toMatchObject({
      name: "ana",
      sessionId: TEST_SFU.sessionId,
      tracks: [{ name: "mic", kind: "audio" }],
    });
  });

  it("replaces a track republished on another transceiver", async () => {
    const ana = await join("track-replace", "ana");

    await publish("track-replace", ana.self, mic("0"));
    await publish("track-replace", ana.self, mic("4"));

    expect((await roster("track-replace"))[0]?.tracks).toEqual([
      { name: "mic", kind: "audio" },
    ]);
  });

  it("carries mic and screen side by side", async () => {
    const ana = await join("track-both", "ana");

    await publish("track-both", ana.self, [...mic(), ...screen()]);

    expect((await roster("track-both"))[0]?.tracks).toEqual([
      { name: "mic", kind: "audio" },
      { name: "screen", kind: "video" },
    ]);
  });
});

describe("publishes the room refuses", () => {
  it("rejects a track name it has no model for", async () => {
    const ana = await join("track-unknown", "ana");

    const response = await publish("track-unknown", ana.self, [
      { location: "local", mid: "0", trackName: "webcam" },
    ]);

    expect(response.status).toBe(400);
    expect(await response.json()).toMatchObject({ error: "bad_request" });
    expect((await roster("track-unknown"))[0]?.tracks).toEqual([]);
  });

  it("rejects a published track with no mid", async () => {
    const ana = await join("track-nomid", "ana");

    const response = await publish("track-nomid", ana.self, [
      { location: "local", trackName: "mic" },
    ]);

    expect(response.status).toBe(400);
    expect((await roster("track-nomid"))[0]?.tracks).toEqual([]);
  });

  it("rejects a second screen in the room", async () => {
    const ana = await join("track-share", "ana");
    const bruno = await join("track-share", "bruno");
    await publish("track-share", ana.self, screen());

    const response = await publish("track-share", bruno.self, screen("2"));

    expect(response.status).toBe(409);
    expect(await response.json()).toMatchObject({
      error: "already_sharing",
      message: "ana is already sharing",
    });
  });

  it("flips the sharing flag with the screen track", async () => {
    const ana = await join("track-flag", "ana");

    await publish("track-flag", ana.self, screen("1"));
    expect((await roster("track-flag"))[0]?.sharing).toBe(true);

    await close("track-flag", ana.self, [{ mid: "1" }]);
    expect((await roster("track-flag"))[0]?.sharing).toBe(false);
  });

  it("leaves the sharing flag alone on an unrelated negotiation", async () => {
    const ana = await join("track-flagkeep", "ana");

    // The UI announced the intent before the encoder was ready.
    const socket = (await openSocket("track-flagkeep", ana.self)).webSocket!;
    socket.accept();
    await nextMessage(socket); // welcome
    const pushed = nextMessage(socket);
    socket.send(JSON.stringify({ type: "share", sharing: true }));
    await pushed;

    await publish("track-flagkeep", ana.self, mic());

    expect((await roster("track-flagkeep"))[0]?.sharing).toBe(true);
    socket.close();
  });

  it("lets the same participant republish their own screen", async () => {
    const ana = await join("track-reshare", "ana");
    await publish("track-reshare", ana.self, screen("1"));

    expect((await publish("track-reshare", ana.self, screen("3"))).status).toBe(
      200,
    );
    expect((await roster("track-reshare"))[0]?.tracks).toEqual([
      { name: "screen", kind: "video" },
    ]);
  });
});

describe("publishes the SFU refuses", () => {
  it("does not announce a track the SFU rejected on its own", async () => {
    const ana = await join("track-rejected", "ana");

    const response = await publish(
      "track-rejected",
      ana.self,
      mic(MID_REJECTED),
    );

    // The request itself succeeded; the track inside it did not.
    expect(response.status).toBe(200);
    expect((await roster("track-rejected"))[0]?.tracks).toEqual([]);
  });

  it("does not announce anything when the whole request fails", async () => {
    const ana = await join("track-refused", "ana");

    const response = await publish("track-refused", ana.self, mic(MID_REFUSED));

    expect(response.status).toBe(400);
    expect((await roster("track-refused"))[0]?.tracks).toEqual([]);
  });
});

describe("tracks going away", () => {
  it("drops a track the caller closed, by mid", async () => {
    const ana = await join("track-close", "ana");
    await publish("track-close", ana.self, [...mic("0"), ...screen("1")]);

    const response = await close("track-close", ana.self, [{ mid: "0" }]);

    expect(response.status).toBe(200);
    expect((await roster("track-close"))[0]?.tracks).toEqual([
      { name: "screen", kind: "video" },
    ]);
  });

  it("ignores a close for a mid that publishes nothing", async () => {
    const ana = await join("track-closeother", "ana");
    await publish("track-closeother", ana.self, mic("0"));

    // Closing a subscription: the mid is the caller's, but it is not one they
    // published on.
    const response = await close("track-closeother", ana.self, [{ mid: "7" }]);

    expect(response.status).toBe(200);
    expect((await roster("track-closeother"))[0]?.tracks).toEqual([
      { name: "mic", kind: "audio" },
    ]);
  });

  it("takes a participant's tracks with them when they leave", async () => {
    const ana = await join("track-leave", "ana");
    await join("track-leave", "bruno");
    await publish("track-leave", ana.self, mic());

    await runInDurableObject(room("track-leave"), (instance: Room) => {
      instance.leave(ana.self);
      expect(instance.roster().map((p) => p.tracks)).toEqual([[]]);
    });
  });

  it("frees the room's screen slot when the sharer leaves", async () => {
    const ana = await join("track-freeshare", "ana");
    const bruno = await join("track-freeshare", "bruno");
    await publish("track-freeshare", ana.self, screen());

    await runInDurableObject(room("track-freeshare"), (instance: Room) => {
      instance.leave(ana.self);
    });

    expect(
      (await publish("track-freeshare", bruno.self, screen())).status,
    ).toBe(200);
  });
});

describe("renegotiation", () => {
  it("leaves the roster alone", async () => {
    const ana = await join("track-renegotiate", "ana");
    await publish("track-renegotiate", ana.self, mic());

    const response = await SELF.fetch(
      `${BASE}/rooms/track-renegotiate/sfu/renegotiate?p=${ana.self}`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          sessionDescription: { type: "answer", sdp: "v=0\r\n" },
        }),
      },
    );

    expect(response.status).toBe(200);
    expect((await roster("track-renegotiate"))[0]?.tracks).toEqual([
      { name: "mic", kind: "audio" },
    ]);
  });
});
