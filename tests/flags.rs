//! src/flags.rs（層⑦ feature flag）の test。src/main.rs への mod 行は統合役が足すので、ここは #[path] で取り込む。
#[path = "../src/flags.rs"]
mod flags;
use flags::Flags;
use std::path::PathBuf;

/// test ごとの flags.json。body が None なら file を作らない（無い場合の test）。
fn flags_file(tag: &str, body: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kangae-flags-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("flags.json");
    let _ = std::fs::remove_file(&path);
    if let Some(body) = body {
        std::fs::write(&path, body).unwrap();
    }
    path
}

#[test]
fn env_wins_over_file() {
    let path = flags_file("env-wins", Some(r#"{"composer": true, "rail": false}"#));
    let f = Flags::from_sources(Some("composer=off"), Some(&path));
    assert!(!f.enabled("composer", true), "env の off が file の true に勝つ");
    assert!(!f.enabled("rail", true), "env に無い flag は file の値");
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
}

#[test]
fn broken_file_is_ignored_with_a_warning() {
    let path = flags_file("broken", Some("{not json"));
    let f = Flags::from_sources(Some("rail=on"), Some(&path));
    assert!(f.enabled("composer", true) && !f.enabled("composer", false), "壊れた file の flag は既定値");
    assert!(f.enabled("rail", false), "file が壊れていても env は効く");
    assert_eq!(f.warnings.len(), 1, "{:?}", f.warnings);
    assert!(f.warnings[0].contains("壊れた JSON"), "{:?}", f.warnings);
}

#[test]
fn unknown_flag_uses_the_callers_default() {
    let path = flags_file("default", Some(r#"{"rail": true}"#));
    let f = Flags::from_sources(None, Some(&path));
    assert!(f.enabled("nope", true));
    assert!(!f.enabled("nope", false));
    assert!(f.enabled("rail", false));
}

#[test]
fn missing_file_is_silent() {
    let path = flags_file("missing", None);
    let f = Flags::from_sources(None, Some(&path));
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    assert!(f.enabled("rail", true) && !f.enabled("rail", false));
}

#[test]
fn bad_entries_are_skipped_one_by_one() {
    let path = flags_file("bad-entries", Some(r#"{"a": "yes", "b": true, "c": 1}"#));
    let f = Flags::from_sources(Some(" d = ON ,e=maybe,,f=0,=on"), Some(&path));
    assert!(f.enabled("b", false));
    assert!(f.enabled("d", false), "大文字と前後の空白を許す");
    assert!(!f.enabled("f", true));
    assert!(f.enabled("a", true) && !f.enabled("a", false), "bool でない file の値は既定値");
    assert!(f.enabled("e", true) && !f.enabled("e", false), "読めない env の項目は既定値");
    assert_eq!(f.warnings.len(), 4, "a / c / e / =on の 4 つ: {:?}", f.warnings);
}

#[test]
fn load_reads_env_and_the_home_file() {
    // process の env を書き換える test はこの 1 本だけ（他の test は from_sources なので env を読まない）
    let home = std::env::temp_dir().join(format!("kangae-flags-home-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".kangae")).unwrap();
    std::fs::write(home.join(".kangae/flags.json"), r#"{"x": true, "y": true}"#).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("KANGAE_FLAGS", "y=off");
    let f = Flags::load();
    assert!(f.enabled("x", false), "~/.kangae/flags.json を読む");
    assert!(!f.enabled("y", true), "env が file に勝つ");
}
