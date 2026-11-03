#!/bin/bash
# integ-ui.sh — INTEG-UI（yc/ui ← yc/trunk ← yc/ui-2b ← yc/ui-2c …）の merge と門を 4 command にする。cargo は直接打たない（gate が ui-gate.sh に渡す）。
# 使い方（cwd = ours の worktree。今の branch が ours）:
#   integ-ui.sh plan [<theirs>…]        merge-preview を打ち、打つ順の command を印字（何もしない・theirs 省略 = yc/trunk yc/ui-2b yc/ui-2c の在る物）
#   integ-ui.sh merge <theirs>          preview → git merge --no-ff。衝突が docs/YC_INTERFACES.md だけなら interfaces.py で生成し直して commit。
#                                       他の file が衝突したら手を付けず exit 3（merge は途中のまま = git status で見る・解くのは operator か pane）
#   integ-ui.sh regen                   interfaces.py → --check → 差分があれば commit
#   integ-ui.sh gate --wave UI-n [--no-cargo] [--rev R] [--json]   = ui-gate.sh
#   integ-ui.sh strings <rev>           <rev> の木（git archive を一時 dir に展開・worktree は作らない）から ui-strings.py --csv で strings.csv を作り、
#                                       operator の写し（docs/internal/ui-pro/strings.csv）との差の行数を印字（SC=写しの path で替えられる）。何も書き換えない
#   integ-ui.sh rows <app> [plan.tsv]   quiet-window-plan.tsv（既定 scripts/qa/bg/quiet-window-plan.tsv）の command の欄の *.app だけを <app> に替えた表を stdout へ
#                                       （comment 行・name・cwd は変えない・file は書き換えない。quiet-window.sh --check に流せる）
#   共通: --dry-run は command を印字するだけ（merge / regen / gate）
# exit: 0 / 1 検査が赤（regen --check・gate） / 2 使い方・dirty・merge 中 / 3 YC_INTERFACES 以外が衝突
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "integ-ui: git の木の中で打つ" >&2; exit 2; }
cd "$ROOT" || exit 2
IFC=docs/YC_INTERFACES.md
dry=0; args=()
for a in "$@"; do if [ "$a" = "--dry-run" ]; then dry=1; else args+=("$a"); fi; done
set -- ${args[@]+"${args[@]}"}
cmd="${1:-}"; [ $# -gt 0 ] && shift
usage() { echo "使い方: integ-ui.sh plan [<theirs>…] | merge <theirs> | regen | gate --wave UI-n [--no-cargo] | strings <rev> | rows <app> [plan.tsv] | (共通 --dry-run)" >&2; exit 2; }
ours=$(git branch --show-current); [ -n "$ours" ] || ours=HEAD

clean_tree() {
  [ -e "$(git rev-parse --git-path MERGE_HEAD)" ] && { echo "integ-ui: merge の途中（先に解くか git merge --abort）" >&2; exit 2; }
  [ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "integ-ui: 木が dirty（commit してから）" >&2; exit 2; }
}

case "$cmd" in
plan)
  th=("$@")
  if [ ${#th[@]} -eq 0 ]; then for b in yc/trunk yc/ui-2b yc/ui-2c; do git rev-parse --verify -q "${b}^{commit}" >/dev/null && th+=("$b"); done; fi
  [ ${#th[@]} -gt 0 ] || { echo "integ-ui: 重ねる branch が無い" >&2; exit 2; }
  bash "$HERE/merge-preview.sh" "$ours" "${th[@]}"; rc=$?
  [ $rc -eq 2 ] && exit 2
  echo; echo "## 打つ順（${ours} の worktree で）"; echo
  i=1; for b in "${th[@]}"; do echo "${i}. bash scripts/qa/integ-ui.sh merge ${b}"; i=$((i+1)); done
  echo "${i}. bash scripts/qa/integ-ui.sh regen"; echo "$((i+1)). bash scripts/qa/integ-ui.sh gate --wave UI-n [--no-cargo]"
  exit 0 ;;
merge)
  [ $# -eq 1 ] || usage
  theirs=$1
  sha=$(git rev-parse --short --verify "${theirs}^{commit}" 2>/dev/null) || { echo "integ-ui: ${theirs} が無い" >&2; exit 2; }
  clean_tree
  msg="統合: ${theirs}（${sha}）を ${ours} へ — INTEG-UI"
  bash "$HERE/merge-preview.sh" "$ours" "$theirs"
  if [ $dry -eq 1 ]; then echo "(dry-run) git merge --no-ff -m '${msg}' ${theirs}"; echo "(dry-run) 衝突が ${IFC} だけなら: python3 -I scripts/interfaces.py → git add → git commit"; exit 0; fi
  if git merge --no-ff -m "$msg" "$theirs" >/dev/null 2>&1; then echo "integ-ui: merge 済（衝突 0）: ${msg}"; exit 0; fi
  conf=$(git diff --name-only --diff-filter=U)
  if [ "$conf" != "$IFC" ]; then
    echo "integ-ui: 衝突 file（手を付けない・merge は途中）:"; echo "| file |"; echo "|---|"; echo "$conf" | sed 's/^/| /; s/$/ |/'
    exit 3
  fi
  python3 -I scripts/interfaces.py && git add -- "$IFC" && python3 -I scripts/interfaces.py --check || { echo "integ-ui: ${IFC} の生成し直しが赤（merge は途中）" >&2; exit 1; }
  git commit -q -m "$msg" -m "docs/YC_INTERFACES.md の衝突は scripts/interfaces.py で生成し直した" || exit 1
  echo "integ-ui: merge 済（${IFC} を生成し直した）: ${msg}"; exit 0 ;;
regen)
  [ $# -eq 0 ] || usage
  clean_tree
  if [ $dry -eq 1 ]; then echo "(dry-run) python3 -I scripts/interfaces.py; --check; 差分があれば commit"; exit 0; fi
  python3 -I scripts/interfaces.py && python3 -I scripts/interfaces.py --check || { echo "integ-ui: interfaces.py が赤" >&2; exit 1; }
  if git diff --quiet -- "$IFC"; then echo "integ-ui: ${IFC} に差分なし"; exit 0; fi
  git add -- "$IFC" && git commit -q -m "INTEG-UI: docs/YC_INTERFACES.md を生成し直した" -- "$IFC" || exit 1
  echo "integ-ui: ${IFC} を生成し直して commit"; exit 0 ;;
gate)
  [ $dry -eq 1 ] && { echo "(dry-run) bash scripts/qa/ui-gate.sh $*"; exit 0; }
  exec bash "$HERE/ui-gate.sh" "$@" ;;
strings)
  [ $# -eq 1 ] || usage
  rev=$1; copy="${SC:-docs/internal/ui-pro/strings.csv}"
  git rev-parse --verify -q "${rev}^{commit}" >/dev/null || { echo "integ-ui: ${rev} が無い" >&2; exit 2; }
  [ $dry -eq 1 ] && { echo "(dry-run) git archive ${rev} → ui-strings.py --csv --root <tmp> → ${copy} と比べる"; exit 0; }
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/integ-ui-strings.XXXXXX") || exit 2
  trap 'rm -rf "$tmp"' EXIT
  git archive "$rev" -- src | tar -x -C "$tmp" || { echo "integ-ui: ${rev} の src を取り出せない" >&2; exit 2; }
  python3 -I "$HERE/ui-strings.py" --root "$tmp" --csv "$tmp/out.csv" || { echo "integ-ui: ui-strings.py が赤" >&2; exit 1; }
  n=$(( $(wc -l < "$tmp/out.csv") - 1 ))
  if [ ! -f "$copy" ]; then echo "integ-ui strings: rev=$(git rev-parse --short "$rev") rows=${n} copy=無し（${copy}）diff=-"; exit 0; fi
  d=$(diff <(sort "$copy") <(sort "$tmp/out.csv") | grep -c '^[<>]')
  echo "integ-ui strings: rev=$(git rev-parse --short "$rev") rows=${n} copy=$(( $(wc -l < "$copy") - 1 )) diff=${d}"; exit 0 ;;
rows)
  [ $# -ge 1 ] && [ $# -le 2 ] || usage
  app=$1; plan="${2:-scripts/qa/bg/quiet-window-plan.tsv}"
  [ -f "$plan" ] || { echo "integ-ui: ${plan} が無い" >&2; exit 2; }
  case "$app" in *.app) ;; *) echo "integ-ui: <app> は *.app の path" >&2; exit 2 ;; esac
  python3 -I - "$app" "$plan" <<'PY'
import re, sys
app, plan = sys.argv[1:3]
pat = re.compile(r"[^\s\"';]*\.app(?![\w./-])")
for line in open(plan, encoding="utf-8").read().splitlines():
    if line.startswith("#") or not line.strip():
        print(line); continue
    cols = line.split("\t")
    cols[2:] = [pat.sub(lambda _: app, c) for c in cols[2:]]
    print("\t".join(cols))
PY
  exit $? ;;
*) usage ;;
esac
