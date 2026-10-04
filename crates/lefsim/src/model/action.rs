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
    /// Step rate of a moving, unblocked leg.
    pub step: f64,
    /// Respawn rate of a LEF with both legs free to move (the basal rate).
    pub respawn: f64,
    /// Respawn rate of a stalled LEF: either leg blocked (a leg directly
    /// ahead, or the lattice's end), the other free or blocked.
    pub respawn_stalled: f64,
    /// Respawn rate of a LEF with either leg evicted (stopped by a CTCF it
    /// met from behind), unless the other is captured.
    pub respawn_evicted: f64,
    /// Respawn rate of a LEF with either leg captured, whatever the other leg
    /// is doing: capture protects a LEF from faster unloading.
    pub respawn_captured: f64,
}

impl Default for Rates {
    /// Reasonable demo rates: stepping is the fast process; respawns take
    /// thousands of steps; stalled LEFs unload ten times faster, evicted ones
    /// a hundred times faster, and captured ones at the basal rate.
    fn default() -> Self {
        Rates {
            step: 1.0,
            respawn: 1e-3,
            respawn_stalled: 1e-2,
            respawn_evicted: 1e-1,
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
    /// A leg attempts to step one cell in its `facing` direction.
    Step { leg: LegKey },
    /// A LEF unbinds and respawns at a new site sampled from the spawn rule.
    Respawn { lef: LefKey },
}

impl Action {
    /// Names indexed by [`Action::code`].
    pub const NAMES: [&str; 2] = ["Step", "Respawn"];

    /// Stable `u8` encoding of the variant, used in recorded event frames.
    pub fn code(self) -> u8 {
        match self {
            Action::Step { .. } => 0,
            Action::Respawn { .. } => 1,
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
    pub(crate) fn dependent_actions(&self, world: &World, fired: Action) -> SmallVec<[Action; 16]> {
        let from_legs = self.legs.iter().flat_map(|&leg_k| {
            let lef_k = world.legs[leg_k].lef;
            [Action::Step { leg: leg_k }, Action::Respawn { lef: lef_k }]
        });
        let from_lefs = self.lefs.iter().map(|&lef_k| Action::Respawn { lef: lef_k });

        let mut actions: SmallVec<[Action; 16]> = SmallVec::new();
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
    /// An empty site: the leg can step onto it. A CTCF there acts on landing
    /// (capture from its capture side, eviction from the other).
    Free,
    /// A CTCF met from behind and holding a captured leg: certainly bound,
    /// so stepping evicts the leg where it stands, without moving.
    CtcfBackHeld,
    /// Any other leg (moving, or evicted onto a CTCF), or the lattice's end:
    /// the leg can't step.
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
    let occupants = world.legs_at(target);
    if occupants.is_empty() {
        return Ahead::Free;
    }
    let ctcf_back = matches!(world.lattice.sites[target as usize].kind,
                             SiteKind::Ctcf { captures_from, .. } if captures_from == leg.facing);
    if ctcf_back && occupants.iter().any(|&o| world.legs[o].mobility == Mobility::Captured) {
        Ahead::CtcfBackHeld
    } else {
        Ahead::Blocked
    }
}

/// Whether a leg is blocked: moving, but unable to step (a leg directly
/// ahead that isn't a held CTCF's, or the lattice's end). Derived from
/// positions, never stored, so it can't go stale as neighbors move.
fn blocked(world: &World, leg_k: LegKey) -> bool {
    world.legs[leg_k].mobility == Mobility::Moving && ahead(world, leg_k) == Ahead::Blocked
}

fn schedule_step(
    leg_k: LegKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    if world.legs[leg_k].mobility != Mobility::Moving || ahead(world, leg_k) == Ahead::Blocked {
        return Schedule::Never;
    }
    Schedule::At(now + sample_exponential(rng, rates.step))
}

fn schedule_respawn(
    lef_k: LefKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    Schedule::At(now + sample_exponential(rng, respawn_rate(lef_k, world, rates)))
}

/// A LEF's respawn rate in the current state, the first that applies:
/// `respawn_captured` if either leg is captured, `respawn_evicted` if either
/// is evicted, `respawn_stalled` if either is blocked, else `respawn`.
fn respawn_rate(lef_k: LefKey, world: &World, rates: &Rates) -> f64 {
    let legs = world.lefs[lef_k].legs;
    let any = |m: Mobility| legs.iter().any(|&l| world.legs[l].mobility == m);
    if any(Mobility::Captured) {
        rates.respawn_captured
    } else if any(Mobility::Evicted) {
        rates.respawn_evicted
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
    let lef_k = world.legs[leg_k].lef;

    // A CTCF met from behind that holds a captured leg is bound for
    // certain: the leg is evicted where it stands, without moving. (An empty
    // CTCF met from behind is an ordinary step; it may evict on landing.)
    match ahead(world, leg_k) {
        Ahead::CtcfBackHeld => {
            world.legs[leg_k].mobility = Mobility::Evicted;
            // Nothing moved: only this leg and its LEF's rate change.
            touched.legs.push(leg_k);
            touched.lefs.push(lef_k);
            return;
        }
        Ahead::Free => {}
        Ahead::Blocked => unreachable!("step scheduled for a blocked leg"),
    }

    let (old_pos, facing) = {
        let leg = &world.legs[leg_k];
        (leg.pos, leg.facing)
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

    // Mutate occupancy and the leg. The CTCF roll happens here, exactly once
    // per actual move — never in scheduling.
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
    leg.mobility = resolve_mobility(kind, arrived_from, Landing::Step, rng);

    // Tell the engine what to reschedule.
    touched.legs.push(leg_k);
    touched.lefs.push(lef_k);
    touched.legs.extend(rear_neighbors.into_iter().chain(post_neighbors).filter(|&n| n != leg_k));
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
/// as under forward-only stepping. Binding isn't stepping, so a CTCF can
/// capture a landing leg but never evict it. Rolls the left leg, then the
/// right.
pub(crate) fn land_lef(kind: SiteKind, rng: &mut SimRng) -> [Mobility; 2] {
    [
        resolve_mobility(kind, Direction::Left.opposite(), Landing::Spawn, rng),
        resolve_mobility(kind, Direction::Right.opposite(), Landing::Spawn, rng),
    ]
}

/// How a leg came to be on a site: by stepping onto it, or by binding there.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum Landing {
    Step,
    Spawn,
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
/// - `Ctcf { captures_from, strength }`, with probability `strength` (a
///   crude stand-in for whether the CTCF is bound), else `Moving`:
///   - arrived from the capture side (`arrived_from == captures_from`):
///     `Captured`;
///   - arrived from the other side, by a step: `Evicted`, on the CTCF. By
///     binding (`Landing::Spawn`), `Moving`: binding can't evict.
pub(crate) fn resolve_mobility(
    site: SiteKind,
    arrived_from: Direction,
    landing: Landing,
    rng: &mut SimRng,
) -> Mobility {
    match site {
        SiteKind::Plain => Mobility::Moving,
        SiteKind::Ctcf { captures_from, strength } => {
            let capture_side = arrived_from == captures_from;
            // Binding can't evict, so it doesn't roll on the eviction side.
            if (!capture_side && landing == Landing::Spawn) || !rng.gen_bool(strength) {
                Mobility::Moving
            } else if capture_side {
                Mobility::Captured
            } else {
                Mobility::Evicted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Lattice;
    use rand::SeedableRng;

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
            &[fired, Action::Step { leg: left }, Action::Step { leg: right }]
        );
    }

    /// Distinct rates, so the chosen one names the rule that chose it.
    const RATES: Rates = Rates { step: 1.0, respawn: 1.0, respawn_stalled: 2.0, respawn_evicted: 3.0, respawn_captured: 4.0 };

    /// A LEF as `((left_pos, mobility), (right_pos, mobility))`.
    type LefSpec = ((u32, Mobility), (u32, Mobility));

    /// 40 sites; CTCFs capturing from the left at 20 and 25 (each captures
    /// right-facing legs arriving from the left, and evicts left-facing legs
    /// meeting it from the right, which wait at 21 or 26), at `strength`.
    fn lattice(strength: f64) -> Lattice {
        let ctcf = |site| crate::model::Ctcf { site, captures_from: Direction::Left, strength };
        Lattice::with_ctcfs(40, &[ctcf(20), ctcf(25)]).unwrap()
    }

    fn world_with(lefs: &[LefSpec]) -> (World, Vec<LefKey>) {
        world_on(lattice(1.0), lefs)
    }

    fn world_on(lattice: Lattice, lefs: &[LefSpec]) -> (World, Vec<LefKey>) {
        let mut world = World::new(lattice);
        let keys = lefs.iter().map(|&(l, r)| world.add_lef(l, r).unwrap()).collect();
        (world, keys)
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
        // Two LEFs near each other but not touching: both free.
        assert_eq!(rate(&[((5, Moving), (8, Moving)), ((10, Moving), (12, Moving))], 1), RATES.respawn);
        // Either leg blocked, by a leg ahead or the lattice's end: stalled.
        assert_eq!(rate(&[((5, Moving), (8, Moving)), ((9, Moving), (12, Moving))], 0), RATES.respawn_stalled);
        assert_eq!(rate(&[((0, Moving), (8, Moving))], 0), RATES.respawn_stalled);
        // A rosette: every LEF has a leg blocked at a junction.
        let rosette = [((2, Moving), (5, Moving)), ((6, Moving), (9, Moving)), ((10, Moving), (13, Moving))];
        for lef in 0..3 {
            assert_eq!(rate(&rosette, lef), RATES.respawn_stalled, "LEF {lef}");
        }
        // Either leg evicted: evicted, over blocked.
        assert_eq!(rate(&[((21, Evicted), (24, Moving))], 0), RATES.respawn_evicted);
        assert_eq!(rate(&[((21, Evicted), (24, Moving)), ((25, Moving), (30, Moving))], 0), RATES.respawn_evicted);
        // Either leg captured: captured, over evicted and blocked.
        assert_eq!(rate(&[((21, Evicted), (25, Captured))], 0), RATES.respawn_captured);
        assert_eq!(rate(&[((0, Moving), (20, Captured))], 0), RATES.respawn_captured);
    }

    #[test]
    fn who_may_step() {
        use Mobility::{Captured, Evicted, Moving};
        let mut rng = SimRng::seed_from_u64(0);
        let steps = |world: &World, leg, rng: &mut SimRng| matches!(Action::Step { leg }.schedule(world, &RATES, 0.0, rng), Schedule::At(_));
        // LEF 0: left leg evicted at 21; right leg at 24 facing the CTCF at 25
        // from its capture side (free). LEF 1: left leg at 26 facing the CTCF
        // at 25 from behind, and that CTCF is empty; right leg at the end.
        let (world, keys) = world_with(&[((21, Evicted), (24, Moving)), ((26, Moving), (39, Moving))]);
        let [evicted, free] = world.lefs[keys[0]].legs;
        let [ctcf_back, at_end] = world.lefs[keys[1]].legs;
        assert!(!steps(&world, evicted, &mut rng), "an evicted leg waits");
        assert!(steps(&world, free, &mut rng));
        assert!(steps(&world, ctcf_back, &mut rng), "an empty CTCF from behind is an ordinary step");
        assert!(!steps(&world, at_end, &mut rng), "the lattice's end blocks");
        // A captured leg on the CTCF at 20: the left leg behind it, at 21,
        // may still attempt (and will be evicted); a leg behind a moving leg
        // is just blocked.
        let (world, keys) = world_with(&[((10, Moving), (20, Captured)), ((21, Moving), (23, Moving)), ((24, Moving), (28, Moving))]);
        let [held_back, blocked] = world.lefs[keys[1]].legs;
        assert!(steps(&world, held_back, &mut rng));
        assert!(!steps(&world, blocked, &mut rng), "23 is blocked by the leg at 24");
        assert!(!super::blocked(&world, held_back), "an attempt is pending, so not blocked");
        // A moving leg, or one evicted onto it, on the CTCF at 25 is traffic:
        // a left leg meeting the CTCF from behind (a fresh LEF at 26) is blocked.
        for occupant in [Moving, Evicted] {
            let (world, keys) = world_with(&[((25, occupant), (27, Moving)), ((26, Moving), (26, Moving))]);
            let [behind, _] = world.lefs[keys[1]].legs;
            assert!(!steps(&world, behind, &mut rng), "{occupant:?} on the CTCF: blocked");
            assert!(super::blocked(&world, behind));
        }
    }

    /// Fire a step of `leg` and return its position and mobility after.
    fn step(world: &mut World, leg: LegKey, seed: u64) -> (Position, Mobility) {
        let mut rng = SimRng::seed_from_u64(seed);
        fire_step(leg, world, &mut rng, &mut Touched::default());
        (world.legs[leg].pos, world.legs[leg].mobility)
    }

    #[test]
    fn meeting_a_ctcf_from_behind() {
        use Mobility::{Captured, Evicted, Moving};
        // A bare CTCF, strength 1: the leg steps onto it and is evicted there.
        let (mut world, keys) = world_on(lattice(1.0), &[((26, Moving), (30, Moving))]);
        let leg = world.lefs[keys[0]].legs[0];
        assert_eq!(step(&mut world, leg, 1), (25, Evicted));
        // Strength 0: steps onto it, still moving.
        let (mut world, keys) = world_on(lattice(0.0), &[((26, Moving), (30, Moving))]);
        let leg = world.lefs[keys[0]].legs[0];
        assert_eq!(step(&mut world, leg, 1), (25, Moving));
        // Strength 0, but a captured leg holds the CTCF: evicted for certain,
        // in place (no step).
        let (world, keys) = world_on(lattice(0.0), &[((10, Moving), (25, Captured)), ((26, Moving), (30, Moving))]);
        let leg = world.lefs[keys[1]].legs[0];
        for seed in 0..20 {
            assert_eq!(step(&mut world.clone(), leg, seed), (26, Evicted), "seed {seed}");
        }
    }

    #[test]
    fn plain_site_always_moving() {
        let mut rng = SimRng::seed_from_u64(0);
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Right, Landing::Step, &mut rng),
            Mobility::Moving
        );
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Left, Landing::Step, &mut rng),
            Mobility::Moving
        );
    }

    #[test]
    fn strength_1_captures_leg_arriving_on_capture_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // CTCF's capture side is its right side. A leg that arrived from
        // the right should be captured.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(
            resolve_mobility(site, Direction::Right, Landing::Step, &mut rng),
            Mobility::Captured
        );
    }

    #[test]
    fn landing_from_behind_evicts_a_step_but_not_a_spawn() {
        let mut rng = SimRng::seed_from_u64(0);
        // The CTCF's capture side is its right side: a leg arriving from the
        // left is evicted if it stepped there, but binding there can't evict.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(resolve_mobility(site, Direction::Left, Landing::Step, &mut rng), Mobility::Evicted);
        assert_eq!(resolve_mobility(site, Direction::Left, Landing::Spawn, &mut rng), Mobility::Moving);
        assert_eq!(resolve_mobility(site, Direction::Right, Landing::Spawn, &mut rng), Mobility::Captured);
    }

    #[test]
    fn strength_0_always_passes_through() {
        let mut rng = SimRng::seed_from_u64(0);
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 0.0 };
        for _ in 0..50 {
            assert_eq!(resolve_mobility(site, Direction::Right, Landing::Step, &mut rng), Mobility::Moving);
            assert_eq!(resolve_mobility(site, Direction::Left, Landing::Step, &mut rng), Mobility::Moving);
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
            if resolve_mobility(site, Direction::Right, Landing::Step, &mut rng) == Mobility::Captured {
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
