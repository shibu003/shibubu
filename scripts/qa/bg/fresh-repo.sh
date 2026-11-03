#!/bin/bash
# fresh-repo.sh — 試験の repo（主デモの「この repo」）を run ごとに初期の状態で用意する（YC_WAVES §3・試験の側の道具・製品は clone も token も持たない）。
#
# 呼ぶ側の使い方:
#   GH_TOKEN=<REGISTRY の既存の token> bash scripts/qa/bg/fresh-repo.sh mirror     # 1 回だけ: $HOME/shibubu-yc-runs/.mirror を作る（network と token はここだけ）
#   bash scripts/qa/bg/fresh-repo.sh clone "$R"                                    # run ごと: mirror から R/shibubu-yc-demo へ clone し yc-initial に置く → R/repo.json
#   GH_TOKEN=… bash scripts/qa/bg/fresh-repo.sh seed --approved <初期の中身の dir> # 1 回だけ（10-20 の ① の承認の後）: 初期の commit と tag yc-initial を作って push
#   bash scripts/qa/bg/fresh-repo.sh init "$R"                                     # W12a: mirror が無い時の run の clone（network・token・承認 0）
#
# init（W12a・$HOME/shibubu-yc-runs/.mirror が無く、mirror / seed は network・token・owner の承認が要るので使わない時）:
#   R/shibubu-yc-demo に `git init -b main` → README.md（1 行・run の名前）を書いて commit（author shibubu-qa・GIT_CONFIG_GLOBAL=/dev/null と
#   GIT_CONFIG_NOSYSTEM=1 で owner の設定〈hook・署名・template〉を読まない）→ tag yc-initial → R/repo.json（origin "init"・repo "local"・mirror ""）。
#   stderr に 1 行「fresh-repo.sh: mirror は使わず git init と README の commit で run の clone を作った: <path>」（実機の段がこの 1 行を log に写す）。
#   保証する: network も token も使わない・HEAD は tag yc-initial・作業木は clean（でなければ exit 4）・既に在る clone 先は上書きしない（exit 4）。
#   保証しない: GitHub の試験の repo と同じ中身（初期の中身は README 1 つ）。
#
# 環境: YC_DEMO_REPO（既定 shibu003/shibubu-yc-demo）・YC_RUNS（既定 $HOME/shibubu-yc-runs）。
# 出力（clone）: R/repo.json = shibubu.qa.repo/1 {schema, t, repo, mirror, path, tag, head_sha, clean}（record.schema.json）。
# 保証する: clone は mirror だけを読み network も token も使わない。HEAD は tag yc-initial と同じ sha・作業木は clean（でなければ exit 4）。
#           token の値は stdout・stderr・repo.json に出さない。seed は --approved が無ければ何も書かず exit 9。
# 保証しない: mirror の鮮度（mirror を作った後の remote の変更は取り込まない）。repo を GitHub に作る事（owner の手・10-20 の ②'）。
#
# exit: 0 ok / 2 使い方 / 3 mirror が無い（先に mirror）/ 4 tag yc-initial が無い・clone が clean でない / 5 GH_TOKEN が無い / 9 承認が無い（seed）/ 70 未実装

set -u
YC_DEMO_REPO="${YC_DEMO_REPO:-shibu003/shibubu-yc-demo}"
YC_RUNS="${YC_RUNS:-$HOME/shibubu-yc-runs}"
MIRROR="$YC_RUNS/.mirror"
TAG=yc-initial

die() { echo "fresh-repo.sh: $1" >&2; exit "${2:-2}"; }
export GIT_TERMINAL_PROMPT=0

cmd_mirror() {
  [ -n "${GH_TOKEN:-}" ] || die "GH_TOKEN が無い" 5
  [ -d "$MIRROR" ] && { echo "fresh-repo.sh: mirror は在る（作り直すなら $MIRROR を消してから）: $MIRROR" >&2; return 0; }
  mkdir -p "$YC_RUNS"
  local tmp="$MIRROR.tmp.$$"
  # gh は GH_TOKEN を環境から読む。token を URL や引数に入れない（ps にも remote の URL にも残さない）。
  gh repo clone "$YC_DEMO_REPO" "$tmp" -- --mirror -q >&2 || { rm -rf "$tmp"; die "gh repo clone --mirror に失敗: $YC_DEMO_REPO" 4; }
  mv "$tmp" "$MIRROR"
  git -C "$MIRROR" rev-parse -q --verify "refs/tags/$TAG^{commit}" >/dev/null || die "mirror に tag $TAG が無い（seed の前？）" 4
}

cmd_clone() {
  local R=$1 d sha head
  [ -d "$MIRROR" ] || die "mirror が無い: 先に mirror" 3
  d="$R/shibubu-yc-demo"
  [ -e "$d" ] && die "clone 先が既に在る（run ごとに新しい run-dir を）: $d" 4
  mkdir -p "$R"
  sha=$(git -C "$MIRROR" rev-parse -q --verify "refs/tags/$TAG^{commit}") || die "mirror に tag $TAG が無い" 4
  git clone -q --no-hardlinks "$MIRROR" "$d" >&2 || die "mirror からの clone に失敗" 4
  git -C "$d" checkout -q -B yc-run "$sha" || die "$TAG に置けない" 4
  head=$(git -C "$d" rev-parse HEAD)
  local clean=true; [ -z "$(git -C "$d" status --porcelain)" ] || clean=false
  python3 -I -c 'import json,sys,time
r,m,p,tag,h,c=sys.argv[1:7]
print(json.dumps({"schema":"shibubu.qa.repo/1","t":time.time(),"repo":r,"mirror":m,"path":p,"tag":tag,"head_sha":h,"clean":c=="true"},sort_keys=True))' \
    "$YC_DEMO_REPO" "$MIRROR" "$d" "$TAG" "$head" "$clean" > "$R/repo.json"
  { [ "$head" = "$sha" ] && [ $clean = true ]; } || die "HEAD が $TAG でない・作業木が clean でない: $d" 4
}

cmd_seed() {
  [ "$1" = --approved ] || die "seed は 10-20 の ① の承認の後だけ（--approved）" 9
  [ -n "${GH_TOKEN:-}" ] || die "GH_TOKEN が無い" 5
  local src=$2 w
  [ -d "$src" ] || die "初期の中身の dir が無い: $src"
  w=$(mktemp -d "${TMPDIR:-/tmp}/yc-seed.XXXX")
  cp -R "$src/." "$w/" && rm -rf "$w/.git"
  git -C "$w" init -q -b main && git -C "$w" add -A \
    && git -C "$w" -c user.name=shibubu-qa -c user.email=qa@localhost commit -q -m "yc-initial: 試験の repo の初期の中身" \
    && git -C "$w" tag "$TAG" || { rm -rf "$w"; die "初期の commit を作れない" 4; }
  # push は 1 回だけ（main と tag）。token は credential helper が実行時に環境から読む（値を config・URL に書かない）。
  # 既に tag が在れば push の前に止める（同じ秒の 2 回目の seed は同じ sha になり、push の拒否に頼ると「最新」で 0 が返る）。
  local auth=(-c credential.helper= -c 'credential.helper=!f() { echo username=x-access-token; echo "password=$GH_TOKEN"; }; f')
  local url="https://github.com/$YC_DEMO_REPO.git"
  if git -C "$w" "${auth[@]}" ls-remote --exit-code -q "$url" "refs/tags/$TAG" >/dev/null 2>&1; then rm -rf "$w"; die "既に $TAG が在る（上書きしない）" 4; fi
  git -C "$w" "${auth[@]}" push -q "$url" main "refs/tags/$TAG" >&2 || { rm -rf "$w"; die "push に失敗（repo が無い・既に $TAG が在る）" 4; }
  echo "fresh-repo.sh: seed $(git -C "$w" rev-parse "$TAG") → $YC_DEMO_REPO" >&2
  rm -rf "$w"
}

cmd_init() {
  local R=$1 d head sha
  d="$R/shibubu-yc-demo"
  [ -e "$d" ] && die "clone 先が既に在る（run ごとに新しい run-dir を）: $d" 4
  mkdir -p "$d" || die "clone 先を作れない: $d" 4
  # owner の設定（hook・署名・template）を読まない git
  qgit() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$d" -c user.name=shibubu-qa -c user.email=qa@localhost "$@"; }
  qgit init -q -b main || die "git init に失敗: $d" 4
  printf '# shibubu-yc-demo\n\nA small sample project for the Shibubu demo.\n\n## Notes\n\n- Ideas go in docs/notes.md\n- Run: %s\n' "$(basename "$R")" > "$d/README.md"
  { qgit add README.md && qgit commit -q -m "yc-initial: 試験の run の repo（mirror を使わず git init）" && qgit tag "$TAG"; } || die "初期の commit を作れない: $d" 4
  head=$(qgit rev-parse HEAD)
  sha=$(qgit rev-parse -q --verify "refs/tags/$TAG^{commit}") || die "tag $TAG が無い" 4
  local clean=true; [ -z "$(qgit status --porcelain)" ] || clean=false
  python3 -I -c 'import json,sys,time
p,tag,h,c=sys.argv[1:5]
print(json.dumps({"schema":"shibubu.qa.repo/1","t":time.time(),"repo":"local","mirror":"","origin":"init","path":p,"tag":tag,"head_sha":h,"clean":c=="true"},sort_keys=True))' \
    "$d" "$TAG" "$head" "$clean" > "$R/repo.json"
  { [ "$head" = "$sha" ] && [ $clean = true ]; } || die "HEAD が $TAG でない・作業木が clean でない: $d" 4
  echo "fresh-repo.sh: mirror は使わず git init と README の commit で run の clone を作った: $d" >&2
}

[ "${BASH_SOURCE[0]}" = "$0" ] || return 0
case "${1:-}" in
  mirror) [ $# -eq 1 ] || die "mirror"; cmd_mirror ;;
  clone)  [ $# -eq 2 ] || die "clone <run-dir>"; cmd_clone "$2" ;;
  seed)   [ $# -eq 3 ] || die "seed --approved <dir>" "$( [ "${2:-}" = --approved ] && echo 2 || echo 9)"; cmd_seed "$2" "$3" ;;
  init)   [ $# -eq 2 ] || die "init <run-dir>"; cmd_init "$2" ;;
  *) die "usage: fresh-repo.sh mirror|clone|seed|init …" ;;
esac
