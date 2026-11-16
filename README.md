# replay-parser

A World of Warships replay parser for the Tactics Suite replay viewer, written
to replace the suite's dependency on landaire's
[wows-toolkit](https://github.com/landaire/wows-toolkit) crates.

## Scope: what is being replaced

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
   Not yet: interpolation, naming the spotting reasons, secondary shells, buildings (no
   fixture has any), weather. Torpedoes that never get a kill message are
   dropped past 1.3 × their GameParams range (`World::with_params`), or at
   150 s without GameParams.
9. **Draw-command emitter** 🖥 — world state → the existing `format.rs` record
   tags (`SHIP`, `HEALTH_BAR`, `SHOT_TRACER`, `TORPEDO`, `SMOKE`, `PLANE`,
   `CAPTURE_POINT`, …).
10. **Parity harness** — done, in the suite: `npm run diff-timeline -- old.wtreply
    new.wtreply`. Running it needs bakes from both sidecars (🖥).
11. **Patch survey** ☁ — `replay-dump stats` (packet level) and
    `replay-dump entities` (entity level, first failure per method/property)
    already report what a new build breaks.
12. **Cut-over** — sidecar depends on this crate; drop `vendor/` fetching,
    `sidecar/patches/`, and the toolkit's licence entries; keep credit for the
    format knowledge.

Next: buildings in 8 (none in the fixtures;
Operations replays have them), then 9 one record tag at a time, each
checked with 10.

## Usage

```bash
cargo build --release
./target/release/replay-dump meta    <file.wowsreplay>   # arena info JSON
./target/release/replay-dump stats   <file.wowsreplay>   # packet counts, decode failures
./target/release/replay-dump packets <file.wowsreplay>   # one JSON line per packet
./target/release/replay-dump entities <file.wowsreplay> <scripts dir>   # entity decode coverage
./target/release/replay-dump events   <file.wowsreplay> <scripts dir>   # one JSON line per entity event
./target/release/replay-dump world    <file.wowsreplay> <scripts dir>   # one world snapshot per game second
./target/release/replay-dump res      <game dir> [path]                 # list packaged files, or extract one
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
