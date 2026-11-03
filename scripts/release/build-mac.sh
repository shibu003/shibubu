#!/usr/bin/env bash
# Shibubu の macOS 配布物を作る（Rust の無い Mac に渡す形）。
#   bash scripts/release/build-mac.sh               この Mac の arch だけ（Intel なら x86_64、Apple Silicon なら arm64）
#   bash scripts/release/build-mac.sh --universal   x86_64 + arm64 を lipo で 1 本にする（もう一方の arch は cross build）
#   KANGAE_BIN=<binary> bash scripts/release/build-mac.sh   cargo build を飛ばしてその binary を包む（別の機械の build など）
#   KANGAE_FEATURES=mirror bash scripts/release/build-mac.sh  既定の webview に足す cargo の feature（「,」区切り）。mirror なら Mirror の helper も Contents/Helpers に入れる（配る dmg は mirror 入り・owner 269「A」）
# 1) cargo build --release --features webview〈+ KANGAE_FEATURES〉 --bin shibubu（WebView = wry 0.57 を release の既定に・owner 2026-10-20「Okです」・YC_DECISIONS 11 通目）  2) $APP_NAME.app（= Shibubu.app）の骨（Info.plist・icon・起動口・surface bridge）  3) 署名（keychain に自己署名の「Shibubu Dev」が在ればそれ・無ければ ad-hoc の codesign --sign -・SHIBUBU_SIGN_ID で上書き）  4) hdiutil で $APP_NAME.dmg
# 出来る物: target/release/bundle/Shibubu.app と Shibubu.dmg（CARGO_TARGET_DIR を立てていればその下の release/bundle/）。
# crate（cargo-bundle 等）は足さず、macOS と Xcode Command Line Tools の道具だけで組む（plutil / sips / iconutil / lipo / vtool / codesign / hdiutil）。
# Developer ID 署名と notarization はこの script の外（docs/RELEASE.md §4）。
set -euo pipefail
cd "$(dirname "$0")/../.."

APP_NAME=Shibubu
BUNDLE_ID=ai.shibubu.desktop
EXE=shibubu                                    # Cargo.toml の [package].name
VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)
TARGET=${CARGO_TARGET_DIR:-target}
OUT=$TARGET/release/bundle
APP=$OUT/$APP_NAME.app
DMG=$OUT/$APP_NAME.dmg
# 最小 macOS の正本（Info.plist の LSMinimumSystemVersion）。実測した OS は 14.5 だけなので 14.0 で止める。下げるのは試してから。
# binary 自体の下限は rustc の既定（x86_64 10.12 / arm64 11.0）で、この値より低い。根拠は docs/RELEASE.md §1。
MIN_MACOS=14.0
# release の cargo build は全部 --features webview（W12W-0・owner 2026-10-20「Okです」・src/project/mod.rs の w12w0_release_build_has_webview）。KANGAE_FEATURES はその後ろに足す
FEATURES=${KANGAE_FEATURES:-}
FEAT_ARGS=()
[ -z "$FEATURES" ] || FEAT_ARGS=(--features "$FEATURES")

UNIVERSAL=0
for a in "$@"; do
  case $a in
    --universal) UNIVERSAL=1 ;;
    *) echo "build-mac.sh: 知らない option: ${a}（使えるのは --universal だけ）"; exit 1 ;;
  esac
done

[ "$(uname -s)" = Darwin ] || { echo "build-mac.sh: macOS でだけ走る（codesign / hdiutil が要る）"; exit 1; }
[ -n "$VERSION" ] || { echo "build-mac.sh: Cargo.toml の version が読めない"; exit 1; }
# 最小 macOS を 2 か所に書いて片方だけ直すのを止める（RELEASE.md の行と照合）
grep -qF -e "最小 macOS: ${MIN_MACOS}（" -e "Minimum macOS: ${MIN_MACOS} (" docs/RELEASE.md || { echo "build-mac.sh: docs/RELEASE.md に「最小 macOS: ${MIN_MACOS}（」の行が無い（MIN_MACOS を変えたら RELEASE.md も直す）"; exit 1; }

# 包む binary。.app に入るのは GUI の bin だけなので --bin で絞る（kangae-rec などの CLI は入れない）。
if [ -n "${KANGAE_BIN:-}" ]; then
  [ "$UNIVERSAL" = 0 ] || { echo "build-mac.sh: KANGAE_BIN と --universal は一緒に使えない（lipo 済みの binary を KANGAE_BIN に渡す）"; exit 1; }
  BIN=$KANGAE_BIN
else
  cargo build --release --features webview --bin "$EXE" ${FEAT_ARGS[@]+"${FEAT_ARGS[@]}"}
  BIN=$TARGET/release/$EXE
  if [ "$UNIVERSAL" = 1 ]; then
    case $(uname -m) in
      x86_64) OTHER=aarch64-apple-darwin ;;
      arm64) OTHER=x86_64-apple-darwin ;;
      *) echo "build-mac.sh: 知らない arch: $(uname -m)"; exit 1 ;;
    esac
    command -v rustup >/dev/null || { echo "build-mac.sh: --universal には rustup が要る（$OTHER の std を入れる）"; exit 1; }
    rustup target add "$OTHER"
    cargo build --release --features webview --bin "$EXE" --target "$OTHER" ${FEAT_ARGS[@]+"${FEAT_ARGS[@]}"}
    mkdir -p "$OUT"
    lipo -create -output "$OUT/$EXE-universal" "$BIN" "$TARGET/$OTHER/release/$EXE"
    BIN=$OUT/$EXE-universal
  fi
fi
[ -x "$BIN" ] || { echo "build-mac.sh: binary が無い: $BIN"; exit 1; }
if [ "$UNIVERSAL" = 1 ]; then
  lipo "$BIN" -verify_arch x86_64 arm64 || { echo "build-mac.sh: $BIN に x86_64 と arm64 の両方が無い"; exit 1; }
fi

# 1) .app の骨
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources/en.lproj"  # en.lproj: 画面の言語は英語（UI-1・CFBundleDevelopmentRegion と同じ）
cp "$BIN" "$APP/Contents/MacOS/$EXE"
# MIRROR-QUIET-1101（owner 208「garagebandのappにいくと何回ももう１つのshibubuが起動しようとしてくる」）: 2 つの helper は Contents/Helpers に置き、
# 自分の Info.plist を埋め込む（別の bundle id・LSUIElement）。Contents/MacOS に居ると Shibubu の bundle の一部として扱われ、NSApplication を
# 作った helper が launchservicesd に $BUNDLE_ID として check in した。画面収録と Accessibility の許可は起こした Shibubu の物のまま（責任を持つ process が親）
helper_plist() {  # helper_plist <bundle id> <名前> -> 埋め込む plist の path（stdout）
  hp="$OUT/$2.Info.plist"
  cat > "$hp" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>$1</string>
	<key>CFBundleName</key><string>$2</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSUIElement</key><true/>
</dict>
</plist>
EOF
  plutil -lint "$hp" >/dev/null || { echo "build-mac.sh: $hp が plist でない" >&2; exit 1; }
  echo "$hp"
}
# surface bridge（v0.14 §5.5・bridge/macos-surface）: 元の app の窓を PortalSlot に重ねる口。swiftc（Command Line Tools）で作り、
# Contents/Helpers に置く（src/surface::find_bridge が実行 file の隣 → ../Helpers を見る・AX の許可は Shibubu.app に付く）。package 依存なし。
BRIDGE_SRC=bridge/macos-surface/KangaeSurfaceBridge.swift
BRIDGE=$APP/Contents/Helpers/kangae-surface-bridge
command -v swiftc >/dev/null || { echo "build-mac.sh: swiftc が無い（xcode-select --install で Command Line Tools を入れる）"; exit 1; }
BRIDGE_PLIST=$(helper_plist "$BUNDLE_ID.bridge" kangae-surface-bridge) || exit 1
EMBED_BRIDGE="-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker $BRIDGE_PLIST"
if [ "$UNIVERSAL" = 1 ]; then
  for a in x86_64 arm64; do swiftc -O -swift-version 5 -target "$a-apple-macosx$MIN_MACOS" $EMBED_BRIDGE "$BRIDGE_SRC" -o "$OUT/bridge-$a"; done
  lipo -create -output "$BRIDGE" "$OUT/bridge-x86_64" "$OUT/bridge-arm64"
  rm -f "$OUT/bridge-x86_64" "$OUT/bridge-arm64"
else
  swiftc -O -swift-version 5 -target "$(uname -m)-apple-macosx$MIN_MACOS" $EMBED_BRIDGE "$BRIDGE_SRC" -o "$BRIDGE"
fi
# Mirror の helper（feature mirror の時だけ・src/ui/mirror_view.rs の helper_path が Contents/Helpers を見る）。
# 入っていない .app の Mirror の Page は「映すための部品が入っていません」と出る。
MIRROR=$APP/Contents/Helpers/kangae-mirror-helper
case ",$FEATURES," in *,mirror,*)
  MIRROR_SRC=bridge/macos-mirror/KangaeMirror.swift
  MIRROR_PLIST=$(helper_plist "$BUNDLE_ID.mirror" kangae-mirror-helper) || exit 1
  EMBED_MIRROR="-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker $MIRROR_PLIST"
  if [ "$UNIVERSAL" = 1 ]; then
    for a in x86_64 arm64; do swiftc -O -parse-as-library -target "$a-apple-macosx$MIN_MACOS" $EMBED_MIRROR "$MIRROR_SRC" -o "$OUT/mirror-$a"; done
    lipo -create -output "$MIRROR" "$OUT/mirror-x86_64" "$OUT/mirror-arm64"
    rm -f "$OUT/mirror-x86_64" "$OUT/mirror-arm64"
  else
    swiftc -O -parse-as-library -target "$(uname -m)-apple-macosx$MIN_MACOS" $EMBED_MIRROR "$MIRROR_SRC" -o "$MIRROR"
  fi
  # 埋め込みが入ったか（入らなければ Shibubu の bundle id で check in する元の形に戻る）。otool は section が無くても path の行を出すので、
  # 番地で始まる中身の行を数える（負の対照: /usr/bin/true = 中身の行 0 → 落ちる・埋め込んだ bridge = 在る → 通る を 11-01 に確かめた）
  otool -s __TEXT __info_plist "$MIRROR" | grep -q '^[0-9a-f]\{8,\}[[:space:]]' || { echo "build-mac.sh: $MIRROR に __info_plist が無い" >&2; exit 1; } ;;
esac
otool -s __TEXT __info_plist "$BRIDGE" | grep -q '^[0-9a-f]\{8,\}[[:space:]]' || { echo "build-mac.sh: $BRIDGE に __info_plist が無い" >&2; exit 1; }
# 起動口。Finder / open から起動した app は CWD が / で PATH が最小なので、PATH を補ってから本体を exec する。
# 保存先は src の `surface::data_dir`（FX1）が env 無しでも同じ ~/Library/Application Support/$APP_NAME に決める。下の KANGAE_DATA_DIR は前の build と揃えるだけ。
# 起動口の名前は本体と大文字小文字だけ違ってはいけない（APFS の既定は区別しないので Shibubu が shibubu を上書きし、exec が自分を呼び続ける。
# 2026-10-19 統合 2: リネーム 7bd6bbc の後の build で本体が 231 byte の起動口になり codesign --verify が落ちた）
LAUNCHER=$APP_NAME-launcher
cat > "$APP/Contents/MacOS/$LAUNCHER" <<EOF
#!/bin/sh
export KANGAE_DATA_DIR="\${KANGAE_DATA_DIR:-\$HOME/Library/Application Support/$APP_NAME}"
mkdir -p "\$KANGAE_DATA_DIR"
export PATH="\$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:\$PATH"
# MIRROR-HYBRID-1101 の測定（operator 11-02）: Finder / Dock から起こすと stderr は /dev/null（launchd）で、本体の 1 行（触ってから本物が前に出るまでの ms 等）が
# どこにも残らない。stderr が /dev/null（端末でない char device）の時だけ data の app.log に足す = 試験の open --stderr（file）と端末から起こした時はそのまま。
# 起こすごとに時刻の見出しを 1 行（本体の行に時刻は無い = 見出しで回を分ける）・5 MB を超えたら 1 つ前（app.log.1）へ回す（溜め続けない）
if [ -c /dev/fd/2 ] && [ ! -t 2 ]; then
  LOG="\$KANGAE_DATA_DIR/app.log"
  SIZE=\$(wc -c < "\$LOG" 2>/dev/null | tr -d ' ')
  [ "\${SIZE:-0}" -gt 5000000 ] && mv -f "\$LOG" "\$LOG.1"
  echo "--- launched \$(date '+%Y-%m-%d %H:%M:%S')" >> "\$LOG"
  exec 2>> "\$LOG"
fi
exec "\$(dirname "\$0")/$EXE" "\$@"
EOF
chmod +x "$APP/Contents/MacOS/$LAUNCHER"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleDisplayName</key><string>$APP_NAME</string>
	<key>CFBundleExecutable</key><string>$LAUNCHER</string>
	<key>CFBundleIconFile</key><string>$APP_NAME</string>
	<key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>$APP_NAME</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSAppleEventsUsageDescription</key><string>Shibubu asks the original app to save the document you are mirroring when you press ⌘S.</string>
	<key>NSMicrophoneUsageDescription</key><string>Shibubu records your humming when you press Record humming. Nothing is sent anywhere.</string>
</dict>
</plist>
EOF
printf 'APPL????' > "$APP/Contents/PkgInfo"
plutil -lint "$APP/Contents/Info.plist"
# UI-1 Done 4: 画面の言語は英語（CFBundleDevelopmentRegion = en と en.lproj）。片方だけ戻したら build を落とす（負の対照: 上の string を ja → exit 1）
REGION=$(plutil -extract CFBundleDevelopmentRegion raw "$APP/Contents/Info.plist")
[ "$REGION" = en ] && [ -d "$APP/Contents/Resources/en.lproj" ] || { echo "build-mac.sh: CFBundleDevelopmentRegion=${REGION}（en でない）か en.lproj が無い" >&2; exit 1; }

# 1b) THIRD-PARTY-NOTICES（RULE-OSS）: 依存の license を検査し、全依存の名前・版・license と LICENSE 本文を Resources に置く。赤・置けなかったら build を落とす
python3 -I scripts/release/third_party_notices.py --out "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt" || { echo "build-mac.sh: THIRD-PARTY-NOTICES を作れなかった（license の赤・上の行）" >&2; exit 1; }
[ -s "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt" ] || { echo "build-mac.sh: THIRD-PARTY-NOTICES.txt が空か無い" >&2; exit 1; }

# 2) icon（LOOK-1101・owner 11-01 の絵 = 黒の角丸に黄のレモン）: assets/icon/shibubu-icon-black.svg から 1 回描いた 1024px の PNG
#    （assets/icon/shibubu-icon-black-1024.png・両方 commit 済み）を sips / iconutil で 10 サイズの icns にする。build は macOS の道具だけ
#    （rsvg-convert は Homebrew なので使わない）。絵を替えたら PNG を描き直して両方を commit する:
#    rsvg-convert -w 1024 -h 1024 assets/icon/shibubu-icon-black.svg -o assets/icon/shibubu-icon-black-1024.png
#    PNG が無い・1024 でない・icns が出来ない → build を落とす（汎用 icon の .app を配らない）。
ICON_PNG=assets/icon/shibubu-icon-black-1024.png
ICON_W=$(sips -g pixelWidth "$ICON_PNG" 2>/dev/null | awk '/pixelWidth/{print $2}' || true)
[ "$ICON_W" = 1024 ] || { echo "build-mac.sh: ${ICON_PNG} が無いか 1024px でない（${ICON_W:-無し}）" >&2; exit 1; }
ICONSET=$OUT/$APP_NAME.iconset
rm -rf "$ICONSET"; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$ICON_PNG" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z "$((s * 2))" "$((s * 2))" "$ICON_PNG" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/$APP_NAME.icns"
rm -rf "$ICONSET"
[ -s "$APP/Contents/Resources/$APP_NAME.icns" ] || { echo "build-mac.sh: $APP_NAME.icns が出来ない" >&2; exit 1; }

# 3) 署名（配布用の Developer ID 署名は docs/RELEASE.md §4）。--deep は使わず内側の本体 → bundle の順に署名する。
# 手元の証明書「Shibubu Dev」（自己署名）が keychain に在ればそれで署名する: ad-hoc は build ごとに cdhash が変わり
# Accessibility・画面収録の許可が外れるが、証明書の署名なら許可が build を跨いで残る。無ければ ad-hoc。SHIBUBU_SIGN_ID で上書き。
SIGN_ID=${SHIBUBU_SIGN_ID:-$(security find-certificate -c "Shibubu Dev" >/dev/null 2>&1 && echo "Shibubu Dev" || echo -)}
codesign --force --sign "$SIGN_ID" "$APP/Contents/MacOS/$EXE"
codesign --force --sign "$SIGN_ID" "$BRIDGE"
[ ! -f "$MIRROR" ] || codesign --force --sign "$SIGN_ID" "$MIRROR"
codesign --force --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# 4) dmg（Applications への link を同梱。初めての人は Shibubu.app を drag して入れる）
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG"   # -quiet にしない（失敗の原文を CI の log に残す）
rm -rf "$STAGE"
[ -f "$DMG" ] || { echo "build-mac.sh: $DMG が出来なかった"; exit 1; }

echo "build-mac.sh: ok  version $VERSION  最小 macOS ${MIN_MACOS}（Info.plist）"
echo "  $APP"
echo "  $DMG ($(du -h "$DMG" | cut -f1))"
echo "  sha256 $(shasum -a 256 "$DMG" | cut -d' ' -f1)"
for a in $(lipo -archs "$APP/Contents/MacOS/$EXE"); do
  echo "  $a: binary の minos $(vtool -arch "$a" -show-build "$APP/Contents/MacOS/$EXE" | awk '$1=="minos"||$1=="version"{print $2; exit}')"
done
echo "  bridge: $(lipo -archs "$BRIDGE")（Contents/Helpers/kangae-surface-bridge）"
codesign -dv "$APP" 2>&1 | grep -E '^(Identifier|Signature)=' | sed 's/^/  /'
