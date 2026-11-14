#!/usr/bin/env bash
# Generate two demo landscapes and render them.
# Usage: cargo build --release -p vf-cli && ./scripts/demo.sh [output-dir]
set -euo pipefail

BIN="${VISTAFORGE_BIN:-target/release/vistaforge}"
OUT="${1:-demo-out}"

if [[ ! -x "$BIN" ]]; then
  echo "error: $BIN not found; run: cargo build --release -p vf-cli" >&2
  exit 1
fi

mkdir -p "$OUT"

echo "== Modern alpine =="
"$BIN" terrain generate \
  --mode modern --size 514 --seed 7 --preset alpine \
  --name "Alpine demo" --out "$OUT/alpine.vfproj"
"$BIN" render "$OUT/alpine.vfproj" "$OUT/alpine.png" \
  --quality high --width 1600 --height 900 --json

echo "== Classic island =="
"$BIN" terrain generate \
  --mode classic --size 514 --seed 42 --island \
  --name "Island demo" --out "$OUT/island.vfproj"
"$BIN" render "$OUT/island.vfproj" "$OUT/island.png" \
  --quality high --width 1600 --height 900 --json

echo "== Project report =="
"$BIN" project info "$OUT/alpine.vfproj" --json

echo "done: $OUT/alpine.png, $OUT/island.png"
