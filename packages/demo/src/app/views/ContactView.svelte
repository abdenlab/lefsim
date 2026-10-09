<!--
  Contact map: every pair of lattice bins colored by its approximate contact
  probability, from the effective distance through the loops (see
  `contacts.ts`). Square (the default), the current map is above the diagonal
  and the time average below; aligned with the stage, the map hangs under the
  loops as a triangle on the same x axis.
-->
<script lang="ts">
  import { onMount, untrack } from "svelte";

  import { drawColorbar, drawColorbarVertical, FALL, SquareMap } from "../../heatmap.ts";
  import { worldOf } from "../../model.ts";
  import { fmt, MARGIN, placeTooltip, xScale } from "../../render.ts";
  import { sizedCanvas } from "../canvas.ts";
  import type { Run } from "../run.svelte.ts";
  import { ContactMaps, drawTriangle, triangleCell } from "./contacts.ts";

  let { run }: { run: Run } = $props();

  /** Bins per side of the map; the bin size follows the lattice size. */
  const BIN_COUNTS = [100, 200, 300, 600];
  // The view is remounted when the lattice size changes.
  const N_SITES = untrack(() => run.nSites);
  const binSize = (count: number) => Math.max(1, Math.ceil(N_SITES / count));

  let layout = $state<"square" | "aligned">("square");
  let colorScale = $state<"log" | "linear">("log");

  function setScale(scale: "log" | "linear") {
    colorScale = scale;
    maps.setScale(scale);
    recolored();
  }

  // The color range's ends, as shown in the fields, and whether each was set by hand.
  let rangeLo = $state(0);
  let rangeHi = $state(1);
  let manual = $state(false);

  function recolored() {
    [rangeLo, rangeHi] = maps.rangeP;
    manual = maps.clipMin !== undefined || maps.clipMax !== undefined;
    draw();
    drawBar();
  }

  /** A contact probability, short: 0.03, 0.0012, 3.7e-5. */
  const fmtP = (v: number) => (v === 0 ? "0" : v < 1e-3 ? v.toExponential(1) : String(Number(v.toPrecision(2))));

  /** Set one end of the color range from its field; an invalid value puts the field back. */
  function setEnd(which: "min" | "max", input: HTMLInputElement) {
    const v = Number(input.value);
    const ok =
      input.value.trim() !== "" &&
      Number.isFinite(v) &&
      maps.setClip(which === "min" ? v : maps.clipMin, which === "max" ? v : maps.clipMax);
    if (!ok) input.value = fmtP(which === "min" ? rangeLo : rangeHi);
    recolored();
  }

  function autoRange() {
    maps.setClip(undefined, undefined);
    recolored();
  }
  let show = $state<"now" | "average">("now");
  let binCount = $state(300);
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
        // The colorbar stands upright in a lane on the right, balancing the CTCF strip on the left.
        square.rightLane = 46;
        square.decorate = (c, box) => {
          const [lo, hi] = maps.range;
          drawColorbarVertical(c, {
            x0: box.left + box.size + 12,
            y0: box.top + 14,
            height: box.size - 14,
            lut: FALL,
            lo,
            hi,
            label: "P",
            linear: maps.scale === "linear",
          });
        };
      }
      square.pixels.set(maps.pixels);
      square.axis = { ctcfs: run.ctcfs, extent: maps.k * maps.bin };
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
    const [lo, hi] = maps.range;
    drawColorbar(barCtx, {
      x0: width - MARGIN.right - barW,
      y0: 8,
      width: barW,
      lut: FALL,
      lo,
      hi,
      label: "P",
      linear: maps.scale === "linear",
    });
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

  function setBins(count: number) {
    binCount = count;
    maps.setBin(binSize(count));
    [rangeLo, rangeHi] = maps.rangeP;
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
    maps.setBin(binSize(binCount));
    [rangeLo, rangeHi] = maps.rangeP;
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

<canvas class="bar" hidden={layout === "square"} bind:this={barCanvas}></canvas>
<div class="map" class:narrow={layout === "square"}>
  {#if layout === "square"}
    <canvas class="square" bind:this={mapCanvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  {:else}
    <canvas class="aligned" bind:this={mapCanvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  {/if}
  <div class="tooltip" bind:this={tooltip} hidden></div>
</div>
<p class="hint">
  Contact probability modeled as P ∝ d<sub>eff</sub><sup>−1.5</sup> (phantom, bead-spring network at equilibrium).<br>{layout === "square" ? "Above: instantaneous. Below: average " : ""}
  accumulated over <span class="mono">{fmt.format(averaged)}</span> time units
  ({fmt.format(nBins)} × {fmt.format(nBins)} bins, <span class="mono">{computeMs.toFixed(1)}</span> ms a frame).
</p>
<div class="head">
  <span class="seg" role="group" aria-label="Layout">
    <button aria-pressed={layout === "square"} onclick={() => (layout = "square")}>Square</button>
    <button aria-pressed={layout === "aligned"} onclick={() => (layout = "aligned")}>Aligned with stage</button>
  </span>
  {#if layout === "aligned"}
    <span class="seg" role="group" aria-label="Show">
      <button aria-pressed={show === "now"} onclick={() => (show = "now")}>Now</button>
      <button aria-pressed={show === "average"} onclick={() => (show = "average")}>Average</button>
    </span>
  {/if}
  <span class="seg" role="group" aria-label="Color scale">
    <button aria-pressed={colorScale === "log"} onclick={() => setScale("log")}>Log</button>
    <button aria-pressed={colorScale === "linear"} onclick={() => setScale("linear")}>Linear</button>
  </span>
  <label class="bin">
    Bins
    <select value={binCount} onchange={(e) => setBins(Number(e.currentTarget.value))}>
      {#each BIN_COUNTS as c (c)}
        <option value={c}>{c} ({fmt.format(binSize(c))} sites each)</option>
      {/each}
    </select>
  </label>
  <button class="btn reset" onclick={reset}>Reset average</button>
</div>
<div class="range" role="group" aria-label="Color range">
  <span>Color range</span>
  <input
    class="mono"
    type="text"
    inputmode="decimal"
    aria-label="Lowest contact probability of the color scale"
    value={fmtP(rangeLo)}
    onchange={(e) => setEnd("min", e.currentTarget)}
  />
  <span>to</span>
  <input
    class="mono"
    type="text"
    inputmode="decimal"
    aria-label="Highest contact probability of the color scale; higher values saturate"
    value={fmtP(rangeHi)}
    onchange={(e) => setEnd("max", e.currentTarget)}
  />
  <button class="btn small" onclick={autoRange} disabled={!manual} title="Go back to the automatic range">Auto</button>
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .reset {
    margin-left: auto;
  }

  .range {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 14px;
    color: var(--ink-2);
  }

  .range input {
    width: 80px;
    text-align: right;
    border: 1px solid var(--rule);
    background: var(--paper);
    border-radius: 6px;
    padding: 4px 8px;
  }

  .small {
    font-size: 13px;
    padding: 4px 10px;
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

  /* Half as tall as wide: the triangle at its true 45°, so its cells stay square. */
  canvas.aligned {
    display: block;
    width: 100%;
    aspect-ratio: 2;
  }

  /* A square map and its colorbar: centered, at most 560px wide. Centering
     the container (not the canvas) keeps tooltips placed against it. */
  .narrow {
    width: min(100%, 600px);
    margin-inline: auto;
  }

  .map.narrow {
    container-type: inline-size;
  }

  /* Square map plus its colorbar lane: as tall as the map (the width less the
     side margins, CTCF strip and colorbar lane, plus the top strip). */
  canvas.square {
    display: block;
    width: 100%;
    height: calc(100cqw - 44px);
  }
</style>
