// The app's one simulation: its model inputs, the Sim, and the frame loop.
//
// Model edits (CTCFs, rates, strength, the number of LEFs) build a new model
// and carry the LEFs over at the current time, so the run continues rather
// than restarting. Restart places the LEFs afresh with the seed.
//
// The visible view drives what each frame does (see `ViewDriver`): it may ask
// for samples at fixed simulation times, independent of playback speed.

import { createLattice, createModel, createWorld, type Lattice, MOBILITY, populate, type Rates, Sim } from "lefsim";

import { type Frame, setLattice, snapshotOf } from "../render.ts";

export const N_SITES = 3000;
export const MAX_LEFS = 700;
const CAPTURED = MOBILITY.indexOf("Captured");
const MOVING = MOBILITY.indexOf("Moving");
/** Cap on events per frame when no view samples, so a burst can't stall the page. */
const MAX_EVENTS_PER_FRAME = 40_000;
/** Wall-clock time per frame for a view's sampling, in ms; past it, playback slows. */
const SAMPLE_BUDGET_MS = 10;

/** A CTCF; `left` means it captures from the left (its chevron points left). */
export interface Ctcf {
  site: number;
  left: boolean;
}

/** The starting CTCFs: `>` `>` `<`, a domain with a second right-facing CTCF inside it. */
export const DEFAULT_CTCFS: readonly Ctcf[] = [
  { site: 750, left: false },
  { site: 1500, left: false },
  { site: 2250, left: true },
];

export const DEFAULT_RATES: Readonly<Rates> = { step: 1, respawn: 0.002, respawnBlocked: 0.02, respawnCaptured: 0.002 };

/** What the visible view asks of the run each frame. */
export interface ViewDriver {
  /** Sample every `sampleDt` of simulation time, by calling `sample` once the sim has reached each sample time. */
  sampleDt?: number;
  sample?(time: number, sim: Sim): void;
  /** Once per animation frame (and after any change while paused), with the current state. */
  frame?(frame: Frame, sim: Sim): void;
  /** The model was edited ("edit") or the run restarted ("restart"). */
  changed?(kind: "edit" | "restart"): void;
}

/** Markers a view draws on the stage, which can be dragged along the lattice. */
export interface StageMarkers {
  sites: number[];
  move(index: number, site: number): void;
}

export class Run {
  // Model inputs.
  nLefs = $state(20);
  seed = $state(1);
  strength = $state(0.8);
  rates = $state<Rates>({ ...DEFAULT_RATES });
  ctcfs = $state<Ctcf[]>(DEFAULT_CTCFS.map((c) => ({ ...c })));

  // Playback.
  playing = $state(false);
  /** Playback speed as log10 of simulation time units per second. */
  speedLog = $state(2);

  // Readouts.
  frame = $state.raw<Frame | undefined>(undefined);
  eventsPerSecond = $state(0);
  /** The last frame couldn't keep up with the speed (event cap or sampling budget). */
  behind = $state(false);
  /** Bumped on every model change and restart, so the stage redraws. */
  revision = $state(0);

  /** Set by the visible view. */
  markers = $state.raw<StageMarkers | undefined>(undefined);

  sim: Sim | undefined;
  #driver: ViewDriver | undefined;
  #nextSample = 0;
  #rebuilds = 0;
  #lastNow: number | undefined;
  #raf = 0;
  #rateWindow: { t: number; events: number }[] = [];

  get speed(): number {
    return Math.round(10 ** this.speedLog);
  }

  get time(): number {
    return this.frame?.time ?? 0;
  }

  get held(): number {
    return this.frame ? this.frame.held.reduce((a, h) => a + h, 0) : 0;
  }

  lattice(): Lattice {
    return createLattice(
      N_SITES,
      this.ctcfs.map((c) => ({ site: c.site, capturesFrom: c.left ? "left" : "right", strength: this.strength })),
    );
  }

  /** A new run: the LEFs placed afresh with the seed, on the current model. */
  restart() {
    this.sim?.free();
    this.#rebuilds = 0;
    const lattice = this.lattice();
    const nLefs = Math.max(0, Math.min(MAX_LEFS, Math.round(this.nLefs)));
    this.sim = new Sim(createModel(populate(lattice, nLefs, { seed: this.seed }), { ...this.rates }), {
      seed: this.seed,
    });
    setLattice(lattice);
    this.#rateWindow = [];
    this.#nextSample = this.sim.time + (this.#driver?.sampleDt ?? 0);
    this.revision++;
    this.#driver?.changed?.("restart");
    this.#publish();
  }

  /**
   * The same LEFs on the current model, continuing from the current time, with
   * LEFs added or dropped to match `nLefs`. A captured leg stays captured only
   * if a CTCF that can hold it is still at its site.
   */
  rebuild() {
    const old = this.sim;
    if (!old) return;
    // Copy the state first: the next call into wasm can detach its views.
    const st = old.state();
    const time = old.time;
    let leftPos = Array.from(st.leftPos);
    let rightPos = Array.from(st.rightPos);
    const leftMob = Array.from(st.leftMobility);
    const rightMob = Array.from(st.rightMobility);
    const capturesLeftAt = new Map(this.ctcfs.map((c) => [c.site, c.left]));
    const captured = (mob: number, site: number, leftLeg: boolean) =>
      mob === CAPTURED && capturesLeftAt.get(site) === !leftLeg ? CAPTURED : MOVING;
    let leftMobility = leftMob.map((m, i) => captured(m, leftPos[i]!, true));
    let rightMobility = rightMob.map((m, i) => captured(m, rightPos[i]!, false));

    // Match the LEF count: drop from the end, or add LEFs at free plain sites.
    const n = Math.max(0, Math.min(MAX_LEFS, Math.round(this.nLefs)));
    if (n < leftPos.length) {
      leftPos = leftPos.slice(0, n);
      rightPos = rightPos.slice(0, n);
      leftMobility = leftMobility.slice(0, n);
      rightMobility = rightMobility.slice(0, n);
    } else if (n > leftPos.length) {
      const taken = new Set([...leftPos, ...rightPos, ...this.ctcfs.map((c) => c.site)]);
      while (leftPos.length < n && taken.size < N_SITES) {
        const s = Math.floor(Math.random() * N_SITES);
        if (taken.has(s)) continue;
        taken.add(s);
        leftPos.push(s);
        rightPos.push(s);
        leftMobility.push(MOVING);
        rightMobility.push(MOVING);
      }
    }

    const lattice = this.lattice();
    const world = createWorld(lattice, { leftPos, rightPos, leftMobility, rightMobility });
    this.sim = new Sim(createModel(world, { ...this.rates }), { seed: this.seed + ++this.#rebuilds, startTime: time });
    old.free();
    setLattice(lattice);
    this.revision++;
    this.#driver?.changed?.("edit");
    this.#publish();
  }

  // ---------------------------------------------------------------- edits

  /** Add a CTCF at `site`; false if one is there already. */
  addCtcf(site: number, left: boolean): Ctcf | undefined {
    if (this.ctcfs.some((c) => c.site === site)) return undefined;
    this.ctcfs.push({ site, left });
    this.rebuild();
    return this.ctcfs.at(-1);
  }

  /** Move CTCF `i` to `site`; false if another CTCF is there. */
  moveCtcf(i: number, site: number): boolean {
    if (this.ctcfs[i]?.site === site) return true;
    if (this.ctcfs.some((c) => c.site === site)) return false;
    this.ctcfs[i]!.site = site;
    this.rebuild();
    return true;
  }

  removeCtcf(i: number) {
    this.ctcfs.splice(i, 1);
    this.rebuild();
  }

  clearCtcfs() {
    this.ctcfs = [];
    this.rebuild();
  }

  /** Set the rates; a value that isn't positive and finite is ignored. */
  setRates(rates: Partial<Rates>) {
    for (const [k, v] of Object.entries(rates) as [keyof Rates, number][]) {
      if (Number.isFinite(v) && v > 0) this.rates[k] = v;
    }
    this.rebuild();
  }

  setStrength(s: number) {
    if (!(s >= 0 && s <= 1)) return;
    this.strength = s;
    this.rebuild();
  }

  setLefCount(n: number) {
    this.nLefs = Math.max(0, Math.min(MAX_LEFS, Math.round(n)));
    this.rebuild();
  }

  // ---------------------------------------------------------------- views

  /** The visible view's driver (or none). */
  setDriver(driver: ViewDriver | undefined) {
    this.#driver = driver;
    if (this.sim && driver?.sampleDt) this.#nextSample = this.sim.time + driver.sampleDt;
    if (this.sim && this.frame) driver?.frame?.(this.frame, this.sim);
  }

  // ---------------------------------------------------------------- playback

  setPlaying(on: boolean) {
    this.playing = on;
    this.#lastNow = undefined;
    this.#rateWindow = [];
    if (on && !this.#raf) this.#raf = requestAnimationFrame(this.#tick);
  }

  #tick = (now: number) => {
    this.#raf = 0;
    const sim = this.sim;
    if (!this.playing || !sim) return;
    const dt = this.#lastNow === undefined ? 0 : Math.min((now - this.#lastNow) / 1000, 0.1);
    this.#lastNow = now;
    const target = sim.time + this.speed * dt;
    const d = this.#driver;
    if (d?.sampleDt && d.sample) {
      // Sample at fixed simulation times, within the frame's budget; if it
      // runs out, stop at the last sample so the samples never skip time.
      const deadline = performance.now() + SAMPLE_BUDGET_MS;
      let behind = false;
      while (this.#nextSample <= target) {
        if (performance.now() > deadline) {
          behind = true;
          break;
        }
        sim.runUntil(this.#nextSample);
        d.sample(this.#nextSample, sim);
        this.#nextSample += d.sampleDt;
      }
      if (!behind) sim.runUntil(target);
      this.behind = behind;
    } else {
      const report = sim.runUntil(target, { maxEvents: MAX_EVENTS_PER_FRAME });
      this.behind = report.stopped === "reached_event_count";
    }
    this.#publish(now);
    this.#raf = requestAnimationFrame(this.#tick);
  };

  /** Copy the current state into `frame`, update the readouts, and hand it to the view. */
  #publish(now = performance.now()) {
    const sim = this.sim;
    if (!sim) return;
    const frame = snapshotOf(sim.state());
    this.frame = frame;
    this.#rateWindow.push({ t: now, events: sim.nEvents });
    while (this.#rateWindow.length > 2 && now - this.#rateWindow[0]!.t > 1000) this.#rateWindow.shift();
    if (this.#rateWindow.length > 1) {
      const a = this.#rateWindow[0]!;
      const b = this.#rateWindow.at(-1)!;
      this.eventsPerSecond = b.t > a.t ? ((b.events - a.events) / (b.t - a.t)) * 1000 : 0;
    }
    this.#driver?.frame?.(frame, sim);
  }
}
