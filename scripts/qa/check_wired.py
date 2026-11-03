#!/usr/bin/env python3
"""Intent / PageIntent の配線の照合（押せる物 → Intent → 処理）。読むだけ。

  python3 -I scripts/qa/check_wired.py             # 表を出す。「処理なし」が 1 つでもあれば exit 1
  python3 -I scripts/qa/check_wired.py --report    # 表を docs/internal/ui-pro/wired.md にも書く
  --root <dir>   src の親 dir（既定 = この repo。負の対照は写しの app.rs の arm を消して --root で回す）

作る側 = src/ui/**・src/generated.rs・src/app.rs の（試験の外の）`Intent::X` / `PageIntent::X` の使用のうち match の arm でない物。
処理する側 = src/app.rs（Intent）・src/generated.rs（PageIntent）の match の arm（`X … =>`・`X | Y =>`）。
状態: 配線済 / 作る側なし（押す物が無い）/ 処理なし（押しても何も起きない・exit 1）/ 未実装の文言（arm が「未実装」「まだ」を返す）。
"""
import argparse
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _rs import lex, blank_tests, match_close, line_of  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
USERS = ["src/ui/**/*.rs", "src/generated.rs", "src/app.rs"]
HANDLERS = {"Intent": "src/app.rs", "PageIntent": "src/generated.rs"}


def load(path):
    src = open(path, encoding="utf-8").read()
    masked, lits = lex(src)
    return src, blank_tests(masked)


def variants(root, enum):
    src, m = load(os.path.join(root, "src/ui/shell.rs"))
    a = re.search(rf"pub enum {enum}\b[^{{]*{{", m)
    body_start = a.end() - 1
    end = match_close(m, body_start)
    out, depth, k = [], 0, body_start + 1
    while k < end:
        ch = m[k]
        if ch in "({[":
            depth += 1
        elif ch in ")}]":
            depth -= 1
        elif depth == 0 and (ch.isupper()) and not m[k - 1].isalnum() and m[k - 1] != "_":
            name = re.match(r"[A-Za-z0-9_]+", m[k:]).group(0)
            out.append(name)
            k += len(name) - 1
        k += 1
    return out


def uses(root, enum, var):
    """-> (producers, arms)。producers = [(file, line)]、arms = [(file, line, body)]。"""
    prods, arms = [], []
    files = sorted({p for t in USERS for p in glob.glob(os.path.join(root, t), recursive=True)})
    pat = re.compile(rf"(?<![A-Za-z0-9_]){enum}::{var}(?![A-Za-z0-9_])")
    for path in files:
        rel = os.path.relpath(path, root)
        src, m = load(path)
        for mm in pat.finditer(m):
            k = mm.end()
            while k < len(m) and m[k].isspace():
                k += 1
            if k < len(m) and m[k] in "({":
                k = match_close(m, k) + 1
                while k < len(m) and m[k].isspace():
                    k += 1
            ls = m.rfind("\n", 0, mm.start()) + 1
            if re.search(r"(if let |while let |matches!\()[^;]*$", m[ls:mm.start()]):  # 見分けているだけで作っていない
                continue
            tail = m[k:k + 2]
            is_arm = tail == "=>" or tail[:1] == "|" and tail != "||" or re.match(r"if\b[^=]*=>", m[k:k + 160]) is not None
            if not is_arm:
                prods.append((rel, line_of(src, mm.start())))
                continue
            # 同じ arm の `A | B =>` の後ろまで進めて body を取る
            e = m.find("=>", k)
            b = e + 2
            while m[b].isspace():
                b += 1
            if m[b] == "{":
                body = src[b:match_close(m, b) + 1]
            else:
                depth, j = 0, b
                while j < len(m):
                    if m[j] in "({[":
                        depth += 1
                    elif m[j] in ")}]":
                        if depth == 0:
                            break
                        depth -= 1
                    elif m[j] == "," and depth == 0:
                        break
                    j += 1
                body = src[b:j]
            arms.append((rel, line_of(src, mm.start()), body))
    return prods, arms


def table(root):
    rows = []
    for enum, handler in HANDLERS.items():
        for v in variants(root, enum):
            prods, arms = uses(root, enum, v)
            arms = [a for a in arms if a[0] == handler or (enum == "Intent" and False)]
            if not arms:
                st = "処理なし"
            elif re.search(r"未実装|まだ", " ".join(a[2] for a in arms)):
                st = "未実装の文言"
            elif not prods:
                st = "作る側なし"
            else:
                st = "配線済"
            if not arms and not prods:
                st = "処理なし"  # 作る側も無いが、処理が無い事を先に言う
            rows.append((f"{enum}::{v}", ", ".join(f"{f}:{l}" for f, l in prods[:3]) + (f" +{len(prods) - 3}" if len(prods) > 3 else "") or "-",
                         ", ".join(f"{f}:{l}" for f, l, _ in arms) or "-", st))
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--root", default=ROOT)
    a = ap.parse_args()
    rows = table(a.root)
    lines = ["| variant | 作る側 | 処理 | 状態 |", "|---|---|---|---|"] + [f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} |" for r in rows]
    text = "\n".join(lines)
    print(text)
    bad = [r for r in rows if r[3] == "処理なし"]
    print(f"\n{len(rows)} variants / 配線済 {sum(r[3] == '配線済' for r in rows)} / 作る側なし {sum(r[3] == '作る側なし' for r in rows)} / "
          f"未実装の文言 {sum(r[3] == '未実装の文言' for r in rows)} / 処理なし {len(bad)}")
    if a.report:
        out = os.path.join(a.root, "docs/internal/ui-pro/wired.md")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        open(out, "w", encoding="utf-8").write("# Intent の配線表（scripts/qa/check_wired.py の出力）\n\n" + text + "\n")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
