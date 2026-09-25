//! Smoke example: 100-site lattice with two oppositely-oriented CTCFs and a
//! single LEF spawning at the midpoint. Runs for a small number of events
//! and prints the trajectory.

use lefsim::*;
use rand::SeedableRng;

fn main() {
    // 100-site lattice with a convergent CTCF pair bounding a "TAD"
    // between sites 30 and 70:
    //  - site 30's capture side is its right side (captures left-walking
    //    legs arriving from inside the TAD);
    //  - site 70's capture side is its left side (captures right-walking
    //    legs arriving from inside the TAD).
    let mut sites = vec![Site::plain(); 100];
    sites[30] = Site::ctcf(Direction::Right, 1.0);
    sites[70] = Site::ctcf(Direction::Left, 1.0);
    let lattice = Lattice::new(sites);

    let mut rng = SimRng::seed_from_u64(42);
    let mut world = World::new(lattice);
    let (_lef, [l_left, l_right]) = world.spawn_lef(50, &mut rng);

    let mut engine = Engine::new(Rates::default());
    engine.initialize(&world, &mut rng);

    let mut touched = Touched::default();
    println!(
        "t=0.0000  init                   pos=({}, {})",
        world.legs[l_left].pos, world.legs[l_right].pos
    );
    for _ in 0..200 {
        let Some((t, action)) =
            engine.step(&mut world, &UniformNonCtcfSpawn, &mut rng, &mut touched)
        else {
            break;
        };
        println!(
            "t={:>8.4}  {:24}  pos=({}, {})  mob=({:?}, {:?})",
            t,
            format!("{:?}", action),
            world.legs[l_left].pos,
            world.legs[l_right].pos,
            world.legs[l_left].mobility,
            world.legs[l_right].mobility,
        );
    }
}
