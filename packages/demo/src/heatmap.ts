// Heatmaps shared by the map pages: colormaps, colorbars, and a square map
// drawn under the loop panel on the same x axis.

import { colors, fit, MARGIN } from "./render.ts";

/** A 256-entry RGB table, interpolated evenly between `stops`. */
function colormap(stops: number[][]): Uint8Array {
  const lut = new Uint8Array(256 * 3);
  for (let i = 0; i < 256; i++) {
    const t = (i / 255) * (stops.length - 1);
    const k = Math.min(Math.floor(t), stops.length - 2);
    const f = t - k;
    for (let c = 0; c < 3; c++) lut[3 * i + c] = Math.round(stops[k]![c]! * (1 - f) + stops[k + 1]![c]! * f);
  }
  return lut;
}

/** "Fall": white through yellow, orange and red to black. */
export const FALL = colormap([
  [255, 255, 255],
  [255, 255, 204],
  [255, 237, 160],
  [254, 217, 118],
  [254, 178, 76],
  [253, 141, 60],
  [252, 78, 42],
  [227, 26, 28],
  [189, 0, 38],
  [128, 0, 38],
  [0, 0, 0],
]);

/** Blues: white through light and mid blue to navy. */
export const BLUES = colormap([
  [255, 255, 255],
  [222, 235, 247],
  [198, 219, 239],
  [158, 202, 225],
  [107, 174, 214],
  [66, 146, 198],
  [33, 113, 181],
  [8, 81, 156],
  [8, 48, 107],
  [4, 22, 52],
]);

const LITTLE_ENDIAN = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

/** An RGB color packed as an opaque pixel: the image's 4 bytes read as one 32-bit word. */
export function packPixel(r: number, g: number, b: number): number {
  return LITTLE_ENDIAN
    ? ((255 << 24) | (b << 16) | (g << 8) | r) >>> 0
    : ((r << 24) | (g << 16) | (b << 8) | 255) >>> 0;
}

/** A colormap's 256 entries as packed pixels. */
export function packedColors(lut: Uint8Array): Uint32Array {
  return Uint32Array.from({ length: 256 }, (_, c) => packPixel(lut[3 * c]!, lut[3 * c + 1]!, lut[3 * c + 2]!));
}

export function superscript(n: number): string {
  const digits = "⁰¹²³⁴⁵⁶⁷⁸⁹";
  return (
    (n < 0 ? "⁻" : "") +
    String(Math.abs(n))
      .split("")
      .map((d) => digits[Number(d)])
      .join("")
  );
}

/**
 * A horizontal colorbar for a log10 scale from `lo` to `hi`, `width` wide
 * with its left end at `x0`, ticked at each power of ten, and `label` to its
 * left.
 */
export function drawColorbar(
  ctx: CanvasRenderingContext2D,
  opts: { x0: number; y0: number; width: number; lut: Uint8Array; lo: number; hi: number; label: string },
) {
  const { x0, y0, width, lut, lo, hi, label } = opts;
  for (let i = 0; i < width; i++) {
    const c = Math.round((i / (width - 1)) * 255);
    ctx.fillStyle = `rgb(${lut[3 * c]}, ${lut[3 * c + 1]}, ${lut[3 * c + 2]})`;
    ctx.fillRect(x0 + i, y0, 1, 10);
  }
  ctx.strokeStyle = colors.rule;
  ctx.lineWidth = 1;
  ctx.strokeRect(x0 + 0.5, y0 + 0.5, width - 1, 9);
  ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
  ctx.fillStyle = colors.muted;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  if (hi > lo) {
    for (let e = Math.ceil(lo); e <= Math.floor(hi); e++) {
      const tx = x0 + ((e - lo) / (hi - lo)) * (width - 1);
      ctx.fillRect(tx, y0 + 10, 1, 3);
      ctx.fillText(e === 0 ? "1" : `10${superscript(e)}`, tx, y0 + 15);
    }
  }
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  ctx.fillText(label, x0 - 8, y0 + 5);
}

/**
 * A square `k × k` heatmap on a canvas, aligned with the loop panel's
 * lattice axis (same side margins). Paint `pixels` (row-major packed
 * pixels), then `draw`.
 */
export class SquareMap {
  #canvas: HTMLCanvasElement;
  #ctx: CanvasRenderingContext2D;
  #image: ImageData | undefined;
  #offscreen = document.createElement("canvas");

  constructor(canvas: HTMLCanvasElement) {
    this.#canvas = canvas;
    this.#ctx = fit(canvas);
  }

  /** Bins per side. */
  get k(): number {
    return this.#image?.width ?? 0;
  }

  setSize(k: number) {
    this.#image = new ImageData(k, k);
    this.#offscreen.width = k;
    this.#offscreen.height = k;
  }

  get pixels(): Uint32Array {
    return new Uint32Array(this.#image!.data.buffer);
  }

  resize() {
    this.#ctx = fit(this.#canvas);
    this.draw();
  }

  /** The map's square in CSS pixels. */
  box() {
    const { width } = this.#canvas.getBoundingClientRect();
    return { left: MARGIN.left, top: 0, size: width - MARGIN.left - MARGIN.right };
  }

  draw() {
    const { width, height } = this.#canvas.getBoundingClientRect();
    const ctx = this.#ctx;
    ctx.clearRect(0, 0, width, height);
    if (!this.#image) return;
    this.#offscreen.getContext("2d")!.putImageData(this.#image, 0, 0);
    const { left, top, size } = this.box();
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(this.#offscreen, left, top, size, size);
    ctx.strokeStyle = colors.rule;
    ctx.lineWidth = 1;
    ctx.strokeRect(left + 0.5, top + 0.5, size - 1, size - 1);
  }

  /** The cell under a pointer event, as `[row, col]`, or undefined outside the map. */
  cellAt(e: PointerEvent): [number, number] | undefined {
    const rect = this.#canvas.getBoundingClientRect();
    const { left, top, size } = this.box();
    const k = this.k;
    const col = Math.floor(((e.clientX - rect.left - left) / size) * k);
    const row = Math.floor(((e.clientY - rect.top - top) / size) * k);
    return col < 0 || row < 0 || col >= k || row >= k ? undefined : [row, col];
  }
}

/** Index of pair `(i, j)`, `i < j`, in a condensed upper triangle of `k` items. */
export function condensedIndex(i: number, j: number, k: number): number {
  return i * k - (i * (i + 1)) / 2 + (j - i - 1);
}
