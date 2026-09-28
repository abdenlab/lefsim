import { describe, expect, it } from "vitest";

import { createLattice, createModel, KEYFRAME, MOBILITY, Replayer, Sim, type DeltaChunk, type LiveState } from "../src/index.ts";
import { makeSim, type Setup } from "./helpers.ts";

const EVICTED = MOBILITY.indexOf("Evicted");

const OPTIONS: Setup = {
  nSites: 200,
  ctcfs: [
    { site: 60, capturesFrom: "right", strength: 0.9 },
    { site: 100, capturesFrom: "right", strength: 0.9 },
    { site: 140, capturesFrom: "left", strength: 0.9 },
  ],
  nLefs: 8,
  rates: { step: 1, respawn: 0.02, respawnCaptured: 0.005 },
};

const LATTICE = createLattice(OPTIONS.nSites, OPTIONS.ctcfs);

const make = (seed: number) => makeSim({ ...OPTIONS, seed });

/** Record a run to `tEnd` as keyframed chunks. */
function record(seed: number, tEnd: number, maxRows: number): DeltaChunk[] {
  using sim = make(seed);
  const chunks: DeltaChunk[] = [];
  for (;;) {
    const chunk = sim.recordEventChunk(tEnd, { maxRows });
    chunks.push(chunk);
    if (chunk.stopped !== "recorder_full") return chunks;
  }
}

const copyTyped = (s: LiveState) => ({
  leftPos: s.leftPos.slice(),
  rightPos: s.rightPos.slice(),
  leftMobility: s.leftMobility.slice(),
  rightMobility: s.rightMobility.slice(),
});

const copy = (s: LiveState) => ({
  leftPos: Array.from(s.leftPos),
  rightPos: Array.from(s.rightPos),
  leftMobility: Array.from(s.leftMobility),
  rightMobility: Array.from(s.rightMobility),
});

/** Reference states from a live Sim at increasing times. */
function reference(seed: number, times: number[]) {
  using sim = make(seed);
  return new Map(times.map((t) => (sim.runUntil(t), [t, copy(sim.state())] as const)));
}

const grid = (from: number, to: number, step: number) =>
  Array.from({ length: Math.floor((to - from) / step) + 1 }, (_, i) => from + i * step);

describe("recordEventChunk", () => {
  it("opens each chunk with a keyframe and tiles the run", () => {
    using sim = make(1);
    const nLegs = sim.nLegs;
    let prevEnd = 0;
    const chunks: ReturnType<Sim["recordEventChunk"]>[] = [];
    for (;;) {
      const c = sim.recordEventChunk(300, { maxRows: 50 });
      chunks.push(c);
      const t0 = c.time[0]!;
      expect(t0).toBe(prevEnd); // each chunk starts where the last one ended
      for (let i = 0; i < nLegs; i++) {
        expect([c.kind[i], c.legId[i], c.time[i]]).toEqual([KEYFRAME, i, t0]);
      }
      expect(c.kind.subarray(nLegs).includes(KEYFRAME)).toBe(false);
      expect(c.time.length).toBeLessThanOrEqual(52); // soft only by a zero-delay chain
      expect(c.end).toBe(sim.time);
      prevEnd = c.end;
      if (c.stopped !== "recorder_full") break;
    }
    expect(chunks.length).toBeGreaterThan(10);
    expect(chunks.at(-1)!.stopped).toBe("reached_time");
    expect(sim.time).toBe(300);
  });

  it("without keyframes records plain events", () => {
    using sim = make(1);
    const c = sim.recordEventChunk(50, { keyframes: false });
    expect(c.kind.includes(KEYFRAME)).toBe(false);
    expect(c.stopped).toBe("reached_time");
  });

  it("rejects bad sizes and times", () => {
    using sim = make(1);
    expect(() => sim.recordEventChunk(10, { maxRows: 17 })).toThrow(/at least 18 with keyframes/);
    expect(() => sim.recordEventChunk(10, { maxRows: 1, keyframes: false })).toThrow(/at least 2/);
    expect(() => sim.recordEventChunk(10, { maxRows: 2 ** 31 })).toThrow(/at most/);
    expect(() => sim.recordEventChunk(Number.NaN)).toThrow(/NaN/);
    expect(sim.time).toBe(0); // nothing ran
  });
});

describe("Replayer", () => {
  const T = 600;
  const chunks = record(2, T, 120);
  const times = grid(0, T, 3.7);
  const ref = reference(2, times);

  it("reproduces the live sim at every time, advancing forward", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    expect(r.start).toBe(0);
    expect(r.end).toBe(T);
    for (const t of times) {
      expect(r.advanceTo(t)).toBe(t);
      expect(copy(r.state())).toEqual(ref.get(t));
    }
  });

  it("jumps forward any distance, including onto keyframe times", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    const starts = chunks.map((c) => c.time[0]!);
    const jumps = [0.5, 0.9, starts[3]!, starts[3]! + 0.1, starts[9]!, 250, starts.at(-2)!, T].sort((a, b) => a - b);
    const expected = reference(2, jumps);
    for (const t of jumps) {
      expect(r.advanceTo(t)).toBe(t);
      expect(copy(r.state())).toEqual(expected.get(t));
    }
  });

  it("seeks to any time in any order", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    // A fixed pseudo-random order, so the test is deterministic.
    const order = [...times].sort((a, b) => ((a * 7919) % 101) - ((b * 7919) % 101));
    for (const t of order) {
      r.seek(t);
      expect(r.time).toBe(t);
      expect(copy(r.state())).toEqual(ref.get(t));
    }
    // advanceTo backwards seeks too.
    r.advanceTo(T);
    expect(r.advanceTo(10)).toBe(10);
    expect(copy(r.state())).toEqual(reference(2, [10]).get(10));
  });

  it("snapshots(dt) decodes to the grid a recording would have taken", () => {
    using sim = make(2);
    const rec = sim.recordUntil(T, { dt: 0.37 }).snapshots!;
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    expect(r.snapshots(0.37)).toEqual(rec);

    // In pieces: each call continues the grid, whether or not it ends on a point.
    const pieces = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) pieces.push(c);
    const parts = [0, 0.37 * 40, 100, 100, 222.2, T].map((tEnd) => pieces.snapshots(0.37, { tEnd }));
    const concat = (key: "time" | "leftPos" | "rightMobility") =>
      parts.flatMap((p) => Array.from(p[key] as ArrayLike<number>));
    for (const key of ["time", "leftPos", "rightMobility"] as const) {
      expect(concat(key)).toEqual(Array.from(rec[key] as ArrayLike<number>));
    }

    // tStart seeks, starting the grid afresh from there.
    const again = r.snapshots(0.37, { tStart: 0.5, tEnd: 1 });
    expect(Array.from(again.time)).toEqual([0.74]);
    expect(() => r.snapshots(0)).toThrow(/dt/);
  });

  it("world() is the state at its time, ready to branch a new run from", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    const world = r.worldAt(250);
    expect(world.lattice).toBe(LATTICE);
    expect({ time: 250, ...world.lefs }).toEqual({ time: 250, ...copyTyped(r.state()) });
    using branch = new Sim(createModel(world, OPTIONS.rates), { seed: 99, startTime: r.time });
    expect(branch.time).toBe(250);
    expect(branch.runUntil(300).nEvents).toBeGreaterThan(0);
  });

  it("can join late, from any chunk", () => {
    for (const k of [1, Math.floor(chunks.length / 2), chunks.length - 1]) {
      const r = new Replayer(LATTICE, OPTIONS.nLefs!);
      for (const c of chunks.slice(k)) r.push(c);
      const start = chunks[k]!.time[0]!;
      expect(r.start).toBe(start);
      for (const t of times.filter((t) => t >= start)) {
        r.advanceTo(t);
        expect(copy(r.state())).toEqual(ref.get(t));
      }
      // Once playing, going back before the data stops at its start.
      expect(r.advanceTo(start - 1)).toBe(start);
      expect(copy(r.state())).toEqual(reference(2, [start]).get(start));
    }
  });

  it("stops at the end of the data (buffering) and resumes when more arrives", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    const half = Math.floor(chunks.length / 2);
    for (const c of chunks.slice(0, half)) r.push(c);
    const end = r.end;
    expect(end).toBeLessThan(T);
    expect(r.advanceTo(T)).toBe(end);
    expect(r.time).toBe(end);
    for (const c of chunks.slice(half)) r.push(c);
    expect(r.advanceTo(T)).toBe(T);
    expect(copy(r.state())).toEqual(reference(2, [T]).get(T));
  });

  it("is not ready until a keyframe is applied, and skips rows before one", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    expect(r.ready).toBe(false);
    expect(() => r.state()).toThrow(/no keyframe/);
    // A chunk with its keyframe stripped can't be decoded on its own...
    const c = chunks[0]!;
    const n = 2 * OPTIONS.nLefs!;
    r.push({
      time: c.time.subarray(n),
      legId: c.legId.subarray(n),
      pos: c.pos.subarray(n),
      mobility: c.mobility.subarray(n),
      kind: c.kind.subarray(n),
      end: c.end,
    });
    expect(r.start).toBeNull();
    expect(r.advanceTo(c.end)).toBe(-Infinity);
    expect(r.ready).toBe(false);
    // ...but the next keyframed chunk makes it ready.
    r.push(chunks[1]!);
    const t = chunks[1]!.end;
    r.advanceTo(t);
    expect(r.ready).toBe(true);
    expect(copy(r.state())).toEqual(reference(2, [t]).get(t));
  });

  it("trimBefore drops old chunks but keeps later seeks working", () => {
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    for (const c of chunks) r.push(c);
    r.advanceTo(T);
    const cut = 300;
    r.trimBefore(cut);
    expect(r.start!).toBeGreaterThan(0);
    expect(r.start!).toBeLessThanOrEqual(cut);
    for (const t of times.filter((t) => t >= cut)) {
      r.seek(t);
      expect(copy(r.state())).toEqual(ref.get(t));
    }
    expect(() => r.seek(0)).toThrow(/no keyframe/);
  });

  it("never shows an Evicted leg in a keyframe", () => {
    const strong = { ...OPTIONS, ctcfs: (OPTIONS.ctcfs as { site: number; capturesFrom: "left" | "right" }[]).map((c) => ({ ...c, strength: 1 })) };
    using sim = makeSim({ ...strong, seed: 3 });
    let evictions = 0;
    for (;;) {
      const c = sim.recordEventChunk(3000, { maxRows: 2 * sim.nLefs + 4 });
      const n = 2 * sim.nLefs;
      for (let i = 0; i < c.kind.length; i++) {
        if (c.mobility[i] === EVICTED) {
          expect(i).toBeGreaterThanOrEqual(n); // never inside the keyframe
          evictions++;
        }
      }
      if (c.stopped !== "recorder_full") break;
    }
    expect(evictions).toBeGreaterThan(10);
  });

  it("checks every row, as the core's replayer does, and rejects a bad chunk whole", () => {
    const first = chunks[0]!;
    const edit = (change: (c: DeltaChunk) => void): DeltaChunk => {
      const c = {
        ...first,
        time: first.time.slice(),
        legId: first.legId.slice(),
        pos: first.pos.slice(),
        mobility: first.mobility.slice(),
        kind: first.kind.slice(),
      };
      change(c);
      return c;
    };
    const r = new Replayer(LATTICE, OPTIONS.nLefs!);
    const cases: [DeltaChunk, RegExp][] = [
      [{ ...first, legId: first.legId.subarray(1) }, /equal length/],
      [edit((c) => (c.legId[20] = 16)), /leg id 16 is out of range for 16 legs/],
      [edit((c) => (c.mobility[20] = 7)), /invalid mobility code 7/],
      [edit((c) => (c.kind[20] = 9)), /invalid row kind code 9/],
      [edit((c) => c.legId.set([1, 0], 0)), /keyframe at t=0 has 0 rows; expected 16, one per leg in order/],
      [edit((c) => (c.end = c.time.at(-1)! - 1)), /chunk end .* is before its last row/],
      [edit((c) => (c.time[17] = c.time[16]! - 1)), /comes after a row/],
    ];
    for (const [chunk, message] of cases) expect(() => r.push(chunk)).toThrow(message);
    expect(r.start).toBeNull(); // nothing was pushed

    // A keyframe for 8 LEFs doesn't fit a 2-LEF replayer.
    expect(() => new Replayer(LATTICE, 2).push(first)).toThrow(/out of range for 4 legs/);
    // Chunks must come in order.
    r.push(chunks[1]!);
    expect(() => r.push(first)).toThrow(/comes after a row/);
  });
});
