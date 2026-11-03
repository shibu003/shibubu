#!/usr/bin/env python3
"""cursor_source — D-g の cursor の赤の「出所の分類」（W15-b68）。判定（赤は赤）は変えず、verdict に分類と前後の行を足すだけ。
入力: run.sh step が bin/axprobe --poll で書く <run>/steps/<name>.cursor.jsonl（0.5 秒ごとの {t, x, y, hid: {種類: 最後の入力からの秒}, comb: {…}}）。
分類（位置が MOVE_PT 以上変わった最初の行 i で）: hid = hidSystemState の mouse 系のどれかの idle < FRESH_S（本物の入力）/ posted = hid 側は古く combinedSessionState の mouse 系が < FRESH_S
  （どこかの process が CGEventPost した）/ warp = どちらも古い（event を伴わない移動。process までは追えない）/ unmeasured = poll の行が無い・位置の変化を挟む 2 行が無い。
verdict に足す欄: cursor_source・cursor_move {t, from, to, hid_mouse_idle_s, comb_mouse_idle_s}・cursor_rows（動いた時刻の前後 ROWS_S 秒の行）・warp の時だけ cursor_context {tool, cursor_off, 手の axprobe の pressure}。
使い方: cursor_source.py annotate <verdict.json> <run dir> <手の名前> <tool>   verdict の violations に cursor の動きが在る時だけ書き換える。exit 0（足した / 足す物が無い）。
"""
import json
import os
import sys

MOVE_PT = 0.5  # judge の閾（front-guard.swift の cursor の 0.5 pt）と同じ
FRESH_S = 1.0
ROWS_S = 2.0
MOUSE = ("moved", "left", "right", "other", "ldrag", "rdrag", "odrag", "scroll", "tablet")


def mouse_idle(side):
    """side = {種類: 秒|null} の mouse 系の最小。読めなければ None。"""
    v = [side.get(k) for k in MOUSE if isinstance(side.get(k), (int, float))]
    return min(v) if v else None


def classify(rows):
    """純関数。rows = poll の行の列 -> {"cursor_source", "cursor_move", "cursor_rows"}。"""
    for i in range(1, len(rows)):
        a, b = rows[i - 1], rows[i]
        try:
            moved = ((b["x"] - a["x"]) ** 2 + (b["y"] - a["y"]) ** 2) ** 0.5 >= MOVE_PT
        except (KeyError, TypeError):
            continue
        if not moved:
            continue
        h, c = mouse_idle(b.get("hid") or {}), mouse_idle(b.get("comb") or {})
        src = "hid" if h is not None and h < FRESH_S else ("posted" if c is not None and c < FRESH_S else "warp")
        near = [r for r in rows if abs(r["t"] - b["t"]) <= ROWS_S]
        return {"cursor_source": src, "cursor_move": {"t": b["t"], "from": [a["x"], a["y"]], "to": [b["x"], b["y"]], "hid_mouse_idle_s": h, "comb_mouse_idle_s": c}, "cursor_rows": near}
    return {"cursor_source": "unmeasured"}


def annotate(vf, R, name, tool):
    try:
        v = json.load(open(vf))
    except (OSError, ValueError):
        return 0
    if not any(str(x.get("detail", "")).startswith("cursor") for x in v.get("violations") or []):
        return 0
    try:
        rows = [json.loads(l) for l in open(f"{R}/steps/{name}.cursor.jsonl") if l.strip()]
    except (OSError, ValueError):
        rows = []
    out = classify(rows)
    if out["cursor_source"] == "warp":  # 同じ時刻に自分が打っていた物を並べる（process までは追えない）
        ctx = {"tool": tool}
        for key, path in (("cursor_off", f"{R}/steps/00-cursor-off.cua.json"),):
            try:
                ctx[key] = open(path, encoding="utf-8").read().strip()[:200]
            except OSError:
                ctx[key] = None
        try:
            ctx["pressure"] = json.load(open(f"{R}/steps/{name}.axprobe.json")).get("pressure")
        except (OSError, ValueError):
            ctx["pressure"] = None
        out["cursor_context"] = ctx
    v.update(out)
    open(vf, "w").write(json.dumps(v, ensure_ascii=False, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 6 and sys.argv[1] == "annotate":
        sys.exit(annotate(*sys.argv[2:6]))
    print("usage: cursor_source.py annotate <verdict.json> <run dir> <name> <tool>", file=sys.stderr)
    sys.exit(2)
