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

/** Mirrors `ClientInfo` in `src-tauri/src/lib.rs`. */
interface ClientInfo {
  name: string;
  version: string;
  server: string;
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

/** The event `push_roster` emits. Kept in step with `ROSTER_EVENT`. */
const ROSTER_EVENT = "goodvoice://roster";

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

/**
 * The room code the server will accept: `roomCodeSchema` in
 * `server/src/protocol.ts`. Checked here only so a typo is a hint rather than
 * a round trip — the server validates regardless.
 */
const ROOM_CODE = /^[a-zA-Z0-9-]{4,24}$/;

/** Mirrors `TransmitMode` in `src-tauri/src/audio/vad.rs`. */
type TransmitMode = "open" | "push-to-talk" | "voice-activity";

/** The modes, in the order they take control away from the microphone. */
const MODES: { id: TransmitMode; label: string; hint: string }[] = [
  {
    id: "open",
    label: "open",
    hint: "the microphone is live until you mute",
  },
  {
    id: "push-to-talk",
    label: "push to talk",
    hint: "heard only while the key below is held",
  },
  {
    id: "voice-activity",
    label: "voice",
    hint: "heard only while you are talking",
  },
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

/**
 * What a fresh install holds down. Space is reachable without a chord and is
 * not a shortcut anywhere in this window.
 */
const DEFAULT_TALK_KEY = "Space";

const isMode = (value: string | null): value is TransmitMode =>
  MODES.some((mode) => mode.id === value);

/** `KeyboardEvent.code` as something a person would recognise on a keycap. */
const keyName = (code: string) =>
  code
    .replace(/^(Key|Digit)/, "")
    .replace(/(Left|Right)$/, " $1")
    .toLowerCase();

const App: Component = () => {
  const [info] = createResource(() => invoke<ClientInfo>("client_info"));

  const [room, setRoom] = createSignal("");
  const [name, setName] = createSignal("");
  const [joining, setJoining] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const [call, setCall] = createSignal<CallStatus | null>(null);
  const [roster, setRoster] = createSignal<Participant[]>([]);
  const [muted, setMuted] = createSignal(false);
  const [deafened, setDeafened] = createSignal(false);
  const [health, setHealth] = createSignal<CallState>({ state: "live" });
  // Ids, not names: two people in a room may share a name, and the id is what
  // the roster rows are keyed on anyway.
  const [speaking, setSpeaking] = createSignal<ReadonlySet<string>>(
    new Set<string>(),
  );

  const savedMode = localStorage.getItem(MODE_STORE);
  const [mode, setMode] = createSignal<TransmitMode>(
    isMode(savedMode) ? savedMode : "open",
  );
  const [talkKey, setTalkKey] = createSignal(
    localStorage.getItem(TALK_KEY_STORE) ?? DEFAULT_TALK_KEY,
  );
  const [rebinding, setRebinding] = createSignal(false);

  // Subscribed once for the life of the window: the room the events belong to
  // is whichever call is open, and there is only ever one.
  const stopping = [
    listen<Participant[]>(ROSTER_EVENT, (event) => setRoster(event.payload)),
    listen<string[]>(SPEAKING_EVENT, (event) =>
      setSpeaking(new Set(event.payload)),
    ),
    listen<Controls>(CONTROLS_EVENT, (event) => {
      setMuted(event.payload.muted);
      setDeafened(event.payload.deafened);
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
        setSpeaking(new Set<string>());
        if (state.reason !== "left") {
          setError(state.detail);
        }
      }
    }),
  ];

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
      setSpeaking(new Set<string>());
      setMuted(false);
      setDeafened(false);
      setHealth({ state: "live" });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setJoining(false);
    }
  };

  const leave = async () => {
    await invoke("leave_room");
    setCall(null);
    setRoster([]);
    setSpeaking(new Set<string>());
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
      void invoke("set_transmit_mode", { mode: next }).catch(() => {});
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
   * How the microphone is gated. On both panels: someone who wants push to
   * talk wants it *before* they join, and someone who guessed wrong wants it
   * without leaving.
   */
  const TransmitSettings = () => (
    <div class="field">
      <span class="field-label">transmit</span>
      <div class="modes">
        <For each={MODES}>
          {(option) => (
            <button
              class="action"
              classList={{ "action-picked": mode() === option.id }}
              type="button"
              aria-pressed={mode() === option.id}
              onClick={() => chooseMode(option.id)}
            >
              {option.label}
            </button>
          )}
        </For>
      </div>
      <p class="notice">{MODES.find((it) => it.id === mode())?.hint}</p>
      <Show when={mode() === "push-to-talk"}>
        <button
          class="action"
          classList={{ "action-picked": rebinding() }}
          type="button"
          onClick={() => setRebinding(true)}
        >
          {rebinding()
            ? "press a key, or escape"
            : `key: ${keyName(talkKey())}`}
        </button>
      </Show>
    </div>
  );

  return (
    <main class="shell">
      <header class="masthead animate-enter">
        <h1 class="wordmark">
          good<span class="wordmark-accent">voice</span>
        </h1>
        <Show when={info()} fallback={<p class="tagline">starting…</p>}>
          {(loaded) => <p class="tagline">v{loaded().version}</p>}
        </Show>
      </header>

      <Show
        when={call()}
        fallback={
          <form class="panel animate-enter" onSubmit={join}>
            <label class="field">
              <span class="field-label">room</span>
              <input
                class="field-input"
                value={room()}
                onInput={(event) => setRoom(event.currentTarget.value)}
                placeholder="squad-night"
                autocapitalize="none"
                autocomplete="off"
                spellcheck={false}
                disabled={joining()}
              />
            </label>

            <label class="field">
              <span class="field-label">name</span>
              <input
                class="field-input"
                value={name()}
                onInput={(event) => setName(event.currentTarget.value)}
                placeholder="anon"
                autocomplete="off"
                maxlength={32}
                disabled={joining()}
              />
            </label>

            <TransmitSettings />

            <button
              class="action action-primary"
              type="submit"
              disabled={!canJoin()}
            >
              {joining() ? "joining…" : "join"}
            </button>

            <Show when={error()}>
              {(reason) => <p class="notice notice-error">{reason()}</p>}
            </Show>
            <Show
              when={!error() && room() !== "" && !ROOM_CODE.test(room().trim())}
            >
              <p class="notice">
                4–24 characters, letters, numbers and hyphens only
              </p>
            </Show>
          </form>
        }
      >
        {(joined) => (
          <section class="panel animate-enter">
            <p class="tagline">{joined().room}</p>

            <Show when={reconnecting()}>
              {(attempt) => (
                <p class="notice notice-warn" role="status">
                  reconnecting… (attempt {attempt()})
                </p>
              )}
            </Show>

            <ul class="roster">
              <For
                each={roster()}
                fallback={<li class="roster-empty">just you</li>}
              >
                {(peer) => (
                  <li class="roster-row">
                    {/* Muted wins over talking: their last few buffered
                        frames can still be playing when the flag arrives, and
                        a dot that says both at once says neither. */}
                    <span
                      class="presence"
                      classList={{
                        "presence-quiet": peer.muted,
                        "presence-live": !peer.muted && speaking().has(peer.id),
                      }}
                      aria-hidden="true"
                    />
                    <span class="roster-name">{peer.name}</span>
                    <Show when={peer.id === joined().self_id}>
                      <span class="roster-tag">you</span>
                    </Show>
                    <Show when={peer.muted}>
                      <span class="roster-tag">muted</span>
                    </Show>
                    {/* Two different facts: muted is "cannot be heard",
                        deafened is "cannot hear you". Someone deafened is
                        still able to talk, so the roster says both. */}
                    <Show when={peer.deafened}>
                      <span class="roster-tag">deafened</span>
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
                {muted() ? "unmute" : "mute"}
              </button>
              <button
                class="action"
                classList={{ "action-on": deafened() }}
                type="button"
                onClick={toggleDeafen}
                aria-pressed={deafened()}
              >
                {deafened() ? "undeafen" : "deafen"}
              </button>
            </div>

            <TransmitSettings />

            <button class="action action-leave" type="button" onClick={leave}>
              leave
            </button>
          </section>
        )}
      </Show>
    </main>
  );
};

export { App };
export type { CallStatus, ClientInfo, Participant };
