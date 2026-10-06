<!--
  Contact map: every pair of lattice bins colored by its approximate contact
  probability, from the effective distance through the loops (see
  `contacts.ts`). Aligned with the stage, the map hangs under the loops as a
  triangle on the same x axis; square, the current map is above the diagonal
  and the time average below.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import { drawColorbar, FALL, SquareMap } from "../../heatmap.ts";
  import { worldOf } from "../../model.ts";
  import { fmt, MARGIN, placeTooltip, xScale } from "../../render.ts";
  import { sizedCanvas } from "../canvas.ts";
  import { N_SITES, type Run } from "../run.svelte.ts";
  import { ContactMaps, drawTriangle, triangleCell } from "./contacts.ts";

  let { run }: { run: Run } = $props();

  const BINS = [5, 10, 20, 50];

  let layout = $state<"aligned" | "square">("aligned");
  let show = $state<"now" | "average">("now");
  let bin = $state(10);
  let averaged = $state(0);
  let computeMs = $state(0);
  let nBins = $state(0);

  const maps = new ContactMaps(N_SITES);
  let mapCanvas = $state<HTMLCanvasElement>();
  let barCanvas: HTMLCanvasElement;
  let tooltip: HTMLDivElement;
  let ctx: CanvasRenderingContext2D | undefined;
  let barCtx: CanvasRenderingContext2D | undefined;
  let square: SquareMap | undefined;
  /** The aligned triangle's vertical scale, for hit-testing. */
  let yScale = 1;

  function draw() {
    if (!mapCanvas || !ctx || maps.k === 0) return;
    if (layout === "square") {
      if (!square || square.k !== maps.k) {
        square ??= new SquareMap(mapCanvas);
        square.setSize(maps.k);
      }
      square.pixels.set(maps.pixels);
      square.draw();
      return;
    }
    // Aligned: device pixels, on the stage's x axis.
    const dpr = devicePixelRatio || 1;
    const { width } = mapCanvas.getBoundingClientRect();
    const x = xScale(width);
    const r = drawTriangle(ctx, {
      pixels: maps.pixels,
      k: maps.k,
      which: show === "now" ? "upper" : "lower",
      x0: x(0) * dpr,
      x1: x(N_SITES - 1) * dpr,
      width: mapCanvas.width,
      height: mapCanvas.height,
    });
    yScale = r.yScale;
  }

  function drawBar() {
    if (!barCtx) return;
    const { width, height } = barCanvas.getBoundingClientRect();
    barCtx.clearRect(0, 0, width, height);
    const barW = Math.min(260, width - MARGIN.left - MARGIN.right);
    const [lo, hi] = maps.logRange;
    drawColorbar(barCtx, { x0: width - MARGIN.right - barW, y0: 8, width: barW, lut: FALL, lo, hi, label: "P" });
  }

  function update() {
    const sim = run.sim;
    if (!sim) return;
    const started = performance.now();
    maps.update(worldOf(sim), sim.time);
    computeMs = 0.8 * computeMs + 0.2 * (performance.now() - started);
    averaged = maps.averagedTime;
    draw();
  }

  function setBin(b: number) {
    bin = b;
    maps.setBin(b);
    nBins = maps.k;
    maps.resetAverage(run.sim?.time);
    square = undefined;
    update();
    drawBar();
  }

  function reset() {
    maps.resetAverage(run.sim?.time);
    averaged = 0;
    update();
  }

  /** The bins under the pointer, as `[i, j]` with `i <= j`. */
  function cellAt(e: PointerEvent): [number, number] | undefined {
    if (!mapCanvas) return undefined;
    if (layout === "square") {
      const c = square?.cellAt(e);
      return c ? [Math.min(...c), Math.max(...c)] : undefined;
    }
    const rect = mapCanvas.getBoundingClientRect();
    const x = xScale(rect.width);
    const dpr = devicePixelRatio || 1;
    return triangleCell((e.clientX - rect.left) * dpr, (e.clientY - rect.top) * dpr, {
      k: maps.k,
      x0: x(0) * dpr,
      x1: x(N_SITES - 1) * dpr,
      yScale,
    });
  }

  function pointermove(e: PointerEvent) {
    const cell = cellAt(e);
    if (!cell || !mapCanvas) {
      tooltip.hidden = true;
      return;
    }
    const [i, j] = cell;
    const a = maps.centers[i]!;
    const b = maps.centers[j]!;
    if (i === j) {
      placeTooltip(tooltip, mapCanvas, e, `Bin at site ${fmt.format(a)}`);
      return;
    }
    const { d, p, mean } = maps.pair(i, j);
    const avg = mean === undefined ? "" : `; averaged, P = ${mean.toExponential(1)}`;
    placeTooltip(
      tooltip,
      mapCanvas,
      e,
      `Sites ${fmt.format(a)} and ${fmt.format(b)}, ${fmt.format(b - a)} apart. Now: effective distance ${fmt.format(d)}, P = ${p.toExponential(1)}${avg}`,
    );
  }

  // Each layout has its own canvas shape; set it up when it mounts.
  $effect(() => {
    const canvas = mapCanvas;
    void layout;
    if (!canvas) return;
    square = undefined;
    const stop = sizedCanvas(canvas, (c) => {
      ctx = c;
      if (layout === "square") square = undefined;
      draw();
    });
    return stop;
  });

  $effect(() => {
    void show;
    draw();
  });

  onMount(() => {
    const stopBar = sizedCanvas(barCanvas, (c) => {
      barCtx = c;
      drawBar();
    });
    maps.setBin(bin);
    nBins = maps.k;
    maps.resetAverage(run.sim?.time);
    run.setDriver({
      frame: () => update(),
      changed: (kind) => {
        if (kind === "restart") reset();
      },
    });
    return () => {
      stopBar();
      run.setDriver(undefined);
    };
  });
</script>

<div class="head">
  <span class="seg" role="group" aria-label="Layout">
    <button aria-pressed={layout === "aligned"} onclick={() => (layout = "aligned")}>Aligned with the loops</button>
    <button aria-pressed={layout === "square"} onclick={() => (layout = "square")}>Square</button>
  </span>
  {#if layout === "aligned"}
    <span class="seg" role="group" aria-label="Show">
      <button aria-pressed={show === "now"} onclick={() => (show = "now")}>Now</button>
      <button aria-pressed={show === "average"} onclick={() => (show = "average")}>Average</button>
    </span>
  {/if}
  <label class="bin">
    Bin
    <select value={bin} onchange={(e) => setBin(Number(e.currentTarget.value))}>
      {#each BINS as b (b)}
        <option value={b}>{b} sites</option>
      {/each}
    </select>
  </label>
  <button class="btn" onclick={reset}>Reset average</button>
</div>
<p class="hint">
  {layout === "square" ? "Above the diagonal: now. Below: the average. " : ""}P ∝ d<sub>eff</sub><sup>−1.5</sup>,
  averaged over <span class="mono">{fmt.format(averaged)}</span> time units while this view is open
  ({fmt.format(nBins)} × {fmt.format(nBins)} bins, <span class="mono">{computeMs.toFixed(1)}</span> ms a frame).
</p>
<canvas class="bar" bind:this={barCanvas}></canvas>
<div class="map">
  {#if layout === "aligned"}
    <canvas class="aligned" bind:this={mapCanvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  {:else}
    <canvas class="square" bind:this={mapCanvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  {/if}
  <div class="tooltip" bind:this={tooltip} hidden></div>
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .bin {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
    color: var(--ink-2);
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
    height: 34px;
  }

  canvas.aligned {
    display: block;
    width: 100%;
    height: min(46vw, 460px);
  }

  canvas.square {
    display: block;
    width: min(100%, 560px);
    aspect-ratio: 1;
  }
</style>
