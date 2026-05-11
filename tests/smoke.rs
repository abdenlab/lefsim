//! Integration tests against the public API.

use lefs::*;
use rand::SeedableRng;

/// On an empty lattice with a single LEF spawning at the midpoint, the legs
/// should — at some point during the run — separate outward from their
/// starting site. We track the extreme positions reached because the run
/// will eventually trigger respawns once both legs hit the lattice
/// boundary, after which positions reset.
#[test]
fn lef_extrudes_outward_on_empty_lattice() {
    let mut world = World::new(Lattice::new_plain(200));
    let (_lef, [l_left, l_right]) = world.spawn_lef(100);

    let mut rng = SimRng::seed_from_u64(7);
    let mut engine = Engine::new(Rates {
        step: 1.0,
        // Effectively never within the test horizon (~1e9 expected wait),
        // but a respawn will still fire once step actions are exhausted at
        // the boundaries.
        respawn_default: 1e-9,
        respawn_captured: 1e-9,
    });
    engine.prime(&world, &mut rng);

    let mut touched = Touched::default();
    let mut leftmost = world.legs[l_left].pos;
    let mut rightmost = world.legs[l_right].pos;
    for _ in 0..400 {
        if engine
            .step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
            .is_none()
        {
            break;
        }
        leftmost = leftmost.min(world.legs[l_left].pos);
        rightmost = rightmost.max(world.legs[l_right].pos);
        if leftmost < 100 && rightmost > 100 {
            return;
        }
    }
    panic!(
        "never observed outward extrusion: leftmost={leftmost}, rightmost={rightmost}"
    );
}

/// A right-facing leg approaching a `Ctcf { stops: Right }` site should
/// arrive captured.
#[test]
fn right_facing_leg_captured_by_right_stopping_ctcf() {
    let mut sites = vec![Site::plain(); 50];
    sites[20] = Site::ctcf(Direction::Right);
    let mut world = World::new(Lattice::new(sites));
    let (_lef, [_l_left, l_right]) = world.spawn_lef(10);

    let mut rng = SimRng::seed_from_u64(11);
    let mut engine = Engine::new(Rates {
        step: 1.0,
        respawn_default: 1e-12,
        respawn_captured: 1e-12,
    });
    engine.prime(&world, &mut rng);

    let mut touched = Touched::default();
    for _ in 0..2000 {
        engine
            .step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
            .expect("queue should not exhaust within horizon");
        if world.legs[l_right].pos == 20 {
            break;
        }
    }
    assert_eq!(world.legs[l_right].pos, 20, "right leg should reach CTCF");
    assert_eq!(world.legs[l_right].mobility, Mobility::Captured);
}

/// Eviction (a leg arriving at a CTCF whose `stops` is *opposite* to the
/// leg's facing) should trigger an immediate respawn (dt = 0).
#[test]
fn eviction_triggers_immediate_respawn() {
    // CTCF at site 20 stopping `Left`. The right-facing leg arriving from the
    // left is moving against `stops`, so it gets evicted on contact.
    let mut sites = vec![Site::plain(); 50];
    sites[20] = Site::ctcf(Direction::Left);
    let mut world = World::new(Lattice::new(sites));
    let (lef_k, [_l_left, l_right]) = world.spawn_lef(10);

    let mut rng = SimRng::seed_from_u64(13);
    let mut engine = Engine::new(Rates {
        step: 1.0,
        respawn_default: 1e-12,
        respawn_captured: 1e-12,
    });
    engine.prime(&world, &mut rng);

    let mut touched = Touched::default();
    for _ in 0..2000 {
        engine
            .step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
            .expect("queue exhausted before reaching CTCF");

        if world.legs[l_right].mobility == Mobility::Evicted {
            // The very next event must be this LEF's respawn at dt = 0.
            let pre_clock = engine.clock;
            let (next_t, next_a) = engine
                .step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
                .expect("respawn should be queued immediately");
            assert_eq!(next_t, pre_clock, "respawn dt should be 0");
            assert!(matches!(next_a, Action::Respawn { lef } if lef == lef_k));
            let [a, b] = world.lefs[lef_k].legs;
            assert_eq!(
                world.legs[a].pos, world.legs[b].pos,
                "both legs should share the new spawn site"
            );
            assert_ne!(world.legs[a].pos, 20, "should not respawn on the CTCF");
            return;
        }
    }
    panic!("never observed eviction");
}
