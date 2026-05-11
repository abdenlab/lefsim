//! Mutable world state: entity arenas plus the per-site occupancy index.
//!
//! The two submodules — [`entity`] and [`lattice`] — describe, respectively,
//! the dynamic units (legs/LEFs) and the immutable substrate they walk on.
//! Their contents are re-exported at this module level for convenience.

pub mod entity;
pub mod lattice;

pub use entity::{Leg, Lef, LefKey, LegKey, Mobility};
pub use lattice::{Direction, Lattice, Site, Position, SiteKind};

use slotmap::SlotMap;
use smallvec::SmallVec;

use crate::action::derive_mobility;

/// Per-site occupancy bucket. Multi-occupancy occurs transiently right after
/// a respawn (paired legs sharing the spawn site). Inline capacity 2 covers
/// that case without paying for a heap allocation.
pub type OccupancyBucket = SmallVec<[LegKey; 2]>;

pub struct World {
    pub lattice: Lattice,
    pub legs: SlotMap<LegKey, Leg>,
    pub lefs: SlotMap<LefKey, Lef>,
    /// `occupancy[i]` lists the leg(s) currently sitting at site `i`.
    pub occupancy: Vec<OccupancyBucket>,
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

    /// `true` iff some leg currently sits at `p`.
    pub fn occupied(&self, p: Position) -> bool {
        !self.occupancy[p as usize].is_empty()
    }

    pub fn legs_at(&self, p: Position) -> &[LegKey] {
        &self.occupancy[p as usize]
    }

    /// Construct a new LEF with both legs sitting at `pos` and facing outward.
    /// Returns the new `LefKey` and the two `LegKey`s in `[Left, Right]` order.
    pub fn spawn_lef(&mut self, pos: Position) -> (LefKey, [LegKey; 2]) {
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

        // Patch the placeholder cross-references.
        let kind = self.lattice.sites[pos as usize].kind;
        {
            let leg = &mut self.legs[l1];
            leg.lef = lef_k;
            leg.partner = l2;
            leg.mobility = derive_mobility(kind, Direction::Left);
        }
        {
            let leg = &mut self.legs[l2];
            leg.lef = lef_k;
            leg.mobility = derive_mobility(kind, Direction::Right);
        }

        self.insert_leg(l1, pos);
        self.insert_leg(l2, pos);
        (lef_k, [l1, l2])
    }
}
