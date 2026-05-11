//! The immutable lattice substrate that LEF legs walk along.

/// Index into a [`Lattice`]'s site array. `u32` is plenty for any plausible
/// chromatin model and keeps `Action` keys compact.
pub type Position = u32;

/// One of the two travel/orientation directions on the 1D lattice.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Direction {
    Left,
    Right,
}

impl Direction {
    pub fn opposite(self) -> Self {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }

    /// Apply this direction as a one-cell offset to a site index.
    ///
    /// Returns `None` on underflow (stepping `Left` from site 0). Callers
    /// must additionally bounds-check the result against the lattice length.
    pub fn shift(self, p: Position) -> Option<Position> {
        match self {
            Direction::Left => p.checked_sub(1),
            Direction::Right => p.checked_add(1),
        }
    }
}

/// What kind of lattice site this is.
#[derive(Clone, Copy, Debug)]
pub enum SiteKind {
    /// Plain lattice cell — leg passes through unchanged.
    Plain,
    /// CTCF site. When a leg lands here, the CTCF "acts" on it with
    /// probability `strength`:
    /// - a leg whose `facing == stops` is **captured** (it was on the
    ///   CTCF's "back" side and arrived at its "front");
    /// - a leg whose `facing == stops.opposite()` is **evicted**.
    ///
    /// With probability `1 - strength`, the leg passes through unchanged
    /// (`Mobility::Moving`). `strength` must lie in `[0, 1]`.
    Ctcf { stops: Direction, strength: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub kind: SiteKind,
}

impl Site {
    pub const fn plain() -> Self {
        Site { kind: SiteKind::Plain }
    }
    /// Construct a CTCF site. `strength` is the per-landing probability
    /// that the CTCF acts on a leg (captures it or evicts it, depending
    /// on the leg's facing); the complement is the per-landing
    /// pass-through probability. Must lie in `[0, 1]`.
    pub fn ctcf(stops: Direction, strength: f64) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&strength),
            "CTCF strength must be in [0, 1], got {strength}"
        );
        Site { kind: SiteKind::Ctcf { stops, strength } }
    }
    pub fn is_ctcf(&self) -> bool {
        matches!(self.kind, SiteKind::Ctcf { .. })
    }
}

/// A 1D array of sites. Immutable for the lifetime of a simulation.
#[derive(Debug)]
pub struct Lattice {
    pub sites: Vec<Site>,
}

impl Lattice {
    pub fn new(sites: Vec<Site>) -> Self {
        Lattice { sites }
    }
    /// All-plain lattice of length `n`.
    pub fn new_plain(n: usize) -> Self {
        Lattice { sites: vec![Site::plain(); n] }
    }
    pub fn len(&self) -> usize {
        self.sites.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}
