// Effective distance demo: a live Sim, a handful of probe sites on its
// lattice, and the probes laid out so that their spacing follows the
// effective distances between them (walking the lattice, or hopping across
// loops), recomputed every frame. Steps and hops both have length 1.
//
// The layout minimizes stress: each pair of probes is a spring whose rest
// length is its effective distance, relaxed a few sweeps per frame from the
// previous positions, so it moves smoothly as loops grow and dissolve.
//
// The CTCFs are editable too, in two lanes by orientation. Changing them
// builds a new lattice and carries the LEFs over to it at the current time,
// so the run continues rather than restarting.

import { createLattice, createModel, createWorld, effectiveDistances, init, MOBILITY, populate, Sim, type Lattice } from "lefsim";

import { CTCFS, N_SITES, RATES, readRunInputs } from "./model.ts";
import { $, colors, fit, fmt, LoopView, onThemeChange, setLattice, siteAt, snapshotOf, xScale } from "./render.ts";

const LENGTHS = { stepLength: 1, hopLength: 1 };
const CTCF_STRENGTH = 0.8;
const MAX_CTCFS = 16;
const CAPTURED = MOBILITY.indexOf("Captured");
const MOVING = MOBILITY.indexOf("Moving");
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
const nLeftIn = $<HTMLInputElement>("nctcf-left");
const nRightIn = $<HTMLInputElement>("nctcf-right");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const trackCanvas = $<HTMLCanvasElement>("tracks");
const fitViewIn = $<HTMLInputElement>("fit-view");
const scaleIn = $<HTMLInputElement>("scale");
const scaleOut = $<HTMLOutputElement>("scale-out");
const graphCanvas = $<HTMLCanvasElement>("graph");
const rTime = $("r-time");
const rClosest = $("r-closest");
const rFit = $("r-fit");
const behind = $("behind");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
let trackCtx = fit(trackCanvas);
let graphCtx = fit(graphCanvas);

// ------------------------------------------------------------------ state

let sim: Sim | undefined;
let seed = 0;
/** Lattice rebuilds since the last restart; varies the continued run's seed. */
let rebuilds = 0;
let playing = false;
let lastFrame: number | undefined;

/** The CTCFs; `left` means it captures from the left (its chevron points left). */
let ctcfs: { site: number; left: boolean }[] = CTCFS.map((c) => ({ site: c.site, left: c.capturesFrom === "left" }));
let hoveredCtcf = -1;
let draggingCtcf = -1;

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
  // Ease the view, so the picture pans and zooms rather than jumps. Fitted,
  // no probe leaves it; at a fixed scale, it only follows the layout's center.
  if (!view) view = { ...target, scale: fitView() ? target.scale : fixedScale() };
  else {
    view.cx += 0.15 * (target.cx - view.cx);
    view.cy += 0.15 * (target.cy - view.cy);
    if (fitView()) view.scale = Math.min(view.scale + 0.15 * (target.scale - view.scale), target.scale * 1.02);
    else view.scale += 0.3 * (fixedScale() - view.scale);
  }
  if (fitView()) showScale(view.scale);
}

// ------------------------------------------------------------------ scale

const fitView = () => fitViewIn.checked;
/** The fixed scale, in pixels per step, from a log-scale slider. */
const fixedScale = () => 10 ** Number(scaleIn.value);

function showScale(pxPerStep: number) {
  scaleOut.value = `${fmt.format(100 * pxPerStep)} px per 100 steps`;
  // While fitted, the slider follows along, so unticking keeps the current scale.
  if (fitView()) scaleIn.value = String(Math.log10(pxPerStep));
}

function setFixedScale(log10Scale: number) {
  const min = Number(scaleIn.min);
  const max = Number(scaleIn.max);
  scaleIn.value = String(Math.max(min, Math.min(max, log10Scale)));
  fitViewIn.checked = false;
  showScale(fixedScale());
  if (!playing) settle(20);
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

// ------------------------------------------------------------------ tracks

/** The tracks under the loop panel, top to bottom: probes, then a lane per CTCF orientation. */
const PROBE_ROW = { top: 0, bottom: 40 };
const CTCF_ROWS = { left: { top: 40, bottom: 66 }, right: { top: 66, bottom: 92 } };

/** Probe markers and CTCF lanes, aligned with the loop panel. */
function drawTracks() {
  const { width, height } = trackCanvas.getBoundingClientRect();
  const ctx = trackCtx;
  const x = xScale(width);
  ctx.clearRect(0, 0, width, height);

  // Lanes: a faint band each; the chevrons in it say which way it faces.
  for (const row of [CTCF_ROWS.left, CTCF_ROWS.right]) {
    ctx.fillStyle = colors.band;
    ctx.fillRect(x(0), row.top + 2, x(N_SITES - 1) - x(0), row.bottom - row.top - 4);
  }

  // CTCF chevrons, pointing toward the side each captures from.
  ctcfs.forEach((c, i) => {
    const row = c.left ? CTCF_ROWS.left : CTCF_ROWS.right;
    const cy = (row.top + row.bottom) / 2;
    const size = i === hoveredCtcf || i === draggingCtcf ? 16 : 12;
    const dir = c.left ? -1 : 1;
    const px = x(c.site);
    ctx.beginPath();
    ctx.moveTo(px + dir * size * 0.6, cy);
    ctx.lineTo(px, cy - size / 2);
    ctx.lineTo(px, cy + size / 2);
    ctx.closePath();
    ctx.fillStyle = colors.ink2;
    ctx.fill();
  });

  // Probes: a stem up to the lattice, and a handle.
  probes.forEach((s, i) => {
    const px = x(s);
    ctx.strokeStyle = probeColor(i);
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(px, PROBE_ROW.top);
    ctx.lineTo(px, PROBE_ROW.bottom - 14);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(px, PROBE_ROW.bottom - 9, i === hovered || i === dragging ? 7 : 5.5, 0, 2 * Math.PI);
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
  drawTracks();
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

function currentLattice(): Lattice {
  return createLattice(
    N_SITES,
    ctcfs.map((c) => ({ site: c.site, capturesFrom: c.left ? "left" : "right", strength: CTCF_STRENGTH })),
  );
}

function restart() {
  sim?.free();
  const inputs = readRunInputs(nLefsIn, seedIn);
  seed = inputs.seed;
  rebuilds = 0;
  const lattice = currentLattice();
  sim = new Sim(createModel(populate(lattice, inputs.nLefs, { seed }), RATES), { seed });
  setLattice(lattice);
  setProbes(readProbeCount());
  summary.textContent =
    `${inputs.nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, and ${probes.length} probe sites laid out ` +
    `by how far apart the loops make them. Simulated in your browser.`;
  lastFrame = undefined;
  draw();
}

/**
 * After a CTCF change: the same LEFs on the new lattice, continuing from the
 * current time. A captured leg stays captured only if a CTCF that can hold
 * it is still at its site (a left leg is held by one capturing from the
 * right, a right leg by one capturing from the left).
 */
function rebuildLattice() {
  const old = sim;
  if (!old) return;
  // Copy the state first: the next call into wasm can detach its views.
  const st = old.state();
  const time = old.time;
  const leftPos = st.leftPos.slice();
  const rightPos = st.rightPos.slice();
  const leftMob = st.leftMobility.slice();
  const rightMob = st.rightMobility.slice();
  const capturesLeftAt = new Map(ctcfs.map((c) => [c.site, c.left]));
  const keep = (mob: number, site: number, needsLeft: boolean) =>
    mob === CAPTURED && capturesLeftAt.get(site) === needsLeft ? CAPTURED : MOVING;
  const lattice = currentLattice();
  const world = createWorld(lattice, {
    leftPos,
    rightPos,
    leftMobility: Array.from(leftMob, (m, i) => keep(m, leftPos[i]!, false)),
    rightMobility: Array.from(rightMob, (m, i) => keep(m, rightPos[i]!, true)),
  });
  sim = new Sim(createModel(world, RATES), { seed: seed + ++rebuilds, startTime: time });
  old.free();
  setLattice(lattice);
}

/** Add or remove CTCFs of one orientation to match its count input. */
function setCtcfCount(left: boolean, input: HTMLInputElement) {
  const n = Math.max(0, Math.min(MAX_CTCFS, Math.floor(Number(input.value) || 0)));
  input.value = String(n);
  let lane = ctcfs.filter((c) => c.left === left);
  while (lane.length > n) {
    const last = lane.at(-1)!;
    ctcfs = ctcfs.filter((c) => c !== last);
    lane = lane.slice(0, -1);
  }
  while (lane.length < n) {
    // A new CTCF goes in the middle of the widest stretch without one.
    const sites = [-1, ...ctcfs.map((c) => c.site).sort((a, b) => a - b), N_SITES];
    let best = 0;
    for (let i = 1; i < sites.length - 1; i++) if (sites[i + 1]! - sites[i]! > sites[best + 1]! - sites[best]!) best = i;
    const c = { site: Math.round((sites[best]! + sites[best + 1]!) / 2), left };
    ctcfs.push(c);
    lane.push(c);
  }
  rebuildLattice();
  if (!playing) settle();
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

// ------------------------------------------------------------------ dragging and hover on the tracks

/** The marker under the pointer: a probe or a CTCF, by the row it's in. */
function markerAt(e: PointerEvent): { probe: number; ctcf: number } {
  const rect = trackCanvas.getBoundingClientRect();
  const x = xScale(rect.width);
  const mx = e.clientX - rect.left;
  const my = e.clientY - rect.top;
  const nearest = (sites: number[]) => {
    let best = -1;
    let bestPx = 12;
    sites.forEach((s, i) => {
      const px = Math.abs(x(s) - mx);
      if (px < bestPx) [best, bestPx] = [i, px];
    });
    return best;
  };
  if (my < PROBE_ROW.bottom) return { probe: nearest(probes), ctcf: -1 };
  const left = my < CTCF_ROWS.left.bottom;
  // Only this lane's CTCFs, mapped back to their index in `ctcfs`.
  const lane = ctcfs.flatMap((c, i) => (c.left === left ? [i] : []));
  const k = nearest(lane.map((i) => ctcfs[i]!.site));
  return { probe: -1, ctcf: k < 0 ? -1 : lane[k]! };
}

function siteUnder(e: PointerEvent): number {
  return Math.round(Math.max(0, Math.min(N_SITES - 1, siteAt(trackCanvas, e.clientX))));
}

trackCanvas.addEventListener("pointerdown", (e) => {
  const m = markerAt(e);
  dragging = m.probe;
  draggingCtcf = m.ctcf;
  if (dragging < 0 && draggingCtcf < 0) return;
  trackCanvas.setPointerCapture(e.pointerId);
  trackCanvas.style.cursor = "grabbing";
  e.preventDefault();
});
trackCanvas.addEventListener("pointermove", (e) => {
  if (dragging >= 0) {
    const site = siteUnder(e);
    if (site === probes[dragging] || probes.some((s, i) => s === site && i !== dragging)) return; // one probe per site
    probes[dragging] = site;
    hovered = dragging;
    if (!playing) settle();
    return;
  }
  if (draggingCtcf >= 0) {
    const site = siteUnder(e);
    if (ctcfs.some((c) => c.site === site)) return; // one CTCF per site
    ctcfs[draggingCtcf]!.site = site;
    rebuildLattice();
    if (!playing) settle();
    return;
  }
  const m = markerAt(e);
  trackCanvas.style.cursor = m.probe >= 0 || m.ctcf >= 0 ? "grab" : "";
  if (m.probe !== hovered || m.ctcf !== hoveredCtcf) {
    hovered = m.probe;
    hoveredCtcf = m.ctcf;
    if (!playing) draw();
  }
});
const endDrag = () => {
  dragging = -1;
  draggingCtcf = -1;
  trackCanvas.style.cursor = "";
  if (!playing) settle();
};
trackCanvas.addEventListener("pointerup", endDrag);
trackCanvas.addEventListener("pointercancel", endDrag);
trackCanvas.addEventListener("pointerleave", () => {
  if (dragging < 0 && draggingCtcf < 0 && (hovered >= 0 || hoveredCtcf >= 0)) {
    hovered = -1;
    hoveredCtcf = -1;
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
  trackCtx = fit(trackCanvas);
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
fitViewIn.addEventListener("change", () => {
  if (!fitView()) showScale(fixedScale());
  if (!playing) settle(20);
});
scaleIn.addEventListener("input", () => setFixedScale(Number(scaleIn.value)));
// Scrolling over the layout zooms it, at a fixed scale.
graphCanvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const current = fitView() ? Math.log10(view?.scale ?? fixedScale()) : Number(scaleIn.value);
    setFixedScale(current - e.deltaY * 0.002);
  },
  { passive: false },
);
nLeftIn.addEventListener("change", () => setCtcfCount(true, nLeftIn));
nRightIn.addEventListener("change", () => setCtcfCount(false, nRightIn));
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
playBtn.disabled = false;
restartBtn.disabled = false;
restart();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
else settle(120);
