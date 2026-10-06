/**
 * Effective distance: the shortest path between lattice sites when a path
 * walks the lattice and may also cross LEFs, as loops bring their anchors
 * together.
 *
 * A path walks at `stepLength` per lattice step and crosses a LEF, from one
 * leg to the other, at `hopLength`. A rosette (a chain of LEFs side by side,
 * one's right leg next to the next's left leg) costs a hop per LEF to cross,
 * and the junctions between its LEFs are free.
 * Both lengths are integers, so distances are exact. Keep hops at least as
 * long as steps: with shorter hops, doubling back across a LEF can beat a
 * single step.
 *
 * Computed by the core, in WebAssembly, so call `init()` first.
 */

import { effective_distances, effective_distances_over } from "#wasm";

import { toU32Array } from "./model.ts";
import type { Lattice, SnapshotColumns, World } from "./types.ts";

/** The lengths of a lattice step and of a hop across a LEF. Both required. */
export interface PathLengths {
  /** The length of a step between neighboring sites: a positive integer. */
  stepLength: number;
  /** The length of a hop across a LEF: a non-negative integer, best at least `stepLength`. */
  hopLength: number;
}

/** Sites as a `Uint32Array`, checked (a typed array would coerce silently). */
function siteArray(sites: ArrayLike<number>, what: string): Uint32Array {
  return sites instanceof Uint32Array ? sites : toU32Array(sites, what);
}

/** The effective distance between sites `a` and `b` in `world`. */
export function effectiveDistance(world: World, a: number, b: number, lengths: PathLengths): number {
  return effectiveDistances(world, [a], { ...lengths, targets: [b] })[0]!;
}

/**
 * Effective distances in `world`. Without `targets`, every pair of `sites`,
 * condensed as scipy's `pdist`: pair `(i, j)` with `i < j`, row-major, for
 * `k(k−1)/2` entries. With `targets`, each of `sites` to each of `targets`,
 * row-major. Many sites share work: one shortest-path search per distinct
 * nearest leg of the sites.
 */
export function effectiveDistances(
  world: World,
  sites: ArrayLike<number>,
  options: PathLengths & { targets?: ArrayLike<number> },
): Float64Array {
  const { lattice, lefs } = world;
  const targets = options.targets === undefined ? undefined : siteArray(options.targets, "targets");
  return effective_distances(
    lattice.nSites,
    lefs.leftPos,
    lefs.rightPos,
    siteArray(sites, "sites"),
    targets,
    options.stepLength,
    options.hopLength,
  );
}

/**
 * Every pair of `sites` in each snapshot of a recording (from
 * `Sim.recordUntil` or `Replayer.snapshots`) on `lattice`: one condensed row
 * of `k(k−1)/2` per snapshot, row-major, so snapshot `s` starts at
 * `s * k(k−1)/2`. The times are the snapshots' `time`.
 */
export function effectiveDistancesOver(
  lattice: Lattice,
  snapshots: SnapshotColumns,
  sites: ArrayLike<number>,
  lengths: PathLengths,
): Float64Array {
  return effective_distances_over(
    lattice.nSites,
    snapshots.nLefs,
    snapshots.leftPos,
    snapshots.rightPos,
    siteArray(sites, "sites"),
    lengths.stepLength,
    lengths.hopLength,
  );
}
