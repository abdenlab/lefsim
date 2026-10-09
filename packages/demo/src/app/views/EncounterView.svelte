<!--
  Encounters of two probe sites, dragged on the stage: while their effective
  distance is at most the threshold, they're in an encounter. The kymograph and
  the distance trace beside it get one row per frame, aligned, newest at the
  top; finished encounters build a histogram of durations (see `encounters.ts`).
-->
<script lang="ts">
  import { effectiveDistance } from "lefsim";
  import { onMount, untrack } from "svelte";

  import { worldOf } from "../../model.ts";
  import { colors, fit, fmt, Kymograph } from "../../render.ts";
  import { niceStep, sizedCanvas } from "../canvas.ts";
  import type { Run } from "../run.svelte.ts";
  import { N_BINS, PairEncounters } from "./encounters.ts";

  let { run }: { run: Run } = $props();

  const LENGTHS = { stepLength: 1, hopLength: 1 };
  const SAMPLE_DT = 1;

  // The view is remounted when the lattice size changes.
  const N_SITES = untrack(() => run.nSites);
  /** The threshold slider: up to a third of the lattice. */
  const THRESHOLD_MAX = Math.max(10, Math.round(N_SITES / 3));
  const THRESHOLD_STEP = Math.max(1, Math.round(THRESHOLD_MAX / 100));

  let probes = $state<[number, number]>([Math.round(0.3 * (N_SITES - 1)), Math.round(0.7 * (N_SITES - 1))]);
  let threshold = $state(Math.max(1, Math.round(N_SITES / 30)));
  const stats = new PairEncounters(SAMPLE_DT, untrack(() => threshold));

  // Readouts, refreshed each frame.
  let distance = $state(0);
  let inEncounter = $state(false);
  let nEncounters = $state(0);
  let meanDuration = $state(0);
  let fraction = $state(0);

  let kymoCanvas: HTMLCanvasElement;
  let traceCanvas: HTMLCanvasElement;
  let histCanvas: HTMLCanvasElement;
  let kymograph: Kymograph | undefined;
  let traceCtx: CanvasRenderingContext2D | undefined;
  let histCtx: CanvasRenderingContext2D | undefined;

  /** Per kymograph row: whether any sample was in an encounter, and the range of distances. */
  let rowEncounter = false;
  let rowMin = Infinity;
  let rowMax = -Infinity;
  let lastTraceX: number | undefined;
  let traceRows = 0;

  const measure = () => effectiveDistance(worldOf(run.sim!), probes[0], probes[1], LENGTHS);

  function resetStats() {
    if (!run.sim) return;
    stats.threshold = threshold;
    stats.reset(run.sim.time, measure());
    readout();
  }

  function sample(t: number) {
    stats.sample(t, measure());
    rowEncounter ||= stats.inEncounter;
    rowMin = Math.min(rowMin, stats.distance);
    rowMax = Math.max(rowMax, stats.distance);
  }

  function readout() {
    distance = stats.distance;
    inEncounter = stats.inEncounter;
    nEncounters = stats.nEncounters;
    meanDuration = stats.nEncounters ? stats.totalDuration / stats.nEncounters : 0;
    fraction = stats.observedTime ? stats.encounterTime / stats.observedTime : 0;
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

  /** The trace's scale: up to the probes' separation, which no distance exceeds. */
  function traceX(d: number, width: number): number {
    const max = Math.max(1, Math.abs(probes[1] - probes[0]));
    return 8 + (Math.min(d, max) / max) * (width - 16);
  }

  function pushTrace() {
    const ctx = traceCtx;
    if (!ctx) return;
    const { width } = traceCanvas.getBoundingClientRect();
    const dpr = devicePixelRatio || 1;
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.drawImage(traceCanvas, 0, 0, traceCanvas.width, traceCanvas.height - dpr, 0, dpr, traceCanvas.width, traceCanvas.height - dpr);
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
    ctx.fillStyle = colors.muted;
    if (traceRows++ % 4 < 2) ctx.fillRect(traceX(threshold, width), 0, 1, 1);
    // The row's range of distances, joined to the last row's.
    const lo = rowMin === Infinity ? stats.distance : rowMin;
    const hi = rowMax === -Infinity ? stats.distance : rowMax;
    let x0 = traceX(lo, width);
    let x1 = traceX(hi, width);
    if (lastTraceX !== undefined) {
      x0 = Math.min(x0, lastTraceX);
      x1 = Math.max(x1, lastTraceX);
    }
    ctx.fillStyle = rowEncounter ? colors.probe : colors.ink2;
    ctx.fillRect(x0 - 1, 0, Math.max(2, x1 - x0 + 2), 1);
    lastTraceX = traceX(stats.distance, width);
  }

  function resetTrace() {
    if (!traceCtx) return;
    const { width, height } = traceCanvas.getBoundingClientRect();
    traceCtx.fillStyle = colors.paper;
    traceCtx.fillRect(0, 0, width, height);
    lastTraceX = undefined;
  }

  function drawHistogram() {
    const ctx = histCtx;
    if (!ctx) return;
    const { width, height } = histCanvas.getBoundingClientRect();
    ctx.clearRect(0, 0, width, height);
    const left = 36;
    const right = 10;
    const top = 10;
    const bottom = height - 34;
    const max = Math.max(1, ...stats.counts);
    const barW = (width - left - right) / N_BINS;
    ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
    const step = niceStep(Math.max(1, max / 4)); // whole counts
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    for (let v = 0; v <= max; v += step) {
      const y = bottom - (v / max) * (bottom - top);
      ctx.fillStyle = colors.rule;
      ctx.fillRect(left, Math.round(y), width - left - right, 1);
      ctx.fillStyle = colors.muted;
      ctx.fillText(fmt.format(v), left - 6, y);
    }
    ctx.fillStyle = colors.probe;
    for (let b = 0; b < N_BINS; b++) {
      const h = (stats.counts[b]! / max) * (bottom - top);
      ctx.fillRect(left + b * barW + 1, bottom - h, barW - 2, h);
    }
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.fillStyle = colors.muted;
    const compact = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 0 });
    for (let b = 0; b <= N_BINS; b += 2) {
      ctx.fillRect(left + b * barW, bottom, 1, 4);
      ctx.fillText(compact.format(SAMPLE_DT * 2 ** b), left + b * barW, bottom + 6);
    }
    ctx.fillText("duration (time units, log scale)", left + (width - left - right) / 2, bottom + 20);
  }

  /** After a probe or threshold change: fresh statistics and a fresh trace. */
  function definitionChanged() {
    resetStats();
    resetTrace();
    drawHistogram();
  }

  onMount(() => {
    kymograph = new Kymograph(kymoCanvas);
    traceCtx = fit(traceCanvas);
    const ro = new ResizeObserver(() => {
      kymograph?.resize();
      traceCtx = fit(traceCanvas);
      resetTrace();
    });
    ro.observe(kymoCanvas);
    const stopHist = sizedCanvas(histCanvas, (c) => {
      histCtx = c;
      drawHistogram();
    });
    run.markers = {
      sites: probes,
      move: (i, site) => {
        if (site === probes[1 - i]) return;
        probes[i] = site;
        run.markers = { ...run.markers!, sites: [...probes] };
        definitionChanged();
      },
    };
    resetStats();
    run.setDriver({
      sampleDt: SAMPLE_DT,
      sample: (t) => sample(t),
      frame: (f) => {
        if (rowMin === Infinity) rowEncounter = stats.inEncounter; // no sample this frame: keep the current state
        kymograph?.push(f, underRow);
        pushTrace();
        rowEncounter = false;
        rowMin = Infinity;
        rowMax = -Infinity;
        drawHistogram();
        readout();
      },
      changed: (kind) => {
        if (kind === "restart") {
          kymograph?.reset();
          resetTrace();
          resetStats();
        }
      },
    });
    return () => {
      ro.disconnect();
      stopHist();
      run.setDriver(undefined);
      run.markers = undefined;
    };
  });
</script>

<div class="head">
  <label class="threshold">
    Threshold
    <input
      type="range"
      min="0"
      max={THRESHOLD_MAX}
      step={THRESHOLD_STEP}
      value={threshold}
      oninput={(e) => {
        threshold = Number(e.currentTarget.value);
        definitionChanged();
      }}
      aria-label="Threshold: the effective distance at or below which the probes are in an encounter"
    />
    <output class="mono">{fmt.format(threshold)}</output>
  </label>
  <button class="btn reset" onclick={definitionChanged}>Reset statistics</button>
</div>
<div class="rows">
  <figure>
    <figcaption>Kymograph</figcaption>
    <canvas class="kymo" bind:this={kymoCanvas}></canvas>
  </figure>
  <figure>
    <figcaption>Effective distance, 0–{fmt.format(Math.abs(probes[1] - probes[0]))}</figcaption>
    <canvas class="trace" bind:this={traceCanvas}></canvas>
  </figure>
</div>
<figure>
  <figcaption>Encounter durations</figcaption>
  <canvas class="hist" bind:this={histCanvas}></canvas>
</figure>
<p class="hint">Drag the two probes on the stage. Distances are sampled every time unit, so durations don't depend on the simulation speed.</p>
<dl class="readout">
  <div><dt>distance now</dt><dd class="mono" class:meeting={inEncounter} title={inEncounter ? "In an encounter" : undefined}>{fmt.format(distance)}</dd></div>
  <div><dt>encounters</dt><dd class="mono">{fmt.format(nEncounters)}</dd></div>
  <div><dt>mean duration</dt><dd class="mono">{nEncounters ? `${fmt.format(meanDuration)} time units` : "–"}</dd></div>
  <div><dt>time in encounters</dt><dd class="mono">{Math.round(100 * fraction)}%</dd></div>
</dl>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  .reset {
    margin-left: auto;
  }

  .threshold {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 14px;
    color: var(--ink-2);
  }

  .threshold input {
    width: min(260px, 50vw);
  }

  .readout {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 28px;
    margin: 0;
  }

  .readout div {
    display: flex;
    flex-direction: column;
  }

  .readout dt {
    font-size: 12px;
    color: var(--muted);
  }

  .readout dd {
    margin: 0;
    font-size: 15px;
  }

  /* The distance while the probes are in an encounter. */
  .readout dd.meeting {
    color: var(--probe);
    font-weight: 600;
  }

  p {
    margin: 0;
  }

  figure {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  figcaption {
    font-size: 13px;
    color: var(--ink-2);
  }

  .rows {
    display: grid;
    grid-template-columns: 3fr 1fr;
    gap: 16px;
  }

  canvas {
    display: block;
    width: 100%;
  }

  .kymo,
  .trace {
    height: 260px;
  }

  .hist {
    height: 200px;
  }

  @media (max-width: 760px) {
    .rows {
      grid-template-columns: 2fr 1fr;
      gap: 8px;
    }
  }
</style>
