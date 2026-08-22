/* @refresh reload */
import { render } from "solid-js/web";

import { App } from "./App";
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

render(() => <App />, root);
