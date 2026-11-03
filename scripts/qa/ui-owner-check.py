#!/usr/bin/env python3
"""owner の UI の決定を、code から読める物だけ 1 件ずつ「今の値 / 目標 / 緑になる Wave」で出す（読むだけ・cargo も画面も使わない）。

  python3 -I scripts/qa/ui-owner-check.py --rev yc/ui --wave UI-3        # git の branch / commit の木を読む（worktree に触らない）
  python3 -I scripts/qa/ui-owner-check.py --root <dir> [--wave UI-n] [--json]   # dir の下の src/ docs/ scripts/ を読む（負の対照の fixture もこれ）
  exit 0 = 全部緑 / 1 = その Wave までに緑であるべき物が赤 / 6 = 赤は無いがまだ先の Wave の物が「未」 / 2 = 木が読めない

--wave を省くと全部が期限内（= 最後の Wave の門）・--wave UI-0 は誰も期限前（今の値を写すだけで exit 6）。表の正本 = docs/evidence/UI/owner-check.md（決定の原文 → 検査 → 読む語 → 緑の Wave。id が 1 対 1）。
読む語は今の src の名前から決めた物。UI-n で名前が変わったら、この file の CHECKS と md の表の 2 か所だけを直す。
文言は check_ui_copy.py（sentence case = en-case）、配線は check_wired.py に任せる（ここは token・寸法・部品の数・動き・sidebar）。
人の目でしか確かめられない決定は HUMAN に並べるだけ（数えない）。Rust の読み方は _rs.py（ui-strings.py と check_wired.py と共通）。python3 標準 library だけ。
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _rs import lex, blank_tests, match_close, line_of  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WAVES = ["UI-1", "UI-2", "UI-3", "UI-4a", "UI-4b", "UI-5a", "UI-5b", "UI-6a", "UI-6b", "UI-7", "UI-8"]
ALIAS = {"UI-4": "UI-4b", "UI-5": "UI-5b", "UI-6": "UI-6b"}  # a と b の和（従来の --wave UI-4 / 5 / 6 の後方互換）
TITLE_LINE_FILES = {"header.rs", "titleline.rs", "title_line.rs", "titlebar.rs", "shell.rs"}  # 一言の欄が居てよい file（title line を描く所）
BUTTON_FNS = r"(\w*button\w*|chip|tabs?|tab_\w+)"  # 本体に枠の token を書かない描画 fn（src/ui の全部。名前が button を含むか chip か tab）
# owner 209 通目（UI-SWEEP-1101 追記「settingやconnectionなどの中だけでいいよ。他は元に戻して」）: button の枠の範囲（`theme::framed`）を立てる画面はこの 3 つだけ
FRAME_SCREENS = ("src/ui/settings.rs", "src/ui/connections.rs", "src/ui/runs.rs")
FRAMED_CALL = r"\bframed\("


class Tree:
    """読む木の dir。--rev は git archive で一時 dir に出す（worktree と branch に触らない）。"""

    def __init__(self, root):
        self.root, self._c = root, {}

    def rs(self, *prefixes):
        out = []
        for base, _, names in os.walk(os.path.join(self.root, "src")):
            for n in sorted(names):
                rel = os.path.relpath(os.path.join(base, n), self.root)
                if n.endswith(".rs") and (not prefixes or any(rel.startswith(p) for p in prefixes)):
                    out.append(rel)
        return sorted(out)

    def exists(self, rel):
        return os.path.isfile(os.path.join(self.root, rel))

    def src(self, rel):
        if rel not in self._c:
            try:
                s = open(os.path.join(self.root, rel), encoding="utf-8").read()
            except OSError:
                s = None
            self._c[rel] = (s, *(lex(s) if s is not None else (None, None)))
            m = self._c[rel][1]
            self._c[rel] += (blank_tests(m) if m is not None else None,)
        return self._c[rel][0]

    def code(self, rel):
        """comment・string の中身・#[cfg(test)] を空白にした本文（無ければ None）。"""
        self.src(rel)
        return self._c[rel][3]

    def lits(self, rel):
        self.src(rel)
        return self._c[rel][2] or []


def fn_body(code, name_re):
    """`fn <name>` の { } の中。無ければ None。"""
    m = re.search(r"\bfn\s+" + name_re + r"\b[^{;]*\{", code)
    return code[m.end():match_close(code, m.end() - 1)] if m else None


def const_num(code, name):
    m = re.search(r"\bconst\s+" + name + r"\s*:\s*\w+\s*=\s*([0-9.]+)", code or "")
    return float(m.group(1)) if m else None


def block_body(code, head_re):
    """`<head_re>` の直後の { } の中（struct・enum の variant・fn どれでも）。無ければ None。"""
    m = re.search(head_re + r"\s*\{", code or "")
    return code[m.end():match_close(code, m.end() - 1)] if m else None


def theme_blocks(c):
    """theme.rs の LIGHT / DARK の { } の中 -> {名前: 本文}。"""
    return {m.group(1): c[m.end():match_close(c, m.end() - 1)] for m in re.finditer(r"pub const (LIGHT|DARK): Tokens = Tokens \{", c or "")}


def hits(t, regex, files):
    out = []
    for rel in files:
        c = t.code(rel)
        if c is not None:
            out += [f"{os.path.basename(rel)}:{line_of(c, m.start())}" for m in re.finditer(regex, c)]
    return out


def short(items, n=3):
    return ", ".join(items[:n]) + (f" …+{len(items) - n}" if len(items) > n else "")


def theme(t):
    return t.code("src/ui/theme.rs")


# ---- 表の 1 行 = 1 関数（ok, 今の値）。ok が None = 読めない（赤でも緑でもなく、期限内なら赤として数える） ----

def c_one_field(t):
    """決定 1: 一言の欄（egui の TextEdit の構築）が title line の file に 1 つ。数えるのは構築であって変数名ではない（畳み先が command でも input でも同じ）。
    殻の file = src/ui 直下 + generated.rs。除く = palette.rs（⌘K の query・modal）/ apps.rs（一覧の filter・UI-4 で消える）/ sidebar.rs（session と Project の filter）/ renderer/（利用者の app の中身）/ app.rs（設定・登録の form）。"""
    found = []
    for rel in t.rs("src/ui/", "src/generated.rs"):
        # SESSIONS-LEFT-1101: sidebar.rs の検索は一覧の filter（owner 198 の 5「searchも左sidebar」）= apps.rs と同じく一言の欄に数えない
        if "/renderer/" in rel or os.path.basename(rel) in {"palette.rs", "apps.rs", "sidebar.rs"}:
            continue
        c = t.code(rel) or ""
        # FX3: theme.rs の `fn rename_inline` の中の 1 つは言葉の欄でない浮く部品 ⑦（tab の席の名前の欄・FINAL SPEC §5.0）= 数えない（緩めるのはこの fn の本体だけ）
        m7 = re.search(r"\bfn\s+rename_inline\b[^{;]*\{", c) if os.path.basename(rel) == "theme.rs" else None
        skip = (m7.end(), match_close(c, m7.end() - 1)) if m7 else (-1, -1)
        found += [(os.path.basename(rel), line_of(c, m.start())) for m in re.finditer(r"\bTextEdit::(?:singleline|multiline)\(", c) if not skip[0] <= m.start() < skip[1]]
    where = [f"{f}:{n}" for f, n in found]
    return (len(found) == 1 and found[0][0] in TITLE_LINE_FILES), f"{len(found)} 個 [{short(where, 5)}]"


def c_field_h(t):
    v = const_num(theme(t), "COMMAND_H")
    return (v == 32.0 if v is not None else None), f"COMMAND_H={v}"


# 範囲の中で枠を持つ theme.rs の描画 fn（UI-SWEEP-1101 = owner 196「押せる物は押せると分かる形」→ owner 209「Settings・Connections・Runs の中だけ」）。
# 枠の token（押せる = border_strong・押せない = border）は `frame_of` だけが読み、範囲の外では Stroke::NONE。描画 fn は `frame_of(` を呼ぶ
# （chip は button_sized / button_compact に委ねる）。icon_button（記号そのものが押せる形）と button_menu_item（popup の枠の中の行）は外す。
FRAMED_FNS = {
    "button_sized": r"\bframe_of\(",
    "button_disabled": r"\bframe_of\(",
    "chip": r"\bbutton_(?:sized|compact)\(",
    "tab": r"\bframe_of\(",
    "native_buttons": r"\bframe_of\(",
}


def c_btn_framed(t):
    """範囲（Settings・Connections・Runs）の中の押せる物は枠を持つ: ① `frame_of` の本体が `border_strong` と `border` を読む ② FRAMED_FNS の本体が `frame_of(` を呼ぶ
    （chip は委ねる・comment は数えない）③ FRAME_SCREENS がそれぞれ `framed(` で範囲を立てる。`button_sized` が無ければ測れない。"""
    c = theme(t)
    if c is None or fn_body(c, "button_sized") is None:
        return None, "theme.rs の button_sized が読めない"
    fo = fn_body(c, "frame_of")
    bare = [] if fo is not None and re.search(r"\.border_strong\b", fo) and re.search(r"\.border\b", fo) else ["frame_of"]
    bare += [f for f, want in FRAMED_FNS.items() if (b := fn_body(c, f)) is not None and not re.search(want, b)]
    unset = [os.path.basename(r) for r in FRAME_SCREENS if not re.search(FRAMED_CALL, t.code(r) or "")]
    return not bare and not unset, f"枠の無い描画 fn {len(bare)} [{short(bare, 5)}]・範囲を立てない画面 {len(unset)} [{short(unset)}]"


def c_btn_no_stroke(t):
    """範囲の外の button は線なし（owner 209 = 5 回目の build 50b9b91c の形に戻す）: ① src/ui の button の描画 fn（名前が button を含む・chip・tab）の本体に
    `hairline(`・`.border*` 0（枠は `frame_of` からだけ）② `frame_of` が範囲の印（`framed_id()`）を読み、外では `Stroke::NONE` ③ `framed(` で範囲を立てるのは
    FRAME_SCREENS だけ（app.rs・shell.rs などで立てると全体に枠が出る）。"""
    c = theme(t)
    if c is None:
        return None, "theme.rs が読めない"
    n, names = 0, []
    for rel in t.rs("src/ui/"):
        code = t.code(rel) or ""
        for m in re.finditer(r"\bfn\s+" + BUTTON_FNS + r"\b", code):
            k = len(re.findall(r"\bhairline\(|\.border\w*\b", fn_body(code, m.group(1)) or ""))
            if k:
                names.append(f"{os.path.basename(rel)}:{m.group(1)}×{k}")
            n += k
    fo = fn_body(c, "frame_of")
    guarded = fo is not None and "framed_id()" in fo and "Stroke::NONE" in fo
    wide = [w for rel in t.rs() if rel not in FRAME_SCREENS and rel != "src/ui/theme.rs" for w in hits(t, FRAMED_CALL, [rel])]
    return n == 0 and guarded and not wide, f"描画 fn の枠線の参照 {n} [{short(names, 5)}]・frame_of が範囲の外で NONE={'有' if guarded else '無'}・3 つの画面の外で範囲を立てる {len(wide)} [{short(wide)}]"


def enclosing_open(c, p):
    """位置 p を囲む一番内側の { の位置（無ければ None）。"""
    depth = 0
    for i in range(p - 1, -1, -1):
        if c[i] == "}":
            depth += 1
        elif c[i] == "{":
            if depth == 0:
                return i
            depth -= 1
    return None


def c_native_buttons(t):
    """egui の素の button（`.button(`・`Button::new(`・`.small_button(`・`.menu_button(` の戸口）は Style の入力欄の枠（border_input）を読む。
    native_buttons を呼んだ scope の中（helper と同じ形 = 範囲の中は border_strong・外は線なし）か helper に換える（決定 ⑤・flat_buttons → framed_buttons → owner 209 で native_buttons）。
    menu_button の { } の中は egui が既に枠を外している（数えない）。"""
    if theme(t) is None:
        return None, "theme.rs が読めない"
    n, where = 0, []
    for rel in t.rs("src/ui/", "src/app.rs", "src/generated.rs"):
        c = t.code(rel) or ""
        menus = [(m.end(), match_close(c, m.end() - 1)) for m in re.finditer(r"\.menu_button\(", c)]
        flats = []  # (scope の { , } , native_buttons を呼んだ位置)
        for m in re.finditer(r"\bnative_buttons\(", c):
            o = enclosing_open(c, m.start())
            if o is not None:
                flats.append((o, match_close(c, o), m.start()))
        for m in re.finditer(r"\.(?:small_|menu_)?button\(|\bButton::new\(", c):
            p = m.start()
            if any(a < p < b for a, b in menus) or any(a < p < b and f < p for a, b, f in flats):
                continue
            n += 1
            where.append(f"{os.path.basename(rel)}:{line_of(c, p)}")
    return n == 0, f"native_buttons の scope の外の素の button {n} [{short(where)}]"


def c_btn_fill(t):
    """hover は surface_3・press は surface_press（名前を読む）を button_sized と icon_button の本体の両方に書く。LIGHT / DARK の両方に surface_press の field。
    helper に括り出すと赤（食い違い 3・実装が両方に書く）。"""
    c = theme(t) or ""
    body = fn_body(c, "button_sized")
    icon = fn_body(c, "icon_button")
    if body is None:
        return None, "button_sized が読めない"
    press = re.compile(r"is_pointer_button_down_on|is_pointer_button_down|\.dragged\(\)")
    flags = {"hover surface_3": "surface_3" in body, "press": bool(press.search(body)), "press surface_press": "surface_press" in body,
             "icon hover": "surface_3" in (icon or ""), "icon press": bool(press.search(icon or "")), "icon press surface_press": "surface_press" in (icon or "")}
    blocks = theme_blocks(c)
    flags["surface_press を LIGHT と DARK に"] = len(blocks) == 2 and all(re.search(r"\bsurface_press\s*:", b) for b in blocks.values())
    return all(flags.values()), " / ".join(f"{k}={'有' if v else '無'}" for k, v in flags.items())


def c_btn_size(t):
    h, ch = const_num(theme(t), "BUTTON_H"), const_num(theme(t), "BUTTON_COMPACT_H")
    return ((h, ch) == (28.0, 24.0) if h is not None else None), f"BUTTON_H={h} COMPACT_H={ch}"


def c_btn_inset(t):
    x, cx = const_num(theme(t), "BUTTON_PAD_X"), const_num(theme(t), "BUTTON_COMPACT_PAD_X")
    return ((x, cx) == (8.0, 6.0) if x is not None else None), f"PAD_X={x} COMPACT_PAD_X={cx}"


def c_blue_refs(t):
    h = hits(t, r"\.interactive_(?:primary|hover)\b", t.rs())
    return len(h) == 0, f"interactive_* の参照 {len(h)} [{short(h)}]"


def c_blue_tokens(t):
    c = theme(t)
    if c is None:
        return None, "theme.rs が読めない"
    # src() の masked は literal の中身だけ潰すので、rgb(0x…) の数字は残る
    blue = []
    for m in re.finditer(r"pub const (LIGHT|DARK): Tokens = Tokens \{", c):
        block = c[m.end():match_close(c, m.end() - 1)]
        block = block.split("ansi:")[0]
        for f in re.finditer(r"(\w+):\s*rgb\(0x([0-9A-Fa-f]{6})\)", block):
            if f.group(1) == "working":
                continue
            r, g, b = (int(f.group(2)[i:i + 2], 16) for i in (0, 2, 4))
            if b - r >= 48 and b >= g:
                blue.append(f"{m.group(1)}.{f.group(1)}")
    return len(blue) == 0, f"青い token {len(blue)} [{short(blue, 4)}]"


def c_working_only_state(t):
    """青は working だけ（c_blue_tokens）で、working を使うのは状態の色（src/ui/states.rs）だけ。波形などの飾りに使わない。"""
    h = [x for x in hits(t, r"\.working\b", t.rs()) if not x.startswith("states.rs:")]
    return len(h) == 0, f"states.rs の外の .working の参照 {len(h)} [{short(h)}]"


def c_attach_model(t):
    """決定 6: `struct Goal`（か `Generate { }`）に attachments・`struct CliConfig`（src/llm_cli.rs・起こし方の設定。TerminalConfig は PTY の側）に model。"""
    has_att = any(re.search(r"\battachments\b", block_body(t.code(rel), r"\bstruct\s+Goal\b") or "") or
                  re.search(r"\battachments\b", block_body(t.code(rel), r"\bGenerate") or "") for rel in t.rs("src/"))
    has_model = bool(re.search(r"\bmodel\b", block_body(t.code("src/llm_cli.rs"), r"\bstruct\s+CliConfig\b") or ""))
    return has_att and has_model, f"Goal.attachments={'有' if has_att else '無'} CliConfig.model={'有' if has_model else '無'}"


def c_reduce_fade(t):
    """答え ①: reduce = 短い fade。`enum MotionMode { Full, Fade, Cut }`（ui-6/motion.md §6-2。Cut は試験用の「動かさない」= 0）の
    `dur` の match で Fade の腕が 0 でなく fade の token（REDUCE* / FADE* の const）を使い、Cut の腕は 0。token は 0 < 数 ≤ PAGE。
    位置を動かす fn（SLIDE_PX・HOVER_SCALE・PRESS_SCALE を使う）は本体が mode を見る（`reduce_motion(`・`motion_mode(`・`MotionMode`）。名前が違えば検査を直さず名前を合わせる。"""
    c = t.code("src/ui/motion.rs")
    if c is None:
        return None, "motion.rs が読めない"
    enum = block_body(c, r"\benum\s+MotionMode") or ""
    variants = sorted(set(re.findall(r"\b(Full|Fade|Cut)\b", enum)))
    mode = variants == ["Cut", "Fade", "Full"]
    dur = fn_body(c, "dur") or ""
    consts = [(n, float(v)) for n, v in re.findall(r"\bconst\s+(\w*(?:REDUCE|FADE)\w*)\s*:\s*f32\s*=\s*([0-9.]+)", c)]
    arm = lambda v: (re.search(r"(?:MotionMode::)?" + v + r"\s*=>\s*([^,}]+)", dur) or [None, ""])[1].strip()
    fade, cut = arm("Fade"), arm("Cut")
    zero = r"0(\.0*)?(_?f32)?"
    a = bool(fade) and not re.fullmatch(zero, fade) and any(re.search(r"\b" + n + r"\b", fade) for n, _ in consts)
    z = bool(cut) and bool(re.fullmatch(zero, cut))
    page = const_num(c, "PAGE") or 0.22
    b = any(0 < v <= page for _, v in consts)
    moving = [fn for fn in re.findall(r"\bfn\s+(\w+)", c)
              if re.search(r"SLIDE_PX|HOVER_SCALE|PRESS_SCALE", fn_body(c, fn) or "") and not re.search(r"reduce_motion\(|motion_mode\(|MotionMode", fn_body(c, fn) or "")]
    return mode and a and z and b and not moving, (f"MotionMode={'/'.join(variants) or '無'} Fade の腕={'token' if a else '0 か token 無'} Cut の腕={'0' if z else '0 でない'} "
                                                    f"fade の token={'有' if b else '無'} 位置を動かす fn で mode を見ない={short(moving) or 'なし'}")


def c_ready_silent(t):
    """問題 3 / 状態「Ready は何も出さない」: states.rs の `fn state_line` の本体に Ready の早い抜け（`if state == RegionState::Ready { return … }`・`matches!`・`if let` も可）が在り、
    renderer/mod.rs の `usable_line_in` の本体に `RegionState::Ready.glyph()` の参照が 0（通った席の `● 文` を描かない。AX の Group は残す = 動的の試験の側）。"""
    st = fn_body(t.code("src/ui/states.rs") or "", "state_line")
    us = fn_body(t.code("src/ui/renderer/mod.rs") or "", "usable_line_in")
    if st is None or us is None:
        return None, f"state_line={'有' if st is not None else '読めない'} usable_line_in={'有' if us is not None else '読めない'}"
    early = bool(re.search(r"\bif\s+[^{]*RegionState::Ready[^{]*\{\s*return\b", st))
    glyph = len(re.findall(r"RegionState::Ready\s*\.\s*glyph\(", us))
    return early and glyph == 0, f"state_line の Ready の早い抜け={'有' if early else '無'} usable_line_in の Ready.glyph() の参照 {glyph}"


def c_once_per_pass(t):
    """問題 3「同じ文は 1 回」: `state_line` の本体が `cumulative_pass_nr` を読む（(pass, state, 文) の記録の key）。在る事だけ。同じ文が 1 回になる事は cargo test（terminal_specs 2 つの Page）が持つ。"""
    st = fn_body(t.code("src/ui/states.rs") or "", "state_line")
    if st is None:
        return None, "state_line が読めない"
    n = len(re.findall(r"\bcumulative_pass_nr\b", st))
    return n >= 1, f"state_line の cumulative_pass_nr の参照 {n}"


def c_no_page_partial(t):
    """問題 3「`◐ 一部のみ表示` が Home と全 Page」: `llm_line(` を呼ぶのは Home（app.rs）だけ。generated.rs の `fn draw_page` の本体に 0・app.rs に 1 つ以上。"""
    page = fn_body(t.code("src/generated.rs") or "", "draw_page")
    if page is None:
        return None, "draw_page が読めない"
    in_page = len(re.findall(r"\bllm_line\(", page))
    home = len(re.findall(r"\bllm_line\(", t.code("src/app.rs") or ""))
    return in_page == 0 and home >= 1, f"draw_page の llm_line( {in_page} / app.rs（Home）の llm_line( {home}"


def dur_args(t):
    """`dur(…)` の呼び出し（fn の定義と test を除く）-> [(file:行, 最後の引数)]。"""
    out = []
    for rel in t.rs("src/ui/", "src/app.rs", "src/generated.rs"):
        c = t.code(rel) or ""
        for m in re.finditer(r"(?<!\bfn )\bdur\(", c):
            args = c[m.end():match_close(c, m.end() - 1)]
            out.append((f"{os.path.basename(rel)}:{line_of(c, m.start())}", args.rsplit(",", 1)[-1].strip()))
    return out


def c_duration_token(t):
    """motion.md §3「時間は token」: `dur(` の時間の引数に数の literal が無い（const か変数）。motion の長さは motion.rs の const 1 か所で決める。"""
    bad = [w for w, a in dur_args(t) if re.search(r"(?<![\w.])\d+(?:\.\d+)?", a)]
    return len(bad) == 0, f"dur( に数の literal {len(bad)} [{short(bad)}]（呼び出し {len(dur_args(t))}）"


def c_timer_not_dur(t):
    """motion.md §2「timer は dur を通さない」: `HOLD_*` / `DELAY_*` を `dur(` に渡さない（渡すと reduce の fade 120 ms に潰れて 4 秒の受け取りが消える）。"""
    bad = [w for w, a in dur_args(t) if re.search(r"\b(?:HOLD|DELAY)_\w+", a)]
    return len(bad) == 0, f"dur( に HOLD_* / DELAY_* {len(bad)} [{short(bad)}]"


def c_exec_strip_gone(t):
    """「下の実行の帯を消す」: `execution_strip` の参照（fn の定義ごと）が非 test の src で 0。"""
    h = hits(t, r"\bexecution_strip\b", t.rs())
    return len(h) == 0, f"execution_strip の参照 {len(h)} [{short(h)}]"


def c_no_bounce(t):
    """DESIGN §9「視差・bounce・自律的な動き禁止」: src/ui・app.rs・generated.rs の識別子に bounce / parallax / spring / elastic / ease_in_out_back / back_out が 0（debounce は別の語として除く）。"""
    h = hits(t, r"(?i)(?<![a-z])(?:bounce|parallax|spring|elastic|ease_in_out_back|back_out)(?![a-z])", t.rs("src/ui/", "src/app.rs", "src/generated.rs"))
    return len(h) == 0, f"bounce / 視差の語 {len(h)} [{short(h)}]"


def c_sidebar_key(t):
    """⌃⌘S = 同じ `consume_key(` の引数の中に Key::S と ctrl と（mac_cmd か command）。隣の文の ctrl を拾わない。"""
    for rel in t.rs("src/ui/", "src/app.rs"):
        c = t.code(rel) or ""
        for m in re.finditer(r"\bconsume_key\(", c):
            args = c[m.end():match_close(c, m.end() - 1)]
            w = args.lower()
            if re.search(r"Key::S\b", args) and "ctrl" in w and ("mac_cmd" in w or "command" in w):
                return True, f"⌃⌘S: {os.path.basename(rel)}:{line_of(c, m.start())}"
    return False, "consume_key( の引数に Key::S と ctrl + command の組が無い"


def c_sidebar_saved(t):
    """答え ②: 畳んだ状態は窓全体の設定 UiSettings（model.rs・rail_pinned の先例）に持つ。derive(Serialize, Deserialize) が save / load の両方を通す（#[serde(skip)] は保存されないので不可）。"""
    for rel in t.rs("src/"):
        c = t.code(rel) or ""
        m = re.search(r"((?:#\[[^\]]*\]\s*)*)(?:pub\s+)?\bstruct\s+UiSettings\s*\{", c)
        if not m:
            continue
        body = c[m.end():match_close(c, m.end() - 1)]
        f = re.search(r"((?:#\[[^\]]*\]\s*)*)(?:pub\s+)?(sidebar_(?:collapsed|hidden|open))\b", body)
        derive = m.group(1)
        both = "Serialize" in derive and "Deserialize" in derive
        ok = bool(f) and both and "skip" not in f.group(1)
        return ok, f"{os.path.basename(rel)} の UiSettings." + (f"{f.group(2)} 有・derive Serialize/Deserialize={'有' if both else '無'}・serde(skip)={'有' if 'skip' in f.group(1) else '無'}" if f else "sidebar の欄が無い")
    return False, "struct UiSettings が無い"


def c_sidebar_narrow(t):
    """`Tier { … }` の塊（struct の定義・impl Tier・for_width の literal）の中だけを読む。同じ file の `Shell { sidebar_open: true, … }` を拾わない（UI-4b の負の対照で false green だった）。"""
    for rel in t.rs("src/ui/"):
        c = t.code(rel) or ""
        if re.search(r"\bstruct\s+Tier\b", c):
            blocks = " ".join(c[m.end():match_close(c, m.end() - 1)] for m in re.finditer(r"\bTier\s*\{", c))
            vals = set(re.findall(r"\bsidebar_(?:collapsed|hidden|open)\s*:\s*(true|false)", blocks))
            return vals == {"true", "false"}, f"{os.path.basename(rel)} の Tier の畳み既定 = {sorted(vals) or '欄なし'}"
    return None, "struct Tier が読めない"


def c_no_apps_view(t):
    enum = ""
    for rel in t.rs("src/ui/"):
        c = t.code(rel) or ""
        m = re.search(r"\benum\s+View\s*\{", c)
        if m:
            enum = c[m.end():match_close(c, m.end() - 1)]
    variant = bool(re.search(r"^\s*Apps\b", enum, re.M))
    refs = hits(t, r"View::Apps\b", t.rs())
    return (not variant and not refs), f"View::Apps variant={'有' if variant else '無'} 参照 {len(refs)} [{short(refs)}]"


def button_calls(t, rel):
    """非 test の本文の button の戸口（名前が button を含む fn・chip・Button::new）-> [(戸口の位置, 引数の原文)]。位置は code と src で共通。"""
    c, s = t.code(rel) or "", t.src(rel) or ""
    return [(m.start(), s[m.end():match_close(c, m.end() - 1)]) for m in re.finditer(r"\b(?:\w*button\w*|chip)\s*\(|\bButton::new\(", c)]


def c_confirm_inline(t):
    """答え ③: 消す確認は同じ場所の 1 行。(1) 消す確認の Window/Modal 0（名前に削除/消す/Delete/Remove）(2) Delete / 消す の button と Keep / やめる の button が**同じ block（{ }）の中**に在る
    （部品の中の button だけを数える。home.rs の例文 "Keep research notes" のような関係ない literal では立たない）。"""
    modal, pair = [], []
    for rel in t.rs():
        c = t.code(rel) or ""
        s = t.src(rel) or ""
        for m in re.finditer(r"\b(?:Window|Modal)::new\(", c):
            call = s[m.start():match_close(c, m.end() - 1) + 1]
            if re.search(r"削除|消す|[Dd]elete|[Rr]emove", call):
                modal.append(f"{os.path.basename(rel)}:{line_of(c, m.start())}")
        btns = button_calls(t, rel)
        for p, args in btns:
            o = enclosing_open(c, p) if re.match(r'[^"]*"(?:Delete|消す)\b', args) else None
            if o is not None:
                e = match_close(c, o)
                if any(o < q < e and re.match(r'[^"]*"(?:Keep|やめる)\b', a2) for q, a2 in btns):
                    pair.append(f"{os.path.basename(rel)}:{line_of(c, p)}")
    return (not modal and bool(pair)), f"消す確認の Window/Modal {len(modal)} [{short(modal)}] 同じ block の中の Delete+Keep の button {len(pair)} [{short(pair)}]"


def c_sentence_case(t):
    """check_ui_copy.py の en-case（EN_RULES）に任せる。木の中で --strict --set ui-1 を打つ（本体が NotImplementedError の間は未測 = 赤として数える）。"""
    if not t.exists("scripts/check_ui_copy.py"):
        return None, "scripts/check_ui_copy.py が無い"
    p = subprocess.run([sys.executable, "-I", "scripts/check_ui_copy.py", "--strict", "--set", "ui-1"], cwd=t.root, capture_output=True, text=True)
    out = (p.stdout + p.stderr).strip().splitlines()
    if "NotImplementedError" in p.stderr:
        return False, "check_ui_copy の英語の規則の本体がまだ無い（NotImplementedError）"
    return p.returncode == 0, f"check_ui_copy --strict --set ui-1 → rc {p.returncode}" + (f"（{out[-1][:80]}）" if p.returncode and out else "")


def c_min_font(t):
    c = theme(t)
    body = fn_body(c or "", "size")
    sizes = [float(x) for x in re.findall(r"=>\s*([0-9.]+)", body or "")]
    small = [f"Type={x}" for x in sizes if x < 12]
    for rel in t.rs("src/ui/", "src/app.rs", "src/generated.rs"):
        cc = t.code(rel) or ""
        small += [f"{os.path.basename(rel)}:{line_of(cc, m.start())}={m.group(1)}"
                  for m in re.finditer(r"FontId::(?:proportional|monospace|new)\(\s*([0-9.]+)", cc) if float(m.group(1)) < 12]
    return (c is not None and not small), f"12 未満の文字 {len(small)} [{short(small)}]"


def c_card_tests(t):
    a = "fn nested_cards" in (t.code("tests/renderer.rs") or "") if t.exists("tests/renderer.rs") else False
    b = "not_a_card" in (t.src("tests/ui_portal.rs") or "")
    return a and b, f"card の入れ子の試験 renderer.rs={'有' if a else '無'} ui_portal.rs={'有' if b else '無'}（走らせるのは cargo test・ここは在る事だけ）"


# id, 決定の出所, 緑になる Wave, 目標, 関数。docs/evidence/UI/owner-check.md の表と id が 1 対 1（試験が見る）。
CHECKS = [
    ("d1_one_field", "決定 1", "UI-4a", "一言の欄の構築 1 個・title line の file", c_one_field),
    ("d1_field_h", "決定 6", "UI-4a", "COMMAND_H = 32", c_field_h),
    ("d1_no_apps_view", "決定 1", "UI-4a", "View::Apps の variant と参照 0", c_no_apps_view),
    ("d2_btn_no_stroke", "決定 2（owner 209 で範囲の外に戻す）", "UI-3", "範囲の外の button は線なし: 描画 fn に枠線の参照 0・frame_of が外で NONE・3 つの画面の外で framed を立てない", c_btn_no_stroke),
    ("d2_btn_framed", "決定 2（owner 196 → 209 で範囲の中だけ）", "UI-3", "Settings・Connections・Runs の中の押せる物は枠: frame_of が枠の token を読み・描画 fn が frame_of を呼び・3 つの画面が framed を立てる", c_btn_framed),
    ("d2_btn_fill", "決定 2", "UI-3", "hover surface_3・press surface_press が button と icon_button の本体に在る", c_btn_fill),
    ("d2_native_buttons", "決定 2・5", "UI-3", "素の button は native_buttons の scope か helper（menu の中を除く）", c_native_buttons),
    ("d5_btn_size", "決定 5", "UI-3", "BUTTON_H 28 / COMPACT_H 24", c_btn_size),
    ("d5_btn_inset", "決定 5", "UI-3", "PAD_X 8 / COMPACT_PAD_X 6", c_btn_inset),
    ("d4_blue_refs", "決定 4・5", "UI-3", "interactive_primary / hover の参照 0", c_blue_refs),
    ("d4_blue_tokens", "決定 4・5", "UI-3", "working 以外の token が青くない", c_blue_tokens),
    ("d4_working_only_state", "決定 4・5", "UI-3", "working を使うのは states.rs の状態の色だけ", c_working_only_state),
    ("d6_attach_model", "決定 6", "UI-8", "Goal.attachments と CliConfig.model", c_attach_model),
    ("a1_reduce_fade", "答え ①", "UI-6a", "reduce = 短い fade・位置と大きさは動かさない", c_reduce_fade),
    ("a6_ready_silent", "問題 3", "UI-6a", "state_line が Ready を描かない・usable_line_in が Ready の glyph を持たない", c_ready_silent),
    ("a6_once_per_pass", "問題 3", "UI-6a", "state_line が cumulative_pass_nr で同じ文を 1 pass に 1 回にする", c_once_per_pass),
    ("a6_no_page_partial", "問題 3", "UI-6a", "llm_line は Home（app.rs）だけ・draw_page に 0", c_no_page_partial),
    ("a6_duration_token", "motion.md §3", "UI-6a", "dur( の時間は const か変数（数の literal 0）", c_duration_token),
    ("a6_timer_not_dur", "motion.md §2", "UI-6a", "HOLD_* / DELAY_* を dur( に渡さない", c_timer_not_dur),
    ("a6_exec_strip_gone", "UI-6 の作る物", "UI-6b", "execution_strip の参照 0（下の実行の帯を消す）", c_exec_strip_gone),
    ("a6_no_bounce", "DESIGN §9", "UI-6a", "bounce / 視差 / spring の語 0", c_no_bounce),
    ("a2_sidebar_key", "答え ②", "UI-4b", "⌃⌘S で sidebar を畳む / 戻す", c_sidebar_key),
    ("a2_sidebar_saved", "答え ②", "UI-4b", "畳んだ状態を UiSettings に保存（sidebar_collapsed|hidden|open・save/load は derive）", c_sidebar_saved),
    ("a2_sidebar_narrow", "答え ②", "UI-4b", "狭い窓は既定で畳む（Tier の欄が幅で変わる・名前は UI-4b で決まる）", c_sidebar_narrow),
    ("a3_confirm_inline", "答え ③", "UI-5a", "消す確認の Window/Modal 0・Delete / Keep の行", c_confirm_inline),
    ("a4_sentence_case", "答え ④", "UI-1", "check_ui_copy の en-case が ui-1 で exit 0", c_sentence_case),
    ("g_min_font", "DESIGN", "UI-1", "12px 未満の文字 0", c_min_font),
    ("g_card_tests", "DESIGN §3", "UI-1", "card の入れ子 0 の試験が在る", c_card_tests),
]

HUMAN = [
    ("h_minimal", "「ミニマム」「無駄な card・一言を足さない」の見た目", "写真（workflow の反証の段）"),
    ("h_motion_feel", "「演出」「何がどこへ動いたかが分かる」", "動画（反証の段）"),
    ("h_primary_one", "primary の塗り button は 1 画面に 1 つ", "画面ごと（static で読めない）"),
    ("h_reach_when_folded", "sidebar を畳んでも Home と Projects に title line と ⌘K から行ける", "実機（computer use）"),
    ("h_hover_gray", "黒い字の button は hover で灰の地・押している間は濃い灰", "写真（hover と press）"),
    ("h_attach_dnd", "入力欄の + と drag & drop の添付が chip で見え × で外せる", "実機（UI-8）"),
]


def run(t, wave):
    wave = ALIAS.get(wave, wave)
    cur = -1 if wave == "UI-0" else WAVES.index(wave) if wave else len(WAVES) - 1
    rows = []
    for cid, src, due, target, fn in CHECKS:
        ok, val = fn(t)
        if ok:
            st = "green"
        elif WAVES.index(due) <= cur:
            st = "red"
        else:
            st = f"pending({due})"
        rows.append({"id": cid, "decision": src, "wave": due, "status": st, "now": val, "target": target})
    return rows


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--rev", help="git の branch / commit（yc/ui）")
    g.add_argument("--root", help="src/ を持つ dir")
    ap.add_argument("--wave", choices=["UI-0"] + WAVES + list(ALIAS), help="この Wave の門（UI-4a / 4b / 5a / 5b / 6a / 6b。UI-4 / 5 / 6 = a ∪ b）。UI-0 = どれも期限前（今の値の写しだけ）。省く = 全部が期限内")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    tmp = None
    if a.rev:
        tmp = tempfile.TemporaryDirectory(prefix="ui-owner-check.")
        want = ["src", "tests", "scripts", "docs/ui-copy", "docs/ui-copy-rules.json", "docs/ui-copy-rules.md", "docs/examples"]  # check_ui_copy.py が読む物も一緒に（rev に無い path は外す）
        have = subprocess.run(["git", "-C", REPO, "ls-tree", "--name-only", a.rev, "--", *want], capture_output=True, text=True).stdout.split()
        ar = subprocess.run(["git", "-C", REPO, "archive", a.rev, *have], capture_output=True)
        if ar.returncode or subprocess.run(["tar", "-x", "-C", tmp.name], input=ar.stdout).returncode:
            print(f"ui-owner-check: {a.rev} の木が読めない: {ar.stderr.decode()[:200]}", file=sys.stderr)
            return 2
        root = tmp.name
    else:
        root = a.root
    if not os.path.isdir(os.path.join(root, "src")):
        print(f"ui-owner-check: {root}/src が無い", file=sys.stderr)
        return 2
    rows = run(Tree(root), a.wave)
    red = [r for r in rows if r["status"] == "red"]
    pend = [r for r in rows if r["status"].startswith("pending")]
    green = [r for r in rows if r["status"] == "green"]
    rc = 1 if red else 6 if pend else 0
    if a.json:
        print(json.dumps({"wave": a.wave, "rows": rows, "human": [h[0] for h in HUMAN], "green": len(green), "red": len(red), "pending": len(pend), "exit": rc}, ensure_ascii=False, indent=1))
    else:
        mark = {"green": "緑", "red": "赤"}
        for r in rows:
            print(f"{mark.get(r['status'], '未(' + r['wave'] + ')'):>8}  {r['id']:<18} {r['decision']:<9} [{r['wave']}]  今: {r['now']}  /  目標: {r['target']}")
        for hid, what, how in HUMAN:
            print(f"{'人の目':>8}  {hid:<18} {what}（{how}）")
        print(f"ui-owner-check: wave={a.wave or '全部'} 緑 {len(green)} / 赤 {len(red)} / 未 {len(pend)} / 人の目 {len(HUMAN)} → exit {rc}")
    if tmp:
        tmp.cleanup()
    return rc


if __name__ == "__main__":
    sys.exit(main())
