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
  Table,
  Uint32,
  Uint8,
  Utf8,
  vectorFromArray,
  type DataType,
} from "apache-arrow";

import { Replayer, replayerFromRows } from "./replayer.ts";
import {
  DIRECTION,
  MOBILITY,
  ROW_KIND,
  type DeltaChunk,
  type Lattice,
  type Lefs,
  type SnapshotColumns,
} from "./types.ts";

export type Layout = "lef" | "leg";

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
 * - `"lef"` (default): one row per LEF per snapshot, the world's LEF table
 *   plus `time`: `time, lef_id, left_pos, right_pos, left_mobility,
 *   right_mobility`.
 * - `"leg"`: one row per leg per snapshot, in the event log's row shape:
 *   `time, leg_id, lef_id, pos, mobility`.
 */
export function snapshotsToArrow(cols: SnapshotColumns, options: { layout?: Layout } = {}): RecordBatch {
  const layout = options.layout ?? "lef";
  const { nLefs } = cols;
  const nSnap = cols.time.length;
  const meta = { n_lefs: String(nLefs) };
  if (layout === "lef") {
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
      "lefs",
      meta,
    );
  }
  if (layout !== "leg") throw new RangeError(`layout must be "lef" or "leg", got ${JSON.stringify(layout)}`);
  // Rows ordered by snapshot, then leg_id = 2 * lef_id + side.
  const perSnap = 2 * nLefs;
  const n = nSnap * perSnap;
  const time = new Float64Array(n);
  const legId = new Uint32Array(n);
  const lefId = new Uint32Array(n);
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
      pos[row] = right ? cols.rightPos[src]! : cols.leftPos[src]!;
      mob[row] = right ? cols.rightMobility[src]! : cols.leftMobility[src]!;
    }
  }
  return batch(
    [
      ["time", f64(time)],
      ["leg_id", u32(legId)],
      ["lef_id", u32(lefId)],
      ["pos", u32(pos)],
      ["mobility", dict(mob, MOBILITY)],
    ],
    "legs",
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

/**
 * The lattice as its CTCF table, with `lefsim.n_sites` in the schema
 * metadata. Matches the Python binding's `Lattice.to_arrow()`.
 */
export function latticeToArrow(lattice: Lattice): RecordBatch {
  return batch(
    [
      ["site", u32(lattice.ctcfs.site)],
      ["captures_from", dict(lattice.ctcfs.capturesFrom, DIRECTION)],
      ["strength", f64(lattice.ctcfs.strength)],
    ],
    "lattice",
    { n_sites: String(lattice.nSites) },
  );
}

/**
 * The LEF table, one row per LEF: a world's LEFs, or a `LiveState` (whose
 * wasm-backed arrays are copied). Matches the Python binding's
 * `World.to_arrow()`.
 */
export function lefsToArrow(lefs: Lefs): RecordBatch {
  const n = lefs.leftPos.length;
  return batch(
    [
      ["lef_id", u32(Uint32Array.from({ length: n }, (_, i) => i))],
      ["left_pos", u32(lefs.leftPos.slice())],
      ["right_pos", u32(lefs.rightPos.slice())],
      ["left_mobility", dict(lefs.leftMobility.slice(), MOBILITY)],
      ["right_mobility", dict(lefs.rightMobility.slice(), MOBILITY)],
    ],
    "lefs",
    { n_lefs: String(n) },
  );
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
    // A sliced Data's `values` already start at the slice (its `offset`
    // refers to the original buffers), so take `length` values from 0.
    return (data.values as T).subarray(0, data.length) as T;
  }
  return Ctor.from(Array.from(vec.toArray() as ArrayLike<number | bigint>, Number));
}

/**
 * A categorical column as our codes: zero-copy when its dictionary is ours;
 * otherwise names (in any case) or integer codes are mapped, e.g. after
 * another tool re-encoded the data.
 */
function codes(b: RecordBatch, name: string, names: readonly string[]): Uint8Array {
  const vec = b.getChild(name);
  if (!vec) throw new RangeError(`missing column "${name}"`);
  const data = vec.data[0];
  const dictionary = data?.dictionary?.toArray() as string[] | undefined;
  if (vec.data.length === 1 && data && dictionary && data.values instanceof Uint8Array) {
    const keys = data.values.subarray(0, data.length); // see `column` on offsets
    if (dictionary.length <= names.length && dictionary.every((v, i) => v === names[i])) return keys;
  }
  const lower = names.map((n) => n.toLowerCase());
  const out = new Uint8Array(vec.length);
  for (let i = 0; i < vec.length; i++) {
    const v: unknown = vec.get(i);
    const code = typeof v === "number" ? (Number.isInteger(v) && v >= 0 && v < names.length ? v : -1) : lower.indexOf(String(v).toLowerCase());
    if (code < 0) throw new RangeError(`column "${name}" has unknown value ${JSON.stringify(v)}`);
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

/**
 * A {@link Replayer} loaded with a whole keyframed event log: a record
 * batch, a table, or any iterable of batches (e.g. from an Arrow IPC
 * stream). Batch boundaries don't matter: the rows are re-split at their
 * keyframes. `nLefs` defaults to the size of the first keyframe. `end` is
 * the time through which the log describes the run (e.g. the `tEnd` it was
 * recorded to); it defaults to the last row. The TS counterpart of the
 * Python binding's `Replayer.from_arrow`.
 */
export function replayerFromArrow(
  source: RecordBatch | Table | Iterable<RecordBatch>,
  lattice: Lattice,
  options: { nLefs?: number; end?: number } = {},
): Replayer {
  const batches: Iterable<RecordBatch> = source instanceof RecordBatch ? [source] : source instanceof Table ? source.batches : source;
  const parts = Array.from(batches, (b) => deltaChunkFromArrow(b));
  const concat = <T extends Float64Array | Uint32Array | Uint8Array>(key: "time" | "legId" | "pos" | "mobility" | "kind", Ctor: { new (n: number): T }): T => {
    const out = new Ctor(parts.reduce((n, p) => n + p[key].length, 0));
    let at = 0;
    for (const p of parts) {
      out.set(p[key] as never, at);
      at += p[key].length;
    }
    return out;
  };
  const rows = {
    time: concat("time", Float64Array),
    legId: concat("legId", Uint32Array),
    pos: concat("pos", Uint32Array),
    mobility: concat("mobility", Uint8Array),
    kind: concat("kind", Uint8Array),
  };
  return replayerFromRows(rows, lattice, options);
}

// ------------------------------------------------------------------ streams

/**
 * A `TransformStream` from simulator chunks (as produced by `Sim.stream`)
 * to Arrow record batches: snapshot chunks become snapshot batches in the
 * given `layout`, event chunks become events batches.
 *
 * ```ts
 * const batches = sim.stream(1e6, { dt: 1 }).pipeThrough(toRecordBatches());
 * ```
 */
export function toRecordBatches(options: { layout?: Layout } = {}): TransformStream<SnapshotColumns | DeltaChunk, RecordBatch> {
  return new TransformStream({
    transform(chunk, controller) {
      controller.enqueue("legId" in chunk ? eventsToArrow(chunk) : snapshotsToArrow(chunk, options));
    },
  });
}
