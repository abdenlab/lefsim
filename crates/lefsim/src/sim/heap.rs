//! Indexed binary min-heap of [`Action`]s keyed by event time.
//!
//! Implements the Gibson-Bruck "next reaction method" data structure: every
//! action stays in the heap for the simulation's lifetime, and a "Never"
//! schedule is represented by an `+∞` priority that sits at the bottom of
//! the heap without ever being removed. Updating an action's priority via
//! [`IndexedHeap::update`] re-heapifies in place in O(log n) using a
//! slotmap-backed position index — no hash table lookups, no stale
//! entries to drain on pop.
//!
//! Internal to the sim module; not part of the public crate surface.

use slotmap::SecondaryMap;
use std::cmp::Ordering;

use crate::model::{Action, LefKey, LegKey};

/// Heap priority. Smaller compares first (min-heap). Total order via
/// [`f64::total_cmp`]; `Priority::NEVER` is `+∞` and acts as a sentinel
/// for actions that aren't currently scheduled.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Priority(pub f64);

impl Priority {
    pub const NEVER: Self = Priority(f64::INFINITY);

    pub fn is_finite(self) -> bool {
        self.0.is_finite()
    }
}

impl Eq for Priority {}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Copy, Clone, Debug)]
struct Node {
    priority: Priority,
    action: Action,
}

/// Min-heap of actions keyed by event time, supporting O(log n) updates
/// of any element's priority by `Action` key.
///
/// Internally a flat binary heap (`Vec<Node>`) plus two slotmap-backed
/// position indices — one per action variant — so a node's heap index
/// can be found from its `Action` in O(1) without hashing.
///
/// Invariant: every action ever inserted stays in the heap for the
/// lifetime of the simulation. To express "never fires", call
/// [`IndexedHeap::update`] with `Priority::NEVER`; the node moves to
/// the bottom but isn't removed. Matches the fixed-cardinality LEF
/// model where actions are created once at init and never destroyed.
#[derive(Clone)]
pub(super) struct IndexedHeap {
    nodes: Vec<Node>,
    /// Heap index of `Action::Step { leg }`.
    step_pos: SecondaryMap<LegKey, usize>,
    /// Heap index of `Action::Respawn { lef }`.
    respawn_pos: SecondaryMap<LefKey, usize>,
    /// Heap index of `Action::Evict { leg }`.
    evict_pos: SecondaryMap<LegKey, usize>,
}

impl IndexedHeap {
    pub fn new() -> Self {
        IndexedHeap {
            nodes: Vec::new(),
            step_pos: SecondaryMap::new(),
            respawn_pos: SecondaryMap::new(),
            evict_pos: SecondaryMap::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        IndexedHeap {
            nodes: Vec::with_capacity(cap),
            step_pos: SecondaryMap::with_capacity(cap),
            respawn_pos: SecondaryMap::with_capacity(cap),
            evict_pos: SecondaryMap::with_capacity(cap),
        }
    }

    /// Insert a new action with the given priority.
    ///
    /// Debug-asserts that the action is not already in the heap.
    pub fn insert(&mut self, action: Action, priority: Priority) {
        debug_assert!(
            self.position(action).is_none(),
            "insert called on action already in heap: {action:?}"
        );
        let idx = self.nodes.len();
        self.nodes.push(Node { priority, action });
        self.set_position(action, idx);
        self.sift_up(idx);
    }

    /// Update an existing action's priority in place. Re-heapifies in
    /// O(log n). Panics if the action isn't already in the heap.
    pub fn update(&mut self, action: Action, priority: Priority) {
        let idx = self
            .position(action)
            .expect("update called on action not in heap");
        let old = self.nodes[idx].priority;
        self.nodes[idx].priority = priority;
        match priority.cmp(&old) {
            Ordering::Less => self.sift_up(idx),
            Ordering::Greater => self.sift_down(idx),
            Ordering::Equal => {}
        }
    }

    /// Peek the entry with the smallest priority (= soonest event time).
    /// `None` only if the heap is empty; if all entries are `NEVER`,
    /// returns `Some` with a non-finite priority — the caller decides
    /// whether to treat that as "no events left".
    pub fn peek(&self) -> Option<(Priority, Action)> {
        self.nodes.first().map(|n| (n.priority, n.action))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    // ----- internals -----

    fn position(&self, action: Action) -> Option<usize> {
        match action {
            Action::Step { leg } => self.step_pos.get(leg).copied(),
            Action::Respawn { lef } => self.respawn_pos.get(lef).copied(),
            Action::Evict { leg } => self.evict_pos.get(leg).copied(),
        }
    }

    fn set_position(&mut self, action: Action, idx: usize) {
        match action {
            Action::Step { leg } => {
                self.step_pos.insert(leg, idx);
            }
            Action::Respawn { lef } => {
                self.respawn_pos.insert(lef, idx);
            }
            Action::Evict { leg } => {
                self.evict_pos.insert(leg, idx);
            }
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        let action_a = self.nodes[a].action;
        let action_b = self.nodes[b].action;
        self.nodes.swap(a, b);
        self.set_position(action_a, b);
        self.set_position(action_b, a);
    }

    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if self.nodes[idx].priority < self.nodes[parent].priority {
                self.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut idx: usize) {
        let n = self.nodes.len();
        loop {
            let left = idx * 2 + 1;
            let right = idx * 2 + 2;
            let mut smallest = idx;
            if left < n && self.nodes[left].priority < self.nodes[smallest].priority {
                smallest = left;
            }
            if right < n && self.nodes[right].priority < self.nodes[smallest].priority {
                smallest = right;
            }
            if smallest != idx {
                self.swap(idx, smallest);
                idx = smallest;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    /// Mint `n` fresh `LegKey`s using a throwaway primary SlotMap.
    /// SecondaryMap accepts any key from the same key type, including
    /// keys minted by an unrelated map, since it identifies slots by the
    /// raw u32 + generation under the hood.
    fn n_leg_keys(n: usize) -> Vec<LegKey> {
        let mut m: SlotMap<LegKey, ()> = SlotMap::with_key();
        (0..n).map(|_| m.insert(())).collect()
    }

    fn step(k: LegKey) -> Action {
        Action::Step { leg: k }
    }

    #[test]
    fn empty_peek_is_none() {
        let h = IndexedHeap::new();
        assert!(h.peek().is_none());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn single_insert() {
        let ks = n_leg_keys(1);
        let mut h = IndexedHeap::new();
        h.insert(step(ks[0]), Priority(5.0));
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 5.0);
        assert_eq!(a, step(ks[0]));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn peek_returns_min_after_inserts() {
        let ks = n_leg_keys(5);
        let mut h = IndexedHeap::new();
        for (k, t) in ks.iter().zip([7.0, 2.0, 9.0, 1.0, 4.0]) {
            h.insert(step(*k), Priority(t));
        }
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 1.0);
        assert_eq!(a, step(ks[3]));
    }

    #[test]
    fn update_to_smaller_moves_to_top() {
        let ks = n_leg_keys(3);
        let mut h = IndexedHeap::new();
        h.insert(step(ks[0]), Priority(5.0));
        h.insert(step(ks[1]), Priority(3.0));
        h.insert(step(ks[2]), Priority(7.0));
        assert_eq!(h.peek().unwrap().1, step(ks[1]));

        h.update(step(ks[2]), Priority(0.5));
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 0.5);
        assert_eq!(a, step(ks[2]));
    }

    #[test]
    fn update_to_larger_moves_down() {
        let ks = n_leg_keys(3);
        let mut h = IndexedHeap::new();
        h.insert(step(ks[0]), Priority(1.0));
        h.insert(step(ks[1]), Priority(3.0));
        h.insert(step(ks[2]), Priority(7.0));
        assert_eq!(h.peek().unwrap().1, step(ks[0]));

        h.update(step(ks[0]), Priority(10.0));
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 3.0);
        assert_eq!(a, step(ks[1]));
    }

    #[test]
    fn update_to_never_keeps_entry_at_bottom() {
        let ks = n_leg_keys(2);
        let mut h = IndexedHeap::new();
        h.insert(step(ks[0]), Priority(1.0));
        h.insert(step(ks[1]), Priority(2.0));
        assert_eq!(h.peek().unwrap().1, step(ks[0]));

        h.update(step(ks[0]), Priority::NEVER);
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 2.0);
        assert_eq!(a, step(ks[1]));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn all_never_peek_is_not_finite() {
        let ks = n_leg_keys(3);
        let mut h = IndexedHeap::new();
        for k in &ks {
            h.insert(step(*k), Priority::NEVER);
        }
        let (p, _a) = h.peek().unwrap();
        assert!(!p.is_finite());
    }

    #[test]
    fn update_back_from_never() {
        let ks = n_leg_keys(2);
        let mut h = IndexedHeap::new();
        h.insert(step(ks[0]), Priority::NEVER);
        h.insert(step(ks[1]), Priority(5.0));
        assert_eq!(h.peek().unwrap().1, step(ks[1]));

        h.update(step(ks[0]), Priority(2.0));
        let (p, a) = h.peek().unwrap();
        assert_eq!(p.0, 2.0);
        assert_eq!(a, step(ks[0]));
    }

    /// Insert a mix of times, then drive a long sequence of updates and
    /// confirm `peek` always returns the running minimum.
    #[test]
    fn heap_property_after_many_updates() {
        let ks = n_leg_keys(20);
        let mut h = IndexedHeap::new();
        let times = [
            3.0, 7.0, 1.0, 9.0, 4.0, 2.0, 8.0, 6.0, 5.0, 0.0, 12.0, 14.0, 10.0, 18.0, 13.0, 11.0,
            17.0, 15.0, 16.0, 19.0,
        ];
        for (k, t) in ks.iter().zip(times.iter()) {
            h.insert(step(*k), Priority(*t));
        }
        assert_eq!(h.peek().unwrap().0.0, 0.0);

        h.update(step(ks[9]), Priority(100.0)); // was 0
        h.update(step(ks[5]), Priority(0.1)); // was 2
        assert_eq!(h.peek().unwrap().0.0, 0.1);

        h.update(step(ks[5]), Priority::NEVER);
        // Minimum is now whatever has the smallest remaining finite time.
        // After above updates, finite priorities are: 3, 7, 1, 9, 4, 8, 6,
        // 5, 12, 14, 10, 18, 13, 11, 17, 15, 16, 19, 100 → min is 1.
        assert_eq!(h.peek().unwrap().0.0, 1.0);
    }
}
