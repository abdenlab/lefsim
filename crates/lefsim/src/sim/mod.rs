//! The [`Sim`] facade: one object that owns a whole run.
//!
//! A run needs a [`World`], an engine, an RNG, a spawn rule, and a
//! touched-entity buffer, all threaded through every step in the right order.
//! `Sim` owns them together so callers (and FFI bindings) wrap a single,
//! concrete, `Send` type.
//!
//! Construction goes through [`SimBuilder`], which is the only place LEFs can
//! be created. Once [`SimBuilder::build`] has initialized the engine's heap,
//! the set of entities is fixed for the rest of the run.
//!
//! ## Dense ids
//!
//! LEFs and legs are identified externally by dense `u32` ids assigned in
//! builder order: `lef_id` in `0..n_lefs`, and `leg_id = 2 * lef_id + side`
//! where side 0 is the left-facing leg and side 1 the right-facing one.
//! Slotmap keys stay internal.

mod engine;
mod heap;
pub mod record;

use std::fmt;

use rand::SeedableRng;
use slotmap::SecondaryMap;

use crate::model::action::Touched;
use crate::model::{
    Action, Lattice, LefKey, LegKey, Mobility, Position, Rates, SimRng, Spawn, SpawnRule, Time,
    World,
};
use engine::Engine;
use record::sealed::Observe;
use record::{LefFrames, Observer, Recorder, SampleMode, SimMeta};

/// How an initial LEF is placed during [`SimBuilder::build`].
#[derive(Copy, Clone, Debug)]
enum InitialLef {
    At(Position),
    FromSpawnRule,
}

/// Construction phase of a [`Sim`]. The only way to create LEFs.
pub struct SimBuilder {
    lattice: Lattice,
    rates: Rates,
    seed: u64,
    spawn: Box<dyn Spawn + Send + Sync>,
    initial: Vec<InitialLef>,
}

impl SimBuilder {
    /// Defaults: seed 0, [`Rates::default`], [`SpawnRule::UniformNonCtcf`], no LEFs.
    pub fn new(lattice: Lattice) -> Self {
        SimBuilder {
            lattice,
            rates: Rates::default(),
            seed: 0,
            spawn: Box::new(SpawnRule::UniformNonCtcf),
            initial: Vec::new(),
        }
    }

    pub fn rates(mut self, rates: Rates) -> Self {
        self.rates = rates;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Rule used to pick respawn sites, and initial sites for [`Self::lefs`].
    ///
    /// `Send + Sync` keeps [`Sim`] `Send + Sync`, which FFI wrappers need
    /// (e.g. a pyo3 `#[pyclass]`). Spawn rules take `&self`, so stateless or
    /// immutable rules satisfy this for free.
    pub fn spawn_rule<S: Spawn + Send + Sync + 'static>(mut self, rule: S) -> Self {
        self.spawn = Box::new(rule);
        self
    }

    /// Add one LEF with both legs at `pos`.
    pub fn lef_at(mut self, pos: Position) -> Self {
        self.initial.push(InitialLef::At(pos));
        self
    }

    /// Add `n` LEFs at sites drawn from the spawn rule.
    pub fn lefs(mut self, n: usize) -> Self {
        self.initial
            .extend(std::iter::repeat_n(InitialLef::FromSpawnRule, n));
        self
    }

    /// Place every LEF (in the order they were added), then initialize the
    /// engine. After this, no LEF can be added.
    ///
    /// The RNG draw order is: all LEF placements first, then initial
    /// scheduling — matching a hand-rolled `spawn_lef` + `Engine::initialize`
    /// sequence with the same seed.
    pub fn build(self) -> Result<Sim, BuildError> {
        let n_sites = self.lattice.len();
        for init in &self.initial {
            if let InitialLef::At(pos) = *init
                && pos as usize >= n_sites
            {
                return Err(BuildError::PositionOutOfBounds { pos, n_sites });
            }
        }

        let mut rng = SimRng::seed_from_u64(self.seed);
        let mut world = World::new(self.lattice);
        let n_lefs = self.initial.len();
        let mut lef_order = Vec::with_capacity(n_lefs);
        let mut leg_order = Vec::with_capacity(2 * n_lefs);
        let mut lef_ids = SecondaryMap::with_capacity(n_lefs);
        let mut leg_ids = SecondaryMap::with_capacity(2 * n_lefs);

        for init in self.initial {
            let pos = match init {
                InitialLef::At(pos) => pos,
                InitialLef::FromSpawnRule => self.spawn.sample(&world, &mut rng),
            };
            let (lef_k, [left, right]) = world.spawn_lef(pos, &mut rng);
            let lef_id = lef_order.len() as u32;
            lef_ids.insert(lef_k, lef_id);
            leg_ids.insert(left, 2 * lef_id);
            leg_ids.insert(right, 2 * lef_id + 1);
            lef_order.push(lef_k);
            leg_order.extend([left, right]);
        }

        let mut engine = Engine::new(self.rates);
        engine.initialize(&world, &mut rng);

        Ok(Sim {
            world,
            engine,
            rng,
            spawn: self.spawn,
            touched: Touched::default(),
            lef_order,
            leg_order,
            lef_ids,
            leg_ids,
            events: 0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    PositionOutOfBounds { pos: Position, n_sites: usize },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::PositionOutOfBounds { pos, n_sites } => {
                write!(f, "LEF position {pos} is outside a lattice of {n_sites} sites")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// A whole simulation run: world, engine, RNG, and spawn rule together.
pub struct Sim {
    world: World,
    engine: Engine,
    rng: SimRng,
    spawn: Box<dyn Spawn + Send + Sync>,
    touched: Touched,
    lef_order: Vec<LefKey>,
    leg_order: Vec<LegKey>,
    lef_ids: SecondaryMap<LefKey, u32>,
    leg_ids: SecondaryMap<LegKey, u32>,
    events: u64,
}

/// Why a batch driver returned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// `run_until` reached its end time.
    ReachedTime,
    /// `run_events` fired its full budget.
    ReachedEventCount,
    /// No finite events remain. Terminal: later calls fire nothing.
    Exhausted,
    /// An observer's buffer is full. Drain it and call the driver again to
    /// continue; no event has been lost.
    RecorderFull,
}

/// Summary of one batch-driver call.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RunReport {
    /// Events fired during this call.
    pub events: u64,
    /// Clock after this call.
    pub time: Time,
    pub stopped: StopReason,
}

impl Sim {
    pub fn builder(lattice: Lattice) -> SimBuilder {
        SimBuilder::new(lattice)
    }

    // ----- introspection -----

    /// Current simulation time.
    pub fn time(&self) -> Time {
        self.engine.clock()
    }

    /// Total events fired since construction.
    pub fn event_count(&self) -> u64 {
        self.events
    }

    /// Time of the next event, or `None` if none remain.
    pub fn next_event_time(&self) -> Option<Time> {
        self.engine.next_event_time()
    }

    pub fn rates(&self) -> Rates {
        self.engine.rates
    }

    pub fn n_sites(&self) -> usize {
        self.world.lattice.len()
    }

    pub fn n_lefs(&self) -> usize {
        self.lef_order.len()
    }

    pub fn n_legs(&self) -> usize {
        self.leg_order.len()
    }

    /// Read-only view of the world, for in-process inspection.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Dense id of a LEF key belonging to this sim.
    pub fn lef_id(&self, key: LefKey) -> Option<u32> {
        self.lef_ids.get(key).copied()
    }

    /// Dense id of a leg key belonging to this sim.
    pub fn leg_id(&self, key: LegKey) -> Option<u32> {
        self.leg_ids.get(key).copied()
    }

    /// `(left, right)` leg positions of a LEF. Panics if `lef_id` is out of range.
    pub fn lef_positions(&self, lef_id: u32) -> (Position, Position) {
        let [left, right] = self.lef_legs(lef_id);
        (self.world.legs[left].pos, self.world.legs[right].pos)
    }

    /// `(left, right)` leg mobilities of a LEF. Panics if `lef_id` is out of range.
    pub fn lef_mobilities(&self, lef_id: u32) -> (Mobility, Mobility) {
        let [left, right] = self.lef_legs(lef_id);
        (self.world.legs[left].mobility, self.world.legs[right].mobility)
    }

    fn lef_legs(&self, lef_id: u32) -> [LegKey; 2] {
        self.world.lefs[self.lef_order[lef_id as usize]].legs
    }

    /// `[(pos, mobility); 2]` for the left and right legs of a LEF.
    pub(crate) fn lef_state(&self, lef_id: u32) -> [(Position, Mobility); 2] {
        self.lef_legs(lef_id).map(|leg| {
            let leg = &self.world.legs[leg];
            (leg.pos, leg.mobility)
        })
    }

    /// LEF keys indexed by `lef_id`.
    pub(crate) fn lef_keys(&self) -> &[LefKey] {
        &self.lef_order
    }

    /// Leg keys indexed by `leg_id`.
    pub(crate) fn leg_keys(&self) -> &[LegKey] {
        &self.leg_order
    }

    /// Static tables describing this run (lattice, CTCFs, id mappings).
    pub fn meta(&self) -> SimMeta {
        SimMeta::from_sim(self)
    }

    /// A [`Recorder`] sized for this sim.
    pub fn recorder(&self, mode: SampleMode) -> Recorder {
        Recorder::new(mode, self.n_lefs())
    }

    // ----- drivers -----

    /// Fire exactly one event. Returns its time and action, or `None` if no
    /// events remain.
    pub fn step(&mut self) -> Option<(Time, Action)> {
        let fired = self.engine.step(
            &mut self.world,
            &*self.spawn,
            &mut self.rng,
            &mut self.touched,
        );
        if fired.is_some() {
            self.events += 1;
        }
        fired
    }

    /// Fire up to `n` events.
    pub fn run_events(&mut self, n: u64) -> RunReport {
        self.run_events_with(n, &mut ())
    }

    /// Fire every event with time `<= t_end`, then advance the clock to
    /// `t_end`.
    ///
    /// Advancing the clock makes calls compose: `run_until(a)` followed by
    /// `run_until(b)` leaves the sim in the same state as `run_until(b)`.
    /// A `t_end` at or before the current time fires nothing.
    pub fn run_until(&mut self, t_end: Time) -> RunReport {
        self.run_until_with(t_end, &mut ())
    }

    /// [`Sim::run_events`], feeding `obs`.
    pub fn run_events_with<O: Observer + ?Sized>(&mut self, n: u64, obs: &mut O) -> RunReport {
        self.drive(Time::INFINITY, n, obs)
    }

    /// [`Sim::run_until`], feeding `obs`. Grid snapshots up to and including
    /// `t_end` are recorded.
    ///
    /// Returns early with [`StopReason::RecorderFull`] if an observer fills
    /// up; drain it and call again with the same `t_end` to continue.
    pub fn run_until_with<O: Observer + ?Sized>(&mut self, t_end: Time, obs: &mut O) -> RunReport {
        assert!(!t_end.is_nan(), "run_until: t_end is NaN");
        self.drive(t_end, u64::MAX, obs)
    }

    /// [`Sim::run_until_with`], but firing at most `max_events` events.
    ///
    /// Returns [`StopReason::ReachedEventCount`] if the cap is hit before
    /// `t_end`; call again to continue. Recording is seamless across calls.
    /// Lets a caller bound the work done per call, e.g. to check for
    /// interrupts between chunks of a long run.
    pub fn run_until_with_limit<O: Observer + ?Sized>(
        &mut self,
        t_end: Time,
        max_events: u64,
        obs: &mut O,
    ) -> RunReport {
        assert!(!t_end.is_nan(), "run_until: t_end is NaN");
        self.drive(t_end, max_events, obs)
    }

    /// Run to `t_end` and return every snapshot taken under `mode`.
    pub fn collect_until(&mut self, t_end: Time, mode: SampleMode) -> (LefFrames, RunReport) {
        let mut rec = self.recorder(mode);
        let report = self.run_until_with(t_end, &mut rec);
        (rec.take_frames(), report)
    }

    /// The one driver loop behind every `run_*` method.
    ///
    /// Invariant: no event fires while the observer still owes samples for
    /// times before it. So stopping on a full observer never loses anything:
    /// the next call picks up exactly where this one left off.
    fn drive<O: Observe + ?Sized>(&mut self, t_end: Time, budget: u64, obs: &mut O) -> RunReport {
        let start = self.events;
        loop {
            let next = self.next_event_time();
            let stop = match next {
                Some(t) if t <= t_end => {
                    (self.events - start >= budget).then_some(StopReason::ReachedEventCount)
                }
                Some(_) => Some(StopReason::ReachedTime),
                None => Some(StopReason::Exhausted),
            };

            if let Some(reason) = stop {
                // Nothing more fires in this call. The current state holds
                // until `next`, so settle the observer up to where the clock
                // will end: `t_end` for a timed run, else the current time.
                let now = self.time();
                let end = if reason != StopReason::ReachedEventCount && t_end.is_finite() {
                    t_end.max(now)
                } else {
                    now
                };
                // A grid point exactly at `end` is final only if no event
                // is also due then (e.g. the respawn after an eviction).
                let inclusive = next.is_none_or(|t| t > end);
                if !obs.sample(end, inclusive, self) {
                    return self.report(start, StopReason::RecorderFull);
                }
                if end > now {
                    self.engine.advance_clock_to(end);
                }
                return self.report(start, reason);
            }

            // Settle everything before the next event, then fire it.
            let t = next.expect("stop handled the no-event case");
            if !obs.sample(t, false, self) || !obs.has_room_for_event() {
                return self.report(start, StopReason::RecorderFull);
            }
            let (t, action) = self.step().expect("an event was pending");
            obs.on_event(t, action, self);
        }
    }

    fn report(&self, start_events: u64, stopped: StopReason) -> RunReport {
        RunReport {
            events: self.events - start_events,
            time: self.time(),
            stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Direction, Site};

    fn two_ctcf_lattice() -> Lattice {
        let mut sites = vec![Site::plain(); 100];
        sites[30] = Site::ctcf(Direction::Right, 0.5);
        sites[70] = Site::ctcf(Direction::Left, 0.5);
        Lattice::new(sites).unwrap()
    }

    #[test]
    fn sim_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Sim>();
    }

    #[test]
    fn build_rejects_out_of_bounds_position() {
        let err = Sim::builder(Lattice::plain(10).unwrap()).lef_at(10).build().err();
        assert_eq!(
            err,
            Some(BuildError::PositionOutOfBounds { pos: 10, n_sites: 10 })
        );
    }

    #[test]
    fn dense_ids_follow_builder_order() {
        let sim = Sim::builder(Lattice::plain(100).unwrap())
            .lef_at(10)
            .lef_at(50)
            .lefs(3)
            .build()
            .unwrap();
        assert_eq!(sim.n_lefs(), 5);
        assert_eq!(sim.n_legs(), 10);
        assert_eq!(sim.lef_positions(0), (10, 10));
        assert_eq!(sim.lef_positions(1), (50, 50));
        for (i, &lef_k) in sim.lef_order.iter().enumerate() {
            assert_eq!(sim.lef_id(lef_k), Some(i as u32));
            let [left, right] = sim.world.lefs[lef_k].legs;
            assert_eq!(sim.leg_id(left), Some(2 * i as u32));
            assert_eq!(sim.leg_id(right), Some(2 * i as u32 + 1));
            assert_eq!(sim.world.legs[left].facing, Direction::Left);
            assert_eq!(sim.world.legs[right].facing, Direction::Right);
        }
    }

    /// The facade must be a faithful wrapper: the same seed gives the same
    /// event sequence as driving World + Engine by hand.
    #[test]
    fn sim_matches_hand_rolled_loop() {
        let seed = 17;
        let mut sim = Sim::builder(two_ctcf_lattice())
            .seed(seed)
            .lef_at(40)
            .lef_at(60)
            .build()
            .unwrap();

        let mut rng = SimRng::seed_from_u64(seed);
        let mut world = World::new(two_ctcf_lattice());
        world.spawn_lef(40, &mut rng);
        world.spawn_lef(60, &mut rng);
        let mut engine = Engine::new(Rates::default());
        engine.initialize(&world, &mut rng);
        let mut touched = Touched::default();

        for _ in 0..5000 {
            let a = sim.step();
            let b = engine.step(&mut world, &SpawnRule::UniformNonCtcf, &mut rng, &mut touched);
            assert_eq!(a.map(|(t, _)| t), b.map(|(t, _)| t));
        }
        for (i, &lef_k) in sim.lef_order.iter().enumerate() {
            let [l, r] = world.lefs[lef_k].legs;
            assert_eq!(
                sim.lef_positions(i as u32),
                (world.legs[l].pos, world.legs[r].pos)
            );
        }
    }

    #[test]
    fn run_until_fires_events_up_to_and_including_t_end() {
        let mut sim = Sim::builder(two_ctcf_lattice()).lef_at(50).build().unwrap();
        let report = sim.run_until(25.0);
        assert_eq!(report.stopped, StopReason::ReachedTime);
        assert_eq!(report.time, 25.0);
        assert_eq!(sim.time(), 25.0);
        assert!(sim.next_event_time().unwrap() > 25.0);
        assert_eq!(report.events, sim.event_count());
    }

    #[test]
    fn run_until_composes() {
        let build = || {
            Sim::builder(two_ctcf_lattice())
                .seed(9)
                .lef_at(45)
                .lef_at(55)
                .build()
                .unwrap()
        };
        let mut split = build();
        split.run_until(10.0);
        split.run_until(20.0);
        let mut whole = build();
        whole.run_until(20.0);

        assert_eq!(split.time(), whole.time());
        assert_eq!(split.event_count(), whole.event_count());
        for i in 0..2 {
            assert_eq!(split.lef_positions(i), whole.lef_positions(i));
        }
    }

    #[test]
    fn run_until_into_the_past_is_a_no_op() {
        let mut sim = Sim::builder(two_ctcf_lattice()).lef_at(50).build().unwrap();
        sim.run_until(10.0);
        let events = sim.event_count();
        let report = sim.run_until(5.0);
        assert_eq!(report.events, 0);
        assert_eq!(sim.time(), 10.0);
        assert_eq!(sim.event_count(), events);
    }

    #[test]
    fn run_events_fires_exact_budget() {
        let mut sim = Sim::builder(two_ctcf_lattice()).lef_at(50).build().unwrap();
        let report = sim.run_events(123);
        assert_eq!(report.events, 123);
        assert_eq!(report.stopped, StopReason::ReachedEventCount);
        assert_eq!(sim.event_count(), 123);
    }

    #[test]
    fn empty_sim_is_exhausted() {
        let mut sim = Sim::builder(Lattice::plain(10).unwrap()).build().unwrap();
        assert_eq!(sim.run_events(5).stopped, StopReason::Exhausted);
        let report = sim.run_until(3.0);
        assert_eq!(report.stopped, StopReason::Exhausted);
        assert_eq!(sim.time(), 3.0);
    }
}
