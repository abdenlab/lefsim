# Agent notes

lefsim: event-driven loop-extrusion simulation. A Rust core (`crates/lefsim`)
with Python (`crates/lefsim-py` → `python/`) and WebAssembly/TypeScript
(`crates/lefsim-wasm` → `packages/lefsim`) bindings, plus browser demos
(`packages/demo`).

Read as needed:

- `docs/concepts/model.md`: the current LEF rules (the source of truth for
    behavior)
- `docs/concepts/framework.md`: the engine (actions, scheduling, `Touched`, the
    heap)
- `docs/concepts/recording.md`: snapshots, event deltas, keyframes, file formats
- `docs/design/vision.md`: the original design brief and its motivation
- README, "Releasing": versioning and the release workflow

## Invariants

- A seed fixes a run bit for bit in all three languages. A golden hash pins it
    in `crates/lefsim/tests/determinism.rs`, `python/tests/test_lefsim.py` and
    `packages/lefsim/test/golden.test.ts`. Any change that alters trajectories
    (rules, rates, random draws, reschedule order) must update all three, and is
    a minor version bump with a CHANGELOG note.
- `Action::schedule` is pure. `fire` must push every entity whose
    scheduling-relevant state changed into `Touched`; a missing one leaves a
    stale schedule, not an error.
- Transcendental functions come from `libm`, not `std`, and the simulation path
    has no hash-ordered iteration (no `HashMap`/`HashSet`). Both keep runs
    identical across platforms, including wasm32.

## Changes that travel together

- A core API or model change goes through the pyo3 bindings and the `.pyi`
    stubs, the wasm bindings, the TS wrapper (`packages/lefsim/src`, including
    the code/name tables in `types.ts`), the docs, and the demos if they use it.
- Cross-language fixtures in `packages/lefsim/test/fixtures` are generated from
    Python (`make_fixtures.py`) and checked by both the Python and TS suites.
- Python docstrings live in the Rust doc comments in `crates/lefsim-py`; after
    changing one, run `make stubs` to copy it into `_lefsim.pyi` (a test fails
    until you do). Write them in Markdown.
- Every export of the TypeScript package needs a TSDoc comment: the API
    reference build (`make docs-api`, TypeDoc) fails on undocumented items and
    broken `{@link}`s.

## Commands

The Makefile wraps these; `make` lists its targets. Versions:
`scripts/bump-version.sh <version>`; never edit versions by hand.
