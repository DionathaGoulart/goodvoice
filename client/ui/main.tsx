/* @refresh reload */
import { invoke } from "@tauri-apps/api/core";
import { render } from "solid-js/web";

import { App } from "./App";
import { Viewer } from "./Viewer";
import { boot } from "./theme";
import "./styles/app.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

/*
 * Before the first render, not inside it: the stored palette has to be on
 * <html> when the window paints, or the default shows for a frame first. Task
 * 4.6 destroys and rebuilds this window on every trip back from the tray, so
 * that frame would not be a one-off at startup — it would be every time.
 */
boot();

/*
 * One bundle, two windows. The viewer (plan.md task 5.4) is a second Tauri
 * window pointed at the same `index.html` with `#screen` on it — a fragment
 * rather than a query so the dev server and the embedded protocol resolve it
 * the same way. `open_screen_viewer` in `lib.rs` is what builds it.
 */
render(() => (location.hash === "#screen" ? <Viewer /> : <App />), root);

/*
 * The window is built hidden and this is what shows it (DR-38, `lib.rs`).
 *
 * A rebuilt WebView2 paints **white for 394 ms** before the page reaches the
 * screen — measured frame by frame by `docs/testing/tray-flicker.ps1` — and on
 * a dark palette that is a full-window white flash, every trip back from the
 * tray (task 4.6). A window nobody can see cannot flash.
 *
 * Two frames, not one: a `requestAnimationFrame` callback runs *before* the
 * paint it was queued for, so the first is the frame being composed and the
 * second is the earliest tick after it is on the webview's own surface.
 *
 * Only the main window. The viewer (task 5.4) is built visible by
 * `open_screen_viewer`, and calling this on it would be a second window asking
 * for focus while somebody is watching a screen.
 *
 * Nothing here is load-bearing for the window existing: `lib.rs` shows it
 * anyway after a grace period, because a promise the webview cannot keep must
 * not cost a person their window.
 */
if (location.hash !== "#screen") {
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      void invoke("window_painted").catch(() => {
        /* The grace timer in `lib.rs` answers this; a retry would not. */
      });
    });
  });
}
