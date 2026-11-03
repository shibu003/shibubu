#!/usr/bin/env bash
# pub-scrub.sh <repo path> | --tree <dir> — 公開前に、owner の実名・login 名・mail が全 branch の tree と履歴のどこに在るかを数える(何も消さない)。
# needles(1 行 1 語)は env SCRUB_NEEDLES(改行区切りの値)か ~/.config/shibubu/scrub-needles.txt から読む = repo に実名を書かない。
# 出力: text の hit は「file <TAB> branch 数 <TAB> branch 一覧」、画像は「hit / OCR 不能」の path 一覧(clean は件数のみ)、履歴は needle ごとでなく合計の件数だけ。
# --tree <dir>: git でなく dir の中の file をそのまま検査する(branch・履歴の節は無し。export-public.sh が書き出した tree に使う)。
# 終了: text か画像の hit が 1 件でもあれば 1(=書き出してはいけない)。
set -euo pipefail
tree=0; [ "${1:-}" = --tree ] && { tree=1; shift; }
repo=${1:?usage: pub-scrub.sh <repo path> | --tree <dir>}
cd "$repo"
nf=$(mktemp); trap 'rm -f "$nf"' EXIT
if [ -n "${SCRUB_NEEDLES:-}" ]; then printf '%s\n' "$SCRUB_NEEDLES" > "$nf"
elif [ -f "$HOME/.config/shibubu/scrub-needles.txt" ]; then cp "$HOME/.config/shibubu/scrub-needles.txt" "$nf"
else echo "needles が無い: env SCRUB_NEEDLES か ~/.config/shibubu/scrub-needles.txt" >&2; exit 2; fi
sed -i.bak '/^[[:space:]]*$/d' "$nf"; rm -f "$nf.bak"
[ -s "$nf" ] || { echo "needles が空" >&2; exit 2; }

if [ $tree = 1 ]; then branches=tree; echo "## tree: $repo"
else branches=$(git for-each-ref refs/heads --format='%(refname:short)'); echo "## branch 数: $(echo "$branches" | wc -l | tr -d ' ')"; fi

echo "## text の hit(file / branch 数 / branch)"
if [ $tree = 1 ]; then
  # file 名に needle が在る物も hit
  hits=$({ grep -rIil -F -f "$nf" . || true; find . -type f | grep -i -F -f "$nf" || true; } | sed 's|^\./||; s|$|\ttree|')
else hits=$(for b in $branches; do
  git grep -I -l -i -F -f "$nf" "refs/heads/$b" -- 2>/dev/null | sed "s|^refs/heads/$b:||; s|\$|\t$b|" || true
done); fi
hits=$(printf '%s\n' "$hits" | sed '/^$/d')
if [ -n "$hits" ]; then
  echo "$hits" | awk -F'\t' '{n[$1]++; b[$1]=b[$1] (b[$1]?",":"") $2} END{for(f in n) printf "%s\t%d\t%s\n", f, n[f], b[f]}' | sort
else echo "(なし)"; fi

echo "## 画像(Vision で OCR → needle に当たれば hit・読めなければ OCR 不能。blob ごとに ~/.cache/shibubu/ocr/<sha>.txt へ cache)"
cache="$HOME/.cache/shibubu/ocr"; mkdir -p "$cache"
work=$(mktemp -d); trap 'rm -f "$nf"; rm -rf "$work"' EXIT
# 「sha<TAB>branch<TAB>path」を全 branch から。同じ blob は 1 回だけ取り出す
if [ $tree = 1 ]; then
  find . -type f | grep -i -E '\.(png|jpe?g|gif|heic|webp)$' | sed 's|^\./||' | while IFS= read -r p; do printf '%s\ttree\t%s\n' "$(git hash-object "$p")" "$p"; done > "$work/all.tsv"
else for b in $branches; do git ls-tree -r "refs/heads/$b" | awk -F'[ \t]' -v b="$b" '$2=="blob"{p=$0; sub(/^[^\t]*\t/,"",p); if (tolower(p) ~ /\.(png|jpe?g|gif|heic|webp)$/) print $3 "\t" b "\t" p}'; done > "$work/all.tsv"; fi
cut -f1 "$work/all.tsv" | sort -u > "$work/blobs"
todo=()
while IFS= read -r sha; do
  [ -f "$cache/$sha.txt" ] && continue
  if [ $tree = 1 ]; then cp "$(awk -F'\t' -v s="$sha" '$1==s{print $3; exit}' "$work/all.tsv")" "$work/$sha.img"
  else git cat-file blob "$sha" > "$work/$sha.img"; fi; todo+=("$work/$sha.img")
done < "$work/blobs"
t0=$(date +%s)
if [ ${#todo[@]} -gt 0 ]; then
  swift "$(dirname "$0")/ocr-text.swift" "${todo[@]}" | while IFS=$'\t' read -r f txt; do
    sha=$(basename "$f" .img); printf '%s\n' "$txt" > "$cache/$sha.txt"
  done
fi
echo "OCR: $(wc -l < "$work/blobs" | tr -d ' ') blob(うち新規 ${#todo[@]})・$(( $(date +%s) - t0 )) 秒"
# blob を hit / OCR 不能 / clean に分ける(subshell を避け file で数える)
: > "$work/hit"; : > "$work/err"; : > "$work/clean"
while IFS= read -r sha; do
  f="$cache/$sha.txt"
  if [ ! -s "$f" ] || grep -q '^<ERR' "$f"; then echo "$sha" >> "$work/err"
  elif grep -q -i -F -f "$nf" "$f"; then echo "$sha" >> "$work/hit"
  else echo "$sha" >> "$work/clean"; fi
done < "$work/blobs"
for st in hit err; do
  lbl=hit; [ $st = err ] && lbl="OCR 不能"
  while IFS= read -r sha; do
    awk -F'\t' -v s="$sha" -v st="$lbl" '$1==s{n[$3]++; b[$3]=b[$3] (b[$3]?",":"") $2} END{for(p in n) printf "%s\t%s\t%d branch\t%s\n", st, p, n[p], b[p]}' "$work/all.tsv"
  done < "$work/$st" | sort
done
imghit=$(wc -l < "$work/hit" | tr -d ' ')
echo "画像 blob: hit $imghit / clean $(wc -l < "$work/clean" | tr -d ' ') / OCR 不能 $(wc -l < "$work/err" | tr -d ' ')(blob 単位・同じ画像が複数 path に在れば表は多い)"


if [ $tree = 0 ]; then
echo "## 履歴(全 ref・needle を含む差分を持つ commit 数)"
tot=0
while IFS= read -r n; do
  c=$(git log --all -i -S"$n" --oneline | wc -l | tr -d ' '); tot=$((tot+c))
  echo "needle $(printf '%s' "$n" | wc -c | tr -d ' ') 文字: $c commit"   # 語そのものは出さない
done < "$nf"
echo "合計 $tot"
fi
[ -z "$hits" ] && [ "$imghit" -eq 0 ]
