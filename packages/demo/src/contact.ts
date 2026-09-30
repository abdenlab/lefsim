// Contact map demo: a live Sim, and every pair of lattice bins colored by
// its approximate contact probability, recomputed as the loops change.
//
// Each bin is represented by its middle site. The contact probability of two
// sites follows a Gaussian chain, P = d^(-3/2), where d is their effective
// distance (steps and hops across loops, each of length 1). The map is drawn
// under the loop arcs on the same x axis, so a loop's legs line up with the
// corner of the square it makes.

import { effectiveDistances, init, type Sim } from "lefsim";

import { N_SITES, newSim, readRunInputs } from "./model.ts";
import { $, colors, fit, fmt, LoopView, MARGIN, onThemeChange, placeTooltip, setLattice, snapshotOf } from "./render.ts";

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
const mapCanvas = $<HTMLCanvasElement>("map");
const mapTooltip = $("map-tooltip");
const colorbarCanvas = $<HTMLCanvasElement>("colorbar");
const rTime = $("r-time");
const rBins = $("r-bins");
const rCompute = $("r-compute");
const rDraw = $("r-draw");
const rFps = $("r-fps");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
let mapCtx = fit(mapCanvas);
let colorbarCtx = fit(colorbarCanvas);

// ------------------------------------------------------------------ state

let sim: Sim | undefined;
let playing = false;
let lastFrame: number | undefined;

/** Sites per bin, the bins' middle sites, and the latest distances between them (condensed). */
let bin = 10;
let centers = new Uint32Array(0);
let dist: Float64Array = new Float64Array(0);
/** The map at one pixel per bin, scaled up when drawn. */
let image: ImageData | undefined;
const offscreen = document.createElement("canvas");
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
 * The "fall" colormap (white through yellow, orange and red to black), as a
 * 256-entry RGB table, interpolated between its stops.
 */
const FALL = (() => {
  const stops = [
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
  ];
  const lut = new Uint8Array(256 * 3);
  for (let i = 0; i < 256; i++) {
    const t = (i / 255) * (stops.length - 1);
    const k = Math.min(Math.floor(t), stops.length - 2);
    const f = t - k;
    for (let c = 0; c < 3; c++) lut[3 * i + c] = Math.round(stops[k]![c]! * (1 - f) + stops[k + 1]![c]! * f);
  }
  return lut;
})();

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
  image = new ImageData(k, k);
  offscreen.width = k;
  offscreen.height = k;
  rBins.textContent = `${fmt.format(k)} × ${fmt.format(k)}, ${fmt.format((k * (k - 1)) / 2)} pairs`;
  buildPixelTable();
  drawColorbar();
}

/** Distances between every pair of bins in the current state. */
function measure(): number {
  const started = performance.now();
  // The live state has the LEF columns a World needs; it's used at once,
  // before the next call into the sim.
  dist = effectiveDistances({ lattice: sim!.lattice, lefs: sim!.state() }, centers, LENGTHS);
  return performance.now() - started;
}

/**
 * The pixel for each effective distance, packed RGBA as the image's bytes
 * read as one 32-bit word. Distances are integers no longer than the walk
 * across the lattice, so a table replaces a logarithm per pair.
 */
let pixelOf = new Uint32Array(0);

function buildPixelTable() {
  const [lo, hi] = logRange();
  const scale = 255 / (hi - lo);
  const littleEndian = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;
  pixelOf = new Uint32Array(N_SITES);
  for (let d = 0; d < N_SITES; d++) {
    // P = d^(-3/2) → log10 P = -1.5 log10 d; the diagonal (d = 0) takes the top.
    const c = d === 0 ? 255 : Math.max(0, Math.min(255, Math.round((-1.5 * Math.log10(d) - lo) * scale)));
    const [r, g, b] = [FALL[3 * c]!, FALL[3 * c + 1]!, FALL[3 * c + 2]!];
    pixelOf[d] = littleEndian ? ((255 << 24) | (b << 16) | (g << 8) | r) >>> 0 : ((r << 24) | (g << 16) | (b << 8) | 255) >>> 0;
  }
}

/** Color the map image from `dist`, both halves of the symmetric matrix. */
function paint() {
  const img = image!;
  const k = centers.length;
  const px = new Uint32Array(img.data.buffer);
  const table = pixelOf;
  const top = table.length - 1;
  let at = 0;
  for (let i = 0; i < k; i++) {
    px[i * k + i] = table[0]!;
    for (let j = i + 1; j < k; j++) {
      const p = table[Math.min(dist[at++]!, top)]!;
      px[i * k + j] = p;
      px[j * k + i] = p;
    }
  }
}

/** The map's square, aligned with the loop panel's lattice axis. */
function mapBox() {
  const { width } = mapCanvas.getBoundingClientRect();
  const size = width - MARGIN.left - MARGIN.right;
  return { left: MARGIN.left, top: 0, size };
}

function drawMap() {
  const { width, height } = mapCanvas.getBoundingClientRect();
  const ctx = mapCtx;
  ctx.clearRect(0, 0, width, height);
  if (!image) return;
  offscreen.getContext("2d")!.putImageData(image, 0, 0);
  const { left, top, size } = mapBox();
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(offscreen, left, top, size, size);
  ctx.strokeStyle = colors.rule;
  ctx.lineWidth = 1;
  ctx.strokeRect(left + 0.5, top + 0.5, size - 1, size - 1);
}

function drawColorbar() {
  const { width, height } = colorbarCanvas.getBoundingClientRect();
  const ctx = colorbarCtx;
  ctx.clearRect(0, 0, width, height);
  const barW = Math.min(260, width - MARGIN.left - MARGIN.right);
  const x0 = width - MARGIN.right - barW;
  const y0 = 8;
  for (let i = 0; i < barW; i++) {
    const c = Math.round((i / (barW - 1)) * 255);
    ctx.fillStyle = `rgb(${FALL[3 * c]}, ${FALL[3 * c + 1]}, ${FALL[3 * c + 2]})`;
    ctx.fillRect(x0 + i, y0, 1, 10);
  }
  ctx.strokeStyle = colors.rule;
  ctx.strokeRect(x0 + 0.5, y0 + 0.5, barW - 1, 9);
  // Ticks at each power of ten in range.
  const [lo, hi] = logRange();
  ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
  ctx.fillStyle = colors.muted;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  for (let e = Math.ceil(lo); e <= Math.floor(hi); e++) {
    const tx = x0 + ((e - lo) / (hi - lo)) * (barW - 1);
    ctx.fillRect(tx, y0 + 10, 1, 3);
    ctx.fillText(e === 0 ? "1" : `10${superscript(e)}`, tx, y0 + 15);
  }
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  ctx.fillText("P", x0 - 8, y0 + 5);
}

function superscript(n: number): string {
  const digits = "⁰¹²³⁴⁵⁶⁷⁸⁹";
  return (n < 0 ? "⁻" : "") + String(Math.abs(n)).split("").map((d) => digits[Number(d)]).join("");
}

// ------------------------------------------------------------------ frames

/** Recompute and redraw everything from the sim's current state. */
function draw() {
  if (!sim) return;
  const f = snapshotOf(sim.state());
  loops.show(f);
  const computeMs = measure();
  const started = performance.now();
  paint();
  drawMap();
  const drawMs = performance.now() - started;
  rTime.textContent = fmt.format(f.time);
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
  sim?.free();
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  sim = newSim(nLefs, seed);
  setLattice(sim.lattice);
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice split into five TADs, and the contact ` +
    `probability of every pair of bins, from the effective distance through the loops. Simulated in your browser.`;
  lastFrame = undefined;
  updates = [];
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
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  const report = sim.runUntil(sim.time + speed() * dt, { maxEvents: MAX_EVENTS_PER_FRAME });
  behind.hidden = report.stopped !== "reached_event_count";
  draw();
  requestAnimationFrame(frame);
}

// ------------------------------------------------------------------ hover

mapCanvas.addEventListener("pointermove", (e) => {
  const rect = mapCanvas.getBoundingClientRect();
  const { left, top, size } = mapBox();
  const k = centers.length;
  const col = Math.floor(((e.clientX - rect.left - left) / size) * k);
  const row = Math.floor(((e.clientY - rect.top - top) / size) * k);
  if (col < 0 || row < 0 || col >= k || row >= k || dist.length === 0) {
    mapTooltip.hidden = true;
    return;
  }
  const [i, j] = row < col ? [row, col] : [col, row];
  const a = centers[i]!;
  const b = centers[j]!;
  if (i === j) {
    placeTooltip(mapTooltip, mapCanvas, e, `Bin at site ${fmt.format(a)}`);
    return;
  }
  // Condensed index of pair (i, j), i < j.
  const d = dist[i * k - (i * (i + 1)) / 2 + (j - i - 1)]!;
  placeTooltip(
    mapTooltip,
    mapCanvas,
    e,
    `Sites ${fmt.format(a)} and ${fmt.format(b)}: ${fmt.format(b - a)} apart, effective distance ${fmt.format(d)}, P = ${(d ** -1.5).toExponential(1)}`,
  );
});
mapCanvas.addEventListener("pointerleave", () => {
  mapTooltip.hidden = true;
});

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  mapCtx = fit(mapCanvas);
  colorbarCtx = fit(colorbarCanvas);
  drawColorbar();
  drawMap();
}).observe(mapCanvas);
onThemeChange(() => {
  loops.draw();
  drawMap();
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

await init();
showSpeed();
setBins();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
