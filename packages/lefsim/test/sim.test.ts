import { describe, expect, it } from "vitest";

import fixture from "./fixtures/determinism_state.json";
import modelFixture from "./fixtures/model.json";
import {
  coreNames,
  createLattice,
  createModel,
  createWorld,
  DIRECTION,
  MOBILITY,
  modelFromJson,
  modelToJson,
  populate,
  ROW_KIND,
  Sim,
  SITE_KIND,
} from "../src/index.ts";
import { makeModel, makeSim, type Setup } from "./helpers.ts";

const TADS: Setup = {
  nSites: 3000,
  ctcfs: [600, 1300, 1800, 2500].flatMap((b) => [
    { site: b, capturesFrom: "left" as const, strength: 0.8 },
    { site: b + 1, capturesFrom: "right" as const, strength: 0.8 },
  ]),
  nLefs: 20,
  rates: { step: 1, respawn: 0.002, respawnBlocked: 0.02, respawnCaptured: 0.0004 },
};

const make = (seed = 1, extra: Partial<Setup> = {}) => makeSim({ ...TADS, seed, ...extra });

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
      rowKind: [...ROW_KIND],
    });
  });
});

describe("building a model", () => {
  it("createLattice takes CTCFs as rows or columns, names or codes", () => {
    const rows = createLattice(50, [{ site: 10, capturesFrom: "left", strength: 0.3 }, { site: 20, capturesFrom: "Right" as "right" }]); // names are case-insensitive
    const cols = createLattice(50, { site: new Uint32Array([10, 20]), capturesFrom: [0, "right"], strength: [0.3, 1] });
    expect(cols).toEqual(rows);
    expect(Array.from(rows.ctcfs.capturesFrom)).toEqual([0, 1]);
    expect(Array.from(createLattice(50, { site: [1, 2], capturesFrom: "left" }).ctcfs.strength)).toEqual([1, 1]);
  });

  it("createWorld takes LEFs as rows or columns; mobility defaults to moving", () => {
    const lattice = createLattice(100, [{ site: 60, capturesFrom: "left" }]);
    const rows = createWorld(lattice, [{ leftPos: 10, rightPos: 12 }, { leftPos: 30, rightPos: 60, rightMobility: "captured" }]);
    const cols = createWorld(lattice, { leftPos: [10, 30], rightPos: [12, 60], rightMobility: [0, 1] });
    expect(cols).toEqual(rows);
    expect(Array.from(rows.lefs.leftMobility)).toEqual([0, 0]);
  });

  it("populate places LEFs reproducibly, with its own seed", () => {
    const lattice = createLattice(3000);
    const a = populate(lattice, 20, { seed: 1 });
    expect(a.lefs.leftPos.length).toBe(20);
    expect(Array.from(a.lefs.leftPos)).toEqual(Array.from(a.lefs.rightPos));
    expect(populate(lattice, 20, { seed: 1 })).toEqual(a);
    expect(populate(lattice, 20, { seed: 2 })).not.toEqual(a);
  });

  it("createModel fills missing rates with the core's defaults", () => {
    const model = createModel(createWorld(createLattice(100)));
    expect(model.rates).toEqual({ step: 1, respawn: 1e-3, respawnBlocked: 1e-2, respawnCaptured: 1e-3 });
    expect(model.spawnRule).toBe("uniform_non_ctcf");
  });

  const lattice10 = () => createLattice(10, [{ site: 3, capturesFrom: "left" }]);
  it.each([
    [() => createLattice(0), /at least one site/],
    [() => createLattice(1.5), /nSites/],
    [() => createLattice(10, [{ site: -1, capturesFrom: "left" }]), /non-negative integers/],
    [() => createLattice(10, [{ site: 2.5, capturesFrom: "left" }]), /non-negative integers/],
    [() => createLattice(10, [{ site: 3, capturesFrom: "up" as "left" }]), /"left" or "right"/],
    [() => createLattice(10, [{ site: 10, capturesFrom: "left" }]), /outside a lattice/],
    [() => createLattice(10, [{ site: 3, capturesFrom: "left" }, { site: 3, capturesFrom: "left" }]), /twice/],
    [() => createLattice(10, [{ site: 3, capturesFrom: "left", strength: 1.5 }]), /\[0, 1\]/],
    [() => createLattice(10, { site: [3, 4], capturesFrom: ["left"] }), /1 entries/],
    [() => createWorld(lattice10(), [{ leftPos: 10, rightPos: 10 }]), /outside/],
    [() => createWorld(lattice10(), [{ leftPos: 5, rightPos: 4 }]), /right of the right leg/],
    [() => createWorld(lattice10(), [{ leftPos: 1, rightPos: 5 }, { leftPos: 5, rightPos: 6 }]), /already occupied/],
    [() => createWorld(lattice10(), [{ leftPos: 1, rightPos: 2, leftMobility: "captured" }]), /not a CTCF/],
    [() => createWorld(lattice10(), [{ leftPos: 3, rightPos: 5, leftMobility: "captured" }]), /can't be captured/],
    [() => createWorld(lattice10(), [{ leftPos: 1, rightPos: 2, leftMobility: "evicted" }]), /"moving" or "captured"/],
    // The CTCF at 3 captures from the left: it captures right-facing legs, so it can't evict one.
    [() => createWorld(lattice10(), [{ leftPos: 1, rightPos: 2, leftMobility: "stuck" }]), /"moving" or "captured"/],
    [() => createWorld(lattice10(), { leftPos: [1, 2], rightPos: [3] }), /different lengths/],
    [() => populate(lattice10(), 6), /too many LEFs/],
    [() => populate(lattice10(), -1), /nLefs/],
    [() => populate(lattice10(), 1, { seed: -1 }), /seed/],
    [() => populate(lattice10(), 1, { seed: 1.5 }), /seed/],
    [() => populate(lattice10(), 1, { seed: 2n ** 64n }), /seed/],
    [() => createModel(createWorld(lattice10()), { step: 0 }), /step rate must be positive/],
    [() => createModel(createWorld(lattice10()), { respawn: Number.NaN }), /respawn rate must be positive/],
    [() => createModel(createWorld(lattice10()), { respawnBlocked: 0 }), /respawn_blocked rate must be positive/],
    [() => createModel(createWorld(lattice10()), {}, { spawnRule: "anywhere" as "uniform_non_ctcf" }), /unknown spawn rule/],
    [() => new Sim(makeModel({ nSites: 10 }), { startTime: Number.POSITIVE_INFINITY }), /startTime/],
  ] as [() => unknown, RegExp][])("rejects case %#", (build, message) => {
    expect(build).toThrow(message);
  });

  it("new Sim checks the model too, so a hand-edited one can't slip through", () => {
    const world = createWorld(createLattice(10), [{ leftPos: 1, rightPos: 2 }]);
    world.lefs.leftPos[0] = 5; // now crossed
    const model = { world, rates: { step: 1, respawn: 1, respawnBlocked: 1, respawnCaptured: 1 }, spawnRule: "uniform_non_ctcf" } as const;
    expect(() => new Sim(model)).toThrow(/right of the right leg/);
  });

  it("accepts bigint seeds, and number seeds equal to them", () => {
    const model = makeModel({ ...TADS, seed: 1 });
    using a = new Sim(model, { seed: 0 });
    using b = new Sim(model, { seed: 0n });
    using c = new Sim(model, { seed: 2n ** 64n - 1n });
    a.runUntil(100);
    b.runUntil(100);
    c.runUntil(100);
    expect(snapshot(a)).toEqual(snapshot(b));
    expect(snapshot(a)).not.toEqual(snapshot(c));
  });
});

describe("JSON", () => {
  it("reads and writes the same document as the other bindings", () => {
    const model = modelFromJson(JSON.stringify(modelFixture));
    expect(JSON.parse(modelToJson(model))).toEqual(modelFixture);
    expect(modelFromJson(modelToJson(model, 2))).toEqual(model);
  });

  it("loads mobility-free LEF rows and checks what it loads", () => {
    const doc = structuredClone(modelFixture) as typeof modelFixture;
    doc.lefs = [{ left_pos: 10, right_pos: 12 }] as typeof doc.lefs;
    expect(Array.from(modelFromJson(JSON.stringify(doc)).world.lefs.leftMobility)).toEqual([0]);
    doc.version = 1; // an older format
    expect(() => modelFromJson(JSON.stringify(doc))).toThrow(/unsupported/);
    doc.version = 2;
    doc.rates.step = -1;
    expect(() => modelFromJson(JSON.stringify(doc))).toThrow(/step rate/);
  });
});

describe("running", () => {
  it("reproduces the Python binding's state exactly (cross-language fixture)", () => {
    const { config, expected } = fixture;
    using sim = makeSim({
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
    expect(sim.nEvents).toBe(expected.eventCount);
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
    expect(first.nEvents).toBeGreaterThan(0);
    const again = sim.runUntil(100);
    expect(again).toEqual({ nEvents: 0, time: 100, stopped: "reached_time" });
    expect(sim.runUntil(50).nEvents).toBe(0);
    expect(sim.time).toBe(100);
  });

  it("split runs end in the same state as one run", () => {
    using whole = make(2);
    whole.runUntil(500);
    using split = make(2);
    for (const t of [123.4, 123.4, 300, 500]) split.runUntil(t);
    expect(split.nEvents).toBe(whole.nEvents);
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
      expect(r.nEvents).toBeLessThanOrEqual(97);
      if (r.stopped === "reached_time") break;
      expect(r.stopped).toBe("reached_event_count");
      expect(r.time).toBeLessThan(400);
    }
    expect(calls).toBeGreaterThan(10);
    expect(capped.nEvents).toBe(whole.nEvents);
    expect(snapshot(capped)).toEqual(snapshot(whole));
  });

  it("runEvents fires exactly n events", () => {
    using sim = make();
    expect(sim.runEvents(1234)).toMatchObject({ nEvents: 1234, stopped: "reached_event_count" });
    expect(sim.nEvents).toBe(1234);
  });

  it("an empty sim is exhausted", () => {
    using sim = makeSim({ nSites: 10 });
    expect(sim.nextEventTime).toBeNull();
    expect(sim.runEvents(5)).toEqual({ nEvents: 0, time: 0, stopped: "exhausted" });
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
    const big = makeSim({ nSites: 2_000_000, nLefs: 10 });
    expect(old.leftPos.length).toBe(0); // detached
    expect(snapshot(sim)).toEqual(before);
    big.free();
  });

  it("works with zero LEFs", () => {
    using sim = makeSim({ nSites: 10 });
    expect(sim.state().leftPos.length).toBe(0);
  });
});

describe("state as a model", () => {
  it("world() and model() are copies of the current state", () => {
    using sim = make(1, { nLefs: 4 });
    sim.runUntil(500);
    const world = sim.world();
    const live = sim.state();
    expect(Array.from(world.lefs.leftPos)).toEqual(Array.from(live.leftPos));
    expect(world.lattice).toBe(sim.lattice);
    const model = sim.model();
    expect(model.rates).toEqual({ step: 1, respawn: 0.002, respawnBlocked: 0.02, respawnCaptured: 0.0004 });
    world.lefs.leftPos[0] = 2999; // a copy: changing it leaves the sim alone
    expect(sim.state().leftPos[0]).toBe(live.leftPos[0]);
    expect(sim.state().leftPos[0]).not.toBe(2999);
    // Restarting from it is a valid run from that time.
    using next = new Sim(model, { seed: 9, startTime: 500 });
    expect(next.time).toBe(500);
    expect(next.runUntil(600).nEvents).toBeGreaterThan(0);
  });

  it("remembers the model, seed and start time it started from", () => {
    const model = makeModel({ ...TADS, seed: 1 });
    using sim = new Sim(model, { seed: 7, startTime: 12.5 });
    sim.runUntil(300);
    expect(sim.initialModel()).toEqual(model);
    expect(sim.model()).not.toEqual(model);
    expect([sim.seed, sim.startTime]).toEqual([7n, 12.5]);
    using again = new Sim(sim.initialModel(), { seed: sim.seed, startTime: sim.startTime });
    again.runUntil(300);
    expect(snapshot(again)).toEqual(snapshot(sim));
  });

  it("clone() forks the run exactly", () => {
    using sim = make(5);
    sim.runUntil(300);
    using fork = sim.clone();
    sim.runUntil(900);
    fork.runUntil(900);
    expect(fork.nEvents).toBe(sim.nEvents);
    expect(snapshot(fork)).toEqual(snapshot(sim));
  });

  it("step() fires one event and names it", () => {
    using sim = make(6, { nLefs: 2 });
    const seen = new Set<string>();
    for (let i = 0; i < 3000; i++) {
      const e = sim.step()!;
      expect(e.time).toBe(sim.time);
      expect(e.id).toBeLessThan(e.action === "Respawn" ? 2 : 4); // a lefId, else a legId
      seen.add(e.action);
    }
    expect(seen).toEqual(new Set(["Step", "Respawn", "Evict"]));
    using empty = makeSim({ nSites: 10 });
    expect(empty.step()).toBeNull();
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
