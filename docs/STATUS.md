# Status and open tasks

Last updated: 2026-11-17.

## Goal

One Rust framework for World of Warships replays: parse → simulate →
draw commands → frames → MP4. The suite's sidecar uses the first layers;
the Discord bot (TypeScript) uses all of them through `wows-render` and
`bindings/node`. The bot's output is a ~30 s MP4: the minimap with a battle
sidebar after the suite's replay "Battle" panel (score, caps, lineups,
events).

## Done in this round (cloud)

- **Renderer** (`render/`): map layer in the suite's replay-layer style
  (class glyphs from the suite, caps with capture arcs, smoke, torpedoes,
  tracers, planes, health bars, buildings, weather, consumable and patrol
  circles, names); sidebar with map/mode/clock header, score bars and cap
  rings, both lineups ordered by class with health bars and sunk state, and a
  kill/capture event feed. Embedded font: Inter (SIL OFL 1.1,
  `render/assets/Inter-OFL.txt`).
- **Timing**: the pre-battle countdown is skipped; the battle up to a few
  seconds after the result is sampled evenly into the video (×15–×37 on the
  fixtures for 30 s); the final frame is held for 1.5 s.
- **Video**: H.264 MP4, built into the binary (no ffmpeg at runtime). *In
  progress*: `render/src/encode.rs` is a placeholder until one of two
  independently built encoders is chosen; `--frames` (PNG stills) works.
- **CLI** `wows-render` with JSON progress/error lines; error codes
  `unsupported_version`, `bad_replay`, `missing_data`, `io`, `internal`.
- **Node wrapper** `bindings/node` (`@tactics/wows-render`): `renderReplay`,
  `supportedVersions`, typed `RenderError`; 22 tests against a fake binary.
- **Data pack**: `replay-dump datapack` writes a build's entity definitions
  and a compact GameParams extract (ships, projectiles, aircraft, buildings,
  drops, English ship names) so rendering does not load the 1.2 GB GameParams
  pickle. The renderer loads it as JSON.
- **Parser**: `World::win_score` (`teamWinScore`); GameParams types are
  deserializable; one clippy fix; 15.9 entity defs added to the fixture fetch.
- **Tests**: `render/tests/render.rs` renders all 7 fixtures (frame count,
  battle span, frames change, sidebar painted), loads a data pack, and checks
  the unsupported-version error. Workspace: 34 Rust tests pass, clippy clean.

## Blocked here, needs a game install (🖥)

1. **15.10 entity definitions.** The public source (Monstrofil/replays_unpack,
   pinned and latest as of 2026-11-06) stops at 15.9, and 15.9's definitions
   do not fit 15.10: on the supplied Wichita replay (15.10.0, build 13533378)
   23,128 of 72,447 entity packets fail (new Avatar and Vehicle methods and
   properties shift the indices). `wows-render` therefore refuses it with
   `unsupported_version`. Fix: on a machine with the game,
   `replay-dump datapack <game dir> <data dir> 13533378 15_10_0`, then render
   the Wichita replay and check it.
2. **GameParams extract.** Same command; without it ships show player names
   and cruiser marks. Not run here: the datapack command is untested against
   a real install (its output format is tested via a hand-made pack).
3. **Run the install tests** (`WOWS_DIR=… cargo test --release -- --ignored`)
   after this round's parser changes.

## Open tasks (not started or partial)

- **Deployment of the bot**: build `wows-render` for the bot host's
  platform, keep a data directory per supported game version, and refresh it
  on every game patch (datapack + suite map seeds). A CI job or script for
  this does not exist yet.
- **Patch cadence**: each game patch needs a new datapack before the bot can
  render its replays. Consider falling back to the nearest older definitions
  only if decode failures stay near zero (not implemented: today it refuses).
- **Fonts**: Inter covers Latin, Cyrillic and Greek. CJK/Thai/Arabic player
  names render with gaps where glyphs are missing. A fallback font would add
  several MB; decide whether it is worth it.
- **Death causes** in the event feed: every kill reads "sank". The death
  reason codes are version-specific and not mapped yet (needs the game's
  constants per version).
- **Label overlap** on crowded maps: names are not de-collided.
- **Sidebar density** for large battles (Operations, 12 v 12 plus bots): the
  lineup rows shrink to fit, events show only the newest that fit.
- **Discord upload limit**: check output size against the server's limit in
  the bot; the CLI has no size target option yet.
- **Suite parity**: the suite's sidebar (`ReplayEventsPanel`) is on the
  suite's `prototype` branch; if it changes, mirror it in `render/src/sidebar.rs`.
- **Node addon**: the wrapper spawns the CLI (simple, isolates crashes). An
  in-process napi-rs addon would save process start-up only; not needed now.
- **Legacy versions**: pre-12.6 replays are still unverified (no fixture).

## Notes

- `WOWS_RENDER_DEBUG=1` prints the detected battle span.
- Map images and ship data belong to Wargaming; the repo stays private, and
  game files are never committed (fixtures and defs are fetched, the data
  directory lives outside the repo).
- The suite's map seeds (`seed/maps/maps.json`, 760 px images) are what the
  renderer uses for backgrounds; `spaceSize` there sets the map scale.
