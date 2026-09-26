//! Keyframed event recording: every batch opens with the full state.

use lefsim::*;

const KEYFRAME: u8 = EventFrames::KEYFRAME;
const EVICTED: u8 = 2; // Mobility::Evicted.code()

fn lattice(strength: f64) -> Lattice {
    let mut sites = vec![Site::plain(); 200];
    sites[60] = Site::ctcf(Direction::Right, strength);
    sites[100] = Site::ctcf(Direction::Right, strength);
    sites[140] = Site::ctcf(Direction::Left, strength);
    Lattice::new(sites)
}

fn build(seed: u64, strength: f64) -> Sim {
    Sim::builder(lattice(strength))
        .seed(seed)
        .rates(Rates { step: 1.0, respawn_default: 0.02, respawn_captured: 0.005 })
        .lefs(8)
        .build()
        .unwrap()
}

/// `(pos, mobility code)` per leg, indexed by `leg_id`.
type LegState = Vec<(u32, u8)>;

fn leg_state(sim: &Sim) -> LegState {
    (0..sim.n_lefs() as u32)
        .flat_map(|i| {
            let (l, r) = sim.lef_positions(i);
            let (lm, rm) = sim.lef_mobilities(i);
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
    (0..f.n_rows()).map(|i| (f.time()[i], f.leg_id()[i], f.pos()[i], f.mob()[i], f.kind()[i]))
}

#[test]
fn keyframe_kinds_extend_the_action_names() {
    assert_eq!(EventFrames::KIND_NAMES[..2], Action::NAMES);
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
fn keyframes_never_show_evicted_and_batches_never_split_a_chain() {
    // Strong CTCFs evict often; a cap just above a keyframe forces a batch
    // boundary every couple of events, so boundaries land on chains.
    let mut sim = build(3, 1.0);
    let n_legs = sim.n_legs();
    let batches = keyframed_batches(&mut sim, 3000.0, n_legs + 4);

    let events = batches.iter().flat_map(rows).filter(|r| r.4 != KEYFRAME);
    let evictions = events.filter(|r| r.3 == EVICTED).count();
    assert!(evictions > 20, "want many evictions, got {evictions}");

    for (b, batch) in batches.iter().enumerate() {
        assert!(rows(batch).take(n_legs).all(|r| r.3 != EVICTED), "keyframe {b} shows Evicted");
        // An eviction is followed by its respawn at the same instant; a batch
        // ending on one would have split the chain.
        if let Some(last) = rows(batch).last() {
            assert_ne!(last.3, EVICTED, "batch {b} ends mid-chain");
        }
    }
}

#[test]
fn a_recorder_attached_mid_chain_opens_after_the_chain() {
    let mut sim = build(4, 1.0);
    // Step until a leg is evicted: its respawn is then due at this instant.
    while !leg_state(&sim).iter().any(|&(_, m)| m == EVICTED) {
        sim.step().expect("run should contain an eviction");
    }
    let t_evict = sim.time();
    assert_eq!(sim.next_event_time(), Some(t_evict));

    let mut rec = EventRecorder::new().with_keyframes(sim.n_legs());
    sim.run_until_with(t_evict + 20.0, &mut rec);
    let all: Vec<_> = rows(rec.frames()).collect();
    let n_legs = sim.n_legs();
    assert!(all[..n_legs].iter().all(|r| r.4 == KEYFRAME && r.0 == t_evict && r.3 != EVICTED));
    // The respawn fired before the keyframe and is folded into it.
    assert!(all[n_legs..].iter().all(|r| r.4 != KEYFRAME && r.0 > t_evict));
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
