# Getting started

lefsim isn't published yet, so for now each binding builds from a clone of the
repository.

## Build

=== "Rust"

    The core crate builds with a stable Rust toolchain:

    ```
    cargo build
    cargo test
    cargo run --example single_lef   # prints a single LEF's trajectory
    ```

    Add `features = ["serde"]` to read and write models as JSON.

=== "Python"

    The Python package lives in `python/` and builds the bindings in
    `crates/lefsim-py` with maturin. With [uv](https://docs.astral.sh/uv/):

    ```
    cd python
    uv venv && uv pip install maturin pytest pyarrow polars duckdb pandas numpy
    maturin develop --uv --release
    uv run --no-sync pytest tests
    ```

    It builds one abi3 wheel for Python 3.9 and later, with no Python
    dependencies of its own.

=== "TypeScript"

    The TypeScript package lives in `packages/lefsim` and wraps the WebAssembly
    build of the core, made with wasm-pack. With [pnpm](https://pnpm.io):

    ```
    pnpm install
    pnpm build:wasm   # wasm-pack → crates/lefsim-wasm/pkg
    pnpm test         # vitest, against the real wasm build
    ```

## A first run

A model is a lattice, the LEFs on it, and their rates. This one has a
convergent pair of CTCFs, each capturing legs that arrive from outside the
domain between them.

=== "Rust"

    ```rust
    use lefsim::*;

    let lattice = Lattice::with_ctcfs(1000, &[
        Ctcf { site: 300, captures_from: Direction::Right, strength: 0.9 },
        Ctcf { site: 700, captures_from: Direction::Left, strength: 0.9 },
    ])?;
    let rule = SpawnRule::UniformNonCtcf;
    let world = populate(lattice, 10, &rule, 1)?; // ten LEFs, placed with seed 1
    let model = Model::new(world, Rates::default(), rule)?;

    let mut sim = Sim::new(model, 42);
    sim.run_until(1000.0);
    println!("{:?}", sim.world().lef_states());
    ```

=== "Python"

    ```python
    import lefsim
    import polars as pl

    lattice = lefsim.Lattice(1000, {"site": [300, 700], "captures_from": ["right", "left"], "strength": 0.9})
    world = lefsim.populate(lattice, 10, seed=1)
    model = lefsim.Model(world, lefsim.Rates())

    sim = lefsim.Sim(model, seed=42)
    rec = sim.record_until(1000.0, dt=1.0)  # advance, taking a snapshot every time unit
    pl.DataFrame(rec.snapshots)            # time, lef_id, left_pos, right_pos, mobilities
    ```

=== "TypeScript"

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

Next, [the LEF model](concepts/model.md) explains what happens during that run.
