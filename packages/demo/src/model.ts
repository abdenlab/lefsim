// The model both demo pages simulate: a 3,000-site lattice split into five
// TADs by convergent CTCF pairs.

import type { SimOptions } from "lefsim";

export const N_SITES = 3000;
export const BOUNDARIES = [600, 1300, 1800, 2500];
export const CTCFS = BOUNDARIES.flatMap((b) => [
  { site: b, capturesFrom: "left" as const, strength: 0.8 },
  { site: b + 1, capturesFrom: "right" as const, strength: 0.8 },
]);
export const RATES = { step: 1, respawn: 0.002, respawnCaptured: 0.0004 };

export function options(nLefs: number, seed: number): SimOptions {
  return { nSites: N_SITES, ctcfs: CTCFS, nLefs, seed, rates: RATES };
}

/** Clamp the LEF-count and seed inputs to valid values (and write them back). */
export function readRunInputs(nLefsIn: HTMLInputElement, seedIn: HTMLInputElement) {
  const nLefs = Math.max(0, Math.min(700, Math.floor(Number(nLefsIn.value) || 0)));
  const seed = Math.max(0, Math.floor(Number(seedIn.value) || 0));
  nLefsIn.value = String(nLefs);
  seedIn.value = String(seed);
  return { nLefs, seed };
}
