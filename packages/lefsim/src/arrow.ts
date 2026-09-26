/**
 * Arrow views of lefsim data, using Arrow JS (`apache-arrow`).
 *
 * A separate entry point (`lefsim/arrow`) so that code that only animates
 * live state never bundles Arrow. Record batches wrap the typed arrays from
 * the simulator without copying, except for columns that must be tiled or
 * interleaved (e.g. `time` and `lef_id` in the wide snapshot layout).
 *
 * Schemas match the Python binding exactly (names, types, dictionary values
 * and schema metadata), so data can move between the two, e.g. as Arrow IPC.
 */

import {
  Data,
  Dictionary,
  Field,
  Float64,
  makeData,
  RecordBatch,
  Schema,
  Struct,
  Uint32,
  Uint8,
  Utf8,
  vectorFromArray,
  type DataType,
} from "apache-arrow";

import {
  DIRECTION,
  MOBILITY,
  ROW_KIND,
  SITE_KIND,
  type DeltaChunk,
  type Meta,
  type SnapshotColumns,
} from "./types.ts";

export type Layout = "wide" | "long";

// ------------------------------------------------------------------ columns

const f64 = (values: Float64Array) => makeData({ type: new Float64(), data: values });
const u32 = (values: Uint32Array) => makeData({ type: new Uint32(), data: values });

/** A `dictionary<uint8, utf8>` column whose keys index into `names`. */
function dict(keys: Uint8Array, names: readonly string[]): Data<Dictionary<Utf8, Uint8>> {
  // A fresh Dictionary type per column gives each its own dictionary id.
  const type = new Dictionary(new Utf8(), new Uint8());
  return makeData({ type, data: keys, dictionary: vectorFromArray([...names], new Utf8()) });
}

function batch(columns: [string, Data][], kind: string, extra: Record<string, string> = {}): RecordBatch {
  const fields = columns.map(([name, data]) => new Field(name, data.type as DataType, false));
  const metadata = new Map([["lefsim.kind", kind], ...Object.entries(extra).map(([k, v]) => [`lefsim.${k}`, v] as [string, string])]);
  const schema = new Schema(fields, metadata);
  const length = columns[0]?.[1].length ?? 0;
  const data = makeData({ type: new Struct(fields), length, nullCount: 0, children: columns.map(([, d]) => d) });
  return new RecordBatch(schema, data);
}

// ------------------------------------------------------------------ builders

/**
 * Grid snapshots as a record batch.
 *
 * - `"wide"` (default): one row per LEF per snapshot; columns `time, lef_id,
 *   left_pos, right_pos, left_mobility, right_mobility`.
 * - `"long"`: one row per leg per snapshot; columns `time, leg_id, lef_id,
 *   facing, pos, mobility`.
 */
export function snapshotsToArrow(cols: SnapshotColumns, options: { layout?: Layout } = {}): RecordBatch {
  const layout = options.layout ?? "wide";
  const { nLefs } = cols;
  const nSnap = cols.time.length;
  const meta = { layout, n_lefs: String(nLefs) };
  if (layout === "wide") {
    const n = nSnap * nLefs;
    const time = new Float64Array(n);
    const lefId = new Uint32Array(n);
    for (let s = 0; s < nSnap; s++) {
      time.fill(cols.time[s]!, s * nLefs, (s + 1) * nLefs);
      for (let i = 0; i < nLefs; i++) lefId[s * nLefs + i] = i;
    }
    return batch(
      [
        ["time", f64(time)],
        ["lef_id", u32(lefId)],
        ["left_pos", u32(cols.leftPos)],
        ["right_pos", u32(cols.rightPos)],
        ["left_mobility", dict(cols.leftMobility, MOBILITY)],
        ["right_mobility", dict(cols.rightMobility, MOBILITY)],
      ],
      "snapshots",
      meta,
    );
  }
  if (layout !== "long") throw new RangeError(`layout must be "wide" or "long", got ${JSON.stringify(layout)}`);
  // Rows ordered by snapshot, then leg_id = 2 * lef_id + side.
  const perSnap = 2 * nLefs;
  const n = nSnap * perSnap;
  const time = new Float64Array(n);
  const legId = new Uint32Array(n);
  const lefId = new Uint32Array(n);
  const facing = new Uint8Array(n);
  const pos = new Uint32Array(n);
  const mob = new Uint8Array(n);
  for (let s = 0; s < nSnap; s++) {
    for (let r = 0; r < perSnap; r++) {
      const row = s * perSnap + r;
      const src = s * nLefs + (r >> 1);
      const right = r & 1;
      time[row] = cols.time[s]!;
      legId[row] = r;
      lefId[row] = r >> 1;
      facing[row] = right; // Direction codes: 0 = Left, 1 = Right
      pos[row] = right ? cols.rightPos[src]! : cols.leftPos[src]!;
      mob[row] = right ? cols.rightMobility[src]! : cols.leftMobility[src]!;
    }
  }
  return batch(
    [
      ["time", f64(time)],
      ["leg_id", u32(legId)],
      ["lef_id", u32(lefId)],
      ["facing", dict(facing, DIRECTION)],
      ["pos", u32(pos)],
      ["mobility", dict(mob, MOBILITY)],
    ],
    "snapshots",
    meta,
  );
}

/** Event deltas as a record batch: `time, leg_id, lef_id, pos, mobility, action`. */
export function eventsToArrow(chunk: DeltaChunk): RecordBatch {
  const lefId = new Uint32Array(chunk.legId.length);
  for (let i = 0; i < lefId.length; i++) lefId[i] = chunk.legId[i]! >>> 1;
  return batch(
    [
      ["time", f64(chunk.time)],
      ["leg_id", u32(chunk.legId)],
      ["lef_id", u32(lefId)],
      ["pos", u32(chunk.pos)],
      ["mobility", dict(chunk.mobility, MOBILITY)],
      ["action", dict(chunk.kind, ROW_KIND)],
    ],
    "events",
  );
}

/** The static tables from `Sim.meta()` as record batches. */
export function metaToArrow(meta: Meta): { lattice: RecordBatch; ctcfs: RecordBatch; lefs: RecordBatch; legs: RecordBatch } {
  const range = (n: number) => Uint32Array.from({ length: n }, (_, i) => i);
  return {
    lattice: batch([["site", u32(range(meta.nSites))], ["kind", dict(meta.lattice.kind, SITE_KIND)]], "lattice"),
    ctcfs: batch(
      [
        ["site", u32(meta.ctcfs.site)],
        ["captures_from", dict(meta.ctcfs.capturesFrom, DIRECTION)],
        ["strength", f64(meta.ctcfs.strength)],
      ],
      "ctcfs",
    ),
    lefs: batch(
      [
        ["lef_id", u32(range(meta.lefs.leftLegId.length))],
        ["left_leg_id", u32(meta.lefs.leftLegId)],
        ["right_leg_id", u32(meta.lefs.rightLegId)],
      ],
      "lefs",
    ),
    legs: batch(
      [
        ["leg_id", u32(range(meta.legs.lefId.length))],
        ["lef_id", u32(meta.legs.lefId)],
        ["facing", dict(meta.legs.facing, DIRECTION)],
      ],
      "legs",
    ),
  };
}

// ------------------------------------------------------------------ adapter

type Typed = Float64Array | Uint32Array | Uint8Array;
type TypedCtor<T extends Typed> = { new (n: number): T; from(src: ArrayLike<number>): T };

/** A column's values as `Ctor`, without copying when they already are. */
function column<T extends Typed>(b: RecordBatch, name: string, Ctor: TypedCtor<T>): T {
  const vec = b.getChild(name);
  if (!vec) throw new RangeError(`missing column "${name}"`);
  const data = vec.data[0];
  if (vec.data.length === 1 && data && data.values instanceof Ctor && !vec.nullCount) {
    return (data.values as T).subarray(data.offset, data.offset + data.length) as T;
  }
  return Ctor.from(Array.from(vec.toArray() as ArrayLike<number | bigint>, Number));
}

/** Dictionary keys mapped to our codes (zero-copy when the dictionary matches). */
function codes(b: RecordBatch, name: string, names: readonly string[]): Uint8Array {
  const vec = b.getChild(name);
  if (!vec) throw new RangeError(`missing column "${name}"`);
  const data = vec.data[0];
  const dictionary = data?.dictionary?.toArray() as string[] | undefined;
  if (vec.data.length === 1 && data && dictionary && data.values instanceof Uint8Array) {
    const keys = data.values.subarray(data.offset, data.offset + data.length);
    if (dictionary.length <= names.length && dictionary.every((v, i) => v === names[i])) return keys;
  }
  // Different encoding (e.g. re-encoded by another tool): map by name.
  const out = new Uint8Array(vec.length);
  for (let i = 0; i < vec.length; i++) {
    const code = names.indexOf(String(vec.get(i)));
    if (code < 0) throw new RangeError(`column "${name}" has unknown value ${JSON.stringify(vec.get(i))}`);
    out[i] = code;
  }
  return out;
}

/**
 * An events record batch as a {@link DeltaChunk}, e.g. to feed a
 * `Replayer` from an Arrow IPC stream. Zero-copy for batches built by lefsim
 * (either binding); other encodings are converted.
 *
 * `end` is the time through which the batch describes the state. It isn't
 * carried in the batch; it defaults to the last row's time, which is always
 * safe but may understate how far the state is known.
 */
export function deltaChunkFromArrow(b: RecordBatch, options: { end?: number } = {}): DeltaChunk {
  const time = column(b, "time", Float64Array);
  return {
    time,
    legId: column(b, "leg_id", Uint32Array),
    pos: column(b, "pos", Uint32Array),
    mobility: codes(b, "mobility", MOBILITY),
    kind: codes(b, "action", ROW_KIND),
    end: options.end ?? (time.length ? time[time.length - 1]! : -Infinity),
  };
}
