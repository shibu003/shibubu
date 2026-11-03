#!/bin/bash
# build.sh — 試験の側の Swift の道具を bin/ に compile して署名する（W2b-2・§8 の ③'）。製品の build-mac.sh とは別。
#
# 呼ぶ側の使い方: SHIBUBU_SIGN_ID="<② の自己署名の id>" bash scripts/qa/bg/build.sh [front-guard|windows|wincap|axprobe]
#   → 引数なしは bin/front-guard・bin/windows・bin/wincap・bin/axprobe。引数 1 つはその道具だけ
#     （どれも swiftc -parse-as-library lib/Records.swift <tool>.swift・wincap は W12W-1 の probe 5・axprobe は W12W-1c）
# 保証する: SHIBUBU_SIGN_ID があればその id で codesign、無ければ ad-hoc（`--sign -`）に落ち、その事を stderr に 1 行出す
#   （ad-hoc は build のたびに TCC の許可が消える）。同じ id で build し直しても Accessibility の許可が残るかは AXIsProcessTrusted で見る（未測）。
# 保証しない: 許可（Accessibility・画面収録）を与える事（owner の手・③'）。
# exit: 0 ok / 1 compile か署名の失敗

set -eu
BG="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$BG/bin"
only="${1:-}"
ID="${SHIBUBU_SIGN_ID:-}"
[ -n "$ID" ] || { echo "build.sh: SHIBUBU_SIGN_ID が無いので ad-hoc で署名する（build のたびに Accessibility の許可が消える）" >&2; ID=-; }
built=0
for t in front-guard:front-guard.swift windows:oracle/windows.swift wincap:wincap.swift axprobe:axprobe.swift; do
  name=${t%%:*}; src=${t#*:}
  if [ -n "$only" ] && [ "$name" != "$only" ]; then continue; fi
  swiftc -O -parse-as-library "$BG/lib/Records.swift" "$BG/$src" -o "$BG/bin/$name"
  codesign --force --sign "$ID" --identifier "ai.shibubu.qa.$name" "$BG/bin/$name"
  built=1
done
if [ -n "$only" ] && [ "$built" -eq 0 ]; then
  echo "build.sh: 知らない道具 $only" >&2
  exit 1
fi
if [ -n "$only" ]; then
  echo "build.sh: ok $BG/bin/$only (sign: $ID)"
else
  echo "build.sh: ok $BG/bin/front-guard $BG/bin/windows $BG/bin/wincap $BG/bin/axprobe (sign: $ID)"
fi
