//! Small drawing helpers over tiny-skia, and the ship class glyphs.

#![allow(clippy::too_many_arguments)]

use tiny_skia::FillRule;
use tiny_skia::LineCap;
use tiny_skia::Paint;
use tiny_skia::Path;
use tiny_skia::PathBuilder;
use tiny_skia::Pixmap;
use tiny_skia::Rect;
use tiny_skia::Stroke;
use tiny_skia::Transform;

pub type Rgb = [u8; 3];

pub fn paint(c: Rgb, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    p
}

pub fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

pub fn fill_circle(pm: &mut Pixmap, x: f32, y: f32, r: f32, c: Rgb, alpha: f32) {
    if let Some(p) = PathBuilder::from_circle(x, y, r.max(0.1)) {
        pm.fill_path(&p, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_circle(pm: &mut Pixmap, x: f32, y: f32, r: f32, width: f32, c: Rgb, alpha: f32) {
    if let Some(p) = PathBuilder::from_circle(x, y, r.max(0.1)) {
        stroke(pm, &p, width, c, alpha);
    }
}

pub fn stroke(pm: &mut Pixmap, p: &Path, width: f32, c: Rgb, alpha: f32) {
    let s = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
    pm.stroke_path(p, &paint(c, alpha), &s, Transform::identity(), None);
}

pub fn line(pm: &mut Pixmap, a: [f32; 2], b: [f32; 2], width: f32, c: Rgb, alpha: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(a[0], a[1]);
    pb.line_to(b[0], b[1]);
    if let Some(p) = pb.finish() {
        stroke(pm, &p, width, c, alpha);
    }
}

/// An arc clockwise on screen from 12 o'clock, `frac` of a turn.
pub fn arc(pm: &mut Pixmap, x: f32, y: f32, r: f32, frac: f32, width: f32, c: Rgb, alpha: f32) {
    let steps = ((64.0 * frac).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU * i as f32 / steps as f32;
        let (px, py) = (x + r * a.cos(), y + r * a.sin());
        if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
    }
    if let Some(p) = pb.finish() {
        stroke(pm, &p, width, c, alpha);
    }
}

/// `img` scaled to `size` px on its longer side, centred on `(x, y)`.
pub fn image(pm: &mut Pixmap, img: &Pixmap, x: f32, y: f32, size: f32, alpha: f32) {
    let k = size / img.width().max(img.height()) as f32;
    let (w, h) = (img.width() as f32 * k, img.height() as f32 * k);
    let paint = tiny_skia::PixmapPaint {
        opacity: alpha.clamp(0.0, 1.0),
        quality: tiny_skia::FilterQuality::Bicubic,
        ..Default::default()
    };
    let t = Transform::from_scale(k, k).post_translate(x - w / 2.0, y - h / 2.0);
    pm.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
}

pub fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Rgb, alpha: f32) {
    if let Some(r) = Rect::from_xywh(x, y, w.max(0.0), h.max(0.0)) {
        pm.fill_rect(r, &paint(c, alpha), Transform::identity(), None);
    }
}

pub fn round_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgb, alpha: f32) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// Ship classes, as the suite draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Dd,
    Ca,
    Bb,
    Cv,
    Ss,
}

impl Class {
    /// GameParams species; unknown species read as a cruiser, the
    /// middle-of-the-road silhouette (the suite's rule).
    pub fn of(species: Option<&str>) -> Class {
        match species {
            Some("Destroyer") => Class::Dd,
            Some("Battleship") => Class::Bb,
            Some("AirCarrier" | "Carrier") => Class::Cv,
            Some("Submarine") => Class::Ss,
            _ => Class::Ca,
        }
    }

    /// Lineup order, as the game's team lists sort.
    pub fn order(self) -> u8 {
        match self {
            Class::Cv => 0,
            Class::Bb => 1,
            Class::Ca => 2,
            Class::Dd => 3,
            Class::Ss => 4,
        }
    }

    /// Polygons in a 24×24 box, bow up; and a relative size. Traced by the
    /// suite (`src/canvas/shipGlyphs.ts`).
    fn glyph(self) -> (&'static [&'static [(f32, f32)]], f32) {
        const BB: &[&[(f32, f32)]] = &[
            &[(12.0, 1.1), (17.1, 6.6), (17.1, 13.5), (6.9, 8.4), (6.9, 6.6)],
            &[(6.9, 10.1), (17.1, 15.2), (17.1, 17.8), (6.9, 12.6)],
            &[(17.1, 19.5), (17.1, 22.9), (6.9, 22.9), (6.9, 14.4)],
        ];
        const CA: &[&[(f32, f32)]] = &[
            &[(12.0, 1.1), (17.1, 6.6), (17.1, 16.1), (6.9, 10.9), (6.9, 6.6)],
            &[(17.1, 17.8), (17.1, 22.9), (6.9, 22.9), (6.9, 12.6)],
        ];
        const CV: &[&[(f32, f32)]] = &[
            &[(12.0, 1.1), (17.1, 6.6), (17.1, 9.6), (6.9, 9.6), (6.9, 6.6)],
            &[(6.9, 11.8), (11.1, 11.8), (11.1, 22.9), (6.9, 22.9)],
            &[(12.9, 11.8), (17.1, 11.8), (17.1, 22.9), (12.9, 22.9)],
        ];
        const DD: &[&[(f32, f32)]] = &[&[(12.0, 1.1), (17.1, 22.9), (6.9, 22.9)]];
        const SS: &[&[(f32, f32)]] =
            &[&[(12.0, 1.1), (17.1, 19.5), (6.9, 19.5)], &[(6.9, 21.2), (17.1, 21.2), (17.1, 22.9), (6.9, 22.9)]];
        match self {
            Class::Bb => (BB, 1.16),
            Class::Ca => (CA, 1.02),
            Class::Cv => (CV, 1.18),
            Class::Dd => (DD, 0.88),
            Class::Ss => (SS, 0.84),
        }
    }

    pub fn path(self) -> Path {
        let (polys, _) = self.glyph();
        let mut pb = PathBuilder::new();
        for poly in polys {
            for (i, &(x, y)) in poly.iter().enumerate() {
                if i == 0 { pb.move_to(x - 12.0, y - 12.0) } else { pb.line_to(x - 12.0, y - 12.0) }
            }
            pb.close();
        }
        pb.finish().expect("glyphs are non-empty")
    }

    pub fn scale(self) -> f32 {
        self.glyph().1
    }
}

/// Glyph paths, built once.
pub struct Glyphs {
    paths: [Path; 5],
}

impl Default for Glyphs {
    fn default() -> Self {
        Self { paths: [Class::Dd, Class::Ca, Class::Bb, Class::Cv, Class::Ss].map(Class::path) }
    }
}

pub struct ShipMark {
    pub x: f32,
    pub y: f32,
    /// Screen angle: 0 points east, π/2 north (as the draw commands give it).
    pub yaw: f32,
    pub class: Class,
    pub size: f32,
    pub fill: Rgb,
    pub alpha: f32,
    pub outline: Option<Rgb>,
}

impl Glyphs {
    pub fn draw(&self, pm: &mut Pixmap, m: &ShipMark) {
        let path = &self.paths[m.class as usize];
        let k = m.size * m.class.scale() / 24.0;
        // Glyphs point up (screen −y, i.e. angle π/2); rotate to `yaw`.
        let rot = (std::f32::consts::FRAC_PI_2 - m.yaw).to_degrees();
        let t = Transform::from_scale(k, k).post_rotate(rot).post_translate(m.x, m.y);
        let stroke =
            |width: f32| Stroke { width: width / k, line_join: tiny_skia::LineJoin::Round, ..Stroke::default() };
        // Spotted: a ring outside the icon. Stroked at twice the width
        // first, so the fill covers its inner half and the icon is unchanged.
        if let Some(c) = m.outline {
            pm.stroke_path(path, &paint(c, m.alpha), &stroke(2.0 * 1.8), t, None);
        }
        pm.fill_path(path, &paint(m.fill, m.alpha), FillRule::Winding, t, None);
        pm.stroke_path(path, &paint(hex(0x080c11), m.alpha), &stroke(0.9), t, None);
    }
}
