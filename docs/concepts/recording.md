# Recording and replay

Only leg positions and mobilities change during a run, and only at events. State
is constant in between, so it can be sampled exactly at any time, with no
interpolation.

## Two kinds of record

- **Snapshots** of every LEF on a time grid. Observations at defined times.
- **Event deltas**: one row per leg changed by each event. Lossless and compact.

The examples below come from one short run: 100 sites, CTCFs at 30 (capturing
from the right) and 70 (capturing from the left), and three LEFs.

**Snapshots.** A snapshot is the world's LEF table, with left and right legs
side by side since the loop is the observable, plus a `time` column. Here are
two time grid points, one time unit apart:

<div class="row-groups" markdown>

| time | lef_id | left_pos | right_pos | left_mobility | right_mobility |
| ---- | ------ | -------- | --------- | ------------- | -------------- |
| 2.0  | 0      | 14       | 16        | Moving        | Moving         |
| 2.0  | 1      | 50       | 56        | Moving        | Moving         |
| 2.0  | 2      | 25       | 29        | Moving        | Moving         |
| 3.0  | 0      | 13       | 17        | Moving        | Moving         |
| 3.0  | 1      | 46       | 56        | Moving        | Moving         |
| 3.0  | 2      | 62       | 62        | Moving        | Moving         |

</div>

**Event deltas.** Each row gives a **leg**'s new state and the action that set
it. These are the events between those same two grid points (times rounded):

| time  | leg_id | lef_id | pos | mobility | action |
| ----- | ------ | ------ | --- | -------- | ------ |
| 2.172 | 1      | 0      | 17  | Moving   | Step   |
| 2.422 | 2      | 1      | 49  | Moving   | Step   |
| 2.493 | 4      | 2      | 62  | Moving   | Evict  |
| 2.493 | 5      | 2      | 62  | Moving   | Evict  |
| 2.493 | 2      | 1      | 48  | Moving   | Step   |
| 2.565 | 0      | 0      | 13  | Moving   | Step   |
| 2.791 | 2      | 1      | 47  | Moving   | Step   |
| 2.838 | 2      | 1      | 46  | Moving   | Step   |

A step changes one leg. A respawn or an eviction places both legs of the LEF at
its new site, so it writes two rows with the same time. At 2.493, LEF 2's right
leg, at 29, stepped toward the CTCF at 30 from its evict side, and the LEF
reloaded at site 62 with both legs on that one site.

A time grid point records the state after every event at or before it. Recording
doesn't modify the random state, so a simulation run with the same seed and
initial state fires the same events regardless of how they are recorded.
Similarly, a run that is paused and resumed will record the same events as an
uninterrupted one.

LEFs and legs are identified by dense ids: `lef_id` runs over `0..n_lefs`, and
`leg_id = 2 * lef_id + side`, with `side` 0 for the left leg and 1 for the
right.

## Bounded recording and streaming

A recorder can have a bounded capacity. When it fills, the run returns *before*
firing another event, so nothing is lost: it can hand off the chunk to the
caller and continue where it left off. In Python and TypeScript,
`sim.stream(...)` wraps this as a lazy stream that runs only as it's read, so a
run of any length will consume bounded memory.

## Keyframes

Keyframes are optional snapshots included in the event delta log as a set of
rows, one per leg in the system.

We prepend batches of event deltas with a full keyframe, such that every batch
opens with one `Keyframe` pseudo-action row per leg, thus encoding the full
world state. This way, each batch can be replayed on its own, allowing for
seeking across a stream or collection of batches, or for playing from a stream
already partway through. Rows carry absolute values, so replaying is an
idempotent overwrite, and dropping the keyframe rows leaves exactly the plain
event log.

Here is the start of a keyframed batch from the same run, recorded from t = 2.
The six `Keyframe` rows restate the snapshot at 2.0 one leg at a time, and the
event deltas follow as before:

<div class="keyframe-rows" markdown>

| time  | leg_id | lef_id | pos | mobility | action   |
| ----- | ------ | ------ | --- | -------- | -------- |
| 2.0   | 0      | 0      | 14  | Moving   | Keyframe |
| 2.0   | 1      | 0      | 16  | Moving   | Keyframe |
| 2.0   | 2      | 1      | 50  | Moving   | Keyframe |
| 2.0   | 3      | 1      | 56  | Moving   | Keyframe |
| 2.0   | 4      | 2      | 25  | Moving   | Keyframe |
| 2.0   | 5      | 2      | 29  | Moving   | Keyframe |
| 2.172 | 1      | 0      | 17  | Moving   | Step     |
| 2.422 | 2      | 1      | 49  | Moving   | Step     |
| 2.493 | 4      | 2      | 62  | Moving   | Evict    |
| …     |        |        |     |          |          |

</div>

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

## Persistence

### Simulations - Parquet

The Python binding writes keyframed event logs to **Parquet**, one row group per
keyframed batch, compressed with zstd. The file's metadata stores where the run
came from: `lefsim.model`, the model it started from, and `lefsim.run`, with the
seed, start time and recorded span.

A replayer can open the model metadata from the file's footer, and then scans
only the row groups it needs. In the browser, it does so over HTTP with range
requests, so a recording of any size can be scrubbed without downloading it.

### Models - JSON

Models (as in the `lefsim.model` serialization above) also save and load as a
standalone versioned JSON document, described by a
[JSON Schema](../schemas/model.v2.json), the same in all three languages:

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

Moreover, when a model is loaded in lefsim it is validated, which covers what a
schema cannot express: CTCF sites and leg positions inside the lattice, legs in
order, no two LEFs on one site, captured legs on a CTCF that captures from their
side, enough free sites for the spawn rule, etc. As a result, a valid model JSON
decodes to a valid simulation state, and a simulation snapshot at any time can
be serialized as a valid model JSON.

??? note "The schema"

    ```json
    --8<-- "docs/schemas/model.v2.json"
    ```
