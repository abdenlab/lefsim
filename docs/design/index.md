# Design notes

Design notes record why things are the way they are: the options considered,
the choices made, and what was left for later. They complement the concept
pages, which describe what the code does now.

Notes to come, cleaned up from working drafts:

- **The core refactor**: one state table for worlds, snapshots and replays;
  keyframed event logs; the model JSON format.
- **Effective distance**: the contracted leg graph, rosettes and their express
  lanes, the hop-per-LEF rule, and the reversal fix for hops shorter than
  steps.
- **Keyframed Parquet**: one row group per keyframed chunk, run metadata, and
  random access from the browser.
- **The LEF model's history**: how capture, blocking and eviction got to their
  current form, from instant eviction to competing clocks and protection.
