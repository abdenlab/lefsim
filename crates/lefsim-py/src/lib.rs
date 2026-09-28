//! Python bindings for `lefsim`, built with pyo3 and maturin.
//!
//! Output crosses into Python as Arrow data through the Arrow PyCapsule
//! Interface, so this extension has no runtime Python dependencies:
//! pyarrow, polars and duckdb can each consume it directly.

mod arrow;
mod batch;
mod model;
mod replay;
mod sim;
mod stream;

use pyo3::prelude::*;

#[pymodule]
fn _lefsim(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<model::PyLattice>()?;
    m.add_class::<model::PyWorld>()?;
    m.add_class::<model::PyRates>()?;
    m.add_class::<model::PyModel>()?;
    m.add_function(wrap_pyfunction!(model::populate, m)?)?;
    m.add_class::<sim::PySim>()?;
    m.add_class::<sim::PyRunReport>()?;
    m.add_class::<sim::PyRecording>()?;
    m.add_class::<replay::PyReplayer>()?;
    m.add_class::<batch::PyBatch>()?;
    m.add_class::<stream::PyStream>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
