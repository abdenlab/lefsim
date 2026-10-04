"""Stochastic simulation of loop-extrusion factors (LEFs) on a 1D lattice.

Describe a model, then run it::

    lattice = lefsim.Lattice(3000, ctcf_sites=[600, 1300], ctcf_captures_from=["left", "right"])
    world = lefsim.populate(lattice, 20, seed=1)       # or lefsim.World(lattice, lefs_table)
    model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=2e-3, respawn_stalled=2e-2, respawn_evicted=2e-1, respawn_captured=2e-3))
    sim = lefsim.Sim(model, seed=1)

    sim.run_until(1_000.0)                             # advance
    rec = sim.record_until(2_000.0, dt=1.0, events=True)   # advance and record

Recorded data comes back as :class:`Batch` objects: Arrow record batches
exposed through the Arrow PyCapsule Interface, so they can be passed straight
to ``pyarrow.table``, ``polars.DataFrame`` or queried from duckdb, without
this package depending on any of them. :meth:`Sim.stream` runs lazily as an
Arrow stream, and :class:`Replayer` rebuilds states from a recorded event log.
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
