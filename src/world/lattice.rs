//! The immutable lattice substrate that LEF legs walk along.

/// Index into a [`Lattice`]'s site array. `u32` is plenty for any plausible
/// chromatin model and keeps `Action` keys compact.
pub type SiteIdx = u32;

/// One of the two travel/orientation directions on the 1D lattice.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Dir {
    Left,
    Right,
}

impl Dir {
    pub fn opposite(self) -> Self {
        match self {
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }

    /// Apply this direction as a one-cell offset to a site index.
    ///
    /// Returns `None` on underflow (stepping `Left` from site 0). Callers
    /// must additionally bounds-check the result against the lattice length.
    pub fn shift(self, p: SiteIdx) -> Option<SiteIdx> {
        match self {
            Dir::Left => p.checked_sub(1),
            Dir::Right => p.checked_add(1),
        }
    }
}

/// What kind of lattice site this is.
#[derive(Clone, Copy, Debug)]
pub enum SiteKind {
    /// Plain lattice cell — leg passes through unchanged.
    Plain,
    /// CTCF that *captures* legs whose `facing == stops` (they were on the
    /// CTCF's "back" side and stepped onto its "front") and *evicts* legs
    /// moving in the opposite direction.
    Ctcf { stops: Dir },
}

#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub kind: SiteKind,
}

impl Site {
    pub const fn plain() -> Self {
        Site { kind: SiteKind::Plain }
    }
    pub const fn ctcf(stops: Dir) -> Self {
        Site { kind: SiteKind::Ctcf { stops } }
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
    pub fn plain(n: usize) -> Self {
        Lattice { sites: vec![Site::plain(); n] }
    }
    pub fn len(&self) -> usize {
        self.sites.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}
