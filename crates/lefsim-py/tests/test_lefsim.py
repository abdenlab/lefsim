"""Tests for the lefsim Python bindings."""

import copy
import json
import pathlib
import pickle
import subprocess
import sys
import threading
import _thread

import duckdb
import numpy as np
import pandas as pd
import polars as pl
import pyarrow as pa
import pytest

import lefsim

# The golden fingerprint pinned by crates/lefsim/tests/determinism.rs.
RUST_GOLDEN = 0xE5A22C6618308528

FIXTURES = pathlib.Path(__file__).parents[3] / "packages" / "lefsim" / "test" / "fixtures"

RATES = lefsim.Rates(step=1.0, respawn=0.02, respawn_captured=0.005)


def make_lattice(strength=0.8):
    """200 sites with a convergent CTCF pair plus an extra right-capturing CTCF."""
    return lefsim.Lattice(
        200,
        ctcf_sites=[60, 100, 140],
        ctcf_captures_from=["right", "right", "left"],
        ctcf_strength=strength,
    )


def make_model(seed=1, n_lefs=8, strength=0.8, rates=RATES):
    return lefsim.Model(lefsim.populate(make_lattice(strength), n_lefs, seed=seed), rates)


def make_sim(seed=1, n_lefs=8, strength=0.8):
    return lefsim.Sim(make_model(seed, n_lefs, strength), seed=seed)


def table(obj):
    return pa.table(obj)


# ---------------------------------------------------------------- lattice


def test_lattice_properties_and_table():
    lattice = make_lattice()
    assert (lattice.n_sites, lattice.n_plain) == (200, 197)
    t = table(lattice.to_arrow())
    assert t.to_pydict() == {
        "site": [60, 100, 140],
        "captures_from": ["Right", "Right", "Left"],
        "strength": [0.8, 0.8, 0.8],
    }
    assert t.schema.metadata == {b"lefsim.kind": b"lattice", b"lefsim.n_sites": b"200"}
    assert lefsim.Lattice(10).to_arrow().num_rows == 0


@pytest.mark.parametrize(
    "kwargs, match",
    [
        (dict(n_sites=0), "at least one site"),
        (dict(n_sites=10, ctcf_sites=[3]), "ctcf_captures_from is required"),
        (dict(n_sites=10, ctcf_sites=[10], ctcf_captures_from="left"), "outside"),
        (dict(n_sites=10, ctcf_sites=[3, 3], ctcf_captures_from="left"), "twice"),
        (dict(n_sites=10, ctcf_sites=[3], ctcf_captures_from="up"), "'left' or 'right'"),
        (dict(n_sites=10, ctcf_sites=[3, 4], ctcf_captures_from=["left"]), "1 entries"),
        (dict(n_sites=10, ctcf_sites=[3], ctcf_captures_from="left", ctcf_strength=1.5), r"\[0, 1\]"),
    ],
)
def test_lattice_errors(kwargs, match):
    n_sites = kwargs.pop("n_sites")
    with pytest.raises(ValueError, match=match):
        lefsim.Lattice(n_sites, **kwargs)


def test_captures_from_is_case_insensitive_and_broadcasts():
    lattice = lefsim.Lattice(50, ctcf_sites=[10, 20], ctcf_captures_from="Right", ctcf_strength=0.5)
    ctcfs = table(lattice.to_arrow())
    assert ctcfs["captures_from"].to_pylist() == ["Right", "Right"]
    assert ctcfs["strength"].to_pylist() == [0.5, 0.5]


def test_numpy_inputs_are_accepted():
    lattice = lefsim.Lattice(
        50,
        ctcf_sites=np.array([10, 20]),
        ctcf_captures_from=["left", "right"],
        ctcf_strength=np.array([0.2, 0.9]),
    )
    assert table(lattice.to_arrow())["site"].to_pylist() == [10, 20]


# ---------------------------------------------------------------- world


def lefs(world):
    return table(world.to_arrow()).to_pydict()


def test_populate_places_lefs_reproducibly():
    lattice = make_lattice()
    world = lefsim.populate(lattice, 8, seed=3)
    assert world.n_lefs == 8 and world.lattice == lattice
    rows = lefs(world)
    assert rows["lef_id"] == list(range(8))
    assert rows["left_pos"] == rows["right_pos"]
    assert set(rows["left_mobility"]) == {"Moving"}
    assert lefsim.populate(lattice, 8, seed=3) == world
    assert lefsim.populate(lattice, 8, seed=4) != world


def test_populate_errors():
    with pytest.raises(ValueError, match="too many LEFs"):
        lefsim.populate(lefsim.Lattice(10), 6)
    with pytest.raises(ValueError, match="unknown spawn rule"):
        lefsim.populate(lefsim.Lattice(10), 1, rule="anywhere")


def test_world_from_a_dict_of_columns():
    lattice = make_lattice()
    world = lefsim.World(
        lattice,
        {
            "left_pos": [10, 60],
            "right_pos": [12, 140],
            "left_mobility": ["moving", "Captured"],
            "right_mobility": [0, 1],
        },
    )
    rows = lefs(world)
    assert rows["left_mobility"] == ["Moving", "Captured"]
    assert rows["right_mobility"] == ["Moving", "Captured"]
    # Mobility defaults to moving.
    assert lefs(lefsim.World(lattice, {"left_pos": [5], "right_pos": [7]}))["left_mobility"] == ["Moving"]
    assert lefsim.World(lattice).n_lefs == 0


def test_world_round_trips_through_arrow_tables():
    world = make_sim().world()
    lattice = world.lattice
    batch = world.to_arrow()
    assert lefsim.World(lattice, batch) == world
    assert lefsim.World(lattice, table(batch)) == world
    assert lefsim.World(lattice, pl.DataFrame(batch)) == world
    assert lefsim.World(lattice, table(batch).to_pandas()) == world


def test_a_sim_state_is_a_world():
    sim = make_sim()
    sim.run_until(100.0)
    assert lefsim.World(sim.world().lattice, sim.state()) == sim.world()


@pytest.mark.parametrize(
    "columns, match",
    [
        ({"left_pos": [5], "right_pos": [3]}, "right of the right leg"),
        ({"left_pos": [5, 6], "right_pos": [8, 8]}, "already occupied"),
        ({"left_pos": [300], "right_pos": [300]}, "outside"),
        ({"left_pos": [5], "right_pos": [7], "left_mobility": ["captured"]}, "not a CTCF"),
        # A left-facing leg can't be captured by a CTCF capturing from the left.
        ({"left_pos": [140], "right_pos": [150], "left_mobility": ["captured"]}, "can't be captured"),
        ({"left_pos": [5], "right_pos": [7], "right_mobility": ["evicted"]}, "Evicted"),
        ({"left_pos": [5], "right_pos": [7], "right_mobility": ["stuck"]}, "mobility must be"),
        ({"left_pos": [5]}, "missing column"),
        ({"left_pos": [5, 6], "right_pos": [7]}, "different lengths"),
        ({"left_pos": [-1], "right_pos": [7]}, "non-negative"),
    ],
)
def test_world_errors(columns, match):
    with pytest.raises(ValueError, match=match):
        lefsim.World(make_lattice(), columns)


def test_world_rejects_non_tables():
    with pytest.raises(TypeError, match="Arrow table"):
        lefsim.World(make_lattice(), 42)


# ---------------------------------------------------------------- model


def test_rates_default_to_the_core():
    r = lefsim.Rates()
    assert (r.step, r.respawn, r.respawn_captured) == (1.0, 1e-3, 1e-4)
    assert lefsim.Rates(respawn=0.5) == lefsim.Rates(1.0, 0.5, 1e-4)


def test_model_parts():
    world = lefsim.populate(make_lattice(), 3, seed=1)
    model = lefsim.Model(world, RATES)
    assert model.world == world
    assert model.lattice == world.lattice
    assert model.rates == RATES
    assert model.spawn == "uniform_non_ctcf"
    assert lefsim.Model(world).rates == lefsim.Rates()


@pytest.mark.parametrize(
    "rates, match",
    [
        (lefsim.Rates(step=0.0), "step rate must be positive"),
        (lefsim.Rates(respawn=float("nan")), "respawn rate must be positive"),
        (lefsim.Rates(respawn_captured=-1.0), "respawn_captured rate"),
    ],
)
def test_model_checks_rates(rates, match):
    with pytest.raises(ValueError, match=match):
        lefsim.Model(lefsim.World(lefsim.Lattice(10)), rates)


def test_model_checks_the_spawn_rule():
    world = lefsim.World(lefsim.Lattice(4), {"left_pos": [0, 1, 2], "right_pos": [0, 1, 2]})
    with pytest.raises(ValueError, match="too many LEFs"):
        lefsim.Model(world)
    with pytest.raises(ValueError, match="unknown spawn rule"):
        lefsim.Model(lefsim.World(lefsim.Lattice(4)), spawn="anywhere")


def test_model_json_round_trip_and_shape():
    model = make_model(n_lefs=2)
    doc = json.loads(model.to_json(indent=2))
    assert doc["format"] == "lefsim.model" and doc["version"] == 1
    assert doc["lattice"]["n_sites"] == 200
    assert doc["lattice"]["ctcfs"][0] == {"site": 60, "captures_from": "right", "strength": 0.8}
    assert doc["rates"] == {"step": 1.0, "respawn": 0.02, "respawn_captured": 0.005}
    assert doc["spawn"] == {"rule": "uniform_non_ctcf"}
    assert lefsim.Model.from_json(model.to_json()) == model

    # Loading validates: a left-facing leg captured by the CTCF at 140,
    # which captures from the left, is rejected.
    doc["lefs"] = {
        "left_pos": [140],
        "right_pos": [150],
        "left_mobility": ["captured"],
        "right_mobility": ["moving"],
    }
    with pytest.raises(ValueError, match="can't be captured"):
        lefsim.Model.from_json(json.dumps(doc))


@pytest.mark.parametrize(
    "obj",
    [make_lattice(), lefsim.populate(make_lattice(), 4, seed=2), RATES, make_model()],
    ids=["Lattice", "World", "Rates", "Model"],
)
def test_model_types_pickle_and_json(obj):
    assert pickle.loads(pickle.dumps(obj)) == obj
    assert type(obj).from_json(obj.to_json()) == obj


# ---------------------------------------------------------------- sim


def test_construction_and_properties():
    sim = make_sim(n_lefs=3)
    assert (sim.n_sites, sim.n_lefs, sim.n_legs) == (200, 3, 6)
    assert sim.time == 0.0 and sim.event_count == 0
    assert sim.next_event_time > 0
    assert sim.rates == RATES
    assert sim.model() == make_model(n_lefs=3)


def test_start_time():
    sim = lefsim.Sim(make_model(), seed=1, start_time=1000.0)
    assert sim.time == 1000.0 and sim.next_event_time > 1000.0
    rec = sim.record_until(1010.0, dt=5.0)
    assert np.unique(table(rec.snapshots)["time"].to_numpy()).tolist() == [1000.0, 1005.0, 1010.0]
    with pytest.raises(ValueError, match="start_time"):
        lefsim.Sim(make_model(), start_time=float("inf"))


def test_the_model_is_copied_in_and_read_back_as_the_current_state():
    model = make_model()
    sim = lefsim.Sim(model, seed=1)
    sim.run_until(100.0)
    assert model == make_model()  # unchanged
    now = sim.model()
    assert now != model and now.world == sim.world()
    # Restarting from it continues from here.
    restarted = lefsim.Sim(now, seed=2, start_time=sim.time)
    assert restarted.run_until(200.0).events_fired > 0


def test_copy_forks_the_run():
    sim = make_sim(seed=3)
    sim.run_until(50.0)
    fork = copy.copy(sim)
    a = sim.record_until(200.0, dt=1.0, events=True)
    b = fork.record_until(200.0, dt=1.0, events=True)
    assert table(a.snapshots).equals(table(b.snapshots))
    assert table(a.events).equals(table(b.events))
    assert copy.deepcopy(sim).model() == sim.model()


def test_run_reports():
    sim = make_sim()
    report = sim.run_until(10.0)
    assert (report.time, report.stopped) == (10.0, "reached_time")
    assert report.events_fired == sim.event_count
    report = sim.run_events(1000)
    assert (report.events_fired, report.stopped) == (1000, "reached_event_count")
    assert "RunReport" in repr(report)


def test_step_reports_actions_by_dense_id():
    sim = make_sim(n_lefs=2)
    seen = set()
    for _ in range(2000):
        t, action, ident = sim.step()
        assert t == sim.time
        seen.add(action)
        assert ident < (4 if action == "Step" else 2)
    assert seen == {"Step", "Respawn"}


def test_step_returns_none_when_exhausted():
    assert lefsim.Sim(lefsim.Model(lefsim.World(lefsim.Lattice(10)))).step() is None


def test_state_matches_recorded_state():
    sim = make_sim(seed=3)
    rec = sim.record_until(20.0, dt=20.0)
    last = table(rec.snapshots).slice(8)  # the t=20 snapshot
    now = table(sim.state())
    assert now["time"].to_pylist() == [20.0] * 8
    assert now.drop_columns(["time"]).equals(last.drop_columns(["time"]))


# ---------------------------------------------------------------- determinism


def fnv1a(data: bytes, h: int = 0xCBF29CE484222325) -> int:
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def test_python_reproduces_the_rust_golden_stream():
    """Same run as crates/lefsim/tests/determinism.rs, hashed the same way
    from the Arrow output. Proves Python sees exactly the Rust trajectory."""
    sites = list(range(25, 500, 50))
    lattice = lefsim.Lattice(
        500,
        ctcf_sites=sites,
        ctcf_captures_from=["right" if i % 100 == 25 else "left" for i in sites],
        ctcf_strength=0.7,
    )
    world = lefsim.populate(lattice, 20, seed=0x5EED)
    model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=0.01, respawn_captured=0.002))
    rec = lefsim.Sim(model, seed=0x5EED).record_events(50_000, dt=5.0, events=True)
    assert rec.events_fired == 50_000

    ev = pa.record_batch(rec.events)
    event_rows = np.zeros(
        ev.num_rows,
        dtype=[("t", "<u8"), ("leg", "<u4"), ("pos", "<u4"), ("mob", "u1"), ("kind", "u1")],
    )
    event_rows["t"] = ev["time"].to_numpy().view("<u8")
    event_rows["leg"] = ev["leg_id"].to_numpy()
    event_rows["pos"] = ev["pos"].to_numpy()
    event_rows["mob"] = ev["mobility"].indices.to_numpy()
    event_rows["kind"] = ev["action"].indices.to_numpy()

    snap = pa.record_batch(rec.snapshots)
    n_lefs = int(snap.schema.metadata[b"lefsim.n_lefs"])
    times = snap["time"].to_numpy()[::n_lefs]
    positions = np.column_stack([snap["left_pos"].to_numpy(), snap["right_pos"].to_numpy()])

    h = fnv1a(event_rows.tobytes())
    h = fnv1a(times.view("<u8").astype("<u8").tobytes(), h)
    h = fnv1a(positions.astype("<u4").tobytes(), h)
    h = fnv1a(snap["left_mobility"].indices.to_numpy().tobytes(), h)
    h = fnv1a(snap["right_mobility"].indices.to_numpy().tobytes(), h)
    assert h == RUST_GOLDEN, hex(h)


def test_same_seed_same_output():
    a = make_sim(seed=5).record_until(100.0, dt=0.5, events=True)
    b = make_sim(seed=5).record_until(100.0, dt=0.5, events=True)
    c = make_sim(seed=6).record_until(100.0, dt=0.5, events=True)
    assert table(a.snapshots).equals(table(b.snapshots))
    assert table(a.events).equals(table(b.events))
    assert not table(a.snapshots).equals(table(c.snapshots))


# ---------------------------------------------------------------- recording


def test_record_until_records_an_inclusive_grid():
    rec = make_sim().record_until(10.0, dt=0.5)
    t = table(rec.snapshots)
    assert rec.stopped == "reached_time" and rec.time == 10.0
    assert rec.events is None
    assert np.unique(t["time"].to_numpy()).tolist() == [0.5 * k for k in range(21)]
    assert t.num_rows == 21 * 8


def test_consecutive_calls_continue_the_grid():
    whole = make_sim(seed=2).record_until(50.0, dt=1.0, events=True)

    sim = make_sim(seed=2)
    parts = [sim.record_until(t, dt=1.0, events=True) for t in (10.0, 10.0, 33.3, 50.0)]
    snaps = pa.concat_tables([table(p.snapshots) for p in parts])
    events = pa.concat_tables([table(p.events) for p in parts])
    assert snaps.equals(table(whole.snapshots))
    assert events.equals(table(whole.events))


def test_record_events_counts():
    sim = make_sim()
    rec = sim.record_events(1000, events=True)
    assert rec.events_fired == 1000 and rec.stopped == "reached_event_count"
    assert rec.snapshots is None and rec.events.num_rows >= 1000
    assert sim.event_count == 1000


def test_snapshot_chunks_tile_the_grid():
    whole = table(make_sim(seed=8).record_until(100.0, dt=0.5).snapshots)
    sim = make_sim(seed=8)
    chunks = []
    while True:
        rec = sim.record_snapshot_chunk(100.0, dt=0.5, max_snapshots=17)
        chunks.append(table(rec.snapshots))
        assert rec.snapshots.num_rows <= 17 * 8
        if rec.stopped != "recorder_full":
            break
    assert len(chunks) > 5 and rec.stopped == "reached_time"
    assert pa.concat_tables(chunks).equals(whole)


def test_event_chunks_open_with_keyframes_and_tile_the_run():
    sim = make_sim(seed=9)
    n_legs = sim.n_legs
    chunks, prev_end = [], 0.0
    while True:
        rec = sim.record_event_chunk(300.0, max_rows=60)
        ev = table(rec.events)
        assert ev.slice(0, n_legs)["action"].to_pylist() == ["Keyframe"] * n_legs
        assert set(ev.slice(0, n_legs)["time"].to_pylist()) == {prev_end}
        assert ev.num_rows <= 62
        chunks.append((rec.events, rec.time))
        prev_end = rec.time
        if rec.stopped != "recorder_full":
            break
    assert len(chunks) > 10 and prev_end == 300.0

    # Replaying the chunks reproduces the run.
    replayer = lefsim.Replayer(sim.world().lattice, sim.n_lefs)
    for events, end in chunks:
        replayer.push(events, end)
    assert replayer.world_at(300.0) == sim.world()


def test_snapshots_never_show_evicted():
    rec = make_sim(seed=4, strength=1.0).record_until(500.0, dt=0.25, events=True)
    ev = table(rec.events)
    assert "Evicted" in ev["mobility"].to_pylist()
    snaps = table(rec.snapshots)
    assert "Evicted" not in snaps["left_mobility"].to_pylist()
    assert "Evicted" not in snaps["right_mobility"].to_pylist()


@pytest.mark.parametrize(
    "call, match",
    [
        (lambda s: s.record_until(10.0), "nothing to record"),
        (lambda s: s.record_until(10.0, dt=0.0), "dt must be positive"),
        (lambda s: s.record_until(float("nan"), dt=1.0), "t_end is NaN"),
        (lambda s: s.record_until(10.0, dt=1.0, layout="tall"), "layout"),
        (lambda s: s.record_until(10.0, keyframes=True), "requires events=True"),
        (lambda s: s.record_snapshot_chunk(10.0, dt=1.0, max_snapshots=0), "max_snapshots"),
        (lambda s: s.record_event_chunk(10.0, max_rows=17), "at least 18 with keyframes"),
        (lambda s: s.record_event_chunk(10.0, max_rows=1, keyframes=False), "at least 2 for events"),
    ],
)
def test_recording_argument_errors(call, match):
    sim = make_sim()  # 8 LEFs = 16 legs
    with pytest.raises(ValueError, match=match):
        call(sim)
    sim.run_until(1.0)  # still usable


# ---------------------------------------------------------------- layouts


def test_long_layout_is_wide_unpivoted():
    wide = table(make_sim(seed=7).record_until(30.0, dt=1.0).snapshots)
    long = table(make_sim(seed=7).record_until(30.0, dt=1.0, layout="long").snapshots)
    assert long.column_names == ["time", "leg_id", "lef_id", "facing", "pos", "mobility"]
    assert long.num_rows == 2 * wide.num_rows

    left = long.filter(pa.compute.equal(long["facing"].cast(pa.string()), "Left"))
    right = long.filter(pa.compute.equal(long["facing"].cast(pa.string()), "Right"))
    assert left["pos"].equals(wide["left_pos"])
    assert right["pos"].equals(wide["right_pos"])
    assert left["lef_id"].equals(wide["lef_id"])
    assert (left["leg_id"].to_numpy() == 2 * wide["lef_id"].to_numpy()).all()
    assert left["mobility"].cast(pa.string()).equals(wide["left_mobility"].cast(pa.string()))


# ---------------------------------------------------------------- consumers


def test_batch_is_readable_by_every_consumer():
    batch = make_sim().record_until(20.0, dt=1.0).snapshots
    expected = table(batch)

    assert pa.record_batch(batch).num_rows == expected.num_rows
    assert batch.to_pyarrow().equals(expected)

    df = pl.DataFrame(batch)
    assert df.shape == (expected.num_rows, 6)
    assert df["left_mobility"].dtype == pl.Categorical
    assert batch.to_polars().equals(df)

    pdf = batch.to_pandas()
    assert isinstance(pdf, pd.DataFrame) and len(pdf) == expected.num_rows
    assert set(pdf["left_mobility"].astype(str)) <= {"Moving", "Captured", "Evicted"}

    got = duckdb.sql("select count(*), max(time) from batch").fetchone()
    assert got == (expected.num_rows, 20.0)


def test_categoricals_carry_names():
    rec = make_sim(strength=1.0).record_until(300.0, events=True)
    ev = table(rec.events)
    assert ev.schema.field("action").type == pa.dictionary(pa.uint8(), pa.string())
    assert set(ev["action"].to_pylist()) == {"Step", "Respawn"}
    assert set(ev["mobility"].to_pylist()) <= {"Moving", "Captured", "Evicted"}


def test_batch_metadata_and_repr():
    batch = make_sim(n_lefs=3).record_until(1.0, dt=1.0).snapshots
    assert batch.metadata == {
        "lefsim.kind": "snapshots",
        "lefsim.layout": "wide",
        "lefsim.n_lefs": "3",
    }
    assert len(batch) == batch.num_rows == 6
    assert "snapshots" in repr(batch)


def test_wide_columns_reshape_to_snapshot_by_lef():
    batch = table(make_sim(n_lefs=8).record_until(10.0, dt=1.0).snapshots)
    left = batch["left_pos"].to_numpy().reshape(-1, 8)
    assert left.shape == (11, 8)
    assert (batch["lef_id"].to_numpy().reshape(-1, 8) == np.arange(8)).all()


# ---------------------------------------------------------------- streaming


def test_iterating_a_stream_matches_record_until():
    expected = make_sim(seed=9).record_until(200.0, dt=0.5).snapshots
    stream = make_sim(seed=9).stream(200.0, dt=0.5, chunk_size=37)
    batches = list(stream)
    assert stream.closed
    assert len(batches) > 5
    assert all(b.num_rows <= 37 * 8 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


def test_event_stream_matches_record_until():
    expected = make_sim(seed=10).record_until(150.0, events=True).events
    stream = make_sim(seed=10).stream(150.0, events=True, chunk_size=50)
    batches = list(stream)
    assert all(b.num_rows <= 50 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


@pytest.mark.parametrize("consumer", ["pyarrow", "polars", "duckdb"])
def test_stream_capsule_consumers(consumer):
    expected = table(make_sim(seed=11).record_until(100.0, dt=0.5).snapshots)
    stream = make_sim(seed=11).stream(100.0, dt=0.5, chunk_size=64)
    if consumer == "pyarrow":
        got = pa.RecordBatchReader.from_stream(stream).read_all()
    elif consumer == "polars":
        got = pl.DataFrame(stream).to_arrow()
        expected = pl.from_arrow(expected).to_arrow()
    else:
        got = duckdb.sql("select * from stream").arrow()
        if isinstance(got, pa.RecordBatchReader):
            got = got.read_all()
        got = got.cast(expected.schema) if got.schema != expected.schema else got
    assert got.num_rows == expected.num_rows
    assert got["time"].to_pylist() == expected["time"].to_pylist()
    assert got["left_pos"].to_pylist() == expected["left_pos"].to_pylist()
    assert stream.closed


def test_stream_consumes_the_sim_but_a_copy_survives():
    sim = make_sim()
    kept = copy.copy(sim)
    sim.stream(10.0, dt=1.0)
    with pytest.raises(RuntimeError, match="consumed"):
        sim.run_until(20.0)
    assert "consumed" in repr(sim)
    assert kept.run_until(20.0).stopped == "reached_time"


def test_stream_can_be_read_once():
    stream = make_sim().stream(10.0, dt=1.0)
    pa.RecordBatchReader.from_stream(stream).read_all()
    with pytest.raises(OSError, match="already read"):
        pa.RecordBatchReader.from_stream(stream)
    assert list(stream) == []


def test_stream_continues_the_grid_of_earlier_calls():
    whole = table(make_sim(seed=12).record_until(40.0, dt=1.0).snapshots)
    sim = make_sim(seed=12)
    head = table(sim.record_until(15.0, dt=1.0).snapshots)
    tail = pa.concat_tables(table(b) for b in sim.stream(40.0, dt=1.0, chunk_size=7))
    assert pa.concat_tables([head, tail]).equals(whole)


@pytest.mark.parametrize(
    "kwargs, match",
    [
        (dict(), "exactly one"),
        (dict(dt=1.0, events=True), "exactly one"),
        (dict(dt=0.0), "dt must be positive"),
        (dict(dt=1.0, chunk_size=0), "chunk_size"),
        (dict(events=True, chunk_size=1), "at least 2"),
        (dict(dt=1.0, keyframes=True), "requires events=True"),
        (dict(events=True, keyframes=True, chunk_size=17), r"at least 18 with keyframes"),
    ],
)
def test_stream_argument_errors(kwargs, match):
    sim = make_sim()
    with pytest.raises(ValueError, match=match):
        sim.stream(10.0, **kwargs)
    sim.run_until(1.0)  # a rejected stream() leaves the Sim usable


# ---------------------------------------------------------------- keyframes


def test_action_dictionary_includes_keyframe():
    ev = pa.record_batch(make_sim().record_until(10.0, events=True).events)
    assert ev["action"].dictionary.to_pylist() == ["Step", "Respawn", "Keyframe"]


def test_keyframed_events_open_with_every_leg():
    ev = table(make_sim(n_lefs=5).record_until(200.0, events=True, keyframes=True).events)
    head = ev.slice(0, 10)
    assert head["action"].to_pylist() == ["Keyframe"] * 10
    assert head["leg_id"].to_pylist() == list(range(10))
    assert set(head["time"].to_pylist()) == {0.0}
    assert "Keyframe" not in ev.slice(10)["action"].to_pylist()


def test_keyframed_stream_reduces_to_the_plain_log():
    plain_sim = make_sim(seed=13)
    plain = table(plain_sim.record_until(300.0, events=True).events)
    n_legs = plain_sim.n_legs

    stream = make_sim(seed=13).stream(300.0, events=True, keyframes=True, chunk_size=60)
    batches = [table(b) for b in stream]
    assert len(batches) > 10
    for b in batches:
        assert b.slice(0, n_legs)["action"].to_pylist() == ["Keyframe"] * n_legs
        assert b.num_rows <= 60 + 2  # soft only by a zero-delay chain

    events = pa.concat_tables(batches)
    events = events.filter(pa.compute.not_equal(events["action"].cast(pa.string()), "Keyframe"))
    assert events.to_pydict() == plain.to_pydict()


# ---------------------------------------------------------------- replayer


def keyframed_stream(seed, t_end, chunk_size=100):
    return make_sim(seed=seed).stream(t_end, events=True, keyframes=True, chunk_size=chunk_size)


def live_worlds(seed, times):
    sim = make_sim(seed=seed)
    out = []
    for t in times:
        sim.run_until(t)
        out.append(sim.world())
    return out


def test_replayer_matches_the_live_run_in_any_order():
    times = [0.0, 3.3, 77.7, 150.0, 222.2, 299.0]
    expected = live_worlds(14, times)
    r = lefsim.Replayer.from_arrow(keyframed_stream(14, 300.0), make_lattice(), end=300.0)
    assert (r.n_lefs, r.start, r.end) == (8, 0.0, 300.0)
    for i in [3, 0, 5, 1, 4, 2]:
        assert r.world_at(times[i]) == expected[i], times[i]
    r.seek(10.0)
    assert r.advance_to(20.0) == 20.0
    assert r.world() == live_worlds(14, [20.0])[0]


def test_replayer_snapshots_equal_a_recording():
    expected = table(make_sim(seed=15).record_until(300.0, dt=0.7).snapshots)
    r = lefsim.Replayer.from_arrow(keyframed_stream(15, 300.0), make_lattice(), end=300.0)
    assert table(r.snapshots(0.7)).equals(expected)
    r.seek(200.0)
    assert table(r.snapshots(0.7, t_start=0.0)).equals(expected)
    # Without `end`, the log ends at its last event.
    r = lefsim.Replayer.from_arrow(keyframed_stream(15, 300.0), make_lattice())
    assert r.end < 300.0


def test_replayer_reads_any_arrow_source(tmp_path):
    import pyarrow.parquet as pq

    batches = [table(b) for b in keyframed_stream(16, 200.0)]
    events = pa.concat_tables(batches)
    path = tmp_path / "run.parquet"
    pq.write_table(events, path, row_group_size=37)  # boundaries that split keyframes

    expected = live_worlds(16, [150.0])[0]
    sources = {
        "table": events,
        "list of tables": batches,
        "parquet batches": pq.ParquetFile(path).iter_batches(batch_size=53),
        "polars": pl.DataFrame(events),
        "strings, not dictionaries": events.cast(
            pa.schema([f.with_type(pa.string()) if pa.types.is_dictionary(f.type) else f for f in events.schema])
        ),
    }
    for name, source in sources.items():
        r = lefsim.Replayer.from_arrow(source, make_lattice())
        assert r.world_at(150.0) == expected, name


def test_replayer_state_and_late_join():
    batches = list(keyframed_stream(17, 300.0, chunk_size=80))
    r = lefsim.Replayer.from_arrow(batches[3:], make_lattice())
    first = table(batches[3])["time"][0].as_py()
    assert r.start == first and not r.ready
    with pytest.raises(ValueError, match="no keyframe applied"):
        r.state()
    with pytest.raises(ValueError, match="no keyframe at or before"):
        r.seek(first - 1.0)
    r.seek(250.0)
    sim = make_sim(seed=17)
    sim.run_until(250.0)
    assert table(r.state()).equals(table(sim.state()))
    assert table(r.state(layout="long")).equals(table(sim.state(layout="long")))


def test_replayer_trim_before():
    r = lefsim.Replayer.from_arrow(keyframed_stream(18, 300.0, chunk_size=60), make_lattice())
    r.advance_to(250.0)
    r.trim_before(200.0)
    assert 0.0 < r.start <= 200.0
    assert r.world_at(260.0) == live_worlds(18, [260.0])[0]


def test_replayer_errors():
    lattice = make_lattice()
    with pytest.raises(ValueError, match="no keyframe in the log"):
        lefsim.Replayer.from_arrow(make_sim().record_until(10.0, events=True).events, lattice)
    events = table(make_sim().record_until(10.0, events=True, keyframes=True).events)
    with pytest.raises(ValueError, match="missing column"):
        lefsim.Replayer.from_arrow(events.drop_columns(["pos"]), lattice)
    with pytest.raises(ValueError, match="out of range"):
        lefsim.Replayer.from_arrow(events, lattice, n_lefs=2)
    # A log from another lattice gives no valid world.
    r = lefsim.Replayer.from_arrow(events, lefsim.Lattice(20))
    with pytest.raises(ValueError, match="replayed state is invalid"):
        r.world_at(10.0)


def test_replayer_reproduces_the_cross_language_fixture():
    """python_events.arrows is also replayed by the TS Replayer against the
    wasm Sim; both matching a live Sim means the two replayers agree."""
    import pyarrow.ipc as ipc

    config = json.loads((FIXTURES / "python_events.json").read_text())["config"]
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
    with ipc.open_stream(FIXTURES / "python_events.arrows") as reader:
        r = lefsim.Replayer.from_arrow(reader, lattice)
    sim = lefsim.Sim(model, seed=config["seed"])
    for t in np.linspace(0.0, r.end, 23):
        sim.run_until(t)
        assert r.world_at(t) == sim.world(), t


# ---------------------------------------------------------------- GIL / signals


def test_long_runs_release_the_gil_and_honor_ctrl_c():
    # A timer thread can only fire while the run holds no GIL, and its
    # interrupt is only seen if the run checks for signals between chunks.
    sim = make_sim(n_lefs=50)
    timer = threading.Timer(0.3, _thread.interrupt_main)
    timer.start()
    try:
        with pytest.raises(KeyboardInterrupt):
            sim.record_until(1e12, dt=1.0)
    finally:
        timer.cancel()
    # The sim is left consistent and usable.
    t = sim.time
    assert t > 0
    rec = sim.record_until(t + 10.0, dt=1.0)
    assert rec.stopped == "reached_time"


def test_import_pulls_in_no_dataframe_libraries():
    code = (
        "import sys, lefsim;"
        "m = lefsim.Model(lefsim.populate(lefsim.Lattice(100), 3));"
        "r = lefsim.Sim(m).record_until(50.0, dt=1.0, events=True);"
        "assert r.snapshots.num_rows == 51 * 3;"
        "bad = {'pyarrow', 'polars', 'duckdb', 'pandas', 'numpy'} & set(sys.modules);"
        "assert not bad, bad"
    )
    subprocess.run([sys.executable, "-c", code], check=True)
