# Rust

The `lefsim` crate is the core: the model, the engine, recording and replay. The
other bindings wrap it.

## Building a model

A `Model` is a `World` (the lattice and the LEFs on it), its `Rates`, and a
spawn rule. `Sim` runs a model from a seed: it owns the world, the event engine,
the random number generator and the spawn rule for one run.

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
```

## Running

```rust
// Event by event:
while let Some((t, action)) = sim.step() {
    // ...
}

// Or in batches:
sim.run_until(1000.0);
sim.run_events(10_000);
```

`sim.world()` shows the current state, and `sim.model()` returns it as a new
`Model`, to save or to restart from with `Sim::with_start_time`. `Sim` is
`Clone`, and a clone continues the run bit for bit. For a custom spawn rule, use
`Sim::with_spawn(world, rates, rule, seed)`.

With the `serde` feature, a `Model` (and `World`, `Lattice`, `Rates` and
`LefStates` on their own) saves and loads as the versioned JSON document
described in [Recording and replay](../concepts/recording.md#persistence).

## Recording

The `*_with` drivers feed observers as they run: a `Recorder` takes snapshots on
a grid, and an `EventRecorder` logs deltas.

```rust
let mut rec = sim.recorder(SampleMode::every_dt(1.0));
let mut events = EventRecorder::new();
sim.run_until_with(500.0, &mut (&mut rec, &mut events));

let frames = rec.frames(); // LefFrames: time, then the LEF table per snapshot
let deltas = events.frames(); // EventFrames: time, leg_id, pos, mob, kind
```

Bound a recorder with `with_max_snapshots(n)` or
`EventRecorder::with_max_rows(n)`. When it fills, the driver returns
`StopReason::RecorderFull` before firing another event: drain it and call again.

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

Frames are plain `Vec`s with no Arrow dependency; the bindings wrap them without
copying. `EventRecorder::with_keyframes(n_legs)` opens every batch with a
keyframe.

## Replaying

```rust
let mut replayer = Replayer::new(lattice, n_lefs);
replayer.push(batch, report.time)?; // each keyframed batch, with the time it runs through
replayer.seek(250.0)?;
let states = replayer.lef_states()?; // the state at t = 250
let frames = replayer.snapshots(SampleMode::every_dt(1.0), 400.0); // decode to a grid
let world = replayer.world()?; // start a new run from here
```
