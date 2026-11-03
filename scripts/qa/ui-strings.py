#!/usr/bin/env python3
"""利用者に見える文言を file:line つきで数え上げる（UI を作り直す前の棚卸し・読むだけ）。

  python3 -I scripts/qa/ui-strings.py --count            # 件数（種類別・言語別・file 別）
  python3 -I scripts/qa/ui-strings.py --csv <out.csv>    # file,line,kind,lang,text
  python3 -I scripts/qa/ui-strings.py --map <copy.en.md> [--map <additions.md> …] [--kinds error,status,…] [--csv-out <p>] [--md-out <p>] [--min 0.9]
      code の文言と英語の文言表（`| JP | EN |`）を突き合わせ、被覆率（対応あり / 全体）を印字し --min 未満で exit 1。
      --map は複数回: 後の表の同じ JP の行が前の行を置き換える（直しの行）。--kinds はその種類だけを被覆率の分母にする（--min もそれに効く）。
      節の名前が `Same EN on purpose` の表（`| EN | 理由 |`）は、複数の JP が同じ EN に落ちてよい物の宣言
  python3 -I scripts/qa/ui-strings.py --classify <out.csv> [--map …]
      text の行を (a) 固定 / (b) 差し込みで組む / (c) 訳さない（生成物の雛形・LLM への指示・利用者が打つ語の照合・英語のまま・font path）に分け、根拠を列に書く。--map が在れば表に無い行だけ。
  --rules <copy-rules.md>   --map の突き合わせに足す表（(b) の枠: `| JP の枠 | EN の枠 | 型 |`・`{…}` はどの差し込みにも当たる）。当たった行は covered（規則）。
  --min-ab 1.0   --rules があれば text の (a)+(b) の被覆（(c) を分母から除く）がこれ未満で exit 1
  --dup-jp --map A --map B [--max-dup N] [--csv-out p]   2 つの表で同じ JP に違う EN の組（A の file:line ‖ B の file:line・JP・EN）を md の表で。N を超えたら exit 1（既定 0）
  --root <dir>   src の親 dir（既定 = この repo。負の対照は写しに試験の文字列を混ぜて --root で回す）
  --set all|ui-1|ui-2|ui-2b|outside   見る file の集合（既定 all = TARGETS）。ui-1 = src/ui/**（src/ui/apps.rs を除く・UI-4 で消す）と src/app.rs。
                 ui-2 = TARGETS 全部から src/ui/apps.rs を除く（UI-2 の Done 1・2・Wave: UI-2）。どの mode にも効く
      ui-2b = demo の筋の 9 file（UI-2 の集合の外・表 = docs/ui-copy/outside.en.md）・outside = copy-scope.md の A 層 20 glob（scripts/qa/ui-strings-outside.py の既定）

UI-1 の口（Wave: UI-1）:
  python3 -I scripts/qa/ui-strings.py --set ui-1 --max-ja 0 [--allow-ja docs/ui-copy/ja-allowlist.md]
      日本語の行（lang=ja）のうち --classify の (a)+(b)（(c) = LLM への指示・font path・利用者が打つ語の照合は数えない・雛形の題の除外は UI-2 holdout H6 の直しで外した）で
      allowlist に無い物を file:line つきで印字し、N 件を超えたら exit 1。allowlist の行が scan の中のどの行にも当たらなければ（古い行）exit 1。
      allowlist の形（docs/ui-copy-rules.md の allowlist と同じ表）: `<!-- ja-allowlist:start -->` と `<!-- ja-allowlist:end -->` の間の
      1 行 = 欄 3 つ（`<file>:<line>` を backtick で囲む | `<文の一部>` を backtick で囲む | 理由）。鍵は file と文の一部（部分一致）で、line は読む人のため（行が動いても当たる）。
      allowlist の 1 行は code の 1 行だけに当たる（2 行以上に当たる鍵は exit 1・反証と holdout の直し: `アプリ` が `はじめての統合アプリ` も黙らせた）。
      理由が空・形の崩れた行は exit 2。対象の集合（--set）の外の file の行は古い行に数えない。
      --map が在れば、表の EN が `(not translated: …)` の行も allowlist の行（UI-2b）: file:line = JP の欄の末尾の注記の `file:line`（src/ からの相対・
      注記に並ぶ全部）・文の一部 = 注記を外した JP（空白・注記・{…} を norm で揃えた全文一致でも当たる）・理由 = 括弧の中。ja-allowlist.md に写さない（2 か所に置かない）。
      表の訳さない行が当たらない（code の側で訳した）時は印字だけで落とさない（表は写しで直せない・訳した理由は docs/ui-copy/outside.additions.md）。
  python3 -I scripts/qa/ui-strings.py --set ui-1 --by en --map docs/ui-copy/copy.en.md --map docs/ui-copy/additions.md --rules docs/ui-copy/rules.md --kinds button,label,placeholder,tooltip,status,error,menu --min 1
      --by en = code の英語の文（lang=en の行だけ・日本語の行は --max-ja が数える）を表の EN の欄で引く（表に在る英語だけを使っているか）。
      EN の欄の ` / ` は JP と同じに分ける（` · ` の片も鍵・本体で足した）・注記（`(…)`）を外して空になる EN（(removed …)）は鍵にしない・
      頭の状態の記号（`✓ Verified` の ✓・`○ Nothing here yet` の ○）は鍵と code の文の両方から外す・--rules の EN の枠（`{…}` は何にでも当たる）も鍵。
      被覆率の分母は lang=en で英字を含む行（記号だけの `×`・`↻` は数えない）。--by jp（既定）は今まで通り JP の欄で引く。
  exit: 0 ok / 1 門が落ちた（--min・--min-ab・--max-ja・古い allowlist・2 行以上に当たる allowlist の行）/ 2 使い方・allowlist の形

見る物: src/ui/**・src/generated.rs・src/app.rs・src/composer/project.rs・src/surface/*.rs。
入る物: 日本語を含む literal / 英語の文（英字の語 2 つ以上）/ 画面の部品の呼び出し（button・label・hint_text …）の引数の literal。
除く物: `#[cfg(test)]` の item・comment・比較や panic の引数（contains / expect / assert …）・match の左辺（`"x" =>`）。
種類 = button / label / placeholder / tooltip / status / error / menu / text（どれにも当たらない）。
"""
import argparse
import collections
import csv
import glob
import os
import re
import sys
import unicodedata

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _rs import lex, blank_tests, line_of  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEBUG_RE = re.compile(r"\{[A-Za-z0-9_.]*:#?\?\}")
TARGETS = ["src/ui/**/*.rs", "src/generated.rs", "src/app.rs", "src/composer/project.rs", "src/surface/*.rs", "src/project/*.rs", "src/adapters/git.rs"]  # G102 FX-105: git.rs の Page の 1 行（not_ready_line）も見る
# --set の集合: (見る glob, 除く file)。ui-1 = UI-1 の Done の集合（YC_WAVES §5.6・apps.rs は UI-4 で消す）
# ui-2 = UI-2 の Done 1・2 の集合（TARGETS 全部・apps.rs は UI-4 で消すので除く）
# ui-2b = UI-2b の Done の集合（demo の筋の 9 file）・outside = copy-scope.md の A 層（goal.rs は C 層・bin/domain/patch などは B 層で入れない）
SETS = {"all": (TARGETS, ()), "ui-1": (["src/ui/**/*.rs", "src/app.rs"], ("src/ui/apps.rs",)), "ui-2": (TARGETS, ("src/ui/apps.rs",)),
        "ui-2b": (["src/capability_engine/*.rs", "src/adapters/mac_gui.rs", "src/intel/**/*.rs", "src/composer/verifier.rs", "src/llm_cli.rs", "src/mcp.rs",
                   "src/terminal/**/*.rs", "src/gui/**/*.rs"], ()),
        "outside": (["src/generated.rs", "src/composer/project.rs", "src/composer/layout.rs", "src/composer/verifier.rs", "src/surface/**/*.rs", "src/project/**/*.rs",
                     "src/mcp.rs", "src/llm_cli.rs", "src/capability_engine/*.rs", "src/intel/**/*.rs", "src/google/**/*.rs", "src/media/**/*.rs", "src/audio/**/*.rs",
                     "src/agent/**/*.rs", "src/github.rs", "src/repos.rs", "src/gui/**/*.rs", "src/terminal/**/*.rs", "src/adapters/mac_gui.rs", "src/ui/apps.rs"], ())}
JA_ALLOW = "docs/ui-copy/ja-allowlist.md"  # --allow-ja の既定（--root からの相対）

JA = re.compile(r"[぀-ヿ一-鿿＀-￯]")
EN = re.compile(r"[A-Za-z]{2,}[ ]+[A-Za-z]{2,}")
KANA_KANJI = re.compile(r"[぀-ヿ一-鿿]")
NOT_TEXT = re.compile(r"::|--|[/\\=]|^\s*$")  # path・flag・式
# 引数が表示ではなく比較・診断の呼び出し
SKIP_CALLS = {"contains", "starts_with", "ends_with", "strip_prefix", "strip_suffix", "find", "split", "split_once", "rsplit_once", "replace", "trim_start_matches",
              "trim_end_matches", "trim_matches", "expect", "unwrap_or_else_panic", "panic", "unreachable", "assert", "assert_eq", "assert_ne",
              "debug_assert", "println", "eprintln", "eprint", "print", "include_str", "env", "arg", "args", "var", "eq_ignore_ascii_case",
              "matches", "register", "has_ja", "is_some_and", "get", "insert_str", "env_var", "Command", "open", "id", "Id"}  # join は入れない: 区切りの literal（「・」）は画面に出る（反証の直し）
PASS = {"format", "Some", "Ok", "to_owned", "to_string", "from", "into", "String", "vec", "push", "push_str", "new", "Cow", "text", "write", "writeln"}
UI_KINDS = {"button", "label", "placeholder", "tooltip", "menu", "status", "error"}
ARM_KINDS = {"label": "label", "wording": "status"}  # この名前の fn の match の腕の文の種類（UI-1・RailTab::label・RegionState::wording ほか）
FN_KINDS = {"enter_label": "button"}  # この名前の fn が返す文は全部その種類（UI-4a: 欄の右端の 1 語 `Create · ⌘↵`・`↵ Change` は header が button で描く・閉包の中の literal も）
GLYPHS = "…○◐◔⊘▣×✓▶●◌↵⌘·"  # 状態の記号（§8）。--by en の鍵と code の文の頭から外す（wording() は記号を除いた語を返す契約）


def enclosing_call(masked, pos):
    """pos を引数に持つ呼び出しの名前の列（内側から）。"""
    names, depth, k = [], 0, pos - 1
    stop = max(0, pos - 400)
    while k >= stop:
        ch = masked[k]
        if ch in ";{}" and depth == 0:
            break
        if ch == ")":
            depth += 1
        elif ch == "(":
            if depth == 0:
                m = re.search(r"([A-Za-z_][A-Za-z0-9_]*)\s*!?\s*$", masked[max(0, k - 60):k])
                if m:
                    names.append((m.group(1), masked[max(0, k - 60):k].rstrip()[: -len(m.group(1))] if m.group(1) else ""))
            else:
                depth -= 1
        k -= 1
    return names


def classify(masked, pos):
    """-> (kind, skip)。"""
    for name, before in enclosing_call(masked, pos):
        if name in SKIP_CALLS:
            return "text", True
        if name == "hint_text":
            return "placeholder", False
        if name in ("on_hover_text", "on_disabled_hover_text", "on_hover_text_at_pointer"):
            return "tooltip", False
        if name == "menu_button":
            return "menu", False
        if "button" in name.lower() or name in ("chip", "line_with_action", "cta"):
            return "button", False
        if name in ("label", "heading", "RichText", "Label", "small", "monospace", "strong", "selectable_label"):
            return "label", False
        if before.endswith("states::") or name in ("inline", "inline_wrap", "region", "execution_strip"):
            return "status", False
        if name == "Err" or name.endswith("_err") or name in ("err", "error"):
            return "error", False
        if name not in PASS:
            break
    # 呼び出しの外: 代入先・return の語で見る
    head = masked[max(0, pos - 80):pos]
    if re.search(r"=>\s*$", head):  # match の腕の右辺: `fn label` の腕は label・`fn wording` の腕は status（UI-1）
        fn = re.findall(r"\bfn\s+([A-Za-z_]\w*)", masked[max(0, pos - 4000):pos])
        if fn and fn[-1] in ARM_KINDS:
            return ARM_KINDS[fn[-1]], False
    fn = re.findall(r"\bfn\s+([A-Za-z_]\w*)", masked[max(0, pos - 4000):pos])
    if fn and fn[-1] in FN_KINDS:
        return FN_KINDS[fn[-1]], False
    stmt = re.split(r"[;{}]", head)[-1]
    if re.search(r"\b(Err|err|error|reject)\b", stmt):
        return "error", False
    if re.search(r"\b(message|note|line|status|detail)\w*\s*=", stmt) or re.search(r"\b(message|note|line|status)\b", stmt):
        return "status", False
    return "text", False


def set_files(root, targets=TARGETS, exclude=()):
    """--set の 1 組 -> 見る file の絶対 path（sorted）。exclude は root からの相対 path。"""
    return sorted({p for t in targets for p in glob.glob(os.path.join(root, t), recursive=True)} - {os.path.join(root, e) for e in exclude})


def scan(root=ROOT, targets=TARGETS, exclude=()):
    """-> [(file, line, kind, lang, text)]。targets / exclude は --set の 1 組。"""
    rows = []
    files = set_files(root, targets, exclude)
    for path in files:
        rel = os.path.relpath(path, root)
        with open(path, encoding="utf-8") as f:
            src = f.read()
        masked, lits = lex(src)
        masked = blank_tests(masked)
        for a, b, val in lits:
            if masked[a] == " " and masked[a:b].strip() == "":  # test の item の中
                continue
            if val.strip() == "" or "\x01" in val:
                continue
            kind, skip = classify(masked, a)
            if skip:
                continue
            if re.match(r"\s*(=>|\|)", masked[b:b + 8]):  # match の左辺
                continue
            if re.fullmatch(r"[a-z_0-9.]+", val):  # field 名・key
                continue
            ja, en = bool(JA.search(val)), bool(EN.search(val))
            if not (ja or (en and not NOT_TEXT.search(val)) or (kind in UI_KINDS and (any(c.isalpha() or ord(c) > 127 for c in val)))):
                continue
            rows.append((rel, line_of(src, a), kind, "ja" if ja else "en", val))
    return rows


# ---- --map: 英語の文言表との突き合わせ ----
NOTE = re.compile(r"[（(][^()（）]*[）)]")  # 注記: （accessible name）・(`src/app.rs:1`)
HOLE = re.compile(r"\{[^{}]*\}")  # 差し込み -> *
STOP_FIRST = {"the", "a", "an", "no", "all", "your", "home", "new", "more", "not", "and", "or"}


def norm(t):
    t = unicodedata.normalize("NFKC", t).replace("`", "")
    t = HOLE.sub("*", NOTE.sub("", t) if NOTE.sub("", t).strip() else t)  # 注記だけの文言（`（{s}）`）は注記を落とさない
    t = re.sub(r"\s+", "", t)
    return re.sub(r"[。.…]+$", "", t)


SAME_SEC = "Same EN on purpose"


def parse_table(path, same=None, only=None, where=None):
    """-> [(section, jp, en)]。(new) / (code) の行は jp=None（新しい文言・code だけの文言）。JP 欄の ` / ` は分けて EN も数が合えば分ける。
    where（list）を渡すと out と同じ並びで行番号を足す（--dup-jp の file:line 用）。"""
    out, sec = [], ""
    for ln, line in enumerate(open(path, encoding="utf-8"), 1):
        if where is not None:
            where += [ln - 1] * (len(out) - len(where))
        if line.startswith("#"):
            sec = line.lstrip("#").strip()
        if only and not sec.startswith(only):  # 規則の表は「(b) Frames」の節だけを読む（型の表・(c) の一覧は読まない）
            continue
        if sec.startswith(SAME_SEC) and same is not None:
            m = re.match(r"\|(.+?)\|", line)
            if m and m.group(1).strip() not in ("EN", "") and not set(m.group(1)) <= set(" -"):
                same.add(m.group(1).strip())
            continue
        m = re.match(r"\|((?:\\\||[^|])+?)\|(.+)\|\s*$", line)  # JP 欄の `\|` は縦線そのもの
        if not m or set(m.group(1)) <= set(" -") or m.group(1).strip() == "JP":
            continue
        en = m.group(2).strip()
        if only:
            en = en.rsplit(" | ", 1)[0]  # 規則の表の最後の欄（差し込みの型）は EN ではない
        jp, en = m.group(1).strip().replace("\\|", "|"), en.replace("\\|", "|")
        whole = jp.startswith("=")  # JP の頭の `=` は ` / ` で分けない（`Desktop / 画面の操作` のように全体で 1 つの文言）
        jp = jp.lstrip("=").strip()
        if jp.startswith("(new)") or jp.startswith("(code)"):
            out.append((sec, None, en))
            continue
        js, es = ([jp], [en]) if whole else ([x.strip() for x in jp.split(" / ")], [x.strip() for x in en.split(" / ")])
        if len(js) > 1 and len(js) == len(es):
            out += [(sec, j, e) for j, e in zip(js, es)]
        else:
            out += [(sec, j, en) for j in js]
    if where is not None:
        where += [ln] * (len(out) - len(where))
    return out


def load_maps(paths, only=None):
    """-> (table, same)。後の表の同じ JP の行が前の行を置き換える。"""
    table, same = [], set()
    for p in paths:
        new = parse_table(p, same, only)
        jps = {t[1] for t in new if t[1]}
        table = [t for t in table if t[1] not in jps] + new
    return table, same


def dup_jp(paths):
    """-> [(A の file:line, B の file:line, JP, EN_A, EN_B)]。2 つの表で同じ JP に違う EN（load_maps は後の表で黙って上書きする）。
    `*` を含む鍵と注記の行（EN が `(` で始まる）は除く。"""
    tabs = []
    for p in paths:
        where = []
        tabs.append({})
        for (_, jp, en), ln in zip(parse_table(p, where=where), where):
            jp = jp and (NOTE.sub("", jp).strip() or jp)  # 鍵の末尾の注記（`(file:line · kind)`）は照合で落とす
            if jp and "*" not in jp and not en.lstrip().startswith("("):
                tabs[-1].setdefault(jp, (f"{os.path.relpath(p, ROOT) if p.startswith(ROOT) else p}:{ln}", en))
    return [(a[0], b[0], jp, a[1], b[1]) for i, ta in enumerate(tabs) for tb in tabs[i + 1:] for jp, a in ta.items()
            if jp in tb and NOTE.sub("", a[1]).strip() != NOTE.sub("", (b := tb[jp])[1]).strip()]


def match_rows(rows, table, fuzzy=True, by="jp"):
    """code の各行 -> (一致の種類, 表の行 index)。完全一致 -> 前方 -> 部分（fuzzy=False なら完全一致だけ）。
    by="en"（UI-1）: 表の EN の欄を鍵にする（norm は同じ・EN の ` / ` は分ける・`(` で始まる EN は鍵にしない）。英語の文が表の英語をそのまま
    使っているかを見るので fuzzy は使わない（完全一致と `{…}` の型だけ）。保証しない: 文の意味が JP の行と合うか（それは --by jp の表の側）。"""
    if by == "en":
        keys = [(i, k) for i, (_, _, en) in enumerate(table) for k in en_keys(en)]
        fuzzy = False
        conv = lambda t: norm(t.lstrip(GLYPHS + " "))
    else:
        keys = [(i, norm(jp)) for i, (_, jp, _) in enumerate(table) if jp and norm(jp)]
        conv = norm
    # 記号だけ・差し込みだけの鍵（`×`・`{} · {}`）は全文一致だけ。パターンにすると何にでも当たる
    # 鍵ごとの型（同じ表の行の鍵が複数〈EN の ` / `・` · ` の片〉でも、どの鍵の `{…}` も何にでも当たる・UI-2 の本体で直した: 行ごとに最後の鍵の型だけが残り、`Redo {action} · ⇧⌘Z` の `Redo` が当たらなかった）
    pats = {(i, k): re.compile("^" + ".*".join(map(re.escape, k.split("*"))) + "$") for i, k in keys if len(k.replace("*", "")) >= 2}
    res = []
    for r in rows:
        c = conv(r[4])
        hit = next((("exact", i) for i, k in keys if k == c or ((i, k) in pats and pats[(i, k)].match(c))), None)
        # 前方 / 部分は短い鍵が長い文に偶然当たるのを避けて、鍵が文の半分以上を占める時だけ
        for kind in (("prefix", "part") if fuzzy else ()):
            if hit:
                break
            for i, k in keys:
                kk = k.replace("*", "")
                if len(kk) < 4 or len(c) < 4 or min(len(kk), len(c)) / max(len(kk), len(c)) < 0.5:
                    continue
                if (kind == "prefix" and (c.startswith(k.rstrip("*")) or k.startswith(c.rstrip("*")))) or (kind == "part" and (kk in c or c in k)):
                    hit = (kind, i)
                    break
        res.append(hit)
    return res


LITERAL_NOTE = re.compile(r"^\((?!removed\)|new\))[a-z]+\)$|^\(\{[^}]*\}\)$")


def en_keys(en):
    """表の EN の欄 -> --by en の鍵（norm 済み）。` / ` と ` · ` で分けた各片と全体・注記（`(…)`）を外して空なら鍵にしない（(removed …)・(nothing) …）・
    頭の状態の記号（`✓ Verified` の ✓）は外す。` · ` の片を鍵にするのは、表が 1 つの欄に文と key の印（`Stop · ⌘.`）や同じ所の
    複数の文（`Go to Home · Go to Runs`）を並べるため（UI-1 の本体で interface の「` / ` で分ける」に足した）。"""
    out = []
    for whole in [x.strip() for x in en.replace("`", "").split(" / ")]:
        for part in [whole] + [x.strip() for x in whole.split(" · ")]:
            bare = NOTE.sub("", part).strip()
            if not bare and LITERAL_NOTE.match(part):
                bare = part  # `(empty)`・`({s})` は注記ではなく画面の文字（括弧ごと）
            if not bare:
                continue
            k = norm(bare.lstrip(GLYPHS + " "))
            if k and k not in out:
                out.append(k)
    return out


def table_errors(table, rows, res, same=frozenset()):
    """表の側の誤り（設計への差し戻しの材料・直さない）。"""
    errs, kinds = [], collections.defaultdict(set)
    for r, h in zip(rows, res):
        if h:
            kinds[h[1]].add(r[2])
    by_en = collections.defaultdict(list)
    for i, (sec, jp, en) in enumerate(table):
        e = NOTE.sub("", en).strip()
        if en.lstrip().startswith("("):  # (removed …)・(moved …)・(nothing)・(Ready; …) は EN ではない
            continue
        if jp:
            by_en[e].append((jp, sec))
        glyph = jp is not None and not any(c.isalnum() for c in jp)  # `＋`・`×` だけの JP は記号の行（EN は注記）
        if KANA_KANJI.search(e):
            errs.append(("EN に日本語が残る", sec, jp or "(new)", en))
        if i in kinds and kinds[i] & {"button", "tab", "label", "menu", "placeholder"} and not e.startswith("(") and not jp.endswith("。") and re.search(r"[^.⌘]\.$", e) and "." not in e[:-1]:
            errs.append(("button / label / placeholder の EN が句点で終わる", sec, jp, en))
        if i in kinds and "button" in kinds[i] and not glyph and not e.startswith("(") and (not e[:1].isalpha() or e.split()[0].lower() in STOP_FIRST):
            errs.append(("button の EN が動詞で始まらない（目視の材料）", sec, jp, en))
    for e, jps in by_en.items():
        if len({j for j, _ in jps}) > 1 and e not in same:
            errs.append(("2 つの JP が同じ EN に落ちる", jps[0][1], " ‖ ".join(sorted({j for j, _ in jps})), e))
    return errs


# ---- --classify: text の (a) 固定 / (b) 差し込み / (c) 訳さない ----
C_GRAMMAR = {"ページ", "をミラーで見る", "を mirror で見る", "を Mirror で見る"}  # 利用者が打つ語の照合（strip_suffix の鍵）


def cls(row, lines):
    """-> (a|b|c, 根拠)。lines = row の file の行（LLM への指示かを行で見る）。"""
    rel, ln, _, _, val = row
    line = lines.get(rel, [""] * (ln + 1))[ln - 1] if ln else ""
    if "JSON Schema" in val or "LLM_INSTRUCTIONS" in line or (rel.endswith("composer/project.rs") and ('"description"' in line or "opt(" in line)):
        return "c", "LLM への指示・schema の説明（利用者の言語を写す側・訳さない）"
    if val.startswith("/System/Library/Fonts"):
        return "c", "font の path（表示する文言ではない）"
    if rel.endswith("composer/project.rs") and val in C_GRAMMAR:
        return "c", "利用者が打つ語の照合（入力の文法・日本語のまま受ける）"
    if not JA.search(val):
        return "c", "既に英語（訳す物が無い）"
    holes = sorted(set(HOLE.findall(val)))
    if holes:
        return "b", "差し込み " + " ".join(holes)
    return "a", "固定の文（差し込み無し）"


def load_ja_allow(path):
    """--allow-ja の表 -> [(file, line, 文の一部, 理由)]。file が無ければ []（allowlist 無し = 全部数える）。
    形: `<!-- ja-allowlist:start -->`〜`<!-- ja-allowlist:end -->` の間の `| `file:line` | `文の一部` | 理由 |`（docs/ui-copy-rules.md の allowlist と同じ表）。
    呼ぶ側: allow = load_ja_allow(os.path.join(root, JA_ALLOW))。保証する: 理由が空・line が数でない行は SystemExit(2)（黙って読み飛ばさない）。
    Wave: UI-1"""
    if not os.path.exists(path):
        return []
    doc = open(path, encoding="utf-8").read()
    m = re.search(r"<!-- ja-allowlist:start -->(.*?)<!-- ja-allowlist:end -->", doc, re.S)
    if not m:
        raise SystemExit(f"ui-strings: {path} に <!-- ja-allowlist:start --> 〜 end の印が無い")
    out = []
    for ln in m.group(1).splitlines():
        ln = ln.strip()
        if not ln.startswith("|") or set(ln) <= set("|- ") or ln.startswith("| file:line"):
            continue
        r = re.fullmatch(r"\|\s*`([^`:]+):(\d+)`\s*\|\s*`([^`]+)`\s*\|(.*)\|", ln)
        if not r:
            raise SystemExit(f"ui-strings: allowlist の行の形が違う（| `file:line` | `文の一部` | 理由 |）: {ln}")
        if not r.group(4).strip():
            raise SystemExit(f"ui-strings: allowlist の行に理由が無い: {ln}")
        out.append((r.group(1), int(r.group(2)), r.group(3), r.group(4).strip()))
    return out


SITE_NOTE = re.compile(r"\s*\((`[^`]+`(?:,\s*`[^`]+`)*)[^()]*\)\s*$")  # JP の欄の末尾の注記 (`file:line`, … · 種類)
NOT_TRANSLATED = re.compile(r"^\(not translated: (.+)\)$")


def map_ja_allow(paths):
    """--map の表の `(not translated: …)` の行 -> load_ja_allow と同じ [(file, line, 文の一部, 理由)]。注記の `file:line` ごとに 1 行（src/ からの相対を src/ つきに）。
    呼ぶ側: bad, stale, wide = ja_rows(rows, a.root, allow, files, exact=map_ja_allow(a.map or []))。
    保証する: 注記に file:line の無い行は allowlist にしない（黙って全 file に効かせない）・当たりは文の全文一致（ja_rows の exact・部分一致にしない =
    `{}` のような短い JP が同じ file の別の文を黙って通さない）。
    Wave: UI-2b"""
    out = []
    for p in paths:
        for _, jp, en in parse_table(p):
            m = NOT_TRANSLATED.match(en.strip())
            note = SITE_NOTE.search(jp or "")
            if not (m and note):
                continue
            part = jp[:note.start()].strip()
            for site in re.findall(r"`([^`:]+):(\d+)`", note.group(1)):
                out.append(("src/" + site[0], int(site[1]), part, m.group(1)))
    return out


def ja_rows(rows, root, allow, files, exact=()):
    """--max-ja の本体 -> (数える行, 古い allowlist の行, 2 行以上に当たる allowlist の行と当たった行)。数える行 = lang=ja で cls が (a) か (b) で、allow のどの行（同じ file・文の一部が text に在る）にも
    exact のどの行（同じ file・空白を除いて文の全文が同じ。UI-2b の --map の `(not translated: …)` の行）にも当たらない行。
    古い行 = file が files（--set の中の全 file の相対 path・行が 0 の file も含む）に在るのに、rows のどの行にも当たらない allow / exact の行。
    広い行 = rows の 2 つ以上の (file, line) に当たる allow の行（file:line は 1 行ずつ・鍵が広いと後から足した日本語も黙る・exact は全文一致なので数えない）。
    呼ぶ側: bad, stale, wide = ja_rows(rows, a.root, allow, {os.path.relpath(p, a.root) for p in set_files(a.root, *SETS[a.set])}, exact=map_ja_allow(a.map or []))
    exit 1 if len(bad) > a.max_ja or allow の古い行（exact の古い行は印字だけ） or wide
    保証する: (c) の行（LLM への指示・font path・利用者が打つ語）は数えない・kind（--kinds）は見ない（日本語は種類を問わず数える）。
    保証しない: 英語の文が表に在るか（--by en）・画面に実際に描かれるか（試験と実機）。
    Wave: UI-1"""
    flat = lambda t: re.sub(r"\s+", "", unicodedata.normalize("NFKC", t))  # 表の JP は literal の改行を空白 1 つで書く
    hit = lambda e, r: r[0] == e[0] and e[2] in r[4]
    same = lambda e, r: r[0] == e[0] and flat(e[2]) == flat(r[4])
    ja = [r for r in rows if r[3] == "ja"]
    counted = [r for r, c, _ in classify_rows(ja, root) if c in ("a", "b") and not any(hit(e, r) for e in allow) and not any(same(e, r) for e in exact)]
    stale = [e for e in allow if e[0] in files and not any(hit(e, r) for r in rows)] + [e for e in exact if e[0] in files and not any(same(e, r) for r in rows)]
    wide = [(e, sorted({(r[0], r[1]) for r in rows if hit(e, r)})) for e in allow]
    return counted, stale, [(e, at) for e, at in wide if len(at) > 1]


def classify_rows(rows, root, only=None):
    lines = {}
    for r in rows:
        if r[0] not in lines:
            lines[r[0]] = open(os.path.join(root, r[0]), encoding="utf-8").read().split("\n")
    return [(r, *cls(r, lines)) for r in rows if only is None or id(r) in only]


def do_map(a, rows):
    if a.by == "en":  # 分母は英語の行だけ（日本語の行は --max-ja が数える）。英字の無い記号だけの行（`×`・`↻`・`•••`）は英語の文ではないので数えない
        rows = [r for r in rows if r[3] == "en" and re.search(r"[A-Za-z]", r[4])]
    table, same = load_maps(a.map)
    res = match_rows(rows, table, by=a.by)
    if a.rules:
        rtab, _ = load_maps([a.rules], only="(b) Frames")
        off = len(table)
        rres = match_rows(rows, rtab, fuzzy=False, by=a.by)
        res = [h or (("rule", off + r[1]) if r else None) for h, r in zip(res, rres)]
        table = table + rtab
    allrows, allres = rows, res
    if a.kinds:
        ks = set(a.kinds.split(","))
        keep = [i for i, r in enumerate(rows) if r[2] in ks]
        rows, res = [rows[i] for i in keep], [res[i] for i in keep]
    hit = sum(1 for h in res if h)
    cov = hit / len(rows) if rows else 0.0
    ab = None
    if a.rules and a.by == "jp":  # (a)/(b)/(c) は日本語の行の分け方（英語の行は全部 (c)）
        cl = classify_rows(rows, a.root)
        ab_rows = [(c[0], h) for c, h in zip(cl, res) if c[1] != "c"]
        ab = (sum(1 for _, h in ab_rows if h), len(ab_rows))
        print(f"(a)+(b) {ab[0]}/{ab[1]} = {ab[0] / max(ab[1], 1):.3f}  (c) {len(rows) - ab[1]} 件  規則で覆った {sum(1 for h in res if h and h[0] == 'rule')} 件")
    print(f"coverage {hit}/{len(rows)} = {cov:.3f}{' [' + a.kinds + ']' if a.kinds else ''}  (表 {len(table)} 行・うち JP あり {sum(1 for t in table if t[1])})")
    if a.csv_out:
        os.makedirs(os.path.dirname(os.path.abspath(a.csv_out)), exist_ok=True)
        with open(a.csv_out, "w", encoding="utf-8", newline="") as f:
            w = csv.writer(f)
            w.writerow(["file:line", "kind", "JP", "EN", "match", "section"])
            for r, h in zip(rows, res):
                t = table[h[1]] if h else None
                w.writerow([f"{r[0]}:{r[1]}", r[2], r[4], t[2] if t else "", h[0] if h else "none", t[0] if t else ""])
    if a.md_out:
        used = {h[1] for h in res if h}
        miss = [r for r, h in zip(rows, res) if not h]
        tmiss = [t for i, t in enumerate(table) if t[1] and i not in used and not t[2].startswith("(removed")]
        errs = table_errors(table, allrows, allres, same)
        L = [f"# copy-coverage（`ui-strings.py --map` の出力・`{', '.join(os.path.basename(m) for m in a.map)}` との突き合わせ）", "",
             f"- 被覆率 **{hit}/{len(rows)} = {cov:.1%}**" + (f"（種類 {a.kinds} だけ）" if a.kinds else "") + "（一致の種類: " + " / ".join(f"{k} {v}" for k, v in collections.Counter(h[0] for h in res if h).items()) + "）",
             f"- 表 {len(table)} 行（JP あり {sum(1 for t in table if t[1])}・(new)/(code) {sum(1 for t in table if not t[1])}）", "",
             f"## code に在って表に無い（{len(miss)} 件）", "", "| 種類 | 件数 |", "|---|---|"]
        L += [f"| {k} | {v} |" for k, v in collections.Counter(r[2] for r in miss).most_common()]
        L += ["", "上位 30 件（file 順）:", "", "| file:line | 種類 | 文言 |", "|---|---|---|"]
        L += [f"| {r[0]}:{r[1]} | {r[2]} | {r[4][:60].replace('|', '\\|').replace(chr(10), ' ')} |" for r in miss[:30]]
        L += ["", f"## 表に在って code に無い（(new) を除く・{len(tmiss)} 件）", "", "| 節 | JP | EN |", "|---|---|---|"]
        L += [f"| {t[0]} | {t[1][:60].replace('|', '\\|')} | {t[2][:60].replace('|', '\\|')} |" for t in tmiss]
        L += ["", f"## 表の側の誤り（設計 workflow への差し戻しの材料・直さない・{len(errs)} 件）", ""]
        for k, n in collections.Counter(e[0] for e in errs).most_common():
            L += [f"### {k}（{n}）", "", "| 節 | JP | EN |", "|---|---|---|"]
            L += [f"| {e[1]} | {e[2][:60].replace('|', '\\|')} | {e[3][:70].replace('|', '\\|')} |" for e in errs if e[0] == k]
            L.append("")
        os.makedirs(os.path.dirname(os.path.abspath(a.md_out)), exist_ok=True)
        open(a.md_out, "w", encoding="utf-8").write("\n".join(L) + "\n")
    if cov < a.min:
        for r, h in zip(rows, res):
            if not h:
                print(f"miss {r[0]}:{r[1]} [{r[2]}] {r[4][:90]}", file=sys.stderr)
        print(f"FAIL: coverage {cov:.3f} < --min {a.min}", file=sys.stderr)
        sys.exit(1)
    if ab and ab[0] / max(ab[1], 1) < a.min_ab:
        print(f"FAIL: (a)+(b) {ab[0]}/{ab[1]} < --min-ab {a.min_ab}", file=sys.stderr)
        sys.exit(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--map", action="append")
    ap.add_argument("--kinds")
    ap.add_argument("--csv-out")
    ap.add_argument("--md-out")
    ap.add_argument("--min", type=float, default=0.0)
    ap.add_argument("--min-ab", type=float, default=0.0)
    ap.add_argument("--rules")
    ap.add_argument("--classify")
    ap.add_argument("--csv")
    ap.add_argument("--count", action="store_true")
    ap.add_argument("--root", default=ROOT)
    ap.add_argument("--set", choices=sorted(SETS), default="all")
    ap.add_argument("--max-ja", type=int)
    ap.add_argument("--max-debug", type=int)
    ap.add_argument("--allow-ja")
    ap.add_argument("--by", choices=("jp", "en"), default="jp")
    ap.add_argument("--dup-jp", action="store_true")
    ap.add_argument("--max-dup", type=int, default=0)
    a = ap.parse_args()
    if a.dup_jp:
        d = dup_jp(a.map or [])
        if a.csv_out:
            with open(a.csv_out, "w", encoding="utf-8", newline="") as f:
                csv.writer(f).writerows([("A", "B", "JP", "EN_A", "EN_B"), *d])
        print("| A | B | JP | EN_A | EN_B |\n|---|---|---|---|---|")
        print(*[("| " + " | ".join(x.replace("|", "\\|") for x in r) + " |") for r in d], sep="\n")
        print(f"dup-jp {len(d)}（max {a.max_dup}）")
        sys.exit(1 if len(d) > a.max_dup else 0)
    rows = scan(a.root, *SETS[a.set])
    if a.max_debug is not None:
        # G102 FX-102: 画面に出る文に Rust の Debug の差し込み（`{e:?}`・`{:?}`）を入れない（利用者に意味が無く壊れた印象を与える）。stderr の行（eprintln）は数えない
        dbg = [r for r in rows if DEBUG_RE.search(r[4]) and not r[4].lstrip().startswith(("G", "M4", "shibubu:")) and "eprintln" not in r[2]]
        for r in dbg:
            print(f"debug {r[0]}:{r[1]} [{r[2]}] {r[4][:80]}")
        print(f"debug {len(dbg)} 件（--max-debug {a.max_debug}）[{a.set}]")
        if len(dbg) > a.max_debug:
            sys.exit(1)
        if a.max_ja is None:
            return
    if a.max_ja is not None:
        allow, exact = load_ja_allow(a.allow_ja or os.path.join(a.root, JA_ALLOW)), map_ja_allow(a.map or [])
        bad, stale, wide = ja_rows(rows, a.root, allow, {os.path.relpath(p, a.root) for p in set_files(a.root, *SETS[a.set])}, exact=exact)
        for r in bad:
            print(f"ja {r[0]}:{r[1]} [{r[2]}] {r[4][:80]}")
        for e in stale:
            print(f"{'表の訳さない行が当たらない（outside.additions.md）' if e in exact else '古い allowlist の行'}: {e[0]}:{e[1]} `{e[2]}`", file=sys.stderr)
        for e, at in wide:
            print(f"allowlist の 1 行が {len(at)} 行に当たる（file:line は 1 行ずつ・鍵を狭める）: {e[0]}:{e[1]} `{e[2]}` -> " + ", ".join(f"{f}:{n}" for f, n in at), file=sys.stderr)
        print(f"ja {len(bad)} 件（--max-ja {a.max_ja}・allowlist {len(allow)} 行・表の訳さない行 {len(exact)}・古い {len(stale)} 行・広い {len(wide)} 行）[{a.set}]")
        if len(bad) > a.max_ja or any(e not in exact for e in stale) or wide:
            sys.exit(1)
        if not a.map:
            return
    if a.classify:
        rows = [r for r in rows if r[2] == "text"]
        if a.map:
            table, _ = load_maps(a.map)
            rows = [r for r, h in zip(rows, match_rows(rows, table)) if not h]
        cl = classify_rows(rows, a.root)
        os.makedirs(os.path.dirname(os.path.abspath(a.classify)), exist_ok=True)
        with open(a.classify, "w", encoding="utf-8", newline="") as f:
            w = csv.writer(f)
            w.writerow(["file:line", "class", "basis", "text"])
            w.writerows([f"{r[0]}:{r[1]}", c, b, r[4]] for r, c, b in cl)
        print(f"text {len(cl)}: " + " ".join(f"({k}) {n}" for k, n in sorted(collections.Counter(c for _, c, _ in cl).items())))
        return
    if a.map:
        return do_map(a, rows)
    if a.csv:
        os.makedirs(os.path.dirname(os.path.abspath(a.csv)), exist_ok=True)
        with open(a.csv, "w", encoding="utf-8", newline="") as f:
            w = csv.writer(f)
            w.writerow(["file", "line", "kind", "lang", "text"])
            w.writerows(rows)
    if a.count or not a.csv:
        print(f"total {len(rows)}")
        for title, idx in (("kind", 2), ("lang", 3), ("file", 0)):
            print(f"-- {title}")
            for k, v in collections.Counter(r[idx] for r in rows).most_common():
                print(f"{v:5d} {k}")


if __name__ == "__main__":
    main()
