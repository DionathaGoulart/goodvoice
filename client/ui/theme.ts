/*
 * Appearance state: a mode (light / dark / follow the system), which palette
 * each mode uses, and which skin paints them.
 *
 * Two differences from GoodChat's version of this, both because goodvoice has
 * no account:
 *
 *  - There is one source of truth, `localStorage`, rather than an account
 *    preference with a local copy in front of it to stop the first paint
 *    flashing. Nothing is ever in flight, so nothing has to be raced.
 *  - `boot()` runs before the app mounts, from main.tsx, for the same reason
 *    GoodChat's does: a webview that paints the default and then corrects
 *    itself is a flash, and task 4.6 rebuilds this window on every trip back
 *    from the tray — so it would be a flash the user sees repeatedly.
 *
 * "System" cannot mean "pin nothing and let the stylesheet decide", because the
 * OS only says light or dark and does not know which of the four light palettes
 * was picked. So the attribute is always pinned, and in system mode a
 * matchMedia listener repins it when the OS flips.
 */

import { createSignal } from "solid-js";
import {
  DEFAULT_DARK,
  DEFAULT_LIGHT,
  DEFAULT_SKIN,
  darkPaletteOr,
  lightPaletteOr,
  skinOr,
  type Mode,
  type ModePreference,
  type PaletteId,
  type SkinId,
} from "./appearance";

export interface ThemePrefs {
  mode: ModePreference;
  light: PaletteId;
  dark: PaletteId;
  /** Skin id — the geometry the palette is painted on. */
  skin: SkinId;
}

const STORAGE_KEY = "goodvoice-theme";

export const DEFAULT_PREFS: ThemePrefs = {
  mode: null,
  light: DEFAULT_LIGHT,
  dark: DEFAULT_DARK,
  skin: DEFAULT_SKIN,
};

function systemMode(): Mode {
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function readStored(): ThemePrefs {
  let raw: string | null = null;
  try {
    raw = localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage can be refused outright; the defaults are a fine answer.
    return DEFAULT_PREFS;
  }
  if (!raw) return DEFAULT_PREFS;

  try {
    const parsed = JSON.parse(raw) as Partial<ThemePrefs>;
    return {
      mode:
        parsed.mode === "light" || parsed.mode === "dark" ? parsed.mode : null,
      light: lightPaletteOr(parsed.light),
      dark: darkPaletteOr(parsed.dark),
      skin: skinOr(parsed.skin),
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

const [prefs, setPrefs] = createSignal<ThemePrefs>(readStored());

/** Current preferences, as a signal the appearance screen reads. */
export { prefs };

/** Effective mode right now, with "system" already resolved. */
export function currentMode(): Mode {
  return prefs().mode ?? systemMode();
}

/** The theme id the current preferences resolve to. */
export function currentTheme(): PaletteId {
  const current = prefs();
  return (current.mode ?? systemMode()) === "dark"
    ? current.dark
    : current.light;
}

function paint(next: ThemePrefs): void {
  const theme = (next.mode ?? systemMode()) === "dark" ? next.dark : next.light;
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.setAttribute("data-skin", next.skin);
}

/** Applies the stored preferences before the app mounts. */
export function boot(): void {
  paint(prefs());
  // In system mode the OS is the input, so keep following it.
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (prefs().mode === null) paint(prefs());
    });
}

/** Pins the preferences on <html> and writes them down. */
export function apply(next: ThemePrefs): void {
  const cleaned: ThemePrefs = {
    mode: next.mode,
    light: lightPaletteOr(next.light),
    dark: darkPaletteOr(next.dark),
    skin: skinOr(next.skin),
  };
  setPrefs(cleaned);
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cleaned));
  } catch {
    // Nothing to do: the choice still holds for this window.
  }
  paint(cleaned);
}

/** Sets one field and leaves the rest alone — what every control here does. */
export function update(patch: Partial<ThemePrefs>): void {
  apply({ ...prefs(), ...patch });
}

/**
 * Picks a palette for whichever mode is showing. The appearance screen only
 * ever offers the palettes of the current mode, so a click means "this one,
 * for this mode" and never silently rewrites the other.
 */
export function pickPalette(id: PaletteId): void {
  update(currentMode() === "dark" ? { dark: id } : { light: id });
}
