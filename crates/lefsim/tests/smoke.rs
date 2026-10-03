//! Integration tests against the public API.

use lefsim::*;

/// `(left, right)` positions and mobilities of LEF 0.
fn lef0(sim: &Sim) -> ((Position, Position), (Mobility, Mobility)) {
    let ((l, lm), (r, rm)) = sim.world().lef_state(0).unwrap();
    ((l, r), (lm, rm))
}

/// A sim with one moving LEF with both legs at `pos`.
fn one_lef(lattice: Lattice, pos: Position, rates: Rates, seed: u64) -> Sim {
    let mut world = World::new(lattice);
    world.add_lef((pos, Mobility::Moving), (pos, Mobility::Moving)).unwrap();
    Sim::new(Model::new(world, rates, SpawnRule::UniformNonCtcf).unwrap(), seed)
}

/// On an empty lattice with a single LEF spawning at the midpoint, the legs
/// should — at some point during the run — separate outward from their
/// starting site. We track the extreme positions reached because the run
/// will eventually trigger respawns once both legs hit the lattice
/// boundary, after which positions reset.
#[test]
fn lef_extrudes_outward_on_empty_lattice() {
    let mut sim = one_lef(
        Lattice::plain(200).unwrap(),
        100,
        Rates {
            step: 1.0,
            // Effectively never within the test horizon (~1e9 expected
            // wait), but a respawn will still fire once step actions are
            // exhausted at the boundaries.
            respawn: 1e-9,
            respawn_captured: 1e-9,
        },
        7,
    );

    let (mut leftmost, mut rightmost) = lef0(&sim).0;
    for _ in 0..400 {
        if sim.step().is_none() {
            break;
        }
        let (l, r) = lef0(&sim).0;
        leftmost = leftmost.min(l);
        rightmost = rightmost.max(r);
        if leftmost < 100 && rightmost > 100 {
            return;
        }
    }
    panic!(
        "never observed outward extrusion: leftmost={leftmost}, rightmost={rightmost}"
    );
}

/// A right-facing leg approaching a CTCF whose capture side faces left
/// (i.e. captures legs arriving from the left) should arrive captured.
#[test]
fn right_facing_leg_captured_at_compatible_ctcf() {
    let mut sites = vec![Site::plain(); 50];
    sites[20] = Site::ctcf(Direction::Left, 1.0);
    let mut sim = one_lef(
        Lattice::new(sites).unwrap(),
        10,
        Rates {
            step: 1.0,
            respawn: 1e-12,
            respawn_captured: 1e-12,
        },
        11,
    );

    for _ in 0..2000 {
        sim.step().expect("queue should not exhaust within horizon");
        if lef0(&sim).0.1 == 20 {
            break;
        }
    }
    assert_eq!(lef0(&sim).0.1, 20, "right leg should reach CTCF");
    assert_eq!(lef0(&sim).1.1, Mobility::Captured);
}

/// Eviction (a leg arriving at the *non-capture* side of a CTCF) should
/// trigger an immediate respawn (dt = 0).
#[test]
fn eviction_triggers_immediate_respawn() {
    // CTCF at site 20 with its capture side on the right. The right-facing
    // leg approaches from the left (the eviction side), so it gets evicted
    // on contact.
    let mut sites = vec![Site::plain(); 50];
    sites[20] = Site::ctcf(Direction::Right, 1.0);
    let mut sim = one_lef(
        Lattice::new(sites).unwrap(),
        10,
        Rates {
            step: 1.0,
            respawn: 1e-12,
            respawn_captured: 1e-12,
        },
        13,
    );

    for _ in 0..2000 {
        sim.step().expect("queue exhausted before reaching CTCF");

        if lef0(&sim).1.1 == Mobility::Evicted {
            // The very next event must be this LEF's respawn at dt = 0.
            let pre_clock = sim.time();
            let (next_t, next_a) = sim.step().expect("respawn should be queued immediately");
            assert_eq!(next_t, pre_clock, "respawn dt should be 0");
            assert!(matches!(next_a, Action::Respawn { lef } if sim.lef_id(lef) == Some(0)));
            let (a, b) = lef0(&sim).0;
            assert_eq!(a, b, "both legs should share the new spawn site");
            assert_ne!(a, 20, "should not respawn on the CTCF");
            return;
        }
    }
    panic!("never observed eviction");
}

#[test]
fn a_fresh_lefs_right_leg_moves_on_when_the_leg_ahead_leaves() {
    // LEF A is freshly placed at 4 (both legs on one site), its left leg
    // blocked by C's right leg at 3 and its right leg by B's right leg at 5.
    // When B's right leg steps on, A's right leg must be rescheduled too,
    // although A's left leg shares its site.
    for seed in 0..10 {
        let mut world = World::new(Lattice::plain(30).unwrap());
        world.add_lef((1, Mobility::Moving), (5, Mobility::Moving)).unwrap(); // B
        world.add_lef((2, Mobility::Moving), (3, Mobility::Moving)).unwrap(); // C
        world.add_lef((4, Mobility::Moving), (4, Mobility::Moving)).unwrap(); // A
        let rates = Rates { step: 1.0, respawn: 1e-12, respawn_captured: 1e-12 };
        let mut sim = Sim::new(Model::new(world, rates, SpawnRule::UniformNonCtcf).unwrap(), seed);
        sim.run_until(200.0);
        let (_, (a_right, _)) = sim.world().lef_state(2).unwrap();
        assert!(a_right > 5, "seed {seed}: A's right leg stuck at {a_right}");
    }
}
