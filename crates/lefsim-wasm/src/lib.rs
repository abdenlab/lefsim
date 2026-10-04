//! WebAssembly bindings for `lefsim`, built with wasm-bindgen.
//!
//! This layer is deliberately thin: typed arrays and numbers in, plain JS
//! objects and typed arrays out. The idiomatic API (options objects, string
//! enums, validation messages) lives in the `lefsim` TypeScript package.
//!
//! Only [`WasmSim`] is a wasm-side handle. Everything returned from it is
//! copied into JS-owned memory, so it never needs freeing and never goes
//! stale. The one exception is live state: [`WasmSim::sync_state`] fills
//! buffers inside wasm memory, and JS reads them through views built from
//! the `*_ptr` methods. Those views are valid only until the next call into
//! the sim, since any wasm allocation can grow memory and detach them.
//!
//! Inputs are validated here too. A Rust panic in wasm traps and leaves the
//! module unusable, so bad input must become a JS exception instead.

use js_sys::{Float64Array, Object, Reflect, Uint8Array, Uint32Array};
use lefsim::{
    Ctcf, Direction, EventFrames, EventRecorder, Lattice, LefFrames, LefStates, LoopGraph,
    Mobility, Model, Rates, Recorder, RunReport, SampleMode, SpawnRule, StopReason, World,
};
use wasm_bindgen::prelude::*;

fn set(obj: &Object, key: &str, value: impl Into<JsValue>) {
    // Setting a plain data property on a plain object can't fail.
    Reflect::set(obj, &JsValue::from_str(key), &value.into()).expect("set property");
}

fn stop_name(stop: StopReason) -> &'static str {
    match stop {
        StopReason::ReachedTime => "reached_time",
        StopReason::ReachedEventCount => "reached_event_count",
        StopReason::Exhausted => "exhausted",
        StopReason::RecorderFull => "recorder_full",
    }
}

fn report_object(report: RunReport) -> Object {
    let obj = Object::new();
    set(&obj, "nEvents", report.n_events as f64);
    set(&obj, "time", report.time);
    set(&obj, "stopped", stop_name(report.stopped));
    obj
}

/// A JS number of events as a `u64` budget. Non-finite or huge means
/// "no limit".
fn event_budget(n: f64) -> Result<u64, JsError> {
    if n.is_nan() || n < 0.0 {
        return Err(JsError::new(&format!(
            "event count must be a non-negative number, got {n}"
        )));
    }
    Ok(if n >= u64::MAX as f64 {
        u64::MAX
    } else {
        n as u64
    })
}

/// Largest accepted chunk size. Bounded recorders preallocate their whole
/// cap, so an enormous "safety" value would trap on allocation instead of
/// throwing.
const MAX_CHUNK_ROWS: u32 = 10_000_000;

/// A simulation. See the `lefsim` TypeScript package for the documented API.
#[wasm_bindgen]
pub struct WasmSim {
    sim: lefsim::Sim,
    /// The world's LEF table as of the last `sync_state`. Refilled in place,
    /// so its buffers (and the views JS builds on them) don't move.
    state: LefStates,
    /// The most recent grid recorder, kept so that consecutive recording
    /// calls on the same grid continue it instead of repeating points.
    grid: Option<Recorder>,
}

/// How a recording call drives the sim.
enum Drive {
    /// To `t_end`, firing at most `cap` events.
    Until { t_end: f64, cap: u64 },
    /// Exactly `n` events (fewer if none remain).
    Events(u64),
}

/// `{time, leftPos, rightPos, leftMobility, rightMobility, nLefs}`: one
/// `time` per snapshot, `nLefs` entries per snapshot in the other columns.
fn snapshot_columns(frames: LefFrames) -> Object {
    let (n_lefs, time, lefs) = frames.into_parts();
    let out = Object::new();
    set(&out, "time", Float64Array::from(&time[..]));
    set(&out, "leftPos", Uint32Array::from(&lefs.left_pos[..]));
    set(&out, "rightPos", Uint32Array::from(&lefs.right_pos[..]));
    set(
        &out,
        "leftMobility",
        Uint8Array::from(Mobility::as_codes(&lefs.left_mobility)),
    );
    set(
        &out,
        "rightMobility",
        Uint8Array::from(Mobility::as_codes(&lefs.right_mobility)),
    );
    set(&out, "nLefs", n_lefs as u32);
    out
}

/// `{time, legId, pos, mobility, kind}`
fn event_columns(frames: EventFrames) -> Object {
    let out = Object::new();
    set(&out, "time", Float64Array::from(frames.time()));
    set(&out, "legId", Uint32Array::from(frames.leg_id()));
    set(&out, "pos", Uint32Array::from(frames.pos()));
    set(
        &out,
        "mobility",
        Uint8Array::from(Mobility::as_codes(frames.mob())),
    );
    set(&out, "kind", Uint8Array::from(frames.kind()));
    out
}

fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

/// A lattice from its size and CTCF columns (`side` holds `Direction` codes).
fn lattice_from(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
) -> Result<Lattice, JsError> {
    if side.len() != site.len() || strength.len() != site.len() {
        return Err(JsError::new(
            "CTCF site, side and strength arrays must have equal length",
        ));
    }
    let ctcfs = (0..site.len())
        .map(|i| {
            let captures_from = match side[i] {
                0 => Direction::Left,
                1 => Direction::Right,
                other => return Err(JsError::new(&format!("invalid CTCF side code {other}"))),
            };
            Ok(Ctcf {
                site: site[i],
                captures_from,
                strength: strength[i],
            })
        })
        .collect::<Result<Vec<_>, JsError>>()?;
    Lattice::with_ctcfs(n_sites as usize, &ctcfs).map_err(js_err)
}

/// A LEF table from its columns (mobility as `Mobility` codes). Column
/// lengths are checked by `World::from_lef_states`.
fn lefs_from(
    left_pos: &[u32],
    right_pos: &[u32],
    left_mob: &[u8],
    right_mob: &[u8],
) -> Result<LefStates, JsError> {
    let mobility = |codes: &[u8]| {
        codes
            .iter()
            .map(|&c| {
                Mobility::from_code(c)
                    .ok_or_else(|| JsError::new(&format!("invalid mobility code {c}")))
            })
            .collect::<Result<Vec<_>, JsError>>()
    };
    Ok(LefStates {
        left_pos: left_pos.to_vec(),
        right_pos: right_pos.to_vec(),
        left_mobility: mobility(left_mob)?,
        right_mobility: mobility(right_mob)?,
    })
}

fn rule_from(name: &str) -> Result<SpawnRule, JsError> {
    match name {
        "uniform_non_ctcf" => Ok(SpawnRule::UniformNonCtcf),
        _ => Err(JsError::new(&format!(
            "unknown spawn rule {name:?} (available: \"uniform_non_ctcf\")"
        ))),
    }
}

/// `{leftPos, rightPos, leftMobility, rightMobility}`, copied.
fn lefs_object(lefs: &LefStates) -> Object {
    let out = Object::new();
    set(&out, "leftPos", Uint32Array::from(&lefs.left_pos[..]));
    set(&out, "rightPos", Uint32Array::from(&lefs.right_pos[..]));
    set(
        &out,
        "leftMobility",
        Uint8Array::from(Mobility::as_codes(&lefs.left_mobility)),
    );
    set(
        &out,
        "rightMobility",
        Uint8Array::from(Mobility::as_codes(&lefs.right_mobility)),
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn model_from(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
    left_pos: &[u32],
    right_pos: &[u32],
    left_mob: &[u8],
    right_mob: &[u8],
    step: f64,
    respawn: f64,
    respawn_stalled: f64,
    respawn_evicted: f64,
    respawn_captured: f64,
    spawn_rule: &str,
) -> Result<Model, JsError> {
    let lattice = lattice_from(n_sites, site, side, strength)?;
    let lefs = lefs_from(left_pos, right_pos, left_mob, right_mob)?;
    let world = World::from_lef_states(lattice, &lefs).map_err(js_err)?;
    let rates = Rates {
        step,
        respawn,
        respawn_stalled,
        respawn_evicted,
        respawn_captured,
    };
    Model::new(world, rates, rule_from(spawn_rule)?).map_err(js_err)
}

// ----- validation and construction outside a sim -----

/// Throws unless the lattice is valid.
#[wasm_bindgen]
pub fn check_lattice(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
) -> Result<(), JsError> {
    lattice_from(n_sites, site, side, strength).map(|_| ())
}

/// Throws unless the LEFs make a valid world on the lattice.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn check_world(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
    left_pos: &[u32],
    right_pos: &[u32],
    left_mob: &[u8],
    right_mob: &[u8],
) -> Result<(), JsError> {
    let lattice = lattice_from(n_sites, site, side, strength)?;
    let lefs = lefs_from(left_pos, right_pos, left_mob, right_mob)?;
    World::from_lef_states(lattice, &lefs)
        .map(|_| ())
        .map_err(js_err)
}

/// Throws unless the world, rates and spawn rule make a valid model.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn check_model(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
    left_pos: &[u32],
    right_pos: &[u32],
    left_mob: &[u8],
    right_mob: &[u8],
    step: f64,
    respawn: f64,
    respawn_stalled: f64,
    respawn_evicted: f64,
    respawn_captured: f64,
    spawn_rule: &str,
) -> Result<(), JsError> {
    model_from(
        n_sites,
        site,
        side,
        strength,
        left_pos,
        right_pos,
        left_mob,
        right_mob,
        step,
        respawn,
        respawn_stalled,
        respawn_evicted,
        respawn_captured,
        spawn_rule,
    )
    .map(|_| ())
}

/// `n_lefs` LEFs placed by the spawn rule, as the LEF columns.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn populate(
    n_sites: u32,
    site: &[u32],
    side: &[u8],
    strength: &[f64],
    n_lefs: u32,
    seed: u64,
    spawn_rule: &str,
) -> Result<Object, JsError> {
    let lattice = lattice_from(n_sites, site, side, strength)?;
    let world = lefsim::populate(lattice, n_lefs as usize, &rule_from(spawn_rule)?, seed)
        .map_err(js_err)?;
    Ok(lefs_object(&world.lef_states()))
}

/// The core's default rates, `{step, respawn, respawnCaptured}`.
#[wasm_bindgen]
pub fn default_rates() -> Object {
    rates_object(Rates::default())
}

fn rates_object(r: Rates) -> Object {
    let obj = Object::new();
    set(&obj, "step", r.step);
    set(&obj, "respawn", r.respawn);
    set(&obj, "respawnStalled", r.respawn_stalled);
    set(&obj, "respawnEvicted", r.respawn_evicted);
    set(&obj, "respawnCaptured", r.respawn_captured);
    obj
}

fn grid_mode(dt: f64, t0: f64) -> Result<SampleMode, JsError> {
    if !(dt.is_finite() && dt > 0.0) {
        return Err(JsError::new(&format!(
            "dt must be positive and finite, got {dt}"
        )));
    }
    if !t0.is_finite() {
        return Err(JsError::new(&format!("t0 must be finite, got {t0}")));
    }
    Ok(SampleMode::EveryDt { dt, t0 })
}

#[wasm_bindgen]
impl WasmSim {
    /// A sim of the model given as columns (see `check_model`), from
    /// `start_time`. `side` and the mobility columns hold the core's codes.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        n_sites: u32,
        site: &[u32],
        side: &[u8],
        strength: &[f64],
        left_pos: &[u32],
        right_pos: &[u32],
        left_mob: &[u8],
        right_mob: &[u8],
        step: f64,
        respawn: f64,
        respawn_stalled: f64,
        respawn_evicted: f64,
        respawn_captured: f64,
        spawn_rule: &str,
        seed: u64,
        start_time: f64,
    ) -> Result<WasmSim, JsError> {
        if !start_time.is_finite() {
            return Err(JsError::new(&format!(
                "startTime must be finite, got {start_time}"
            )));
        }
        let model = model_from(
            n_sites,
            site,
            side,
            strength,
            left_pos,
            right_pos,
            left_mob,
            right_mob,
            step,
            respawn,
            respawn_stalled,
            respawn_evicted,
            respawn_captured,
            spawn_rule,
        )?;
        let sim = lefsim::Sim::with_start_time(model, seed, start_time);
        let state = sim.world().lef_states();
        Ok(WasmSim {
            sim,
            state,
            grid: None,
        })
    }

    // ----- driving -----

    /// Fire every event at or before `t_end` (absolute time), firing at most
    /// `max_events`. Returns `{eventsFired, time, stopped}`; `stopped` is
    /// "reached_event_count" if the cap cut the call short.
    pub fn run_until(&mut self, t_end: f64, max_events: f64) -> Result<Object, JsError> {
        if t_end.is_nan() {
            return Err(JsError::new("t_end is NaN"));
        }
        let cap = event_budget(max_events)?;
        Ok(report_object(self.sim.run_until_with_limit(
            t_end,
            cap,
            &mut (),
        )))
    }

    /// Fire up to `n` events. Returns `{eventsFired, time, stopped}`.
    pub fn run_events(&mut self, n: f64) -> Result<Object, JsError> {
        Ok(report_object(self.sim.run_events(event_budget(n)?)))
    }

    /// Run toward `t_end` (absolute time), logging events into one chunk of
    /// at most `max_rows` rows. With `keyframes`, the chunk opens with one
    /// `Keyframe` row per leg, and may exceed `max_rows` by the rows of a
    /// zero-delay chain in progress (chunks only end at settled times).
    ///
    /// Returns event columns (see `record_until`) plus `end`, the time
    /// through which the chunk describes the state (the clock after the
    /// call), and `stopped`: "recorder_full" when the chunk filled first;
    /// call again for the next chunk. Consecutive chunks tile the run
    /// exactly. Every event adds a row, so `max_rows` also bounds the work.
    pub fn record_event_chunk(
        &mut self,
        t_end: f64,
        max_rows: u32,
        keyframes: bool,
    ) -> Result<Object, JsError> {
        if t_end.is_nan() {
            return Err(JsError::new("t_end is NaN"));
        }
        let n_legs = self.sim.n_legs();
        let min = if keyframes { n_legs + 2 } else { 2 };
        if (max_rows as usize) < min {
            let why = if keyframes {
                format!(" with keyframes ({n_legs} legs + 2)")
            } else {
                String::new()
            };
            return Err(JsError::new(&format!(
                "maxRows must be at least {min}{why}, got {max_rows}"
            )));
        }
        if max_rows > MAX_CHUNK_ROWS {
            return Err(JsError::new(&format!(
                "maxRows must be at most {MAX_CHUNK_ROWS}, got {max_rows}"
            )));
        }
        let mut rec = EventRecorder::new().with_max_rows(max_rows as usize);
        if keyframes {
            rec = rec.with_keyframes(n_legs);
        }
        let report = self.sim.run_until_with(t_end, &mut rec);
        let out = event_columns(rec.take_frames());
        set(&out, "end", report.time);
        set(&out, "stopped", stop_name(report.stopped));
        Ok(out)
    }

    /// Run toward `t_end`, taking grid snapshots (`t0 + k * dt`) into one
    /// chunk of at most `max_snapshots`, firing at most `max_events`.
    ///
    /// Returns snapshot columns (see `record_until`) plus `end` and
    /// `stopped` ("recorder_full" or "reached_event_count" mean call again).
    /// The grid continues across calls on the same `dt`/`t0`.
    pub fn record_snapshot_chunk(
        &mut self,
        t_end: f64,
        dt: f64,
        t0: f64,
        max_snapshots: u32,
        max_events: f64,
    ) -> Result<Object, JsError> {
        if t_end.is_nan() {
            return Err(JsError::new("t_end is NaN"));
        }
        let n_lefs = self.sim.n_lefs().max(1) as u64;
        if max_snapshots == 0 || max_snapshots as u64 * n_lefs > MAX_CHUNK_ROWS as u64 {
            return Err(JsError::new(&format!(
                "maxSnapshots must be positive, and at most {} with {n_lefs} LEFs, got {max_snapshots}",
                MAX_CHUNK_ROWS as u64 / n_lefs
            )));
        }
        let cap = event_budget(max_events)?;
        let mut rec = self.grid_recorder(grid_mode(dt, t0)?);
        rec.set_max_snapshots(Some(max_snapshots as usize));
        let report = self.sim.run_until_with_limit(t_end, cap, &mut rec);
        let out = snapshot_columns(rec.take_frames());
        self.grid = Some(rec);
        set(&out, "end", report.time);
        set(&out, "stopped", stop_name(report.stopped));
        Ok(out)
    }

    /// Run to `t_end`, firing at most `max_events`, and record: grid
    /// snapshots if `dt` is given, event deltas if `events` (keyframed if
    /// `keyframes`). Returns `{eventsFired, time, stopped, snapshots, events}`
    /// where `snapshots` is `{time, leftPos, rightPos, leftMobility,
    /// rightMobility, nLefs}` or null and `events` is `{time, legId, pos,
    /// mobility, kind}` or null.
    pub fn record_until(
        &mut self,
        t_end: f64,
        max_events: f64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
    ) -> Result<Object, JsError> {
        if t_end.is_nan() {
            return Err(JsError::new("t_end is NaN"));
        }
        let cap = event_budget(max_events)?;
        self.record(Drive::Until { t_end, cap }, dt, t0, events, keyframes)
    }

    /// Fire up to `n` events, recording as in `record_until`.
    pub fn record_events(
        &mut self,
        n: f64,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
    ) -> Result<Object, JsError> {
        let n = event_budget(n)?;
        self.record(Drive::Events(n), dt, t0, events, keyframes)
    }

    fn record(
        &mut self,
        drive: Drive,
        dt: Option<f64>,
        t0: f64,
        events: bool,
        keyframes: bool,
    ) -> Result<Object, JsError> {
        if keyframes && !events {
            return Err(JsError::new("keyframes require events"));
        }
        let mode = dt.map(|dt| grid_mode(dt, t0)).transpose()?;
        let mut deltas = events.then(|| {
            let rec = EventRecorder::new();
            if keyframes {
                rec.with_keyframes(self.sim.n_legs())
            } else {
                rec
            }
        });
        let mut grid = mode.map(|m| {
            let mut rec = self.grid_recorder(m);
            rec.set_max_snapshots(None);
            rec
        });
        let obs = &mut (&mut grid, &mut deltas);
        let report = match drive {
            Drive::Until { t_end, cap } => self.sim.run_until_with_limit(t_end, cap, obs),
            Drive::Events(n) => self.sim.run_events_with(n, obs),
        };
        let out = report_object(report);
        let snapshots = match grid.as_mut() {
            Some(rec) => snapshot_columns(rec.take_frames()).into(),
            None => JsValue::NULL,
        };
        if grid.is_some() {
            self.grid = grid;
        }
        set(&out, "snapshots", snapshots);
        set(
            &out,
            "events",
            deltas.map_or(JsValue::NULL, |mut d| event_columns(d.take_frames()).into()),
        );
        Ok(out)
    }

    /// The kept grid recorder if it's on the same grid, else a new one.
    fn grid_recorder(&mut self, mode: SampleMode) -> Recorder {
        match self.grid.take() {
            Some(rec) if rec.mode() == mode => rec,
            _ => self.sim.recorder(mode),
        }
    }

    // ----- introspection -----

    pub fn time(&self) -> f64 {
        self.sim.time()
    }

    /// Exact up to 2^53 events, which avoids BigInt on the JS side.
    pub fn n_events(&self) -> f64 {
        self.sim.n_events() as f64
    }

    pub fn next_event_time(&self) -> Option<f64> {
        self.sim.next_event_time()
    }

    pub fn n_sites(&self) -> u32 {
        self.sim.n_sites() as u32
    }

    pub fn n_lefs(&self) -> u32 {
        self.sim.n_lefs() as u32
    }

    pub fn n_legs(&self) -> u32 {
        self.sim.n_legs() as u32
    }

    /// `{step, respawn, respawnCaptured}`
    pub fn rates(&self) -> Object {
        rates_object(self.sim.rates())
    }

    /// The current LEF table, `{leftPos, rightPos, leftMobility,
    /// rightMobility}`, copied (safe to keep).
    pub fn lefs(&self) -> Object {
        lefs_object(&self.sim.world().lef_states())
    }

    /// The LEF table this run started from, copied.
    pub fn initial_lefs(&self) -> Object {
        let model = self
            .sim
            .initial_model()
            .expect("wasm sims use built-in spawn rules");
        lefs_object(&model.world().lef_states())
    }

    /// The seed this run was started with.
    pub fn seed(&self) -> u64 {
        self.sim.seed()
    }

    /// The clock at construction.
    pub fn start_time(&self) -> f64 {
        self.sim.start_time()
    }

    /// Fire one event. Returns `[time, actionCode, id]` (`id` is a `leg_id`
    /// for a step, a `lef_id` for a respawn) or `null` if none remain.
    pub fn step(&mut self) -> JsValue {
        match self.sim.step() {
            None => JsValue::NULL,
            Some((t, action)) => {
                let id = match action {
                    lefsim::Action::Step { leg } => self.sim.leg_id(leg),
                    lefsim::Action::Respawn { lef } => self.sim.lef_id(lef),
                }
                .expect("the sim's own entity");
                js_sys::Array::of3(&t.into(), &action.code().into(), &id.into()).into()
            }
        }
    }

    /// An exact fork: a copy that continues the run as this one would.
    pub fn fork(&self) -> WasmSim {
        WasmSim {
            sim: self.sim.clone(),
            state: self.state.clone(),
            grid: self.grid.clone(),
        }
    }

    // ----- live state -----

    /// Copy every LEF's current leg positions and mobility codes into the
    /// state buffers, indexed by `lef_id`.
    pub fn sync_state(&mut self) {
        self.state.clear();
        self.sim.world().push_lef_states(&mut self.state);
    }

    pub fn left_pos_ptr(&self) -> *const u32 {
        self.state.left_pos.as_ptr()
    }

    pub fn right_pos_ptr(&self) -> *const u32 {
        self.state.right_pos.as_ptr()
    }

    pub fn left_mob_ptr(&self) -> *const u8 {
        Mobility::as_codes(&self.state.left_mobility).as_ptr()
    }

    pub fn right_mob_ptr(&self) -> *const u8 {
        Mobility::as_codes(&self.state.right_mobility).as_ptr()
    }
}

// ----- effective distance -----

/// Step and hop lengths from JS numbers, checked so every distance is an
/// exact JS number: a distance is at most the walk between its sites,
/// `step_length * (n_sites - 1)`.
fn lengths_from(n_sites: u32, step_length: f64, hop_length: f64) -> Result<(u64, u64), JsError> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0; // 2^53 - 1
    for (name, v) in [("stepLength", step_length), ("hopLength", hop_length)] {
        if !(v.fract() == 0.0 && (0.0..=MAX_SAFE).contains(&v)) {
            return Err(JsError::new(&format!(
                "{name} must be a non-negative integer, got {v}"
            )));
        }
    }
    if step_length * f64::from(n_sites.saturating_sub(1)) > MAX_SAFE {
        return Err(JsError::new(
            "stepLength is too large: distances on this lattice could exceed 2^53",
        ));
    }
    Ok((step_length as u64, hop_length as u64))
}

fn distances_array(d: &[u64]) -> Float64Array {
    let d: Vec<f64> = d.iter().map(|&x| x as f64).collect();
    Float64Array::from(&d[..])
}

/// Effective distances in one state. Without `targets`, every pair of
/// `sites` (condensed upper triangle, row-major); with `targets`, each of
/// `sites` to each of `targets`, row-major.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn effective_distances(
    n_sites: u32,
    left_pos: &[u32],
    right_pos: &[u32],
    sites: &[u32],
    targets: Option<Box<[u32]>>,
    step_length: f64,
    hop_length: f64,
) -> Result<Float64Array, JsError> {
    if left_pos.len() != right_pos.len() {
        return Err(JsError::new("leftPos and rightPos must have equal length"));
    }
    let (s, h) = lengths_from(n_sites, step_length, hop_length)?;
    let graph =
        LoopGraph::from_legs(n_sites as usize, left_pos, right_pos, s, h).map_err(js_err)?;
    let d = match targets {
        Some(t) => graph.distances(sites, &t),
        None => graph.pairwise(sites),
    };
    Ok(distances_array(&d.map_err(js_err)?))
}

/// Every pair of `sites` in each of a series of snapshots (stacked LEF
/// columns, `n_lefs` per snapshot): one condensed row per snapshot.
#[wasm_bindgen]
pub fn effective_distances_over(
    n_sites: u32,
    n_lefs: u32,
    left_pos: &[u32],
    right_pos: &[u32],
    sites: &[u32],
    step_length: f64,
    hop_length: f64,
) -> Result<Float64Array, JsError> {
    let n = n_lefs as usize;
    if left_pos.len() != right_pos.len() || !left_pos.len().is_multiple_of(n) {
        return Err(JsError::new(
            "leftPos and rightPos must hold nLefs entries per snapshot",
        ));
    }
    let (s, h) = lengths_from(n_sites, step_length, hop_length)?;
    let mut out = Vec::new();
    if n > 0 {
        for (l, r) in left_pos.chunks(n).zip(right_pos.chunks(n)) {
            let graph = LoopGraph::from_legs(n_sites as usize, l, r, s, h).map_err(js_err)?;
            out.extend(graph.pairwise(sites).map_err(js_err)?);
        }
    }
    Ok(distances_array(&out))
}

/// Code → name tables from the core, so JS never hardcodes them.
#[wasm_bindgen]
pub fn names() -> Object {
    let to_array = |names: &[&str]| {
        names
            .iter()
            .map(|&n| JsValue::from_str(n))
            .collect::<js_sys::Array>()
    };
    let obj = Object::new();
    set(&obj, "mobility", to_array(&Mobility::NAMES));
    set(&obj, "direction", to_array(&Direction::NAMES));
    set(&obj, "siteKind", to_array(&lefsim::SiteKind::NAMES));
    set(&obj, "rowKind", to_array(&EventFrames::KIND_NAMES));
    obj
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
