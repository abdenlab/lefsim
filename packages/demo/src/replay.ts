// Replay demo: a run is recorded in the browser as keyframed event chunks and
// played back from a Replayer, with seeking. The loop and kymograph panels
// are the live page's own views; here they're fed from `replayer.state()`
// instead of `sim.state()`.
//
// Recording runs progressively, one chunk per animation frame, the way data
// would arrive from a network source. Playback reads only from the Replayer,
// so it can't pass what has been recorded: it waits, as it would while
// buffering a stream.

import { init, iterate, Replayer, Sim } from "lefsim";

import { N_SITES, newSim, readRunInputs } from "./model.ts";
import { $, fmt, fmtCompact, Kymograph, LoopView, onThemeChange, setLattice, snapshotOf, type Frame } from "./render.ts";

/** Length of each recorded run, in simulation time units. */
const T_END = 50_000;
/** Rows per recorded chunk; each chunk opens with a keyframe (2 rows per LEF). */
const CHUNK_ROWS = 1_000;

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const recordBtn = $<HTMLButtonElement>("record");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const nLefsIn = $<HTMLInputElement>("nlefs");
const seedIn = $<HTMLInputElement>("seed");
const summary = $<HTMLParagraphElement>("summary");
const seekIn = $<HTMLInputElement>("seek");
const seekOut = $<HTMLOutputElement>("seek-out");
const buffered = $("buffered");
const rRecorded = $("r-recorded");
const rChunks = $("r-chunks");
const rRows = $("r-rows");
const rSeek = $("r-seek");
const waiting = $("waiting");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
const kymograph = new Kymograph($<HTMLCanvasElement>("kymo"));

seekIn.max = String(T_END);

// ------------------------------------------------------------------ state

let replayer: Replayer | undefined;
let recordGeneration = 0;
let recording = false;
let chunks = 0;
let rows = 0;
let playhead = 0;
let playing = false;
let lastFrame: number | undefined;
let dragging = false;

const nextFrame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** Replay time units per wall-clock second, from a log-scale slider. */
function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

// ------------------------------------------------------------------ recording

async function recordRun() {
  const generation = ++recordGeneration;
  const { nLefs, seed } = readRunInputs(nLefsIn, seedIn);
  summary.textContent =
    `${nLefs} loop extruders on a ${fmt.format(N_SITES)}-site lattice, recorded in your browser as ` +
    `keyframed event chunks and replayed. The panels are drawn by the live page's code, from a ` +
    `Replayer instead of a Sim.`;
  const sim = newSim(nLefs, seed);
  setLattice(sim.lattice);
  const r = new Replayer(sim.lattice, sim.nLefs);
  replayer = r;
  recording = true;
  chunks = 0;
  rows = 0;
  playhead = 0;
  lastFrame = undefined;
  waiting.hidden = true;
  rSeek.textContent = "–";
  kymograph.reset();
  updateRecording();
  try {
    for await (const chunk of iterate(sim.stream(T_END, { events: true, maxRows: CHUNK_ROWS }))) {
      if (generation !== recordGeneration) break; // a new run started: breaking cancels the stream
      r.push(chunk);
      chunks++;
      rows += chunk.time.length;
      if (!r.ready) {
        r.seek(0); // the first chunk's keyframe defines the state at t = 0
        render(true);
      }
      updateRecording();
      await nextFrame();
    }
  } finally {
    sim.free(); // the recording now lives in the Replayer
    if (generation === recordGeneration) {
      recording = false;
      updateRecording();
    }
  }
}

function updateRecording() {
  const end = replayer && Number.isFinite(replayer.end) ? replayer.end : 0;
  buffered.style.width = `${(100 * end) / T_END}%`;
  rRecorded.textContent = `${fmt.format(end)} of ${fmt.format(T_END)}${recording ? "" : ", done"}`;
  rChunks.textContent = fmt.format(chunks);
  rRows.textContent = fmtCompact.format(rows);
}

// ------------------------------------------------------------------ playback

function setPlaying(on: boolean) {
  if (on && playhead >= T_END) playhead = 0; // replay from the start
  playing = on;
  playBtn.textContent = on ? "Pause" : "Play";
  lastFrame = undefined;
  if (on) requestAnimationFrame(frame);
}

function frame(now: number) {
  if (!playing) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  const r = replayer;
  if (r?.ready) {
    const target = Math.min(playhead + speed() * dt, T_END);
    // Clamped to what has been recorded: short of the target means waiting.
    const reached = r.advanceTo(target);
    waiting.hidden = reached >= target;
    const moved = reached !== playhead;
    playhead = reached;
    render(moved);
    if (playhead >= T_END) {
      setPlaying(false);
      return;
    }
  }
  requestAnimationFrame(frame);
}

/** Draw the replayer's current state; `advance` adds a kymograph row. */
function render(advance: boolean) {
  const r = replayer;
  if (!r?.ready) return;
  const f: Frame = snapshotOf(r.state());
  loops.show(f);
  if (advance) kymograph.push(f);
  if (!dragging) seekIn.value = String(f.time);
  seekOut.value = `t = ${fmt.format(f.time)} of ${fmt.format(T_END)}`;
}

// ------------------------------------------------------------------ seeking

function seekTo(t: number) {
  const r = replayer;
  if (!r?.ready) return;
  // Only what's been recorded can be shown.
  const target = Math.min(t, r.end);
  const start = performance.now();
  r.seek(target);
  const ms = performance.now() - start;
  // Browsers coarsen performance.now() to about 0.1 ms, below which a seek
  // (restore a keyframe, replay at most one chunk) usually finishes.
  rSeek.textContent = ms < 0.1 ? "under 0.1 ms" : `${ms.toFixed(1)} ms`;
  playhead = target;
  lastFrame = undefined;
  kymograph.reset();
  render(true);
}

seekIn.addEventListener("pointerdown", () => (dragging = true));
seekIn.addEventListener("pointerup", () => (dragging = false));
seekIn.addEventListener("input", () => seekTo(Number(seekIn.value)));

// ------------------------------------------------------------------ wiring

new ResizeObserver(() => {
  loops.resize();
  kymograph.resize();
}).observe($("arcs"));
onThemeChange(() => {
  loops.draw();
  kymograph.reset();
});

playBtn.addEventListener("click", () => setPlaying(!playing));
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  void recordRun();
});
speedIn.addEventListener("input", showSpeed);

await init();
showSpeed();
playBtn.disabled = false;
recordBtn.disabled = false;
void recordRun();
// Respect reduced motion: start paused, let the viewer press Play.
if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
