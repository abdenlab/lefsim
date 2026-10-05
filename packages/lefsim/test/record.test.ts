import { describe, expect, it } from "vitest";

import { KEYFRAME, Sim, type SnapshotColumns } from "../src/index.ts";
import { makeSim, type Setup } from "./helpers.ts";

const OPTIONS: Setup = {
  nSites: 200,
  ctcfs: [
    { site: 60, capturesFrom: "right", strength: 0.8 },
    { site: 100, capturesFrom: "right", strength: 0.8 },
    { site: 140, capturesFrom: "left", strength: 0.8 },
  ],
  nLefs: 8,
  rates: { step: 1, respawn: 0.02, respawnStalled: 0.02, respawnCaptured: 0.005 },
};
const make = (seed: number) => makeSim({ ...OPTIONS, seed });

/** Concatenate snapshot columns. */
function concat(parts: SnapshotColumns[]): SnapshotColumns {
  const cat = <T extends Float64Array | Uint32Array | Uint8Array>(key: keyof SnapshotColumns): number[] =>
    parts.flatMap((p) => Array.from(p[key] as T));
  return {
    nLefs: parts[0]!.nLefs,
    time: Float64Array.from(cat("time")),
    leftPos: Uint32Array.from(cat("leftPos")),
    rightPos: Uint32Array.from(cat("rightPos")),
    leftMobility: Uint8Array.from(cat("leftMobility")),
    rightMobility: Uint8Array.from(cat("rightMobility")),
  };
}

const plain = (c: SnapshotColumns) => ({
  nLefs: c.nLefs,
  time: Array.from(c.time),
  leftPos: Array.from(c.leftPos),
  rightPos: Array.from(c.rightPos),
  leftMobility: Array.from(c.leftMobility),
  rightMobility: Array.from(c.rightMobility),
});

describe("recordUntil", () => {
  it("records an inclusive, exact grid", () => {
    using sim = make(1);
    const rec = sim.recordUntil(10, { dt: 0.5 });
    const s = rec.snapshots!;
    expect(rec).toMatchObject({ time: 10, stopped: "reached_time", events: null });
    expect(Array.from(s.time)).toEqual(Array.from({ length: 21 }, (_, k) => 0.5 * k));
    expect(s.nLefs).toBe(8);
    expect(s.leftPos.length).toBe(21 * 8);
    // The last grid point is t = 10: the current state.
    const now = sim.state();
    expect(Array.from(s.leftPos.subarray(20 * 8))).toEqual(Array.from(now.leftPos));
    expect(Array.from(s.rightMobility.subarray(20 * 8))).toEqual(Array.from(now.rightMobility));
  });

  it("continues the grid across calls", () => {
    using whole = make(2);
    const all = whole.recordUntil(120, { dt: 1, events: true });
    using split = make(2);
    const parts = [10.37, 10.37, 55, 120].map((t) => split.recordUntil(t, { dt: 1, events: true }));
    expect(plain(concat(parts.map((p) => p.snapshots!)))).toEqual(plain(all.snapshots!));
    const events = parts.flatMap((p) => Array.from(p.events!.legId));
    expect(events).toEqual(Array.from(all.events!.legId));
  });

  it("does not perturb the run", () => {
    using a = make(3);
    a.recordUntil(200, { dt: 0.3, events: true, keyframes: true });
    using b = make(3);
    b.runUntil(200);
    expect(a.nEvents).toBe(b.nEvents);
    expect(Array.from(a.state().leftPos)).toEqual(Array.from(b.state().leftPos));
  });

  it("opens a keyframed event log with every leg", () => {
    using sim = make(4);
    const ev = sim.recordUntil(100, { events: true, keyframes: true }).events!;
    for (let i = 0; i < sim.nLegs; i++) expect([ev.kind[i], ev.legId[i], ev.time[i]]).toEqual([KEYFRAME, i, 0]);
    expect(ev.kind.subarray(sim.nLegs).includes(KEYFRAME)).toBe(false);
    expect(ev.end).toBe(100);
  });

  it("stops at maxEvents and resumes seamlessly", () => {
    using whole = make(5);
    const all = whole.recordUntil(200, { dt: 0.5 });
    using capped = make(5);
    const parts: SnapshotColumns[] = [];
    for (;;) {
      const r = capped.recordUntil(200, { dt: 0.5, maxEvents: 83 });
      parts.push(r.snapshots!);
      if (r.stopped === "reached_time") break;
      expect(r.stopped).toBe("reached_event_count");
    }
    expect(parts.length).toBeGreaterThan(5);
    expect(plain(concat(parts))).toEqual(plain(all.snapshots!));
  });
});

describe("recordEvents", () => {
  it("fires exactly n events and only records settled grid points", () => {
    using sim = make(6);
    const rec = sim.recordEvents(500, { dt: 0.5, events: true });
    expect(rec.nEvents).toBe(500);
    expect(Array.from(rec.snapshots!.time).every((t) => t <= sim.time)).toBe(true);
    const rest = sim.recordUntil(100, { dt: 0.5 });
    using fresh = make(6);
    const all = fresh.recordUntil(100, { dt: 0.5 });
    expect(plain(concat([rec.snapshots!, rest.snapshots!]))).toEqual(plain(all.snapshots!));
  });
});

describe("recordSnapshotChunk", () => {
  it("tiles into the one-shot recording, whatever stops each chunk", () => {
    using whole = make(7);
    const all = whole.recordUntil(300, { dt: 0.25 });
    using sim = make(7);
    const parts: SnapshotColumns[] = [];
    const reasons = new Set<string>();
    for (;;) {
      const c = sim.recordSnapshotChunk(300, { dt: 0.25, maxSnapshots: 7, maxEvents: 10 });
      expect(c.time.length).toBeLessThanOrEqual(7);
      expect(c.end).toBe(sim.time);
      parts.push(c);
      reasons.add(c.stopped);
      if (c.stopped === "reached_time") break;
    }
    expect(reasons).toEqual(new Set(["recorder_full", "reached_event_count", "reached_time"]));
    expect(plain(concat(parts))).toEqual(plain(all.snapshots!));
  });

  it("mixes with recordUntil on the same grid", () => {
    using whole = make(8);
    const all = whole.recordUntil(200, { dt: 0.5 });
    using sim = make(8);
    const parts = [sim.recordUntil(50, { dt: 0.5 }).snapshots!];
    for (let c = sim.recordSnapshotChunk(150, { dt: 0.5, maxSnapshots: 9 }); ; c = sim.recordSnapshotChunk(150, { dt: 0.5, maxSnapshots: 9 })) {
      parts.push(c);
      if (c.stopped === "reached_time") break;
    }
    parts.push(sim.recordUntil(200, { dt: 0.5 }).snapshots!);
    expect(plain(concat(parts))).toEqual(plain(all.snapshots!));
  });
});

describe("validation", () => {
  it.each([
    ["dt = 0", (s: Sim) => s.recordUntil(10, { dt: 0 }), /dt must be positive/],
    ["t0 = NaN", (s: Sim) => s.recordUntil(10, { dt: 1, t0: Number.NaN }), /t0 must be finite/],
    ["keyframes without events", (s: Sim) => s.recordUntil(10, { keyframes: true }), /keyframes require events/],
    ["maxSnapshots = 0", (s: Sim) => s.recordSnapshotChunk(10, { dt: 1, maxSnapshots: 0 }), /maxSnapshots/],
    ["NaN tEnd", (s: Sim) => s.recordUntil(Number.NaN), /NaN/],
  ])("rejects %s", (_, call, message) => {
    using sim = make(9);
    expect(() => call(sim)).toThrow(message);
    expect(sim.time).toBe(0);
  });
});
