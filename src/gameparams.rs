//! GameParams: the client's table of every ship, projectile, aircraft and
//! more, by GameParams id (the `shipId` / `paramsID` values replays carry).
//!
//! `content/GameParams.data` is a zlib stream written back to front; inflated,
//! it is a Python 2 pickle of `{region: {name: GPData}}`. Region `""` is the
//! base table; the others (`RU`, `NA`, …) share almost all of its entries.
//! Only the fields the battle world and the labels need are extracted.
//!
//! Loading the full pickle takes about 2 s and peaks around 1.2 GB (15
//! million values); the extract kept afterwards is small.

use std::collections::HashMap;
use std::io::Read;

use serde::Deserialize;
use serde::Serialize;

use crate::pickle;
use crate::pickle::Value;

#[derive(Debug, thiserror::Error)]
pub enum GameParamsError {
    #[error("read or inflate: {0}")]
    Io(#[from] std::io::Error),
    #[error("pickle: {0}")]
    Pickle(#[from] pickle::PickleError),
    #[error("unexpected layout: {0}")]
    Layout(&'static str),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShipParams {
    pub id: i64,
    /// `PJSD207`; the ship's display name is `IDS_<index>` in the catalogue.
    pub index: String,
    /// `PJSD207_Shiratsuyu`.
    pub name: String,
    /// `Destroyer`, `Cruiser`, `Battleship`, `AirCarrier`, `Submarine`, …
    pub species: String,
    pub nation: String,
    pub level: i64,
    /// Every consumable the ship can carry (all slots, all alternatives).
    pub consumables: Vec<ConsumableParams>,
    /// Projectile names the secondary battery (`atba`) of any hull fires.
    pub secondary_ammo: Vec<String>,
}

/// One variant of an ability (`PCY019_RLSSearch` / `USSR_10`), base values
/// before upgrades and commander skills.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsumableParams {
    /// `rls`, `sonar`, `crashCrew`, …; see `consumable_type` for the id
    /// replays use.
    pub kind: String,
    pub work_time: f32,
    /// Area in world units: patrol radius for fighters, else the detection
    /// range (`distShip`; the hydrophone's `hydrophoneWaveRadius`).
    pub radius: Option<f32>,
}

/// The consumable type for the numeric id replays carry. Fitted on the local
/// 15.9/15.10 replays (each id's uses match one type's work time on every
/// ship that used it); only ids seen there are listed.
pub fn consumable_type(id: i64) -> Option<&'static str> {
    Some(match id {
        0 => "crashCrew",
        1 => "scout",
        3 => "speedBoosters",
        4 => "artilleryBoosters",
        6 => "smokeGenerator",
        8 => "regenCrew",
        9 => "fighter",
        10 => "sonar",
        11 => "torpedoReloader",
        12 => "rls",
        31 => "buff",
        36 => "hydrophone",
        37 => "fastRudders",
        38 => "subsEnergyFreeze",
        42 => "submarineLocator",
        65 => "auxTorpBooster",
        _ => return None,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectileParams {
    pub id: i64,
    pub index: String,
    pub name: String,
    /// `Artillery`, `Torpedo`, `Bomb`, `DepthCharge`, …
    pub species: String,
    /// Range in world units (torpedoes; base value, before upgrades, which
    /// add up to about a quarter on the local replays).
    pub max_dist: Option<f32>,
    /// Knots (torpedoes; base value).
    pub speed: Option<f32>,
    /// `AP`, `HE`, `CS` (semi-armour-piercing), `torpedo`, …
    pub ammo_type: String,
    /// Calibre in metres (`bulletDiametr`; shells).
    pub caliber: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AircraftParams {
    pub id: i64,
    pub index: String,
    pub name: String,
    /// `Fighter`, `Dive`, `Bomber`, `Scout`, …
    pub species: String,
    /// `planeSubtype`: `consumable` (catapult fighters, spotters),
    /// `airsupport` (depth-charge and mine planes); empty for carrier planes.
    pub subtypes: Vec<String>,
    /// The `ammoType` of its `bombName` projectile; empty without one.
    pub ammo_type: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameParams {
    pub ships: HashMap<i64, ShipParams>,
    pub projectiles: HashMap<i64, ProjectileParams>,
    pub aircraft: HashMap<i64, AircraftParams>,
    /// Building species (`AirBase`, `AntiAircraft`, `CoastalArtillery`, …).
    pub buildings: HashMap<i64, String>,
    /// Arms-race drops: minimap marker names (`markerNameActive`,
    /// `markerNameInactive`), e.g. `damage_active`.
    pub drops: HashMap<i64, (String, String)>,
}

fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn float(v: Option<&Value>) -> Option<f32> {
    v.and_then(Value::as_f64).map(|f| f as f32)
}

/// `ShipAbilities: {slot: {abils: [(ability name, variant), …]}}`.
fn ship_consumables(ship: &Value, abilities: &HashMap<&str, &Value>) -> Vec<ConsumableParams> {
    let Some(Value::Dict(slots)) = ship.get("ShipAbilities").and_then(Value::attrs) else { return Vec::new() };
    let mut out = Vec::new();
    for (_, slot) in slots.iter() {
        let Some(Value::List(abils)) = slot.attrs().and_then(|s| s.get("abils")) else { continue };
        for pair in abils.iter() {
            let (Value::Tuple(t) | Value::List(t)) = pair else { continue };
            let (Some(name), Some(variant)) = (t.first().and_then(Value::as_str), t.get(1).and_then(Value::as_str))
            else {
                continue;
            };
            let Some(v) = abilities.get(name).and_then(|a| a.get(variant)).and_then(Value::attrs) else { continue };
            let kind = text(v.get("consumableType"));
            let logic = v.get("logic").and_then(Value::attrs);
            let field = |k| float(logic.and_then(|l| l.get(k)));
            let radius = match kind.as_str() {
                "fighter" | "callFighters" | "planeTacticalFighters" => field("radius"),
                // The hydrophone's wave radius is in metres.
                _ => field("hydrophoneWaveRadius").map(|m| m / 30.0).or_else(|| field("distShip")),
            };
            out.push(ConsumableParams { kind, work_time: float(v.get("workTime")).unwrap_or(0.0), radius });
        }
    }
    out
}

/// `ShipUpgradeInfo: {name: {ucType: "_Hull", components: {atba: [component]}}}`;
/// the component is a ship attribute of mounts, each with an `ammoList`.
fn secondary_ammo(ship: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(Value::Dict(upgrades)) = ship.get("ShipUpgradeInfo").and_then(Value::attrs) else { return out };
    for (_, u) in upgrades.iter() {
        let Some(u) = u.attrs() else { continue };
        if u.get("ucType").and_then(Value::as_str) != Some("_Hull") {
            continue;
        }
        let atba = u.get("components").and_then(Value::attrs).and_then(|c| c.get("atba"));
        let Some(Value::List(atba) | Value::Tuple(atba)) = atba else { continue };
        let Some(Value::Dict(mounts)) =
            atba.first().and_then(Value::as_str).and_then(|c| ship.get(c)).and_then(Value::attrs)
        else {
            continue;
        };
        for (_, m) in mounts.iter() {
            let Some(Value::List(ammo) | Value::Tuple(ammo)) = m.attrs().and_then(|m| m.get("ammoList")) else {
                continue;
            };
            for name in ammo.iter().filter_map(Value::as_str) {
                if !out.iter().any(|n| n == name) {
                    out.push(name.to_owned());
                }
            }
        }
    }
    out
}

impl GameParams {
    /// From the raw `content/GameParams.data` bytes.
    pub fn from_data(data: &[u8]) -> Result<Self, GameParamsError> {
        let reversed: Vec<u8> = data.iter().rev().copied().collect();
        let mut pickled = Vec::new();
        flate2::read::ZlibDecoder::new(&reversed[..]).read_to_end(&mut pickled)?;
        Self::from_pickle(&pickled)
    }

    /// From the inflated pickle.
    pub fn from_pickle(pickled: &[u8]) -> Result<Self, GameParamsError> {
        let root = pickle::from_slice(pickled)?;
        let base = root.get("").ok_or(GameParamsError::Layout("no base region \"\""))?;
        let Value::Dict(entries) = base else { return Err(GameParamsError::Layout("base region is not a dict")) };
        let mut out = GameParams::default();
        let abilities: HashMap<&str, &Value> =
            entries.iter().filter_map(|(k, v)| Some((k.as_str()?, v.attrs()?))).collect();
        for (_, entry) in entries.iter() {
            let Some(attrs) = entry.attrs() else { continue };
            let Some(id) = attrs.get("id").and_then(Value::as_i64) else { continue };
            let info = attrs.get("typeinfo").and_then(Value::attrs);
            let (kind, species) = (text(info.and_then(|i| i.get("type"))), text(info.and_then(|i| i.get("species"))));
            let (index, name) = (text(attrs.get("index")), text(attrs.get("name")));
            if let Some(active) = attrs.get("markerNameActive") {
                let inactive = text(attrs.get("markerNameInactive"));
                out.drops.insert(id, (text(Some(active)), inactive));
            }
            match kind.as_str() {
                "Ship" => {
                    let nation = text(info.and_then(|i| i.get("nation")));
                    let level = attrs.get("level").and_then(Value::as_i64).unwrap_or(0);
                    let consumables = ship_consumables(attrs, &abilities);
                    let secondary_ammo = secondary_ammo(attrs);
                    out.ships.insert(
                        id,
                        ShipParams { id, index, name, species, nation, level, consumables, secondary_ammo },
                    );
                }
                "Projectile" => {
                    let (max_dist, speed) = (float(attrs.get("maxDist")), float(attrs.get("speed")));
                    let (ammo_type, caliber) = (text(attrs.get("ammoType")), float(attrs.get("bulletDiametr")));
                    out.projectiles
                        .insert(id, ProjectileParams { id, index, name, species, max_dist, speed, ammo_type, caliber });
                }
                "Building" => {
                    out.buildings.insert(id, species);
                }
                "Aircraft" => {
                    let subtypes = match attrs.get("planeSubtype") {
                        Some(Value::List(l) | Value::Tuple(l)) => {
                            l.iter().filter_map(Value::as_str).map(str::to_owned).collect()
                        }
                        _ => Vec::new(),
                    };
                    // Resolved once all projectiles are in.
                    let ammo_type = text(attrs.get("bombName"));
                    out.aircraft.insert(id, AircraftParams { id, index, name, species, subtypes, ammo_type });
                }
                _ => {}
            }
        }
        let ammo: HashMap<&str, &str> =
            out.projectiles.values().map(|p| (p.name.as_str(), p.ammo_type.as_str())).collect();
        for a in out.aircraft.values_mut() {
            a.ammo_type = ammo.get(a.ammo_type.as_str()).copied().unwrap_or_default().to_owned();
        }
        if out.ships.is_empty() {
            return Err(GameParamsError::Layout("no ships"));
        }
        Ok(out)
    }

    pub fn load(res: &crate::res::Resources) -> Result<Self, GameParamsError> {
        Self::from_data(&res.read("content/GameParams.data")?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-entry table as the client pickles it (protocol 2, GPData
    /// objects built from a state dict, typeinfo as a nested object).
    fn sample() -> Vec<u8> {
        let mut p = b"\x80\x02}(U\x00}(".to_vec();
        let entry = |p: &mut Vec<u8>, name: &str, fields: &[(&str, &[u8])], info: &[(&str, &str)]| {
            p.push(b'U');
            p.push(name.len() as u8);
            p.extend(name.as_bytes());
            p.extend(b"cGameParams\nGPData\n)\x81}(");
            for (k, v) in fields {
                p.push(b'U');
                p.push(k.len() as u8);
                p.extend(k.as_bytes());
                p.extend(*v);
            }
            p.extend(b"U\x08typeinfocGameParams\nTypeInfo\n)\x81}(");
            for (k, v) in info {
                for s in [k, v] {
                    p.push(b'U');
                    p.push(s.len() as u8);
                    p.extend(s.as_bytes());
                }
            }
            p.extend(b"ubub"); // typeinfo state, BUILD; entry state, BUILD
        };
        entry(
            &mut p,
            "PJSD207_Shiratsuyu",
            &[
                ("id", b"J\xd0\x1c\x0c\xf3"),
                ("index", b"U\x07PJSD207"),
                ("name", b"U\x12PJSD207_Shiratsuyu"),
                ("level", b"K\x07"),
            ],
            &[("type", "Ship"), ("species", "Destroyer"), ("nation", "Japan")],
        );
        entry(
            &mut p,
            "PBPT206_Furious_top",
            &[("id", b"J\xd0\x5c\x16\xf3"), ("index", b"U\x07PBPT206"), ("maxDist", b"KP"), ("speed", b"K#")],
            &[("type", "Projectile"), ("species", "Torpedo"), ("nation", "UK")],
        );
        p.extend(b"uu.");
        p
    }

    #[test]
    fn extracts_ships_and_torpedoes() {
        let gp = GameParams::from_pickle(&sample()).unwrap();
        let ship = gp.ships.values().next().unwrap();
        assert_eq!((ship.index.as_str(), ship.species.as_str(), ship.level), ("PJSD207", "Destroyer", 7));
        let t = gp.projectiles.values().next().unwrap();
        assert_eq!((t.species.as_str(), t.max_dist, t.speed), ("Torpedo", Some(80.0), Some(35.0)));
    }

    #[test]
    fn from_data_inflates_the_reversed_stream() {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &sample()).unwrap();
        let mut data = z.finish().unwrap();
        data.reverse();
        assert_eq!(GameParams::from_data(&data).unwrap().ships.len(), 1);
        assert!(GameParams::from_pickle(b"\x80\x02}.").is_err());
    }
}
