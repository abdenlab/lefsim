//! Golden-vector test: a fixed seed must reproduce an exact event stream.
//!
//! Guards against accidental changes to RNG consumption or event ordering,
//! and — once run on other targets (e.g. wasm32) — against platform
//! differences. The simulation's only transcendental, `ln` in the
//! exponential sampler, goes through the pure-Rust `libm` crate precisely
//! so that this hash is the same everywhere.
//!
//! If a deliberate model change alters the stream, update `GOLDEN` and say
//! why in the commit message.

use lefsim::*;

const GOLDEN: u64 = 0x788bb5556feaed1b;

/// FNV-1a over the bytes of every event row and the final grid snapshot.
fn fingerprint() -> u64 {
    let mut sites = vec![Site::plain(); 500];
    for (i, site) in sites.iter_mut().enumerate().skip(25).step_by(50) {
        let side = if i % 100 == 25 { Direction::Right } else { Direction::Left };
        *site = Site::ctcf(side, 0.7);
    }
    let rule = SpawnRule::UniformNonCtcf;
    let world = populate(Lattice::new(sites).unwrap(), 20, &rule, 0x5EED).unwrap();
    let rates = Rates { step: 1.0, respawn: 0.01, respawn_stalled: 0.05, respawn_captured: 0.002 };
    let mut sim = Sim::new(Model::new(world, rates, rule).unwrap(), 0x5EED);
    let mut rec = sim.recorder(SampleMode::every_dt(5.0));
    let mut ev = EventRecorder::new();
    let report = sim.run_events_with(50_000, &mut (&mut rec, &mut ev));
    assert_eq!(report.n_events, 50_000);

    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    let e = ev.frames();
    for i in 0..e.n_rows() {
        eat(&e.time()[i].to_bits().to_le_bytes());
        eat(&e.leg_id()[i].to_le_bytes());
        eat(&e.pos()[i].to_le_bytes());
        eat(&[e.mob()[i].code(), e.kind()[i]]);
    }
    let f = rec.frames();
    for &t in f.time() {
        eat(&t.to_bits().to_le_bytes());
    }
    let lefs = f.lefs();
    for (&l, &r) in lefs.left_pos.iter().zip(&lefs.right_pos) {
        eat(&l.to_le_bytes());
        eat(&r.to_le_bytes());
    }
    eat(Mobility::as_codes(&lefs.left_mobility));
    eat(Mobility::as_codes(&lefs.right_mobility));
    h
}

#[test]
fn fixed_seed_reproduces_golden_stream() {
    assert_eq!(fingerprint(), GOLDEN, "event stream changed: got {:#018x}", fingerprint());
}
