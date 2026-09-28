/**
 * Replaying keyframed event logs from Parquet, with random access.
 *
 * A separate entry point (`lefsim/parquet`) so that pages which don't read
 * Parquet never bundle a reader. Uses hyparquet (pure JavaScript, reading
 * single row groups by byte range, so over HTTP a replayer fetches the
 * footer and then only the row groups it needs) and fzstd for zstd.
 *
 * Reading is asynchronous here (it may be a network fetch), so the methods
 * that move return promises; reading the state stays synchronous for drawing.
 */

import { decompress as zstdDecompress } from "fzstd";
import {
  asyncBufferFromUrl,
  cachedAsyncBuffer,
  parquetMetadataAsync,
  parquetRead,
  type AsyncBuffer,
  type Compressors,
  type FileMetaData,
} from "hyparquet";

import { Replayer, replayerFromRows, type EventRows } from "./replayer.ts";
import { DIRECTION, MOBILITY, ROW_KIND, type Lattice, type LiveState, type SnapshotColumns, type World } from "./types.ts";

/** Where a Parquet file comes from: a URL (read with range requests), a `Blob`/`File`, bytes, or a hyparquet `AsyncBuffer`. */
export type ParquetSource = string | URL | Blob | ArrayBuffer | Uint8Array | AsyncBuffer;

/** Codecs besides hyparquet's built-in snappy. */
const COMPRESSORS: Compressors = {
  ZSTD: (input, outputLength) => zstdDecompress(input, new Uint8Array(outputLength)),
};

async function asyncBuffer(source: ParquetSource): Promise<AsyncBuffer> {
  if (typeof source === "string" || source instanceof URL) {
    return cachedAsyncBuffer(await asyncBufferFromUrl({ url: String(source) }));
  }
  if (typeof Blob !== "undefined" && source instanceof Blob) {
    return { byteLength: source.size, slice: (start, end) => source.slice(start, end).arrayBuffer() };
  }
  if (source instanceof Uint8Array) {
    // Copy from the underlying buffer at the view's offset: a Node `Buffer`
    // is a view into a shared pool, so its `.buffer` is more than the file.
    const bytes = source;
    const at = bytes.byteOffset;
    return {
      byteLength: bytes.byteLength,
      slice: (start, end = bytes.byteLength) => bytes.buffer.slice(at + start, at + end) as ArrayBuffer,
    };
  }
  if (source instanceof ArrayBuffer) {
    return { byteLength: source.byteLength, slice: (start, end) => source.slice(start, end) };
  }
  return source as AsyncBuffer;
}

const COLUMNS = ["time", "leg_id", "pos", "mobility", "action"] as const;

/** Names (any case) or codes, as codes into `names`. */
function toCodes(values: ArrayLike<unknown>, names: readonly string[], column: string): Uint8Array {
  const lower = names.map((n) => n.toLowerCase());
  return Uint8Array.from({ length: values.length }, (_, i) => {
    const v = values[i];
    const code = typeof v === "number" ? v : lower.indexOf(String(v).toLowerCase());
    if (!(Number.isInteger(code) && code >= 0 && code < names.length)) {
      throw new RangeError(`column "${column}" has unknown value ${JSON.stringify(v)}`);
    }
    return code;
  });
}

/** Rows `[rowStart, rowEnd)` of the event columns. */
async function readRows(file: AsyncBuffer, metadata: FileMetaData, rowStart: number, rowEnd: number): Promise<EventRows> {
  const parts: Record<string, unknown[]> = Object.fromEntries(COLUMNS.map((c) => [c, []]));
  try {
    await parquetRead({
      file,
      metadata,
      rowStart,
      rowEnd,
      columns: [...COLUMNS],
      compressors: COMPRESSORS,
      onChunk: ({ columnName, columnData }) => {
        const out = parts[columnName];
        if (out) for (let i = 0; i < columnData.length; i++) out.push(columnData[i]);
      },
    });
  } catch (e) {
    if (e instanceof Error && e.message.includes("unsupported compression codec")) {
      throw new Error(`${e.message} (lefsim/parquet reads snappy and zstd)`);
    }
    throw e;
  }
  for (const c of COLUMNS) if (parts[c]!.length !== rowEnd - rowStart) throw new RangeError(`missing column "${c}"`);
  return {
    time: Float64Array.from(parts.time as number[]),
    legId: Uint32Array.from(parts.leg_id as number[], Number),
    pos: Uint32Array.from(parts.pos as number[], Number),
    mobility: toCodes(parts.mobility!, MOBILITY, "mobility"),
    kind: toCodes(parts.action!, ROW_KIND, "action"),
  };
}

function footerEntry(metadata: FileMetaData, key: string): string | undefined {
  return metadata.key_value_metadata?.find((kv) => kv.key === key)?.value;
}

/**
 * The lattice and LEF count from `lefsim.model`, without WebAssembly: the
 * replayer is pure TypeScript and doesn't check them (`new Sim` does).
 */
function fromModelJson(json: string): { lattice: Lattice; nLefs: number } {
  const doc = JSON.parse(json) as {
    lattice: { n_sites: number; ctcfs: { site: number; captures_from: string; strength: number }[] };
    lefs: unknown[];
  };
  const ctcfs = doc.lattice.ctcfs;
  const lattice: Lattice = {
    nSites: doc.lattice.n_sites,
    ctcfs: {
      site: Uint32Array.from(ctcfs, (c) => c.site),
      capturesFrom: toCodes(ctcfs.map((c) => c.captures_from), DIRECTION, "captures_from"),
      strength: Float64Array.from(ctcfs, (c) => c.strength),
    },
  };
  return { lattice, nLefs: doc.lefs.length };
}

/** A keyframed file's row groups: where each starts, in rows and in time. */
interface RowGroups {
  file: AsyncBuffer;
  metadata: FileMetaData;
  /** Keyframe time of each row group. */
  starts: number[];
  /** First row of each row group, then the total. */
  rows: number[];
  end: number;
}

/**
 * A replayer reading a keyframed Parquet file on demand: see
 * {@link replayerFromParquet}. The same methods as a `Replayer`, but the
 * ones that move are asynchronous.
 *
 * It holds a window of consecutive row groups. `seek` outside the window
 * reopens at the row group holding the target (one read); `advanceTo` loads
 * the next row groups as it goes (reopening on a far jump) and drops ones
 * it has passed; `snapshots` walks row group by row group.
 */
export class ParquetReplayer {
  readonly lattice: Lattice;
  readonly nLefs: number;
  #source: RowGroups | null;
  #core: Replayer;
  #first = 0;
  #last = 0;
  #loaded = false;
  #reads = 0;

  /** Use {@link replayerFromParquet}. */
  constructor(lattice: Lattice, nLefs: number, source: RowGroups | null, core?: Replayer) {
    this.lattice = lattice;
    this.nLefs = nLefs;
    this.#source = source;
    this.#core = core ?? new Replayer(lattice, nLefs);
  }

  /** How many row groups have been read so far (0 for a file read whole). */
  get rowGroupsRead(): number {
    return this.#reads;
  }

  /** Whether a keyframe has been applied, i.e. whether there's a state. */
  get ready(): boolean {
    return this.#core.ready;
  }

  /** Current replay time. */
  get time(): number {
    return this.#core.time;
  }

  /** Earliest time that can be reached (the first keyframe), or `null`. */
  get start(): number | null {
    return this.#source ? (this.#source.starts[0] ?? null) : this.#core.start;
  }

  /** Latest time the file describes. */
  get end(): number {
    return this.#source ? this.#source.end : this.#core.end;
  }

  /** The state at {@link ParquetReplayer.time}, as `Replayer.state()`. */
  state(): LiveState {
    return this.#core.state();
  }

  /** The state at {@link ParquetReplayer.time} as a `World` (copied). */
  world(): World {
    return this.#core.world();
  }

  #groupEnd(g: number): number {
    const s = this.#source!;
    return s.starts[g + 1] ?? s.end;
  }

  /** The row group holding time `t` (-1 before the first). */
  #groupAt(t: number): number {
    const starts = this.#source!.starts;
    let lo = 0;
    let hi = starts.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (starts[mid]! <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo - 1;
  }

  async #load(g: number): Promise<void> {
    const s = this.#source!;
    const rows = await readRows(s.file, s.metadata, s.rows[g]!, s.rows[g + 1]!);
    this.#core.push({ ...rows, end: this.#groupEnd(g) });
    if (!this.#loaded) this.#first = g;
    this.#last = g;
    this.#loaded = true;
    this.#reads++;
  }

  async #reopen(g: number): Promise<void> {
    this.#core = new Replayer(this.lattice, this.nLefs);
    this.#loaded = false;
    await this.#load(g);
  }

  #trim(): void {
    this.#core.trimBefore(this.#core.time);
    const start = this.#core.start;
    if (start !== null) this.#first = this.#groupAt(start);
  }

  /** Jump to `t` in either direction (clamped to `end`). Rejects before the first keyframe. */
  async seek(t: number): Promise<number> {
    if (Number.isNaN(t)) throw new RangeError("t is NaN");
    const s = this.#source;
    if (s) {
      const g = this.#groupAt(Math.min(t, s.end));
      if (g < 0) throw new RangeError(`no keyframe at or before t=${t} (earliest is ${this.start})`);
      if (!(this.#loaded && this.#first <= g && g <= this.#last)) await this.#reopen(g);
    }
    return this.#core.seek(t);
  }

  /**
   * Move to `t`: forward loads what it needs, back seeks. As
   * `Replayer.advanceTo`, it never fails for missing data: it waits before
   * the first keyframe, and once playing stays within `start` and `end`.
   */
  async advanceTo(t: number): Promise<number> {
    if (Number.isNaN(t)) throw new RangeError("t is NaN");
    const s = this.#source;
    if (!s) return this.#core.advanceTo(t);
    const first = s.starts[0];
    if (first === undefined) return this.#core.time;
    if (!this.#core.ready) return t < first ? this.#core.time : this.seek(t);
    if (t < this.#core.time) return this.seek(Math.max(t, first));
    const target = Math.min(t, s.end);
    const g = this.#groupAt(target);
    if (g > this.#last + 1) {
      // A far jump: start over at the row group holding the target.
      await this.#reopen(g);
      return this.#core.seek(target);
    }
    for (let h = this.#last + 1; h <= g; h++) await this.#load(h);
    const reached = this.#core.advanceTo(target);
    this.#trim();
    return reached;
  }

  /** {@link ParquetReplayer.seek} to `t`, then {@link ParquetReplayer.world}. */
  async worldAt(t: number): Promise<World> {
    await this.seek(t);
    return this.world();
  }

  /**
   * Snapshots on the grid `t0 + k * dt`, as `Replayer.snapshots`, reading
   * only the row groups the range covers.
   */
  async snapshots(dt: number, options: { t0?: number; tStart?: number; tEnd?: number } = {}): Promise<SnapshotColumns> {
    if (options.tStart !== undefined) await this.seek(options.tStart);
    const tEnd = options.tEnd ?? this.end;
    const t0 = options.t0;
    const s = this.#source;
    if (!s || s.starts.length === 0) return this.#core.snapshots(dt, { t0, tEnd });
    if (!this.#loaded) await this.#reopen(0);
    const pieces: SnapshotColumns[] = [];
    for (;;) {
      const groupEnd = this.#groupEnd(this.#last);
      pieces.push(this.#core.snapshots(dt, { t0, tEnd: Math.min(tEnd, groupEnd) }));
      if (tEnd <= groupEnd || this.#last + 1 >= s.starts.length) break;
      await this.#load(this.#last + 1);
      this.#trim();
    }
    return concatSnapshots(pieces, this.nLefs);
  }
}

function concatSnapshots(pieces: SnapshotColumns[], nLefs: number): SnapshotColumns {
  const cat = <T extends Float64Array | Uint32Array | Uint8Array>(key: Exclude<keyof SnapshotColumns, "nLefs">, Ctor: { new (n: number): T }): T => {
    const out = new Ctor(pieces.reduce((n, p) => n + p[key].length, 0));
    let at = 0;
    for (const p of pieces) {
      out.set(p[key] as never, at);
      at += p[key].length;
    }
    return out;
  };
  return {
    nLefs,
    time: cat("time", Float64Array),
    leftPos: cat("leftPos", Uint32Array),
    rightPos: cat("rightPos", Uint32Array),
    leftMobility: cat("leftMobility", Uint8Array),
    rightMobility: cat("rightMobility", Uint8Array),
  };
}

/**
 * Open a keyframed event log in Parquet for replay. For a file written by
 * the Python binding's `Stream.to_parquet`, only the footer is read up
 * front (the lattice and end come from its `lefsim.model` and `lefsim.run`);
 * each move then reads just the row groups it needs. Any other file is read
 * whole and re-split at its keyframes (pass `lattice` if it has no
 * `lefsim.model`). Reads snappy and zstd. The TS counterpart of Python's
 * `Replayer.from_parquet`.
 */
export async function replayerFromParquet(source: ParquetSource, options: { lattice?: Lattice } = {}): Promise<ParquetReplayer> {
  const file = await asyncBuffer(source);
  const metadata = await parquetMetadataAsync(file);
  const modelJson = footerEntry(metadata, "lefsim.model");
  const runJson = footerEntry(metadata, "lefsim.run");
  const fromModel = modelJson ? fromModelJson(modelJson) : undefined;
  const lattice = options.lattice ?? fromModel?.lattice;
  if (!lattice) throw new RangeError("the file has no lefsim.model: pass lattice");
  const recordEnd = runJson ? (JSON.parse(runJson) as { record_end: number | null }).record_end : null;

  const rows = [0];
  for (const g of metadata.row_groups) rows.push(rows.at(-1)! + Number(g.num_rows));
  const timeColumn = metadata.row_groups[0]?.columns.findIndex((c) => c.meta_data?.path_in_schema[0] === "time") ?? -1;
  const starts = metadata.row_groups.map((g) => {
    const stats = g.columns[timeColumn]?.meta_data?.statistics;
    const min = stats?.min_value ?? stats?.min;
    // Parquet writes a zero float minimum as -0; `+ 0` makes it +0.
    return typeof min === "number" ? min + 0 : Number.NaN;
  });
  const keyframed = footerEntry(metadata, "lefsim.row_groups") === "keyframed";
  if (!keyframed || !fromModel || starts.some(Number.isNaN)) {
    // Not our layout: read everything and re-split at keyframes.
    const all = await readRows(file, metadata, 0, rows.at(-1)!);
    const core = replayerFromRows(all, lattice, { end: recordEnd ?? undefined });
    return new ParquetReplayer(lattice, core.nLefs, null, core);
  }
  const end = recordEnd ?? starts.at(-1) ?? Number.NEGATIVE_INFINITY;
  return new ParquetReplayer(lattice, fromModel.nLefs, { file, metadata, starts, rows, end });
}
