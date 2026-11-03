"""Rust の source を読むだけの小さな lexer（ui-strings.py と check_wired.py が共有する）。python3 標準 library だけ。"""
import re

PH = "\x01"  # 文字列 literal の中身の代わりに置く 1 字（括弧や // を含む文を構文として読まないため）


def lex(src):
    """-> (masked, literals)。masked = comment を空白に・string literal の中身を PH に置き換えた同じ長さの文字列。
    literals = [(start, end, value)]（start/end は引用符を含む位置・value は escape を解かない中身）。"""
    n, i = len(src), 0
    out = list(src)
    lits = []

    def blank(a, b, keep=None):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = keep if keep else " "

    while i < n:
        c = src[i]
        if c == "/" and src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif c == "/" and src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', src[i:i + 12]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            m = re.match(r'r(#*)"', src[i:])
            close = '"' + m.group(1)
            a = i + m.end()
            j = src.find(close, a)
            j = n if j < 0 else j
            lits.append((i, j + len(close), src[a:j]))
            blank(a, j, PH)
            i = j + len(close)
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            lits.append((i, j + 1, src[i + 1:j]))
            blank(i + 1, j, PH)
            i = j + 1
        elif c == "'":
            m = re.match(r"'(\\.[^']*|[^\\'])'", src[i:i + 12])  # 'x' '\n' '"'。lifetime ('a) は match しない
            if m:
                blank(i + 1, i + m.end() - 1, PH)
                i += m.end()
            else:
                i += 1
        else:
            i += 1
    return "".join(out), lits


def match_close(masked, open_pos):
    """masked[open_pos] の ( [ { に対応する閉じの位置。"""
    pairs = {"(": ")", "[": "]", "{": "}"}
    o, c = masked[open_pos], pairs[masked[open_pos]]
    depth = 0
    for k in range(open_pos, len(masked)):
        if masked[k] == o:
            depth += 1
        elif masked[k] == c:
            depth -= 1
            if depth == 0:
                return k
    return len(masked) - 1


def blank_tests(masked):
    """`#[cfg(test)]` が付いた item（mod / fn / use）を空白にした masked を返す。"""
    out = list(masked)
    for m in re.finditer(r"#\[cfg\(test\)\]", masked):
        a = m.start()
        b = re.search(r"[{;]", masked[m.end():])
        if not b:
            continue
        p = m.end() + b.start()
        end = match_close(masked, p) if masked[p] == "{" else p
        for k in range(a, end + 1):
            if out[k] != "\n":
                out[k] = " "
    return "".join(out)


def line_of(src, pos):
    return src.count("\n", 0, pos) + 1
