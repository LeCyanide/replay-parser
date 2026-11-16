//! Inspects a replay without game data.
//!
//!   replay-dump meta    <file.wowsreplay>   arena info as JSON
//!   replay-dump stats   <file.wowsreplay>   per-kind packet counts and decode failures
//!   replay-dump packets <file.wowsreplay>   one JSON line per packet
//!   replay-dump entities <file.wowsreplay> <scripts dir>
//!       entity-level decode coverage, with the first failure of each kind
//!   replay-dump events <file.wowsreplay> <scripts dir>
//!       one JSON line per decoded entity event
//!   replay-dump world <file.wowsreplay> <scripts dir>
//!       one JSON world snapshot per second of game clock

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
            "usage: replay-dump <meta|stats|packets> <file.wowsreplay>\n       replay-dump <entities|events|world> <file.wowsreplay> <scripts dir>"
        );
        std::process::exit(2);
    };
    if let Err(e) = run(cmd, std::path::Path::new(path), args.get(2).map(std::path::Path::new)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cmd: &str, path: &std::path::Path, scripts: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
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
            let defs = EntityDefs::load(&ScriptDir(scripts.to_path_buf()))?;
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
            let defs = EntityDefs::load(&ScriptDir(scripts.to_path_buf()))?;
            let mut decoder = EntityDecoder::new(&defs);
            let mut world = replay_parser::world::World::new();
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
        other => return Err(format!("unknown command {other:?}").into()),
    }
    Ok(())
}
