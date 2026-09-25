"""Stochastic simulation of loop-extrusion factors (LEFs) on a 1D lattice.

Build a :class:`Sim`, run it with :meth:`Sim.run_until` / :meth:`Sim.run_events`
(optionally recording grid snapshots and/or event deltas), or turn it into a
lazy Arrow :class:`Stream` with :meth:`Sim.stream`.

Results are :class:`Batch` objects: Arrow record batches exposed through the
Arrow PyCapsule Interface, so they can be passed straight to
``pyarrow.table``, ``polars.DataFrame`` or queried from duckdb, without this
package depending on any of them.
"""

from ._lefsim import Batch, Recording, Sim, Stream, __version__

__all__ = ["Batch", "Recording", "Sim", "Stream", "__version__"]
