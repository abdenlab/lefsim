//! Mutable world state: entity arenas plus the per-site occupancy index.
//!
//! The two submodules — [`entity`] and [`lattice`] — describe, respectively,
//! the dynamic units (legs/LEFs) and the immutable substrate they walk on.
//! Their contents are re-exported at this module level for convenience.

pub mod entity;
pub mod lattice;

pub use entity::{Leg, Lef, LefKey, LegKey, Mobility};
pub use lattice::{Direction, Lattice, Site, Position, SiteKind};

use rand::Rng;
use slotmap::SlotMap;
use smallvec::SmallVec;

use crate::engine::SimRng;

/// Per-site occupancy bucket. Multi-occupancy occurs transiently right after
/// a respawn (paired legs sharing the spawn site). Inline capacity 2 covers
/// that case without paying for a heap allocation.
pub type OccupancyBucket = SmallVec<[LegKey; 2]>;

/// Fields are crate-private so that the derived occupancy index can't drift
/// out of sync with leg positions; read through the accessors.
pub struct World {
    pub(crate) lattice: Lattice,
    pub(crate) legs: SlotMap<LegKey, Leg>,
    pub(crate) lefs: SlotMap<LefKey, Lef>,
    /// `occupancy[i]` lists the leg(s) currently sitting at site `i`.
    pub(crate) occupancy: Vec<OccupancyBucket>,
}

impl World {
    pub fn new(lattice: Lattice) -> Self {
        let n = lattice.len();
        World {
            lattice,
            legs: SlotMap::with_key(),
            lefs: SlotMap::with_key(),
            occupancy: vec![OccupancyBucket::new(); n],
        }
    }

    /// Place a leg at a site. Caller must ensure the leg is not already
    /// elsewhere on the lattice.
    pub(crate) fn insert_leg(&mut self, k: LegKey, p: Position) {
        self.occupancy[p as usize].push(k);
    }

    /// Remove a leg from a site if present.
    pub(crate) fn remove_leg(&mut self, k: LegKey, p: Position) {
        self.occupancy[p as usize].retain(|x| *x != k);
    }

    pub fn lattice(&self) -> &Lattice {
        &self.lattice
    }

    pub fn legs(&self) -> &SlotMap<LegKey, Leg> {
        &self.legs
    }

    pub fn lefs(&self) -> &SlotMap<LefKey, Lef> {
        &self.lefs
    }

    /// `true` iff some leg currently sits at `p`.
    pub fn occupied(&self, p: Position) -> bool {
        !self.occupancy[p as usize].is_empty()
    }

    pub fn legs_at(&self, p: Position) -> &[LegKey] {
        &self.occupancy[p as usize]
    }

    /// Construct a new LEF with both legs sitting at `pos` and facing outward.
    /// Returns the new `LefKey` and the two `LegKey`s in `[Left, Right]` order.
    ///
    /// Counts as a *landing* for both legs (each rolls `resolve_mobility`
    /// against the site they're placed on), so callers must thread an RNG.
    ///
    /// Crate-private: a LEF added after the engine is initialized would have
    /// no actions in the heap. [`crate::SimBuilder`] is the public way in.
    pub(crate) fn spawn_lef(
        &mut self,
        pos: Position,
        rng: &mut SimRng,
    ) -> (LefKey, [LegKey; 2]) {
        // Two-pass insert because each leg must reference the other.
        let l1 = self.legs.insert(Leg {
            pos,
            lef: LefKey::default(),
            partner: LegKey::default(),
            facing: Direction::Left,
            mobility: Mobility::Moving,
        });
        let l2 = self.legs.insert(Leg {
            pos,
            lef: LefKey::default(),
            partner: l1,
            facing: Direction::Right,
            mobility: Mobility::Moving,
        });
        let lef_k = self.lefs.insert(Lef { legs: [l1, l2] });

        // Patch the placeholder cross-references and roll for landing
        // mobility. Initial placement isn't a "step" in the usual sense;
        // we treat it as if each leg arrived from the side opposite its
        // facing, matching the convention used by fire_step under
        // forward-only motion. See resolve_mobility for why this matters.
        let kind = self.lattice.sites[pos as usize].kind;
        {
            let facing = Direction::Left;
            let leg = &mut self.legs[l1];
            leg.lef = lef_k;
            leg.partner = l2;
            leg.mobility = resolve_mobility(kind, facing.opposite(), rng);
        }
        {
            let facing = Direction::Right;
            let leg = &mut self.legs[l2];
            leg.lef = lef_k;
            leg.mobility = resolve_mobility(kind, facing.opposite(), rng);
        }

        self.insert_leg(l1, pos);
        self.insert_leg(l2, pos);
        (lef_k, [l1, l2])
    }
}

/// Stochastic resolution of a leg's mobility *at the moment it lands at a site*.
///
/// Takes `arrived_from` — the side of the destination cell from which the
/// leg arrived — rather than the leg's facing. The caller decides how to
/// derive that from its step model; in the current forward-only stepping
/// model, `arrived_from == leg.facing.opposite()`, but a model with
/// back-stepping or independent step direction would derive it differently.
///
/// # Invariant
///
/// **Only call this once per actual landing.** Capture and eviction are
/// modeled as events that fire *on the act of stepping onto a site* — not
/// as facts derivable from position. Calling this function on a leg that
/// hasn't moved would give it repeated, independent chances of being
/// captured/evicted, which is physically wrong: a leg standing still on a
/// CTCF that already failed to capture it must not get fresh rolls each
/// time the scheduler revisits it.
///
/// Concretely: never call from any `schedule_*` function or any other
/// non-mutating context. The correct call sites are exactly:
///
/// - [`crate::action`]`::fire_step` — leg stepped to a new cell
/// - [`crate::action`]`::fire_respawn` — LEF placed its legs at the spawn cell
/// - [`World::spawn_lef`] — initial placement at simulation start
///
/// # Semantics
///
/// - `Plain` sites: always returns `Moving`.
/// - `Ctcf { captures_from, strength }`:
///   - With probability `strength`: capture if `arrived_from == captures_from`,
///     otherwise evict.
///   - With probability `1 - strength`: pass through (`Moving`).
pub(crate) fn resolve_mobility(
    site: SiteKind,
    arrived_from: Direction,
    rng: &mut SimRng,
) -> Mobility {
    match site {
        SiteKind::Plain => Mobility::Moving,
        SiteKind::Ctcf { captures_from, strength } => {
            if !rng.gen_bool(strength) {
                Mobility::Moving
            } else if arrived_from == captures_from {
                Mobility::Captured
            } else {
                Mobility::Evicted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn plain_site_always_moving() {
        let mut rng = SimRng::seed_from_u64(0);
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Right, &mut rng),
            Mobility::Moving
        );
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Left, &mut rng),
            Mobility::Moving
        );
    }

    #[test]
    fn strength_1_captures_leg_arriving_on_capture_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // CTCF's capture side is its right side. A leg that arrived from
        // the right should be captured.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(
            resolve_mobility(site, Direction::Right, &mut rng),
            Mobility::Captured
        );
    }

    #[test]
    fn strength_1_evicts_leg_arriving_on_other_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // CTCF's capture side is its right side; the left side evicts.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(
            resolve_mobility(site, Direction::Left, &mut rng),
            Mobility::Evicted
        );
    }

    #[test]
    fn strength_0_always_passes_through() {
        let mut rng = SimRng::seed_from_u64(0);
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 0.0 };
        for _ in 0..50 {
            assert_eq!(resolve_mobility(site, Direction::Right, &mut rng), Mobility::Moving);
            assert_eq!(resolve_mobility(site, Direction::Left, &mut rng), Mobility::Moving);
        }
    }

    #[test]
    fn capture_fraction_tracks_strength() {
        let mut rng = SimRng::seed_from_u64(42);
        let p = 0.3;
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: p };
        // Arrive from the capture side so the outcome on success is Captured.
        let n = 50_000;
        let mut captured = 0usize;
        for _ in 0..n {
            if resolve_mobility(site, Direction::Right, &mut rng) == Mobility::Captured {
                captured += 1;
            }
        }
        let frac = captured as f64 / n as f64;
        // ~4 sigma for a Bernoulli(p) sample mean.
        let sigma = (p * (1.0 - p) / n as f64).sqrt();
        assert!(
            (frac - p).abs() < 4.0 * sigma,
            "capture fraction {frac} differs from p={p} by more than 4σ ({sigma:.4})"
        );
    }
}
