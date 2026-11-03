# Building and packaging Shibubu (macOS)

This page covers how to build the app bundle and disk image, how to open an unsigned build for the first time, what CI runs, and what notarization would need.

## 1. Platform support

Minimum macOS: 14.0 (`LSMinimumSystemVersion` in `Info.plist`). The value comes from `MIN_MACOS` in `scripts/release/build-mac.sh`, and the script checks that this line matches it.

- The app bundle declares support for macOS 14.0 and later only. Earlier versions of macOS refuse to launch it.
- So far it has been tested only on macOS 14.5 on Intel (x86_64).
- In a universal build, the arm64 slice is cross-compiled. It has not been run on Apple Silicon yet.
- The main binary could load on older systems (the rustc defaults are 10.12 for x86_64 and 11.0 for arm64). The bundle does not claim those versions because nobody has tested them.
- Recording (Record humming) uses `cpal` 0.18, whose README lists macOS 14.2 as its minimum. Recording on 14.0 and 14.1 is untested.

## 2. Build locally

You need Rust stable and the Xcode Command Line Tools (`xcode-select --install`). The tools provide `swiftc`, `codesign`, `hdiutil`, `lipo` and `vtool`. No extra crates such as `cargo-bundle` are needed.

```sh
bash scripts/release/build-mac.sh               # this Mac's architecture only
bash scripts/release/build-mac.sh --universal   # x86_64 + arm64 merged with lipo (the other arch is cross-compiled)
# -> target/release/bundle/Shibubu.app and target/release/bundle/Shibubu.dmg
# KANGAE_BIN=<binary> bash scripts/release/build-mac.sh   # skip cargo build and wrap an existing binary
# KANGAE_FEATURES=mirror bash scripts/release/build-mac.sh  # add cargo features (mirror also bundles the Mirror helper)
```

The released `Shibubu.dmg` is built with `KANGAE_FEATURES=mirror bash scripts/release/build-mac.sh --universal`.

What the script does:

1. Runs `cargo build --release --features webview --bin shibubu`, plus any `KANGAE_FEATURES` (web pages are shown with `wry`). Only the GUI binary goes into the bundle, not the `kangae-*` CLIs. With `--universal`, it also builds the other target, combines the two with `lipo -create`, and checks both slices with `lipo -verify_arch x86_64 arm64`.
2. Assembles `Shibubu.app`, which contains:
   - a small launcher script, `Contents/MacOS/Shibubu-launcher` (the bundle's `CFBundleExecutable`; its name must differ from `shibubu` by more than case, because APFS is case-insensitive by default)
   - the `shibubu` binary
   - the `kangae-surface-bridge` helper, compiled from `bridge/macos-surface` with `swiftc`
   - with `KANGAE_FEATURES=mirror` only: the `kangae-mirror-helper`, compiled from `bridge/macos-mirror`. The released `Shibubu.dmg` includes it.
   - `Info.plist`, with bundle id `ai.shibubu.desktop` and the version from `Cargo.toml`
   - `PkgInfo`, a placeholder icon, and `Resources/THIRD-PARTY-NOTICES.txt` (every dependency with its version, license and license text, written by `scripts/release/third_party_notices.py`, which also fails the build on a license that `deny.toml` does not allow)
3. Signs the inner binaries first and the bundle last, without `--deep`, and then verifies with `codesign --verify --deep --strict`. It uses a keychain certificate named `Shibubu Dev` if there is one (macOS permissions then survive rebuilds), and otherwise signs ad hoc (`codesign --sign -`). `SHIBUBU_SIGN_ID` overrides the choice. Release builds are signed ad hoc (`SHIBUBU_SIGN_ID=-`).
4. Creates a compressed disk image with `hdiutil create -format UDZO`. The image includes a link to `Applications`. The script prints the image's SHA-256 and the minimum OS of each slice.

On a heavily loaded machine, `hdiutil create` can be very slow or fail with `Device not configured`. If that happens, run the script again when the machine is idle.

### Why the bundle has a launcher script

Apps started from Finder run with `/` as the working directory and a minimal `PATH`. The launcher does two things:

- It sets `KANGAE_DATA_DIR` to `~/Library/Application Support/Shibubu`, unless the variable is already set.
- It adds `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin` to `PATH`, so the app can find the AI command-line tools (`claude`, `grok`) installed there.

The app uses the same data directory when the variable is not set.

## 3. Installing and first launch

| Step | Action | Result |
|---|---|---|
| Open `Shibubu.dmg` | Double-click the image | A window shows `Shibubu.app` and `Applications`. Drag the app into `Applications`. |
| First launch | Double-click `Shibubu.app` | Gatekeeper blocks it, because the build is ad-hoc signed and not notarized. See the steps below. |
| Later launches | Double-click | The app opens normally. |

To open the app the first time on macOS 14 (Apple's guide: [Open a Mac app from an unknown developer](https://support.apple.com/guide/mac-help/open-a-mac-app-from-an-unknown-developer-mh40616/mac)):

1. The warning dialog has no Open button. Click `OK`.
2. Then use one of these:
   - **System Settings > Privacy & Security > Security**: click `Open Anyway` next to the message about Shibubu. The button appears for about an hour after the app was blocked.
   - **Finder**: Control-click `Shibubu.app` and choose `Open`.
3. Click `Open` to confirm.

As a last resort, if both options fail, you can run `xattr -dr com.apple.quarantine /Applications/Shibubu.app`. This skips the macOS check, so do it only after you have confirmed that the image's SHA-256 matches the published value.

To read and operate other apps, Shibubu needs the Accessibility permission. To show another app's window inside a Page (Mirror), it also needs Screen Recording; macOS asks the first time. **Settings › Connections** shows each permission and opens the right pane of System Settings.

## 4. CI (`.github/workflows/rust.yml`)

| Job | Runner | What it runs | Status |
|---|---|---|---|
| `macos` | `macos-latest` | `bash scripts/lint.sh`, then `bash scripts/release/build-mac.sh --universal` (without `KANGAE_FEATURES`, so without the Mirror helper), then it uploads `Shibubu.dmg` and `Shibubu.app.zip` as the artifact `Shibubu-macos-<sha>` | Required |
| `check` | `ubuntu-latest`, `windows-latest` | `cargo check` and `cargo test` only | Not supported. The steps use `continue-on-error`, so failures do not block. |

- The workflow zips the `.app` with `ditto -c -k --keepParent` before uploading it, because `upload-artifact` does not keep executable bits or symlinks.
- Each job writes the platform, toolchain version, UTC timestamp and result to the step summary.

## 5. Notarization (not set up yet)

Getting rid of the first-launch block needs a Developer ID signature and notarization. Neither is configured. You would need:

| Requirement | Where to get it |
|---|---|
| Apple Developer Program membership | <https://developer.apple.com/programs/> |
| A Developer ID Application certificate in the keychain | Your Apple Developer account |
| notarytool credentials (an app-specific password or an App Store Connect API key) | `xcrun notarytool store-credentials <profile> --apple-id <Apple ID> --team-id <TEAMID>`. The credentials go in the keychain, never in the repository. |

Outline of the steps (see Apple's [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)):

```sh
ID="Developer ID Application: <Name> (<TEAMID>)"
B=target/release/bundle
codesign --force --options runtime --timestamp --sign "$ID" $B/Shibubu.app/Contents/MacOS/shibubu
codesign --force --options runtime --timestamp --sign "$ID" $B/Shibubu.app/Contents/Helpers/kangae-surface-bridge
codesign --force --options runtime --timestamp --sign "$ID" $B/Shibubu.app/Contents/Helpers/kangae-mirror-helper
codesign --force --options runtime --timestamp --sign "$ID" $B/Shibubu.app
# rebuild the disk image (step 4 of the script), then sign it
codesign --sign "$ID" --timestamp $B/Shibubu.dmg
xcrun notarytool submit $B/Shibubu.dmg --keychain-profile <profile> --wait
xcrun stapler staple $B/Shibubu.dmg
spctl --assess --type open --context context:primary-signature -vv $B/Shibubu.dmg
```

Two things are still untested: whether the app runs under the hardened runtime, and whether notarization accepts the shell-script launcher.
