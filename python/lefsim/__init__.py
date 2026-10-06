"""Stochastic simulation of loop-extrusion factors (LEFs) on a 1D lattice.

Describe a model, then run it:

```python
lattice = lefsim.Lattice(3000, {"site": [600, 1300], "captures_from": ["right", "left"], "strength": 0.9})
world = lefsim.populate(lattice, 20, seed=1)  # or lefsim.World(lattice, lefs_table)
model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=2e-3, respawn_blocked=2e-2, respawn_captured=2e-3))
sim = lefsim.Sim(model, seed=1)

sim.run_until(1_000.0)  # advance
rec = sim.record_until(2_000.0, dt=1.0, events=True)  # advance and record
```

Recorded data comes back as [`Batch`][lefsim.Batch] objects: Arrow record
batches exposed through the Arrow PyCapsule Interface, so they can be passed
straight to `pyarrow.table`, `polars.DataFrame` or queried from duckdb, without
this package depending on any of them. [`Sim.stream`][lefsim.Sim.stream] runs
lazily as an Arrow stream, and [`Replayer`][lefsim.Replayer] rebuilds states
from a recorded event log.
"""

from ._lefsim import (
    Batch,
    Lattice,
    Model,
    Rates,
    Recording,
    Replayer,
    RunReport,
    Sim,
    Stream,
    World,
    __version__,
    populate,
)

__all__ = [
    "Batch",
    "Lattice",
    "Model",
    "Rates",
    "Recording",
    "Replayer",
    "RunReport",
    "Sim",
    "Stream",
    "World",
    "__version__",
    "populate",
]
