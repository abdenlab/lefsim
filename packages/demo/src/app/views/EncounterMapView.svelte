<!--
  Encounter map: for every pair of lattice bins, how often their effective
  distance comes within the threshold, and for how long. Below the diagonal is
  the frequency (encounters per time unit), above it the mean duration. The
  distances between all pairs are sampled every time unit (see `encounters.ts`).
-->
<script lang="ts">
  import { effectiveDistances } from "lefsim";
  import { onMount } from "svelte";

  import { BLUES, drawColorbar, FALL, packedColors, packPixel, SquareMap } from "../../heatmap.ts";
  import { worldOf } from "../../model.ts";
  import { fmt, MARGIN, placeTooltip } from "../../render.ts";
  import { sizedCanvas } from "../canvas.ts";
  import { N_SITES, type Run } from "../run.svelte.ts";
  import { MapEncounters } from "./encounters.ts";

  let { run }: { run: Run } = $props();

  const LENGTHS = { stepLength: 1, hopLength: 1 };
  const SAMPLE_DT = 1;
  const BINS = [10, 20, 50];
  /** Pairs always within the threshold along the lattice. */
  const TRIVIAL = packPixel(208, 208, 204);
  const freqColors = packedColors(FALL);
  const durColors = packedColors(BLUES);

  let bin = $state(20);
  let threshold = $state(100);
  let observed = $state(0);
  let sampleMs = $state(0);
  let nBins = $state(0);
  const enc = new MapEncounters(SAMPLE_DT, 100);
  let freqRange: [number, number] = [0, 1];
  let durRange: [number, number] = [0, 1];

  let mapCanvas: HTMLCanvasElement;
  let barCanvas: HTMLCanvasElement;
  let tooltip: HTMLDivElement;
  let map: SquareMap | undefined;
  let barCtx: CanvasRenderingContext2D | undefined;

  const fmtNum = (v: number) => (v >= 100 ? fmt.format(v) : v >= 1 ? v.toFixed(1) : v.toPrecision(2));

  function colorIndex(log: number, lo: number, hi: number): number {
    const c = Math.round(((log - lo) / (hi - lo)) * 255);
    return c < 0 ? 0 : c > 255 ? 255 : c;
  }

  function setBins(b: number) {
    bin = b;
    enc.threshold = threshold;
    enc.setBins(N_SITES, b);
    nBins = enc.k;
    map?.setSize(enc.k);
    draw();
  }

  function reset() {
    enc.threshold = threshold;
    enc.reset();
    draw();
  }

  function sample() {
    const started = performance.now();
    enc.sample(effectiveDistances(worldOf(run.sim!), enc.centers, LENGTHS));
    sampleMs = 0.9 * sampleMs + 0.1 * (performance.now() - started);
  }

  /** Frequency below the diagonal, mean duration above; scales from the current totals. */
  function paint() {
    if (!map) return;
    const k = enc.k;
    const px = map.pixels;
    const time = enc.observedTime;
    let maxStarts = 0;
    let maxMean = SAMPLE_DT;
    for (let at = 0; at < enc.starts.length; at++) {
      if (enc.trivial[at] || enc.starts[at] === 0) continue;
      maxStarts = Math.max(maxStarts, enc.starts[at]!);
      maxMean = Math.max(maxMean, (enc.samplesIn[at]! * SAMPLE_DT) / enc.starts[at]!);
    }
    const fLo = time > 0 ? Math.log10(1 / time) : -3;
    const fHi = time > 0 && maxStarts > 1 ? Math.log10(maxStarts / time) : fLo + 1;
    const dLo = Math.log10(SAMPLE_DT);
    const dHi = Math.max(dLo + 1, Math.log10(maxMean));
    freqRange = [fLo, fHi];
    durRange = [dLo, dHi];
    const empty = freqColors[0]!;
    let at = 0;
    for (let i = 0; i < k; i++) {
      px[i * k + i] = TRIVIAL;
      for (let j = i + 1; j < k; j++, at++) {
        if (enc.trivial[at]) {
          px[i * k + j] = TRIVIAL;
          px[j * k + i] = TRIVIAL;
          continue;
        }
        const n = enc.starts[at]!;
        if (n === 0 || time === 0) {
          px[i * k + j] = empty;
          px[j * k + i] = empty;
          continue;
        }
        px[j * k + i] = freqColors[colorIndex(Math.log10(n / time), fLo, fHi)]!;
        px[i * k + j] = durColors[colorIndex(Math.log10((enc.samplesIn[at]! * SAMPLE_DT) / n), dLo, dHi)]!;
      }
    }
  }

  function drawBars() {
    if (!barCtx) return;
    const { width, height } = barCanvas.getBoundingClientRect();
    barCtx.clearRect(0, 0, width, height);
    const barW = Math.min(260, width - MARGIN.left - MARGIN.right - 200);
    const x0 = width - MARGIN.right - barW;
    drawColorbar(barCtx, { x0, y0: 8, width: barW, lut: FALL, lo: freqRange[0], hi: freqRange[1], label: "Encounters per time unit (lower)" });
    drawColorbar(barCtx, { x0, y0: 44, width: barW, lut: BLUES, lo: durRange[0], hi: durRange[1], label: "Mean duration (upper)" });
  }

  function draw() {
    paint();
    if (map) map.axis = { ctcfs: run.ctcfs, extent: enc.k * bin };
    map?.draw();
    drawBars();
    observed = enc.observedTime;
  }

  function pointermove(e: PointerEvent) {
    const cell = map?.cellAt(e);
    if (!cell) {
      tooltip.hidden = true;
      return;
    }
    const [i, j] = [Math.min(...cell), Math.max(...cell)];
    const a = enc.centers[i]!;
    const b = enc.centers[j]!;
    if (i === j) {
      placeTooltip(tooltip, mapCanvas, e, `Bin at site ${fmt.format(a)}`);
      return;
    }
    const at = enc.index(i, j);
    const head = `Sites ${fmt.format(a)} and ${fmt.format(b)}, ${fmt.format(b - a)} apart: `;
    const time = enc.observedTime;
    let text: string;
    if (enc.trivial[at]) text = `${head}always within the threshold`;
    else if (enc.starts[at] === 0) text = `${head}no encounters in ${fmt.format(time)} time units`;
    else {
      const n = enc.starts[at]!;
      const tin = enc.samplesIn[at]! * SAMPLE_DT;
      text =
        `${head}${fmt.format(n)} encounter${n === 1 ? "" : "s"} in ${fmt.format(time)} time units ` +
        `(${fmtNum((1000 * n) / time)} per 1,000), mean duration ${fmtNum(tin / n)}, ${Math.round((100 * tin) / time)}% of the time`;
    }
    placeTooltip(tooltip, mapCanvas, e, text);
  }

  onMount(() => {
    map = new SquareMap(mapCanvas);
    const stopMap = sizedCanvas(mapCanvas, () => map?.resize());
    const stopBar = sizedCanvas(barCanvas, (c) => {
      barCtx = c;
      drawBars();
    });
    setBins(bin);
    run.setDriver({
      sampleDt: SAMPLE_DT,
      sample: () => sample(),
      frame: () => draw(),
      changed: (kind) => {
        if (kind === "restart") reset();
      },
    });
    return () => {
      stopMap();
      stopBar();
      run.setDriver(undefined);
    };
  });
</script>

<div class="head">
  <label class="field">
    Threshold
    <input
      type="range"
      min="0"
      max="1000"
      step="10"
      value={threshold}
      oninput={(e) => {
        threshold = Number(e.currentTarget.value);
        reset();
      }}
    />
    <output class="mono">{fmt.format(threshold)}</output>
  </label>
  <label class="field">
    Bin
    <select value={bin} onchange={(e) => setBins(Number(e.currentTarget.value))}>
      {#each BINS as b (b)}
        <option value={b}>{b} sites</option>
      {/each}
    </select>
  </label>
  <button class="btn" onclick={reset}>Reset statistics</button>
</div>
<p class="hint">
  Two bins meet while their effective distance is at most the threshold. Sampled every time unit over
  <span class="mono">{fmt.format(observed)}</span> time units while this view is open
  ({fmt.format(nBins)} × {fmt.format(nBins)} bins, <span class="mono">{sampleMs.toFixed(1)}</span> ms a sample). Gray
  pairs are always within the threshold along the lattice.
</p>
<canvas class="bar" bind:this={barCanvas}></canvas>
<div class="map">
  <canvas class="square" bind:this={mapCanvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  <div class="tooltip" bind:this={tooltip} hidden></div>
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    color: var(--ink-2);
  }

  .field input[type="range"] {
    width: min(220px, 45vw);
  }

  p {
    margin: 0;
  }

  .map {
    position: relative;
  }

  .bar {
    display: block;
    width: 100%;
    height: 70px;
  }

  canvas.square {
    display: block;
    width: min(100%, 560px);
    aspect-ratio: 1;
  }
</style>
