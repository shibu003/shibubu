#!/usr/bin/env python3
"""rehearse.py — G-final の録画 n の台本（docs/demo/full.md）の BG の手を、背景で機械が通す（試験の側の道具・製品の依存ではない）。

使い方（rehearse.sh が呼ぶ）:
  python3 -I rehearse.py <n 1..5|scene2|short> --dry-run              台本を読んで打つ手を印字するだけ（.app を起こさない）。手の数 = 台本の行数・文言が今の src に無ければ exit 1
  python3 -I rehearse.py --fg <fg-checklist.md>          owner の FG の表の「見える筈の物」の文言が strings.csv に在るか（無ければ exit 1）
  python3 -I rehearse.py <n> <Shibubu.app> [--out DIR]   run.sh の start / step / stop で BG の手を通す。FG の手は通さず「FG」と数える

出す物（run の dir・w15 と同じ形）: steps.jsonl・tally（tally.py の出力）・rehearse.json（手ごとの状態・秒）・標準出力の表（手 | 誰 | 状態 | 秒 | 目安）。
保証する: 前面・cursor・座標を使う手は無い（run.sh が拒否する）・FG の手は通さない・driver の無い手は「未測」と書いて緑にしない・起動の前に .app の kangae-mirror-helper を見る（無ければ exit 3）・owner_idle.py で idle を 1 回見る（自分の打鍵は除く・120 秒未満は走らせず exit 5）。
保証しない: 録画の中の FG の手の合否（owner の前面）・driver の無い手の合否（下の NO_DRIVER に理由）・外へ効く手（b4 = GitHub への comment）は GFINAL_REALLY=1 が無い run では打たず「未測」（宛先は YC_DEMO_REPO の Issue SHIBUBU_QA_GH_ISSUE・既定 1）・人の lease を要る ask_ai の F2（#18）は FG の #17 を通さない run では「未測」（録画 mode の owner の手が要る）。
録画 mode（SHIBUBU_QA_RECORD=1・record.sh が立てる）: owner が机に居る前提。idle の門を通らず（run.sh も同じ）・FG の手は「[owner] …」を出す。fg_textedit / fg_mirror_type は stdin の Enter を待ち（待つ間も録画は続く・前面を 0.5 秒ごとに記録・Enter の前後で relay / sha / AX value / app.log を読む）・fg_lease / fg_front は stdin を待たず前面を見張る（上限 60 s・切れたら未測）・
  手ごとの時刻と読み戻しの note（前面の pid など）を SHIBUBU_QA_CHAPTERS（TSV）に書く（SHIBUBU_QA_REC_T0 = 録画の始めの epoch）・SHIBUBU_QA_FROM=<手番> で手前の手を飛ばす・lies / D-g は判定外（exit に数えない）。
exit: 0 ok / 1 赤（台本の嘘・D-g の violation・手の失敗）/ 2 使い方 / 3 起動できない / 5 owner 使用中で未測 / 8 本物の AI（REHEARSE_LLM=real）の owner の ok（SHIBUBU_QA_LLM_OK）が無い / 6 未測あり（driver の無い手・手の未測・AX の木が無い等の tally の未測。surface の oracle の対が未実装という標準の未測は数えない）
"""
import argparse, collections, csv, difflib, glob, hashlib, importlib.util, json, os, re, select, shutil, sqlite3, subprocess, sys, tempfile, time
from dataclasses import asdict

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, HERE)
import labels as L  # noqa: E402  label の 1 か所の表（EN / JP の対・identifier）
import tally as T  # noqa: E402  数え方の正本（R1 と R2 の両方を数える count_runs）
DEMO = os.environ.get("DEMO_SCRIPT", os.path.join(REPO, "docs/demo/full.md"))
SHORT = os.path.join(REPO, "docs/demo/short.md")  # 録画する短い台本（full.md の 27 手は回帰の試験）
TARGET_S = 600  # 台本 10 分（YC_WAVES G-final の時間の内訳）

# 手 → driver が無い理由と手本（W15-b49 で 4 手とも配線して空）。driver の無い BG の手が出来たらここに足す（--dry-run の表と run の「未測」に出る）。
NO_DRIVER = {}
# 手 → 外へ効く手の門（--dry-run の表に出す）。門が開いていない run は打たず「未測」で返す。
EXTERNAL = {"b4": "外へ効く: GFINAL_REALLY=1 の時だけ書く・Issue #1"}
FG_PREFIX = "fg_"
# 手 → run の外で要る段取り（--dry-run の表に出す）。Mirror は書類の絶対 path で窓を選ぶので、README を init の後に後ろへ開いておく
SETUP = {"mirror_consent": "+ TextEdit で README.md を open -g -n -a で後ろに開く・終わりに自分の pid だけ終了"}


def uses_readme_te(r):
    """この手が、run の外で後ろに開いた README の TextEdit を使うか（Mirror here の同意・ask_ai の宛先）。full.md は mirror_consent（#22）が持つが、short.md には無い。"""
    return r["who"] == "BG" and (r["key"] in SETUP or r["key"] == "ask_ai" or (r["key"] == "goto_tab" and "Mirror here" in r["doing"]))


def readme_te_row(rows):
    """README の TextEdit の段取りを表に出す手（SETUP の key の手があればそれ・無ければ使う最初の手）。無ければ None。"""
    return next((r for r in rows if r["who"] == "BG" and r["key"] in SETUP), None) or next((r for r in rows if uses_readme_te(r)), None)


def te_missing(C):
    """README の TextEdit の pid が決まっていない理由: 開いていない（段取りが走っていない）か、開いたが自分の pid が 1 つに決まらないか、file の窓が出なかった（te_window_miss）。"""
    return C.state.get("te_why") or "README の TextEdit を開いていない（段取りが走っていない）"


# ---- 台本の読み込みと検査（--dry-run の本体） ----
def parse(path):
    text = open(path, encoding="utf-8").read()
    rows = []
    for ln in text.splitlines():
        m = re.match(r"^\| (\d+) \|(.*)\|\s*$", ln)
        if not m:
            continue
        cells = [c.strip() for c in ("| " + m.group(1) + " |" + m.group(2)).strip().strip("|").split("|")]
        if len(cells) != 7:
            raise SystemExit(f"rehearse: 台本の行 {m.group(1)} の欄が 7 つでない（{len(cells)}）")
        n, who, key, doing, sees, readback, sec = cells
        rows.append(dict(n=int(n), orig=int(n), who=who, key=key, doing=doing, sees=sees, readback=readback, sec=int(sec)))
    raw = len(re.findall(r"^\| \d+ \|", text, re.M))  # 表を読む道とは別に行を数える
    decl = re.search(r"^手の数: (\d+)", text, re.M)  # 台本が自分で言う手の数（行を消しても番号が続くだけでは赤にならないので）
    if not decl or int(decl.group(1)) != len(rows):
        raise SystemExit(f"rehearse: 台本の「手の数: {decl.group(1) if decl else '無し'}」≠ 表の手 {len(rows)}")
    if (om := re.search(r"^元の番号: ([\d,]+)", text, re.M)):  # short.md: 各手が full.md のどの手か。REQUIRES はこの番号で引く（数字の写しを 2 つ持たない）
        origs = [int(x) for x in om.group(1).split(",")]
        if len(origs) != len(rows):
            raise SystemExit(f"rehearse: 台本の「元の番号」{len(origs)} 件 ≠ 表の手 {len(rows)}")
        full = {r["n"]: r["key"] for r in parse(DEMO)[0]} if os.path.abspath(path) != os.path.abspath(DEMO) else {}
        for r, o in zip(rows, origs):
            r["orig"] = o
            if full.get(o) != r["key"]:
                raise SystemExit(f"rehearse: {r['n']}: 元の番号 {o} の手は full.md では {full.get(o)}・この台本では {r['key']}")
    return rows, raw


def csv_patterns(path):
    pats = []
    for r in csv.DictReader(open(path, encoding="utf-8")):
        parts = re.split(r"\{[^}]*\}", r["text"])
        if len(parts) > 1 and sum(len(x) for x in parts) < 3:  # 「{}」「{name} 秒」のような穴ばかりの型は何にでも当たるので使わない
            continue
        if not re.search(r"[0-9A-Za-z\u3040-\u30ff\u3400-\u9fff]", "".join(parts)):  # 「 · 」のような記号だけの型も何にでも当たる（`.+ · .+`）
            continue
        pats.append(re.compile(".+".join(re.escape(p) for p in parts), re.S))
    return pats


def fresh_strings_csv(out_dir):
    """台本と照らす文言の CSV を今の src からその場で作る -> CSV の path（`python3 -I scripts/qa/ui-strings.py --csv <out_dir>/strings.csv` を 1 回）。
    呼ぶ側（UI-1 の本体で check() の既定にする）: bad, quoted, pending = check(rows, raw, fresh_strings_csv(tmp_dir))
    保証する: gitignore の docs/internal/ui-pro/strings.csv と環境変数 DEMO_STRINGS を読まない（古い CSV で緑にならない）・ui-strings.py が
    exit ≠ 0 か CSV が空なら SystemExit(1)（文言 0 件で全部「CSV に無い」にも「一致」にもしない）。列は ui-strings.py --csv と同じ file,line,kind,lang,text。
    保証しない: 文言が画面に描かれるか（実機の手）・表（docs/ui-copy）の EN との一致（それは ui-strings.py --by en）。
    Wave: UI-1"""
    path = os.path.join(out_dir, "strings.csv")
    p = subprocess.run([sys.executable, "-I", os.path.join(REPO, "scripts/qa/ui-strings.py"), "--csv", path], capture_output=True, text=True)
    if p.returncode != 0:
        raise SystemExit(f"rehearse: ui-strings.py --csv が exit {p.returncode}: {(p.stderr or p.stdout)[-300:]}")
    with open(path, encoding="utf-8") as f:
        if sum(1 for _ in csv.DictReader(f)) == 0:
            raise SystemExit("rehearse: ui-strings.py --csv の文言が 0 件（台本を照らせない）")
    return path


def check(rows, raw, csv_path=None):
    """台本の検査 -> (赤の list, 照らした文言の数, 未収録の数)。csv_path が None なら今の src から fresh_strings_csv で作る（既定）。"""
    if csv_path is None:
        with tempfile.TemporaryDirectory(prefix="rehearse-strings-") as d:
            return check(rows, raw, fresh_strings_csv(d))
    bad = []
    if len(rows) != raw or [r["n"] for r in rows] != list(range(1, raw + 1)):
        bad.append(f"手の数 {len(rows)} ≠ 台本の行 {raw}（番号が 1 から続かない）")
    pats = csv_patterns(csv_path)
    quoted = pending = 0
    for r in rows:
        if r["who"] not in ("BG", "FG"):
            bad.append(f"{r['n']}: 誰が BG でも FG でもない（{r['who']}）")
        if (r["who"] == "FG") != r["key"].startswith(FG_PREFIX):
            bad.append(f"{r['n']}: 誰（{r['who']}）と手（{r['key']}）が合わない")
        if r["who"] == "BG" and r["key"] not in HANDLERS and r["key"] not in NO_DRIVER:
            bad.append(f"{r['n']}: 手 {r['key']} を知らない")
        pending += r["sees"].count("⟦未収録:")
        sees = re.sub(r"⟦未収録:[^⟧]*⟧", "", r["sees"])
        for s in re.findall(r"`([^`]+)`", sees):
            quoted += 1
            if not any(p.fullmatch(s) for p in pats):
                bad.append(f"{r['n']}: 画面の文言「{s}」が今の src に無い（嘘の文言）")
    return bad, quoted, pending


def fill(s, nonce, a=7, b=13):
    return s.replace("<nonce>", nonce).replace("<a>", str(a)).replace("<b>", str(b))


def items(row, nonce):
    return [fill(x, nonce) for x in re.findall(r"『([^』]*)』", row["doing"])]


def dry_run(rows, raw, nonce):
    bad, quoted, pending = check(rows, raw)
    for r in rows:
        tag = r["who"] + " " + r["key"]
        extra = ""
        if r["who"] == "BG":
            extra = "driver 無し: " + NO_DRIVER[r["key"]] if NO_DRIVER.get(r["key"]) else "driver 在り" + (f"（{EXTERNAL[r['key']]}）" if r["key"] in EXTERNAL else "")
        print(f"{r['n']:>2}. {tag:<22} {r['sec']:>3}s  {fill(r['doing'], nonce)[:90]}  " + (f"{{{SETUP2[r['key']] if r['key'] in SETUP2 else list(SETUP.values())[0]}}} " if r["who"] == "BG" and (r["key"] in SETUP2 or r is readme_te_row(rows)) else "") + f"[{extra or 'FG: 通さない'}]")
    fg = sum(r["who"] == "FG" for r in rows)
    nod = sum(1 for r in rows if r["who"] == "BG" and NO_DRIVER.get(r["key"]))
    print(f"手 {len(rows)} = 台本の行 {raw} · BG {len(rows) - fg}（driver 無し {nod}）· FG {fg} · 文言 {quoted} 件が CSV と一致 · 未収録 {pending} 件 · 目安 {sum(r['sec'] for r in rows)} 秒")
    for b in bad:
        print("赤:", b, file=sys.stderr)
    return 1 if bad else 0


# ---- 実機（BG）。primitive は docs/evidence/W15/drive-w15.py と同じ（統合は live で測れた後） ----
class Unmeasured(Exception):
    pass


class DriverDown(Exception):
    """同じ原文の driver error が SAME_ERR_MAX 手続いた = 続けても測れない（W15-b48・255 手が全部 window_id 欠落で 2 時間走った）。"""


SAME_ERR_MAX = 3


def step_args(j, args):
    """cua-driver の呼びに pid・session・window_id（run.json の物）を足す。get_window_state / set_value は window_id が必須（drive-w12w1c / drive-w2b-ax と同じ形）。"""
    return {**args, "pid": j["pid"], "window_id": j["window_id"], "session": j["session"]}


class Ctx:
    def __init__(self, R, app, nonce):
        self.R, self.app, self.nonce, self.n, self.state = R, app, nonce, 0, {}
        self.repo = os.path.join(R, "shibubu-yc-demo")
        self.depth = 8


class LlmGate(Exception):
    """REHEARSE_LLM=real なのに owner の ok（SHIBUBU_QA_LLM_OK）が無い・claude CLI が無い。main は rc 8（ok 無し）/ 3（CLI 無し）で返す。"""

    def __init__(self, msg, rc):
        super().__init__(msg)
        self.rc = rc


LLM_GATE_RC = 8  # exit の表: owner の ok（quota）が無い。0 ok / 1 赤 / 2 使い方 / 3 起動できない / 5 owner 使用中 / 6 未測 / 7 driver down / 8 本物の AI の ok が無い


SCRIPT = "full"  # 回している台本（main が置く）: full（回帰の 27 手）| short（録画する短い台本）| scene2。本物の AI の上限の既定を台本ごとに引く
# 台本 -> 本物の AI の上限の既定 (calls, secs)。full は r26 / r27 / r28 の llm-calls.jsonl（9 回・59〜81 秒）に余裕を足した値。
# short は plan 1 + edit 1 + ask_ai の interpret / generate を数えた見積もり（r27 の 9 回から Terminal・TextEdit・Mirror 2 つ目の分を除く）で、本物の AI の run の実測は 5 回 86 秒（cu.VNsZ）。
# L2（118 通目）から手 1 は Home の会話の talk で 1 回増える（full 9→10・short 5→6 の見積もり・どちらも上限の中）。TALK-1101 から手 1 は `Build now` だけ（Preview を押さない = plan の読みは Build now の 1 回・回数は前と同じ）。上限は 1 つの頼みごと（一言・Build now で数え直す）。偽の run は上限を渡さない = 製品の既定 10 回（偽の full の回数は未測）。
# scene2 は本物の AI の run の実測が無い（見積もり: plan 1 + 4 つの app の手）。REHEARSE_LLM_CALLS / REHEARSE_LLM_SECS が在ればそちらが勝つ
LLM_BUDGET = {"full": (12, 900), "short": (8, 300), "scene2": (8, 600)}


def llm_mode():
    """本物の AI（real）が既定（owner 10-29 104 通目「常に実際に本物のaiを使いながらtestしてください」）。偽は REHEARSE_LLM=fake と書いた時だけ（道具の試験 test.sh は fake を置く）。"""
    return "fake" if os.environ.get("REHEARSE_LLM") == "fake" else "real"


def mark_ai(R):
    """run.json に ai = real|fake を刻み、報告の頭の行（`AI: fake` / `AI: real`）を返す。偽で回した run を本物の通しと読み違えない。"""
    p = os.path.join(R, "run.json")
    d = json.load(open(p))
    d["ai"] = llm_mode()
    json.dump(d, open(p, "w"), sort_keys=True)
    return f"AI: {d['ai']}"


def llm_env():
    """本物の AI の時（既定。REHEARSE_LLM=fake でない時）run.sh の start に渡す env（本物の claude CLI・上限）を返す。REHEARSE_LLM=fake は None = 偽の CLI。
    門: SHIBUBU_QA_LLM_OK（owner が chat で ok を出した語）が空なら LlmGate(rc 8)・CLI が見つからなければ LlmGate(rc 3)。上限は LLM_BUDGET の台本ごとの既定。REHEARSE_LLM_CALLS / REHEARSE_LLM_SECS で run ごとに置き換える（製品の既定 10 回 600 秒に頼らない）。
    CLI は SHIBUBU_LLM_CLI（絶対 path）か PATH の claude。回数と秒は製品が <data>/llm-calls.jsonl に 1 行ずつ書く（at_ms・name・ms・outcome）。"""
    if llm_mode() != "real":
        return None
    if not os.environ.get("SHIBUBU_QA_LLM_OK"):
        raise LlmGate("owner の ok が無い（本物の AI は quota を使う: SHIBUBU_QA_LLM_OK に owner が chat で出した語を置く）", LLM_GATE_RC)
    cli = os.environ.get("SHIBUBU_LLM_CLI") or shutil.which("claude")
    if not cli or not os.path.isabs(cli) or not os.access(cli, os.X_OK):
        raise LlmGate(f"claude CLI が無い（{cli or 'PATH に claude が無い'}）", 3)
    return {"SHIBUBU_QA_LLM": "1", "SHIBUBU_LLM_CLI": cli,
            "SHIBUBU_LLM_BUDGET": f"calls={os.environ.get('REHEARSE_LLM_CALLS') or LLM_BUDGET[SCRIPT][0]},secs={os.environ.get('REHEARSE_LLM_SECS') or LLM_BUDGET[SCRIPT][1]}"}


def llm_scale():
    """本物の model は偽の CLI（約 1.5 秒）より遅い: real の時だけ #1 / #20 の待ちの上限を伸ばす倍率（REHEARSE_LLM_WAIT_X・既定 2）。fake は 1。"""
    return float(os.environ.get("REHEARSE_LLM_WAIT_X") or 2) if llm_mode() == "real" else 1.0


def write_llm_json(R, cli=None):
    """<run>/llm.json = {mode, calls, secs, cli, budget}。calls = <data>/llm-calls.jsonl の行数（失敗の行も 1 回）・secs = 行の ms の合計。tally.py が mode を読む（real の run を fake と混ぜて数えない）。"""
    real = llm_mode() == "real"
    rows = []
    try:
        rows = [json.loads(l) for l in open(os.path.join(R, "data", "llm-calls.jsonl")) if l.strip()]
    except (OSError, ValueError):
        pass
    d = {"mode": "real" if real else "fake", "calls": len(rows), "secs": round(sum(r.get("ms", 0) for r in rows) / 1000, 1), "cli": (llm_env() or {}).get("SHIBUBU_LLM_CLI") if real else None,
         "budget": (llm_env() or {}).get("SHIBUBU_LLM_BUDGET") if real else None, "outcomes": sorted({r.get("outcome") for r in rows if r.get("outcome")})}
    json.dump(d, open(os.path.join(R, "llm.json"), "w"), ensure_ascii=False)
    return d


def run_sh(*a, ws=None, fake=None):
    env = dict(os.environ)
    env.pop("ANTHROPIC_API_KEY", None)
    env.pop("SHIBUBU_WORKSPACE_DIR", None)
    if ws:
        env["SHIBUBU_WORKSPACE_DIR"] = ws
    real = llm_env()
    if real:  # 本物の claude CLI（owner の ok の後だけ・llm_env が門）。偽の CLI は探させない
        env.update(real)
        env["SHIBUBU_QA_APP_ARGS"] = "-NSAppSleepDisabled YES"
    elif fake:  # 偽の CLI（owner の quota を使わない・fake-claude.sh）を製品に探させる
        env["SHIBUBU_QA_LLM"] = "1"
        env["SHIBUBU_LLM_CLI"] = os.path.join(fake, "claude")
        env["SHIBUBU_QA_APP_ARGS"] = "-NSAppSleepDisabled YES"  # App Nap で裏の AX が遅れない（その process の引数だけ・owner の defaults は書かない）
    if a[:1] == ("start",):
        env["KANGAE_MIRROR_FRAME_LOG"] = os.path.join(a[1], "frames.jsonl")  # e0: 製品の FrameLog
    p = subprocess.run(["bash", os.path.join(HERE, "run.sh"), *a], capture_output=True, text=True, env=env)
    return p.returncode, (p.stderr or "").strip()[-300:]


def step(C, tool, args, *extra, own=False):
    """own=True = 別の app の窓（自分の TextEdit）への読むだけの手: pid・window_id は args の物（run.json の Shibubu の物を足さない）。"""
    C.n += 1
    name = f"{C.n:02d}-{tool}"
    j = json.load(open(C.R + "/run.json"))
    rc, err = run_sh("step", C.R, name, tool, json.dumps({"session": j["session"], **args} if own else step_args(j, args), ensure_ascii=False), *extra)
    if rc == 1:
        raise RuntimeError("D-g の violation: " + name)
    try:
        out = json.load(open(f"{C.R}/steps/{name}.cua.json"))
    except Exception:
        try:
            raw = open(f"{C.R}/steps/{name}.cua.json", encoding="utf-8").read()
        except OSError:
            raw = err
        out = {"raw": raw or err}
    line = next((x.strip() for x in str(out.get("raw", "")).splitlines() if x.strip()), "") if "raw" in out else ""
    last, k = C.state.get("same_err", ("", 0))
    C.state["same_err"] = (line, k + 1 if line == last else 1) if line else ("", 0)
    if C.state["same_err"][1] >= SAME_ERR_MAX:
        raise DriverDown(line[:120])
    return out


def elements(C):
    """AX の要素。動作の無い AXStaticText（状態の 1 行 = 承認 / 拒否 / 結果）は cua-driver の elements に入らず tree_markdown の `- AXStaticText = "…"` にだけ出る
    （demo-loop r2 の #18: 「Approval needed」を 42 回読んで見えなかった）→ その行を md=True（押せない・find は飛ばす）で後ろに足す。
    動作の無い AXImage（Mirror の矩形 `Not running · …`）も同じく tree_markdown の `- AXImage "…"`（label・`=` 無し）にだけ出る（r10 の #26: 画面に在るのに d2 が見えず未測）→ label の md で足す。"""
    o = step(C, "get_window_state", {"timeout_ms": 10000, "max_depth": C.depth, "include_screenshot": False})
    md = o.get("tree_markdown") or ""
    return ((o.get("elements") or []) + [{"role": r, "value": v, "md": True} for r, v in re.findall(r'^\s*- (AX\w+) = "(.*)"$', md, re.M)]
            + [{"role": r, "label": v, "md": True} for r, v in re.findall(r'^\s*- (AX\w+) "(.*?)"(?: \[.*\])?$', md, re.M)])


def find(C, role, label, tries=20, contains=False):
    for k in range(tries):
        for e in elements(C):
            if not e.get("md") and (role is None or e.get("role") == role) and L.hit(e, label, contains):
                return e
        if k + 1 < tries:  # B64: 最後の試みの後は寝ない（try_press の 1 回の読みで 2 秒寝ていた）
            time.sleep(2)
    raise Unmeasured(f"{role or '*'}「{label}」が AX に出ない")


def press(C, role, label, expect=None, e=None):
    """expect = run.sh step の --expect（L2: 宣言した効果が前後の木で見えなければ tally が dead_click と数える）。
    e = 呼ぶ側が直前の読みで見つけた要素（B64: 同じ木を読み直さない・token は次の読みまで有効）。expect を付けるなら e はこの手の直前の読みの物（tally の前の木）。"""
    o = step(C, "click", {"element_token": (e or find(C, role, label))["element_token"]}, *(["--expect", expect] if expect else []))
    if o.get("effect") == "refused":
        raise Unmeasured(f"「{label}」が refused")


def try_press(C, role, label):
    """在れば押す（無ければ何もしない・待たない）-> 押したか。"""
    try:
        e = find(C, role, label, tries=1)
    except Unmeasured:
        return False
    step(C, "click", {"element_token": e["element_token"]})
    return True


def wait_enabled(C, role, label, reads=20):
    """UI-4a: header の欄は AI が一言 / 変更を読んでいる間（busy・CLI を確かめている間も）灰色になる（header.rs `add_enabled_ui(!busy)`）→ AX の enabled が false。
    灰色の欄に打つと type_text が refused になり手が「AX で打てない」の未測になるので、有効になるまで読み直す（読む回数で数える・時計に頼らない）。有効にならなければ未測（赤にしない）。"""
    for k in range(reads):
        e = find(C, role, label)
        if e.get("enabled") is not False:
            return e
        time.sleep(2)
    raise Unmeasured(f"「{label}」の欄が灰色のまま（AI が読んでいる間は送れない・{reads} 回読んで）")


def send_field(C, role, label, text, enter=False, expect=None):
    # B64（demoshort.k7oc）: 1 行に 4 回読んでいた（wait_enabled・press の find・打つ前・Enter の前）→ wait_enabled の要素で押して打つ。
    # Enter の前の読みは expect の時だけ（tally は手の直前の読みを前の木に使い、間に別の手が居ると未測）
    f = wait_enabled(C, role, label)
    press(C, role, label, e=f)
    o = step(C, "type_text", {"text": text, "element_token": f["element_token"]}, "--needle", text)
    if o.get("effect") == "partial":  # 混雑で途中までしか入らなかった: 1 回だけ送り直す（同じなら手が未測・drive-integ1.py の send_field と同じ）
        o = step(C, "type_text", {"text": text, "element_token": f["element_token"]}, "--needle", text)
        if o.get("effect") == "partial":
            raise Unmeasured(f"「{label}」が途中までしか入らない（2 回送って 2 回とも）")
    if o.get("effect") == "refused" or "raw" in o:
        raise Unmeasured(f"「{label}」に AX で打てない")
    if enter:
        step(C, "press_key", {"key": "return", "element_token": (find(C, role, label) if expect else f)["element_token"]}, *(["--expect", expect] if expect else []))


def ax_ids(C):
    step(C, "ax_tree", {})
    try:
        return [e.get("identifier") or "" for e in json.load(open(f"{C.R}/ax/{C.n:02d}-ax_tree.json")).get("elements") or []]
    except Exception:
        return []


# ---- 読み戻し（W15-b52）: 画面の「押せた」ではなく、製品の DB / disk / ps の状態が台本どおりに変わった事を別の道で読む ----
def canon(d):
    return hashlib.sha256(json.dumps(d, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def db_state(C):
    """C.R/data/kangae.sqlite3 を ro で読む -> 最後の Project の {rev, head_sha, pages(id の順), page_sha, d(head の JSON), rows{revision: {origin, ops, sha}}}。読めなければ None。
    Undo / Redo は projects の行を revision の行の json に置き換えるだけ（行は書かない）なので、A3 は head_sha と rows[rev].sha の一致で読む。"""
    try:
        con = sqlite3.connect(f"file:{os.path.join(C.R, 'data', 'kangae.sqlite3')}?mode=ro", uri=True, timeout=5)
        try:
            row = con.execute("SELECT id, json FROM projects ORDER BY created_at DESC LIMIT 1").fetchone()
            if not row:
                return None
            pid, d = row[0], json.loads(row[1])
            rows = {r: dict(origin=o, ops=json.loads(ops), sha=canon(json.loads(j)))
                    for r, o, ops, j in con.execute("SELECT revision, origin, ops_json, json FROM project_revisions WHERE project_id=?", (pid,))}
        finally:
            con.close()
        pages = d.get("pages") or []
        return dict(id=pid, rev=d.get("revision"), head_sha=canon(d), pages=[p["id"] for p in pages], page_sha={p["id"]: canon(p) for p in pages}, d=d, rows=rows)
    except (sqlite3.Error, ValueError, KeyError, TypeError):
        return None


def wait_db(C, pred, secs=30):
    st = None
    for _ in range(int(secs / 2) + 1):
        st = db_state(C)
        if st and pred(st):
            return st
        time.sleep(2)
    return st


def has_key(n, k):
    if isinstance(n, dict):
        return n.get("key") == k or any(has_key(v, k) for v in n.values())
    return isinstance(n, list) and any(has_key(v, k) for v in n)


def holder(d, key):
    """面の node（key）が居る Page の id。"""
    return next((p["id"] for p in d.get("pages") or [] if has_key(p.get("root"), key)), None)


def surface_pages(d, pred):
    """Project d で pred(面の spec) に当たる面（source_surface の props.surface_ref）を持つ Page の題名（= tab の label）を Page の順で。
    面は nl_edit で動く（手 13 の後 Excalidraw は Tests・手 14 の後 README の Mirror は Diagram）ので tab の名前を決め打ちしない。"""
    ids = {s.get("id") for s in d.get("surface_specs") or [] if pred(s)}
    def refs(n):
        if isinstance(n, dict):
            return (n.get("component") == "source_surface" and (n.get("props") or {}).get("surface_ref") in ids) or any(refs(v) for v in n.values())
        return isinstance(n, list) and any(refs(v) for v in n)
    return [p.get("title") for p in d.get("pages") or [] if refs(p.get("root"))]


def project_mod():
    spec = importlib.util.spec_from_file_location("oracle_project", os.path.join(HERE, "oracle", "project.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def fresh_projects(C, since):
    """oracle/project.py の Project のうち revision 1 が since（epoch 秒）より後の物。DB が無い・読めなければ空（未測を赤にしない）。"""
    import datetime
    try:
        ps = project_mod().read_projects(os.path.join(C.R, "data"), since)["projects"]
    except (FileNotFoundError, sqlite3.Error):
        return []
    return [p for p in ps if p["rev1_created_at"] and datetime.datetime.fromisoformat(p["rev1_created_at"].replace("Z", "+00:00")).timestamp() > since]


def h_generate(C, row):
    said = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. Put each on its own page."  # nonce は付けない: I1 の Project は送信の時刻（t_send）で選ぶ（oracle/project.py --since）・nonce は Terminal の手（C1）が持つ
    said = (items(row, C.nonce) if row.get("doing") else [said])[0]  # scene 2 は台本の『』の一言（band の一言）
    t_send = time.time()
    send_field(C, L.LABELS["goal"][0], "goal", said)  # UI-4a: header の 1 つの欄 = AXTextField（role は表の 1 か所・labels.py）
    # TALK-1101（owner 191 通目）: Home の右端 `↵ Send` は作らずに会話へ → 案の card の先頭の `Build now`（最初から出る）で作る（`Preview plan` は押さない）。
    # 案は model の答え（本物の AI は問いを返して案を出さない事がある）か、偽の CLI の失敗の後の local_talk（長い一言はその文が案）
    press(C, "AXButton", "send")
    build = []
    for _ in range(int(40 * llm_scale())):  # 本物の AI の run は待ちの上限を伸ばす（REHEARSE_LLM_WAIT_X・既定 2）
        els = elements(C)
        build = [e for e in els if e.get("role") == "AXButton" and L.hit(e, "accept")]
        # 作る前に答えの要る問い（needs）の間は灰・card に `Answer this first: …`（agent_rail.rs の Card::plan）→ 待たずに未測（問いの原文を付ける）。
        # cua-driver は灰の button を elements に入れず tree_markdown の `- AXButton "Build now"`（token 無し = md）にだけ出す（demoshort.P3V4）。
        # 問いの行の無い灰は待ち（CLI を確かめている・AI が読んでいる）= 読み直す
        if build and all(e.get("md") or e.get("enabled") is False for e in build):
            q = next((v for e in els if (v := e.get("value") or e.get("label") or "").startswith("Answer this first")), "")
            if q:
                raise Unmeasured(f"L2: `Build now` が灰（AI が作る前に答えの要る問いを返した）: {q[:200]}")
        elif build:
            break
        time.sleep(2)
    else:
        raise Unmeasured("L2: 案の card の `Build now` が押せる形で出ない（AI が案でなく問いだけを返したか、会話の答えが来ない・待ちが続く）")
    # 「Build now の前に Project が無い」= 押す直前に読む
    if (made := fresh_projects(C, t_send)):
        raise RuntimeError(f"L2: `Build now` を押す前に Project ができた（{len(made)} 件）")
    press(C, "AXButton", "accept", e=next((e for e in build if not e.get("md") and e.get("enabled") is not False), None))
    # Build now は作る時の AI の読みを 1 回（答えていない問いは AI が選ぶ）→ 作って Project を開く = 待ちは生成の長さ
    find(C, "AXTextField", "change", tries=int(80 * llm_scale()))  # Project の欄（CHAT-ONE-1101 から Chat の tab の chat bar の `What to create` だけ = 畳んだ panel には無い・送ると会話の案）
    p = None  # I1: revision 1 の origin が generate・送信より後・Page 2 枚以上・面の spec 1 つ以上（oracle/project.py）
    for _ in range(15):
        try:
            p = project_mod().read_projects(os.path.join(C.R, "data"), t_send)
        except (FileNotFoundError, sqlite3.Error):
            p = None
        if p and p["ok"]:
            return
        time.sleep(2)
    if p is None:
        raise Unmeasured("DB が読めない（I1）")
    raise RuntimeError("I1: Project が generate / 2 Page / 面 1 つ以上でない: " + json.dumps(p["projects"], ensure_ascii=False)[:200])


def h_add_terminal(C, row):
    press(C, "AXButton", "add_menu")  # MIRROR-FRAME-1101: Add terminal は Page の右下の ＋（Add to page）の menu の中
    press(C, "AXButton", "add_term", expect="ax:*")
    elements(C)  # 効果の後の木（get_window_state）。terminal:* は ax_tree の側で読む
    for _ in range(15):
        if any(i.startswith("terminal:") for i in ax_ids(C)):
            return
        time.sleep(2)
    raise Unmeasured("terminal:* が AX に出ない")


def term_text(C):
    return "\n".join((e.get("value") or e.get("label") or "") for e in elements(C) if e.get("role") == "AXStaticText")


def term_pid(text):
    m = re.search(r" · pid (\d+)", text)
    return int(m.group(1)) if m else None


def wait_term(C, pred, secs):
    """Terminal の出力（AXStaticText）が pred を満たすまで待つ -> その時の全文 / 満たさなければ None。"""
    for _ in range(int(secs / 2) + 1):
        t = term_text(C)
        if pred(t):
            return t
        time.sleep(2)
    return None


def ps_col(pid, col):
    return subprocess.run(["/bin/ps", "-o", col, "-p", str(pid)], capture_output=True, text=True).stdout.strip()


def line_in(text, s):
    return re.search(r"(?m)^" + re.escape(s) + r"[ \r]*$", text) is not None


def check_c1(C):
    """C1: 出力に nonce と積（<a>*<b>）と <nonce>:1 の行・状態行の pid の ps が 親 = Shibubu・tty あり・comm が zsh か bash（drive-w5.py と同じ読み）。"""
    want = (C.nonce, str(7 * 13), f"{C.nonce}:1")
    text = wait_term(C, lambda t: all(line_in(t, w) for w in want), 40)
    if text is None:
        raise RuntimeError("C1: Terminal の出力に " + " / ".join(want) + " の行が揃わない")
    pid = term_pid(text)
    if pid is None:
        raise Unmeasured("状態行に pid が出ない（C1）")
    ppid, tty, comm = (ps_col(pid, c) for c in ("ppid=", "tty=", "comm="))
    if ppid != str(json.load(open(C.R + "/run.json"))["pid"]) or not tty.startswith("ttys") or os.path.basename(comm).lstrip("-") not in ("zsh", "bash"):
        raise RuntimeError(f"C1: ps が ppid={ppid} tty={tty} comm={comm}（親 = Shibubu・tty あり・zsh か bash でない）")


def check_probe(C):
    p = os.path.join(C.repo, "undo-probe.txt")
    for _ in range(10):
        if os.path.exists(p) and open(p, encoding="utf-8").read().strip() == C.nonce:
            return
        time.sleep(2)
    raise RuntimeError("C1: repo の undo-probe.txt に nonce が書かれない（A3 の外の効果の元）")


def check_c2(C):
    """C2: sleep 15 を打った Terminal を 17 s 後に別 Page から戻って読む -> `done` の行が在り、状態行の pid が打った時と同じ。"""
    c2 = C.state.pop("c2")
    time.sleep(max(0, c2["t"] + 17 - time.time()))
    text = wait_term(C, lambda t: line_in(t, "done"), 30)
    if text is None:
        raise RuntimeError("C2: sleep 15 の後に `done` の行が Terminal に無い")
    if term_pid(text) != c2["pid"]:
        raise RuntimeError(f"C2: Terminal の pid が {c2['pid']} → {term_pid(text)} に変わった")


def term_field(C):
    """L7（yc/snap-b70 696ef28c）: Terminal の下の欄（`Type in the terminal`）は既定で隠れる → 帯の `•••`（`Terminal options`・menu は上に開く）の `Input field (for IME)` で出す。
    押すたびに出る / 隠れるが入れ替わる（Project ごとに保存）ので、同じ 1 つの木で欄が無く ••• が在り帯が畳まれていない（`Show terminal` が無い）時だけ押す。
    ••• の無い古い .app・畳んだ帯は押さない（send_field の待ちが前と同じに測る）。
    TERM-ONE-1101 の後の .app は欄も ••• も無い → 押さずに戻り、send_field が未測にする（BG の手は grid に打てない・diagrams §54）。"""
    els = [e for e in elements(C) if not e.get("md")]
    seen = lambda role, key: any(e.get("role") == role and L.hit(e, key) for e in els)
    if seen("AXTextField", "term_in") or not seen("AXButton", "term_opts") or seen("AXButton", "Show terminal"):
        return
    press(C, "AXButton", "term_opts")
    press(C, "AXButton", "term_ime")


def h_term_type(C, row):
    cmds = items(row, C.nonce)
    term_field(C)
    for cmd in cmds:
        send_field(C, "AXTextField", "term_in", cmd, enter=True)
    if any(c.startswith("false;") for c in cmds):
        check_c1(C)
    if any("undo-probe.txt" in c for c in cmds):
        check_probe(C)
    if any(c.startswith("sleep") for c in cmds):
        pid = term_pid(term_text(C))
        if pid is None:
            raise Unmeasured("状態行に pid が出ない（C2 の始め）")
        C.state["c2"] = dict(pid=pid, t=time.time())


def consents(C):
    try:
        con = sqlite3.connect(f"file:{os.path.join(C.R, 'data', 'kangae.sqlite3')}?mode=ro", uri=True, timeout=5)
        try:
            return con.execute("SELECT COUNT(*) FROM ui_state WHERE session_id='mirror-consent'").fetchone()[0]
        finally:
            con.close()
    except sqlite3.Error:
        return None


def consent_docs(C):
    """#5: Docs の「Mirror here」を押して TextEdit の Mirror に同意（製品の口で・docs/demo/mirror-consent.md）-> surface-frame:docs が AX に出て、同意の記録（ui_state の mirror-consent）が 1 件以上。"""
    try:
        press(C, "AXButton", "mirror", e=find(C, "AXButton", "mirror", tries=8))
    except Unmeasured:
        pass  # 同意済みか TextEdit の窓が無い。下の surface-frame が決める
    sid = C.state["docs_sid"] = surface_id(C, "docs")
    for _ in range(10):
        if f"surface-frame:{sid}" in ax_ids(C):
            break
        time.sleep(3)
    else:
        raise Unmeasured(f"surface-frame:{sid} が AX に出ない（「Mirror here」が出ない・TextEdit の README が後ろに無い・画面収録の許可）")
    n = consents(C)
    if n is None:
        raise Unmeasured("DB が読めない（同意の記録）")
    if n < 1:
        # 矩形は Ambiguous / NoConsent の面でも出る → 製品が app.log に書いた最後の Mirror docs の行を添える（どの面で止まったか）
        why = [l.strip() for l in open(os.path.join(C.R, "app.log"), errors="replace") if f"Mirror {sid}:" in l][-1:] if os.path.exists(os.path.join(C.R, "app.log")) else []
        if not C.state.get("te_pid"):  # 前提（driver が後ろに開く README の TextEdit）が無い = 製品の赤ではない（r24: load1 204 で open の後に新しい pid が出ず・製品は NoWindow）
            raise Unmeasured(f"setup: {te_missing(C)}" + (f"（{why[0][:160]}）" if why else ""))
        raise RuntimeError("Mirror の矩形は出たが、同意の記録（ui_state mirror-consent）が無い" + (f"（{why[0][:160]}）" if why else ""))
    C.state["consents"] = n


def h_goto_tab(C, row):
    for t in items(row, C.nonce):
        # UI-5a: Page の tab は WidgetInfo::selected（toggled）= macOS では AXCheckBox（accesskit_macos 0.18.1 node.rs:62）。名前は Project ごとに違う → 種類で引く（W15-b71）
        press_tab(C, TAB_KIND[t], expect="ax:*") if t in TAB_KIND else press(C, "AXCheckBox", t, expect="ax:*")
        elements(C)  # 効果の後の木（tally の後の木 = 手の直後の読み。B64: 前の sleep 2 は外した = click の手の後始末で 1 秒余り経っている）
        if TAB_KIND.get(t) == "dev" and C.state.get("c2"):
            check_c2(C)
    if "Mirror here" in row["doing"]:
        consent_docs(C)


def h_b1(C, row):
    nonce = f"n-demo-b1-{C.nonce}"
    b1_flow(C, nonce, [nonce], "B1")


def b1_flow(C, nonce, want, out):
    """Excalidraw に Mermaid で nonce を置き → repo に保存 → 最新の .excalidraw を disk から読み、want の nonce が全部在るか（B1 は [n1]・B3 は [n1, n2]）。oracle/<out>.excalidraw.after.json に書く。"""
    sys.path.insert(0, os.path.join(REPO, "docs/evidence/W15"))
    import w15_judge as J
    depth, C.depth = C.depth, 30
    try:
        deselect(C)  # r9: 前の run が置いた図が選ばれたまま開いた（website data は全 run で 1 つ・surface/mod.rs の SHARED_WEB_STORE）→ More tools が木に無かった
        for label in ("More tools", "Mermaid to Excalidraw"):
            step(C, "click", {"element_token": find(C, None, label, tries=8, contains=True)["element_token"]})
        end = time.time() + 20  # r7: click の直後の木では dialog がまだ描かれていない（load 35・次の読みでは開いていた）→ 1 回で決めず時計で待つ
        while not J.mermaid_dialog_open(elements(C)):
            if time.time() >= end:
                raise Unmeasured("Mermaid の dialog が 20 秒で開かない")
            time.sleep(3)
        area = find(C, "AXTextArea", "", tries=3, contains=True)
        step(C, "set_value", {"value": f"flowchart TD\nA[{nonce}] --> B[docs]", "element_token": area["element_token"]}, "--needle", nonce)
        step(C, "click", {"element_token": find(C, "AXButton", "Insert", tries=8, contains=True)["element_token"]})
        deselect(C)  # r8: Insert の直後の木に AXWebArea が無く escape を飛ばし、置いた図の Stroke を主 menu と取り違えた
        step(C, "click", {"element_token": excalidraw_menu(C)["element_token"]})
        for label in ("Save to...", "Save to file"):
            step(C, "click", {"element_token": find(C, None, label, tries=8, contains=True)["element_token"]})
        step(C, "click", {"element_token": find(C, "AXButton", "save_repo", tries=15, contains=True)["element_token"]})
        time.sleep(2)
        docs = os.path.join(C.repo, "docs")
        files = sorted((os.path.join(docs, f) for f in os.listdir(docs) if f.endswith(".excalidraw")), key=os.path.getmtime) if os.path.isdir(docs) else []
        if not files:
            raise Unmeasured("docs に .excalidraw が無い")
        o = subprocess.run(["bash", os.path.join(HERE, "oracle/excalidraw.sh"), "read", files[-1], *want], capture_output=True, text=True)
        orc = json.loads(o.stdout) if o.stdout.strip() else None
        os.makedirs(C.R + "/oracle", exist_ok=True)
        json.dump(orc, open(f"{C.R}/oracle/{out}.excalidraw.after.json", "w"), ensure_ascii=False)
        if not all(J.b1_ok(orc, n) for n in want):
            raise RuntimeError(f"{out}: disk の file に nonce が無い")
    finally:
        C.depth = depth


def deselect(C):
    """Excalidraw の選択を外す（AXWebArea を find で待って escape）。選択中は More tools と主 menu が木に無く、面の右に Stroke・Background などの AXPopUpButton が出る。"""
    step(C, "press_key", {"key": "escape", "element_token": find(C, "AXWebArea", "", tries=5, contains=True)["element_token"]})


def excalidraw_menu(C, tries=8):
    """Excalidraw の主 menu = 名前の無い AXPopUpButton（r5 の通った木: [26] AXPopUpButton ""・名前の有る More tools / Stroke / Canvas background は別の物）。"""
    for _ in range(tries):
        e = next((e for e in elements(C) if e.get("role") == "AXPopUpButton" and not e.get("md") and not (e.get("label") or e.get("title"))), None)
        if e:
            return e
        time.sleep(2)
    raise Unmeasured("Excalidraw の menu（名前の無い AXPopUpButton）が AX に出ない")


def h_nl_edit(C, row):
    text = items(row, C.nonce)[0]
    if "notes.md" in text and not C.state.get("notes_pid"):  # 普通は起動の前に開いてある（main の open_notes）
        open_notes(C)
    before = db_state(C)
    d1 = "move TextEdit right" in text
    w0 = None
    if d1:  # D1: 前の窓（測れなければ Unmeasured を持ち越し、送る手は止めない）
        try:
            w0 = te_windows(C)
        except Unmeasured as e:
            w0 = e
    send_field(C, "AXTextField", "change", text, enter=True, expect="ax:*")
    elements(C)  # 効果の後の木（tally の後の木・B64: 前の sleep 2 は外した）
    if before is None:
        raise Unmeasured("DB が読めない（I2）")
    st = wait_db(C, lambda s: s["rev"] > before["rev"])
    if st is None or st["rev"] <= before["rev"]:
        raise RuntimeError("I2: 送ったのに Project の revision が増えない")
    check_edit(text, before, st)
    if d1:
        check_d1(C, w0)


def te_windows(C):
    """自分の TextEdit の書類の窓（CGWindowList・bin/windows once）→ 窓 id の順の [(window_id, x, y, w, h)]。pid / 道具が無ければ Unmeasured。
    100×100 未満は書類の窓でないので外す（menu bar の 1680×24・title bar の脇の 52×20 の一時の窓。r10 の #25: 52×20 が再起動の前だけ在って D1 が赤）。"""
    pid = C.state.get("te_pid")
    exe = os.path.join(HERE, "bin", "windows")
    if not pid:
        raise Unmeasured("自分の TextEdit の pid が無い（D1）")
    if not os.path.exists(exe):
        raise Unmeasured("bin/windows が無い（build.sh）")
    ws = json.loads(subprocess.run([exe, "once", "--pid", str(pid)], capture_output=True, text=True).stdout or "{}").get("windows") or []
    return sorted((w["window_id"], *(w["bounds"][k] for k in "xywh")) for w in ws if w.get("pid") == pid and w.get("layer") == 0 and w["bounds"]["w"] >= 100 and w["bounds"]["h"] >= 100)


def check_d1(C, w0):
    """D1: Mirror は映すだけ = 動かした前後で TextEdit の窓の id と bounds が同じ（本物の窓は動かさない）。前が 0 枚なら測っていない。"""
    if isinstance(w0, Unmeasured):
        raise w0
    if not w0:
        raise Unmeasured("D1: 前の TextEdit の窓が 0 枚（測っていない）")
    w1 = te_windows(C)
    if w0 != w1:
        raise RuntimeError(f"D1: TextEdit の窓が前後で違う（id・bounds）: {w0} → {w1}")


# 一言 → 当たる op の種類（ops_json の op 名）。「notes.md」は「add a Tests page」と同じ AddPage を持つので先
EDIT_OPS = (("notes.md", ["add_page", "add_surface"]), ("add a Tests page", ["add_page"]), ("move Excalidraw right", ["move_surface"]), ("move TextEdit right", ["move_surface"]))


def check_edit(text, before, st):
    """I2: head の revision の origin が nl_edit・ops が台本の種類と数どおり・AddPage なら Page が 1 枚増えて他の Page の hash が同じ・MoveSurface なら面が to_page へ（Excalidraw は右へ）・AddSurface は TextEdit の notes.md を映す Mirror。"""
    want = next((w for k, w in EDIT_OPS if k in text), None)
    if want is None:
        return
    last = st["rows"].get(st["rev"])
    if not last:
        raise RuntimeError(f"I2: revision {st['rev']} の行が project_revisions に無い")
    if last["origin"] != "nl_edit":
        raise RuntimeError("I2: revision の origin が nl_edit でない: " + last["origin"])
    got = sorted(o.get("op") for o in last["ops"])
    if got != want:
        raise RuntimeError(f"I2: ops が {got}（台本は {want}）")
    if "add_page" in want:
        if len(st["pages"]) != len(before["pages"]) + 1:
            raise RuntimeError(f"I2: Page が {len(before['pages'])} → {len(st['pages'])} 枚（1 枚増えるはず）")
        if any(st["page_sha"].get(i) != h for i, h in before["page_sha"].items()):
            raise RuntimeError("I2: 前からある Page の hash が変わった")
    if "move_surface" in want:
        op = next(o for o in last["ops"] if o["op"] == "move_surface")
        now, was = holder(st["d"], op["surface_key"]), holder(before["d"], op["surface_key"])
        if now != op["to_page"]:
            raise RuntimeError(f"I2: 面 {op['surface_key']} が {now} にあり、to_page {op['to_page']} でない")
        if " right" in text and (was not in st["pages"] or st["pages"].index(now) <= st["pages"].index(was)):
            raise RuntimeError(f"I2: 面が右の Page へ動いていない（{was} → {now}）")
    if "add_surface" in want:
        spec = next(o for o in last["ops"] if o["op"] == "add_surface")["spec"]
        dump = json.dumps(spec, ensure_ascii=False)
        if spec.get("backend") != "mirror" or "com.apple.TextEdit" not in dump or "notes.md" not in dump:
            raise RuntimeError("I2: 足した面が TextEdit の notes.md の Mirror でない: " + dump[:160])


def repo_shas(C):
    """A3 の「外の効果」: undo-probe.txt・図・README の sha（無ければ None）。"""
    return {n: sha_of(os.path.join(C.repo, n)) if os.path.exists(os.path.join(C.repo, n)) else None for n in ("undo-probe.txt", "docs/arch.excalidraw", "README.md")}


def undo_redo(C, label, back):
    """A3: 押す前後の Project の hash が、行き先の revision の行（Undo = 前・Redo = 後）の hash と一致し、外の効果の sha は変わらない。"""
    before, files = db_state(C), repo_shas(C)
    if before is None:
        raise Unmeasured("DB が読めない（A3）")
    press(C, "AXButton", label, expect="ax:*")
    elements(C)  # 効果の後の木（tally の後の木・B64: 前の sleep 2 は外した）
    st = wait_db(C, lambda s: s["head_sha"] != before["head_sha"], 20)
    if st is None or st["head_sha"] == before["head_sha"]:
        raise RuntimeError(f"A3: {label} を押したのに Project の hash が変わらない")
    if (st["rev"] < before["rev"]) != back or st["head_sha"] != (st["rows"].get(st["rev"]) or {}).get("sha"):
        raise RuntimeError(f"A3: {label} の後の Project（revision {st['rev']}）が、その revision の行の hash と一致しない（前は {before['rev']}）")
    if repo_shas(C) != files:
        raise RuntimeError(f"A3: {label} の前後で外の効果の sha が変わった: {files} → {repo_shas(C)}")


def h_undo(C, row):
    undo_redo(C, "undo", True)


def h_redo(C, row):
    undo_redo(C, "redo", False)


def frames(C):
    """今の Page に描かれた Mirror の矩形の AX の名前 {面の id: 名前}（surface-frame:<id>）。"""
    step(C, "ax_tree", {})
    try:
        els = json.load(open(f"{C.R}/ax/{C.n:02d}-ax_tree.json")).get("elements") or []
    except Exception:
        return {}
    return {e["identifier"].split(":", 1)[1]: e.get("label") or "" for e in els if (e.get("identifier") or "").startswith("surface-frame:")}


def wait_live(C, sid, secs):
    """面 sid（None = docs 以外の最初の面）が「Mirroring · …」になるまで secs 秒 -> (面の id, 最後に読んだ名前)。映れば名前は live の物。"""
    end, live = time.time() + secs, L.names("mirror_live")
    while True:
        fr = frames(C)
        s = sid or next((k for k in fr if k != C.state.get("docs_sid", "docs")), None)
        name = fr.get(s, "")
        if name.startswith(live) or time.time() >= end:
            return s, name
        time.sleep(3)


# 台本の tab の呼び名 -> 面の種類。tab の名前は Project ごとに違う（fake = Docs / Dev / Diagram・本物の AI = README / Dev terminal / Architecture 等・r18）ので、
# 製品の AX が出さない種類は押す直前の DB（surface_specs と Page の木）から引く（W15-b71）。
TAB_KIND = {"Docs": "docs", "Dev": "dev", "Diagram": "diagram"}
SURFACE_KIND = {
    "docs": lambda s: s.get("backend") == "mirror" and (s.get("target") or {}).get("file") == "README.md",
    "notes": lambda s: s.get("backend") == "mirror" and str((s.get("target") or {}).get("file") or "").endswith("notes.md"),
    "diagram": lambda s: s.get("backend") == "webview" and "excalidraw" in str((s.get("target") or {}).get("url") or ""),
}


def surface_id(C, kind):
    """押す直前の DB から、種類（docs / notes / diagram）の面の spec の id（= AX の surface-frame:<id>）。本物の AI は id を自分で決める（r21b: readme・fake = docs）ので決め打ちしない。
    DB が読めない・0 個・2 個以上は代用せず Unmeasured（b77）。"""
    st = db_state(C)
    if st is None:
        raise Unmeasured(f"DB が読めない（{kind} の面の id）")
    ids = [x.get("id") for x in st["d"].get("surface_specs") or [] if SURFACE_KIND[kind](x)]
    if len(ids) != 1:
        raise Unmeasured(f"{kind} の面の spec が {len(ids)} 個（{ids}）: " + ("その種類の面が無い" if not ids else "1 つに決まらない"))
    return ids[0]


def page_title(C, kind):
    """押す直前の DB から、種類（docs / notes / diagram = 面の spec・dev = terminal の部品）の面を持つ Page の題（= tab の AX の名前）。
    DB が読めない・0 枚・2 枚以上（1 つに決まらない）は代用せず Unmeasured。"""
    st = db_state(C)
    if st is None:
        raise Unmeasured(f"DB が読めない（{kind} の tab の題）")
    d = st["d"]
    if kind == "dev":
        def has_term(n):
            return isinstance(n, dict) and (n.get("component") == "terminal" or any(has_term(v) for v in n.get("children") or []))
        titles = [p.get("title") for p in d.get("pages") or [] if has_term(p.get("root"))]
    else:
        titles = surface_pages(d, SURFACE_KIND[kind])
    titles = list(dict.fromkeys(titles))
    if len(titles) != 1:
        raise Unmeasured(f"{kind} の面がある Page が {len(titles)} 枚（{titles}）: " + ("その種類の面が無い" if not titles else "1 つに決まらない"))
    return titles[0]


def press_tab(C, kind, expect=None):
    """kind の Page の tab を DB の題そのままで押す。AX にその名前の tab が 2 つ以上なら押さず Unmeasured（0 なら press が待つ）。"""
    title = page_title(C, kind)
    tabs = [e for e in elements(C) if e.get("role") == "AXCheckBox" and not e.get("md") and e.get("label") == title]
    if len(tabs) > 1:
        raise Unmeasured(f"AX に「{title}」の tab が 2 つ以上")
    press(C, "AXCheckBox", title, expect=expect, e=tabs[0] if tabs else None)  # B64: 数えた読みの tab を押す（0 なら press が待つ）
    return title


def press_project(C):
    """#25 の入口: sidebar の Project の行を DB の Project の title で押す（固定の「Dev environment」は fake の名前）。"""
    st = db_state(C)
    name = ((st or {}).get("d") or {}).get("name")
    if not name:
        raise Unmeasured("DB が読めない（Project の title）")
    press(C, "AXCheckBox", name)


def h_mirror_consent(C, row):
    """#22: 手 5 の同意は bundle id ごと → #21 が足した notes.md の Mirror の Page は同意の button 無しで映り、手 5 の README の Mirror も止まらない（full.md の 22）。
    r5 まで「Mirror here」を押していた（同意済みなので出ない = 93 秒探して未測）。今は notes の tab を押して矩形の名前で決める。
    赤 = notes.md が映らない（依存 G32 ＋ sendback 69 = mirror_view の other_live）/「Mirror here」がまた出た / 同意の記録が手 5 から変わった / Docs の Mirror が止まった。"""
    live = L.names("mirror_live")
    press_tab(C, "notes", expect="ax:*")  # #21 の notes.md の面がある Page
    sid, name = wait_live(C, None, 60)
    if not sid:
        raise RuntimeError("notes の Page に Mirror の矩形（surface-frame:*）が無い")
    if not name.startswith(live):
        raise RuntimeError(f"notes.md の Mirror が 60 秒で映らない（矩形の名前「{name[:80]}」）")
    if seen(C, L.names("mirror")[0], "AXButton"):
        raise RuntimeError("同意済みなのに「Mirror here」がまた出た")
    n = consents(C)
    if n != C.state.get("consents"):
        raise RuntimeError(f"同意の記録が手 5 の {C.state.get('consents')} 件から {n} 件に変わった")
    press_tab(C, "docs", expect="ax:*")  # docs の面は #14 で右の Page へ動く = その時点の DB で引く（r14 の嘘の赤）
    _, dname = wait_live(C, C.state.get("docs_sid") or surface_id(C, "docs"), 20)
    if not dname.startswith(live):
        raise RuntimeError(f"notes.md を映した後に README の Mirror が止まった（「{dname[:80]}」）")
    press_tab(C, "notes", expect="ax:*")  # #23 は notes の Page の矩形を撮る
    C.state["notes_sid"] = sid


def h_restart(C, row):
    """#25: 同じ data dir で起こし直し、B3（b3_check）と D1（TextEdit の窓 id・bounds が前後で同じ・Page の tab の順が DB の Page の順）を両方測る。
    片方が落ちても他方を測り、赤があれば赤・無ければ最初の未測を投げる。"""
    R1 = C.R
    try:
        w0 = te_windows(C)  # D1: 止める前の窓（測れなければ Unmeasured を持ち越し・h_nl_edit と同じ）
    except Unmeasured as e:
        w0 = e
    run_sh("stop", R1)
    R2 = R1 + "-restart"
    os.makedirs(R2, exist_ok=True)
    d2 = os.path.join(R2, "data")  # 同じ data dir（stop 済みで書き手は居ない・copy せず symlink）。repo も R1 の物のまま（C.repo は付け替えない）
    if os.path.islink(d2):
        os.unlink(d2)
    elif os.path.exists(d2):
        shutil.rmtree(d2)
    os.symlink(os.path.join(R1, "data"), d2)
    C.R = R2
    if os.path.isdir(os.path.join(R1, "fake")):  # 本物の AI の run は fake dir が無い
        shutil.copytree(os.path.join(R1, "fake"), os.path.join(R2, "fake"), dirs_exist_ok=True)
    rc, err = run_sh("start", R2, C.app, ws=C.repo, fake=os.path.join(R2, "fake"))
    if rc not in (0, 6):
        raise RuntimeError(f"再起動の start rc={rc} {err}")
    # rc 6 は 2 つ: owner 使用中で起こしていない（owner_active）と、起こした後の 00-launch の判定が未測（focus 無し等・app は居る = 初回の start と同じく続ける・r8 はこれで 26 / 27 を測れなかった）。
    # owner_active か window_id が null = app が居ない = 後続の手は測れない（空の緑にしない）
    try:
        j = json.load(open(R2 + "/run.json"))
    except (OSError, ValueError):
        j = {}  # start が run.json を書かずに落ちた = app が居ない
    if j.get("owner_active") or j.get("window_id") is None:
        raise DriverDown(f"再起動の start が app を起こせていない（rc={rc}・owner_active={j.get('owner_active')}）: {err[-100:]}")
    C.state["R1"], C.state["restarted"] = R1, R2
    try:
        press_project(C)  # 入口 = Project を開く（B3・D1・#26 の tab はこの後に在る。r9 は B1 の無い B3 が入口を押す前に抜け、D1 と #26 が tab を探して未測）
    except Unmeasured:  # UI-4b: 窓の幅 < 1200 は sidebar が畳んで始まる（Tier::sidebar_open）→ 入口の行が木に無い。header の ◧（AX の名前 `Show sidebar`）で開いて 1 回だけ押し直す
        if not try_press(C, "AXButton", "Show sidebar"):
            raise
        press_project(C)
    errs = []
    for check in (b3_check, lambda C_: check_d1_restart(C_, w0)):
        try:
            check(C)
        except (Unmeasured, RuntimeError) as e:
            errs.append(e)
    if errs:
        raise next((e for e in errs if isinstance(e, RuntimeError)), errs[0])


def check_d1_restart(C, w0):
    """#25 の D1: 起こし直した後の Page の tab（AXCheckBox・x の順）が DB の Page の題名の順と同じ・TextEdit の窓 id と bounds が止める前と同じ（check_d1）。"""
    st = db_state(C)
    if st is None:
        raise Unmeasured("DB が読めない（D1 の Page の順）")
    want = [p.get("title") for p in st["d"].get("pages") or []]
    tabs = [e.get("label") for e in sorted((e for e in elements(C) if e.get("role") == "AXCheckBox" and not e.get("md") and e.get("label") in want), key=lambda e: (e.get("frame") or {}).get("x", 0))]
    if not tabs:
        raise Unmeasured("起こし直した後に Page の tab が AX に無い（D1 の Page の順）")
    if sorted(tabs) != sorted(want):  # 狭い窓で tab が溢れて AX に無い等 = 順を比べられない（赤にしない）
        raise Unmeasured(f"起こし直した後の Page の tab {tabs} が DB の Page {want} と揃わない（順を比べられない）")
    if tabs != want:
        raise RuntimeError(f"D1: 起こし直した後の Page の tab の順 {tabs} が DB の順 {want} と違う")
    check_d1(C, w0)


def b3_check(C):
    """#25 の B3: 入口（h_restart が押す）の後に Excalidraw の面が居る Page（DB から・手 13 の後は Tests）へ移り、2 つ目の nonce を置いて repo に保存
    → 最新の .excalidraw に n1 と n2 の両方が在る（n1 = 再起動の前の B1 が置いた物）。B1 が済んでいなければ n1 が無いので未測。入口・Excalidraw の面が AX に出なければ未測（find）。"""
    if not os.path.exists(os.path.join(C.state["R1"], "oracle", "B1.excalidraw.after.json")):
        raise Unmeasured("B1 が済んでいない（再起動の前に n1 が置かれていない）")
    st = db_state(C)
    page = next(iter(surface_pages(st["d"], lambda s: "excalidraw" in json.dumps(s.get("target") or {})) if st else []), None)
    if not page:
        raise Unmeasured("DB の Project に Excalidraw の面を持つ Page が無い（B3）")
    press(C, "AXCheckBox", page)  # expect は付けない（Excalidraw の面が入口の Page と同じ時は木が変わらず dead_click）。r8 まで入口の Page（Dev）のまま More tools を探していた
    n2 = f"n-demo-b3-{C.nonce}"
    b1_flow(C, n2, [f"n-demo-b1-{C.nonce}", n2], "B3")

def tally_all(C):
    """tally の dict（lies / dead_clicks / unmeasured…）。再起動していれば R1（再起動の前）と R2（今の C.R）の両方を数える。読めなければ ValueError / OSError。"""
    return asdict(T.count_runs(C.state.get("R1", C.R), C.state.get("restarted")))


def h_recount(C, row):
    """#27: R1 と R2 の両方で数え直す。赤 = lies か dead_clicks が 1 件でも在る（full.md 27）・再起動の前に verified だった kind が後に減った。
    未測 = 再起動が済んでいない・後に oracle が無い・前に verified の kind が 1 つも無い（比べる物が無いのを空の緑にしない）。"""
    elements(C)  # R2 の木を 1 回（lies は最後の木の文を見る）
    try:
        t = C.state["recount"] = tally_all(C)
    except (ValueError, OSError) as e:
        raise Unmeasured(f"tally が読めない: {e}"[:160])
    red = [f"{k} {len(t[k])} 件" for k in ("lies", "dead_clicks") if t[k]]
    if not C.state.get("restarted"):
        if red:
            raise RuntimeError("数え直し: " + "・".join(red))
        raise Unmeasured("再起動の手が済んでいない（before / after の oracle を比べられない）")
    before, after = (T.load_run(r)["oracle"] for r in (C.state["R1"], C.state["restarted"]))
    lost, blind = T.lost_kinds(before, after)
    if lost:
        red.append(f"再起動の後に verified が減った kind {lost}")
    if red:
        raise RuntimeError("数え直し: " + "・".join(red))
    if not any(T.verified_kinds(before).values()):
        raise Unmeasured("再起動の前に verified の kind が 1 つも無い（数え直しで比べる物が無い）")
    if blind:
        raise Unmeasured(f"再起動の後の oracle が無く kind {blind} の before / after を比べられない")


# ---- 4 手の driver（W15-b49）: b4（外へ効く）・ask_ai（#18 の lease の拒否 / #20 の承認）・e0（Mirror の 2 秒）・d2（TextEdit を外から終了） ----
GOAL_AI = "Change the README title to “Shibubu Demo” and save it"  # owner 10-27「英語」: 画面は英語・審査は YC なので打つ文も英語
LEASE_WAIT_S = 30  # #18 の Approve の後に lease の拒否の行を待つ秒（製品の人の待ち HUMAN_WAIT_DEFAULT 10 秒・mac_gui.rs ＋ 読み 1 回 1〜7 秒の余裕）


def write_fake(R, repo, te=None, wid=None):
    """偽の CLI（fake-claude.sh）と答え 2 つ（interpret / generate）を R/fake に置く。generate.json は TextEdit の窓番号が分かってから（drive-w19-3b.py の write_fake と同じ形）。"""
    d = os.path.join(R, "fake")
    if llm_env():  # 本物の AI の run: interpret / generate の答えを置かない・fake dir も作らない（run_sh は本物の CLI を渡す）
        return d
    os.makedirs(d, exist_ok=True)
    shutil.copy2(os.path.join(HERE, "fake-claude.sh"), os.path.join(d, "claude"))
    os.chmod(os.path.join(d, "claude"), 0o755)
    json.dump({"operations": [{"id": "op1", "want": "Change the README title to Shibubu Demo and save it", "success": "README.md contains Shibubu Demo", "target": None}]},
              open(os.path.join(d, "interpret.json"), "w"), ensure_ascii=False)
    if wid is None:
        return d
    uri = json.dumps({"uri": "file://" + repo})
    tgt = {"pid": te, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "README.md"}
    json.dump({"none_why": None, "workflow_steps": [
        {"id": "read", "capability": "git.status.read", "input_json": uri},
        {"id": "set", "capability": "desktop.ax.type_text", "input_json": json.dumps({**tgt, "window_id": wid, "label": "", "role": "AXTextArea", "replace": "# shibubu-yc-demo", "text": "# Shibubu Demo"}, ensure_ascii=False)},
        {"id": "write", "capability": "desktop.ax.menu", "input_json": json.dumps({**tgt, "path": ["File", "Save"]}, ensure_ascii=False)}],
        "checks": [
            {"action": "git.status.read", "input_json": uri, "pointer": "/changes", "expect": "changed", "value_json": None},
            {"action": "git.status.read", "input_json": uri, "pointer": "/head/sha", "expect": "equals", "value_json": json.dumps({"$from": "read.head.sha"})}],
        "recipe": None, "trial": None}, open(os.path.join(d, "generate.json"), "w"), ensure_ascii=False)
    return d


def setup_ai(C):
    """start の後: 自分の TextEdit（README.md）の窓番号を引いて偽の CLI の generate.json を置く。引けなければ C.state["ai_target"] = None（ask_ai は未測）。"""
    pid = C.state.get("te_pid")
    C.state["ai_target"] = None
    if not pid:
        return
    for _ in range(10):
        for w in step(C, "list_windows", {"pid": pid}, own=True).get("windows") or []:
            if w.get("pid") == pid and (w.get("title") or "").startswith("README.md"):
                write_fake(C.R, C.repo, pid, w["window_id"])
                C.state["ai_target"] = (pid, w["window_id"])
                return
        time.sleep(2)


def n_acts(C):
    p = os.path.join(C.R, "data", "gui-acts.jsonl")
    return sum(1 for x in open(p, encoding="utf-8") if x.strip()) if os.path.exists(p) else 0


def note_backing(C, kind, at):
    """tally.BACKED_SENTENCES の kind の裏の状態が済んだ時刻（epoch）を run の dir の backing.jsonl に足す。裏の状態を読み戻した driver だけが書く。"""
    with open(os.path.join(C.R, "backing.jsonl"), "a", encoding="utf-8") as f:
        f.write(json.dumps({"kind": kind, "at": at}) + "\n")


def other_files(C):
    """C.repo の README.md 以外の file の sha（.git の外）-> {相対 path: sha}。F1 で README 以外の書類が変わっていないかの比較に使う。"""
    out = {}
    for d, ds, fs in os.walk(C.repo):
        ds[:] = [x for x in ds if x != ".git"]
        for f in fs:
            p = os.path.join(d, f)
            rel = os.path.relpath(p, C.repo)
            if rel != "README.md" and os.path.isfile(p):
                out[rel] = sha_of(p)
    return out


def sha_of(path):
    import hashlib
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def seen(C, needle, role=None, els=None):
    """画面（AX）に needle を含む要素が在れば、その原文。els = 読んだ木（無ければ 1 回読む）。"""
    for e in (elements(C) if els is None else els):
        for k in ("label", "value", "title"):
            if (role is None or e.get("role") == role) and needle in (e.get(k) or ""):
                return e[k]
    return None


def ax_rows(C):
    """B69: 今の木の行の数（(role, label, value, title) ごと）。送る前に取って wait_seen / wait_approval / fresh の base に渡す。"""
    return collections.Counter(ax_key(e) for e in elements(C))


def ax_key(e):
    return (e.get("role"), e.get("label"), e.get("value"), e.get("title"))


def fresh(els, base):
    """B69: base（送る前の ax_rows）より増えた行だけ。同じ形の行は、送る前の数の分を前から外して後ろ（新しい方）を残す。
    cu.umDP の手 9: chat に残った手 7 の lease の拒否の行を手 9 の答えと読み、送った一言の答え（承認の 1 行）を待たずに未測で打ち切った。"""
    if base is None:
        return els
    left, out = collections.Counter(base), []
    for e in els:
        k = ax_key(e)
        if left[k] > 0:
            left[k] -= 1
        else:
            out.append(e)
    return out


def wait_seen(C, needles, secs, role=None, base=None):
    """needles のどれかが出るまで secs 秒。読み 1 回で needles を全部見て、回数でなく時計で切る（r2: 読み 1 回 ≈ 60 秒 × needle ごとの読みで「60 秒」が 45 分になった）。
    base（ax_rows）を渡すと、送る前から在った行は見ない（B69）。"""
    end = time.time() + secs
    while True:
        els = fresh(elements(C), base)
        for n in needles:
            t = seen(C, n, role, els)
            if t:
                return n, t
        if time.time() >= end:
            return None, None
        time.sleep(1)  # B64: 3 → 1（読み 1 回が約 2.7 秒・出てから気付くまで 6 秒遅れていた。上限は時計なので回数が増えるだけ）


def product_stop(C, n0, els):
    """製品が止まった印 -> "<kind>: <why の頭>" か None。`<data>/engine-runs.jsonl` の n0 行より後に error の行が出た / Page に NoMeans の文（G100 の前「Couldn't find a way」・後「No steps came back」）が出た。
    承認の 1 行は止まった製品からは出ない（r18 #20: ≈ 30 秒で NoMeans を出して止まっていたのに 162.9 秒待って未測）。
    engine は承認を頼む時も error（kind denied・pending に承認待ちの手）を書き、AX の button より先に出る（r20 #20: その隙に読んで止まりと誤読・b75）。"""
    p = os.path.join(C.R, "data", "engine-runs.jsonl")
    if os.path.exists(p):
        for x in [x for x in open(p, encoding="utf-8") if x.strip()][n0:]:
            try:
                err = json.loads(x).get("error")
            except ValueError:
                continue
            if err and not (err.get("kind") == "denied" and err.get("pending")):  # denied ＋ pending = 承認を頼む行（止まりでない・r17 / r20）。pending 0 の denied（r19）・failed（r18）は止まり
                return f"{err.get('kind') or 'error'}: {str(err.get('why') or '')[:140]}"
    t = seen(C, "Couldn't find a way", None, els) or seen(C, "No steps came back", None, els)  # G100（FX-101・yc/gui 3bc8b4f）で NoMeans の文が替わる。merge の前後どちらでも当たるように両方
    return f"no_means: {t[:140]}" if t else None


def n_runs(C):
    p = os.path.join(C.R, "data", "engine-runs.jsonl")
    return sum(1 for x in open(p, encoding="utf-8") if x.strip()) if os.path.exists(p) else 0


def wait_approval(C, secs, extra=(), stop=None, base=None):
    """#18 / #20 の送った後の 1 行を secs 秒待つ -> (種類, 原文)。種類 = "approve"（`Approve and send` の button が在る行）/ "denied"（宛先を決められずに拒否した行 `N windows of … can't tell which`・Blocked の `N windows…` も）/ extra の語 / None。
    `Approval needed` の語は読まない（今の製品は拒否の行にも `▣ Needs approval` を付ける・G73。承認かどうかは押せる button で決める）。
    base（ax_rows）を渡すと、送る前から在った行は見ない（B69: 前の手の拒否の行をこの手の答えと読まない）。"""
    end = time.time() + secs
    while True:
        els = fresh(elements(C), base)
        if any(e.get("role") == "AXButton" and not e.get("md") and L.hit(e, "approve") for e in els):
            return "approve", None
        t = next((t for e in els for t in (e.get("label"), e.get("value")) if t and re.search(r"\d+ windows", t)), None)
        if t:
            return "denied", t
        for n in extra:
            t = seen(C, n, None, els)
            if t:
                return n, t
        why = stop(els) if stop else None
        if why:  # 製品が止まった = 待たない（承認の 1 行はもう出ない）
            return "product_stopped", why
        if time.time() >= end:
            return None, None
        time.sleep(1)  # B64: 3 → 1（wait_seen と同じ）


def front_name():
    """前面の app の名前（front_app()・読めなければ空）。読むだけ。B65: `lsappinfo front` の「ASN:0x0-48048:」を `info` は読めず毎回空だった。"""
    return (front_app() or {}).get("name") or ""


def new_acts(C, n0):
    """gui-acts.jsonl の n0 行より後の行（原文）。"""
    p = os.path.join(C.R, "data", "gui-acts.jsonl")
    return [x for x in open(p, encoding="utf-8") if x.strip()][n0:] if os.path.exists(p) else []


def foreign_titled(C, title):
    """自分が起こした以外の TextEdit（owner の）で、題名が title で始まる窓の数（bin/windows で数えるだけ・題名は記録しない）。読めなければ 0。"""
    own = {p for t in C.state.get("te_sets", []) for p in mine(*t)}
    exe = os.path.join(HERE, "bin", "windows")
    pids = [int(x) for x in subprocess.run(["pgrep", "-x", "TextEdit"], capture_output=True, text=True).stdout.split()]
    n = 0
    for p in (p for p in pids if p not in own and os.path.exists(exe)):
        try:
            ws = json.loads(subprocess.run([exe, "once", "--pid", str(p)], capture_output=True, text=True, timeout=20).stdout or "{}").get("windows") or []
        except (subprocess.TimeoutExpired, ValueError):
            continue
        n += sum((w.get("name") or "").startswith(title) for w in ws)
    return n


def h_ask_ai(C, row):
    if not C.state.get("ai_target"):
        raise Unmeasured("自分の TextEdit の README.md の窓が引けない（" + (te_missing(C) if not C.state.get("te_pid") else "TextEdit は開いているが README.md の窓が見つからない") + "）" + ("" if llm_mode() == "real" else "・偽の CLI の generate.json を置けない"))
    readme = os.path.join(C.repo, "README.md")
    lease = "以内" in row["doing"]  # #18 = 人の lease の 5 秒以内（F2）・#20 = 5 秒より後（F1）
    if lease and "TextEdit" not in front_name():  # 人が TextEdit を前に出して打っていない run（#17 は FG）に書き込まない
        raise Unmeasured("前面が TextEdit でない（人の lease が無い・#17 は FG の手＝録画 mode の owner の手が要る）。何も書いていない")
    for lbl in ("close", "cancel"):  # 前の手の 1 行が残っていれば先に閉じる（#18 の拒否の 1 行・送った後の Cancel は今の依頼を取り消す）
        try_press(C, "AXButton", lbl)
    acts0, sha0, runs0 = n_acts(C), sha_of(readme), n_runs(C)
    before_lines, others0 = open(readme, encoding="utf-8").read().splitlines(), other_files(C)
    rows0 = ax_rows(C)  # B69: 送る前の行。答えはこれより増えた行だけで読む（前の手の拒否・承認・結果の行が chat に残る）
    send_field(C, "AXTextField", "change", (items(row, C.nonce) or [GOAL_AI])[0])  # #20 は「同じ一言」（『』が無い）
    t_send = time.time()  # 一言を送った時刻（README の mtime がこれより前なら古い file で、裏の時刻に使わない・W15-b67）
    press(C, "AXButton", "send_edit")
    nothing = L.names("nothing_sent")[0]
    if lease:
        def product_sent():  # B70: 製品の手の印 = act の記録が増えた・頼んだ題が README に在る。sha は人が手 6 で打った字の自動保存でも変わるので見ない（cu.ukZY 手 7）
            return n_acts(C) != acts0 or "Shibubu Demo" in open(readme, encoding="utf-8").read()
        def human_only():  # B70: README は手の始めのままか、人の TextEdit の中身（打った字が自動保存で disk に落ちた形）と同じ
            now = open(readme, encoding="utf-8").read().splitlines()
            return now == before_lines or bool(C.state.get("te_pid")) and now == (te_value(C, C.state["te_pid"], "README.md") or "").splitlines()
        hit, line = wait_approval(C, int(60 * llm_scale()), (nothing,), stop=lambda els: product_stop(C, runs0, els), base=rows0)
        if hit == "product_stopped":  # B74: 製品が承認を頼む前に止まった（cu.GkMp 手 7: AI の手順の書く前の読み git.file.read が InvalidInput →「Nothing was sent, because the original
            # couldn't be read first」・engine-runs は pending 0 の denied）。lease の行の語（so nothing was sent）に当たらず 161.6 秒待って「どちらの行も出ない」の未測になっていた
            if not human_only() or product_sent():
                raise RuntimeError(f"F2: 製品が止まったのに act の記録が {n_acts(C) - acts0} 件増え / README が製品の手で変わった: {line}")
            raise RuntimeError(f"product_stopped({line})。lease の前に製品が止まった（何も送っていない・人の lease の拒否まで届かない）")
        if hit == "approve":  # B68: 製品は承認の後、送る直前に前面を読む（mac_gui.rs の人の lease）。宛先の TextEdit が前面なら最長 10 秒待ち（HUMAN_WAIT_DEFAULT）、
            # lease の 1 行（HUMAN_LEASE_LINE）で拒む。act は書かない（拒否は record_act の前に返る）。承認の 1 行は lease が無い印ではない
            press(C, "AXButton", "approve")
            hit, line = wait_seen(C, (nothing, L.names("done_checked")[0]), LEASE_WAIT_S, base=rows0)
            if hit != nothing:
                if product_sent():
                    raise RuntimeError(f"F2: 人が前面の間に Approve で送った（act の記録 {n_acts(C) - acts0} 件・README に Shibubu Demo が{'在る' if 'Shibubu Demo' in open(readme, encoding='utf-8').read() else '無い'}）: {(line or '')[:120]}")
                raise Unmeasured(f"Approve の後 {LEASE_WAIT_S} 秒で lease の拒否の行（{nothing}）が出ない")
        if hit != nothing:
            if hit == "denied":
                raise RuntimeError("F2: 拒否の行が「N windows…」（Blocked）で、lease の拒否（" + nothing + "）でない")
            raise Unmeasured("拒否の行も承認の行も出ない")
        if product_sent() or not human_only():
            raise RuntimeError(f"F2: 拒否の行が出たのに act の記録が {n_acts(C) - acts0} 件増え / README が製品の手で変わった（頼んだ題が在る・人の TextEdit の中身でも手の始めでもない）")
        return
    hit, line = wait_approval(C, int(60 * llm_scale()), (nothing,), stop=lambda els: product_stop(C, runs0, els), base=rows0)
    if hit == "product_stopped":  # 製品が止まった（engine-runs の error / NoMeans）= 製品の赤。承認の 1 行を待ち続けない
        raise RuntimeError("product_stopped(" + line + ")")
    if hit == "denied":  # 宛先を決められずに拒否した行 = 製品の赤（未測にしない・直すのは製品 = G73）
        raise RuntimeError("宛先を決められずに拒否（製品・G73）: " + (line or "")[:160])
    if hit != "approve":
        raise Unmeasured("承認の 1 行が出ない" if not hit else "承認でなく拒否の行（人の lease が残っている）")
    if not seen(C, "Shibubu Demo", "AXStaticText", fresh(elements(C), rows0)):
        raise RuntimeError("承認の 1 行に送る内容（Shibubu Demo）が無い")
    press(C, "AXButton", "approve")
    done = L.names("done_checked")[0]
    hit, line = wait_seen(C, (done, nothing, "Needs your consent", "Failed", "can't tell which"), int(240 * llm_scale()), base=rows0)
    if hit == "can't tell which" and (m := re.search(r"(\d+) windows", line or "")) and (n := foreign_titled(C, "README.md")) and int(m[1]) - n == 1:  # 自分の README.md は 1 枚・残りが全部よその窓の時だけ環境
        raise Unmeasured(f"環境: 自分のでない TextEdit に README.md の窓が {n} 枚 → 製品は題名で宛先を決め直す（gui::retarget）ので決められず止めた（正しい拒否）。owner の TextEdit の窓を閉じてから測る")
    if hit != done:
        raise RuntimeError(f"F1: 結果の行が「{done}」でない: {hit}: {(line or '')[:120]}")
    t_changed = None
    for _ in range(24):  # 保存は数秒〜30 秒後に disk へ・120 秒まで（W19-3b run 1）
        if sha_of(readme) != sha0:
            t_changed = time.time()
            break
        time.sleep(5)
    new = new_acts(C, acts0)
    i = next((k for k, x in enumerate(new) if "set_value" in x), None)
    saved = lambda x: "menu" in x or re.search(r'"keys"\s*:\s*"cmd\+s"', x)  # 保存 = File › Save の menu か pid 宛ての cmd+s（本物の model は key で保存を組む・r20・b75）
    if i is None or not any(saved(x) for x in new[i + 1:]):
        raise RuntimeError("F1: act の記録に set_value → 保存（menu か cmd+s）の順が無い（AI が fs で直接書いた疑い）: " + " | ".join(x.strip()[:60] for x in new))
    if sha_of(readme) == sha0:
        raise RuntimeError("F1: README の sha が 120 秒たっても変わらない")
    # F1 の合格 = 頼んだ見出しの行だけが変わる: 消えた行は 1 行以下・増えた行はちょうど 1 行で Shibubu Demo を含む・他の行は順序ごと一致・README 以外の書類は不変
    sm = difflib.SequenceMatcher(None, before_lines, open(readme, encoding="utf-8").read().splitlines(), autojunk=False)
    gone, added = [], []
    for tag, a0, a1, b0, b1 in sm.get_opcodes():
        if tag != "equal":
            gone += before_lines[a0:a1]
            added += sm.b[b0:b1]
    if len(gone) > 1 or len(added) != 1 or "Shibubu Demo" not in added[0]:
        raise RuntimeError(f"F1: README の行の差が見出しの 1 行だけでない（消えた行 {len(gone)}・増えた行 {len(added)}・増えた行に Shibubu Demo が{'在る' if any('Shibubu Demo' in x for x in added) else '無い'}）")
    if (moved := sorted(k for k in set(others0) | set(other_files(C)) if others0.get(k) != other_files(C).get(k))):
        raise RuntimeError(f"F1: README 以外の書類が変わった（{len(moved)} 件: {', '.join(moved[:3])}）")
    # Engine の「✓ Verified」の裏 = README の sha が変わり中身に頼んだ題が在る（tally の engine_verified_readme・W15-b66）
    at = t_changed  # 裏の時刻 = README が disk で変わった時刻。driver の poll が気付いた時刻（t_changed）は製品の「✓ Verified」より遅れうる（r17: 5.2 秒）ので、送った後に書かれた mtime が早ければそちら
    mtime = os.path.getmtime(readme)
    if mtime >= t_send:
        at = min(at, mtime)
    note_backing(C, "engine_verified_readme", at)


def h_b4(C, row):
    if os.environ.get("GFINAL_REALLY") != "1":
        raise Unmeasured("外へ効く手なので打たない（GFINAL_REALLY=1 の時だけ書く・drive-b4.py の門）")
    issue = os.environ.setdefault("SHIBUBU_QA_GH_ISSUE", "1")  # 宛先 = private repo YC_DEMO_REPO（既定 shibu003/shibubu-yc-demo）の Issue #1・oracle/github.sh も同じ番号を読む
    import importlib.util
    spec = importlib.util.spec_from_file_location("drive_b4", os.path.join(HERE, "drive-b4.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    depth, C.depth = C.depth, 30
    try:
        m.comment(sys.modules[__name__], C, issue, f"b4x{int(time.time())}")
    finally:
        C.depth = depth
    m.check_oracle(C.R)


def swift_tool(C, name):
    """oracle/<name>.swift を run の dir の tools/ に build（cargo に頼らない）-> 実行 file の path。
    r5: run の dir の直下に build していて、ax.swift の出力先が AX の木を溜める ax/ の dir と同じ名前 = 「在る」と見て dir を実行し Errno 13。"""
    out = os.path.join(C.R, "tools", name)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    if not os.path.isfile(out):
        p = subprocess.run(["xcrun", "swiftc", "-O", os.path.join(HERE, "oracle", name + ".swift"), "-o", out], capture_output=True, text=True)
        if p.returncode != 0:
            raise Unmeasured(f"{name}.swift を build できない: {(p.stderr or '')[-120:]}")
    return out


def h_e0(C, row):
    pid = C.state.get("notes_pid")
    if not pid:
        raise Unmeasured("notes.md の TextEdit を起こしていない（#21 の手が足す）")
    if not C.state.get("notes_sid"):  # r8: #22 が赤のまま矩形を探して 330 秒の後に未測だった
        raise Unmeasured("#22 が notes.md の Mirror を映せていない（notes の面が決まっていない）")
    if seen(C, L.names("mirror_denied")[0], "AXStaticText"):
        raise Unmeasured("画面収録の許可が .app に無く Mirror が映らない: " + seen(C, L.names("mirror_denied")[0], "AXStaticText")[:100])
    ax = swift_tool(C, "ax")
    j = json.load(open(C.R + "/run.json"))
    wid = j["window_id"]
    d = json.loads(subprocess.run([ax, "tree", str(j["pid"]), str(wid)], capture_output=True, text=True).stdout or "{}")
    rect = next((f"{int(e['frame']['x'] - d['window']['x'])},{int(e['frame']['y'] - d['window']['y'])},{int(e['frame']['w'])},{int(e['frame']['h'])}"
                 for e in d.get("elements", []) if (e.get("identifier") or "").startswith("surface-frame:" + C.state["notes_sid"])), None)  # #22 が決めた notes の面
    if not rect:
        raise Unmeasured("Mirror の矩形が AX に無い（映っていない）")
    x, y, w, h = rect.split(",")
    find(C, "AXImage", "", tries=10, contains=True)  # Mirror の映像（AXImage）が出るまで待つ
    odir = os.path.join(C.R, "e0")
    os.makedirs(odir, exist_ok=True)

    def put(text):
        p = subprocess.run([ax, "set", str(pid), "notes.md", text], capture_output=True, text=True)
        if p.returncode != 0:
            raise Unmeasured("TextEdit の notes.md に AX で書けない: " + (p.stderr or "")[-100:])
        return time.time()

    def shoot(label, count, every):
        """Shibubu の窓を every 秒おきに count 枚撮り（撮った時刻つき）、写真を OCR して [(撮った時刻, 読んだ行)]。"""
        shots = []
        for i in range(count):
            f = os.path.join(odir, f"{label}-{i}.png")
            t = time.time()
            if subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), f], capture_output=True).returncode != 0:
                raise Unmeasured("screencapture が撮れない（画面収録の許可）")
            shots.append((t, f))
            time.sleep(every)
        out = []
        for t, f in shots:
            p = subprocess.run([ax, "ocr", f, str(wid), x, y, w, h], capture_output=True, text=True)
            if p.returncode != 0:
                raise Unmeasured("OCR が読めない: " + (p.stderr or "")[-100:])
            out.append((t, " ".join(json.loads(p.stdout))))
        return out

    fails = []
    for k in range(3):  # 混んだ機械の 1 回の遅れで赤にしない: 3 回まで測り直す（毎回新しい nonce）
        # Vision が 0 と O・1 と l を読み違えるので、紛れない小文字だけで作る（tests/mirror_e0.rs:93 と同じ表）
        tag = "".join(chr(b"abcdefhkmnprtwxy"[int(c, 16)]) for c in "%x" % int(time.time() * 1000))
        n1, n2 = "first" + chr(97 + k) + tag, "second" + chr(97 + k) + tag
        put(n1)
        first = shoot(f"a{k}", 12, 0.5)
        if not any(n1 in txt for _, txt in first):
            # 矩形に何も読めない = 映っていない（画面収録の許可が無い）・他の字は読めるのに nonce1 だけ無い = Mirror が写さない
            if not any(txt.strip() for _, txt in first):
                raise Unmeasured("Mirror の矩形が空（画面収録の許可が .app / 撮る側に無い疑い）")
            fails.append("nonce1 が写らない: " + first[-1][1][:60])
            continue
        t1 = put(n2)
        second = shoot(f"b{k}", 15, 0.2)
        got = [t - t1 for t, txt in second if n2 in txt]
        C.state["e0"] = dict(try_n=k + 1, n2_after_s=round(got[0], 2) if got else None)
        if got and got[0] <= 2.0:
            return
        fails.append(f"nonce2 が 2 秒以内に写らない（{C.state['e0']}）")
    raise RuntimeError("E0: 3 回とも外れた: " + " / ".join(fails))


def h_d2(C, row):
    te_all = [t for t in C.state["te_sets"] if mine(*t)]
    pids = [p for t in te_all for p in mine(*t)]
    if not pids:
        raise Unmeasured("自分の TextEdit の pid が無い（owner の TextEdit には触らない）")
    for t in te_all:
        kill_mine(*t)
    for _ in range(25):
        if not any(subprocess.run(["ps", "-p", str(p)], capture_output=True).returncode == 0 for p in pids):
            break
        time.sleep(0.2)
    exe = os.path.join(HERE, "bin", "windows")
    if not os.path.exists(exe):
        raise Unmeasured("bin/windows が無い（build.sh）")
    for p in pids:
        ws = json.loads(subprocess.run([exe, "once", "--pid", str(p)], capture_output=True, text=True).stdout or "{}").get("windows") or []
        if ws:
            raise RuntimeError(f"D2: 終了した TextEdit {p} の窓が残っている（{len(ws)} 枚）")
    # Mirror の面を持つ Page を全部見る（DB から・手 14 の後 README は Diagram・#21 の notes.md は notes の Page）。r8 まで AXButton の Docs / Dev を探していた（tab は AXCheckBox）。
    st = db_state(C)
    tabs = surface_pages(st["d"], lambda s: s.get("backend") == "mirror") if st else []
    gone, live, still, miss = L.names("mirror_gone"), L.names("mirror_live"), [], []
    for tab in tabs or [None]:  # DB が読めなければ今の Page だけ
        if tab:
            press(C, "AXCheckBox", tab)  # expect は付けない: 今の Page の tab を押す時は木が変わらず dead_click（#27 の赤）になる
        # 矩形の AX の名前 `Not available · …`（B23 の前の .app は `Not running · …`・portal_frame.rs:41・labels.py の mirror_gone）を 30 秒かつ 3 回は読む
        # （W15-b60: 混雑で読み 1 回が 10〜20 秒だと 30 秒で 1〜2 回しか読めず、製品が窓の消えを知る前の木で決めてしまう）
        hit, n, end = False, 0, time.time() + 30
        while not hit and (time.time() < end or n < 3):
            if n:
                time.sleep(3)
            els, n = elements(C), n + 1
            hit = any(seen(C, g, "AXImage", els) for g in gone)
        if not hit:
            (still if any(seen(C, x, "AXImage", els) for x in live) else miss).append(tab or "今の Page")
    if any(L.hit(e, "connected", contains=True) for e in elements(C)):
        raise RuntimeError("D2: TextEdit が居ないのに画面に Connected が出ている")
    if still:
        raise RuntimeError(f"D2: TextEdit が居ないのに {still} の Mirror の矩形が 30 秒・3 回の読みで「Mirroring」のまま")
    if miss:
        raise Unmeasured(f"窓は消えたが {miss} の AX の木に「Not available」（前の .app は「Not running」）が出ない")


# ---- scene 2（W15-M3・docs/demo/scene2.md）: 1 つの一言の Project に本物の app 4 つ。送らない（Discord の入力欄・meeting・Claude への prompt は触らない） ----
SCENE2 = os.path.join(REPO, "docs/demo/scene2.md")
APPS2 = {"REAPER": ["--args", "-new"], "Discord": [], "zoom.us": [], "Claude": []}  # open -g -j -a <名前> の後ろの引数。REAPER は保存しない新しい project
SETUP2 = {"mirror_apps": "+ REAPER(-new)・Discord・zoom.us・Claude を open -g -j で先に起こし（Discord は自己更新の pid が落ち着くまで待つ）・終わりに自分の pid だけ終了"}
CHANNEL = "general"  # 台本の『Open the general channel』の語。Discord の題名そのものは記録に残さず、含むかの真偽だけ残す


def pids_of(name):
    p = subprocess.run(["pgrep", "-x", name], capture_output=True, text=True)
    return {int(x) for x in p.stdout.split()}


def solo_discord():
    """Discord は member が owner 1 人だけの server の channel だけ開いてよい（owner 10-26）。G の読むだけの AX で確かめた後に operator が SOLO_DISCORD=1 を付ける。無ければ Discord は起こさず押さない（起こすと online に見える）。"""
    return os.environ.get("SOLO_DISCORD") == "1"


def open_apps(C, settle_s=60):
    """APPS2 を後ろで起こす（-g = 前面を取らない・-j = 隠す）。Discord は自己更新で pid が変わるので、全部の pid が 10 秒変わらなくなるまで待つ（上限 settle_s）。
    C.state["app_sets"][名前] = (開く前の pid 集合, 見た pid 全部)。終わりの kill_mine は (開く前, 見た全部) の差 = 自分が起こした物だけ。"""
    sets = C.state.setdefault("app_sets", {})
    for name, args in APPS2.items():
        before = pids_of(name)
        if not (name == "Discord" and not solo_discord()):
            subprocess.run(["open", "-g", "-j", "-a", name, *args], capture_output=True)
        sets[name] = (before, set(before))
    t_end, calm, prev = time.time() + settle_s, time.time(), None
    while time.time() < t_end:
        cur = {n: pids_of(n) for n in APPS2}
        for n, c in cur.items():
            sets[n][1].update(c)
        if cur != prev:
            calm, prev = time.time(), cur
        elif time.time() - calm >= 10:
            break
        time.sleep(2)


def app_pid(C, name):
    """今動いている name の pid と、自分が起こした物か -> (pid or None, owned)。"""
    live = pids_of(name)
    mine_ = sorted(live - C.state.get("app_sets", {}).get(name, (set(), set()))[0])
    return (mine_[-1], True) if mine_ else ((max(live), False) if live else (None, False))


def app_windows(C, pid):
    return [w for w in step(C, "list_windows", {"pid": pid}, own=True).get("windows") or [] if w.get("pid") == pid]


def app_els(C, pid, depth=30):
    """別の app（pid）の全部の窓の AX の要素（読むだけ）。"""
    out = []
    for w in app_windows(C, pid):
        out += step(C, "get_window_state", {"pid": pid, "window_id": w["window_id"], "timeout_ms": 10000, "max_depth": depth, "include_screenshot": False}, own=True).get("elements") or []
    return out


def track_count(els):
    """REAPER の track の数 = AX の `Track number: N` の button の数（製品の「済み」と独立に数える）。"""
    return sum(1 for e in els if e.get("role") == "AXButton" and any((e.get(k) or "").startswith("Track number:") for k in ("label", "title", "value")))


def titles(C, pid):
    return [w.get("title") or "" for w in app_windows(C, pid)]


def is_frame(e, app):
    """Mirror の矩形（surface-frame:*）が app の名前と `Mirroring` を label に持つ = 映っている。"""
    return (e.get("identifier") or "").startswith("surface-frame:") and any("Mirroring" in (e.get(k) or "") and app in (e.get(k) or "") for k in ("label", "value", "title"))


def h_mirror_apps(C, row):
    """『Page:App』を順に: その Page へ移り、同意の「Mirror here」が在れば押し、矩形が `Mirroring` で出るまで待つ。1 つでも出なければ未測（許可・app が居ない等の理由を note に）。"""
    miss = []
    for pair in items(row, C.nonce):
        tab, app = pair.split(":", 1)
        press(C, "AXCheckBox", tab, expect="ax:*")  # 固定名のまま: scene 2 の Page 名は台本の一言に入れて model に付けさせる Page（27 手の Docs / Dev / Diagram とは別・W15-b71 の監査）
        time.sleep(2)
        try_press(C, "AXButton", "mirror")
        for _ in range(10):
            if any(is_frame(e, app) for e in elements(C)):
                break
            time.sleep(3)
        else:
            miss.append(app)
    C.state["note"] = f"live {len(items(row, C.nonce)) - len(miss)}/{len(items(row, C.nonce))}"
    if miss:
        denied = seen(C, L.names("mirror_denied")[0], "AXStaticText")
        raise Unmeasured(f"Mirror が映らない Page: {', '.join(miss)}" + ("（画面収録の許可が .app に無い）" if denied else ""))


def write_fake2(R, want, success, cap, inp):
    """偽の CLI の答え（interpret と generate）を R/fake に置き直す。Page ごとに依頼が違うので ask の前に毎回。checks は空（読み戻しは rehearse が製品の外で数える）。"""
    d = os.path.join(R, "fake")
    if llm_env():
        return
    json.dump({"operations": [{"id": "op1", "want": want, "success": success, "target": None}]}, open(os.path.join(d, "interpret.json"), "w"), ensure_ascii=False)
    json.dump({"none_why": None, "workflow_steps": [{"id": "do", "capability": cap, "input_json": json.dumps(inp, ensure_ascii=False)}], "checks": [], "recipe": None, "trial": None}, open(os.path.join(d, "generate.json"), "w"), ensure_ascii=False)


def ask2(C, tab, said, fake, press_key):
    """tab の Page で said を打って送り、承認の 1 行を読んで承認する -> 承認の 1 行の原文。承認でなく拒否の行 / 行が出ないは未測。
    押す先の 1 行（press_key）は G の M2 が入れる語: labels.py の status が pending の間は照合せず、en が入ったら無ければ赤。"""
    write_fake2(C.R, *fake)
    press(C, "AXCheckBox", tab, expect="ax:*")
    time.sleep(2)
    for lbl in ("close", "cancel"):  # 前の依頼の 1 行が残っていれば先に閉じる
        try_press(C, "AXButton", lbl)
    send_field(C, "AXTextField", "change", said)
    press(C, "AXButton", "send_edit")
    nothing = L.names("nothing_sent")[0]
    hit, line = wait_seen(C, ("Approval needed", nothing), 60)
    if hit != "Approval needed":
        raise Unmeasured("承認の 1 行が出ない" if not hit else "承認でなく拒否の行（前面が Shibubu でない・人が操作中の疑い）")
    want = [] if L.LABELS[press_key][4].startswith("pending") else L.names(press_key)
    if want and not any(seen(C, w) for w in want):
        raise RuntimeError(f"承認の 1 行に押す先（{want[0]}）が無い")
    press(C, "AXButton", "approve")
    return line


def ax_windows(pid):
    """pid の AXWindows を 1 枚ずつ {title, subrole, movable, main, minimized, x, y, w, h}（bin/axprobe --windows・読むだけ）。cua-driver の list_windows にはこの役割の属性が無い。読めなければ []。"""
    try:
        raw = subprocess.run([os.environ.get("AXPROBE", HERE + "/bin/axprobe"), "--windows", "--pid", str(pid)], capture_output=True, text=True, timeout=30).stdout
        return json.loads(raw)
    except Exception:
        return []


def pick_main(wins):
    """製品の pick_by_role（src/adapters/mac_gui.rs・yc-gui ab9f5af）と同じ役割の規則で主窓 1 枚 -> 無ければ None。最小化を除く -> そこで 1 枚ならそれ -> AXStandardWindow かつ movable で 1 枚ならそれ
    -> （それが 0 枚なら最小化を除いた全部から）AXMain が 1 枚ならそれ。題名は見ない。属性が取れない（null）窓は「そうでない」側に数える。"""
    open_ = [w for w in wins if w.get("minimized") is not True]
    work = [w for w in open_ if w.get("subrole") == "AXStandardWindow" and w.get("movable") is True]
    main = [w for w in (work or open_) if w.get("main") is True]
    return next((g[0] for g in (open_, work, main) if len(g) == 1), None)


def close_about(C, pid):
    """自分が起こした REAPER の主窓（pick_main）でない窓（評価版の About）を閉じる -> 閉じ切れず残った枚数。主窓が決まらなければ何も閉じず Unmeasured。close ボタン（AXCloseButton / Close / OK）を押し、無ければ Esc。"""
    for _ in range(3):
        wins = ax_windows(pid)
        main = pick_main(wins)
        if main is None:
            raise Unmeasured("REAPER の主窓を役割（最小化・標準の窓かつ動かせる・main）で 1 枚に決められない（何も閉じない）")
        extra = [w for w in wins if w is not main and w.get("minimized") is not True]
        if not extra:
            return 0
        cg = app_windows(C, pid)
        for w in extra:
            at = [c for c in cg if (c.get("title") or "") == (w.get("title") or "") and all(round((c.get("bounds") or {}).get(a, -1)) == round(w.get(b) or -2) for a, b in (("x", "x"), ("y", "y"), ("width", "w"), ("height", "h")))]
            if len(at) != 1:
                continue
            els = step(C, "get_window_state", {"pid": pid, "window_id": at[0]["window_id"], "timeout_ms": 10000, "max_depth": 6, "include_screenshot": False}, own=True).get("elements") or []
            btn = next((e for e in els if e.get("role") == "AXButton" and (e.get("subrole") == "AXCloseButton" or (e.get("label") or e.get("title")) in ("Close", "OK"))), None)
            if btn:
                step(C, "click", {"element_token": btn["element_token"]})
            elif els:
                step(C, "press_key", {"key": "escape", "element_token": els[0]["element_token"]})
        time.sleep(2)
    wins = ax_windows(pid)
    main = pick_main(wins)
    return len([w for w in wins if w is not main and w.get("minimized") is not True])


def h_app_track(C, row):
    """REAPER の Page で "Add a new track" -> 承認 -> REAPER の track の数が**ちょうど 1 増えた**時だけ ok（製品の「済み」を信じず AX で数える）。
    増えたのに行が済みでなくても ok・済みと言ったのに 1 増えていない / 2 以上増えた = 赤（G-b の嘘の済み）・行が済みでなく増えてもいない = 未測。owner の REAPER が前から動いていれば触らない。"""
    tab, said = items(row, C.nonce)[:2]
    pid, owned = app_pid(C, "REAPER")
    if not pid:
        raise Unmeasured("REAPER が起きていない")
    if not owned:
        raise Unmeasured("REAPER が前から動いている（owner の project を触らない）")
    if close_about(C, pid):
        raise Unmeasured("REAPER の評価版の About が閉じられない（主窓の menu が効かない）")
    els = app_els(C, pid)
    if not els:
        raise Unmeasured("REAPER の AX の木が空（応答しない）")
    n0 = track_count(els)
    ask2(C, tab, said, ("Add a new track", "REAPER に track が 1 本増える", "desktop.ax.menu",
                        {"pid": pid, "app_name": "REAPER", "bundle_id": "com.cockos.reaper", "path": ["Track", "Insert new track"]}), "press_track")
    done = L.names("done_checked")[0]
    hit, _ = wait_seen(C, (done, L.names("nothing_sent")[0], "Needs your consent", "Failed"), 120)
    n1 = n0
    for _ in range(6):
        n1 = track_count(app_els(C, pid))
        if n1 != n0:
            break
        time.sleep(3)
    C.state["note"] = f"track {n0}→{n1}"
    if n1 == n0 + 1:
        return
    if hit != done and n1 == n0:
        raise Unmeasured(f"結果の行が済みでなく（{hit}）track も増えていない（{n0}→{n1}）")
    raise RuntimeError(f"REAPER の track が {n0}→{n1}（1 増えていない）・製品の行: {hit}")


def h_app_channel(C, row):
    """Discord の Page で "Open the general channel" -> 承認 -> Discord の窓の題名に general を含む。押すだけ・送らない。
    題名が空（画面収録の許可が無い）・前から general が開いている（前後で比べられない）は未測。題名は記録に残さず、枚数と含むかの真偽だけ。"""
    if not solo_discord():
        raise Unmeasured("owner の自分だけの server でだけ開く（10-26・SOLO_DISCORD=1 が無い）")
    tab, said = items(row, C.nonce)[:2]
    pid, _ = app_pid(C, "Discord")
    if not pid:
        raise Unmeasured("Discord が起きていない")
    t0 = titles(C, pid)
    if not any(t0):
        raise Unmeasured("Discord の窓の題名が空（画面収録の許可が無い）")
    if any(CHANNEL in t.lower() for t in t0):
        raise Unmeasured("前から general が開いている（前後で比べられない）")
    ask2(C, tab, said, ("Open the general channel", "Discord で general の channel が開く", "desktop.ax.click",
                        {"pid": pid, "app_name": "Discord", "bundle_id": "com.hnc.Discord", "role": "AXButton", "label": CHANNEL, "match": "contains"}), "press_channel")  # click の field は G の M2（G-c）が決める形に、その commit の後で合わせる
    done = L.names("done_checked")[0]
    hit, _ = wait_seen(C, (done, L.names("nothing_sent")[0], "Needs your consent", "Failed"), 120)
    got = False
    for _ in range(8):
        pid2, _ = app_pid(C, "Discord")
        ts = titles(C, pid2) if pid2 else []
        got = any(CHANNEL in t.lower() for t in ts)
        if got:
            break
        time.sleep(3)
    C.state["note"] = f"窓 {len(ts)} 枚・題名に {CHANNEL} を含む: {got}"
    if got:
        return
    if hit != done:
        raise Unmeasured(f"結果の行が済みでなく（{hit}）題名にも出ない")
    raise RuntimeError(f"Discord の題名に {CHANNEL} が出ない・製品の行は済み")


HANDLERS = {k[2:]: v for k, v in globals().items() if k.startswith("h_")}
# 手の番号（full.md の番号 = 台本の「元の番号」）-> 前提の手。前提の手が ok でない時、その手は打たず blocked(#N)（赤にも ok にも数えない・理由に前提の状態）。前提が ok なのに失敗した手は今までどおり赤
REQUIRES = {10: 9, 22: 5, 23: 22, 26: 25, 27: 25}


def prereq(res, r):
    """手 r の前提の手の結果（無ければ None）。番号は台本ごとに違う（short.md は 9 手）ので、full.md の番号 = r["orig"] で REQUIRES を引く。"""
    return next((x for x in res if x.get("orig", x["n"]) == REQUIRES.get(r.get("orig", r["n"]))), None)  # orig の無い行（試験の合成の行）は full.md の番号 n


def textedit_pids():
    p = subprocess.run(["pgrep", "-x", "TextEdit"], capture_output=True, text=True)
    return {int(x) for x in p.stdout.split()}


def mine(before, after, *_):
    """自分が起こした pid = 開く前に居なかった物だけ（owner の TextEdit は含めない）。3 つ目（窓が出なかった理由）は数えない。"""
    return sorted(set(after) - set(before))


def kill_mine(before, after, *_):
    for pid in mine(before, after):
        try:
            os.kill(pid, 15)
        except ProcessLookupError:
            pass


def open_notes(C):
    """Mirror の相手 = repo の docs/notes.md を書いて TextEdit で後ろに開く（自分の pid だけ後で終了・e0 が nonce を入れる）。
    git status には出さない（.git/info/exclude）: 起動の前から在ると #4 の git status と #20 の changes の check が変わる。"""
    path = os.path.join(C.repo, "docs", "notes.md")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w", encoding="utf-8").write("notes\n")
    os.makedirs(os.path.join(C.repo, ".git", "info"), exist_ok=True)
    open(os.path.join(C.repo, ".git", "info", "exclude"), "a", encoding="utf-8").write("docs/notes.md\n")
    place_te(C, open_textedit(path), "notes_pid")


def front_app():
    """前面の app（bin/front-guard snap の front・lsappinfo は画面が眠っている間 空を返す）-> {bundle_id, name, pid} か None。"""
    with tempfile.NamedTemporaryFile(suffix=".json") as f:
        if subprocess.run([os.path.join(HERE, "bin", "front-guard"), "snap", "--out", f.name], capture_output=True).returncode != 0:
            return None
        try:
            return json.load(open(f.name)).get("front")
        except ValueError:
            return None


def restore_front(front0):
    """新しい TextEdit の instance は -g でも前に出る（demo-loop r2〜r4: launch の前面 = 自分の TextEdit）→ 開く前の前面（owner の app）に返す。
    返さないと owner の前面が試験の TextEdit のまま測られ、d2（#26）で自分の TextEdit を終えると前面が動いて violation になる。前が TextEdit・読めない時は何もしない。"""
    b = (front0 or {}).get("bundle_id")
    if not b or b == "com.apple.TextEdit" or ((front_app() or {}).get("pid") == front0.get("pid")):
        return
    subprocess.run(["open", "-b", b], capture_output=True)
    time.sleep(1)


def te_window_miss(name):
    """file の窓が 2 回とも出なかった時の未測の文（te_missing が te_why 経由で返す）。"""
    return f"TextEdit が {name} の窓を出さなかった（Open の窓のまま・2 回）"


def te_titled(pid, title):
    """新しい pid の窓の題名が title で始まるまで（20 秒・1 秒ごと）。bin/windows once --pid の windows[].name（foreign_titled と同じ読み）。"""
    exe = os.path.join(HERE, "bin", "windows")
    end = time.time() + 20
    while True:
        ws = []
        if os.path.exists(exe):
            try:
                ws = json.loads(subprocess.run([exe, "once", "--pid", str(pid)], capture_output=True, text=True, timeout=20).stdout or "{}").get("windows") or []
            except (subprocess.TimeoutExpired, ValueError, OSError):
                ws = []
        if any((w.get("name") or "").startswith(title) for w in ws):
            return True
        if time.time() >= end:
            return False
        time.sleep(1)


def open_textedit(path, tries=30):  # 30 秒: 混雑（r24 load1 204）で 10 秒では新しい instance が出なかった
    """path を TextEdit で後ろに開く（-g = 前面を取らない・-n = 新しい instance）。
    新しい pid の窓の題名が file 名で始まるまで待つ。出なければその pid だけ終了して 1 回開き直す。
    戻り = (開く前の pid 集合, 開いた後)。2 回とも窓が出なければ 3 つ目に te_window_miss（呼ぶ側は pid を置かない）。"""
    title = os.path.basename(path)
    before = textedit_pids()
    dead = set()  # 題が出ずに終了した pid。pgrep に残っていても次の「新しい pid」に数えない
    missed = False
    for _attempt in (1, 2):
        # 状態の復元を切る: 新しい instance が前の run の README.md / notes.md の窓まで復元すると、製品の Mirror が同じ題名の窓を
        # 選べず Ambiguous で止まる（demo-loop r1 00:39: TextEdit の README.md の窓 10 個・書類の path の問いが ae_-1712）
        # App Nap を外す（その process の引数の domain だけ・owner の defaults は書かない）: 背景に長く置いた TextEdit は Apple Event に答えず -1712 になる
        # （r19 #5: README の instance の書類 path の問いが 5 秒で時間切れ → Mirror here が出ない。実測 120 秒後: 付けない 5.3 s -1712 / 付ける 0.5 s で答える）
        subprocess.run(["open", "-g", "-n", "-a", "TextEdit", path, "--args", "-ApplePersistenceIgnoreState", "YES", "-NSAppSleepDisabled", "YES"], check=True)
        pid, after = None, set(before)
        for _ in range(tries):
            time.sleep(1)
            after = textedit_pids()
            got = [p for p in mine(before, after) if p not in dead]
            if len(got) > 1:
                return set(before) | dead, after
            if len(got) == 1:
                pid = got[0]
                break
        if pid is None:
            return (set(before) | dead, after, te_window_miss(title)) if missed else (set(before) | dead, after)
        if te_titled(pid, title):
            return set(before) | dead, after
        missed = True
        dead.add(pid)
        kill_mine(before, set(before) | {pid})
    return set(before) | dead, textedit_pids(), te_window_miss(title)


def place_te(C, te, key):
    """開いた TextEdit を te_sets に足す。pid が 1 つで、窓が出なかった理由が無い時だけ key に置く。README は te_why も書く。"""
    C.state["te_sets"].append(te)
    why = te[2] if len(te) > 2 else None
    pids = mine(*te)
    pid = pids[0] if len(pids) == 1 and not why else None
    C.state[key] = pid
    if key == "te_pid":
        C.state["te_why"] = None if pid else (why or f"README の TextEdit を開いたが、自分の pid が 1 つに決まらない（{len(pids)} 個）")


def owner_idle_s(*dirs):
    """run.sh の start の門と同じ読み（owner_idle.py = mouse と、自分の type_text を除いた key）。dirs の直下の run dir（run.* / demo* / *-restart / --out）の steps.jsonl を自分の打鍵として除く。読めなければ None。"""
    steps = sorted(f for d in dirs for f in glob.glob(os.path.join(d, "*", "steps.jsonl")))
    p = subprocess.run(["python3", "-I", os.path.join(HERE, "owner_idle.py"), "--steps", *steps], capture_output=True, text=True)
    try:
        return float(p.stdout.strip())
    except ValueError:
        return None


# ---- FG の手（W15-b53）: owner の手の前後を機械が読む。Enter を待つのは打つ手（#6・#8・#24）だけ。前面に出す手（#17・#19）は stdin を待たず前面を見張る ----
FG_WATCH_S = 60


def front_pid():
    """前面の app の pid（front_app()・1 回 約 0.3 秒）。読めなければ None。B65: `lsappinfo info` は front の「ASN:0x0-48048:」に空を返し、毎回 None だった。"""
    return (front_app() or {}).get("pid")


def watch_front(want, secs=FG_WATCH_S):
    """前面の pid が want になるまで 0.5 秒ごとに見る（stdin は待たない）-> (なったか, 見た pid の順)。"""
    seen_pids, t_end = [], time.time() + secs
    while True:
        p = front_pid()
        if p is not None and (not seen_pids or seen_pids[-1] != p):
            seen_pids.append(p)
        if p == want:
            return True, seen_pids
        if time.time() >= t_end:
            return False, seen_pids
        time.sleep(0.5)


def wait_enter():
    """stdin の Enter を待つ間、前面の pid を 0.5 秒ごとに記録する -> 見た pid の順。Enter を押すと terminal が前に出るので、前面の判定は Enter の後の値を使わない。"""
    seen_pids = []
    while True:
        p = front_pid()
        if p is not None and (not seen_pids or seen_pids[-1] != p):
            seen_pids.append(p)
        if select.select([sys.stdin], [], [], 0.5)[0]:
            sys.stdin.readline()
            return seen_pids


def te_value(C, pid, title):
    """pid の TextEdit の title で始まる窓の AXTextArea の value（読むだけ・step に TextEdit の pid を向ける own=True）。窓が引けなければ None。"""
    w = next((w for w in step(C, "list_windows", {"pid": pid}, own=True).get("windows") or [] if w.get("pid") == pid and (w.get("title") or "").startswith(title)), None)
    if not w:
        return None
    els = step(C, "get_window_state", {"pid": pid, "window_id": w["window_id"], "timeout_ms": 10000, "max_depth": C.depth, "include_screenshot": False}, own=True).get("elements") or []
    return next((e.get("value") or "" for e in els if e.get("role") == "AXTextArea"), None)


def save_lines(C):
    """app.log のうち保存・Automation に触れる行（helper の保存の答えの形は未測なので、語で拾って原文を残す）。"""
    try:
        return [x.strip() for x in open(os.path.join(C.R, "app.log"), encoding="utf-8", errors="replace") if re.search(r"(?i)save|automation", x)]
    except OSError:
        return []


def fg_snap(C, notes):
    """打つ手の前後に読む物: relay の行数・書類の sha・TextEdit の AX value・app.log の保存の行。notes=True なら notes.md（#24）・でなければ README.md（#6・#8）。"""
    pid, title, path = (C.state.get("notes_pid"), "notes.md", os.path.join(C.repo, "docs", "notes.md")) if notes else (C.state.get("te_pid"), "README.md", os.path.join(C.repo, "README.md"))
    rl = os.path.join(C.R, "data", "mirror-relay.jsonl")
    return dict(relay=sum(1 for x in open(rl, encoding="utf-8") if x.strip()) if os.path.exists(rl) else 0,
                sha=sha_of(path) if os.path.exists(path) else None, path=path,
                ax=te_value(C, pid, title) if pid else None, log=save_lines(C))


def fg_typed(C, r, nonce):
    """#6・#8・#24: Enter の前後を読む -> (状態, note)。#6 = A2（AX value に nonce・disk に無い）・#8 = A1→E1・#24 = E1（notes.md）。"""
    notes = r["key"] == "fg_mirror_type"
    b = fg_snap(C, notes)
    print(f"[owner] 手 {r['n']}: {fill(r['doing'], nonce)}（打つ文字に {nonce} を含める） → 終わったら Enter", flush=True)
    fronts = wait_enter()
    saves = "⌘S" in r["doing"]
    if not saves:  # #6: 保存しない手。自動保存（約 19〜29 s）が来る前に読む
        a = fg_snap(C, notes)
    else:  # #8・#24: ⌘S の答えと disk への書き込みを 15 s まで待って読む
        for _ in range(15):
            a = fg_snap(C, notes)
            if a["sha"] != b["sha"]:
                break
            time.sleep(1)
    note = f"front={fronts} relay {b['relay']}→{a['relay']} sha {(b['sha'] or '-')[:8]}→{(a['sha'] or '-')[:8]} ax {len(b['ax'] or '')}→{len(a['ax'] or '')}字 save_log +{len(a['log']) - len(b['log'])}: {' / '.join(a['log'][len(b['log']):][:2])}"
    if a["ax"] is None:
        raise Unmeasured("TextEdit の AX value が読めない（pid か窓が引けない）。" + note)
    if a["relay"] <= b["relay"]:
        raise RuntimeError("A1: relay の記録が増えない（人の入力が Mirror を通っていない）。" + note)
    if not saves:  # #6 A2
        if nonce not in a["ax"]:
            raise RuntimeError("A2: TextEdit の AX value に nonce が無い。" + note)
        if a["sha"] != b["sha"]:
            raise Unmeasured("自動保存が先に disk へ書いた（README の sha が変わった）ので「disk に無い」は判定できない。" + note)
        return "owner", note
    if a["ax"] == (b["ax"] or ""):
        raise RuntimeError("E1: AX value が変わらない。" + note)
    if a["sha"] == b["sha"]:
        raise RuntimeError("E1: ⌘S の後 15 s たっても disk の sha が変わらない。" + note)
    if nonce not in open(a["path"], encoding="utf-8", errors="replace").read():
        raise RuntimeError("E1: disk の書類に nonce が無い。" + note)
    if not notes and nonce not in subprocess.run(["git", "-C", C.repo, "diff", "--no-color"], capture_output=True, text=True).stdout:
        raise RuntimeError("A1: git diff に nonce が無い。" + note)
    return "owner", note


def fg_lease(C, r, nonce):
    """#17: TextEdit が前面になるまで見張り（stdin は待たない）、続けて 1 文字が AX value に出るのを待つ -> (状態, note)。"""
    print(f"[owner] 手 {r['n']}: {fill(r['doing'], nonce)}（Enter は押さない・{FG_WATCH_S} 秒まで見ている）", flush=True)
    pid = C.state.get("te_pid")
    if not pid:
        raise Unmeasured("自分の TextEdit の pid が無い（前面の TextEdit を見分けられない）")
    ax0 = te_value(C, pid, "README.md") or ""
    ok, fronts = watch_front(pid)
    if not ok:
        raise Unmeasured(f"{FG_WATCH_S} 秒たっても前面が TextEdit（pid {pid}）にならない。front={fronts}")
    for _ in range(5):  # 1 文字が打たれるまで（3 秒ごと・15 秒）
        if (te_value(C, pid, "README.md") or "") != ax0:
            return "owner", f"front={fronts} TextEdit {pid} が前面・AX value {len(ax0)}字→変化あり"
        time.sleep(3)
    raise Unmeasured(f"TextEdit は前面になったが AX value が変わらない（1 文字が打たれていない）。front={fronts}")


def fg_front(C, r, nonce):
    """#19: 前面が Shibubu（run.json の pid）になるまで見張る（stdin は待たない＝Enter で terminal が前に出る前に読む）-> (状態, note)。"""
    print(f"[owner] 手 {r['n']}: {fill(r['doing'], nonce)}（Enter は押さない・{FG_WATCH_S} 秒まで見ている）", flush=True)
    me = json.load(open(C.R + "/run.json"))["pid"]
    ok, fronts = watch_front(me)
    if not ok:
        raise Unmeasured(f"{FG_WATCH_S} 秒たっても前面が Shibubu（pid {me}）にならない。front={fronts}")
    behind = C.state.get("te_pid")
    return "owner", f"front={fronts} Shibubu {me} が前面" + (f"・TextEdit {behind} は後ろ" if behind and behind != me else "")


FG_HANDLERS = {"fg_textedit": fg_typed, "fg_mirror_type": fg_typed, "fg_lease": fg_lease, "fg_front": fg_front}


def run_fg(C, r, nonce):
    """録画 mode の FG の手 -> (状態, note)。handler の無い FG の手が出来たら、従来どおり Enter を待つだけ。"""
    h = FG_HANDLERS.get(r["key"])
    if h:
        return h(C, r, nonce)
    print(f"[owner] 手 {r['n']}: {fill(r['doing'], nonce)}  → 終わったら Enter", flush=True)
    sys.stdin.readline()
    return "owner", ""


def live(rows, n, app, out):
    try:
        llm_env()  # 本物の AI の門（REHEARSE_LLM=real は owner の ok が要る・何も起こす前に決める）
    except LlmGate as e:
        print(f"rehearse: {e}", file=sys.stderr)
        return e.rc
    rec = os.environ.get("SHIBUBU_QA_RECORD") == "1"
    first = int(os.environ.get("SHIBUBU_QA_FROM") or 1)
    if not os.path.exists(os.path.join(app, "Contents/Helpers/kangae-mirror-helper")):
        print(f"rehearse: {app} に Contents/Helpers/kangae-mirror-helper が無い（mirror 付きの release を作る・KANGAE_FEATURES=mirror）: 起こさない", file=sys.stderr)
        return 3
    idle = owner_idle_s(*([os.path.dirname(os.path.abspath(out))] if out else []), os.path.expanduser("~/shibubu-yc-runs")) if not rec else None
    if idle is not None and idle < 120:
        print(f"rehearse: owner 使用中で未測（idle {idle:.0f}s < 120s）", file=sys.stderr)
        return 5
    nonce = f"d{n}x{int(time.time())}"
    R = out or subprocess.check_output(["mktemp", "-d", os.path.expanduser("~/shibubu-yc-runs/demo%s.XXXX" % n)], text=True).strip()
    p = subprocess.run(["bash", os.path.join(HERE, "fresh-repo.sh"), "init", R], capture_output=True, text=True)
    if p.returncode != 0:
        print("rehearse: fresh-repo.sh init が失敗", p.stderr[-200:], file=sys.stderr)
        return 3
    C = Ctx(R, app, nonce)
    C.state["te_sets"] = []  # 自分が起こした TextEdit（開く前後の pid 集合）。終わりに自分の pid だけ終了
    front0 = front_app()
    if any(uses_readme_te(r) for r in rows):
        place_te(C, open_textedit(os.path.join(C.repo, "README.md")), "te_pid")
    if any(r["key"] == "nl_edit" and r["who"] == "BG" and "notes.md" in r["doing"] for r in rows):
        open_notes(C)  # 起動の前に: run の途中の open は新しい TextEdit の instance が前に出て D-g の violation（demo-loop r4 #21）
    restore_front(front0)
    try:
        if any(r["key"] in SETUP2 and r["who"] == "BG" for r in rows):  # Shibubu を起こす前に: Discord の自己更新が前面を取るのは start の前に済ませる
            open_apps(C)
        return _live(rows, n, app, rec, first, nonce, C)
    finally:
        for te in C.state["te_sets"]:
            kill_mine(*te)
        for te in C.state.get("app_sets", {}).values():
            kill_mine(*te)


def _live(rows, n, app, rec, first, nonce, C):
    R = C.R
    rc, err = run_sh("start", R, app, ws=C.repo, fake=write_fake(R, C.repo))
    if rc not in (0, 6):
        print("rehearse: start rc", rc, err, file=sys.stderr)
        return 3
    if json.load(open(R + "/run.json")).get("window_id") is None:
        run_sh("stop", R)
        print("rehearse: driver down: run.json の window_id が null（窓が取れていない）", file=sys.stderr)
        return 7
    ai_line = mark_ai(R)
    res, red, down, fg_red = [], False, None, False
    if any(r["key"] == "ask_ai" and r["who"] == "BG" for r in rows):
        try:
            setup_ai(C)
        except DriverDown as e:
            down = str(e)
    t_rec = float(os.environ.get("SHIBUBU_QA_REC_T0") or time.time())
    until = int(os.environ.get("REHEARSE_UNTIL") or 10**6)  # この手番で止める（#5 / #20 だけを直した時に 27 手を回さない・後ろは skip）
    for r in rows:
        t0 = time.time()
        note = ""
        if r["n"] < first or r["n"] > until:
            st = "skip"
        elif r["who"] == "FG":
            st, note = "FG", ""
            if rec:
                try:
                    st, note = run_fg(C, r, nonce)
                except Unmeasured as e:
                    st, note = f"未測: {e}", str(e)
                except DriverDown as e:
                    st, down = f"未測 (driver: {e})", str(e)
                except Exception as e:  # 赤でも owner の録画は続ける（後の手は別の物を測る）
                    st, note, fg_red = f"赤: {e}", str(e), True
        elif NO_DRIVER.get(r["key"]):
            st = "未測: " + NO_DRIVER[r["key"]]
        elif (pre := prereq(res, r)) and pre["status"] not in ("skip",) and not pre["status"].startswith("ok"):
            st = f"blocked(#{pre['n']}): 前提の #{pre['n']} が {pre['status'][:50]}（この手は打たない・赤にも ok にも数えない）"
        else:
            try:
                HANDLERS[r["key"]](C, r)
                st, note = "ok", C.state.pop("note", "")
            except Unmeasured as e:
                st = f"未測: {e}"
            except DriverDown as e:
                st, down = f"未測 (driver: {e})", str(e)
            except Exception as e:  # D-g の violation・B1 の外れなど
                st, red = f"赤: {e}", True
        res.append(dict(n=r["n"], orig=r.get("orig", r["n"]), who=r["who"], key=r["key"], status=st, sec=round(time.time() - t0, 1), target=r["sec"], t_start=round(t0 - t_rec, 1), t_end=round(time.time() - t_rec, 1), note=note))
        # REHEARSE_KEEP_GOING=1: 手の赤の後も次の手を測る（r7 まで赤 1 つで止まり、24〜27 を 1 度も測れなかった）。D-g の violation（owner が居る）と driver down（app が居ない）は止まる
        if down or (red and (os.environ.get("REHEARSE_KEEP_GOING") != "1" or st.startswith("赤: D-g"))):
            break
    rc_stop, err_stop = run_sh("stop", C.R)
    lj = write_llm_json(R)  # <run>/llm.json（real / fake・回数・秒）
    try:
        tally = {k: v for k, v in tally_all(C).items() if k in ("lies", "dead_clicks", "claimed_connected", "unmeasured")}
    except (ValueError, OSError) as e:
        tally = {"error": str(e)[-200:]}
    dg = 0
    try:
        dg = len(json.load(open(C.R + "/steps/zz-run.verdict.json")).get("violations") or [])
    except Exception:
        dg = None
    if os.environ.get("SHIBUBU_QA_CHAPTERS"):
        with open(os.environ["SHIBUBU_QA_CHAPTERS"], "w", encoding="utf-8") as f:
            f.write("n\twho\tkey\tstart_s\tend_s\tstatus\tnote\n" + "".join(f"{x['n']}\t{x['who']}\t{x['key']}\t{x['t_start']}\t{x['t_end']}\t{x['status'][:60]}\t{x['note'].replace('\t', ' ').replace('\n', ' ')[:300]}\n" for x in res))
    json.dump(dict(take=n, ai=llm_mode(), nonce=nonce, steps=res, stop_rc=rc_stop, stop=err_stop, tally=tally, d_g=dg), open(C.R + "/rehearse.json", "w"), ensure_ascii=False, indent=1)
    print(ai_line)  # 頭の行: 偽の AI で回した run は `AI: fake`
    print("| # | 誰 | 手 | 状態 | 秒 | 目安 |\n|---|---|---|---|---|---|")
    for x in res:
        print(f"| {x['n']} | {x['who']} | {x['key']} | {x['status'][:70]} | {x['sec']} | {x['target']} |")
    print(f"lies/dead_clicks = {tally.get('lies')}/{tally.get('dead_clicks')} · D-g violations = {dg} · stop rc = {rc_stop} · 合計 {sum(x['sec'] for x in res):.0f} 秒 · 結果 {C.R}/rehearse.json" + (f" · llm real {lj['calls']} 回 {lj['secs']} 秒" if lj["mode"] == "real" else ""))
    if red or fg_red or (not rec and (dg or tally.get("lies") or tally.get("dead_clicks"))):
        return 1
    if down:
        print(f"rehearse: driver down: {down}", file=sys.stderr)
        return 7
    # 標準の未測（今の tally が常に積む surface の oracle の対・STANDING_UNMEASURED）は 6 にしない。手の未測・AX の木が無い・error は 6
    return 6 if any(x["status"].startswith(("未測", "blocked")) for x in res) or "error" in tally or T.open_unmeasured(tally.get("unmeasured") or {}) else 0


def fg_check(path):
    """`--fg <md>`: owner の FG の表（fg-checklist.md）の「見える筈の物」の列（4 列目）の `…` が strings.csv に在るかを見る。英語の .app 用に CSV を取り直した後の 1 command。"""
    with tempfile.TemporaryDirectory(prefix="rehearse-strings-") as d:
        pats = csv_patterns(fresh_strings_csv(d))
    n = 0
    bad = []
    for line in open(path, encoding="utf-8"):
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 6 or not line.startswith("|"):
            continue
        for q in re.findall(r"`([^`]+)`", re.sub(r"⟦未収録:[^⟧]*⟧", "", cells[3])):
            if re.fullmatch(r"[\w/]+\.rs(:\d+)?", q):  # code の場所の指し示しは画面の文言ではない
                continue
            n += 1
            if not any(x.fullmatch(q) for x in pats):
                bad.append(f"{cells[0]}: 「{q}」が strings.csv に無い")
    print(f"fg-check: 文言 {n} 件 · 不一致 {len(bad)} 件")
    print("\n".join("赤: " + b for b in bad))
    return 1 if bad else 0


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--fg":
        return fg_check(sys.argv[2])
    ap = argparse.ArgumentParser()
    ap.add_argument("n", choices=["1", "2", "3", "4", "5", "scene2", "short"])  # 1..5 = full.md の録画の回・scene2 = docs/demo/scene2.md（W15-M3）・short = docs/demo/short.md（録画する短い台本・102 通目）
    ap.add_argument("app", nargs="?")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--out")
    a = ap.parse_args()
    rows, raw = parse({"scene2": SCENE2, "short": SHORT}.get(a.n, DEMO))
    global SCRIPT
    SCRIPT = a.n if a.n in ("short", "scene2") else "full"
    if a.dry_run:
        return dry_run(rows, raw, f"d{a.n}x<epoch>")
    bad, _, _ = check(rows, raw)
    if bad:
        print("\n".join("赤: " + b for b in bad), file=sys.stderr)
        return 1
    if not a.app or not os.path.isdir(a.app):
        print("usage: rehearse.sh <1..5|scene2|short> <Shibubu.app> | --dry-run", file=sys.stderr)
        return 2
    return live(rows, a.n, a.app, a.out)


if __name__ == "__main__":
    sys.exit(main())
