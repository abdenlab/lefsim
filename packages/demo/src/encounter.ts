// Encounters demo: a live Sim with two probe sites, whose effective distance
// is tracked over time. While it's at most a threshold, the probes are in an
// encounter; the durations of finished encounters build a histogram.
//
// The distance is sampled every SAMPLE_DT of simulation time, not once per
// animation frame, so durations don't depend on the playback speed (and are
// resolved to SAMPLE_DT). The kymograph and the distance trace beside it get
// one row per frame, aligned, newest at top. The CTCFs, rates and strength
// are editable while it runs (see `editable.ts`).

import { effectiveDistance, init } from "lefsim";

import { CtcfLanes, DEFAULT_CTCFS, EditableRun } from "./editable.ts";
import { N_SITES, readRunInputs, worldOf } from "./model.ts";
import { $, colors, fit, fmt, Kymograph, LoopView, onThemeChange, siteAt, snapshotOf, xScale } from "./render.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
/** Simulation time between distance samples. */
const SAMPLE_DT = 1;
/** Cap on samples per frame, so a fast speed can't stall the page. */
const MAX_SAMPLES_PER_FRAME = 4_000;
/** Histogram bins: durations from SAMPLE_DT up, each bin twice as wide as the last. */
const N_BINS = 16;

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const seedIn = $<HTMLInputElement>("seed");
const thresholdIn = $<HTMLInputElement>("threshold");
const thresholdOut = $<HTMLOutputElement>("threshold-out");
const summary = $<HTMLParagraphElement>("summary");
const trackCanvas = $<HTMLCanvasElement>("tracks");
const histCanvas = $<HTMLCanvasElement>("hist");
const traceCanvas = $<HTMLCanvasElement>("trace");
const resetBtn = $<HTMLButtonElement>("reset-stats");
const rTime = $("r-time");
const rDistance = $("r-distance");
const rCount = $("r-count");
const rMean = $("r-mean");
const rFraction = $("r-fraction");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
const kymograph = new Kymograph($<HTMLCanvasElement>("kymo"));
let trackCtx = fit(trackCanvas);
let histCtx = fit(histCanvas);
let traceCtx = fit(traceCanvas);

// ------------------------------------------------------------------ state

const run = new EditableRun(DEFAULT_CTCFS, {
  nLeft: $<HTMLInputElement>("nctcf-left"),
  nRight: $<HTMLInputElement>("nctcf-right"),
  step: $<HTMLInputElement>("rate-step"),
  respawn: $<HTMLInputElement>("rate-respawn"),
  respawnBlocked: $<HTMLInputElement>("rate-respawn-blocked"),
  respawnHeld: $<HTMLInputElement>("rate-respawn-held"),
  strength: $<HTMLInputElement>("strength"),
});
run.onChange = () => {
  if (!playing) draw(false);
};

let playing = false;
let lastFrame: number | undefined;

/** The two probe sites. */
const probes: [number, number] = [900, 2100];
let hoveredProbe = -1;
let draggingProbe = -1;

/** The latest sample, and when the next is due. */
let distance = 0;
let nextSample = 0;
/** Whether the probes are in an encounter, and since when. */
let inEncounter = false;
let encounterStart = 0;
/** Whether any sample since the last kymograph row was in an encounter. */
let rowEncounter = false;
/** The row's smallest and largest distance, for the trace. */
let rowMin = Infinity;
let rowMax = -Infinity;
/** The trace's previous row, to join to, and rows drawn (for the dotted threshold). */
let lastTraceX: number | undefined;
let traceRows = 0;

/** Statistics since the last reset: finished encounters by duration bin, and time observed. */
let counts = new Float64Array(N_BINS);
let nEncounters = 0;
let totalDuration = 0;
let observedTime = 0;
let encounterTime = 0;

const threshold = () => Number(thresholdIn.value);

function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

function showThreshold() {
  thresholdOut.value = fmt.format(threshold());
}

// ------------------------------------------------------------------ sampling

function sampleDistance(): number {
  return effectiveDistance(worldOf(run.sim!), probes[0], probes[1], LENGTHS);
}

/** Start the statistics afresh from the current state. */
function resetStats() {
  counts = new Float64Array(N_BINS);
  nEncounters = 0;
  totalDuration = 0;
  observedTime = 0;
  encounterTime = 0;
  const sim = run.sim;
  if (!sim) return;
  distance = sampleDistance();
  inEncounter = distance <= threshold();
  encounterStart = sim.time;
  nextSample = sim.time + SAMPLE_DT;
}

/** The histogram bin of a duration: [1, 2), [2, 4), … in units of SAMPLE_DT. */
function binOf(duration: number): number {
  return Math.max(0, Math.min(N_BINS - 1, Math.floor(Math.log2(duration / SAMPLE_DT))));
}

/** Record a sample at time `t`. */
function sample(t: number) {
  distance = sampleDistance();
  const now = distance <= threshold();
  observedTime += SAMPLE_DT;
  if (now) encounterTime += SAMPLE_DT;
  if (now && !inEncounter) encounterStart = t;
  if (!now && inEncounter) {
    // The encounter ended at this sample: it lasted from its first sample to here.
    const duration = t - encounterStart;
    counts[binOf(duration)]!++;
    nEncounters++;
    totalDuration += duration;
  }
  inEncounter = now;
  rowEncounter ||= now;
  rowMin = Math.min(rowMin, distance);
  rowMax = Math.max(rowMax, distance);
}

/** Advance the sim to `target`, sampling on the way. False if the per-frame cap cut it short. */
function advance(target: number): boolean {
  const sim = run.sim!;
  let n = 0;
  while (nextSample <= target) {
    if (n++ >= MAX_SAMPLES_PER_FRAME) return false;
    sim.runUntil(nextSample);
    sample(nextSample);
    nextSample += SAMPLE_DT;
  }
  sim.runUntil(target);
  return true;
}

// ------------------------------------------------------------------ drawing

const PROBE_ROW = { top: 0, bottom: 40 };
const lanes = new CtcfLanes(run, PROBE_ROW.bottom);

function drawTracks() {
  const { width, height } = trackCanvas.getBoundingClientRect();
  const ctx = trackCtx;
  const x = xScale(width);
  ctx.clearRect(0, 0, width, height);
  lanes.draw(ctx, x);
  probes.forEach((s, i) => {
    const px = x(s);
    ctx.strokeStyle = colors.probe;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(px, PROBE_ROW.top);
    ctx.lineTo(px, PROBE_ROW.bottom - 14);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(px, PROBE_ROW.bottom - 9, i === hoveredProbe || i === draggingProbe ? 7 : 5.5, 0, 2 * Math.PI);
    ctx.fillStyle = colors.probe;
    ctx.fill();
  });
}

/** The probes' columns, and a band between them while in an encounter, under a kymograph row. */
function underRow(ctx: CanvasRenderingContext2D, x: (site: number) => number) {
  const [a, b] = [x(Math.min(...probes)), x(Math.max(...probes))];
  if (rowEncounter) {
    ctx.globalAlpha = 0.35;
    ctx.fillStyle = colors.probe;
    ctx.fillRect(a, 0, b - a, 1);
    ctx.globalAlpha = 1;
  }
  ctx.fillStyle = colors.probe;
  ctx.fillRect(a - 1, 0, 2, 1);
  ctx.fillRect(b - 1, 0, 2, 1);
}

/** The distance scale: up to the probes' separation, which no distance exceeds. */
function traceX(d: number, width: number): number {
  const pad = 10;
  const max = Math.max(1, Math.abs(probes[1] - probes[0]));
  return pad + (Math.min(d, max) / max) * (width - 2 * pad);
}

/** One trace row, scrolled in like the kymograph's. */
function pushTrace() {
  const canvas = traceCanvas;
  const ctx = traceCtx;
  const { width } = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.drawImage(canvas, 0, 0, canvas.width, canvas.height - dpr, 0, dpr, canvas.width, canvas.height - dpr);
  ctx.restore();
  ctx.fillStyle = colors.paper;
  ctx.fillRect(0, 0, width, 1);
  if (rowEncounter) {
    ctx.globalAlpha = 0.15;
    ctx.fillStyle = colors.probe;
    ctx.fillRect(0, 0, width, 1);
    ctx.globalAlpha = 1;
  }
  // The threshold, as a dotted line.
  const tx = traceX(threshold(), width);
  ctx.fillStyle = colors.muted;
  if (traceRows++ % 4 < 2) ctx.fillRect(tx, 0, 1, 1);
  // The row's range of distances, joined to the last row's.
  const lo = rowMin === Infinity ? distance : rowMin;
  const hi = rowMax === -Infinity ? distance : rowMax;
  let x0 = traceX(lo, width);
  let x1 = traceX(hi, width);
  if (lastTraceX !== undefined) {
    x0 = Math.min(x0, lastTraceX);
    x1 = Math.max(x1, lastTraceX);
  }
  ctx.fillStyle = rowEncounter ? colors.probe : colors.ink2;
  ctx.fillRect(x0 - 1, 0, Math.max(2, x1 - x0 + 2), 1);
  lastTraceX = traceX(distance, width);
}

function resetTrace() {
  const { width, height } = traceCanvas.getBoundingClientRect();
  traceCtx.fillStyle = colors.paper;
  traceCtx.fillRect(0, 0, width, height);
  lastTraceX = undefined;
}

/** The trace's heading gives its scale, which follows the probes' separation. */
function drawTraceAxis() {
  const label = $("trace-title");
  label.innerHTML = `Effective distance <span class="hint">0–${fmt.format(Math.abs(probes[1] - probes[0]))}</span>`;
}

function drawHistogram() {
  const { width, height } = histCanvas.getBoundingClientRect();
  const ctx = histCtx;
  ctx.clearRect(0, 0, width, height);
  const left = 36;
  const right = 10;
  const top = 10;
  const bottom = height - 34;
  const max = Math.max(1, ...counts);
  const barW = (width - left - right) / N_BINS;
  ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";

  // Gridlines at round counts.
  const step = niceStep(max / 4);
  ctx.fillStyle = colors.muted;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  for (let v = 0; v <= max; v += step) {
    const y = bottom - (v / max) * (bottom - top);
    ctx.fillStyle = colors.rule;
    ctx.fillRect(left, Math.round(y), width - left - right, 1);
    ctx.fillStyle = colors.muted;
    ctx.fillText(fmt.format(v), left - 6, y);
  }

  // Bars.
  ctx.fillStyle = colors.probe;
  for (let b = 0; b < N_BINS; b++) {
    const h = (counts[b]! / max) * (bottom - top);
    ctx.fillRect(left + b * barW + 1, bottom - h, barW - 2, h);
  }

  // Duration axis: bin edges are powers of two; label the powers of ten among them roughly.
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  ctx.fillStyle = colors.muted;
  for (let b = 0; b <= N_BINS; b += 2) {
    const edge = SAMPLE_DT * 2 ** b;
    ctx.fillRect(left + b * barW, bottom, 1, 4);
    ctx.fillText(fmtCompactDuration(edge), left + b * barW, bottom + 6);
  }
  ctx.fillText("duration (time units, log scale)", left + (width - left - right) / 2, bottom + 20);
}

function niceStep(target: number): number {
  const p = 10 ** Math.floor(Math.log10(Math.max(target, 1)));
  return [1, 2, 5, 10].map((m) => m * p).find((v) => v >= target) ?? p;
}

function fmtCompactDuration(v: number): string {
  return new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 0 }).format(v);
}

function updateReadout(time: number) {
  rTime.textContent = fmt.format(time);
  rDistance.textContent = `${fmt.format(distance)}${inEncounter ? ", in an encounter" : ""}`;
  rCount.textContent = fmt.format(nEncounters);
  rMean.textContent = nEncounters ? `${fmt.format(totalDuration / nEncounters)} time units` : "–";
  rFraction.textContent = observedTime ? `${Math.round((100 * encounterTime) / observedTime)}%` : "–";
}

/** Draw the current state; `row` adds a kymograph and trace row. */
function draw(row: boolean) {
  const sim = run.sim;
  if (!sim) return;
  const f = snapshotOf(sim.state());
  loops.show(f);
  drawTracks();
  if (row) {
    if (rowMin === Infinity) rowEncounter = inEncounter; // no sample this frame: keep the current state
    kymograph.push(f, underRow);
    pushTrace();
    rowEncounter = false;
    rowMin = Infinity;
    rowMax = -Infinity;
  }
  drawHistogram();
  updateReadout(f.time);
}

// ------------------------------------------------------------------ loop

function restart() {
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  run.restart(nLefs, seed);
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, and two probe sites whose effective distance ` +
    `is followed over time: within the threshold, they're in an encounter. Simulated in your browser.`;
  lastFrame = undefined;
  kymograph.reset();
  resetTrace();
  resetStats();
  draw(true);
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  if (on) requestAnimationFrame(frame);
}

function frame(now: number) {
  const sim = run.sim;
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  behind.hidden = advance(sim.time + speed() * dt);
  draw(true);
  requestAnimationFrame(frame);
}

/** After a probe or threshold change: fresh statistics and a fresh trace. */
function definitionChanged() {
  resetStats();
  resetTrace();
  drawTraceAxis();
  if (!playing) draw(false);
}

// ------------------------------------------------------------------ dragging and hover on the tracks

function markerAt(e: PointerEvent): { probe: number; ctcf: number } {
  const rect = trackCanvas.getBoundingClientRect();
  const x = xScale(rect.width);
  const mx = e.clientX - rect.left;
  const my = e.clientY - rect.top;
  if (my >= PROBE_ROW.bottom) return { probe: -1, ctcf: lanes.at(mx, my, x) };
  let best = -1;
  let bestPx = 12;
  probes.forEach((s, i) => {
    const px = Math.abs(x(s) - mx);
    if (px < bestPx) [best, bestPx] = [i, px];
  });
  return { probe: best, ctcf: -1 };
}

function siteUnder(e: PointerEvent): number {
  return Math.round(Math.max(0, Math.min(N_SITES - 1, siteAt(trackCanvas, e.clientX))));
}

trackCanvas.addEventListener("pointerdown", (e) => {
  const m = markerAt(e);
  draggingProbe = m.probe;
  lanes.dragging = m.ctcf;
  if (draggingProbe < 0 && lanes.dragging < 0) return;
  trackCanvas.setPointerCapture(e.pointerId);
  trackCanvas.style.cursor = "grabbing";
  e.preventDefault();
});
trackCanvas.addEventListener("pointermove", (e) => {
  if (draggingProbe >= 0) {
    const site = siteUnder(e);
    if (site === probes[draggingProbe] || site === probes[1 - draggingProbe]) return; // one probe per site
    probes[draggingProbe] = site;
    definitionChanged();
    return;
  }
  if (lanes.dragging >= 0) {
    if (run.moveCtcf(lanes.dragging, siteUnder(e)) && !playing) draw(false);
    return;
  }
  const m = markerAt(e);
  trackCanvas.style.cursor = m.probe >= 0 || m.ctcf >= 0 ? "grab" : "";
  if (m.probe !== hoveredProbe || m.ctcf !== lanes.hovered) {
    hoveredProbe = m.probe;
    lanes.hovered = m.ctcf;
    drawTracks();
  }
});
const endDrag = () => {
  draggingProbe = -1;
  lanes.dragging = -1;
  trackCanvas.style.cursor = "";
  drawTracks();
};
trackCanvas.addEventListener("pointerup", endDrag);
trackCanvas.addEventListener("pointercancel", endDrag);
trackCanvas.addEventListener("pointerleave", () => {
  if (draggingProbe < 0 && lanes.dragging < 0 && (hoveredProbe >= 0 || lanes.hovered >= 0)) {
    hoveredProbe = -1;
    lanes.hovered = -1;
    drawTracks();
  }
});

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  kymograph.resize();
  trackCtx = fit(trackCanvas);
  traceCtx = fit(traceCanvas);
  histCtx = fit(histCanvas);
  resetTrace();
  if (run.sim) draw(false);
}).observe($("arcs"));
new ResizeObserver(() => {
  histCtx = fit(histCanvas);
  drawHistogram();
}).observe(histCanvas);
onThemeChange(() => {
  kymograph.reset();
  resetTrace();
  draw(false);
});

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  restart();
});
thresholdIn.addEventListener("input", () => {
  showThreshold();
  definitionChanged();
});
resetBtn.addEventListener("click", () => {
  resetStats();
  if (!playing) draw(false);
});
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
showThreshold();
drawTraceAxis();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
