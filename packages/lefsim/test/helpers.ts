/** Test setups in the shape the tests were written in, built through the model API. */

import {
  type CtcfColumns,
  type CtcfSpec,
  createLattice,
  createModel,
  type Model,
  populate,
  type Rates,
  Sim,
} from "../src/index.ts";

export interface Setup {
  nSites: number;
  ctcfs?: CtcfSpec[] | CtcfColumns;
  /** LEFs placed by `populate` with the same seed as the run. */
  nLefs?: number;
  seed?: number | bigint;
  rates?: Partial<Rates>;
}

export function makeModel(s: Setup): Model {
  const lattice = createLattice(s.nSites, s.ctcfs);
  return createModel(populate(lattice, s.nLefs ?? 0, { seed: s.seed }), s.rates ?? {});
}

/** A Sim as the Python fixtures build it: LEFs placed with the seed, run with the same seed. */
export function makeSim(s: Setup): Sim {
  return new Sim(makeModel(s), { seed: s.seed });
}
