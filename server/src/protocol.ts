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

/**
 * What a participant may publish. A goodvoice client has exactly two things to
 * send — their microphone, and at most one screen (prd.md §3) — so the track
 * name is a closed vocabulary rather than free text.
 *
 * Pinning it buys the room the track's media kind without parsing SDP, and
 * lets the server reject a publish it has no model for instead of relaying a
 * track no peer knows how to interpret. Widening this (a second monitor, a
 * camera) is a one-line change here plus a client that names the track.
 */
export const TRACK_KINDS = {
  mic: "audio",
  screen: "video",
} as const;

export type TrackName = keyof typeof TRACK_KINDS;
export type TrackKind = (typeof TRACK_KINDS)[TrackName];

export function isTrackName(value: string): value is TrackName {
  return Object.hasOwn(TRACK_KINDS, value);
}

/** One live track, as every other participant sees it. */
export interface PublishedTrack {
  name: TrackName;
  kind: TrackKind;
}

/**
 * Body of a proxied `/rooms/:code/sfu/*` call — but only the parts the room
 * has to police or learn from. A track that names a `sessionId` is pulling
 * someone else's media, and the room checks that someone is in it; a track
 * without one is the caller publishing, and the room records it so the rest of
 * the room can pull it.
 *
 * Everything else is deliberately loose and forwarded untouched: the SDP and
 * the track flags are between the client and the SFU, and mirroring
 * Cloudflare's request model here would mean re-releasing the Worker every
 * time they extend it.
 */
export const sfuProxyBodySchema = z.looseObject({
  tracks: z
    .array(
      z.looseObject({
        location: z.enum(["local", "remote"]).optional(),
        mid: z.string().optional(),
        trackName: z.string().optional(),
        sessionId: z.string().optional(),
      }),
    )
    .optional(),
});

export type SfuProxyBody = z.infer<typeof sfuProxyBodySchema>;

/**
 * The parts of Realtime's answer the room reads back. A `tracks/new` can come
 * back 200 with individual tracks rejected, so success is per track, not per
 * request.
 */
export const sfuProxyResultSchema = z.looseObject({
  tracks: z
    .array(
      z.looseObject({
        trackName: z.string().optional(),
        mid: z.string().optional(),
        error: z.unknown().optional(),
        errorCode: z.unknown().optional(),
      }),
    )
    .optional(),
});

/**
 * A participant as broadcast to everyone in the room.
 *
 * `sessionId` and `tracks` are the pull address for their media: a peer
 * subscribes with `{ location: "remote", sessionId, trackName }` through the
 * SFU proxy. Both are deliberately visible to the room — the proxy already
 * refuses to pull from a session that is not in it, so a roommate's session id
 * grants nothing beyond the media they joined to hear.
 */
export interface Participant {
  id: string;
  name: string;
  joinedAt: number;
  muted: boolean;
  deafened: boolean;
  sharing: boolean;
  /** Their Realtime session; null until the credential exchange lands. */
  sessionId: string | null;
  /** What they are publishing right now. Empty until they offer a track. */
  tracks: PublishedTrack[];
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
