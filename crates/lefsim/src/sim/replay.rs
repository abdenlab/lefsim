//! Replaying a keyframed event log: the state at any recorded time, with no
//! simulation.
//!
//! The Rust twin of the TS `Replayer` in `packages/lefsim`, with the same
//! semantics.

use crate::model::{Lattice, LefStates, Mobility, Time, World, WorldError};

use super::record::{EventFrames, LefFrames, SampleMode};

/// Where a keyframe starts: chunk index, row index, and its time.
#[derive(Copy, Clone, Debug)]
struct KeyframeRef {
    chunk: usize,
    row: usize,
    time: Time,
}

/// Rebuilds the state of a run from its keyframed event log (see
/// [`EventRecorder::with_keyframes`](crate::EventRecorder::with_keyframes)).
///
/// Feed chunks with [`Replayer::push`], each starting where the last one
/// ended, then move through time with [`Replayer::advance_to`] (forward,
/// cheap) or [`Replayer::seek`] (any direction: it restarts from the last
/// keyframe at or before the target, so its cost is bounded by chunk size).
///
/// Rows before the first keyframe can't be decoded and are skipped, so a
/// replayer can start partway into a stream (a late joiner).
///
/// The state at time `t` is the state after every row with time `<= t`,
/// exactly what [`Sim::run_until`](crate::Sim::run_until)`(t)` leaves.
/// [`Replayer::world`] returns it as a [`World`], to start a new run from:
/// a statistically faithful continuation (the dynamics are memoryless),
/// though not a bit-identical one, which would need the original RNG and
/// event queue.
#[derive(Clone, Debug)]
pub struct Replayer {
    lattice: Lattice,
    n_lefs: usize,
    chunks: Vec<EventFrames>,
    keyframes: Vec<KeyframeRef>,
    /// Next row to apply: `(chunk, row)`.
    cursor: (usize, usize),
    time: Time,
    end: Time,
    ready: bool,
    /// The LEF table at `time`.
    lefs: LefStates,
    /// The grid of the last `snapshots` call and its next index, so the
    /// next call on the same grid continues it. Cleared by a seek.
    grid: Option<(SampleMode, u64)>,
}

/// Why a chunk can't be pushed, or a time or state can't be reached.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayError {
    /// Rows must be in time order, within and across chunks.
    OutOfOrder { time: Time, previous: Time },
    /// A chunk's `end` must be a number no earlier than its last row.
    BadEnd { end: Time, last_row: Time },
    LegOutOfRange { leg_id: u32, n_legs: usize },
    BadKind { code: u8 },
    /// A keyframe has one row per leg, in `leg_id` order, at one time.
    BadKeyframe { time: Time, rows: usize, n_legs: usize },
    /// No keyframe at or before the requested time.
    NoKeyframe { t: Time, earliest: Option<Time> },
    /// Nothing to read yet: no keyframe has been applied.
    NotReady,
    /// The replayed state isn't a valid world (the log doesn't match the
    /// lattice, or it was cut mid-chain).
    World(WorldError),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::OutOfOrder { time, previous } => {
                write!(f, "row at t={time} comes after a row at t={previous}")
            }
            ReplayError::BadEnd { end, last_row } => {
                write!(f, "chunk end {end} is before its last row at t={last_row}")
            }
            ReplayError::LegOutOfRange { leg_id, n_legs } => {
                write!(f, "leg id {leg_id} is out of range for {n_legs} legs")
            }
            ReplayError::BadKind { code } => write!(f, "invalid row kind code {code}"),
            ReplayError::BadKeyframe { time, rows, n_legs } => write!(
                f,
                "keyframe at t={time} has {rows} rows; expected {n_legs}, one per leg in order"
            ),
            ReplayError::NoKeyframe { t, earliest: Some(e) } => {
                write!(f, "no keyframe at or before t={t} (the earliest is at t={e})")
            }
            ReplayError::NoKeyframe { t, earliest: None } => {
                write!(f, "no keyframe at or before t={t} (none pushed yet)")
            }
            ReplayError::NotReady => {
                write!(f, "no keyframe applied yet: push a keyframed chunk and advance")
            }
            ReplayError::World(e) => write!(f, "replayed state is invalid: {e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl Replayer {
    /// A replayer for a run on `lattice` with `n_lefs` LEFs.
    pub fn new(lattice: Lattice, n_lefs: usize) -> Self {
        Replayer {
            lattice,
            n_lefs,
            chunks: Vec::new(),
            keyframes: Vec::new(),
            cursor: (0, 0),
            time: Time::NEG_INFINITY,
            end: Time::NEG_INFINITY,
            ready: false,
            lefs: LefStates {
                left_pos: vec![0; n_lefs],
                right_pos: vec![0; n_lefs],
                left_mobility: vec![Mobility::Moving; n_lefs],
                right_mobility: vec![Mobility::Moving; n_lefs],
            },
            grid: None,
        }
    }

    pub fn lattice(&self) -> &Lattice {
        &self.lattice
    }

    pub fn n_lefs(&self) -> usize {
        self.n_lefs
    }

    /// Whether a keyframe has been applied, i.e. whether there's a state.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Current replay time (`-inf` until ready).
    pub fn time(&self) -> Time {
        self.time
    }

    /// Earliest time that can be sought to (the first keyframe), if any.
    pub fn start(&self) -> Option<Time> {
        self.keyframes.first().map(|k| k.time)
    }

    /// Latest time the pushed chunks describe; replay can't go past it.
    pub fn end(&self) -> Time {
        self.end
    }

    /// Append a chunk of rows, which describes the run through `end` (the
    /// clock when it was recorded; the `time` of the driver's report). Its
    /// rows must not precede rows already pushed. Checks every row; on
    /// error, nothing is pushed.
    pub fn push(&mut self, chunk: EventFrames, end: Time) -> Result<(), ReplayError> {
        let n_legs = 2 * self.n_lefs;
        let (time, leg_id, kind) = (chunk.time(), chunk.leg_id(), chunk.kind());
        let mut previous = self.last_row_time();
        for row in 0..chunk.n_rows() {
            if time[row].is_nan() || time[row] < previous {
                return Err(ReplayError::OutOfOrder { time: time[row], previous });
            }
            previous = time[row];
            if leg_id[row] as usize >= n_legs {
                return Err(ReplayError::LegOutOfRange { leg_id: leg_id[row], n_legs });
            }
            if kind[row] as usize >= EventFrames::KIND_NAMES.len() {
                return Err(ReplayError::BadKind { code: kind[row] });
            }
        }
        if end.is_nan() || end < previous {
            return Err(ReplayError::BadEnd { end, last_row: previous });
        }

        let index = self.chunks.len();
        let mut found = Vec::new();
        let mut row = 0;
        while row < chunk.n_rows() {
            if kind[row] != EventFrames::KEYFRAME {
                row += 1;
                continue;
            }
            let start = row;
            while row < chunk.n_rows()
                && kind[row] == EventFrames::KEYFRAME
                && time[row] == time[start]
                && leg_id[row] as usize == row - start
            {
                row += 1;
            }
            if row - start != n_legs {
                let rows = row - start;
                return Err(ReplayError::BadKeyframe { time: time[start], rows, n_legs });
            }
            found.push(KeyframeRef { chunk: index, row: start, time: time[start] });
        }
        self.keyframes.extend(found);
        self.chunks.push(chunk);
        self.end = self.end.max(end);
        Ok(())
    }

    /// Move to time `t`, clamped to [`Replayer::end`]. Going forward applies
    /// the rows in between, or restarts from a keyframe in between if there
    /// is one (so any jump costs at most one chunk); going backward seeks.
    /// Unlike [`Replayer::seek`] it never fails, which suits a playback
    /// loop: it waits before the first keyframe and at the end of the data.
    /// Returns the time
    /// reached: less than `t` when the data runs out (while buffering), and
    /// unchanged before the first keyframe. Panics if `t` is NaN.
    pub fn advance_to(&mut self, t: Time) -> Time {
        assert!(!t.is_nan(), "advance_to: t is NaN");
        if !self.ready {
            return match self.start() {
                Some(first) if t >= first => self.seek(t).expect("a keyframe precedes t"),
                _ => self.time,
            };
        }
        if t < self.time {
            return self.seek(t).expect("the current time follows a keyframe");
        }
        let target = t.min(self.end);
        // A keyframe between here and the target holds the full state, so
        // restarting from it skips every row before it: a long jump costs
        // at most one chunk, as a seek does. Positions, not times, decide
        // "ahead", since a keyframe can share its time with the rows before it.
        if let Some(k) = self.keyframe_at_or_before(target)
            && (k.chunk, k.row) > self.cursor
        {
            self.cursor = (k.chunk, k.row);
        }
        self.apply_through(target);
        self.time = self.time.max(target);
        self.time
    }

    /// Jump to time `t` (clamped to [`Replayer::end`]) in either direction:
    /// restore the last keyframe at or before it, then replay forward.
    /// Panics if `t` is NaN.
    pub fn seek(&mut self, t: Time) -> Result<Time, ReplayError> {
        assert!(!t.is_nan(), "seek: t is NaN");
        let target = t.min(self.end);
        let k = self
            .keyframe_at_or_before(target)
            .ok_or(ReplayError::NoKeyframe { t, earliest: self.start() })?;
        self.cursor = (k.chunk, k.row);
        self.apply_through(target);
        self.ready = true;
        self.time = target;
        self.grid = None;
        Ok(target)
    }

    /// The LEF table at [`Replayer::time`].
    pub fn lef_states(&self) -> Result<&LefStates, ReplayError> {
        if !self.ready {
            return Err(ReplayError::NotReady);
        }
        Ok(&self.lefs)
    }

    /// The state at [`Replayer::time`] as a [`World`]: start a new run from
    /// it with [`Sim::with_start_time`](crate::Sim::with_start_time).
    pub fn world(&self) -> Result<World, ReplayError> {
        World::from_lef_states(self.lattice.clone(), self.lef_states()?)
            .map_err(ReplayError::World)
    }

    /// Snapshots on the grid `t0 + k * dt` from the current time (or the
    /// first keyframe, if not ready yet) through `t_end`, clamped to
    /// [`Replayer::end`]: the same frames a [`Recorder`](crate::Recorder)
    /// on that grid records during the run. Leaves the replayer at the last
    /// grid point.
    ///
    /// Like a `Recorder`, consecutive calls on the same grid continue it
    /// without repeating a point; a seek starts afresh. Panics unless `mode`
    /// is `EveryDt` with a positive, finite `dt` and a finite `t0`.
    pub fn snapshots(&mut self, mode: SampleMode, t_end: Time) -> LefFrames {
        let SampleMode::EveryDt { dt, t0 } = mode else {
            panic!("snapshots need a grid (SampleMode::EveryDt)");
        };
        assert!(dt.is_finite() && dt > 0.0, "dt must be positive and finite, got {dt}");
        assert!(t0.is_finite(), "t0 must be finite, got {t0}");
        assert!(!t_end.is_nan(), "snapshots: t_end is NaN");

        let mut frames = LefFrames::new(self.n_lefs);
        let from = if self.ready { Some(self.time) } else { self.start() };
        let Some(from) = from else { return frames };
        let point = |k: u64| t0 + k as f64 * dt; // exactly as the Recorder computes it
        // The first index at or after `from`, but never one already recorded.
        let mut k = ((from - t0) / dt).ceil().max(0.0) as u64;
        while k > 0 && point(k - 1) >= from {
            k -= 1;
        }
        while point(k) < from {
            k += 1;
        }
        if let Some((m, next)) = self.grid
            && m == mode
        {
            k = k.max(next);
        }
        let last = t_end.min(self.end);
        while point(k) <= last {
            self.advance_to(point(k));
            frames.push(point(k), &self.lefs);
            k += 1;
        }
        self.grid = Some((mode, k));
        frames
    }

    /// Drop chunks no longer needed to seek to `t` or later, to bound memory
    /// in a long stream. Keeps everything from the last keyframe at or
    /// before `t`, and the current position.
    pub fn trim_before(&mut self, t: Time) {
        let Some(k) = self.keyframe_at_or_before(t) else { return };
        let drop = k.chunk.min(self.cursor.0);
        if drop == 0 {
            return;
        }
        self.chunks.drain(..drop);
        self.keyframes.retain(|r| r.chunk >= drop);
        for r in &mut self.keyframes {
            r.chunk -= drop;
        }
        self.cursor.0 -= drop;
    }

    // ----- internals -----

    fn last_row_time(&self) -> Time {
        self.chunks
            .iter()
            .rev()
            .find_map(|c| c.time().last().copied())
            .unwrap_or(Time::NEG_INFINITY)
    }

    fn keyframe_at_or_before(&self, t: Time) -> Option<KeyframeRef> {
        let n = self.keyframes.partition_point(|k| k.time <= t);
        n.checked_sub(1).map(|i| self.keyframes[i])
    }

    /// Apply rows from the cursor while their time is `<= t`.
    fn apply_through(&mut self, t: Time) {
        let (mut c, mut row) = self.cursor;
        while let Some(chunk) = self.chunks.get(c) {
            let n = chunk.n_rows();
            while row < n && chunk.time()[row] <= t {
                let (lef, pos, mob) = (chunk.leg_id()[row] as usize / 2, chunk.pos()[row], chunk.mob()[row]);
                if chunk.leg_id()[row] % 2 == 0 {
                    (self.lefs.left_pos[lef], self.lefs.left_mobility[lef]) = (pos, mob);
                } else {
                    (self.lefs.right_pos[lef], self.lefs.right_mobility[lef]) = (pos, mob);
                }
                row += 1;
            }
            if row < n {
                break;
            }
            c += 1;
            row = 0;
        }
        self.cursor = (c, row);
    }
}
