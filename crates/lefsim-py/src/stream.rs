//! `lefsim.Stream`: a lazily-run simulation exposed as an Arrow stream.
//!
//! The stream owns its `lefsim::Sim`, and advancing it never touches Python.
//! That is what lets `__arrow_c_stream__` hand it to a consumer that may
//! pull batches from another thread without the GIL.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StructArray};
use arrow_schema::{ArrowError, FieldRef, SchemaRef};
use lefsim::{EventRecorder, Recorder, StopReason, Time};
use pyo3::exceptions::{PyIOError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use pyo3_arrow::error::PyArrowResult;
use pyo3_arrow::ffi::{ArrayReader, to_schema_pycapsule, to_stream_pycapsule};

use crate::arrow::{Layout, events_batch, events_schema, frames_batch, state_schema};
use crate::batch::{PyBatch, batch_field};
use crate::sim::EVENTS_PER_CHUNK;

/// Result of advancing a [`SimReader`] by one bounded step.
pub enum Advance {
    /// The event cap was hit before a batch filled; call again.
    Pending,
    Batch(RecordBatch),
    Done,
}

/// Runs a simulation to `t_end`, yielding a batch each time the bounded
/// recorder fills (and a final partial one).
pub struct SimReader {
    sim: lefsim::Sim,
    t_end: Time,
    grid: Option<Recorder>,
    events: Option<EventRecorder>,
    layout: Layout,
    schema: SchemaRef,
    done: bool,
}

impl SimReader {
    pub fn snapshots(sim: lefsim::Sim, t_end: Time, rec: Recorder, layout: Layout) -> Self {
        let schema = state_schema(layout, true, sim.n_lefs());
        SimReader { sim, t_end, grid: Some(rec), events: None, layout, schema, done: false }
    }

    pub fn events(sim: lefsim::Sim, t_end: Time, rec: EventRecorder) -> Self {
        SimReader {
            sim,
            t_end,
            grid: None,
            events: Some(rec),
            layout: Layout::Lef,
            schema: events_schema(),
            done: false,
        }
    }

    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn take_batch(&mut self) -> Result<RecordBatch, ArrowError> {
        match (&mut self.grid, &mut self.events) {
            (Some(rec), _) => frames_batch(rec.take_frames(), self.layout),
            (None, Some(ev)) => events_batch(ev.take_frames()),
            (None, None) => unreachable!("a reader always has a recorder"),
        }
    }

    /// Run until a batch is ready, the run ends, or the per-call event cap
    /// is hit (so callers can check for interrupts between steps).
    pub fn advance(&mut self) -> Result<Advance, ArrowError> {
        if self.done {
            return Ok(Advance::Done);
        }
        let report = self.sim.run_until_with_limit(
            self.t_end,
            EVENTS_PER_CHUNK,
            &mut (&mut self.grid, &mut self.events),
        );
        match report.stopped {
            StopReason::ReachedEventCount => Ok(Advance::Pending),
            StopReason::RecorderFull => Ok(Advance::Batch(self.take_batch()?)),
            StopReason::ReachedTime | StopReason::Exhausted => {
                self.done = true;
                let batch = self.take_batch()?;
                Ok(if batch.num_rows() == 0 { Advance::Done } else { Advance::Batch(batch) })
            }
        }
    }
}

impl Iterator for SimReader {
    type Item = Result<ArrayRef, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.advance() {
                Ok(Advance::Pending) => continue,
                Ok(Advance::Batch(b)) => return Some(Ok(Arc::new(StructArray::from(b)))),
                Ok(Advance::Done) => return None,
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl ArrayReader for SimReader {
    fn field(&self) -> FieldRef {
        Arc::new(batch_field(&self.schema))
    }
}

/// A simulation that runs lazily as it is read, one Arrow batch at a time.
///
/// Iterate it for `lefsim.Batch`es, or pass it to an Arrow PyCapsule stream
/// consumer. Either way it can be read only once.
#[pyclass(module = "lefsim", name = "Stream")]
pub struct PyStream {
    reader: Option<SimReader>,
    schema: SchemaRef,
}

impl PyStream {
    pub fn new(reader: SimReader) -> Self {
        let schema = reader.schema();
        PyStream { reader: Some(reader), schema }
    }
}

#[pymethods]
impl PyStream {
    #[getter]
    fn column_names(&self) -> Vec<String> {
        self.schema.fields().iter().map(|f| f.name().clone()).collect()
    }

    /// Whether the stream has been fully read or handed off.
    #[getter]
    fn closed(&self) -> bool {
        self.reader.is_none()
    }

    fn __repr__(&self) -> String {
        let kind = self.schema.metadata().get("lefsim.kind").cloned().unwrap_or_default();
        let state = if self.reader.is_some() { "open" } else { "closed" };
        format!("lefsim.Stream({kind}, {state})")
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<PyBatch>> {
        let Some(reader) = self.reader.as_mut() else {
            return Ok(None);
        };
        loop {
            let step = py.detach(|| reader.advance());
            py.check_signals()?;
            match step.map_err(|e| PyRuntimeError::new_err(e.to_string()))? {
                Advance::Pending => continue,
                Advance::Batch(b) => return Ok(Some(PyBatch(b))),
                Advance::Done => {
                    self.reader = None;
                    return Ok(None);
                }
            }
        }
    }

    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyArrowResult<Bound<'py, PyCapsule>> {
        to_schema_pycapsule(py, self.schema.as_ref())
    }

    /// Hands the remaining stream to the consumer; this object is closed
    /// afterwards.
    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &mut self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyCapsule>>,
    ) -> PyArrowResult<Bound<'py, PyCapsule>> {
        let reader = self
            .reader
            .take()
            .ok_or_else(|| PyIOError::new_err("stream was already read"))?;
        to_stream_pycapsule(py, Box::new(reader), requested_schema)
    }
}
