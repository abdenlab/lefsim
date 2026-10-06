<!--
  The stage: loops as arcs over the lattice, the two CTCF lanes, and any
  markers the visible view puts on the lattice (e.g. probes).

  CTCFs are edited here: click an empty spot in a lane to add one facing that
  way, drag one to move it, or drag it out of the lanes to remove it.
-->
<script lang="ts">
  import { onMount } from "svelte";

  import { colors, drawTadBands, fit, fmt, MARGIN, onThemeChange, placeTooltip, xScale } from "../render.ts";
  import { N_SITES, type Run } from "./run.svelte.ts";

  let { run }: { run: Run } = $props();

  /** Layout, in CSS pixels: the arcs, then the lanes, then the markers row. */
  const ARCS = 190;
  const LANE = 22;
  const LANES_TOP = ARCS + 10;
  const MARKERS_TOP = LANES_TOP + 2 * LANE + 6;

  let canvas: HTMLCanvasElement;
  let tooltip: HTMLDivElement;
  let ctx: CanvasRenderingContext2D | undefined;
  let width = $state(0);
  /** The CTCF being dragged (index into run.ctcfs), and whether it's out of the lanes (to be removed). */
  let drag = $state<{ ctcf: number; out: boolean } | { marker: number } | undefined>(undefined);
  let hover = $state<{ ctcf: number } | { marker: number } | undefined>(undefined);
  let themeRevision = $state(0);

  const height = $derived(run.markers ? MARKERS_TOP + 26 : MARKERS_TOP);

  /** The lane a CTCF facing this way sits in: capturing from the left on top. */
  const laneTop = (left: boolean) => LANES_TOP + (left ? 0 : LANE);

  function siteAtX(mx: number): number {
    const inner = width - MARGIN.left - MARGIN.right;
    const s = Math.round(((mx - MARGIN.left) / inner) * (N_SITES - 1));
    return Math.max(1, Math.min(N_SITES - 2, s));
  }

  function draw() {
    if (!ctx) return;
    const x = xScale(width);
    const f = run.frame;
    ctx.clearRect(0, 0, width, height);
    drawTadBands(ctx, x, MARGIN.top, ARCS);

    // The lattice.
    ctx.strokeStyle = colors.ink2;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x(0), ARCS + 0.5);
    ctx.lineTo(x(N_SITES - 1), ARCS + 0.5);
    ctx.stroke();

    // The lanes, labeled at the end where they point.
    ctx.font = "11px 'Atkinson Hyperlegible Next', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    for (const left of [true, false]) {
      const top = laneTop(left);
      ctx.fillStyle = colors.band;
      ctx.fillRect(x(0), top + 2, x(N_SITES - 1) - x(0), LANE - 4);
      ctx.fillStyle = colors.muted;
      ctx.textAlign = left ? "left" : "right";
      ctx.fillText(
        left ? "◂ captures from the left" : "captures from the right ▸",
        left ? x(0) + 4 : x(N_SITES - 1) - 4,
        top + LANE / 2,
      );
    }
    run.ctcfs.forEach((c, i) => {
      const top = laneTop(c.left);
      const active =
        (drag && "ctcf" in drag && drag.ctcf === i) || (hover && "ctcf" in hover && hover.ctcf === i);
      const size = active ? 16 : 12;
      const dir = c.left ? -1 : 1;
      const px = x(c.site);
      const cy = top + LANE / 2;
      ctx!.globalAlpha = drag && "ctcf" in drag && drag.ctcf === i && drag.out ? 0.25 : 1;
      // A faint stem up to the lattice, so the site reads against the arcs.
      ctx!.fillStyle = colors.rule;
      ctx!.fillRect(Math.round(px), ARCS + 1, 1, top - ARCS + 2);
      ctx!.fillStyle = colors.ink;
      ctx!.beginPath();
      ctx!.moveTo(px + dir * size * 0.6, cy);
      ctx!.lineTo(px, cy - size / 2);
      ctx!.lineTo(px, cy + size / 2);
      ctx!.closePath();
      ctx!.fill();
      ctx!.globalAlpha = 1;
    });

    // The view's markers.
    const markers = run.markers;
    if (markers) {
      markers.sites.forEach((s, i) => {
        const px = x(s);
        const active =
          (drag && "marker" in drag && drag.marker === i) || (hover && "marker" in hover && hover.marker === i);
        ctx!.strokeStyle = colors.probe;
        ctx!.setLineDash([2, 3]);
        ctx!.beginPath();
        ctx!.moveTo(px, MARGIN.top);
        ctx!.lineTo(px, MARKERS_TOP + 6);
        ctx!.stroke();
        ctx!.setLineDash([]);
        ctx!.fillStyle = colors.probe;
        ctx!.beginPath();
        ctx!.arc(px, MARKERS_TOP + 13, active ? 7 : 5.5, 0, 2 * Math.PI);
        ctx!.fill();
      });
    }

    if (!f) return;
    // Loops as half-ellipses whose height grows with span; held loops on top.
    const maxH = ARCS - MARGIN.top - 4;
    const full = 900;
    ctx.lineWidth = 2;
    for (const pass of [0, 1]) {
      ctx.strokeStyle = pass ? colors.held : colors.extruding;
      ctx.beginPath();
      for (let i = 0; i < f.leftPos.length; i++) {
        if (f.held[i] !== pass) continue;
        const x1 = x(f.leftPos[i]!);
        const x2 = x(f.rightPos[i]!);
        const r = (x2 - x1) / 2;
        const h = Math.min(maxH, Math.max(2, (maxH * (f.rightPos[i]! - f.leftPos[i]!)) / full));
        ctx.moveTo(x1, ARCS);
        ctx.ellipse(x1 + r, ARCS, Math.max(r, 0.5), h, 0, Math.PI, 2 * Math.PI);
      }
      ctx.stroke();
    }
    for (let i = 0; i < f.leftPos.length; i++) {
      ctx.fillStyle = f.held[i] ? colors.held : colors.extruding;
      ctx.fillRect(x(f.leftPos[i]!) - 1, ARCS - 4, 2, 5);
      ctx.fillRect(x(f.rightPos[i]!) - 1, ARCS - 4, 2, 5);
    }
  }

  // Redraw on any change the drawing reads.
  $effect(() => {
    void [run.frame, run.revision, run.markers, run.ctcfs.length, drag, hover, width, height, themeRevision];
    if (ctx && canvas.height !== Math.round(height * (devicePixelRatio || 1))) ctx = fit(canvas);
    draw();
  });

  // ---------------------------------------------------------------- pointer

  function point(e: PointerEvent) {
    const rect = canvas.getBoundingClientRect();
    return { mx: e.clientX - rect.left, my: e.clientY - rect.top };
  }

  /** What's under the pointer: a CTCF (index) or a marker (index), or which lane is empty there. */
  function hit(mx: number, my: number): { ctcf: number } | { marker: number } | { lane: boolean } | undefined {
    const x = xScale(width);
    if (run.markers && my >= MARKERS_TOP && my < MARKERS_TOP + 26) {
      let best = -1;
      let bestPx = 14;
      run.markers.sites.forEach((s, i) => {
        const d = Math.abs(x(s) - mx);
        if (d < bestPx) [best, bestPx] = [i, d];
      });
      return best >= 0 ? { marker: best } : undefined;
    }
    if (my < LANES_TOP || my >= LANES_TOP + 2 * LANE) return undefined;
    const left = my < LANES_TOP + LANE;
    let best = -1;
    let bestPx = 10;
    run.ctcfs.forEach((c, i) => {
      if (c.left !== left) return;
      const d = Math.abs(x(c.site) - mx);
      if (d < bestPx) [best, bestPx] = [i, d];
    });
    return best >= 0 ? { ctcf: best } : { lane: left };
  }

  function pointerdown(e: PointerEvent) {
    const { mx, my } = point(e);
    const h = hit(mx, my);
    if (!h) return;
    if ("lane" in h) {
      // Add a CTCF here, facing the lane's way, and keep dragging it.
      if (!run.addCtcf(siteAtX(mx), h.lane)) return;
      drag = { ctcf: run.ctcfs.length - 1, out: false };
    } else if ("ctcf" in h) {
      drag = { ctcf: h.ctcf, out: false };
    } else {
      drag = { marker: h.marker };
    }
    canvas.setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function pointermove(e: PointerEvent) {
    const { mx, my } = point(e);
    if (drag && "marker" in drag) {
      run.markers?.move(drag.marker, siteAtX(mx));
      return;
    }
    if (drag) {
      const out = my < LANES_TOP - 24 || my > LANES_TOP + 2 * LANE + 24;
      if (out !== drag.out) drag = { ...drag, out };
      if (!out) run.moveCtcf(drag.ctcf, siteAtX(mx));
      return;
    }
    const h = hit(mx, my);
    hover = h && !("lane" in h) ? h : undefined;
    canvas.style.cursor = hover ? "grab" : h ? "copy" : "";
    loopTooltip(e, mx, my);
  }

  function pointerup() {
    if (drag && "ctcf" in drag && drag.out) run.removeCtcf(drag.ctcf);
    drag = undefined;
  }

  /** Hovering the arcs: the innermost loop around the pointer. */
  function loopTooltip(e: PointerEvent, mx: number, my: number) {
    const f = run.frame;
    if (!f || my > ARCS) {
      tooltip.hidden = true;
      return;
    }
    const site = siteAtX(mx);
    let best = -1;
    for (let i = 0; i < f.leftPos.length; i++) {
      if (f.leftPos[i]! - 4 <= site && site <= f.rightPos[i]! + 4) {
        if (best < 0 || f.rightPos[i]! - f.leftPos[i]! < f.rightPos[best]! - f.leftPos[best]!) best = i;
      }
    }
    if (best < 0) {
      tooltip.hidden = true;
      return;
    }
    const l = f.leftPos[best]!;
    const r = f.rightPos[best]!;
    placeTooltip(
      tooltip,
      canvas,
      e,
      `LEF ${best}: sites ${fmt.format(l)}–${fmt.format(r)}, ${fmt.format(r - l)} long, ${f.held[best] ? "held at a CTCF" : "extruding"}`,
    );
  }

  onMount(() => {
    const ro = new ResizeObserver(() => {
      width = canvas.getBoundingClientRect().width;
      ctx = fit(canvas);
      draw();
    });
    ro.observe(canvas);
    onThemeChange(() => themeRevision++);
    return () => ro.disconnect();
  });
</script>

<div class="stage">
  <canvas
    bind:this={canvas}
    style:height="{height}px"
    onpointerdown={pointerdown}
    onpointermove={pointermove}
    onpointerup={pointerup}
    onpointercancel={pointerup}
    onpointerleave={() => {
      if (!drag) {
        hover = undefined;
        tooltip.hidden = true;
      }
    }}
    aria-label="Loops on the lattice, with CTCF lanes: click a lane to add a CTCF, drag one to move it, drag it out of the lanes to remove it"
  ></canvas>
  <div class="tooltip" bind:this={tooltip} hidden></div>
</div>

<style>
  .stage {
    position: relative;
  }

  canvas {
    display: block;
    width: 100%;
    touch-action: none;
  }
</style>
