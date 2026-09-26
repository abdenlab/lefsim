import { defineConfig } from "vite";

export default defineConfig({
  // The wasm glue locates its .wasm with `new URL(..., import.meta.url)`,
  // which dependency pre-bundling would break.
  optimizeDeps: { exclude: ["lefsim-wasm"] },
  build: { target: "es2023" },
});
