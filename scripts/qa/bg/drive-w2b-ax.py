#!/usr/bin/env python3
"""W2b-AX の受け入れの通し: 背景起動 → Home の欄に一言 → 作る → 題名に needle → 保存 → ui_state の読み戻し。
run.sh の start / step / stop だけを使う（element_token の手だけ・座標も前面も使わない）。token は手ごとに新しい get_window_state から label で引く。
使い方: SHIBUBU_QA_VARIANT=<build の名前> python3 scripts/qa/bg/drive-w2b-ax.py <run-dir> <Shibubu.app>   → <run-dir>/drive.log
exit: 0 = ui_state に needle が在り ax-judge が ok / 1 = それ以外（D-g の violation は run.sh がその場で止める）"""
import json, os, subprocess, sys, time, random
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import labels as L  # label の 1 か所の表（EN / JP の対）

BG = os.path.dirname(os.path.abspath(__file__))
R, APP = sys.argv[1], sys.argv[2]
os.makedirs(R, exist_ok=True)
LOG = open(os.path.join(R, "drive.log"), "a")
n = 0


def log(*a):
    s = " ".join(str(x) for x in a)
    print(s, flush=True); LOG.write(s + "\n"); LOG.flush()


def run(*a):
    p = subprocess.run(["bash", BG + "/run.sh", *a], capture_output=True, text=True)
    return p.returncode, p.stderr.strip()[-300:]


def rj():
    return json.load(open(R + "/run.json"))


def base():
    j = rj(); return {"pid": j["pid"], "window_id": j["window_id"], "session": j["session"]}


def step(tool, args, *extra):
    global n
    n += 1
    name = f"{n:02d}-{tool}"
    rc, err = run("step", R, name, tool, json.dumps({**args, **base()}, ensure_ascii=False), *extra)
    try:
        out = json.load(open(f"{R}/steps/{name}.cua.json"))
    except Exception:
        out = {"raw": open(f"{R}/steps/{name}.cua.json").read()[:300]}
    log(name, "rc", rc, json.dumps({k: out.get(k) for k in ("effect", "summary", "element_count", "error", "raw") if k in out}, ensure_ascii=False)[:300])
    if rc == 1:
        log("violation → stop"); sys.exit(1)
    return out


def find(role, label, tries=15):
    for _ in range(tries):
        # 木の walk の予算を契約（10 秒）に揃える。cua-driver の既定 1 秒では遅い答えが「窓 0 個・欄が無い」に見える（W2b-AX の原因の半分）
        out = step("get_window_state", {"timeout_ms": 10000, "include_screenshot": False})
        for e in out.get("elements") or []:
            if e.get("role") == role and L.hit(e, label):
                return e["element_token"]
        time.sleep(2)
    log("not found", role, label); return None


def must(x, what):
    if not x:
        log("FAIL", what); run("stop", R); sys.exit(1)
    return x


needle = f"qaneedle{random.randint(1000, 9999)}"
rc, err = run("start", R, APP)
log("start rc", rc, err)
if rc not in (0, 6):
    sys.exit(1)
tok = must(find("AXTextField", "goal"), "goal field")
step("click", {"element_token": tok})
tok = must(find("AXTextField", "goal"), "goal field 2")
o = step("type_text", {"text": "読書記録をつけたい", "element_token": tok})
tok = must(find("AXButton", "create"), "make button")
step("click", {"element_token": tok})
tok = must(find("AXTextField", "title") or find("AXTextArea", "title", 3), "title field")
step("click", {"element_token": tok})
tok = must(find("AXTextField", "title") or find("AXTextArea", "title", 3), "title field 2")
step("type_text", {"text": needle, "element_token": tok}, "--needle", needle)
tok = must(find("AXButton", "save", 5) or find("AXButton", "add", 5), "save button")
step("click", {"element_token": tok}, "--expect", f"ui_state:{needle}")
time.sleep(1)
o = subprocess.run(["python3", "-I", BG + "/oracle/ui-state.py", "read", R + "/data", "--needle", needle], capture_output=True, text=True)
has = json.loads(o.stdout).get("has_needle") if o.returncode == 0 else None
log("ui_state has_needle", has, "needle", needle)
rc, err = run("stop", R)
log("stop rc", rc, err)
v = json.load(open(R + "/ax-windows.verdict.json")) if os.path.exists(R + "/ax-windows.verdict.json") else None
log("ax verdict", json.dumps(v))
sys.exit(0 if has and v and v["ok"] else 1)
