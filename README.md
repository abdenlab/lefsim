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
  - otherwise → `rates.respawn_default`.

#### Firing rules

When `Respawn` is fired, the LEF's legs are lifted off their current sites, a
new spawn site is chosen by the `SpawnRule`, and both legs are placed there
facing outward. `SpawnRule` is a small trait. The crate ships
`UniformNonCtcfSpawn`, which rejection-samples until it lands on an unoccupied
`Plain` site. Models that need a nonuniform or precomputed-free-list rule
implement the trait.

On each `Step` or `Respawn` fire, the mobility of the leg(s) is reassessed using
a `resolve_mobility(site, arrived_from, rng)` function. `arrived_from` is the
side of the landing site the leg came from. This is the single place where a
CTCF can act on a leg. Its `captures_from` determines its capture side (the
opposite side evicts). If the site is a CTCF:

- if `arrived_from == captures_from` → with probability `strength` return
  `Captured` else `Moving` (pass-through).
- if `arrived_from != captures_from` → return `Evicted`.

This is called _only_ once at actual landings, never during scheduling.
Otherwise a leg standing still on a CTCF that already failed to capture it would
get fresh rolls every time the scheduler revisited it, which is physically
wrong.

## Quick example

```rust
use lefs::*;
use rand::SeedableRng;

// 100-site lattice with a convergent CTCF pair at 30 and 70.
let mut sites = vec![Site::plain(); 100];
sites[30] = Site::ctcf(Direction::Right, 1.0);
sites[70] = Site::ctcf(Direction::Left, 1.0);

let mut rng = SimRng::seed_from_u64(42);
let mut world = World::new(Lattice::new(sites));
world.spawn_lef(50, &mut rng);

let mut engine = Engine::new(Rates::default());
engine.initialize(&world, &mut rng);

let mut touched = Touched::default();
while let Some((t, action)) =
    engine.step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
{
    // record trajectory, break on some condition, etc.
    # let _ = (t, action); break;
}
```

Run the bundled trajectory printer with:

```
cargo run --example single_lef
```
