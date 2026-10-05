# lefsim

Stochastic, event-driven simulation of loop-extrusion factors (LEFs) on a 1D
lattice of chromatin sites, in WebAssembly. Results come as typed arrays or
Arrow JS, and recordings replay from Parquet files over HTTP.

```
npm install lefsim
```

```ts
import { createLattice, createModel, init, populate, Sim } from "lefsim";

await init(); // load the WebAssembly module
const lattice = createLattice(1000, [
  { site: 300, capturesFrom: "right", strength: 0.9 },
  { site: 700, capturesFrom: "left", strength: 0.9 },
]);
const model = createModel(populate(lattice, 10, { seed: 1 }));

using sim = new Sim(model, { seed: 42 });
sim.runUntil(1000);
const { leftPos, rightPos } = sim.state(); // views into wasm memory
```

`init()` with no arguments fetches the module next to the package, which works
in browsers and bundlers. In Node, pass the bytes:

```ts
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

await init(await readFile(fileURLToPath(import.meta.resolve("lefsim/lefsim_wasm_bg.wasm"))));
```

A seed gives the same run, bit for bit, here, in Rust and in Python.

Documentation: <https://abdenlab.github.io/lefsim/>
