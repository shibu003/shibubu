#!/bin/bash
# oracle/git.sh — run の clone（試験の repo）の HEAD・branch・変更 file を、試験の道具の shell の git で読み戻す（W12a・
# 製品の画面とも、製品の Terminal の中の git とも別の道。製品の Terminal の出力は製品の画面なので照合に使わない）。
#
# 呼ぶ側の使い方:
#   bash scripts/qa/bg/oracle/git.sh read "$R/shibubu-yc-demo" > "$R/oracle/05-reread.after.git.json"
#   run.sh の手からは --expect "oracle:git"（expect_oracle が R/shibubu-yc-demo を読む・R/oracle/<手>.<before|after>.git.json）
#   画面との照合は tally.py の git_seen（AX の木の data:dev-git-sha・data:dev-git-branch・data:dev-git-changes と比べる）
#
# 出力（shibubu.qa.git/1・record.schema.json の git）: {schema, t, path, head_sha, branch, changes:[{status, path, orig_path?}]}
#   head_sha = `git -C <dir> rev-parse HEAD`（40 桁・commit が無ければ null）
#   branch   = `git -C <dir> branch --show-current`（detached HEAD なら null・空文字にしない）
#   changes  = `git -C <dir> status --porcelain=v1 -z`（並びのまま・status は XY の 2 字そのまま・path は引用符を外した生の path・
#              rename / copy の時だけ orig_path）。製品の adapters::git::RepoStatus と同じ 3 つの読み。
# 保証する: 読むだけ（`--no-optional-locks` で index.lock も取らない）。GIT_DIR・GIT_WORK_TREE・GIT_INDEX_FILE を外して <dir> だけを読む。
#           <dir> が無い・git の repo でない・git が 0 以外は exit 4 で、空の changes の緑にしない。
# 保証しない: 製品が同じ値を出す事（それを測るのが git_seen）・読んだ後の変化・submodule の中身。
#
# exit: 0 読めた / 2 使い方 / 4 <dir> が無い・git の repo でない・git が 0 以外・出力が読めない

set -u
case "${1:-}" in
  read) [ $# -eq 2 ] || { echo "git.sh read <repo-dir>" >&2; exit 2; }
        [ -n "$2" ] || { echo "git.sh: repo-dir が空" >&2; exit 2; }
        python3 -I - "$2" <<'EOF' ;;
import json,os,subprocess,sys,time
d=sys.argv[1]
if not os.path.isdir(d): print("git.sh: dir が無い: %s"%d,file=sys.stderr); sys.exit(4)
env={k:v for k,v in os.environ.items() if k not in ("GIT_DIR","GIT_WORK_TREE","GIT_INDEX_FILE")}
env["LC_ALL"]="C"
def git(*a):
    try: return subprocess.run(["git","--no-optional-locks","-C",d,*a],capture_output=True,env=env,timeout=20)
    except Exception as e: print("git.sh: git を起こせない: %s"%e,file=sys.stderr); sys.exit(4)
def need(p):
    if p.returncode!=0: print("git.sh: git が 0 以外: %s"%p.stderr.decode(errors="replace").strip()[:200],file=sys.stderr); sys.exit(4)
    return p.stdout
if need(git("rev-parse","--is-inside-work-tree")).strip()!=b"true": print("git.sh: work tree でない: %s"%d,file=sys.stderr); sys.exit(4)
raw=need(git("status","--porcelain=v1","-z"))
head=git("rev-parse","-q","--verify","HEAD")
sha=head.stdout.decode().strip() if head.returncode==0 else None
branch=need(git("branch","--show-current")).decode(errors="replace").strip() or None
f=[x for x in raw.split(b"\0") if x]; changes=[]; i=0
while i<len(f):
    x=f[i]; i+=1
    if len(x)<4 or x[2:3]!=b" ": print("git.sh: status の行が読めない",file=sys.stderr); sys.exit(4)
    c={"status":x[:2].decode(),"path":x[3:].decode(errors="replace")}
    if b"R" in x[:2] or b"C" in x[:2]:
        if i>=len(f): print("git.sh: rename の元が無い",file=sys.stderr); sys.exit(4)
        c["orig_path"]=f[i].decode(errors="replace"); i+=1
    changes.append(c)
print(json.dumps({"schema":"shibubu.qa.git/1","t":time.time(),"path":os.path.abspath(d),"head_sha":sha,"branch":branch,"changes":changes},ensure_ascii=False,sort_keys=True))
EOF
  *) echo "usage: git.sh read <repo-dir>" >&2; exit 2 ;;
esac
