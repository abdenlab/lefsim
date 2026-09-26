/**
 * Stochastic simulation of loop-extrusion factors (LEFs) on a 1D lattice,
 * running in WebAssembly.
 *
 * Call {@link init} (or {@link initSync}) once, then create {@link Sim}s.
 * A `Sim` is one realization: a single forward-only clock, advanced in place
 * by {@link Sim.runUntil} (absolute time) or {@link Sim.runEvents}.
 */

import wasmInit, { initSync as wasmInitSync, names, WasmSim } from "lefsim-wasm";
import type { InitInput } from "lefsim-wasm";

import type { DeltaChunk, LiveState, Meta, Rates, SnapshotChunk, SnapshotColumns } from "./types.ts";

export { DIRECTION, KEYFRAME, MOBILITY, ROW_KIND, SITE_KIND } from "./types.ts";
export type { DeltaChunk, LiveState, Meta, Mobility, Rates, SnapshotChunk, SnapshotColumns } from "./types.ts";

// ------------------------------------------------------------------ setup

let memory: WebAssembly.Memory | undefined;

/**
 * Load and instantiate the WebAssembly module. Safe to call repeatedly.
 *
 * In a browser or bundler, call with no arguments to fetch the module next
 * to the package. Elsewhere, pass a URL, a `Response`, bytes or a compiled
 * `WebAssembly.Module`.
 */
export async function init(source?: InitInput | Promise<InitInput>): Promise<void> {
  if (memory) return;
  const out = await wasmInit(source === undefined ? undefined : { module_or_path: source });
  memory = out.memory;
}

/** Synchronous {@link init}, from bytes or a compiled module (e.g. in Node). */
export function initSync(module: BufferSource | WebAssembly.Module): void {
  if (memory) return;
  memory = wasmInitSync({ module }).memory;
}

function wasmMemory(): WebAssembly.Memory {
  if (!memory) throw new Error("lefsim: call init() before creating a Sim");
  return memory;
}

// ------------------------------------------------------------------ names

/** The code → name tables as the core defines them (requires {@link init}). */
export function coreNames(): { mobility: string[]; direction: string[]; siteKind: string[]; rowKind: string[] } {
  wasmMemory();
  return names() as ReturnType<typeof coreNames>;
}

// ------------------------------------------------------------------ types

export type Side = "left" | "right";

/** A CTCF site. `capturesFrom` is its capture side; the opposite side evicts. */
export interface CtcfSpec {
  site: number;
  capturesFrom: Side;
  /** Probability the CTCF acts when a leg lands on it. Default 1. */
  strength?: number;
}

/** CTCFs as columns, convenient for large (e.g. genome-scale) inputs. */
export interface CtcfColumns {
  site: ArrayLike<number>;
  capturesFrom: ArrayLike<Side> | Side;
  strength?: ArrayLike<number> | number;
}

export interface SimOptions {
  nSites: number;
  ctcfs?: CtcfSpec[] | CtcfColumns;
  /** Explicit LEF sites; these LEFs get the first ids. */
  lefSites?: ArrayLike<number>;
  /** LEFs to place at random non-CTCF sites, after `lefSites`. */
  nLefs?: number;
  /** Any integer in [0, 2^64). Default 0. */
  seed?: number | bigint;
  /** Omitted rates use the core's defaults. */
  rates?: Partial<Rates>;
}

export type StopReason = "reached_time" | "reached_event_count" | "exhausted";

/** A recorded chunk of event deltas plus why recording stopped. */
export interface EventChunk extends DeltaChunk {
  /** "recorder_full" means the chunk filled first: call again for the next. */
  stopped: StopReason | "recorder_full";
}

/** What to record in {@link Sim.recordUntil} / {@link Sim.recordEvents}. */
export interface RecordOptions {
  /** Take grid snapshots at `t0 + k * dt`. */
  dt?: number;
  /** Grid origin. Default 0. */
  t0?: number;
  /** Log event deltas. */
  events?: boolean;
  /** Open the event log with a keyframe (requires `events`). */
  keyframes?: boolean;
}

/** The outcome of {@link Sim.recordUntil} / {@link Sim.recordEvents}. */
export interface Recording extends RunReport {
  /** Grid snapshots, if `dt` was given. */
  snapshots: SnapshotColumns | null;
  /** Event deltas, if `events` was set. `end` is the time after the call. */
  events: DeltaChunk | null;
}

export interface RunReport {
  eventsFired: number;
  time: number;
  stopped: StopReason;
}

// ------------------------------------------------------------------ validation

const U32_MAX = 2 ** 32 - 1;

// Typed arrays coerce silently (-1 → 4294967295, 1.5 → 1), so check first.
function toU32Array(values: ArrayLike<number>, what: string): Uint32Array {
  const out = new Uint32Array(values.length);
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (!Number.isInteger(v) || v < 0 || v > U32_MAX) {
      throw new RangeError(`${what} must be non-negative integers, got ${v}`);
    }
    out[i] = v;
  }
  return out;
}

function sideCode(side: unknown): number {
  if (side === "left") return 0;
  if (side === "right") return 1;
  throw new RangeError(`capturesFrom must be "left" or "right", got ${JSON.stringify(side)}`);
}

function ctcfArrays(ctcfs: SimOptions["ctcfs"]): [Uint32Array, Uint8Array, Float64Array] {
  if (ctcfs === undefined) return [new Uint32Array(), new Uint8Array(), new Float64Array()];
  if (Array.isArray(ctcfs)) {
    return [
      toU32Array(ctcfs.map((c) => c.site), "CTCF sites"),
      Uint8Array.from(ctcfs, (c) => sideCode(c.capturesFrom)),
      Float64Array.from(ctcfs, (c) => c.strength ?? 1),
    ];
  }
  const cols = ctcfs as CtcfColumns;
  const n = cols.site.length;
  const at = <T>(v: ArrayLike<T> | T, i: number, name: string): T => {
    if (typeof v === "object" && v !== null && "length" in v) {
      if (v.length !== n) throw new RangeError(`ctcfs.${name} has ${v.length} entries, site has ${n}`);
      return v[i]!;
    }
    return v as T;
  };
  return [
    toU32Array(cols.site, "CTCF sites"),
    Uint8Array.from({ length: n }, (_, i) => sideCode(at(cols.capturesFrom, i, "capturesFrom"))),
    Float64Array.from({ length: n }, (_, i) => at(cols.strength ?? 1, i, "strength")),
  ];
}

function seedBigInt(seed: number | bigint | undefined): bigint {
  if (seed === undefined) return 0n;
  if (typeof seed === "number" && !Number.isSafeInteger(seed)) {
    throw new RangeError(`seed must be an integer (use a bigint beyond 2^53), got ${seed}`);
  }
  const s = BigInt(seed);
  if (s < 0n || s >= 2n ** 64n) throw new RangeError(`seed must be in [0, 2^64), got ${seed}`);
  return s;
}

/** Event columns from wasm lack `end`; it is the time after the call. */
function withEventEnd(r: Recording): Recording {
  if (r.events) r.events.end = r.time;
  return r;
}

function checkCount(n: number, what: string): number {
  if (!Number.isInteger(n) || n < 0) throw new RangeError(`${what} must be a non-negative integer, got ${n}`);
  return n;
}

export { Replayer } from "./replayer.ts";

// ------------------------------------------------------------------ Sim

/**
 * A loop-extrusion simulation on a 1D lattice.
 *
 * LEFs are placed at construction: one at each of `lefSites`, then `nLefs`
 * more at random non-CTCF sites, numbered in that order (`lef_id`). Legs are
 * numbered `leg_id = 2 * lef_id + side`, where side 0 faces left and side 1
 * faces right.
 *
 * Holds WebAssembly memory: call {@link Sim.free} when done, or declare it
 * with `using`. A forgotten Sim is freed eventually by the garbage collector.
 */
export class Sim implements Disposable {
  #inner: WasmSim | undefined;
  readonly nSites: number;
  readonly nLefs: number;

  constructor(options: SimOptions) {
    wasmMemory();
    const nSites = options.nSites;
    if (!Number.isInteger(nSites) || nSites < 1 || nSites > U32_MAX) {
      throw new RangeError(`nSites must be an integer in [1, 2^32), got ${nSites}`);
    }
    const [site, side, strength] = ctcfArrays(options.ctcfs);
    const rates = options.rates ?? {};
    this.#inner = new WasmSim(
      nSites,
      site,
      side,
      strength,
      toU32Array(options.lefSites ?? [], "lefSites"),
      checkCount(options.nLefs ?? 0, "nLefs"),
      seedBigInt(options.seed),
      rates.step,
      rates.respawn,
      rates.respawnCaptured,
    );
    this.nSites = nSites;
    this.nLefs = this.#inner.n_lefs();
  }

  get #sim(): WasmSim {
    if (!this.#inner) throw new Error("this Sim has been freed");
    return this.#inner;
  }

  get nLegs(): number {
    return 2 * this.nLefs;
  }

  /** Current simulation time. */
  get time(): number {
    return this.#sim.time();
  }

  /** Total events fired so far. */
  get eventCount(): number {
    return this.#sim.event_count();
  }

  /** Time of the next event, or `null` if none remain. */
  get nextEventTime(): number | null {
    return this.#sim.next_event_time() ?? null;
  }

  get rates(): Rates {
    return this.#sim.rates() as Rates;
  }

  /**
   * Fire every event at or before `tEnd` (an absolute time; a `tEnd` at or
   * before {@link Sim.time} does nothing).
   *
   * `maxEvents` caps the work per call, e.g. to keep an animation frame
   * within budget. If the cap is hit first, `stopped` is
   * `"reached_event_count"` and the clock stays at the last event; call
   * again to continue.
   */
  runUntil(tEnd: number, options: { maxEvents?: number } = {}): RunReport {
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");
    const maxEvents = options.maxEvents ?? Infinity;
    if (maxEvents !== Infinity) checkCount(maxEvents, "maxEvents");
    return this.#sim.run_until(tEnd, maxEvents) as RunReport;
  }

  /** Fire up to `n` events. */
  runEvents(n: number): RunReport {
    return this.#sim.run_events(checkCount(n, "n")) as RunReport;
  }

  /**
   * {@link Sim.runUntil}, recording grid snapshots (`dt`) and/or event
   * deltas (`events`, optionally `keyframes`). Snapshots are exact, not
   * interpolated: a grid point records the state after every event at or
   * before it, and the grid includes `tEnd`. Consecutive calls with the same
   * grid continue it without repeating points.
   */
  recordUntil(tEnd: number, options: RecordOptions & { maxEvents?: number } = {}): Recording {
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");
    const maxEvents = options.maxEvents ?? Infinity;
    if (maxEvents !== Infinity) checkCount(maxEvents, "maxEvents");
    const r = this.#sim.record_until(tEnd, maxEvents, options.dt, options.t0 ?? 0, options.events ?? false, options.keyframes ?? false);
    return withEventEnd(r as Recording);
  }

  /** {@link Sim.runEvents}, recording as in {@link Sim.recordUntil}. */
  recordEvents(n: number, options: RecordOptions = {}): Recording {
    const r = this.#sim.record_events(checkCount(n, "n"), options.dt, options.t0 ?? 0, options.events ?? false, options.keyframes ?? false);
    return withEventEnd(r as Recording);
  }

  /**
   * Run toward `tEnd`, taking grid snapshots into one chunk of at most
   * `maxSnapshots` (default 1,000), firing at most `maxEvents` (default
   * 250,000) so each call stays short. Call repeatedly while `stopped` is
   * `"recorder_full"` or `"reached_event_count"`; the grid continues across
   * calls.
   */
  recordSnapshotChunk(
    tEnd: number,
    options: { dt: number; t0?: number; maxSnapshots?: number; maxEvents?: number },
  ): SnapshotChunk {
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");
    const maxSnapshots = checkCount(options.maxSnapshots ?? 1_000, "maxSnapshots");
    const maxEvents = checkCount(options.maxEvents ?? 250_000, "maxEvents");
    return this.#sim.record_snapshot_chunk(tEnd, options.dt, options.t0 ?? 0, maxSnapshots, maxEvents) as SnapshotChunk;
  }

  /**
   * Run toward `tEnd` (absolute time), recording event deltas into one chunk
   * of at most `maxRows` rows (default 10,000). Call repeatedly until
   * `stopped` isn't `"recorder_full"`; consecutive chunks tile the run.
   * Every event adds a row, so `maxRows` also bounds the work per call.
   *
   * With `keyframes` (the default), each chunk opens with one `Keyframe`
   * row per leg, so it can be replayed on its own, and may exceed `maxRows`
   * by the rows of a zero-delay chain in progress (typically two).
   */
  recordEventChunk(tEnd: number, options: { maxRows?: number; keyframes?: boolean } = {}): EventChunk {
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");
    const maxRows = checkCount(options.maxRows ?? 10_000, "maxRows");
    return this.#sim.record_event_chunk(tEnd, maxRows, options.keyframes ?? true) as EventChunk;
  }

  /** The current state of every LEF. See {@link LiveState} for the view contract. */
  state(): LiveState {
    const sim = this.#sim;
    sim.sync_state();
    const buf = wasmMemory().buffer; // re-read: memory may have grown
    const n = this.nLefs;
    return {
      time: sim.time(),
      leftPos: new Uint32Array(buf, sim.left_pos_ptr(), n),
      rightPos: new Uint32Array(buf, sim.right_pos_ptr(), n),
      leftMobility: new Uint8Array(buf, sim.left_mob_ptr(), n),
      rightMobility: new Uint8Array(buf, sim.right_mob_ptr(), n),
    };
  }

  /** Static tables describing the run. Arrays are copies, safe to keep. */
  meta(): Meta {
    return this.#sim.meta() as Meta;
  }

  /** Release the WebAssembly memory. Idempotent. */
  free(): void {
    this.#inner?.free();
    this.#inner = undefined;
  }

  [Symbol.dispose](): void {
    this.free();
  }
}
