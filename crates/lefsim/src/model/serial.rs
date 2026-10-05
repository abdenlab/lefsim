//! Serialized forms of the validated types (the `serde` feature).
//!
//! [`Lattice`], [`World`] and [`Model`] never deserialize field by field:
//! serde goes through these plain-data mirrors and then the usual
//! constructors, so loading checks everything building does, and derived
//! state (occupancy, slotmap keys) is rebuilt rather than read.

use serde::{Deserialize, Serialize};

use super::{
    Ctcf, Lattice, LatticeError, LefStates, Mobility, Model, Position, Rates, SpawnRule, World,
    WorldError,
};

/// The `format` field of a model document.
pub(super) const MODEL_FORMAT: &str = "lefsim.model";
/// The current `version` of the model document.
pub(super) const MODEL_VERSION: u32 = 2;

/// One row of a [`LefStates`] table. Mobility defaults to moving.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LefRow {
    left_pos: Position,
    right_pos: Position,
    #[serde(default = "moving")]
    left_mobility: Mobility,
    #[serde(default = "moving")]
    right_mobility: Mobility,
}

fn moving() -> Mobility {
    Mobility::Moving
}

impl From<Vec<LefRow>> for LefStates {
    fn from(rows: Vec<LefRow>) -> Self {
        let mut lefs = LefStates::with_capacity(rows.len());
        for r in rows {
            lefs.push(
                (r.left_pos, r.left_mobility),
                (r.right_pos, r.right_mobility),
            );
        }
        lefs
    }
}

impl From<LefStates> for Vec<LefRow> {
    fn from(lefs: LefStates) -> Self {
        (0..lefs.len())
            .map(|i| {
                let ((left_pos, left_mobility), (right_pos, right_mobility)) =
                    lefs.get(i).expect("in range");
                LefRow {
                    left_pos,
                    right_pos,
                    left_mobility,
                    right_mobility,
                }
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LatticeData {
    n_sites: usize,
    #[serde(default)]
    ctcfs: Vec<Ctcf>,
}

impl TryFrom<LatticeData> for Lattice {
    type Error = LatticeError;
    fn try_from(d: LatticeData) -> Result<Self, Self::Error> {
        Lattice::with_ctcfs(d.n_sites, &d.ctcfs)
    }
}

impl From<Lattice> for LatticeData {
    fn from(l: Lattice) -> Self {
        LatticeData {
            n_sites: l.len(),
            ctcfs: l.ctcfs(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorldData {
    lattice: Lattice,
    lefs: LefStates,
}

impl TryFrom<WorldData> for World {
    type Error = WorldError;
    fn try_from(d: WorldData) -> Result<Self, Self::Error> {
        World::from_lef_states(d.lattice, &d.lefs)
    }
}

impl From<World> for WorldData {
    fn from(w: World) -> Self {
        WorldData {
            lefs: w.lef_states(),
            lattice: w.lattice,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelData {
    format: String,
    version: u32,
    lattice: Lattice,
    lefs: LefStates,
    rates: Rates,
    spawn: SpawnRule,
}

impl TryFrom<ModelData> for Model {
    type Error = String;
    fn try_from(d: ModelData) -> Result<Self, Self::Error> {
        if d.format != MODEL_FORMAT {
            return Err(format!(
                "expected format {MODEL_FORMAT:?}, got {:?}",
                d.format
            ));
        }
        if d.version != MODEL_VERSION {
            return Err(format!(
                "unsupported {MODEL_FORMAT} version {} (this build reads version {MODEL_VERSION})",
                d.version
            ));
        }
        let world = World::from_lef_states(d.lattice, &d.lefs).map_err(|e| e.to_string())?;
        Model::new(world, d.rates, d.spawn).map_err(|e| e.to_string())
    }
}

impl From<Model> for ModelData {
    fn from(m: Model) -> Self {
        ModelData {
            format: MODEL_FORMAT.to_string(),
            version: MODEL_VERSION,
            lefs: m.world.lef_states(),
            lattice: m.world.lattice,
            rates: m.rates,
            spawn: m.spawn,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::{Direction, Mobility};

    fn model() -> Model {
        let ctcfs = [
            Ctcf {
                site: 30,
                captures_from: Direction::Right,
                strength: 0.8,
            },
            Ctcf {
                site: 70,
                captures_from: Direction::Left,
                strength: 1.0,
            },
        ];
        let mut world = World::new(Lattice::with_ctcfs(100, &ctcfs).unwrap());
        world
            .add_lef((10, Mobility::Moving), (12, Mobility::Moving))
            .unwrap();
        world
            .add_lef((30, Mobility::Captured), (70, Mobility::Captured))
            .unwrap();
        let rates = Rates {
            step: 1.0,
            respawn: 0.002,
            respawn_blocked: 0.02,
            respawn_captured: 0.0004,
        };
        Model::new(world, rates, SpawnRule::UniformNonCtcf).unwrap()
    }

    fn document() -> serde_json::Value {
        json!({
            "format": "lefsim.model",
            "version": 2,
            "lattice": {
                "n_sites": 100,
                "ctcfs": [
                    { "site": 30, "captures_from": "right", "strength": 0.8 },
                    { "site": 70, "captures_from": "left", "strength": 1.0 },
                ],
            },
            "lefs": [
                { "left_pos": 10, "right_pos": 12, "left_mobility": "moving", "right_mobility": "moving" },
                { "left_pos": 30, "right_pos": 70, "left_mobility": "captured", "right_mobility": "captured" },
            ],
            "rates": { "step": 1.0, "respawn": 0.002, "respawn_blocked": 0.02, "respawn_captured": 0.0004 },
            "spawn": { "rule": "uniform_non_ctcf" },
        })
    }

    #[test]
    fn model_serializes_to_the_documented_shape() {
        assert_eq!(serde_json::to_value(model()).unwrap(), document());
    }

    #[test]
    fn model_round_trips() {
        let text = serde_json::to_string(&model()).unwrap();
        let back: Model = serde_json::from_str(&text).unwrap();
        assert_eq!(back, model());

        // A running sim's model round-trips too, and restarts identically.
        let mut sim = crate::Sim::new(model(), 5);
        sim.run_until(300.0);
        let now = sim.model().unwrap();
        let back: Model = serde_json::from_str(&serde_json::to_string(&now).unwrap()).unwrap();
        assert_eq!(back, now);
        let (mut a, mut b) = (crate::Sim::new(now, 1), crate::Sim::new(back, 1));
        a.run_until(300.0);
        b.run_until(300.0);
        assert_eq!(a.world(), b.world());
    }

    #[test]
    fn lattice_and_world_round_trip_on_their_own() {
        let world = model().world().clone();
        let text = serde_json::to_string(&world).unwrap();
        assert_eq!(serde_json::from_str::<World>(&text).unwrap(), world);
        let lattice: Lattice = serde_json::from_value(json!({ "n_sites": 5 })).unwrap();
        assert_eq!(lattice, Lattice::plain(5).unwrap());

        // Mobility may be left out: LEFs load as moving.
        let short: World = serde_json::from_value(json!({
            "lattice": { "n_sites": 5 },
            "lefs": [{ "left_pos": 1, "right_pos": 3 }],
        }))
        .unwrap();
        assert_eq!(
            short.lef_state(0),
            Some(((1, Mobility::Moving), (3, Mobility::Moving)))
        );
    }

    /// Each edit breaks the document; loading must say why.
    #[test]
    fn invalid_documents_are_rejected() {
        let cases: [(&str, serde_json::Value, &str); 8] = [
            ("/format", json!("something.else"), "expected format"),
            ("/version", json!(1), "unsupported"),
            ("/lattice/n_sites", json!(0), "at least one site"),
            ("/lattice/ctcfs/0/strength", json!(1.5), "[0, 1]"),
            ("/lefs/0/right_pos", json!(30), "already occupied"),
            ("/lefs/0/left_mobility", json!("captured"), "not a CTCF"),
            ("/rates/step", json!(-1.0), "step rate"),
            ("/spawn/rule", json!("anywhere"), "unknown variant"),
        ];
        for (path, value, message) in cases {
            let mut doc = document();
            *doc.pointer_mut(path).unwrap() = value;
            let err = serde_json::from_value::<Model>(doc)
                .unwrap_err()
                .to_string();
            assert!(err.contains(message), "{path}: {err}");
        }

        let mut doc = document();
        doc["lefs"][0].as_object_mut().unwrap().remove("right_pos");
        let err = serde_json::from_value::<Model>(doc)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `right_pos`"), "{err}");

        let mut doc = document();
        doc["rates"]["setp"] = json!(1.0);
        let err = serde_json::from_value::<Model>(doc)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"), "{err}");
    }
}
