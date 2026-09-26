"""Regenerate cross-language fixtures from the Python binding.

Run with the lefsim-py dev venv:

    crates/lefsim-py/.venv/bin/python packages/lefsim/test/fixtures/make_fixtures.py

The TS tests check that the wasm build, driven through the TS wrapper,
reproduces these values exactly (both bindings wrap the same core, and the
core is bit-identical across native and wasm32).
"""

import json
import pathlib

import lefsim
import pyarrow as pa

HERE = pathlib.Path(__file__).parent

# Same model as crates/lefsim/tests/determinism.rs.
sites = list(range(25, 500, 50))
config = {
    "nSites": 500,
    "ctcfSites": sites,
    "ctcfCapturesFrom": ["right" if i % 100 == 25 else "left" for i in sites],
    "ctcfStrength": 0.7,
    "nLefs": 20,
    "seed": 0x5EED,
    "rates": {"step": 1.0, "respawn": 0.01, "respawnCaptured": 0.002},
    "tEnd": 1000.0,
}
sim = lefsim.Sim(
    config["nSites"],
    ctcf_sites=config["ctcfSites"],
    ctcf_captures_from=config["ctcfCapturesFrom"],
    ctcf_strength=config["ctcfStrength"],
    n_lefs=config["nLefs"],
    seed=config["seed"],
    step_rate=config["rates"]["step"],
    respawn_rate=config["rates"]["respawn"],
    respawn_rate_captured=config["rates"]["respawnCaptured"],
)
sim.run_until(config["tEnd"])
snap = pa.record_batch(sim.snapshot())
expected = {
    "time": sim.time,
    "eventCount": sim.event_count,
    "nextEventTime": sim.next_event_time,
    "leftPos": snap["left_pos"].to_pylist(),
    "rightPos": snap["right_pos"].to_pylist(),
    "leftMobility": snap["left_mobility"].indices.to_pylist(),
    "rightMobility": snap["right_mobility"].indices.to_pylist(),
}
out = HERE / "determinism_state.json"
out.write_text(json.dumps({"config": config, "expected": expected}, indent=1) + "\n")
print("wrote", out)
