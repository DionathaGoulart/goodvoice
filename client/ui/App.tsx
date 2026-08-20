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

/** The event `push_roster` emits. Kept in step with `ROSTER_EVENT`. */
const ROSTER_EVENT = "goodvoice://roster";

/** The event `push_state` emits. Kept in step with `STATE_EVENT`. */
const STATE_EVENT = "goodvoice://state";

/**
 * The room code the server will accept: `roomCodeSchema` in
 * `server/src/protocol.ts`. Checked here only so a typo is a hint rather than
 * a round trip — the server validates regardless.
 */
const ROOM_CODE = /^[a-zA-Z0-9-]{4,24}$/;

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

  // Subscribed once for the life of the window: the room the events belong to
  // is whichever call is open, and there is only ever one.
  const stopping = [
    listen<Participant[]>(ROSTER_EVENT, (event) => setRoster(event.payload)),
    listen<CallHealth>(STATE_EVENT, (event) => {
      const { self_id, ...state } = event.payload;
      setHealth(state);
      // A reconnect takes a new seat, so the id the roster marks as "you"
      // changes mid-call.
      setCall((current) => (current ? { ...current, self_id } : current));
      if (state.state === "ended") {
        setCall(null);
        setRoster([]);
        if (state.reason !== "left") {
          setError(state.detail);
        }
      }
    }),
  ];
  onCleanup(() => {
    for (const pending of stopping) {
      void pending.then((stop) => stop());
    }
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
      });
      setCall(status);
      setRoster(status.participants);
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
  };

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
                    <span
                      class="presence"
                      classList={{ "presence-quiet": peer.muted }}
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
