/*
 * Which language this window is in, and where that is remembered.
 *
 * The same shape as theme.ts, for the same reasons: one source of truth in
 * `localStorage` because goodvoice has no account, and a `boot()` that runs
 * before the app mounts rather than inside it — task 4.6 rebuilds this window
 * on every trip back from the tray, so a window that painted English and
 * corrected itself would do it every time, not once at startup.
 *
 * # The one thing this does that theme.ts does not
 *
 * It tells Rust. The tray menu is the whole app while goodvoice is hidden
 * (tray/menu.rs), and it is built before any webview exists — so the client
 * keeps its own copy of the language in `settings.json` beside the server and
 * the window's rectangle (home.rs), and `set_language` is how this window
 * hands it over. Two consequences worth knowing:
 *
 *  - A fresh install's tray is in English until this window has mounted once,
 *    because until then nothing has told the client what the browser thinks.
 *    It is stored on that first push, so it is right from the second run on.
 *  - The picker below changes the tray in the same click it changes the
 *    window. Nothing has to be restarted.
 *
 * # Detection
 *
 * `navigator.language` and nothing cleverer. Any Portuguese — `pt`, `pt-BR`,
 * `pt-PT` — gets the Brazilian catalog, which is the only Portuguese this
 * build has and is much closer to a European Portuguese speaker than English
 * is. Everything else gets English.
 */

import { invoke } from "@tauri-apps/api/core";
import { createSignal } from "solid-js";

import { CATALOG, type Lang, type Strings } from "./strings";

const STORAGE_KEY = "goodvoice.lang";

const isLang = (value: string | null | undefined): value is Lang =>
  value === "en" || value === "pt-BR";

/** What the browser says the person reads, as one of the two we have. */
function detected(): Lang {
  const tag = navigator.language || "";
  return tag.toLowerCase().startsWith("pt") ? "pt-BR" : "en";
}

function readStored(): Lang {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return isLang(raw) ? raw : detected();
  } catch {
    // Storage can be refused outright; the browser's own answer is a fine one.
    return detected();
  }
}

const [lang, setLangSignal] = createSignal<Lang>(readStored());

/** The language in force, as a signal the picker reads. */
export { lang };

/**
 * The strings, as a signal.
 *
 * Called as `t().join` at every use site rather than destructured once, which
 * is what keeps a language change reactive: destructuring reads the catalog at
 * component-setup time and pins that language for the life of the component.
 */
export function t(): Strings {
  return CATALOG[lang()];
}

/**
 * Puts the language on `<html>`.
 *
 * Not decoration: it is what a screen reader picks a voice from, and what
 * hyphenation and quotation marks follow. The attribute is the BCP-47 tag,
 * which is exactly what `Lang` is.
 */
function paint(next: Lang): void {
  document.documentElement.setAttribute("lang", next);
}

/** Hands the client the language, so the tray menu is in it too. */
function tell(next: Lang): void {
  void invoke("set_language", { tag: next }).catch(() => {
    // A tray one language behind is not worth a dialog, and the next change
    // sends it again.
  });
}

/**
 * Applies the stored language before the app mounts, and tells the client.
 *
 * The telling is here rather than only in [`choose`] because of the fresh
 * install: nothing has ever been stored, the browser's own language is the
 * answer, and the client has no way to reach that answer on its own.
 */
export function boot(): void {
  paint(lang());
  tell(lang());
}

/** Switches language: the window now, the tray with it, and the disk. */
export function choose(next: Lang): void {
  setLangSignal(next);
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // Nothing to do: the choice still holds for this window.
  }
  paint(next);
  tell(next);
}
