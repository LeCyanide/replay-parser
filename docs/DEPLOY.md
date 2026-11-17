# Deploying wows-render for the Discord bot

The bot (TypeScript) calls the `wows-render` binary through
`bindings/node` (`@tactics/wows-render`). The host needs three things: the
binary, a data directory, and the wrapper package.

## 1. The binary

**Docker (recommended).** From the repository root:

```bash
docker build -t wows-render .
```

The image is Debian bookworm-slim plus the stripped binary (about 3.5 MB).
For a bot that runs in its own Node image, copy only the binary:

```dockerfile
FROM wows-render AS render
FROM node:22-bookworm-slim
COPY --from=render /usr/local/bin/wows-render /usr/local/bin/wows-render
```

If Docker Hub rate-limits the build (HTTP 429), pass a mirror:
`--build-arg RUST_IMAGE=mirror.gcr.io/library/rust:1-bookworm --build-arg RUNTIME_IMAGE=mirror.gcr.io/library/debian:bookworm-slim`.

**Without Docker.** `cargo build --release -p wows-replay-render` with a
C/C++ compiler and `nasm` installed. The binary needs `libstdc++6` and a
glibc at least as new as the symbols it uses (check with
`objdump -T wows-render | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1`; a
build on Ubuntu 24.04 needed 2.35 and ran on Debian 12). Building on the
oldest system you deploy to avoids surprises.

## 2. The data directory

One per host, shared by every render (mount it read-only):

```text
data/defs/<key>/            entity definitions per game version (required)
data/gameparams/<key>.json  ship names and classes (optional but expected)
data/maps/                  map images (optional; a plain grid without)
data/fonts/                 fallback fonts (optional; .ttf/.otf/.ttc)
```

The built-in font (Inter) covers Latin, Cyrillic and Greek. For CJK and
other player names, put a font that covers them in `data/fonts/`, e.g. Noto
Sans CJK (`fonts-noto-cjk` on Debian, SIL OFL); without one those characters
show as gaps. Fonts are tried in file-name order.

Build or update it on a machine with the game installed, after every game
patch:

```bash
scripts/build-data.sh "<WoWS install>" <tactics-suite>/seed/maps ./data 15_10_0
```

Name each entry after the version (`15_10_0`) so all regional builds of it
use it; keep older entries so older replays still render. Copy `data/` to the
bot host. The files are Wargaming's: keep them private.

Replays of a version with no entry fail with `unsupported_version`;
`supportedVersions(dataDir)` lists what the host can render.

## 3. The bot

```bash
cd bindings/node && npm ci && npm run build
```

Use it as in `bindings/node/README.md`. Set `WOWS_RENDER_BIN` to the binary
(or put it on `PATH`) and point `dataDir` at the data directory.

Defaults: ×32 playback (a 20-minute battle becomes a 37.5 s video), 1080×720
at 30 fps, at most 8 MB. Measured on the fixtures with the native binary:
4–14 s per replay and 2–6 MB. Renders use about one core for rendering plus
one for encoding; run one or two at a time per core pair.

## Checks before going live

- Render a current-version replay on the host and watch the video.
- `supportedVersions` lists the current game version.
- The bot replies within Discord's 15-minute interaction window even when
  renders queue (keep `timeoutMs` well below it).
