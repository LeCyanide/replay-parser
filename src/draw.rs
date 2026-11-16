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

use crate::gameparams::GameParams;
use crate::world::Player;
use crate::world::World;

/// Output pixel size, as the suite's renderer draws.
pub const MINIMAP_SIZE: u32 = 768;

pub const SELF_COLOR: [u8; 3] = [255, 255, 255];
pub const ALLY_COLOR: [u8; 3] = [76, 232, 170];
pub const ENEMY_COLOR: [u8; 3] = [254, 77, 42];
pub const NEUTRAL_COLOR: [u8; 3] = [255, 255, 255];
const SMOKE_COLOR: [u8; 3] = [200, 200, 200];
const SMOKE_ALPHA: f32 = 0.5;
const HP_BAR_BG_COLOR: [u8; 3] = [50, 50, 50];
const HP_BAR_BG_ALPHA: f32 = 0.7;
const UNDETECTED_OPACITY: f32 = 0.4;
const CAP_ALPHA: f32 = 0.15;

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
/// [`labels`] for this world's players.
///
/// Not yet, against the renderer: division mates' gold colour, the
/// disconnected flag, name colours by loaded ammo, submarine depth in the
/// ship name, position interpolation between packets.
pub fn frame<'a>(world: &World, map: &MapGeometry, labels: &'a Labels, clock: f32) -> Vec<Draw<'a>> {
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

    let half = map.space_size as f32 / 2.0;
    for t in &world.torpedoes {
        let [x, _, z] = t.position_at(clock);
        // Unknown owners are skipped rather than coloured by a guess.
        let Some(r) = relation(t.owner) else { continue };
        if x.abs() <= half && z.abs() <= half {
            out.push(Draw::Torpedo { pos: map.px(x, z), color: relation_color(r) });
        }
    }

    for s in world.smokes.values() {
        for p in &s.points {
            let radius = map.len(s.radius).max(3);
            out.push(Draw::Smoke { pos: map.px(p[0], p[2]), radius, color: SMOKE_COLOR, alpha: SMOKE_ALPHA });
        }
    }

    let dead: HashSet<u32> = world.deaths.iter().map(|d| d.victim).collect();
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
                (Some(s), Some(m)) if !s.in_aoi => map.px(m.position[0], m.position[1]),
                (Some(s), _) => map.px(s.position.x, s.position.z),
                (None, Some(m)) => map.px(m.position[0], m.position[1]),
                (None, None) => unreachable!(),
            };
            out.push(Draw::Ship {
                entity_id: id,
                pos,
                yaw,
                species,
                color: Some(relation_color(rel)),
                detected: true,
                opacity: 1.0,
                is_self: rel == 0,
                player_name: l.map(|l| l.player.as_str()),
                ship_name: l.and_then(|l| l.ship.as_deref()).map(|n| match ship.and_then(|s| depth(s.buoyancy)) {
                    Some(d) => Cow::Owned(format!("{n}{d}")),
                    None => Cow::Borrowed(n),
                }),
                is_detected_teammate: rel != 2 && ship.is_some_and(|s| s.spotted()),
            });
            // A rostered ship whose entity has not arrived yet: the old
            // renderer draws a full bar for allies, and for some enemies by a
            // rule not worked out (under 1.5% of its bars on the local replays).
            let fraction = match ship {
                Some(s) if s.max_health > 0.0 => Some((s.health / s.max_health).clamp(0.0, 1.0)),
                Some(_) => None,
                None => (l.is_some() && rel != 2).then_some(1.0),
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
            });
        }
    }

    for d in &world.deaths {
        let (ship, sighting) = (world.ships.get(&d.victim), world.minimap.get(&d.victim));
        let pos = match (ship, sighting) {
            (Some(s), _) => map.px(s.position.x, s.position.z),
            (None, Some(m)) => map.px(m.position[0], m.position[1]),
            (None, None) => continue,
        };
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
    fn maps_world_to_pixels_north_up() {
        let m = MapGeometry { space_size: 1600 };
        assert_eq!(m.px(0.0, 0.0), [384.0, 384.0]);
        assert_eq!(m.px(800.0, 800.0), [768.0, 0.0]);
        assert_eq!(m.len(400.0), 192);
    }
}
