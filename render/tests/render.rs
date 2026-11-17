//! Renders the fixture replays (see the root README for fetching them and
//! their entity definitions). Fails without them, as the parser's tests do.

use std::path::Path;
use std::path::PathBuf;

use replay_render::DataDir;
use replay_render::RenderOptions;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// A data dir over the fixture defs; `extra` adds GameParams packs.
fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wows-render-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gameparams")).unwrap();
    let scripts = root().join("tests/fixtures/scripts");
    assert!(scripts.is_dir(), "run scripts/fetch-entity-defs.sh first");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&scripts, dir.join("defs")).unwrap();
    dir
}

fn fixture(part: &str) -> Vec<u8> {
    let dir = root().join("tests/fixtures/replays");
    let f = std::fs::read_dir(&dir)
        .expect("run scripts/fetch-fixtures.sh first")
        .filter_map(|e| Some(e.ok()?.path()))
        .find(|p| p.to_string_lossy().contains(part))
        .unwrap_or_else(|| panic!("no fixture matching {part}"));
    std::fs::read(f).unwrap()
}

fn small() -> RenderOptions {
    RenderOptions { width: 600, height: 360, fps: 4, duration: 5.0, hold_end: true }
}

#[test]
fn renders_every_fixture() {
    let data = DataDir::new(data_dir("all"));
    let dir = root().join("tests/fixtures/replays");
    let mut n = 0;
    for f in std::fs::read_dir(&dir).expect("run scripts/fetch-fixtures.sh first") {
        let path = f.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        let mut frames = Vec::new();
        let summary = replay_render::scene::render(&bytes, &data, &small(), &mut |_| {}, &mut |pm| {
            frames.push(pm.clone());
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(summary.frames, 20, "{}", path.display());
        assert_eq!(frames.len(), 20);
        // The battle is sped up to fit, and the first frame is in battle.
        assert!(summary.battle_seconds > 300.0, "{}: {}", path.display(), summary.battle_seconds);
        assert!(summary.speed > 10.0);
        // Frames change over the battle, and the sidebar is painted.
        assert_ne!(frames[1].data(), frames[15].data(), "{}", path.display());
        let side = frames[10].pixel(500, 300).unwrap();
        assert!(side.red() < 40 && side.blue() < 60, "{}: sidebar background", path.display());
        n += 1;
    }
    assert_eq!(n, 7);
}

#[test]
fn game_data_names_ships() {
    // A hand-made extract in `replay-dump datapack`'s format.
    let dir = data_dir("gp");
    let bytes = fixture("Valparaiso");
    let replay = replay_parser::Replay::from_bytes(&bytes).unwrap();
    let species = ["Destroyer", "Cruiser", "Battleship", "AirCarrier", "Submarine"];
    let mut ships = serde_json::Map::new();
    let mut names = serde_json::Map::new();
    for (i, v) in replay.meta.vehicles.iter().enumerate() {
        let index = format!("PXSX{i:03}");
        ships.insert(
            v.ship_id.to_string(),
            serde_json::json!({
                "id": v.ship_id, "index": index, "name": format!("{index}_Test"), "species": species[i % 5],
                "nation": "x", "level": 10, "consumables": [], "secondary_ammo": []
            }),
        );
        names.insert(format!("IDS_{index}"), format!("Ship {i}").into());
    }
    let pack = serde_json::json!({
        "build": 0,
        "gameparams": { "ships": ships, "projectiles": {}, "aircraft": {}, "buildings": {}, "drops": {} },
        "names": names,
    });
    let v = replay.meta.version().unwrap();
    std::fs::write(dir.join(format!("gameparams/{}_{}_{}.json", v.major, v.minor, v.patch)), pack.to_string()).unwrap();
    let data = DataDir::new(&dir);
    let gd = data.game_data(&v).unwrap().expect("pack found by version");
    let unique: std::collections::HashSet<i64> = replay.meta.vehicles.iter().map(|v| v.ship_id).collect();
    assert_eq!(gd.gameparams.ships.len(), unique.len());
    let summary = replay_render::scene::render(&bytes, &data, &small(), &mut |_| {}, &mut |_| Ok(())).unwrap();
    assert!(summary.warnings.iter().all(|w| !w.contains("GameParams")), "{:?}", summary.warnings);
}

#[test]
fn unknown_version_is_reported() {
    let dir = std::env::temp_dir().join(format!("wows-render-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err =
        replay_render::scene::render(&fixture("Faroe"), &DataDir::new(&dir), &small(), &mut |_| {}, &mut |_| Ok(()))
            .err()
            .expect("no defs");
    assert_eq!(err.code(), "unsupported_version");
}

#[test]
fn rejects_odd_sizes() {
    let o = RenderOptions { width: 1081, ..RenderOptions::default() };
    assert!(o.validate().is_err());
    assert!(RenderOptions::default().validate().is_ok());
}
