#!/usr/bin/env python3
"""公開インターフェイスの一覧（owner 2026-10-21 21 通目「interfaceを前提にまず全て作ってから必要になったら足し引きしていく形がいい。
みやすく、全てを参照しやすく統合しまくってrepo管理を軽くしよう。」・CLAUDE.md 7b）。

src/（src/bin/ を除く）の doc comment（`///` と `//!`）の「Wave: <Wave>[, <Wave>…]」を拾い、Wave ごとに path:line・名前・署名 1 行・状態を
docs/YC_INTERFACES.md に書き出す。契約の正本は各 doc comment で、この一覧は索引（手で書かない）。
  - 並びは docs/YC_WAVES.md §0 の表の行の順。表に無い Wave の tag は打ち間違いとして赤（--check も書き出しも exit 1）。
  - 状態: todo!() = fn の本体が todo!() だけ・一部 todo!() = 中身の一部が todo!()・本体あり = fn の中身が在る・宣言 = 型・定数・trait の口・enum の腕・
    module の行（`//!`）は file の中に todo!() が在れば「todo!() を含む」。
  - 各 file の `#[cfg(test)]` の行から後ろは見ない（scripts/check_app_agnostic.py と同じ規則）。

  python3 -I scripts/interfaces.py              # docs/YC_INTERFACES.md を書き直す（src の tag を変えたら同じ commit で）
  python3 -I scripts/interfaces.py --check      # 書き直した物と 1 byte でも違えば exit 1（scripts/lint.sh から）
  python3 -I scripts/interfaces.py --self-test  # 拾い方の負の対照
merge で docs/YC_INTERFACES.md が衝突したら、src を merge した後に書き出しを 1 回回して作り直す（手で直さない）。
"""
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
OUT = os.path.join(ROOT, "docs", "YC_INTERFACES.md")
WAVES = os.path.join(ROOT, "docs", "YC_WAVES.md")
WAVE_ID = r"[A-Za-z0-9][A-Za-z0-9\-]*"
TAG = re.compile(rf"Wave:\s*({WAVE_ID}(?:\s*[,、・]\s*{WAVE_ID})*)")
ITEM = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:const|async|unsafe)\s+)*"
    r"(fn|struct|enum|trait|type|const|static|mod|union)\s+([A-Za-z_][A-Za-z0-9_]*)")
VARIANT = re.compile(r"^\s*([A-Z][A-Za-z0-9_]*)\s*(?:[{(,]|$)")
OUTER = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:impl(?:<[^>]*>)?\s+(?:[A-Za-z0-9_:<>, ]+?\s+for\s+)?([A-Za-z_][A-Za-z0-9_]*)"
    r"|(?:enum|trait|struct|mod)\s+([A-Za-z_][A-Za-z0-9_]*))")


def wave_order(path=WAVES):
    """§0 の表の 1 列目の Wave（`**` と、空白・括弧から後ろの注を落とす）を表の順に。"""
    order, on = [], False
    for line in open(path, encoding="utf-8"):
        if line.startswith("## "):
            on = line.startswith("## §0")
            continue
        if on and line.startswith("| ") and not line.startswith(("| Wave", "|---")):
            w = re.split(r"[ （(]", line.split("|")[1].replace("*", "").strip(), maxsplit=1)[0]
            if w and w not in order:
                order.append(w)
    return order


def code(line):
    """1 行の `//` の comment を落とし、文字列の中身を `_` に替えた物（長さは comment の前まで元の行と同じ・brace と `;` を数えるため）。"""
    out, in_str, esc = [], False, False
    for i, ch in enumerate(line):
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
                out.append('"')
                continue
            out.append("_")
        elif ch == '"':
            in_str = True
            out.append('"')
        elif line.startswith("//", i):
            break
        else:
            out.append(ch)
    return "".join(out)


def item_at(lines, start):
    """lines[start] から始まる item の署名（最初の `{` か `;` まで・括弧の中は除く・元の文字で 1 行に詰める）と本体の行（無ければ None）。"""
    sig, depth = [], 0
    for i in range(start, len(lines)):
        raw = lines[i][1]
        s = code(raw)
        for j, ch in enumerate(s):
            depth += (ch in "([") - (ch in ")]")
            if depth == 0 and ch in "{;":
                head = " ".join(" ".join(sig + [raw[:j]]).split())
                if ch == ";":
                    return head, None
                body, d = [], 0
                for k in range(i, len(lines)):
                    seg = s[j:] if k == i else code(lines[k][1])
                    body.append(seg)
                    d += seg.count("{") - seg.count("}")
                    if d <= 0:
                        break
                return head, body
        sig.append(raw[:len(s)])
    return " ".join(" ".join(sig).split()), None


def enclosing(lines, idx):
    """lines[idx] より字下げの浅い、直前の impl / enum / trait / struct / mod の名前（top level なら None）。"""
    ind = indent(lines[idx][1])
    if ind == 0:
        return None
    for k in range(idx - 1, -1, -1):
        t = lines[k][1]
        if not t.strip() or t.lstrip().startswith(("//", "#")):
            continue
        if indent(t) < ind:
            m = OUTER.match(t)
            return (m.group(1) or m.group(2)) if m else None
    return None


def indent(line):
    return len(line) - len(line.lstrip())


def tags(doc):
    found = []
    for m in TAG.finditer(doc):
        for w in re.split(r"\s*[,、・]\s*", m.group(1)):
            if w and w not in found:
                found.append(w)
    return found


CHAR = re.compile(r"'(?:\\.|[^\\'])'")


def without_tests(text):
    """(行番号, 行) の列から `#[cfg(test)]` の付いた item（mod tests・fn・static・use・enum の腕・field）だけを除く。除いた所には
    `#[cfg(test)]` の 1 行（印）を残す = 前の doc はそこで切れる（test の物の「Wave:」は数えない）。
    前は最初の `#[cfg(test)]` で file の残りを全部捨て、impl の中の test 用の fn の後ろの公開の口が索引から黙って消えていた（ISSUES B29: generated.rs の
    poll_asks より後ろ = agent_app・agent_rows・…）。item の終わり = 深さ 0 の `;` か `,`・深さ 0 から開いた `{` が閉じた所・外の括弧が閉じる手前。"""
    lines = list(enumerate(text.split("\n"), 1))
    out, i = [], 0
    while i < len(lines):
        no, line = lines[i]
        st = line.lstrip()
        if not st.startswith("#[cfg(test)]"):
            out.append((no, line))
            i += 1
            continue
        out.append((no, line[: len(line) - len(st)] + "#[cfg(test)]"))
        depth, opened, end = 0, False, None
        for k in range(i, len(lines)):
            s = code(CHAR.sub("'_'", lines[k][1]))
            if k == i:
                s = s.lstrip()[len("#[cfg(test)]"):]
            for ch in s:
                if ch in "([{":
                    opened = opened or (ch == "{" and depth == 0)
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                    if depth < 0:
                        end = k - 1
                        break
                    if depth == 0 and ch == "}" and opened:
                        end = k
                        break
                elif depth == 0 and ch in ";,":
                    end = k
                    break
            if end is not None:
                break
        i = (len(lines) if end is None else end) + 1
    return out


def entries(rel, text):
    """1 file の tag つきの項目: (wave, rel, line, name, sig, state)。`#[cfg(test)]` の item は数えない（`without_tests`）。"""
    lines = without_tests(text)
    rel = rel.replace(os.sep, "/")
    out = []
    module = " ".join(l.lstrip()[3:] for _, l in lines if l.lstrip().startswith("//!"))
    for w in tags(module):
        mod = rel.removeprefix("src/").removesuffix(".rs").removesuffix("/mod").replace("/", "::")
        todo = any("todo!(" in code(l) for _, l in lines)
        out.append((w, rel, 1, mod, "（module）", "todo!() を含む" if todo else "本体あり"))
    doc, doc_line = [], 0
    for idx, (no, line) in enumerate(lines):
        st = line.lstrip()
        if st.startswith("#[cfg(test)]"):
            doc = []
            continue
        if st.startswith("///"):
            doc_line = doc_line if doc else no
            doc.append(st[3:])
            continue
        if doc and (not st or st.startswith("#[")):
            continue
        waves, doc = tags(" ".join(doc)), []
        if not waves:
            continue
        m = ITEM.match(line)
        v = None if m else VARIANT.match(line)
        if not m and not v:
            raise SystemExit(f"interfaces: {rel}:{doc_line} の「Wave:」の doc の後ろに item が無い（{st[:60]}）")
        kind, name = (m.group(1), m.group(2)) if m else ("variant", v.group(1))
        sig, body = item_at(lines, idx)
        outer = enclosing(lines, idx)
        full = f"{outer}::{name}" if outer and kind in ("fn", "variant", "const", "type") else name
        state = fn_state(body) if kind == "fn" and body is not None else "宣言"
        out += [(w, rel, no, full, sig, state) for w in waves]
    return out


def fn_state(body):
    """fn の本体の状態: todo!() だけ（`let _ = …;` は数えない）= todo!()・todo!() と他の中身が混ざる = 一部 todo!()・todo!() が無い = 本体あり。"""
    stmts = [l.strip(" {}\t") for l in body]
    stmts = [l for l in stmts if l and not l.startswith("let _ =")]
    todo = [l for l in stmts if "todo!(" in l]
    if not todo:
        return "本体あり"
    return "todo!()" if len(todo) == len(stmts) else "一部 todo!()"


def scan(root=ROOT):
    out, src = [], os.path.join(root, "src")
    for d, dirs, fs in os.walk(src):
        dirs[:] = sorted(x for x in dirs if os.path.join(d, x) != os.path.join(src, "bin"))
        for f in sorted(fs):
            if f.endswith(".rs"):
                p = os.path.join(d, f)
                out += entries(os.path.relpath(p, root), open(p, encoding="utf-8", errors="replace").read())
    return out


def render(items, order):
    """(書き出す本文, §0 に無い Wave)。"""
    unknown = sorted({it[0] for it in items if it[0] not in order})
    by = {}
    for it in items:
        by.setdefault(it[0], []).append(it)
    waves = [w for w in order if w in by]
    todo = lambda its: sum(1 for it in its if it[5] == "todo!()")
    cell = lambda s: s.replace("|", "\\|")
    rows = [
        "# YC の公開インターフェイスの一覧",
        "",
        "<!-- 生成物: python3 -I scripts/interfaces.py が src の doc comment の「Wave:」から作る。手で書かない（scripts/lint.sh の --check が古いと赤）。 -->",
        "",
        "owner 21 通目「interfaceを前提にまず全て作ってから必要になったら足し引きしていく形がいい。みやすく、全てを参照しやすく統合しまくってrepo管理を軽くしよう。」の索引。",
        "契約の正本は各 path:line の doc comment（呼ぶ側の例・保証する / しない）。Wave の並びは `docs/YC_WAVES.md` §0 の表の順で、本体・試験・実機の状態は §0 を見る。",
        "状態: `todo!()` = fn の本体が todo!() だけ・`一部 todo!()` = 中身の一部（新しい腕など）が todo!()・`本体あり` = 中身が在る・`宣言` = 型・定数・trait の口・enum の腕・`todo!() を含む` = module の中に todo!() が在る。",
        "",
        f"項目 {len(items)} 行・Wave {len(waves)} 個・todo!() {todo(items)} 行。",
        "",
        "| Wave | 項目 | todo!() |",
        "|---|---|---|",
    ]
    rows += [f"| [{w}](#{w.lower()}) | {len(by[w])} | {todo(by[w])} |" for w in waves]
    for w in waves:
        rows += ["", f"## {w}", "", "| path:line | 名前 | 署名 | 状態 |", "|---|---|---|---|"]
        rows += [f"| `{rel}:{no}` | `{cell(name)}` | `{cell(sig)}` | {state} |" for _, rel, no, name, sig, state in sorted(by[w], key=lambda it: it[1:3])]
    return "\n".join(rows) + "\n", unknown


def self_test():
    rs = (
        "//! 説明\n//! Wave: W2\nuse x;\n"
        "/// a\n/// Wave: W1, W3\n#[derive(Debug)]\npub struct A {\n    x: u8,\n}\n"
        "/// Wave: W3\npub fn f(a: u8,\n    b: u8) -> Result<(), E> {\n    todo!(\"x {{ ; }}\")\n}\n"
        "/// Wave: W3\npub const S: &str = \"a;b{\";\n"
        "/// Wave: W3\npub fn g() -> u8 { 1 }\n"
        "/// Wave: W3\npub fn m(x: u8) -> u8 {\n    match x {\n        0 => 1,\n        _ => todo!(\"new arm\"),\n    }\n}\n"
        "pub enum E {\n    /// Wave: W3\n    Add { spec: S },\n}\n"
        "impl A {\n    /// Wave: W1\n    pub fn new() -> A {\n        todo!()\n    }\n}\n"
        "pub trait T {\n    /// Wave: W1\n    fn h(&self) -> u8;\n}\n"
        "/// 普通の doc\npub fn untagged() {}\n"
        "#[cfg(test)]\nmod t {\n    /// Wave: W9\n    fn hidden() {}\n}\n"
        # B29: test の物の後ろの公開の口（impl の中の test 用 fn・top level の static・enum の test の腕・文字の '{' を持つ test の本体）
        "impl A {\n    #[cfg(test)]\n    pub(crate) fn put(&mut self) {\n        if true { let c = '{'; }\n    }\n"
        "    /// Wave: W1\n    pub fn after_test_fn(&self) -> u8 {\n        1\n    }\n}\n"
        "#[cfg(test)] static LOCK: Mutex<()> = Mutex::new(());\n"
        "/// Wave: W9\n#[cfg(test)]\nfn tagged_test_helper() {}\n"
        "/// Wave: W3\npub fn after_mod() -> u8 { 2 }\n"
        "pub enum F {\n    #[cfg(test)]\n    Probe,\n    /// Wave: W3\n    Real,\n}\n"
    )
    got = entries("src/x.rs", rs)
    have = {(w, n, s) for w, _, _, n, _, s in got}
    want = {
        ("W2", "x", "todo!() を含む"),
        ("W1", "A", "宣言"), ("W3", "A", "宣言"),
        ("W3", "f", "todo!()"), ("W3", "S", "宣言"), ("W3", "g", "本体あり"), ("W3", "m", "一部 todo!()"), ("W3", "E::Add", "宣言"),
        ("W1", "A::new", "todo!()"), ("W1", "T::h", "宣言"),
        ("W1", "A::after_test_fn", "本体あり"), ("W3", "after_mod", "本体あり"), ("W3", "F::Real", "宣言"),
    }
    assert have == want, f"拾い方が違う: 余分 {have - want} / 足りない {want - have}"
    sigs = {n: s for _, _, _, n, s, _ in got}
    assert sigs["f"] == "pub fn f(a: u8, b: u8) -> Result<(), E>", sigs["f"]
    assert sigs["S"] == 'pub const S: &str = "a;b{"', f"文字列の中の ; と {{ で切った: {sigs['S']}"
    assert [no for w, _, no, n, _, _ in got if n == "f"] == [11], "path:line が item の行でない"
    _, unknown = render(got, ["W1", "W2"])
    assert unknown == ["W3"], f"§0 に無い Wave を拾わない: {unknown}"
    # B29: 索引（--check が比べる本文）に test の物の後ろの口が載る = 索引がその口を欠けば --check は赤
    text, _ = render(got, ["W1", "W2", "W3"])
    for name in ("A::after_test_fn", "after_mod", "F::Real"):
        assert f"| `{name}` |" in text, f"test の物の後ろの {name} が索引に無い（--check が測らない）"
    assert "hidden" not in text and "tagged_test_helper" not in text, "test の物を索引に載せた"
    try:
        entries("src/x.rs", "/// Wave: W1\nlet x = 1;\n")
        raise AssertionError("item の無い tag を通した")
    except SystemExit:
        pass
    order = wave_order()
    assert order[:2] == ["W1", "W2"] and "G-final" in order and "W19-3b" in order, f"§0 の表を読めない: {order[:5]}"
    print("interfaces: self-test ok")


def main():
    if "--self-test" in sys.argv:
        return self_test()
    text, unknown = render(scan(), wave_order())
    if unknown:
        sys.exit("interfaces: docs/YC_WAVES.md §0 の表に無い Wave の tag（打ち間違いか、§0 に行を足す）: " + ", ".join(unknown))
    n = text.count("\n| `")
    if "--check" in sys.argv:
        old = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        if old != text:
            sys.exit("interfaces: docs/YC_INTERFACES.md が src の「Wave:」と合っていない。python3 -I scripts/interfaces.py で書き直して同じ commit に入れる")
        return print(f"interfaces: ok（{n} 行）")
    with open(OUT, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"interfaces: {os.path.relpath(OUT, ROOT)} を書いた（{n} 行）")


if __name__ == "__main__":
    main()
