# lefsim

![Loops extruded by LEFs, drawn as arcs over the lattice](assets/banner-light.svg#only-light)
![Loops extruded by LEFs, drawn as arcs over the lattice](assets/banner-dark.svg#only-dark)

**`lefsim`** models and simulates the process of chromatin **loop extrusion** by
SMC-family complexes (e.g. cohesin and condensin).

In this system, **loop-extrusion factors** (LEFs) operate on a 1D lattice of
chromatin sites, some of which may be **extrusion barrier** elements. Each LEF
binds at a site and walks its two legs outward, extruding a loop, until it
unbinds, encountering obstacles or barriers along the way. Upon unbinding it
loads again elsewhere, simulating a steady state LEF density.

Simulations are exact and **event-driven**: there's no fixed time step, only the
next event.

The core is a Rust library, with bindings for two other environments:

|         | Package              | Built with     | Results as                                   |
| ------- | -------------------- | -------------- | -------------------------------------------- |
| Rust    | `lefsim` crate       |                | plain vectors                                |
| Python  | `lefsim` wheel       | pyo3 + maturin | Arrow data (pyarrow, polars, pandas, duckdb) |
| Browser | `lefsim` npm package | wasm-bindgen   | typed arrays, Arrow JS                       |

All three run the same core, so a seed gives the same run, bit for bit, in each.

## What it does

- **Runs a model**: a lattice with CTCFs, LEFs on it, and the rates of their
    actions, from a seed. You can step event by event, run to a time, or stream
    a run far too long to hold in memory.
- **Records it**: snapshots of every LEF on a time grid, or a lossless log of
    every change, with keyframes so any part can be replayed on its own.
- **Replays it**: rebuild the state at any recorded time, from memory, Arrow
    streams, or Parquet files (in the browser, reading only the parts it needs).
- **Measures what loops do**: the effective distance between any two sites when
    loops bring their anchors together.

## Where to go next

- [Getting started](getting-started.md): install, and a first run in each
    language.
- [The LEF model](concepts/model.md): what the legs and CTCFs do, and when LEFs
    unbind.
- [The framework](concepts/framework.md): the event-driven engine underneath.
- [Recording and replay](concepts/recording.md): snapshots, event logs,
    keyframes and files.
- The guides for [Rust](guides/rust.md), [Python](guides/python.md) and
    [TypeScript](guides/typescript.md), and the
    [browser demos](guides/demos.md).
