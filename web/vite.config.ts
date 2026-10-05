import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Build to web/dist, which rust-embed compiles into the binary.
//
// `npm run dev` runs in "mock" mode: every API call is answered in-process by
// src/mock (generated, realistic data) so every page can be developed without
// a backend. `npm run dev:real` proxies to a running binary
// (HOGLET_ORIGIN, default http://127.0.0.1:8124).
//
// Absolute base: the SPA is served for deep links (/project/…/insights/…), so
// asset URLs must not be relative to the current path.
const origin = process.env.HOGLET_ORIGIN ?? "http://127.0.0.1:8124";

export default defineConfig(({ command }) => ({
  plugins: [react(), tailwindcss()],
  base: "/",
  resolve: {
    alias: {
      "@": resolve(import.meta.dirname, "./src"),
      // TanStack Form's devtools event client is dead weight in the shipped bundle.
      ...(command === "build" ? { "@tanstack/devtools-event-client": resolve(import.meta.dirname, "./src/lib/event-client-stub.ts") } : {}),
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    minify: "terser",
    terserOptions: { compress: { passes: 2, pure_getters: true }, format: { comments: false } },
  },
  server: {
    proxy: {
      "/api": origin,
      "/capture": origin,
      "/i/v0/e": origin,
    },
  },
}));
