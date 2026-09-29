/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The API the dev server proxies to. Point it at a scratch instance (e.g. a copy of the data dir
// served on another port) to work on the UI without touching the live board.
const api = process.env.AKB_API_TARGET ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: api, changeOrigin: false },
      "/login": { target: api, changeOrigin: false },
    },
  },
  test: {
    environment: "jsdom",
  },
});
