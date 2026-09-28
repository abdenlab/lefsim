/**
 * Replays keyframed event deltas into a live state, with no WebAssembly.
 *
 * A Replayer exposes the same `state()` as a `Sim`, so a renderer can't
 * tell whether it's watching a local simulation or a recorded/streamed one.
 */

import { KEYFRAME, MOBILITY, ROW_KIND, type DeltaChunk, type Lattice, type LiveState, type SnapshotColumns, type World } from "./types.ts";

interface KeyframeRef {
  chunk: number;
  row: number;
  time: number;
}

/**
 * Applies {@link DeltaChunk}s in order and reconstructs every LEF's state.
 *
 * Feed chunks with {@link Replayer.push} (each starting where the last one
 * ended), then move through time with {@link Replayer.advanceTo} (forward,
 * cheap) or {@link Replayer.seek} (any direction; restarts from the nearest
 * keyframe at or before the target, so its cost is bounded by chunk size).
 *
 * Rows before the first keyframe can't be decoded and are skipped, so a
 * replayer can start partway into a stream (a late joiner).
 */
export class Replayer {
  readonly lattice: Lattice;
  readonly nLefs: number;
  #chunks: DeltaChunk[] = [];
  #keyframes: KeyframeRef[] = [];
  #cursor = { chunk: 0, row: 0 }; // next row to apply
  #time = -Infinity;
  #end = -Infinity;
  #ready = false;
  #leftPos: Uint32Array;
  #rightPos: Uint32Array;
  #leftMob: Uint8Array;
  #rightMob: Uint8Array;
  /** The grid of the last `snapshots` call and its next index; cleared by a seek. */
  #grid: { dt: number; t0: number; next: number } | null = null;

  /** A replayer for a run on `lattice` with `nLefs` LEFs. */
  constructor(lattice: Lattice, nLefs: number) {
    if (!Number.isInteger(nLefs) || nLefs < 0) throw new RangeError(`nLefs must be a non-negative integer, got ${nLefs}`);
    this.lattice = lattice;
    this.nLefs = nLefs;
    this.#leftPos = new Uint32Array(nLefs);
    this.#rightPos = new Uint32Array(nLefs);
    this.#leftMob = new Uint8Array(nLefs);
    this.#rightMob = new Uint8Array(nLefs);
  }

  /** Whether a keyframe has been applied, i.e. whether {@link Replayer.state} is defined. */
  get ready(): boolean {
    return this.#ready;
  }

  /** Current replay time. */
  get time(): number {
    return this.#time;
  }

  /** Earliest time that can be sought to (the first keyframe), or `null`. */
  get start(): number | null {
    return this.#keyframes[0]?.time ?? null;
  }

  /** Latest time the pushed chunks describe; replay can't go past it. */
  get end(): number {
    return this.#end;
  }

  /**
   * Append a chunk of rows, which describes the run through `chunk.end`. Its
   * rows must not precede rows already pushed, and it must hold whole
   * keyframes (one row per leg, in `leg_id` order, at one time). Checks
   * every row, as the core's replayer does; on error, nothing is pushed.
   */
  push(chunk: DeltaChunk): void {
    const n = chunk.time.length;
    const { time, legId, mobility, kind } = chunk;
    if (legId.length !== n || chunk.pos.length !== n || mobility.length !== n || kind.length !== n) {
      throw new RangeError("chunk columns must have equal length");
    }
    const nLegs = 2 * this.nLefs;
    let previous = this.#lastRowTime();
    for (let row = 0; row < n; row++) {
      const t = time[row]!;
      if (Number.isNaN(t) || t < previous) throw new RangeError(`row at t=${t} comes after a row at t=${previous}`);
      previous = t;
      if (legId[row]! >= nLegs) throw new RangeError(`leg id ${legId[row]} is out of range for ${nLegs} legs`);
      if (mobility[row]! >= MOBILITY.length) throw new RangeError(`invalid mobility code ${mobility[row]}`);
      if (kind[row]! >= ROW_KIND.length) throw new RangeError(`invalid row kind code ${kind[row]}`);
    }
    if (Number.isNaN(chunk.end) || chunk.end < previous) {
      throw new RangeError(`chunk end ${chunk.end} is before its last row at t=${previous}`);
    }

    const index = this.#chunks.length;
    const found: KeyframeRef[] = [];
    for (let row = 0; row < n; ) {
      if (kind[row] !== KEYFRAME) {
        row++;
        continue;
      }
      const start = row;
      while (row < n && kind[row] === KEYFRAME && time[row] === time[start] && legId[row] === row - start) row++;
      if (row - start !== nLegs) {
        throw new RangeError(`keyframe at t=${time[start]} has ${row - start} rows; expected ${nLegs}, one per leg in order`);
      }
      found.push({ chunk: index, row: start, time: time[start]! });
    }
    this.#keyframes.push(...found);
    this.#chunks.push(chunk);
    this.#end = Math.max(this.#end, chunk.end);
  }

  /**
   * Move to time `t`, clamped to {@link Replayer.end}. Going forward applies
   * the rows in between, or restarts from a keyframe in between if there is
   * one (so any jump costs at most one chunk); going backward seeks. Unlike
   * {@link Replayer.seek} it never throws for missing data, which suits a
   * playback loop: it waits before the first keyframe, and once playing it
   * stays within {@link Replayer.start} and {@link Replayer.end}. Returns
   * the time reached (less than `t` when the data runs out, i.e. while
   * buffering; more than `t` when asked to go back before the data).
   */
  advanceTo(t: number): number {
    if (Number.isNaN(t)) throw new RangeError("t is NaN");
    if (!this.#ready) {
      const first = this.start;
      if (first === null || t < first) return this.#time;
      return this.seek(t);
    }
    // Back no further than the first keyframe, as forward stops at `end`.
    if (t < this.#time) return this.seek(Math.max(t, this.start!));
    const target = Math.min(t, this.#end);
    // A keyframe between here and the target holds the full state, so
    // restarting from it skips every row before it: a long jump costs at
    // most one chunk, as a seek does. Positions, not times, decide "ahead",
    // since a keyframe can share its time with the rows before it.
    const k = this.#keyframeAtOrBefore(target);
    const cur = this.#cursor;
    if (k && (k.chunk > cur.chunk || (k.chunk === cur.chunk && k.row > cur.row))) {
      this.#cursor = { chunk: k.chunk, row: k.row };
    }
    this.#applyThrough(target);
    this.#time = Math.max(this.#time, target);
    return this.#time;
  }

  /**
   * Jump to time `t` in either direction: restore the last keyframe at or
   * before `t`, then replay forward. Throws if `t` precedes every keyframe.
   */
  seek(t: number): number {
    if (Number.isNaN(t)) throw new RangeError("t is NaN");
    const target = Math.min(t, this.#end);
    const k = this.#keyframeAtOrBefore(target);
    if (!k) throw new RangeError(`no keyframe at or before t=${t} (earliest is ${this.start})`);
    this.#cursor = { chunk: k.chunk, row: k.row };
    this.#applyThrough(target);
    this.#ready = true;
    this.#time = target;
    this.#grid = null;
    return target;
  }

  /**
   * Snapshots on the grid `t0 + k * dt` from `tStart` (default: the current
   * time, or the first keyframe if not ready) through `tEnd` (default:
   * {@link Replayer.end}): the same columns `Sim.recordUntil(tEnd, { dt })`
   * records. Leaves the replayer at the last grid point. Like a recording,
   * consecutive calls on the same grid continue it without repeating a
   * point; a seek starts afresh.
   */
  snapshots(dt: number, options: { t0?: number; tStart?: number; tEnd?: number } = {}): SnapshotColumns {
    const t0 = options.t0 ?? 0;
    if (!(Number.isFinite(dt) && dt > 0)) throw new RangeError(`dt must be positive and finite, got ${dt}`);
    if (!Number.isFinite(t0)) throw new RangeError(`t0 must be finite, got ${t0}`);
    if (options.tStart !== undefined) this.seek(options.tStart);
    const tEnd = options.tEnd ?? this.#end;
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");

    const from = this.#ready ? this.#time : this.start;
    const times: number[] = [];
    let next: number | null = null;
    if (from !== null) {
      // Grid points are `t0 + k * dt`, computed exactly as a recording
      // computes them, so the two agree bit for bit.
      const point = (k: number) => t0 + k * dt;
      let k = Math.max(0, Math.ceil((from - t0) / dt));
      while (k > 0 && point(k - 1) >= from) k--;
      while (point(k) < from) k++;
      const g = this.#grid;
      if (g && g.dt === dt && g.t0 === t0) k = Math.max(k, g.next);
      const last = Math.min(tEnd, this.#end);
      for (; point(k) <= last; k++) times.push(point(k));
      next = k;
    }

    const n = this.nLefs;
    const out: SnapshotColumns = {
      nLefs: n,
      time: Float64Array.from(times),
      leftPos: new Uint32Array(times.length * n),
      rightPos: new Uint32Array(times.length * n),
      leftMobility: new Uint8Array(times.length * n),
      rightMobility: new Uint8Array(times.length * n),
    };
    times.forEach((t, s) => {
      this.advanceTo(t);
      out.leftPos.set(this.#leftPos, s * n);
      out.rightPos.set(this.#rightPos, s * n);
      out.leftMobility.set(this.#leftMob, s * n);
      out.rightMobility.set(this.#rightMob, s * n);
    });
    // After the loop: its first advance may seek, which clears the grid.
    if (next !== null) this.#grid = { dt, t0, next };
    return out;
  }

  /**
   * The state at {@link Replayer.time}, in the same shape as `Sim.state()`.
   * The arrays are updated in place: valid until the next advance or seek.
   */
  state(): LiveState {
    if (!this.#ready) throw new Error("no keyframe applied yet: push a keyframed chunk and advance");
    return {
      time: this.#time,
      leftPos: this.#leftPos,
      rightPos: this.#rightPos,
      leftMobility: this.#leftMob,
      rightMobility: this.#rightMob,
    };
  }

  /**
   * The state at {@link Replayer.time} as a {@link World}, with copied
   * arrays: to start a new run from, e.g.
   * `new Sim(createModel(replayer.world(), rates), { seed, startTime: replayer.time })`.
   * The replayer doesn't check it (it has no WebAssembly); `createModel`
   * and `new Sim` do.
   */
  world(): World {
    const s = this.state();
    return {
      lattice: this.lattice,
      lefs: {
        leftPos: s.leftPos.slice(),
        rightPos: s.rightPos.slice(),
        leftMobility: s.leftMobility.slice(),
        rightMobility: s.rightMobility.slice(),
      },
    };
  }

  /** {@link Replayer.seek} to `t`, then {@link Replayer.world}. */
  worldAt(t: number): World {
    this.seek(t);
    return this.world();
  }

  /**
   * Drop chunks no longer needed to seek to `t` or later, to bound memory in
   * a long-running stream. Keeps everything from the last keyframe at or
   * before `t` (and the current position).
   */
  trimBefore(t: number): void {
    const k = this.#keyframeAtOrBefore(t);
    if (!k) return;
    const drop = Math.min(k.chunk, this.#cursor.chunk);
    if (drop === 0) return;
    this.#chunks.splice(0, drop);
    this.#keyframes = this.#keyframes
      .filter((ref) => ref.chunk >= drop)
      .map((ref) => ({ ...ref, chunk: ref.chunk - drop }));
    this.#cursor = { chunk: this.#cursor.chunk - drop, row: this.#cursor.row };
  }

  // ---------------------------------------------------------------- internals

  #lastRowTime(): number {
    for (let c = this.#chunks.length - 1; c >= 0; c--) {
      const times = this.#chunks[c]!.time;
      if (times.length) return times[times.length - 1]!;
    }
    return -Infinity;
  }

  #keyframeAtOrBefore(t: number): KeyframeRef | undefined {
    // Binary search for the last keyframe with time <= t.
    let lo = 0;
    let hi = this.#keyframes.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.#keyframes[mid]!.time <= t) lo = mid + 1;
      else hi = mid;
    }
    return this.#keyframes[lo - 1];
  }

  /** Apply rows from the cursor while their time is <= `t`. */
  #applyThrough(t: number): void {
    let { chunk: c, row } = this.#cursor;
    while (c < this.#chunks.length) {
      const ch = this.#chunks[c]!;
      const n = ch.time.length;
      while (row < n && ch.time[row]! <= t) {
        const leg = ch.legId[row]!;
        const lef = leg >> 1;
        if (leg & 1) {
          this.#rightPos[lef] = ch.pos[row]!;
          this.#rightMob[lef] = ch.mobility[row]!;
        } else {
          this.#leftPos[lef] = ch.pos[row]!;
          this.#leftMob[lef] = ch.mobility[row]!;
        }
        row++;
      }
      if (row < n) break;
      c++;
      row = 0;
    }
    this.#cursor = { chunk: c, row };
  }
}
