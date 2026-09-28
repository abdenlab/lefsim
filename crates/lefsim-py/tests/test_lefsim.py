"""Tests for the lefsim Python bindings."""

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


def make_sim(seed=1, n_lefs=8, strength=0.8, **kw):
    """200 sites with a convergent CTCF pair plus an extra right-capturing CTCF."""
    kw.setdefault("step_rate", 1.0)
    kw.setdefault("respawn_rate", 0.02)
    kw.setdefault("respawn_rate_captured", 0.005)
    return lefsim.Sim(
        200,
        ctcf_sites=[60, 100, 140],
        ctcf_captures_from=["right", "right", "left"],
        ctcf_strength=strength,
        n_lefs=n_lefs,
        seed=seed,
        **kw,
    )


def table(obj):
    return pa.table(obj)


# ---------------------------------------------------------------- construction


def test_construction_and_properties():
    sim = make_sim(n_lefs=3)
    assert (sim.n_sites, sim.n_lefs, sim.n_legs) == (200, 3, 6)
    assert sim.time == 0.0 and sim.event_count == 0
    assert sim.next_event_time > 0
    assert sim.rates == {"step_rate": 1.0, "respawn_rate": 0.02, "respawn_rate_captured": 0.005}


def test_default_rates_come_from_the_core():
    assert lefsim.Sim(10).rates == {
        "step_rate": 1.0,
        "respawn_rate": 1e-3,
        "respawn_rate_captured": 1e-4,
    }


def test_explicit_lef_sites_come_first():
    snap = table(lefsim.Sim(100, lef_sites=[10, 90], n_lefs=2).snapshot())
    assert snap["lef_id"].to_pylist() == [0, 1, 2, 3]
    assert snap["left_pos"].to_pylist()[:2] == [10, 90]


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
        (dict(n_sites=10, lef_sites=[10]), "outside"),
        (dict(n_sites=10, n_lefs=6), "too many LEFs"),
        (dict(n_sites=10, step_rate=0.0), "step_rate must be positive"),
        (dict(n_sites=10, respawn_rate=float("nan")), "respawn_rate must be positive"),
    ],
)
def test_construction_errors(kwargs, match):
    n_sites = kwargs.pop("n_sites")
    with pytest.raises(ValueError, match=match):
        lefsim.Sim(n_sites, **kwargs)


def test_captures_from_is_case_insensitive_and_broadcasts():
    sim = lefsim.Sim(50, ctcf_sites=[10, 20], ctcf_captures_from="Right", ctcf_strength=0.5)
    ctcfs = table(sim.meta()["ctcfs"])
    assert ctcfs["captures_from"].to_pylist() == ["Right", "Right"]
    assert ctcfs["strength"].to_pylist() == [0.5, 0.5]


def test_numpy_inputs_are_accepted():
    sim = lefsim.Sim(
        50,
        ctcf_sites=np.array([10, 20]),
        ctcf_captures_from=["left", "right"],
        ctcf_strength=np.array([0.2, 0.9]),
        lef_sites=np.array([30]),
    )
    assert table(sim.meta()["ctcfs"])["site"].to_pylist() == [10, 20]


# ---------------------------------------------------------------- determinism


def fnv1a(data: bytes, h: int = 0xCBF29CE484222325) -> int:
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def test_python_reproduces_the_rust_golden_stream():
    """Same run as crates/lefsim/tests/determinism.rs, hashed the same way
    from the Arrow output. Proves Python sees exactly the Rust trajectory."""
    sites = list(range(25, 500, 50))
    sim = lefsim.Sim(
        500,
        ctcf_sites=sites,
        ctcf_captures_from=["right" if i % 100 == 25 else "left" for i in sites],
        ctcf_strength=0.7,
        n_lefs=20,
        seed=0x5EED,
        step_rate=1.0,
        respawn_rate=0.01,
        respawn_rate_captured=0.002,
    )
    rec = sim.run_events(50_000, dt=5.0, events=True)
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
    a = make_sim(seed=5).run_until(100.0, dt=0.5, events=True)
    b = make_sim(seed=5).run_until(100.0, dt=0.5, events=True)
    c = make_sim(seed=6).run_until(100.0, dt=0.5, events=True)
    assert table(a.snapshots).equals(table(b.snapshots))
    assert table(a.events).equals(table(b.events))
    assert not table(a.snapshots).equals(table(c.snapshots))


# ---------------------------------------------------------------- running


def test_run_until_records_an_inclusive_grid():
    rec = make_sim().run_until(10.0, dt=0.5)
    t = table(rec.snapshots)
    assert rec.stopped == "reached_time" and rec.time == 10.0
    assert np.unique(t["time"].to_numpy()).tolist() == [0.5 * k for k in range(21)]
    assert t.num_rows == 21 * 8


def test_consecutive_calls_continue_the_grid():
    whole = make_sim(seed=2).run_until(50.0, dt=1.0, events=True)

    sim = make_sim(seed=2)
    parts = [sim.run_until(t, dt=1.0, events=True) for t in (10.0, 10.0, 33.3, 50.0)]
    snaps = pa.concat_tables([table(p.snapshots) for p in parts])
    events = pa.concat_tables([table(p.events) for p in parts])
    assert snaps.equals(table(whole.snapshots))
    assert events.equals(table(whole.events))


def test_run_events_counts_and_recording_options():
    sim = make_sim()
    rec = sim.run_events(1000)
    assert rec.events_fired == 1000 and rec.stopped == "reached_event_count"
    assert rec.snapshots is None and rec.events is None
    assert sim.event_count == 1000


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
    assert lefsim.Sim(10).step() is None


def test_snapshot_matches_recorded_state():
    sim = make_sim(seed=3)
    rec = sim.run_until(20.0, dt=20.0)
    last = table(rec.snapshots).slice(8)  # the t=20 snapshot
    now = table(sim.snapshot())
    assert now["time"].to_pylist() == [20.0] * 8
    assert now.drop_columns(["time"]).equals(last.drop_columns(["time"]))


def test_snapshots_never_show_evicted():
    rec = make_sim(seed=4, strength=1.0).run_until(500.0, dt=0.25, events=True)
    ev = table(rec.events)
    assert "Evicted" in ev["mobility"].to_pylist()
    snaps = table(rec.snapshots)
    assert "Evicted" not in snaps["left_mobility"].to_pylist()
    assert "Evicted" not in snaps["right_mobility"].to_pylist()


# ---------------------------------------------------------------- layouts


def test_long_layout_is_wide_unpivoted():
    wide = table(make_sim(seed=7).run_until(30.0, dt=1.0).snapshots)
    long = table(make_sim(seed=7).run_until(30.0, dt=1.0, layout="long").snapshots)
    assert long.column_names == ["time", "leg_id", "lef_id", "facing", "pos", "mobility"]
    assert long.num_rows == 2 * wide.num_rows

    left = long.filter(pa.compute.equal(long["facing"].cast(pa.string()), "Left"))
    right = long.filter(pa.compute.equal(long["facing"].cast(pa.string()), "Right"))
    assert left["pos"].equals(wide["left_pos"])
    assert right["pos"].equals(wide["right_pos"])
    assert left["lef_id"].equals(wide["lef_id"])
    assert (left["leg_id"].to_numpy() == 2 * wide["lef_id"].to_numpy()).all()
    assert left["mobility"].cast(pa.string()).equals(wide["left_mobility"].cast(pa.string()))


def test_bad_layout():
    with pytest.raises(ValueError, match="layout"):
        make_sim().run_until(1.0, dt=1.0, layout="tall")


# ---------------------------------------------------------------- consumers


def test_batch_is_readable_by_every_consumer():
    batch = make_sim().run_until(20.0, dt=1.0).snapshots
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
    rec = make_sim(strength=1.0).run_until(300.0, dt=1.0, events=True)
    ev = table(rec.events)
    assert ev.schema.field("action").type == pa.dictionary(pa.uint8(), pa.string())
    assert set(ev["action"].to_pylist()) == {"Step", "Respawn"}
    assert set(ev["mobility"].to_pylist()) <= {"Moving", "Captured", "Evicted"}


def test_batch_metadata_and_repr():
    batch = make_sim(n_lefs=3).run_until(1.0, dt=1.0).snapshots
    assert batch.metadata == {
        "lefsim.kind": "snapshots",
        "lefsim.layout": "wide",
        "lefsim.n_lefs": "3",
    }
    assert len(batch) == batch.num_rows == 6
    assert "snapshots" in repr(batch)


def test_wide_columns_reshape_to_snapshot_by_lef():
    batch = table(make_sim(n_lefs=8).run_until(10.0, dt=1.0).snapshots)
    left = batch["left_pos"].to_numpy().reshape(-1, 8)
    assert left.shape == (11, 8)
    assert (batch["lef_id"].to_numpy().reshape(-1, 8) == np.arange(8)).all()


# ---------------------------------------------------------------- meta


def test_meta_tables():
    meta = make_sim(n_lefs=4).meta()
    assert set(meta) == {"lattice", "ctcfs", "lefs", "legs", "rates"}
    ctcfs = table(meta["ctcfs"])
    assert ctcfs.to_pydict() == {
        "site": [60, 100, 140],
        "captures_from": ["Right", "Right", "Left"],
        "strength": [0.8, 0.8, 0.8],
    }
    lattice = table(meta["lattice"])
    assert lattice.num_rows == 200
    assert lattice["kind"].to_pylist().count("Ctcf") == 3
    assert table(meta["lefs"]).to_pydict() == {
        "lef_id": [0, 1, 2, 3],
        "left_leg_id": [0, 2, 4, 6],
        "right_leg_id": [1, 3, 5, 7],
    }
    legs = table(meta["legs"])
    assert legs["facing"].to_pylist() == ["Left", "Right"] * 4
    assert legs["lef_id"].to_pylist() == [0, 0, 1, 1, 2, 2, 3, 3]


# ---------------------------------------------------------------- streaming


def test_iterating_a_stream_matches_run_until():
    expected = make_sim(seed=9).run_until(200.0, dt=0.5, events=False).snapshots
    stream = make_sim(seed=9).stream(200.0, dt=0.5, chunk_size=37)
    batches = list(stream)
    assert stream.closed
    assert len(batches) > 5
    assert all(b.num_rows <= 37 * 8 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


def test_event_stream_matches_run_until():
    expected = make_sim(seed=10).run_until(150.0, events=True).events
    stream = make_sim(seed=10).stream(150.0, events=True, chunk_size=50)
    batches = list(stream)
    assert all(b.num_rows <= 50 for b in batches)
    assert pa.concat_tables(table(b) for b in batches).equals(table(expected))


@pytest.mark.parametrize("consumer", ["pyarrow", "polars", "duckdb"])
def test_stream_capsule_consumers(consumer):
    expected = table(make_sim(seed=11).run_until(100.0, dt=0.5).snapshots)
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


def test_stream_consumes_the_sim():
    sim = make_sim()
    sim.stream(10.0, dt=1.0)
    with pytest.raises(RuntimeError, match="consumed"):
        sim.run_until(20.0)
    assert "consumed" in repr(sim)


def test_stream_can_be_read_once():
    stream = make_sim().stream(10.0, dt=1.0)
    pa.RecordBatchReader.from_stream(stream).read_all()
    with pytest.raises(OSError, match="already read"):
        pa.RecordBatchReader.from_stream(stream)
    assert list(stream) == []


def test_stream_continues_the_grid_of_earlier_calls():
    whole = table(make_sim(seed=12).run_until(40.0, dt=1.0).snapshots)
    sim = make_sim(seed=12)
    head = table(sim.run_until(15.0, dt=1.0).snapshots)
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
    ],
)
def test_stream_argument_errors(kwargs, match):
    sim = make_sim()
    with pytest.raises(ValueError, match=match):
        sim.stream(10.0, **kwargs)
    sim.run_until(1.0)  # a rejected stream() leaves the Sim usable


# ---------------------------------------------------------------- GIL / signals


def test_long_runs_release_the_gil_and_honor_ctrl_c():
    # A timer thread can only fire while the run holds no GIL, and its
    # interrupt is only seen if the run checks for signals between chunks.
    sim = make_sim(n_lefs=50)
    timer = threading.Timer(0.3, _thread.interrupt_main)
    timer.start()
    try:
        with pytest.raises(KeyboardInterrupt):
            sim.run_until(1e12, dt=1.0)
    finally:
        timer.cancel()
    # The sim is left consistent and usable.
    t = sim.time
    assert t > 0
    rec = sim.run_until(t + 10.0, dt=1.0)
    assert rec.stopped == "reached_time"


def test_import_pulls_in_no_dataframe_libraries():
    code = (
        "import sys, lefsim;"
        "r = lefsim.Sim(100, n_lefs=3).run_until(50.0, dt=1.0, events=True);"
        "assert r.snapshots.num_rows == 51 * 3;"
        "bad = {'pyarrow', 'polars', 'duckdb', 'pandas', 'numpy'} & set(sys.modules);"
        "assert not bad, bad"
    )
    subprocess.run([sys.executable, "-c", code], check=True)


# ---------------------------------------------------------------- keyframes


def replay(events: pa.Table, n_legs: int) -> dict:
    """Apply delta rows in order; returns {leg_id: (pos, mobility)}."""
    state = {}
    for leg, pos, mob in zip(
        events["leg_id"].to_pylist(), events["pos"].to_pylist(), events["mobility"].to_pylist()
    ):
        state[leg] = (pos, mob)
    assert len(state) == n_legs
    return state


def test_action_dictionary_includes_keyframe():
    ev = pa.record_batch(make_sim().run_until(10.0, events=True).events)
    assert ev["action"].dictionary.to_pylist() == ["Step", "Respawn", "Keyframe"]


def test_keyframed_events_open_with_every_leg():
    ev = table(make_sim(n_lefs=5).run_until(200.0, events=True, keyframes=True).events)
    head = ev.slice(0, 10)
    assert head["action"].to_pylist() == ["Keyframe"] * 10
    assert head["leg_id"].to_pylist() == list(range(10))
    assert set(head["time"].to_pylist()) == {0.0}
    assert "Keyframe" not in ev.slice(10)["action"].to_pylist()


def test_keyframed_stream_replays_and_reduces_to_the_plain_log():
    plain_sim = make_sim(seed=13)
    plain = table(plain_sim.run_until(300.0, events=True).events)
    n_legs = plain_sim.n_legs

    stream = make_sim(seed=13).stream(300.0, events=True, keyframes=True, chunk_size=60)
    batches = [table(b) for b in stream]
    assert len(batches) > 10
    for b in batches:
        assert b.slice(0, n_legs)["action"].to_pylist() == ["Keyframe"] * n_legs
        assert b.num_rows <= 60 + 2  # soft only by a zero-delay chain

    # Each batch replays on its own; the last one ends in the plain run's state.
    snap = table(plain_sim.snapshot(layout="long"))
    expected = dict(zip(snap["leg_id"].to_pylist(), zip(snap["pos"].to_pylist(), snap["mobility"].to_pylist())))
    assert replay(batches[-1], n_legs) == expected

    # Dropping keyframes leaves exactly the plain event log.
    events = pa.concat_tables(batches)
    events = events.filter(pa.compute.not_equal(events["action"].cast(pa.string()), "Keyframe"))
    assert events.to_pydict() == plain.to_pydict()


@pytest.mark.parametrize(
    "call, match",
    [
        (lambda s: s.run_until(10.0, keyframes=True), "requires events=True"),
        (lambda s: s.stream(10.0, dt=1.0, keyframes=True), "requires events=True"),
        (lambda s: s.stream(10.0, events=True, keyframes=True, chunk_size=17), r"at least 18 with keyframes"),
        (lambda s: s.stream(10.0, events=True, chunk_size=1), "at least 2 for events"),
    ],
)
def test_keyframe_argument_errors(call, match):
    sim = make_sim()  # 8 LEFs = 16 legs
    with pytest.raises(ValueError, match=match):
        call(sim)
    sim.run_until(1.0)  # still usable
