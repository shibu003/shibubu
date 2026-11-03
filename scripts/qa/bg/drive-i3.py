#!/usr/bin/env python3
"""drive-i3.py — I3（held-out の一言）の BG の 1 run: heldout.txt の 1 文を Home の欄に打って「作る」→ DB と AX で読み戻す。
使い方: python3 -I scripts/qa/bg/drive-i3.py <run-dir> <Shibubu.app> [--heldout <file>] [--dry-run]
  heldout の既定 = ~/.config/shibubu/heldout.txt（1 行 1 文・空行と # 行は無視・repo の外・文は commit にも log にも書かない）。1 run で 1 文を乱数で引く。
  LLM を使う run は呼ぶ側が SHIBUBU_QA_LLM=1（owner の quota の承認がある時だけ）を渡す。回数は SHIBUBU_LLM_BUDGET（既定 calls=10）。
  --dry-run: 文を引いて sha256 の頭 8 桁だけ印字する（.app を起こさない・画面に触らない）。
判定（oracle/project.py read・tally.py の lies / dead_clicks とは別）: since より後に作られた revision 1 が generate・Page が 2 枚以上・surface_specs が在る（AX は「この Project への変更」の欄が出る事）。
  ponytail: target 集合が一言の名指した app / origin と一致するか（I3 の本判定）は文を repo に書かない都合で、読み戻した target の集合を oracle/project.json に残すだけ・突き合わせは operator が文を見て行う。
保証する: 文は log・stdout・steps・ax から消す（sha256 の頭 8 桁に置換）・--needle を渡さない（snap に残さない）。DB の goal は製品のデータで run の dir の外へ出さない。
exit: 0 ok / 1 赤（D-g の violation・DB が条件を満たさない）/ 2 使い方・heldout が無い・app が無い / 6 owner 使用中か AX に出ず未測"""
import hashlib, json, os, random, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import rehearse as RH  # noqa: E402  step / find / press / run_sh / Ctx / Unmeasured（台本の通しと同じ primitive）

DEFAULT_HELDOUT = os.path.expanduser("~/.config/shibubu/heldout.txt")
FIELD, BUTTON, DONE = "goal", "create", "change"  # labels.py の key


def pick(path, rng=random):
    lines = [l.strip() for l in open(path, encoding="utf-8") if l.strip() and not l.lstrip().startswith("#")]
    if not lines:
        raise ValueError(f"{path} に文が無い")
    return rng.choice(lines)


def scrub(R, text, sha8):
    """run の dir（steps / ax / drive.log）の文字列から文を消す。DB（R/data）は製品の物なので触らない。"""
    for sub in ("steps", "ax", "oracle"):
        for root, _, fs in os.walk(os.path.join(R, sub)):
            for f in fs:
                p = os.path.join(root, f)
                try:
                    s = open(p, encoding="utf-8").read()
                except (UnicodeDecodeError, OSError):
                    continue
                if text in s:
                    open(p, "w", encoding="utf-8").write(s.replace(text, f"<heldout:{sha8}>"))


def main(a):
    if len(a) < 2:
        print("usage: drive-i3.py <run-dir> <Shibubu.app> [--heldout <file>] [--dry-run]", file=sys.stderr)
        return 2
    R, app = a[0], a[1]
    hf = a[a.index("--heldout") + 1] if "--heldout" in a else DEFAULT_HELDOUT
    if not os.path.isfile(hf):
        print(f"drive-i3: heldout が無い: {hf}（文を 1 行 1 文で置く）", file=sys.stderr)
        return 2
    S = pick(hf)
    sha8 = hashlib.sha256(S.encode()).hexdigest()[:8]
    os.makedirs(R, exist_ok=True)
    LOG = open(os.path.join(R, "drive.log"), "a")

    def log(*x):
        s = " ".join(str(y) for y in x)
        print(s, flush=True); LOG.write(s + "\n"); LOG.flush()

    log("i3: heldout sha8", sha8)
    if "--dry-run" in a:
        return 0
    if not os.path.isdir(app):
        log("app が無い:", app); return 2
    os.environ.setdefault("SHIBUBU_LLM_BUDGET", "calls=10")
    t0 = time.time()
    rc, err = RH.run_sh("start", R, app)
    log("start rc", rc, err)
    if rc == 6 and json.load(open(R + "/run.json")).get("owner_active"):
        RH.run_sh("stop", R); return 6
    if rc not in (0, 6):
        RH.run_sh("stop", R); return 1
    C = RH.Ctx(R, app, sha8)
    verdict, code = "ok", 0
    try:
        e = RH.find(C, "AXTextField", FIELD)
        RH.step(C, "click", {"element_token": e["element_token"]})
        e = RH.find(C, "AXTextField", FIELD)
        o = RH.step(C, "type_text", {"text": S, "element_token": e["element_token"]})  # --needle を渡さない
        if o.get("effect") == "refused" or "raw" in o:
            raise RH.Unmeasured("Home の欄に AX で打てない")
        RH.press(C, "AXButton", BUTTON)
        RH.find(C, "AXTextField", DONE, tries=60)  # LLM の生成を待つ（1 周 約 2 秒）
    except RH.Unmeasured as e:
        verdict, code = f"未測: {e}", 6
    except RuntimeError as e:
        verdict, code = f"赤: {e}", 1
    log("stop rc", *RH.run_sh("stop", R))
    t = subprocess.run(["python3", "-I", HERE + "/tally.py", R], capture_output=True, text=True)
    open(R + "/tally.json", "w").write(t.stdout)
    os.makedirs(R + "/oracle", exist_ok=True)
    p = subprocess.run(["python3", "-I", HERE + "/oracle/project.py", "read", R + "/data", "--since", str(t0)], capture_output=True, text=True)
    open(R + "/oracle/project.json", "w").write(p.stdout)
    proj_ok = p.returncode == 0 and json.loads(p.stdout)["ok"]
    if code == 0 and not proj_ok:
        verdict, code = "赤: DB に条件を満たす生成が無い（revision 1 が generate・Page 2 枚以上・surface_specs）", 1
    log("i3:", verdict, "· tally rc", t.returncode)
    json.dump({"heldout_sha8": sha8, "verdict": verdict, "project_ok": proj_ok}, open(R + "/i3.json", "w"), ensure_ascii=False)
    scrub(R, S, sha8)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
