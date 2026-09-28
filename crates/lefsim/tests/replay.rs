//! Replaying keyframed event logs, checked against a live `Sim`.

use lefsim::*;

fn lattice() -> Lattice {
    let ctcf = |site, captures_from| Ctcf { site, captures_from, strength: 0.9 };
    Lattice::with_ctcfs(
        200,
        &[ctcf(60, Direction::Right), ctcf(100, Direction::Right), ctcf(140, Direction::Left)],
    )
    .unwrap()
}

const RATES: Rates = Rates { step: 1.0, respawn: 0.02, respawn_captured: 0.005 };
const N_LEFS: usize = 8;

fn model(seed: u64) -> Model {
    let rule = SpawnRule::UniformNonCtcf;
    let world = populate(lattice(), N_LEFS, &rule, seed).unwrap();
    Model::new(world, RATES, rule).unwrap()
}

fn sim(seed: u64) -> Sim {
    Sim::new(model(seed), seed)
}

/// A run to `t_end` as keyframed chunks of about `max_rows` rows, each with
/// the time it describes the run through.
fn record(seed: u64, t_end: Time, max_rows: usize) -> Vec<(EventFrames, Time)> {
    let mut sim = sim(seed);
    let mut rec = EventRecorder::new().with_max_rows(max_rows).with_keyframes(sim.n_legs());
    let mut chunks = Vec::new();
    loop {
        let report = sim.run_until_with(t_end, &mut rec);
        chunks.push((rec.take_frames(), report.time));
        if report.stopped != StopReason::RecorderFull {
            return chunks;
        }
    }
}

fn replayer(chunks: &[(EventFrames, Time)]) -> Replayer {
    let mut r = Replayer::new(lattice(), N_LEFS);
    for (chunk, end) in chunks {
        r.push(chunk.clone(), *end).unwrap();
    }
    r
}

/// The live state at each of `times` (increasing).
fn reference(seed: u64, times: &[Time]) -> Vec<LefStates> {
    let mut sim = sim(seed);
    times
        .iter()
        .map(|&t| {
            sim.run_until(t);
            sim.world().lef_states()
        })
        .collect()
}

fn grid(from: Time, to: Time, step: Time) -> Vec<Time> {
    (0..).map(|i| from + i as f64 * step).take_while(|&t| t <= to).collect()
}

#[test]
fn forward_replay_matches_the_live_run() {
    let times = grid(0.0, 400.0, 0.7);
    let mut r = replayer(&record(1, 400.0, 150));
    assert_eq!((r.start(), r.end()), (Some(0.0), 400.0));
    for (t, expected) in times.iter().zip(reference(1, &times)) {
        assert_eq!(r.advance_to(*t), *t);
        assert_eq!(r.lef_states().unwrap(), &expected, "t={t}");
    }
}

#[test]
fn seeking_in_any_order_matches_the_live_run() {
    let times = grid(0.0, 400.0, 3.1);
    let expected = reference(2, &times);
    let mut r = replayer(&record(2, 400.0, 100));
    // A fixed shuffle: stride through the times by a step coprime to their count.
    let n = times.len();
    for i in (0..n).map(|i| (i * 37) % n) {
        assert_eq!(r.seek(times[i]).unwrap(), times[i]);
        assert_eq!(r.lef_states().unwrap(), &expected[i], "t={}", times[i]);
    }
}

#[test]
fn a_late_joiner_starts_at_its_first_keyframe() {
    let chunks = record(3, 400.0, 100);
    let mut r = replayer(&chunks[3..]);
    let first = chunks[3].0.time()[0];
    assert_eq!(r.start(), Some(first));

    // Before the first keyframe: not ready, and nothing to seek to.
    assert!(!r.ready());
    assert_eq!(r.advance_to(first - 1.0), Time::NEG_INFINITY);
    assert_eq!(r.lef_states(), Err(ReplayError::NotReady));
    assert!(matches!(r.seek(first - 1.0), Err(ReplayError::NoKeyframe { .. })));

    let times = grid(first, 400.0, 1.3);
    for (t, expected) in times.iter().zip(reference(3, &times)) {
        r.advance_to(*t);
        assert_eq!(r.lef_states().unwrap(), &expected, "t={t}");
    }
}

#[test]
fn replay_waits_at_the_end_of_the_data_while_buffering() {
    let chunks = record(4, 400.0, 100);
    let mut r = replayer(&chunks[..2]);
    let end = chunks[1].1;
    assert_eq!(r.advance_to(end + 50.0), end, "clamped to the data");
    for (chunk, end) in &chunks[2..] {
        r.push(chunk.clone(), *end).unwrap();
    }
    assert_eq!(r.advance_to(end + 50.0), end + 50.0);
    assert_eq!(r.lef_states().unwrap(), &reference(4, &[end + 50.0])[0]);
}

#[test]
fn trimming_drops_old_chunks_but_keeps_the_future_reachable() {
    let chunks = record(5, 400.0, 100);
    let mut r = replayer(&chunks);
    r.advance_to(250.0);
    r.trim_before(200.0);
    let start = r.start().unwrap();
    assert!(start > 0.0 && start <= 200.0, "start={start}");
    assert!(r.seek(start - 1e-9).is_err());

    let times = grid(200.0, 400.0, 2.9);
    let expected = reference(5, &times);
    for (t, e) in times.iter().zip(&expected).rev() {
        r.seek(*t).unwrap();
        assert_eq!(r.lef_states().unwrap(), e, "t={t}");
    }
}

#[test]
fn snapshots_equal_a_recorder_on_the_same_grid() {
    let mode = SampleMode::EveryDt { dt: 0.37, t0: 0.0 };
    let mut live = sim(6);
    let (expected, _) = live.collect_until(300.0, mode);

    let mut r = replayer(&record(6, 300.0, 120));
    assert_eq!(r.snapshots(mode, 300.0), expected);

    // In pieces: each call continues the grid where the last one stopped,
    // whether or not a piece ends on a grid point.
    let mut r = replayer(&record(6, 300.0, 120));
    let mut pieces = LefFrames::new(N_LEFS);
    for t in [0.0, 0.37 * 40.0, 100.0, 100.0, 222.2, 300.0] {
        pieces.extend_from(&r.snapshots(mode, t));
    }
    assert_eq!(pieces, expected);

    // A seek starts afresh from the new time.
    r.seek(0.5).unwrap();
    let again = r.snapshots(mode, 1.0);
    assert_eq!(again.time(), &[0.74]);
}

#[test]
fn a_replayed_world_starts_a_new_run() {
    let mut r = replayer(&record(7, 400.0, 100));
    r.seek(250.0).unwrap();
    let world = r.world().unwrap();
    assert_eq!(world.lef_states(), reference(7, &[250.0])[0]);

    let model = Model::new(world, RATES, SpawnRule::UniformNonCtcf).unwrap();
    let mut branch = Sim::with_start_time(model, 99, r.time());
    assert_eq!(branch.time(), 250.0);
    assert!(branch.run_until(300.0).n_events > 0);
}

#[test]
fn bad_chunks_are_rejected_whole() {
    let chunks = record(8, 100.0, 60);
    let cols = |c: &EventFrames| c.clone().into_columns();
    let rebuild = |c: EventColumns| EventFrames::try_from(c).unwrap();
    let push = |r: &mut Replayer, c: EventColumns, end| r.push(rebuild(c), end);

    let mut r = Replayer::new(lattice(), N_LEFS);
    let (first, end) = (&chunks[0].0, chunks[0].1);

    let mut c = cols(first);
    c.leg_id[20] = 16;
    assert_eq!(push(&mut r, c, end), Err(ReplayError::LegOutOfRange { leg_id: 16, n_legs: 16 }));
    let mut c = cols(first);
    c.kind[20] = 9;
    assert_eq!(push(&mut r, c, end), Err(ReplayError::BadKind { code: 9 }));
    let mut c = cols(first);
    c.leg_id.swap(0, 1); // keyframe out of leg order
    assert!(matches!(push(&mut r, c, end), Err(ReplayError::BadKeyframe { rows: 0, .. })));
    let mut c = cols(first);
    let last = *c.time.last().unwrap();
    assert!(matches!(push(&mut r, c.clone(), last - 1.0), Err(ReplayError::BadEnd { .. })));
    c.time[17] = c.time[16] - 1.0;
    assert!(matches!(push(&mut r, c, end), Err(ReplayError::OutOfOrder { .. })));
    assert_eq!(r.start(), None, "nothing was pushed");

    // Chunks must come in order.
    r.push(chunks[1].0.clone(), chunks[1].1).unwrap();
    assert!(matches!(r.push(first.clone(), end), Err(ReplayError::OutOfOrder { .. })));

    let mut c = cols(first);
    c.pos.pop();
    assert_eq!(EventFrames::try_from(c), Err(ColumnLengthError));
}

#[test]
fn a_log_from_another_lattice_gives_no_world() {
    let chunks = record(9, 100.0, 1000);
    let mut r = Replayer::new(Lattice::plain(50).unwrap(), N_LEFS);
    for (chunk, end) in chunks {
        r.push(chunk, end).unwrap();
    }
    r.advance_to(100.0);
    assert!(matches!(r.world(), Err(ReplayError::World(_))));
}
