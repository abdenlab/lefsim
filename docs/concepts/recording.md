# Recording and replay

Only leg positions and mobilities change during a run, and only at events.
State is constant in between, so it can be sampled exactly at any time, with no
interpolation.

## Two kinds of record

- **Snapshots** of every LEF on a regular time grid. A snapshot is the world's
  LEF table (left and right leg side by side, since the loop is the observable)
  plus a `time` column.
- **Event deltas**: one row per leg changed by each event,
  `(time, leg_id, pos, mobility, action)`. A step changes one leg; a respawn
  and an eviction (which unbinds the LEF) place both. Lossless and compact.

A grid point records the state after every event at or before it. Recording
never draws random numbers, so a run fires the same events whether or not it's
observed, and splitting a run in two records exactly what one call would.

LEFs and legs are identified by dense ids: `lef_id` runs over `0..n_lefs`, and
`leg_id = 2 * lef_id + side`, with side 0 the left leg and 1 the right.

## Bounded recording and streams

A recorder can be bounded. When it fills, the run returns *before* firing
another event, so nothing is lost: hand off the chunk and continue. In Python
and TypeScript, `sim.stream(...)` wraps this as a lazy stream that runs only as
it's read, so a run of any length takes bounded memory.

## Keyframes

With keyframes, every batch of event deltas opens with one `Keyframe` row per
leg holding the full state, so each batch can be replayed on its own. That's
what seeking and joining a stream partway through need. Rows carry absolute
values, so replaying is an idempotent overwrite, and dropping the keyframe rows
leaves exactly the plain event log.

## Replaying

A **replayer** rebuilds the state at any recorded time from keyframed batches,
with no simulation:

- `advance_to(t)` moves forward through the data received so far.
- `seek(t)` jumps anywhere, by restoring the nearest keyframe at or before `t`.
- `snapshots(dt)` decodes the log to a time grid: the same table a recorder on
  that grid would have produced.
- `world()` gives the state as a world to start a new run from. Such a run
  continues the original faithfully in distribution, though not event for
  event, which would need the original random state.

Replayers exist in all three languages. The TypeScript one is plain TypeScript,
with no WebAssembly, so a page can replay streamed data without loading the
simulator.

## Files

The Python binding writes keyframed event logs to **Parquet**, one row group per
keyframed chunk, compressed with zstd. The file's metadata records where the run
came from: `lefsim.model`, the model it started from, and `lefsim.run`, with the
seed, start time and recorded span. A replayer opens such a file from its footer
alone, then reads only the row groups each call needs. In the browser, it does
so over HTTP with range requests, so a recording of any size can be scrubbed
without downloading it.

Models also save and load as a versioned JSON document, the same in all three
languages:

```json
{
  "format": "lefsim.model", "version": 2,
  "lattice": { "n_sites": 100,
               "ctcfs": [{ "site": 30, "captures_from": "right", "strength": 0.8 }] },
  "lefs": [{ "left_pos": 10, "right_pos": 12,
             "left_mobility": "moving", "right_mobility": "moving" }],
  "rates": { "step": 1.0, "respawn": 0.001, "respawn_blocked": 0.01, "respawn_captured": 0.001 },
  "spawn": { "rule": "uniform_non_ctcf" }
}
```

Loading goes through the same checks as building in code, so a document that
describes an invalid model is rejected with the reason.
