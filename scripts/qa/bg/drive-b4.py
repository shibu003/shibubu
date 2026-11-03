#!/usr/bin/env python3
"""drive-b4.py — B4（代わりの Web service・GitHub の Issue への comment）の BG の 1 run。外へ効く操作なので既定は dry-run。
使い方: python3 -I scripts/qa/bg/drive-b4.py <run-dir> <Shibubu.app> [--really]
  既定（dry-run）: 宛先の repo / issue と打つ comment を印字するだけ。.app も gh も起こさない・外に出ない・exit 6（未測）。
  --really: operator の合図の時だけ（gfinal-plan は GFINAL_REALLY=1 の時だけ付ける）。Home の一言 → Page の ＋ →「Add browser」→ Issue の URL → comment の欄に nonce → 「Comment」を押す手で、
    oracle/github.sh の before / after（run.sh step --expect oracle:github）が同じ nonce の comment の増えを読む（判定は tally.py の b4_left・WebView とも製品とも別の道）。
  宛先: YC_DEMO_REPO（既定 run.sh と同じ）の SHIBUBU_QA_GH_ISSUE 番号（--really では必須）。owner の GitHub login は WebView の永続 store に在る事（YC_WAVES §3 ⑨'）。
  ponytail: GitHub の頁の URL 欄・comment の欄・「Comment」の AX の label は実機で未測。見つからなければ打たず未測（exit 6）で止める＝偽の緑にしない。
exit: 0 ok（comment が増えた）/ 1 赤 / 2 使い方 / 6 未測（dry-run・owner 使用中・login が無い等で打てない）"""
import json, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tally  # noqa: E402

REPO = os.environ.get("YC_DEMO_REPO", "shibu003/shibubu-yc-demo")


def comment(RH, C, issue, nonce):
    """Page で Add browser → Issue の URL → comment の欄に nonce → Comment を押す（rehearse.py の #11 と同じ手・RH = rehearse の module）。"""
    RH.press(C, "AXButton", "add_menu")  # MIRROR-FRAME-1101: Add browser は Page の右下の ＋（Add to page）の menu の中（labels.py の key で引く）
    RH.press(C, "AXButton", "add_browser")
    url = RH.find(C, "AXTextField", "browser_url", tries=10, contains=True)
    RH.step(C, "type_text", {"text": f"https://github.com/{REPO}/issues/{issue}", "element_token": url["element_token"]})
    RH.step(C, "press_key", {"key": "return", "element_token": url["element_token"]})
    time.sleep(5)
    RH.ax_ids(C)  # ax_tree を R/ax/NN-ax_tree.json に残す（login の読みに使う）
    t = subprocess.run(["python3", "-I", os.path.join(HERE, "tally.py"), "github-login", f"{C.R}/ax/{C.n:02d}-ax_tree.json"], capture_output=True, text=True)
    if t.returncode != 0:  # 5 = login が切れている（owner が FG 枠で login）・6/他 = 木から読めない → comment は打たず未測
        raise RH.Unmeasured("GitHub の login を確認できない（tally.py github-login rc=%d）: %s" % (t.returncode, (t.stderr or "")[-150:]))
    area = RH.find(C, "AXTextArea", "comment", tries=20, contains=True)
    RH.step(C, "type_text", {"text": f"shibubu-qa {nonce}", "element_token": area["element_token"]})
    btn = RH.find(C, "AXButton", "Comment", tries=5, contains=True)
    RH.step(C, "click", {"element_token": btn["element_token"]}, "--expect", "oracle:github", "--needle", nonce)
    time.sleep(3)


def check_oracle(R, retry_s=15):
    """oracle/github.sh の before / after で nonce の comment が増えたか（tally.b4_pairs）。GitHub の反映の遅れに retry_s 秒まで after を取り直し、増えていなければ RuntimeError。"""
    def read():
        return {f: json.load(open(f"{R}/oracle/{f}")) for f in os.listdir(R + "/oracle") if f.endswith(".github.json")} if os.path.isdir(R + "/oracle") else {}
    end = time.time() + retry_s
    while True:
        o = read()
        if any(left is True for left in tally.b4_pairs(o).values()):
            return
        if time.time() > end:
            raise RuntimeError("oracle/github.sh の before / after で nonce の comment が増えていない（after を %d 秒まで取り直した）" % retry_s)
        time.sleep(3)
        for f, d in o.items():
            if f.endswith(".after.github.json"):  # 同じ宛先・同じ nonce で after を書き直す
                r = subprocess.run(["bash", os.path.join(HERE, "oracle/github.sh"), "comments", d["repo"], str(d["issue"]), d["nonce"]], capture_output=True, text=True)
                if r.returncode == 0:
                    open(f"{R}/oracle/{f}", "w").write(r.stdout)


def main(a):
    import rehearse as RH  # 単体で動かす時だけ（rehearse.py が #11 から呼ぶ時は自分の module を渡すので import しない）
    if len(a) < 2:
        print("usage: drive-b4.py <run-dir> <Shibubu.app> [--really]", file=sys.stderr)
        return 2
    R, app = a[0], a[1]
    issue = os.environ.get("SHIBUBU_QA_GH_ISSUE", "")
    nonce = f"b4x{int(time.time())}"
    body = f"shibubu-qa {nonce}"
    os.makedirs(R, exist_ok=True)
    LOG = open(os.path.join(R, "drive.log"), "a")

    def log(*x):
        s = " ".join(str(y) for y in x)
        print(s, flush=True); LOG.write(s + "\n"); LOG.flush()

    if "--really" not in a:
        log(f"B4 dry-run: 宛先 {REPO}#{issue or '（SHIBUBU_QA_GH_ISSUE 未設定）'} に comment「{body}」を打つ予定・外には出していない（--really が無い）")
        return 6
    if not issue.isdigit() or not os.path.isdir(app):
        log("B4: SHIBUBU_QA_GH_ISSUE（整数）と app が要る"); return 2
    rc, err = RH.run_sh("start", R, app)
    log("start rc", rc, err)
    if rc == 6 and json.load(open(R + "/run.json")).get("owner_active"):
        RH.run_sh("stop", R); return 6
    if rc not in (0, 6):
        RH.run_sh("stop", R); return 1
    C = RH.Ctx(R, app, nonce)
    C.depth = 30
    verdict, code = "ok", 0
    try:
        RH.h_generate(C, None)  # 主デモの一言と同じ Home の欄 → 作る → Page（rehearse.py の手）
        comment(RH, C, issue, nonce)
    except RH.Unmeasured as e:
        verdict, code = f"未測: {e}", 6
    except RuntimeError as e:
        verdict, code = f"赤: {e}", 1
    log("stop rc", *RH.run_sh("stop", R))
    t = subprocess.run(["python3", "-I", HERE + "/tally.py", R], capture_output=True, text=True)
    open(R + "/tally.json", "w").write(t.stdout)
    if code == 0:
        try:
            check_oracle(R)
        except RuntimeError as e:
            verdict, code = f"赤: {e}", 1
    log("b4:", verdict, "· tally rc", t.returncode)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
