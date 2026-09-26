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

use std::collections::HashSet;

use js_sys::{Float64Array, Object, Reflect, Uint32Array, Uint8Array};
use lefsim::{Direction, EventFrames, EventRecorder, Lattice, Mobility, Rates, RunReport, Site, StopReason};
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
    set(&obj, "eventsFired", report.events as f64);
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
struct StateBuffers {
    left_pos: Vec<u32>,
    right_pos: Vec<u32>,
    left_mob: Vec<u8>,
    right_mob: Vec<u8>,
}

/// A simulation. See the `lefsim` TypeScript package for the documented API.
#[wasm_bindgen]
pub struct WasmSim {
    sim: lefsim::Sim,
    state: StateBuffers,
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

        let mut sites = vec![Site::plain(); n_sites as usize];
        let mut seen = HashSet::with_capacity(n_ctcf);
        for i in 0..n_ctcf {
            let site = ctcf_sites[i];
            if site >= n_sites {
                return Err(JsError::new(&format!(
                    "CTCF site {site} is outside a lattice of {n_sites} sites"
                )));
            }
            if !seen.insert(site) {
                return Err(JsError::new(&format!("CTCF site {site} is listed twice")));
            }
            let side = match ctcf_captures_from[i] {
                0 => Direction::Left,
                1 => Direction::Right,
                other => return Err(JsError::new(&format!("invalid CTCF side code {other}"))),
            };
            let p = ctcf_strength[i];
            if !(0.0..=1.0).contains(&p) {
                return Err(JsError::new(&format!("CTCF strength must be in [0, 1], got {p}")));
            }
            sites[site as usize] = Site::ctcf(side, p);
        }

        // The uniform spawn rule rejection-samples a free non-CTCF site, so
        // it would spin forever if none were free. Each LEF covers at most
        // two sites, so this bound guarantees one at every respawn.
        let total_lefs = lef_sites.len() + n_lefs as usize;
        let n_plain = n_sites as usize - n_ctcf;
        if 2 * total_lefs > n_plain {
            return Err(JsError::new(&format!(
                "too many LEFs: {total_lefs} LEFs need at least {} non-CTCF sites, \
                 the lattice has {n_plain}",
                2 * total_lefs
            )));
        }

        let defaults = Rates::default();
        let rates = Rates {
            step: positive_rate("step rate", step_rate, defaults.step)?,
            respawn_default: positive_rate("respawn rate", respawn_rate, defaults.respawn_default)?,
            respawn_captured: positive_rate(
                "captured respawn rate",
                respawn_rate_captured,
                defaults.respawn_captured,
            )?,
        };

        let mut builder = lefsim::Sim::builder(Lattice::new(sites)).seed(seed).rates(rates);
        for &pos in lef_sites {
            builder = builder.lef_at(pos);
        }
        let sim = builder
            .lefs(n_lefs as usize)
            .build()
            .map_err(|e| JsError::new(&e.to_string()))?;

        let n = sim.n_lefs();
        let state = StateBuffers {
            left_pos: vec![0; n],
            right_pos: vec![0; n],
            left_mob: vec![0; n],
            right_mob: vec![0; n],
        };
        Ok(WasmSim { sim, state })
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
    /// Returns `{time: Float64Array, legId: Uint32Array, pos: Uint32Array,
    /// mobility: Uint8Array, kind: Uint8Array, end, stopped}`. `end` is the
    /// time through which the chunk describes the state (the clock after the
    /// call). `stopped` is "recorder_full" when the chunk filled first; call
    /// again for the next chunk. Consecutive chunks tile the run exactly.
    pub fn record_chunk(&mut self, t_end: f64, max_rows: u32, keyframes: bool) -> Result<Object, JsError> {
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
        let f = rec.take_frames();
        let out = Object::new();
        set(&out, "time", Float64Array::from(f.time()));
        set(&out, "legId", Uint32Array::from(f.leg_id()));
        set(&out, "pos", Uint32Array::from(f.pos()));
        set(&out, "mobility", Uint8Array::from(f.mob()));
        set(&out, "kind", Uint8Array::from(f.kind()));
        set(&out, "end", report.time);
        set(&out, "stopped", stop_name(report.stopped));
        Ok(out)
    }

    // ----- introspection -----

    pub fn time(&self) -> f64 {
        self.sim.time()
    }

    /// Exact up to 2^53 events, which avoids BigInt on the JS side.
    pub fn event_count(&self) -> f64 {
        self.sim.event_count() as f64
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

    /// `{step, respawn, respawnCaptured}`
    pub fn rates(&self) -> Object {
        let r = self.sim.rates();
        let obj = Object::new();
        set(&obj, "step", r.step);
        set(&obj, "respawn", r.respawn_default);
        set(&obj, "respawnCaptured", r.respawn_captured);
        obj
    }

    // ----- live state -----

    /// Copy every LEF's current leg positions and mobility codes into the
    /// state buffers, indexed by `lef_id`.
    pub fn sync_state(&mut self) {
        let s = &mut self.state;
        for i in 0..s.left_pos.len() {
            let lef = i as u32;
            let (l, r) = self.sim.lef_positions(lef);
            let (lm, rm) = self.sim.lef_mobilities(lef);
            s.left_pos[i] = l;
            s.right_pos[i] = r;
            s.left_mob[i] = lm.code();
            s.right_mob[i] = rm.code();
        }
    }

    pub fn left_pos_ptr(&self) -> *const u32 {
        self.state.left_pos.as_ptr()
    }

    pub fn right_pos_ptr(&self) -> *const u32 {
        self.state.right_pos.as_ptr()
    }

    pub fn left_mob_ptr(&self) -> *const u8 {
        self.state.left_mob.as_ptr()
    }

    pub fn right_mob_ptr(&self) -> *const u8 {
        self.state.right_mob.as_ptr()
    }

    // ----- static metadata -----

    /// `{nSites, rates, lattice: {kind}, ctcfs: {site, capturesFrom,
    /// strength}, lefs: {leftLegId, rightLegId}, legs: {lefId, facing}}`,
    /// columns as typed arrays of the core's codes.
    pub fn meta(&self) -> Object {
        let m = self.sim.meta();
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
        set(&out, "lattice", table(&[("kind", Uint8Array::from(&m.lattice.kind[..]).into())]));
        set(
            &out,
            "ctcfs",
            table(&[
                ("site", Uint32Array::from(&m.ctcfs.site[..]).into()),
                ("capturesFrom", Uint8Array::from(&m.ctcfs.captures_from[..]).into()),
                ("strength", Float64Array::from(&m.ctcfs.strength[..]).into()),
            ]),
        );
        set(
            &out,
            "lefs",
            table(&[
                ("leftLegId", Uint32Array::from(&m.lefs.left_leg_id[..]).into()),
                ("rightLegId", Uint32Array::from(&m.lefs.right_leg_id[..]).into()),
            ]),
        );
        set(
            &out,
            "legs",
            table(&[
                ("lefId", Uint32Array::from(&m.legs.lef_id[..]).into()),
                ("facing", Uint8Array::from(&m.legs.facing[..]).into()),
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
