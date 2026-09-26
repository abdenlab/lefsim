/**
 * Replays keyframed event deltas into a live state, with no WebAssembly.
 *
 * A Replayer exposes the same `state()` as a `Sim`, so a renderer can't
 * tell whether it's watching a local simulation or a recorded/streamed one.
 */

import { KEYFRAME, type DeltaChunk, type LiveState } from "./types.ts";

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

  constructor(nLefs: number) {
    if (!Number.isInteger(nLefs) || nLefs < 0) throw new RangeError(`nLefs must be a non-negative integer, got ${nLefs}`);
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

  /** Append a chunk. Its rows must not precede rows already pushed. */
  push(chunk: DeltaChunk): void {
    const n = chunk.time.length;
    if (chunk.legId.length !== n || chunk.pos.length !== n || chunk.mobility.length !== n || chunk.kind.length !== n) {
      throw new RangeError("chunk columns must have equal length");
    }
    const last = this.#lastRowTime();
    if (n > 0 && chunk.time[0]! < last) throw new RangeError("chunk rows precede rows already pushed");
    const index = this.#chunks.length;
    for (let row = 0; row < n; ) {
      if (chunk.kind[row] !== KEYFRAME) {
        row++;
        continue;
      }
      const start = row;
      while (row < n && chunk.kind[row] === KEYFRAME && chunk.time[row] === chunk.time[start]) row++;
      if (row - start !== 2 * this.nLefs) {
        throw new RangeError(`keyframe has ${row - start} rows, expected ${2 * this.nLefs} (2 per LEF)`);
      }
      this.#keyframes.push({ chunk: index, row: start, time: chunk.time[start]! });
    }
    this.#chunks.push(chunk);
    this.#end = Math.max(this.#end, chunk.end);
  }

  /**
   * Move to time `t`, clamped to {@link Replayer.end}. Going forward applies
   * only the rows in between; going backward seeks. Returns the time reached
   * (less than `t` when the data runs out, i.e. while buffering).
   */
  advanceTo(t: number): number {
    if (Number.isNaN(t)) throw new RangeError("t is NaN");
    if (!this.#ready) {
      const first = this.start;
      if (first === null || t < first) return this.#time;
      return this.seek(t);
    }
    if (t < this.#time) return this.seek(t);
    const target = Math.min(t, this.#end);
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
    return target;
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
