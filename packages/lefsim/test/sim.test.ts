import { describe, expect, it } from "vitest";

import fixture from "./fixtures/determinism_state.json";
import { coreNames, DIRECTION, MOBILITY, Sim, SITE_KIND, type SimOptions } from "../src/index.ts";

const TADS: SimOptions = {
  nSites: 3000,
  ctcfs: [600, 1300, 1800, 2500].flatMap((b) => [
    { site: b, capturesFrom: "left" as const, strength: 0.8 },
    { site: b + 1, capturesFrom: "right" as const, strength: 0.8 },
  ]),
  nLefs: 20,
  rates: { step: 1, respawn: 0.002, respawnCaptured: 0.0004 },
};

const make = (seed = 1, extra: Partial<SimOptions> = {}) => new Sim({ ...TADS, seed, ...extra });

/** A copy of the live state that outlives the next call into the sim. */
function snapshot(sim: Sim) {
  const s = sim.state();
  return {
    time: s.time,
    leftPos: Array.from(s.leftPos),
    rightPos: Array.from(s.rightPos),
    leftMobility: Array.from(s.leftMobility),
    rightMobility: Array.from(s.rightMobility),
  };
}

describe("names", () => {
  it("TS constants match the core's tables", () => {
    expect(coreNames()).toEqual({
      mobility: [...MOBILITY],
      direction: [...DIRECTION],
      siteKind: [...SITE_KIND],
    });
  });
});

describe("construction", () => {
  it("exposes sizes and defaults rates from the core", () => {
    using sim = new Sim({ nSites: 100, nLefs: 3 });
    expect([sim.nSites, sim.nLefs, sim.nLegs]).toEqual([100, 3, 6]);
    expect(sim.rates).toEqual({ step: 1, respawn: 1e-3, respawnCaptured: 1e-4 });
    expect(sim.time).toBe(0);
    expect(sim.eventCount).toBe(0);
  });

  it("places explicit LEF sites first", () => {
    using sim = new Sim({ nSites: 100, lefSites: [10, 90], nLefs: 2 });
    const s = sim.state();
    expect(Array.from(s.leftPos.subarray(0, 2))).toEqual([10, 90]);
    expect(Array.from(s.rightPos.subarray(0, 2))).toEqual([10, 90]);
  });

  it("accepts CTCFs as rows or as columns", () => {
    using rows = new Sim({
      nSites: 50,
      ctcfs: [{ site: 10, capturesFrom: "left", strength: 0.3 }, { site: 20, capturesFrom: "right" }],
    });
    using cols = new Sim({
      nSites: 50,
      ctcfs: { site: new Uint32Array([10, 20]), capturesFrom: ["left", "right"], strength: [0.3, 1] },
    });
    expect(cols.meta().ctcfs).toEqual(rows.meta().ctcfs);
    expect(Array.from(rows.meta().ctcfs.capturesFrom)).toEqual([0, 1]);
  });

  it.each([
    [{ nSites: 0 }, /nSites/],
    [{ nSites: 1.5 }, /nSites/],
    [{ nSites: 10, ctcfs: [{ site: -1, capturesFrom: "left" }] }, /non-negative integers/],
    [{ nSites: 10, ctcfs: [{ site: 2.5, capturesFrom: "left" }] }, /non-negative integers/],
    [{ nSites: 10, ctcfs: [{ site: 3, capturesFrom: "up" }] }, /"left" or "right"/],
    [{ nSites: 10, ctcfs: [{ site: 10, capturesFrom: "left" }] }, /outside a lattice/],
    [{ nSites: 10, ctcfs: [{ site: 3, capturesFrom: "left" }, { site: 3, capturesFrom: "left" }] }, /twice/],
    [{ nSites: 10, ctcfs: [{ site: 3, capturesFrom: "left", strength: 1.5 }] }, /\[0, 1\]/],
    [{ nSites: 10, ctcfs: { site: [3, 4], capturesFrom: ["left"] } }, /1 entries/],
    [{ nSites: 10, lefSites: [10] }, /outside/],
    [{ nSites: 10, nLefs: 6 }, /too many LEFs/],
    [{ nSites: 10, nLefs: -1 }, /nLefs/],
    [{ nSites: 10, seed: -1 }, /seed/],
    [{ nSites: 10, seed: 1.5 }, /seed/],
    [{ nSites: 10, seed: 2n ** 64n }, /seed/],
    [{ nSites: 10, rates: { step: 0 } }, /step rate must be positive/],
    [{ nSites: 10, rates: { respawn: Number.NaN } }, /respawn rate must be positive/],
  ] as [SimOptions, RegExp][])("rejects %o", (options, message) => {
    expect(() => new Sim(options)).toThrow(message);
  });

  it("accepts bigint seeds, and number seeds equal to them", () => {
    using a = make(0);
    using b = new Sim({ ...TADS, seed: 0n });
    using c = new Sim({ ...TADS, seed: 2n ** 64n - 1n });
    a.runUntil(100);
    b.runUntil(100);
    c.runUntil(100);
    expect(snapshot(a)).toEqual(snapshot(b));
    expect(snapshot(a)).not.toEqual(snapshot(c));
  });
});

describe("running", () => {
  it("reproduces the Python binding's state exactly (cross-language fixture)", () => {
    const { config, expected } = fixture;
    using sim = new Sim({
      nSites: config.nSites,
      ctcfs: {
        site: config.ctcfSites,
        capturesFrom: config.ctcfCapturesFrom as ("left" | "right")[],
        strength: config.ctcfStrength,
      },
      nLefs: config.nLefs,
      seed: config.seed,
      rates: config.rates,
    });
    sim.runUntil(config.tEnd);
    expect(sim.eventCount).toBe(expected.eventCount);
    expect(sim.nextEventTime).toBe(expected.nextEventTime);
    const { time, ...state } = snapshot(sim);
    expect(time).toBe(expected.time);
    expect(state).toEqual({
      leftPos: expected.leftPos,
      rightPos: expected.rightPos,
      leftMobility: expected.leftMobility,
      rightMobility: expected.rightMobility,
    });
  });

  it("runUntil takes an absolute time; repeating it is a no-op", () => {
    using sim = make();
    const first = sim.runUntil(100);
    expect(first).toMatchObject({ time: 100, stopped: "reached_time" });
    expect(first.eventsFired).toBeGreaterThan(0);
    const again = sim.runUntil(100);
    expect(again).toEqual({ eventsFired: 0, time: 100, stopped: "reached_time" });
    expect(sim.runUntil(50).eventsFired).toBe(0);
    expect(sim.time).toBe(100);
  });

  it("split runs end in the same state as one run", () => {
    using whole = make(2);
    whole.runUntil(500);
    using split = make(2);
    for (const t of [123.4, 123.4, 300, 500]) split.runUntil(t);
    expect(split.eventCount).toBe(whole.eventCount);
    expect(snapshot(split)).toEqual(snapshot(whole));
  });

  it("maxEvents caps the work per call and resumes seamlessly", () => {
    using whole = make(3);
    whole.runUntil(400);
    using capped = make(3);
    let calls = 0;
    for (;;) {
      const r = capped.runUntil(400, { maxEvents: 97 });
      calls++;
      expect(r.eventsFired).toBeLessThanOrEqual(97);
      if (r.stopped === "reached_time") break;
      expect(r.stopped).toBe("reached_event_count");
      expect(r.time).toBeLessThan(400);
    }
    expect(calls).toBeGreaterThan(10);
    expect(capped.eventCount).toBe(whole.eventCount);
    expect(snapshot(capped)).toEqual(snapshot(whole));
  });

  it("runEvents fires exactly n events", () => {
    using sim = make();
    expect(sim.runEvents(1234)).toMatchObject({ eventsFired: 1234, stopped: "reached_event_count" });
    expect(sim.eventCount).toBe(1234);
  });

  it("an empty sim is exhausted", () => {
    using sim = new Sim({ nSites: 10 });
    expect(sim.nextEventTime).toBeNull();
    expect(sim.runEvents(5)).toEqual({ eventsFired: 0, time: 0, stopped: "exhausted" });
    expect(sim.runUntil(3)).toMatchObject({ time: 3, stopped: "exhausted" });
  });

  it.each([Number.NaN])("rejects tEnd = %s", (t) => {
    using sim = make();
    expect(() => sim.runUntil(t)).toThrow(/NaN/);
  });
});

describe("live state", () => {
  it("is a set of typed-array views, one entry per LEF", () => {
    using sim = make();
    sim.runUntil(1000);
    const s = sim.state();
    expect(s.leftPos).toBeInstanceOf(Uint32Array);
    expect(s.leftMobility).toBeInstanceOf(Uint8Array);
    expect(s.leftPos.length).toBe(20);
    expect(s.time).toBe(1000);
    for (let i = 0; i < 20; i++) {
      expect(s.leftPos[i]!).toBeLessThanOrEqual(s.rightPos[i]!);
      expect(s.leftMobility[i]!).toBeLessThan(MOBILITY.length);
    }
  });

  it("stays correct across wasm memory growth, which detaches old views", () => {
    using sim = make(4);
    sim.runUntil(200);
    const old = sim.state();
    const before = snapshot(sim);
    // Force memory to grow with a large allocation.
    const big = new Sim({ nSites: 2_000_000, nLefs: 10 });
    expect(old.leftPos.length).toBe(0); // detached
    expect(snapshot(sim)).toEqual(before);
    big.free();
  });

  it("works with zero LEFs", () => {
    using sim = new Sim({ nSites: 10 });
    expect(sim.state().leftPos.length).toBe(0);
  });
});

describe("meta", () => {
  it("describes the lattice, CTCFs and id mappings", () => {
    using sim = make(1, { nLefs: 4 });
    const m = sim.meta();
    expect(m.nSites).toBe(3000);
    expect(m.rates).toEqual(TADS.rates);
    expect(m.lattice.kind.length).toBe(3000);
    expect(m.lattice.kind.reduce((a, k) => a + k, 0)).toBe(8);
    expect(Array.from(m.ctcfs.site)).toEqual([600, 601, 1300, 1301, 1800, 1801, 2500, 2501]);
    expect(Array.from(m.ctcfs.capturesFrom)).toEqual([0, 1, 0, 1, 0, 1, 0, 1]);
    expect(Array.from(m.lefs.leftLegId)).toEqual([0, 2, 4, 6]);
    expect(Array.from(m.lefs.rightLegId)).toEqual([1, 3, 5, 7]);
    expect(Array.from(m.legs.lefId)).toEqual([0, 0, 1, 1, 2, 2, 3, 3]);
    expect(Array.from(m.legs.facing)).toEqual([0, 1, 0, 1, 0, 1, 0, 1]);
  });
});

describe("lifetime", () => {
  it("free() is idempotent, and a freed Sim throws clearly", () => {
    const sim = make();
    sim.free();
    sim.free();
    expect(() => sim.time).toThrow(/freed/);
    expect(() => sim.runUntil(1)).toThrow(/freed/);
  });

  it("Symbol.dispose frees", () => {
    const sim = make();
    sim[Symbol.dispose]();
    expect(() => sim.state()).toThrow(/freed/);
  });

  it("using frees at the end of the block", () => {
    let escaped: Sim;
    {
      using sim = make();
      escaped = sim;
      expect(escaped.time).toBe(0);
    }
    expect(() => escaped.time).toThrow(/freed/);
  });
});
