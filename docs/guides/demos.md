# Browser demos

`packages/demo` is the lefsim explorer: a Svelte app that runs the simulator in
the browser. It's live at <https://lefsim.abdenlab.org>, deployed from `main` to
Cloudflare by `.github/workflows/demo.yml`. To run it locally:

```
pnpm install
pnpm build:wasm      # wasm-pack → packages/lefsim/wasm
pnpm demo            # Vite dev server
```

The app has three parts:

- **The shelf** runs the simulation (play, speed, restart) and holds the model:
    the number of loop extruders, the CTCF strength, the four rates and the
    seed. Edits apply while the run continues: the LEFs carry over to the new
    model at the current time.
- **The stage** draws the loops as arcs over the lattice, and the CTCFs in two
    lanes, one per direction. Click a lane to add a CTCF, drag one to move it,
    or drag it out of the lanes to remove it.
- **The views**, one tab each:
    - **Kymograph**: every leg's position over time.
    - **Occupancy**: mean legs per site, averaged over time, on a log scale.
    - **Contact map**: the contact probability of every pair of lattice bins,
        d<sup>−3/2</sup> of their effective distance as for a Gaussian chain.
        Aligned with the stage, it hangs under the loops as a triangle on the same
        x axis; square, the current map is above the diagonal and the time average
        below.
    - **Distance**: the effective distance between two probes you drag on the
        stage, against their distance along the lattice, and as a trace.
    - **Encounters**: the same two probes' encounters (distance within a threshold
        you set), sampled every time unit: intervals in a kymograph and durations
        in a histogram.
    - **Encounter map**: encounters of every pair of bins at once: frequency below
        the diagonal, mean duration above.

Only the open view measures: cumulative views pause while hidden and report the
time they've averaged over.

Two older pages remain until the app can open recordings: **Replay**
(`/replay.html`), which records a run as keyframed chunks and plays it back from
a `Replayer`, and **Parquet file** (`/parquet.html`), which opens a file written
by the Python binding, reading only the row groups it needs.
