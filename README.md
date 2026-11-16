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
   Version, entity create/enter/leave/control, and method/property envelopes
   with raw argument bytes.
3. **BigWorld type decoder** ☁ to write (synthetic tests), 🖥 to validate.
   `INT*/UINT*/FLOAT*/STRING/BLOB/VECTOR*`, `ARRAY`, `TUPLE`, `FIXED_DICT`
   (incl. `AllowNone`), `USER_TYPE`, `PYTHON` (pickle), `MAILBOX`.
4. **Python pickle reader** ☁ — needed by both `PYTHON` args and GameParams.
5. **Game file access** 🖥 — `res_packages` `.idx/.pkg` reader, `bin/<build>`
   selection, gettext catalog for ship names.
6. **Entity definitions** 🖥 — `entities.xml`, `alias.xml`, `.def` files and
   interfaces/components; property and client-method index ordering (methods
   are ordered by argument stream size, the main source of silent drift after
   a patch). Turns step 2's raw envelopes into named calls.
7. **GameParams** 🖥 — ships, artillery/torpedo/plane params, consumables, the
   subset the simulation and labels need.
8. **Battle world** 🖥 — the largest piece: ship state (position/heading
   interpolation, health, death, visibility/spotting), shells and tracer
   timing, torpedoes, squadrons, smoke, capture points, buildings, weather,
   consumable radii, the recording player's camera.
9. **Draw-command emitter** 🖥 — world state → the existing `format.rs` record
   tags (`SHIP`, `HEALTH_BAR`, `SHOT_TRACER`, `TORPEDO`, `SMOKE`, `PLANE`,
   `CAPTURE_POINT`, …).
10. **Parity harness** ☁ to write, 🖥 to run — bake one replay with the current
    sidecar and the new one, diff the two `.wtreply` files per frame and per
    record with tolerances. This is the acceptance test for cutting over.
11. **Patch survey** ☁ — a command that reports unknown wire ids, undecodable
    payloads and entity-def drift for a new build, so a patch is diagnosed in
    minutes. (`replay-dump stats` is the start of it.)
12. **Cut-over** — sidecar depends on this crate; drop `vendor/` fetching,
    `sidecar/patches/`, and the toolkit's licence entries; keep credit for the
    format knowledge.

Order of work: 3 → 4 in the cloud; then 5 → 6 → 7 locally, which unlock real
decoding; then 8 → 9 one record tag at a time, each checked with 10.

## Usage

```bash
cargo build --release
./target/release/replay-dump meta    <file.wowsreplay>   # arena info JSON
./target/release/replay-dump stats   <file.wowsreplay>   # packet counts, decode failures
./target/release/replay-dump packets <file.wowsreplay>   # one JSON line per packet
```

## Tests

```bash
./scripts/fetch-fixtures.sh
cargo test
```

The fixtures are seven replays from 13.11 to 15.5, taken from the toolkit's own
test fixtures at a pinned commit. They are not committed (other players'
battles); the fixture tests fail, rather than pass empty, without them. On
every fixture, every packet frames and every engine-fixed payload decodes to
exactly its length, the stream ends with the `0xffffffff` trailer, and the
Version and Map packets agree with the arena info.

No pre-12.6 replay is in the corpus, so the legacy wire-id table is unverified.

## Credits

The container layout, cipher key and wire-id tables follow what landaire's
wows-toolkit (MIT) documents; the code here is a separate implementation.
