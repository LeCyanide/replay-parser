//! Battle world, the part the replay alone determines: ships (position,
//! heading, health, team, alive), deaths, team scores, capture points,
//! smoke screens and the battle clock.
//!
//! [`World::apply`] takes each packet together with the event the
//! [`EntityDecoder`] made of it. Where an entity's state matters, it is read
//! back from the decoder, which already holds every property with nested
//! updates applied, rather than re-interpreted from the event.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::entity::EntityDecoder;
use crate::entity::Event;
use crate::packet::Decoded;
use crate::packet::Packet;
use crate::packet::Vec3;
use crate::value::Value;

#[derive(Clone, Debug, Serialize)]
pub struct Ship {
    /// The Avatar entity id, which is the arena info's vehicle `id`.
    pub owner: i64,
    pub team: i64,
    pub health: f32,
    pub max_health: f32,
    pub alive: bool,
    pub position: Vec3,
    pub yaw: f32,
    /// Clock of the last position update.
    pub seen: f32,
    /// Between EntityCreate and EntityLeave: the recording client receives
    /// updates for it. Raw `visibilityFlags` alongside, meaning not decoded.
    pub in_aoi: bool,
    pub visibility_flags: i64,
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
}

#[derive(Clone, Debug, Serialize)]
pub struct Smoke {
    pub radius: f32,
    pub points: Vec<[f32; 3]>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct World {
    pub clock: f32,
    pub time_left: Option<i64>,
    pub battle_stage: Option<i64>,
    /// Indexed by team id.
    pub scores: Vec<i64>,
    pub winner: Option<i64>,
    pub ships: BTreeMap<u32, Ship>,
    pub caps: BTreeMap<u32, CapturePoint>,
    pub smokes: BTreeMap<u32, Smoke>,
    pub deaths: Vec<Death>,
}

fn i64_of(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64)
}

fn f32_of(v: Option<&Value>) -> Option<f32> {
    v.and_then(Value::as_f32)
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, dec: &EntityDecoder, packet: &Packet, event: Option<&Event>) {
        self.clock = self.clock.max(packet.clock);
        if let Ok(decoded) = packet.decode() {
            match decoded {
                Decoded::Position { entity_id, position, rotation, .. } => {
                    self.moved(entity_id, position, rotation.yaw, packet.clock)
                }
                // Relative orientations (parent_id != 0) are the camera's.
                Decoded::PlayerOrientation { entity_id, parent_id: 0, position, rotation } => {
                    self.moved(entity_id, position, rotation.yaw, packet.clock)
                }
                _ => {}
            }
        }
        let Some(event) = event else { return };
        match *event {
            Event::Create { entity_id, entity, position, rotation, .. } => {
                match entity {
                    "Vehicle" => {
                        let ship = self.ships.entry(entity_id).or_insert_with(|| Ship {
                            owner: 0,
                            team: -1,
                            health: 0.0,
                            max_health: 0.0,
                            alive: true,
                            position: Vec3 { x: 0.0, y: 0.0, z: 0.0 },
                            yaw: 0.0,
                            seen: packet.clock,
                            in_aoi: true,
                            visibility_flags: 0,
                        });
                        ship.in_aoi = true;
                        if let (Some(p), Some(r)) = (position, rotation) {
                            ship.position = p;
                            ship.yaw = r.yaw;
                            ship.seen = packet.clock;
                        }
                    }
                    "InteractiveZone" => {
                        if let Some(p) = position {
                            let radius = f32_of(dec.property(entity_id, "radius")).unwrap_or(0.0);
                            self.caps.insert(
                                entity_id,
                                CapturePoint {
                                    index: -1,
                                    position: p,
                                    radius,
                                    team: -1,
                                    progress: 0.0,
                                    invader_team: -1,
                                    both_inside: false,
                                },
                            );
                        }
                    }
                    _ => {}
                }
                self.refresh(dec, entity_id, entity);
            }
            Event::Property { entity_id, entity, .. } | Event::Nested { entity_id, entity, .. } => {
                self.refresh(dec, entity_id, entity)
            }
            Event::Leave { entity_id, entity } => match entity {
                "Vehicle" => {
                    if let Some(s) = self.ships.get_mut(&entity_id) {
                        s.in_aoi = false;
                    }
                }
                // shortcut: leaving the AoI and dissipating look the same here;
                // a smoke that re-enters is re-created by its EntityCreate.
                "SmokeScreen" => {
                    self.smokes.remove(&entity_id);
                }
                _ => {}
            },
            Event::Method { method: "receiveVehicleDeath", ref args, .. } => {
                let arg = |i: usize| args.get(i).and_then(|(_, v)| v.as_i64());
                if let (Some(victim), Some(killer)) = (arg(0), arg(1)) {
                    let (victim, killer) = (victim as u32, killer as u32);
                    self.deaths.push(Death { clock: packet.clock, victim, killer, reason: arg(2).unwrap_or(-1) });
                    if let Some(s) = self.ships.get_mut(&victim) {
                        s.alive = false;
                    }
                }
            }
            Event::Method { .. } => {}
        }
    }

    fn moved(&mut self, entity_id: u32, position: Vec3, yaw: f32, clock: f32) {
        if let Some(s) = self.ships.get_mut(&entity_id) {
            s.position = position;
            s.yaw = yaw;
            s.seen = clock;
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
            }
            "SmokeScreen" => {
                let radius = f32_of(prop("bcRadius")).or(f32_of(prop("radius"))).unwrap_or(0.0);
                let points = match prop("points") {
                    Some(Value::List(ps)) => {
                        ps.iter().filter_map(|p| if let Value::Vector3(v) = p { Some(*v) } else { None }).collect()
                    }
                    _ => Vec::new(),
                };
                self.smokes.insert(id, Smoke { radius, points });
            }
            _ => {}
        }
    }
}
