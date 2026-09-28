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

use js_sys::{Float64Array, Object, Reflect, Uint32Array, Uint8Array};
use lefsim::{
    Ctcf, Direction, EventFrames, EventRecorder, Lattice, LefFrames, LefStates, Mobility, Model, Rates, Recorder, RunReport,
    SampleMode, SpawnRule, StopReason,
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
    set(&obj, "eventsFired", report.n_events as f64);
    set(&obj, "time", report.time);
    set(&obj, "stopped", stop_name(report.stopped));
    obj
}

/// A JS number of events as a `u64` budget. Non-finite or huge means
/// "no limit".
fn event_budget(n: f64) -> Result<u64, JsError> {
    if n.is_nan() || n < 0.0 {
        return Err(JsError::new(&format!("event count must be a non-negative number, got {n}")));
    }
    Ok(if n >= u64::MAX as f64 { u64::MAX } else { n as u64 })
}

/// Largest accepted chunk size. Bounded recorders preallocate their whole
/// cap, so an enormous "safety" value would trap on allocation instead of
/// throwing.
const MAX_CHUNK_ROWS: u32 = 10_000_000;

fn positive_rate(name: &str, value: Option<f64>, default: f64) -> Result<f64, JsError> {
    let rate = value.unwrap_or(default);
    if !(rate.is_finite() && rate > 0.0) {
        return Err(JsError::new(&format!("{name} must be positive and finite, got {rate}")));
    }
    Ok(rate)
}

/// Contiguous per-LEF copies of the live state, for zero-copy JS views.
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
    set(&out, "leftMobility", Uint8Array::from(Mobility::as_codes(&lefs.left_mobility)));
    set(&out, "rightMobility", Uint8Array::from(Mobility::as_codes(&lefs.right_mobility)));
    set(&out, "nLefs", n_lefs as u32);
    out
}

/// `{time, legId, pos, mobility, kind}`
fn event_columns(frames: EventFrames) -> Object {
    let out = Object::new();
    set(&out, "time", Float64Array::from(frames.time()));
    set(&out, "legId", Uint32Array::from(frames.leg_id()));
    set(&out, "pos", Uint32Array::from(frames.pos()));
    set(&out, "mobility", Uint8Array::from(Mobility::as_codes(frames.mob())));
    set(&out, "kind", Uint8Array::from(frames.kind()));
    out
}

/// A world with a moving LEF at each of `lef_sites`, then `n_lefs` more
/// placed by the uniform spawn rule. With no explicit sites, that's
/// [`lefsim::populate`]; mixing the two goes away with the new constructor.
fn initial_world(lattice: Lattice, lef_sites: &[u32], n_lefs: usize, seed: u64) -> Result<lefsim::World, String> {
    use lefsim::{Mobility::Moving, Spawn};
    use rand::SeedableRng;

    let rule = SpawnRule::UniformNonCtcf;
    if lef_sites.is_empty() {
        return lefsim::populate(lattice, n_lefs, &rule, seed).map_err(|e| e.to_string());
    }
    rule.check(&lattice, lef_sites.len() + n_lefs).map_err(|e| e.to_string())?;
    let mut world = lefsim::World::new(lattice);
    for &p in lef_sites {
        world.add_lef((p, Moving), (p, Moving)).map_err(|e| e.to_string())?;
    }
    let mut rng = lefsim::SimRng::seed_from_u64(seed);
    for _ in 0..n_lefs {
        // The uniform rule only picks plain sites, where landing is Moving.
        let p = rule.sample(&world, &mut rng);
        world.add_lef((p, Moving), (p, Moving)).map_err(|e| e.to_string())?;
    }
    Ok(world)
}

fn grid_mode(dt: f64, t0: f64) -> Result<SampleMode, JsError> {
    if !(dt.is_finite() && dt > 0.0) {
        return Err(JsError::new(&format!("dt must be positive and finite, got {dt}")));
    }
    if !t0.is_finite() {
        return Err(JsError::new(&format!("t0 must be finite, got {t0}")));
    }
    Ok(SampleMode::EveryDt { dt, t0 })
}

#[wasm_bindgen]
impl WasmSim {
    /// `ctcf_captures_from` holds `Direction` codes (0 = left, 1 = right).
    /// Omitted rates fall back to the core's `Rates::default()`.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        n_sites: u32,
        ctcf_sites: &[u32],
        ctcf_captures_from: &[u8],
        ctcf_strength: &[f64],
        lef_sites: &[u32],
        n_lefs: u32,
        seed: u64,
        step_rate: Option<f64>,
        respawn_rate: Option<f64>,
        respawn_rate_captured: Option<f64>,
    ) -> Result<WasmSim, JsError> {
        let n_ctcf = ctcf_sites.len();
        if ctcf_captures_from.len() != n_ctcf || ctcf_strength.len() != n_ctcf {
            return Err(JsError::new("CTCF site, side and strength arrays must have equal length"));
        }

        let ctcfs = (0..n_ctcf)
            .map(|i| {
                let captures_from = match ctcf_captures_from[i] {
                    0 => Direction::Left,
                    1 => Direction::Right,
                    other => return Err(JsError::new(&format!("invalid CTCF side code {other}"))),
                };
                Ok(Ctcf { site: ctcf_sites[i], captures_from, strength: ctcf_strength[i] })
            })
            .collect::<Result<Vec<_>, JsError>>()?;
        let lattice = Lattice::with_ctcfs(n_sites as usize, &ctcfs).map_err(|e| JsError::new(&e.to_string()))?;

        let defaults = Rates::default();
        let rates = Rates {
            step: positive_rate("step rate", step_rate, defaults.step)?,
            respawn: positive_rate("respawn rate", respawn_rate, defaults.respawn)?,
            respawn_captured: positive_rate(
                "captured respawn rate",
                respawn_rate_captured,
                defaults.respawn_captured,
            )?,
        };

        let world = initial_world(lattice, lef_sites, n_lefs as usize, seed).map_err(|e| JsError::new(&e))?;
        let model = Model::new(world, rates, SpawnRule::UniformNonCtcf).map_err(|e| JsError::new(&e.to_string()))?;
        let sim = lefsim::Sim::new(model, seed);

        let state = sim.world().lef_states();
        Ok(WasmSim { sim, state, grid: None })
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
        Ok(report_object(self.sim.run_until_with_limit(t_end, cap, &mut ())))
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
    pub fn record_event_chunk(&mut self, t_end: f64, max_rows: u32, keyframes: bool) -> Result<Object, JsError> {
        if t_end.is_nan() {
            return Err(JsError::new("t_end is NaN"));
        }
        let n_legs = self.sim.n_legs();
        let min = if keyframes { n_legs + 2 } else { 2 };
        if (max_rows as usize) < min {
            let why = if keyframes { format!(" with keyframes ({n_legs} legs + 2)") } else { String::new() };
            return Err(JsError::new(&format!("maxRows must be at least {min}{why}, got {max_rows}")));
        }
        if max_rows > MAX_CHUNK_ROWS {
            return Err(JsError::new(&format!("maxRows must be at most {MAX_CHUNK_ROWS}, got {max_rows}")));
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

    fn record(&mut self, drive: Drive, dt: Option<f64>, t0: f64, events: bool, keyframes: bool) -> Result<Object, JsError> {
        if keyframes && !events {
            return Err(JsError::new("keyframes require events"));
        }
        let mode = dt.map(|dt| grid_mode(dt, t0)).transpose()?;
        let mut deltas = events.then(|| {
            let rec = EventRecorder::new();
            if keyframes { rec.with_keyframes(self.sim.n_legs()) } else { rec }
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
        set(&out, "events", deltas.map_or(JsValue::NULL, |mut d| event_columns(d.take_frames()).into()));
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
    pub fn event_count(&self) -> f64 {
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
        let r = self.sim.rates();
        let obj = Object::new();
        set(&obj, "step", r.step);
        set(&obj, "respawn", r.respawn);
        set(&obj, "respawnCaptured", r.respawn_captured);
        obj
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

    // ----- static metadata -----

    /// `{nSites, rates, lattice: {kind}, ctcfs: {site, capturesFrom,
    /// strength}, lefs: {leftLegId, rightLegId}, legs: {lefId, facing}}`,
    /// columns as typed arrays of the core's codes.
    pub fn meta(&self) -> Object {
        let lattice = self.sim.world().lattice();
        let kinds: Vec<u8> = lattice.sites().iter().map(|s| s.kind.code()).collect();
        let ctcfs = lattice.ctcfs();
        let n = self.sim.n_lefs() as u32;
        let table = |cols: &[(&str, JsValue)]| {
            let obj = Object::new();
            for (k, v) in cols {
                set(&obj, k, v.clone());
            }
            obj
        };
        let out = Object::new();
        set(&out, "nSites", self.sim.n_sites() as u32);
        set(&out, "rates", self.rates());
        set(&out, "lattice", table(&[("kind", Uint8Array::from(&kinds[..]).into())]));
        set(
            &out,
            "ctcfs",
            table(&[
                ("site", Uint32Array::from(&ctcfs.iter().map(|c| c.site).collect::<Vec<_>>()[..]).into()),
                ("capturesFrom", Uint8Array::from(&ctcfs.iter().map(|c| c.captures_from.code()).collect::<Vec<_>>()[..]).into()),
                ("strength", Float64Array::from(&ctcfs.iter().map(|c| c.strength).collect::<Vec<_>>()[..]).into()),
            ]),
        );
        set(
            &out,
            "lefs",
            table(&[
                ("leftLegId", Uint32Array::from(&(0..n).map(|i| 2 * i).collect::<Vec<_>>()[..]).into()),
                ("rightLegId", Uint32Array::from(&(0..n).map(|i| 2 * i + 1).collect::<Vec<_>>()[..]).into()),
            ]),
        );
        set(
            &out,
            "legs",
            table(&[
                ("lefId", Uint32Array::from(&(0..2 * n).map(|i| i / 2).collect::<Vec<_>>()[..]).into()),
                ("facing", Uint8Array::from(&(0..2 * n).map(|i| [Direction::Left, Direction::Right][i as usize % 2].code()).collect::<Vec<_>>()[..]).into()),
            ]),
        );
        out
    }
}

/// Code → name tables from the core, so JS never hardcodes them.
#[wasm_bindgen]
pub fn names() -> Object {
    let to_array = |names: &[&str]| names.iter().map(|&n| JsValue::from_str(n)).collect::<js_sys::Array>();
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
