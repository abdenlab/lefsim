//! Smoke example: 100-site lattice with two oppositely-oriented CTCFs and a
//! single LEF spawning at the midpoint. Runs for a small number of events
//! and prints the trajectory.

use lefs::*;
use rand::SeedableRng;

fn main() {
    // 100-site lattice with right-stopping CTCF at site 30 and left-stopping
    // CTCF at site 70 (i.e. a "TAD" between them in the convergent convention).
    let mut sites = vec![Site::plain(); 100];
    sites[30] = Site::ctcf(Dir::Right);
    sites[70] = Site::ctcf(Dir::Left);
    let lattice = Lattice::new(sites);

    let mut world = World::new(lattice);
    let (_lef, [l_left, l_right]) = world.spawn_lef(50);

    let mut rng = SimRng::seed_from_u64(42);
    let mut engine = Engine::new(Rates::default_demo());
    engine.prime(&world, &mut rng);

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
