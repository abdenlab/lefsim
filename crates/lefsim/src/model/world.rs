//! The model's state: the lattice plus the entity arenas, and the per-site
//! occupancy index derived from leg positions.

use slotmap::SlotMap;
use smallvec::SmallVec;

use super::action::resolve_mobility;
use super::entity::{Leg, Lef, LefKey, LegKey, Mobility};
use super::lattice::{Direction, Lattice, Position};
use super::SimRng;

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
