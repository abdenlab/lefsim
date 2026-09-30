/**
 * Stochastic simulation of loop-extrusion factors (LEFs) on a 1D lattice,
 * running in WebAssembly.
 *
 * Call {@link init} (or {@link initSync}) once. Describe a model with
 * {@link createLattice}, {@link populate} or {@link createWorld}, and
 * {@link createModel}; then run it with a {@link Sim}. A `Sim` is one
 * realization: a single forward-only clock, advanced in place by
 * {@link Sim.runUntil} (absolute time), {@link Sim.runEvents} or
 * {@link Sim.step}.
 */

import wasmInit, { initSync as wasmInitSync, names, WasmSim } from "lefsim-wasm";
import type { InitInput } from "lefsim-wasm";

import { lefArgs, modelArgs, seedBigInt } from "./model.ts";
import { ROW_KIND } from "./types.ts";
import type { DeltaChunk, Lattice, LiveState, Model, Rates, SnapshotChunk, SnapshotColumns, SpawnRule, StopReason, World } from "./types.ts";

export { DIRECTION, KEYFRAME, MOBILITY, ROW_KIND, SITE_KIND } from "./types.ts";
export type {
  Ctcfs,
  DeltaChunk,
  Lattice,
  Lefs,
  LiveState,
  Mobility,
  Model,
  Rates,
  SnapshotChunk,
  SnapshotColumns,
  SpawnRule,
  StopReason,
  World,
} from "./types.ts";
export {
  createLattice,
  createModel,
  createWorld,
  defaultRates,
  modelFromJson,
  modelToJson,
  populate,
  SPAWN_RULES,
} from "./model.ts";
export type { CtcfColumns, CtcfSpec, LefColumns, LefSpec, Side } from "./model.ts";
export { effectiveDistance, effectiveDistances, effectiveDistancesOver, type PathLengths } from "./distance.ts";

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

export interface SimOptions {
  /** Any integer in [0, 2^64). Default 0. */
  seed?: number | bigint;
  /** The clock's start, e.g. to continue from a recorded state. Default 0. */
  startTime?: number;
}

/** What a run did: events fired by this call, the time reached, and why it stopped. */
export interface RunReport {
  nEvents: number;
  time: number;
  stopped: StopReason;
}

/** A recorded chunk of event deltas plus why recording stopped. */
export interface EventChunk extends DeltaChunk {
  /** "recorder_full" means the chunk filled first: call again for the next. */
  stopped: StopReason;
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

/** One fired event: its time, action, and the `leg_id` (a step) or `lef_id` (a respawn). */
export interface FiredEvent {
  time: number;
  action: "Step" | "Respawn";
  id: number;
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

/** Internal: `new Sim` wraps an existing wasm sim (for {@link Sim.clone}). */
const FORK = Symbol("fork");

export { Replayer } from "./replayer.ts";

/**
 * Iterate a `ReadableStream` with `for await`, in every browser.
 *
 * (`ReadableStream` is async-iterable natively only in recent Safari.)
 * Breaking out of the loop cancels the stream, as native iteration does.
 */
export async function* iterate<T>(stream: ReadableStream<T>): AsyncGenerator<T, void, undefined> {
  const reader = stream.getReader();
  let finished = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        finished = true;
        return;
      }
      yield value;
    }
  } finally {
    if (finished) reader.releaseLock();
    else await reader.cancel();
  }
}

// ------------------------------------------------------------------ Sim

/**
 * A running simulation of a {@link Model}.
 *
 * `seed` drives the whole run: the same model and seed give the same run,
 * bit for bit, on every platform. LEFs keep their row numbers from the
 * model's world (`lef_id`); legs are numbered `leg_id = 2 * lef_id + side`,
 * where side 0 faces left and side 1 faces right.
 *
 * Holds WebAssembly memory: call {@link Sim.free} when done, or declare it
 * with `using`. A forgotten Sim is freed eventually by the garbage collector.
 */
export class Sim implements Disposable {
  #inner: WasmSim | undefined;
  readonly lattice: Lattice;
  readonly spawnRule: SpawnRule;
  readonly nSites: number;
  readonly nLefs: number;

  constructor(model: Model, options: SimOptions = {}) {
    wasmMemory();
    const fork = (options as { [FORK]?: WasmSim })[FORK];
    this.#inner = fork ?? new WasmSim(...modelArgs(model), seedBigInt(options.seed), options.startTime ?? 0);
    this.lattice = model.world.lattice;
    this.spawnRule = model.spawnRule;
    this.nSites = this.lattice.nSites;
    this.nLefs = this.#inner.n_lefs();
  }

  /** Set while a {@link Sim.stream} is open: only the stream may drive. */
  #streaming = false;

  get #sim(): WasmSim {
    if (!this.#inner) throw new Error("this Sim has been freed");
    return this.#inner;
  }

  /** The wasm sim, for calls that advance it. */
  get #driver(): WasmSim {
    if (this.#streaming) throw new Error("this Sim is being streamed; finish or cancel the stream first");
    return this.#sim;
  }

  get nLegs(): number {
    return 2 * this.nLefs;
  }

  /** Current simulation time. */
  get time(): number {
    return this.#sim.time();
  }

  /** Total events fired so far. */
  get nEvents(): number {
    return this.#sim.n_events();
  }

  /** Time of the next event, or `null` if none remain. */
  get nextEventTime(): number | null {
    return this.#sim.next_event_time() ?? null;
  }

  get rates(): Rates {
    return this.#sim.rates() as Rates;
  }

  /** The current state as a {@link World}. Arrays are copies, safe to keep. */
  world(): World {
    return { lattice: this.lattice, lefs: this.#sim.lefs() as World["lefs"] };
  }

  /**
   * The current state as a {@link Model}: the world now, with this run's
   * rates and spawn rule. `new Sim(sim.model(), { seed, startTime: sim.time })`
   * continues from here with fresh randomness; {@link Sim.clone} continues
   * exactly.
   */
  model(): Model {
    return { world: this.world(), rates: this.rates, spawnRule: this.spawnRule };
  }

  /**
   * The model this run started from. With {@link Sim.seed} and
   * {@link Sim.startTime}, it says where a recording came from:
   * `new Sim(sim.initialModel(), { seed: sim.seed, startTime: sim.startTime })`
   * replays the run from its start, event for event.
   */
  initialModel(): Model {
    const lefs = this.#sim.initial_lefs() as World["lefs"];
    return { world: { lattice: this.lattice, lefs }, rates: this.rates, spawnRule: this.spawnRule };
  }

  /** The seed this run was started with. */
  get seed(): bigint {
    return this.#sim.seed();
  }

  /** The clock at construction. */
  get startTime(): number {
    return this.#sim.start_time();
  }

  /** An exact fork: a copy that continues the run as this one would. */
  clone(): Sim {
    const model = { world: { lattice: this.lattice }, spawnRule: this.spawnRule } as Model;
    return new Sim(model, { [FORK]: this.#sim.fork() } as SimOptions);
  }

  /** Fire one event, or return `null` if none remain. */
  step(): FiredEvent | null {
    const fired = this.#driver.step() as [number, number, number] | null;
    if (!fired) return null;
    const [time, code, id] = fired;
    return { time, action: ROW_KIND[code] as FiredEvent["action"], id };
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
    return this.#driver.run_until(tEnd, maxEvents) as RunReport;
  }

  /** Fire up to `n` events. */
  runEvents(n: number): RunReport {
    return this.#driver.run_events(checkCount(n, "n")) as RunReport;
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
    const r = this.#driver.record_until(tEnd, maxEvents, options.dt, options.t0 ?? 0, options.events ?? false, options.keyframes ?? false);
    return withEventEnd(r as Recording);
  }

  /** {@link Sim.runEvents}, recording as in {@link Sim.recordUntil}. */
  recordEvents(n: number, options: RecordOptions = {}): Recording {
    const r = this.#driver.record_events(checkCount(n, "n"), options.dt, options.t0 ?? 0, options.events ?? false, options.keyframes ?? false);
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
    return this.#driver.record_snapshot_chunk(tEnd, options.dt, options.t0 ?? 0, maxSnapshots, maxEvents) as SnapshotChunk;
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
    return this.#driver.record_event_chunk(tEnd, maxRows, options.keyframes ?? true) as EventChunk;
  }

  /**
   * Stream the run to `tEnd` as a `ReadableStream` of chunks, computed on
   * demand: the sim advances only when the consumer reads, which gives
   * backpressure for free. Pass exactly one of:
   *
   * - `dt` (optional `t0`): {@link SnapshotChunk}s of at most
   *   `maxSnapshots` snapshots (default 1,000), each call firing at most
   *   `maxEvents`;
   * - `events: true` (optional `keyframes`, default true):
   *   {@link EventChunk}s of about `maxRows` rows (default 10,000).
   *
   * While the stream is open, calls that advance this Sim throw; read-only
   * ones (`time`, `state()`, `world()`) still work. Finishing or cancelling
   * the stream releases it. Runs on the calling thread, one bounded chunk
   * per read. To read with `for await` everywhere (including Safari), use
   * {@link iterate}. For Arrow record batches, pipe it through
   * `toRecordBatches()` from `lefsim/arrow`.
   */
  stream(tEnd: number, options: { dt: number; t0?: number; maxSnapshots?: number; maxEvents?: number }): ReadableStream<SnapshotChunk>;
  stream(tEnd: number, options: { events: true; keyframes?: boolean; maxRows?: number }): ReadableStream<EventChunk>;
  stream(
    tEnd: number,
    options:
      | { dt: number; t0?: number; maxSnapshots?: number; maxEvents?: number }
      | { events: true; keyframes?: boolean; maxRows?: number },
  ): ReadableStream<SnapshotChunk | EventChunk> {
    if (Number.isNaN(tEnd)) throw new RangeError("tEnd is NaN");
    const isEvents = "events" in options && options.events === true;
    const hasDt = "dt" in options && options.dt !== undefined;
    if (isEvents === hasDt) throw new RangeError("pass exactly one of dt or events: true");
    void this.#driver; // throws if freed or already streaming
    let next: () => SnapshotChunk | EventChunk;
    if (isEvents) {
      if ("maxSnapshots" in options) throw new RangeError("maxSnapshots applies to dt; use maxRows with events: true");
      const keyframes = options.keyframes ?? true;
      const maxRows = checkCount(options.maxRows ?? 10_000, "maxRows");
      const min = keyframes ? this.nLegs + 2 : 2;
      if (maxRows < min) throw new RangeError(`maxRows must be at least ${min}${keyframes ? " with keyframes" : ""}, got ${maxRows}`);
      next = () => this.#sim.record_event_chunk(tEnd, maxRows, keyframes) as EventChunk;
    } else {
      if ("maxRows" in options) throw new RangeError("maxRows applies to events: true; use maxSnapshots with dt");
      const { dt, t0 = 0, maxEvents = 250_000 } = options as { dt: number; t0?: number; maxEvents?: number };
      const maxSnapshots = checkCount((options as { maxSnapshots?: number }).maxSnapshots ?? 1_000, "maxSnapshots");
      if (maxSnapshots < 1) throw new RangeError("maxSnapshots must be positive");
      checkCount(maxEvents, "maxEvents");
      next = () => this.#sim.record_snapshot_chunk(tEnd, dt, t0, maxSnapshots, maxEvents) as SnapshotChunk;
    }

    this.#streaming = true;
    const release = () => {
      this.#streaming = false;
    };
    return new ReadableStream<SnapshotChunk | EventChunk>(
      {
        pull: async (controller) => {
          try {
            for (;;) {
              const chunk = next();
              const done = chunk.stopped !== "recorder_full" && chunk.stopped !== "reached_event_count";
              if (chunk.time.length > 0) controller.enqueue(chunk);
              if (done) {
                release();
                controller.close();
                return;
              }
              if (chunk.time.length > 0) return;
              // The event cap ran out before a snapshot was due: yield to the
              // event loop before continuing, so the page stays responsive.
              await new Promise((resolve) => setTimeout(resolve, 0));
            }
          } catch (e) {
            release();
            controller.error(e);
          }
        },
        cancel: release,
      },
      // No read-ahead: compute a chunk only when the consumer asks for one.
      new CountQueuingStrategy({ highWaterMark: 0 }),
    );
  }

  /**
   * The current state of every LEF, as zero-copy views: see
   * {@link LiveState} for the contract. {@link Sim.world} gives a copy.
   */
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

  /** Release the WebAssembly memory. Idempotent. */
  free(): void {
    this.#inner?.free();
    this.#inner = undefined;
  }

  [Symbol.dispose](): void {
    this.free();
  }
}
