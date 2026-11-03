"""driver に残る JP の label の literal を数える（test.sh 用・W15-b34）。

labels.py の表の JP と**完全一致**する文字列（状態の行の key = AXStaticText は部分一致）が、log / print / append の外・docstring の外に在れば 1 件。
usage: python3 -I jp_literals.py <labels.py の dir> <driver.py>...  → 件数の合計を 1 行
       python3 -I jp_literals.py --cjk <labels.py の dir> <judge.py>...  → 判定の側（W15-b36）: 日本語を含む文字列 literal のうち、docstring・log / print / append / raise / assert の文・`_selftest` の入力・ALLOW に無い物の件数。
       app から読む文は labels.py の key で引く事（key の JP と同じ literal は上の数え方でも 1 件）。一覧と区分は docs/evidence/W15/judge-labels.md。
"""
import ast, os, re, sys

CJK_MODE = sys.argv[1:2] == ["--cjk"]
ARGS = sys.argv[2:] if CJK_MODE else sys.argv[1:]
sys.path.insert(0, ARGS[0])
import labels as L

EXACT = {v[1] for v in L.LABELS.values() if v[0] != "AXStaticText"}
SUB = [v[1] for v in L.LABELS.values() if v[0] == "AXStaticText"]


CJK = re.compile("[ぁ-んァ-ヶ一-龥]")
# (b) 利用者が打った文・(c) script 自身の語（verdict・理由）。file の basename → その file で通す部分文字列
ALLOW = {
    "tally.py": ("済み", "L1 の ①", "elements も", " 秒", "ax の木", "surface の oracle", "cua-driver の応答", "start の owner の idle", "owner の idle",
                 "JSON でない", "が居る", "前後の木", "owner の pointer", "手の対象の app", "画面に窓", "の間の赤",
                 "focused element が無い", "owner 不在", "type_text の needle",
                 "確かめました", "repo の場所は保存しました", "録画 mode"),  # (c) 製品の成功の文の印（BACKED_SENTENCES・EN は UI-2c で決まるまで JP）  # (a) 語尾「済み」は 接続済み・保存済み を含む / (c) 未測の理由
    "gfinal-tally.py": ("緑", "赤", "未測", "任意", "走らせていない", "owner 使用中", "tally 不能", "driver が", "G-final", "| run |"),  # (c) 判定の語・表の見出し・理由
    "ui-state.py": (),
}


def count_cjk(path):
    t = ast.parse(open(path, encoding="utf-8").read())
    allow = ALLOW.get(os.path.basename(path), ())
    skip = set()
    for n in ast.walk(t):
        if isinstance(n, ast.Call):
            f = n.func
            if (f.id if isinstance(f, ast.Name) else getattr(f, "attr", "")) in ("log", "print", "append"):
                skip.update(id(c) for c in ast.walk(n))
        if isinstance(n, (ast.Raise, ast.Assert)) or (isinstance(n, ast.FunctionDef) and n.name == "_selftest"):
            skip.update(id(c) for c in ast.walk(n))
        if isinstance(n, (ast.Module, ast.FunctionDef, ast.ClassDef)) and ast.get_docstring(n, clean=False) is not None:
            skip.add(id(n.body[0].value))
    return sum(1 for c in ast.walk(t) if isinstance(c, ast.Constant) and isinstance(c.value, str) and id(c) not in skip
               and CJK.search(c.value) and not any(a in c.value for a in allow))


def count(path):
    t = ast.parse(open(path, encoding="utf-8").read())
    skip = set()
    for n in ast.walk(t):
        if isinstance(n, ast.Call):
            f = n.func
            if (f.id if isinstance(f, ast.Name) else getattr(f, "attr", "")) in ("log", "print", "append"):
                skip.update(id(c) for c in ast.walk(n))
        if isinstance(n, (ast.Module, ast.FunctionDef)) and ast.get_docstring(n, clean=False) is not None:
            skip.add(id(n.body[0].value))
    return sum(1 for c in ast.walk(t) if isinstance(c, ast.Constant) and isinstance(c.value, str) and id(c) not in skip
               and (c.value in EXACT or any(j in c.value for j in SUB)))


if __name__ == "__main__":
    print(sum((count_cjk if CJK_MODE else count)(f) for f in ARGS[1:]))
