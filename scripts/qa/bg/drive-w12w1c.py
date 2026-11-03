#!/usr/bin/env python3
"""W12W-1c の場の通し: 同じ手を場（SHIBUBU_QA_SCENE）ごとに回し、tally.py の causes（製品 / cua-driver）と scene_held を並べる。
run.sh の load / start / step / stop だけを使う（element_token の手だけ・座標も前面も使わない）。token は直前の get_window_state から label で引く。
使い方: python3 -I scripts/qa/bg/drive-w12w1c.py <scene> <run-dir> <Shibubu.app>   → <run-dir>/drive.log・<run-dir>/tally.json
  scene: normal_low（自分の負荷なし）/ normal_load（start の前に run.sh load <R> 600 4・窓の残りが LOAD_MARGIN_S を切ったら手の前に重ねる）
         / fullscreen・split_view（その Space は operator が run の前に作り後で戻す・この script は Space を変えない）
  normal_load の負荷は owner が使用中なら作らない（run.sh を source して owner_active "$(owner_idle_s)" を load の前に 1 回ずつ・run.sh load は gate を持たない）:
    start の前なら作らずに start へ（start の gate が run.json に owner_active を書いて exit 6）・run の途中なら stop して exit 6。app が無ければ負荷も start も打たない。
手（名前・tool・--want は場によらず同じ）: 00-launch → 01-home（読む）→ 02-click-input → 03-type-goal → 04-read-typed（読む）→ 05-create → 06-read-page（読む）。
  --want（bin/axprobe が AX で探す物・cause_of の ok/reached と defect/elements_missing を決める）: 読む手は次の手の的の role・打つ手は自分の的の role・06 は Page の button。
  ponytail: Home の欄も「作る」も AX identifier を持たず、axprobe は label を見ないので role= で探す（「作る」を他の button と区別しない）。
    この goal の Page に必ず在る identifier も無い（slot:* は配置の型の在る Page だけ・src/composer/layout.rs の place）ので 06 も role=AXButton。区別が要る時は axprobe に label の want を足す。
  打つ手の token が直前の木に無い時はその手を打たず（run.sh の step_allowed は token の無い click を通さない）、tally.json の unmeasured["hand.<名前>"] に理由を書く
    （場ごとの手の集合 = causes の手 ∪ hand.* の手が同じ）。
exit: 0 = stop まで回して tally.json を書いた / 6 = owner が使用中（start の gate で起こさなかった・run の途中で負荷を作らなかった）/ 2 = app が無い（何も打たない）
  / 1 = それ以外（D-g の violation は run.sh がその場で止める）"""
import json, os, subprocess, sys, time

BG = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, BG)
import labels as L  # noqa: E402  label の 1 か所の表（EN / JP の対）
import tally  # noqa: E402  driver_of（打たなかった手の理由に読みの手の cua-driver の判定を書く）

SCENE, R, APP = sys.argv[1], sys.argv[2], sys.argv[3]
GOAL = "読書記録をつけたい"
FIELD, BUTTON = "role=AXTextField", "role=AXButton"  # --want
LOAD_SECS, LOAD_PROCS, LOAD_MARGIN_S = 600, 4, 150  # 本数は 8 CPU の半分（他の列の load が 50〜170 の機械で owner の邪魔を増やさない）
SKIPPED: dict[str, str] = {}  # 打たなかった手 → 理由（tally.json の unmeasured["hand.<名前>"]）
os.makedirs(R, exist_ok=True)
LOG = open(os.path.join(R, "drive.log"), "a")


def log(*a):
    s = " ".join(str(x) for x in a)
    print(s, flush=True); LOG.write(s + "\n"); LOG.flush()


def run(*a, env=None):
    env = dict(env or os.environ)
    for k in ("SHIBUBU_QA_LLM", "ANTHROPIC_API_KEY"):  # model を使わない（run.sh が SHIBUBU_LLM_CLI を空にする・quota と従量課金 0）
        env.pop(k, None)
    p = subprocess.run(["bash", BG + "/run.sh", *a], capture_output=True, text=True, env=env)
    return p.returncode, p.stderr.strip()[-300:]


def tally_and_exit(code):
    p = subprocess.run(["python3", "-I", BG + "/tally.py", R], capture_output=True, text=True)
    out = p.stdout
    if p.returncode == 0 and SKIPPED:
        d = json.loads(out)
        d["unmeasured"].update({"hand." + k: v for k, v in SKIPPED.items()})
        out = json.dumps(d, ensure_ascii=False, sort_keys=True) + "\n"
    open(R + "/tally.json", "w").write(out)
    log("tally rc", p.returncode, p.stderr.strip()[-200:])
    if code == 1 and p.returncode == 0 and tally.guard_state(json.loads(out)["guard_failures"], json.loads(out)["unmeasured"]) == "unmeasured(owner_returned)":
        log("owner が戻ったので止めた（未測・再試行なし）"); code = 6
    sys.exit(code if p.returncode == 0 else 1)


def owner_active():
    """start の gate と同じ読み（run.sh の owner_idle_s と owner_active・閾値は OWNER_IDLE_MIN_S）。True = 使用中か読めない。"""
    return subprocess.run(["bash", "-c", 'source "$1" && owner_active "$(owner_idle_s)"', "drive", BG + "/run.sh"]).returncode == 0


def keep_loaded():
    """normal_load: 窓の残りが LOAD_MARGIN_S を切っていたら次の手の前に load を重ねる（probe の在る手を全部どれかの窓に入れる）。"""
    if SCENE != "normal_load":
        return
    p = R + "/load.jsonl"
    t1 = max((json.loads(l)["t1"] for l in open(p) if l.strip()), default=0) if os.path.exists(p) else 0
    if t1 - time.time() >= LOAD_MARGIN_S:
        return
    started = os.path.exists(R + "/run.json")
    if owner_active():  # owner が使っている時に負荷を作らない（YC_DECISIONS 19 通目）
        log("owner が使用中: load を打たない")
        if started:
            run("stop", R); tally_and_exit(6)
        return  # start の前: start の gate が決める
    rc, err = run("load", R, str(LOAD_SECS), str(LOAD_PROCS))
    log("load rc", rc, err)
    if rc != 0:
        run("stop", R)  # run.json の前でも stop は load.jsonl の自分の loop を kill する
        tally_and_exit(1) if started else sys.exit(1)


def base():
    j = json.load(open(R + "/run.json"))
    return {"pid": j["pid"], "window_id": j["window_id"], "session": j["session"]}


def step(name, tool, args, want):
    """1 手（run.sh step --want）。返り値はその手の cua-driver の応答（JSON・JSON でなければ原文・無ければ None）。"""
    keep_loaded()
    rc, err = run("step", R, name, tool, json.dumps({**args, **base()}, ensure_ascii=False), "--want", want)
    f = f"{R}/steps/{name}.cua.json"
    raw = open(f, errors="replace").read() if os.path.exists(f) else None
    try:
        out = json.loads(raw) if raw else None
    except ValueError:
        out = raw
    shown = {k: out.get(k) for k in ("effect", "summary", "element_count", "degraded_reason") if k in out} if isinstance(out, dict) else (out or "")[:300]
    log(name, "rc", rc, json.dumps(shown, ensure_ascii=False)[:300])
    if rc == 1:
        log("violation → run.sh が止めた"); tally_and_exit(1)
    return out


def read(name, want):
    # 木の walk の予算は契約の 10 秒（drive-w2b-ax.py と同じ）
    return step(name, "get_window_state", {"timeout_ms": 10000, "include_screenshot": False}, want)


def token(out, role, label):
    els = out.get("elements") if isinstance(out, dict) else None
    return next((e["element_token"] for e in els or [] if e.get("role") == role and L.hit(e, label)), None)


def act(name, tool, want, src, out, role, label, **extra):
    """打つ手。src（読みの手）の木に role / label の token が無ければ打たず、理由を SKIPPED に書く。"""
    tok = token(out, role, label)
    if tok:
        return step(name, tool, {"element_token": tok, **extra}, want)
    d = tally.driver_of(out)
    SKIPPED[name] = f"{src} の木に {role}「{label}」の element_token が無い（cua-driver: {d.kind}・{d.reason}・要素 {d.element_count}）: 打っていない"
    log(name, "打たない:", SKIPPED[name])
    return None


if not os.path.isdir(APP):
    log("app が無い:", APP, "（負荷も start も打たない）"); sys.exit(2)
keep_loaded()
env = dict(os.environ, SHIBUBU_QA_SCENE=SCENE)
rc, err = run("start", R, APP, env=env)
log("start", SCENE, "rc", rc, err)
if rc == 6 and json.load(open(R + "/run.json")).get("owner_active"):
    run("stop", R)  # 自分の負荷だけ止める
    tally_and_exit(6)
if rc not in (0, 6):
    run("stop", R); tally_and_exit(1)
out = read("01-home", FIELD)
act("02-click-input", "click", FIELD, "01-home", out, "AXTextField", "goal")
act("03-type-goal", "type_text", FIELD, "01-home", out, "AXTextField", "goal", text=GOAL)
out = read("04-read-typed", BUTTON)
act("05-create", "click", BUTTON, "04-read-typed", out, "AXButton", "create")
read("06-read-page", BUTTON)
rc, err = run("stop", R)
log("stop rc", rc, err)
tally_and_exit(0)
