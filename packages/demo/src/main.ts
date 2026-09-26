// Live demo of the lefsim WebAssembly build: a Sim advanced every animation
// frame, drawn as loop arcs and a scrolling kymograph.

import { init, MOBILITY, Sim, type LiveState, type SimOptions } from "lefsim";

// ------------------------------------------------------------------ model

const N_SITES = 3000;
const BOUNDARIES = [600, 1300, 1800, 2500];
const CTCFS = BOUNDARIES.flatMap((b) => [
  { site: b, capturesFrom: "left" as const, strength: 0.8 },
  { site: b + 1, capturesFrom: "right" as const, strength: 0.8 },
]);
const RATES = { step: 1, respawn: 0.002, respawnCaptured: 0.0004 };
/** Loop span (sites) drawn at full panel height: about the largest TAD. */
const FULL_HEIGHT_SPAN = 750;
const CAPTURED = MOBILITY.indexOf("Captured");

/** Cap on events per frame, so a burst can't stall the page. */
const MAX_EVENTS_PER_FRAME = 40_000;

function options(nLefs: number, seed: number): SimOptions {
  return { nSites: N_SITES, ctcfs: CTCFS, nLefs, seed, rates: RATES };
}

// ------------------------------------------------------------------ DOM

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const playBtn = $<HTMLButtonElement>("play");
const restartBtn = $<HTMLButtonElement>("restart");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const arcsCanvas = $<HTMLCanvasElement>("arcs");
const kymoCanvas = $<HTMLCanvasElement>("kymo");
const tooltip = $<HTMLDivElement>("tooltip");
const rTime = $("r-time");
const rEvents = $("r-events");
const rRate = $("r-rate");
const rHeld = $("r-held");
const behind = $("behind");

const fmt = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });
const fmtCompact = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });

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
let colors = palette();
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  colors = palette();
  resetKymograph();
});

/** Size a canvas's backing store to its CSS box at device resolution. */
function fit(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const dpr = window.devicePixelRatio || 1;
  const { width, height } = canvas.getBoundingClientRect();
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

// ------------------------------------------------------------------ state

let sim: Sim | undefined;
let playing = false;
let lastFrame: number | undefined;
let rateWindow: { t: number; events: number }[] = [];
let lastState: { leftPos: Uint32Array; rightPos: Uint32Array; held: Uint8Array } | undefined;

/** Sim time units per wall-clock second, from a log-scale slider. */
function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

function restart() {
  sim?.free();
  const nLefs = Math.max(0, Math.min(700, Math.floor(Number(nLefsIn.value) || 0)));
  const seed = Math.max(0, Math.floor(Number(seedIn.value) || 0));
  nLefsIn.value = String(nLefs);
  seedIn.value = String(seed);
  sim = new Sim(options(nLefs, seed));
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, split into five TADs ` +
    `by convergent CTCF pairs. Simulated in your browser.`;
  rateWindow = [];
  lastFrame = undefined;
  resetKymograph();
  draw(sim.state());
}

function setPlaying(on: boolean) {
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  if (on) requestAnimationFrame(frame);
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

  draw(sim.state());
  requestAnimationFrame(frame);
}

function draw(state: LiveState) {
  // Copy out of wasm memory: the views die at the next call into the sim,
  // and hover needs this state between frames.
  const n = state.leftPos.length;
  const held = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    held[i] = state.leftMobility[i] === CAPTURED || state.rightMobility[i] === CAPTURED ? 1 : 0;
  }
  lastState = { leftPos: state.leftPos.slice(), rightPos: state.rightPos.slice(), held };

  drawArcs();
  drawKymographRow();
  updateReadout(state.time);
}

// ------------------------------------------------------------------ arcs

let arcs = fit(arcsCanvas);
const MARGIN = { left: 8, right: 8, bottom: 30, top: 10 };

function xScale(width: number) {
  const inner = width - MARGIN.left - MARGIN.right;
  return (site: number) => MARGIN.left + (site / (N_SITES - 1)) * inner;
}

function drawArcs() {
  const { width, height } = arcsCanvas.getBoundingClientRect();
  const ctx = arcs;
  const x = xScale(width);
  const base = height - MARGIN.bottom;
  ctx.clearRect(0, 0, width, height);

  // TADs as alternating bands.
  const edges = [0, ...BOUNDARIES.map((b) => b + 0.5), N_SITES - 1];
  for (let i = 0; i < edges.length - 1; i++) {
    if (i % 2 === 1) continue;
    ctx.fillStyle = colors.band;
    ctx.fillRect(x(edges[i]!), MARGIN.top, x(edges[i + 1]!) - x(edges[i]!), base - MARGIN.top);
  }

  // The lattice.
  ctx.strokeStyle = colors.ink2;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(x(0), base + 0.5);
  ctx.lineTo(x(N_SITES - 1), base + 0.5);
  ctx.stroke();

  // CTCFs: chevrons pointing toward the side each one captures from.
  ctx.fillStyle = colors.ink2;
  for (const c of CTCFS) {
    const cx = x(c.site) + (c.capturesFrom === "left" ? -2 : 2);
    const dir = c.capturesFrom === "left" ? -1 : 1;
    ctx.beginPath();
    ctx.moveTo(cx + dir * 6, base + 11);
    ctx.lineTo(cx, base + 6);
    ctx.lineTo(cx, base + 16);
    ctx.closePath();
    ctx.fill();
  }

  if (!lastState) return;
  const { leftPos, rightPos, held } = lastState;
  const maxH = base - MARGIN.top - 4;

  // Loops as half-ellipses whose height grows with span, so a whole-TAD
  // loop nearly fills the panel. Height is capped for the rare loop that
  // crosses a boundary.
  ctx.lineWidth = 2;
  for (const pass of [0, 1]) {
    // Held loops on top: they are the ones the boundaries are shaping.
    ctx.strokeStyle = pass ? colors.held : colors.extruding;
    ctx.beginPath();
    for (let i = 0; i < leftPos.length; i++) {
      if (held[i] !== pass) continue;
      const x1 = x(leftPos[i]!);
      const x2 = x(rightPos[i]!);
      const r = (x2 - x1) / 2;
      const span = rightPos[i]! - leftPos[i]!;
      const h = Math.min(maxH, Math.max(2, (maxH * span) / FULL_HEIGHT_SPAN));
      ctx.moveTo(x1, base);
      ctx.ellipse(x1 + r, base, Math.max(r, 0.5), h, 0, Math.PI, 2 * Math.PI);
    }
    ctx.stroke();
  }

  // Legs as ticks on the lattice.
  for (let i = 0; i < leftPos.length; i++) {
    ctx.fillStyle = held[i] ? colors.held : colors.extruding;
    ctx.fillRect(x(leftPos[i]!) - 1, base - 4, 2, 5);
    ctx.fillRect(x(rightPos[i]!) - 1, base - 4, 2, 5);
  }
}

// Hover: the innermost loop containing the pointer.
arcsCanvas.addEventListener("pointermove", (e) => {
  if (!lastState) return;
  const rect = arcsCanvas.getBoundingClientRect();
  const inner = rect.width - MARGIN.left - MARGIN.right;
  const site = ((e.clientX - rect.left - MARGIN.left) / inner) * (N_SITES - 1);
  const { leftPos, rightPos, held } = lastState;
  let best = -1;
  for (let i = 0; i < leftPos.length; i++) {
    if (leftPos[i]! - 4 <= site && site <= rightPos[i]! + 4) {
      if (best < 0 || rightPos[i]! - leftPos[i]! < rightPos[best]! - leftPos[best]!) best = i;
    }
  }
  if (best < 0) {
    tooltip.hidden = true;
    return;
  }
  const l = leftPos[best]!;
  const r = rightPos[best]!;
  tooltip.textContent = `LEF ${best}: sites ${fmt.format(l)}–${fmt.format(r)}, ${fmt.format(r - l)} long, ${
    held[best] ? "held at a boundary" : "extruding"
  }`;
  tooltip.hidden = false;
  const tx = Math.min(e.clientX - rect.left + 12, rect.width - tooltip.offsetWidth - 4);
  tooltip.style.left = `${Math.max(4, tx)}px`;
  tooltip.style.top = `${Math.max(4, e.clientY - rect.top - 36)}px`;
});
arcsCanvas.addEventListener("pointerleave", () => {
  tooltip.hidden = true;
});

// ------------------------------------------------------------------ kymograph

let kymo = fit(kymoCanvas);

function resetKymograph() {
  const { width, height } = kymoCanvas.getBoundingClientRect();
  kymo.fillStyle = colors.paper;
  kymo.fillRect(0, 0, width, height);
}

/** Scroll the kymograph down one row and draw the current legs at the top. */
function drawKymographRow() {
  if (!lastState) return;
  const { width, height } = kymoCanvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = xScale(width);
  const row = 1;
  // Shift the backing store down by `row` CSS pixels.
  kymo.save();
  kymo.setTransform(1, 0, 0, 1, 0, 0);
  kymo.drawImage(kymoCanvas, 0, 0, kymoCanvas.width, kymoCanvas.height - row * dpr, 0, row * dpr,
    kymoCanvas.width, kymoCanvas.height - row * dpr);
  kymo.restore();
  kymo.fillStyle = colors.paper;
  kymo.fillRect(0, 0, width, row);

  // Faint TAD boundaries, so domains read in the kymograph too.
  kymo.fillStyle = colors.rule;
  for (const b of BOUNDARIES) kymo.fillRect(x(b + 0.5), 0, 1, row);

  const { leftPos, rightPos, held } = lastState;
  for (let i = 0; i < leftPos.length; i++) {
    kymo.fillStyle = held[i] ? colors.held : colors.extruding;
    kymo.fillRect(x(leftPos[i]!) - 1, 0, 2, row);
    kymo.fillRect(x(rightPos[i]!) - 1, 0, 2, row);
  }
  void height;
}

// ------------------------------------------------------------------ readout

function updateReadout(time: number) {
  rTime.textContent = fmt.format(time);
  rEvents.textContent = fmtCompact.format(sim?.eventCount ?? 0);
  if (rateWindow.length >= 2) {
    const a = rateWindow[0]!;
    const b = rateWindow[rateWindow.length - 1]!;
    const perSec = ((b.events - a.events) / (b.t - a.t)) * 1000;
    rRate.textContent = `${fmtCompact.format(perSec)} events/s`;
  } else {
    rRate.textContent = "–";
  }
  if (lastState) {
    const nHeld = lastState.held.reduce((a, h) => a + h, 0);
    rHeld.textContent = `${nHeld} of ${lastState.held.length}`;
  }
}

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  arcs = fit(arcsCanvas);
  kymo = fit(kymoCanvas);
  resetKymograph();
  drawArcs();
}).observe(arcsCanvas);

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
