<!-- Leg positions over time, newest at the top, one row per frame. -->
<script lang="ts">
  import { onMount } from "svelte";

  import { Kymograph } from "../../render.ts";
  import type { Run } from "../run.svelte.ts";

  let { run }: { run: Run } = $props();

  let canvas: HTMLCanvasElement;

  onMount(() => {
    const kymograph = new Kymograph(canvas);
    const ro = new ResizeObserver(() => kymograph.resize());
    ro.observe(canvas);
    run.setDriver({
      frame: (f) => kymograph.push(f),
      changed: (kind) => {
        if (kind === "restart") kymograph.reset();
      },
    });
    return () => {
      ro.disconnect();
      run.setDriver(undefined);
    };
  });
</script>

<canvas bind:this={canvas}></canvas>
<p class="hint">The kymograph shows each leg's position over time, one row per animation frame, newest at the top.</p>

<style>
  p {
    margin: 0;
  }

  canvas {
    display: block;
    width: 100%;
    height: 360px;
  }
</style>
