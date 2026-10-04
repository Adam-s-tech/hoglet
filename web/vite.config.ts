import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

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

export default defineConfig({
  plugins: [react()],
  base: "/",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
  },
  server: {
    proxy: {
      "/api": origin,
      "/capture": origin,
      "/i/v0/e": origin,
    },
  },
});
