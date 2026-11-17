#!/usr/bin/env bash
# Assembles a wows-render data directory for one game build, on a machine
# with the game installed:
#
#   scripts/build-data.sh <game dir> <suite seed/maps dir> <out dir> [version key]
#
#   <game dir>     the World of Warships install (holds bin/ and res_packages/)
#   <suite maps>   tactics-suite's seed/maps (maps.json + assets/)
#   <out dir>      the data directory to create or update
#   [version key]  e.g. 15_10_0: name the entry after the version so it serves
#                  every regional build of it (default: the build number)
#
# Run it after every game patch; keep older versions' entries for old replays.
# The game files it writes belong to Wargaming: keep the data directory private.
set -euo pipefail
if [ $# -lt 3 ]; then
  sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi
GAME=$1 MAPS=$2 OUT=$3 KEY=${4:-}
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DUMP="$ROOT/target/release/replay-dump"
[ -x "$DUMP" ] || cargo build --release --manifest-path "$ROOT/Cargo.toml" --bin replay-dump
BUILD=$(ls "$GAME/bin" | grep -E '^[0-9]+$' | sort -n | tail -1)
[ -n "$BUILD" ] || { echo "no builds under $GAME/bin" >&2; exit 1; }
mkdir -p "$OUT"
"$DUMP" datapack "$GAME" "$OUT" "$BUILD" ${KEY:+"$KEY"}
# Map images and sizes, as the suite ships them.
mkdir -p "$OUT/maps"
cp "$MAPS/maps.json" "$OUT/maps/"
if [ -d "$MAPS/assets" ]; then
  mkdir -p "$OUT/maps/assets"
  cp "$MAPS/assets/"* "$OUT/maps/assets/"
fi
echo "data for build $BUILD${KEY:+ ($KEY)} in $OUT"
ls "$OUT/defs"
