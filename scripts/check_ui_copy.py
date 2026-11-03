#!/usr/bin/env python3
"""余計なキャッチコピー 0 の lint（DESIGN.md §15.2-3・4・5(a)）。規則・allowlist・切替の手順は docs/ui-copy-rules.md。

  python3 -I scripts/check_ui_copy.py             # --strict と同じ。赤が 1 件でもあれば exit 1
  python3 -I scripts/check_ui_copy.py --report    # 一覧だけ出して exit 0（文言を消す wave が終わるまで lint.sh はこれ）
  python3 -I scripts/check_ui_copy.py --self-test # tests/fixtures/ui_copy/{ok,ng}.txt で規則を試す
  --disable <rule>（何度でも）で規則を 1 つ外す（負の対照）。

UI-1（Wave: UI-1・YC_WAVES §5.6 の UI-1 の Done 3）:
  python3 -I scripts/check_ui_copy.py --strict --set ui-1   # 英語の規則（EN_RULES）と日本語の規則を UI-1 の集合で。赤が 1 件でもあれば exit 1
  --set all|ui-1|ui-2  見る file の集合（scripts/qa/ui-strings.py の SETS と同じ名前と中身。ui-1 = src/ui/**〈apps.rs を除く〉と src/app.rs・
      ui-2 = TARGETS 全部〈apps.rs を除く・UI-2 の Done 1〜3〉・JSON は見ない）。
      渡さなければ今まで通り（下の「見る物」・JSON を含む）。
  英語の文の入り口: 日本語の literal は今まで通り rust_items()。英語の literal は scripts/qa/ui-strings.py の scan()（lang=en の行・
      種類 button / label / placeholder / tooltip / status / error / menu を分ける字句）を importlib で借りる（lexer を 2 つ持たない）。
      種類 text の英語の行には英語の規則を当てない（id・key・format の雛形が混ざるので）。`fn label` の腕は label・`fn wording` の腕は status（UI-1 の本体で ui-strings の classify に足す）。
  英語の規則の id（--disable にも使う）: en-case・en-period・en-word・en-status-long（EN_RULES の説明が正）。
  EN_RULES は RULES に入っている（--disable en-* が使える）。judge() は日本語を含む文には日本語の規則だけ、含まない文には英語の規則だけを当てる。
  self-test は tests/fixtures/ui_copy/{ok,ng}.en.txt も読む（ng の各行はその規則 1 つで赤・規則を外すと緑）。
  en-period と en-status-long は狭めない（holdout の直し・YC_WAVES §5.6 の UI-1 の口のとおり）: button・label・状態・placeholder・menu の文は長さに関わらず
  末尾に句点を置かない（文の間の `. ` はよい）。状態の 1 行は全体の語を数えて 8 語以下。表の EN がこれに合わない文は docs/ui-copy-rules.md §3 の
  allowlist に sendback の番号つきで入れる（表は直さない・食い違いは allowlist の 1 表に見える）。

見る物: src/ui/**/*.rs・src/app.rs・src/generated.rs・src/surface/*.rs・src/project/*.rs の日本語を含む文字列 literal、docs/examples/**/*.json の text 系の値。
python3 標準 library だけ。
"""
import argparse
import fnmatch
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RULES_DOC = os.path.join(ROOT, "docs", "ui-copy-rules.md")
FIXTURES = os.path.join(ROOT, "tests", "fixtures", "ui_copy")

JA = re.compile(r"[぀-ヿ一-鿿]")
HOLE = re.compile(r"\{[^{}]*\}")  # format! の穴
EXAMPLE = re.compile(r"^例\s*[:：]")

# 語の表は docs/ui-copy-rules.json の 1 つ（src/refine/mod.rs も同じ file を読む）。ここは表から regex を組むだけ
with open(os.path.join(ROOT, "docs", "ui-copy-rules.json"), encoding="utf-8") as _f:
    TABLE = json.load(_f)


def _alt(words):
    return "|".join(map(re.escape, words))


def _chars(words):
    return "[" + "".join(map(re.escape, words)) + "]"


# 規則 id → (理由, 判定)。判定は (kind, text) → bool。kind = text / button / label / placeholder / hero / lead
DEKIMASU = re.compile(f"({_alt(TABLE['dekimasu_endings'])}){_chars(TABLE['dekimasu_trailing'])}?$")
AD_WORDS = TABLE["ad_words"]
EXCLAIM = re.compile(_chars(TABLE["exclaim"]))
SENT_END = re.compile(_chars(TABLE["sentence_ends"]))
STATE_END = re.compile(f"({_alt(TABLE['state_endings'])}|{re.escape(TABLE['state_prefix'])}\\w*)$")
FACT_END = re.compile(f"({_alt(TABLE['fact_endings'])})$")
NAME_MAX, NEXT_MAX, HERO_MAX, LEAD_MIN = (TABLE[k] for k in ("name_max_width", "next_step_max_width", "hero_max_width", "lead_min_width"))


def width(s):
    """字数。format の穴を除き、ASCII は半字。"""
    s = HOLE.sub("", s).strip()
    return sum(0.5 if ord(c) < 128 else 1 for c in s)


def sentences(s):
    return [p.strip() for p in SENT_END.split(s) if p.strip()]


def multi(kind, s):
    parts = sentences(s)
    if len(parts) < 2:
        return False
    # 許す 2 文: 状態・理由の事実 + 次の一手（「まだやる事がありません。「追加する」で 1 つ目を書く」型）
    if len(parts) == 2 and STATE_END.search(parts[0]) and width(parts[1]) <= NEXT_MAX and not DEKIMASU.search(parts[1]):
        return False
    return True


RULES = {
    "dekimasu": ("「〜できます / 〜られます / 頼めます」型の説明", lambda k, s: bool(DEKIMASU.search(s))),
    "ad-word": ("宣伝の語（" + "・".join(AD_WORDS) + "）", lambda k, s: any(w in s for w in AD_WORDS)),
    "exclaim": ("感嘆符", lambda k, s: bool(EXCLAIM.search(s))),
    "multi": ("2 文以上の説明", multi),
    "hero": ("hero の文（Display の大きさで問い以外）", lambda k, s: k == "hero" and not re.search(r"[？?]$", s) and width(s) > HERO_MAX),
    "lead": ("見出しの下の説明文", lambda k, s: k == "lead" and width(s) >= LEAD_MIN and not FACT_END.search(s.rstrip("。"))),
    "long-name": (f"button / label の名前が長い（{NAME_MAX} 字超）", lambda k, s: k in ("button", "label") and width(s) > NAME_MAX),
}
# 「例: 」で始まる placeholder は利用者の声なので語の規則を当てない（1 行である事だけ見る）
EXAMPLE_SKIPS = {"dekimasu", "ad-word", "exclaim", "long-name", "lead", "hero"}

# ---------- 英語の規則（UI-1・SPEC §11 の style sheet = docs/ui-copy/copy.en.md の頭の段落・YC_WAVES §5.6） ----------
EN_RULES = {
    "en-case": "sentence case でない（文の頭の語は大文字で始まり、文の途中の大文字の語は固有名か略語だけ。固有名 = docs/ui-copy の copy.en.md・"
               "additions.md の EN の欄で文の頭以外に大文字で出る語〈表が決める・手で並べない〉・略語 = 全部大文字の語・{…} と “…” の中は見ない）",
    "en-period": "button・tab・label・状態・placeholder・menu の文の末尾に句点（`.`。`…`・`⌘.` と tooltip・error の文は句点でよい・文の間の `. ` はよい）",
    "en-word": "謝り・お願い・成功の語（sorry・apolog・unfortunately・oops・please・successfully・not implemented・isn't available yet・not available yet）",
    "en-status-long": "状態（status）の 1 行が 8 語を超える（1 行の全体を数える・{…} は 1 語・glyph は数えない）",
}
EN_KINDS = {"button", "label", "placeholder", "tooltip", "status", "error", "menu"}  # 英語の規則を当てる種類（ui-strings.py の UI_KINDS と同じ）


_UI_STRINGS = None


def ui_strings():
    """scripts/qa/ui-strings.py を module として読む（file 名に - が在るので importlib・lexer を 2 つ持たない）。"""
    global _UI_STRINGS
    if _UI_STRINGS is None:
        import importlib.util
        spec = importlib.util.spec_from_file_location("ui_strings", os.path.join(ROOT, "scripts", "qa", "ui-strings.py"))
        _UI_STRINGS = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_UI_STRINGS)
    return _UI_STRINGS


def en_items(root, targets, exclude):
    """英語の UI の文 -> [(rel, 行, kind, 文)]。scripts/qa/ui-strings.py の scan(root, targets, exclude) の lang=en・kind ∈ EN_KINDS の行。
    呼ぶ側: for rel, no, kind, text in en_items(ROOT, *SETS["ui-1"]): hits = judge_en(kind, text)
    保証する: #[cfg(test)]・comment・比較と panic の引数は入らない（scan と同じ）。保証しない: 画面に実際に描かれるか・format! の穴を埋めた後の文。
    Wave: UI-1"""
    return [(r[0], r[1], r[2], r[4]) for r in ui_strings().scan(root, targets, exclude) if r[3] == "en" and r[2] in EN_KINDS]


EN_TABLES = [os.path.join(ROOT, "docs", "ui-copy", f) for f in ("copy.en.md", "additions.md")]
EN_WORD = re.compile(r"[A-Za-z][A-Za-z'’-]*")
EN_BAD = re.compile(r"sorry|apolog|unfortunately|oops|please|successfully|not implemented|(isn't|is not|not) available yet", re.I)
QUOTED = re.compile(r"\{[^{}]*\}|“[^”]*”|`[^`]*`|\"[^\"]*\"")
SEG = re.compile(r"(?:[.?!:;]\s+|\s+·\s+|\s+/\s+|\s+—\s+|[(\[]|^)")  # 文と片の頭（ここの次の語は大文字でよい）
_PROPER = None


def _words_mid(text):
    """-> [(語, 片の頭か)]。{…}・“…”・`…` の中は見ない。"""
    out = []
    for seg in SEG.split(QUOTED.sub(" ", text)):
        for n, m in enumerate(EN_WORD.finditer(seg)):
            out.append((m.group(), n == 0))
    return out


def proper_nouns():
    """固有名 = 表（copy.en.md・additions.md）の EN の欄で、片の頭でない所に大文字で出る語（手で並べない）。"""
    global _PROPER
    if _PROPER is None:
        _PROPER = set()
        for path in EN_TABLES:
            for _, _, en in ui_strings().parse_table(path):
                _PROPER |= {w for w, head in _words_mid(en) if not head and w[0].isupper()}
    return _PROPER


def _en_words(text):
    """状態の 1 行の語の数え方: {…} は 1 語・glyph（英数字を含まない token）は数えない。"""
    return [w for w in re.sub(r"\{[^{}]*\}", "x", text).split() if re.search(r"[A-Za-z0-9]", w)]


def _en_case(text):
    t = text.strip().lstrip("…○◐◔⊘▣×✓▶●◌↵⌘·•✦+＋←→↻↶↷ ")
    first = t.split(" ", 1)[0]
    if re.fullmatch(r"[a-z][A-Za-z'’-]*[:,]?", first):
        return True  # 文の頭が小文字（{…}・“…”・記号で始まる文と、owner/repo・v{} のような語でない頭は見ない）
    for w, head in _words_mid(text):
        if head or not w[0].isupper() or len(w) == 1 or w.isupper() or any(c.isupper() for c in w[1:]):
            continue  # 片の頭・小文字・1 字（key の印）・略語・GitHub 型は良い
        if w not in proper_nouns():
            return True
    return False


def _en_period(kind, text):
    t = text.rstrip()
    return kind in ("button", "label", "status", "placeholder", "menu") and t.endswith(".") and not t.endswith(("…", "...", "⌘."))


def _en_status_long(kind, text):
    return kind == "status" and len(_en_words(text.strip())) > 8


def judge_en(kind, text, disabled=()):
    """英語の 1 文 -> 当たった EN_RULES の id の list（judge() の英語版・同じ形）。kind が EN_KINDS の外なら []。
    呼ぶ側: judge_en("button", "Delete page.") == ["en-period"]
    Wave: UI-1"""
    if kind not in EN_KINDS:
        return []
    return [rid for rid in EN_RULES if rid not in disabled and RULES[rid][1](kind, text)]


RULES.update({
    "en-case": (EN_RULES["en-case"], lambda k, s: _en_case(s)),
    "en-period": (EN_RULES["en-period"], _en_period),
    "en-word": (EN_RULES["en-word"], lambda k, s: bool(EN_BAD.search(s))),
    "en-status-long": (EN_RULES["en-status-long"], _en_status_long),
})


def judge(kind, text, disabled=()):
    text = text.strip()
    if not JA.search(text):  # 英語の文には英語の規則だけ（日本語の規則〈multi・long-name〉を当てない）
        return judge_en(kind, text, disabled)
    hits = []
    for rid, (_, fn) in RULES.items():
        if rid in disabled or rid in EN_RULES:
            continue
        if EXAMPLE.match(text) and rid in EXAMPLE_SKIPS:
            continue
        if fn(kind, text):
            hits.append(rid)
    return hits


# ---------- 抜き出し: Rust ----------
# 行ではなく字句（文字列・raw 文字列・char・lifetime・comment を分けた token）で 1 file を通しで読む。
# 開いている括弧の stack が行を跨いで続くので、rustfmt が引数ごとに改行した呼び出しも 1 つの呼び出しとして見る。
TOKEN = re.compile(r"""
    (?P<ws>\s+)
  | (?P<line>//[^\n]*)
  | (?P<block>/\*)
  | (?P<raw>b?r(?P<hashes>\#*)")
  | (?P<str>b?")
  | (?P<char>b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'\n])')
  | (?P<life>'\w+)
  | (?P<ident>[A-Za-z_]\w*(?:!(?!=))?)
  | (?P<num>\d\w*)
  | (?P<punct>::|=>|.)
""", re.X | re.S)
STR_BODY = re.compile(r'(?:[^"\\]|\\.)*"', re.S)
CLOSE = {")": "(", "]": "[", "}": "{"}
WRAPPERS = {"text", "new", "format", "from", "to_owned", "into", "to_string", "Some", "String"}
BUTTONS = {"button", "button_compact", "button_sized", "chip", "icon_button", "selectable_label", "checkbox", "radio_value", "menu_button", "small_button"}
# 名前が何番目の引数か（ui を最初に取る theme:: / shell:: の関数は ui を数えない）。無ければ 0
NAME_ARG = {"selectable_label": 1, "checkbox": 1, "radio_value": 2}
TEXTS = {"label", "small", "heading", "monospace", "weak", "strong"}
NON_UI_MACRO = re.compile(r"^((debug_)?assert\w*|panic|eprintln|println|unreachable|todo|debug|trace)!$")
KEYWORDS = {"if", "match", "while", "for", "in", "return", "let", "else", "loop", "move", "as", "mut", "ref", "where", "fn"}
UI_ARG = re.compile(r"^&?(mut)?\*?\w*ui$")
HEADING = re.compile(r"Type::(Display|Heading\w+)|\.heading\(|\.strong\(\)|Window::new\(")  # .heading( は RichText の .heading() と egui の ui.heading("…") の両方
# cfg(test) の付いた item の種類: fn / mod / impl / struct などは ; か {…} でしか終わらない（where 節・generics の , では終わらない）
ITEM_KW = {"fn", "mod", "impl", "struct", "enum", "trait", "union", "const", "static", "use", "type", "extern", "macro_rules!", "unsafe", "async", "default"}


def lex(src):
    """[(kind, text, line, value)] と、comment と文字列の中身を空白にした code（行の数は src と同じ）。kind = str / bytes / ident / punct。"""
    toks, code, pos, line = [], [], 0, 1
    blank = lambda s: re.sub(r"[^\n]", " ", s)
    while pos < len(src):
        m = TOKEN.match(src, pos)
        k, end = m.lastgroup, m.end()
        if k == "hashes":
            k = "raw"
        if k == "block":  # /* */ は入れ子にできる
            depth, end = 1, pos + 2
            while depth and end < len(src):
                if src.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                elif src.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                else:
                    end += 1
        elif k == "raw":
            close = '"' + m.group("hashes")
            j = src.find(close, end)
            j = len(src) if j < 0 else j
            value, end = src[end:j], min(len(src), j + len(close))
            toks.append(("bytes" if src[pos] == "b" else "str", src[pos:end], line, value))
        elif k == "str":
            b = STR_BODY.match(src, end)
            body_end = b.end() if b else len(src)
            # 行末の \ は改行と次の行の頭の空白ごと消える（Rust の規則）。\" と \n だけ戻す
            value = re.sub(r"\\\n\s*", "", src[end:body_end - 1]).replace('\\"', '"').replace("\\n", "\n")
            toks.append(("bytes" if src[pos] == "b" else "str", src[pos:body_end], line, value))
            end = body_end
        elif k in ("ident", "punct", "num", "char", "life"):
            toks.append(("ident" if k == "ident" else "punct", src[pos:end], line, None))
        text = src[pos:end]
        if k in ("line", "block"):
            code.append(blank(text))
        elif k in ("raw", "str"):
            code.append(text[0] + blank(text[1:-1]) + text[-1] if len(text) > 1 else text)
        else:
            code.append(text)
        line += text.count("\n")
        pos = end
    return toks, "".join(code).split("\n")


def _cfg_has_test(toks, i):
    """cfg( の中身を toks[i] から読む → (test を含むか, 次の位置)。all / any の中の test は含む・not(…) の中は含まない。"""
    name = toks[i][1]
    i += 1
    if i < len(toks) and toks[i][1] == "(":
        i, kids = i + 1, []
        while toks[i][1] != ")":
            g, i = _cfg_has_test(toks, i)
            kids.append(g)
            if toks[i][1] == ",":
                i += 1
        return (name != "not" and any(kids)), i + 1
    if i < len(toks) and toks[i][1] == "=":  # feature = "test" は feature の名前
        return False, i + 2
    return name == "test", i


def _attribute(toks, i):
    """toks[i] が # なら attribute を読む → (次の位置, inner か, test を含む cfg か)。attribute でなければ None。"""
    inner = i + 1 < len(toks) and toks[i + 1][1] == "!"
    j = i + (2 if inner else 1)
    if j >= len(toks) or toks[j][1] != "[":
        return None
    depth, k = 0, j
    while k < len(toks):
        depth += {"[": 1, "]": -1}.get(toks[k][1], 0)
        if depth == 0:
            break
        k += 1
    test = False
    if j + 2 < k and toks[j + 1][1] == "cfg" and toks[j + 2][1] == "(":
        try:
            test = _cfg_has_test(toks, j + 1)[0]
        except IndexError:
            test = False
    return k + 1, inner, test


def _item_mode(toks, i):
    """cfg(test) の後ろ（他の attribute を飛ばした所）の item の終わり方: item（; か {…}）/ let（;）/ expr（, ; か {…} の後）。"""
    while i < len(toks) and toks[i][1] == "#" and (a := _attribute(toks, i)):
        i = a[0]
    if i < len(toks) and toks[i][1] == "pub":  # pub(crate) の (…) も飛ばして次の語で決める（pub な field は expr）
        i += 1
        if i < len(toks) and toks[i][1] == "(":
            while i < len(toks) and toks[i][1] != ")":
                i += 1
            i += 1
    word = toks[i][1] if i < len(toks) else ""
    return "item" if word in ITEM_KW else "let" if word == "let" else "expr"


def rust_items(path, warn=None):
    """(rel, 行, kind, 文)。括弧の対応が取れない・cfg(test) の item が file の終わりまで閉じない時は warn に 1 行足す（黙って見落とさない）。"""
    rel = os.path.relpath(path, ROOT)
    with open(path, encoding="utf-8") as f:
        toks, code = lex(f.read())
    # 論理行: 括弧（( か [）が開いたまま終わる行と、. / ? で始まる続きの行は次の行と 1 つの文にする（rustfmt の改行）
    stack = []  # 開いている括弧: [開き, 呼び出しの名前, method か, 引数の区切りの数, 1 つ目の引数の token]
    open_at_end, found, skip = {}, [], None  # skip = (終わり方, 括弧の深さ) か ("inner", 深さ)
    i = 0
    while i < len(toks):
        kind, text, line, value = toks[i]
        if text == "#" and (a := _attribute(toks, i)):
            nxt, inner, test = a
            if test and skip is None:
                skip = ("inner", len(stack)) if inner else (_item_mode(toks, nxt), len(stack))
            i = nxt
            continue
        if skip and text in CLOSE and len(stack) - 1 < skip[1]:
            skip = None  # 囲む括弧が閉じた（最後の field・腕・inner の mod の終わり）。この } は item の外なので見る
        ends = False  # この token で cfg(test) の item が終わる（token 自身は item の中）
        if text in ("(", "[", "{"):
            j, name = i - 1, None
            if j >= 0 and toks[j][0] == "ident" and toks[j][1] not in KEYWORDS and (text == "(" or toks[j][1].endswith("!")):
                parts = [toks[j][1]]
                while j >= 2 and toks[j - 1][1] == "::" and toks[j - 2][0] == "ident":
                    j -= 2
                    parts.insert(0, toks[j][1])
                name = "::".join(parts)
            if stack and stack[-1][3] == 0:
                stack[-1][4].append(text)
            stack.append([text, name, j >= 1 and toks[j - 1][1] == ".", 0, []])
        elif text in CLOSE:
            while stack and stack[-1][0] != CLOSE[text]:
                stack.pop()
                if warn is not None:
                    warn.append(f"{rel}:{line}: 括弧の対応が取れない（{text}）")
            closed = stack.pop()[0] if stack else None
            if skip and skip[0] != "inner" and len(stack) == skip[1] and closed == "{":
                # {…} の塊の終わり。expr（match の腕・field）は後ろに , . ? else が続けばまだ同じ式
                ends = skip[0] == "item" or (skip[0] == "expr" and (toks[i + 1][1] if i + 1 < len(toks) else "") not in (",", ".", "?", "else"))
        elif text in (",", ";"):
            if stack and text == ",":
                stack[-1][3] += 1
            ends = bool(skip) and skip[0] != "inner" and len(stack) == skip[1] and (text == ";" or skip[0] == "expr")
        elif stack and stack[-1][3] == 0 and len(stack[-1][4]) < 4:
            stack[-1][4].append(text)
        open_at_end[line] = stack[-1][0] if stack else None
        if kind == "str" and not skip:
            found.append((line, value, [tuple(s[:4]) + ("".join(s[4]),) for s in stack]))
        if ends:
            skip = None
        i += 1
    if skip and skip[0] != "inner" and warn is not None:  # file の頭の #![cfg(test)] は file の終わりまでで正しい
        warn.append(f"{rel}: cfg(test) の item が file の終わりまで閉じない（以後を見ていない）")
    if stack and warn is not None:
        warn.append(f"{rel}: 括弧が {len(stack)} 個閉じないまま終わる")

    logical, state, cur = {}, None, 0  # 行 → 論理行の頭の行
    for no in range(1, len(code) + 1):
        joined = no > 1 and (state in ("(", "[") or code[no - 1].lstrip().startswith((".", "?")))
        cur = cur if joined else no
        logical[no] = cur
        state = open_at_end.get(no, state)
    text_of = {}
    for no in range(1, len(code) + 1):
        text_of[logical[no]] = text_of.get(logical[no], "") + code[no - 1] + "\n"
    heads = sorted(text_of)
    index = {h: n for n, h in enumerate(heads)}

    lead_until, by_head = -1, {}
    for line, value, frames in found:
        by_head.setdefault(logical[line], []).append((line, value, frames))
    for h in heads:
        ctx = text_of[h]
        heading_line = bool(HEADING.search(ctx))
        for line, value, frames in by_head.get(h, []):
            if not JA.search(value) or value.startswith("/") or _non_ui(frames):
                continue
            fn, arg = _callee(frames)
            if fn == "content_opening":  # (title, lead, cta: Option<&str>) — cta は button として描かれる
                kind = {0: "label", 1: "lead"}.get(arg, "button")
            elif fn == "heading":  # ui.heading("…") の文は見出しそのもの
                kind = "label"
            elif fn in BUTTONS:
                kind = "button" if arg == NAME_ARG.get(fn, 0) else "text"
            elif fn == "hint_text":
                kind = "placeholder"
            elif "Type::Display" in ctx and fn not in ("on_hover_text",):
                kind = "hero"
            elif not heading_line and index[h] <= lead_until and fn in TEXTS | {"label"}:  # 見出しの直後 3 行までの最初の説明 literal
                kind = "lead"
                lead_until = -1
            else:
                kind = "text"
            yield rel, line, kind, value
        if heading_line or "content_opening(" in ctx:
            lead_until = index[h] + 3


def _non_ui(frames):
    """literal を囲む呼び出しのどれかが assert! / panic! / eprintln! / log:: / tracing:: / .expect( なら UI の文ではない。"""
    for _, name, method, _, _ in frames:
        if not name:
            continue
        if NON_UI_MACRO.match(name.split("::")[-1]) or name.split("::")[0] in ("log", "tracing") or (method and name == "expect"):
            return True
    return False


def _callee(frames):
    """literal を囲む一番内側の呼び出し (名前, 何番目の引数か)。Some( / format!( / .text( など包むだけの呼び出しと {…} [ … ] は外へ抜ける。
    egui::Button::new( は button。ui を最初に取る関数（theme::button(ui, …)）は ui を数えない。
    ponytail: closure の |a, b| と turbofish の , も引数の区切りに数える（kind の位置がずれる）。"""
    for delim, name, method, commas, first in reversed(frames):
        if delim != "(" or not name:
            continue
        last = name.split("::")[-1].rstrip("!")
        if name.endswith("Button::new"):
            return "button", commas
        if last in WRAPPERS:
            continue
        shift = 1 if commas and not method and UI_ARG.match(first) else 0
        return last, commas - shift
    return "", -1


# ---------- 抜き出し: JSON ----------
JSON_KEYS = {"title": "label", "label": "label", "text": "text", "description": "text", "placeholder": "placeholder", "help": "text", "empty": "text"}
SCHEMA_KEYS = {"input_schema", "output_schema", "properties", "schema", "$defs", "definitions"}


def json_items(path):
    rel = os.path.relpath(path, ROOT)
    with open(path, encoding="utf-8") as f:
        raw = f.read()
    try:
        data = json.loads(raw)
    except ValueError as e:
        print(f"{rel}: JSON が読めない ({e})", file=sys.stderr)
        return
    found = []

    def walk(o, key, parents):
        if parents & SCHEMA_KEYS:  # schema の下は dict も文字列も model 向け
            return
        if isinstance(o, dict):
            for k, v in o.items():
                walk(v, k, parents | ({key} if key else set()))
        elif isinstance(o, list):
            for v in o:
                walk(v, key, parents)
        elif isinstance(o, str) and key in JSON_KEYS and JA.search(o):
            # states.empty だけでなく states.* の文も同じ扱い
            found.append((key, JSON_KEYS[key], o))
        elif isinstance(o, str) and "states" in parents | {key} and JA.search(o):  # "states": ["…"] の配列も
            found.append((key, "text", o))

    walk(data, None, set())
    pos = 0
    for key, kind, text in found:
        needle = json.dumps(text, ensure_ascii=False)
        i = raw.find(needle, pos)
        if i < 0:
            i = raw.find(needle)
        if i >= 0:
            pos = i + len(needle)
        yield rel, raw.count("\n", 0, max(i, 0)) + 1, kind, text


def sources(warn=None):
    rs = sorted(glob.glob(os.path.join(ROOT, "src", "ui", "**", "*.rs"), recursive=True))
    # app.rs（画面にそのまま出る文字を持つ）・Project の Page を描く generated.rs と、その面の状態の文（surface・project）
    for extra in ("app.rs", "generated.rs"):
        p = os.path.join(ROOT, "src", extra)
        if os.path.exists(p):
            rs.append(p)
    for d in ("surface", "project"):
        rs += sorted(glob.glob(os.path.join(ROOT, "src", d, "*.rs")))
    js = sorted(set(glob.glob(os.path.join(ROOT, "docs", "examples", "**", "*.json"), recursive=True)))
    for p in rs:
        yield from rust_items(p, warn)
    for p in js:
        yield from json_items(p)


# ---------- allowlist（docs/ui-copy-rules.md の表） ----------
ROW = re.compile(r"^\|\s*`([^`]+)`\s*\|\s*`([^`]+)`\s*\|")


def allowlist():
    try:
        with open(RULES_DOC, encoding="utf-8") as f:
            doc = f.read()
    except OSError:
        return []
    m = re.search(r"<!-- allowlist:start -->(.*?)<!-- allowlist:end -->", doc, re.S)
    return [r.groups() for r in map(ROW.match, (m.group(1).splitlines() if m else [])) if r]


def allowed(rel, text, allow):
    return any((rel == f or fnmatch.fnmatch(rel, f)) and part in text for f, part in allow)


# ---------- self-test ----------
def read_fixture(name):
    rows = []
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
        for no, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            cols = line.split("\t")
            rows.append((no, cols[0], cols[1], cols[2] if len(cols) > 2 else None))
    return rows


KIND_NOTE = re.compile(r"// kind=([\w,-]+)\s*$")


def self_test(disabled):
    fails = 0
    # 抜き出しの試験: extract_rs*.txt（Rust の断片）の各行の literal の kind が行末の // kind=… と一致する。字句が狂った警告も fail
    for path in sorted(glob.glob(os.path.join(FIXTURES, "extract_rs*.txt"))):
        name, got, warn = os.path.basename(path), {}, []
        for _, no, kind, _ in rust_items(path, warn):
            got.setdefault(no, []).append(kind)
        with open(path, encoding="utf-8") as f:
            want = {no: m.group(1).split(",") for no, line in enumerate(f, 1) if (m := KIND_NOTE.search(line))}
        for no in sorted(set(got) | set(want)):
            if got.get(no) != want.get(no):
                fails += 1
                print(f"FAIL {name}:{no} kind が {got.get(no)}（期待 {want.get(no)}）")
        for w in warn:
            fails += 1
            print(f"FAIL {name}: 警告 {w}")
    # 抜き出しの試験: extract_json.json の日本語の文は「見る」を含む物だけ抜き出される（「見ない」は抜き出されない）
    path = os.path.join(FIXTURES, "extract_json.json")
    got = {t for _, _, _, t in json_items(path)}
    with open(path, encoding="utf-8") as f:
        want = {t for t in re.findall(r'"([^"]*)"', f.read()) if "見る" in t}
    for t in sorted(got ^ want):
        fails += 1
        print(f"FAIL extract_json.json: {'抜き出すはずが漏れた' if t in want else '抜き出さないはずが入った'}: {t}")
    oks, ngs = ("ok.txt", "ok.en.txt"), ("ng.txt", "ng.en.txt")
    for name in oks:
        for no, kind, text, _ in read_fixture(name):
            hits = judge(kind, text, disabled)
            if hits:
                fails += 1
                print(f"FAIL {name}:{no} 緑のはずが赤 {hits}: {text}")
    for name in ngs:
        for no, kind, text, want in read_fixture(name):
            hits = judge(kind, text, disabled)
            if want not in hits:
                fails += 1
                print(f"FAIL {name}:{no} [{want}] で赤のはずが {hits or '緑'}: {text}")
    # 負の対照: 規則を 1 つ外すと、その規則だけで赤だった ng の行が緑になる（＝その規則が効いている）
    if not disabled:
        for rid in RULES:
            flipped = [f"{name}:{no}" for name in ngs for no, kind, text, want in read_fixture(name) if want == rid and not judge(kind, text, {rid})]
            if not flipped:
                fails += 1
                print(f"FAIL 負の対照: [{rid}] を外しても緑になる ng の行が無い（規則が効いているか測れていない）")
            else:
                print(f"ok 負の対照: [{rid}] を外すと {','.join(flipped)} が緑になる")
    n_ok, n_ng = (sum(len(read_fixture(x)) for x in xs) for xs in (oks, ngs))
    print(f"self-test: {'PASS' if not fails else 'FAIL'}（ok {n_ok} 行・ng {n_ng} 行・fail {fails}）")
    return 1 if fails else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--report", action="store_true", help="赤でも exit 0")
    g.add_argument("--strict", action="store_true", help="赤があれば exit 1（既定）")
    g.add_argument("--self-test", action="store_true")
    ap.add_argument("--disable", action="append", default=[], choices=sorted(RULES), help="規則を外す（負の対照）")
    ap.add_argument("--set", choices=("all", "ui-1", "ui-2"), help="見る file の集合（UI-1・UI-2・英語の規則と一緒）")
    a = ap.parse_args()
    disabled = set(a.disable)
    if a.self_test:
        return self_test(disabled)

    allow = allowlist()
    red = 0
    seen = set()
    warn = []
    if a.set:  # UI-1: その集合の Rust の日本語の文（rust_items）と英語の文（ui-strings の scan）・JSON は見ない
        U = ui_strings()
        items = [x for p in U.set_files(ROOT, *U.SETS[a.set]) for x in rust_items(p, warn)] + en_items(ROOT, *U.SETS[a.set])
    else:
        items = sources(warn)
    for rel, no, kind, text in items:
        if (rel, no, text) in seen or allowed(rel, text, allow):
            continue
        seen.add((rel, no, text))
        hits = judge(kind, text, disabled)
        if hits:
            red += 1
            why = "・".join(RULES[h][0] for h in hits)
            print(f"{rel}:{no}: [{','.join(hits)}] {why} — {text}")
    for w in warn:  # 字句が狂った file は以後の literal を見ていない = 赤 0 でも strict では通さない
        print(f"警告 {w}", file=sys.stderr)
    label = "report" if a.report else "strict"
    print(f"check_ui_copy ({label}): 赤 {red} 件（allowlist {len(allow)} 行・docs/ui-copy-rules.md）" + (f"・抜き出しの警告 {len(warn)} 件" if warn else "") + (f" [{a.set}]" if a.set else ""))
    return 0 if a.report or (red == 0 and not warn) else 1


if __name__ == "__main__":
    sys.exit(main())
