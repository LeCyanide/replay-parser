//! Battle world, the part the replay alone determines: ships (position,
//! heading, health, team, alive, player), deaths, team scores, capture
//! points, smoke screens, squadrons on the minimap, torpedoes, shells in
//! flight, and the battle clock.
//!
//! [`World::apply`] takes each packet together with the event the
//! [`EntityDecoder`] made of it. Where an entity's state matters, it is read
//! back from the decoder, which already holds every property with nested
//! updates applied, rather than re-interpreted from the event.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use serde::Serialize;

use crate::ReplayMeta;
use crate::entity::EntityDecoder;
use crate::entity::Event;
use crate::gameparams::GameParams;
use crate::packet::Decoded;
use crate::packet::Packet;
use crate::packet::Vec3;
use crate::pickle;
use crate::value::Value;

/// A shell's real flight time over its `serverTimeLeft`. Measured on the
/// fixtures from hits that land at the aim point (5th–95th percentile
/// 0.349–0.374); `tests/world.rs` re-measures it.
pub const SHELL_TIME_SCALE: f32 = 0.36;

/// Torpedoes that run out of range normally get a kill message; a few never
/// do (about 1 in 25 on the fixtures). Those are dropped past their range
/// times this (upgrades add up to a quarter; fitted on local 15.10 replays),
/// or at `TORPEDO_MAX_AGE` when the range is unknown.
pub const TORPEDO_RANGE_SLACK: f32 = 1.3;
/// About twice the longest run seen.
pub const TORPEDO_MAX_AGE: f32 = 150.0;

#[derive(Clone, Debug, Serialize)]
pub struct Ship {
    /// The owning Avatar's entity id; 0 for bots and, in some builds, for
    /// ships first created outside the area of interest.
    pub owner: i64,
    /// Index into [`World::players`], once the roster links it.
    pub player: Option<usize>,
    pub team: i64,
    pub health: f32,
    pub max_health: f32,
    pub alive: bool,
    pub position: Vec3,
    pub yaw: f32,
    /// Clock of the last position update.
    pub seen: f32,
    /// Between EntityCreate and EntityLeave: the recording client receives
    /// updates for it.
    pub in_aoi: bool,
    /// Non-zero while the opposing team spots this ship; the bits are the
    /// reasons, not yet named. See [`Ship::spotted`].
    pub visibility_flags: i64,
    /// Active consumables: (consumable type, clock it ends). Types are the
    /// game's `CONSUMABLE_ID`s; names need GameParams-era tables.
    pub consumables: Vec<(i64, f32)>,
}

impl Ship {
    /// Spotted by the opposing team. On the fixtures, enemies of the
    /// recording player carry this about 98% of the time they are in the area
    /// of interest and almost never outside it; allies carry it while the
    /// enemy sees them.
    pub fn spotted(&self) -> bool {
        self.visibility_flags != 0
    }

    pub fn consumable_active(&self, kind: i64, clock: f32) -> bool {
        self.consumables.iter().any(|&(k, until)| k == kind && until > clock)
    }
}

/// A fighter patrol ("ward") with its own circle on the minimap.
#[derive(Clone, Debug, Serialize)]
pub struct Patrol {
    pub id: u64,
    pub owner: u32,
    pub team: i64,
    pub kind: i64,
    pub position: [f32; 3],
    pub radius: f32,
    /// The add message's `time`: 0 or 60 on the fixtures, and not when the
    /// patrol ends (some outlive it); `receive_wardRemoved` ends it.
    pub time: f32,
    pub added: f32,
}

/// One arena-info vehicle entry.
#[derive(Clone, Debug, Serialize)]
pub struct Player {
    pub name: String,
    /// GameParams id of the ship.
    pub ship_params_id: i64,
    /// 0 = the recording player, 1 = ally, 2 = enemy.
    pub relation: u32,
    /// The arena info's `id`; see [`crate::meta::VehicleMeta::id`].
    pub meta_id: i64,
    /// Every integer in this player's `onArenaStateReceived` roster entry.
    /// The entry's keys are version-specific indices (players and bots use
    /// different ones), so a ship is linked by finding its Vehicle entity id
    /// among these instead. Bots' ships have `owner` 0, so that cannot link.
    #[serde(skip)]
    roster: HashSet<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Death {
    pub clock: f32,
    pub victim: u32,
    pub killer: u32,
    /// The game's death-reason code; names need GameParams-era tables.
    pub reason: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapturePoint {
    /// `controlPoint.index`: 0 is "A".
    pub index: i64,
    pub position: Vec3,
    pub radius: f32,
    /// Owning team, -1 when neutral.
    pub team: i64,
    pub progress: f32,
    pub invader_team: i64,
    pub both_inside: bool,
    pub has_invaders: bool,
    /// `captureLogic.isEnabled`; disabled zones are not drawn.
    pub enabled: bool,
    /// `controlPoint.type`: 0 control, 1 base, 2 mega base, 3 building,
    /// 4 base with points, 5 epicenter.
    pub kind: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Smoke {
    pub radius: f32,
    pub points: Vec<[f32; 3]>,
}

/// A squadron as the minimap shows it.
#[derive(Clone, Debug, Serialize)]
pub struct Squadron {
    pub id: u64,
    /// The launching ship: the plane id's low 32 bits.
    pub owner: u32,
    pub team: i64,
    pub params_id: i64,
    /// World x, z.
    pub position: [f32; 2],
    pub seen: f32,
}

/// A ship's position as the minimap shows it, from `updateMinimapVisionInfo`.
/// Ships outside the area of interest can be here: spotted, but minimap only.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sighting {
    /// World x, z, quantized to 5000 / 2047 ≈ 2.44 units.
    pub position: [f32; 2],
    /// Quantized to 2π / 256.
    pub yaw: f32,
    /// Clock of the last update with a position.
    pub at: f32,
    /// False once an update says the position is gone; `position` and `yaw`
    /// are then the last known ones.
    pub visible: bool,
}

/// Unpacks `MINIMAP_USER_INFO.packedData`: bits 0–10 x, 11–21 z (each
/// 0..2047 over −2500..2500 world units, whatever the map size), 22–29 yaw
/// (0..255 over −π..π); bit 31 set means no position (x and z then 0).
/// Bit 30 was never seen set. Checked against `Position` packets in
/// `tests/world.rs`.
pub fn unpack_minimap(packed: u32) -> Option<([f32; 2], f32)> {
    if packed >> 31 != 0 {
        return None;
    }
    let coord = |v: u32| (v as f32 / 2047.0 - 0.5) * 5000.0;
    let yaw = ((packed >> 22) & 0xff) as f32 / 256.0 * std::f32::consts::TAU - std::f32::consts::PI;
    Some(([coord(packed & 0x7ff), coord((packed >> 11) & 0x7ff)], yaw))
}

#[derive(Clone, Debug, Serialize)]
pub struct Torpedo {
    pub owner: u32,
    pub shot_id: i64,
    pub params_id: i64,
    pub launched: f32,
    /// Known position at clock `at`, moving by `velocity` per second since.
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub at: f32,
    /// Base range in world units, from GameParams when the world has them.
    pub range: Option<f32>,
}

impl Torpedo {
    pub fn position_at(&self, clock: f32) -> [f32; 3] {
        let dt = clock - self.at;
        std::array::from_fn(|i| self.position[i] + self.velocity[i] * dt)
    }

    /// Past its range, or too old when the range is unknown. Course changes
    /// keep the speed, so distance run is speed times age.
    fn spent(&self, clock: f32) -> bool {
        let age = clock - self.launched;
        match self.range {
            Some(r) => self.velocity.iter().map(|v| v * v).sum::<f32>().sqrt() * age > r * TORPEDO_RANGE_SLACK,
            None => age >= TORPEDO_MAX_AGE,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Shell {
    pub owner: u32,
    pub shot_id: i64,
    pub params_id: i64,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub launched: f32,
    /// Estimated landing clock; a hit before then removes the shell early.
    pub arrives: f32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct World {
    pub clock: f32,
    pub time_left: Option<i64>,
    pub battle_stage: Option<i64>,
    /// Indexed by team id.
    pub scores: Vec<i64>,
    pub winner: Option<i64>,
    pub players: Vec<Player>,
    pub ships: BTreeMap<u32, Ship>,
    pub caps: BTreeMap<u32, CapturePoint>,
    pub smokes: BTreeMap<u32, Smoke>,
    pub squadrons: BTreeMap<u64, Squadron>,
    /// By Vehicle entity id, which may not be in `ships` yet. Kept, marked
    /// not visible, when an update says the position is gone.
    pub minimap: BTreeMap<u32, Sighting>,
    pub patrols: BTreeMap<u64, Patrol>,
    pub torpedoes: Vec<Torpedo>,
    pub shells: Vec<Shell>,
    pub deaths: Vec<Death>,
    /// Torpedo range by projectile params id; see `with_params`.
    #[serde(skip)]
    pub torpedo_ranges: HashMap<i64, f32>,
    /// Consumable radii by (ship params id, consumable type name).
    #[serde(skip)]
    pub consumable_radii: HashMap<(i64, String), f32>,
}

fn i64_of(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64)
}

fn f32_of(v: Option<&Value>) -> Option<f32> {
    v.and_then(Value::as_f32)
}

fn vec3(v: Option<&Value>) -> Option<[f32; 3]> {
    match v {
        Some(Value::Vector3(p)) => Some(*p),
        _ => None,
    }
}

fn list<'v, 'a>(v: Option<&'v Value<'a>>) -> &'v [Value<'a>] {
    match v {
        Some(Value::List(xs)) => xs,
        _ => &[],
    }
}

fn pickle_ints(v: &pickle::Value, out: &mut HashSet<i64>) {
    use pickle::Value as P;
    match v {
        P::Int(i) => {
            out.insert(*i);
        }
        P::List(xs) | P::Tuple(xs) | P::Set(xs) => xs.iter().for_each(|x| pickle_ints(x, out)),
        P::Dict(kv) => kv.iter().for_each(|(k, x)| {
            pickle_ints(k, out);
            pickle_ints(x, out)
        }),
        P::Object { args, state, .. } => {
            pickle_ints(args, out);
            if let Some(s) = state {
                pickle_ints(s, out);
            }
        }
        _ => {}
    }
}

impl World {
    pub fn new(meta: &ReplayMeta) -> Self {
        let players = meta
            .vehicles
            .iter()
            .map(|v| Player {
                name: v.name.clone(),
                ship_params_id: v.ship_id,
                relation: v.relation,
                meta_id: v.id,
                roster: HashSet::new(),
            })
            .collect();
        Self { players, ..Self::default() }
    }

    /// Uses GameParams for torpedo ranges.
    pub fn with_params(mut self, gp: &GameParams) -> Self {
        self.torpedo_ranges = gp.projectiles.values().filter_map(|p| Some((p.id, p.max_dist?))).collect();
        for ship in gp.ships.values() {
            for c in &ship.consumables {
                let Some(r) = c.radius else { continue };
                // Alternatives of one type in different slots: keep the larger.
                let e = self.consumable_radii.entry((ship.id, c.kind.clone())).or_insert(r);
                *e = e.max(r);
            }
        }
        self
    }

    /// The area of a consumable type (`Ship::consumables` ids) on a linked
    /// ship, in world units, base value; needs `with_params`.
    pub fn consumable_radius(&self, ship: &Ship, kind: i64) -> Option<f32> {
        let params = self.players.get(ship.player?)?.ship_params_id;
        let name = crate::gameparams::consumable_type(kind)?;
        self.consumable_radii.get(&(params, name.to_owned())).copied()
    }

    /// The arena-info player of a Vehicle entity, whether or not its ship
    /// has been created (minimap-only ships are not).
    pub fn player_of(&self, id: u32) -> Option<usize> {
        self.ships
            .get(&id)
            .and_then(|s| s.player)
            .or_else(|| self.players.iter().position(|p| p.roster.contains(&i64::from(id))))
    }

    /// The recording player's team, once its ship is linked.
    pub fn own_team(&self) -> Option<i64> {
        let me = self.players.iter().position(|p| p.relation == 0)?;
        self.ships.values().find(|s| s.player == Some(me)).map(|s| s.team)
    }

    pub fn apply(&mut self, dec: &EntityDecoder, packet: &Packet, event: Option<&Event>) {
        // The stream's final trailer packet carries clock 0; never go back.
        self.clock = self.clock.max(packet.clock);
        let clock = self.clock;
        self.shells.retain(|s| s.arrives > clock);
        self.torpedoes.retain(|t| !t.spent(clock));
        if let Ok(decoded) = packet.decode() {
            match decoded {
                Decoded::Position { entity_id, position, rotation, .. } => {
                    self.moved(entity_id, position, rotation.yaw, clock)
                }
                // Relative orientations (parent_id != 0) are the camera's.
                Decoded::PlayerOrientation { entity_id, parent_id: 0, position, rotation } => {
                    self.moved(entity_id, position, rotation.yaw, clock)
                }
                _ => {}
            }
        }
        let Some(event) = event else { return };
        match event {
            Event::Create { entity_id, entity, position, rotation, .. } => {
                let entity_id = *entity_id;
                match *entity {
                    "Vehicle" => {
                        let ship = self.ships.entry(entity_id).or_insert_with(|| Ship {
                            owner: 0,
                            player: None,
                            team: -1,
                            health: 0.0,
                            max_health: 0.0,
                            alive: true,
                            position: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
                            yaw: 0.0,
                            seen: clock,
                            in_aoi: true,
                            visibility_flags: 0,
                            consumables: Vec::new(),
                        });
                        ship.in_aoi = true;
                        if let (Some(p), Some(r)) = (position, rotation) {
                            ship.position = *p;
                            ship.yaw = r.yaw;
                            ship.seen = clock;
                        }
                    }
                    "InteractiveZone" => {
                        if let Some(p) = position {
                            let radius = f32_of(dec.property(entity_id, "radius")).unwrap_or(0.0);
                            self.caps.insert(
                                entity_id,
                                CapturePoint {
                                    index: -1,
                                    position: *p,
                                    radius,
                                    team: -1,
                                    progress: 0.0,
                                    invader_team: -1,
                                    both_inside: false,
                                    has_invaders: false,
                                    enabled: true,
                                    kind: 0,
                                },
                            );
                        }
                    }
                    _ => {}
                }
                self.refresh(dec, entity_id, entity);
            }
            Event::Property { entity_id, entity, .. } | Event::Nested { entity_id, entity, .. } => {
                self.refresh(dec, *entity_id, entity)
            }
            Event::Leave { entity_id, entity } => match *entity {
                "Vehicle" => {
                    if let Some(s) = self.ships.get_mut(entity_id) {
                        s.in_aoi = false;
                    }
                }
                // shortcut: leaving the AoI and dissipating look the same here;
                // a smoke that re-enters is re-created by its EntityCreate.
                "SmokeScreen" => {
                    self.smokes.remove(entity_id);
                }
                _ => {}
            },
            Event::Method { entity: "Vehicle", entity_id, method, args } => {
                self.vehicle_method(*entity_id, method, args, clock)
            }
            Event::Method { method, args, .. } => self.method(method, args, clock),
        }
    }

    fn vehicle_method(&mut self, id: u32, method: &str, args: &[(&str, Value)], clock: f32) {
        let Some(ship) = self.ships.get_mut(&id) else { return };
        let arg = |i: usize| args.get(i).map(|(_, v)| v);
        match method {
            "onConsumableUsed" => {
                // Up to 15.1 a CONSUMABLE_ID; from 15.4 a 2-byte USER_TYPE whose
                // second byte matches the old id's work times type for type.
                // shortcut: its first byte was always 1 and is ignored.
                let kind = match arg(0) {
                    Some(Value::Blob([_, kind])) => Some(i64::from(*kind)),
                    v => i64_of(v),
                };
                if let (Some(kind), Some(left)) = (kind, f32_of(arg(1))) {
                    ship.consumables.retain(|&(k, until)| k != kind && until > clock);
                    ship.consumables.push((kind, clock + left));
                }
            }
            "onConsumableInterrupted" => {
                if let Some(kind) = i64_of(arg(0)) {
                    ship.consumables.retain(|&(k, _)| k != kind);
                }
            }
            _ => {}
        }
    }

    fn method(&mut self, method: &str, args: &[(&str, Value)], clock: f32) {
        let arg = |i: usize| args.get(i).map(|(_, v)| v);
        let int = |i: usize| i64_of(arg(i));
        match method {
            "receiveVehicleDeath" => {
                if let (Some(victim), Some(killer)) = (int(0), int(1)) {
                    let (victim, killer) = (victim as u32, killer as u32);
                    self.deaths.push(Death { clock, victim, killer, reason: int(2).unwrap_or(-1) });
                    if let Some(s) = self.ships.get_mut(&victim) {
                        s.alive = false;
                    }
                }
            }
            // Rosters: the arena's at the start, then ships that spawn later
            // (Operations waves). shortcut: spawned bots missing from the arena
            // info get no Player, so their ships stay unnamed.
            "onArenaStateReceived" | "onNewPlayerSpawnedInBattle" => {
                for (name, v) in args {
                    let ("playersStates" | "botsStates" | "playersData" | "botsData", Value::Blob(b)) = (*name, v)
                    else {
                        continue;
                    };
                    // An unreadable roster leaves ships unlinked, not the world broken.
                    let Ok(pickle::Value::List(entries)) = pickle::from_slice(b) else { continue };
                    for entry in entries.iter() {
                        let mut ints = HashSet::new();
                        pickle_ints(entry, &mut ints);
                        if let Some(p) = self
                            .players
                            .iter_mut()
                            .find(|p| ints.contains(&p.meta_id) && ints.contains(&p.ship_params_id))
                        {
                            p.roster = ints;
                        }
                    }
                }
                let ids: Vec<u32> = self.ships.keys().copied().collect();
                ids.into_iter().for_each(|id| self.link(id));
            }
            // The second list was empty on every fixture; meaning unknown.
            "updateMinimapVisionInfo" => {
                for item in list(arg(0)) {
                    let (Some(id), Some(packed)) = (i64_of(item.get("vehicleID")), i64_of(item.get("packedData")))
                    else {
                        continue;
                    };
                    match unpack_minimap(packed as u32) {
                        Some((position, yaw)) => {
                            self.minimap.insert(id as u32, Sighting { position, yaw, at: clock, visible: true });
                        }
                        None => {
                            if let Some(m) = self.minimap.get_mut(&(id as u32)) {
                                m.visible = false;
                            }
                        }
                    }
                }
            }
            "receive_wardAdded" => {
                let (Some(id), Some(position), Some(time), Some(radius)) =
                    (int(0), vec3(arg(1)), f32_of(arg(2)), f32_of(arg(3)))
                else {
                    return;
                };
                let id = id as u64;
                let patrol = Patrol {
                    id,
                    owner: int(5).unwrap_or(0) as u32,
                    team: int(4).unwrap_or(-1),
                    kind: int(6).unwrap_or(-1),
                    position,
                    radius,
                    time,
                    added: clock,
                };
                self.patrols.insert(id, patrol);
            }
            "receive_wardRemoved" => {
                if let Some(id) = int(0) {
                    self.patrols.remove(&(id as u64));
                }
            }
            "receive_addMinimapSquadron" => {
                let (Some(id), Some(Value::Vector2(p))) = (int(0), arg(3)) else { return };
                let id = id as u64;
                self.squadrons.insert(
                    id,
                    Squadron {
                        id,
                        owner: id as u32,
                        team: int(1).unwrap_or(-1),
                        params_id: int(2).unwrap_or(0),
                        position: *p,
                        seen: clock,
                    },
                );
            }
            "receive_updateMinimapSquadron" => {
                let (Some(id), Some(Value::Vector2(p))) = (int(0), arg(1)) else { return };
                if let Some(s) = self.squadrons.get_mut(&(id as u64)) {
                    s.position = *p;
                    s.seen = clock;
                }
            }
            "receive_removeMinimapSquadron" => {
                if let Some(id) = int(0) {
                    self.squadrons.remove(&(id as u64));
                }
            }
            "receiveTorpedoes" => {
                for pack in list(arg(0)) {
                    let owner = i64_of(pack.get("ownerID")).unwrap_or(0) as u32;
                    let params_id = i64_of(pack.get("paramsID")).unwrap_or(0);
                    for t in list(pack.get("torpedoes")) {
                        let (Some(position), Some(velocity), Some(shot_id)) =
                            (vec3(t.get("pos")), vec3(t.get("dir")), i64_of(t.get("shotID")))
                        else {
                            continue;
                        };
                        self.torpedoes.push(Torpedo {
                            owner,
                            shot_id,
                            params_id,
                            launched: clock,
                            position,
                            velocity,
                            at: clock,
                            range: self.torpedo_ranges.get(&params_id).copied(),
                        });
                    }
                }
            }
            // A course change: new position and yaw, same speed.
            "receiveTorpedoDirection" | "receiveTorpedoManeuverEnd" => {
                let (Some(owner), Some(shot_id), Some(pos), Some(yaw)) = (int(0), int(1), vec3(arg(2)), f32_of(arg(3)))
                else {
                    return;
                };
                if let Some(t) = self.torpedo(owner as u32, shot_id) {
                    let speed = t.velocity[0].hypot(t.velocity[2]);
                    t.position = pos;
                    t.velocity = [speed * yaw.sin(), 0.0, speed * yaw.cos()];
                    t.at = clock;
                }
            }
            "receiveTorpedoSynchronization" => {
                let (Some(owner), Some(shot_id), Some(pos)) = (int(0), int(1), vec3(arg(2))) else { return };
                if let Some(t) = self.torpedo(owner as u32, shot_id) {
                    t.position = pos;
                    t.at = clock;
                }
            }
            "receiveArtilleryShots" => {
                for pack in list(arg(0)) {
                    let owner = i64_of(pack.get("ownerID")).unwrap_or(0) as u32;
                    let params_id = i64_of(pack.get("paramsID")).unwrap_or(0);
                    for s in list(pack.get("shots")) {
                        let (Some(from), Some(to), Some(shot_id), Some(left)) = (
                            vec3(s.get("pos")),
                            vec3(s.get("tarPos")),
                            i64_of(s.get("shotID")),
                            f32_of(s.get("serverTimeLeft")),
                        ) else {
                            continue;
                        };
                        let arrives = clock + left * SHELL_TIME_SCALE;
                        self.shells.push(Shell { owner, shot_id, params_id, from, to, launched: clock, arrives });
                    }
                }
            }
            "receiveShotKills" => {
                for pack in list(arg(0)) {
                    let owner = i64_of(pack.get("ownerID")).unwrap_or(0) as u32;
                    let ids: HashSet<i64> =
                        list(pack.get("kills")).iter().filter_map(|k| i64_of(k.get("shotID"))).collect();
                    self.shells.retain(|s| !(s.owner == owner && ids.contains(&s.shot_id)));
                    self.torpedoes.retain(|t| !(t.owner == owner && ids.contains(&t.shot_id)));
                }
            }
            _ => {}
        }
    }

    fn torpedo(&mut self, owner: u32, shot_id: i64) -> Option<&mut Torpedo> {
        self.torpedoes.iter_mut().find(|t| t.owner == owner && t.shot_id == shot_id)
    }

    fn moved(&mut self, entity_id: u32, position: Vec3, yaw: f32, clock: f32) {
        if let Some(s) = self.ships.get_mut(&entity_id) {
            s.position = position;
            s.yaw = yaw;
            s.seen = clock;
        }
    }

    /// Links a ship to its player once both the ship and the roster are known.
    fn link(&mut self, id: u32) {
        let Some(s) = self.ships.get_mut(&id) else { return };
        if s.player.is_none() {
            s.player = self.players.iter().position(|p| p.roster.contains(&i64::from(id)));
        }
    }

    /// Re-reads the state this world tracks for one entity from the decoder.
    fn refresh(&mut self, dec: &EntityDecoder, id: u32, entity: &str) {
        let prop = |name| dec.property(id, name);
        match entity {
            "Vehicle" => {
                let Some(s) = self.ships.get_mut(&id) else { return };
                s.owner = i64_of(prop("owner")).unwrap_or(s.owner);
                s.team = i64_of(prop("teamId")).unwrap_or(s.team);
                s.health = f32_of(prop("health")).unwrap_or(s.health);
                s.max_health = f32_of(prop("maxHealth")).unwrap_or(s.max_health);
                s.visibility_flags = i64_of(prop("visibilityFlags")).unwrap_or(s.visibility_flags);
                // A death is final even if a later isAlive arrives out of order.
                if i64_of(prop("isAlive")) == Some(0) {
                    s.alive = false;
                }
                self.link(id);
            }
            "BattleLogic" => {
                self.time_left = i64_of(prop("timeLeft")).or(self.time_left);
                self.battle_stage = i64_of(prop("battleStage")).or(self.battle_stage);
                if let Some(w) = i64_of(prop("battleResult").and_then(|r| r.get("winnerTeamId"))) {
                    // -2 is "no result yet".
                    self.winner = (w != -2).then_some(w);
                }
                let teams = prop("state").and_then(|s| s.get("missions")).and_then(|m| m.get("teamsScore"));
                if let Some(Value::List(teams)) = teams {
                    self.scores = teams.iter().map(|t| i64_of(t.get("score")).unwrap_or(0)).collect();
                }
            }
            "InteractiveZone" => {
                let Some(c) = self.caps.get_mut(&id) else { return };
                c.team = i64_of(prop("teamId")).unwrap_or(c.team);
                let comp = prop("componentsState");
                let cp = comp.and_then(|s| s.get("controlPoint"));
                c.index = i64_of(cp.and_then(|p| p.get("index"))).unwrap_or(c.index);
                let logic = comp.and_then(|s| s.get("captureLogic"));
                c.progress = f32_of(logic.and_then(|l| l.get("progress"))).unwrap_or(c.progress);
                c.invader_team = i64_of(logic.and_then(|l| l.get("invaderTeam"))).unwrap_or(c.invader_team);
                c.both_inside = i64_of(logic.and_then(|l| l.get("bothInside"))).unwrap_or(0) != 0;
                c.has_invaders = i64_of(logic.and_then(|l| l.get("hasInvaders"))).unwrap_or(0) != 0;
                c.enabled = i64_of(logic.and_then(|l| l.get("isEnabled"))).is_none_or(|e| e != 0);
                c.kind = i64_of(cp.and_then(|p| p.get("type"))).unwrap_or(c.kind);
            }
            "SmokeScreen" => {
                let radius = f32_of(prop("bcRadius")).or(f32_of(prop("radius"))).unwrap_or(0.0);
                let points = list(prop("points")).iter().filter_map(|p| vec3(Some(p))).collect();
                self.smokes.insert(id, Smoke { radius, points });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unpack_minimap;

    #[test]
    fn unpacks_minimap_positions() {
        // From the Gold Harbor fixture; the Position packet at the same
        // clock put this ship at (-473.1, 304.8), yaw 2.36.
        let ([x, z], yaw) = unpack_minimap(937681726).unwrap();
        assert!((x - -473.1).abs() < 2.5 && (z - 304.8).abs() < 2.5, "{x} {z}");
        assert!((yaw - 2.36).abs() < 0.03, "{yaw}");
        assert_eq!(unpack_minimap(1 << 31), None);
    }
}
