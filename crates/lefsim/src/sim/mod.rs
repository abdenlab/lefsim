//! The [`Sim`] facade: one object that owns a whole run.
//!
//! A run needs a [`World`], an engine, an RNG, a spawn rule, and a
//! touched-entity buffer, all threaded through every step in the right order.
//! `Sim` owns them together so callers (and FFI bindings) wrap a single,
//! concrete, `Send` type.
//!
//! A `Sim` starts from a [`Model`] and a seed ([`Sim::new`]). It takes the
//! model by value and only exposes the world read-only afterwards, so the
//! set of entities is fixed once the event heap exists.
//!
//! ## Dense ids
//!
//! LEFs and legs are identified externally by dense `u32` ids: `lef_id` is
//! the LEF's row in the world's [`LefStates`](crate::LefStates), and
//! `leg_id = 2 * lef_id + side` where side 0 is the left-facing leg and side
//! 1 the right-facing one. Slotmap keys stay internal.

mod engine;
mod heap;
pub mod record;
mod replay;

pub use replay::{ReplayError, Replayer};

use std::sync::Arc;

use rand::SeedableRng;
use slotmap::SecondaryMap;

use crate::model::action::Touched;
use crate::model::{
    self, Action, LefKey, LegKey, Model, ModelError, Rates, SimRng, Spawn,
    SpawnRule, Time, World,
};
use engine::Engine;
use record::sealed::Observe;
use record::{LefFrames, Observer, Recorder, SampleMode};

/// The spawn rule a `Sim` runs with. Custom rules are shared, not copied,
/// when a `Sim` is cloned: rules take `&self`, so that's safe.
#[derive(Clone)]
enum SpawnSource {
    BuiltIn(SpawnRule),
    Custom(Arc<dyn Spawn + Send + Sync>),
}

impl SpawnSource {
    fn get(&self) -> &dyn Spawn {
        match self {
            SpawnSource::BuiltIn(rule) => rule,
            SpawnSource::Custom(rule) => &**rule,
        }
    }
}

/// A whole simulation run: world, engine, RNG, and spawn rule together.
///
/// `Clone` gives an independent copy that continues bit for bit like the
/// original: a fork of the run at its current time.
#[derive(Clone)]
pub struct Sim {
    world: World,
    engine: Engine,
    rng: SimRng,
    spawn: SpawnSource,
    touched: Touched,
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
    // ----- construction -----

    /// Run `model` from time 0. `seed` drives the whole run.
    pub fn new(model: Model, seed: u64) -> Self {
        Self::with_start_time(model, seed, 0.0)
    }

    /// Run `model` with the clock starting at `start_time`, e.g. to continue
    /// from a state recorded at that time. Panics if `start_time` isn't
    /// finite.
    pub fn with_start_time(model: Model, seed: u64, start_time: Time) -> Self {
        let (world, rates, spawn) = model.into_parts();
        Self::start(world, rates, SpawnSource::BuiltIn(spawn), seed, start_time)
    }

    /// Run a world with a custom spawn rule, from time 0. Checks the rates
    /// and the rule as [`Model::new`] does.
    ///
    /// `Send + Sync` keeps [`Sim`] `Send + Sync`, which FFI wrappers need
    /// (e.g. a pyo3 `#[pyclass]`). Spawn rules take `&self`, so stateless or
    /// immutable rules satisfy this for free.
    pub fn with_spawn<S: Spawn + Send + Sync + 'static>(
        world: World,
        rates: Rates,
        spawn: S,
        seed: u64,
    ) -> Result<Self, ModelError> {
        model::check(&world, &rates, &spawn)?;
        Ok(Self::start(world, rates, SpawnSource::Custom(Arc::new(spawn)), seed, 0.0))
    }

    fn start(world: World, rates: Rates, spawn: SpawnSource, seed: u64, start_time: Time) -> Self {
        assert!(start_time.is_finite(), "start time must be finite, got {start_time}");
        let n_lefs = world.n_lefs();
        let mut lef_ids = SecondaryMap::with_capacity(n_lefs);
        let mut leg_ids = SecondaryMap::with_capacity(2 * n_lefs);
        for (lef_id, &lef_k) in world.lef_order.iter().enumerate() {
            let lef_id = lef_id as u32;
            let [left, right] = world.lefs[lef_k].legs;
            lef_ids.insert(lef_k, lef_id);
            leg_ids.insert(left, 2 * lef_id);
            leg_ids.insert(right, 2 * lef_id + 1);
        }

        let mut rng = SimRng::seed_from_u64(seed);
        let mut engine = Engine::new(rates, start_time);
        engine.initialize(&world, &mut rng);
        Sim {
            world,
            engine,
            rng,
            spawn,
            touched: Touched::default(),
            lef_ids,
            leg_ids,
            events: 0,
        }
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
        self.world.n_lefs()
    }

    pub fn n_legs(&self) -> usize {
        2 * self.n_lefs()
    }

    /// The current state, read-only.
    ///
    /// Between an eviction and the respawn it triggers (at the same
    /// instant), the world holds an `Evicted` leg. The batch drivers only
    /// stop there when an event budget runs out mid-chain; [`Sim::step`] can.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The current state as a model: the world now, plus the rates and
    /// spawn rule. `None` if this sim runs a custom spawn rule
    /// ([`Sim::with_spawn`]), which a [`Model`] can't hold.
    ///
    /// Running it with `Sim::with_start_time(model, seed, sim.time())`
    /// continues from here with fresh randomness. To continue bit for bit,
    /// clone the sim instead.
    pub fn model(&self) -> Option<Model> {
        match self.spawn {
            SpawnSource::BuiltIn(spawn) => {
                Some(Model { world: self.world.clone(), rates: self.rates(), spawn })
            }
            SpawnSource::Custom(_) => None,
        }
    }

    /// Dense id of a LEF key belonging to this sim.
    pub fn lef_id(&self, key: LefKey) -> Option<u32> {
        self.lef_ids.get(key).copied()
    }

    /// Dense id of a leg key belonging to this sim.
    pub fn leg_id(&self, key: LegKey) -> Option<u32> {
        self.leg_ids.get(key).copied()
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
            self.spawn.get(),
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
    use crate::model::{Direction, Lattice, Mobility, Position, Site};

    /// `(left, right)` leg positions of LEF `i`.
    fn positions(sim: &Sim, i: usize) -> (Position, Position) {
        let ((l, _), (r, _)) = sim.world().lef_state(i).unwrap();
        (l, r)
    }

    fn two_ctcf_lattice() -> Lattice {
        let mut sites = vec![Site::plain(); 100];
        sites[30] = Site::ctcf(Direction::Right, 0.5);
        sites[70] = Site::ctcf(Direction::Left, 0.5);
        Lattice::new(sites).unwrap()
    }

    /// A model with a moving LEF at each of `sites`.
    fn model_at(sites: &[Position]) -> Model {
        let mut world = World::new(two_ctcf_lattice());
        for &p in sites {
            world.add_lef((p, Mobility::Moving), (p, Mobility::Moving)).unwrap();
        }
        Model::new(world, Rates::default(), SpawnRule::UniformNonCtcf).unwrap()
    }

    #[test]
    fn sim_is_send_sync_and_clone() {
        fn assert_traits<T: Send + Sync + Clone>() {}
        assert_traits::<Sim>();
    }

    #[test]
    fn dense_ids_follow_world_order() {
        let sim = Sim::new(model_at(&[10, 50, 20]), 0);
        assert_eq!(sim.n_lefs(), 3);
        assert_eq!(sim.n_legs(), 6);
        assert_eq!(positions(&sim, 0), (10, 10));
        assert_eq!(positions(&sim, 1), (50, 50));
        assert_eq!(positions(&sim, 2), (20, 20));
        for (i, &lef_k) in sim.world.lef_order.iter().enumerate() {
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
        let model = model_at(&[40, 60]);
        let mut world = model.world().clone();
        let mut sim = Sim::new(model, seed);

        let mut rng = SimRng::seed_from_u64(seed);
        let mut engine = Engine::new(Rates::default(), 0.0);
        engine.initialize(&world, &mut rng);
        let mut touched = Touched::default();

        for _ in 0..5000 {
            let a = sim.step();
            let b = engine.step(&mut world, &SpawnRule::UniformNonCtcf, &mut rng, &mut touched);
            assert_eq!(a.map(|(t, _)| t), b.map(|(t, _)| t));
        }
        assert_eq!(sim.world(), &world);
    }

    #[test]
    fn a_clone_continues_bit_for_bit() {
        let mut sim = Sim::new(model_at(&[40, 60]), 3);
        sim.run_until(50.0);
        let mut fork = sim.clone();
        sim.run_until(500.0);
        fork.run_until(500.0);
        assert_eq!(sim.event_count(), fork.event_count());
        assert_eq!(sim.world(), fork.world());
    }

    #[test]
    fn model_reads_back_the_current_state() {
        let mut sim = Sim::new(model_at(&[40, 60]), 3);
        assert_eq!(sim.model(), Some(model_at(&[40, 60])));
        sim.run_until(100.0);
        let model = sim.model().unwrap();
        assert_eq!(model.world(), sim.world());
        assert_eq!(model.rates(), sim.rates());

        // Restarting from it is a valid run from the current time.
        let mut next = Sim::with_start_time(model, 4, sim.time());
        assert_eq!(next.time(), 100.0);
        assert!(next.next_event_time().unwrap() >= 100.0);
        assert!(next.run_until(200.0).events > 0);
    }

    #[test]
    fn custom_spawn_rules_are_checked_and_have_no_model() {
        /// Always the same site; fine for one LEF.
        struct Fixed(Position);
        impl Spawn for Fixed {
            fn sample(&self, _: &World, _: &mut SimRng) -> Position {
                self.0
            }
            fn check(&self, _: &Lattice, n_lefs: usize) -> Result<(), crate::SpawnError> {
                if n_lefs <= 1 {
                    Ok(())
                } else {
                    Err(crate::SpawnError::NotEnoughSites { n_lefs, needed: n_lefs, n_plain: 1 })
                }
            }
        }

        let one = model_at(&[40]).world().clone();
        let mut sim = Sim::with_spawn(one, Rates::default(), Fixed(12), 0).unwrap();
        assert_eq!(sim.model(), None);
        while !matches!(sim.step(), Some((_, Action::Respawn { .. }))) {}
        assert_eq!(positions(&sim, 0), (12, 12));
        let _fork = sim.clone(); // shares the rule

        let two = model_at(&[40, 60]).world().clone();
        assert!(matches!(
            Sim::with_spawn(two, Rates::default(), Fixed(12), 0),
            Err(ModelError::Spawn(_))
        ));
    }

    #[test]
    fn run_until_fires_events_up_to_and_including_t_end() {
        let mut sim = Sim::new(model_at(&[50]), 0);
        let report = sim.run_until(25.0);
        assert_eq!(report.stopped, StopReason::ReachedTime);
        assert_eq!(report.time, 25.0);
        assert_eq!(sim.time(), 25.0);
        assert!(sim.next_event_time().unwrap() > 25.0);
        assert_eq!(report.events, sim.event_count());
    }

    #[test]
    fn run_until_composes() {
        let build = || Sim::new(model_at(&[45, 55]), 9);
        let mut split = build();
        split.run_until(10.0);
        split.run_until(20.0);
        let mut whole = build();
        whole.run_until(20.0);

        assert_eq!(split.time(), whole.time());
        assert_eq!(split.event_count(), whole.event_count());
        assert_eq!(split.world(), whole.world());
    }

    #[test]
    fn run_until_into_the_past_is_a_no_op() {
        let mut sim = Sim::new(model_at(&[50]), 0);
        sim.run_until(10.0);
        let events = sim.event_count();
        let report = sim.run_until(5.0);
        assert_eq!(report.events, 0);
        assert_eq!(sim.time(), 10.0);
        assert_eq!(sim.event_count(), events);
    }

    #[test]
    fn run_events_fires_exact_budget() {
        let mut sim = Sim::new(model_at(&[50]), 0);
        let report = sim.run_events(123);
        assert_eq!(report.events, 123);
        assert_eq!(report.stopped, StopReason::ReachedEventCount);
        assert_eq!(sim.event_count(), 123);
    }

    #[test]
    fn empty_sim_is_exhausted() {
        let mut sim = Sim::new(model_at(&[]), 0);
        assert_eq!(sim.run_events(5).stopped, StopReason::Exhausted);
        let report = sim.run_until(3.0);
        assert_eq!(report.stopped, StopReason::Exhausted);
        assert_eq!(sim.time(), 3.0);
    }

    #[test]
    fn start_time_offsets_the_clock() {
        let mut sim = Sim::with_start_time(model_at(&[50]), 0, 1000.0);
        assert_eq!(sim.time(), 1000.0);
        assert!(sim.next_event_time().unwrap() > 1000.0);
        let (frames, _) = sim.collect_until(1010.0, SampleMode::every_dt(5.0));
        assert_eq!(frames.time(), &[1000.0, 1005.0, 1010.0]);
    }
}
