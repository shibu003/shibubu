#!/usr/bin/env bash
# 7 層レビューの機械側（docs/REVIEW_PROCESS.md §2）。main へ merge する前に打つ:
#   bash scripts/merge-gate.sh <branch> [--fast]
# 1) 証拠（層①④）: branch の commit message か docs/internal/evidence/<branch-slug>.md に「テスト:」と「戻し方:」がある
# 2) security（層③）: docs/internal/security/<branch-slug>.md が status: open なら止める
# 3) 検査（層④）: scripts/lint.sh を branch の一時 worktree で回す（--fast は省略）
# 4) risk（層⑤）: 変更 path で 要 人 0 / 1 / 2 人 を表示（数えるのは人。agent の approve は代わりにならない）
set -euo pipefail
cd "$(dirname "$0")/.."
branch="${1:?usage: merge-gate.sh <branch> [--fast]}"; fast="${2:-}"
slug="${branch//\//-}"
git rev-parse --verify -q "$branch" >/dev/null || { echo "gate: branch $branch が無い"; exit 1; }
base=$(git merge-base main "$branch")
fail=0

# 1) 証拠
msgs=$(git log --format=%B "$base..$branch")
ev="docs/internal/evidence/$slug.md"
has_test=0; has_back=0
# here-string で渡す（echo | grep -q は grep が先に閉じると echo が SIGPIPE で落ち、pipefail で 64KB を超える message の「在る」が「無い」になる）
grep -qE 'テスト:|tested:|test:' <<<"$msgs" && has_test=1
grep -qE '戻し方:|rollback:|revert:' <<<"$msgs" && has_back=1
if [ -f "$ev" ]; then grep -qE 'テスト:|tested:' "$ev" && has_test=1; grep -qE '戻し方:|rollback:' "$ev" && has_back=1; fi
[ "$has_test" -eq 1 ] || { echo "gate ①: 「テスト:」が commit message にも $ev にも無い（どうテストしたか）"; fail=1; }
[ "$has_back" -eq 1 ] || { echo "gate ④: 「戻し方:」が commit message にも $ev にも無い（どう戻すか）"; fail=1; }

# 2) security
sec="docs/internal/security/$slug.md"
if [ -f "$sec" ] && grep -qE '^status: *open' "$sec"; then echo "gate ③: $sec が open（直すか、理由をつけて closed に）"; fail=1; fi

# 4) risk（path で決める。書いた人では決めない）
files=$(git diff --name-only "$base" "$branch")
risk=0
if grep -qE '^(src|tests)/' <<<"$files"; then risk=1; fi
if grep -qE '^(\.github/|Cargo\.toml$|scripts/release/|src/storage/secret|src/billing|src/update)' <<<"$files"; then risk=2; fi
echo "gate ⑤: 変更 $(echo "$files" | grep -c .) file → 要 人 ${risk} 人の承認（docs/flag 裏 = 0・普通 = 1・課金/自動更新/CI/依存/secret = 2）"

# 層①の見直し（docs/REVIEW_PROMPTS.md「やり方から見直す」1・2026-10-17）: 外の実物（CLI・app・API）に触る変更は、
# 読むレビューでは取れない挙動を「実物で 1 回通した出力の写し」で押さえる。commit message か evidence に「実物:」の行が要る。
if grep -qE '^src/(cua\.rs|github\.rs|repos\.rs|audio/|media/|google/|adapters/|llm/|intel/)' <<<"$files"; then
  has_real=0
  grep -qE '実物:|real-run:' <<<"$msgs" && has_real=1
  if [ -f "$ev" ] && grep -qE '実物:|real-run:' "$ev"; then has_real=1; fi
  [ "$has_real" -eq 1 ] || { echo "gate ①: 外の実物（cua / GitHub / CLI / audio / media / Google / adapters / LLM / intel）に触る変更なのに「実物:」の行が無い（実物で 1 回通した出力の写しの path か、それで作った偽物の test の元を ${ev} か commit message に）"; fail=1; }
fi

# 3) 検査
if [ "$fast" != "--fast" ]; then
  tmp=$(mktemp -d /tmp/kangae-gate.XXXX); root=$(git rev-parse --show-toplevel)
  # --detach: pane の worktree が branch を checkout 中でも検査できる（2026-10-17: 無いと fatal で止まった）
  git worktree add -q --detach "$tmp" "$branch"
  # target は gate の間で使い回す（毎回 eframe から全 build しない）。cargo が dir を lock するので並んでも壊れない
  if (cd "$tmp" && CARGO_TARGET_DIR="$root/target-gate" bash scripts/lint.sh >"$tmp/lint.log" 2>&1); then echo "gate ④: lint ok（${branch}）"; else echo "gate ④: lint fail（${tmp}/lint.log）"; tail -5 "$tmp/lint.log"; fail=1; fi
  git worktree remove --force "$tmp" 2>/dev/null || true
fi

[ "$fail" -eq 0 ] && echo "gate: 通過（人の承認 ${risk} 人の後に merge）" || { echo "gate: 止めた"; exit 1; }
