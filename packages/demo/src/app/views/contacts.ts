// Approximate contact maps from effective distance, independent of the UI.
//
// Each bin is represented by its middle site. The contact probability of two
// sites follows a Gaussian chain, P = d^(-3/2), where d is their effective
// distance (steps and hops across loops, each of length 1). The maps are the
// current state and its average over simulation time, each frame weighted by
// the time since the previous one.

import { effectiveDistances, type World } from "lefsim";

import { FALL, packedColors } from "../../heatmap.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
/** The float64 high word's index in a Uint32Array view of it. */
const HIGH_WORD = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1 ? 1 : 0;

export class ContactMaps {
  readonly nSites: number;
  /** Sites per bin, and the bins' middle sites. */
  bin = 10;
  centers = new Uint32Array(0);
  /** The latest distances between bins (condensed upper triangle). */
  dist: Float64Array = new Float64Array(0);
  /** Per pair: the sum of P weighted by the simulation time it held, over `averagedTime`. */
  sumP = new Float64Array(0);
  averagedTime = 0;
  /** Packed pixels, row-major `k × k`: the current map above the diagonal, the average below. */
  pixels = new Uint32Array(0);

  #lastTime: number | undefined;
  /** Pixel and P for each effective distance (distances are integers below nSites). */
  #pixelOf = new Uint32Array(0);
  #pOf = new Float64Array(0);
  /** Colormap entries, and a colormap index for any P keyed by the top 20 bits of its float64. */
  #colors = packedColors(FALL);
  #indexOfBits = new Uint8Array(1 << 20);
  #means = new Float64Array(0);
  #meanWords = new Uint32Array(0);

  constructor(nSites: number) {
    this.nSites = nSites;
  }

  get k(): number {
    return this.centers.length;
  }

  /**
   * The color scale. Log: log10 P from the farthest pair of sites to the
   * nearest pair of distinct bins. Linear: P from 0 to that nearest pair's,
   * saturating above it (the diagonal).
   */
  scale: "log" | "linear" = "log";

  get logRange(): [number, number] {
    return [-1.5 * Math.log10(this.nSites - 1), -1.5 * Math.log10(this.bin)];
  }

  /** Manual ends of the color range, as contact probabilities (undefined: automatic). */
  clipMin: number | undefined;
  clipMax: number | undefined;

  /** The automatic color range, in P, for the current scale. */
  get autoRange(): [number, number] {
    const top = this.bin ** -1.5;
    return this.scale === "log" ? [(this.nSites - 1) ** -1.5, top] : [0, top];
  }

  /** The color range in P: the manual ends where set, else the automatic ones. */
  get rangeP(): [number, number] {
    const [lo, hi] = this.autoRange;
    return [this.clipMin ?? lo, this.clipMax ?? hi];
  }

  /** The colorbar's range: log10 P for the log scale, P for the linear one. */
  get range(): [number, number] {
    const [lo, hi] = this.rangeP;
    return this.scale === "log" ? [Math.log10(lo), Math.log10(hi)] : [lo, hi];
  }

  setScale(scale: "log" | "linear") {
    this.scale = scale;
    this.#recolor();
  }

  /**
   * Set the manual ends of the color range (undefined for automatic). False,
   * changing nothing, if the range would be empty, or start at or below 0 on
   * the log scale.
   */
  setClip(min: number | undefined, max: number | undefined): boolean {
    const [autoLo, autoHi] = this.autoRange;
    const lo = min ?? autoLo;
    const hi = max ?? autoHi;
    if (!Number.isFinite(lo) || !Number.isFinite(hi) || !(lo < hi) || lo < 0 || (this.scale === "log" && lo <= 0)) {
      return false;
    }
    this.clipMin = min;
    this.clipMax = max;
    this.#recolor();
    return true;
  }

  #recolor() {
    // A manual minimum of 0 has no log: fall back to the automatic one there.
    if (this.scale === "log" && this.clipMin !== undefined && this.clipMin <= 0) this.clipMin = undefined;
    this.#buildTables();
    if (this.dist.length) this.#paint();
  }

  /** Colormap index (0 to 255) for a contact probability, on the current scale. */
  #colorIndex(p: number): number {
    if (!(p > 0) || !Number.isFinite(p)) return 0;
    const [lo, hi] = this.range;
    const c = this.scale === "log" ? ((Math.log10(p) - lo) / (hi - lo)) * 255 : (p / hi) * 255;
    return c < 0 ? 0 : c > 255 ? 255 : Math.round(c);
  }

  setBin(bin: number) {
    this.bin = bin;
    const k = Math.ceil(this.nSites / bin);
    this.centers = Uint32Array.from({ length: k }, (_, i) => Math.min(this.nSites - 1, Math.floor(i * bin + bin / 2)));
    this.pixels = new Uint32Array(k * k);
    this.dist = new Float64Array(0);
    this.#buildTables();
    this.resetAverage(this.#lastTime);
  }

  resetAverage(time?: number) {
    const n = (this.k * (this.k - 1)) / 2;
    this.sumP = new Float64Array(n);
    this.#means = new Float64Array(n);
    this.#meanWords = new Uint32Array(this.#means.buffer);
    this.averagedTime = 0;
    this.#lastTime = time;
  }

  /** Measure the current state at `time`, add it to the average, and paint. */
  update(world: World, time: number) {
    this.dist = effectiveDistances(world, this.centers, LENGTHS);
    const dt = this.#lastTime === undefined ? 0 : time - this.#lastTime;
    this.#lastTime = time;
    if (dt > 0) {
      const pOf = this.#pOf;
      const top = pOf.length - 1;
      for (let at = 0; at < this.dist.length; at++) this.sumP[at]! += pOf[Math.min(this.dist[at]!, top)]! * dt;
      this.averagedTime += dt;
    }
    this.#paint();
  }

  /** Index of pair `(i, j)`, `i < j`, in the condensed triangle. */
  index(i: number, j: number): number {
    return i * this.k - (i * (i + 1)) / 2 + (j - i - 1);
  }

  /** The effective distance now and P (now, and averaged if any time has passed) of bins `i < j`. */
  pair(i: number, j: number): { d: number; p: number; mean: number | undefined } {
    const at = this.index(i, j);
    const d = this.dist[at] ?? 0;
    return { d, p: d ** -1.5, mean: this.averagedTime > 0 ? this.sumP[at]! / this.averagedTime : undefined };
  }

  #buildTables() {
    this.#pixelOf = new Uint32Array(this.nSites);
    this.#pOf = new Float64Array(this.nSites);
    for (let d = 0; d < this.nSites; d++) {
      // P = d^(-3/2); the diagonal (d = 0) takes the top.
      this.#pOf[d] = d === 0 ? 1 : d ** -1.5;
      this.#pixelOf[d] = this.#colors[d === 0 ? 255 : this.#colorIndex(this.#pOf[d]!)]!;
    }
    // Each key's middle value stands for its range, so coloring an average is a lookup, not a logarithm.
    const value = new Float64Array(1);
    const words = new Uint32Array(value.buffer);
    for (let key = 0; key < 1 << 20; key++) {
      words[HIGH_WORD] = (key << 12) | 0x800;
      words[1 - HIGH_WORD] = 0;
      this.#indexOfBits[key] = this.#colorIndex(value[0]!);
    }
  }

  /** The current map above the diagonal, the average below (the current map until time has passed). */
  #paint() {
    const k = this.k;
    const px = this.pixels;
    const table = this.#pixelOf;
    const top = table.length - 1;
    const averaged = this.averagedTime > 0;
    if (averaged) {
      const inv = 1 / this.averagedTime;
      for (let n = 0; n < this.sumP.length; n++) this.#means[n] = this.sumP[n]! * inv;
    }
    const words = this.#meanWords;
    let at = 0;
    for (let i = 0; i < k; i++) {
      px[i * k + i] = table[0]!;
      for (let j = i + 1; j < k; j++, at++) {
        const now = table[Math.min(this.dist[at] ?? top, top)]!;
        px[i * k + j] = now;
        px[j * k + i] = averaged ? this.#colors[this.#indexOfBits[words[2 * at + HIGH_WORD]! >>> 12]!]! : now;
      }
    }
  }
}

/**
 * Draw a `k × k` map rotated 45° as a triangle hanging from a horizontal axis:
 * the pair of bins `(i, j)` at x halfway between them, and as far down as they
 * are apart. Each canvas pixel looks up its pair, so the image stays sharp at
 * any bin count. `which` picks the half of `pixels` to show (above or below
 * the diagonal).
 *
 * `x0` and `x1` are the axis's ends in canvas pixels. A pair `b - a` apart
 * along the axis sits `(b - a) * yScale` down: 0.5 is the true 45° rotation,
 * and the triangle is squashed below that to fit `height`.
 */
export function drawTriangle(
  ctx: CanvasRenderingContext2D,
  opts: {
    pixels: Uint32Array;
    k: number;
    which: "upper" | "lower";
    x0: number;
    x1: number;
    width: number;
    height: number;
  },
): { yScale: number } {
  const { pixels, k, which, x0, x1, width, height } = opts;
  const w = Math.max(1, Math.round(width));
  const h = Math.max(1, Math.round(height));
  const span = x1 - x0;
  const yScale = Math.min(0.5, h / span);
  const image = ctx.createImageData(w, h);
  const out = new Uint32Array(image.data.buffer);
  for (let py = 0; py < h; py++) {
    const apart = (py + 0.5) / yScale; // how far apart the pair is, in axis pixels
    for (let px = 0; px < w; px++) {
      const a = px + 0.5 - x0 - apart / 2;
      const b = px + 0.5 - x0 + apart / 2;
      if (a < 0 || b >= span) continue;
      const i = Math.min(k - 1, Math.floor((a / span) * k));
      const j = Math.min(k - 1, Math.floor((b / span) * k));
      out[py * w + px] = which === "upper" ? pixels[i * k + j]! : pixels[j * k + i]!;
    }
  }
  ctx.putImageData(image, 0, 0);
  return { yScale };
}

/** The pair of bins `[i, j]` (`i <= j`) under canvas point (`px`, `py`) of a triangle drawn by `drawTriangle`. */
export function triangleCell(
  px: number,
  py: number,
  opts: { k: number; x0: number; x1: number; yScale: number },
): [number, number] | undefined {
  const { k, x0, x1, yScale } = opts;
  const span = x1 - x0;
  const apart = py / yScale;
  const a = px - x0 - apart / 2;
  const b = px - x0 + apart / 2;
  if (a < 0 || b >= span || py < 0) return undefined;
  return [Math.min(k - 1, Math.floor((a / span) * k)), Math.min(k - 1, Math.floor((b / span) * k))];
}
