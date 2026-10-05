# lefsim

Stochastic, event-driven simulation of loop-extrusion factors (LEFs) on a 1D
lattice of chromatin sites, with CTCF boundaries that capture or evict them. The
core is written in Rust. Results come back as Arrow data, readable by pyarrow,
polars, pandas and duckdb without copying, and the package has no Python
dependencies.

```
pip install lefsim
```

```python
import lefsim
import polars as pl

lattice = lefsim.Lattice(1000, {"site": [300, 700], "captures_from": ["right", "left"], "strength": 0.9})
world = lefsim.populate(lattice, 10, seed=1)
model = lefsim.Model(world, lefsim.Rates())

sim = lefsim.Sim(model, seed=42)
rec = sim.record_until(1000.0, dt=1.0)  # advance, taking a snapshot every time unit
pl.DataFrame(rec.snapshots)            # time, lef_id, left_pos, right_pos, mobilities
```

A seed gives the same run, bit for bit, here, in Rust and in the browser.

Documentation: <https://abdenlab.github.io/lefsim/>
