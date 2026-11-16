//! Checks against a local game install (🖥). Opt in with
//!
//!     WOWS_DIR=<install> cargo test --release -- --ignored
//!
//! Uses the install's own `replays/` folder: every replay recorded by a build
//! that is still installed is decoded against that build's files.

use std::path::Path;
use std::path::PathBuf;

use replay_parser::Replay;
use replay_parser::defs::EntityDefs;
use replay_parser::entity::EntityDecoder;
use replay_parser::packet::Packets;
use replay_parser::res::InstallScripts;
use replay_parser::res::Resources;
use replay_parser::world::World;

fn install() -> PathBuf {
    let dir = std::env::var_os("WOWS_DIR").expect("set WOWS_DIR to a World of Warships install");
    let dir = PathBuf::from(dir);
    assert!(dir.join("bin").is_dir(), "{} has no bin/", dir.display());
    dir
}

fn replays(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            replays(&p, out);
        } else if p.extension().is_some_and(|x| x == "wowsreplay") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs WOWS_DIR"]
fn every_installed_build_reads_its_files() {
    let dir = install();
    let builds = replay_parser::res::builds(&dir).unwrap();
    assert!(!builds.is_empty());
    for build in builds {
        let res = Resources::open_build(&dir, build).unwrap();
        // Every script and every map's space.settings inflates and passes
        // its CRC32 (read checks both).
        let paths: Vec<String> = res
            .paths()
            .filter(|p| p.starts_with("scripts/") || p.ends_with("/space.settings"))
            .map(str::to_owned)
            .collect();
        assert!(paths.len() > 100, "build {build}: only {} scripts and maps", paths.len());
        for p in &paths {
            res.read(p).unwrap_or_else(|e| panic!("build {build}: {p}: {e}"));
        }
        assert!(res.read("content/GameParams.data").unwrap().len() > 1_000_000);
        let defs = EntityDefs::load(&InstallScripts(&res)).unwrap();
        assert!(defs.entities.iter().any(|e| e.name == "Vehicle"), "build {build}: no Vehicle def");
        let names = res.catalog("en").unwrap();
        assert_eq!(names.get("IDS_PJSD207").map(String::as_str), Some("Shiratsuyu"));
    }
}

#[test]
#[ignore = "needs WOWS_DIR"]
fn installed_builds_decode_their_replays() {
    let dir = install();
    let builds = replay_parser::res::builds(&dir).unwrap();
    let mut files = Vec::new();
    replays(&dir.join("replays"), &mut files);
    files.sort();

    let mut checked = 0;
    for build in builds {
        let res = Resources::open_build(&dir, build).unwrap();
        let defs = EntityDefs::load(&InstallScripts(&res)).unwrap();
        let names = res.catalog("en").unwrap();
        for path in &files {
            let Ok(replay) = Replay::from_file(path) else { continue };
            let v = replay.meta.version().unwrap();
            if v.build != build {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut dec = EntityDecoder::new(&defs);
            let mut world = World::new(&replay.meta);
            let pve = replay.meta.game_type.as_deref().is_some_and(|t| t.contains("PVE"));
            for p in Packets::new(&replay.packets, v) {
                let event = dec.feed(&p).unwrap_or_else(|e| panic!("{name} @ {:.2}s: {e}", p.clock));
                world.apply(&dec, &p, event.as_ref());
            }
            for (i, pl) in world.players.iter().enumerate() {
                let n = world.ships.values().filter(|s| s.player == Some(i)).count();
                // Allies are always in view; an enemy the recording player
                // never saw (one 15.9 tournament replay saw none) has no ship.
                // In PvE an arena-info bot is a template that can spawn several
                // ships, or none yet; only the recording player is fixed.
                let want = match (pve, pl.relation) {
                    (_, 0) | (false, 1) => 1..=1,
                    (false, _) => 0..=1,
                    (true, _) => 0..=usize::MAX,
                };
                assert!(want.contains(&n), "{name}: player {} (relation {}) has {n} ships", pl.name, pl.relation);
            }
            // The file name carries the recording player's ship index
            // ("…_PJSD207-Shiratsuyu_…"), which the catalogue names.
            if let Some(index) = name.split('_').nth(2).and_then(|s| s.split('-').next()) {
                assert!(names.contains_key(&format!("IDS_{index}")), "{name}: no name for {index}");
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no replay in {} matches an installed build", dir.join("replays").display());
    eprintln!("{checked} replays decoded against their installed build");
}
