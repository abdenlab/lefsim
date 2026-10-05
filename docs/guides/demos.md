# Browser demos

`packages/demo` is a Vite app with seven pages, all running the simulator in the
browser:

```
pnpm install
pnpm build:wasm      # wasm-pack → crates/lefsim-wasm/pkg
pnpm demo            # Vite dev server
```

- **Live simulation**: loop arcs over the lattice and a scrolling kymograph,
    with speed, LEF count and seed controls, plus a leg-occupancy profile
    averaged from a second simulation streamed as Arrow batches in the
    background.
- **Replay**: records a run as keyframed chunks and plays it back from a
    `Replayer`, with seeking.
- **Parquet file**: opens a file written by the Python binding (pick it, drop
    it, or pass `?url=`), reading only the row groups it needs.
    `packages/demo/make_sample.py` writes a sample into `public/sample.parquet`,
    which the page loads by default.
- **Effective distance**: probe sites and CTCFs you can drag along the lattice,
    and the probes laid out, next to the live loops, by their effective
    distances (step and hop length 1), recomputed every frame.
- **Contact map**: under the live loops, the contact probability of every pair
    of lattice bins, d<sup>−3/2</sup> of their effective distance as for a
    Gaussian chain, with the current state above the diagonal and its time
    average below. All 4.5 million site pairs of the 3,000-site lattice take
    about 30 ms.
- **Encounters**: the effective distance between two probes, sampled every time
    unit, and their encounters (distance within a threshold you set): intervals
    in the kymograph, and durations in a histogram.
- **Encounter map**: the same for every pair of lattice bins at once, from two
    running totals per pair: encounter frequency below the diagonal, mean
    duration above.

The last four pages let you drag and count CTCFs and set the rates and CTCF
strength while the run continues. The loop panel is the same drawing code on
every page, drawn from whatever lattice the source has; only the source of the
state differs.
