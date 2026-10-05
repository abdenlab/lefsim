# lefsim

Action-based stochastic simulation of loop-extrusion factors (LEFs) on a 1D
lattice.

LEFs bind chromatin, extrude loops by walking two legs outward, and are stopped
or turned back by CTCF boundary elements, depending on their orientation.
lefsim simulates this exactly and event by event, with a Rust core and bindings
for Python and the browser. A seed gives the same run, bit for bit, in all
three.

- **Rust**: the `lefsim` crate (`crates/lefsim`).
- **Python**: the `lefsim` package (`python/`, built from `crates/lefsim-py`
  with pyo3 and maturin). Results come back as Arrow data, for pyarrow, polars,
  pandas and duckdb.
- **Browser**: the `lefsim` TypeScript package (`packages/lefsim`, wrapping the
  WebAssembly build in `crates/lefsim-wasm`), with live demos in
  `packages/demo`.

## Documentation

The documentation lives in [`docs/`](docs/index.md): getting started, the
model, recording and replay, effective distance, and a guide per language.
Preview it as a site with [Zensical](https://zensical.org), configured in
`zensical.toml`, without installing anything:

```
uvx zensical serve
```

## A quick look

```python
import lefsim
import polars as pl

lattice = lefsim.Lattice(1000, {"site": [300, 700], "captures_from": ["right", "left"], "strength": 0.9})
model = lefsim.Model(lefsim.populate(lattice, 10, seed=1), lefsim.Rates())
sim = lefsim.Sim(model, seed=42)

rec = sim.record_until(1000.0, dt=1.0, events=True)
pl.DataFrame(rec.snapshots)  # time, lef_id, left_pos, right_pos, left/right_mobility
pl.DataFrame(rec.events)  # one row per leg changed by each event
```

The same in [Rust and TypeScript](docs/getting-started.md#a-first-run).

## Development

```
cargo test                                   # the core
cargo run --example single_lef               # a single LEF's trajectory

cd python                                    # the Python package
uv venv && uv pip install maturin pytest pyarrow polars duckdb pandas numpy
maturin develop --uv --release
uv run --no-sync pytest tests

pnpm install                                 # the TypeScript package and demos
pnpm build:wasm                              # wasm-pack → crates/lefsim-wasm/pkg
pnpm test                                    # vitest, against the real wasm build
pnpm demo                                    # the demos, on a Vite dev server
```

## Layout

```
Cargo.toml              workspace
crates/lefsim/          core crate
  src/model/            the model (never depends on sim/):
    mod.rs                Model = World + Rates + SpawnRule
    world.rs              World: the state (lattice + legs and LEFs), LefStates
    lattice.rs, entity.rs sites and CTCFs; legs, LEFs and their keys
    action.rs             Action, Rates, scheduling and firing rules, CTCF landing
    spawn.rs              Spawn, SpawnRule, populate
    distance.rs           LoopGraph: effective distance
  src/sim/              running and observing a model:
    mod.rs                Sim, its constructors, batch drivers
    record.rs             Recorder, EventRecorder, frames
    replay.rs             Replayer
    engine.rs, heap.rs    event loop and indexed heap (internal)
  examples/single_lef.rs
  tests/
crates/lefsim-py/       Python bindings (pyo3): Sim, Batch, Stream; Arrow conversion
python/                 the Python package built from them (maturin)
  lefsim/               package, type stubs
  tests/                pytest suite
crates/lefsim-wasm/     WebAssembly bindings (wasm-bindgen)
packages/lefsim/        TypeScript package wrapping them
packages/demo/          browser demos (Vite)
notebooks/              a tour of the Python package
docs/                   documentation (Zensical, configured in zensical.toml)
```
