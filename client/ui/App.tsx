import { createResource, Show, type Component } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `ClientInfo` in `src-tauri/src/lib.rs`. */
interface ClientInfo {
  name: string;
  version: string;
}

const App: Component = () => {
  const [info] = createResource(() => invoke<ClientInfo>("client_info"));

  return (
    <main class="shell animate-enter">
      <h1 class="wordmark">
        good<span class="wordmark-accent">voice</span>
      </h1>
      <Show when={info()} fallback={<p class="tagline">starting…</p>}>
        {(loaded) => <p class="tagline">v{loaded().version}</p>}
      </Show>
    </main>
  );
};

export { App };
export type { ClientInfo };
