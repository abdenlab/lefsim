// Node can't fetch file: URLs, so load the wasm bytes directly.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

import { initSync } from "../src/index.ts";

const wasmPath = createRequire(import.meta.url).resolve("lefsim-wasm/lefsim_wasm_bg.wasm");
initSync(readFileSync(wasmPath));
