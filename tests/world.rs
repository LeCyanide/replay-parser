//! Battle world over the fixture replays. Needs both fetch scripts.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;

use replay_parser::Replay;
use replay_parser::defs::EntityDefs;
use replay_parser::defs::ScriptDir;
use replay_parser::entity::EntityDecoder;
use replay_parser::entity::Event;
use replay_parser::packet::Packets;
use replay_parser::value::Value;
use replay_parser::world::World;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn on_map(xs: &[f32]) -> bool {
    xs.iter().all(|c| c.is_finite() && c.abs() < 1500.0)
}

fn median(mut xs: Vec<f32>) -> f32 {
    xs.sort_by(f32::total_cmp);
    xs[xs.len() / 2]
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

    let (mut total_squadrons, mut total_torpedoes, mut total_aimed_hits) = (0usize, 0usize, 0usize);
    let (mut yaw_errors, mut minimap_only) = (Vec::new(), 0usize);
    let mut patrols = 0usize;
    // (consumable type, from the 15.4+ blob) -> distinct work times (f32 bits).
    let mut consumable_times: BTreeMap<(i64, bool), HashSet<u32>> = BTreeMap::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let replay = Replay::from_file(&path).unwrap();
        let v = replay.meta.version().unwrap();
        let scripts = root().join("scripts").join(format!("{}_{}_{}", v.major, v.minor, v.patch));
        assert!(scripts.is_dir(), "no entity defs at {}; run scripts/fetch-entity-defs.sh", scripts.display());
        let defs = EntityDefs::load(&ScriptDir(scripts)).unwrap();
        let mut dec = EntityDecoder::new(&defs);
        let mut world = World::new(&replay.meta);
        let mut cap_progress_seen = false;
        let (mut squadrons, mut torpedoes, mut shells) = (0usize, 0usize, 0usize);
        // How far dead reckoning drifts before each torpedo sync.
        let mut torpedo_drift = Vec::new();
        // Shell hits at the aim point: elapsed time over predicted flight time.
        let mut shell_timing = Vec::new();
        let mut squadron_owners = HashSet::new();
        // Seconds enemy ships spend [in AoI, out of AoI] x [spotted, not].
        let mut enemy_time = [[0f32; 2]; 2];
        let blob_era = (v.major, v.minor) >= (15, 4);

        for p in Packets::new(&replay.packets, v) {
            let event = dec.feed(&p).unwrap();
            let dt = p.clock - world.clock;
            if let Some(own) = world.own_team()
                && dt > 0.0
            {
                for s in world.ships.values().filter(|s| s.alive && s.team != own) {
                    enemy_time[usize::from(!s.in_aoi)][usize::from(!s.spotted())] += dt;
                }
            }
            match &event {
                Some(Event::Method { method: "receive_wardRemoved", args, .. }) => {
                    let id = args[0].1.as_i64().unwrap() as u64;
                    if let Some(w) = world.patrols.get(&id) {
                        assert!(p.clock >= w.added, "{name}: patrol removed before added: {w:?}");
                        patrols += 1;
                    }
                }
                Some(Event::Method { method: "onConsumableUsed", entity_id, args, .. }) => {
                    let left = args[1].1.as_f32().unwrap();
                    world.apply(&dec, &p, event.as_ref());
                    let ship = &world.ships[entity_id];
                    let &(kind, until) = ship.consumables.last().unwrap();
                    assert!((until - world.clock - left).abs() < 1e-3 && ship.consumable_active(kind, world.clock));
                    consumable_times.entry((kind, blob_era)).or_default().insert(left.to_bits());
                    continue;
                }
                _ => {}
            }
            if let Some(Event::Method { method, args, .. }) = &event {
                let arg = |i: usize| args.get(i).map(|(_, v)| v);
                match *method {
                    "receiveTorpedoSynchronization" => {
                        let (owner, shot) = (arg(0).unwrap().as_i64().unwrap(), arg(1).unwrap().as_i64().unwrap());
                        let Some(Value::Vector3(at)) = arg(2) else { panic!("{name}: sync pos") };
                        if let Some(t) =
                            world.torpedoes.iter().find(|t| i64::from(t.owner) == owner && t.shot_id == shot)
                        {
                            let q = t.position_at(world.clock);
                            torpedo_drift.push((q[0] - at[0]).hypot(q[2] - at[2]));
                        }
                    }
                    "receiveShotKills" => {
                        let Some(Value::List(packs)) = arg(0) else { panic!("{name}: kills") };
                        for pack in packs {
                            let owner = pack.get("ownerID").unwrap().as_i64().unwrap();
                            let Some(Value::List(kills)) = pack.get("kills") else { continue };
                            for k in kills {
                                let shot = k.get("shotID").unwrap().as_i64().unwrap();
                                let Some(Value::Vector3(at)) = k.get("pos") else { continue };
                                let Some(s) =
                                    world.shells.iter().find(|s| i64::from(s.owner) == owner && s.shot_id == shot)
                                else {
                                    continue;
                                };
                                let aimed = (s.to[0] - s.from[0]).hypot(s.to[2] - s.from[2]);
                                let hit = (at[0] - s.from[0]).hypot(at[2] - s.from[2]);
                                if aimed > 100.0 && (hit - aimed).abs() < 0.03 * aimed {
                                    shell_timing.push((p.clock - s.launched) / (s.arrives - s.launched));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            world.apply(&dec, &p, event.as_ref());

            // Fresh minimap sightings of ships whose real position is also
            // fresh agree with it to the quantization step; heading also
            // drifts with turn rate over the gap, so it is checked in bulk.
            if matches!(&event, Some(Event::Method { method: "updateMinimapVisionInfo", .. })) {
                for (id, m) in world.minimap.iter().filter(|(_, m)| m.at == world.clock) {
                    match world.ships.get(id) {
                        Some(s) if s.in_aoi && world.clock - s.seen < 0.3 => {
                            let d = (m.position[0] - s.position.x).hypot(m.position[1] - s.position.z);
                            let turn = (m.yaw - s.yaw).rem_euclid(std::f32::consts::TAU);
                            let dyaw = turn.min(std::f32::consts::TAU - turn);
                            assert!(d < 2.5 && dyaw < 0.2, "{name} @ {}: sighting {m:?} vs ship {s:?}", world.clock);
                            yaw_errors.push(dyaw);
                        }
                        Some(s) if s.in_aoi => {}
                        _ => minimap_only += 1,
                    }
                }
            }

            for s in world.ships.values() {
                let pos = s.position;
                assert!(on_map(&[pos.x, pos.z]), "{name}: ship off the map at {pos:?}");
                assert!(s.health >= 0.0 && s.health <= s.max_health, "{name}: health {}/{}", s.health, s.max_health);
            }
            for c in world.caps.values() {
                assert!((0.0..=1.0).contains(&c.progress), "{name}: cap progress {}", c.progress);
                cap_progress_seen |= c.progress > 0.0;
            }
            for s in world.smokes.values() {
                assert!(!s.points.is_empty() && s.radius > 0.0, "{name}: empty smoke");
            }
            for s in world.squadrons.values() {
                // The owner may not have entered the area of interest yet.
                squadron_owners.insert(s.owner);
                assert!(on_map(&s.position), "{name}: squadron off the map");
            }
            for t in &world.torpedoes {
                assert!(on_map(&t.position_at(world.clock)), "{name}: torpedo off the map at {}: {t:?}", world.clock);
            }
            squadrons = squadrons.max(world.squadrons.len());
            torpedoes = torpedoes.max(world.torpedoes.len());
            shells = shells.max(world.shells.len());
        }

        // Every arena-info player is linked to exactly one ship, on team 0/1,
        // and allies share the recording player's team.
        let mut linked = HashSet::new();
        for (i, pl) in world.players.iter().enumerate() {
            let ships: Vec<_> = world.ships.values().filter(|s| s.player == Some(i)).collect();
            assert_eq!(ships.len(), 1, "{name}: player {} has {} ships", pl.name, ships.len());
            assert!((0..=1).contains(&ships[0].team), "{name}: team {}", ships[0].team);
            linked.insert(i);
        }
        let team_of = |rel: u32| {
            let i = world.players.iter().position(|p| p.relation == rel)?;
            world.ships.values().find(|s| s.player == Some(i)).map(|s| s.team)
        };
        let own = team_of(0).unwrap_or_else(|| panic!("{name}: no recording player"));
        for (i, pl) in world.players.iter().enumerate() {
            let team = world.ships.values().find(|s| s.player == Some(i)).unwrap().team;
            assert_eq!(team == own, pl.relation < 2, "{name}: {} relation {} on team {team}", pl.name, pl.relation);
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
        assert!(world.ships.values().any(|s| s.seen > world.clock - 60.0), "{name}: positions stopped");
        assert_eq!(world.scores.len(), 2, "{name}: scores {:?}", world.scores);
        assert!(world.winner.is_some(), "{name}: no battle result");
        assert!(!world.caps.is_empty() && cap_progress_seen, "{name}: caps never moved");
        // Squadron owners (plane id low 32 bits) are ships of this battle,
        // allowing the few that never enter the recording player's view.
        let unknown = squadron_owners.iter().filter(|o| !world.ships.contains_key(o)).count();
        assert!(
            unknown * 4 <= squadron_owners.len(),
            "{name}: {unknown} of {} squadron owners unknown",
            squadron_owners.len()
        );
        assert!(shells > 0, "{name}: no shells");
        total_squadrons += squadrons;
        total_torpedoes += torpedoes;

        // Enemies are flagged spotted while in view and not out of it.
        let [[in_spotted, in_hidden], [out_spotted, out_hidden]] = enemy_time;
        let in_share = in_spotted / (in_spotted + in_hidden);
        let out_share = out_spotted / (out_spotted + out_hidden).max(1.0);
        assert!(in_share > 0.9 && out_share < 0.02, "{name}: spotted {in_share} in view, {out_share} out");
        for w in world.patrols.values() {
            assert!(w.radius > 0.0 && on_map(&w.position), "{name}: bad patrol {w:?}");
        }

        // Dead reckoning stays within a few world units between syncs.
        if !torpedo_drift.is_empty() {
            let worst = torpedo_drift.iter().copied().fold(0.0, f32::max);
            assert!(median(torpedo_drift.clone()) < 1.0 && worst < 10.0, "{name}: torpedo drift up to {worst}");
        }
        // Shells landing at their aim point arrive close to the predicted time.
        if shell_timing.len() >= 20 {
            let m = median(shell_timing.clone());
            assert!((0.95..1.05).contains(&m), "{name}: shells land at {m} of predicted flight");
        }
        total_aimed_hits += shell_timing.len();
    }
    // Not every battle has carriers or torpedo boats, but the corpus does.
    assert!(total_squadrons > 0 && total_torpedoes > 0, "no squadrons or torpedoes in any fixture");
    assert!(total_aimed_hits > 500, "only {total_aimed_hits} aimed shell hits checked");
    assert!(yaw_errors.len() > 10_000, "only {} minimap sightings checked", yaw_errors.len());
    // Half a 2π/256 step from rounding, plus turning between messages.
    let m = median(yaw_errors);
    assert!(m < 0.02, "minimap yaw off by {m} rad (median)");
    assert!(minimap_only > 100, "only {minimap_only} minimap-only sightings");
    assert!(patrols > 0, "no fighter patrol was removed");
    // The 15.4+ consumable blob means the same types as the old id: every
    // type used at least 3 times in both eras shares a work time across them.
    let mut compared = 0;
    for (&(kind, blob), times) in consumable_times.iter().filter(|((_, blob), _)| *blob) {
        let Some(old) = consumable_times.get(&(kind, !blob)) else { continue };
        if times.len() >= 3 && old.len() >= 3 {
            assert!(!times.is_disjoint(old), "consumable {kind}: no shared work time across eras");
            compared += 1;
        }
    }
    assert!(compared >= 3, "only {compared} consumable types compared across eras");
}
