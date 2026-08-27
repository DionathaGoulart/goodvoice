/*
 * Every word this window puts in front of a person, in each language it
 * speaks.
 *
 * # Why a catalog and not a lookup by key
 *
 * `Strings` is an interface, and each language is a value that has to satisfy
 * it. A missing translation is a type error at build time rather than a key
 * shining through the UI at run time, and a string that takes a number or a
 * room name is a *function* here — so a translation that forgets an argument
 * cannot compile either. There is no interpolation engine, no key parsing and
 * no bundle to fetch: both languages are in the JS the window already loads,
 * which is what lets `i18n.ts` switch them inside a frame.
 *
 * # What is deliberately not translated
 *
 * **Diagnostics.** What comes back from `join_room`, `start_share` or
 * `set_server` is Rust's own prose — a sentence written where the failure
 * happened, sometimes carrying a Windows or an SFU error inside it. Those
 * reach the window as text and are shown as they arrive, in English, in both
 * languages. Half-translating them would be worse than not: the surface is
 * every failure path in the client, and the half that stayed English would be
 * the half somebody has to paste into an issue.
 *
 * **Room codes, names and `anon`.** They are what somebody typed, or the
 * value the server is given when they typed nothing.
 *
 * # Tone
 *
 * Lower case, terse, and no exclamation marks — the skins are the loud part
 * (styleguide.md §3). Portuguese keeps that: `mudo`, not `Mudo`.
 */

import type { PaletteId, SkinId } from "./appearance";

/** The languages this build speaks. The value is a BCP-47 tag. */
export type Lang = "en" | "pt-BR";

/** In the order the picker offers them. */
export const LANGS: readonly Lang[] = ["en", "pt-BR"] as const;

/**
 * What each language calls itself.
 *
 * Endonyms, and the one thing here that is *not* translated: somebody looking
 * for their own language finds it faster written the way they write it than
 * translated into a language they do not read.
 */
export const LANG_NAMES: Record<Lang, string> = {
  en: "english",
  "pt-BR": "português",
};

export interface Strings {
  /** The masthead and the way into the settings screen. */
  readonly starting: string;
  readonly settings: string;
  readonly back: string;
  readonly done: string;

  /** The join form. */
  readonly room: string;
  readonly roomPlaceholder: string;
  readonly name: string;
  readonly namePlaceholder: string;
  readonly join: string;
  readonly joining: string;
  readonly roomCodeRule: string;

  /** The call panel. */
  readonly roomLabel: (room: string) => string;
  readonly reconnecting: (attempt: number) => string;
  readonly justYou: string;
  readonly you: string;
  readonly mutedTag: string;
  readonly deafenedTag: string;
  readonly mute: string;
  readonly unmute: string;
  readonly deafen: string;
  readonly undeafen: string;
  readonly copyInvite: string;
  readonly inviteCopied: string;
  readonly leave: string;

  /** A `goodvoice://join/<room>` link the client would not act on alone. */
  /** Reads straight into the room name, which is drawn in bold after it. */
  readonly inviteLead: string;
  readonly leaveAndJoin: (room: string) => string;
  readonly tryAgain: (room: string) => string;
  readonly dismiss: string;

  /** Screen share: the picker, the live share, and watching somebody else's. */
  readonly watchScreen: (who: string) => string;
  readonly shareAScreen: string;
  readonly sharingAt: (target: string, width: number, height: number) => string;
  readonly noHardwareEncoder: string;
  readonly stopSharing: string;
  readonly quality: string;
  readonly qualityHint: Record<"p720" | "p1080", string>;
  readonly whatToShare: string;
  readonly nothingToShare: string;
  readonly targetKind: Record<"monitor" | "window", string>;
  readonly cancel: string;

  /** The viewer window. */
  readonly waitingForPicture: string;
  readonly nobodyIsSharing: string;
  readonly noDecoder: string;
  readonly decoderStopped: (detail: string) => string;

  /** Settings — audio. */
  readonly audio: string;
  readonly transmit: string;
  readonly transmitMode: Record<
    "open" | "push-to-talk" | "voice-activity",
    { label: string; hint: string }
  >;
  readonly pressAKey: string;
  readonly talkKeyIs: (key: string) => string;
  readonly keyHeardAnywhere: string;
  readonly keyHeardHereOnly: string;
  readonly sensitivity: string;
  readonly automatic: string;
  readonly manual: string;
  readonly automaticHint: string;
  readonly thresholdAria: string;
  readonly thresholdHint: (decibels: number) => string;
  readonly thresholdInCall: string;
  readonly thresholdBeforeCall: string;
  readonly noiseSuppression: string;
  readonly noiseSuppressionHint: string;
  readonly echoCancellation: string;
  readonly echoCancellationHint: string;

  /** Settings — server. */
  readonly server: string;
  readonly workerUrl: string;
  readonly useThisServer: string;
  readonly backToBundled: string;
  readonly serverNextCall: string;
  readonly serverYourOwn: string;

  /** Settings — appearance and language. */
  readonly appearance: string;
  readonly mode: string;
  readonly modeChoice: Record<"light" | "dark" | "system", string>;
  readonly palette: string;
  readonly palettes: Record<PaletteId, string>;
  readonly skin: string;
  readonly skins: Record<SkinId, { label: string; hint: string }>;
  readonly language: string;
}

const en: Strings = {
  starting: "starting…",
  settings: "settings",
  back: "back",
  done: "done",

  room: "room",
  roomPlaceholder: "squad-night",
  name: "name",
  namePlaceholder: "anon",
  join: "join",
  joining: "joining…",
  roomCodeRule: "4–24 characters, letters, numbers and hyphens only",

  roomLabel: (room) => `room ${room}`,
  reconnecting: (attempt) => `reconnecting… (attempt ${attempt})`,
  justYou: "just you",
  you: "you",
  mutedTag: "muted",
  deafenedTag: "deafened",
  mute: "mute",
  unmute: "unmute",
  deafen: "deafen",
  undeafen: "undeafen",
  copyInvite: "copy invite",
  inviteCopied: "invite copied",
  leave: "leave",

  inviteLead: "an invite to",
  leaveAndJoin: (room) => `leave and join ${room}`,
  tryAgain: (room) => `try ${room} again`,
  dismiss: "dismiss",

  watchScreen: (who) => `watch ${who}'s screen`,
  shareAScreen: "share a screen",
  sharingAt: (target, width, height) =>
    `sharing ${target} at ${width}×${height}`,
  noHardwareEncoder: "no hardware encoder — this will cost the machine frames",
  stopSharing: "stop sharing",
  quality: "quality",
  qualityHint: {
    p720: "lighter on the network",
    p1080: "sharper, and what the budget is for",
  },
  whatToShare: "what to share",
  nothingToShare: "nothing to share",
  targetKind: { monitor: "monitor", window: "window" },
  cancel: "cancel",

  waitingForPicture: "waiting for a picture…",
  nobodyIsSharing: "nobody is sharing",
  noDecoder: "this webview cannot decode video",
  decoderStopped: (detail) => `the decoder stopped: ${detail}`,

  audio: "audio",
  transmit: "transmit",
  transmitMode: {
    open: {
      label: "open",
      hint: "the microphone is live until you mute",
    },
    "push-to-talk": {
      label: "push to talk",
      hint: "heard only while the key below is held",
    },
    "voice-activity": {
      label: "voice",
      hint: "heard only while you are talking",
    },
  },
  pressAKey: "press a key, or escape",
  talkKeyIs: (key) => `key: ${key}`,
  keyHeardAnywhere: "heard from anywhere, including over a game",
  keyHeardHereOnly: "heard only while this window has focus",
  sensitivity: "sensitivity",
  automatic: "automatic",
  manual: "manual",
  automaticHint:
    "a detector decides what is a voice. it ignores a keyboard at any volume, and lets a quiet room through at almost none",
  thresholdAria: "input threshold in decibels",
  thresholdHint: (decibels) =>
    `${decibels} dB — the room hears you past the mark.`,
  thresholdInCall: "talk, and put the mark just under where the bar sits.",
  thresholdBeforeCall: "the meter is live once you are in a room.",
  noiseSuppression: "noise suppression",
  noiseSuppressionHint:
    "takes a fan and a room's hum out from under your voice, and a little of the consonants with them",
  echoCancellation: "echo cancellation",
  echoCancellationHint:
    "stops the room hearing itself back. pointless on a headset, and the difference between a call and a howl on speakers",

  server: "server",
  workerUrl: "worker url",
  useThisServer: "use this server",
  backToBundled: "back to the bundled one",
  serverNextCall: "the next call joins here; this one stays where it is",
  serverYourOwn: "your own deploy goes here — docs/self-hosting.md",

  appearance: "appearance",
  mode: "mode",
  modeChoice: { light: "light", dark: "dark", system: "system" },
  palette: "palette",
  palettes: {
    "goodvoice-crimson": "crimson chalk",
    "goodvoice-frost": "abyss frost",
    "goodvoice-forest": "forest mist",
    "goodvoice-sand": "sand dusk",
    "goodvoice-rose": "noir rose",
    "goodvoice-gold": "vault gold",
    "goodvoice-ember": "midnight ember",
    "goodvoice-cyan": "cyber teal",
    "goodvoice-violet": "velvet purple",
    "goodvoice-matrix": "neon matrix",
  },
  skin: "skin",
  skins: {
    retro: { label: "neobrutal", hint: "thick frame, hard shadow" },
    terminal: { label: "terminal", hint: "crt, prompts, phosphor" },
  },
  language: "language",
};

/*
 * Brazilian Portuguese.
 *
 * Two choices worth writing down, because a later translator will otherwise
 * undo them:
 *
 *  - **`mudo` and `sem áudio`, not `mutado` and `ensurdecido`.** The roster
 *    has to say two different facts in one word each — cannot be heard, and
 *    cannot hear — and the loanwords say neither to somebody who has not met
 *    the English first.
 *  - **`falar apertando`, not `push to talk`.** The English term is widely
 *    understood by the people this app is for, but it is four syllables of a
 *    foreign language on a button that sits next to `aberto` and `voz`, and
 *    the row reads as two settings and a brand otherwise.
 */
const ptBR: Strings = {
  starting: "iniciando…",
  settings: "ajustes",
  back: "voltar",
  done: "pronto",

  room: "sala",
  roomPlaceholder: "noite-da-squad",
  name: "nome",
  namePlaceholder: "anon",
  join: "entrar",
  joining: "entrando…",
  roomCodeRule: "4–24 caracteres, apenas letras, números e hifens",

  roomLabel: (room) => `sala ${room}`,
  reconnecting: (attempt) => `reconectando… (tentativa ${attempt})`,
  justYou: "só você",
  you: "você",
  mutedTag: "mudo",
  deafenedTag: "sem áudio",
  mute: "silenciar",
  unmute: "reativar",
  deafen: "desligar áudio",
  undeafen: "religar áudio",
  copyInvite: "copiar convite",
  inviteCopied: "convite copiado",
  leave: "sair",

  inviteLead: "um convite para",
  leaveAndJoin: (room) => `sair e entrar em ${room}`,
  tryAgain: (room) => `tentar ${room} de novo`,
  dismiss: "dispensar",

  watchScreen: (who) => `ver a tela de ${who}`,
  shareAScreen: "compartilhar uma tela",
  sharingAt: (target, width, height) =>
    `compartilhando ${target} em ${width}×${height}`,
  noHardwareEncoder:
    "sem encoder de hardware — isso vai custar quadros da máquina",
  stopSharing: "parar de compartilhar",
  quality: "qualidade",
  qualityHint: {
    p720: "mais leve para a rede",
    p1080: "mais nítido, e é para isso que o orçamento existe",
  },
  whatToShare: "o que compartilhar",
  nothingToShare: "nada para compartilhar",
  targetKind: { monitor: "monitor", window: "janela" },
  cancel: "cancelar",

  waitingForPicture: "esperando uma imagem…",
  nobodyIsSharing: "ninguém está compartilhando",
  noDecoder: "esta webview não decodifica vídeo",
  decoderStopped: (detail) => `o decodificador parou: ${detail}`,

  audio: "áudio",
  transmit: "transmissão",
  transmitMode: {
    open: {
      label: "aberto",
      hint: "o microfone fica aberto até você silenciar",
    },
    "push-to-talk": {
      label: "falar apertando",
      hint: "só te ouvem enquanto a tecla abaixo estiver pressionada",
    },
    "voice-activity": {
      label: "voz",
      hint: "só te ouvem enquanto você estiver falando",
    },
  },
  pressAKey: "aperte uma tecla, ou escape",
  talkKeyIs: (key) => `tecla: ${key}`,
  keyHeardAnywhere: "ouvida de qualquer lugar, inclusive por cima de um jogo",
  keyHeardHereOnly: "ouvida só enquanto esta janela estiver em foco",
  sensitivity: "sensibilidade",
  automatic: "automática",
  manual: "manual",
  automaticHint:
    "um detector decide o que é voz. ignora um teclado em qualquer volume, e deixa passar uma sala silenciosa em quase nenhum",
  thresholdAria: "limiar de entrada em decibéis",
  thresholdHint: (decibels) =>
    `${decibels} dB — a sala te ouve a partir da marca.`,
  thresholdInCall: "fale, e ponha a marca logo abaixo de onde a barra fica.",
  thresholdBeforeCall: "o medidor fica vivo assim que você entrar numa sala.",
  noiseSuppression: "supressão de ruído",
  noiseSuppressionHint:
    "tira o ventilador e o zumbido da sala debaixo da sua voz, e um pouco das consoantes junto",
  echoCancellation: "cancelamento de eco",
  echoCancellationHint:
    "impede a sala de ouvir a si mesma. inútil num headset, e a diferença entre uma conversa e um microfonia na caixa de som",

  server: "servidor",
  workerUrl: "url do worker",
  useThisServer: "usar este servidor",
  backToBundled: "voltar ao que veio no app",
  serverNextCall: "a próxima chamada entra aqui; esta fica onde está",
  serverYourOwn: "o seu próprio deploy vai aqui — docs/self-hosting.md",

  appearance: "aparência",
  mode: "modo",
  modeChoice: { light: "claro", dark: "escuro", system: "sistema" },
  palette: "paleta",
  palettes: {
    "goodvoice-crimson": "giz carmesim",
    "goodvoice-frost": "gelo abissal",
    "goodvoice-forest": "névoa da mata",
    "goodvoice-sand": "areia ao anoitecer",
    "goodvoice-rose": "rosa noir",
    "goodvoice-gold": "ouro do cofre",
    "goodvoice-ember": "brasa da meia-noite",
    "goodvoice-cyan": "ciano cibernético",
    "goodvoice-violet": "roxo veludo",
    "goodvoice-matrix": "matrix neon",
  },
  skin: "pele",
  skins: {
    retro: { label: "neobrutal", hint: "moldura grossa, sombra dura" },
    terminal: { label: "terminal", hint: "crt, prompts, fósforo" },
  },
  language: "idioma",
};

export const CATALOG: Record<Lang, Strings> = { en, "pt-BR": ptBR };
