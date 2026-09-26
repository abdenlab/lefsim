/**
 * Shared constants and data shapes, used by both `Sim` and `Replayer`. Kept separate so neither module imports the other.
 */

/** Mobility names, indexed by the codes in {@link LiveState} mobility arrays. */
export const MOBILITY = ["Moving", "Captured", "Evicted"] as const;
export type Mobility = (typeof MOBILITY)[number];

/** Direction names, indexed by the codes in {@link Meta} `capturesFrom` / `facing`. */
export const DIRECTION = ["Left", "Right"] as const;

/** Site-kind names, indexed by the codes in {@link Meta} `lattice.kind`. */
export const SITE_KIND = ["Plain", "Ctcf"] as const;

/** Row-kind names, indexed by the codes in a {@link DeltaChunk}'s `kind`. */
export const ROW_KIND = ["Step", "Respawn", "Keyframe"] as const;

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
