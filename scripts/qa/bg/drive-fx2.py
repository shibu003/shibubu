#!/usr/bin/env python3
"""FX2 の実機の通し（試験の側・BG）: Home で一言を送る → 欄の下に `Reading your request`（経過）と `Stop` → Stop → `Stopping the AI…` →
偽の CLI の子が消えてから `Stopped. Nothing was added to Projects.`・欄の文は残る → 同じ文をもう 1 回（今度の偽の CLI は plan の上限の答え）→
`AI wasn't available · …` と次の一手 `Use simple rules` → 押す → 名前の表で Project が 1 つ・`Built with simple rules because …`。
cua-driver は element_token（type_text・press_key・click）だけ・座標の click・activate・invoke_menu は使わない。各手の前後に前面の app を記録する。
LLM は偽の CLI（run dir の fake/claude・owner の quota を使わない・model を呼ばない）: `--version` と `auth status` に plan の login で答え、
`-p` は fake/mode が sleep なら pid を書いて 120 秒寝る（exec = 子の pid のまま）・limit なら上限の答えを返す。
OS の Reduce Motion（env `KANGAE_REDUCE_MOTION=1`・利用者の設定と同じ段）で起こす: debug binary は骨格の shimmer（毎 frame の repaint）の間 AX の木が
欠ける（87 → 2〜5 要素・実測）ので、1 秒に 1 frame（経過の m:ss）にして木を読む。App Nap は `-NSAppSleepDisabled YES`（その process の引数だけ）。
最後に生成中の Quit で子が残るか（FX2 の外・FX1 の終了の道）を記録して、残った子は自分で消す。
使い方: python3 -I scripts/qa/bg/drive-fx2.py <shibubu の binary> <run dir> <evidence dir>   → <evidence dir>/log.jsonl と 01〜04 の png
（.app でなく cargo の debug binary を直に起こす）"""
import json, os, sqlite3, subprocess, sys, time

BIN, R, EV = sys.argv[1], sys.argv[2], sys.argv[3]
CUA = os.path.expanduser("~/.local/bin/cua-driver")
SESSION = "fx2-" + os.path.basename(R)
DATA, FAKE = os.path.join(R, "data"), os.path.join(R, "fake")
for d in (DATA, FAKE, EV):
    os.makedirs(d, exist_ok=True)
LOG = open(os.path.join(EV, "log.jsonl"), "a")
SAID = "Keep research notes about bees"

def log(step, **kw):
    kw.update(step=step, t=round(time.time(), 2))
    LOG.write(json.dumps(kw, ensure_ascii=False) + "\n"); LOG.flush()
    print(json.dumps(kw, ensure_ascii=False)[:600])

def front():
    p = subprocess.run(["osascript", "-l", "JavaScript", "-e",
        "ObjC.import('AppKit'); var f=$.NSWorkspace.sharedWorkspace.frontmostApplication; f.isNil()?'nil':f.localizedName.js+' '+f.processIdentifier"],
        capture_output=True, text=True, timeout=30)
    return (p.stdout + p.stderr).strip()

def quit_ae(pid):
    p = subprocess.run(["osascript", "-l", "JavaScript", "-e",
        "ObjC.import('AppKit'); var a=$.NSRunningApplication.runningApplicationWithProcessIdentifier(%d); a.isNil()?'nil':String(a.terminate)" % pid],
        capture_output=True, text=True, timeout=30)
    return (p.stdout + p.stderr).strip()

def cua(tool, args):
    args = dict(args, session=SESSION) if tool != "list_windows" else args
    p = subprocess.run([CUA, "call", tool, json.dumps(args)], capture_output=True, text=True, timeout=120)
    try:
        return json.loads(p.stdout)
    except Exception:
        return {"_raw": p.stdout[-800:], "_err": p.stderr[-800:], "_rc": p.returncode}

def window(pid):
    for _ in range(120):
        w = cua("list_windows", {})
        ws = [x for x in (w.get("windows") or []) if x.get("pid", x.get("owner_pid")) == pid] if isinstance(w, dict) else []
        if ws:
            return ws[0].get("window_id")
        time.sleep(0.5)
    return None

def tree(pid, wid):
    s = cua("get_window_state", {"pid": pid, "window_id": wid, "include_screenshot": False, "max_elements": 500})
    return s.get("elements") or (s.get("structuredContent") or {}).get("elements") or [], s

def texts(s, *parts):
    """elements は値だけの AXStaticText を落とすので、文は tree_markdown の行から読む（1 行の段は AXGroup の label）。"""
    md = (s or {}).get("tree_markdown") or ""
    return [l.strip() for l in md.splitlines() if any(x in l for x in parts)]

def find(els, role_part, label):
    for e in els:
        if label == (e.get("label") or "") and role_part in (e.get("role") or ""):
            return e
    return None

def until(pid, wid, ok, secs, every=0.3):
    """ok(els, st) が真になるまで木を読み直す（返り値 = (els, st, 秒) か (els, st, None)）。"""
    t = time.time()
    while True:
        els, st = tree(pid, wid)
        if ok(els, st):
            return els, st, round(time.time() - t, 2)
        if time.time() - t > secs:
            return els, st, None
        time.sleep(every)

def shot(name, wid):
    path = os.path.join(EV, name + ".png")
    subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), path])
    return path

def alive(pid):
    return subprocess.run(["/bin/kill", "-0", str(pid)], stderr=subprocess.DEVNULL).returncode == 0

def projects():
    """作った物の数（Project と 1 Page の app の行）。名前の表は一言によってどちらかを作る。"""
    try:
        c = sqlite3.connect("file:%s?mode=ro" % os.path.join(DATA, "kangae.sqlite3"), uri=True)
        n = c.execute("SELECT (SELECT COUNT(*) FROM projects) + (SELECT COUNT(*) FROM apps)").fetchone()[0]; c.close(); return n
    except Exception as e:
        return "error: %s" % e

def field_value(els):
    return (find(els, "Text", "What to create") or {}).get("value")

# ---- 偽の CLI（plan の login・-p は mode で寝るか上限を返す・呼ばれた順に calls.log）
fake = os.path.join(FAKE, "claude")
with open(fake, "w") as f:
    f.write("""#!/bin/bash
here=$(cd "$(dirname "$0")" && pwd)
case "$1" in
  --version) echo '9.9.1 (Claude Code)'; exit 0 ;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}'; exit 0 ;;
  -p) ;;
  *) exit 2 ;;
esac
n=$(( $(cat "$here/calls.log" 2>/dev/null | wc -l) + 1 ))
mode=$(cat "$here/mode")
echo "$(date +%s) $$ $mode" >> "$here/calls.log"
cat > "$here/calls.$n.stdin"
if [ "$mode" = sleep ]; then echo $$ > "$here/pid.$n"; exec /bin/sleep 120; fi
printf '%s\\n' '{"type":"result","subtype":"success","is_error":true,"result":"You have hit your limit · resets 3pm"}'
""")
os.chmod(fake, 0o755)
open(os.path.join(FAKE, "mode"), "w").write("sleep\n")

env = {k: v for k, v in os.environ.items() if k not in ("SHIBUBU_DATA_DIR", "KANGAE_DATA_DIR")}
env["PATH"] = ":".join(d for d in env.get("PATH", "").split(":") if not os.path.exists(os.path.join(d, "cua-driver")))
trap = os.path.join(R, "trap-cua-driver")
open(trap, "w").write('#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap-marks"\nexit 1\n' % R); os.chmod(trap, 0o755)
mcp = os.path.join(R, "mcp-empty.json"); open(mcp, "w").write('{"mcpServers":{}}\n')
env.update(SHIBUBU_BACKGROUND="1", SHIBUBU_LLM_CLI=fake, CUA_DRIVER_BIN=trap, SHIBUBU_MCP_CONFIG=mcp, SHIBUBU_DATA_DIR=DATA, KANGAE_REDUCE_MOTION="1")

# ---- 1. 起こして Home の欄に打ち、Return で送る
before = front()
p = subprocess.Popen([BIN, "-NSAppSleepDisabled", "YES"], env=env, cwd="/", stdout=open(os.path.join(R, "app.log"), "w"), stderr=subprocess.STDOUT)
wid = window(p.pid)
els, st, _ = until(p.pid, wid, lambda e, s: find(e, "Text", "What to create"), 30)
log("a-launch", pid=p.pid, window_id=wid, front_before=before, front_after=front(), projects=projects())
field = find(els, "Text", "What to create")
cua("type_text", {"pid": p.pid, "window_id": wid, "element_token": field["element_token"], "text": SAID})
time.sleep(1)
els, _ = tree(p.pid, wid)
cua("press_key", {"pid": p.pid, "window_id": wid, "key": "return", "element_token": find(els, "Text", "What to create")["element_token"]})
t_sent = time.time()
# ---- 2. 欄の下の段: Reading your request と Stop（CLI を確かめる間は Checking for AI on this Mac）
els, st, secs = until(p.pid, wid, lambda e, s: find(e, "Button", "Stop") and os.path.exists(os.path.join(FAKE, "pid.1")), 30)
pid1 = open(os.path.join(FAKE, "pid.1")).read().strip() if os.path.exists(os.path.join(FAKE, "pid.1")) else None
time.sleep(1.5)
els, st = tree(p.pid, wid)
log("b-reading", stop=bool(find(els, "Button", "Stop")), after_s=secs, line=texts(st, "Reading your request", "Checking for AI"),
    child_pid=pid1, child_alive=bool(pid1) and alive(pid1), field=field_value(els), front=front())
shot("01-reading", wid)
# ---- 3. Stop → Stopping the AI… → 子が消えてから Stopped. Nothing was added to Projects.
stop = find(els, "Button", "Stop")
if stop:
    cua("click", {"pid": p.pid, "window_id": wid, "element_token": stop["element_token"]})
    t_stop = time.time()
    els, st = tree(p.pid, wid)
    log("c-stopping", line=texts(st, "Stopping the AI"), stop_left=bool(find(els, "Button", "Stop")), front=front())
    els, st, secs = until(p.pid, wid, lambda e, s: texts(s, "Stopped. Nothing was added to Projects."), 20)
    log("d-stopped", after_s=secs, line=texts(st, "Stopped. Nothing was added"), child_alive=bool(pid1) and alive(pid1),
        field=field_value(els), projects=projects(), stop_left=bool(find(els, "Button", "Stop")), front=front())
    shot("02-stopped", wid)
# ---- 4. 同じ文をもう 1 回（偽の CLI は plan の上限）→ AI wasn't available · … と次の一手 1 つ
open(os.path.join(FAKE, "mode"), "w").write("limit\n")
els, _ = tree(p.pid, wid)
f = find(els, "Text", "What to create")
if f:
    cua("press_key", {"pid": p.pid, "window_id": wid, "key": "return", "element_token": f["element_token"]})
els, st, secs = until(p.pid, wid, lambda e, s: texts(s, "AI wasn't available"), 30)
actions = [e.get("label") for e in els if "Button" in (e.get("role") or "") and e.get("label") in ("Try again", "Say it another way", "Use simple rules", "Stop")]
log("e-failed", after_s=secs, line=texts(st, "AI wasn't available"), actions=actions, field=field_value(els), projects=projects(), front=front())
shot("03-ai-failed", wid)
# ---- 5. Use simple rules → 名前の表で Project が 1 つ・AI を使わずに作った事を 1 行
simple = find(els, "Button", "Use simple rules")
if simple:
    cua("click", {"pid": p.pid, "window_id": wid, "element_token": simple["element_token"]})
    els, st, secs = until(p.pid, wid, lambda e, s: texts(s, "Built with simple rules"), 30)
    # 名前の表は一言によって Project か 1 Page の app を作る（どちらも Home から移る）
    els, st, secs = until(p.pid, wid, lambda e, s: not find(e, "Text", "What to create"), 30)
    log("f-simple", after_s=secs, projects=projects(), left_home=not find(els, "Text", "What to create"),
        labels=[e.get("label") for e in els if e.get("label")][:12], calls_after=len(open(os.path.join(FAKE, "calls.log")).read().splitlines()), front=front())
    shot("04-simple-rules-project", wid)
    # 作った後の 1 行（llm_line）は Home の欄の下 → sidebar の Home へ
    home = find(els, "", "Home")  # Home の外では sidebar の Home だけ（header の題は作った物の名前）
    if home:
        cua("click", {"pid": p.pid, "window_id": wid, "element_token": home["element_token"]})
    els, st, secs = until(p.pid, wid, lambda e, s: texts(s, "Built with simple rules"), 20)
    log("f-simple-line", home_button=bool(home), after_s=secs, line=texts(st, "Built with simple rules"), front=front())
    shot("05-simple-rules-line", wid)
calls = open(os.path.join(FAKE, "calls.log")).read().splitlines() if os.path.exists(os.path.join(FAKE, "calls.log")) else []
log("g-calls", calls=[" ".join(c.split()[2:]) for c in calls])
# ---- 6. 生成中に Quit（FX2 の外 = 記録だけ）: 子が残るか
open(os.path.join(FAKE, "mode"), "w").write("sleep\n")
els, _ = tree(p.pid, wid)
f = find(els, "Text", "What to create")
if f:
    cua("type_text", {"pid": p.pid, "window_id": wid, "element_token": f["element_token"], "text": "Plan a garden"}); time.sleep(1)
    els, _ = tree(p.pid, wid); f = find(els, "Text", "What to create")
    cua("press_key", {"pid": p.pid, "window_id": wid, "key": "return", "element_token": f["element_token"]})
n = len(calls)
t = time.time()
while not os.path.exists(os.path.join(FAKE, "pid.%d" % (n + 1))) and time.time() - t < 30:
    time.sleep(0.3)
last = open(os.path.join(FAKE, "pid.%d" % (n + 1))).read().strip() if os.path.exists(os.path.join(FAKE, "pid.%d" % (n + 1))) else None
q = quit_ae(p.pid)
try:
    code = p.wait(timeout=20)
except subprocess.TimeoutExpired:
    code = None
marks = os.path.join(R, "trap-marks")
time.sleep(1)
left = bool(last) and alive(last)
log("z-quit", invoke=q[:200], exit_code=code, child_during_quit=last, child_left_after_quit=left, front=front(), trap=open(marks).read() if os.path.exists(marks) else "")
if left:
    subprocess.run(["/bin/kill", last])  # 試験の偽の CLI（自分が起こした子）だけ
