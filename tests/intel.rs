//! src/intel（kangae-intel）を fixture で。本物の Spotlight・lsappinfo・bridge は叩かない: mdfind と lsappinfo は
//! KANGAE_INTEL_TOOLS の偽物、窓は偽の bridge（tests/fake-surface-bridge.py）で、どれも呼ばれた引数を log に残す。plutil は本物。
//! CUA_DRIVER_BIN には呼ばれたら log に残す偽物を置く（試験の側の罠・YC W18-1: 製品は読まないので log に cua-driver が出たら赤）
//! （tmp に置いた架空の .app の Info.plist を読むだけで決定的）。fixture の sdef は架空の app で、形は Music.app 1.4.5 の
//! Contents/Resources/com.apple.Music.sdef と同じ。実物（REAPER・Music・Google Chrome・TextEdit）の出力の原文は report に。

#[allow(dead_code)]
#[path = "../src/intel/mod.rs"]
mod intel;

use intel::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const PAD_SDEF: &str = include_str!("fixtures/intel/pad.sdef");
const STANDARD_SDEF: &str = include_str!("fixtures/intel/standard.sdef");
const LEGACY_SUITE: &str = include_str!("fixtures/intel/legacy.scriptSuite.json");
const LEGACY_TERM: &str = include_str!("fixtures/intel/legacy.scriptTerminology.json");

fn include_standard(href: &str) -> Option<String> {
    (href == "file:///System/Library/ScriptingDefinitions/CocoaStandard.sdef").then(|| STANDARD_SDEF.to_string())
}

#[test]
fn sdef_suites_commands_classes() {
    let d = parse_sdef(PAD_SDEF, &include_standard);
    let names: Vec<&str> = d.suites.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Standard Suite", "Pad Suite"], "hidden の suite は外す・dictionary 直下の include は suite として足す");
    let pad = &d.suites[1];
    let cmds: Vec<&str> = pad.commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(cmds, ["count", "search", "send note"], "hidden・comment・CDATA の中の command は数えない");
    assert_eq!(pad.description, "Pad & friends <fixture>");
    assert_eq!(pad.commands[1].description, "search notes for text > 3 chars", "属性値の中の > で tag を閉じない");
    assert_eq!(pad.classes, ["note", "application"]);
    assert_eq!(d.properties, 4, "class と class-extension の property だけ（record-type は数えない）");
    assert_eq!(d.hidden, 2);
}

#[test]
fn sdef_include_reads_only_the_os_dictionaries() {
    let xml = |href: &str| format!(r#"<dictionary><xi:include href="{href}" xpointer="xpointer(/dictionary/suite)"/></dictionary>"#);
    for href in ["file:///etc/hosts", "file:///System/Library/ScriptingDefinitions/../../../etc/hosts", "https://example.com/x.sdef"] {
        assert!(system_include(href).is_none(), "{href}");
        assert!(parse_sdef(&xml(href), &system_include).suites.is_empty(), "{href}");
    }
}

#[test]
fn legacy_script_suite() {
    let (s, properties) = parse_script_suite(&serde_json::from_str(LEGACY_SUITE).unwrap(), &serde_json::from_str(LEGACY_TERM).unwrap());
    assert_eq!(s.name, "Legacy suite");
    assert_eq!(s.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["reload"], "名前は .scriptTerminology の Name");
    assert_eq!(s.commands[0].description, "Reload the document.");
    assert_eq!(s.classes, ["document", "application"]);
    assert_eq!(properties, 3);
}

#[test]
fn gate_is_decided_in_one_place() {
    for e in ["local_read", "local_write", "external_read"] {
        assert_eq!(gate(Some(e)), GateClass::Read, "{e}");
    }
    for e in ["external_write", "external_send", "destructive", "made_up"] {
        assert_eq!(gate(Some(e)), GateClass::Outward, "{e}");
    }
    assert_eq!(gate(None), GateClass::Outward, "作用が分からない物は外に効く扱い");
}

#[test]
fn normalize_and_bundle_shape() {
    assert_eq!(normalize("  REAPER.app ").unwrap(), "REAPER");
    assert_eq!(normalize("Google Chrome").unwrap(), "Google Chrome");
    let long = "x".repeat(121);
    for bad in ["", "   ", ".app", "../etc/passwd", "a\nb", long.as_str()] {
        assert!(normalize(bad).is_err(), "{bad:?}");
    }
    assert!(is_bundle_id("com.apple.Music"));
    assert!(!is_bundle_id("Google Chrome"));
    assert!(!is_bundle_id("Notion.so"));
}

#[test]
fn spotlight_query_escapes() {
    let q = spotlight_query(Some("com.apple.Music"), &["Mu\"si*c\\".to_string()]);
    assert!(q.starts_with("kMDItemCFBundleIdentifier == \"com.apple.Music\" || "), "{q}");
    assert!(q.contains(r#"kMDItemFSName == "Mu\"si\*c\\.app"cd"#), "{q}");
}

#[test]
fn lsappinfo_pid() {
    assert_eq!(parse_lsappinfo_pid("\"pid\"=812\n"), Some(812));
    assert_eq!(parse_lsappinfo_pid(""), None);
    assert_eq!(parse_lsappinfo_pid("\"pid\"=0"), None);
}

#[test]
fn utc_dates() {
    assert_eq!(utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(utc(1_700_000_000), "2023-11-14T22:13:20Z");
}

fn src(read: &str) -> Source {
    Source { read: read.into(), checked_at: "2026-10-17T00:00:00Z".into() }
}

fn pad_app(pid: Option<u32>, asks: &[&str]) -> Probe<App> {
    let value = App {
        path: "/Applications/Pad.app".into(),
        bundle_id: Some("dev.kangae.fixture.pad".into()),
        name: "Pad".into(),
        version: Some("2.3".into()),
        pid,
        asks: asks.iter().map(|s| s.to_string()).collect(),
        other_copies: vec![],
    };
    Probe::Found { value, source: src("Info.plist") }
}

#[test]
fn assemble_dictionary_windows() {
    let windows = Probe::Found { value: vec![AxWindow { id: 7, title: "Untitled".into(), on_screen: true }], source: src("macOS bridge window_list {\"pid\":4242}") };
    let dict = Probe::Found { value: parse_sdef(PAD_SDEF, &include_standard), source: src("Contents/Resources/pad.sdef") };
    let r = assemble("Pad", "2026-10-17T00:00:00Z", pad_app(Some(4242), &["NSMicrophoneUsageDescription"]), dict, windows, vec![]);
    let cap = |id: &str| r.capabilities.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} が無い: {:#?}", r.capabilities));
    assert_eq!(cap("ax.windows.read").gate, GateClass::Read);
    assert_eq!(cap("applescript.count").gate, GateClass::Read);
    assert_eq!(cap("applescript.get").gate, GateClass::Read);
    for id in ["applescript.open", "applescript.delete", "applescript.search", "applescript.send_note"] {
        assert_eq!(cap(id).gate, GateClass::Outward, "{id}: 辞書は作用を書かないので外に効く扱い");
        assert_eq!(cap(id).effect, None, "{id}");
        assert_eq!(cap(id).requires, ["tcc:automation:dev.kangae.fixture.pad"], "{id}");
    }
    assert!(r.capabilities.iter().all(|c| c.id != "applescript.secret_wipe" && c.id != "applescript.debug_dump"), "hidden は試せる事に出さない");
    assert_eq!(r.capabilities.iter().filter(|c| c.id == "applescript.count").count(), 1, "同じ名前の command は 1 つ");
    // service ごとの能力の一覧は製品に持たない（owner 10-23）: 試せる事はこの Mac で読めた物だけ・外の口は未調査と言う
    assert!(r.capabilities.iter().all(|c| c.id.starts_with("ax.") || c.id.starts_with("applescript.")), "{:#?}", r.capabilities);
    assert!(r.unknowns.iter().any(|u| u.what == "Outside this Mac" && u.why.contains("Shibubu looks when it needs to")), "{:#?}", r.unknowns);
    let perm = |id: &str| r.permissions.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("{id} が無い: {:#?}", r.permissions));
    let automation = perm("tcc:automation:dev.kangae.fixture.pad");
    assert!(automation.because.iter().any(|b| b.starts_with("applescript.")), "{automation:#?}");
    assert!(automation.settings.ends_with("Automation"), "{automation:#?}");
    // AI が窓を読む・押すのは Shibubu Bridge（Kangae の process）なので AX の許可は Kangae に（YC W18-1）
    assert!(perm("tcc:accessibility").who.starts_with("Shibubu"), "{:#?}", perm("tcc:accessibility"));
    assert!(perm("tcc:microphone").who.starts_with("Pad itself"));
}

#[test]
fn assemble_without_dictionary_or_running() {
    let dict = Probe::Absent { why: "辞書が無い".into(), source: src("Info.plist") };
    let r = assemble("Pad", "t", pad_app(None, &[]), dict, Probe::Unchecked { why: NOT_RUNNING.into() }, vec![]);
    assert!(r.capabilities.is_empty());
    assert!(r.permissions.is_empty(), "辞書も窓も無ければ権限は出さない");
    let whats: Vec<&str> = r.unknowns.iter().map(|u| u.what.as_str()).collect();
    assert_eq!(whats, ["Outside this Mac", "Windows that accessibility can reach"]);
    assert!(r.unknowns[1].why.contains("it isn't running"));
}

#[test]
fn intel_never_launches_or_sends() {
    let src = include_str!("../src/intel/mod.rs");
    for banned in ["surface::open", "surface::probe", "/usr/bin/open", "osascript", "\"click\"", "type_text", "get_window_state"] {
        assert!(!src.contains(banned), "src/intel は起動・Apple Events・入力をしない: {banned}");
    }
}

// ---- CLI（偽の mdfind / lsappinfo・罠の cua-driver と tmp の架空の .app）

const WINDOWS: &str =
    r#"{"windows":[{"window_id":7,"pid":4242,"app_name":"KangaeFixturePad","title":"Untitled","is_on_screen":true,"on_current_space":true,"z_index":3}]}"#;
const SCRIPTABLE: &str = "<key>OSAScriptingDefinition</key><string>pad.sdef</string>\n<key>NSAppleScriptEnabled</key><true/>";

struct Fake {
    dir: PathBuf,
    app: PathBuf,
}

fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// 偽の command 3 つと架空の .app（Info.plist に plist_extra を足す・sdef があれば Resources/pad.sdef）を tmp に作る。
fn fake(plist_extra: &str, sdef: Option<&str>) -> Fake {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("kangae-intel-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let tools = dir.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    script(&tools, "mdfind", "printf 'mdfind %s\\n' \"$*\" >> \"$FAKE_LOG\"\n[ -n \"$FAKE_SLEEP\" ] && exec sleep \"$FAKE_SLEEP\"\nprintf '%s' \"$FAKE_MDFIND_OUT\"\n");
    script(&tools, "lsappinfo", "printf 'lsappinfo %s\\n' \"$*\" >> \"$FAKE_LOG\"\nprintf '%s' \"$FAKE_PID_OUT\"\n");
    script(
        &tools,
        "cua-driver",
        "printf 'cua-driver %s\\n' \"$*\" >> \"$FAKE_LOG\"\ncase \"$2\" in\n  list_windows) printf '%s' \"$FAKE_WINDOWS\" ;;\n  *) echo '{\"refusal\":{\"code\":\"invalid_arguments\",\"message\":\"intel は list_windows しか呼ばない\"},\"status\":\"refused\"}'; exit 1 ;;\nesac\n",
    );
    let app = dir.join("Apps/KangaeFixturePad.app");
    std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>CFBundleIdentifier</key><string>dev.kangae.fixture.pad</string>\n<key>CFBundleName</key><string>KangaeFixturePad</string>\n<key>CFBundleShortVersionString</key><string>2.3</string>\n{plist_extra}\n</dict></plist>\n"
        ),
    )
    .unwrap();
    if let Some(s) = sdef {
        std::fs::write(app.join("Contents/Resources/pad.sdef"), s).unwrap();
    }
    Fake { dir, app }
}

impl Fake {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kangae-intel"));
        c.args(args)
            .env("KANGAE_INTEL_TOOLS", self.dir.join("tools"))
            .env("CUA_DRIVER_BIN", self.dir.join("tools/cua-driver"))
            // 本物の bridge を拾わない（bridge の道は bridge を渡す test だけ）
            .env("KANGAE_SURFACE_BRIDGE", "")
            .env("FAKE_LOG", self.dir.join("calls.log"))
            .env("HOME", &self.dir)
            .env("FAKE_MDFIND_OUT", format!("{}\n", self.app.display()))
            .env("FAKE_PID_OUT", "")
            .env("FAKE_WINDOWS", WINDOWS)
            .env_remove("FAKE_SLEEP");
        c
    }
    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default()
    }
}

fn stdout(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn json_of(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

#[test]
fn cli_installed_not_running_reads_dictionary_and_never_touches_the_app() {
    let f = fake(&format!("{SCRIPTABLE}\n<key>NSMicrophoneUsageDescription</key><string>fixture</string>"), Some(PAD_SDEF));
    let o = f.cmd(&["dev.kangae.fixture.pad"]).output().unwrap();
    let s = stdout(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.starts_with("KangaeFixturePad — 入っている: "), "{s}");
    assert!(s.contains("（dev.kangae.fixture.pad・版 2.3）"), "{s}");
    assert!(s.contains("(found: With Spotlight)"), "{s}");
    assert!(s.contains("AppleScript の command") && s.contains("send note") && !s.contains("secret wipe"), "{s}");
    assert!(s.contains("Windows that accessibility can reach: Not confirmed (it isn't running)"), "{s}");
    assert!(s.contains("Automation (sends Apple events to dev.kangae.fixture.pad)"), "{s}");
    assert!(s.contains("Microphone — KangaeFixturePad itself"), "{s}");
    assert!(s.contains("Outside this Mac: Pricing, APIs, web access and terms can't be read from an app on this Mac"), "{s}");
    let calls = f.calls();
    assert!(calls.contains("mdfind kMDItemCFBundleIdentifier == \"dev.kangae.fixture.pad\""), "{calls}");
    assert!(calls.contains("lsappinfo info -only pid dev.kangae.fixture.pad"), "{calls}");
    assert!(!calls.contains("cua-driver"), "起動していない app に cua-driver を呼ばない: {calls}");
}

#[test]
fn cli_running_lists_windows_with_list_windows_only() {
    let f = fake(SCRIPTABLE, Some(PAD_SDEF));
    let o = f.cmd(&["dev.kangae.fixture.pad", "--json"]).env("FAKE_PID_OUT", "\"pid\"=4242\n").env("KANGAE_SURFACE_BRIDGE", intel_bridge(&f, "ok")).output().unwrap();
    let v = json_of(&o);
    assert_eq!(v["app"]["state"], "found", "{v}");
    assert_eq!(v["app"]["value"]["pid"], 4242);
    assert_eq!(v["windows"]["state"], "found", "{v}");
    assert_eq!(v["windows"]["value"][0]["title"], "Untitled");
    let caps = v["capabilities"].as_array().unwrap();
    assert!(caps.iter().all(|c| c["gate"] == "read" || c["gate"] == "outward"), "{v}");
    let ax = caps.iter().find(|c| c["id"] == "ax.windows.read").unwrap();
    assert_eq!(ax["gate"], "read");
    assert_eq!(ax["requires"], json!(["tcc:accessibility"]));
    assert!(v["permissions"].as_array().unwrap().iter().any(|p| p["id"] == "tcc:accessibility"), "{v}");
    // 読むだけ: bridge の window_list を 1 回（押す・打つ cmd は送らない）・罠の cua-driver は呼ばない
    let sent = f.calls(); // 偽の bridge も FAKE_LOG（calls.log）に要求を 1 行ずつ書く
    assert_eq!(sent.lines().filter(|l| l.contains("window_list")).count(), 1, "{sent}");
    assert!(!sent.contains("ax_press") && !sent.contains("ax_set_value") && !sent.contains("key_to_pid"), "{sent}");
    assert!(!f.calls().contains("cua-driver"), "{}", f.calls());
}

/// 偽の bridge（tests/fake-surface-bridge.py・窓「Untitled」1 つ）を env つきで起こす sh
fn intel_bridge(f: &Fake, mode: &str) -> PathBuf {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py");
    script(&f.dir, "bridge.sh", &format!("export FAKE_TITLES='Untitled'\nexport FAKE_MODE='{mode}'\nexec python3 '{}'\n", py.display()));
    f.dir.join("bridge.sh")
}

#[test]
fn cli_running_lists_windows_through_the_bridge_without_cua_driver() {
    // CuaDriver が無い Mac: 窓は macOS bridge で読む（route native_docking・許可は Kangae に）
    let f = fake(SCRIPTABLE, Some(PAD_SDEF));
    let o = f
        .cmd(&["dev.kangae.fixture.pad", "--json"])
        .env("FAKE_PID_OUT", "\"pid\"=4242\n")
        .env("CUA_DRIVER_BIN", "")
        .env("KANGAE_SURFACE_BRIDGE", intel_bridge(&f, "ok"))
        .output()
        .unwrap();
    let v = json_of(&o);
    assert_eq!(v["windows"]["state"], "found", "{v}");
    assert_eq!(v["windows"]["value"][0]["title"], "Untitled");
    assert!(v["windows"]["source"]["read"].as_str().unwrap().starts_with("macOS bridge window_list"), "{v}");
    let ax = v["capabilities"].as_array().unwrap().iter().find(|c| c["id"] == "ax.windows.read").unwrap().clone();
    assert_eq!(ax["route"], "native_docking");
    assert!(ax["summary"].as_str().unwrap().contains("macOS bridge"), "{ax}");
    let perm = v["permissions"].as_array().unwrap().iter().find(|p| p["id"] == "tcc:accessibility").unwrap().clone();
    assert!(perm["who"].as_str().unwrap().starts_with("Shibubu"), "{perm}");
    assert!(!f.calls().contains("cua-driver"), "{}", f.calls());
}

/// YC W18-1（YC_GATE #15）: bridge が無い → 窓は未確認（理由）。absent にしない・罠の cua-driver（CUA_DRIVER_BIN）が在っても呼ばない
#[test]
fn cli_running_is_unchecked_without_bridge() {
    let f = fake(SCRIPTABLE, Some(PAD_SDEF));
    let o = f.cmd(&["dev.kangae.fixture.pad", "--json"]).env("FAKE_PID_OUT", "\"pid\"=4242\n").output().unwrap();
    let v = json_of(&o);
    assert_eq!(v["windows"]["state"], "unchecked", "{v}");
    let why = v["windows"]["why"].as_str().unwrap();
    assert!(why.contains("bridge: ") && !why.to_lowercase().contains("cua"), "{why}");
    assert!(v["capabilities"].as_array().unwrap().iter().all(|c| c["id"] != "ax.windows.read"), "読めていない窓を試せる事に出さない: {v}");
    assert!(!f.calls().contains("cua-driver"), "{}", f.calls());
}

#[test]
fn cli_not_installed_says_where_it_looked() {
    let f = fake("", None);
    let o = f.cmd(&["KangaeNoSuchApp"]).env("FAKE_MDFIND_OUT", "").output().unwrap();
    let s = stdout(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.starts_with("KangaeNoSuchApp — Not found on this Mac\n  読んだ所: mdfind "), "{s}");
    assert!(s.contains("~/Applications の 名前.app"), "{s}");
    assert!(s.contains("試せる事:\n  （無し）"), "{s}");
    assert!(s.contains("Outside this Mac: It wasn't found on this Mac, so"), "{s}");
    let calls = f.calls();
    assert!(!calls.contains("lsappinfo") && !calls.contains("cua-driver"), "{calls}");
}

#[test]
fn cli_spotlight_hang_is_unchecked_not_absent() {
    let f = fake(SCRIPTABLE, Some(PAD_SDEF));
    let t = Instant::now();
    let o = f.cmd(&["dev.kangae.fixture.pad", "--timeout", "1", "--json"]).env("FAKE_SLEEP", "30").output().unwrap();
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let v = json_of(&o);
    assert_eq!(v["app"]["state"], "unchecked", "Spotlight が答えないのに「無い」と言わない: {v}");
    assert!(v["unknowns"].as_array().unwrap().iter().any(|u| u["what"] == "Spotlight"), "{v}");
}

#[test]
fn cli_no_dictionary_needs_no_automation() {
    let f = fake("", None);
    let o = f.cmd(&["KangaeFixturePad", "--json"]).output().unwrap();
    let v = json_of(&o);
    assert_eq!(v["app"]["state"], "found", "{v}");
    assert_eq!(v["dictionary"]["state"], "absent", "{v}");
    let s = stdout(&f.cmd(&["KangaeFixturePad"]).output().unwrap());
    assert!(s.contains("  AppleScript の辞書がありません（"), "確かめて無かった事も人向けに出す: {s}");
    assert!(v["permissions"].as_array().unwrap().iter().all(|p| !p["id"].as_str().unwrap().starts_with("tcc:automation")), "{v}");
    assert!(f.calls().contains("kMDItemFSName == \"KangaeFixturePad.app\"cd"), "{}", f.calls());
}

#[test]
fn cli_usage_errors_touch_nothing() {
    let f = fake("", None);
    let cases: [&[&str]; 5] = [&[], &["a", "b"], &["--nope", "x"], &["x", "--timeout", "0"], &["../x"]];
    for args in cases {
        let o = f.cmd(args).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stdout(&o));
    }
    assert!(f.calls().is_empty(), "断った入力で外の command を呼ばない: {}", f.calls());
}
