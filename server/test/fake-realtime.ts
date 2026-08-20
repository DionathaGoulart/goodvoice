/**
 * A stand-in for the Cloudflare Realtime API, wired up as Miniflare's outbound
 * service in `vitest.config.ts`. It answers the endpoints the Worker calls and
 * rejects everything else loudly, so an unmocked call cannot slip through.
 */

export const TEST_SFU = {
  appId: "test-app",
  appSecret: "test-app-secret",
  turnKeyId: "test-turn-key",
  turnKeyToken: "test-turn-token",
  sessionId: "test-session-id",
  turnUrl: "turn:turn.cloudflare.test:3478",
} as const;

/** What the proxied track endpoints echo back, so tests can inspect the hop. */
export interface ProxyEcho {
  method: string;
  session: string;
  operation: string;
  body: unknown;
}

/** A track the fake accepts the request for but refuses individually. */
export const MID_REJECTED = "rejected";

/** A track whose presence makes the fake refuse the whole request. */
export const MID_REFUSED = "refused";

const PROXIED_PATH = /^\/v1\/apps\/([^/]+)\/sessions\/([^/]+)\/(.+)$/;

const PROXIED_OPERATIONS = new Set([
  "tracks/new",
  "renegotiate",
  "tracks/close",
]);

export async function fakeRealtimeApi(request: Request): Promise<Response> {
  const url = new URL(request.url);
  const auth = request.headers.get("authorization");

  if (url.pathname === `/v1/apps/${TEST_SFU.appId}/sessions/new`) {
    if (auth !== `Bearer ${TEST_SFU.appSecret}`) {
      return new Response("unauthorized", { status: 401 });
    }
    return Response.json({ sessionId: TEST_SFU.sessionId }, { status: 201 });
  }

  if (
    url.pathname ===
    `/v1/turn/keys/${TEST_SFU.turnKeyId}/credentials/generate-ice-servers`
  ) {
    if (auth !== `Bearer ${TEST_SFU.turnKeyToken}`) {
      return new Response("unauthorized", { status: 401 });
    }
    return Response.json(
      {
        iceServers: {
          urls: [TEST_SFU.turnUrl],
          username: "test-user",
          credential: "test-credential",
        },
      },
      { status: 201 },
    );
  }

  // Track negotiation: echo the hop back so a test can assert what the Worker
  // signed and forwarded, without the real API's SDP machinery.
  const proxied = PROXIED_PATH.exec(url.pathname);
  if (proxied) {
    const [, appId, session, operation] = proxied as unknown as [
      string,
      string,
      string,
      string,
    ];
    if (appId !== TEST_SFU.appId) {
      return new Response("unknown app", { status: 404 });
    }
    if (auth !== `Bearer ${TEST_SFU.appSecret}`) {
      return new Response("unauthorized", { status: 401 });
    }
    if (!PROXIED_OPERATIONS.has(operation)) {
      return new Response("unknown operation", { status: 404 });
    }

    const body = (await request.json().catch(() => null)) as {
      tracks?: { mid?: string; trackName?: string }[];
    } | null;

    const echo: ProxyEcho = {
      method: request.method,
      session,
      operation,
      body,
    };

    // Two mids stand in for the failure modes the room has to survive without
    // a real SDP exchange: MID_REJECTED is a track the SFU turns down inside an
    // otherwise successful answer, MID_REFUSED sinks the whole request.
    const requested = body?.tracks ?? [];
    if (requested.some((track) => track.mid === MID_REFUSED)) {
      return Response.json(
        { errorCode: "invalid_offer", errorDescription: "no" },
        { status: 400 },
      );
    }

    return Response.json({
      echo,
      tracks: requested.map((track) => ({
        mid: track.mid,
        trackName: track.trackName,
        ...(track.mid === MID_REJECTED
          ? { error: { errorCode: "track_rejected" } }
          : {}),
      })),
    });
  }

  return new Response(`unexpected outbound request: ${request.url}`, {
    status: 502,
  });
}
