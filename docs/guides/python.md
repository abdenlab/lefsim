# Python

`crates/lefsim-py` wraps the core as a Python extension with pyo3, and `python/`
is the `lefsim` package built from it: one abi3 wheel for Python 3.9 and later.
The classes mirror the Rust types. Results come back as Arrow data through the
[Arrow PyCapsule Interface](https://arrow.apache.org/docs/format/CDataInterface/PyCapsuleInterface.html),
so pyarrow, polars and duckdb read them directly, without copying, and the
package itself has no Python dependencies.

## A run

```python
import lefsim
import polars as pl

lattice = lefsim.Lattice(1000, {"site": [300, 700], "captures_from": ["right", "left"], "strength": 0.9})
world = lefsim.populate(lattice, 10, seed=1)  # or lefsim.World(lattice, table_of_lefs)
model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=1e-3, respawn_blocked=1e-2, respawn_captured=1e-3))
sim = lefsim.Sim(model, seed=42)

sim.run_until(100.0)  # advance only
rec = sim.record_until(500.0, dt=1.0, events=True)  # advance and record

snaps = pl.DataFrame(rec.snapshots)  # time, lef_id, left_pos, right_pos, left/right_mobility
deltas = pl.DataFrame(rec.events)  # time, leg_id, lef_id, pos, mobility, action
pl.DataFrame(sim.state())  # the current state, one row per LEF
```

## Tables in and out

A `Lattice` takes its CTCFs, and a `World` its LEFs, as any Arrow table
(pyarrow, polars, pandas) or a dict of columns: `site`, `captures_from` and
optional `strength` for CTCFs (single values apply to all), and `left_pos`,
`right_pos` and optional `left_mobility`/`right_mobility` for LEFs.
`lattice.to_arrow()` and `world.to_arrow()` give the tables back, so
`lefsim.World(lattice, sim.state())` works too.

Snapshots, `sim.state()` and `world.to_arrow()` are all the same LEF table
(snapshots and states add `time`). Pass `layout="leg"` for one row per leg
instead, the same row shape as the event log.

## Saving and forking

`sim.model()` returns the current state as a `Model`, which saves with
`model.to_json()` and loads with `lefsim.Model.from_json(s)` (or `to_dict()` /
`from_dict()` for plain Python data). The model types also pickle.
`copy.copy(sim)` forks a run: the copy continues exactly as the original would.

## Long runs

Runs release the GIL and respond to Ctrl-C. `record_snapshot_chunk` and
`record_event_chunk` record in bounded pieces. For runs too large to hold in
memory, `sim.stream(t_end, dt=...)` turns the simulation into a lazy Arrow
stream that runs as it's read:

```python
sim.stream(1e7, events=True, keyframes=True).to_parquet("events.parquet")  # consumes `sim`
```

`to_parquet` writes one row group per keyframed chunk (zstd by default, with the
event times byte-stream-split, which makes files about a third smaller), and
records where the run came from in the file's metadata. Streams carry the same
metadata in their Arrow schema.

## Replaying

```python
r = lefsim.Replayer.from_parquet("events.parquet")  # reads only the footer
r.world_at(5_000.0)  # reads one row group: a World to start a new run from
pl.DataFrame(r.snapshots(dt=10.0, t_start=4_000.0, t_end=6_000.0))  # a grid, as record_until would have
```

`from_parquet` takes the lattice and end time from the file's metadata, and
reads only the row groups each call needs, so a file of any size opens
instantly. Other Arrow sources load whole with
`Replayer.from_arrow(source, lattice, end=...)`, which accepts any batch
boundaries.

## Effective distance

See [Effective distance](../concepts/effective-distance.md) for the rules.
`World.effective_distance` and `effective_distances` work on one state, and
`Replayer.effective_distances` on a time grid.

## The tour notebook

`notebooks/lefsim_tour.ipynb` walks through a model with a few TADs: leg
trajectories, an occupancy profile, where loops anchor, loop sizes, the event
log, a streamed run, saving, replaying and branching. Run it with the `python/`
venv as its kernel.
