# lefsim

Action-based stochastic simulation of loop-extrusion factors (LEFs) on a 1D
lattice.

## The framework

The engine borrows its shape from the "next reaction method" variant of the
Gillespie algorithm: a priority queue of upcoming events, each with a
next-firing time, drives a global clock forward one event at a time.

### Actions

An `Action` is a schedulable event, expressed as an enum whose variants carry
keys pointing to the entities they act on.

```rust
enum Action {
    Step    { leg: LegKey },
    Respawn { lef: LefKey },
}
```

In the current architecture, each action instance is its own unique
priority-queue key, and two action instances of a particular variant pointing to
the same entity keys are considered equal.

Every action instance exposes two methods:

- `schedule(&World, ..., now, &mut Rng) -> Schedule` — **pure**. Given the
  current world state and clock, when would this action next fire? Returns
  `Schedule::At(t)` or `Schedule::Never`. Must not mutate.
- `fire(&mut World, ..., now, &mut Rng, &mut Touched)` — **mutates**. Applies
  the event's effects to the world and pushes every leg/LEF whose state (or
  scheduling-relevant neighborhood) changed into `Touched`.

Note that pushing all necessary entity keys into `Touched` in the implementation
of `fire` is critical for logical correctness.

Dependent actions after `fire`:

In the current architecture, the entity-to-action dependency map is a constant:
each leg corresponds to exactly one `Step` action, each LEF corresponds to
exactly one `Respawn` action. The `Touched::dependent_actions` method hard-codes
that mapping directly in order to translate touched entities into an iterable of
actions that require rescheduling.

### Entity arenas

Rust's borrow checker rules have trouble dealing with graph-shaped data where
lots of things point at lots of other things. Our LEFs are exactly that: a LEF
points at two legs, each leg points back at its LEF and its partner leg, and the
per-site occupancy list also points at legs. To avoid passing around references,
we instead use "arena" containers (`SlotMap<K, V>`), which are like efficient
lookup tables where we address each entity by a cheap-to-copy (integer) key
returned upon insertion in the arena.

### The engine loop

With this architecture in place, the main simulation loop is very simple:

```
loop:
  (t, action) = heap.peek()
  if t is +∞: stop
  clock = t
  touched.clear()
  action.fire(world, ..., touched)
  reschedule(action)                              # the fired action itself
  for dep_action in touched.dependent_actions():  # everything the fired action invalidated
      reschedule(dep_action)
```

Rescheduling is a single call to `Action::schedule` followed by a priority
update on the heap.

## The model

### World model

A LEF (loop extrusion factor) is a protein complex that binds a chromatin
polymer and extrudes a loop by walking two "legs" outward from a shared binding
site. In the simulation:

- The **lattice** is a 1D array of `Site`s. Most are `Plain`; some are
  `Ctcf { captures_from, strength }`, representing chromatin boundary elements
  with an intrinsic polarity. The number of lattice sites is fixed and the
  character of the sites themselves is immutable for the lifetime of a run.
- A **LEF** consists of two **legs** that face outward from a shared starting
  position. Under the current model each leg only steps "forward" (in its
  `facing` direction); backsliding is not scheduled.
- A leg's **mobility** is set at the moment of landing on a site and then
  persists until the next landing:
  - `Moving`: free to step.
  - `Captured`: a CTCF has held it in place.
  - `Evicted`: a CTCF has kicked it off, forcing an immediate respawn.

So we have 1 mutable entity type: leg. A leg's properties: its lattice site, its
facing direction, and its mobility state all live in the `Leg` struct. Lattice
occupancy is maintained as a separate vec of `SmallVec<[LegKey; 2]>`, where each
lattice site points to 1 or more leg keys. These are stack-allocated vecs of
length 2 since each site holds 0 or 1 legs, and only briefly 2 right after a
respawn.

Note that LEFs are "recycled" in the current model: upon eviction or unbinding,
they respawn. No new entities get created and none get destroyed over a run.

Arenas:

- Legs: one leg key points to a leg object.
- LEFs: one LEF key points to a pair of leg keys.
- Lattice: one site index points to 0, 1, or 2 leg keys.

### Actions

#### Scheduling rules

- `Step { leg }`
  - schedules to `Never` if the leg isn't `Moving`, would step off the lattice,
    or is blocked by another leg on the target site.
  - otherwise it draws an exponential wait with rate `rates.step`.
- `Respawn { lef }` looks at the mobility of _both_ legs together:
  - either leg `Evicted` → `At(now)` (immediate).
  - either leg `Captured` → exponential wait at `rates.respawn_captured` (longer
    lifetime while paused at a boundary).
  - otherwise → `rates.respawn`.

#### Firing rules

When `Respawn` is fired, the LEF's legs are lifted off their current sites, a
new spawn site is chosen by the spawn rule, and both legs are placed there
facing outward. A spawn rule implements the small `Spawn` trait: `sample`
picks a site, and `check` says whether the rule can always find one for a
given lattice and number of LEFs. The built-in rules are the `SpawnRule`
enum; for now that's `SpawnRule::UniformNonCtcf`, which rejection-samples
until it lands on an unoccupied `Plain` site (and needs 2n − 1 plain sites
for n LEFs). Models that need a nonuniform or precomputed-free-list rule
implement the trait. The free function `populate` uses a rule to place the
starting LEFs, exactly as respawns would.

On each `Step` or `Respawn` fire, the mobility of the leg(s) is reassessed using
a `resolve_mobility(site, arrived_from, rng)` function. `arrived_from` is the
side of the landing site the leg came from. This is the single place where a
CTCF can act on a leg. Its `captures_from` determines its capture side (the
opposite side evicts). If the site is a CTCF:

- if `arrived_from == captures_from` → with probability `strength` return
  `Captured` else `Moving` (pass-through).
- if `arrived_from != captures_from` → with probability `strength` return `Evicted`.

This is called _only_ once at actual landings, never during scheduling.
Otherwise a leg standing still on a CTCF that already failed to capture it would
get fresh rolls every time the scheduler revisited it, which is physically
wrong.

## Quick example

A `Model` is a `World` (the lattice and the LEFs on it), its `Rates`, and a
spawn rule. `Sim` runs a model from a seed: it owns the world, the event engine,
the RNG, and the spawn rule for one run.

```rust
use lefsim::*;

// 100-site lattice with a convergent CTCF pair at 30 and 70.
let lattice = Lattice::with_ctcfs(100, &[
    Ctcf { site: 30, captures_from: Direction::Right, strength: 1.0 },
    Ctcf { site: 70, captures_from: Direction::Left, strength: 1.0 },
])?;

// Four LEFs at sites drawn from the spawn rule (placement has its own seed)...
let rule = SpawnRule::UniformNonCtcf;
let world = populate(lattice, 4, &rule, 7)?;
// ...or one by one, each leg with a position and mobility:
// let mut world = World::new(lattice);
// world.add_lef((50, Mobility::Moving), (50, Mobility::Moving))?;

let model = Model::new(world, Rates::default(), rule)?;
let mut sim = Sim::new(model, 42);

// Event by event:
while let Some((t, action)) = sim.step() {
    let (left, right) = sim.lef_positions(0);
    // ...
}

// Or in batches:
sim.run_until(1000.0);
sim.run_events(10_000);
```

A world is validated as it's built, so it's always a state the dynamics could
be in: legs on the lattice and not crossed, no two LEFs on one site, and a
`Captured` leg only on a CTCF that could have captured it. `Model::new` checks
the rates and that the spawn rule can always place the LEFs.

`sim.world()` shows the current state, and `sim.model()` returns it as a new
`Model` (to save, or to restart from with `Sim::with_start_time`). `Sim` is
`Clone`, and a clone continues the run bit for bit. For a custom spawn rule,
use `Sim::with_spawn(world, rates, rule, seed)`.

Externally, LEFs and legs are identified by dense ids in the world's LEF order:
`lef_id` runs over `0..n_lefs`, and `leg_id = 2 * lef_id + side`, where side 0
is the left-facing leg and side 1 the right-facing one.

With the `serde` feature, a `Model` saves and loads as a versioned JSON
document (and so do `World`, `Lattice`, `Rates` and `LefStates` on their own):

```json
{
  "format": "lefsim.model", "version": 1,
  "lattice": { "n_sites": 100,
               "ctcfs": [{ "site": 30, "captures_from": "right", "strength": 0.8 }] },
  "lefs": [{ "left_pos": 10, "right_pos": 12,
              "left_mobility": "moving", "right_mobility": "moving" }],
  "rates": { "step": 1.0, "respawn": 0.001, "respawn_captured": 0.0001 },
  "spawn": { "rule": "uniform_non_ctcf" }
}
```

Loading goes through the same constructors as building in code, so a document
that describes an invalid model is rejected with the reason.

Run the bundled trajectory printer with:

```
cargo run --example single_lef
```

## Observing a run

Only leg positions and mobilities change during a run, and they change only at
events. State is piecewise-constant in between, so it can be sampled exactly at
any time without interpolation. Two recorders are fed by the `*_with` drivers:

- `Recorder` takes **snapshots** of every LEF on a regular time grid
  (`SampleMode::every_dt(dt)`). A snapshot is the world's LEF table
  (`LefStates`: left and right leg together per LEF, since the loop is the
  observable), and `LefFrames` stacks them: `time` has one entry per
  snapshot, and each table column has `n_lefs` entries per snapshot, so it
  reshapes to an `(n_snapshots, n_lefs)` array.
- `EventRecorder` logs **deltas**: one row `(time, leg_id, pos, mob, kind)` per
  leg changed by each event. Lossless and compact; replay it over a base
  snapshot to reconstruct any state.

```rust
let mut rec = sim.recorder(SampleMode::every_dt(1.0));
let mut events = EventRecorder::new();
sim.run_until_with(500.0, &mut (&mut rec, &mut events));

let frames = rec.frames(); // LefFrames: time, left_pos, right_pos, left_mob, right_mob
let deltas = events.frames(); // EventFrames: time, leg_id, pos, mob, kind
```

A grid point `g` records the state after _every_ event at or before `g`. So a
zero-delay chain (an eviction and its immediate respawn) is fully applied before
a coincident grid point is recorded, and snapshots never show the transient
`Evicted` state. Recording never touches the RNG, so a run fires the same
events whether or not it is observed, and split runs (`run_until(a)` then
`run_until(b)`) record exactly what one `run_until(b)` would.

**Streaming.** Bound a recorder with `with_max_snapshots(n)` (or
`EventRecorder::with_max_rows(n)`). When it fills, the driver returns
`StopReason::RecorderFull` _before_ firing another event, so nothing is lost:
drain it and call again.

```rust
let mut rec = sim.recorder(SampleMode::every_dt(1.0)).with_max_snapshots(10_000);
let mut chunk = LefFrames::default();
loop {
    let report = sim.run_until_with(1e6, &mut rec);
    rec.drain_into(&mut chunk); // hand `chunk` off, e.g. as an Arrow batch
    if report.stopped != StopReason::RecorderFull {
        break;
    }
}
```

Frames are plain `Vec`s with no Arrow dependency; binding crates wrap them
without copying.

**Keyframes.** `EventRecorder::with_keyframes(n_legs)` opens every batch with
one `Keyframe` row per leg holding the full state, so each batch can be replayed
on its own. That's what seeking and joining a stream partway through need.
Rows carry absolute values, so replay is an idempotent overwrite, and dropping
the keyframe rows leaves exactly the plain event log. Keyframes are written
only at settled times, never partway through an eviction and its immediate
respawn, so they never show a transient `Evicted` leg. For the same reason a
bounded batch may run past its cap by the rows of such a chain (typically two).

**Replaying.** A `Replayer` rebuilds the state at any recorded time from
keyframed batches, with no simulation:

```rust
let mut replayer = Replayer::new(lattice, n_lefs);
replayer.push(batch, report.time)?; // each batch, with the time it runs through
replayer.seek(250.0)?; // any direction; forward with advance_to(t) is cheaper
let states = replayer.lef_states()?; // the state at t = 250
let frames = replayer.snapshots(SampleMode::every_dt(1.0), 400.0); // decode to a grid
let world = replayer.world()?; // start a new run from here
```

`snapshots` gives the same frames a `Recorder` on that grid would have
recorded. A run started from `replayer.world()` (with
`Sim::with_start_time`) continues the original faithfully in distribution,
though not event for event: that would need the original RNG and event queue.
Rows before the first keyframe are skipped, so a replayer can join a stream
partway through, and `trim_before(t)` drops batches it no longer needs.

**Reproducibility.** A seed fixes the whole run, bit for bit, across platforms,
including wasm32. `tests/determinism.rs` pins a golden hash of a long run.

## Python

`crates/lefsim-py` wraps the core as a Python extension (pyo3 + maturin, one
abi3 wheel for Python >= 3.9). The classes mirror the Rust types. Results come
back as Arrow data through the
[Arrow PyCapsule Interface](https://arrow.apache.org/docs/format/CDataInterface/PyCapsuleInterface.html),
so pyarrow, polars and duckdb read them directly, without copying, and the
package itself has no Python dependencies.

```python
import lefsim
import polars as pl

lattice = lefsim.Lattice(1000, {"site": [300, 700], "captures_from": ["right", "left"], "strength": 0.9})
world = lefsim.populate(lattice, 10, seed=1)  # or lefsim.World(lattice, table_of_lefs)
model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=1e-3, respawn_captured=1e-4))
sim = lefsim.Sim(model, seed=42)

sim.run_until(100.0)  # advance only
rec = sim.record_until(500.0, dt=1.0, events=True)  # advance and record

snaps = pl.DataFrame(rec.snapshots)  # time, lef_id, left_pos, right_pos, left/right_mobility
deltas = pl.DataFrame(rec.events)  # time, leg_id, lef_id, pos, mobility, action
pl.DataFrame(sim.state())  # the current state, one row per LEF
```

Both tables go in and out the same way. A `Lattice` takes its CTCFs, and a
`World` its LEFs, as any Arrow table (pyarrow, polars, pandas) or a dict of
columns: `site`, `captures_from` and optional `strength` for CTCFs (single
values apply to all), and `left_pos`, `right_pos` and optional
`left_mobility`/`right_mobility` for LEFs. `lattice.to_arrow()` and
`world.to_arrow()` give the tables back, so `lefsim.World(lattice, sim.state())`
works too.

`sim.model()` returns the current state as a `Model`, which saves with
`model.to_json()` and loads with `lefsim.Model.from_json(s)` (or `to_dict()` /
`from_dict()` for plain Python data); the model types also pickle. `copy.copy(sim)` forks a run: the copy continues exactly as the
original would.

Add `keyframes=True` (with `events=True`) to open each event batch with the full
state. Snapshots, `sim.state()` and `world.to_arrow()` are all the same LEF table
(snapshots and states add `time`); pass `layout="leg"` for one row per leg instead,
the same row shape as the event log (`time, leg_id, lef_id, pos, mobility`). Runs release the
GIL and respond to Ctrl-C. `record_snapshot_chunk` and `record_event_chunk`
record in bounded pieces. For runs too large to hold in memory,
`sim.stream(t_end, dt=...)` turns the simulation into a lazy Arrow stream that
runs as it is read, in bounded memory:

```python
import duckdb
stream = sim.stream(1e7, events=True, keyframes=True)  # consumes `sim`
duckdb.sql("copy (select * from stream) to 'events.parquet'")
```

A `Replayer` reads a keyframed event log back from any Arrow source and
rebuilds the state at any time:

```python
import pyarrow.parquet as pq
r = lefsim.Replayer.from_arrow(pq.ParquetFile("events.parquet").iter_batches(), lattice, end=1e7)
r.world_at(5_000.0)  # a World: start a new run from it
pl.DataFrame(r.snapshots(dt=10.0))  # decode to a grid, as record_until(dt=10.0) would have
```

Develop with:

```
cd crates/lefsim-py
uv venv && uv pip install maturin pytest pyarrow polars duckdb pandas numpy
maturin develop --uv --release
uv run --no-sync pytest tests
```

## Browser (WebAssembly)

`crates/lefsim-wasm` compiles the core to WebAssembly (~37 KB gzipped), and the
`lefsim` TypeScript package in `packages/lefsim` wraps it. A `Sim` there is the
same single realization as in Rust and Python, advanced in place. Each frame,
`state()` returns every LEF's current positions and mobility as typed-array
views over wasm memory, with no copying.

```ts
import { init, Sim } from "lefsim";

await init();
using sim = new Sim({
  nSites: 3000,
  ctcfs: [{ site: 600, capturesFrom: "left", strength: 0.8 }, { site: 601, capturesFrom: "right", strength: 0.8 }],
  nLefs: 20,
  seed: 1,
});

function frame() {
  sim.runUntil(sim.time + 5, { maxEvents: 40_000 }); // absolute time; cap the work per frame
  const { leftPos, rightPos, leftMobility } = sim.state(); // views: valid until the next call into the sim
  // ...draw
  requestAnimationFrame(frame);
}
```

The views are only valid until the next call into the sim, because wasm memory
can grow and invalidate them; copy anything you need to keep. Seeded runs
match the Rust and Python bindings exactly.

`sim.recordEventChunk(tEnd)` records keyframed event deltas as typed-array chunks,
and a `Replayer` rebuilds live state from them in plain TypeScript, with no wasm.
Its `state()` has the same shape as `Sim.state()`, so a renderer doesn't care
whether it's watching a local sim or replaying recorded or streamed data.
`advanceTo(t)` moves forward and stops at the end of the data received so far.
`seek(t)` jumps anywhere by restoring the nearest keyframe.

```ts
const replayer = new Replayer(sim.nLefs);
for (let c = sim.recordEventChunk(1000); ; c = sim.recordEventChunk(1000)) {
  replayer.push(c);
  if (c.stopped !== "recorder_full") break;
}
replayer.seek(250);
const { leftPos, rightPos } = replayer.state();
```

For history rather than live state, `recordUntil(t, { dt, events, keyframes })`
records grid snapshots and/or event deltas as typed-array columns, and
`sim.stream(tEnd, { dt } | { events: true })` returns a `ReadableStream` of
chunks, computed only as it's read. Arrow lives in a separate entry point,
`lefsim/arrow`, so live-only pages don't bundle it. It builds Arrow JS record
batches with exactly the Python binding's schemas, and `toRecordBatches()` turns
a stream into batches:

```ts
import { iterate } from "lefsim";
import { toRecordBatches } from "lefsim/arrow";

for await (const batch of iterate(sim.stream(1e6, { dt: 1 }).pipeThrough(toRecordBatches()))) {
  // batch is an Arrow RecordBatch: time, lef_id, left_pos, right_pos, ...
}
```

`packages/demo` is a live demo: loop arcs over the lattice and a scrolling
kymograph, with speed, LEF count and seed controls, plus a leg-occupancy profile
averaged from a second simulation streamed as Arrow batches in the background.
Its replay page records a run as keyframed chunks and plays it back from a
`Replayer`, with seeking. The loop and kymograph panels are the same drawing
code on both pages; only the source of `state()` differs. To run it:

```
pnpm install
pnpm build:wasm      # wasm-pack → crates/lefsim-wasm/pkg
pnpm demo            # Vite dev server
pnpm test            # vitest, against the real wasm build
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
  src/sim/              running and observing a model:
    mod.rs                Sim, its constructors, batch drivers
    record.rs             Recorder, EventRecorder, frames
    replay.rs             Replayer
    engine.rs, heap.rs    event loop and indexed heap (internal)
  examples/single_lef.rs
  tests/
crates/lefsim-py/       Python bindings
  src/                  Sim, Batch, Stream (pyo3); Arrow conversion
  python/lefsim/        package, type stubs
  tests/                pytest suite
crates/lefsim-wasm/     WebAssembly bindings (wasm-bindgen)
packages/lefsim/        TypeScript package wrapping them
packages/demo/          live browser demo (Vite)
```
