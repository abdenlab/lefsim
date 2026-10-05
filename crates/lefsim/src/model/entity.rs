//! LEF entities: legs and the pair structures that group them.
//!
//! Legs are the dynamic units that walk on the lattice; LEFs are the pair
//! structure that owns two legs and is the target of the [`crate::Action::Respawn`]
//! action.

use slotmap::new_key_type;

use super::lattice::{Direction, Position};

new_key_type! {
    /// Typed handle into [`crate::World::legs`].
    pub struct LegKey;
    /// Typed handle into [`crate::World::lefs`].
    pub struct LefKey;
}

/// Per-leg intrinsic mobility status, set by the most recent CTCF interaction.
///
/// Stored as one byte, its [`Mobility::code`], so mobility columns hand off
/// to Arrow or JS as bytes without copying (see [`Mobility::into_codes`]).
#[derive(Copy, Clone, Eq, PartialEq, Debug, bytemuck::NoUninit)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Mobility {
    /// Free to step in `facing` direction (subject to extrinsic blocking:
    /// a leg directly ahead, or the lattice's end).
    Moving = 0,
    /// Held by a CTCF it reached from the CTCF's capture side. Protects its
    /// LEF from faster unloading.
    Captured = 1,
}

impl Mobility {
    /// Names indexed by [`Mobility::code`].
    pub const NAMES: [&str; 2] = ["Moving", "Captured"];

    /// Stable `u8` encoding: the index into [`Mobility::NAMES`].
    pub fn code(self) -> u8 {
        self as u8
    }

    /// A mobility column as its codes, without copying.
    pub fn as_codes(column: &[Mobility]) -> &[u8] {
        bytemuck::cast_slice(column)
    }

    /// A mobility column as its codes, reusing the allocation.
    pub fn into_codes(column: Vec<Mobility>) -> Vec<u8> {
        bytemuck::allocation::cast_vec(column)
    }

    /// The inverse of [`Mobility::code`].
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Mobility::Moving),
            1 => Some(Mobility::Captured),
            _ => None,
        }
    }
}

/// A single LEF leg. Two legs make a LEF.
#[derive(Copy, Clone, Debug)]
pub struct Leg {
    /// Current site on the lattice.
    pub pos: Position,
    /// LEF this leg belongs to.
    pub lef: LefKey,
    /// The other leg of the same LEF.
    pub partner: LegKey,
    /// Direction this leg extrudes (fixed for the leg's lifetime).
    pub facing: Direction,
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
