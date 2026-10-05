# lefsim

Action-based stochastic simulation of loop-extrusion factors (LEFs) on a 1D
lattice.

LEFs bind chromatin, extrude loops by walking two legs outward, and are stopped
or turned back by CTCF boundary elements, depending on their orientation. lefsim
simulates this exactly and event by event, with a Rust core and bindings for
Python and the browser. A seed gives the same run, bit for bit, in all three.

- **Rust**: the `lefsim` crate (`crates/lefsim`).
- **Python**: the `lefsim` package (`python/`, built from `crates/lefsim-py`
    with pyo3 and maturin). Results come back as Arrow data, for pyarrow,
    polars, pandas and duckdb.
- **Browser**: the `lefsim` TypeScript package (`packages/lefsim`, wrapping the
    WebAssembly build in `crates/lefsim-wasm`), with live demos in
    `packages/demo`.

## Documentation

The documentation lives in [`docs/`](docs/index.md): getting started, the model,
recording and replay, effective distance, and a guide per language. Preview it
as a site with [Zensical](https://zensical.org), configured in `zensical.toml`,
without installing anything:

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
uv sync                                      # venv, dependencies and the extension
uv run pytest tests                          # rebuilds first if the Rust code changed

pnpm install                                 # the TypeScript package and demos
pnpm build:wasm                              # wasm-pack → packages/lefsim/wasm
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
  notebooks/            a tour of the package (not shipped in the sdist)
crates/lefsim-wasm/     WebAssembly bindings (wasm-bindgen)
packages/lefsim/        TypeScript package wrapping them (wasm-pack builds into wasm/)
packages/demo/          browser demos (Vite)
docs/                   documentation (Zensical, configured in zensical.toml)
scripts/                release helpers: bump and check the version
.github/workflows/      CI, docs deployment, releases
```

## Releasing

The `lefsim` crate, the Python wheel and the npm package share one version and
are released together. While it's 0.x, bump the minor version for any breaking
API change in any language *or any change to seeded trajectories* (a new golden
hash), and the patch version for anything else.

1. On `main`, with CI green, set the version and start its changelog section:

    ```
    scripts/bump-version.sh 0.2.0
    ```

    This updates `Cargo.toml` (which the wheel reads too),
    `packages/lefsim/package.json` and `Cargo.lock`, and turns the "Unreleased"
    section of `CHANGELOG.md` into a `0.2.0` section.

1. Review `CHANGELOG.md`, then commit, tag and push:

    ```
    git commit -am "Release 0.2.0"
    git tag v0.2.0
    git push origin main v0.2.0
    ```

The tag starts `.github/workflows/release.yml`, which:

- checks that the tag matches the version, and runs the full CI suite;
- builds the wheels (Linux, musllinux, macOS and Windows, x86_64 and arm64, each
    tested on its own platform where a runner exists), the sdist and the npm
    tarball;
- once all of that passes, publishes to crates.io, PyPI and npm in parallel;
- creates a GitHub release with the changelog section and the built files.

A version with a pre-release suffix (`0.2.0-rc.1`) is published as a
pre-release: PyPI gets `0.2.0rc1`, and npm gets the `next` dist-tag, so
`npm install lefsim` still installs the latest stable version.

Each publish job skips a registry that already has the version, so if one fails,
fix the cause and re-run the workflow. A registry never accepts the same version
twice, so never move a tag that reached any registry: bump to the next patch
version instead.

### One-time setup

Publishing uses trusted publishing (OIDC), so the repository stores no tokens.
Each registry is told which repository, workflow (`release.yml`) and GitHub
environment may publish:

- **GitHub:** create the environments `crates-io`, `pypi` and `npm` (Settings →
    Environments), optionally requiring approval. Under Settings → Pages, set
    the source to "GitHub Actions" for the docs.
- **PyPI:** add a pending trusted publisher for the project `lefsim`
    (environment `pypi`). It works before the project exists.
- **crates.io and npm:** both need the package to exist before trusted
    publishing can be configured, so publish the first version by hand:
    `cargo publish -p lefsim` and, after `pnpm build:wasm`,
    `pnpm --filter lefsim publish`. Then add the trusted publisher in each
    package's settings (environments `crates-io` and `npm`). After that, push
    the tag as usual: the workflow skips those two, publishes to PyPI and
    creates the GitHub release.
