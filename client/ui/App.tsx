import {
  createResource,
  createSignal,
  For,
  onCleanup,
  Show,
  type Component,
} from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  DARK_PALETTES,
  LIGHT_PALETTES,
  SKINS,
  type ModePreference,
  type PaletteOption,
} from "./appearance";
import { choose as chooseLanguage, lang, t } from "./i18n";
import { LANG_NAMES, LANGS } from "./strings";
import { currentMode, currentTheme, pickPalette, prefs, update } from "./theme";

/** Mirrors `ClientInfo` in `src-tauri/src/lib.rs`. */
/**
 * A `goodvoice://join/<room>` link the client would not act on by itself, or
 * tried to and could not (plan.md task 6.2). `joinable` is false for an invite
 * to another deploy — a link is never allowed to move this client to somebody
 * else's server; it is true both for a link that arrived mid-call and for one
 * whose join failed, which are the two the window can offer to act on.
 */
interface InviteOffer {
  room: string;
  reason: string;
  joinable: boolean;
}

interface ClientInfo {
  name: string;
  version: string;
  server: string;
  /** What the build shipped with, for the "back to it" button. */
  defaultServer: string;
  /** Whether the server above was chosen by a person (prd.md §9). */
  serverChosen: boolean;
}

/** Mirrors `Participant` in `src-tauri/src/rtc/signaling.rs`. */
interface Participant {
  id: string;
  name: string;
  joinedAt: number;
  muted: boolean;
  deafened: boolean;
  sharing: boolean;
  sessionId: string | null;
  tracks: { name: string; kind: string }[];
}

/** Mirrors `CallStatus` in `src-tauri/src/lib.rs`. */
interface CallStatus {
  self_id: string;
  room: string;
  participants: Participant[];
}

/** Mirrors `CallState` in `src-tauri/src/rtc/reconnect.rs`. */
type CallState =
  | { state: "live" }
  | { state: "reconnecting"; attempt: number }
  | { state: "ended"; reason: "left" }
  | { state: "ended"; reason: "refused" | "unreachable"; detail: string };

/** Mirrors `CallHealth` in `src-tauri/src/lib.rs`. */
type CallHealth = CallState & { self_id: string };

/** Mirrors `Controls` in `src-tauri/src/lib.rs`. */
interface Controls {
  in_call: boolean;
  muted: boolean;
  deafened: boolean;
}

/** Mirrors `Talker` in `src-tauri/src/rtc/session.rs`. */
interface Talker {
  id: string;
  /** 0–1, quantised to 1/255 by the client. */
  level: number;
}

/** Mirrors `Levels` in `src-tauri/src/rtc/session.rs`. */
interface Levels {
  talking: Talker[];
  /** The microphone before mute and before the gate. */
  input: number;
}

/** Mirrors `Target` in `src-tauri/src/capture/wgc.rs`. */
interface ShareTarget {
  kind: "monitor" | "window";
  handle: number;
  name: string;
  width: number;
  height: number;
}

/** Mirrors `Quality` in `src-tauri/src/capture/encoder.rs`. */
type Quality = "p720" | "p1080";

/** Mirrors `ShareState` in `src-tauri/src/rtc/screen.rs`. */
type ShareState =
  | { state: "idle" }
  | {
      state: "sharing";
      target: string;
      width: number;
      height: number;
      hardware: boolean;
    }
  | { state: "failed"; detail: string };

/** Mirrors `AudioSettings` in `src-tauri/src/audio/prefs.rs`. */
interface AudioSettings {
  automaticSensitivity: boolean;
  threshold: number;
  noiseSuppression: boolean;
  echoCancellation: boolean;
}

/** Mirrors `Snapshot` in `src-tauri/src/lib.rs`. */
interface Snapshot {
  call: CallStatus | null;
  controls: Controls;
  health: CallHealth | null;
  speaking: Levels;
  audio: AudioSettings;
  share: ShareState;
  /** A link the client would not act on, still waiting to be answered. */
  invite: InviteOffer | null;
}

/** The event `push_roster` emits. Kept in step with `ROSTER_EVENT`. */
const ROSTER_EVENT = "goodvoice://roster";

/**
 * A call that began without this window asking for it. Kept in step with
 * `CALL_EVENT`.
 *
 * `join` below sets the call from what `join_room` returns, because it asked.
 * Autojoin (`GOODVOICE_AUTOJOIN`) and, later, invite links do not ask, and no
 * other event carries the room name.
 */
const CALL_EVENT = "goodvoice://call";

/** The event `push_state` emits. Kept in step with `STATE_EVENT`. */
const STATE_EVENT = "goodvoice://state";

/** The event `push_speaking` emits. Kept in step with `SPEAKING_EVENT`. */
const SPEAKING_EVENT = "goodvoice://speaking";

/**
 * The event `push_controls` emits. Kept in step with `CONTROLS_EVENT`.
 *
 * Mute and deafen can be changed from the tray menu, so this window is not
 * where either of them lives — it is told, the same as the tray is.
 */
const CONTROLS_EVENT = "goodvoice://controls";

/** The event `push_share` emits. Kept in step with `SHARE_EVENT`. */
const SHARE_EVENT = "goodvoice://share";
const INVITE_EVENT = "goodvoice://invite";

/** The two the picker offers, in the order prd.md §3 F3 names them. */
const QUALITIES: readonly Quality[] = ["p720", "p1080"] as const;

/**
 * A resolution is digits in every language, so this is not in strings.ts and
 * the hint beside it is (`t().qualityHint`).
 */
const QUALITY_LABEL: Record<Quality, string> = {
  p720: "720p",
  p1080: "1080p",
};

/**
 * The room code the server will accept: `roomCodeSchema` in
 * `server/src/protocol.ts`. Checked here only so a typo is a hint rather than
 * a round trip — the server validates regardless.
 */
const ROOM_CODE = /^[a-zA-Z0-9-]{4,24}$/;

/** Mirrors `TransmitMode` in `src-tauri/src/audio/vad.rs`. */
type TransmitMode = "open" | "push-to-talk" | "voice-activity";

/** The modes, in the order they take control away from the microphone. */
const MODES: readonly TransmitMode[] = [
  "open",
  "push-to-talk",
  "voice-activity",
] as const;

/**
 * The three answers to "which palette", in the order they give up control:
 * two that pin it and one that hands it to the operating system. `null` is a
 * real choice, not a missing one — see theme.ts.
 */
const MODE_CHOICES: { id: ModePreference; key: "light" | "dark" | "system" }[] =
  [
    { id: "light", key: "light" },
    { id: "dark", key: "dark" },
    { id: null, key: "system" },
  ];

/**
 * Where the transmit settings live between runs.
 *
 * The webview's own storage, because this window is the only thing that reads
 * them: the mode is handed to `join_room`, and the key never leaves here. The
 * global hotkey (plan.md task 4.3) is the point at which Rust needs its own
 * copy, and that is the task that should give it one.
 */
const MODE_STORE = "goodvoice.transmit-mode";
const TALK_KEY_STORE = "goodvoice.talk-key";
const AUDIO_STORE = "goodvoice.audio";
const QUALITY_STORE = "goodvoice.share-quality";

/**
 * The quietest and loudest a manual threshold can be. Mirrors
 * `MIN_THRESHOLD` and `MAX_THRESHOLD` in `src-tauri/src/audio/prefs.rs`,
 * which clamps whatever this sends anyway.
 */
const MIN_THRESHOLD = 0.002;
const MAX_THRESHOLD = 0.25;

/**
 * The bottom of every meter in this window, in decibels below full scale.
 *
 * Levels arrive linear, and speech spends nearly all of its time in the bottom
 * tenth of that range — drawn linearly, a conversation is a bar that never
 * leaves the left edge. Decibels are the scale the ear uses and the scale the
 * numbers were chosen on: `SPEAKING_LEVEL` is 0.02, which is −34 dB, and reads
 * as a sensible two fifths of the way up rather than as 2%.
 */
const METER_FLOOR_DB = -60;

/** A level as decibels below full scale. Silence is the floor, not −∞. */
const decibels = (level: number) =>
  level <= 0
    ? METER_FLOOR_DB
    : Math.max(METER_FLOOR_DB, 20 * Math.log10(level));

/** A level as a 0–1 position on the meters. */
const meterFraction = (level: number) =>
  Math.min(
    1,
    Math.max(0, (decibels(level) - METER_FLOOR_DB) / -METER_FLOOR_DB),
  );

/** A decibel reading back to the linear level the client wants. */
const levelFromDecibels = (db: number) =>
  Math.min(MAX_THRESHOLD, Math.max(MIN_THRESHOLD, 10 ** (db / 20)));

const DEFAULT_AUDIO: AudioSettings = {
  automaticSensitivity: true,
  threshold: 0.02,
  noiseSuppression: true,
  echoCancellation: true,
};

/** What was stored last run, if any of it still looks like settings. */
const storedAudio = (): AudioSettings | null => {
  const raw = localStorage.getItem(AUDIO_STORE);
  if (!raw) {
    return null;
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return null;
    }
    const held = parsed as Partial<AudioSettings>;
    return {
      automaticSensitivity:
        held.automaticSensitivity ?? DEFAULT_AUDIO.automaticSensitivity,
      threshold:
        typeof held.threshold === "number" && Number.isFinite(held.threshold)
          ? held.threshold
          : DEFAULT_AUDIO.threshold,
      noiseSuppression: held.noiseSuppression ?? DEFAULT_AUDIO.noiseSuppression,
      echoCancellation: held.echoCancellation ?? DEFAULT_AUDIO.echoCancellation,
    };
  } catch {
    // Storage somebody else wrote, or a half-written entry. The defaults are
    // a working call; a thrown exception here would be a blank window.
    return null;
  }
};

/**
 * What a fresh install holds down. Space is reachable without a chord and is
 * not a shortcut anywhere in this window.
 */
const DEFAULT_TALK_KEY = "Space";

const isMode = (value: string | null): value is TransmitMode =>
  (MODES as readonly string[]).includes(value ?? "");

const isQuality = (value: string | null): value is Quality =>
  (QUALITIES as readonly string[]).includes(value ?? "");

/** `KeyboardEvent.code` as something a person would recognise on a keycap. */
const keyName = (code: string) =>
  code
    .replace(/^(Key|Digit)/, "")
    .replace(/(Left|Right)$/, " $1")
    .toLowerCase();

const App: Component = () => {
  const [info, { mutate: setInfo }] = createResource(() =>
    invoke<ClientInfo>("client_info"),
  );

  /**
   * The Worker this client joins rooms on, while somebody is editing it.
   *
   * Self-hosting ends with "paste the Worker URL into the client's settings"
   * (prd.md §9, docs/self-hosting.md), and this is that box. The client is the
   * source of truth — it remembers the choice on disk, because the window is
   * not the only thing that joins — so this signal is only what is being
   * typed, and `info()` is what is in force.
   */
  /** A link that arrived while this window was open, and what to do with it. */
  const [invite, setInvite] = createSignal<InviteOffer | null>(null);
  /**
   * What the *copy invite* button last did, so the click has an answer.
   *
   * Which of two things happened, not the words for it: the button can still
   * be showing its answer when somebody changes language, and a stored
   * sentence would stay in the language it was written in.
   */
  const [copied, setCopied] = createSignal<
    { ok: true } | { ok: false; detail: string } | null
  >(null);

  const [serverDraft, setServerDraft] = createSignal<string | null>(null);
  const [serverError, setServerError] = createSignal<string | null>(null);
  const [savingServer, setSavingServer] = createSignal(false);

  const serverText = () => serverDraft() ?? info()?.server ?? "";

  const saveServer = async (url: string) => {
    setSavingServer(true);
    setServerError(null);
    try {
      const next = await invoke<ClientInfo>("set_server", { url });
      setInfo(next);
      setServerDraft(null);
    } catch (reason) {
      setServerError(String(reason));
    } finally {
      setSavingServer(false);
    }
  };

  const [room, setRoom] = createSignal("");
  const [name, setName] = createSignal("");
  const [joining, setJoining] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const [call, setCall] = createSignal<CallStatus | null>(null);
  const [roster, setRoster] = createSignal<Participant[]>([]);
  const [muted, setMuted] = createSignal(false);
  const [deafened, setDeafened] = createSignal(false);
  const [health, setHealth] = createSignal<CallState>({ state: "live" });
  const [share, setShare] = createSignal<ShareState>({ state: "idle" });
  // The picker's own state. Open is a deliberate act — enumerating windows
  // costs nothing but the list is stale the moment it is drawn, so it is
  // fetched when the picker opens and not before.
  const [picking, setPicking] = createSignal(false);
  const [targets, setTargets] = createSignal<ShareTarget[]>([]);
  const [quality, setQuality] = createSignal<Quality>(
    isQuality(localStorage.getItem(QUALITY_STORE))
      ? (localStorage.getItem(QUALITY_STORE) as Quality)
      : "p1080",
  );
  // Keyed by id, not by name: two people in a room may share a name, and the
  // id is what the roster rows are keyed on anyway. A level rather than
  // membership — the dot fades now, and a set cannot say how bright.
  const [speaking, setSpeaking] = createSignal<ReadonlyMap<string, number>>(
    new Map<string, number>(),
  );
  /** The microphone before the gate, for the sensitivity meter. */
  const [inputLevel, setInputLevel] = createSignal(0);

  const [audio, setAudio] = createSignal<AudioSettings>(
    storedAudio() ?? DEFAULT_AUDIO,
  );

  const savedMode = localStorage.getItem(MODE_STORE);
  const [mode, setMode] = createSignal<TransmitMode>(
    isMode(savedMode) ? savedMode : "open",
  );
  const [talkKey, setTalkKey] = createSignal(
    localStorage.getItem(TALK_KEY_STORE) ?? DEFAULT_TALK_KEY,
  );
  const [rebinding, setRebinding] = createSignal(false);
  // Which screen is showing. Not part of the call's state: it survives joining
  // and leaving, and a call carries on underneath it.
  const [settings, setSettings] = createSignal(false);
  // Whether the key is heard from anywhere or only in this window. The two
  // look identical until somebody is inside a game, which is the one place it
  // matters, so the window says which one it has.
  const [globalKey, setGlobalKey] = createSignal(false);

  /**
   * Tells the client which key push to talk means, and asks back whether it
   * managed to watch the whole desktop for it.
   *
   * Sent on every join and every rebind: the key is stored here (task 3.3) and
   * the client learns it the same way anyone else would.
   */
  const bindTalkKey = async () => {
    try {
      await invoke("set_talk_binding", { code: talkKey() });
      setGlobalKey(await invoke<boolean>("talk_key_is_global"));
    } catch {
      setGlobalKey(false);
    }
  };

  /**
   * Hands the client the audio settings this window remembers.
   *
   * Unconditional and immediate, because they are not call state: the capture
   * path reads them whether or not anyone has joined anything, and a client
   * that autojoined before this window existed (task 4.6, `GOODVOICE_AUTOJOIN`)
   * is running on defaults until it is told otherwise. When there is nothing
   * stored, the snapshot below adopts whatever the client already has instead.
   */
  const held = storedAudio();
  if (held) {
    void invoke<AudioSettings>("set_audio_settings", { settings: held })
      .then(setAudio)
      .catch(() => {});
  }

  // Subscribed once for the life of the window: the room the events belong to
  // is whichever call is open, and there is only ever one.
  const stopping = [
    listen<Participant[]>(ROSTER_EVENT, (event) => setRoster(event.payload)),
    listen<CallStatus>(CALL_EVENT, (event) => {
      setCall(event.payload);
      setRoster(event.payload.participants);
      setHealth({ state: "live" });
      setError(null);
    }),
    listen<Levels>(SPEAKING_EVENT, (event) => {
      setSpeaking(
        new Map(event.payload.talking.map((who) => [who.id, who.level])),
      );
      setInputLevel(event.payload.input);
    }),
    listen<Controls>(CONTROLS_EVENT, (event) => {
      setMuted(event.payload.muted);
      setDeafened(event.payload.deafened);
    }),
    listen<InviteOffer>(INVITE_EVENT, (event) => setInvite(event.payload)),
    listen<ShareState>(SHARE_EVENT, (event) => {
      setShare(event.payload);
      // A share that went live has answered the picker's question, so the
      // picker gets out of the way. A failure leaves it open: whatever went
      // wrong, the next thing the user wants is to pick again.
      if (event.payload.state === "sharing") {
        setPicking(false);
      }
    }),
    listen<CallHealth>(STATE_EVENT, (event) => {
      const { self_id, ...state } = event.payload;
      setHealth(state);
      // A reconnect takes a new seat, so the id the roster marks as "you"
      // changes mid-call.
      setCall((current) => (current ? { ...current, self_id } : current));
      if (state.state === "ended") {
        setCall(null);
        setRoster([]);
        setSpeaking(new Map<string, number>());
        setInputLevel(0);
        setShare({ state: "idle" });
        setPicking(false);
        if (state.reason !== "left") {
          setError(state.detail);
        }
      }
    }),
  ];

  /**
   * What this window missed before it existed.
   *
   * goodvoice drops its webview while it sits in the tray and builds a new one
   * when somebody opens it (plan.md task 4.6), so this component now starts up
   * in the middle of calls it never saw begin. The events above only carry
   * changes; `current_status` carries the state.
   *
   * Applied only while this window still knows nothing. The listeners are
   * registered before the ask, so anything that happens in between arrives as
   * an event — and an event is newer than the answer to a question asked
   * before it.
   */
  void Promise.all(stopping)
    .then(() => invoke<Snapshot>("current_status"))
    .then((status) => {
      // Before the call check below, and unconditionally: an invite is the one
      // thing here that is routinely offered *before this window existed*.
      // Windows starts the process for a `goodvoice://` link, so the client has
      // usually already refused it — or tried to join and failed — while the
      // webview was still being built, and the event that said so reached
      // nobody. Without this, that link looks like a link that did nothing.
      if (status.invite) {
        setInvite(status.invite);
      }
      if (call() !== null) {
        return;
      }
      setMuted(status.controls.muted);
      setDeafened(status.controls.deafened);
      setShare(status.share);
      if (!held) {
        setAudio(status.audio);
      }
      if (!status.call) {
        return;
      }
      setCall(status.call);
      setRoster(status.call.participants);
      setSpeaking(
        new Map(status.speaking.talking.map((who) => [who.id, who.level])),
      );
      setInputLevel(status.speaking.input);
      if (status.health) {
        const { self_id: _self, ...state } = status.health;
        setHealth(state);
      }
      // Asked rather than re-bound: the hook is in Rust and outlived this
      // window, so re-binding would uninstall a working one to install the
      // same one. All this window is missing is the answer.
      void invoke<boolean>("talk_key_is_global")
        .then(setGlobalKey)
        .catch(() => setGlobalKey(false));
    })
    .catch(() => {
      // A client that cannot say what it is doing is a client that is not in a
      // call yet, which is what the window already shows.
    });

  /** A key pressed into a text field is text, not a talk key. */
  const isTyping = (event: KeyboardEvent) =>
    event.target instanceof HTMLInputElement;

  /**
   * Tells the call the talk key moved. Failures are dropped on purpose: the
   * only way this rejects is a key that arrives as the call ends, and there is
   * nothing left to gate by then.
   */
  const pressTalk = (down: boolean) => {
    if (!call()) {
      return;
    }
    void invoke("set_talk_key", { down }).catch(() => {});
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (rebinding()) {
      event.preventDefault();
      if (event.code !== "Escape") {
        setTalkKey(event.code);
        localStorage.setItem(TALK_KEY_STORE, event.code);
        void bindTalkKey();
      }
      setRebinding(false);
      return;
    }
    if (
      isTyping(event) ||
      mode() !== "push-to-talk" ||
      event.code !== talkKey()
    ) {
      return;
    }
    // Space would otherwise press whichever button has focus.
    event.preventDefault();
    // Holding a key repeats keydown; only the first one is news.
    if (!event.repeat) {
      pressTalk(true);
    }
  };

  const onKeyUp = (event: KeyboardEvent) => {
    if (
      isTyping(event) ||
      mode() !== "push-to-talk" ||
      event.code !== talkKey()
    ) {
      return;
    }
    event.preventDefault();
    pressTalk(false);
  };

  // A key held while the window loses focus never sends its keyup, and the
  // microphone would stay open behind whatever the user alt-tabbed into.
  const onBlur = () => pressTalk(false);

  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("keyup", onKeyUp);
  window.addEventListener("blur", onBlur);

  onCleanup(() => {
    for (const pending of stopping) {
      void pending.then((stop) => stop());
    }
    window.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("keyup", onKeyUp);
    window.removeEventListener("blur", onBlur);
  });

  const canJoin = () => ROOM_CODE.test(room().trim()) && !joining();

  /**
   * The attempt number while the client is taking a new seat, or undefined.
   * A dropped call is never silent: it says what it is doing (prd.md §5 flow E).
   */
  const reconnecting = () => {
    const current = health();
    return current.state === "reconnecting" ? current.attempt : undefined;
  };

  const join = async (event: Event) => {
    event.preventDefault();
    if (!canJoin()) {
      return;
    }

    setJoining(true);
    setError(null);
    try {
      const status = await invoke<CallStatus>("join_room", {
        server: info()?.server ?? "",
        room: room().trim(),
        name: name().trim() || "anon",
        mode: mode(),
      });
      setCall(status);
      setRoster(status.participants);
      setSpeaking(new Map<string, number>());
      setMuted(false);
      setDeafened(false);
      setHealth({ state: "live" });
      void bindTalkKey();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setJoining(false);
    }
  };

  /**
   * Puts this room's invite link on the clipboard.
   *
   * The link is built by the client, not here: half of it is which server this
   * client is on, and that is the client's to know (DR-36).
   *
   * `execCommand` is the fallback rather than the plan — WebView2 grants
   * `navigator.clipboard` to a page it considers secure, and the custom
   * protocol is one, but a build running against the Vite dev server over
   * plain http is not.
   */
  const copyInvite = async () => {
    try {
      const link = await invoke<string>("invite_link");
      try {
        await navigator.clipboard.writeText(link);
      } catch {
        const box = document.createElement("textarea");
        box.value = link;
        box.setAttribute("readonly", "");
        box.style.position = "fixed";
        box.style.opacity = "0";
        document.body.append(box);
        box.select();
        document.execCommand("copy");
        box.remove();
      }
      setCopied({ ok: true });
    } catch (reason) {
      setCopied({ ok: false, detail: String(reason) });
    }
    window.setTimeout(() => setCopied(null), 4000);
  };

  /**
   * Takes up a link the client handed back: one that arrived while a call was
   * already running — leave the one you are in, join the one you were sent —
   * or one whose own join failed, where there is nothing to leave and this is
   * simply trying it again.
   *
   * Only ever from a click. The client refuses to do the first on its own, and
   * the reason is the same one — somebody is mid-conversation, and a link is
   * not grounds for ending it on their behalf.
   */
  const acceptInvite = async (offer: InviteOffer) => {
    setInvite(null);
    void invoke("dismiss_invite").catch(() => {});
    await leave();
    setRoom(offer.room);
    try {
      const status = await invoke<CallStatus>("join_room", {
        server: info()?.server ?? "",
        room: offer.room,
        name: name().trim() || "anon",
        mode: mode(),
      });
      setCall(status);
      setRoster(status.participants);
      setHealth({ state: "live" });
      void bindTalkKey();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const leave = async () => {
    await invoke("leave_room");
    setCall(null);
    setRoster([]);
    setSpeaking(new Map<string, number>());
    setInputLevel(0);
  };

  /**
   * Stores the audio settings and sends them to the client.
   *
   * Both, every time, and in that order. Storage is what survives a restart —
   * Rust holds these in memory only — and the client is what the microphone
   * actually reads. What comes back is what was stored after clamping, which
   * is not always what was sent, so the slider ends up where the value landed
   * rather than where it aimed.
   */
  const changeAudio = (change: Partial<AudioSettings>) => {
    const next = { ...audio(), ...change };
    setAudio(next);
    localStorage.setItem(AUDIO_STORE, JSON.stringify(next));
    void invoke<AudioSettings>("set_audio_settings", { settings: next })
      .then(setAudio)
      .catch(() => {
        // Nothing to recover: the settings are already stored, and the next
        // change sends the whole set again.
      });
  };

  /**
   * Switches how transmission is gated. Saved whether or not there is a call
   * to apply it to — it is a setting, and the next join carries it.
   */
  const chooseMode = (next: TransmitMode) => {
    setMode(next);
    localStorage.setItem(MODE_STORE, next);
    // A key held across the switch has no keyup to look forward to under the
    // new mode, so it is let go here.
    pressTalk(false);
    if (call()) {
      void invoke("set_transmit_mode", { mode: next })
        .then(() => invoke<boolean>("talk_key_is_global"))
        .then(setGlobalKey)
        .catch(() => setGlobalKey(false));
    }
  };

  /**
   * Set here as well as awaited from `CONTROLS_EVENT`: the button should look
   * pressed on the click rather than a round trip later. The event that
   * follows says the same thing, and says it again when the change came from
   * the tray instead.
   */
  const toggleMute = async () => {
    const next = !muted();
    setMuted(next);
    await invoke("set_muted", { muted: next });
  };

  const toggleDeafen = async () => {
    const next = !deafened();
    setDeafened(next);
    await invoke("set_deafened", { deafened: next });
  };

  /**
   * Opens the picker and asks the client what there is to share.
   *
   * Asked on every open. Windows open and close while the app is running, so
   * a list cached at startup would offer things that are gone — and a target
   * that closes between the picker and the share fails at `start_share`,
   * which is where it has to be handled anyway.
   */
  const openPicker = async () => {
    setPicking(true);
    try {
      setTargets(await invoke<ShareTarget[]>("share_targets"));
    } catch (failure) {
      setTargets([]);
      setShare({ state: "failed", detail: String(failure) });
    }
  };

  const pickQuality = (next: Quality) => {
    setQuality(next);
    localStorage.setItem(QUALITY_STORE, next);
  };

  /**
   * Starts a share, and says nothing about whether it worked.
   *
   * The answer arrives on `SHARE_EVENT`, because it is not this window's to
   * give: opening a capture and renegotiating with the SFU takes a moment, and
   * the room can refuse (prd.md §8, one sharer at a time).
   */
  const startShare = async (target: ShareTarget) => {
    try {
      await invoke("start_share", { target, quality: quality() });
    } catch (failure) {
      setShare({ state: "failed", detail: String(failure) });
    }
  };

  /**
   * Who, if anybody, is sharing a screen that this client could watch.
   *
   * Never yourself: the app already knows what your own screen looks like,
   * and pulling your own track back through the SFU would be paying twice for
   * a picture you are looking at.
   */
  const sharer = () => {
    const joined = call();
    if (!joined) {
      return null;
    }
    return (
      roster().find((peer) => peer.sharing && peer.id !== joined.self_id) ??
      null
    );
  };

  /**
   * Opens the viewer window.
   *
   * Opening it is the whole of subscribing: the window asks the client for the
   * video when it mounts and gives it up when it closes, so a client with no
   * viewer open is never sent any (prd.md §3 F3).
   */
  const watchScreen = async () => {
    try {
      await invoke("open_screen_viewer");
    } catch (failure) {
      setShare({ state: "failed", detail: String(failure) });
    }
  };

  const stopShare = async () => {
    try {
      await invoke("stop_share");
    } catch {
      // Nothing to stop, which is the state the button was trying to reach.
    }
    setShare({ state: "idle" });
  };

  /**
   * A meter, drawn in decibels. Used for the input level and, with a marker,
   * for the threshold the input is being judged against.
   */
  const Meter = (props: { level: number; threshold?: number }) => (
    <div
      class="meter"
      style={{ "--fill": String(meterFraction(props.level)) }}
      aria-hidden="true"
    >
      <span class="meter-fill" />
      <Show when={props.threshold !== undefined}>
        <span
          class="meter-mark"
          style={{ "--at": String(meterFraction(props.threshold ?? 0)) }}
        />
      </Show>
    </div>
  );

  /**
   * A switch. Two of them, and both are the same shape: a thing WebRTC does to
   * the microphone that somebody might not want done.
   */
  const Toggle = (props: {
    label: string;
    hint: string;
    on: boolean;
    onChange: (on: boolean) => void;
  }) => (
    <div class="field">
      <button
        class="action toggle"
        classList={{ "action-picked": props.on }}
        type="button"
        role="switch"
        aria-checked={props.on}
        onClick={() => props.onChange(!props.on)}
      >
        <span class="toggle-box" aria-hidden="true">
          {props.on ? "\u00d7" : "\u00a0"}
        </span>
        {props.label}
      </button>
      <p class="notice">{props.hint}</p>
    </div>
  );

  /**
   * How loud the microphone has to be before the room hears it.
   *
   * Only shown in voice mode, because it is the only mode that asks the
   * question: open mode sends everything and push to talk asks a key instead.
   * The meter is live only during a call — the microphone is opened by the
   * join, so before one there is nothing to measure.
   */
  const Sensitivity = () => (
    <div class="field">
      <span class="field-label">{t().sensitivity}</span>
      <div class="modes">
        <button
          class="action"
          classList={{ "action-picked": audio().automaticSensitivity }}
          type="button"
          aria-pressed={audio().automaticSensitivity}
          onClick={() => changeAudio({ automaticSensitivity: true })}
        >
          {t().automatic}
        </button>
        <button
          class="action"
          classList={{ "action-picked": !audio().automaticSensitivity }}
          type="button"
          aria-pressed={!audio().automaticSensitivity}
          onClick={() => changeAudio({ automaticSensitivity: false })}
        >
          {t().manual}
        </button>
      </div>

      <Show
        when={!audio().automaticSensitivity}
        fallback={<p class="notice">{t().automaticHint}</p>}
      >
        <Meter level={inputLevel()} threshold={audio().threshold} />
        <input
          class="slider"
          type="range"
          min={decibels(MIN_THRESHOLD)}
          max={decibels(MAX_THRESHOLD)}
          step={0.5}
          value={decibels(audio().threshold)}
          aria-label={t().thresholdAria}
          onInput={(event) =>
            changeAudio({
              threshold: levelFromDecibels(event.currentTarget.valueAsNumber),
            })
          }
        />
        <p class="notice">
          {t().thresholdHint(Math.round(decibels(audio().threshold)))}{" "}
          {call() ? t().thresholdInCall : t().thresholdBeforeCall}
        </p>
      </Show>
    </div>
  );

  /**
   * Everything that is a setting rather than a call. Its own screen rather
   * than a section on the other two: it is opened rarely and read carefully,
   * which is the opposite of everything else in this window, and the roster
   * has no room to spare.
   *
   * Only the current mode's palettes are offered, so a click means "this one,
   * for this mode" and can never silently rewrite the other mode's choice.
   */
  const Settings = () => (
    <section class="panel animate-enter">
      <h2 class="section">{t().audio}</h2>

      <TransmitSettings />

      <Show when={mode() === "voice-activity"}>
        <Sensitivity />
      </Show>

      <Toggle
        label={t().noiseSuppression}
        hint={t().noiseSuppressionHint}
        on={audio().noiseSuppression}
        onChange={(on) => changeAudio({ noiseSuppression: on })}
      />

      <Toggle
        label={t().echoCancellation}
        hint={t().echoCancellationHint}
        on={audio().echoCancellation}
        onChange={(on) => changeAudio({ echoCancellation: on })}
      />

      <h2 class="section">{t().server}</h2>

      <label class="field">
        <span class="field-label">{t().workerUrl}</span>
        <input
          class="field-input"
          value={serverText()}
          onInput={(event) => setServerDraft(event.currentTarget.value)}
          placeholder={info()?.defaultServer ?? "https://…"}
          autocapitalize="none"
          autocomplete="off"
          spellcheck={false}
          disabled={savingServer()}
        />
      </label>

      <div class="modes">
        <button
          class="action"
          type="button"
          disabled={savingServer() || serverText().trim() === info()?.server}
          onClick={() => void saveServer(serverText())}
        >
          {t().useThisServer}
        </button>
        <Show when={info()?.serverChosen}>
          <button
            class="action"
            type="button"
            disabled={savingServer()}
            onClick={() => void saveServer("")}
          >
            {t().backToBundled}
          </button>
        </Show>
      </div>

      <Show
        when={serverError()}
        fallback={
          <p class="notice">
            {call() ? t().serverNextCall : t().serverYourOwn}
          </p>
        }
      >
        {(message) => (
          <p class="notice notice-error" role="alert">
            {message()}
          </p>
        )}
      </Show>

      <h2 class="section">{t().appearance}</h2>

      <div class="field">
        <span class="field-label appearance-label">{t().mode}</span>
        <div class="modes">
          <For each={MODE_CHOICES}>
            {(choice) => (
              <button
                class="action"
                classList={{ "action-picked": prefs().mode === choice.id }}
                type="button"
                aria-pressed={prefs().mode === choice.id}
                onClick={() => update({ mode: choice.id })}
              >
                {t().modeChoice[choice.key]}
              </button>
            )}
          </For>
        </div>
      </div>

      <div class="field">
        <span class="field-label appearance-label">{t().palette}</span>
        <div class="swatches">
          <For each={currentMode() === "dark" ? DARK_PALETTES : LIGHT_PALETTES}>
            {(palette) => <Swatch palette={palette} />}
          </For>
        </div>
      </div>

      <div class="field">
        <span class="field-label appearance-label">{t().skin}</span>
        <div class="controls">
          <For each={SKINS}>
            {(skin) => (
              <button
                class="action"
                classList={{ "action-picked": prefs().skin === skin }}
                type="button"
                aria-pressed={prefs().skin === skin}
                onClick={() => update({ skin })}
              >
                {t().skins[skin].label}
              </button>
            )}
          </For>
        </div>
        <p class="notice">{t().skins[prefs().skin].hint}</p>
      </div>

      {/* Last of the three, and its own field rather than a row inside
          appearance: it is not a look, it changes every other word on this
          screen — and the tray menu with them (i18n.ts). */}
      <div class="field">
        <span class="field-label appearance-label">{t().language}</span>
        <div class="controls">
          <For each={LANGS}>
            {(option) => (
              <button
                class="action"
                classList={{ "action-picked": lang() === option }}
                type="button"
                lang={option}
                aria-pressed={lang() === option}
                onClick={() => chooseLanguage(option)}
              >
                {LANG_NAMES[option]}
              </button>
            )}
          </For>
        </div>
      </div>

      <button
        class="action action-primary"
        type="button"
        onClick={() => setSettings(false)}
      >
        {t().done}
      </button>
    </section>
  );

  /**
   * One palette, previewed in its own colours. The swatch reads the same
   * `--palette-*` variables the theme is built from, so a preview cannot drift
   * from the thing it previews (styleguide.md §2.1).
   */
  const Swatch = (props: { palette: PaletteOption }) => (
    <button
      class="swatch"
      classList={{ "swatch-picked": currentTheme() === props.palette.id }}
      type="button"
      title={t().palettes[props.palette.id]}
      aria-label={t().palettes[props.palette.id]}
      aria-pressed={currentTheme() === props.palette.id}
      style={{
        "--swatch-bg": props.palette.bg,
        "--swatch-acc": props.palette.acc,
        "--swatch-fg": props.palette.fg,
      }}
      onClick={() => pickPalette(props.palette.id)}
    >
      <span class="swatch-acc" aria-hidden="true" />
      <span class="swatch-name">{t().palettes[props.palette.id]}</span>
    </button>
  );

  /**
   * How the microphone is gated.
   *
   * It used to sit on both the join form and the call panel, because someone
   * who wants push to talk wants it *before* they join and someone who guessed
   * wrong wants it without leaving. The settings screen answers both — it is
   * one click from either place, and a call goes on running underneath it —
   * and it gives the roster its room back.
   */
  const TransmitSettings = () => (
    <div class="field">
      <span class="field-label">{t().transmit}</span>
      <div class="modes">
        <For each={MODES}>
          {(option) => (
            <button
              class="action"
              classList={{ "action-picked": mode() === option }}
              type="button"
              aria-pressed={mode() === option}
              onClick={() => chooseMode(option)}
            >
              {t().transmitMode[option].label}
            </button>
          )}
        </For>
      </div>
      <p class="notice">{t().transmitMode[mode()].hint}</p>
      <Show when={mode() === "push-to-talk"}>
        <button
          class="action"
          classList={{ "action-picked": rebinding() }}
          type="button"
          onClick={() => setRebinding(true)}
        >
          {rebinding() ? t().pressAKey : t().talkKeyIs(keyName(talkKey()))}
        </button>
        <Show when={call()}>
          <p class="notice">
            {globalKey() ? t().keyHeardAnywhere : t().keyHeardHereOnly}
          </p>
        </Show>
      </Show>
    </div>
  );

  return (
    <main class="shell">
      {/* The CRT bezel of the terminal skin. Always in the markup and drawn
          only by that skin's stylesheet — no component branches on a skin
          (styleguide.md §3.3). */}
      <div class="crt" aria-hidden="true" />

      <header class="masthead animate-enter">
        <h1 class="wordmark">
          good<span class="wordmark-accent">voice</span>
        </h1>
        <Show when={info()} fallback={<p class="tagline">{t().starting}</p>}>
          {(loaded) => <p class="tagline">v{loaded().version}</p>}
        </Show>
        <button
          class="action appearance-open"
          type="button"
          aria-pressed={settings()}
          onClick={() => setSettings(!settings())}
        >
          {settings() ? t().back : t().settings}
        </button>
      </header>

      {/* A link that arrived and could not simply be acted on (task 6.2).
          Above everything, because it is about something that just happened
          rather than about what is on screen. */}
      <Show when={invite()}>
        {(offer) => (
          <div class="panel invite animate-enter" role="status">
            {/* The room in bold and the client's own reason after it. The
                reason is Rust's prose and arrives in English in both
                languages — see strings.ts on diagnostics. */}
            <p class="notice">
              {t().inviteLead} <strong>{offer().room}</strong> —{" "}
              {offer().reason}
            </p>
            <div class="modes">
              <Show when={offer().joinable}>
                <button
                  class="action"
                  type="button"
                  onClick={() => void acceptInvite(offer())}
                >
                  {call()
                    ? t().leaveAndJoin(offer().room)
                    : t().tryAgain(offer().room)}
                </button>
              </Show>
              <button
                class="action"
                type="button"
                onClick={() => {
                  setInvite(null);
                  // The client is holding this offer for whichever window asks
                  // next; saying no here has to mean no there too.
                  void invoke("dismiss_invite").catch(() => {});
                }}
              >
                {t().dismiss}
              </button>
            </div>
          </div>
        )}
      </Show>

      <Show when={!settings()} fallback={<Settings />}>
        <Show
          when={call()}
          fallback={
            <form class="panel animate-enter" onSubmit={join}>
              <label class="field">
                <span class="field-label">{t().room}</span>
                <input
                  class="field-input"
                  value={room()}
                  onInput={(event) => setRoom(event.currentTarget.value)}
                  placeholder={t().roomPlaceholder}
                  autocapitalize="none"
                  autocomplete="off"
                  spellcheck={false}
                  disabled={joining()}
                />
              </label>

              <label class="field">
                <span class="field-label">{t().name}</span>
                <input
                  class="field-input"
                  value={name()}
                  onInput={(event) => setName(event.currentTarget.value)}
                  placeholder={t().namePlaceholder}
                  autocomplete="off"
                  maxlength={32}
                  disabled={joining()}
                />
              </label>

              <button
                class="action action-primary"
                type="submit"
                disabled={!canJoin()}
              >
                {joining() ? t().joining : t().join}
              </button>

              <Show when={error()}>
                {(reason) => <p class="notice notice-error">{reason()}</p>}
              </Show>
              <Show
                when={
                  !error() && room() !== "" && !ROOM_CODE.test(room().trim())
                }
              >
                <p class="notice">{t().roomCodeRule}</p>
              </Show>
            </form>
          }
        >
          {(joined) => (
            <section class="panel animate-enter">
              {/* Named, not just shown: "sha-37539" on its own is a
                  puzzle to a screen reader, and to anything else reading this
                  window — docs/testing/invite.ps1 has to tell the room a
                  client is *in* from a room an invite is *offering*. */}
              <p class="tagline" aria-label={t().roomLabel(joined().room)}>
                {joined().room}
              </p>

              <Show when={reconnecting()}>
                {(attempt) => (
                  <p class="notice notice-warn" role="status">
                    {t().reconnecting(attempt())}
                  </p>
                )}
              </Show>

              <ul class="roster">
                <For
                  each={roster()}
                  fallback={<li class="roster-empty">{t().justYou}</li>}
                >
                  {(peer) => (
                    <li class="roster-row">
                      {/* Muted wins over talking: their last few buffered
                        frames can still be playing when the flag arrives, and
                        a dot that says both at once says neither. Otherwise
                        the dot is a level — grey at silence, the theme's
                        accent at full voice, and every shade between. */}
                      <span
                        class="presence"
                        classList={{ "presence-quiet": peer.muted }}
                        style={{
                          "--level": peer.muted
                            ? "0"
                            : String(
                                meterFraction(speaking().get(peer.id) ?? 0),
                              ),
                        }}
                        aria-hidden="true"
                      />
                      <span class="roster-name">{peer.name}</span>
                      <Show when={peer.id === joined().self_id}>
                        <span class="roster-tag">{t().you}</span>
                      </Show>
                      <Show when={peer.muted}>
                        <span class="roster-tag">{t().mutedTag}</span>
                      </Show>
                      {/* Two different facts: muted is "cannot be heard",
                        deafened is "cannot hear you". Someone deafened is
                        still able to talk, so the roster says both. */}
                      <Show when={peer.deafened}>
                        <span class="roster-tag">{t().deafenedTag}</span>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>

              <div class="controls">
                <button
                  class="action"
                  classList={{ "action-on": muted() }}
                  type="button"
                  onClick={toggleMute}
                  aria-pressed={muted()}
                >
                  {muted() ? t().unmute : t().mute}
                </button>
                <button
                  class="action"
                  classList={{ "action-on": deafened() }}
                  type="button"
                  onClick={toggleDeafen}
                  aria-pressed={deafened()}
                >
                  {deafened() ? t().undeafen : t().deafen}
                </button>
              </div>

              <button
                class="action"
                type="button"
                onClick={() => void copyInvite()}
              >
                {(() => {
                  const answer = copied();
                  if (!answer) return t().copyInvite;
                  return answer.ok ? t().inviteCopied : answer.detail;
                })()}
              </button>

              <Show when={sharer()}>
                {(who) => (
                  <button
                    class="action"
                    type="button"
                    onClick={() => void watchScreen()}
                  >
                    {t().watchScreen(who().name)}
                  </button>
                )}
              </Show>

              <Show
                when={(() => {
                  const current = share();
                  return current.state === "sharing" ? current : null;
                })()}
                fallback={
                  <button class="action" type="button" onClick={openPicker}>
                    {t().shareAScreen}
                  </button>
                }
              >
                {(live) => (
                  <div class="sharing">
                    <p class="notice" role="status">
                      {t().sharingAt(
                        live().target,
                        live().width,
                        live().height,
                      )}
                    </p>
                    {/* prd.md §3 F3: a software fallback is allowed and the
                      user must be told. This is the telling. */}
                    <Show when={!live().hardware}>
                      <p class="notice notice-warn" role="status">
                        {t().noHardwareEncoder}
                      </p>
                    </Show>
                    <button
                      class="action action-on"
                      type="button"
                      onClick={stopShare}
                    >
                      {t().stopSharing}
                    </button>
                  </div>
                )}
              </Show>

              <Show
                when={(() => {
                  const current = share();
                  return current.state === "failed" ? current.detail : null;
                })()}
              >
                {(detail) => (
                  <p class="notice notice-error" role="status">
                    {detail()}
                  </p>
                )}
              </Show>

              <Show when={picking()}>
                <div class="picker animate-enter">
                  <p class="field-label">{t().quality}</p>
                  <div class="modes">
                    <For each={QUALITIES}>
                      {(option) => (
                        <button
                          class="action"
                          classList={{
                            "action-picked": quality() === option,
                          }}
                          type="button"
                          onClick={() => pickQuality(option)}
                          aria-pressed={quality() === option}
                          title={t().qualityHint[option]}
                        >
                          {QUALITY_LABEL[option]}
                        </button>
                      )}
                    </For>
                  </div>

                  <p class="field-label">{t().whatToShare}</p>
                  <ul class="targets">
                    <For
                      each={targets()}
                      fallback={
                        <li class="roster-empty">{t().nothingToShare}</li>
                      }
                    >
                      {(target) => (
                        <li>
                          <button
                            class="target"
                            type="button"
                            onClick={() => void startShare(target)}
                          >
                            <span class="target-kind">
                              {t().targetKind[target.kind]}
                            </span>
                            <span class="target-name">{target.name}</span>
                            <span class="target-size">
                              {target.width}×{target.height}
                            </span>
                          </button>
                        </li>
                      )}
                    </For>
                  </ul>

                  <button
                    class="action"
                    type="button"
                    onClick={() => setPicking(false)}
                  >
                    {t().cancel}
                  </button>
                </div>
              </Show>

              <button class="action action-leave" type="button" onClick={leave}>
                {t().leave}
              </button>
            </section>
          )}
        </Show>
      </Show>
    </main>
  );
};

export { App };
export type { CallStatus, ClientInfo, Participant };
