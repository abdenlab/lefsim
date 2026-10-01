import { resolve } from "node:path";

import { defineConfig } from "vite";

export default defineConfig({
  // The wasm glue locates its .wasm with `new URL(..., import.meta.url)`,
  // which dependency pre-bundling would break.
  optimizeDeps: { exclude: ["lefsim-wasm"] },
  build: {
    target: "es2023",
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        replay: resolve(import.meta.dirname, "replay.html"),
        parquet: resolve(import.meta.dirname, "parquet.html"),
        distance: resolve(import.meta.dirname, "distance.html"),
        contact: resolve(import.meta.dirname, "contact.html"),
        encounter: resolve(import.meta.dirname, "encounter.html"),
      },
    },
  },
});
