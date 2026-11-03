#!/usr/bin/env bash
# Kangae の機械検査。手元と CI（.github/workflows/rust.yml）で同じ物を回す。
#   bash scripts/lint.sh
# 1) cargo check  2) cargo clippy（在れば。警告は表示のみ）  3) cargo test（#[ignore] は走らない）
# 4) 後で行う受け入れ test（owner 2026-10-16・docs/ACCEPTANCE.md）が tests/acceptance.rs に 3 本あるか
set -euo pipefail
cd "$(dirname "$0")/.."

python3 -I scripts/check_ui_copy.py --self-test >/dev/null || { echo "lint: scripts/check_ui_copy.py --self-test が fail（直接回して見る）"; exit 1; }; python3 -I scripts/check_ui_copy.py --report  # 余計なキャッチコピー 0（DESIGN §15.2-5a）。文言を消す wave の後に --strict へ（docs/ui-copy-rules.md §4）
python3 -I scripts/check_app_agnostic.py --self-test >/dev/null || { echo "lint: scripts/check_app_agnostic.py --self-test が fail（直接回して見る）"; exit 1; }; python3 -I scripts/check_app_agnostic.py  # 汎用 Core と Adapter の境界（owner 18・19 通目・YC_WAVES §1）
# YCM-066（W14）: クレジット残高・購入・課金 plan を MVP に足さない = main から分かれた後に src へ足した行にその語が 0（main が無い clone では skip と言う）
if base=$(git merge-base main HEAD 2>/dev/null); then
  # apple_music.rs は外す: Apple の Music.app が返す契約の状態の語。Shibubu の課金ではない（YCM-066 は Shibubu に credit・購入・課金 plan を足さない門）
  hits=$(git diff -U0 "$base" -- src ':(exclude)src/media/apple_music.rs' | grep -E '^\+[^+]' | grep -Ei 'credit|課金|購入|subscription' || true)
  [ -z "$hits" ] || { echo "lint: YCM-066 の語（credit・課金・購入・subscription）が src の差分に在る:"; echo "$hits"; exit 1; }
  echo "lint: YCM-066 の語は src の差分に 0 ok"
else echo "lint: skip — main が無いので YCM-066 の差分を見ない"; fi
python3 -I scripts/interfaces.py --self-test >/dev/null || { echo "lint: scripts/interfaces.py --self-test が fail（直接回して見る）"; exit 1; }; python3 -I scripts/interfaces.py --check  # 公開の口の索引 docs/YC_INTERFACES.md が src の「Wave:」と合っている（owner 21 通目・古ければ python3 -I scripts/interfaces.py で書き直す）
# release の門（RULE-OSS / RULE-DENY・YC_DECISIONS 86 通目）: GPL / AGPL / license 無しの依存と LICENSE の無い vendor/ を赤にする
python3 -I scripts/release/third_party_notices.py --self-test >/dev/null || { echo "lint: scripts/release/third_party_notices.py --self-test が fail（直接回して見る）"; exit 1; }; python3 -I scripts/release/third_party_notices.py --check
if cargo deny --version >/dev/null 2>&1; then cargo deny --offline check licenses  # advisories と sources は今は見ない（11/02 の後に owner が決める）
else echo "lint: skip — cargo-deny が無い（cargo install cargo-deny）"; fi
cargo check
cargo check --all-targets --all-features  # feature の枝（mirror・native_direct・webview）も compile する（R4 反証: merge cd14d5c で --features mirror だけ壊れ、既定の check では見えなかった）
if cargo clippy --version >/dev/null 2>&1; then
  cargo clippy 2>&1 | grep -E '^warning: `|^error' || true
fi
cargo test

# v0.12 の schema / example の静的検査（docs/scripts/*.py・jsonschema が要る。無ければ skip と言う）
if python3 -c 'import jsonschema' 2>/dev/null; then
  for s in validate_goal_plans validate_app_examples validate_v2_apps validate_agent_native_contracts validate_placement_templates; do
    (cd docs && python3 -I "scripts/$s.py" >/dev/null) || { echo "lint: docs/scripts/$s.py が fail"; exit 1; }
  done
# service ごとの能力の一覧を製品に入れない（owner 10-23「registryとか作らない約束だったのでは？serviceごとに能力をこの製品に入れるのではないと伝えたはず」）
if [ -e docs/registry ] || grep -rqE "include_str!\([^)]*registry/software" src; then echo "lint: service ごとの能力の一覧（docs/registry・その埋め込み）が製品に在る。要る時に Engine が調べる（owner 10-23）"; exit 1; fi
echo "lint: service ごとの能力の一覧は製品に無い ok"
  python3 -I scripts/qa_run.py --check docs/qa/scenarios/*.json >/dev/null && echo "lint: QA の台本（docs/qa/scenarios）の schema・意味・自己検査 ok" || { echo "lint: scripts/qa_run.py --check が fail（docs/qa/README.md）"; exit 1; }
  (cd docs && python3 -I scripts/test_check_holdout.py >/dev/null 2>&1) && echo "lint: holdout の採点器（check_holdout.py の --prompts と surface の規則）の test ok" || { echo "lint: docs/scripts/test_check_holdout.py が fail"; exit 1; }
  echo "lint: v0.12 の schema / example 検査 4 本と配置の型（W13c）の 1 本 ok"
else
  echo "lint: skip — python の jsonschema が無い（pip3 install jsonschema で 5 本の検査が走る）"
fi

ACC=tests/acceptance.rs
[ -f "$ACC" ] || { echo "lint: $ACC が無い（docs/ACCEPTANCE.md §4）"; exit 1; }
missing=0
for t in test1_linkedin_gmail_outreach test2_calendar_gmail_claude_app test3_reaper_youtube_applemusic_app; do
  if ! grep -q "fn $t" "$ACC"; then echo "lint: $ACC に $t が無い（docs/ACCEPTANCE.md §3 と同じ名前で置く）"; missing=1; fi
done
[ "$missing" -eq 0 ] || exit 1
# 独立に作った module も crate の中で compile する（tests / bin の #[path] からしか compile されない状態に戻さない・p-wire 1）
for f in src/*/mod.rs src/*.rs; do
  m=$(basename "$f" .rs); [ "$m" = mod ] && m=$(basename "$(dirname "$f")")
  [ "$m" = main ] && continue
  grep -qE "^(#\[[^]]*\] *)?mod $m;" src/main.rs || { echo "lint: src/$m が src/main.rs の mod に無い（crate の中で compile されず、警告も clippy も本体では見えない）"; exit 1; }
done
pending=$(grep -c '^#\[ignore' "$ACC" || true)
echo "lint: ok。後で行う test ${pending} 件（未開始。MVP 完成後に cargo test --test acceptance -- --ignored・docs/ACCEPTANCE.md）"
