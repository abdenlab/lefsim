//! Smoke example: 100-site lattice with two oppositely-oriented CTCFs and a
//! single LEF spawning at the midpoint. Prints the first events one by one,
//! then the same run sampled on a regular time grid.

use lefsim::*;

fn build() -> Sim {
    // 100-site lattice with a convergent CTCF pair bounding a "TAD"
    // between sites 30 and 70:
    //  - site 30's capture side is its right side (captures left-walking
    //    legs arriving from inside the TAD);
    //  - site 70's capture side is its left side (captures right-walking
    //    legs arriving from inside the TAD).
    let mut sites = vec![Site::plain(); 100];
    sites[30] = Site::ctcf(Direction::Right, 1.0);
    sites[70] = Site::ctcf(Direction::Left, 1.0);

    let mut world = World::new(Lattice::new(sites).expect("valid lattice"));
    world
        .add_lef((50, Mobility::Moving), (50, Mobility::Moving))
        .expect("valid LEF");
    let model = Model::new(world, Rates::default(), SpawnRule::UniformNonCtcf).expect("valid model");
    Sim::new(model, 42)
}

fn describe(sim: &Sim, action: Action) -> String {
    match action {
        Action::Step { leg } => format!("Step leg {}", sim.leg_id(leg).unwrap()),
        Action::Respawn { lef } => format!("Respawn lef {}", sim.lef_id(lef).unwrap()),
    }
}

fn main() {
    // Event by event.
    let mut sim = build();
    let ((l, _), (r, _)) = sim.world().lef_state(0).unwrap();
    println!("t=0.0000  init             pos=({l}, {r})");
    for _ in 0..200 {
        let Some((t, action)) = sim.step() else { break };
        let ((l, lm), (r, rm)) = sim.world().lef_state(0).unwrap();
        println!(
            "t={t:>8.4}  {:15}  pos=({l}, {r})  mob=({lm:?}, {rm:?})",
            describe(&sim, action)
        );
    }

    // The same run, sampled every 10 time units.
    let mut sim = build();
    let (frames, report) = sim.collect_until(200.0, SampleMode::every_dt(10.0));
    println!(
        "\n{} events, {} snapshots, stopped: {:?}",
        report.n_events,
        frames.n_snapshots(),
        report.stopped
    );
    for s in 0..frames.n_snapshots() {
        let snap = frames.snapshot(s);
        println!(
            "t={:>6.1}  pos=({}, {})  mob=({:?}, {:?})",
            snap.time,
            snap.left_pos[0],
            snap.right_pos[0],
            snap.left_mobility[0],
            snap.right_mobility[0],
        );
    }
}
