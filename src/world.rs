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
use std::collections::HashSet;

use serde::Serialize;

use crate::ReplayMeta;
use crate::entity::EntityDecoder;
use crate::entity::Event;
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
/// do (about 1 in 25 on the fixtures).
/// shortcut: dropped at this age, about twice the longest run seen; use the
/// torpedo's range from GameParams once step 7 lands.
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
    /// updates for it. Raw `visibilityFlags` alongside, meaning not decoded.
    pub in_aoi: bool,
    pub visibility_flags: i64,
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
}

impl Torpedo {
    pub fn position_at(&self, clock: f32) -> [f32; 3] {
        let dt = clock - self.at;
        std::array::from_fn(|i| self.position[i] + self.velocity[i] * dt)
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
    pub torpedoes: Vec<Torpedo>,
    pub shells: Vec<Shell>,
    pub deaths: Vec<Death>,
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

    pub fn apply(&mut self, dec: &EntityDecoder, packet: &Packet, event: Option<&Event>) {
        // The stream's final trailer packet carries clock 0; never go back.
        self.clock = self.clock.max(packet.clock);
        let clock = self.clock;
        self.shells.retain(|s| s.arrives > clock);
        self.torpedoes.retain(|t| clock - t.launched < TORPEDO_MAX_AGE);
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
            Event::Method { method, args, .. } => self.method(method, args, clock),
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
            "onArenaStateReceived" => {
                for (name, v) in args {
                    let ("playersStates" | "botsStates", Value::Blob(b)) = (*name, v) else { continue };
                    // An unreadable roster leaves ships unlinked, not the world broken.
                    let Ok(pickle::Value::List(entries)) = pickle::from_slice(b) else { continue };
                    for entry in &entries {
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
