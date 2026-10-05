# TypeScript

`crates/lefsim-wasm` compiles the core to WebAssembly (about 57 KB gzipped), and
the `lefsim` TypeScript package in `packages/lefsim` wraps it with the same API
as Python, in camelCase. Lattices, worlds and models are plain objects with
typed-array columns, built by functions that check them through the core, so an
invalid one is reported where it's written.

The WebAssembly module ships inside the npm package. `init()` with no arguments
fetches it from next to the package, which works in browsers and bundlers. Node
can't fetch `file:` URLs, so there you pass the bytes:

```ts
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

await init(await readFile(fileURLToPath(import.meta.resolve("lefsim/lefsim_wasm_bg.wasm"))));
```

## A live run

A `Sim` is the same single run as in Rust and Python, advanced in place. Each
frame, `state()` returns every LEF's positions and mobilities as typed-array
views over wasm memory, with no copying.

```ts
import { createLattice, createModel, init, populate, Sim } from "lefsim";

await init();
const lattice = createLattice(3000, [
  { site: 600, capturesFrom: "left", strength: 0.8 },
  { site: 601, capturesFrom: "right", strength: 0.8 },
]);
const model = createModel(populate(lattice, 20, { seed: 1 }), { step: 1, respawn: 2e-3, respawnBlocked: 2e-2, respawnCaptured: 2e-3 });
using sim = new Sim(model, { seed: 1 });

function frame() {
  sim.runUntil(sim.time + 5, { maxEvents: 40_000 }); // absolute time; cap the work per frame
  const { leftPos, rightPos, leftMobility } = sim.state(); // views: valid until the next call into the sim
  // ...draw
  requestAnimationFrame(frame);
}
```

!!! note "Views into wasm memory"

    The arrays from `state()` are only valid until the next call into the sim,
    because wasm memory can grow and invalidate them. Copy anything you need to
    keep; `sim.world()` returns a copy.

Seeded runs match the Rust and Python bindings exactly. `sim.model()` returns
the current state as a model, `sim.clone()` forks the run exactly, and
`modelToJson` / `modelFromJson` read and write the same model document as Python
and Rust.

## Recording and replaying

`sim.recordEventChunk(tEnd)` records keyframed event deltas as typed-array
chunks, and a `Replayer` rebuilds live state from them in plain TypeScript, with
no wasm. Its `state()` has the same shape as `Sim.state()`, so a renderer
doesn't care whether it's watching a local sim or replaying recorded or streamed
data.

```ts
const replayer = new Replayer(sim.lattice, sim.nLefs);
for (let c = sim.recordEventChunk(1000); ; c = sim.recordEventChunk(1000)) {
  replayer.push(c);
  if (c.stopped !== "recorder_full") break;
}
replayer.seek(250);
const { leftPos, rightPos } = replayer.state();
const branch = new Sim(createModel(replayer.world(), model.rates), { seed: 2, startTime: 250 });
```

For history rather than live state, `recordUntil(t, { dt, events, keyframes })`
records grid snapshots and/or event deltas as typed-array columns, and
`sim.stream(tEnd, ...)` returns a `ReadableStream` of chunks, computed only as
it's read.

## Arrow

Arrow lives in a separate entry point, `lefsim/arrow`, so live-only pages don't
bundle it. It builds Arrow JS record batches with exactly the Python binding's
schemas. `toRecordBatches()` turns a stream into batches, and
`replayerFromArrow(batches, lattice, { end })` loads a keyframed event log from
any Arrow source.

```ts
import { iterate } from "lefsim";
import { toRecordBatches } from "lefsim/arrow";

for await (const batch of iterate(sim.stream(1e6, { dt: 1 }).pipeThrough(toRecordBatches()))) {
  // batch is an Arrow RecordBatch: time, lef_id, left_pos, right_pos, ...
}
```

## Parquet

The `lefsim/parquet` entry point replays files written by the Python binding's
`Stream.to_parquet`, from a URL, a `File` or bytes. It reads the footer, then
only the row groups each move needs, over HTTP with range requests, so a
recording of any size can be scrubbed without downloading it. Reading is
asynchronous, so the methods that move return promises:

```ts
import { replayerFromParquet } from "lefsim/parquet";

const r = await replayerFromParquet("https://example.org/run.parquet");
await r.seek(5000); // fetches one row group
const { leftPos, rightPos } = r.state(); // synchronous, as with a Replayer
```

## Effective distance

See [Effective distance](../concepts/effective-distance.md). `effectiveDistance`
and `effectiveDistances` work on a `World`, and `effectiveDistancesOver` on
recorded snapshots (from `recordUntil` or any replayer's `snapshots`).

## Packaging

The package isn't published yet. For now its exports point at its TypeScript
sources, which works inside this workspace. Publishing it needs a build step
that compiles to JavaScript with type declarations and ships the wasm file
alongside, as one package; apps that use it bundle the wasm as an asset.
