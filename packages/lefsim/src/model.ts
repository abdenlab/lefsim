/**
 * Building models: lattices, worlds and their rates, as plain objects.
 *
 * Every constructor checks its input through the core (in WebAssembly), so
 * an invalid lattice or world is reported where it's written, with the
 * core's message. The objects themselves are plain data: typed-array
 * columns with the core's codes for categorical values.
 */

import {
  check_lattice,
  check_model,
  check_world,
  default_rates as wasmDefaultRates,
  populate as wasmPopulate,
} from "#wasm";

import { DIRECTION, MOBILITY, type Ctcfs, type Lattice, type Lefs, type Model, type Rates, type SpawnRule, type World } from "./types.ts";

export type Side = "left" | "right";

/** One CTCF, as a row. `capturesFrom` is its capture side; the other side evicts. */
export interface CtcfSpec {
  site: number;
  capturesFrom: Side | number;
  /** Probability, in [0, 1], that a landing leg interacts with it. Default 1. */
  strength?: number;
}

/** CTCFs as columns; `capturesFrom` and `strength` may be one value for all. */
export interface CtcfColumns {
  site: ArrayLike<number>;
  capturesFrom: ArrayLike<Side | number> | Side | number;
  strength?: ArrayLike<number> | number;
}

type MobilityInput = number | string;

/** One LEF, as a row. Mobility defaults to moving. */
export interface LefSpec {
  leftPos: number;
  rightPos: number;
  leftMobility?: MobilityInput;
  rightMobility?: MobilityInput;
}

/** LEFs as columns (a {@link Lefs} or `LiveState` works too). Mobility defaults to moving. */
export interface LefColumns {
  leftPos: ArrayLike<number>;
  rightPos: ArrayLike<number>;
  leftMobility?: ArrayLike<MobilityInput>;
  rightMobility?: ArrayLike<MobilityInput>;
}

const U32_MAX = 2 ** 32 - 1;
export const SPAWN_RULES: readonly SpawnRule[] = ["uniform_non_ctcf"];

// Typed arrays coerce silently (-1 → 4294967295, 1.5 → 1), so check first.
export function toU32Array(values: ArrayLike<number>, what: string): Uint32Array {
  const out = new Uint32Array(values.length);
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (!Number.isInteger(v) || v < 0 || v > U32_MAX) {
      throw new RangeError(`${what} must be non-negative integers, got ${v}`);
    }
    out[i] = v;
  }
  return out;
}

/** A name (case-insensitive) or a code, as a code into `names`. */
function code(value: unknown, names: readonly string[], what: string): number {
  if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < names.length) return value;
  if (typeof value === "string") {
    const i = names.findIndex((n) => n.toLowerCase() === value.toLowerCase());
    if (i >= 0) return i;
  }
  const expected = names.map((n) => JSON.stringify(n.toLowerCase())).join(" or ");
  throw new RangeError(`${what} must be ${expected}, got ${JSON.stringify(value)}`);
}

function isArrayLike(v: unknown): v is ArrayLike<unknown> {
  return typeof v === "object" && v !== null && "length" in v;
}

/** Column `name`, or one value repeated: `n` entries. */
function column<T>(value: ArrayLike<T> | T, n: number, name: string): ArrayLike<T> {
  if (!isArrayLike(value)) return Array<T>(n).fill(value);
  if (value.length !== n) throw new RangeError(`${name} has ${value.length} entries, site has ${n}`);
  return value as ArrayLike<T>;
}

function ctcfColumns(ctcfs: CtcfSpec[] | CtcfColumns | undefined): Ctcfs {
  if (ctcfs === undefined) return { site: new Uint32Array(), capturesFrom: new Uint8Array(), strength: new Float64Array() };
  if (Array.isArray(ctcfs)) {
    return {
      site: toU32Array(ctcfs.map((c) => c.site), "CTCF sites"),
      capturesFrom: Uint8Array.from(ctcfs, (c) => code(c.capturesFrom, DIRECTION, "capturesFrom")),
      strength: Float64Array.from(ctcfs, (c) => c.strength ?? 1),
    };
  }
  const n = ctcfs.site.length;
  const sides = column(ctcfs.capturesFrom, n, "capturesFrom");
  const strength = column(ctcfs.strength ?? 1, n, "strength");
  return {
    site: toU32Array(ctcfs.site, "CTCF sites"),
    capturesFrom: Uint8Array.from({ length: n }, (_, i) => code(sides[i], DIRECTION, "capturesFrom")),
    strength: Float64Array.from({ length: n }, (_, i) => strength[i]!),
  };
}

function lefColumns(lefs: LefSpec[] | LefColumns): Lefs {
  if (Array.isArray(lefs)) {
    return {
      leftPos: toU32Array(lefs.map((l) => l.leftPos), "leftPos"),
      rightPos: toU32Array(lefs.map((l) => l.rightPos), "rightPos"),
      leftMobility: Uint8Array.from(lefs, (l) => code(l.leftMobility ?? 0, MOBILITY, "leftMobility")),
      rightMobility: Uint8Array.from(lefs, (l) => code(l.rightMobility ?? 0, MOBILITY, "rightMobility")),
    };
  }
  const n = lefs.leftPos.length;
  const mobility = (col: ArrayLike<MobilityInput> | undefined, name: string) =>
    col === undefined ? new Uint8Array(n) : Uint8Array.from({ length: col.length }, (_, i) => code(col[i], MOBILITY, name));
  return {
    leftPos: toU32Array(lefs.leftPos, "leftPos"),
    rightPos: toU32Array(lefs.rightPos, "rightPos"),
    leftMobility: mobility(lefs.leftMobility, "leftMobility"),
    rightMobility: mobility(lefs.rightMobility, "rightMobility"),
  };
}

function checkNSites(nSites: number): number {
  if (!Number.isInteger(nSites) || nSites < 0 || nSites > U32_MAX) {
    throw new RangeError(`nSites must be an integer in [0, 2^32), got ${nSites}`);
  }
  return nSites;
}

function checkSpawnRule(rule: string): SpawnRule {
  if (!SPAWN_RULES.includes(rule as SpawnRule)) {
    throw new RangeError(`unknown spawn rule ${JSON.stringify(rule)} (available: ${SPAWN_RULES.map((r) => JSON.stringify(r)).join(", ")})`);
  }
  return rule as SpawnRule;
}

export function seedBigInt(seed: number | bigint | undefined): bigint {
  if (seed === undefined) return 0n;
  if (typeof seed === "number" && !Number.isSafeInteger(seed)) {
    throw new RangeError(`seed must be an integer (use a bigint beyond 2^53), got ${seed}`);
  }
  const s = BigInt(seed);
  if (s < 0n || s >= 2n ** 64n) throw new RangeError(`seed must be in [0, 2^64), got ${seed}`);
  return s;
}

/** The core's arguments for a lattice. */
export function latticeArgs(l: Lattice): [number, Uint32Array, Uint8Array, Float64Array] {
  return [l.nSites, l.ctcfs.site, l.ctcfs.capturesFrom, l.ctcfs.strength];
}

/** The core's arguments for a LEF table. */
export function lefArgs(l: Lefs): [Uint32Array, Uint32Array, Uint8Array, Uint8Array] {
  return [l.leftPos, l.rightPos, l.leftMobility, l.rightMobility];
}

// ------------------------------------------------------------------ constructors

/**
 * A lattice of `nSites` sites with the given CTCFs (rows, or columns where
 * `capturesFrom` and `strength` may be one value for all). Checked by the
 * core: at least one site, CTCFs on the lattice and not repeated, strength
 * in [0, 1].
 */
export function createLattice(nSites: number, ctcfs?: CtcfSpec[] | CtcfColumns): Lattice {
  const lattice: Lattice = { nSites: checkNSites(nSites), ctcfs: ctcfColumns(ctcfs) };
  check_lattice(...latticeArgs(lattice));
  return lattice;
}

/**
 * A world: the LEFs on a lattice (rows, or columns such as a {@link Lefs}),
 * row `i` becoming LEF `i`. Mobility defaults to moving. Checked by the
 * core: legs on the lattice and not crossed, no two LEFs on one site, and a
 * captured leg only on a CTCF that could have captured it.
 */
export function createWorld(lattice: Lattice, lefs: LefSpec[] | LefColumns = []): World {
  const world: World = { lattice, lefs: lefColumns(lefs) };
  check_world(...latticeArgs(lattice), ...lefArgs(world.lefs));
  return world;
}

/** The core's default rates. */
export function defaultRates(): Rates {
  return wasmDefaultRates() as Rates;
}

/**
 * A model: a world, its rates (missing ones take the core's defaults) and a
 * spawn rule. Checked by the core: rates positive and finite, and the
 * spawn rule able to place the world's LEFs.
 */
export function createModel(world: World, rates: Partial<Rates> = {}, options: { spawnRule?: SpawnRule } = {}): Model {
  const model: Model = {
    world,
    rates: { ...defaultRates(), ...rates },
    spawnRule: checkSpawnRule(options.spawnRule ?? "uniform_non_ctcf"),
  };
  check_model(...modelArgs(model));
  return model;
}

/** The core's arguments for a model. */
export function modelArgs(m: Model) {
  const r = m.rates;
  return [...latticeArgs(m.world.lattice), ...lefArgs(m.world.lefs), r.step, r.respawn, r.respawnBlocked, r.respawnCaptured, m.spawnRule] as const;
}

/**
 * A world with `nLefs` LEFs placed one at a time by the spawn rule, exactly
 * as respawns place them. `seed` drives only this placement.
 */
export function populate(
  lattice: Lattice,
  nLefs: number,
  options: { seed?: number | bigint; spawnRule?: SpawnRule } = {},
): World {
  if (!Number.isInteger(nLefs) || nLefs < 0 || nLefs > U32_MAX) {
    throw new RangeError(`nLefs must be a non-negative integer, got ${nLefs}`);
  }
  const rule = checkSpawnRule(options.spawnRule ?? "uniform_non_ctcf");
  const lefs = wasmPopulate(...latticeArgs(lattice), nLefs, seedBigInt(options.seed), rule) as Lefs;
  return { lattice, lefs };
}

// ------------------------------------------------------------------ JSON

const FORMAT = "lefsim.model";
const VERSION = 2;

/**
 * A model as the versioned JSON document the other bindings read and write:
 * the lattice (size and CTCF rows), the LEF rows, the rates and the spawn
 * rule.
 */
export function modelToJson(model: Model, indent?: number | string): string {
  const { lattice, lefs } = model.world;
  const side = (c: number) => DIRECTION[c]!.toLowerCase();
  const mob = (c: number) => MOBILITY[c]!.toLowerCase();
  const doc = {
    format: FORMAT,
    version: VERSION,
    lattice: {
      n_sites: lattice.nSites,
      ctcfs: Array.from(lattice.ctcfs.site, (site, i) => ({
        site,
        captures_from: side(lattice.ctcfs.capturesFrom[i]!),
        strength: lattice.ctcfs.strength[i]!,
      })),
    },
    lefs: Array.from(lefs.leftPos, (leftPos, i) => ({
      left_pos: leftPos,
      right_pos: lefs.rightPos[i]!,
      left_mobility: mob(lefs.leftMobility[i]!),
      right_mobility: mob(lefs.rightMobility[i]!),
    })),
    rates: {
      step: model.rates.step,
      respawn: model.rates.respawn,
      respawn_blocked: model.rates.respawnBlocked,
      respawn_captured: model.rates.respawnCaptured,
    },
    spawn: { rule: model.spawnRule },
  };
  return JSON.stringify(doc, null, indent);
}

function field<T>(obj: unknown, key: string, where: string): T {
  if (typeof obj !== "object" || obj === null || !(key in obj)) throw new RangeError(`${where}: missing field "${key}"`);
  return (obj as Record<string, T>)[key]!;
}

/** A model from its JSON document, checked as {@link createModel} checks. */
export function modelFromJson(text: string): Model {
  const doc: unknown = JSON.parse(text);
  const format = field<string>(doc, "format", "model");
  if (format !== FORMAT) throw new RangeError(`expected format "${FORMAT}", got ${JSON.stringify(format)}`);
  const version = field<number>(doc, "version", "model");
  if (version !== VERSION) throw new RangeError(`unsupported ${FORMAT} version ${version} (this build reads version ${VERSION})`);

  const lat = field<object>(doc, "lattice", "model");
  const ctcfs = (field<unknown[]>(lat, "ctcfs", "lattice") ?? []).map((c) => ({
    site: field<number>(c, "site", "ctcf"),
    capturesFrom: field<Side>(c, "captures_from", "ctcf"),
    strength: field<number>(c, "strength", "ctcf"),
  }));
  const lattice = createLattice(field<number>(lat, "n_sites", "lattice"), ctcfs);

  const lefs = field<unknown[]>(doc, "lefs", "model").map((l) => ({
    leftPos: field<number>(l, "left_pos", "lef"),
    rightPos: field<number>(l, "right_pos", "lef"),
    leftMobility: (l as { left_mobility?: string }).left_mobility,
    rightMobility: (l as { right_mobility?: string }).right_mobility,
  }));
  const world = createWorld(lattice, lefs);

  const r = field<object>(doc, "rates", "model");
  const rates: Rates = {
    step: field<number>(r, "step", "rates"),
    respawn: field<number>(r, "respawn", "rates"),
    respawnBlocked: field<number>(r, "respawn_blocked", "rates"),
    respawnCaptured: field<number>(r, "respawn_captured", "rates"),
  };
  const spawnRule = field<SpawnRule>(field<object>(doc, "spawn", "model"), "rule", "spawn");
  return createModel(world, rates, { spawnRule });
}
