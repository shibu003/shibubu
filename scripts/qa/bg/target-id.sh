#!/bin/bash
# target-id.sh — 「対象の取り違え」の 4 場面（README が開いている / 閉じている / 同名 2 つ / 別の書類が前面）を実機で 1 回ずつ回す（27 手の外・W15-b64）。
# 使い方:
#   bash scripts/qa/bg/target-id.sh --dry-run                           4 場面の窓の並べ方と合格を印字するだけ（.app を起こさない）
#   bash scripts/qa/bg/target-id.sh <Shibubu.app> --out DIR [--only S2]  静かな窓で。出す物 = DIR/target-id.md・target-id.json（場面ごとに「どの file の sha が変わったか」と製品の 1 行）
# 本物の AI: REHEARSE_LLM=real SHIBUBU_QA_LLM_OK=<owner の語> を付けると偽の CLI を置かず owner の claude CLI を使う（上限は台本ごとの既定（rehearse.py の LLM_BUDGET）を REHEARSE_LLM_CALLS / REHEARSE_LLM_SECS で置き換え・出す物に <場面>/llm.json）。
# exit: 0 全場面 ok / 1 赤 / 2 使い方 / 3 起こせない（claude CLI 無しを含む） / 5 owner 使用中で未測 / 6 未測あり / 8 本物の AI の owner の ok が無い。出す物と保証は target_id.py の冒頭。
set -u
BG="$(cd "$(dirname "$0")" && pwd)"
exec python3 -I "$BG/target_id.py" "$@"
