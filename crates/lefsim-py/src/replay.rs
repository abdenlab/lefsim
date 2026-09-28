//! `lefsim.Replayer`: the Python face of [`lefsim::Replayer`].

use arrow_array::RecordBatch;
use lefsim::{EventColumns, EventFrames, Mobility, SampleMode, Time};
use pyo3::prelude::*;
use pyo3::types::PyType;
use pyo3_arrow::input::AnyRecordBatch;

use crate::arrow::{code_values, f64_values, snapshots_batch, u32_values};
use crate::batch::PyBatch;
use crate::model::{PyLattice, PyWorld, arrow_error, value_error};
use crate::sim::{grid_mode, parse_layout};

fn replay_error(e: lefsim::ReplayError) -> PyErr {
    value_error(e.to_string())
}

/// The event columns of an Arrow batch (as written by `record_*`/`stream`,
/// or read back from storage: names or codes both work).
fn event_columns(batch: &RecordBatch) -> PyResult<EventColumns> {
    Ok(EventColumns {
        time: f64_values(batch, "time")?,
        leg_id: u32_values(batch, "leg_id")?,
        pos: u32_values(batch, "pos")?,
        mob: code_values(batch, "mobility", &Mobility::NAMES)?,
        kind: code_values(batch, "action", &EventFrames::KIND_NAMES)?,
    })
}

fn append(to: &mut EventColumns, from: EventColumns) {
    to.time.extend(from.time);
    to.leg_id.extend(from.leg_id);
    to.pos.extend(from.pos);
    to.mob.extend(from.mob);
    to.kind.extend(from.kind);
}

/// Split off rows `..at` of `cols`, leaving the rest.
fn split_off_front(cols: &mut EventColumns, at: usize) -> EventColumns {
    let rest = EventColumns {
        time: cols.time.split_off(at),
        leg_id: cols.leg_id.split_off(at),
        pos: cols.pos.split_off(at),
        mob: cols.mob.split_off(at),
        kind: cols.kind.split_off(at),
    };
    std::mem::replace(cols, rest)
}

fn frames(cols: EventColumns) -> EventFrames {
    EventFrames::try_from(cols).expect("columns read together have equal lengths")
}

/// Rebuilds the state of a run at any recorded time from a keyframed event
/// log (`events=True, keyframes=True`), with no simulation.
///
/// Use `Replayer.from_arrow(...)` to load a whole log, or `push()` chunks as
/// they arrive. Then `seek(t)` (any direction) or `advance_to(t)` (forward,
/// cheaper), and read `state()`, `world()` or `snapshots(dt)`. Rows before
/// the first keyframe are skipped, so a log may start partway into a run.
///
/// The state at time `t` is the state after every event at or before `t`,
/// exactly what `Sim.run_until(t)` leaves. `world_at(t)` returns it as a
/// `World`, to start a new run from.
#[pyclass(module = "lefsim", name = "Replayer")]
pub struct PyReplayer(lefsim::Replayer);

#[pymethods]
impl PyReplayer {
    #[new]
    fn new(lattice: &PyLattice, n_lefs: usize) -> Self {
        PyReplayer(lefsim::Replayer::new(lattice.0.clone(), n_lefs))
    }

    /// Load a whole keyframed event log: any Arrow table, batch or stream
    /// (pyarrow, polars, a `Stream`, `pq.ParquetFile(...).iter_batches()`)
    /// or an iterable of them. Batch boundaries don't matter: the log is
    /// re-split at its keyframes. `n_lefs` defaults to the size of the
    /// first keyframe. `end` is the time through which the log describes
    /// the run (e.g. the `t_end` it was recorded to); it defaults to the
    /// last row, since event rows alone can't say how long the state after
    /// the last event lasted.
    #[classmethod]
    #[pyo3(signature = (source, lattice, *, n_lefs = None, end = None))]
    fn from_arrow(
        _cls: &Bound<'_, PyType>,
        source: &Bound<'_, PyAny>,
        lattice: &PyLattice,
        n_lefs: Option<usize>,
        end: Option<f64>,
    ) -> PyResult<Self> {
        let mut rows = EventColumns::default();
        let mut add = |item: &Bound<'_, PyAny>| -> PyResult<()> {
            let reader = item.extract::<AnyRecordBatch>()?.into_reader()?;
            for batch in reader {
                append(&mut rows, event_columns(&batch.map_err(arrow_error)?)?);
            }
            Ok(())
        };
        if source.hasattr("__arrow_c_stream__")? || source.hasattr("__arrow_c_array__")? {
            add(source)?;
        } else {
            for item in source.try_iter()? {
                add(&item?)?;
            }
        }

        let is_keyframe = |c: &EventColumns, i: usize| c.kind[i] == EventFrames::KEYFRAME;
        let n_lefs = match n_lefs {
            Some(n) => n,
            None => {
                let first = (0..rows.time.len())
                    .find(|&i| is_keyframe(&rows, i))
                    .ok_or_else(|| value_error("no keyframe in the log: pass n_lefs, or record with keyframes=True"))?;
                let len = (first..rows.time.len())
                    .take_while(|&i| is_keyframe(&rows, i) && rows.time[i] == rows.time[first] && rows.leg_id[i] as usize == i - first)
                    .count();
                len / 2
            }
        };

        // Push one chunk per keyframe (a keyframe starts with leg 0), each
        // describing the run up to the next keyframe.
        let mut replayer = lefsim::Replayer::new(lattice.0.clone(), n_lefs);
        let starts: Vec<usize> = (0..rows.time.len())
            .filter(|&i| i > 0 && is_keyframe(&rows, i) && rows.leg_id[i] == 0)
            .collect();
        let mut taken = 0;
        for start in starts {
            let end = rows.time[start - taken];
            let chunk = split_off_front(&mut rows, start - taken);
            taken = start;
            replayer.push(frames(chunk), end).map_err(replay_error)?;
        }
        let last = rows.time.last().copied().unwrap_or(Time::NEG_INFINITY);
        if !rows.time.is_empty() {
            replayer.push(frames(rows), end.unwrap_or(last)).map_err(replay_error)?;
        }
        Ok(PyReplayer(replayer))
    }

    /// Append a chunk of event rows (whole keyframes only). `end` is the
    /// time through which the chunk describes the run (a `Recording`'s
    /// `time`); it defaults to the chunk's last row.
    #[pyo3(signature = (events, end = None))]
    fn push(&mut self, events: AnyRecordBatch, end: Option<f64>) -> PyResult<()> {
        let mut cols = EventColumns::default();
        for batch in events.into_reader()? {
            append(&mut cols, event_columns(&batch.map_err(arrow_error)?)?);
        }
        let last = cols.time.last().copied().unwrap_or(Time::NEG_INFINITY);
        self.0.push(frames(cols), end.unwrap_or(last)).map_err(replay_error)
    }

    #[getter]
    fn lattice(&self) -> PyLattice {
        PyLattice(self.0.lattice().clone())
    }

    #[getter]
    fn n_lefs(&self) -> usize {
        self.0.n_lefs()
    }

    /// Whether a keyframe has been applied, i.e. whether there's a state.
    #[getter]
    fn ready(&self) -> bool {
        self.0.ready()
    }

    /// Current replay time (`-inf` until ready).
    #[getter]
    fn time(&self) -> f64 {
        self.0.time()
    }

    /// Earliest time that can be sought to (the first keyframe), or `None`.
    #[getter]
    fn start(&self) -> Option<f64> {
        self.0.start()
    }

    /// Latest time the log describes.
    #[getter]
    fn end(&self) -> f64 {
        self.0.end()
    }

    /// Move forward to `t` (or back, by seeking), clamped to `end`. Returns
    /// the time reached.
    fn advance_to(&mut self, t: f64) -> PyResult<f64> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        Ok(self.0.advance_to(t))
    }

    /// Jump to `t` in either direction (clamped to `end`).
    fn seek(&mut self, t: f64) -> PyResult<f64> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        self.0.seek(t).map_err(replay_error)
    }

    /// The state at `time` as a one-snapshot Batch, like `Sim.state()`.
    #[pyo3(signature = (*, layout = "wide"))]
    fn state(&self, layout: &str) -> PyResult<PyBatch> {
        let layout = parse_layout(layout)?;
        let lefs = self.0.lef_states().map_err(replay_error)?;
        let codes = |m: &[Mobility]| m.iter().map(|m| m.code()).collect();
        let cols = lefsim::LefColumns {
            n_lefs: lefs.len(),
            time: vec![self.0.time()],
            left_mob: codes(&lefs.left_mobility),
            right_mob: codes(&lefs.right_mobility),
            left_pos: lefs.left_pos,
            right_pos: lefs.right_pos,
        };
        Ok(PyBatch(snapshots_batch(cols, layout).map_err(arrow_error)?))
    }

    /// The state at `time` as a `World`.
    fn world(&self) -> PyResult<PyWorld> {
        Ok(PyWorld(self.0.world().map_err(replay_error)?))
    }

    /// Seek to `t` and return the state there as a `World`.
    fn world_at(&mut self, t: f64) -> PyResult<PyWorld> {
        self.seek(t)?;
        self.world()
    }

    /// Snapshots on the grid `t0 + k * dt` from `t_start` through `t_end`
    /// (default: the end of the log): the same table
    /// `Sim.record_until(dt=...)` records. Without `t_start`, starts from
    /// the current time (or the first keyframe), so consecutive calls on the
    /// same grid continue it.
    #[pyo3(signature = (dt, *, t0 = 0.0, t_start = None, t_end = None, layout = "wide"))]
    fn snapshots(
        &mut self,
        dt: f64,
        t0: f64,
        t_start: Option<f64>,
        t_end: Option<f64>,
        layout: &str,
    ) -> PyResult<PyBatch> {
        let mode: SampleMode = grid_mode(dt, t0)?;
        let layout = parse_layout(layout)?;
        if let Some(t) = t_start {
            self.seek(t)?;
        }
        let t_end = t_end.unwrap_or(self.0.end());
        if t_end.is_nan() {
            return Err(value_error("t_end is NaN"));
        }
        let frames = self.0.snapshots(mode, t_end);
        Ok(PyBatch(snapshots_batch(frames.into_columns(), layout).map_err(arrow_error)?))
    }

    /// Drop data no longer needed to seek to `t` or later.
    fn trim_before(&mut self, t: f64) -> PyResult<()> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        self.0.trim_before(t);
        Ok(())
    }

    fn __repr__(&self) -> String {
        let start = self.0.start().map_or("None".to_string(), |t| format!("{t:?}"));
        format!(
            "lefsim.Replayer(n_lefs={}, start={start}, end={:?}, time={:?})",
            self.0.n_lefs(),
            self.0.end(),
            self.0.time()
        )
    }
}
