// Small helpers for the views' canvases.

import { fit, onThemeChange } from "../render.ts";

/**
 * Keep a canvas's backing store sized to its box, calling `redraw` with a
 * fresh context after each resize and theme change. Returns a cleanup.
 */
export function sizedCanvas(canvas: HTMLCanvasElement, redraw: (ctx: CanvasRenderingContext2D) => void): () => void {
  let ctx = fit(canvas);
  const ro = new ResizeObserver(() => {
    ctx = fit(canvas);
    redraw(ctx);
  });
  ro.observe(canvas);
  const unsubscribe = onThemeChange(() => redraw(ctx));
  return () => {
    unsubscribe();
    ro.disconnect();
  };
}

/** A round step near `target` (1, 2 or 5 times a power of ten), for axis ticks. */
export function niceStep(target: number): number {
  const p = 10 ** Math.floor(Math.log10(Math.max(target, 1e-12)));
  return [1, 2, 5, 10].map((m) => m * p).find((v) => v >= target) ?? p;
}
