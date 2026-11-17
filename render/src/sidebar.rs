//! The battle sidebar beside the map, after the suite's replay "Battle"
//! panel (`src/components/ReplayEventsPanel.tsx`): score and caps, both
//! lineups with health, and the event list.

use tiny_skia::Pixmap;

use crate::map::ALLY;
use crate::map::ENEMY;
use crate::paint::*;
use crate::text::Align;
use crate::text::Text;
use crate::text::Weight;

const BG: Rgb = [0x0d, 0x11, 0x17];
const BG_HEAD: Rgb = [0x12, 0x1a, 0x23];
const LINE: Rgb = [0x1e, 0x29, 0x37];
const LINE_STRONG: Rgb = [0x2a, 0x39, 0x48];
const FG: Rgb = [0xe6, 0xed, 0xf3];
const FG_DIM: Rgb = [0x97, 0xa5, 0xb4];
const FG_FAINT: Rgb = [0x64, 0x74, 0x8b];
const SELF: Rgb = [0xff, 0xff, 0xff];

pub struct Row {
    /// Ship name if known, else the player's.
    pub name: String,
    pub class: Class,
    pub ally: bool,
    pub is_self: bool,
    /// Health fraction, 0..1.
    pub health: f32,
    pub alive: bool,
}

pub struct Who {
    pub name: String,
    pub class: Class,
    pub ally: bool,
}

pub enum EventKind {
    Kill {
        killer: Option<Who>,
        victim: Who,
        /// As the feed shows it: "sank", "torpedoed", "burned", …
        verb: &'static str,
    },
    /// A cap changed hands; `owner` None when neutralised.
    Cap { label: String, owner: Option<bool> },
}

pub struct Event {
    /// Battle seconds since the start.
    pub t: f32,
    pub kind: EventKind,
}

pub struct Cap {
    pub label: String,
    pub color: Rgb,
    pub invader: Option<Rgb>,
    pub progress: f32,
}

pub struct Sidebar {
    pub title: String,
    pub subtitle: String,
    /// Seconds left on the battle clock.
    pub time_left: Option<i64>,
    /// The battle's time limit in seconds: event times then read as the
    /// header's countdown clock instead of time since the start.
    pub time_limit: Option<f32>,
    /// Playback speed against real time.
    pub speed: f32,
    pub ally_score: i64,
    pub enemy_score: i64,
    pub win_score: i64,
    /// Some(true) on victory, once decided.
    pub result: Option<bool>,
    pub caps: Vec<Cap>,
    pub allies: Vec<Row>,
    pub enemies: Vec<Row>,
    pub events: Vec<Event>,
}

pub fn clock(s: f32) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn side(ally: bool) -> Rgb {
    if ally { ALLY } else { ENEMY }
}

#[derive(Default)]
pub struct SidebarPainter {
    glyphs: Glyphs,
}

impl SidebarPainter {
    /// Paints the sidebar into the `w`×`h` area of `pm` at `x0`.
    pub fn paint(&self, pm: &mut Pixmap, text: &mut Text, s: &Sidebar, x0: f32, w: f32, h: f32) {
        // Layout is authored for a 720 px tall frame and scaled.
        let u = h / 720.0;
        let pad = 14.0 * u;
        rect(pm, x0, 0.0, w, h, BG, 1.0);
        rect(pm, x0, 0.0, 1.0 * u, h, LINE_STRONG, 1.0);

        // Header: map and mode, battle clock.
        let head = 58.0 * u;
        rect(pm, x0, 0.0, w, head, BG_HEAD, 1.0);
        rect(pm, x0, head - u, w, u, LINE, 1.0);
        let clock_w = text.width("00:00", 24.0 * u, Weight::Bold) + pad;
        let title = text.fit(&s.title, 16.0 * u, Weight::Bold, w - 2.0 * pad - clock_w);
        text.draw(pm, &title, x0 + pad, 27.0 * u, 16.0 * u, Weight::Bold, Align::Left, FG, 1.0);
        let sub = text.fit(&s.subtitle, 11.0 * u, Weight::Regular, w - 2.0 * pad - clock_w);
        text.draw(pm, &sub, x0 + pad, 45.0 * u, 11.0 * u, Weight::Regular, Align::Left, FG_DIM, 1.0);
        if let Some(t) = s.time_left {
            text.draw(pm, &clock(t as f32), x0 + w - pad, 30.0 * u, 24.0 * u, Weight::Bold, Align::Right, FG, 1.0);
        }
        let speed = format!("×{:.0} speed", s.speed);
        text.draw(pm, &speed, x0 + w - pad, 46.0 * u, 10.0 * u, Weight::Regular, Align::Right, FG_FAINT, 1.0);

        // Score.
        let mut y = head + 30.0 * u;
        text.draw(pm, &s.ally_score.to_string(), x0 + pad, y, 22.0 * u, Weight::Bold, Align::Left, ALLY, 1.0);
        text.draw(pm, &s.enemy_score.to_string(), x0 + w - pad, y, 22.0 * u, Weight::Bold, Align::Right, ENEMY, 1.0);
        let (mid, mid_c, mid_px, mid_w) = match s.result {
            Some(true) => ("Victory".to_owned(), ALLY, 15.0, Weight::Bold),
            Some(false) => ("Defeat".to_owned(), ENEMY, 15.0, Weight::Bold),
            None => (format!("of {}", s.win_score), FG_FAINT, 11.0, Weight::Regular),
        };
        text.draw(pm, &mid, x0 + w / 2.0, y - 3.0 * u, mid_px * u, mid_w, Align::Center, mid_c, 1.0);
        y += 9.0 * u;
        let bar_w = (w - 2.0 * pad - 4.0 * u) / 2.0;
        let bar_h = 5.0 * u;
        let frac = |v: i64| (v as f32 / s.win_score.max(1) as f32).clamp(0.0, 1.0);
        round_rect(pm, x0 + pad, y, bar_w, bar_h, 2.0 * u, LINE, 1.0);
        round_rect(pm, x0 + pad, y, bar_w * frac(s.ally_score), bar_h, 2.0 * u, ALLY, 1.0);
        let ex = x0 + w - pad - bar_w;
        round_rect(pm, ex, y, bar_w, bar_h, 2.0 * u, LINE, 1.0);
        let ef = bar_w * frac(s.enemy_score);
        round_rect(pm, ex + bar_w - ef, y, ef, bar_h, 2.0 * u, ENEMY, 1.0);
        y += bar_h;

        // Caps: a ring per cap, the capturing team's colour filling in.
        if !s.caps.is_empty() {
            let r = 13.0 * u;
            let gap = 10.0 * u;
            let total = s.caps.len() as f32 * 2.0 * r + (s.caps.len() as f32 - 1.0) * gap;
            let mut cx = x0 + (w - total) / 2.0 + r;
            let cy = y + 10.0 * u + r;
            for c in &s.caps {
                fill_circle(pm, cx, cy, r, c.color, 0.18);
                stroke_circle(pm, cx, cy, r, 2.0 * u, c.color, 1.0);
                if let Some(ic) = c.invader.filter(|_| c.progress > 0.001) {
                    arc(pm, cx, cy, r, c.progress, 2.6 * u, ic, 1.0);
                }
                text.draw(
                    pm,
                    &c.label,
                    cx,
                    cy + text.cap_height(12.0 * u) / 2.0,
                    12.0 * u,
                    Weight::Bold,
                    Align::Center,
                    FG,
                    1.0,
                );
                cx += 2.0 * r + gap;
            }
            y = cy + r;
        }
        y += 12.0 * u;
        rect(pm, x0, y, w, u, LINE, 1.0);

        // Lineups.
        let rows = s.allies.len().max(s.enemies.len());
        let row_h = (19.0 * u).min(((h - y) * 0.55 / rows.max(1) as f32).max(12.0 * u));
        let col_w = (w - 2.0 * 6.0 * u - 4.0 * u) / 2.0;
        let top = y + 6.0 * u;
        for (i, team) in [&s.allies, &s.enemies].into_iter().enumerate() {
            let cx = x0 + 6.0 * u + i as f32 * (col_w + 4.0 * u);
            for (j, r) in team.iter().enumerate() {
                self.row(pm, text, r, cx, top + j as f32 * row_h, col_w, row_h, u);
            }
        }
        rect(pm, x0 + 6.0 * u + col_w + 1.5 * u, top, u, rows as f32 * row_h, LINE_STRONG, 1.0);
        y = top + rows as f32 * row_h + 6.0 * u;
        rect(pm, x0, y, w, u, LINE, 1.0);

        // Events, newest at the bottom; as many as fit.
        y += 8.0 * u;
        text.draw(pm, "BATTLE EVENTS", x0 + pad, y + 9.0 * u, 9.5 * u, Weight::Bold, Align::Left, FG_FAINT, 1.0);
        y += 16.0 * u;
        let ev_h = 19.0 * u;
        let fit = ((h - y - 6.0 * u) / ev_h).floor().max(0.0) as usize;
        if s.events.is_empty() {
            text.draw(
                pm,
                "No kills or captures yet.",
                x0 + pad,
                y + 12.0 * u,
                11.0 * u,
                Weight::Regular,
                Align::Left,
                FG_FAINT,
                1.0,
            );
        }
        let skip = s.events.len().saturating_sub(fit);
        for (k, e) in s.events.iter().skip(skip).enumerate() {
            let ey = y + k as f32 * ev_h + 13.0 * u;
            // Newest is brightest.
            let age = (s.events.len() - skip - 1 - k) as f32;
            let a = (1.0 - age * 0.06).max(0.55);
            let at = s.time_limit.map_or(e.t, |l| l - e.t);
            self.event(pm, text, e, at, x0 + pad, ey, w - 2.0 * pad, u, a);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row(&self, pm: &mut Pixmap, text: &mut Text, r: &Row, x: f32, y: f32, w: f32, h: f32, u: f32) {
        let c = side(r.ally);
        let a = if r.alive { 1.0 } else { 0.35 };
        let g = 13.0 * u;
        let gy = y + h * 0.45;
        if r.is_self {
            round_rect(pm, x, y + 1.0 * u, w, h - 2.0 * u, 2.0 * u, LINE, 1.0);
        }
        self.glyphs.draw(
            pm,
            &ShipMark {
                x: x + 4.0 * u + g / 2.0,
                y: gy,
                yaw: std::f32::consts::FRAC_PI_2,
                class: r.class,
                size: g,
                fill: c,
                alpha: a,
                outline: None,
            },
        );
        let tx = x + 8.0 * u + g;
        let px = 11.0 * u;
        let name = text.fit(&r.name, px, Weight::Regular, w - (tx - x) - 3.0 * u);
        let color = if !r.alive {
            FG_FAINT
        } else if r.is_self {
            SELF
        } else {
            FG
        };
        let weight = if r.is_self { Weight::Bold } else { Weight::Regular };
        let baseline = gy + text.cap_height(px) / 2.0 - 1.0 * u;
        let tw = text.draw(pm, &name, tx, baseline, px, weight, Align::Left, color, 1.0);
        if !r.alive {
            rect(pm, tx, baseline - text.cap_height(px) * 0.45, tw, 1.0 * u, FG_FAINT, 1.0);
        } else {
            // Health: a thin bar under the name.
            let by = y + h - 3.5 * u;
            let bw = w - (tx - x) - 4.0 * u;
            rect(pm, tx, by, bw, 2.0 * u, LINE_STRONG, 1.0);
            rect(pm, tx, by, bw * r.health.clamp(0.0, 1.0), 2.0 * u, c, 0.9);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn event(&self, pm: &mut Pixmap, text: &mut Text, e: &Event, at: f32, x: f32, y: f32, w: f32, u: f32, a: f32) {
        let px = 11.0 * u;
        text.draw(pm, &clock(at), x, y, 10.5 * u, Weight::Regular, Align::Left, FG_FAINT, a);
        let mut cx = x + 38.0 * u;
        let end = x + w;
        let who = |pm: &mut Pixmap, text: &mut Text, cx: &mut f32, wh: &Who, max: f32| {
            let g = 11.0 * u;
            self.glyphs.draw(
                pm,
                &ShipMark {
                    x: *cx + g / 2.0,
                    y: y - text.cap_height(px) / 2.0,
                    yaw: std::f32::consts::FRAC_PI_2,
                    class: wh.class,
                    size: g,
                    fill: side(wh.ally),
                    alpha: a,
                    outline: None,
                },
            );
            *cx += g + 3.0 * u;
            let n = text.fit(&wh.name, px, Weight::Regular, max - g - 3.0 * u);
            *cx += text.draw(pm, &n, *cx, y, px, Weight::Regular, Align::Left, side(wh.ally), a) + 5.0 * u;
        };
        match &e.kind {
            EventKind::Kill { killer, victim, verb } => {
                let verb_w = text.width(verb, px, Weight::Regular) + 5.0 * u;
                let half = (end - cx - verb_w) / 2.0;
                match killer {
                    Some(k) => {
                        who(pm, text, &mut cx, k, half);
                        cx += text.draw(pm, verb, cx, y, px, Weight::Regular, Align::Left, FG_DIM, a) + 5.0 * u;
                    }
                    None => {
                        cx += text.draw(pm, "Sunk:", cx, y, px, Weight::Regular, Align::Left, FG_DIM, a) + 5.0 * u;
                    }
                }
                let rest = end - cx;
                who(pm, text, &mut cx, victim, rest);
            }
            EventKind::Cap { label, owner } => {
                let r = 7.0 * u;
                let cy = y - text.cap_height(px) / 2.0;
                fill_circle(pm, cx + r, cy, r, LINE_STRONG, a);
                text.draw(
                    pm,
                    label,
                    cx + r,
                    cy + text.cap_height(9.5 * u) / 2.0,
                    9.5 * u,
                    Weight::Bold,
                    Align::Center,
                    FG,
                    a,
                );
                cx += 2.0 * r + 6.0 * u;
                let (msg, c) = match owner {
                    Some(true) => ("captured by allies", ALLY),
                    Some(false) => ("captured by enemies", ENEMY),
                    None => ("neutralised", FG_DIM),
                };
                text.draw(pm, msg, cx, y, px, Weight::Regular, Align::Left, c, a);
            }
        }
    }
}

/// The feed's verb for a death reason constant (`replay_parser::constants`).
pub fn kill_verb(reason: Option<&str>) -> &'static str {
    match reason {
        Some("TORPEDO" | "TBOMB") => "torpedoed",
        Some("BURNING") => "burned",
        Some("FLOOD") => "flooded",
        Some("RAM") => "rammed",
        Some("BOMB" | "SKIP_BOMB") => "bombed",
        Some("ROCKET") => "rocketed",
        Some("DBOMB" | "ADBOMB") => "depth-charged",
        Some("DETONATE") => "detonated",
        Some("SEA_MINE") => "mined",
        Some("ATBA") => "sank (secondaries)",
        _ => "sank",
    }
}
