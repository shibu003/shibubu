#!/usr/bin/env python3
"""FX1 の実機の通し（試験の側・BG）: 書けない data dir で Quit（terminate: の口）→ 黙って終わらず 1 行 → Keep open → 文が残る →
Quit anyway で終わる → 壊れた sessions.json で開き直す → Damaged layout · Reveal in Finder → 書ける dir の Quit はすぐ終わる。
cua-driver は element_token（type_text・click）だけ・座標の click・activate・invoke_menu（owner の画面に menu を出す）は使わない。
Quit は NSRunningApplication.terminate（quit の Apple Event → NSApp terminate: → applicationShouldTerminate: = ⌘Q・Dock の Quit と同じ口）。各手の前後に前面の app（NSWorkspace）を記録する。
env 無しの起動（手 g・h）は HOME を run の中の temp にする（owner の本物の data dir に書かない）。手 h は cwd に FX1 の前の既定 `.shibubu-data`（手 a〜f の data の写し）を置いて起こし、新しい dir へ写されて旧 dir が残る事を見る。
使い方: python3 -I scripts/qa/bg/drive-fx1.py <shibubu の binary> <run dir> <evidence dir>   → <evidence dir>/log.jsonl と 01〜05 の png
（.app でなく cargo の debug binary を直に起こす。run.sh の start は .app を open -g するので使わない）"""
import json, os, subprocess, sys, time

BIN, R, EV = sys.argv[1], sys.argv[2], sys.argv[3]
CUA = os.path.expanduser("~/.local/bin/cua-driver")
SESSION = "fx1-" + os.path.basename(R)
DATA = os.path.join(R, "data")
os.makedirs(DATA, exist_ok=True); os.makedirs(EV, exist_ok=True)
LOG = open(os.path.join(EV, "log.jsonl"), "a")

def front():
    """前面の app（NSWorkspace.frontmostApplication・lsappinfo info は前面の ASN に空を返した）。"""
    p = subprocess.run(["osascript", "-l", "JavaScript", "-e",
        "ObjC.import('AppKit'); var f=$.NSWorkspace.sharedWorkspace.frontmostApplication; f.isNil()?'nil':f.localizedName.js+' '+f.processIdentifier"],
        capture_output=True, text=True, timeout=30)
    return (p.stdout + p.stderr).strip()

def quit_ae(pid):
    """⌘Q と同じ terminate: の口（menu を開かない）。返り値は terminate の bool の文字列か error。"""
    p = subprocess.run(["osascript", "-l", "JavaScript", "-e",
        "ObjC.import('AppKit'); var a=$.NSRunningApplication.runningApplicationWithProcessIdentifier(%d); a.isNil()?'nil':String(a.terminate)" % pid],
        capture_output=True, text=True, timeout=30)
    return (p.stdout + p.stderr).strip()

def log(step, **kw):
    kw.update(step=step, t=round(time.time(), 2))
    LOG.write(json.dumps(kw, ensure_ascii=False) + "\n"); LOG.flush()
    print(json.dumps(kw, ensure_ascii=False)[:600])

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
    """elements は値だけの AXStaticText を落とすので、文は tree_markdown の行から読む。"""
    md = (s or {}).get("tree_markdown") or ""
    return [l.strip() for l in md.splitlines() if "AXStaticText" in l and any(x in l for x in parts)]

def find(els, role_part, label):
    for e in els:
        if label == (e.get("label") or "") and role_part in (e.get("role") or ""):
            return e
    return None

def dark_pixels(png):
    """骨格（薄い灰色だけ）か本物の frame（黒い文字がある）かを見る: 縮めた BMP の暗い画素の数。"""
    bmp = png + ".bmp"
    subprocess.run(["sips", "-s", "format", "bmp", "-Z", "800", png, "--out", bmp], capture_output=True)
    b = open(bmp, "rb").read(); os.remove(bmp)
    off = int.from_bytes(b[10:14], "little"); w = int.from_bytes(b[18:22], "little", signed=True); bpp = int.from_bytes(b[28:30], "little")
    step = bpp // 8
    px = b[off:]
    return sum(1 for i in range(0, len(px) - step + 1, step) if px[i] < 70 and px[i + 1] < 70 and px[i + 2] < 70)

def shot_rendered(name, wid, tries=10, every=4):
    """背景の窓は描き直しが遅れる（骨格のまま残る）ので、黒い文字が写るまで撮り直す。返り値 = (path, 撮った回数, 暗い画素)。"""
    for i in range(tries):
        path = shot(name, wid); d = dark_pixels(path)
        if d > 100:
            return path, i + 1, d
        time.sleep(every)
    return path, tries, d

def shot(name, wid):
    path = os.path.join(EV, name + ".png")
    subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), path])
    return path

def launch(env_extra, tag, cwd="/"):
    env = {k: v for k, v in os.environ.items() if k not in ("SHIBUBU_DATA_DIR", "KANGAE_DATA_DIR")}
    path = ":".join(d for d in env.get("PATH", "").split(":") if not os.path.exists(os.path.join(d, "cua-driver")))
    trap = os.path.join(R, "trap-cua-driver")
    with open(trap, "w") as f:
        f.write('#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap-marks"\nexit 1\n' % R)
    os.chmod(trap, 0o755)
    mcp = os.path.join(R, "mcp-empty.json"); open(mcp, "w").write('{"mcpServers":{}}\n')
    env.update(SHIBUBU_BACKGROUND="1", SHIBUBU_LLM_CLI="", CUA_DRIVER_BIN=trap, PATH=path, SHIBUBU_MCP_CONFIG=mcp)
    env.update(env_extra)
    before = front()
    out = open(os.path.join(R, f"app-{tag}.log"), "w")
    p = subprocess.Popen([BIN], env=env, cwd=cwd, stdout=out, stderr=out)
    wid = window(p.pid)
    time.sleep(3)
    log(f"{tag}-launch", pid=p.pid, window_id=wid, front_before=before, front_after=front())
    return p, wid

def wait_exit(p, secs):
    try:
        return p.wait(timeout=secs)
    except subprocess.TimeoutExpired:
        return None

# ---- 1. 書ける dir で起こし、Home の欄に打つ
p, wid = launch({"SHIBUBU_DATA_DIR": DATA}, "a")
els, _ = tree(p.pid, wid)
field = find(els, "Text", "What to create")
log("a-field", found=bool(field), token=(field or {}).get("element_token"))
r = cua("type_text", {"pid": p.pid, "window_id": wid, "element_token": field["element_token"], "text": "keep this text"})
time.sleep(1.5)
els, _ = tree(p.pid, wid)
log("a-typed", value=(find(els, "Text", "What to create") or {}).get("value"), type_text=(r.get("effect") if isinstance(r, dict) else str(r)[:200]), front=front())
shot("01-typed", wid)
# ---- 2. data dir を書けなくして Quit（menu の AX = NSApp terminate: = ⌘Q と同じ道）
os.chmod(DATA, 0o500)
f0 = front()
q = quit_ae(p.pid)
time.sleep(2.5)
alive = p.poll() is None
els, st = tree(p.pid, wid) if alive else ([], {})
row = texts(st, "Couldn't save") + [e.get("label") for e in els if e.get("label") in ("Quit anyway", "Keep open")]
log("b-quit-unwritable", invoke=str(q)[:300], alive=alive, row=row, front_before=f0, front_after=front())
if alive: shot("02-quit-refused", wid)
# ---- 3. Keep open → 1 行が消え、欄の文は残る
keep = find(els, "Button", "Keep open")
if keep:
    cua("click", {"pid": p.pid, "window_id": wid, "element_token": keep["element_token"]})
    time.sleep(1.5)
    els, st = tree(p.pid, wid)
    log("c-keep-open", row_left=texts(st, "Couldn't save") + [e.get("label") for e in els if e.get("label") in ("Quit anyway", "Keep open")],
        field=(find(els, "Text", "What to create") or {}).get("value"), alive=p.poll() is None, front=front())
    shot("03-keep-open", wid)
# ---- 4. もう 1 回 Quit → Quit anyway で終わる
quit_ae(p.pid)
time.sleep(2.5)
els, _ = tree(p.pid, wid)
anyway = find(els, "Button", "Quit anyway")
if anyway:
    shot("04-quit-again", wid)
    cua("click", {"pid": p.pid, "window_id": wid, "element_token": anyway["element_token"]})
code = wait_exit(p, 20)
log("d-quit-anyway", row_again=bool(anyway), exited=code is not None, exit_code=code, front=front())
os.chmod(DATA, 0o700)
# ---- 5. 壊れた sessions.json で開き直す → Damaged layout · Reveal in Finder（押さない = Finder が前に出る）
open(os.path.join(DATA, "sessions.json"), "w").write("{not json")
p, wid = launch({"SHIBUBU_DATA_DIR": DATA}, "e")
els, st = tree(p.pid, wid)
# 背景の窓は全 app の後ろ（覆われている）ので、起動の骨格の fade が描き直されないまま残る。欄に 1 文字打って本物の frame を描かせてから撮る
f = find(els, "Text", "What to create")
if f: cua("type_text", {"pid": p.pid, "window_id": wid, "element_token": f["element_token"], "text": "x"}); time.sleep(2)
els, st = tree(p.pid, wid)
aside = [n for n in os.listdir(DATA) if n.startswith("sessions.json.corrupt-")]
log("e-reopen-damaged", reveal=bool(find(els, "Button", "Reveal in Finder")),
    line=texts(st, "set aside"), aside_files=aside, front=front())
_, n, d = shot_rendered("05-damaged-reveal", wid)
log("e-shot", tries=n, dark_pixels=d)
# ---- 6. 書ける dir の Quit はすぐ終わり、sessions.json が書かれる
q = quit_ae(p.pid)
code = wait_exit(p, 20)
log("f-quit-writable", invoke=str(q)[:200], exited=code is not None, exit_code=code,
    sessions_json=os.path.exists(os.path.join(DATA, "sessions.json")), front=front())
# ---- 7. env 無し（HOME は run の中の temp・owner の本物の dir に書かない）→ HOME/Library/Application Support/Shibubu
home = os.path.join(R, "home"); os.makedirs(home, exist_ok=True)
p, wid = launch({"HOME": home}, "g")
q = quit_ae(p.pid)
code = wait_exit(p, 20)
d = os.path.join(home, "Library/Application Support/Shibubu")
log("g-default-dir", exited=code is not None, dir_exists=os.path.isdir(d), files=sorted(os.listdir(d)) if os.path.isdir(d) else [],
    cwd_dir=os.path.exists("/.shibubu-data"), front=front())
# ---- 8. FX1 の前の既定（cwd の .shibubu-data）に保存が在る・env 無し → 新しい dir へ写してから読む（旧 dir は残す）。HOME は run の中の別の temp
import shutil, sqlite3
cwd = os.path.join(R, "cwd"); old = os.path.join(cwd, ".shibubu-data")
shutil.rmtree(cwd, ignore_errors=True); shutil.copytree(DATA, old)
def projects(db):
    try:
        c = sqlite3.connect("file:%s?mode=ro" % db, uri=True); n = c.execute("SELECT COUNT(*) FROM projects").fetchone()[0]; c.close(); return n
    except Exception as e:
        return "error: %s" % e
home2 = os.path.join(R, "home2"); os.makedirs(home2, exist_ok=True)
p, wid = launch({"HOME": home2}, "h", cwd=cwd)
q = quit_ae(p.pid)
code = wait_exit(p, 20)
d = os.path.join(home2, "Library/Application Support/Shibubu")
log("h-old-dir-moved", exited=code is not None, exit_code=code, new_files=sorted(os.listdir(d)) if os.path.isdir(d) else [],
    old_files=sorted(os.listdir(old)), projects_old=projects(os.path.join(old, "kangae.sqlite3")),
    projects_new=projects(os.path.join(d, "kangae.sqlite3")), front=front())
marks = os.path.join(R, "trap-marks")
log("z-trap", marks=open(marks).read() if os.path.exists(marks) else "")
