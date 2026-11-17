//! The map layer: the draw commands for one frame painted over the map
//! image, in the suite's replay-layer style (`src/canvas/ReplayLayer.tsx`).

use replay_parser::draw::Draw;
use replay_parser::draw::MINIMAP_SIZE;
use replay_parser::world::World;
use tiny_skia::FilterQuality;
use tiny_skia::Pixmap;
use tiny_skia::PixmapPaint;
use tiny_skia::Transform;

use crate::paint::*;
use crate::text::Align;
use crate::text::Text;
use crate::text::Weight;

pub const ALLY: Rgb = [0x4c, 0xe8, 0xaa];
pub const ENEMY: Rgb = [0xfe, 0x4d, 0x2a];
const NEUTRAL_FILL: Rgb = [0x5f, 0x6d, 0x7c];
const SPOTTED_OUTLINE: Rgb = [0xb3, 0x9a, 0x5e];
const LABEL: Rgb = [0xe8, 0xee, 0xf6];
const SEA: Rgb = [0x10, 0x26, 0x38];
const GRID: Rgb = [0xff, 0xff, 0xff];

/// Screen sizes at a 768 px map; scaled with the map.
const SHIP_PX: f32 = 22.0;
const NAME_PX: f32 = 10.0;
const HEALTH_W_PX: f32 = 22.0;
const HEALTH_H_PX: f32 = 3.0;
const TRACER_PX: f32 = 1.1;
const TORPEDO_PX: f32 = 2.6;
const PLANE_PX: f32 = 6.0;

/// Battle seconds of track drawn behind each ship (about 1.2 s of video at ×32).
const TRAIL_SECONDS: f32 = 40.0;
/// A jump longer than this (minimap px at 768) breaks a trail: a respawn or a
/// ship re-entering sight elsewhere.
const TRAIL_BREAK_PX: f32 = 40.0;

struct Trail {
    color: Rgb,
    /// (battle clock, minimap px at 768), oldest first.
    points: std::collections::VecDeque<(f32, [f32; 2])>,
}

pub struct MapLayer {
    /// The background at output size: the map image, or a plain sea with a
    /// grid when there is none.
    pub background: Pixmap,
    pub size: u32,
    glyphs: Glyphs,
    trails: std::collections::HashMap<u32, Trail>,
}

impl MapLayer {
    pub fn new(size: u32, png: Option<&[u8]>) -> Self {
        let mut bg = Pixmap::new(size, size).expect("non-zero map size");
        bg.fill(tiny_skia::Color::from_rgba8(SEA[0], SEA[1], SEA[2], 255));
        let image = png.and_then(|b| Pixmap::decode_png(b).ok());
        match &image {
            Some(img) => {
                let k = size as f32 / img.width().max(img.height()) as f32;
                let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
                bg.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_scale(k, k), None);
            }
            None => {
                // A 10×10 grid, as the in-game minimap has.
                let s = size as f32;
                for i in 1..10 {
                    let p = s * i as f32 / 10.0;
                    line(&mut bg, [p, 0.0], [p, s], 1.0, GRID, 0.08);
                    line(&mut bg, [0.0, p], [s, p], 1.0, GRID, 0.08);
                }
            }
        }
        Self { background: bg, size, glyphs: Glyphs::default(), trails: std::collections::HashMap::new() }
    }

    /// Paints `cmds` onto `pm` (which already holds the background at
    /// `(0, 0)`). `world` resolves dead ships' sides.
    pub fn paint(&mut self, pm: &mut Pixmap, text: &mut Text, cmds: &[Draw<'_>], world: &World, clock: f32) {
        let k = self.size as f32 / MINIMAP_SIZE as f32;
        self.update_trails(cmds, clock);
        self.paint_trails(pm, k, clock);
        let p = |v: [f32; 2]| [v[0] * k, v[1] * k];
        let r = |v: i32| v as f32 * k;
        let ship_size = SHIP_PX * k;
        let name_px = NAME_PX * k;
        let own = world.own_team();
        let side_color = |id: u32| match (world.ships.get(&id).map(|s| s.team), own) {
            (Some(t), Some(o)) if t == o => ALLY,
            (Some(_), Some(_)) => ENEMY,
            _ => NEUTRAL_FILL,
        };
        let mut marks: Vec<Rect> = Vec::new();
        let mut labels: Vec<NameLabel> = Vec::new();
        // The recording player's ship in its team's colour, not the
        // renderer's white (the suite's rule).
        for cmd in cmds {
            match cmd {
                Draw::WeatherZone { pos, radius } => {
                    let [x, y] = p(*pos);
                    fill_circle(pm, x, y, r(*radius), [255; 3], 0.12);
                }
                Draw::BuffZone { pos, radius, color, alpha, .. } => {
                    let [x, y] = p(*pos);
                    fill_circle(pm, x, y, r(*radius), *color, *alpha);
                }
                Draw::CapturePoint { pos, radius, color, alpha, label, progress, invader_color } => {
                    let [x, y] = p(*pos);
                    let rad = r(*radius);
                    fill_circle(pm, x, y, rad, *color, alpha * 0.28);
                    stroke_circle(pm, x, y, rad, 1.4 * k, *color, 1.0);
                    if let Some(ic) = invader_color.filter(|_| *progress > 0.001) {
                        arc(pm, x, y, rad, *progress, 3.0 * k, ic, 1.0);
                    }
                    let px = name_px * 1.6;
                    text.draw(
                        pm,
                        label,
                        x,
                        y + text.cap_height(px) / 2.0,
                        px,
                        Weight::Bold,
                        Align::Center,
                        *color,
                        1.0,
                    );
                }
                Draw::Building { pos, color, is_alive, .. } => {
                    let [x, y] = p(*pos);
                    let s = 5.0 * k;
                    rect(pm, x - s / 2.0, y - s / 2.0, s, s, *color, if *is_alive { 0.9 } else { 0.35 });
                }
                Draw::Smoke { pos, radius, color, alpha } => {
                    let [x, y] = p(*pos);
                    fill_circle(pm, x, y, r(*radius), *color, *alpha);
                }
                Draw::ConsumableRadius { pos, radius, color, alpha, .. }
                | Draw::PatrolRadius { pos, radius, color, alpha, .. } => {
                    let [x, y] = p(*pos);
                    stroke_circle(pm, x, y, r(*radius), k, *color, (*alpha * 3.0).min(0.6));
                }
                Draw::ShotTracer { from, to, color, secondary } => {
                    let w = if *secondary { TRACER_PX * 0.7 } else { TRACER_PX } * k;
                    line(pm, p(*from), p(*to), w, *color, if *secondary { 0.35 } else { 0.85 });
                }
                Draw::ShotTracerTip { at, color, secondary } => {
                    let [x, y] = p(*at);
                    fill_circle(
                        pm,
                        x,
                        y,
                        if *secondary { 1.0 } else { 1.5 } * k,
                        *color,
                        if *secondary { 0.5 } else { 1.0 },
                    );
                }
                Draw::Torpedo { pos, color } => {
                    let [x, y] = p(*pos);
                    fill_circle(pm, x, y, TORPEDO_PX / 2.0 * k, *color, 1.0);
                }
                Draw::Plane { pos, .. } => {
                    let [x, y] = p(*pos);
                    let s = PLANE_PX / 2.0 * k;
                    let mut pb = tiny_skia::PathBuilder::new();
                    pb.move_to(x, y - s);
                    pb.line_to(x + s, y);
                    pb.line_to(x, y + s);
                    pb.line_to(x - s, y);
                    pb.close();
                    if let Some(path) = pb.finish() {
                        pm.fill_path(
                            &path,
                            &paint(hex(0xffd479), 0.9),
                            tiny_skia::FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                Draw::CameraDirection { pos, yaw, color, length, .. } => {
                    let [x, y] = p(*pos);
                    let len = r(*length);
                    line(pm, [x, y], [x + yaw.cos() * len, y - yaw.sin() * len], k, *color, 0.7);
                }
                Draw::HealthBar { pos, fraction, fill_color, background_color, background_alpha, .. } => {
                    let [x, y] = p(*pos);
                    let (w, h) = (HEALTH_W_PX * k, HEALTH_H_PX * k);
                    let (bx, by) = (x - w / 2.0, y - ship_size * 0.75);
                    rect(pm, bx, by, w, h, *background_color, *background_alpha);
                    rect(pm, bx, by, w * fraction.clamp(0.0, 1.0), h, *fill_color, 1.0);
                    marks.push(Rect { x0: bx, y0: by, x1: bx + w, y1: by + h });
                }
                Draw::DeadShip { entity_id, pos, yaw, species, is_self, .. } => {
                    let [x, y] = p(*pos);
                    let fill = if *is_self { ALLY } else { side_color(*entity_id) };
                    self.glyphs.draw(
                        pm,
                        &ShipMark {
                            x,
                            y,
                            yaw: *yaw,
                            class: Class::of(*species),
                            size: ship_size * 0.85,
                            fill,
                            alpha: 0.32,
                            outline: None,
                        },
                    );
                }
                Draw::Ship {
                    pos,
                    yaw,
                    species,
                    color,
                    detected,
                    opacity,
                    is_self,
                    player_name,
                    ship_name,
                    is_detected_teammate,
                    name_color,
                    ..
                } => {
                    let [x, y] = p(*pos);
                    let alpha = if *detected { *opacity } else { opacity * 0.55 };
                    let fill = if *is_self { ALLY } else { color.unwrap_or(NEUTRAL_FILL) };
                    let outline = (!is_self && *is_detected_teammate).then_some(SPOTTED_OUTLINE);
                    let size = ship_size * if *is_self { 1.08 } else { 1.0 };
                    if *is_self {
                        // A ring, so viewers find the recording player at once.
                        stroke_circle(pm, x, y, size * 0.78, 1.3 * k, [255; 3], 0.75 * alpha);
                    }
                    self.glyphs.draw(
                        pm,
                        &ShipMark { x, y, yaw: *yaw, class: Class::of(*species), size, fill, alpha, outline },
                    );
                    let label = ship_name.as_deref().or(*player_name).map(|n| n.trim_matches(':'));
                    marks.push(Rect::around(x, y, size * 0.6));
                    if let Some(l) = label {
                        labels.push(NameLabel {
                            text: l.to_owned(),
                            x,
                            y,
                            color: name_color.unwrap_or(LABEL),
                            alpha: (alpha + 0.25).min(1.0),
                            detected: *detected,
                            is_self: *is_self,
                        });
                    }
                }
            }
        }
        place_labels(pm, text, labels, &marks, ship_size, name_px);
    }
}

impl MapLayer {
    /// Extends the trails of ships in sight; a ship out of sight, dead, or
    /// jumping loses its trail.
    fn update_trails(&mut self, cmds: &[Draw<'_>], clock: f32) {
        let mut seen = std::collections::HashSet::new();
        for cmd in cmds {
            let Draw::Ship { entity_id, pos, color: Some(color), detected: true, .. } = cmd else { continue };
            seen.insert(*entity_id);
            let t =
                self.trails.entry(*entity_id).or_insert_with(|| Trail { color: *color, points: Default::default() });
            t.color = *color;
            if let Some(&(_, last)) = t.points.back()
                && (last[0] - pos[0]).hypot(last[1] - pos[1]) > TRAIL_BREAK_PX
            {
                t.points.clear();
            }
            t.points.push_back((clock, *pos));
            while t.points.front().is_some_and(|&(c, _)| c < clock - TRAIL_SECONDS) {
                t.points.pop_front();
            }
        }
        self.trails.retain(|id, _| seen.contains(id));
    }

    /// Each trail fades out towards its tail.
    fn paint_trails(&self, pm: &mut Pixmap, k: f32, clock: f32) {
        for t in self.trails.values() {
            for (a, b) in t.points.iter().zip(t.points.iter().skip(1)) {
                let age = (clock - b.0) / TRAIL_SECONDS;
                let alpha = 0.55 * (1.0 - age).clamp(0.0, 1.0);
                line(pm, [a.1[0] * k, a.1[1] * k], [b.1[0] * k, b.1[1] * k], 1.4 * k, t.color, alpha);
            }
        }
    }

    /// "Victory" / "Defeat" / "Draw" and the final score across the map, for
    /// the held last frame.
    pub fn result_banner(&self, pm: &mut Pixmap, text: &mut Text, result: Option<bool>, scores: (i64, i64)) {
        let s = self.size as f32;
        let (word, color) = match result {
            Some(true) => ("Victory", ALLY),
            Some(false) => ("Defeat", ENEMY),
            None => ("Draw", LABEL),
        };
        let h = s * 0.2;
        let y0 = (s - h) / 2.0;
        rect(pm, 0.0, y0, s, h, [0x0d, 0x11, 0x17], 0.82);
        rect(pm, 0.0, y0, s, 2.0, color, 0.9);
        rect(pm, 0.0, y0 + h - 2.0, s, 2.0, color, 0.9);
        let big = h * 0.36;
        text.draw(pm, word, s / 2.0, y0 + h * 0.5, big, Weight::Bold, Align::Center, color, 1.0);
        let score = format!("{}  :  {}", scores.0, scores.1);
        text.draw(pm, &score, s / 2.0, y0 + h * 0.8, big * 0.45, Weight::Bold, Align::Center, LABEL, 0.9);
    }
}

/// An axis-aligned box, for keeping names apart.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn around(x: f32, y: f32, half: f32) -> Self {
        Self { x0: x - half, y0: y - half, x1: x + half, y1: y + half }
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

struct NameLabel {
    text: String,
    x: f32,
    y: f32,
    color: Rgb,
    alpha: f32,
    detected: bool,
    is_self: bool,
}

/// Draws ship names after every mark, each at the first free spot of: below
/// the ship, above its health bar, right, left. Names that fit nowhere are
/// dropped for ships out of sight and drawn faded below for ships in sight.
/// The recording player's ship is placed first, then ships in sight.
fn place_labels(pm: &mut Pixmap, text: &mut Text, mut labels: Vec<NameLabel>, marks: &[Rect], ship: f32, px: f32) {
    labels.sort_by_key(|l| (!l.is_self, !l.detected));
    let cap = text.cap_height(px);
    let pad = px * 0.15;
    let mut placed: Vec<Rect> = Vec::new();
    for l in labels {
        let w = text.width(&l.text, px, Weight::Bold);
        // Candidate baselines and left edges.
        let below = (l.x - w / 2.0, l.y + ship * 0.72 + cap);
        let candidates = [
            below,
            (l.x - w / 2.0, l.y - ship * 0.95),
            // Clear of the ship's own mark (half-size 0.6 × ship).
            (l.x + ship * 0.62 + pad, l.y + cap / 2.0),
            (l.x - ship * 0.62 - pad - w, l.y + cap / 2.0),
        ];
        let rect = |(x, b): (f32, f32)| Rect { x0: x - pad, y0: b - cap - pad, x1: x + w + pad, y1: b + pad * 2.0 };
        let free = candidates.iter().copied().find(|&c| {
            let r = rect(c);
            !placed.iter().any(|p| p.overlaps(&r)) && !marks.iter().any(|m| m.overlaps(&r))
        });
        let (at, alpha) = match free {
            Some(c) => (c, l.alpha),
            None if l.detected || l.is_self => (below, l.alpha * 0.45),
            None => continue,
        };
        placed.push(rect(at));
        // A dark halo keeps names readable over bright map areas.
        text.draw(pm, &l.text, at.0 + 0.6, at.1 + 0.6, px, Weight::Bold, Align::Left, [0; 3], 0.6 * alpha);
        text.draw(pm, &l.text, at.0, at.1, px, Weight::Bold, Align::Left, l.color, alpha);
    }
}
