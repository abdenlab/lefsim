// Contact map demo: a live Sim, and every pair of lattice bins colored by
// its approximate contact probability, recomputed as the loops change. The
// upper triangle is the current state; the lower triangle is the average
// over simulation time since the last reset.
//
// Each bin is represented by its middle site. The contact probability of two
// sites follows a Gaussian chain, P = d^(-3/2), where d is their effective
// distance (steps and hops across loops, each of length 1). The map is drawn
// under the loop arcs on the same x axis, so a loop's legs line up with the
// corner of the square it makes. The CTCFs, rates and strength are editable
// while it runs (see `editable.ts`); the average carries on across changes.

import { effectiveDistances, init } from "lefsim";

import { CtcfLanes, DEFAULT_CTCFS, EditableRun } from "./editable.ts";
import { N_SITES, readRunInputs, worldOf } from "./model.ts";
import { condensedIndex, drawColorbar as drawBar, FALL, packedColors, SquareMap } from "./heatmap.ts";
import { $, fit, fmt, LoopView, MARGIN, onThemeChange, placeTooltip, siteAt, snapshotOf, xScale } from "./render.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
/** Cap on events per frame, so a burst can't stall the page. */
const MAX_EVENTS_PER_FRAME = 40_000;

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const binIn = $<HTMLSelectElement>("bin");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const laneCanvas = $<HTMLCanvasElement>("lanes");
const mapCanvas = $<HTMLCanvasElement>("map");
const mapTooltip = $("map-tooltip");
const colorbarCanvas = $<HTMLCanvasElement>("colorbar");
const rTime = $("r-time");
const rBins = $("r-bins");
const rCompute = $("r-compute");
const rDraw = $("r-draw");
const rFps = $("r-fps");
const rAveraged = $("r-averaged");
const resetBtn = $<HTMLButtonElement>("reset-average");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
let laneCtx = fit(laneCanvas);
const map = new SquareMap(mapCanvas);
let colorbarCtx = fit(colorbarCanvas);

// ------------------------------------------------------------------ state

/** The run and its editable model. */
const run = new EditableRun(
  DEFAULT_CTCFS,
  {
    nLeft: $<HTMLInputElement>("nctcf-left"),
    nRight: $<HTMLInputElement>("nctcf-right"),
    step: $<HTMLInputElement>("rate-step"),
    respawn: $<HTMLInputElement>("rate-respawn"),
    respawnStalled: $<HTMLInputElement>("rate-respawn-stalled"),
    respawnEvicted: $<HTMLInputElement>("rate-respawn-evicted"),
    respawnHeld: $<HTMLInputElement>("rate-respawn-held"),
    strength: $<HTMLInputElement>("strength"),
  },
);
run.onChange = () => {
  if (!playing) draw();
};
const lanes = new CtcfLanes(run, 0);
let playing = false;
let lastFrame: number | undefined;

/** Sites per bin, the bins' middle sites, and the latest distances between them (condensed). */
let bin = 10;
let centers = new Uint32Array(0);
let dist: Float64Array = new Float64Array(0);
/**
 * The time average: for each pair, the sum of P weighted by the simulation
 * time it held, over `averagedTime`. Each frame's state is weighted by the
 * time since the previous frame, so pausing adds nothing.
 */
let sumP = new Float64Array(0);
let averagedTime = 0;
let lastTime: number | undefined;
/** Recent map update times, for the rate readout. */
let updates: number[] = [];

function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

const fmtMs = (ms: number) => (ms < 10 ? `${ms.toFixed(1)} ms` : `${fmt.format(ms)} ms`);

// ------------------------------------------------------------------ colors

/**
 * The color range in log10 P: from the farthest pair of sites to the
 * nearest pair of distinct bins. Distances within a bin (the diagonal) are
 * clamped to the top.
 */
function logRange(): [number, number] {
  return [-1.5 * Math.log10(N_SITES - 1), -1.5 * Math.log10(bin)];
}

// ------------------------------------------------------------------ the map

function setBins() {
  bin = Number(binIn.value);
  const k = Math.ceil(N_SITES / bin);
  centers = Uint32Array.from({ length: k }, (_, i) => Math.min(N_SITES - 1, Math.floor(i * bin + bin / 2)));
  map.setSize(k);
  rBins.textContent = `${fmt.format(k)} × ${fmt.format(k)}, ${fmt.format((k * (k - 1)) / 2)} pairs`;
  buildPixelTable();
  drawColorbar();
  resetAverage();
}

function resetAverage() {
  const k = centers.length;
  sumP = new Float64Array((k * (k - 1)) / 2);
  if (means.length !== sumP.length) {
    means = new Float64Array(sumP.length);
    meanWords = new Uint32Array(means.buffer);
  }
  averagedTime = 0;
  lastTime = run.sim?.time;
}

/** Add the current state to the time average, weighted by the time since the last frame. */
function accumulate(time: number) {
  const dt = lastTime === undefined ? 0 : time - lastTime;
  lastTime = time;
  if (!(dt > 0)) return;
  const pOf = pOfDistance;
  const top = pOf.length - 1;
  for (let at = 0; at < dist.length; at++) sumP[at]! += pOf[Math.min(dist[at]!, top)]! * dt;
  averagedTime += dt;
}

/** Distances between every pair of bins in the current state. */
function measure(): number {
  const started = performance.now();
  dist = effectiveDistances(worldOf(run.sim!), centers, LENGTHS);
  return performance.now() - started;
}

/**
 * The pixel for each effective distance, packed RGBA as the image's bytes
 * read as one 32-bit word. Distances are integers no longer than the walk
 * across the lattice, so a table replaces a logarithm per pair.
 */
let pixelOf = new Uint32Array(0);
/** The 256 colormap entries as packed pixels. */
let pixelOfIndex: Uint32Array = new Uint32Array(256);
/** P for each effective distance: `d^(-3/2)`. */
let pOfDistance = new Float64Array(0);
/**
 * Colormap index for any P in (0, 1], keyed by the top 20 bits of its
 * float64 encoding (exponent and 8 bits of mantissa, about 0.4% apart), so
 * coloring an average takes a lookup instead of a logarithm.
 */
const indexOfBits = new Uint8Array(1 << 20);
/** Scratch for the averages, and a view of their high 32-bit words. */
let means = new Float64Array(0);
let meanWords = new Uint32Array(0);
const HIGH_WORD = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1 ? 1 : 0;

function buildPixelTable() {
  const [lo, hi] = logRange();
  const scale = 255 / (hi - lo);
  pixelOfIndex = packedColors(FALL);
  pixelOf = new Uint32Array(N_SITES);
  pOfDistance = new Float64Array(N_SITES);
  for (let d = 0; d < N_SITES; d++) {
    // P = d^(-3/2) → log10 P = -1.5 log10 d; the diagonal (d = 0) takes the top.
    const c = d === 0 ? 255 : Math.max(0, Math.min(255, Math.round((-1.5 * Math.log10(d) - lo) * scale)));
    pixelOf[d] = pixelOfIndex[c]!;
    pOfDistance[d] = d === 0 ? 1 : d ** -1.5;
  }
  // Each key's middle value stands for its range.
  const value = new Float64Array(1);
  const words = new Uint32Array(value.buffer);
  for (let key = 0; key < 1 << 20; key++) {
    words[HIGH_WORD] = (key << 12) | 0x800;
    words[1 - HIGH_WORD] = 0;
    const p = value[0]!;
    const c = p > 0 && Number.isFinite(p) ? Math.round((Math.log10(p) - lo) * scale) : 0;
    indexOfBits[key] = c < 0 ? 0 : c > 255 ? 255 : c;
  }
}

/**
 * Color the map image: the current state (`dist`) above the diagonal, the
 * time average below it (the current state too, until time has passed).
 */
function paint() {
  const k = centers.length;
  const px = map.pixels;
  const table = pixelOf;
  const top = table.length - 1;
  const averaged = averagedTime > 0;
  if (averaged) {
    const inv = 1 / averagedTime;
    for (let n = 0; n < sumP.length; n++) means[n] = sumP[n]! * inv;
  }
  const words = meanWords;
  const keyIndex = indexOfBits;
  const colors = pixelOfIndex;
  let at = 0;
  for (let i = 0; i < k; i++) {
    px[i * k + i] = table[0]!;
    for (let j = i + 1; j < k; j++, at++) {
      const now = table[Math.min(dist[at]!, top)]!;
      px[i * k + j] = now;
      if (averaged) {
        px[j * k + i] = colors[keyIndex[words[2 * at + HIGH_WORD]! >>> 12]!]!;
      } else {
        px[j * k + i] = now;
      }
    }
  }
}

function drawColorbar() {
  const { width, height } = colorbarCanvas.getBoundingClientRect();
  const ctx = colorbarCtx;
  ctx.clearRect(0, 0, width, height);
  const barW = Math.min(260, width - MARGIN.left - MARGIN.right);
  const [lo, hi] = logRange();
  drawBar(ctx, { x0: width - MARGIN.right - barW, y0: 8, width: barW, lut: FALL, lo, hi, label: "P" });
}

// ------------------------------------------------------------------ frames

/** Recompute and redraw everything from the sim's current state. */
function draw() {
  const sim = run.sim;
  if (!sim) return;
  const f = snapshotOf(sim.state());
  loops.show(f);
  drawLanes();
  const computeMs = measure();
  const started = performance.now();
  accumulate(f.time);
  paint();
  map.draw();
  const drawMs = performance.now() - started;
  rTime.textContent = fmt.format(f.time);
  rAveraged.textContent = averagedTime > 0 ? `${fmt.format(averagedTime)} time units` : "–";
  rCompute.textContent = fmtMs(computeMs);
  rDraw.textContent = fmtMs(drawMs);
  const now = performance.now();
  updates.push(now);
  while (updates.length > 2 && now - updates[0]! > 1000) updates.shift();
  if (updates.length > 1) {
    const perSecond = ((updates.length - 1) * 1000) / (now - updates[0]!);
    rFps.textContent = `${perSecond.toFixed(perSecond < 10 ? 1 : 0)} / s`;
  }
}

function restart() {
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  run.restart(nLefs, seed);
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, and the contact ` +
    `probability of every pair of bins, from the effective distance through the loops. Simulated in your browser.`;
  lastFrame = undefined;
  updates = [];
  resetAverage();
  draw();
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  updates = [];
  if (on) requestAnimationFrame(frame);
}

function frame(now: number) {
  const sim = run.sim;
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  const report = sim.runUntil(sim.time + speed() * dt, { maxEvents: MAX_EVENTS_PER_FRAME });
  behind.hidden = report.stopped !== "reached_event_count";
  draw();
  requestAnimationFrame(frame);
}

// ------------------------------------------------------------------ CTCF lanes

function drawLanes() {
  const { width, height } = laneCanvas.getBoundingClientRect();
  laneCtx.clearRect(0, 0, width, height);
  lanes.draw(laneCtx, xScale(width));
}

function laneAt(e: PointerEvent): number {
  const rect = laneCanvas.getBoundingClientRect();
  return lanes.at(e.clientX - rect.left, e.clientY - rect.top, xScale(rect.width));
}

laneCanvas.addEventListener("pointerdown", (e) => {
  lanes.dragging = laneAt(e);
  if (lanes.dragging < 0) return;
  laneCanvas.setPointerCapture(e.pointerId);
  laneCanvas.style.cursor = "grabbing";
  e.preventDefault();
});
laneCanvas.addEventListener("pointermove", (e) => {
  if (lanes.dragging >= 0) {
    const site = Math.round(Math.max(0, Math.min(N_SITES - 1, siteAt(laneCanvas, e.clientX))));
    if (run.moveCtcf(lanes.dragging, site) && !playing) draw();
    return;
  }
  const near = laneAt(e);
  laneCanvas.style.cursor = near >= 0 ? "grab" : "";
  if (near !== lanes.hovered) {
    lanes.hovered = near;
    drawLanes();
  }
});
const endDrag = () => {
  lanes.dragging = -1;
  laneCanvas.style.cursor = "";
  drawLanes();
};
laneCanvas.addEventListener("pointerup", endDrag);
laneCanvas.addEventListener("pointercancel", endDrag);
laneCanvas.addEventListener("pointerleave", () => {
  if (lanes.dragging < 0 && lanes.hovered >= 0) {
    lanes.hovered = -1;
    drawLanes();
  }
});

// ------------------------------------------------------------------ hover

mapCanvas.addEventListener("pointermove", (e) => {
  const cell = map.cellAt(e);
  if (!cell || dist.length === 0) {
    mapTooltip.hidden = true;
    return;
  }
  const [row, col] = cell;
  const k = centers.length;
  const [i, j] = row < col ? [row, col] : [col, row];
  const a = centers[i]!;
  const b = centers[j]!;
  if (i === j) {
    placeTooltip(mapTooltip, mapCanvas, e, `Bin at site ${fmt.format(a)}`);
    return;
  }
  const at = condensedIndex(i, j, k);
  const d = dist[at]!;
  const mean = averagedTime > 0 ? `; averaged, P = ${(sumP[at]! / averagedTime).toExponential(1)}` : "";
  placeTooltip(
    mapTooltip,
    mapCanvas,
    e,
    `Sites ${fmt.format(a)} and ${fmt.format(b)}: ${fmt.format(b - a)} apart. Now: effective distance ${fmt.format(d)}, P = ${(d ** -1.5).toExponential(1)}${mean}`,
  );
});
mapCanvas.addEventListener("pointerleave", () => {
  mapTooltip.hidden = true;
});

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  laneCtx = fit(laneCanvas);
  drawLanes();
  map.resize();
  colorbarCtx = fit(colorbarCanvas);
  drawColorbar();
}).observe(mapCanvas);
onThemeChange(() => {
  loops.draw();
  drawLanes();
  map.draw();
  drawColorbar();
});

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  restart();
});
binIn.addEventListener("change", () => {
  setBins();
  updates = [];
  draw();
});
speedIn.addEventListener("input", showSpeed);
resetBtn.addEventListener("click", () => {
  resetAverage();
  if (!playing) draw();
});

await init();
showSpeed();
setBins();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
