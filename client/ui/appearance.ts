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
 * **What is not here any more is the words.** A palette's name and a skin's
 * one-line hint are shown to a person, and this client speaks two languages
 * (i18n.ts), so they live in strings.ts keyed by the ids below. That is what
 * `PaletteId` and `SkinId` are for: adding a palette without naming it in both
 * languages is a type error rather than a blank swatch.
 *
 * Adding a palette: declare it in styles/themes.css, add it here, and name it
 * in strings.ts. Adding a skin: declare its `[data-skin='<id>']` block in
 * styles/skins.css, add it here, name it in strings.ts — and if it invents
 * rules an implementer would otherwise guess, it owes a style guide of its
 * own (§5).
 */

export type Mode = "light" | "dark";
/** null is a real choice: "follow the operating system". */
export type ModePreference = Mode | null;

/**
 * Every palette this build has, as a type.
 *
 * Written out rather than derived from the arrays below because it is the
 * thing strings.ts is checked against: a union inferred from the catalog would
 * widen the moment somebody typed `as string`, and the missing translation
 * would ship as a blank name.
 */
export type PaletteId =
  | "goodvoice-crimson"
  | "goodvoice-frost"
  | "goodvoice-forest"
  | "goodvoice-sand"
  | "goodvoice-rose"
  | "goodvoice-gold"
  | "goodvoice-ember"
  | "goodvoice-cyan"
  | "goodvoice-violet"
  | "goodvoice-matrix";

export type SkinId = "retro" | "terminal";

export interface PaletteOption {
  id: PaletteId;
  /** Swatch: page background, accent, foreground. */
  bg: string;
  acc: string;
  fg: string;
}

export const LIGHT_PALETTES: readonly PaletteOption[] = [
  {
    id: "goodvoice-crimson",
    bg: "var(--palette-cream)",
    acc: "var(--palette-crimson)",
    fg: "var(--palette-ink)",
  },
  {
    id: "goodvoice-frost",
    bg: "var(--palette-frost-bg)",
    acc: "var(--palette-abyss)",
    fg: "var(--palette-frost-ink)",
  },
  {
    id: "goodvoice-forest",
    bg: "var(--palette-forest-bg)",
    acc: "var(--palette-forest-green)",
    fg: "var(--palette-forest-ink)",
  },
  {
    id: "goodvoice-sand",
    bg: "var(--palette-sand-bg)",
    acc: "var(--palette-copper)",
    fg: "var(--palette-sand-ink)",
  },
] as const;

export const DARK_PALETTES: readonly PaletteOption[] = [
  {
    id: "goodvoice-rose",
    bg: "var(--palette-noir)",
    acc: "var(--palette-rose)",
    fg: "var(--palette-cream)",
  },
  {
    id: "goodvoice-gold",
    bg: "var(--palette-graphite)",
    acc: "var(--palette-gold)",
    fg: "var(--palette-silver)",
  },
  {
    id: "goodvoice-ember",
    bg: "var(--palette-midnight)",
    acc: "var(--palette-ember)",
    fg: "var(--palette-mint)",
  },
  {
    id: "goodvoice-cyan",
    bg: "var(--palette-cyan-bg)",
    acc: "var(--palette-cyan)",
    fg: "var(--palette-cyan-mist)",
  },
  {
    id: "goodvoice-violet",
    bg: "var(--palette-violet-bg)",
    acc: "var(--palette-violet)",
    fg: "var(--palette-violet-mist)",
  },
  {
    id: "goodvoice-matrix",
    bg: "var(--palette-black)",
    acc: "var(--palette-matrix-green)",
    fg: "var(--palette-matrix-mist)",
  },
] as const;

export const SKINS: readonly SkinId[] = ["retro", "terminal"] as const;

export const DEFAULT_LIGHT: PaletteId = "goodvoice-crimson";
export const DEFAULT_DARK: PaletteId = "goodvoice-rose";
export const DEFAULT_SKIN: SkinId = "retro";

const LIGHT_IDS: readonly string[] = LIGHT_PALETTES.map(
  (palette) => palette.id,
);
const DARK_IDS: readonly string[] = DARK_PALETTES.map((palette) => palette.id);
const SKIN_IDS: readonly string[] = SKINS;

/*
 * Each falls back to the default whenever the stored value is unknown — a
 * palette dropped from the catalog, or a hand-edited storage entry, must not
 * leave the document with a `data-theme` no stylesheet answers to.
 *
 * They return the *narrow* type rather than `string`, which is what makes an
 * unnamed palette a build error: the value goes on to index strings.ts, and a
 * `string` there would index a `Record<PaletteId, …>` with anything at all.
 */
export function lightPaletteOr(id: string | null | undefined): PaletteId {
  return id && LIGHT_IDS.includes(id) ? (id as PaletteId) : DEFAULT_LIGHT;
}

export function darkPaletteOr(id: string | null | undefined): PaletteId {
  return id && DARK_IDS.includes(id) ? (id as PaletteId) : DEFAULT_DARK;
}

export function skinOr(id: string | null | undefined): SkinId {
  return id && SKIN_IDS.includes(id) ? (id as SkinId) : DEFAULT_SKIN;
}
