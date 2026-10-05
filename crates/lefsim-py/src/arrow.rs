//! Conversions from `lefsim`'s plain columnar buffers to Arrow record batches.
//!
//! Position and id columns adopt the core crate's `Vec` allocations without
//! copying. Categorical columns (mobility, action, facing, ...) become
//! `dictionary<uint8, utf8>` arrays whose keys are the core's `code()`
//! values, so they display as names in pandas, polars and duckdb.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::types::{Float64Type, UInt8Type, UInt32Type};
use arrow_array::{ArrayRef, DictionaryArray, PrimitiveArray, RecordBatch, StringArray};
use arrow_buffer::ScalarBuffer;
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use lefsim::{Direction, EventFrames, Lattice, LefFrames, LefStates, Mobility};

/// What one row of a snapshot table is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One row per LEF per snapshot: the world's LEF table plus `time`.
    Lef,
    /// One row per leg per snapshot, in the event log's row shape.
    Leg,
}

impl Layout {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lef" => Some(Layout::Lef),
            "leg" => Some(Layout::Leg),
            _ => None,
        }
    }
}

// ----- column builders -----

/// Collect into a `Vec` allocated at exactly `len` up front. `flat_map`
/// iterators don't report an exact size, so plain `collect` would grow by
/// repeated doubling — churn that the allocator then holds on to.
fn exact<T>(len: usize, iter: impl Iterator<Item = T>) -> Vec<T> {
    let mut v = Vec::with_capacity(len);
    v.extend(iter);
    debug_assert_eq!(v.len(), len);
    v
}

fn u32_col(v: Vec<u32>) -> ArrayRef {
    Arc::new(PrimitiveArray::<UInt32Type>::new(
        ScalarBuffer::from(v),
        None,
    ))
}

fn f64_col(v: Vec<f64>) -> ArrayRef {
    Arc::new(PrimitiveArray::<Float64Type>::new(
        ScalarBuffer::from(v),
        None,
    ))
}

/// A dictionary column whose keys index into `names`.
fn dict_col(keys: Vec<u8>, names: &[&str]) -> ArrayRef {
    let keys = PrimitiveArray::<UInt8Type>::new(ScalarBuffer::from(keys), None);
    let values = Arc::new(StringArray::from(names.to_vec()));
    // Keys are core `code()` values, always in range, so this can't fail.
    Arc::new(DictionaryArray::<UInt8Type>::try_new(keys, values).expect("valid dictionary keys"))
}

fn dict_type() -> DataType {
    DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Utf8))
}

fn field(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, false)
}

fn schema(fields: Vec<Field>, kind: &str, extra: &[(&str, String)]) -> SchemaRef {
    let mut metadata = HashMap::from([("lefsim.kind".to_string(), kind.to_string())]);
    for (k, v) in extra {
        metadata.insert(format!("lefsim.{k}"), v.clone());
    }
    Arc::new(Schema::new_with_metadata(fields, metadata))
}

// ----- the two row shapes -----
//
// Every table of simulation state is the world's LEF table (one row per
// LEF) or its per-leg unpivot (one row per leg), optionally with `time`:
// `World.to_arrow()` and `Sim.state()` are LEF tables, snapshots are
// stacked LEF or leg tables, and the event log is leg rows plus `action`.

fn lef_fields(with_time: bool) -> Vec<Field> {
    let mut fields = Vec::new();
    if with_time {
        fields.push(field("time", DataType::Float64));
    }
    fields.extend([
        field("lef_id", DataType::UInt32),
        field("left_pos", DataType::UInt32),
        field("right_pos", DataType::UInt32),
        field("left_mobility", dict_type()),
        field("right_mobility", dict_type()),
    ]);
    fields
}

fn leg_fields() -> Vec<Field> {
    vec![
        field("time", DataType::Float64),
        field("leg_id", DataType::UInt32),
        field("lef_id", DataType::UInt32),
        field("pos", DataType::UInt32),
        field("mobility", dict_type()),
    ]
}

/// The schema of a LEF table (with `time` for snapshots) or of leg rows.
pub fn state_schema(layout: Layout, with_time: bool, n_lefs: usize) -> SchemaRef {
    let n = [("n_lefs", n_lefs.to_string())];
    match layout {
        Layout::Lef => schema(lef_fields(with_time), "lefs", &n),
        Layout::Leg => schema(leg_fields(), "legs", &n),
    }
}

/// Stacked LEF tables, one per entry of `time` (or a single table without
/// a `time` column), in either layout. Leg rows always carry `time`: pass
/// `None` only with `Layout::Lef`.
pub fn state_batch(
    time: Option<Vec<f64>>,
    n_lefs: usize,
    lefs: LefStates,
    layout: Layout,
) -> Result<RecordBatch, ArrowError> {
    let n_tables = time.as_ref().map_or(1, |t| t.len());
    let schema = state_schema(layout, time.is_some(), n_lefs);
    let LefStates {
        left_pos,
        right_pos,
        left_mobility,
        right_mobility,
    } = lefs;
    let columns = match layout {
        Layout::Lef => {
            let n_rows = n_tables * n_lefs;
            let mut columns = Vec::with_capacity(6);
            if let Some(time) = time {
                columns.push(f64_col(exact(
                    n_rows,
                    time.iter().flat_map(|&t| std::iter::repeat_n(t, n_lefs)),
                )));
            }
            columns.extend([
                u32_col(exact(n_rows, (0..n_tables).flat_map(|_| 0..n_lefs as u32))),
                u32_col(left_pos),
                u32_col(right_pos),
                dict_col(Mobility::into_codes(left_mobility), &Mobility::NAMES),
                dict_col(Mobility::into_codes(right_mobility), &Mobility::NAMES),
            ]);
            columns
        }
        Layout::Leg => {
            // Rows ordered by snapshot, then leg_id = 2 * lef_id + side.
            let time = time.expect("callers pass time for leg rows");
            let n_rows = 2 * n_tables * n_lefs;
            let pos = exact(
                n_rows,
                left_pos.iter().zip(&right_pos).flat_map(|(&a, &b)| [a, b]),
            );
            let mob = exact(
                n_rows,
                left_mobility
                    .iter()
                    .zip(&right_mobility)
                    .flat_map(|(&a, &b)| [a, b]),
            );
            leg_columns(
                exact(
                    n_rows,
                    time.iter()
                        .flat_map(|&t| std::iter::repeat_n(t, 2 * n_lefs)),
                ),
                exact(n_rows, (0..n_tables).flat_map(|_| 0..2 * n_lefs as u32)),
                pos,
                mob,
            )
        }
    };
    RecordBatch::try_new(schema, columns)
}

/// Snapshot frames as a batch in `layout`.
pub fn frames_batch(frames: LefFrames, layout: Layout) -> Result<RecordBatch, ArrowError> {
    let (n_lefs, time, lefs) = frames.into_parts();
    state_batch(Some(time), n_lefs, lefs, layout)
}

/// The leg-row columns: `time, leg_id, lef_id, pos, mobility`.
fn leg_columns(
    time: Vec<f64>,
    leg_id: Vec<u32>,
    pos: Vec<u32>,
    mob: Vec<Mobility>,
) -> Vec<ArrayRef> {
    let lef_id = leg_id.iter().map(|&leg| leg / 2).collect();
    vec![
        f64_col(time),
        u32_col(leg_id),
        u32_col(lef_id),
        u32_col(pos),
        dict_col(Mobility::into_codes(mob), &Mobility::NAMES),
    ]
}

// ----- event deltas -----

pub fn events_schema() -> SchemaRef {
    let mut fields = leg_fields();
    fields.push(field("action", dict_type()));
    schema(fields, "events", &[])
}

/// The event log: leg rows plus `action`.
pub fn events_batch(frames: EventFrames) -> Result<RecordBatch, ArrowError> {
    let c = frames.into_columns();
    let mut columns = leg_columns(c.time, c.leg_id, c.pos, c.mob);
    columns.push(dict_col(c.kind, &EventFrames::KIND_NAMES));
    RecordBatch::try_new(events_schema(), columns)
}

// ----- model tables -----

/// A lattice as its CTCF table, with the lattice size in the metadata:
/// everything needed to rebuild it.
pub fn lattice_batch(lattice: &Lattice) -> Result<RecordBatch, ArrowError> {
    let ctcfs = lattice.ctcfs();
    RecordBatch::try_new(
        schema(
            vec![
                field("site", DataType::UInt32),
                field("captures_from", dict_type()),
                field("strength", DataType::Float64),
            ],
            "lattice",
            &[("n_sites", lattice.len().to_string())],
        ),
        vec![
            u32_col(ctcfs.iter().map(|c| c.site).collect()),
            dict_col(
                ctcfs.iter().map(|c| c.captures_from.code()).collect(),
                &Direction::NAMES,
            ),
            f64_col(ctcfs.iter().map(|c| c.strength).collect()),
        ],
    )
}

// ----- reading columns back -----

/// Why a column couldn't be read.
pub struct ColumnError(pub String);

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef, ColumnError> {
    batch
        .column_by_name(name)
        .ok_or_else(|| ColumnError(format!("missing column {name:?}")))
}

fn cast_to(array: &ArrayRef, to: &DataType, name: &str) -> Result<ArrayRef, ColumnError> {
    let cast =
        arrow_cast::cast(array, to).map_err(|e| ColumnError(format!("column {name:?}: {e}")))?;
    // Safe casts turn values that don't fit into nulls.
    if cast.null_count() > array.null_count() || array.null_count() > 0 {
        return Err(ColumnError(format!(
            "column {name:?} has missing or out-of-range values"
        )));
    }
    Ok(cast)
}

/// An integer column as `u32` (e.g. positions and ids).
pub fn u32_values(batch: &RecordBatch, name: &str) -> Result<Vec<u32>, ColumnError> {
    let array = column(batch, name)?;
    if !array.data_type().is_integer() {
        return Err(ColumnError(format!(
            "column {name:?} must hold integers, got {}",
            array.data_type()
        )));
    }
    let cast = cast_to(array, &DataType::UInt32, name)?;
    Ok(cast
        .as_any()
        .downcast_ref::<PrimitiveArray<UInt32Type>>()
        .expect("cast")
        .values()
        .to_vec())
}

/// A float column as `f64`.
pub fn f64_values(batch: &RecordBatch, name: &str) -> Result<Vec<f64>, ColumnError> {
    let cast = cast_to(column(batch, name)?, &DataType::Float64, name)?;
    Ok(cast
        .as_any()
        .downcast_ref::<PrimitiveArray<Float64Type>>()
        .expect("cast")
        .values()
        .to_vec())
}

/// A categorical column as codes into `names`: either names (strings or a
/// dictionary of strings, matched case-insensitively) or the codes
/// themselves (integers).
pub fn code_values(
    batch: &RecordBatch,
    name: &str,
    names: &[&str],
) -> Result<Vec<u8>, ColumnError> {
    let array = column(batch, name)?;
    if array.data_type().is_integer() {
        let codes = cast_to(array, &DataType::UInt8, name)?;
        let codes = codes
            .as_any()
            .downcast_ref::<PrimitiveArray<UInt8Type>>()
            .expect("cast");
        if let Some(&bad) = codes.values().iter().find(|&&c| c as usize >= names.len()) {
            return Err(ColumnError(format!("column {name:?}: invalid code {bad}")));
        }
        return Ok(codes.values().to_vec());
    }
    let strings = cast_to(array, &DataType::Utf8, name)?;
    let strings = strings
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("cast");
    strings
        .iter()
        .map(|s| {
            let s = s.expect("no nulls");
            names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(s))
                .map(|i| i as u8)
                .ok_or_else(|| {
                    ColumnError(format!(
                        "column {name:?}: unknown value {s:?} (expected one of {names:?})"
                    ))
                })
        })
        .collect()
}
