/*
 * What this window says out loud when a command it asked for does not work.
 *
 * # The failures that had nowhere to go
 *
 * Every one of the client's commands returns a `Result`, and this window
 * handles every rejection by showing something and carrying on. That is right
 * — somebody mid-call should not lose the window because a tray icon refused
 * — and it is also why none of those failures reached anybody: nothing throws
 * past the handler, so nothing catches it. A crash reporter watching for
 * uncaught errors sees a window where nothing ever goes wrong.
 *
 * # Why it goes through Rust
 *
 * Not because the browser SDK is missing — the Sentry plugin injects one into
 * every webview — but because the injected one only exists when this build has
 * a DSN *and* somebody consented, and the rotating log exists either way. Rust
 * takes both: `report::failure` writes the line to the log first and reports it
 * second. A tester with no network and no Sentry project still ends up with a
 * file that says what happened.
 */

import { invoke } from "@tauri-apps/api/core";

/**
 * Hands a failure to the client.
 *
 * Never throws and never awaits: a report is not worth delaying, or breaking,
 * whatever the caller was doing when it failed. If reporting the failure fails
 * there is nothing sensible left to try.
 */
export function report(where: string, detail: unknown): void {
  void invoke("report_failure", {
    where_: where,
    detail: detail instanceof Error ? detail.message : String(detail),
  }).catch(() => {});
}

/**
 * `invoke`, with the rejection reported before it is rethrown.
 *
 * The caller still sees the rejection and still decides what the window does
 * about it. All this adds is that somebody finds out.
 */
export async function called<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (reason) {
    report(command, reason);
    throw reason;
  }
}
