//! PV0 の反証（module PV0 = PV-026・yc/pv d994741..96a5616 の review・branch yc/review-pv）: AC の外を破りに行く攻撃の試験。
//! `project::acquire` だけを `#[path]` で取り込む（serde_json と std しか使わない）。この crate は cfg(test) なので `pv0_tests` の
//! `listing`・`temp`・`calls`・`URL` を使い、brew は下の `fake`（sh・この Mac に何も入れない・置き場は一時の dir だけ）。本物の brew は叩かない。
//! 承認の前に入る道の Generated の段（review を飛ばす・二度押し・Cancel の後）は `tests/pv0_attack/generated.rs`（generated.rs の試験に include!）。

#[path = "../src/project/acquire.rs"]
#[allow(dead_code)]
mod acquire;

use acquire::pv0_tests::{calls, listing, temp, URL};
use acquire::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 偽の brew（sh）。`info` は `info`（既定 = `dir/info.json` を出す）、`install` は `install` の sh を走らせる（`$app` = `--appdir` の値・
/// `put <dir> <bundle id> <版>` で `<dir>/Godot.app` を作る）。呼ばれた argv は `dir/calls`、渡された HOMEBREW_* / NONINTERACTIVE / SUDO_ASKPASS は `dir/env`。
fn fake(d: &Path, l: &Value, info: &str, install: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(d).unwrap();
    std::fs::write(d.join("info.json"), l.to_string()).unwrap();
    let script = r#"#!/bin/sh
d='@DIR@'
printf '%s\n' "$*" >> "$d/calls"
env | grep -E '^(HOMEBREW_|NONINTERACTIVE|SUDO_ASKPASS)' | sort >> "$d/env"
put() { mkdir -p "$1/Godot.app/Contents" && printf '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>%s</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>' "$2" "$3" > "$1/Godot.app/Contents/Info.plist"; }
case "$1" in
  info) @INFO@ ;;
  install)
    app=''
    for a in "$@"; do case "$a" in --appdir=*) app="${a#--appdir=}" ;; esac; done
    @INSTALL@ ;;
esac
"#
    .replace("@DIR@", &d.display().to_string())
    .replace("@INFO@", info)
    .replace("@INSTALL@", install);
    let brew = d.join("brew");
    std::fs::write(&brew, script).unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    brew
}

const INFO: &str = r#"cat "$d/info.json""#;
const PUT_OK: &str = r#"put "$app" org.godotengine.godot 4.7.2"#;

fn brew_at(name: &str, info: &str, install: &str) -> (Homebrew, PathBuf) {
    let d = temp(name);
    (Homebrew::at(fake(&d, &listing("4.7.2", URL), info, install), d.join("apps")), d)
}

fn approved() -> Candidate {
    candidate(&CATALOG[0], &listing("4.7.2", URL)).unwrap()
}

fn installs(d: &Path) -> usize {
    calls(d).iter().filter(|c| c.starts_with("install")).count()
}

/// 撃つ所 1（承認の card が古い）: review を出した後、承認までの間に Homebrew の listing が script 入り（postflight）に替わった。
/// 照らしたのは review の時だけなので、承認の後の install は「listing の中の script を信じない」（図 §49）を守らずに走る。
/// 期待: 承認の直前に listing を照らし直し、承認した 1 件と違えば入れない（Review に戻す・理由の 1 行）。
#[test]
fn pv0x_a_listing_that_changed_after_the_review_is_not_installed() {
    let (h, d) = brew_at("x-stale", INFO, PUT_OK);
    let Resolution::Review(c) = h.resolve(&CATALOG[0]) else { panic!("review にならない") };
    let mut later = listing("4.7.2", URL);
    later["casks"][0]["artifacts"].as_array_mut().unwrap().push(json!({ "postflight": null }));
    std::fs::write(d.join("info.json"), later.to_string()).unwrap();
    let r = h.install(&c);
    assert_eq!(installs(&d), 0, "承認の後に script 入りに替わった listing を照らし直さずに入れた: {r:?} / calls {:?}", calls(&d));
    assert!(!matches!(r, Installation::Verified(_)), "{r:?}");
}

/// 撃つ所 3（env の別の口）: 親の env の `HOMEBREW_CASK_OPTS`（Homebrew は全ての cask の command に足す）は brew にそのまま届く。
/// argv を縛っても `--no-quarantine` が env から入る（Homebrew 5.0.0 で deprecated・効くかは版次第 = https://brew.sh/2025/11/12/homebrew-5.0.0）。
/// 期待: brew の子に `HOMEBREW_CASK_OPTS` を渡さない。
#[test]
fn pv0x_cask_options_from_the_parent_env_do_not_reach_brew() {
    let (h, d) = brew_at("x-caskopts", INFO, PUT_OK);
    std::env::set_var("HOMEBREW_CASK_OPTS", "--no-quarantine");
    let r = h.install(&approved());
    std::env::remove_var("HOMEBREW_CASK_OPTS");
    let env = std::fs::read_to_string(d.join("env")).unwrap_or_default();
    assert!(!env.contains("HOMEBREW_CASK_OPTS"), "親の env の cask の引数が brew に届いた（review に無い引数）: {env} / {r:?}");
}

/// 撃つ所 3: `SUDO_ASKPASS` が親に在っても brew に渡さない（PV の試験は親に無い時だけ測っていた = 外す行を消しても緑のまま）。
/// 負の対照: `brew_cmd` の `env_remove("SUDO_ASKPASS")` を外す → 赤。
#[test]
fn pv0x_an_askpass_in_the_parent_env_does_not_reach_brew() {
    let (h, d) = brew_at("x-askpass", INFO, PUT_OK);
    std::env::set_var("SUDO_ASKPASS", "/tmp/pv0x-askpass");
    let r = h.install(&approved());
    std::env::remove_var("SUDO_ASKPASS");
    let env = std::fs::read_to_string(d.join("env")).unwrap_or_default();
    assert!(!env.contains("SUDO_ASKPASS"), "管理者の問いに答える口を brew に渡した: {env} / {r:?}");
}

/// 撃つ所 5（画面に /Users の path）: brew の失敗の最後の行は利用者の HOME の下の cache の path をよく含む。`last_line` は切るだけで伏せない。
/// install と info（resolve）の両方の 1 行を見る。期待: HOME（/Users/<名前>）を画面の 1 行に出さない。
#[test]
fn pv0x_brew_errors_on_screen_carry_no_home_path() {
    let home = std::env::var("HOME").unwrap();
    let (h, _) = brew_at(
        "x-home-install",
        INFO,
        r#"echo "Error: Permission denied @ rb_sysopen - $HOME/Library/Caches/Homebrew/downloads/0a1b--Godot_v4.7.2-stable_macos.universal.zip" >&2; exit 1"#,
    );
    let Installation::Failed(line) = h.install(&approved()) else { panic!("Failed にならない") };
    assert!(!line.contains(&home) && !line.contains("/Users/"), "install の 1 行に HOME の path: {line}");
    let (h, _) = brew_at("x-home-info", r#"echo "Error: $HOME/Library/Caches/Homebrew/api/cask.jws.json is not readable" >&2; exit 1"#, PUT_OK);
    let Resolution::Refused(line) = h.resolve(&CATALOG[0]) else { panic!("Refused にならない") };
    assert!(!line.contains(&home) && !line.contains("/Users/"), "resolve の 1 行に HOME の path: {line}");
}

/// 撃つ所 5: brew の出力の生の全文を出さない（stderr の最後の 1 行・200 文字まで）。
/// 負の対照: `last_line` が stderr を全部返す → 前の行が出て赤。
#[test]
fn pv0x_only_the_last_short_line_of_brew_reaches_the_screen() {
    let (h, _) = brew_at(
        "x-noise",
        INFO,
        r#"i=0; while [ $i -lt 40 ]; do echo "==> noise line $i" >&2; i=$((i+1)); done; printf 'Error: %0300d\n' 0 >&2; exit 1"#,
    );
    let Installation::Failed(line) = h.install(&approved()) else { panic!("Failed にならない") };
    assert!(!line.contains("noise line"), "前の行まで出した: {line}");
    let zeros = line.chars().filter(|c| *c == '0').count();
    assert!(zeros > 0 && zeros <= 200, "最後の行を 200 文字で切っていない（0 が {zeros} 個）: {line}");
}

/// 撃つ所 2（model の文字列）: 席の名前に `;`・空白・`--`・path・bundle id・見た目の似た字を混ぜても表を引かない（Install が出ない・argv に入らない）。
/// 負の対照: `entry_for` を大小を問わない「含む」の比べにする → `Godot; rm -rf ~` が表を引いて赤。
#[test]
fn pv0x_model_names_with_shell_or_path_text_never_reach_the_catalog() {
    for bad in [
        "Godot; rm -rf ~",
        "godot --no-quarantine",
        "--godot",
        "godot-mono",
        "Godot\u{0}",
        "../Godot",
        "/Applications/Godot.app",
        "Godot.app",
        "org.godotengine.godot",
        "Godot Engine 4",
        "Gödot",
        "Ｇodot",
        "Godot\u{200b}",
        "",
    ] {
        assert_eq!(entry_for(bad).map(|e| e.cask), None, "{bad:?} が表を引いた");
    }
    for good in ["Godot", "godot", "GODOT", "  Godot Engine\n"] {
        assert_eq!(entry_for(good).map(|e| e.cask), Some("godot"), "{good:?}");
    }
    let h = Homebrew::at(PathBuf::from("/x/brew"), PathBuf::from("/tmp/apps"));
    assert_eq!(h.install_args(&CATALOG[0]), ["install", "--cask", "--appdir=/tmp/apps", "godot"]);
}

/// 撃つ所 2: listing の url の host の照合は userinfo・port・path・大文字の scheme・前の空白で騙されない。
/// 負の対照: `candidate` の `source_hosts.contains` の照合を外す → `github.com@evil.example` で赤。
#[test]
fn pv0x_the_source_host_check_is_not_fooled_by_userinfo_or_path() {
    let e = &CATALOG[0];
    for bad in [
        "https://github.com@evil.example/Godot.zip",
        "https://github.com:443@evil.example/Godot.zip",
        "https://evil.example/github.com/Godot.zip",
        "https://evil.example?github.com",
        "HTTPS://evil.example/Godot.zip",
        " https://github.com/Godot.zip",
        "ftp://github.com/Godot.zip",
        "https://github.com.evil.example/Godot.zip",
    ] {
        assert!(candidate(e, &listing("4.7.2", bad)).is_err(), "{bad} を通した");
    }
    assert!(candidate(e, &listing("4.7.2", "https://github.com:443/godotengine/godot/releases/x.zip")).is_ok());
}

/// 撃つ所 4（読み戻しの嘘）: 時間切れの前に Godot.app が置かれていても、止めた install は「入った」と言わない（途中まで写した物かもしれない）。
/// 負の対照: install の腕の順を替えて readback が合えば先に Verified にする → 赤。
#[test]
fn pv0x_a_timed_out_install_never_says_installed_even_if_the_app_is_there() {
    let (h, _) = brew_at("x-late", INFO, r#"put "$app" org.godotengine.godot 4.7.2; exec sleep 60"#);
    let h = Homebrew { install_limit: Duration::from_secs(5), ..h };
    let r = h.install(&approved());
    assert!(matches!(&r, Installation::Unsure(l) if l.contains("Check again")), "止めた install を入ったと言った: {r:?}");
}

/// 撃つ所 4: 置き場に読めない Godot.app（Info.plist が無い）が在れば、resolve も install も brew を叩かず上書きしない。
/// 負の対照: `readback` の「読めない」を None（無い）にする → resolve が info を叩いて赤。
#[test]
fn pv0x_an_unreadable_app_in_the_way_is_never_overwritten() {
    let (h, d) = brew_at("x-broken", INFO, PUT_OK);
    std::fs::create_dir_all(d.join("apps/Godot.app/Contents")).unwrap();
    let r = h.resolve(&CATALOG[0]);
    assert!(matches!(&r, Resolution::Refused(l) if l.contains("couldn't read")), "{r:?}");
    let i = h.install(&approved());
    assert!(matches!(&i, Installation::Failed(_)), "{i:?}");
    assert_eq!(calls(&d), Vec::<String>::new(), "読めない app の上に brew を叩いた");
}

/// 撃つ所 4: brew が 0 で終わったのに Godot.app が置き場に無い（別の dir に入った）→ Unsure（入ったと言わない・入れ直さない）・Check again も Verified にしない。
/// 負の対照: install の「0 で終わったのに無い」の腕を Failed にする → 赤。
#[test]
fn pv0x_brew_success_without_the_app_in_place_is_unsure() {
    let (h, d) = brew_at("x-elsewhere", INFO, r#"put "$d/elsewhere" org.godotengine.godot 4.7.2"#);
    let r = h.install(&approved());
    assert!(matches!(&r, Installation::Unsure(l) if l.contains("isn't in Applications")), "{r:?}");
    assert!(matches!(h.check(&CATALOG[0]), Installation::Failed(_)));
    assert_eq!(installs(&d), 1);
}

/// 撃つ所 4（読み戻しの嘘）: brew が別の bundle id の Godot.app を置いて 0 で終わった後の 1 行が、入れる前の読みと同じ文（`Found … · Nothing was placed`）で、
/// Homebrew がこの Mac に物を置いた事を言わない。見る性質（operator の頼み・Failed / Unsure の種類と比べない）: 入ったと言わない・
/// `Found` と `Nothing was placed` を言わない・`Check again` を出す（読み戻しだけの次の一手）。
#[test]
fn pv0x_a_wrong_app_installed_by_brew_is_not_reported_as_found() {
    let (h, _) = brew_at("x-wrong", INFO, r#"put "$app" com.example.other 1.0"#);
    let r = h.install(&approved());
    assert!(!matches!(r, Installation::Verified(_)), "別の bundle id の app を入ったと言った: {r:?}");
    // 種類に依らず 1 行を見る（PV の直しで `Installation` に腕が増えても compile が通る）
    let line = format!("{r:?}");
    assert!(!line.contains("Found") && !line.contains("Nothing was placed"), "brew が置いた別の app を『見つけた・何も置いていない』と言った: {line}");
    assert!(matches!(&r, Installation::Unsure(l) if l.contains("Check again")), "Check again を出さない: {r:?}");
}

/// 撃つ所 2（review の嘘）: listing の app の artifact が別名で入れる（`["Godot.app", {"target": "Evil.app"}]`）と、review は
/// `Adds Godot.app to Applications` と言うが入るのは別名。見る性質（operator の頼み）: 承認しても install を 0 回・拒否の 1 行に正式名と別名の
/// 両方・Applications に何も増えない（偽の brew は target の通り Evil.app を置く）。
#[test]
fn pv0x_an_app_artifact_renamed_by_target_is_refused() {
    let mut l = listing("4.7.2", URL);
    for a in l["casks"][0]["artifacts"].as_array_mut().unwrap() {
        if a.get("app").is_some() {
            *a = json!({ "app": ["Godot.app", { "target": "Evil.app" }], "target": "/Applications/Evil.app" });
        }
    }
    let d = temp("x-target");
    let h = Homebrew::at(fake(&d, &l, INFO, r#"mkdir -p "$app/Evil.app/Contents""#), d.join("apps"));
    let r = h.resolve(&CATALOG[0]);
    // review が出たなら利用者は承認する（別名を言わない review の後に何が起きるか）
    let installed = match &r {
        Resolution::Review(c) => Some(h.install(c)),
        _ => None,
    };
    assert_eq!(installs(&d), 0, "別名で入れる listing を入れた: {r:?} → {installed:?}");
    assert!(matches!(&r, Resolution::Refused(l) if l.contains("Godot.app") && l.contains("Evil.app")), "拒否の 1 行に正式名と別名が無い: {r:?}");
    let added: Vec<_> = std::fs::read_dir(d.join("apps")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(added.is_empty(), "Applications に増えた: {added:?}");
}
