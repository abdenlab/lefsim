// The golden fingerprint from crates/lefsim/tests/determinism.rs, reproduced
// through the whole JS stack: wasm → Sim.recordEvents → Arrow JS batches.
// Rust (native and wasm32) and Python already produce this exact value.

import type { RecordBatch } from "apache-arrow";
import { expect, it } from "vitest";

import { eventsToArrow, snapshotsToArrow } from "../src/arrow.ts";
import { Sim } from "../src/index.ts";
import { makeSim, type Setup } from "./helpers.ts";

const RUST_GOLDEN = 0x1b52b58ebe898456n;

/** FNV-1a, 64-bit, over a byte array. */
function fnv1a(bytes: Uint8Array, h = 0xcbf29ce484222325n): bigint {
  for (const b of bytes) h = BigInt.asUintN(64, (h ^ BigInt(b)) * 0x100000001b3n);
  return h;
}

/** A column's raw values (these batches are single-chunk, zero-offset). */
const values = <T>(b: RecordBatch, name: string) => b.getChild(name)!.data[0]!.values as T;

it("reproduces the Rust golden fingerprint from Arrow JS columns", () => {
  const sites = Array.from({ length: 10 }, (_, k) => 25 + 50 * k);
  using sim = makeSim({
    nSites: 500,
    ctcfs: sites.map((site) => ({ site, capturesFrom: site % 100 === 25 ? "right" : "left", strength: 0.7 })),
    nLefs: 20,
    seed: 0x5eed,
    rates: { step: 1, respawn: 0.01, respawnCaptured: 0.002 },
  });
  const rec = sim.recordEvents(50_000, { dt: 5, events: true });
  expect(rec.nEvents).toBe(50_000);
  const events = eventsToArrow(rec.events!);
  const snaps = snapshotsToArrow(rec.snapshots!);

  // Events, row by row: time bits, leg_id, pos (little-endian), mobility, kind.
  const n = events.numRows;
  const rows = new DataView(new ArrayBuffer(n * 18));
  const [time, leg, pos] = [values<Float64Array>(events, "time"), values<Uint32Array>(events, "leg_id"), values<Uint32Array>(events, "pos")];
  const [mob, kind] = [values<Uint8Array>(events, "mobility"), values<Uint8Array>(events, "action")];
  for (let i = 0; i < n; i++) {
    const o = i * 18;
    rows.setFloat64(o, time[i]!, true);
    rows.setUint32(o + 8, leg[i]!, true);
    rows.setUint32(o + 12, pos[i]!, true);
    rows.setUint8(o + 16, mob[i]!);
    rows.setUint8(o + 17, kind[i]!);
  }
  let h = fnv1a(new Uint8Array(rows.buffer));

  // Snapshots: each snapshot time, then (left, right) position pairs, then
  // the left and right mobility columns.
  const nLefs = Number(snaps.schema.metadata.get("lefsim.n_lefs"));
  const tiled = values<Float64Array>(snaps, "time");
  const times = new DataView(new ArrayBuffer((tiled.length / nLefs) * 8));
  for (let s = 0; s < tiled.length / nLefs; s++) times.setFloat64(s * 8, tiled[s * nLefs]!, true);
  h = fnv1a(new Uint8Array(times.buffer), h);
  const [left, right] = [values<Uint32Array>(snaps, "left_pos"), values<Uint32Array>(snaps, "right_pos")];
  const pairs = new DataView(new ArrayBuffer(left.length * 8));
  for (let i = 0; i < left.length; i++) {
    pairs.setUint32(i * 8, left[i]!, true);
    pairs.setUint32(i * 8 + 4, right[i]!, true);
  }
  h = fnv1a(new Uint8Array(pairs.buffer), h);
  h = fnv1a(values<Uint8Array>(snaps, "left_mobility"), h);
  h = fnv1a(values<Uint8Array>(snaps, "right_mobility"), h);

  expect(h.toString(16)).toBe(RUST_GOLDEN.toString(16));
});
