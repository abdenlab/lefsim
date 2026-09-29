// File replay demo: a keyframed event log written by the Python binding's
// `Stream.to_parquet`, replayed with random access. The loop and kymograph
// panels are the live page's own views, fed from the file's replayer.
//
// The replayer reads the file's footer, then only the row groups each move
// needs (over HTTP, with range requests), so reading is asynchronous: the
// playback loop runs one move at a time and seeks are coalesced while the
// scrubber is dragged.

import { init } from "lefsim";
import { replayerFromParquet, type ParquetReplayer, type ParquetSource } from "lefsim/parquet";

import { $, fmt, fmtCompact, Kymograph, LoopView, onThemeChange, setLattice, snapshotOf, type Frame } from "./render.ts";

/** Where `make_sample.py` writes its file; the page offers it if it's there. */
const SAMPLE = "/sample.parquet";

// ------------------------------------------------------------------ DOM

const playBtn = $<HTMLButtonElement>("play");
const speedIn = $<HTMLInputElement>("speed");
const speedOut = $<HTMLOutputElement>("speed-out");
const fileIn = $<HTMLInputElement>("file");
const urlIn = $<HTMLInputElement>("url");
const sampleBtn = $<HTMLButtonElement>("load-sample");
const summary = $<HTMLParagraphElement>("summary");
const seekIn = $<HTMLInputElement>("seek");
const seekOut = $<HTMLOutputElement>("seek-out");
const buffered = $("buffered");
const rSource = $("r-source");
const rSize = $("r-size");
const rGroups = $("r-groups");
const rSeed = $("r-seed");
const rVersion = $("r-version");
const rSeek = $("r-seek");
const errorRow = $("error");
const rError = $("r-error");

const loops = new LoopView($<HTMLCanvasElement>("arcs"), $("tooltip"));
const kymograph = new Kymograph($<HTMLCanvasElement>("kymo"));

// ------------------------------------------------------------------ state

let replayer: ParquetReplayer | undefined;
let loadGeneration = 0;
let playing = false;
let lastFrame: number | undefined;
let dragging = false;
/** A move (advance or seek) is reading; the next one waits for it. */
let busy = false;
/** The latest seek asked for while busy; runs when the current move ends. */
let pendingSeek: number | undefined;

/** Replay time units per wall-clock second, from a log-scale slider. */
function speed(): number {
  return Math.round(10 ** Number(speedIn.value));
}

function showSpeed() {
  speedOut.value = `${fmt.format(speed())} / s`;
}

const fmtBytes = (n: number) =>
  n < 1e6 ? `${fmt.format(n / 1e3)} KB` : `${new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 }).format(n / 1e6)} MB`;

function showError(e: unknown) {
  errorRow.hidden = false;
  rError.textContent = e instanceof Error ? e.message : String(e);
}

// ------------------------------------------------------------------ loading

/** Open `source` (named `name`, `size` bytes if known) and show its first state. */
async function open(source: ParquetSource, name: string, size?: number) {
  const generation = ++loadGeneration;
  setPlaying(false);
  errorRow.hidden = true;
  playBtn.disabled = true;
  rSource.textContent = name;
  rSize.textContent = size === undefined ? "–" : fmtBytes(size);
  summary.textContent = `Opening ${name}…`;
  let r: ParquetReplayer;
  try {
    r = await replayerFromParquet(source);
  } catch (e) {
    if (generation === loadGeneration) {
      summary.textContent = `Couldn't open ${name}.`;
      showError(e);
    }
    return;
  }
  if (generation !== loadGeneration) return; // another file was opened meanwhile
  replayer = r;
  setLattice(r.lattice);
  const start = r.start ?? 0;
  const run = r.run;
  const nCtcfs = r.lattice.ctcfs.site.length;
  summary.textContent =
    `${fmt.format(r.nLefs)} loop extruders on a ${fmt.format(r.lattice.nSites)}-site lattice with ` +
    `${fmt.format(nCtcfs)} CTCFs, recorded from t = ${fmt.format(start)} to ${fmt.format(r.end)}. ` +
    `Only the row groups each move needs are read.`;
  rSeed.textContent = run?.seed ?? "–";
  rVersion.textContent = run?.lefsimVersion ?? "–";
  rSeek.textContent = "–";
  seekIn.min = String(start);
  seekIn.max = String(r.end);
  buffered.style.width = "100%"; // every time in the file is reachable
  kymograph.reset();
  await move(() => r.seek(start));
  playBtn.disabled = false;
  if (!matchMedia("(prefers-reduced-motion: reduce)").matches) setPlaying(true);
}

async function openSample() {
  const head = await fetch(SAMPLE, { method: "HEAD" }).catch(() => undefined);
  const size = Number(head?.headers.get("content-length")) || undefined;
  await open(new URL(SAMPLE, location.href).href, "sample.parquet", size);
}

// ------------------------------------------------------------------ moving

/** Run one move at a time, then any seek that was asked for meanwhile. */
async function move(step: () => Promise<number>): Promise<void> {
  busy = true;
  try {
    await step();
    render(true);
    while (pendingSeek !== undefined) {
      const t = pendingSeek;
      pendingSeek = undefined;
      await seekNow(t);
    }
  } catch (e) {
    showError(e);
  } finally {
    busy = false;
  }
}

async function seekNow(t: number) {
  const r = replayer!;
  const started = performance.now();
  const reads = r.rowGroupsRead;
  await r.seek(Math.max(t, r.start ?? t));
  const ms = performance.now() - started;
  const read = r.rowGroupsRead - reads;
  rSeek.textContent = `${ms < 0.1 ? "under 0.1" : ms.toFixed(1)} ms, ${read} row group${read === 1 ? "" : "s"} read`;
  lastFrame = undefined;
  kymograph.reset();
  render(true);
}

function seekTo(t: number) {
  if (!replayer?.ready) return;
  if (busy) {
    pendingSeek = t; // the latest wins
    return;
  }
  void move(() => seekNow(t).then(() => replayer!.time));
}

seekIn.addEventListener("pointerdown", () => (dragging = true));
seekIn.addEventListener("pointerup", () => (dragging = false));
seekIn.addEventListener("input", () => seekTo(Number(seekIn.value)));

// ------------------------------------------------------------------ playback

function setPlaying(on: boolean) {
  const r = replayer;
  if (on && r && r.time >= r.end) seekTo(r.start ?? 0); // replay from the start
  playing = on && r !== undefined;
  playBtn.textContent = playing ? "Pause" : "Play";
  lastFrame = undefined;
  if (playing) requestAnimationFrame(frame);
}

function frame(now: number) {
  if (!playing) return;
  const dt = lastFrame === undefined ? 0 : Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;
  const r = replayer;
  // One move at a time: while a row group is being read, frames just wait.
  if (r?.ready && !busy) {
    const target = Math.min(r.time + speed() * dt, r.end);
    void move(() => r.advanceTo(target)).then(() => {
      if (r.time >= r.end) setPlaying(false);
    });
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
  seekOut.value = `t = ${fmt.format(f.time)} of ${fmt.format(r.end)}`;
  rGroups.textContent = `${fmtCompact.format(r.rowGroupsRead)} of ${fmtCompact.format(r.rowGroupCount)}`;
}

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
speedIn.addEventListener("input", showSpeed);
fileIn.addEventListener("change", () => {
  const file = fileIn.files?.[0];
  if (file) void open(file, file.name, file.size);
  fileIn.value = ""; // so choosing the same file again reloads it
});
$<HTMLFormElement>("controls").addEventListener("submit", (e) => {
  e.preventDefault();
  const url = urlIn.value.trim();
  if (url) void open(url, url.split("/").at(-1) || url);
});
sampleBtn.addEventListener("click", () => void openSample());

// Drop a file anywhere on the page.
document.addEventListener("dragover", (e) => {
  e.preventDefault();
  document.body.classList.add("dragging");
});
document.addEventListener("dragleave", (e) => {
  if (e.relatedTarget === null) document.body.classList.remove("dragging");
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  document.body.classList.remove("dragging");
  const file = e.dataTransfer?.files[0];
  if (file) void open(file, file.name, file.size);
});

await init();
showSpeed();

// `?url=...` opens a file straight away; otherwise offer the sample if it exists.
const urlParam = new URLSearchParams(location.search).get("url");
const sampleExists = await fetch(SAMPLE, { method: "HEAD" }).then((r) => r.ok, () => false);
sampleBtn.hidden = !sampleExists;
if (urlParam) {
  urlIn.value = urlParam;
  void open(new URL(urlParam, location.href).href, urlParam.split("/").at(-1) || urlParam);
} else if (sampleExists) {
  void openSample();
}
