# lefsim

Stochastic, event-driven simulation of loop-extrusion factors (LEFs) on a 1D
lattice of chromatin sites, with CTCF boundaries that capture or evict them.
Simulations are exact: there's no fixed time step, only the next event. A seed
gives the same run, bit for bit, on every platform, and in the Python and
browser bindings too.

```rust
use lefsim::*;

let lattice = Lattice::with_ctcfs(1000, &[
    Ctcf { site: 300, captures_from: Direction::Right, strength: 0.9 },
    Ctcf { site: 700, captures_from: Direction::Left, strength: 0.9 },
])?;
let rule = SpawnRule::UniformNonCtcf;
let world = populate(lattice, 10, &rule, 1)?; // ten LEFs, placed with seed 1
let model = Model::new(world, Rates::default(), rule)?;

let mut sim = Sim::new(model, 42);
sim.run_until(1000.0);
println!("{:?}", sim.world().lef_states());
```

Documentation: <https://abdenlab.github.io/lefsim/>. Bindings: `lefsim` on PyPI
and npm.
