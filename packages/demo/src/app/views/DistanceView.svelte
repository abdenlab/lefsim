<!--
  Effective distance between two probe sites, dragged on the stage: now,
  against the distance along the lattice, and as a trace over simulation time.
-->
<script lang="ts">
  import { effectiveDistance } from "lefsim";
  import { onMount } from "svelte";

  import { worldOf } from "../../model.ts";
  import { colors, fmt } from "../../render.ts";
  import { niceStep, sizedCanvas } from "../canvas.ts";
  import type { Run } from "../run.svelte.ts";

  let { run }: { run: Run } = $props();

  const LENGTHS = { stepLength: 1, hopLength: 1 };
  /** Simulation time the trace spans. */
  const WINDOW = 2_000;

  let probes = $state<[number, number]>([1000, 2000]);
  let distance = $state(0);
  const lattice = $derived(Math.abs(probes[1] - probes[0]));

  let canvas: HTMLCanvasElement;
  let ctx: CanvasRenderingContext2D | undefined;
  /** The trace: (time, distance) samples, one per frame, over the last WINDOW of time. */
  let trace: { t: number; d: number }[] = [];

  function measure() {
    const sim = run.sim;
    if (!sim) return;
    distance = effectiveDistance(worldOf(sim), probes[0], probes[1], LENGTHS);
    const t = sim.time;
    if (trace.length && t < trace.at(-1)!.t) trace = [];
    if (!trace.length || t > trace.at(-1)!.t) trace.push({ t, d: distance });
    while (trace.length > 2 && trace[0]!.t < t - WINDOW) trace.shift();
  }

  function draw() {
    if (!ctx) return;
    const { width, height } = canvas.getBoundingClientRect();
    const left = 44;
    const right = 10;
    const top = 10;
    const bottom = height - 26;
    ctx.clearRect(0, 0, width, height);
    const max = Math.max(1, lattice);
    const y = (d: number) => bottom - (Math.min(d, max) / max) * (bottom - top);
    const tEnd = trace.at(-1)?.t ?? 0;
    const x = (t: number) => left + ((t - (tEnd - WINDOW)) / WINDOW) * (width - left - right);

    // Distance gridlines, up to the lattice distance (which no effective distance exceeds).
    ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    const step = niceStep(max / 4);
    for (let v = 0; v <= max; v += step) {
      ctx.fillStyle = colors.rule;
      ctx.fillRect(left, Math.round(y(v)), width - left - right, 1);
      ctx.fillStyle = colors.muted;
      ctx.fillText(fmt.format(v), left - 6, y(v));
    }
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.fillText(`the last ${fmt.format(WINDOW)} time units`, left + (width - left - right) / 2, bottom + 8);

    ctx.strokeStyle = colors.probe;
    ctx.lineWidth = 2;
    ctx.beginPath();
    trace.forEach((s, i) => (i ? ctx!.lineTo(x(s.t), y(s.d)) : ctx!.moveTo(x(s.t), y(s.d))));
    ctx.stroke();
  }

  onMount(() => {
    const stop = sizedCanvas(canvas, (c) => {
      ctx = c;
      draw();
    });
    run.markers = {
      sites: probes,
      move: (i, site) => {
        if (site === probes[1 - i]) return; // one probe per site
        probes[i] = site;
        run.markers = { ...run.markers!, sites: [...probes] };
        trace = [];
        measure();
        draw();
      },
    };
    run.setDriver({
      frame: () => {
        measure();
        draw();
      },
    });
    return () => {
      stop();
      run.setDriver(undefined);
      run.markers = undefined;
    };
  });
</script>

<div class="stats">
  <div><b class="mono">{fmt.format(distance)}</b><span>effective distance</span></div>
  <div><b class="mono">{fmt.format(lattice)}</b><span>along the lattice</span></div>
  <p class="hint">
    Drag the two probes on the stage. The effective distance is the shortest path between them when a path may hop
    across loops: each step and each hop has length 1.
  </p>
</div>
<canvas bind:this={canvas}></canvas>

<style>
  .stats {
    display: flex;
    align-items: center;
    gap: 28px;
    flex-wrap: wrap;
  }

  .stats div {
    display: flex;
    flex-direction: column;
  }

  .stats b {
    font-size: 26px;
    font-weight: 600;
  }

  .stats span {
    font-size: 13px;
    color: var(--muted);
  }

  .stats p {
    margin: 0;
    flex: 1 1 280px;
  }

  canvas {
    display: block;
    width: 100%;
    height: 280px;
  }
</style>
