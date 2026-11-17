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

pub struct MapLayer {
    /// The background at output size: the map image, or a plain sea with a
    /// grid when there is none.
    pub background: Pixmap,
    pub size: u32,
    glyphs: Glyphs,
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
        Self { background: bg, size, glyphs: Glyphs::default() }
    }

    /// Paints `cmds` onto `pm` (which already holds the background at
    /// `(0, 0)`). `world` resolves dead ships' sides.
    pub fn paint(&self, pm: &mut Pixmap, text: &mut Text, cmds: &[Draw<'_>], world: &World) {
        let k = self.size as f32 / MINIMAP_SIZE as f32;
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
                    self.glyphs.draw(
                        pm,
                        &ShipMark { x, y, yaw: *yaw, class: Class::of(*species), size, fill, alpha, outline },
                    );
                    let label = ship_name.as_deref().or(*player_name).map(|n| n.trim_matches(':'));
                    if let Some(l) = label {
                        let ty = y + ship_size * 0.72 + text.cap_height(name_px);
                        let c = name_color.unwrap_or(LABEL);
                        // A dark halo keeps names readable over bright map areas.
                        text.draw(pm, l, x + 0.6, ty + 0.6, name_px, Weight::Bold, Align::Center, [0; 3], 0.6 * alpha);
                        text.draw(pm, l, x, ty, name_px, Weight::Bold, Align::Center, c, (alpha + 0.25).min(1.0));
                    }
                }
            }
        }
    }
}
