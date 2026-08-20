import { z } from "zod";

/**
 * Every byte the client sends crosses this file first. The Durable Object
 * trusts nothing (styleguide.md, TypeScript conventions).
 */

/** Room codes are short, shareable and case-insensitive. */
export const roomCodeSchema = z
  .string()
  .trim()
  .min(4)
  .max(24)
  .regex(/^[a-zA-Z0-9-]+$/, "room code must be alphanumeric or hyphens")
  .transform((code) => code.toLowerCase());

export const displayNameSchema = z.string().trim().min(1).max(32);

/** Body of `POST /rooms/:code/join`. */
export const joinRequestSchema = z.object({
  name: displayNameSchema,
});

export type JoinRequest = z.infer<typeof joinRequestSchema>;

/** A participant as broadcast to everyone in the room. */
export interface Participant {
  id: string;
  name: string;
  joinedAt: number;
  muted: boolean;
  deafened: boolean;
  sharing: boolean;
}

/** Messages the client may send over the room WebSocket. */
export const clientMessageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("heartbeat") }),
  z.object({ type: z.literal("mute"), muted: z.boolean() }),
  z.object({ type: z.literal("deafen"), deafened: z.boolean() }),
  z.object({ type: z.literal("share"), sharing: z.boolean() }),
  z.object({ type: z.literal("leave") }),
]);

export type ClientMessage = z.infer<typeof clientMessageSchema>;

/** Messages the room pushes down the WebSocket. */
export type ServerMessage =
  | { type: "welcome"; self: string; participants: Participant[] }
  | { type: "roster"; participants: Participant[] }
  | { type: "error"; code: RoomErrorCode; message: string };

/** Typed failures. The client switches on `code`, never on prose. */
export const ROOM_ERROR_CODES = [
  "room_full",
  "bad_request",
  "unknown_participant",
  "already_sharing",
  "sfu_unavailable",
] as const;

export type RoomErrorCode = (typeof ROOM_ERROR_CODES)[number];

/** HTTP status that best matches each failure. */
const ERROR_STATUS: Record<RoomErrorCode, number> = {
  room_full: 409,
  bad_request: 400,
  unknown_participant: 404,
  already_sharing: 409,
  sfu_unavailable: 502,
};

export class RoomError extends Error {
  readonly code: RoomErrorCode;

  constructor(code: RoomErrorCode, message: string) {
    super(message);
    this.name = "RoomError";
    this.code = code;
  }

  get status(): number {
    return ERROR_STATUS[this.code];
  }

  toResponse(): Response {
    return Response.json(
      { error: this.code, message: this.message },
      { status: this.status },
    );
  }
}

/** Hard cap from prd.md §3 F1, enforced here and nowhere else. */
export const MAX_PARTICIPANTS = 8;
