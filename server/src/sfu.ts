import type { Env } from "./env";
import { RoomError } from "./protocol";

/**
 * Cloudflare Realtime (Calls) credential exchange, and the narrow proxy the
 * client negotiates tracks through.
 *
 * The app secret and the TURN key never leave the Worker: the client receives
 * a session id and short-lived ICE credentials, nothing else. Track
 * negotiation needs that same secret, so it cannot happen client-side either —
 * see DR-2 in .harness/plan.md.
 */

const DEFAULT_API_BASE = "https://rtc.live.cloudflare.com/v1";

/** Long enough to outlive any realistic session, per Cloudflare's guidance. */
const TURN_TTL_SECONDS = 86400;

/** Used when the self-hoster has not configured a TURN key. */
const STUN_ONLY: IceServer[] = [
  { urls: ["stun:stun.cloudflare.com:3478", "stun:stun.cloudflare.com:53"] },
];

const DEFAULT_AUDIO_BITRATE = 32_000;
const DEFAULT_VIDEO_BITRATE = 2_500_000;

export interface IceServer {
  urls: string[] | string;
  username?: string;
  credential?: string;
}

/** Everything a client needs to reach the SFU, and nothing more. */
export interface SfuCredentials {
  sessionId: string;
  iceServers: IceServer[];
  maxAudioBitrate: number;
  maxVideoBitrate: number;
}

/** Injectable for tests; production always uses the global `fetch`. */
export type FetchLike = typeof fetch;

function bitrate(raw: string | undefined, fallback: number): number {
  const parsed = Number(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/**
 * Creates a Realtime session and pairs it with fresh ICE servers.
 *
 * @throws {RoomError} `sfu_unavailable` when the app is unconfigured or the
 * Realtime API refuses the request — there is no voice without a session.
 */
export async function createSfuSession(
  env: Env,
  fetchImpl: FetchLike = fetch,
): Promise<SfuCredentials> {
  const appId = env.CALLS_APP_ID;
  const appSecret = env.CALLS_APP_SECRET;
  if (!appId || !appSecret) {
    throw new RoomError(
      "sfu_unavailable",
      "CALLS_APP_ID and CALLS_APP_SECRET are not configured",
    );
  }

  const base = env.CALLS_API_BASE ?? DEFAULT_API_BASE;
  const response = await fetchImpl(`${base}/apps/${appId}/sessions/new`, {
    method: "POST",
    headers: { authorization: `Bearer ${appSecret}` },
  }).catch(() => null);

  if (!response?.ok) {
    throw new RoomError(
      "sfu_unavailable",
      `Realtime refused the session (${response?.status ?? "network error"})`,
    );
  }

  const body = (await response.json().catch(() => null)) as {
    sessionId?: unknown;
  } | null;

  if (typeof body?.sessionId !== "string") {
    throw new RoomError("sfu_unavailable", "Realtime returned no sessionId");
  }

  return {
    sessionId: body.sessionId,
    iceServers: await iceServers(env, fetchImpl),
    maxAudioBitrate: bitrate(env.MAX_AUDIO_BITRATE, DEFAULT_AUDIO_BITRATE),
    maxVideoBitrate: bitrate(env.MAX_VIDEO_BITRATE, DEFAULT_VIDEO_BITRATE),
  };
}

/**
 * Short-lived TURN credentials, or plain STUN when TURN is unconfigured or
 * unreachable. A failure here degrades NAT traversal; it must not drop a call
 * that would have connected over STUN anyway.
 */
async function iceServers(
  env: Env,
  fetchImpl: FetchLike,
): Promise<IceServer[]> {
  const keyId = env.TURN_KEY_ID;
  const keyToken = env.TURN_KEY_API_TOKEN;
  if (!keyId || !keyToken) {
    return STUN_ONLY;
  }

  const base = env.CALLS_API_BASE ?? DEFAULT_API_BASE;
  const response = await fetchImpl(
    `${base}/turn/keys/${keyId}/credentials/generate-ice-servers`,
    {
      method: "POST",
      headers: {
        authorization: `Bearer ${keyToken}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ ttl: TURN_TTL_SECONDS }),
    },
  ).catch(() => null);

  if (!response?.ok) {
    console.warn("TURN credential generation failed; falling back to STUN");
    return STUN_ONLY;
  }

  const body = (await response.json().catch(() => null)) as {
    iceServers?: unknown;
  } | null;

  // Cloudflare answers with a single object today; accept both shapes.
  if (Array.isArray(body?.iceServers)) {
    return body.iceServers as IceServer[];
  }
  if (body?.iceServers && typeof body.iceServers === "object") {
    return [body.iceServers as IceServer];
  }

  console.warn("TURN response had no iceServers; falling back to STUN");
  return STUN_ONLY;
}

/**
 * The only Realtime endpoints reachable through the Worker, each pinned to the
 * one method it accepts. An allowlist rather than a passthrough: the proxy
 * signs every request with the app secret, so whatever it forwards is
 * effectively performed by the account owner.
 */
const PROXIED_OPERATIONS = {
  "tracks/new": "POST",
  renegotiate: "PUT",
  "tracks/close": "PUT",
} as const;

export type SfuOperation = keyof typeof PROXIED_OPERATIONS;

export function isSfuOperation(value: string): value is SfuOperation {
  return Object.hasOwn(PROXIED_OPERATIONS, value);
}

/** The HTTP method {@link proxySfuRequest} will use for an operation. */
export function sfuOperationMethod(operation: SfuOperation): string {
  return PROXIED_OPERATIONS[operation];
}

/**
 * Forwards one track-negotiation call to the Realtime API with the app secret
 * attached, and hands the answer back untouched.
 *
 * The caller is responsible for deciding that `sessionId` belongs to whoever
 * is asking — this function authenticates to Cloudflare, it does not authorise
 * the client.
 *
 * @throws {RoomError} `sfu_unavailable` when the app is unconfigured or the
 * API cannot be reached at all.
 */
export async function proxySfuRequest(
  env: Env,
  sessionId: string,
  operation: SfuOperation,
  body: string,
  fetchImpl: FetchLike = fetch,
): Promise<Response> {
  const appId = env.CALLS_APP_ID;
  const appSecret = env.CALLS_APP_SECRET;
  if (!appId || !appSecret) {
    throw new RoomError(
      "sfu_unavailable",
      "CALLS_APP_ID and CALLS_APP_SECRET are not configured",
    );
  }

  const base = env.CALLS_API_BASE ?? DEFAULT_API_BASE;
  const response = await fetchImpl(
    `${base}/apps/${appId}/sessions/${sessionId}/${operation}`,
    {
      method: PROXIED_OPERATIONS[operation],
      headers: {
        authorization: `Bearer ${appSecret}`,
        "content-type": "application/json",
      },
      body,
    },
  ).catch(() => null);

  if (!response) {
    throw new RoomError("sfu_unavailable", "Realtime is unreachable");
  }

  // The SFU's verdict is passed through as-is: a 400 on a malformed SDP is the
  // client's problem to fix, and repackaging it as a 502 would erase the
  // reason. Only the headers are dropped, so nothing we sent comes back out.
  return new Response(response.body, {
    status: response.status,
    headers: {
      "content-type":
        response.headers.get("content-type") ?? "application/json",
    },
  });
}

export { PROXIED_OPERATIONS, STUN_ONLY, TURN_TTL_SECONDS };
