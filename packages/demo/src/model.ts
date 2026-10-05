// The model both demo pages simulate: a 3,000-site lattice split into five
// TADs by convergent CTCF pairs.

import { createLattice, createModel, populate, Sim, type World } from "lefsim";

export const N_SITES = 3000;
export const BOUNDARIES = [600, 1300, 1800, 2500];
export const CTCFS = BOUNDARIES.flatMap((b) => [
  { site: b, capturesFrom: "left" as const, strength: 0.8 },
  { site: b + 1, capturesFrom: "right" as const, strength: 0.8 },
]);
export const RATES = { step: 1, respawn: 0.002, respawnBlocked: 0.02, respawnCaptured: 0.002 };

/**
 * A new run of the model with `nLefs` LEFs: placed with `seed`, and run with
 * the same seed. Call after `init()`.
 */
export function newSim(nLefs: number, seed: number): Sim {
  const lattice = createLattice(N_SITES, CTCFS);
  const model = createModel(populate(lattice, nLefs, { seed }), RATES);
  return new Sim(model, { seed });
}

/**
 * The sim's current state as a World, with copied columns. The live state's
 * arrays are views into wasm memory, which the next wasm call can detach
 * (if it grows memory), so they're copied before being passed to one.
 */
export function worldOf(sim: Sim): World {
  const s = sim.state();
  return {
    lattice: sim.lattice,
    lefs: { leftPos: s.leftPos.slice(), rightPos: s.rightPos.slice(), leftMobility: s.leftMobility.slice(), rightMobility: s.rightMobility.slice() },
  };
}

/** Clamp the LEF-count and seed inputs to valid values (and write them back). */
export function readRunInputs(nLefsIn: HTMLInputElement, seedIn: HTMLInputElement) {
  const nLefs = Math.max(0, Math.min(700, Math.floor(Number(nLefsIn.value) || 0)));
  const seed = Math.max(0, Math.floor(Number(seedIn.value) || 0));
  nLefsIn.value = String(nLefs);
  seedIn.value = String(seed);
  return { nLefs, seed };
}
