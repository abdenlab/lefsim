"""Regenerate cross-language fixtures from the Python binding.

Run with the lefsim-py dev venv, from crates/lefsim-py:

    uv run --no-sync python ../../packages/lefsim/test/fixtures/make_fixtures.py

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


def make_sim(config):
    """The Sim that the TS `new Sim(config)` builds: LEFs placed with the
    seed, then run with the same seed."""
    lattice = lefsim.Lattice(
        config["nSites"],
        ctcf_sites=config["ctcfSites"],
        ctcf_captures_from=config["ctcfCapturesFrom"],
        ctcf_strength=config["ctcfStrength"],
    )
    rates = config["rates"]
    model = lefsim.Model(
        lefsim.populate(lattice, config["nLefs"], seed=config["seed"]),
        lefsim.Rates(rates["step"], rates["respawn"], rates["respawnCaptured"]),
    )
    return lefsim.Sim(model, seed=config["seed"])


sim = make_sim(config)
sim.run_until(config["tEnd"])
snap = pa.record_batch(sim.state())
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


# ---------------------------------------------------------------- schemas
# A neutral description of every table the Python binding emits, which the
# TS Arrow builders must reproduce (names, types, nullability, dictionary
# values, schema metadata).

NEUTRAL = {pa.float64(): "float64", pa.uint32(): "uint32", pa.uint8(): "uint8", pa.string(): "utf8"}


def describe(batch) -> dict:
    rb = pa.record_batch(batch)
    fields = []
    for f in rb.schema:
        if pa.types.is_dictionary(f.type):
            kind = f"dict<{NEUTRAL[f.type.index_type]},{NEUTRAL[f.type.value_type]}>"
            dictionary = rb.column(f.name).dictionary.to_pylist()
        else:
            kind, dictionary = NEUTRAL[f.type], None
        fields.append({"name": f.name, "type": kind, "nullable": f.nullable, "dictionary": dictionary})
    metadata = {k.decode(): v.decode() for k, v in (rb.schema.metadata or {}).items()}
    return {"metadata": metadata, "fields": fields}


small_lattice = lefsim.Lattice(200, ctcf_sites=[60, 140], ctcf_captures_from=["right", "left"])
small = lefsim.Sim(lefsim.Model(lefsim.populate(small_lattice, 4, seed=1)), seed=1)
schemas = {
    "lattice": describe(small_lattice.to_arrow()),
    "lefs": describe(small.world().to_arrow()),
    "snapshots_lef": describe(small.record_until(10.0, dt=1.0).snapshots),
    "snapshots_leg": describe(small.record_until(20.0, dt=1.0, layout="leg").snapshots),
    "events": describe(small.record_until(30.0, events=True, keyframes=True).events),
}
out = HERE / "schemas.json"
out.write_text(json.dumps(schemas, indent=1) + "\n")
print("wrote", out)


# ---------------------------------------------------------------- IPC stream
# A keyframed event log written as an Arrow IPC stream, the way a server
# would send it. The TS tests replay it and compare with the wasm Sim.

ipc_config = {**config, "tEnd": 300.0, "chunkSize": 400}
streamed = make_sim(ipc_config)
reader = pa.RecordBatchReader.from_stream(
    streamed.stream(ipc_config["tEnd"], events=True, keyframes=True, chunk_size=ipc_config["chunkSize"])
)
out = HERE / "python_events.arrows"
with pa.OSFile(str(out), "wb") as sink, pa.ipc.new_stream(sink, reader.schema) as writer:
    n_batches = 0
    for b in reader:
        writer.write_batch(b)
        n_batches += 1
(HERE / "python_events.json").write_text(json.dumps({"config": ipc_config, "batches": n_batches}, indent=1) + "\n")
print("wrote", out, f"({n_batches} batches, {out.stat().st_size:,} bytes)")
