//! The model classes: `Lattice`, `World`, `Rates`, `Model`, and `populate`.
//!
//! Each wraps the core type of the same name and validates through the
//! core's constructors. All are immutable, comparable, picklable and
//! (through serde) convertible to and from JSON.

use arrow_array::RecordBatch;
use lefsim::{Ctcf, Direction, LefStates, Mobility, SpawnRule};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyString, PyType};
use pyo3_arrow::input::AnyRecordBatch;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::arrow::{ColumnError, code_values, lattice_batch, lefs_batch, u32_values};
use crate::batch::PyBatch;

pub fn value_error(msg: impl Into<String>) -> PyErr {
    PyValueError::new_err(msg.into())
}

pub fn arrow_error(e: arrow_schema::ArrowError) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
}

impl From<ColumnError> for PyErr {
    fn from(e: ColumnError) -> Self {
        value_error(e.0)
    }
}

fn parse_direction(s: &str) -> PyResult<Direction> {
    match s.to_ascii_lowercase().as_str() {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        _ => Err(value_error(format!("captures_from must be 'left' or 'right', got {s:?}"))),
    }
}

fn parse_mobility(s: &str) -> PyResult<Mobility> {
    Mobility::NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(s))
        .and_then(|i| Mobility::from_code(i as u8))
        .ok_or_else(|| value_error(format!("mobility must be 'moving', 'captured' or 'evicted', got {s:?}")))
}

pub fn parse_rule(s: &str) -> PyResult<SpawnRule> {
    match s {
        "uniform_non_ctcf" => Ok(SpawnRule::UniformNonCtcf),
        _ => Err(value_error(format!("unknown spawn rule {s:?} (available: 'uniform_non_ctcf')"))),
    }
}

pub fn rule_name(rule: SpawnRule) -> &'static str {
    match rule {
        SpawnRule::UniformNonCtcf => "uniform_non_ctcf",
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
        return Err(value_error(format!("{name} has {} entries but ctcf_sites has {n}", many.len())));
    }
    Ok(many)
}

fn to_json<T: Serialize>(value: &T, indent: Option<usize>) -> PyResult<String> {
    let out = match indent {
        None => serde_json::to_string(value),
        Some(n) => {
            let indent = " ".repeat(n);
            let mut buf = Vec::new();
            let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
            let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
            value.serialize(&mut ser).map(|_| String::from_utf8(buf).expect("JSON is UTF-8"))
        }
    };
    out.map_err(|e| value_error(e.to_string()))
}

fn from_json<T: DeserializeOwned>(s: &str) -> PyResult<T> {
    serde_json::from_str(s).map_err(|e| value_error(e.to_string()))
}

/// `__reduce__` through JSON: `(cls.from_json, (json,))`.
fn reduce<'py, T: Serialize>(
    slf: &Bound<'py, PyAny>,
    value: &T,
) -> PyResult<(Bound<'py, PyAny>, (String,))> {
    Ok((slf.get_type().getattr("from_json")?, (to_json(value, None)?,)))
}

// ===================================================================
// Lattice
// ===================================================================

/// A 1D lattice of `n_sites` sites, some of them CTCFs.
///
/// `ctcf_captures_from` says which side of each CTCF captures ('left' or
/// 'right'); `ctcf_strength` is the probability, in [0, 1], that a landing
/// leg interacts with it. Both may be one value for all CTCFs.
#[pyclass(module = "lefsim", name = "Lattice", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq)]
pub struct PyLattice(pub lefsim::Lattice);

#[pymethods]
impl PyLattice {
    #[new]
    #[pyo3(signature = (n_sites, *, ctcf_sites = Vec::new(), ctcf_captures_from = None, ctcf_strength = None))]
    fn new(
        n_sites: usize,
        ctcf_sites: Vec<u32>,
        ctcf_captures_from: Option<&Bound<'_, PyAny>>,
        ctcf_strength: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let n = ctcf_sites.len();
        let sides: Vec<String> = match ctcf_captures_from {
            Some(obj) => broadcast(obj, n, "ctcf_captures_from")?,
            None if n == 0 => Vec::new(),
            None => return Err(value_error("ctcf_captures_from is required with ctcf_sites")),
        };
        let strength: Vec<f64> = match ctcf_strength {
            Some(obj) => broadcast(obj, n, "ctcf_strength")?,
            None => vec![1.0; n],
        };
        let ctcfs = (0..n)
            .map(|i| {
                Ok(Ctcf {
                    site: ctcf_sites[i],
                    captures_from: parse_direction(&sides[i])?,
                    strength: strength[i],
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        let lattice = lefsim::Lattice::with_ctcfs(n_sites, &ctcfs).map_err(|e| value_error(e.to_string()))?;
        Ok(PyLattice(lattice))
    }

    #[getter]
    fn n_sites(&self) -> usize {
        self.0.len()
    }

    /// Number of non-CTCF sites.
    #[getter]
    fn n_plain(&self) -> usize {
        self.0.n_plain()
    }

    /// The CTCF table (`site`, `captures_from`, `strength`), with
    /// `lefsim.n_sites` in the schema metadata.
    fn to_arrow(&self) -> PyResult<PyBatch> {
        Ok(PyBatch(lattice_batch(&self.0).map_err(arrow_error)?))
    }

    #[pyo3(signature = (*, indent = None))]
    fn to_json(&self, indent: Option<usize>) -> PyResult<String> {
        to_json(&self.0, indent)
    }

    #[classmethod]
    fn from_json(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        Ok(PyLattice(from_json(s)?))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<(Bound<'py, PyAny>, (String,))> {
        reduce(slf.as_any(), &slf.get().0)
    }

    fn __repr__(&self) -> String {
        format!("lefsim.Lattice(n_sites={}, n_ctcfs={})", self.0.len(), self.0.len() - self.0.n_plain())
    }
}

// ===================================================================
// World
// ===================================================================

/// The LEF columns, read from an Arrow table or batch.
fn lefs_from_arrow(source: AnyRecordBatch) -> PyResult<LefStates> {
    let reader = source.into_reader()?;
    let mut lefs = LefStates::new();
    for batch in reader {
        let batch: RecordBatch = batch.map_err(arrow_error)?;
        let mobility = |name: &str| -> PyResult<Vec<Mobility>> {
            if batch.column_by_name(name).is_none() {
                return Ok(vec![Mobility::Moving; batch.num_rows()]);
            }
            Ok(code_values(&batch, name, &Mobility::NAMES)?
                .into_iter()
                .map(|c| Mobility::from_code(c).expect("checked"))
                .collect())
        };
        lefs.left_pos.extend(u32_values(&batch, "left_pos")?);
        lefs.right_pos.extend(u32_values(&batch, "right_pos")?);
        lefs.left_mobility.extend(mobility("left_mobility")?);
        lefs.right_mobility.extend(mobility("right_mobility")?);
    }
    Ok(lefs)
}

/// The LEF columns, read from a mapping of sequences (e.g. a dict).
fn lefs_from_mapping(obj: &Bound<'_, PyAny>) -> PyResult<LefStates> {
    let positions = |name: &str| -> PyResult<Vec<u32>> {
        let col = obj.get_item(name).map_err(|_| value_error(format!("missing column {name:?}")))?;
        col.extract().map_err(|_| value_error(format!("{name} must be a sequence of non-negative integers")))
    };
    let left_pos = positions("left_pos")?;
    let n = left_pos.len();
    let mobility = |name: &str| -> PyResult<Vec<Mobility>> {
        let Ok(col) = obj.get_item(name) else {
            return Ok(vec![Mobility::Moving; n]);
        };
        if let Ok(names) = col.extract::<Vec<String>>() {
            return names.iter().map(|s| parse_mobility(s)).collect();
        }
        let codes: Vec<u8> = col
            .extract()
            .map_err(|_| value_error(format!("{name} must be a sequence of names or codes")))?;
        codes
            .into_iter()
            .map(|c| Mobility::from_code(c).ok_or_else(|| value_error(format!("{name}: invalid code {c}"))))
            .collect()
    };
    Ok(LefStates {
        left_pos,
        right_pos: positions("right_pos")?,
        left_mobility: mobility("left_mobility")?,
        right_mobility: mobility("right_mobility")?,
    })
}

/// The LEF columns from any supported table-like object.
fn lefs_from(obj: &Bound<'_, PyAny>) -> PyResult<LefStates> {
    if obj.hasattr("__arrow_c_stream__")? || obj.hasattr("__arrow_c_array__")? {
        lefs_from_arrow(obj.extract::<AnyRecordBatch>()?)
    } else if obj.hasattr("__getitem__")? && !obj.is_instance_of::<PyString>() {
        lefs_from_mapping(obj)
    } else {
        Err(PyTypeError::new_err("lefs must be an Arrow table (e.g. pyarrow, polars) or a mapping of columns"))
    }
}

/// The state of a simulation at one moment: a lattice and the LEFs on it.
///
/// `lefs` is a table with one row per LEF (row `i` is LEF `i`): columns
/// `left_pos` and `right_pos`, and optionally `left_mobility` and
/// `right_mobility` ('moving' by default). Pass any Arrow table (pyarrow,
/// polars, `World.to_arrow()`, `Sim.state()`) or a dict of sequences.
///
/// Every LEF is checked: legs on the lattice and not crossed, no two LEFs
/// on one site, no 'evicted' leg, and a 'captured' leg only on a CTCF that
/// could have captured it (a left-facing leg by a CTCF that captures from
/// the right, a right-facing leg by one that captures from the left).
#[pyclass(module = "lefsim", name = "World", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq)]
pub struct PyWorld(pub lefsim::World);

#[pymethods]
impl PyWorld {
    #[new]
    #[pyo3(signature = (lattice, lefs = None))]
    fn new(lattice: &PyLattice, lefs: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let states = match lefs {
            Some(obj) => lefs_from(obj)?,
            None => LefStates::new(),
        };
        let world = lefsim::World::from_lef_states(lattice.0.clone(), &states).map_err(|e| value_error(e.to_string()))?;
        Ok(PyWorld(world))
    }

    #[getter]
    fn lattice(&self) -> PyLattice {
        PyLattice(self.0.lattice().clone())
    }

    #[getter]
    fn n_lefs(&self) -> usize {
        self.0.n_lefs()
    }

    /// The LEF table: `lef_id`, `left_pos`, `right_pos`, `left_mobility`,
    /// `right_mobility`.
    fn to_arrow(&self) -> PyResult<PyBatch> {
        Ok(PyBatch(lefs_batch(&self.0.lef_states()).map_err(arrow_error)?))
    }

    #[pyo3(signature = (*, indent = None))]
    fn to_json(&self, indent: Option<usize>) -> PyResult<String> {
        to_json(&self.0, indent)
    }

    #[classmethod]
    fn from_json(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        Ok(PyWorld(from_json(s)?))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<(Bound<'py, PyAny>, (String,))> {
        reduce(slf.as_any(), &slf.get().0)
    }

    fn __repr__(&self) -> String {
        format!("lefsim.World(n_sites={}, n_lefs={})", self.0.lattice().len(), self.0.n_lefs())
    }
}

/// A world with `n_lefs` LEFs placed one at a time by a spawn rule, exactly
/// as respawns place them. `seed` drives only this placement.
#[pyfunction]
#[pyo3(signature = (lattice, n_lefs, *, seed = 0, rule = "uniform_non_ctcf"))]
pub fn populate(lattice: &PyLattice, n_lefs: usize, seed: u64, rule: &str) -> PyResult<PyWorld> {
    let rule = parse_rule(rule)?;
    let world = lefsim::populate(lattice.0.clone(), n_lefs, &rule, seed).map_err(|e| value_error(e.to_string()))?;
    Ok(PyWorld(world))
}

// ===================================================================
// Rates
// ===================================================================

/// Event rates (per unit time): `step` for a moving, unblocked leg;
/// `respawn` for a LEF with no captured leg; `respawn_captured` for one
/// with at least one. Checked (positive and finite) by `Model`.
#[pyclass(module = "lefsim", name = "Rates", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq)]
pub struct PyRates(pub lefsim::Rates);

#[pymethods]
impl PyRates {
    #[new]
    #[pyo3(signature = (step = None, respawn = None, respawn_captured = None))]
    fn new(step: Option<f64>, respawn: Option<f64>, respawn_captured: Option<f64>) -> Self {
        let d = lefsim::Rates::default();
        PyRates(lefsim::Rates {
            step: step.unwrap_or(d.step),
            respawn: respawn.unwrap_or(d.respawn),
            respawn_captured: respawn_captured.unwrap_or(d.respawn_captured),
        })
    }

    #[getter]
    fn step(&self) -> f64 {
        self.0.step
    }

    #[getter]
    fn respawn(&self) -> f64 {
        self.0.respawn
    }

    #[getter]
    fn respawn_captured(&self) -> f64 {
        self.0.respawn_captured
    }

    #[pyo3(signature = (*, indent = None))]
    fn to_json(&self, indent: Option<usize>) -> PyResult<String> {
        to_json(&self.0, indent)
    }

    #[classmethod]
    fn from_json(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        Ok(PyRates(from_json(s)?))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<(Bound<'py, PyAny>, (String,))> {
        reduce(slf.as_any(), &slf.get().0)
    }

    fn __repr__(&self) -> String {
        let r = &self.0;
        format!("lefsim.Rates(step={:?}, respawn={:?}, respawn_captured={:?})", r.step, r.respawn, r.respawn_captured)
    }
}

// ===================================================================
// Model
// ===================================================================

/// A complete model: a world, its rates, and a spawn rule. Everything a run
/// needs except a seed.
///
/// Checks that the rates are positive and finite, and that the spawn rule
/// can always place the world's LEFs. `to_json()` and `Model.from_json()`
/// save and load it as a versioned document.
#[pyclass(module = "lefsim", name = "Model", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq)]
pub struct PyModel(pub lefsim::Model);

#[pymethods]
impl PyModel {
    #[new]
    #[pyo3(signature = (world, rates = None, *, spawn = "uniform_non_ctcf"))]
    fn new(world: &PyWorld, rates: Option<&PyRates>, spawn: &str) -> PyResult<Self> {
        let rates = rates.map_or_else(lefsim::Rates::default, |r| r.0);
        let model = lefsim::Model::new(world.0.clone(), rates, parse_rule(spawn)?)
            .map_err(|e| value_error(e.to_string()))?;
        Ok(PyModel(model))
    }

    #[getter]
    fn world(&self) -> PyWorld {
        PyWorld(self.0.world().clone())
    }

    #[getter]
    fn lattice(&self) -> PyLattice {
        PyLattice(self.0.world().lattice().clone())
    }

    #[getter]
    fn rates(&self) -> PyRates {
        PyRates(self.0.rates())
    }

    /// The spawn rule's name.
    #[getter]
    fn spawn(&self) -> &'static str {
        rule_name(self.0.spawn())
    }

    #[pyo3(signature = (*, indent = None))]
    fn to_json(&self, indent: Option<usize>) -> PyResult<String> {
        to_json(&self.0, indent)
    }

    #[classmethod]
    fn from_json(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        Ok(PyModel(from_json(s)?))
    }

    fn __reduce__<'py>(slf: &Bound<'py, Self>) -> PyResult<(Bound<'py, PyAny>, (String,))> {
        reduce(slf.as_any(), &slf.get().0)
    }

    fn __repr__(&self) -> String {
        let w = self.0.world();
        format!(
            "lefsim.Model(n_sites={}, n_lefs={}, {}, spawn={:?})",
            w.lattice().len(),
            w.n_lefs(),
            PyRates(self.0.rates()).__repr__().trim_start_matches("lefsim."),
            rule_name(self.0.spawn())
        )
    }
}
