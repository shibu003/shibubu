#!/bin/bash
# ui-gate.sh — UI の門の検査を決まった順に 1 回で流して 1 枚の表にする（画面に触らない・.app を起こさない）。
# 使い方: bash scripts/qa/ui-gate.sh --wave UI-n|UI-4a|4b|5a|5b|6a|6b [--no-cargo] [--rev <rev>] [--json]
#   順: interfaces → app-agnostic → check_wired → ui-owner-check（W15 UI-check）→ check_ui_copy --set all → ui-strings --set ui-2b（G13）→ --set outside（UI-2c の後）→ cargo の targeted 2 本。
#   この木に無い道具（file が無い・必要な引数を持たない旧い版）は「無し（在る branch）」と表に出し、exit に数えない（旧い使い方に落とさない）。
#   cargo は CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 nice -n 15 で 1 本ずつ・打つ前の load1 を表に書く（待たない）。
#   cargo が赤なら log を scripts/qa/retest-red.sh（在れば）に渡し、実物 / 環境由来 / 壊れを表に書く。環境由来だけなら赤に数えず、最後の行に別に数える。
# exit: 0 赤 0（無し・飛ばすは数えない）/ 1 赤あり / 2 使い方
# 試験用: UI_GATE_ROOT=<別の root> で検査の file の置き場を替えられる。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${UI_GATE_ROOT:-$(cd "$HERE/../.." && pwd)}"
wave=""; rev=HEAD; cargo=1; json=0
while [ $# -gt 0 ]; do
  case "$1" in
    --wave) wave="${2:-}"; shift ;;
    --rev) rev="${2:-}"; shift ;;
    --no-cargo) cargo=0 ;;
    --json) json=1 ;;
    *) wave=""; break ;;
  esac
  shift
done
case "$wave" in UI-[0-9]|UI-[456][ab]) ;; *) echo "使い方: ui-gate.sh --wave UI-n|UI-4a|4b|5a|5b|6a|6b [--no-cargo] [--rev <rev>] [--json]" >&2; exit 2 ;; esac
[ -n "$rev" ] || { echo "使い方: --rev に値が要る" >&2; exit 2; }

T=$(mktemp -d "${TMPDIR:-/tmp}/ui-gate.XXXX"); trap 'rm -rf "$T"' EXIT
ROWS="$T/rows.tsv"; : > "$ROWS"; n=0
load1() { sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}'; }
last() { awk 'NF{l=$0} END{print l}' "$1" | tr '\t|' '  ' | cut -c1-110; }
row() { n=$((n+1)); printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$n" "$1" "$2" "$3" "$4" "$5" >> "$ROWS"; }   # 検査 command 状態 exit 要約

# 道具の検査: tool <名前> <file> <必要な字面（空白区切り・全部 file に在る時だけ新しい版）> <在る branch> <command...>
tool() {
  local name=$1 file=$2 need=$3 where=$4; shift 4
  local cmd="$*"
  local w ok=1; [ -f "$ROOT/$file" ] || ok=0
  for w in $need; do [ $ok -eq 1 ] && ! grep -qF -- "$w" "$ROOT/$file" && ok=0; done
  if [ $ok -eq 0 ]; then
    row "$name" "$cmd" 無し - "無し（在る branch: ${where}）"; return
  fi
  ( cd "$ROOT" && "$@" ) > "$T/out" 2>&1; local rc=$?
  if [ $rc -eq 0 ]; then row "$name" "$cmd" 緑 0 "$(last "$T/out")"; else row "$name" "$cmd" 赤 $rc "$(last "$T/out")"; fi
}

tool interfaces scripts/interfaces.py "" yc/w15 python3 -I scripts/interfaces.py --check
tool app-agnostic scripts/check_app_agnostic.py "" yc/w15 python3 -I scripts/check_app_agnostic.py
tool wired scripts/qa/check_wired.py "" yc/w15 python3 -I scripts/qa/check_wired.py
tool ui-owner-check scripts/qa/ui-owner-check.py "" "yc/w15 14e8603" python3 -I scripts/qa/ui-owner-check.py --rev "$rev" --wave "$wave"
tool ui-copy scripts/check_ui_copy.py '"--set"' yc/ui python3 -I scripts/check_ui_copy.py --strict --set all
tool ui-strings:ui-2b scripts/qa/ui-strings.py '"--max-ja" "ui-2b"' "yc/ui-2b 9ac442b" python3 -I scripts/qa/ui-strings.py --set ui-2b --max-ja 0 --map docs/ui-copy/outside.en.md
tool ui-strings:outside scripts/qa/ui-strings.py '"outside":' "yc/ui-2b（UI-2c の後）" python3 -I scripts/qa/ui-strings.py --set outside --max-ja 0 --map docs/ui-copy/outside.en.md

if [ $cargo -eq 0 ]; then
  row cargo:integration "" 飛ばす - "--no-cargo"; row cargo:bin "" 飛ばす - "--no-cargo"
elif ! command -v cargo >/dev/null 2>&1; then
  row cargo:integration "" 無し - "無し（cargo が PATH に無い）"; row cargo:bin "" 無し - "無し（cargo が PATH に無い）"
else
  # G13 が「targeted は INTEG-UI-a の門で」と残した一覧
  cg() {
    local name=$1; shift; local l; l=$(load1)
    ( cd "$ROOT" && CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 nice -n 15 cargo test "$@" ) > "$T/cg.$name" 2>&1; local rc=$?
    local cmd="cargo test $*"
    if [ $rc -eq 0 ]; then row "cargo:$name" "$cmd" 緑 0 "load1 $l: $(grep -E '^test result' "$T/cg.$name" | tail -1 | cut -c1-80)"; return; fi
    if [ -f "$ROOT/scripts/qa/retest-red.sh" ]; then
      ( cd "$ROOT" && bash scripts/qa/retest-red.sh "$T/cg.$name" ) > "$T/rr.$name" 2>&1; local rr=$?
      local sum; sum=$(last "$T/rr.$name")
      if [ $rr -eq 0 ] && grep -q '環境由来' "$T/rr.$name" && ! grep -q '実物 [1-9]\|壊れ [1-9]' "$T/rr.$name"; then row "cargo:$name" "$cmd" 環境由来 $rc "load1 $l: $sum"
      else row "cargo:$name" "$cmd" 赤 $rc "load1 $l: $sum"; fi
    else
      row "cargo:$name" "$cmd" 赤 $rc "load1 $l: $(last "$T/cg.$name")（retest-red.sh が無く分けていない）"
    fi
  }
  cg integration --test adapters --test intel --test yc_demo_e2e
  cg bin --bin shibubu -- w13 w14b w19_2 w19_3 terminal::tests gui:: engine_page:: app::tests::desktop analysis::tests
fi

exec python3 -I - "$ROWS" "$wave" "$json" <<'PY'
import json, sys
rows = [dict(zip(("n", "check", "command", "state", "exit", "summary"), l.rstrip("\n").split("\t"))) for l in open(sys.argv[1], encoding="utf-8")]
cnt = {s: sum(r["state"] == s for r in rows) for s in ("緑", "赤", "環境由来", "無し", "飛ばす")}
last = f"{sys.argv[2]}: 緑 {cnt['緑']} / 赤 {cnt['赤']} / 環境由来 {cnt['環境由来']} / 無し {cnt['無し']} / 飛ばす {cnt['飛ばす']} → {'exit 0' if cnt['赤'] == 0 else 'exit 1'}"
if sys.argv[3] == "1": print(json.dumps({"wave": sys.argv[2], "rows": rows, "counts": cnt}, ensure_ascii=False, indent=1))
else:
    print("| # | 検査 | command | 状態 | exit | 要約 |\n|---|---|---|---|---|---|")
    for r in rows: print(f"| {r['n']} | {r['check']} | {('`' + r['command'] + '`') if r['command'] else '-'} | {r['state']} | {r['exit']} | {r['summary']} |")
    print(last)
sys.exit(0 if cnt["赤"] == 0 else 1)
PY
