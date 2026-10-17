import { defineConfig } from "vite";
import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  // Tauri has no Node.js server for SSR, so the site is built with adapter-static and a
  // fallback to index.html, which puts it in SPA mode.
  // See https://svelte.dev/docs/kit/single-page-apps and https://v2.tauri.app/start/frontend/sveltekit/
  plugins: [sveltekit({ adapter: adapter({ fallback: "index.html" }) })],

  // Rolldown cannot follow the `"extends": "$app/tsconfig"` of SvelteKit 3 and then fails to
  // prebundle the SSR dependencies of the dev server, so it does not read tsconfig there.
  environments: { ssr: { optimizeDeps: { rolldownOptions: { tsconfig: false } } } },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
