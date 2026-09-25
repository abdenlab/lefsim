//! Priority-queue-driven simulation loop.
//!
//! Built around an indexed binary min-heap of actions keyed by event time
//! ([`heap::IndexedHeap`], a textbook Gibson-Bruck "next reaction method"
//! structure). Rescheduling an action mutates its priority in place via a
//! slotmap-keyed position lookup; every action stays in the heap for the
//! simulation's lifetime. A `Schedule::Never` schedule is represented by
//! `+∞` priority, which sits at the bottom of the heap without being
//! removed.

mod heap;

use rand_pcg::Pcg64;
use smallvec::SmallVec;

use crate::action::{Action, Rates, Touched};
use crate::spawn::SpawnRule;
use crate::world::World;

use heap::{IndexedHeap, Priority};

/// Continuous simulation clock. `f64` is plenty of precision for the
/// time scales we care about.
pub type Time = f64;

/// Result returned by [`Action::schedule`]. Tells the engine when (or
/// whether) to next fire the action.
#[derive(Copy, Clone, Debug)]
pub enum Schedule {
    At(Time),
    Never,
}

/// Concrete RNG type used throughout the simulation. Locked in so that
/// signatures stay simple and seeded runs are reproducible. Trade off:
/// callers wanting a different RNG must change this alias.
pub type SimRng = Pcg64;

pub struct Engine {
    pub clock: Time,
    pub rates: Rates,
    heap: IndexedHeap,
}

impl Engine {
    pub fn new(rates: Rates) -> Self {
        Engine {
            clock: 0.0,
            rates,
            heap: IndexedHeap::new(),
        }
    }

    /// Populate the heap with one entry per action in `world`. Call
    /// once after constructing the engine. Subsequent rescheduling
    /// updates entries in place.
    pub fn initialize(&mut self, world: &World, rng: &mut SimRng) {
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

    /// Recompute an action's priority and update its position in the
    /// heap in O(log n). The action must already be in the heap (added
    /// during [`Engine::initialize`]); for the fixed-cardinality LEF
    /// model that's every leg's `Step` and every LEF's `Respawn`.
    pub fn reschedule(&mut self, action: Action, world: &World, rng: &mut SimRng) {
        let priority = match action.schedule(world, &self.rates, self.clock, rng) {
            Schedule::At(t) => Priority(t),
            Schedule::Never => Priority::NEVER,
        };
        self.heap.update(action, priority);
    }

    /// Fire the next event, advance the clock, and reschedule whatever
    /// the action's mutations invalidated. Returns the time and action
    /// that fired, or `None` if no finite events remain.
    pub fn step<S: SpawnRule>(
        &mut self,
        world: &mut World,
        spawn: &S,
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

        // Reactive rescheduling. The fired action is always re-evaluated;
        // beyond that, `touched.dependent_actions` names every action that
        // may have been invalidated by this step (the model's entity →
        // action dependency map). Dedup against a small in-step set.
        let mut seen: SmallVec<[Action; 16]> = SmallVec::new();
        self.reschedule(action, world, rng);
        seen.push(action);
        for dep in touched.dependent_actions(world) {
            if !seen.contains(&dep) {
                seen.push(dep);
                self.reschedule(dep, world, rng);
            }
        }

        Some((time, action))
    }
}
