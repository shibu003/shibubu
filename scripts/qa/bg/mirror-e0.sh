#!/bin/bash
# mirror-e0.sh — W16M-1b の実機の E0（試験の側の道具・製品の依存ではない）。
# 端末から直に起こした release の Shibubu（背景・罠の環境）で Mirror の Page を開き、`tests/mirror_e0.rs` の E0 を回す。
# .app を `open -g` で起こす run.sh の道は、.app に画面収録の許可が無い間は映らないので、許可は端末の物を借りる（TCC の主体が端末になる）。
#
# 使い方:
#   IDLE=180 bash scripts/qa/bg/mirror-e0.sh <out-dir> <shibubu の実行 file> <data dir の種>
#     <data dir の種> = `w16m1_seed_mirror_page` で Mirror の Page（surface "notes"・TextEdit の notes.md）を置き、同意を 1 回押した data dir（写して使う）
#     IDLE = Mirror を開いてから E0 までの背景の放置（秒・App Nap が入る間）。放置の後に probe（TextEdit の窓と Shibubu の窓を同じ時刻に撮って OCR）
#   前提: TextEdit が notes.md を開いている・端末に画面収録の許可・E0 の試験の実行 file（cargo test --features mirror --test mirror_e0 --no-run）
# 出す物: <out-dir>/{run.log, frames.jsonl（製品の FrameLog）, e0.txt, probe/, s1.json, after.png}
# 保証する: Shibubu は SHIBUBU_BACKGROUND=1・PATH から cua-driver の dir を外し CUA_DRIVER_BIN は罠（印の数を最後に書く）・操作は cua-driver の
#   element_token の AXPress だけ（座標・activate なし）・前面の app を起動の前と最後に記録・自分の pid だけ kill。
# 保証しない: .app の映像（画面収録の許可は owner の手）・D-g の 5 項目の全部（前面だけ見る。全部は run.sh）。
set -u
WT="$(cd "$(dirname "$0")/../../.." && pwd)"
CUA="${CUA_DRIVER_REAL:-$HOME/.local/bin/cua-driver}"
R=$1; BIN=$2; SEED=$3
TE=$(pgrep -x TextEdit | head -1); [ -n "$TE" ] || { echo "TextEdit が起きていない（open -g -a TextEdit notes.md）" >&2; exit 3; }
rm -rf "$R"; mkdir -p "$R/trap"; cp -R "$SEED" "$R/data"
L=$R/run.log
log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$L"; }
now(){ python3 -I -c 'import time;print(time.time())'; }
front(){ osascript -e 'tell application "System Events" to get name of first process whose frontmost is true'; }
printf '#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap/marks"\nexit 1\n' "$R" > "$R/trap/cua-driver"; chmod +x "$R/trap/cua-driver"
P=""; for d in ${PATH//:/ }; do [ -x "$d/cua-driver" ] || P="${P:+$P:}$d"; done

# TextEdit の窓と Shibubu の窓を同じ時刻に撮り、どちらに nonce が出たかを並べる（どちらの側が遅いか）
probe() {
  local O=$1 SW=$2 RECT=$3 TW N t1 i x y w h
  mkdir -p "$O"
  TW=$("$CUA" call list_windows "{\"pid\":$TE}" | python3 -I -c 'import json,sys;print([w["window_id"] for w in json.load(sys.stdin)["windows"] if w["title"].startswith("notes.md")][0])')
  N=pr$(date +%s | tr 0-9 abcdefhkmn)
  "$WT/target/w16m1-ax" set "$TE" notes.md "$N"; t1=$(now)
  for i in 1 2 3 4 5 6; do screencapture -x -o -l "$TW" "$O/t$i.png"; screencapture -x -o -l "$SW" "$O/s$i.png"; echo "$i +$(python3 -I -c "import time;print(round(time.time()-$t1,2))")" >> "$O/times"; /bin/sleep 0.3; done
  IFS=, read -r x y w h <<< "$RECT"
  for i in 1 2 3 4 5 6; do echo "$(sed -n ${i}p "$O/times") textedit=$("$WT/target/w16m1-ocr" "$O/t$i.png" "$TW" 0 0 2000 2000 | grep -c "$N") shibubu=$("$WT/target/w16m1-ocr" "$O/s$i.png" "$SW" "$x" "$y" "$w" "$h" | grep -c "$N")"; done
}

F0=$(front); log "front before launch: $F0 · load $(uptime | sed 's/.*averages: //')"
env PATH="$P" CUA_DRIVER_BIN="$R/trap/cua-driver" SHIBUBU_BACKGROUND=1 SHIBUBU_DATA_DIR="$R/data" KANGAE_DATA_DIR="$R/data" \
  KANGAE_MIRROR_HELPER="${KANGAE_MIRROR_HELPER:-$WT/target/w16m0-kangae-mirror-helper}" KANGAE_MIRROR_FRAME_LOG="$R/frames.jsonl" KANGAE_MIRROR_RELAY_LOG="$R/relay.jsonl" \
  nohup "$BIN" > "$R/app.log" 2>&1 &
PID=$!; log "launched pid $PID ($BIN)"
WID=""; for _ in $(seq 60); do
  WID=$("$CUA" call list_windows "{\"pid\":$PID}" 2>/dev/null | python3 -I -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: sys.exit()
w=[x for x in d.get("windows",[]) if x.get("pid")=='"$PID"' and x.get("title")]
print(w[0]["window_id"] if w else "")')
  [ -n "$WID" ] && break; sleep 2; done
[ -n "$WID" ] || { log "窓が出ない"; kill "$PID"; exit 3; }
log "window $WID · front $(front)"
state(){ "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"timeout_ms\":20000,\"session\":\"mirror-e0\"}" > "$R/$1.json" 2>/dev/null; }
TOK=""; for _ in $(seq 10); do state s1; TOK=$(python3 -I "$WT/scripts/qa/bg/labels.py" --token "$R/s1.json" memo AXCheckBox); [ -n "$TOK" ] && break; sleep 3; done
[ -n "$TOK" ] || { log "サイドバーの「メモ」が見つからない"; kill "$PID"; exit 3; }
"$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"element_token\":\"$TOK\",\"session\":\"mirror-e0\"}" > "$R/click.json" 2>&1
T0=$(now); log "click メモ ($TOK) · front $(front)"
for _ in $(seq 120); do [ -s "$R/frames.jsonl" ] && break; sleep 1; done
log "first texture after click: $(python3 -I -c "import time;print(int((time.time()-$T0)*1000))") ms"
state s2
# cua-driver の木は action の無い AXGroup を落とす（W16M-2 で実測）。矩形は試験の側の ax tree が Group も含めて辿る
xcrun swiftc -O "$WT/scripts/qa/bg/oracle/ax.swift" -o "$R/ax" 2>>"$L" || { log "ax を build できない"; kill "$PID"; exit 3; }
RECT=$("$R/ax" tree "$PID" "$WID" | python3 -I -c 'import json,sys
d=json.load(sys.stdin); b=d["window"]
for e in d.get("elements",[]):
  if (e.get("identifier") or "").startswith("surface-frame:"):
    f=e["frame"]; print("%d,%d,%d,%d"%(int(f["x"]-b["x"]),int(f["y"]-b["y"]),int(f["w"]),int(f["h"]))); break')
[ -n "$RECT" ] || { log "Mirror の矩形が AX に無い（映っていない）"; kill "$PID"; exit 3; }
log "rect $RECT"
if [ -n "${IDLE:-}" ]; then log "idle $IDLE s"; /bin/sleep "$IDLE"; probe "$R/probe" "$WID" "$RECT" | tee -a "$L"; fi
TEST=$(/bin/ls -t "$WT"/target/debug/deps/mirror_e0-* | grep -v '\.d$' | head -1)
(cd "$WT" && W16M1_SHIBUBU_WID=$WID W16M1_MIRROR_RECT=$RECT W16M1_TEXTEDIT_DOC=notes.md KANGAE_MIRROR_FRAME_LOG="$R/frames.jsonl" \
  "$TEST" w16m1_e0 --ignored --nocapture > "$R/e0.txt" 2>&1)
log "e0 exit $? · front $(front)"
grep -E "E0 |panicked|test result" "$R/e0.txt" | tee -a "$L"
screencapture -x -o -l "$WID" "$R/after.png"
kill "$PID"; sleep 1; kill -0 "$PID" 2>/dev/null && kill -9 "$PID"
log "killed $PID · trap marks $(cat "$R/trap/marks" 2>/dev/null | wc -l | tr -d ' ') · front $(front) (before $F0)"
