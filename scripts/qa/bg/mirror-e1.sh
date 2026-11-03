#!/bin/bash
# mirror-e1.sh — W16M-2 の実機の E1（試験の側の道具・製品の依存ではない）。
# 端末から直に起こした Shibubu（背景・罠の環境）の Mirror の Page で、Mirror の矩形を focus にして `tests/mirror_e0.rs` の w16m2_e1 を回す。
#
# 使い方:
#   bash scripts/qa/bg/mirror-e1.sh <out-dir> <shibubu の実行 file> <data dir の種> <TextEdit の書類（絶対 path）>
#     <data dir の種> = `w16m1_seed_mirror_page`（W16M1_TEXTEDIT_DOC = 同じ書類）で Mirror の Page を置いた data dir（写して使う・同意は run の中で押す）
#   前提: TextEdit が <書類> を開いている（open -g -a TextEdit・試験のために作った書類だけ）・端末に画面収録・Accessibility・TextEdit の
#         オートメーションの許可・KANGAE_MIRROR_HELPER（KangaeMirror.swift を build した物）・E1 の試験の実行 file
#         （cargo test --features mirror --test mirror_e0 --no-run）・bin/front-guard（scripts/qa/bg/build.sh）
# 手: 起動 → cua-driver の element_token の AXPress で「メモ」→「映す」→ Mirror の矩形（keyboard の focus・AX の hit test と identifier の一致で AXPress）→ 試験が Shibubu の pid へ
#     postToPid で文字と ⌘S（人の手の代わり）→ TextEdit の AX と disk の読み戻し・e1_path・負の対照（直接 postToPid → 赤）
# 出す物: <out-dir>/{run.log, relay.jsonl（製品の relay の記録）, e1.txt, steps/e1.{before,after,verdict}.json, events.jsonl, s*.json, mirror.png}
# 保証する: Shibubu は SHIBUBU_BACKGROUND=1・PATH から cua-driver の dir を外し CUA_DRIVER_BIN は罠・座標の click（event）・activate・cursor の移動なし・
#   D-g は front-guard の watch（Shibubu と TextEdit を suspect）と、試験の前後の snap（needle = 打つ文字・owner の前面の欄に漏れていない）の judge・
#   自分の pid だけ kill。保証しない: 人の手の pointer（Mirror の矩形は自分で描いていて AX に出ない＝FG）・.app の許可（端末の許可を借りる）・
#   前面の app の keyboard focus と漏れ（前面が Chrome / Electron だと AXFocusedUIElement が取れない＝ judge の unmeasured → exit 6）。
# 背景の Shibubu が pid 宛ての key を受ける前提: SHIBUBU_BACKGROUND=1 の窓は app の中の key の窓（main.rs `order_back_and_make_key`・非 active のまま）。
set -u
WT="$(cd "$(dirname "$0")/../../.." && pwd)"
BG="$WT/scripts/qa/bg"
CUA="${CUA_DRIVER_REAL:-$HOME/.local/bin/cua-driver}"
FG="${FRONT_GUARD:-$BG/bin/front-guard}"
R=$1; BIN=$2; SEED=$3; DOC=$4
: "${KANGAE_MIRROR_HELPER:?KANGAE_MIRROR_HELPER（KangaeMirror.swift を build した物）が要る}"
# 書類の窓を持つ TextEdit の pid（他の列の試験が別の TextEdit を起こしている事がある）
# Shibubu を起こす前に引く: helper の list（ScreenCaptureKit）の process が終わると、動いている Mirror の stream が -3805 で止まる（W16M-2 で実測）
read -r TE TEW < <("$KANGAE_MIRROR_HELPER" list --all | python3 -I -c 'import json,sys
print(next(("%d %d" % (w["pid"], w["window_id"]) for w in json.load(sys.stdin) if w["bundle_id"]=="com.apple.TextEdit" and w["title"]==sys.argv[1]), ""))' "$(basename "$DOC")")
[ -n "${TE:-}" ] || { echo "TextEdit が ${DOC} を開いていない（open -g -a TextEdit ${DOC}）" >&2; exit 3; }
rm -rf "$R"; mkdir -p "$R/trap" "$R/steps"; cp -R "$SEED" "$R/data"
L=$R/run.log
log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$L"; }
front(){ lsappinfo front; }
printf '#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap/marks"\nexit 1\n' "$R" > "$R/trap/cua-driver"; chmod +x "$R/trap/cua-driver"
P=""; for d in ${PATH//:/ }; do [ -x "$d/cua-driver" ] || P="${P:+$P:}$d"; done
NONCE=w16m2$(date +%s | tr 0-9 abcdefhkmn)
# cua-driver の session は終わると同じ名前で使えない（2 回目の run が「session has ended」で全部断られた）。run ごとに別の名前
SESSION=mirror-e1-$NONCE
# owner = 起動の前の前面（run.sh と同じく front-guard の snap から。lsappinfo info は Orca の ASN に答えない）
"$FG" snap --out "$R/steps/launch.before.json"
OWNER=$(python3 -I -c 'import json,sys;f=json.load(open(sys.argv[1])).get("front");print(f["pid"] if f else "")' "$R/steps/launch.before.json")

F0=$(front); log "front before launch: $F0 (owner pid $OWNER) · load $(uptime | sed 's/.*averages: //') · nonce $NONCE"
env PATH="$P" CUA_DRIVER_BIN="$R/trap/cua-driver" SHIBUBU_BACKGROUND=1 SHIBUBU_DATA_DIR="$R/data" KANGAE_DATA_DIR="$R/data" \
  KANGAE_MIRROR_HELPER="$KANGAE_MIRROR_HELPER" KANGAE_MIRROR_FRAME_LOG="$R/frames.jsonl" KANGAE_MIRROR_RELAY_LOG="$R/relay.jsonl" \
  nohup "$BIN" > "$R/app.log" 2>&1 &
PID=$!; log "launched pid $PID ($BIN)"
"$FG" watch --out "$R/events.jsonl" --stop "$R/watch.stop" --suspect "$PID" --suspect "$TE" >/dev/null 2>&1 & WATCH=$!
stop(){ touch "$R/watch.stop"; wait "$WATCH" 2>/dev/null; kill "$PID" 2>/dev/null; sleep 1; kill -0 "$PID" 2>/dev/null && kill -9 "$PID"
  log "killed $PID · trap marks $(cat "$R/trap/marks" 2>/dev/null | wc -l | tr -d ' ') · front $(front) (before $F0)"; }
WID=""; for _ in $(seq 60); do
  WID=$("$CUA" call list_windows "{\"pid\":$PID}" 2>/dev/null | python3 -I -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: sys.exit()
w=[x for x in d.get("windows",[]) if x.get("pid")=='"$PID"' and x.get("title")]
print(w[0]["window_id"] if w else "")')
  [ -n "$WID" ] && break; sleep 2; done
[ -n "$WID" ] || { log "窓が出ない"; stop; exit 3; }
"$CUA" call set_agent_cursor_enabled "{\"session\":\"$SESSION\",\"enabled\":false}" > "$R/cursor-off.json" 2>&1
log "window $WID · front $(front)"
# timeout_ms は mirror-e0.sh と同じ 20000・写真は要らない（walk の時間を写真に使わない）
state(){ "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"timeout_ms\":20000,\"include_screenshot\":false,\"session\":\"$SESSION\"}" > "$R/$1.json" 2>/dev/null; }
# 要素を label（と role）で探して element_token を返す
token(){ python3 -I "$BG/labels.py" --token "$R/$1.json" "$2" "${3:-}"; }
press(){ "$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"element_token\":\"$1\",\"session\":\"$SESSION\"}" > "$R/$2.click.json" 2>&1; log "AXPress $2 ($1) · front $(front) · $(head -c 160 "$R/$2.click.json" | LC_ALL=C tr '\n' ' ')"; ! grep -q "failed" "$R/$2.click.json"; }
# 混んだ機械では AXPress が -25204（答えが期限に間に合わない）を返す事がある（load 165 で実測）。押せたか分からないので、要素を引き直して押し直す
# （押せていて要素が消えていれば、それで済み）
find_press(){ local name=$1 label=$2 role=${3:-} tok="" tried=0; for _ in $(seq 12); do state "s-$name"; tok=$(token "s-$name" "$label" "$role")
    if [ -z "$tok" ]; then [ $tried = 1 ] && return 0; sleep 2; continue; fi
    press "$tok" "$name" && return 0; tried=1; sleep 3; done
  log "${name}（$(python3 -I "$BG/labels.py" --print "$label" | paste -sd/ -)）を押せない"; stop; exit 3; }
find_press memo memo AXCheckBox
find_press consent mirror AXButton
for _ in $(seq 120); do [ -s "$R/frames.jsonl" ] && break; sleep 1; done
[ -s "$R/frames.jsonl" ] || { log "映像が来ない（frames.jsonl が空）"; stop; exit 3; }
# Mirror の矩形（AXIdentifier surface-frame:notes）: Mirror を開いた debug の Shibubu は 33ms ごとに描き直すので、cua-driver の木の walk が混んだ機械では
# 矩形まで届かない（W16M-2 で実測: 20 秒で 16 / 48 node）。AX の hit test と identifier の一致で同じ要素を AXPress する（oracle/ax.swift press）
xcrun swiftc -O "$BG/oracle/ax.swift" -o "$R/ax" 2>>"$R/run.log" || { log "ax を build できない"; stop; exit 3; }
"$R/ax" press "$PID" "$WID" "surface-frame:notes" > "$R/mirror.press.json" 2>&1; log "AXPress mirror (surface-frame:notes) · $(cat "$R/mirror.press.json") · front $(front)"
grep -q '"pressed":true' "$R/mirror.press.json" || { stop; exit 3; }
sleep 1
"$FG" snap --out "$R/steps/e1.before.json" --needle "$NONCE"
TEST=$(/bin/ls -t "$WT"/target/debug/deps/mirror_e0-* | grep -v '\.d$' | head -1)
(cd "$WT" && W16M2_SHIBUBU_WID=$WID W16M2_SHIBUBU_PID=$PID W16M2_TEXTEDIT_DOC="$DOC" W16M2_TEXTEDIT_PID=$TE W16M2_TEXTEDIT_WID=$TEW W16M2_NONCE="$NONCE" KANGAE_MIRROR_RELAY_LOG="$R/relay.jsonl" \
  KANGAE_MIRROR_HELPER="$KANGAE_MIRROR_HELPER" "$TEST" w16m2_e1_typing_and_cmd_s_on_the_mirror_reach_textedit --exact --ignored --nocapture > "$R/e1.txt" 2>&1)
RC=$?
"$FG" snap --out "$R/steps/e1.after.json" --needle "$NONCE"
sleep 0.5
"$FG" judge --step e1 --before "$R/steps/e1.before.json" --after "$R/steps/e1.after.json" --events "$R/events.jsonl" --suspect "$PID" --suspect "$TE" ${OWNER:+--owner $OWNER} > "$R/steps/e1.verdict.json"
JRC=$?
log "e1 exit $RC · D-g judge exit $JRC · front $(front)"
grep -E "E1 |panicked|test result" "$R/e1.txt" | tee -a "$L"
state s-after
screencapture -x -o -l "$WID" "$R/mirror.png"
stop
# exit: 0 = E1 緑・D-g の検出 0・罠の印 0 / 6 = E1 緑・D-g の violation 0 だが未測の項目あり（front-guard の unmeasured・緑と読まない）/ 1 = それ以外
[ "$RC" = 0 ] && [ ! -s "$R/trap/marks" ] && { [ "$JRC" = 0 ] || [ "$JRC" = 6 ]; } || exit 1
exit "$JRC"
