#!/usr/bin/env python3
"""owner_idle — owner が最後に「人として」入力してからの秒を返す 1 か所の読み（W15-b42）。
run.sh の start の門・quiet-window.sh・gfinal-bg.sh・tally.py（owner_returned / no_focus）が全部ここを通る。

なぜ HIDIdleTime だけでは足りないか: cua-driver の type_text は key event を HID に post する → HIDIdleTime が 0 に戻り、
次の run の start が「owner が戻った」と読んで止まる（窓が 5 分で閉じた原因・docs/evidence/W15/log.md W15-b42）。click・get_window_state は戻さない。

読み方: effective_idle = min(mouse の idle, key の idle)。key の最後の入力が「自分の key post（type_text / press_key の手の t0〜t1）の ±SLACK_S」に
入っていれば key は無かった事にする（自分の打鍵）。それより後の key（外付け keyboard で先に打つ人）は数える。
HIDIdleTime も同じ除き方で数える（mouse / key 以外の種類の入力を落とさない・mouse / key の欄が無い古い記録の互換）。

使い方（CLI）: owner_idle.py [--steps steps.jsonl ...]   → axprobe --idle-only を 1 回読んで有効な idle 秒を 1 行。読めなければ空行。
  環境: AXPROBE（bin/axprobe の path）・OWNER_IDLE_JSON（検査用・axprobe の代わりの JSON）・OWNER_NOW（検査用の現在の epoch）
"""
import json
import os
import subprocess
import sys
import time

SLACK_S = 2.0
KEY_TOOLS = ("type_text", "press_key")
HERE = os.path.dirname(os.path.abspath(__file__))


def own_key_posts(steps_paths, now, horizon_s=3600):
    """steps.jsonl（run.sh が 1 手 1 行で書く）から自分の key post の (t0, t1)。horizon_s より古い file は読まない。"""
    out = []
    for p in steps_paths:
        try:
            if time.time() - os.path.getmtime(p) > horizon_s:
                continue
            for line in open(p):
                d = json.loads(line)
                if d.get("tool") in KEY_TOOLS:
                    out.append((d["t0"], d["t1"]))
        except (OSError, ValueError, KeyError):
            continue
    return out


def effective_idle(sig, now, own_posts):
    """sig = {hid_idle_s, mouse_idle_s, key_idle_s}（axprobe の pressure と同じ鍵）→ 秒か None（全部読めない）。
    now は sig を読んだ時刻（epoch）。pointer は常に数え、key と HIDIdleTime（mouse / key 以外の種類も含む）は、最後の入力が自分の post の窓に入る時だけ除く。"""
    num = lambda k: sig.get(k) if isinstance(sig.get(k), (int, float)) else None
    mine = lambda idle: any(t0 - SLACK_S <= now - idle <= t1 + SLACK_S for t0, t1 in own_posts)
    terms = [v for v in (num("mouse_idle_s"),) if v is not None]
    terms += [v for v in (num("key_idle_s"), num("hid_idle_s")) if v is not None and not mine(v)]
    if terms:
        return min(terms)
    return float("inf") if any(num(k) is not None for k in ("mouse_idle_s", "key_idle_s", "hid_idle_s")) else None  # 人の入力が見えない = 長く不在


def read_signal():
    raw = os.environ.get("OWNER_IDLE_JSON")
    if raw is None:
        try:
            raw = subprocess.run([os.environ.get("AXPROBE", HERE + "/bin/axprobe"), "--idle-only"], capture_output=True, text=True, timeout=10).stdout
        except (OSError, subprocess.SubprocessError):
            return None
    try:
        return json.loads(raw)
    except ValueError:
        return None


def main(argv):
    steps = []
    if argv[:1] == ["--steps"]:
        steps = argv[1:]
    now = float(os.environ.get("OWNER_NOW") or time.time())
    sig = read_signal()
    idle = effective_idle(sig, now, own_key_posts(steps, now)) if sig else None
    print("" if idle is None else ("999999" if idle == float("inf") else f"{idle:.1f}"))


if __name__ == "__main__":
    main(sys.argv[1:])
