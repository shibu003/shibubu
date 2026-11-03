#!/bin/bash
# oracle/github.sh — 試験 repo の Issue の comment を gh api で読み戻す（B4・WebView とも製品の src/github.rs とも別の道）。
#
# 呼ぶ側の使い方:
#   bash scripts/qa/bg/oracle/github.sh comments <owner>/shibubu-yc-demo <issue> <nonce> > R/oracle/github.before.json
#   … WebView で comment …
#   bash scripts/qa/bg/oracle/github.sh comments <owner>/shibubu-yc-demo <issue> <nonce> > R/oracle/github.after.json
# 出力（shibubu.qa.github/1）: {schema, t, repo, issue, nonce, comments:[{id, created_at, body_sha256, has_nonce}], has_nonce}
# 保証する: 読むだけ（GET）。本文は sha256 と has_nonce だけ残す（本文そのものは evidence に書かない）。token は GH_TOKEN（REGISTRY の既存の物）。
# 保証しない: 書く道（comment を作る）は持たない。API の 404 / 401 は exit 4 で、has_nonce=false の緑にしない。
# exit: 0 読めた / 2 使い方 / 4 API の失敗（gh が 0 以外・JSON でない）

set -u
case "${1:-}" in
  comments) [ $# -eq 4 ] || { echo "github.sh comments <repo> <issue> <nonce>" >&2; exit 2; }
            [ -n "$4" ] || { echo "github.sh: nonce が空" >&2; exit 2; }
            case $3 in ''|*[!0-9]*) echo "github.sh: issue は整数" >&2; exit 2;; esac
            raw=$(gh api --paginate "repos/$2/issues/$3/comments") || { echo "github.sh: gh api に失敗（$2#$3）" >&2; exit 4; }
            printf '%s' "$raw" | python3 -I -c '
import hashlib,json,sys,time
repo,issue,nonce=sys.argv[1],int(sys.argv[2]),sys.argv[3]
txt=sys.stdin.read()
try:  # --paginate は頁ごとの配列を続けて出す
    pages,dec,i=[],json.JSONDecoder(),0
    while i<len(txt.strip()):
        v,i=dec.raw_decode(txt.strip(),i); pages+=v; i+=len(txt.strip()[i:])-len(txt.strip()[i:].lstrip())
except Exception as e: print("github.sh: JSON でない: %s"%e,file=sys.stderr); sys.exit(4)
cs=[{"id":c["id"],"created_at":c["created_at"],"body_sha256":hashlib.sha256((c.get("body") or "").encode()).hexdigest(),"has_nonce":nonce in (c.get("body") or "")} for c in pages]
print(json.dumps({"schema":"shibubu.qa.github/1","t":time.time(),"repo":repo,"issue":issue,"nonce":nonce,"comments":cs,"has_nonce":any(c["has_nonce"] for c in cs)},sort_keys=True))' "$2" "$3" "$4" ;;
  *) echo "usage: github.sh comments <repo> <issue> <nonce>" >&2; exit 2 ;;
esac
