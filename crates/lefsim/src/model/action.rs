//! The [`Action`] enum, scheduling (pure), and firing (mutating) logic.

use rand::Rng;
use rand::distributions::OpenClosed01;
use smallvec::SmallVec;

use super::entity::{LefKey, LegKey, Mobility};
use super::lattice::{Direction, Position, SiteKind};
use super::spawn::Spawn;
use super::world::World;
use super::{Schedule, SimRng, Time};

#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Rates {
    /// Step rate of a moving, unblocked leg. A leg facing a CTCF from behind
    /// splits it between passing and eviction (see [`Action::Evict`]).
    pub step: f64,
    /// Respawn rate of a LEF with both legs free to move (the basal rate).
    pub respawn: f64,
    /// Respawn rate of a stalled LEF: either leg blocked (a leg directly
    /// ahead, or the lattice's end), the other free or blocked.
    pub respawn_stalled: f64,
    /// Respawn rate of a LEF with either leg captured, whatever the other leg
    /// is doing: capture protects a LEF from faster unloading, and from
    /// eviction.
    pub respawn_captured: f64,
}

impl Default for Rates {
    /// Reasonable demo rates: stepping is the fast process; respawns take
    /// thousands of steps; stalled LEFs unload ten times faster, and captured
    /// ones at the basal rate.
    fn default() -> Self {
        Rates {
            step: 1.0,
            respawn: 1e-3,
            respawn_stalled: 1e-2,
            respawn_captured: 1e-3,
        }
    }
}

/// A schedulable event.
///
/// `Action` is both the action *value* and the priority-queue *key*. Two
/// actions are equal iff they refer to the same entity-specific event.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum Action {
    /// A leg steps one cell in its `facing` direction.
    Step { leg: LegKey },
    /// A LEF unbinds and respawns at a new site sampled from the spawn rule.
    Respawn { lef: LefKey },
    /// A leg facing a CTCF from behind is evicted by it. Competes with the
    /// leg's step: a step at rate `k` splits into passing at `(1 - p) k` and
    /// eviction at `p k`, where `p` is the CTCF's strength (the chance it's
    /// bound), or `p = 1` if a captured leg holds it. Unless the other leg is
    /// captured, eviction unloads the LEF at once (it respawns in the same
    /// event); a captured partner protects it, and the leg waits, evicted.
    Evict { leg: LegKey },
}

impl Action {
    /// Stable `u8` encoding of the variant, used in recorded event frames:
    /// an index into [`crate::EventFrames::KIND_NAMES`] (`Keyframe` holds
    /// code 2, so `Evict` is 3).
    pub fn code(self) -> u8 {
        match self {
            Action::Step { .. } => 0,
            Action::Respawn { .. } => 1,
            Action::Evict { .. } => 3,
        }
    }

    /// Pure: compute when this action would next fire given the current world
    /// state and clock. Does not mutate.
    pub fn schedule(
        &self,
        world: &World,
        rates: &Rates,
        now: Time,
        rng: &mut SimRng,
    ) -> Schedule {
        match *self {
            Action::Step { leg } => schedule_step(leg, world, rates, now, rng),
            Action::Respawn { lef } => schedule_respawn(lef, world, rates, now, rng),
            Action::Evict { leg } => schedule_evict(leg, world, rates, now, rng),
        }
    }

    /// Mutating: apply this action's effects to the world. The action pushes
    /// every affected leg/LEF key into `touched` so the engine can reschedule
    /// dependents.
    pub(crate) fn fire(
        &self,
        world: &mut World,
        spawn: &dyn Spawn,
        _now: Time,
        rng: &mut SimRng,
        touched: &mut Touched,
    ) {
        match *self {
            Action::Step { leg } => fire_step(leg, world, rng, touched),
            Action::Respawn { lef } => fire_respawn(lef, world, spawn, rng, touched),
            Action::Evict { leg } => fire_evict(leg, world, spawn, rng, touched),
        }
    }
}

/// Set of entities mutated by a fired action. The model author pushes
/// every leg/LEF whose state changed (or whose neighborhood changed in a
/// way that affects scheduling) into this buffer. The engine then asks
/// it which actions to reschedule via [`Touched::dependent_actions`].
#[derive(Clone, Default, Debug)]
pub(crate) struct Touched {
    pub(crate) legs: SmallVec<[LegKey; 8]>,
    pub(crate) lefs: SmallVec<[LefKey; 4]>,
}

impl Touched {
    pub(crate) fn clear(&mut self) {
        self.legs.clear();
        self.lefs.clear();
    }

    /// The actions to reschedule after `fired` fired: `fired` itself first
    /// (its schedule depends on the state it just changed), then every
    /// action that depends on an entity in this set, each exactly once.
    ///
    /// This is the model's entity → action dependency map. Each reschedule
    /// draws a fresh random wait, so both the order and the absence of
    /// duplicates are part of a seeded trajectory.
    pub(crate) fn dependent_actions(&self, world: &World, fired: Action) -> SmallVec<[Action; 32]> {
        let from_legs = self.legs.iter().flat_map(|&leg_k| {
            let lef_k = world.legs[leg_k].lef;
            [Action::Step { leg: leg_k }, Action::Evict { leg: leg_k }, Action::Respawn { lef: lef_k }]
        });
        let from_lefs = self.lefs.iter().map(|&lef_k| Action::Respawn { lef: lef_k });

        let mut actions: SmallVec<[Action; 32]> = SmallVec::new();
        actions.push(fired);
        // A touched set is small (a handful of entities), so a linear scan
        // beats hashing.
        for action in from_legs.chain(from_lefs) {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        actions
    }
}

// ----- per-variant schedule implementations -----

/// What's on the site ahead of a leg, as far as stepping is concerned.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Ahead {
    /// An empty site that isn't a CTCF met from behind: the leg steps onto
    /// it. (A CTCF met from its capture side may capture it on landing.)
    Free,
    /// An empty CTCF met from behind (its capture side faces away from the
    /// leg), with this strength, the chance it's bound: the leg passes onto
    /// it or is evicted by it, competing.
    CtcfBack(f64),
    /// A CTCF met from behind and holding a captured leg: bound for certain,
    /// so the leg can only be evicted.
    CtcfBackHeld,
    /// Any other leg, or the lattice's end: the leg can't step.
    Blocked,
}

/// What's ahead of `leg_k`. A leg captured on a CTCF always faces back
/// toward the CTCF's other side, so a captured leg ahead on a CTCF met from
/// behind is exactly a held CTCF; any other leg ahead is traffic.
fn ahead(world: &World, leg_k: LegKey) -> Ahead {
    let leg = &world.legs[leg_k];
    let Some(target) = leg.facing.shift(leg.pos).filter(|&t| (t as usize) < world.lattice.len()) else {
        return Ahead::Blocked;
    };
    let back = match world.lattice.sites[target as usize].kind {
        SiteKind::Ctcf { captures_from, strength } if captures_from == leg.facing => Some(strength),
        _ => None,
    };
    let occupants = world.legs_at(target);
    match back {
        Some(strength) if occupants.is_empty() => Ahead::CtcfBack(strength),
        Some(_) if occupants.iter().any(|&o| world.legs[o].mobility == Mobility::Captured) => Ahead::CtcfBackHeld,
        _ if occupants.is_empty() => Ahead::Free,
        _ => Ahead::Blocked,
    }
}

/// Whether a leg is blocked: moving, but with another leg ahead (not a
/// captured one holding a CTCF met from behind) or the lattice's end.
/// Derived from positions, never stored, so it can't go stale as neighbors
/// move.
fn blocked(world: &World, leg_k: LegKey) -> bool {
    world.legs[leg_k].mobility == Mobility::Moving && ahead(world, leg_k) == Ahead::Blocked
}

/// An exponential wait at `rate`, or never for a rate of zero.
fn at_rate(rate: f64, now: Time, rng: &mut SimRng) -> Schedule {
    if rate > 0.0 {
        Schedule::At(now + sample_exponential(rng, rate))
    } else {
        Schedule::Never
    }
}

fn schedule_step(
    leg_k: LegKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    if world.legs[leg_k].mobility != Mobility::Moving {
        return Schedule::Never;
    }
    match ahead(world, leg_k) {
        Ahead::Free => at_rate(rates.step, now, rng),
        // Passing: the CTCF turns out unbound.
        Ahead::CtcfBack(strength) => at_rate((1.0 - strength) * rates.step, now, rng),
        Ahead::CtcfBackHeld | Ahead::Blocked => Schedule::Never,
    }
}

fn schedule_evict(
    leg_k: LegKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    if world.legs[leg_k].mobility != Mobility::Moving {
        return Schedule::Never;
    }
    match ahead(world, leg_k) {
        // The CTCF turns out bound...
        Ahead::CtcfBack(strength) => at_rate(strength * rates.step, now, rng),
        // ...or is, for certain.
        Ahead::CtcfBackHeld => at_rate(rates.step, now, rng),
        Ahead::Free | Ahead::Blocked => Schedule::Never,
    }
}

fn schedule_respawn(
    lef_k: LefKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    at_rate(respawn_rate(lef_k, world, rates), now, rng)
}

/// A LEF's respawn rate in the current state, the first that applies:
/// `respawn_captured` if either leg is captured (an evicted leg always has a
/// captured partner), `respawn_stalled` if either is blocked, else `respawn`.
fn respawn_rate(lef_k: LefKey, world: &World, rates: &Rates) -> f64 {
    let legs = world.lefs[lef_k].legs;
    if legs.iter().any(|&l| world.legs[l].mobility == Mobility::Captured) {
        rates.respawn_captured
    } else if legs.iter().any(|&l| blocked(world, l)) {
        rates.respawn_stalled
    } else {
        rates.respawn
    }
}

// ----- per-variant fire implementations -----

fn fire_step(
    leg_k: LegKey,
    world: &mut World,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    let (old_pos, lef_k, facing) = {
        let leg = &world.legs[leg_k];
        (leg.pos, leg.lef, leg.facing)
    };
    let new_pos = facing.shift(old_pos).expect("step scheduled at lattice boundary");

    // The legs whose way ahead this step changes: those behind the old site
    // (no longer blocked) and those beyond the new one (now blocked). Every
    // leg at each site, since a freshly placed LEF has both legs on one site
    // and either may be the one affected.
    let neighbors_at = |p: Option<Position>| -> SmallVec<[LegKey; 2]> {
        p.filter(|p| (*p as usize) < world.lattice.len())
            .map(|p| world.legs_at(p).iter().copied().collect())
            .unwrap_or_default()
    };
    let rear_neighbors = neighbors_at(facing.opposite().shift(old_pos));
    let post_neighbors = neighbors_at(facing.shift(new_pos));

    // Mutate occupancy and the leg. The capture roll happens here, exactly
    // once per actual move — never in scheduling.
    //
    // Under the current forward-only step model, the step direction equals
    // `facing`, so the leg arrived from the side opposite its facing. A
    // future model with back-stepping or independent step direction would
    // derive `arrived_from` from the actual step direction instead.
    world.remove_leg(leg_k, old_pos);
    world.insert_leg(leg_k, new_pos);
    let kind = world.lattice.sites[new_pos as usize].kind;
    let arrived_from = facing.opposite();
    let leg = &mut world.legs[leg_k];
    leg.pos = new_pos;
    leg.mobility = resolve_mobility(kind, arrived_from, rng);

    // Tell the engine what to reschedule.
    touched.legs.push(leg_k);
    touched.lefs.push(lef_k);
    touched.legs.extend(rear_neighbors.into_iter().chain(post_neighbors).filter(|&n| n != leg_k));
}

/// Eviction: a captured partner protects the LEF, and the leg waits where it
/// stands, evicted (it never moves again, and its LEF keeps unloading at
/// `respawn_captured`). Otherwise the LEF unloads at once, respawning in the
/// same event, so no transient state is ever stored.
fn fire_evict(
    leg_k: LegKey,
    world: &mut World,
    spawn: &dyn Spawn,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    let lef_k = world.legs[leg_k].lef;
    let protected = world.lefs[lef_k].legs.iter().any(|&l| l != leg_k && world.legs[l].mobility == Mobility::Captured);
    if protected {
        world.legs[leg_k].mobility = Mobility::Evicted;
        // Nothing moved: only this leg's own actions change (its LEF's rate
        // is the captured one either way).
        touched.legs.push(leg_k);
    } else {
        fire_respawn(lef_k, world, spawn, rng, touched);
    }
}

fn fire_respawn(
    lef_k: LefKey,
    world: &mut World,
    spawn: &dyn Spawn,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    let [l1, l2] = world.lefs[lef_k].legs;
    let p1 = world.legs[l1].pos;
    let p2 = world.legs[l2].pos;

    // Touch neighbors of both old positions; then vacate.
    for p in [p1, p2] {
        for d in [Direction::Left, Direction::Right] {
            if let Some(np) = d.shift(p)
                && (np as usize) < world.lattice.len()
            {
                for &n in world.legs_at(np) {
                    if n != l1 && n != l2 {
                        touched.legs.push(n);
                    }
                }
            }
        }
    }
    world.remove_leg(l1, p1);
    world.remove_leg(l2, p2);

    let new_pos = spawn.sample(world, rng);

    // Touch neighbors of the new spawn site.
    for d in [Direction::Left, Direction::Right] {
        if let Some(np) = d.shift(new_pos)
            && (np as usize) < world.lattice.len()
        {
            for &n in world.legs_at(np) {
                touched.legs.push(n);
            }
        }
    }

    // Place both legs at the spawn site, landing as initial placement does.
    let kind = world.lattice.sites[new_pos as usize].kind;
    let mobility = land_lef(kind, rng);
    for (l, m) in [l1, l2].into_iter().zip(mobility) {
        let leg = &mut world.legs[l];
        leg.pos = new_pos;
        leg.mobility = m;
    }
    world.insert_leg(l1, new_pos);
    world.insert_leg(l2, new_pos);

    touched.legs.extend([l1, l2]);
    touched.lefs.push(lef_k);
}

/// Sample an exponential dt with the given rate (events per unit time).
fn sample_exponential(rng: &mut SimRng, rate: f64) -> Time {
    // u ~ (0, 1] avoids ln(0).
    let u: f64 = rng.sample(OpenClosed01);
    // `libm::log` rather than `f64::ln`: `ln` calls the platform's libm,
    // which can differ in the last ulp between targets (e.g. macOS vs
    // wasm32). Event times feed heap ordering, so that would let the same
    // seed produce different trajectories on different targets. This is
    // the only transcendental in the simulation; everything else is exact
    // IEEE arithmetic or integer bit manipulation.
    -libm::log(u) / rate
}

// ----- landing on a site -----

/// The mobilities of a LEF's left and right legs as it lands, both at once,
/// on a site of `kind`: at initial placement ([`World::spawn_lef`]) and at
/// respawn (`fire_respawn`). Sharing it means the two can't diverge.
///
/// Each leg is treated as if it arrived from the side opposite its facing,
/// as under forward-only stepping. Rolls the left leg, then the right.
pub(crate) fn land_lef(kind: SiteKind, rng: &mut SimRng) -> [Mobility; 2] {
    [
        resolve_mobility(kind, Direction::Left.opposite(), rng),
        resolve_mobility(kind, Direction::Right.opposite(), rng),
    ]
}

/// Stochastic resolution of a leg's mobility *at the moment it lands at a site*.
///
/// Takes `arrived_from` — the side of the destination cell from which the
/// leg arrived — rather than the leg's facing. The caller decides how to
/// derive that from its step model; in the current forward-only stepping
/// model, `arrived_from == leg.facing.opposite()`, but a model with
/// back-stepping or independent step direction would derive it differently.
///
/// # Invariant
///
/// **Only call this once per actual landing.** Capture is
/// modeled as events that fire *on the act of stepping onto a site* — not
/// as facts derivable from position. Calling this function on a leg that
/// hasn't moved would give it repeated, independent chances of being
/// captured, which is physically wrong: a leg standing still on a
/// CTCF that already failed to capture it must not get fresh rolls each
/// time the scheduler revisits it.
///
/// Concretely: never call from any `schedule_*` function or any other
/// non-mutating context. The correct call sites are exactly:
///
/// - `fire_step` — leg stepped to a new cell
/// - [`land_lef`] — a LEF placed its legs at a spawn cell, initially or
///   on respawn
///
/// # Semantics
///
/// - `Plain` sites: always returns `Moving`.
/// - `Ctcf { captures_from, strength }`: arrived from the capture side
///   (`arrived_from == captures_from`), `Captured` with probability
///   `strength`, else `Moving`. Arrived from the other side, `Moving`: a
///   leg stepping there won the race against eviction ([`Action::Evict`]),
///   and binding there can't evict.
pub(crate) fn resolve_mobility(
    site: SiteKind,
    arrived_from: Direction,
    rng: &mut SimRng,
) -> Mobility {
    match site {
        SiteKind::Plain => Mobility::Moving,
        SiteKind::Ctcf { captures_from, strength } => {
            if arrived_from == captures_from && rng.gen_bool(strength) {
                Mobility::Captured
            } else {
                Mobility::Moving
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Lattice, SpawnRule};
    use rand::SeedableRng;
    use Mobility::Moving;

    #[test]
    fn dependent_actions_start_with_the_fired_action_and_have_no_duplicates() {
        let mut rng = SimRng::seed_from_u64(0);
        let mut world = World::new(Lattice::plain(20).unwrap());
        let (lef, [left, right]) = world.spawn_lef(10, &mut rng);
        let mut touched = Touched::default();
        touched.legs.extend([left, right, left]); // a leg touched twice
        touched.lefs.push(lef); // and the LEF both legs belong to

        let fired = Action::Respawn { lef };
        let deps = touched.dependent_actions(&world, fired);
        assert_eq!(
            deps.as_slice(),
            &[fired, Action::Step { leg: left }, Action::Evict { leg: left }, Action::Step { leg: right }, Action::Evict { leg: right }]
        );
    }

    /// Distinct rates, so the chosen one names the rule that chose it.
    const RATES: Rates = Rates { step: 1.0, respawn: 1.0, respawn_stalled: 2.0, respawn_captured: 3.0 };

    /// A LEF as `((left_pos, mobility), (right_pos, mobility))`.
    type LefSpec = ((u32, Mobility), (u32, Mobility));

    /// 40 sites; CTCFs capturing from the left at 20 and 25 (each captures
    /// right-facing legs arriving from the left, and evicts left-facing legs
    /// meeting it from the right), at `strength`.
    fn lattice(strength: f64) -> Lattice {
        let ctcf = |site| crate::model::Ctcf { site, captures_from: Direction::Left, strength };
        Lattice::with_ctcfs(40, &[ctcf(20), ctcf(25)]).unwrap()
    }

    fn world_on(lattice: Lattice, lefs: &[LefSpec]) -> (World, Vec<LefKey>) {
        let mut world = World::new(lattice);
        let keys = lefs.iter().map(|&(l, r)| world.add_lef(l, r).unwrap()).collect();
        (world, keys)
    }

    fn world_with(lefs: &[LefSpec]) -> (World, Vec<LefKey>) {
        world_on(lattice(1.0), lefs)
    }

    /// Whether `action` is scheduled to fire (rather than never).
    fn scheduled(world: &World, action: Action) -> bool {
        let mut rng = SimRng::seed_from_u64(0);
        matches!(action.schedule(world, &RATES, 0.0, &mut rng), Schedule::At(_))
    }

    /// Whether a leg's step and eviction are scheduled, as `(step, evict)`.
    fn clocks(world: &World, leg: LegKey) -> (bool, bool) {
        (scheduled(world, Action::Step { leg }), scheduled(world, Action::Evict { leg }))
    }

    #[test]
    fn respawn_rate_takes_the_first_rule_that_applies() {
        use Mobility::{Captured, Evicted, Moving};
        let rate = |lefs: &[LefSpec], which: usize| {
            let (world, keys) = world_with(lefs);
            respawn_rate(keys[which], &world, &RATES)
        };
        // Both legs free: basal.
        assert_eq!(rate(&[((5, Moving), (8, Moving))], 0), RATES.respawn);
        assert_eq!(rate(&[((5, Moving), (8, Moving)), ((10, Moving), (12, Moving))], 1), RATES.respawn);
        // Either leg blocked, by a leg ahead or the lattice's end: stalled.
        assert_eq!(rate(&[((5, Moving), (8, Moving)), ((9, Moving), (12, Moving))], 0), RATES.respawn_stalled);
        assert_eq!(rate(&[((0, Moving), (8, Moving))], 0), RATES.respawn_stalled);
        let rosette = [((2, Moving), (5, Moving)), ((6, Moving), (9, Moving)), ((10, Moving), (13, Moving))];
        for lef in 0..3 {
            assert_eq!(rate(&rosette, lef), RATES.respawn_stalled, "LEF {lef}");
        }
        // Facing a CTCF from behind isn't blocked: an eviction is pending.
        assert_eq!(rate(&[((26, Moving), (30, Moving))], 0), RATES.respawn);
        // Either leg captured: captured, over blocked, and with an evicted leg.
        assert_eq!(rate(&[((0, Moving), (20, Captured))], 0), RATES.respawn_captured);
        assert_eq!(rate(&[((21, Evicted), (25, Captured))], 0), RATES.respawn_captured);
    }

    #[test]
    fn step_and_eviction_split_the_encounter_with_a_ctcf_from_behind() {
        use Mobility::{Captured, Evicted, Moving};
        // A plain site ahead: only a step.
        let (world, keys) = world_with(&[((5, Moving), (8, Moving))]);
        assert_eq!(clocks(&world, world.lefs[keys[0]].legs[0]), (true, false));
        // A bare CTCF from behind (the left leg at 26, facing 25): both clocks
        // at strengths in (0, 1), only eviction at 1, only the step at 0.
        for (strength, expected) in [(0.5, (true, true)), (1.0, (false, true)), (0.0, (true, false))] {
            let (world, keys) = world_on(lattice(strength), &[((26, Moving), (30, Moving))]);
            assert_eq!(clocks(&world, world.lefs[keys[0]].legs[0]), expected, "strength {strength}");
        }
        // A CTCF held by a captured leg: only eviction, even at strength 0.
        let (world, keys) = world_on(lattice(0.0), &[((10, Moving), (25, Captured)), ((26, Moving), (30, Moving))]);
        assert_eq!(clocks(&world, world.lefs[keys[1]].legs[0]), (false, true));
        // A moving leg on the CTCF is traffic: neither, and blocked.
        let (world, keys) = world_with(&[((25, Moving), (27, Moving)), ((26, Moving), (26, Moving))]);
        let behind = world.lefs[keys[1]].legs[0];
        assert_eq!(clocks(&world, behind), (false, false));
        assert!(blocked(&world, behind));
        // An evicted leg, and a leg at the lattice's end: neither.
        let (world, keys) = world_with(&[((21, Evicted), (25, Captured)), ((30, Moving), (39, Moving))]);
        assert_eq!(clocks(&world, world.lefs[keys[0]].legs[0]), (false, false));
        assert_eq!(clocks(&world, world.lefs[keys[1]].legs[1]), (false, false));
    }

    #[test]
    fn eviction_wins_the_race_with_probability_strength() {
        // Competing clocks at (1 - p) k and p k: eviction comes first with
        // probability p, as a single step at k followed by a coin flip would.
        let p = 0.3;
        let (world, keys) = world_on(lattice(p), &[((26, Moving), (30, Moving))]);
        let leg = world.lefs[keys[0]].legs[0];
        let mut rng = SimRng::seed_from_u64(9);
        let n = 20_000;
        let mut evicted_first = 0;
        for _ in 0..n {
            let time = |action: Action, rng: &mut SimRng| match action.schedule(&world, &RATES, 0.0, rng) {
                Schedule::At(t) => t,
                Schedule::Never => f64::INFINITY,
            };
            if time(Action::Evict { leg }, &mut rng) < time(Action::Step { leg }, &mut rng) {
                evicted_first += 1;
            }
        }
        let frac = evicted_first as f64 / n as f64;
        let sigma = (p * (1.0 - p) / n as f64).sqrt();
        assert!((frac - p).abs() < 4.0 * sigma, "eviction first in {frac}, want {p} (σ {sigma:.4})");
    }

    /// Fire an eviction of `leg`.
    fn evict(world: &mut World, leg: LegKey) {
        let mut rng = SimRng::seed_from_u64(1);
        fire_evict(leg, world, &SpawnRule::UniformNonCtcf, &mut rng, &mut Touched::default());
    }

    #[test]
    fn an_unprotected_eviction_unloads_the_lef_at_once() {
        let (mut world, keys) = world_with(&[((26, Moving), (30, Moving))]);
        let [left, right] = world.lefs[keys[0]].legs;
        evict(&mut world, left);
        // Respawned: both legs on one new site, moving.
        let (l, r) = (world.legs[left], world.legs[right]);
        assert_eq!(l.pos, r.pos);
        assert_ne!(l.pos, 26);
        assert_eq!((l.mobility, r.mobility), (Mobility::Moving, Mobility::Moving));
        assert!(!world.occupied(30));
    }

    #[test]
    fn a_captured_partner_protects_and_the_leg_waits_evicted() {
        // The left leg at 21 faces the CTCF at 20 from behind; its partner is
        // captured at 25.
        let (mut world, keys) = world_with(&[((21, Moving), (25, Mobility::Captured))]);
        let [left, right] = world.lefs[keys[0]].legs;
        evict(&mut world, left);
        assert_eq!((world.legs[left].pos, world.legs[left].mobility), (21, Mobility::Evicted));
        assert_eq!((world.legs[right].pos, world.legs[right].mobility), (25, Mobility::Captured));
    }

    #[test]
    fn stepping_onto_a_ctcf_from_behind_passes_it() {
        // At strength 0 the leg passes onto the CTCF, still moving.
        let (mut world, keys) = world_on(lattice(0.0), &[((26, Moving), (30, Moving))]);
        let leg = world.lefs[keys[0]].legs[0];
        let mut rng = SimRng::seed_from_u64(1);
        fire_step(leg, &mut world, &mut rng, &mut Touched::default());
        assert_eq!((world.legs[leg].pos, world.legs[leg].mobility), (25, Mobility::Moving));
    }

    #[test]
    fn plain_site_always_moving() {
        let mut rng = SimRng::seed_from_u64(0);
        assert_eq!(resolve_mobility(SiteKind::Plain, Direction::Right, &mut rng), Mobility::Moving);
        assert_eq!(resolve_mobility(SiteKind::Plain, Direction::Left, &mut rng), Mobility::Moving);
    }

    #[test]
    fn landing_captures_only_from_the_capture_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // The CTCF's capture side is its right side.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(resolve_mobility(site, Direction::Right, &mut rng), Mobility::Captured);
        assert_eq!(resolve_mobility(site, Direction::Left, &mut rng), Mobility::Moving);
    }

    #[test]
    fn strength_0_always_passes_through() {
        let mut rng = SimRng::seed_from_u64(0);
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 0.0 };
        for _ in 0..50 {
            assert_eq!(resolve_mobility(site, Direction::Right, &mut rng), Mobility::Moving);
            assert_eq!(resolve_mobility(site, Direction::Left, &mut rng), Mobility::Moving);
        }
    }

    #[test]
    fn capture_fraction_tracks_strength() {
        let mut rng = SimRng::seed_from_u64(42);
        let p = 0.3;
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: p };
        // Arrive from the capture side so the outcome on success is Captured.
        let n = 50_000;
        let mut captured = 0usize;
        for _ in 0..n {
            if resolve_mobility(site, Direction::Right, &mut rng) == Mobility::Captured {
                captured += 1;
            }
        }
        let frac = captured as f64 / n as f64;
        // ~4 sigma for a Bernoulli(p) sample mean.
        let sigma = (p * (1.0 - p) / n as f64).sqrt();
        assert!(
            (frac - p).abs() < 4.0 * sigma,
            "capture fraction {frac} differs from p={p} by more than 4σ ({sigma:.4})"
        );
    }
}
