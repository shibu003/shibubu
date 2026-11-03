#!/bin/bash
# run.sh — 背景の実機試験の 1 run（YC_WAVES §1 D-f の 9 通目・D-g・実機の間の罠）。製品の依存ではない（試験の側の道具）。
#
# 呼ぶ側の使い方:
#   R=$(mktemp -d "$HOME/shibubu-yc-runs/run.XXXX")
#   bash scripts/qa/bg/run.sh start "$R" <Shibubu.app>            # 罠の環境で open -g・watch を起こす → R/run.json
#   bash scripts/qa/bg/run.sh step  "$R" 03-type type_text '{"pid":…,"element_token":"…","text":"…","session":"…"}' \
#        [--needle "<打つ文字>"] [--expect "<期待する効果の 1 行>"] [--want "<AX identifier>"]   # snap → cua → axprobe → snap → 写真 → judge → R/steps.jsonl
#   bash scripts/qa/bg/run.sh step  "$R" 05-tree ax_tree '{"pid":…,"session":"…"}'   # W12a: front-guard ax の木（AXIdentifier つき）を R/ax/05-tree.json へ（読むだけ）
#   bash scripts/qa/bg/run.sh load  "$R" 120 8                      # W12W-1c: 自分の process 8 本で CPU の負荷を 120 秒（すぐ返る）→ R/load.jsonl
#   SHIBUBU_QA_SCENE=normal_load bash scripts/qa/bg/run.sh start "$R" <Shibubu.app>   # W12W-1c: 場を宣言して起こす（下の「W12W-1c の場」・load は start の前に打てる）
#   bash scripts/qa/bg/run.sh stop  "$R"                           # watch を止め・自分の pid だけ kill・罠の印を数える → R/run.json の ok
#
# 保証する:
#   - Shibubu は `open -g -n --env SHIBUBU_BACKGROUND=1` で起こし、PATH から cua-driver の dir だけ外し、CUA_DRIVER_BIN は罠（呼んだ親 pid を R/trap/marks に書く）。
#   - cua-driver は試験の側の絶対 path（CUA_DRIVER_REAL・既定 $HOME/.local/bin/cua-driver）で呼ぶ。session 付きの element_token の手だけ通す（step_allowed）。
#   - judge が 0 以外を返したらその場で exit（run は失敗・後の手を打たない）。各手の写真は `screencapture -x -l <wid>`。
#   - stop で「印が 0 件・Shibubu を親に持つ cua-driver の process が 0」でなければ run の合格を全部取り消す（run.json ok=false）。
#   - owner は起動の前の前面（run.json owner_pid）で固定し、毎手の judge に --owner で渡す（手の間に前面を取られた後の手も赤）。
#   - stop で run 全体（起動の前の snap → stop の snap・launch と手の events を継ぎ目なく）を 1 回 judge する（steps/zz-run.verdict.json）。
#     手と手の間の前面化・Space の変化もこれで数える。起動の after は窓が出てから 3 秒待って取る（約 2 秒後の前面化・48fa952）。
# 保証しない:
#   - 期待する効果（L2）の判定は tally.py の仕事。ここでは --expect を steps.jsonl に書くだけ。
#   - WKWebView の中身は screencapture -l に写らない（写真は wincap・W12W-1）。
#   - 試験の repo の clone し直しは fresh-repo.sh（別の file）。
#
# --expect の形（W2b-3・tally.py の parse_expect が正）: "ax:<identifier か role:label>" / "ui_state:<needle>" / "oracle:<名前>" / "refused:"。
#   ui_state と oracle の手は、cua の前後で expect_oracle が R/oracle/<name>.{before,after}.<kind>.json を取る（ui_state → uistate・oracle:windows → windows・oracle:ax_windows → axwindows・oracle:git → git〈W12a・R/shibubu-yc-demo〉・
#   oracle:excalidraw → excalidraw〈W12W-2 の B1・R/shibubu-yc-demo/docs/arch.excalidraw・nonce は手の --needle、SHIBUBU_QA_NONCES〈空白区切り・B3 の 2 つ〉が在ればそれ・どちらも無ければ取らない〉・
#   oracle:github → github〈W12W-3 の B4・oracle/github.sh comments $YC_DEMO_REPO $SHIBUBU_QA_GH_ISSUE <--needle>・判定は tally.py の b4_left〉）。
#
# W2b-AX（AX の窓が常に 1 個在る）: start が `front-guard ax-sample --pid <shibubu> --out R/ax-windows.jsonl --stop R/ax.stop`
#   を watch と並べて起こし、stop が止めて `front-guard ax-judge --samples R/ax-windows.jsonl > R/ax-windows.verdict.json` を取る。
#   detached（exit 1）なら run.json ok=false・unresponsive だけ（exit 6）も ok=false（測れない＝緑にしない）。SHIBUBU_QA_VARIANT=<build の名前> で比べる build を記録に残す。
#   読めない --expect は何も打たずに exit 2。
#
# W12W-1c: step（と start の 00-launch）は cua-driver の手の直後に bin/axprobe --pid <run.json の pid> --step <name> [--want <AX identifier>] を呼び
#   （AXPROBE=<別の binary> で差し替え・test.sh の偽物）、R/steps/<name>.axprobe.json（shibubu.qa.axprobe/1）に書いて steps.jsonl の axprobe 欄に path を入れる。
#   --want は step の --want（AX identifier・末尾 * で前方一致）。切り分け（製品の欠陥 / 環境 / 未測・cua-driver の判定は別）は tally.py の cause_of。
#   axprobe が 0・3（許可無しの記録）以外で終わった・何も書かなかった手は file を残さず axprobe 欄は null（cause_of で no_probe・緑にしない）。
#
# W12W-1c の場（同じ手を 3 つの場で回して判定を比べる・YC_WAVES §5.3 の W12W-1c・同じ手の通しは drive-w12w1c.py）:
#   - start は起こす前に 1 回だけ owner_idle.py（pointer の idle と、自分の type_text を除いた key の idle・W15-b42）を読み、run.json の owner_idle_s に書く。120 秒（OWNER_IDLE_MIN_S）未満か読めない時は
#     owner が使用中（居ないと言えない）: watch も open も打たず・待たず、run.json に owner_active=true・ok=false を書いて exit 6（tally は unmeasured.owner_active）。
#     その run に step は打てない（exit 6・何も書かない）・stop は load の残りを kill するだけで exit 6。owner の gate は start だけ（load は gate を持たない・c2d4e40 の契約）。
#   - start は bin/axprobe（AXPROBE）が実行可能でなければ何も起こさず exit 2（W15-b39・手ごとの idle が無い run は始めない）。
#   - SHIBUBU_QA_SCENE=<normal_low|normal_load|fullscreen|split_view> を run.json の scene に書く（無ければ null・知らない値は起こさず exit 2）。
#     fullscreen・split_view の Space は run の前に operator が作り、終わったら元の Space に戻す（run.sh は Space を変えない）。宣言が手の axprobe の space と
#     load の窓に合うかは tally.py の scene_held（合わない run をその場の判定に数えない）。
#   - load <R> <秒> <本数>: 試験の側が自分の process で CPU の負荷を作る（<本数> 本の busy loop・各々が <秒> で自分で終わる・<秒> は 1〜600・<本数> は 1〜32・画面・入力・前面に触らない）。
#     R/load.jsonl に shibubu.qa.load/1（t0・t1・seconds・procs・pids）を 1 行足してすぐ返る（手はその間に打つ）。stop は load.jsonl の pid のうち残っている自分の物
#     （argv に LOAD_MARK の在る bash）だけ kill（start が run.json の前で落ちた run でも・その時の stop は exit 2）。owner が使用中の時に負荷を作らないのは呼ぶ側
#     （drive-w12w1c.py が打つ前に owner_active "$(owner_idle_s)" を見る）。
#     normal_load は probe の在る手が全部どれかの窓の中である事（R が在れば start の前に打てる = 00-launch も入る・600 秒を超える run は load を重ねる）。
#     ponytail: 終わりは bash の SECONDS（1 秒刻み）で見るので、各 loop は t1 の後 1 秒以内に終わる（窓 t0〜t1 の間は必ず回っている）。
#
# 録画 mode（DEMO-5・SHIBUBU_QA_RECORD=1・record.sh が立てる）: owner が机に居る前で画面を録る。測るのをやめず、止めるのをやめる。
#   - start の HIDIdleTime の gate は owner_idle_s に記録するだけ（起こす）・open は -g 無し（前面に出して録画に映す）・起動の途中で前面を取っても kill しない。
#   - 手ごとの写真と judge は飛ばす（B64-TOOL 5・録画の長さに入る）。verdict は unmeasured.record_mode=not_judged（tally は unmeasured(record_mode)）。
#     run 全体の judge（zz-run）は取り、stop は 0 で返る＝録画・判定外。罠の印・cua の子は従来どおり 7。env 無しは 1 bit も変わらない。
#
# exit: 0 ok / 1 violation（front-guard の判定・その場で stop して止まる）/ 6 未測あり（run は ok=false のまま続けられる・owner が使用中で起こさなかった run）/ 7 罠に印 / 8 拒否した手（座標・前面を変える tool）/ 2 使い方 / 3 起動できない

set -u
BG="$(cd "$(dirname "$0")" && pwd)"
CUA_DRIVER_REAL="${CUA_DRIVER_REAL:-$HOME/.local/bin/cua-driver}"
FRONT_GUARD="${FRONT_GUARD:-$BG/bin/front-guard}"
AXPROBE="${AXPROBE:-$BG/bin/axprobe}"
snap() { "$FRONT_GUARD" snap "$@"; }

# app_sha <app>: MacOS の下の file の sha。path の綴り（$HOME/… と /Users/… と相対）で変わらないよう、実体の path（pwd -P）で数える（W15-b46）
app_sha() { local d; d=$(cd "$1/Contents/MacOS" && pwd -P) || return 1; find "$d" -type f -exec shasum -a 256 {} + | sort -k2 | shasum -a 256 | cut -c1-64; }

# probe_step <R> <name> [want]: 手の直後の AX の切り分けの記録（W12W-1c）。書けなければ file を残さない（tally は no_probe）。
probe_step() {
  local R=$1 name=$2 want=${3:-} rc
  "$AXPROBE" --pid "$(run_get "$R" pid)" --step "$name" ${want:+--want "$want"} > "$R/steps/$name.axprobe.json" 2> "$R/steps/$name.axprobe.err"; rc=$?
  { [ $rc -eq 0 ] || [ $rc -eq 3 ]; } && [ -s "$R/steps/$name.axprobe.json" ] || { rm -f "$R/steps/$name.axprobe.json"; echo "run.sh: $name の axprobe が書けない（rc=${rc}）" >&2; }
  [ -s "$R/steps/$name.axprobe.err" ] || rm -f "$R/steps/$name.axprobe.err"
  return 0
}

# 通す cua-driver の tool（読むか、element_token で 1 つの element に効く物だけ）。
# ax_tree は cua-driver ではなく試験の側の front-guard ax（AXIdentifier つきの木・読むだけ・W12a: cua-driver の木は identifier を返さない）。
STEP_TOOLS_ALLOWED=(get_window_state list_windows ax_tree click type_text set_value press_key scroll)
# 通さない tool（前面・cursor・窓の位置を変える物）。
# invoke_menu も通さない（反証の直し・9 通目「私の computer 操作を邪魔しないで」）: element_token を持たず、menu bar を開いて owner の画面に menu を出す。
# W16-2 run c（01:21:45）で TextEdit の File の menu が owner の画面に約 6 分開いたまま残った。保存・Undo を BG で打つ道はまだ無い（未測と書く）。
STEP_TOOLS_DENIED=(launch_app activate_app focus_app move_cursor drag keep_front invoke_menu)

# 録画 mode か（上の「録画 mode」）
rec() { [ "${SHIBUBU_QA_RECORD:-}" = 1 ]; }
die() { echo "run.sh: $*" >&2; exit "${2:-2}"; }
py() { python3 -I -c "$@"; }
now() { py 'import time;print(time.time())'; }
# W12W-1c の owner の gate（start の）: owner が最後に入力してからの秒（IOHIDSystem の HIDIdleTime・ns）。読めなければ空。test.sh が source して差し替える・drive-w12w1c.py も source して load の前に読む
OWNER_IDLE_MIN_S=120
# 手を打つ前の 1 分 load（sysctl vm.loadavg = "{ 1分 5分 15分 }"）。steps.jsonl の load1 になる（tally が > CONGESTED_LOAD1 の手を混雑と印を付ける）。読めなければ空。test.sh が差し替える
load1_now() { sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}'; }
# owner_idle_s [R]: owner が最後に人として入力してからの秒（owner_idle.py = mouse と、自分の type_text を除いた key・W15-b42。HIDIdleTime は自分の打鍵でも戻るので使わない）。
# R を渡すと、同じ dir に並ぶ他の run（run.* / demo* / *-restart / --out の兄弟）の steps.jsonl の key post を「自分の打鍵」として除く（同じ窓の 2 run 目の start）。読めなければ空。test.sh が差し替える
owner_idle_s() { AXPROBE="$AXPROBE" python3 -I "$BG/owner_idle.py" --steps ${1:+"${1%/*}"/*/steps.jsonl} 2>/dev/null; }
# owner_active <秒か空>: 0 = 使用中（OWNER_IDLE_MIN_S 未満か読めない＝居ないと言えない）。QW_OPENED_AT（quiet-window.sh が窓を開けた epoch）が在れば、
# 窓を開けてからの経過より idle が短い = 窓の途中で人の入力が在った → これも使用中（窓の開始の基準・W15-b42）
owner_active() { [ -z "$1" ] || awk -v s="$1" -v m="$OWNER_IDLE_MIN_S" -v o="${QW_OPENED_AT:-}" -v n="$(date +%s)" 'BEGIN { if (o != "" && n - o > m) m = n - o; exit !(s + 0 < m + 0) }'; }
# W12W-1c の load の busy loop の印（argv）。stop はこの印の在る pid だけ kill する（pid の使い回しで他人を kill しない）
LOAD_MARK=shibubu-qa-load
LOAD_PROCS_MAX=32
wid_of() { "$BG/bin/windows" once --pid "$1" | py 'import json,sys;w=[x for x in json.load(sys.stdin)["windows"] if x["layer"]==0 and x["onscreen"]];print(w[0]["window_id"] if w else "")'; }
# B65: `lsappinfo front` は「ASN:0x0-48048:」を返し、`lsappinfo info` はその形に空を返す（前面が毎回空＝起動中に前面を取った自分の shibubu を kill しなかった）。
# 「ASN:0x0-0x48048:」に直して聞く（front-guard snap は 1 回 約 0.3 秒で、0.1 秒おきの起動の見張りの刻みが 4 倍に延びる）
front_pid() { lsappinfo info -only pid "$(lsappinfo front | sed 's/^\(ASN:0x[0-9a-f]*-\)\([0-9a-f]*:\)$/\10x\2/')" | sed -n 's/.*= *\([0-9]*\).*/\1/p'; }
# run.json の欄を書き換える（key=JSON値 の列）。
run_set() { local R=$1; shift; py 'import json,sys;p=sys.argv[1];d=json.load(open(p));[d.__setitem__(k,json.loads(v)) for k,v in (a.split("=",1) for a in sys.argv[2:])];open(p,"w").write(json.dumps(d,ensure_ascii=False,sort_keys=True)+"\n")' "$R/run.json" "$@"; }
run_get() { py 'import json,sys;v=json.load(open(sys.argv[1])).get(sys.argv[2]);print(" ".join(map(str,v)) if isinstance(v,list) else ("" if v is None else v))' "$1/run.json" "$2"; }
# events の最後の行の t が $2 以上になるまで待つ（judge の watcher の生存の条件・最大 3 秒）。最後の行だけ読む（全部読むと run が進むほど遅い・W15-b59）。
wait_events() { local f=$1 t=$2 i; for i in $(seq 30); do tail -n 1 "$f" 2>/dev/null | py 'import json,sys;l=sys.stdin.read().strip();sys.exit(0 if l and json.loads(l)["t"]>=float(sys.argv[1]) else 1)' "$t" 2>/dev/null && return 0; sleep 0.1; done; return 1; }

trap_env() {
  local R=$1 d out=""
  mkdir -p "$R/trap" "$R/data"
  printf '#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap/marks"\nexit 1\n' "$R" > "$R/trap/cua-driver"
  chmod +x "$R/trap/cua-driver"
  # PATH から cua-driver の在る dir だけ外す（他は保つ）。/Applications/CuaDriver.app は owner の物で動かせないので、製品が先に読む CUA_DRIVER_BIN の罠で捕まえる。
  local IFS=:
  for d in $PATH; do [ -x "$d/cua-driver" ] || out="${out:+$out:}$d"; done
  echo "SHIBUBU_BACKGROUND=1"; echo "SHIBUBU_DATA_DIR=$R/data"; echo "KANGAE_DATA_DIR=$R/data"
  echo "CUA_DRIVER_BIN=$R/trap/cua-driver"; echo "PATH=$out"
  # W13: 製品は利用者が login 済みの LLM の CLI（claude -p）を見つけると生成・編集のたびに呼ぶ（owner の plan の quota）。既定は切る
  # （SHIBUBU_LLM_CLI を空 = 製品は使わず「LLM の CLI が使えません」の 1 行）。quota の承認のある run だけ SHIBUBU_QA_LLM=1 で製品に探させ、
  # 呼ぶ側が SHIBUBU_LLM_CLI に絶対 path（数える包み）を置けばそれを渡す。回数は $R/data/llm-calls.jsonl の行（W13 の行の上限）
  if [ "${SHIBUBU_QA_LLM:-}" = 1 ]; then
    # G76: 製品の engine-trace.jsonl は SHIBUBU_QA_LLM=1 の時だけ書く。open の env の引き継ぎに頼らず明示で渡す
    echo "SHIBUBU_QA_LLM=1"
    [ -z "${SHIBUBU_LLM_CLI:-}" ] || echo "SHIBUBU_LLM_CLI=$SHIBUBU_LLM_CLI"
  else
    echo "SHIBUBU_LLM_CLI="
  fi
  # W14: 生成と修正の上限（llm::BUDGET_ENV・calls=<n>,secs=<n>）。呼ぶ側が置いた時だけ渡す（無ければ製品の既定 10 回 600 秒）
  [ -z "${SHIBUBU_LLM_BUDGET:-}" ] || echo "SHIBUBU_LLM_BUDGET=$SHIBUBU_LLM_BUDGET"
  # W2b-AX の比べ: 呼ぶ側が KANGAE_REDUCE_MOTION を置いた時だけ渡す（無ければ製品の既定のまま）
  [ -z "${KANGAE_REDUCE_MOTION:-}" ] || echo "KANGAE_REDUCE_MOTION=$KANGAE_REDUCE_MOTION"
  # W15-b49: e0 の Mirror の FrameLog（呼ぶ側が置いた時だけ渡す・製品の KANGAE_MIRROR_FRAME_LOG）
  [ -z "${KANGAE_MIRROR_FRAME_LOG:-}" ] || echo "KANGAE_MIRROR_FRAME_LOG=$KANGAE_MIRROR_FRAME_LOG"
  # 初期画面（app.rs の SHIBUBU_START・旧 canvas の Desktop パネルは compat でしか開けない）: 呼ぶ側が置いた時だけ渡す
  [ -z "${SHIBUBU_START:-}" ] || echo "SHIBUBU_START=$SHIBUBU_START"
  # W19-2 の配線: Engine が起こしてよい MCP の server（mcp::load_engine_config）。既定は run の中の空の設定 = owner の ~/.shibubu/mcp.json の server を
  # 試験の run から起こさない（Connections の MCP の行にも owner の HOME の path を出さない）。呼ぶ側が SHIBUBU_MCP_CONFIG を置けばそれ
  [ -f "$R/mcp-empty.json" ] || { mkdir -p "$R" && printf '{"mcpServers":{}}\n' > "$R/mcp-empty.json"; }
  echo "SHIBUBU_MCP_CONFIG=${SHIBUBU_MCP_CONFIG:-$R/mcp-empty.json}"
  # 製品の Terminal の zsh は app の env を継ぐ。run の中の ZDOTDIR で、録画に映る prompt を「dir 名 %」にし（owner の user 名・機械名を映さない）、
  # driver が打つ echo を owner の ~/.zsh_history に書かない。owner の ~/.zshrc は読まない（PATH は上で渡している）
  mkdir -p "$R/zdot" && printf '%s\n' "PROMPT='%1~ %# '" 'HISTFILE="$ZDOTDIR/.zsh_history"' > "$R/zdot/.zshrc"
  echo "ZDOTDIR=$R/zdot"
  # bash が Terminal の shell の機械（/etc/bashrc の既定 PS1 = 「<機械名>:<dir> <user>$」）: env の PS1 は login でない bash だけがそのまま使う
  echo 'PS1=$ '
  # B71: 製品の Terminal は login shell（TerminalConfig の -l -i）なので、bash は /etc/profile → /etc/bashrc が env の PS1 を既定に戻す（cu.vKn8 の ax 16〜18 に利用者名と機械名）。
  # 録画 mode だけ、prompt を出す直前に毎回 PS1 を短くする（/etc/bashrc は PROMPT_COMMAND を書かない・Apple_Terminal の rc は前に足すだけ）。owner の rc・PATH は今のまま
  rec && echo "PROMPT_COMMAND=PS1='\$ '"
  # W3: 主デモの「この repo」（呼ぶ側の env に在れば渡す・project::WORKSPACE_ENV）
  [ -z "${SHIBUBU_WORKSPACE_DIR:-}" ] || echo "SHIBUBU_WORKSPACE_DIR=$SHIBUBU_WORKSPACE_DIR"
  # W16-2: 混んだ機械で bridge の AX の待ち（製品の既定 3 秒）を伸ばす試験の環境の値（呼ぶ側の env に在れば渡す・log に書く）
  [ -z "${KANGAE_BRIDGE_AX_TIMEOUT:-}" ] || echo "KANGAE_BRIDGE_AX_TIMEOUT=$KANGAE_BRIDGE_AX_TIMEOUT"
  # W12W-1b: 試験の側が 1 回だけ Recoverable を起こす手（surface::WEB_FAULT_ENV・値は start-once だけ・owner の network に触らない）
  [ -z "${SHIBUBU_WEB_FAULT:-}" ] || echo "SHIBUBU_WEB_FAULT=$SHIBUBU_WEB_FAULT"
  # R-M の D-f: 製品の FrameLog（E0 の frame_latency の半分）。呼ぶ側が置いた時だけ渡す
  [ -z "${KANGAE_MIRROR_FRAME_LOG:-}" ] || echo "KANGAE_MIRROR_FRAME_LOG=$KANGAE_MIRROR_FRAME_LOG"
  # W16M-3: 試験の側の偽の Mirror の helper（ScreenCaptureKit を使わずに Mirror の矩形を映っている状態にして、AX の木を見る時だけ）。呼ぶ側が置いた時だけ渡す
  [ -z "${KANGAE_MIRROR_HELPER:-}" ] || echo "KANGAE_MIRROR_HELPER=$KANGAE_MIRROR_HELPER"
  return 0
}

step_allowed() {
  local tool=$1 json=$2
  py 'import json,sys
tool,raw,allowed,denied=sys.argv[1],sys.argv[2],sys.argv[3].split(),sys.argv[4].split()
def no(m): print("run.sh: 拒否: "+m,file=sys.stderr); sys.exit(8)
if tool in denied or tool not in allowed: no(tool+" は通さない tool")
try: a=json.loads(raw)
except Exception as e: no("JSON でない: %s"%e)
if not isinstance(a,dict): no("JSON が object でない")
bad={"x","y","delivery","bring_to_front","activate","keep_front","foreground"}&set(a)
if bad: no("座標・前面の鍵 %s"%sorted(bad))
if not a.get("session"): no("session が無い")
if "pid" not in a: no("pid が無い")
if tool not in ("get_window_state","list_windows","ax_tree") and not a.get("element_token"): no(tool+" に element_token が無い")' "$tool" "$json" "${STEP_TOOLS_ALLOWED[*]}" "${STEP_TOOLS_DENIED[*]}"
}

# 自分の run が起こした process か: open -g --env で渡した SHIBUBU_DATA_DIR が R/data の物だけ。
# 同じ秒に別の run が起こした shibubu を拾わない（10-22 axfix の negnap と yc-s9 の w9.EjpY が互いの pid を拾い、stop が相手の app も止めた）。
mine() { ps -E -ww -o command= -p "$1" 2>/dev/null | tr ' ' '\n' | grep -qxF "SHIBUBU_DATA_DIR=$2/data"; }

# cua_session <session> <start_session の答え> <cursor off の答え>（B67）: cua-driver の session を起こし agent cursor を消す（run の始めと、途中で切れた時）。
# cua-driver は 5 分使わない session を終わらせ、終わった名の手を rc 1「session has ended; tool call '…' was rejected. Call start_session …」で拒み続ける（cu.kGB1）。agent cursor は session ごと
cua_session() {
  "$CUA_DRIVER_REAL" call start_session "{\"session\":\"$1\"}" > "$2" 2>&1
  "$CUA_DRIVER_REAL" call set_agent_cursor_enabled "{\"session\":\"$1\",\"enabled\":false}" > "$3" 2>&1
}

cmd_start() {
  local R=$1 app=$2 envs=() line p new="" stole=0 i
  [ -d "$app" ] || die "app が無い: $app" 3
  [ -x "$AXPROBE" ] || die "axprobe が無い（${AXPROBE}）: この run は始めない（手ごとの hid_idle_s が消え owner_returned を読み分けられない）。bash scripts/qa/bg/build.sh を先に" 2
  local scene=${SHIBUBU_QA_SCENE:-} idle active=false
  case $scene in ""|normal_low|normal_load|fullscreen|split_view) ;; *) die "知らない SHIBUBU_QA_SCENE: ${scene}（normal_low|normal_load|fullscreen|split_view・起こさない）";; esac
  # 起こす前に 1 回だけ（待たない）。使用中なら watch も open も打たない
  idle=$(owner_idle_s "$R"); owner_active "$idle" && active=true
  rec && active=false  # 録画 mode: owner が居る前提（idle は owner_idle_s に残る）
  mkdir -p "$R/steps" "$R/shots" "$R/ax" "$R/oracle"
  while IFS= read -r line; do envs+=(--env "$line"); done < <(trap_env "$R")
  local before_pids=" $(pgrep -x shibubu | tr '\n' ' ') $(pgrep -x Shibubu-launcher | tr '\n' ' ') "
  local sha; sha=$(app_sha "$app")
  py 'import json,sys,time;open(sys.argv[1],"w").write(json.dumps({"schema":"shibubu.qa.run/1","app":sys.argv[2],"app_sha256":sys.argv[3],"pid":0,"window_id":None,"data_dir":sys.argv[4],"started_at":time.time(),"stopped_at":None,"trap_marks":None,"cua_children_of_shibubu":None,"ok":None,"session":sys.argv[5],
 "scene":sys.argv[6] or None,"owner_idle_s":float(sys.argv[7]) if sys.argv[7] else None,"owner_active":sys.argv[8]=="true"},sort_keys=True)+"\n")' \
    "$R/run.json" "$app" "$sha" "$R/data" "qa-$(basename "$R")-$(printf '%s %s' "$(cd "$R" && pwd -P)" "$(now)" | shasum | cut -c1-8)" "$scene" "$idle" "$active"  # B67: run ごとに別の名（末尾が同じ out の前の run が終わらせた名を継がない）
  rec && run_set "$R" record_mode=true
  [ $active = true ] && { run_set "$R" ok=false; die "owner が使用中（idle ${idle:-読めない} 秒 < ${OWNER_IDLE_MIN_S}${QW_OPENED_AT:+・窓を開けてからの経過より短い}）: 起こさない・待たない（未測・run.json owner_active=true）。owner の離席の後（idle ≥ ${OWNER_IDLE_MIN_S} 秒）に新しい run-dir で start し直す" 6; }
  # 起動の手は launch.events.jsonl（suspect の pid がまだ無い）。手の events は pid が決まってから events.jsonl に別の watch で取る。
  "$FRONT_GUARD" watch --out "$R/launch.events.jsonl" --stop "$R/launch.stop" & local w0=$!
  wait_events "$R/launch.events.jsonl" 0 || die "watch が起きない" 3
  snap --out "$R/steps/00-launch.before.json" || die "snap に失敗" 3
  run_set "$R" "owner_pid=$(py 'import json,sys;f=json.load(open(sys.argv[1])).get("front");print(json.dumps(f and f["pid"]))' "$R/steps/00-launch.before.json")"
  # SHIBUBU_QA_APP_ARGS: 試験の起動にだけ渡す引数（例 `-NSAppSleepDisabled YES` = その process の引数の domain だけで App Nap を外す・owner の defaults は書かない）
  # shellcheck disable=SC2086
  open $(rec || echo -g) -n "${envs[@]}" --stdout "$R/app.log" --stderr "$R/app.log" "$app" ${SHIBUBU_QA_APP_ARGS:+--args $SHIBUBU_QA_APP_ARGS} || die "open -g に失敗" 3
  [ -n "${SHIBUBU_QA_APP_ARGS:-}" ] && run_set "$R" "app_args=$(py 'import json,sys;print(json.dumps(sys.argv[1]))' "$SHIBUBU_QA_APP_ARGS")"
  local settle=0 w=""
  for i in $(seq 130); do
    for p in $(pgrep -x shibubu) $(pgrep -x Shibubu-launcher); do case "$before_pids $new " in *" $p "*) ;; *) mine "$p" "$R" && new="$new $p";; esac; done
    # 起動の途中で前面を取ったら、その場で自分の pid だけ kill（owner の操作を邪魔しない・guard.sh の役）
    rec || { p=$(front_pid); case " $new " in *" $p "*) [ -n "$p" ] && { echo "run.sh: 起動した $p が前面を取った → kill" >&2; kill $new; stole=1; break; };; esac; }
    p=$(pgrep -x shibubu | while read -r q; do case "$new" in *" $q"*) echo "$q";; esac; done | head -1)
    # 窓が出てからも 3 秒は前面を見張る（起動の約 2 秒後に前面を取った実測がある）
    [ -n "$p" ] && [ -z "$w" ] && w=$(wid_of "$p")
    [ -n "$w" ] && { settle=$((settle + 1)); [ $settle -ge 30 ] && break; }
    sleep 0.1
  done
  [ $stole = 1 ] || [ -n "$p" ] || { kill $new 2>/dev/null; die "shibubu の pid が出ない（10 秒）" 3; }
  snap --out "$R/steps/00-launch.after.json" ${p:+--pid "$p"}
  wait_events "$R/launch.events.jsonl" "$(py 'import json,sys;print(json.load(open(sys.argv[1]))["t"])' "$R/steps/00-launch.after.json")"
  # 開始時の対象の見え方 1 行（start.txt・hidden / 窓 0 なら注意を stderr にも。止めない・unhide もしない・W15-b70）
  py 'import json,sys
R=sys.argv[1];s=json.load(open(R+"/steps/00-launch.after.json"));f=s.get("front") or {};ts=s.get("targets") or []
line="front="+str(f.get("name") or "none")+" "+" ".join("target "+str(t["pid"])+" "+str(t.get("name") or "?")+" hidden="+str(t["hidden"]).lower()+" onscreen_windows="+str(t["onscreen_windows"]) for t in ts)
open(R+"/start.txt","w").write(line+"\n")
if any(t["hidden"] or t["onscreen_windows"]==0 for t in ts): print("run.sh: 注意: 対象の app が hidden / 画面に窓が無い（AX の手は rc 0 で何も起きない・赤は未測(hidden)に倒す）: "+line,file=sys.stderr)' "$R" 2>&1 >/dev/null | head -2 >&2
  run_set "$R" pid="${p:-0}" window_id="$( [ $stole = 1 ] && echo null || echo "${w:-null}")" "suspects=[$(echo $new | tr ' ' ',')]"
  [ $stole = 1 ] && { touch "$R/launch.stop"; wait $w0; judge_step "$R" 00-launch "$R/launch.events.jsonl" open '{}' 0 "" ""; exit 1; }
  # 手の watch が起きてから launch の watch を止める（継ぎ目に穴を空けない）
  "$FRONT_GUARD" watch --out "$R/events.jsonl" --stop "$R/watch.stop" $(for q in $new; do echo --suspect $q; done) >/dev/null 2>&1 &
  run_set "$R" "watch_pid=$!"
  wait_events "$R/events.jsonl" 0 || die "watch が起きない" 3
  # W2b-AX: AX の窓の数を run の間ずっと（1 秒ごと）
  "$FRONT_GUARD" ax-sample --pid "$p" --out "$R/ax-windows.jsonl" --stop "$R/ax.stop" ${SHIBUBU_QA_VARIANT:+--variant "$SHIBUBU_QA_VARIANT"} >/dev/null 2>>"$R/ax-sample.err" &
  run_set "$R" "ax_sample_pid=$!" "variant=$(py 'import json,sys;print(json.dumps(sys.argv[1] or None))' "${SHIBUBU_QA_VARIANT:-}")"
  touch "$R/launch.stop"; wait $w0
  probe_step "$R" 00-launch
  cua_session "$(run_get "$R" session)" "$R/steps/00-session.json" "$R/steps/00-cursor-off.cua.json"
  judge_step "$R" 00-launch "$R/launch.events.jsonl" open '{}' 0 "" ""
}

# owner_pointer <verdict> step|run <tool> <R> <name> <lo>: owner が pointer を動かしただけの cursor の赤を未測にする（W15-b62）。直した時だけ 0。
#   step = 手の violation が cursor の動きだけ（front の app の替わり・他の検出が混じれば対象外）・手が pointer を使わない（click / scroll でない。click / scroll は cua.json が route accessibility・delivery background = AX で押した時だけ・W15-b65）・
#   その手の axprobe の mouse_idle_s < probe の t - 手の before（= 手の間に HID の pointer の入力が在った。cua の BG の手は mouse_idle_s を戻さない: r13 の type_text 122 本が mouse 20 秒超）。
#   run = run 全体の judge（zz-run）の violation が cursor の動きだけで、手のどれかが owner_pointer になっている時（手の間の動きも owner の pointer）。
#   verdict は violations 空・unmeasured.front=owner_pointer・ok=false に書き換える（元の detail は owner_pointer 欄に残す）。tally は owner_returned.<step> の未測にする。
owner_pointer() {
  py 'import json,os,sys,glob
vf,mode,tool,R,name,lo=sys.argv[1:7]
try:
    v=json.load(open(vf))
    vs=v.get("violations") or []
    if not vs or any(x.get("detection")!="front" or not str(x.get("detail","")).startswith("cursor") for x in vs): sys.exit(1)
    if mode=="step":
        route=None
        if tool in ("click","scroll"):  # AX で押した手（cua.json が route accessibility・background）だけ分ける。pointer の道・cua.json が無い / 読めない手は赤のまま（W15-b65）
            c=json.load(open(f"{R}/steps/{name}.cua.json"))
            route=c["route"]
            if route!="accessibility" or c["delivery"]["mode"]!="background": sys.exit(1)
        p=json.load(open(f"{R}/steps/{name}.axprobe.json")); m=p["pressure"]["mouse_idle_s"]; t=p["t"]
        if isinstance(m,bool) or not isinstance(m,(int,float)) or isinstance(t,bool) or not isinstance(t,(int,float)) or not m<t-float(lo): sys.exit(1)
        why={"mouse_idle_s":m,"gap_s":t-float(lo),"route":route}
    else:
        if not any(((json.load(open(f)).get("unmeasured") or {}).get("front")=="owner_pointer") for f in glob.glob(f"{R}/steps/*.verdict.json") if os.path.getsize(f)): sys.exit(1)
        why={"run":True}
except (OSError,ValueError,KeyError,TypeError,AttributeError): sys.exit(1)
v["owner_pointer"]=dict(why,was=[x["detail"] for x in vs]); v["violations"]=[]; v["ok"]=False
v["measured"]=[x for x in v.get("measured") or [] if x!="front"]; v["unmeasured"]=dict(v.get("unmeasured") or {},front="owner_pointer")
open(vf,"w").write(json.dumps(v,ensure_ascii=False,sort_keys=True)+"\n")' "$@"
}

# target_hidden <verdict> <R> <name>: 手の対象の app が hidden / 窓 0 の間の赤を未測にする（W15-b70）。直した時だけ 0。
#   見るのは手の before の snapshot の targets（snap --pid: 手の json の pid）。無い手（pid の無い手・targets の無い古い snapshot）は赤のまま。
#   hidden の app は AX の手が rc 0 でも何も起きない（G74e: Discord が ⌘H の間 AXShowMenu が効かず unhide で遅れて出た）= 製品の不具合でなく測れていない。
#   verdict は violations 空・unmeasured.target=hidden・ok=false に書き換える（元の violation は target_hidden 欄）。unhide も前面に出しもしない。
target_hidden() {
  py 'import json,sys
vf,R,name=sys.argv[1:4]
try:
    v=json.load(open(vf))
    vs=v.get("violations") or []
    if not vs: sys.exit(1)
    ts=[t for t in json.load(open(f"{R}/steps/{name}.before.json")).get("targets") or [] if t["hidden"] is True or t["onscreen_windows"]==0]
    if not ts: sys.exit(1)
except (OSError,ValueError,KeyError,TypeError,AttributeError): sys.exit(1)
v["target_hidden"]={"targets":ts,"was":[x["detail"] for x in vs]}; v["violations"]=[]; v["ok"]=False
gone={x["detection"] for x in vs}; v["measured"]=[x for x in v.get("measured") or [] if x not in gone]
v["unmeasured"]=dict(v.get("unmeasured") or {},target="hidden")
open(vf,"w").write(json.dumps(v,ensure_ascii=False,sort_keys=True)+"\n")' "$@"
}

# judge して verdict と steps.jsonl の 1 行を書き、judge の exit を返す。violation（1）ならその場で stop。
judge_step() {
  local R=$1 name=$2 ev=$3 tool=$4 json=$5 rc=$6 needle=$7 expect=$8 load1=${9:-} t0 t1 jrc=0
  # 録画 mode（B64-TOOL 5）: 手ごとの写真と judge を飛ばす（rehearse の手ごとに 1〜2 秒・録画の長さにそのまま入る・owner が机に居る）。
  # tally は全手の verdict を読むので「測っていない」verdict を書く（unmeasured.record_mode=not_judged → 緑でも guard failure でもなく unmeasured(record_mode)）。
  # run 全体の judge（stop の zz-run）は取る
  if rec; then
    py 'import json,sys;open(sys.argv[1],"w").write(json.dumps({"schema":"shibubu.qa.verdict/1","step":sys.argv[2],"ok":False,"violations":[],"unmeasured":{"record_mode":"not_judged"},"measured":[],"record_mode":True},sort_keys=True)+"\n")' \
      "$R/steps/$name.verdict.json" "$name"
  else
  # run.json の欄と before の t を 1 回の python で（手ごとに 4 回起こすと混んだ機械で秒単位・W15-b59）
  local wid o susp lo
  IFS='|' read -r wid o susp lo < <(py 'import json,sys
d=json.load(open(sys.argv[1]+"/run.json"));g=lambda k:"" if d.get(k) is None else d[k]
v=g("suspects");print("|".join(map(str,[g("window_id"),g("owner_pid")," ".join(map(str,v)) if isinstance(v,list) else v,json.load(open(sys.argv[1]+"/steps/"+sys.argv[2]+".before.json"))["t"]])))' "$R" "$name")
  [ -n "$wid" ] && screencapture -x -l "$wid" "$R/shots/$name.png" 2>/dev/null
  # events は手の窓の前後だけ切り出して渡す（全部だと run が進むほど judge が遅い・15MB で 13 秒・W15-b59。verdict は同じ = test.sh）
  python3 -I "$BG/evslice.py" "$ev" "$lo" > "$R/steps/$name.events.tmp"
  "$FRONT_GUARD" judge --step "$name" --before "$R/steps/$name.before.json" --after "$R/steps/$name.after.json" --events "$R/steps/$name.events.tmp" \
    $(for q in $susp; do echo --suspect $q; done) ${o:+--owner $o} > "$R/steps/$name.verdict.json"
  jrc=$?
  rm -f "$R/steps/$name.events.tmp"
  # cursor の赤の出所（hid / posted / warp）を verdict に足す。判定は変えない（足すだけ・W15-b68）
  [ $jrc -eq 1 ] && python3 -I "$BG/cursor_source.py" annotate "$R/steps/$name.verdict.json" "$R" "$name" "$tool" 2>/dev/null
  # 手の対象の app が hidden / 窓 0 の間の赤は赤でなく未測（exit 6・run は続ける・W15-b70）
  [ $jrc -eq 1 ] && target_hidden "$R/steps/$name.verdict.json" "$R" "$name" && { jrc=6; echo "run.sh: $name の対象の app が hidden / 画面に窓が無い: 未測(hidden)として続ける" >&2; }
  # owner が pointer を動かしただけ（pointer を使わない手の間の cursor）は赤でなく未測（exit 6・run は続ける・W15-b62）
  [ $jrc -eq 1 ] && owner_pointer "$R/steps/$name.verdict.json" step "$tool" "$R" "$name" "$lo" && { jrc=6; echo "run.sh: $name の cursor の動きは owner の pointer（mouse_idle_s < 手の間）: 未測として続ける" >&2; }
  fi
  py 'import json,sys
R,name,tool,args,rc,needle,expect,shot,load1=sys.argv[1:10]
b=json.load(open(f"{R}/steps/{name}.before.json"));a=json.load(open(f"{R}/steps/{name}.after.json"))
open(f"{R}/steps.jsonl","a").write(json.dumps({"schema":"shibubu.qa.step/1","step":name,"tool":tool,"args":json.loads(args),"rc":int(rc),"t0":b["t"],"t1":a["t"],
 "needle":needle or None,"expect":expect or None,"load1":float(load1) if load1 else None,"shot":shot if __import__("os").path.exists(f"{R}/{shot}") else None,"verdict":f"steps/{name}.verdict.json",
 "axprobe":f"steps/{name}.axprobe.json" if __import__("os").path.exists(f"{R}/steps/{name}.axprobe.json") else None},ensure_ascii=False,sort_keys=True)+"\n")' \
    "$R" "$name" "$tool" "$json" "$rc" "$needle" "$expect" "shots/$name.png" "$load1"
  [ $jrc -eq 1 ] && { echo "run.sh: $name で D-g の violation → run を止める（owner が戻った手かは tally が hid_idle_s で読み分ける: 戻っていれば未測・再試行なし）" >&2; cmd_stop "$R" >/dev/null; }
  return $jrc
}

# expect_oracle <R> <name> before|after <expect> [needle]: 期待する効果の oracle を 1 回取る（ui_state → oracle/ui-state.py read R/data --needle）。
# 保証する: 取れなければ 0 以外（tally はその手を「効果無し」と数える＝測れない手を緑にしない）。保証しない: ax の kind（AX の木は get_window_state の手で取る）。
expect_oracle() {
  local R=$1 name=$2 when=$3 kind=${4%%:*} target=${4#*:} needle=${5:-}
  mkdir -p "$R/oracle"
  case $kind in
    ui_state) python3 -I "$BG/oracle/ui-state.py" read "$R/data" --needle "$target" > "$R/oracle/$name.$when.uistate.json" ;;
    oracle) case $target in
        windows) "$BG/bin/windows" once --pid "$(run_get "$R" pid)" > "$R/oracle/$name.$when.windows.json" ;;
        # W2b-AX: AX の窓の数（AxWindowsReading）。variant は run.json の variant（start の SHIBUBU_QA_VARIANT・無ければ null）
        ax_windows) "$FRONT_GUARD" ax-windows --pid "$(run_get "$R" pid)" ${SHIBUBU_QA_VARIANT:+--variant "$SHIBUBU_QA_VARIANT"} > "$R/oracle/$name.$when.axwindows.json" ;;
        # W12a: run の clone（fresh-repo.sh の clone / init が作る R/shibubu-yc-demo）を試験の道具の git で読む
        git) bash "$BG/oracle/git.sh" read "$R/shibubu-yc-demo" > "$R/oracle/$name.$when.git.json" ;;
        # W12W-2: B1・B3 の図の file を disk から（nonce が無ければ excalidraw.sh が 2 で、半端な file は下で消す）
        # shellcheck disable=SC2086
        excalidraw) bash "$BG/oracle/excalidraw.sh" read "$R/shibubu-yc-demo/docs/arch.excalidraw" ${SHIBUBU_QA_NONCES:-$needle} > "$R/oracle/$name.$when.excalidraw.json" ;;
        # W12W-3: B4 の Issue の comment を service の側から（oracle/github.sh・gh の login・WebView とも製品の src/github.rs とも別の道）。
        #   repo は YC_DEMO_REPO（既定 shibu003/shibubu-yc-demo・fresh-repo.sh と同じ）・issue は SHIBUBU_QA_GH_ISSUE・nonce は手の --needle。issue か nonce が無ければ github.sh が 2・API の失敗は 4（半端な file は下で消す）
        github) bash "$BG/oracle/github.sh" comments "${YC_DEMO_REPO:-shibu003/shibubu-yc-demo}" "${SHIBUBU_QA_GH_ISSUE:-}" "$needle" > "$R/oracle/$name.$when.github.json" ;;
        *) echo "run.sh: oracle:$target を取る道が無い" >&2; return 4 ;;
      esac ;;
    *) return 0 ;;
  esac
  local rc=$?
  # 取れなかった時は半端な file を残さない（tally が「効果無し」と数える）
  [ $rc -eq 0 ] || { rm -f "$R/oracle/$name.$when."*.json; echo "run.sh: $name の $when の oracle を取れない（rc=${rc}）" >&2; }
  return $rc
}

# stop_poll <R> <name> <pid>: cursor の poll を止める（自分が起こした pid だけ・pkill しない）。止まらなければ kill。
stop_poll() {
  local R=$1 name=$2 p=$3 i
  [ -n "$p" ] || return 0
  touch "$R/steps/$name.cursor.stop"
  for i in $(seq 20); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$p" 2>/dev/null && kill "$p" 2>/dev/null
  rm -f "$R/steps/$name.cursor.stop"
  return 0
}

cmd_step() {
  local R=$1 name=$2 tool=$3 json=$4 needle="" expect="" want="" rc snapn=()
  shift 4
  while [ $# -gt 0 ]; do case $1 in --needle) needle=$2; shift 2;; --expect) expect=$2; shift 2;; --want) want=$2; shift 2;; *) die "知らない引数 $1";; esac; done
  [ -f "$R/run.json" ] || die "start していない: $R"
  [ "$(run_get "$R" owner_active)" = True ] && die "owner が使用中で起こしていない run（start の exit 6）: 手を打たない" 6
  step_allowed "$tool" "$json" || exit 8
  if [ -n "$expect" ]; then py 'import sys;sys.path.insert(0,sys.argv[1]);import tally;tally.parse_expect(sys.argv[2])' "$BG" "$expect" || die "--expect が読めない: $expect"; fi
  [ -n "$needle" ] && snapn=(--needle "$needle")
  # 手の対象の app の hidden・窓の数を before / after の snapshot に（json の pid・無い手は前面の app だけ・W15-b70）
  local tpid; tpid=$(py 'import json,sys;p=json.loads(sys.argv[1]).get("pid");print(p if isinstance(p,int) and not isinstance(p,bool) else "")' "$json" 2>/dev/null)
  [ -n "$tpid" ] && snapn+=(--pid "$tpid")
  local load1; load1=$(load1_now)
  [ -n "$expect" ] && expect_oracle "$R" "$name" before "$expect" "$needle"
  snap --out "$R/steps/$name.before.json" "${snapn[@]}" || exit $?
  # cursor の位置と種類ごとの最後の入力からの秒を 0.5 秒ごとに（D-g の cursor の赤の出所の分類・W15-b68）。axprobe に --poll が無ければ file が出来ず unmeasured
  local pollpid=""; rm -f "$R/steps/$name.cursor.stop"
  "$AXPROBE" --poll --out "$R/steps/$name.cursor.jsonl" --stop "$R/steps/$name.cursor.stop" >/dev/null 2>&1 & pollpid=$!
  if [ "$tool" = ax_tree ]; then
    "$FRONT_GUARD" ax --pid "$(py 'import json,sys;print(json.loads(sys.argv[1])["pid"])' "$json")" --out "$R/steps/$name.cua.json" 2>&1; rc=$?
  else
    "$CUA_DRIVER_REAL" call "$tool" "$json" > "$R/steps/$name.cua.json" 2>&1; rc=$?
    # B67: 切れていたら（owner の手の Enter を 5 分以上待った等・wait_enter の間は cua-driver を呼ばない）起こし直して 1 回だけ打ち直す。
    # 拒まれた答えは $name.ended.txt に残す。element_token の手は死んだ session の token で失敗しうる（rehearse の find が読み直す）
    if grep -q 'session has ended; tool call' "$R/steps/$name.cua.json"; then
      mv "$R/steps/$name.cua.json" "$R/steps/$name.ended.txt"
      cua_session "$(py 'import json,sys;print(json.loads(sys.argv[1])["session"])' "$json")" "$R/steps/$name.session.json" "$R/steps/$name.cursor-off.json"
      "$CUA_DRIVER_REAL" call "$tool" "$json" > "$R/steps/$name.cua.json" 2>&1; rc=$?
    fi
  fi
  # 木を返さなかった手（timeout の 1 行など）は ax に置かない（tally が壊れた木として止まらない・前後の木はその前後の手で取る）
  case $tool in get_window_state|ax_tree) [ $rc -eq 0 ] && cp "$R/steps/$name.cua.json" "$R/ax/$name.json";; esac
  # commit してよい形の木（tally.py shape: 窓の中だけ・menu bar の実名と最近使った項目を落とす）。証拠に写すのはこれだけ（生の ax/*.json は写さない）
  case $tool in get_window_state|ax_tree) [ $rc -eq 0 ] && { python3 -I "$BG/tally.py" shape "$R/ax/$name.json" > "$R/steps/$name.list.txt" || rm -f "$R/steps/$name.list.txt"; };; esac
  probe_step "$R" "$name" "$want"
  sleep 0.3
  [ -n "$expect" ] && expect_oracle "$R" "$name" after "$expect" "$needle"
  snap --out "$R/steps/$name.after.json" "${snapn[@]}" || { rc=$?; stop_poll "$R" "$name" "$pollpid"; exit $rc; }
  stop_poll "$R" "$name" "$pollpid"
  wait_events "$R/events.jsonl" "$(py 'import json,sys;print(json.load(open(sys.argv[1]))["t"])' "$R/steps/$name.after.json")"
  judge_step "$R" "$name" "$R/events.jsonl" "$tool" "$json" "$rc" "$needle" "$expect" "$load1"
}

# load <R> <秒> <本数>（W12W-1c の normal_load）: 自分の busy loop を起こし R/load.jsonl に 1 行足してすぐ返る。
cmd_load() {
  local R=$1 secs=$2 procs=$3 t0 i pids=()
  case $secs in ""|*[!0-9]*) die "load の秒は 1〜600 の整数: $secs";; esac
  case $procs in ""|*[!0-9]*) die "load の本数は 1〜$LOAD_PROCS_MAX の整数: $procs";; esac
  { [ "$secs" -ge 1 ] && [ "$secs" -le 600 ]; } || die "load の秒は 1〜600: $secs"
  { [ "$procs" -ge 1 ] && [ "$procs" -le $LOAD_PROCS_MAX ]; } || die "load の本数は 1〜$LOAD_PROCS_MAX: $procs"
  mkdir -p "$R" || die "run-dir を作れない: $R" 3
  t0=$(now)
  for i in $(seq "$procs"); do
    # fd を全部外す（呼ぶ側が出力を待って止まらない）。-le なので窓の終わり t1 まで必ず回り、その後 1 秒以内に自分で終わる
    /bin/bash -c 's=$SECONDS; while [ $((SECONDS - s)) -le "$1" ]; do :; done' "$LOAD_MARK" "$secs" </dev/null >/dev/null 2>&1 &
    pids+=($!)
  done
  py 'import json,sys;a=sys.argv;t0=float(a[2]);open(a[1],"a").write(json.dumps({"schema":"shibubu.qa.load/1","t0":t0,"t1":t0+int(a[3]),"seconds":int(a[3]),"procs":int(a[4]),"pids":[int(p) for p in a[5:]]},sort_keys=True)+"\n")' \
    "$R/load.jsonl" "$t0" "$secs" "$procs" "${pids[@]}"
  echo "run.sh: load ${procs} 本 × ${secs} 秒（pid ${pids[*]}）→ $R/load.jsonl" >&2
}

# load.jsonl の pid のうち、まだ在る自分の busy loop（argv に LOAD_MARK）だけ kill（stop と owner の未測の run）。
kill_loads() {
  local R=$1 p
  [ -f "$R/load.jsonl" ] || return 0
  for p in $(py 'import json,sys;print(" ".join(str(p) for l in open(sys.argv[1]) if l.strip() for p in json.loads(l)["pids"]))' "$R/load.jsonl"); do
    ps -o command= -p "$p" 2>/dev/null | grep -qF -- "$LOAD_MARK" && kill "$p" 2>/dev/null
  done
  return 0
}

cmd_stop() {
  local R=$1 p children=0 marks=0 w
  # start が run.json の前で落ちた（app が無い・知らない scene）run でも、先に打った自分の load は止める
  [ -f "$R/run.json" ] || { kill_loads "$R"; die "start していない: $R"; }
  # owner が使用中で起こしていない run: 判定する物が無い（ok=false のまま）。自分の負荷だけ止める
  [ "$(run_get "$R" owner_active)" = True ] && { kill_loads "$R"; echo "run.sh: stop: owner が使用中で起こしていない run（ok=false のまま・未測）" >&2; return 6; }
  # Shibubu を親に持つ cua-driver（kill の前に数える）
  for p in $(run_get "$R" suspects); do
    children=$((children + $(pgrep -P "$p" 2>/dev/null | while read -r c; do ps -o comm= -p "$c"; done | grep -c cua-driver)))
  done
  # run 全体の判定: 起動の前 → 今。events は launch（stop 行を除く）と手（start 行を除く）を t 順に継ぐ。
  snap --out "$R/steps/zz-run.after.json"
  wait_events "$R/events.jsonl" "$(py 'import json,sys;print(json.load(open(sys.argv[1]))["t"])' "$R/steps/zz-run.after.json")"
  py 'import json,sys,os
R=sys.argv[1];ev=[]
for f,drop in (("launch.events.jsonl","stop"),("events.jsonl","start")):
  p=os.path.join(R,f)
  if os.path.exists(p): ev+=[e for e in (json.loads(l) for l in open(p) if l.strip()) if e["kind"]!=drop]
open(R+"/run.events.jsonl","w").write("".join(json.dumps(e)+"\n" for e in sorted(ev,key=lambda e:e["t"])))' "$R"
  local o; o=$(run_get "$R" owner_pid)
  "$FRONT_GUARD" judge --step zz-run --before "$R/steps/00-launch.before.json" --after "$R/steps/zz-run.after.json" --events "$R/run.events.jsonl" \
    $(for p in $(run_get "$R" suspects); do echo --suspect $p; done) ${o:+--owner $o} > "$R/steps/zz-run.verdict.json" 2>/dev/null
  owner_pointer "$R/steps/zz-run.verdict.json" run "" "$R" zz-run 0  # 手のどれかが owner の pointer なら run 全体の cursor の動きも同じ（W15-b62）
  touch "$R/watch.stop"
  w=$(run_get "$R" watch_pid); for p in $(seq 30); do [ -n "$w" ] && kill -0 "$w" 2>/dev/null || break; sleep 0.1; done
  # W2b-AX: 標本を止めて判定（1 回の読みは最長 10 秒）。detached（1）も unresponsive だけ（6）も ok=false
  local ax=0 axrc=4
  touch "$R/ax.stop"; w=$(run_get "$R" ax_sample_pid); for p in $(seq 120); do [ -n "$w" ] && kill -0 "$w" 2>/dev/null || break; sleep 0.1; done
  if [ -f "$R/ax-windows.jsonl" ]; then "$FRONT_GUARD" ax-judge --samples "$R/ax-windows.jsonl" > "$R/ax-windows.verdict.json"; axrc=$?; fi
  [ $axrc -eq 0 ] && ax=1
  for p in $(run_get "$R" suspects); do kill "$p" 2>/dev/null; done
  kill_loads "$R"
  [ -f "$R/trap/marks" ] && marks=$(wc -l < "$R/trap/marks" | tr -d ' ')
  local verdicts; verdicts=$(py 'import json,sys,os
R=sys.argv[1];v=[json.load(open(os.path.join(R,json.loads(l)["verdict"]))) for l in open(R+"/steps.jsonl") if l.strip()] if os.path.exists(R+"/steps.jsonl") else []
z=R+"/steps/zz-run.verdict.json";v+=[json.load(open(z))] if os.path.exists(z) and os.path.getsize(z) else [{"ok":False,"violations":[]}]
print("viol" if any(x["violations"] for x in v) else ("un" if (not v or any(not x["ok"] for x in v)) else "ok"))' "$R")
  local ok=false; [ "$verdicts" = ok ] && [ "$marks" = 0 ] && [ "$children" = 0 ] && [ $ax = 1 ] && ok=true
  run_set "$R" "stopped_at=$(now)" "trap_marks=$marks" "cua_children_of_shibubu=$children" "ax_judge_rc=$axrc" "ok=$ok"
  echo "run.sh: stop ok=$ok steps=$verdicts trap_marks=$marks cua_children=$children ax_judge=$axrc" >&2
  { [ "$marks" != 0 ] || [ "$children" != 0 ]; } && return 7
  rec && { echo "run.sh: stop: 録画・判定外（record_mode）" >&2; return 0; }
  [ $axrc -eq 1 ] && return 1
  case $verdicts in ok) [ $ax = 1 ] && return 0 || return 6;; viol) return 1;; *) return 6;; esac
}

# source した時（test.sh）は関数だけ読む。
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0
case "${1:-}" in
  start) [ $# -eq 3 ] || die "start <run-dir> <Shibubu.app>"; cmd_start "$2" "$3" ;;
  step)  [ $# -ge 5 ] || die "step <run-dir> <name> <tool> <json> [--needle s] [--expect s] [--want id]"; shift; cmd_step "$@" ;;
  load)  [ $# -eq 4 ] || die "load <run-dir> <seconds 1〜600> <procs 1〜32>"; cmd_load "$2" "$3" "$4" ;;
  stop)  [ $# -eq 2 ] || die "stop <run-dir>"; cmd_stop "$2" ;;
  *) die "usage: run.sh start|step|load|stop …" ;;
esac
