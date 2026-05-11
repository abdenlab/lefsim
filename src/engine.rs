//! Priority-queue-driven simulation loop.
//!
//! The scheduler stores `(time, action, epoch)` tuples in a min-heap. When an
//! action is rescheduled, its epoch is bumped; old entries are dropped on pop
//! when their epoch no longer matches. This avoids a `decrease-key` operation
//! and lets us use the standard library's `BinaryHeap`.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use rand_pcg::Pcg64;
use smallvec::SmallVec;

use crate::action::{Action, Rates, Schedule, Time, Touched};
use crate::spawn::SpawnRule;
use crate::world::World;

/// Concrete RNG type used throughout the simulation. Locked in so that
/// signatures stay simple and seeded runs are reproducible. Trade off:
/// callers wanting a different RNG must change this alias.
pub type SimRng = Pcg64;

pub struct Engine {
    pub clock: Time,
    pub rates: Rates,
    queue: BinaryHeap<HeapEntry>,
    /// Current epoch per action. A heap entry is valid iff its epoch matches.
    epoch: HashMap<Action, u64>,
}

#[derive(Debug)]
struct HeapEntry {
    time: Time,
    epoch: u64,
    action: Action,
}

// Min-heap behavior: smaller `time` comes out first.
// `Schedule::Never` actions never enter the heap, so NaN is not expected;
// we fall back to `Equal` defensively.
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time.partial_cmp(&self.time).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && self.action == other.action && self.epoch == other.epoch
    }
}
impl Eq for HeapEntry {}

impl Engine {
    pub fn new(rates: Rates) -> Self {
        Engine {
            clock: 0.0,
            rates,
            queue: BinaryHeap::new(),
            epoch: HashMap::new(),
        }
    }

    /// (Re)schedule an action: bump its epoch, run [`Action::schedule`], and
    /// push a fresh heap entry. Older entries become stale and are discarded
    /// on pop.
    pub fn reschedule(&mut self, action: Action, world: &World, rng: &mut SimRng) {
        let e = self.epoch.entry(action).or_insert(0);
        *e += 1;
        match action.schedule(world, &self.rates, self.clock, rng) {
            Schedule::At(t) => self.queue.push(HeapEntry { time: t, epoch: *e, action }),
            Schedule::Never => {}
        }
    }

    /// Prime the queue by scheduling every leg's `Step` and every LEF's
    /// `Respawn`. Call once after constructing the world and engine.
    pub fn prime(&mut self, world: &World, rng: &mut SimRng) {
        let leg_keys: Vec<_> = world.legs.keys().collect();
        for k in leg_keys {
            self.reschedule(Action::Step { leg: k }, world, rng);
        }
        let lef_keys: Vec<_> = world.lefs.keys().collect();
        for k in lef_keys {
            self.reschedule(Action::Respawn { lef: k }, world, rng);
        }
    }

    /// Pop and execute the next valid action. Advances the clock to its time.
    /// Returns `None` if the queue is exhausted.
    pub fn step<S: SpawnRule>(
        &mut self,
        world: &mut World,
        spawn: &S,
        rng: &mut SimRng,
        touched: &mut Touched,
    ) -> Option<(Time, Action)> {
        // Pop until we find a non-stale entry.
        let entry = loop {
            let head = self.queue.pop()?;
            let current = self.epoch.get(&head.action).copied().unwrap_or(0);
            if head.epoch == current {
                break head;
            }
        };
        let HeapEntry { time, action, .. } = entry;
        self.clock = time;

        touched.clear();
        action.fire(world, spawn, time, rng, touched);

        // Reactive rescheduling, deduped against a small in-step set.
        // Constant fan-out per touched entity:
        //   touched leg  -> Step{leg}, Respawn{leg.lef}
        //   touched lef  -> Respawn{lef}
        //   plus the fired action itself.
        let mut seen: SmallVec<[Action; 16]> = SmallVec::new();
        let visit = |eng: &mut Engine, w: &World, r: &mut SimRng, a: Action,
                     seen: &mut SmallVec<[Action; 16]>| {
            if !seen.contains(&a) {
                seen.push(a);
                eng.reschedule(a, w, r);
            }
        };

        visit(self, world, rng, action, &mut seen);
        for &leg_k in &touched.legs {
            let lef_k = world.legs[leg_k].lef;
            visit(self, world, rng, Action::Step { leg: leg_k }, &mut seen);
            visit(self, world, rng, Action::Respawn { lef: lef_k }, &mut seen);
        }
        for &lef_k in &touched.lefs {
            visit(self, world, rng, Action::Respawn { lef: lef_k }, &mut seen);
        }

        Some((time, action))
    }
}
