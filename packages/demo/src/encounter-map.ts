// Encounter map demo: a live Sim, and for every pair of lattice bins, how
// often they meet and for how long. Two bins are in an encounter while their
// effective distance is at most a threshold.
//
// The distances between all pairs are sampled every SAMPLE_DT of simulation
// time. Each pair keeps two running totals, from which both maps follow:
// encounters started, and time spent in encounters. Below the diagonal is the
// frequency (encounters per time unit), above it the mean duration (time in
// encounters per encounter). Sampling has a time budget per frame; when it
// runs out, playback slows rather than skipping samples.

import { effectiveDistances, init } from "lefsim";

import { CtcfLanes, DEFAULT_CTCFS, EditableRun } from "./editable.ts";
import { BLUES, condensedIndex, drawColorbar, FALL, packedColors, packPixel, SquareMap } from "./heatmap.ts";
import { N_SITES, readRunInputs, worldOf } from "./model.ts";
import { $, fit, fmt, LoopView, MARGIN, onThemeChange, placeTooltip, siteAt, snapshotOf, xScale } from "./render.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
/** Simulation time between samples. */
const SAMPLE_DT = 1;
/** Wall-clock time per frame for sampling, in ms. */
const BUDGET_MS = 10;
/** Pairs always within the threshold along the lattice. */
const TRIVIAL = packPixel(208, 208, 204);

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const binIn = $<HTMLSelectElement>("bin");
const seedIn = $<HTMLInputElement>("seed");
const thresholdIn = $<HTMLInputElement>("threshold");
const thresholdOut = $<HTMLOutputElement>("threshold-out");
const summary = $<HTMLParagraphElement>("summary");
const laneCanvas = $<HTMLCanvasElement>("lanes");
const mapCanvas = $<HTMLCanvasElement>("map");
const mapTooltip = $("map-tooltip");
const colorbarCanvas = $<HTMLCanvasElement>("colorbar");
const resetBtn = $<HTMLButtonElement>("reset-stats");
const rTime = $("r-time");
const rBins = $("r-bins");
const rObserved = $("r-observed");
const rCompute = $("r-compute");
const rRate = $("r-rate");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
const map = new SquareMap(mapCanvas);
let laneCtx = fit(laneCanvas);
let colorbarCtx = fit(colorbarCanvas);

// ------------------------------------------------------------------ state

const run = new EditableRun(DEFAULT_CTCFS, {
  nLeft: $<HTMLInputElement>("nctcf-left"),
  nRight: $<HTMLInputElement>("nctcf-right"),
  step: $<HTMLInputElement>("rate-step"),
  respawn: $<HTMLInputElement>("rate-respawn"),
  respawnStalled: $<HTMLInputElement>("rate-respawn-stalled"),
  respawnHeld: $<HTMLInputElement>("rate-respawn-held"),
  strength: $<HTMLInputElement>("strength"),
});
run.onChange = () => {
  if (!playing) draw();
};
const lanes = new CtcfLanes(run, 0);

let playing = false;
let lastFrame: number | undefined;

/** Bins: their middle sites. */
let centers = new Uint32Array(0);
/** Per pair (condensed upper triangle): always within the threshold, in an encounter now, encounters started, samples in encounters. */
let trivial = new Uint8Array(0);
let inEncounter = new Uint8Array(0);
let starts = new Float64Array(0);
let samplesIn = new Float64Array(0);
/** Samples since the last reset, and when the next is due. */
let nSamples = 0;
let nextSample = 0;
/** Recent sampling cost and rate, for the readout. */
let sampleMs = 0;
let rateWindow: { t: number; samples: number }[] = [];

const freqColors = packedColors(FALL);
const durColors = packedColors(BLUES);
/** The scales the map was last painted with (log10), for the colorbars. */
let freqRange: [number, number] = [0, 1];
let durRange: [number, number] = [0, 1];

const threshold = () => Number(thresholdIn.value);

function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

const fmtMs = (ms: number) => (ms < 10 ? `${ms.toFixed(1)} ms` : `${fmt.format(ms)} ms`);
const fmtNum = (v: number) => (v >= 100 ? fmt.format(v) : v >= 1 ? v.toFixed(1) : v.toPrecision(2));

// ------------------------------------------------------------------ the totals

function setBins() {
  const bin = Number(binIn.value);
  const k = Math.ceil(N_SITES / bin);
  centers = Uint32Array.from({ length: k }, (_, i) => Math.min(N_SITES - 1, Math.floor(i * bin + bin / 2)));
  map.setSize(k);
  rBins.textContent = `${fmt.format(k)} × ${fmt.format(k)}, ${fmt.format((k * (k - 1)) / 2)} pairs`;
  resetStats();
}

/** Start the totals afresh, from the next sample. */
function resetStats() {
  const k = centers.length;
  const n = (k * (k - 1)) / 2;
  trivial = new Uint8Array(n);
  inEncounter = new Uint8Array(n);
  starts = new Float64Array(n);
  samplesIn = new Float64Array(n);
  const thr = threshold();
  let at = 0;
  for (let i = 0; i < k; i++) for (let j = i + 1; j < k; j++) trivial[at++] = centers[j]! - centers[i]! <= thr ? 1 : 0;
  nSamples = 0;
  nextSample = (run.sim?.time ?? 0) + SAMPLE_DT;
  rateWindow = [];
}

/** One sample of every pair at the sim's current time. */
function sample() {
  const started = performance.now();
  const dist = effectiveDistances(worldOf(run.sim!), centers, LENGTHS);
  const thr = threshold();
  for (let at = 0; at < dist.length; at++) {
    if (dist[at]! <= thr) {
      samplesIn[at]!++;
      if (!inEncounter[at]) {
        starts[at]!++;
        inEncounter[at] = 1;
      }
    } else {
      inEncounter[at] = 0;
    }
  }
  nSamples++;
  sampleMs = 0.9 * sampleMs + 0.1 * (performance.now() - started);
}

/**
 * Advance toward `target`, sampling on the way, within the frame's budget.
 * Stops at the last sample if the budget runs out, so the totals never skip
 * time. False if it stopped short.
 */
function advance(target: number): boolean {
  const sim = run.sim!;
  const deadline = performance.now() + BUDGET_MS;
  while (nextSample <= target) {
    if (performance.now() > deadline) return false;
    sim.runUntil(nextSample);
    sample();
    nextSample += SAMPLE_DT;
  }
  sim.runUntil(target);
  return true;
}

// ------------------------------------------------------------------ painting

/** Map a value's log10 onto a colormap index over `[lo, hi]`. */
function colorIndex(log: number, lo: number, hi: number): number {
  const c = Math.round(((log - lo) / (hi - lo)) * 255);
  return c < 0 ? 0 : c > 255 ? 255 : c;
}

/** Frequency below the diagonal, mean duration above; scales from the current totals. */
function paint() {
  const k = centers.length;
  const px = map.pixels;
  const observed = nSamples * SAMPLE_DT;
  // Scales: frequency from one encounter in the whole time up to the most
  // frequent pair; duration from one sample up to the longest mean.
  let maxStarts = 0;
  let maxMean = SAMPLE_DT;
  for (let at = 0; at < starts.length; at++) {
    if (trivial[at] || starts[at] === 0) continue;
    maxStarts = Math.max(maxStarts, starts[at]!);
    maxMean = Math.max(maxMean, (samplesIn[at]! * SAMPLE_DT) / starts[at]!);
  }
  const fLo = observed > 0 ? Math.log10(1 / observed) : -3;
  const fHi = observed > 0 && maxStarts > 1 ? Math.log10(maxStarts / observed) : fLo + 1;
  const dLo = Math.log10(SAMPLE_DT);
  const dHi = Math.max(dLo + 1, Math.log10(maxMean));
  freqRange = [fLo, fHi];
  durRange = [dLo, dHi];
  const empty = freqColors[0]!;
  let at = 0;
  for (let i = 0; i < k; i++) {
    px[i * k + i] = TRIVIAL;
    for (let j = i + 1; j < k; j++, at++) {
      if (trivial[at]) {
        px[i * k + j] = TRIVIAL;
        px[j * k + i] = TRIVIAL;
        continue;
      }
      const n = starts[at]!;
      if (n === 0 || observed === 0) {
        px[i * k + j] = empty;
        px[j * k + i] = empty;
        continue;
      }
      px[j * k + i] = freqColors[colorIndex(Math.log10(n / observed), fLo, fHi)]!;
      px[i * k + j] = durColors[colorIndex(Math.log10((samplesIn[at]! * SAMPLE_DT) / n), dLo, dHi)]!;
    }
  }
}

function drawColorbars() {
  const { width, height } = colorbarCanvas.getBoundingClientRect();
  const ctx = colorbarCtx;
  ctx.clearRect(0, 0, width, height);
  const barW = Math.min(260, width - MARGIN.left - MARGIN.right - 180);
  const x0 = width - MARGIN.right - barW;
  drawColorbar(ctx, { x0, y0: 8, width: barW, lut: FALL, lo: freqRange[0], hi: freqRange[1], label: "Encounters per time unit (lower)" });
  drawColorbar(ctx, { x0, y0: 44, width: barW, lut: BLUES, lo: durRange[0], hi: durRange[1], label: "Mean duration (upper)" });
}

function drawLanes() {
  const { width, height } = laneCanvas.getBoundingClientRect();
  laneCtx.clearRect(0, 0, width, height);
  lanes.draw(laneCtx, xScale(width));
}

function updateReadout(time: number) {
  rTime.textContent = fmt.format(time);
  rObserved.textContent = `${fmt.format(nSamples * SAMPLE_DT)} time units`;
  rCompute.textContent = nSamples ? fmtMs(sampleMs) : "–";
  const now = performance.now();
  rateWindow.push({ t: now, samples: nSamples });
  while (rateWindow.length > 2 && now - rateWindow[0]!.t > 1000) rateWindow.shift();
  if (rateWindow.length > 1) {
    const first = rateWindow[0]!;
    const perSecond = ((nSamples - first.samples) * 1000) / (now - first.t);
    rRate.textContent = `${fmt.format(perSecond)} / s`;
  }
}

function draw() {
  const sim = run.sim;
  if (!sim) return;
  const f = snapshotOf(sim.state());
  loops.show(f);
  drawLanes();
  paint();
  map.draw();
  drawColorbars();
  updateReadout(f.time);
}

// ------------------------------------------------------------------ loop

function restart() {
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  run.restart(nLefs, seed);
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, and for every pair of bins, how often their ` +
    `effective distance comes within the threshold and for how long. Simulated in your browser.`;
  lastFrame = undefined;
  resetStats();
  draw();
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  rateWindow = [];
  if (on) requestAnimationFrame(frame);
}

function frame(now: number) {
  const sim = run.sim;
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  behind.hidden = advance(sim.time + speed() * dt);
  draw();
  requestAnimationFrame(frame);
}

// ------------------------------------------------------------------ CTCF lanes

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
  if (!cell) {
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
  const head = `Sites ${fmt.format(a)} and ${fmt.format(b)}, ${fmt.format(b - a)} apart: `;
  const observed = nSamples * SAMPLE_DT;
  let text: string;
  if (trivial[at]) text = `${head}always within the threshold`;
  else if (starts[at] === 0) text = `${head}no encounters in ${fmt.format(observed)} time units`;
  else {
    const n = starts[at]!;
    const time = samplesIn[at]! * SAMPLE_DT;
    text =
      `${head}${fmt.format(n)} encounter${n === 1 ? "" : "s"} in ${fmt.format(observed)} time units ` +
      `(${fmtNum((1000 * n) / observed)} per 1,000), mean duration ${fmtNum(time / n)}, ` +
      `${Math.round((100 * time) / observed)}% of the time`;
  }
  placeTooltip(mapTooltip, mapCanvas, e, text);
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
  drawColorbars();
}).observe(mapCanvas);
onThemeChange(() => {
  loops.draw();
  drawLanes();
  map.draw();
  drawColorbars();
});

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  restart();
});
binIn.addEventListener("change", () => {
  setBins();
  if (!playing) draw();
});
thresholdIn.addEventListener("input", () => {
  thresholdOut.value = fmt.format(threshold());
  resetStats();
  if (!playing) draw();
});
resetBtn.addEventListener("click", () => {
  resetStats();
  if (!playing) draw();
});
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
thresholdOut.value = fmt.format(threshold());
setBins();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
