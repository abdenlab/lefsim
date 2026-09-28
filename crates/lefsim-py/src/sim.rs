//! `lefsim.Sim`: the Python face of [`lefsim::Sim`].

use lefsim::{Action, EventRecorder, Observer, Recorder, RunReport, SampleMode, StopReason, Time};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::arrow::{Layout, events_batch, snapshots_batch};
use crate::batch::PyBatch;
use crate::model::{PyModel, PyRates, PyWorld, arrow_error, value_error};
use crate::stream::{PyStream, SimReader};

/// Events fired per call into the core between checks for Ctrl-C.
pub const EVENTS_PER_CHUNK: u64 = 250_000;

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

fn check_time(name: &str, t: f64) -> PyResult<()> {
    if t.is_nan() {
        return Err(value_error(format!("{name} is NaN")));
    }
    Ok(())
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

/// An event recorder, keyframed if asked. Validates first: the core
/// asserts (which would surface as a PanicException) on a cap too small
/// to hold a keyframe plus one event.
fn event_recorder(
    n_legs: usize,
    max_rows: Option<(usize, &str)>,
    keyframes: bool,
) -> PyResult<EventRecorder> {
    if let Some((max, name)) = max_rows {
        let min = if keyframes { n_legs + 2 } else { 2 };
        if max < min {
            let why = if keyframes { format!(" with keyframes ({n_legs} legs + 2)") } else { " for events".into() };
            return Err(value_error(format!("{name} must be at least {min}{why}")));
        }
    }
    let mut rec = EventRecorder::new();
    if let Some((max, _)) = max_rows {
        rec = rec.with_max_rows(max);
    }
    Ok(if keyframes { rec.with_keyframes(n_legs) } else { rec })
}

// ===================================================================
// Results
// ===================================================================

/// What a run did: events fired, the time reached, and why it stopped
/// ("reached_time", "reached_event_count", "exhausted", or for the chunk
/// methods "recorder_full").
#[pyclass(module = "lefsim", name = "RunReport", frozen, get_all)]
pub struct PyRunReport {
    events_fired: u64,
    time: f64,
    stopped: &'static str,
}

impl From<RunReport> for PyRunReport {
    fn from(r: RunReport) -> Self {
        PyRunReport { events_fired: r.events, time: r.time, stopped: stop_name(r.stopped) }
    }
}

#[pymethods]
impl PyRunReport {
    fn __repr__(&self) -> String {
        format!(
            "lefsim.RunReport(events_fired={}, time={:?}, stopped={:?})",
            self.events_fired, self.time, self.stopped
        )
    }
}

/// What a `record_*` call recorded, plus its run report fields.
#[pyclass(module = "lefsim", name = "Recording", frozen, get_all)]
pub struct PyRecording {
    /// Grid snapshots, if `dt` was given.
    snapshots: Option<Py<PyBatch>>,
    /// Event deltas, if `events=True`.
    events: Option<Py<PyBatch>>,
    events_fired: u64,
    /// Simulation time after the call: the recording describes the run
    /// through this time.
    time: f64,
    stopped: &'static str,
}

#[pymethods]
impl PyRecording {
    fn __repr__(&self) -> String {
        let rows = |b: &Option<Py<PyBatch>>| match b {
            Some(b) => b.get().0.num_rows().to_string(),
            None => "None".to_string(),
        };
        format!(
            "lefsim.Recording(events_fired={}, time={:?}, stopped={:?}, snapshots={}, events={})",
            self.events_fired,
            self.time,
            self.stopped,
            rows(&self.snapshots),
            rows(&self.events)
        )
    }
}

// ===================================================================
// Sim
// ===================================================================

fn consumed() -> PyErr {
    PyRuntimeError::new_err("this Sim was consumed by Sim.stream()")
}

/// A running simulation of a `Model`.
///
/// `seed` drives the whole run: the same model and seed give the same run,
/// bit for bit, on every platform. `start_time` sets the clock, e.g. to
/// continue from a state recorded at that time. The model is copied in;
/// `sim.model()` returns the current state as a new `Model`.
///
/// `run_*` methods advance the run; `record_*` methods advance it and
/// return what happened as Arrow data. `copy.copy(sim)` forks the run: the
/// copy continues exactly as the original would.
#[pyclass(module = "lefsim", name = "Sim")]
pub struct PySim {
    /// `None` once handed to a `Stream`.
    sim: Option<lefsim::Sim>,
    /// The most recent grid recorder, kept so that consecutive calls with
    /// the same grid continue it instead of repeating boundary points.
    grid: Option<Recorder>,
}

/// What to record in `PySim::record`.
struct Record {
    grid: Option<(SampleMode, Option<usize>)>,
    events: Option<(Option<(usize, &'static str)>, bool)>,
    layout: Layout,
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

    fn run(&mut self, py: Python<'_>, t_end: Time, max_events: Option<u64>) -> PyResult<PyRunReport> {
        let sim = self.sim.as_mut().ok_or_else(consumed)?;
        Ok(run_interruptibly(py, sim, t_end, max_events, &mut ())?.into())
    }

    fn record(
        &mut self,
        py: Python<'_>,
        t_end: Time,
        max_events: Option<u64>,
        what: Record,
    ) -> PyResult<PyRecording> {
        let n_legs = self.sim()?.n_legs();
        let mut deltas = match what.events {
            Some((max_rows, keyframes)) => Some(event_recorder(n_legs, max_rows, keyframes)?),
            None => None,
        };
        let mut grid = match what.grid {
            Some((mode, max)) => {
                let mut rec = self.grid_recorder(mode)?;
                rec.set_max_snapshots(max);
                Some(rec)
            }
            None => None,
        };

        let sim = self.sim.as_mut().ok_or_else(consumed)?;
        let report = match run_interruptibly(py, sim, t_end, max_events, &mut (&mut grid, &mut deltas)) {
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
                let frames = rec.take_frames().into_columns();
                Some(Py::new(py, PyBatch(snapshots_batch(frames, what.layout).map_err(arrow_error)?))?)
            }
            None => None,
        };
        if grid.is_some() {
            self.grid = grid;
        }
        let events = match deltas {
            Some(mut ev) => Some(Py::new(py, PyBatch(events_batch(ev.take_frames()).map_err(arrow_error)?))?),
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

/// Validate the shared recording options of `record_until` / `record_events`.
fn record_opts(dt: Option<f64>, t0: f64, events: bool, keyframes: bool, layout: &str) -> PyResult<Record> {
    if keyframes && !events {
        return Err(value_error("keyframes=True requires events=True"));
    }
    if dt.is_none() && !events {
        return Err(value_error("nothing to record: pass dt=... and/or events=True (or use run_*)"));
    }
    Ok(Record {
        grid: dt.map(|dt| grid_mode(dt, t0)).transpose()?.map(|m| (m, None)),
        events: events.then_some((None, keyframes)),
        layout: parse_layout(layout)?,
    })
}

#[pymethods]
impl PySim {
    #[new]
    #[pyo3(signature = (model, *, seed = 0, start_time = 0.0))]
    fn new(model: &PyModel, seed: u64, start_time: f64) -> PyResult<Self> {
        if !start_time.is_finite() {
            return Err(value_error(format!("start_time must be finite, got {start_time}")));
        }
        let sim = lefsim::Sim::with_start_time(model.0.clone(), seed, start_time);
        Ok(PySim { sim: Some(sim), grid: None })
    }

    // ----- state -----

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

    #[getter]
    fn rates(&self) -> PyResult<PyRates> {
        Ok(PyRates(self.sim()?.rates()))
    }

    /// The current state as a `Model`: the world now, with this run's rates
    /// and spawn rule. `Sim(sim.model(), seed=..., start_time=sim.time)`
    /// continues from here with fresh randomness.
    fn model(&self) -> PyResult<PyModel> {
        Ok(PyModel(self.sim()?.model().expect("Python sims use built-in spawn rules")))
    }

    /// The current state as a `World`.
    fn world(&self) -> PyResult<PyWorld> {
        Ok(PyWorld(self.sim()?.world().clone()))
    }

    /// The current state as a one-snapshot Batch, without advancing.
    #[pyo3(signature = (*, layout = "wide"))]
    fn state(&self, layout: &str) -> PyResult<PyBatch> {
        let layout = parse_layout(layout)?;
        let sim = self.sim()?;
        let mut rec = sim.recorder(SampleMode::Manual);
        rec.capture(sim);
        Ok(PyBatch(snapshots_batch(rec.take_frames().into_columns(), layout).map_err(arrow_error)?))
    }

    fn __repr__(&self) -> String {
        match &self.sim {
            Some(s) => format!(
                "lefsim.Sim(n_sites={}, n_lefs={}, time={:?}, event_count={})",
                s.n_sites(),
                s.n_lefs(),
                s.time(),
                s.event_count()
            ),
            None => "lefsim.Sim(<consumed by stream>)".to_string(),
        }
    }

    /// A fork of this run at its current time, which continues exactly as
    /// the original would.
    fn __copy__(&self) -> PyResult<Self> {
        Ok(PySim { sim: Some(self.sim()?.clone()), grid: self.grid.clone() })
    }

    fn __deepcopy__(&self, _memo: &Bound<'_, PyAny>) -> PyResult<Self> {
        self.__copy__()
    }

    // ----- running -----

    /// Fire one event. Returns `(time, action, id)` — `id` is a `leg_id`
    /// for "Step" and a `lef_id` for "Respawn" — or `None` if no events remain.
    fn step(&mut self) -> PyResult<Option<(f64, &'static str, u32)>> {
        let sim = self.sim.as_mut().ok_or_else(consumed)?;
        Ok(sim.step().map(|(t, action)| match action {
            Action::Step { leg } => (t, "Step", sim.leg_id(leg).expect("own leg")),
            Action::Respawn { lef } => (t, "Respawn", sim.lef_id(lef).expect("own lef")),
        }))
    }

    /// Fire every event at or before `t_end`, then set the clock to `t_end`.
    /// Releases the GIL while running; Ctrl-C interrupts.
    fn run_until(&mut self, py: Python<'_>, t_end: f64) -> PyResult<PyRunReport> {
        check_time("t_end", t_end)?;
        self.run(py, t_end, None)
    }

    /// Fire up to `n` events.
    fn run_events(&mut self, py: Python<'_>, n: u64) -> PyResult<PyRunReport> {
        self.run(py, f64::INFINITY, Some(n))
    }

    // ----- recording -----

    /// Run until `t_end`, recording grid snapshots (with `dt`, on the grid
    /// `t0 + k * dt`, inclusive of `t_end`) and/or event deltas (with
    /// `events=True`; add `keyframes=True` to open the event table with one
    /// `Keyframe` row per leg holding the full state). Consecutive calls on
    /// the same grid continue it seamlessly. Releases the GIL while running;
    /// Ctrl-C interrupts.
    #[pyo3(signature = (t_end, *, dt = None, t0 = 0.0, events = false, keyframes = false, layout = "wide"))]
    #[allow(clippy::too_many_arguments)]
    fn record_until(
        &mut self,
        py: Python<'_>,
        t_end: f64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
        layout: &str,
    ) -> PyResult<PyRecording> {
        check_time("t_end", t_end)?;
        let what = record_opts(dt, t0, events, keyframes, layout)?;
        self.record(py, t_end, None, what)
    }

    /// Fire up to `n` events, recording as in `record_until`; snapshots
    /// cover grid points up to the current time.
    #[pyo3(signature = (n, *, dt = None, t0 = 0.0, events = false, keyframes = false, layout = "wide"))]
    #[allow(clippy::too_many_arguments)]
    fn record_events(
        &mut self,
        py: Python<'_>,
        n: u64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
        layout: &str,
    ) -> PyResult<PyRecording> {
        let what = record_opts(dt, t0, events, keyframes, layout)?;
        self.record(py, f64::INFINITY, Some(n), what)
    }

    /// Run toward `t_end`, taking at most `max_snapshots` grid snapshots.
    /// `stopped` is "recorder_full" when the chunk filled first: call again
    /// for the next one. Consecutive chunks tile the grid exactly.
    #[pyo3(signature = (t_end, *, dt, t0 = 0.0, max_snapshots, layout = "wide"))]
    fn record_snapshot_chunk(
        &mut self,
        py: Python<'_>,
        t_end: f64,
        dt: f64,
        t0: f64,
        max_snapshots: usize,
        layout: &str,
    ) -> PyResult<PyRecording> {
        check_time("t_end", t_end)?;
        if max_snapshots == 0 {
            return Err(value_error("max_snapshots must be positive"));
        }
        let what = Record {
            grid: Some((grid_mode(dt, t0)?, Some(max_snapshots))),
            events: None,
            layout: parse_layout(layout)?,
        };
        self.record(py, t_end, None, what)
    }

    /// Run toward `t_end`, logging event deltas into a chunk of about
    /// `max_rows` rows. With `keyframes` (the default) the chunk opens with
    /// the full state, so it can be replayed on its own, and may exceed
    /// `max_rows` by the rows of a zero-delay chain in progress (chunks only
    /// end at settled times). `stopped` is "recorder_full" when the chunk
    /// filled first: call again for the next one. `time` is the time through
    /// which the chunk describes the run.
    #[pyo3(signature = (t_end, *, max_rows, keyframes = true))]
    fn record_event_chunk(
        &mut self,
        py: Python<'_>,
        t_end: f64,
        max_rows: usize,
        keyframes: bool,
    ) -> PyResult<PyRecording> {
        check_time("t_end", t_end)?;
        let what = Record { grid: None, events: Some((Some((max_rows, "max_rows")), keyframes)), layout: Layout::Wide };
        self.record(py, t_end, None, what)
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
    /// The Sim is moved into the stream and can't be used afterwards; copy
    /// it first (`copy.copy(sim)`) to keep going from here.
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
        check_time("t_end", t_end)?;
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
                let rec = event_recorder(self.sim()?.n_legs(), Some((chunk_size, "chunk_size")), keyframes)?;
                let sim = self.sim.take().ok_or_else(consumed)?;
                SimReader::events(sim, t_end, rec)
            }
            _ => return Err(value_error("pass exactly one of dt=... or events=True")),
        };
        self.grid = None;
        Ok(PyStream::new(reader))
    }
}
