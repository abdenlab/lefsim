//! `lefsim.Sim`: the Python face of [`lefsim::Sim`].

use std::collections::HashSet;

use lefsim::{
    Action, Direction, EventRecorder, Lattice, Observer, Rates, Recorder, RunReport, SampleMode,
    Site, StopReason, Time,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::arrow::{Layout, events_batch, meta_batches, snapshots_batch};
use crate::batch::PyBatch;
use crate::stream::{PyStream, SimReader};

/// Events fired per call into the core between checks for Ctrl-C.
pub const EVENTS_PER_CHUNK: u64 = 250_000;

fn value_error(msg: impl Into<String>) -> PyErr {
    PyValueError::new_err(msg.into())
}

fn arrow_error(e: arrow_schema::ArrowError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

pub fn parse_layout(layout: &str) -> PyResult<Layout> {
    Layout::parse(layout)
        .ok_or_else(|| value_error(format!("layout must be 'wide' or 'long', got {layout:?}")))
}

pub fn grid_mode(dt: f64, t0: f64) -> PyResult<SampleMode> {
    if !(dt.is_finite() && dt > 0.0) {
        return Err(value_error(format!("dt must be positive and finite, got {dt}")));
    }
    if !t0.is_finite() {
        return Err(value_error(format!("t0 must be finite, got {t0}")));
    }
    Ok(SampleMode::EveryDt { dt, t0 })
}

pub fn stop_name(stop: StopReason) -> &'static str {
    match stop {
        StopReason::ReachedTime => "reached_time",
        StopReason::ReachedEventCount => "reached_event_count",
        StopReason::Exhausted => "exhausted",
        StopReason::RecorderFull => "recorder_full",
    }
}

fn parse_direction(s: &str) -> PyResult<Direction> {
    match s.to_ascii_lowercase().as_str() {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        _ => Err(value_error(format!(
            "captures_from must be 'left' or 'right', got {s:?}"
        ))),
    }
}

/// A scalar broadcast to `n`, or a sequence of length `n`.
fn broadcast<'py, T>(obj: &Bound<'py, PyAny>, n: usize, name: &str) -> PyResult<Vec<T>>
where
    T: Clone + for<'a> FromPyObject<'a, 'py>,
{
    // Try a scalar first: a str is also a sequence, but it's one value here.
    if let Ok(one) = obj.extract::<T>() {
        return Ok(vec![one; n]);
    }
    let many: Vec<T> = obj
        .extract()
        .map_err(|_| value_error(format!("{name} must be a scalar or a sequence")))?;
    if many.len() != n {
        return Err(value_error(format!(
            "{name} has {} entries but ctcf_sites has {n}",
            many.len()
        )));
    }
    Ok(many)
}

fn positive_rate(name: &str, value: Option<f64>, default: f64) -> PyResult<f64> {
    let rate = value.unwrap_or(default);
    if !(rate.is_finite() && rate > 0.0) {
        return Err(value_error(format!("{name} must be positive and finite, got {rate}")));
    }
    Ok(rate)
}

/// Run `sim` to `t_end` (or for `max_events`) in chunks, releasing the GIL
/// during each chunk and checking for Ctrl-C between them.
pub fn run_interruptibly<O: Observer + Send>(
    py: Python<'_>,
    sim: &mut lefsim::Sim,
    t_end: Time,
    max_events: Option<u64>,
    obs: &mut O,
) -> PyResult<RunReport> {
    let start_events = sim.event_count();
    loop {
        let fired = sim.event_count() - start_events;
        let report = match max_events {
            Some(n) => {
                let budget = (n - fired).min(EVENTS_PER_CHUNK);
                py.detach(|| sim.run_events_with(budget, obs))
            }
            None => py.detach(|| sim.run_until_with_limit(t_end, EVENTS_PER_CHUNK, obs)),
        };
        py.check_signals()?;
        let fired = sim.event_count() - start_events;
        let budget_left = max_events.is_none_or(|n| fired < n);
        if report.stopped != StopReason::ReachedEventCount || !budget_left {
            return Ok(RunReport { events: fired, ..report });
        }
    }
}

/// Recording options shared by `run_until` and `run_events`.
struct RecordOpts<'a> {
    dt: Option<f64>,
    t0: f64,
    events: bool,
    keyframes: bool,
    layout: &'a str,
}

/// An event recorder, keyframed if asked. Validates first: the core
/// asserts (which would surface as a PanicException) on a cap too small
/// to hold a keyframe plus one event.
fn event_recorder(n_legs: usize, max_rows: Option<usize>, keyframes: bool) -> PyResult<EventRecorder> {
    if let Some(max) = max_rows {
        let min = if keyframes { n_legs + 2 } else { 2 };
        if max < min {
            let why = if keyframes { format!(" with keyframes ({n_legs} legs + 2)") } else { " for events".into() };
            return Err(value_error(format!("chunk_size must be at least {min}{why}")));
        }
    }
    let mut rec = EventRecorder::new();
    if let Some(max) = max_rows {
        rec = rec.with_max_rows(max);
    }
    Ok(if keyframes { rec.with_keyframes(n_legs) } else { rec })
}

/// The outcome of a `run_until` / `run_events` call.
#[pyclass(module = "lefsim", name = "Recording", frozen, get_all)]
pub struct PyRecording {
    /// Grid snapshots, if `dt` was given.
    snapshots: Option<Py<PyBatch>>,
    /// Event deltas, if `events=True`.
    events: Option<Py<PyBatch>>,
    /// Number of events fired by this call.
    events_fired: u64,
    /// Simulation time after this call.
    time: f64,
    /// Why the call returned: "reached_time", "reached_event_count" or
    /// "exhausted".
    stopped: &'static str,
}

#[pymethods]
impl PyRecording {
    fn __repr__(&self) -> String {
        let has = |b: &Option<Py<PyBatch>>| if b.is_some() { "yes" } else { "no" };
        format!(
            "lefsim.Recording(events_fired={}, time={}, stopped={:?}, snapshots={}, events={})",
            self.events_fired,
            self.time,
            self.stopped,
            has(&self.snapshots),
            has(&self.events)
        )
    }
}

/// A loop-extrusion simulation on a 1D lattice.
///
/// LEFs are placed at construction: first one at each of `lef_sites`, then
/// `n_lefs` more at random non-CTCF sites. They are numbered in that order
/// (`lef_id`), and legs are numbered `leg_id = 2 * lef_id + side`, where side
/// 0 is the left-facing leg and side 1 the right-facing one.
#[pyclass(module = "lefsim", name = "Sim")]
pub struct PySim {
    /// `None` once handed to a `Stream`.
    sim: Option<lefsim::Sim>,
    /// The most recent grid recorder, kept so that consecutive calls with
    /// the same grid continue it instead of repeating boundary points.
    grid: Option<Recorder>,
}

impl PySim {
    fn sim(&self) -> PyResult<&lefsim::Sim> {
        self.sim.as_ref().ok_or_else(consumed)
    }

    /// A recorder for `mode`: the kept one if it's on the same grid.
    fn grid_recorder(&mut self, mode: SampleMode) -> PyResult<Recorder> {
        match self.grid.take() {
            Some(rec) if rec.mode() == mode => Ok(rec),
            _ => Ok(self.sim()?.recorder(mode)),
        }
    }

    fn record(
        &mut self,
        py: Python<'_>,
        t_end: Time,
        max_events: Option<u64>,
        opts: RecordOpts<'_>,
    ) -> PyResult<PyRecording> {
        let layout = parse_layout(opts.layout)?;
        if opts.keyframes && !opts.events {
            return Err(value_error("keyframes=True requires events=True"));
        }
        let n_legs = self.sim()?.n_legs();
        let mut deltas = match opts.events {
            true => Some(event_recorder(n_legs, None, opts.keyframes)?),
            false => None,
        };
        let mut grid = match opts.dt.map(|dt| grid_mode(dt, opts.t0)).transpose()? {
            Some(mode) => Some(self.grid_recorder(mode)?),
            None => None,
        };

        let sim = self.sim.as_mut().ok_or_else(consumed)?;
        let report = run_interruptibly(py, sim, t_end, max_events, &mut (&mut grid, &mut deltas));
        let report = match report {
            Ok(r) => r,
            Err(e) => {
                // Interrupted: drop the partial recording; a later call on
                // the same grid resumes from the current time.
                self.grid = None;
                return Err(e);
            }
        };

        let snapshots = match grid.as_mut() {
            Some(rec) => {
                let batch = snapshots_batch(rec.take_frames(), layout).map_err(arrow_error)?;
                Some(Py::new(py, PyBatch(batch))?)
            }
            None => None,
        };
        if grid.is_some() {
            self.grid = grid;
        }
        let events = match deltas {
            Some(mut ev) => {
                let batch = events_batch(ev.take_frames()).map_err(arrow_error)?;
                Some(Py::new(py, PyBatch(batch))?)
            }
            None => None,
        };
        Ok(PyRecording {
            snapshots,
            events,
            events_fired: report.events,
            time: report.time,
            stopped: stop_name(report.stopped),
        })
    }
}

fn consumed() -> PyErr {
    PyRuntimeError::new_err("this Sim was consumed by Sim.stream()")
}

#[pymethods]
impl PySim {
    #[new]
    #[pyo3(signature = (
        n_sites,
        *,
        ctcf_sites = Vec::new(),
        ctcf_captures_from = None,
        ctcf_strength = None,
        lef_sites = Vec::new(),
        n_lefs = 0,
        seed = 0,
        step_rate = None,
        respawn_rate = None,
        respawn_rate_captured = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        n_sites: u32,
        ctcf_sites: Vec<u32>,
        ctcf_captures_from: Option<&Bound<'_, PyAny>>,
        ctcf_strength: Option<&Bound<'_, PyAny>>,
        lef_sites: Vec<u32>,
        n_lefs: usize,
        seed: u64,
        step_rate: Option<f64>,
        respawn_rate: Option<f64>,
        respawn_rate_captured: Option<f64>,
    ) -> PyResult<Self> {
        let n_ctcf = ctcf_sites.len();
        let captures_from: Vec<String> = match ctcf_captures_from {
            Some(obj) => broadcast(obj, n_ctcf, "ctcf_captures_from")?,
            None if n_ctcf == 0 => Vec::new(),
            None => return Err(value_error("ctcf_captures_from is required with ctcf_sites")),
        };
        let strength: Vec<f64> = match ctcf_strength {
            Some(obj) => broadcast(obj, n_ctcf, "ctcf_strength")?,
            None => vec![1.0; n_ctcf],
        };

        let mut sites = vec![Site::plain(); n_sites as usize];
        let mut seen = HashSet::with_capacity(n_ctcf);
        for i in 0..n_ctcf {
            let site = ctcf_sites[i];
            if site >= n_sites {
                return Err(value_error(format!(
                    "CTCF site {site} is outside a lattice of {n_sites} sites"
                )));
            }
            if !seen.insert(site) {
                return Err(value_error(format!("CTCF site {site} is listed twice")));
            }
            let p = strength[i];
            if !(0.0..=1.0).contains(&p) {
                return Err(value_error(format!("CTCF strength must be in [0, 1], got {p}")));
            }
            sites[site as usize] = Site::ctcf(parse_direction(&captures_from[i])?, p);
        }

        // UniformNonCtcfSpawn rejection-samples a free non-CTCF site, so it
        // would spin forever if there were none. Each LEF covers at most two
        // sites, so this bound guarantees a free one at every respawn.
        let total_lefs = lef_sites.len() + n_lefs;
        let n_plain = n_sites as usize - n_ctcf;
        if 2 * total_lefs > n_plain {
            return Err(value_error(format!(
                "too many LEFs: {total_lefs} LEFs need at least {} non-CTCF sites, \
                 the lattice has {n_plain}",
                2 * total_lefs
            )));
        }

        let defaults = Rates::default();
        let rates = Rates {
            step: positive_rate("step_rate", step_rate, defaults.step)?,
            respawn_default: positive_rate("respawn_rate", respawn_rate, defaults.respawn_default)?,
            respawn_captured: positive_rate(
                "respawn_rate_captured",
                respawn_rate_captured,
                defaults.respawn_captured,
            )?,
        };

        let mut builder = lefsim::Sim::builder(Lattice::new(sites))
            .seed(seed)
            .rates(rates);
        for pos in lef_sites {
            builder = builder.lef_at(pos);
        }
        let sim = builder
            .lefs(n_lefs)
            .build()
            .map_err(|e| value_error(e.to_string()))?;
        Ok(PySim { sim: Some(sim), grid: None })
    }

    /// Current simulation time.
    #[getter]
    fn time(&self) -> PyResult<f64> {
        Ok(self.sim()?.time())
    }

    /// Total events fired so far.
    #[getter]
    fn event_count(&self) -> PyResult<u64> {
        Ok(self.sim()?.event_count())
    }

    /// Time of the next event, or `None` if none remain.
    #[getter]
    fn next_event_time(&self) -> PyResult<Option<f64>> {
        Ok(self.sim()?.next_event_time())
    }

    #[getter]
    fn n_sites(&self) -> PyResult<usize> {
        Ok(self.sim()?.n_sites())
    }

    #[getter]
    fn n_lefs(&self) -> PyResult<usize> {
        Ok(self.sim()?.n_lefs())
    }

    #[getter]
    fn n_legs(&self) -> PyResult<usize> {
        Ok(self.sim()?.n_legs())
    }

    /// `{"step_rate": ..., "respawn_rate": ..., "respawn_rate_captured": ...}`
    #[getter]
    fn rates<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let r = self.sim()?.rates();
        let d = PyDict::new(py);
        d.set_item("step_rate", r.step)?;
        d.set_item("respawn_rate", r.respawn_default)?;
        d.set_item("respawn_rate_captured", r.respawn_captured)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        match &self.sim {
            Some(s) => format!(
                "lefsim.Sim(n_sites={}, n_lefs={}, time={}, event_count={})",
                s.n_sites(),
                s.n_lefs(),
                s.time(),
                s.event_count()
            ),
            None => "lefsim.Sim(<consumed by stream>)".to_string(),
        }
    }

    /// Fire one event. Returns `(time, action, id)` — `id` is a `leg_id`
    /// for "Step" and a `lef_id` for "Respawn" — or `None` if no events remain.
    fn step(&mut self) -> PyResult<Option<(f64, &'static str, u32)>> {
        let sim = self.sim.as_mut().ok_or_else(consumed)?;
        Ok(sim.step().map(|(t, action)| match action {
            Action::Step { leg } => (t, "Step", sim.leg_id(leg).expect("own leg")),
            Action::Respawn { lef } => (t, "Respawn", sim.lef_id(lef).expect("own lef")),
        }))
    }

    /// Run until `t_end`, firing every event at or before it.
    ///
    /// With `dt`, record snapshots on the grid `t0 + k * dt` (inclusive of
    /// `t_end`). With `events=True`, record one row per leg changed by each
    /// event; add `keyframes=True` to open the event table with one
    /// `Keyframe` row per leg holding the full state. Consecutive calls with
    /// the same grid continue it seamlessly. Releases the GIL while running;
    /// Ctrl-C interrupts.
    #[pyo3(signature = (t_end, *, dt = None, t0 = 0.0, events = false, keyframes = false, layout = "wide"))]
    #[allow(clippy::too_many_arguments)]
    fn run_until(
        &mut self,
        py: Python<'_>,
        t_end: f64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
        layout: &str,
    ) -> PyResult<PyRecording> {
        if t_end.is_nan() {
            return Err(value_error("t_end is NaN"));
        }
        self.record(py, t_end, None, RecordOpts { dt, t0, events, keyframes, layout })
    }

    /// Fire up to `n` events. Takes the same recording options as
    /// `run_until`; snapshots cover grid points up to the current time.
    #[pyo3(signature = (n, *, dt = None, t0 = 0.0, events = false, keyframes = false, layout = "wide"))]
    #[allow(clippy::too_many_arguments)]
    fn run_events(
        &mut self,
        py: Python<'_>,
        n: u64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
        layout: &str,
    ) -> PyResult<PyRecording> {
        self.record(py, f64::INFINITY, Some(n), RecordOpts { dt, t0, events, keyframes, layout })
    }

    /// The current state as a one-snapshot batch, without advancing.
    #[pyo3(signature = (*, layout = "wide"))]
    fn snapshot(&self, layout: &str) -> PyResult<PyBatch> {
        let layout = parse_layout(layout)?;
        let sim = self.sim()?;
        let mut rec = sim.recorder(SampleMode::Manual);
        rec.capture(sim);
        Ok(PyBatch(snapshots_batch(rec.take_frames(), layout).map_err(arrow_error)?))
    }

    /// Static tables describing the run: `lattice`, `ctcfs`, `lefs` and
    /// `legs` as Batches, plus `rates` as a dict.
    fn meta<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (name, batch) in meta_batches(self.sim()?.meta()).map_err(arrow_error)? {
            d.set_item(name, PyBatch(batch))?;
        }
        d.set_item("rates", self.rates(py)?)?;
        Ok(d)
    }

    /// Hand this simulation to a lazy Arrow stream that runs it to `t_end`
    /// in chunks of `chunk_size` rows.
    ///
    /// Pass exactly one of `dt` (grid snapshots) or `events=True` (deltas).
    /// With `events=True, keyframes=True`, every batch opens with one
    /// `Keyframe` row per leg, so each batch can be replayed on its own; a
    /// batch may then exceed `chunk_size` by the rows of a zero-delay chain
    /// in progress (typically two), since batches only end at settled times.
    /// The stream can be iterated for `Batch`es, or passed to any Arrow
    /// PyCapsule consumer (e.g. `pyarrow.RecordBatchReader.from_stream`,
    /// `polars.DataFrame`, duckdb). It runs without the GIL, so it can be
    /// consumed from any thread.
    ///
    /// The Sim is moved into the stream and can't be used afterwards.
    #[pyo3(signature = (
        t_end, *, dt = None, t0 = 0.0, events = false, keyframes = false, layout = "wide",
        chunk_size = 10_000,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn stream(
        &mut self,
        t_end: f64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
        layout: &str,
        chunk_size: usize,
    ) -> PyResult<PyStream> {
        if t_end.is_nan() {
            return Err(value_error("t_end is NaN"));
        }
        if keyframes && !events {
            return Err(value_error("keyframes=True requires events=True"));
        }
        let layout = parse_layout(layout)?;
        let reader = match (dt, events) {
            (Some(dt), false) => {
                if chunk_size == 0 {
                    return Err(value_error("chunk_size must be positive"));
                }
                let rec = self.grid_recorder(grid_mode(dt, t0)?)?;
                let sim = self.sim.take().ok_or_else(consumed)?;
                SimReader::snapshots(sim, t_end, rec.with_max_snapshots(chunk_size), layout)
            }
            (None, true) => {
                let rec = event_recorder(self.sim()?.n_legs(), Some(chunk_size), keyframes)?;
                let sim = self.sim.take().ok_or_else(consumed)?;
                SimReader::events(sim, t_end, rec)
            }
            _ => return Err(value_error("pass exactly one of dt=... or events=True")),
        };
        self.grid = None;
        Ok(PyStream::new(reader))
    }
}
