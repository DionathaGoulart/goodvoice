/** Bindings declared in `wrangler.toml`, plus the two self-host secrets. */
export interface Env {
  ROOM: DurableObjectNamespace;
  /** Cloudflare Realtime (Calls) app id. Secret. */
  CALLS_APP_ID?: string;
  /** Cloudflare Realtime (Calls) app secret. Secret. */
  CALLS_APP_SECRET?: string;
  /** Opus bitrate cap, bits per second. */
  MAX_AUDIO_BITRATE?: string;
  /** H.264 bitrate cap, bits per second. */
  MAX_VIDEO_BITRATE?: string;
}
