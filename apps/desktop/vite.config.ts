import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri sets these while running `tauri dev` / `tauri build`.
const host = process.env.TAURI_DEV_HOST;
const platform = process.env.TAURI_ENV_PLATFORM;

export default defineConfig({
  plugins: [react()],
  // Keep Rust compiler output visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // WebView2 on Windows (Chromium), WebKit on macOS/Linux.
    target: platform === "windows" ? "chrome105" : "safari13",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    // Assets are loaded from disk by the desktop app, not over the network.
    chunkSizeWarningLimit: 2048,
  },
});
