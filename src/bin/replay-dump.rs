//! Inspects a replay without game data.
//!
//!   replay-dump meta    <file.wowsreplay>   arena info as JSON
//!   replay-dump stats   <file.wowsreplay>   per-kind packet counts and decode failures
//!   replay-dump packets <file.wowsreplay>   one JSON line per packet
//!   <scripts dir> below can also be a game install directory; the build
//!   that recorded the replay is read from it.
//!   replay-dump entities <file.wowsreplay> <scripts dir>
//!       entity-level decode coverage, with the first failure of each kind
//!   replay-dump events <file.wowsreplay> <scripts dir>
//!       one JSON line per decoded entity event
//!   replay-dump world <file.wowsreplay> <scripts dir>
//!       one JSON world snapshot per second of game clock (with a game
//!       install, torpedoes stop at their GameParams range)
//!   replay-dump draw <file.wowsreplay> <game dir>
//!       one JSON line of draw commands per second of game clock
//!   replay-dump res <game dir> [path]
//!       list the newest build's packaged files, or write one to stdout
//!   replay-dump datapack <game dir> <out dir> [build] [key]
//!       writes what wows-render needs for one build (default: the newest)
//!       into a data directory: <out>/defs/<key>/ (entity definitions) and
//!       <out>/gameparams/<key>.json (GameParams extract and ship names), and
//!       <out>/battle_hud/ (minimap plane markers and kill-type icons).
//!       <key> defaults to the build number; name it after the version
//!       (15_10_0) to serve every regional build of it.

use std::collections::BTreeMap;
use std::io::Write;

use replay_parser::Replay;
use replay_parser::defs::EntityDefs;
use replay_parser::defs::ScriptDir;
use replay_parser::entity::EntityDecoder;
use replay_parser::entity::EntityError;
use replay_parser::packet::Packets;

/// Groups failures by entity and member, not by entity id or value.
fn failure_key(e: &EntityError) -> String {
    match e {
        EntityError::UnknownEntity(_) => "unknown entity".into(),
        EntityError::UnknownType(t) => format!("unknown type {t}"),
        EntityError::MethodIndex { entity, index, .. } => format!("{entity}: method index {index}"),
        EntityError::PropertyIndex { entity, index, .. } => format!("{entity}: property index {index}"),
        EntityError::Value { entity, member, .. } => format!("{entity}.{member}: value"),
        EntityError::Leftover { entity, member, .. } => format!("{entity}.{member}: leftover"),
        EntityError::Nested { entity, member, error } => format!(
            "{entity}.{member}: nested: {}",
            match error {
                replay_parser::nested::NestedError::Value(_) => "value".to_string(),
                replay_parser::nested::NestedError::Leftover(_) => "leftover".to_string(),
                other => format!("{other:?}").split(['(', ' ']).next().unwrap_or("").to_string(),
            }
        ),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(path)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: replay-dump <meta|stats|packets> <file.wowsreplay>\n       replay-dump <entities|events|world> <file.wowsreplay> <scripts dir>
       replay-dump draw <file.wowsreplay> <game dir>\n       replay-dump res <game dir> [path]
       replay-dump datapack <game dir> <out dir> [build] [key]"
        );
        std::process::exit(2);
    };
    if let Err(e) = run(cmd, std::path::Path::new(path), args.get(2).map(std::path::Path::new)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cmd: &str, path: &std::path::Path, scripts: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    if cmd == "datapack" {
        let out = scripts.ok_or("needs an output directory")?;
        let rest: Vec<String> = std::env::args().skip(4).collect();
        let res = match rest.first() {
            Some(b) => replay_parser::res::Resources::open_build(path, b.parse()?)?,
            None => replay_parser::res::Resources::open(path)?,
        };
        let key = rest.get(1).cloned().unwrap_or_else(|| res.build().to_string());
        return datapack(&res, out, &key);
    }
    if cmd == "res" {
        let res = replay_parser::res::Resources::open(path)?;
        match scripts.and_then(|p| p.to_str()) {
            Some(file) => std::io::stdout().write_all(&res.read(file)?)?,
            None => {
                let mut out = std::io::stdout().lock();
                for p in res.paths() {
                    writeln!(out, "{p}")?;
                }
            }
        }
        return Ok(());
    }
    if cmd == "meta" {
        let bytes = std::fs::read(path)?;
        let (_, raw) = replay_parser::container::read_meta(&bytes)?;
        println!("{raw}");
        return Ok(());
    }
    let replay = Replay::from_file(path)?;
    let version = replay.meta.version()?;
    let mut packets = Packets::new(&replay.packets, version);
    match cmd {
        "stats" => {
            let mut counts: BTreeMap<String, (u64, u64)> = BTreeMap::new();
            let mut last_clock = 0f32;
            for p in packets.by_ref() {
                let entry = counts.entry(format!("{:?}(0x{:02x})", p.kind, p.wire_type)).or_default();
                entry.0 += 1;
                if p.decode().is_err() {
                    entry.1 += 1;
                }
                last_clock = last_clock.max(p.clock);
            }
            println!("version {version}, stream {} bytes, complete {}", replay.packets.len(), replay.stream_complete);
            println!("last clock {last_clock:.1} s, {} trailing bytes", packets.trailing());
            for (k, (n, bad)) in counts {
                println!("{k:<32} {n:>8}  decode failures {bad}");
            }
        }
        "packets" => {
            let mut out = std::io::stdout().lock();
            for p in packets {
                let decoded = p.decode().map_err(|e| e.to_string());
                let line = serde_json::json!({
                    "clock": p.clock,
                    "wire": p.wire_type,
                    "len": p.payload.len(),
                    "decoded": match decoded { Ok(d) => serde_json::to_value(d)?, Err(e) => serde_json::json!({"error": e}) },
                });
                writeln!(out, "{line}")?;
            }
        }
        "entities" | "events" => {
            let scripts = scripts.ok_or("needs a scripts directory")?;
            let defs = load_defs(scripts, version)?;
            let mut decoder = EntityDecoder::new(&defs);
            let mut out = std::io::stdout().lock();
            let mut ok = 0u64;
            let mut failures: BTreeMap<String, (u64, String)> = BTreeMap::new();
            for p in packets {
                match decoder.feed(&p) {
                    Ok(Some(event)) => {
                        ok += 1;
                        if cmd == "events" {
                            writeln!(out, "{}", serde_json::json!({ "clock": p.clock, "e": event }))?;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let key = failure_key(&e);
                        let entry = failures.entry(key).or_insert_with(|| (0, format!("@{:.2}s {e}", p.clock)));
                        entry.0 += 1;
                    }
                }
            }
            if cmd == "entities" {
                let bad: u64 = failures.values().map(|(n, _)| n).sum();
                println!("decoded {ok}, failed {bad}");
                for (k, (n, first)) in failures {
                    println!("{n:>7}  {k}\n         first: {first}");
                }
            }
        }
        "world" => {
            let scripts = scripts.ok_or("needs a scripts directory")?;
            let defs = load_defs(scripts, version)?;
            let mut decoder = EntityDecoder::new(&defs);
            let mut world = replay_parser::world::World::new(&replay.meta);
            if scripts.join("bin").is_dir() {
                let res = replay_parser::res::Resources::open_build(scripts, version.build)?;
                world = world.with_params(&replay_parser::gameparams::GameParams::load(&res)?);
            }
            let mut out = std::io::stdout().lock();
            let mut next = 0f32;
            for p in packets {
                // Undecodable packets still move the clock and positions.
                let event = decoder.feed(&p).ok().flatten();
                world.apply(&decoder, &p, event.as_ref());
                if world.clock >= next {
                    writeln!(out, "{}", serde_json::to_string(&world)?)?;
                    next = world.clock.floor() + 1.0;
                }
            }
            writeln!(out, "{}", serde_json::to_string(&world)?)?;
        }
        "draw" => {
            let dir = scripts.ok_or("needs a game directory")?;
            let res = replay_parser::res::Resources::open_build(dir, version.build)?;
            let defs = EntityDefs::load(&replay_parser::res::InstallScripts(&res))?;
            let gp = replay_parser::gameparams::GameParams::load(&res)?;
            let names = res.catalog("en")?;
            let space = res.read(&format!("{}/space.settings", replay.meta.map_name))?;
            let map = replay_parser::draw::MapGeometry::from_space_settings(&String::from_utf8_lossy(&space))
                .ok_or("no map bounds in space.settings")?;
            // A first pass learns shell flight times from their hits.
            let mut decoder = EntityDecoder::new(&defs);
            let mut world = replay_parser::world::World::new(&replay.meta).with_params(&gp);
            for p in Packets::new(&replay.packets, version) {
                let event = decoder.feed(&p).ok().flatten();
                world.apply(&decoder, &p, event.as_ref());
            }
            let flights = replay_parser::draw::flights(&world.shell_hits);
            let mut decoder = EntityDecoder::new(&defs);
            let mut world = replay_parser::world::World::new(&replay.meta).with_params(&gp);
            let mut out = std::io::stdout().lock();
            let mut next = 0f32;
            for p in packets {
                let event = decoder.feed(&p).ok().flatten();
                world.apply(&decoder, &p, event.as_ref());
                if world.clock >= next {
                    let labels = replay_parser::draw::labels(&world.players, Some(&gp), &names);
                    let cmds = replay_parser::draw::frame(&world, &map, &labels, Some(&gp), &flights, world.clock);
                    writeln!(out, "{}", serde_json::json!({ "clock": world.clock, "draw": cmds }))?;
                    next = world.clock.floor() + 1.0;
                }
            }
        }
        other => return Err(format!("unknown command {other:?}").into()),
    }
    Ok(())
}

/// Entity definitions and a GameParams extract for one build; see the
/// module docs.
fn datapack(
    res: &replay_parser::res::Resources,
    out: &std::path::Path,
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let defs = out.join("defs").join(key);
    let mut n = 0;
    for p in res.paths() {
        let Some(rel) = p.strip_prefix("scripts/") else { continue };
        if rel != "entities.xml" && !rel.starts_with("entity_defs/") {
            continue;
        }
        let dest = defs.join(rel);
        std::fs::create_dir_all(dest.parent().ok_or("bad path")?)?;
        std::fs::write(&dest, res.read(p)?)?;
        n += 1;
    }
    // Minimap plane markers and kill-type icons, shared by every build.
    let mut icons = 0;
    for p in res.paths() {
        let Some(rel) = p.strip_prefix("gui/battle_hud/") else { continue };
        if !(rel.starts_with("markers_minimap/plane/") || rel.starts_with("icon_frag/")) || !rel.ends_with(".png") {
            continue;
        }
        let dest = out.join("battle_hud").join(rel);
        std::fs::create_dir_all(dest.parent().ok_or("bad path")?)?;
        std::fs::write(&dest, res.read(p)?)?;
        icons += 1;
    }
    if icons == 0 {
        eprintln!("warning: no gui/battle_hud icons found; planes will be drawn as shapes");
    }
    // Checks the copy loads before anything relies on it.
    EntityDefs::load(&ScriptDir(defs.clone()))?;
    let gp = replay_parser::gameparams::GameParams::load(res)?;
    let catalog = res.catalog("en")?;
    let names: BTreeMap<String, String> = gp
        .ships
        .values()
        .filter_map(|s| {
            let k = format!("IDS_{}", s.index);
            Some((k.clone(), catalog.get(&k)?.clone()))
        })
        .collect();
    // Death reasons by index, the build's own (the default table otherwise).
    let death_reasons = res
        .read("gui/data/constants/battle.xml")
        .ok()
        .and_then(|b| replay_parser::constants::death_reasons_from_xml(&String::from_utf8_lossy(&b)));
    if death_reasons.is_none() {
        eprintln!("warning: no DEATH_REASON_NAME table in gui/data/constants/battle.xml; the default is used");
    }
    let json = serde_json::json!({
        "build": res.build(), "gameparams": gp, "names": names, "death_reasons": death_reasons,
    });
    let gp_dir = out.join("gameparams");
    std::fs::create_dir_all(&gp_dir)?;
    let path = gp_dir.join(format!("{key}.json"));
    std::fs::write(&path, serde_json::to_vec(&json)?)?;
    eprintln!("{n} definition files in {}, {} ships in {}, {icons} icons", defs.display(), names.len(), path.display());
    Ok(())
}

/// Entity defs from a loose `scripts/` directory, or from a game install
/// (a directory with `bin/`), using the build that recorded the replay.
fn load_defs(dir: &std::path::Path, version: replay_parser::Version) -> Result<EntityDefs, Box<dyn std::error::Error>> {
    if dir.join("bin").is_dir() {
        let res = replay_parser::res::Resources::open_build(dir, version.build)
            .map_err(|e| format!("build {} of {}: {e}", version.build, dir.display()))?;
        return Ok(EntityDefs::load(&replay_parser::res::InstallScripts(&res))?);
    }
    Ok(EntityDefs::load(&ScriptDir(dir.to_path_buf()))?)
}
