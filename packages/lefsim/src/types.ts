/**
 * Shared constants and data shapes, used by both `Sim` and `Replayer`. Kept separate so neither module imports the other.
 */

/** Mobility names, indexed by the codes in {@link LiveState} mobility arrays. */
export const MOBILITY = ["Moving", "Captured"] as const;
export type Mobility = (typeof MOBILITY)[number];

/** Direction names, indexed by the codes in {@link Ctcfs} `capturesFrom`. */
export const DIRECTION = ["Left", "Right"] as const;

/** Site-kind names, indexed by the core's site-kind codes. */
export const SITE_KIND = ["Plain", "Ctcf"] as const;

/** Row-kind names, indexed by the codes in a {@link DeltaChunk}'s `kind`. */
export const ROW_KIND = ["Step", "Respawn", "Keyframe", "Evict"] as const;

/** The `kind` code of keyframe rows. */
export const KEYFRAME = 2;

/**
 * A chunk of event deltas: one row per leg changed by each event, sorted by
 * time. Rows carry absolute values (the leg's new position and mobility), so
 * applying them in order is an idempotent overwrite. A keyframed chunk opens
 * with one `Keyframe` row per leg holding the full state.
 *
 * This plain-columns shape is what {@link Replayer} consumes, whatever the
 * source (a local {@link Sim}, or later a network stream).
 */
export interface DeltaChunk {
  time: Float64Array;
  legId: Uint32Array;
  pos: Uint32Array;
  /** {@link MOBILITY} codes. */
  mobility: Uint8Array;
  /** {@link ROW_KIND} codes. */
  kind: Uint8Array;
  /** The chunk describes the state through this time, even past its last row. */
  end: number;
}

/**
 * Every LEF's current state, indexed by `lef_id`.
 *
 * The arrays are views into WebAssembly memory. **They are valid only until
 * the next call into the Sim** (or any Sim): wasm memory can grow, which
 * detaches them. Copy what you need to keep (`arr.slice()`).
 */
export interface LiveState {
  time: number;
  leftPos: Uint32Array;
  rightPos: Uint32Array;
  /** {@link MOBILITY} codes. */
  leftMobility: Uint8Array;
  rightMobility: Uint8Array;
}

/**
 * Grid snapshots of every LEF, snapshot-major: `time` has one entry per
 * snapshot, and every other column has `nLefs` entries per snapshot, so
 * snapshot `s`, LEF `i` is at index `s * nLefs + i`.
 */
export interface SnapshotColumns {
  nLefs: number;
  time: Float64Array;
  leftPos: Uint32Array;
  rightPos: Uint32Array;
  /** {@link MOBILITY} codes. */
  leftMobility: Uint8Array;
  rightMobility: Uint8Array;
}

/** A chunk of grid snapshots, from `Sim.recordSnapshotChunk`. */
export interface SnapshotChunk extends SnapshotColumns {
  /** The sim's time after the call. */
  end: number;
  stopped: StopReason;
}

/**
 * Why a run call returned. `"recorder_full"` comes only from the chunk
 * methods and streams: the chunk filled first; call again for the next.
 */
export type StopReason = "reached_time" | "reached_event_count" | "exhausted" | "recorder_full";

export interface Rates {
  /** Step rate of a moving, unblocked leg. */
  step: number;
  /** Basal respawn rate: a LEF with both legs free to move. */
  respawn: number;
  /**
   * Respawn rate of a LEF with either leg blocked (by a leg ahead or the
   * lattice's end), unless the other is captured.
   */
  respawnBlocked: number;
  /** Respawn rate of a LEF with a captured leg, whatever the other leg does. */
  respawnCaptured: number;
}

/** The built-in spawn rules: where a LEF lands when it (re)spawns. */
export type SpawnRule = "uniform_non_ctcf";

/** A lattice's CTCFs as columns. `capturesFrom` holds {@link DIRECTION} codes. */
export interface Ctcfs {
  site: Uint32Array;
  capturesFrom: Uint8Array;
  strength: Float64Array;
}

/** A 1D lattice: its size and its CTCFs. Create with `createLattice`. */
export interface Lattice {
  readonly nSites: number;
  readonly ctcfs: Ctcfs;
}

/**
 * The LEF table: one row per LEF, row `i` being LEF `i`. Mobility columns
 * hold {@link MOBILITY} codes. The same columns as a {@link LiveState}.
 */
export interface Lefs {
  leftPos: Uint32Array;
  rightPos: Uint32Array;
  leftMobility: Uint8Array;
  rightMobility: Uint8Array;
}

/** The state of a simulation at one moment. Create with `createWorld` or `populate`. */
export interface World {
  readonly lattice: Lattice;
  readonly lefs: Lefs;
}

/** A complete model: everything a run needs except a seed. Create with `createModel`. */
export interface Model {
  readonly world: World;
  readonly rates: Rates;
  readonly spawnRule: SpawnRule;
}
