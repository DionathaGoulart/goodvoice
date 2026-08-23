/* @refresh reload */
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
