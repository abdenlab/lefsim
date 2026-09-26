// Live demo of the lefsim WebAssembly build: a Sim advanced every animation
// frame, drawn as loop arcs and a scrolling kymograph, plus a leg-occupancy
// profile streamed as Arrow record batches from a second, background Sim.

import { init, iterate, Sim } from "lefsim";
import { toRecordBatches } from "lefsim/arrow";

import { BOUNDARIES, N_SITES, options, readRunInputs } from "./model.ts";
import {
  $,
  colors,
  drawCtcfs,
  drawTadBands,
  fit,
  fmt,
  fmtCompact,
  Kymograph,
  LoopView,
  onThemeChange,
  placeTooltip,
  siteAt,
  snapshotOf,
  xScale,
  type Frame,
} from "./render.ts";

/** Cap on events per frame, so a burst can't stall the page. */
const MAX_EVENTS_PER_FRAME = 40_000;

/** Background run: snapshot spacing, and snapshots per streamed batch (one batch per frame). */
const BG_DT = 1;
const BG_CHUNK = 400;
/** Discard the background run's first stretch, while LEFs settle from random placement. */
const BG_BURN_IN = 2_000;

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const occCanvas = $<HTMLCanvasElement>("occ");
const occTooltip = $<HTMLDivElement>("occ-tooltip");
const occHint = $("occ-hint");
const rTime = $("r-time");
const rEvents = $("r-events");
const rRate = $("r-rate");
const rHeld = $("r-held");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
const kymograph = new Kymograph($<HTMLCanvasElement>("kymo"));

// ------------------------------------------------------------------ state

let sim: Sim | undefined;
let playing = false;
let lastFrame: number | undefined;
let rateWindow: { t: number; events: number }[] = [];

/** Sim time units per wall-clock second, from a log-scale slider. */
function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

function restart() {
  sim?.free();
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  sim = new Sim(options(nLefs, seed));
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, split into five TADs ` +
    `by convergent CTCF pairs. Simulated in your browser.`;
  rateWindow = [];
  lastFrame = undefined;
  kymograph.reset();
  draw(snapshotOf(sim.state()));
  startBackground(nLefs, seed + 1);
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  if (on) {
    requestAnimationFrame(frame);
    resumeBackground?.();
  }
}

// ------------------------------------------------------------------ loop

function frame(now: number) {
  if (!playing || !sim) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;

  const report = sim.runUntil(sim.time + speed() * dt, { maxEvents: MAX_EVENTS_PER_FRAME });
  behind.hidden = report.stopped !== "reached_event_count";

  rateWindow.push({ t: now, events: sim.eventCount });
  while (rateWindow.length > 2 && now - rateWindow[0]!.t > 1000) rateWindow.shift();

  draw(snapshotOf(sim.state()));
  requestAnimationFrame(frame);
}

function draw(f: Frame) {
  loops.show(f);
  kymograph.push(f);
  updateReadout(f);
}

// ------------------------------------------------------------------ background run

// A second Sim with the same model, streamed as Arrow record batches and
// averaged into a leg-occupancy profile. The stream only computes when read,
// so reading one batch per frame paces it, and pausing just stops reading.

let bgGeneration = 0;
let resumeBackground: (() => void) | undefined;
let occCounts = new Float64Array(N_SITES);
let occSnapshots = 0;
let occTime = 0;
let occ = fit(occCanvas);

const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const whilePaused = () =>
  playing ? Promise.resolve() : new Promise<void>((resolve) => (resumeBackground = resolve));

async function startBackground(nLefs: number, seed: number) {
  const generation = ++bgGeneration;
  occCounts = new Float64Array(N_SITES);
  occSnapshots = 0;
  occTime = 0;
  drawOccupancy();
  const bg = new Sim(options(nLefs, seed));
  try {
    const batches = bg
      .stream(Infinity, { dt: BG_DT, t0: BG_BURN_IN, chunkSize: BG_CHUNK })
      .pipeThrough(toRecordBatches());
    for await (const batch of iterate(batches)) {
      if (generation !== bgGeneration) break; // restarted: breaking cancels the stream
      const left = batch.getChild("left_pos")!.data[0]!.values as Uint32Array;
      const right = batch.getChild("right_pos")!.data[0]!.values as Uint32Array;
      for (let i = 0; i < left.length; i++) {
        occCounts[left[i]!]!++;
        occCounts[right[i]!]!++;
      }
      occSnapshots += nLefs > 0 ? left.length / nLefs : 0;
      occTime = bg.time;
      drawOccupancy();
      await nextFrame();
      await whilePaused();
      if (generation !== bgGeneration) break;
    }
  } finally {
    bg.free();
  }
}

const OCC_LOG_MIN = 1e-4;

function drawOccupancy() {
  const { width, height } = occCanvas.getBoundingClientRect();
  const ctx = occ;
  const x = xScale(width);
  const top = 16;
  const base = height - 18;
  ctx.clearRect(0, 0, width, height);
  drawTadBands(ctx, x, top, base);

  // Log scale: 1e-4 .. 1 legs per site, gridlines at each decade.
  const y = (v: number) =>
    base - ((Math.log10(Math.max(v, OCC_LOG_MIN)) - Math.log10(OCC_LOG_MIN)) / -Math.log10(OCC_LOG_MIN)) * (base - top);
  ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
  for (let e = -4; e <= 0; e++) {
    const gy = Math.round(y(10 ** e)) + 0.5;
    ctx.strokeStyle = colors.rule;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x(0), gy);
    ctx.lineTo(x(N_SITES - 1), gy);
    ctx.stroke();
  }
  const drawLabels = () => {
    // Labels last, with a paper-colored halo so the curve can't hide them.
    ctx.lineWidth = 3;
    ctx.lineJoin = "round";
    ctx.strokeStyle = colors.paper;
    ctx.fillStyle = colors.muted;
    for (let e = -4; e <= 0; e++) {
      const label = e === 0 ? "1" : `10⁻${"⁰¹²³⁴"[-e]}`;
      const gy = Math.round(y(10 ** e)) - 3;
      ctx.strokeText(label, x(0) + 3, gy);
      ctx.fillText(label, x(0) + 3, gy);
    }
  };
  drawCtcfs(ctx, x, base + 5, 8);

  if (occSnapshots === 0) {
    drawLabels();
    occHint.textContent = "mean legs per site, log scale; warming up…";
    return;
  }
  // One point per pixel column: the mean over the sites it covers.
  ctx.strokeStyle = colors.extruding;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  const x0 = x(0);
  const x1 = x(N_SITES - 1);
  for (let px = Math.floor(x0); px <= Math.ceil(x1); px++) {
    const lo = Math.max(0, Math.floor(((px - x0) / (x1 - x0)) * (N_SITES - 1)));
    const hi = Math.min(N_SITES - 1, Math.floor(((px + 1 - x0) / (x1 - x0)) * (N_SITES - 1)));
    let sum = 0;
    for (let s = lo; s <= hi; s++) sum += occCounts[s]!;
    const py = y(sum / (hi - lo + 1) / occSnapshots);
    if (px === Math.floor(x0)) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();
  drawLabels();
  occHint.textContent = `mean legs per site, log scale; ${fmtCompact.format(occTime - BG_BURN_IN)} time units averaged`;
}

occCanvas.addEventListener("pointermove", (e) => {
  if (occSnapshots === 0) return;
  const site = Math.round(siteAt(occCanvas, e.clientX));
  if (site < 0 || site >= N_SITES) {
    occTooltip.hidden = true;
    return;
  }
  const v = occCounts[site]! / occSnapshots;
  placeTooltip(occTooltip, occCanvas, e, `Site ${fmt.format(site)}: ${v < 0.001 ? v.toExponential(1) : v.toFixed(3)} legs on average`);
});
occCanvas.addEventListener("pointerleave", () => {
  occTooltip.hidden = true;
});

// ------------------------------------------------------------------ readout

function updateReadout(f: Frame) {
  rTime.textContent = fmt.format(f.time);
  rEvents.textContent = fmtCompact.format(sim?.eventCount ?? 0);
  if (rateWindow.length >= 2) {
    const a = rateWindow[0]!;
    const b = rateWindow[rateWindow.length - 1]!;
    const perSec = ((b.events - a.events) / (b.t - a.t)) * 1000;
    rRate.textContent = `${fmtCompact.format(perSec)} events/s`;
  } else {
    rRate.textContent = "–";
  }
  const nHeld = f.held.reduce((a, h) => a + h, 0);
  rHeld.textContent = `${nHeld} of ${f.held.length}`;
}

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  kymograph.resize();
  occ = fit(occCanvas);
  drawOccupancy();
}).observe($("arcs"));
onThemeChange(() => {
  loops.draw();
  kymograph.reset();
  drawOccupancy();
});

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  restart();
});
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
