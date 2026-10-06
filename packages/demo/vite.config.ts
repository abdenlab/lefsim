import { resolve } from "node:path";

import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [svelte()],
  // The wasm glue locates its .wasm with `new URL(..., import.meta.url)`,
  // which dependency pre-bundling would break.
  optimizeDeps: { exclude: ["lefsim"] },
  build: {
    target: "es2023",
    rollupOptions: {
      input: {
        // The explorer app, and two legacy pages kept until the app can open recordings.
        main: resolve(import.meta.dirname, "index.html"),
        replay: resolve(import.meta.dirname, "replay.html"),
        parquet: resolve(import.meta.dirname, "parquet.html"),
      },
    },
  },
});
