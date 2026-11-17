# replay-parser

A World of Warships replay framework in Rust, independent of landaire's
[wows-toolkit](https://github.com/landaire/wows-toolkit): it parses replays,
simulates the battle, and renders it. Its users:

- the **Tactics Suite** replay viewer, whose sidecar bakes timelines with the
  parser crate (draw commands; the suite paints them);
- the **Discord bot**, which turns a replay into a ~30 s MP4 of the minimap
  with a battle sidebar, through `wows-render` and its Node wrapper;
- anything else, through the library crates.

| Part | Path | What it is |
|---|---|---|
| `wows-replay-parser` | `src/` | container, packets, entity decoding, game files, GameParams, battle world, draw commands; `replay-dump` CLI |
| `wows-replay-render` | `render/` | frames (map layer + sidebar) and MP4; `wows-render` CLI |
| `wows-replay-encode` | `encode/` | RGBA frames to H.264 MP4 (OpenH264 from source, no ffmpeg) |
| `@tactics/wows-render` | `bindings/node/` | TypeScript wrapper that runs `wows-render` for the bot |

Open tasks and notes: [`docs/STATUS.md`](docs/STATUS.md). Deploying for the
bot: [`docs/DEPLOY.md`](docs/DEPLOY.md).

## Rendering a replay

Building needs a C/C++ compiler, and `nasm` for the encoder's fast paths.

```bash
cargo build --release -p wows-replay-render
./target/release/wows-render replay.wowsreplay --out battle.mp4 --data <data dir>
```

Options: `--speed 32` (default; the video lasts the battle / 32, at most
90 s) or `--duration <s>` (fixed length), `--max-mb 8` (size cap; the bitrate
follows from it, error `too_large` if it cannot be met), `--fps 30`,
`--width 1080 --height 720` (square map at full height, sidebar in the
rest), `--progress json` (machine-readable progress and errors on stderr; see
`bindings/node/README.md`), `-` for stdin/stdout, `--frames <dir>
[--every N]` to write PNG stills instead of a video, `--no-hold` to skip the
held result frame, and `--no-grid` to leave the 1–10 / A–J grid off the map.

### What the video shows

The map (left, square):

- the map image with a faded 10×10 grid (1–10 across, A–J down), as the
  in-game minimap;
- ships as class icons in the team colour (allies `#4CE8AA`, enemies
  `#FE4D2A`), heading-rotated, with a fading trail; ships out of sight
  faded, wrecks where they sank; the recording player's ship slightly
  larger with a white ring;
- a gold ring outside the icon on allies (and the recording player) while
  the enemy spots them;
- names in white, orange while the player is disconnected; placed so they
  don't overlap; health bars above each ship (green, amber, red);
- shell streaks in yellow, the front of each in the ammo's colour (AP
  white, HE orange, SAP red); torpedoes as team-coloured dots;
- planes as the game's minimap markers (`<data>/battle_hud/`; team-coloured
  shapes per squadron type without them), fighter patrol circles;
- radar, hydro and similar as tinted circles; smoke; capture points with
  the capturing team's progress arc; weather, buffs and buildings;
- on the held final frame, a Victory / Defeat / Draw banner with the score.

The sidebar (right):

- map, mode, team sizes, player and ship; the battle clock counting down;
  the playback speed;
- both scores against the win score, and each cap's owner and progress;
- both lineups, sorted CV, BB, CA, DD, SS, with health; sunk ships struck
  through, the recording player in bold;
- battle events, newest at the bottom: kills with the game's kill-type icon
  between killer and victim (the cause in words without the icons) and cap
  changes, timed by the battle clock.

The data directory:

```text
<data>/defs/<key>/            entity definitions (scripts/: entities.xml, entity_defs/) — required
<data>/gameparams/<key>.json  GameParams extract and ship names — optional (without it: player names, cruiser marks)
<data>/maps/maps.json         map images and sizes, in the suite's seed/maps format — optional (plain sea without)
<data>/fonts/                 fallback fonts for names Inter lacks (CJK, …) — optional
<data>/battle_hud/            the client's gui/battle_hud/ plane markers and kill icons — optional (drawn shapes and words without)
```

`<key>` is the replay's build number or its version (`15_10_0`). The
definitions, the GameParams extract and the `battle_hud/` icons come from a
game install:

```bash
./target/release/replay-dump datapack <game dir> <data dir> [build] [15_10_0]
```

The suite's `seed/maps/` directory works as `<data>/maps` as it is.
`scripts/build-data.sh` does both for a new game build. The
data directory holds game files: keep it out of the repository.

## Scope: what the parser replaced in the suite

The suite already draws replays itself (`src/canvas/ReplayLayer.tsx`). What
it takes from landaire is everything upstream of drawing, inside the Rust
sidecar (`tactics-suite/sidecar`):

| Sidecar step | Upstream crate today |
|---|---|
| Read `.wowsreplay`: decrypt, inflate | `wows_replays` |
| Frame and decode packets with the game's entity definitions | `wows_replays`, `wowsunpack` |
| Read game files: `.idx/.pkg`, `GameParams`, entity defs, `space.settings` | `wowsunpack` |
| Simulate the battle (ships, shells, planes, smoke, caps, visibility) | `wows-battle-world`, `wows-replay-insights` |
| Turn world state into draw commands | `wows_minimap_renderer` |

The `.wtreply` format (`sidecar/src/format.rs`) and the renderer stay as they
are. The target is a sidecar with the same `probe` / `bake` / `caps` CLI and
the same output, built on this crate instead.

## Roadmap

☁ = can be built and verified with no game install (cloud/CI).
🖥 = needs a local game install (and its exact build) to build or verify.

1. **Container** ☁ — done. Arena-info JSON, extra blocks, Blowfish/XOR-chain
   decryption, zlib, truncated-stream recovery. `probe` needs only this.
2. **Packet framing + engine-fixed payloads** ☁ — done. Version-keyed wire-id
   table (layout shift at 12.6.0), Position, PlayerOrientation, Camera, Map,
   Version, entity create/enter/leave/control, and method/property envelopes.
3. **BigWorld type decoder** ☁ — done (`value`). Verified on real packets:
   a `USER_TYPE` is a length-prefixed blob on the wire whatever inner type it
   declares.
4. **Python pickle reader** ☁ — done (`pickle`). Matches CPython on a
   279-case corpus across protocols 0–5 and Python 2 opcodes.
5. **Game file access** 🖥 — done (`res`). Reads a build's `idx/*.idx`
   and inflates files from `res_packages/` (CRC32-checked), loose
   `bin/<build>/res/` first, `res_mods/` ignored; picks `bin/<build>` from
   the replay's build number; reads the gettext catalogue
   (`IDS_<ship index>` → ship name). Step 6 reads the defs straight from the
   install through it (`InstallScripts`).
6. **Entity definitions + entity decoding** ☁ — done (`defs`, `entity`,
   `nested`) from a loose `scripts/` directory or the install. Methods, properties, creates and nested property updates decode
   against tracked per-entity state; see Tests for what is checked.
7. **GameParams** 🖥 — *started* (`gameparams`): `content/GameParams.data`
   is a zlib stream written back to front holding a protocol-2 pickle of
   `{region: {name: GPData}}`; region `""` is the base. Extracted: ships (id,
   index for the catalogue name, species, nation, tier), projectiles
   (species; torpedo `maxDist` and `speed`) and aircraft (species). Loading
   takes about 2 s and 1.2 GB peak (15 M values; the pickle reader shares
   memoized values). Torpedo `maxDist` is in world units: torpedoes that
   run out on the local 15.10 replays die at a median 0.998 × `maxDist`
   (max 1.25, upgrades). Consumables per ship (every slot and
   alternative: type, work time, radius); the type and its parameters are
   in the ability's variant, under `logic`. Radii, in world units: radar,
   hydroacoustic search and submarine locator `distShip`, fighters
   `radius`. The replay's numeric consumable id is mapped to the type by a
   table fitted on local replays (`consumable_type`); `World::consumable_radius`
   gives a linked ship's radius. Checked: 97.6% of enemies inside an active
   radar's radius are spotted, 64% just outside. Not yet: hydrophone (its
   `hydrophoneWaveRadius` looks like metres, unverified), upgrade and skill
   modifiers, the 3.5 km submarine wards.
8. **Battle world** — the largest piece: ship state (position/heading
   interpolation, health, death, visibility/spotting), shells and tracer
   timing, torpedoes, squadrons, smoke, capture points, buildings, weather,
   consumable radii, the recording player's camera. Much of it can be built
   and checked here on the fixtures; what needs GameParams (ranges, shell
   flight times, plane icons) waits for step 7.
   *Started* (`world`): ship position/heading, health, team, alive and
   area-of-interest; each ship's arena-info player (by its Vehicle entity
   id, which every `onArenaStateReceived` roster entry holds, for players
   and bots alike); deaths with killer and reason code; team scores, battle
   result, capture points (owner, progress, invader), smoke screens, clock
   and battle stage; minimap squadrons (the plane id's low 32 bits are the
   owning ship); torpedoes, dead-reckoned between course changes and syncs;
   shells in flight from muzzle to aim point, landing at
   `0.36 × serverTimeLeft` (measured; see Tests); minimap sightings from
   `updateMinimapVisionInfo`, including ships spotted outside the area of
   interest (minimap only). Its `packedData` is x and z in 11 bits each over
   a fixed −2500..2500, yaw in 8 bits, and bit 31 for "no position".
   Spotting: `visibilityFlags` is non-zero while the opposing team spots
   the ship (`Ship::spotted`; the bits are reasons, not yet named).
   Active consumables per ship with their end time; the type is the
   `CONSUMABLE_ID`, which from 15.4 is the second byte of a 2-byte
   `consumableUsageParams`. Fighter patrols (`receive_wardAdded`): position,
   radius, team, owner, until `receive_wardRemoved` (on the local 15.x
   replays every ward is a submarine's, kind 2, 3.5 km).
   Buildings (Operations) are kept from their create packet on, with
   `isAlive`/`isSuppressed` kept current. Not yet: interpolation, naming
   the spotting reasons. Torpedoes that never get a kill message are
   dropped past 1.3 × their GameParams range (`World::with_params`), or at
   150 s without GameParams.
9. **Draw-command emitter** 🖥 — world state → the existing `format.rs` record
   tags (`SHIP`, `HEALTH_BAR`, `SHOT_TRACER`, `TORPEDO`, `SMOKE`, `PLANE`,
   `CAPTURE_POINT`, …). *Started* (`draw`): `draw::frame` gives one frame's
   commands in the renderer's pixel space (768 px square, north up; the
   map's side from `space.settings` bounds, `(chunks − 4) × chunkSize`) with
   its colours and rules, for `CAPTURE_POINT`, `TORPEDO`, `SMOKE`, `SHIP`
   (detected from the minimap entry, else drawn at the last known minimap
   position, faded and unnamed), `HEALTH_BAR` and `DEAD_SHIP`; labels
   (player, ship and species names) from GameParams and the catalogue.
   `replay-dump draw <replay> <game dir>` prints a frame per second.
   The suite's sidecar bakes with it via `bake --engine parser` (branch
   `claude/keen-hawking-k8h8pf` of tactics-suite). Diffed against the
   toolkit bake (10) on five local 15.10 replays (two Operations, three
   randoms): same frame count, and `CAPTURE_POINT`, `SMOKE` and
   `DEAD_SHIP` match on every record;
   `SHIP` pairs on every record with positions, yaw, species, names,
   colours, the name colour (`selectedWeapon`; main guns by the shell
   from `setAmmoForWeapon`) and the disconnected flag (the roster's
   `isConnected` from `onGameRoomStateChanged`, counted from the first
   time the player is seen connected) matching, bar one frame where a
   change lands on the frame boundary (a ship's world position counts
   only once a position packet follows its create packet, the minimap
   sighting until then, as the toolkit's);
   `HEALTH_BAR` has every toolkit bar; a ship not yet in view gets a full
   bar from the arena roster's `maxHealth`, as the toolkit means to, but
   its bake gives every ship whose build it can resolve a consumable
   inventory first, which registers the ship and so skips that seeding
   (0.1–1.6% extra bars of ours on three replays);
   `TORPEDO` matches on the randoms, and differs on Operations where the
   toolkit freezes homing torpedoes that the server's own
   `receiveTorpedoDirection` positions show moving. Torpedoes launched
   with a turn (`maneuverDump`) run on `targetYaw` from launch (syncs
   agree to 0.2 units). `SHOT_TRACER` and its tip (main and secondary)
   match on every record: shells are kept 30 s like the toolkit's salvos
   and paced by flight times learned from their hits in a first pass over
   the replay. `PLANE` (icon key from the aircraft's species, subtype and
   bomb ammo; owner names except on consumable planes) and fighter
   `PATROL_RADIUS` match on every record. `CONSUMABLE_RADIUS` matches on
   two replays; on two others we draw circles the toolkit misses, both
   its gaps: the suite's toolkit bake builds its consumable id table with
   `GameConstants::from_vfs`, not `for_build`, so it keeps the pre-15.2
   ids and reads 15.10's id 42 (submarine locator) as a plane smoke
   generator; and it has no radius for an Operations bot's radar (bots
   have no roster entry, and it only looks at their equipped abilities,
   taking whichever variant a `HashMap` yields first). `CAMERA_DIRECTION`
   (the low byte of `targetLocalPos` updates) matches on every record.
   `BUILDING` matches on every record of the Operation (none of its
   buildings dies; the toolkit never updates a building after its create
   packet, so ours would differ once one is destroyed, showing it dead).
   `BUFF_ZONE` (InteractiveZones with no control point, other than
   weather; icon from the drop named in BattleLogic `state.drop.data`) and
   `WEATHER_ZONE` (the toolkit's one ordered list fed by
   `state.weather.localWeather` changes and weather zone entities, which
   move by their position packets) match on every record of three replays
   (Atoll and an Operation with both, Greece with a moving storm).
   Division mates are gold (the roster's `prebattleId`, at a
   version-specific index; not in clan battles) on ships, tracers,
   torpedoes and camera lines, matching on every record. Wrecks stay where
   the ship died (its position if in view, else its minimap sighting),
   matching on every record of all seven replays. Two Greece differences
   are the toolkit's: it reads depth states from the game's
   `DEPTH_STATE` table, which on 15.10 stops at 3, so a submarine at
   state 4 (deep) keeps its last label, `(Scope)`, where we show
   `(60m*)`; and it multiplies each `receiveTorpedoDirection` speed factor
   into the torpedo's current speed and ignores `receiveTorpedoManeuverEnd`
   and `receiveTorpedoSynchronization`, so homing torpedoes crawl and then
   stand still (ours land within a world unit of their server hit
   position; this is also Atoll's torpedo difference).
   Interpolation between packets is not needed for parity: the suite's
   toolkit bake sets a render clock but installs no position timeline, so
   the toolkit draws each ship at its latest packet, as we do (the suite's
   viewer interpolates between frames itself).
10. **Parity harness** — done, in the suite: `npm run diff-timeline -- old.wtreply
    new.wtreply`. Running it needs bakes from both sidecars (🖥).
11. **Patch survey** ☁ — `replay-dump stats` (packet level) and
    `replay-dump entities` (entity level, first failure per method/property)
    already report what a new build breaks.
12. **Cut-over** — done: the sidecar's `probe`, `bake` and `caps` run on
    this crate alone (no `--engine`; no toolkit crates, `vendor/` fetching or
    patches), and the licence notice became a credit for the format
    knowledge. Bakes are byte-identical to the last `--engine parser` ones,
    and the `.wtreply` format is unchanged, so toolkit-era bakes still load.
    Comparing against the toolkit now needs a toolkit-era sidecar binary.

Next: the toolkit's gaps in 9 are ours to fix or keep now that parity is
no longer the target; new game builds go through 11.

## Usage

```bash
cargo build --release
./target/release/replay-dump meta    <file.wowsreplay>   # arena info JSON
./target/release/replay-dump stats   <file.wowsreplay>   # packet counts, decode failures
./target/release/replay-dump packets <file.wowsreplay>   # one JSON line per packet
./target/release/replay-dump entities <file.wowsreplay> <scripts dir>   # entity decode coverage
./target/release/replay-dump events   <file.wowsreplay> <scripts dir>   # one JSON line per entity event
./target/release/replay-dump world    <file.wowsreplay> <scripts dir>   # one world snapshot per game second
./target/release/replay-dump draw     <file.wowsreplay> <game dir>      # one JSON line of draw commands per second
./target/release/replay-dump res      <game dir> [path]                 # list packaged files, or extract one
./target/release/replay-dump datapack <game dir> <data dir> [build] [key]   # what wows-render needs, see above
```

`<scripts dir>` can also be a game install directory (the one holding
`bin/`); the defs then come from the build that recorded the replay, which
must still be installed (the client keeps the current and previous build).

On Windows, build with the GNU toolchain if MSVC build tools are not
installed (`cargo +stable-x86_64-pc-windows-gnu test`), and run the
entity-defs fetch from Git Bash as
`MSYS_NO_PATHCONV=1 TMPDIR="$(cygpath -m "$PWD/target")" ./scripts/fetch-entity-defs.sh`,
since Git Bash otherwise rewrites the sparse-checkout paths.

## Tests

```bash
./scripts/fetch-fixtures.sh       # seven replays, 13.11 to 15.5
./scripts/fetch-entity-defs.sh    # the client's scripts/ for those versions
cargo test
```

Neither set is committed: the replays are other players' battles (taken from
the toolkit's test fixtures, MIT), and the entity defs are game files (taken
from Monstrofil/replays_unpack, which carries them per version). Both are
pinned to one commit. Tests that need them fail, rather than pass empty,
without them.

On every fixture:

- every packet frames and every engine-fixed payload decodes to exactly its
  length; the stream ends with the `0xffffffff` trailer; the Version and Map
  packets agree with the arena info;
- every entity packet — about 540,000 across the seven — decodes with no
  bytes left over, including every nested property update;
- the content cross-checks: vehicle teams are 0/1 and health is within max;
  `timeLeft` counts down (pre-battle countdown, then the time limit from the
  arena info); team scores and capture progress are in range; and every
  vehicle in the arena info appears, by avatar id and ship id, in the pickled
  rosters of `onArenaStateReceived`.

The world (`tests/world.rs`), at every packet of every fixture: ships stay
on the map with health within max, capture progress stays in 0–1, smokes
have points and a radius, squadrons and torpedoes stay on the map; at the
end, every arena-info player is linked to exactly one ship, on team 0/1,
with allies on the recording player's team and enemies not; the ships
marked dead are exactly the `receiveVehicleDeath` victims (each dying
once), killers are known ships, there are two team scores and a battle
result, a cap moved, and most squadron owners are ships of the battle.
Torpedo dead reckoning is compared with every `receiveTorpedoSynchronization`
(median drift under 1 unit, worst under 10). Shells that hit at their aim
point (about 3,400 across the fixtures) land at 0.95–1.05 of the predicted
flight time, median per replay. Every minimap sighting of a ship whose
real position arrived within 0.3 s matches it within 2.5 units (one
quantization step); headings match to a median under 0.02 rad over more
than 10,000 sightings, and over 100 sightings are of ships outside the
area of interest. Enemies of the recording player are flagged spotted
over 90% of their time in view and under 2% out of it, on every fixture.
Every consumable type used at least 3 times both before and after 15.4
shares a work time across the two encodings (at least 3 types compared).

The renderer (`render/tests/render.rs`) renders every fixture at a small
size: the right frame count, a battle span over 5 minutes sped up to fit,
frames that change over the battle, and a painted sidebar; it also loads a
GameParams data pack in `replay-dump datapack`'s format and checks the
`unsupported_version` error. These tests build their data directory with a
symlink and so run on Linux and macOS only (CI runs them); the renderer's
unit tests run everywhere. The Node wrapper has its own tests
(`cd bindings/node && npm install && npm test`), run against a fake binary.

The pickle reader is checked against CPython on `tests/pickle_corpus.json`
(regenerate with `python3 -I scripts/gen-pickle-corpus.py`).

Against a local install (opt in; not run in CI):

```bash
WOWS_DIR=/path/to/World_of_Warships cargo test --release -- --ignored
```

checks, for every installed build: every script and map `space.settings`
inflates and matches its CRC32, GameParams is present, the entity defs load,
and the catalogue names ships; then, with that build's GameParams, every
roster ship is a GameParams ship with a catalogue name, every torpedo's
params are a torpedo with a range, active radars spot what is inside
their radius, consumable ids map to types the ship carries, and every replay in the install's
`replays/` recorded by an installed build decodes with no failures against
that build's own defs, links its players (allies always; enemies at most
once; PvE bot templates may spawn several ships, or none), and the
recording player's ship index has a name. On the author's machine that is
217 replays of 15.9 and 15.10, including Operations and a tournament.

No pre-12.6 replay is in the fixture corpus, so the legacy wire-id table is
unverified; 15.6+ is covered only by the local install test above.

## Credits

The container layout, cipher key and wire-id tables follow what landaire's
wows-toolkit (MIT) documents; the code here is a separate implementation.
