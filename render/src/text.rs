//! Text drawn as filled glyph outlines, so it antialiases like every other
//! shape. One embedded variable font (Inter, SIL OFL 1.1) at two weights.

use std::collections::HashMap;

use ab_glyph::Font;
use ab_glyph::FontRef;
use ab_glyph::FontVec;
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

/// Titles the game abbreviates in ship names; other leading words become
/// their initial.
const TITLES: &[(&str, &str)] = &[("Almirante", "Almte."), ("Admiral", "Adm.")];

/// A word of a name that can be shortened or kept as its identifying part:
/// capitalised, not all capitals (II, CLR, HSF), not the "Golden" variant tag.
fn proper(w: &str) -> bool {
    w.chars().next().is_some_and(char::is_uppercase) && w.chars().any(char::is_lowercase) && w != "Golden"
}

/// "Almirante" → "Almte.", "Marcantonio" → "M."; None for words that stay.
fn abbreviate(w: &str) -> Option<String> {
    if !proper(w) || w.ends_with('.') {
        return None;
    }
    if let Some((_, short)) = TITLES.iter().find(|(t, _)| *t == w) {
        return Some((*short).to_owned());
    }
    w.chars().next().map(|c| format!("{c}."))
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

/// Which font a glyph comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Face {
    Inter(Weight),
    /// Index into [`Text::fallbacks`].
    Fallback(usize),
}

/// One laid-out character: a glyph at a pen position, or the base flag.
enum Piece {
    Glyph { face: Face, id: GlyphId, x: f32 },
    Flag { x: f32 },
}

pub struct Text {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
    /// Fonts for characters Inter lacks (CJK names, …), in order; loaded at
    /// run time so the binary does not carry them.
    fallbacks: Vec<FontVec>,
    /// Glyph outlines in font units (y up), built once.
    paths: HashMap<(Face, GlyphId), Option<Path>>,
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
        Self { regular: font(400.0), bold: font(650.0), fallbacks: Vec::new(), paths: HashMap::new() }
    }

    /// Adds a fallback font (TrueType/OpenType, or the first face of a
    /// collection). Returns false if it does not parse.
    pub fn add_fallback(&mut self, bytes: Vec<u8>) -> bool {
        match FontVec::try_from_vec_and_index(bytes, 0) {
            Ok(f) => {
                self.fallbacks.push(f);
                true
            }
            Err(_) => false,
        }
    }

    fn with_face<R>(&self, face: Face, f: impl FnOnce(&dyn FontDyn) -> R) -> R {
        match face {
            Face::Inter(Weight::Regular) => f(&self.regular),
            Face::Inter(Weight::Bold) => f(&self.bold),
            Face::Fallback(i) => f(&self.fallbacks[i]),
        }
    }

    /// The face that has `c`: Inter, else the first fallback with it, else
    /// Inter (whose missing glyph is skipped when drawing).
    fn face_for(&self, c: char, w: Weight) -> (Face, GlyphId) {
        let inter = if w == Weight::Bold { &self.bold } else { &self.regular };
        let g = inter.glyph_id(c);
        if g.0 != 0 || c.is_whitespace() {
            return (Face::Inter(w), g);
        }
        for (i, f) in self.fallbacks.iter().enumerate() {
            let fg = f.glyph_id(c);
            if fg.0 != 0 {
                return (Face::Fallback(i), fg);
            }
        }
        (Face::Inter(w), g)
    }

    /// Pen positions from 0 and the total advance, at `px`.
    fn layout(&self, s: &str, px: f32, w: Weight) -> (Vec<Piece>, f32) {
        let mut out = Vec::with_capacity(s.len());
        let mut pen = 0.0;
        let mut prev: Option<(Face, GlyphId)> = None;
        for c in s.chars() {
            if c == FLAG {
                out.push(Piece::Flag { x: pen });
                pen += px * FLAG_ADVANCE;
                prev = None;
                continue;
            }
            let (face, id) = self.face_for(c, w);
            let (kern, advance) = self.with_face(face, |f| {
                let k = px / f.upem();
                let kern = match prev {
                    Some((pf, pg)) if pf == face => f.kern(pg, id) * k,
                    _ => 0.0,
                };
                (kern, f.advance(id) * k)
            });
            pen += kern;
            out.push(Piece::Glyph { face, id, x: pen });
            pen += advance;
            prev = Some((face, id));
        }
        (out, pen)
    }

    /// Advance width of `s` at `px`.
    pub fn width(&self, s: &str, px: f32, w: Weight) -> f32 {
        self.layout(s, px, w).1
    }

    /// A ship name made to fit `max` pixels the way the game shortens its own
    /// names: leading words become initials, one at a time from the left,
    /// until it fits ("Marcantonio Colonna" → "M. Colonna", "Almirante
    /// Cochrane" → "Almte. Cochrane", "Alberico da Barbiano" → "A. da
    /// Barbiano"). The last proper word and anything after it ("'44",
    /// "Golden") stay, as do particles and capitals ("da", "II", "HSF").
    /// Cut with an ellipsis only if that is still too long.
    pub fn fit_name(&self, s: &str, px: f32, w: Weight, max: f32) -> String {
        let mut words: Vec<String> = s.split(' ').map(str::to_owned).collect();
        let keep = words.iter().rposition(|w| proper(w)).unwrap_or(words.len());
        for i in 0..keep {
            if self.width(&words.join(" "), px, w) <= max {
                break;
            }
            if let Some(short) = abbreviate(&words[i]) {
                words[i] = short;
            }
        }
        self.fit(&words.join(" "), px, w, max)
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

    fn glyph_path(&mut self, face: Face, g: GlyphId) -> Option<&Path> {
        if !self.paths.contains_key(&(face, g)) {
            let path = self.with_face(face, |f| f.outline_path(g));
            self.paths.insert((face, g), path);
        }
        self.paths.get(&(face, g)).and_then(Option::as_ref)
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
        let (pieces, width) = self.layout(s, px, w);
        let x0 = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8);
        for piece in pieces {
            match piece {
                Piece::Flag { x } => flag(pm, x0 + x, y, px, &paint),
                // No glyph anywhere: leave a gap rather than a box.
                Piece::Glyph { id, .. } if id.0 == 0 => {}
                Piece::Glyph { face, id, x } => {
                    let k = self.with_face(face, |f| px / f.upem());
                    if let Some(path) = self.glyph_path(face, id) {
                        let t = Transform::from_row(k, 0.0, 0.0, -k, x0 + x, y);
                        pm.fill_path(path, &paint, FillRule::Winding, t, None);
                    }
                }
            }
        }
        width
    }

    /// Height above the baseline of a capital letter, for centring.
    pub fn cap_height(&self, px: f32) -> f32 {
        px * 0.727
    }
}

/// What layout needs from a font, object-safe over `FontRef` and `FontVec`.
trait FontDyn {
    fn upem(&self) -> f32;
    fn kern(&self, a: GlyphId, b: GlyphId) -> f32;
    fn advance(&self, g: GlyphId) -> f32;
    fn outline_path(&self, g: GlyphId) -> Option<Path>;
}

impl<F: Font> FontDyn for F {
    fn upem(&self) -> f32 {
        self.units_per_em().unwrap_or(1000.0)
    }

    fn kern(&self, a: GlyphId, b: GlyphId) -> f32 {
        self.kern_unscaled(a, b)
    }

    fn advance(&self, g: GlyphId) -> f32 {
        self.h_advance_unscaled(g)
    }

    fn outline_path(&self, g: GlyphId) -> Option<Path> {
        let outline = self.outline(g)?;
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
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ship_names_shorten_like_the_game() {
        let t = Text::new();
        let (px, w) = (11.0, Weight::Regular);
        let fits = |short: &str| t.width(short, px, w) + 0.5;
        for (full, short) in [
            ("Marcantonio Colonna", "M. Colonna"),
            ("Almirante Cochrane", "Almte. Cochrane"),
            ("Alberico da Barbiano", "A. da Barbiano"),
            ("West Virginia '44", "W. Virginia '44"),
            ("Prins van Oranje Golden", "P. van Oranje Golden"),
            ("Admiral Graf Spee", "Adm. G. Spee"),
            ("St. Vincent", "St. Vincent"),
        ] {
            assert_eq!(t.fit_name(full, px, w, fits(short)), short, "{full}");
        }
        // Nothing to shorten when it fits; one word at a time otherwise.
        assert_eq!(t.fit_name("Marcantonio Colonna", px, w, 1000.0), "Marcantonio Colonna");
        assert_eq!(t.fit_name("Admiral Graf Spee", px, w, fits("Adm. Graf Spee")), "Adm. Graf Spee");
    }

    use super::*;

    fn ink(text: &mut Text, s: &str) -> usize {
        let mut pm = Pixmap::new(200, 40).unwrap();
        text.draw(&mut pm, s, 4.0, 30.0, 20.0, Weight::Regular, Align::Left, [255; 3], 1.0);
        pm.pixels().iter().filter(|p| p.alpha() > 0).count()
    }

    #[test]
    fn latin_draws_and_missing_glyphs_leave_gaps() {
        let mut t = Text::new();
        assert!(ink(&mut t, "Wichita") > 50);
        // No CJK in Inter: nothing drawn, but the text still has a width.
        assert_eq!(ink(&mut t, "大和"), 0);
        assert!(t.width("大和", 20.0, Weight::Regular) > 0.0);
    }

    #[test]
    fn fallback_fonts_fill_in() {
        // Any CJK font the test machine has; skipped without one.
        let candidates =
            ["/usr/share/fonts/opentype/ipafont-gothic/ipag.ttf", "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc"];
        let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else { return };
        let mut t = Text::new();
        assert!(!t.add_fallback(b"not a font".to_vec()));
        assert!(t.add_fallback(bytes));
        assert!(ink(&mut t, "大和") > 50);
        // Latin still comes from Inter: same ink as without fallbacks.
        assert_eq!(ink(&mut t, "Wichita"), ink(&mut Text::new(), "Wichita"));
    }
}
