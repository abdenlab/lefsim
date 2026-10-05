//! Keyframed event recording: every batch opens with the full state.

use lefsim::*;

const KEYFRAME: u8 = EventFrames::KEYFRAME;

fn lattice(strength: f64) -> Lattice {
    let mut sites = vec![Site::plain(); 200];
    sites[60] = Site::ctcf(Direction::Right, strength);
    sites[100] = Site::ctcf(Direction::Right, strength);
    sites[140] = Site::ctcf(Direction::Left, strength);
    Lattice::new(sites).unwrap()
}

fn build(seed: u64, strength: f64) -> Sim {
    let rule = SpawnRule::UniformNonCtcf;
    let world = populate(lattice(strength), 8, &rule, seed).unwrap();
    let rates = Rates { step: 1.0, respawn: 0.02, respawn_blocked: 0.02, respawn_captured: 0.005 };
    Sim::new(Model::new(world, rates, rule).unwrap(), seed)
}

/// `(pos, mobility code)` per leg, indexed by `leg_id`.
type LegState = Vec<(u32, u8)>;

fn leg_state(sim: &Sim) -> LegState {
    let lefs = sim.world().lef_states();
    (0..lefs.len())
        .flat_map(|i| {
            let ((l, lm), (r, rm)) = lefs.get(i).unwrap();
            [(l, lm.code()), (r, rm.code())]
        })
        .collect()
}

/// State after every event up to `t_end`, plus the initial state.
fn trajectory(mut sim: Sim, t_end: Time) -> Vec<(Time, LegState)> {
    let mut traj = vec![(sim.time(), leg_state(&sim))];
    while sim.next_event_time().is_some_and(|t| t <= t_end) {
        sim.step();
        traj.push((sim.time(), leg_state(&sim)));
    }
    traj
}

/// The state after every event at or before `t`.
fn state_at(traj: &[(Time, LegState)], t: Time) -> &LegState {
    &traj.iter().rev().find(|(te, _)| *te <= t).unwrap().1
}

/// Run to `t_end` with a bounded keyframed recorder, draining a batch
/// each time it fills.
fn keyframed_batches(sim: &mut Sim, t_end: Time, max_rows: usize) -> Vec<EventFrames> {
    let mut rec = EventRecorder::new().with_max_rows(max_rows).with_keyframes(sim.n_legs());
    let mut batches = Vec::new();
    loop {
        let report = sim.run_until_with(t_end, &mut rec);
        batches.push(rec.take_frames());
        if report.stopped != StopReason::RecorderFull {
            return batches;
        }
    }
}

/// Rows `lo..hi` of a batch as `(time, leg_id, pos, mob, kind)`.
fn rows(f: &EventFrames) -> impl Iterator<Item = (Time, u32, u32, u8, u8)> + '_ {
    (0..f.n_rows()).map(|i| (f.time()[i], f.leg_id()[i], f.pos()[i], f.mob()[i].code(), f.kind()[i]))
}

#[test]
fn row_kinds_name_the_actions_and_keyframes() {
    // Codes 0, 1 and 3 are the actions (`Action::code`); 2 is the keyframe.
    assert_eq!(EventFrames::KIND_NAMES, ["Step", "Respawn", "Keyframe", "Evict"]);
    assert_eq!(EventFrames::KIND_NAMES[KEYFRAME as usize], "Keyframe");
}

#[test]
fn every_batch_opens_with_a_keyframe() {
    let mut sim = build(1, 0.8);
    let n_legs = sim.n_legs();
    let max_rows = 200;
    let batches = keyframed_batches(&mut sim, 400.0, max_rows);
    assert!(batches.len() > 10, "want many batches, got {}", batches.len());

    for (b, batch) in batches.iter().enumerate() {
        let head: Vec<_> = rows(batch).take(n_legs).collect();
        assert_eq!(head.len(), n_legs, "batch {b} is shorter than a keyframe");
        let t0 = head[0].0;
        for (id, &(t, leg, _, _, kind)) in head.iter().enumerate() {
            assert_eq!((kind, leg, t), (KEYFRAME, id as u32, t0), "batch {b} row {id}");
        }
        assert!(rows(batch).skip(n_legs).all(|r| r.4 != KEYFRAME), "keyframe mid-batch {b}");
        // The cap is soft only by a zero-delay chain in progress.
        assert!(batch.n_rows() <= max_rows + 2, "batch {b} has {} rows", batch.n_rows());
    }
}

#[test]
fn replaying_from_any_batch_reproduces_the_run() {
    let t_end = 400.0;
    let traj = trajectory(build(2, 0.8), t_end);
    let mut sim = build(2, 0.8);
    let n_legs = sim.n_legs();
    let batches = keyframed_batches(&mut sim, t_end, 150);
    let all: Vec<_> = batches.iter().flat_map(rows).collect();

    // Row index where each batch starts.
    let starts: Vec<usize> = batches
        .iter()
        .scan(0, |at, b| {
            let s = *at;
            *at += b.n_rows();
            Some(s)
        })
        .collect();

    for (b, &start) in starts.iter().enumerate() {
        // Decode the keyframe alone: it must equal the state at its time.
        let mut state: LegState = vec![(0, 0); n_legs];
        for &(_, leg, pos, mob, _) in &all[start..start + n_legs] {
            state[leg as usize] = (pos, mob);
        }
        let t_key = all[start].0;
        assert_eq!(&state, state_at(&traj, t_key), "keyframe of batch {b} at t={t_key}");

        // Replay forward a few batches, checking after each group of rows
        // sharing a time. Later keyframes must agree with the replay.
        let end = starts.get(b + 3).copied().unwrap_or(all.len());
        let mut i = start + n_legs;
        while i < end {
            let t = all[i].0;
            while i < end && all[i].0 == t {
                let (_, leg, pos, mob, kind) = all[i];
                if kind == KEYFRAME {
                    assert_eq!(state[leg as usize], (pos, mob), "later keyframe disagrees at t={t}");
                }
                state[leg as usize] = (pos, mob);
                i += 1;
            }
            assert_eq!(&state, state_at(&traj, t), "replay from batch {b}, at t={t}");
        }
    }
}

#[test]
fn dropping_keyframes_leaves_the_plain_event_log() {
    let t_end = 500.0;
    let mut plain_sim = build(5, 0.8);
    let mut plain = EventRecorder::new();
    plain_sim.run_until_with(t_end, &mut plain);

    let mut sim = build(5, 0.8);
    let batches = keyframed_batches(&mut sim, t_end, 120);
    let events: Vec<_> = batches.iter().flat_map(rows).filter(|r| r.4 != KEYFRAME).collect();
    assert_eq!(events, rows(plain.frames()).collect::<Vec<_>>());
    assert_eq!(leg_state(&sim), leg_state(&plain_sim));
}

#[test]
fn an_unbounded_keyframed_recorder_writes_one_keyframe() {
    let mut sim = build(6, 0.8);
    let mut rec = EventRecorder::new().with_keyframes(sim.n_legs());
    sim.run_until_with(100.0, &mut rec);
    sim.run_until_with(200.0, &mut rec);
    let keyframe_rows = rows(rec.frames()).filter(|r| r.4 == KEYFRAME).count();
    assert_eq!(keyframe_rows, sim.n_legs());
}

#[test]
#[should_panic(expected = "n_legs + 2")]
fn keyframes_need_room_for_an_event_after_max_rows() {
    let _ = EventRecorder::new().with_max_rows(10).with_keyframes(16);
}

#[test]
#[should_panic(expected = "n_legs + 2")]
fn keyframes_need_room_for_an_event_before_max_rows() {
    let _ = EventRecorder::new().with_keyframes(16).with_max_rows(10);
}
