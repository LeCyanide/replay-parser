//! Runs the parser over real replays. Fetch them first with
//! `scripts/fetch-fixtures.sh`; without them these tests fail rather than pass
//! vacuously.

use std::path::PathBuf;

use replay_parser::Replay;
use replay_parser::packet::Decoded;
use replay_parser::packet::Kind;
use replay_parser::packet::Packets;

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replays");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("no fixtures at {}; run scripts/fetch-fixtures.sh", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wowsreplay"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .wowsreplay in {}; run scripts/fetch-fixtures.sh", dir.display());
    files
}

#[test]
fn every_fixture_frames_and_decodes_cleanly() {
    for path in fixtures() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let replay = Replay::from_file(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(replay.stream_complete, "{name}: zlib stream did not end cleanly");
        let version = replay.meta.version().unwrap();

        let mut packets = Packets::new(&replay.packets, version);
        let mut seen_version = None;
        let mut seen_map = None;
        let mut positions = 0usize;
        let mut last_clock = f32::MIN;
        let mut ended = false;
        for p in packets.by_ref() {
            assert!(!ended, "{name}: packets after the stream-end marker");
            if p.kind == Kind::StreamEnd {
                ended = true;
                continue;
            }
            assert!(p.clock.is_finite() && p.clock >= 0.0, "{name}: bad clock {}", p.clock);
            assert!(p.clock + 1e-3 >= last_clock, "{name}: clock went backwards {} -> {}", last_clock, p.clock);
            last_clock = p.clock;

            let decoded = p.decode().unwrap_or_else(|e| panic!("{name} @ {:.2}s: {e}", p.clock));
            match decoded {
                Decoded::Version { version } => seen_version = Some(version.to_owned()),
                Decoded::Map { name: map, .. } => seen_map = Some(map.to_owned()),
                Decoded::Position { position, rotation, .. } => {
                    positions += 1;
                    for v in [position.x, position.y, position.z, rotation.yaw] {
                        assert!(v.is_finite(), "{name}: non-finite position/rotation");
                    }
                    // Largest WoWS maps are ~48 km across, i.e. ±2400 BigWorld units.
                    assert!(position.x.abs() < 3000.0 && position.z.abs() < 3000.0, "{name}: position {position:?}");
                }
                _ => {}
            }
        }
        assert!(ended, "{name}: no stream-end marker");
        assert_eq!(packets.trailing(), 0, "{name}: stream has bytes past the last whole packet");

        // The Version packet carries "major,minor,patch,0"; the meta's build
        // number is not in it.
        let sv = seen_version.unwrap_or_else(|| panic!("{name}: no Version packet"));
        let expected = format!("{},{},{},0", version.major, version.minor, version.patch);
        assert_eq!(sv, expected, "{name}: Version packet disagrees with arena info");
        assert_eq!(seen_map.as_deref(), Some(replay.meta.map_name.as_str()), "{name}: Map packet disagrees");
        assert!(positions > 1000, "{name}: only {positions} Position packets");
        // The stream should run for most of the recorded battle.
        assert!(last_clock > 300.0, "{name}: stream ends at {last_clock}s");
    }
}

#[test]
fn modern_fixtures_use_the_modern_id_layout() {
    for path in fixtures() {
        let replay = Replay::from_file(&path).unwrap();
        let version = replay.meta.version().unwrap();
        let kinds: std::collections::HashSet<Kind> = Packets::new(&replay.packets, version).map(|p| p.kind).collect();
        for k in [
            Kind::EntityCreate,
            Kind::EntityMethod,
            Kind::EntityProperty,
            Kind::Position,
            Kind::PlayerOrientation,
            Kind::Camera,
        ] {
            assert!(kinds.contains(&k), "{}: no {k:?}", path.display());
        }
    }
}
