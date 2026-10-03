//! The model's state: the lattice plus the entity arenas, and the per-site
//! occupancy index derived from leg positions.
//!
//! Outside the world, the LEF state is the key-free [`LefStates`] table: one
//! row per LEF (the row index is its `lef_id`), with each leg's position and
//! mobility. Slotmap keys and the occupancy index stay internal.

use slotmap::SlotMap;
use smallvec::SmallVec;

use super::action::land_lef;
use super::entity::{Leg, Lef, LefKey, LegKey, Mobility};
use super::lattice::{Direction, Lattice, Position, SiteKind};
use super::SimRng;

/// The state of one leg: where it is and whether it can move.
pub type LegState = (Position, Mobility);

/// The LEF table, as plain data: one row per LEF, each column stored as its
/// own `Vec`. In a world's table (see [`World::lef_states`]) row `i` is LEF
/// `i`: its left-facing leg (`leg_id = 2i`) and right-facing leg
/// (`leg_id = 2i + 1`). Recorded snapshots stack such tables (see
/// [`LefFrames`](crate::LefFrames)).
///
/// Serialized (with the `serde` feature) as a list of rows.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(into = "Vec<super::serial::LefRow>", from = "Vec<super::serial::LefRow>")
)]
pub struct LefStates {
    pub left_pos: Vec<Position>,
    pub right_pos: Vec<Position>,
    pub left_mobility: Vec<Mobility>,
    pub right_mobility: Vec<Mobility>,
}

impl LefStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty table with room for `n` rows.
    pub fn with_capacity(n: usize) -> Self {
        LefStates {
            left_pos: Vec::with_capacity(n),
            right_pos: Vec::with_capacity(n),
            left_mobility: Vec::with_capacity(n),
            right_mobility: Vec::with_capacity(n),
        }
    }

    /// Ensure room for `n` more rows.
    pub fn reserve(&mut self, n: usize) {
        self.left_pos.reserve(n);
        self.right_pos.reserve(n);
        self.left_mobility.reserve(n);
        self.right_mobility.reserve(n);
    }

    /// Append all rows of `other`.
    pub fn extend_from(&mut self, other: &LefStates) {
        self.left_pos.extend_from_slice(&other.left_pos);
        self.right_pos.extend_from_slice(&other.right_pos);
        self.left_mobility.extend_from_slice(&other.left_mobility);
        self.right_mobility.extend_from_slice(&other.right_mobility);
    }

    /// Drop all rows, keeping allocated capacity.
    pub fn clear(&mut self) {
        self.left_pos.clear();
        self.right_pos.clear();
        self.left_mobility.clear();
        self.right_mobility.clear();
    }

    pub fn len(&self) -> usize {
        self.left_pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left_pos.is_empty()
    }

    /// Append a row.
    pub fn push(&mut self, left: LegState, right: LegState) {
        self.left_pos.push(left.0);
        self.left_mobility.push(left.1);
        self.right_pos.push(right.0);
        self.right_mobility.push(right.1);
    }

    /// Row `i`: the left and right leg states of LEF `i`.
    pub fn get(&self, i: usize) -> Option<(LegState, LegState)> {
        Some((
            (*self.left_pos.get(i)?, *self.left_mobility.get(i)?),
            (*self.right_pos.get(i)?, *self.right_mobility.get(i)?),
        ))
    }

    pub(crate) fn columns_agree(&self) -> bool {
        let n = self.left_pos.len();
        self.right_pos.len() == n && self.left_mobility.len() == n && self.right_mobility.len() == n
    }
}

/// Why a LEF state isn't valid in a world. `lef` is the LEF's id (its row).
#[derive(Clone, Debug, PartialEq)]
pub enum WorldError {
    /// The columns of a [`LefStates`] have different lengths.
    ColumnLengths,
    PositionOutOfRange { lef: usize, pos: Position, n_sites: usize },
    /// The left leg must not be to the right of the right leg.
    LegsCrossed { lef: usize, left: Position, right: Position },
    /// Another leg already sits there. A LEF's own two legs may share a site
    /// (as right after it's placed); two LEFs may not.
    SiteOccupied { lef: usize, pos: Position },
    /// A captured leg must sit on a CTCF...
    CapturedOffCtcf { lef: usize, pos: Position },
    /// ...that captures from the side it arrived from. Legs only step
    /// forward, so they arrive from the side opposite their facing: a
    /// left-facing leg can only be captured by a CTCF that captures from the
    /// right, and a right-facing leg by one that captures from the left.
    CapturedWrongSide { lef: usize, pos: Position, facing: Direction },
    /// A stalled leg must sit on a CTCF...
    StalledOffCtcf { lef: usize, pos: Position },
    /// ...that it reached from the side opposite the capture side: a
    /// left-facing leg is stalled by a CTCF capturing from the left, a
    /// right-facing leg by one capturing from the right.
    StalledWrongSide { lef: usize, pos: Position, facing: Direction },
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorldError::ColumnLengths => write!(f, "LEF state columns have different lengths"),
            WorldError::PositionOutOfRange { lef, pos, n_sites } => {
                write!(f, "LEF {lef}: position {pos} is outside a lattice of {n_sites} sites")
            }
            WorldError::LegsCrossed { lef, left, right } => {
                write!(f, "LEF {lef}: left leg at {left} is to the right of the right leg at {right}")
            }
            WorldError::SiteOccupied { lef, pos } => {
                write!(f, "LEF {lef}: site {pos} is already occupied by another LEF")
            }
            WorldError::CapturedOffCtcf { lef, pos } => {
                write!(f, "LEF {lef}: leg at {pos} is Captured but the site is not a CTCF")
            }
            WorldError::CapturedWrongSide { lef, pos, facing } => write!(
                f,
                "LEF {lef}: a {}-facing leg can't be captured by the CTCF at {pos}, \
                 which captures from the {}",
                Direction::NAMES[facing.code() as usize].to_lowercase(),
                Direction::NAMES[facing.code() as usize].to_lowercase(),
            ),
            WorldError::StalledOffCtcf { lef, pos } => {
                write!(f, "LEF {lef}: leg at {pos} is Stalled but the site is not a CTCF")
            }
            WorldError::StalledWrongSide { lef, pos, facing } => write!(
                f,
                "LEF {lef}: a {}-facing leg can't be stalled by the CTCF at {pos}, \
                 which captures from the {}",
                Direction::NAMES[facing.code() as usize].to_lowercase(),
                Direction::NAMES[facing.opposite().code() as usize].to_lowercase(),
            ),
        }
    }
}

impl std::error::Error for WorldError {}

/// Per-site occupancy bucket. Multi-occupancy occurs transiently right after
/// a respawn (paired legs sharing the spawn site). Inline capacity 2 covers
/// that case without paying for a heap allocation.
pub type OccupancyBucket = SmallVec<[LegKey; 2]>;

/// The state of the system at a moment: a lattice and the LEFs on it.
///
/// Built with [`World::new`] plus [`World::add_lef`], or all at once with
/// [`World::from_lef_states`]; either way every LEF is validated, so a world
/// is always a state the dynamics could be in. Fields are crate-private so
/// that the derived occupancy index can't drift out of sync with leg
/// positions; read through the accessors.
///
/// Two worlds are equal when their lattices and [`LefStates`] are equal,
/// whatever their internal keys.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "super::serial::WorldData", into = "super::serial::WorldData")
)]
pub struct World {
    pub(crate) lattice: Lattice,
    pub(crate) legs: SlotMap<LegKey, Leg>,
    pub(crate) lefs: SlotMap<LefKey, Lef>,
    /// LEF keys in `lef_id` order (the order LEFs were added).
    pub(crate) lef_order: Vec<LefKey>,
    /// `occupancy[i]` lists the leg(s) currently sitting at site `i`.
    pub(crate) occupancy: Vec<OccupancyBucket>,
}

impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        self.lattice == other.lattice && self.lef_states() == other.lef_states()
    }
}

impl World {
    /// A world with no LEFs.
    pub fn new(lattice: Lattice) -> Self {
        let n = lattice.len();
        World {
            lattice,
            legs: SlotMap::with_key(),
            lefs: SlotMap::with_key(),
            lef_order: Vec::new(),
            occupancy: vec![OccupancyBucket::new(); n],
        }
    }

    /// A world with the LEFs in `lefs`, row `i` becoming LEF `i`.
    pub fn from_lef_states(lattice: Lattice, lefs: &LefStates) -> Result<Self, WorldError> {
        if !lefs.columns_agree() {
            return Err(WorldError::ColumnLengths);
        }
        let mut world = World::new(lattice);
        for i in 0..lefs.len() {
            let (left, right) = lefs.get(i).expect("columns agree");
            world.add_lef(left, right)?;
        }
        Ok(world)
    }

    /// Add a LEF with its left-facing leg in state `left` and its
    /// right-facing leg in state `right`. It becomes LEF `n_lefs()`.
    ///
    /// Mobility is taken as given (not rolled against the site), and the LEF
    /// is checked against the world: see [`WorldError`] for the rules.
    pub fn add_lef(&mut self, left: LegState, right: LegState) -> Result<LefKey, WorldError> {
        self.check_lef(left, right)?;
        Ok(self.insert_lef(left, right).0)
    }

    /// Number of LEFs.
    pub fn n_lefs(&self) -> usize {
        self.lef_order.len()
    }

    /// The left and right leg states of LEF `lef_id`.
    pub fn lef_state(&self, lef_id: usize) -> Option<(LegState, LegState)> {
        let [l, r] = self.lefs[*self.lef_order.get(lef_id)?].legs;
        let (l, r) = (&self.legs[l], &self.legs[r]);
        Some(((l.pos, l.mobility), (r.pos, r.mobility)))
    }

    /// The whole LEF table, in `lef_id` order.
    pub fn lef_states(&self) -> LefStates {
        let mut states = LefStates::with_capacity(self.n_lefs());
        self.push_lef_states(&mut states);
        states
    }

    /// Append the LEF table, in `lef_id` order, to `out`: how snapshots are
    /// recorded without allocating.
    pub fn push_lef_states(&self, out: &mut LefStates) {
        for &lef in &self.lef_order {
            let [l, r] = self.lefs[lef].legs;
            let (l, r) = (&self.legs[l], &self.legs[r]);
            out.push((l.pos, l.mobility), (r.pos, r.mobility));
        }
    }

    /// Check a would-be LEF against the lattice and the LEFs already here.
    fn check_lef(&self, left: LegState, right: LegState) -> Result<(), WorldError> {
        let lef = self.n_lefs();
        let n_sites = self.lattice.len();
        for (pos, _) in [left, right] {
            if pos as usize >= n_sites {
                return Err(WorldError::PositionOutOfRange { lef, pos, n_sites });
            }
        }
        if left.0 > right.0 {
            return Err(WorldError::LegsCrossed { lef, left: left.0, right: right.0 });
        }
        // Its own two legs may share a site; any other leg may not.
        for pos in [left.0, right.0] {
            if self.occupied(pos) {
                return Err(WorldError::SiteOccupied { lef, pos });
            }
        }
        for ((pos, mobility), facing) in [(left, Direction::Left), (right, Direction::Right)] {
            match mobility {
                Mobility::Moving => {}
                Mobility::Stalled => match self.lattice.sites[pos as usize].kind {
                    SiteKind::Plain => return Err(WorldError::StalledOffCtcf { lef, pos }),
                    SiteKind::Ctcf { captures_from, .. } if captures_from != facing => {
                        return Err(WorldError::StalledWrongSide { lef, pos, facing });
                    }
                    SiteKind::Ctcf { .. } => {}
                },
                Mobility::Captured => match self.lattice.sites[pos as usize].kind {
                    SiteKind::Plain => return Err(WorldError::CapturedOffCtcf { lef, pos }),
                    SiteKind::Ctcf { captures_from, .. } if captures_from != facing.opposite() => {
                        return Err(WorldError::CapturedWrongSide { lef, pos, facing });
                    }
                    SiteKind::Ctcf { .. } => {}
                },
            }
        }
        Ok(())
    }

    /// Create a LEF in the given state, unchecked. Callers either validated
    /// it or produced it from the dynamics.
    pub(crate) fn insert_lef(&mut self, left: LegState, right: LegState) -> (LefKey, [LegKey; 2]) {
        // Two-pass insert because each leg must reference the other.
        let l1 = self.legs.insert(Leg {
            pos: left.0,
            lef: LefKey::default(),
            partner: LegKey::default(),
            facing: Direction::Left,
            mobility: left.1,
        });
        let l2 = self.legs.insert(Leg {
            pos: right.0,
            lef: LefKey::default(),
            partner: l1,
            facing: Direction::Right,
            mobility: right.1,
        });
        let lef_k = self.lefs.insert(Lef { legs: [l1, l2] });
        let leg = &mut self.legs[l1];
        leg.lef = lef_k;
        leg.partner = l2;
        self.legs[l2].lef = lef_k;
        self.insert_leg(l1, left.0);
        self.insert_leg(l2, right.0);
        self.lef_order.push(lef_k);
        (lef_k, [l1, l2])
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
        let [left, right] = land_lef(self.lattice.sites[pos as usize].kind, rng);
        self.insert_lef((pos, left), (pos, right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Ctcf;
    use Mobility::{Captured, Moving, Stalled};

    /// 20 sites: a CTCF capturing from the left at 5 (captures right-facing
    /// legs), and one capturing from the right at 15 (captures left-facing).
    fn lattice() -> Lattice {
        Lattice::with_ctcfs(
            20,
            &[
                Ctcf { site: 5, captures_from: Direction::Left, strength: 1.0 },
                Ctcf { site: 15, captures_from: Direction::Right, strength: 1.0 },
            ],
        )
        .unwrap()
    }

    #[test]
    fn add_lef_builds_a_world_that_reads_back() {
        let mut world = World::new(lattice());
        world.add_lef((2, Moving), (5, Captured)).unwrap();
        world.add_lef((15, Captured), (18, Moving)).unwrap();
        world.add_lef((9, Moving), (9, Moving)).unwrap(); // a freshly placed LEF

        assert_eq!(world.n_lefs(), 3);
        assert_eq!(world.lef_state(1), Some(((15, Captured), (18, Moving))));
        assert_eq!(world.lef_state(3), None);
        assert!(world.occupied(9) && world.occupied(5) && !world.occupied(10));

        let states = world.lef_states();
        assert_eq!(states.left_pos, [2, 15, 9]);
        assert_eq!(states.right_mobility, [Captured, Moving, Moving]);
        let rebuilt = World::from_lef_states(lattice(), &states).unwrap();
        assert_eq!(rebuilt, world);
    }

    #[test]
    fn a_stalled_leg_sits_on_a_ctcf_it_reached_from_behind() {
        // The CTCF at 5 captures from the left: a left-facing leg reaching it
        // (from the right) is stalled; at 15 (captures from the right), a
        // right-facing leg is.
        let mut world = World::new(lattice());
        world.add_lef((5, Stalled), (8, Moving)).unwrap();
        world.add_lef((10, Moving), (15, Stalled)).unwrap();
        assert_eq!(world.lef_state(1), Some(((10, Moving), (15, Stalled))));
    }

    #[test]
    fn equality_compares_state_not_identity() {
        let mut a = World::new(lattice());
        a.add_lef((1, Moving), (3, Moving)).unwrap();
        let mut b = a.clone();
        assert_eq!(a, b);
        b.add_lef((10, Moving), (11, Moving)).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.n_lefs(), 1); // the clone is independent
        assert_ne!(World::new(lattice()), World::new(Lattice::plain(20).unwrap()));
    }

    #[test]
    fn rejects_invalid_lefs() {
        let mut world = World::new(lattice());
        world.add_lef((8, Moving), (9, Moving)).unwrap();
        let cases = [
            ((20, Moving), (20, Moving), WorldError::PositionOutOfRange { lef: 1, pos: 20, n_sites: 20 }),
            ((4, Moving), (3, Moving), WorldError::LegsCrossed { lef: 1, left: 4, right: 3 }),
            ((9, Moving), (12, Moving), WorldError::SiteOccupied { lef: 1, pos: 9 }),
            ((1, Stalled), (2, Moving), WorldError::StalledOffCtcf { lef: 1, pos: 1 }),
            // A left-facing leg isn't stalled by a CTCF capturing from the right (it captures it).
            ((15, Stalled), (16, Moving), WorldError::StalledWrongSide { lef: 1, pos: 15, facing: Direction::Left }),
            ((1, Captured), (2, Moving), WorldError::CapturedOffCtcf { lef: 1, pos: 1 }),
            // A left-facing leg can't be captured by a CTCF capturing from the left.
            ((5, Captured), (6, Moving), WorldError::CapturedWrongSide { lef: 1, pos: 5, facing: Direction::Left }),
            // A right-facing leg can't be captured by one capturing from the right.
            ((12, Moving), (15, Captured), WorldError::CapturedWrongSide { lef: 1, pos: 15, facing: Direction::Right }),
        ];
        for (left, right, expected) in cases {
            assert_eq!(world.add_lef(left, right), Err(expected.clone()), "{expected}");
        }
        assert_eq!(world.n_lefs(), 1, "rejected LEFs leave the world unchanged");
    }

    #[test]
    fn from_lef_states_checks_column_lengths_and_each_row() {
        let mut states = LefStates::new();
        states.push((1, Moving), (2, Moving));
        states.right_pos.push(3);
        assert_eq!(World::from_lef_states(lattice(), &states), Err(WorldError::ColumnLengths));

        let mut clash = LefStates::new();
        clash.push((1, Moving), (4, Moving));
        clash.push((4, Moving), (6, Moving));
        assert_eq!(
            World::from_lef_states(lattice(), &clash),
            Err(WorldError::SiteOccupied { lef: 1, pos: 4 })
        );
    }
}
