#!/usr/bin/env python3
"""WF2 の短いテイク（試験の側・BG・27 手とは別）: 保存した手順（読む → 下書きを書く → 判断 → 選んだ題を書く）を Page の一言で走らせ、
承認 → 2 手が済んだ後の判断の card → Shibubu を閉じて開く → 同じ card が同じ Project に → 選ぶ → 選んだ題だけを書く（承認）→ 読み戻し。
2 本目: 判断の card で待っている間に元の文を人が変える → 選んでも 1 手も送らず理由の 1 行と card の出し直し。
元の app は偽の bridge（tests/fake-surface-bridge.py・REAPER = pid 222・本文の値は FAKE_BODY_FILE の file）。model は使わない（SHIBUBU_LLM_CLI 空）・外へ送らない。
cua-driver は element_token（type_text・click・press_key）だけ・座標の click・activate・menu は使わない。Quit は NSRunningApplication.terminate（⌘Q と同じ口）。
data dir・HOME・同意の file は run の中（owner の本物の dir に書かない）。Project は主デモの一言（model 無しの名前の表）で作り、Dev の Page から動かない（Diagram の Page の Web には移らない）。log と写真に /Users の path を書かない（run dir は <run>）。
使い方: python3 -I scripts/qa/bg/drive-wf2.py <shibubu の debug binary> <run dir> <evidence dir>   → <evidence dir>/log.jsonl と 01〜07 の png
exit: 0 = 全部の手の oracle が揃った / 1 = 赤（どの手かを log の最後の行）/ 6 = 未測（AX に要素が出ない等で打てない）"""
import hashlib, json, os, sqlite3, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import labels as L  # noqa: E402

BIN, R, EV = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3]
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
CUA = os.path.expanduser("~/.local/bin/cua-driver")
SESSION = "wf2-" + os.path.basename(R)
DATA, HOME, BODY = os.path.join(R, "data"), os.path.join(R, "home"), os.path.join(R, "body.txt")
BRIDGE_LOG = os.path.join(R, "bridge.log")
for d in (DATA, HOME, EV, os.path.join(DATA, "workflows")):
    os.makedirs(d, exist_ok=True)
LOG = open(os.path.join(EV, "log.jsonl"), "a")

# Home の一言は rehearse.h_generate と同じ主デモの文（model 無しの名前の表が Project にする唯一の形・UI-2）
HOME_SAID = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw."
GOAL = "Pick a title for my song notes and set it"
DRAFT, SHORT, LONG = "Song notes (draft)", "Song notes", "Song notes: verse, chorus, bridge"
QUESTION = "Which title for the song notes?"
PICK_SHORT, PICK_LONG = "Use the short title", "Use the long title"
WIN = {"pid": 222, "window_id": 300}
# 書類の窓（bundle_id と document = 窓の題）: 欄に字が在っても置き換えてよい窓（MacGui の G74 ① は書類の窓でない欄の人の字に置かない）。
# 下書きの後に選んだ題を同じ本文へ書くので、書類の窓でないと 2 手目が「人の字が在る」で止まる
DOC = "song-notes.txt"
TEXT = dict(WIN, app_name="REAPER", bundle_id="com.cockos.reaper", document=DOC, label="本文", role="AXTextArea")
APPS = []


class Unmeasured(Exception):
    pass


class Red(Exception):
    pass


def scrub(v):
    s = json.dumps(v, ensure_ascii=False)
    return json.loads(s.replace(R, "<run>").replace(REPO, "<repo>").replace(os.path.expanduser("~"), "~"))


def log(step, **kw):
    kw = scrub(dict(kw, step=step, t=round(time.time(), 2)))
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


def tree(app):
    s = cua("get_window_state", {"pid": app["pid"], "window_id": app["wid"], "include_screenshot": False, "max_elements": 500})
    return s.get("elements") or (s.get("structuredContent") or {}).get("elements") or [], s


def texts(s, *parts):
    """elements は値だけの AXStaticText を落とすので、文は tree_markdown の行から読む。"""
    md = (s or {}).get("tree_markdown") or ""
    return [l.strip() for l in md.splitlines() if "AXStaticText" in l and (not parts or any(x in l for x in parts))]


def find(els, role, key):
    for e in els:
        if role in (e.get("role") or "") and L.hit(e, key):
            return e
    return None


def wait_for(app, role, key, reads=30, every=2):
    """要素が出るまで読み直す（読む回数で数える）。返り値 = (要素, elements, state)。出なければ未測。"""
    for _ in range(reads):
        els, st = tree(app)
        e = find(els, role, key)
        if e:
            return e, els, st
        time.sleep(every)
    raise Unmeasured(f"{role}「{key}」が AX に出ない（{reads} 回読んで）")


def click(app, e):
    r = cua("click", {"pid": app["pid"], "window_id": app["wid"], "element_token": e["element_token"]})
    if isinstance(r, dict) and r.get("effect") == "refused":
        raise Unmeasured(f"「{e.get('label')}」の click が refused")
    return r


def type_into(app, e, text, enter=False):
    r = cua("type_text", {"pid": app["pid"], "window_id": app["wid"], "element_token": e["element_token"], "text": text})
    if enter:
        cua("press_key", {"pid": app["pid"], "window_id": app["wid"], "element_token": e["element_token"], "key": "return"})
    return r


def dark_pixels(png):
    bmp = png + ".bmp"
    subprocess.run(["sips", "-s", "format", "bmp", "-Z", "800", png, "--out", bmp], capture_output=True)
    b = open(bmp, "rb").read(); os.remove(bmp)
    off, bpp = int.from_bytes(b[10:14], "little"), int.from_bytes(b[28:30], "little")
    step, px = bpp // 8, b[off:]
    return sum(1 for i in range(0, len(px) - step + 1, step) if px[i] < 70 and px[i + 1] < 70 and px[i + 2] < 70)


def shot(name, app, tries=8, every=3):
    """背景の窓は描き直しが遅れる（骨格のまま・前の画面のまま残る）ので、黒い文字が写り、続けて 2 回同じ絵になるまで撮り直す
    （1 回目の絵は前の画面のことがある・再起動の後の判断の card が Home の絵で写った）。返り値 = 撮った回数。"""
    path = os.path.join(EV, name + ".png")
    last = None
    for i in range(tries):
        time.sleep(every)
        subprocess.run(["screencapture", "-x", "-o", "-l", str(app["wid"]), path])
        now = open(path, "rb").read() if os.path.exists(path) else None
        if now and now == last and dark_pixels(path) > 100:
            return i + 1
        last = now
    return tries


def body():
    return open(BODY, encoding="utf-8").read() if os.path.exists(BODY) else None


def sets(value=None):
    """偽の bridge が受けた ax_set_value の数（value を渡せばその値の物だけ）= 元の app に送った書き込みの数。"""
    n = 0
    for line in open(BRIDGE_LOG, encoding="utf-8") if os.path.exists(BRIDGE_LOG) else []:
        try:
            q = json.loads(line)
        except ValueError:
            continue
        n += q.get("cmd") == "ax_set_value" and (value is None or q.get("value") == value)
    return n


def runs():
    """runs 表の Engine の行（id → status）を ro で。"""
    try:
        c = sqlite3.connect("file:%s?mode=ro" % os.path.join(DATA, "kangae.sqlite3"), uri=True, timeout=5)
        rows = c.execute("SELECT id, json FROM runs").fetchall(); c.close()
    except sqlite3.Error as e:
        return {"error": str(e)}
    out = {}
    for i, j in rows:
        d = json.loads(j)
        if d.get("workflow") is not None:
            out[i] = (d.get("run") or {}).get("status")
    return out



def seed():
    """保存した手順（Engine の model 無しの一言 = 操作 op1・id = wf-<一言の sha256 の 12 字>-op1）と偽の app の本文。"""
    wid = "wf-%s-op1" % hashlib.sha256(GOAL.strip().encode()).hexdigest()[:12]
    choice = lambda i, label, value, note: {"id": i, "label": label, "value": value, "note": note}
    wf = {
        "schema": "shibubu.workflow/1", "id": wid, "goal": GOAL,
        "steps": [
            {"id": "read", "capability": "desktop.window.state", "input": WIN},
            {"id": "draft", "capability": "desktop.ax.type_text", "input": dict(TEXT, text=DRAFT)},
            {"id": "pick", "kind": {"type": "decide", "question": QUESTION, "choices": [
                choice("short", PICK_SHORT, SHORT, "Fits the track list"), choice("long", PICK_LONG, LONG, "Says what is in the song")]}},
            {"id": "set", "capability": "desktop.ax.type_text", "input": dict(TEXT, text={"$from": "pick.value"})},
        ],
        "checks": [{"action": "desktop.window.state", "input": WIN, "pointer": "/controls/2/value", "expect": {"is": "equals", "value": {"$from": "pick.value"}}}],
    }
    json.dump(wf, open(os.path.join(DATA, "workflows", wid + ".json"), "w"), ensure_ascii=False, indent=2)
    open(BODY, "w", encoding="utf-8").write("Notes")
    bridge = os.path.join(R, "bridge.sh")
    open(bridge, "w").write("#!/bin/sh\nexport FAKE_BODY_FILE='%s' FAKE_LOG='%s' FAKE_TITLES='%s'\nexec python3 '%s/tests/fake-surface-bridge.py'\n" % (BODY, BRIDGE_LOG, DOC, REPO))
    os.chmod(bridge, 0o755)
    return wid, bridge


def launch(bridge, tag):
    env = {k: v for k, v in os.environ.items() if k not in ("SHIBUBU_DATA_DIR", "KANGAE_DATA_DIR", "ANTHROPIC_API_KEY")}
    path = ":".join(d for d in env.get("PATH", "").split(":") if not os.path.exists(os.path.join(d, "cua-driver")))
    trap = os.path.join(R, "trap-cua-driver")
    open(trap, "w").write('#!/bin/bash\necho "$PPID $(date +%%s) $*" >> "%s/trap-marks"\nexit 1\n' % R)
    os.chmod(trap, 0o755)
    mcp = os.path.join(R, "mcp-empty.json"); open(mcp, "w").write('{"mcpServers":{}}\n')
    env.update(SHIBUBU_BACKGROUND="1", SHIBUBU_LLM_CLI="", CUA_DRIVER_BIN=trap, PATH=path, SHIBUBU_MCP_CONFIG=mcp, HOME=HOME,
               SHIBUBU_DATA_DIR=DATA, KANGAE_SURFACE_BRIDGE=bridge, SHIBUBU_GUI_ACT_CONSENTS=os.path.join(DATA, "gui-act-consents.jsonl"))
    before = front()
    out = open(os.path.join(R, f"app-{tag}.log"), "w")
    p = subprocess.Popen([BIN], env=env, cwd=R, stdout=out, stderr=out)
    APPS.append(p)
    wid = window(p.pid)
    time.sleep(3)
    log(f"{tag}-launch", window=wid is not None, front_before=before, front_after=front())
    if wid is None:
        raise Unmeasured("Shibubu の窓が出ない")
    return {"p": p, "pid": p.pid, "wid": wid}


def wait_exit(app, secs=20):
    try:
        return app["p"].wait(timeout=secs)
    except subprocess.TimeoutExpired:
        return None


def ask(app, reads=45):
    """Page の欄に一言 → Enter（Project の編集として読めない文は Engine へ）→ 承認の card（返り値の 1 つ目）か、承認が要らずに判断の card（None）。"""
    f, _, _ = wait_for(app, "AXTextField", "change", reads=40)
    type_into(app, f, GOAL, enter=True)
    for _ in range(reads):
        els, st = tree(app)
        a, d = find(els, "AXButton", "approve"), find(els, "AXButton", PICK_LONG)
        if a or d:
            return a, st
        time.sleep(2)
    raise Unmeasured(f"一言の後に承認の card も判断の card も出ない（{reads} 回読んで）")


def decide_card(app, reads=45):
    b, els, st = wait_for(app, "AXButton", PICK_LONG, reads=reads)
    return {"short": find(els, "AXButton", PICK_SHORT), "long": b}, st


def main():
    wid, bridge = seed()
    log("0-seed", workflow=wid, body=body())
    # ---- 1. Home の一言で Project を作る（model 無し = 名前の表の主デモの一言・面の app は偽の bridge に聞く）→ Page の欄に一言 → 承認の card（下書きを書く手）
    app = launch(bridge, "a")
    g, _, _ = wait_for(app, "AXTextField", "goal")
    type_into(app, g, HOME_SAID)
    c, _, _ = wait_for(app, "AXButton", "create")
    click(app, c)
    approve, st = ask(app)
    if approve is None:
        raise Red("承認の前に判断の card（下書きを書く手を承認なしで送った）")
    log("1-approve-card", line=texts(st, "Approv", "once"), sets=sets(), body=body(), runs=runs(), front=front())
    log("1-shot", tries=shot("01-approve-draft", app))
    if sets() != 0:
        raise Red("承認の前に送った")
    # ---- 2. 承認 → 下書きを書く → 2 手が済んだ後の判断の card（後ろの手は送らない）
    click(app, approve)
    pick, st = decide_card(app)
    log("2-decide-card", question=texts(st, QUESTION), evidence=texts(st, SHORT), sets=sets(), body=body(), runs=runs(), front=front())
    log("2-shot", tries=shot("02-decide", app))
    if (sets(DRAFT), sets(), body()) != (1, 1, DRAFT) or "waiting_human" not in runs().values():
        raise Red("判断の前の 2 手・待ちの行が揃わない")
    # ---- 3. Shibubu を閉じて開く → 同じ card が同じ Project に
    q = quit_ae(app["pid"])
    code = wait_exit(app)
    log("3-quit", invoke=q[:120], exited=code is not None, runs=runs())
    app = launch(bridge, "b")
    try:
        pick, st = decide_card(app, reads=8)
    except Unmeasured:
        e, _, _ = wait_for(app, "AXCheckBox", "dev_env", reads=20)  # sidebar の Project の入口（labels.py #25）
        click(app, e)
        pick, st = decide_card(app)
    log("3-decide-after-restart", question=texts(st, QUESTION), evidence=texts(st, SHORT), sets=sets(), body=body(), runs=runs(), front=front())
    log("3-shot", tries=shot("03-decide-after-restart", app))
    # ---- 4. 長い題を選ぶ → 選んだ題を書く手の承認（下書きは走り直さない）→ 承認 → 読み戻し
    click(app, pick["long"])
    approve, _, st = wait_for(app, "AXButton", "approve", reads=45)
    log("4-approve-set", line=texts(st, "Approv", LONG), sets=sets(), body=body(), runs=runs())
    log("4-shot", tries=shot("04-approve-set", app))
    if sets() != 1 or sets(SHORT) != 0:
        raise Red("選んだ後・承認の前に送った")
    click(app, approve)
    for _ in range(30):
        if sets(LONG) and "verified" in runs().values():
            break
        time.sleep(2)
    _, st = tree(app)
    log("5-done", lines=texts(st), sets_draft=sets(DRAFT), sets_long=sets(LONG), sets_short=sets(SHORT), body=body(), runs=runs())
    log("5-shot", tries=shot("05-done", app))
    if (sets(DRAFT), sets(LONG), sets(SHORT), body()) != (1, 1, 0, LONG) or "verified" not in runs().values():
        raise Red("選んだ道だけを 1 回・下書きは 1 回・読み戻しが揃わない")
    # ---- 6. 2 本目: 判断の card で待っている間に人が本文を変える → 選んでも 1 手も送らず理由の 1 行と card の出し直し
    close = find(tree(app)[0], "AXButton", "Close")
    if close:
        click(app, close)
    approve, _ = ask(app)
    if approve:
        click(app, approve)
    pick, _ = decide_card(app)
    n = sets()
    open(BODY, "w", encoding="utf-8").write("Song notes, edited by hand")
    click(app, pick["short"])
    time.sleep(4)
    _, st = decide_card(app, reads=20)
    log("6-changed", line=texts(st, "changed", "nothing was sent"), sets_before=n, sets_after=sets(), body=body(), runs=runs())
    log("6-shot", tries=shot("06-changed", app))
    if sets() != n or body() != "Song notes, edited by hand" or not texts(st, "nothing was sent"):
        raise Red("元が変わったのに送った・理由の 1 行が無い")
    cancel = find(tree(app)[0], "AXButton", "cancel")
    if cancel:
        click(app, cancel)
        time.sleep(2)
        log("7-cancel", runs=runs(), lines=texts(tree(app)[1], "Your choice"))
        log("7-shot", tries=shot("07-cancelled", app))
    q = quit_ae(app["pid"])
    log("8-quit", invoke=q[:120], exited=wait_exit(app) is not None)


if __name__ == "__main__":
    rc = 0
    try:
        main()
    except Unmeasured as e:
        log("z-unmeasured", why=str(e)); rc = 6
    except Red as e:
        log("z-red", why=str(e)); rc = 1
    # 赤・未測で途中に止まっても Shibubu を残さない（自分で起こした process だけ・⌘Q と同じ口 → 応えなければ terminate）
    for p in APPS:
        if p.poll() is None:
            quit_ae(p.pid)
            try:
                p.wait(timeout=20)
            except subprocess.TimeoutExpired:
                p.terminate()
    marks = os.path.join(R, "trap-marks")
    log("z-trap", marks=open(marks).read() if os.path.exists(marks) else "")
    sys.exit(rc)
