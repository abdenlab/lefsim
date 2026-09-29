// Drawing shared by the live and replay pages.
//
// Views take a `Frame`: a copy of a LiveState. Whether that state came from a
// Sim (live) or a Replayer (replay) is invisible here, which is the point.

import { MOBILITY, type Lattice, type LiveState } from "lefsim";

export const fmt = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });
export const fmtCompact = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });

export const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ------------------------------------------------------------------ theme

// `?theme=light` or `?theme=dark` overrides the OS setting.
const themeParam = new URLSearchParams(location.search).get("theme");
if (themeParam === "light" || themeParam === "dark") document.documentElement.dataset.theme = themeParam;

/** Colors come from CSS custom properties, so they follow the theme. */
function palette() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    paper: v("--paper"),
    ink: v("--ink"),
    ink2: v("--ink-2"),
    muted: v("--muted"),
    rule: v("--rule"),
    band: v("--band"),
    extruding: v("--extruding"),
    held: v("--held"),
  };
}

export let colors = palette();

/** Refresh colors when the OS theme changes, then call `redraw`. */
export function onThemeChange(redraw: () => void) {
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    colors = palette();
    redraw();
  });
}

// ------------------------------------------------------------------ lattice

/** What the views need to know about the lattice being drawn. */
interface Geometry {
  nSites: number;
  ctcfs: { site: number; left: boolean }[];
  /** TAD boundaries: one per cluster of neighboring CTCFs (e.g. a convergent pair). */
  boundaries: number[];
  /** Loop span drawn at full panel height: the largest TAD. */
  fullHeightSpan: number;
}

let geometry: Geometry = { nSites: 2, ctcfs: [], boundaries: [], fullHeightSpan: 1 };

/** Set the lattice every view draws (call before drawing, and on change). */
export function setLattice(lattice: Lattice) {
  const { site, capturesFrom } = lattice.ctcfs;
  const ctcfs = Array.from(site, (s, i) => ({ site: s, left: capturesFrom[i] === 0 })).sort((a, b) => a.site - b.site);
  // CTCFs within a couple of sites of each other form one boundary.
  const boundaries: number[] = [];
  let cluster: number[] = [];
  const close = () => {
    if (cluster.length) boundaries.push((cluster[0]! + cluster.at(-1)!) / 2);
    cluster = [];
  };
  for (const c of ctcfs) {
    if (cluster.length && c.site - cluster.at(-1)! > 2) close();
    cluster.push(c.site);
  }
  close();
  const edges = [0, ...boundaries, lattice.nSites - 1];
  const largest = Math.max(...edges.slice(1).map((e, i) => e - edges[i]!));
  geometry = { nSites: Math.max(2, lattice.nSites), ctcfs, boundaries, fullHeightSpan: Math.max(10, largest) };
}

/** The lattice size the views are drawing. */
export function nSites(): number {
  return geometry.nSites;
}

// ------------------------------------------------------------------ geometry

/** Size a canvas's backing store to its CSS box at device resolution. */
export function fit(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const dpr = window.devicePixelRatio || 1;
  const { width, height } = canvas.getBoundingClientRect();
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

export const MARGIN = { left: 8, right: 8, bottom: 30, top: 10 };

/** Lattice site → x pixel, shared by every panel so they line up. */
export function xScale(width: number) {
  const inner = width - MARGIN.left - MARGIN.right;
  return (site: number) => MARGIN.left + (site / (geometry.nSites - 1)) * inner;
}

/** Pointer x → nearest lattice site. */
export function siteAt(canvas: HTMLCanvasElement, clientX: number): number {
  const rect = canvas.getBoundingClientRect();
  const inner = rect.width - MARGIN.left - MARGIN.right;
  return ((clientX - rect.left - MARGIN.left) / inner) * (geometry.nSites - 1);
}

/** TADs as alternating bands between `top` and `bottom`. */
export function drawTadBands(ctx: CanvasRenderingContext2D, x: (site: number) => number, top: number, bottom: number) {
  const edges = [0, ...geometry.boundaries, geometry.nSites - 1];
  ctx.fillStyle = colors.band;
  for (let i = 0; i < edges.length - 1; i += 2) {
    ctx.fillRect(x(edges[i]!), top, x(edges[i + 1]!) - x(edges[i]!), bottom - top);
  }
}

/** CTCF chevrons (pointing toward the side each captures from), `size` tall, from `top`. */
export function drawCtcfs(ctx: CanvasRenderingContext2D, x: (site: number) => number, top: number, size: number) {
  ctx.fillStyle = colors.ink2;
  for (const c of geometry.ctcfs) {
    const dir = c.left ? -1 : 1;
    const cx = x(c.site) + dir * 2;
    ctx.beginPath();
    ctx.moveTo(cx + dir * size * 0.6, top + size / 2);
    ctx.lineTo(cx, top);
    ctx.lineTo(cx, top + size);
    ctx.closePath();
    ctx.fill();
  }
}

/** Place a tooltip next to the pointer, inside its container. */
export function placeTooltip(tip: HTMLElement, canvas: HTMLCanvasElement, e: PointerEvent, text: string) {
  const rect = canvas.getBoundingClientRect();
  tip.textContent = text;
  tip.hidden = false;
  const tx = Math.min(e.clientX - rect.left + 12, rect.width - tip.offsetWidth - 4);
  tip.style.left = `${Math.max(4, tx)}px`;
  tip.style.top = `${Math.max(4, e.clientY - rect.top - 36)}px`;
}

// ------------------------------------------------------------------ frames

const CAPTURED = MOBILITY.indexOf("Captured");

/** What the views draw: a copy of a LiveState, which outlives its source's views. */
export interface Frame {
  time: number;
  leftPos: Uint32Array;
  rightPos: Uint32Array;
  /** 1 if either leg of the LEF is captured. */
  held: Uint8Array;
}

/** Copy a LiveState (from a Sim or a Replayer) into a Frame. */
export function snapshotOf(state: LiveState): Frame {
  const n = state.leftPos.length;
  const held = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    held[i] = state.leftMobility[i] === CAPTURED || state.rightMobility[i] === CAPTURED ? 1 : 0;
  }
  return { time: state.time, leftPos: state.leftPos.slice(), rightPos: state.rightPos.slice(), held };
}

// ------------------------------------------------------------------ loop arcs

/** Loops as arcs over the lattice, with a hover readout. */
export class LoopView {
  #canvas: HTMLCanvasElement;
  #ctx: CanvasRenderingContext2D;
  #frame: Frame | undefined;

  constructor(canvas: HTMLCanvasElement, tooltip: HTMLElement) {
    this.#canvas = canvas;
    this.#ctx = fit(canvas);
    // Hover: the innermost loop containing the pointer.
    canvas.addEventListener("pointermove", (e) => {
      const f = this.#frame;
      if (!f) return;
      const site = siteAt(canvas, e.clientX);
      let best = -1;
      for (let i = 0; i < f.leftPos.length; i++) {
        if (f.leftPos[i]! - 4 <= site && site <= f.rightPos[i]! + 4) {
          if (best < 0 || f.rightPos[i]! - f.leftPos[i]! < f.rightPos[best]! - f.leftPos[best]!) best = i;
        }
      }
      if (best < 0) {
        tooltip.hidden = true;
        return;
      }
      const l = f.leftPos[best]!;
      const r = f.rightPos[best]!;
      const status = f.held[best] ? "held at a boundary" : "extruding";
      placeTooltip(tooltip, canvas, e, `LEF ${best}: sites ${fmt.format(l)}–${fmt.format(r)}, ${fmt.format(r - l)} long, ${status}`);
    });
    canvas.addEventListener("pointerleave", () => {
      tooltip.hidden = true;
    });
  }

  show(frame: Frame) {
    this.#frame = frame;
    this.draw();
  }

  resize() {
    this.#ctx = fit(this.#canvas);
    this.draw();
  }

  draw() {
    const { width, height } = this.#canvas.getBoundingClientRect();
    const ctx = this.#ctx;
    const x = xScale(width);
    const base = height - MARGIN.bottom;
    ctx.clearRect(0, 0, width, height);
    drawTadBands(ctx, x, MARGIN.top, base);

    // The lattice.
    ctx.strokeStyle = colors.ink2;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x(0), base + 0.5);
    ctx.lineTo(x(geometry.nSites - 1), base + 0.5);
    ctx.stroke();
    drawCtcfs(ctx, x, base + 6, 10);

    const f = this.#frame;
    if (!f) return;
    const maxH = base - MARGIN.top - 4;

    // Loops as half-ellipses whose height grows with span, so a whole-TAD
    // loop nearly fills the panel. Height is capped for the rare loop that
    // crosses a boundary.
    ctx.lineWidth = 2;
    for (const pass of [0, 1]) {
      // Held loops on top: they are the ones the boundaries are shaping.
      ctx.strokeStyle = pass ? colors.held : colors.extruding;
      ctx.beginPath();
      for (let i = 0; i < f.leftPos.length; i++) {
        if (f.held[i] !== pass) continue;
        const x1 = x(f.leftPos[i]!);
        const x2 = x(f.rightPos[i]!);
        const r = (x2 - x1) / 2;
        const span = f.rightPos[i]! - f.leftPos[i]!;
        const h = Math.min(maxH, Math.max(2, (maxH * span) / geometry.fullHeightSpan));
        ctx.moveTo(x1, base);
        ctx.ellipse(x1 + r, base, Math.max(r, 0.5), h, 0, Math.PI, 2 * Math.PI);
      }
      ctx.stroke();
    }

    // Legs as ticks on the lattice.
    for (let i = 0; i < f.leftPos.length; i++) {
      ctx.fillStyle = f.held[i] ? colors.held : colors.extruding;
      ctx.fillRect(x(f.leftPos[i]!) - 1, base - 4, 2, 5);
      ctx.fillRect(x(f.rightPos[i]!) - 1, base - 4, 2, 5);
    }
  }
}

// ------------------------------------------------------------------ kymograph

/** Leg positions over time: each `push` scrolls down a row and draws at the top. */
export class Kymograph {
  #canvas: HTMLCanvasElement;
  #ctx: CanvasRenderingContext2D;

  constructor(canvas: HTMLCanvasElement) {
    this.#canvas = canvas;
    this.#ctx = fit(canvas);
  }

  resize() {
    this.#ctx = fit(this.#canvas);
    this.reset();
  }

  reset() {
    const { width, height } = this.#canvas.getBoundingClientRect();
    this.#ctx.fillStyle = colors.paper;
    this.#ctx.fillRect(0, 0, width, height);
  }

  push(frame: Frame) {
    const canvas = this.#canvas;
    const ctx = this.#ctx;
    const { width } = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const x = xScale(width);
    const row = 1;
    // Shift the backing store down by `row` CSS pixels.
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.drawImage(canvas, 0, 0, canvas.width, canvas.height - row * dpr, 0, row * dpr, canvas.width, canvas.height - row * dpr);
    ctx.restore();
    ctx.fillStyle = colors.paper;
    ctx.fillRect(0, 0, width, row);

    // Faint TAD boundaries, so domains read in the kymograph too.
    ctx.fillStyle = colors.rule;
    for (const b of geometry.boundaries) ctx.fillRect(x(b), 0, 1, row);

    for (let i = 0; i < frame.leftPos.length; i++) {
      ctx.fillStyle = frame.held[i] ? colors.held : colors.extruding;
      ctx.fillRect(x(frame.leftPos[i]!) - 1, 0, 2, row);
      ctx.fillRect(x(frame.rightPos[i]!) - 1, 0, 2, row);
    }
  }
}
