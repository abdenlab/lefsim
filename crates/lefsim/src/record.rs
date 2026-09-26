//! Observation: recording what a [`Sim`] does, as plain columnar buffers.
//!
//! Two recorders, both fed by the [`Sim`] batch drivers:
//!
//! - [`Recorder`] takes **snapshots** of every LEF on a regular
//!   simulation-time grid. It stores them wide (one row per LEF: left and
//!   right leg together) and rectangular: `time` has one entry per snapshot,
//!   and every other column has `n_lefs` entries per snapshot.
//! - [`EventRecorder`] logs **deltas**: one row per leg changed by each
//!   fired event. Lossless and compact; replay it over a base snapshot to
//!   reconstruct the state at any time.
//!
//! Buffers are plain `Vec`s — Arrow-shaped, but with no Arrow dependency.
//! Binding crates wrap them (zero-copy) as Arrow arrays or JS typed arrays.
//!
//! ## Grid semantics
//!
//! State is piecewise-constant between events, so grid snapshots are exact,
//! not interpolated. A grid point `g` records the state after *every* event
//! with time `<= g`. In particular, a chain of zero-delay events (an
//! eviction followed by its immediate respawn) is fully applied before a
//! coincident grid point is recorded, so snapshots never show the transient
//! `Evicted` state.
//!
//! Recording never touches the RNG: a run produces the same events whether
//! or not anything is observing it.

use crate::action::{Action, Rates};
use crate::engine::Time;
use crate::sim::Sim;
use crate::world::SiteKind;

// ===================================================================
// Grid snapshots
// ===================================================================

/// When a [`Recorder`] takes snapshots.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SampleMode {
    /// On the grid `t0 + k * dt` for `k = 0, 1, 2, ...`.
    EveryDt { dt: Time, t0: Time },
    /// Only when the caller asks, via [`Recorder::capture`].
    Manual,
}

impl SampleMode {
    /// Grid of spacing `dt` starting at `t = 0`.
    pub fn every_dt(dt: Time) -> Self {
        SampleMode::EveryDt { dt, t0: 0.0 }
    }
}

/// Snapshots of every LEF, stored wide and snapshot-major.
///
/// For snapshot `s` and LEF `i`, the per-LEF columns hold the value at index
/// `s * n_lefs + i`, so each column reshapes to an `(n_snapshots, n_lefs)`
/// array without copying. Mobility columns hold [`Mobility::code`](crate::Mobility::code) values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LefFrames {
    n_lefs: usize,
    time: Vec<f64>,
    left_pos: Vec<u32>,
    right_pos: Vec<u32>,
    left_mob: Vec<u8>,
    right_mob: Vec<u8>,
}

/// Owned columns of a [`LefFrames`], for handing buffers to Arrow or JS
/// without copying.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LefColumns {
    pub n_lefs: usize,
    pub time: Vec<f64>,
    pub left_pos: Vec<u32>,
    pub right_pos: Vec<u32>,
    pub left_mob: Vec<u8>,
    pub right_mob: Vec<u8>,
}

/// One snapshot borrowed from a [`LefFrames`]. Slices are indexed by `lef_id`.
#[derive(Copy, Clone, Debug)]
pub struct LefSnapshot<'a> {
    pub time: f64,
    pub left_pos: &'a [u32],
    pub right_pos: &'a [u32],
    pub left_mob: &'a [u8],
    pub right_mob: &'a [u8],
}

impl LefFrames {
    pub fn new(n_lefs: usize) -> Self {
        LefFrames { n_lefs, ..Default::default() }
    }

    /// Empty frames with room for `n_snapshots` snapshots.
    pub fn with_capacity(n_lefs: usize, n_snapshots: usize) -> Self {
        let rows = n_lefs * n_snapshots;
        LefFrames {
            n_lefs,
            time: Vec::with_capacity(n_snapshots),
            left_pos: Vec::with_capacity(rows),
            right_pos: Vec::with_capacity(rows),
            left_mob: Vec::with_capacity(rows),
            right_mob: Vec::with_capacity(rows),
        }
    }

    /// Ensure room for `n_snapshots` more snapshots.
    pub fn reserve(&mut self, n_snapshots: usize) {
        let rows = self.n_lefs * n_snapshots;
        self.time.reserve(n_snapshots);
        self.left_pos.reserve(rows);
        self.right_pos.reserve(rows);
        self.left_mob.reserve(rows);
        self.right_mob.reserve(rows);
    }

    pub fn n_lefs(&self) -> usize {
        self.n_lefs
    }

    pub fn n_snapshots(&self) -> usize {
        self.time.len()
    }

    pub fn is_empty(&self) -> bool {
        self.time.is_empty()
    }

    pub fn time(&self) -> &[f64] {
        &self.time
    }

    pub fn left_pos(&self) -> &[u32] {
        &self.left_pos
    }

    pub fn right_pos(&self) -> &[u32] {
        &self.right_pos
    }

    pub fn left_mob(&self) -> &[u8] {
        &self.left_mob
    }

    pub fn right_mob(&self) -> &[u8] {
        &self.right_mob
    }

    /// Snapshot `s`. Panics if `s >= n_snapshots()`.
    pub fn snapshot(&self, s: usize) -> LefSnapshot<'_> {
        let rows = s * self.n_lefs..(s + 1) * self.n_lefs;
        LefSnapshot {
            time: self.time[s],
            left_pos: &self.left_pos[rows.clone()],
            right_pos: &self.right_pos[rows.clone()],
            left_mob: &self.left_mob[rows.clone()],
            right_mob: &self.right_mob[rows],
        }
    }

    /// Append all snapshots of `other`. Panics if `n_lefs` differs.
    pub fn extend_from(&mut self, other: &LefFrames) {
        assert_eq!(self.n_lefs, other.n_lefs, "n_lefs mismatch");
        self.time.extend_from_slice(&other.time);
        self.left_pos.extend_from_slice(&other.left_pos);
        self.right_pos.extend_from_slice(&other.right_pos);
        self.left_mob.extend_from_slice(&other.left_mob);
        self.right_mob.extend_from_slice(&other.right_mob);
    }

    /// Drop all snapshots, keeping allocated capacity.
    pub fn clear(&mut self) {
        self.time.clear();
        self.left_pos.clear();
        self.right_pos.clear();
        self.left_mob.clear();
        self.right_mob.clear();
    }

    pub fn into_columns(self) -> LefColumns {
        LefColumns {
            n_lefs: self.n_lefs,
            time: self.time,
            left_pos: self.left_pos,
            right_pos: self.right_pos,
            left_mob: self.left_mob,
            right_mob: self.right_mob,
        }
    }

    fn push_snapshot(&mut self, time: Time, sim: &Sim) {
        assert_eq!(self.n_lefs, sim.n_lefs(), "recorder built for a different sim");
        self.time.push(time);
        for lef_id in 0..self.n_lefs as u32 {
            let [(lp, lm), (rp, rm)] = sim.lef_state(lef_id);
            self.left_pos.push(lp);
            self.right_pos.push(rp);
            self.left_mob.push(lm.code());
            self.right_mob.push(rm.code());
        }
    }
}

/// Takes grid snapshots of a [`Sim`] according to a [`SampleMode`].
///
/// A recorder is separate from the `Sim` it observes and is passed to the
/// `*_with` drivers (e.g. [`Sim::run_until_with`]). It keeps its own grid
/// cursor, so it can be reused across calls and several recorders can sample
/// one run at different spacings.
///
/// With [`Recorder::with_max_snapshots`], a full buffer stops the driver with
/// [`StopReason::RecorderFull`](crate::StopReason::RecorderFull); drain it and
/// call the driver again to continue. This is how output is streamed.
#[derive(Clone, Debug)]
pub struct Recorder {
    mode: SampleMode,
    frames: LefFrames,
    /// Index of the next grid point to record (`EveryDt` only).
    next_k: u64,
    max_snapshots: Option<usize>,
}

impl Recorder {
    /// Panics if `mode` is `EveryDt` with a non-positive or non-finite `dt`,
    /// or a non-finite `t0`.
    pub fn new(mode: SampleMode, n_lefs: usize) -> Self {
        if let SampleMode::EveryDt { dt, t0 } = mode {
            assert!(dt.is_finite() && dt > 0.0, "dt must be positive and finite, got {dt}");
            assert!(t0.is_finite(), "t0 must be finite, got {t0}");
        }
        Recorder {
            mode,
            frames: LefFrames::new(n_lefs),
            next_k: 0,
            max_snapshots: None,
        }
    }

    /// Stop the driver once this many snapshots are buffered.
    ///
    /// The buffer is preallocated to exactly this size (and reallocated at
    /// that size after each [`Recorder::take_frames`]), so streaming a long
    /// run allocates the same few buffer sizes over and over instead of
    /// growing each chunk by repeated doubling. Choose a chunk-sized bound,
    /// not a huge safety cap.
    pub fn with_max_snapshots(mut self, n: usize) -> Self {
        self.set_max_snapshots(Some(n));
        self
    }

    /// Change the bound in place (`None` removes it), keeping the grid
    /// cursor, so one recorder can serve both bounded (chunked) and
    /// unbounded calls without repeating or skipping grid points.
    pub fn set_max_snapshots(&mut self, max: Option<usize>) {
        if let Some(n) = max {
            assert!(n > 0, "max_snapshots must be positive");
            self.frames.reserve(n.saturating_sub(self.frames.n_snapshots()));
        }
        self.max_snapshots = max;
    }

    pub fn mode(&self) -> SampleMode {
        self.mode
    }

    pub fn frames(&self) -> &LefFrames {
        &self.frames
    }

    pub fn is_full(&self) -> bool {
        self.max_snapshots
            .is_some_and(|max| self.frames.n_snapshots() >= max)
    }

    /// Take the buffered snapshots, leaving the recorder empty.
    pub fn take_frames(&mut self) -> LefFrames {
        let n_lefs = self.frames.n_lefs;
        let empty = match self.max_snapshots {
            Some(max) => LefFrames::with_capacity(n_lefs, max),
            None => LefFrames::new(n_lefs),
        };
        std::mem::replace(&mut self.frames, empty)
    }

    /// Move the buffered snapshots into `out` (replacing its contents) and
    /// reuse `out`'s allocation for further recording. `out` may be any
    /// `LefFrames`, e.g. `LefFrames::default()`.
    pub fn drain_into(&mut self, out: &mut LefFrames) {
        out.clear();
        out.n_lefs = self.frames.n_lefs;
        std::mem::swap(out, &mut self.frames);
        if let Some(max) = self.max_snapshots {
            self.frames.reserve(max);
        }
    }

    /// Record the current state at the current time, regardless of mode.
    /// Returns `false` (and records nothing) if the buffer is full.
    pub fn capture(&mut self, sim: &Sim) -> bool {
        if self.is_full() {
            return false;
        }
        self.frames.push_snapshot(sim.time(), sim);
        true
    }

    fn grid_point(&self, k: u64) -> Option<Time> {
        match self.mode {
            SampleMode::EveryDt { dt, t0 } => Some(t0 + k as f64 * dt),
            SampleMode::Manual => None,
        }
    }
}

// ===================================================================
// Event deltas
// ===================================================================

/// Per-event deltas: one row per leg changed by a fired event.
///
/// A `Step` produces one row, a `Respawn` two (one per leg, sharing a
/// `time`). With keyframes enabled (see [`EventRecorder::with_keyframes`]),
/// each batch also opens with one `Keyframe` row per leg.
///
/// `kind` holds codes into [`EventFrames::KIND_NAMES`] (for events these
/// equal [`Action::code`]); `mob` holds [`Mobility::code`](crate::Mobility::code).
/// Rows carry absolute values, so replaying them is an idempotent overwrite.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventFrames {
    time: Vec<f64>,
    leg_id: Vec<u32>,
    pos: Vec<u32>,
    mob: Vec<u8>,
    kind: Vec<u8>,
}

/// Owned columns of an [`EventFrames`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventColumns {
    pub time: Vec<f64>,
    pub leg_id: Vec<u32>,
    pub pos: Vec<u32>,
    pub mob: Vec<u8>,
    pub kind: Vec<u8>,
}

impl EventFrames {
    /// Row-kind names, indexed by the codes in the `kind` column. The first
    /// two are the [`Action`] variants; `Keyframe` is not an action.
    pub const KIND_NAMES: [&str; 3] = ["Step", "Respawn", "Keyframe"];

    /// The `kind` code of keyframe rows.
    pub const KEYFRAME: u8 = 2;

    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure room for `n_rows` more rows.
    pub fn reserve(&mut self, n_rows: usize) {
        self.time.reserve(n_rows);
        self.leg_id.reserve(n_rows);
        self.pos.reserve(n_rows);
        self.mob.reserve(n_rows);
        self.kind.reserve(n_rows);
    }

    /// Empty frames with room for `n_rows` rows.
    pub fn with_capacity(n_rows: usize) -> Self {
        EventFrames {
            time: Vec::with_capacity(n_rows),
            leg_id: Vec::with_capacity(n_rows),
            pos: Vec::with_capacity(n_rows),
            mob: Vec::with_capacity(n_rows),
            kind: Vec::with_capacity(n_rows),
        }
    }

    pub fn n_rows(&self) -> usize {
        self.time.len()
    }

    pub fn is_empty(&self) -> bool {
        self.time.is_empty()
    }

    pub fn time(&self) -> &[f64] {
        &self.time
    }

    pub fn leg_id(&self) -> &[u32] {
        &self.leg_id
    }

    pub fn pos(&self) -> &[u32] {
        &self.pos
    }

    pub fn mob(&self) -> &[u8] {
        &self.mob
    }

    pub fn kind(&self) -> &[u8] {
        &self.kind
    }

    /// Append all rows of `other`.
    pub fn extend_from(&mut self, other: &EventFrames) {
        self.time.extend_from_slice(&other.time);
        self.leg_id.extend_from_slice(&other.leg_id);
        self.pos.extend_from_slice(&other.pos);
        self.mob.extend_from_slice(&other.mob);
        self.kind.extend_from_slice(&other.kind);
    }

    /// Drop all rows, keeping allocated capacity.
    pub fn clear(&mut self) {
        self.time.clear();
        self.leg_id.clear();
        self.pos.clear();
        self.mob.clear();
        self.kind.clear();
    }

    pub fn into_columns(self) -> EventColumns {
        EventColumns {
            time: self.time,
            leg_id: self.leg_id,
            pos: self.pos,
            mob: self.mob,
            kind: self.kind,
        }
    }

    fn push_event(&mut self, time: Time, action: Action, sim: &Sim) {
        let legs = match action {
            Action::Step { leg } => [Some(leg), None],
            Action::Respawn { lef } => sim.world().lefs[lef].legs.map(Some),
        };
        for leg in legs.into_iter().flatten() {
            let state = &sim.world().legs[leg];
            self.time.push(time);
            self.leg_id.push(sim.leg_id(leg).expect("leg belongs to this sim"));
            self.pos.push(state.pos);
            self.mob.push(state.mobility.code());
            self.kind.push(action.code());
        }
    }

    /// One `Keyframe` row per leg, in `leg_id` order, with its current state.
    fn push_keyframe(&mut self, time: Time, sim: &Sim, n_legs: usize) {
        assert_eq!(n_legs, sim.n_legs(), "event recorder built for a different sim");
        for (id, &leg) in sim.leg_keys().iter().enumerate() {
            let state = &sim.world().legs[leg];
            self.time.push(time);
            self.leg_id.push(id as u32);
            self.pos.push(state.pos);
            self.mob.push(state.mobility.code());
            self.kind.push(Self::KEYFRAME);
        }
    }
}

/// Logs [`EventFrames`] from the [`Sim`] drivers.
///
/// Deltas alone don't say where legs started. Either pair them with a base
/// snapshot (e.g. a [`Recorder`] snapshot at the start time), or turn on
/// [`EventRecorder::with_keyframes`] so every batch carries its own.
///
/// With [`EventRecorder::with_max_rows`], the driver stops with
/// [`StopReason::RecorderFull`](crate::StopReason::RecorderFull) *before*
/// firing an event that might not fit, so no event is ever dropped.
#[derive(Clone, Debug)]
pub struct EventRecorder {
    frames: EventFrames,
    max_rows: Option<usize>,
    /// `Some(n_legs)` when keyframes are on.
    keyframe_legs: Option<usize>,
    /// Whether the sim's time was settled (no event pending at exactly the
    /// current time) at the last `sample` call.
    settled: bool,
}

impl Default for EventRecorder {
    fn default() -> Self {
        EventRecorder { frames: EventFrames::new(), max_rows: None, keyframe_legs: None, settled: true }
    }
}

impl EventRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop the driver before the buffer could exceed `n` rows. Must be at
    /// least 2, the most rows one event can produce.
    ///
    /// Like [`Recorder::with_max_snapshots`], the buffer is preallocated to
    /// exactly this size, so choose a chunk-sized bound.
    pub fn with_max_rows(mut self, n: usize) -> Self {
        assert!(n >= 2, "max_rows must be at least 2");
        if let Some(n_legs) = self.keyframe_legs {
            assert!(n >= n_legs + 2, "max_rows must be at least n_legs + 2 with keyframes");
        }
        self.max_rows = Some(n);
        self.frames.reserve(n.saturating_sub(self.frames.n_rows()));
        self
    }

    /// Open every batch with a keyframe: one `Keyframe` row per leg holding
    /// the full state, so each batch can be replayed on its own.
    ///
    /// A keyframe is written whenever the buffer is empty (at the start, and
    /// after each [`EventRecorder::take_frames`] / [`EventRecorder::drain_into`]),
    /// stamped with the current time, and only once that time is *settled*.
    /// During a zero-delay chain (an eviction and its immediate respawn),
    /// it waits for the chain to finish, so keyframes never show the
    /// transient `Evicted` state. Events fired before the first keyframe
    /// aren't logged: the keyframe already reflects them.
    ///
    /// Batches also only end at settled times, so the next one can open with
    /// a settled keyframe. A bounded buffer may therefore run past its cap by
    /// the rows of a zero-delay chain in progress (typically two).
    ///
    /// `n_legs` is the sim's leg count; a bounded buffer must hold at least
    /// a keyframe plus one event (`max_rows >= n_legs + 2`).
    pub fn with_keyframes(mut self, n_legs: usize) -> Self {
        if let Some(max) = self.max_rows {
            assert!(max >= n_legs + 2, "max_rows must be at least n_legs + 2 with keyframes");
        }
        self.keyframe_legs = Some(n_legs);
        self
    }

    /// Whether the next rows written will be a keyframe.
    fn keyframe_pending(&self) -> bool {
        self.keyframe_legs.is_some() && self.frames.is_empty()
    }

    pub fn frames(&self) -> &EventFrames {
        &self.frames
    }

    pub fn take_frames(&mut self) -> EventFrames {
        let empty = match self.max_rows {
            Some(max) => EventFrames::with_capacity(max),
            None => EventFrames::new(),
        };
        std::mem::replace(&mut self.frames, empty)
    }

    /// Move the buffered rows into `out` (replacing its contents) and reuse
    /// `out`'s allocation for further recording.
    pub fn drain_into(&mut self, out: &mut EventFrames) {
        out.clear();
        std::mem::swap(out, &mut self.frames);
        if let Some(max) = self.max_rows {
            self.frames.reserve(max);
        }
    }
}

// ===================================================================
// Observer plumbing
// ===================================================================

/// Something the [`Sim`] drivers can feed: a [`Recorder`], an
/// [`EventRecorder`], `()` for nothing, a pair of observers such as
/// `(&mut recorder, &mut events)`, an `Option` of an observer (`None`
/// observes nothing), or a `&mut` to any of these.
///
/// This trait is sealed; it can't be implemented outside this crate.
pub trait Observer: sealed::Observe {}

impl<T: sealed::Observe + ?Sized> Observer for T {}

pub(crate) mod sealed {
    use super::*;

    /// The driver-facing hooks behind [`Observer`](super::Observer).
    pub trait Observe {
        /// Record everything due at times `< bound` (or `<= bound` if
        /// `inclusive`). The sim's state is known to be constant from now
        /// through `bound`. Returns `false` if the buffer filled up before
        /// everything due was recorded.
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool;

        /// Whether the next fired event can be recorded without overflow.
        fn has_room_for_event(&self) -> bool;

        /// Called right after an event fires.
        fn on_event(&mut self, time: Time, action: Action, sim: &Sim);
    }

    impl Observe for () {
        fn sample(&mut self, _: Time, _: bool, _: &Sim) -> bool {
            true
        }
        fn has_room_for_event(&self) -> bool {
            true
        }
        fn on_event(&mut self, _: Time, _: Action, _: &Sim) {}
    }

    impl Observe for Recorder {
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool {
            let now = sim.time();
            while let Some(g) = self.grid_point(self.next_k) {
                let due = if inclusive { g <= bound } else { g < bound };
                if !due {
                    break;
                }
                // Grid points before `now` belong to a stretch of the run
                // this recorder didn't observe; their state is gone.
                if g >= now {
                    if self.is_full() {
                        return false;
                    }
                    self.frames.push_snapshot(g, sim);
                }
                self.next_k += 1;
            }
            true
        }

        fn has_room_for_event(&self) -> bool {
            true
        }

        fn on_event(&mut self, _: Time, _: Action, _: &Sim) {}
    }

    impl Observe for EventRecorder {
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool {
            let now = sim.time();
            // State at `now` is final unless another event is due at `now`.
            self.settled = inclusive || bound > now;
            if let Some(n_legs) = self.keyframe_legs
                && self.settled
                && self.frames.is_empty()
            {
                self.frames.push_keyframe(now, sim, n_legs);
            }
            true
        }

        fn has_room_for_event(&self) -> bool {
            let Some(max) = self.max_rows else {
                return true;
            };
            // With keyframes, only end a batch at a settled time, so the next
            // one opens with a settled keyframe.
            if self.keyframe_legs.is_some() && (self.frames.is_empty() || !self.settled) {
                return true;
            }
            self.frames.n_rows() + 2 <= max
        }

        fn on_event(&mut self, time: Time, action: Action, sim: &Sim) {
            // Until a keyframe opens the batch, it will capture this event.
            if self.keyframe_pending() {
                return;
            }
            self.frames.push_event(time, action, sim);
        }
    }

    impl<A: Observe, B: Observe> Observe for (A, B) {
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool {
            // Sample both even if the first fills up; each keeps its own cursor.
            let a = self.0.sample(bound, inclusive, sim);
            let b = self.1.sample(bound, inclusive, sim);
            a && b
        }
        fn has_room_for_event(&self) -> bool {
            self.0.has_room_for_event() && self.1.has_room_for_event()
        }
        fn on_event(&mut self, time: Time, action: Action, sim: &Sim) {
            self.0.on_event(time, action, sim);
            self.1.on_event(time, action, sim);
        }
    }

    impl<T: Observe> Observe for Option<T> {
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool {
            self.as_mut().is_none_or(|o| o.sample(bound, inclusive, sim))
        }
        fn has_room_for_event(&self) -> bool {
            self.as_ref().is_none_or(|o| o.has_room_for_event())
        }
        fn on_event(&mut self, time: Time, action: Action, sim: &Sim) {
            if let Some(o) = self {
                o.on_event(time, action, sim);
            }
        }
    }

    impl<T: Observe + ?Sized> Observe for &mut T {
        fn sample(&mut self, bound: Time, inclusive: bool, sim: &Sim) -> bool {
            (**self).sample(bound, inclusive, sim)
        }
        fn has_room_for_event(&self) -> bool {
            (**self).has_room_for_event()
        }
        fn on_event(&mut self, time: Time, action: Action, sim: &Sim) {
            (**self).on_event(time, action, sim)
        }
    }
}

// ===================================================================
// Static metadata
// ===================================================================

/// Everything about a run that never changes, as columnar tables. Build once
/// with [`Sim::meta`] and ship alongside the frames.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMeta {
    pub rates: Rates,
    pub lattice: LatticeTable,
    pub ctcfs: CtcfTable,
    pub lefs: LefTable,
    pub legs: LegTable,
}

/// One row per site; row index is the site's `Position`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LatticeTable {
    /// [`SiteKind::code`] per site.
    pub kind: Vec<u8>,
}

/// One row per CTCF site, in position order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CtcfTable {
    pub site: Vec<u32>,
    /// [`Direction::code`](crate::Direction::code) of the CTCF's capture side.
    pub captures_from: Vec<u8>,
    pub strength: Vec<f64>,
}

/// One row per LEF; row index is `lef_id`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LefTable {
    pub left_leg_id: Vec<u32>,
    pub right_leg_id: Vec<u32>,
}

/// One row per leg; row index is `leg_id`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LegTable {
    pub lef_id: Vec<u32>,
    /// [`Direction::code`](crate::Direction::code) of the leg's facing.
    pub facing: Vec<u8>,
}

impl SimMeta {
    pub(crate) fn from_sim(sim: &Sim) -> Self {
        let world = sim.world();
        let mut lattice = LatticeTable::default();
        let mut ctcfs = CtcfTable::default();
        for (pos, site) in world.lattice.sites.iter().enumerate() {
            lattice.kind.push(site.kind.code());
            if let SiteKind::Ctcf { captures_from, strength } = site.kind {
                ctcfs.site.push(pos as u32);
                ctcfs.captures_from.push(captures_from.code());
                ctcfs.strength.push(strength);
            }
        }

        let mut lefs = LefTable::default();
        for &lef in sim.lef_keys() {
            let [left, right] = world.lefs[lef].legs;
            lefs.left_leg_id.push(sim.leg_id(left).expect("leg belongs to this sim"));
            lefs.right_leg_id.push(sim.leg_id(right).expect("leg belongs to this sim"));
        }
        let mut legs = LegTable::default();
        for &leg in sim.leg_keys() {
            let state = &world.legs[leg];
            legs.lef_id.push(sim.lef_id(state.lef).expect("lef belongs to this sim"));
            legs.facing.push(state.facing.code());
        }

        SimMeta { rates: sim.rates(), lattice, ctcfs, lefs, legs }
    }
}
