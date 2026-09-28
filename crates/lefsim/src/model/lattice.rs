//! The immutable lattice substrate that LEF legs walk along.

/// Index into a [`Lattice`]'s site array. `u32` is plenty for any plausible
/// chromatin model and keeps `Action` keys compact.
pub type Position = u32;

/// One of the two travel/orientation directions on the 1D lattice.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Direction {
    Left,
    Right,
}

impl Direction {
    /// Names indexed by [`Direction::code`].
    pub const NAMES: [&str; 2] = ["Left", "Right"];

    /// Stable `u8` encoding used in recorded frames and metadata.
    pub fn code(self) -> u8 {
        match self {
            Direction::Left => 0,
            Direction::Right => 1,
        }
    }

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SiteKind {
    /// Plain lattice cell — leg passes through unchanged.
    Plain,
    /// CTCF site with an intrinsic polarity. `captures_from` names the
    /// CTCF's capture side; the opposite side is its eviction side.
    /// When a leg lands here, the CTCF acts with probability `strength`:
    ///
    /// - if the leg arrived from the **`captures_from`** side, it is
    ///   **captured**;
    /// - if the leg arrived from the opposite side, it is **evicted**.
    ///
    /// With probability `1 - strength` the leg passes through unchanged
    /// (`Mobility::Moving`). `strength` must lie in `[0, 1]`.
    ///
    /// Note: "arrival side" is the side of the CTCF the leg stepped *from*,
    /// not its facing. A right-facing leg (moving right) arrives from the
    /// **left** side. So `captures_from: Left` captures right-facing legs;
    /// `captures_from: Right` captures left-facing legs.
    Ctcf { captures_from: Direction, strength: f64 },
}

impl SiteKind {
    /// Names indexed by [`SiteKind::code`].
    pub const NAMES: [&str; 2] = ["Plain", "Ctcf"];

    /// Stable `u8` encoding used in metadata.
    pub fn code(self) -> u8 {
        match self {
            SiteKind::Plain => 0,
            SiteKind::Ctcf { .. } => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    pub kind: SiteKind,
}

impl Site {
    pub const fn plain() -> Self {
        Site { kind: SiteKind::Plain }
    }
    /// Construct a CTCF site. `captures_from` is the CTCF's capture side
    /// (the opposite side evicts). `strength` is the per-landing
    /// probability that the CTCF acts at all; the complement is the
    /// pass-through probability. It must lie in `[0, 1]`, which
    /// [`Lattice::new`] checks.
    pub fn ctcf(captures_from: Direction, strength: f64) -> Self {
        Site { kind: SiteKind::Ctcf { captures_from, strength } }
    }
    pub fn is_ctcf(&self) -> bool {
        matches!(self.kind, SiteKind::Ctcf { .. })
    }
}

/// One CTCF, as plain data: where it is, which side captures, and how
/// strongly. Used to build a [`Lattice`] and to describe one.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Ctcf {
    pub site: Position,
    pub captures_from: Direction,
    pub strength: f64,
}

/// Why a lattice couldn't be built.
#[derive(Clone, Debug, PartialEq)]
pub enum LatticeError {
    /// A lattice needs at least one site.
    Empty,
    /// Sites are indexed by [`Position`] (`u32`).
    TooLarge { n_sites: usize },
    CtcfOutOfRange { site: Position, n_sites: usize },
    DuplicateCtcf { site: Position },
    /// Strength must lie in `[0, 1]`.
    BadStrength { site: Position, strength: f64 },
}

impl std::fmt::Display for LatticeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LatticeError::Empty => write!(f, "a lattice needs at least one site"),
            LatticeError::TooLarge { n_sites } => {
                write!(f, "a lattice has at most {} sites, got {n_sites}", Position::MAX)
            }
            LatticeError::CtcfOutOfRange { site, n_sites } => {
                write!(f, "CTCF site {site} is outside a lattice of {n_sites} sites")
            }
            LatticeError::DuplicateCtcf { site } => write!(f, "CTCF site {site} is listed twice"),
            LatticeError::BadStrength { site, strength } => {
                write!(f, "CTCF strength must be in [0, 1], got {strength} at site {site}")
            }
        }
    }
}

impl std::error::Error for LatticeError {}

/// A 1D array of sites. Immutable for the lifetime of a simulation.
///
/// Always valid: every constructor checks that there's at least one site,
/// that the size fits a [`Position`], and that CTCF strengths lie in
/// `[0, 1]`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "super::serial::LatticeData", into = "super::serial::LatticeData")
)]
pub struct Lattice {
    pub(crate) sites: Vec<Site>,
}

impl Lattice {
    /// A lattice from explicit sites.
    pub fn new(sites: Vec<Site>) -> Result<Self, LatticeError> {
        if sites.is_empty() {
            return Err(LatticeError::Empty);
        }
        if sites.len() > Position::MAX as usize {
            return Err(LatticeError::TooLarge { n_sites: sites.len() });
        }
        for (i, site) in sites.iter().enumerate() {
            if let SiteKind::Ctcf { strength, .. } = site.kind
                && !(0.0..=1.0).contains(&strength)
            {
                return Err(LatticeError::BadStrength { site: i as Position, strength });
            }
        }
        Ok(Lattice { sites })
    }

    /// An all-plain lattice of `n_sites`.
    pub fn plain(n_sites: usize) -> Result<Self, LatticeError> {
        Self::with_ctcfs(n_sites, &[])
    }

    /// A lattice of `n_sites` with the given CTCFs (every other site plain).
    pub fn with_ctcfs(n_sites: usize, ctcfs: &[Ctcf]) -> Result<Self, LatticeError> {
        if n_sites == 0 {
            return Err(LatticeError::Empty);
        }
        if n_sites > Position::MAX as usize {
            return Err(LatticeError::TooLarge { n_sites });
        }
        let mut sites = vec![Site::plain(); n_sites];
        for c in ctcfs {
            let slot = sites
                .get_mut(c.site as usize)
                .ok_or(LatticeError::CtcfOutOfRange { site: c.site, n_sites })?;
            if slot.is_ctcf() {
                return Err(LatticeError::DuplicateCtcf { site: c.site });
            }
            *slot = Site::ctcf(c.captures_from, c.strength);
        }
        Self::new(sites)
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    /// Always `false`: a lattice has at least one site.
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    /// The CTCFs, in site order.
    pub fn ctcfs(&self) -> Vec<Ctcf> {
        self.sites
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.kind {
                SiteKind::Ctcf { captures_from, strength } => {
                    Some(Ctcf { site: i as Position, captures_from, strength })
                }
                SiteKind::Plain => None,
            })
            .collect()
    }

    /// Number of non-CTCF sites.
    pub fn n_plain(&self) -> usize {
        self.sites.iter().filter(|s| !s.is_ctcf()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctcf(site: Position, captures_from: Direction, strength: f64) -> Ctcf {
        Ctcf { site, captures_from, strength }
    }

    #[test]
    fn with_ctcfs_places_ctcfs_and_reads_them_back() {
        let specs = [ctcf(3, Direction::Left, 0.5), ctcf(7, Direction::Right, 1.0)];
        let lattice = Lattice::with_ctcfs(10, &specs).unwrap();
        assert_eq!(lattice.len(), 10);
        assert_eq!(lattice.ctcfs(), specs);
        assert_eq!(lattice.n_plain(), 8);
        assert_eq!(lattice.sites()[3], Site::ctcf(Direction::Left, 0.5));
    }

    #[test]
    fn plain_lattice_has_no_ctcfs() {
        let lattice = Lattice::plain(5).unwrap();
        assert!(lattice.ctcfs().is_empty());
        assert_eq!(lattice.n_plain(), 5);
    }

    #[test]
    fn rejects_invalid_lattices() {
        let left = Direction::Left;
        assert_eq!(Lattice::plain(0), Err(LatticeError::Empty));
        assert_eq!(Lattice::new(vec![]), Err(LatticeError::Empty));
        assert_eq!(
            Lattice::with_ctcfs(10, &[ctcf(10, left, 1.0)]),
            Err(LatticeError::CtcfOutOfRange { site: 10, n_sites: 10 })
        );
        assert_eq!(
            Lattice::with_ctcfs(10, &[ctcf(3, left, 1.0), ctcf(3, left, 0.5)]),
            Err(LatticeError::DuplicateCtcf { site: 3 })
        );
        assert_eq!(
            Lattice::with_ctcfs(10, &[ctcf(3, left, 1.5)]),
            Err(LatticeError::BadStrength { site: 3, strength: 1.5 })
        );
        assert!(matches!(
            Lattice::new(vec![Site::ctcf(left, f64::NAN)]),
            Err(LatticeError::BadStrength { site: 0, .. })
        ));
    }
}
