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
- **Video** (`encode/`, crate `wows-replay-encode`): H.264 Main profile,
  yuv420p BT.709, MP4 with `moov` first, no ffmpeg at runtime (Cisco
  OpenH264 built from source + a hand-written muxer). Measured on the
  fixtures at 1080×720, 30 fps, 30 s: 3.2–3.5 MB, 15–18 s per replay in
  total (about 7–8 s of it encoding), decodes with zero ffmpeg errors.
- **Encoder choice** (built twice, independently): both used OpenH264 and a
  hand-written muxer; on the same 900 real frames of two replays, version A
  (own GOP-level rate control, screen-content mode) scored +1.4 dB PSNR over
  version B (OpenH264's rate control) at equal or lower size; B needed ~50%
  more bytes for similar quality but encoded 2.2× faster (3.5 s vs 7–8 s). A
  was kept: file size against Discord's upload limit matters more than ~5 s.
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
  the unsupported-version error, and encodes a short MP4. Workspace: 41 Rust
  tests pass (including the encoder's ffprobe/ffmpeg checks), clippy clean.

## Blocked here, needs a game install (🖥)

1. **15.10 entity definitions.** The public source (Monstrofil/replays_unpack,
   pinned and latest as of 2026-11-06) stops at 15.9, and 15.9's definitions
   do not fit 15.10: on the supplied Wichita replay (15.10.0, build 13533378)
   23,128 of 72,447 entity packets fail (new Avatar and Vehicle methods and
   properties shift the indices). `wows-render` therefore refuses it with
   `unsupported_version`. The Cleveland replay supplied as 15.9 is also
   15.10.0, build 13533378, by its own metadata (12,678 of 46,871 fail). Fix: on a machine with the game,
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

- **Encoder build/runtime**: OpenH264 is C++, so the binary links
  `libstdc++.so.6` dynamically (present on Debian/Ubuntu images, not on
  distroless/musl); build with `nasm` installed, or encoding is ~2× slower
  (the build does not warn). Cisco's H.264 patent coverage applies to its
  prebuilt binaries only, not to builds from source; check this before any
  public distribution.
- **Encode speed**: encoding runs after each frame on one thread; running it
  on a second thread would overlap rendering and encoding (~40% faster).

## Notes

- `WOWS_RENDER_DEBUG=1` prints the detected battle span.
- Map images and ship data belong to Wargaming; the repo stays private, and
  game files are never committed (fixtures and defs are fetched, the data
  directory lives outside the repo).
- The suite's map seeds (`seed/maps/maps.json`, 760 px images) are what the
  renderer uses for backgrounds; `spaceSize` there sets the map scale.
