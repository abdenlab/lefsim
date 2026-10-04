"""Tests for the lefsim Python bindings."""

import copy
import math
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
RUST_GOLDEN = 0x4A7DF0425E0D4F0B

FIXTURES = pathlib.Path(__file__).parents[3] / "packages" / "lefsim" / "test" / "fixtures"

RATES = lefsim.Rates(step=1.0, respawn=0.02, respawn_stalled=0.02, respawn_evicted=0.5, respawn_captured=0.005)


def make_lattice(strength=0.8):
    """200 sites with a convergent CTCF pair plus an extra right-capturing CTCF."""
    return lefsim.Lattice(
        200, {"site": [60, 100, 140], "captures_from": ["right", "right", "left"], "strength": strength}
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
    "n_sites, ctcfs, match",
    [
        (0, None, "at least one site"),
        (10, {"site": [3]}, "missing column \"captures_from\""),
        (10, {"captures_from": "left"}, "missing column \"site\""),
        (10, {"site": [10], "captures_from": "left"}, "outside"),
        (10, {"site": [3, 3], "captures_from": "left"}, "twice"),
        (10, {"site": [3], "captures_from": "up"}, "'left' or 'right'"),
        (10, {"site": [3, 4], "captures_from": ["left"]}, "1 entries"),
        (10, {"site": [3], "captures_from": "left", "strength": 1.5}, r"\[0, 1\]"),
    ],
)
def test_lattice_errors(n_sites, ctcfs, match):
    with pytest.raises(ValueError, match=match):
        lefsim.Lattice(n_sites, ctcfs)


def test_lattice_ctcfs_broadcast_and_default_strength():
    lattice = lefsim.Lattice(50, {"site": [10, 20], "captures_from": "Right"})
    ctcfs = table(lattice.to_arrow())
    assert ctcfs["captures_from"].to_pylist() == ["Right", "Right"]
    assert ctcfs["strength"].to_pylist() == [1.0, 1.0]
    assert (lattice.n_sites, lattice.n_ctcfs, lattice.n_plain) == (50, 2, 48)


def test_lattice_round_trips_through_arrow_tables():
    lattice = make_lattice()
    batch = lattice.to_arrow()
    assert lefsim.Lattice(200, batch) == lattice
    assert lefsim.Lattice(200, table(batch)) == lattice
    assert lefsim.Lattice(200, pl.DataFrame(batch)) == lattice
    assert lefsim.Lattice(200, table(batch).to_pandas()) == lattice
    codes = pa.table({"site": pa.array([60], pa.uint32()), "captures_from": pa.array([1], pa.uint8())})
    assert table(lefsim.Lattice(200, codes).to_arrow())["captures_from"].to_pylist() == ["Right"]


def test_numpy_inputs_are_accepted():
    lattice = lefsim.Lattice(
        50,
        {"site": np.array([10, 20]), "captures_from": ["left", "right"], "strength": np.array([0.2, 0.9])},
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
        lefsim.populate(lefsim.Lattice(10), 1, spawn_rule="anywhere")


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
        ({"left_pos": [5], "right_pos": [7], "right_mobility": ["evicted"]}, "neither its site nor"),
        # A right-facing leg at the CTCF at 140 (capturing from the left) can be captured, not evicted.
        ({"left_pos": [130], "right_pos": [139], "right_mobility": ["evicted"]}, "neither its site nor"),
        ({"left_pos": [130], "right_pos": [140], "right_mobility": ["evicted"]}, "neither its site nor"),
        ({"left_pos": [5], "right_pos": [7], "right_mobility": ["stalled"]}, "mobility must be"),
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
    assert (r.step, r.respawn, r.respawn_stalled, r.respawn_evicted, r.respawn_captured) == (1.0, 1e-3, 1e-2, 1e-1, 1e-3)
    assert lefsim.Rates(respawn=0.5) == lefsim.Rates(1.0, 0.5, 1e-2, 1e-1, 1e-3)


def test_model_parts():
    world = lefsim.populate(make_lattice(), 3, seed=1)
    model = lefsim.Model(world, RATES)
    assert model.world == world
    assert model.lattice == world.lattice
    assert model.rates == RATES
    assert model.spawn_rule == "uniform_non_ctcf"
    assert lefsim.Model(world).rates == lefsim.Rates()


@pytest.mark.parametrize(
    "rates, match",
    [
        (lefsim.Rates(step=0.0), "step rate must be positive"),
        (lefsim.Rates(respawn=float("nan")), "respawn rate must be positive"),
        (lefsim.Rates(respawn_stalled=0.0), "respawn_stalled rate"),
        (lefsim.Rates(respawn_evicted=float("inf")), "respawn_evicted rate"),
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
        lefsim.Model(lefsim.World(lefsim.Lattice(4)), spawn_rule="anywhere")


def test_model_json_round_trip_and_shape():
    model = make_model(n_lefs=2)
    doc = json.loads(model.to_json(indent=2))
    assert doc["format"] == "lefsim.model" and doc["version"] == 2
    assert doc["lattice"]["n_sites"] == 200
    assert doc["lattice"]["ctcfs"][0] == {"site": 60, "captures_from": "right", "strength": 0.8}
    assert doc["rates"] == {"step": 1.0, "respawn": 0.02, "respawn_stalled": 0.02, "respawn_evicted": 0.5, "respawn_captured": 0.005}
    assert doc["spawn"] == {"rule": "uniform_non_ctcf"}
    rows = table(model.world.to_arrow()).drop_columns(["lef_id"]).to_pylist()
    assert doc["lefs"] == [{k: v.lower() if isinstance(v, str) else v for k, v in r.items()} for r in rows]
    assert lefsim.Model.from_json(model.to_json()) == model

    # Mobility may be left out when loading.
    doc["lefs"] = [{"left_pos": 10, "right_pos": 12}]
    assert lefs(lefsim.Model.from_json(json.dumps(doc)).world)["left_mobility"] == ["Moving"]

    # Loading validates: a left-facing leg captured by the CTCF at 140,
    # which captures from the left, is rejected.
    doc["lefs"] = [{"left_pos": 140, "right_pos": 150, "left_mobility": "captured"}]
    with pytest.raises(ValueError, match="can't be captured"):
        lefsim.Model.from_json(json.dumps(doc))


@pytest.mark.parametrize(
    "obj",
    [make_lattice(), lefsim.populate(make_lattice(), 4, seed=2), RATES, make_model()],
    ids=["Lattice", "World", "Rates", "Model"],
)
def test_model_types_pickle_json_and_dicts(obj):
    assert pickle.loads(pickle.dumps(obj)) == obj
    assert type(obj).from_json(obj.to_json()) == obj
    d = obj.to_dict()
    assert d == json.loads(obj.to_json())
    assert type(obj).from_dict(d) == obj


def test_from_dict_validates():
    d = make_model().to_dict()
    d["rates"]["step"] = -1.0
    with pytest.raises(ValueError, match="step rate"):
        lefsim.Model.from_dict(d)


# ---------------------------------------------------------------- effective distance


def rosette_world():
    """LEF 1 at 10-20 and LEF 2 at 21-30: linked, a rosette of two."""
    return lefsim.World(lefsim.Lattice(40), {"left_pos": [10, 21], "right_pos": [20, 30]})


def test_effective_distance_follows_the_rosette_rules():
    w = rosette_world()
    long_hop = dict(step_length=1, hop_length=5)
    assert w.effective_distance(10, 30, **long_hop) == 10  # across the chain: a hop per LEF, the junction free
    assert w.effective_distance(20, 21, **long_hop) == 1  # the junction step
    assert w.effective_distance(20, 30, **long_hop) == 6  # the step, then a hop
    short_hop = dict(step_length=1000, hop_length=100)
    # With hops shorter than steps, routes with extra hops win.
    assert [w.effective_distance(a, b, **short_hop) for a, b in [(20, 21), (20, 30), (10, 30), (10, 21)]] == [
        400,
        300,
        200,
        300,
    ]
    assert lefsim.World(lefsim.Lattice(40)).effective_distance(3, 9, step_length=2, hop_length=0) == 12


def test_effective_distances_pairs_and_rectangles():
    import array

    w = make_sim(seed=31).world()
    sites = [0, 37, 60, 99, 140, 150, 199]
    lengths = dict(step_length=1000, hop_length=100)
    pairs = w.effective_distances(sites, **lengths)
    assert isinstance(pairs, array.array) and pairs.typecode == "q"
    expected = [w.effective_distance(a, b, **lengths) for i, a in enumerate(sites) for b in sites[i + 1 :]]
    assert list(pairs) == expected
    targets = [5, 120]
    rect = np.asarray(w.effective_distances(sites, targets, **lengths)).reshape(len(sites), len(targets))
    assert rect.dtype == np.int64
    assert rect.tolist() == [[w.effective_distance(a, b, **lengths) for b in targets] for a in sites]


def test_effective_distance_errors():
    w = rosette_world()
    with pytest.raises(ValueError, match="step length must be positive"):
        w.effective_distance(1, 2, step_length=0, hop_length=1)
    with pytest.raises(ValueError, match="outside a lattice"):
        w.effective_distances([1, 40], step_length=1, hop_length=1)
    with pytest.raises(TypeError):
        w.effective_distance(1, 2, step_length=1)  # both lengths are required


@pytest.mark.parametrize("source", ["memory", "parquet"])
def test_replayer_effective_distances_match_each_world(tmp_path, source):
    sites = [5, 61, 99, 140, 199]
    lengths = dict(step_length=1000, hop_length=100)
    if source == "parquet":
        r = lefsim.Replayer.from_parquet(write_run(tmp_path, 32, t_end=300.0))
    else:
        r = lefsim.Replayer.from_arrow(keyframed_stream(32, 300.0), make_lattice(), end=300.0)
    times, d = r.effective_distances(sites, dt=7.5, **lengths)
    d = np.asarray(d).reshape(len(times), -1)
    assert d.shape == (41, 10)
    worlds = live_worlds(32, list(times))
    for t, row, world in zip(times, d, worlds):
        assert list(row) == list(world.effective_distances(sites, **lengths)), t
    with pytest.raises(ValueError, match="outside a lattice"):
        r.effective_distances([500], dt=1.0, **lengths)


# ---------------------------------------------------------------- sim


def test_construction_and_properties():
    sim = make_sim(n_lefs=3)
    assert (sim.n_sites, sim.n_lefs, sim.n_legs) == (200, 3, 6)
    assert sim.time == 0.0 and sim.n_events == 0
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
    assert restarted.run_until(200.0).n_events > 0


def test_a_sim_remembers_what_it_started_from():
    model = make_model()
    sim = lefsim.Sim(model, seed=7, start_time=12.5)
    sim.run_until(300.0)
    assert sim.initial_model() == model and sim.model() != model
    assert (sim.seed, sim.start_time) == (7, 12.5)
    again = lefsim.Sim(sim.initial_model(), seed=sim.seed, start_time=sim.start_time)
    again.run_until(300.0)
    assert again.world() == sim.world()


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
    assert report.n_events == sim.n_events
    report = sim.run_events(1000)
    assert (report.n_events, report.stopped) == (1000, "reached_event_count")
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
    sides = ["right" if i % 100 == 25 else "left" for i in sites]
    lattice = lefsim.Lattice(500, {"site": sites, "captures_from": sides, "strength": 0.7})
    world = lefsim.populate(lattice, 20, seed=0x5EED)
    model = lefsim.Model(world, lefsim.Rates(step=1.0, respawn=0.01, respawn_stalled=0.05, respawn_evicted=0.2, respawn_captured=0.002))
    rec = lefsim.Sim(model, seed=0x5EED).record_events(50_000, dt=5.0, events=True)
    assert rec.n_events == 50_000

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
    assert rec.n_events == 1000 and rec.stopped == "reached_event_count"
    assert rec.snapshots is None and rec.events.num_rows >= 1000
    assert sim.n_events == 1000


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


def test_evicted_legs_wait_on_or_before_their_ctcf():
    rec = make_sim(seed=4, strength=1.0).record_until(500.0, dt=0.25, events=True)
    assert "Evicted" in table(rec.events)["mobility"].to_pylist()
    snaps = table(rec.snapshots)
    # make_lattice: CTCFs at 60 and 100 capture from the right (evicting
    # right-facing legs on them, or at 59 and 99 if held), and at 140 from the
    # left (evicting left-facing legs on it, or at 141).
    for side, where in (("left", {140, 141}), ("right", {59, 60, 99, 100})):
        mob = snaps[f"{side}_mobility"].to_pylist()
        evicted = {p for p, m in zip(snaps[f"{side}_pos"].to_pylist(), mob) if m == "Evicted"}
        assert evicted <= where, (side, evicted)
    assert "Evicted" in snaps["left_mobility"].to_pylist() + snaps["right_mobility"].to_pylist()


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


def test_leg_layout_is_the_lef_layout_unpivoted():
    wide = table(make_sim(seed=7).record_until(30.0, dt=1.0).snapshots)
    long = table(make_sim(seed=7).record_until(30.0, dt=1.0, layout="leg").snapshots)
    assert long.column_names == ["time", "leg_id", "lef_id", "pos", "mobility"]
    assert long.num_rows == 2 * wide.num_rows

    is_left = pa.compute.equal(pa.compute.bit_wise_and(long["leg_id"], 1), 0)
    left = long.filter(is_left)
    right = long.filter(pa.compute.invert(is_left))
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
    assert batch.metadata == {"lefsim.kind": "lefs", "lefsim.n_lefs": "3"}
    assert len(batch) == batch.num_rows == 6
    assert "lefs" in repr(batch)
    leg = make_sim(n_lefs=3).record_until(1.0, dt=1.0, layout="leg").snapshots
    assert leg.metadata == {"lefsim.kind": "legs", "lefsim.n_lefs": "3"}


def test_one_schema_for_every_lef_table():
    """World tables, states and snapshots are the same table (snapshots and
    states add `time`), and leg rows share the event log's columns."""
    sim = make_sim(n_lefs=3)
    sim.run_until(5.0)
    world = table(sim.world().to_arrow())
    state = table(sim.state())
    snaps = table(sim.record_until(7.0, dt=1.0).snapshots)
    assert state.schema == snaps.schema
    assert state.drop_columns(["time"]).schema.remove_metadata() == world.schema.remove_metadata()
    assert state.drop_columns(["time"]).equals(world)
    legs = table(sim.record_until(9.0, dt=1.0, layout="leg").snapshots)
    events = table(sim.record_until(11.0, events=True).events)
    assert events.drop_columns(["action"]).schema.remove_metadata() == legs.schema.remove_metadata()


def test_wide_columns_reshape_to_snapshot_by_lef():
    batch = table(make_sim(n_lefs=8).record_until(10.0, dt=1.0).snapshots)
    left = batch["left_pos"].to_numpy().reshape(-1, 8)
    assert left.shape == (11, 8)
    assert (batch["lef_id"].to_numpy().reshape(-1, 8) == np.arange(8)).all()


# ---------------------------------------------------------------- streaming


def test_iterating_a_stream_matches_record_until():
    expected = make_sim(seed=9).record_until(200.0, dt=0.5).snapshots
    stream = make_sim(seed=9).stream(200.0, dt=0.5, max_snapshots=37)
    batches = list(stream)
    assert stream.closed
    assert len(batches) > 5
    assert all(b.num_rows <= 37 * 8 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


def test_event_stream_matches_record_until():
    expected = make_sim(seed=10).record_until(150.0, events=True).events
    stream = make_sim(seed=10).stream(150.0, events=True, max_rows=50)
    batches = list(stream)
    assert all(b.num_rows <= 50 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


@pytest.mark.parametrize("consumer", ["pyarrow", "polars", "duckdb"])
def test_stream_capsule_consumers(consumer):
    expected = table(make_sim(seed=11).record_until(100.0, dt=0.5).snapshots)
    stream = make_sim(seed=11).stream(100.0, dt=0.5, max_snapshots=64)
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


def test_a_stream_says_where_it_came_from():
    sim = make_sim(seed=21)
    sim.run_until(40.0)  # record from t = 40
    stream = sim.stream(100.0, events=True, keyframes=True, max_rows=60)
    reader = pa.RecordBatchReader.from_stream(stream)
    meta = reader.schema.metadata
    model = lefsim.Model.from_json(meta[b"lefsim.model"].decode())
    run = json.loads(meta[b"lefsim.run"])
    assert model == make_model(seed=21)  # the model the run started from
    assert run == {
        "format": "lefsim.run",
        "version": 1,
        "seed": "21",
        "start_time": 0.0,
        "record_start": 40.0,
        "record_end": 100.0,
        "lefsim_version": lefsim.__version__,
    }
    assert meta[b"lefsim.kind"] == b"events"
    batches = list(reader)
    # Every batch carries it (however many a quiet stretch leaves).
    assert len(batches) > 1 and all(b.schema.metadata == meta for b in batches)

    # Re-running from it by hand gives the same rows.
    again = lefsim.Sim(model, seed=int(run["seed"]), start_time=run["start_time"])
    again.run_until(run["record_start"])
    expected = pa.RecordBatchReader.from_stream(again.stream(100.0, events=True, keyframes=True, max_rows=60))
    assert pa.Table.from_batches(batches).equals(expected.read_all())


def test_an_unbounded_stream_has_no_record_end():
    stream = make_sim().stream(float("inf"), dt=1.0)
    run = json.loads(pa.RecordBatchReader.from_stream(stream).schema.metadata[b"lefsim.run"])
    assert run["record_end"] is None


def test_recordings_carry_no_provenance():
    rec = make_sim().record_until(10.0, dt=1.0, events=True)
    assert "lefsim.model" not in rec.snapshots.metadata and "lefsim.run" not in rec.events.metadata


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
    tail = pa.concat_tables(table(b) for b in sim.stream(40.0, dt=1.0, max_snapshots=7))
    assert pa.concat_tables([head, tail]).equals(whole)


@pytest.mark.parametrize(
    "kwargs, match",
    [
        (dict(), "exactly one"),
        (dict(dt=1.0, events=True), "exactly one"),
        (dict(dt=0.0), "dt must be positive"),
        (dict(dt=1.0, max_snapshots=0), "max_snapshots must be positive"),
        (dict(dt=1.0, max_rows=10), "max_rows applies to events"),
        (dict(events=True, max_snapshots=10), "max_snapshots applies to dt"),
        (dict(events=True, max_rows=1), "at least 2"),
        (dict(dt=1.0, keyframes=True), "requires events=True"),
        (dict(events=True, keyframes=True, max_rows=17), r"at least 18 with keyframes"),
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

    stream = make_sim(seed=13).stream(300.0, events=True, keyframes=True, max_rows=60)
    batches = [table(b) for b in stream]
    assert len(batches) > 10
    for b in batches:
        assert b.slice(0, n_legs)["action"].to_pylist() == ["Keyframe"] * n_legs
        assert b.num_rows <= 60 + 2  # soft only by a zero-delay chain

    events = pa.concat_tables(batches)
    events = events.filter(pa.compute.not_equal(events["action"].cast(pa.string()), "Keyframe"))
    assert events.to_pydict() == plain.to_pydict()


# ---------------------------------------------------------------- parquet


def test_to_parquet_writes_one_keyframed_row_group_per_chunk(tmp_path):
    import pyarrow.parquet as pq

    path = tmp_path / "run.parquet"
    sim = make_sim(seed=22)
    sim.run_until(30.0)
    stream = sim.stream(300.0, events=True, keyframes=True, max_rows=80)
    assert stream.time == 30.0
    stream.to_parquet(path)
    assert stream.closed and stream.time == 300.0

    f = pq.ParquetFile(path)
    meta = f.metadata.metadata  # the footer's key-value metadata
    time_column = f.metadata.row_group(0).column(0)
    assert time_column.compression == "ZSTD" and "BYTE_STREAM_SPLIT" in time_column.encodings
    run = json.loads(meta[b"lefsim.run"])
    assert (run["record_start"], run["record_end"]) == (30.0, 300.0)
    assert meta[b"lefsim.row_groups"] == b"keyframed"
    assert lefsim.Model.from_json(meta[b"lefsim.model"].decode()) == make_model(seed=22)
    # The schema's view (the stream's metadata when writing began) has the
    # same model and marker; its run info has the requested end.
    view = f.schema_arrow.metadata
    assert view[b"lefsim.model"] == meta[b"lefsim.model"]
    assert view[b"lefsim.row_groups"] == b"keyframed"
    assert json.loads(view[b"lefsim.run"])["record_end"] == 300.0

    # Each row group is one chunk and opens with a full keyframe.
    fresh = make_sim(seed=22)
    fresh.run_until(30.0)
    expected = [table(b) for b in fresh.stream(300.0, events=True, keyframes=True, max_rows=80)]
    assert f.num_row_groups == len(expected) > 3
    for g in range(f.num_row_groups):
        group = f.read_row_group(g)
        head = group.slice(0, 16)
        assert head["action"].to_pylist() == ["Keyframe"] * 16
        assert head["leg_id"].to_pylist() == list(range(16))
        assert group.to_pydict() == expected[g].to_pydict()


def test_an_interrupted_write_leaves_a_valid_file(tmp_path):
    import pyarrow.parquet as pq

    path = tmp_path / "run.parquet"
    stream = make_sim(n_lefs=50).stream(float("inf"), events=True, keyframes=True, max_rows=2_000)
    timer = threading.Timer(0.3, _thread.interrupt_main)
    timer.start()
    try:
        with pytest.raises(KeyboardInterrupt):
            stream.to_parquet(path)
    finally:
        timer.cancel()

    f = pq.ParquetFile(path)
    run = json.loads(f.metadata.metadata[b"lefsim.run"])
    last_row = f.read_row_group(f.num_row_groups - 1)["time"].to_pylist()[-1]
    assert f.num_row_groups > 0 and last_row <= run["record_end"] < float("inf")
    # The schema's view still has what the stream asked for: no end.
    assert json.loads(f.schema_arrow.metadata[b"lefsim.run"])["record_end"] is None


def write_run(tmp_path, seed, t_end=400.0, record_from=0.0, max_rows=80):
    sim = make_sim(seed=seed)
    sim.run_until(record_from)
    path = tmp_path / f"run{seed}.parquet"
    sim.stream(t_end, events=True, keyframes=True, max_rows=max_rows).to_parquet(path)
    return path


def test_from_parquet_reads_only_the_row_groups_it_needs(tmp_path):
    import pyarrow.parquet as pq

    path = write_run(tmp_path, 23)
    n_groups = pq.ParquetFile(path).num_row_groups
    assert n_groups > 10
    r = lefsim.Replayer.from_parquet(path)
    assert (r.n_lefs, r.start, r.end, r.row_groups_read) == (8, 0.0, 400.0, 0)
    assert r.lattice == make_lattice()

    # Random access: each far seek reads one row group, and matches the live run.
    times = [350.0, 12.3, 199.9, 399.0, 77.7]
    expected = live_worlds(23, sorted(times))
    for t in times:
        before = r.row_groups_read
        assert r.world_at(t) == expected[sorted(times).index(t)], t
        assert r.row_groups_read - before <= 1

    # Playing forward in small steps reads each row group once more at most.
    r.seek(0.0)
    before = r.row_groups_read
    steps = [0.5 * k for k in range(801)]
    worlds = live_worlds(23, steps)
    for t, w in zip(steps, worlds):
        r.advance_to(t)
        if int(t) % 50 == 0:
            assert r.world() == w, t
    assert r.row_groups_read - before <= n_groups


def test_from_parquet_snapshots_equal_a_recording(tmp_path):
    expected = table(make_sim(seed=24).record_until(400.0, dt=0.9).snapshots)
    r = lefsim.Replayer.from_parquet(write_run(tmp_path, 24))
    assert table(r.snapshots(0.9)).equals(expected)
    # From partway, via t_start.
    part = table(r.snapshots(0.9, t_start=200.0))
    assert part.equals(expected.filter(pa.compute.greater_equal(expected["time"], 200.0)))


def test_from_parquet_of_a_recording_that_starts_mid_run(tmp_path):
    path = write_run(tmp_path, 25, record_from=100.0)
    r = lefsim.Replayer.from_parquet(path)
    assert (r.start, r.end) == (100.0, 400.0)
    assert "end=400.0" in repr(r)
    assert r.advance_to(50.0) == -math.inf and not r.ready  # not playing yet: it waits
    assert r.world_at(250.0) == live_worlds(25, [250.0])[0]
    with pytest.raises(ValueError, match="no keyframe at or before"):
        r.seek(50.0)
    assert r.advance_to(50.0) == 100.0  # playing: stops at the start of the data
    assert r.world() == live_worlds(25, [100.0])[0]


def test_from_parquet_falls_back_for_other_files(tmp_path):
    import pyarrow.parquet as pq

    events = pa.RecordBatchReader.from_stream(make_sim(seed=26).stream(200.0, events=True, keyframes=True, max_rows=80)).read_all()
    plain = tmp_path / "plain.parquet"
    pq.write_table(events.replace_schema_metadata(None), plain, row_group_size=37)
    with pytest.raises(ValueError, match="pass lattice="):
        lefsim.Replayer.from_parquet(plain)
    r = lefsim.Replayer.from_parquet(plain, make_lattice())
    assert r.row_groups_read == 0  # read whole, not lazily
    assert r.world_at(150.0) == live_worlds(26, [150.0])[0]
    with pytest.raises(ValueError, match="reads its own data"):
        lefsim.Replayer.from_parquet(write_run(tmp_path, 27)).push(events)


def test_to_parquet_compression_is_an_option(tmp_path):
    import pyarrow.parquet as pq

    path = tmp_path / "snappy.parquet"
    make_sim().stream(50.0, events=True, keyframes=True).to_parquet(path, compression="snappy")
    assert pq.ParquetFile(path).metadata.row_group(0).column(0).compression == "SNAPPY"
    assert lefsim.Replayer.from_parquet(path).world_at(40.0) == live_worlds(1, [40.0])[0]


def test_to_parquet_takes_only_keyframed_event_streams(tmp_path):
    for kw in (dict(dt=1.0), dict(events=True)):
        with pytest.raises(ValueError, match="keyframed event streams"):
            make_sim().stream(10.0, **kw).to_parquet(tmp_path / "x.parquet")


# ---------------------------------------------------------------- replayer


def keyframed_stream(seed, t_end, max_rows=100):
    return make_sim(seed=seed).stream(t_end, events=True, keyframes=True, max_rows=max_rows)


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
    batches = list(keyframed_stream(17, 300.0, max_rows=80))
    # The stream ran to 300; say so, rather than ending at the last event.
    r = lefsim.Replayer.from_arrow(batches[3:], make_lattice(), end=300.0)
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
    assert table(r.state(layout="leg")).equals(table(sim.state(layout="leg")))

    # Once playing, advance_to never fails: going back before the data
    # stops at its start.
    assert r.advance_to(first - 1.0) == first
    assert r.world() == live_worlds(17, [first])[0]


def test_replayer_trim_before():
    r = lefsim.Replayer.from_arrow(keyframed_stream(18, 300.0, max_rows=60), make_lattice())
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
    ctcfs = {"site": config["ctcfSites"], "captures_from": config["ctcfCapturesFrom"], "strength": config["ctcfStrength"]}
    lattice = lefsim.Lattice(config["nSites"], ctcfs)
    rates = config["rates"]
    model = lefsim.Model(
        lefsim.populate(lattice, config["nLefs"], seed=config["seed"]),
        lefsim.Rates(
            step=rates["step"],
            respawn=rates["respawn"],
            respawn_stalled=rates["respawnStalled"],
            respawn_evicted=rates["respawnEvicted"],
            respawn_captured=rates["respawnCaptured"],
        ),
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
