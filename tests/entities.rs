//! Entity-level decoding over the fixture replays. Needs both
//! `scripts/fetch-fixtures.sh` and `scripts/fetch-entity-defs.sh`; fails
//! without them rather than passing empty.

use std::collections::HashSet;
use std::path::PathBuf;

use replay_parser::Replay;
use replay_parser::defs::EntityDefs;
use replay_parser::defs::ScriptDir;
use replay_parser::entity::EntityDecoder;
use replay_parser::entity::Event;
use replay_parser::packet::Kind;
use replay_parser::packet::Packets;
use replay_parser::pickle;
use replay_parser::value::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixtures() -> Vec<(PathBuf, PathBuf)> {
    let dir = root().join("replays");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("no fixtures at {}; run scripts/fetch-fixtures.sh", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wowsreplay"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "run scripts/fetch-fixtures.sh");
    files
        .into_iter()
        .map(|f| {
            let bytes = std::fs::read(&f).unwrap();
            let (meta, _) = replay_parser::container::read_meta(&bytes).unwrap();
            let v = meta.version().unwrap();
            let scripts = root().join("scripts").join(format!("{}_{}_{}", v.major, v.minor, v.patch));
            assert!(scripts.is_dir(), "no entity defs at {}; run scripts/fetch-entity-defs.sh", scripts.display());
            (f, scripts)
        })
        .collect()
}

/// All integers anywhere in a pickled value.
fn ints(v: &pickle::Value, out: &mut HashSet<i64>) {
    use pickle::Value as P;
    match v {
        P::Int(i) => {
            out.insert(*i);
        }
        P::List(xs) | P::Tuple(xs) | P::Set(xs) => xs.iter().for_each(|x| ints(x, out)),
        P::Dict(kv) => kv.iter().for_each(|(k, x)| {
            ints(k, out);
            ints(x, out)
        }),
        P::Object { args, state, .. } => {
            ints(args, out);
            if let Some(s) = state {
                ints(s, out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_entity_packet_decodes_with_sane_content() {
    for (path, scripts) in fixtures() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let replay = Replay::from_file(&path).unwrap();
        let defs = EntityDefs::load(&ScriptDir(scripts)).unwrap();
        let mut dec = EntityDecoder::new(&defs);

        let mut entity_packets = 0usize;
        let mut decoded = 0usize;
        let mut vehicles = 0usize;
        let mut rosters: Vec<HashSet<i64>> = Vec::new();
        let mut time_left: Vec<i64> = Vec::new();

        for p in Packets::new(&replay.packets, replay.meta.version().unwrap()) {
            if matches!(
                p.kind,
                Kind::EntityCreate
                    | Kind::EntityMethod
                    | Kind::EntityProperty
                    | Kind::BasePlayerCreate
                    | Kind::BasePlayerCreateStub
                    | Kind::CellPlayerCreate
            ) {
                entity_packets += 1;
            }
            let event = dec.feed(&p).unwrap_or_else(|e| panic!("{name} @ {:.2}s: {e}", p.clock));
            let Some(event) = event else { continue };
            decoded += 1;
            match event {
                Event::Create { entity: "Vehicle", properties, .. } => {
                    vehicles += 1;
                    let get = |k: &str| properties.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
                    let team = get("teamId").and_then(|v| v.as_i64()).expect("teamId");
                    assert!((0..=1).contains(&team), "{name}: teamId {team}");
                    let hp = get("health").and_then(|v| v.as_f32()).expect("health");
                    let max = get("maxHealth").and_then(|v| v.as_f32()).expect("maxHealth");
                    assert!(max > 0.0 && (0.0..=max).contains(&hp), "{name}: health {hp}/{max}");
                }
                Event::Create { entity: "Avatar", trailing, position: Some(_), .. } => {
                    assert_eq!(trailing, [0x02], "{name}: CellPlayerCreate trailer changed");
                }
                Event::Property { property: "health", value, .. } => {
                    let hp = value.as_f32().unwrap();
                    assert!(hp.is_finite() && hp >= 0.0, "{name}: health {hp}");
                }
                Event::Property { entity: "BattleLogic", property: "timeLeft", value, .. } => {
                    time_left.push(value.as_i64().unwrap());
                }
                Event::Method { method: "onArenaStateReceived", args, .. } => {
                    for (arg, v) in &args {
                        if *arg == "playersStates" || *arg == "botsStates" {
                            let Value::Blob(b) = v else { panic!("{name}: {arg} is not a blob") };
                            let roster = pickle::from_slice(b).unwrap_or_else(|e| panic!("{name}: {arg}: {e}"));
                            let pickle::Value::List(entries) = roster else { panic!("{name}: {arg} not a list") };
                            for entry in &entries {
                                let mut s = HashSet::new();
                                ints(entry, &mut s);
                                rosters.push(s);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        assert_eq!(decoded, entity_packets, "{name}: some entity packets produced no event");
        assert!(vehicles >= replay.meta.vehicles.len(), "{name}: {vehicles} vehicles created");
        // timeLeft counts the pre-battle countdown down to 0, then restarts at
        // the battle's time limit. Any other increase means a mis-decode.
        for w in time_left.windows(2) {
            if w[1] > w[0] {
                assert_eq!((w[0], w[1]), (0, i64::from(replay.meta.duration)), "{name}: timeLeft jumped");
            }
        }
        assert!(time_left.len() > 100, "{name}: only {} timeLeft updates", time_left.len());
        // Every vehicle in the arena info has a roster entry holding both its
        // avatar id and its ship's GameParams id.
        for v in &replay.meta.vehicles {
            assert!(
                rosters.iter().any(|r| r.contains(&v.id) && r.contains(&v.ship_id)),
                "{name}: no roster entry for avatar {} / ship {}",
                v.id,
                v.ship_id
            );
        }
    }
}
