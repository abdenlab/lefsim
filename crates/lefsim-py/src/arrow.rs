//! Conversions from `lefsim`'s plain columnar buffers to Arrow record batches.
//!
//! Position and id columns adopt the core crate's `Vec` allocations without
//! copying. Categorical columns (mobility, action, facing, ...) become
//! `dictionary<uint8, utf8>` arrays whose keys are the core's `code()`
//! values, so they display as names in pandas, polars and duckdb.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::types::{Float64Type, UInt32Type, UInt8Type};
use arrow_array::{ArrayRef, DictionaryArray, PrimitiveArray, RecordBatch, StringArray};
use arrow_buffer::ScalarBuffer;
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use lefsim::{Direction, EventFrames, Lattice, LefColumns, LefStates, Mobility};

/// Row layout of grid snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One row per LEF per snapshot, left and right legs side by side.
    Wide,
    /// One row per leg per snapshot.
    Long,
}

impl Layout {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "wide" => Some(Layout::Wide),
            "long" => Some(Layout::Long),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Layout::Wide => "wide",
            Layout::Long => "long",
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
    Arc::new(PrimitiveArray::<UInt32Type>::new(ScalarBuffer::from(v), None))
}

fn f64_col(v: Vec<f64>) -> ArrayRef {
    Arc::new(PrimitiveArray::<Float64Type>::new(ScalarBuffer::from(v), None))
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

// ----- snapshots -----

pub fn snapshots_schema(layout: Layout, n_lefs: usize) -> SchemaRef {
    let fields = match layout {
        Layout::Wide => vec![
            field("time", DataType::Float64),
            field("lef_id", DataType::UInt32),
            field("left_pos", DataType::UInt32),
            field("right_pos", DataType::UInt32),
            field("left_mobility", dict_type()),
            field("right_mobility", dict_type()),
        ],
        Layout::Long => vec![
            field("time", DataType::Float64),
            field("leg_id", DataType::UInt32),
            field("lef_id", DataType::UInt32),
            field("facing", dict_type()),
            field("pos", DataType::UInt32),
            field("mobility", dict_type()),
        ],
    };
    schema(
        fields,
        "snapshots",
        &[
            ("layout", layout.name().to_string()),
            ("n_lefs", n_lefs.to_string()),
        ],
    )
}

pub fn snapshots_batch(c: LefColumns, layout: Layout) -> Result<RecordBatch, ArrowError> {
    let n_lefs = c.n_lefs;
    let n_snapshots = c.time.len();
    let schema = snapshots_schema(layout, n_lefs);
    let columns = match layout {
        Layout::Wide => {
            let n_rows = n_snapshots * n_lefs;
            let time = exact(n_rows, c.time.iter().flat_map(|&t| std::iter::repeat_n(t, n_lefs)));
            let lef_id = exact(n_rows, (0..n_snapshots).flat_map(|_| 0..n_lefs as u32));
            vec![
                f64_col(time),
                u32_col(lef_id),
                u32_col(c.left_pos),
                u32_col(c.right_pos),
                dict_col(c.left_mob, &Mobility::NAMES),
                dict_col(c.right_mob, &Mobility::NAMES),
            ]
        }
        Layout::Long => {
            // Rows ordered by snapshot, then leg_id = 2 * lef_id + side.
            let n_rows = 2 * c.left_pos.len();
            let time =
                exact(n_rows, c.time.iter().flat_map(|&t| std::iter::repeat_n(t, 2 * n_lefs)));
            let leg_id = exact(n_rows, (0..n_snapshots).flat_map(|_| 0..2 * n_lefs as u32));
            let lef_id = (0..n_rows as u32).map(|row| (row / 2) % n_lefs.max(1) as u32).collect();
            let facing = (0..n_rows)
                .map(|row| if row % 2 == 0 { Direction::Left } else { Direction::Right })
                .map(Direction::code)
                .collect();
            let pos = exact(n_rows, c.left_pos.iter().zip(&c.right_pos).flat_map(|(&a, &b)| [a, b]));
            let mob = exact(n_rows, c.left_mob.iter().zip(&c.right_mob).flat_map(|(&a, &b)| [a, b]));
            vec![
                f64_col(time),
                u32_col(leg_id),
                u32_col(lef_id),
                dict_col(facing, &Direction::NAMES),
                u32_col(pos),
                dict_col(mob, &Mobility::NAMES),
            ]
        }
    };
    RecordBatch::try_new(schema, columns)
}

// ----- event deltas -----

pub fn events_schema() -> SchemaRef {
    schema(
        vec![
            field("time", DataType::Float64),
            field("leg_id", DataType::UInt32),
            field("lef_id", DataType::UInt32),
            field("pos", DataType::UInt32),
            field("mobility", dict_type()),
            field("action", dict_type()),
        ],
        "events",
        &[],
    )
}

pub fn events_batch(frames: EventFrames) -> Result<RecordBatch, ArrowError> {
    let c = frames.into_columns();
    let lef_id = c.leg_id.iter().map(|&leg| leg / 2).collect();
    RecordBatch::try_new(
        events_schema(),
        vec![
            f64_col(c.time),
            u32_col(c.leg_id),
            u32_col(lef_id),
            u32_col(c.pos),
            dict_col(c.mob, &Mobility::NAMES),
            dict_col(c.kind, &EventFrames::KIND_NAMES),
        ],
    )
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
            dict_col(ctcfs.iter().map(|c| c.captures_from.code()).collect(), &Direction::NAMES),
            f64_col(ctcfs.iter().map(|c| c.strength).collect()),
        ],
    )
}

/// The LEF state table: one row per LEF, in `lef_id` order.
pub fn lefs_batch(lefs: &LefStates) -> Result<RecordBatch, ArrowError> {
    let codes = |m: &[Mobility]| m.iter().map(|m| m.code()).collect();
    RecordBatch::try_new(
        schema(
            vec![
                field("lef_id", DataType::UInt32),
                field("left_pos", DataType::UInt32),
                field("right_pos", DataType::UInt32),
                field("left_mobility", dict_type()),
                field("right_mobility", dict_type()),
            ],
            "lefs",
            &[("n_lefs", lefs.len().to_string())],
        ),
        vec![
            u32_col((0..lefs.len() as u32).collect()),
            u32_col(lefs.left_pos.clone()),
            u32_col(lefs.right_pos.clone()),
            dict_col(codes(&lefs.left_mobility), &Mobility::NAMES),
            dict_col(codes(&lefs.right_mobility), &Mobility::NAMES),
        ],
    )
}

// ----- reading columns back -----

/// Why a column couldn't be read.
pub struct ColumnError(pub String);

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef, ColumnError> {
    batch.column_by_name(name).ok_or_else(|| ColumnError(format!("missing column {name:?}")))
}

fn cast_to(array: &ArrayRef, to: &DataType, name: &str) -> Result<ArrayRef, ColumnError> {
    let cast = arrow_cast::cast(array, to)
        .map_err(|e| ColumnError(format!("column {name:?}: {e}")))?;
    // Safe casts turn values that don't fit into nulls.
    if cast.null_count() > array.null_count() || array.null_count() > 0 {
        return Err(ColumnError(format!("column {name:?} has missing or out-of-range values")));
    }
    Ok(cast)
}

/// An integer column as `u32` (e.g. positions and ids).
pub fn u32_values(batch: &RecordBatch, name: &str) -> Result<Vec<u32>, ColumnError> {
    let array = column(batch, name)?;
    if !array.data_type().is_integer() {
        return Err(ColumnError(format!("column {name:?} must hold integers, got {}", array.data_type())));
    }
    let cast = cast_to(array, &DataType::UInt32, name)?;
    Ok(cast.as_any().downcast_ref::<PrimitiveArray<UInt32Type>>().expect("cast").values().to_vec())
}

/// A float column as `f64`.
pub fn f64_values(batch: &RecordBatch, name: &str) -> Result<Vec<f64>, ColumnError> {
    let cast = cast_to(column(batch, name)?, &DataType::Float64, name)?;
    Ok(cast.as_any().downcast_ref::<PrimitiveArray<Float64Type>>().expect("cast").values().to_vec())
}

/// A categorical column as codes into `names`: either names (strings or a
/// dictionary of strings, matched case-insensitively) or the codes
/// themselves (integers).
pub fn code_values(batch: &RecordBatch, name: &str, names: &[&str]) -> Result<Vec<u8>, ColumnError> {
    let array = column(batch, name)?;
    if array.data_type().is_integer() {
        let codes = cast_to(array, &DataType::UInt8, name)?;
        let codes = codes.as_any().downcast_ref::<PrimitiveArray<UInt8Type>>().expect("cast");
        if let Some(&bad) = codes.values().iter().find(|&&c| c as usize >= names.len()) {
            return Err(ColumnError(format!("column {name:?}: invalid code {bad}")));
        }
        return Ok(codes.values().to_vec());
    }
    let strings = cast_to(array, &DataType::Utf8, name)?;
    let strings = strings.as_any().downcast_ref::<StringArray>().expect("cast");
    strings
        .iter()
        .map(|s| {
            let s = s.expect("no nulls");
            names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(s))
                .map(|i| i as u8)
                .ok_or_else(|| ColumnError(format!("column {name:?}: unknown value {s:?} (expected one of {names:?})")))
        })
        .collect()
}
