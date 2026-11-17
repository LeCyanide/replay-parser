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
  seconds after the result plays at ×32 (a 20-minute battle → 37.5 s; the
  fixtures give 15–34 s), capped at 90 s by raising the speed; the final
  frame is held for 1.5 s. `--duration` fixes the length instead.
- **Size cap**: `--max-mb` (default 8 MB, under Discord's free limit) sets
  the bitrate from the video's length; if the result still exceeds it, one
  re-render at a lower bitrate follows, else the error `too_large`. On the
  fixtures at ×32 the videos are 2.0–6.0 MB.
- **Video** (`encode/`, crate `wows-replay-encode`): H.264 Main profile,
  yuv420p BT.709, MP4 with `moov` first, no ffmpeg at runtime (Cisco
  OpenH264 built from source + a hand-written muxer). Measured on the
  fixtures at 1080×720, 30 fps: 4–14 s per replay in total (encoding runs
  on its own thread, overlapping rendering: about 13.6 ms per frame against
  20.3 ms before), decodes with zero ffmpeg errors.
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
- **Packaging**: `Dockerfile` (Rust build stage → Debian slim with the
  stripped binary), `scripts/build-data.sh` (data directory from a game
  install plus the suite's map seeds), `docs/DEPLOY.md`. Tested here: the
  build stage's commands in the Rust image, and the runtime image rendering
  a fixture (valid 27 s MP4); the literal `docker build` could not run here
  (no network inside containers), CI runs it.
- **CI** (`.github/workflows/ci.yml`): format, clippy `-D warnings`, all
  tests with fetched fixtures and ffmpeg, the Node wrapper's tests, and a
  Docker build with a smoke render. Its steps pass locally; it has not run
  on GitHub yet.
- **Death causes** in the event feed (torpedoed, burned, flooded, rammed,
  bombed, secondaries, …): `replay_parser::constants`, with a default table
  checked against the replays (every torpedo death follows a torpedo hit by
  the killer, shell deaths follow shell hits) and each build's own table from
  `battle.xml` via `datapack`.
- **Map extras**: fading 40 s trails behind ships in sight, a ring around
  the recording player's ship, and a Victory/Defeat/Draw banner with the
  final score on the held last frame.
- **Fallback fonts**: fonts in `<data>/fonts/` fill in characters Inter
  lacks (CJK, Thai, …), loaded at run time so the binary stays small;
  without one such names show gaps.
- **Deterministic output**: the same replay and options give the same MP4
  bytes (tested).
- **Ship names** on the map are placed to avoid each other, ship marks and
  health bars.
- **Tests**: `render/tests/render.rs` renders all 7 fixtures (frame count,
  battle span, frames change, sidebar painted), loads a data pack, and checks
  the unsupported-version error, and encodes a short MP4. Workspace: 48 Rust
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

- **Data refresh per patch**: `scripts/build-data.sh` must be run on a
  machine with the game after each patch; it is untested against a real
  install (as is `datapack`).
- **Patch cadence**: each game patch needs a new datapack before the bot can
  render its replays. Consider falling back to the nearest older definitions
  only if decode failures stay near zero (not implemented: today it refuses).
- **Label overlap** on crowded maps: names are not de-collided.
- **Sidebar density** for large battles (Operations, 12 v 12 plus bots): the
  lineup rows shrink to fit, events show only the newest that fit.
- **Suite sidecar**: tactics-suite's `claude/keen-hawking-k8h8pf` sidecar
  (path dependency on a sibling checkout of this repo) builds unchanged
  against this branch and `probe` works (checked 2026-11-17). Its event
  feed could adopt `replay_parser::constants` for death causes; that change
  belongs in the suite repo.
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

## Notes

- `WOWS_RENDER_DEBUG=1` prints the detected battle span.
- Map images and ship data belong to Wargaming; the repo stays private, and
  game files are never committed (fixtures and defs are fetched, the data
  directory lives outside the repo).
- The suite's map seeds (`seed/maps/maps.json`, 760 px images) are what the
  renderer uses for backgrounds; `spaceSize` there sets the map scale.
