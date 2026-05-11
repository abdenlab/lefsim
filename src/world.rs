//! Mutable world state: entity arenas plus the per-site occupancy index.

use slotmap::SlotMap;
use smallvec::SmallVec;

use crate::action::derive_mobility;
use crate::entity::{Leg, LefKey, LegKey, Lef, Mobility};
use crate::lattice::{Dir, Lattice, SiteIdx};

/// Per-site occupancy bucket. Multi-occupancy occurs transiently right after
/// a respawn (paired legs sharing the spawn site). Inline capacity 2 covers
/// that case without paying for a heap allocation.
pub type OccupancyBucket = SmallVec<[LegKey; 2]>;

pub struct World {
    pub lattice: Lattice,
    pub legs: SlotMap<LegKey, Leg>,
    pub lefs: SlotMap<LefKey, Lef>,
    /// `occupancy[i]` lists the legs currently sitting at site `i`.
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

    /// `true` iff some leg currently sits at `p`.
    pub fn occupied(&self, p: SiteIdx) -> bool {
        !self.occupancy[p as usize].is_empty()
    }

    pub fn legs_at(&self, p: SiteIdx) -> &[LegKey] {
        &self.occupancy[p as usize]
    }

    /// Place a leg at a site. Caller must ensure the leg is not already
    /// elsewhere on the lattice.
    pub(crate) fn place(&mut self, k: LegKey, p: SiteIdx) {
        self.occupancy[p as usize].push(k);
    }

    /// Remove a leg from a site if present.
    pub(crate) fn unplace(&mut self, k: LegKey, p: SiteIdx) {
        self.occupancy[p as usize].retain(|x| *x != k);
    }

    /// Construct a new LEF with both legs sitting at `pos` and facing outward.
    /// Returns the new `LefKey` and the two `LegKey`s in `[Left, Right]` order.
    pub fn spawn_lef(&mut self, pos: SiteIdx) -> (LefKey, [LegKey; 2]) {
        // Two-pass insert because each leg must reference the other.
        let l1 = self.legs.insert(Leg {
            pos,
            lef: LefKey::default(),
            partner: LegKey::default(),
            facing: Dir::Left,
            mobility: Mobility::Moving,
        });
        let l2 = self.legs.insert(Leg {
            pos,
            lef: LefKey::default(),
            partner: l1,
            facing: Dir::Right,
            mobility: Mobility::Moving,
        });
        let lef_k = self.lefs.insert(Lef { legs: [l1, l2] });

        // Patch the placeholder cross-references.
        let kind = self.lattice.sites[pos as usize].kind;
        {
            let leg = &mut self.legs[l1];
            leg.lef = lef_k;
            leg.partner = l2;
            leg.mobility = derive_mobility(kind, Dir::Left);
        }
        {
            let leg = &mut self.legs[l2];
            leg.lef = lef_k;
            leg.mobility = derive_mobility(kind, Dir::Right);
        }

        self.place(l1, pos);
        self.place(l2, pos);
        (lef_k, [l1, l2])
    }
}
