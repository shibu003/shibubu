#!/bin/bash
# gfinal-bg.sh — G-final の「BG の走らせ直し」を順序の表（gfinal-plan.tsv）の順に 1 本で流し、結果を 1 つの表（docs/evidence/G-final/bg-tally.md）に集める。
# 使い方:
#   bash scripts/qa/bg/gfinal-bg.sh --dry-run [--from n] [--only n]        起こす run の一覧・driver・見積もりを印字（.app を起こさない・cargo 無し）
#   bash scripts/qa/bg/gfinal-bg.sh <Shibubu.app> [--from n] [--only n] [--out DIR]   静かな窓で。各 run の前に owner の idle（owner_idle.py）を 1 回（120 秒未満はその run を未測にして次へ・待たない）
# GFINAL_PLAN=<tsv> で表を差し替え（検査用）。run の dir = <out>/<n>/run・meta は <out>/<n>/meta.json。
# 対象外（別の worktree の .app / FG の手）: G の ⑥・本物の REA・録画・A1・E1・F2 の lease・REAPER の drag
# exit: 0 全部緑 / 1 赤・表が壊れている / 2 使い方 / 6 未測あり
set -u
BG="$(cd "$(dirname "$0")" && pwd)"; REPO="$(cd "$BG/../../.." && pwd)"
PLAN="${GFINAL_PLAN:-$BG/gfinal-plan.tsv}"; MD="${GFINAL_MD:-$REPO/docs/evidence/G-final/bg-tally.md}"
usage() { echo "usage: gfinal-bg.sh --dry-run [--from n] [--only n] | <Shibubu.app> [--from n] [--only n] [--out DIR]" >&2; exit 2; }
[ $# -ge 1 ] || usage
APP=$1; shift; SEL=(); OUT=""
while [ $# -gt 0 ]; do
  case $1 in --from|--only) [ $# -ge 2 ] || usage; SEL+=("$1" "$2"); shift 2;; --out) [ $# -ge 2 ] || usage; OUT=$2; shift 2;; *) usage;; esac
done
if [ "$APP" = "--dry-run" ]; then exec python3 -I "$BG/gfinal-tally.py" plan "$PLAN" "${SEL[@]}" --dry; fi
[ -d "$APP" ] || { echo "gfinal-bg: $APP が無い" >&2; exit 2; }
ROWS=$(python3 -I "$BG/gfinal-tally.py" plan "$PLAN" "${SEL[@]}") || exit 1   # 表が壊れていれば 1 本も起こさない
[ -n "$OUT" ] || OUT=$(mktemp -d "$HOME/shibubu-yc-runs/gfinal.XXXX"); mkdir -p "$OUT"
echo "gfinal-bg: out=$OUT"
while IFS=$'\t' read -r n cmd; do
  [ -n "$n" ] || continue
  s=$(python3 -I "$BG/owner_idle.py" --steps "$HOME"/shibubu-yc-runs/*/steps.jsonl 2>/dev/null | awk '{printf "%d", $1}'); s=${s:-0}  # mouse と、自分の type_text を除いた key（W15-b42）
  mkdir -p "$OUT/$n"
  if [ "$s" -lt 120 ]; then
    printf '{"status":"idle","idle_s":%s}\n' "$s" > "$OUT/$n/meta.json"; echo "run $n: owner 使用中で未測（owner の idle ${s}s < 120s）"; continue
  fi
  R="$OUT/$n/run"; mkdir -p "$R"; t0=$(date +%s)
  c=${cmd//\{R\}/$(printf %q "$R")}; c=${c//\{app\}/$(printf %q "$APP")}
  (cd "$REPO" && bash -c "$c") > "$OUT/$n/stdout.log" 2>&1; rc=$?
  printf '{"status":"ran","rc":%s,"secs":%s,"idle_s":%s}\n' "$rc" "$(( $(date +%s) - t0 ))" "$s" > "$OUT/$n/meta.json"
  echo "run $n: rc=$rc $(( $(date +%s) - t0 ))s（idle ${s}s）"
done <<< "$ROWS"
python3 -I "$BG/gfinal-tally.py" table "$PLAN" "$OUT" "$MD"
