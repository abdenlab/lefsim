//! Integration tests against the public API.

use lefsim::*;

/// On an empty lattice with a single LEF spawning at the midpoint, the legs
/// should — at some point during the run — separate outward from their
/// starting site. We track the extreme positions reached because the run
/// will eventually trigger respawns once both legs hit the lattice
/// boundary, after which positions reset.
#[test]
fn lef_extrudes_outward_on_empty_lattice() {
    let mut sim = Sim::builder(Lattice::new_plain(200))
        .seed(7)
        .rates(Rates {
            step: 1.0,
            // Effectively never within the test horizon (~1e9 expected
            // wait), but a respawn will still fire once step actions are
            // exhausted at the boundaries.
            respawn_default: 1e-9,
            respawn_captured: 1e-9,
        })
        .lef_at(100)
        .build()
        .unwrap();

    let (mut leftmost, mut rightmost) = sim.lef_positions(0);
    for _ in 0..400 {
        if sim.step().is_none() {
            break;
        }
        let (l, r) = sim.lef_positions(0);
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
    let mut sim = Sim::builder(Lattice::new(sites))
        .seed(11)
        .rates(Rates {
            step: 1.0,
            respawn_default: 1e-12,
            respawn_captured: 1e-12,
        })
        .lef_at(10)
        .build()
        .unwrap();

    for _ in 0..2000 {
        sim.step().expect("queue should not exhaust within horizon");
        if sim.lef_positions(0).1 == 20 {
            break;
        }
    }
    assert_eq!(sim.lef_positions(0).1, 20, "right leg should reach CTCF");
    assert_eq!(sim.lef_mobilities(0).1, Mobility::Captured);
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
    let mut sim = Sim::builder(Lattice::new(sites))
        .seed(13)
        .rates(Rates {
            step: 1.0,
            respawn_default: 1e-12,
            respawn_captured: 1e-12,
        })
        .lef_at(10)
        .build()
        .unwrap();

    for _ in 0..2000 {
        sim.step().expect("queue exhausted before reaching CTCF");

        if sim.lef_mobilities(0).1 == Mobility::Evicted {
            // The very next event must be this LEF's respawn at dt = 0.
            let pre_clock = sim.time();
            let (next_t, next_a) = sim.step().expect("respawn should be queued immediately");
            assert_eq!(next_t, pre_clock, "respawn dt should be 0");
            assert!(matches!(next_a, Action::Respawn { lef } if sim.lef_id(lef) == Some(0)));
            let (a, b) = sim.lef_positions(0);
            assert_eq!(a, b, "both legs should share the new spawn site");
            assert_ne!(a, 20, "should not respawn on the CTCF");
            return;
        }
    }
    panic!("never observed eviction");
}
