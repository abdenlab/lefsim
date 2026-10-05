import { describe, expect, it } from "vitest";

import {
  createLattice,
  createWorld,
  effectiveDistance,
  effectiveDistances,
  effectiveDistancesOver,
  type World,
} from "../src/index.ts";
import { makeSim } from "./helpers.ts";

const lattice = createLattice(100);
// A rosette of two: LEF 1 at 10–20, LEF 2 at 21–30.
const rosette = createWorld(lattice, [
  { leftPos: 10, rightPos: 20 },
  { leftPos: 21, rightPos: 30 },
]);
const lengths = { stepLength: 10, hopLength: 15 };
const s = lengths.stepLength;
const h = lengths.hopLength;

describe("effectiveDistance", () => {
  it("walks, hops, and crosses a rosette at a hop per LEF with free junctions", () => {
    const d = (a: number, b: number) => effectiveDistance(rosette, a, b, lengths);
    expect(d(20, 21)).toBe(s); // the junction step
    expect(d(20, 30)).toBe(s + h);
    expect(d(10, 30)).toBe(2 * h); // over the chain: a hop per LEF, the junction free
    expect(d(10, 21)).toBe(h + s);
    expect(d(5, 35)).toBe(5 * s + 2 * h + 5 * s);
    expect(d(35, 5)).toBe(d(5, 35));
    expect(d(40, 40)).toBe(0);
    expect(effectiveDistance(createWorld(lattice), 5, 35, lengths)).toBe(30 * s);
  });

  it("rejects bad lengths and sites", () => {
    expect(() => effectiveDistance(rosette, 1, 2, { stepLength: 0, hopLength: 1 })).toThrow(
      /step length must be positive/,
    );
    expect(() => effectiveDistance(rosette, 1, 2, { stepLength: 1.5, hopLength: 1 })).toThrow(
      /stepLength must be a non-negative integer/,
    );
    expect(() => effectiveDistance(rosette, 1, 2, { stepLength: 1, hopLength: -1 })).toThrow(
      /hopLength must be a non-negative integer/,
    );
    expect(() => effectiveDistance(rosette, 1, 2, { stepLength: 2 ** 50, hopLength: 1 })).toThrow(/exceed 2\^53/);
    expect(() => effectiveDistance(rosette, 1, 100, lengths)).toThrow(/site 100 is outside a lattice of 100 sites/);
    expect(() => effectiveDistance(rosette, -1, 2, lengths)).toThrow(/sites must be non-negative integers/);
  });
});

describe("effectiveDistances", () => {
  const sites = [0, 10, 20, 21, 30, 55, 99];

  it("gives every pair, condensed, or sites against targets", () => {
    const k = sites.length;
    const pairs = effectiveDistances(rosette, sites, lengths);
    const full = effectiveDistances(rosette, sites, { ...lengths, targets: sites });
    expect(pairs.length).toBe((k * (k - 1)) / 2);
    expect(full.length).toBe(k * k);
    let at = 0;
    for (let i = 0; i < k; i++) {
      expect(full[i * k + i]).toBe(0);
      for (let j = i + 1; j < k; j++) {
        expect(pairs[at++]).toBe(full[i * k + j]);
        expect(full[j * k + i]).toBe(full[i * k + j]);
        expect(full[i * k + j]).toBe(effectiveDistance(rosette, sites[i]!, sites[j]!, lengths));
      }
    }
    expect(effectiveDistances(rosette, new Uint32Array(sites), lengths)).toEqual(pairs);
  });

  it("over a recording, matches each snapshot's world", () => {
    using sim = makeSim({ nSites: 500, nLefs: 12, seed: 3, rates: { step: 1, respawn: 0.01 } });
    const lat = sim.initialModel().world.lattice;
    const snaps = sim.recordUntil(300, { dt: 25 }).snapshots!;
    const sites = [3, 77, 150, 151, 260, 499];
    const pairs = (sites.length * (sites.length - 1)) / 2;
    const d = effectiveDistancesOver(lat, snaps, sites, { stepLength: 1000, hopLength: 1000 });
    expect(d.length).toBe(snaps.time.length * pairs);
    const n = snaps.nLefs;
    for (let t = 0; t < snaps.time.length; t++) {
      const world: World = createWorld(lat, {
        leftPos: snaps.leftPos.subarray(t * n, (t + 1) * n),
        rightPos: snaps.rightPos.subarray(t * n, (t + 1) * n),
      });
      const expected = effectiveDistances(world, sites, { stepLength: 1000, hopLength: 1000 });
      expect(d.subarray(t * pairs, (t + 1) * pairs)).toEqual(expected);
    }
  });
});
