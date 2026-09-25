from collections.abc import Iterator, Sequence
from typing import Any, Literal, Optional, Union

__version__: str

Layout = Literal["wide", "long"]

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

class Recording:
    """The outcome of ``Sim.run_until`` / ``Sim.run_events``."""

    snapshots: Optional[Batch]
    events: Optional[Batch]
    events_fired: int
    time: float
    stopped: Literal["reached_time", "reached_event_count", "exhausted"]

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
    """A loop-extrusion simulation on a 1D lattice.

    LEFs are placed at construction: one at each of ``lef_sites``, then
    ``n_lefs`` more at random non-CTCF sites, numbered in that order
    (``lef_id``). Legs are numbered ``leg_id = 2 * lef_id + side``, where side
    0 faces left and side 1 faces right.
    """

    def __init__(
        self,
        n_sites: int,
        *,
        ctcf_sites: Sequence[int] = (),
        ctcf_captures_from: Union[Literal["left", "right"], Sequence[str], None] = None,
        ctcf_strength: Union[float, Sequence[float], None] = None,
        lef_sites: Sequence[int] = (),
        n_lefs: int = 0,
        seed: int = 0,
        step_rate: Optional[float] = None,
        respawn_rate: Optional[float] = None,
        respawn_rate_captured: Optional[float] = None,
    ) -> None: ...
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
    def rates(self) -> dict[str, float]: ...
    def step(self) -> Optional[tuple[float, Literal["Step", "Respawn"], int]]:
        """Fire one event: ``(time, action, id)``, or ``None`` if none remain."""
    def run_until(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        layout: Layout = "wide",
    ) -> Recording: ...
    def run_events(
        self,
        n: int,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        layout: Layout = "wide",
    ) -> Recording: ...
    def snapshot(self, *, layout: Layout = "wide") -> Batch: ...
    def meta(self) -> dict[str, Any]:
        """``lattice``, ``ctcfs``, ``lefs``, ``legs`` Batches and a ``rates`` dict."""
    def stream(
        self,
        t_end: float,
        *,
        dt: Optional[float] = None,
        t0: float = 0.0,
        events: bool = False,
        layout: Layout = "wide",
        chunk_size: int = 10_000,
    ) -> Stream:
        """Move this Sim into a lazy Arrow stream (pass ``dt`` or ``events=True``)."""
