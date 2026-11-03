#!/usr/bin/env bash
# export-public.sh <repo> <out dir> [--dry-run] [--ref <ref>] [--evidence-images=mask|exclude|keep] — 履歴を持ち込まず、<ref>(既定 HEAD)の tracked file だけを 1 commit にして書き出す(docs/RELEASE.md §6)。
# 実行は owner の手。<out dir> は無いか空の dir。元の repo には何も書かない・checkout もしない・push しない。
# 順: git archive → docs/evidence の画像を黒塗りの写しに置換(既定 mask・hit の画像だけ。exclude=外す・keep=そのまま) → text の needle を置換(login 名→you・mail→you@example.com・それ以外(実名)→<owner>) → pub-scrub.sh --tree(text も画像 OCR も 0 件) → gitleaks。
# needles は pub-scrub.sh と同じ(env SCRUB_NEEDLES か ~/.config/shibubu/scrub-needles.txt)。作者は env EXPORT_AUTHOR_NAME / EXPORT_AUTHOR_EMAIL(既定は匿名)。
# 終了: 0 = 公開してよい tree / 1 = 残り hit か gitleaks が赤 / 2 = 使い方・needles。--dry-run は一時 dir に展開して検査だけ。
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=${1:?usage: export-public.sh <repo> <out dir> [--dry-run] [--ref <ref>] [--evidence-images=mask|exclude|keep]}; out=${2:?out dir}; shift 2
dry=0; ref=HEAD; imgmode=mask
while [ $# -gt 0 ]; do case $1 in
  --dry-run) dry=1;; --ref) ref=${2:?--ref には ref}; shift;;
  --evidence-images=mask|--evidence-images=exclude|--evidence-images=keep) imgmode=${1#*=};;
  *) echo "知らない引数: $1" >&2; exit 2;; esac; shift; done
cd "$repo"
[ "$ref" != HEAD ] || [ -z "$(git status --porcelain)" ] || { echo "tree が clean でない" >&2; exit 1; }
nf=$(mktemp)
if [ "$dry" = 1 ]; then out=$(mktemp -d)
else mkdir -p "$out"; [ -z "$(ls -A "$out")" ] || { echo "$out が空でない" >&2; exit 1; }; fi
trap 'rm -f "$nf"; [ "$dry" = 1 ] && rm -rf "$out"' EXIT
t0=$(date +%s)
git archive "$ref" | tar -x -C "$out"

if [ -n "${SCRUB_NEEDLES:-}" ]; then printf '%s\n' "$SCRUB_NEEDLES" > "$nf"
elif [ -f "$HOME/.config/shibubu/scrub-needles.txt" ]; then cp "$HOME/.config/shibubu/scrub-needles.txt" "$nf"
else echo "needles が無い" >&2; exit 2; fi
sed -i.bak '/^[[:space:]]*$/d' "$nf"; rm -f "$nf.bak"; [ -s "$nf" ] || { echo "needles が空" >&2; exit 2; }

echo "## 画像(docs/evidence・$imgmode)"
imgs=$(find "$out/docs/evidence" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.gif' -o -iname '*.heic' -o -iname '*.webp' \) 2>/dev/null || true)
case $imgmode in
exclude) if [ -n "$imgs" ]; then printf '%s\n' "$imgs" | tr '\n' '\0' | xargs -0 rm -f; fi; echo "除外した画像: $(printf '%s' "$imgs" | grep -c . || true) 枚";;
mask) # Vision の矩形で needle の所だけ塗る。hit の画像だけ置換(clean は触らない)。png 以外は OCR 後の pub-scrub が拾う
  bin=$(mktemp -d)/ocr-mask; swiftc -O -o "$bin" "$here/../qa/ocr-mask.swift" 2>/dev/null || { echo "ocr-mask の compile に失敗(swiftc)" >&2; exit 1; }
  args=(); while IFS= read -r f; do [ -n "$f" ] && args+=("$f" "$f.masked.png"); done <<< "$imgs"
  rn=0; rr=0
  if [ ${#args[@]} -gt 0 ]; then
    while IFS=$'\t' read -r f st; do case $st in
      masked*) mv "$f.masked.png" "$f"; rn=$((rn+1)); rr=$((rr+${st#masked }));; esac; done < <("$bin" "$nf" "${args[@]}")
  fi
  rm -rf "$(dirname "$bin")"
  echo "黒塗りに置換した画像: $rn 枚(塗った矩形 $rr)・読んだ画像 $(( ${#args[@]} / 2 )) 枚";;
*) echo "そのまま";; esac

echo "## text の置換(file / 件数。長い needle から)"
list=$(grep -rIil -F -f "$nf" "$out" || true)
if [ -n "$list" ]; then
  printf '%s\n' "$list" | NF="$nf" OUT="$out" perl -e '
    open F, "<", $ENV{NF}; chomp(my @n = grep { length } <F>); close F;
    @n = sort { length($b) <=> length($a) } @n;
    my $total = 0;
    while (my $p = <STDIN>) { chomp $p; open I, "<:raw", $p or next; local $/; my $s = <I>; close I; my $c = 0;
      for my $n (@n) { my $r = $n =~ /@/ ? "you\@example.com" : $n =~ /^[A-Za-z0-9._-]+$/ ? "you" : "<owner>"; $c += ($s =~ s/\Q$n\E/$r/gi); }
      next unless $c; open O, ">:raw", $p or die; print O $s; close O; $total += $c; (my $q = $p) =~ s{^\Q$ENV{OUT}\E/}{}; print "$q\t$c\n"; }
    print "置換 合計 $total\n";'
else echo "(なし)"; fi

echo "## 書き出した tree の検査(pub-scrub.sh --tree)"
bash "$here/../qa/pub-scrub.sh" --tree "$out" || { echo "残り hit が在る → 公開してはいけない(exit 1)" >&2; exit 1; }
if command -v gitleaks >/dev/null; then gitleaks dir "$out" -c "$here/gitleaks.toml" --no-banner --redact || { echo "gitleaks が赤" >&2; exit 1; }; else echo "要 install: gitleaks(検査していない)"; fi
if [ "$dry" = 1 ]; then echo "dry-run ok(何も書き出していない・$(( $(date +%s) - t0 )) 秒)"; exit 0; fi
cd "$out"; git init -q -b main
git ls-files -z --others | xargs -0 git add --   # 新しい空 repo なので全 file = 書き出した物だけ
export GIT_AUTHOR_NAME=${EXPORT_AUTHOR_NAME:-kangae} GIT_AUTHOR_EMAIL=${EXPORT_AUTHOR_EMAIL:-noreply@example.invalid}
GIT_COMMITTER_NAME=$GIT_AUTHOR_NAME GIT_COMMITTER_EMAIL=$GIT_AUTHOR_EMAIL git commit -q -m "Initial public snapshot"
gitleaks git . -c "$here/gitleaks.toml" --no-banner --redact || { echo "gitleaks が赤" >&2; exit 1; }
echo "書き出した: $out(1 commit)。push は owner の手"
