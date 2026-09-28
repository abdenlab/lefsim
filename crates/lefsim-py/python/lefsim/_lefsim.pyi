from collections.abc import Iterable, Iterator, Mapping, Sequence
from typing import Any, Literal, Optional, Union

__version__: str

Layout = Literal["lef", "leg"]
Stopped = Literal["reached_time", "reached_event_count", "exhausted", "recorder_full"]
SpawnRuleName = Literal["uniform_non_ctcf"]

class Batch:
    """An Arrow record batch (Arrow PyCapsule Interface)."""

    @property
    def num_rows(self) -> int: ...
    @property
    def num_columns(self) -> int: ...
    @property
    def column_names(self) -> list[str]: ...
    @property
    def metadata(self) -> dict[str, str]: ...
    def __len__(self) -> int: ...
    def __arrow_c_schema__(self) -> object: ...
    def __arrow_c_array__(self, requested_schema: object = None) -> tuple[object, object]: ...
    def __arrow_c_stream__(self, requested_schema: object = None) -> object: ...
    def to_pyarrow(self) -> Any:
        """As a ``pyarrow.Table`` (requires pyarrow)."""
    def to_polars(self) -> Any:
        """As a ``polars.DataFrame`` (requires polars)."""
    def to_pandas(self) -> Any:
        """As a ``pandas.DataFrame`` (requires pyarrow and pandas)."""

class Lattice:
    """A 1D lattice of ``n_sites`` sites, some of them CTCFs.

    ``ctcf_captures_from`` says which side of each CTCF captures ("left" or
    "right"); ``ctcf_strength`` is the probability, in [0, 1], that a landing
    leg interacts with it. Both may be one value for all CTCFs.
    """

    def __init__(
        self,
        n_sites: int,
        *,
        ctcf_sites: Sequence[int] = (),
        ctcf_captures_from: Union[Literal["left", "right"], Sequence[str], None] = None,
        ctcf_strength: Union[float, Sequence[float], None] = None,
    ) -> None: ...
    @property
    def n_sites(self) -> int: ...
    @property
    def n_plain(self) -> int:
        """Number of non-CTCF sites."""
    def to_arrow(self) -> Batch:
        """The CTCF table (``site``, ``captures_from``, ``strength``), with
        ``lefsim.n_sites`` in the schema metadata."""
    def to_dict(self) -> Any:
        """As plain Python data, in the same shape as the JSON."""
    @classmethod
    def from_dict(cls, d: Any) -> "Lattice": ...
    def to_json(self, *, indent: Optional[int] = None) -> str: ...
    @classmethod
    def from_json(cls, s: str) -> "Lattice": ...

class World:
    """The state of a simulation at one moment: a lattice and the LEFs on it.

    ``lefs`` has one row per LEF (row ``i`` is LEF ``i``): columns
    ``left_pos`` and ``right_pos``, and optionally ``left_mobility`` and
    ``right_mobility`` ("moving" by default). Pass any Arrow table (pyarrow,
    polars, ``World.to_arrow()``, ``Sim.state()``) or a dict of sequences.
    Every LEF is checked against the lattice and the others.
    """

    def __init__(self, lattice: Lattice, lefs: Union[Mapping[str, Sequence[Any]], Any, None] = None) -> None: ...
    @property
    def lattice(self) -> Lattice: ...
    @property
    def n_lefs(self) -> int: ...
    def to_arrow(self) -> Batch:
        """The LEF table: ``lef_id``, ``left_pos``, ``right_pos``,
        ``left_mobility``, ``right_mobility``."""
    def to_dict(self) -> Any:
        """As plain Python data, in the same shape as the JSON."""
    @classmethod
    def from_dict(cls, d: Any) -> "World": ...
    def to_json(self, *, indent: Optional[int] = None) -> str: ...
    @classmethod
    def from_json(cls, s: str) -> "World": ...

def populate(lattice: Lattice, n_lefs: int, *, seed: int = 0, rule: SpawnRuleName = "uniform_non_ctcf") -> World:
    """A world with ``n_lefs`` LEFs placed one at a time by a spawn rule,
    exactly as respawns place them. ``seed`` drives only this placement."""

class Rates:
    """Event rates per unit time. Checked (positive and finite) by ``Model``."""

    def __init__(
        self,
        step: Optional[float] = None,
        respawn: Optional[float] = None,
        respawn_captured: Optional[float] = None,
    ) -> None: ...
    @property
    def step(self) -> float:
        """Step rate of a moving, unblocked leg."""
    @property
    def respawn(self) -> float:
        """Respawn rate of a LEF with no captured leg."""
    @property
    def respawn_captured(self) -> float:
        """Respawn rate of a LEF with at least one captured leg."""
    def to_dict(self) -> Any:
        """As plain Python data, in the same shape as the JSON."""
    @classmethod
    def from_dict(cls, d: Any) -> "Rates": ...
    def to_json(self, *, indent: Optional[int] = None) -> str: ...
    @classmethod
    def from_json(cls, s: str) -> "Rates": ...

class Model:
    """A complete model: a world, its rates, and a spawn rule.

    ``to_json()`` / ``Model.from_json()`` save and load it as a versioned
    document; loading checks everything construction does.
    """

    def __init__(self, world: World, rates: Optional[Rates] = None, *, spawn: SpawnRuleName = "uniform_non_ctcf") -> None: ...
    @property
    def world(self) -> World: ...
    @property
    def lattice(self) -> Lattice: ...
    @property
    def rates(self) -> Rates: ...
    @property
    def spawn(self) -> SpawnRuleName: ...
    def to_dict(self) -> Any:
        """As plain Python data, in the same shape as the JSON."""
    @classmethod
    def from_dict(cls, d: Any) -> "Model": ...
    def to_json(self, *, indent: Optional[int] = None) -> str: ...
    @classmethod
    def from_json(cls, s: str) -> "Model": ...

class RunReport:
    """What a run did."""

    events_fired: int
    time: float
    stopped: Stopped

class Recording:
    """What a ``record_*`` call recorded, plus its run report fields."""

    snapshots: Optional[Batch]
    events: Optional[Batch]
    events_fired: int
    time: float
    stopped: Stopped

class Stream:
    """A simulation that runs lazily as it is read, one Arrow batch at a time."""

    @property
    def column_names(self) -> list[str]: ...
    @property
    def closed(self) -> bool: ...
    def __iter__(self) -> Iterator[Batch]: ...
    def __next__(self) -> Batch: ...
    def __arrow_c_schema__(self) -> object: ...
    def __arrow_c_stream__(self, requested_schema: object = None) -> object: ...

class Sim:
    """A running simulation of a ``Model``.

    ``seed`` drives the whole run; ``start_time`` sets the clock. ``run_*``
    methods advance the run; ``record_*`` methods advance it and return what
    happened as Arrow data. ``copy.copy(sim)`` forks the run.
    """

    def __init__(self, model: Model, *, seed: int = 0, start_time: float = 0.0) -> None: ...
    @property
    def time(self) -> float: ...
    @property
    def event_count(self) -> int: ...
    @property
    def next_event_time(self) -> Optional[float]: ...
    @property
    def n_sites(self) -> int: ...
    @property
    def n_lefs(self) -> int: ...
    @property
    def n_legs(self) -> int: ...
    @property
    def rates(self) -> Rates: ...
    def model(self) -> Model:
        """The current state as a ``Model``."""
    def world(self) -> World:
        """The current state as a ``World``."""
    def state(self, *, layout: Layout = "lef") -> Batch:
        """The current state as a one-snapshot Batch."""
    def __copy__(self) -> "Sim": ...
    def __deepcopy__(self, memo: Any) -> "Sim": ...
    def step(self) -> Optional[tuple[float, Literal["Step", "Respawn"], int]]:
        """Fire one event: ``(time, action, id)``, or ``None`` if none remain."""
    def run_until(self, t_end: float) -> RunReport: ...
    def run_events(self, n: int) -> RunReport: ...
    def record_until(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
    ) -> Recording: ...
    def record_events(
        self,
        n: int,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
    ) -> Recording: ...
    def record_snapshot_chunk(
        self, t_end: float, *, dt: float, t0: float = 0.0, max_snapshots: int, layout: Layout = "lef"
    ) -> Recording: ...
    def record_event_chunk(self, t_end: float, *, max_rows: int, keyframes: bool = True) -> Recording: ...
    def stream(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        keyframes: bool = False,
        layout: Layout = "lef",
        chunk_size: int = 10_000,
    ) -> Stream:
        """Move this Sim into a lazy Arrow stream (pass ``dt`` or ``events=True``)."""

class Replayer:
    """Rebuilds the state of a run at any recorded time from a keyframed
    event log, with no simulation."""

    def __init__(self, lattice: Lattice, n_lefs: int) -> None: ...
    @classmethod
    def from_arrow(
        cls,
        source: Union[Any, Iterable[Any]],
        lattice: Lattice,
        *,
        n_lefs: Optional[int] = None,
        end: Optional[float] = None,
    ) -> "Replayer": ...
    def push(self, events: Any, end: Optional[float] = None) -> None: ...
    @property
    def lattice(self) -> Lattice: ...
    @property
    def n_lefs(self) -> int: ...
    @property
    def ready(self) -> bool: ...
    @property
    def time(self) -> float: ...
    @property
    def start(self) -> Optional[float]: ...
    @property
    def end(self) -> float: ...
    def advance_to(self, t: float) -> float: ...
    def seek(self, t: float) -> float: ...
    def state(self, *, layout: Layout = "lef") -> Batch: ...
    def world(self) -> World: ...
    def world_at(self, t: float) -> World: ...
    def snapshots(
        self,
        dt: float,
        *,
        t0: float = 0.0,
        t_start: Optional[float] = None,
        t_end: Optional[float] = None,
        layout: Layout = "lef",
    ) -> Batch:
        """Decode to the grid ``t0 + k * dt`` from ``t_start`` (default: the
        current time) through ``t_end`` (default: the end of the log)."""
    def trim_before(self, t: float) -> None: ...
