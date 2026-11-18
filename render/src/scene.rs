//! Runs the battle and samples it into frames: a first pass learns the
//! battle's span and shell flight times, a second paints frames at evenly
//! spaced battle times — at a fixed speed-up (×32 by default), or spread
//! over a requested duration.

use std::collections::BTreeMap;
use std::collections::HashMap;

use replay_parser::Replay;
use replay_parser::draw;
use replay_parser::draw::Draw;
use replay_parser::draw::MapGeometry;
use replay_parser::entity::EntityDecoder;
use replay_parser::packet::Packets;
use replay_parser::world::World;
use tiny_skia::Pixmap;
use tiny_skia::PixmapPaint;
use tiny_skia::Transform;

use crate::Progress;
use crate::RenderError;
use crate::RenderOptions;
use crate::Stage;
use crate::data::DataDir;
use crate::map::MapLayer;
use crate::paint::Class;
use crate::sidebar::*;
use crate::text::Text;

/// What the second pass will produce, known before its first frame.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub frames: u32,
    pub fps: u32,
    /// Video length in seconds (`frames / fps`).
    pub seconds: f32,
    pub battle_seconds: f32,
    pub speed: f32,
}

/// Receives the frames. Any `FnMut(&Pixmap) -> Result<(), RenderError>`
/// is one; implement [`FrameSink::begin`] to learn the [`Plan`] first.
pub trait FrameSink {
    fn begin(&mut self, _plan: &Plan) -> Result<(), RenderError> {
        Ok(())
    }
    fn frame(&mut self, pm: &Pixmap) -> Result<(), RenderError>;
}

impl<F: FnMut(&Pixmap) -> Result<(), RenderError>> FrameSink for F {
    fn frame(&mut self, pm: &Pixmap) -> Result<(), RenderError> {
        self(pm)
    }
}

/// Longest video a speed-up may produce before the speed is raised to fit.
pub const MAX_SECONDS: f32 = 90.0;
/// Final frame hold, seconds.
const HOLD_SECONDS: f32 = 1.5;

pub struct Summary {
    pub frames: u32,
    pub warnings: Vec<String>,
    /// Battle seconds covered, and the playback speed-up.
    pub battle_seconds: f32,
    pub speed: f32,
}

/// The battle's span in replay clock: from the battle clock starting (the
/// pre-battle countdown is skipped) to a few seconds after the result, or the
/// last packet.
struct Span {
    start: f32,
    end: f32,
}

const AFTER_RESULT: f32 = 4.0;

pub fn render(
    bytes: &[u8],
    data: &DataDir,
    opts: &RenderOptions,
    progress: &mut dyn FnMut(Progress),
    sink: &mut dyn FrameSink,
) -> Result<Summary, RenderError> {
    let mut warnings = Vec::new();
    progress(Progress { stage: Stage::Parse, done: 0, total: 1 });
    let replay = Replay::from_bytes(bytes).map_err(|e| RenderError::BadReplay(e.to_string()))?;
    let meta = &replay.meta;
    let version = meta.version().map_err(|e| RenderError::BadReplay(e.to_string()))?;
    if !replay.stream_complete {
        warnings.push("the replay's packet stream is truncated; the video ends where it does".into());
    }
    let defs = data.defs(&version)?;
    let game = data.game_data(&version)?;
    if game.is_none() {
        warnings.push(format!(
            "no GameParams data for {}.{}.{}: ship names and classes are unknown (player names and cruiser marks are shown)",
            version.major, version.minor, version.patch
        ));
    }
    let gp = game.as_ref().map(|g| &g.gameparams);
    let death_reasons = game.as_ref().and_then(|g| g.death_reasons.as_deref());
    let empty = HashMap::new();
    let names = game.as_ref().map_or(&empty, |g| &g.names);
    let map = data.map(&meta.map_name)?;
    if map.is_none() {
        warnings.push(format!("no map image for {}: drawn on a plain grid", meta.map_name));
    }
    progress(Progress { stage: Stage::Parse, done: 1, total: 1 });

    // Pass 1: span, flight times, map size fallback.
    let packet_total = Packets::new(&replay.packets, version).count();
    let mut decoder = EntityDecoder::new(&defs);
    let mut world = World::new(meta);
    if let Some(gp) = gp {
        world = world.with_params(gp);
    }
    let (mut failed, mut entity_packets) = (0usize, 0usize);
    let mut start = None;
    let mut first_clock_left = None;
    let mut prev_left: Option<i64> = None;
    let mut time_limit = None;
    let mut decided = None;
    let mut last = 0f32;
    let mut extent = 0f32;
    for (i, p) in Packets::new(&replay.packets, version).enumerate() {
        let event = match decoder.feed(&p) {
            Ok(e) => {
                entity_packets += usize::from(e.is_some());
                e
            }
            Err(_) => {
                failed += 1;
                None
            }
        };
        world.apply(&decoder, &p, event.as_ref());
        last = world.clock;
        if let Some(left) = world.time_left {
            first_clock_left.get_or_insert(world.clock);
            // The countdown ends when the clock jumps up to the time limit
            // (the countdown itself starts with a small jump, 0 to 30).
            if start.is_none() && prev_left.is_some_and(|p| left > p + 60) {
                start = Some(world.clock);
                time_limit = Some(left as f32);
            }
            prev_left = Some(left);
        }
        if decided.is_none() && world.winner.is_some() {
            decided = Some(world.clock);
        }
        for s in world.ships.values() {
            extent = extent.max(s.position.x.abs()).max(s.position.z.abs());
        }
        if i % 4096 == 0 {
            progress(Progress { stage: Stage::Simulate, done: i as u32, total: packet_total as u32 });
        }
    }
    progress(Progress { stage: Stage::Simulate, done: packet_total as u32, total: packet_total as u32 });
    let attempted = entity_packets + failed;
    if attempted > 0 && failed * 100 > attempted {
        warnings.push(format!(
            "{failed} of {attempted} entity packets failed to decode: the entity definitions may not match this game build"
        ));
    }
    let flights = draw::flights(&world.shell_hits);
    let span = Span {
        // A moment in, so the packet that starts the clock has been applied.
        start: start.or(first_clock_left).unwrap_or(0.0) + 0.5,
        end: decided.map_or(last, |d| (d + AFTER_RESULT).min(last)),
    };
    if std::env::var_os("WOWS_RENDER_DEBUG").is_some() {
        eprintln!("span {:.1}..{:.1}", span.start, span.end);
    }
    if span.end <= span.start {
        return Err(RenderError::BadReplay("the replay holds no battle time".into()));
    }
    let geometry = match map.as_ref().and_then(|m| m.space_size) {
        Some(space_size) => MapGeometry { space_size },
        None => {
            // The playable square is centred on the origin; ships stay inside.
            let side = ((extent * 2.0 / 100.0).ceil() * 100.0).max(600.0) as i32;
            warnings.push(format!("map size unknown for {}: estimated {side} from ship positions", meta.map_name));
            MapGeometry { space_size: side }
        }
    };

    // Pass 2: frames.
    let (w, h) = (opts.width, opts.height);
    let map_px = h.min(w);
    let side_w = (w - map_px) as f32;
    let painter = SidebarPainter::default();
    let icons = data.icons();
    if icons.planes.is_empty() {
        warnings.push("no battle_hud icons in the data dir: planes drawn as shapes, kills named in words".into());
    }
    let mut text = Text::new();
    for (path, bytes) in data.fonts() {
        if !text.add_fallback(bytes) {
            warnings.push(format!("{}: not a font this renderer can read; skipped", path.display()));
        }
    }
    let mut layer = MapLayer::new(map_px, map.as_ref().map(|m| m.png.as_slice()), opts.grid, &mut text);
    let fps = opts.fps as f32;
    let battle = span.end - span.start;
    let hold = if opts.hold_end { (fps * HOLD_SECONDS) as u32 } else { 0 };
    let moving = match opts.duration {
        // The whole video, hold included, lasts `duration`.
        Some(d) => ((d * fps).round() as u32).saturating_sub(hold),
        None => ((battle / opts.speed).min(MAX_SECONDS) * fps).round() as u32,
    }
    .max(2);
    let total = moving + hold;
    let speed = battle / (moving as f32 / fps);
    if opts.duration.is_none() && speed > opts.speed * 1.01 {
        warnings.push(format!(
            "the battle is long: sped up ×{speed:.0} instead of ×{} to stay under {MAX_SECONDS} s",
            opts.speed
        ));
    }
    sink.begin(&Plan { frames: total, fps: opts.fps, seconds: total as f32 / fps, battle_seconds: battle, speed })?;
    let at = |k: u32| span.start + battle * k as f32 / (moving - 1).max(1) as f32;

    let mut decoder = EntityDecoder::new(&defs);
    let mut world = World::new(meta);
    if let Some(gp) = gp {
        world = world.with_params(gp);
    }
    let labels = draw::labels(&world.players, gp, names);
    let mut caps_seen: BTreeMap<u32, i64> = BTreeMap::new();
    let mut cap_events: Vec<Event> = Vec::new();
    let mut frame = Pixmap::new(w, h).ok_or_else(|| RenderError::Internal("bad frame size".into()))?;
    let mut k = 0u32;
    let title = map.as_ref().map_or_else(|| map_title(&meta.map_display_name), |m| m.name.clone());
    let subtitle = subtitle(meta, &labels, &world);
    let mut paint_frame =
        |world: &World, t: f32, frame: &mut Pixmap, cap_events: &mut Vec<Event>, caps_seen: &mut BTreeMap<u32, i64>| {
            let cmds = draw::frame(world, &geometry, &labels, gp, &flights, t);
            frame.draw_pixmap(0, 0, layer.background.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            layer.paint(frame, &mut text, &icons, &cmds, t);
            track_caps(world, t - span.start, caps_seen, cap_events);
            let sb = sidebar(
                world,
                &labels,
                &cmds,
                cap_events,
                span.start,
                speed,
                time_limit,
                title.clone(),
                subtitle.clone(),
                death_reasons,
            );
            painter.paint(frame, &mut text, &icons, &sb, map_px as f32, side_w, h as f32);
        };
    for p in Packets::new(&replay.packets, version) {
        let event = decoder.feed(&p).ok().flatten();
        world.apply(&decoder, &p, event.as_ref());
        while k < moving && world.clock >= at(k) {
            paint_frame(&world, at(k), &mut frame, &mut cap_events, &mut caps_seen);
            sink.frame(&frame)?;
            k += 1;
            progress(Progress { stage: Stage::Render, done: k, total });
        }
        if k >= moving {
            break;
        }
    }
    while k < moving {
        paint_frame(&world, at(k), &mut frame, &mut cap_events, &mut caps_seen);
        sink.frame(&frame)?;
        k += 1;
        progress(Progress { stage: Stage::Render, done: k, total });
    }
    if hold > 0 {
        let own = world.own_team();
        let score = |t: Option<i64>| t.and_then(|t| world.scores.get(t as usize).copied()).unwrap_or(0);
        // winnerTeamId: a team, or -1 for a draw; none when the replay ends first.
        if let Some(w) = world.winner {
            let result = (w >= 0).then(|| Some(w) == own);
            layer.result_banner(&mut frame, &mut text, result, (score(own), score(own.map(|o| 1 - o))));
        }
    }
    for _ in 0..hold {
        sink.frame(&frame)?;
        k += 1;
        progress(Progress { stage: Stage::Render, done: k, total });
    }
    Ok(Summary { frames: k, warnings, battle_seconds: battle, speed })
}

/// `42_Neighbors` → `Neighbors`, for maps `maps.json` doesn't name.
fn map_title(display: &str) -> String {
    let s = display.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches('_');
    s.replace('_', " ")
}

fn subtitle(meta: &replay_parser::ReplayMeta, labels: &draw::Labels, world: &World) -> String {
    let mode = match meta.game_type.as_deref() {
        // Short, so the player and ship fit beside them.
        Some("RandomBattle") => "Random".to_owned(),
        Some("RankedBattle") => "Ranked".to_owned(),
        Some("ClanBattle") => "Clan".to_owned(),
        Some("CooperativeBattle") => "Co-op".to_owned(),
        Some("PVE") => "Operation".to_owned(),
        Some("EventBattle") => "Event".to_owned(),
        Some(other) => other.to_owned(),
        None => meta.scenario.clone(),
    };
    let me = world.players.iter().position(|p| p.relation == 0);
    let who = me
        .and_then(|i| labels.players.get(i))
        .map(|l| match &l.ship {
            Some(s) => format!("{} · {s}", l.player),
            None => l.player.clone(),
        })
        .unwrap_or_else(|| meta.player_name.clone());
    let allies = meta.vehicles.iter().filter(|v| v.relation != 2).count();
    let enemies = meta.vehicles.len() - allies;
    format!("{mode} · {allies} v {enemies} · {who}")
}

/// As the draw commands label caps: a letter, or a flag for bases.
fn cap_label(c: &replay_parser::world::CapturePoint) -> String {
    match c.kind {
        2 | 3 | 5 => "\u{2691}".to_owned(),
        _ => char::from_u32('A' as u32 + c.index.clamp(0, 25) as u32).unwrap_or('?').to_string(),
    }
}

/// Bots are named by catalogue key in colons (`:Bouvet:`) when the
/// catalogue does not resolve them.
fn display_name(s: &str) -> String {
    s.trim_matches(':').to_owned()
}

fn track_caps(world: &World, t: f32, seen: &mut BTreeMap<u32, i64>, events: &mut Vec<Event>) {
    let own = world.own_team();
    for (id, c) in &world.caps {
        if !c.enabled {
            continue;
        }
        match seen.insert(*id, c.team) {
            Some(before) if before != c.team => events.push(Event {
                t,
                kind: EventKind::Cap { label: cap_label(c), owner: (c.team >= 0).then(|| Some(c.team) == own) },
            }),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sidebar(
    world: &World,
    labels: &draw::Labels,
    cmds: &[Draw<'_>],
    cap_events: &[Event],
    start: f32,
    speed: f32,
    time_limit: Option<f32>,
    title: String,
    subtitle: String,
    death_reasons: Option<&[String]>,
) -> Sidebar {
    let own = world.own_team();
    let ship_of: HashMap<usize, (u32, &replay_parser::world::Ship)> =
        world.ships.iter().filter_map(|(id, s)| Some((s.player?, (*id, s)))).collect();
    let dead: std::collections::HashSet<u32> = world.deaths.iter().map(|d| d.victim).collect();
    let mut allies = Vec::new();
    let mut enemies = Vec::new();
    for (i, p) in world.players.iter().enumerate() {
        let l = labels.players.get(i);
        let ship = ship_of.get(&i);
        let health = match ship {
            Some((_, s)) if s.max_health > 0.0 => s.health / s.max_health,
            _ => 1.0,
        };
        let alive = ship.is_none_or(|(id, s)| s.alive && !dead.contains(id));
        let row = Row {
            name: l.and_then(|l| l.ship.clone()).unwrap_or_else(|| display_name(l.map_or(&p.name, |l| &l.player))),
            class: Class::of(l.and_then(|l| l.species.as_deref())),
            ally: p.relation != 2,
            is_self: p.relation == 0,
            health,
            alive,
        };
        if row.ally { allies.push(row) } else { enemies.push(row) }
    }
    for team in [&mut allies, &mut enemies] {
        team.sort_by(|a, b| a.class.order().cmp(&b.class.order()).then_with(|| a.name.cmp(&b.name)));
    }

    let who = |id: u32| -> Option<Who> {
        let s = world.ships.get(&id);
        let ally = match (s.map(|s| s.team), own) {
            (Some(t), Some(o)) => t == o,
            _ => world.player_of(id).is_some_and(|p| world.players[p].relation != 2),
        };
        let (name, species) = match world.player_of(id).and_then(|p| labels.players.get(p).map(|l| (p, l))) {
            Some((_, l)) => (l.ship.clone().unwrap_or_else(|| display_name(&l.player)), l.species.clone()),
            None => {
                let sp = s.and_then(|s| labels.species.get(&s.params_id)).cloned();
                (sp.clone().unwrap_or_else(|| "Bot".into()), sp)
            }
        };
        Some(Who { name, class: Class::of(species.as_deref()), ally })
    };
    let reason = |code: i64| replay_parser::constants::death_reason(code, death_reasons);
    let mut events: Vec<Event> = world
        .deaths
        .iter()
        .filter_map(|d| {
            Some(Event {
                t: d.clock - start,
                kind: EventKind::Kill {
                    killer: (d.killer != 0 && d.killer != d.victim).then(|| who(d.killer)).flatten(),
                    victim: who(d.victim)?,
                    verb: kill_verb(reason(d.reason)),
                    frag: frag_icon(reason(d.reason)),
                },
            })
        })
        .collect();
    events.extend(cap_events.iter().map(|e| Event {
        t: e.t,
        kind: match &e.kind {
            EventKind::Cap { label, owner } => EventKind::Cap { label: label.clone(), owner: *owner },
            EventKind::Kill { .. } => unreachable!("only caps are tracked"),
        },
    }));
    events.sort_by(|a, b| a.t.total_cmp(&b.t));

    let mut caps: Vec<Cap> = cmds
        .iter()
        .filter_map(|c| match c {
            Draw::CapturePoint { label, color, invader_color, progress, .. } => {
                Some(Cap { label: label.clone(), color: *color, invader: *invader_color, progress: *progress })
            }
            _ => None,
        })
        .collect();
    caps.sort_by(|a, b| a.label.cmp(&b.label));
    let score = |team: Option<i64>| team.and_then(|t| world.scores.get(t as usize).copied()).unwrap_or(0);
    let enemy_team = own.map(|o| 1 - o);
    Sidebar {
        title,
        subtitle,
        time_left: world.time_left,
        time_limit,
        speed,
        ally_score: score(own),
        enemy_score: score(enemy_team),
        win_score: world.win_score.unwrap_or(1000),
        result: world.winner.filter(|w| *w >= 0).map(|w| Some(w) == own),
        caps,
        allies,
        enemies,
        events,
    }
}
