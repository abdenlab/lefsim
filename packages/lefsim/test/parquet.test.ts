import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { asyncBufferFromUrl } from "hyparquet";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { createLattice, type Side, type World } from "../src/index.ts";
import { replayerFromParquet } from "../src/parquet.ts";
import ipcInfo from "./fixtures/python_events.json";
import { makeSim } from "./helpers.ts";

const { config } = ipcInfo;
const keyframed = readFileSync(new URL("./fixtures/python_events.parquet", import.meta.url));
const plain = readFileSync(new URL("./fixtures/python_events_plain.parquet", import.meta.url));
const ctcfs = { site: config.ctcfSites, capturesFrom: config.ctcfCapturesFrom as Side[], strength: config.ctcfStrength };
const lattice = createLattice(config.nSites, ctcfs);

/** The live run the fixtures recorded. */
const liveSim = () => makeSim({ nSites: config.nSites, ctcfs, nLefs: config.nLefs, seed: config.seed, rates: config.rates });

function liveWorlds(times: number[]): Map<number, World> {
  using sim = liveSim();
  return new Map([...times].sort((a, b) => a - b).map((t) => (sim.runUntil(t), [t, sim.world()] as const)));
}

describe("replayerFromParquet", () => {
  it("opens from the footer alone, with the lattice and end from the file", async () => {
    const r = await replayerFromParquet(keyframed);
    expect([r.nLefs, r.start, r.end, r.rowGroupsRead, r.ready]).toEqual([config.nLefs, 0, config.tEnd, 0, false]);
    expect(r.lattice).toEqual(lattice);
  });

  it("seeks anywhere reading at most one row group, matching the live run", async () => {
    const r = await replayerFromParquet(keyframed);
    const times = [250, 12.3, 199.9, 299, 77.7, 150];
    const expected = liveWorlds(times);
    for (const t of times) {
      const before = r.rowGroupsRead;
      expect((await r.worldAt(t)).lefs).toEqual(expected.get(t)!.lefs);
      expect(r.rowGroupsRead - before).toBeLessThanOrEqual(1);
    }
  });

  it("plays forward reading each row group once", async () => {
    const r = await replayerFromParquet(keyframed);
    const steps = Array.from({ length: 601 }, (_, k) => 0.5 * k);
    const expected = liveWorlds(steps);
    for (const t of steps) {
      expect(await r.advanceTo(t)).toBe(t);
      if (t % 25 === 0) expect(r.world().lefs).toEqual(expected.get(t)!.lefs);
    }
    expect(r.rowGroupsRead).toBe(ipcInfo.batches);
    // Going back before the data stops at its start; before any keyframe it waits.
    expect(await r.advanceTo(-5)).toBe(0);
    const fresh = await replayerFromParquet(keyframed);
    expect(await fresh.advanceTo(-5)).toBe(Number.NEGATIVE_INFINITY);
    await expect(fresh.seek(-5)).rejects.toThrow(/no keyframe at or before/);
  });

  it("decodes snapshots equal to a recording of the same run", async () => {
    using sim = liveSim();
    const rec = sim.recordUntil(config.tEnd, { dt: 0.9 }).snapshots!;
    const r = await replayerFromParquet(keyframed);
    expect(await r.snapshots(0.9)).toEqual(rec);
  });

  it("reads other files whole, given the lattice", async () => {
    await expect(replayerFromParquet(plain)).rejects.toThrow(/pass lattice/);
    const r = await replayerFromParquet(plain, { lattice });
    expect(r.rowGroupsRead).toBe(0);
    const expected = liveWorlds([123.4]);
    expect((await r.worldAt(123.4)).lefs).toEqual(expected.get(123.4)!.lefs);
  });

  it("reads from a Blob", async () => {
    const r = await replayerFromParquet(new Blob([keyframed]));
    const expected = liveWorlds([222.2]);
    expect((await r.worldAt(222.2)).lefs).toEqual(expected.get(222.2)!.lefs);
  });
});

describe("replayerFromParquet over HTTP", () => {
  let server: Server;
  let url = "";
  let bytesServed = 0;

  beforeAll(async () => {
    // A minimal static server with range requests, counting what it sends.
    server = createServer((req, res) => {
      const range = /bytes=(\d+)-(\d+)?/.exec(req.headers.range ?? "");
      if (req.method === "HEAD" || !range) {
        res.writeHead(200, { "Content-Length": keyframed.length, "Accept-Ranges": "bytes" });
        if (req.method !== "HEAD") {
          bytesServed += keyframed.length;
          res.end(keyframed);
        } else res.end();
        return;
      }
      const start = Number(range[1]);
      const end = range[2] === undefined ? keyframed.length - 1 : Math.min(Number(range[2]), keyframed.length - 1);
      const body = keyframed.subarray(start, end + 1);
      bytesServed += body.length;
      res.writeHead(206, { "Content-Range": `bytes ${start}-${end}/${keyframed.length}`, "Content-Length": body.length });
      res.end(body);
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/run.parquet`;
  });
  afterAll(() => new Promise<void>((resolve) => server.close(() => resolve())));

  it("reads from a URL", async () => {
    const r = await replayerFromParquet(url);
    const expected = liveWorlds([275]);
    expect((await r.worldAt(275)).lefs).toEqual(expected.get(275)!.lefs);
  });

  it("fetches the footer, then only the row groups it needs", async () => {
    // Uncached, so every read after opening is a range request. (Opening
    // reads the last 512 KB for the footer, which for this small fixture is
    // the whole file; real recordings are far larger.)
    bytesServed = 0;
    const r = await replayerFromParquet(await asyncBufferFromUrl({ url }));
    const footer = bytesServed;
    const expected = liveWorlds([275]);
    expect((await r.worldAt(275)).lefs).toEqual(expected.get(275)!.lefs);
    expect(r.rowGroupsRead).toBe(1);
    // One row group of seven: well under the whole file.
    expect(bytesServed - footer).toBeLessThan(keyframed.length / 3);
  });
});
