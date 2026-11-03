#!/bin/bash
# merge-preview.sh — merge の下見（読むだけ・checkout / merge / ref の書き込み無し・cargo 無し）。
# 使い方: bash scripts/qa/merge-preview.sh [--json] <ours> <theirs> [<theirs2> …]
#   ours に theirs を順に重ねた時（git merge-tree --write-tree。2 つ目以降は 1 つ前の結果の tree を ours にする）の
#   ① 衝突 file（hunk 数・両側の最後の commit）② 衝突しないが同じ file に同じ名前の定義（fn / const / static / struct / enum / trait / type）が
#   両側から入る候補（E0428 の型）③ 両側が触った同じ file（衝突 0 でも compile の罠の候補。同じ fn の型の食い違いはここまでしか出せない）を markdown の表で出す。
# exit: 0 衝突 0 かつ重複定義の候補 0 / 1 衝突か候補あり / 2 使い方・git が古い・rev が無い
set -u
GV=$(git --version 2>/dev/null | sed -E 's/[^0-9]*([0-9]+)\.([0-9]+).*/\1 \2/')
read -r GMAJ GMIN <<<"$GV"
if [ "${GMAJ:-0}" -lt 2 ] || { [ "${GMAJ:-0}" -eq 2 ] && [ "${GMIN:-0}" -lt 38 ]; }; then echo "merge-preview: git 2.38+ が要る（今: ${GV:-無し}）" >&2; exit 2; fi
exec python3 -I - "$@" <<'PY'
import json, os, re, subprocess, sys

args = sys.argv[1:]
as_json = False
if args and args[0] == "--json":
    as_json, args = True, args[1:]
if len(args) < 2:
    sys.stderr.write("使い方: merge-preview.sh [--json] <ours> <theirs> [<theirs2> …]\n")
    sys.exit(2)

ENV = dict(os.environ, GIT_AUTHOR_NAME="mp", GIT_AUTHOR_EMAIL="mp@invalid", GIT_COMMITTER_NAME="mp", GIT_COMMITTER_EMAIL="mp@invalid")

def git(*a, ok=(0,)):
    r = subprocess.run(["git", *a], capture_output=True, env=ENV)
    if r.returncode not in ok:
        sys.stderr.write("merge-preview: git %s: %s\n" % (" ".join(a[:3]), r.stderr.decode(errors="replace").strip()))
        sys.exit(2)
    return r.stdout.decode(errors="replace")

def short(rev): return git("rev-parse", "--short", rev).strip()
def commit_of(rev): return git("rev-parse", "--verify", rev + "^{commit}").strip()
def last(rev, f): return git("log", "-1", "--format=%h", rev, "--", f).strip() or "-"

DEF = re.compile(r"^\+\s*(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:fn|const|static|struct|enum|trait|type)\s+(\w+)")
def added_names(base, side, f):
    out = git("diff", "--no-color", "-U0", base, side, "--", f)
    return {m.group(1) for l in out.splitlines() if (m := DEF.match(l))}

ours0 = args[0]
cur = commit_of(ours0)
steps = []
for th in args[1:]:
    thc = commit_of(th)
    base = git("merge-base", cur, thc, ok=(0, 1)).strip()
    r = subprocess.run(["git", "merge-tree", "--write-tree", "--name-only", cur, thc], capture_output=True, env=ENV)
    if r.returncode not in (0, 1):
        sys.stderr.write("merge-preview: merge-tree: %s\n" % r.stderr.decode(errors="replace").strip()); sys.exit(2)
    lines = r.stdout.decode(errors="replace").split("\n")
    tree = lines[0]
    conflicted = []
    for l in lines[1:]:
        if not l: break
        conflicted.append(l)
    conf = []
    for f in conflicted:
        blob = subprocess.run(["git", "show", "%s:%s" % (tree, f)], capture_output=True, env=ENV).stdout.decode(errors="replace")
        conf.append({"file": f, "hunks": sum(1 for l in blob.splitlines() if l.startswith("<<<<<<< ")),
                     "ours_last": last(cur, f), "theirs_last": last(thc, f)})
    ours_files = set(git("diff", "--name-only", base, cur).split()) if base else set()
    theirs_files = set(git("diff", "--name-only", base, thc).split()) if base else set()
    both = sorted(ours_files & theirs_files)
    dups = []
    for f in both:
        for n in sorted(added_names(base, cur, f) & added_names(base, thc, f)):
            dups.append({"file": f, "name": n})
    same = [f for f in both if f not in conflicted]
    steps.append({"theirs": th, "theirs_short": short(thc), "conflicts": conf, "dups": dups, "same_files": same})
    # 次の段の ours = この結果の tree を持つ合成 commit（object を書くだけ・ref は書かない）
    cur = git("commit-tree", tree, "-p", cur, "-p", thc, "-m", "merge-preview").strip()

nc = sum(len(s["conflicts"]) for s in steps)
nd = sum(len(s["dups"]) for s in steps)
ns = sum(len(s["same_files"]) for s in steps)
summary = "衝突 %d file / 重複定義の候補 %d / 同じ file %d" % (nc, nd, ns)
code = 1 if nc or nd else 0

if as_json:
    print(json.dumps({"ours": ours0, "ours_short": short(ours0), "steps": steps, "summary": summary, "exit": code}, ensure_ascii=False, indent=1))
    sys.exit(code)

print("# merge-preview: %s(%s) ← %s\n" % (ours0, short(ours0), " → ".join("%s(%s)" % (s["theirs"], s["theirs_short"]) for s in steps)))
for s in steps:
    print("## 重ねる: %s(%s)\n" % (s["theirs"], s["theirs_short"]))
    print("### ① 衝突 file\n")
    if s["conflicts"]:
        print("| file | hunk | ours 最後 | theirs 最後 |\n|---|---|---|---|")
        for c in s["conflicts"]: print("| %s | %d | %s | %s |" % (c["file"], c["hunks"], c["ours_last"], c["theirs_last"]))
    else: print("なし")
    print("\n### ② 両側が同じ名前の定義を足す（衝突 0 でも compile が落ちる型）\n")
    if s["dups"]:
        print("| file | 名前 |\n|---|---|")
        for d in s["dups"]: print("| %s | %s |" % (d["file"], d["name"]))
    else: print("なし")
    print("\n### ③ 両側が触った同じ file（衝突 0）\n")
    print("\n".join("- " + f for f in s["same_files"]) if s["same_files"] else "なし")
    print()
print("**要約: %s**" % summary)
sys.exit(code)
PY
