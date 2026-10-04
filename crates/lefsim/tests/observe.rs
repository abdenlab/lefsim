//! Observation semantics: grid snapshots, event deltas, streaming.

use lefsim::*;

/// 200 sites with a convergent CTCF pair and a third CTCF facing the other
/// way, so runs see captures, evictions, and respawns.
fn lattice(strength: f64) -> Lattice {
    let mut sites = vec![Site::plain(); 200];
    sites[60] = Site::ctcf(Direction::Right, strength);
    sites[100] = Site::ctcf(Direction::Right, strength);
    sites[140] = Site::ctcf(Direction::Left, strength);
    Lattice::new(sites).unwrap()
}

/// Respawns fast enough that a few hundred time units cover many LEF
/// lifetimes.
fn rates() -> Rates {
    Rates {
        step: 1.0,
        respawn: 0.02,
        respawn_stalled: 0.02,
        respawn_evicted: 0.5,
        respawn_captured: 0.005,
    }
}

fn build(seed: u64) -> Sim {
    build_with_strength(seed, 0.8)
}

fn build_with_strength(seed: u64, strength: f64) -> Sim {
    let rule = SpawnRule::UniformNonCtcf;
    let world = populate(lattice(strength), 8, &rule, seed).unwrap();
    Sim::new(Model::new(world, rates(), rule).unwrap(), seed)
}

type LefState = (Position, Position, u8, u8);

fn state(sim: &Sim) -> Vec<LefState> {
    (0..sim.n_lefs())
        .map(|i| {
            let ((l, lm), (r, rm)) = sim.world().lef_state(i).unwrap();
            (l, r, lm.code(), rm.code())
        })
        .collect()
}

fn snapshot_state(frames: &LefFrames, s: usize) -> Vec<LefState> {
    let snap = frames.snapshot(s);
    (0..frames.n_lefs())
        .map(|i| {
            (
                snap.left_pos[i],
                snap.right_pos[i],
                snap.left_mobility[i].code(),
                snap.right_mobility[i].code(),
            )
        })
        .collect()
}

/// Step one event at a time up to `t_end`, returning the state after every
/// event (plus the initial state at t = 0).
fn event_trajectory(mut sim: Sim, t_end: Time) -> Vec<(Time, Vec<LefState>)> {
    let mut traj = vec![(sim.time(), state(&sim))];
    while sim.next_event_time().is_some_and(|t| t <= t_end) {
        sim.step();
        traj.push((sim.time(), state(&sim)));
    }
    traj
}

/// The state after every event with time <= t.
fn state_at(traj: &[(Time, Vec<LefState>)], t: Time) -> &[LefState] {
    &traj.iter().rev().find(|(te, _)| *te <= t).unwrap().1
}

#[test]
fn grid_snapshots_match_event_level_trajectory() {
    let (t_end, dt) = (300.0, 0.7);
    let (frames, report) = build(1).collect_until(t_end, SampleMode::every_dt(dt));
    assert_eq!(report.stopped, StopReason::ReachedTime);

    let expected_count = (0..).take_while(|&k| k as f64 * dt <= t_end).count();
    assert_eq!(frames.n_snapshots(), expected_count);

    let traj = event_trajectory(build(1), t_end);
    assert!(traj.len() > 200, "want a busy run, got {} events", traj.len());
    for s in 0..frames.n_snapshots() {
        let g = frames.time()[s];
        assert_eq!(g, s as f64 * dt);
        assert_eq!(snapshot_state(&frames, s), state_at(&traj, g), "at t={g}");
    }
}

#[test]
fn recording_does_not_perturb_the_run() {
    let t_end = 250.0;
    let mut plain = build(2);
    plain.run_until(t_end);

    let mut observed = build(2);
    let mut rec = observed.recorder(SampleMode::every_dt(0.3));
    let mut events = EventRecorder::new();
    observed.run_until_with(t_end, &mut (&mut rec, &mut events));

    assert_eq!(observed.n_events(), plain.n_events());
    assert_eq!(observed.time(), plain.time());
    assert_eq!(state(&observed), state(&plain));
    assert!(events.frames().n_rows() as u64 >= plain.n_events());
}

#[test]
fn split_runs_record_the_same_as_one_run() {
    let dt = 0.5;
    let mut whole = build(3);
    let mut whole_rec = whole.recorder(SampleMode::every_dt(dt));
    let mut whole_ev = EventRecorder::new();
    whole.run_until_with(120.0, &mut (&mut whole_rec, &mut whole_ev));

    let mut split = build(3);
    let mut split_rec = split.recorder(SampleMode::every_dt(dt));
    let mut split_ev = EventRecorder::new();
    for t in [10.37, 10.37, 55.0, 55.25, 120.0] {
        split.run_until_with(t, &mut (&mut split_rec, &mut split_ev));
    }

    assert_eq!(split_rec.frames(), whole_rec.frames());
    assert_eq!(split_ev.frames(), whole_ev.frames());
    assert_eq!(state(&split), state(&whole));
}

#[test]
fn bounded_recorders_stream_without_loss() {
    let t_end = 200.0;
    let mut whole = build(4);
    let mut whole_rec = whole.recorder(SampleMode::every_dt(0.4));
    let mut whole_ev = EventRecorder::new();
    whole.run_until_with(t_end, &mut (&mut whole_rec, &mut whole_ev));

    let mut sim = build(4);
    let mut rec = sim.recorder(SampleMode::every_dt(0.4)).with_max_snapshots(7);
    let mut ev = EventRecorder::new().with_max_rows(11);
    let mut frames = LefFrames::new(sim.n_lefs());
    let mut events = EventFrames::new();
    let (mut chunk, mut ev_chunk) = (LefFrames::default(), EventFrames::default());
    let mut calls = 0;
    loop {
        let report = sim.run_until_with(t_end, &mut (&mut rec, &mut ev));
        calls += 1;
        rec.drain_into(&mut chunk);
        ev.drain_into(&mut ev_chunk);
        assert!(chunk.n_snapshots() <= 7);
        assert!(ev_chunk.n_rows() <= 11);
        frames.extend_from(&chunk);
        events.extend_from(&ev_chunk);
        if report.stopped != StopReason::RecorderFull {
            assert_eq!(report.stopped, StopReason::ReachedTime);
            break;
        }
    }

    assert!(calls > 10, "expected many resumes, got {calls}");
    assert_eq!(&frames, whole_rec.frames());
    assert_eq!(&events, whole_ev.frames());
    assert_eq!(state(&sim), state(&whole));
}

#[test]
fn first_snapshot_is_the_initial_state() {
    let fresh = build(5);
    let (frames, _) = build(5).collect_until(10.0, SampleMode::every_dt(1.0));
    assert_eq!(frames.time()[0], 0.0);
    assert_eq!(snapshot_state(&frames, 0), state(&fresh));
}

#[test]
fn an_evicted_leg_waits_just_before_a_ctcf_that_faces_away() {
    // Strength 1: every leg meeting a CTCF from behind is evicted. The
    // CTCFs at 60 and 100 capture from the right, so right-facing legs are
    // evicted at 59 and 99; the one at 140 captures from the left, so
    // left-facing legs are evicted at 141.
    let (frames, _) = build_with_strength(6, 1.0).collect_until(500.0, SampleMode::every_dt(0.25));
    let mut evicted = 0;
    for s in 0..frames.n_snapshots() {
        let snap = frames.snapshot(s);
        for lef in 0..snap.left_pos.len() {
            if snap.left_mobility[lef] == Mobility::Evicted {
                evicted += 1;
                assert_eq!(snap.left_pos[lef], 141, "an evicted left leg off its place");
            }
            if snap.right_mobility[lef] == Mobility::Evicted {
                evicted += 1;
                assert!([59, 99].contains(&snap.right_pos[lef]), "an evicted right leg at {}", snap.right_pos[lef]);
            }
        }
    }
    assert!(evicted > 20, "want evicted legs to exercise this, got {evicted}");
}

#[test]
fn replaying_deltas_reconstructs_snapshots() {
    let mut sim = build(8);
    let mut rec = sim.recorder(SampleMode::every_dt(0.9));
    let mut ev = EventRecorder::new();
    sim.run_until_with(300.0, &mut (&mut rec, &mut ev));
    let (frames, deltas) = (rec.frames(), ev.frames());

    // Deltas carry absolute values, so re-applying one already reflected in
    // the base snapshot is harmless.
    let mut replay = snapshot_state(frames, 0);
    let mut row = 0;
    for s in 0..frames.n_snapshots() {
        let g = frames.time()[s];
        while row < deltas.n_rows() && deltas.time()[row] <= g {
            let leg = deltas.leg_id()[row] as usize;
            let lef = &mut replay[leg / 2];
            if leg.is_multiple_of(2) {
                (lef.0, lef.2) = (deltas.pos()[row], deltas.mob()[row].code());
            } else {
                (lef.1, lef.3) = (deltas.pos()[row], deltas.mob()[row].code());
            }
            row += 1;
        }
        assert_eq!(replay, snapshot_state(frames, s), "at t={g}");
    }
}

#[test]
fn event_rows_match_actions() {
    let mut sim = build(9);
    let mut ev = EventRecorder::new();
    let report = sim.run_events_with(2000, &mut ev);
    assert_eq!(report.n_events, 2000);

    let f = ev.frames();
    let step = Action::Step { leg: LegKey::default() }.code();
    let respawn = Action::Respawn { lef: LefKey::default() }.code();
    let n_steps = f.kind().iter().filter(|&&k| k == step).count();
    let n_respawn_rows = f.kind().iter().filter(|&&k| k == respawn).count();
    // One row per step, two per respawn.
    assert_eq!(n_respawn_rows % 2, 0);
    assert_eq!(n_steps + n_respawn_rows / 2, 2000);
    assert!(f.time().windows(2).all(|w| w[0] <= w[1]));
    assert!(f.leg_id().iter().all(|&id| (id as usize) < sim.n_legs()));
}

#[test]
fn same_seed_same_output() {
    let run = |seed| {
        let mut sim = build(seed);
        let mut rec = sim.recorder(SampleMode::every_dt(0.5));
        let mut ev = EventRecorder::new();
        sim.run_until_with(150.0, &mut (&mut rec, &mut ev));
        (rec.take_frames(), ev.take_frames())
    };
    assert_eq!(run(10), run(10));
    assert_ne!(run(10), run(11));
}

#[test]
fn late_recorder_skips_unobserved_grid_points() {
    let mut sim = build(12);
    sim.run_until(10.0);
    let mut rec = sim.recorder(SampleMode::every_dt(1.0));
    sim.run_until_with(20.0, &mut rec);
    assert_eq!(rec.frames().time(), (10..=20).map(f64::from).collect::<Vec<_>>());

    // And those snapshots agree with a recorder attached from the start.
    let (full, _) = build(12).collect_until(20.0, SampleMode::every_dt(1.0));
    for s in 0..rec.frames().n_snapshots() {
        assert_eq!(snapshot_state(rec.frames(), s), snapshot_state(&full, s + 10));
    }
}

#[test]
fn run_events_only_records_settled_grid_points() {
    let mut sim = build(13);
    let mut rec = sim.recorder(SampleMode::every_dt(0.5));
    sim.run_events_with(500, &mut rec);
    let now = sim.time();
    assert!(rec.frames().time().iter().all(|&g| g <= now));
    // Resuming by time fills in the rest of the grid seamlessly.
    sim.run_until_with(100.0, &mut rec);
    let (full, _) = build(13).collect_until(100.0, SampleMode::every_dt(0.5));
    assert_eq!(rec.frames(), &full);
}

#[test]
fn manual_mode_records_only_on_capture() {
    let mut sim = build(14);
    let mut rec = sim.recorder(SampleMode::Manual);
    sim.run_until_with(5.0, &mut rec);
    assert!(rec.frames().is_empty());
    assert!(rec.capture(&sim));
    sim.run_until_with(9.0, &mut rec);
    assert!(rec.capture(&sim));
    assert_eq!(rec.frames().time(), &[5.0, 9.0]);
    assert_eq!(snapshot_state(rec.frames(), 1), state(&sim));
}

#[test]
fn model_describes_the_run() {
    let mut sim = build(15);
    sim.run_until(50.0);
    let model = sim.model().unwrap();
    assert_eq!(model.rates(), rates());
    assert_eq!(model.spawn(), SpawnRule::UniformNonCtcf);
    let lattice = model.world().lattice();
    assert_eq!(lattice.len(), 200);
    let sites: Vec<_> = lattice.ctcfs().iter().map(|c| c.site).collect();
    assert_eq!(sites, vec![60, 100, 140]);
    assert_eq!(model.world().lef_states().len(), sim.n_lefs());
    for (i, row) in state(&sim).into_iter().enumerate() {
        let ((l, lm), (r, rm)) = model.world().lef_state(i).unwrap();
        assert_eq!(row, (l, r, lm.code(), rm.code()));
    }
}

#[test]
fn event_capped_chunks_record_the_same_as_one_run() {
    let (t_end, dt) = (200.0, 0.3);
    let mut whole = build(16);
    let mut whole_rec = whole.recorder(SampleMode::every_dt(dt));
    let mut whole_ev = EventRecorder::new();
    whole.run_until_with(t_end, &mut (&mut whole_rec, &mut whole_ev));

    let mut sim = build(16);
    let mut rec = sim.recorder(SampleMode::every_dt(dt));
    let mut ev = EventRecorder::new();
    let mut chunks = 0;
    loop {
        let report = sim.run_until_with_limit(t_end, 37, &mut (&mut rec, &mut ev));
        chunks += 1;
        assert!(report.n_events <= 37);
        match report.stopped {
            StopReason::ReachedEventCount => continue,
            StopReason::ReachedTime => break,
            other => panic!("unexpected stop: {other:?}"),
        }
    }
    assert!(chunks > 10, "expected many chunks, got {chunks}");
    assert_eq!(sim.time(), t_end);
    assert_eq!(rec.frames(), whole_rec.frames());
    assert_eq!(ev.frames(), whole_ev.frames());
}

#[test]
fn optional_observers_behave_like_present_or_absent_ones() {
    let t_end = 100.0;
    let mut direct = build(17);
    let mut rec = direct.recorder(SampleMode::every_dt(0.5));
    direct.run_until_with(t_end, &mut rec);

    let mut via_option = build(17);
    let mut grid = Some(via_option.recorder(SampleMode::every_dt(0.5)));
    let mut events: Option<EventRecorder> = None;
    via_option.run_until_with(t_end, &mut (&mut grid, &mut events));

    assert_eq!(grid.unwrap().frames(), rec.frames());
    assert_eq!(state(&via_option), state(&direct));
}

#[test]
fn one_recorder_can_switch_between_bounded_and_unbounded() {
    let dt = 0.5;
    let (whole, _) = build(18).collect_until(200.0, SampleMode::every_dt(dt));

    let mut sim = build(18);
    let mut rec = sim.recorder(SampleMode::every_dt(dt));
    let mut frames = LefFrames::new(sim.n_lefs());
    sim.run_until_with(50.0, &mut rec); // unbounded
    frames.extend_from(&rec.take_frames());
    rec.set_max_snapshots(Some(9)); // bounded chunks
    while sim.run_until_with(150.0, &mut rec).stopped == StopReason::RecorderFull {
        frames.extend_from(&rec.take_frames());
    }
    frames.extend_from(&rec.take_frames());
    rec.set_max_snapshots(None); // unbounded again
    sim.run_until_with(200.0, &mut rec);
    frames.extend_from(&rec.take_frames());
    assert_eq!(frames, whole);
}
