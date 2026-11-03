#!/usr/bin/env python3
"""oracle/a1 — W16-2 の A1（人の編集が Page に重ねた本物の窓の中で起きて disk に残った）を、試験の側の道だけで 1 件にする。
製品の自己申告（surface-status・overlay.jsonl）は照合の相手で、それだけでは数えない（YC_WAVES §5.2 W16-2・record.schema.json の $defs.a1）。

呼ぶ側の使い方（run.sh の手の外で、試験の道具が呼ぶ）:
  S=$(python3 -I oracle/a1.py sha <repo>/README.md)                         # 編集の手の前の disk の sha（読めなければ "-"）
  bin/windows sample --pid <TextEdit> --pid <Shibubu> --out R/oracle/a1.samples.jsonl --stop R/a1.stop &   # 手の始め〜保存の終わり
  …編集の手・保存の手… ; touch R/a1.stop
  python3 -I oracle/a1.py judge --file <repo>/README.md --rel README.md --nonce <N> --sha-before "$S" \\
      --samples R/oracle/a1.samples.jsonl --t0 <手の始め> --t1 <保存の終わり> \\
      --edited-pid <pid> --edited-window <CG の窓番号> --shibubu-pid <pid> \\
      (--frame x,y,w,h | --ax R/ax/<木>.json [--frame-id surface-frame:docs]) --overlay R/data/overlay.jsonl --surface docs \\
      --out R/oracle/a1.json
保証する: 読むだけ（file・窓・app に書かない）。中身は has_nonce だけ記録し、文書の本文を evidence に書かない。
  route_ok = 標本が 1 つ以上・外れ 0・edited と frame が在る。外れは標本ごとに最初の 1 つ（no_window → offscreen → bounds → not_above_shibubu）。
  「Shibubu の窓の直上」= 編集した窓より後ろ（z が大きい）で、それと面積が重なる layer 0 の on screen の窓のうち、一番前が Shibubu の pid。
  overlay_match = t1 までの overlay.jsonl のその面の最後の event が attached か host_shown で、pid と window_id が edited と同じ。
保証しない: 編集した窓の特定（呼ぶ側が --edited-window で渡す・cua-driver の窓番号か CG の名前）。frame の正しさ（--ax の木の bounds をそのまま使う）。
exit: 0 書いた（合否は中の値）/ 2 使い方 / 4 読めない入力
"""
import argparse
import hashlib
import json
import os
import sys
import time


def sha(path):
    try:
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None


def rect(s):
    x, y, w, h = (float(v) for v in s.split(","))
    return {"x": x, "y": y, "w": w, "h": h}


def frame_from_ax(path, ident):
    """cua-driver get_window_state の木（elements）から identifier / label が ident の要素の frame（global の point）。"""
    with open(path, encoding="utf-8") as f:
        d = json.load(f)
    for e in d.get("elements") or []:
        if ident in (e.get("identifier"), e.get("label"), e.get("ax_identifier")) and e.get("frame"):
            fr = e["frame"]
            return {"x": float(fr["x"]), "y": float(fr["y"]), "w": float(fr["w"]), "h": float(fr["h"])}
    return None


def overlaps(a, b):
    return min(a["x"] + a["w"], b["x"] + b["w"]) - max(a["x"], b["x"]) > 0 and min(a["y"] + a["h"], b["y"] + b["h"]) - max(a["y"], b["y"]) > 0


def miss(sample, edited, shibubu_pid, frame):
    """1 つの標本の外れの理由（無ければ None）。"""
    ws = sample.get("windows") or []
    w = next((x for x in ws if x["window_id"] == edited["window_id"] and x["pid"] == edited["pid"]), None)
    if w is None:
        return "no_window"
    if not w.get("onscreen") or w.get("z", -1) < 0:
        return "offscreen"
    b = w["bounds"]
    if any(abs(b[k] - frame[k]) > 1.0 for k in "xywh"):
        return "bounds"
    behind = sorted((x for x in ws if x.get("layer") == 0 and x.get("onscreen") and x.get("z", -1) > w["z"] and overlaps(x["bounds"], b)), key=lambda x: x["z"])
    if not behind or behind[0]["pid"] != shibubu_pid:
        return "not_above_shibubu"
    return None


def overlay_match(path, surface, t1, edited):
    if edited is None or not path or not os.path.exists(path):
        return False
    last = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            if r.get("surface_id") == surface and r.get("at_ms", 0) / 1000.0 <= t1:
                last = r
    return bool(last) and last["event"] in ("attached", "host_shown") and last.get("pid") == edited["pid"] and last.get("window_id") == edited["window_id"]


def judge(a):
    edited = {"pid": a.edited_pid, "window_id": a.edited_window} if a.edited_pid and a.edited_window else None
    frame = rect(a.frame) if a.frame else (frame_from_ax(a.ax, a.frame_id) if a.ax else None)
    samples, misses = 0, []
    with open(a.samples, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            s = json.loads(line)
            if s.get("schema") != "shibubu.qa.windows/1":
                raise ValueError("知らない schema: %s" % s.get("schema"))
            if not (a.t0 <= s["t"] <= a.t1):
                continue
            samples += 1
            why = "no_window" if edited is None or frame is None else miss(s, edited, a.shibubu_pid, frame)
            if why:
                misses.append({"t": s["t"], "why": why})
    before = None if a.sha_before in ("", "-") else a.sha_before
    after = sha(a.file)
    try:
        with open(a.file, encoding="utf-8", errors="replace") as f:
            has_nonce = bool(a.nonce) and a.nonce in f.read()
    except OSError:
        has_nonce = False
    return {
        "schema": "shibubu.qa.a1/1", "t": time.time(), "file": a.rel, "nonce": a.nonce,
        "sha_before": before, "sha_after": after, "sha_changed": before is not None and after is not None and before != after,
        "has_nonce": has_nonce, "edited": edited, "shibubu_pid": a.shibubu_pid, "frame": frame,
        "samples": samples, "misses": misses,
        "route_ok": samples > 0 and not misses and edited is not None and frame is not None,
        "overlay_match": overlay_match(a.overlay, a.surface, a.t1, edited),
    }


def main(argv):
    p = argparse.ArgumentParser(prog="oracle/a1.py")
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sha")
    s.add_argument("file")
    j = sub.add_parser("judge")
    for k in ("--file", "--rel", "--nonce", "--sha-before", "--samples", "--out"):
        j.add_argument(k, required=True)
    for k in ("--t0", "--t1"):
        j.add_argument(k, type=float, required=True)
    j.add_argument("--edited-pid", type=int)
    j.add_argument("--edited-window", type=int)
    j.add_argument("--shibubu-pid", type=int, required=True)
    j.add_argument("--frame")
    j.add_argument("--ax")
    j.add_argument("--frame-id", default="surface-frame:docs")
    j.add_argument("--overlay")
    j.add_argument("--surface", default="docs")
    try:
        a = p.parse_args(argv)
    except SystemExit:
        return 2
    if a.cmd == "sha":
        print(sha(a.file) or "-")
        return 0
    try:
        d = judge(a)
    except (OSError, ValueError, KeyError) as e:
        sys.stderr.write("a1: %s\n" % e)
        return 4
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(json.dumps(d, ensure_ascii=False, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
