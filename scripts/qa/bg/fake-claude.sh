#!/bin/bash
# 試験の偽の claude CLI（owner の quota を使わない・model を呼ばない）。BG の run で SHIBUBU_LLM_CLI に置く物の元。
#   cp scripts/qa/bg/fake-claude.sh "$R/fake/claude"; 答えは同じ dir の interpret.json（Engine の Goal Interpretation）・generate.json（Dynamic Generation）
# 製品の llm_cli::detect が打つ --version / auth status --json には plan の login の形で答える。-p の時は --json-schema の中身で依頼を見分け、
# Engine の 2 つと、Project の計画 2 つに答える: 主デモの一言（"Set up a dev environment"・席 3 つ = Terminal・Docs の TextEdit・Diagram の Excalidraw）と
# scene 2 の一言（"Set up a band session"・席 4 つ = REAPER・Discord・zoom.us・Claude）。
# 他の依頼（Project の編集など）は exit 1。AI の失敗の時、製品は名前の表で黙って作らず「AI wasn't available」で止まる（FX-006）ので、計画に答えない偽の CLI では Project が出来ない。
# 呼ばれた依頼は同じ dir の calls.log に 1 行（時刻・種類・答えた file）。stdin（利用者の一言）は calls.<n>.stdin に写す（evidence）。
here=$(cd "$(dirname "$0")" && pwd)
case "$1" in
  --version) echo '9.9.1 (Claude Code)'; exit 0 ;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}'; exit 0 ;;
  -p) ;;
  *) echo "fake-claude: unknown arg ${1}" >&2; exit 2 ;;
esac
schema=""; prev=""
for a in "$@"; do [ "$prev" = "--json-schema" ] && schema=$a; prev=$a; done
n=$(( $(cat "$here/calls.log" 2>/dev/null | wc -l) + 1 ))
cat > "$here/calls.${n}.stdin"
case "$schema" in
  *'"workflow_steps"'*) kind=generate ;;
  *'"operations"'*) kind=interpret ;;
  *shibubu_project_plan*) if grep -q 'Set up a band session' "$here/calls.${n}.stdin"; then kind=plan_band; elif grep -q 'Set up a dev environment' "$here/calls.${n}.stdin"; then kind=plan_main; else kind=other; fi ;;
  *) kind=other ;;
esac
f="$here/${kind}.json"
if [ "$kind" = plan_band ]; then  # bundle id は /Applications の Info.plist の実物（REAPER.app・Discord.app・zoom.us.app・Claude.app）
  f="$here/plan_band.json"
  cat > "$f" <<'EOP'
{"shape":"project","name":"Band session","pages_requested":true,"seats":[
{"title":"Record","kind":"app","app_name":"REAPER","bundle_id":"com.cockos.reaper","file":null,"url":null,"install_url":null,"instead":[],"page":0,"slot":"main","why":"REAPER is where you record, so it gets the big space","feeds":null},
{"title":"Band chat","kind":"app","app_name":"Discord","bundle_id":"com.hnc.Discord","file":null,"url":null,"install_url":null,"instead":[],"page":1,"slot":"main","why":"Each app is on its own page as you asked","feeds":null},
{"title":"Call","kind":"app","app_name":"zoom.us","bundle_id":"us.zoom.xos","file":null,"url":null,"install_url":null,"instead":[],"page":2,"slot":"main","why":"Each app is on its own page as you asked","feeds":null},
{"title":"Lyrics","kind":"app","app_name":"Claude","bundle_id":"com.anthropic.claudefordesktop","file":null,"url":null,"install_url":null,"instead":[],"page":3,"slot":"main","why":"Each app is on its own page as you asked","feeds":null}]}
EOP
fi
if [ "$kind" = plan_main ]; then  # 主デモの一言: 席 3 つが Terminal / Docs / Diagram の順の Page に出る（kind は 面の種類）
  f="$here/plan_main.json"
  cat > "$f" <<'EOP'
{"shape":"project","name":"Dev environment","pages_requested":true,"seats":[
{"title":"Terminal","kind":"terminal","app_name":null,"bundle_id":null,"file":null,"url":null,"install_url":null,"instead":[],"page":0,"slot":"main","why":"A shell in this repo","feeds":null},
{"title":"Docs","kind":"app","app_name":"TextEdit","bundle_id":"com.apple.TextEdit","file":"README.md","url":null,"install_url":null,"instead":[],"page":1,"slot":"main","why":"Edit the README in TextEdit","feeds":null},
{"title":"Diagram","kind":"web","app_name":null,"bundle_id":null,"file":null,"url":"https://excalidraw.com","install_url":null,"instead":[],"page":2,"slot":"main","why":"Draw the architecture in Excalidraw","feeds":null}]}
EOP
fi
echo "$(date +%s) ${kind} $([ -f "$f" ] && echo "$f" || echo none)" >> "$here/calls.log"
[ "$kind" != other ] && [ -f "$f" ] || { echo "fake-claude: no answer for ${kind}" >&2; exit 1; }
printf '{"type":"result","subtype":"success","is_error":false,"result":"fake","structured_output":%s}\n' "$(tr -d '\n' < "$f")"
