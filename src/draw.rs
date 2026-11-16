//! Draw commands: what one frame of the replay viewer shows, from a
//! [`World`]. Mirrors the map-layer commands the suite's `.wtreply` format
//! carries (`sidecar/src/format.rs`), with the same pixel space, colours and
//! rules as the renderer it replaces, so the two bakes can be diffed record
//! by record.
//!
//! Positions are minimap pixels on a [`MINIMAP_SIZE`] square, y down; radii
//! are pixels too.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::gameparams::AircraftParams;
use crate::gameparams::GameParams;
use crate::world::Player;
use crate::world::ShellHit;
use crate::world::Ship;
use crate::world::World;

/// Output pixel size, as the suite's renderer draws.
pub const MINIMAP_SIZE: u32 = 768;

pub const SELF_COLOR: [u8; 3] = [255, 255, 255];
pub const DIVISION_COLOR: [u8; 3] = [255, 215, 0];
pub const ALLY_COLOR: [u8; 3] = [76, 232, 170];
pub const ENEMY_COLOR: [u8; 3] = [254, 77, 42];
pub const NEUTRAL_COLOR: [u8; 3] = [255, 255, 255];
const SMOKE_COLOR: [u8; 3] = [200, 200, 200];
const SMOKE_ALPHA: f32 = 0.5;
const HP_BAR_BG_COLOR: [u8; 3] = [50, 50, 50];
const HP_BAR_BG_ALPHA: f32 = 0.7;
const UNDETECTED_OPACITY: f32 = 0.4;
const CAP_ALPHA: f32 = 0.15;
const PATROL_ALPHA: f32 = 0.12;
const CONSUMABLE_ALPHA: f32 = 0.15;
const CAMERA_LINE: i32 = 18;
/// A tracer's length as a share of the shell's path, at [`MAX_TRACER_CALIBER`]
/// and above; smaller guns draw shorter, down to [`MIN_TRACER_RATIO`] of it.
const TRACER_LEN: f32 = 0.12;
const MAX_TRACER_CALIBER: f32 = 0.46;
const MIN_TRACER_RATIO: f32 = 0.3;

/// A map's playable square, from its `space.settings`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapGeometry {
    /// Side of the square in world units.
    pub space_size: i32,
}

impl MapGeometry {
    /// `bounds` (chunk indices, as attributes or child elements) and
    /// `chunkSize` (default 100): the side is `(chunks − 4) × chunkSize`,
    /// the larger of the two axes.
    pub fn from_space_settings(xml: &str) -> Option<Self> {
        let bounds = &xml[xml.find("<bounds")?..];
        let value = |name: &str| -> Option<f64> {
            let attr = format!("{name}=\"");
            if let Some(i) = bounds.find(&attr) {
                let rest = &bounds[i + attr.len()..];
                return rest[..rest.find('"')?].trim().parse().ok();
            }
            let open = format!("<{name}>");
            let rest = &bounds[bounds.find(&open)? + open.len()..];
            rest[..rest.find('<')?].trim().parse().ok()
        };
        let chunk = xml
            .find("<chunkSize>")
            .and_then(|i| xml[i + 11..].split('<').next()?.trim().parse::<f64>().ok())
            .unwrap_or(100.0);
        let side = |lo, hi| Some(((value(hi)? - value(lo)? + 1.0 - 4.0) * chunk).round() as i32);
        Some(Self { space_size: side("minX", "maxX")?.max(side("minY", "maxY")?) })
    }

    /// World x, z to minimap pixels (north up).
    pub fn px(&self, x: f32, z: f32) -> [f32; 2] {
        let scale = f64::from(MINIMAP_SIZE) / f64::from(self.space_size);
        let half = f64::from(MINIMAP_SIZE) / 2.0;
        [(f64::from(x) * scale + half) as f32, (-f64::from(z) * scale + half) as f32]
    }

    /// A world distance in whole pixels, as the renderer truncates it.
    pub fn len(&self, d: f32) -> i32 {
        (d / self.space_size as f32 * MINIMAP_SIZE as f32) as i32
    }
}

/// What a player's ship is labelled with, resolved once per replay.
#[derive(Clone, Debug, Default)]
pub struct Label {
    pub player: String,
    pub ship: Option<String>,
    /// GameParams species: `Destroyer`, `Cruiser`, …
    pub species: Option<String>,
}

/// Labels for a replay: per arena-info player, plus species by ship params
/// id for ships with no player (bots spawned mid-battle).
#[derive(Clone, Debug, Default)]
pub struct Labels {
    pub players: Vec<Label>,
    pub species: HashMap<i64, String>,
}

/// Labels for [`World::players`], by index. `names` is the gettext
/// catalogue: ship names are `IDS_<ship index>`, and bots' names are keys too.
pub fn labels(players: &[Player], gp: Option<&GameParams>, names: &HashMap<String, String>) -> Labels {
    let species = gp.map(|gp| gp.ships.values().map(|s| (s.id, s.species.clone())).collect()).unwrap_or_default();
    let players = players
        .iter()
        .map(|p| {
            let ship = gp.and_then(|gp| gp.ships.get(&p.ship_params_id));
            let player = match names.get(&p.name) {
                Some(n) if p.name.starts_with("IDS_") => n.clone(),
                _ => p.name.clone(),
            };
            Label {
                player,
                ship: ship.and_then(|s| names.get(&format!("IDS_{}", s.index))).cloned(),
                species: ship.map(|s| s.species.clone()),
            }
        })
        .collect();
    Labels { players, species }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "t")]
pub enum Draw<'a> {
    CapturePoint {
        pos: [f32; 2],
        radius: i32,
        color: [u8; 3],
        alpha: f32,
        /// `A`, `B`, … or a flag for bases.
        label: String,
        progress: f32,
        invader_color: Option<[u8; 3]>,
    },
    /// A squall or storm.
    WeatherZone {
        pos: [f32; 2],
        radius: i32,
    },
    /// An arms-race buff waiting to be picked up.
    BuffZone {
        pos: [f32; 2],
        radius: i32,
        color: [u8; 3],
        alpha: f32,
        /// Icon name from the drop's GameParams, e.g. `damage_active`.
        marker_name: Option<&'a str>,
    },
    Building {
        pos: [f32; 2],
        color: [u8; 3],
        is_alive: bool,
        /// Icon by GameParams species: `airbase`, `air_defence`, `artillery`,
        /// `generator`, `radar`, `station`, `supply`, `tower`.
        icon: Option<&'static str>,
        /// `ally`, `enemy`, `neutral`, `dead`, or `suppressed_` and a side.
        relation: &'static str,
    },
    Torpedo {
        pos: [f32; 2],
        color: [u8; 3],
    },
    Smoke {
        pos: [f32; 2],
        radius: i32,
        color: [u8; 3],
        alpha: f32,
    },
    Ship {
        entity_id: u32,
        pos: [f32; 2],
        /// Screen angle: π/2 − world yaw (0 points east, π/2 north).
        yaw: f32,
        species: Option<&'a str>,
        /// None while undetected.
        color: Option<[u8; 3]>,
        detected: bool,
        opacity: f32,
        is_self: bool,
        player_name: Option<&'a str>,
        /// With the submarine depth appended (" (Scope)", " (30m)", …).
        ship_name: Option<Cow<'a, str>>,
        /// An ally (or the recording player) the enemy currently spots.
        is_detected_teammate: bool,
        /// The player's client has dropped.
        is_disconnected: bool,
        /// The selected weapon (main guns: by the loaded shell), detected
        /// ships only.
        name_color: Option<[u8; 3]>,
    },
    /// Where a ship's guns point: a line from the ship.
    CameraDirection {
        entity_id: u32,
        pos: [f32; 2],
        /// Screen angle, as [`Draw::Ship`]'s.
        yaw: f32,
        color: [u8; 3],
        /// Pixels.
        length: i32,
    },
    HealthBar {
        entity_id: u32,
        pos: [f32; 2],
        fraction: f32,
        fill_color: [u8; 3],
        background_color: [u8; 3],
        background_alpha: f32,
    },
    DeadShip {
        entity_id: u32,
        pos: [f32; 2],
        yaw: f32,
        species: Option<&'a str>,
        is_self: bool,
        player_name: Option<&'a str>,
        ship_name: Option<&'a str>,
    },
    /// A shell's streak, tail to head; `secondary` for the secondary battery.
    ShotTracer {
        from: [f32; 2],
        to: [f32; 2],
        color: [u8; 3],
        secondary: bool,
    },
    /// The head of a [`Draw::ShotTracer`], coloured by ammo type.
    ShotTracerTip {
        at: [f32; 2],
        color: [u8; 3],
        secondary: bool,
    },
    /// An active consumable's area (radar, hydroacoustic search, …) around
    /// its ship.
    ConsumableRadius {
        entity_id: u32,
        pos: [f32; 2],
        radius: i32,
        color: [u8; 3],
        alpha: f32,
    },
    /// A fighter patrol's circle.
    PatrolRadius {
        plane_id: u64,
        pos: [f32; 2],
        radius: i32,
        color: [u8; 3],
        alpha: f32,
    },
    Plane {
        plane_id: u64,
        owner_entity_id: u32,
        pos: [f32; 2],
        /// `<dir>/<icon>_<ally|enemy>`, e.g. `controllable/fighter_he_enemy`.
        icon_key: String,
        /// Not for consumable planes (catapult fighters, spotters).
        player_name: Option<&'a str>,
        ship_name: Option<&'a str>,
    },
}

/// Learned shell flight times: per shooter, (fired at, mean seconds to its
/// hits) for each salvo that hit something, by fire time.
pub type Flights = HashMap<u32, Vec<(f32, f32)>>;

/// From [`World::shell_hits`] after a full pass over the replay: the old
/// renderer paces tracers by these rather than by `serverTimeLeft`.
pub fn flights(hits: &[ShellHit]) -> Flights {
    let mut salvos: HashMap<(u32, u32), (f32, u32)> = HashMap::new();
    for h in hits {
        let e = salvos.entry((h.owner, h.launched.to_bits())).or_default();
        e.0 += h.flight;
        e.1 += 1;
    }
    let mut out = Flights::new();
    for ((owner, at), (sum, n)) in salvos {
        out.entry(owner).or_default().push((f32::from_bits(at), sum / n as f32));
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    out
}

/// The same shooter's salvo nearest in fire time, else `fallback`.
fn flight(learned: Option<&Vec<(f32, f32)>>, fired: f32, fallback: f32) -> f32 {
    let mut best: Option<(f32, f32)> = None;
    for &(at, f) in learned.into_iter().flatten() {
        let dt = (at - fired).abs();
        if best.is_none_or(|b| dt < b.0) {
            best = Some((dt, f));
        }
    }
    best.map_or(fallback, |b| b.1)
}

/// Tracer tip by `ammoType`.
/// A ship name's colour by the selected weapon, as the old renderer's.
fn name_color(ship: &Ship, gp: Option<&GameParams>) -> Option<[u8; 3]> {
    match ship.weapon? {
        0 => Some(ammo_color(&gp?.projectiles.get(&ship.ammo?)?.ammo_type)),
        1 => Some(ammo_color("HE")),
        2 => Some([100, 255, 160]),
        3 => Some([200, 160, 255]),
        6 => Some([100, 220, 255]),
        _ => None,
    }
}

fn ammo_color(ammo: &str) -> [u8; 3] {
    match ammo {
        "HE" => [255, 180, 80],
        "CS" => [255, 100, 100],
        _ => [140, 200, 255],
    }
}

/// `seaMine` → `sea_mine`, `HE` → `he`, `Scout` → `scout`.
fn snake(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &ch) in c.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| c[j]);
        let next = c.get(i + 1);
        let boundary = ch.is_uppercase()
            && prev.is_some_and(|p| {
                p.is_lowercase() || p.is_ascii_digit() || (p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
            });
        if boundary && !out.ends_with('_') {
            out.push('_');
        }
        out.extend(if ch == '-' || ch == ' ' { '_' } else { ch }.to_lowercase());
    }
    out
}

/// A squadron's icon (`<dir>/<base>`, without the ally/enemy suffix) and
/// whether it is a consumable plane, as the old renderer picks them.
fn plane_icon(a: Option<&AircraftParams>) -> (String, bool) {
    let species = a.map(|a| a.species.as_str()).filter(|s| !s.is_empty());
    let ammo = a.map(|a| snake(&a.ammo_type)).unwrap_or_default();
    let ammo = match ammo.as_str() {
        "sea_mine" => "mine",
        "depthcharge" => "depth_charge",
        other => other,
    };
    let has = |k: &str| a.is_some_and(|a| a.subtypes.iter().any(|s| s == k));
    let dir = match (a, species) {
        (None, _) => "consumables",
        (_, Some("Airship" | "Auxiliary")) => "controllable",
        _ if has("airsupport") => "airsupport",
        _ if has("consumable") || ammo.is_empty() => "consumables",
        _ => "controllable",
    };
    let consumable = dir == "consumables";
    let with = |base: &str| if ammo.is_empty() { base.to_owned() } else { format!("{base}_{ammo}") };
    let base = match (species, consumable) {
        (None, _) => "fighter".to_owned(),
        (Some("Dive"), _) => with("bomber"),
        (Some(sp), true) => snake(sp),
        (Some("Fighter"), false) => with("fighter"),
        (Some("Bomber"), false) if ammo == "torpedo_deepwater" => ammo.to_owned(),
        (Some("Bomber"), false) => "torpedo_regular".to_owned(),
        (Some("Skip"), false) => with("skip"),
        (Some("Airship" | "Auxiliary"), false) => "auxiliary".to_owned(),
        (Some(_), false) => with("fighter"),
    };
    (format!("{dir}/{base}"), consumable)
}

/// A consumable circle's colour by type, friendly or not.
fn consumable_color(kind: &str, friendly: bool) -> [u8; 3] {
    match (kind, friendly) {
        ("rls", true) => [40, 80, 200],
        ("rls", false) => [180, 40, 50],
        ("sonar", true) => [40, 180, 170],
        ("sonar", false) => [200, 90, 30],
        ("hydrophone", true) => [70, 110, 180],
        ("hydrophone", false) => [170, 70, 50],
        ("submarineLocator", true) => [60, 60, 190],
        ("submarineLocator", false) => [160, 30, 60],
        (_, true) => ALLY_COLOR,
        (_, false) => ENEMY_COLOR,
    }
}

fn building_icon(species: &str) -> Option<&'static str> {
    Some(match species {
        "AirBase" => "airbase",
        "AntiAircraft" => "air_defence",
        "CoastalArtillery" | "Complex" => "artillery",
        "Generator" => "generator",
        "SensorTower" => "radar",
        "SpaceStation" => "station",
        "Military" => "supply",
        "RayTower" => "tower",
        _ => return None,
    })
}

fn relation_color(relation: u32) -> [u8; 3] {
    match relation {
        0 => SELF_COLOR,
        1 => ALLY_COLOR,
        _ => ENEMY_COLOR,
    }
}

fn team_color(team: i64, own: Option<i64>) -> [u8; 3] {
    match (team, own) {
        (t, _) if t < 0 => NEUTRAL_COLOR,
        (t, Some(o)) if t == o => ALLY_COLOR,
        (_, Some(_)) => ENEMY_COLOR,
        (0, None) => ALLY_COLOR,
        _ => ENEMY_COLOR,
    }
}

fn hp_color(fraction: f32) -> [u8; 3] {
    if fraction > 0.66 {
        [0, 255, 0]
    } else if fraction > 0.33 {
        [255, 255, 0]
    } else {
        [255, 0, 0]
    }
}

/// Depth suffix the renderer appends to a submarine's name.
fn depth(buoyancy: i64) -> Option<&'static str> {
    Some(match buoyancy {
        1 => " (Scope)",
        2 => " (30m)",
        3 => " (60m)",
        4 => " (60m*)",
        _ => return None,
    })
}

/// One frame at `clock` (torpedoes are dead-reckoned to it). `labels` is
/// [`labels`] for this world's players; `flights` paces shell tracers (see
/// [`flights`]; empty falls back to `serverTimeLeft`).
///
/// Not yet, against the renderer: division mates' gold colour, the
/// disconnected flag, name colours by loaded ammo, submarine depth in the
/// ship name, position interpolation between packets.
pub fn frame<'a>(
    world: &World,
    map: &MapGeometry,
    labels: &'a Labels,
    gp: Option<&'a GameParams>,
    flights: &Flights,
    clock: f32,
) -> Vec<Draw<'a>> {
    let mut out = Vec::new();
    let own = world.own_team();
    let relation = |id: u32| world.player_of(id).map(|p| world.players[p].relation);
    let label = |id: u32| world.player_of(id).and_then(|p| labels.players.get(p));
    let species = |id: u32| {
        label(id).and_then(|l| l.species.as_deref()).or_else(|| {
            let params = world.ships.get(&id)?.params_id;
            labels.species.get(&params).map(String::as_str)
        })
    };
    let screen = |yaw: f32| std::f32::consts::FRAC_PI_2 - yaw;
    // Division mates are gold, outside clan battles.
    let ship_color = |id: u32, r: u32| match world.player_of(id) {
        Some(p) if r != 0 && world.division_mate(p) => DIVISION_COLOR,
        _ => relation_color(r),
    };

    // Zones without a control point (arms-race buffs, …) are not caps.
    for c in world.caps.values().filter(|c| c.enabled && c.index >= 0) {
        let label = match c.kind {
            2 | 3 | 5 => "\u{2691}".to_owned(),
            _ => char::from(b'A' + c.index.clamp(0, 25) as u8).to_string(),
        };
        out.push(Draw::CapturePoint {
            pos: map.px(c.position.x, c.position.z),
            radius: map.len(c.radius).max(5),
            color: team_color(c.team, own),
            alpha: CAP_ALPHA,
            label,
            progress: c.progress,
            invader_color: (c.has_invaders && c.invader_team >= 0).then(|| team_color(c.invader_team, own)),
        });
    }

    // Arms-race buffs, behind everything else but the caps.
    for (id, c) in world.caps.iter().filter(|(_, c)| c.enabled && c.index < 0) {
        let drop = world.drops.get(id).and_then(|d| gp?.drops.get(d));
        out.push(Draw::BuffZone {
            pos: map.px(c.position.x, c.position.z),
            radius: map.len(c.radius).max(5),
            color: team_color(c.team, own),
            alpha: CAP_ALPHA,
            marker_name: drop.map(|(active, inactive)| if c.team >= 0 { active } else { inactive }.as_str()),
        });
    }

    for s in &world.shells {
        let Some(r) = relation(s.owner) else { continue };
        let fallback = if s.server_time_left > 0.0 { s.server_time_left } else { 3.0 };
        let duration = flight(flights.get(&s.owner), s.launched, fallback);
        let elapsed = clock - s.launched;
        if elapsed < 0.0 || elapsed > duration {
            continue;
        }
        let shell = gp.and_then(|gp| gp.projectiles.get(&s.params_id));
        let len = shell
            .and_then(|p| p.caliber)
            .map_or(TRACER_LEN, |d| TRACER_LEN * (d / MAX_TRACER_CALIBER).clamp(MIN_TRACER_RATIO, 1.0));
        // Secondaries come down the same path; only the ammo tells them apart.
        let secondary = shell.is_some_and(|p| {
            // A shooter not in view yet has its ship from the roster.
            let ship = world.ships.get(&s.owner).map(|o| o.params_id);
            let ship = ship.or_else(|| world.player_of(s.owner).map(|i| world.players[i].ship_params_id));
            ship.and_then(|id| gp?.ships.get(&id)).is_some_and(|o| o.secondary_ammo.contains(&p.name))
        });
        let at = |f: f32| map.px(s.from[0] + (s.to[0] - s.from[0]) * f, s.from[2] + (s.to[2] - s.from[2]) * f);
        let frac = elapsed / duration;
        let head = at(frac);
        out.push(Draw::ShotTracer {
            from: at((frac - len).max(0.0)),
            to: head,
            color: ship_color(s.owner, r),
            secondary,
        });
        if let Some(p) = shell {
            out.push(Draw::ShotTracerTip { at: head, color: ammo_color(&p.ammo_type), secondary });
        }
    }

    let half = map.space_size as f32 / 2.0;
    for t in &world.torpedoes {
        let [x, _, z] = t.position_at(clock);
        // Unknown owners are skipped rather than coloured by a guess.
        let Some(r) = relation(t.owner) else { continue };
        if x.abs() <= half && z.abs() <= half {
            out.push(Draw::Torpedo { pos: map.px(x, z), color: ship_color(t.owner, r) });
        }
    }

    for s in world.smokes.values() {
        for p in &s.points {
            let radius = map.len(s.radius).max(3);
            out.push(Draw::Smoke { pos: map.px(p[0], p[2]), radius, color: SMOKE_COLOR, alpha: SMOKE_ALPHA });
        }
    }

    for w in &world.weather {
        out.push(Draw::WeatherZone { pos: map.px(w.position[0], w.position[1]), radius: map.len(w.radius).max(5) });
    }

    for b in world.buildings.values().filter(|b| !b.hidden) {
        let side = match (b.team, own) {
            (t, _) if t < 0 => "neutral",
            (t, Some(o)) if t == o => "ally",
            _ => "enemy",
        };
        let relation = match (b.alive, b.suppressed, side) {
            (false, ..) => "dead",
            (_, true, "neutral") => "suppressed_neutral",
            (_, true, "ally") => "suppressed_ally",
            (_, true, _) => "suppressed_enemy",
            (_, false, side) => side,
        };
        out.push(Draw::Building {
            pos: map.px(b.position[0], b.position[2]),
            color: if b.alive { team_color(b.team, own) } else { [40, 40, 40] },
            is_alive: b.alive,
            icon: gp.and_then(|gp| gp.buildings.get(&b.params_id)).and_then(|s| building_icon(s)),
            relation,
        });
    }

    let dead: HashSet<u32> = world.deaths.iter().map(|d| d.victim).collect();
    let disconnected = |id: u32| world.player_of(id).is_some_and(|p| world.players[p].connection == Some(false));
    let ids: std::collections::BTreeSet<u32> = world.ships.keys().chain(world.minimap.keys()).copied().collect();
    for &id in ids.iter().filter(|id| !dead.contains(id)) {
        let ship = world.ships.get(&id);
        let sighting = world.minimap.get(&id);
        let rel = relation(id).unwrap_or(2);
        let l = label(id);
        let species = species(id);
        let yaw = screen(sighting.map(|m| m.yaw).or(ship.map(|s| s.yaw)).unwrap_or(0.0));
        if sighting.is_some_and(|m| m.visible) {
            // Outside the area of interest the world position is stale.
            let pos = match (ship, sighting) {
                (Some(s), Some(m)) if !(s.in_aoi && s.located) => map.px(m.position[0], m.position[1]),
                (Some(s), _) => map.px(s.position.x, s.position.z),
                (None, Some(m)) => map.px(m.position[0], m.position[1]),
                (None, None) => unreachable!(),
            };
            out.push(Draw::Ship {
                entity_id: id,
                pos,
                yaw,
                species,
                color: Some(ship_color(id, rel)),
                detected: true,
                opacity: 1.0,
                is_self: rel == 0,
                player_name: l.map(|l| l.player.as_str()),
                ship_name: l.and_then(|l| l.ship.as_deref()).map(|n| match ship.and_then(|s| depth(s.buoyancy)) {
                    Some(d) => Cow::Owned(format!("{n}{d}")),
                    None => Cow::Borrowed(n),
                }),
                is_detected_teammate: rel != 2 && ship.is_some_and(|s| s.spotted()),
                is_disconnected: disconnected(id),
                name_color: ship.and_then(|s| name_color(s, gp)),
            });
            // A rostered ship whose entity has not arrived yet: the old
            // renderer seeds it full from the arena roster's `maxHealth`.
            let fraction = match ship {
                Some(s) if s.max_health > 0.0 => Some((s.health / s.max_health).clamp(0.0, 1.0)),
                Some(_) => None,
                None => world.player_of(id).filter(|&p| world.players[p].max_health > 0).map(|_| 1.0),
            };
            if let Some(fraction) = fraction {
                out.push(Draw::HealthBar {
                    entity_id: id,
                    pos,
                    fraction,
                    fill_color: hp_color(fraction),
                    background_color: HP_BAR_BG_COLOR,
                    background_alpha: HP_BAR_BG_ALPHA,
                });
            }
        } else if let Some(m) = sighting {
            out.push(Draw::Ship {
                entity_id: id,
                pos: map.px(m.position[0], m.position[1]),
                yaw,
                species,
                color: None,
                detected: false,
                opacity: UNDETECTED_OPACITY,
                is_self: rel == 0,
                player_name: None,
                ship_name: None,
                is_detected_teammate: false,
                is_disconnected: disconnected(id),
                name_color: None,
            });
        }
    }

    // Where the guns point, for ships the minimap shows (aim is stale otherwise).
    for (&id, s) in world.ships.iter().filter(|(id, _)| !dead.contains(id)) {
        let (Some(aim), Some(m)) = (s.aim_yaw, world.minimap.get(&id).filter(|m| m.visible)) else { continue };
        out.push(Draw::CameraDirection {
            entity_id: id,
            pos: if s.in_aoi && s.located {
                map.px(s.position.x, s.position.z)
            } else {
                map.px(m.position[0], m.position[1])
            },
            yaw: screen(aim),
            color: ship_color(id, relation(id).unwrap_or(2)),
            length: CAMERA_LINE,
        });
    }

    for d in &world.deaths {
        let (ship, sighting) = (world.ships.get(&d.victim), world.minimap.get(&d.victim));
        let Some([x, z]) = d.position else { continue };
        let pos = map.px(x, z);
        let l = label(d.victim);
        out.push(Draw::DeadShip {
            entity_id: d.victim,
            pos,
            yaw: screen(sighting.map(|m| m.yaw).or(ship.map(|s| s.yaw)).unwrap_or(0.0)),
            species: species(d.victim),
            is_self: relation(d.victim) == Some(0),
            player_name: l.map(|l| l.player.as_str()),
            ship_name: l.and_then(|l| l.ship.as_deref()),
        });
    }

    for (&id, ship) in world.ships.iter().filter(|(id, _)| !dead.contains(id)) {
        // Only ships the minimap shows; the world position while in view.
        let Some(m) = world.minimap.get(&id).filter(|m| m.visible) else { continue };
        let pos = if ship.in_aoi && ship.located {
            map.px(ship.position.x, ship.position.z)
        } else {
            map.px(m.position[0], m.position[1])
        };
        let friendly = relation(id).unwrap_or(2) < 2;
        for &(kind, until) in &ship.consumables {
            let Some(name) = crate::gameparams::consumable_type(kind) else { continue };
            // Fighters' circles are their patrols, drawn with the planes.
            if until <= clock || name == "fighter" || name == "callFighters" {
                continue;
            }
            let Some(r) = world.consumable_radius(ship, kind) else { continue };
            out.push(Draw::ConsumableRadius {
                entity_id: id,
                pos,
                radius: map.len(r),
                color: consumable_color(name, friendly),
                alpha: CONSUMABLE_ALPHA,
            });
        }
    }

    for sq in world.squadrons.values() {
        let enemy = match relation(sq.owner) {
            Some(r) => r >= 2,
            None => own.is_some_and(|o| sq.team != o),
        };
        let (icon, consumable) = plane_icon(gp.and_then(|gp| gp.aircraft.get(&sq.params_id)));
        if let Some(w) = world.patrols.get(&sq.id) {
            out.push(Draw::PatrolRadius {
                plane_id: sq.id,
                pos: map.px(w.position[0], w.position[2]),
                radius: map.len(w.radius),
                color: if enemy { ENEMY_COLOR } else { ALLY_COLOR },
                alpha: PATROL_ALPHA,
            });
        }
        let l = label(sq.owner).filter(|_| !consumable);
        out.push(Draw::Plane {
            plane_id: sq.id,
            owner_entity_id: sq.owner,
            pos: map.px(sq.position[0], sq.position[1]),
            icon_key: format!("{icon}_{}", if enemy { "enemy" } else { "ally" }),
            player_name: l.map(|l| l.player.as_str()),
            ship_name: l.and_then(|l| l.ship.as_deref()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_space_settings_both_ways() {
        let attrs = r#"<space.settings><bounds minX="-10" maxX="9" minY="-10" maxY="9" /></space.settings>"#;
        assert_eq!(MapGeometry::from_space_settings(attrs), Some(MapGeometry { space_size: 1600 }));
        let elems = "<root><bounds><minX> -9 </minX><maxX>8</maxX><minY>-7</minY><maxY>6</maxY></bounds>\
                     <chunkSize>50</chunkSize></root>";
        assert_eq!(MapGeometry::from_space_settings(elems), Some(MapGeometry { space_size: 700 }));
        assert_eq!(MapGeometry::from_space_settings("<x/>"), None);
    }

    #[test]
    fn paces_tracers_by_the_nearest_learned_salvo() {
        let hits = [(7, 10.0, 4.0), (7, 10.0, 6.0), (7, 30.0, 9.0)].map(|(owner, launched, flight)| ShellHit {
            owner,
            launched,
            flight,
        });
        let f = flights(&hits);
        assert_eq!(f[&7], vec![(10.0, 5.0), (30.0, 9.0)]);
        assert_eq!(flight(f.get(&7), 18.0, 2.0), 5.0);
        assert_eq!(flight(f.get(&7), 22.0, 2.0), 9.0);
        assert_eq!(flight(f.get(&8), 22.0, 2.0), 2.0);
    }

    #[test]
    fn names_plane_icons_as_the_renderer() {
        let plane = |species: &str, subtypes: &[&str], ammo: &str| AircraftParams {
            id: 0,
            index: String::new(),
            name: String::new(),
            species: species.into(),
            subtypes: subtypes.iter().map(|s| s.to_string()).collect(),
            ammo_type: ammo.into(),
        };
        let icon = |a: AircraftParams| plane_icon(Some(&a));
        assert_eq!(icon(plane("Fighter", &[], "HE")), ("controllable/fighter_he".into(), false));
        assert_eq!(icon(plane("Bomber", &[], "torpedo")), ("controllable/torpedo_regular".into(), false));
        assert_eq!(icon(plane("Scout", &["consumable"], "")), ("consumables/scout".into(), true));
        assert_eq!(icon(plane("Fighter", &[], "")), ("consumables/fighter".into(), true));
        assert_eq!(icon(plane("Dive", &["airsupport"], "seaMine")), ("airsupport/bomber_mine".into(), false));
        assert_eq!(plane_icon(None), ("consumables/fighter".into(), true));
        assert_eq!(snake("HEBomb"), "he_bomb");
    }

    #[test]
    fn maps_world_to_pixels_north_up() {
        let m = MapGeometry { space_size: 1600 };
        assert_eq!(m.px(0.0, 0.0), [384.0, 384.0]);
        assert_eq!(m.px(800.0, 800.0), [768.0, 0.0]);
        assert_eq!(m.len(400.0), 192);
    }
}
