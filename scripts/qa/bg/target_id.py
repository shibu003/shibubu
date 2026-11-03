#!/usr/bin/env python3
"""target_id — 「名指しの対象を正しく編集できる・決められない時は何も壊さない」を 4 場面で測る（W15-b64・owner 10-26「README が開いている場合だけでなく、閉じている場合、同名 file が複数ある場合、README 以外の書類が前面にある場合も」）。
27 手の台本（docs/demo/full.md）と rehearse.py の手は変えない。rehearse の部品（run_sh・step・send_field・h_generate・open_textedit・write_fake）を使い、場面ごとに新しい run で回す。

使い方（target-id.sh が呼ぶ）: target_id.py --dry-run | target_id.py <Shibubu.app> --out DIR [--only S1,S2]
場面（どれも Page の欄に #20 と同じ一言『Change the README title to “Shibubu Demo” and save it』・偽の CLI の generate.json は document = README.md）:
  S1 開いている   README.md と notes.md・README が後ろ                 合格: README の sha だけ変わり中身に Shibubu Demo・notes は不変
  S2 閉じている   notes.md だけ                                         合格: どの file も不変・製品が README の開いていない旨の 1 行・Verified にならない
  S3 同名 2 つ    別 folder の README.md 2 枚（＋notes）                合格: どれも不変（区別できない 1 行）か、repo の README だけ変わる。もう片方・notes が変わったら赤
  S4 前面が別     notes.md を前面・README.md は後ろ                     合格: README だけ変わる（前面の notes に釣られたら赤）
赤 = 名指しでない file が 1 つでも変わった / Verified なのに README の中身が違う / 合格に届かない。未測 = 窓を並べられない・owner が使用中・run が始まらない。
保証する: file と folder は run の out の下（owner の file に触らない）・表に実名の path を出さない（README / notes / other の呼び名だけ）・自分が起こした TextEdit の pid だけ終了・27 手と同じ BG の作法（run.sh・front-guard・b62 の owner_pointer）。
保証しない: 製品の 1 行の文言（G75 の前は決まっていない = 新しく出た AXStaticText を全部写すだけ）。
"""
import argparse, json, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import rehearse as R  # noqa: E402

SAID = "Change the README title to “Shibubu Demo” and save it"  # #20 の一言（full.md の行 20）
SCENES = {
    "S1": ("開いている", "README.md と notes.md・README が後ろ", "README の sha だけ変わり中身に Shibubu Demo・notes は不変"),
    "S2": ("閉じている", "notes.md だけ", "どの file も不変・製品が README の開いていない旨の 1 行・Verified にならない"),
    "S3": ("同名 2 つ", "別 folder の README.md 2 枚（＋notes）", "どれも不変（区別できない 1 行）か、repo の README だけ変わる"),
    "S4": ("前面が別", "notes.md を前面・README.md は後ろ", "README だけ変わる（前面の notes に釣られたら赤）"),
}
NAMES = {"README": "README", "notes": "notes", "other": "other"}  # 表に出す file の呼び名（実名の path は出さない）


def judge(scene, changed, readme_ok, verified, line):
    """純関数。changed = 前後で sha が変わった file の呼び名の集合（README = repo の README・notes・other = 別 folder の README）。
    -> (状態 "ok"/"red", 理由)。未測は run_scene が窓を並べられない時に作る（ここには来ない）。"""
    allowed = {"S1": {"README"}, "S2": set(), "S3": {"README"}, "S4": {"README"}}[scene]
    if changed - allowed:
        return "red", "名指しでない file が変わった: " + ",".join(sorted(changed - allowed))
    if verified and "README" in changed and not readme_ok:
        return "red", "Verified なのに README の中身に Shibubu Demo が無い"
    if scene in ("S1", "S4"):
        if changed != {"README"}:
            return "red", "README が変わっていない"
        return ("ok", "") if readme_ok else ("red", "README の中身に Shibubu Demo が無い")
    if changed == {"README"}:  # S3 で repo の README だけ変えた = 正しい path の 1 枚
        return ("ok", "") if readme_ok else ("red", "README の中身に Shibubu Demo が無い")
    if verified:
        return "red", "どの file も変わっていないのに Verified"
    if scene == "S2" and "README" not in (line or ""):
        return "red", "製品の 1 行が README の開いていない旨でない / 無い"
    if not line:
        return "red", "何も変えず製品の 1 行も無い"
    return "ok", ""


def new_lines(before, after):
    """前に無かった AXStaticText の原文（出た順）。"""
    seen = set(before)
    out = []
    for t in after:
        if t not in seen:
            seen.add(t)
            out.append(t)
    return out


def texts(C):
    return [e.get("value") or e.get("label") for e in R.elements(C) if e.get("role") == "AXStaticText" and (e.get("value") or e.get("label"))]


def table(rows):
    out = ["| 場面 | 状態 | 変わった file | README の中身 | Verified | 製品の 1 行 | 秒 |", "|---|---|---|---|---|---|---|"]
    for r in rows:
        name = SCENES[r["scene"]][0]
        out.append(f"| {r['scene']} {name} | {r['status']}{(': ' + r['why']) if r['why'] else ''} | {','.join(r['changed']) or '(なし)'} | {'Shibubu Demo' if r['readme_ok'] else '違う/不明'} | {'あり' if r['verified'] else 'なし'} | {(r['line'] or '(なし)')[:160].replace('|', '/')} | {r['sec']} |")
    return "\n".join(out)


def exit_code(rows):
    s = [r["status"] for r in rows]
    return 1 if "赤" in s else (6 if "未測" in s else 0)


def shas(C, paths):
    return {k: (R.sha_of(p) if os.path.exists(p) else None) for k, p in paths.items()}


def poll_product(C, base, secs, stop_words):
    """送った後の製品の 1 行を待つ -> ("approve"/"denied"/"words"/"stable"/"timeout", 新しく出た原文の列)。
    承認の button・拒否の行（N windows…）・stop_words のどれか・新しい原文が 6 秒変わらない（G75 の前は文言が決まっていない）で止まる。"""
    end, last, same = time.time() + secs, None, 0
    while True:
        els = R.elements(C)
        if any(e.get("role") == "AXButton" and not e.get("md") and R.L.hit(e, "approve") for e in els):
            return "approve", new_lines(base, texts(C))
        new = new_lines(base, [e.get("value") or e.get("label") for e in els if e.get("role") == "AXStaticText" and (e.get("value") or e.get("label"))])
        if any(R.re.search(r"\d+ windows", t) for t in new):
            return "denied", new
        if any(w in t for t in new for w in stop_words):
            return "words", new
        same = same + 1 if new and new == last else 0
        last = new
        if same >= 2:
            return "stable", new
        if time.time() >= end:
            return "timeout", new
        time.sleep(3)


def open_in_scene(C, name, path):
    te = R.open_textedit(path)
    C.state["te_sets"].append(te)
    pids = R.mine(*te)
    C.state.setdefault("pids", {})[name] = pids[0] if len(pids) == 1 else None


def win_of(C, pid, prefix):
    for _ in range(10):
        for w in R.step(C, "list_windows", {"pid": pid}, own=True).get("windows") or []:
            if w.get("pid") == pid and (w.get("title") or "").startswith(prefix):
                return w["window_id"]
        time.sleep(2)
    return None


def run_scene(scene, app, out):
    """-> row（scene・status・why・changed・readme_ok・verified・line・sec）。"""
    t0 = time.time()
    row = dict(scene=scene, status="未測", why="", changed=[], readme_ok=False, verified=False, line="", sec=0)
    fin = lambda st, why="": (row.update(status=st, why=why, sec=round(time.time() - t0)), row)[1]
    Rd = os.path.join(out, f"{scene}-{int(t0)}")  # run.sh の session = qa-<dir 名>。cua-driver は終えた session の名前を使い回させないので、場面の dir 名は run ごとに変える
    os.makedirs(out, exist_ok=True)
    p = subprocess.run(["bash", os.path.join(HERE, "fresh-repo.sh"), "init", Rd], capture_output=True, text=True)
    if p.returncode != 0:
        return fin("未測", "fresh-repo.sh init が失敗: " + p.stderr[-120:])
    C = R.Ctx(Rd, app, f"t{scene}x{int(time.time())}")
    C.state["te_sets"] = []
    readme = os.path.join(C.repo, "README.md")
    notes = os.path.join(C.repo, "docs", "notes.md")
    other = os.path.join(Rd, "other", "README.md")
    front0 = R.front_app()
    try:
        os.makedirs(os.path.dirname(notes), exist_ok=True)
        open(notes, "w", encoding="utf-8").write("notes\n")
        os.makedirs(os.path.join(C.repo, ".git", "info"), exist_ok=True)
        open(os.path.join(C.repo, ".git", "info", "exclude"), "a", encoding="utf-8").write("docs/notes.md\n")
        if scene in ("S1", "S3", "S4"):
            open_in_scene(C, "README", readme)
        if scene == "S3":
            os.makedirs(os.path.dirname(other), exist_ok=True)
            open(other, "w", encoding="utf-8").write("# other readme\n")
            open_in_scene(C, "other", other)
        open_in_scene(C, "notes", notes)  # S4 は最後に開いた notes が前面のまま（restore_front しない）
        if scene != "S4":
            R.restore_front(front0)
        rc, err = R.run_sh("start", Rd, app, ws=C.repo, fake=R.write_fake(Rd, C.repo))
        try:
            j = json.load(open(Rd + "/run.json"))
        except (OSError, ValueError):
            j = {}  # start が run.json を書かずに落ちた = app が居ない
        # rc 6 は 2 つ（rehearse の再起動と同じ）: owner 使用中で起こしていない / 起こした後の 00-launch が未測（focus 無し = owner 不在でも出る・app は居る → 続ける）
        if j.get("owner_active"):
            return fin("未測", "owner が使用中で run が始まらない")
        if rc not in (0, 6) or j.get("window_id") is None:
            return fin("未測", f"run.sh start rc={rc} {err[-100:]}")
        rows, _ = R.parse(R.DEMO)
        R.h_generate(C, next(r for r in rows if r["key"] == "generate"))
        pids = C.state["pids"]
        # 偽の CLI の「正しい対象」= repo の README の窓。S2 は README が無いので notes の窓（誤った代用先）を名指しの窓番号に入れる
        tgt = "README" if scene != "S2" else "notes"
        wid = win_of(C, pids.get(tgt), "README.md" if tgt == "README" else "notes.md") if pids.get(tgt) else None
        if not wid:
            return fin("未測", "窓番号が引けない（TextEdit の窓を並べられない）")
        R.write_fake(Rd, C.repo, pids[tgt], wid)
        paths = {"README": readme, "notes": notes, **({"other": other} if scene == "S3" else {})}
        before = shas(C, paths)
        base = texts(C)
        for lbl in ("close", "cancel"):
            R.try_press(C, "AXButton", lbl)
        R.send_field(C, "AXTextField", "change", SAID)
        R.press(C, "AXButton", "send_edit")
        done = R.L.names("done_checked")[0]
        stop_words = (done, R.L.names("nothing_sent")[0], "Needs your consent", "Failed")
        kind, new = poll_product(C, base, int(90 * R.llm_scale()), stop_words)
        if kind == "approve":
            R.press(C, "AXButton", "approve")
            kind, new = poll_product(C, base, int(240 * R.llm_scale()), stop_words + ("can't tell which",))
            for _ in range(12):  # 保存は数秒〜30 秒後に disk へ
                if shas(C, paths) != before:
                    break
                time.sleep(5)
        after = shas(C, paths)
        row["changed"] = sorted(k for k in paths if after[k] != before[k])
        row["readme_ok"] = "Shibubu Demo" in open(readme, encoding="utf-8").read()
        row["verified"] = any(done in t for t in new)
        row["line"] = " / ".join(t[:120] for t in new if not t.startswith("To do "))[:300]
        st, why = judge(scene, set(row["changed"]), row["readme_ok"], row["verified"], row["line"])
        return fin("ok" if st == "ok" else "赤", why)
    except R.Unmeasured as e:
        return fin("未測", str(e)[:200])
    except R.DriverDown as e:
        return fin("未測", "driver: " + str(e)[:160])
    except Exception as e:  # D-g の violation など
        return fin("赤", str(e)[:200])
    finally:
        R.run_sh("stop", Rd)
        R.write_llm_json(Rd)  # real / fake・回数・秒（W15-b69）
        for te in C.state["te_sets"]:
            R.kill_mine(*te)
        R.restore_front(front0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("app", nargs="?")
    ap.add_argument("--out")
    ap.add_argument("--only")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    ids = [s for s in SCENES if not a.only or s in a.only.split(",")]
    if a.dry_run:
        print("| 場面 | 窓の並べ方 | 合格 |\n|---|---|---|")
        for s in ids:
            print(f"| {s} {SCENES[s][0]} | {SCENES[s][1]} | {SCENES[s][2]} |")
        return 0
    if not a.app or not os.path.isdir(a.app) or not a.out:
        print("usage: target-id.sh --dry-run | <Shibubu.app> --out DIR [--only S1,S2]", file=sys.stderr)
        return 2
    try:
        R.llm_env()  # 本物の AI の門（REHEARSE_LLM=real は SHIBUBU_QA_LLM_OK が要る）
    except R.LlmGate as e:
        print(f"target-id: {e}", file=sys.stderr)
        return e.rc
    idle = R.owner_idle_s(os.path.dirname(os.path.abspath(a.out)), os.path.expanduser("~/shibubu-yc-runs"))
    if idle is not None and idle < 120:
        print(f"target-id: owner 使用中で未測（idle {idle:.0f}s < 120s）", file=sys.stderr)
        return 5
    rows = [run_scene(s, a.app, a.out) for s in ids]
    md = table(rows)
    open(os.path.join(a.out, "target-id.md"), "w", encoding="utf-8").write(md + "\n")
    json.dump(rows, open(os.path.join(a.out, "target-id.json"), "w"), ensure_ascii=False, indent=1)
    print(md)
    return exit_code(rows)


if __name__ == "__main__":
    sys.exit(main())
