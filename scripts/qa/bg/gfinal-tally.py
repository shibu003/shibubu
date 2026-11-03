#!/usr/bin/env python3
"""gfinal-tally.py — G-final の BG の走らせ直し（gfinal-bg.sh）の順序の表の読み書きと、結果を 1 つの表にする。数え方は tally.py のまま（import して呼ぶだけ）。

使い方:
  gfinal-tally.py plan <plan.tsv> [--from n] [--only n] [--dry]   表を検査（壊れていれば exit 1）。--dry は一覧と見積もりを印字・無ければ run の行（n<TAB>cmd）を印字
  gfinal-tally.py table <plan.tsv> <out-dir> <bg-tally.md>        <out-dir>/<n>/{run,meta.json} を集めて md に書く。exit: 0 全部緑（任意の行の未測は数えない）/ 1 赤あり / 6 未測あり（赤なし）
meta.json は gfinal-bg.sh が書く: {"status": "ran"|"idle", "rc", "secs", "idle_s"}。
保証する: lies・dead_clicks・D-g は tally.count の値そのまま。駆動 rc≠0（6 = driver の未測は赤にしない）・lies/dead/D-g が 1 件でもあれば赤。driver 無し・走らせていない・owner 使用中は未測と理由を書き、緑にしない。
任意: plan の item が「[任意]」で始まる行（本物の REA など）の未測は「任意未測」と別に数え、exit を 6 にしない（赤は赤のまま）。
保証しない: 録画（FG）の合否・driver の無い項目の合否。
"""
import glob, json, os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tally  # 数え方の正本

OPT = "[任意]"  # item の頭にこれが在る行は、未測でも完成を止めない
LLM_MAX = 10  # 1 run の LLM 呼び出しの上限（YC_WAVES W13 の行）


def load_plan(path):
    rows, last = [], 0
    for i, ln in enumerate(open(path, encoding="utf-8"), 1):
        if not ln.strip() or ln.startswith("#"):
            continue
        c = ln.rstrip("\n").split("\t")
        if len(c) != 7:
            raise ValueError(f"{path}:{i}: 欄が {len(c)} 個（7 個要る）")
        n, wave, item, llm, est, cmd, note = c
        if not (n.isdigit() and int(n) > last):
            raise ValueError(f"{path}:{i}: n={n!r} は前の行より大きい整数でない")
        last = int(n)
        if not (llm.isdigit() and est.isdigit()):
            raise ValueError(f"{path}:{i}: llm / est_s が整数でない")
        if int(llm) > LLM_MAX:
            raise ValueError(f"{path}:{i}: llm={llm} が 1 run の上限 {LLM_MAX} を超える（run を分ける）")
        if cmd == "-":
            if note in ("", "-"):
                raise ValueError(f"{path}:{i}: driver 無しの行に理由（note）が無い")
        else:
            for tok in cmd.split():
                if tok.endswith((".py", ".sh")) and not tok.startswith(("{", "-")) and not os.path.isfile(os.path.join(HERE, "../../..", tok)):
                    raise ValueError(f"{path}:{i}: driver {tok} が無い")
        rows.append(dict(n=int(n), wave=wave, opt=item.startswith(OPT), item=item, llm=int(llm), est=int(est), cmd=cmd, note=note))
    if not rows:
        raise ValueError(f"{path}: 行が無い")
    return rows


def pick(rows, a):
    lo = int(a[a.index("--from") + 1]) if "--from" in a else 0
    only = int(a[a.index("--only") + 1]) if "--only" in a else None
    return [r for r in rows if r["n"] >= lo and (only is None or r["n"] == only)]


def cmd_plan(a):
    rows = pick(load_plan(a[0]), a)
    if "--dry" not in a:
        for r in rows:
            if r["cmd"] != "-":
                print(f"{r['n']}\t{r['cmd']}")
        return 0
    run = [r for r in rows if r["cmd"] != "-"]
    for r in rows:
        print(f"{r['n']:>3} | {r['wave']} | {r['item']} | llm {r['llm']} | {r['est']}s | {r['cmd'] if r['cmd'] != '-' else '未測（driver 無し）: ' + r['note']}")
    tot = sum(r["est"] for r in run)
    print(f"起こす run {len(run)} 本・driver 無しの未測 {len(rows) - len(run)} 件・見積もり {tot}s（{tot / 3600:.1f}h・上限か過去 log の実測の足し算・.app は起こさない）")
    return 0


def judge(r, out):
    """1 行 → (lies, dead, dg, secs, 判定, 理由)。"""
    if r["cmd"] == "-":
        return "-", "-", "-", "-", "未測", r["note"]
    d = os.path.join(out, str(r["n"]))
    try:
        meta = json.load(open(os.path.join(d, "meta.json")))
    except (OSError, ValueError):
        return "-", "-", "-", "-", "未測", "走らせていない"
    if meta.get("status") == "idle":
        return "-", "-", "-", "-", "未測", f"owner 使用中（HIDIdleTime {meta.get('idle_s')}s < 120s）"
    try:
        t = tally.count_runs(os.path.join(d, "run"), os.path.join(d, "run-restart"))  # rehearse は再起動の後を run-restart に書く（run だけでは再起動の後の lies を見ない）
    except (ValueError, OSError) as e:
        return "-", "-", "-", meta.get("secs"), "未測", f"tally 不能: {str(e)[:80]}"
    lies, dead, dg = len(t.lies), len(t.dead_clicks), len(t.guard_failures)
    um = []
    for f in sorted(glob.glob(os.path.join(d, "run", "*.json")) + glob.glob(os.path.join(d, "run-restart", "*.json"))):
        if os.path.basename(f) in ("run.json", "tally.json"):
            continue
        try:
            j = json.load(open(f))
        except ValueError:
            continue
        if isinstance(j, dict) and j.get("unmeasured"):
            um = list(j["unmeasured"])
            break
    owner = tally.guard_state(t.guard_failures, t.unmeasured) == "unmeasured(owner_returned)"  # owner が戻って front が鳴っただけ = 製品の違反でない
    bad = [x for x in (f"driver rc={meta.get('rc')}" if meta.get("rc") not in (None, 0, 6) and not (owner and meta.get("rc") == 1) else "", f"lies {lies}" if lies else "", f"dead_clicks {dead}" if dead else "", f"D-g {dg}" if dg else "") if x]
    if bad:
        return lies, dead, dg, meta.get("secs"), "赤", "・".join(bad)
    if owner:
        return lies, dead, dg, meta.get("secs"), "未測", "unmeasured(owner_returned): " + ",".join(k for k in t.unmeasured if k.removeprefix("restart:").startswith("owner_returned."))
    if tally.guard_state(t.guard_failures, t.unmeasured) == "unmeasured(hidden)":  # 対象の app が hidden / 窓 0 の間の赤 = 測れていない（W15-b70）
        return lies, dead, dg, meta.get("secs"), "未測", "unmeasured(hidden): " + ",".join(k for k in t.unmeasured if k.removeprefix("restart:").startswith("hidden."))
    if meta.get("rc") == 6:  # driver の約束: 6 = 未測（owner 使用中・dry-run・AX に出ない）。赤にも緑にも数えない
        return lies, dead, dg, meta.get("secs"), "未測", "driver が未測を返した（rc=6）"
    if um:
        return lies, dead, dg, meta.get("secs"), "未測あり", "手の未測 " + ",".join(str(x) for x in um)[:120]
    return lies, dead, dg, meta.get("secs"), "緑", ""


def llm_real(out, r):
    """その行の run の dir（<out>/<n>/run・run-restart）に llm.json の mode が real の物が在るか。"""
    for sub in ("run", "run-restart"):
        try:
            if json.load(open(os.path.join(out, str(r["n"]), sub, "llm.json")))["mode"] == "real":
                return True
        except (OSError, ValueError, KeyError):
            pass
    return False


def cmd_table(a):
    rows, out, md = load_plan(a[0]), a[1], a[2]
    res = [(r, judge(r, out)) for r in rows]
    res = [(r, j[:4] + ("任意未測",) + j[5:] if r["opt"] and j[4] == "未測" else j) for r, j in res]
    res = [(r, j[:5] + (("[llm real] " + j[5]).strip(),)) if llm_real(out, r) else (r, j) for r, j in res]  # 本物の AI の run は理由の欄に印（fake と混ぜて数えない・W15-b69）
    lines = ["# G-final の BG の走らせ直し（gfinal-bg.sh・gfinal-tally.py）", "",
             "| run | Wave | 項目 | lies | dead_clicks | D-g | 秒 | 判定 | 理由 |", "|---|---|---|---|---|---|---|---|---|"]
    lines += [f"| {r['n']} | {r['wave']} | {r['item']} | {j[0]} | {j[1]} | {j[2]} | {j[3]} | {j[4]} | {j[5]} |" for r, j in res]
    c = {k: sum(1 for _, j in res if j[4] == k) for k in ("緑", "赤", "未測あり", "未測", "任意未測")}
    lines += ["", f"緑 {c['緑']} / 赤 {c['赤']} / 未測あり {c['未測あり']} / 未測 {c['未測']} / 任意未測 {c['任意未測']}（未測は緑に数えない・任意未測は exit に数えない）"]
    os.makedirs(os.path.dirname(os.path.abspath(md)), exist_ok=True)
    open(md, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 1 if c["赤"] else (0 if not (c["未測あり"] or c["未測"]) else 6)


if __name__ == "__main__":
    try:
        if len(sys.argv) >= 3 and sys.argv[1] == "plan":
            sys.exit(cmd_plan(sys.argv[2:]))
        if len(sys.argv) == 5 and sys.argv[1] == "table":
            sys.exit(cmd_table(sys.argv[2:]))
    except (ValueError, OSError) as e:
        print(f"gfinal-tally.py: {e}", file=sys.stderr)
        sys.exit(1)
    print(__doc__, file=sys.stderr)
    sys.exit(2)
