//! `lefsim.Batch`: an Arrow record batch exposed through the Arrow PyCapsule
//! Interface, so pyarrow, polars, duckdb (and anything else that speaks the
//! protocol) can read it without copying and without this extension
//! depending on any of them.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StructArray};
use arrow_schema::Field;
use pyo3::prelude::*;
use pyo3::types::{PyCapsule, PyTuple};
use pyo3_arrow::error::PyArrowResult;
use pyo3_arrow::ffi::{
    ArrayIterator, to_array_pycapsules, to_schema_pycapsule, to_stream_pycapsule,
};

/// The struct field that represents a record batch on the C interfaces.
pub fn batch_field(batch_schema: &arrow_schema::Schema) -> Field {
    Field::new_struct("", batch_schema.fields().clone(), false)
        .with_metadata(batch_schema.metadata().clone())
}

/// An immutable Arrow record batch.
///
/// Implements `__arrow_c_array__`, `__arrow_c_stream__` and
/// `__arrow_c_schema__`, so it can be passed directly to e.g.
/// `pyarrow.table(...)`, `polars.DataFrame(...)` or queried by name from
/// duckdb.
#[pyclass(module = "lefsim", name = "Batch", frozen)]
pub struct PyBatch(pub RecordBatch);

#[pymethods]
impl PyBatch {
    /// Number of rows.
    #[getter]
    fn num_rows(&self) -> usize {
        self.0.num_rows()
    }

    /// Number of columns.
    #[getter]
    fn num_columns(&self) -> usize {
        self.0.num_columns()
    }

    /// The column names, in order.
    #[getter]
    fn column_names(&self) -> Vec<String> {
        self.0
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    /// Schema metadata, e.g. `{"lefsim.kind": "snapshots", ...}`.
    #[getter]
    fn metadata(&self) -> std::collections::HashMap<String, String> {
        self.0.schema().metadata().clone()
    }

    fn __len__(&self) -> usize {
        self.0.num_rows()
    }

    fn __repr__(&self) -> String {
        let kind = self
            .0
            .schema()
            .metadata()
            .get("lefsim.kind")
            .cloned()
            .unwrap_or_default();
        let cols: Vec<String> = self
            .0
            .schema()
            .fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect();
        format!(
            "lefsim.Batch({kind}, {} rows)\n  {}",
            self.0.num_rows(),
            cols.join("\n  ")
        )
    }

    fn __arrow_c_schema__<'py>(&'py self, py: Python<'py>) -> PyArrowResult<Bound<'py, PyCapsule>> {
        to_schema_pycapsule(py, self.0.schema_ref().as_ref())
    }

    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_array__<'py>(
        &'py self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyCapsule>>,
    ) -> PyArrowResult<Bound<'py, PyTuple>> {
        let field = Arc::new(batch_field(self.0.schema_ref()));
        let array: ArrayRef = Arc::new(StructArray::from(self.0.clone()));
        to_array_pycapsules(py, field, &array, requested_schema)
    }

    /// A stream of this single batch. Cloning a `RecordBatch` only bumps
    /// buffer refcounts.
    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &'py self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyCapsule>>,
    ) -> PyArrowResult<Bound<'py, PyCapsule>> {
        let field = Arc::new(batch_field(self.0.schema_ref()));
        let array: ArrayRef = Arc::new(StructArray::from(self.0.clone()));
        let reader = ArrayIterator::new(std::iter::once(Ok(array)), field);
        to_stream_pycapsule(py, Box::new(reader), requested_schema)
    }

    /// As a `pyarrow.Table` (requires pyarrow).
    fn to_pyarrow<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        slf.py().import("pyarrow")?.call_method1("table", (slf,))
    }

    /// As a `polars.DataFrame` (requires polars).
    fn to_polars<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        slf.py().import("polars")?.call_method1("DataFrame", (slf,))
    }

    /// As a `pandas.DataFrame` (requires pyarrow and pandas).
    fn to_pandas<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        Self::to_pyarrow(slf)?.call_method0("to_pandas")
    }
}
