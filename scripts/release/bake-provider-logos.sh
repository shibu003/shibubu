#!/bin/bash
# PROVIDER-ICONS-1101: provider / agent の brand の logo の SVG（@lobehub/icons-static-svg 1.95.1・MIT）を 64 px の PNG に焼く（build の外で 1 回・足す時だけ回す）。
# 使い方: bash scripts/release/bake-provider-logos.sh <SVG の dir（<key>.svg）>  → assets/provider-logos/<key>.png
# 焼くのは macOS の sips（qlmanage は src/audio/surface.rs の注のとおりこの Mac で 120 秒返らなかった）。width / height の 1em を 64 にしてから焼く（1em のままだと 1 px）。
# currentColor の logo（字の色で塗る形）は白で焼き、描く時に字の色を掛ける（domain::icon::logo_png の mono）。
set -eu
src=${1:?SVG の dir}
out=$(cd "$(dirname "$0")/../.." && pwd)/assets/provider-logos
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for svg in "$src"/*.svg; do
  key=$(basename "$svg" .svg)
  sed -e 's/height="1em"/height="64"/' -e 's/width="1em"/width="64"/' -e 's/currentColor/#fff/g' "$svg" > "$tmp/$key.svg"
  sips -s format png "$tmp/$key.svg" --out "$out/$key.png" >/dev/null
  echo "$key $(sips -g pixelWidth "$out/$key.png" | awk '/pixelWidth/{print $2}')px $(grep -c currentColor "$svg" || true)"
done
