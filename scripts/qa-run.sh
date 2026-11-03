#!/usr/bin/env bash
# Kangae の QA 台本を cua-driver の AX 経路で走らせる（書き方・読み方は docs/qa/README.md）。
#   bash scripts/qa-run.sh docs/qa/scenarios/qa-01-timer.json [...]
#   KANGAE_BIN=<Kangae の実行 file> QA_TIME_SCALE=2 bash scripts/qa-run.sh --interactive docs/qa/scenarios/*.json
# 台本ごとに一時の KANGAE_DATA_DIR で Kangae を起こし、結果（RESULTS.md・results.json・写真・AX の木）を
# docs/internal/qa-runs/<UTC>/ に置く。終了 code: 0 = 全部 PASS / 1 = FAIL あり / 3 = FAIL は無いが 未測 あり / 2 = 台本か環境の誤り。
# python は標準 library と jsonschema だけ（jsonschema が無ければ schema 検査を飛ばして意味の検査だけで走る）。PYTHON で差し替えられる。
set -euo pipefail
[ "$#" -ge 1 ] || { echo "使い方: bash scripts/qa-run.sh <scenario.json>…（docs/qa/README.md）" >&2; exit 2; }
exec "${PYTHON:-python3}" -I "$(dirname "$0")/qa_run.py" "$@"
