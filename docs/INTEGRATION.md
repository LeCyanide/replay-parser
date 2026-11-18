# Integration brief: replay videos in the bot and the suite

A handover for a cloud session. Written 2026-11-17. Read this first, then
`README.md` (what the video shows, CLI flags), `docs/DEPLOY.md` (host setup)
and `bindings/node/README.md` (the Node API, including a discord.js handler
to start from).

## Where things stand (2026-11-18, evening)

Sections 1 and 2 are done and live; what is left is the open-task table at
the end.

| Repo | State |
| --- | --- |
| `LeCyanide/replay-parser` (this) | **Public** since 2026-11-18 (GPL-3.0). `main` at 14a4c36; CI keeps the Linux binary as the `wows-render-linux` artifact. |
| `LeCyanide/wows-suite-bot` (private, clan bot) | `/render` merged (#1) and live on the VPS, registered in Tactics Haven. |
| `LeCyanide/sye-wows-bot` (public, self-hosted) | `/render` merged (#1): optional, the Docker image builds `wows-render` from this repo; relicensed GPL-3.0 because it carries `bindings/node`. |
| `LeCyanide/tactics-suite` | Cut-over (#8) and battle panel (#9) merged into `main`; the sidecar pins this repo at 7855113. |
| `LeCyanide/fw-suite.de` | Replay Renderer page live at projects.fw-suite.de/replays/ (the site moved to fw-suite.de + projects.fw-suite.de the same day). |

Game files never go into any repo.

## 1. Bot: `/render` (done)

Kept for the reasoning behind the bot's choices.

Goal: a member attaches a `.wowsreplay`, the bot answers with the MP4.

Decisions already made by the owner:
- Input is a single `.wowsreplay` attachment, **not a zip**. Replays are
  almost never over 5 MB; refuse anything over ~10 MB before downloading.
- Default look: grid on, ×32 speed, 1080×720, ≤ 8 MB.

Steps:
1. Get `@tactics/wows-render` into the bot. It is private and not on npm. The
   bot bundles with esbuild, so a `file:` dependency on a checkout works
   (`"@tactics/wows-render": "file:../replay-parser/bindings/node"`), or vendor
   its `src/index.ts` (one file, no dependencies) into the bot under
   `src/render/`. Vendoring keeps the bot's build self-contained on the VPS;
   pick that unless there's a reason not to, and note the source commit.
2. `src/commands/render.ts`: start from the handler in
   `bindings/node/README.md`. Add: extension and size check before fetching;
   `deferReply` at once; a queue of at most 1–2 renders at a time
   (renders take 4–14 s and ~2 cores); `timeoutMs` well under Discord's
   15-minute interaction window; map `RenderError.code` to short messages
   (`unsupported_version` should say which versions the host has, via
   `supportedVersions(dataDir)`).
3. Config: `WOWS_RENDER_BIN` and `WOWS_DATA_DIR` in `.env.example` with
   comments, read in `src/config.ts`. Without them the command answers
   "rendering is not set up" instead of crashing.
4. Add it to `COMMANDS` in `src/commands/index.ts`; document it in the
   bot's README command table.
5. Test without Discord: the wrapper's tests use a fake binary
   (`bindings/node/test/`); do the same for the handler's error mapping and
   the size/extension checks. The bot's `npm run selftest` and `typecheck`
   must pass.
6. Deploy notes for the owner (do not deploy): build `wows-render` per
   `docs/DEPLOY.md` (Docker or `cargo build --release -p
   wows-replay-render` on Debian/Ubuntu), put it in `/usr/local/bin`, and
   copy the data directory to e.g. `/srv/wows-data` (read-only). The
   systemd unit has `ProtectSystem=strict`: reading those paths is fine, but
   check nothing writes outside `/srv/wows-bot/data` (the wrapper uses
   stdin/stdout, so it shouldn't).

What a cloud session cannot do here: build the data pack (needs the game),
or run a real Discord. It can fetch the public test replays and 15.9-or-older
definitions with `scripts/fetch-fixtures.sh` and render those end to end on
Linux.

## 2. Suite (1–3 done, 4 parked)

1. Open a PR in `tactics-suite` for `claude/keen-hawking-k8h8pf` (the sidecar
   cut-over; its commit message lists what was checked). Before merging,
   switch `sidecar/Cargo.toml` from the sibling path to a git dependency
   pinned to a commit of this repo:
   `wows-replay-parser = { git = "https://github.com/LeCyanide/replay-parser", rev = "<sha>" }`,
   and drop the sibling-checkout check in `scripts/build-sidecar.mjs`. The
   sidecar is built on Windows (GNU toolchain by default).
2. Its event feed (`ReplayEventsPanel`) can take death causes from
   `replay_parser::constants` instead of its own table.
3. Keep the two sidebars alike: a change to the suite's "Battle" panel should
   be mirrored in `render/src/sidebar.rs` here, and the other way round.
4. Optional, ask the owner first: an "Export video" action in the replay
   viewer that runs `wows-render` on the open replay. It would ship a second
   binary and needs the data pack on the user's machine, so it is a product
   decision, not a given.

## Ground rules

- Smallest change that does the job; each repo keeps its own style (read its
  README and recent commits).
- Never commit game files (defs, GameParams, maps, `battle_hud` icons) or
  other players' replays.
- One branch and PR per repo; don't merge without the owner.

## Progress (2026-11-18, cloud session)

Owner decisions taken this round: `/render` is open to every member and
answers publicly (refusals before the download stay private); the suite PR
comes from `claude/festive-einstein-a1jjmn`; "Export video" (§2.4) is not
built now.

### Done

- **Bot `/render`**: [LeCyanide/wows-suite-bot#1](https://github.com/LeCyanide/wows-suite-bot/pull/1),
  merged and deployed 2026-11-18. Steps 1–5 above:
  - wrapper vendored as `src/render/wows-render.ts` from `bindings/node`
    (8466aba, unchanged at 7855113). One change: a constructor parameter
    property became a field, because the bot's `npm run dev` runs `.ts`
    through Node's type stripping, which cannot erase it. Keep
    `bindings/node/src/index.ts` free of parameter properties, enums and
    namespaces so it can be copied over as is.
  - queue: 1 running, 3 waiting, 1 per member, 150 s timeout per render.
    The worst case is 10 min, inside Discord's 15-minute window.
  - the CLI's `unsupported_version` message holds the server's data path,
    so the bot extracts only the version from it. If that message's wording
    changes, the bot's regex (`failureMessage` in `src/render/replay.ts`)
    must follow.
  - typecheck clean, selftest 105/105 (8 new, fake binary). End to end
    without Discord: 7 fixtures through the bot's download → queue →
    wrapper path with the real release binary, 2.2–6.2 MB, 4.5–14 s each.
    This was measured in the cloud container, not on the VPS.
  - live on the VPS (`/usr/local/bin/wows-render` from the CI artifact,
    data pack in `/srv/wows-data` with 15.9 and 15.10). Measured there: about
    10 s and 65 MB peak per render on 1 core, so the queue stays at one.
- **Public bot `/render`**: [LeCyanide/sye-wows-bot#1](https://github.com/LeCyanide/sye-wows-bot/pull/1),
  merged. Same code as the clan bot, but optional: registered globally only
  when `WOWS_RENDER_BIN` and `WOWS_DATA_DIR` are set and the data folder has
  `defs/`. Docker builds `wows-render` from this repo at a pinned commit
  (bump `REPLAY_PARSER_REF` in its Dockerfile to follow `main`). Each host
  builds its own data pack with `scripts/build-data.sh`. The copied wrapper
  is GPL-3.0, so that bot is GPL-3.0 now too. Selftest 33/33 and two real
  15.10 replays end to end; its Docker build and a live self-hosted Discord
  are untested.
- **Suite §2.1**: [LeCyanide/tactics-suite#8](https://github.com/LeCyanide/tactics-suite/pull/8),
  merged 2026-11-18 together with #9. It carries keen-hawking's cut-over plus one commit: a git
  dependency at rev 7855113 (main), `sidecar/.cargo/config.toml` with
  `net.git-fetch-with-cli = true` (needed while this repo was private;
  harmless now), no sibling check,
  and docs for bumping `rev` or patching in a local checkout. Built and
  probed on Linux; the Windows GNU build is untested.

### Open tasks

| # | Task | Owner | Notes |
| --- | --- | --- | --- |
| 1 | ~~Install on the VPS, `npm run register`, try `/render`~~ | done 2026-11-18 | Registered in Tactics Haven only. To add it to another server (e.g. Western Approaches Command), run `npm run register` with that server's `DISCORD_GUILD_ID` |
| 2 | ~~Build the sidecar on Windows~~ | done 2026-11-18 | GNU toolchain, 15 s, 1.8 MB |
| 3 | ~~Suite §2.2 / #9: bake a real replay, check the panel, review~~ | done 2026-11-18 | Merged. Not checked inside Electron (the IPC hop) |
| 4 | ~~Suite §2.3 sidebar parity~~ | done in suite#9 | Keep `render/src/sidebar.rs` and the suite's `ReplayEventsPanel` in step |
| 5 | ~~Measure on the VPS~~ | done 2026-11-18 | ~10 s, 65 MB peak, 1 core: queue stays `RenderQueue(1, 3)` |
| 6 | Data pack after every patch (15.11 next) | owner, each patch | `scripts/build-data.sh` with the version key, copy to `/srv/wows-data` on the VPS. If the patch needs parser changes: push them, let CI pass, install that run's `wows-render-linux` artifact (`docs/DEPLOY.md`), and bump `REPLAY_PARSER_REF` in sye-wows-bot's Dockerfile |
| 7 | Suite §2.4 "Export video" | parked | Product decision; not built |
| 8 | Kill icons in the suite: copy `gui/battle_hud/icon_frag/*.png` into the suite's `seed/battle_hud/icon_frag/` | owner | Still only the folder's README on `main`; the panel shows words until then. The suite's notices say it redistributes no game files; reword them |
| 9 | ~~Name placement with collision checks~~ | dropped | Names sit under the ship, overlaps allowed; division mates in team colour |
| 10 | Bump the suite's sidecar `rev` from 7855113 to 14a4c36 | owner / next suite release | Picks up division mates in team colour (`src/draw.rs`); nothing else in the parser changed since |
| 11 | Suite selftest: 2 failures against a real bake | later | Test-fixture artefacts, not bugs: the frame count (round vs floor+1) and a hard-coded Des Moines name |
| 12 | Unsupported-version reply shows doubled brackets, e.g. "(13.0.0 (build 7))" | later | `failureMessage` in both bots' `src/render/replay.ts` |
| 13 | Clan bot README still has the "port `/render` to the public bot" to-do | later | Done in sye-wows-bot#1; drop the paragraph |
| 14 | Check sye-wows-bot's Docker build and `/render` on a self-hosted copy | owner | Needs Docker and a test server; not run here |

**3. Death causes in the suite (done, suite#9).** `ReplayEventsPanel` (and its
`causeLabel` table, toolkit `DeathCause` names) exists only on the suite's
`prototype` branch. That branch's toolkit-based sidecar writes an `events`
header block (kills with cause, roster, scores, caps, winner). The cut-over
sidecar (keen-hawking, now suite#8) writes no `events` block, so the panel
gets nothing on top of it. Adopting `replay_parser::constants` therefore
means:
(a) the sidecar emits `events` from the parser's world, with causes as
`DEATH_REASON_NAME` strings via `constants::death_reason` (and the build's
own table where `battle.xml` is available);
(b) `causeLabel` maps those names (TORPEDO, TBOMB, BURNING, FLOOD, RAM,
BOMB, SKIP_BOMB, ROCKET, DBOMB, ADBOMB, DETONATE, ATBA, SEA_MINE, …);
(c) `prototype` merges with the cut-over.
The owner chose a separate PR: [LeCyanide/tactics-suite#9](https://github.com/LeCyanide/tactics-suite/pull/9)
(branch `claude/festive-einstein-a1jjmn-events`, based on #8's branch) merges
`prototype`, adds `sidecar/src/events.rs` for (a), and updates `causeLabel`
with the same words as `kill_verb` in `render/src/sidebar.rs` for (b). Keep
the two in step. The fixture test passes (7 replays). A real bake and the
panel in the app are untested. Bakes made on `prototype` with the toolkit
keep toolkit cause names, which now read "sank"; delete them to rebake.

**4. Parity (done, suite#9).** Owner decision: the video sidebar
(`render/src/sidebar.rs`) is the reference, and the suite follows it within
its fixed sidebar. The suite now has health bars, the banded recording
player, faded and struck-through sunk ships, a "Battle events" feed with
past events only, newest at the bottom, on the countdown clock, and caps
in label order. The Playback panel's clock is also the countdown. The
suite keeps on purpose: theme colours, trail toggles and seek, kill words
instead of the game's kill icons, the map, mode and clock in the Playback
panel, and wrapped event lines. The sidecar added `timeLimit`,
`roster[].isSelf` and a `health` track to `header.events`. A change to
either side should be mirrored in the other.

**Wrecks and kill icons (2026-11-18).** Owner decisions: wrecks keep their
name in grey on both maps (video b155e76: placed after live names, dropped
where it does not fit; suite#9: under the wreck, following the name
toggles). Kill icons ship in the suite's seed rather than being read from
the install. Copying them in is the owner's step (task 8).
