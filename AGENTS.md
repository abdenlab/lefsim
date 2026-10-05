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

## Commands

- Rust: `cargo test`,
    `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
    `cargo fmt --all`
- Python, from `python/`: `uv run pytest tests`. `uv sync` and `uv run` rebuild
    the extension whenever the Rust code changes (`tool.uv.cache-keys` in
    `python/pyproject.toml`). Go through `uv run`, not the venv's python, which
    may hold a stale build.
- TypeScript: `pnpm build:wasm` (after any Rust change), then `pnpm test` and
    `pnpm typecheck`
- Docs: `uvx zensical build --strict`. Format Markdown with
    `uvx --with mdformat-mkdocs --with mdformat-tables mdformat docs/ README.md`
- Versions: `scripts/bump-version.sh <version>`; never edit versions by hand.
