import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri drives this dev server; the port is fixed so tauri.conf.json can point at it.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // src-tauri is Rust; Vite reloading on it only costs CPU.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "chrome110",
    sourcemap: false,
  },
});
