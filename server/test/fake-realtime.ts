/**
 * A stand-in for the Cloudflare Realtime API, wired up as Miniflare's outbound
 * service in `vitest.config.ts`. It answers the two endpoints the Worker calls
 * and rejects everything else loudly, so an unmocked call cannot slip through.
 */

export const TEST_SFU = {
  appId: "test-app",
  appSecret: "test-app-secret",
  turnKeyId: "test-turn-key",
  turnKeyToken: "test-turn-token",
  sessionId: "test-session-id",
  turnUrl: "turn:turn.cloudflare.test:3478",
} as const;

export function fakeRealtimeApi(request: Request): Response {
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

  return new Response(`unexpected outbound request: ${request.url}`, {
    status: 502,
  });
}
