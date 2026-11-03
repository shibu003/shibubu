#!/bin/bash
# oracle/excalidraw.sh — run の clone（試験の repo）の Excalidraw の file（主デモの docs/arch.excalidraw）を disk から読み戻す（W12W-2 の B1・B3・
# YC_ACCEPTANCE §3。製品の WebView・localStorage・evaluate とも、製品の Terminal とも別の道）。
#
# 呼ぶ側の使い方:
#   bash scripts/qa/bg/oracle/excalidraw.sh read "$R/shibubu-yc-demo/docs/arch.excalidraw" "$N1"        > "$R/oracle/b1.before.excalidraw.json"
#   … Diagram の Page で文字（$N1）を置く → 書き出し → inline の欄で「repo に保存」…
#   bash scripts/qa/bg/oracle/excalidraw.sh read "$R/shibubu-yc-demo/docs/arch.excalidraw" "$N1"        > "$R/oracle/b1.after.excalidraw.json"
#   B3: Shibubu を kill → open -g → 文字（$N2）をもう 1 つ置いて書き出し・保存 →
#   bash scripts/qa/bg/oracle/excalidraw.sh read "$R/shibubu-yc-demo/docs/arch.excalidraw" "$N1" "$N2" > "$R/oracle/b3.after.excalidraw.json"
#   git の側（`git status` に docs/arch.excalidraw が出る）は oracle/git.sh（shibubu.qa.git/1）を同じ手の前後で取る。
#   判定は tally.py: B1 = b1_saved（ORACLE_CHECKS["B1"]）・B3 = b3_kept(b1.after, b3.after)。
#   run.sh の手からは --expect "oracle:excalidraw"（file = R/shibubu-yc-demo/docs/arch.excalidraw・nonce = 手の --needle、SHIBUBU_QA_NONCES〈空白区切り〉が在ればそれ）。
#
# 出力（shibubu.qa.excalidraw/1・record.schema.json の excalidraw）: {schema, t, path, exists, sha256, elements, nonces}
#   exists   = path が普通の file として在る（無ければ false・sha256 / elements は null・nonces は全部 false。before の「まだ無い」はこれで緑の 0 件）
#   sha256   = file の byte の sha256（16 進 64 桁）
#   elements = JSON の elements[] のうち isDeleted が true でない要素の数
#   nonces   = 引数の nonce ごとに {nonce: bool}。isDeleted でない要素の text か originalText に nonce がそのまま在れば true
#              （消した要素・appState・files の中は数えない＝消した文字や名前の欄で緑にしない）
# 保証する: 読むだけ（file を開いて読むだけ・書かない・WebView も Shibubu も起こさない）。在るのに JSON でない・type が "excalidraw" でない・
#           elements が配列でない file は exit 4 で、要素 0 の緑にしない。nonce を 1 つも渡さない呼び出しは exit 2（nonce の無い読みで B1 を緑にしない）。
# 保証しない: 図が画面に出ている事（WebView の中身は wincap と AXWebArea の text・YC_ACCEPTANCE §2）・file が git に入った事（oracle/git.sh）・
#             service の側に残った事（B4 / B5 は W12W-3）。
#
# exit: 0 読めた（在らない file を含む）/ 2 使い方（path が空・nonce が 0 個）/ 4 在るのに Excalidraw の file として読めない

set -u
case "${1:-}" in
  read) [ $# -ge 3 ] || { echo "excalidraw.sh read <file> <nonce>…（nonce は 1 つ以上）" >&2; exit 2; }
        [ -n "$2" ] || { echo "excalidraw.sh: file の path が空" >&2; exit 2; }
        python3 -I - "${@:2}" <<'EOF' ;;
import hashlib,json,os,sys,time
path,nonces=sys.argv[1],sys.argv[2:]
if any(not n for n in nonces): print("excalidraw.sh: 空の nonce",file=sys.stderr); sys.exit(2)
out={"schema":"shibubu.qa.excalidraw/1","t":time.time(),"path":os.path.abspath(path),"exists":os.path.isfile(path),"sha256":None,"elements":None,"nonces":{n:False for n in nonces}}
if out["exists"]:
    raw=open(path,"rb").read()
    try: d=json.loads(raw)
    except Exception as e: print("excalidraw.sh: JSON でない: %s"%e,file=sys.stderr); sys.exit(4)
    if not isinstance(d,dict) or d.get("type")!="excalidraw" or not isinstance(d.get("elements"),list):
        print("excalidraw.sh: type が excalidraw の file でない・elements が配列でない",file=sys.stderr); sys.exit(4)
    live=[e for e in d["elements"] if isinstance(e,dict) and e.get("isDeleted") is not True]
    texts=[t for e in live for t in (e.get("text"),e.get("originalText")) if isinstance(t,str)]
    out.update(sha256=hashlib.sha256(raw).hexdigest(),elements=len(live),nonces={n:any(n in t for t in texts) for n in nonces})
print(json.dumps(out,ensure_ascii=False,sort_keys=True))
EOF
  *) echo "usage: excalidraw.sh read <file> <nonce>…" >&2; exit 2 ;;
esac
