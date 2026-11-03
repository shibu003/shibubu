#!/bin/bash
# mirror-w16m3.sh — W16M-3 の BG（試験の側の道具・製品の依存ではない）。
# 使い方:
#   bash scripts/qa/bg/mirror-w16m3.sh <out-dir> <shibubu の実行 file> <data dir の種> <TextEdit の書類（絶対 path）>
#   DENIED=1 の時は fake helper（list が exit 3）で「Screen recording …」の 1 行と relay 0 を見る（画面収録の取り消しは OWNER・未測）
# 保証する: 自分の pid だけ kill・PATH から cua-driver を外し罠・前面を取らない。
# 保証しない: OWNER の画面収録の取り消し・pointer の FG・REAPER。
set -u
WT="$(cd "$(dirname "$0")/../../.." && pwd)"
BG="$WT/scripts/qa/bg"
CUA="${CUA_DRIVER_REAL:-$HOME/.local/bin/cua-driver}"
R=$1; BIN=$2; SEED=$3; DOC=${4:-}
rm -rf "$R"; mkdir -p "$R/trap"; cp -R "$SEED" "$R/data"
L=$R/run.log
log(){ echo "[$(date +%H:%M:%S)] $*" | tee -a "$L"; }
front(){ lsappinfo front; }
printf '#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap/marks"\nexit 1\n' "$R" > "$R/trap/cua-driver"; chmod +x "$R/trap/cua-driver"
P=""; for d in ${PATH//:/ }; do [ -x "$d/cua-driver" ] || P="${P:+$P:}$d"; done
xcrun swiftc -O "$BG/oracle/ax.swift" -o "$R/ax" 2>>"$L" || { log "ax を build できない"; exit 3; }

if [ "${DENIED:-}" = 1 ]; then
  cat > "$R/fake-helper" <<'SH'
#!/bin/bash
if [ "$1" = list ]; then echo '{"error":"no_screen_capture_permission"}' >&2; exit 3; fi
echo '{"error":"no_screen_capture_permission"}' >&2; exit 3
SH
  chmod +x "$R/fake-helper"
  HELPER="$R/fake-helper"
else
  : "${KANGAE_MIRROR_HELPER:?KANGAE_MIRROR_HELPER が要る}"
  HELPER="$KANGAE_MIRROR_HELPER"
fi

F0=$(front); log "front before launch: $F0 · load $(uptime | sed 's/.*averages: //') · denied=${DENIED:-0}"
env PATH="$P" CUA_DRIVER_BIN="$R/trap/cua-driver" SHIBUBU_BACKGROUND=1 SHIBUBU_DATA_DIR="$R/data" KANGAE_DATA_DIR="$R/data" \
  KANGAE_MIRROR_HELPER="$HELPER" KANGAE_MIRROR_FRAME_LOG="$R/frames.jsonl" KANGAE_MIRROR_RELAY_LOG="$R/relay.jsonl" \
  nohup "$BIN" > "$R/app.log" 2>&1 &
PID=$!; log "launched pid $PID"
WID=""; for _ in $(seq 60); do
  WID=$("$CUA" call list_windows "{\"pid\":$PID}" 2>/dev/null | python3 -I -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: sys.exit()
w=[x for x in d.get("windows",[]) if x.get("pid")=='"$PID"' and x.get("title")]
print(w[0]["window_id"] if w else "")')
  [ -n "$WID" ] && break; sleep 2; done
[ -n "$WID" ] || { log "窓が出ない"; kill "$PID"; exit 3; }
log "window $WID · front $(front)"
state(){ "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"timeout_ms\":10000,\"include_screenshot\":false,\"session\":\"w16m3-$$\"}" > "$R/$1.json" 2>/dev/null; }
token(){ python3 -I "$BG/labels.py" --token "$R/$1.json" "$2" "${3:-}"; }
press(){ "$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"element_token\":\"$1\",\"session\":\"w16m3-$$\"}" > "$R/$2.click.json" 2>&1; log "AXPress $2 · $(head -c 120 "$R/$2.click.json" | LC_ALL=C tr '\n' ' ')"; }
find_press(){ local name=$1 label=$2 role=${3:-} tok=""; for _ in $(seq 12); do state "s-$name"; tok=$(token "s-$name" "$label" "$role")
    [ -n "$tok" ] && press "$tok" "$name" && return 0; sleep 2; done
  log "${name}（$(python3 -I "$BG/labels.py" --print "$label" | paste -sd/ -)）を押せない"; kill "$PID"; exit 3; }
find_press memo memo AXCheckBox
sleep 2
"$R/ax" tree "$PID" "$WID" > "$R/ax-id.json" 2>"$R/ax-id.err" || true
log "ax-id $(head -c 200 "$R/ax-id.json" 2>/dev/null) err=$(head -c 120 "$R/ax-id.err" 2>/dev/null)"
if [ "${DENIED:-}" = 1 ]; then
  [ -n "$(python3 -I "$BG/labels.py" --token "$R/s-memo.json" mirror_denied)" ] || log "cua-driver の木に許可の 1 行が無い（ax tree を見る）"
  n=$(wc -l < "$R/relay.jsonl" 2>/dev/null | tr -d ' '); n=${n:-0}
  log "relay lines $n"
  [ "$n" = 0 ] || { log "許可が無いのに relay がある"; kill "$PID"; exit 3; }
  kill "$PID"; sleep 1; kill -0 "$PID" 2>/dev/null && kill -9 "$PID"
  log "denied ok · trap $(wc -l < "$R/trap/marks" 2>/dev/null | tr -d ' ') · front $(front) (before $F0)"
  exit 0
fi
# 正の道: 種の Mirror Page へ → ax tree で矩形 → 他の Page へ移って戻る → frames が増え続ける（keep_alive）
find_press notes "notes" || find_press notes memo
for _ in $(seq 60); do [ -s "$R/frames.jsonl" ] && break; sleep 1; done
N0=$(wc -l < "$R/frames.jsonl" 2>/dev/null | tr -d ' '); N0=${N0:-0}
log "frames before leave $N0"
find_press dev "Dev" AXCheckBox || true
sleep 3
N1=$(wc -l < "$R/frames.jsonl" 2>/dev/null | tr -d ' '); N1=${N1:-0}
log "frames after leave $N1 (keep_alive なら増えるか、少なくとも消えない)"
kill "$PID"; sleep 1; kill -0 "$PID" 2>/dev/null && kill -9 "$PID"
log "killed $PID · trap $(wc -l < "$R/trap/marks" 2>/dev/null | tr -d ' ') · front $(front) (before $F0)"
