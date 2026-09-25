//! Python bindings for `lefsim`, built with pyo3 and maturin.
//!
//! Output crosses into Python as Arrow data through the Arrow PyCapsule
//! Interface, so this extension has no runtime Python dependencies:
//! pyarrow, polars and duckdb can each consume it directly.

mod arrow;
mod batch;
mod sim;
mod stream;

use pyo3::prelude::*;

#[pymodule]
fn _lefsim(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<sim::PySim>()?;
    m.add_class::<sim::PyRecording>()?;
    m.add_class::<batch::PyBatch>()?;
    m.add_class::<stream::PyStream>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
