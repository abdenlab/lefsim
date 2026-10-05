// Node can't fetch file: URLs, so load the wasm bytes directly.
import { readFileSync } from "node:fs";

import { initSync } from "../src/index.ts";

initSync(readFileSync(new URL("../wasm/lefsim_wasm_bg.wasm", import.meta.url)));
