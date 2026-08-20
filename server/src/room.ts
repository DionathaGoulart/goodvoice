import { DurableObject } from "cloudflare:workers";

import type { Env } from "./env";

/**
 * One Durable Object per room code. State is in-memory only: rooms are
 * ephemeral and the storage API is never touched (prd.md §7).
 *
 * Roster, cap enforcement and SFU credential exchange land in plan.md 1.1–1.4.
 */
export class Room extends DurableObject<Env> {
  override fetch(_request: Request): Response {
    return new Response("not implemented", { status: 501 });
  }
}
