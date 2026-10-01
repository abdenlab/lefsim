// A live run whose model can be edited while it runs: the CTCFs (moved by
// dragging them in two lanes, one per orientation, or added and removed by
// count), the three rates and the CTCF strength. Shared by the pages that
// offer these controls.
//
// A change builds a new model and carries the LEFs over to it at the current
// time, so the run continues rather than restarting.

import { createLattice, createModel, createWorld, MOBILITY, populate, Sim, type Lattice, type Rates } from "lefsim";

import { N_SITES, RATES } from "./model.ts";
import { colors, setLattice } from "./render.ts";

const MAX_CTCFS = 16;
const CAPTURED = MOBILITY.indexOf("Captured");
const MOVING = MOBILITY.indexOf("Moving");

/** A CTCF; `left` means it captures from the left (its chevron points left). */
export interface CtcfMark {
  site: number;
  left: boolean;
}

/** The inputs an editable run reads, by role. */
export interface ModelInputs {
  nLeft: HTMLInputElement;
  nRight: HTMLInputElement;
  step: HTMLInputElement;
  respawn: HTMLInputElement;
  respawnHeld: HTMLInputElement;
  strength: HTMLInputElement;
}

export class EditableRun {
  sim: Sim | undefined;
  ctcfs: CtcfMark[];
  rates: Rates = { ...RATES };
  strength = 0.8;
  /** Called after a change from the inputs has replaced the sim. */
  onChange: () => void = () => {};
  #inputs: ModelInputs;
  #seed = 0;
  /** Rebuilds since the last restart; varies the continued run's seed. */
  #rebuilds = 0;

  constructor(ctcfs: CtcfMark[], inputs: ModelInputs) {
    this.ctcfs = ctcfs.map((c) => ({ ...c }));
    this.#inputs = inputs;
    // The inputs start from the model, whatever the markup says.
    inputs.nLeft.value = String(this.ctcfs.filter((c) => c.left).length);
    inputs.nRight.value = String(this.ctcfs.filter((c) => !c.left).length);
    inputs.step.value = String(this.rates.step);
    inputs.respawn.value = String(this.rates.respawn);
    inputs.respawnHeld.value = String(this.rates.respawnCaptured);
    inputs.strength.value = String(this.strength);
    for (const input of [inputs.step, inputs.respawn, inputs.respawnHeld, inputs.strength]) {
      input.addEventListener("change", () => {
        this.#readParams();
        this.rebuild();
        this.onChange();
      });
    }
    inputs.nLeft.addEventListener("change", () => this.#setCount(true, inputs.nLeft));
    inputs.nRight.addEventListener("change", () => this.#setCount(false, inputs.nRight));
  }

  lattice(): Lattice {
    return createLattice(
      N_SITES,
      this.ctcfs.map((c) => ({ site: c.site, capturesFrom: c.left ? "left" : "right", strength: this.strength })),
    );
  }

  /** A new run: `nLefs` LEFs placed with `seed`, and run with it, on the current model. */
  restart(nLefs: number, seed: number) {
    this.sim?.free();
    this.#seed = seed;
    this.#rebuilds = 0;
    this.#readParams();
    const lattice = this.lattice();
    this.sim = new Sim(createModel(populate(lattice, nLefs, { seed }), this.rates), { seed });
    setLattice(lattice);
  }

  /**
   * The same LEFs on the current model, continuing from the current time. A
   * captured leg stays captured only if a CTCF that can hold it is still at
   * its site (a left leg is held by one capturing from the right, a right leg
   * by one capturing from the left).
   */
  rebuild() {
    const old = this.sim;
    if (!old) return;
    // Copy the state first: the next call into wasm can detach its views.
    const st = old.state();
    const time = old.time;
    const leftPos = st.leftPos.slice();
    const rightPos = st.rightPos.slice();
    const leftMob = st.leftMobility.slice();
    const rightMob = st.rightMobility.slice();
    const capturesLeftAt = new Map(this.ctcfs.map((c) => [c.site, c.left]));
    const keep = (mob: number, site: number, needsLeft: boolean) =>
      mob === CAPTURED && capturesLeftAt.get(site) === needsLeft ? CAPTURED : MOVING;
    const lattice = this.lattice();
    const world = createWorld(lattice, {
      leftPos,
      rightPos,
      leftMobility: Array.from(leftMob, (m, i) => keep(m, leftPos[i]!, false)),
      rightMobility: Array.from(rightMob, (m, i) => keep(m, rightPos[i]!, true)),
    });
    this.sim = new Sim(createModel(world, this.rates), { seed: this.#seed + ++this.#rebuilds, startTime: time });
    old.free();
    setLattice(lattice);
  }

  /** Move CTCF `i` to `site` and rebuild; false if another CTCF is there. */
  moveCtcf(i: number, site: number): boolean {
    if (this.ctcfs.some((c) => c.site === site)) return false;
    this.ctcfs[i]!.site = site;
    this.rebuild();
    return true;
  }

  /**
   * Read the rates and strength from their inputs. A value the model can't
   * take (a rate that isn't positive, a strength outside [0, 1]) puts the
   * input back to the current value.
   */
  #readParams() {
    const { step, respawn, respawnHeld, strength } = this.#inputs;
    const positive = (input: HTMLInputElement, current: number) => {
      const v = Number(input.value);
      if (input.value.trim() !== "" && Number.isFinite(v) && v > 0) return v;
      input.value = String(current);
      return current;
    };
    this.rates = {
      step: positive(step, this.rates.step),
      respawn: positive(respawn, this.rates.respawn),
      respawnCaptured: positive(respawnHeld, this.rates.respawnCaptured),
    };
    const st = Number(strength.value);
    if (strength.value.trim() !== "" && st >= 0 && st <= 1) this.strength = st;
    else strength.value = String(this.strength);
  }

  /** Add or remove CTCFs of one orientation to match its count input. */
  #setCount(left: boolean, input: HTMLInputElement) {
    const n = Math.max(0, Math.min(MAX_CTCFS, Math.floor(Number(input.value) || 0)));
    input.value = String(n);
    let lane = this.ctcfs.filter((c) => c.left === left);
    while (lane.length > n) {
      const last = lane.at(-1)!;
      this.ctcfs = this.ctcfs.filter((c) => c !== last);
      lane = lane.slice(0, -1);
    }
    while (lane.length < n) {
      // A new CTCF goes in the middle of the widest stretch without one.
      const sites = [-1, ...this.ctcfs.map((c) => c.site).sort((a, b) => a - b), N_SITES];
      let best = 0;
      for (let i = 1; i < sites.length - 1; i++) if (sites[i + 1]! - sites[i]! > sites[best + 1]! - sites[best]!) best = i;
      const c = { site: Math.round((sites[best]! + sites[best + 1]!) / 2), left };
      this.ctcfs.push(c);
      lane.push(c);
    }
    this.rebuild();
    this.onChange();
  }
}

/**
 * Two CTCF lanes on a canvas, from `top` down: CTCFs capturing from the left,
 * then from the right. Draws them and finds the one under the pointer; the
 * page owns the pointer events, since its canvas may hold other tracks.
 */
export class CtcfLanes {
  static readonly HEIGHT = 52;
  hovered = -1;
  dragging = -1;
  #run: EditableRun;
  #top: number;

  constructor(run: EditableRun, top: number) {
    this.#run = run;
    this.#top = top;
  }

  #row(left: boolean) {
    const top = this.#top + (left ? 0 : CtcfLanes.HEIGHT / 2);
    return { top, bottom: top + CtcfLanes.HEIGHT / 2 };
  }

  /** Whether canvas y `my` is in the lanes. */
  contains(my: number): boolean {
    return my >= this.#top && my < this.#top + CtcfLanes.HEIGHT;
  }

  draw(ctx: CanvasRenderingContext2D, x: (site: number) => number) {
    // A faint band per lane; the chevrons in it say which way it faces.
    for (const left of [true, false]) {
      const row = this.#row(left);
      ctx.fillStyle = colors.band;
      ctx.fillRect(x(0), row.top + 2, x(N_SITES - 1) - x(0), row.bottom - row.top - 4);
    }
    // Chevrons, pointing toward the side each captures from.
    this.#run.ctcfs.forEach((c, i) => {
      const row = this.#row(c.left);
      const cy = (row.top + row.bottom) / 2;
      const size = i === this.hovered || i === this.dragging ? 16 : 12;
      const dir = c.left ? -1 : 1;
      const px = x(c.site);
      ctx.beginPath();
      ctx.moveTo(px + dir * size * 0.6, cy);
      ctx.lineTo(px, cy - size / 2);
      ctx.lineTo(px, cy + size / 2);
      ctx.closePath();
      ctx.fillStyle = colors.ink2;
      ctx.fill();
    });
  }

  /** The CTCF under canvas point (`mx`, `my`), as an index into the run's CTCFs, or -1. */
  at(mx: number, my: number, x: (site: number) => number): number {
    if (!this.contains(my)) return -1;
    const left = my < this.#row(true).bottom;
    let best = -1;
    let bestPx = 12;
    this.#run.ctcfs.forEach((c, i) => {
      if (c.left !== left) return;
      const px = Math.abs(x(c.site) - mx);
      if (px < bestPx) [best, bestPx] = [i, px];
    });
    return best;
  }
}
