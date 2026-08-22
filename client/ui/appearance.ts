/*
 * Appearance catalogs — the palettes and skins the appearance screen offers,
 * and the only place on the client that knows which of each exist.
 *
 * An id here is two things at once: the `data-theme` / `data-skin` value on
 * <html>, and the value stored locally (theme.ts). goodvoice has no account to
 * sync against, so unlike GoodChat there is no third place to keep in step.
 *
 * The swatch colours point at the same `--palette-*` variables the themes are
 * built from (styles/palettes.css), so a preview can never drift from the
 * palette it previews — and no hex value leaks outside that file
 * (styleguide.md §2.1).
 *
 * Adding a palette: declare it in styles/themes.css and add it here. Adding a
 * skin: declare its `[data-skin='<id>']` block in styles/skins.css and add it
 * here — and if it invents rules an implementer would otherwise guess, it owes
 * a style guide of its own (§5).
 */

export type Mode = "light" | "dark";
/** null is a real choice: "follow the operating system". */
export type ModePreference = Mode | null;

export interface PaletteOption {
  id: string;
  /** Shown in the appearance screen. */
  label: string;
  /** Swatch: page background, accent, foreground. */
  bg: string;
  acc: string;
  fg: string;
}

export const LIGHT_PALETTES: readonly PaletteOption[] = [
  {
    id: "goodvoice-crimson",
    label: "crimson chalk",
    bg: "var(--palette-cream)",
    acc: "var(--palette-crimson)",
    fg: "var(--palette-ink)",
  },
  {
    id: "goodvoice-frost",
    label: "abyss frost",
    bg: "var(--palette-frost-bg)",
    acc: "var(--palette-abyss)",
    fg: "var(--palette-frost-ink)",
  },
  {
    id: "goodvoice-forest",
    label: "forest mist",
    bg: "var(--palette-forest-bg)",
    acc: "var(--palette-forest-green)",
    fg: "var(--palette-forest-ink)",
  },
  {
    id: "goodvoice-sand",
    label: "sand dusk",
    bg: "var(--palette-sand-bg)",
    acc: "var(--palette-copper)",
    fg: "var(--palette-sand-ink)",
  },
] as const;

export const DARK_PALETTES: readonly PaletteOption[] = [
  {
    id: "goodvoice-rose",
    label: "noir rose",
    bg: "var(--palette-noir)",
    acc: "var(--palette-rose)",
    fg: "var(--palette-cream)",
  },
  {
    id: "goodvoice-gold",
    label: "vault gold",
    bg: "var(--palette-graphite)",
    acc: "var(--palette-gold)",
    fg: "var(--palette-silver)",
  },
  {
    id: "goodvoice-ember",
    label: "midnight ember",
    bg: "var(--palette-midnight)",
    acc: "var(--palette-ember)",
    fg: "var(--palette-mint)",
  },
  {
    id: "goodvoice-cyan",
    label: "cyber teal",
    bg: "var(--palette-cyan-bg)",
    acc: "var(--palette-cyan)",
    fg: "var(--palette-cyan-mist)",
  },
  {
    id: "goodvoice-violet",
    label: "velvet purple",
    bg: "var(--palette-violet-bg)",
    acc: "var(--palette-violet)",
    fg: "var(--palette-violet-mist)",
  },
  {
    id: "goodvoice-matrix",
    label: "neon matrix",
    bg: "var(--palette-black)",
    acc: "var(--palette-matrix-green)",
    fg: "var(--palette-matrix-mist)",
  },
] as const;

export interface SkinOption {
  id: string;
  label: string;
  /** One line on what changes — the colours never do. */
  hint: string;
}

export const SKINS: readonly SkinOption[] = [
  { id: "retro", label: "neobrutal", hint: "thick frame, hard shadow" },
  { id: "terminal", label: "terminal", hint: "crt, prompts, phosphor" },
] as const;

export const DEFAULT_LIGHT = "goodvoice-crimson";
export const DEFAULT_DARK = "goodvoice-rose";
export const DEFAULT_SKIN = "retro";

const LIGHT_IDS = LIGHT_PALETTES.map((palette) => palette.id);
const DARK_IDS = DARK_PALETTES.map((palette) => palette.id);
const SKIN_IDS = SKINS.map((skin) => skin.id);

/*
 * Each falls back to the default whenever the stored value is unknown — a
 * palette dropped from the catalog, or a hand-edited storage entry, must not
 * leave the document with a `data-theme` no stylesheet answers to.
 */
export function lightPaletteOr(id: string | null | undefined): string {
  return id && LIGHT_IDS.includes(id) ? id : DEFAULT_LIGHT;
}

export function darkPaletteOr(id: string | null | undefined): string {
  return id && DARK_IDS.includes(id) ? id : DEFAULT_DARK;
}

export function skinOr(id: string | null | undefined): string {
  return id && SKIN_IDS.includes(id) ? id : DEFAULT_SKIN;
}
