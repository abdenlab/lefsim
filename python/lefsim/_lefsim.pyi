from collections.abc import Iterable, Iterator, Mapping, Sequence
from typing import Any, Literal, Optional, Union

__version__: str

Layout = Literal["lef", "leg"]
Stopped = Literal["reached_time", "reached_event_count", "exhausted", "recorder_full"]
SpawnRuleName = Literal["uniform_non_ctcf"]

class Batch:
    """An immutable Arrow record batch.

    Implements `__arrow_c_array__`, `__arrow_c_stream__` and
    `__arrow_c_schema__`, so it can be passed directly to e.g.
    `pyarrow.table(...)`, `polars.DataFrame(...)` or queried by name from
    duckdb.
    """

    @property
    def num_rows(self) -> int:
        """Number of rows."""
    @property
    def num_columns(self) -> int:
        """Number of columns."""
    @property
    def column_names(self) -> list[str]:
        """The column names, in order."""
    @property
    def metadata(self) -> dict[str, str]:
        """Schema metadata, e.g. `{"lefsim.kind": "snapshots", ...}`."""
    def __len__(self) -> int: ...
    def __arrow_c_schema__(self) -> object: ...
    def __arrow_c_array__(self, requested_schema: object = None) -> tuple[object, object]: ...
    def __arrow_c_stream__(self, requested_schema: object = None) -> object: ...
    def to_pyarrow(self) -> Any:
        """As a `pyarrow.Table` (requires pyarrow)."""
    def to_polars(self) -> Any:
        """As a `polars.DataFrame` (requires polars)."""
    def to_pandas(self) -> Any:
        """As a `pandas.DataFrame` (requires pyarrow and pandas)."""

class Lattice:
    """A 1D lattice of `n_sites` sites, some of them CTCFs.

    `ctcfs` is a table with one row per CTCF: `site`, `captures_from`
    ('left' or 'right': the side that captures) and optionally `strength`
    (the probability, in [0, 1], that a landing leg interacts; 1 by
    default). Pass any Arrow table (pyarrow, polars, `Lattice.to_arrow()`)
    or a dict of columns, where `captures_from` and `strength` may be single
    values for all CTCFs.
    """

    def __init__(self, n_sites: int, ctcfs: Union[Mapping[str, Any], Any, None] = None) -> None: ...
    @property
    def n_sites(self) -> int:
        """Number of sites."""
    @property
    def n_ctcfs(self) -> int:
        """Number of CTCF sites."""
    @property
    def n_plain(self) -> int:
        """Number of non-CTCF sites."""
    def to_arrow(self) -> Batch:
        """The CTCF table (`site`, `captures_from`, `strength`), with
        `lefsim.n_sites` in the schema metadata.
        """
    def to_dict(self) -> Any:
        """As plain Python data (dicts, lists, numbers and strings), in the same
        shape as the JSON.
        """
    @classmethod
    def from_dict(cls, d: Any) -> "Lattice":
        """Load from plain Python data shaped as `to_dict` returns it, with the same checks as
        construction.
        """
    def to_json(self, *, indent: Optional[int] = None) -> str:
        """As a JSON string; `indent` pretty-prints."""
    @classmethod
    def from_json(cls, s: str) -> "Lattice":
        """Load from a JSON string as `to_json` writes it, with the same checks as construction."""

class World:
    """The state of a simulation at one moment: a lattice and the LEFs on it.

    `lefs` is a table with one row per LEF (row `i` is LEF `i`): columns
    `left_pos` and `right_pos`, and optionally `left_mobility` and
    `right_mobility` ('moving' by default). Pass any Arrow table (pyarrow,
    polars, `World.to_arrow()`, `Sim.state()`) or a dict of sequences.

    Every LEF is checked: legs on the lattice and not crossed, no two LEFs
    on one site, and a 'captured' leg only on a CTCF that could have captured
    it (a left-facing leg by a CTCF that captures from the right, a
    right-facing leg by one that captures from the left).
    """

    def __init__(self, lattice: Lattice, lefs: Union[Mapping[str, Sequence[Any]], Any, None] = None) -> None: ...
    @property
    def lattice(self) -> Lattice:
        """The lattice."""
    @property
    def n_lefs(self) -> int:
        """Number of LEFs."""
    def to_arrow(self) -> Batch:
        """The LEF table: `lef_id`, `left_pos`, `right_pos`, `left_mobility`,
        `right_mobility`.
        """
    def effective_distance(self, a: int, b: int, *, step_length: int, hop_length: int) -> int:
        """The effective distance between sites `a` and `b`: the shortest path
        when a path walks the lattice (`step_length` per step) and may cross
        LEFs (`hop_length` per crossing; across a chain of LEFs side by side,
        a rosette, a hop per LEF and the junctions between them free). Both
        lengths are integers, so distances are exact; divide by `step_length`
        for lattice steps.
        """
    def effective_distances(
        self, sites: Sequence[int], targets: Optional[Sequence[int]] = None, *, step_length: int, hop_length: int
    ) -> Any:
        """Effective distances (see `effective_distance`). Without `targets`,
        every pair of `sites`, condensed as `scipy.spatial.distance.pdist`
        (`squareform` makes it a matrix). With `targets`, each of `sites` to
        each of `targets`, row-major. Returns an `array.array("q")`;
        `numpy.asarray` wraps it without copying.
        """
    def to_dict(self) -> Any:
        """As plain Python data (dicts, lists, numbers and strings), in the same
        shape as the JSON.
        """
    @classmethod
    def from_dict(cls, d: Any) -> "World":
        """Load from plain Python data shaped as `to_dict` returns it, with the same checks as
        construction.
        """
    def to_json(self, *, indent: Optional[int] = None) -> str:
        """As a JSON string; `indent` pretty-prints."""
    @classmethod
    def from_json(cls, s: str) -> "World":
        """Load from a JSON string as `to_json` writes it, with the same checks as construction."""

def populate(lattice: Lattice, n_lefs: int, *, seed: int = 0, spawn_rule: SpawnRuleName = "uniform_non_ctcf") -> World:
    """A world with `n_lefs` LEFs placed one at a time by a spawn rule, exactly
    as respawns place them. `seed` drives only this placement.
    """

class Rates:
    """Event rates (per unit time): `step` for a moving, unblocked leg (a leg
    facing a CTCF from behind splits it between passing and eviction);
    `respawn`, the basal rate, for a LEF with both legs free to move;
    `respawn_blocked` for a LEF with either leg blocked (by a leg ahead or the
    lattice's end); `respawn_captured` for one with either leg captured,
    which wins over blocked. Eviction unloads a LEF at once; a leg whose
    partner is captured is immune to it. Checked (positive and finite) by
    `Model`.
    """

    def __init__(
        self,
        step: Optional[float] = None,
        respawn: Optional[float] = None,
        respawn_blocked: Optional[float] = None,
        respawn_captured: Optional[float] = None,
    ) -> None: ...
    @property
    def step(self) -> float:
        """Step rate of a moving, unblocked leg."""
    @property
    def respawn(self) -> float:
        """Basal respawn rate: a LEF with both legs free to move."""
    @property
    def respawn_blocked(self) -> float:
        """Respawn rate of a LEF with either leg blocked (by a leg ahead or the
        lattice's end), unless the other is captured."""
    @property
    def respawn_captured(self) -> float:
        """Respawn rate of a LEF with a captured leg, whatever the other leg does."""
    def to_dict(self) -> Any:
        """As plain Python data (dicts, lists, numbers and strings), in the same
        shape as the JSON.
        """
    @classmethod
    def from_dict(cls, d: Any) -> "Rates":
        """Load from plain Python data shaped as `to_dict` returns it, with the same checks as
        construction.
        """
    def to_json(self, *, indent: Optional[int] = None) -> str:
        """As a JSON string; `indent` pretty-prints."""
    @classmethod
    def from_json(cls, s: str) -> "Rates":
        """Load from a JSON string as `to_json` writes it, with the same checks as construction."""

class Model:
    """A complete model: a world, its rates, and a spawn rule. Everything a run
    needs except a seed.

    Checks that the rates are positive and finite, and that the spawn rule
    can always place the world's LEFs. `to_json()` and `Model.from_json()`
    save and load it as a versioned document.
    """

    def __init__(
        self, world: World, rates: Optional[Rates] = None, *, spawn_rule: SpawnRuleName = "uniform_non_ctcf"
    ) -> None: ...
    @property
    def world(self) -> World:
        """The world."""
    @property
    def lattice(self) -> Lattice:
        """The world's lattice."""
    @property
    def rates(self) -> Rates:
        """The rates."""
    @property
    def spawn_rule(self) -> SpawnRuleName:
        """The spawn rule's name."""
    def to_dict(self) -> Any:
        """As plain Python data (dicts, lists, numbers and strings), in the same
        shape as the JSON.
        """
    @classmethod
    def from_dict(cls, d: Any) -> "Model":
        """Load from plain Python data shaped as `to_dict` returns it, with the same checks as
        construction.
        """
    def to_json(self, *, indent: Optional[int] = None) -> str:
        """As the versioned JSON model document (`lefsim.model`, version 2); `indent` pretty-prints."""
    @classmethod
    def from_json(cls, s: str) -> "Model":
        """Load a JSON model document as `to_json` writes it, with the same checks as construction."""

class RunReport:
    """What a run did: events fired, the time reached, and why it stopped
    ("reached_time", "reached_event_count", "exhausted", or for the chunk
    methods "recorder_full").
    """

    n_events: int
    """Events fired by this call."""
    time: float
    """Simulation time after the call."""
    stopped: Stopped
    """Why the run stopped: "reached_time", "reached_event_count", "exhausted", or, for the chunk
    methods, "recorder_full".
    """

class Recording:
    """What a `record_*` call recorded, plus its run report fields."""

    snapshots: Optional[Batch]
    """Grid snapshots, if `dt` was given."""
    events: Optional[Batch]
    """Event deltas, if `events=True`."""
    n_events: int
    """Events fired by this call."""
    time: float
    """Simulation time after the call: the recording describes the run
    through this time.
    """
    stopped: Stopped
    """Why the run stopped: "reached_time", "reached_event_count", "exhausted", or, for the chunk
    methods, "recorder_full".
    """

class Stream:
    """A simulation that runs lazily as it is read, one Arrow batch at a time.

    Iterate it for `lefsim.Batch`es, or pass it to an Arrow PyCapsule stream
    consumer. Either way it can be read only once.
    """

    @property
    def column_names(self) -> list[str]:
        """The column names of every batch, in order."""
    @property
    def closed(self) -> bool:
        """Whether the stream has been fully read or handed off."""
    @property
    def time(self) -> float:
        """The time the batches read so far (by iterating) describe the run
        through: where the next batch starts. At the end of the stream it's
        the time the stream was asked to end at.
        """
    def to_parquet(self, path: Any, *, compression: str = "zstd") -> None:
        """Write this stream to a Parquet file as it runs, one row group per
        batch, so the file indexes itself for random access
        (`Replayer.from_parquet`). Only for keyframed event streams
        (`events=True, keyframes=True`). The footer carries `lefsim.model`
        and `lefsim.run`, with `record_end` set to where the data actually
        ends (if writing stops early, e.g. on Ctrl-C, the file is still
        valid up to there), and `lefsim.row_groups = "keyframed"`.
        `compression` is any codec pyarrow writes; the default, zstd, and
        snappy are also readable by the TS package's `lefsim/parquet`.
        Requires pyarrow. Consumes the stream.
        """
    def __iter__(self) -> Iterator[Batch]: ...
    def __next__(self) -> Batch: ...
    def __arrow_c_schema__(self) -> object: ...
    def __arrow_c_stream__(self, requested_schema: object = None) -> object: ...

class Sim:
    """A running simulation of a `Model`.

    `seed` drives the whole run: the same model and seed give the same run,
    bit for bit, on every platform. `start_time` sets the clock, e.g. to
    continue from a state recorded at that time. The model is copied in;
    `sim.model()` returns the current state as a new `Model`.

    `run_*` methods advance the run; `record_*` methods advance it and
    return what happened as Arrow data. `copy.copy(sim)` forks the run: the
    copy continues exactly as the original would.
    """

    def __init__(self, model: Model, *, seed: int = 0, start_time: float = 0.0) -> None: ...
    @property
    def time(self) -> float:
        """Current simulation time."""
    @property
    def n_events(self) -> int:
        """Total events fired so far."""
    @property
    def next_event_time(self) -> Optional[float]:
        """Time of the next event, or `None` if none remain."""
    @property
    def n_sites(self) -> int:
        """Number of lattice sites."""
    @property
    def n_lefs(self) -> int:
        """Number of LEFs."""
    @property
    def n_legs(self) -> int:
        """Number of legs (two per LEF)."""
    @property
    def rates(self) -> Rates:
        """The rates this run uses."""
    def model(self) -> Model:
        """The current state as a `Model`: the world now, with this run's rates
        and spawn rule. `Sim(sim.model(), seed=..., start_time=sim.time)`
        continues from here with fresh randomness.
        """
    def initial_model(self) -> Model:
        """The model this run started from. With `seed` and `start_time`, it
        says where a recording came from: `Sim(sim.initial_model(),
        seed=sim.seed, start_time=sim.start_time)` replays the run from its
        start, event for event.
        """
    @property
    def seed(self) -> int:
        """The seed this run was started with."""
    @property
    def start_time(self) -> float:
        """The clock at construction."""
    def world(self) -> World:
        """The current state as a `World`."""
    def state(self, *, layout: Layout = "lef") -> Batch:
        """The current state as a one-snapshot Batch, without advancing."""
    def __copy__(self) -> "Sim": ...
    def __deepcopy__(self, memo: Any) -> "Sim": ...
    def step(self) -> Optional[tuple[float, Literal["Step", "Respawn", "Evict"], int]]:
        """Fire one event. Returns `(time, action, id)` — `id` is a `leg_id`
        for "Step" and "Evict", a `lef_id` for "Respawn" — or `None` if no
        events remain.
        """
    def run_until(self, t_end: float) -> RunReport:
        """Fire every event at or before `t_end`, then set the clock to `t_end`.
        Releases the GIL while running; Ctrl-C interrupts.
        """
    def run_events(self, n: int) -> RunReport:
        """Fire up to `n` events."""
    def record_until(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
    ) -> Recording:
        """Run until `t_end`, recording grid snapshots (with `dt`, on the grid
        `t0 + k * dt`, inclusive of `t_end`) and/or event deltas (with
        `events=True`; add `keyframes=True` to open the event table with one
        `Keyframe` row per leg holding the full state). Consecutive calls on
        the same grid continue it seamlessly. Releases the GIL while running;
        Ctrl-C interrupts.
        """
    def record_events(
        self,
        n: int,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
    ) -> Recording:
        """Fire up to `n` events, recording as in `record_until`; snapshots
        cover grid points up to the current time.
        """
    def record_snapshot_chunk(
        self, t_end: float, *, dt: float, t0: float = 0.0, max_snapshots: int, layout: Layout = "lef"
    ) -> Recording:
        """Run toward `t_end`, taking at most `max_snapshots` grid snapshots.
        `stopped` is "recorder_full" when the chunk filled first: call again
        for the next one. Consecutive chunks tile the grid exactly.
        """
    def record_event_chunk(self, t_end: float, *, max_rows: int, keyframes: bool = True) -> Recording:
        """Run toward `t_end`, logging event deltas into a chunk of about
        `max_rows` rows. With `keyframes` (the default) the chunk opens with
        the full state, so it can be replayed on its own, and may exceed
        `max_rows` by the rows of a zero-delay chain in progress (chunks only
        end at settled times). `stopped` is "recorder_full" when the chunk
        filled first: call again for the next one. `time` is the time through
        which the chunk describes the run.
        """
    def stream(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
        max_snapshots: Optional[int] = None,
        max_rows: Optional[int] = None,
    ) -> Stream:
        """Hand this simulation to a lazy Arrow stream that runs it to `t_end`,
        one bounded chunk per batch, as the `record_*_chunk` methods would.

        Pass exactly one of `dt` (grid snapshots, at most `max_snapshots` per
        batch) or `events=True` (deltas, about `max_rows` rows per batch).
        With `events=True, keyframes=True`, every batch opens with one
        `Keyframe` row per leg, so each batch can be replayed on its own; a
        batch may then exceed `max_rows` by the rows of a zero-delay chain
        in progress (typically two), since batches only end at settled times.
        The stream can be iterated for `Batch`es, or passed to any Arrow
        PyCapsule consumer (e.g. `pyarrow.RecordBatchReader.from_stream`,
        `polars.DataFrame`, duckdb). It runs without the GIL, so it can be
        consumed from any thread.

        The stream's schema metadata says where it came from, once for the
        whole stream: `lefsim.model` (the model the run started from, as
        JSON) and `lefsim.run` (JSON: `seed` as a string, `start_time`,
        `record_start`, the requested `record_end` or null, `lefsim_version`).

        The Sim is moved into the stream and can't be used afterwards; copy
        it first (`copy.copy(sim)`) to keep going from here.
        """

class Replayer:
    """Rebuilds the state of a run at any recorded time from a keyframed event
    log (`events=True, keyframes=True`), with no simulation.

    Use `Replayer.from_arrow(...)` to load a whole log, or `push()` chunks as
    they arrive. Then `seek(t)` (any direction) or `advance_to(t)` (forward,
    cheaper), and read `state()`, `world()` or `snapshots(dt)`. Rows before
    the first keyframe are skipped, so a log may start partway into a run.

    The state at time `t` is the state after every event at or before `t`,
    exactly what `Sim.run_until(t)` leaves. `world_at(t)` returns it as a
    `World`, to start a new run from.
    """

    def __init__(self, lattice: Lattice, n_lefs: int) -> None: ...
    @classmethod
    def from_arrow(
        cls,
        source: Union[Any, Iterable[Any]],
        lattice: Lattice,
        *,
        n_lefs: Optional[int] = None,
        end: Optional[float] = None,
    ) -> "Replayer":
        """Load a whole keyframed event log: any Arrow table, batch or stream
        (pyarrow, polars, a `Stream`, `pq.ParquetFile(...).iter_batches()`)
        or an iterable of them. Batch boundaries don't matter: the log is
        re-split at its keyframes. `n_lefs` defaults to the size of the
        first keyframe. `end` is the time through which the log describes
        the run (e.g. the `t_end` it was recorded to); it defaults to the
        last row, since event rows alone can't say how long the state after
        the last event lasted.
        """
    @classmethod
    def from_parquet(cls, path: Any, lattice: Optional[Lattice] = None) -> "Replayer":
        """Open a Parquet file written by `Stream.to_parquet` for random access:
        only the footer is read up front, and each `seek`, `advance_to` or
        `snapshots` reads just the row groups it needs (one per keyframed
        chunk, found from the footer's time statistics). The lattice and the
        end come from the file's `lefsim.model` and `lefsim.run`; pass
        `lattice` for a file without them. A file not written that way
        (e.g. by `duckdb COPY`) is read whole, as `from_arrow` would.
        Requires pyarrow.
        """
    @property
    def row_groups_read(self) -> int:
        """How many Parquet row groups `from_parquet` has read so far (0 for a
        replayer not backed by a file).
        """
    def push(self, events: Any, end: Optional[float] = None) -> None:
        """Append a chunk of event rows (whole keyframes only). `end` is the
        time through which the chunk describes the run (a `Recording`'s
        `time`); it defaults to the chunk's last row.
        """
    @property
    def lattice(self) -> Lattice:
        """The lattice of the recorded run."""
    @property
    def n_lefs(self) -> int:
        """Number of LEFs in the recorded run."""
    @property
    def ready(self) -> bool:
        """Whether a keyframe has been applied, i.e. whether there's a state."""
    @property
    def time(self) -> float:
        """Current replay time (`-inf` until ready)."""
    @property
    def start(self) -> Optional[float]:
        """Earliest time that can be sought to (the first keyframe), or `None`."""
    @property
    def end(self) -> float:
        """Latest time the log describes."""
    def advance_to(self, t: float) -> float:
        """Move forward to `t` (or back, by seeking), clamped to the data: it
        never fails. Returns the time reached.
        """
    def seek(self, t: float) -> float:
        """Jump to `t` in either direction (clamped to `end`)."""
    def state(self, *, layout: Layout = "lef") -> Batch:
        """The state at `time` as a one-snapshot Batch, like `Sim.state()`."""
    def world(self) -> World:
        """The state at `time` as a `World`."""
    def world_at(self, t: float) -> World:
        """Seek to `t` and return the state there as a `World`."""
    def snapshots(
        self,
        dt: float,
        *,
        t0: float = 0.0,
        t_start: Optional[float] = None,
        t_end: Optional[float] = None,
        layout: Layout = "lef",
    ) -> Batch:
        """Snapshots on the grid `t0 + k * dt` from `t_start` through `t_end`
        (default: the end of the log): the same table
        `Sim.record_until(dt=...)` records. Without `t_start`, starts from
        the current time (or the first keyframe), so consecutive calls on the
        same grid continue it.
        """
    def trim_before(self, t: float) -> None:
        """Drop data no longer needed to seek to `t` or later."""
    def effective_distances(
        self,
        sites: Sequence[int],
        *,
        dt: float,
        t0: float = 0.0,
        t_start: Optional[float] = None,
        t_end: Optional[float] = None,
        step_length: int,
        hop_length: int,
    ) -> tuple[Any, Any]:
        """For each point of the grid `snapshots(dt, ...)` would take, the
        effective distance of every pair of `sites` (condensed, as
        `World.effective_distances`). Returns `(times, distances)`:
        `array.array("d")` of grid times, and `array.array("q")` of distances,
        row-major, one row of `k(k-1)/2` per time. A replayer opened with
        `from_parquet` reads row groups as it walks the grid.
        """
