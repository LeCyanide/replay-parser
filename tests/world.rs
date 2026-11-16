//! Battle world over the fixture replays. Needs both fetch scripts.

use std::collections::HashSet;
use std::path::PathBuf;

use replay_parser::Replay;
use replay_parser::defs::EntityDefs;
use replay_parser::defs::ScriptDir;
use replay_parser::entity::EntityDecoder;
use replay_parser::entity::Event;
use replay_parser::packet::Packets;
use replay_parser::pickle;
use replay_parser::value::Value;
use replay_parser::world::World;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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
fn world_is_consistent_on_every_fixture() {
    let dir = root().join("replays");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("no fixtures at {}; run scripts/fetch-fixtures.sh", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wowsreplay"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "run scripts/fetch-fixtures.sh");

    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let replay = Replay::from_file(&path).unwrap();
        let v = replay.meta.version().unwrap();
        let scripts = root().join("scripts").join(format!("{}_{}_{}", v.major, v.minor, v.patch));
        assert!(scripts.is_dir(), "no entity defs at {}; run scripts/fetch-entity-defs.sh", scripts.display());
        let defs = EntityDefs::load(&ScriptDir(scripts)).unwrap();
        let mut dec = EntityDecoder::new(&defs);
        let mut world = World::new();
        let mut cap_progress_seen = false;
        let mut rosters: Vec<HashSet<i64>> = Vec::new();

        for p in Packets::new(&replay.packets, v) {
            let event = dec.feed(&p).unwrap();
            world.apply(&dec, &p, event.as_ref());
            if let Some(Event::Method { method: "onArenaStateReceived", args, .. }) = &event {
                for (arg, v) in args {
                    if let ("playersStates" | "botsStates", Value::Blob(b)) = (*arg, v) {
                        let pickle::Value::List(entries) = pickle::from_slice(b).unwrap() else { panic!("{name}") };
                        rosters.extend(entries.iter().map(|e| {
                            let mut s = HashSet::new();
                            ints(e, &mut s);
                            s
                        }));
                    }
                }
            }
            for s in world.ships.values() {
                let pos = s.position;
                assert!(
                    [pos.x, pos.z].iter().all(|c| c.is_finite() && c.abs() < 1500.0),
                    "{name}: ship off the map at {pos:?}"
                );
                assert!(s.health >= 0.0 && s.health <= s.max_health, "{name}: health {}/{}", s.health, s.max_health);
            }
            for c in world.caps.values() {
                assert!((0.0..=1.0).contains(&c.progress), "{name}: cap progress {}", c.progress);
                cap_progress_seen |= c.progress > 0.0;
            }
            for s in world.smokes.values() {
                assert!(!s.points.is_empty() && s.radius > 0.0, "{name}: empty smoke");
            }
        }

        // Every arena-info vehicle is a ship on team 0/1, linked through the
        // roster entry that holds both its arena-info id and the ship's owner.
        for m in &replay.meta.vehicles {
            let entry = rosters.iter().find(|r| r.contains(&m.id) && r.contains(&m.ship_id));
            let entry = entry.unwrap_or_else(|| panic!("{name}: no roster entry for {}", m.id));
            let ship = world.ships.values().find(|s| entry.contains(&s.owner));
            let ship = ship.unwrap_or_else(|| panic!("{name}: no ship for player {}", m.id));
            assert!((0..=1).contains(&ship.team), "{name}: team {}", ship.team);
        }
        // Deaths and the alive flags agree, and each ship dies at most once.
        let victims: HashSet<u32> = world.deaths.iter().map(|d| d.victim).collect();
        assert_eq!(victims.len(), world.deaths.len(), "{name}: a ship died twice");
        let dead: HashSet<u32> = world.ships.iter().filter(|(_, s)| !s.alive).map(|(id, _)| *id).collect();
        assert_eq!(dead, victims, "{name}: isAlive and receiveVehicleDeath disagree");
        assert!(
            world.deaths.iter().all(|d| world.ships.contains_key(&d.killer) || d.killer == 0),
            "{name}: unknown killer"
        );
        // Positions keep coming for most of the battle.
        assert!(world.ships.values().any(|s| s.seen > world.clock - 60.0), "{name}: positions stopped");
        assert_eq!(world.scores.len(), 2, "{name}: scores {:?}", world.scores);
        assert!(world.winner.is_some(), "{name}: no battle result");
        assert!(!world.caps.is_empty() && cap_progress_seen, "{name}: caps never moved");
    }
}
