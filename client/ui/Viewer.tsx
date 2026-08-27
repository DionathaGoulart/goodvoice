import { createSignal, onCleanup, Show, type Component } from "solid-js";
import { Channel, invoke } from "@tauri-apps/api/core";

import { t } from "./i18n";

/**
 * The screen-share viewer window (plan.md task 5.4).
 *
 * # Where the decoding happens
 *
 * In the webview, by `VideoDecoder`. WebView2 ships WebCodecs and decodes
 * H.264 on the same silicon the sharer encoded it with, so the alternative —
 * decoding in Rust and shipping 8 MB of pixels per frame across the IPC — is
 * both more code and slower. What crosses the boundary is what came off the
 * wire: tens of kilobytes an access unit.
 *
 * # Opting in
 *
 * This window subscribes when it mounts and unsubscribes when it unmounts, and
 * `Call::watch_screen` is the only thing that ever pulls the video track. A
 * client with no viewer open is a client Cloudflare is not sending video to
 * at all — prd.md §3 F3's opt-in, made true by the window's own lifetime
 * rather than by a flag.
 */

/**
 * What the decoder is told it is being given.
 *
 * Constrained Baseline 3.1, which is the profile the encoder is asked for and
 * the `profile-level-id` the SDP offers (`rtc/session.rs`). The level is a
 * floor rather than a promise — `level-asymmetry-allowed=1` means a 1080p
 * share arrives at a higher one — and a decoder that has hardware for baseline
 * has hardware for all of it.
 */
const CODEC = "avc1.42e01f";

/**
 * How long a black window is allowed to be a black window before it says so.
 *
 * Two things look identical for the first second of a viewer's life: waiting
 * for the next keyframe, and nobody sharing anything. This is where they stop
 * being the same thing to a person looking at it.
 */
const PATIENCE_MS = 4_000;

/**
 * What the window has to say for itself, as a state rather than as a sentence.
 *
 * A signal holding finished text is a signal that keeps the language it was
 * written in: this window can be open across a language change (the picker is
 * in the other one, and both are alive at once), and "waiting for a picture…"
 * stored at mount would still be in English afterwards. So what is held is
 * which of four things is true, and the words are read from `t()` where they
 * are drawn.
 */
type Status =
  | { kind: "waiting" }
  | { kind: "nobody" }
  | { kind: "no-decoder" }
  /** Whatever went wrong, in the prose it arrived in. */
  | { kind: "failed"; detail: string };

const Viewer: Component = () => {
  const [status, setStatus] = createSignal<Status | null>({ kind: "waiting" });

  /** One status as the sentence for it, in the language in force right now. */
  const sentence = (current: Status): string => {
    switch (current.kind) {
      case "waiting":
        return t().waitingForPicture;
      case "nobody":
        return t().nobodyIsSharing;
      case "no-decoder":
        return t().noDecoder;
      case "failed":
        return current.detail;
    }
  };
  let canvas: HTMLCanvasElement | undefined;
  let decoder: VideoDecoder | undefined;
  let patience: number | undefined;

  /** Paints one decoded frame, letterboxed into whatever shape the window is. */
  const paint = (frame: VideoFrame) => {
    const target = canvas;
    if (!target) {
      frame.close();
      return;
    }
    // The canvas follows the *picture*, not the window: sizing the backing
    // store to the frame and letting CSS fit it inside the window is what
    // keeps the aspect ratio right through a resize, and through a sharer who
    // changes what they are sharing mid-call.
    if (target.width !== frame.displayWidth) {
      target.width = frame.displayWidth;
    }
    if (target.height !== frame.displayHeight) {
      target.height = frame.displayHeight;
    }
    const context = target.getContext("2d");
    if (context) {
      context.drawImage(frame, 0, 0);
      setStatus(null);
    }
    // Not optional: a `VideoFrame` holds a decoder buffer, and a few
    // unreleased ones stall the decode entirely.
    frame.close();
  };

  const build = () => {
    if (typeof VideoDecoder === "undefined") {
      setStatus({ kind: "no-decoder" });
      return undefined;
    }
    const built = new VideoDecoder({
      output: paint,
      error: (failure) =>
        setStatus({
          kind: "failed",
          detail: t().decoderStopped(failure.message),
        }),
    });
    // No `description`: that is what tells WebCodecs the chunks are Annex B
    // rather than length-prefixed AVCC, and Annex B is what comes off the
    // wire.
    built.configure({ codec: CODEC, optimizeForLatency: true });
    return built;
  };

  /**
   * One message from the client.
   *
   * The first byte is the keyframe flag and the rest is the access unit; an
   * empty message means the share ended. See `watch_screen` in `lib.rs` for
   * why the flag travels in the payload rather than beside it.
   */
  const onMessage = (message: ArrayBuffer) => {
    if (message.byteLength === 0) {
      setStatus({ kind: "nobody" });
      // The next share starts a new sequence with its own parameter sets, and
      // a decoder holding the last one's state would reject it.
      decoder?.close();
      decoder = undefined;
      return;
    }

    const bytes = new Uint8Array(message);
    const key = bytes[0] === 1;
    const unit = bytes.subarray(1);

    if (!decoder) {
      decoder = build();
    }
    if (!decoder || decoder.state !== "configured") {
      return;
    }
    // Everything before the first keyframe is undecodable on its own, and
    // feeding it produces an error rather than a picture.
    if (!key && !started) {
      return;
    }
    started = true;

    decoder.decode(
      new EncodedVideoChunk({
        type: key ? "key" : "delta",
        // The wall clock is close enough: nothing here reorders, and the
        // timestamp is only used to keep frames in the order they arrived.
        timestamp: performance.now() * 1000,
        data: unit,
      }),
    );
  };

  let started = false;

  const frames = new Channel<ArrayBuffer>();
  frames.onmessage = onMessage;

  void invoke("watch_screen", { frames }).catch((failure) => {
    setStatus({ kind: "failed", detail: String(failure) });
  });

  patience = window.setTimeout(() => {
    setStatus((current) => (current === null ? null : { kind: "nobody" }));
  }, PATIENCE_MS);

  onCleanup(() => {
    window.clearTimeout(patience);
    // The subscription is not given up here, and this is the only place it
    // looks like it should be. A window is *destroyed*, not unmounted: closing
    // it takes the webview down with it and none of this runs. So the client
    // watches for the window ending instead (`viewer_closed` in lib.rs), which
    // is the only version of it that is true whichever way the window goes.
    decoder?.close();
    decoder = undefined;
  });

  return (
    <main class="crt viewer">
      <canvas ref={canvas} class="screen" />
      <Show when={status()}>
        {(current) => (
          <p class="screen-status" role="status">
            {sentence(current())}
          </p>
        )}
      </Show>
    </main>
  );
};

export { Viewer };
