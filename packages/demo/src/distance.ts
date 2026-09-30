// Effective distance demo: a live Sim, a handful of probe sites on its
// lattice, and the probes laid out so that their spacing follows the
// effective distances between them (walking the lattice, or hopping across
// loops), recomputed every frame. Steps and hops both have length 1.
//
// The layout minimizes stress: each pair of probes is a spring whose rest
// length is its effective distance, relaxed a few sweeps per frame from the
// previous positions, so it moves smoothly as loops grow and dissolve.

import { effectiveDistances, init, type Sim } from "lefsim";

import { N_SITES, newSim, readRunInputs } from "./model.ts";
import { $, colors, fit, fmt, LoopView, onThemeChange, setLattice, siteAt, snapshotOf, xScale } from "./render.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
/** Cap on events per frame, so a burst can't stall the page. */
const MAX_EVENTS_PER_FRAME = 40_000;
/** Stress sweeps over all pairs per frame. */
const SWEEPS = 20;

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const nProbesIn = $<HTMLInputElement>("nprobes");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const probeCanvas = $<HTMLCanvasElement>("probes");
const graphCanvas = $<HTMLCanvasElement>("graph");
const rTime = $("r-time");
const rClosest = $("r-closest");
const rFit = $("r-fit");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
let probeCtx = fit(probeCanvas);
let graphCtx = fit(graphCanvas);

// ------------------------------------------------------------------ state

let sim: Sim | undefined;
let playing = false;
let lastFrame: number | undefined;

/** Probe sites, in probe order (a probe keeps its color when dragged past another). */
let probes: number[] = [];
/** Effective distances, `k × k`, and lattice separations. */
let dist = new Float64Array(0);
/** Layout positions in distance units, `[x0, y0, x1, y1, …]`. */
let pos = new Float64Array(0);
/** Pairs `[i, j]`, shuffled every sweep. */
let pairs: [number, number][] = [];
/** The view transform, eased toward the layout's bounding box. */
let view: { cx: number; cy: number; scale: number } | undefined;
let hovered = -1;
let dragging = -1;

function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

/** Probe `i`'s color: hues spread over the probes, in their starting order. */
function probeColor(i: number, k = probes.length): string {
  const hue = 150 + (190 * i) / Math.max(1, k - 1);
  return `oklch(0.64 0.16 ${hue})`;
}

// ------------------------------------------------------------------ probes

/** `k` probes spread evenly over the lattice, and a fresh layout. */
function setProbes(k: number) {
  const step = N_SITES / k;
  probes = Array.from({ length: k }, (_, i) => Math.round(step * (i + 0.5)));
  pairs = [];
  for (let i = 0; i < k; i++) for (let j = i + 1; j < k; j++) pairs.push([i, j]);
  dist = new Float64Array(k * k);
  // Start on the lattice's own line, nudged off it: a layout that starts
  // exactly collinear can never leave the line.
  pos = new Float64Array(2 * k);
  probes.forEach((s, i) => {
    pos[2 * i] = s;
    pos[2 * i + 1] = (i % 2 ? 1 : -1) * 20;
  });
  view = undefined;
  hovered = -1;
}

function readProbeCount(): number {
  const k = Math.max(2, Math.min(12, Math.floor(Number(nProbesIn.value) || 8)));
  nProbesIn.value = String(k);
  return k;
}

function measure() {
  if (!sim) return;
  // The live state has the LEF columns a World needs; it's used at once,
  // before the next call into the sim.
  const d = effectiveDistances({ lattice: sim.lattice, lefs: sim.state() }, probes, { ...LENGTHS, targets: probes });
  dist.set(d);
}

// ------------------------------------------------------------------ layout

/** One stress sweep: move each pair toward its target distance. */
function relax() {
  const k = probes.length;
  let dMin = Infinity;
  for (const [i, j] of pairs) dMin = Math.min(dMin, Math.max(1, dist[i * k + j]!));
  // Fisher–Yates, so no pair always goes first.
  for (let n = pairs.length - 1; n > 0; n--) {
    const m = Math.floor(Math.random() * (n + 1));
    [pairs[n], pairs[m]] = [pairs[m]!, pairs[n]!];
  }
  for (const [i, j] of pairs) {
    const d = Math.max(1, dist[i * k + j]!);
    const dx = pos[2 * i]! - pos[2 * j]!;
    const dy = pos[2 * i + 1]! - pos[2 * j + 1]!;
    const len = Math.hypot(dx, dy) || 1e-6;
    // Weight 1/d²: near pairs are placed accurately, far ones loosely.
    const mu = Math.min(0.5, (dMin * dMin) / (d * d));
    const r = (mu * (len - d)) / (2 * len);
    pos[2 * i] = pos[2 * i]! - r * dx;
    pos[2 * i + 1] = pos[2 * i + 1]! - r * dy;
    pos[2 * j] = pos[2 * j]! + r * dx;
    pos[2 * j + 1] = pos[2 * j + 1]! + r * dy;
  }
}

/** How far drawn spacing is from effective distance: the mean relative error over pairs. */
function layoutError(): number {
  const k = probes.length;
  let sum = 0;
  for (const [i, j] of pairs) {
    const d = Math.max(1, dist[i * k + j]!);
    const len = Math.hypot(pos[2 * i]! - pos[2 * j]!, pos[2 * i + 1]! - pos[2 * j + 1]!);
    sum += Math.abs(len - d) / d;
  }
  return pairs.length ? sum / pairs.length : 0;
}

// ------------------------------------------------------------------ drawing

const PAD = 34;

function updateView(width: number, height: number) {
  const k = probes.length;
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  for (let i = 0; i < k; i++) {
    x0 = Math.min(x0, pos[2 * i]!);
    x1 = Math.max(x1, pos[2 * i]!);
    y0 = Math.min(y0, pos[2 * i + 1]!);
    y1 = Math.max(y1, pos[2 * i + 1]!);
  }
  const target = {
    cx: (x0 + x1) / 2,
    cy: (y0 + y1) / 2,
    scale: Math.min((width - 2 * PAD) / Math.max(x1 - x0, 1), (height - 2 * PAD - 20) / Math.max(y1 - y0, 1)),
  };
  // Ease the view, so the picture zooms rather than jumps; never let a probe leave it.
  if (!view) view = target;
  else {
    view.cx += 0.15 * (target.cx - view.cx);
    view.cy += 0.15 * (target.cy - view.cy);
    view.scale = Math.min(view.scale + 0.15 * (target.scale - view.scale), target.scale * 1.02);
  }
}

function toScreen(i: number, width: number, height: number): [number, number] {
  const v = view!;
  return [width / 2 + (pos[2 * i]! - v.cx) * v.scale, (height - 20) / 2 + (pos[2 * i + 1]! - v.cy) * v.scale];
}

/** A round length near `target`: 1, 2 or 5 times a power of ten. */
function niceLength(target: number): number {
  const p = 10 ** Math.floor(Math.log10(target));
  return [1, 2, 5, 10].map((m) => m * p).find((v) => v >= target * 0.7) ?? p;
}

function drawGraph() {
  const { width, height } = graphCanvas.getBoundingClientRect();
  const ctx = graphCtx;
  ctx.clearRect(0, 0, width, height);
  const k = probes.length;
  if (k === 0) return;
  updateView(width, height);
  const pts = Array.from({ length: k }, (_, i) => toScreen(i, width, height));
  ctx.font = "12px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
  ctx.lineJoin = "round";

  // The lattice order: probes joined left to right along the lattice.
  const order = probes.map((s, i) => [s, i] as const).sort((a, b) => a[0] - b[0]).map(([, i]) => i);
  ctx.strokeStyle = colors.rule;
  ctx.lineWidth = 2;
  ctx.beginPath();
  order.forEach((i, n) => (n ? ctx.lineTo(...pts[i]!) : ctx.moveTo(...pts[i]!)));
  ctx.stroke();

  // A hovered probe's distances to the others.
  if (hovered >= 0) {
    const [hx, hy] = pts[hovered]!;
    ctx.strokeStyle = colors.muted;
    ctx.lineWidth = 1;
    ctx.setLineDash([3, 3]);
    for (let j = 0; j < k; j++) {
      if (j === hovered) continue;
      ctx.beginPath();
      ctx.moveTo(hx, hy);
      ctx.lineTo(...pts[j]!);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    for (let j = 0; j < k; j++) {
      if (j === hovered) continue;
      const label = fmt.format(dist[hovered * k + j]!);
      const mx = (hx + pts[j]![0]) / 2;
      const my = (hy + pts[j]![1]) / 2;
      ctx.lineWidth = 4;
      ctx.strokeStyle = colors.paper;
      ctx.strokeText(label, mx, my);
      ctx.fillStyle = colors.ink2;
      ctx.fillText(label, mx, my);
    }
  }

  // The probes, labeled with their sites.
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  for (let i = 0; i < k; i++) {
    const [px, py] = pts[i]!;
    ctx.beginPath();
    ctx.arc(px, py, i === hovered ? 9 : 7, 0, 2 * Math.PI);
    ctx.fillStyle = probeColor(i);
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = colors.paper;
    ctx.stroke();
    const label = fmt.format(probes[i]!);
    ctx.lineWidth = 4;
    ctx.strokeText(label, px + 11, py - 8);
    ctx.fillStyle = colors.ink;
    ctx.fillText(label, px + 11, py - 8);
  }

  // Scale bar, in steps (or hops).
  const bar = niceLength((width * 0.2) / view!.scale);
  const barPx = bar * view!.scale;
  const by = height - 12;
  ctx.strokeStyle = colors.ink2;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(12, by - 4);
  ctx.lineTo(12, by);
  ctx.lineTo(12 + barPx, by);
  ctx.lineTo(12 + barPx, by - 4);
  ctx.stroke();
  ctx.fillStyle = colors.muted;
  ctx.fillText(`${fmt.format(bar)} steps`, 18 + barPx, by);
}

/** Probe markers under the loop panel, aligned with it. */
function drawProbes() {
  const { width, height } = probeCanvas.getBoundingClientRect();
  const ctx = probeCtx;
  const x = xScale(width);
  ctx.clearRect(0, 0, width, height);
  probes.forEach((s, i) => {
    const px = x(s);
    ctx.strokeStyle = probeColor(i);
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, height - 14);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(px, height - 9, i === hovered || i === dragging ? 7 : 5.5, 0, 2 * Math.PI);
    ctx.fillStyle = probeColor(i);
    ctx.fill();
  });
}

function draw() {
  if (!sim) return;
  const f = snapshotOf(sim.state());
  loops.show(f);
  measure();
  for (let n = 0; n < SWEEPS; n++) relax();
  drawProbes();
  drawGraph();
  updateReadout(f.time);
}

function updateReadout(time: number) {
  rTime.textContent = fmt.format(time);
  const k = probes.length;
  let best: [number, number] | undefined;
  for (const [i, j] of pairs) {
    if (!best || dist[i * k + j]! < dist[best[0] * k + best[1]]!) best = [i, j];
  }
  if (best) {
    const [i, j] = best;
    const [a, b] = [probes[i]!, probes[j]!].sort((p, q) => p - q);
    rClosest.textContent = `${fmt.format(a!)} and ${fmt.format(b!)}: ${fmt.format(dist[i * k + j]!)} apart, ${fmt.format(b! - a!)} on the lattice`;
  }
  rFit.textContent = `${Math.round(100 * layoutError())}% off on average`;
}

// ------------------------------------------------------------------ loop

function restart() {
  sim?.free();
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  sim = newSim(nLefs, seed);
  setLattice(sim.lattice);
  setProbes(readProbeCount());
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, and ${probes.length} probe sites laid out ` +
    `by how far apart the loops make them. Simulated in your browser.`;
  lastFrame = undefined;
  draw();
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  if (on) requestAnimationFrame(frame);
}

function frame(now: number) {
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  const report = sim.runUntil(sim.time + speed() * dt, { maxEvents: MAX_EVENTS_PER_FRAME });
  behind.hidden = report.stopped !== "reached_event_count";
  draw();
  requestAnimationFrame(frame);
}

/** While paused, keep relaxing the layout for a moment after a change. */
function settle(frames = 60) {
  if (playing || frames <= 0) return;
  draw();
  requestAnimationFrame(() => settle(frames - 1));
}

// ------------------------------------------------------------------ probe dragging and hover

function probeNear(clientX: number): number {
  const rect = probeCanvas.getBoundingClientRect();
  const x = xScale(rect.width);
  let best = -1;
  let bestPx = 12;
  probes.forEach((s, i) => {
    const px = Math.abs(x(s) - (clientX - rect.left));
    if (px < bestPx) [best, bestPx] = [i, px];
  });
  return best;
}

probeCanvas.addEventListener("pointerdown", (e) => {
  dragging = probeNear(e.clientX);
  if (dragging < 0) return;
  probeCanvas.setPointerCapture(e.pointerId);
  e.preventDefault();
});
probeCanvas.addEventListener("pointermove", (e) => {
  if (dragging < 0) {
    const near = probeNear(e.clientX);
    probeCanvas.style.cursor = near >= 0 ? "grab" : "";
    if (near !== hovered) {
      hovered = near;
      if (!playing) draw();
    }
    return;
  }
  const site = Math.round(Math.max(0, Math.min(N_SITES - 1, siteAt(probeCanvas, e.clientX))));
  if (probes.some((s, i) => s === site && i !== dragging)) return; // one probe per site
  probes[dragging] = site;
  hovered = dragging;
  if (!playing) settle();
});
const endDrag = () => {
  dragging = -1;
  if (!playing) settle();
};
probeCanvas.addEventListener("pointerup", endDrag);
probeCanvas.addEventListener("pointercancel", endDrag);
probeCanvas.addEventListener("pointerleave", () => {
  if (dragging < 0 && hovered >= 0) {
    hovered = -1;
    if (!playing) draw();
  }
});

graphCanvas.addEventListener("pointermove", (e) => {
  if (!view) return;
  const rect = graphCanvas.getBoundingClientRect();
  const { width, height } = rect;
  const mx = e.clientX - rect.left;
  const my = e.clientY - rect.top;
  let near = -1;
  let bestPx = 14;
  for (let i = 0; i < probes.length; i++) {
    const [px, py] = toScreen(i, width, height);
    const d = Math.hypot(px - mx, py - my);
    if (d < bestPx) [near, bestPx] = [i, d];
  }
  if (near !== hovered) {
    hovered = near;
    if (!playing) draw();
  }
});
graphCanvas.addEventListener("pointerleave", () => {
  hovered = -1;
  if (!playing) draw();
});

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  probeCtx = fit(probeCanvas);
  graphCtx = fit(graphCanvas);
  if (sim) draw();
}).observe($("arcs"));
new ResizeObserver(() => {
  graphCtx = fit(graphCanvas);
  if (sim) draw();
}).observe(graphCanvas);
onThemeChange(() => draw());

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  restart();
});
nProbesIn.addEventListener("change", () => {
  setProbes(readProbeCount());
  if (!playing) settle();
});
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
else settle(120);
