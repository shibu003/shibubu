#!/bin/bash
# test.sh — scripts/qa/bg の道具の検査（画面に触らない・前面も cursor も変えない・network を使わない）。
# 呼ぶ側の使い方: bash scripts/qa/bg/test.sh   （先に build.sh）。FRONT_GUARD=<別の binary> で変異を入れた build を測れる。
# 見る物: judge が D-g の 5 つ + owner_frame + leak をそれぞれ赤にする・きれいな手は緑・測れない時は 6・
#   run.sh の step_allowed が座標と前面の手を 8 で拒む・罠に印があれば stop が 7・tally の L1 / L2・github.sh の読み方と API 失敗の 4・
#   W12W-1c の cause_of（§30 の枝ごと・cua-driver の判定は別の欄）と step の axprobe の配線（AXPROBE は偽物）・本物の bin/axprobe の形・commit した run-1 の数え直し（tally.py redact の形）。
#   oracle/a1.py の A1（標本の外れ 4 種・枠との誤差 1pt・重ねた記録との食い違い・sha と nonce）・run.sh は invoke_menu を通さない（owner の画面に menu を開く）・
#   W19-3b の drive-w19-3b.py --dry-run（手の表と流れの順が合う・何も起こさない）。
# exit: 0 全部通った / 1 落ちた検査がある
set -u
export REHEARSE_LLM=fake  # 道具の試験は偽の CLI（quota を使わない）。rehearse.py の既定は real（104 通目）なので明示する
BG="$(cd "$(dirname "$0")" && pwd)"
FRONT_GUARD="${FRONT_GUARD:-$BG/bin/front-guard}"
T=$(mktemp -d "${TMPDIR:-/tmp}/qa-bg-test.XXXX")
trap 'rm -rf "$T"' EXIT
pass=0; fail=0
check() { local name=$1 want=$2 got=$3; if [ "$want" = "$got" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL $name: want $want got $got" >&2; fi; }

# ---- judge（作った snapshot と events）----
# 変える物を python の式で受け、before / after / events を書く。owner=100（窓 1 が z1）・suspect=200（窓 2 が z2 = owner の下）。
mk() {
  python3 -I - "$T" "$1" <<'EOF'
import json,sys
T,mut=sys.argv[1],sys.argv[2]
def win(i,pid,z,x=0,y=0,w=400,h=300,layer=0): return {"window_id":i,"pid":pid,"layer":layer,"bounds":{"x":x,"y":y,"w":w,"h":h},"onscreen":True,"z":z}
def snap(t): return {"schema":"shibubu.qa.snapshot/1","t":t,"front":{"pid":100,"name":"Owner"},"cursor":{"x":10,"y":10},
  "focus":{"pid":100,"role":"AXTextArea","frame":{"x":0,"y":0,"w":10,"h":10}},"windows":[win(1,100,1),win(2,200,2)],"unmeasured":{}}
b,a=snap(10.0),snap(12.0)
ev=[{"schema":"shibubu.qa.event/1","t":9.0,"kind":"start"},{"schema":"shibubu.qa.event/1","t":12.2,"kind":"z_sample","windows":[win(2,200,2)]}]
exec(mut)
json.dump(b,open(T+"/b.json","w")); json.dump(a,open(T+"/a.json","w"))
open(T+"/e.jsonl","w").write("".join(json.dumps(e)+"\n" for e in ev))
EOF
}
judge() { "$FRONT_GUARD" judge --step t --before "$T/b.json" --after "$T/a.json" --events "$T/e.jsonl" --suspect 200 > "$T/v.json"; echo $?; }
det() { python3 -I -c 'import json,sys;print(",".join(sorted({v["detection"] for v in json.load(open(sys.argv[1]))["violations"]})))' "$T/v.json"; }

mk 'pass';                                                  check "clean" 0 "$(judge)"
mk 'a["front"]={"pid":200,"name":"Shibubu"}';                check "1 front" "1 front" "$(judge) $(det)"
mk 'a["cursor"]={"x":50,"y":10}';                            check "1 cursor" "1 front" "$(judge) $(det)"
mk 'ev.insert(1,{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":200}})'; check "2 activation" "1 activation" "$(judge) $(det)"
mk 'ev.insert(1,{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":100}})'; check "2 owner re-activated is not red" 0 "$(judge)"
mk 'ev.insert(1,{"schema":"shibubu.qa.event/1","t":11,"kind":"z_sample","windows":[win(2,200,0),win(1,100,1)]})'; check "3 zorder (sample)" "1 zorder" "$(judge) $(det)"
mk 'a["windows"]=[win(2,200,0),win(1,100,1)]';               check "3 zorder (after)" "1 zorder" "$(judge) $(det)"
mk 'a["windows"]=[win(2,200,0,x=500),win(1,100,1)]';         check "3 suspect above but not overlapping" 0 "$(judge)"
mk 'ev.insert(1,{"schema":"shibubu.qa.event/1","t":11,"kind":"space_changed"})'; check "4 space" "1 space" "$(judge) $(det)"
mk 'a["focus"]["pid"]=200';                                  check "5 focus" "1 focus" "$(judge) $(det)"
mk 'a["windows"][0]["bounds"]["x"]=1';                       check "owner_frame" "1 owner_frame" "$(judge) $(det)"
mk 'b["focus"]["needle_present"]=False;a["focus"]["needle_present"]=True'; check "leak" "1 leak" "$(judge) $(det)"
mk 'b["focus"]["needle_present"]=True;a["focus"]["needle_present"]=True';  check "leak already there is not red" 0 "$(judge)"
mk 'ev=[]';                                                  check "no watcher -> unmeasured" 6 "$(judge)"
mk 'ev.append({"schema":"shibubu.qa.event/1","t":11,"kind":"stop"})'; check "watcher stopped mid-step -> unmeasured" 6 "$(judge)"
mk 'b["focus"]=None;a["focus"]=None';                        check "no focus both -> unmeasured" 6 "$(judge)"
mk 'b["unmeasured"]={"focus":"accessibility_denied"}';       check "permission -> unmeasured" 6 "$(judge)"
mk 'b["front"]=None';                                        check "no owner front -> unmeasured" 6 "$(judge)"
# 手の間に前面を取られた後の手: before も after も Shibubu が前面。owner は run の始めの記録（--owner）で比べる。
mk 'b["front"]=a["front"]={"pid":200,"name":"Shibubu"};b["windows"]=a["windows"]=[win(2,200,0),win(1,100,1)]'
"$FRONT_GUARD" judge --step t --before "$T/b.json" --after "$T/a.json" --events "$T/e.jsonl" --suspect 200 --owner 100 > "$T/v.json"
check "owner taken between steps -> front+zorder" "1 front,zorder" "$? $(det)"
# W15-b59: judge に渡す events は evslice.py で手の窓の前後だけに切る。全部渡した時と同じ verdict（古い行に 200 の activated・大きな padding）・窓の中の違反は切っても拾う
sl() { python3 -I "${1:-$BG/evslice.py}" "$T/e.jsonl" 10.0 > "$T/e.slice"; "$FRONT_GUARD" judge --step t --before "$T/b.json" --after "$T/a.json" --events "$T/e.slice" --suspect 200 | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["ok"],sorted({v["detection"] for v in d["violations"]}))'; }
full() { "$FRONT_GUARD" judge --step t --before "$T/b.json" --after "$T/a.json" --events "$T/e.jsonl" --suspect 200 | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["ok"],sorted({v["detection"] for v in d["violations"]}))'; }
PAD='ev[1:1]=[{"schema":"shibubu.qa.event/1","t":1+i*0.001,"kind":"z_sample","windows":[win(2,200,2)]} for i in range(3000)]+[{"schema":"shibubu.qa.event/1","t":3,"kind":"activated","app":{"pid":200}}]'
mk "$PAD"; check "evslice: old activation + 3000 padding lines -> same verdict as the full file" "$(full)" "$(sl)"
mk "$PAD"; check "evslice: clean step stays clean" "True []" "$(sl)"
mk "$PAD;"'ev.insert(-1,{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":200}})'; check "evslice: an activation inside the window is still red" "False ['activation']" "$(sl)"
sed 's/sys.stdout.buffer.write(first)/pass/' "$BG/evslice.py" > "$T/ev-nostart.py"
sed 's/float(sys.argv\[2\]) - 1/float(sys.argv[2]) + 100/' "$BG/evslice.py" > "$T/ev-late.py"
mk "$PAD;"'ev.insert(-1,{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":200}})'
check "evslice negative controls: start line dropped -> clean step unmeasured; cutoff after the window -> the activation is missed" "False [] False []" "$(mk "$PAD"; sl "$T/ev-nostart.py") $(mk "$PAD;"'ev.insert(-1,{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":200}})'; sl "$T/ev-late.py")"
check "evslice: a missing events file -> empty output (judge reads it as watcher_not_running)" "0 0" "$(python3 -I "$BG/evslice.py" "$T/none.jsonl" 1 | wc -c | tr -d ' ') $?"
mk 'b["front"]=a["front"]={"pid":200,"name":"Shibubu"};b["windows"]=a["windows"]=[win(2,200,0),win(1,100,1)]'  # 後の検査が前提にする fixture の状態に戻す
echo '{"schema":"shibubu.qa.event/1","t":1,"kind":"nope"}' > "$T/bad.jsonl"
"$FRONT_GUARD" judge --step t --before "$T/b.json" --after "$T/a.json" --events "$T/bad.jsonl" >/dev/null 2>&1; check "broken events -> 4" 4 $?
"$FRONT_GUARD" provoke activation >/dev/null 2>&1;            check "provoke without --fg-window -> 5" 5 $?
"$FRONT_GUARD" nope >/dev/null 2>&1;                          check "unknown sub -> 2" 2 $?

# ---- run.sh（関数だけ source する）----
# step の axprobe は偽物（本物は pid の AX を timeout まで待つ）: 呼ばれた引数を axprobe.args に書き、$T/probe.json を返す（FAKE_AXPROBE_RC で失敗）
python3 -I -c 'import json,sys
a=lambda o,c,n:{"outcome":o,"ax_code":c,"elapsed_ms":12.0,"count":n}
json.dump({"schema":"shibubu.qa.axprobe/1","t":1,"step":"x","pid":1,"trusted":True,"ax":{"windows":a("empty","success",0),"main_window":a("empty","no_value",0),
 "focused_window":a("empty","no_value",0)},"cg":[{"window_id":7,"pid":1,"layer":0,"bounds":{"x":0,"y":25,"w":800,"h":600},"onscreen":True,"z":2}],
 "space":"normal","pressure":{"load1":3.1,"ncpu":8,"memory_pressure":"normal"}},open(sys.argv[1],"w"))' "$T/probe.json"
printf '#!/bin/bash\necho "$*" >> "%s/axprobe.args"\n[ -n "${FAKE_AXPROBE_RC:-}" ] && exit $FAKE_AXPROBE_RC\ncat "%s/probe.json"\n' "$T" "$T" > "$T/fake-axprobe"; chmod +x "$T/fake-axprobe"
export AXPROBE="$T/fake-axprobe"
source "$BG/run.sh"
# mine: 同じ時に別の run が起こした shibubu を自分の物と読まない（env の SHIBUBU_DATA_DIR で見分ける）
# （/bin/sleep のような Apple の binary は ps -E が env を見せないので、自前で build した物で測る。shibubu も自前の build）
mkdir -p "$T/ra" "$T/rb"; printf '#include <unistd.h>\nint main(void){sleep(30);return 0;}\n' > "$T/nap.c"; cc -o "$T/nap" "$T/nap.c"
env SHIBUBU_DATA_DIR="$T/ra/data" "$T/nap" & pa=$!
env SHIBUBU_DATA_DIR="$T/rb/data" "$T/nap" & pb=$!
sleep 0.3
mine $pa "$T/ra"; check "mine: own run's process" 0 $?
mine $pb "$T/ra"; check "mine: other run's process is not mine" 1 $?
kill $pa $pb 2>/dev/null; wait $pa $pb 2>/dev/null
sa() { (step_allowed "$1" "$2") 2>/dev/null; echo $?; }
check "allow token click"        0 "$(sa click '{"pid":1,"element_token":"s1:2","session":"q"}')"
check "allow read state"         0 "$(sa get_window_state '{"pid":1,"window_id":3,"session":"q"}')"
check "allow ax_tree without token" 0 "$(sa ax_tree '{"pid":1,"session":"q"}')"
check "deny ax_tree without session" 8 "$(sa ax_tree '{"pid":1}')"
check "deny coordinates"         8 "$(sa click '{"pid":1,"x":10,"y":20,"session":"q"}')"
check "deny token+coordinates"   8 "$(sa click '{"pid":1,"element_token":"s1:2","x":1,"y":2,"session":"q"}')"
check "deny foreground delivery" 8 "$(sa type_text '{"pid":1,"element_token":"s1:2","text":"a","session":"q","delivery":"foreground"}')"
check "deny activate tool"       8 "$(sa activate_app '{"pid":1,"session":"q"}')"
check "deny unknown tool"        8 "$(sa hotkey '{"pid":1,"session":"q"}')"
check "deny no session"          8 "$(sa click '{"pid":1,"element_token":"s1:2"}')"
check "deny click without token" 8 "$(sa click '{"pid":1,"session":"q"}')"
check "deny invoke_menu (opens a menu on the owner's screen)" 8 "$(sa invoke_menu '{"pid":1,"window_id":3,"path":["ファイル","保存"],"session":"q"}')"
R="$T/run"; trap_env "$R" > "$T/env"
check "trap env PATH has no cua-driver" 0 "$(P=$(sed -n 's/^PATH=//p' "$T/env"); PATH=$P command -v cua-driver >/dev/null; [ $? -ne 0 ]; echo $?)"
check "trap env points CUA_DRIVER_BIN at trap" "CUA_DRIVER_BIN=$R/trap/cua-driver" "$(grep ^CUA_DRIVER_BIN "$T/env")"
check "trap env background" 1 "$(grep -c '^SHIBUBU_BACKGROUND=1$' "$T/env")"
check "trap env no workspace unless given" 0 "$(grep -c '^SHIBUBU_WORKSPACE_DIR=' "$T/env")"
check "trap env gives the Terminal's zsh a run-local ZDOTDIR: prompt is 'dir %' (no user / host name on the recording), history stays in the run" "ZDOTDIR=$R/zdot|shibubu-yc-demo %|$R/zdot/.zsh_history" "$(grep ^ZDOTDIR= "$T/env")|$(cd "$T" && mkdir -p shibubu-yc-demo && cd shibubu-yc-demo && ZDOTDIR=$R/zdot zsh -i -c 'print -rP -- "$PROMPT"; print -r -- $HISTFILE' 2>/dev/null | tr '\n' '|' | sed 's/|$//; s/% $/%/; s/% |/%|/')"
check "trap env passes workspace" "SHIBUBU_WORKSPACE_DIR=/tmp/ws demo" "$(SHIBUBU_WORKSPACE_DIR='/tmp/ws demo' trap_env "$R" | grep ^SHIBUBU_WORKSPACE_DIR)"
check "trap env turns the LLM CLI off by default" "SHIBUBU_LLM_CLI=" "$(SHIBUBU_LLM_CLI=/x/claude trap_env "$R" | grep ^SHIBUBU_LLM_CLI)"
check "trap env lets an approved run find the CLI" 0 "$(SHIBUBU_QA_LLM=1 trap_env "$R" | grep -c ^SHIBUBU_LLM_CLI)"
check "trap env passes the counting wrapper" "SHIBUBU_LLM_CLI=/x/claude" "$(SHIBUBU_QA_LLM=1 SHIBUBU_LLM_CLI=/x/claude trap_env "$R" | grep ^SHIBUBU_LLM_CLI)"
check "trap env tells an approved run to write the engine trace (G76)" 1 "$(SHIBUBU_QA_LLM=1 trap_env "$R" | grep -c '^SHIBUBU_QA_LLM=1$')"
check "trap env keeps the engine trace off by default" 0 "$(trap_env "$R" | grep -c '^SHIBUBU_QA_LLM=')"
check "trap env passes the W14 LLM budget" "SHIBUBU_LLM_BUDGET=calls=1" "$(SHIBUBU_LLM_BUDGET=calls=1 trap_env "$R" | grep ^SHIBUBU_LLM_BUDGET)"
check "trap env leaves the budget to the product by default" 0 "$(trap_env "$R" | grep -c ^SHIBUBU_LLM_BUDGET)"
check "trap env points MCP at the run's empty config" "SHIBUBU_MCP_CONFIG=$R/mcp-empty.json {\"mcpServers\":{}}" "$(grep ^SHIBUBU_MCP_CONFIG "$T/env") $(cat "$R/mcp-empty.json")"
check "trap env passes the caller's MCP config" "SHIBUBU_MCP_CONFIG=/x/mcp.json" "$(SHIBUBU_MCP_CONFIG=/x/mcp.json trap_env "$R" | grep ^SHIBUBU_MCP_CONFIG)"
# alive <R>: R/load.jsonl の pid のうち、まだ在る自分の busy loop（argv に shibubu-qa-load）の数
alive() { local n=0 p; for p in $(python3 -I -c 'import json,sys;print(" ".join(str(p) for l in open(sys.argv[1]) for p in json.loads(l)["pids"]))' "$1/load.jsonl"); do ps -o command= -p "$p" 2>/dev/null | grep -qF shibubu-qa-load && n=$((n+1)); done; echo $n; }
sleep 30 & SP=$!
printf '{"schema":"shibubu.qa.run/1","app":"x","pid":%s,"data_dir":"d","started_at":1,"suspects":[%s]}\n' $SP $SP > "$R/run.json"
mk 'pass'; mkdir -p "$R/steps"; cp "$T/b.json" "$R/steps/s.before.json"; cp "$T/a.json" "$R/steps/s.after.json"
"$FRONT_GUARD" judge --step s --before "$T/b.json" --after "$T/a.json" --events "$T/e.jsonl" --suspect 200 > "$R/steps/s.verdict.json"
echo '{"schema":"shibubu.qa.step/1","step":"s","tool":"click","rc":0,"t0":10,"t1":12,"verdict":"steps/s.verdict.json"}' > "$R/steps.jsonl"
# stop は run 全体（起動の前 → stop）を 1 回判定する。snap は作った after に差し替え（画面を読まない）。
printf '{"schema":"shibubu.qa.run/1","app":"x","pid":%s,"data_dir":"d","started_at":1,"suspects":[%s],"owner_pid":100}\n' $SP $SP > "$R/run.json"
cp "$T/b.json" "$R/steps/00-launch.before.json"; cp "$T/e.jsonl" "$R/events.jsonl"
echo '{"schema":"shibubu.qa.event/1","t":9.0,"kind":"start"}' > "$R/launch.events.jsonl"
snap() { local o; while [ $# -gt 0 ]; do [ "$1" = --out ] && o=$2; shift; done; cp "$T/a.json" "$o"; }
echo '{"schema":"shibubu.qa.event/1","t":11,"kind":"activated","app":{"pid":200}}' >> "$R/launch.events.jsonl"
(cmd_stop "$R") 2>/dev/null;                                  check "stop: activation between steps -> 1" 1 $?
cp "$T/e.jsonl" "$R/events.jsonl"; echo '{"schema":"shibubu.qa.event/1","t":9.0,"kind":"start"}' > "$R/launch.events.jsonl"
axok() { echo '{"schema":"shibubu.qa.axwindows/1","t":10,"pid":2,"variant":null,"count":1,"err":0,"secs":0.1,"cg_onscreen":1,"load1":null,"state":"attached"}' > "$R/ax-windows.jsonl"; }
rm -f "$R/ax-windows.jsonl"
(cmd_stop "$R") 2>/dev/null;                                  check "stop without ax samples -> 6" 6 $?
axok; echo '{"schema":"shibubu.qa.axwindows/1","t":11,"pid":2,"variant":null,"count":0,"err":0,"secs":0.1,"cg_onscreen":1,"load1":null,"state":"attached"}' >> "$R/ax-windows.jsonl"
(cmd_stop "$R") 2>/dev/null;                                  check "stop with ax detached -> 1" "1 False 1" "$? $(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["ok"],d["ax_judge_rc"])' "$R/run.json")"
axok; (cmd_load "$R" 60 1) 2>/dev/null; before=$(alive "$R")
(cmd_stop "$R") 2>/dev/null;                                  check "stop clean -> 0" 0 $?
# 負の対照の的: 起こした run の stop（gate の外の道）の kill_loads を消すとここが赤（前の 1 = loop が本当に回っていた）
check "stop clean kills its own 60 s load loop (alive before / after)" "1 0" "$before $(alive "$R")"; rm -f "$R/load.jsonl"
"$R/trap/cua-driver" status 2>/dev/null
(cmd_stop "$R") 2>/dev/null;                                  check "stop with trap mark -> 7" 7 $?
check "run.json ok=false after trap" "False 1" "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["ok"],d["trap_marks"])' "$R/run.json")"
kill -0 $SP 2>/dev/null;                                      check "stop killed own pid" 1 $?

# ---- tally.py ----
mkdir -p "$R/ax"
echo '{"role":"AXWindow","children":[{"role":"AXStaticText","value":"接続済み"},{"identifier":"terminal:1","children":[{"value":"Done"}]}]}' > "$R/ax/s.json"
python3 -I "$BG/tally.py" "$R" > "$T/tally.json";             check "tally exit" 0 $?
check "tally L1 lie (terminal subtree excluded)" "1 接続済み 1" "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(len(d["lies"]),d["lies"][0]["word"],d["claimed_connected"])' "$T/tally.json")"
python3 -I - "$R" <<'EOF'
import json,sys
R=sys.argv[1]
open(R+"/steps.jsonl","a").write(json.dumps({"schema":"shibubu.qa.step/1","step":"p","tool":"click","rc":0,"t0":10,"t1":12,"expect":"ui_state:qaneedle1","verdict":"steps/s.verdict.json"})+"\n")
json.dump({"effect":"unverifiable"},open(R+"/steps/p.cua.json","w"))
EOF
check "tally L2 dead click" "p" "$(python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print(",".join(x["step"] for x in json.load(sys.stdin)["dead_clicks"]))')"
echo '{"effect":"confirmed"}' > "$R/steps/p.cua.json"
dead() { python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print(",".join(x["step"] for x in json.load(sys.stdin)["dead_clicks"]))'; }
check "tally cua confirmed alone is not an effect" "p" "$(dead)"
mkdir -p "$R/oracle"
us() { echo "{\"schema\":\"shibubu.qa.uistate/1\",\"t\":$2,\"db\":\"x\",\"needle\":\"qaneedle1\",\"rows\":[],\"has_needle\":$3}" > "$R/oracle/p.$1.uistate.json"; }
us before 1 false; us after 2 true;                          check "L2 ui_state false->true is an effect" "" "$(dead)"
# W2b-AX: tally が axwindows の oracle を読む
echo '{"schema":"shibubu.qa.axwindows/1","t":1,"pid":2,"variant":"head","count":1,"err":0,"secs":0.2,"cg_onscreen":1,"load1":3.0,"state":"attached"}' > "$R/oracle/x.after.axwindows.json"
python3 -I "$BG/tally.py" "$R" >/dev/null 2>&1;               check "tally reads axwindows oracle" 0 $?
rm "$R/oracle/x.after.axwindows.json"
# W2b-AX: ax-judge（作った標本）。attached だけ → 0・detached が 1 件 → 1・unresponsive だけ → 6・0 件 → 6・壊れた行 → 4
# 書いてある state は囮（常に attached）。ax-judge は count・err・cg_onscreen から classifyAxWindows で決め直す（書いた値を数えるだけなら下の 4 本が赤）
axl() { echo "{\"schema\":\"shibubu.qa.axwindows/1\",\"t\":$1,\"pid\":2,\"variant\":\"head\",\"count\":$2,\"err\":$3,\"secs\":0.1,\"cg_onscreen\":${4:-1},\"load1\":3.0,\"state\":\"attached\"}"; }
axj() { "$FRONT_GUARD" ax-judge --samples "$T/ax.jsonl" > "$T/axv.json" 2>/dev/null; echo $?; }
axby() { python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]))["by_state"];print(",".join(f"{k}={d[k]}" for k in sorted(d)))' "$T/axv.json"; }
{ axl 1 1 0; axl 2 1 0; } > "$T/ax.jsonl";                                     check "ax-judge attached -> 0" 0 "$(axj)"
{ axl 1 1 0; axl 2 0 0; axl 3 1 0; } > "$T/ax.jsonl";                           check "ax-judge count 0 err 0 cg 1 -> detached 1" "1 2" "$(axj) $(python3 -I -c 'import json,sys;print(int(json.load(open(sys.argv[1]))["first_detached"]))' "$T/axv.json")"
{ axl 1 null -25204; } > "$T/ax.jsonl";                                        check "ax-judge err -25204 -> unresponsive 6" "6 unresponsive=1" "$(axj) $(axby)"
{ axl 1 1 0; axl 2 null -25211; } > "$T/ax.jsonl";                             check "ax-judge err -25211 -> error (ok)" "0 attached=1,error=1" "$(axj) $(axby)"
{ axl 1 0 -25204 0; axl 2 1 0; } > "$T/ax.jsonl";                              check "ax-judge cg 0 before the window -> no_window (ok)" "0 attached=1,no_window=1" "$(axj) $(axby)"
{ axl 1 1 0; axl 2 0 -25204 0; } > "$T/ax.jsonl";                              check "ax-judge window gone mid-run -> 6 (unmeasured)" "6 False" "$(axj) $(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["ok"])' "$T/axv.json")"
cp /dev/null "$T/ax.jsonl";                                                   check "ax-judge no samples -> 6" 6 "$(axj)"
{ axl 1 1 0 attached; echo '{"schema":"shibubu.qa.axwindows/1"'; } > "$T/ax.jsonl"; check "ax-judge broken line -> 4" 4 "$(axj)"
# ax-windows: 自分の shell（窓 0）→ no_window・答えは err で返り exit 0（許可が無ければ 3）
"$FRONT_GUARD" ax-windows --pid $$ --timeout-s 2 > "$T/axw.json" 2>/dev/null; check "ax-windows no window -> no_window" "0 no_window" "$? $(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["state"])' "$T/axw.json" 2>/dev/null)"
us after 2 false;                                            check "L2 ui_state false->false is dead" "p" "$(dead)"
us before 1 true; us after 2 true;                           check "L2 ui_state already there is dead" "p" "$(dead)"
us before 1 false; rm "$R/oracle/p.after.uistate.json";      check "L2 ui_state after missing is dead" "p" "$(dead)"
rm -f "$R/oracle/p."*
# ax: 手 p の前後の木（cua の get_window_state の形・平らな elements と parent_index）
axt() { python3 -I -c 'import json,sys;v,spin=sys.argv[2],sys.argv[3]
print(json.dumps({"elements":[{"element_index":0,"role":"AXWindow","label":"S"},{"element_index":1,"parent_index":0,"role":"AXGroup","label":"一覧"},
 {"element_index":2,"parent_index":1,"role":"AXStaticText","value":v},{"element_index":3,"parent_index":1,"role":"AXProgressIndicator","value":spin},
 {"element_index":4,"parent_index":0,"role":"AXStaticText","value":"外"}]}))' x "$1" "$2" > "$R/ax/$3.json"; }
python3 -I - "$R" <<'PY'
import json,sys
R=sys.argv[1];l=[json.loads(x) for x in open(R+"/steps.jsonl") if x.strip()]
l[-1]["expect"]="ax:AXGroup:一覧"
ws={**l[0],"tool":"get_window_state","expect":None};l.insert(len(l)-1,{**ws,"step":"o"});l.append({**ws,"step":"z"})
open(R+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in l))
PY
rm -f "$R/ax/s.json"
axt a 1 o; axt b 1 z;                                        check "L2 ax subtree text changed is an effect" "" "$(dead)"
axt a 1 o; axt a 1 z;                                        check "L2 ax unchanged is dead" "p" "$(dead)"
axt a 1 o; axt a 2 z;                                        check "L2 ax spinner-only change is dead" "p" "$(dead)"
echo '{"elements":[]}' > "$R/ax/z.json";                     check "L2 ax target gone from an empty after-tree is dead" "p" "$(dead)"
echo '{"elements":[]}' > "$R/ax/o.json"; axt a 1 z;          check "L2 ax target appearing is an effect" "" "$(dead)"
axt a 1 o; axt a 2 z
python3 -I -c 'import json,sys;p=sys.argv[1];d=json.load(open(p));d["elements"][4]["value"]="外2";json.dump(d,open(p,"w"))' "$R/ax/z.json"
check "L2 ax change outside the target subtree is dead" "p" "$(dead)"
# 木 o と手 p の間に別の手 q（打鍵）が居る: o→z の変化は q の効果かもしれない → p の効果は測れない（dead・unmeasured に名前）
python3 -I - "$R" <<'PY'
import json,sys
R=sys.argv[1];l=[json.loads(x) for x in open(R+"/steps.jsonl") if x.strip()]
i=[x["step"] for x in l].index("p");l.insert(i,{**l[i],"step":"q","expect":None})
open(R+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in l))
PY
axt a 1 o; axt b 1 z;                                        check "L2 ax with another step between tree and step is dead" "p" "$(dead)"
check "L2 ax with another step between names it in unmeasured" "True" "$(python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print("effect.p" in json.load(sys.stdin)["unmeasured"])')"
rm -f "$R/ax/o.json" "$R/ax/z.json"
# demo-loop r3: 100 手を超える run で木を手の番号順に並べる（文字の順だと 100- が 10- の直後 → 11 手目の後の木が 100 手目・最後の木が 12 手目）
t100() {  # t100 <tally.py> -> "<dead_clicks> <lies の数>"
  local D="$T/t100.$RANDOM"; mkdir -p "$D/steps" "$D/ax"; cp "$R/run.json" "$D/"; cp "$R/steps/s.verdict.json" "$D/steps/"
  python3 -I - "$D" <<'PY'
import json,sys
D=sys.argv[1]
st=lambda n,tool,t,**k: dict(schema="shibubu.qa.step/1",step=n,tool=tool,rc=0,t0=t,t1=t+1,verdict="steps/s.verdict.json",**k)
open(D+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in [st("10-get_window_state","get_window_state",1),st("11-click","click",3,expect="ax:*"),st("12-get_window_state","get_window_state",5),st("100-get_window_state","get_window_state",7)]))
tree=lambda v:{"elements":[{"element_index":0,"role":"AXWindow","label":"w"},{"element_index":1,"parent_index":0,"role":"AXStaticText","value":v}]}
for n,v in (("10","before · 接続済み"),("12","after"),("100","before · 接続済み")): json.dump(tree(v),open(f"{D}/ax/{n}-get_window_state.json","w"),ensure_ascii=False)
PY
  python3 -I "$1" "$D" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(",".join(x["step"] for x in d["dead_clicks"]) or "-", len(d["lies"]))'
}
sed 's/, key=step_order):$/):/' "$BG/tally.py" > "$BG/tally-ord.$$.py"; cmp -s "$BG/tally.py" "$BG/tally-ord.$$.py" && echo "tally-ord: mutation did not apply" >&2
check "tally orders AX trees by step number past 99 (after-tree of 11 = 12, last tree = 100); negative control: plain string sort -> 11 dead and the last tree is 12" "- 1|11-click 0" "$(t100 "$BG/tally.py")|$(t100 "$BG/tally-ord.$$.py")"; rm -f "$BG/tally-ord.$$.py"
python3 -I - "$R" <<'PY'
import json,sys
R=sys.argv[1];l=[json.loads(x) for x in open(R+"/steps.jsonl") if x.strip()]
l=[x for x in l if x["step"] not in ("o","z","q")];l[-1]["expect"]="refused:Unsupported";l[-1]["rc"]=1
open(R+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in l))
PY
echo 'cua-driver: set_value: AXValue Unsupported' > "$R/steps/p.cua.json"; check "L2 refused with the line is an effect" "" "$(dead)"
echo 'cua-driver: timeout' > "$R/steps/p.cua.json";            check "L2 refused without the line is dead" "p" "$(dead)"
python3 -I - "$R" <<'PY'
import json,sys
R=sys.argv[1];l=[json.loads(x) for x in open(R+"/steps.jsonl") if x.strip()]
l[-1]["expect"]="保存で一覧に 1 行"
open(R+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in l))
PY
python3 -I "$BG/tally.py" "$R" >/dev/null 2>&1;               check "tally free-text expect -> 4" 4 $?
python3 -I - "$R" <<'PY'
import json,sys
R=sys.argv[1];l=[json.loads(x) for x in open(R+"/steps.jsonl") if x.strip()]
l[-1]["expect"]=None;l[-1]["rc"]=0
open(R+"/steps.jsonl","w").write("".join(json.dumps(x)+"\n" for x in l))
PY
echo '{"effect":"confirmed","window":{"frontmost":true}}' > "$R/steps/p.cua.json"
check "tally cua frontmost key -> listed" "p" "$(python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print(",".join(json.load(sys.stdin)["cua_frontmost"]))')"
echo '{"effect":"confirmed"}' > "$R/steps/p.cua.json"
check "tally no frontmost key -> empty" "" "$(python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print(",".join(json.load(sys.stdin)["cua_frontmost"]))')"
echo '{"role":"AXWindow","children":[{"role":"AXStaticText","value":"接続済み"}]}' > "$R/ax/p.json"
echo '{"schema":"shibubu.qa.github/1","repo":"o/r","has_nonce":true}' > "$R/oracle/g.json"
check "tally claimed (kind unknown) stays unverified" "1 1" "$(python3 -I "$BG/tally.py" "$R" 2>&1 | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["claimed_connected"],d["claimed_unverified"])')"
rm "$R/oracle/g.json" "$R/ax/p.json"
# W12W-3: B4 は 1 件の has_nonce では数えない（before に無かった事を見ていない）。対の検査は下の W12W-3 の節
check "verified_kinds: lone B4 record with the nonce (no before) -> Web false" "False" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t;print(t.verified_kinds({"g":{"schema":"shibubu.qa.github/1","has_nonce":True}})["Web"])' "$BG")"
check "find_lies: Saved, not checked is not a claim / Saved alone is" "0 1" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
f=lambda s: len(t.find_lies({"role":"AXStaticText","label":s},{},"s"))
print(f("Saved, not checked · docs/arch.excalidraw"),f("Saved · docs/arch.excalidraw"))' "$BG")"
check "verified_kinds: no nonce -> Web false" "False" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t;print(t.verified_kinds({"g":{"schema":"shibubu.qa.github/1","has_nonce":False}})["Web"])' "$BG")"
# W16-2 の A1（$defs.a1）: 4 つ全部 true の時だけ NativeApp。1 つでも false（sha が同じ・nonce が無い・経路が枠の外・重ねた記録と違う窓）なら false
check "verified_kinds: A1 all four -> NativeApp only" "True False False" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t;v=t.verified_kinds({"a":{"schema":"shibubu.qa.a1/1","sha_changed":True,"has_nonce":True,"route_ok":True,"overlay_match":True}});print(v["NativeApp"],v["Web"],v["Terminal"])' "$BG")"
check "verified_kinds: A1 any one false -> NativeApp false" "False False False False" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
ks=("sha_changed","has_nonce","route_ok","overlay_match")
print(*[t.verified_kinds({"a":{"schema":"shibubu.qa.a1/1",**{k:(k!=off) for k in ks}}})["NativeApp"] for off in ks])' "$BG")"
check "parse_expect refused without target ok, ui_state without target -> error" "refused ValueError" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t;print(t.parse_expect("refused:").kind,end=" ")
try: t.parse_expect("ui_state:")
except ValueError: print("ValueError")' "$BG")"

# W19-1（operator 2026-10-23）: 凡例（AX の identifier legend:）の部分木は L1 の外。3 つの木の形（入れ子・平らな elements・cua-driver の tree_markdown）とも、
# 凡例の「Connected = 認証済み」は lies にも claimed にも数えず、凡例の外の本物の未検証の「接続済み」1 つは数える（正の対照）。
# 負の対照: tally.py の EXCLUDED_PREFIXES から "legend:" を外すと 3 形とも lies 2（W19-1 の実機 run の偽陽性と同じ数え方に戻る）
legend() { python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
md="- [0] AXWindow \"Shibubu\"\n  - AXGroup [id=legend:connections]\n    - AXStaticText = \"Connected = 認証済み\"\n    - AXStaticText = \"Ready = 権限と疎通 OK\"\n  - AXStaticText = \"信頼済 CLI\"\n  - AXStaticText = \"接続済み\"\n  - AXStaticText = \"取得した機能 0 件\"\n"
trees={"nested":{"role":"AXWindow","children":[{"role":"AXGroup","identifier":"legend:connections","children":[{"role":"AXStaticText","value":"Connected = 認証済み"}]},{"role":"AXStaticText","value":"接続済み"}]},
 "flat":{"elements":[{"element_index":0,"role":"AXWindow"},{"element_index":1,"parent_index":0,"role":"AXGroup","identifier":"legend:connections"},{"element_index":2,"parent_index":1,"role":"AXGroup"},{"element_index":3,"parent_index":2,"role":"AXStaticText","value":"Connected = 認証済み"},{"element_index":4,"parent_index":0,"role":"AXStaticText","value":"接続済み"}]},
 "markdown":{"elements":[],"tree_markdown":md}}
step="s"
print(" ".join(f"{k}:{len(t.find_lies(v,{},step))}/{len(t.count_claimed(v))}" for k,v in trees.items()))' "$BG"; }
check "tally L1: legend subtree excluded in 3 tree shapes, one real unverified 接続済み still counted" "nested:1/1 flat:1/1 markdown:1/1" "$(legend)"

# ---- oracle/ui-state.py（作った sqlite）----
U="$T/u"; mkdir -p "$U"
uread() { python3 -I "$BG/oracle/ui-state.py" read "$U" "$@"; }
uread >/dev/null 2>&1;                                       check "ui-state no DB -> 4" 4 $?
sqlite3 "$U/kangae.sqlite3" 'CREATE TABLE other(x)'
uread >/dev/null 2>&1;                                       check "ui-state no table -> 4" 4 $?
sqlite3 "$U/kangae.sqlite3" "PRAGMA journal_mode=WAL; CREATE TABLE ui_state(session_id TEXT, key TEXT, json TEXT, updated_at TEXT); INSERT INTO ui_state VALUES('main.a','books','{\"items\":[{\"title\":\"qaneedle1 吾輩\"}]}','2026-10-20');" >/dev/null
check "ui-state needle found" "True 1 64" "$(uread --needle qaneedle1 | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["has_needle"],len(d["rows"]),len(d["rows"][0]["json_sha256"]))')"
check "ui-state needle absent" "False" "$(uread --needle qaneedle2 | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["has_needle"])')"
check "ui-state no needle -> false, no json body in output" "False 0" "$(uread | python3 -I -c 'import sys;s=sys.stdin.read();import json;print(json.loads(s)["has_needle"],s.count("吾輩"))')"
s0=$(shasum "$U/kangae.sqlite3"); uread --needle x >/dev/null; check "ui-state does not write the DB" "$s0" "$(shasum "$U/kangae.sqlite3")"
python3 -I "$BG/oracle/ui-state.py" read >/dev/null 2>&1;    check "ui-state usage -> 2" 2 $?

# ---- run.sh の expect_oracle（作った DB）----
mkdir -p "$R/data"; cp "$U/kangae.sqlite3" "$R/data/"
expect_oracle "$R" q before ui_state:qaneedle1 2>/dev/null
check "expect_oracle ui_state writes the pair file" "True" "$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["has_needle"])' "$R/oracle/q.before.uistate.json")"
rm -f "$R/data/kangae.sqlite3"; expect_oracle "$R" q after ui_state:qaneedle1 2>/dev/null; rc=$?
check "expect_oracle without DB -> 4 and no file" "4 no" "$rc $([ -e "$R/oracle/q.after.uistate.json" ] && echo yes || echo no)"
expect_oracle "$R" q after oracle:nope 2>/dev/null;           check "expect_oracle unknown oracle -> non-zero" 4 $?
rm -f "$R/oracle/q."*
# step の配線: 偽の cua-driver が click で DB に needle を書く → before / after の対が取れて tally が効果ありと数える
cp "$U/kangae.sqlite3" "$R/data/"; sqlite3 "$R/data/kangae.sqlite3" "DELETE FROM ui_state"
printf '#!/bin/bash\nsqlite3 "%s" "INSERT INTO ui_state VALUES(%s)"\necho "{\\"effect\\":\\"confirmed\\"}"\n' "$R/data/kangae.sqlite3" "'main.a','books','qaneedle9','x'" > "$T/fake-cua"; chmod +x "$T/fake-cua"
(CUA_DRIVER_REAL="$T/fake-cua"; cmd_step "$R" w click '{"pid":1,"element_token":"s1:2","session":"q"}' --expect ui_state:qaneedle9) >/dev/null 2>&1
check "step takes the ui_state pair around the click" "False True" "$(python3 -I -c 'import json,sys;print(*(json.load(open(f"{sys.argv[1]}/oracle/w.{x}.uistate.json"))["has_needle"] for x in ("before","after")))' "$R" 2>&1)"
check "step with a real ui_state effect is not dead" "" "$(dead)"
# W12W-1c: step は cua の手の直後に axprobe を呼び、path を steps.jsonl に・tally の causes がその手を切り分ける
lastax() { python3 -I -c 'import json,os,sys;l=[json.loads(x) for x in open(sys.argv[1]+"/steps.jsonl") if x.strip()];s=[x for x in l if x["step"]==sys.argv[2]][-1];a=s.get("axprobe");print(a,bool(a) and os.path.exists(sys.argv[1]+"/"+a))' "$R" "$1"; }
causeof() { python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;c=[x for x in json.load(sys.stdin)["causes"] if x["step"]==sys.argv[1]][-1];print(c["product"]["kind"]+"/"+c["product"]["reason"],c["driver"]["kind"],c["operable"])' "$1"; }
check "step writes the axprobe path into steps.jsonl" "steps/w.axprobe.json True" "$(lastax w)"
check "tally causes: AX all empty with a CG window -> environment, driver separate" "environment/ax_empty resolved False" "$(causeof w)"
check "tally causes: a step written without a probe -> no_probe" "unmeasured/no_probe" "$(causeof p | cut -d' ' -f1)"
# W15-b1: step は手の前の 1 分 load を steps.jsonl の load1 に書き、tally は load1 > 16 の手だけを congested に並べる（16 ちょうどは混雑でない・機械の本物の load で打った他の手は見ない）
steploads() { python3 -I -c 'import json,sys;print(*[x.get("load1") for x in map(json.loads,open(sys.argv[1]+"/steps.jsonl")) if x["step"] in ("l1","l2","l3")])' "$R"; }
congested() { python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print(",".join(x for x in json.load(sys.stdin)["congested"] if x in ("l1","l2","l3")))'; }
real_load1=$(declare -f load1_now)
for l in "l1 33.5" "l2 3.0" "l3 16.0"; do
  load1_now() { echo "${l#* }"; }
  (CUA_DRIVER_REAL="$T/fake-cua"; cmd_step "$R" "${l%% *}" click '{"pid":1,"element_token":"s1:2","session":"q"}') >/dev/null 2>&1
done
eval "$real_load1"
check "step writes load1 (the 1-min load before the step) into steps.jsonl" "33.5 3.0 16.0" "$(steploads)"
check "tally congested: only load1 > 16 (16.0 exactly, 3.0 and steps without load1 are not)" "l1" "$(congested)"
: > "$T/axprobe.args"
(export FAKE_AXPROBE_RC=70 CUA_DRIVER_REAL="$T/fake-cua"; cmd_step "$R" v click '{"pid":1,"element_token":"s1:2","session":"q"}' --want 'web-retry:*') >/dev/null 2>&1
check "step passes --pid of run.json, --step and --want to axprobe" "1" "$(grep -c -- "--pid $SP --step v --want web-retry:\*" "$T/axprobe.args")"
check "step with a failed axprobe keeps no file -> null" "None False" "$(lastax v)"
check "tally causes: failed probe -> no_probe (not green)" "unmeasured/no_probe" "$(causeof v | cut -d' ' -f1)"
echo '{"schema":"shibubu.qa.axprobe/9"}' > "$R/steps/w.axprobe.json"
python3 -I "$BG/tally.py" "$R" >/dev/null 2>&1;               check "tally unknown axprobe schema -> 4" 4 $?
cp "$T/probe.json" "$R/steps/w.axprobe.json"
(cmd_step "$R" q click '{"pid":1,"element_token":"s1:2","session":"q"}' --expect "保存で一覧") >/dev/null 2>&1; check "step with unreadable --expect -> 2" 2 $?
# get_window_state の手は commit してよい形の list.txt（tally.py shape）を steps/ に書く: menu bar の実名は入らない
printf '#!/bin/bash\necho %s\n' "'{\"elements\":[{\"element_index\":0,\"depth\":0,\"role\":\"AXWindow\",\"label\":\"Shibubu\"},{\"element_index\":1,\"depth\":0,\"role\":\"AXMenuBar\"},{\"element_index\":2,\"depth\":1,\"parent_index\":1,\"role\":\"AXMenuItem\",\"label\":\"Log Out 山田太郎\"}]}'" > "$T/fake-tree"; chmod +x "$T/fake-tree"
cp -R "$R" "$T/gwsrun"; (CUA_DRIVER_REAL="$T/fake-tree"; cmd_step "$T/gwsrun" gws get_window_state '{"pid":1,"window_id":1,"timeout_ms":10000,"session":"q"}') >/dev/null 2>&1
check "step get_window_state writes the shape list without the menu bar" "elements 1|0 0 AXWindow 'Shibubu'" "$(paste -sd'|' "$T/gwsrun/steps/gws.list.txt" 2>&1)"
# W12a: ax_tree の手は front-guard ax の木（AXIdentifier の鍵を持つ形）を ax/ に置き、shape の list も書く（pid 1 = 窓の無い process → 0 要素）
(cmd_step "$T/gwsrun" axt ax_tree '{"pid":1,"session":"q"}') >/dev/null 2>&1
check "step ax_tree writes the front-guard tree to ax/ and the shape list" "shibubu.qa.axtree/1 0|elements 0" "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["schema"],len(d["elements"]),end="|")' "$T/gwsrun/ax/axt.json" 2>&1; head -1 "$T/gwsrun/steps/axt.list.txt" 2>&1)"
rm -rf "$T/gwsrun"

# ---- tally.py cause_of（W12W-1c・docs/diagrams.md §30 の順）: 変える物を python の式で受ける。既定 = 3 属性とも答えた・on screen の窓 1 つ・want 無し・load 120（記録だけ）
cause() {
  python3 -I - "$BG" "$1" <<'EOF'
import sys
sys.path.insert(0, sys.argv[1]); import tally as t
def att(o="answered", code="success", ms=20.0, n=1): return {"outcome": o, "ax_code": code, "elapsed_ms": ms, "count": n}
def win(on=True, w=1440, h=928): return {"window_id": 5226, "pid": 42, "layer": 0, "bounds": {"x": 120, "y": 32, "w": w, "h": h}, "onscreen": on, "z": 3 if on else -1}
def want(found=False, complete=True): return {"identifier": "web-retry:*", "found": found, "visited": 40, "complete": complete, "elapsed_ms": 30.0}
p = {"schema": "shibubu.qa.axprobe/1", "t": 1, "step": "s", "pid": 42, "trusted": True, "ax": {"windows": att(), "main_window": att(), "focused_window": att()},
     "cg": [win()], "space": "normal", "pressure": {"load1": 120.5, "ncpu": 8, "memory_pressure": "normal"}}
cua = {"elements": [{"element_index": 0}] * 3}
empty = lambda: {k: att("empty", "success" if k == "windows" else "no_value", 12.0, 0) for k in ("windows", "main_window", "focused_window")}
exec(sys.argv[2])
c = t.cause_of("s", p, cua)
print(c.product.kind + "/" + c.product.reason, c.operable, c.driver.kind + "/" + str(c.driver.reason) + "/" + str(c.driver.element_count))
EOF
}
pc() { cause "$1" | cut -d' ' -f1-2; }
dc() { cause "$1" | cut -d' ' -f3; }
check "cause: AX answered, no want -> ok/answered, not operable" "ok/answered False" "$(pc 'pass')"
check "cause: want reached -> ok/reached, operable (only here)" "ok/reached True" "$(pc 'p["want"]=want(found=True)')"
check "cause: CG window on screen alone is not operable" "False" "$(pc 'p["ax"]=empty()' | cut -d' ' -f2)"
check "cause: not trusted -> environment" "environment/not_trusted False" "$(pc 'p["trusted"]=False')"
check "cause: one attribute not_trusted -> environment" "environment/not_trusted False" "$(pc 'p["ax"]["focused_window"]=att("not_trusted","api_disabled",1,0)')"
check "cause: no CG window -> unmeasured" "unmeasured/no_cg_window False" "$(pc 'p["cg"]=[]')"
check "cause: only a 24pt strip -> unmeasured" "unmeasured/no_cg_window False" "$(pc 'p["cg"]=[win(w=1680,h=24)]')"
check "cause: CG window off screen -> environment/off_space" "environment/off_space False" "$(pc 'p["cg"]=[win(on=False)];p["ax"]=empty()')"
check "cause: an attribute timed out -> environment/ax_timeout" "environment/ax_timeout False" "$(pc 'p["ax"]["main_window"]=att("timeout","cannot_complete",10000.0,0)')"
check "cause: slow AX (>= ax_slow_ms) -> environment/congested" "environment/congested False" "$(pc 'p["ax"]["windows"]=att(ms=2500.0)')"
check "cause: memory pressure warn -> environment/congested" "environment/congested False" "$(pc 'p["pressure"]["memory_pressure"]="warn"')"
check "cause: load above ncpu alone is not congestion (recorded only)" "ok/answered 120.5 8" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
p={"trusted":True,"ax":{k:{"outcome":"answered","ax_code":"success","elapsed_ms":5.0,"count":1} for k in ("windows","main_window","focused_window")},"cg":[{"layer":0,"bounds":{"w":800,"h":600},"onscreen":True}],"pressure":{"load1":120.5,"ncpu":8,"memory_pressure":"normal"}}
c=t.cause_of("s",p,None);print(c.product.kind+"/"+c.product.reason,c.product.numbers["load1"],c.product.numbers["ncpu"])' "$BG")"
# 負の対照の的（D-d）: 判定を「AX 0 ＝ 製品の欠陥」に戻すとここが赤
check "cause: AX 0 with a CG window on screen -> environment/ax_empty (not a defect)" "environment/ax_empty False" "$(pc 'p["ax"]=empty()')"
check "cause: answered and the whole tree has no want -> defect (the only defect)" "defect/elements_missing False" "$(pc 'p["want"]=want()')"
check "cause: answered but the walk stopped early -> unmeasured" "unmeasured/walk_incomplete False" "$(pc 'p["want"]=want(complete=False)')"
check "cause: AX errors (not timeout) with want -> unmeasured, not a defect" "unmeasured/other False" "$(pc 'p["ax"]={k:att("error","failure",3.0,0) for k in ("windows","main_window","focused_window")};p["want"]=want()')"
check "cause: no probe -> unmeasured/no_probe" "unmeasured/no_probe False" "$(pc 'p=None')"
check "cause: want null and missing swap/hid are read" "ok/answered False" "$(pc 'p["want"]=None;p["pressure"].pop("ncpu")')"
check "driver: no cua response -> unmeasured" "unmeasured" "$(dc 'cua=None' | cut -d/ -f1)"
check "driver: a text AX failure of the hand -> unresolved(ax_action_failed:-25204)" "unresolved/ax_action_failed:-25204/None" "$(dc 'cua="AX action failed: AXUIElementPerformAction(AXPress) returned -25204\n"')"
check "driver: other non-JSON text -> unmeasured" "unmeasured" "$(dc 'cua="usage: cua-driver call <tool>"' | cut -d/ -f1)"
check "driver: run-4 shaped degraded get_window_state -> unresolved(ax_window_unresolved, 0)" "unresolved/ax_window_unresolved/0" \
  "$(dc 'cua={"elements":[],"degraded":True,"degraded_reason":"ax_window_unresolved: window_id 664 exists and is owned by pid 31277, but none of the 0 AXWindow element(s)","background_input":{"exact_window":{"pid":31277,"status":"ax_unresolved","window_id":664}}}')"
check "driver: refusal -> unresolved(code)" "unresolved/element_outside_target_window/None" "$(dc 'cua={"refusal":{"code":"element_outside_target_window","message":"x"}}')"
check "driver: elements -> resolved(count)" "resolved/None/3" "$(dc 'pass')"
check "driver: walk cut short -> resolved, reason elements_incomplete" "resolved/elements_incomplete/1" "$(dc 'cua={"elements":[{"element_index":0}],"elements_complete":False,"nodes_pending":28}')"
check "driver: a refused element_token hand -> unresolved(code)" "unresolved/off_space_or_ax_unresolved/None" "$(dc 'cua={"code":"off_space_or_ax_unresolved","effect":"refused","escalation":{"reason":"window 769 is not among the process current AXWindows"}}')"
check "driver: an element_token hand that went through -> resolved" "resolved/None/None" "$(dc 'cua={"effect":"unverifiable","route":"accessibility","summary":"✅ Performed AXPress on [14] AXButton"}')"
check "independent: driver unresolved does not make the product a defect" "ok/reached True unresolved/ax_window_unresolved/0" \
  "$(cause 'p["want"]=want(found=True);cua={"elements":[],"degraded":True,"degraded_reason":"ax_window_unresolved: x"}')"
check "independent: driver resolved does not make the product ok" "environment/ax_empty False resolved/None/3" "$(cause 'p["ax"]=empty()')"

# 本物の bin/axprobe（画面に触らない: 窓の無い自分の sleep の pid を読む・使い方の 2）
"$BG/bin/axprobe" --step x >/dev/null 2>&1;                   check "axprobe usage -> 2" 2 $?
sleep 30 & SP3=$!
"$BG/bin/axprobe" --pid $SP3 --step real --want 'web-retry:*' --timeout-ms 1000 > "$T/real.axprobe.json" 2>/dev/null; rc=$?
kill $SP3 2>/dev/null
check "axprobe writes a record (0, or 3 without the permission)" "yes" "$( { [ $rc -eq 0 ] || [ $rc -eq 3 ]; } && echo yes || echo "rc=$rc")"
check "axprobe record has the schema, 3 attributes, want, space and pressure" "shibubu.qa.axprobe/1 focused_window,main_window,windows web-retry:* True" \
  "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["schema"],",".join(sorted(d["ax"])),d["want"]["identifier"],d["space"] in ("normal","fullscreen","split_view","unknown") and d["pressure"]["ncpu"]>0)' "$T/real.axprobe.json" 2>&1)"
check "axprobe of a windowless pid -> cause unmeasured/no_cg_window (not a defect)" "unmeasured/no_cg_window" \
  "$(python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally as t;c=t.cause_of("real",json.load(open(sys.argv[2])),None);print(c.product.kind+"/"+c.product.reason)' "$BG" "$T/real.axprobe.json" 2>&1)"

# ---- fresh-repo.sh（local の bare repo と gh の偽物・network を使わない）----
F="$T/fr"; mkdir -p "$F/src/docs" "$F/gh"; echo "# demo" > "$F/src/README.md"; echo n > "$F/src/docs/notes.md"
git init -q --bare "$F/remote.git"
FR() { env YC_RUNS="$F/runs" YC_DEMO_REPO=o/demo GH_TOKEN="${FAKE_TOK-tok-SECRET-123}" GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0="url.$F/remote.git.insteadOf" GIT_CONFIG_VALUE_0=https://github.com/o/demo.git PATH="$F/gh:$PATH" bash "$BG/fresh-repo.sh" "$@"; }
printf '#!/bin/bash
# gh repo clone <repo> <dir> -- <gitflags>
[ "$1 $2" = "repo clone" ] || exit 9
[ -n "$GH_TOKEN" ] || exit 9
git clone "${@:6}" "%s/remote.git" "$4"
' "$F" > "$F/gh/gh"; chmod +x "$F/gh/gh"
FR seed --nope "$F/src" >/dev/null 2>&1;                     check "seed without approval -> 9" 9 $?
FAKE_TOK= FR seed --approved "$F/src" >/dev/null 2>&1;        check "seed without GH_TOKEN -> 5" 5 $?
check "seed nothing pushed before approval" "" "$(git -C "$F/remote.git" tag)"
FR clone "$F/r1" >/dev/null 2>&1;                            check "clone without mirror -> 3" 3 $?
FR seed --approved "$F/src" > "$F/seed.out" 2>&1;            check "seed pushes main and tag" "0 yc-initial" "$? $(git -C "$F/remote.git" tag)"
FR seed --approved "$F/src" >/dev/null 2>&1;                 check "seed again does not overwrite the tag -> 4" 4 $?
FAKE_TOK= FR mirror >/dev/null 2>&1;                          check "mirror without GH_TOKEN -> 5" 5 $?
FR mirror > "$F/mirror.out" 2>&1;                            check "mirror" 0 $?
FR clone "$F/r1" > "$F/clone.out" 2>&1;                       check "clone from mirror" 0 $?
check "clone repo.json: head = tag, clean, path" "True True $F/r1/shibubu-yc-demo" "$(python3 -I -c 'import json,sys,subprocess;d=json.load(open(sys.argv[1]));t=subprocess.check_output(["git","-C",sys.argv[2],"rev-parse","refs/tags/yc-initial^{commit}"],text=True).strip();print(d["head_sha"]==t,d["clean"],d["path"])' "$F/r1/repo.json" "$F/runs/.mirror")"
check "clone origin is the local mirror (no network)" "$F/runs/.mirror" "$(git -C "$F/r1/shibubu-yc-demo" remote get-url origin)"
FR clone "$F/r1" >/dev/null 2>&1;                            check "clone into an existing run -> 4" 4 $?
git -C "$F/runs/.mirror" tag -d yc-initial >/dev/null; FR clone "$F/r2" >/dev/null 2>&1; check "clone without tag -> 4" 4 $?
check "token never written" 0 "$(grep -rl tok-SECRET-123 "$F/runs" "$F/r1" "$F/seed.out" "$F/mirror.out" "$F/clone.out" "$F/remote.git" 2>/dev/null | wc -l | tr -d ' ')"
check "tally surfaces are unmeasured, not 0" "True" "$(python3 -I "$BG/tally.py" "$R" | python3 -I -c 'import json,sys;print("native_verified" in json.load(sys.stdin)["unmeasured"])')"
sed -i '' 's#shibubu.qa.run/1#shibubu.qa.run/9#' "$R/run.json"
python3 -I "$BG/tally.py" "$R" >/dev/null 2>&1;               check "tally unknown schema -> 4" 4 $?

# ---- tally.py shape（R の AX の木の比べ・commit する list.txt の形）: 窓の中だけ・menu bar（実名の Log Out・最近使った項目）と AXValue と nonce を落とす
python3 -I -c 'import json,sys
e=[{"element_index":0,"depth":0,"role":"AXWindow","label":"Shibubu"},
   {"element_index":1,"depth":1,"parent_index":0,"role":"AXButton","label":"Dev","value":"0"},
   {"element_index":2,"depth":1,"parent_index":0,"role":"AXTextArea","label":"goal n-r2-1792531221 /Users/someone/x","value":"typed secret"},
   {"element_index":3,"depth":0,"role":"AXMenuBar"},
   {"element_index":4,"depth":1,"parent_index":3,"role":"AXMenuItem","label":"Log Out 山田太郎…"},
   {"element_index":5,"depth":1,"parent_index":3,"role":"AXMenuItem","label":"notes.md"}]
json.dump({"element_count":6,"elements":e},open(sys.argv[1],"w"))' "$T/shape.json"
python3 -I "$BG/tally.py" shape "$T/shape.json" > "$T/shape.txt";  check "shape exit" 0 $?
check "shape: window only, no menu bar, no value, nonce and home redacted" \
  "elements 3|0 0 AXWindow 'Shibubu'|1 1 AXButton 'Dev'|2 1 AXTextArea 'goal n-* /Users/you/x'" "$(paste -sd'|' "$T/shape.txt")"
check "shape: no real name / recent item / value leaks" 0 "$(grep -c '山田\|notes.md\|typed secret\|someone' "$T/shape.txt")"
# tally.py redact（commit する get_window_state の cua.json の形）: driver_of の判定は同じ・label / value / tree_markdown は落とす
python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));d.update(elements_complete=False,tree_markdown="- Log Out 山田太郎… typed secret",background_input={"exact_window":{"status":"matched"}});json.dump(d,open(sys.argv[1],"w"))' "$T/shape.json"
python3 -I "$BG/tally.py" redact "$T/shape.json" > "$T/redact.json";  check "redact exit" 0 $?
check "redact keeps the driver verdict" "$(python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally;print(tally.driver_of(json.load(open(sys.argv[2]))))' "$BG" "$T/shape.json")" \
  "$(python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally;print(tally.driver_of(json.load(open(sys.argv[2]))))' "$BG" "$T/redact.json")"
check "redact: no real name / recent item / value / nonce / home leaks" 0 "$(grep -c '山田\|notes.md\|typed secret\|someone\|n-r2-' "$T/redact.json")"
# W12W-1c の実機 run-1（commit した記録）を tally し直す: 製品と cua-driver の数が log.md と同じ（記録だけから数え直せる・判定を変えると赤）
E1="$BG/../../../docs/evidence/W12W-1c/run-1"; mkdir -p "$T/run1/steps"
cp "$E1/run.json" "$E1/steps.jsonl" "$T/run1/"; cp "$E1"/*.verdict.json "$E1"/*.axprobe.json "$E1"/*.cua.json "$T/run1/steps/"
check "run-1 re-tallied from the committed evidence: product | cua-driver" "ok=4 defect=0 environment=5 unmeasured=2|resolved=6 unresolved=3 unmeasured=2" \
  "$(python3 -I "$BG/tally.py" "$T/run1" | python3 -I -c 'import json,sys
c=json.load(sys.stdin)["causes"]
n=lambda k,vs:" ".join("%s=%d"%(v,sum(1 for x in c if x[k]["kind"]==v)) for v in vs)
print(n("product",("ok","defect","environment","unmeasured"))+"|"+n("driver",("resolved","unresolved","unmeasured")))' 2>&1)"

# ---- W12W-1c の場（run.sh の owner の gate・SHIBUBU_QA_SCENE・load と stop の kill・tally.py の scene_held / in_load / self_load / owner_active）----
# HIDIdleTime は差し替える（本物の ioreg を読まない＝ owner の状態で検査の結果を変えない）。open と front-guard は偽物（呼ばれたら印・open は失敗して exit 3）
SC="$T/scene"; mkdir -p "$SC/app.app/Contents/MacOS"; echo x > "$SC/app.app/Contents/MacOS/x"
printf '#!/bin/bash\necho "$1" >> "%s/fg.calls"\nif [ "$1" = watch ]; then while [ $# -gt 0 ]; do [ "$1" = --out ] && o=$2; shift; done; echo "{\\"schema\\":\\"shibubu.qa.event/1\\",\\"t\\":9e9,\\"kind\\":\\"start\\"}" > "$o"; fi\n' "$SC" > "$SC/fg"; chmod +x "$SC/fg"
open() { echo "$*" >> "$SC/open.calls"; return 1; }
IDLE=300; owner_idle_s() { echo "$IDLE"; }
# gate <idle> <scene> → "<rc> <owner_idle_s> <owner_active> <ok> <scene> <watch を打ったか> <open を打ったか>"
gate() {
  local R="$SC/g.$1.$2"; rm -rf "$R" "$SC/fg.calls" "$SC/open.calls"; mkdir -p "$R"
  ( IDLE=$1; SHIBUBU_QA_SCENE=$2 FRONT_GUARD="$SC/fg" cmd_start "$R" "$SC/app.app" ) >/dev/null 2>&1; local rc=$?
  echo "$rc $(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["owner_idle_s"],d["owner_active"],d["ok"],d["scene"])' "$R/run.json" 2>/dev/null || echo no-run.json) $([ -s "$SC/fg.calls" ] && echo watch || echo no-watch) $([ -s "$SC/open.calls" ] && echo open || echo no-open)"
}
# 負の対照の的（D-d）: start の「120 秒未満なら起こさない」を外すとここが赤
check "gate: owner idle 5 s -> 6, owner_active, ok false, neither watch nor open" "6 5.0 True False normal_load no-watch no-open" "$(gate 5 normal_load)"
check "gate: 119.9 s is still the owner -> 6" "6 119.9 True False None no-watch no-open" "$(gate 119.9 '')"
check "gate: HIDIdleTime unreadable -> 6, owner_idle_s null, owner_active true" "6 None True False None no-watch no-open" "$(gate '' '')"
check "gate: 120 s passes on to watch and open (the fake open fails -> 3)" "3 120.0 False None normal_low watch open" "$(gate 120 normal_low)"
check "gate: unknown SHIBUBU_QA_SCENE -> 2 before run.json, watch and open" "2 no-run.json no-watch no-open" "$(gate 300 bogus)"
# W15-b39: axprobe（AXPROBE）が実行可能でなければ start は何も起こさず 2（run.json も watch も open も無い）。負の対照: その 1 行を外した run.sh は idle 300 s で 3 まで進む（手ごとの idle 無しで run が始まる）
agate() {  # agate <run.sh> → "<rc> <run.json> <watch> <open>"
  local R="$SC/ax.$$"; rm -rf "$R" "$SC/fg.calls" "$SC/open.calls"; mkdir -p "$R"
  ( source "$1"; owner_idle_s() { echo 300; }; AXPROBE="$SC/no-such-axprobe" FRONT_GUARD="$SC/fg" cmd_start "$R" "$SC/app.app" ) >/dev/null 2>&1; local rc=$?
  echo "$rc $([ -e "$R/run.json" ] && echo run.json || echo no-run.json) $([ -s "$SC/fg.calls" ] && echo watch || echo no-watch) $([ -s "$SC/open.calls" ] && echo open || echo no-open)"
}
sed '/axprobe が無い/d' "$BG/run.sh" > "$BG/run-mut7.$$.sh"; am=$(agate "$BG/run-mut7.$$.sh"); rm -f "$BG/run-mut7.$$.sh"
check "start without an executable axprobe -> 2, no run.json, no watch, no open; negative control: line removed -> 3 (starts without axprobe)" "2 no-run.json no-watch no-open|3 run.json watch open" "$(agate "$BG/run.sh")|$am"
# DEMO-5 の録画 mode（SHIBUBU_QA_RECORD=1・record.sh）: idle 5 s でも起こす（open は -g 無し・run.json に record_mode）・env 無しは従来どおり（exit 6 / open は -g）
# rgate <idle> <record|空> → "<rc> <owner_active> <record_mode> <open に -g が在るか> <open を打ったか>"
rgate() {
  local R="$SC/r.$1.${2:-off}"; rm -rf "$R" "$SC/fg.calls" "$SC/open.calls"; mkdir -p "$R"
  ( IDLE=$1; SHIBUBU_QA_RECORD=${2:-} FRONT_GUARD="$SC/fg" cmd_start "$R" "$SC/app.app" ) >/dev/null 2>&1; local rc=$?
  echo "$rc $(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["owner_active"],d.get("record_mode"))' "$R/run.json") $(grep -q -- '^-g ' "$SC/open.calls" 2>/dev/null && echo -g || echo no-g) $([ -s "$SC/open.calls" ] && echo open || echo no-open)"
}
# 負の対照の的: run.sh の `rec` の分岐（gate の例外・-g を外す）を外すと 1 本目が赤、default の分岐を壊すと 2 本目が赤
check "record: SHIBUBU_QA_RECORD=1 starts although owner idle 5 s (fake open fails -> 3), record_mode true, open without -g" "3 False True no-g open" "$(rgate 5 1)"
check "record: no env -> owner idle 5 s still 6, nothing opened; idle 120 opens with -g and no record_mode" "6 True None no-g no-open | 3 False None -g open" "$(rgate 5 '') | $(rgate 120 '')"
# B67: cua-driver は 5 分使わない session を終わらせ、終わった名の手を rc 1「session has ended; tool call '…' was rejected. Call start_session …」で拒み続ける（operator の cu.kGB1 の原文）。
# 偽の cua-driver はその原文で拒み、start_session の後だけ答える。名は run ごとに別（out の末尾が同じでも前の run の名を継がない）
cat > "$T/fake-cua67" <<'EOFC67'
#!/bin/bash
echo "$2" >> "$C67/calls"; [ "$2" = set_agent_cursor_enabled ] && echo "$3" > "$C67/cursor"
[ "$2" = start_session ] && { : > "$C67/alive"; echo '{"active":true}'; exit 0; }
[ -e "$C67/alive" ] && { echo '{"windows":[]}'; exit 0; }
echo "session has ended; tool call '$2' was rejected. Call start_session with session 'q' to start it again, or use a new session label."; exit 1
EOFC67
chmod +x "$T/fake-cua67"
s67() {  # s67 <run.sh> -> "<cua-driver の呼び> <cua.json の頭> <ended.txt> <起こし直しの cursor off>"
  local R67="$T/r67"; rm -rf "$R67"; cp -R "$R" "$R67"; export C67="$T/c67"; rm -rf "$C67"; mkdir -p "$C67"
  ( source "$1"; CUA_DRIVER_REAL="$T/fake-cua67" cmd_step "$R67" s67 list_windows '{"pid":1,"session":"q"}' ) >/dev/null 2>&1
  echo "$(tr '\n' ',' < "$C67/calls") $(head -c 7 "$R67/steps/s67.cua.json") $([ -e "$R67/steps/s67.ended.txt" ] && echo ended.txt || echo no-ended) $(cat "$C67/cursor" 2>/dev/null || echo -)"
}
l67() {  # l67 <run.sh> -> "<末尾が同じ out の 2 つの run の名が違う|同じ> <名の頭>"
  local d a b
  for d in a b; do rm -rf "$SC/l67$d"; mkdir -p "$SC/l67$d/run"; ( source "$1"; owner_idle_s() { echo 300; }; FRONT_GUARD="$SC/fg" cmd_start "$SC/l67$d/run" "$SC/app.app" ) >/dev/null 2>&1; done
  a=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["session"])' "$SC/l67a/run/run.json"); b=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["session"])' "$SC/l67b/run/run.json")
  echo "$([ "$a" != "$b" ] && echo differ || echo same) ${a:0:7}"
}
python3 -I - "$BG" "$$" <<'EOFB67M'
import sys
bg, pid = sys.argv[1:3]
s = open(f"{bg}/run.sh", encoding="utf-8").read()
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
open(f"{bg}/run-b67a.{pid}.sh", "w", encoding="utf-8").write(mut("""    if grep -q 'session has ended; tool call' "$R/steps/$name.cua.json"; then""", "    if false; then"))  # 切れた時に起こし直さない（B67 の前）
open(f"{bg}/run-b67b.{pid}.sh", "w", encoding="utf-8").write(mut(""""qa-$(basename "$R")-$(printf '%s %s' "$(cd "$R" && pwd -P)" "$(now)" | shasum | cut -c1-8)\"""", '"qa-$(basename "$R")"'))  # 名を末尾だけで作る（B67 の前）
EOFB67M
check "B67: an ended cua-driver session (rc 1 'session has ended; tool call ...') -> start_session, agent cursor off again, the same call once more (the rejected answer kept as .ended.txt); two runs whose out dirs end in the same name get different session names; negative controls: no restart -> the rejection stays in cua.json; the pre-B67 name qa-<basename> -> the same name" \
  'list_windows,start_session,set_agent_cursor_enabled,list_windows, {"windo ended.txt {"session":"q","enabled":false}|differ qa-run-|list_windows, session no-ended -|same qa-run' \
  "$(s67 "$BG/run.sh")|$(l67 "$BG/run.sh")|$(s67 "$BG/run-b67a.$$.sh")|$(l67 "$BG/run-b67b.$$.sh")"
rm -f "$BG"/run-b67?.$$.sh
RT=$(python3 -I "$BG/rehearse.py" 1 --dry-run | sed -e 's/\[driver 在り[^]]*\]/[driver]/' -e 's/\[FG: 通さない\]/[owner]/' -e 's/\[driver 無し:.*\]/[未測]/')
check "record --dry-run table: 27 rows = 22 driver + 5 owner + 0 未測 (full.md・W15-b49 で b4・ask_ai×2・e0・d2 を配線)" "27 22 5 0" "$(echo "$RT" | grep -c '^ *[0-9]*\. ') $(echo "$RT" | grep -c '\[driver\]$') $(echo "$RT" | grep -c '\[owner\]$') $(echo "$RT" | grep -c '\[未測\]$')"
# W15-b51: 記号だけの型（` · `）は何にでも当たる。csv_patterns が捨てれば `zzz qqq · yyy` は一致せず、文字のある型 `Mirror · {}` は残る
# 負の対照の的: rehearse.py の「英数字か仮名・漢字が 1 つも無い型を捨てる」の 2 行を消すと 1 つ目が True になって赤
cpy() {  # cpy <rehearse.py> -> "<zzz qqq · yyy が一致したか> <Mirror · TextEdit が一致したか>"
  python3 -I - "$1" "$T" <<'EOF2'
import importlib.util, os, sys
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
p = os.path.join(sys.argv[2], "cp.csv")
open(p, "w", encoding="utf-8").write("file,line,kind,lang,text\nx,1,a,en,{} · {}\nx,2,a,en, · \nx,3,a,en,Mirror · {}\n")
ps = m.csv_patterns(p)
print(any(q.fullmatch("zzz qqq · yyy") for q in ps), any(q.fullmatch("Mirror · TextEdit") for q in ps))
EOF2
}
CPM="$BG/.rehearse-cp-mut.py"  # HERE を使う rehearse なので BG の中に置く
python3 -I -c 'import re,sys;t=open(sys.argv[1],encoding="utf-8").read();t2=re.sub(r"        if not re\.search\(.*\n            continue\n","",t);assert t2!=t;open(sys.argv[2],"w",encoding="utf-8").write(t2)' "$BG/rehearse.py" "$CPM"
check "csv_patterns drops symbol-only patterns: 'zzz qqq · yyy' does not match, 'Mirror · TextEdit' does; the filter removed -> both match" "False True | True True" "$(cpy "$BG/rehearse.py") | $(cpy "$CPM")"
rm -f "$CPM"
# git の外の docs/internal/ui-pro/strings.csv が無い木（CI の checkout・git archive の写し）でも dry-run は緑（W15-b26）: 毎回 ui-strings.py --csv でその木から生成（UI-1 の fresh_strings_csv）
# 負の対照: 生成をやめて git の外の strings.csv を返す写しは、その file を開けず FileNotFoundError で落ちる
NI="$T/noint"; NIX="$T/noint-mut"; mkdir -p "$NI" "$NIX"
(cd "$BG/../../.." && tar -c scripts docs/demo src) | tar -x -C "$NI"; cp -R "$NI/." "$NIX/"
sed -i.bak 's|^    path = os.path.join(out_dir, "strings.csv")$|    return os.path.join(REPO, "docs/internal/ui-pro/strings.csv")|' "$NIX/scripts/qa/bg/rehearse.py"
nirun() { (cd "$1" && python3 -I scripts/qa/bg/rehearse.py 1 --dry-run 2>"$T/ni.err"); echo "rc=$?"; }
NR=$(nirun "$NI"); NM=$(nirun "$NIX")
check "no docs/internal tree: rehearse --dry-run is green with 27 rows (strings.csv generated from the tree)" "rc=0 27" "$(echo "$NR" | tail -1) $(echo "$NR" | grep -c '^ *[0-9]*\. ')"
check "no docs/internal tree, generation branch removed -> not green (FileNotFoundError)" "rc=1 1" "$(echo "$NM" | tail -1) $(grep -c FileNotFoundError "$T/ni.err")"

GR="$SC/g.5.normal_load"
printf '#!/bin/bash\necho "$*" >> "%s/cua.calls"\nexit 1\n' "$SC" > "$SC/no-cua"; chmod +x "$SC/no-cua"
(CUA_DRIVER_REAL="$SC/no-cua" cmd_step "$GR" 01-home get_window_state '{"pid":1,"session":"q"}') >/dev/null 2>&1
check "step on a run the gate did not launch -> 6, nothing written, cua-driver not called" "6 no no-cua" "$? $([ -e "$GR/steps.jsonl" ] && echo yes || echo no) $([ -e "$SC/cua.calls" ] && echo cua || echo no-cua)"
# load: 引数の範囲・owner の gate・1 行の形・自分で終わる・stop は自分の loop だけ kill
lrc() { (cmd_load "$SC/l" "$1" "$2") >/dev/null 2>&1; echo $?; }
check "load: seconds 0 / 601 / 1.5 / abc, procs 0 / 33 -> 2" "2 2 2 2 2 2" "$(lrc 0 1) $(lrc 601 1) $(lrc 1.5 1) $(lrc abc 1) $(lrc 1 0) $(lrc 1 33)"
# 負の対照の的: owner の gate は start だけ（c2d4e40 の契約）。load に HIDIdleTime の gate を戻すとここが赤（holdout H2・H3）
check "load: no owner gate in load (start has it) -> owner active 1 s / 600 s still 0, load.jsonl, the loops killed after" "0 0 yes 0" \
  "$(IDLE=5 lrc 1 1) $(IDLE=5 lrc 600 1) $([ -e "$SC/l/load.jsonl" ] && echo yes || echo no) $(kill_loads "$SC/l"; sleep 0.3; alive "$SC/l")"
L="$SC/load"; (cmd_load "$L" 1 2) 2>/dev/null;               check "load 1 s x 2 -> 0 (returns at once)" 0 $?
check "load.jsonl: schema, t1 = t0 + seconds, procs, 2 pids" "shibubu.qa.load/1 1 2 2 True" \
  "$(python3 -I -c 'import json,sys;d=json.loads(open(sys.argv[1]).read());print(d["schema"],d["seconds"],d["procs"],len(d["pids"]),d["t1"]==d["t0"]+d["seconds"])' "$L/load.jsonl" 2>&1)"
sleep 2.5
check "load: the 1 s loops end by themselves after the window" 0 "$(alive "$L")"
# 生きている事は 60 秒の loop で見る（混雑の下では 1 秒の loop は検査の前に終わる）
sleep 30 & FP=$!
rm -f "$L/load.jsonl"; (cmd_load "$L" 60 2) 2>/dev/null
check "load: both 60 s loops run and carry the mark" 2 "$(alive "$L")"
echo "{\"schema\":\"shibubu.qa.load/1\",\"t0\":1,\"t1\":61,\"seconds\":60,\"procs\":1,\"pids\":[$FP]}" >> "$L/load.jsonl"
kill_loads "$L"; sleep 0.3
check "kill_loads: the own 60 s loop is killed, a foreign pid in load.jsonl is left alone" "0 alive" "$(alive "$L") $(kill -0 $FP 2>/dev/null && echo alive || echo killed)"
kill $FP 2>/dev/null; wait $FP 2>/dev/null
# 負の対照の的: load の後に start が run.json の前で落ちた（app が無い・知らない scene）run の stop。run.json の検査の前の kill_loads を消すとここが赤
NR="$SC/norun"; (cmd_load "$NR" 60 1) 2>/dev/null; before=$(alive "$NR"); (cmd_stop "$NR") 2>/dev/null; rc=$?
check "stop before run.json (start died after load) -> 2, its load loop killed (alive before / after)" "2 1 0" "$rc $before $(alive "$NR")"
(cmd_load "$GR" 60 1) 2>/dev/null
(cmd_stop "$GR") 2>/dev/null; rc=$?
check "stop on a run the gate did not launch -> 6, its load loop killed, ok stays false" "6 0 False" "$rc $(alive "$GR") $(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["ok"])' "$GR/run.json")"
# tally.py: owner の gate の run（上の GR）は unmeasured.owner_active・causes は空・self_load は load.jsonl の行
check "tally of a gated run: unmeasured.owner_active, no causes, scene kept, self_load listed" "True 0 normal_load 1 None" \
  "$(python3 -I "$BG/tally.py" "$GR" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print("owner_active" in d["unmeasured"],len(d["causes"]),d["scene"],len(d["self_load"]),d["scene_held"])' 2>&1)"
held() {
  python3 -I - "$BG" "$1" <<'EOF'
import sys
sys.path.insert(0, sys.argv[1]); import tally as t
pr = lambda tt, sp="normal": {"t": tt, "space": sp}
L = [{"t0": 100.0, "t1": 200.0}]
scene, probes, loads = None, [], []
exec(sys.argv[2])
print(t.scene_held(scene, probes, loads))
EOF
}
check "scene_held: no scene -> None" None "$(held 'probes=[pr(1)]')"
check "scene_held: unknown scene name -> None" None "$(held 'scene="bogus";probes=[pr(1)]')"
check "scene_held: no probe (all None) -> None" None "$(held 'scene="normal_low";probes=[None,None]')"
check "scene_held: a probe with space unknown -> None" None "$(held 'scene="normal_low";probes=[pr(1),pr(2,"unknown")]')"
# 負の対照の的（D-d・interface の commit が名指した物）: scene_held が space を見ないと fullscreen 宣言の normal の run が True で赤
check "scene_held: fullscreen declared but the probes read normal -> False" False "$(held 'scene="fullscreen";probes=[pr(1),pr(2)]')"
check "scene_held: fullscreen on fullscreen probes ignores the load windows -> True" True "$(held 'scene="fullscreen";probes=[pr(150,"fullscreen")];loads=L')"
check "scene_held: split_view on split_view probes -> True" True "$(held 'scene="split_view";probes=[pr(1,"split_view")]')"
check "scene_held: split_view declared on fullscreen probes -> False" False "$(held 'scene="split_view";probes=[pr(1,"fullscreen")]')"
check "scene_held: normal_low without load (None probes skipped) -> True" True "$(held 'scene="normal_low";probes=[pr(1),None,pr(2)]')"
check "scene_held: normal_low with a probe inside a load window -> False" False "$(held 'scene="normal_low";probes=[pr(50),pr(150)];loads=L')"
check "scene_held: normal_load, all probes inside (both ends included) -> True" True "$(held 'scene="normal_load";probes=[pr(100),pr(200)];loads=L')"
check "scene_held: normal_load with one probe outside -> False" False "$(held 'scene="normal_load";probes=[pr(150),pr(201)];loads=L')"
check "scene_held: normal_load without any load -> False" False "$(held 'scene="normal_load";probes=[pr(150)]')"
check "scene_held: normal_load across two stacked windows -> True" True "$(held 'scene="normal_load";probes=[pr(150),pr(650)];loads=L+[{"t0":600.0,"t1":700.0}]')"
# count の配線（commit した run-1 の記録に場を足す）: 判定（causes の kind）は変えず、scene_held と numbers.self_load だけが変わる
S1="$T/run1s"; cp -R "$T/run1" "$S1"
check "run-1 as committed (no scene, no owner_idle_s): scene_held None, no owner_active key, no self_load" "None None False 0" \
  "$(python3 -I "$BG/tally.py" "$S1" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["scene"],d["scene_held"],"owner_active" in d["unmeasured"],len(d["self_load"]))' 2>&1)"
scn() { python3 -I -c 'import json,sys;p=sys.argv[1]+"/run.json";d=json.load(open(p));d["scene"]=sys.argv[2];open(p,"w").write(json.dumps(d))' "$S1" "$1"; }
win() { python3 -I -c 'import json,sys,glob;ts=sorted(json.load(open(f))["t"] for f in glob.glob(sys.argv[1]+"/steps/*.axprobe.json"));a,b=ts[int(sys.argv[2])]-1,ts[int(sys.argv[3])]+1
open(sys.argv[1]+"/load.jsonl","a").write(json.dumps({"schema":"shibubu.qa.load/1","t0":a,"t1":b,"seconds":max(1,int(b-a)),"procs":8,"pids":[1]})+"\n")' "$S1" "$1" "$2"; }
sct() { python3 -I "$BG/tally.py" "$S1" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);c=d["causes"]
print(d["scene_held"],"".join("T" if x["product"]["numbers"].get("self_load") else ("." if "self_load" in x["product"]["numbers"] else "-") for x in c),
 " ".join("%s=%d"%(v,sum(1 for x in c if x["product"]["kind"]==v)) for v in ("ok","defect","environment","unmeasured")))' 2>&1; }
scn normal_low
check "run-1 declared normal_low (normal Space, no own load) -> True, no hand inside a window" "True ........... ok=4 defect=0 environment=5 unmeasured=2" "$(sct)"
scn normal_load; win 0 11
check "run-1 declared normal_load with a window over every probe -> True, every hand self_load, same causes" "True TTTTTTTTTTT ok=4 defect=0 environment=5 unmeasured=2" "$(sct)"
rm "$S1/load.jsonl"; win 0 4
check "run-1 normal_load with the window over the first 5 probes only -> False" "False TTTTT...... ok=4 defect=0 environment=5 unmeasured=2" "$(sct)"
scn fullscreen
check "run-1 declared fullscreen on its normal probes -> False" "False" "$(sct | cut -d' ' -f1)"
echo '{"schema":"shibubu.qa.load/9","t0":1,"t1":2}' >> "$S1/load.jsonl"
python3 -I "$BG/tally.py" "$S1" >/dev/null 2>&1;             check "tally: an unknown load.jsonl schema -> 4" 4 $?
# W12W-1c の場の実機（commit した記録）: owner が使用中の時の本物の start（HIDIdleTime 0.404 秒）は起こさず、tally し直すと unmeasured.owner_active・causes 0
E2="$BG/../../../docs/evidence/W12W-1c/scene-low-1"; mkdir -p "$T/sl1"; cp "$E2/run.json" "$T/sl1/"
check "scene-low-1 re-tallied from the committed run.json: gate refused (owner_active, idle < 120, pid 0, no causes, scene normal_low)" "True True 0 0 normal_low None" \
  "$(python3 -I "$BG/tally.py" "$T/sl1" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);r=json.load(open(sys.argv[1]));print("owner_active" in d["unmeasured"],r["owner_idle_s"]<120,r["pid"],len(d["causes"]),d["scene"],d["scene_held"])' "$E2/run.json" 2>&1)"
unset -f open owner_idle_s

# ---- drive-w12w1c.py（偽の run.sh の横に写して手の並びだけを見る・画面に触らない・cua-driver も Shibubu も起こさない）----
# 偽の run.sh: 呼ばれた引数を DRV_CALLS に 1 行・start は run.json・step は DRV_CUA を cua.json に・load は load.jsonl に 1 行（t1 = 今 + 秒）。
# source された時は本物の run.sh の関数（owner_active）を読み、hid_idle_s だけ DRV_IDLE に差し替える
DV="$T/drv"; mkdir -p "$DV/app.app"; cp "$BG/drive-w12w1c.py" "$DV/"; ln -s "$BG/tally.py" "$DV/tally.py"; ln -s "$BG/labels.py" "$DV/labels.py"; ln -s "$BG/owner_idle.py" "$DV/owner_idle.py"
cat > "$DV/run.sh" <<'EOF'
#!/bin/bash
source "$DRV_REAL/run.sh"; owner_idle_s() { echo "${DRV_IDLE:-300}"; }
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0
echo "$*" >> "$DRV_CALLS"
case $1 in
  start) [ -d "$3" ] || exit 3; printf '{"schema":"shibubu.qa.run/1","pid":42,"window_id":7,"session":"q"}\n' > "$2/run.json" ;;
  step) mkdir -p "$2/steps"; cp "$DRV_CUA" "$2/steps/$3.cua.json" ;;
  load) python3 -I -c 'import json,sys,time;t=time.time();open(sys.argv[1]+"/load.jsonl","a").write(json.dumps({"schema":"shibubu.qa.load/1","t0":t,"t1":t+int(sys.argv[2]),"seconds":int(sys.argv[2]),"procs":int(sys.argv[3]),"pids":[]})+"\n")' "$2" "$3" "$4" ;;
esac
exit 0
EOF
chmod +x "$DV/run.sh"
echo '{"elements":[{"role":"AXTextField","label":"What to create","element_token":"s:1"},{"role":"AXButton","label":"Create","element_token":"s:2"}]}' > "$DV/found.json"
echo '{"degraded":true,"degraded_reason":"ax_window_unresolved: none of the 0 AXWindow element(s)","elements":[]}' > "$DV/none.json"
# drv <名前> <scene> <cua> [idle] [app] → "<exit> | <手の名前=want ...> | <step 以外の呼び出し ...> | <tally.json の hand.* の鍵>"
drv() {
  local R="$DV/$1"; rm -rf "$R"
  DRV_REAL="$BG" DRV_CALLS="$R.calls" DRV_CUA="$DV/$3" DRV_IDLE="${4:-300}" python3 -I "$DV/drive-w12w1c.py" "$2" "$R" "${5:-$DV/app.app}" >/dev/null 2>&1; local rc=$?
  python3 -I - "$rc" "$R.calls" "$R/tally.json" <<'EOF'
import json,os,sys
rc,calls,tj=sys.argv[1:4]
L=[l.split() for l in open(calls)] if os.path.exists(calls) else []
st=" ".join(l[2]+"="+(l[l.index("--want")+1] if "--want" in l else "-") for l in L if l[0]=="step")
other=" ".join(l[0] for l in L if l[0]!="step")
try: h=",".join(sorted(k for k in json.load(open(tj))["unmeasured"] if k.startswith("hand.")))
except (OSError,ValueError): h="no-tally"
print(rc,"|",st,"|",other,"|",h)
EOF
}
# 負の対照の的（反証 1）: step に --want を渡さないと「=-」で赤・token の無い手を unmeasured の hand.* に書かないと 2 本目が赤
check "drive: the same 6 hands each with --want (read = the next hand's target role, act = its own target)" \
  "0 | 01-home=role=AXTextField 02-click-input=role=AXTextField 03-type-goal=role=AXTextField 04-read-typed=role=AXButton 05-create=role=AXButton 06-read-page=role=AXButton | start stop | " \
  "$(drv found normal_low found.json)"
check "drive: no token in the read -> the act hands are not sent but listed as tally.json unmeasured hand.*" \
  "0 | 01-home=role=AXTextField 04-read-typed=role=AXButton 06-read-page=role=AXButton | start stop | hand.02-click-input,hand.03-type-goal,hand.05-create" \
  "$(drv none normal_low none.json)"
check "drive normal_load: load before start, then the same hands" \
  "0 | 01-home=role=AXTextField 02-click-input=role=AXTextField 03-type-goal=role=AXTextField 04-read-typed=role=AXButton 05-create=role=AXButton 06-read-page=role=AXButton | load start stop | " \
  "$(drv load normal_load found.json)"
# 負の対照の的（反証 2・4）: app の検査を keep_loaded の後に回すと load が打たれて赤
check "drive normal_load with no app -> 2, nothing called (no load left behind)" "2 |  |  | no-tally" "$(drv noapp normal_load found.json 300 "$DV/nope.app")"
# 負の対照の的（holdout H2・H3 で run.sh load の gate を外した代わり）: owner が使用中なら drive は負荷を作らない（start の gate へ・run の途中なら stop して 6）
check "drive normal_load with the owner active -> no load, start (its gate decides), stop, 6" "6 |  | start stop | " "$(drv owner normal_load found.json 5)"

# ---- docs/evidence/W19-3b/drive-w19-3b.py --dry-run（.app も run.sh も TextEdit も起こさない・手の表 = 流れの hand(鍵) の順）----
# 名前は D3B / dw3b（後ろの drive-w15 の検査が DW="${DW:-…}" と dw を使う・同じ名前だと drive-w15 の 2 本が drive-w19-3b を読んで赤）。偽の run.sh は呼ばれたら印を書く・--out は作られない事も見る。負の対照の的: 表の 1 行（⑥ の承認）を消した写しは表と流れが合わず 2
D3B="$BG/../../../docs/evidence/W19-3b/drive-w19-3b.py"; mkdir -p "$T/dw/bg"
printf '#!/bin/bash\ntouch "%s/dw/called"\n' "$T" > "$T/dw/bg/run.sh"
dw3b() {
  QA_BG="$T/dw/bg" python3 -I "$1" "$T/dw/nope.app" --out "$T/dw/out" --dry-run > "$T/dw/o.txt" 2>&1; local rc=$?
  echo "$rc $([ "$(grep -c '^hand ' "$T/dw/o.txt")" -ge 8 ] && echo hands8 || echo few) $(grep -c '^judge ' "$T/dw/o.txt") $(grep '^hand ' "$T/dw/o.txt" | awk '{print $3}' | uniq | tr -d '\n') $( { [ -e "$T/dw/called" ] || [ -e "$T/dw/out" ]; } && echo touched || echo clean)"
}
check "drive-w19-3b --dry-run: exit 0, hands >= 8, 5 judge items, ①〜⑥ in order, nothing started or written (no .app)" "0 hands8 5 ①②③④⑤⑥ clean" "$(dw3b "$D3B")"
grep -v '("approve2", "⑥"' "$D3B" > "$T/dw/cut.py"
check "drive-w19-3b --dry-run with one hand line deleted -> 2 (the table and the flow disagree)" 2 "$(dw3b "$T/dw/cut.py" | cut -d' ' -f1)"

# ---- oracle/github.sh（gh の偽物）----
mkdir -p "$T/gh"
printf '#!/bin/bash\n[ -n "${GH_FAIL:-}" ] && { echo "HTTP 404" >&2; exit 1; }\necho %s\necho %s\n' \
  "'[{\"id\":1,\"created_at\":\"2026-10-20T00:00:00Z\",\"body\":\"hello\"}]'" "'[{\"id\":2,\"created_at\":\"2026-10-20T00:01:00Z\",\"body\":\"run N123 ok\"}]'" > "$T/gh/gh"
chmod +x "$T/gh/gh"
check "github nonce across pages" "True 2 False" "$(PATH="$T/gh:$PATH" bash "$BG/oracle/github.sh" comments o/r 1 N123 | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["has_nonce"],len(d["comments"]),"body" in d["comments"][0])')"
check "github no nonce" "False" "$(PATH="$T/gh:$PATH" bash "$BG/oracle/github.sh" comments o/r 1 ZZZ | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["has_nonce"])')"
PATH="$T/gh:$PATH" GH_FAIL=1 bash "$BG/oracle/github.sh" comments o/r 1 N123 >/dev/null 2>&1; check "github API failure -> 4" 4 $?
bash "$BG/oracle/github.sh" comments o/r 1 "" >/dev/null 2>&1; check "github empty nonce -> 2" 2 $?

# ---- oracle/a1.py（W16-2 の A1・作った標本と overlay）----
# 編集した窓 = TextEdit（pid 222）の窓 500・Shibubu（pid 9）の窓 600 がその直後ろ・枠 = 窓 500 の bounds。mut で 1 つずつ壊す。
mkdir -p "$T/a1"
printf '# demo\n' > "$T/a1/README.md"
A1_BEFORE=$(python3 -I "$BG/oracle/a1.py" sha "$T/a1/README.md")
printf '# demo\nn-a1-1\n' > "$T/a1/README.md"
a1mk() {
  python3 -I - "$T/a1" "$1" <<'PY'
import json,sys
D,mut=sys.argv[1],sys.argv[2]
def win(i,pid,z,x=100,y=100,w=800,h=600,layer=0,on=True): return {"window_id":i,"pid":pid,"layer":layer,"bounds":{"x":x,"y":y,"w":w,"h":h},"onscreen":on,"z":z}
samples=[{"schema":"shibubu.qa.windows/1","t":10.0+i*0.2,"pid":None,"windows":[win(1,1,0,0,0,1680,24,layer=25),win(500,222,1),win(600,9,2,0,0,1440,900),win(700,333,3)]} for i in range(5)]
overlay=[{"surface_id":"docs","page_id":"page.docs","event":"attached","at_ms":9000.0,"bundle_id":"com.apple.TextEdit","pid":222,"window_id":500,"frame":{"x":100,"y":100,"w":800,"h":600}}]
exec(mut)
open(D+"/s.jsonl","w").write("".join(json.dumps(s)+"\n" for s in samples))
open(D+"/o.jsonl","w").write("".join(json.dumps(o)+"\n" for o in overlay))
PY
}
a1show() { python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["sha_changed"],d["has_nonce"],d["route_ok"],d["overlay_match"],d["samples"],",".join(sorted({m["why"] for m in d["misses"]})) or "-")' "$T/a1/a1.json"; }
a1j() { python3 -I "$BG/oracle/a1.py" judge --file "$T/a1/README.md" --rel README.md --samples "$T/a1/s.jsonl" --t0 10 --t1 11 --shibubu-pid 9 --overlay "$T/a1/o.jsonl" --surface docs --out "$T/a1/a1.json" "$@"; }
a1() { a1j --nonce n-a1-1 --sha-before "$A1_BEFORE" --edited-pid 222 --edited-window 500 --frame 100,100,800,600 && a1show; }
a1mk 'pass';                                                                            check "a1 clean" "True True True True 5 -" "$(a1)"
a1mk 'samples[2]["windows"][1]["bounds"]["x"]=102';                                     check "a1 bounds > 1pt" "True True False True 5 bounds" "$(a1)"
a1mk 'samples[2]["windows"][1]["bounds"]["x"]=100.8';                                   check "a1 bounds <= 1pt ok" "True True True True 5 -" "$(a1)"
a1mk 'samples[1]["windows"]=[w for w in samples[1]["windows"] if w["window_id"]!=500]'; check "a1 window gone" "True True False True 5 no_window" "$(a1)"
a1mk 'samples[3]["windows"][1]["onscreen"]=False;samples[3]["windows"][1]["z"]=-1';     check "a1 offscreen" "True True False True 5 offscreen" "$(a1)"
a1mk 'samples[0]["windows"].insert(2,win(800,333,2));samples[0]["windows"][3]["z"]=3';  check "a1 window between -> not above" "True True False True 5 not_above_shibubu" "$(a1)"
a1mk 'samples[0]["windows"].insert(2,win(800,333,2,x=1000,w=100))';                     check "a1 non-overlapping between is ok" "True True True True 5 -" "$(a1)"
a1mk 'samples[4]["windows"][2]["z"]=0;samples[4]["windows"][1]["z"]=1';                 check "a1 shibubu in front -> not above" "True True False True 5 not_above_shibubu" "$(a1)"
a1mk 'samples=[]';                                                                      check "a1 no samples (Shibubu stopped)" "True True False True 0 -" "$(a1)"
a1mk 'samples=[dict(s,t=s["t"]+5) for s in samples]';                                   check "a1 samples outside the edit" "True True False True 0 -" "$(a1)"
a1mk 'overlay.append(dict(overlay[0],event="host_hidden",at_ms=10500.0))';              check "a1 overlay hidden during edit" "True True True False 5 -" "$(a1)"
a1mk 'overlay[0]["window_id"]=501';                                                     check "a1 overlay other window (L1)" "True True True False 5 -" "$(a1)"
a1mk 'overlay=[]';                                                                      check "a1 no overlay record" "True True True False 5 -" "$(a1)"
a1mk 'pass'; check "a1 no edited window" "True True False False 5 no_window" "$(a1j --nonce n-a1-1 --sha-before "$A1_BEFORE" --frame 100,100,800,600 && a1show)"
a1mk 'pass'; check "a1 same sha / other nonce" "False False True True 5 -" "$(a1j --nonce n-other --sha-before "$(python3 -I "$BG/oracle/a1.py" sha "$T/a1/README.md")" --edited-pid 222 --edited-window 500 --frame 100,100,800,600 && a1show)"
printf '{"elements":[{"role":"AXGroup","label":"surface-frame:docs","frame":{"x":100,"y":100,"w":800,"h":600}}]}\n' > "$T/a1/ax.json"
a1mk 'pass'; check "a1 frame from the AX tree" "True True True True 5 -" "$(a1j --nonce n-a1-1 --sha-before "$A1_BEFORE" --edited-pid 222 --edited-window 500 --ax "$T/a1/ax.json" && a1show)"
a1mk 'pass'; a1 >/dev/null
check "a1 record -> tally A1 NativeApp" "True" "$(python3 -I -c 'import sys,json;sys.path.insert(0,sys.argv[1]);import tally as t;print(t.verified_kinds({"a":json.load(open(sys.argv[2]))})["NativeApp"])' "$BG" "$T/a1/a1.json")"
python3 -I "$BG/oracle/a1.py" judge --file x >/dev/null 2>&1; check "a1 usage -> 2" 2 $?
# ---- W12a: fresh-repo.sh init・oracle/git.sh・run.sh の oracle:git・tally.py git_seen（temp の repo・owner の設定を読まない・network を使わない）----
G="$T/w12a"; mkdir -p "$G"
qg() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$1" -c user.name=qa -c user.email=qa@localhost "${@:2}"; }
bash "$BG/fresh-repo.sh" init "$G/r1" >/dev/null 2> "$G/init.err"; check "init" 0 $?
D="$G/r1/shibubu-yc-demo"
check "init repo.json: origin init, head = tag, clean, no mirror" "init True True local " "$(python3 -I -c 'import json,sys,subprocess;d=json.load(open(sys.argv[1]));t=subprocess.check_output(["git","-C",sys.argv[2],"rev-parse","refs/tags/yc-initial^{commit}"],text=True).strip();print(d["origin"],d["head_sha"]==t,d["clean"],d["repo"],d["mirror"])' "$G/r1/repo.json" "$D")"
check "init says so in one stderr line (copied to the device log)" 1 "$(grep -c 'mirror は使わず git init と README の commit で run の clone を作った' "$G/init.err")"
bash "$BG/fresh-repo.sh" init "$G/r1" >/dev/null 2>&1;          check "init into an existing run -> 4" 4 $?
gread() { bash "$BG/oracle/git.sh" read "$@"; }
check "git.sh clean: 40-hex sha = rev-parse HEAD, branch main, 0 changes" "True main 0" "$(gread "$D" | python3 -I -c 'import json,sys,subprocess;d=json.load(sys.stdin);print(d["schema"]=="shibubu.qa.git/1" and len(d["head_sha"])==40 and d["head_sha"]==subprocess.check_output(["git","-C",sys.argv[1],"rev-parse","HEAD"],text=True).strip(),d["branch"],len(d["changes"]))' "$D")"
echo nonce-w12a >> "$D/README.md"; : > "$D/a b 日本.txt"
check "git.sh after the README edit: modified + raw path with space and kana" " M README.md|?? a b 日本.txt" "$(gread "$D" | python3 -I -c 'import json,sys;print("|".join(c["status"]+" "+c["path"] for c in json.load(sys.stdin)["changes"]))')"
qg "$D" add -A; qg "$D" commit -qm two; qg "$D" mv README.md DOC.md
check "git.sh rename: new path and orig_path" "R  DOC.md README.md" "$(gread "$D" | python3 -I -c 'import json,sys;c=json.load(sys.stdin)["changes"][0];print(c["status"],c["path"],c["orig_path"])')"
qg "$D" checkout -q --detach
check "git.sh detached: branch null" "None" "$(gread "$D" | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["branch"])')"
mkdir -p "$G/unborn"; qg "$G/unborn" init -q -b main
check "git.sh unborn: sha null, branch main" "None main" "$(gread "$G/unborn" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["head_sha"],d["branch"])')"
mkdir -p "$G/plain"; gread "$G/plain" >/dev/null 2>&1;           check "git.sh not a repo -> 4" 4 $?
gread "$G/nope" >/dev/null 2>&1;                                 check "git.sh no dir -> 4" 4 $?
gread >/dev/null 2>&1;                                           check "git.sh usage -> 2" 2 $?
bash "$BG/fresh-repo.sh" init "$G/run" >/dev/null 2>&1
expect_oracle "$G/run" g before oracle:git 2>/dev/null
check "expect_oracle oracle:git reads R/shibubu-yc-demo" "shibubu.qa.git/1 main" "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["schema"],d["branch"])' "$G/run/oracle/g.before.git.json" 2>&1)"
mkdir -p "$G/empty"; expect_oracle "$G/empty" g before oracle:git 2>/dev/null; rc=$?
check "expect_oracle oracle:git without a clone -> 4 and no file" "4 no" "$rc $([ -e "$G/empty/oracle/g.before.git.json" ] && echo yes || echo no)"
# git_seen（AX の木は作った elements の列・oracle は git.sh の形）
gs() { python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
S="0123456789abcdef0123456789abcdef01234567"
def tree(sha=S,branch="main",rows=("README.md",),ids=True):
    els=[{"element_index":0,"parent_index":None,"role":"AXWindow","title":"Shibubu"}]
    def add(ident,kids):
        i=len(els); els.append({"element_index":i,"parent_index":0,"role":"AXGroup","identifier":ident if ids else None})
        for k in kids: els.append({"element_index":len(els),"parent_index":i,"role":k[0],"label":k[1]})
    add("data:dev-git-sha",[("AXStaticText",sha)] if sha else [])
    add("data:dev-git-branch",[("AXStaticText",branch)] if branch else [])
    add("data:dev-git-changes",[("AXButton",r) for r in rows])
    return {"elements":els}
o={"schema":"shibubu.qa.git/1","t":0,"path":"/x","head_sha":S,"branch":"main","changes":[{"status":" M","path":"README.md"}]}
kw={}
exec(sys.argv[2])
v=t.git_seen(tree(**kw),o);print(v["sha"],v["branch"],v["changes"])' "$BG" "$1"; }
check "git_seen all three match" "True True True" "$(gs '')"
check "git_seen a fixed SHA on the screen is red (negative control)" "False True True" "$(gs 'kw={"sha":"f"*40}')"
check "git_seen a truncated SHA is not a match" "False True True" "$(gs 'kw={"sha":S[:7]+"…"}')"
check "git_seen a missing changed file is red" "True True False" "$(gs 'kw={"rows":()}')"
check "git_seen an extra row is red" "True True False" "$(gs 'kw={"rows":("README.md","x")}')"
check "git_seen null sha with no 40-hex on the screen" "True True True" "$(gs 'o["head_sha"]=None;kw={"sha":None}')"
check "git_seen null branch while a branch is drawn is red" "True False True" "$(gs 'o["branch"]=None')"
check "git_seen identifiers absent -> None (unmeasured, not False)" "None None None" "$(gs 'kw={"ids":False}')"
check "git_seen wrong oracle schema -> ValueError" "ValueError" "$(gs 'o["schema"]="shibubu.qa.github/1"' 2>&1 | grep -o ValueError | head -1)"
echo '{"elements":[]}' > "$G/ax.json"; gread "$G/run/shibubu-yc-demo" > "$G/o.json"
check "tally.py git CLI: no identifiers -> nulls" '{"branch": null, "changes": null, "sha": null}' "$(python3 -I "$BG/tally.py" git "$G/ax.json" "$G/o.json")"
python3 -I "$BG/tally.py" git "$G/ax.json" "$G/ax.json" >/dev/null 2>&1; check "tally.py git CLI: not a git oracle -> 4" 4 $?

# ---- W12W-2: oracle/excalidraw.sh（作った .excalidraw）・tally.py の b1_saved / b3_kept / load_run・run.sh の oracle:excalidraw ----
X="$T/xd"; mkdir -p "$X/run/shibubu-yc-demo/docs" "$X/run/oracle"
# mkx <file> <elements の python の式>: Excalidraw の書き出しの形（type・elements・appState・files）。appState の名前に N1 を入れて、数えない事を見る
mkx() { python3 -I -c 'import json,sys;els=eval(sys.argv[2]);json.dump({"type":"excalidraw","version":2,"source":"https://excalidraw.com","elements":els,"appState":{"name":"N1-in-appstate"},"files":{}},open(sys.argv[1],"w"))' "$1" "$2"; }
xread() { bash "$BG/oracle/excalidraw.sh" read "$@"; }
xs() { python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(d["schema"],d["exists"],d["elements"],",".join(f"{k}={v}" for k,v in sorted(d["nonces"].items())),len(d["sha256"] or ""))'; }
mkx "$X/a.excalidraw" '[{"type":"rectangle","id":"r"},{"type":"text","id":"t","text":"N1 box","originalText":"N1 box"}]'
check "excalidraw.sh: file with the nonce text" "shibubu.qa.excalidraw/1 True 2 N1=True,N9=False 64" "$(xread "$X/a.excalidraw" N1 N9 | xs)"
mkx "$X/del.excalidraw" '[{"type":"rectangle","id":"r"},{"type":"text","id":"t","text":"N1","isDeleted":True}]'
check "excalidraw.sh: a deleted element's nonce is not counted (negative control)" "True 1 N1=False" "$(xread "$X/del.excalidraw" N1 | xs | cut -d' ' -f2-4)"
check "excalidraw.sh: the nonce only in appState is not counted" "N1=False" "$(mkx "$X/s.excalidraw" '[{"type":"rectangle"}]'; xread "$X/s.excalidraw" N1 | xs | cut -d' ' -f4)"
check "excalidraw.sh: originalText counts (wrapped text)" "N1=True" "$(mkx "$X/o.excalidraw" '[{"type":"text","text":"N\n1","originalText":"N1"}]'; xread "$X/o.excalidraw" N1 | xs | cut -d' ' -f4)"
check "excalidraw.sh: missing file -> exists false, rc 0" "0 shibubu.qa.excalidraw/1 False None N1=False 0" "$(xread "$X/none.excalidraw" N1 > "$X/n.json"; echo -n "$? "; xs < "$X/n.json")"
xread "$X/a.excalidraw" >/dev/null 2>&1;                          check "excalidraw.sh: no nonce -> 2" 2 $?
xread "" N1 >/dev/null 2>&1;                                       check "excalidraw.sh: empty path -> 2" 2 $?
xread "$X/a.excalidraw" "" >/dev/null 2>&1;                        check "excalidraw.sh: empty nonce -> 2" 2 $?
printf '{"type":"excalidraw","elements":[' > "$X/broken.excalidraw"; xread "$X/broken.excalidraw" N1 >/dev/null 2>&1; check "excalidraw.sh: broken JSON -> 4" 4 $?
echo '{"type":"excalidrawlib","elements":[]}' > "$X/lib.excalidraw"; xread "$X/lib.excalidraw" N1 >/dev/null 2>&1; check "excalidraw.sh: not type excalidraw -> 4" 4 $?
echo '{"type":"excalidraw","elements":{}}' > "$X/obj.excalidraw"; xread "$X/obj.excalidraw" N1 >/dev/null 2>&1; check "excalidraw.sh: elements not a list -> 4" 4 $?
# b1_saved / b3_kept（excalidraw.sh の実物の出力で）: b1 = 再起動の前・b3 = 後に N2 を足した・b3same = b3 と同じ file・b3lost = N1 の要素が消えた
xread "$X/a.excalidraw" N1 > "$X/b1.json"
mkx "$X/a.excalidraw" '[{"type":"rectangle","id":"r"},{"type":"text","id":"t","text":"N1 box"},{"type":"text","id":"u","text":"N2"}]'
xread "$X/a.excalidraw" N1 N2 > "$X/b3.json"; xread "$X/a.excalidraw" N1 N2 N3 > "$X/b3more.json"
mkx "$X/a.excalidraw" '[{"type":"text","id":"u","text":"N2"},{"type":"text","id":"v","text":"N3"}]'
xread "$X/a.excalidraw" N1 N2 N3 > "$X/b3lost.json"; xread "$X/a.excalidraw" N2 N3 > "$X/b3lost2.json"
tl() { python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally as t;L=lambda n:json.load(open(sys.argv[2]+"/"+n+".json"));print(eval(sys.argv[3]))' "$BG" "$X" "$1"; }
check "b1_saved: present, element, all nonces" "True" "$(tl 't.b1_saved(L("b1"))')"
check "b1_saved: one nonce false -> False" "False" "$(tl 't.b1_saved(L("b3lost"))')"
check "b1_saved: missing file -> False" "False" "$(tl 't.b1_saved(L("n"))')"
check "b1_saved: 0 elements -> False" "False" "$(tl 't.b1_saved({**L("b1"),"elements":0})')"
check "b1_saved: no nonces -> False" "False" "$(tl 't.b1_saved({**L("b1"),"nonces":{}})')"
check "b1_saved: other schema -> False" "False" "$(tl 't.b1_saved({**L("b1"),"schema":"shibubu.qa.github/1"})')"
check "b3_kept: sha changed, both nonces -> True" "True" "$(tl 't.b3_kept(L("b1"),L("b3"))')"
check "b3_kept: same sha -> False (positive control)" "False" "$(tl 't.b3_kept({**L("b1"),"sha256":L("b3")["sha256"]},L("b3"))')"
check "b3_kept: a nonce of the second read missing -> False" "False" "$(tl 't.b3_kept(L("b1"),L("b3more"))')"
check "b3_kept: the first nonce lost after restart -> False" "False" "$(tl 't.b3_kept(L("b1"),L("b3lost"))')"
check "b3_kept: second read without the first nonce -> False" "False" "$(tl 't.b3_kept(L("b1"),L("b3lost2"))')"
check "b3_kept: no new nonce -> False" "False" "$(tl 't.b3_kept(L("b1"),{**L("b1"),"sha256":"0"*64})')"
check "b3_kept: missing file -> None (unmeasured)" "None" "$(tl 't.b3_kept(L("b1"),L("n"))')"
check "b3_kept: other path -> None" "None" "$(tl 't.b3_kept(L("b1"),{**L("b3"),"path":"/x"})')"
check "b3_kept: other schema -> None" "None" "$(tl 't.b3_kept(L("b1"),{**L("b3"),"schema":"shibubu.qa.git/1"})')"
check "verified_kinds: B1 record -> Web only" "True False" "$(tl '" ".join(str(t.verified_kinds({"x":L("b1")})[k]) for k in ("Web","NativeApp"))')"
check "verified_kinds: B1 without the nonce -> Web false" "False" "$(tl 't.verified_kinds({"x":L("b3lost")})["Web"]')"
# run.sh の oracle:excalidraw（手の --needle・B3 は SHIBUBU_QA_NONCES）と tally の load_run
mkx "$X/run/shibubu-yc-demo/docs/arch.excalidraw" '[{"type":"text","text":"N7"}]'
expect_oracle "$X/run" x before oracle:excalidraw N7 2>/dev/null
check "expect_oracle oracle:excalidraw uses the step needle" "N7=True" "$(xs < "$X/run/oracle/x.before.excalidraw.json" | cut -d' ' -f4)"
SHIBUBU_QA_NONCES="N7 N8" expect_oracle "$X/run" x after oracle:excalidraw N7 2>/dev/null
check "expect_oracle oracle:excalidraw takes SHIBUBU_QA_NONCES for B3" "N7=True,N8=False" "$(xs < "$X/run/oracle/x.after.excalidraw.json" | cut -d' ' -f4)"
expect_oracle "$X/run" y before oracle:excalidraw 2>/dev/null; rc=$?
check "expect_oracle oracle:excalidraw without a nonce -> 2 and no file" "2 no" "$rc $([ -e "$X/run/oracle/y.before.excalidraw.json" ] && echo yes || echo no)"
echo '{"schema":"shibubu.qa.run/1"}' > "$X/run/run.json"
check "tally reads an excalidraw oracle record (load_run)" "0" "$(python3 -I "$BG/tally.py" "$X/run" >/dev/null 2>&1; echo $?)"

# ---- W12W-3: B4 の対（tally.py の b4_left / b4_kept / verified_kinds / github_login と subcommand・run.sh の oracle:github・gh の偽物・network を使わない）----
# g(comments, nonce, repo, issue) = oracle/github.sh の 1 件。B = 手の前（id 1 だけ）・A = 手の後（id 2 が nonce の新しい comment）
b4() { python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally as t
def g(cs,nonce="N9",repo="o/r",issue=3):
    return {"schema":"shibubu.qa.github/1","t":1.0,"repo":repo,"issue":issue,"nonce":nonce,"comments":[{"id":i,"created_at":"x","body_sha256":s,"has_nonce":h} for i,s,h in cs],"has_nonce":any(h for _,_,h in cs)}
B=g([(1,"a",False)]);A=g([(1,"a",False),(2,"b",True)])
print(eval(sys.argv[2]))' "$BG" "$@"; }
check "b4_left: a new comment with the nonce -> True" True "$(b4 't.b4_left(B,A)')"
check "b4_left: the nonce was already there before (even with another new one) -> False" False "$(b4 't.b4_left(g([(2,"b",True)]),g([(2,"b",True),(3,"c",True)]))')"
check "b4_left: no new comment with the nonce -> False" False "$(b4 't.b4_left(B,g([(1,"a",False),(2,"b",False)]))')"
check "b4_left: the nonce only on an old id (edited, not written) -> False" False "$(b4 't.b4_left(B,g([(1,"a2",True)]))')"
check "b4_left: before missing -> None" None "$(b4 't.b4_left(None,A)')"
check "b4_left: other issue / repo / nonce -> None" "None None None" "$(b4 '" ".join(str(t.b4_left(x,A)) for x in (g([],issue=4),g([],repo="o/x"),g([],nonce="N8")))')"
check "b4_left: other schema -> None" None "$(b4 't.b4_left({**B,"schema":"shibubu.qa.git/1"},A)')"
check "b4_kept: the same id and body after the restart -> True" True "$(b4 't.b4_kept(A,g([(2,"b",True),(5,"c",False)]))')"
check "b4_kept: the comment is gone -> False" False "$(b4 't.b4_kept(A,B)')"
check "b4_kept: the body was rewritten -> False" False "$(b4 't.b4_kept(A,g([(2,"b2",True)]))')"
check "b4_kept: after has no comment with the nonce -> None" None "$(b4 't.b4_kept(B,A)')"
check "verified_kinds: B4 before/after pair -> Web only" "True False False" "$(b4 '" ".join(str(t.verified_kinds({"c.before.github.json":B,"c.after.github.json":A})[k]) for k in ("Web","NativeApp","Terminal"))')"
check "verified_kinds: B4 after alone -> Web false" False "$(b4 't.verified_kinds({"c.after.github.json":A})["Web"]')"
check "verified_kinds: B4 nonce already before -> Web false" False "$(b4 't.verified_kinds({"c.before.github.json":A,"c.after.github.json":A})["Web"]')"
check "verified_kinds: B4 pair of different steps is not a pair -> Web false" False "$(b4 't.verified_kinds({"c.before.github.json":B,"d.after.github.json":A})["Web"]')"
# github_login: AXWebArea（idx 1）の子孫の語だけ。idx 9 は Shibubu の欄（WebArea の外）に "Sign in" の文字 = 数えない
gl() { python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally as t
def tree(*kids,web=True,title="Issue · GitHub"):
    els=[{"element_index":0,"role":"AXWindow"}]+([{"element_index":1,"parent_index":0,"role":"AXWebArea",**({"label":title} if title is not None else {})}] if web else [])
    els.append({"element_index":9,"parent_index":0,"role":"AXTextField","value":"Sign in"})
    els+=[{"element_index":10+i,"parent_index":(k[2] if len(k)>2 else 1),"role":k[0],"label":k[1]} for i,k in enumerate(kids)]
    return {"elements":els}
print(eval(sys.argv[2]))' "$BG" "$@"; }
check "github_login: Sign in link in the web area -> False" False "$(gl 't.github_login(tree(("AXLink","Sign in")))')"
check "github_login: the comment field and no Sign in -> True" True "$(gl 't.github_login(tree(("AXTextArea","Use Markdown to format your comment")))')"
check "github_login: Sign in only outside the web area -> None" None "$(gl 't.github_login(tree(("AXStaticText","hello")))')"
check "github_login: no web area -> None" None "$(gl 't.github_login(tree(("AXTextArea","Add a comment"),web=False))')"
check "github_login: both words -> False (signed out wins)" False "$(gl 't.github_login(tree(("AXTextArea","Add a comment"),("AXLink","Sign in")))')"
check "github_login: a grandchild of the web area is read" False "$(gl 't.github_login(tree(("AXGroup","box"),("AXButton","Sign in to comment",10)))')"
check "github_login: a word inside a longer text is not counted -> None" None "$(gl 't.github_login(tree(("AXStaticText","Please Sign in later")))')"
# 題の無い AXWebArea（計画の木の形・題が出る前の頁）は読む。題が在って「· GitHub」で終わらない頁（Google）は読まない（holdout B4-6 と実機 run-1 の 19 の両方）
check "github_login: untitled web area: Sign in / comment field / both -> False True False" "False True False" "$(gl '" ".join(str(t.github_login(tree(*k,title=None))) for k in ((("AXLink","Sign in"),),(("AXTextArea","Add a comment"),),(("AXTextArea","Add a comment"),("AXLink","Sign in"))))')"
check "github_login: empty title is untitled -> read" False "$(gl 't.github_login(tree(("AXLink","Sign in"),title=""))')"
check "github_login: a titled page that is not GitHub (Google) -> None" "None None" "$(gl '" ".join(str(t.github_login(tree(k,title="Google"))) for k in (("AXLink","Sign in"),("AXTextArea","Add a comment")))')"
# ax_shape（commit する *.list.txt）: AXWebArea の子孫の label は第三者の頁の文字なので伏せる（run-1 の 23 は Google が IP から推定した居場所を出した）・頁の題と WebArea の外の兄弟は残す
check "ax_shape masks labels under an AXWebArea, keeps its title and the siblings outside" "elements 5|0 0 AXWindow 'Shibubu'|1 1 AXWebArea 'Google'|2 2 AXGroup '…'|3 3 AXStaticText '…'|4 1 AXButton 'Go'" "$(python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as t
e=lambda i,p,d,r,l:{"element_index":i,"parent_index":p,"depth":d,"role":r,"label":l}
print("|".join(t.ax_shape({"elements":[e(0,None,0,"AXWindow","Shibubu"),e(1,0,1,"AXWebArea","Google"),e(2,1,2,"AXGroup","box"),e(3,2,3,"AXStaticText","Hayward, CA"),e(4,0,1,"AXButton","Go")]})))' "$BG" 2>&1)"
# 実機 run-1 の木（commit した AXWebArea の部分木）: GitHub の login の頁は切れている・Google の頁の Sign in は GitHub の login と読まない
W3E="$BG/../../../docs/evidence/W12W-3/run-1"
check "github-login on the real GitHub sign-in page (run-1 25) -> 5" 5 "$(python3 -I "$BG/tally.py" github-login "$W3E/25-read-github.web.json" >/dev/null 2>&1; echo $?)"
check "github-login on the real Google page with a Sign in link (run-1 19) -> 6" 6 "$(python3 -I "$BG/tally.py" github-login "$W3E/19-read.web.json" >/dev/null 2>&1; echo $?)"
mkdir -p "$T/w3"
gl 'json.dump(tree(("AXLink","Sign in")),open(sys.argv[3]+"/out.json","w"))' "$T/w3" >/dev/null
gl 'json.dump(tree(("AXTextArea","Add a comment")),open(sys.argv[3]+"/in.json","w"))' "$T/w3" >/dev/null
gl 'json.dump(tree(),open(sys.argv[3]+"/none.json","w"))' "$T/w3" >/dev/null
echo '{' > "$T/w3/broken.json"
check "tally.py github-login: signed out / in / unreadable / broken -> 5 0 6 4" "5 0 6 4" "$(for f in out in none broken; do python3 -I "$BG/tally.py" github-login "$T/w3/$f.json" >/dev/null 2>&1; printf '%s ' $?; done | sed 's/ $//')"
check "tally.py github-login signed out says one line to the owner" "1" "$(python3 -I "$BG/tally.py" github-login "$T/w3/out.json" 2>&1 >/dev/null | grep -c 'owner が FG 枠')"
b4 'json.dump(B,open(sys.argv[3]+"/b.json","w")) or json.dump(A,open(sys.argv[3]+"/a.json","w"))' "$T/w3" >/dev/null
check "tally.py b4 before after again" '{"left": true, "kept": true}' "$(python3 -I "$BG/tally.py" b4 "$T/w3/b.json" "$T/w3/a.json" "$T/w3/a.json")"
check "tally.py b4 with a non-github record -> 4" 4 "$(python3 -I "$BG/tally.py" b4 "$T/w3/b.json" "$T/w3/in.json" >/dev/null 2>&1; echo $?)"
# run.sh の oracle:github（YC_DEMO_REPO・SHIBUBU_QA_GH_ISSUE・手の --needle。gh は上の偽物: id 1 hello・id 2 "run N123 ok"）
mkdir -p "$T/w3run"
PATH="$T/gh:$PATH" YC_DEMO_REPO=o/demo SHIBUBU_QA_GH_ISSUE=3 expect_oracle "$T/w3run" c after oracle:github N123 2>/dev/null
check "expect_oracle oracle:github reads YC_DEMO_REPO, SHIBUBU_QA_GH_ISSUE and the step needle" "o/demo 3 N123 True" "$(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["repo"],d["issue"],d["nonce"],d["has_nonce"])' "$T/w3run/oracle/c.after.github.json" 2>&1)"
PATH="$T/gh:$PATH" expect_oracle "$T/w3run" d before oracle:github N123 2>/dev/null; rc=$?
check "expect_oracle oracle:github without SHIBUBU_QA_GH_ISSUE -> 2 and no file" "2 no" "$rc $([ -e "$T/w3run/oracle/d.before.github.json" ] && echo yes || echo no)"
PATH="$T/gh:$PATH" SHIBUBU_QA_GH_ISSUE=3 expect_oracle "$T/w3run" e before oracle:github 2>/dev/null; rc=$?
check "expect_oracle oracle:github without a needle -> 2 and no file" "2 no" "$rc $([ -e "$T/w3run/oracle/e.before.github.json" ] && echo yes || echo no)"
PATH="$T/gh:$PATH" GH_FAIL=1 SHIBUBU_QA_GH_ISSUE=3 expect_oracle "$T/w3run" f before oracle:github N123 2>/dev/null; rc=$?
check "expect_oracle oracle:github API failure -> 4 and no file" "4 no" "$rc $([ -e "$T/w3run/oracle/f.before.github.json" ] && echo yes || echo no)"
# tally の count の L1 の ② の免除: repo の名前の行（identifier o/r・成功の語）は、その repo の B4 の対のどれかが True の時だけ免除。
# 同じ run の後の読み直し（Page 移動・Undo の後の --expect oracle:github）の対は before に nonce が在るので False — それで前の True を消さない。
cp "$T/w3/b.json" "$X/run/oracle/c.before.github.json"; cp "$T/w3/a.json" "$X/run/oracle/c.after.github.json"
mkdir -p "$X/run/ax"; echo '{"role":"AXWindow","children":[{"role":"AXStaticText","identifier":"o/r","value":"o/r に comment を保存しました"}]}' > "$X/run/ax/z.json"
lies() { python3 -I "$BG/tally.py" "$X/run" 2>&1 | python3 -I -c 'import json,sys;print(len(json.load(sys.stdin)["lies"]))' 2>&1; }
check "count: the repo row with a success word and a true B4 pair -> lies 0" 0 "$(lies)"
cp "$T/w3/a.json" "$X/run/oracle/d-reread.before.github.json"; cp "$T/w3/a.json" "$X/run/oracle/d-reread.after.github.json"
check "count: a later re-read pair (False) does not undo the true pair -> lies 0" 0 "$(lies)"
rm "$X/run/oracle/c.before.github.json"
check "count: no true pair (lone after and the re-read) -> the repo row is a lie" 1 "$(lies)"

# ---- quiet-window.sh（偽の表・偽の門の値。command は T の中の file を作るだけ）----
QW="$BG/quiet-window.sh"; mkdir -p "$T/qw/cwd"
printf '%s\t%s\t%s\n' '# 註' x y 'ok1' "$T/qw/cwd" 'echo out=/tmp/x; touch ran1; exit 0' 'bad2' "$T/qw/cwd" 'touch ran2; exit 1' 'empty3' "$T/qw/cwd" '# operator が埋める' > "$T/qw/plan.tsv"
check "quiet-window dry-run prints rows and gate values" "rows=3 cargo=0 idle=500s load1=1.5 -> go 3" "$(QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=1.5 bash "$QW" --dry-run "$T/qw/plan.tsv" | awk 'NR==1{l=$0} END{sub(/^quiet-window: /,"",l); print l, NR-1}')"
QW_CARGO_N=0 QW_IDLE_S=10 bash "$QW" "$T/qw/plan.tsv" --out "$T/qw/o1" >/dev/null 2>&1; rc=$?
check "quiet-window no-go (idle 10): exit 6, both command rows unmeasured, command did not run" "6 2 no" "$rc $(grep -c 'unmeasured (idle:10)' "$T/qw/o1/summary.md") $([ -e "$T/qw/cwd/ran1" ] && echo yes || echo no)"
QW_CARGO_N=2 QW_IDLE_S=500 bash "$QW" "$T/qw/plan.tsv" --out "$T/qw/o1b" >/dev/null 2>&1; rc=$?
check "quiet-window no-go (cargo 2): exit 6 and nothing ran" "6 no" "$rc $([ -e "$T/qw/cwd/ran1" ] && echo yes || echo no)"
QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=0 bash "$QW" "$T/qw/plan.tsv" --out "$T/qw/o2" >/dev/null 2>&1; rc=$?
check "quiet-window go: runs in cwd, records exit/state/run dir, fail continues, empty skipped, exit 1" "1 yes yes 1 1 1 2" "$rc $([ -e "$T/qw/cwd/ran1" ] && echo yes || echo no) $([ -e "$T/qw/cwd/ran2" ] && echo yes || echo no) $(grep -c '| 0 | ok | /tmp/x |' "$T/qw/o2/summary.md") $(grep -c '| 1 | fail |' "$T/qw/o2/summary.md") $(grep -c 'skip (command' "$T/qw/o2/summary.md") $(ls "$T/qw/o2"/*.log | wc -l | tr -d ' ')"
rm -f "$T/qw/cwd/ran1" "$T/qw/cwd/ran2"
QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=20 bash "$QW" "$T/qw/plan.tsv" --out "$T/qw/o3" >/dev/null 2>&1; rc=$?
check "quiet-window no-go (load1 20 > cpu): exit 6, reason load1:20 in summary and dry-run, nothing ran" "6 2 no no-go(load1:20)" "$rc $(grep -c 'unmeasured (load1:20)' "$T/qw/o3/summary.md") $([ -e "$T/qw/cwd/ran1" ] && echo yes || echo no) $(QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=20 bash "$QW" --dry-run "$T/qw/plan.tsv" | head -1 | grep -o 'no-go.*')"
rm -f "$T/qw/cwd/ran1"
QW_CARGO_N=1 QW_CARGO_MAX=1 QW_IDLE_S=500 QW_LOAD1=2 QW_LOAD1_MAX=8 bash "$QW" "$T/qw/plan.tsv" --out "$T/qw/o4" >/dev/null 2>&1; rc=$?
check "quiet-window go with QW_CARGO_MAX=1 and cargo 1, load1 2 <= 8: ran (exit 1 = bad2 fails)" "1 yes" "$rc $([ -e "$T/qw/cwd/ran1" ] && echo yes || echo no)"

printf '%s\t%s\t%s\n' 'own6' "$T/qw/cwd" 'exit 6' 'bad1' "$T/qw/cwd" 'exit 1' > "$T/qw/rc6.tsv"
QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=0 bash "$QW" "$T/qw/rc6.tsv" --out "$T/qw/o6" >/dev/null 2>&1; rc=$?
check "quiet-window: command exit 6 (owner_returned) is 'unmeasured (owner)' not fail; exit 1 stays fail; overall 6 (exit 1 is not hidden in the table)" "6 1 1 0" "$rc $(grep -c '| 6 | unmeasured (owner) |' "$T/qw/o6/summary.md") $(grep -c '| 1 | fail |' "$T/qw/o6/summary.md") $(grep -c '| 6 | fail |' "$T/qw/o6/summary.md")"

# W15-b8: --check（偽の表。.app は T の中の dir・driver は --dry-run を持つ偽の script）
mkdir -p "$T/qw/X.app"; printf '[ "$1" = "--dry-run" ] && exit 0\nexit 9\n' > "$T/qw/cwd/drv.sh"
printf 'okrow\t%s\tbash drv.sh %s\nskiprow\t%s\t# 空\n' "$T/qw/cwd" "$T/qw/X.app" "$T/qw/cwd" > "$T/qw/chk-ok.tsv"
printf 'badrow\t%s\tbash drv.sh %s\n' "$T/qw/cwd" "$T/qw/NONE.app" > "$T/qw/chk-ng.tsv"
mkdir -p "$T/qw/cwd/scripts/qa/bg/bin"; for t in axprobe front-guard windows wincap; do : > "$T/qw/cwd/scripts/qa/bg/bin/$t"; chmod +x "$T/qw/cwd/scripts/qa/bg/bin/$t"; done
out=$(bash "$QW" --check "$T/qw/chk-ok.tsv"); rc=$?
check "quiet-window --check: all present -> exit 0, okrow all ok, skiprow skipped" "0 1 1" "$rc $(grep -c '^| okrow | ok | ok | ok | ok | ok |' <<< "$out") $(grep -c 'skiprow.*skip' <<< "$out")"
out=$(bash "$QW" --check "$T/qw/chk-ng.tsv"); rc=$?
check "quiet-window --check: missing .app -> exit 1 and app column ng" "1 1" "$rc $(grep -c '^| badrow | ok | ng | ok |' <<< "$out")"
# W15-b39: 道具の列。axprobe を外すと ng(axprobe)・exit 1（負の対照: 検査を外した旧 script は同じ表で exit 0）
rm "$T/qw/cwd/scripts/qa/bg/bin/axprobe"
out=$(bash "$QW" --check "$T/qw/chk-ok.tsv"); rc=$?
sed 's/\[ -x "\$c\/scripts\/qa\/bg\/bin\/\$t" \] || miss=.*$/true; done/' "$QW" > "$BG/qw-mut6.$$.sh"
out0=$(bash "$BG/qw-mut6.$$.sh" --check "$T/qw/chk-ok.tsv"); rc0=$?; rm -f "$BG/qw-mut6.$$.sh"
check "quiet-window --check tools: no bin/axprobe -> exit 1 and ng(axprobe); negative control: check removed -> exit 0" "1 1|0" "$rc $(grep -c '^| okrow .*| ng(axprobe) |' <<< "$out")|$rc0"

# W15-b3: owner が戻った（violation が front だけ・その手の axprobe の hid_idle_s < 手の間）手は guard_failures でなく unmeasured["owner_returned.<step>"]。入力なしの front・hid_idle_s 無しは従来どおり violation
ow() {  # ow <hid_idle_s|none> [detection]: $R の手 w を「front の violation・手の間 30 秒」にして tally を読む → "<guard_failures> <owner_returned> <state>"
  rm -rf "$T/ow"; cp -R "$R" "$T/ow"
  python3 -I - "$T/ow" "$1" "${2:-front}" "$BG" <<'EOF2'
import json, sys
R, idle, det, bg = sys.argv[1:5]
rows = [json.loads(l) for l in open(R + "/steps.jsonl") if l.strip()]
i = [x["step"] for x in rows].index("w")
if i:
    rows[i - 1]["t1"] = 100.0
rows[i]["t0"], rows[i]["t1"] = 100.0, 130.0
open(R + "/steps.jsonl", "w").write("".join(json.dumps(x, sort_keys=True) + "\n" for x in rows))
rj = json.load(open(R + "/run.json")); rj["schema"] = "shibubu.qa.run/1"; json.dump(rj, open(R + "/run.json", "w"))  # 前の検査が壊した schema を戻す
v = json.load(open(R + "/" + rows[i]["verdict"]))
v["ok"], v["violations"] = False, [{"detection": det, "t": 120.0, "detail": "x"}]
json.dump(v, open(R + "/" + rows[i]["verdict"], "w"))
pf = R + "/" + rows[i]["axprobe"]
p = json.load(open(pf))
if idle == "none":
    p.get("pressure", {}).pop("hid_idle_s", None)
else:
    p.setdefault("pressure", {})["hid_idle_s"] = float(idle)
json.dump(p, open(pf, "w"))
EOF2
  python3 -I "$BG/tally.py" "$T/ow" 2>"$T/ow.err" | python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally;d=json.load(sys.stdin);print(len([g for g in d["guard_failures"] if g.startswith("w:")]),len([k for k in d["unmeasured"] if k=="owner_returned.w"]),tally.guard_state(d["guard_failures"],d["unmeasured"]))' "$BG"
}
check "tally owner_returned: front violation with hid_idle_s 5 < gap 30 -> unmeasured, not a guard failure" "0 1 unmeasured(owner_returned)" "$(ow 5; head -c 300 "$T/ow.err")"
check "tally owner_returned: hid_idle_s 300 >= gap 30 (front changed without input) -> product violation" "1 0 fail(violation)" "$(ow 300)"
check "tally owner_returned: no hid_idle_s -> unreadable, stays a violation" "1 0 fail(violation)" "$(ow none)"
check "tally owner_returned: hid_idle_s 5 but a zorder violation -> stays a violation" "1 0 fail(violation)" "$(ow 5 zorder)"

# W15-b38: violation 0 で未測が focus=no_focus_element / leak=no_needle だけの手は、owner 不在（hid_idle_s か run.json の owner_idle_s ≥ 120）なら guard failure でなく unmeasured["no_focus.<step>" / "no_needle.<step>"]。idle が短い・読めない・他の未測が混じる手は従来どおり
nf() {  # nf <hid_idle_s|none> <run の owner_idle_s|none> <未測 JSON> → "<guard_failures w:> <no_focus.w> <no_needle.w> <state>"
  rm -rf "$T/nf"; cp -R "$R" "$T/nf"
  python3 -I - "$T/nf" "$1" "$2" "$3" <<'EOF3'
import json, sys
R, idle, oidle, un = sys.argv[1:5]
rows = [json.loads(l) for l in open(R + "/steps.jsonl") if l.strip()]
i = [x["step"] for x in rows].index("w")
rj = json.load(open(R + "/run.json")); rj["schema"] = "shibubu.qa.run/1"
if oidle == "none": rj.pop("owner_idle_s", None)
else: rj["owner_idle_s"] = float(oidle)
json.dump(rj, open(R + "/run.json", "w"))
v = json.load(open(R + "/" + rows[i]["verdict"]))
v["ok"], v["violations"], v["unmeasured"] = False, [], json.loads(un)
json.dump(v, open(R + "/" + rows[i]["verdict"], "w"))
pf = R + "/" + rows[i]["axprobe"]
p = json.load(open(pf))
if idle == "none": p.get("pressure", {}).pop("hid_idle_s", None)
else: p.setdefault("pressure", {})["hid_idle_s"] = float(idle)
json.dump(p, open(pf, "w"))
EOF3
  python3 -I "$BG/tally.py" "$T/nf" 2>"$T/nf.err" | python3 -I -c 'import json,sys;sys.path.insert(0,sys.argv[1]);import tally;d=json.load(sys.stdin);print(len([g for g in d["guard_failures"] if g.startswith("w:")]),len([k for k in d["unmeasured"] if k=="no_focus.w"]),len([k for k in d["unmeasured"] if k=="no_needle.w"]),tally.guard_state(d["guard_failures"],d["unmeasured"]))' "$BG"
}
NF='{"focus":"no_focus_element"}'
check "tally no_focus: no focused element + hid_idle_s 2300 (owner away) -> unmeasured no_focus, not a guard failure" "0 1 0 unmeasured(no_focus)" "$(nf 2300 none "$NF"; head -c 300 "$T/nf.err")"
check "tally no_focus: hid_idle_s 5 (owner at the keyboard) -> stays a guard failure" "1 0 0 fail(violation)" "$(nf 5 none "$NF")"
check "tally no_focus: no step idle but run owner_idle_s 2318 -> unmeasured; neither readable -> guard failure" "0 1 0 unmeasured(no_focus) / 1 0 0 fail(violation)" "$(nf none 2318 "$NF") / $(nf none none "$NF")"
check "tally no_needle: leak=no_needle alone is its own unmeasured whatever the idle (idle 5 too); with focus and idle 5 the focus part keeps it a failure" "0 0 1 unmeasured(no_focus) / 1 0 0 fail(violation)" "$(nf 5 none '{"leak":"no_needle"}') / $(nf 5 none '{"focus":"no_focus_element","leak":"no_needle"}')"
check "tally no_focus: another unmeasured reason mixed in (accessibility_denied) -> stays a guard failure" "1 0 0 fail(violation)" "$(nf 2300 none '{"focus":"no_focus_element","ax":"accessibility_denied"}')"

# B64-TOOL 5: 録画 mode（SHIBUBU_QA_RECORD=1）の手は写真と front-guard の judge を飛ばし、「測っていない」verdict を書く（録画の長さに入る・owner が机に居る）。
# tally はその手を緑にも guard failure にもせず unmeasured["record_skipped.<step>"]・guard_state unmeasured(record_mode)・open_unmeasured は数えない（take の exit が毎回 6 にならない）。env 無しは今のまま
r64() {  # r64 <run.sh> <tally.py の dir> <1|空> → "<rc> <写真> <judge> <verdict の unmeasured> <steps.jsonl の shot> <guard_failures w:> <record_skipped.w> <state> <open_unmeasured>"
  local D; D=$(mktemp -d "$T/r64.XXXX"); mkdir -p "$D/steps" "$D/shots"
  python3 -I -c 'import json,sys
D=sys.argv[1]; snap=lambda t: {"schema":"shibubu.qa.snapshot/1","t":t,"front":{"pid":100,"name":"Owner"},"cursor":{"x":1,"y":1},"windows":[],"unmeasured":{}}
json.dump({"schema":"shibubu.qa.run/1","owner_pid":100,"suspects":[200],"window_id":7},open(D+"/run.json","w"))
json.dump(snap(10.0),open(D+"/steps/w.before.json","w")); json.dump(snap(12.0),open(D+"/steps/w.after.json","w"))
open(D+"/events.jsonl","w").write(json.dumps({"schema":"shibubu.qa.event/1","t":9.0,"kind":"start"})+"\n")' "$D"
  printf '#!/bin/bash\necho "$1" >> "%s/fg.calls"\n[ "$1" = judge ] && echo "{\\"schema\\":\\"shibubu.qa.verdict/1\\",\\"step\\":\\"w\\",\\"ok\\":true,\\"violations\\":[],\\"unmeasured\\":{},\\"measured\\":[\\"front\\"]}"\nexit 0\n' "$D" > "$D/fg"; chmod +x "$D/fg"
  local rc; rc=$( ( source "$1"; screencapture() { echo x > "$D/shot.calls"; : > "${@: -1}"; }; FRONT_GUARD="$D/fg" SHIBUBU_QA_RECORD=$3 judge_step "$D" w "$D/events.jsonl" click '{}' 0 "" "" "" >/dev/null 2>&1; echo $? ) | tail -1 )
  python3 -I - "$D" "$2" "$rc" <<'EOFR64'
import json, os, sys
D, td, rc = sys.argv[1:4]; TR = D  # tally は r64 の dir だけを読む（前の節の $R の状態に頼らない）
sys.path.insert(0, td); import tally
v = json.load(open(D + "/steps/w.verdict.json")); row = json.loads(open(D + "/steps.jsonl").read().splitlines()[-1])
calls = open(D + "/fg.calls").read().split() if os.path.exists(D + "/fg.calls") else []
t = tally.count(TR)
print(rc, "shot" if os.path.exists(D + "/shot.calls") else "no-shot", "judge" if "judge" in calls else "no-judge", json.dumps(v.get("unmeasured"), sort_keys=True).replace(" ", ""), row["shot"],
      len([g for g in t.guard_failures if g.startswith("w:")]), int("record_skipped.w" in t.unmeasured), tally.guard_state(t.guard_failures, t.unmeasured),
      ",".join(k for k in tally.open_unmeasured(t.unmeasured) if k.startswith("record_skipped.")) or "-")
EOFR64
}
python3 -I - "$BG" "$$" <<'EOFR64M'
import sys
bg, pid = sys.argv[1:3]
rs, ts = open(f"{bg}/run.sh", encoding="utf-8").read(), open(f"{bg}/tally.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"run-r64a.{pid}.sh", mut(rs, "  if rec; then\n    py 'import json,sys;open(sys.argv[1],\"w\").write(json.dumps({\"schema\":\"shibubu.qa.verdict/1\"", "  if false; then\n    py 'import json,sys;open(sys.argv[1],\"w\").write(json.dumps({\"schema\":\"shibubu.qa.verdict/1\""))  # 録画 mode でも写真と judge を打つ
os_ = f"{bg}/r64m.{pid}"; __import__("os").makedirs(os_, exist_ok=True)
open(f"{os_}/tally.py", "w", encoding="utf-8").write(mut(ts, "        if rs := record_skipped(v):", "        if rs := None:"))  # tally が録画の手を読み分けない
w(f"r64n.{pid}.py", mut(ts, 'and not k.removeprefix("restart:").startswith("record_skipped.")]', "]"))  # open_unmeasured が録画の手も数える
EOFR64M
mkdir -p "$BG/r64n.$$" && mv "$BG/r64n.$$.py" "$BG/r64n.$$/tally.py"
for f in "$BG"/*.py; do [ -e "$BG/r64m.$$/$(basename "$f")" ] || cp "$f" "$BG/r64m.$$/"; [ -e "$BG/r64n.$$/$(basename "$f")" ] || cp "$f" "$BG/r64n.$$/"; done
check "B64-TOOL 5: SHIBUBU_QA_RECORD=1 -> judge_step takes no screenshot and no judge, writes verdict unmeasured.record_mode=not_judged (rc 0, shot null); tally reads that step as unmeasured record_skipped.w (not a guard failure), state unmeasured(record_mode), open_unmeasured leaves it out; no env -> screenshot + judge as before; negative controls: the rec branch off -> shot+judge in record mode; tally without record_skipped -> guard failure; open_unmeasured without the exclusion -> counts it" \
  '0 no-shot no-judge {"record_mode":"not_judged"} None 0 1 unmeasured(record_mode) -|0 shot judge {} shots/w.png 0 0 ok -|shot judge|1 0 fail(violation)|record_skipped.w' \
  "$(r64 "$BG/run.sh" "$BG" 1)|$(r64 "$BG/run.sh" "$BG" '')|$(r64 "$BG/run-r64a.$$.sh" "$BG" 1 | cut -d' ' -f2-3)|$(r64 "$BG/run.sh" "$BG/r64m.$$" 1 | cut -d' ' -f6-8)|$(r64 "$BG/run.sh" "$BG/r64n.$$" 1 | cut -d' ' -f9)"
rm -rf "$BG"/run-r64a.$$.sh "$BG"/r64m.$$ "$BG"/r64n.$$

# ---- ui-owner-check（owner の UI の決定を code から読む門・fixture は scripts/qa/fixtures/ui-check）----
UIC="$BG/../ui-owner-check.py"; UIF="$BG/../fixtures/ui-check"
uic() { local o=$1; shift; rm -rf "$T/uic"; cp -R "$UIF/green" "$T/uic"; [ "$o" = green ] || cp -R "$UIF/$o/." "$T/uic/"; python3 -I "$UIC" --root "$T/uic" "$@" 2>&1; }
ids_py() { python3 -I -c 'import re,sys;sys.path.insert(0,sys.argv[1]);import importlib.util as u;s=u.spec_from_file_location("m",sys.argv[1]+"/ui-owner-check.py");m=u.module_from_spec(s);s.loader.exec_module(m);md=open(sys.argv[2]).read();print(len(m.CHECKS),sorted(c[0] for c in m.CHECKS)==sorted(re.findall(r"^\| `(\w+)` \|",md,re.M)))' "$BG/.." "$BG/../../../docs/evidence/UI/owner-check.md"; }
uic green >/dev/null; rc=$?
check "ui-owner-check: green fixture -> exit 0, and the md table has the same ids as CHECKS" "0 28 True" "$rc $(ids_py)"
out=$(uic red-blue); rc=$?
check "ui-owner-check: button painted blue with a hairline drawn in its own body (not through frame_of) -> exit 1, exactly the blue-refs and no-stroke rows are red" "1 d2_btn_no_stroke,d4_blue_refs" "$rc $(echo "$out" | awk '$1=="赤"{print $2}' | paste -sd, -)"
out=$(uic red-two-fields); rc=$?
check "ui-owner-check: two command fields -> exit 1, only one_field is red" "1 d1_one_field" "$rc $(echo "$out" | awk '$1=="赤"{print $2}' | paste -sd, -)"
out=$(uic red-renamed-fields); rc=$?
check "ui-owner-check: two TextEdits with names no list knows (input / text) -> exit 1, only one_field is red (counts constructions, not variable names)" "1 d1_one_field 2" "$rc $(echo "$out" | awk '$1=="赤"{print $2}' | paste -sd, -) $(echo "$out" | grep -o 'd1_one_field.*今: [0-9]*' | grep -o '[0-9]*$')"
out=$(uic red-no-sidebar-store); rc=$?
check "ui-owner-check: UiSettings without a sidebar field -> exit 1, only sidebar_saved is red" "1 a2_sidebar_saved" "$rc $(echo "$out" | awk '$1=="赤"{print $2}' | paste -sd, -)"
uic red-blue --wave UI-1 >/dev/null; r1=$?; uic red-blue --wave UI-3 >/dev/null; r3=$?; uic red-blue --wave UI-0 >/dev/null; r0=$?
check "ui-owner-check: the same blue tree is pending (6) before UI-3 and red (1) at UI-3; nothing is due at UI-0 (6)" "6 1 6" "$r1 $r3 $r0"

# 負の対照 = green の木を 1 か所だけ壊す（perl で置換）。赤の id が「壊した所の行だけ」になる事を見る
uicm() { local expr=$1 file=$2; shift 2; rm -rf "$T/uic"; cp -R "$UIF/green" "$T/uic"; perl -0pi -e "$expr" "$T/uic/$file"; python3 -I "$UIC" --root "$T/uic" "$@" 2>&1; }
reds() { awk '$1=="赤"{print $2}' | paste -sd, -; }
out=$(uicm 's/t\.surface_press \}/t.surface_3 }/g' src/ui/theme.rs); rc=$?
check "ui-owner-check: press painted surface_3 not surface_press -> only d2_btn_fill is red" "1 d2_btn_fill" "$rc $(echo "$out" | reds)"
out=$(uicm 's/button_compact\(ui, label, Variant::Ghost\)/ui.painter().rect(rect, fill, Stroke::NONE)/' src/ui/theme.rs); rc=$?
check "ui-owner-check: chip draws its own face with no frame (UI-SWEEP-1101) -> only d2_btn_framed is red" "1 d2_btn_framed" "$rc $(echo "$out" | reds)"
out=$(uicm 's/rect\(rect, fill, frame_of\(ui, enabled\)\)/rect(rect, fill, Stroke::NONE)/' src/ui/theme.rs); rc=$?
check "ui-owner-check: button_sized stops asking frame_of for its frame -> only d2_btn_framed is red" "1 d2_btn_framed" "$rc $(echo "$out" | reds)"
# owner 209（枠は Settings・Connections・Runs の中だけ）: 範囲の印を全体で立てる / 3 つの画面で外す / frame_of が範囲を見ない
out=$(uicm 's/\z/fn g(ui: &mut Ui) { let _framed = theme::framed(ui.ctx()); }\n/' src/app.rs); rc=$?
check "ui-owner-check: the framed scope raised outside Settings / Connections / Runs (app.rs) -> only d2_btn_no_stroke is red" "1 d2_btn_no_stroke" "$rc $(echo "$out" | reds)"
out=$(uicm 's/let _framed = theme::framed\(ui\.ctx\(\)\);//' src/ui/settings.rs); rc=$?
check "ui-owner-check: Settings stops raising the framed scope -> only d2_btn_framed is red" "1 d2_btn_framed" "$rc $(echo "$out" | reds)"
out=$(uicm 's/    if !ui\.ctx\(\)\.data[^\n]*\n//' src/ui/theme.rs); rc=$?
check "ui-owner-check: frame_of frames everywhere (no framed-scope guard) -> only d2_btn_no_stroke is red" "1 d2_btn_no_stroke" "$rc $(echo "$out" | reds)"
out=$(uicm 's/\z/fn x(ui: &mut Ui) { ui.button("x"); }\n/' src/app.rs); rc=$?
check "ui-owner-check: a bare ui.button outside any native_buttons scope -> only d2_native_buttons is red" "1 d2_native_buttons" "$rc $(echo "$out" | reds)"
out=$(uicm 's/theme::native_buttons\(ui\); //' src/app.rs); rc=$?
check "ui-owner-check: menu_button without native_buttons -> red with exactly 1 (the menu itself; the button inside its closure is not counted)" "1 d2_native_buttons 1" "$rc $(echo "$out" | reds) $(echo "$out" | grep -o 'の素の button [0-9]*' | grep -o '[0-9]*$')"
out=$(uicm 's/\z/fn y(t: &Tokens) -> Color32 { t.working }\n/' src/app.rs); rc=$?
check "ui-owner-check: .working used outside states.rs -> only d4_working_only_state is red" "1 d4_working_only_state" "$rc $(echo "$out" | reds)"
out=$(uicm 's/BUTTON_PAD_X/BUTTON_INSET/' src/ui/theme.rs); rc=$?
check "ui-owner-check: BUTTON_PAD_X renamed (the name is kept on purpose) -> only d5_btn_inset is red" "1 d5_btn_inset" "$rc $(echo "$out" | reds)"
out=$(uicm 's/ctrl: true, mac_cmd: true, \.\.Default::default\(\)/ctrl: true, ..Default::default()/; s/\z/fn z(i: &InputState) -> bool { i.consume_key(Modifiers { mac_cmd: true, ..Default::default() }, Key::Z) }\n/' src/ui/shell.rs); rc=$?
check "ui-owner-check: Key::S with ctrl only while a neighbouring consume_key has mac_cmd (the old 200-char window was green) -> only a2_sidebar_key is red" "1 a2_sidebar_key" "$rc $(echo "$out" | reds)"
out=$(uicm 's/CliConfig/TerminalConfig2/' src/llm_cli.rs); rc=$?
check "ui-owner-check: the model field is not on CliConfig (a TerminalConfig has it) -> only d6_attach_model is red" "1 d6_attach_model" "$rc $(echo "$out" | reds)"
out=$(uicm 's/attachments/files/' src/domain/app.rs); rc=$?
check "ui-owner-check: Goal has no attachments -> only d6_attach_model is red" "1 d6_attach_model" "$rc $(echo "$out" | reds)"

# UI-6（W15-b18）: before = yc/ui a18b14c の形（reduce は 0・Ready を描く・Page が llm_line・execution_strip）を fixture に写した物。1 本ずつ壊す負の対照
out=$(uic red-ui6-before --wave UI-6); rc=$?
check "ui-owner-check: yc/ui's UI-6 shape (reduce 0, Ready drawn, llm_line on a Page, execution_strip) -> exit 1, exactly the 5 UI-6 rows red; dur-token / timer / bounce stay green" "1 a1_reduce_fade,a6_ready_silent,a6_once_per_pass,a6_no_page_partial,a6_exec_strip_gone" "$rc $(echo "$out" | reds)"
# Wave を a / b に割る（W15-b19）: 6b の行（execution_strip）だけを足した木。UI-6a の門は 6b の行を「未」で exit 6・UI-6（= 6a ∪ 6b）は従来どおり赤
EXS='s/\z/fn execution_strip() {}\n/'
out=$(uicm "$EXS" src/ui/shell.rs --wave UI-6a); rc=$?
check "ui-owner-check: --wave UI-6a with only the UI-6b row (execution_strip) failing -> exit 6, that row is 未(UI-6b), nothing red" "6 a6_exec_strip_gone:未(UI-6b) [] " "$rc $(echo "$out" | awk '$1 ~ /^未/ && $2=="a6_exec_strip_gone"{print $2":"$1}') [$(echo "$out" | reds)] "
out=$(uicm "$EXS" src/ui/shell.rs --wave UI-6); rc=$?
check "ui-owner-check: --wave UI-6 (= 6a + 6b, the old meaning) on the same tree -> exit 1, a6_exec_strip_gone red" "1 a6_exec_strip_gone" "$rc $(echo "$out" | reds)"
uic green --wave UI-9 >/dev/null; r9=$?; uic green --wave UI-6c >/dev/null; r6c=$?
check "ui-owner-check: an unknown wave (UI-9, UI-6c) -> exit 2 (argparse)" "2 2" "$r9 $r6c"
rm -rf "$T/uimut"; mkdir "$T/uimut"; cp "$BG/../ui-owner-check.py" "$BG/../_rs.py" "$T/uimut/"
perl -0pi -e 's/("a6_exec_strip_gone", "[^"]*", )"UI-6b"/$1"UI-6a"/' "$T/uimut/ui-owner-check.py"
rm -rf "$T/uic"; cp -R "$UIF/green" "$T/uic"; perl -0pi -e "$EXS" "$T/uic/src/ui/shell.rs"
check "ui-owner-check negative control: exec_strip's wave put back to UI-6a -> --wave UI-6a is red (exit 1), so the 6b test above measures the wave" "1" "$(python3 -I "$T/uimut/ui-owner-check.py" --root "$T/uic" --wave UI-6a >/dev/null 2>&1; echo $?)"
out=$(uic red-keep-unrelated); rc=$?
check "ui-owner-check: no Keep button, only the example text \"Keep research notes\" -> only a3_confirm_inline is red (the literal alone no longer counts)" "1 a3_confirm_inline" "$rc $(echo "$out" | reds)"
out=$(uicm 's/theme::button_compact\(ui, "Keep", Variant::Ghost\);//; s/\z/fn k(ui: &mut Ui) { theme::button_compact(ui, "Keep", Variant::Ghost); }\n/' src/app.rs); rc=$?
check "ui-owner-check: Delete and Keep buttons in different blocks -> only a3_confirm_inline is red" "1 a3_confirm_inline" "$rc $(echo "$out" | reds)"
out=$(uicm 's/MotionMode::Fade => base\.min\(REDUCE_FADE\)/MotionMode::Fade => 0.0/' src/ui/motion.rs); rc=$?
check "ui-owner-check: Fade arm of dur is 0.0 -> only a1_reduce_fade is red" "1 a1_reduce_fade" "$rc $(echo "$out" | reds)"
out=$(uicm 's/MotionMode::Fade => base\.min\(REDUCE_FADE\)/MotionMode::Fade => base.min(PAGE)/' src/ui/motion.rs); rc=$?
check "ui-owner-check: Fade arm does not use the fade token (const defined but unused) -> only a1_reduce_fade is red" "1 a1_reduce_fade" "$rc $(echo "$out" | reds)"
out=$(uicm 's/MotionMode::Cut => 0\.0/MotionMode::Cut => REDUCE_FADE/' src/ui/motion.rs); rc=$?
check "ui-owner-check: Cut arm is not 0 -> only a1_reduce_fade is red" "1 a1_reduce_fade" "$rc $(echo "$out" | reds)"
out=$(uicm 's/motion_mode\(ui\.ctx\(\)\) == MotionMode::Full/true/' src/ui/motion.rs); rc=$?
check "ui-owner-check: slide_in no longer reads the mode -> only a1_reduce_fade is red" "1 a1_reduce_fade" "$rc $(echo "$out" | reds)"
out=$(uicm 's/if state == RegionState::Ready \{ return; \}//' src/ui/states.rs); rc=$?
check "ui-owner-check: state_line without the Ready early return -> only a6_ready_silent is red" "1 a6_ready_silent" "$rc $(echo "$out" | reds)"
out=$(uicm 's/None => \{\}/None => { ui.label(RegionState::Ready.glyph()); }/' src/ui/renderer/mod.rs); rc=$?
check "ui-owner-check: usable_line_in draws the Ready glyph -> only a6_ready_silent is red" "1 a6_ready_silent" "$rc $(echo "$out" | reds)"
out=$(uicm 's/let pass = [^\n]*\n//' src/ui/states.rs); rc=$?
check "ui-owner-check: state_line without cumulative_pass_nr -> only a6_once_per_pass is red" "1 a6_once_per_pass" "$rc $(echo "$out" | reds)"
out=$(uicm 's/states::inline\(ui, RegionState::Error, "x"\);/if let Some(l) = self.llm_line() { states::inline(ui, RegionState::Partial, l); }/' src/generated.rs); rc=$?
check "ui-owner-check: draw_page calls llm_line -> only a6_no_page_partial is red" "1 a6_no_page_partial" "$rc $(echo "$out" | reds)"
out=$(uicm 's/dur\(ctx, PAGE\)/dur(ctx, 0.22)/' src/ui/motion.rs); rc=$?
check "ui-owner-check: a number literal passed to dur( -> only a6_duration_token is red" "1 a6_duration_token" "$rc $(echo "$out" | reds)"
out=$(uicm 's/dur\(ctx, PAGE\)/dur(ctx, HOLD_RECEIPT)/' src/ui/motion.rs); rc=$?
check "ui-owner-check: HOLD_RECEIPT passed to dur( -> only a6_timer_not_dur is red" "1 a6_timer_not_dur" "$rc $(echo "$out" | reds)"
out=$(uicm 's/\z/fn s(c: &Context) { shell::execution_strip(c, 1); }\n/' src/app.rs); rc=$?
check "ui-owner-check: execution_strip referenced -> only a6_exec_strip_gone is red" "1 a6_exec_strip_gone" "$rc $(echo "$out" | reds)"
out=$(uicm 's/\z/fn bounce_in() {}\n/' src/ui/shell.rs); rc=$?
check "ui-owner-check: a bounce identifier -> only a6_no_bounce is red" "1 a6_no_bounce" "$rc $(echo "$out" | reds)"
out=$(uicm 's/\z/const SAVE_DEBOUNCE: u8 = 1; fn Debounce_it() {}\n/' src/ui/shell.rs); rc=$?
check "ui-owner-check: debounce is not a bounce (negative of the negative) -> exit 0" "0" "$rc"

# ---- merge-preview.sh（一時 repo。MP は変異を入れた copy で差し替えられる）----
MP="${MP:-$BG/../merge-preview.sh}"
mkdir -p "$T/mp" && ( cd "$T/mp" && git init -q -b main . && git config user.email t@t && git config user.name t \
  && printf 'line1\nline2\nline3\n' > a.rs && printf 'fn shared() {}\n' > b.rs && printf 'x\n' > c.rs && git add a.rs b.rs c.rs && git commit -qm base \
  && git checkout -q -b ours && printf 'OURS\nline2\nline3\n' > a.rs && printf 'fn shared() {}\nconst SEAT: u8 = 1;\n' > b.rs && git add a.rs b.rs && git commit -qm o \
  && git checkout -q main && git checkout -q -b theirs-conf && printf 'THEIRS\nline2\nline3\n' > a.rs && git add a.rs && git commit -qm t1 \
  && git checkout -q main && git checkout -q -b theirs-dup && printf 'pub const SEAT: u8 = 2;\nfn shared() {}\n' > b.rs && git add b.rs && git commit -qm t2 \
  && git checkout -q main && git checkout -q -b theirs-free && printf 'y\n' > d.rs && git add d.rs && git commit -qm t3 )
mp() { ( cd "$T/mp" && bash "$MP" "$@" 2>&1 ); }
out=$(mp ours theirs-conf); rc=$?
check "merge-preview: both sides edit the same line -> exit 1, one conflict file a.rs with 1 hunk" "1 1 1" "$rc $(echo "$out" | grep -c '^| a.rs | 1 |') $(echo "$out" | grep -c '衝突 1 file / 重複定義の候補 0')"
out=$(mp ours theirs-dup); rc=$?
check "merge-preview: no conflict but both sides add const SEAT in b.rs -> exit 1, 1 dup candidate" "1 1 1" "$rc $(echo "$out" | grep -c '^| b.rs | SEAT |') $(echo "$out" | grep -c '衝突 0 file / 重複定義の候補 1')"
out=$(mp ours theirs-free); rc=$?
check "merge-preview: unrelated change -> exit 0, nothing in any table; bad rev -> exit 2" "0 1 2" "$rc $(echo "$out" | grep -c '衝突 0 file / 重複定義の候補 0 / 同じ file 0') $(mp ours nosuchrev >/dev/null; echo $?)"

# ---- retest-red.sh（偽の cargo を PATH の先頭に置く。RR は変異を入れた copy で差し替えられる）----
RR="${RR:-$BG/../retest-red.sh}"
mkdir -p "$T/rr/bin" "$T/rr/n"
cat > "$T/rr/bin/cargo" <<'FAKE'
#!/bin/bash
echo "$*" >> "$RR_DIR/cargo.args"
for a in "$@"; do [ "$a" = -- ] && break; name=$a; done
n=$(cat "$RR_DIR/n/$name" 2>/dev/null || echo 0); echo $((n + 1)) > "$RR_DIR/n/$name"
r=$(awk -v t="$name" -v k=$((n + 1)) '$1 == t { print $(k + 1) }' "$RR_DIR/behave")
case "$r" in
  green) echo "test result: ok. 1 passed; 0 failed; 0 ignored"; exit 0 ;;
  red) echo "test $name ... FAILED"; echo "test result: FAILED. 0 passed; 1 failed; 0 ignored"; exit 101 ;;
  *) echo "error: could not compile"; exit 101 ;;
esac
FAKE
chmod +x "$T/rr/bin/cargo"
printf 'flaky green green\nreal green red\nbroken x x\nuni green green\n' > "$T/rr/behave"
printf '     Running tests/gate.rs (target/debug/deps/gate-1)\ntest flaky ... FAILED\ntest real ... FAILED\ntest ok_one ... ok\ntest result: FAILED. 1 passed; 2 failed\n     Running unittests src/main.rs (target/debug/deps/shibubu-2)\ntest uni ... FAILED\n   Doc-tests shibubu\ntest doc_one ... FAILED\n' > "$T/rr/red.log"
printf '     Running tests/gate.rs (x)\ntest a ... ok\ntest result: ok. 1 passed; 0 failed\n' > "$T/rr/green.log"
printf '     Running tests/gate.rs (x)\ntest broken ... FAILED\n' > "$T/rr/broken.log"
rr() { ( RR_DIR="$T/rr" PATH="$T/rr/bin:$PATH" bash "$RR" "$@" 2>&1 ); }
rm -f "$T/rr/cargo.args"; rm -f "$T/rr/n"/*
out=$(RR_DIR="$T/rr" PATH="$T/rr/bin:$PATH" bash "$RR" "$T/rr/red.log" 2>&1); rc=$?
check "retest-red: flaky green/green = 環境由来, real green/red = 実物, uni on --bin shibubu, doc-tests skipped -> exit 1" "1 1 1 1 2 0" "$rc $(echo "$out" | grep -c '^| flaky | --test gate | 緑 / 緑 | .* | 環境由来 |') $(echo "$out" | grep -c '^| real | --test gate | 緑 / 赤 | .* | 実物 |') $(echo "$out" | grep -c '^| uni | --bin shibubu | 緑 / 緑 | .* | 環境由来 |') $(grep -c '^test --bin shibubu uni -- --exact --nocapture$' "$T/rr/cargo.args") $(echo "$out" | grep -c doc_one)"
out=$(rr "$T/rr/green.log"); rc=$?
check "retest-red: log with no red -> exit 0, 赤 0" "0 1" "$rc $(echo "$out" | grep -c '赤 0 本')"
out=$(rr "$T/rr/broken.log"); rc=$?
check "retest-red: a test that cannot compile is 壊れ -> exit 1; unreadable log -> 2" "1 1 2" "$rc $(echo "$out" | grep -c '| 壊れ |') $(rr "$T/rr/nosuch.log" >/dev/null; echo $?)"

# ---- ui-gate.sh（偽の検査 script・偽の cargo。UG は変異を入れた copy で差し替えられる。UI_GATE_ROOT で偽の root）----
UG="${UG:-$BG/../ui-gate.sh}"
mkroot() {   # $1 = root。全部緑の偽の道具一式（python は exit 0 で 1 行出すだけ）
  mkdir -p "$1/scripts/qa" "$1/bin"
  for f in scripts/interfaces.py scripts/check_app_agnostic.py scripts/qa/check_wired.py scripts/qa/ui-owner-check.py; do echo 'print("fake ok")' > "$1/$f"; done
  printf '# "--set"\nprint("fake ok")\n' > "$1/scripts/check_ui_copy.py"
  printf '# "--max-ja" "ui-2b" "outside":\nprint("fake ok")\n' > "$1/scripts/qa/ui-strings.py"
  printf '#!/bin/bash\necho "test result: ok. 1 passed; 0 failed"\n' > "$1/bin/cargo"; chmod +x "$1/bin/cargo"
}
ug() { local r=$1; shift; ( UI_GATE_ROOT="$r" PATH="$r/bin:$PATH" bash "$UG" "$@" 2>&1 ); }
mkroot "$T/ug1"
out=$(ug "$T/ug1" --wave UI-2); rc=$?
check "ui-gate: all fake tools green -> exit 0, 9 rows 緑 and the last line counts them" "0 9 1" "$rc $(echo "$out" | grep -c '| 緑 | 0 |') $(echo "$out" | grep -c '^UI-2: 緑 9 / 赤 0 / 環境由来 0 / 無し 0 / 飛ばす 0 → exit 0$')"
mkroot "$T/ug2"; echo 'print("wired broke: 3"); raise SystemExit(1)' > "$T/ug2/scripts/qa/check_wired.py"
out=$(ug "$T/ug2" --wave UI-2 --no-cargo); rc=$?
check "ui-gate: one tool exits 1 -> exit 1, its row is 赤 with the last line, the rest still run; --no-cargo rows are 飛ばす" "1 1 6 2" "$rc $(echo "$out" | grep -c '^| 3 | wired | .* | 赤 | 1 | wired broke: 3 |$') $(echo "$out" | grep -c '| 緑 | 0 |') $(echo "$out" | grep -c '| 飛ばす |')"
mkroot "$T/ug3"; rm "$T/ug3/scripts/check_ui_copy.py"; echo 'print("old")' > "$T/ug3/scripts/qa/ui-strings.py"
out=$(ug "$T/ug3" --wave UI-2); rc=$?
check "ui-gate: missing tool / old version without --max-ja are 無し with the branch, not counted -> exit 0" "0 3 1" "$rc $(echo "$out" | grep -c '| 無し | - | 無し（在る branch: ') $(echo "$out" | grep -c '緑 6 / 赤 0 / 環境由来 0 / 無し 3 ')"
out=$(ug "$T/ug1" --wave UIX; echo "rc=$?"); check "ui-gate: bad --wave -> exit 2" "rc=2" "$(echo "$out" | tail -1)"
# 負の対照: 無しを赤に数える変異を入れた copy では 3 本目の期待（exit 0）が崩れる
sed 's/cnt\["赤"\] == 0/cnt["赤"] + cnt["無し"] == 0/g' "$UG" > "$T/ug-mut.sh"
check "ui-gate negative control: counting 無し as red turns the 3rd check red (exit 1)" "1" "$(UI_GATE_ROOT="$T/ug3" PATH="$T/ug3/bin:$PATH" bash "$T/ug-mut.sh" --wave UI-2 >/dev/null 2>&1; echo $?)"
# cargo が赤で retest-red.sh が環境由来と言えば赤に数えない・実物なら数える
mkroot "$T/ug4"; printf '#!/bin/bash\necho "test result: FAILED. 0 passed; 1 failed"; exit 101\n' > "$T/ug4/bin/cargo"
printf '#!/bin/bash\necho "赤 1 本: 実物 0 / 環境由来 1 / 壊れ 0"; exit 0\n' > "$T/ug4/scripts/qa/retest-red.sh"
out=$(ug "$T/ug4" --wave UI-2); rc=$?
check "ui-gate: cargo red but retest-red says 環境由来 only -> exit 0, counted apart" "0 2" "$rc $(echo "$out" | grep -c '| 環境由来 | 101 |')"
printf '#!/bin/bash\necho "赤 1 本: 実物 1 / 環境由来 0 / 壊れ 0"; exit 1\n' > "$T/ug4/scripts/qa/retest-red.sh"
out=$(ug "$T/ug4" --wave UI-2); rc=$?
check "ui-gate: cargo red and retest-red says 実物 -> exit 1" "1 2" "$rc $(echo "$out" | grep -c '| 赤 | 101 |')"

# ---- integ-ui.sh（一時 repo。IU は変異を入れた copy で差し替えられる・merge は一時 repo の中だけ・偽の interfaces.py）----
IU="${IU:-$BG/../integ-ui.sh}"
mkdir -p "$T/iu" && ( cd "$T/iu" && git init -q -b main . && git config user.email t@t && git config user.name t && mkdir -p docs scripts \
  && printf 'import sys\nf="docs/YC_INTERFACES.md"\nif "--check" in sys.argv: sys.exit(0 if open(f).read()=="generated\\n" else 1)\nopen(f,"w").write("generated\\n")\n' > scripts/interfaces.py \
  && printf 'base\n' > docs/YC_INTERFACES.md && printf 'l1\nl2\n' > a.rs && git add scripts/interfaces.py docs/YC_INTERFACES.md a.rs && git commit -qm base \
  && git checkout -q -b ours && printf 'ours\n' > docs/YC_INTERFACES.md && printf 'OURS\nl2\n' > a.rs && printf 'o\n' > o.rs && git add docs/YC_INTERFACES.md a.rs o.rs && git commit -qm o \
  && git checkout -q main && git checkout -q -b theirs-free && printf 'f\n' > f.rs && git add f.rs && git commit -qm t1 \
  && git checkout -q main && git checkout -q -b theirs-ifc && printf 'theirs\n' > docs/YC_INTERFACES.md && printf 't\n' > t.rs && git add docs/YC_INTERFACES.md t.rs && git commit -qm t2 \
  && git checkout -q main && git checkout -q -b theirs-both && printf 'theirs2\n' > docs/YC_INTERFACES.md && printf 'THEIRS\nl2\n' > a.rs && git add docs/YC_INTERFACES.md a.rs && git commit -qm t3 \
  && git checkout -q ours )
iu() { ( cd "$T/iu" && git merge --abort >/dev/null 2>&1; git reset -q --hard ours-base 2>/dev/null; bash "$IU" "$@" 2>&1 ); }
( cd "$T/iu" && git branch ours-base ours )
out=$(iu plan theirs-free theirs-ifc); rc=$?
check "integ-ui plan: prints the preview and the command order, changes nothing" "0 1 1 1 2" "$rc $(echo "$out" | grep -c '^# merge-preview') $(echo "$out" | grep -c '^1\. bash scripts/qa/integ-ui.sh merge theirs-free') $(echo "$out" | grep -c '^3\. bash scripts/qa/integ-ui.sh regen') $(cd "$T/iu" && git rev-list --count HEAD)"
out=$(iu merge theirs-free); rc=$?
check "integ-ui merge: no conflict -> exit 0, a --no-ff merge commit with the INTEG-UI subject" "0 2 1" "$rc $(cd "$T/iu" && git rev-list --parents -1 HEAD | wc -w | tr -d ' ' | awk '{print $1-1}') $(cd "$T/iu" && git log -1 --format=%s | grep -c '^統合: theirs-free（.*）を ours へ — INTEG-UI$')"
out=$(iu merge theirs-ifc); rc=$?
check "integ-ui merge: only YC_INTERFACES conflicts -> regenerated by interfaces.py and committed (no markers, 2 parents)" "0 generated 0 2" "$rc $(cd "$T/iu" && cat docs/YC_INTERFACES.md) $(cd "$T/iu" && git status --porcelain --untracked-files=no | wc -l | tr -d ' ') $(cd "$T/iu" && git rev-list --parents -1 HEAD | wc -w | tr -d ' ' | awk '{print $1-1}')"
out=$(iu merge theirs-both); rc=$?
check "integ-ui merge: another file conflicts too -> exit 3, merge left half-done with a.rs UU, table names a.rs but not YC_INTERFACES-only" "3 1 1" "$rc $(cd "$T/iu" && git status --porcelain | grep -c '^UU a.rs') $(echo "$out" | grep -c '^| a.rs |$')"
# 負の対照: 「YC_INTERFACES だけ」の判定を外した copy では 4 本目が exit 3 にならない
sed 's/"\$conf" != "\$IFC"/1 -eq 0/' "$IU" > "$BG/../iu-mut.$$.sh"
rc=$(IU="$BG/../iu-mut.$$.sh"; iu merge theirs-both >/dev/null; echo $?); rm -f "$BG/../iu-mut.$$.sh"
check "integ-ui negative control: dropping the 'YC_INTERFACES only' test no longer gives exit 3 on a real conflict" "1" "$([ "$rc" != 3 ] && echo 1 || echo 0)"

# ---- ui-strings.py の scan（1 行の全部の literal・format! の中・raw string・join の区切りを数える・W15-b22）----
US="${US:-$BG/../ui-strings.py}"; USF="$BG/../fixtures/ui-strings-scan"
uslines() { python3 -I "$1" --root "$USF" --csv /dev/stdout | awk -F, 'NR>1 && $4=="ja"{n[$2]++} END{for(l=1;l<=9;l++) if(n[l]) printf "%d:%d ", l, n[l]}'; }
check "ui-strings scan: 2 JP literals on one line, one inside format!, a raw string and a .join(\"・\") separator are all counted (line:count)" "3:2 4:2 5:1 6:1 " "$(uslines "$US")"
# 負の対照: 1 行に 1 件しか数えない変異・join を飛ばす呼び出しに戻した変異では期待が崩れる
sed 's/^            rows.append(/            if not rows or rows[-1][:2] != (rel, line_of(src, a)): rows.append(/' "$US" > "$BG/../us-mut1.$$.py"
sed 's/"open", "id", "Id"}/"open", "join", "id", "Id"}/' "$US" > "$BG/../us-mut2.$$.py"
got1=$(uslines "$BG/../us-mut1.$$.py"); got2=$(uslines "$BG/../us-mut2.$$.py"); rm -f "$BG/../us-mut1.$$.py" "$BG/../us-mut2.$$.py"
check "ui-strings scan negative control: counting one literal per line gives 3:1 4:1 5:1 6:1; skipping join drops line 6" "3:1 4:1 5:1 6:1 |3:2 4:2 5:1 " "$got1|$got2"

# ---- ui-strings.py --dup-jp: 2 つの表で同じ JP に違う EN（W15-b40）----
printf '# s\n\n| JP | EN |\n|---|---|\n| あ | One |\n| い | Same |\n| う | Z |\n| `*` x*y | S |\n| 注 | (note) |\n' > "$T/dj-a.md"
printf '# s\n\n| JP | EN |\n|---|---|\n| い | Same |\n| あ | Two |\n| う | Zed |\n| `*` x*y | T |\n| 注 | Other |\n' > "$T/dj-b.md"
djn() { python3 -I "${1:-$US}" --dup-jp --map "$T/dj-a.md" --map "$T/dj-b.md" | sed -n 's/^dup-jp \([0-9]*\).*/\1/p'; }
check "ui-strings --dup-jp: 2 JPs with different EN across two tables (the same-EN, the \`*\` key and the note row are not counted)" "2" "$(djn)"
check "ui-strings --dup-jp: the table lists A file:line || B file:line for the JP" "dj-a.md:5 dj-b.md:6" "$(python3 -I "$US" --dup-jp --map "$T/dj-a.md" --map "$T/dj-b.md" | awk -F' [|] ' '/ あ /{n=split($1,x,"/"); m=split($2,y,"/"); print x[n], y[m]}')"
sed 's/Zed/Z/' "$T/dj-b.md" > "$T/dj-b3.md"
dj3() { python3 -I "$US" --dup-jp "$@" --map "$T/dj-a.md" --map "$T/dj-b3.md"; }
check "ui-strings --dup-jp: aligning one pair leaves 1 (exit 1); --max-dup 1 passes (exit 0)" "1 1 0" "$(dj3 | sed -n 's/^dup-jp \([0-9]*\).*/\1/p' | tr -d '\n'; dj3 >/dev/null; printf ' %s' $?; dj3 --max-dup 1 >/dev/null; printf ' %s' $?)"
sed 's/ and "\*" not in jp//' "$US" > "$BG/../us-mut3.$$.py"; got3=$(djn "$BG/../us-mut3.$$.py"); rm -f "$BG/../us-mut3.$$.py"
check "ui-strings --dup-jp negative control: counting the \`*\` key too gives 3" "3" "$got3"

export QA_BG="$BG"  # w15_judge は labels.py を QA_BG から読む（壊した写しを別の場所に置いても同じ表）
# ---- INTEG-3 run-3 の記録の写し: drive-integ1.py の判定 2 つ（W15-b30）----
IJ="${IJ:-$BG/../../../docs/evidence/W15/w15_judge.py}"; IF="$BG/../fixtures/integ-run3"
ij() {  # ij <w15_judge.py>: run-3 の再起動の木 → 入口の role / 一言の type_text → 状態 / 手の状態
  python3 -I - "$1" "$IF" <<'EOF3'
import importlib.util, json, sys
sp = importlib.util.spec_from_file_location("j", sys.argv[1]); J = importlib.util.module_from_spec(sp); sp.loader.exec_module(J)
els = json.load(open(sys.argv[2] + "/restart-52-elements.json"))["elements"]
e = J.restart_entry(els)
need = ["tests_added", "project_restored"]
print(e and e["role"], J.type_state(json.load(open(sys.argv[2] + "/type-28.cua.json"))), J.hands_state({"tests_added": None, "project_restored": e is not None}, need), J.hands_state({"tests_added": True, "project_restored": e is not None}, need))
EOF3
}
check "integ run-3: restart tree reads the project (AXCheckBox 開発環境) -> restored; type_text partial -> partial; a None hand is unmeasured, not false" "AXCheckBox partial unmeasured True" "$(ij "$IJ")"
# 負の対照: 入口を旧 driver の role の表（Button / StaticText / Row / OutlineRow）に戻すと再起動が読めず false 側へ戻る・None を False 扱いにすると未測が消える
sed 's/return next((e for e in els if any(n in _txt(e) for n in L.names("dev_env"))/return next((e for e in els if e.get("role") in ("AXButton", "AXStaticText", "AXRow", "AXOutlineRow") and any(n in _txt(e) for n in L.names("dev_env"))/' "$IJ" > "$BG/../ij-mut1.$$.py"
sed 's/return False if any(v is False for v in vals)/return False if any(not v for v in vals)/' "$IJ" > "$BG/../ij-mut2.$$.py"
g1=$(ij "$BG/../ij-mut1.$$.py"); g2=$(ij "$BG/../ij-mut2.$$.py"); rm -f "$BG/../ij-mut1.$$.py" "$BG/../ij-mut2.$$.py"
check "integ run-3 negative control: old role filter -> project not found (None partial False False: the hand turns false); None as False -> hands False" "None partial False False|AXCheckBox partial False True" "$g1|$g2"

# ---- 主デモの driver（drive-w15.py）が同じ判定を w15_judge 経由で持つ（W15-b31）----
DW="${DW:-$BG/../../../docs/evidence/W15/drive-w15.py}"
dw() {  # dw <drive-w15.py>: 入口は w15_judge.restart_entry か・旧 role の表が残っていないか・type_text partial を扱うか・最後の合否が hands_state か → 判定を fixture の写しに当てる
  python3 -I - "$IJ" "$IF" "$1" <<'EOF4'
import importlib.util, json, re, sys
sp = importlib.util.spec_from_file_location("j", sys.argv[1]); J = importlib.util.module_from_spec(sp); sp.loader.exec_module(J)
src = open(sys.argv[3]).read()
els = json.load(open(sys.argv[2] + "/restart-52-elements.json"))["elements"]
wired = "J.restart_entry(els)" in src and "AXOutlineRow" not in src and 'J.type_state(o) == "partial"' in src
need = re.search(r"ok = J\.hands_state\(h, (\[.*?\])\)", src)
need = json.loads(need.group(1)) if need else ["x"]
h = {k: True for k in need}
h["project_restored"] = J.restart_entry(els) is not None
h["tests_added"] = None if J.type_state(json.load(open(sys.argv[2] + "/type-28.cua.json"))) == "partial" else True
print(wired, J.hands_state(h, need), J.hands_state({**h, "tests_added": True}, need))
EOF4
}
check "drive-w15: restart entry / type partial go through w15_judge; run-3 restart tree = restored, Tests partial = unmeasured (not false), all hands true = true" "True unmeasured True" "$(dw "$DW")"
# 負の対照: 入口を旧 role の表に戻す / 合否を bool(...) に戻すと、写しが赤くなる
sed 's/proj = J.restart_entry(els)/proj = next((e for e in els if e.get("role") in ("AXButton", "AXStaticText", "AXRow", "AXOutlineRow") and "開発環境" in str(e.get("label"))), None)/' "$DW" > "$BG/../dw-mut1.$$.py"
sed 's/^ok = J.hands_state(h, \(.*\))  #.*$/ok = bool(h.get("x"))/' "$DW" > "$BG/../dw-mut2.$$.py"
g1=$(dw "$BG/../dw-mut1.$$.py"); g2=$(dw "$BG/../dw-mut2.$$.py"); rm -f "$BG/../dw-mut1.$$.py" "$BG/../dw-mut2.$$.py"
check "drive-w15 negative control: old role table -> not wired; hands_state replaced by bool -> the need list is lost and the unmeasured hand turns True" "False unmeasured True|True True True" "$g1|$g2"

# ---- W15-b57: 録画のデモを英語に。同梱の型の name は表の EN（copy.en.md:156）・Home の一言に CJK・nonce が無い・再起動の入口は EN の木でも JP の古い木でも見つかる ----
tpl() { python3 -I - "$1" "$2" <<'EOF5'
import json, re, sys, importlib.util, os, types
root, ij = sys.argv[1], sys.argv[2]
names = [json.load(open(f"{root}/assets/templates/{k}.json", encoding="utf-8"))["name"] for k in ("dev", "music", "tasks")]
src = open(f"{root}/scripts/qa/bg/rehearse.py", encoding="utf-8").read()
said = re.search(r'def h_generate.*?said = "([^"]*)"(.*)', src, re.S)
sp = importlib.util.spec_from_file_location("j", ij); J = importlib.util.module_from_spec(sp); sp.loader.exec_module(J)
els = json.load(open(f"{root}/scripts/qa/fixtures/integ-run3/restart-52-elements.json"))["elements"]
en = [{"role": "AXCheckBox", "label": "Dev environment", "element_token": "t:9"}]
print(names, bool(re.search("[ぁ-んァ-ヶ一-龥]", said.group(1))), "C.nonce" in said.group(2).split("\n")[0], J.restart_entry(els) is not None, J.restart_entry(en)["element_token"])
EOF5
}
WANT="['Development', 'Music', 'Tasks'] False False True t:9"
check "b57: templates named in EN, Home sentence has no CJK / nonce, restart entry found in the old JP tree and in an EN tree" "$WANT" "$(tpl "$BG/../../.." "$IJ")"
# 負の対照: dev.json の name を 開発環境 に戻す / 入口の表を JP だけにすると赤
RT57="$T/rt57"; mkdir -p "$RT57/assets/templates" "$RT57/scripts/qa/bg" "$RT57/scripts/qa/fixtures"
cp "$BG/../../../assets/templates/"*.json "$RT57/assets/templates/"; cp "$BG/rehearse.py" "$RT57/scripts/qa/bg/"; cp -R "$IF" "$RT57/scripts/qa/fixtures/integ-run3"
sed -i.bak 's/"name": "Development"/"name": "開発環境"/' "$RT57/assets/templates/dev.json"
check "b57 negative control: dev.json name back to JP -> red" "False" "$([ "$(tpl "$RT57" "$IJ")" = "$WANT" ] && echo True || echo False)"
rm -rf "$RT57"

# ---- label の 1 か所の表（labels.py）: driver は EN の画面でも JP の画面でも同じ手で通る（W15-b32）----
LB="${LB:-$BG/labels.py}"; LF="$BG/../fixtures/labels"
lb() {  # lb <labels.py>: 表の全 key を JP の木 / EN の木（fixture）で引く → 当たった数 と EN で当たらない key
  python3 -I - "$1" "$LF" <<'EOF5'
import importlib.util, json, sys
sp = importlib.util.spec_from_file_location("labels", sys.argv[1]); L = importlib.util.module_from_spec(sp); sp.loader.exec_module(L)
tree = lambda n: json.load(open(f"{sys.argv[2]}/tree-{n}.json"))["elements"]
KEYS = ["goal", "create", "change", "term_in", "add_term", "save_repo", "mirror", "add", "send", "run_tests", "reload", "title", "save", "undo", "redo", "back", "drawer",
        "checking", "running_cmd", "exit_code", "usage_refuse", "saved_line", "connected", "saved_msg", "finished", "failed", "ai_cap", "human_typing", "memo", "mirror_denied"]
def got(els, k):
    try:
        return any(L.hit(e, k, contains=L.LABELS[k][0] == "AXStaticText") and e["element_token"] == "t-" + k for e in els)
    except Exception:
        return False
jp, en = tree("jp"), tree("en")
miss = sorted(k for k in KEYS if not got(en, k))
print(sum(got(jp, k) for k in KEYS), sum(got(en, k) for k in KEYS), ",".join(miss))
EOF5
}
check "labels: 30 keys all found in the JP tree; 28 in the EN tree (title, memo wait for UI-n / UI-5a and are searched in JP; the status-line keys are matched by containment)" "30 28 memo,title" "$(lb "$LB")"
# identifier が在れば label に依らず当たる（mirror-consent:* は label が空でも）・literal（ユーザーが名前を付けた tab）はそのまま 1 語
idt() {
  python3 -I - "$1" <<'EOF6'
import importlib.util, sys
sp = importlib.util.spec_from_file_location("labels", sys.argv[1]); L = importlib.util.module_from_spec(sp); sp.loader.exec_module(L)
print(L.hit({"identifier": "mirror-consent:com.apple.Notes", "label": ""}, "mirror"), L.hit({"label": "Tests"}, "Tests"), L.hit({"label": "Create a thing"}, "create"))
EOF6
}
check "labels: identifier hit without a label; a literal tab name matches itself; a longer label does not match exactly" "True True False" "$(idt "$LB")"
# 負の対照: 表から create の行を消すと EN の木で create が当たらない・mirror の identifier を消すと identifier だけの要素が当たらない
sed '/^    "create": /d' "$LB" > "$BG/lb-mut1.$$.py"; sed 's/"mirror-consent:\*"/None/' "$LB" > "$BG/lb-mut2.$$.py"
g1=$(lb "$BG/lb-mut1.$$.py"); g2=$(idt "$BG/lb-mut2.$$.py"); rm -f "$BG/lb-mut1.$$.py" "$BG/lb-mut2.$$.py"
check "labels negative control: the create row removed -> not found in either tree (29 27); the mirror identifier removed -> identifier-only element not hit" "29 27 create,memo,title|False True False" "$(echo "$g1" | sed 's/create,save,title/create,save,title/')|$g2"
# driver の配線（W15-b44 で 17 本）: 5 本とも labels を import し、find / press / send_field / eq / tok の label に JP の literal を直に書かない。rehearse.find は表を通す
RH="${RH:-$BG/rehearse.py}"
LD="$BG/../../../docs/evidence"
DRV15="$BG/drive-w2b-ax.py $BG/drive-w12w1c.py $BG/rehearse.py $BG/drive-i3.py $BG/drive-b4.py $LD/W15/drive-w15.py $LD/integ-2026-10-24/drive-integ1.py $LD/W5/drive-w5.py $LD/W9/drive-w9.py $LD/W10/drive-w10.py $LD/W11/drive-w11.py $LD/W11/drive-w11b.py $LD/W13c/drive-w13c.py $LD/W14/drive-w14.py $LD/W14b/drive-w14b.py $LD/integ-pty-qa/drive-integ.py $LD/W2b-AX/drive-axfix-term.py"
wl() {  # wl <rehearse.py>: 17 driver の JP literal の数（AST・W15-b34）と import の数・rehearse.find が EN / JP の木で当たるか
  python3 -I - "$BG" "$LD" "$LF" "$1" $DRV15 <<'EOF7'
import importlib.util, json, subprocess, sys
bg, ev, lf, rh = sys.argv[1:5]
files = sys.argv[5:]
n_imp = sum(("import labels as L" in open(f, encoding="utf-8").read() or "import rehearse as RH" in open(f, encoding="utf-8").read()) for f in files)
n_jp = int(subprocess.run(["python3", "-I", bg + "/jp_literals.py", bg, *files], capture_output=True, text=True).stdout)
sp = importlib.util.spec_from_file_location("rh", rh); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
ok = []
for n in ("jp", "en"):
    t = json.load(open(f"{lf}/tree-{n}.json"))["elements"]; m.elements = lambda C, t=t: t
    try:
        ok.append(m.find(None, "AXButton", "create", tries=1)["element_token"] == "t-create" and m.find(None, "AXTextField", "change", tries=1)["element_token"] == "t-change")
    except Exception:
        ok.append(False)
print(n_jp, n_imp, *ok)
EOF7
}
check "drivers: 0 JP label literals in the 17 python drivers (AST), all 17 reach the table (labels or rehearse), rehearse.find reads the create / change fields in the JP tree and the EN tree" "0 17 True True" "$(wl "$RH")"
# 負の対照: rehearse.find を完全一致に戻すと両方の木で赤・drive-w15 に JP の literal を 1 つ戻すと数が 1 になる
sed 's/and L.hit(e, label, contains):/and any((label in (e.get(k) or "")) if contains else ((e.get(k) or "") == label) for k in ("label", "value", "title")):/' "$RH" > "$BG/rh-mut1.$$.py"
sed 's/press("AXButton", "create")/press("AXButton", "作る")/' "$LD/W15/drive-w15.py" > "$LD/W15/dw-mut2.$$.py"
g1=$(wl "$BG/rh-mut1.$$.py"); rm -f "$BG/rh-mut1.$$.py"
sed 's/send_field("AXTextField", "term_in",/send_field("AXTextField", "Terminal に打つ",/' "$LD/W11/drive-w11b.py" > "$LD/W11/dw-mut3.$$.py"
sed 's/has("human_typing")/has("人が Terminal に打っている間は")/' "$LD/W11/drive-w11b.py" > "$LD/W11/dw-mut4.$$.py"
g2=$(python3 -I "$BG/jp_literals.py" "$BG" "$LD/W15/dw-mut2.$$.py"); g3=$(python3 -I "$BG/jp_literals.py" "$BG" "$LD/W11/dw-mut3.$$.py")
g4=$(python3 -I "$BG/jp_literals.py" "$BG" "$LD/W11/dw-mut4.$$.py"); rm -f "$LD/W15/dw-mut2.$$.py" "$LD/W11/dw-mut3.$$.py" "$LD/W11/dw-mut4.$$.py"
check "drivers negative control: find back to exact match on the key -> both trees red (0 5 False False); a JP literal put back in drive-w15 / in drive-w11b (2 field sends, a status line) -> 1 / 2 / 1" "0 17 False False|1 2 1" "$g1|$g2 $g3 $g4"
# UI-5a: Page の tab は WidgetInfo::selected（toggled）= macOS では AXCheckBox。rehearse の goto_tab（full.md の 5・7・9）は tab を押し、同じ名前の文字は押さない
gt() {  # gt <rehearse.py>: AXStaticText「Docs」と AXCheckBox「Docs」の木で h_goto_tab が click した token（測れなければ例外の名前）
  python3 -I - "$1" <<'EOF9'
import importlib.util, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
m.elements = lambda C: [{"role": "AXStaticText", "label": "Docs", "element_token": "t-text"}, {"role": "AXCheckBox", "label": "Docs", "element_token": "t-tab"}]
m.time.sleep = lambda s: None
m.db_state = lambda C: {"d": {"surface_specs": [{"id": "docs", "backend": "mirror", "target": {"file": "README.md"}}], "pages": [{"title": "Docs", "root": {"component": "column", "children": [{"component": "source_surface", "props": {"surface_ref": "docs"}}]}}]}}
hit = []
m.step = lambda C, name, args, *extra: hit.append(args.get("element_token")) or {}
try:
    m.h_goto_tab(types.SimpleNamespace(nonce="n"), {"doing": "『Docs』のタブを押す"}); print(*hit)
except Exception as e:
    print(type(e).__name__)
EOF9
}
sed 's/e.get("role") == "AXCheckBox" and not e.get("md") and e.get("label") == title/e.get("role") == "AXButton" and not e.get("md") and e.get("label") == title/; s/press(C, "AXCheckBox", title,/press(C, "AXButton", title,/' "$RH" > "$BG/rh-mut5.$$.py"; g5=$(gt "$BG/rh-mut5.$$.py"); rm -f "$BG/rh-mut5.$$.py"
check "rehearse goto_tab presses the AXCheckBox tab, not the text with the same name; negative control: role back to AXButton -> Unmeasured" "t-tab|Unmeasured" "$(gt "$RH")|$g5"

# demo-loop r2: 動作の無い AXStaticText（承認 / 拒否 / 結果の 1 行）は cua-driver の elements に無く tree_markdown にだけ出る（r2 の 152 手目の形）。
# rehearse は md の行も読む・find は md の行を押す先にしない・wait_seen は読み 1 回で needles を全部見て時計で切る（読み 1 回 61 秒の偽の時計で 60 秒 = 1 回）
ws() {  # ws <rehearse.py>: "<wait_seen が md の承認の行を見たか> <find が md の行で Unmeasured か> <61 秒の読みで 60 秒待つ時の読みの回数>"
  python3 -I - "$1" <<'EOFWS'
import importlib.util, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
md = '- [0] AXWindow "Shibubu"\n  - [21] AXButton "Send changes" [actions=[press]]\n  - AXStaticText = "▣ Needs approval · Approval needed before sending"\n  - [23] AXButton "Approve and send" [actions=[press]]'
clock = [0.0]; reads = []
m.time = types.SimpleNamespace(time=lambda: clock[0], sleep=lambda s: None)
def fstep(C, tool, args, *extra, own=False):
    reads.append(tool); clock[0] += 61
    return {"elements": [{"role": "AXButton", "label": "Send changes", "element_token": "t-send"}, {"role": "AXButton", "label": "Approve and send", "element_token": "t-ok"}], "tree_markdown": md}
m.step = fstep; C = types.SimpleNamespace(depth=8)
hit = m.wait_seen(C, ("Approval needed",), 60)[0] == "Approval needed"
try:
    m.find(C, None, "Approval needed", tries=1, contains=True); f = "found"
except m.Unmeasured:
    f = "Unmeasured"
reads.clear(); m.wait_seen(C, ("nope", "nada"), 60)
print(hit, f, len(reads))
EOFWS
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();a=t.replace("o.get(\"tree_markdown\") or \"\"", "\"\"",1);b=t.replace("if not e.get(\"md\") and (role","if (role",1);import re;c=re.sub(r"    end = time\.time\(\) \+ secs\n    while True:\n        els = fresh\(elements\(C\), base\)\n        for n in needles:\n            t = seen\(C, n, role, els\)\n            if t:\n                return n, t\n        if time\.time\(\) >= end:\n            return None, None\n        time\.sleep\(1\)","    for _ in range(int(secs / 3) + 1):\n        for n in needles:\n            t = seen(C, n, role)\n            if t:\n                return n, t\n        time.sleep(3)\n    return None, None",t);assert a!=t and b!=t and c!=t;[open(f"{sys.argv[2]}.{k}.py","w",encoding="utf-8").write(x) for k,x in (("a",a),("b",b),("c",c))]' "$RH" "$BG/rh-ws.$$"
wsa=$(ws "$BG/rh-ws.$$.a.py"); wsb=$(ws "$BG/rh-ws.$$.b.py"); wsc=$(ws "$BG/rh-ws.$$.c.py"); rm -f "$BG/rh-ws.$$".?.py
check "rehearse reads md-only AXStaticText (approval line), find skips it, wait_seen is 1 read per poll and clock-bounded; negative controls: drop md lines / find takes md / old count loop" "True Unmeasured 1|False|found|42" "$(ws "$RH")|$(echo "$wsa" | cut -d' ' -f1)|$(echo "$wsb" | cut -d' ' -f2)|$(echo "$wsc" | cut -d' ' -f3)"

# demo-loop r10: 動作の無い AXImage（Mirror の矩形 `Not running · …`）は tree_markdown の `- AXImage "…"`（label・`=` 無し）にだけ出る（r10-restart の 190 手目の形）→ seen が見る。
# te_windows は 100×100 未満の窓（menu bar の 1680×24・title bar の脇の 52×20）を外す（r10 の #25: 52×20 が再起動の前だけ在って D1 が赤）
im() {  # im <rehearse.py>: "<seen が md の AXImage を見たか> <te_windows の窓 id>"
  python3 -I - "$1" <<'EOFIM'
import importlib.util, json, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
md = '- [0] AXWindow "Shibubu" [actions=[raise]]\n  - [17] AXCheckBox "notes" [actions=[press]]\n  - AXImage "Not running · No TextEdit window with docs/notes.md open was found"\n  - AXStaticText = "⊘ Not connected"'
m.step = lambda C, tool, args, *extra, own=False: {"elements": [{"role": "AXCheckBox", "label": "notes", "element_token": "t"}], "tree_markdown": md}
C = types.SimpleNamespace(depth=8, state={"te_pid": 5})
b = lambda i, w, h: {"window_id": i, "pid": 5, "layer": 0, "bounds": {"x": 0, "y": 0, "w": w, "h": h}}
m.os = types.SimpleNamespace(path=types.SimpleNamespace(join=lambda *a: "/".join(a), exists=lambda p: True))
m.subprocess = types.SimpleNamespace(run=lambda *a, **k: types.SimpleNamespace(stdout=json.dumps({"windows": [b(2655, 500, 500), b(2657, 1680, 24), b(2683, 52, 20)]})))
print(bool(m.seen(C, "Not running · ", "AXImage")), ",".join(str(w[0]) for w in m.te_windows(C)))
EOFIM
}
python3 -I - "$RH" "$BG/rh-im.$$" <<'EOFIMM'
import sys
t = open(sys.argv[1], encoding="utf-8").read()
a = t.replace('(AX\\w+) "(.*?)"', '(AXNOPE) "(.*?)"', 1)
b = t.replace(' and w["bounds"]["w"] >= 100 and w["bounds"]["h"] >= 100', '', 1)
assert a != t and b != t
for k, x in (("a", a), ("b", b)):
    open(f"{sys.argv[2]}.{k}.py", "w", encoding="utf-8").write(x)
EOFIMM
ima=$(im "$BG/rh-im.$$.a.py"); imb=$(im "$BG/rh-im.$$.b.py"); rm -f "$BG/rh-im.$$".?.py
check "rehearse sees md-only AXImage (Mirror frame Not running) and te_windows drops <100x100 windows; negative controls: no label md / no size filter" "True 2655|False 2655|True 2655,2657,2683" "$(im "$RH")|$ima|$imb"


# W15-b45: driver が探す key は EN を持つ（status が pending の key を使う driver を一覧 → allowlist 以外は赤）
pk() {  # pk <labels.py>: 17 driver + rehearse が使う key のうち pending な物の "<driver>:<key>" を allowlist 抜きで出す
  python3 -I - "$1" $DRV15 $BG/mirror-e1.sh $BG/mirror-e0.sh $BG/mirror-w16m3.sh <<'EOF10'
import ast, importlib.util, os, re, sys
sp = importlib.util.spec_from_file_location("labels", sys.argv[1]); L = importlib.util.module_from_spec(sp); sp.loader.exec_module(L)
PEND = {k for k, v in L.LABELS.items() if v[4].startswith("pending")}
ALLOW = {"undo", "redo", "back", "drawer", "memo",  # UI-5a 待ち
         "title", "failed", "human_typing",  # 表に EN の出典が無い（UI-n 待ち・W15-b45 のスコープ外）。EN が決まったら外す
         "press_track", "press_channel"}  # M2（G-e の承認の 1 行の押す先）待ち。M2 の commit の後に en を埋めて外す（W15-M3）
out = set()
for f in sys.argv[2:]:
    src = open(f, encoding="utf-8").read()
    if f.endswith(".py"):
        used = {n.value for n in ast.walk(ast.parse(src)) if isinstance(n, ast.Constant) and isinstance(n.value, str)}
    else:
        used = set(re.findall(r'[\w"\' ]+', src.replace('"', " ").replace("'", " ")))
    out |= {os.path.basename(f) + ":" + k for k in (PEND - ALLOW) & used}
print(",".join(sorted(out)))
EOF10
}
check "labels: no driver uses a pending key outside the allowlist (UI-5a: undo / redo / back / drawer / memo; no EN source yet: title / failed / human_typing)" "" "$(pk "$LB")"
sed 's/("Save",), None, "en"/(), None, "pending:UI-n"/' "$LB" > "$BG/lb-mut3.$$.py"; g1=$(pk "$BG/lb-mut3.$$.py"); rm -f "$BG/lb-mut3.$$.py"
check "labels negative control: the EN of save removed -> the drivers that use save are listed" "drive-integ.py:save,drive-w2b-ax.py:save" "$g1"
# W15-b44: gfinal の外の driver 3 本（drive-integ・drive-axfix-term・mirror-e1/e0/w16m3）も labels.py の key で探す
DI="$LD/integ-pty-qa/drive-integ.py"
hp() {  # hp <drive-integ.py>: drive-integ の tok / has を JP の木 / EN の木（fixture）に当てる → "<jp> <en>"（1 = create / term_in の token が取れ undo / redo が有り Docs は無い）
  python3 -I - "$1" "$BG" "$LF" <<'EOF8'
import ast, json, sys
src, bg, lf = sys.argv[1:4]
sys.path.insert(0, bg)
import labels as L
ns = {"L": L}
for n in ast.parse(open(src, encoding="utf-8").read()).body:
    if isinstance(n, ast.FunctionDef) and n.name in ("txt", "tok", "has"):
        exec(compile(ast.Module([n], []), src, "exec"), ns)
tok, has = ns["tok"], ns["has"]
out = []
for lang in ("jp", "en"):
    els = json.load(open(f"{lf}/tree-{lang}.json"))["elements"]
    out.append(int(tok(els, "AXButton", "create") == "t-create" and tok(els, "AXTextField", "term_in") == "t-term_in"
                   and has(els, "AXButton", "undo") and has(els, "AXButton", "redo") and tok(els, "AXButton", "Docs") is None))
print(*out)
EOF8
}
check "drive-integ: tok / has find create, term_in, undo, redo by key in the JP tree and the EN tree; a literal (Docs) is not in either" "1 1" "$(hp "$DI")"
sed 's/any(v in L.names(label) for v in txt(e))/label in txt(e)/' "$DI" > "$LD/integ-pty-qa/di-mut1.$$.py"
g1=$(hp "$LD/integ-pty-qa/di-mut1.$$.py"); rm -f "$LD/integ-pty-qa/di-mut1.$$.py"
sed 's/eq("goal")/eq("何を作りたいですか？")/' "$DI" > "$LD/integ-pty-qa/di-mut2.$$.py"
sed 's/press("AXTextField", "term_in")/press("AXTextField", "Terminal に打つ")/' "$LD/W2b-AX/drive-axfix-term.py" > "$LD/W2b-AX/dx-mut3.$$.py"
g2=$(python3 -I "$BG/jp_literals.py" "$BG" "$LD/integ-pty-qa/di-mut2.$$.py"); g3=$(python3 -I "$BG/jp_literals.py" "$BG" "$LD/W2b-AX/dx-mut3.$$.py")
rm -f "$LD/integ-pty-qa/di-mut2.$$.py" "$LD/W2b-AX/dx-mut3.$$.py"
check "drive-integ negative control: tok back to an exact JP/EN-blind match -> 0 0; a JP literal put back in drive-integ (2 eq calls) / drive-axfix-term -> 2 / 1" "0 0|2 1" "$g1|$g2 $g3"
# shell: token() は labels.py --token（identifier も見る・EN / JP の両方）・find_press の call は key
ME1="${ME1:-$BG/mirror-e1.sh}"; ME0="$BG/mirror-e0.sh"; M3="$BG/mirror-w16m3.sh"
tk() {  # tk <labels.py の dir> <tree.json> <key> [role]: mirror-e1.sh の token() の 1 行を取り出して fixture の木に当てる → element_token（無ければ空）
  local d=$T/tk.$$; mkdir -p "$d"; cp "$2" "$d/s.json"
  ( BG=$1; R=$d; eval "$(grep '^token(){' "$ME1")"; token s "$3" "${4:-}" )
}
tkall() { echo "$(tk "$1" "$LF/tree-jp.json" mirror)|$(tk "$1" "$LF/tree-en.json" mirror)|$(tk "$1" "$LF/tree-jp.json" memo AXCheckBox)|$(tk "$1" "$LF/tree-en.json" memo AXCheckBox)|$(tk "$1" "$LF/tree-jp.json" mirror_denied)"; }
check "mirror-e1 token(): the mirror button is found in the JP tree and the EN tree, the memo tab in JP only (pending UI-5a), the denied line in JP" "t-mirror|t-mirror|t-memo||t-mirror_denied" "$(tkall "$BG")"
mkdir -p "$T/lbm"; sed 's/("Mirror here",)/()/' "$LB" > "$T/lbm/labels.py"
check "mirror-e1 token() negative control: the EN of mirror removed from the table -> not found in the EN tree" "t-mirror||t-memo||t-mirror_denied" "$(tkall "$T/lbm")"
check "labels.py --print: EN names then the JP, one per line; a literal prints itself" "Create/Create · ⌘↵/作る Dev" "$(python3 -I "$LB" --print create | paste -sd/ -) $(python3 -I "$LB" --print Dev)"
cj() {  # cj <shell>...: find_press の call と --token の key に日本語が直に書かれている行の数
  python3 -I - "$@" <<'EOF9'
import re, sys
CJK = re.compile("[ぁ-んァ-ヶ一-龥＋]")
n = 0
for f in sys.argv[1:]:
    for l in open(f, encoding="utf-8"):
        m = re.match(r'find_press\s+\S+\s+(\S+)', l) or re.search(r'--token\s+\S+\s+(\S+)', l)
        n += bool(m and CJK.search(m.group(1)))
print(n)
EOF9
}
sed 's/find_press consent mirror AXButton/find_press consent "映す" AXButton/' "$ME1" > "$BG/me-mut1.$$.sh"
g1=$(cj "$ME1" "$ME0" "$M3"); g2=$(cj "$BG/me-mut1.$$.sh"); rm -f "$BG/me-mut1.$$.sh"
check "mirror shells: 0 JP labels in find_press / --token calls of mirror-e1 / e0 / w16m3 (syntax ok); one put back -> 1" "0 ok|1" "$g1 $(bash -n "$ME1" && bash -n "$ME0" && bash -n "$M3" && echo ok)|$g2"

# W15-b33: rehearse の Mirror の同意の手 = init の後に README を TextEdit で後ろに開き、終わりに「自分の pid だけ」終了（owner の TextEdit は触らない）
mt() {  # mt <rehearse.py>: dry-run の 22 行目に TextEdit の段取りが出るか・mine() が開く前に居た pid を除くか・kill_mine が自分の pid だけ殺すか → "<出た数> <mine> <自分 dead> <他人 alive>"
  python3 -I - "$1" <<'EOF9'
import importlib.util, os, subprocess, sys, time
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); spec.loader.exec_module(rh)
dry = subprocess.run([sys.executable, "-I", sys.argv[1], "1", "--dry-run"], capture_output=True, text=True).stdout
shown = sum(1 for ln in dry.splitlines() if ln.startswith("22.") and "TextEdit" in ln and "自分の pid だけ" in ln)
other, my = subprocess.Popen(["sleep", "30"]), subprocess.Popen(["sleep", "30"])
before, after = {other.pid}, {other.pid, my.pid}
m = rh.mine(before, after) == [my.pid]
rh.kill_mine(before, after); time.sleep(0.3)
print(shown, m, my.poll() is not None, other.poll() is None)
other.kill(); other.wait(); my.wait()
EOF9
}
check "rehearse mirror consent: dry-run shows the TextEdit step on row 22, mine() excludes pre-existing pids, kill_mine ends only my pid" "1 True True True" "$(mt "$BG/rehearse.py")"
# 負の対照: 段取りの表を消すと 0 · mine を「全部」に変えると他人の pid が死ぬ
sed 's/^SETUP = {.*$/SETUP = {}/' "$BG/rehearse.py" > "$BG/rh-mut3.$$.py"; sed 's/return sorted(set(after) - set(before))/return sorted(set(after))/' "$BG/rehearse.py" > "$BG/rh-mut4.$$.py"
m3=$(mt "$BG/rh-mut3.$$.py" | cut -d' ' -f1); m4=$(mt "$BG/rh-mut4.$$.py" | cut -d' ' -f2,4); rm -f "$BG/rh-mut3.$$.py" "$BG/rh-mut4.$$.py"
check "rehearse mirror consent negative control: no SETUP -> 0 shown; mine() = everything -> mine False and the other pid dies (False False)" "0|False False" "$m3|$m4"


# W15-b33: --check の probe は driver の形ごとに正しい dry-run を打つ（rehearse.sh は第 1 引数が <n>・n 無しの --dry-run は exit 2）。本物の rehearse.sh で
printf 'rh\t%s\tbash rehearse.sh 1 %s\nnodrv\t%s\tbash nothing-here.sh %s\n' "$BG" "$T/qw/X.app" "$BG" "$T/qw/X.app" > "$T/qw/chk-rh.tsv"
out=$(bash "$QW" --check "$T/qw/chk-rh.tsv"); rc=$?
check "quiet-window --check: rehearse.sh 1 <app> -> dry-run ok (probe is '1 --dry-run'); a row with no driver file -> driver ng, exit 1" "1 1 1" "$rc $(grep -c '^| rh | ok | ok | ok | ok |' <<< "$out") $(grep -c '^| nodrv | ok | ok | ng |' <<< "$out")"
# 負の対照: <n> の枝を外した probe（旧: --dry-run だけ）だと rehearse の行が ng(2)
sed 's/^        if \[\[ "\$nx" =~ .*$/        if false; then :/' "$QW" > "$BG/qw-mut5.$$.sh"
out=$(bash "$BG/qw-mut5.$$.sh" --check "$T/qw/chk-rh.tsv"); rm -f "$BG/qw-mut5.$$.sh"
check "quiet-window --check negative control: without the <n> branch the rehearse row is ng(2)" "1" "$(grep -c '^| rh | ok | ok | ok | ng(2) |' <<< "$out")"

# ---- integ-ui.sh strings / rows（W15-b35。一時 repo。strings は rev の木から作る・rows は .app の欄だけ替える）----
mkdir -p "$T/ius/src" "$T/ius-c" && ( cd "$T/ius" && git init -q -b main . && git config user.email t@t && git config user.name t \
  && printf 'fn f() { let a = String::from("名前を入力"); }\n' > src/generated.rs && git add src && git commit -qm c1 \
  && printf 'fn f() { let a = String::from("名前を入力"); let b = String::from("保存に失敗"); }\n' > src/generated.rs && git add src && git commit -qm c2 )
( cd "$T/ius" && git archive HEAD~1 -- src | tar -x -C "$T/ius-c" ) && python3 -I "$BG/../ui-strings.py" --root "$T/ius-c" --csv "$T/ius-copy.csv" >/dev/null
ius() { ( cd "$T/ius" && SC="$T/ius-copy.csv" bash "${1:-$IU}" strings "$2" 2>&1 ); }
check "integ-ui strings: the copy was made from HEAD~1 -> HEAD~1 gives diff=0 and HEAD gives diff>0" "0 1" "$(ius "$IU" HEAD~1 | sed -n 's/.*diff=//p') $(ius "$IU" HEAD | sed -n 's/.*diff=//p' | awk '{print ($1>0)}')"
sed 's/git archive "\$rev"/git archive HEAD/' "$IU" > "$BG/../iu-mut2.$$.sh"
got=$(ius "$BG/../iu-mut2.$$.sh" HEAD~1 | sed -n 's/.*diff=//p'); rm -f "$BG/../iu-mut2.$$.sh"
check "integ-ui strings negative control: ignoring <rev> (always HEAD) makes HEAD~1 differ from its own copy" "1" "$(awk -v g="$got" 'BEGIN{print (g>0)}')"
printf '# c Old.app\nn1\t/work/Old.app\tbash a.sh /old/Old.app --x\nn2\t/work\tbash b.sh /o/Other.app; echo src/app.rs\n' > "$T/rows.tsv"
rows() { bash "${1:-$IU}" rows /new/X.app "$T/rows.tsv" 2>&1; }
check "integ-ui rows: only the .app in the command column changes (comment, name, cwd, src/app.rs kept)" "# c Old.app|n1	/work/Old.app	bash a.sh /new/X.app --x|n2	/work	bash b.sh /new/X.app; echo src/app.rs" "$(rows | paste -sd'|' -)"
sed 's/\[2:\]/[1:]/g' "$IU" > "$BG/../iu-mut3.$$.sh"
got=$(rows "$BG/../iu-mut3.$$.sh" | grep -c $'^n1\t/new/X.app'); rm -f "$BG/../iu-mut3.$$.sh"
check "integ-ui rows negative control: also replacing the cwd column changes n1's cwd" "1" "$got"


# ---- 判定の側（w15_judge / tally / gfinal-tally / ui-state）が app から読む文も表の key で引く（W15-b36）----
JL="$BG/jp_literals.py"; JT="${JT:-$BG/tally.py}"
JUD="$BG/../../../docs/evidence"
jd() {  # jd <w15_judge.py> <tally.py>: JP の木 / EN の木（fixture）に同じ判定を当てる → "<save_field_ready> <saved_line> <connected 1 件> <SUCCESS_WORDS に当たる文の数>" を木ごとに
  python3 -I - "$1" "$2" "$LF" "$BG" <<'EOF8'
import importlib.util, json, sys
def load(n, p):
    sp = importlib.util.spec_from_file_location(n, p); m = importlib.util.module_from_spec(sp); sys.modules[n] = m; sp.loader.exec_module(m); return m
sys.path.insert(0, sys.argv[4])
J = load("w15_judge", sys.argv[1]); T = load("tally", sys.argv[2])
out = []
for n in ("jp", "en"):
    els = json.load(open(f"{sys.argv[3]}/tree-{n}.json"))["elements"]
    claimed = len(T.count_claimed({"elements": [{"role": e["role"], "label": e["label"]} for e in els]}))
    words = sum(1 for e in els if any(w in e["label"] for w in T.SUCCESS_WORDS))
    out.append(f"{J.save_field_ready(els)} {J.saved_line(els)} {claimed} {words}")
print("|".join(out))
EOF8
}
check "judge/tally: the same verdicts on the JP tree and the EN tree (save field, saved line, 1 connected claim, 3 success words)" "True True 1 3|True True 1 3" "$(jd "$JUD/W15/w15_judge.py" "$JT")"
# 負の対照: judge に JP の literal を戻すと EN の木で save_field_ready が false・tally の connected を JP だけに戻すと EN の木で 0 件
sed 's/L.hit(e, "save_repo", contains=True)/"repo に保存" in (e.get("label") or "")/' "$JUD/W15/w15_judge.py" > "$JUD/W15/jd-mut2.$$.py"
sed 's/L.names("connected"))/("接続済み",))/' "$JT" > "$BG/jd-mut3.$$.py"
g2=$(jd "$JUD/W15/jd-mut2.$$.py" "$JT"); g3=$(jd "$JUD/W15/w15_judge.py" "$BG/jd-mut3.$$.py"); rm -f "$JUD/W15/jd-mut2.$$.py" "$BG/jd-mut3.$$.py"
check "judge negative control: save_repo matched by the JP literal -> false on the EN tree; connected searched in JP only -> 0 claims on the EN tree" "True True 1 3|False True 1 3;True True 1 3|True True 0 3" "$g2;$g3"
# AST の検査: 判定の側 4 file に日本語の literal が allowlist 以外に 0（app の文は key）・JP の literal を 1 つ戻すと 1
js() { python3 -I "$JL" --cjk "$BG" "$@"; }
check "judge files: 0 Japanese literals outside the allowlist (judge, tally, gfinal-tally, ui-state)" "0" "$(js "$JUD/W15/w15_judge.py" "$BG/tally.py" "$BG/gfinal-tally.py" "$BG/oracle/ui-state.py")"
mkdir -p "$BG/jm.$$"  # allowlist は basename で引くので、壊した写しも同じ名前で別 dir に置く
sed 's/L.hit(e, "saved_line", contains=True)/"に置きました" in (e.get("label") or "")/' "$JUD/W15/w15_judge.py" > "$BG/jm.$$/w15_judge.py"
sed 's/L.names("finished")/("完了",)/' "$BG/tally.py" > "$BG/jm.$$/tally.py"
h1=$(js "$BG/jm.$$/w15_judge.py"); h2=$(js "$BG/jm.$$/tally.py"); rm -rf "$BG/jm.$$"
check "judge files negative control: a JP literal put back in w15_judge / in tally -> 1 / 1" "1 1" "$h1 $h2"

# ---- Lie.word は当たった語のうち一番長い物（W15-b37・「保存済み」の行が「済み」と数えられない）----
lw() { python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as T;print(",".join(l.word for l in T.find_lies({"role":"AXWindow","children":[{"role":"AXStaticText","label":x} for x in ("保存済み","接続済み","Connected")]},{},"s")))' "$BG"; }
check "tally Lie.word: the longest matching word (保存済み, 接続済み, Connected)" "保存済み,接続済み,Connected" "$(lw)"
mkdir -p "$BG/lm.$$"; cp "$BG/labels.py" "$BG/owner_idle.py" "$BG/lm.$$/"; sed 's/w = max((w for w in SUCCESS_WORDS if w in t), key=len, default=None)/w = next((w for w in SUCCESS_WORDS if w in t), None)/; s/("保存済み", "済み"/("済み"/' "$BG/tally.py" > "$BG/lm.$$/tally.py"
got=$(BG="$BG/lm.$$" python3 -I -c 'import sys;sys.path.insert(0,sys.argv[1]);import tally as T;print(T.find_lies({"role":"AXWindow","children":[{"role":"AXStaticText","label":"保存済み"}]},{},"s")[0].word)' "$BG/lm.$$"); rm -rf "$BG/lm.$$"
check "tally Lie.word negative control: first-match order + no 保存済み word -> 済み" "済み" "$got"

# ---- fg-checklist の文言の照合（rehearse.py --fg・W15-b37）----
FGM="$BG/../../../docs/demo/fg-checklist.md"
fgc() { python3 -I "$BG/rehearse.py" --fg "$1" >/dev/null 2>&1; echo $?; }
check "rehearse --fg: the real fg-checklist.md quotes are all in strings.csv" "0" "$(fgc "$FGM")"
{ cat "$FGM"; echo '| 後-99 | x | x | `この文言は表に無い` | x | x |'; } > "$T/fg-bad.md"
check "rehearse --fg negative control: a quote that is not in strings.csv -> 1" "1" "$(fgc "$T/fg-bad.md")"

# ---- 在席の信号は 1 か所（owner_idle.py・W15-b42）: 自分の type_text では戻らない・pointer は戻す・自分の post から 10 秒後の key は戻す ----
OI="$T/oi"; mkdir -p "$OI"; printf '{"tool":"get_window_state","t0":990,"t1":995}\n{"tool":"type_text","t0":1000,"t1":1008}\n' > "$OI/steps.jsonl"
oi() {  # oi <owner_idle.py> <JSON> <now> → 秒（steps.jsonl の type_text は t=1000〜1008）
  OWNER_IDLE_JSON="$2" OWNER_NOW="$3" python3 -I "$1" --steps "$OI/steps.jsonl"
}
oa() { awk -v s="$1" 'BEGIN { exit !(s + 0 < 120) }' && echo active || echo away; }  # start の門の閾値 120 秒
OIP="$BG/owner_idle.py"
check "owner_idle: right after our own type_text (hid 12 / key 12 / mouse 5000) -> away" "away" "$(oa "$(oi "$OIP" '{"hid_idle_s":12,"key_idle_s":12,"mouse_idle_s":5000}' 1020)")"
check "owner_idle: pointer moved 30 s ago -> active" "active" "$(oa "$(oi "$OIP" '{"hid_idle_s":30,"key_idle_s":12,"mouse_idle_s":30}' 1020)")"
check "owner_idle: a key 10 s after our post (outside +-2 s) -> active" "active" "$(oa "$(oi "$OIP" '{"hid_idle_s":10,"key_idle_s":10,"mouse_idle_s":5000}' 1100)")"
check "owner_idle: old axprobe json (hid only, 5 s) and no own post -> active (compat)" "active" "$(oa "$(oi "$OIP" '{"hid_idle_s":5}' 5000)")"
# 負の対照: 信号を HIDIdleTime だけに戻した写しでは最初の検査が赤くなる
python3 -I -c 'import sys;s=open(sys.argv[1]).read();i=s.index("    num = lambda");j=s.index("def read_signal");open(sys.argv[2],"w").write(s[:i]+"    return sig.get(\"hid_idle_s\")\n\n\n"+s[j:])' "$OIP" "$OI/owner_idle_hid.py"
check "owner_idle negative control: HIDIdleTime only -> our own type_text reads as active" "active" "$(oa "$(oi "$OI/owner_idle_hid.py" '{"hid_idle_s":12,"key_idle_s":12,"mouse_idle_s":5000}' 1020)")"
# 窓の開始の基準（QW_OPENED_AT）: idle 200 秒でも窓を開けて 300 秒経っていれば、窓の途中の入力 = 使用中。同じ idle で基準が無ければ不在
oqw() { ( source "$BG/run.sh"; [ -n "$1" ] && export QW_OPENED_AT=$(( $(date +%s) - $1 )); owner_active 200 && echo active || echo away ); }
check "owner_active: idle 200 s with the window opened 300 s ago -> active (input during the window)" "active" "$(oqw 300)"
check "owner_active: idle 200 s with the window opened 150 s ago -> away" "away" "$(oqw 150)"
check "owner_active: idle 200 s without QW_OPENED_AT -> away (threshold 120 s unchanged)" "away" "$(oqw '')"

qwd() { printf 'a\t.\techo hi\n' > "$T/oi/p.tsv"; QW_CARGO_N=0 QW_LOAD1=0 QW_IDLE_S=200 command env ${1:+QW_OPENED_AT=$(( $(date +%s) - $1 ))} bash "$BG/quiet-window.sh" --dry-run "$T/oi/p.tsv" | head -1 | grep -o 'no-go([a-z_]*:[0-9]*)\|go$'; }
check "quiet-window gate: idle 200 s but the window opened 300 s ago -> no-go(owner_since_open)" "no-go(owner_since_open:200)" "$(qwd 300)"
check "quiet-window gate: idle 200 s, window opened 150 s ago -> go" "go" "$(qwd 150)"

# ---- scripts/qa/README.md の道具の表: 書いた file と option が実在する（W15-b43）----
QAD="$(cd "$BG/.." && pwd)"; RDM="$QAD/README.md"
rdm_missing() {  # rdm_missing <README> → 表の第 1 列の file（`a` / `b` を含む）と、行の `<tool> --opt` の組で実在しない物を 1 語ずつ
  python3 -I - "$1" "$QAD" <<'PY'
import os, re, sys
rd, qa = sys.argv[1], sys.argv[2]; repo = os.path.dirname(os.path.dirname(qa))
def find(f):
    f = f.split()[0]
    for b in (qa, repo, os.path.join(qa, "bg")):
        if os.path.exists(os.path.join(b, f)): return os.path.join(b, f)
miss = []
for line in open(rd, encoding="utf-8"):
    if not line.startswith("| `") or line.startswith("| `bg/test.sh`"): continue
    first = line.split("|")[1]
    files = re.findall(r"`([^`]+)`", first)
    for f in files:
        f = re.sub(r"^(integ-ui\.sh) .*", r"\1", f); f = re.sub(r"^(ui-strings\.py) .*", r"\1", f); f = re.sub(r"^(bg/rehearse\.py) .*", r"\1", f)
        if not f.endswith((".sh", ".py", ".tsv", ".swift")): continue
        if not find(f): miss.append(f); continue
    # option は「その行の 打つ 1 行 欄に出る <tool> 以外の --opt」を第 1 列の file の source で探す
    cols = line.split("|"); cmd = cols[3] if len(cols) > 3 else ""
    f0 = files[0].split()[0] if files else ""
    t = find(f0) if f0.endswith((".sh", ".py")) else None
    if t:
        src = open(t, encoding="utf-8", errors="replace").read()
        for opt in set(re.findall(r"(?<![\w-])(--[a-z][a-z-]+)", first + cmd)):
            if opt not in src and opt not in ("--help",): miss.append(f0 + " " + opt)
        for sub in re.findall(r"integ-ui\.sh (strings|rows)\b", first):
            if sub not in src: miss.append(f0 + " " + sub)
print("\n".join(miss))
PY
}
check "qa/README.md: every file and option in the tool table exists" "" "$(rdm_missing "$RDM")"
check "qa/README.md: rows added in W15-b43 are in the table" "7" "$(grep -cE '^\| `(bg/labels\.py|bg/jp_literals\.py|bg/owner_idle\.py|integ-ui\.sh strings <rev>|integ-ui\.sh rows <app> \[plan\.tsv\]|ui-strings\.py --dup-jp|bg/rehearse\.py --fg)`' "$RDM")"
sed 's#`bg/owner_idle\.py`#`bg/owner_idle_gone.py`#; s#`ui-strings\.py --dup-jp`#`ui-strings.py --dup-gone`#; s#--dup-jp --map#--dup-gone --map#' "$RDM" > "$T/README.mut.md"
check "qa/README.md: a made-up file and a made-up option are red (negative control)" "bg/owner_idle_gone.py|ui-strings.py --dup-gone" "$(rdm_missing "$T/README.mut.md" | paste -sd'|' -)"

# ---- W15-b46: ⑥ の run（run.w19-3b-q511s8wz と -2・写し）の lies と guard_failures・app_sha256 ----
# fixture = scripts/qa/fixtures/lies-q511（run = 1 run 目・run-2 = 2 run 目。実名は ~・ax は成功の文の要素だけ・DB は下で作る）
LQ="$T/lq"; mkdir -p "$LQ"; cp -R "$BG/../fixtures/lies-q511/." "$LQ/"
python3 -I - "$LQ" <<'EOF9'
import json, sqlite3, sys
d = sys.argv[1]
for r in ("run", "run-2"):
    c = sqlite3.connect(f"{d}/{r}/data/kangae.sqlite3"); c.execute("create table projects(id text, json text, created_at text, updated_at text)")
    for p in json.load(open(d + "/projects.json")):
        c.execute("insert into projects values('p', ?, ?, ?)", (json.dumps({"resources": p["resources"]}), p["created_at"], p["created_at"]))
    c.commit()
EOF9
lq() {  # lq <tally.py> <run-dir> [args…] → "<lies の数> <guard_failures の数>"
  local tl=$1 rd=$2; shift 2
  python3 -I "$tl" "$rd" "$@" | python3 -I -c 'import json,sys;d=json.load(sys.stdin);print(len(d["lies"]),len(d["guard_failures"]))'
}
check "tally lies: the success sentences of run 1 and run 2 are true (backed before they appear) -> 0 0 (was 5 and 4 lies)" "0 0|0 0" "$(lq "$BG/tally.py" "$LQ/run")|$(lq "$BG/tally.py" "$LQ/run-2")"
# 負の対照: 裏（engine の verified の at / DB の created_at）を文より後にずらすと嘘になる（verified の 2 文 + repo の 1 文）・裏が無い run は 3 文 = 3 件（重ね数えの 5 ではない）
cp -R "$LQ" "$LQ.late"; python3 -I - "$LQ.late" <<'EOF9'
import json, sqlite3, sys
d = sys.argv[1]
f = f"{d}/run/data/engine-runs.jsonl"; recs = [json.loads(l) for l in open(f)]
open(f, "w").write("".join(json.dumps(dict(r, at="2026-10-25T16:00:00.000Z")) + "\n" for r in recs))
sqlite3.connect(f"{d}/run/data/kangae.sqlite3").execute("update projects set created_at='2026-10-25T16:00:00.000Z'").connection.commit()
EOF9
cp -R "$LQ" "$LQ.none"; rm -f "$LQ.none/run/data/engine-runs.jsonl" "$LQ.none/run/data/kangae.sqlite3"
sed 's/        if s not in seen:/        if True:/' "$BG/tally.py" > "$BG/lq-mut1.$$.py"
check "tally lies negative control: backing after the sentence -> 3 lies; no backing -> 3 lies (one per sentence); dedupe line removed -> more than 3" "3 0|3 0|y" "$(lq "$BG/tally.py" "$LQ.late/run")|$(lq "$BG/tally.py" "$LQ.none/run")|$(lq "$BG/lq-mut1.$$.py" "$LQ.none/run" | awk '{print ($1>3)?"y":"n"}')"
rm -f "$BG/lq-mut1.$$.py"
# ② 2 run 目の guard: 前の run（<out>-2 の名前から <out>）の type_text を自分の打鍵として除く。負の対照 = 前の run を渡さない（--prev-run を存在しない dir）と 8
check "tally guard: run 2 does not read the type_text of run 1 as the owner (default prev run, or --prev-run) -> 0; no prev run -> 8" "0|0|8" "$(lq "$BG/tally.py" "$LQ/run-2" | cut -d' ' -f2)|$(lq "$BG/tally.py" "$LQ/run-2" --prev-run "$LQ/run" | cut -d' ' -f2)|$(lq "$BG/tally.py" "$LQ/run-2" --prev-run "$LQ/nowhere" | cut -d' ' -f2)"
rm -rf "$LQ.late" "$LQ.none"
# ③ app_sha256: 同じ .app を 2 つの綴り（symlink 経由 / 実体）で渡して同じ sha。負の対照 = path を数える旧い式は違う sha
mkdir -p "$T/as/real/X.app/Contents/MacOS"; echo bin > "$T/as/real/X.app/Contents/MacOS/x"; ln -s real "$T/as/link"
old_sha() { find "$1/Contents/MacOS" -type f -exec shasum -a 256 {} + | sort -k2 | shasum -a 256 | cut -c1-64; }
check "run.sh app_sha: the same .app by two spellings (real path, symlink) gives one sha; the old path-counting formula gives two" "same|differ" "$([ "$(app_sha "$T/as/real/X.app")" = "$(app_sha "$T/as/link/X.app")" ] && echo same || echo differ)|$([ "$(old_sha "$T/as/real/X.app")" = "$(old_sha "$T/as/link/X.app")" ] && echo same || echo differ)"

# W15-b48: 4 回目の窓（2 時間 12 分・255 手中 254 手が rc=1）の原因 = step() が window_id を渡さず get_window_state が「Missing required integer field: window_id」。
# ① step() の args に run.json の window_id が入る（fixture = その run の run.json / steps の形・実名無し） ② 同じ原文の error が 3 手続いたら DriverDown（以降の手を打たない）。
b48() {  # b48 <rehearse.py>: 偽の run.sh 経由で get_window_state を 5 回打つ → "<window_id が入った呼びの数> <DriverDown が出た手 or none> <打った呼びの数>"
  python3 -I - "$1" "$T/b48" <<'EOF9'
import importlib.util, json, os, sys
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); spec.loader.exec_module(rh)
R = sys.argv[2]; os.makedirs(R + "/steps", exist_ok=True)
json.dump({"pid": 4242, "window_id": 1421, "session": "s-fixture"}, open(R + "/run.json", "w"))
calls = []
def fake(*a, ws=None):  # a = ("step", R, name, tool, json, ...)
    calls.append(json.loads(a[4]))
    open(f"{R}/steps/{a[2]}.cua.json", "w").write("Missing required integer field: window_id\n")
    return 2, ""
rh.run_sh = fake
C = rh.Ctx(R, "X.app", "n")
hit = "none"
for i in range(5):
    try:
        rh.step(C, "get_window_state", {"timeout_ms": 1})
    except rh.DriverDown:
        hit = i + 1
        break
print(sum(c.get("window_id") == 1421 for c in calls), hit, len(calls))
EOF9
}
rm -rf "$T/b48"
sed 's/"pid": j\["pid"\], "window_id": j\["window_id"\], /"pid": j["pid"], /' "$BG/rehearse.py" > "$BG/rh-b48a.$$.py"; sed 's/^SAME_ERR_MAX = 3$/SAME_ERR_MAX = 999/' "$BG/rehearse.py" > "$BG/rh-b48b.$$.py"
check "rehearse step(): every driver call carries run.json's window_id (1421); the same driver error 3 steps running -> DriverDown at step 3, 3 calls only" "3 3 3" "$(b48 "$BG/rehearse.py")"
check "rehearse step() negative controls: window_id line removed -> 0 calls carry it; threshold removed -> never stops (5 calls)" "0 3 3|5 none 5" "$(b48 "$BG/rh-b48a.$$.py")|$(b48 "$BG/rh-b48b.$$.py" | cut -d' ' -f1-3 | sed 's/^3 none 5/5 none 5/')"
rm -f "$BG/rh-b48a.$$.py" "$BG/rh-b48b.$$.py"
printf 'drv\t%s\techo "rehearse: driver down: Missing required integer field: window_id | x" >&2; exit 7\n' "$T/qw/cwd" > "$T/qw/rc7.tsv"
QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=0 bash "$QW" "$T/qw/rc7.tsv" --out "$T/qw/o7" >/dev/null 2>&1; rc=$?
check "quiet-window: command exit 7 (driver down) is 'unmeasured (driver: <original line>)', not fail; overall 6" "6 1 0" "$rc $(grep -c '| 7 | unmeasured (Missing required integer field: window_id ¦ x) |' "$T/qw/o7/summary.md") $(grep -c '| fail |' "$T/qw/o7/summary.md")"
sed 's/elif \[ \$rc -eq 7 \]; then/elif false; then/' "$BG/quiet-window.sh" > "$BG/qw-b48.$$.sh"
QW_CARGO_N=0 QW_IDLE_S=500 QW_LOAD1=0 bash "$BG/qw-b48.$$.sh" "$T/qw/rc7.tsv" --out "$T/qw/o7n" >/dev/null 2>&1; rm -f "$BG/qw-b48.$$.sh"
check "quiet-window exit 7 negative control: branch removed -> the row is 'fail'" "1" "$(grep -c '| 7 | fail |' "$T/qw/o7n/summary.md")"

# ---- W15-b49: rehearse の 4 手（b4・ask_ai ×2・e0・d2）の handler ----
# dry-run の末尾が「driver 無し 0」・b4 だけ「外へ効く」の印・NO_DRIVER に 1 つ戻すと「driver 無し 1」
b49dry() { python3 -I "$1" 1 --dry-run 2>/dev/null; }
b49_tail() { b49dry "$1" | tail -1 | grep -o 'driver 無し [0-9]*'; }
b49_ext() { b49dry "$1" | grep -c '^ *11\. BG b4 .*\[driver 在り（外へ効く: GFINAL_REALLY=1 の時だけ書く・Issue #1）\]$'; }
sed 's/^NO_DRIVER = {}$/NO_DRIVER = {"b4": "x"}/' "$BG/rehearse.py" > "$BG/rh-b49a.$$.py"
check "rehearse --dry-run: driver 無し 0, the b4 row says it acts outside (REALLY only); NO_DRIVER[b4] put back -> driver 無し 1" "driver 無し 0 1|driver 無し 1" "$(b49_tail "$BG/rehearse.py") $(b49_ext "$BG/rehearse.py")|$(b49_tail "$BG/rh-b49a.$$.py")"
rm -f "$BG/rh-b49a.$$.py"
# 偽の画面と偽の道具で 4 手の handler を回す（ok / red / unm）。ask_ai は R/data/gui-acts.jsonl の行と README の sha を読み戻す
b49() {  # b49 <rehearse.py> -> "<#20 ok> <#20 sha そのまま> <#20 fs 直書き> <#18 拒否> <#18 拒否なのに sha が変わる> <#18 lease 無し> <b4 の門が閉じている> <e0 ok> <e0 nonce が写らない> <e0 空> <e0 許可なし> <d2 ok> <d2 停止の 1 行なし> <d2 窓が残る> <d2 Connected> <owner の pid が生きている>"
  python3 -I - "$1" "$T/b49" "$BG" <<'EOF11'
import importlib.util, json, os, shutil, subprocess, sys, time
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
base = sys.argv[2]; shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
import types
clk = [time.time()]  # 仮想の時計: sleep が進める（待ちを速くし、「2 秒後」を決めた通りに作る）
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + s))
rows, _ = rh.parse(rh.DEMO); r18, r20 = rows[17], rows[19]
cnt = [0]
def mk(name):
    cnt[0] += 1; R = f"{base}/{name}-{cnt[0]}"; os.makedirs(R + "/data"); os.makedirs(R + "/shibubu-yc-demo")
    open(R + "/shibubu-yc-demo/README.md", "w").write("# old\n")
    json.dump({"pid": 1, "window_id": 2, "session": "s"}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); C.state["te_sets"] = []; C.state["ai_target"] = (9, 9); C.state["te_pid"] = 9
    return C
def out(fn, C, row=None):
    try:
        fn(C, row); return "ok"
    except rh.Unmeasured:
        return "unm"
    except RuntimeError:
        return "red"
def T(role, v=None, label=None, tok=None):
    e = {"role": role}
    if v: e["value"] = v
    if label: e["label"] = label
    if tok: e["element_token"] = tok
    return e
def ai(mode, row):
    CUR[0] = mode
    C = mk("ai-" + mode + str(row["n"])); st = {"sent": False, "ok": False}; readme = C.repo + "/README.md"
    def els(C_):
        e = [T("AXTextField", label="Change this project", tok="t-change"), T("AXButton", label="Send changes", tok="t-send_changes")]
        if mode == "stale20": e.append(T("AXStaticText", "Someone is using the app in front, so nothing was sent. Go back to Shibubu, then ask again"))  # B69: 手 7 の lease の拒否の行が chat に残ったまま手 9 を送る
        if st["sent"] and mode == "lease_blocked": e.append(T("AXStaticText", "2 windows are Blocked"))
        elif st["sent"] and mode == "lease_readfirst": e.append(T("AXStaticText", "Nothing was sent, because the original couldn't be read first: Input missing or wrong"))  # B74: lease の語（so nothing was sent）に当たらない製品の拒否
        elif st["sent"] and mode.startswith("lease"): e.append(T("AXStaticText", "Someone is using the app in front, so nothing was sent. Go back to Shibubu, then ask again"))
        if st["sent"]: st["n"] = st.get("n", 0) + 1  # 送った後の読みの回数
        if st["sent"] and not mode.startswith("lease") and (mode != "stale20" or st["n"] > 1):  # stale20: 承認の行は AI の待ちの後（2 回目の読み）に出る（cu.umDP 手 9 の Loading の間）
            e += [T("AXStaticText", "Approval needed before sending"), T("AXStaticText", "To do X, Shibubu will send 2 steps: Send “Shibubu Demo” to TextEdit"), T("AXButton", label="Approve and send", tok="t-approve"), T("AXButton", label="Cancel", tok="t-cancel")]
        if st["ok"]: e.append(T("AXStaticText", "5 windows of TextEdit have “README.md” open and Shibubu can't tell which. Close the others, then ask again" if mode.startswith("ambig") else
                                "Someone is using the app in front, so nothing was sent. Go back to Shibubu, then ask again" if mode == "appr_lease" else "Done. Checked 2 items in the original app"))  # B68: 承認の後に送る直前で前面を読んで拒む
        return e
    def acts(n): open(C.R + "/data/gui-acts.jsonl", "a").write('{"tool":"set_value"}\n{"tool":"menu"}\n' * n)
    def fstep(C_, tool, args, *extra, own=False):
        t = args.get("element_token")
        if t == "t-send_changes":
            SENT[0] = clk[0]  # b67: 一言を送った時刻（rehearse の t_send と同じ virtual clock）
            st["sent"] = True
            if mode == "lease_sha": open(readme, "w").write("changed\n")
            if mode == "lease_auto": open(readme, "w").write("x# old\n")  # B70: 手 6 で人が打った「x」を TextEdit が手 7 の途中で自動保存した（cu.ukZY）
            if mode == "lease_demo": open(readme, "w").write("# Shibubu Demo\n")  # B70 の負の対照: 拒否の行が出たのに頼んだ題が README に在る
            if mode == "lease_act": acts(1)  # TESTSH-NEG2: 拒否の行が出たのに act の記録が増えた（README は手の始めのまま）
            if mode == "lease_readfirst": open(C.R + "/data/engine-runs.jsonl", "a").write('{"error":{"kind":"denied","pending":[],"why":"Nothing was sent, because the original couldn\'t be read first: Input missing or wrong"}}\n')  # B74: 承認を頼む前に製品が止まった（cu.GkMp 手 7）
        if t == "t-cancel": st["sent"] = False
        if t == "t-approve":
            st["ok"] = True
            if mode in ("approve", "approve_old", "stale20"):
                open(readme, "w").write("Shibubu Demo"); acts(1); clk[0] += 5
                mt = SENT[0] - 100 if mode == "approve_old" else SENT[0] + 2  # b67: approve = 送った後・driver が気付く（clk + 5）より前に書かれた / approve_old = 送る前から在る古い mtime
                os.utime(readme, (mt, mt)); MT[mode] = mt
            if mode == "wrongtitle": open(readme, "w").write("other"); acts(1)  # b66: sha は変わるが頼んだ題が無い
            if mode == "nosha": acts(1)
            if mode == "fs": open(readme, "w").write("Shibubu Demo")
        return {"effect": "ok"}
    rh.elements = els; rh.step = fstep; rh.foreign_titled = lambda C_, t: {"ambig": 4, "ambig2": 3}.get(mode, 0)
    return out(rh.h_ask_ai, C, row)
rh.front_name = lambda: "TextEdit"
rh.te_value = lambda C_, pid, t: {"lease_auto": "x# old\n", "lease_demo": "# Shibubu Demo\n"}.get(CUR[0])  # B70: 人の TextEdit の中身（AX value）
BK = []; BK2 = []; CUR = [None]; SENT = [0.0]; MT = {}; rh.note_backing = lambda C_, k, at: (BK.append(k), BK2.append(f"{CUR[0]}:{'mtime' if at == MT.get(CUR[0]) else 'tchanged'}"))  # b66: #20 の裏（README の sha と中身）を note_backing に渡した kind
r = [ai("approve", r20), ai("nosha", r20), ai("fs", r20), ai("lease", r18), ai("lease_sha", r18), ai("approve", r18)]
ai("wrongtitle", r20); open(base + "/bk.txt", "w").write(",".join(BK))
ai("approve_old", r20); open(base + "/bk2.txt", "w").write(",".join(BK2))  # b67: 裏の時刻の出所（mtime / t_changed）
# b4: 門（REALLY と issue）が閉じていれば press まで進まない
reached = []; real_press = rh.press; rh.press = lambda *a, **k: reached.append(a); rh.elements = lambda C_: []
for k in ("GFINAL_REALLY", "SHIBUBU_QA_GH_ISSUE"): os.environ.pop(k, None)
C = mk("b4"); a = out(rh.h_b4, C); closed = not reached  # 門が閉じている間は press まで進まない
os.environ["GFINAL_REALLY"] = "1"; out(rh.h_b4, C); os.environ.pop("GFINAL_REALLY"); os.environ.pop("SHIBUBU_QA_GH_ISSUE", None)  # REALLY=1 なら Issue #1 の既定で press まで進む
r.append(int(a == "unm" and closed and bool(reached))); rh.press = real_press
# e0: 偽の ax（set / tree / ocr）/ screencapture（ocr は直前に ax set が書いた字を読む）
fb = base + "/fb"; os.makedirs(fb)
def w(name, body):
    open(f"{fb}/{name}", "w").write("#!/bin/bash\n" + body + "\n"); os.chmod(f"{fb}/{name}", 0o755)
w("screencapture", 'touch "${@: -1}"')
w("ax", 'd=$(dirname "$0"); c=$1; shift; case "$c" in set) echo "$3" > "$d/cur"; exit;; tree) echo \'{"window":{"x":0,"y":0},"elements":[{"identifier":"surface-frame:notes","frame":{"x":10,"y":20,"w":300,"h":200}}]}\'; exit;; esac; case "$(cat "$d/mode" 2>/dev/null)" in blank) echo "[]";; other) echo "[\\"sidebar\\"]";; late) f=${1##*/}; n=${f##*-}; n=${n%.png}; case "$f" in b*-*) [ "$n" -ge 12 ] && printf \'["%s"]\' "$(cat "$d/cur")" || echo "[\\"sidebar\\"]";; *) printf \'["%s"]\' "$(cat "$d/cur")";; esac;; *) printf \'["%s"]\' "$(cat "$d/cur")";; esac')
os.environ["PATH"] = fb + ":" + os.environ["PATH"]; rh.swift_tool = lambda C_, n: f"{fb}/{n}"
def e0(mode, els=(), tag=""):
    open(fb + "/mode", "w").write(mode); rh.elements = lambda C_: list(els)
    C = mk("e0-" + mode + tag); C.state["notes_pid"] = 4242; C.state["notes_sid"] = "notes"
    return out(rh.h_e0, C)
r += [e0("shown", [T("AXImage", label="mirror")]), e0("other", [T("AXImage", label="mirror")]), e0("blank", [T("AXImage", label="mirror")]), e0("shown", [T("AXImage", label="mirror"), T("AXStaticText", "Screen recording is off · Open settings")], "p")]
# d2: sleep の process を owner と自分で 1 つずつ。自分のだけ終わる
own, me = subprocess.Popen(["sleep", "60"]), subprocess.Popen(["sleep", "60"])
hd = base + "/hd"; os.makedirs(hd + "/bin"); open(hd + "/bin/windows", "w").write('#!/bin/bash\nd=$(dirname "$0"); [ -f "$d/out.json" ] && { cat "$d/out.json"; exit; }\nst=$(ps -o stat= -p "$3"); case "$st" in ""|Z*) echo \'{"windows":[]}\';; *) echo \'{"windows":[{"window_id":7}]}\';; esac\n'); os.chmod(hd + "/bin/windows", 0o755); rh.HERE = hd
def d2(els, wins=None, name="d2"):
    (open(hd + "/bin/out.json", "w").write(wins) if wins else (os.path.exists(hd + "/bin/out.json") and os.remove(hd + "/bin/out.json"))); rh.elements = els if callable(els) else (lambda C_: list(els))
    C = mk(name); C.state["te_sets"] = [({own.pid}, {own.pid, me.pid})]
    return out(rh.h_d2, C)
stop = T("AXImage", label="Not running · TextEdit")
r += [d2([stop], name="d2a"), d2([], name="d2b"), d2([stop], '{"windows":[{"window_id":7}]}', "d2c"), d2([stop, T("AXStaticText", "Connected")], name="d2d")]
# r8 の直し: Mirror の面を持つ Page（DB から・README は Diagram・notes.md は notes）の tab（AXCheckBox）を押して見る・Mirroring のままなら赤
D2D = {"surface_specs": [{"id": "docs", "backend": "mirror"}, {"id": "notes", "backend": "mirror"}, {"id": "diagram", "backend": "webview"}],
       "pages": [{"title": t, "root": {"children": [{"component": "source_surface", "props": {"surface_ref": s}}] if s else []}} for t, s in (("Docs", None), ("Diagram", "docs"), ("Tests", "diagram"), ("notes", "notes"))]}
on2, db0, press0 = [("", "")], rh.db_state, rh.press
rh.db_state = lambda C_: {"d": D2D}; rh.press = lambda C_, role, label, expect=None, **k: on2.__setitem__(0, (role, label if expect is None else "expect"))  # W15-b60: 今の Page の tab は木が変わらず dead_click になるので expect を付けない
tabbed = lambda fr: lambda C_: [T("AXImage", label=fr[on2[0][1]])] if on2[0][0] == "AXCheckBox" and on2[0][1] in fr else []  # 押した tab の Page の矩形だけを描く
r.append(d2(tabbed({"Diagram": "Not running · TextEdit", "notes": "Not running · TextEdit"}), name="d2e")); on2[0] = ("", "")
r.append(d2(tabbed({"Diagram": "Not running · TextEdit", "notes": "Mirroring · TextEdit"}), name="d2f")); on2[0] = ("", "")
seen3 = [0]
def slow(C_):  # 混雑: 読み 1 回 20 秒・notes の矩形は 3 回目の読みで Not running
    clk[0] += 20; t = on2[0][1] if on2[0][0] == "AXCheckBox" else ""
    if t == "notes":
        seen3[0] += 1; return [T("AXImage", label="Mirroring · TextEdit" if seen3[0] <= 2 else "Not running · TextEdit")]
    return [T("AXImage", label="Not running · TextEdit")] if t == "Diagram" else []
r.append(d2(slow, name="d2g")); rh.db_state, rh.press = db0, press0
me.kill(); me.wait(); r.append(int(own.poll() is None)); own.kill()
rh.front_name = lambda: "Finder"; r.append(ai("approve", r18)); rh.front_name = lambda: "TextEdit"  # 前面が TextEdit でなければ書き込まず未測
r.append(ai("lease_blocked", r18))
r.append(ai("appr_lease", r18))  # B68: #18 で承認の 1 行 → Approve → 製品が lease の 1 行で拒む（README も act も不変）= ok
r.append(ai("stale20", r20))  # B69: 前の手の lease の拒否の行が残ったまま #20 を送る → 送った後に増えた承認の行で進む = ok
r += [ai("lease_auto", r18), ai("lease_demo", r18)]  # B70: 人の字の自動保存で sha が変わっても製品の手が無ければ ok・頼んだ題が在れば赤
r.append(e0("late", [T("AXImage", label="mirror")], "l"))
r += [ai("ambig", r20), ai("ambig0", r20), ai("ambig2", r20)]  # demo-loop r3: 承認の後に「5 windows … can't tell which」= よその TextEdit に同じ題名 4 枚（自分 1）なら環境の未測・0 枚・3 枚（自分 2）なら赤
r.append(ai("lease_act", r18))  # TESTSH-NEG2: #18 の拒否の行が出たのに act の記録が 1 件増えた = 赤（B70 の act の見方だけが捕まえる）
r.append(ai("lease_readfirst", r18))  # B74: #18 で承認を頼む前に製品が止まった（pending 0 の denied）= 待たずに製品の赤
print(*r)
EOF11
}
rm -rf "$T/b49"
B49=$(b49 "$BG/rehearse.py")
BK67=$(cat "$T/b49/bk2.txt")
BK66=$(cat "$T/b49/bk.txt")  # b66: この 1 回目の b49 が #20 の裏として note_backing に渡した kind（後の変異の b49 が上書きする前に）
check "rehearse 4 hands (fake screen + fake tools): #20 ok / sha unchanged red / AI wrote fs directly red; #18 refusal ok / refusal but sha changed red / approval line, Approve and the product sends (target not in front) red (B68); b4 gate closed; e0 ok / no nonce red / blank unm / permission off unm; d2 ok / no stop line unm / window left red / Connected red / Mirror pages from the DB pressed as AXCheckBox tabs without --expect ok / notes still Mirroring red / 20 s reads: Not running on the 3rd read ok; owner's pid alive; #18 approval line -> Approve -> lease refusal, README and acts unchanged ok (B68); #20 with #18's lease refusal still in the chat reads only the rows added after its send -> ok (B69); #18 the human's typed char autosaved into README during the hand (sha changed, no act, no Shibubu Demo, README = the TextEdit value) ok / refusal but Shibubu Demo in README red (B70); #18 refusal but an act recorded red (B70's act view); #18 the product stopped before asking approval (pending 0 denied, 'couldn't be read first') red at once (B74); #20 refused as ambiguous: 5 windows = own 1 + foreign 4 -> unm, foreign 0 -> red, own 2 + foreign 3 -> red" "ok red red ok red red 1 ok red unm unm ok unm red red ok red ok 1 unm red ok ok ok red red unm red red red red" "$B49"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="            press(C, \"AXCheckBox\", tab)  #";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"            try_press(C, \"AXButton\", tab)  #"))' "$BG/rehearse.py" "$BG/rh-d2.$$.py"; rm -rf "$T/b49"; BD2=$(b49 "$BG/rh-d2.$$.py"); rm -f "$BG/rh-d2.$$.py"; rm -rf "$T/b49"
check "negative control: d2 pressing the tabs as AXButton (r8 and before) never reaches the Mirror pages -> both DB cases unmeasured" "unm unm" "$(echo "$BD2" | awk '{print $16, $17}')"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="(time.time() < end or n < 3)";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"time.time() < end"))' "$BG/rehearse.py" "$BG/rh-d3.$$.py"; rm -rf "$T/b49"; BD3=$(b49 "$BG/rh-d3.$$.py"); rm -f "$BG/rh-d3.$$.py"; rm -rf "$T/b49"
check "negative control: d2 deciding by the 30 s clock alone (2 reads of 20 s) reads the slow case red" "red" "$(echo "$BD3" | awk '{print $18}')"
python3 -I -c 'import re,sys;t=open(sys.argv[1],encoding="utf-8").read();u=re.sub(r"    if hit == \"can.t tell which\" and .*\n        raise Unmeasured\(f\"環境: .*\n","",t);assert u!=t;open(sys.argv[2],"w",encoding="utf-8").write(u)' "$BG/rehearse.py" "$BG/rh-amb.$$.py"; rm -rf "$T/b49"; BAM=$(b49 "$BG/rh-amb.$$.py"); rm -f "$BG/rh-amb.$$.py"; rm -rf "$T/b49"
check "negative control: without the environment branch the ambiguous refusal with foreign same-title windows is red" "red red red" "$(echo "$BAM" | awk '{print $(NF-2), $(NF-1), $NF}')"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="            press(C, \"AXButton\", \"approve\")\n            hit, line = wait_seen(C, (nothing, L.names(\"done_checked\")[0]), LEASE_WAIT_S, base=rows0)\n";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"            try_press(C, \"AXButton\", \"cancel\")\n            raise Unmeasured(\"人の lease が無く承認の 1 行が出た\")\n"))' "$BG/rehearse.py" "$BG/rh-b68.$$.py"; rm -rf "$T/b49"; B68=$(b49 "$BG/rh-b68.$$.py"); rm -f "$BG/rh-b68.$$.py"; rm -rf "$T/b49"
check "negative control (B68): the pre-B68 #18 (approval line read as no human lease -> Cancel) leaves both the product-sends case and the lease-refusal-after-Approve case unmeasured" "unm unm" "$(echo "$B68" | awk '{print $6, $22}')"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="    if base is None:\n        return els\n";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"    return els\n"))' "$BG/rehearse.py" "$BG/rh-b69.$$.py"; rm -rf "$T/b49"; B69=$(b49 "$BG/rh-b69.$$.py"); rm -f "$BG/rh-b69.$$.py"; rm -rf "$T/b49"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="        if product_sent() or not human_only():\n";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"        if n_acts(C) != acts0 or sha_of(readme) != sha0:\n"))' "$BG/rehearse.py" "$BG/rh-b70.$$.py"; rm -rf "$T/b49"; B70=$(b49 "$BG/rh-b70.$$.py"); rm -f "$BG/rh-b70.$$.py"; rm -rf "$T/b49"
check "negative control (B70): the pre-B70 sha comparison reads the human's autosaved char as the product's change -> red (cu.ukZY hand 7); Shibubu Demo in README stays red" "red red" "$(echo "$B70" | awk '{print $24, $25}')"
check "negative control (B69): reading every row (the pre-B69 waits) takes #18's leftover lease refusal as #20's answer -> unmeasured (cu.umDP hand 9); the other cases do not move" "unm|none" "$(echo "$B69" | awk '{print $23}')|$(python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i] and i!=22) or "none")' "$B49" "$B69")"
# demo-loop r4: notes.md は起動の前に開く（git status に出さない）・開く前の前面（owner の app）に返す（前が TextEdit・同じ pid なら何もしない）
rf() {  # rf <rehearse.py> -> "<exclude に notes が在る> <notes の中身> <返した open -b の bundle の列（Orca 前 / TextEdit 前 / 戻っている）>"
  python3 -I - "$1" "$T" <<'EOFRF'
import importlib.util, os, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
R = os.path.join(sys.argv[2], f"rf{os.getpid()}"); os.makedirs(R + "/shibubu-yc-demo/.git")
C = m.Ctx(R, "X.app", "n1"); C.state["te_sets"] = []
m.open_textedit = lambda p: ({1}, {1, 2})
m.open_notes(C)
ex = "docs/notes.md" in open(R + "/shibubu-yc-demo/.git/info/exclude").read()
calls = []; real = m.subprocess.run
m.subprocess = types.SimpleNamespace(run=lambda a, **k: calls.append(a[2]) if a[:2] == ["open", "-b"] else real(a, **k))
m.time = types.SimpleNamespace(sleep=lambda s: None, time=m.time.time)
m.front_app = lambda: {"bundle_id": "com.apple.TextEdit", "pid": 2}
m.restore_front({"bundle_id": "com.stablyai.orca", "pid": 478}); m.restore_front({"bundle_id": "com.apple.TextEdit", "pid": 9})
m.front_app = lambda: {"bundle_id": "com.stablyai.orca", "pid": 478}; m.restore_front({"bundle_id": "com.stablyai.orca", "pid": 478})
print(ex, open(R + "/shibubu-yc-demo/docs/notes.md").read().strip(), C.state["notes_pid"], ",".join(calls) or "-")
EOFRF
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();u=t.replace("    subprocess.run([\"open\", \"-b\", b], capture_output=True)\n","",1);assert u!=t;open(sys.argv[2],"w",encoding="utf-8").write(u)' "$BG/rehearse.py" "$BG/rh-rf.$$.py"
check "rehearse opens notes.md before launch hidden from git status and gives the front back to the owner's app (not when it was TextEdit or already back); negative control: no open -b -> nothing given back" "True notes 2 com.stablyai.orca|-" "$(rf "$BG/rehearse.py")|$(rf "$BG/rh-rf.$$.py" | awk '{print $NF}')"; rm -f "$BG/rh-rf.$$.py"
# ---- r5 の直し: #22 は notes の tab を押して矩形の名前で決める（「Mirror here」を押さない）・swift_tool は ax/ の dir と名前が重ならない ----
mc() {  # mc <rehearse.py> -> "<ok> <映らない> <Mirror here がまた出た> <同意が増えた> <Docs が止まった> <notes の tab なし> <swift_tool の出力が file>"
  python3 -I - "$1" "$T" <<'EOFMC'
import importlib.util, os, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
clock = [0.0]
m.time = types.SimpleNamespace(time=lambda: clock.__setitem__(0, clock[0] + 10) or clock[0], sleep=lambda s: None)
LIVE = "Mirroring · TextEdit"
def run(notes=LIVE, docs=LIVE, here=False, n=1, tabs=("notes", "Docs")):
    R = os.path.join(sys.argv[2], f"mc{os.getpid()}-{clock[0]}"); os.makedirs(R, exist_ok=True)
    C = m.Ctx(R, "X.app", "n1"); C.state["consents"] = 1
    on = ["?"]
    els = [{"role": "AXCheckBox", "label": t, "element_token": "t-" + t.lower()} for t in tabs] + ([{"role": "AXButton", "label": "Mirror here", "element_token": "t-h"}] if here else [])
    m.elements = lambda C_: els
    m.step = lambda C_, tool, args, *a: (on.__setitem__(0, args["element_token"]) or {}) if tool == "click" else {}
    m.frames = lambda C_: {"t-notes": {"notes": notes}, "t-docs": {"docs": docs}}.get(on[0], {})
    m.consents = lambda C_: n
    m.db_state = lambda C_: {"d": {"surface_specs": [{"id": "docs", "backend": "mirror", "target": {"file": "README.md"}}, {"id": "notes", "backend": "mirror", "target": {"file": "docs/notes.md"}}],
        "pages": [{"title": t, "root": {"children": [{"component": "source_surface", "props": {"surface_ref": s}}]}} for t, s in (("Docs", "docs"), ("notes", "notes"))]}}  # b71: 面のある Page は DB から種類で引く
    try:
        m.h_mirror_consent(C, {}); return "ok" if C.state.get("notes_sid") == "notes" and on[0] == "t-notes" else "bad"
    except m.Unmeasured:
        return "unm"
    except RuntimeError:
        return "red"
r = [run(), run(notes="Attaching · Looking for the original window"), run(here=True), run(n=2), run(docs="Not running · TextEdit"), run(tabs=("Docs",))]
R = os.path.join(sys.argv[2], f"mcs{os.getpid()}"); os.makedirs(R + "/ax", exist_ok=True)
C = m.Ctx(R, "X.app", "n1")
m.subprocess = types.SimpleNamespace(run=lambda a, **k: open(a[a.index("-o") + 1], "w").close() or types.SimpleNamespace(returncode=0, stderr=""))
p = m.swift_tool(C, "ax")
print(*r, os.path.isfile(p))
EOFMC
}
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOFMCM'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/rh-mc{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(*pairs):
    t = s
    for old, new in pairs:
        assert t.count(old) == 1, old
        t = t.replace(old, new)
    return t
out("a", mut(('    if not name.startswith(live):\n', '    if False:\n')))  # 映らない notes を ok にする
out("b", mut(('    if not dname.startswith(live):\n', '    if False:\n')))  # Docs の Mirror が止まっても見ない
out("c", mut(('out = os.path.join(C.R, "tools", name)', 'out = os.path.join(C.R, name)'), ('if not os.path.isfile(out):', 'if not os.path.exists(out):')))  # r5 の swift_tool
EOFMCM
check "rehearse #22 presses the notes tab and judges by the Mirror frame name: ok / not mirroring red / 'Mirror here' again red / consent count changed red / Docs stopped red / no notes tab unm; swift_tool builds beside ax/ not onto it; negative controls: no name check -> ok, no Docs check -> ok, r5 swift_tool -> dir" "ok red red red red unm True|ok|ok|False" "$(mc "$BG/rehearse.py")|$(mc "$BG/rh-mca.$$.py" | cut -d' ' -f2)|$(mc "$BG/rh-mcb.$$.py" | cut -d' ' -f5)|$(mc "$BG/rh-mcc.$$.py" | cut -d' ' -f7)"
rm -f "$BG"/rh-mc?.$$.py
# ---- REHEARSE_KEEP_GOING: 手の赤の後も次の手を測る（D-g の violation と driver down は止まる）----
kg() {  # kg <rehearse.py> -> "<既定で測った手の数> <KEEP で測った数> <KEEP で D-g の赤の後の数> <KEEP の rc>"
  python3 -I - "$1" "$T" <<'EOFKG'
import contextlib, importlib.util, io, json, os, sys
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
def go(second, keep):
    R = os.path.join(sys.argv[2], f"kg{os.getpid()}-{second[:3]}-{keep}"); os.makedirs(R + "/steps", exist_ok=True)
    json.dump({"window_id": 1}, open(R + "/run.json", "w"))
    C = m.Ctx(R, "X.app", "n1")
    m.run_sh = lambda *a, **k: (0, "")
    m.tally_all = lambda C_: {}
    def bad(C_, r):
        raise RuntimeError(second)
    m.HANDLERS.update(kg_ok=lambda C_, r: None, kg_bad=bad)
    rows = [dict(n=i + 1, who="BG", key=k, sec=1, doing="") for i, k in enumerate(["kg_ok", "kg_bad", "kg_ok", "kg_ok"])]
    os.environ.pop("REHEARSE_KEEP_GOING", None)
    if keep:
        os.environ["REHEARSE_KEEP_GOING"] = "1"
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):  # _live の表を出さない
        rc = m._live(rows, "1", "X.app", False, 1, "n1", C)
    return len(json.load(open(R + "/rehearse.json"))["steps"]), rc
print(go("F1: x", False)[0], *go("F1: x", True), go("D-g の violation: 9-click", True)[0])
EOFKG
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();u=t.replace("or st.startswith(\"赤: D-g\")", "", 1);assert u!=t;open(sys.argv[2],"w",encoding="utf-8").write(u)' "$BG/rehearse.py" "$BG/rh-kg.$$.py"
check "rehearse REHEARSE_KEEP_GOING: default stops at the first red (2 hands); keep-going measures all 4 and still returns 1; a D-g red still stops; negative control: without the D-g exception it keeps going past D-g" "2 4 1 2|4" "$(kg "$BG/rehearse.py")|$(kg "$BG/rh-kg.$$.py" | awk '{print $NF}')"; rm -f "$BG/rh-kg.$$.py"
# ---- r7 の #10: Mermaid の dialog は click の直後の木に無くても、時計で待って次の読みで見る ----
md() {  # md <rehearse.py> -> "<1 回目の読みで無く 2 回目で在る> <ずっと無い>"
  python3 -I - "$1" "$T" "$BG" <<'EOFMD'
import importlib.util, os, sys, types
sys.path.insert(0, sys.argv[3])  # 壊した写しは $T に置く（repo の dir に残さない）・labels / tally は本物の dir から
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
clock = [0.0]
m.time = types.SimpleNamespace(time=lambda: clock.__setitem__(0, clock[0] + 4) or clock[0], sleep=lambda s: None)
m.step = lambda *a, **k: {}
m.elements = lambda C_: []
def run(opens):
    seq = iter(opens)
    sys.modules["w15_judge"] = types.SimpleNamespace(mermaid_dialog_open=lambda els: next(seq, opens[-1]))
    def find(C_, role, label, tries=20, contains=False):
        if role == "AXTextArea":
            raise m.Unmeasured("past-dialog")
        return {"element_token": "t"}
    m.find = find
    R = os.path.join(sys.argv[2], f"md{os.getpid()}-{len(opens)}"); os.makedirs(R, exist_ok=True)
    try:
        m.b1_flow(m.Ctx(R, "X.app", "n1"), "n", ["n"], "B1")
    except m.Unmeasured as e:
        return "past" if str(e) == "past-dialog" else "unm"
    return "?"
print(run([False, True]), run([False]))
EOFMD
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();u=t.replace("        while not J.mermaid_dialog_open(elements(C)):\n            if time.time() >= end:", "        if not J.mermaid_dialog_open(elements(C)):\n            if True:", 1);assert u!=t;open(sys.argv[2],"w",encoding="utf-8").write(u)' "$BG/rehearse.py" "$T/rh-md.$$.py"
check "rehearse #10 waits for the Mermaid dialog by the clock (r7: not in the tree right after the click, open on the next read); never opens -> unm; negative control: decide on the first read -> unm" "past unm|unm" "$(md "$BG/rehearse.py")|$(md "$T/rh-md.$$.py" | cut -d' ' -f1)"
# ---- r8 の #10: Insert の後に図の選択を escape で外し（AXWebArea を待つ）、Excalidraw の主 menu = 名前の無い AXPopUpButton を押す（Stroke を押さない） ----
xm() {  # xm <rehearse.py> -> "<web・Stroke・menu> <menu 無し> <web 無し>"（各 = 最後に押した物[+esc]:止まった理由）
  python3 -I - "$1" "$T" "$BG" <<'EOFXM'
import importlib.util, os, sys, types
sys.path.insert(0, sys.argv[3])
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
m.time = types.SimpleNamespace(time=lambda: 0.0, sleep=lambda s: None)
sys.modules["w15_judge"] = types.SimpleNamespace(mermaid_dialog_open=lambda els: True)
E = lambda role, label, tok: {"role": role, "label": label, "element_token": tok} if label else {"role": role, "element_token": tok}
BASE = [E("AXButton", "More tools", "t-more"), E("AXButton", "Mermaid to Excalidraw", "t-mer"), E("AXTextArea", None, "t-area"), E("AXButton", "Insert", "t-insert")]
def run(*extra, sel=False):  # sel = 開いた時に前の run の図が選ばれている（r9: escape するまで More tools が無い）
    clicks, why = [], ""
    m.elements = lambda C_: [e for e in BASE if not (sel and e.get("label") == "More tools" and not any(t == "press_key" for t, _ in clicks))] + list(extra)
    m.step = lambda C_, tool, args, *a, **k: clicks.append((tool, args.get("element_token"))) or {}
    R = os.path.join(sys.argv[2], f"xm{os.getpid()}-{len(extra)}"); os.makedirs(R, exist_ok=True)
    try:
        m.b1_flow(m.Ctx(R, "X.app", "n1"), "n", ["n"], "B1")
    except m.Unmeasured as e:
        why = str(e)
    last = ([t for tool, t in clicks if tool == "click"] or ["-"])[-1]
    return last + ("+esc" if ("press_key", "t-web") in clicks else "") + ":" + ("save" if "Save to" in why else "more" if "More tools" in why else "menu" if "menu" in why else "web" if "AXWebArea" in why else "?")
web, stroke, menu = E("AXWebArea", None, "t-web"), E("AXPopUpButton", "Stroke", "t-stroke"), E("AXPopUpButton", None, "t-menu")
print(run(web, stroke, menu), run(web, stroke), run(stroke, menu), run(web, menu, sel=True))
EOFXM
}
python3 -I - "$BG/rehearse.py" "$T/rh-xm.$$.py" <<'EOFXMM'
import sys
t = open(sys.argv[1], encoding="utf-8").read()
new = """        deselect(C)  # r8: Insert の直後の木に AXWebArea が無く escape を飛ばし、置いた図の Stroke を主 menu と取り違えた
        step(C, "click", {"element_token": excalidraw_menu(C)["element_token"]})
"""
old = """        web = next((e for e in elements(C) if e.get("role") == "AXWebArea"), None)
        if web:
            step(C, "press_key", {"key": "escape", "element_token": web["element_token"]})
        step(C, "click", {"element_token": find(C, "AXPopUpButton", "", tries=8, contains=True)["element_token"]})
"""
assert t.count(new) == 1
open(sys.argv[2], "w", encoding="utf-8").write(t.replace(new, old))
EOFXMM
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="        deselect(C)  # r9:";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"        pass  # r9:"))' "$BG/rehearse.py" "$T/rh-xm2.$$.py"
check "rehearse #10: escape on the web area first (r9: a drawing left selected hides More tools) and after Insert, then the unnamed AXPopUpButton (Excalidraw's menu), never the selection's Stroke; no unnamed popup -> unm; no web area -> unm before any click; negative controls: the r8 code presses Stroke; without the first escape the selected case never finds More tools" "t-menu+esc:save t-insert+esc:menu -:web t-menu+esc:save|t-stroke+esc:save t-stroke+esc:save -:web|-:more" "$(xm "$BG/rehearse.py")|$(xm "$T/rh-xm.$$.py" | cut -d' ' -f1-3)|$(xm "$T/rh-xm2.$$.py" | cut -d' ' -f4 | sed 's/+esc//')"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF12'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/rh-b49{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
out("b", mut('    if sha_of(readme) == sha0:\n        raise RuntimeError("F1', '    if False:\n        raise RuntimeError("F1'))  # #20 の sha の検査を外す
out("c", mut('        if product_sent() or not human_only():', '        if product_sent():'))  # #18 の読み戻しから「README は手の始めか人の TextEdit の中身」の見方を外す（B70）
out("j", mut('        return n_acts(C) != acts0 or "Shibubu Demo" in open(readme, encoding="utf-8").read()', '        return n_acts(C) != acts0'))  # 「頼んだ題が README に在る」の見方を外す（B70）
out("k", mut('        return n_acts(C) != acts0 or "Shibubu Demo" in open(readme, encoding="utf-8").read()', '        return "Shibubu Demo" in open(readme, encoding="utf-8").read()'))  # 「act の記録が増えた」の見方を外す（B70）
out("d", mut('return sorted(set(after) - set(before))', 'return sorted(set(after))'))  # owner の pid も数える
out("e", mut('    if os.environ.get("GFINAL_REALLY") != "1":', '    if False:'))  # b4 の門を外す
out("f", mut('    if i is None or not any(saved(x) for x in new[i + 1:]):', '    if False:'))  # act の順（set_value → 保存）の検査を外す
out("g", mut('        t = next((t for e in els for t in (e.get("label"), e.get("value")) if t and re.search(r"\\d+ windows", t)), None)', '        t = None'))  # 「N windows…」の拒否を赤にする検査を外す
out("h", mut('        if got and got[0] <= 2.0:', '        if got:'))  # 2 秒の上限を外す
out("i", mut('            os.kill(pid, 15)', '            pass'))  # 自分の TextEdit を終了しない
EOF12
check "rehearse 4 hands negative controls: no F1 sha check -> #20 sha-unchanged still red (b72's line-difference check also catches it); no F2 'README = the hand's start or the human's TextEdit value' view (B70) -> #18 sha-changed ok; mine() = all pids -> owner dies; no b4 gate -> reached; no act-order check -> fs-write ok; no 'N windows' check -> unm; no 2 s limit -> late nonce2 ok; no kill -> d2 red" "red|ok|0|0|ok|unm|ok|red" "$(b49 "$BG/rh-b49b.$$.py" | cut -d' ' -f2)|$(b49 "$BG/rh-b49c.$$.py" | cut -d' ' -f5)|$(b49 "$BG/rh-b49d.$$.py" | cut -d' ' -f19)|$(b49 "$BG/rh-b49e.$$.py" | cut -d' ' -f7)|$(b49 "$BG/rh-b49f.$$.py" | cut -d' ' -f3)|$(b49 "$BG/rh-b49g.$$.py" | cut -d' ' -f21)|$(b49 "$BG/rh-b49h.$$.py" | cut -d' ' -f22)|$(b49 "$BG/rh-b49i.$$.py" | cut -d' ' -f12)"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="        hit, line = wait_approval(C, int(60 * llm_scale()), (nothing,), stop=lambda els: product_stop(C, runs0, els), base=rows0)\n        if hit == \"product_stopped\":  # B74";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"        hit, line = wait_approval(C, int(60 * llm_scale()), (nothing,), base=rows0)\n        if hit == \"product_stopped\":  # B74"))' "$BG/rehearse.py" "$BG/rh-b74.$$.py"
check "negative control (B74): the #18 lease wait without product_stop -> the product's 'couldn't be read first' refusal is read as neither line -> unm (cu.GkMp hand 7, 161.6 s)" "unm" "$(b49 "$BG/rh-b74.$$.py" | cut -d' ' -f31)"
rm -f "$BG/rh-b74.$$.py"
check "negative control (B70's other two views): no 'Shibubu Demo in README' view -> #18 refusal with the asked title in README ok; no act-count view -> #18 refusal with an act recorded ok" \
  "ok ok" "$(b49 "$BG/rh-b49j.$$.py" | cut -d' ' -f25) $(b49 "$BG/rh-b49k.$$.py" | cut -d' ' -f30)"
rm -f "$BG"/rh-b49?.$$.py
# ---- W15-b50: rehearse の途中切れ・再起動・owner の戻り・helper の無い .app（偽の cua.json / 偽の restart / 偽の .app・実機を起こさない） ----
B50="$T/b50"; rm -rf "$B50"; mkdir -p "$B50/runs/demo1.A" "$B50/app/Contents/MacOS" "$B50/app/Contents/Helpers"
printf '{"tool":"type_text","t0":1000,"t1":1008}\n' > "$B50/runs/demo1.A/steps.jsonl"
b50() {  # b50 <rehearse.py> -> "<送り直し 1 回で ok の type_text の数> <2 回とも partial の手> <2 回とも partial の type_text の数> <h_b1 が途中で落ちた後の depth> <rc 6 の restart> <window_id null の restart> <正常な restart> <demo* の兄弟の steps を除いた idle>"
  python3 -I - "$1" "$B50" "$BG" <<'EOF13'
import importlib.util, json, os, shutil, sys
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
base = sys.argv[2]
def send(effects):  # type_text の結果の列（partial / ok）を順に返す偽の cua.json
    calls = []
    rh.press = lambda C, role, label, expect=None, **k: None
    rh.find = lambda C, role, label, tries=20, contains=False: {"element_token": "t"}
    def fake(C, tool, args, *extra, own=False):
        if tool == "type_text":
            calls.append(1)
            return {"effect": effects[min(len(calls), len(effects)) - 1]}
        return {}
    rh.step = fake
    try:
        rh.send_field(rh.Ctx(base, "X.app", "n"), "AXTextField", "change", "hello")
        return "ok", len(calls)
    except rh.Unmeasured:
        return "unmeasured", len(calls)
a = send(["partial", "ok"]); b = send(["partial", "partial"])
def b1():
    C = rh.Ctx(base, "X.app", "n"); rh.find = lambda *x, **k: (_ for _ in ()).throw(rh.Unmeasured("x")); rh.step = lambda *x, **k: {}
    try:
        rh.h_b1(C, {})
    except rh.Unmeasured:
        pass
    return C.depth
d = b1()
def restart(rc, wid, active=False):
    R1 = f"{base}/rs{rc}{wid}{active}"; shutil.rmtree(R1 + "-restart", ignore_errors=True)
    for x in ("data", "shibubu-yc-demo", "fake"): os.makedirs(f"{R1}/{x}", exist_ok=True)
    def fake_run(*a, ws=None, fake=None):
        if a[0] == "start":
            json.dump({"window_id": wid, "owner_active": active}, open(a[1] + "/run.json", "w"))
        return rc, "owner busy"
    rh.run_sh = fake_run; rh.b3_check = lambda C: None; rh.check_d1_restart = lambda C, w0: None; rh.press = lambda *a, **k: None; rh.db_state = lambda C: {"d": {"name": "Dev environment"}}  # 入口・B3・D1 は b54 の test が見る
    C = rh.Ctx(R1, "X.app", "n")
    try:
        rh.h_restart(C, {})
        return "ok"
    except rh.DriverDown:
        return "down"
os.environ["OWNER_IDLE_JSON"] = '{"hid_idle_s":12,"key_idle_s":12,"mouse_idle_s":5000}'; os.environ["OWNER_NOW"] = "1020"
idle = rh.owner_idle_s(base + "/runs")
print(a[0], a[1], b[0], b[1], d, restart(6, 7, True), restart(0, None), restart(0, 7), int(idle), restart(6, 7))  # 最後 = r8: rc 6 でも app が居れば（00-launch の判定の未測）続ける
EOF13
}
b50_gate() {  # b50_gate <rehearse.py> <helper の有無 yes|no> <idle JSON> -> rehearse.py の rc（偽の .app・fresh-repo の前に止まる）
  rm -f "$B50/app/Contents/Helpers/kangae-mirror-helper"; [ "$2" = yes ] && : > "$B50/app/Contents/Helpers/kangae-mirror-helper"
  OWNER_IDLE_JSON="$3" python3 -I "$1" 1 "$B50/app" --out "$B50/runs/demo1.B" >/dev/null 2>&1 </dev/null; echo $?
}
B50W=$(b50 "$BG/rehearse.py")
check "rehearse b50: partial then ok -> one resend (2 calls, ok); partial twice -> unmeasured after 2 calls; h_b1 falling midway restores depth 8; restart rc 6 with owner_active / null window_id -> driver down, good restart ok; own key posts in a sibling demo* dir are not the owner (idle 5000); rc 6 with the app up goes on" "ok 2 unmeasured 2 8 down down ok 5000 ok" "$B50W"
check "rehearse b50 gates: .app without kangae-mirror-helper -> 3; helper + owner mouse 5 s ago -> 5 (idle read before anything starts)" "3 5" "$(b50_gate "$BG/rehearse.py" no '{"mouse_idle_s":5}') $(b50_gate "$BG/rehearse.py" yes '{"mouse_idle_s":5}')"
oib() { ( source "$BG/run.sh"; OWNER_IDLE_JSON='{"hid_idle_s":12,"key_idle_s":12,"mouse_idle_s":5000}' OWNER_NOW=1020 owner_idle_s "$B50/runs/demo1.A-restart" ); }
check "run.sh owner_idle_s: own type_text posts in a sibling demo1.A dir are not the owner when starting demo1.A-restart (5000.0)" "5000.0" "$(oib)"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF14'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/rh-b50{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
out("a", mut('    if o.get("effect") == "partial":  # 混雑', '    if False:  # 混雑'))  # 送り直しを消す
out("b", mut('nonce が無い")\n    finally:\n        C.depth = depth', 'nonce が無い")\n    finally:\n        pass'))  # depth を戻さない
out("c", mut('    if j.get("owner_active") or j.get("window_id") is None:', '    if j.get("window_id") is None:'))  # owner_active の rc 6 を ok と読む
out("d", mut('    if j.get("owner_active") or j.get("window_id") is None:', '    if j.get("owner_active") or False:'))  # window_id null を見ない
out("e", mut('os.path.join(d, "*", "steps.jsonl")', 'os.path.join(d, "run.*", "steps.jsonl")'))  # run.* に戻す
out("f", mut('    if not os.path.exists(os.path.join(app, "Contents/Helpers/kangae-mirror-helper")):', '    if False:'))  # helper の門を外す
EOF14
python3 -I - "$BG/run.sh" "$B50/run-b50.sh" <<'EOF15'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); o = '${1:+"${1%/*}"/*/steps.jsonl}'
assert s.count(o) == 1
open(sys.argv[2], "w", encoding="utf-8").write(s.replace(o, '${1:+"${1%/*}"/run.*/steps.jsonl}'))
EOF15
cp "$B50/run-b50.sh" "$BG/run-b50.$$.sh"
oibm() { ( source "$BG/run-b50.$$.sh"; OWNER_IDLE_JSON='{"hid_idle_s":12,"key_idle_s":12,"mouse_idle_s":5000}' OWNER_NOW=1020 owner_idle_s "$B50/runs/demo1.A-restart" ); }
ga=$(b50 "$BG/rh-b50a.$$.py"); gb=$(b50 "$BG/rh-b50b.$$.py"); gc=$(b50 "$BG/rh-b50c.$$.py"); gd=$(b50 "$BG/rh-b50d.$$.py"); ge=$(b50 "$BG/rh-b50e.$$.py")
check "rehearse b50 negative controls: no resend -> partial x2 reads ok; depth not restored -> 30; owner_active not checked -> restart ok; window_id not checked -> null-window restart ok; run.* glob -> idle 12; no helper gate -> not 3; run.sh run.* glob -> 12.0" "ok 1 ok 1|30|ok|ok|12|0 or 5|12.0" "$(cut -d' ' -f1-4 <<< "$ga")|$(cut -d' ' -f5 <<< "$gb")|$(cut -d' ' -f6 <<< "$gc")|$(cut -d' ' -f7 <<< "$gd")|$(cut -d' ' -f9 <<< "$ge")|$([ "$(b50_gate "$BG/rh-b50f.$$.py" no '{"mouse_idle_s":5}')" = 3 ] && echo 3 || echo "0 or 5")|$(oibm)"
rm -f "$BG"/rh-b50?.$$.py "$BG/run-b50.$$.sh"

# ---- W15-b52: driver 在りの手の読み戻し（I1 / I2 / A3 / C1 / C2 / #5 の同意）。偽の DB・偽の画面・偽の ps（実機を起こさない） ----
b52() {  # b52 <tag> <rehearse.py> ... -> "<scenario>=<ok|red|unm> ..." （1 行）
  python3 -I - "$B52/$1" "$BG" "$2" <<'EOF16'
import datetime, importlib.util, json, os, shutil, sqlite3, sys, time, types
base, BG, path = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
spec = importlib.util.spec_from_file_location("rh", path); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, BG); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
clk = [time.time()]  # 仮想の時計: sleep が進める
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + s))
iso = lambda t: datetime.datetime.fromtimestamp(t, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
def proj(rev, pages): return {"id": "p1", "revision": rev, "surface_specs": [{"id": "s"}], "pages": [{"id": i, "root": {"key": "root", "children": [{"key": k} for k in ks]}} for i, ks in pages]}
def put(R, revs, head, consents=0):
    db = R + "/data/kangae.sqlite3"
    if os.path.exists(db): os.remove(db)
    con = sqlite3.connect(db)
    con.executescript("CREATE TABLE projects(id TEXT PRIMARY KEY,json TEXT,created_at TEXT,updated_at TEXT);CREATE TABLE project_revisions(project_id,revision INTEGER,json TEXT,base_hash,ops_json TEXT,origin TEXT,created_at TEXT,PRIMARY KEY(project_id,revision));CREATE TABLE ui_state(session_id TEXT,key TEXT,json TEXT);")
    con.execute("INSERT INTO projects VALUES('p1',?,?,?)", (json.dumps(head), iso(clk[0]), iso(clk[0])))
    for rev, (origin, ops, pj, at) in revs.items(): con.execute("INSERT INTO project_revisions VALUES('p1',?,?,NULL,?,?,?)", (rev, json.dumps(pj), json.dumps(ops), origin, iso(at)))
    for i in range(consents): con.execute("INSERT INTO ui_state VALUES('mirror-consent',?,'{}')", (f"b{i}",))
    con.commit(); con.close()
cnt = [0]
def mk():
    cnt[0] += 1; R = f"{base}/s{cnt[0]}"; os.makedirs(R + "/data"); os.makedirs(R + "/shibubu-yc-demo/docs")
    open(R + "/shibubu-yc-demo/README.md", "w").write("# old\n")
    json.dump({"pid": 1, "window_id": 2, "session": "s"}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); C.state["te_sets"] = []
    return C
def out(fn, C, row):
    try:
        fn(C, row); return "ok"
    except rh.Unmeasured: return "unm"
    except rh.DriverDown: return "down"
    except RuntimeError: return "red"
res = {}
nop = lambda *a, **k: None
# I1 (#1)・L2: Project は案の `Build now`（TALK-1101・前の Accept）を押した時に書かれる（early = 押す前に在る → 赤）
def i1(name, origin="generate", n=2, at=5, early=False):
    C = mk(); pj = proj(1, [(f"pg{i}", ["k"]) for i in range(n)]); made = lambda: put(C.R, {1: (origin, [], pj, clk[0] + at)}, pj)
    if early: made()
    rh.send_field = nop; rh.find = lambda *a, **k: {}; rh.elements = lambda C_: [{"role": "AXButton", "label": "Build now", "element_token": "t"}]  # h_generate は Build now を elements で読む（灰 = md を見る）
    rh.press = lambda C_, role, label, *a, **k: label == "accept" and not early and made()
    res[name] = out(rh.h_generate, C, {})
i1("i1_ok"); i1("i1_origin", "nl_edit"); i1("i1_pages", n=1); i1("i1_stale", at=-100); i1("i1_early", early=True)
# I2 (#12 #13 #14 #21)
BASE = [("pg1", ["s.excal", "t.term"]), ("pg2", ["s.docs"])]; ADD = BASE + [("pg3", [])]
MV = lambda k, to: [{"op": "move_surface", "surface_key": k, "to_page": to, "parent": "root"}]
SPEC = lambda f: {"id": "notes", "backend": "mirror", "target": {"App": {"bundle_id": "com.apple.TextEdit", "file": f}}}
W1, W2 = [(7, 10, 20, 400, 300)], [(7, 12, 20, 400, 300)]  # D1: TextEdit の窓(id, x, y, w, h)の前後
def edit(name, text, before, after, ops, origin="nl_edit", apply=True, wins=None):
    it = iter(wins or []); rh.te_windows = lambda C_: next(it)
    C = mk(); p1, p2 = proj(1, before), proj(2, after); g = ("generate", [], p1, clk[0] - 50)
    put(C.R, {1: g}, p1)
    rh.send_field = lambda C_, role, label, t, enter=False, expect=None: apply and put(C.R, {1: g, 2: (origin, ops, p2, clk[0])}, p2)
    rh.open_textedit = lambda p: ({1}, {1})
    res[name] = out(rh.h_nl_edit, C, {"doing": f"欄に『{text}』と打って送る"})
AP = [{"op": "add_page"}]
edit("e_add", "add a Tests page", BASE, ADD, AP)
edit("e_add2", "add a Tests page", BASE, ADD, AP + AP)
edit("e_origin", "add a Tests page", BASE, ADD, AP, origin="direct_edit")
edit("e_changed", "add a Tests page", BASE, [("pg1", ["s.excal", "t.term", "x"]), ("pg2", ["s.docs"]), ("pg3", [])], AP)
edit("e_norev", "add a Tests page", BASE, ADD, AP, apply=False)
edit("e_move", "move Excalidraw right", BASE, [("pg1", ["t.term"]), ("pg2", ["s.docs", "s.excal"])], MV("s.excal", "pg2"))
edit("e_move_left", "move Excalidraw right", [("pg1", ["s.docs"]), ("pg2", ["s.excal"])], [("pg1", ["s.docs", "s.excal"]), ("pg2", [])], MV("s.excal", "pg1"))
TE_AFTER = [("pg1", ["s.excal", "t.term"]), ("pg2", []), ("pg3", ["s.docs"])]
edit("e_textedit", "move TextEdit right", ADD, TE_AFTER, MV("s.docs", "pg3"), wins=(W1, W1))
edit("e_textedit_moved", "move TextEdit right", ADD, TE_AFTER, MV("s.docs", "pg3"), wins=(W1, W2))
edit("e_textedit_id", "move TextEdit right", ADD, TE_AFTER, MV("s.docs", "pg3"), wins=(W1, [(8, 10, 20, 400, 300)]))
edit("e_textedit_left", "move TextEdit right", [("pg1", ["s.excal", "t.term"]), ("pg2", []), ("pg3", ["s.docs"])], ADD, MV("s.docs", "pg2"), wins=(W1, W1))
edit("e_textedit_nowin", "move TextEdit right", ADD, TE_AFTER, MV("s.docs", "pg3"), wins=([], []))
N3 = [("pg1", ["s.excal", "t.term"]), ("pg2", ["s.docs"]), ("pg3", ["surface.notes"])]
edit("e_notes", "notes.md を Mirror で見る Page を足して", BASE, N3, AP + [{"op": "add_surface", "spec": SPEC("docs/notes.md")}])
edit("e_notes_readme", "notes.md を Mirror で見る Page を足して", BASE, N3, AP + [{"op": "add_surface", "spec": SPEC("README.md")}])
# A3 (#15 #16)
J1, J2 = proj(1, [("pg1", ["a"])]), proj(2, [("pg1", ["a"]), ("pg2", [])]); JX = proj(1, [("pgX", [])]); JY = proj(2, [("pgY", [])])
def ur(name, fn, label, head, to=None, readme=False):
    C = mk(); put(C.R, {1: ("generate", [], J1, clk[0]), 2: ("nl_edit", [], J2, clk[0])}, head)
    def pr(C_, role, lab, expect=None, **k):
        if to: put(C.R, {1: ("generate", [], J1, clk[0]), 2: ("nl_edit", [], J2, clk[0])}, to)
        if readme: open(C.repo + "/README.md", "w").write("# changed\n")
    rh.press = pr
    res[name] = out(fn, C, {})
ur("u_ok", rh.h_undo, "undo", J2, J1); ur("u_nochange", rh.h_undo, "undo", J2); ur("u_wrong", rh.h_undo, "undo", J2, JX); ur("u_file", rh.h_undo, "undo", J2, J1, readme=True)
ur("r_ok", rh.h_redo, "redo", J1, J2); ur("r_wrong", rh.h_redo, "redo", J1, JY)
# C1 (#3)
ROW3 = {"doing": "Terminal に『echo <nonce>』『echo $((<a>*<b>))』『false; echo \"<nonce>:$?\"』『git status』『echo <nonce> > undo-probe.txt』を打つ"}
GOOD = "% echo n1\nn1\n% echo $((7*13))\n91\n% false; echo \"n1:$?\"\nn1:1\n"
def c1(name, text=GOOD, probe=True, **ps):
    C = mk(); p = {"ppid=": "1", "tty=": "ttys003", "comm=": "/bin/zsh", **ps}
    rh.elements = lambda C_: [{"role": "AXStaticText", "value": text}, {"role": "AXStaticText", "value": "zsh · pid 4242"}]
    rh.ps_col = lambda pid, col: p[col]
    rh.send_field = lambda C_, role, label, t, enter=False, expect=None: probe and "undo-probe" in t and open(C.repo + "/undo-probe.txt", "w").write("n1\n")
    res[name] = out(rh.h_term_type, C, ROW3)
c1("c1_ok"); c1("c1_noline", GOOD.replace("n1:1\n", "n1:0\n")); c1("c1_ppid", **{"ppid=": "999"}); c1("c1_tty", **{"tty=": "??"}); c1("c1_comm", **{"comm=": "fish"}); c1("c1_noprobe", probe=False)
# C2 (#4 → #7)
rh.press_tab = lambda C_, kind, expect=None: rh.press(C_, "AXCheckBox", {"docs": "Docs", "dev": "Dev", "diagram": "Diagram"}[kind], expect=expect)  # b71: 種類で Page の題を引く所は b71 の塊で測る（ここは押した後の振る舞い）
def c2(name, after_pid=4242, done=True):
    C = mk(); sent = [None]
    def els(C_):
        fin = sent[0] is not None and clk[0] >= sent[0] + 15
        return [{"role": "AXStaticText", "value": "% sleep 15; echo done\n" + ("done\n" if fin and done else "")}, {"role": "AXStaticText", "value": f"zsh · pid {after_pid if fin else 4242}"}]
    rh.elements = els; rh.press = nop
    rh.send_field = lambda C_, role, label, t, enter=False, expect=None: sent.__setitem__(0, clk[0])
    a = out(rh.h_term_type, C, {"doing": "Terminal に『sleep 15; echo done』を打つ"})
    res[name] = a if a != "ok" else out(rh.h_goto_tab, C, {"doing": "『Dev』のタブへ移り、『Docs』へ戻る"})
c2("c2_ok"); c2("c2_pid", 4300); c2("c2_nodone", done=False)
# #5 の同意
def mc(name, consents=1, frame=True, db=True, sids=("docs",), ax="docs", te=4242):
    C = mk(); C.state["te_pid"] = te; (db and put(C.R, {}, {**J1, "surface_specs": [{"id": i, "backend": "mirror", "target": {"kind": "app", "file": "README.md"}} for i in sids]}, consents)) or (not db and None)
    rh.find = lambda *a, **k: {"element_token": "t"}; rh.press = nop; rh.ax_ids = lambda C_: [f"surface-frame:{ax}"] if frame else []
    res[name] = out(rh.h_goto_tab, C, {"doing": "『Docs』のタブを押し、`Mirror here` を押して TextEdit の Mirror に同意する"})
mc("m_ok"); mc("m_noconsent", 0); mc("m_noframe", frame=False); mc("m_nodb", db=False)
mc("m_readme", sids=("readme",), ax="readme"); mc("m_readme_axdocs", sids=("readme",)); mc("m_two", sids=("readme", "docs")); mc("m_nospec", sids=()); mc("m_nopid", 0, te=None)  # b77: 面の id は DB から（本物の AI = readme・r21b）
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF16
}
B52="$T/b52"; rm -rf "$B52"; mkdir -p "$B52"
B52C=$(b52 clean "$BG/rehearse.py")
check "rehearse b52 readbacks (fake DB / screen / ps): I1 generate ok / other origin / 1 page / stale red; I2 add_page ok / 2 ops / origin / old page changed / no new revision red; Excalidraw right ok / left red; TextEdit move ok / window bounds moved red / window id changed red / moved left red / no window before unm; notes.md Mirror ok / README spec red; A3 undo+redo ok / unchanged / other hash / README changed red; C1 ok / no nonce:1 / ppid / tty / comm / no probe red; C2 ok / pid changed / no done red; #5 consent ok / no record red / no frame unm / no db unm / b77 id readme from DB ok / readme in DB but docs on AX unm / 2 specs unm / no spec unm / b79 no record but the driver never got its README TextEdit pid (setup) unm" \
  "i1_ok=ok i1_origin=red i1_pages=red i1_stale=red i1_early=red e_add=ok e_add2=red e_origin=red e_changed=red e_norev=red e_move=ok e_move_left=red e_textedit=ok e_textedit_moved=red e_textedit_id=red e_textedit_left=red e_textedit_nowin=unm e_notes=ok e_notes_readme=red u_ok=ok u_nochange=red u_wrong=red u_file=red r_ok=ok r_wrong=red c1_ok=ok c1_noline=red c1_ppid=red c1_tty=red c1_comm=red c1_noprobe=red c2_ok=ok c2_pid=red c2_nodone=red m_ok=ok m_noconsent=red m_noframe=unm m_nodb=unm m_readme=ok m_readme_axdocs=unm m_two=unm m_nospec=unm m_nopid=unm" "$B52C"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF17'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
def mut(n, old, new):
    assert s.count(old) == 1, old
    open(f"{sys.argv[2]}/rh-b52{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(s.replace(old, new))
mut("a", '    raise RuntimeError("I1: Project が', '    return None  # Project が')  # I1 の外れを赤にしない
mut("b", 'if last["origin"] != "nl_edit":', 'if False:')
mut("c", 'if got != want:', 'if False:')
mut("d", 'if any(st["page_sha"].get(i) != h', 'if False and any(st["page_sha"].get(i) != h')
mut("e", 'if " right" in text and (', 'if False and (')
mut("f", 'if spec.get("backend") != "mirror" or "com.apple.TextEdit" not in dump or "notes.md" not in dump:', 'if False:')
mut("g", '        raise RuntimeError("I2: 送ったのに', '        return  # 送ったのに')
mut("h", 'st["head_sha"] != (st["rows"].get(st["rev"]) or {}).get("sha")', 'False')
mut("i", 'if repo_shas(C) != files:', 'if False:')
mut("j", 'if ppid != str(json.load(open(C.R + "/run.json"))["pid"]) or not tty', 'if not tty')
mut("k", 'want = (C.nonce, str(7 * 13), f"{C.nonce}:1")', 'want = (C.nonce,)')
mut("l", '    raise RuntimeError("C1: repo の undo-probe.txt', '    return  # repo の undo-probe.txt')
mut("m", 'if term_pid(text) != c2["pid"]:', 'if False:')
mut("n", '        raise RuntimeError("C2: sleep 15', '        return  # sleep 15')
mut("o", 'if n < 1:', 'if False:')
mut("p", '    if w0 != w1:', '    if False:')
mut("q", '    if not w0:', '    if False:')
mut("r", '    if (made := fresh_projects(C, t_send)):', '    if False:')  # L2: Accept の前に在る Project を赤にしない
EOF17
flip() {  # flip <tag> -> 清浄から ok に変わった scenario の名前
  python3 -I -c 'import sys;a=dict(x.split("=") for x in sys.argv[1].split());b=dict(x.split("=") for x in sys.argv[2].split());print(",".join(k for k in a if a[k]!=b[k] and b[k]=="ok") or "none")' "$B52C" "$(b52 "mut$1" "$BG/rh-b52$1.$$.py")"
}
check "rehearse b52 negative controls: no I1 raise -> i1_origin,i1_pages,i1_stale read ok; no origin check -> e_origin; no ops check -> e_add2; no old-page hash -> e_changed; no right-of check -> e_move_left,e_textedit_left; no window compare -> e_textedit_moved,e_textedit_id; no empty-before guard -> e_textedit_nowin; no spec check -> e_notes_readme; no revision-grew check -> e_norev; no A3 hash -> u_wrong,r_wrong; no A3 files -> u_file; no C1 ppid -> c1_ppid; no nonce:1 -> c1_noline; no probe -> c1_noprobe; no C2 pid -> c2_pid; no done -> c2_nodone; no consent record -> m_noconsent,m_nopid; no pre-Accept check (L2) -> i1_early" \
  "i1_origin,i1_pages,i1_stale|e_origin|e_add2|e_changed|e_move_left,e_textedit_left|e_notes_readme|e_norev|u_wrong,r_wrong|u_file|c1_ppid|c1_noline|c1_noprobe|c2_pid|c2_nodone|m_noconsent,m_nopid|e_textedit_moved,e_textedit_id|e_textedit_nowin|i1_early" \
  "$(for m in a b c d e f g h i; do flip $m; done | paste -sd'|' -)|$(for m in j k l m n o p q r; do flip $m; done | paste -sd'|' -)"
rm -f "$BG"/rh-b52?.$$.py

# ---- W15-b53: 録画 mode の FG 5 手（偽の前面・偽の AX value・偽の relay / README・実機を起こさない） ----
b53() {  # b53 <tag> <rehearse.py> -> "<scenario>=<ok|red|unm|stuck|...> ..." （1 行）
  python3 -I - "$B53/$1" "$BG" "$2" <<'EOF18' | tail -1
import importlib.util, json, os, shutil, signal, subprocess, sys, time, types
base, BG, path = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
spec = importlib.util.spec_from_file_location("rh", path); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, BG); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
clk = [time.time()]
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + s))
r_in, w_in = os.pipe(); sys.stdin = os.fdopen(r_in)  # 何も書かない pipe: stdin を待つ道は返らない
class Stuck(Exception): pass
def alarm(*a): raise Stuck()
signal.signal(signal.SIGALRM, alarm)
rows, _ = rh.parse(rh.DEMO); ROW = {r["n"]: r for r in rows}
ME, TE, NT = 100, 200, 300  # Shibubu / README の TextEdit / notes.md の TextEdit の pid
os.environ["GIT_CONFIG_GLOBAL"] = "/dev/null"; os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
cnt = [0]
def sh(repo, *a): subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@l", *a], cwd=repo, capture_output=True)
def mk(git=True):
    cnt[0] += 1; R = f"{base}/s{cnt[0]}"; os.makedirs(R + "/data"); os.makedirs(R + "/shibubu-yc-demo/docs")
    repo = R + "/shibubu-yc-demo"; open(repo + "/README.md", "w").write("# old\n"); open(repo + "/docs/notes.md", "w").write("notes\n")
    if git:
        sh(repo, "init", "-q", "-b", "main"); sh(repo, "add", "README.md"); sh(repo, "commit", "-q", "-m", "i")
    json.dump({"pid": ME, "window_id": 2, "session": "s"}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); C.state.update(te_sets=[], te_pid=TE, notes_pid=NT)
    return C
def fronts(seq):
    it = iter(seq); last = [seq[0]]
    def f():
        last[0] = next(it, last[0]); return last[0]
    rh.front_pid = f
def run(C, n, nonce="n1"):
    signal.alarm(3)
    try:
        return rh.run_fg(C, ROW[n], nonce)
    except rh.Unmeasured as e: return "unm", str(e)
    except rh.DriverDown: return "down", ""
    except RuntimeError as e: return "red", str(e)
    except Stuck: return "stuck", ""
    finally: signal.alarm(0)
res = {}
# #17: 前面が TextEdit になるのを見張る（stdin は待たない）
def lease(name, seq, ax=("", "x"), pid=TE):
    C = mk(); C.state["te_pid"] = pid; vals = iter(ax); lastv = [ax[0]]
    def tv(C_, p, t):
        lastv[0] = next(vals, lastv[0]); return lastv[0]
    rh.te_value = tv; fronts(seq); res[name] = run(C, 17)[0]
lease("lease_ok", [9, 9, TE]); lease("lease_timeout", [9]); lease("lease_noax", [9, TE], ax=("", "")); lease("lease_nopid", [TE], pid=None)
# #19: 前面が Shibubu になるのを見張る・前面の pid の列を note に残す
def front(name, seq):
    C = mk(); fronts(seq); st, note = run(C, 19); res[name] = st; res[name + "_note"] = "yes" if "front=[" in note and str(ME) in note else "no"
front("front_ok", [TE, 9, ME]); front("front_never", [TE])
# #6 #8 #24: Enter の前後の読み戻し。Enter（wait_enter）で owner の手を模す
def typed(name, n, relay=1, ax="old|new", sha=True, disk="n1", git=True, nonce="n1", pre=False):
    C = mk(git); st = {"ax": ax.split("|")[0] or None}; notes = n == 24
    doc = C.repo + ("/docs/notes.md" if notes else "/README.md")
    rh.te_value = lambda C_, p, t: st["ax"]
    if pre: open(doc, "w").write("text n1\n")  # 打つ前から disk に nonce が有る（sha が変わらなければ ⌘S が効いていない）
    def enter():
        for _ in range(relay): open(C.R + "/data/mirror-relay.jsonl", "a").write('{"seq":1}\n')
        if "|" in ax: st["ax"] = ax.split("|")[1] or None
        if sha: open(doc, "w").write(f"text {disk}\n")
        return [TE, 9]
    rh.wait_enter = enter; fronts([TE])
    res[name] = run(C, n, nonce)[0]
typed("t6_ok", 6, ax="old|old n1", sha=False); typed("t6_relay_flat", 6, relay=0, ax="old|old n1", sha=False); typed("t6_no_nonce", 6, ax="old|old zz", sha=False)
typed("t6_autosaved", 6, ax="old|old n1"); typed("t6_noax", 6, ax="|")
typed("t8_ok", 8); typed("t8_sha_same", 8, sha=False, pre=True); typed("t8_ax_same", 8, ax="same|same"); typed("t8_no_nonce_disk", 8, disk="zz"); typed("t8_no_git_diff", 8, git=False)
typed("t24_ok", 24, git=False); typed("t24_sha_same", 24, sha=False, git=False, pre=True); typed("t24_no_nonce_disk", 24, disk="zz", git=False)
# _live: 章の tsv に front の行が出る（#19 だけを台本に残した 1 手の run）
def live_tsv():
    C = mk(); fronts([9, ME]); out = f"{base}/ch.tsv"; os.environ["SHIBUBU_QA_RECORD"] = "1"; os.environ["SHIBUBU_QA_CHAPTERS"] = out
    rh.run_sh = lambda *a, ws=None, fake=None: (0, ""); rh.write_fake = lambda *a, **k: C.R
    class P: returncode = 0; stdout = "{}"; stderr = ""
    real = subprocess.run
    rh.subprocess = types.SimpleNamespace(run=lambda a, **k: P if "tally.py" in " ".join(a) else real(a, **k), check_output=subprocess.check_output)
    signal.alarm(5)
    try: rh._live([ROW[19]], 1, "X.app", True, 1, "n1", C)
    except Stuck: pass
    finally: signal.alarm(0)
    txt = open(out, encoding="utf-8").read() if os.path.exists(out) else ""
    res["tsv_front"] = "yes" if "\tfront=[" in txt and "\towner\t" in txt else "no"
live_tsv()
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF18
}
B53="$T/b53"; rm -rf "$B53"; mkdir -p "$B53"
B53C=$(b53 clean "$BG/rehearse.py")
check "rehearse b53 FG hands (fake front / AX / relay): #17 front TextEdit ok / never -> unm / AX unchanged -> unm / no pid -> unm, never reads stdin; #19 front Shibubu ok with the pid list in the note / never -> unm; #6 ok / relay flat red / no nonce red / autosave first unm / no AX unm; #8 ok / sha same red / AX same red / no nonce on disk red / no git diff red; #24 ok / sha same red; chapters tsv has the front row" \
  "lease_ok=owner lease_timeout=unm lease_noax=unm lease_nopid=unm front_ok=owner front_ok_note=yes front_never=unm front_never_note=yes t6_ok=owner t6_relay_flat=red t6_no_nonce=red t6_autosaved=unm t6_noax=unm t8_ok=owner t8_sha_same=red t8_ax_same=red t8_no_nonce_disk=red t8_no_git_diff=red t24_ok=owner t24_sha_same=red t24_no_nonce_disk=red tsv_front=yes" "$B53C"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF19'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
def mut(n, old, new):
    assert s.count(old) == 1, old
    open(f"{sys.argv[2]}/rh-b53{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(s.replace(old, new))
mut("a", '    ok, fronts = watch_front(pid)\n', '    fronts = wait_enter(); ok = True\n')  # lease が stdin の Enter を待つ
mut("b", '        if time.time() >= t_end:', '        if False:')  # 見張りの上限を外す
mut("c", '    ok, fronts = watch_front(me)\n', '    fronts = wait_enter(); ok = True\n')  # front が stdin の Enter を待つ
mut("d", 'return "owner", f"front={fronts} Shibubu', 'return "owner", f"Shibubu')  # 前面の pid の列を note に書かない
mut("e", '    if a["relay"] <= b["relay"]:', '    if False:')
mut("f", '        if nonce not in a["ax"]:', '        if False:')
mut("g", '        if a["sha"] != b["sha"]:\n            raise Unmeasured("自動保存', '        if False:\n            raise Unmeasured("自動保存')
mut("h", '    if a["sha"] == b["sha"]:\n        raise RuntimeError("E1', '    if False:\n        raise RuntimeError("E1')
mut("i", '    if not notes and nonce not in subprocess.run(["git"', '    if False and nonce not in subprocess.run(["git"')
mut("j", '        if (te_value(C, pid, "README.md") or "") != ax0:', '        if True:')
mut("k", 'if a["ax"] == (b["ax"] or ""):', 'if False:')
mut("l", 'if nonce not in open(a["path"]', 'if False and nonce not in open(a["path"]')
mut("m", "{x['status'][:60]}\\t{x['note']", "{x['status'][:60]}\\t{'' and x['note']")
EOF19
flip53() {  # flip53 <tag> -> 清浄から変わった scenario の名前
  python3 -I -c 'import sys;a=dict(x.split("=") for x in sys.argv[1].split());b=dict(x.split("=") for x in sys.argv[2].split());print(",".join(k+":"+b[k] for k in a if a[k]!=b[k]) or "none")' "$B53C" "$(b53 "mut$1" "$BG/rh-b53$1.$$.py")"
}
check "rehearse b53 negative controls: lease waits stdin -> lease_ok stuck; no watch limit -> lease_timeout stuck; front waits stdin -> front_ok stuck; no pid list -> front_ok_note no; no relay check -> t6_relay_flat ok; no nonce check -> t6_no_nonce ok; no autosave check -> t6_autosaved ok; no sha check -> t8_sha_same / t24_sha_same ok; no git diff check -> t8_no_git_diff ok; lease AX unchanged -> lease_noax ok; no AX-changed check -> t8_ax_same ok; no disk nonce -> t24_no_nonce_disk ok; no tsv note -> tsv_front no" \
  "lease_ok:stuck,lease_timeout:stuck,lease_noax:stuck|lease_timeout:stuck,front_never:stuck,front_never_note:no|front_ok:stuck,front_ok_note:no,front_never:stuck,front_never_note:no|front_ok_note:no,tsv_front:no|t6_relay_flat:owner|t6_no_nonce:owner|t6_autosaved:owner|t8_sha_same:owner,t24_sha_same:owner|t8_no_git_diff:owner|lease_noax:owner|t8_ax_same:owner|t24_no_nonce_disk:owner|tsv_front:no" \
  "$(for m in a b c d e f g h i j k l m; do flip53 $m; done | paste -sd'|' -)"
rm -f "$BG"/rh-b53?.$$.py

# ---- W15-b54: R1 と R2 の両方を数える・標準の未測は exit に数えない・ax:* の効果・EN の BACKED_SENTENCES・h_recount の before / after・h_restart の symlink・B3・--expect ----
B54="$T/b54"; rm -rf "$B54"; mkdir -p "$B54"
b54t() {  # b54t <tally.py> -> "n1=<R1 だけの lies> n12=<R1 + R2(裏無し)の lies> nt=<R2(木無し)に restart:lies が在る> std=<標準だけの open> one=<lies を含む open> axc=<ax:* が変化を見る> axs=<同じ木は見ない> en=<EN の文の lies>"
  python3 -I - "$1" "$LQ" "$B54" <<'EOF20'
import importlib.util, os, shutil, sys
spec = importlib.util.spec_from_file_location("tl", sys.argv[1]); tl = importlib.util.module_from_spec(spec); sys.path.insert(0, os.path.dirname(sys.argv[1])); sys.modules["tl"] = tl; spec.loader.exec_module(tl)
lq, base = sys.argv[2], sys.argv[3]
r1, ra, rb = lq + "/run", base + "/r2a", base + "/r2b"
for r in (ra, rb):
    shutil.rmtree(r, ignore_errors=True); shutil.copytree(r1, r)
for f in ("engine-runs.jsonl", "kangae.sqlite3"): os.remove(f"{ra}/data/{f}")  # R2 a: 裏の状態が無い = 成功の文は嘘
shutil.rmtree(rb + "/ax")                                                       # R2 b: AX の木が無い
u = tl.count_runs(r1, rb).unmeasured
t = lambda texts: {"elements": [{"element_index": i, "role": "AXStaticText", "label": x} for i, x in enumerate(texts)]}
seen = lambda a, b: tl.effect_seen({"step": "s"}, tl.parse_expect("ax:*"), t(a), t(b), {})
tr = t(["Done. Checked 2 items in the original app"]); tx = []; tl._texts(tr, tx)
print(f"n1={len(tl.count(r1).lies)} n12={len(tl.count_runs(r1, ra).lies)} nt={int('restart:lies' in u)} std={len(tl.open_unmeasured({k: 'x' for k in ('lies.surface_status', 'native_verified', 'web_verified', 'service_verified', 'terminal_real', 'local_only')}))}",
      f"one={len(tl.open_unmeasured({'lies': 'x', 'native_verified': 'y'}))} axc={int(seen(['a'], ['b']))} axs={int(seen(['a'], ['a']))}",
      f"en={len(tl.find_lies(tr, {}, 's', {tl.sentence_of(x[1]): 10.0 for x in tx}, {'verified': 5.0}))}")
EOF20
}
B54T=$(b54t "$BG/tally.py")
check "tally b54: R1 only 0 lies; R1 + R2 without backing 3 lies; R2 without a tree -> restart:lies unmeasured; standing-only unmeasured -> 0 open, with lies -> 1; ax:* sees a changed tree, not the same tree; EN 'Done. Checked' backed -> 0 lies" "n1=0 n12=3 nt=1 std=0 one=1 axc=1 axs=0 en=0" "$B54T"
python3 -I - "$BG/tally.py" "$BG" "$$" <<'EOF21'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/tl-b54{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
out("a", mut('    if r2 and os.path.isdir(r2):\n        u = count(', '    if False:\n        u = count('))  # R2 を数えない
out("b", mut('"restart:" + k: v for k, v in b.items() if k not in STANDING_UNMEASURED', '"restart:" + k: v for k, v in b.items() if False'))  # R2 の unmeasured を落とす
out("c", mut('STANDING_UNMEASURED = ("lies.surface_status", "native_verified", "web_verified", "service_verified", "terminal_real", "local_only")', 'STANDING_UNMEASURED = ()'))  # 標準の未測を空に
out("d", mut('target == "*" or target in name(e)', 'target in name(e)'))  # ax:* を読まない
out("e", mut('("確かめました", "検証済み", "Done. Checked")', '("確かめました", "検証済み")'))  # EN の文を足さない
EOF21
flip54t() {  # flip54t <tag> -> 清浄から変わった key
  python3 -I -c 'import sys;a=dict(x.split("=") for x in sys.argv[1].split());b=dict(x.split("=") for x in sys.argv[2].split());print(",".join(k for k in a if a[k]!=b[k]) or "none")' "$B54T" "$(b54t "$BG/tl-b54$1.$$.py")"
}
check "tally b54 negative controls: R2 not counted -> n12,nt; R2 unmeasured dropped -> nt; STANDING empty -> std; ax:* unread -> axc; EN backed sentence removed -> en" "n12,nt|nt|std,one|axc|en" "$(for m in a b c d e; do flip54t $m; done | paste -sd'|' -)"
rm -f "$BG"/tl-b54?.$$.py
b54r() {  # b54r <rehearse.py> -> "rc_same=… rc_lost=… rc_blind=… rc_none=… sym=… repo=… b3_called=… no_b1=… b3_calls=… ex_*=…"
  python3 -I - "$1" "$B54" "$BG" "$LQ" <<'EOF22'
import importlib.util, json, os, shutil, sys, time, types
def load(path, bg):
    spec = importlib.util.spec_from_file_location("rh", path); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, bg); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
    rh.time = types.SimpleNamespace(time=time.time, sleep=lambda s: None)
    return rh
base, lq = sys.argv[2], sys.argv[4]
rh = load(sys.argv[1], sys.argv[3])
rec = lambda ok: {"schema": "shibubu.qa.excalidraw/1", "t": 1, "path": "/x", "exists": True, "sha256": "0" * 64, "elements": 3, "nonces": {"n-demo-b1-n": ok, "n-demo-b3-n": ok}}
def pair(tag, after, before=True):
    r1, r2 = f"{base}/{tag}1", f"{base}/{tag}2"
    for r in (r1, r2): shutil.rmtree(r, ignore_errors=True); shutil.copytree(lq + "/run", r); os.makedirs(r + "/oracle", exist_ok=True)
    json.dump(rec(before), open(r1 + "/oracle/B1.excalidraw.after.json", "w"))
    if after is not None: json.dump(rec(after), open(r2 + "/oracle/B3.excalidraw.after.json", "w"))
    return r1, r2
def run(fn, C, row=None):
    try:
        fn(C, row or {}); return "ok"
    except rh.Unmeasured: return "unm"
    except RuntimeError: return "red"
rh.elements = lambda C: []
def recount(after, restarted=True, before=True, extra=None):
    r1, r2 = pair("rc", after, before); C = rh.Ctx(r2, "X.app", "n"); C.state.update(R1=r1, **({"restarted": r2} if restarted else {}))
    if extra:  # 数えた物に lies / dead_clicks を 1 件足す（full.md 27 の「lies 0・dead_clicks 0」）
        real = rh.tally_all; rh.tally_all = lambda C_: {**real(C_), extra: [{"x": 1}]}
        try:
            return run(rh.h_recount, C)
        finally:
            rh.tally_all = real
    return run(rh.h_recount, C)
res = dict(rc_same=recount(True), rc_lost=recount(False), rc_blind=recount(None), rc_none=recount(True, False),
           rc_lies=recount(True, extra="lies"), rc_dead=recount(True, extra="dead_clicks"), rc_empty=recount(False, before=False))
# h_restart: data は symlink・C.repo は R1 のまま・B3 を呼ぶ・2 回目でも落ちない
R1 = f"{base}/rs"; shutil.rmtree(R1 + "-restart", ignore_errors=True)
for x in ("data", "shibubu-yc-demo", "fake"): os.makedirs(f"{R1}/{x}", exist_ok=True)
open(R1 + "/data/k", "w").write("1")
def fake_run(*a, ws=None, fake=None):
    if a[0] == "start": json.dump({"window_id": 7}, open(a[1] + "/run.json", "w"))
    return 0, ""
called = []; rh.run_sh = fake_run; rh.b3_check = lambda C: called.append(1)
order = []; rh.press = lambda C, role, label, expect=None, **k: order.append(label)
# D1: 止める前と後の TextEdit の窓・起こし直した後の tab の順（x の順）と DB の Page の順
win = [[(7, 0, 0, 500, 400)]]; rh.te_windows = lambda C: list(win[0])
rh.db_state = lambda C: {"d": {"name": "Dev environment", "pages": [{"title": t} for t in ("Dev", "Docs", "Diagram")]}}
tabs = [("Dev environment", 132), ("Dev", 413), ("Docs", 461), ("Diagram", 518)]
rh.elements = lambda C: [{"role": "AXCheckBox", "label": t, "frame": {"x": x}} for t, x in tabs]
C = rh.Ctx(R1, "X.app", "n"); rh.h_restart(C, {})
res_entry = order[:1] == ["Dev environment"] and len(called) == 1  # 入口を押してから B3
d2 = R1 + "-restart/data"
res.update(entry=int(res_entry), sym=int(os.path.islink(d2) and os.path.realpath(d2) == os.path.realpath(R1 + "/data") and os.path.exists(d2 + "/k")), repo=int(C.repo == R1 + "/shibubu-yc-demo"), b3_called=len(called))
rh.h_restart(rh.Ctx(R1, "X.app", "n"), {}); res["again"] = len(called)
# r8: 起こし直しの start の rc 6 は 00-launch の判定の未測（app は居る）でも出る → 続ける。owner_active なら driver down
def rc6_run(active):
    def f(*a, ws=None, fake=None):
        if a[0] == "start":
            json.dump({"window_id": 7, "owner_active": active}, open(a[1] + "/run.json", "w")); return 6, ""
        return 0, ""
    return f
def down(fn, C):
    try: return run(fn, C)
    except rh.DriverDown: return "down"
    except OSError: return "crash"
n0 = len(called); rh.run_sh = rc6_run(False); res["rc6_up"] = down(rh.h_restart, rh.Ctx(R1, "X.app", "n")) + str(len(called) - n0)
rh.run_sh = rc6_run(True); res["rc6_owner"] = down(rh.h_restart, rh.Ctx(R1, "X.app", "n")); rh.run_sh = fake_run
def nojson_run(*a, ws=None, fake=None):  # start が run.json を書かずに落ちた
    if a[0] == "start" and os.path.exists(a[1] + "/run.json"): os.remove(a[1] + "/run.json")
    return 6 if a[0] == "start" else 0, "boom"
rh.run_sh = nojson_run; res["nojson"] = down(rh.h_restart, rh.Ctx(R1, "X.app", "n")); rh.run_sh = fake_run
tabs[2], tabs[3] = ("Docs", 518), ("Diagram", 461); res["d1_order"] = run(rh.h_restart, rh.Ctx(R1, "X.app", "n")); tabs[2], tabs[3] = ("Docs", 461), ("Diagram", 518)
gone_tab = tabs.pop(); res["d1_overflow"] = run(rh.h_restart, rh.Ctx(R1, "X.app", "n")); tabs.append(gone_tab)  # 狭い窓で Diagram の tab が AX に無い = 順を比べられない
real_stop = fake_run
def moved_run(*a, ws=None, fake=None):  # 止めた時に TextEdit の窓が替わる（新しい窓 id）
    if a[0] == "stop": win[0] = [(8, 0, 0, 500, 400)]
    return real_stop(*a, ws=ws, fake=fake)
rh.run_sh = moved_run; res["d1_win"] = run(rh.h_restart, rh.Ctx(R1, "X.app", "n")); rh.run_sh = fake_run; win[0] = [(7, 0, 0, 500, 400)]
order.clear(); rh.b3_check = lambda C: (_ for _ in ()).throw(rh.Unmeasured("b3")); res["d1_b3unm"] = run(rh.h_restart, rh.Ctx(R1, "X.app", "n"))  # B3 が未測でも D1 は測る（D1 が緑なら未測のまま）
rh.run_sh = moved_run; res["d1_b3unm_win"] = run(rh.h_restart, rh.Ctx(R1, "X.app", "n")); rh.run_sh = fake_run; win[0] = [(7, 0, 0, 500, 400)]  # B3 未測 + D1 赤 = 赤
# B3: B1 が済んでいなければ未測・済んでいれば入口を押して n1 と n2 の両方を読む
rh = load(sys.argv[1], sys.argv[3]); rh.elements = lambda C: []
calls = []; rh.press = lambda C, role, label, expect=None, **k: calls.append(("press", role, label) + ((expect,) if expect else ())); rh.b1_flow = lambda C, n2, want, out: calls.append(("flow", n2, tuple(want), out))
# 手 13 の後: Excalidraw の面（target の url に excalidraw）は Tests・README の Mirror は Diagram
src = lambda ref: {"children": [{"component": "source_surface", "props": {"surface_ref": ref}}] if ref else []}
rh.db_state = lambda C: {"d": {"surface_specs": [{"id": "docs", "backend": "mirror", "target": {"kind": "app", "bundle_id": "com.apple.TextEdit"}}, {"id": "diagram", "backend": "webview", "target": {"kind": "url", "url": "https://excalidraw.com"}}],
                               "pages": [{"title": t, "root": src(ref)} for t, ref in (("Dev", None), ("Docs", None), ("Diagram", "docs"), ("Tests", "diagram"))]}}
r1, r2 = pair("b3", None); C = rh.Ctx(r2, "X.app", "n"); C.state["R1"] = r1
os.remove(r1 + "/oracle/B1.excalidraw.after.json"); res["no_b1"] = run(lambda C_, r_: rh.b3_check(C_), C); res["no_b1_calls"] = len(calls)
json.dump(rec(True), open(r1 + "/oracle/B1.excalidraw.after.json", "w")); run(lambda C_, r_: rh.b3_check(C_), C)
res["b3_calls"] = int(calls == [("press", "AXCheckBox", "Tests"), ("flow", "n-demo-b3-n", ("n-demo-b1-n", "n-demo-b3-n"), "B3")])
# 手ごとの --expect
ex = []
rh.press = lambda C, role, label, expect=None, **k: ex.append(expect); rh.send_field = lambda C, role, label, t, enter=False, expect=None: ex.append(expect)
rh.ax_ids = lambda C: ["terminal:1"]; rh.find = lambda *a, **k: {"element_token": "t"}
rh.db_state = lambda C: {"head_sha": "a", "rev": 2, "rows": {}, "pages": [], "page_sha": {}, "d": {}}
rh.press_tab = lambda C, kind, expect=None: rh.press(C, "AXCheckBox", kind, expect=expect)  # b71: 種類で題を引く所は b71 の塊で測る（ここは手ごとの --expect）
C = rh.Ctx(r2, "X.app", "n")
for name, fn, row in (("add_terminal", rh.h_add_terminal, {}), ("goto_tab", rh.h_goto_tab, {"doing": "『Docs』"}), ("nl_edit", rh.h_nl_edit, {"doing": "『add a Tests page』"}), ("undo", rh.h_undo, {}), ("redo", rh.h_redo, {})):
    ex.clear(); run(fn, C, row); res["ex_" + name] = int(ex == ([None, "ax:*"] if name == "add_terminal" else ["ax:*"]))  # add_terminal は帯の ＋ の menu（expect 無し）→ add_term（UI-5a）
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF22
}
B54RC=$(b54r "$BG/rehearse.py")
check "rehearse b54 (fake run dirs): recount same oracle ok / nonce lost red / no oracle after unm / no restart unm / a lie red / a dead click red / nothing verified before unm; restart presses the entry (dev_env) before B3, data is a symlink to R1/data, repo stays R1's, B3 called, survives a 2nd restart; start rc 6 with the app up goes on to B3, with owner_active is driver down, without run.json is driver down; D1 tab order differs red / a tab missing from AX unm / TextEdit window changed red / B3 unmeasured stays unm / B3 unm + D1 red -> red; B3 without B1 unmeasured and calls nothing, with B1 presses the Excalidraw page from the DB (Tests) then reads n1 + n2 as B3; add_terminal / goto_tab / nl_edit / undo / redo pass --expect ax:*" "rc_same=ok rc_lost=red rc_blind=unm rc_none=unm rc_lies=red rc_dead=red rc_empty=unm entry=1 sym=1 repo=1 b3_called=1 again=2 rc6_up=ok1 rc6_owner=down nojson=down d1_order=red d1_overflow=unm d1_win=red d1_b3unm=unm d1_b3unm_win=red no_b1=unm no_b1_calls=0 b3_calls=1 ex_add_terminal=1 ex_goto_tab=1 ex_nl_edit=1 ex_undo=1 ex_redo=1" "$B54RC"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF23'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/rh-b54{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
out("a", mut('    if lost:\n        red.append(', '    if False:\n        red.append('))  # 減った kind を赤にしない
out("b", mut('    if blind:\n        raise', '    if False:\n        raise'))  # oracle が無いのを未測にしない
out("c", mut('    os.symlink(os.path.join(R1, "data"), d2)', '    shutil.copytree(os.path.join(R1, "data"), d2)'))  # symlink をやめて copy
out("d", mut('b1_flow(C, n2, [f"n-demo-b1-{C.nonce}", n2], "B3")', 'b1_flow(C, n2, [n2], "B3")'))  # n1 を見ない
out("e", mut('    if not os.path.exists(os.path.join(C.state["R1"], "oracle", "B1.excalidraw.after.json")):', '    if False:'))  # B1 の無い B3 を未測にしない
out("f", mut('press(C, "AXButton", "add_term", expect="ax:*")', 'press(C, "AXButton", "add_term")'))
out("g", mut('send_field(C, "AXTextField", "change", text, enter=True, expect="ax:*")', 'send_field(C, "AXTextField", "change", text, enter=True)'))
out("h", mut('    press(C, "AXCheckBox", page)  #', '    pass  #'))  # B3 が Excalidraw の Page へ移らない（r8 まで）
out("i", mut('red = [f"{k} {len(t[k])} 件" for k in ("lies", "dead_clicks") if t[k]]', 'red = []'))  # lies / dead_clicks を見ない
out("j", mut('    if not any(T.verified_kinds(before).values()):\n        raise Unmeasured(', '    if False:\n        raise Unmeasured('))  # 前に verified が無いのを空の緑にする
out("k", mut('    check_d1(C, w0)\n\n\ndef b3_check', '    pass\n\n\ndef b3_check'))  # D1 の窓を見ない
out("l", mut('    if tabs != want:', '    if False:'))  # D1 の tab の順を見ない
out("m", mut('raise next((e for e in errs if isinstance(e, RuntimeError)), errs[0])', 'raise errs[0]'))  # 先の未測が後の赤を隠す
out("n", mut('    if j.get("owner_active") or j.get("window_id") is None:', '    if rc == 6 or j.get("window_id") is None:'))  # r8 の形: rc 6 を全部 driver down に読む
out("o", mut('    if sorted(tabs) != sorted(want):', '    if False:'))  # 揃わない tab を順の赤にする
out("p", mut('        j = {}  # start が run.json を書かずに落ちた = app が居ない', '        raise'))  # run.json が無いと run ごと落ちる
out("q", mut('press_project(C)  # 入口', 'pass  # 入口'))  # r9 の形: 入口を押すのは B3 だけ
EOF23
flip54r() {
  python3 -I -c 'import sys;a=dict(x.split("=") for x in sys.argv[1].split());b=dict(x.split("=") for x in sys.argv[2].split());print(",".join(k for k in a if a[k]!=b[k]) or "none")' "$B54RC" "$(b54r "$BG/rh-b54$1.$$.py")"
}
check "rehearse b54 negative controls: lost kind not red -> rc_lost; blind not unmeasured -> rc_blind; copy instead of symlink -> sym; n1 not read -> b3_calls; B3 without B1 -> no_b1,no_b1_calls; no expect on add_terminal -> ex_add_terminal; none on nl_edit -> ex_nl_edit; B3 stays on the entry page -> b3_calls; lies / dead_clicks unread -> rc_lies,rc_dead; nothing verified before reads ok -> rc_empty; D1 window unread -> d1_win,d1_b3unm_win; D1 tab order unread -> d1_order; first unmeasured hides a later red -> d1_b3unm_win; every start rc 6 read as driver down (r8) -> rc6_up; a missing tab read as an order red -> d1_overflow; no run.json fallback -> nojson; the entry not pressed before B3 (r9) -> entry" "rc_lost|rc_blind|sym|b3_calls|no_b1,no_b1_calls,b3_calls|ex_add_terminal|ex_nl_edit|b3_calls|rc_lies,rc_dead|rc_empty|d1_win,d1_b3unm_win|d1_order|d1_b3unm_win|rc6_up|d1_overflow|nojson|entry" "$(for m in a b c d e f g h i j k l m n o p q; do flip54r $m; done | paste -sd'|' -)"
rm -f "$BG"/rh-b54?.$$.py

# ---- W15-M1: 偽の CLI は scene 2 の一言（band session）の Project の計画にだけ答える。席 4 つ = REAPER・Discord・zoom.us・Claude・欄は src/composer/project.rs の plan_schema() の required と同じ・bundle id は Info.plist の実物・主デモの一言（"Set up a dev environment"）は plan_main（下の 102 通目の追加）・それ以外の一言は exit 1 ----
PS="$BG/../../../src/composer/project.rs"; FC="$BG/fake-claude.sh"
plan_band() {  # plan_band <fake-claude.sh> <一言> <schema の title> → "<rc> <席の app 名 bundle id…> <欄の不一致>"
  local d; d=$(mktemp -d "$T/fc.XXXX"); cp "$1" "$d/claude"; chmod +x "$d/claude"
  local out rc; out=$(printf '%s' "$2" | "$d/claude" -p --output-format json --json-schema "{\"title\":\"$3\"}" 2>/dev/null); rc=$?
  python3 -I - "$rc" "$PS" "$out" <<'EOF6'
import json, re, sys
rc, ps, out = sys.argv[1], sys.argv[2], sys.argv[3]
if rc != "0": print(rc); sys.exit()
src = open(ps, encoding="utf-8").read()
body = src[src.index("pub fn plan_schema()"):src.index("/// W13: 一言を model に読ませて")]
req = [json.loads("[" + m + "]") for m in re.findall(r'"required": \[([^\]]*)\]', body)]   # 順: 上位・席・instead の item
plan = json.loads(out)["structured_output"]
bad = sorted(set(req[0]) ^ set(plan)) + [k for seat in plan["seats"] for k in sorted(set(req[1]) ^ set(seat))]
print(rc, "|".join(f'{s["app_name"]}:{s["bundle_id"]}' for s in plan["seats"]), plan["shape"], ",".join(bad) or "fields-ok")
EOF6
}
BAND="Set up a band session: record in REAPER, keep our Discord and Zoom open, and Claude for lyric ideas. Put each on its own page."
IDS=$(for a in REAPER Discord zoom.us Claude; do /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "/Applications/$a.app/Contents/Info.plist" 2>/dev/null || echo none; done | paste -sd'|' -)
WANT_BAND="0 REAPER:com.cockos.reaper|Discord:com.hnc.Discord|zoom.us:us.zoom.xos|Claude:com.anthropic.claudefordesktop project fields-ok"
if [ "$IDS" = "com.cockos.reaper|com.hnc.Discord|us.zoom.xos|com.anthropic.claudefordesktop" ]; then
  check "M1: the 4 bundle ids in the fake answer are the ones in /Applications" "ok" "$([ "$(plan_band "$FC" "$BAND" shibubu_project_plan | cut -d' ' -f2)" = "$(echo "$WANT_BAND" | cut -d' ' -f2)" ] && echo ok || echo no)"
else echo "M1: unmeasured — an app of the 4 is not in /Applications ($IDS)" >&2; fi
check "M1: scene 2 sentence -> plan with 4 app seats whose fields equal plan_schema()'s required lists" "$WANT_BAND" "$(plan_band "$FC" "$BAND" shibubu_project_plan)"
check "M1: the main demo sentence (and the Interpret call of the band sentence) is not answered with a plan" "1 1" "$(plan_band "$FC" "Set up my dev environment" shibubu_project_plan | cut -d' ' -f1) $(plan_band "$FC" "$BAND" shibubu_edit | cut -d' ' -f1)"
# 負の対照: 席を 3 にする / 主デモでも plan を返す / 席の欄を 1 つ落とす
sed 's/^{"title":"Lyrics".*$/]}/; s/^{"title":"Call"\(.*\)},$/{"title":"Call"\1}/' "$FC" > "$T/fc-3seats.sh"
sed 's/^  \*shibubu_project_plan\*) if .* fi ;;$/  *shibubu_project_plan*) kind=plan_band ;;/' "$FC" > "$T/fc-always.sh"
sed 's/"file":null,"url":null,//' "$FC" > "$T/fc-nofield.sh"
check "M1 negative controls: 3 seats -> wrong; plan for every sentence -> main demo answered; a field dropped -> fields differ" "wrong|answered|differ" "$(
  [ "$(plan_band "$T/fc-3seats.sh" "$BAND" shibubu_project_plan)" = "$WANT_BAND" ] && echo same || echo wrong)|$(
  [ "$(plan_band "$T/fc-always.sh" "Set up my dev environment" shibubu_project_plan | cut -d' ' -f1)" = 1 ] && echo same || echo answered)|$(
  [ "$(plan_band "$T/fc-nofield.sh" "$BAND" shibubu_project_plan)" = "$WANT_BAND" ] && echo same || echo differ)"

# ---- W15-M3: rehearse に scene 2（band の一言 → 4 Page → REAPER の track・Discord の channel）の手。偽の画面・偽の読みで判定の枝を回す（実機を起こさない） ----
m3dry() { bash "$BG/rehearse.sh" scene2 --dry-run 2>&1 | tail -1; echo "rc=${PIPESTATUS[0]}"; }
check "M3: rehearse.sh scene2 --dry-run is green: 5 rows = 4 BG (no driver-less) + 1 FG, 5 pending lines (gate tree: hand 1's plan lines are chat consts); rehearse.sh bogus -> usage 2; the 27-step full.md dry-run is unchanged" "手 5 = 台本の行 5 · BG 4（driver 無し 0）· FG 1 · 文言 6 件が CSV と一致 · 未収録 5 件 · 目安 250 秒 rc=0|2|手 27 = 台本の行 27 · BG 22（driver 無し 0）· FG 5" "$(m3dry | tr '\n' ' ' | sed 's/ $//')|$(bash "$BG/rehearse.sh" bogus >/dev/null 2>&1; echo $?)|$(bash "$BG/rehearse.sh" 1 --dry-run 2>/dev/null | tail -1 | cut -d'·' -f1-3 | sed 's/ $//')"
cp "$BG/../../../docs/demo/scene2.md" "$T/scene2-bad.md"; sed -i.bak 's/^手の数: 5/手の数: 4/' "$T/scene2-bad.md"
sed "s|^SCENE2 = .*|SCENE2 = \"$T/scene2-bad.md\"|" "$BG/rehearse.py" > "$BG/rh-m3z.$$.py"
check "M3 negative control: the script declares 手の数 4 but has 5 rows -> dry-run red (nonzero)" "1" "$(python3 -I "$BG/rh-m3z.$$.py" scene2 --dry-run >/dev/null 2>&1; [ $? -ne 0 ] && echo 1 || echo 0)"
rm -f "$BG/rh-m3z.$$.py"
m3() {  # m3 <rehearse.py> -> "k=v ..."（ok / red / unm）
  python3 -I - "$1" "$T/m3" "$BG" <<'EOFM3'
import importlib.util, json, os, shutil, sys, time, types
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
base = sys.argv[2]; shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
clk = [time.time()]
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + s))
rows, _ = rh.parse(rh.SCENE2); R2, R3, R4 = rows[1], rows[2], rows[3]
DONE = rh.L.names("done_checked")[0]; ask2_real = rh.ask2; close_about_real = rh.close_about
cnt = [0]
def mk():
    cnt[0] += 1; R = f"{base}/r{cnt[0]}"; os.makedirs(R + "/fake")
    return rh.Ctx(R, "X.app", "n1")
def out(fn, C, row):
    try:
        fn(C, row); return "ok"
    except rh.Unmeasured:
        return "unm"
    except RuntimeError:
        return "red"
def E(n): return [{"role": "AXGroup", "label": "Arrange"}] + [{"role": "AXButton", "label": f"Track number: {i}"} for i in range(1, n + 1)]
def trk(n0, n1, hit, owned=True, empty=False):
    C = mk(); calls = [0]
    rh.app_pid = lambda C_, name: (5, owned)
    def els(C_, pid, depth=30):
        calls[0] += 1
        return [] if empty else E(n0 if calls[0] == 1 else n1)
    rh.app_els = els; rh.wait_seen = lambda *a, **k: (hit, "x"); rh.ask2 = lambda *a, **k: "Approval needed"; rh.close_about = lambda C_, pid: 0
    return out(rh.h_app_track, C, R3)
def W(i, title, sub="AXStandardWindow", mov=True, main=False, mini=False): return {"window_id": i, "title": title, "subrole": sub, "movable": mov, "main": main, "minimized": mini, "x": 10.0 * i, "y": 5.0, "w": 400.0, "h": 300.0}
def about(wins):  # wins = 閉じる前の窓（役割の属性つき）。About の close ボタンが効くと消える偽の窓の木 -> "閉じ切れず残った枚数:押した window_id" か（主窓が決まらず何も閉じない）"unm:押した window_id"
    C = mk(); live = list(wins); clicked = []
    rh.ax_windows = lambda pid: [dict(w) for w in live]
    rh.app_windows = lambda C_, pid: [{"window_id": w["window_id"], "pid": pid, "title": w["title"], "bounds": {"x": w["x"], "y": w["y"], "width": w["w"], "height": w["h"]}} for w in live]
    def stp(C_, tool, args, *extra, own=False):
        if tool == "get_window_state": return {"elements": [{"role": "AXButton", "subrole": "AXCloseButton", "element_token": f"t{args['window_id']}"}]}
        if tool == "click":
            i = int(args["element_token"][1:]); clicked.append(i); live[:] = [w for w in live if w["window_id"] != i]
        return {}
    rh.step = stp; rh.time = types.SimpleNamespace(time=lambda: 0, sleep=lambda s: None)
    rh.close_about = close_about_real
    try:
        left = rh.close_about(C, 5)
    except rh.Unmeasured:
        left = "unm"
    return f"{left}:{','.join(map(str, clicked))}"
def chn(t0, t1, hit):
    os.environ["SOLO_DISCORD"] = "1"
    C = mk(); calls = [0]
    rh.app_pid = lambda C_, name: (7, False)
    def tt(C_, pid):
        calls[0] += 1
        return t0 if calls[0] == 1 else t1
    rh.titles = tt; rh.wait_seen = lambda *a, **k: (hit, "x"); rh.ask2 = lambda *a, **k: "Approval needed"
    return out(rh.h_app_channel, C, R4)
def solo_off():  # SOLO_DISCORD 無し: 手は Discord を探さず押さず未測・段取りは Discord を open しない -> "手の結果:app_pid の呼び出し:ask2 の呼び出し:open した app"。SOLO_DISCORD=1 なら Discord も open
    os.environ.pop("SOLO_DISCORD", None); C = mk(); n = [0, 0]
    rh.app_pid = lambda C_, name: (n.__setitem__(0, n[0] + 1) or (7, False)); rh.ask2 = lambda *a, **k: n.__setitem__(1, n[1] + 1) or "x"
    rh.titles = lambda C_, pid: []
    res = out(rh.h_app_channel, C, R4)
    return f"{res}:{n[0]}:{n[1]}"
def opens(solo):
    if solo: os.environ["SOLO_DISCORD"] = "1"
    else: os.environ.pop("SOLO_DISCORD", None)
    C = mk(); got = []; clk[0] = time.time()  # about() が時計を止めた（time=0）ので進む時計に戻す: open_apps の待ち loop が終わる
    rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + s))
    rh.pids_of = lambda name: set(); rh.subprocess = types.SimpleNamespace(run=lambda cmd, **k: got.append(cmd[4]))
    rh.open_apps(C)
    return "+".join(got)
def mir(frames):
    C = mk(); rh.press = lambda *a, **k: None; rh.try_press = lambda *a, **k: False; rh.seen = lambda *a, **k: None
    rh.elements = lambda C_: [{"role": "AXImage", "identifier": "surface-frame:" + a.lower(), "label": f"● {a} · {st} · Live view"} for a, st in frames]
    return out(rh.h_mirror_apps, C, R2)
four = [("REAPER", "Mirroring"), ("Discord", "Mirroring"), ("zoom.us", "Mirroring"), ("Claude", "Mirroring")]
r = dict(
    trk_ok=trk(0, 1, DONE), trk_ok_noline=trk(0, 1, None), trk_lie=trk(0, 0, DONE), trk_two=trk(0, 2, DONE), trk_none=trk(0, 0, None),
    trk_empty=trk(0, 0, DONE, empty=True), trk_notowned=trk(0, 1, DONE, owned=False),
    chn_ok=chn(["Friends - Discord"], ["#general | Srv - Discord"], DONE), chn_lie=chn(["Friends - Discord"], ["Friends - Discord"], DONE), chn_none=chn(["Friends - Discord"], ["Friends - Discord"], None),
    chn_blank=chn([""], [""], DONE), chn_already=chn(["#general | Srv"], ["#general | Srv"], DONE),
    mir_ok=mir(four), mir_missing=mir(four[:3]), mir_notrunning=mir(four[:3] + [("Claude", "Not running")]))
# ask2: 承認の 1 行まで。押す先の 1 行は labels の status が pending の間は照合せず、en が入ったら無ければ赤
def gate(hit, line_seen, en=False):
    C = mk(); rh.press = lambda *a, **k: None; rh.try_press = lambda *a, **k: False; rh.ask2 = ask2_real; rh.send_field = lambda *a, **k: None
    rh.wait_seen = lambda *a, **k: (hit, "x"); rh.seen = lambda C_, needle, role=None: needle if line_seen else None
    old = rh.L.LABELS["press_track"]
    if en: rh.L.LABELS["press_track"] = (old[0], old[1], ("Press Track › Insert new track in REAPER",), None, "en")
    try:
        res = out(lambda C_, row: rh.ask2(C_, "Record", "Add a new track", ("w", "s", "desktop.ax.menu", {"pid": 5}), "press_track"), C, None)
    finally:
        rh.L.LABELS["press_track"] = old
    fk = json.load(open(C.R + "/fake/generate.json"))["workflow_steps"][0]["capability"]
    return res + ":" + fk
r.update(gate_pending=gate("Approval needed", False), gate_en_missing=gate("Approval needed", False, True), gate_en_present=gate("Approval needed", True, True), gate_refused=gate("so nothing was sent", False), gate_none=gate(None, False))
# 題名は 2 枚とも `REAPER v…`（題名の規則では About も主窓になる偽の木）・About は標準の窓でない
r.update(abt_one=about([W(0, "REAPER v7.81 - EVALUATION LICENSE", main=True), W(1, "REAPER v7.81 - About", sub="AXDialog", mov=False)]), abt_none=about([W(0, "REAPER v7.81", main=True)]),
         abt_pairmain=about([W(0, "A", main=True), W(1, "B")]), abt_pair=about([W(0, "A"), W(1, "B")]), abt_mini=about([W(0, "REAPER v7.81", mini=True)]))
r.update(solo_off=solo_off(), open_off=opens(False), open_on=opens(True))
print(*(f"{k}={v}" for k, v in r.items()))
EOFM3
}
rm -rf "$T/m3"; M3=$(m3 "$BG/rehearse.py")
pickm() { tr ' ' '\n' <<< "$1" | grep "^$2=" | cut -d= -f2; }
check "M3 track (REAPER): +1 ok (also when the product line is missing); product says Done but 0 -> red; +2 -> red; no line and 0 -> unm; unreadable tree -> unm; owner's REAPER (not mine) -> unm" "ok ok red red unm unm unm" "$(for k in trk_ok trk_ok_noline trk_lie trk_two trk_none trk_empty trk_notowned; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
check "M3 channel (Discord): title gains general -> ok; Done but no general -> red; no line and none -> unm; blank titles (no screen-recording permission) -> unm; general already open -> unm" "ok red unm unm unm" "$(for k in chn_ok chn_lie chn_none chn_blank chn_already; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
check "M3 About (REAPER eval, main picked by role like the product's pick_by_role): About + main -> About closed (0 left, only window 1 clicked); main only -> nothing clicked; two standard windows, one AXMain -> the other closed; two standard windows, none AXMain -> unmeasured, nothing closed; only a minimised window -> unmeasured" "0:1 0: 0:1 unm: unm:" "$(for k in abt_one abt_none abt_pairmain abt_pair abt_mini; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
check "M3 Discord scope (owner 10-26: only a server with the owner alone): no SOLO_DISCORD -> channel step unmeasured, Discord not looked up, nothing sent; setup does not open Discord (3 apps); SOLO_DISCORD=1 -> all 4 opened" "unm:0:0 REAPER+zoom.us+Claude REAPER+Discord+zoom.us+Claude" "$(for k in solo_off open_off open_on; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
check "M3 mirrors: 4 frames saying Mirroring -> ok; Lyrics missing -> unm; Claude 'Not running' -> unm" "ok unm unm" "$(for k in mir_ok mir_missing mir_notrunning; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
check "M3 approval line: key pending -> not checked (ok) and the fake generate has the menu step; en filled + line missing -> red; en filled + present -> ok; refusal line -> unm; no line -> unm" "ok:desktop.ax.menu red:desktop.ax.menu ok:desktop.ax.menu unm:desktop.ax.menu unm:desktop.ax.menu" "$(for k in gate_pending gate_en_missing gate_en_present gate_refused gate_none; do pickm "$M3" $k; done | tr '\n' ' ' | sed 's/ $//')"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOFM3M'
import sys
s = open(sys.argv[1], encoding="utf-8").read(); out = lambda n, t: open(f"{sys.argv[2]}/rh-m3{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
def mut(old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
out("a", mut("    if n1 == n0 + 1:\n        return", "    if n1 > n0:\n        return"))  # 「ちょうど 1 増えた」を「増えた」に
out("b", mut("    if not els:\n        raise Unmeasured(\"REAPER の AX", "    if False:\n        raise Unmeasured(\"REAPER の AX"))  # 木が空の門を外す
out("c", mut("    if not any(t0):", "    if False:"))  # 題名が空の門を外す
out("d", mut("any(\"Mirroring\" in (e.get(k) or \"\") and app in", "any(app in"))  # Mirroring の語を見ない
out("e", mut("    if want and not any(seen(C, w) for w in want):", "    if False:"))  # 押す先の 1 行の照合を外す
out("f", mut("    if not owned:", "    if False:"))  # owner の REAPER を触らない門を外す
out("g", mut("    if any(CHANNEL in t.lower() for t in t0):", "    if False:"))  # 前から general が開いている門を外す
out("h", mut("            if btn:\n                step(C, \"click\", {\"element_token\": btn[\"element_token\"]})", "            if False:\n                pass"))  # About の close を押さない
out("i", mut("    main = [w for w in (work or open_) if w.get(\"main\") is True]", "    main = []"))  # main の段を外す -> 同じ役割の 2 枚から決まらない
out("j", mut("    return next((g[0] for g in (open_, work, main) if len(g) == 1), None)", "    t = [w for w in wins if (w.get(\"title\") or \"\").startswith(\"REAPER v\")]\n    return t[0] if len(t) == 1 else None"))  # 題名の規則に戻す
out("l", mut("    if not solo_discord():\n        raise Unmeasured(\"owner", "    if False:\n        raise Unmeasured(\"owner"))  # 手の門を外す
out("m", mut("        if not (name == \"Discord\" and not solo_discord()):", "        if True:"))  # 段取りの門を外す
out("k", mut("    open_ = [w for w in wins if w.get(\"minimized\") is not True]\n    work", "    open_ = list(wins)\n    work"))  # 最小化を除かない
EOFM3M
for x in a b c d e f g h i j k l m; do eval "M3$x=\$(m3 \"\$BG/rh-m3$x.\$\$.py\")"; done
check "M3 negative controls: '+1' loosened to '>0' -> +2 reads ok; blank-tree gate removed -> red; blank-title gate removed -> red; Mirroring not read -> 'Not running' reads ok; approval-line check removed -> missing line reads ok; owner-REAPER gate removed -> ok; already-general gate removed -> ok; About close not pressed -> 1 left; main stage removed -> two standard windows undecided; title rule instead of role -> About (titled REAPER v) is also main; minimised not excluded -> a minimised-only app reads as having a main; channel-step scope gate removed -> Discord is looked up without SOLO_DISCORD; setup scope gate removed -> Discord is opened" "ok red red ok ok ok ok 1: unm: unm: 0: unm:1:0 REAPER+Discord+zoom.us+Claude" "$(pickm "$M3a" trk_two) $(pickm "$M3b" trk_empty) $(pickm "$M3c" chn_blank) $(pickm "$M3d" mir_notrunning) $(pickm "$M3e" gate_en_missing | cut -d: -f1) $(pickm "$M3f" trk_notowned) $(pickm "$M3g" chn_already) $(pickm "$M3h" abt_one | sed 's/^0:/0:/') $(pickm "$M3i" abt_pairmain) $(pickm "$M3j" abt_one) $(pickm "$M3k" abt_mini) $(pickm "$M3l" solo_off) $(pickm "$M3m" open_off)"
rm -f "$BG"/rh-m3?.$$.py

# ---- W15-b61: INTEG-UI-b の .app（trunk 9f60c76・UI-4a〜6）の AX の名前に 27 手の driver を合わせる ----
# 実測の木 = Home（demo-loop r11 の 02-get_window_state・fixture home-ui4a.json）: 一言は header の AXTextField 'What to create'・primary は AXButton 'Create'（古い .app は AXTextArea の欄）。
# Project の木は src から（合成・shell.rs:70 の欄の名前 `Change this project`・header.rs:119 の右端の button `↵ Change` の AX の名前 `Change`・Page の中の `Send changes` は消えた〈generated.rs:1614〉）。
# 古い .app の Project の木（欄 + `Send changes`）は fixture に残し、send_edit は両方の名前で引ける。UI-4b: 窓の幅 < 1200 は sidebar が畳んで始まる → 入口 dev_env が木に無い時だけ ◧（`Show sidebar`）を 1 回押す。
b61() {  # b61 <rehearse.py> -> "<k>=<v> …"（goal_* = h_generate が一言の欄に使う role と label で引いた token・rs_* = 再起動の入口を押した token の列）
  python3 -I - "$1" "$LF/home-ui4a.json" "$LF/tree-en.json" "$T/b61" <<'EOF61'
import importlib.util, json, os, shutil, sys, time, types
rhp, home_p, old_p, base = sys.argv[1:5]
spec = importlib.util.spec_from_file_location("rh", rhp); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, os.path.dirname(rhp)); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
rh.time = types.SimpleNamespace(time=time.time, sleep=lambda s: None)
E = lambda role, label, tok: {"role": role, "label": label, "element_token": tok, "enabled": True}
HOME = json.load(open(home_p))["elements"]
OLD = json.load(open(old_p))["elements"]
PROJ = [E("AXButton", "Hide sidebar", "t-hide"), E("AXTextField", "Change this project", "t-field"), E("AXButton", "Change", "t-send"), E("AXButton", "•••", "t-dots"),
        E("AXCheckBox", "Dev environment", "t-entry"), E("AXButton", "Undo · ⌘Z", "t-undo"), E("AXButton", "Page options", "t-opts"), E("AXCheckBox", "Dev", "t-dev")]
OLDPROJ = [E("AXTextField", "Change this project", "t-oldfield"), E("AXButton", "Send changes", "t-oldsend")]
def got(els, role, key):
    rh.elements = lambda C: els
    try:
        return rh.find(None, role, key, tries=1)["element_token"]
    except rh.Unmeasured:
        return "unm"
res = {}
class Stop(Exception): pass
seen = []
def cap(C, role, label, text, enter=False, expect=None):
    seen.append((role, label)); raise Stop()
real_send, rh.send_field = rh.send_field, cap
try:
    rh.h_generate(rh.Ctx(base, "X.app", "n"), {})
except Stop:
    pass
role, label = seen[0]
res["goal_new"], res["goal_old"] = got(HOME, role, label), got(OLD, role, label)
res["create"] = got(HOME, "AXButton", "create")
res["change_field"], res["send_edit"], res["send_old"] = got(PROJ, "AXTextField", "change"), got(PROJ, "AXButton", "send_edit"), got(OLDPROJ, "AXButton", "send_edit")
n = [0]
def restart(folded, button=True):
    n[0] += 1; R1 = f"{base}/rs{n[0]}"
    for x in ("data", "shibubu-yc-demo", "fake"): os.makedirs(f"{R1}/{x}", exist_ok=True)
    shutil.rmtree(R1 + "-restart", ignore_errors=True)
    st, clicks = {"open": not folded}, []
    def fake_run(*a, ws=None, fake=None):
        if a[0] == "start": json.dump({"window_id": 7}, open(a[1] + "/run.json", "w"))
        return 0, ""
    def els(C):
        toggle = [E("AXButton", "Hide sidebar" if st["open"] else "Show sidebar", "t-toggle")] if button or st["open"] else []
        return toggle + ([E("AXCheckBox", "Dev environment", "t-entry")] if st["open"] else [])
    def step(C, tool, args, *extra, own=False):
        clicks.append(args.get("element_token"))
        if args.get("element_token") == "t-toggle": st["open"] = True
        return {}
    rh.run_sh, rh.elements, rh.step = fake_run, els, step
    rh.db_state = lambda C: {"d": {"name": "Dev environment"}}  # b71: 入口は DB の Project の title
    rh.te_windows = lambda C: [(7, 0, 0, 500, 400)]; rh.b3_check = lambda C: None; rh.check_d1_restart = lambda C, w0: None
    try:
        rh.h_restart(rh.Ctx(R1, "X.app", "n"), {})
    except rh.Unmeasured:
        return "unm"
    return ",".join(clicks)
res["rs_open"], res["rs_folded"], res["rs_nobutton"] = restart(False), restart(True), restart(True, button=False)
# UI-4a: 灰色（busy）の欄には打たない: 数回灰色の後に有効 → 打つ・ずっと灰色 → 打たずに未測・最初から有効 → 打つ
def sf(seq):
    reads, did = iter(seq), []
    rh.elements = lambda C: [{**E("AXTextField", "What to create", "t-f"), "enabled": next(reads, seq[-1])}]
    rh.step = lambda C, tool, args, *extra, own=False: did.append(tool) or {}
    try:
        real_send(None, "AXTextField", "goal", "hi")
    except rh.Unmeasured:
        return "unm"
    return "typed" if "type_text" in did else "none"
res["en_wait"], res["en_never"], res["en_ready"] = sf([False, False, True]), sf([False]), sf([True])
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF61
}
mkdir -p "$T/b61"
B61=$(b61 "$BG/rehearse.py")
check "b61 UI-4a/4b names (trunk 9f60c76): the goal field is looked up as AXTextField 'What to create' in the real Home tree (token t-4) and no longer in the old AXTextArea tree; Create found; the Project's send is the header button 'Change' (not the field), the old 'Send changes' still found; restart presses the entry, opens the folded sidebar first (◧ 'Show sidebar') when the entry is missing, and is unmeasured when there is no such button; a greyed (busy) field is waited on before typing, never typed into, unmeasured if it stays grey" \
  "goal_new=t-4 goal_old=unm create=t-5 change_field=t-field send_edit=t-send send_old=t-oldsend rs_open=t-entry rs_folded=t-toggle,t-entry rs_nobutton=unm en_wait=typed en_never=unm en_ready=typed" "$B61"
python3 -I - "$BG" "$$" <<'EOF62'
import sys
bg, pid = sys.argv[1:3]
rs, ls = open(f"{bg}/rehearse.py", encoding="utf-8").read(), open(f"{bg}/labels.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"rh-b61a.{pid}.py", mut(rs, 'send_field(C, L.LABELS["goal"][0], "goal", said)', 'send_field(C, "AXTextArea", "goal", said)'))  # 古い .app の role に戻す
w(f"rh-b61e.{pid}.py", mut(rs, "    f = wait_enabled(C, role, label)\n", "    f = find(C, role, label)\n"))  # 灰色の欄にも打つ
w(f"rh-b61d.{pid}.py", mut(rs, '        if not try_press(C, "AXButton", "Show sidebar"):\n            raise', '        if True:\n            raise'))  # 畳んだ sidebar を開かない
w(f"lb-b61b.{pid}.py", mut(ls, '("Change", "Send changes")', '("Send changes",)'))  # send_edit が古い名前だけ
w(f"lb-b61c.{pid}.py", mut(ls, '"goal":      ("AXTextField",', '"goal":      ("AXTextArea", '))  # 表の role が古いまま
EOF62
b61m() {  # b61m <mutated labels.py> -> b61 の結果（mutated labels と rehearse を別の dir に置いて import させる）
  local d; d=$(mktemp -d "$T/b61m.XXXX"); cp "$BG"/*.py "$d/"; cp "$1" "$d/labels.py"; b61 "$d/rehearse.py"
}
flip61() {  # flip61 <b61 の結果> -> 正と違った key
  python3 -I -c 'import sys;a=dict(x.split("=") for x in sys.argv[1].split());b=dict(x.split("=") for x in sys.argv[2].split());print(",".join(k for k in a if a[k]!=b[k]) or "none")' "$B61" "$1"
}
f61a=$(flip61 "$(b61 "$BG/rh-b61a.$$.py")"); f61b=$(flip61 "$(b61m "$BG/lb-b61b.$$.py")"); f61c=$(flip61 "$(b61m "$BG/lb-b61c.$$.py")"); f61d=$(flip61 "$(b61 "$BG/rh-b61d.$$.py")"); f61e=$(flip61 "$(b61 "$BG/rh-b61e.$$.py")")
rm -f "$BG"/rh-b61?.$$.py "$BG"/lb-b61?.$$.py
check "b61 negative controls: the goal role hard-coded back to AXTextArea in rehearse -> goal_new,goal_old; send_edit without 'Change' in the table -> send_edit; the table's goal role back to AXTextArea -> goal_new,goal_old; the folded-sidebar fallback removed -> rs_folded; the grey-field wait removed -> en_never" "goal_new,goal_old|send_edit|goal_new,goal_old|rs_folded|en_never" "$f61a|$f61b|$f61c|$f61d|$f61e"

# ---- W15-b62: owner が pointer を動かしただけの cursor の赤は未測（pointer を使わない手だけ・run は続く）----
# s62 <run.sh> <tally.py の dir> <tool> <mouse_idle_s|none> <after.front.pid> -> "<judge_step の rc>/<violations 数>/<未測 front>/<tally.owner_returned が文を返したか>"
s62() {
  local R; R=$(mktemp -d "$T/b62.XXXX"); mkdir -p "$R/steps"
  python3 -I - "$R" "$3" "$4" "$5" "${6:--}" <<'EOF61'
import json, sys
R, tool, idle, front, cua = sys.argv[1:6]
def win(i, pid, z): return {"window_id": i, "pid": pid, "layer": 0, "bounds": {"x": 0, "y": 0, "w": 400, "h": 300}, "onscreen": True, "z": z}
def snap(t, x, fpid): return {"schema": "shibubu.qa.snapshot/1", "t": t, "front": {"pid": fpid, "name": "Owner"}, "cursor": {"x": x, "y": 10},
  "focus": {"pid": 100, "role": "AXTextArea", "frame": {"x": 0, "y": 0, "w": 10, "h": 10}}, "windows": [win(1, 100, 1)], "unmeasured": {}}
json.dump({"owner_pid": 100, "suspects": [200]}, open(R + "/run.json", "w"))
json.dump(snap(10.0, 10, 100), open(R + "/steps/s.before.json", "w")); json.dump(snap(12.0, 80, int(front)), open(R + "/steps/s.after.json", "w"))  # cursor が 70pt 動いた
ev = [{"schema": "shibubu.qa.event/1", "t": 9.0, "kind": "start"}, {"schema": "shibubu.qa.event/1", "t": 12.2, "kind": "z_sample", "windows": []}]
open(R + "/events.jsonl", "w").write("".join(json.dumps(e) + "\n" for e in ev))
pr = {"hid_idle_s": 0.2, "key_idle_s": 900.0}
if idle != "none": pr["mouse_idle_s"] = float(idle)
if cua != "-": open(R + "/steps/s.cua.json", "w").write(cua)  # b65: click / scroll は cua.json の route を読む
json.dump({"schema": "shibubu.qa.axprobe/1", "t": 11.0, "pressure": pr}, open(R + "/steps/s.axprobe.json", "w"))  # probe は手の中（before + 1 秒）
EOF61
  [ -f "${7:-}" ] && cp "$7" "$R/steps/s.cursor.jsonl"; echo "$R" > "$T/last62"  # b68: cursor の poll の行（任意）と、後で verdict を読む run dir
  local rc; rc=$( ( source "$1"; cmd=judge_step; judge_step "$R" s "$R/events.jsonl" "$3" '{}' 0 "" "" "" >/dev/null 2>&1; echo $? ) | tail -1 )
  python3 -I - "$R" "$2" "$rc" <<'EOF61B'
import json, sys
R, td, rc = sys.argv[1:4]
sys.path.insert(0, td); import tally
v = json.load(open(R + "/steps/s.verdict.json"))
print(f'{rc}/{len(v["violations"])}/{(v.get("unmeasured") or {}).get("front", "-")}/{tally.owner_returned(v, {"t0": 10.0, "t1": 12.0, "owner_idle_s": 900.0}, None) is not None}')
EOF61B
}
# z62 <run.sh> <step の front 未測 owner_pointer を置くか 0|1> -> run 全体の verdict（cursor だけの violation）が書き換わったか
z62() {
  local R; R=$(mktemp -d "$T/b62z.XXXX"); mkdir -p "$R/steps"
  echo '{"ok":false,"measured":["front"],"unmeasured":{},"violations":[{"detection":"front","detail":"cursor (10,10) -> (80,10)（owner の手でも赤）","t":12}]}' > "$R/steps/zz-run.verdict.json"
  [ "$2" = 1 ] && echo '{"ok":false,"violations":[],"unmeasured":{"front":"owner_pointer"}}' > "$R/steps/s.verdict.json"
  ( source "$1"; owner_pointer "$R/steps/zz-run.verdict.json" run "" "$R" zz-run 0 ); echo "$? $(python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(len(d["violations"]),d["unmeasured"].get("front","-"))' "$R/steps/zz-run.verdict.json")"
}
S62C=$(for a in "type_text 0.2 100" "click 0.2 100" "type_text 500 100" "type_text 0.2 300" "get_window_state none 100" "scroll 0.2 100"; do set -- $a; s62 "$BG/run.sh" "$BG" "$1" "$2" "$3"; done | paste -sd' ' -)
check "b62 owner pointer: a cursor-only red on a pointer-free step with a pointer input in the step window (r13 #14 shape: type_text, mouse_idle 0.2 < 1 s) -> rc 6, no violation, unmeasured front=owner_pointer, tally owner_returned fires; click / scroll, mouse idle 500, a front change mixed in, no mouse_idle_s -> rc 1 red" \
  "6/0/owner_pointer/True 1/1/-/False 1/1/-/False 1/2/-/False 1/1/-/False 1/1/-/False" "$S62C"
Z62C="$(z62 "$BG/run.sh" 1) | $(z62 "$BG/run.sh" 0)"
check "b62 owner pointer at stop: zz-run cursor-only violation is rewritten to unmeasured only when a step was owner_pointer; with no such step it stays a violation" "0 0 owner_pointer | 1 1 -" "$Z62C"
check "b62 stop calls owner_pointer on zz-run" "1" "$(grep -c 'owner_pointer "$R/steps/zz-run.verdict.json" run' "$BG/run.sh")"
python3 -I - "$BG" "$$" <<'EOF62X'
import sys
bg, pid = sys.argv[1:3]
rs, ts = open(f"{bg}/run.sh", encoding="utf-8").read(), open(f"{bg}/tally.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
call = '  [ $jrc -eq 1 ] && owner_pointer "$R/steps/$name.verdict.json" step "$tool" "$R" "$name" "$lo" && { jrc=6;'
w(f"run-b62a.{pid}.sh", mut(rs, call, '  [ $jrc -eq 1 ] && false && { jrc=6;'))  # 分ける処理を外す
w(f"run-b62b.{pid}.sh", mut(rs, '        if tool in ("click","scroll"):  # AX で押した手', '        if False:  # AX で押した手'))  # pointer を使う手も分ける
w(f"run-b62c.{pid}.sh", mut(rs, "or not m<t-float(lo): sys.exit(1)", "or False: sys.exit(1)"))  # pointer の入力を見ない
w(f"run-b62d.{pid}.sh", mut(rs, 'or not str(x.get("detail","")).startswith("cursor")', 'or False'))  # cursor 以外の front の赤も分ける
w(f"run-b62f.{pid}.sh", mut(rs, "        if not any(((json.load(open(f)).get(", "        if False and not any(((json.load(open(f)).get("))  # 手が owner_pointer でなくても zz-run を分ける
w(f"tl-b62.{pid}.py", mut(ts, 'if not vs and (verdict.get("unmeasured") or {}).get("front") == "owner_pointer":', 'if False:'))  # tally が読まない
EOF62X
m62() {  # m62 <run.sh> -> s62 の全部
  for a in "type_text 0.2 100" "click 0.2 100" "type_text 500 100" "type_text 0.2 300" "get_window_state none 100" "scroll 0.2 100"; do set -- $a; s62 "$BG/$M62" "$BG" "$1" "$2" "$3"; done | paste -sd' ' -
}
flip62() {  # flip62 <変異の結果> -> 正と違った scenario の番号（1 始まり）
  python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$S62C" "$1"
}
M62=run-b62a.$$.sh; f62a=$(flip62 "$(m62)")
M62=run-b62b.$$.sh; f62b=$(flip62 "$(m62)")
M62=run-b62c.$$.sh; f62c=$(flip62 "$(m62)")
M62=run-b62d.$$.sh; f62d=$(flip62 "$(m62)")
mkdir -p "$T/t62m"; cp "$BG"/*.py "$T/t62m/"; cp "$BG/tl-b62.$$.py" "$T/t62m/tally.py"
f62g=$(flip62 "$(for a in "type_text 0.2 100" "click 0.2 100" "type_text 500 100" "type_text 0.2 300" "get_window_state none 100" "scroll 0.2 100"; do set -- $a; s62 "$BG/run.sh" "$T/t62m" "$1" "$2" "$3"; done | paste -sd' ' -)")
f62f=$(z62 "$BG/run-b62f.$$.sh" 0)
rm -f "$BG"/run-b62?.$$.sh "$BG"/tl-b62.$$.py
check "b62 negative controls: separation removed -> 1; click not excluded -> 2,6; mouse idle not read -> 3; non-cursor front red also separated -> 4; tally ignoring the rewritten verdict -> 1 (owner_returned off); zz-run rewritten without an owner_pointer step -> rewritten (0 0 owner_pointer)" \
  "1|2,6|3|4|1|0 0 owner_pointer" "$f62a|$f62b|$f62c|$f62d|$f62g|$f62f"

# ---- W15-b63: #22 は docs の面がある Page（DB）の tab を押す・#20 は Approve and send の button で承認を決め、拒否の行は製品の赤 ----
# b63 <rehearse.py> -> "<#22: Diagram ok の押した tab> <DB 読めず> <Diagram の tab 無し> <Diagram の Mirror が止まった> | <#20: 承認> <拒否の行> <何も出ない> <lease に承認の行>"
b63() {
  python3 -I - "$1" "$T/b63" "$BG" <<'EOF63'
import importlib.util, json, os, shutil, sys, types
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
base = sys.argv[2]; shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
clk = [1000.0]
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + max(s, 1)))
rows, _ = rh.parse(rh.DEMO); r18, r20 = rows[17], rows[19]
cnt = [0]
def mk():
    cnt[0] += 1; R = f"{base}/r{cnt[0]}"; os.makedirs(R + "/data"); os.makedirs(R + "/shibubu-yc-demo")
    open(R + "/shibubu-yc-demo/README.md", "w").write("# old\n"); open(R + "/data/gui-acts.jsonl", "w").close()
    json.dump({"pid": 1, "window_id": 2, "session": "s"}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); C.state["consents"] = 1; C.state["ai_target"] = (9, 9); return C
T = lambda role, label=None, value=None, tok=None: {k: v for k, v in dict(role=role, label=label, value=value, element_token=tok).items() if v}
LIVE = "Mirroring · TextEdit"
def m22(pages, tabs, diagram=LIVE):
    C = mk(); on = ["?"]; pressed = []
    els = [T("AXCheckBox", t, tok="t-" + t.lower()) for t in tabs]
    rh.elements = lambda C_: els
    rh.db_state = lambda C_: None if pages is None else {"d": {"surface_specs": [{"id": "docs", "backend": "mirror", "target": {"file": "README.md"}}, {"id": "notes", "backend": "mirror", "target": {"file": "docs/notes.md"}}],
        "pages": [{"title": t, "root": {"children": [{"component": "source_surface", "props": {"surface_ref": "docs" if t == pages else "notes" if t == "notes" else None}}] if t == pages or t == "notes" else []}} for t in ("Docs", "Diagram", "notes")]}}  # b71: 種類は surface_specs の target で決める
    def fstep(C_, tool, args, *a, **k):
        if tool == "click": on[0] = args["element_token"]; pressed.append(on[0])
        return {}
    rh.step = fstep
    rh.frames = lambda C_: {"t-notes": {"notes": LIVE}, "t-docs": {"docs": diagram}, "t-diagram": {"docs": diagram}}.get(on[0], {})
    rh.consents = lambda C_: 1
    try:
        rh.h_mirror_consent(C, {}); return "ok:" + ",".join(pressed)
    except rh.Unmeasured: return "unm"
    except RuntimeError: return "red"
def m20(row, rows_after, lease=False):
    C = mk(); pressed = []; state = {"sent": False}
    rh.front_name = lambda: "TextEdit"
    rh.send_field = lambda *a, **k: None
    def fpress(C_, role, label, **k):
        pressed.append(label)
        if label == "send_edit": state["sent"] = True
    rh.press = fpress; rh.try_press = lambda C_, role, label: pressed.append("try:" + label) or True
    rh.elements = lambda C_: list(rows_after) if state["sent"] else [T("AXTextField", "Change this project")]
    def stop(*a, **k): raise rh.Unmeasured("after-approve")
    rh.wait_seen = stop
    try:
        rh.h_ask_ai(C, row); return "ok"
    except rh.Unmeasured as e: return "unm:" + ("approved" if "approve" in pressed else "cancelled" if "人の lease が無く" in str(e) else "none" if "承認の 1 行が出ない" in str(e) else "other")
    except RuntimeError as e: return "red" + (":approved" if "approve" in pressed else "") + (":denied" if "宛先" in str(e) or "F2" in str(e) else "")
APPROVE = [T("AXStaticText", value="▣ Needs approval · Approval needed before sending"), T("AXStaticText", value="To do X, Shibubu will send 2 steps: Send “Shibubu Demo” to TextEdit"), T("AXButton", "Approve and send", tok="t-a"), T("AXButton", "Cancel", tok="t-c")]
DENIED = [T("AXStaticText", value="▣ Needs approval"), T("AXStaticText", value="3 windows of TextEdit are open and Shibubu can't tell which. Bring the one you want to the front, then ask again")]
print(m22("Diagram", ("notes", "Diagram", "Docs")), m22(None, ("notes", "Diagram")), m22("Diagram", ("notes", "Docs")), m22("Diagram", ("notes", "Diagram"), diagram="Not running · TextEdit"), "|",
      m20(r20, APPROVE), m20(r20, DENIED), m20(r20, []), m20(r18, APPROVE))
EOF63
}
B63=$(b63 "$BG/rehearse.py")
check "b63 #22 presses the Page that holds the docs surface (Diagram after #14), not a fixed 'Docs' tab: ok pressing t-notes,t-diagram,t-notes / DB unreadable unm / no tab for that Page unm / its Mirror stopped red; #20 decides approval by the Approve and send button: approve pressed / a refusal row (also carrying 'Needs approval') is a product red with the text, nothing pressed / nothing shown unm / lease run seeing an approval row presses Approve (B68: the product reads the front after Approve and refuses), then waits" \
  "ok:t-notes,t-diagram,t-notes unm unm red | unm:approved red:denied unm:none unm:approved" "$B63"
python3 -I - "$BG" "$$" <<'EOF63M'
import sys
bg, pid = sys.argv[1:3]
rs = open(f"{bg}/rehearse.py", encoding="utf-8").read()
def mut(old, new):
    assert rs.count(old) == 1, old
    return rs.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"rh-b63a.{pid}.py", mut('press_tab(C, "docs", expect="ax:*")  # docs の面は', 'press(C, "AXCheckBox", "Docs", expect="ax:*")  # docs の面は'))  # 固定の Docs の tab に戻す
w(f"rh-b63b.{pid}.py", mut('if any(e.get("role") == "AXButton" and not e.get("md") and L.hit(e, "approve") for e in els):', 'if seen(C, "approval", None, els) or seen(C, "Approval", None, els):'))  # 語だけで承認と読む
w(f"rh-b63c.{pid}.py", mut('raise RuntimeError("宛先を決められずに拒否（製品・G73）: "', 'raise Unmeasured("宛先を決められずに拒否（製品・G73）: "'))  # 拒否の行を未測にする
EOF63M
flip63() {  # flip63 <変異の結果> -> 正と違った位置（1 始まり・| は数えない）
  python3 -I -c 'import sys;a=sys.argv[1].replace("|","").split();b=sys.argv[2].replace("|","").split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$B63" "$1"
}
f63a=$(flip63 "$(b63 "$BG/rh-b63a.$$.py")"); f63b=$(flip63 "$(b63 "$BG/rh-b63b.$$.py")"); f63c=$(flip63 "$(b63 "$BG/rh-b63c.$$.py")")
rm -f "$BG"/rh-b63?.$$.py
check "b63 negative controls: fixed 'Docs' tab -> #22 case 1 (and the Diagram-stopped case 4) change (1, 3, 4); approval read by the word -> the refusal row is taken as an approval (#20 case 2); refusal row made unmeasured -> #20 case 2" "1,3,4|6|6" "$f63a|$f63b|$f63c"

# ---- W15-b64: target-id.sh の表の判定（4 場面 × 赤 / 未測 / ok）----
# ti <target_id.py> -> 12 場面の判定を | 区切り + 道具の形（exit 6 / 1 / 0・表の行数・新しい原文）
ti() {
  python3 -I - "$1" "$BG" <<'EOF64'
import importlib.util, sys
sp = importlib.util.spec_from_file_location("ti", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sp.loader.exec_module(m)
S = lambda *a: m.judge(*a)[0]
cases = [("S1", {"README"}, True, False, ""), ("S1", {"README", "notes"}, True, False, ""), ("S2", set(), False, False, "README isn't open"), ("S2", {"notes"}, False, False, "README isn't open"),
         ("S2", set(), False, True, "README isn't open"), ("S3", set(), False, False, "2 windows can't tell which"), ("S3", {"other"}, False, False, ""), ("S3", {"README"}, True, False, ""),
         ("S4", {"notes"}, False, False, ""), ("S4", {"README"}, True, False, ""), ("S1", set(), True, False, "x"), ("S2", set(), False, False, "")]
print("|".join(S(*c) for c in cases))
rows = [dict(scene=s, status=st, why="", changed=[], readme_ok=False, verified=False, line="x", sec=1) for s, st in (("S1", "ok"), ("S2", "未測"), ("S3", "ok"), ("S4", "ok"))]
print(m.exit_code(rows), m.exit_code(rows[:1]), m.exit_code(rows[:1] + [dict(rows[0], status="赤")]), m.table(rows).count("| S"), m.new_lines(["a"], ["a", "b", "b", "c"]))
EOF64
}
TI=$(ti "$BG/target_id.py")
check "b64 target-id judge: S1 ok / extra file changed red / S2 untouched+README line ok / S2 notes changed red / S2 Verified red / S3 untouched ok / S3 other README changed red / S3 repo README only ok / S4 notes changed red / S4 README ok / S1 nothing changed red / S2 no product line red; exit 6 (unmeasured) / 0 / 1 (red); 4 table rows; new lines only" \
  "ok|red|ok|red|red|ok|red|ok|red|ok|red|red
6 0 1 4 ['b', 'c']" "$TI"
check "b64 target-id --dry-run prints the 4 scenes and starts nothing" "4" "$(bash "$BG/target-id.sh" --dry-run | grep -c '^| S')"
python3 -I - "$BG" "$$" <<'EOF64M'
import sys
bg, pid = sys.argv[1:3]
ts = open(f"{bg}/target_id.py", encoding="utf-8").read()
def mut(old, new):
    assert ts.count(old) == 1, old
    return ts.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"target_id.py.a{pid}", mut('    if changed - allowed:\n', '    if False:\n'))  # 名指しでない file の変化を見ない
w(f"target_id.py.b{pid}", mut('    if verified:\n        return "red", "どの file も変わっていないのに Verified"\n', ''))  # 何も変わらない Verified を見ない
w(f"target_id.py.c{pid}", mut('        if changed != {"README"}:\n            return "red", "README が変わっていない"\n', ''))  # README が変わらなくても通す
EOF64M
for x in a b c; do cp "$BG/target_id.py.$x$$" "$BG/ti-b64$x.$$.py"; done
flip64() {  # flip64 <変異の結果の 1 行目> -> 正と違った位置（1 始まり）
  python3 -I -c 'import sys;a=sys.argv[1].split("|");b=sys.argv[2].split("|");print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$(echo "$TI" | head -1)" "$(ti "$1" | head -1)"
}
f64a=$(flip64 "$BG/ti-b64a.$$.py"); f64b=$(flip64 "$BG/ti-b64b.$$.py"); f64c=$(flip64 "$BG/ti-b64c.$$.py")
rm -f "$BG"/target_id.py.?$$ "$BG"/ti-b64?.$$.py
check "b64 negative controls: not looking at un-named file changes -> the S2 notes-changed case reads ok (4; the S1/S3/S4 cases are also caught by their own checks); not checking 'Verified with nothing changed' -> case 5 reads ok; not requiring README to change -> case 11 reads ok" "4|5|11" "$f64a|$f64b|$f64c"

# ---- W15-b65: AX で押した click / scroll（cua.json が route accessibility・delivery background）の間の owner の pointer も未測。pointer の道・cua.json が無い / 壊れた手は赤のまま ----
AXC='{"delivery":{"mode":"background"},"effect":"unverifiable","route":"accessibility","summary":"AXPress"}'
s65() {  # s65 <run.sh> -> r15 の 80-click の形（mouse_idle 0.2）を cua.json の 7 通りで
  for a in "click $AXC" "click {\"route\":\"pointer\",\"delivery\":{\"mode\":\"background\"}}" "click -" "click {" "scroll $AXC" "click {\"route\":\"accessibility\",\"delivery\":{\"mode\":\"foreground\"}}" "click []"; do set -- $a; s62 "$BG/$M65" "$BG" "$1" 0.2 100 "$2"; done | paste -sd' ' -
}
M65=run.sh; S65C=$(s65)
check "b65 AX click: cua.json route accessibility + background + mouse_idle 0.2 -> rc 6, no violation, owner_pointer, tally fires (click and scroll); route pointer / no cua.json / broken JSON / delivery foreground / a JSON list -> red as before" \
  "6/0/owner_pointer/True 1/1/-/False 1/1/-/False 1/1/-/False 6/0/owner_pointer/True 1/1/-/False 1/1/-/False" "$S65C"
python3 -I - "$BG" "$$" <<'EOF65M'
import sys
bg, pid = sys.argv[1:3]
rs = open(f"{bg}/run.sh", encoding="utf-8").read()
def mut(old, new):
    assert rs.count(old) == 1, old
    return rs.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
chk = '            if route!="accessibility" or c["delivery"]["mode"]!="background": sys.exit(1)\n'
w(f"run-b65a.{pid}.sh", mut(chk, '            if False: sys.exit(1)\n'))  # route の確かめを外す
w(f"run-b65b.{pid}.sh", mut('or c["delivery"]["mode"]!="background"', 'or False'))  # delivery の確かめを外す
w(f"run-b65c.{pid}.sh", mut(chk, '            sys.exit(1)\n'))  # b62 のまま（click / scroll は一律に赤）
w(f"run-b65d.{pid}.sh", mut('            c=json.load(open(f"{R}/steps/{name}.cua.json"))\n            route=c["route"]\n' + chk, '            route="accessibility"\n'))  # cua.json を読まない
EOF65M
flip65() {  # flip65 <変異の run.sh> -> 正と違った位置（1 始まり）
  M65=$1; python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$S65C" "$(s65)"
}
f65a=$(flip65 run-b65a.$$.sh); f65b=$(flip65 run-b65b.$$.sh); f65c=$(flip65 run-b65c.$$.sh); f65d=$(flip65 run-b65d.$$.sh)
rm -f "$BG"/run-b65?.$$.sh
check "b65 negative controls: no route check -> the pointer-route and delivery-foreground clicks read unmeasured (2,6); no delivery check -> (6); b62 as it was (click / scroll always red) -> the AX click and scroll stay red (1,5); cua.json not read -> everything unmeasured (2,3,4,6,7)" "2,6|6|1,5|2,3,4,6,7" "$f65a|$f65b|$f65c|$f65d"

# ---- W15-b66: #20 の Engine の「✓ Verified」は、#20 が README の sha と中身を読み戻した時刻（backing.jsonl）が木に初めて出た時以前なら lies に数えない ----
check "b66 driver: #20 ok with the README sha changed and the asked title inside records engine_verified_readme once; sha changed but another title, sha unchanged, fs write, lease runs record nothing" "engine_verified_readme" "$BK66"
tb66() {  # tb66 <tally.py を含む dir> -> "<裏 90・文 100> <裏 無し> <裏 110・文 100> <engine の自己申告 verified だけ> <backing_times: file 有り / started_at 200 / file 無し>"
  python3 -I - "$1" "$T" <<'EOF66'
import json, os, sys
sys.path.insert(0, sys.argv[1]); import tally as t
lines = ["✓ Verified", "Done. Checked 2 items in the original app"]
tree = {"elements": [{"element_index": i, "role": "AXStaticText", "label": x} for i, x in enumerate(lines)]}
fs = {t.sentence_of(x): 100.0 for x in lines}
n = lambda backing: len(t.find_lies(tree, {}, "s", fs, backing))
R = os.path.join(sys.argv[2], "b66run"); os.makedirs(R, exist_ok=True)
open(R + "/backing.jsonl", "w").write(json.dumps({"kind": "engine_verified_readme", "at": 90.0}) + "\n")
E2 = os.path.join(sys.argv[2], "b66none"); os.makedirs(E2, exist_ok=True)
print(n({"engine_verified_readme": 90.0}), n({}), n({"engine_verified_readme": 110.0}), n({"verified": 90.0}), t.backing_times(R, 0), t.backing_times(R, 200), t.backing_times(E2, 0))
EOF66
}
TB66=$(tb66 "$BG")
check "b66 tally: backing 90 <= first seen 100 -> lies 0; no backing -> 2 (both sentences); backing 110 (after the sentence) -> 2; only the engine's own verified record -> 1 ('✓ Verified' stays a lie); backing_times reads backing.jsonl (and ignores one older than started_at; none -> {})" "0 2 2 1 {'engine_verified_readme': 90.0} {} {}" "$TB66"
python3 -I - "$BG" "$$" <<'EOF66M'
import sys
bg, pid = sys.argv[1:3]
rs, ts = open(f"{bg}/rehearse.py", encoding="utf-8").read(), open(f"{bg}/tally.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"rh-b66a.{pid}.py", mut(rs, '    note_backing(C, "engine_verified_readme", at)\n', '    pass\n'))  # driver が裏を書かない
w(f"rh-b66b.{pid}.py", mut(rs, '    if len(gone) > 1 or len(added) != 1 or "Shibubu Demo" not in added[0]:', '    if False:'))  # 中身を見ずに裏を書く
w(f"tl-b66c.{pid}.py", mut(ts, "backing[k] <= at for k, ms", "True for k, ms"))  # 時刻の比べを外す
w(f"tl-b66d.{pid}.py", mut(ts, '("engine_verified_readme", ("✓ Verified", "Done. Checked")))', '("engine_verified_readme", ("Done. Checked",)))'))  # ✓ Verified の印を持たない
EOF66M
mkdir -p "$T/t66c" "$T/t66d"; cp "$BG"/*.py "$T/t66c/"; cp "$BG"/*.py "$T/t66d/"; cp "$BG/tl-b66c.$$.py" "$T/t66c/tally.py"; cp "$BG/tl-b66d.$$.py" "$T/t66d/tally.py"
B66C=$(tb66 "$T/t66c" | cut -d' ' -f1-4); B66D=$(tb66 "$T/t66d" | cut -d' ' -f1-4)
B66A=$(b49 "$BG/rh-b66a.$$.py" >/dev/null; cat "$T/b49/bk.txt"); B66B=$(b49 "$BG/rh-b66b.$$.py" >/dev/null; cat "$T/b49/bk.txt")
rm -f "$BG"/rh-b66?.$$.py "$BG"/tl-b66?.$$.py
check "b66 negative controls: the driver not writing the backing -> nothing recorded; recording without looking at the README content -> the wrong-title run records too (2); comparing no time -> backing 110 passes (0); no '✓ Verified' mark -> '✓ Verified' stays a lie with backing 90 (1)" "||engine_verified_readme,engine_verified_readme|0 2 0 1|1 2 2 1" "$B66A||$B66B|$(echo "$B66C")|$(echo "$B66D")"

# ---- W15-b67: 裏の時刻 = min(driver が気付いた時刻, README の mtime)。mtime が一言を送った時刻より前（古い file）なら使わない ----
check "b67 driver: README written after the send (mtime send+2, noticed at send+5) -> backing time is the mtime (r17 shape); a stale mtime (before the send) -> the noticed time; the wrong-title run records nothing" "approve:mtime,approve_old:tchanged" "$BK67"
python3 -I - "$BG" "$$" <<'EOF67M'
import sys
bg, pid = sys.argv[1:3]
rs = open(f"{bg}/rehearse.py", encoding="utf-8").read()
def mut(old, new):
    assert rs.count(old) == 1, old
    return rs.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"rh-b67a.{pid}.py", mut("    if mtime >= t_send:\n", "    if True:\n"))  # 送った時刻より前の mtime も使う
w(f"rh-b67b.{pid}.py", mut("        at = min(at, mtime)\n", "        pass\n"))  # mtime を使わない（b66 のまま）
EOF67M
B67A=$(b49 "$BG/rh-b67a.$$.py" >/dev/null; cat "$T/b49/bk2.txt"); B67B=$(b49 "$BG/rh-b67b.$$.py" >/dev/null; cat "$T/b49/bk2.txt")
rm -f "$BG"/rh-b67?.$$.py
check "b67 negative controls: not skipping a stale mtime -> the stale run also takes the mtime (approve_old:mtime); never using the mtime -> the r17 shape stays at the noticed time (approve:tchanged)" "approve:mtime,approve_old:mtime|approve:tchanged,approve_old:tchanged" "$B67A|$B67B"

# ---- W15-b69: 本物の AI で回す口（REHEARSE_LLM=real は owner の ok の門つき・既定は偽の CLI のまま）----
# l69 <rehearse.py> <gfinal-tally.py> -> 11 場面
l69() {
  python3 -I - "$1" "$2" "$T" "$BG" <<'EOF69'
import importlib.util, json, os, subprocess, sys
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[4]); sys.modules["rh"] = m; sp.loader.exec_module(m)
sg = importlib.util.spec_from_file_location("gt", sys.argv[2]); g = importlib.util.module_from_spec(sg); sg.loader.exec_module(g)
base = os.path.join(sys.argv[3], f"l69-{os.getpid()}"); os.makedirs(base, exist_ok=True)  # 呼ぶたびに新しい dir
cli = os.path.join(base, "claude"); open(cli, "w").write("#!/bin/sh\n"); os.chmod(cli, 0o755)
KEYS = ("REHEARSE_LLM", "SHIBUBU_QA_LLM_OK", "SHIBUBU_LLM_CLI", "REHEARSE_LLM_CALLS", "REHEARSE_LLM_SECS", "REHEARSE_LLM_WAIT_X", "SHIBUBU_QA_LLM", "SHIBUBU_LLM_BUDGET")
def env(**kw):
    for k in KEYS: os.environ.pop(k, None)
    os.environ.update(kw)
def gate():
    try:
        e = m.llm_env()
        return "fake" if e is None else f"{e['SHIBUBU_QA_LLM']}:{os.path.basename(e['SHIBUBU_LLM_CLI'])}:{e['SHIBUBU_LLM_BUDGET']}"
    except m.LlmGate as x:
        return f"gate{x.rc}"
out = []
env(); a0 = gate(); env(REHEARSE_LLM="fake"); out.append(f"{a0}/{gate()}")
env(REHEARSE_LLM="real"); out.append(gate())
env(REHEARSE_LLM="real", SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI=cli); out.append(gate())
env(REHEARSE_LLM="real", SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI=cli, REHEARSE_LLM_CALLS="3", REHEARSE_LLM_SECS="100"); out.append(gate())
env(REHEARSE_LLM="real", SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI="/nonexistent/claude"); out.append(gate())
def fakedir(real):
    R = os.path.join(base, "wf-" + ("real" if real else "fake")); os.makedirs(R, exist_ok=True)
    env(**({"REHEARSE_LLM": "real", "SHIBUBU_QA_LLM_OK": "ok", "SHIBUBU_LLM_CLI": cli} if real else {"REHEARSE_LLM": "fake"}))
    m.write_fake(R, R, 1, 2)
    return os.path.isdir(os.path.join(R, "fake"))
out.append(f"fakedir:{fakedir(False)}/{fakedir(True)}")
seen = []
m.subprocess = type("S", (), {"run": staticmethod(lambda cmd, **k: seen.append(k["env"]) or type("P", (), {"returncode": 0, "stderr": ""})())})
env(REHEARSE_LLM="fake"); m.run_sh("start", "R", "app", fake="/x/fake")
env(REHEARSE_LLM="real", SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI=cli); m.run_sh("start", "R", "app", fake="/x/fake")
out.append(f"env:{seen[0]['SHIBUBU_LLM_CLI']}|{seen[1].get('SHIBUBU_LLM_CLI') == cli}:{seen[1].get('SHIBUBU_QA_LLM')}:{seen[1].get('SHIBUBU_LLM_BUDGET')}")
env(REHEARSE_LLM="fake"); a = m.llm_scale(); env(REHEARSE_LLM="real"); b = m.llm_scale(); env(REHEARSE_LLM="real", REHEARSE_LLM_WAIT_X="3"); out.append(f"scale:{a}/{b}/{m.llm_scale()}")
R = os.path.join(base, "lj"); os.makedirs(R + "/data", exist_ok=True)
open(R + "/data/llm-calls.jsonl", "w").write("".join(json.dumps({"at_ms": 1, "name": "plan", "ms": ms, "outcome": o}) + "\n" for ms, o in ((1500, "ok"), (2500, "timeout"))))
env(REHEARSE_LLM="real", SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI=cli); d = m.write_llm_json(R); env(REHEARSE_LLM="fake"); f = m.write_llm_json(R)
out.append(f"json:{d['mode']}:{d['calls']}:{d['secs']}:{f['mode']}:{f['cli']}")
O = os.path.join(base, "out"); os.makedirs(O + "/1/run", exist_ok=True); os.makedirs(O + "/2/run", exist_ok=True)
open(O + "/1/run/llm.json", "w").write('{"mode": "real"}'); open(O + "/2/run/llm.json", "w").write('{"mode": "fake"}')
out.append(f"marker:{g.llm_real(O, {'n': 1})}/{g.llm_real(O, {'n': 2})}/{g.llm_real(O, {'n': 3})}")
print(" ".join(out))
EOF69
}
L69=$(l69 "$BG/rehearse.py" "$BG/gfinal-tally.py")
check "b69 real-AI mouth: default = real (gate 8 without the ok; fake only with REHEARSE_LLM=fake) / real without the owner's ok -> gate rc 8 / real+ok -> SHIBUBU_QA_LLM=1 + the real CLI + budget calls=6,secs=900 / own limits from REHEARSE_LLM_CALLS,SECS / no claude CLI -> rc 3; real writes no fake dir (fake does); run_sh hands the real CLI and the budget (fake: fake/claude); waits x1 fake, x2 real, x3 with WAIT_X; llm.json counts calls and secs from llm-calls.jsonl; gfinal table marks real runs only" \
  "gate8/fake gate8 1:claude:calls=12,secs=900 1:claude:calls=3,secs=100 gate3 fakedir:True/False env:/x/fake/claude|True:1:calls=12,secs=900 scale:1.0/2.0/3.0 json:real:2:4.0:fake:None marker:True/False/False" "$L69"
python3 -I - "$BG" "$$" <<'EOF69M'
import sys
bg, pid = sys.argv[1:3]
rs, gs = open(f"{bg}/rehearse.py", encoding="utf-8").read(), open(f"{bg}/gfinal-tally.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"rh-b69a.{pid}.py", mut(rs, 'if not os.environ.get("SHIBUBU_QA_LLM_OK"):', 'if False:'))  # 門を外す
w(f"rh-b69b.{pid}.py", mut(rs, '    if llm_env():  # 本物の AI の run: interpret / generate の答えを置かない', '    if False:  # 本物の AI の run: interpret / generate の答えを置かない'))  # real でも fake を置く
w(f"rh-b69c.{pid}.py", mut(rs, "    real = llm_env()\n    if real:  # 本物", "    real = None\n    if real:  # 本物"))  # run_sh が本物の CLI を渡さない
w(f"rh-b69d.{pid}.py", mut(rs, 'os.environ.get("REHEARSE_LLM_WAIT_X") or 2)', 'os.environ.get("REHEARSE_LLM_WAIT_X") or 1)'))  # 待ちを伸ばさない
w(f"gt-b69e.{pid}.py", mut(gs, '["mode"] == "real":', '["mode"] == "nope":'))  # 表に印を出さない
EOF69M
flip69() {  # flip69 <rehearse.py> [gfinal-tally.py] -> 正と違った位置（1 始まり）
  python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$L69" "$(l69 "${1:-$BG/rehearse.py}" "${2:-$BG/gfinal-tally.py}")"
}
f69a=$(flip69 "$BG/rh-b69a.$$.py"); f69b=$(flip69 "$BG/rh-b69b.$$.py"); f69c=$(flip69 "$BG/rh-b69c.$$.py"); f69d=$(flip69 "$BG/rh-b69d.$$.py"); f69e=$(flip69 "$BG/rehearse.py" "$BG/gt-b69e.$$.py")
rm -f "$BG"/rh-b69?.$$.py "$BG"/gt-b69?.$$.py
check "b69 negative controls: gate removed -> default and real without ok pass (1,2); real still writes the fake dir (6); run_sh not handing the real CLI (7); waits not stretched (8); table marker off (10)" "1,2|6|7|8|10" "$f69a|$f69b|$f69c|$f69d|$f69e"

# ---- W15-b68: D-g の cursor の赤の出所の分類（hid / posted / warp / unmeasured）。判定は変えず verdict に足すだけ ----
# cs68 <cursor_source.py> -> 8 場面の分類
cs68() {
  python3 -I - "$1" <<'EOF68'
import importlib.util, sys
sp = importlib.util.spec_from_file_location("cs", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
KINDS = ("moved", "left", "right", "other", "ldrag", "rdrag", "odrag", "scroll", "tablet", "key", "flags")
def row(t, x, y, hid=None, comb=None):  # hid / comb = {種類: 秒}（無い種類は 1000 秒）
    side = lambda d: {k: (d or {}).get(k, 1000.0) for k in KINDS}
    return {"t": t, "x": x, "y": y, "hid": side(hid), "comb": side(comb)}
S = lambda rows: m.classify(rows)["cursor_source"]
two = lambda **kw: [row(0, 10, 10), row(0.5, 60, 10, **kw)]
print(S(two(hid={"moved": 0.3}, comb={"moved": 0.3})), S(two(comb={"moved": 0.2})), S(two()), S([row(0, 10, 10)]), S([row(0, 10, 10), row(0.5, 10, 10)]), S([row(0, 10, 10), row(0.5, 10.4, 10)]),
      S(two(hid={"tablet": 0.2}, comb={"tablet": 0.2})), S(two(hid={"key": 0.1}, comb={"key": 0.1})))
EOF68
}
CS68=$(cs68 "$BG/cursor_source.py")
check "b68 classify: HID mouse event -> hid / only the combined session fresh -> posted / both old -> warp / one row -> unmeasured / no move -> unmeasured / 0.4 pt (below the threshold) -> unmeasured / a tablet event -> hid / a key event alone is not a mouse input -> warp" "hid posted warp unmeasured unmeasured unmeasured hid warp" "$CS68"
python3 -I - "$BG" "$$" <<'EOF68M'
import sys
bg, pid = sys.argv[1:3]
cs = open(f"{bg}/cursor_source.py", encoding="utf-8").read()
def mut(old, new):
    assert cs.count(old) == 1, old
    return cs.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"cs-b68a.{pid}.py", mut('("posted" if c is not None and c < FRESH_S else "warp")', '"warp"'))  # combined 側を見ない
w(f"cs-b68b.{pid}.py", mut(">= MOVE_PT", ">= 0"))  # 位置の閾を外す
w(f"cs-b68c.{pid}.py", mut('"scroll", "tablet")', '"scroll")'))  # tablet を mouse 系に数えない
w(f"cs-b68d.{pid}.py", mut('"scroll", "tablet")', '"scroll", "tablet", "key")'))  # key も mouse 系に数える
EOF68M
flip68() {  # flip68 <変異した cursor_source.py> -> 正と違った位置（1 始まり）
  python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$CS68" "$(cs68 "$1")"
}
f68a=$(flip68 "$BG/cs-b68a.$$.py"); f68b=$(flip68 "$BG/cs-b68b.$$.py"); f68c=$(flip68 "$BG/cs-b68c.$$.py"); f68d=$(flip68 "$BG/cs-b68d.$$.py")
rm -f "$BG"/cs-b68?.$$.py
check "b68 negative controls: combined side not read -> the posted case reads warp (2); no position threshold -> the no-move and 0.4 pt cases get a source (5,6); tablet not counted -> the tablet case reads warp (7); key counted as mouse -> the key case reads hid (8)" "2|5,6|7|8" "$f68a|$f68b|$f68c|$f68d"
# judge_step の配線: cursor の赤の verdict に分類を足す（赤は赤・rc も不変）／owner_pointer に回る手にも足す／cursor の無い violation には足さない
python3 -I - "$T/cur-posted.jsonl" "$T/cur-warp.jsonl" <<'EOF68F'
import json, sys
K = ("moved", "left", "right", "other", "ldrag", "rdrag", "odrag", "scroll", "tablet", "key", "flags")
def row(t, x, c): return {"t": t, "x": x, "y": 10, "hid": {k: 1000.0 for k in K}, "comb": {k: (c if k == "moved" else 1000.0) for k in K}}
open(sys.argv[1], "w").write("".join(json.dumps(r) + "\n" for r in (row(10.0, 10, 1000.0), row(10.5, 80, 0.2))))  # posted
open(sys.argv[2], "w").write("".join(json.dumps(r) + "\n" for r in (row(10.0, 10, 1000.0), row(10.5, 80, 1000.0))))  # warp
EOF68F
v68() {  # v68 <tool> <mouse_idle> <front pid> <cursor file|-> -> "<rc> <violations> <cursor_source> <context の tool>"
  local rc; rc=$(s62 "$BG/run.sh" "$BG" "$1" "$2" "$3" - "$4" | cut -d/ -f1)
  python3 -I -c 'import json,sys;v=json.load(open(sys.argv[2]+"/steps/s.verdict.json"));print(sys.argv[1],len(v["violations"]),v.get("cursor_source","-"),(v.get("cursor_context") or {}).get("tool","-"))' "$rc" "$(cat "$T/last62")"
}
V68=$(echo "$(v68 type_text 500 100 "$T/cur-posted.jsonl")|$(v68 type_text 500 100 "$T/cur-warp.jsonl")|$(v68 type_text 0.2 100 "$T/cur-posted.jsonl")|$(v68 type_text 500 100 -)|$(v68 type_text 500 300 "$T/cur-posted.jsonl")")
check "b68 judge_step: a cursor red gets cursor_source (posted / warp with the step's tool as context) and stays red (rc 1, 1 violation); a step turned into owner_pointer is annotated too (rc 6, 0 violations); no poll file -> unmeasured; a front change as well as the cursor still gets it (2 violations)" "1 1 posted -|1 1 warp type_text|6 0 posted -|1 1 unmeasured -|1 2 posted -" "$V68"
# poll の配線: step が axprobe --poll を起こし、手の後に止め（自分の pid だけ・stop file を残さない）、cursor.jsonl を残す
printf '#!/bin/bash\nif [ "$1" = --poll ]; then out=; stop=; while [ $# -gt 0 ]; do case $1 in --out) out=$2;; --stop) stop=$2;; esac; shift; done; echo $$ > "%s/poll.pid"; echo "{\\"t\\":1}" > "$out"; for i in $(seq 1200); do [ -e "$stop" ] && exit 0; sleep 0.1; done; exit 0; fi\nexec "%s/fake-axprobe" "$@"\n' "$T" "$T" > "$T/fake-axprobe-poll"; chmod +x "$T/fake-axprobe-poll"
printf '#!/bin/bash\necho "{\\"effect\\":\\"ok\\"}"\n' > "$T/fake-cua68"; chmod +x "$T/fake-cua68"
pl68() {  # pl68 <run.sh> -> "<cursor.jsonl の行数> <stop file が残った> <poll の process が生きている>"
  rm -f "$T/poll.pid"; rm -rf "$R/steps/pl68".*
  ( source "$1"; AXPROBE="$T/fake-axprobe-poll" CUA_DRIVER_REAL="$T/fake-cua68" cmd_step "$R" pl68 click '{"pid":1,"element_token":"s1:2","session":"q"}' ) >/dev/null 2>&1
  local alive=no; [ -s "$T/poll.pid" ] && kill -0 "$(cat "$T/poll.pid")" 2>/dev/null && { alive=yes; kill "$(cat "$T/poll.pid")" 2>/dev/null; }
  echo "$([ -f "$R/steps/pl68.cursor.jsonl" ] && wc -l < "$R/steps/pl68.cursor.jsonl" | tr -d ' ') $([ -e "$R/steps/pl68.cursor.stop" ] && echo yes || echo no) $alive"
}
python3 -I - "$BG" "$$" <<'EOF68R'
import sys
bg, pid = sys.argv[1:3]
rs = open(f"{bg}/run.sh", encoding="utf-8").read()
def mut(old, new):
    assert rs.count(old) == 1, old
    return rs.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"run-b68a.{pid}.sh", mut('  stop_poll "$R" "$name" "$pollpid"\n  wait_events', '  wait_events'))  # 止めない
w(f"run-b68b.{pid}.sh", mut('"$AXPROBE" --poll --out', ': --poll --out'))  # 起こさない
EOF68R
check "b68 poll wiring: step starts axprobe --poll, stops it after the step (own pid, no stop file left) and keeps cursor.jsonl; negative controls: not stopping -> the poll process is still alive; not starting -> no cursor.jsonl" "1 no no|1 no yes| no no" "$(pl68 "$BG/run.sh")|$(pl68 "$BG/run-b68a.$$.sh")|$(pl68 "$BG/run-b68b.$$.sh")"
rm -f "$BG"/run-b68?.$$.sh


# ---- W15-b70: 手の対象の app が hidden / 窓 0 の間の赤は未測(hidden)（snap --pid の targets・赤だけ倒す・unhide しない）----
# s70 <run.sh> <tally.py の dir> <before.targets の JSON|-> <cursor が動くか 0|1> -> "<judge_step の rc>/<violations 数>/<unmeasured target>/<tally.target_hidden が文を返したか>/<guard_state>"
s70() {
  local R; R=$(mktemp -d "$T/b70.XXXX"); mkdir -p "$R/steps"
  python3 -I - "$R" "$3" "$4" <<'EOF70'
import json, sys
R, targets, move = sys.argv[1:4]
def win(i, pid, z): return {"window_id": i, "pid": pid, "layer": 0, "bounds": {"x": 0, "y": 0, "w": 400, "h": 300}, "onscreen": True, "z": z}
def snap(t, x): return {"schema": "shibubu.qa.snapshot/1", "t": t, "front": {"pid": 100, "name": "Owner"}, "cursor": {"x": x, "y": 10},
  "focus": {"pid": 100, "role": "AXTextArea", "frame": {"x": 0, "y": 0, "w": 10, "h": 10}}, "windows": [win(1, 100, 1)], "unmeasured": {}}
json.dump({"owner_pid": 100, "suspects": [200]}, open(R + "/run.json", "w"))
b = snap(10.0, 10)
if targets != "-": b["targets"] = json.loads(targets)
json.dump(b, open(R + "/steps/s.before.json", "w")); json.dump(snap(12.0, 80 if move == "1" else 10), open(R + "/steps/s.after.json", "w"))
ev = [{"schema": "shibubu.qa.event/1", "t": 9.0, "kind": "start"}, {"schema": "shibubu.qa.event/1", "t": 12.2, "kind": "z_sample", "windows": []}]
open(R + "/events.jsonl", "w").write("".join(json.dumps(e) + "\n" for e in ev))
json.dump({"schema": "shibubu.qa.axprobe/1", "t": 11.0, "pressure": {"hid_idle_s": 900.0, "mouse_idle_s": 900.0, "key_idle_s": 900.0}}, open(R + "/steps/s.axprobe.json", "w"))  # owner の入力なし = owner_pointer にならない
EOF70
  local rc; rc=$( ( source "$1"; judge_step "$R" s "$R/events.jsonl" type_text '{}' 0 "" "" "" >/dev/null 2>&1; echo $? ) | tail -1 )
  python3 -I - "$R" "$2" "$rc" <<'EOF70B'
import json, sys
R, td, rc = sys.argv[1:4]
sys.path.insert(0, td); import tally
v = json.load(open(R + "/steps/s.verdict.json"))
w = tally.target_hidden(v)
print(f'{rc}/{len(v["violations"])}/{(v.get("unmeasured") or {}).get("target", "-")}/{w is not None}/{tally.guard_state([], {"hidden.s": w} if w else {})}')
EOF70B
}
H70='[{"pid":200,"name":"Discord","hidden":true,"onscreen_windows":2}]'
V70='[{"pid":200,"name":"Discord","hidden":false,"onscreen_windows":2}]'
W70='[{"pid":200,"name":"Discord","hidden":false,"onscreen_windows":0}]'
X70='[{"pid":300,"name":"TextEdit","hidden":false,"onscreen_windows":1},{"pid":200,"name":"Discord","hidden":true,"onscreen_windows":1}]'
sc70() {  # sc70 <run.sh> <tally dir> -> 6 場面
  echo "$(s70 "$1" "$2" "$H70" 1) $(s70 "$1" "$2" "$V70" 1) $(s70 "$1" "$2" "$H70" 0) $(s70 "$1" "$2" "$W70" 1) $(s70 "$1" "$2" - 1) $(s70 "$1" "$2" "$X70" 1)"
}
S70=$(sc70 "$BG/run.sh" "$BG")
check "b70 target hidden: red + target hidden -> rc 6, no violation, unmeasured target=hidden, tally unmeasured(hidden); red + visible -> rc 1 red; green + hidden -> rc 0 unchanged; red + 0 on-screen windows -> rc 6; red with no targets in the snapshot -> rc 1; any hidden among several targets -> rc 6" \
  "6/0/hidden/True/unmeasured(hidden) 1/1/-/False/ok 0/0/-/False/ok 6/0/hidden/True/unmeasured(hidden) 1/1/-/False/ok 6/0/hidden/True/unmeasured(hidden)" "$S70"
python3 -I - "$BG" "$$" <<'EOF70X'
import sys
bg, pid = sys.argv[1:3]
rs, ts = open(f"{bg}/run.sh", encoding="utf-8").read(), open(f"{bg}/tally.py", encoding="utf-8").read()
def mut(s, old, new):
    assert s.count(old) == 1, old
    return s.replace(old, new)
w = lambda n, t: open(f"{bg}/{n}", "w", encoding="utf-8").write(t)
w(f"run-b70a.{pid}.sh", mut(rs, 't["hidden"] is True or ', ''))  # hidden を見ない
g = mut(rs, '  [ $jrc -eq 1 ] && target_hidden "$R/steps/$name.verdict.json" "$R" "$name"', '  target_hidden "$R/steps/$name.verdict.json" "$R" "$name"')
w(f"run-b70b.{pid}.sh", mut(g, "    if not vs: sys.exit(1)\n    ts=", "    ts="))  # 赤以外にも倒す
w(f"run-b70c.{pid}.sh", mut(rs, ' or t["onscreen_windows"]==0]', ']'))  # 窓 0 を見ない
w(f"run-b70d.{pid}.sh", mut(rs, '  [ $jrc -eq 1 ] && target_hidden "$R/steps/$name.verdict.json" "$R" "$name" && { jrc=6;', '  [ $jrc -eq 1 ] && false && { jrc=6;'))  # 倒す処理を外す
w(f"tl-b70.{pid}.py", mut(ts, '    if any(k.startswith("hidden.") for k in keys):\n        return "unmeasured(hidden)"\n', ''))  # 表の語に出ない
w(f"run-b70e.{pid}.sh", mut(rs, '  [ -n "$tpid" ] && snapn+=(--pid "$tpid")\n', ''))  # snap に pid を渡さない
EOF70X
flip70() {  # flip70 <変異の結果> -> 正と違った場面の番号
  python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(str(i+1) for i in range(len(a)) if a[i]!=b[i]) or "none")' "$S70" "$1"
}
f70a=$(flip70 "$(sc70 "$BG/run-b70a.$$.sh" "$BG")")
f70b=$(flip70 "$(sc70 "$BG/run-b70b.$$.sh" "$BG")")
f70c=$(flip70 "$(sc70 "$BG/run-b70c.$$.sh" "$BG")")
f70d=$(flip70 "$(sc70 "$BG/run-b70d.$$.sh" "$BG")")
mkdir -p "$T/t70m"; cp "$BG"/*.py "$T/t70m/"; cp "$BG/tl-b70.$$.py" "$T/t70m/tally.py"
f70t=$(flip70 "$(sc70 "$BG/run.sh" "$T/t70m")")
check "b70 negative controls: hidden not read -> 1,6; non-red also turned (gate and vs check removed) -> 3; windows 0 not read -> 4; turning removed -> 1,4,6; tally state word removed -> 1,4,6" "1,6|3|4|1,4,6|1,4,6" "$f70a|$f70b|$f70c|$f70d|$f70t"
# snap の配線: step が手の json の pid を before / after の snap に --pid で渡す（pid の無い手は渡さない）
printf '#!/bin/bash\necho "$*" >> "%s/fg70.log"\nexec "%s" "$@"\n' "$T" "$FRONT_GUARD" > "$T/fake-fg70"; chmod +x "$T/fake-fg70"
pp70() {  # pp70 <run.sh> <json> -> "<snap の呼び出し数> <うち --pid 付き>"
  rm -f "$T/fg70.log"; rm -rf "$R/steps/pp70".*
  ( source "$1"; FRONT_GUARD="$T/fake-fg70" AXPROBE="$T/fake-axprobe-poll" CUA_DRIVER_REAL="$T/fake-cua68" cmd_step "$R" pp70 click "$2" ) >/dev/null 2>&1
  echo "$(grep -c '^snap .*pp70' "$T/fg70.log" 2>/dev/null) $(grep -c '^snap .*pp70.* --pid 42' "$T/fg70.log" 2>/dev/null)"
}
check "b70 snap wiring: a step with a pid in its json passes --pid to the before and after snap; a step whose pid is not an integer passes none; negative control: not passing -> 0" "2 2|2 0|2 0" "$(pp70 "$BG/run.sh" '{"pid":42,"element_token":"s1:2","session":"q"}')|$(pp70 "$BG/run.sh" '{"pid":"x","element_token":"s1:2","session":"q"}')|$(pp70 "$BG/run-b70e.$$.sh" '{"pid":42,"element_token":"s1:2","session":"q"}')"
rm -f "$BG"/run-b70?.$$.sh "$BG"/tl-b70.$$.py
# 本物の snap --pid: 起きている app（Finder）は hidden / onscreen_windows が出る・--pid 無しは targets が無い・起きていない pid は入れない
# bin/ は gitignore。671e1589 より古い binary は targets キーが無く、get("targets") が null になる。既定の binary がソースより古い時だけ front-guard を build し直す（差し替えた FRONT_GUARD はそのまま）。
b70_skip=0
if [ "$FRONT_GUARD" = "$BG/bin/front-guard" ]; then
  if [ ! -x "$FRONT_GUARD" ] || [ "$BG/front-guard.swift" -nt "$FRONT_GUARD" ] || [ "$BG/lib/Records.swift" -nt "$FRONT_GUARD" ]; then
    if [ -z "${SHIBUBU_SIGN_ID:-}" ] && security find-certificate -c "Shibubu Dev" >/dev/null 2>&1; then
      export SHIBUBU_SIGN_ID="Shibubu Dev"
    fi
    if ! bash "$BG/build.sh" front-guard; then
      check "b70 real snap --pid: a running app gets pid / hidden(bool) / onscreen_windows(int), a dead pid is left out (1 target), no --pid -> no targets key" "1 True True True False" "build-failed"
      b70_skip=1
    fi
  fi
fi
FIN=$(pgrep -x Finder | head -1)
if [ "$b70_skip" -eq 0 ] && [ -n "$FIN" ] && [ -x "$FRONT_GUARD" ]; then
  SN70=$("$FRONT_GUARD" snap --out "$T/sn70a.json" --pid "$FIN" --pid 999999 && "$FRONT_GUARD" snap --out "$T/sn70b.json" && python3 -I -c 'import json,sys;a=json.load(open(sys.argv[1]));b=json.load(open(sys.argv[2]));t=a.get("targets");print(len(t),t[0]["pid"]==int(sys.argv[3]),isinstance(t[0]["hidden"],bool),isinstance(t[0]["onscreen_windows"],int),"targets" in b)' "$T/sn70a.json" "$T/sn70b.json" "$FIN")
  check "b70 real snap --pid: a running app gets pid / hidden(bool) / onscreen_windows(int), a dead pid is left out (1 target), no --pid -> no targets key" "1 True True True False" "$SN70"
fi

# ---- W15-b71: tab の名前は Project ごとに違う（本物の AI = r18）→ 押す直前の DB から種類（面の spec・terminal）で Page の題を引く / #25 の入口は DB の Project の title / #22 は #5 の同意が無ければ未測 ----
p71() {  # p71 <rehearse.py> -> 各場面の結果
  python3 -I - "$1" <<'EOF71'
import importlib.util, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
m.time.sleep = lambda s: None
LBL = {"readme": "TextEdit", "architecture": "https://excalidraw.com", "docs": "TextEdit", "diagram": "https://excalidraw.com"}  # r18 の最終 revision: label は readme / architecture の node だけ・notes の node には無い
def src(ref): return {"component": "column", "children": [dict({"component": "source_surface", "props": {"surface_ref": ref}}, **({"label": LBL[ref]} if ref in LBL else {}))] if ref else []}
def term(n=1): return {"component": "column", "children": [{"component": "terminal"} for _ in range(n)]}
def proj(name, specs, pages): return {"d": {"name": name, "surface_specs": specs, "pages": [{"title": t, "root": r} for t, r in pages]}}
S18 = [{"id": "readme", "backend": "mirror", "target": {"bundle_id": "com.apple.TextEdit", "file": "README.md", "kind": "app"}}, {"id": "architecture", "backend": "webview", "target": {"kind": "url", "url": "https://excalidraw.com"}}, {"id": "notes", "backend": "mirror", "target": {"bundle_id": "com.apple.TextEdit", "file": "docs/notes.md", "kind": "app"}}]
S17 = [{"id": "docs", "backend": "mirror", "target": {"file": "README.md"}}, {"id": "diagram", "backend": "webview", "target": {"url": "https://excalidraw.com"}}, {"id": "notes", "backend": "mirror", "target": {"file": "docs/notes.md"}}]
R18_PRE = proj("Repo dev environment", S18, [("Dev terminal", term()), ("README", src("readme")), ("Architecture", src("architecture"))])  # r18 の #5 の時点（形だけ）
R18_END = proj("Repo dev environment", S18, [("Dev terminal", term(2)), ("README", src(None)), ("Architecture", src("readme")), ("Tests", src("architecture")), ("notes", src("notes"))])  # r18 の最終（README の Page は空・docs は Architecture へ）
R17_PRE = proj("Dev environment", S17, [("Dev", term()), ("Docs", src("docs")), ("Diagram", src("diagram"))])
cur = [R18_PRE]; m.db_state = lambda C: cur[0]
tabs = lambda *ts: [{"role": "AXCheckBox", "label": t, "element_token": "t-" + t} for t in ts]
els = [tabs("Home", "Runs", "Repo dev environment", "Dev terminal", "README", "Architecture")]; m.elements = lambda C: els[0]
clicks = []; m.step = lambda C, tool, args, *a, **k: clicks.append(args.get("element_token")) or {}
C = types.SimpleNamespace(nonce="n", state={})
def goto(db, ts):
    cur[0] = db; els[0] = tabs(*ts); clicks.clear()
    try:
        m.h_goto_tab(C, {"doing": "『Docs』『Dev』『Diagram』"}); return ",".join(c[2:] for c in clicks)
    except m.Unmeasured as e:
        return "unm"
def kind(db, k):
    cur[0] = db
    try: return m.page_title(C, k).replace(" ", "_")
    except m.Unmeasured: return "unm"
res = {}
res["r18_goto"] = goto(R18_PRE, ["Dev terminal", "README", "Architecture"]).replace(" ", "_")
res["r17_goto"] = goto(R17_PRE, ["Dev", "Docs", "Diagram"])
res["r18_end"] = "|".join(kind(R18_END, k) for k in ("docs", "diagram", "notes", "dev"))  # 手 14 / 13 の後: docs は Architecture・diagram は Tests
res["no_terminal"] = kind(proj("P", S17, [("Docs", src("docs"))]), "dev")
res["two_docs"] = kind(proj("P", S17, [("A", src("docs")), ("B", src("docs"))]), "docs")
res["no_notes"] = kind(R17_PRE, "notes")
els[0] = tabs("Home") + tabs("README", "README"); cur[0] = R18_PRE; res["dup_tab"] = goto(R18_PRE, ["Dev terminal", "README", "README", "Architecture"])
# 押す直前の DB: #14 の移動の前後で同じ種類の題が変わる
res["moved"] = kind(R18_PRE, "docs") + ">" + kind(R18_END, "docs")
# #25 の入口は DB の Project の title
pressed = []; m.press = lambda C_, role, label, expect=None, **k: pressed.append(label)
cur[0] = R18_PRE; m.press_project(C); cur[0] = R17_PRE; m.press_project(C)
res["entry"] = ",".join(x.replace(" ", "_") for x in pressed)
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF71
}
P71_WANT="r18_goto=README,Dev_terminal,Architecture r17_goto=Docs,Dev,Diagram r18_end=Architecture|Tests|notes|Dev_terminal no_terminal=unm two_docs=unm no_notes=unm dup_tab=unm moved=README>Architecture entry=Repo_dev_environment,Dev_environment"
check "b71 tab by kind: r18 (README / Dev terminal / Architecture) and r17 fake (Docs / Dev / Diagram) both resolve from the DB at press time (docs moves to Architecture after #14); no terminal / two docs pages / no notes surface / two AX tabs with the title -> Unmeasured; #25 entry = DB Project title" "$P71_WANT" "$(p71 "$BG/rehearse.py")"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF71M'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
def mut(n, *pairs):
    t = s
    for old, new in pairs:
        assert t.count(old) == 1, old
        t = t.replace(old, new)
    open(f"{sys.argv[2]}/rh-b71{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(t)
mut("a", ('press_tab(C, TAB_KIND[t], expect="ax:*") if t in TAB_KIND else press(C, "AXCheckBox", t, expect="ax:*")', 'press(C, "AXCheckBox", t, expect="ax:*")'))  # 固定名に戻す
mut("b", ('    st = db_state(C)\n    if st is None:\n        raise Unmeasured(f"DB が読めない（{kind} の tab の題）")', '    st = C.state.setdefault("db71", db_state(C))\n    if st is None:\n        raise Unmeasured(f"DB が読めない（{kind} の tab の題）")'))  # 押す直前でなく最初に 1 回だけ読む
mut("d", ('    press(C, "AXCheckBox", name)\n\n\n', '    press(C, "AXCheckBox", "dev_env")\n\n\n'))  # 入口を固定名に戻す
mut("f", ('        titles = [p.get("title") for p in d.get("pages") or [] if has_term(p.get("root"))]\n', '        titles = [p.get("title") for p in d.get("pages") or [] for _ in range(len(re.findall(\"terminal\", json.dumps(p.get(\"root\")))))]\n'), ('    titles = list(dict.fromkeys(titles))\n', '    titles = list(titles)\n'))  # Page でなく component の数で数える（同じ Page の terminal 2 つ → dev が 2 つ扱い）
mut("e", ('        titles = surface_pages(d, SURFACE_KIND[kind])', '        lab = {"docs": "TextEdit", "notes": "TextEdit", "diagram": "https://excalidraw.com"}[kind]\n        has_l = lambda n: isinstance(n, dict) and (n.get("label") == lab or any(has_l(v) for v in (n.get("children") or [])))\n        titles = [p.get("title") for p in d.get("pages") or [] if has_l(p.get("root"))]'))  # 種類を label で決める（notes の node には label が無い・TextEdit の面が 2 つ）
EOF71M
for x in a b d e f; do eval "m71$x=\$(p71 \"\$BG/rh-b71$x.$$.py\" 2>&1 | tail -1)"; done
f71() { python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(x.split("=")[0] for x,y in zip(a,b) if x!=y) or ("none" if len(a)==len(b) else "len"))' "$P71_WANT" "$1"; }
check "b71 negative controls: fixed names -> the r18 goto only (r17 fake still resolves); DB read once instead of at press time -> r17_goto,r18_end,no_terminal,two_docs,moved; entry back to dev_env -> entry; kind decided by the node label (notes has none, two TextEdit surfaces) -> r18_end,no_notes; counting terminal components instead of Pages (r18 Dev terminal has 2) -> r18_end" "r18_goto|r17_goto,r18_end,no_terminal,two_docs,moved|entry|r18_end,no_notes|r18_end" "$(f71 "$m71a")|$(f71 "$m71b")|$(f71 "$m71d")|$(f71 "$m71e")|$(f71 "$m71f")"
rm -f "$BG"/rh-b71?.$$.py

# ---- W15-b72: #20 の合格 = 頼んだ見出しの行だけが変わる・製品が止まったら待たない(product_stopped)・前提の手が ok でなければ blocked(#N) ----
p72() {  # p72 <rehearse.py> -> 各場面の結果
  python3 -I - "$1" "$T/b72" "$BG" <<'EOF72'
import importlib.util, json, os, shutil, sys, types
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
base = sys.argv[2]; shutil.rmtree(base, ignore_errors=True); os.makedirs(base)
clk = [1000.0]
rh.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: clk.__setitem__(0, clk[0] + max(s, 1)))
rows, _ = rh.parse(rh.DEMO); r20 = rows[19]
cnt = [0]
T = lambda role, label=None, value=None, tok=None: {k: v for k, v in dict(role=role, label=label, value=value, element_token=tok).items() if v}
APPROVE = [T("AXStaticText", value="To do X, Shibubu will send 2 steps: Send “Shibubu Demo” to TextEdit"), T("AXButton", "Approve and send", tok="t-a")]
def case(after, other_after=None, stop_line=None, acts='{"tool":"set_value"}\n{"tool":"menu"}\n', race=False):
    cnt[0] += 1; R = f"{base}/r{cnt[0]}"; os.makedirs(R + "/data"); os.makedirs(R + "/shibubu-yc-demo/docs")
    readme = R + "/shibubu-yc-demo/README.md"; notes = R + "/shibubu-yc-demo/docs/notes.md"
    open(readme, "w").write("# old\nline A\nline B\n"); open(notes, "w").write("n1\n"); open(R + "/data/gui-acts.jsonl", "w").close(); open(R + "/data/engine-runs.jsonl", "w").close()
    json.dump({"pid": 1, "window_id": 2, "session": "s"}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); C.state["ai_target"] = (9, 9)
    st = {"sent": False}; t0 = clk[0]
    rh.front_name = lambda: "TextEdit"; rh.send_field = lambda *a, **k: None; rh.try_press = lambda *a, **k: True
    def fpress(C_, role, label, **k):
        if label == "send_edit":
            st["sent"] = True
            if stop_line: open(R + "/data/engine-runs.jsonl", "a").write(stop_line + "\n")
        if label == "approve":
            open(readme, "w").write(after)
            if other_after is not None: open(notes, "w").write(other_after)
            open(R + "/data/gui-acts.jsonl", "a").write(acts)
    rh.press = fpress
    polls = [0]
    def fel(C_):  # race: 承認を頼む行（denied ＋ pending）が engine-runs に先に出て、button は 2 回目の読みから（r20 #20）
        if not st["sent"]: return [T("AXTextField", "Change this project")]
        polls[0] += 1
        return list(APPROVE) if (race and polls[0] > 1) or not stop_line else []
    rh.elements = fel
    rh.wait_seen = lambda C_, needles, secs, role=None, **k: (rh.L.names("done_checked")[0], "Done"); rh.foreign_titled = lambda C_, t: 0; rh.note_backing = lambda *a: None
    try:
        rh.h_ask_ai(C, r20); r = "ok"
    except rh.Unmeasured: r = "unm"
    except RuntimeError as e: r = "red:" + ("stopped" if "product_stopped" in str(e) else "diff" if "行の差" in str(e) else "other" if "書類" in str(e) else "f1")
    return f"{r}@{int(clk[0] - t0)}s" if (stop_line and not race) or r == "unm" else r
res = {}
res["heading"] = case("# Shibubu Demo\nline A\nline B\n")
res["inserted"] = case("# Shibubu Demo\n# old\nline A\nline B\n")
res["one_line"] = case("# Shibubu Demo\n")
res["extra_gone"] = case("# Shibubu Demo\nline A\n")
res["notes_changed"] = case("# Shibubu Demo\nline A\nline B\n", other_after="n2\n")
res["stopped"] = case("", stop_line=json.dumps({"error": {"kind": "failed", "why": "No way to do X was found"}}))
res["denied_nopend"] = case("", stop_line=json.dumps({"error": {"kind": "denied", "why": "Nothing was sent: 3 windows of TextEdit", "pending": []}}))
res["approval_race"] = case("# Shibubu Demo\nline A\nline B\n", stop_line=json.dumps({"error": {"kind": "denied", "why": "'desktop.key' needs approval", "pending": [{"action": "desktop.key", "step": "go_top"}]}}), race=True)
res["key_save"] = case("# Shibubu Demo\nline A\nline B\n", acts='{"receipt":{"action":{"op":"set_value","value":"# Shibubu Demo"}}}\n{"receipt":{"action":{"op":"key","keys":"cmd+s"}}}\n')
res["key_nosave"] = case("# Shibubu Demo\nline A\nline B\n", acts='{"receipt":{"action":{"op":"set_value","value":"# Shibubu Demo"}}}\n{"receipt":{"action":{"op":"key","keys":"cmd+up"}}}\n')
# 前提: _live の blocked(#N)
def live(specs, keep=True):
    R = f"{base}/live{cnt[0] + 1}"; cnt[0] += 1; os.makedirs(R + "/steps"); json.dump({"window_id": 1}, open(R + "/run.json", "w"))
    C = rh.Ctx(R, "X.app", "n1"); rh.run_sh = lambda *a, **k: (0, ""); rh.tally_all = lambda C_: {}
    called = []
    def mkh(n, mode):
        def h(C_, r):
            called.append(n)
            if mode == "unm": raise rh.Unmeasured("x")
            if mode == "red": raise RuntimeError("boom")
        return h
    rh.HANDLERS.update({f"k{n}": mkh(n, m) for n, m in specs})
    rows = [dict(n=n, who="BG", key=f"k{n}", sec=1, doing="") for n, _ in specs]
    os.environ["REHEARSE_KEEP_GOING"] = "1"
    import contextlib, io
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        rc = rh._live(rows, "1", "X.app", False, 1, "n1", C)
    stt = [x["status"] for x in json.load(open(R + "/rehearse.json"))["steps"]]
    return "/".join(("blocked" if s.startswith("blocked(#") else "unm" if s.startswith("未測") else "red" if s.startswith("赤") else s) for s in stt) + f":rc{rc}:{','.join(map(str, called))}"
res["req_unm"] = live([(9, "unm"), (10, "ok")])
res["req_ok_red"] = live([(9, "ok"), (10, "red")])
res["req_red"] = live([(9, "red"), (10, "ok")])
res["req_chain"] = live([(5, "unm"), (22, "ok"), (23, "ok")])
os.environ["REHEARSE_UNTIL"] = "9"; res["until"] = live([(9, "ok"), (10, "red")]); del os.environ["REHEARSE_UNTIL"]  # REHEARSE_UNTIL: 9 で止め #10 は打たない
print(" ".join(f"{k}={v}" for k, v in res.items()))
EOF72
}
P72_WANT="heading=ok inserted=ok one_line=red:diff extra_gone=red:diff notes_changed=red:other stopped=red:stopped@0s denied_nopend=red:stopped@0s approval_race=ok key_save=ok key_nosave=red:f1 req_unm=unm/blocked:rc6:9 req_ok_red=ok/red:rc1:9,10 req_red=red/blocked:rc1:9 req_chain=unm/blocked/blocked:rc6:5 until=ok/skip:rc0:9"
check "b72 #20 passes only when just the heading line changed (heading replaced / inserted ok; the body cut to one line, another line gone, notes.md changed -> red); a product error in engine-runs stops the wait at once (product_stopped, not 60 s of waiting); a step whose prerequisite is not ok is blocked(#N) (not run, rc 6) while a failing step with an ok prerequisite stays red" "$P72_WANT" "$(p72 "$BG/rehearse.py")"
python3 -I - "$BG/rehearse.py" "$BG" "$$" <<'EOF72M'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
def mut(n, old, new):
    assert s.count(old) == 1, old
    open(f"{sys.argv[2]}/rh-b72{n}.{sys.argv[3]}.py", "w", encoding="utf-8").write(s.replace(old, new))
mut("a", '    if len(gone) > 1 or len(added) != 1 or "Shibubu Demo" not in added[0]:', '    if "Shibubu Demo" not in open(readme, encoding="utf-8").read():')  # 行の比較を外す（中身に題が在るかだけ）
mut("b", "        why = stop(els) if stop else None\n", "        why = None\n")  # 早抜けを外す
mut("c", 'elif (pre := prereq(res, r))', 'elif (pre := next((x for x in res if x["n"] == -1), None))')  # requires を見ない
mut("d", '    if (moved := sorted(', '    if False and (moved := sorted(')  # README 以外の書類を見ない
mut("e", '            if err and not (err.get("kind") == "denied" and err.get("pending")):', '            if err:')  # b75: 承認を頼む行（denied ＋ pending）も止まりと読む（r20 #20 の誤読）
mut("f", '    saved = lambda x: "menu" in x or re.search(', '    saved = lambda x: "menu" in x or False and re.search(')  # b75: cmd+s を保存と読まない
EOF72M
f72() { python3 -I -c 'import sys;a=sys.argv[1].split();b=sys.argv[2].split();print(",".join(x.split("=")[0] for x,y in zip(a,b) if x!=y) or "none")' "$P72_WANT" "$1"; }
check "b72 / b75 negative controls: line comparison removed -> one_line,extra_gone; early exit removed -> stopped,denied_nopend; requires ignored -> req_unm,req_red,req_chain; other-files check removed -> notes_changed; approval request read as a stop -> approval_race; cmd+s not read as save -> key_save" "one_line,extra_gone|stopped,denied_nopend|req_unm,req_red,req_chain|notes_changed|approval_race|key_save" "$(f72 "$(p72 "$BG/rh-b72a.$$.py")")|$(f72 "$(p72 "$BG/rh-b72b.$$.py")")|$(f72 "$(p72 "$BG/rh-b72c.$$.py")")|$(f72 "$(p72 "$BG/rh-b72d.$$.py")")|$(f72 "$(p72 "$BG/rh-b72e.$$.py")")|$(f72 "$(p72 "$BG/rh-b72f.$$.py")")"
rm -f "$BG"/rh-b72?.$$.py

# ---- W15-b74: TextEdit を背景に長く置くと App Nap で Apple Event が -1712 になる（r19 #5）→ open_textedit は -NSAppSleepDisabled YES を process の引数に渡す ----
te74() {  # te74 <rehearse.py> -> 渡された open の引数
  python3 -I - "$1" "$BG" <<'EOF74'
import importlib.util, sys, types
spec = importlib.util.spec_from_file_location("rh", sys.argv[1]); rh = importlib.util.module_from_spec(spec); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = rh; spec.loader.exec_module(rh)
calls = []
rh.subprocess = types.SimpleNamespace(run=lambda a, **k: calls.append(a) or types.SimpleNamespace(returncode=0, stdout='{"windows":[{"name":"README.md"}]}' if len(a) > 1 and a[1] == "once" else "", stderr=""))
seq = iter([{1}, {1, 2}, {1, 2}]); rh.textedit_pids = lambda: next(seq); rh.mine = lambda b, a: a - b
rh.time = types.SimpleNamespace(sleep=lambda s: None, time=lambda: 0.0)
rh.open_textedit("/x/README.md")
a = calls[0]; i = a.index("--args")
print(a[:i] == ["open", "-g", "-n", "-a", "TextEdit", "/x/README.md"], a[i + 1:])
EOF74
}
check "b74 open_textedit passes -NSAppSleepDisabled YES to the new TextEdit process only (--args; still -g -n and no persistence); negative control: the flag removed -> not passed" "True ['-ApplePersistenceIgnoreState', 'YES', '-NSAppSleepDisabled', 'YES']|True ['-ApplePersistenceIgnoreState', 'YES']" "$(te74 "$BG/rehearse.py")|$(python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o=", \"-NSAppSleepDisabled\", \"YES\"], check=True)";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"], check=True)"))' "$BG/rehearse.py" "$BG/rh-b74.$$.py" && te74 "$BG/rh-b74.$$.py")"
rm -f "$BG/rh-b74.$$.py"

# 102 通目: 録画する短い台本 docs/demo/short.md（9 手）。full.md の 27 手は回帰の試験のまま。python だけ（cargo 無し・.app 無し）
SH_MD="$BG/../../../docs/demo/short.md"
shp() {  # shp <rehearse.py> <台本 md> -> parse の結果（手の数と元の番号）か SystemExit の 1 行
  python3 -I - "$1" "$2" <<'EOF75'
import importlib.util, sys
s = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(s); s.loader.exec_module(m)
try:
    rows, raw = m.parse(sys.argv[2]); print(len(rows), raw, [r["orig"] for r in rows])
except SystemExit as e:
    print("exit:", str(e)[:100])
EOF75
}
check "short.md parses: 9 rows, 元の番号 = full.md の 1,5,9,12,15,17,18,19,20 (each key equals full.md's)" "9 9 [1, 5, 9, 12, 15, 17, 18, 19, 20]" "$(shp "$BG/rehearse.py" "$SH_MD")"
grep -v '^| 4 |' "$SH_MD" > "$T/short-nr.md"
check "short.md with a row deleted -> 手の数 mismatch exits" "exit: rehearse: 台本の「手の数: 9」≠ 表の手 8" "$(shp "$BG/rehearse.py" "$T/short-nr.md")"
sed 's/^元の番号: 1,5,/元の番号: 1,6,/' "$SH_MD" > "$T/short-bo.md"
check "short.md with a wrong 元の番号 (full.md #6 is not goto_tab) -> exits" "exit: rehearse: 2: 元の番号 6 の手は full.md では fg_textedit・この台本では goto_tab" "$(shp "$BG/rehearse.py" "$T/short-bo.md")"
pq() {  # pq <rehearse.py> -> short の 2 手目（元の 5）と 3 手目（元の 10 = 前提は元の 9）で、前提が赤の時に 3 手目が前提の手を見つけるか / 番号が同じだけの別の手を見つけないか
  python3 -I - "$1" <<'EOF76'
import importlib.util, sys
s = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(s); s.loader.exec_module(m)
res = [dict(n=1, orig=9, status="赤: x"), dict(n=2, orig=10, status="ok")]
print((m.prereq(res, dict(n=3, orig=10)) or {}).get("n"), (m.prereq(res, dict(n=9, orig=3)) or {}).get("n"), (m.prereq([dict(n=9, orig=20, status="ok")], dict(n=9, orig=20)) or {}).get("n"))
EOF76
}
sed 's/x.get("orig", x\["n"\]) == REQUIRES.get(r.get("orig", r\["n"\]))/x["n"] == REQUIRES.get(r["n"])/' "$BG/rehearse.py" > "$BG/rh-b102.$$.py"
check "REQUIRES is looked up by the full.md number (元の番号), not the script's own row number; negative control: looked up by row number -> the short script's numbers point at the wrong rows" "1 None None|None None None" "$(pq "$BG/rehearse.py")|$(pq "$BG/rh-b102.$$.py")"
rm -f "$BG/rh-b102.$$.py"
rs() { python3 -I "$BG/rehearse.py" "$1" --dry-run >/dev/null 2>&1; echo $?; }
check "dry-run is green for short (9) / full (27) / scene2 (5)" "0 0 0" "$(rs short) $(rs 1) $(rs scene2)"
check "record.sh --script: short -> 9 rows with 2 owner, full -> 27, an unknown script -> usage 2" "9 2 | 27 | 2" "$(bash "$BG/record.sh" /x --script short --take 1 --dry-run | grep -c '^ *[0-9]*\. ') $(bash "$BG/record.sh" /x --script short --take 1 --dry-run | grep -c '\[owner\]$') | $(bash "$BG/record.sh" /x --script full --take 1 --dry-run | grep -c '^ *[0-9]*\. ') | $(bash "$BG/record.sh" /x --script nope --take 1 --dry-run >/dev/null 2>&1; echo $?)"
# REC-SHORT: --script を付けない record.sh は録画する short（9 手・owner の手 2）。full（27 手）は --script full の回帰の試験
rdf() { bash "$1" /x --take 1 --dry-run | awk '/^ *[0-9]+\. /{n++} /\[owner\]$/{o++} /台本=/{s=$0; sub(/.*台本=/, "", s); sub(/ .*/, "", s)} END{print n+0, o+0, s}'; }
sed 's/script=short dry=0/script=full dry=0/' "$BG/record.sh" > "$BG/record-recshort.$$.sh"
check "REC-SHORT: record.sh with no --script dry-runs the short script (9 rows, 2 owner, 台本=short); negative control: the default back to full -> the 27 rows" "9 2 short|27 5 full" "$(rdf "$BG/record.sh")|$(rdf "$BG/record-recshort.$$.sh")"
rm -f "$BG/record-recshort.$$.sh"
# REC-10: 録画の take だけ AI の呼びの上限を 10（製品の既定）に。REHEARSE_LLM_CALLS が空の時だけ record.sh が置き、外から渡せばそちら。偽の rehearse.py が受け取った env を書く（1 秒寝る）・screencapture も偽（録らない・下の B77 の形）
rec10() {  # rec10 <record.sh> [REHEARSE_LLM_CALLS の値] -> rehearse.py が受け取った値（env に無ければ "-"・走らなければ none）
  local d="$T/rec10"; rm -rf "$d"; mkdir -p "$d/bg" "$d/bin" "$d/app"; cp "$1" "$d/bg/record.sh"
  printf '#!/bin/bash\necho "{ %s 1.00 1.00 }"\n' "${REC_LOAD:-1.00}" > "$d/bin/sysctl"; chmod +x "$d/bin/sysctl"  # load1 は REC_LOAD（既定 1.00・B77 の門の試験が上げる）
  printf '%s\n' 'import os, sys, time' 'o = sys.argv[sys.argv.index("--out") + 1]' 'open(os.path.join(o, "calls"), "w").write(os.environ.get("REHEARSE_LLM_CALLS", "-"))' \
    'try: os.fstat(3); fd3 = "open"' 'except OSError: fd3 = "closed"' 'open(os.path.join(o, "fd3"), "w").write(fd3)' 'time.sleep(1)' > "$d/bg/rehearse.py"
  # 本物の screencapture -v の止まり方を写す: stdin の 1 文字か EOF で止まる・自分で SIGINT の handler を置く（& の子の SIGINT 無視を上書き）。.mov の代わりに録った秒を書く
  printf '%s\n' '#!/usr/bin/env python3' 'import signal, sys, time' 't0 = time.time()' 'def stop(*_):' '    open(sys.argv[-1], "w").write("%.1f" % (time.time() - t0)); sys.exit(0)' \
    'signal.signal(signal.SIGINT, stop)' 'sys.stdin.read(1)' 'stop()' > "$d/bin/screencapture"; chmod +x "$d/bin/screencapture"
  ( unset REHEARSE_LLM_CALLS; if [ $# -gt 1 ]; then export REHEARSE_LLM_CALLS=$2; fi
    PATH="$d/bin:$PATH" bash "$d/bg/record.sh" "$d/app" --script short --take 1 --out "$d/out" >"$d/rec.out" 2>&1; echo $? > "$d/rec.rc" )
  cat "$d/out/run-take-1/calls" 2>/dev/null || echo none
}
sed 's/ REHEARSE_LLM_CALLS="\$REC_LLM_CALLS"//' "$BG/record.sh" > "$BG/record-rec10a.$$.sh"
sed 's/^REC_LLM_CALLS=.*/REC_LLM_CALLS=10/' "$BG/record.sh" > "$BG/record-rec10b.$$.sh"
check "REC-10: record.sh passes REHEARSE_LLM_CALLS=10 to rehearse.py when it is empty, and an outside value (7) wins; dry-run shows the cap; negative controls: the env not passed -> rehearse.py sees none (its own default 8) / 10 hard-coded -> the outside 7 is lost" \
  "10 7 10 | - 7 | 10 10" "$(rec10 "$BG/record.sh") $(rec10 "$BG/record.sh" 7) $(bash "$BG/record.sh" /x --script short --take 1 --dry-run | sed -n 's/.*AI の上限 \([0-9]*\) 回.*/\1/p') | $(rec10 "$BG/record-rec10a.$$.sh") $(rec10 "$BG/record-rec10a.$$.sh" 7) | $(rec10 "$BG/record-rec10b.$$.sh") $(rec10 "$BG/record-rec10b.$$.sh" 7)"
rm -f "$BG/record-rec10a.$$.sh" "$BG/record-rec10b.$$.sh"
# B77: 本物の screencapture -v は & の子の stdin（/dev/null）の EOF で 1 コマ（0.03 秒）録って止まっていた（load1 2.3 の本物の probe: 今の形 0.03 秒 / stdin を開いたまま 2.8 秒・INT で 1.1 秒）。偽は同じ止まり方で、録った秒を .mov に書く
rec77() {  # rec77 <record.sh> -> "<録画 long（0.8 秒以上）| short> <rehearse.py の fd 3 open | closed>"
  rec10 "$1" >/dev/null; local d="$T/rec10"
  echo "$(python3 -I -c 'import sys; print("long" if float(sys.argv[1]) >= 0.8 else "short")' "$(cat "$d/out/take-1.mov" 2>/dev/null)" 2>/dev/null || echo none) $(cat "$d/out/run-take-1/fd3" 2>/dev/null || echo none)"
}
sed -e 's/ <"\$scin" &/ \&/' -e '/^exec 3>"\$scin"/d' "$BG/record.sh" > "$BG/record-b77a.$$.sh"
sed 's/ 3>&-$//' "$BG/record.sh" > "$BG/record-b77b.$$.sh"
check "B77: record.sh gives screencapture -v a stdin it holds open, so the recording runs until the INT after rehearse.py (not one frame), and rehearse.py does not inherit the write end; negative controls: the old '& with /dev/null stdin' -> short (EOF stops it at once) / without 3>&- -> rehearse.py holds fd 3 open" \
  "long closed | short closed | long open" "$(rec77 "$BG/record.sh") | $(rec77 "$BG/record-b77a.$$.sh") | $(rec77 "$BG/record-b77b.$$.sh")"
rm -f "$BG/record-b77a.$$.sh" "$BG/record-b77b.$$.sh"
# B77 の門: load1 が 10 を超えていたら録画を起こさない（1 行で終わる・exit 6）。--dry-run は門の外
rec77g() {  # rec77g <record.sh> <load1> -> "<exit> <rehearse.py が走った ran | none> <混んでいる行の数>" ＋ load1 が高い時は dry-run の exit
  local d="$T/rec10"; REC_LOAD=$2 rec10 "$1" >/dev/null
  echo "$(cat "$d/rec.rc") $([ -f "$d/out/run-take-1/calls" ] && echo ran || echo none) $(grep -c -F "機械が混んでいます（load $2）。下がってから打ってください" "$d/rec.out")$([ "$2" = 1.00 ] || echo " dry=$(PATH="$d/bin:$PATH" bash "$1" /x --script short --take 1 --dry-run >/dev/null 2>&1; echo $?)")"
}
sed '/機械が混んでいます/d' "$BG/record.sh" > "$BG/record-b77g.$$.sh"
check "B77 gate: load1 1.00 -> records (rehearse.py runs); load1 42.00 -> exit 6 with the one 'busy' line and nothing started, while --dry-run still works; negative control: the gate removed -> it starts at load1 42.00" \
  "0 ran 0|6 none 1 dry=0|0 ran 0 dry=0" "$(rec77g "$BG/record.sh" 1.00)|$(rec77g "$BG/record.sh" 42.00)|$(rec77g "$BG/record-b77g.$$.sh" 42.00)"
rm -f "$BG/record-b77g.$$.sh"

# 102 通目の追加: 偽の CLI は主デモの一言の計画にも答える（FX-006 で AI の失敗は名前の表で続かない）・Terminal の bash の prompt に機械名と user 名を映さない
fk() {  # fk <fake-claude.sh> <一言> -> "<exit> <name or none>"
  local d="$T/fk.$$" out; rm -rf "$d"; mkdir -p "$d"; cp "$1" "$d/claude"
  out=$(printf '%s' "$2" | bash "$d/claude" -p --json-schema '{"title":"shibubu_project_plan"}' 2>/dev/null); local rc=$?
  echo "$rc $(printf '%s' "$out" | python3 -I -c 'import json,sys
try: print(json.load(sys.stdin)["structured_output"]["name"])
except Exception: print("none")')"
}
sed 's/elif grep -q .Set up a dev environment. "$here\/calls.${n}.stdin"; then kind=plan_main;/elif false; then kind=plan_main;/' "$BG/fake-claude.sh" > "$T/fake-nomain.sh"
check "fake claude: the main demo sentence -> plan_main (name 'Dev environment'), the band sentence -> plan_band, another sentence -> exit 1; negative control: the plan_main branch removed -> the main sentence exits 1" "0 Dev environment|0 Band session|1 none|1 none" "$(fk "$BG/fake-claude.sh" 'Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. Put each on its own page.')|$(fk "$BG/fake-claude.sh" 'Set up a band session: record in REAPER')|$(fk "$BG/fake-claude.sh" 'hello')|$(fk "$T/fake-nomain.sh" 'Set up a dev environment for this repo.')"
trap_env "$T/run-ps1" > "$T/env-ps1"
bp() {  # bp <env の file> -> bash -i --norc に run の env だけ渡した時の PS1
  local e=(); while IFS= read -r l; do case $l in PS1=*) e+=("$l");; esac; done < "$1"
  env -i PATH=/usr/bin:/bin ${e[@]+"${e[@]}"} /bin/bash --norc -i -c 'printf "[%s]" "$PS1"' 2>/dev/null
}
grep -v '^PS1=' "$T/env-ps1" > "$T/env-nops1"
check "trap env gives a non-login bash (--norc) PS1='\$ '; a login bash resets it in /etc/bashrc (B71 below); negative control: the PS1 line removed -> bash's default prompt" "[\$ ]|[\\s-\\v\\\$ ]" "$(bp "$T/env-ps1")|$(bp "$T/env-nops1")"
# B71: 製品の Terminal は login shell（TerminalConfig の -l -i）。login の bash は /etc/profile → /etc/bashrc が env の PS1 を「<機械名>:<dir> <利用者名>$」に戻す（cu.vKn8 の ax 16〜18）。
# 録画 mode の env だけ PROMPT_COMMAND で prompt の直前に毎回 PS1 を短くする。製品と同じ -l -i で、捨ての HOME（owner の rc を読まない・履歴も捨ての HOME に）で回す。名は出さず数と <H> / <U> だけ
b71() {  # b71 <SHIBUBU_QA_RECORD の値> [run.sh] -> "<機械名・利用者名の当たりの行> [<prompt の行（名は <H> / <U> に）>]"
  local R71="$T/run-b71.$1" H71="$T/h71" e=() l out
  rm -rf "$R71" "$H71"; mkdir -p "$H71"
  while IFS= read -r l; do case $l in PS1=*|PROMPT_COMMAND=*) e+=("$l");; esac; done < <( source "${2:-$BG/run.sh}"; SHIBUBU_QA_RECORD=$1 trap_env "$R71" )
  out=$(cd "$H71" && env -i HOME="$H71" PATH=/usr/bin:/bin TERM=dumb BASH_SILENCE_DEPRECATION_WARNING=1 ${e[@]+"${e[@]}"} /bin/bash -l -i <<< 'exit' 2>&1 >/dev/null)
  echo "$(printf '%s\n' "$out" | grep -c -F -e "$(hostname -s)" -e "$(id -un)") [$(printf '%s\n' "$out" | grep 'exit$' | sed -e "s/$(hostname -s)/<H>/g" -e "s/$(id -un)/<U>/g")]"
}
grep -v -F "rec && echo \"PROMPT_COMMAND=PS1='\\\$ '\"" "$BG/run.sh" > "$BG/run-b71.$$.sh"
check "B71: record mode -> the Terminal's login bash (-l -i as the product) shows the prompt '\$ ' with no machine / user name although /etc/bashrc resets PS1; not record -> as before; negative control: the PROMPT_COMMAND line removed -> /etc/bashrc's '<H>:~ <U>\$' (cu.vKn8 ax 16-18)" \
  '0 [$ exit]|1 [<H>:~ <U>$ exit]|1 [<H>:~ <U>$ exit]' "$(b71 1)|$(b71 '')|$(b71 1 "$BG/run-b71.$$.sh")"
rm -f "$BG/run-b71.$$.sh"

# 104 通目 AI-REAL: 本物の AI が既定（偽は REHEARSE_LLM=fake だけ）・上限は台本ごと・偽で回した run は run.json と報告の頭の行に `AI: fake`
air() {  # air <rehearse.py> -> 台本ごとの上限 | 既定の門 | 上限の置き換え | 刻み
  python3 -I - "$1" "$T" "$BG" <<'EOF77'
import importlib.util, json, os, sys
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[3]); sys.modules["rh"] = m; sp.loader.exec_module(m)
base = os.path.join(sys.argv[2], f"air-{os.getpid()}"); os.makedirs(base, exist_ok=True)
cli = os.path.join(base, "claude"); open(cli, "w").write("#!/bin/sh\n"); os.chmod(cli, 0o755)
for k in ("REHEARSE_LLM", "SHIBUBU_QA_LLM_OK", "REHEARSE_LLM_CALLS", "REHEARSE_LLM_SECS"): os.environ.pop(k, None)
os.environ.update(SHIBUBU_QA_LLM_OK="ok", SHIBUBU_LLM_CLI=cli)
out = []
for s in ("short", "full", "scene2"):
    m.SCRIPT = s; out.append(m.llm_env()["SHIBUBU_LLM_BUDGET"])
os.environ.update(REHEARSE_LLM_CALLS="2"); m.SCRIPT = "short"; out.append(m.llm_env()["SHIBUBU_LLM_BUDGET"]); os.environ.pop("REHEARSE_LLM_CALLS")
os.environ.pop("SHIBUBU_QA_LLM_OK")
try: m.llm_env(); gate = "open"
except m.LlmGate as e: gate = f"gate{e.rc}"
os.environ["REHEARSE_LLM"] = "fake"; fake = m.llm_env()
res = []
for mode in ("fake", "real"):
    os.environ["REHEARSE_LLM"] = mode; R = os.path.join(base, mode); os.makedirs(R, exist_ok=True); json.dump({"window_id": 1}, open(R + "/run.json", "w"))
    res.append(f"{m.mark_ai(R)}:{json.load(open(R + '/run.json'))['ai']}")
print("|".join(out), gate, fake, "|".join(res))
EOF77
}
AIR_WANT="calls=8,secs=300|calls=12,secs=900|calls=8,secs=600|calls=2,secs=300 gate8 None AI: fake:fake|AI: real:real"
sed 's/    d\["ai"\] = llm_mode()/    d["ai"] = "real"/' "$BG/rehearse.py" > "$BG/rh-b104a.$$.py"
sed 's/^LLM_BUDGET = {"full": (12, 900), "short": (8, 300), "scene2": (8, 600)}/LLM_BUDGET = {"full": (12, 900), "short": (12, 900), "scene2": (12, 900)}/' "$BG/rehearse.py" > "$BG/rh-b104b.$$.py"
sed 's/    return "fake" if os.environ.get("REHEARSE_LLM") == "fake" else "real"/    return "real" if os.environ.get("REHEARSE_LLM") == "real" else "fake"/' "$BG/rehearse.py" > "$BG/rh-b104c.$$.py"
check "AI-REAL: per-script budgets (short 8/300, full 12/900, scene2 8/600; REHEARSE_LLM_CALLS wins), no ok -> gate 8 by default, fake -> no real env, mark_ai writes run.json ai and returns 'AI: <mode>'" "$AIR_WANT" "$(air "$BG/rehearse.py")"
check "AI-REAL negative controls: ai not read from the mode -> the fake mark says real; every script on the full budget -> the short budget differs; default fake (not real) -> the gate opens" "differ|differ|differ" "$([ "$(air "$BG/rh-b104a.$$.py")" != "$AIR_WANT" ] && echo differ || echo same)|$([ "$(air "$BG/rh-b104b.$$.py")" != "$AIR_WANT" ] && echo differ || echo same)|$([ "$(air "$BG/rh-b104c.$$.py" 2>/dev/null)" != "$AIR_WANT" ] && echo differ || echo same)"
rm -f "$BG"/rh-b104?.$$.py
check "AI-REAL: the live report prints the AI line first (print(ai_line) once); negative control: the line removed -> 0" "1 0" "$(grep -c '^    print(ai_line)' "$BG/rehearse.py") $(grep -v '^    print(ai_line)' "$BG/rehearse.py" | grep -c '^    print(ai_line)')"

# SHORT-TE: short.md には mirror_consent が無く、README の TextEdit が開かれなかった（te_pid None → #2 #7 #9 が未測）。開く条件 = README の TextEdit を使う BG の手（Mirror here の goto_tab・ask_ai・mirror_consent）
ste() {  # ste <rehearse.py> -> "<使う手の判定 4 つ> <理由の文 2 つ> <short の dry-run の段取りの数> <full の段取りの数>"
  python3 -I - "$1" "$BG" <<'EOF78'
import importlib.util, subprocess, sys
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
row = lambda key, doing="": dict(who="BG", key=key, doing=doing)
use = [m.uses_readme_te(r) for r in (row("goto_tab", "『Docs』のタブを押し、`Mirror here` を押して"), row("ask_ai"), row("goto_tab", "『Diagram』のタブを押す"), row("mirror_consent"))]
C = type("C", (), {"state": {}})(); a = m.te_missing(C); C.state["te_why"] = "README の TextEdit を開いたが、自分の pid が 1 つに決まらない（0 個）"; b = m.te_missing(C)
n = lambda s: subprocess.run([sys.executable, "-I", sys.argv[1], s, "--dry-run"], capture_output=True, text=True).stdout.count("TextEdit で README.md を open")
print(use, "開いていない" in a and "決まらない" not in a, "決まらない" in b and "開いていない" not in b, n("short"), n("1"))
EOF78
}
sed 's/    return r\["who"\] == "BG" and (r\["key"\] in SETUP or r\["key"\] == "ask_ai" or (r\["key"\] == "goto_tab" and "Mirror here" in r\["doing"\]))/    return r["who"] == "BG" and r["key"] in SETUP/' "$BG/rehearse.py" > "$BG/rh-b105a.$$.py"
check "SHORT-TE: the README TextEdit is opened for a Mirror-here goto_tab / ask_ai / mirror_consent hand (not for a plain tab), short's dry-run shows the step once and full's once; te_missing tells 'not opened' from 'opened but pid not single'; negative control: the condition back to mirror_consent only -> short shows 0" "[True, True, False, True] True True 1 1|[False, False, False, True] True True 0 1" "$(ste "$BG/rehearse.py")|$(ste "$BG/rh-b105a.$$.py")"
rm -f "$BG"/rh-b105?.$$.py

# GROK-TE: 新しい TextEdit の pid だけでは te_pid を置かない。窓の題が file 名で始まるまで待ち、出なければその pid だけ終了して 1 回開き直す。2 回とも出なければ未測。
tw() {  # tw <rehearse.py> -> "ready retry miss notes notes-miss <未測の文>"
  python3 -I - "$1" "$T" <<'EOFTE'
import importlib.util, json, os, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sp.loader.exec_module(m)
T = os.path.join(sys.argv[2], "tw-" + str(os.getpid())); hd = T + "/hd"; os.makedirs(hd + "/bin")
open(hd + "/bin/windows", "w").write("""#!/usr/bin/env python3
import json, os, sys
pid = sys.argv[sys.argv.index("--pid") + 1]
plan = json.load(open(os.path.join(os.path.dirname(__file__), "plan.json")))
print(json.dumps({"windows": [{"name": plan.get(pid, "Open")}]}))
""")
os.chmod(hd + "/bin/windows", 0o755)
m.HERE = hd
clock, live, opens, kills, wins, nxt = [0.0], {50}, [], [], [0], [1000]
real_run, real_kill, real_time, real_sleep = m.subprocess.run, m.os.kill, m.time.time, m.time.sleep
def run(a, **k):
    if a[0] == "pgrep":
        return types.SimpleNamespace(returncode=0, stdout=" ".join(str(p) for p in sorted(live)) + "\n", stderr="")
    if a[0] == "open":
        opens.append(a); nxt[0] += 1; live.add(nxt[0])
        return types.SimpleNamespace(returncode=0, stdout="", stderr="")
    if len(a) > 1 and a[1] == "once":
        wins[0] += 1
    return real_run(a, **k)
m.subprocess.run = run
m.os.kill = lambda pid, sig: (kills.append(pid), live.discard(pid))
m.time.time = lambda: clock[0]
m.time.sleep = lambda s: clock.__setitem__(0, clock[0] + s)
def one(kind, plan):
    clock[0] = 0.0; live.clear(); live.add(50); opens.clear(); kills.clear(); wins[0] = 0; nxt[0] = 1000
    json.dump({str(k): v for k, v in plan.items()}, open(hd + "/bin/plan.json", "w"))
    C = m.Ctx(os.path.join(T, kind), "X.app", "n1"); C.state["te_sets"] = []
    if kind.startswith("notes"):
        m.open_notes(C); pid = C.state.get("notes_pid"); msg = "-"
    else:
        m.place_te(C, m.open_textedit(os.path.join(C.repo, "README.md")), "te_pid")
        pid = C.state.get("te_pid"); msg = m.te_missing(C) if not pid else "-"
    return f"{1 if pid else 0},{len(opens)},{len(kills)},{wins[0]}", msg
try:
    rows = [("ready", {1001: "README.md"}), ("retry", {1001: "Open", 1002: "README.md"}), ("miss", {1001: "Open", 1002: "Open"}), ("notes", {1001: "notes.md"}), ("notesmiss", {1001: "Open", 1002: "Open"})]
    bits, msg = [], "-"
    for kind, plan in rows:
        bit, m_ = one(kind, plan); bits.append(bit)
        if kind == "miss":
            msg = m_
    print(" ".join(bits), msg)
finally:
    m.subprocess.run, m.os.kill, m.time.time, m.time.sleep = real_run, real_kill, real_time, real_sleep
EOFTE
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="        if any((w.get(\"name\") or \"\").startswith(title) for w in ws):\n            return True";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"        if pid:\n            return True",1))' "$BG/rehearse.py" "$BG/rh-te.$$.py"
check "GROK-TE: a file-named window on the first open keeps te_pid; Open then the file name reopens once and keeps the new pid; Open twice leaves te_pid unset and te_missing names the miss; notes.md uses the same wait; negative control: accept the pid without the title -> the Open-twice case keeps a pid" "1,1,0,1 1,2,1,22 0,2,2,42 1,1,0,1 0,2,2,42 TextEdit が README.md の窓を出さなかった（Open の窓のまま・2 回）|1,1,0,1 1,1,0,1 1,1,0,1 1,1,0,1 1,1,0,1 -" "$(tw "$BG/rehearse.py")|$(tw "$BG/rh-te.$$.py")"
rm -f "$BG/rh-te.$$.py"

# TALK-1101（owner 191 通目・L2 は 118 通目・YC_DECISIONS 109 通目 15・111 通目）: Home の一言は会話になり、作るのは案の card の先頭の Build now だけ（最初から出る・Preview plan は押さない）。
# 偽の木（src の AX の名前: shell.rs enter_label の `↵ Send` → `Send`・domain/mod.rs chat::BUILD_NOW / PREVIEW_PLAN）の上で本物の find / press を通す（elements と step だけ差し替え・実機を起こさない）。
# 場面: needs = 答えの要る問いの間は Build now が灰で `Answer this first: 問い`（agent_rail.rs Card::plan）・slow = 問いの無い灰（CLI を確かめている・AI が読んでいる）が 30 回続いてから押せる・
# early = 押す前に Project ができている（fresh_projects だけ差し替え）
# 打つ場所（L1-NORAIL R2 = 欄は下の panel の底の 1 行・どの tab でも）は AX の名前が同じ `Change this project` なので手は変わらない
l12() {  # l12 <rehearse.py> [plain|needs|slow|early] -> "<h_generate が押した token の列>|<結果>"
  python3 -I - "$1" "$BG" "$T/l12" "${2:-plain}" <<'EOFL12'
import importlib.util, os, sys, time, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
m.time = types.SimpleNamespace(time=time.time, sleep=lambda s: None)
E = lambda role, label, tok, **k: {"role": role, "label": label, "element_token": tok, "enabled": True, **k}
st, clicks, scene = {"stage": 0}, [], sys.argv[4]
def els(C):  # Home（L2）: 欄と右端の Send → 押すと案の card（先頭の Build now・Change plan・Later・Preview plan）→ Build now を押すと Project の欄
    k = st["stage"]
    if k == 0: return [E("AXTextField", "What to create", "t-goal"), E("AXButton", "Send", "t-send"), E("AXButton", "Attach", "t-attach")]
    if k == 1 and scene == "needs":  # 灰の Build now は cua-driver の elements に無く tree_markdown にだけ（token 無し = md・demoshort.P3V4 の 34-get_window_state の形）
        return [E("AXTextField", "What to create", "t-goal"), {"role": "AXButton", "label": "Build now", "md": True}, E("AXButton", "Change plan", "t-chg"),
                E("AXButton", "Preview plan", "t-preview"), {"role": "AXStaticText", "value": "Answer this first: Which repo?", "md": True}]
    if k == 1:
        st["grey"] = st.get("grey", 0) + 1
        on = st["grey"] > (30 if scene == "slow" else 0)
        build = E("AXButton", "Build now", "t-build") if on else {"role": "AXButton", "label": "Build now", "md": True}
        return [E("AXTextField", "What to create", "t-goal"), build, E("AXButton", "Change plan", "t-chg"), E("AXButton", "Later", "t-later"), E("AXButton", "Preview plan", "t-preview")]
    return [E("AXTextField", "Change this project", "t-field")]
def step(C, tool, args, *extra, own=False):
    if tool == "click":
        clicks.append(args["element_token"]); st["stage"] += 1
    return {}
m.elements, m.step = els, step
m.send_field = lambda C_, role, label, text, enter=False, expect=None: None
if scene == "early": m.fresh_projects = lambda C_, since: [{"id": "p-early"}] if st["stage"] >= 1 else []
C = m.Ctx(os.path.join(sys.argv[3], "g"), "X.app", "n"); os.makedirs(C.R + "/data", exist_ok=True)
try:
    m.h_generate(C, {}); r = "ok"
except m.Unmeasured as e:
    r = "unm" + ("+q" if "Which repo?" in str(e) else "")  # DB は無い → I1 は未測（押す順だけを見る）・+q = 灰の理由に問いの原文が付いた
except RuntimeError:
    r = "red"
print(",".join(clicks) + "|" + r)
EOFL12
}
mkdir -p "$T/l12"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="    press(C, \"AXButton\", \"send\")\n";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"    press(C, \"AXButton\", \"create\")\n"))' "$BG/rehearse.py" "$BG/rh-l12a.$$.py"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="press(C, \"AXButton\", \"accept\", e=";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"press(C, \"AXButton\", \"preview_plan\", e=None) or (lambda e: None)(e="))' "$BG/rehearse.py" "$BG/rh-l12b.$$.py"
check "L2: h_generate presses Send -> Build now (no Preview plan) on the L2 Home tree (I1 unmeasured: no DB); negative controls: the old Create press -> nothing pressed, unmeasured (the L2 tree has no Create); Preview plan pressed instead of Build now -> no Project field, unmeasured" "t-send,t-build|unm#|unm#t-send,t-preview|unm" "$(l12 "$BG/rehearse.py")#$(l12 "$BG/rh-l12a.$$.py" 2>/dev/null)#$(l12 "$BG/rh-l12b.$$.py" 2>/dev/null)"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="range(int(40 * llm_scale()))";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"range(20)"))' "$BG/rehearse.py" "$BG/rh-l12c.$$.py"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="if build and all(e.get(\"md\") or e.get(\"enabled\") is False for e in build):";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"if False:"))' "$BG/rehearse.py" "$BG/rh-l12d.$$.py"
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="if (made := fresh_projects(C, t_send)):";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"if False:"))' "$BG/rehearse.py" "$BG/rh-l12e.$$.py"
check "L2 gate tree: a Project made before Build now -> red, Build now not pressed; negative control: no Project check before the press -> Build now pressed" "t-send|red#t-send,t-build|unm" "$(l12 "$BG/rehearse.py" early)#$(l12 "$BG/rh-l12e.$$.py" early 2>/dev/null)"
check "L2 gate tree: a grey Build now with Answer this first (cua-driver shows it only in tree_markdown, no token) -> unmeasured at once with the question; Build now grey without a question for 30 reads (waiting) -> still pressed; negative controls: a 20-read wait -> Build now never pressed; no grey check -> waits for a token that never comes, unmeasured without the question" "t-send|unm+q#t-send,t-build|unm#t-send|unm#t-send|unm" "$(l12 "$BG/rehearse.py" needs)#$(l12 "$BG/rehearse.py" slow)#$(l12 "$BG/rh-l12c.$$.py" slow 2>/dev/null)#$(l12 "$BG/rh-l12d.$$.py" needs 2>/dev/null)"
check "L2 short.md / full.md / scene2.md hand 1: says ↵ Send → Build now, no Create · ⌘↵; negative control: Create put back in short.md -> 1" "0 0 0 1" "$(for f in short full scene2; do grep '^| 1 | BG | generate |' "$BG/../../../docs/demo/$f.md" | grep -c 'Create · ⌘↵\|`Create`' ; done | paste -sd' ' -) $(grep '^| 1 | BG | generate |' "$BG/../../../docs/demo/short.md" | sed 's/`↵ Send`/`Create · ⌘↵`/' | grep -c 'Create · ⌘↵')"
rm -f "$BG"/rh-l12?.$$.py

# L7（yc/snap-b70 696ef28c・src/ui/terminal_panel.rs）: Terminal の下の欄は既定で隠れる → 帯の •••（Terminal options）の Input field (for IME) で出す。押すたびに出る / 隠れる
# 偽の木（場面 = 隠れている・出ている・••• の無い古い .app・畳んだ帯）の上で本物の term_field / press / find を通す（elements と step だけ差し替え）
l7() {  # l7 <rehearse.py> -> "<場面>:<押した token の列> ..."
  python3 -I - "$1" "$BG" "$T/l7" <<'EOFL7'
import importlib.util, os, sys, time, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
m.time = types.SimpleNamespace(time=time.time, sleep=lambda s: None)
E = lambda role, label, tok: {"role": role, "label": label, "element_token": tok, "enabled": True}
out = []
for scene in ("hidden", "shown", "old", "folded"):
    st = {"menu": False, "field": scene == "shown"}; clicks = []
    def els(C):
        t = [E("AXStaticText", "zsh · pid 4242", "t-grid"), E("AXButton", "Show terminal" if scene == "folded" else "Hide terminal", "t-drawer")]
        if scene != "old": t.append(E("AXButton", "Terminal options", "t-opts"))
        if st["menu"]: t.append(E("AXButton", "Input field (for IME)", "t-ime"))
        if st["field"]: t.append(E("AXTextField", "Type in the terminal", "t-field"))
        return t
    def step(C, tool, args, *extra, own=False):
        if tool == "click":
            k = args["element_token"]; clicks.append(k)
            if k == "t-opts": st["menu"] = not st["menu"]
            if k == "t-ime": st["menu"] = False; st["field"] = not st["field"]
        return {}
    m.elements, m.step = els, step
    C = m.Ctx(os.path.join(sys.argv[3], scene), "X.app", "n")
    m.term_field(C)
    out.append(f"{scene}:{','.join(clicks)}{'+field' if st['field'] else ''}")
print(" ".join(out))
EOFL7
}
mkdir -p "$T/l7"
for x in "a|seen(\"AXTextField\", \"term_in\") or |" "b| or seen(\"AXButton\", \"Show terminal\")|"; do
  python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o=sys.argv[3];assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,sys.argv[4],1))' "$BG/rehearse.py" "$BG/rh-l7${x%%|*}.$$.py" "$(cut -d'|' -f2 <<< "$x")" "$(cut -d'|' -f3 <<< "$x")"
done
check "L7: term_field opens the hidden terminal field once (Terminal options -> Input field (for IME)); leaves a shown field, an old .app without the ••• and a folded strip alone; negative controls: no shown-field guard -> the shown field is hidden; no folded guard -> the folded strip is toggled" \
  "hidden:t-opts,t-ime+field shown:+field old: folded:#hidden:t-opts,t-ime+field shown:t-opts,t-ime old: folded:#hidden:t-opts,t-ime+field shown:+field old: folded:t-opts,t-ime+field" \
  "$(l7 "$BG/rehearse.py")#$(l7 "$BG/rh-l7a.$$.py" 2>/dev/null)#$(l7 "$BG/rh-l7b.$$.py" 2>/dev/null)"
rm -f "$BG"/rh-l7?.$$.py

# B60-TERM: 手 #3 / #4 の h_term_type を「欄が隠れた」偽の木で通す（L7 の後の既定の .app）。打つ先は欄のまま: grid の AX は Label で SetValue の受け口が無く
# （components.rs の terminal_grid）、grid が打鍵を PTY へ送るのは窓が key の時だけ（resp.has_focus = egui 0.31 の i.focused && focus）= BG の手は grid に打てない。
# 負の対照: h_term_type が term_field を呼ばない（L7 の前の rehearse.py = trunk 8060b357 の形）→ 欄が無いで未測
l7h() {  # l7h <rehearse.py> -> "<押した token・打った字・押した key の列>" か "unm:<理由>"
  python3 -I - "$1" "$BG" "$T/l7h" <<'EOFL7H'
import importlib.util, os, sys, time, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
m.time = types.SimpleNamespace(time=time.time, sleep=lambda s: None)
E = lambda role, label, tok: {"role": role, "label": label, "element_token": tok, "enabled": True}
st = {"menu": False, "field": False}; log = []
def els(C):
    t = [E("AXStaticText", "zsh · pid 4242", "t-grid"), E("AXButton", "Hide terminal", "t-drawer"), E("AXButton", "Terminal options", "t-opts")]
    if st["menu"]: t.append(E("AXButton", "Input field (for IME)", "t-ime"))
    if st["field"]: t.append(E("AXTextField", "Type in the terminal", "t-field"))
    return t
def step(C, tool, args, *extra, own=False):
    k = args.get("element_token")
    log.append(k if tool == "click" else "type=" + args["text"] if tool == "type_text" else args.get("key", tool))
    if tool == "click" and k == "t-opts": st["menu"] = not st["menu"]
    if tool == "click" and k == "t-ime": st["menu"] = False; st["field"] = not st["field"]
    return {}
m.elements, m.step = els, step
try:
    m.h_term_type(m.Ctx(sys.argv[3], "X.app", "n1"), {"doing": "Terminal に『echo <nonce>』『git status』を打つ"})
    print(",".join(log))
except m.Unmeasured as e:
    print("unm:" + str(e))
EOFL7H
}
python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o="    term_field(C)\n";assert t.count(o)==1;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,"",1))' "$BG/rehearse.py" "$BG/rh-l7h.$$.py"
check "B60-TERM: h_term_type on the default tree (terminal field hidden) opens it via Terminal options -> Input field (for IME) once, then types each line into Type in the terminal + Return; negative control: no term_field (the pre-L7 rehearse.py) -> unmeasured, no field" \
  "t-opts,t-ime,t-field,type=echo n1,return,t-field,type=git status,return#unm:AXTextField「term_in」が AX に出ない" \
  "$(l7h "$BG/rehearse.py")#$(l7h "$BG/rh-l7h.$$.py" 2>/dev/null)"
rm -f "$BG"/rh-l7h.$$.py

# ---- B65: 前面の pid / 名前は front-guard snap から（`lsappinfo front` は「ASN:0x0-48048:」を返し、`lsappinfo info` はその形に空を返す＝前面が毎回 None・手 6・7・8 が測れなかった）----
b65() {  # b65 <rehearse.py> -> "<front_pid()>|<front_name()>"。lsappinfo と front-guard は 11-01 の実測の答えを返す偽物（前面 = pid 4242 の TextEdit）
  python3 -I - "$1" "$BG" <<'EOFB65'
import importlib.util, json, subprocess, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
def run(a, **k):
    out = ""
    if a[:2] == ["lsappinfo", "front"]: out = "ASN:0x0-48048:\n"
    elif a[:2] == ["lsappinfo", "info"] and a[-1] == "ASN:0x0-0x48048:": out = '"pid"=4242\n' if "pid" in a else '"LSDisplayName"="TextEdit"\n'
    elif a[0].endswith("/front-guard") and a[1] == "snap": json.dump({"front": {"bundle_id": "com.apple.TextEdit", "name": "TextEdit", "pid": 4242}}, open(a[a.index("--out") + 1], "w"))
    return types.SimpleNamespace(returncode=0, stdout=out, stderr="")
m.subprocess = types.SimpleNamespace(run=run, TimeoutExpired=subprocess.TimeoutExpired)
print(f"{m.front_pid()}|{m.front_name()}")
EOFB65
}
python3 -I - "$BG/rehearse.py" "$BG/rh-b65.$$.py" <<'EOFB65M'
import sys
t = open(sys.argv[1], encoding="utf-8").read()
new_pid = '    return (front_app() or {}).get("pid")\n'
old_pid = ('    asn = subprocess.run(["lsappinfo", "front"], capture_output=True, text=True).stdout.strip()\n'
           '    m = re.search(r"(\\d+)\\s*$", subprocess.run(["lsappinfo", "info", "-only", "pid", asn], capture_output=True, text=True).stdout) if asn else None\n'
           '    return int(m.group(1)) if m else None\n')
new_name = '    return (front_app() or {}).get("name") or ""\n'
old_name = ('    asn = subprocess.run(["lsappinfo", "front"], capture_output=True, text=True).stdout.strip()\n'
            '    return subprocess.run(["lsappinfo", "info", "-only", "name", asn], capture_output=True, text=True).stdout if asn else ""\n')
assert t.count(new_pid) == 1 and t.count(new_name) == 1
open(sys.argv[2], "w", encoding="utf-8").write(t.replace(new_pid, old_pid, 1).replace(new_name, old_name, 1))
EOFB65M
check "B65: front_pid / front_name read the front app from front-guard snap (pid 4242 TextEdit); negative control: the pre-B65 lsappinfo front -> info path -> None and empty" \
  "4242|TextEdit#None|" "$(b65 "$BG/rehearse.py")#$(b65 "$BG/rh-b65.$$.py" 2>&1)"
rm -f "$BG"/rh-b65.$$.py

# B65 の 2: run.sh の front_pid（起動の途中で前面を取った自分の shibubu を kill する見張り・start の loop）も同じ根。lsappinfo は 11-01 の実測の答えを返す偽物（前面 = pid 4242）
mkdir -p "$T/b65b"
cat > "$T/b65b/lsappinfo" <<'EOFB65L'
#!/bin/bash
case "$1" in front) echo "ASN:0x0-48048:";; info) [ "${!#}" = "ASN:0x0-0x48048:" ] && echo '"pid"=4242';; esac
EOFB65L
chmod +x "$T/b65b/lsappinfo"
cat > "$T/b65b/old.sh" <<'EOFB65O'
front_pid() { lsappinfo info -only pid "$(lsappinfo front)" | sed -n 's/.*= *\([0-9]*\).*/\1/p'; }
EOFB65O
grep '^front_pid()' "$BG/run.sh" > "$T/b65b/new.sh"
fp65() { PATH="$T/b65b:$PATH" bash -c '. "$1"; front_pid' _ "$1"; }
check "B65-2: run.sh front_pid asks lsappinfo info with the 0x form of the front ASN -> 4242; negative control: the pre-B65 line (short ASN as is) -> empty" \
  "4242#" "$(fp65 "$T/b65b/new.sh")#$(fp65 "$T/b65b/old.sh")"

# B64（demoshort.k7oc の秒の内訳）: 道具の待ちを削った形を、本物の send_field / find / wait_seen / h_goto_tab で（elements・step・time を差し替え・実機を起こさない）。
# 列の字: r = 読み・c = 押す・t = 打つ・k = Enter・! = --expect 付き・zN = N 秒寝る・unm = 未測。tally は --expect の手の直前と直後の読みを前後の木に使う（間に別の手が居ると未測）
b64() {  # b64 <rehearse.py> -> "<send_field expect 付き>|<send_field expect 無し>|<find tries=1 で無い>|<wait_seen 5 秒>|<goto_tab の literal の tab>"
  python3 -I - "$1" "$BG" <<'EOFB64'
import importlib.util, sys, types
sp = importlib.util.spec_from_file_location("rh", sys.argv[1]); m = importlib.util.module_from_spec(sp); sys.path.insert(0, sys.argv[2]); sys.modules["rh"] = m; sp.loader.exec_module(m)
clk, log = [0.0], []
m.time = types.SimpleNamespace(time=lambda: clk[0], sleep=lambda s: (log.append(f"z{s:g}"), clk.__setitem__(0, clk[0] + s)))
E = lambda role, label, tok: {"role": role, "label": label, "element_token": tok, "enabled": True}
def els(C):
    log.append("r")
    return [E("AXTextField", "Change this project", "t-f"), E("AXCheckBox", "Tests", "t-tests")]
def step(C, tool, args, *extra, own=False):
    log.append({"click": "c", "type_text": "t", "press_key": "k"}.get(tool, tool) + ("!" if "--expect" in extra else ""))
    return {}
m.elements, m.step = els, step
C = m.Ctx("/nonexistent-b64", "X.app", "n")
def run(f):
    log.clear()
    try:
        f()
    except m.Unmeasured:
        log.append("unm")
    return ",".join(log)
print("|".join([run(lambda: m.send_field(C, "AXTextField", "change", "hi", enter=True, expect="ax:*")),
                run(lambda: m.send_field(C, "AXTextField", "change", "hi", enter=True)),
                run(lambda: m.find(C, "AXButton", "close", tries=1)),
                run(lambda: m.wait_seen(C, ("never",), 5)),
                run(lambda: m.h_goto_tab(C, {"doing": "『Tests』のタブへ移る"}))]))
EOFB64
}
for x in "a|    press(C, role, label, e=f)|    press(C, role, label)" "b|(find(C, role, label) if expect else f)|f" "c|        if k + 1 < tries:|        if True:" \
         "d|        time.sleep(1)  # B64: 3 → 1（読み 1 回|        time.sleep(3)  # B64: 3 → 1（読み 1 回" "e|        elements(C)  # 効果の後の木（tally の後の木 = 手の直後の読み。|        pass  #"; do
  python3 -I -c 'import sys;t=open(sys.argv[1],encoding="utf-8").read();o=sys.argv[3];assert t.count(o)==1,o;open(sys.argv[2],"w",encoding="utf-8").write(t.replace(o,sys.argv[4],1))' "$BG/rehearse.py" "$BG/rh-b64${x%%|*}.$$.py" "$(cut -d'|' -f2 <<< "$x")" "$(cut -d'|' -f3 <<< "$x")"
done
check "B64: send_field reads once and types with that element (one more read right before an --expect Enter = tally's before tree); find(tries=1) does not sleep after its last try; wait_seen polls every 1 s; goto_tab reads right after the --expect click with no sleep (tally's after tree); negative controls: a = press re-finds (2 reads); b = no read before the --expect Enter (click and type between the tree and the Enter); c = sleeps after the last try; d = 3 s poll (3 reads in 5 s); e = no after read" \
  "r,c,t,r,k!|r,c,t,k|r,unm|r,z1,r,z1,r,z1,r,z1,r,z1,r|r,c!,r#r,r,c,t,r,k!#r,c,t,k!#r,z2,unm#r,z3,r,z3,r#r,c!" \
  "$(l=$(b64 "$BG/rehearse.py"); echo "$l")#$(b64 "$BG/rh-b64a.$$.py" 2>/dev/null | cut -d'|' -f1)#$(b64 "$BG/rh-b64b.$$.py" 2>/dev/null | cut -d'|' -f1)#$(b64 "$BG/rh-b64c.$$.py" 2>/dev/null | cut -d'|' -f3)#$(b64 "$BG/rh-b64d.$$.py" 2>/dev/null | cut -d'|' -f4)#$(b64 "$BG/rh-b64e.$$.py" 2>/dev/null | cut -d'|' -f5)"
rm -f "$BG"/rh-b64?.$$.py

echo "test.sh: pass=$pass fail=$fail"
[ $fail -eq 0 ]
