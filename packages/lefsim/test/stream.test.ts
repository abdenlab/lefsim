import { RecordBatch } from "apache-arrow";
import { describe, expect, it } from "vitest";

import { toRecordBatches } from "../src/arrow.ts";
import {
  iterate,
  KEYFRAME,
  Replayer,
  Sim,
  type EventChunk,
  type SnapshotChunk,
} from "../src/index.ts";
import { makeSim, type Setup } from "./helpers.ts";

const OPTIONS: Setup = {
  nSites: 200,
  ctcfs: [
    { site: 60, capturesFrom: "right", strength: 0.8 },
    { site: 100, capturesFrom: "right", strength: 0.8 },
    { site: 140, capturesFrom: "left", strength: 0.8 },
  ],
  nLefs: 8,
  rates: { step: 1, respawn: 0.02, respawnStalled: 0.1, respawnEvicted: 0.5, respawnCaptured: 0.005 },
};
const make = (seed: number) => makeSim({ ...OPTIONS, seed });

async function collect<T>(stream: ReadableStream<T>): Promise<T[]> {
  const out: T[] = [];
  for await (const x of iterate(stream)) out.push(x);
  return out;
}

const flat = (chunks: SnapshotChunk[], key: "time" | "leftPos" | "rightMobility") =>
  chunks.flatMap((c) => Array.from(c[key]));

describe("Sim.stream", () => {
  it("streams snapshots that tile into the one-shot recording", async () => {
    using whole = make(1);
    const all = whole.recordUntil(300, { dt: 0.5 }).snapshots!;
    using sim = make(1);
    const chunks = (await collect(sim.stream(300, { dt: 0.5, maxSnapshots: 40 }))) as SnapshotChunk[];
    expect(chunks.length).toBeGreaterThan(10);
    expect(chunks.every((c) => c.time.length <= 40)).toBe(true);
    expect(flat(chunks, "time")).toEqual(Array.from(all.time));
    expect(flat(chunks, "leftPos")).toEqual(Array.from(all.leftPos));
    expect(flat(chunks, "rightMobility")).toEqual(Array.from(all.rightMobility));
    expect(sim.time).toBe(300);
    sim.runUntil(310); // released after finishing
  });

  it("survives an event cap that runs out between snapshots", async () => {
    using whole = make(2);
    const all = whole.recordUntil(400, { dt: 25 }).snapshots!;
    using sim = make(2);
    const chunks = (await collect(sim.stream(400, { dt: 25, maxSnapshots: 3, maxEvents: 20 }))) as SnapshotChunk[];
    expect(flat(chunks, "time")).toEqual(Array.from(all.time));
    expect(flat(chunks, "leftPos")).toEqual(Array.from(all.leftPos));
  });

  it("streams keyframed events that replay to the live state", async () => {
    using sim = make(3);
    const chunks = (await collect(sim.stream(250, { events: true, maxRows: 100 }))) as EventChunk[];
    const replayer = new Replayer(sim.lattice, sim.nLefs);
    for (const c of chunks) {
      for (let i = 0; i < sim.nLegs; i++) expect(c.kind[i]).toBe(KEYFRAME);
      replayer.push(c);
    }
    replayer.advanceTo(250);
    const [a, b] = [sim.state(), replayer.state()];
    expect(Array.from(b.leftPos)).toEqual(Array.from(a.leftPos));
    expect(Array.from(b.rightMobility)).toEqual(Array.from(a.rightMobility));
  });

  it("computes chunks only when read (no read-ahead)", async () => {
    using sim = make(4);
    const reader = sim.stream(500, { dt: 1, maxSnapshots: 10 }).getReader();
    expect(sim.time).toBe(0); // nothing computed before the first read
    const first = (await reader.read()).value as SnapshotChunk;
    expect(sim.time).toBe(first.end);
    await new Promise((r) => setTimeout(r, 20));
    expect(sim.time).toBe(first.end); // still nothing until the next read
    const second = (await reader.read()).value as SnapshotChunk;
    expect(sim.time).toBe(second.end);
    await reader.cancel();
  });

  it("blocks driving calls while open, but not reads", async () => {
    using sim = make(5);
    const stream = sim.stream(100, { dt: 1, maxSnapshots: 5 });
    expect(() => sim.runUntil(10)).toThrow(/being streamed/);
    expect(() => sim.recordUntil(10, { dt: 1 })).toThrow(/being streamed/);
    expect(() => sim.stream(10, { dt: 1 })).toThrow(/being streamed/);
    expect(sim.time).toBe(0);
    expect(sim.state().leftPos.length).toBe(8);
    expect(sim.nSites).toBe(200);
    await stream.cancel();
    expect(sim.runUntil(10).time).toBe(10); // cancelling releases it
  });

  it("breaking out of iterate() cancels the stream", async () => {
    using sim = make(6);
    for await (const chunk of iterate(sim.stream(1e6, { dt: 1, maxSnapshots: 10 }))) {
      expect(chunk.time.length).toBe(10);
      break;
    }
    expect(() => sim.runUntil(sim.time + 1)).not.toThrow();
  });

  it("errors, clearly, if the Sim is freed mid-stream", async () => {
    const sim = make(7);
    const reader = sim.stream(1e6, { dt: 1, maxSnapshots: 10 }).getReader();
    await reader.read();
    sim.free();
    await expect(reader.read()).rejects.toThrow(/freed/);
  });

  it.each([
    [{}, /exactly one/],
    [{ dt: 1, events: true }, /exactly one/],
    [{ events: true, maxRows: 17 }, /at least 18 with keyframes/],
    [{ events: true, keyframes: false, maxRows: 1 }, /at least 2/],
    [{ dt: 1, maxSnapshots: 0 }, /positive/],
  ])("rejects options %o", (options, message) => {
    using sim = make(8);
    expect(() => sim.stream(10, options as never)).toThrow(message);
    sim.runUntil(1); // a rejected stream leaves the Sim usable
  });
});

describe("toRecordBatches", () => {
  it("turns snapshot and event streams into Arrow record batches", async () => {
    using a = make(9);
    const snaps = await collect(a.stream(100, { dt: 1, maxSnapshots: 30 }).pipeThrough(toRecordBatches({ layout: "leg" })));
    expect(snaps.every((b) => b instanceof RecordBatch)).toBe(true);
    expect(snaps.reduce((n, b) => n + b.numRows, 0)).toBe(101 * 16);
    expect(snaps[0]!.schema.metadata.get("lefsim.kind")).toBe("legs");

    using b = make(9);
    const events = await collect(b.stream(100, { events: true, maxRows: 200 }).pipeThrough(toRecordBatches()));
    expect(events[0]!.schema.metadata.get("lefsim.kind")).toBe("events");
    expect(events[0]!.getChild("action")!.get(0)).toBe("Keyframe");
  });
});
