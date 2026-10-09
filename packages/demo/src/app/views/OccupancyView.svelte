<!--
  Leg occupancy: the mean number of legs on each site, averaged over
  simulation time while this view is open. Each frame's state is weighted by
  the time since the previous frame, so pausing adds nothing.
-->
<script lang="ts">
  import { onMount, untrack } from "svelte";

  import { colors, drawCtcfs, drawTadBands, fmt, placeTooltip, xScale } from "../../render.ts";
  import { niceStep, sizedCanvas } from "../canvas.ts";
  import type { Run } from "../run.svelte.ts";

  let { run }: { run: Run } = $props();

  const LOG_MIN = 1e-4;

  let canvas: HTMLCanvasElement;
  let tooltip: HTMLDivElement;
  let ctx: CanvasRenderingContext2D | undefined;
  // The view is remounted when the lattice size changes.
  const N_SITES = untrack(() => run.nSites);
  let counts = new Float64Array(N_SITES);
  let lastTime: number | undefined;
  let averaged = $state(0);
  /** The y axis: linear (the peaks in proportion; the default) or log (the floor between them legible too). */
  let scale = $state<"linear" | "log">("linear");

  function reset() {
    counts = new Float64Array(N_SITES);
    averaged = 0;
    lastTime = run.sim?.time;
    draw();
  }

  function accumulate(time: number, left: Uint32Array, right: Uint32Array) {
    const dt = lastTime === undefined ? 0 : time - lastTime;
    lastTime = time;
    if (!(dt > 0)) return;
    for (let i = 0; i < left.length; i++) {
      counts[left[i]!]! += dt;
      counts[right[i]!]! += dt;
    }
    averaged += dt;
  }

  function draw() {
    if (!ctx) return;
    const { width, height } = canvas.getBoundingClientRect();
    const x = xScale(width);
    const top = 16;
    const base = height - 18;
    ctx.clearRect(0, 0, width, height);
    drawTadBands(ctx, x, top, base);

    // One value per pixel column: the mean over the sites it covers.
    const x0 = x(0);
    const x1 = x(N_SITES - 1);
    const columns: { px: number; v: number }[] = [];
    if (averaged > 0) {
      for (let px = Math.floor(x0); px <= Math.ceil(x1); px++) {
        const lo = Math.max(0, Math.floor(((px - x0) / (x1 - x0)) * (N_SITES - 1)));
        const hi = Math.min(N_SITES - 1, Math.floor(((px + 1 - x0) / (x1 - x0)) * (N_SITES - 1)));
        let sum = 0;
        for (let s = lo; s <= hi; s++) sum += counts[s]!;
        columns.push({ px, v: sum / (hi - lo + 1) / averaged });
      }
    }

    // The scale and its gridlines: decades for log; round steps up to the highest column for linear.
    let y: (v: number) => number;
    let ticks: { v: number; label: string }[];
    if (scale === "log") {
      y = (v) => base - ((Math.log10(Math.max(v, LOG_MIN)) - Math.log10(LOG_MIN)) / -Math.log10(LOG_MIN)) * (base - top);
      ticks = [-4, -3, -2, -1, 0].map((e) => ({ v: 10 ** e, label: e === 0 ? "1" : `10⁻${"⁰¹²³⁴"[-e]}` }));
    } else {
      const peak = Math.max(1e-3, ...columns.map((c) => c.v));
      const step = niceStep(peak / 4);
      const max = Math.ceil(peak / step) * step;
      y = (v) => base - (Math.min(v, max) / max) * (base - top);
      const digits = Math.max(0, -Math.floor(Math.log10(step)));
      ticks = Array.from({ length: Math.round(max / step) + 1 }, (_, k) => ({
        v: k * step,
        label: (k * step).toFixed(digits),
      }));
    }

    ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
    ctx.strokeStyle = colors.rule;
    ctx.lineWidth = 1;
    for (const t of ticks) {
      const gy = Math.round(y(t.v)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(x0, gy);
      ctx.lineTo(x1, gy);
      ctx.stroke();
    }
    drawCtcfs(ctx, x, base + 5, 8);
    if (columns.length) {
      ctx.strokeStyle = colors.extruding;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      columns.forEach((c, i) => (i ? ctx!.lineTo(c.px, y(c.v)) : ctx!.moveTo(c.px, y(c.v))));
      ctx.stroke();
    }
    // Labels last, with a paper-colored halo so the curve can't hide them.
    ctx.lineWidth = 3;
    ctx.lineJoin = "round";
    ctx.strokeStyle = colors.paper;
    ctx.fillStyle = colors.muted;
    for (const t of ticks) {
      const gy = Math.round(y(t.v)) - 3;
      ctx.strokeText(t.label, x0 + 3, gy);
      ctx.fillText(t.label, x0 + 3, gy);
    }
  }

  function pointermove(e: PointerEvent) {
    if (averaged === 0) return;
    const rect = canvas.getBoundingClientRect();
    const x = xScale(rect.width);
    const site = Math.round(((e.clientX - rect.left - x(0)) / (x(N_SITES - 1) - x(0))) * (N_SITES - 1));
    if (site < 0 || site >= N_SITES) {
      tooltip.hidden = true;
      return;
    }
    const v = counts[site]! / averaged;
    placeTooltip(tooltip, canvas, e, `Site ${fmt.format(site)}: ${v < 0.001 ? v.toExponential(1) : v.toFixed(3)} legs on average`);
  }

  onMount(() => {
    const stop = sizedCanvas(canvas, (c) => {
      ctx = c;
      draw();
    });
    lastTime = run.sim?.time;
    run.setDriver({
      frame: (f) => {
        accumulate(f.time, f.leftPos, f.rightPos);
        draw();
      },
      changed: (kind) => {
        if (kind === "restart") reset();
        else draw();
      },
    });
    return () => {
      stop();
      run.setDriver(undefined);
    };
  });
</script>

<div class="plot">
  <canvas bind:this={canvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  <div class="tooltip" bind:this={tooltip} hidden></div>
</div>
<p class="hint">
  Mean legs per site, accumulated over
  <span class="mono">{averaged > 0 ? fmt.format(averaged) : "0"}</span> time units.
</p>
<div class="head">
  <span class="seg" role="group" aria-label="Scale">
    <button
      aria-pressed={scale === "linear"}
      onclick={() => {
        scale = "linear";
        draw();
      }}>Linear</button
    >
    <button
      aria-pressed={scale === "log"}
      onclick={() => {
        scale = "log";
        draw();
      }}>Log</button
    >
  </span>
  <button class="btn" onclick={reset}>Reset average</button>
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  p {
    margin: 0;
  }

  .plot {
    position: relative;
  }

  canvas {
    display: block;
    width: 100%;
    height: 260px;
  }
</style>
