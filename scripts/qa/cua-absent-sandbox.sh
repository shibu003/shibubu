#!/bin/bash
# W2c（YC_WAVES の「絶対 path の罠を実行時に」）: CuaDriver 不在の通し（tests/yc_demo_e2e.rs の D-a の 2 本）を、
# cua-driver の絶対 path の候補の exec を拒否する sandbox-exec の profile の下で回し、拒否の記録（kernel の Sandbox の log）が 0 件かを見る。
# PATH・CUA_DRIVER_BIN の経路は E2E 自身が罠に向ける。ここが見るのは「source に書いた絶対 path を実行時に exec したか」。
#   使い方: bash scripts/qa/cua-absent-sandbox.sh          → 拒否 0 件なら exit 0（最後の行 RESULT pass）
#           NEG=1 bash scripts/qa/cua-absent-sandbox.sh    → 負の対照: 同じ sandbox の中で候補を 1 つ exec する 1 行を足す → 拒否が出て exit 1
# 候補: /Applications/CuaDriver.app の下全部（src の走査で見つかる絶対 path はここだけ）と、PATH の慣例の置き場の cua-driver。
# 毎回の正の対照: 同じ sandbox の中で囮（/usr/bin/true の写し）を 1 回 exec して拒否させる。その拒否が log にちょうど 1 件
# 見えなければ「log を読めていない」として exit 2（RESULT unmeasured）。log show の失敗・空の出力で pass を出さない。
set -u
cd "$(dirname "$0")/../.."
export CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0} CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
bin=$(cargo test --test yc_demo_e2e --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import json,sys
for l in sys.stdin:
    m=json.loads(l)
    if m.get("reason")=="compiler-artifact" and m.get("target",{}).get("name")=="yc_demo_e2e" and m.get("executable"): print(m["executable"])' | tail -1)
[ -x "$bin" ] || { echo "yc_demo_e2e の test binary を作れない" >&2; exit 2; }
tag="shibubu-cua-trap"
work=$(cd "$(mktemp -d -t cua-absent)" && pwd -P) # sandbox の literal は実 path で当てる（/var → /private/var）
decoy="$work/shibubu-decoy-$$"
prof="$work/trap.sb"
cleanup() { rm -f "$decoy" "$prof"; rmdir "$work" 2>/dev/null; }
cp /usr/bin/true "$decoy"
cat > "$prof" <<EOF
(version 1)
(allow default)
(deny process-exec (with message "$tag")
  (literal "$decoy")
  (subpath "/Applications/CuaDriver.app")
  (literal "$HOME/.local/bin/cua-driver")
  (literal "/usr/local/bin/cua-driver")
  (literal "/opt/homebrew/bin/cua-driver"))
EOF
start=$(date '+%Y-%m-%d %H:%M:%S')
neg=""
[ "${NEG:-0}" = 1 ] && neg='/Applications/CuaDriver.app/Contents/MacOS/cua-driver --version >/dev/null 2>&1;'
sandbox-exec -f "$prof" /bin/sh -c "\"\$1\" >/dev/null 2>&1; $neg \"\$0\" --exact demo_runs_with_cuadriver_absent demo_never_starts_cuadriver --test-threads=1" "$bin" "$decoy"
rc=$?
# 拒否の行（kernel の Sandbox）。tag で引くと log 自身の起動の行（引数に tag）も当たるので、path で引く。sandbox の外の process は拒否されないので、当たるのはこの run の物だけ
logs=""
seen=0
for _ in 1 2 3 4 5; do # kernel の log が届くまで
  sleep 2
  if ! logs=$(log show --start "$start" --style compact --predicate 'sender == "Sandbox" AND eventMessage CONTAINS "deny" AND eventMessage CONTAINS "process-exec"'); then
    echo "log show が失敗した: 拒否を数えられないので未測" >&2
    cleanup
    echo "RESULT unmeasured"
    exit 2
  fi
  seen=$(printf '%s\n' "$logs" | grep -cF "$decoy")
  [ "$seen" -ge 1 ] && break
done
cleanup
if [ "$seen" != 1 ]; then
  echo "正の対照（囮の exec の拒否）が log に $seen 件（1 件のはず）: log を読めていないので未測" >&2
  echo "RESULT unmeasured"
  exit 2
fi
hits=$(printf '%s\n' "$logs" | grep -E "deny\([0-9]+\) process-exec.*(CuaDriver\.app|/cua-driver)")
denied=$(printf '%s' "$hits" | grep -c .)
[ -n "$hits" ] && printf '%s\n' "$hits" | sed 's/^/  /'
echo "e2e rc=$rc control=$seen denied=$denied"
[ "$rc" = 0 ] && [ "$denied" = 0 ] && { echo "RESULT pass"; exit 0; }
echo "RESULT fail"
exit 1
