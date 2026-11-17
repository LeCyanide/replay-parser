# Integration brief: replay videos in the bot and the suite

A handover for a cloud session. Written 2026-11-17. Read this first, then
`README.md` (what the video shows, CLI flags), `docs/DEPLOY.md` (host setup)
and `bindings/node/README.md` (the Node API, including a discord.js handler
to start from).

## Where things stand

| Repo | State |
| --- | --- |
| `LeCyanide/replay-parser` (this) | Parser, `wows-render` CLI, MP4 encoder, Node wrapper (`bindings/node`, `@tactics/wows-render`) on `main`. PR 2 (`claude/render-cosmetics`, render looks) open; merge it first or branch from it. |
| `LeCyanide/wows-tactics-bot` | discord.js 14, Node, TypeScript bundled with esbuild to `dist/bot.js`, SQLite, systemd on a VPS behind Caddy. **No replay code yet.** Commands live in `src/commands/` (one file each, listed in `src/commands/index.ts`, registered with `npm run register`). |
| `LeCyanide/tactics-suite` | Electron app. Its replay viewer bakes replays with a Rust sidecar (`sidecar/`). The cut-over of that sidecar to this parser is on branch `claude/keen-hawking-k8h8pf` (commit 530a14f), pushed, **no PR yet**; `main` still uses the old toolkit crates. |

Verified locally on 2026-11-17: `replay-dump datapack` against a live install
built the 15.10 data pack (build 13533378), and a 15.10 Random battle
rendered with it (706 frames, 5 MB). Game files never go into any repo.

## 1. Bot: `/render` (do this first)

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

## 2. Suite

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
