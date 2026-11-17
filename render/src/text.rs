//! Text drawn as filled glyph outlines, so it antialiases like every other
//! shape. One embedded variable font (Inter, SIL OFL 1.1) at two weights.

use std::collections::HashMap;

use ab_glyph::Font;
use ab_glyph::FontRef;
use ab_glyph::GlyphId;
use ab_glyph::OutlineCurve;
use ab_glyph::VariableFont;
use tiny_skia::FillRule;
use tiny_skia::Paint;
use tiny_skia::Path;
use tiny_skia::PathBuilder;
use tiny_skia::Pixmap;
use tiny_skia::Transform;

static INTER: &[u8] = include_bytes!("../assets/Inter.ttf");

/// U+2691 BLACK FLAG, which marks bases in the draw commands; Inter has no
/// glyph for it, so it is drawn as a shape.
const FLAG: char = '\u{2691}';
const FLAG_ADVANCE: f32 = 0.8;

/// A pennant on a pole, cap-height tall, from `x` on baseline `y`.
fn flag(pm: &mut Pixmap, x: f32, y: f32, px: f32, paint: &Paint) {
    let h = px * 0.75;
    let w = px * 0.6;
    let pole = (px * 0.09).max(1.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + px * 0.1, y);
    pb.line_to(x + px * 0.1 + pole, y);
    pb.line_to(x + px * 0.1 + pole, y - h * 0.45);
    pb.line_to(x + px * 0.1 + w, y - h * 0.72);
    pb.line_to(x + px * 0.1 + pole, y - h);
    pb.line_to(x + px * 0.1, y - h);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, paint, FillRule::Winding, Transform::identity(), None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Text {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
    /// Glyph outlines in font units (y up), built once.
    paths: HashMap<(Weight, GlyphId), Option<Path>>,
}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl Text {
    pub fn new() -> Self {
        let font = |w: f32| {
            let mut f = FontRef::try_from_slice(INTER).expect("embedded font parses");
            f.set_variation(b"wght", w);
            f
        };
        Self { regular: font(400.0), bold: font(650.0), paths: HashMap::new() }
    }

    fn font(&self, w: Weight) -> &FontRef<'static> {
        match w {
            Weight::Regular => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    fn scale(&self, w: Weight, px: f32) -> f32 {
        px / self.font(w).units_per_em().unwrap_or(1000.0)
    }

    /// Advance width of `s` at `px`.
    pub fn width(&self, s: &str, px: f32, w: Weight) -> f32 {
        let f = self.font(w);
        let k = self.scale(w, px);
        let mut x = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            if c == FLAG {
                x += px * FLAG_ADVANCE / k;
                prev = None;
                continue;
            }
            let g = f.glyph_id(c);
            if let Some(p) = prev {
                x += f.kern_unscaled(p, g);
            }
            x += f.h_advance_unscaled(g);
            prev = Some(g);
        }
        x * k
    }

    /// Cuts `s` to fit `max` pixels, with an ellipsis.
    pub fn fit(&self, s: &str, px: f32, w: Weight, max: f32) -> String {
        if self.width(s, px, w) <= max {
            return s.to_owned();
        }
        let mut out: String = s.to_owned();
        while !out.is_empty() {
            out.pop();
            let t = format!("{}…", out.trim_end());
            if self.width(&t, px, w) <= max {
                return t;
            }
        }
        String::new()
    }

    fn glyph_path(&mut self, w: Weight, g: GlyphId) -> Option<&Path> {
        let font = self.font(w).clone();
        self.paths
            .entry((w, g))
            .or_insert_with(|| {
                let outline = font.outline(g)?;
                let mut pb = PathBuilder::new();
                let mut last: Option<(f32, f32)> = None;
                for c in &outline.curves {
                    let (start, end) = match c {
                        OutlineCurve::Line(a, b) => (*a, *b),
                        OutlineCurve::Quad(a, _, b) => (*a, *b),
                        OutlineCurve::Cubic(a, _, _, b) => (*a, *b),
                    };
                    if last != Some((start.x, start.y)) {
                        if last.is_some() {
                            pb.close();
                        }
                        pb.move_to(start.x, start.y);
                    }
                    match c {
                        OutlineCurve::Line(_, b) => pb.line_to(b.x, b.y),
                        OutlineCurve::Quad(_, c1, b) => pb.quad_to(c1.x, c1.y, b.x, b.y),
                        OutlineCurve::Cubic(_, c1, c2, b) => pb.cubic_to(c1.x, c1.y, c2.x, c2.y, b.x, b.y),
                    }
                    last = Some((end.x, end.y));
                }
                if last.is_some() {
                    pb.close();
                }
                pb.finish()
            })
            .as_ref()
    }

    /// Draws `s` with its baseline at `y`, aligned on `x`. Returns the width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        y: f32,
        px: f32,
        w: Weight,
        align: Align,
        color: [u8; 3],
        alpha: f32,
    ) -> f32 {
        let width = self.width(s, px, w);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let k = self.scale(w, px);
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8);
        let font = self.font(w).clone();
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            if c == FLAG {
                flag(pm, pen, y, px, &paint);
                pen += px * FLAG_ADVANCE;
                prev = None;
                continue;
            }
            let g = font.glyph_id(c);
            if let Some(p) = prev {
                pen += font.kern_unscaled(p, g) * k;
            }
            // No glyph in the font (CJK names, symbols): leave a gap rather
            // than a box.
            if g.0 == 0 {
                pen += font.h_advance_unscaled(g) * k;
                prev = None;
                continue;
            }
            if let Some(path) = self.glyph_path(w, g) {
                let t = Transform::from_row(k, 0.0, 0.0, -k, pen, y);
                pm.fill_path(path, &paint, FillRule::Winding, t, None);
            }
            pen += font.h_advance_unscaled(g) * k;
            prev = Some(g);
        }
        width
    }

    /// Height above the baseline of a capital letter, for centring.
    pub fn cap_height(&self, px: f32) -> f32 {
        px * 0.727
    }
}
