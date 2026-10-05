//! `lefsim.Replayer`: the Python face of [`lefsim::Replayer`].

use arrow_array::RecordBatch;
use lefsim::{EventColumns, EventFrames, LefFrames, Mobility, ReplayError, SampleMode, Time};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyType};
use pyo3_arrow::input::AnyRecordBatch;

use crate::arrow::{code_values, f64_values, frames_batch, state_batch, u32_values};
use crate::batch::PyBatch;
use crate::model::{PyLattice, PyWorld, arrow_error, float_array, int_array, value_error};
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
        mob: code_values(batch, "mobility", &Mobility::NAMES)?
            .into_iter()
            .map(|c| Mobility::from_code(c).expect("checked"))
            .collect(),
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

/// Every event row of an Arrow source (a table, batch or stream, or an
/// iterable of them), in order.
fn collect_rows(source: &Bound<'_, PyAny>) -> PyResult<EventColumns> {
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
    Ok(rows)
}

/// A replayer holding `rows`, pushed as one chunk per keyframe (a keyframe
/// starts with leg 0), each describing the run up to the next keyframe.
fn replayer_from_rows(
    mut rows: EventColumns,
    lattice: lefsim::Lattice,
    n_lefs: Option<usize>,
    end: Option<f64>,
) -> PyResult<lefsim::Replayer> {
    let is_keyframe = |c: &EventColumns, i: usize| c.kind[i] == EventFrames::KEYFRAME;
    let n_lefs = match n_lefs {
        Some(n) => n,
        None => {
            let first = (0..rows.time.len())
                .find(|&i| is_keyframe(&rows, i))
                .ok_or_else(|| {
                    value_error(
                        "no keyframe in the log: pass n_lefs, or record with keyframes=True",
                    )
                })?;
            let len = (first..rows.time.len())
                .take_while(|&i| {
                    is_keyframe(&rows, i)
                        && rows.time[i] == rows.time[first]
                        && rows.leg_id[i] as usize == i - first
                })
                .count();
            len / 2
        }
    };
    let mut replayer = lefsim::Replayer::new(lattice, n_lefs);
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
        replayer
            .push(frames(rows), end.unwrap_or(last))
            .map_err(replay_error)?;
    }
    Ok(replayer)
}

/// A footer key-value entry of a pyarrow `ParquetFile`, as text.
fn footer_entry(file: &Bound<'_, PyAny>, key: &str) -> PyResult<Option<String>> {
    let kv = file.getattr("metadata")?.getattr("metadata")?;
    if kv.is_none() {
        return Ok(None);
    }
    let value = kv.call_method1("get", (PyBytes::new(file.py(), key.as_bytes()),))?;
    if value.is_none() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&value.extract::<Vec<u8>>()?).into_owned(),
    ))
}

/// Each row group's smallest `time` (its keyframe's time, in a keyframed
/// file), from the footer statistics; `None` if any are missing.
fn row_group_starts(file: &Bound<'_, PyAny>) -> PyResult<Option<Vec<Time>>> {
    let meta = file.getattr("metadata")?;
    let n: usize = meta.getattr("num_row_groups")?.extract()?;
    let column: usize = file
        .getattr("schema_arrow")?
        .call_method1("get_field_index", ("time",))?
        .extract()?;
    let mut starts = Vec::with_capacity(n);
    for g in 0..n {
        let stats = meta
            .call_method1("row_group", (g,))?
            .call_method1("column", (column,))?
            .getattr("statistics")?;
        if stats.is_none() || !stats.getattr("has_min_max")?.extract::<bool>()? {
            return Ok(None);
        }
        // Parquet writes a zero float minimum as -0.0; `+ 0.0` makes it +0.0.
        starts.push(stats.getattr("min")?.extract::<f64>()? + 0.0);
    }
    Ok(Some(starts))
}

impl PyReplayer {
    /// Read row group `g` and push it (it must follow the window).
    fn load(&mut self, py: Python<'_>, g: usize) -> PyResult<()> {
        let Self { core, source } = self;
        let src = source.as_mut().expect("a Parquet-backed replayer");
        let table = src.file.bind(py).call_method1("read_row_group", (g,))?;
        let mut cols = EventColumns::default();
        for batch in table.extract::<AnyRecordBatch>()?.into_reader()? {
            append(&mut cols, event_columns(&batch.map_err(arrow_error)?)?);
        }
        core.push(frames(cols), src.group_end(g))
            .map_err(replay_error)?;
        if !src.loaded {
            src.first = g;
        }
        src.last = g;
        src.loaded = true;
        src.reads += 1;
        Ok(())
    }

    /// Start over from row group `g` alone: one read, however far away.
    fn reopen(&mut self, py: Python<'_>, g: usize) -> PyResult<()> {
        self.core = lefsim::Replayer::new(self.core.lattice().clone(), self.core.n_lefs());
        self.source
            .as_mut()
            .expect("a Parquet-backed replayer")
            .loaded = false;
        self.load(py, g)
    }

    /// The snapshot grid `t0 + k·dt` from `t_start` (or the current time)
    /// through `t_end` (or the end), walking row groups for a file.
    fn grid_frames(
        &mut self,
        py: Python<'_>,
        mode: SampleMode,
        t_start: Option<f64>,
        t_end: Option<f64>,
    ) -> PyResult<LefFrames> {
        if let Some(t) = t_start {
            self.seek(py, t)?;
        }
        let t_end = t_end.unwrap_or(self.end());
        if t_end.is_nan() {
            return Err(value_error("t_end is NaN"));
        }
        let Some(src) = &self.source else {
            return Ok(self.core.snapshots(mode, t_end));
        };
        if src.starts.is_empty() {
            return Ok(LefFrames::new(self.core.n_lefs()));
        }
        if !src.loaded {
            self.reopen(py, 0)?;
        }
        // Row group by row group: each piece continues the grid.
        let mut frames = LefFrames::new(self.core.n_lefs());
        loop {
            let src = self.source.as_ref().expect("a Parquet-backed replayer");
            let (last, n, group_end) = (src.last, src.starts.len(), src.group_end(src.last));
            frames.extend_from(&self.core.snapshots(mode, t_end.min(group_end)));
            if t_end <= group_end || last + 1 >= n {
                break;
            }
            self.load(py, last + 1)?;
            self.trim();
        }
        Ok(frames)
    }

    /// Drop row groups before the one holding the current time.
    fn trim(&mut self) {
        self.core.trim_before(self.core.time());
        if let (Some(src), Some(start)) = (self.source.as_mut(), self.core.start()) {
            src.first = src.group_at(start).unwrap_or(src.first);
        }
    }
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
pub struct PyReplayer {
    core: lefsim::Replayer,
    /// For `from_parquet`: the file's row groups, loaded on demand.
    source: Option<RowGroups>,
}

/// A keyframed Parquet file as a lazily loaded sequence of chunks: row
/// group `g` opens with a keyframe at `starts[g]` and describes the run up
/// to the next one (the last one up to `end`). The core replayer holds the
/// row groups `first..=last`, a contiguous window.
struct RowGroups {
    file: Py<PyAny>,
    starts: Vec<Time>,
    end: Time,
    first: usize,
    last: usize,
    loaded: bool,
    reads: usize,
}

impl RowGroups {
    fn group_end(&self, g: usize) -> Time {
        self.starts.get(g + 1).copied().unwrap_or(self.end)
    }

    /// The row group holding time `t`: the last one starting at or before it.
    fn group_at(&self, t: Time) -> Option<usize> {
        self.starts.partition_point(|&s| s <= t).checked_sub(1)
    }
}

#[pymethods]
impl PyReplayer {
    #[new]
    fn new(lattice: &PyLattice, n_lefs: usize) -> Self {
        PyReplayer {
            core: lefsim::Replayer::new(lattice.0.clone(), n_lefs),
            source: None,
        }
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
        let rows = collect_rows(source)?;
        let replayer = replayer_from_rows(rows, lattice.0.clone(), n_lefs, end)?;
        Ok(PyReplayer {
            core: replayer,
            source: None,
        })
    }

    /// Open a Parquet file written by `Stream.to_parquet` for random access:
    /// only the footer is read up front, and each `seek`, `advance_to` or
    /// `snapshots` reads just the row groups it needs (one per keyframed
    /// chunk, found from the footer's time statistics). The lattice and the
    /// end come from the file's `lefsim.model` and `lefsim.run`; pass
    /// `lattice` for a file without them. A file not written that way
    /// (e.g. by `duckdb COPY`) is read whole, as `from_arrow` would.
    /// Requires pyarrow.
    #[classmethod]
    #[pyo3(signature = (path, lattice = None))]
    fn from_parquet(
        _cls: &Bound<'_, PyType>,
        path: &Bound<'_, PyAny>,
        lattice: Option<&PyLattice>,
    ) -> PyResult<Self> {
        let py = path.py();
        let file = py
            .import("pyarrow.parquet")?
            .call_method1("ParquetFile", (path,))?;
        let model: Option<lefsim::Model> = match footer_entry(&file, "lefsim.model")? {
            Some(json) => Some(
                serde_json::from_str(&json)
                    .map_err(|e| value_error(format!("lefsim.model: {e}")))?,
            ),
            None => None,
        };
        let record_end = match footer_entry(&file, "lefsim.run")? {
            Some(json) => {
                let run: serde_json::Value = serde_json::from_str(&json)
                    .map_err(|e| value_error(format!("lefsim.run: {e}")))?;
                run.get("record_end").and_then(|v| v.as_f64())
            }
            None => None,
        };
        let lattice = match (lattice, &model) {
            (Some(l), _) => l.0.clone(),
            (None, Some(m)) => m.world().lattice().clone(),
            (None, None) => return Err(value_error("the file has no lefsim.model: pass lattice=")),
        };
        let keyframed = footer_entry(&file, "lefsim.row_groups")?.as_deref() == Some("keyframed");
        let starts = if keyframed {
            row_group_starts(&file)?
        } else {
            None
        };
        let (Some(model), Some(starts)) = (model, starts) else {
            // Not our layout: read everything and re-split at keyframes.
            let rows = collect_rows(&file.call_method0("iter_batches")?)?;
            return Ok(PyReplayer {
                core: replayer_from_rows(rows, lattice, None, record_end)?,
                source: None,
            });
        };
        let end =
            record_end.unwrap_or_else(|| starts.last().copied().unwrap_or(Time::NEG_INFINITY));
        let core = lefsim::Replayer::new(lattice, model.world().n_lefs());
        let source = RowGroups {
            file: file.unbind(),
            starts,
            end,
            first: 0,
            last: 0,
            loaded: false,
            reads: 0,
        };
        Ok(PyReplayer {
            core,
            source: Some(source),
        })
    }

    /// How many Parquet row groups `from_parquet` has read so far (0 for a
    /// replayer not backed by a file).
    #[getter]
    fn row_groups_read(&self) -> usize {
        self.source.as_ref().map_or(0, |s| s.reads)
    }

    /// Append a chunk of event rows (whole keyframes only). `end` is the
    /// time through which the chunk describes the run (a `Recording`'s
    /// `time`); it defaults to the chunk's last row.
    #[pyo3(signature = (events, end = None))]
    fn push(&mut self, events: AnyRecordBatch, end: Option<f64>) -> PyResult<()> {
        if self.source.is_some() {
            return Err(value_error(
                "a replayer opened with from_parquet reads its own data",
            ));
        }
        let mut cols = EventColumns::default();
        for batch in events.into_reader()? {
            append(&mut cols, event_columns(&batch.map_err(arrow_error)?)?);
        }
        let last = cols.time.last().copied().unwrap_or(Time::NEG_INFINITY);
        self.core
            .push(frames(cols), end.unwrap_or(last))
            .map_err(replay_error)
    }

    #[getter]
    fn lattice(&self) -> PyLattice {
        PyLattice(self.core.lattice().clone())
    }

    #[getter]
    fn n_lefs(&self) -> usize {
        self.core.n_lefs()
    }

    /// Whether a keyframe has been applied, i.e. whether there's a state.
    #[getter]
    fn ready(&self) -> bool {
        self.core.ready()
    }

    /// Current replay time (`-inf` until ready).
    #[getter]
    fn time(&self) -> f64 {
        self.core.time()
    }

    /// Earliest time that can be sought to (the first keyframe), or `None`.
    #[getter]
    fn start(&self) -> Option<f64> {
        match &self.source {
            Some(src) => src.starts.first().copied(),
            None => self.core.start(),
        }
    }

    /// Latest time the log describes.
    #[getter]
    fn end(&self) -> f64 {
        match &self.source {
            Some(src) => src.end,
            None => self.core.end(),
        }
    }

    /// Move forward to `t` (or back, by seeking), clamped to the data: it
    /// never fails. Returns the time reached.
    fn advance_to(&mut self, py: Python<'_>, t: f64) -> PyResult<f64> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        let Some(src) = &self.source else {
            return Ok(self.core.advance_to(t));
        };
        let Some(&first) = src.starts.first() else {
            return Ok(self.core.time());
        };
        if !self.core.ready() {
            return if t < first {
                Ok(self.core.time())
            } else {
                self.seek(py, t)
            };
        }
        if t < self.core.time() {
            return self.seek(py, t.max(first));
        }
        let target = t.min(src.end);
        let g = src
            .group_at(target)
            .expect("the target follows the first keyframe");
        if g > src.last + 1 {
            // A far jump: start over at the row group holding the target.
            self.reopen(py, g)?;
            return self.core.seek(target).map_err(replay_error);
        }
        for h in src.last + 1..=g {
            self.load(py, h)?;
        }
        let reached = self.core.advance_to(target);
        self.trim();
        Ok(reached)
    }

    /// Jump to `t` in either direction (clamped to `end`).
    fn seek(&mut self, py: Python<'_>, t: f64) -> PyResult<f64> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        if let Some(src) = &self.source {
            let g = src.group_at(t.min(src.end)).ok_or_else(|| {
                replay_error(ReplayError::NoKeyframe {
                    t,
                    earliest: src.starts.first().copied(),
                })
            })?;
            if !(src.loaded && src.first <= g && g <= src.last) {
                self.reopen(py, g)?;
            }
        }
        self.core.seek(t).map_err(replay_error)
    }

    /// The state at `time` as a one-snapshot Batch, like `Sim.state()`.
    #[pyo3(signature = (*, layout = "lef"))]
    fn state(&self, layout: &str) -> PyResult<PyBatch> {
        let layout = parse_layout(layout)?;
        let lefs = self.core.lef_states().map_err(replay_error)?.clone();
        let batch = state_batch(
            Some(vec![self.core.time()]),
            self.core.n_lefs(),
            lefs,
            layout,
        );
        Ok(PyBatch(batch.map_err(arrow_error)?))
    }

    /// The state at `time` as a `World`.
    fn world(&self) -> PyResult<PyWorld> {
        Ok(PyWorld(self.core.world().map_err(replay_error)?))
    }

    /// Seek to `t` and return the state there as a `World`.
    fn world_at(&mut self, py: Python<'_>, t: f64) -> PyResult<PyWorld> {
        self.seek(py, t)?;
        self.world()
    }

    /// Snapshots on the grid `t0 + k * dt` from `t_start` through `t_end`
    /// (default: the end of the log): the same table
    /// `Sim.record_until(dt=...)` records. Without `t_start`, starts from
    /// the current time (or the first keyframe), so consecutive calls on the
    /// same grid continue it.
    #[pyo3(signature = (dt, *, t0 = 0.0, t_start = None, t_end = None, layout = "lef"))]
    #[allow(clippy::too_many_arguments)]
    fn snapshots(
        &mut self,
        py: Python<'_>,
        dt: f64,
        t0: f64,
        t_start: Option<f64>,
        t_end: Option<f64>,
        layout: &str,
    ) -> PyResult<PyBatch> {
        let mode: SampleMode = grid_mode(dt, t0)?;
        let layout = parse_layout(layout)?;
        let frames = self.grid_frames(py, mode, t_start, t_end)?;
        Ok(PyBatch(frames_batch(frames, layout).map_err(arrow_error)?))
    }

    /// For each point of the grid `snapshots(dt, ...)` would take, the
    /// effective distance of every pair of `sites` (condensed, as
    /// `World.effective_distances`). Returns `(times, distances)`:
    /// `array.array("d")` of grid times, and `array.array("q")` of distances,
    /// row-major, one row of `k(k-1)/2` per time. A replayer opened with
    /// `from_parquet` reads row groups as it walks the grid.
    #[pyo3(signature = (sites, *, dt, t0 = 0.0, t_start = None, t_end = None, step_length, hop_length))]
    #[allow(clippy::too_many_arguments)]
    fn effective_distances<'py>(
        &mut self,
        py: Python<'py>,
        sites: Vec<u32>,
        dt: f64,
        t0: f64,
        t_start: Option<f64>,
        t_end: Option<f64>,
        step_length: u64,
        hop_length: u64,
    ) -> PyResult<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
        let mode: SampleMode = grid_mode(dt, t0)?;
        let n_sites = self.core.lattice().len();
        // Check the arguments before moving.
        lefsim::LoopGraph::from_legs(n_sites, &[], &[], step_length, hop_length)
            .and_then(|g| g.pairwise(&sites))
            .map_err(|e| value_error(e.to_string()))?;
        let frames = self.grid_frames(py, mode, t_start, t_end)?;
        let distances = py
            .detach(|| {
                lefsim::sim::distances_over(&frames, n_sites, &sites, step_length, hop_length)
            })
            .map_err(|e| value_error(e.to_string()))?;
        Ok((float_array(py, frames.time())?, int_array(py, &distances)?))
    }

    /// Drop data no longer needed to seek to `t` or later.
    fn trim_before(&mut self, t: f64) -> PyResult<()> {
        if t.is_nan() {
            return Err(value_error("t is NaN"));
        }
        self.core.trim_before(t);
        if let (Some(src), Some(start)) = (self.source.as_mut(), self.core.start()) {
            src.first = src.group_at(start).unwrap_or(src.first);
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        let start = self
            .start()
            .map_or("None".to_string(), |t| format!("{t:?}"));
        format!(
            "lefsim.Replayer(n_lefs={}, start={start}, end={:?}, time={:?})",
            self.core.n_lefs(),
            self.end(),
            self.core.time()
        )
    }
}
