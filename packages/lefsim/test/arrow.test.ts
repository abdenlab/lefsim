import { readFileSync } from "node:fs";

import {
  Dictionary,
  Field,
  Float64,
  makeData,
  RecordBatch,
  RecordBatchReader,
  Schema,
  Struct,
  Table,
  tableFromArrays,
  tableFromIPC,
  tableToIPC,
  Uint32,
  Uint8,
  Utf8,
  vectorFromArray,
  type DataType,
} from "apache-arrow";
import { describe, expect, it } from "vitest";

import { deltaChunkFromArrow, eventsToArrow, latticeToArrow, lefsToArrow, replayerFromArrow, snapshotsToArrow } from "../src/arrow.ts";
import { createLattice, KEYFRAME, Replayer, ROW_KIND, type Side } from "../src/index.ts";
import { makeSim, type Setup } from "./helpers.ts";
import schemas from "./fixtures/schemas.json";
import ipcInfo from "./fixtures/python_events.json";

// ------------------------------------------------------------------ helpers

const NEUTRAL: Record<string, string> = { Float64: "float64", Uint32: "uint32", Uint8: "uint8", Utf8: "utf8" };

function neutralType(t: DataType): string {
  if (t instanceof Dictionary) return `dict<${neutralType(t.indices)},${neutralType(t.dictionary)}>`;
  const name = NEUTRAL[String(t)];
  if (!name) throw new Error(`unexpected type ${String(t)}`);
  return name;
}

/** The same neutral description the Python fixture script writes. */
function describe_(batch: RecordBatch) {
  return {
    metadata: Object.fromEntries(batch.schema.metadata),
    fields: batch.schema.fields.map((f) => ({
      name: f.name,
      type: neutralType(f.type),
      nullable: f.nullable,
      dictionary: f.type instanceof Dictionary ? (batch.getChild(f.name)!.data[0]!.dictionary!.toArray() as string[]) : null,
    })),
  };
}

// The model the schema fixture was generated from (see make_fixtures.py).
const SMALL: Setup = {
  nSites: 200,
  ctcfs: [
    { site: 60, capturesFrom: "right" },
    { site: 140, capturesFrom: "left" },
  ],
  nLefs: 4,
  seed: 1,
};

// ------------------------------------------------------------------ tests

describe("schema parity with the Python binding", () => {
  using sim = makeSim(SMALL);
  const lattice = latticeToArrow(sim.lattice);
  const lefs = lefsToArrow(sim.state());
  const built = {
    snapshots_lef: snapshotsToArrow(sim.recordUntil(10, { dt: 1 }).snapshots!),
    snapshots_leg: snapshotsToArrow(sim.recordUntil(20, { dt: 1 }).snapshots!, { layout: "leg" }),
    events: eventsToArrow(sim.recordUntil(30, { events: true, keyframes: true }).events!),
    lattice,
    lefs,
  } as Record<keyof typeof schemas, RecordBatch>;

  it.each(Object.keys(schemas) as (keyof typeof schemas)[])("%s", (name) => {
    expect(describe_(built[name])).toEqual(schemas[name]);
  });
});

describe("builders", () => {
  it("wrap the simulator's arrays without copying", () => {
    using sim = makeSim({ ...SMALL, seed: 2 });
    const s = sim.recordUntil(50, { dt: 1 }).snapshots!;
    const b = snapshotsToArrow(s);
    const left = b.getChild("left_pos")!.data[0]!.values as Uint32Array;
    expect(left.buffer).toBe(s.leftPos.buffer);
    expect(b.numRows).toBe(51 * 4);
    expect(b.getChild("left_mobility")!.get(0)).toBe("Moving");
  });

  it("the leg layout is the lef layout unpivoted", () => {
    using sim = makeSim({ ...SMALL, seed: 3 });
    const s = sim.recordUntil(80, { dt: 2 }).snapshots!;
    const wide = snapshotsToArrow(s);
    const long = snapshotsToArrow(s, { layout: "leg" });
    expect(long.numRows).toBe(2 * wide.numRows);
    for (let row = 0; row < wide.numRows; row++) {
      const l = 2 * row;
      expect(long.getChild("time")!.get(l)).toBe(wide.getChild("time")!.get(row));
      expect(long.getChild("lef_id")!.get(l)).toBe(wide.getChild("lef_id")!.get(row));
      expect(long.getChild("pos")!.get(l)).toBe(wide.getChild("left_pos")!.get(row));
      expect(long.getChild("pos")!.get(l + 1)).toBe(wide.getChild("right_pos")!.get(row));
      expect(long.getChild("leg_id")!.get(l + 1)).toBe(2 * wide.getChild("lef_id")!.get(row)! + 1);
      expect(long.getChild("mobility")!.get(l + 1)).toBe(wide.getChild("right_mobility")!.get(row));
    }
  });

  it("survive an Arrow IPC round trip", () => {
    using sim = makeSim({ ...SMALL, seed: 4 });
    const b = eventsToArrow(sim.recordUntil(100, { events: true, keyframes: true }).events!);
    const back = tableFromIPC(tableToIPC(new Table(b)));
    expect(back.schema.metadata.get("lefsim.kind")).toBe("events");
    expect(back.getChild("action")!.toArray()).toEqual(b.getChild("action")!.toArray());
    expect(Array.from(back.getChild("leg_id")!)).toEqual(Array.from(b.getChild("leg_id")!));
  });
});

describe("deltaChunkFromArrow", () => {
  it("inverts eventsToArrow without copying", () => {
    using sim = makeSim({ ...SMALL, seed: 5 });
    const ev = sim.recordUntil(100, { events: true, keyframes: true }).events!;
    const chunk = deltaChunkFromArrow(eventsToArrow(ev), { end: ev.end });
    expect(chunk.legId.buffer).toBe(ev.legId.buffer);
    expect(chunk.kind.buffer).toBe(ev.kind.buffer);
    expect(Array.from(chunk.mobility)).toEqual(Array.from(ev.mobility));
    expect(chunk.end).toBe(ev.end);
  });

  it("defaults end to the last row's time", () => {
    using sim = makeSim({ ...SMALL, seed: 5 });
    const ev = sim.recordUntil(100, { events: true }).events!;
    expect(deltaChunkFromArrow(eventsToArrow(ev)).end).toBe(ev.time[ev.time.length - 1]);
  });

  it("maps re-encoded dictionaries by name", () => {
    // mobility dictionary in a different order: [Captured, Moving, Evicted]
    const mobType = new Dictionary(new Utf8(), new Uint8());
    const actType = new Dictionary(new Utf8(), new Uint8());
    const columns = [
      ["time", makeData({ type: new Float64(), data: new Float64Array([1, 2]) })],
      ["leg_id", makeData({ type: new Uint32(), data: new Uint32Array([0, 1]) })],
      ["pos", makeData({ type: new Uint32(), data: new Uint32Array([5, 6]) })],
      ["mobility", makeData({ type: mobType, data: new Uint8Array([0, 1]), dictionary: vectorFromArray(["Captured", "Moving", "Evicted"], new Utf8()) })],
      ["action", makeData({ type: actType, data: new Uint8Array([0, 0]), dictionary: vectorFromArray(["Respawn"], new Utf8()) })],
    ] as const;
    const fields = columns.map(([n, d]) => new Field(n, d.type as DataType, false));
    const batch = new RecordBatch(
      new Schema(fields),
      makeData({ type: new Struct(fields), length: 2, nullCount: 0, children: columns.map(([, d]) => d) }),
    );
    const chunk = deltaChunkFromArrow(batch);
    expect(Array.from(chunk.mobility)).toEqual([1, 0]); // Captured, Moving
    expect(Array.from(chunk.kind)).toEqual([1, 1]); // Respawn
  });
});

describe("Arrow IPC from the Python binding", () => {
  const { config } = ipcInfo;
  const nLefs = config.nLefs;
  const lattice = createLattice(config.nSites, {
    site: config.ctcfSites,
    capturesFrom: config.ctcfCapturesFrom as Side[],
    strength: config.ctcfStrength,
  });
  const batches = () => [...RecordBatchReader.from(readFileSync(new URL("./fixtures/python_events.arrows", import.meta.url)))];

  /** Replay against a live wasm Sim of the same run at many times. */
  function expectMatchesLiveRun(replayer: Replayer) {
    using sim = makeSim({
      nSites: config.nSites,
      ctcfs: { site: config.ctcfSites, capturesFrom: config.ctcfCapturesFrom as Side[], strength: config.ctcfStrength },
      nLefs,
      seed: config.seed,
      rates: config.rates,
    });
    let checked = 0;
    for (let t = 0; t <= replayer.end; t += 1.7) {
      sim.runUntil(t);
      replayer.advanceTo(t);
      const [a, b] = [sim.state(), replayer.state()];
      expect(Array.from(b.leftPos)).toEqual(Array.from(a.leftPos));
      expect(Array.from(b.rightPos)).toEqual(Array.from(a.rightPos));
      expect(Array.from(b.leftMobility)).toEqual(Array.from(a.leftMobility));
      expect(Array.from(b.rightMobility)).toEqual(Array.from(a.rightMobility));
      checked++;
    }
    expect(checked).toBeGreaterThan(150);
  }

  it("replays in the TS Replayer to the wasm Sim's states", () => {
    const replayer = new Replayer(lattice, nLefs);
    for (const batch of batches()) {
      const chunk = deltaChunkFromArrow(batch);
      for (let i = 0; i < 2 * nLefs; i++) expect(chunk.kind[i]).toBe(KEYFRAME);
      replayer.push(chunk);
    }
    expect(batches().length).toBe(ipcInfo.batches);
    expectMatchesLiveRun(replayer);
  });

  it("replayerFromArrow loads a log whatever its batch boundaries", () => {
    // Re-slice into 37-row batches, which cut keyframes in half.
    const table = new Table(batches());
    const sliced: RecordBatch[] = [];
    for (let at = 0; at < table.numRows; at += 37) sliced.push(...table.slice(at, at + 37).batches);
    const replayer = replayerFromArrow(sliced, lattice, { end: config.tEnd });
    expect([replayer.nLefs, replayer.start, replayer.end]).toEqual([nLefs, 0, config.tEnd]);
    expectMatchesLiveRun(replayer);
    // A table works too, and end defaults to the last row.
    expect(replayerFromArrow(table, lattice).end).toBeLessThan(config.tEnd);
  });

  it("replayerFromArrow reads names in any case, or codes", () => {
    const table = new Table(batches());
    const time = table.getChild("time")!.toArray() as Float64Array;
    const plain = tableFromArrays({
      time,
      leg_id: table.getChild("leg_id")!.toArray() as Uint32Array,
      pos: table.getChild("pos")!.toArray() as Uint32Array,
      mobility: Array.from(table.getChild("mobility")!, (v) => String(v).toUpperCase()),
      action: Uint8Array.from(table.getChild("action")!, (v) => ROW_KIND.indexOf(v as (typeof ROW_KIND)[number])),
    });
    const replayer = replayerFromArrow(plain, lattice, { end: config.tEnd });
    expectMatchesLiveRun(replayer);
  });

  it("replayerFromArrow needs a keyframe to size itself", () => {
    using sim = makeSim({ nSites: 100, nLefs: 2, seed: 1 });
    const log = eventsToArrow(sim.recordUntil(50, { events: true }).events!);
    expect(() => replayerFromArrow(log, lattice)).toThrow(/no keyframe/);
  });
});
