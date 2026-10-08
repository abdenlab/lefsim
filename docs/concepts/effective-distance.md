# Effective distance

Loops bring their anchors together, so two sites far apart on the lattice can be
close in space. We can approximate this by calculating the **effective
distance**, which is the shortest linear path between between two sites that
walks the lattice and may also cross LEFs.

## The rules

- A **step** moves one site along the lattice and costs `step_length` (s).
- A **hop** crosses a LEF, from one leg to the other, and costs `hop_length`
    (h).
- Both are integers you choose; neither has a default. s must be positive, h can
    be 0. Distances are therefore exact integers.
- Only leg positions matter: whether a leg is moving or captured, and where the
    CTCFs are, doesn't.

**Rosettes.** Two LEFs are linked when one's right leg sits at site p and the
other's left leg at p + 1. A chain of linked LEFs is a rosette. Crossing it
costs one hop per LEF crossed, and the junctions between linked LEFs are free.
Walking across a junction is still an ordinary step.

For linked LEFs 1 (legs l₁, r₁) and 2 (l₂, r₂), with h ≥ s:

| Between   | Distance | Route                               |
| --------- | -------- | ----------------------------------- |
| r₁ and l₂ | s        | the junction step                   |
| r₁ and r₂ | s + h    | step, then hop across LEF 2         |
| l₁ and r₂ | 2h       | across the chain, the junction free |
| l₁ and l₂ | h + s    | hop across LEF 1, then step         |

!!! warning "Keep hops at least as long as steps"

    With `hop_length < step_length`, a path can double back across a LEF it just
    crossed and beat a direct step: r₁ to l₂ becomes min(s, 4h). Nothing currently
    prevents this; a fix is sketched in the design notes.

## How it's computed

The lattice is reduced to a graph of leg positions: consecutive legs are joined
by the walk between them, and each rosette gets two one-way "express lanes", one
per direction, that carry a path across its LEFs at a hop each. A site joins the
graph through its nearest leg on each side. Distances come from Dijkstra's
algorithm, run once per distinct entry leg when many sites are asked for
together. At a million sites with 5,000 LEFs, all pairs of 1,000 sites take
about a second.

## Using it

=== "Rust"

    ```rust
    let graph = LoopGraph::new(sim.world(), 1, 1)?; // step_length, hop_length
    let d = graph.distance(100, 2500)?;
    let pairs = graph.pairwise(&sites)?; // condensed upper triangle
    ```

=== "Python"

    ```python
    w = sim.world()
    w.effective_distance(100, 2500, step_length=1, hop_length=1)  # int
    d = w.effective_distances(sites, step_length=1, hop_length=1)  # all pairs, as scipy's pdist
    times, d = replayer.effective_distances(sites, dt=10.0, step_length=1, hop_length=1)  # over time
    ```

    Results are `array.array("q")`, which numpy wraps without copying.

=== "TypeScript"

    ```ts
    const lengths = { stepLength: 1, hopLength: 1 };
    effectiveDistance(sim.world(), 100, 2500, lengths); // number
    effectiveDistances(sim.world(), sites, lengths); // all pairs, as scipy's pdist
    effectiveDistancesOver(lattice, snapshots, sites, lengths); // one row of pairs per snapshot
    ```

    Results are `Float64Array`s, exact because the lengths are checked so no
    distance can pass 2^53.
