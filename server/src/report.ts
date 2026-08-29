/*
 * What the Worker reports, and — the part that took the thinking — what it
 * deliberately does not.
 *
 * # Why nothing would be reported without this file
 *
 * `withSentry` captures what escapes a handler, and nothing escapes this one.
 * `Room.fetch` turns every `RoomError` into a response and rethrows the rest;
 * `index.ts` then catches *that* and answers 500. By the time a request
 * finishes, every failure has been handled, so the automatic capture sees an
 * empty room. The two `unexpected` calls below are where the errors actually
 * are.
 *
 * # The line between a bug and the product working
 *
 * A room code that does not exist, a name that is too long, a room that is
 * full: those are `RoomError`s, they already answer with the right status, and
 * a person typing the wrong code is not an incident. Reporting them would
 * spend a free-tier quota that is counted per organisation — the same 5,000 the
 * client's crashes come out of — on the client behaving exactly as designed.
 *
 * `sfu_unavailable` is the exception and is reported: it means Cloudflare
 * Realtime is unreachable or this deploy is missing its credentials, which is
 * never something a client did.
 */

import * as Sentry from "@sentry/cloudflare";
import type { CloudflareOptions } from "@sentry/cloudflare";

import type { Env } from "./env";
import { RoomError } from "./protocol";

/**
 * How the SDK is set up, or `undefined` where it should stay switched off.
 *
 * A deploy with no `SENTRY_DSN` — every self-host that has not asked for this,
 * and `wrangler dev` — gets `undefined` and the SDK never initialises. That is
 * the default, and it is why this needs no opt-out.
 */
export function options(env: Env): CloudflareOptions | undefined {
  if (!env.SENTRY_DSN) {
    return undefined;
  }
  return {
    dsn: env.SENTRY_DSN,
    // Errors are the whole ask. Tracing on a Worker that already reports its
    // own timings through Workers Logs would buy a second copy of what the
    // dashboard has, out of a quota that is shared with the client's crashes.
    tracesSampleRate: 0,
    sendDefaultPii: false,
  };
}

/**
 * The same options where a `CloudflareOptions` is required rather than
 * optional — the Durable Object wrapper takes no `undefined`.
 *
 * A `dsn` of `undefined` is the SDK's own way of staying inert, so this is the
 * off switch too, spelled the way that call site accepts.
 */
export function objectOptions(env: Env): CloudflareOptions {
  return options(env) ?? { dsn: undefined };
}

/**
 * Reports a failure that no client could have caused.
 *
 * `where` names the call site rather than being derived from the stack: a
 * minified Worker bundle gives frames that change every deploy, and two
 * hand-written strings group better than any fingerprint inferred from them.
 */
export function unexpected(error: unknown, where: string): void {
  // A `RoomError` that reaches here is one of the handful worth reporting —
  // the caller has already decided. Everything else arrives as itself.
  Sentry.withScope((scope) => {
    scope.setTag("where", where);
    if (error instanceof RoomError) {
      scope.setTag("room_error", error.code);
    }
    Sentry.captureException(error);
  });
}

/**
 * Whether a `RoomError` is worth an issue.
 *
 * Only `sfu_unavailable`, for the reason in this file's header: the other four
 * codes are all a client being told no.
 */
export function isWorthReporting(error: unknown): boolean {
  return error instanceof RoomError && error.code === "sfu_unavailable";
}
