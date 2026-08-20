import type { Room } from "./room";

/** Bindings declared in `wrangler.toml`, plus the self-host secrets. */
export interface Env {
  ROOM: DurableObjectNamespace<Room>;
  /** Cloudflare Realtime (Calls) app id. Secret. */
  CALLS_APP_ID?: string;
  /** Cloudflare Realtime (Calls) app secret. Secret. */
  CALLS_APP_SECRET?: string;
  /** TURN key id — a separate credential pair from the Calls app. Secret. */
  TURN_KEY_ID?: string;
  /** TURN key API token. Secret. */
  TURN_KEY_API_TOKEN?: string;
  /** Overrides the Realtime API base URL. Only tests and staging set this. */
  CALLS_API_BASE?: string;
  /** Opus bitrate cap, bits per second. */
  MAX_AUDIO_BITRATE?: string;
  /** H.264 bitrate cap, bits per second. */
  MAX_VIDEO_BITRATE?: string;
}
