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

/** The event `push_roster` emits. Kept in step with `ROSTER_EVENT`. */
const ROSTER_EVENT = "goodvoice://roster";

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

  // Subscribed once for the life of the window: the room the events belong to
  // is whichever call is open, and there is only ever one.
  const unlisten = listen<Participant[]>(ROSTER_EVENT, (event) =>
    setRoster(event.payload),
  );
  onCleanup(() => void unlisten.then((stop) => stop()));

  const canJoin = () => ROOM_CODE.test(room().trim()) && !joining();

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
