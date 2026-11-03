//! adapters（unit AD）の試験。本物の GitHub / Google / bridge は叩かない:
//! HTTP は 127.0.0.1 の偽 server、GUI 操作は偽の bridge（tests/fake-surface-bridge.py）を包む使い捨ての script。
//! bin crate なので main.rs と同じ並びで module を `#[path]` で取り込む（repos.rs の `crate::model` が解ける）。
//! 取り込んだ file の中の `#[cfg(test)]` も一緒に走るが、この file の試験は環境変数を触らない（設定は値で渡す）ので干渉しない。
#![allow(dead_code)]
#[path = "../src/crypto.rs"]
mod crypto;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/github.rs"]
mod github;
#[path = "../src/repos.rs"]
mod repos;
#[path = "../src/google/mod.rs"]
mod google;
// runtime への橋（AdapterRegistry::contract）の test が runtime と storage を使う（runtime は super::storage を引く）
#[allow(dead_code, unused_imports)]
#[path = "../src/storage/mod.rs"]
mod storage;
#[allow(dead_code, unused_imports)]
#[path = "../src/runtime/mod.rs"]
mod runtime;
// adapters::mac_gui が gui の契約の型（GuiTarget・GuiAction・ActReceipt・WindowState）で読み・押し・act の記録を持つ（W17-1・W6b で observe / act / verify は消した）
#[path = "../src/gui/mod.rs"]
mod gui;
// adapters::mac_gui が surface::Bridge を包む（W17-0）
#[allow(dead_code, unused_imports)]
#[path = "../src/surface/mod.rs"]
mod surface;
// ENGINE-AUDIT W4 の口の棚（`mac_gui::app_surfaces` = `intel::investigate` / `read_plist` / `run_deadline`）。intel は serde と std だけ（audio/surface.rs と storage/redact.rs を #[path] で持つ）
#[path = "../src/intel/mod.rs"]
mod intel;
#[path = "../src/adapters/mod.rs"]
mod adapters;
// W17-3: 新旧の比べの表（試験の側の oracle・W18 の回帰の正本）。readback は std と serde_json だけなので ui を引かずに取り込める
#[path = "common/readback.rs"]
mod readback;
#[path = "common/old_new.rs"]
mod old_new;

use adapters::{sdk::{self, Adapter, Route, SourceSync, Spec}, AdapterEvent, AdapterRegistry, Caller, RegisterError};
use domain::capability::AdapterKind;
use domain::action::Effect;
use domain::binding::{BindingReadiness, DataBinding, SourceReadiness};
use domain::capability::{CapabilityReadiness, DisplayTier};
use domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject};
use domain::run::{ErrorCode, Observation, Outcome};
use serde_json::{json, Value};
use std::{fs, io::{Read, Write}, net::{TcpListener, TcpStream}, path::{Path, PathBuf}, sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex}, thread, time::{Duration, Instant}};

// ---------- 偽 HTTP server（tests/google.rs と同じ型。method と path の前方一致で route を選ぶ。status 0 = request を読んだ後に応答せず切る） ----------
#[derive(Debug, Clone)]
struct Req { method: String, path: String, body: String, auth: Option<String> }
type Log = Arc<Mutex<Vec<Req>>>;
fn fake(routes: Vec<(&'static str, &'static str, u16, Value)>) -> (String, Log) {
    fake_with(move |req| routes.iter().find(|(m, p, _, _)| *m == req.method && req.path.starts_with(p)).map(|(_, _, c, b)| (*c, b.to_string())).unwrap_or((404, "{}".to_string())))
}
/// 偽 server の本体。handler が request ごとに (status, body) を決める（状態を持つ偽 server 用。tests/google.rs の fake_with と同じ型）
fn fake_with(handler: impl Fn(&Req) -> (u16, String) + Send + 'static) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let log2 = Arc::clone(&log);
    thread::spawn(move || for conn in l.incoming() {
        let Ok(mut s) = conn else { break };
        let req = read_request(&mut s);
        let (status, body) = handler(&req);
        log2.lock().unwrap().push(req);
        if status == 0 { let _ = s.shutdown(std::net::Shutdown::Both); continue }
        let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    });
    (base, log)
}
fn read_request(s: &mut TcpStream) -> Req {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut buf, mut tmp) = (Vec::new(), [0u8; 4096]);
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p }
        match s.read(&mut tmp) { Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]), _ => break buf.len() }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let start = (head_end + 4).min(buf.len());
    while buf.len() < start + len { match s.read(&mut tmp) { Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]), _ => break } }
    let body = String::from_utf8_lossy(&buf[start..(start + len).min(buf.len())]).to_string();
    Req { method, path, body, auth: headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone()) }
}
fn closed_port() -> String { let l = TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://127.0.0.1:{}", l.local_addr().unwrap().port()) }
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kangae-ad-{tag}-{}-{}", std::process::id(), model::new_id()));
    fs::create_dir_all(&d).unwrap();
    d
}
/// W19-1: AI に任せる同意（試験の人が偽の bridge の app = REAPER に「AI に任せる」を押した状態）を bridge の隣の file に書き、それを読む MacGui。
/// 同意の無い MacGui（`MacGui::at`）は送る前に断る（既定は無し・YC_DECISIONS 11 通目）ので、送る手を試す試験はこれで作る
#[cfg(unix)]
fn gui_at(sh: PathBuf) -> adapters::mac_gui::MacGui {
    let f = sh.with_file_name("act-consents.jsonl");
    surface::grant_act_consent(&f, "com.cockos.reaper", 1.0).unwrap();
    adapters::mac_gui::MacGui::at(sh).with_act_consents(f)
}
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

// ---------- github::Api::create_issue_outcome（批評 safety #1: 12 秒の時間切れを「失敗」と出して重複作成させない） ----------
fn api(base: &str) -> github::Api { github::Api::new(base, Some("tok".into())) }

#[test]
fn create_issue_201_with_number_is_verified_by_source_receipt() {
    let (base, log) = fake(vec![("POST", "/repos/o/r/issues", 201, json!({"number": 7, "html_url": "https://github.com/o/r/issues/7"}))]);
    let o = api(&base).create_issue_outcome("o/r", "件名", "本文");
    assert_eq!(o, Outcome::Verified { evidence: vec!["#7".into(), "https://github.com/o/r/issues/7".into()] });
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].auth.as_deref(), Some("Bearer tok"));
    assert_eq!(serde_json::from_str::<Value>(&log[0].body).unwrap(), json!({"title": "件名", "body": "本文"}));
}

#[test]
fn create_issue_after_send_failures_are_effect_unknown_never_failed() {
    // 応答の前に切れた・5xx: 作られた可能性がある → EffectUnknown（Failed にすると人が再実行して重複する）
    for (status, label) in [(0u16, "切断"), (502, "502")] {
        let (base, _) = fake(vec![("POST", "/repos/o/r/issues", status, json!({"message": "x"}))]);
        let o = api(&base).create_issue_outcome("o/r", "t", "");
        assert!(matches!(&o, Outcome::EffectUnknown { reason } if reason.contains("may have been created")), "{label}: {o:?}");
    }
    // 2xx だが番号が無い: 受理だけ → Unverified
    let (base, _) = fake(vec![("POST", "/repos/o/r/issues", 201, json!({"ok": true}))]);
    assert!(matches!(api(&base).create_issue_outcome("o/r", "t", ""), Outcome::Unverified { reason } if reason.contains("業務上の成功は未検証")));
}

#[test]
fn create_issue_definitive_failures_carry_error_codes() {
    for (status, code) in [(422u16, ErrorCode::InvalidInput), (401, ErrorCode::PermissionDenied), (404, ErrorCode::TargetNotFound), (429, ErrorCode::RateLimited)] {
        let (base, _) = fake(vec![("POST", "/repos/o/r/issues", status, json!({}))]);
        assert_eq!(api(&base).create_issue_outcome("o/r", "t", ""), Outcome::Failed { error: code }, "HTTP {status}");
    }
    // 接続できない = request は出ていない → Failed{ConnectionLost}（再試行できる）
    assert_eq!(api(&closed_port()).create_issue_outcome("o/r", "t", ""), Outcome::Failed { error: ErrorCode::ConnectionLost });
}

#[test]
fn create_issue_rejects_before_sending_without_token_or_with_bad_input() {
    let (base, log) = fake(vec![("POST", "/repos/o/r/issues", 201, json!({"number": 1, "html_url": "u"}))]);
    for (a, repo, title) in [(github::Api::new(&base, None), "o/r", "t"), (api(&base), "o/r/x", "t"), (api(&base), "o/r", " ")] {
        assert!(matches!(a.create_issue_outcome(repo, title, ""), Outcome::Rejected { .. }), "{repo} {title:?}");
    }
    assert_eq!(log.lock().unwrap().len(), 0, "拒否は送る前（request 0 件）");
    // token を他所へ送らない: base は https://api.github.com か loopback だけ
    for evil in ["http://evil.example", "http://127.0.0.1.evil.example", "https://api.github.com.evil.example"] {
        assert_eq!(github::Api::new(evil, Some("t".into())).base, "https://api.github.com", "{evil}");
    }
}

#[test]
fn legacy_create_issue_string_wrapper_keeps_app_rs_contract() {
    // app.rs は Ok を「完了」、Ok かつ「業務上の成功は未検証」を「受理」、Err を「失敗」と出す
    let (base, _) = fake(vec![("POST", "/repos/o/r/issues", 201, json!({"ok": true}))]);
    let o = api(&base).create_issue_outcome("o/r", "t", "");
    let Outcome::Unverified { reason } = o else { panic!("{o:?}") };
    assert!(json!({"verified": false, "reason": reason}).to_string().contains("業務上の成功は未検証"));
}

// ---------- repos::run_trusted_feature_outcome（CD-19: exit 0 は自己申告・features dir の code は実行しない・pipe で止まらない） ----------
fn feature(cli: &Path, local_path: &str) -> model::RepoFeature {
    pinned(model::RepoFeature { id: 9, owner: "a".into(), repo: "b".into(), subdir: "src".into(), commit: "a".repeat(40), license: "MIT".into(),
        local_path: local_path.into(), cli_path: cli.display().to_string(), cli_args: vec![], enabled: true, cli_pin: String::new() })
}
/// 登録した時（app.rs の「CLI接続を登録する」）と同じく、今の実行ファイルと引数の指紋を固定する（ADR-002 硬化 (b)）
fn pinned(f: model::RepoFeature) -> model::RepoFeature {
    model::RepoFeature { cli_pin: repos::cli_pin(&f).unwrap_or_default(), ..f }
}

#[cfg(unix)]
#[test]
fn cli_exit_zero_is_unverified_and_200kb_output_does_not_stall() {
    let d = tmp("cli");
    let cli = script(&d, "big.sh", "head -c 200000 /dev/zero | tr '\\0' 'x'\nexit 0");
    let t = Instant::now();
    let o = repos::run_trusted_feature_outcome(&feature(&cli, ""));
    assert!(t.elapsed() < Duration::from_secs(15), "pipe が詰まって 25 秒の kill まで待った: {:?}", t.elapsed());
    assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("自己申告") && reason.contains("xxxx")), "{o:?}");
}

#[cfg(unix)]
#[test]
fn cli_nonzero_exit_is_failed_and_disabled_is_rejected() {
    let d = tmp("cli");
    let cli = script(&d, "fail.sh", "echo boom >&2\nexit 3");
    assert_eq!(repos::run_trusted_feature_outcome(&feature(&cli, "")), Outcome::Failed { error: ErrorCode::DriverRefused });
    let off = model::RepoFeature { enabled: false, ..feature(&cli, "") };
    assert!(matches!(repos::run_trusted_feature_outcome(&off), Outcome::Rejected { .. }));
    assert!(repos::run_trusted_feature(&off).is_err());
}

#[cfg(unix)]
#[test]
fn cli_inside_features_dir_is_refused_and_never_runs_even_via_symlink() {
    let d = tmp("cli");
    let features = d.join("features");
    let imported = features.join("a-b-aaaaaaaaaaaa");
    fs::create_dir_all(&imported).unwrap();
    let marker = d.join("ran");
    let inside = script(&imported, "run.sh", &format!("touch '{}'", marker.display()));
    let link = d.join("innocent-looking");
    std::os::unix::fs::symlink(&inside, &link).unwrap();
    for cli in [&inside, &link] {
        let o = repos::run_trusted_feature_outcome(&feature(cli, &imported.display().to_string()));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Code that was brought in")), "{cli:?}: {o:?}");
    }
    assert!(!marker.exists(), "取り込んだ code が走った");
    // features の外の CLI は走る（同じ RepoFeature の local_path のまま）
    let outside = script(&d, "ok.sh", "echo fine");
    assert!(matches!(repos::run_trusted_feature_outcome(&feature(&outside, &imported.display().to_string())), Outcome::Unverified { .. }));
}

// ---------- SDK: tier()（CD-02）・AdapterKind（CD-07）・経路（CON-006）・source の状態（CON-010） ----------
#[test]
fn tier_maps_readiness_to_design_section_10_and_verified_only_from_outcome() {
    use CapabilityReadiness::*;
    let table = [(Unknown, DisplayTier::Unavailable), (Discoverable, DisplayTier::Available), (RequiresConnection, DisplayTier::Available),
        (ConnectedUnverified, DisplayTier::Connected), (Ready, DisplayTier::Ready), (Degraded, DisplayTier::Connected),
        (Unsupported, DisplayTier::Unavailable), (Revoked, DisplayTier::Unavailable), (SimulatedInPreview, DisplayTier::Unavailable)];
    for (r, t) in table { assert_eq!(r.tier(), t, "{r:?}") }
    let sources = [(SourceReadiness::Local, DisplayTier::Ready), (SourceReadiness::RequiresConnection, DisplayTier::Available),
        (SourceReadiness::ConnectedUnverified, DisplayTier::Connected), (SourceReadiness::Ready, DisplayTier::Ready),
        (SourceReadiness::Degraded, DisplayTier::Connected), (SourceReadiness::Unsupported, DisplayTier::Unavailable)];
    for (r, t) in sources { assert_eq!(r.tier(), t, "{r:?}") }
    let verified = Outcome::Verified { evidence: vec!["e".into()] };
    assert_eq!(sdk::tier_with(Ready, Some(&verified)), DisplayTier::Verified);
    assert_eq!(sdk::tier_with(Ready, Some(&Outcome::Unverified { reason: "受理".into() })), DisplayTier::Ready, "受理だけでは Verified にしない");
    assert_eq!(sdk::tier_with(Ready, Some(&Outcome::Verified { evidence: vec![] })), DisplayTier::Ready, "根拠の無い Verified（自己申告）では Verified にしない");
    assert_eq!(sdk::tier_with(Degraded, Some(&verified)), DisplayTier::Connected, "今 Ready でなければ過去の Verified を出さない");
}

/// YC W17-2 #12: W17-2 より前の保存（serde "cua"）・GoalPlan の "cua"・FromStr の旧名は MacGui として読み、書く時は "mac_gui"。
/// 旧 lane の Desktop の binding（app_name の無い保存）も読める（空 = 照合しない旧 binding）。
#[test]
fn adapter_kind_cua_alias_reads_as_mac_gui() {
    for old in ["\"cua\"", "\"mac_gui\""] {
        assert_eq!(serde_json::from_str::<AdapterKind>(old).unwrap(), AdapterKind::MacGui, "{old}");
    }
    for old in ["cua", "Cua", "cua_browser_tab", "mac_gui"] {
        assert_eq!(old.parse::<AdapterKind>(), Ok(AdapterKind::MacGui), "{old}");
    }
    assert_eq!(serde_json::to_string(&AdapterKind::MacGui).unwrap(), "\"mac_gui\"");
    let old: model::ActionKind = serde_json::from_value(json!({"Desktop": {"pid": 1, "window_id": 2, "label": "7", "role": "AXButton", "tool": "Click", "text": ""}})).unwrap();
    assert!(matches!(old, model::ActionKind::Desktop { ref app_name, .. } if app_name.is_empty()), "{old:?}");
}

#[test]
fn adapter_kind_takes_canonical_names_and_aliases_and_refuses_unknown_words() {
    for k in AdapterKind::ALL { assert_eq!(k.as_str().parse::<AdapterKind>(), Ok(k)) }
    for (alias, k) in [("local_state", AdapterKind::LocalCore), ("local_collection", AdapterKind::LocalCore), ("entity", AdapterKind::LocalCore),
        ("external_api", AdapterKind::Api), ("public_web", AdapterKind::Api), ("web_fetch", AdapterKind::Api), ("cua_browser_tab", AdapterKind::MacGui),
        ("trusted_adapter", AdapterKind::Repo), ("handoff", AdapterKind::OriginalSurface), ("embed", AdapterKind::OriginalSurface),
        ("companion", AdapterKind::Device), ("manual_handoff", AdapterKind::Manual), ("external_optional", AdapterKind::Undecided),
        // 2 語（CD-07・統合 2 の merge 条件 4）: AppleScript / ReaScript は api、mic は local_core。CuaDriver は GUI の最終手段なので写さない
        ("os_automation", AdapterKind::Api), ("local_device", AdapterKind::LocalCore)] {
        assert_eq!(alias.parse::<AdapterKind>(), Ok(k), "{alias}");
    }
    let e = "gmail".parse::<AdapterKind>().unwrap_err();
    assert!(e.to_string().contains("'gmail'"), "{e}");
    // corpus 10 本の source kind と capability adapter は全部写せる
    for f in fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples/v3")).unwrap() {
        let v: Value = serde_json::from_str(&fs::read_to_string(f.unwrap().path()).unwrap()).unwrap();
        let words = v["sources"].as_array().unwrap().iter().map(|s| s["kind"].clone())
            .chain(v["capabilities"].as_array().into_iter().flatten().map(|c| c["adapter"].clone()));
        for w in words.filter_map(|w| w.as_str().map(String::from)) { assert!(w.parse::<AdapterKind>().is_ok(), "corpus の '{w}' が写せない") }
    }
}

#[test]
fn route_choice_follows_con006_order_and_source_constraints_with_reasons() {
    use CapabilityReadiness::*;
    let c = sdk::choose_route("github", &[(Route::OsAutomation, Ready), (Route::OfficialApi, Ready), (Route::Handoff, Ready)]);
    assert_eq!(c.chosen, Some(Route::OfficialApi));
    assert_eq!(c.considered.len(), 4);
    assert!(c.considered.iter().any(|(r, why)| *r == Route::OsAutomation && why.contains("上の経路")), "{:?}", c.considered);
    // official API が未接続なら次の段へ。採らなかった理由が残る
    let c = sdk::choose_route("github", &[(Route::OfficialApi, RequiresConnection), (Route::OsAutomation, Ready)]);
    assert_eq!(c.chosen, Some(Route::OsAutomation));
    assert!(c.considered.iter().any(|(r, why)| *r == Route::OfficialApi && why.contains("RequiresConnection")), "{:?}", c.considered);
    // LinkedIn は OS automation を使わない（規約）→ handoff に落ちる
    let c = sdk::choose_route("linkedin", &[(Route::OsAutomation, Ready), (Route::Handoff, Ready)]);
    assert_eq!(c.chosen, Some(Route::Handoff));
    assert!(c.considered.iter().any(|(r, why)| *r == Route::OsAutomation && why.contains("User Agreement 8.2")), "{:?}", c.considered);
    // 使える経路が無い → 選ばない（手で続ける）
    assert_eq!(sdk::choose_route("ycombinator", &[(Route::OfficialApi, Ready)]).chosen, None);
}

#[test]
fn source_sync_tracks_offline_revoked_stale_and_backs_off_per_source() {
    let mut s = SourceSync::default();
    assert_eq!(s.capability(true), CapabilityReadiness::ConnectedUnverified);
    assert_eq!(s.capability(false), CapabilityReadiness::RequiresConnection);
    s.record(1_000, &Outcome::Verified { evidence: vec![] }, Some(&sdk::observe("github.issues", 1_000, json!([]))));
    assert_eq!((s.capability(true), s.source(true)), (CapabilityReadiness::Ready, SourceReadiness::Ready));
    let seen = s.last_observed_at("github.issues").unwrap();
    assert_eq!(seen, google::rfc3339_utc(1_000));
    // 鮮度は domain の DataBinding が last_observed_at から測る（別の時計を作らない）
    let src = sdk::source("github.issues", AdapterKind::Api, "github", &s, true);
    let binding = DataBinding { source_ref: "github.issues".into(), path: "items".into(), extra: Default::default() };
    assert_eq!(binding.readiness(std::slice::from_ref(&src), Some(&google::rfc3339_utc(900))), BindingReadiness::Ready);
    assert_eq!(binding.readiness(std::slice::from_ref(&src), Some(&google::rfc3339_utc(2_000))), BindingReadiness::Stale);
    // 通信が切れた: Degraded（表示は Stale）・前の時刻は残す・再試行は 5 秒 → 10 秒
    s.record(2_000, &Outcome::Failed { error: ErrorCode::ConnectionLost }, None);
    assert_eq!((s.capability(true), s.source(true)), (CapabilityReadiness::Degraded, SourceReadiness::Degraded));
    assert_eq!(s.last_observed_at("github.issues"), Some(seen));
    assert_eq!(s.retry_at(), Some(2_005));
    s.record(2_005, &Outcome::EffectUnknown { reason: "切断".into() }, None);
    assert_eq!(s.retry_at(), Some(2_015));
    // gate で止めた物は source の状態を変えない
    s.record(2_010, &Outcome::Rejected { reason: "承認なし".into() }, None);
    assert_eq!(s.retry_at(), Some(2_015));
    // 操作の権限の拒否は状態を変えない（対象への 403 かもしれない: AD-R2-3）。check（auth の確認）の拒否 = token 自体の拒否: Revoked・自動では叩かない
    s.record(2_020, &Outcome::Failed { error: ErrorCode::PermissionDenied }, None);
    assert_eq!((s.capability(true), s.retry_at()), (CapabilityReadiness::Degraded, Some(2_015)));
    s.record_check(2_020, &Outcome::Failed { error: ErrorCode::PermissionDenied });
    assert_eq!((s.capability(true), s.retry_at()), (CapabilityReadiness::Revoked, None));
    s.record(2_030, &Outcome::Verified { evidence: vec![] }, None);
    assert_eq!((s.capability(true), s.retry_at()), (CapabilityReadiness::Ready, None));
    // 再試行の間隔は 5 分で頭打ち
    let mut many = SourceSync::default();
    for t in 0..20 { many.record(t, &Outcome::Failed { error: ErrorCode::Timeout }, None) }
    assert_eq!(many.retry_at(), Some(19 + 300));
}

#[test]
fn reconcile_never_silently_overwrites_conflicting_observations() {
    let obs = |at: u64, v: Value| sdk::observe("google.calendar", at, v);
    let (a, b) = (obs(100, json!({"summary": "会議"})), obs(200, json!({"summary": "会議"})));
    assert_eq!(sdk::reconcile(a.clone(), b.clone()).unwrap(), b, "同じ中身なら新しい方");
    let theirs = obs(300, json!({"summary": "会議（変更）"}));
    let c = sdk::reconcile(a.clone(), theirs.clone()).unwrap_err();
    assert_eq!((c.ours, c.theirs), (a, theirs), "両方の根拠と時刻を持ったまま返す");
}

// ---------- registry: 5 adapter の登録・domain の契約・gate ----------
/// 試験の app（run の外から）。承認の記録の app_id と同じ
const APP: Caller = Caller { app_id: "test", run_id: None };
/// 見せた入力（input）への承認の記録。digest は registry と同じ sdk::payload_digest、id は毎回別（1 回の承認 = 1 回の実行）
fn consent(action: &str, scopes: &[&str], input: &Value) -> ConsentReceipt {
    ConsentReceipt { id: format!("consent-{}", model::new_id()), app_id: "test".into(), subject: ConsentSubject::Action { action_ref: action.into(), run_id: None },
        scopes: scopes.iter().map(|s| s.to_string()).collect(), payload_digest: Some(sdk::payload_digest(action, input)), decision: ConsentDecision::Approved,
        decided_at: "2026-10-17T00:00:00Z".into() }
}
fn client(base: &str) -> google::Client { google::Client { access_token: "gtok".into(), gmail_base: base.into(), calendar_base: base.into() } }
const ALL_GOOGLE: &str = "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.compose https://www.googleapis.com/auth/calendar.events";

/// 試験の中で定義する adapter（composer / renderer / registry の code を変えずに載る事を見る）
struct Fake { id: &'static str, specs: &'static [Spec] }
const ECHO_SPEC: Spec = Spec { id: "local.echo.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "local_state" };
const ECHO: &[Spec] = &[ECHO_SPEC];
impl Adapter for Fake {
    fn id(&self) -> &'static str { self.id }
    fn kind(&self) -> AdapterKind { AdapterKind::LocalCore }
    fn route(&self) -> Route { Route::OfficialApi }
    fn provider(&self) -> &'static str { "local" }
    fn specs(&self) -> &'static [Spec] { self.specs }
    fn readiness(&self, _: &str) -> CapabilityReadiness { CapabilityReadiness::Ready }
    fn sources(&self) -> Vec<domain::binding::Source> { vec![] }
    fn check(&mut self) {}
    fn invoke(&mut self, _: &str, input: &Value) -> (Outcome, Option<Observation>) { sdk::read_result(Ok(input.clone()), "local.echo", "echo", 0) }
    fn cleanup(&mut self) {}
}

#[cfg(unix)]
#[test]
fn five_adapters_register_and_their_definitions_validate_as_a_domain_app() {
    let d = tmp("reg");
    let cli = script(&d, "ok.sh", "echo ok");
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(api("http://127.0.0.1:9")))).unwrap();
    // GUI 操作は MacGui（Shibubu Bridge・YC W18-1 で旧い desktop の adapter を消した）。登録だけでは bridge を起こさない
    r.register(Box::new(adapters::mac_gui::MacGui::new())).unwrap();
    r.register(Box::new(adapters::web_json::WebJson::new(Some("http://127.0.0.1:9/x.json".into()), None))).unwrap();
    r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![feature(&cli, "")]))).unwrap();
    r.register(Box::new(adapters::google::Google::new(Some(client("http://127.0.0.1:9")), ALL_GOOGLE))).unwrap();
    // 6 つ目（試験の中で定義）も同じ口で載り、使える = adapter を足しても composer / renderer / registry は変わらない（CON-001 AC）
    r.register(Box::new(Fake { id: "echo", specs: ECHO })).unwrap();
    assert!(matches!(r.invoke(APP, "local.echo.read", &json!({"x": 1}), None).0, Outcome::Verified { .. }));
    let ids: Vec<String> = r.actions().iter().map(|a| a.id.clone()).collect();
    assert_eq!(ids.len(), 21, "{ids:?}"); // ENGINE-AUDIT W3 desktop.menu.read・W4 desktop.app.surfaces（18 のまま古かった）・W7 desktop.ax.reveal
    for want in ["github.issues.list", "github.pulls.list", "github.issues.create", "web.json.read", "desktop.windows.list", "desktop.apps.list", "desktop.ax.click", "desktop.ax.reveal",
                 "desktop.ax.type_text", "desktop.ax.menu", "desktop.key", "cli.trusted.run", "gmail.threads.read", "gmail.drafts.create", "calendar.events.read", "calendar.events.create", "desktop.document.open"] {
        assert!(ids.iter().any(|i| i == want), "{want} が無い");
    }
    // 定義を app に入れて domain の validate に通す（composer が registry の出力をそのまま使える形か）
    let mut app: Value = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples/v3/01_timer_local.json")).unwrap()).unwrap();
    app["capabilities"] = json!(r.capabilities());
    app["actions"].as_array_mut().unwrap().extend(r.actions().iter().map(|a| json!(a)));
    app["sources"].as_array_mut().unwrap().extend(r.sources().iter().map(|s| json!(s)));
    app["state"]["execution_mode"] = json!("integrated");
    let def: domain::app::AppDefinitionV3 = serde_json::from_value(app).expect("registry の出力が schema の型に入る");
    domain::validate::validate(&def).expect("registry の出力が domain の検査を通る");
    // 確認（check）の前は action を Ready にしない
    assert!(def.actions.iter().filter(|a| a.id.starts_with("github.")).all(|a| a.readiness == domain::action::ActionReadiness::RequiresConnection));
    // 効果と承認（CD-01: GitHub 読み = external_read / 作成 = external_write、desktop の観測 = local_read・click / type_text = external_write、CLI = external_write）
    let effect = |id: &str| r.actions().into_iter().find(|a| a.id == id).map(|a| (a.effect, a.requires_approval)).unwrap();
    assert_eq!(effect("github.issues.list"), (Effect::ExternalRead, false));
    assert_eq!(effect("github.issues.create"), (Effect::ExternalWrite, true));
    assert_eq!(effect("desktop.windows.list"), (Effect::LocalRead, false));
    assert_eq!(effect("desktop.apps.list"), (Effect::LocalRead, false));
    assert_eq!(effect("desktop.ax.click"), (Effect::ExternalWrite, true));
    assert_eq!(effect("cli.trusted.run"), (Effect::ExternalWrite, true));
    assert_eq!(effect("gmail.drafts.create"), (Effect::ExternalWrite, true));
    // corpus 09（owner test2）の Google の id・effect・scope と一致（composer の出力にそのまま繋がる）
    let corpus: Value = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples/v3/09_owner_test2_calendar_mail_chat.json")).unwrap()).unwrap();
    for id in ["gmail.threads.read", "gmail.drafts.create", "calendar.events.read", "calendar.events.create"] {
        let want = corpus["actions"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap();
        let got = json!(r.actions().into_iter().find(|a| a.id == id).unwrap());
        assert_eq!((&got["effect"], &got["required_scopes"]), (&want["effect"], &want["required_scopes"]), "{id}");
    }
}

#[test]
fn register_refuses_names_without_namespace_duplicates_and_bad_schemas() {
    let mut r = AdapterRegistry::default();
    r.register(Box::new(Fake { id: "echo", specs: ECHO })).unwrap();
    assert_eq!(r.register(Box::new(Fake { id: "echo", specs: &[] })).unwrap_err(), RegisterError::DuplicateAdapter("echo".into()));
    assert_eq!(r.register(Box::new(Fake { id: "echo2", specs: ECHO })).unwrap_err(), RegisterError::DuplicateAction("local.echo.read".into()));
    const NODOT: &[Spec] = &[Spec { id: "nodot", ..ECHO_SPEC }];
    const UPPER: &[Spec] = &[Spec { id: "Local.Echo", ..ECHO_SPEC }];
    const SCHEMA: &[Spec] = &[Spec { id: "local.bad.schema", input: "[1]", ..ECHO_SPEC }];
    assert_eq!(r.register(Box::new(Fake { id: "a", specs: NODOT })).unwrap_err(), RegisterError::BadId("nodot".into()));
    assert_eq!(r.register(Box::new(Fake { id: "b", specs: UPPER })).unwrap_err(), RegisterError::BadId("Local.Echo".into()));
    assert_eq!(r.register(Box::new(Fake { id: "c", specs: SCHEMA })).unwrap_err(), RegisterError::BadSchema("local.bad.schema".into()));
    assert_eq!(r.actions().len(), 1, "拒んだ adapter は載らない");
}

#[test]
fn registry_gate_needs_ready_and_an_exact_approval_and_sends_nothing_otherwise() {
    let (base, log) = fake(vec![("GET", "/rate_limit", 200, json!({"resources": {}})),
        ("POST", "/repos/o/r/issues", 201, json!({"number": 3, "html_url": "https://github.com/o/r/issues/3"}))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(api(&base)))).unwrap();
    let input = json!({"repo": "o/r", "title": "t"});
    let ok = consent("github.issues.create", &["github.issues.write"], &input);
    // 未確認（ConnectedUnverified）: 承認があっても送らない
    assert!(matches!(r.invoke(APP, "github.issues.create", &input, Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("ConnectedUnverified")));
    assert_eq!(r.tier("github.issues.create"), DisplayTier::Connected);
    r.check();
    assert_eq!(r.tier("github.issues.create"), DisplayTier::Ready);
    // 承認なし・別 action への承認・scope 不足・却下 → 送らない
    let other = consent("github.issues.list", &["github.issues.write"], &input);
    let narrow = consent("github.issues.create", &[], &input);
    let denied = ConsentReceipt { decision: ConsentDecision::Denied, ..ok.clone() };
    for c in [None, Some(&other), Some(&narrow), Some(&denied)] {
        assert!(matches!(r.invoke(APP, "github.issues.create", &input, c).0, Outcome::Rejected { reason } if reason.contains("承認")), "{c:?}");
    }
    assert!(matches!(r.invoke(APP, "github.issue.create", &input, Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("登録されていない")), "名前の似た別物を引かない");
    assert_eq!(log.lock().unwrap().iter().filter(|q| q.method == "POST").count(), 0, "gate を越えずに送った");
    // 承認つき → 1 回だけ送り、Verified の段になる
    let (o, _) = r.invoke(APP, "github.issues.create", &input, Some(&ok));
    assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0] == "#3"), "{o:?}");
    assert_eq!(log.lock().unwrap().iter().filter(|q| q.method == "POST").count(), 1);
    assert_eq!(r.tier("github.issues.create"), DisplayTier::Verified);
    let events = r.drain_events();
    assert!(events.contains(&AdapterEvent::Readiness { action: "github.issues.create".into(), from: CapabilityReadiness::ConnectedUnverified, to: CapabilityReadiness::Ready }), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, AdapterEvent::Invoked { action, outcome: Outcome::Verified { .. } } if action == "github.issues.create")));
    assert!(r.drain_events().is_empty());
    // 後始末で token を捨てる → 書き込みは接続が必要に戻る
    r.cleanup();
    assert_eq!(r.tier("github.issues.create"), DisplayTier::Available);
    assert!(matches!(r.invoke(APP, "github.issues.create", &input, Some(&ok)).0, Outcome::Rejected { .. }));
}

#[test]
fn github_reads_carry_provenance_and_an_auth_failure_revokes() {
    let (base, _) = fake(vec![("GET", "/rate_limit", 200, json!({})), ("GET", "/repos/o/r/issues", 200, json!([{"number": 1, "title": "t", "html_url": "u"}]))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(github::Api::new(&base, None)))).unwrap();
    r.check();
    let (o, obs) = r.invoke(APP, "github.issues.list", &json!({"repo": "o/r"}), None);
    let obs = obs.expect("読んだ物は Observation で返る");
    assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].contains(&obs.id)), "{o:?}");
    assert_eq!((obs.source_ref.as_str(), obs.payload["items"][0]["number"].as_u64()), ("github.issues", Some(1)));
    assert_eq!(r.sources()[0].last_observed_at.as_deref(), Some(obs.observed_at.as_str()), "source に最後に読めた時刻（出典と最終更新）");
    assert_eq!(r.tier("github.issues.create"), DisplayTier::Available, "token が無ければ書き込みは接続が必要");
    // check の後に token が取り消された: 読み取りの 401 → registry が check をやり直し、/rate_limit も 401 → Revoked
    let probes = AtomicUsize::new(0);
    let (base2, _) = fake_with(move |q| if q.path == "/rate_limit" && probes.fetch_add(1, Ordering::SeqCst) == 0 { (200, "{}".into()) } else { (401, json!({"message": "Bad credentials"}).to_string()) });
    let mut r2 = AdapterRegistry::default();
    r2.register(Box::new(adapters::github::GitHub::new(api(&base2)))).unwrap();
    r2.check();
    assert_eq!(r2.invoke(APP, "github.issues.list", &json!({"repo": "o/r"}), None).0, Outcome::Failed { error: ErrorCode::PermissionDenied });
    assert_eq!(r2.capabilities()[0].readiness, CapabilityReadiness::Revoked, "認証を拒まれた → 再接続する");
    assert_eq!(r2.sources()[0].readiness, SourceReadiness::RequiresConnection);
}

/// YC W17-2 #11: registry の desktop の口は MacGui（bridge）でも cua と同じ約束: check は bridge の hello（許可）・承認が無ければ押さない・
/// 保存した app 名が無い / 違えば押さない・窓の中身の読みに ref を出さない・受理は Verified の段にしない。cua-driver は起こさない
#[cfg(unix)]
#[test]
fn mac_gui_adapter_checks_the_bridge_and_never_presses_without_the_saved_app_name() {
    let d = tmp("mg-reg");
    let log = d.join("bridge.log");
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\nexec python3 '{py}'", log.display()));
    let presses = || fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(r#""cmd":"ax_press""#)).count();
    // R-G2: Core（Connections の行）が使う desktop() は画面の操作の adapter だけ（GitHub / Google を載せない = check が外へ通信しない）・作った直後は未確認
    let caps = AdapterRegistry::desktop().capabilities();
    assert!(!caps.is_empty() && caps.iter().all(|c| c.provider.as_deref() == Some("desktop") && c.readiness == CapabilityReadiness::ConnectedUnverified), "{caps:?}");
    let mut r = AdapterRegistry::default();
    r.register(Box::new(gui_at(sh))).unwrap();
    assert_eq!(r.tier("desktop.ax.click"), DisplayTier::Connected);
    r.check();
    assert_eq!(r.tier("desktop.ax.click"), DisplayTier::Ready);
    let (o, obs) = r.invoke(APP, "desktop.windows.list", &json!({}), None);
    assert!(matches!(o, Outcome::Verified { .. }), "{o:?}");
    assert_eq!(obs.unwrap().payload[0]["app_name"], "REAPER", "登録時に保存する app 名が一覧に出る");
    // 同意を書く前の窓の読み（engine_page の grant）は bundle id で絞る: 他の app の窓は読まない
    let mut by_bundle = |b: &str| r.invoke(APP, "desktop.windows.list", &json!({ "bundle_id": b }), None).1.unwrap().payload;
    assert!(by_bundle("com.cockos.reaper")[0]["window_id"] == 300 && by_bundle("com.apple.Notes") == json!([]));
    assert!(seen(&r, "desktop.windows").is_some() && seen(&r, "desktop.window").is_none());
    let state = r.invoke(APP, "desktop.window.state", &json!({"pid": 222, "window_id": 300}), None).1.unwrap();
    assert_eq!(state.payload["app_name"], "REAPER");
    // W19-1（operator 2026-10-23）: ② の ref は payload の Control::element で返す（その読みの間だけ有効・押す側は label + role で読み直す）
    assert!(state.payload.to_string().contains("\"label\":\"保存\"") && state.payload.to_string().contains("\"element\":\"e"), "② の ref が payload に無い: {}", state.payload);
    // R-G2 の契約 gui::WindowState: 押せる要素は actions・値を置ける欄は value_settable を載せる（serde の default で通るだけでは緑にしない）
    let ws: gui::WindowState = serde_json::from_value(state.payload.clone()).unwrap();
    assert!(ws.controls.iter().any(|c| c.label == "保存" && c.actions == ["AXPress"]) && ws.controls.iter().any(|c| c.label == "本文" && c.value_settable), "{ws:?}");
    assert!(ws.controls.iter().all(|c| c.role != "AXWindow"), "窓そのものは controls に入れない: {ws:?}");
    // 窓を決められない時の 1 行に入力の app 名を出す（照合には使わない）
    let gone = r.invoke(APP, "desktop.window.state", &json!({"pid": 222, "window_id": 999, "app_name": "REAPER"}), None).0;
    assert!(matches!(&gone, Outcome::Rejected { reason } if reason.starts_with("REAPER's window wasn't found. Reopen the window")), "完全な 1 行を '…' で包み直さない: {gone:?}");
    let target =|app: &str| json!({"pid": 222, "window_id": 300, "app_name": app, "label": "保存", "role": "AXButton"});
    let ok = |input: &Value| consent("desktop.ax.click", &["desktop.input"], input);
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &target("REAPER"), None).0, Outcome::Rejected { reason } if reason.contains("承認")));
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &target(""), Some(&ok(&target("")))).0, Outcome::Rejected { reason } if reason.contains("old format") && reason.contains("Find windows")), "app 名の無い旧 binding は押さず次の一手");
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &target("Notes"), Some(&ok(&target("Notes")))).0, Outcome::Rejected { reason } if reason.contains("Notes")));
    assert_eq!(presses(), 0);
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &target("REAPER"), Some(&ok(&target("REAPER")))).0, Outcome::Unverified { .. }));
    assert_eq!(presses(), 1);
    assert_eq!(r.tier("desktop.ax.click"), DisplayTier::Ready, "受理（Unverified）では Verified の段にしない");
    // menu と key: pid の app 名を読んで照合してから送る
    let menu = |app: &str| json!({"pid": 222, "app_name": app, "path": ["ファイル", "保存"]});
    let okm = |input: &Value| consent("desktop.ax.menu", &["desktop.input"], input);
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &menu("Notes"), Some(&okm(&menu("Notes")))).0, Outcome::Rejected { reason } if reason.contains("Notes")));
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &menu("REAPER"), Some(&okm(&menu("REAPER")))).0, Outcome::Unverified { .. }));
    let key = json!({"pid": 222, "app_name": "REAPER", "key": "s", "mods": ["cmd"]});
    assert!(matches!(r.invoke(APP, "desktop.key", &key, Some(&consent("desktop.key", &["desktop.input"], &key))).0, Outcome::Unverified { .. }));
    // 許可が無い・bridge が無い: 接続が必要、何も読まない
    for env in ["export FAKE_MODE=untrusted\n", "exit 3\n"] {
        let d = tmp("mg-reg-bad");
        let log = d.join("bridge.log");
        let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\n{env}exec python3 '{py}'", log.display()));
        let mut r = AdapterRegistry::default();
        r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
        r.check();
        assert_eq!(r.capabilities()[0].readiness, CapabilityReadiness::RequiresConnection, "{env}");
        // operator 2026-10-23 (B): registry の定型文ではなく Adapter の理由の 1 行（許可が無い / bridge を起こせない）
        let want = if env.contains("untrusted") { "Accessibility permission is missing" } else { "Screen control didn't start" };
        assert!(matches!(r.invoke(APP, "desktop.windows.list", &json!({}), None).0, Outcome::Rejected { reason } if reason.contains(want) && !reason.contains("RequiresConnection")), "{env}");
        assert!(!fs::read_to_string(&log).unwrap_or_default().contains("ax_tree"));
    }
}

/// W19-1 act の記録（gui::ActLog）: registry → MacGui で送った操作だけを 1 行ずつ（Press・SetValue・Menu・Key・宛先の pid と bundle id・Adapter と版・時刻の順）。
/// 送らなかった操作（承認なし・別 app・旧 binding・許可なし）は書かない（W19-3b「許可が無い間は act の記録が 0 件」）。
/// act の後の確かめ: 置いた値を同じ欄で読み戻して一致した type_text だけ records が Verified（押しただけの click・menu・key は Unverified のまま）
#[cfg(unix)]
#[test]
fn mac_gui_act_log_has_only_sent_acts_and_records_raise_only_read_back_acts() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let registry = |tag: &str, env: &str| {
        let d = tmp(tag);
        let path = d.join(gui::ACT_LOG_FILE);
        let sh = script(&d, "bridge.sh", &format!("{env}exec python3 '{py}'"));
        let mut r = AdapterRegistry::default();
        r.register(Box::new(gui_at(sh).with_act_log(path.clone()))).unwrap();
        r.check();
        (r, path)
    };
    let acts = |p: &Path| if p.exists() { gui::ActLog::read(p).unwrap() } else { vec![] };
    let click = |app: &str| json!({"pid": 222, "window_id": 300, "app_name": app, "label": "保存", "role": "AXButton"});
    let typed = json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": "本文", "role": "AXTextArea", "text": "nonce-1"});
    let menu = json!({"pid": 222, "app_name": "REAPER", "path": ["ファイル", "保存"]});
    let key = json!({"pid": 222, "app_name": "REAPER", "key": "s", "mods": ["cmd"]});
    let ok = |a: &str, i: &Value| consent(a, &["desktop.input"], i);
    let (mut r, path) = registry("mg-actlog", "");
    // 送らない: 承認なし・別 app・旧 binding
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &click("REAPER"), None).0, Outcome::Rejected { .. }));
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &click("Notes"), Some(&ok("desktop.ax.click", &click("Notes")))).0, Outcome::Rejected { .. }));
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &click(""), Some(&ok("desktop.ax.click", &click("")))).0, Outcome::Rejected { .. }));
    assert!(acts(&path).is_empty(), "送っていない操作を記録した: {:?}", acts(&path));
    // 送る
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &click("REAPER"), Some(&ok("desktop.ax.click", &click("REAPER")))).0, Outcome::Unverified { .. }));
    assert!(matches!(r.invoke(APP, "desktop.ax.type_text", &typed, Some(&ok("desktop.ax.type_text", &typed))).0, Outcome::Verified { .. }));
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &menu, Some(&ok("desktop.ax.menu", &menu))).0, Outcome::Unverified { .. }));
    assert!(matches!(r.invoke(APP, "desktop.key", &key, Some(&ok("desktop.key", &key))).0, Outcome::Unverified { .. }));
    let got = acts(&path);
    let kinds: Vec<String> = got.iter().map(|a| match &a.receipt.action {
        gui::GuiAction::Press { .. } => "press".into(),
        gui::GuiAction::SetValue { value, .. } => format!("set:{value}"),
        gui::GuiAction::Menu { path } => format!("menu:{}", path.join(">")),
        gui::GuiAction::Key { keys } => format!("key:{keys}"),
    }).collect();
    assert_eq!(kinds, ["press", "set:nonce-1", "menu:ファイル>保存", "key:cmd+s"]);
    assert!(got.iter().all(|a| a.adapter == "mac_gui" && !a.adapter_version.is_empty() && a.receipt.target.pid == 222 && a.receipt.target.bundle_id == "com.cockos.reaper"), "{got:?}");
    assert_eq!((got[0].receipt.target.window_id, got[2].receipt.target.window_id), (Some(300), None));
    assert!(got.windows(2).all(|w| w[0].receipt.sent_ms <= w[1].receipt.sent_ms), "時刻の順");
    // records: 読み戻して一致した type_text だけ Verified
    let rec = r.records();
    let verified = |id: &str| rec.iter().find(|x| x.id == id).map(|x| matches!(x.verification, domain::capability::Verification::Verified { .. })).unwrap();
    assert!(verified("desktop.ax.type_text"), "読み戻して一致した操作が Verified にならない");
    for id in ["desktop.ax.click", "desktop.ax.menu", "desktop.key", "desktop.windows.list"] {
        assert!(!verified(id), "{id}: 送っただけ・試していないのに Verified");
    }
    // bridge が送る前に断った（stale_element）は書かない・送った後かもしれない（cannot_complete）は書く（送っていないと言わない向き）
    for (fail, n) in [("stale_element", 0), ("cannot_complete", 1)] {
        let (mut r3, path3) = registry(&format!("mg-actlog-{fail}"), &format!("export FAKE_AX_FAIL={fail}\n"));
        r3.invoke(APP, "desktop.ax.click", &click("REAPER"), Some(&ok("desktop.ax.click", &click("REAPER"))));
        assert_eq!(acts(&path3).len(), n, "{fail}: {:?}", acts(&path3));
    }
    // 許可が無い間は 0 件（W19-3b）
    let (mut r2, path2) = registry("mg-actlog-untrusted", "export FAKE_MODE=untrusted\n");
    assert!(matches!(r2.invoke(APP, "desktop.ax.click", &click("REAPER"), Some(&ok("desktop.ax.click", &click("REAPER")))).0, Outcome::Rejected { .. }));
    assert!(acts(&path2).is_empty());
    // 置き場の dir がまだ無くても作って書く（書けずに 0 件 =「送っていない」と区別できない、にしない）
    let d = tmp("mg-actlog-nodir");
    let deep = d.join("まだ無い").join(gui::ACT_LOG_FILE);
    let sh = script(&d, "bridge.sh", &format!("exec python3 '{py}'"));
    let mut r4 = AdapterRegistry::default();
    r4.register(Box::new(gui_at(sh).with_act_log(deep.clone()))).unwrap();
    r4.check();
    assert!(matches!(r4.invoke(APP, "desktop.ax.click", &click("REAPER"), Some(&ok("desktop.ax.click", &click("REAPER")))).0, Outcome::Unverified { .. }));
    assert_eq!(acts(&deep).len(), 1, "置き場の dir が無いと、送った操作を記録しなかった");
}

/// W19-1 app ごとの AI の同意（YC_DECISIONS 11 通目「既定は無し・同意の無い app への act は 0 件で 1 行」・operator 2026-10-23）の 1 本目:
/// 同意の無い app（偽の bridge の REAPER）へは、registry の道（click・type_text・menu・key）も MacGui::act も bridge に何も送らず、理由は
/// 「REAPER を AI に任せる」を押す 1 行。act の記録には断った操作ごとに `not_sent: needs_consent` の 1 行（送った行は 0）。別の app の同意は効かない。
/// Accessibility の許可も無い時は許可の 1 行だけで、act の記録は 0 件（W19-3b）
#[cfg(unix)]
#[test]
fn mac_gui_sends_nothing_to_an_app_without_ai_consent() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let fresh = |tag: &str, env: &str| {
        let d = tmp(tag);
        let log = d.join("bridge.log");
        let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\n{env}exec python3 '{py}'", log.display()));
        let consents = d.join("act-consents.jsonl");
        // 別の app（TextEdit）だけに同意がある
        surface::grant_act_consent(&consents, "com.apple.TextEdit", 1.0).unwrap();
        (adapters::mac_gui::MacGui::at(sh).with_act_log(d.join(gui::ACT_LOG_FILE)).with_act_consents(consents), log, d.join(gui::ACT_LOG_FILE))
    };
    let sent = |log: &Path| fs::read_to_string(log).unwrap_or_default().lines().filter(|l| ["ax_press", "ax_set_value", "ax_menu", "key_to_pid"].iter().any(|c| l.contains(&format!(r#""cmd":"{c}""#)))).count();
    let acts = |p: &Path| if p.exists() { gui::ActLog::read(p).unwrap() } else { vec![] };
    // W8a: 宛先は隠れている（FAKE_HIDDEN）= 同意の無い app は見せもしない（app_unhide 0・見える変化を同意の前に起こさない）
    let (g, log, path) = fresh("mg-noconsent", "export FAKE_HIDDEN=1\n");
    let mut r = AdapterRegistry::default();
    r.register(Box::new(g)).unwrap();
    r.check();
    let click = json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": "保存", "role": "AXButton"});
    let typed = json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": "本文", "role": "AXTextArea", "text": "nonce-1"});
    let menu = json!({"pid": 222, "app_name": "REAPER", "path": ["ファイル", "保存"]});
    let key = json!({"pid": 222, "app_name": "REAPER", "key": "s", "mods": ["cmd"]});
    for (a, i) in [("desktop.ax.click", &click), ("desktop.ax.type_text", &typed), ("desktop.ax.menu", &menu), ("desktop.key", &key)] {
        let o = r.invoke(APP, a, i, Some(&consent(a, &["desktop.input"], i))).0;
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("REAPER isn't allowed to be controlled by the AI yet") && reason.contains("“Hand REAPER to the AI”")), "{a}: {o:?}");
    }
    let (mut g2, log2, path2) = fresh("mg-noconsent-act", "");
    // W6b: 直の口（`key`）も同じ門 = 同意は今の app（REAPER）で見る（呼ぶ側の言い分は無い）
    let denied = g2.key(222, &"cmd+s".parse().unwrap());
    assert!(matches!(&denied, adapters::mac_gui::GuiResult::Unsupported(why) if why.contains("REAPER")), "同意の無い app に key を送った: {denied:?}");
    assert_eq!((sent(&log), sent(&log2)), (0, 0), "同意の無い app に送った");
    assert!(!fs::read_to_string(&log).unwrap_or_default().contains(r#""cmd":"app_unhide""#), "W8a: 同意の無い隠れた app を見せた");
    let got = acts(&path);
    assert_eq!(got.len(), 4, "断った操作ごとに 1 行: {got:?}");
    assert!(got.iter().all(|a| a.not_sent == Some(gui::NotSent::NeedsConsent) && a.receipt.target.bundle_id == "com.cockos.reaper"), "{got:?}");
    assert!(acts(&path2).iter().all(|a| a.not_sent == Some(gui::NotSent::NeedsConsent) && a.receipt.target.bundle_id == "com.cockos.reaper"));
    // 許可も無い: 許可の 1 行・act の記録 0 件
    let (mut g3, log3, path3) = fresh("mg-noconsent-untrusted", "export FAKE_MODE=untrusted\n");
    let o = g3.run_desktop(&model::ActionKind::Desktop { pid: 222, window_id: 300, label: "保存".into(), role: "AXButton".into(), tool: model::DesktopTool::Click, text: String::new(), app_name: "REAPER".into() });
    assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Accessibility permission is missing")), "{o:?}");
    let k = g3.menu(222, &["ファイル", "保存"]);
    assert!(matches!(&k, adapters::mac_gui::GuiResult::Unsupported(why) if why.contains("Accessibility permission is missing")), "{k:?}");
    assert_eq!((sent(&log3), acts(&path3).len()), (0, 0), "許可が無い間に送った・記録した");
}

/// ENGINE-AUDIT W7: 承認の要らない押す手（`desktop.ax.reveal`）は承認なしで押す。ただし押す直前に解決した要素の名前が消す・送る・買う系の語なら、
/// 何も押さずに `Failed { ApprovalDenied }`（Core が click に書き換えて承認の card）。click は今まで通り承認が無ければ Rejected。
/// 偽の bridge の FAKE_AX_ROWS = 押せる AXLink「<名前> (text channel)」。負の対照: run_desktop_matching の reveal の門を外す → 「Delete all」が押されて赤。
#[cfg(unix)]
#[test]
fn w7_reveal_presses_without_asking_but_not_a_name_that_deletes() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let d = tmp("mg-w7-reveal");
    let log = d.join("bridge.log");
    let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\nexport FAKE_AX_ROWS='Delete all|general'\nexec python3 '{py}'", log.display()));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(gui_at(sh))).unwrap();
    r.check();
    let sent = || fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(r#""cmd":"ax_press""#)).count();
    let row = |name: &str| json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": format!("{name} (text channel)"), "role": "AXLink"});
    let (o, _) = r.invoke(APP, adapters::mac_gui::REVEAL, &row("Delete all"), None);
    assert!(matches!(o, Outcome::Failed { error: ErrorCode::ApprovalDenied }), "{o:?}");
    assert_eq!(sent(), 0, "消す名前の行を承認なしで押した");
    let (o, _) = r.invoke(APP, adapters::mac_gui::REVEAL, &row("general"), None);
    assert!(matches!(o, Outcome::Verified { .. } | Outcome::Unverified { .. }), "戻せる手を承認なしで押せない: {o:?}");
    assert_eq!(sent(), 1);
    let (o, _) = r.invoke(APP, "desktop.ax.click", &row("general"), None);
    assert!(matches!(o, Outcome::Rejected { .. }), "click を承認なしで押した: {o:?}");
    assert_eq!(sent(), 1);
}

/// W19-1 app ごとの AI の同意の 2 本目（正の対照）: 人が「REAPER を AI に任せる」を押す（`surface::grant_act_consent`・旧 Desktop パネルの button と同じ口）と、
/// 同じ MacGui（起動の後に押した同意も送る前に読み直す）の同じ操作が送られ、act の記録に送った 1 行（`not_sent` 無し）が足される
#[cfg(unix)]
#[test]
fn mac_gui_acts_after_ai_consent_is_granted() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let d = tmp("mg-consent-granted");
    let log = d.join("bridge.log");
    let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\nexec python3 '{py}'", log.display()));
    let (consents, path) = (d.join("act-consents.jsonl"), d.join(gui::ACT_LOG_FILE));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::mac_gui::MacGui::at(sh).with_act_log(path.clone()).with_act_consents(consents.clone()))).unwrap();
    r.check();
    let click = json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": "保存", "role": "AXButton"});
    let presses = || fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(r#""cmd":"ax_press""#)).count();
    assert!(matches!(r.invoke(APP, "desktop.ax.click", &click, Some(&consent("desktop.ax.click", &["desktop.input"], &click))).0, Outcome::Rejected { .. }));
    assert_eq!(presses(), 0);
    surface::grant_act_consent(&consents, "com.cockos.reaper", surface::now_ms()).unwrap();
    let o = r.invoke(APP, "desktop.ax.click", &click, Some(&consent("desktop.ax.click", &["desktop.input"], &click))).0;
    assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("AXPress")), "同意の後も送らない: {o:?}");
    assert_eq!(presses(), 1, "同意の後に 1 回だけ送る");
    let got = gui::ActLog::read(&path).unwrap();
    assert_eq!(got.iter().map(|a| a.not_sent).collect::<Vec<_>>(), vec![Some(gui::NotSent::NeedsConsent), None], "断った 1 行の後に送った 1 行");
    assert!(matches!(got[1].receipt.action, gui::GuiAction::Press { .. }) && got[1].receipt.target.bundle_id == "com.cockos.reaper");
}

/// W19-3b（YC_WAVES §5.5「壊れない・許可」）の Adapter の側: 宛先は保存せず、起動ごとに窓の一覧から `gui::retarget`（bundle id + 題名）で決め直して
/// act → verify（同じ欄の AXValue の読み戻し）・act の記録に送った 1 行。bridge を起こし直した後（再起動）も同じ手順で 2 回目が通る（F1 の経路と読み戻しが揃う）。
/// Accessibility の許可を取り消した間は、窓の一覧も操作も許可の 1 行で、何も送らず act の記録は増えない。
/// Page の移動・自然言語の編集の後の 2 回目は、W19-2 の act が Page に配線された後に yc_demo_e2e で通す（今は Page から act を送る道が無い）
#[cfg(unix)]
#[test]
fn w19_3b_retarget_act_and_read_back_survive_a_restart_and_untrusted_sends_nothing() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let d = tmp("w19-3b");
    let (consents, acts) = (d.join("act-consents.jsonl"), d.join(gui::ACT_LOG_FILE));
    surface::grant_act_consent(&consents, "com.apple.TextEdit", 1.0).unwrap();
    let log = d.join("bridge.log");
    let wl = r#"[[{"title":"README.md"},{"title":"notes.md"}]]"#;
    let bridge = |tag: &str, extra: &str| script(&d, &format!("{tag}.sh"), &format!("export FAKE_LOG='{}'\nexport FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{wl}'\n{extra}exec python3 '{py}'", log.display()));
    let gui = |sh: PathBuf| adapters::mac_gui::MacGui::at(sh).with_act_log(acts.clone()).with_act_consents(consents.clone());
    let sent = || fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(r#""cmd":"ax_set_value""#)).count();
    let once = |g: &mut adapters::mac_gui::MacGui, nonce: &str| {
        let ws = g.windows().unwrap();
        let t = gui::retarget("com.apple.TextEdit", "TextEdit", "notes.md", None, &ws).unwrap();
        // W6b: 読み（elements）→ 置く（set_text = 同じ欄の AXValue の読み戻しで Verified）
        let w = gui::WindowRef { pid: t.pid, window_id: i64::from(t.window_id.expect("CG の窓番号")), app_name: "TextEdit".into(), title: String::new(), bundle_id: "com.apple.TextEdit".into(), document: String::new() };
        let body = g.elements(&w).unwrap().items.iter().find(|e| e.role == "AXTextArea").expect("本文の欄が無い").r.clone();
        assert!(matches!(g.set_text(&w, &body, nonce, None), adapters::mac_gui::GuiResult::Verified { .. }), "読み戻しが揃わない");
        t
    };
    let first = once(&mut gui(bridge("a", "")), "w19-3b-1");
    // 再起動: bridge を起こし直し、窓の一覧から決め直す（前の pid・窓 id を保存して使わない）
    let second = once(&mut gui(bridge("b", "")), "w19-3b-2");
    assert_eq!((first.bundle_id.as_str(), second.window_id.is_some()), ("com.apple.TextEdit", true));
    let got = gui::ActLog::read(&acts).unwrap();
    assert_eq!(got.iter().filter(|a| a.not_sent.is_none()).count(), 2, "送った 2 回が act の記録に無い: {got:?}");
    assert_eq!(sent(), 2);
    // 許可の取り消し: 一覧も操作も許可の 1 行・何も送らない・記録は増えない
    let mut u = gui(bridge("u", "export FAKE_MODE=untrusted\n"));
    assert!(u.windows().unwrap_err().contains("Accessibility permission is missing"));
    let denied = u.key(second.pid, &"cmd+s".parse().unwrap());
    assert!(matches!(&denied, adapters::mac_gui::GuiResult::Unsupported(why) if why.contains("アクセシビリティ") || why.contains("Accessibility")), "{denied:?}");
    assert_eq!((sent(), gui::ActLog::read(&acts).unwrap().len()), (2, 2), "許可が無い間に送った・記録した");
}

/// W19-3b の実機（再起動の後）: 決め直せない時の 1 行は、読めた窓と読めなかった物（CG の窓番号が付かない窓）を言う。「無い」と断定しない・
/// retarget の完全な文を GuiError の Display（'…' が見つかりません）で包み直さない
#[cfg(unix)]
#[test]
fn w19_3b_retarget_refusal_says_what_was_read_and_what_was_not() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let d = tmp("w19-3b-why");
    let wl = r#"[[{"title":"README.md"},{"title":""},{"title":"draft.md","no_cg":true}]]"#;
    let sh = script(&d, "bridge.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{wl}'\nexec python3 '{py}'"));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
    r.check();
    let at = json!({ "pid": 999, "window_id": 1, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "draft.md" });
    let o = r.invoke(APP, "desktop.window.state", &at, None).0;
    let Outcome::Rejected { reason } = &o else { panic!("{o:?}") };
    assert!(reason.starts_with("“draft.md” isn't open in TextEdit. Nothing was changed. Open it, then ask again"), "{reason}");
    assert!(reason.contains("2 TextEdit windows were read: “README.md”“(untitled)”") && reason.contains("“draft.md” in TextEdit (the window number can't be read)"), "{reason}");
    assert!(!reason.contains("” wasn't found. Read the screen again"), "包み直した: {reason}");
    // 読めた窓が 1 つなら単数（rules.md I3: `1 window` は呼ぶ側の仕事）
    let one = r#"[[{"title":"README.md"},{"title":"draft.md","no_cg":true}]]"#;
    let sh = script(&d, "bridge-one.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{one}'\nexec python3 '{py}'"));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
    r.check();
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &at, None).0 else { panic!("決め直せない窓を選んだ") };
    assert!(reason.contains("(1 TextEdit window was read: “README.md”"), "{reason}");
    // app は答えたが窓番号の付かない窓だけ = 待っても直らない（「答えないので…少し待って」と言わない）
    let only_no_cg = r#"[[{"title":"draft.md","no_cg":true}]]"#;
    let sh = script(&d, "bridge-no-cg.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{only_no_cg}'\nexec python3 '{py}'"));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
    r.check();
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &at, None).0 else { panic!("窓番号の付かない窓には決め直さない") };
    assert!(reason.starts_with("“draft.md” isn't open in TextEdit. Nothing was changed") && reason.contains("0 TextEdit windows were read · not readable: “draft.md” in TextEdit (the window number can't be read)"), "{reason}");
    assert!(!reason.contains("isn't answering"), "答えた app を「答えない」と言わない: {reason}");
}

/// W19-3b 実機の ⑥: 裏に押し出された app は最初の AX の要求に 3 秒では答えない（load 166 の TextEdit で 1 回目 14.3 秒・2 回目 0.1 秒）。
/// 宛先を決め直す読みは、答えなかった app だけ AX の上限を伸ばして 1 回読み直し、元の上限に戻す。伸ばしても答えなければ「開いてから」と言わない。
/// 絞らない一覧（全部の app）は伸ばさない（1 つの忙しい app で長く止めない）
#[cfg(unix)]
#[test]
fn w19_3b_retarget_waits_longer_for_an_app_that_did_not_answer() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let wl = r#"[[{"title":"draft.md"}]]"#;
    let at = json!({ "pid": 999, "window_id": 1, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "draft.md" });
    let fake = |wake: &str| {
        let d = tmp(&format!("w19-3b-wake-{wake}"));
        let log = d.join("requests.jsonl");
        let sh = script(&d, "bridge.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{wl}' FAKE_WL_WAKE={wake} FAKE_LOG='{}'\nexec python3 '{py}'", log.display()));
        let mut r = AdapterRegistry::default();
        r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
        r.check();
        (r, log)
    };
    let set = |log: &PathBuf| -> Vec<f64> {
        let reqs = std::fs::read_to_string(log).unwrap_or_default();
        reqs.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["cmd"] == "config").filter_map(|v| v["ax_timeout"].as_f64()).collect()
    };
    let (mut r, log) = fake("10");
    let (o, obs) = r.invoke(APP, "desktop.window.state", &at, None);
    assert!(!matches!(o, Outcome::Rejected { .. }) && obs.is_some(), "伸ばして読み直せば決め直せる: {o:?}");
    assert_eq!(set(&log), [30.0, 1.0], "伸ばして、元の上限に戻す");
    // G31b: 伸ばした読み直しの window_list だけが budget_s 30 を持つ（本物の bridge は 3 × 30 秒まで読むので、読み手の 32 秒の中で打ち切らせる）。
    // 負の対照: windows_where の patient の要求から budget_s を外すと赤
    let budgets: Vec<Value> = fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["cmd"] == "window_list").map(|v| v["budget_s"].clone()).collect();
    assert_eq!(budgets, [Value::Null, json!(30.0), Value::Null], "決め直しの 1 回目と木の前の読みは渡さず、読み直しだけ 30 秒");
    let (mut r, log) = fake("10");
    assert_eq!(r.invoke(APP, "desktop.windows.list", &json!({}), None).1.unwrap().payload, json!([]));
    assert_eq!(set(&log), Vec::<f64>::new(), "絞らない一覧は伸ばさない");
    let (mut r, _) = fake("60");
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &at, None).0 else { panic!("30 秒でも答えない app は決め直せない") };
    assert!(reason.starts_with("TextEdit isn't answering, so the window with “draft.md” couldn't be found. Wait a moment, then ask again"), "{reason}");
    assert!(reason.contains("0 TextEdit windows were read · not readable: TextEdit (窓の一覧を読めません: app が応答しません"), "{reason}");
    assert!(!reason.contains("Open it, then"), "開いている窓に「開いてから」と言わない: {reason}");
}

/// W19-3b G2: 答えない app を読み直して待つ間、進行の 1 行（`set_progress`）に「X が答えないので待っています（最長 N 秒）」を出し、
/// 読み直しが終わったら消す（答えても答えなくても）。N = 待つ app の数 × 伸ばした後の 1 回の上限（AX 30 秒 + 2・G31b から bridge は window_list の
/// 読み直しを budget_s 30 秒で打ち切る = 文の秒と待ちの実体が同じ）。答えた app だけなら出さない。
/// 負の対照（手で取る）: `MacGui::patiently` の Some の 1 行を外すと赤
#[cfg(unix)]
#[test]
fn w19_3b_retarget_says_it_is_waiting_for_a_slow_app_until_it_answers() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let wl = r#"[[{"title":"draft.md"}]]"#;
    let at = json!({ "pid": 999, "window_id": 1, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "draft.md" });
    let said = |tag: &str, env: &str| -> Vec<Option<String>> {
        let d = tmp(&format!("w19-3b-g2-{tag}"));
        let sh = script(&d, "bridge.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{wl}' {env}\nexec python3 '{py}'"));
        let mut r = AdapterRegistry::default();
        r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
        r.check();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l = lines.clone();
        r.set_progress(Arc::new(move |line| l.lock().unwrap().push(line)));
        r.invoke(APP, "desktop.window.state", &at, None);
        let got = lines.lock().unwrap().clone();
        got
    };
    let waiting = |s: u32| Some(format!("Waiting because TextEdit isn't answering (up to {s} s)"));
    assert_eq!(said("wake", "FAKE_WL_WAKE=10"), [waiting(32), None], "読み直す間だけ出て、答えたら消える");
    assert_eq!(said("never", "FAKE_WL_WAKE=60"), [waiting(32), None], "伸ばしても答えない時も消す");
    assert_eq!(said("two", "FAKE_WL_WAKE=10 FAKE_APP_MORE_PID=223"), [waiting(64), None], "同じ app の 2 つの起動 = 2 × 32 秒（決め打ちしない）");
    assert_eq!(said("awake", ""), Vec::<Option<String>>::new(), "答えた app では出さない");
}

/// G31（本物の app の実測 docs/evidence/G31/log.md を偽の bridge の形に）: registry → MacGui の AI の道の 4 つの形。
/// 偽の bridge を env つきで起こし、act の記録と進行の 1 行を受ける registry を返す
#[cfg(unix)]
fn g31_fake(tag: &str, env: &str) -> (AdapterRegistry, PathBuf, PathBuf, Arc<Mutex<Vec<Option<String>>>>) {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let d = tmp(&format!("g31-{tag}"));
    let (log, acts) = (d.join("requests.jsonl"), d.join(gui::ACT_LOG_FILE));
    let sh = script(&d, "bridge.sh", &format!("export FAKE_LOG='{}' {env}\nexec python3 '{py}'", log.display()));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(gui_at(sh).with_act_log(acts.clone()))).unwrap();
    r.check();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l = lines.clone();
    r.set_progress(Arc::new(move |line| l.lock().unwrap().push(line)));
    (r, log, acts, lines)
}
#[cfg(unix)]
fn g31_reqs(log: &Path) -> Vec<Value> {
    fs::read_to_string(log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}
#[cfg(unix)]
fn g31_sent(log: &Path, cmd: &str) -> usize {
    g31_reqs(log).iter().filter(|v| v["cmd"] == cmd).count()
}
#[cfg(unix)]
fn g31_controls(obs: Option<Observation>) -> usize {
    obs.unwrap().payload["controls"].as_array().unwrap().len()
}
#[cfg(unix)]
const G31_STATE: &str = r#"{ "pid": 222, "window_id": 300, "app_name": "REAPER" }"#;

/// G31 ① Discord（Chromium）: 中身を頼んだ直後の枠だけの木（bridge の waking）は 1 秒おきに読み直して中身を待つ・待っても出なければ
/// 「まだ見せていない」の 1 行（枠だけの木を「無い」と読まない）。負の対照: `MacGui::read` の waking の読み直しを外すと赤
#[cfg(unix)]
#[test]
fn g31_chromium_window_is_read_again_until_it_shows_its_contents() {
    let state: Value = serde_json::from_str(G31_STATE).unwrap();
    let (mut r, log, _, said) = g31_fake("wake", "FAKE_AX_WAKING=2");
    let (o, obs) = r.invoke(APP, "desktop.window.state", &state, None);
    assert!(!matches!(o, Outcome::Rejected { .. }), "{o:?}");
    assert_eq!((g31_controls(obs), g31_sent(&log, "ax_tree")), (3, 3), "枠だけの 2 回を読み直して中身（3 つ）を返す");
    assert_eq!(*said.lock().unwrap(), [Some("Waiting for REAPER to show what's in its window".to_string()), None]);
    let (mut r, log, _, _) = g31_fake("wake-never", "FAKE_AX_WAKING=99");
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &state, None).0 else { panic!("枠だけの木を中身として返した") };
    assert_eq!(reason, "REAPER hasn't shown what's in its window yet. Wait a moment, then ask again");
    assert_eq!(g31_sent(&log, "ax_tree"), 5, "1 回 + 読み直し 4 回で止める");
}

/// G31 ② Zoom / REAPER（混んだ機械）: 窓の最初の読みも答えない空の木は AX の上限を伸ばして 1 回読み直し・元に戻す・それでも空なら「答えない」の 1 行
/// （前は空の木で window.state が controls 0 個の Ok・observe は「Reopen Shibubu」）。負の対照: 空の木の Err を外すと赤
#[cfg(unix)]
#[test]
fn g31_unanswering_window_is_read_again_and_never_reads_as_empty() {
    let state: Value = serde_json::from_str(G31_STATE).unwrap();
    let (mut r, log, _, said) = g31_fake("slow", "FAKE_AX_TREE_WAKE=10");
    let (o, obs) = r.invoke(APP, "desktop.window.state", &state, None);
    assert!(!matches!(o, Outcome::Rejected { .. }), "{o:?}");
    assert_eq!(g31_controls(obs), 3);
    let set: Vec<f64> = g31_reqs(&log).iter().filter(|v| v["cmd"] == "config").filter_map(|v| v["ax_timeout"].as_f64()).collect();
    assert_eq!(set, [30.0, 1.0], "伸ばして読み直し、元の上限に戻す");
    assert_eq!(*said.lock().unwrap(), [Some("Waiting because REAPER isn't answering (up to 32 s)".to_string()), None]);
    let (mut r, _, _, _) = g31_fake("dead", "FAKE_AX_TREE_WAKE=60");
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &state, None).0 else { panic!("答えない app の空の木を Ok にした") };
    assert_eq!(reason, "REAPER isn't answering, so its window couldn't be read. Wait a moment, then ask again");
}

/// owner 256（「300要素で打ち切るとか打ち切ることはしないで」）: 5000 要素の窓を bridge が 1000 ずつ区切って返しても、MacGui は続き（`more`）を読み終えるまで
/// 頼んで全部を読む（window.state の controls = 窓を除く全部）・区切りの間は進行の 1 行「Reading REAPER… N items so far」を出して最後に消す・4999 番目の名前を押せる。
/// 負の対照（cargo 無しで未測・壊し方だけ）: `AxTreeReq` に max_elements 300 を戻す（偽の bridge は前の bridge と同じく 300 で切り続き無し）→ 読みが Err で赤 /
/// `MacGui::read` の続きの loop を外す → controls 999 で赤・押す前の読みにも 4999 番目が無く TargetNotFound で赤
#[cfg(unix)]
#[test]
fn owner256_whole_window_is_read_however_many_elements_it_has() {
    let state: Value = serde_json::from_str(G31_STATE).unwrap();
    let (mut r, log, _, said) = g31_fake("many", "FAKE_AX_MANY=5000 FAKE_AX_SLICE=1000");
    let (o, obs) = r.invoke(APP, "desktop.window.state", &state, None);
    assert!(!matches!(o, Outcome::Rejected { .. }), "{o:?}");
    assert_eq!(g31_controls(obs), 5003, "窓を除く全部（枠・保存・本文 ＋ Item 1〜5000）");
    let trees: Vec<Value> = g31_reqs(&log).into_iter().filter(|v| v["cmd"] == "ax_tree").collect();
    assert_eq!((trees.len(), trees.iter().filter(|v| v["more"] == true).count()), (6, 5), "1 回目 ＋ 続き 5 回");
    assert!(trees.iter().all(|v| v.get("max_elements").is_none()), "読みの数の上限を送った: {trees:?}");
    let reading: Vec<Option<String>> = [1000, 2000, 3000, 4000, 5000].iter().map(|n| Some(format!("Reading REAPER… {n} items so far"))).chain([None]).collect();
    assert_eq!(*said.lock().unwrap(), reading);
    let click = json!({ "pid": 222, "window_id": 300, "app_name": "REAPER", "label": "Item 4999", "role": "AXButton" });
    let (o, _) = r.invoke(APP, "desktop.ax.click", &click, Some(&consent("desktop.ax.click", &["desktop.input"], &click)));
    assert!(matches!(o, Outcome::Unverified { .. }), "{o:?}");
    let pressed: Vec<String> = g31_reqs(&log).iter().filter(|v| v["cmd"] == "ax_press").map(|v| v["element"].as_str().unwrap_or_default().to_owned()).collect();
    assert!(matches!(pressed.as_slice(), [one] if one.ends_with(".5002")), "4999 番目（窓・枠・保存・本文の後の 5002 番）を 1 回: {pressed:?}");
}

/// ENGINE-ADJUST-1102 ①（owner 256「止めるのは人の Stop だけ」・interface = 呼ぶ側の形・本体の前は赤）: 窓の木の続き（bridge の more）を頼む前に人の Stop の印
/// （`AdapterRegistry::set_stop`）を見る。1 区切り目の進行の 1 行「Reading REAPER… 1000 items so far」が出た所で印が立つ（card の Stop）→ 続きを 1 回も頼まず、
/// 読みかけ（1000 個）を使わず Rejected の 1 行・進行の 1 行は消す・act の記録 0 件。
/// 負の対照（cargo 無しで未測・壊し方だけ）: `MacGui::read` の more の前の見張りを外す → ax_tree が 6 回・controls 5003 で赤
#[cfg(unix)]
#[test]
fn engine_adjust_stop_asks_for_no_more_slices_and_drops_the_partial_read() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let state: Value = serde_json::from_str(G31_STATE).unwrap();
    let (mut r, log, acts, _) = g31_fake("stop", "FAKE_AX_MANY=5000 FAKE_AX_SLICE=1000");
    let stop = Arc::new(AtomicBool::new(false));
    r.set_stop(stop.clone());
    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let (lines, flag) = (seen.clone(), stop.clone());
    // 進行の 1 行 = 1 区切り目を読み終えた所。そこで人が Stop を押す
    r.set_progress(Arc::new(move |line: Option<String>| {
        if line.as_deref().is_some_and(|l| l.contains("so far")) {
            flag.store(true, Ordering::SeqCst);
        }
        lines.lock().unwrap().push(line);
    }));
    let (o, obs) = r.invoke(APP, "desktop.window.state", &state, None);
    let Outcome::Rejected { reason } = o else { panic!("読みかけを使った: {o:?} controls {:?}", obs.map(|o| o.payload["controls"].as_array().map(Vec::len))) };
    assert!(reason.contains("Stopped by you"), "{reason}");
    let trees: Vec<Value> = g31_reqs(&log).into_iter().filter(|v| v["cmd"] == "ax_tree").collect();
    assert_eq!((trees.len(), trees.iter().filter(|v| v["more"] == true).count()), (1, 0), "印が立った後に続きを頼んだ: {trees:?}");
    assert_eq!(*seen.lock().unwrap(), [Some("Reading REAPER… 1000 items so far".to_string()), None], "進行の 1 行を消していない");
    assert!(!acts.exists(), "読むだけなのに act の記録を書いた");
}

/// G31 ③ Zoom: menu の押す前の読みが答えない（bridge の ax_cannot_complete）は「送っていない」= Rejected・act の記録 0 件
/// （前は「送ったかもしれない・2 回打つな」）。負の対照: `gui_result` の ax_cannot_complete の腕を外すと赤
#[cfg(unix)]
#[test]
fn g31_menu_that_could_not_be_read_says_nothing_was_sent() {
    let menu = json!({ "pid": 222, "app_name": "REAPER", "path": ["ファイル", "保存"] });
    let (mut r, log, acts, _) = g31_fake("menu-busy", "FAKE_AX_FAIL=ax_cannot_complete");
    let ok = consent("desktop.ax.menu", &["desktop.input"], &menu);
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.ax.menu", &menu, Some(&ok)).0 else { panic!("押す前に答えなかった menu を送ったかもしれない扱いにした") };
    assert_eq!(reason, "The app isn't answering, so nothing was sent. Wait a moment, then ask again");
    assert_eq!((g31_sent(&log, "ax_menu"), acts.exists()), (1, false), "送っていない操作を act の記録に書いた");
}

/// G31 ④ Discord の自己更新（pid が変わる）: menu / key は保存した pid が終わっていれば bundle の起動中の process 1 つに送り直す・2 つなら送らない・
/// bundle が無ければ前のまま「起動していない」。負の対照: `MacGui::retargeted` の repid の腕を外すと赤
#[cfg(unix)]
#[test]
fn g31_menu_and_key_follow_an_app_that_restarted_with_a_new_pid() {
    let ok = |a: &str, i: &Value| consent(a, &["desktop.input"], i);
    let moved = json!({ "pid": 111, "bundle_id": "com.cockos.reaper", "app_name": "REAPER", "path": ["ファイル", "保存"] });
    let (mut r, log, acts, _) = g31_fake("repid", "");
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &moved, Some(&ok("desktop.ax.menu", &moved))).0, Outcome::Unverified { .. }));
    let pids: Vec<Value> = g31_reqs(&log).iter().filter(|v| v["cmd"] == "ax_menu").map(|v| v["pid"].clone()).collect();
    assert_eq!(pids, [json!(222)], "起き直した REAPER（222）に送る");
    assert_eq!(gui::ActLog::read(&acts).unwrap()[0].receipt.target.pid, 222);
    let key = json!({ "pid": 111, "bundle_id": "com.cockos.reaper", "app_name": "REAPER", "key": "escape" });
    assert!(matches!(r.invoke(APP, "desktop.key", &key, Some(&ok("desktop.key", &key))).0, Outcome::Unverified { .. }));
    let same = json!({ "pid": 222, "bundle_id": "com.cockos.reaper", "app_name": "REAPER", "path": ["ファイル", "保存"] });
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &same, Some(&ok("desktop.ax.menu", &same))).0, Outcome::Unverified { .. }), "動いている pid はそのまま");
    let (mut r, log, _, _) = g31_fake("repid-two", "FAKE_APP_MORE_PID=223");
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.ax.menu", &moved, Some(&ok("desktop.ax.menu", &moved))).0 else { panic!("2 つの起動のどちらかに送った") };
    assert_eq!(reason, "2 copies of REAPER are running and Shibubu can't tell which. Quit the others, then ask again");
    assert_eq!(g31_sent(&log, "ax_menu"), 0);
    let old = json!({ "pid": 111, "app_name": "REAPER", "path": ["ファイル", "保存"] });
    let (mut r, log, _, _) = g31_fake("repid-nobundle", "");
    assert!(matches!(r.invoke(APP, "desktop.ax.menu", &old, Some(&ok("desktop.ax.menu", &old))).0, Outcome::Rejected { reason } if reason.contains("isn't running")));
    assert_eq!(g31_sent(&log, "ax_menu"), 0, "bundle が無い時は推測で別の pid に送らない");
}

/// G31b ① window_list の締め切り: bridge は window_list を 3 × 今の AX の上限まで読む（G31 実測: 起動直後の Zoom で 9.18 秒）。読み手も同じ長さを待つ
/// （前は 1 回分の上限 = AX + 2 秒で諦め、答えた app を「Timed out」で一覧から落とした）。負の対照: `MacGui::call` の window_list の腕を外すと赤
#[cfg(unix)]
#[test]
fn g31b_window_list_is_waited_for_as_long_as_the_bridge_reads() {
    // AX の上限 2 秒の bridge が 5.5 秒で答える: 1 回分 max(5, 2 + 2) = 5 秒は過ぎ、3 × 2 + 2 = 8 秒には入る
    let (mut r, log, _, _) = g31_fake("wl-slow", "FAKE_AX_TIMEOUT=2 FAKE_WL_SLOW=5.5");
    let ws = r.invoke(APP, "desktop.windows.list", &json!({}), None).1.expect("一覧の観測").payload;
    assert_eq!(ws.as_array().map(Vec::len), Some(1), "答えた app を時間切れで落とした: {ws}");
    assert_eq!(g31_sent(&log, "window_list"), 1);
}

/// 108 通目の #12（L5-STICK）: 宛先を決め直した時の窓の一覧と、木を読む時の一覧で窓の並びがずれても（Mirror が書類の窓を並べ直す最中）、
/// 名指しの書類の窓を読む（窓番号 300 のまま読むと 1 つ目の別の書類の窓を読む・無ければ「window wasn't found」）。
/// 負の対照: `MacGui::read` の `by_document`（読む一覧の中で書類から決め直す）を外すと ax_tree の handle が w1 で赤
#[cfg(unix)]
#[test]
fn l5_stick_read_follows_the_named_document_when_the_window_list_shifts_between_reads() {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
    let wl = r#"[[{"title":"draft.md"}],[{"title":"other.md"},{"title":"draft.md"}]]"#;
    let d = tmp("l5-stick-shift");
    let log = d.join("requests.jsonl");
    let sh = script(&d, "bridge.sh", &format!("export FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_WL='{wl}' FAKE_LOG='{}'\nexec python3 '{py}'", log.display()));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::mac_gui::MacGui::at(sh))).unwrap();
    r.check();
    let at = json!({ "pid": 999, "window_id": 1, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "draft.md" });
    let (o, _) = r.invoke(APP, "desktop.window.state", &at, None);
    assert!(!matches!(o, Outcome::Rejected { .. }), "{o:?}");
    let handles: Vec<Value> = fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["cmd"] == "ax_tree").map(|v| v["handle"].clone()).collect();
    assert_eq!(handles.first(), Some(&json!("w2")), "名指しの書類（draft.md）の窓を読まない: {handles:?}");
}

/// G31b ② REAPER の起動直後: AX の窓の枠が CG にまだ無い窓（cg_match none・AeroSpace が並べ直す間・G31 log.md:12〜14）は短く読み直して窓番号を待つ・
/// 待つ間は進行の 1 行。読み直しても出なければ「無い」と言わず、待った秒と bridge の原文を 1 行に（木は読まない）。
/// 負の対照: `MacGui::window_list` の読み直しを外すと赤
#[cfg(unix)]
#[test]
fn g31b_window_not_yet_on_the_cg_list_is_read_again() {
    let settling = r#"[[{"title":"REAPER v7.81","cg":"none"}],[{"title":"REAPER v7.81"}]]"#;
    let (mut r, log, _, said) = g31_fake("settle", &format!("FAKE_WL='{settling}'"));
    let ws = r.invoke(APP, "desktop.windows.list", &json!({}), None).1.expect("一覧の観測").payload;
    assert_eq!(ws[0]["window_id"], 300, "読み直して窓番号の付いた窓を返す: {ws}");
    assert_eq!(g31_sent(&log, "window_list"), 2);
    assert_eq!(*said.lock().unwrap(), [Some("Waiting for REAPER to finish placing its window".to_string()), None]);
    let never = r#"[[{"title":"REAPER v7.81","cg":"none"}]]"#;
    let (mut r, log, _, _) = g31_fake("settle-never", &format!("FAKE_WL='{never}'"));
    let at = json!({ "pid": 111, "window_id": 1, "app_name": "REAPER", "bundle_id": "com.cockos.reaper" });
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &at, None).0 else { panic!("CG に枠の無い窓に決めた") };
    assert!(reason.starts_with("REAPER is still placing its window, so its window couldn't be found. Wait a moment, then ask again"), "{reason}");
    assert!(reason.contains("“REAPER v7.81” in REAPER (the window number can't be read · waited ") && reason.contains(" s · cg_match: none)"), "{reason}");
    assert_eq!((g31_sent(&log, "window_list"), g31_sent(&log, "ax_tree")), (4, 0), "1 回 + 読み直し 3 回で止める");
}

/// G31b ③ AXDocument の無い窓（REAPER・Zoom）: document の無い保存した手順の宛先は、保存した窓が無くなっていれば窓の役割で決め直す
/// （REAPER の起動直後の評価版の「About」は main だが動かせない → 本体・Zoom の唯一の窓は AXDialog → それ）。同じ役割の窓が 2 つ以上なら
/// 何も読まず「どの窓か」の 1 行・保存した窓が今も在ればそのまま。負の対照: `MacGui::retargeted` の役割の腕を外すと赤
#[cfg(unix)]
#[test]
fn g31b_window_without_a_document_is_chosen_by_its_role() {
    let at = json!({ "pid": 111, "window_id": 1, "app_name": "REAPER", "bundle_id": "com.cockos.reaper" });
    let trees = |log: &Path| -> Vec<Value> { g31_reqs(log).iter().filter(|v| v["cmd"] == "ax_tree").map(|v| v["handle"].clone()).collect() };
    let reaper = r#"[[{"title":"About REAPER v7.81","main":true,"movable":false},{"title":"REAPER v7.81 - EVALUATION LICENSE","main":false}]]"#;
    let (mut r, log, _, _) = g31_fake("role-reaper", &format!("FAKE_WL='{reaper}'"));
    let (o, obs) = r.invoke(APP, "desktop.window.state", &at, None);
    assert!(!matches!(o, Outcome::Rejected { .. }), "{o:?}");
    assert_eq!(obs.expect("窓の観測").payload["window_id"], 301, "本体の窓に決め直す");
    assert_eq!(trees(&log), [json!("w2")]);
    let zoom = r#"[[{"title":"Zoom Workplace","subrole":"AXDialog"}]]"#;
    let (mut r, log, _, _) = g31_fake("role-zoom", &format!("FAKE_APP_BUNDLE=us.zoom.xos FAKE_APP_NAME=zoom.us FAKE_WL='{zoom}'"));
    let z = json!({ "pid": 111, "window_id": 1, "app_name": "zoom.us", "bundle_id": "us.zoom.xos" });
    let o = r.invoke(APP, "desktop.window.state", &z, None).0;
    assert!(!matches!(o, Outcome::Rejected { .. }), "1 つの AXDialog を選ばなかった: {o:?}");
    assert_eq!(trees(&log), [json!("w1")]);
    let two = r#"[[{"title":"a","main":false},{"title":"b","main":false}]]"#;
    let (mut r, log, _, _) = g31_fake("role-two", &format!("FAKE_WL='{two}'"));
    let Outcome::Rejected { reason } = r.invoke(APP, "desktop.window.state", &at, None).0 else { panic!("決められない窓の 1 つに繋いだ") };
    assert!(reason.starts_with("2 windows of REAPER are open and Shibubu can't tell which. Bring the one you want to the front, then ask again"), "{reason}");
    assert_eq!(trees(&log), Vec::<Value>::new());
    let kept = json!({ "pid": 222, "window_id": 301, "app_name": "REAPER", "bundle_id": "com.cockos.reaper" });
    let body_and_mixer = r#"[[{"title":"REAPER"},{"title":"Mixer","main":false}]]"#;
    let (mut r, log, _, _) = g31_fake("role-kept", &format!("FAKE_WL='{body_and_mixer}'"));
    assert!(!matches!(r.invoke(APP, "desktop.window.state", &kept, None).0, Outcome::Rejected { .. }));
    assert_eq!(trees(&log), [json!("w2")], "保存した窓（main でない Mixer）を役割で選び直した");
}

/// W19-1 Capability Registry「app 固有 / 複数 app で再利用できる を区別」: MacGui の AX の能力（窓の一覧・読み・click・type_text・menu・key）は
/// どの app の窓にも使うので Shared（provider の "desktop" は bundle id でも origin でもない）。公式・確かめる前は Unverified のまま
#[test]
fn mac_gui_records_are_shared_across_apps() {
    use domain::capability::{Origin, Reuse, Verification};
    let g = adapters::mac_gui::MacGui::at(PathBuf::from("/nonexistent/shibubu-bridge"));
    let rec = g.records();
    assert_eq!(rec.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), g.specs().iter().map(|s| s.id).collect::<Vec<_>>());
    for r in &rec {
        assert_eq!((&r.reuse, &r.origin, &r.verification), (&Reuse::Shared, &Origin::Official, &Verification::Unverified), "{}", r.id);
    }
}

#[test]
fn web_json_reads_over_loopback_and_an_unreachable_source_degrades() {
    let (base, log) = fake(vec![("GET", "/data.json", 200, json!({"n": 1}))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::web_json::WebJson::new(Some(format!("{base}/data.json")), Some("k".into())))).unwrap();
    r.check();
    let (o, obs) = r.invoke(APP, "web.json.read", &json!({}), None);
    assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].starts_with("GET 127.0.0.1:") && !evidence[0].contains("data.json")), "{o:?}");
    assert_eq!(obs.unwrap().payload["items"]["n"], 1);
    assert_eq!(log.lock().unwrap()[0].auth.as_deref(), Some("Bearer k"));
    let mut down = adapters::web_json::WebJson::new(Some(format!("{}/data.json", closed_port())), None);
    down.check();
    assert_eq!((down.readiness("web.json.read"), down.sources()[0].readiness), (CapabilityReadiness::Degraded, SourceReadiness::Degraded));
    for bad in ["http://example.com/x.json", "https://user:pw@example.com/x.json", "http://127.0.0.1.evil.example/x"] {
        assert_eq!(adapters::web_json::WebJson::new(Some(bad.into()), None).readiness("web.json.read"), CapabilityReadiness::RequiresConnection, "{bad}");
    }
}

#[cfg(unix)]
#[test]
fn trusted_cli_is_an_external_write_and_runs_only_with_approval() {
    let d = tmp("tcli");
    let marker = d.join("ran");
    let cli = script(&d, "touch.sh", &format!("touch '{}'", marker.display()));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![feature(&cli, "")]))).unwrap();
    r.check();
    assert_eq!(r.tier("cli.trusted.run"), DisplayTier::Ready);
    let input = json!({"feature_id": 9});
    assert!(matches!(r.invoke(APP, "cli.trusted.run", &input, None).0, Outcome::Rejected { .. }));
    assert!(!marker.exists(), "承認なしで走った");
    let unknown = json!({"feature_id": 10});
    assert!(matches!(r.invoke(APP, "cli.trusted.run", &unknown, Some(&consent("cli.trusted.run", &["exec:10"], &unknown))).0, Outcome::Rejected { reason } if reason.contains("登録されていません")));
    assert!(matches!(r.invoke(APP, "cli.trusted.run", &input, Some(&consent("cli.trusted.run", &["exec:9"], &input))).0, Outcome::Unverified { reason } if reason.contains("自己申告")));
    assert!(marker.exists());
    // 実行ファイルが消えた → check で接続が必要
    fs::remove_file(&cli).unwrap();
    r.check();
    assert_eq!(r.tier("cli.trusted.run"), DisplayTier::Available);
}

#[test]
fn google_adapter_wraps_src_google_and_keeps_writes_honest() {
    let t1 = json!({"id": "t1", "messages": [{"id": "m1", "payload": {"headers": [{"name": "Subject", "value": "Hi"}, {"name": "From", "value": "a@x.io"}]}}]});
    let draft = json!({"to": "b@x.io", "subject": "件名", "body": "本文"});
    // drafts.get の読み戻し（印 = 承認した入力の digest）と events.get の読み戻し（件名・開始・終了を瞬間で比べる）
    let echo = json!({"id": "d1", "message": {"id": "m1", "payload": {"mimeType": "text/plain", "headers": [{"name": "To", "value": "b@x.io"}, {"name": "Subject", "value": "件名"},
        {"name": "X-Kangae-Key", "value": sdk::payload_digest("gmail.drafts.create", &draft)}], "body": {"data": crypto::base64("本文".as_bytes(), true)}}}});
    let event_echo = json!({"summary": "打合せ", "start": {"dateTime": "2026-10-20T01:00:00Z"}, "end": {"dateTime": "2026-10-20T11:00:00+09:00"}, "htmlLink": "https://calendar.google.com/e1"});
    let (base, log) = fake(vec![
        ("GET", "/gmail/v1/users/me/profile", 200, json!({"emailAddress": "me@x.io"})),
        ("GET", "/gmail/v1/users/me/threads/t1", 200, t1),
        ("GET", "/gmail/v1/users/me/threads", 200, json!({"threads": [{"id": "t1", "snippet": "s"}]})),
        ("GET", "/gmail/v1/users/me/drafts/d1", 200, echo),
        ("GET", "/gmail/v1/users/me/drafts", 200, json!({"drafts": []})),
        ("POST", "/gmail/v1/users/me/drafts", 200, json!({"id": "d1"})),
        ("GET", "/calendar/v3/calendars/primary/events/kangae", 200, event_echo),
        ("GET", "/calendar/v3/calendars/primary/events", 200, json!({"items": [{"id": "e0", "summary": "既存", "start": {"date": "2026-10-18"}, "end": {"date": "2026-10-19"}}]})),
        ("POST", "/calendar/v3/calendars/primary/events", 200, json!({"id": "e1", "htmlLink": "https://calendar.google.com/e1"})),
    ]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE))).unwrap();
    r.check();
    for id in ["gmail.threads.read", "gmail.drafts.create", "calendar.events.read", "calendar.events.create"] { assert_eq!(r.tier(id), DisplayTier::Ready, "{id}") }
    let (o, obs) = r.invoke(APP, "gmail.threads.read", &json!({"query": "newer_than:1d"}), None);
    let obs = obs.unwrap();
    assert!(matches!(o, Outcome::Verified { .. }));
    assert_eq!((obs.source_ref.as_str(), obs.payload[0]["subject"].as_str()), ("google.gmail", Some("Hi")));
    let (o, obs) = r.invoke(APP, "calendar.events.read", &json!({}), None);
    assert!(matches!(o, Outcome::Verified { .. }) && obs.unwrap().payload[0]["summary"] == "既存");
    // 下書き: 承認つきで作る（送信はしない）。宛先が mail address でなければ送る前に止める
    let ok = |input: &Value| consent("gmail.drafts.create", &["mail.compose"], input);
    assert!(matches!(r.invoke(APP, "gmail.drafts.create", &draft, None).0, Outcome::Rejected { .. }));
    let o = r.invoke(APP, "gmail.drafts.create", &draft, Some(&ok(&draft))).0;
    assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].starts_with("drafts.get d1")), "{o:?}");
    let nobody = json!({"to": "nobody", "subject": "s", "body": ""});
    assert!(matches!(r.invoke(APP, "gmail.drafts.create", &nobody, Some(&ok(&nobody))).0, Outcome::Rejected { reason } if reason.contains("recipient")));
    // 予定: 参加者つきは招待 mail が出るので作らない。参加者なしは作る
    let cal = |input: &Value| consent("calendar.events.create", &["calendar.write"], input);
    let event = json!({"summary": "打合せ", "start": "2026-10-20T10:00:00+09:00", "end": "2026-10-20T11:00:00+09:00"});
    let mut invite = event.clone();
    invite["attendees"] = json!(["c@x.io"]);
    assert!(matches!(r.invoke(APP, "calendar.events.create", &invite, Some(&cal(&invite))).0, Outcome::Rejected { reason } if reason.contains("招待")));
    let o = r.invoke(APP, "calendar.events.create", &event, Some(&cal(&event))).0;
    assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].starts_with("events.get kangae") && evidence[1].ends_with("/e1")), "{o:?}");
    {
        let log = log.lock().unwrap();
        assert_eq!(log.iter().filter(|q| q.method == "POST" && q.path.starts_with("/calendar")).count(), 1, "参加者つきは送っていない");
        assert_eq!(log.iter().filter(|q| q.method == "POST" && q.path.starts_with("/gmail")).count(), 1, "宛先の不正な下書きは送っていない");
        assert!(log.iter().all(|q| !q.path.contains("/send")), "送信の endpoint は叩かない");
        assert!(log.iter().all(|q| q.auth.as_deref() == Some("Bearer gtok")));
    }
    // 同意していない scope の action は接続が必要（gmail.readonly だけ）。Client が無ければ全部
    let only_read = adapters::google::Google::new(Some(client(&base)), "https://www.googleapis.com/auth/gmail.readonly");
    assert_eq!(only_read.readiness("gmail.drafts.create"), CapabilityReadiness::RequiresConnection);
    assert_eq!(only_read.readiness("gmail.threads.read"), CapabilityReadiness::ConnectedUnverified);
    assert_eq!(adapters::google::Google::new(None, ALL_GOOGLE).readiness("gmail.threads.read"), CapabilityReadiness::RequiresConnection);
}

#[test]
fn google_failures_are_classified_and_writes_after_send_are_effect_unknown() {
    let (base, _) = fake(vec![
        ("GET", "/gmail/v1/users/me/profile", 200, json!({})),
        ("GET", "/calendar/v3/calendars/primary/events", 401, json!({"error": {"code": 401, "message": "Invalid Credentials"}})),
        ("GET", "/gmail/v1/users/me/drafts", 200, json!({"drafts": []})),
        ("POST", "/gmail/v1/users/me/drafts", 0, json!({})),
        ("POST", "/calendar/v3/calendars/primary/events", 503, json!({})),
    ]);
    let mut g = adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE);
    g.check();
    assert_eq!(g.readiness("gmail.threads.read"), CapabilityReadiness::Ready);
    assert_eq!(g.readiness("calendar.events.read"), CapabilityReadiness::Revoked, "401 = 認証切れ → 再接続する");
    let o = g.invoke("gmail.drafts.create", &json!({"to": "b@x.io", "subject": "s", "body": "b"})).0;
    assert!(matches!(&o, Outcome::EffectUnknown { reason } if reason.contains("Can't tell whether it was created")), "{o:?}");
    let o = g.invoke("calendar.events.create", &json!({"summary": "s", "start": "2026-10-20", "end": "2026-10-21"})).0;
    assert!(matches!(&o, Outcome::EffectUnknown { .. }), "5xx の後は作られた可能性: {o:?}");
}

// ---------- 修正ラウンド 1（確定した指摘ごとに 1 本。指摘の id を名に添える） ----------
fn seen(r: &AdapterRegistry, id: &str) -> Option<String> { r.sources().into_iter().find(|s| s.id == id).and_then(|s| s.last_observed_at) }

/// AD-R1-2: agent は redirects(0)（token を転送先へ送らない）で、ureq 2.12 は 3xx を Ok で返す。転送を受理・作成済み・読めた・疎通とは言わない。
/// 口は 4 つ（src/github.rs の send / web_read_at・src/google/mod.rs の finish / Client::call）なので 1 本ずつ
#[test]
fn ad_r1_2_github_redirect_is_failed_not_accepted_or_ready() {
    let moved = json!({"message": "Temporary Redirect", "url": "https://api.github.com/repositories/1/issues"});
    let (base, log) = fake(vec![("GET", "/rate_limit", 302, json!({})), ("POST", "/repos/o/r/issues", 307, moved), ("GET", "/repos/o/r/pulls", 301, json!({"message": "Moved Permanently"}))]);
    // 改名した repo への作成: 何も作られていない = Failed（「受理（結果未検証）」にしない）。転送先へは送り直さない
    assert_eq!(api(&base).create_issue_outcome("o/r", "t", ""), Outcome::Failed { error: ErrorCode::TargetNotFound });
    assert_eq!(log.lock().unwrap().iter().filter(|q| q.method == "POST").count(), 1);
    assert!(matches!(api(&base).pulls("o/r"), Err(github::Fail::Status { code: 301, message, .. }) if message.contains("redirect")));
    let mut gh = adapters::github::GitHub::new(api(&base));
    gh.check();
    assert_eq!(gh.readiness("github.issues.create"), CapabilityReadiness::ConnectedUnverified, "/rate_limit の 302 は疎通の証拠にならない");
    assert_eq!(gh.invoke("github.issues.create", &json!({"repo": "o/r", "title": "t"})).0, Outcome::Failed { error: ErrorCode::TargetNotFound });
    assert_eq!(gh.readiness("github.issues.create"), CapabilityReadiness::ConnectedUnverified, "転送を受理として記録しない");
}

#[test]
fn ad_r1_2_web_json_redirect_is_not_a_read() {
    let (base, _) = fake(vec![("GET", "/data.json", 302, json!({"items": ["転送先の物"]}))]);
    let mut w = adapters::web_json::WebJson::new(Some(format!("{base}/data.json")), None);
    assert_eq!(w.invoke("web.json.read", &json!({})), (Outcome::Failed { error: ErrorCode::TargetNotFound }, None));
}

#[test]
fn ad_r1_2_google_redirect_is_failed_not_accepted_or_ready() {
    // 下書きの作成に 307 → 受理（Unverified）にも作成済み（Verified）にもしない。profile の 302 は Ready にしない
    for body in [json!({"error": {"code": 307, "message": "Temporary Redirect"}}), json!({"id": "d1"})] {
        let (base, _) = fake(vec![("GET", "/gmail/v1/users/me/profile", 302, json!({})), ("GET", "/gmail/v1/users/me/drafts", 200, json!({"drafts": []})),
            ("POST", "/gmail/v1/users/me/drafts", 307, body.clone())]);
        let mut g = adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE);
        g.check();
        assert_eq!(g.readiness("gmail.threads.read"), CapabilityReadiness::ConnectedUnverified, "{body}");
        let o = g.invoke("gmail.drafts.create", &json!({"to": "b@x.io", "subject": "s", "body": "b"})).0;
        assert!(matches!(o, Outcome::Failed { .. }), "{body}: {o:?}");
    }
}

#[test]
fn ad_r1_2_google_write_call_redirect_is_neither_accepted_nor_unknown() {
    // 書き込みと読み戻しの口（p/google の create_*_checked が使う）も同じ: 3xx は Fail（受理でも「作られたか不明」でもない）
    let (base, _) = fake(vec![("POST", "/gmail/v1/users/me/drafts", 307, json!({"id": "d1"}))]);
    let f = client(&base).call("POST", &format!("{base}/gmail/v1/users/me/drafts"), Some(&json!({}))).expect_err("3xx を Ok にした");
    assert_eq!((f.status, f.accepted(), f.unknown()), (Some(307), false, false));
}

/// AD-R1-3: 鮮度（last_observed_at）はその source を読んだ Observation だけが進める。書き込み・check・別の source の読み取りでは進めない
#[test]
fn ad_r1_3_freshness_moves_only_with_a_read_of_that_source() {
    let mut s = SourceSync::default();
    s.record(1_000, &Outcome::Verified { evidence: vec![] }, Some(&sdk::observe("github.issues", 1_000, json!([]))));
    s.record(2_000, &Outcome::Verified { evidence: vec!["#3".into()] }, None); // Issue 作成の受領
    s.record(2_100, &Outcome::Unverified { reason: "exit 0".into() }, None); // CLI / click の受理
    assert_eq!(s.capability(true), CapabilityReadiness::Ready, "書き込みの成功は接続の証拠にはなる");
    assert_eq!(s.last_observed_at("github.issues"), Some(google::rfc3339_utc(1_000)), "書き込みで「読んだ時刻」を進めない");
    s.record(2_200, &Outcome::Verified { evidence: vec![] }, Some(&sdk::observe("github.pulls", 2_200, json!([]))));
    assert_eq!(s.last_observed_at("github.issues"), Some(google::rfc3339_utc(1_000)), "別の source の読み取りで進めない");
    let src = sdk::source("github.issues", AdapterKind::Api, "github", &s, true);
    let binding = DataBinding { source_ref: "github.issues".into(), path: "items".into(), extra: Default::default() };
    assert_eq!(binding.readiness(std::slice::from_ref(&src), Some(&google::rfc3339_utc(1_500))), BindingReadiness::Stale, "1_000 に読んだ一覧は 1_500 の基準で古い");
    // registry: check（GET /rate_limit）は何も読んでいない。issues を読んでも pulls は読んでいない
    let (base, _) = fake(vec![("GET", "/rate_limit", 200, json!({})), ("GET", "/repos/o/r/issues", 200, json!([]))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(api(&base)))).unwrap();
    r.check();
    assert_eq!((seen(&r, "github.issues"), seen(&r, "github.pulls")), (None, None), "check は data を読んでいない");
    let obs = r.invoke(APP, "github.issues.list", &json!({"repo": "o/r"}), None).1.unwrap();
    assert_eq!((seen(&r, "github.issues"), seen(&r, "github.pulls")), (Some(obs.observed_at), None));
    // Web JSON: check は同じ GET でも中身を捨てるので鮮度を進めない
    let (wbase, _) = fake(vec![("GET", "/data.json", 200, json!({"n": 1}))]);
    let mut w = adapters::web_json::WebJson::new(Some(format!("{wbase}/data.json")), None);
    w.check();
    assert_eq!((w.readiness("web.json.read"), w.sources()[0].last_observed_at.clone()), (CapabilityReadiness::Ready, None));
    let obs = w.invoke("web.json.read", &json!({})).1.unwrap();
    assert_eq!(w.sources()[0].last_observed_at, Some(obs.observed_at));
}

/// AD-R1-4 / AD-SEC-3: path の無い URL（`http://host?key=…`）でも、根拠（evidence）に query を書かない
#[test]
fn ad_r1_4_web_json_evidence_never_carries_the_query_even_without_a_path() {
    let (base, log) = fake(vec![("GET", "/", 200, json!({"n": 1}))]);
    let mut w = adapters::web_json::WebJson::new(Some(format!("{base}?api_key=SECRET")), None);
    let (o, obs) = w.invoke("web.json.read", &json!({}));
    let Outcome::Verified { evidence } = &o else { panic!("{o:?}") };
    assert!(evidence[0].starts_with(&format!("GET {} · ", &base["http://".len()..])) && !evidence[0].contains("SECRET"), "{evidence:?}");
    assert_eq!(obs.unwrap().payload["items"]["n"], 1);
    assert!(log.lock().unwrap()[0].path.ends_with("?api_key=SECRET"), "鍵は request にだけ載る");
}

/// AD-R1-5: §10 の段（Unavailable / Available / Connected / Ready / Verified）を持つ enum は domain の DisplayTier 1 つだけ（2 つ目は食い違う）
#[test]
fn ad_r1_5_display_tier_is_the_only_enum_with_the_section_10_tiers() {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for p in fs::read_dir(dir).unwrap().flatten().map(|e| e.path()) {
            if p.is_dir() { walk(&p, out); continue }
            if p.extension().is_none_or(|x| x != "rs") { continue }
            let text = fs::read_to_string(&p).unwrap();
            for (i, _) in text.match_indices("enum ") {
                let rest = &text[i + 5..];
                let body = &rest[..rest.find('}').unwrap_or(rest.len())];
                if ["Unavailable", "Available", "Connected", "Ready", "Verified"].iter().all(|w| body.contains(w)) {
                    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    out.push(format!("{}:{name}", p.strip_prefix(concat!(env!("CARGO_MANIFEST_DIR"), "/src/")).unwrap().display()));
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut found);
    assert_eq!(found, vec!["domain/capability.rs:DisplayTier".to_string()]);
}

/// AD-SEC-1: 信頼済 CLI の承認は CLI 1 本ごと（exec:<feature_id>・CD-01）。9 への承認・平たい "exec" では 10 は走らない
#[cfg(unix)]
#[test]
fn ad_sec_1_trusted_cli_approval_is_bound_to_one_cli() {
    let d = tmp("tcli2");
    let (ran9, ran10) = (d.join("ran-9"), d.join("ran-10"));
    let cli9 = script(&d, "nine.sh", &format!("touch '{}'", ran9.display()));
    let cli10 = script(&d, "ten.sh", &format!("touch '{}'", ran10.display()));
    let (f9, f10) = (feature(&cli9, ""), model::RepoFeature { id: 10, ..feature(&cli10, "") });
    let (exec9, exec10) = ("exec:9".to_string(), "exec:10".to_string());
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![f9, f10]))).unwrap();
    r.check();
    let run10 = json!({"feature_id": 10});
    // digest は 10 の入力に合っていても、scope が 10 の物でなければ走らせない（9 の物・平たい exec）
    for scopes in [&[exec9.as_str()][..], &["exec"][..]] {
        let o = r.invoke(APP, "cli.trusted.run", &run10, Some(&consent("cli.trusted.run", scopes, &run10))).0;
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("scope")), "{scopes:?}: {o:?}");
    }
    assert!(!ran10.exists(), "別の CLI への承認で 10 が走った");
    assert!(matches!(r.invoke(APP, "cli.trusted.run", &run10, Some(&consent("cli.trusted.run", &[&exec10], &run10))).0, Outcome::Unverified { .. }));
    assert!(ran10.exists() && !ran9.exists());
    // 宣言は有効な CLI ごとの名（grant の scope 検査 validate_grant が通る名）
    assert_eq!(r.actions()[0].required_scopes, vec![exec9, exec10]);
}

/// AD-SEC-2: 承認は「見せた exact payload・その app・その run」に 1 回だけ効く（DESIGN §5.3・ADR-005 §5）。照合ごとに 1 本
fn github_ready() -> (AdapterRegistry, Log) {
    let (base, log) = fake(vec![("GET", "/rate_limit", 200, json!({})), ("POST", "/repos/", 201, json!({"number": 5, "html_url": "https://github.com/o/r/issues/5"}))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(api(&base)))).unwrap();
    r.check();
    (r, log)
}
fn posts(log: &Log) -> usize { log.lock().unwrap().iter().filter(|q| q.method == "POST").count() }
fn shown() -> Value { json!({"repo": "o/r", "title": "見せた件名", "body": "見せた本文"}) }

#[test]
fn ad_sec_2_approval_does_not_cover_a_swapped_payload_or_a_receipt_without_digest() {
    let (mut r, log) = github_ready();
    let ok = consent("github.issues.create", &["github.issues.write"], &shown());
    let swapped = json!({"repo": "evil/repo", "title": "見せた件名", "body": "別の本文"});
    assert!(matches!(r.invoke(APP, "github.issues.create", &swapped, Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("承認した内容と違います")));
    let no_digest = ConsentReceipt { payload_digest: None, ..ok.clone() };
    assert!(matches!(r.invoke(APP, "github.issues.create", &shown(), Some(&no_digest)).0, Outcome::Rejected { reason } if reason.contains("承認した内容と違います")));
    assert_eq!(posts(&log), 0, "見せていない payload を送った");
    assert!(matches!(r.invoke(APP, "github.issues.create", &shown(), Some(&ok)).0, Outcome::Verified { .. }), "見せた物は送れる");
}

#[test]
fn ad_sec_2_approval_is_bound_to_its_app_and_run() {
    let (mut r, log) = github_ready();
    let ok = consent("github.issues.create", &["github.issues.write"], &shown());
    let other_app = ConsentReceipt { app_id: "other".into(), ..ok.clone() };
    let run_bound = ConsentReceipt { subject: ConsentSubject::Action { action_ref: "github.issues.create".into(), run_id: Some("run-1".into()) }, ..ok.clone() };
    for (c, caller) in [(&other_app, APP), (&run_bound, APP), (&run_bound, Caller { app_id: "test", run_id: Some("run-2") })] {
        let o = r.invoke(caller, "github.issues.create", &shown(), Some(c)).0;
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("別の app / run")), "{caller:?}: {o:?}");
    }
    assert_eq!(posts(&log), 0, "別の app / run の承認で送った");
    assert!(matches!(r.invoke(Caller { app_id: "test", run_id: Some("run-1") }, "github.issues.create", &shown(), Some(&run_bound)).0, Outcome::Verified { .. }), "その run からなら通る");
}

#[test]
fn ad_sec_2_one_approval_runs_once_even_after_cleanup() {
    let (mut r, log) = github_ready();
    let ok = consent("github.issues.create", &["github.issues.write"], &shown());
    assert!(matches!(r.invoke(APP, "github.issues.create", &shown(), Some(&ok)).0, Outcome::Verified { .. }));
    assert!(matches!(r.invoke(APP, "github.issues.create", &shown(), Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("使用済み")));
    assert_eq!(posts(&log), 1, "同じ承認で 2 通目を送った");
    // 後始末（cleanup）でも使用済みは消さない（消すと同じ承認の送り直しが通る）
    const WRITE: &[Spec] = &[Spec { id: "local.echo.write", approval: true, ..ECHO_SPEC }];
    let mut f = AdapterRegistry::default();
    f.register(Box::new(Fake { id: "w", specs: WRITE })).unwrap();
    let input = json!({"x": 1});
    let once = consent("local.echo.write", &[], &input);
    assert!(matches!(f.invoke(APP, "local.echo.write", &input, Some(&once)).0, Outcome::Verified { .. }));
    f.cleanup();
    assert!(matches!(f.invoke(APP, "local.echo.write", &input, Some(&once)).0, Outcome::Rejected { reason } if reason.contains("使用済み")));
}

/// r10 ①（dogfood r10・2 手の card の承認の後に「この承認は使用済み」）: adapter が押していない（対象が無い・1 つに絞れない）時は承認を使用済みにしない =
/// 同じ run の同じ内容のやり直し（W5 の探す段の後に同じ手から）に同じ承認が効く。送った後の 2 回目は今まで通り断る。
/// 負の対照: `untouched` を常に false にする → 2 回目の invoke が「使用済み」で赤。
#[test]
fn r10_an_approval_is_not_used_up_by_a_step_that_pressed_nothing() {
    const WRITE: &[Spec] = &[Spec { id: "local.echo.write", approval: true, ..ECHO_SPEC }];
    struct Late(u32);
    impl Adapter for Late {
        fn id(&self) -> &'static str { "late" }
        fn kind(&self) -> AdapterKind { AdapterKind::LocalCore }
        fn route(&self) -> Route { Route::OfficialApi }
        fn provider(&self) -> &'static str { "local" }
        fn specs(&self) -> &'static [Spec] { WRITE }
        fn readiness(&self, _: &str) -> CapabilityReadiness { CapabilityReadiness::Ready }
        fn sources(&self) -> Vec<domain::binding::Source> { vec![] }
        fn check(&mut self) {}
        fn invoke(&mut self, _: &str, input: &Value) -> (Outcome, Option<Observation>) {
            self.0 += 1;
            if self.0 == 1 { (Outcome::Failed { error: ErrorCode::TargetNotFound }, None) } else { sdk::read_result(Ok(input.clone()), "local.echo", "echo", 0) }
        }
        fn cleanup(&mut self) {}
    }
    let mut r = AdapterRegistry::default();
    r.register(Box::new(Late(0))).unwrap();
    let input = json!({"x": 1});
    let once = consent("local.echo.write", &[], &input);
    assert!(matches!(r.invoke(APP, "local.echo.write", &input, Some(&once)).0, Outcome::Failed { error: ErrorCode::TargetNotFound }));
    assert!(matches!(r.invoke(APP, "local.echo.write", &input, Some(&once)).0, Outcome::Verified { .. }), "押していない 1 回目で承認を使い切った");
    assert!(matches!(r.invoke(APP, "local.echo.write", &input, Some(&once)).0, Outcome::Rejected { reason } if reason.contains("使用済み")), "送った後の 2 回目を通した");
}

/// AD-SEC-4: `@` の前は userinfo。`http://127.0.0.1:@evil.example` の本当の host は evil.example なので、token を載せる base にしない
#[test]
fn ad_sec_4_loopback_refuses_userinfo_so_the_token_never_goes_elsewhere() {
    for evil in ["http://127.0.0.1:@evil.example", "http://127.0.0.1:8080@evil.example/x", "http://127.0.0.1:x@evil.example", "http://127.0.0.1@evil.example", "http://127.0.0.1:/x"] {
        assert!(!github::loopback(evil), "{evil}");
        assert_eq!(github::Api::new(evil, Some("t".into())).base, "https://api.github.com", "{evil}");
        assert!(!github::web_url_ok(evil), "{evil}");
    }
    for ok in ["http://127.0.0.1", "http://127.0.0.1:9", "http://127.0.0.1:9/x.json", "http://127.0.0.1/x", "http://127.0.0.1:9?k=v"] {
        assert!(github::loopback(ok), "{ok}");
    }
}

// ---------- 修正ラウンド 2（確定した指摘ごとに 1 本以上。指摘の id を名に添える） ----------
/// AD-SEC-R2-3: web_read_at の失敗文に URL（query の鍵）を入れない。ureq の Display は URL を前に付けるので使わない
/// （旧来の github::web_read は f.message() を返し、app.rs の WebRead が run の結果として画面と履歴に出す）
#[test]
fn ad_sec_r2_3_web_read_failure_text_never_carries_the_query() {
    let (base, _) = fake(vec![("GET", "/v1/data", 401, json!({"message": "bad key"}))]);
    for (url, want) in [(format!("{base}/v1/data?api_key=SECRET"), "401"), (format!("{}/v1/data?api_key=SECRET", closed_port()), "can't connect")] {
        let f = github::web_read_at(&url, None).expect_err("失敗するはず");
        assert!(!f.message().contains("SECRET") && f.message().contains(want) && f.message().contains("127.0.0.1:"), "{}", f.message());
    }
}

/// AD-R2-3: 1 つの対象への権限不足（403）で接続全体を Revoked にしない（token の要らない読み取りまで止めない）。
/// token 自体の拒否（401）かは registry が auth の確認（check）をやり直して決める
#[test]
fn ad_r2_3_a_403_on_one_target_does_not_revoke_the_connection() {
    // GitHub: token はあるが o/r への issues:write が無い（fine-grained PAT・SSO 未承認）。/rate_limit はどの有効な token でも 200
    let (base, _) = fake(vec![("GET", "/rate_limit", 200, json!({})), ("POST", "/repos/o/r/issues", 403, json!({"message": "Resource not accessible by personal access token"})),
        ("GET", "/repos/o/r/issues", 200, json!([{"number": 1, "title": "t", "html_url": "u"}]))]);
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::github::GitHub::new(api(&base)))).unwrap();
    r.check();
    let input = json!({"repo": "o/r", "title": "t"});
    assert_eq!(r.invoke(APP, "github.issues.create", &input, Some(&consent("github.issues.create", &["github.issues.write"], &input))).0, Outcome::Failed { error: ErrorCode::PermissionDenied });
    assert_eq!(r.tier("github.issues.list"), DisplayTier::Ready, "1 つの repo への 403 で接続全体を止めた");
    assert!(matches!(r.invoke(APP, "github.issues.list", &json!({"repo": "o/r"}), None).0, Outcome::Verified { .. }));
    // Google: generic な 403（アクセスを拒否しました）で Gmail / Calendar の読み取りを止めない
    let (gbase, _) = fake(vec![("GET", "/gmail/v1/users/me/profile", 200, json!({})), ("POST", "/calendar/v3/calendars/primary/events", 403, json!({"error": {"code": 403, "message": "Forbidden"}})),
        ("GET", "/calendar/v3/calendars/primary/events", 200, json!({"items": []}))]);
    let mut g = AdapterRegistry::default();
    g.register(Box::new(adapters::google::Google::new(Some(client(&gbase)), ALL_GOOGLE))).unwrap();
    g.check();
    let event = json!({"summary": "s", "start": "2026-10-20", "end": "2026-10-21"});
    assert_eq!(g.invoke(APP, "calendar.events.create", &event, Some(&consent("calendar.events.create", &["calendar.write"], &event))).0, Outcome::Failed { error: ErrorCode::PermissionDenied });
    for id in ["calendar.events.read", "gmail.threads.read"] { assert_eq!(g.tier(id), DisplayTier::Ready, "{id}: generic な 403 で接続全体を止めた") }
    assert!(matches!(g.invoke(APP, "calendar.events.read", &json!({}), None).0, Outcome::Verified { .. }));
}

/// AD-SEC-R2-1: 信頼済 CLI は「登録した（承認 card が見せる）実行ファイルと引数」でしか走らない（登録時の指紋 cli_pin・ADR-002 硬化 (b)）。
/// 承認の後に store の feature 10 の引数・実行ファイル・symlink の先が書き換わったら、前の承認（action・app・scope・digest・未使用は全部合う）では走らない
#[cfg(unix)]
#[test]
fn ad_sec_r2_1_trusted_cli_approval_binds_the_shown_path_and_args() {
    let d = tmp("tcli3");
    let marker = d.join("ran");
    let cli = script(&d, "backup.sh", &format!("echo \"backup $*\" > '{}'", marker.display()));
    let other = script(&d, "other.sh", &format!("echo other > '{}'", marker.display()));
    let link = d.join("backup-link");
    std::os::unix::fs::symlink(&cli, &link).unwrap();
    let shown = pinned(model::RepoFeature { id: 10, cli_path: link.display().to_string(), cli_args: vec!["--dry-run".into()], ..feature(&cli, "") });
    let registry = |f: &model::RepoFeature| {
        let mut r = AdapterRegistry::default();
        r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![f.clone()]))).unwrap();
        r.check();
        r
    };
    let input = json!({"feature_id": 10});
    // 人が card で見た時の registry が宣言した scope で承認を作る
    let scopes = registry(&shown).actions()[0].required_scopes.clone();
    let ok = consent("cli.trusted.run", &scopes.iter().map(String::as_str).collect::<Vec<_>>(), &input);
    for swapped in [model::RepoFeature { cli_args: vec!["--delete-all".into()], ..shown.clone() }, model::RepoFeature { cli_path: other.display().to_string(), ..shown.clone() }] {
        let o = registry(&swapped).invoke(APP, "cli.trusted.run", &input, Some(&ok)).0;
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Register the connection again")), "{swapped:?}: {o:?}");
    }
    // store の文字列は同じでも、symlink の先が別の実行ファイルに替わった
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&other, &link).unwrap();
    assert!(matches!(registry(&shown).invoke(APP, "cli.trusted.run", &input, Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("Register the connection again")));
    assert!(!marker.exists(), "見せていない CLI が走った");
    // 見せた物のままなら走る
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&cli, &link).unwrap();
    assert!(matches!(registry(&shown).invoke(APP, "cli.trusted.run", &input, Some(&ok)).0, Outcome::Unverified { .. }));
    assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "backup --dry-run");
}

/// AD-R2-2: 子が background の孫を起こしてすぐ終わる（`nohup daemon &`）と、孫が stdout / stderr を継いで握り EOF が来ない。
/// それでも invoke は期限（CLI 25 秒・cua-driver 16 秒）で返り、子が書いた分は残る。試験は別 thread で走らせ、返らなければ赤（固まらない）
#[cfg(unix)]
fn returns_within<T: Send + 'static>(limit: Duration, pid_file: &Path, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || { let _ = tx.send(f()); });
    let got = rx.recv_timeout(limit).ok();
    // 孫（sleep）を止める（赤の時は、これで固まっていた thread も抜ける）
    if let Ok(pid) = fs::read_to_string(pid_file) { let _ = std::process::Command::new("kill").arg(pid.trim()).status(); }
    got
}

#[cfg(unix)]
#[test]
fn ad_r2_2_cli_whose_grandchild_holds_the_pipe_still_returns() {
    let d = tmp("cli-bg");
    let pid = d.join("pid");
    let cli = script(&d, "daemonize.sh", &format!("sleep 120 &\necho $! > '{}'\necho started\nexit 0", pid.display()));
    let f = feature(&cli, "");
    let o = returns_within(Duration::from_secs(60), &pid, move || repos::run_trusted_feature_outcome(&f)).expect("孫が pipe を握ると返らなかった");
    assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("started")), "{o:?}");
}

/// AD-R2-1（CD-53・RUN-004）: Google の作成は POST の id では Verified にしない。client が決めた印（event id・X-Kangae-Key）で読み戻し、一致した時だけ
#[test]
fn ad_r2_1_google_writes_are_verified_only_by_the_read_back() {
    // POST は id を返すが、読み戻し（events.get / drafts.get）は 404 → 受理だけ = Unverified
    let (base, log) = fake(vec![("GET", "/gmail/v1/users/me/drafts/", 404, json!({})), ("GET", "/gmail/v1/users/me/drafts", 200, json!({"drafts": []})),
        ("POST", "/gmail/v1/users/me/drafts", 200, json!({"id": "x"})), ("GET", "/calendar/v3/calendars/primary/events/", 404, json!({})),
        ("POST", "/calendar/v3/calendars/primary/events", 200, json!({"id": "x", "htmlLink": "https://calendar.google.com/x"}))]);
    let mut g = adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE);
    let draft = json!({"to": "b@x.io", "subject": "s", "body": "b"});
    let event = json!({"summary": "s", "start": "2026-10-20", "end": "2026-10-21"});
    for (action, input) in [("gmail.drafts.create", &draft), ("calendar.events.create", &event)] {
        let o = g.invoke(action, input).0;
        assert!(matches!(&o, Outcome::Unverified { .. }), "{action}: POST の id で Verified にした: {o:?}");
    }
    // 印は承認した入力の digest から決まる（送り直しても同じ）。予定は id、下書きは X-Kangae-Key header
    let log = log.lock().unwrap();
    let event_id = format!("{}{}", google::calendar::ID_PREFIX, sdk::payload_digest("calendar.events.create", &event));
    let posted: Value = serde_json::from_str(&log.iter().find(|q| q.method == "POST" && q.path.starts_with("/calendar")).unwrap().body).unwrap();
    assert_eq!(posted["id"], json!(event_id));
    assert!(log.iter().any(|q| q.method == "GET" && q.path == format!("/calendar/v3/calendars/primary/events/{event_id}")), "読み戻しは client の id で引く");
    let sent: Value = serde_json::from_str(&log.iter().find(|q| q.method == "POST" && q.path.starts_with("/gmail")).unwrap().body).unwrap();
    let raw = String::from_utf8(crypto::base64_decode(sent["message"]["raw"].as_str().unwrap()).unwrap()).unwrap();
    assert!(raw.starts_with(&format!("X-Kangae-Key: {}\r\n", sdk::payload_digest("gmail.drafts.create", &draft))), "{raw}");
}

/// 状態を持つ偽 Google（予定と下書き）。それぞれ POST の 1 本目は request を読んで保存した後に応答せず切る（届いた後の時間切れ）。
/// 予定は同じ id の 2 本目に 409（Google の events.insert と同じ）、下書きの一覧は新しい順
fn fake_google_store() -> (String, Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) {
    let (events, drafts) = (Arc::new(Mutex::new(Vec::<Value>::new())), Arc::new(Mutex::new(Vec::<Value>::new())));
    let (ev, dr) = (Arc::clone(&events), Arc::clone(&drafts));
    const EV: &str = "/calendar/v3/calendars/primary/events";
    const DR: &str = "/gmail/v1/users/me/drafts";
    let (base, _) = fake_with(move |q| {
        let path = q.path.split('?').next().unwrap_or("");
        let found = |store: &Mutex<Vec<Value>>, id: &str| store.lock().unwrap().iter().find(|x| x["id"] == id).map_or((404, "{}".to_string()), |x| (200, x.to_string()));
        match (q.method.as_str(), path) {
            ("GET", "/gmail/v1/users/me/profile") => (200, "{}".into()),
            ("GET", EV) => (200, json!({"items": []}).to_string()),
            ("POST", EV) => {
                let mut e: Value = serde_json::from_str(&q.body).unwrap_or_default();
                let mut ev = ev.lock().unwrap();
                if e["id"].is_string() && ev.iter().any(|x| x["id"] == e["id"]) { return (409, json!({"error": {"code": 409, "message": "The requested identifier already exists."}}).to_string()) }
                if !e["id"].is_string() { e["id"] = json!(format!("e{}", ev.len() + 1)) }
                ev.push(e.clone());
                (if ev.len() == 1 { 0 } else { 200 }, e.to_string())
            }
            ("GET", p) if p.starts_with(&format!("{EV}/")) => found(&ev, &p[EV.len() + 1..]),
            ("POST", DR) => {
                let raw = serde_json::from_str::<Value>(&q.body).ok().and_then(|v| v["message"]["raw"].as_str().and_then(crypto::base64_decode)).unwrap_or_default();
                let raw = String::from_utf8_lossy(&raw).to_string();
                let (head, text) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
                let headers: Vec<Value> = head.split("\r\n").filter_map(|l| l.split_once(": ")).map(|(k, v)| json!({"name": k, "value": v})).collect();
                let mut dr = dr.lock().unwrap();
                let id = format!("d{}", dr.len() + 1);
                dr.push(json!({"id": id, "message": {"id": format!("m-{id}"), "payload": {"mimeType": "text/plain", "headers": headers, "body": {"data": crypto::base64(text.as_bytes(), true)}}}}));
                (if dr.len() == 1 { 0 } else { 200 }, json!({"id": id}).to_string())
            }
            ("GET", DR) => (200, json!({"drafts": dr.lock().unwrap().iter().rev().map(|d| json!({"id": d["id"]})).collect::<Vec<_>>()}).to_string()),
            ("GET", p) if p.starts_with(&format!("{DR}/")) => found(&dr, &p[DR.len() + 1..]),
            _ => (404, "{}".into()),
        }
    });
    (base, events, drafts)
}

/// AD-R2-1: 送れたか分からない（POST が届いた後に切れた = EffectUnknown）後に、人が同じ内容を承認し直して打つ（receipt の id は新しいので
/// used の照合では止まらない）。印が同じなので、予定は 409 → 読み戻し、下書きは key で先に見つけ、2 つ目を作らない
#[test]
fn ad_r2_1_a_reapproved_retry_after_effect_unknown_never_creates_a_second_one() {
    let (base, events, drafts) = fake_google_store();
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE))).unwrap();
    for (action, scope, input, store) in [("gmail.drafts.create", "mail.compose", json!({"to": "b@x.io", "subject": "s", "body": "b"}), &drafts),
                                          ("calendar.events.create", "calendar.write", json!({"summary": "s", "start": "2026-10-20", "end": "2026-10-21"}), &events)] {
        r.check();
        let first = r.invoke(APP, action, &input, Some(&consent(action, &[scope], &input))).0;
        assert!(matches!(&first, Outcome::EffectUnknown { .. }), "{action}: {first:?}");
        r.check(); // 次の Run の前の確認（EffectUnknown で degraded になった接続を Ready に戻す）
        let again = r.invoke(APP, action, &input, Some(&consent(action, &[scope], &input))).0;
        assert!(matches!(&again, Outcome::Verified { evidence } if evidence.iter().any(|e| e.contains("既に在りました") || e.contains("already existed"))), "{action}: {again:?}");
        assert_eq!(store.lock().unwrap().len(), 1, "{action}: 2 つ作った");
    }
}

// ---------- review round 3 の指摘（AD-R3-1〜5）と統合 2 の merge 条件 3 / 4: 先に赤くなるのを見てから直した ----------

/// AD-R3-1 / AD-R3-2（CD-01:289 / 290・ADR-002:46）: desktop の click / type_text と信頼済 CLI は external_write。local_demo の app に
/// ready で入れると domain の V4（PhantomReady）が落とす（demo の app から他 app への本物の操作や任意の実行ファイルを押せる形にしない）。
#[cfg(unix)]
#[test]
fn ad_r3_1_2_desktop_input_and_trusted_cli_cannot_be_ready_in_a_demo_app() {
    // desktop の口は MacGui（menu・key を含む・bridge は起こさない・YC W18-1 で旧い desktop の adapter を消した）
    let mut actions = adapters::mac_gui::MacGui::new().actions();
    actions.extend(adapters::trusted_cli::TrustedCli::new(vec![]).actions());
    let caps: Vec<Value> = actions.iter().filter_map(|a| a.capability_ref.clone()).map(|id| json!({"id": id, "readiness": "ready"})).collect();
    for id in ["desktop.ax.click", "desktop.ax.type_text", "desktop.ax.menu", "desktop.key", "cli.trusted.run"] {
        let mut a = actions.iter().find(|a| a.id == id).unwrap().clone();
        assert_eq!(a.effect, Effect::ExternalWrite, "{id}");
        a.readiness = domain::action::ActionReadiness::Ready;
        let mut app: Value = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples/v3/01_timer_local.json")).unwrap()).unwrap();
        app["state"]["execution_mode"] = json!("local_demo");
        app["capabilities"] = json!(caps);
        app["actions"].as_array_mut().unwrap().push(json!(a));
        let def: domain::app::AppDefinitionV3 = serde_json::from_value(app).unwrap();
        let errs = domain::validate::validate(&def).expect_err(id);
        assert!(errs.iter().any(|e| matches!(e, domain::validate::ValidationError::PhantomReady { action } if action == id)), "{id}: {errs:?}");
    }
}

/// AD-R3-3: 入力の key が空（""）でも、X-Kangae-Key の無い利用者自身の下書きを「同じ key の下書き」と取り違えない（空の key は無い物として
/// 承認した入力の digest を印にする）。利用者の下書きに当たって毎回 InvalidInput になったり、作っていない物を Verified と見せたりしない。
#[test]
fn ad_r3_3_an_empty_draft_key_never_matches_the_users_own_draft() {
    let (base, _, drafts) = fake_google_store();
    drafts.lock().unwrap().push(json!({"id": "mine", "message": {"id": "m-mine", "payload": {"mimeType": "text/plain",
        "headers": [{"name": "To", "value": "friend@x.io"}, {"name": "Subject", "value": "手書き"}], "body": {"data": crypto::base64("手で書いた下書き".as_bytes(), true)}}}}));
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::google::Google::new(Some(client(&base)), ALL_GOOGLE))).unwrap();
    r.check();
    let input = json!({"to": "b@x.io", "subject": "s", "body": "b", "key": ""});
    let o = r.invoke(APP, "gmail.drafts.create", &input, Some(&consent("gmail.drafts.create", &["mail.compose"], &input))).0;
    assert!(!matches!(&o, Outcome::Failed { .. } | Outcome::Rejected { .. }), "空の key が利用者の下書きに当たった: {o:?}");
    assert!(!matches!(&o, Outcome::Verified { evidence } if evidence.iter().any(|e| e.contains("既に在りました"))), "作っていない下書きを Verified にした: {o:?}");
    let drafts = drafts.lock().unwrap();
    assert_eq!(drafts.len(), 2, "新しい下書きを作っていない");
    let key = drafts[1]["message"]["payload"]["headers"].as_array().unwrap().iter().find(|h| h["name"] == "X-Kangae-Key").map(|h| h["value"].clone());
    assert!(matches!(&key, Some(Value::String(k)) if !k.is_empty()), "空の印で作った（後で Kangae から消せない）: {key:?}");
}

/// AD-R3-4: 有効な CLI の 1 本の実行ファイルが消えても、他の正常な CLI は止めない（接続全体を RequiresConnection にしない）。
/// 消えた方は走らせない（登録した物が無い = 拒否）。
#[cfg(unix)]
#[test]
fn ad_r3_4_one_missing_cli_does_not_stop_the_others() {
    let d = tmp("tcli4");
    let ran = d.join("ran-a");
    let a = script(&d, "a.sh", &format!("touch '{}'", ran.display()));
    let b = script(&d, "b.sh", "exit 0");
    let (fa, fb) = (model::RepoFeature { id: 10, ..feature(&a, "") }, model::RepoFeature { id: 11, ..feature(&b, "") });
    fs::remove_file(&b).unwrap();
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![fa, fb]))).unwrap();
    r.check();
    assert_eq!(r.tier("cli.trusted.run"), DisplayTier::Ready, "1 本の欠けで接続全体を止めた");
    let run = |id: u64| json!({"feature_id": id});
    let o = r.invoke(APP, "cli.trusted.run", &run(10), Some(&consent("cli.trusted.run", &["exec:10"], &run(10)))).0;
    assert!(matches!(&o, Outcome::Unverified { .. }), "{o:?}");
    assert!(ran.exists());
    let o = r.invoke(APP, "cli.trusted.run", &run(11), Some(&consent("cli.trusted.run", &["exec:11"], &run(11)))).0;
    assert!(matches!(&o, Outcome::Rejected { .. }), "消えた CLI を走らせようとした: {o:?}");
}

/// AD-R3-5（ADR-002 硬化 (b)）: 登録した時の実行ファイルの中身を固定し、実行の直前に読み直す。同じ path のまま中身が替わっていたら
/// （更新・差し替え）、前の承認では走らせない。
#[cfg(unix)]
#[test]
fn ad_r3_5_a_binary_replaced_in_place_after_registration_never_runs() {
    let d = tmp("tcli5");
    let (ok, swapped) = (d.join("ran-registered"), d.join("ran-swapped"));
    let cli = script(&d, "tool.sh", &format!("touch '{}'", ok.display()));
    let f = model::RepoFeature { id: 10, ..feature(&cli, "") };
    script(&d, "tool.sh", &format!("touch '{}'", swapped.display())); // 同じ path に別の中身
    let mut r = AdapterRegistry::default();
    r.register(Box::new(adapters::trusted_cli::TrustedCli::new(vec![f]))).unwrap();
    r.check();
    let input = json!({"feature_id": 10});
    let o = r.invoke(APP, "cli.trusted.run", &input, Some(&consent("cli.trusted.run", &["exec:10"], &input))).0;
    assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Register the connection again")), "{o:?}");
    assert!(!swapped.exists() && !ok.exists(), "登録した時と中身の違う実行ファイルが走った");
}

/// runtime の handler（呼ばれた数を数える）。
struct Counting(Arc<AtomicUsize>);
impl runtime::registry::Handler for Counting {
    fn execute(&self, _: &runtime::registry::Ctx, _: &Value) -> Result<Value, ErrorCode> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({}))
    }
}

/// 統合 2 の merge 条件 3（CD-01・RT-SEC-R2-1）: runtime の gate が見る scope は、adapter の Spec（registry の契約）から運ぶ。
/// app の文書が gmail.threads.read の scope を calendar.read と書き、利用者が calendar.read だけを許しても gmail は読まない（PermissionDenied）。
#[test]
fn bridge_carries_adapter_scopes_into_the_runtime_gate() {
    use domain::{action::InvocationOrigin, patch::AppRevision, permission::PermissionGrant};
    use runtime::Invoked;
    const GMAIL: &[Spec] = &[Spec { id: "gmail.threads.read", effect: Effect::ExternalRead, approval: false, scopes: &["mail.read"], input: r#"{"type":"object"}"#, verifier: "read_back" }];
    let mut ad = AdapterRegistry::default();
    ad.register(Box::new(Fake { id: "gmail", specs: GMAIL })).unwrap();
    let contract = ad.contract("gmail.threads.read").expect("登録した action の契約");
    assert_eq!((contract.adapter, contract.scopes, contract.effect), ("gmail", &["mail.read"][..], Effect::ExternalRead));
    assert!(ad.contract("gmail.unknown.read").is_none());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = runtime::registry::ActionRegistry::default();
    reg.register("gmail.threads.read", contract, Counting(calls.clone()));
    let rt = runtime::Runtime::start(Arc::new(Mutex::new(storage::Store::open_in_memory().unwrap())), reg).unwrap();
    ad.sync_readiness(rt.registry());
    let app = |id: &str, scope: &str| {
        AppRevision::new(serde_json::from_value(json!({
            "schema_version": "3.0", "id": id, "version": 1, "title": "bridge",
            "goal": { "literal_goal": "メールを読む", "outcomes": ["読める"] },
            "composition": { "root": { "key": "root", "component": "column" } },
            "capabilities": [ { "id": "gmail.api", "provider": "google", "readiness": "ready", "adapter": "api" } ],
            "actions": [ { "id": "gmail.threads.read", "capability_ref": "gmail.api", "effect": "external_read", "readiness": "ready",
                           "input_schema": { "type": "object" }, "required_scopes": [scope] } ],
            "sources": [],
            "agent": { "suggestions_enabled": true, "autonomy_level": "ask_before_external_effects", "entrypoint": "agent_rail" },
            "state": { "readiness": "ready", "execution_mode": "integrated" }
        })).unwrap()).unwrap()
    };
    let grant = |app: &str, scope: &str| PermissionGrant { id: format!("g-{app}-{scope}"), app_id: app.into(), scope: scope.into(),
        granted_at: "2026-10-01T00:00:00.000Z".into(), expires_at: None, consent_ref: None };
    let button = || InvocationOrigin::Button { node_key: "read".into() };
    let disguised = rt.open(app("test.disguised", "calendar.read"), "s-1").unwrap();
    disguised.grant(grant("test.disguised", "calendar.read")).unwrap();
    assert_eq!(disguised.invoke("gmail.threads.read", json!({}), button()), Err(ErrorCode::PermissionDenied));
    assert_eq!(disguised.grant(grant("test.disguised", "mail.read")), Err(ErrorCode::PermissionDenied), "文書が宣言していない契約の scope を grant できた");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "契約の scope の grant が無いのに gmail を読んだ");
    // 裏: 文書が契約の scope を宣言し、その grant があれば読める
    let honest = rt.open(app("test.honest", "mail.read"), "s-2").unwrap();
    honest.grant(grant("test.honest", "mail.read")).unwrap();
    assert!(matches!(honest.invoke("gmail.threads.read", json!({}), button()), Ok(Invoked::Done(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ---------- git（W12a: Project の repo を読む Adapter。本物の git で temp の repo を読む・network を使わない・owner の設定を読まない） ----------
use adapters::git::{self as gitad, Git, RepoStatus};
use domain::project::{ProjectResource, ResourceReader};
use std::process::Command;

/// 試験の側の git（product とは別の道・owner の hook・署名・template を読まない）。
fn sh_git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "qa").env("GIT_AUTHOR_EMAIL", "qa@localhost").env("GIT_COMMITTER_NAME", "qa").env("GIT_COMMITTER_EMAIL", "qa@localhost")
        .output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}
fn repo_with_commit(tag: &str) -> PathBuf {
    let d = tmp(tag);
    sh_git(&d, &["init", "-q", "-b", "main"]);
    fs::write(d.join("README.md"), "hello\n").unwrap();
    sh_git(&d, &["add", "README.md"]);
    sh_git(&d, &["commit", "-q", "-m", "init"]);
    d
}
fn repo_resource(dir: &Path) -> ProjectResource { ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some(format!("file://{}", dir.display())) } }
/// registry の ResourceReader（Core が使う唯一の道）で読む。読めた時の Observation は git の source で、payload は RepoStatus の形。
fn read_repo(r: &mut AdapterRegistry, res: &ProjectResource) -> (Outcome, Option<RepoStatus>) {
    let (o, obs) = r.read(res);
    let status = obs.map(|obs| {
        assert_eq!(obs.source_ref, gitad::SOURCE);
        serde_json::from_value(obs.payload).expect("payload は RepoStatus の形（deny_unknown_fields）")
    });
    (o, status)
}
fn changes(s: &RepoStatus) -> Vec<(String, String, Option<String>)> {
    s.changes.iter().map(|c| (c.status.clone(), c.path.clone(), c.orig_path.clone())).collect()
}

/// W12a の Adapter の芯: 試験の側の git（rev-parse HEAD・branch --show-current・status --porcelain）と同じ値。README を変えると変更 file に出る（正の対照）。
/// 空白・日本語の path は引用符も escape も無い生の path・rename は元の path を持つ。
/// 読みは index を書かない（index.lock を取らない）: 中身が同じで stat だけ古い file を読んでも `.git/index` の bytes が変わらない
/// （`--no-optional-locks` が無い git の status は stat を直して index を書き、lock を外してから終わるので、lock の有無では測れない）。
#[test]
fn git_reads_head_branch_and_changes_like_the_test_side_git() {
    let d = repo_with_commit("git-read");
    let res = repo_resource(&d);
    let mut r = AdapterRegistry::with_local_adapters();
    let (o, s) = read_repo(&mut r, &res);
    assert!(matches!(o, Outcome::Verified { .. }), "{o:?}");
    let s = s.unwrap();
    assert_eq!(s.head.sha.as_deref(), Some(sh_git(&d, &["rev-parse", "HEAD"]).as_str()), "40 桁の SHA がそのまま");
    assert_eq!(s.head.sha.as_ref().map(String::len), Some(40));
    assert_eq!(s.head.branch.as_deref(), Some("main"));
    assert!(s.changes.is_empty(), "clean: {:?}", s.changes);
    let index = d.join(".git/index");
    let before = fs::read(&index).unwrap();
    // 中身は同じ・mtime だけ 2001 年に（index の stat が古くなる = lock を取る git なら index を書き直す）
    fs::File::options().write(true).open(d.join("README.md")).unwrap().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(978_307_200)).unwrap();
    let (_, s) = read_repo(&mut r, &res);
    assert!(s.unwrap().changes.is_empty(), "中身が同じなら変更ではない");
    assert!(fs::read(&index).unwrap() == before, "読みが .git/index を書き直した（index.lock を取った）");
    fs::write(d.join("README.md"), "hello\nnonce-w12a\n").unwrap();
    fs::write(d.join("new file 日本.txt"), "x").unwrap();
    let (_, s) = read_repo(&mut r, &res);
    assert_eq!(changes(&s.unwrap()), [(" M".into(), "README.md".into(), None), ("??".into(), "new file 日本.txt".into(), None)]);
    assert!(!index.with_file_name("index.lock").exists());
    sh_git(&d, &["add", "-A"]);
    sh_git(&d, &["commit", "-q", "-m", "two"]);
    sh_git(&d, &["mv", "README.md", "DOC.md"]);
    let (_, s) = read_repo(&mut r, &res);
    let s = s.unwrap();
    assert_eq!(changes(&s), [("R ".into(), "DOC.md".into(), Some("README.md".into()))], "-z の rename は「新 元」の順");
    assert_eq!(s.head.sha.as_deref(), Some(sh_git(&d, &["rev-parse", "HEAD"]).as_str()), "commit の後の HEAD");
    let _ = fs::remove_dir_all(&d);
}

/// unborn（commit 0）の SHA と detached HEAD の branch は null（空文字にしない・偽の値を作らない）。
#[test]
fn git_unborn_sha_and_detached_branch_are_null() {
    let d = tmp("git-unborn");
    sh_git(&d, &["init", "-q", "-b", "main"]);
    let mut r = AdapterRegistry::with_local_adapters();
    let (o, s) = read_repo(&mut r, &repo_resource(&d));
    assert!(matches!(o, Outcome::Verified { .. }), "{o:?}");
    let s = s.unwrap();
    assert_eq!((s.head.sha, s.head.branch.as_deref()), (None, Some("main")));
    let d2 = repo_with_commit("git-detached");
    sh_git(&d2, &["checkout", "-q", "--detach"]);
    let (_, s) = read_repo(&mut r, &repo_resource(&d2));
    let s = s.unwrap();
    assert_eq!(s.head.branch, None);
    assert_eq!(s.head.sha.as_deref(), Some(sh_git(&d2, &["rev-parse", "HEAD"]).as_str()));
    for x in [d, d2] {
        let _ = fs::remove_dir_all(x);
    }
}

/// 読めない場所は読まずに誤りの種類で返し、Observation を作らない: repo でない dir・無い dir = TargetNotFound・file:// の絶対 path でない = InvalidInput。
/// uri の無い resource・読む Adapter の無い kind は Rejected（画面に出せる 1 行・action id を入れない）。
#[test]
fn git_refuses_places_it_cannot_read_without_an_observation() {
    let plain = tmp("git-plain");
    let mut r = AdapterRegistry::with_local_adapters();
    let at = |uri: &str| ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some(uri.into()) };
    for (res, want) in [
        (repo_resource(&plain), Outcome::Failed { error: ErrorCode::TargetNotFound }),
        (repo_resource(&plain.join("gone")), Outcome::Failed { error: ErrorCode::TargetNotFound }),
        (at("https://github.com/o/r"), Outcome::Failed { error: ErrorCode::InvalidInput }),
        (at("file://relative/dir"), Outcome::Failed { error: ErrorCode::InvalidInput }),
    ] {
        let (o, obs) = r.read(&res);
        assert_eq!((o, obs.is_none()), (want, true), "{:?}", res.uri);
    }
    for res in [ProjectResource { id: "repo".into(), kind: "repo".into(), uri: None }, ProjectResource { id: "d".into(), kind: "drive".into(), uri: Some("file:///x".into()) }] {
        let (o, obs) = r.read(&res);
        let Outcome::Rejected { reason } = o else { panic!("{o:?}") };
        assert!(obs.is_none() && !reason.contains(gitad::STATUS_READ) && !reason.contains("git"), "{reason}");
    }
    let _ = fs::remove_dir_all(&plain);
}

/// git が無い Mac: check の後も Ready にならず、Adapter が言う「何を入れれば読めるか」の 1 行で Rejected（D-e・Core は git を名指ししない）・
/// Observation 無し・readiness は RequiresConnection（「接続済み」と言わない）。
#[test]
fn git_missing_binary_is_not_ready_and_reads_nothing() {
    let d = repo_with_commit("git-missing");
    let mut r = AdapterRegistry::default();
    r.register(Box::new(Git::at(PathBuf::from("/nonexistent/bin/git")))).unwrap();
    let (o, obs) = r.read(&repo_resource(&d));
    assert!(obs.is_none());
    let Outcome::Rejected { reason } = o else { panic!("入れる物を言わない: {o:?}") };
    // UI-2 反証の直し: 押す button の名前は画面の今の名前（`Refresh`・前の `↻ 読み直す` はもう無い）
    assert!(reason.contains("Command Line Tools") && reason.contains("git") && reason.contains("press Refresh") && !reason.contains("読み直す") && !reason.contains(gitad::STATUS_READ), "{reason}");
    assert_eq!(r.capabilities()[0].readiness, CapabilityReadiness::RequiresConnection);
    assert_eq!(r.sources()[0].readiness, SourceReadiness::RequiresConnection);
    let _ = fs::remove_dir_all(&d);
}

/// G81: `git.file.read` は repo の work tree の中の file 1 つの中身（Engine が Page の書類の保存を disk で確かめる読み）。git の実行 file を使わない =
/// git の無い Mac でも Ready で読める（repo の読みは RequiresConnection のまま）。無い file は exists = false（保存の途中の置き換えで止めない）・
/// repo の外（`.git` の無い所・symlink で外を指す）は TargetNotFound・`..`・相対・file: でない uri は InvalidInput・1MB を超えたら頭だけで complete = false。
#[cfg(unix)]
#[test]
fn git_file_read_reads_one_file_in_a_repo_and_nothing_outside() {
    let (repo, outside) = (tmp("git-file"), tmp("git-file-out"));
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("docs")).unwrap();
    fs::write(repo.join("README.md"), "# Shibubu Demo\n日本語\n").unwrap();
    fs::write(repo.join("docs/notes.md"), "notes").unwrap();
    fs::write(outside.join("id_key"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.join("id_key"), repo.join("link.md")).unwrap();
    fs::write(repo.join("big.md"), "a".repeat((1 << 20) + 10)).unwrap();
    let mut r = AdapterRegistry::default();
    r.register(Box::new(Git::at(PathBuf::from("/nonexistent/bin/git")))).unwrap();
    r.check();
    let uri = |p: &Path| json!({ "uri": format!("file://{}", p.display()) });
    let read = |r: &mut AdapterRegistry, input: Value| r.invoke(APP, gitad::FILE_READ, &input, None);
    for (p, text) in [(repo.join("README.md"), "# Shibubu Demo\n日本語\n"), (repo.join("docs/notes.md"), "notes")] {
        let (o, obs) = read(&mut r, uri(&p));
        assert!(matches!(o, Outcome::Verified { .. }), "{o:?}");
        let obs = obs.unwrap();
        assert_eq!((obs.source_ref.as_str(), obs.payload), (gitad::FILE_SOURCE, json!({ "exists": true, "text": text, "complete": true })));
    }
    let (_, obs) = read(&mut r, uri(&repo.join("gone.md")));
    assert_eq!(obs.unwrap().payload, json!({ "exists": false, "text": "", "complete": true }), "無い file は読めないと止めない");
    // G81: Page の書類（docs/README.md）の folder もまだ無い = 同じく無い file（Engine の手の前の読みで止めない）・repo の外の無い folder は読まない
    let (_, obs) = read(&mut r, uri(&repo.join("drafts/new/README.md")));
    assert_eq!(obs.map(|o| o.payload), Some(json!({ "exists": false, "text": "", "complete": true })), "無い folder の下も無い file");
    assert_eq!(read(&mut r, uri(&outside.join("drafts/README.md"))).0, Outcome::Failed { error: ErrorCode::TargetNotFound });
    let (_, obs) = read(&mut r, uri(&repo.join("big.md")));
    let p = obs.unwrap().payload;
    assert_eq!((p["text"].as_str().map(str::len), &p["complete"]), (Some(1 << 20), &json!(false)));
    for (input, want) in [
        (uri(&outside.join("id_key")), ErrorCode::TargetNotFound),
        (uri(&repo.join("link.md")), ErrorCode::TargetNotFound),
        (uri(&repo.join("docs/../README.md")), ErrorCode::InvalidInput),
        (json!({ "uri": "file://README.md" }), ErrorCode::InvalidInput),
        (json!({ "uri": format!("https://example.invalid{}", repo.join("README.md").display()) }), ErrorCode::InvalidInput),
    ] {
        let (o, obs) = read(&mut r, input.clone());
        assert_eq!((o, obs.is_none()), (Outcome::Failed { error: want }, true), "{input}");
    }
    // repo の読みは git が無いので今まで通り読まない
    assert!(matches!(r.read(&repo_resource(&repo)).0, Outcome::Rejected { .. }));
    for x in [repo, outside] {
        let _ = fs::remove_dir_all(x);
    }
}

/// 答えない git: 1 回の読みは 5 秒で打ち切る（Failed{Timeout}・UI の thread を止め続けない）。
#[cfg(unix)]
#[test]
fn git_that_hangs_times_out_in_five_seconds() {
    let bin = tmp("git-slow");
    let slow = script(&bin, "git", r#"case "$*" in *--version*) echo "git version 9.9";; *) exec sleep 30;; esac"#);
    let d = repo_with_commit("git-slow-repo");
    let mut r = AdapterRegistry::default();
    r.register(Box::new(Git::at(slow))).unwrap();
    let t = Instant::now();
    let (o, obs) = r.read(&repo_resource(&d));
    assert_eq!((o, obs.is_none()), (Outcome::Failed { error: ErrorCode::Timeout }, true));
    let took = t.elapsed();
    assert!(took >= Duration::from_secs(4) && took < Duration::from_secs(9), "{took:?}");
    for x in [bin, d] {
        let _ = fs::remove_dir_all(x);
    }
}

/// 契約: git の Adapter は読むだけ（LocalRead・承認無し・OS automation）で、registry は `reads` の kind で引く（Core は git を名指ししない）。
/// 同じ kind を読む別の Adapter に替えれば、Core の code を変えずにそれが読む（§1 の交換）。承認の要る action を `reads` に載せた Adapter は呼ばない。
#[test]
fn registry_reads_by_kind_swaps_the_reader_and_never_calls_an_approval_action() {
    let r = AdapterRegistry::with_local_adapters();
    let a = r.actions();
    assert_eq!(a.iter().map(|a| (a.id.as_str(), a.effect, a.requires_approval)).collect::<Vec<_>>(), [(gitad::STATUS_READ, Effect::LocalRead, false), (gitad::FILE_READ, Effect::LocalRead, false)]);
    assert_eq!(r.sources().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [gitad::SOURCE]);
    assert_eq!(Git::new().reads(), &[("repo", gitad::STATUS_READ)]);
    // 替えの Adapter（試験の中で定義）: 同じ kind "repo" を読む
    struct OtherRepo;
    const OTHER: &[Spec] = &[Spec { id: "other.repo.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
    impl Adapter for OtherRepo {
        fn id(&self) -> &'static str { "other" }
        fn kind(&self) -> AdapterKind { AdapterKind::Repo }
        fn route(&self) -> Route { Route::OfficialApi }
        fn provider(&self) -> &'static str { "other" }
        fn specs(&self) -> &'static [Spec] { OTHER }
        fn reads(&self) -> &'static [(&'static str, &'static str)] { &[("repo", "other.repo.read")] }
        fn readiness(&self, _: &str) -> CapabilityReadiness { CapabilityReadiness::Ready }
        fn sources(&self) -> Vec<domain::binding::Source> { vec![] }
        fn check(&mut self) {}
        fn invoke(&mut self, _: &str, input: &Value) -> (Outcome, Option<Observation>) { sdk::read_result(Ok(json!({ "from": "other", "input": input })), "other.repo", "other", 0) }
        fn cleanup(&mut self) {}
    }
    let mut swapped = AdapterRegistry::default();
    swapped.register(Box::new(OtherRepo)).unwrap();
    let (o, obs) = swapped.read(&ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some("file:///x".into()) });
    assert!(matches!(o, Outcome::Verified { .. }));
    assert_eq!(obs.unwrap().payload, json!({ "from": "other", "input": { "uri": "file:///x" } }));
    // 承認の要る action を reads に載せた Adapter: 呼ばずに Rejected
    let calls = Arc::new(AtomicUsize::new(0));
    struct Writer(Arc<AtomicUsize>);
    const WRITE: &[Spec] = &[Spec { id: "writer.repo.write", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "human_review" }];
    impl Adapter for Writer {
        fn id(&self) -> &'static str { "writer" }
        fn kind(&self) -> AdapterKind { AdapterKind::Repo }
        fn route(&self) -> Route { Route::OfficialApi }
        fn provider(&self) -> &'static str { "writer" }
        fn specs(&self) -> &'static [Spec] { WRITE }
        fn reads(&self) -> &'static [(&'static str, &'static str)] { &[("repo", "writer.repo.write")] }
        fn readiness(&self, _: &str) -> CapabilityReadiness { CapabilityReadiness::Ready }
        fn sources(&self) -> Vec<domain::binding::Source> { vec![] }
        fn check(&mut self) {}
        fn invoke(&mut self, _: &str, _: &Value) -> (Outcome, Option<Observation>) { self.0.fetch_add(1, Ordering::SeqCst); (Outcome::Unverified { reason: "書いた".into() }, None) }
        fn cleanup(&mut self) {}
    }
    let mut w = AdapterRegistry::default();
    w.register(Box::new(Writer(calls.clone()))).unwrap();
    let (o, obs) = w.read(&ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some("file:///x".into()) });
    assert!(matches!(o, Outcome::Rejected { .. }) && obs.is_none(), "{o:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "承認の要る action を resource の読みで呼んだ");
}
// ---- W17-0: adapters::mac_gui の bridge に依らない純関数（本物の bridge・cua-driver は起こさない）----
mod mac_gui_pure {
    use super::adapters::mac_gui::*;
    use super::domain::run::Outcome;
    use super::surface::{BridgeError, WindowInfo};

    fn el(r: &str, label: &str, role: &str) -> Element {
        Element { r: ElementRef(r.into()), role: role.into(), subrole: String::new(), label: label.into(), value: None, actions: vec!["AXPress".into()], value_settable: false, enabled: true, selected: None, expanded: None, depth: 1, parent: None }
    }
    fn els(items: Vec<Element>) -> Elements {
        Elements { app_name: "TextEdit".into(), snapshot: 3, items }
    }
    fn win(handle: &str, pid: u32, cg: Option<i64>) -> WindowInfo {
        WindowInfo { handle: handle.into(), pid, cg_id: cg, ..Default::default() }
    }
    fn target() -> WindowRef {
        WindowRef { pid: 501, window_id: 8812, app_name: "TextEdit".into(), title: String::new(), bundle_id: String::new(), document: String::new() }
    }
    fn unsupported(r: Result<String, GuiResult>) -> String {
        match r {
            Err(GuiResult::Unsupported(why)) => why,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mac_gui_find_presses_only_a_single_exact_match() {
        let e = els(vec![el("e3.1", "保存", "AXButton"), el("e3.2", "保存", "AXMenuItem"), el("e3.3", "閉じる", "AXButton")]);
        assert_eq!(e.find("保存", "AXButton").unwrap().r, ElementRef("e3.1".into()));
        assert!(e.find("保", "AXButton").unwrap_err().contains("wasn't found"), "部分一致で押さない");
        let dup = els(vec![el("e3.1", "7", "AXButton"), el("e3.2", "7", "AXButton")]);
        assert!(dup.find("7", "AXButton").unwrap_err().contains("There are 2 controls named"));
    }

    /// OPEN-INPLACE-1101 の 3（owner 241「No control whose name contains “Sounds”」）: label の無い一覧の行は、子の AXStaticText が名前（`name_of`）で、
    /// `find_containing` は役割を名指さない時その行も押せる物に数える（AXPress を持たない行 = 選ぶ）。label の在る物は今まで通り label。
    /// 負の対照: `name_of` を label だけに戻す → 1 つ目が赤・`find_containing` の押せる物から行を外す → 2 つ目が赤。
    #[test]
    fn open_inplace_list_rows_are_named_by_their_text_and_can_be_pressed() {
        let child = |r: &str, parent: &str, text: &str| Element { r: ElementRef(r.into()), role: "AXStaticText".into(), subrole: String::new(), label: String::new(), value: Some(text.into()), actions: vec![], value_settable: false, enabled: true, selected: None, expanded: None, depth: 3, parent: Some(ElementRef(parent.into())) };
        let row = |r: &str| Element { r: ElementRef(r.into()), role: "AXRow".into(), subrole: String::new(), label: String::new(), value: None, actions: vec![], value_settable: false, enabled: true, selected: None, expanded: None, depth: 2, parent: Some(ElementRef("e3.1".into())) };
        let outline = Element { r: ElementRef("e3.1".into()), role: "AXOutline".into(), subrole: String::new(), label: String::new(), value: None, actions: vec![], value_settable: false, enabled: true, selected: None, expanded: None, depth: 1, parent: None };
        let e = els(vec![outline, row("e3.2"), child("e3.3", "e3.2", "Acoustic Guitar"), row("e3.4"), child("e3.5", "e3.4", "Clean Guitar"), el("e3.6", "Library", "AXButton")]);
        let names: Vec<String> = e.items.iter().filter(|i| i.role == "AXRow" || i.role == "AXButton").map(|i| e.name_of(i)).collect();
        assert_eq!(names, ["Acoustic Guitar", "Clean Guitar", "Library"], "行の名前が子の字にならない");
        assert_eq!(e.find_containing("acoustic", "", "AXPress").map(|x| x.r.clone()), Ok(ElementRef("e3.2".into())), "子の字で名前の付いた行を押せる物に数えない");
        assert!(e.find_containing("guitar", "", "AXPress").unwrap_err().contains("There are 2 controls"), "2 つの行を 1 つに数えた");
    }

    /// G31b interface: AXDocument の無い窓を役割で決める口の署名（呼ぶ側の形 = 1 つの app の window_list の窓 → 窓 1 つか理由の 1 行）
    #[test]
    fn g31b_pick_by_role_signature() {
        let _: fn(&[WindowInfo], &str) -> Result<WindowInfo, String> = pick_by_role;
    }

    /// G31b: 役割の決め方（題名を使わない）。負の対照: 「普通の窓 && 動かせる」の絞りを外すと About（main）を選んで赤
    #[test]
    fn g31b_pick_by_role_prefers_the_movable_standard_window_then_main() {
        let w = |h: &str, cg: Option<i64>, subrole: &str, main: bool, movable: bool| WindowInfo { handle: h.into(), pid: 222, cg_id: cg, subrole: subrole.into(), main, movable, ..Default::default() };
        let std = "AXStandardWindow";
        let pick = |ws: &[WindowInfo]| pick_by_role(ws, "REAPER");
        assert_eq!(pick(&[w("w1", Some(1), std, true, false), w("w2", Some(2), std, false, true)]).unwrap().handle, "w2", "About（main・動かせない）より本体");
        assert_eq!(pick(&[w("w1", Some(1), "AXDialog", false, false)]).unwrap().handle, "w1", "1 つなら役割を問わない（Zoom）");
        assert_eq!(pick(&[w("w1", Some(1), std, false, true), w("w2", Some(2), std, true, true)]).unwrap().handle, "w2", "普通の窓が 2 つなら main");
        assert_eq!(pick(&[w("w1", Some(1), "AXDialog", false, false), w("w2", Some(2), "AXSystemDialog", true, false)]).unwrap().handle, "w2", "普通の窓が無ければ main");
        assert!(pick(&[w("w1", Some(1), std, false, true), w("w2", Some(2), std, false, true)]).unwrap_err().starts_with("2 windows of REAPER are open"));
        assert!(pick(&[w("w1", Some(1), std, true, true), w("w2", None, std, false, true)]).unwrap_err().starts_with("1 REAPER window can't be matched"), "窓番号の無い窓が在れば決めない");
        let mut min = w("w1", Some(1), std, false, true);
        min.minimized = true;
        assert!(pick(&[min]).unwrap_err().contains("minimised"));
        assert!(pick(&[]).unwrap_err().starts_with("No window of REAPER is open"));
    }

    #[test]
    fn mac_gui_handle_for_needs_exactly_one_window_with_the_cg_id() {
        assert_eq!(handle_for(&[win("w1", 501, Some(1)), win("w2", 501, Some(8812)), win("w3", 777, Some(8812))], &target()), Ok("w2".to_string()));
        assert!(unsupported(handle_for(&[win("w1", 501, Some(1))], &target())).contains("wasn't found"));
        assert!(unsupported(handle_for(&[], &target())).contains("wasn't found"));
        // cg_match が none / ambiguous の窓がある → 推測で選ばない
        assert!(unsupported(handle_for(&[win("w1", 501, None)], &target())).contains("Can't settle on one window"));
        assert!(unsupported(handle_for(&[win("w1", 501, Some(8812)), win("w2", 501, Some(8812))], &target())).contains("2 windows of TextEdit match"));
    }

    #[test]
    fn mac_gui_gui_result_never_calls_a_maybe_sent_action_unsupported() {
        let failed = |code: &str| BridgeError::Failed { code: code.into(), message: format!("{code} の理由") };
        assert!(matches!(gui_result(&failed("cannot_complete"), true), GuiResult::Delivered { sure: false, .. }));
        assert!(matches!(gui_result(&BridgeError::Timeout { cmd: "ax_press".into(), ms: 5000 }, true), GuiResult::Delivered { sure: false, .. }));
        assert!(matches!(gui_result(&BridgeError::Gone("x".into()), true), GuiResult::Delivered { sure: false, .. }));
        // 送る前の失敗・明示の拒否は「送っていない」
        assert!(matches!(gui_result(&BridgeError::Gone("x".into()), false), GuiResult::Unsupported(_)));
        assert!(matches!(gui_result(&BridgeError::NotFound("無い".into()), false), GuiResult::Unsupported(_)));
        for code in ["stale_element", "not_trusted", "not_settable", "unknown_cmd"] {
            match gui_result(&failed(code), true) {
                GuiResult::Unsupported(why) => assert!(why.contains(&format!("{code} の理由")), "{why}"),
                other => panic!("{code}: {other:?}"),
            }
        }
        let o: Outcome = gui_result(&failed("cannot_complete"), true).into();
        assert!(matches!(o, Outcome::EffectUnknown { .. }));
    }

    #[test]
    fn mac_gui_error_codes_round_trip_the_bridge_names() {
        let all = [
            ("not_trusted", AxErrorCode::NotTrusted),
            ("unknown_handle", AxErrorCode::UnknownHandle),
            ("stale_element", AxErrorCode::StaleElement),
            ("invalid_element", AxErrorCode::InvalidElement),
            ("action_unsupported", AxErrorCode::ActionUnsupported),
            ("cannot_complete", AxErrorCode::CannotComplete),
            ("not_settable", AxErrorCode::NotSettable),
            ("menu_not_found", AxErrorCode::MenuNotFound),
            ("no_such_process", AxErrorCode::NoSuchProcess),
            ("bad_args", AxErrorCode::BadArgs),
        ];
        for (code, want) in all {
            assert_eq!(AxErrorCode::from_code(code), Some(want), "{code}");
        }
        assert_eq!(AxErrorCode::from_code("unknown_cmd"), None);
        // W17-2 戻し: bridge は灰色と読んでも押す（menu_disabled を返さない）
        assert_eq!(AxErrorCode::from_code("menu_disabled"), None);
        assert_eq!(AxErrorCode::from_code("StaleElement"), None);
    }

    #[test]
    fn mac_gui_chord_becomes_the_key_to_pid_request() {
        let save = Chord { key: Key::Char('s'), cmd: true, shift: true, option: false, control: false };
        let r = KeyToPidReq::chord(42, &save);
        assert_eq!(serde_json::to_value(&r).unwrap(), serde_json::json!({"pid": 42, "key": "s", "mods": ["cmd", "shift"]}));
        let ret = KeyToPidReq::chord(42, &Chord { key: Key::Named("return".into()), cmd: false, shift: false, option: false, control: false });
        assert_eq!(serde_json::to_value(&ret).unwrap(), serde_json::json!({"pid": 42, "key": "return"}), "mods が空なら送らない");
    }
}

// ---- W17-1: adapters::mac_gui の読む口（偽の bridge tests/fake-surface-bridge.py・本物の bridge と cua-driver は起こさない）----
mod mac_gui_read {
    use super::adapters::mac_gui::*;
    use super::gui::GuiError;
    use super::surface::BridgeError;
    use super::{script, tmp};
    use std::path::PathBuf;

    /// 偽の bridge を env つきで包む。log = 受け取った要求（1 行 1 JSON）
    fn bridge(tag: &str, env: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let d = tmp(tag);
        let log = d.join("bridge.log");
        let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
        let exports: String = env.iter().map(|(k, v)| format!("export {k}='{v}'\n")).collect();
        (script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\n{exports}exec python3 '{py}'", log.display())), log)
    }

    #[test]
    fn mac_gui_elements_come_from_bridge_ax_tree() {
        let (sh, log) = bridge("mg-read", &[("FAKE_AX_VALUE", "下書き")]);
        let mut g = MacGui::at(sh);
        let ws = g.windows().unwrap();
        assert_eq!(ws, vec![WindowRef { pid: 222, window_id: 300, app_name: "REAPER".into(), title: "test.rpp - REAPER".into(), bundle_id: "com.cockos.reaper".into(), document: String::new() }]);
        let els = g.elements(&ws[0]).unwrap();
        assert_eq!((els.app_name.as_str(), els.items.len()), ("REAPER", 4), "app 名は bridge の apps で読んだ値");
        assert_eq!(els.items[0].r, ElementRef(format!("e{}.0", els.snapshot)), "先頭は窓・ref は bridge の e<snapshot>.<n>");
        assert_eq!(els.find("保存", "AXButton").unwrap().actions, vec!["AXPress".to_string()]);
        assert_eq!(els.find("本文", "AXTextArea").unwrap().value.as_deref(), Some("下書き"));
        // 読み直すと snapshot が上がる（前の ref は bridge の側で stale_element）
        let again = g.elements(&ws[0]).unwrap();
        assert!(again.snapshot > els.snapshot, "{} → {}", els.snapshot, again.snapshot);
        let sent = std::fs::read_to_string(&log).unwrap();
        // owner 256: 読みの数の上限を送らない（負の対照: `AxTreeReq` に max_elements を戻すと赤）
        assert!(sent.lines().any(|l| l.contains(r#""cmd":"ax_tree""#) && l.contains(r#""handle":"w1""#)) && !sent.contains("max_elements"), "{sent}");
        // W6b（observe を消した）: 読む口は elements = read だけ。無い CG の窓番号 → 読まずに NotFound の 1 行
        assert!(g.elements(&WindowRef { window_id: 999, ..ws[0].clone() }).unwrap_err().contains("wasn't found"));
        // 許可が無い → Denied（空の木・空の一覧にしない）
        let (untrusted, _) = bridge("mg-untrusted", &[("FAKE_MODE", "untrusted")]);
        let mut u = MacGui::at(untrusted);
        assert!(u.elements(&ws[0]).is_err(), "許可が無いのに読めた（空の木にしない）");
        assert!(u.windows().unwrap_err().contains("Accessibility permission is missing"));
        // CG の窓番号と AX の窓の対応が付かない窓は一覧に出さない（推測で選ばない）
        let (no_cg, _) = bridge("mg-nocg", &[("FAKE_WL", r#"[[{"title":"a"},{"title":"b","no_cg":true}]]"#)]);
        assert_eq!(MacGui::at(no_cg).windows().unwrap().iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), vec!["a"]);
        // bridge が無い → 理由 1 行（起こせない）
        let gone = MacGui::at(PathBuf::from("/nonexistent/kangae-surface-bridge")).windows().unwrap_err();
        assert!(gone.contains("stopped") && gone.contains("/nonexistent"), "{gone}");
    }

    /// 実機（試験の側が手で回す・`--ignored`）: 本物の bridge（`KANGAE_REAL_BRIDGE`）で `KANGAE_REAL_PID` の app の窓と要素を読み、JSON を
    /// `KANGAE_REAL_OUT` に書く。照合（試験の側の CGWindowList / AX の読みと一致するか）は呼んだ側が見る。前面は変えない（読むだけ）。
    #[test]
    #[ignore]
    fn mac_gui_real_app_reads_through_the_bridge() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} が要ります"));
        let pid: u32 = var("KANGAE_REAL_PID").parse().unwrap();
        let mut g = MacGui::at(PathBuf::from(var("KANGAE_REAL_BRIDGE")));
        let ws: Vec<WindowRef> = g.windows().unwrap().into_iter().filter(|w| w.pid == pid).collect();
        let mut out = Vec::new();
        for w in &ws {
            let els = g.elements(w).unwrap();
            let items = els.items.len();
            out.push(serde_json::json!({ "window": w, "elements": els, "items": items }));
        }
        std::fs::write(var("KANGAE_REAL_OUT"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
        assert!(!ws.is_empty(), "pid {pid} の窓を 1 つも読めない");
    }

    #[test]
    fn mac_gui_gui_error_maps_bridge_failures_for_observe() {
        let f = |code: &str| gui_error(&BridgeError::Failed { code: code.into(), message: format!("m-{code}") });
        assert_eq!(f("not_trusted"), GuiError::Denied("m-not_trusted".into()));
        for code in ["unknown_handle", "stale_element", "invalid_element", "no_such_process", "menu_not_found"] {
            assert_eq!(f(code), GuiError::NotFound(format!("m-{code}")), "{code}");
        }
        for code in ["bad_args", "unknown_cmd", "action_unsupported"] {
            assert!(matches!(f(code), GuiError::Unsupported(w) if w.contains(&format!("m-{code}"))), "{code}");
        }
        assert_eq!(gui_error(&BridgeError::Timeout { cmd: "ax_tree".into(), ms: 5000 }), GuiError::Timeout { ms: 5000 });
        assert!(matches!(gui_error(&BridgeError::NotFound("無い".into())), GuiError::NotFound(w) if w == "無い"));
        for e in [BridgeError::Gone("x".into()), BridgeError::Spawn("x".into()), BridgeError::Broken("x".into())] {
            assert!(matches!(gui_error(&e), GuiError::Bridge(w) if w.contains('x')), "{e:?}");
        }
    }
}

// ---- W17-2: adapters::mac_gui の操作の口（偽の bridge の ax_press / ax_set_value / ax_menu / key_to_pid・本物の bridge と cua-driver は起こさない）----
mod mac_gui_act {
    use super::adapters::mac_gui::*;
    use super::adapters::sdk::legacy;
    use super::domain::run::{ErrorCode, Outcome};
    use super::gui::GuiError;
    use super::model::{ActionKind, DesktopTool};
    use super::{gui_at, script, tmp};
    use std::path::{Path, PathBuf};

    fn bridge(tag: &str, env: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let d = tmp(tag);
        let log = d.join("bridge.log");
        let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
        let exports: String = env.iter().map(|(k, v)| format!("export {k}='{v}'\n")).collect();
        (script(&d, "bridge.sh", &format!("export FAKE_LOG='{}'\n{exports}exec python3 '{py}'", log.display())), log)
    }
    /// log の中でその cmd を送った回数
    fn sent(log: &Path, cmd: &str) -> usize {
        std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.contains(&format!(r#""cmd":"{cmd}""#))).count()
    }
    /// 旧 lane の binding（偽の bridge の窓 = pid 222・CG の窓番号 300）
    fn desktop(app: &str, label: &str, role: &str, tool: DesktopTool, text: &str) -> ActionKind {
        ActionKind::Desktop { pid: 222, window_id: 300, label: label.into(), role: role.into(), tool, text: text.into(), app_name: app.into() }
    }
    fn save(app: &str) -> ActionKind {
        desktop(app, "保存", "AXButton", DesktopTool::Click, "")
    }
    fn body(text: &str) -> ActionKind {
        desktop("REAPER", "本文", "AXTextArea", DesktopTool::TypeText, text)
    }
    fn run(tag: &str, env: &[(&str, &str)], kind: &ActionKind) -> (Outcome, PathBuf) {
        let (sh, log) = bridge(tag, env);
        (gui_at(sh).run_desktop(kind), log)
    }

    /// YC_GATE #3・#6（CON-005 / ADR-003 C7）: pid / window_id が別の app に使い回された窓は、読み直して app 名が違えば押さない
    #[test]
    fn mac_gui_refuses_reused_window_of_another_app() {
        let (o, log) = run("mg-reused", &[], &save("Notes"));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Notes") && reason.contains("REAPER") && reason.contains("Add the step again")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 0, "別 app の窓を押した");
        assert_eq!(sent(&log, "ax_tree"), 1, "押す前に読み直す");
        assert!(legacy(o).is_err());
    }

    /// W19-1 人の lease（YC_ACCEPTANCE F2・Native Direct の面は人の入力が Shibubu を通らないので元の app を観る）: 宛先の app が前面の間は
    /// AI の操作を送らない（registry の道の run_desktop も MacGui::act も）。正の対照: 前面が別の app（Shibubu）なら送る
    #[test]
    fn mac_gui_refuses_to_act_while_the_human_holds_the_app() {
        let (sh, log) = bridge("mg-lease", &[("FAKE_FRONT_PID", "222")]);
        let mut g = gui_at(sh);
        let o = g.run_desktop(&save("REAPER"));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Someone is using the app")), "{o:?}");
        let denied = g.key(222, &"cmd+s".parse().unwrap());
        assert!(matches!(&denied, GuiResult::Unsupported(why) if why.contains("Someone is using the app")), "{denied:?}");
        assert_eq!(sent(&log, "ax_press") + sent(&log, "key_to_pid"), 0, "人が持っている間に送った");
        assert!(sent(&log, "frontmost") >= 2, "送る前に前面を読んでいない");
        let (sh, log) = bridge("mg-lease-free", &[]);
        assert!(matches!(gui_at(sh).run_desktop(&save("REAPER")), Outcome::Unverified { .. }));
        assert_eq!(sent(&log, "ax_press"), 1, "人が持っていないのに送らない（いつも拒否する実装）");
    }

    /// 押した = 送っただけ。効いたかは読んでいない → Unverified（legacy の Ok の文に「未検証」）・Verified にしない
    #[test]
    fn mac_gui_press_is_delivered_not_verified() {
        for app in ["REAPER", ""] {
            let (o, log) = run("mg-press", &[], &save(app));
            assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("AXPress") && reason.contains("isn't verified")), "{app}: {o:?}");
            let text = legacy(o).unwrap();
            assert!(text.contains("未検証"), "{text}");
            // 読み直した最新の ref（偽の bridge の 2 番目 =「保存」）を押した
            let lines = std::fs::read_to_string(&log).unwrap();
            let press = lines.lines().position(|l| l.contains(r#""cmd":"ax_press""#)).expect("押していない");
            let tree = lines.lines().position(|l| l.contains(r#""cmd":"ax_tree""#)).unwrap();
            assert!(tree < press, "読む前に押した");
            assert!(lines.lines().nth(press).unwrap().contains(r#""element":"e1.2""#), "{lines}");
            assert_eq!(sent(&log, "ax_press"), 1);
        }
    }

    /// 送る前の失敗（Rejected / Failed・何も送っていない）と送った後に分からない（EffectUnknown）を分ける（CD-03）
    #[test]
    fn mac_gui_outcomes_before_and_after_dispatch() {
        let pressed = |tag: &str, env: &[(&str, &str)], kind: &ActionKind| {
            let (o, log) = run(tag, env, kind);
            (o, sent(&log, "ax_press") + sent(&log, "ax_set_value") + sent(&log, "key_to_pid"), sent(&log, "ax_tree"))
        };
        // 送る前: 同名が複数・途中までの読み・許可なし・無い要素・入力の長さ
        // ENGINE-AUDIT W5 の契約: 同名が複数 = 押さずに Failed { TargetAmbiguous }（Core の execute が探す段で道〈within〉で絞る・前は Rejected の文）
        assert!(matches!(pressed("mg-dup", &[("FAKE_AX_DUP", "1")], &save("REAPER")), (Outcome::Failed { error: ErrorCode::TargetAmbiguous }, 0, 1)));
        // 窓だけ読んだ所で app が答えなくなった（G31・owner 256）: AX の上限を伸ばして続きを 1 回 = ax_tree 2 回・それでも進まなければ途中の読みを使わず押さない
        assert!(matches!(pressed("mg-part", &[("FAKE_AX_PARTIAL", "1")], &save("REAPER")), (Outcome::Rejected { reason }, 0, 2) if reason.contains("stopped answering") && reason.contains("1 item so far")));
        assert!(matches!(pressed("mg-untr", &[("FAKE_MODE", "untrusted")], &save("REAPER")), (Outcome::Rejected { reason }, 0, 0) if reason.contains("permission")));
        assert_eq!(pressed("mg-gone", &[], &desktop("REAPER", "無い", "AXButton", DesktopTool::Click, "")), (Outcome::Failed { error: ErrorCode::TargetNotFound }, 0, 1));
        assert!(matches!(pressed("mg-empty", &[], &body("")), (Outcome::Rejected { reason }, 0, 0) if reason.contains("empty or too long")));
        assert!(matches!(pressed("mg-long", &[], &body(&"a".repeat(1001))), (Outcome::Rejected { .. }, 0, 0)));
        // bridge が要素を拒んだ（action_unsupported）= 送ったが bridge は何もしていない → Rejected
        assert!(matches!(pressed("mg-unsup", &[("FAKE_AX_FAIL", "action_unsupported")], &save("REAPER")), (Outcome::Rejected { reason }, 1, 1) if reason.contains("Not sent")));
        // 送った後: app が答えない（cannot_complete）→ EffectUnknown（自動で再送しない）
        assert!(matches!(pressed("mg-cc", &[("FAKE_AX_FAIL", "cannot_complete")], &save("REAPER")), (Outcome::EffectUnknown { reason }, 1, 1) if reason.contains("run it twice")));
        // 送った後に bridge が切れた（応答なし）→ 送ったかもしれない = EffectUnknown（「送っていない」と言わない）
        assert!(matches!(pressed("mg-die", &[("FAKE_DIE_ON", "ax_press")], &save("REAPER")), (Outcome::EffectUnknown { reason }, 1, 1) if reason.contains("run it twice")));
        // 文字: 値を置いて読み戻しが一致 = Verified・違う = Unverified（旧 lane の履歴で「完了」にしない語を持つ）
        // G74: 置いた後に窓をもう 1 回読む（置いた道と別の読み）= ax_tree 2 回
        assert!(matches!(pressed("mg-set", &[], &body("こんにちは")), (Outcome::Verified { evidence }, 1, 2) if evidence[0].contains("they match")));
        // ③-8・owner 256: 打つ前の読みも click と同じく窓の全部（Discord の入力欄は窓の奥・bridge の ax_tree は幅優先）= 読みの数の上限を送らない
        let (_, log) = run("mg-set-max", &[], &body("こんにちは"));
        let tree = std::fs::read_to_string(&log).unwrap().lines().filter(|l| l.contains(r#""cmd":"ax_tree""#)).map(String::from).collect::<Vec<_>>();
        assert!(!tree.is_empty() && tree.iter().all(|l| !l.contains("max_elements")), "打つ前の読みに上限を付けた: {tree:?}");
        assert!(matches!(pressed("mg-mis", &[("FAKE_SET_MISMATCH", "1")], &body("こんにちは")), (Outcome::Unverified { reason }, 1, 1) if reason.contains("differs") && reason.contains("Whether the work itself succeeded isn't verified")));
        // 置けない欄は打たずに拒む（pid 宛てに打つと app の今の focus = 承認していない欄や 1 字の shortcut に入る）。W19 で AXFocused を置いてから打つ
        let (o, log) = run("mg-type", &[("FAKE_AX_NOT_SETTABLE", "1")], &body("こんにちは"));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Can't put text") && reason.contains("typing step again")), "{o:?}");
        assert_eq!((sent(&log, "ax_set_value"), sent(&log, "key_to_pid")), (0, 0), "置けない欄に何か送った");
        // 前面が変わった（D-g）: 送れていても成功の文にしない
        let (o, _) = run("mg-front", &[("FAKE_FRONT_CHANGE", "1")], &save("REAPER"));
        assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("The app in front changed")), "{o:?}");
        // bridge を起こせない → 理由 1 行（押していない）
        let gone = MacGui::at(PathBuf::from("/nonexistent/kangae-surface-bridge")).run_desktop(&save("REAPER"));
        assert!(matches!(&gone, Outcome::Rejected { reason } if reason.contains("/nonexistent")), "{gone:?}");
        assert!(matches!(MacGui::at(PathBuf::from("/x")).run_desktop(&ActionKind::WebRead), Outcome::Rejected { .. }));
    }

    /// G74（owner 12:2x・P0・③-8 = 1117961 で Discord の入力欄に在った書きかけ 6 字が投稿された）: registry の `desktop.ax.type_text` は
    /// ① 打つ前の欄に字が在る（placeholder = label と同じ値は空とみなす）時は、書類の窓（input の bundle_id と document で決め直した窓 = 頼みの中身が
    ///    書類そのもの）でない限り置かずに Rejected（ax_set_value 0・何も送っていない・人の言葉の 1 行）
    /// ② 置いた後、置いた道（bridge の ax_set_value の読み戻し）と別の読み（窓を読み直した ax_tree の欄の値）の両方が打った文の時だけ Verified（証拠に両方）。
    ///    rich text の欄（FAKE_SET_LASTS=0 = 置いた道だけが合うと言う）は Unverified
    /// ③ `recheck` = 同じ欄を label と role で読み直す（送らない）: 置いた後に戻った欄（FAKE_SET_LASTS=1）は Verified でない・key は None（読み直す口が無い）
    /// 負の対照（手で取る）: ① の止めを外す → draft で ax_set_value 1・② の別の読みを外す → rich で Verified・③ の読みを外す → reverts で Verified
    #[test]
    fn g74_type_text_keeps_a_draft_and_reads_the_field_again() {
        use super::adapters::AdapterRegistry;
        let typed = |extra: &[(&str, &str)]| {
            let mut v = serde_json::json!({"pid": 222, "window_id": 300, "app_name": "REAPER", "label": "本文", "role": "AXTextArea", "text": "nonce-g74"});
            for (k, x) in extra {
                v[*k] = serde_json::json!(x);
            }
            v
        };
        let reg = |tag: &str, env: &[(&str, &str)], also: Option<&str>| {
            let (sh, log) = bridge(tag, env);
            let g = gui_at(sh.clone());
            if let Some(b) = also {
                super::surface::grant_act_consent(&sh.with_file_name("act-consents.jsonl"), b, 1.0).unwrap();
            }
            let mut r = AdapterRegistry::default();
            r.register(Box::new(g)).unwrap();
            r.check();
            (r, log)
        };
        let go = |r: &mut AdapterRegistry, i: &serde_json::Value| r.invoke(super::APP, "desktop.ax.type_text", i, Some(&super::consent("desktop.ax.type_text", &["desktop.input"], i))).0;
        // ① 書きかけの欄: 置かない
        let (mut r, log) = reg("g74-draft", &[("FAKE_AX_VALUE", "人の書きかけ")], None);
        let o = go(&mut r, &typed(&[]));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("already has text") && reason.contains("nothing was sent")), "{o:?}");
        assert_eq!(sent(&log, "ax_set_value"), 0, "書きかけの欄に置いた");
        // placeholder（値 = label）は空とみなして置く
        let (mut r, log) = reg("g74-placeholder", &[("FAKE_AX_VALUE", "本文")], None);
        assert!(matches!(go(&mut r, &typed(&[])), Outcome::Verified { .. }));
        assert_eq!(sent(&log, "ax_set_value"), 1);
        // 書類の窓（bundle_id と document で決め直した TextEdit の README.md）= 頼みの中身が書類そのもの → 中身が在っても置く
        let doc = [("FAKE_AX_VALUE", "人の書きかけ"), ("FAKE_APP_BUNDLE", "com.apple.TextEdit"), ("FAKE_APP_NAME", "TextEdit"), ("FAKE_TITLES", "README.md")];
        let (mut r, log) = reg("g74-document", &doc, Some("com.apple.TextEdit"));
        let o = go(&mut r, &typed(&[("app_name", "TextEdit"), ("bundle_id", "com.apple.TextEdit"), ("document", "README.md")]));
        assert!(matches!(&o, Outcome::Verified { .. }), "{o:?}");
        assert_eq!(sent(&log, "ax_set_value"), 1);
        // document だけ（bundle_id が無い = 窓を書類で決め直していない）は書類と読まない
        let (mut r, log) = reg("g74-document-only", &doc, Some("com.apple.TextEdit"));
        assert!(matches!(go(&mut r, &typed(&[("app_name", "TextEdit"), ("document", "README.md")])), Outcome::Rejected { .. }));
        assert_eq!(sent(&log, "ax_set_value"), 0);
        // ② 別の読み: 置いた道だけが合うと言う rich text の欄 → Verified にしない
        let (mut r, log) = reg("g74-rich", &[("FAKE_SET_LASTS", "0")], None);
        let o = go(&mut r, &typed(&[]));
        assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("read again")), "{o:?}");
        assert_eq!((sent(&log, "ax_set_value"), sent(&log, "ax_tree")), (1, 2), "置く前の読み・置いた後の別の読み");
        // 普通の欄: 両方の読みを証拠に
        let (mut r, log) = reg("g74-plain", &[], None);
        let o = go(&mut r, &typed(&[]));
        assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].contains("they match") && evidence[0].contains("read again")), "{o:?}");
        // ③ 送る直前の再確認（送らない読み）
        let rc = r.recheck("desktop.ax.type_text", &typed(&[]));
        assert!(matches!(&rc, Some(Outcome::Verified { evidence }) if evidence[0].contains("right before")), "{rc:?}");
        assert!(r.recheck("desktop.key", &serde_json::json!({"pid": 222, "app_name": "REAPER", "key": "return"})).is_none());
        assert_eq!(sent(&log, "ax_set_value"), 1, "再確認で置き直した");
        let (mut r, _) = reg("g74-reverts", &[("FAKE_SET_LASTS", "1")], None);
        assert!(matches!(go(&mut r, &typed(&[])), Outcome::Verified { .. }), "置いた直後の別の読みは合う");
        let rc = r.recheck("desktop.ax.type_text", &typed(&[]));
        assert!(matches!(&rc, Some(Outcome::Unverified { .. })), "置いた後に戻った欄を合うと言った: {rc:?}");
    }

    /// G79（r20 #20: 本物の計画の type_text が README の全文を 1 行に置き換える形だった）: `desktop.ax.type_text` の replace = 欄の今の字の中で完全一致する所が
    /// ちょうど 1 つの時だけ、そこだけを text に（bridge の `ax_replace_text`・全選択しない）→ 他の行は残る・Verified（読み戻しに text・長さの差 = 置き換えた分）・
    /// 送る直前の再確認も合う。0 か所・2 か所 → 何も送らず Rejected（数を 1 行に）・欄は元のまま。読んだ字（bridge は 2000 字で切る）では 1 か所でも、
    /// 切れた先にもう 1 か所在る → bridge が全文で数え直して何も置かない。replace が無い → 今まで通り全文を置き換える。
    /// 負の対照（G79_MUT=all = replace を bridge に渡さず全選択の置き方に戻す）→ 1 か所の場面で他の行が消えて赤
    #[test]
    fn g79_type_text_replaces_exactly_one_place_or_nothing() {
        use super::adapters::AdapterRegistry;
        const README: &str = "# README\n\nline two\nline three";
        let typed = |extra: &[(&str, &str)]| {
            let mut v = serde_json::json!({"pid": 222, "window_id": 300, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "README.md", "label": "本文", "role": "AXTextArea", "text": "# Shibubu Demo"});
            for (k, x) in extra {
                v[*k] = serde_json::json!(x);
            }
            v
        };
        let reg = |tag: &str, body: &str| {
            let (sh, log) = bridge(tag, &[("FAKE_AX_VALUE", body), ("FAKE_APP_BUNDLE", "com.apple.TextEdit"), ("FAKE_APP_NAME", "TextEdit"), ("FAKE_TITLES", "README.md")]);
            super::surface::grant_act_consent(&sh.with_file_name("act-consents.jsonl"), "com.apple.TextEdit", 1.0).unwrap();
            let mut r = AdapterRegistry::default();
            r.register(Box::new(gui_at(sh))).unwrap();
            r.check();
            (r, log)
        };
        let go = |r: &mut AdapterRegistry, i: &serde_json::Value| r.invoke(super::APP, "desktop.ax.type_text", i, Some(&super::consent("desktop.ax.type_text", &["desktop.input"], i))).0;
        let field = |r: &mut AdapterRegistry| {
            let (_, obs) = r.invoke(super::APP, "desktop.window.state", &serde_json::json!({"pid": 222, "window_id": 300, "app_name": "TextEdit", "role": "AXTextArea"}), None);
            obs.and_then(|o| o.payload["controls"][0]["value"].as_str().map(String::from)).unwrap_or_default()
        };
        // 1 か所: 題の行だけが変わる
        let (mut r, log) = reg("g79-one", README);
        let o = go(&mut r, &typed(&[("replace", "# README")]));
        assert!(matches!(&o, Outcome::Verified { evidence } if evidence[0].contains("Replaced the one place") && evidence[0].contains("read again")), "{o:?}");
        assert_eq!(field(&mut r), "# Shibubu Demo\n\nline two\nline three", "題の行の他も変えた");
        assert_eq!((sent(&log, "ax_replace_text"), sent(&log, "ax_set_value")), (1, 0));
        let rc = r.recheck("desktop.ax.type_text", &typed(&[("replace", "# README")]));
        assert!(matches!(&rc, Some(Outcome::Verified { .. })), "置き換えた後の送る直前の再確認: {rc:?}");
        // 0 か所・2 か所: 何も送らない
        for (tag, body, rep, n) in [("g79-none", README, "# Nope", "0 places"), ("g79-two", "x\nline\nx", "x", "2 places")] {
            let (mut r, log) = reg(tag, body);
            let o = go(&mut r, &typed(&[("replace", rep)]));
            assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains(n) && reason.contains("nothing was put")), "{tag}: {o:?}");
            assert_eq!((sent(&log, "ax_replace_text"), sent(&log, "ax_set_value")), (0, 0), "{tag}");
            assert_eq!(field(&mut r), body, "{tag}");
        }
        // 読んだ字（2000 字で切れる）の中では 1 か所・全文では 2 か所 → bridge が何も置かない
        let long = format!("x{}x", "y".repeat(2100));
        let (mut r, log) = reg("g79-cut", &long);
        let o = go(&mut r, &typed(&[("replace", "x")]));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("2 か所")), "{o:?}");
        assert_eq!(sent(&log, "ax_replace_text"), 1);
        assert_eq!(field(&mut r).chars().count(), 2000, "偽の bridge も本物と同じく 2000 字で切る（切れた先の x は読みに無い）");
        // 空の replace は送らない・replace が無い = 今まで通り全文
        let (mut r, log) = reg("g79-empty", README);
        assert!(matches!(go(&mut r, &typed(&[("replace", "")])), Outcome::Rejected { .. }));
        assert_eq!(sent(&log, "ax_replace_text") + sent(&log, "ax_set_value"), 0);
        let (mut r, log) = reg("g79-whole", README);
        assert!(matches!(go(&mut r, &typed(&[])), Outcome::Verified { .. }));
        assert_eq!((field(&mut r).as_str(), sent(&log, "ax_set_value")), ("# Shibubu Demo", 1), "replace 無しは全文");
    }

    /// G74b（owner 12:2x「削除が shibubu からできるかテスト」・偽の bridge の FAKE_MESSAGES）: message の行に AXShowMenu → menu の Delete Message →
    /// 確認の dialog の Delete は、dialog の preview が消したい本文を見せている時だけ押す（`only_if`・同じ読みの中で・dialog の中だけを見る）。
    /// 合う: 行（外の group と中の字 = 1 本の鎖なので 1 つ）・menu の項目・dialog の Delete の 3 回押す → 行が 1 つ減り、その本文の行は無い（window.state を絞って読む）
    /// 違う（FAKE_MESSAGES_OFF=1 = app が別の行の menu を開いた）: preview は別の本文・後ろの行には消したい本文がまだ在る → Delete を押さない・行は減らない
    /// 候補が 2 つ・dialog の外の要素の only_if → 何も押さない。絞る読みが途中まで → Rejected（無いと言わない）。
    /// 負の対照（手で取る）: dialog の中だけでなく窓全体で探す → 違う場面で Delete を押す・鎖を 1 つと数えない → 合う場面の行で押さない
    #[test]
    fn g74b_delete_presses_only_after_the_dialog_shows_the_message() {
        use super::adapters::AdapterRegistry;
        let reg = |tag: &str, env: &[(&str, &str)]| {
            let (sh, log) = bridge(tag, env);
            let mut r = AdapterRegistry::default();
            r.register(Box::new(gui_at(sh))).unwrap();
            r.check();
            (r, log)
        };
        let with = |extra: serde_json::Value| {
            let mut v = serde_json::json!({"pid": 222, "window_id": 300, "app_name": "REAPER"});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let click = |r: &mut AdapterRegistry, extra: serde_json::Value| {
            let i = with(extra);
            r.invoke(super::APP, "desktop.ax.click", &i, Some(&super::consent("desktop.ax.click", &["desktop.input"], &i))).0
        };
        let rows = |r: &mut AdapterRegistry, extra: serde_json::Value| {
            let mut i = with(serde_json::json!({"subrole": "AXDocumentArticle"}));
            for (k, x) in extra.as_object().unwrap() {
                i[k] = x.clone();
            }
            let (o, obs) = r.invoke(super::APP, "desktop.window.state", &i, None);
            (o, obs.map(|o| o.payload["controls"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap().to_string()).collect::<Vec<_>>()))
        };
        let msgs = ("FAKE_MESSAGES", "hello-row|typo-six|later-row");
        let menu = serde_json::json!({"label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu"});
        let item = serde_json::json!({"label": "Delete Message", "role": "AXMenuItem", "match": "contains"});
        let delete = serde_json::json!({"label": "Delete", "role": "AXButton", "only_if": ["typo-six"]});
        // 合う
        let (mut r, log) = reg("g74b-match", &[msgs]);
        assert_eq!(rows(&mut r, serde_json::json!({})).1.unwrap(), ["hello-row", "typo-six", "later-row"]);
        let o = click(&mut r, menu.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "{o:?}");
        assert!(std::fs::read_to_string(&log).unwrap().contains(r#""action":"AXShowMenu""#), "行に AXShowMenu を送った");
        let o = click(&mut r, item.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "{o:?}");
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Unverified { reason } if reason.contains("dialog") && !reason.contains("typo-six")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 3);
        assert_eq!(rows(&mut r, serde_json::json!({})).1.unwrap(), ["hello-row", "later-row"]);
        assert_eq!(rows(&mut r, serde_json::json!({"contains": "typo-six"})).1.unwrap(), Vec::<String>::new());
        // 違う: app が別の行の menu を開いた → dialog は later-row を見せる。後ろの行に typo-six はまだ在る
        let (mut r, log) = reg("g74b-other", &[msgs, ("FAKE_MESSAGES_OFF", "1")]);
        click(&mut r, menu.clone());
        click(&mut r, item.clone());
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Nothing was pressed") && !reason.contains("typo-six")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 2, "Delete を押した");
        assert_eq!(rows(&mut r, serde_json::json!({})).1.unwrap(), ["hello-row", "typo-six", "later-row"], "行が消えた");
        // 候補が 2 つ（hello-row と later-row）→ 何も押さない
        let (mut r, log) = reg("g74b-two", &[msgs]);
        let o = click(&mut r, serde_json::json!({"label": "row", "role": "", "match": "contains", "press": "AXShowMenu"}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("There are 2")), "{o:?}");
        // dialog の外の要素に only_if → 押さない
        let o = click(&mut r, serde_json::json!({"label": "保存", "role": "AXButton", "only_if": ["typo-six"]}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("dialog")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 0);
        // 絞る読みの途中で app が答えなくなった → 途中の読みで無い・減ったと言わない（owner 256: 途中までの読みを読めた物として使わない）
        let (mut r, _) = reg("g74b-partial", &[msgs, ("FAKE_AX_PARTIAL", "1")]);
        let (o, obs) = rows(&mut r, serde_json::json!({"contains": "typo-six"}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("stopped answering")) && obs.is_none(), "{o:?}");
        // G74c（owner 14:4x「文字そのものを長押しすると出てくる」・Discord 15:0x の実測の形 = menu は AXGroup ＋ AXMenuItem・遅れて描かれ・その間は行が消える）:
        // 2 手目（role = AXMenuItem）は menu が出るまで読み直し、menu の項目だけを押す。
        // late: 2 回の読みの間 menu がまだ無い → 待って押す（dialog まで通る）/ never: menu が開かない → 何も押さない /
        // missing: menu に Delete Message が無い → 何も押さず AXCancel を 1 回 → 閉じた / sticky: 閉じない（Discord の実測）→ 開いたまま・Esc /
        // hidden: menu が出ている間の window.state は読まない（行 0 を消えたと読まない）
        // 負の対照（G74C_MUT）: axmenu = menu を role AXMenu だけで探す → late が「No menu」/ nowait = 読み直さない → late が「No menu」/
        // cancel = AXCancel を送らない → missing が「still open」/ state = menu の間も読む → hidden が行 0 を返す
        let (mut r, log) = reg("g74c-late", &[msgs, ("FAKE_MENU_LATE", "2")]);
        click(&mut r, menu.clone());
        let o = click(&mut r, item.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "menu が描かれる前に探して止まった: {o:?}");
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "dialog が開いていない: {o:?}");
        assert_eq!(sent(&log, "ax_press"), 3);
        let (mut r, log) = reg("g74c-never", &[msgs, ("FAKE_MENU_NEVER", "1")]);
        click(&mut r, menu.clone());
        let o = click(&mut r, item.clone());
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("No menu") && reason.contains("“Delete Message”")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 1, "menu が無いのに押した");
        for (tag, sticky, says) in [("g74c-missing", "0", "was closed"), ("g74c-sticky", "1", "still open")] {
            let (mut r, log) = reg(tag, &[msgs, ("FAKE_MENU_ITEMS", "Reply|Copy Text"), ("FAKE_MENU_STICKY", sticky)]);
            click(&mut r, menu.clone());
            let o = click(&mut r, item.clone());
            assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains(says) && reason.contains("nothing in it was pressed") && (sticky == "0" || reason.contains("Esc"))), "{tag}: {o:?}");
            assert_eq!(std::fs::read_to_string(&log).unwrap().matches(r#""action":"AXCancel""#).count(), 1, "{tag}: AXCancel を 1 回");
            assert_eq!(sent(&log, "ax_press"), 2, "{tag}: AXShowMenu と AXCancel だけ（項目を押していない）");
        }
        let (mut r, _) = reg("g74c-hidden", &[msgs]);
        click(&mut r, menu.clone());
        let (o, obs) = rows(&mut r, serde_json::json!({"contains": "typo-six"}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("menu")) && obs.is_none(), "menu の間の行 0 を読んだ: {o:?} {obs:?}");
        // G74e（本物 17:10 の 3 回目 = Delete Message への AXPress は届いたが Discord は何もしなかった・menu は開いたまま・dialog 0）:
        // only_if の手（確認の dialog の button）は前の手で出る dialog を MENU_WAIT まで待ち、出なければ何も押さず Rejected（確認が出なかった・menu が開いたままなら Esc）。
        // dead: 項目を押しても何も起きない / late: dialog が 2 回の読みの間まだ無い → 待って押す / only_if の無い手の無い要素は今までどおり TargetNotFound
        // 負の対照（G74E_MUT）: arm = only_if の手の (0, true) を TargetNotFound に戻す → dead が Failed / dialogwait = dialog を待たない → late が止まる
        let (mut r, log) = reg("g74e-dead", &[msgs, ("FAKE_ITEM_DEAD", "1")]);
        click(&mut r, menu.clone());
        click(&mut r, item.clone());
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("No confirmation") && reason.contains("“Delete” wasn't pressed") && reason.contains("Esc") && !reason.contains("typo-six")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 2, "Delete を押した");
        let o = click(&mut r, serde_json::json!({"label": "Delete", "role": "AXButton"}));
        assert_eq!(o, Outcome::Failed { error: ErrorCode::TargetNotFound }, "only_if の無い手まで変えた");
        let (mut r, log) = reg("g74e-late", &[msgs, ("FAKE_DIALOG_LATE", "2")]);
        click(&mut r, menu.clone());
        click(&mut r, item.clone());
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "dialog が描かれる前に探して止まった: {o:?}");
        assert_eq!(sent(&log, "ax_press"), 3);
        // G74f（本物 18:37 = ⌘H で隠れた Discord への AXShowMenu は rc 0 で答えたが menu は出なかった・18:45 = 見せた後に AXFocused → AXPress で確認の dialog が出た）:
        // hidden: 隠れたままの app に AXShowMenu を送らない（どの app でも・読むのは通す・W8a から解いてから送る = 下の 3 行）= Rejected「… is hidden, so nothing was sent …」・押した手 0 /
        // focus: menu の項目への AXPress だけ focus を付けて送る（行の AXShowMenu と dialog の Delete には付けない）・focus が要る app（FAKE_ITEM_FOCUS）でも dialog まで通る
        // 負の対照（G74F_MUT）: hidden = 隠れた app の検査を外す → hidden で AXShowMenu が送られる / focus = focus を付けない → 付いた手が無い・Delete が「No confirmation」
        // ENGINE-AUDIT W8a（spec 262「隠れていたら解いてから読み直す」・r8b = 隠れた GarageBand に承認の後 0 手）: 隠れた app は窓を読む前に app_unhide（前面は取らない）→
        // 読み直して送る。解けない（FAKE_UNHIDE_STUCK）= 隠れたままの app への AXShowMenu だけ断る（G74f の本物）・AXPress の手は送る（効いたかは読み戻し = FX-114）。
        // 読むだけでは見せない。負の対照（本体の後・手で）: invoke の shown_for_act を外す → g74f-hidden が「is hidden」で 0 手の赤 / op の AXShowMenu の条件を外す → stuck の「保存」が 0 手で赤
        let (mut r, log) = reg("g74f-hidden", &[msgs, ("FAKE_HIDDEN", "1")]);
        assert_eq!(rows(&mut r, serde_json::json!({})).1.unwrap().len(), 3, "隠れた app を読めなかった");
        assert_eq!(sent(&log, "app_unhide"), 0, "読むだけで隠れた app を見せた");
        let o = click(&mut r, menu.clone());
        assert!(!matches!(&o, Outcome::Rejected { reason } if reason.contains("is hidden")), "解かずに断った: {o:?}");
        assert_eq!((sent(&log, "app_unhide"), sent(&log, "ax_press")), (1, 1), "解いてから送る");
        let (mut r, log) = reg("g74f-stuck", &[msgs, ("FAKE_HIDDEN", "1"), ("FAKE_UNHIDE_STUCK", "1")]);
        let o = click(&mut r, menu.clone());
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.starts_with("REAPER is hidden, so nothing was sent") && reason.contains("Show its window (it can stay behind other windows)")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 0, "隠れたままの app に AXShowMenu を送った");
        let o = click(&mut r, serde_json::json!({"label": "保存", "role": "AXButton"}));
        assert_eq!(sent(&log, "ax_press"), 1, "隠れたままでも AXPress の手は送る（効いたかは読み戻し）: {o:?}");
        let (mut r, log) = reg("g74f-focus", &[msgs, ("FAKE_ITEM_FOCUS", "1")]);
        click(&mut r, menu.clone());
        click(&mut r, item.clone());
        let o = click(&mut r, delete.clone());
        assert!(matches!(&o, Outcome::Unverified { .. }), "focus を付けずに項目を押した: {o:?}");
        let focused: Vec<bool> = std::fs::read_to_string(&log).unwrap().lines().filter(|l| l.contains(r#""cmd":"ax_press""#)).map(|l| l.contains(r#""focus":true"#)).collect();
        assert_eq!(focused, [false, true, false], "focus は menu の項目の AXPress だけ");
    }

    /// G101b FX-112（Discord の窓は 2000 要素・窓全体の部分一致は別の server・channel の同じ字に当たる）: within（役割と名前）の手は bridge の ax_tree に root を渡し、
    /// その部分木の中だけで決める。within の無い手は root を送らない（今までどおり窓全体）。根が見つからない時は窓全体で代わりにせず、何も押さず 1 行。
    /// FX-114（隠れた app への入力は後で反映されうる）: この起動で書く手を送った app が後から隠れたら、window.state はその窓の読みで済み・消えたを言わず Unverified・
    /// 何も送り直さない。送っていない隠れた app は今までどおり読む（g74f-hidden）。
    /// 負の対照（G101B_MUT・g101bm.py）: root = read が root を渡さない → within の手が「There are 2」/ found = root_found の検査を外す → 根の無い手の 1 行が TargetNotFound に /
    /// sent = sent_to に入れない → 隠れた後の window.state が行を返す / hidden = window.state の隠れた検査を外す → 同じ
    #[test]
    fn g101_a_step_reads_only_its_part_and_a_hidden_app_after_a_send_is_not_called_done() {
        use super::adapters::AdapterRegistry;
        let reg = |tag: &str, env: &[(&str, &str)]| {
            let (sh, log) = bridge(tag, env);
            let mut r = AdapterRegistry::default();
            r.register(Box::new(gui_at(sh))).unwrap();
            r.check();
            (r, log)
        };
        let with = |extra: serde_json::Value| {
            let mut v = serde_json::json!({"pid": 222, "window_id": 300, "app_name": "REAPER"});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let click = |r: &mut AdapterRegistry, extra: serde_json::Value| {
            let i = with(extra);
            r.invoke(super::APP, "desktop.ax.click", &i, Some(&super::consent("desktop.ax.click", &["desktop.input"], &i))).0
        };
        let msgs = ("FAKE_MESSAGES", "hello-row|typo-six|later-row");
        let roots = |log: &std::path::Path| std::fs::read_to_string(log).unwrap().lines().filter(|l| l.contains(r#""cmd":"ax_tree""#) && l.contains(r#""root":"#)).count();
        // 窓全体: 「row」は 2 行に当たる → 押さない・root を送らない
        let (mut r, log) = reg("g101b-whole", &[msgs]);
        let o = click(&mut r, serde_json::json!({"label": "row", "role": "", "match": "contains", "press": "AXShowMenu"}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("There are 2")), "{o:?}");
        assert_eq!((sent(&log, "ax_press"), roots(&log)), (0, 0), "within の無い手が root を送った");
        // 部分木: later-row の行の中だけ → 1 つに決まり AXShowMenu を 1 回・ax_tree に root を送った
        let (mut r, log) = reg("g101b-within", &[msgs]);
        // 部分木の外の要素（窓の「保存」）は within の手では押さない（menu を開く前 = 行が見えている間に打つ。開いた後の偽の bridge は行を返さない）
        let o = click(&mut r, serde_json::json!({"label": "保存", "role": "AXButton", "within": {"role": "AXGroup", "label": "hello-row"}}));
        assert_eq!(o, Outcome::Failed { error: ErrorCode::TargetNotFound }, "部分木の外を押した");
        assert_eq!(sent(&log, "ax_press"), 0);
        let o = click(&mut r, serde_json::json!({"label": "row", "role": "", "match": "contains", "press": "AXShowMenu", "within": {"role": "AXGroup", "label": "LATER-row"}}));
        assert!(matches!(&o, Outcome::Unverified { .. }), "部分木で決まらなかった: {o:?}");
        assert_eq!(sent(&log, "ax_press"), 1);
        assert!(roots(&log) >= 1, "ax_tree に root を送っていない");
        // 根が無い → 窓全体で代わりにしない・何も押さない・1 行
        let o = click(&mut r, serde_json::json!({"label": "保存", "role": "AXButton", "within": {"role": "AXGroup", "label": "no-such-channel"}}));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("this step works in wasn't found") && reason.contains("nothing there was read or changed") && !reason.contains("no-such-channel")), "{o:?}");
        assert_eq!(sent(&log, "ax_press"), 1, "根が無いのに押した");
        // FX-114: 消した後に隠れた（⌘H）→ window.state は Unverified・送り直さない
        let (mut r, log) = reg("g101b-hidden", &[msgs, ("FAKE_HIDE_AFTER_DELETE", "1")]);
        click(&mut r, serde_json::json!({"label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu"}));
        click(&mut r, serde_json::json!({"label": "Delete Message", "role": "AXMenuItem", "match": "contains"}));
        click(&mut r, serde_json::json!({"label": "Delete", "role": "AXButton", "only_if": ["typo-six"]}));
        assert_eq!(sent(&log, "ax_press"), 3);
        let (o, obs) = r.invoke(super::APP, "desktop.window.state", &with(serde_json::json!({"subrole": "AXDocumentArticle", "contains": "typo-six"})), None);
        assert!(matches!(&o, Outcome::Unverified { reason } if reason.starts_with("REAPER is hidden now") && reason.contains("won't be sent again")) && obs.is_none(), "隠れた後の窓の読みで済みを言った: {o:?} {obs:?}");
        assert_eq!(sent(&log, "ax_press"), 3, "送り直した");
        // 対照: 隠れなければ同じ読みは行 0（消えた）を返す
        let (mut r, _) = reg("g101b-shown", &[msgs]);
        click(&mut r, serde_json::json!({"label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu"}));
        click(&mut r, serde_json::json!({"label": "Delete Message", "role": "AXMenuItem", "match": "contains"}));
        click(&mut r, serde_json::json!({"label": "Delete", "role": "AXButton", "only_if": ["typo-six"]}));
        let (o, obs) = r.invoke(super::APP, "desktop.window.state", &with(serde_json::json!({"subrole": "AXDocumentArticle", "contains": "typo-six"})), None);
        assert!(!matches!(&o, Outcome::Unverified { .. } | Outcome::Rejected { .. }) && obs.is_some_and(|x| x.payload["controls"].as_array().unwrap().is_empty()), "{o:?}");
    }

    /// W17-2 戻し: 背景の app の menu の AXEnabled は古い（TextEdit の Save は保存できる時も false と読めた・2026-10-21 の偵察）。
    /// bridge が灰色と読んでも押し（ax_menu を 1 回送る）、MacGui は成功にせず Delivered の 1 行に「押す前の読みは灰色」を足す。読みが有効なら足さない
    #[test]
    fn mac_gui_menu_presses_stale_gray_item_and_says_so() {
        let (sh, log) = bridge("mg-menu-stale", &[("FAKE_MENU_STALE", "1")]);
        let r = gui_at(sh).menu(222, &["ファイル", "保存"]);
        assert!(matches!(&r, GuiResult::Delivered { detail, sure: true } if detail.contains("grayed out") && detail.contains("isn't verified")), "{r:?}");
        assert_eq!(sent(&log, "ax_menu"), 1);
        let (sh, _) = bridge("mg-menu-fresh", &[]);
        let r = gui_at(sh).menu(222, &["ファイル", "保存"]);
        assert!(matches!(&r, GuiResult::Delivered { detail, sure: true } if !detail.contains("grayed out")), "{r:?}");
    }

    /// 旧い偽物（tests/fake-cua-driver.sh の `CUA_FAKE_MODE`）の 8 つの場面を、偽の bridge の環境変数で同じ場面に写す表（YC W18-1 interface）。
    /// W18-1 で tests/fake-cua-driver.sh を消す時、旧い場面が 1 つも試験から落ちていない事をこの表で見る。
    /// 欄: (旧い mode, 偽の bridge の環境変数, 押す要素の label, 期待する Outcome の種類, ax_press を送った回数)。
    /// 保証しない: 旧い mode の文言（cua-driver の拒否の文）と同じ文が出る事（種類と送った回数だけを揃える）。
    pub const OLD_FAKE_CUA_MODES: [(&str, &[(&str, &str)], &str, &str, usize); 8] = [
        // 押せた・効いたかは読んでいない
        ("ok", &[], "保存", "unverified", 1),
        // 古い token（stale_element_token）→ bridge が古い ref を拒む・何もしていない
        ("refuse", &[("FAKE_AX_FAIL", "stale_element")], "保存", "rejected", 1),
        // 読みが返らない（get_window_state が 20 秒寝る）→ 読む途中で bridge が切れる・押す前
        ("hang", &[("FAKE_DIE_ON", "ax_tree")], "保存", "rejected", 0),
        // 同名の要素が 2 つ → 押さない
        ("dup", &[("FAKE_AX_DUP", "1")], "保存", "rejected", 0),
        // 途中までの読み（truncated）→ 押さない
        ("truncated", &[("FAKE_AX_PARTIAL", "1")], "保存", "rejected", 0),
        // 窓の AX が CG の窓番号に結び付かない（ax_window_unresolved）→ 窓が曖昧なので押さない
        ("degraded", &[("FAKE_WL", r#"[[{"title":"test.rpp - REAPER","no_cg":true}]]"#)], "保存", "rejected", 0),
        // 登録した要素が消えた → 押さない（TargetNotFound）
        ("gone", &[], "無い", "failed", 0),
        // AXPress が -25204（cannot complete）→ 送った後に分からない
        ("axfail", &[("FAKE_AX_FAIL", "cannot_complete")], "保存", "effect_unknown", 1),
    ];

    fn outcome_kind(o: &Outcome) -> &'static str {
        match o {
            Outcome::Verified { .. } => "verified",
            Outcome::Unverified { .. } => "unverified",
            Outcome::Rejected { .. } => "rejected",
            Outcome::Failed { .. } => "failed",
            Outcome::EffectUnknown { .. } => "effect_unknown",
        }
    }

    /// YC W18-1: 偽の cua-driver の 8 つの場面（ok | refuse | hang | dup | truncated | degraded | gone | axfail）が全部、偽の bridge の上で
    /// 同じ種類の結果と同じ送った回数になる（tests/fake-cua-driver.sh を消しても場面が落ちない）
    #[test]
    fn fake_bridge_modes_cover_old_fake_cua_modes() {
        let names: Vec<&str> = OLD_FAKE_CUA_MODES.iter().map(|m| m.0).collect();
        assert_eq!(names, ["ok", "refuse", "hang", "dup", "truncated", "degraded", "gone", "axfail"], "旧い偽物の mode が表から落ちた");
        for (old, env, label, want, presses) in OLD_FAKE_CUA_MODES {
            let (o, log) = run(&format!("mg-old-{old}"), env, &desktop("REAPER", label, "AXButton", DesktopTool::Click, ""));
            assert_eq!((outcome_kind(&o), sent(&log, "ax_press")), (want, presses), "{old}: {o:?}");
        }
    }

    /// W6b（observe / act / verify を消した後）: 置く・menu・key の口（`set_text` = 同じ欄の読み戻しで Verified・`menu` / `key` = 送った記録だけ）。
    /// 前面を奪えば Delivered の detail に言う（D-g）・送った後に分からないのも Delivered・許可なしは Unsupported・読めない key は何も送らない
    #[test]
    fn mac_gui_set_menu_key_send_and_read_back() {
        let (sh, log) = bridge("mg-act", &[("FAKE_AX_VALUE", "前")]);
        let mut g = gui_at(sh);
        let w = WindowRef { pid: 222, window_id: 300, app_name: "REAPER".into(), title: String::new(), bundle_id: "com.cockos.reaper".into(), document: String::new() };
        let field = g.elements(&w).unwrap().find("本文", "AXTextArea").unwrap().r.clone();
        assert!(matches!(g.set_text(&w, &field, "後", None), GuiResult::Verified { .. }), "置いた値の読み戻し");
        assert!(matches!(g.menu(222, &["ファイル", "保存"]), GuiResult::Delivered { .. }));
        assert!(matches!(g.menu(222, &["ファイル", "無い"]), GuiResult::Unsupported(_)), "無い項目を押した");
        assert!(matches!(g.key(222, &"cmd+s".parse().unwrap()), GuiResult::Delivered { .. }));
        let keys = sent(&log, "key_to_pid");
        assert!(matches!("cmd+cmd+s".parse::<Chord>(), Err(GuiError::Unsupported(_))), "読めない key を通した");
        assert_eq!(sent(&log, "key_to_pid"), keys, "読めない key で何か送った");
        assert!(std::fs::read_to_string(&log).unwrap().contains(r#""key":"s","mods":["cmd"]"#));
        // 前面を奪った（D-g）→ Delivered の detail に言う・送った後に分からない → Delivered・許可なし → Unsupported
        for (env, want) in [("FAKE_FRONT_CHANGE", "front"), ("FAKE_AX_FAIL", "sent"), ("FAKE_DIE_ON", "sent"), ("FAKE_MODE", "denied")] {
            let val = match env { "FAKE_AX_FAIL" => "cannot_complete", "FAKE_DIE_ON" => "key_to_pid", "FAKE_MODE" => "untrusted", _ => "1" };
            let (sh, _) = bridge("mg-act-env", &[(env, val)]);
            let r = gui_at(sh).key(222, &"cmd+s".parse().unwrap());
            match want {
                "front" => assert!(matches!(&r, GuiResult::Delivered { detail, .. } if detail.contains("in front")), "{env}: {r:?}"),
                "sent" => assert!(matches!(&r, GuiResult::Delivered { .. }), "{env}: {r:?}"),
                _ => assert!(matches!(&r, GuiResult::Unsupported(_)), "{env}: {r:?}"),
            }
        }
    }

    /// 実機（試験の側が手で回す・`--ignored`）: 本物の bridge（`KANGAE_REAL_BRIDGE`）で、前に出ない試験用の app（`KANGAE_REAL_PID`・
    /// 本文の欄 AXTextField と「保存」AXButton・保存で欄の値を `KANGAE_REAL_OUT` に書く）に、旧 lane の run_desktop と MacGui の act / verify で
    /// 文字を入れて保存を押す。読み戻しは AX を通らない file（試験の側の oracle）。前面が変われば act が Err（D-g）。
    #[test]
    #[ignore]
    fn mac_gui_real_app_types_and_presses_through_the_bridge() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} が要ります"));
        let (pid, out, app) = (var("KANGAE_REAL_PID").parse::<u32>().unwrap(), PathBuf::from(var("KANGAE_REAL_OUT")), var("KANGAE_REAL_APP"));
        let path = PathBuf::from(var("KANGAE_REAL_BRIDGE"));
        // CG の窓番号は試験の側が別の bridge の window_list で決める（試験用の app は regular でないので windows() の一覧に出ない）
        let mut side = super::surface::Bridge::spawn(&path, super::surface::BRIDGE_READY_LIMIT).unwrap();
        let wl = side.call("window_list", serde_json::json!({ "pid": pid }), std::time::Duration::from_secs(10)).unwrap();
        let window_id = super::surface::parse_windows(&wl).iter().find_map(|w| w.cg_id).expect("CG の窓番号が決まる窓が無い");
        let kind = |label: &str, role: &str, tool, text: &str| ActionKind::Desktop { pid: pid as i64, window_id, label: label.into(), role: role.into(), tool, text: text.into(), app_name: app.clone() };
        // AI に任せる同意（W19-1）: 試験の人が宛先の app に「AI に任せる」を押した状態（bundle id は試験の側の bridge の apps で読む）
        let apps = side.call("apps", serde_json::json!({ "regular_only": false }), std::time::Duration::from_secs(10)).unwrap();
        let bundle = apps["apps"].as_array().into_iter().flatten().find(|a| a["pid"].as_u64() == Some(pid as u64)).and_then(|a| a["bundle_id"].as_str()).expect("pid の app の bundle id が読めない").to_string();
        let consents = super::tmp("mg-real-consent").join("act-consents.jsonl");
        super::surface::grant_act_consent(&consents, &bundle, 1.0).unwrap();
        let mut g = MacGui::at(path).with_act_consents(consents);
        let nonce = format!("w17-2-{}", std::process::id());
        let typed = g.run_desktop(&kind("本文", "AXTextField", DesktopTool::TypeText, &nonce));
        println!("run_desktop type -> {typed:?}");
        let pressed = g.run_desktop(&kind("保存", "AXButton", DesktopTool::Click, ""));
        println!("run_desktop press -> {pressed:?}");
        assert!(matches!(pressed, Outcome::Unverified { .. }), "{pressed:?}");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), nonce, "oracle（保存の file）が run_desktop の文字と違う");
        // W6b（observe / act / verify を消した）: 読み（elements）→ 置く（set_text = 読み戻しで Verified）→ 読み直して押す（古い ref は bridge が stale_element で拒む）→ oracle
        let w = WindowRef { pid, window_id, app_name: app.clone(), title: String::new(), bundle_id: bundle.clone(), document: String::new() };
        let els = g.elements(&w).unwrap();
        let field = els.items.iter().find(|e| e.role == "AXTextField").unwrap().r.clone();
        let button = els.find("保存", "AXButton").unwrap().r.clone();
        let nonce2 = format!("{nonce}-act");
        let v = g.set_text(&w, &field, &nonce2, None);
        println!("set_text -> {v:?}");
        assert!(matches!(v, GuiResult::Verified { .. }), "{v:?}");
        // set_text で読み直したので button の ref は古い → 読み直して押す
        let button2 = g.elements(&w).unwrap().find("保存", "AXButton").unwrap().r.clone();
        assert!(matches!(g.press(&w, &button, "AXPress"), GuiResult::Unsupported(_)), "古い ref で押した");
        assert!(matches!(g.press(&w, &button2, "AXPress"), GuiResult::Delivered { .. }));
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), nonce2, "oracle が set_text の文字と違う");
    }

    #[test]
    fn mac_gui_chord_parses_mods_and_refuses_unknown_shapes() {
        let c: Chord = "cmd+s".parse().unwrap();
        assert_eq!(c, Chord { key: Key::Char('s'), cmd: true, shift: false, option: false, control: false });
        let c: Chord = "Shift+Cmd+Return".parse().unwrap();
        assert_eq!(c, Chord { key: Key::Named("return".into()), cmd: true, shift: true, option: false, control: false });
        assert_eq!("tab".parse::<Chord>().unwrap().key, Key::Named("tab".into()));
        for bad in ["", "cmd+", "+", "cmd+cmd+s", "hyper+s", "cmd++s"] {
            assert!(matches!(bad.parse::<Chord>(), Err(GuiError::Unsupported(_))), "{bad:?}");
        }
    }
}
