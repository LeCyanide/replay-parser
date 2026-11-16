#!/usr/bin/env bash
# Fetches the sample replays the tests run against into tests/fixtures/replays/.
#
# They come from landaire's wows-toolkit test fixtures (MIT), pinned to one
# commit. They are not committed here: they are other players' battles, and the
# tests only need them present locally. Integration tests that need them fail
# with a pointer to this script when they are missing.
set -euo pipefail
REV=1c1ef3ed37b39924322b7a9c6ce1812b48e536fc
BASE="https://raw.githubusercontent.com/landaire/wows-toolkit/$REV/crates/wows-replay-insights/tests/fixtures/replays"
FILES=(
  1852_1736026132_20250104_225447_PRSD103-Derzky_04_Archipelago.wowsreplay
  1003_1780609677_20260604_172336_PASD008-Benson-1945_52_Britain.wowsreplay
  10000_1736020571_20250101_215420_PVSB018-Ipiranga_41_Conquest.wowsreplay
  10000_1767829698_20260107_182737_PJSB018-Yamato-1944_50_Gold_harbor.wowsreplay
  10000_1773241614_20260214_215653_PVSB719-Valparaiso_42_Neighbors.wowsreplay
  1005_1751197418_20250629_132500_PHSC010-Utrecht_54_Faroe.wowsreplay
  1148_1782049673_20260619_175139_PRSB909-East-Navarin_44_Path_warrior.wowsreplay
)
DIR="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures/replays"
mkdir -p "$DIR"
for f in "${FILES[@]}"; do
  [ -s "$DIR/$f" ] && continue
  echo "fetching $f"
  curl -fsSL --retry 3 -o "$DIR/$f.part" "$BASE/$f"
  mv "$DIR/$f.part" "$DIR/$f"
done
echo "fixtures in $DIR"
