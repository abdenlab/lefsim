<!--
  Leg occupancy: the mean number of legs on each site, averaged over
  simulation time while this view is open. Each frame's state is weighted by
  the time since the previous frame, so pausing adds nothing.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import { colors, drawCtcfs, drawTadBands, fmt, placeTooltip, xScale } from "../../render.ts";
  import { sizedCanvas } from "../canvas.ts";
  import { N_SITES, type Run } from "../run.svelte.ts";

  let { run }: { run: Run } = $props();

  const LOG_MIN = 1e-4;

  let canvas: HTMLCanvasElement;
  let tooltip: HTMLDivElement;
  let ctx: CanvasRenderingContext2D | undefined;
  let counts = new Float64Array(N_SITES);
  let lastTime: number | undefined;
  let averaged = $state(0);

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
    const y = (v: number) =>
      base - ((Math.log10(Math.max(v, LOG_MIN)) - Math.log10(LOG_MIN)) / -Math.log10(LOG_MIN)) * (base - top);
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
    drawCtcfs(ctx, x, base + 5, 8);
    if (averaged > 0) {
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
        for (let s = lo; s <= hi; s++) sum += counts[s]!;
        const py = y(sum / (hi - lo + 1) / averaged);
        if (px === Math.floor(x0)) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.stroke();
    }
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

<div class="head">
  <p class="hint">
    Mean legs per site, log scale, averaged over
    <span class="mono">{averaged > 0 ? fmt.format(averaged) : "0"}</span> time units while this view is open.
  </p>
  <button class="btn" onclick={reset}>Reset average</button>
</div>
<div class="plot">
  <canvas bind:this={canvas} onpointermove={pointermove} onpointerleave={() => (tooltip.hidden = true)}></canvas>
  <div class="tooltip" bind:this={tooltip} hidden></div>
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
