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
  tableFromIPC,
  tableToIPC,
  Uint32,
  Uint8,
  Utf8,
  vectorFromArray,
  type DataType,
} from "apache-arrow";
import { describe, expect, it } from "vitest";

import { deltaChunkFromArrow, eventsToArrow, latticeToArrow, lefsToArrow, snapshotsToArrow } from "../src/arrow.ts";
import { KEYFRAME, Replayer, Sim, type SimOptions } from "../src/index.ts";
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
const SMALL: SimOptions = {
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
  using sim = new Sim(SMALL);
  const lattice = latticeToArrow(sim.meta());
  const lefs = lefsToArrow(sim.state());
  const built = {
    snapshots_wide: snapshotsToArrow(sim.recordUntil(10, { dt: 1 }).snapshots!),
    snapshots_long: snapshotsToArrow(sim.recordUntil(20, { dt: 1 }).snapshots!, { layout: "long" }),
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
    using sim = new Sim({ ...SMALL, seed: 2 });
    const s = sim.recordUntil(50, { dt: 1 }).snapshots!;
    const b = snapshotsToArrow(s);
    const left = b.getChild("left_pos")!.data[0]!.values as Uint32Array;
    expect(left.buffer).toBe(s.leftPos.buffer);
    expect(b.numRows).toBe(51 * 4);
    expect(b.getChild("left_mobility")!.get(0)).toBe("Moving");
  });

  it("long is wide unpivoted", () => {
    using sim = new Sim({ ...SMALL, seed: 3 });
    const s = sim.recordUntil(80, { dt: 2 }).snapshots!;
    const wide = snapshotsToArrow(s);
    const long = snapshotsToArrow(s, { layout: "long" });
    expect(long.numRows).toBe(2 * wide.numRows);
    for (let row = 0; row < wide.numRows; row++) {
      const l = 2 * row;
      expect(long.getChild("time")!.get(l)).toBe(wide.getChild("time")!.get(row));
      expect(long.getChild("lef_id")!.get(l)).toBe(wide.getChild("lef_id")!.get(row));
      expect(long.getChild("pos")!.get(l)).toBe(wide.getChild("left_pos")!.get(row));
      expect(long.getChild("pos")!.get(l + 1)).toBe(wide.getChild("right_pos")!.get(row));
      expect(long.getChild("facing")!.get(l + 1)).toBe("Right");
      expect(long.getChild("mobility")!.get(l + 1)).toBe(wide.getChild("right_mobility")!.get(row));
    }
  });

  it("survive an Arrow IPC round trip", () => {
    using sim = new Sim({ ...SMALL, seed: 4 });
    const b = eventsToArrow(sim.recordUntil(100, { events: true, keyframes: true }).events!);
    const back = tableFromIPC(tableToIPC(new Table(b)));
    expect(back.schema.metadata.get("lefsim.kind")).toBe("events");
    expect(back.getChild("action")!.toArray()).toEqual(b.getChild("action")!.toArray());
    expect(Array.from(back.getChild("leg_id")!)).toEqual(Array.from(b.getChild("leg_id")!));
  });
});

describe("deltaChunkFromArrow", () => {
  it("inverts eventsToArrow without copying", () => {
    using sim = new Sim({ ...SMALL, seed: 5 });
    const ev = sim.recordUntil(100, { events: true, keyframes: true }).events!;
    const chunk = deltaChunkFromArrow(eventsToArrow(ev), { end: ev.end });
    expect(chunk.legId.buffer).toBe(ev.legId.buffer);
    expect(chunk.kind.buffer).toBe(ev.kind.buffer);
    expect(Array.from(chunk.mobility)).toEqual(Array.from(ev.mobility));
    expect(chunk.end).toBe(ev.end);
  });

  it("defaults end to the last row's time", () => {
    using sim = new Sim({ ...SMALL, seed: 5 });
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
  it("replays in the TS Replayer to the wasm Sim's states", async () => {
    const bytes = readFileSync(new URL("./fixtures/python_events.arrows", import.meta.url));
    const reader = RecordBatchReader.from(bytes);
    const { config } = ipcInfo;
    const nLefs = config.nLefs;
    const replayer = new Replayer(nLefs);
    let batches = 0;
    for (const batch of reader) {
      const chunk = deltaChunkFromArrow(batch);
      for (let i = 0; i < 2 * nLefs; i++) expect(chunk.kind[i]).toBe(KEYFRAME);
      replayer.push(chunk);
      batches++;
    }
    expect(batches).toBe(ipcInfo.batches);

    using sim = new Sim({
      nSites: config.nSites,
      ctcfs: { site: config.ctcfSites, capturesFrom: config.ctcfCapturesFrom as ("left" | "right")[], strength: config.ctcfStrength },
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
  });
});
