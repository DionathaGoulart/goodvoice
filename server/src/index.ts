import * as Sentry from "@sentry/cloudflare";

import type { Env } from "./env";
import { roomCodeSchema } from "./protocol";
import * as report from "./report";

import { Room as PlainRoom } from "./room";

/**
 * The Durable Object wrangler binds, wrapped so that a failure inside the room
 * — an alarm that throws, a WebSocket handler that does — is reported the same
 * way a request is. The name is what `wrangler.toml` names, so the wrapper has
 * to keep it.
 */
export const Room = Sentry.instrumentDurableObjectWithSentry(
  report.objectOptions,
  PlainRoom,
);

/**
 * Self-hosters point clients at arbitrary Worker URLs, so there is no origin
 * worth allowlisting. The API carries no cookies and no credentials.
 */
const CORS_HEADERS: Record<string, string> = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, PUT, OPTIONS",
  "access-control-allow-headers": "content-type",
  "access-control-max-age": "86400",
};

function withCors(response: Response): Response {
  // A 101 is immutable and carries no body a browser could read.
  if (response.status === 101) {
    return response;
  }
  const headers = new Headers(response.headers);
  for (const [key, value] of Object.entries(CORS_HEADERS)) {
    headers.set(key, value);
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

/** Maps the public path to the room's internal one, or null if it is neither. */
function matchRoom(
  pathname: string,
  method: string,
): { code: string; path: string } | null {
  const parts = pathname.split("/").filter(Boolean);
  if (parts.length < 3 || parts[0] !== "rooms") {
    return null;
  }

  const code = roomCodeSchema.safeParse(parts[1]);
  if (!code.success) {
    return null;
  }

  const action = parts[2];

  if (parts.length === 3) {
    if (action === "join" && method === "POST") {
      return { code: code.data, path: "/join" };
    }
    if (action === "ws" && method === "GET") {
      return { code: code.data, path: "/ws" };
    }
    return null;
  }

  // `/rooms/:code/sfu/<operation>`, where the operation is the whole remainder
  // because Realtime spells some of them with a slash (`tracks/new`). Which
  // operations exist, and which method each takes, is the room's call.
  if (action === "sfu") {
    return { code: code.data, path: `/sfu/${parts.slice(3).join("/")}` };
  }
  return null;
}

const NOT_FOUND = () => new Response("not found", { status: 404 });

const handler: ExportedHandler<Env> = {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method === "OPTIONS") {
      return withCors(new Response(null, { status: 204 }));
    }

    if (request.method === "GET" && url.pathname === "/health") {
      return withCors(Response.json({ ok: true }));
    }

    const match = matchRoom(url.pathname, request.method);
    if (!match) {
      return withCors(NOT_FOUND());
    }

    // One Durable Object per room code: the code *is* the address.
    const room = env.ROOM.get(env.ROOM.idFromName(match.code));

    const internal = new URL(request.url);
    internal.pathname = match.path;

    try {
      return withCors(await room.fetch(new Request(internal, request)));
    } catch (error) {
      console.error("room request failed", error);
      // The only place an unexpected failure lands. `Room.fetch` answers every
      // `RoomError` itself and rethrows everything else, so what arrives here
      // is a bug in this Worker or in Realtime — never a client's doing.
      report.unexpected(error, "room request");
      return withCors(new Response("internal error", { status: 500 }));
    }
  },
};

export default Sentry.withSentry(report.options, handler);
