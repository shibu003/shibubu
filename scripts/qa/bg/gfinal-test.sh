#!/bin/bash
# gfinal-test.sh — gfinal-bg.sh / gfinal-tally.py の検査（画面に触らない・.app も cargo も起こさない）。
# 負の対照: ① 表から 1 本消すと dry-run の本数が減る ② 存在しない driver を指すと exit 1 ③ lies 1 の偽の run を混ぜると表が赤（exit 1）④ llm が上限を超えると exit 1。正の対照: lies 0 の run だけなら exit 0。
# DEMO-2: ⑤ driver を 1 行 '-' に戻すと未測が 2 → 3 に増える ⑥ drive-b4 は既定で gh を呼ばない（dry-run の分岐で gh を呼ぶ写しは赤）⑦ drive-i3 は held-out の文を log・stdout に書かない（文を書く写しは赤）⑧ driver rc 6 は未測（rc 1 は赤）。
set -u
BG="$(cd "$(dirname "$0")" && pwd)"; T=$(mktemp -d "${TMPDIR:-/tmp}/gfinal-test.XXXX"); trap 'rm -rf "$T" "$BG"/drive-*.mut.py "$BG"/gfinal-tally.mut.py' EXIT
pass=0; fail=0
check() { if [ "$2" = "$3" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL $1: want $2 got $3" >&2; fi; }
count() { GFINAL_PLAN=$1 bash "$BG/gfinal-bg.sh" --dry-run 2>&1 | grep -o '^起こす run [0-9]*' | grep -o '[0-9]*$'; }
P="$BG/gfinal-plan.tsv"
check "dry-run counts the real plan" 17 "$(count "$P")"
awk -F'\t' '$1!="3"' "$P" > "$T/less.tsv"; check "removing a row drops the count" 16 "$(count "$T/less.tsv")"
python3 -I -c 'import sys;sys.stdout.write(open(sys.argv[1]).read().replace("W5/drive-w5.py","W5/no-such.py"))' "$P" > "$T/bad.tsv"
GFINAL_PLAN="$T/bad.tsv" bash "$BG/gfinal-bg.sh" --dry-run >/dev/null 2>&1; check "missing driver -> 1" 1 $?
awk -F'\t' 'BEGIN{OFS="\t"} $1=="1"{$4=11} {print}' "$P" > "$T/llm.tsv"
GFINAL_PLAN="$T/llm.tsv" bash "$BG/gfinal-bg.sh" --dry-run >/dev/null 2>&1; check "llm over cap -> 1" 1 $?
bash -n "$BG/gfinal-bg.sh"; check "bash -n" 0 $?
printf '1\tW5\titem\t0\t60\ttrue\t-\n' > "$T/one.tsv"
mkrun() { mkdir -p "$T/out/1/run/ax"; echo '{"schema":"shibubu.qa.run/1"}' > "$T/out/1/run/run.json"; echo '{"status":"ran","rc":0,"secs":5}' > "$T/out/1/meta.json"; echo "$1" > "$T/out/1/run/ax/s.json"; }
mkrun '{"role":"AXWindow","children":[{"role":"AXStaticText","value":"Hello"}]}'
python3 -I "$BG/gfinal-tally.py" table "$T/one.tsv" "$T/out" "$T/clean.md" >/dev/null; check "clean run -> 0" 0 $?
mkrun '{"role":"AXWindow","children":[{"role":"AXStaticText","value":"接続済み"}]}'
python3 -I "$BG/gfinal-tally.py" table "$T/one.tsv" "$T/out" "$T/lie.md" >/dev/null; check "lies 1 run -> 1" 1 $?
check "lie row says 赤 / lies 1" 1 "$(grep -c '| 1 | W5 | item | 1 | 0 | 0 | 5 | 赤 |' "$T/lie.md")"
# ⑤ 未測は G ⑥ と REA の 2 件だけ
unm() { GFINAL_PLAN=$1 bash "$BG/gfinal-bg.sh" --dry-run 2>&1 | grep -o 'driver 無しの未測 [0-9]*' | grep -o '[0-9]*$'; }
check "unmeasured is 2 (G6 + REA)" 2 "$(unm "$P")"
awk -F'\t' 'BEGIN{OFS="\t"} $1=="16"{$6="-"} {print}' "$P" > "$T/nodrv.tsv"; check "row 16 without driver -> unmeasured 3" 3 "$(unm "$T/nodrv.tsv")"
# ⑥ drive-b4: 偽の gh を PATH の先頭に置き、呼ばれたら印を残す
mkdir -p "$T/bin"; printf '#!/bin/sh\ntouch "%s/gh-called"\nexit 1\n' "$T" > "$T/bin/gh"; chmod +x "$T/bin/gh"
PATH="$T/bin:$PATH" python3 -I "$BG/drive-b4.py" "$T/b4run" /nonexistent.app >/dev/null 2>&1; check "drive-b4 default -> exit 6 (dry-run)" 6 $?
check "drive-b4 default never calls gh" 0 "$([ -e "$T/gh-called" ] && echo 1 || echo 0)"
python3 -I -c 'import sys;s=open(sys.argv[1]).read();open(sys.argv[2],"w").write(s.replace("    if \"--really\" not in a:\n","    if \"--really\" not in a:\n        subprocess.run([\"gh\", \"api\", \"x\"])\n"))' "$BG/drive-b4.py" "$BG/drive-b4.mut.py"
PATH="$T/bin:$PATH" python3 -I "$BG/drive-b4.mut.py" "$T/b4mut" /nonexistent.app >/dev/null 2>&1
check "mutant drive-b4 that calls gh is caught" 1 "$([ -e "$T/gh-called" ] && echo 1 || echo 0)"
# ⑦ drive-i3: 文を log に書かない
S='取り違えない合言葉-held-out-dummy'; printf '# c\n%s\n' "$S" > "$T/held.txt"
leak() { { python3 -I "$1" "$T/$2" /nonexistent.app --heldout "$T/held.txt" --dry-run; cat "$T/$2/drive.log"; } 2>&1 | grep -cF -- "$S"; }
check "drive-i3 dry-run exit 0" 0 "$(python3 -I "$BG/drive-i3.py" "$T/i3x" /nonexistent.app --heldout "$T/held.txt" --dry-run >/dev/null 2>&1; echo $?)"
check "drive-i3 never writes the sentence" 0 "$(leak "$BG/drive-i3.py" i3a)"
python3 -I -c 'import sys;s=open(sys.argv[1]).read();open(sys.argv[2],"w").write(s.replace("log(\"i3: heldout sha8\", sha8)","log(\"i3: heldout\", S)"))' "$BG/drive-i3.py" "$BG/drive-i3.mut.py"
check "mutant drive-i3 that logs the sentence is caught" 1 "$([ "$(leak "$BG/drive-i3.mut.py" i3b)" -ge 1 ] && echo 1 || echo 0)"
# ⑧ driver rc 6 は未測・rc 1 は赤
mkrun '{"role":"AXWindow","children":[{"role":"AXStaticText","value":"Hello"}]}'
echo '{"status":"ran","rc":6,"secs":5}' > "$T/out/1/meta.json"
python3 -I "$BG/gfinal-tally.py" table "$T/one.tsv" "$T/out" "$T/rc6.md" >/dev/null; check "driver rc 6 -> 6 (unmeasured)" 6 $?
echo '{"status":"ran","rc":1,"secs":5}' > "$T/out/1/meta.json"
python3 -I "$BG/gfinal-tally.py" table "$T/one.tsv" "$T/out" "$T/rc1.md" >/dev/null; check "driver rc 1 -> 1 (red)" 1 $?
# oracle/project.py: generate・Page 2 枚・surface_specs 1 つ → ok / origin が generate でない → ok でない
mkdb() { mkdir -p "$T/d"; rm -f "$T/d/kangae.sqlite3"; python3 -I -c 'import sqlite3,json,sys;c=sqlite3.connect(sys.argv[1]);c.executescript("CREATE TABLE projects(id TEXT,json TEXT,created_at TEXT);CREATE TABLE project_revisions(project_id TEXT,revision INT,origin TEXT,created_at TEXT);");c.execute("INSERT INTO projects VALUES(?,?,?)",("p",json.dumps({"goal":"g","pages":[1,2],"surface_specs":[1]}),"2026-10-25T00:00:00.000Z"));c.execute("INSERT INTO project_revisions VALUES(?,?,?,?)",("p",1,sys.argv[2],"2026-10-25T00:00:00.000Z"));c.commit()' "$T/d/kangae.sqlite3" "$1"; }
okof() { python3 -I "$BG/oracle/project.py" read "$T/d" --since 0 | python3 -I -c 'import json,sys;print(json.load(sys.stdin)["ok"])'; }
mkdb generate; check "project oracle: generate -> True" True "$(okof)"
mkdb import; check "project oracle: origin import -> False" False "$(okof)"
# ⑨ [任意] の行だけが未測なら exit 0・任意でない行が未測なら 6（印を読まない写しは赤）
printf '1\tW19-3\t[任意] 本物の REA\t0\t0\t-\t別の worktree\n' > "$T/opt.tsv"
printf '1\tW19-3\t本物の REA\t0\t0\t-\t別の worktree\n' > "$T/req.tsv"
python3 -I "$BG/gfinal-tally.py" table "$T/opt.tsv" "$T/out" "$T/opt.md" >/dev/null; check "optional-only unmeasured -> 0" 0 $?
check "optional row counted as 任意未測" 1 "$(grep -c '任意未測 1' "$T/opt.md")"
python3 -I "$BG/gfinal-tally.py" table "$T/req.tsv" "$T/out" "$T/req.md" >/dev/null; check "required unmeasured -> 6" 6 $?
sed 's/^OPT = .*/OPT = "\0never"/' "$BG/gfinal-tally.py" > "$BG/gfinal-tally.mut.py"
python3 -I "$BG/gfinal-tally.mut.py" table "$T/opt.tsv" "$T/out" "$T/mut.md" >/dev/null; check "mutant that ignores the 任意 mark is caught (-> 6)" 6 $?
echo "gfinal-test: pass $pass fail $fail"; [ "$fail" -eq 0 ]
