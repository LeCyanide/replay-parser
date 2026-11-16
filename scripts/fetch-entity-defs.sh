#!/usr/bin/env bash
# Fetches the client's entity definitions (scripts/*.xml, *.def) for the
# fixture replays' versions into tests/fixtures/scripts/<major>_<minor>_<patch>/.
#
# These are game files. They are not committed here; they are copied from
# Monstrofil/replays_unpack, which carries them per client version, pinned to
# one commit. On your own machine the same files come from the game install.
set -euo pipefail
REV=36e839ddc14213a797e2c0327829c5044583e7e4
VERSIONS=(13_11_0 14_5_0 14_11_0 15_1_0 15_4_0 15_5_0)
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/tests/fixtures/scripts"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
git -C "$TMP" init -q
git -C "$TMP" remote add origin https://github.com/Monstrofil/replays_unpack
git -C "$TMP" sparse-checkout set --no-cone $(printf '/replay_unpack/clients/wows/versions/%s/scripts/ ' "${VERSIONS[@]}")
git -C "$TMP" fetch -q --depth 1 --filter=blob:none origin "$REV"
git -C "$TMP" checkout -q FETCH_HEAD
mkdir -p "$DEST"
for v in "${VERSIONS[@]}"; do
  rm -rf "$DEST/$v"
  cp -r "$TMP/replay_unpack/clients/wows/versions/$v/scripts" "$DEST/$v"
done
echo "entity defs in $DEST"
