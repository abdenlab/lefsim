# The framework

The engine borrows its shape from the "Next Reaction Method" variant of the
Gillespie algorithm [Gibson and Bruck, 2000](https://doi.org/10.1021/jp993732q):
a priority queue of upcoming events, each with a next-firing time, drives a
global clock forward one event at a time. Only the event that fired, and the
events that depend on what it changed, are rescheduled.

## Actions

An `Action` is a schedulable event, expressed as an enum whose variants carry
keys pointing to the entities they act on.

```rust
enum Action {
    Step    { leg: LegKey },
    Respawn { lef: LefKey },
    Evict   { leg: LegKey },
}
```

Each action instance is its own priority-queue key: two instances of a variant
pointing to the same entity are equal. Every action exposes two methods:

- `schedule(&World, ..., now, &mut Rng) -> Schedule`: **pure**. Given the
    current world state and clock, when would this action next fire? Returns
    `Schedule::At(t)` or `Schedule::Never`. It must not mutate.
- `fire(&mut World, ..., now, &mut Rng, &mut Touched)`: **mutates**. Applies the
    event's effects to the world and pushes every leg or LEF whose state (or
    scheduling-relevant neighborhood) changed into `Touched`.

Pushing every affected entity into `Touched` is critical for correctness: an
action left out keeps a schedule computed from a state that no longer holds.

## Dependencies

After `fire`, the engine turns the touched entities into the actions to
reschedule. The entity-to-action map is fixed: each leg has one `Step` and one
`Evict` action, and each LEF one `Respawn` action. `Touched::dependent_actions`
encodes that mapping, listing each action once, starting with the one that
fired.

Rescheduling draws a fresh exponential wait. That's statistically exact, because
exponential waits don't depend on how long you've already waited, but it means
the order and number of reschedules are part of a seeded trajectory.

## Entity arenas

Rust's borrow checker has trouble with graph-shaped data where lots of things
point at lots of other things. LEFs are exactly that: a LEF points at two legs,
each leg points back at its LEF, and each lattice site's occupancy list points
at legs. Instead of references, entities live in arenas (`SlotMap<K, V>`):
lookup tables addressed by small, cheap-to-copy keys handed out on insertion.

- Legs: one leg key points to a leg (position, facing, mobility, its LEF).
- LEFs: one LEF key points to a pair of leg keys.
- Lattice occupancy: one site index points to 0, 1 or 2 leg keys (two only when
    a freshly placed LEF has both legs on one site).

LEFs are recycled: when one unbinds, it rebinds elsewhere. No entities are
created or destroyed during a run.

## The engine loop

With this in place, the main loop is short:

```
loop:
  (t, action) = heap.peek()
  if t is +∞: stop
  clock = t
  touched.clear()
  action.fire(world, ..., touched)
  for dep in touched.dependent_actions(action):   # the fired action first
      reschedule(dep)
```

Rescheduling is one call to `Action::schedule` and a priority update on the
heap, which stays a flat binary heap with an index from each action to its
position.

## Reproducibility

A seed fixes the whole run, bit for bit, across platforms, including wasm32. The
only transcendental function in the simulation, the logarithm in the exponential
sampler, comes from the pure-Rust `libm` crate so that it rounds the same
everywhere.

To ensure deterministic reproducibility, the core crate contains a pinned
"golden hash" of a long run that the Python and TypeScript test suites must
reproduce.
