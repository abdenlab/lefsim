//! LEF entities: legs and the pair structures that group them.
//!
//! Legs are the dynamic units that walk on the lattice; LEFs are the pair
//! structure that owns two legs and is the target of the [`crate::Action::Respawn`]
//! action.

use slotmap::new_key_type;

use crate::lattice::{Dir, SiteIdx};

new_key_type! {
    /// Typed handle into [`crate::World::legs`].
    pub struct LegKey;
    /// Typed handle into [`crate::World::lefs`].
    pub struct LefKey;
}

/// Per-leg intrinsic mobility status, set by the most recent CTCF interaction.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Mobility {
    /// Free to step in `facing` direction (subject to extrinsic blocking).
    Moving,
    /// Pinned by a same-orientation CTCF.
    Captured,
    /// Knocked off by an opposite-orientation CTCF; awaits immediate respawn.
    Evicted,
}

/// A single LEF leg. Two legs make a LEF.
#[derive(Copy, Clone, Debug)]
pub struct Leg {
    /// Current site on the lattice.
    pub pos: SiteIdx,
    /// LEF this leg belongs to.
    pub lef: LefKey,
    /// The other leg of the same LEF.
    pub partner: LegKey,
    /// Direction this leg extrudes (fixed for the leg's lifetime).
    pub facing: Dir,
    pub mobility: Mobility,
}

/// A LEF: a pair of legs that share scheduling for the [`crate::Action::Respawn`]
/// action.
#[derive(Copy, Clone, Debug)]
pub struct Lef {
    /// The two legs of this LEF.
    /// By convention, `legs[0]` faces `Dir::Left` and `legs[1]` faces `Dir::Right`.
    pub legs: [LegKey; 2],
}
