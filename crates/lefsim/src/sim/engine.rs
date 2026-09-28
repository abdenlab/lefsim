//! Priority-queue-driven simulation loop.
//!
//! Built around an indexed binary min-heap of actions keyed by event time
//! (`super::heap::IndexedHeap`, a textbook Gibson-Bruck "next reaction method"
//! structure). Rescheduling an action mutates its priority in place via a
//! slotmap-keyed position lookup; every action stays in the heap for the
//! simulation's lifetime. A `Schedule::Never` schedule is represented by
//! `+∞` priority, which sits at the bottom of the heap without being
//! removed.


use crate::model::action::{Action, Rates, Touched};
use crate::model::{Schedule, SimRng, SpawnRule, Time, World};

use super::heap::{IndexedHeap, Priority};

/// The event loop. Crate-internal: [`crate::Sim`] is the public driver.
pub(crate) struct Engine {
    clock: Time,
    pub(crate) rates: Rates,
    heap: IndexedHeap,
}

impl Engine {
    pub(crate) fn new(rates: Rates) -> Self {
        Engine {
            clock: 0.0,
            rates,
            heap: IndexedHeap::new(),
        }
    }

    /// Populate the heap with one entry per action in `world`. Call
    /// once after constructing the engine. Subsequent rescheduling
    /// updates entries in place.
    pub(crate) fn initialize(&mut self, world: &World, rng: &mut SimRng) {
        let total = world.legs.len() + world.lefs.len();
        self.heap = IndexedHeap::with_capacity(total);
        for k in world.legs.keys().collect::<Vec<_>>() {
            let action = Action::Step { leg: k };
            let priority = match action.schedule(world, &self.rates, self.clock, rng) {
                Schedule::At(t) => Priority(t),
                Schedule::Never => Priority::NEVER,
            };
            self.heap.insert(action, priority);
        }
        for k in world.lefs.keys().collect::<Vec<_>>() {
            let action = Action::Respawn { lef: k };
            let priority = match action.schedule(world, &self.rates, self.clock, rng) {
                Schedule::At(t) => Priority(t),
                Schedule::Never => Priority::NEVER,
            };
            self.heap.insert(action, priority);
        }
    }

    /// Current simulation time.
    pub(crate) fn clock(&self) -> Time {
        self.clock
    }

    /// Time of the soonest scheduled event, or `None` if every action is
    /// scheduled `Never`. Does not mutate.
    ///
    /// Together with `Engine::advance_clock_to`, this is what makes exact
    /// time-grid sampling possible: state is piecewise-constant between
    /// events, so the state at any `t < next_event_time()` is the current one.
    pub(crate) fn next_event_time(&self) -> Option<Time> {
        self.heap
            .peek()
            .filter(|(p, _)| p.is_finite())
            .map(|(p, _)| p.0)
    }

    /// Advance the clock to `t` without firing anything.
    ///
    /// Sound only because state is piecewise-constant between events: `t`
    /// must not move backwards and must not pass a pending event.
    pub(crate) fn advance_clock_to(&mut self, t: Time) {
        debug_assert!(t >= self.clock, "clock must be monotone");
        debug_assert!(
            self.next_event_time().is_none_or(|next| t <= next),
            "cannot advance past a pending event"
        );
        self.clock = t;
    }

    /// Recompute an action's priority and update its position in the
    /// heap in O(log n). The action must already be in the heap (added
    /// during [`Engine::initialize`]); for the fixed-cardinality LEF
    /// model that's every leg's `Step` and every LEF's `Respawn`.
    pub(crate) fn reschedule(&mut self, action: Action, world: &World, rng: &mut SimRng) {
        let priority = match action.schedule(world, &self.rates, self.clock, rng) {
            Schedule::At(t) => Priority(t),
            Schedule::Never => Priority::NEVER,
        };
        self.heap.update(action, priority);
    }

    /// Fire the next event, advance the clock, and reschedule whatever
    /// the action's mutations invalidated. Returns the time and action
    /// that fired, or `None` if no finite events remain.
    pub(crate) fn step(
        &mut self,
        world: &mut World,
        spawn: &dyn SpawnRule,
        rng: &mut SimRng,
        touched: &mut Touched,
    ) -> Option<(Time, Action)> {
        let (priority, action) = self.heap.peek()?;
        if !priority.is_finite() {
            return None;
        }
        let time = priority.0;
        self.clock = time;

        touched.clear();
        action.fire(world, spawn, time, rng, touched);

        // Reactive rescheduling: the fired action and everything it may have
        // invalidated, each once, as the model's dependency map names them.
        for dep in touched.dependent_actions(world, action) {
            self.reschedule(dep, world, rng);
        }

        Some((time, action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Direction, Lattice, Site, UniformNonCtcfSpawn};
    use rand::SeedableRng;

    #[test]
    fn next_event_time_predicts_next_step() {
        let mut sites = vec![Site::plain(); 100];
        sites[30] = Site::ctcf(Direction::Right, 0.5);
        sites[70] = Site::ctcf(Direction::Left, 0.5);
        let mut world = World::new(Lattice::new(sites).unwrap());
        let mut rng = SimRng::seed_from_u64(3);
        world.spawn_lef(40, &mut rng);
        world.spawn_lef(60, &mut rng);
        let mut engine = Engine::new(Rates::default());
        engine.initialize(&world, &mut rng);

        let mut touched = Touched::default();
        for _ in 0..2000 {
            let predicted = engine.next_event_time();
            let fired = engine
                .step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
                .map(|(t, _)| t);
            assert_eq!(predicted, fired);
            assert_eq!(engine.clock(), fired.unwrap());
        }
    }

    #[test]
    fn advance_clock_to_leaves_next_event_untouched() {
        let mut world = World::new(Lattice::plain(50).unwrap());
        let mut rng = SimRng::seed_from_u64(5);
        world.spawn_lef(25, &mut rng);
        let mut engine = Engine::new(Rates::default());
        engine.initialize(&world, &mut rng);

        let next = engine.next_event_time().unwrap();
        engine.advance_clock_to(next / 2.0);
        assert_eq!(engine.clock(), next / 2.0);
        assert_eq!(engine.next_event_time(), Some(next));
    }
}
