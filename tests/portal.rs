//! src/surface（v0.14 §5・docs/diagrams.md §18）の検査。macOS の AX にも元の app にも触らない
//! （bridge は tests/fake-surface-bridge.py・python3 が要る）。PORTAL-02 / 05 / 09 / 10 / 11 / 13 の型と状態の側。
//! 実物（REAPER の窓・人の drag・保存）は docs/internal/p-portal-owner-steps.md と kangae-portal --measure。

#[allow(dead_code)]
#[path = "../src/surface/mod.rs"]
mod surface;

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use surface::*;

const HOST_PID: u32 = 4242;

fn fake_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py")
}

/// 環境変数を付けて偽の bridge を起こす小さな sh（test は並行に走るので、process の env は触らない）
fn fake(name: &str, env: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kangae-portal-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exports: String = env.iter().map(|(k, v)| format!("export {k}='{v}'\n")).collect();
    let sh = dir.join("bridge.sh");
    std::fs::write(&sh, format!("#!/bin/sh\n{exports}exec python3 '{}'\n", fake_script().display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    sh
}

fn spawn(name: &str, env: &[(&str, &str)]) -> Bridge {
    Bridge::spawn(&fake(name, env), Duration::from_secs(30)).expect("偽の bridge が立たない")
}

fn reaper() -> AppIdentity {
    AppIdentity { bundle_id: "com.cockos.reaper".into(), name: "REAPER".into(), pid: Some(222), version: Some("7.81".into()), path: None }
}

fn slot(x: f64) -> Host {
    Host { slot: Some(Frame { x, y: 120.0, w: 800.0, h: 500.0 }), ..Host::default() }
}

/// cond が真になるまで bridge の物を dock に渡す（混んだ機械向けに 30 秒まで）
fn pump(d: &mut Dock, b: &mut Bridge, host: Host, cond: impl Fn(&Dock) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        for m in b.poll() {
            d.on_msg(b, m, Instant::now());
        }
        d.on_host(b, host, Instant::now());
        d.tick(b, Instant::now());
        if cond(d) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn window(title: &str, subrole: &str) -> WindowInfo {
    WindowInfo {
        handle: "w1".into(),
        pid: 222,
        title: title.into(),
        subrole: subrole.into(),
        main: true,
        movable: true,
        frame: Some(Frame { x: 0.0, y: 0.0, w: 400.0, h: 300.0 }),
        ..WindowInfo::default()
    }
}

// ---- Lifecycle（図 §18 の表）----

#[test]
fn lifecycle_follows_diagram_18() {
    use LifeEvent as E;
    use Lifecycle as L;
    let ok = [
        (L::Unresolved, E::Found, L::Discovered),
        (L::Unresolved, E::NotFound, L::Unresolved),
        (L::Discovered, E::AttachRequested { trusted: false }, L::PermissionRequired),
        (L::Discovered, E::AttachRequested { trusted: true }, L::Attaching),
        (L::PermissionRequired, E::PermissionGranted, L::Discovered),
        (L::PermissionRequired, E::Cancel, L::Closed),
        (L::Attaching, E::Attached, L::Active),
        (L::Attaching, E::AttachFailed, L::Degraded),
        (L::Active, E::HostMoved, L::Resizing),
        (L::Resizing, E::FrameOk, L::Active),
        (L::Resizing, E::FrameOff, L::Degraded),
        (L::Active, E::HostHidden, L::Hidden),
        (L::Hidden, E::HostShown, L::Active),
        (L::Active, E::HumanMoved, L::Degraded),
        (L::Degraded, E::Redock, L::Attaching),
        (L::Active, E::SourceGone, L::SourceUnavailable),
        (L::Hidden, E::SourceGone, L::SourceUnavailable),
        (L::SourceUnavailable, E::SourceBack, L::Attaching),
        (L::SourceUnavailable, E::SourceAmbiguous, L::SourceUnavailable),
        (L::Active, E::Handoff, L::SwitchingBackend),
        (L::SourceUnavailable, E::Handoff, L::SwitchingBackend),
        (L::SwitchingBackend, E::HandoffDone, L::Detached),
        (L::Detached, E::Redock, L::Attaching),
        (L::Detached, E::Close, L::Closed),
        (L::Active, E::Close, L::Closed),
    ];
    for (from, ev, to) in ok {
        assert_eq!(from.next(ev), Ok(to), "{from:?} + {ev:?}");
    }
    // 表に無い組は Err（状態を変えない）
    let refused = [
        (L::Unresolved, E::Attached),
        (L::Discovered, E::Attached),
        (L::PermissionRequired, E::AttachRequested { trusted: true }),
        (L::Hidden, E::HostMoved),
        (L::Detached, E::HostMoved),
        (L::Degraded, E::Attached),
        (L::SourceUnavailable, E::HostMoved),
        (L::SwitchingBackend, E::HostMoved),
        (L::Detached, E::Handoff),
        (L::Unresolved, E::SourceBack),
    ];
    for (from, ev) in refused {
        assert!(from.next(ev).is_err(), "{from:?} + {ev:?} が通った");
    }
    // closed は終わり
    for ev in [E::Found, E::Close, E::Redock, E::Attached] {
        assert!(L::Closed.next(ev).is_err());
    }
}

#[test]
fn only_attaching_active_resizing_touch_the_window() {
    use Lifecycle as L;
    let all = [L::Unresolved, L::Discovered, L::PermissionRequired, L::Attaching, L::Active, L::Hidden, L::Resizing, L::SourceUnavailable, L::Degraded, L::SwitchingBackend, L::Detached, L::Closed];
    let touching: Vec<Lifecycle> = all.into_iter().filter(|l| l.touches_window()).collect();
    assert_eq!(touching, vec![L::Attaching, L::Active, L::Resizing]);
}

// ---- Control Lease（PORTAL-09）----

#[test]
fn lease_refuses_agent_while_human_has_control() {
    let mut lease = ControlLease::human(0.0);
    for a in [AgentAction::Input, AgentAction::Focus, AgentAction::MoveWindow] {
        assert!(lease.agent_may(a).is_err(), "人が操作中なのに {a:?} が通った");
    }
    // 負の対照: 人が渡せば通る（検査が常に Err を返すだけではない）
    lease.hand_to_agent(1.0);
    for a in [AgentAction::Input, AgentAction::Focus, AgentAction::MoveWindow] {
        assert!(lease.agent_may(a).is_ok());
    }
    // 人はいつでも取り戻せる
    lease.human_input(2.0);
    assert!(lease.agent_may(AgentAction::Input).is_err());
    assert_eq!(lease.since_ms, 2.0);
    // drag の途中は backend を切り替えない
    assert!(lease.may_switch_backend(1).is_err());
    assert!(lease.may_switch_backend(0).is_ok());
}

// ---- Capability と同期の確度（PORTAL-13）----

#[test]
fn new_surface_claims_nothing_ready() {
    let mut s = AppSurfaceSpec::native_direct("p", reaper());
    assert!(s.capabilities.iter().all(|c| !c.readiness.is_ready()), "未接続なのに Ready がある");
    assert_eq!(s.sync_mode, SyncMode::Unsupported);
    assert_eq!(s.status_line(), "Display: Checked when you open Connections · Control: Checked when you open Connections · State: Not supported");
    assert!(matches!(s.readiness(Layer::Input, Actor::Agent), Some(Readiness::Unsupported { .. })));
    assert_eq!(s.fallback_chain, vec![SurfaceBackend::NativeDirect, SurfaceBackend::ExternalHandoff]);
    // 表示だけ確かめた → ライブ画面。意味の同期は非対応のまま
    s.set_readiness(Layer::Visual, Actor::Human, Readiness::Ready { evidence: "e".into(), at_ms: 1.0 });
    assert_eq!(s.sync_mode, SyncMode::VisualLive);
    assert_eq!(s.status_line(), "Display: Confirmed · Control: Checked when you open Connections · State: Not supported");
    assert_eq!(s.last_verified, Some((1.0, "e".into())));
    s.set_readiness(Layer::Semantic, Actor::Human, Readiness::Ready { evidence: "rpp".into(), at_ms: 2.0 });
    assert_eq!(s.sync_mode, SyncMode::SemanticPolled);
    // serde の名前は §5.1 / §5.4 の綴り
    assert_eq!(serde_json::to_value(SurfaceBackend::WebView).unwrap(), json!("webview"));
    assert_eq!(serde_json::to_value(SurfaceBackend::ExternalHandoff).unwrap(), json!("external_handoff"));
    assert_eq!(serde_json::to_value(SyncMode::InferredUnverified).unwrap(), json!("inferred_unverified"));
}

#[test]
fn fallback_chain_ends_in_external_handoff() {
    let c = native_direct_chain();
    assert_eq!(next_fallback(&c, SurfaceBackend::NativeDirect), Some(SurfaceBackend::ExternalHandoff));
    assert_eq!(next_fallback(&c, SurfaceBackend::ExternalHandoff), None);
    assert_eq!(next_fallback(&c, SurfaceBackend::InteractiveMirror), None);
}

// ---- 窓を選ぶ・繋ぎ直す（PORTAL-01 / 11）----

#[test]
fn pick_main_window_never_guesses() {
    let main = window("a.rpp - REAPER", "AXStandardWindow");
    let mut palette = window("Mixer", "AXFloatingWindow");
    palette.main = false;
    assert_eq!(pick_main_window(&[palette.clone(), main.clone()]).unwrap().title, "a.rpp - REAPER");
    // main が無くても普通の窓が 1 つなら選ぶ
    let mut only = main.clone();
    only.main = false;
    assert!(pick_main_window(&[only.clone(), palette.clone()]).is_ok());
    // 普通の窓が 2 つで main が無い → 決めない
    let mut other = only.clone();
    other.handle = "w2".into();
    assert!(pick_main_window(&[only.clone(), other]).unwrap_err().contains("2 windows"));
    // palette だけ・最小化・全画面・動かせない → Err
    assert!(pick_main_window(&[palette]).is_err());
    let mut mini = main.clone();
    mini.minimized = true;
    assert!(pick_main_window(&[mini]).unwrap_err().contains("minimised"));
    let mut full = main.clone();
    full.fullscreen = true;
    assert!(pick_main_window(&[full]).unwrap_err().contains("full screen"));
    let mut fixed = main.clone();
    fixed.movable = false;
    assert!(pick_main_window(&[fixed.clone()]).unwrap_err().contains("Open in"));
    // 実測（REAPER 7.81 の起動直後）: 動かせない「About REAPER」が main・本体の窓は main でない → 本体を選ぶ
    let mut nag = fixed;
    nag.title = "About REAPER v7.81".into();
    let mut body = main;
    body.handle = "w2".into();
    body.main = false;
    body.title = "REAPER v7.81 - EVALUATION LICENSE".into();
    assert_eq!(pick_main_window(&[nag, body]).unwrap().handle, "w2");
}

#[test]
fn reconnect_only_to_the_same_titled_window() {
    let b = "com.cockos.reaper";
    let same = window("a.rpp - REAPER", "AXStandardWindow");
    assert_eq!(pick_reconnect(b, "a.rpp - REAPER", b, &[same.clone()]), Reconnect::One(same.clone()));
    // 別の app には繋がない
    assert!(matches!(pick_reconnect(b, "a.rpp - REAPER", "com.other", &[same.clone()]), Reconnect::None(_)));
    // 前の題名が空 → 決めない
    assert!(matches!(pick_reconnect(b, "", b, &[same.clone()]), Reconnect::Ask(_)));
    // 題名が違う窓しか無い → 繋がない（人に選ばせる）
    let other = window("b.rpp - REAPER", "AXStandardWindow");
    assert!(matches!(pick_reconnect(b, "a.rpp - REAPER", b, &[other]), Reconnect::Ask(_)));
    // 同じ題名が 2 つ → 決めない
    let mut twin = same.clone();
    twin.handle = "w2".into();
    assert!(matches!(pick_reconnect(b, "a.rpp - REAPER", b, &[same.clone(), twin]), Reconnect::Ask(_)));
    // 窓がまだ無い
    assert!(matches!(pick_reconnect(b, "a.rpp - REAPER", b, &[]), Reconnect::None(_)));
    // dialog だけ → まだ無い扱い
    assert!(matches!(pick_reconnect(b, "a.rpp - REAPER", b, &[window("a.rpp - REAPER", "AXDialog")]), Reconnect::None(_)));
}

// ---- 追従の部品 ----

#[test]
fn tracker_coalesces_and_detects_echo() {
    let mut t = Tracker::default();
    let t0 = Instant::now();
    let a = Frame { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
    let (first, since) = t.want(a, t0).expect("初回は送る");
    assert_eq!(first, a);
    t.sent(1, a, since);
    // 応答待ちの間の動きは覚えるだけ（最新だけ）
    let b = Frame { x: 10.0, ..a };
    let c = Frame { x: 20.0, ..a };
    assert!(t.want(b, t0 + Duration::from_millis(5)).is_none());
    assert!(t.want(c, t0 + Duration::from_millis(9)).is_none());
    let (got, sample) = t.replied(1, Some(Frame { x: 1.0, ..a }), t0 + Duration::from_millis(40)).unwrap();
    assert_eq!(got, a);
    let s = sample.unwrap();
    assert_eq!(s.error_px, 1.0);
    assert!((s.lag_ms - 40.0).abs() < 1.0);
    // 覚えたのは最新の c・変わった時刻は最初に覚えた時（5ms）
    let (q, qsince) = t.take_queued().unwrap();
    assert_eq!(q, c);
    assert_eq!(qsince, t0 + Duration::from_millis(5));
    // 違う id の応答は無視
    t.sent(2, c, qsince);
    assert!(t.replied(99, Some(c), t0).is_none());
    // echo: 送った target の 2pt 以内
    assert!(t.is_echo(&Frame { x: 21.5, ..a }));
    assert!(!t.is_echo(&Frame { x: 60.0, ..a }));
    // 同じ所（0.5pt 未満）は送らない
    t.replied(2, Some(c), t0);
    assert!(t.want(Frame { x: 20.3, ..a }, t0).is_none());
    assert!(t.want(Frame { x: 21.0, ..a }, t0).is_some());
}

#[test]
fn metrics_summary_is_measured_not_assumed() {
    let mut m = Metrics::default();
    assert_eq!(m.summary().n, 0);
    assert!(m.line().starts_with("0 measurements"));
    for (e, l) in [(0.0, 10.0), (1.0, 20.0), (0.0, 30.0), (3.0, 400.0)] {
        m.record(Sample { error_px: e, lag_ms: l });
    }
    let s = m.summary();
    assert_eq!((s.n, s.error_max, s.lag_max), (4, 3.0, 400.0));
    assert_eq!(s.lag_p50, 30.0);
    assert!(Frame { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }.error_px(&Frame { x: 0.0, y: -2.5, w: 11.0, h: 10.0 }) == 2.5);
}

// ---- bridge の行と client ----

#[test]
fn parse_line_separates_replies_events_and_garbage() {
    assert!(matches!(parse_line(r#"{"event":"moved","pid":1}"#), Msg::Event(_)));
    assert_eq!(parse_line(r#"{"id":3,"ok":true,"result":{"a":1}}"#), Msg::Reply { id: 3, result: Ok(json!({ "a": 1 })) });
    match parse_line(r#"{"id":4,"ok":false,"error":{"code":"ax_cannot_complete","message":"答えません"}}"#) {
        Msg::Reply { id: 4, result: Err(e) } => {
            assert_eq!(e.code(), "ax_cannot_complete");
            assert_eq!(e.to_string(), "答えません");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(parse_line(r#"{"id":null,"ok":false,"error":{"code":"bad_json","message":"x"}}"#), Msg::Broken(_)));
    assert!(matches!(parse_line("not json"), Msg::Broken(_)));
    assert!(matches!(parse_line(r#"{"id":1}"#), Msg::Broken(_)));
}

#[test]
fn find_bridge_says_where_it_looked() {
    std::env::set_var("KANGAE_SURFACE_BRIDGE", "/nonexistent/kangae-surface-bridge");
    let e = find_bridge().unwrap_err();
    std::env::remove_var("KANGAE_SURFACE_BRIDGE");
    assert_eq!(e.code(), "bridge_not_found");
    assert!(e.to_string().contains("/nonexistent/kangae-surface-bridge"));
}

#[test]
fn bridge_failures_are_explicit() {
    // 実行 file が無い
    let e = Bridge::spawn(&PathBuf::from("/nonexistent/bridge"), Duration::from_secs(2)).err().unwrap();
    assert_eq!(e.code(), "bridge_spawn");
    // ready を出さない → timeout
    let e = Bridge::spawn(&fake("noready", &[("FAKE_MODE", "noready")]), Duration::from_millis(1500)).err().unwrap();
    assert_eq!(e.code(), "timeout");
    // 普通: 往復・知らない cmd は明示の error
    let mut b = spawn("ok", &[]);
    assert_eq!(b.call("hello", json!({}), Duration::from_secs(20)).unwrap()["version"], "fake");
    assert_eq!(b.call("nope", json!({}), Duration::from_secs(20)).unwrap_err().code(), "unknown_cmd");
    // 応答が遅い → timeout（黙って成功にしない）
    let mut slow = spawn("slow", &[("FAKE_MODE", "slow"), ("FAKE_SLOW", "4")]);
    assert_eq!(slow.call("hello", json!({}), Duration::from_millis(800)).unwrap_err().code(), "timeout");
    // 壊れた行が混ざる → 応答は取れ、壊れた行は poll に Broken で出る
    let mut g = spawn("garbage", &[("FAKE_MODE", "garbage")]);
    assert!(g.call("hello", json!({}), Duration::from_secs(20)).is_ok());
    assert!(g.poll().iter().any(|m| matches!(m, Msg::Broken(s) if s.contains("not json"))));
    // 落ちる → Gone（stderr の最後の行つき）・以後の send も Gone
    let mut d = spawn("die", &[("FAKE_MODE", "die")]);
    let e = d.call("hello", json!({}), Duration::from_secs(20)).unwrap_err();
    assert_eq!(e.code(), "bridge_gone");
    assert!(e.to_string().contains("落ちます"), "{e}");
    assert_eq!(d.send("hello", json!({})).unwrap_err().code(), "bridge_gone");
    assert!(!d.alive());
}

// ---- Dock（偽の bridge で図 §18 の追従を通す）----

#[test]
fn dock_attaches_and_follows_the_slot() {
    let mut b = spawn("follow", &[("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(matches!(d.life, Lifecycle::Attaching | Lifecycle::Active), "{:?} {}", d.life, d.reason);
    assert_eq!(d.original, Some(Frame { x: 100.0, y: 100.0, w: 800.0, h: 600.0 }));
    // 枠に合い、前後で Kangae の窓より手前 → 表示が確認済み（操作・状態はまだ）
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0), "{:?} {}", d.life, d.reason);
    assert!(d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready());
    assert!(!d.spec.readiness(Layer::Input, Actor::Human).unwrap().is_ready());
    assert_eq!(d.spec.sync_mode, SyncMode::VisualLive);
    // Kangae が動く → 付いていく（PORTAL-02）
    assert!(pump(&mut d, &mut b, slot(160.0), |d| d.life == Lifecycle::Active && d.tracker.last_target.map(|t| t.x) == Some(160.0)));
    assert!(d.metrics.samples.len() >= 2);
    assert!(d.metrics.samples.iter().all(|s| s.error_px == 0.0));
    // Kangae が最小化 → hidden（元の窓には触らない）→ 戻ると active
    let hidden = Host { minimized: true, ..slot(160.0) };
    d.on_host(&mut b, hidden, Instant::now());
    assert_eq!(d.life, Lifecycle::Hidden);
    assert!(pump(&mut d, &mut b, slot(160.0), |d| d.life == Lifecycle::Active));
    assert!(!d.reason.contains("minimi"), "戻ったのに理由が最小化のまま: {}", d.reason);
    // 人が元の窓を外から動かした → 追従を止める（取り合わない）。自分の echo は数えない
    let echo = json!({ "event": "moved", "pid": 222, "handle": "w1", "frame": { "x": 160.0, "y": 120.0, "w": 800.0, "h": 500.0 } });
    d.on_msg(&mut b, Msg::Event(echo), Instant::now());
    assert_eq!(d.life, Lifecycle::Active);
    let human = json!({ "event": "moved", "pid": 222, "handle": "w1", "frame": { "x": 600.0, "y": 300.0, "w": 800.0, "h": 500.0 } });
    d.z_check(&mut b); // 外れる前に頼んだ前後の確認の応答が、外れた後に届く（実測 2026-10-18 で 表示: 確認済み のまま残った）
    d.on_msg(&mut b, Msg::Event(human), Instant::now());
    assert_eq!(d.life, Lifecycle::Degraded);
    assert_eq!(d.metrics.human_moves, 1);
    let checks = d.metrics.z_order_checks;
    assert!(pump(&mut d, &mut b, slot(160.0), |d| d.metrics.notes.iter().any(|n| n.contains("The window order check is skipped while degraded"))));
    assert_eq!(d.metrics.z_order_checks, checks);
    assert!(!d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready(), "外れたのに 表示: 確認済み");
    // degraded の間は送らない（負の対照: 枠を動かしても tracker は空のまま）
    d.on_host(&mut b, slot(300.0), Instant::now());
    assert!(d.tracker.waiting().is_none());
    // Kangae が全画面の間は合わせ直さない（別の Space）
    d.attach(&mut b, Host { fullscreen: true, ..slot(300.0) });
    assert_eq!(d.life, Lifecycle::Degraded);
    assert!(d.reason.contains("full screen"));
    // 合わせ直す（元の frame は初めて触った時の物のまま = 外で開くで戻る先が枠の位置にならない）
    d.attach(&mut b, slot(300.0));
    assert!(pump(&mut d, &mut b, slot(300.0), |d| d.life == Lifecycle::Active));
    assert_eq!(d.original, Some(Frame { x: 100.0, y: 100.0, w: 800.0, h: 600.0 }));
}

#[test]
fn dock_refuses_impossible_sizes_but_keeps_the_app_usable() {
    // PORTAL-05: app の最小の大きさより小さい枠 → degraded（窓は残る・外で開くで戻れる）
    let mut b = spawn("minsize", &[("FAKE_MIN_W", "900"), ("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Degraded), "{:?}", d.life);
    assert!(d.reason.contains("won't fit the frame"), "{}", d.reason);
    assert!(d.window.is_some());
    assert_eq!(d.metrics.samples.last().map(|s| s.error_px), Some(100.0));
    d.handoff(&mut b).unwrap();
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Detached), "{:?}", d.life);
    assert_eq!(d.spec.surface_backend, SurfaceBackend::ExternalHandoff);
}

#[test]
fn dock_without_permission_touches_nothing() {
    // PORTAL-10: 許可が無い → permission_required。窓の一覧も set_frame も送らない
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-perm.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("untrusted", &[("FAKE_MODE", "untrusted"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert_eq!(d.life, Lifecycle::PermissionRequired);
    assert!(d.reason.contains("Accessibility"));
    d.on_host(&mut b, slot(200.0), Instant::now());
    let sent = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(sent.contains("\"permission\""));
    for cmd in ["windows", "set_frame", "observe", "raise"] {
        assert!(!sent.contains(&format!("\"{cmd}\"")), "許可が無いのに {cmd} を送った: {sent}");
    }
    // やめる → closed（Kangae 本体はそのまま）
    assert_eq!(d.life.next(LifeEvent::Cancel), Ok(Lifecycle::Closed));
}

#[test]
fn dock_reconnects_only_to_the_same_window() {
    // PORTAL-11: 終了 → source_unavailable。再起動後に同じ題名の窓が出たら繋ぎ直す
    let mut b = spawn("reconnect", &[("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active));
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_terminated", "pid": 222, "bundle_id": "com.cockos.reaper" })), Instant::now());
    assert_eq!(d.life, Lifecycle::SourceUnavailable);
    assert!(d.window.is_none());
    assert!(!d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready());
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_launched", "pid": 222, "bundle_id": "com.cockos.reaper" })), Instant::now());
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active), "{:?} {}", d.life, d.reason);
    assert!(d.metrics.notes.iter().any(|n| n.contains("Reconnected")));

    // 再起動後の窓の題名が違う → 繋がない（source_unavailable のまま、理由を出す）
    let mut b2 = spawn("reconnect-other", &[("FAKE_TITLES", "a.rpp - REAPER|b.rpp - REAPER"), ("FAKE_HOST_PID", "4242")]);
    let mut d2 = Dock::new(reaper(), HOST_PID);
    d2.attach(&mut b2, slot(100.0));
    assert!(pump(&mut d2, &mut b2, slot(100.0), |d| d.life == Lifecycle::Active));
    d2.on_msg(&mut b2, Msg::Event(json!({ "event": "destroyed", "pid": 222, "handle": "w1" })), Instant::now());
    assert_eq!(d2.life, Lifecycle::SourceUnavailable);
    assert!(pump(&mut d2, &mut b2, slot(100.0), |d| d.reason.contains("b.rpp") || d.reason.contains("a.rpp")));
    assert_eq!(d2.life, Lifecycle::SourceUnavailable, "別の題名の窓に繋いだ: {}", d2.reason);
}

#[test]
fn raising_on_host_focus_is_counted_when_it_steals() {
    // Kangae が前に来た → AXRaise だけ（activate しない）。それでも元の app が前に来たら focus_steal_events。
    // 重ねた直後の activate（人が選んだ）は数えない
    let mut b = spawn("steal", &[("FAKE_STEAL", "1"), ("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active));
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_activated", "pid": HOST_PID, "bundle_id": "x" })), Instant::now());
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.metrics.focus_steal_events == 1));
    // 負の対照: 奪わない bridge では数えない
    let mut b2 = spawn("nosteal", &[("FAKE_HOST_PID", "4242")]);
    let mut d2 = Dock::new(reaper(), HOST_PID);
    d2.attach(&mut b2, slot(100.0));
    assert!(pump(&mut d2, &mut b2, slot(100.0), |d| d.life == Lifecycle::Active));
    let checks = d2.metrics.z_order_checks;
    d2.on_msg(&mut b2, Msg::Event(json!({ "event": "app_activated", "pid": HOST_PID, "bundle_id": "x" })), Instant::now());
    assert!(pump(&mut d2, &mut b2, slot(100.0), |d| d.metrics.z_order_checks > checks));
    assert_eq!(d2.metrics.focus_steal_events, 0);
    // 重ねた直後の自分の activate の結果（遅れて届いても）は人の操作に数えない。その次の 1 回が人
    let activated = json!({ "event": "app_activated", "pid": 222, "bundle_id": "com.cockos.reaper" });
    d2.on_msg(&mut b2, Msg::Event(activated.clone()), Instant::now());
    assert!(!d2.spec.readiness(Layer::Input, Actor::Human).unwrap().is_ready(), "自分の activate を人の操作に数えた");
    d2.on_msg(&mut b2, Msg::Event(activated), Instant::now());
    assert!(d2.spec.readiness(Layer::Input, Actor::Human).unwrap().is_ready());
    assert!(d2.lease.agent_may(AgentAction::Focus).is_err());
}

#[test]
fn z_check_finds_the_window_by_frame_when_attach_could_not() {
    // attach の時に CG の窓番号と対応が付かなくても、重ねた後の frame が一致する窓が 1 つなら前後を確かめる
    let mut b = spawn("nocg", &[("FAKE_NO_CG", "1"), ("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert_eq!(d.window.as_ref().and_then(|w| w.cg_id), None);
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.metrics.z_order_checks > 0), "{:?}", d.metrics.notes);
    assert_eq!(d.window.as_ref().and_then(|w| w.cg_id), Some(500));
    assert!(d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready());
}

#[test]
fn hole_shows_the_window_beneath_only_when_nothing_is_between() {
    // 実測（2026-10-18）: AXRaise は activate しない限り、前に居る app の窓より上に出ない。Kangae の枠が透明なら真下で見える
    let run = |name: &str, env: &[(&str, &str)], hole: bool| {
        let mut b = spawn(name, env);
        let mut d = Dock::new(reaper(), HOST_PID);
        d.host_has_hole = hole;
        d.attach(&mut b, slot(100.0));
        assert!(pump(&mut d, &mut b, slot(100.0), |d| d.metrics.z_order_checks > 0), "{:?}", d.metrics.notes);
        (d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready(), d.metrics.z_order_breaks)
    };
    let above = [("FAKE_HOST_ABOVE", "1"), ("FAKE_HOST_PID", "4242")];
    assert_eq!(run("above-nohole", &above, false), (false, 1));
    assert_eq!(run("above-hole", &above, true), (true, 0));
    let between = [("FAKE_HOST_ABOVE", "1"), ("FAKE_BETWEEN", "1"), ("FAKE_HOST_PID", "4242")];
    assert_eq!(run("above-hole-between", &between, true), (false, 1));
}

#[test]
fn a_tiling_window_manager_is_named_when_it_takes_the_window_back() {
    // 実測（2026-10-18）: AeroSpace が Kangae と元の窓を並べ直し、重ねた窓を取り返した → 外れた理由に名前と次の一手
    let apps = json!({ "apps": [{ "bundle_id": "com.google.Chrome" }, { "bundle_id": "bobko.aerospace" }] });
    assert_eq!(running_tiler(&apps).as_deref(), Some("AeroSpace"));
    assert_eq!(running_tiler(&json!({ "apps": [{ "bundle_id": "com.google.Chrome" }] })), None);
    let mut b = spawn("tiler", &[("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.tiler = Some("AeroSpace".into());
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active));
    d.on_msg(&mut b, Msg::Event(json!({ "event": "moved", "pid": 222, "handle": "w1", "frame": { "x": 420.0, "y": 25.0, "w": 420.0, "h": 958.0 } })), Instant::now());
    assert_eq!(d.life, Lifecycle::Degraded);
    assert!(d.reason.contains("set REAPER and Shibubu to floating in AeroSpace"), "{}", d.reason);
}

#[test]
fn l5_stick_a_frame_moved_while_waiting_is_sent_without_the_settle_wait() {
    // L5-STICK ②（owner 109 通目 10「動かす時、本物が遅れずに付いて来る」）: set_frame の応答を待つ間に枠がまた動き（drag）、その応答の読み戻しが
    // 途中の大きさだった（前に居る TextEdit の実測 = FAKE_LAGGED_SETS）→ 古い枠を SETTLE（0.5 秒）待って読み直さず、覚えた新しい枠をすぐ送る。
    // 負の対照: on_reply の `take_queued` の腕を外す → 応答を受けた直後に送っている物が無く（SETTLE 待ち）赤
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-l5-follow.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("l5-follow", &[("FAKE_HOST_PID", "4242"), ("FAKE_LAGGED_SETS", "1"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    // 1 つ目（x 100）の応答の前に枠が x 160 へ
    d.on_host(&mut b, slot(160.0), Instant::now());
    // 1 つ目の応答を受け取るまで bridge の物だけを渡す（on_host を呼ばない = 新しい枠を送れるのは応答を受けた側だけ）
    let deadline = Instant::now() + Duration::from_secs(30);
    while d.tracker.last_target.is_none() && Instant::now() < deadline {
        for m in b.poll() {
            d.on_msg(&mut b, m, Instant::now());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(d.tracker.last_target.map(|t| t.x), Some(100.0), "前提: 1 つ目の set_frame の応答が来ない");
    assert!(d.tracker.waiting().is_some(), "途中の大きさの読み戻しの後、覚えた新しい枠（x 160）を送らずに SETTLE を待った = 本物が 0.5 秒遅れて付いて来る");
    assert!(pump(&mut d, &mut b, slot(160.0), |d| d.life == Lifecycle::Active && d.tracker.last_target.map(|t| t.x) == Some(160.0)), "{:?} {}", d.life, d.reason);
    let xs: Vec<f64> = std::fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|v| v["cmd"] == "set_frame").filter_map(|v| v["x"].as_f64()).collect();
    assert_eq!(xs, vec![100.0, 160.0], "set_frame の順が違う（古い枠を送り直した・新しい枠を 2 回送った）");
}

#[test]
fn b33_a_frame_queued_before_shibubu_was_minimized_is_not_sent_while_hidden() {
    // B33（bfae19b2 の独立 review 黄・L5 の 10-31 の未 commit の試験を今の source に移した）: set_frame（x 160）の応答を待つ間に枠が動き（覚えた枠 x 220）、
    // 応答の前に Shibubu を最小化した（Hidden）→ 応答の読み戻しが途中の大きさでも、覚えた枠を送らない（Hidden の間は本物の窓に触らない = `frame_ok` と同じ確かめ）。
    // 負の対照（未測）: on_reply の `take_queued` の前の `self.life.touches_window()` を外す → Hidden のまま x 220 を送って赤
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-b33-hidden.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    // 途中の大きさの読み戻しは 1 つ目（重ねる時）と 2 つ目（x 160）
    let mut b = spawn("b33-hidden", &[("FAKE_HOST_PID", "4242"), ("FAKE_LAGGED_SETS", "2"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active), "前提: 重ならない {:?} {}", d.life, d.reason);
    d.on_host(&mut b, slot(160.0), Instant::now());
    d.on_host(&mut b, slot(220.0), Instant::now());
    d.on_host(&mut b, Host { minimized: true, ..slot(220.0) }, Instant::now());
    assert_eq!(d.life, Lifecycle::Hidden, "前提: 最小化で Hidden にならない");
    let deadline = Instant::now() + Duration::from_secs(30);
    while d.tracker.last_target.map(|t| t.x) != Some(160.0) && Instant::now() < deadline {
        for m in b.poll() {
            d.on_msg(&mut b, m, Instant::now());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(d.tracker.last_target.map(|t| t.x), Some(160.0), "前提: x 160 の set_frame の応答が来ない");
    // 送っていれば偽の bridge が log に書くまで待つ
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        for m in b.poll() {
            d.on_msg(&mut b, m, Instant::now());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let xs: Vec<f64> = std::fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|v| v["cmd"] == "set_frame").filter_map(|v| v["x"].as_f64()).collect();
    assert_eq!(xs, vec![100.0, 160.0], "Shibubu が最小化（Hidden）の間に、覚えた枠（x 220）で本物の窓を動かした");
    assert_eq!(d.life, Lifecycle::Hidden);
}

thread_local! {
    /// 偽の「Kangae の窓を真下に置く」口が受けた CG の窓番号（test の thread ごと）
    static ORDERED: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn fake_order(wid: u32) -> Result<(), String> {
    ORDERED.with(|o| o.borrow_mut().push(wid));
    Ok(())
}

fn failing_order(_wid: u32) -> Result<(), String> {
    Err("AppKit が読み込まれていません".into())
}

fn ordered() -> Vec<u32> {
    ORDERED.with(|o| o.borrow().clone())
}

#[test]
fn keep_front_puts_kangae_below_the_docked_window_without_activating() {
    // v0.14 P3 で採った公開 API の道: Kangae が前に来た・click・枠が動いた直後に、Kangae の窓を docked の窓（CG 500）の真下へ
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-keep.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("keep", &[("FAKE_HOST_PID", "4242"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.host_order = Some(fake_order);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0));
    let before = ordered().len();
    // Kangae が前に来た
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_activated", "pid": HOST_PID, "bundle_id": "x" })), Instant::now());
    assert_eq!(ordered()[before..], [500]);
    // click（kangae-portal の pointer の press が呼ぶ口）
    d.keep_front(&mut b);
    assert_eq!(ordered().len(), before + 2);
    // 枠が動いて合った直後
    assert!(pump(&mut d, &mut b, slot(160.0), |d| d.tracker.last_target.map(|t| t.x) == Some(160.0) && d.life == Lifecycle::Active));
    assert!(pump(&mut d, &mut b, slot(160.0), |_| ordered().len() >= before + 3), "枠が動いた後に真下へ置いていない");
    assert_eq!(d.metrics.order_failures, 0);
    assert_eq!(d.metrics.focus_steal_events, 0);
    // activate しない: keep_front は raise（activate:true）も open_app も送らない
    let sent = std::fs::read_to_string(&log).unwrap();
    let raises_with_activate = sent.lines().filter(|l| l.contains("\"raise\"") && l.contains("\"activate\":true")).count();
    assert_eq!(raises_with_activate, 1, "重ねた直後の 1 回（人が選んだ）だけ: {sent}");
    assert!(!sent.lines().any(|l| l.contains("\"raise\"") && l.contains("\"activate\":false")), "口が在る時は AXRaise の道を使わない: {sent}");
}

#[test]
fn keep_front_is_not_called_when_the_docked_window_is_gone_or_elsewhere() {
    // 別の Space・最小化: 前後の一覧（今の画面）に docked の窓が無い → Kangae の窓を動かさない
    let mut b = spawn("keep-hidden", &[("FAKE_HOST_PID", "4242"), ("FAKE_HIDE_DOCKED", "1")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.host_order = Some(fake_order);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_breaks > 0), "{:?}", d.metrics.notes);
    let n = ordered().len();
    d.keep_front(&mut b);
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_activated", "pid": HOST_PID, "bundle_id": "x" })), Instant::now());
    assert_eq!(ordered().len(), n, "docked の窓が今の画面に無いのに Kangae の窓を動かした");
    // 負の対照: 見えている bridge なら動かす
    let mut b2 = spawn("keep-visible", &[("FAKE_HOST_PID", "4242")]);
    let mut d2 = Dock::new(reaper(), HOST_PID);
    d2.host_order = Some(fake_order);
    d2.attach(&mut b2, slot(100.0));
    assert!(pump(&mut d2, &mut b2, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0));
    let n2 = ordered().len();
    d2.keep_front(&mut b2);
    assert_eq!(ordered().len(), n2 + 1);
    // 窓が消えた（source_unavailable）→ 動かさない
    d2.on_msg(&mut b2, Msg::Event(json!({ "event": "destroyed", "pid": 222, "handle": "w1" })), Instant::now());
    d2.keep_front(&mut b2);
    assert_eq!(ordered().len(), n2 + 1);
}

#[test]
fn keep_front_falls_back_to_ax_raise_when_the_order_fails() {
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-keepfail.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("keep-fail", &[("FAKE_HOST_PID", "4242"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.host_order = Some(failing_order);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0));
    d.keep_front(&mut b);
    assert_eq!(d.metrics.order_failures, 1);
    assert!(d.metrics.notes.iter().any(|n| n.contains("Raising the window instead")), "{:?}", d.metrics.notes);
    assert!(pump(&mut d, &mut b, slot(100.0), |_| std::fs::read_to_string(&log).unwrap_or_default().lines().any(|l| l.contains("\"raise\"") && l.contains("\"activate\":false"))));
}

#[test]
fn l5_stick_two_mirrors_stay_behind_shibubu_when_shibubu_and_then_another_app_come_front() {
    // L5-STICK ①（operator の音楽の通し 10-31・写真 12〜16）: Shibubu が前・Mirror が 2 つ（REAPER と Music の本物を Shibubu の下にしまう）・
    // 間にもう 1 つの窓（もう一方の本物・REAPER の本体の窓 = FAKE_BETWEEN の 700）。Shibubu が前に来た → 別の app（333）が前に来た → 本物は両方とも
    // Shibubu の後ろのまま（Shibubu を動かさない・AXRaise しない）。前は keep_front が Mirror ごとに無条件で Shibubu を本物の直上の段へ移し、
    // 下の本物の直上へ沈んで、間の窓と上の本物が Shibubu の上に出た。
    // 負の対照: on_reply の Pending::Tuck の `d < h` の確かめを外す（常に動かす）→ order が 2 回以上で赤
    let scene = [("FAKE_HOST_ABOVE", "1"), ("FAKE_BETWEEN", "1"), ("FAKE_HOST_PID", "4242")];
    let start = |name: &str, env: &[(&str, &str)]| {
        let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-{name}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let env: Vec<(&str, &str)> = env.iter().copied().chain([("FAKE_LOG", log.to_str().unwrap())]).collect();
        let mut b = spawn(name, &env);
        let mut d = Dock::new(reaper(), HOST_PID);
        d.host_order = Some(fake_order);
        (d.tuck, d.front_on_attach) = (true, false);
        d.attach(&mut b, slot(100.0));
        assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0), "{name}: {:?}", d.metrics.notes);
        (d, b, log)
    };
    let raises = |log: &PathBuf| std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.contains("\"raise\"")).count();
    // 遅れて着く読みの答え（混んだ機械）を受け取ってから数える
    let settle = |d: &mut Dock, b: &mut Bridge| {
        std::thread::sleep(Duration::from_millis(300));
        for m in b.poll() {
            d.on_msg(b, m, Instant::now());
        }
    };
    let mut mirrors = [start("l5-two-a", &scene[..]), start("l5-two-b", &scene[..])];
    for (d, b, _) in mirrors.iter_mut() {
        settle(d, b);
    }
    let n = ordered().len();
    for pid in [HOST_PID, 333] {
        for (d, b, _) in mirrors.iter_mut() {
            let checks = d.metrics.z_order_checks;
            d.on_msg(b, Msg::Event(json!({ "event": "app_activated", "pid": pid, "bundle_id": "x" })), Instant::now());
            if pid == HOST_PID {
                assert!(pump(d, b, slot(100.0), |d| d.metrics.z_order_checks > checks), "Shibubu が前に来た後に前後を読まない: {:?}", d.metrics.notes);
            }
        }
    }
    // 別の app が前に来た後に送った物があれば bridge の log に出るまで待つ
    for (d, b, _) in mirrors.iter_mut() {
        settle(d, b);
    }
    for (d, _, log) in &mirrors {
        assert_eq!(raises(log), 0, "しまっている本物を AXRaise で Shibubu の上へ出した");
        assert_eq!(d.metrics.z_order_breaks, 0, "{:?}", d.metrics.notes);
        assert!(d.spec.readiness(Layer::Visual, Actor::Human).unwrap().is_ready(), "本物が Shibubu の後ろに居るのに確認済みにしない");
    }
    assert_eq!(ordered().len(), n, "Shibubu が既に本物より手前なのに Shibubu を本物の直上の段へ動かした（Mirror が 2 つなら下の本物の直上へ沈む）");
    // 本物が Shibubu より手前（取り残された）なら、その本物の真上へ Shibubu を 1 回だけ動かす
    let (mut d, mut b, log) = start("l5-two-front", &[("FAKE_HOST_PID", "4242")][..]);
    settle(&mut d, &mut b);
    let n = ordered().len();
    d.on_msg(&mut b, Msg::Event(json!({ "event": "app_activated", "pid": HOST_PID, "bundle_id": "x" })), Instant::now());
    assert!(pump(&mut d, &mut b, slot(100.0), |_| ordered().len() > n), "Shibubu の上に残った本物の真上へ Shibubu を動かさない: {:?}", d.metrics.notes);
    settle(&mut d, &mut b);
    assert_eq!(ordered()[n..], [500], "本物の真上へ Shibubu を 1 回だけでなく動かした");
    assert_eq!(raises(&log), 0);
}

#[test]
fn l5_stick_a_window_order_read_that_lands_after_tuck_ends_moves_nothing() {
    // L5-STICK ①: しまっている間（tuck）に頼んだ前後の読みの答えが、直接触る間（tuck = false・本物の app が前に来た lift）へ替わった後に着いた →
    // その古い読みで Shibubu を動かさない（lift で Shibubu を本物の真下へ置いた後に、古い読みが Shibubu を本物の真上へ戻すと本物が後ろへ沈む）。
    // 偽の bridge は本物が Shibubu より手前（FAKE_HOST_ABOVE 無し）= tuck の間なら Shibubu を動かす場面。答えは必ず tuck を外した後に受ける（時間に頼らない）
    // 負の対照（m12）: on_reply の Pending::Tuck の `self.tuck` を常に true に → 古い読みで order して赤
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-l5-late-tuck.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("l5-late-tuck", &[("FAKE_HOST_PID", "4242"), ("FAKE_LOG", log.to_str().unwrap())]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.host_order = Some(fake_order);
    (d.tuck, d.front_on_attach) = (true, false);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active && d.metrics.z_order_checks > 0), "{:?}", d.metrics.notes);
    let reads = |log: &PathBuf| std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.contains("\"cg_windows\"")).count();
    // attach の間に頼んだ読みと order の後の読みを全部受け取ってから数える
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet = 0;
    while quiet < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        let msgs = b.poll();
        quiet = if msgs.is_empty() { quiet + 1 } else { 0 };
        for m in msgs {
            d.on_msg(&mut b, m, Instant::now());
        }
    }
    let (n, asked) = (ordered().len(), reads(&log));
    // 読みを頼む（keep_front の tuck の道）→ 答えを受ける前に直接触る間へ
    d.keep_front(&mut b);
    d.tuck = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while reads(&log) == asked && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(reads(&log) > asked, "前提: keep_front が前後を読まない");
    std::thread::sleep(Duration::from_millis(300));
    for m in b.poll() {
        d.on_msg(&mut b, m, Instant::now());
    }
    assert_eq!(ordered().len(), n, "直接触る間へ替わった後に着いた古い前後の読みで Shibubu を動かした");
}

#[test]
fn order_host_below_says_why_without_appkit() {
    // test の process には AppKit の窓が無い: 黙って成功にしない
    let e = order_host_below(500).unwrap_err();
    assert!(e.contains("AppKit") || e.contains("窓") || e.contains("macOS"), "{e}");
}

#[test]
fn handoff_waits_for_the_drag_to_end() {
    let mut b = spawn("drag", &[("FAKE_BUTTONS", "1"), ("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active));
    assert!(d.handoff(&mut b).unwrap_err().contains("drag"));
    assert_eq!(d.life, Lifecycle::Active);
}

#[test]
fn close_puts_the_window_back() {
    let log = std::env::temp_dir().join(format!("kangae-portal-test-{}-close.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut b = spawn("close", &[("FAKE_LOG", log.to_str().unwrap()), ("FAKE_HOST_PID", "4242")]);
    let mut d = Dock::new(reaper(), HOST_PID);
    d.attach(&mut b, slot(100.0));
    assert!(pump(&mut d, &mut b, slot(100.0), |d| d.life == Lifecycle::Active));
    d.close(&mut b);
    assert_eq!(d.life, Lifecycle::Closed);
    let last_set: Value = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| l.contains("\"set_frame\""))
        .last()
        .map(|l| serde_json::from_str(l).unwrap())
        .unwrap();
    assert_eq!((last_set["x"].as_f64(), last_set["w"].as_f64()), (Some(100.0), Some(800.0)));
    assert_eq!((last_set["y"].as_f64(), last_set["h"].as_f64()), (Some(100.0), Some(600.0)));
}

// ---- 証拠（PORTAL-14）----

const RPP: &str = r#"<REAPER_PROJECT 0.1 "7.81/macOS-x86_64" 1760000000
  RIPPLE 0
  <TRACK {AAAA}
    NAME "Drums"
    <ITEM
      POSITION 1.5
      LENGTH 2
      IGUID {I-1}
      NAME 'take "one"'
      <SOURCE WAVE
        FILE "/x/a.wav"
      >
    >
  >
  <TRACK {BBBB}
    NAME Bass
    <ITEM
      POSITION 0
      LENGTH 4.25
      IGUID {I-2}
      NAME bass
    >
  >
>
"#;

#[test]
fn rpp_items_read_the_saved_file() {
    let items = rpp_items(RPP);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], RppItem { guid: "{I-1}".into(), track: "Drums".into(), take: "take \"one\"".into(), position: 1.5, length: 2.0 });
    assert_eq!((items[1].track.as_str(), items[1].take.as_str(), items[1].length), ("Bass", "bass", 4.25));
}

#[test]
fn diff_items_names_what_changed() {
    let before = rpp_items(RPP);
    let after = rpp_items(&RPP.replace("POSITION 1.5", "POSITION 2.75").replace("IGUID {I-2}", "IGUID {I-3}"));
    let d = diff_items(&before, &after);
    assert!(d.iter().any(|l| l.starts_with("Moved") && l.contains("from 1.500 to 2.750")), "{d:?}");
    assert!(d.iter().any(|l| l.starts_with("Added") && l.contains("{I-3}")));
    assert!(d.iter().any(|l| l.starts_with("Removed") && l.contains("{I-2}")));
    assert!(diff_items(&before, &before).is_empty());
}

#[test]
fn reaper_state_lines() {
    let s = parse_reaper_state("proj\t/x/t.rpp\t1\tMove media items\nitem\t{I-1}\t2.5\t1\ttake\tDrums").unwrap();
    assert_eq!((s.project.as_str(), s.dirty, s.last_undo.as_str()), ("/x/t.rpp", true, "Move media items"));
    assert_eq!(s.items[0].position, 2.5);
    assert!(!parse_reaper_state("proj\t\t0\t").unwrap().dirty);
    assert!(parse_reaper_state("err\tboom").unwrap_err().contains("boom"));
    assert!(parse_reaper_state("garbage").is_err());
    assert!(parse_reaper_state("proj\t/x\t0\t\nitem\tbad").is_err());
}

// ---- WebView の面（判定の純関数・v0.14 §3.1・CD-24〜26。WebKit を使わない）----

#[test]
fn web_store_is_split_per_space_and_app_and_stable() {
    let a = store_identifier("default", "claude.ai");
    assert_eq!(a, store_identifier("default", "claude.ai"), "同じ組は毎回同じ（再起動で残る）");
    assert_ne!(a, store_identifier("default", "youtube"), "別の app は別の store");
    assert_ne!(a, store_identifier("work", "claude.ai"), "別の space は別の store");
    assert_ne!(store_identifier("ab", "c"), store_identifier("a", "bc"), "区切りがずれても混ざらない");
}

#[test]
fn youtube_embed_only_takes_a_video_id() {
    assert_eq!(youtube_embed_url("M7lc1UVf-VE").unwrap(), "https://www.youtube.com/embed/M7lc1UVf-VE");
    for bad in ["", "short", "M7lc1UVf-VE/../x", "M7lc1UVf-V?", "https://evil"] {
        assert!(youtube_embed_url(bad).is_err(), "{bad}");
    }
    assert_eq!(KANGAE_REFERER, "https://app.kangae.desktop");
}

#[test]
fn navigation_is_allowlisted_and_sign_in_goes_to_the_browser() {
    let allow = vec!["youtube.com".to_string(), "example.com".to_string()];
    assert_eq!(host_of("https://Www.YouTube.com:443/embed/x?y#z").as_deref(), Some("www.youtube.com"));
    assert_eq!(decide_navigation("https://www.youtube.com/embed/x", &allow), WebNav::Allow);
    assert_eq!(decide_navigation("https://example.com/", &allow), WebNav::Allow);
    assert_eq!(decide_navigation("about:blank", &allow), WebNav::Allow, "sub-frame の空の頁");
    // sign-in / OAuth は許可の host でも handoff（CD-25 (6)）
    for url in ["https://accounts.google.com/v3/signin/identifier", "https://example.com/o/oauth2/auth?x", "https://appleid.apple.com/auth"] {
        assert!(matches!(decide_navigation(url, &allow), WebNav::Deny { handoff: true, .. }), "{url}");
    }
    // 許可の外・https でない・似た名前の別 host は止める（面ごとの handoff はしない: sub-frame かもしれない）
    for url in ["https://evil.com/", "http://example.com/", "https://notexample.com/", "https://example.com.evil.com/", "file:///etc/passwd", "javascript:alert(1)"] {
        assert!(matches!(decide_navigation(url, &allow), WebNav::Deny { handoff: false, .. }), "{url}");
    }
    assert_eq!(decide_navigation("http://127.0.0.1:47833/store.html", &["127.0.0.1".to_string()]), WebNav::Allow, "自前の頁だけ http を許す");
}

#[test]
fn w12wb_bar_input_to_url_and_empty_allow_hosts_keep_https_and_sign_in_rules() {
    assert_eq!(bar_input_to_url("https://www.youtube.com/watch?v=x").as_deref(), Some("https://www.youtube.com/watch?v=x"));
    assert_eq!(bar_input_to_url("example.com").as_deref(), Some("https://example.com"));
    assert_eq!(bar_input_to_url("  猫 動画  ").as_deref(), Some("https://www.google.com/search?q=%E7%8C%AB+%E5%8B%95%E7%94%BB"));
    assert_eq!(bar_input_to_url("   "), None, "空は送らない");
    let none: &[String] = &[];
    assert_eq!(decide_navigation("https://www.youtube.com/watch?v=x", none), WebNav::Allow);
    assert_eq!(decide_navigation("https://excalidraw.com/", none), WebNav::Allow);
    assert!(matches!(decide_navigation("http://example.com/", none), WebNav::Deny { handoff: false, .. }), "allowlist を外しても http は止まる");
    assert!(matches!(decide_navigation("https://accounts.google.com/signin", none), WebNav::Deny { handoff: true, .. }), "sign-in は本人の browser へ");
    assert_eq!(decide_navigation("http://127.0.0.1/", none), WebNav::Allow);
}

#[test]
fn w12wb_persistable_url_only_when_live_and_different() {
    assert_eq!(persistable_url("https://a/", "https://a/", true), None, "同じ URL は毎 frame 書かない");
    assert_eq!(persistable_url("https://a/", "", true), None, "空は書かない");
    assert_eq!(persistable_url("https://a/", "https://b/", false), None, "Live になる前は書かない");
    assert_eq!(persistable_url("https://a/", "https://b/", true).as_deref(), Some("https://b/"));
}

#[test]
fn integ3c_persistable_url_ignores_trailing_slash_and_scheme_case() {
    // INTEG-3b: Diagram を開いた 3.6 秒後に rev 3（url の差は末尾の / だけ）が ↶ の対象として書かれた
    assert_eq!(persistable_url("https://excalidraw.com", "https://excalidraw.com/", true), None, "WKWebView が足す末尾の / は変更ではない");
    assert_eq!(persistable_url("https://excalidraw.com/", "https://excalidraw.com", true), None, "逆向きも同じ");
    assert_eq!(persistable_url("HTTPS://excalidraw.com", "https://excalidraw.com/", true), None, "scheme の大文字小文字も同じ");
}

#[test]
fn integ3c_persistable_url_still_writes_a_real_move() {
    assert_eq!(
        persistable_url("https://excalidraw.com", "https://excalidraw.com/#json=1", true).as_deref(),
        Some("https://excalidraw.com/#json=1"),
        "末尾の / を揃えても、本当に違う URL は書く"
    );
    assert_eq!(persistable_url("https://excalidraw.com", "https://excalidraw.com/a", true).as_deref(), Some("https://excalidraw.com/a"));
    assert_eq!(persistable_url("https://excalidraw.com", "https://excalidraw.com/", false), None, "Live になる前は書かない");
}

#[test]
fn web_life_follows_the_web_stage_of_diagram_18() {
    let t0 = Instant::now();
    let at = |s: u64| t0 + Duration::from_secs(s);
    // 読み込み → 直接操作
    let mut w = WebLife::new("https://example.com/", false, t0);
    assert_eq!(w.state, WebState::Starting);
    w.on(0, WebEvent::Started("https://example.com/".into()), at(0));
    w.on(0, WebEvent::Finished("https://example.com/".into()), at(1));
    assert_eq!(w.state, WebState::Live);
    // 許可の外の読み込み: 止めて帯に 1 行（handoff しない）
    w.on(0, WebEvent::Blocked { url: "https://ads.example.net/".into(), handoff: false, why: "ads.example.net は開きません".into() }, at(2));
    assert_eq!(w.state, WebState::Live);
    assert!(w.detail.as_deref().unwrap().contains("ads.example.net"));
    // 新しい窓 → handoff（その URL）
    w.on(0, WebEvent::NewWindow("https://example.com/popup".into()), at(3));
    assert!(matches!(w.state, WebState::Recoverable(_)));
    assert_eq!(w.handoff_url.as_deref(), Some("https://example.com/popup"));
    // 読み込みが始まらない: 作った時から 20 秒では諦めない（W12W-1b・WebContent の起動に 35 秒）・WEB_START_LIMIT 60 秒で Recoverable
    let mut slow = WebLife::new("https://example.com/", false, t0);
    slow.tick(at(21));
    assert_eq!(slow.state, WebState::Starting);
    slow.tick(at(61));
    assert!(matches!(&slow.state, WebState::Recoverable(why) if why.contains("hadn't started")));
    // 始まったが finished が来ない重い頁: 題名が出れば使える（実測: load 200 の YouTube）。何も出ずに 60 秒なら handoff
    let mut heavy = WebLife::new("https://example.com/", false, t0);
    heavy.on(0, WebEvent::Started("https://example.com/".into()), at(1));
    heavy.tick(at(30));
    assert_eq!(heavy.state, WebState::Loading);
    heavy.on(0, WebEvent::Title("Example Domain".into()), at(31));
    assert_eq!(heavy.state, WebState::Live);
    let mut stuck = WebLife::new("https://example.com/", false, t0);
    stuck.on(0, WebEvent::Started("https://example.com/".into()), at(1));
    stuck.tick(at(61));
    assert_eq!(stuck.state, WebState::Loading, "起点は navigation の開始（Started の 1 秒）・作った時ではない");
    stuck.tick(at(62));
    assert!(matches!(&stuck.state, WebState::Recoverable(why) if why.contains("60 s")));
    // YouTube: 10 秒たっても題名が「YouTube」のまま = 153 → handoff。動画の題名に変われば再生（負の対照）
    let mut yt = WebLife::new("https://www.youtube.com/embed/M7lc1UVf-VE", true, t0);
    yt.on(0, WebEvent::Finished("https://www.youtube.com/embed/M7lc1UVf-VE".into()), at(1));
    yt.on(0, WebEvent::Title("YouTube".into()), at(1));
    yt.tick(at(5));
    assert_eq!(yt.state, WebState::Live);
    yt.tick(at(12));
    assert!(matches!(&yt.state, WebState::Recoverable(why) if why.contains("153")));
    let mut ok = WebLife::new("https://www.youtube.com/embed/M7lc1UVf-VE", true, t0);
    ok.on(0, WebEvent::Finished("https://www.youtube.com/embed/M7lc1UVf-VE".into()), at(1));
    ok.on(0, WebEvent::Title("YouTube Developers Live: Embedded Web Player Customization - YouTube".into()), at(3));
    ok.tick(at(30));
    assert_eq!(ok.state, WebState::Live);
    // finished が来ないまま: 「YouTube」→ 動画の題名 = 再生できる / 「YouTube」のまま 10 秒 = 153（実測の 2 本）
    let mut yt2 = WebLife::new("https://www.youtube.com/embed/M7lc1UVf-VE", true, t0);
    yt2.on(0, WebEvent::Started("https://www.youtube.com/embed/M7lc1UVf-VE".into()), at(1));
    yt2.on(0, WebEvent::Title("YouTube".into()), at(9));
    assert_eq!(yt2.state, WebState::Loading, "「YouTube」だけでは使えるとみなさない");
    yt2.on(0, WebEvent::Title("YouTube Developers Live: Embedded Web Player Customization - YouTube".into()), at(19));
    yt2.tick(at(40));
    assert_eq!(yt2.state, WebState::Live);
    let mut yt3 = WebLife::new("https://www.youtube.com/embed/M7lc1UVf-VE", true, t0);
    yt3.on(0, WebEvent::Started("https://www.youtube.com/embed/M7lc1UVf-VE".into()), at(1));
    yt3.on(0, WebEvent::Title("YouTube".into()), at(13));
    yt3.tick(at(22));
    assert_eq!(yt3.state, WebState::Loading);
    yt3.tick(at(24));
    assert!(matches!(&yt3.state, WebState::Recoverable(why) if why.contains("153")));
    // WebContent process: 1 回目は reload・2 回目で終了
    let mut g = WebLife::new("https://example.com/", false, t0);
    g.on(0, WebEvent::Finished("https://example.com/".into()), at(1));
    g.on(0, WebEvent::ProcessGone, at(2));
    assert!(g.want_reload && g.state == WebState::Starting, "process の終わった面は空白（Live と言わない）");
    g.on(0, WebEvent::ProcessGone, at(3));
    assert!(matches!(&g.state, WebState::Recoverable(why) if why.contains("twice")), "2 回目でも終端にしない");
    // sign-in の navigation → handoff（その URL）
    let mut s = WebLife::new("https://claude.ai/", false, t0);
    s.on(0, WebEvent::Blocked { url: "https://accounts.google.com/x".into(), handoff: true, why: "sign-in".into() }, at(1));
    assert_eq!(s.handoff_url.as_deref(), Some("https://accounts.google.com/x"));
}

// W12W-1b の契約（YC_WAVES §5.3 の試験 ①〜④ と、止まり・試験の失敗の手）。時計は Instant を足して注入する。本体（retry・Progress・起点）までは赤。
const XL: &str = "https://excalidraw.com/";

fn ms(t0: Instant, m: u64) -> Instant {
    t0 + Duration::from_millis(m)
}

#[test]
fn w12w1b_slow_webkit_start_waits_and_goes_live() {
    // ① WebContent の起動に 35 秒: WebKit は load を頼んだ直後（pending の API request）から loading = true・progress = 0.1 を返す
    // （PageLoadState.cpp の estimatedProgress・initialProgressValue）。それを navigation の開始と数えず Starting のまま・20 秒で諦めない
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    for s in [0, 10, 21, 30, 34] {
        w.on(0, WebEvent::Progress { loading: true, progress: 0.1 }, ms(t0, s * 1000));
        w.tick(ms(t0, s * 1000));
    }
    assert_eq!(w.state, WebState::Starting, "作った時から 20 秒を過ぎても Recoverable にしない（起点を作った時に戻す・pending の 0.1 を開始と数えると赤）");
    w.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 35_000));
    w.tick(ms(t0, 35_000));
    assert_eq!(w.state, WebState::Loading);
    w.on(0, WebEvent::Started(XL.into()), ms(t0, 40_000));
    w.on(0, WebEvent::Finished(XL.into()), ms(t0, 50_000));
    w.tick(ms(t0, 70_000));
    assert_eq!(w.state, WebState::Live, "作った時から 60 秒を過ぎても navigation の開始から数える");
}

#[test]
fn w12w1b_retry_leaves_recoverable() {
    // ② 何も始まらず WEB_START_LIMIT → Recoverable → retry → Starting（起点は retry の時）→ Live
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    w.tick(ms(t0, (WEB_START_LIMIT.as_secs() + 1) * 1000));
    assert!(matches!(&w.state, WebState::Recoverable(why) if why.contains("hadn't started")), "{:?}", w.state);
    assert_eq!(w.handoff_url.as_deref(), Some(XL), "「browser で開く」の URL");
    w.on(0, WebEvent::Finished(XL.into()), ms(t0, 62_000));
    assert!(matches!(w.state, WebState::Recoverable(_)), "同じ generation の遅れた Finished では抜けない");
    let g = w.retry(ms(t0, 70_000));
    assert_eq!((g, w.generation()), (1, 1));
    assert_eq!(w.state, WebState::Starting, "Recoverable を終端に戻すと赤");
    w.tick(ms(t0, 100_000));
    assert_eq!(w.state, WebState::Starting, "上限の起点は retry の時（30 秒しかたっていない）");
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 101_000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 105_000));
    assert_eq!(w.state, WebState::Live);
}

#[test]
fn w12w1b_events_of_an_older_generation_are_dropped() {
    // ③ retry の前の読み込みの Finished が、新しい読み込みの状態を上書きしない
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    w.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 1000));
    let g = w.retry(ms(t0, 2000));
    w.on(0, WebEvent::Finished(XL.into()), ms(t0, 3000));
    w.on(0, WebEvent::Title("Excalidraw".into()), ms(t0, 3000));
    assert_eq!(w.state, WebState::Starting, "前の generation の Finished / 題名を捨てる（generation を見ないと赤）");
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 4000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 5000));
    assert_eq!(w.state, WebState::Live);
}

#[test]
fn w12w1b_process_end_then_retry_goes_live_even_the_second_time() {
    // ④ process の終了 → retry → Live。2 回目の終了は Recoverable（Gone で終わらない）→ もう一度 → Live
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    w.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 1000));
    w.on(0, WebEvent::Finished(XL.into()), ms(t0, 2000));
    w.on(0, WebEvent::ProcessGone, ms(t0, 3000));
    assert!(w.want_reload, "1 回目は自動で読み直す");
    w.want_reload = false; // WebSurface::frame がする事
    let g = w.retry(ms(t0, 3000));
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 4000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 5000));
    assert_eq!(w.state, WebState::Live);
    w.on(g, WebEvent::ProcessGone, ms(t0, 6000));
    assert!(matches!(w.state, WebState::Recoverable(_)) && !w.want_reload, "2 回目は人の「もう一度」を待つ");
    let g = w.retry(ms(t0, 7000));
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 8000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 9000));
    assert_eq!(w.state, WebState::Live);
}

#[test]
fn w12w1b_a_face_whose_process_ended_is_not_live() {
    // holdout の FAIL: 1 回目の process の終了の後も Live のままだった（WebView の中は空白・読み直すのは WebSurface::frame）。
    // WebLife だけで Starting（同じ generation・時計の起点は終了の時）に戻し、2 回目の終了は読み直しの前でも Recoverable
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    w.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 1000));
    w.on(0, WebEvent::Finished(XL.into()), ms(t0, 2000));
    w.on(0, WebEvent::Progress { loading: false, progress: 1.0 }, ms(t0, 3000));
    w.on(0, WebEvent::ProcessGone, ms(t0, 10_000));
    w.tick(ms(t0, 10_000));
    assert_eq!((&w.state, w.want_reload, w.generation()), (&WebState::Starting, true, 0), "Live のままだと赤");
    w.tick(ms(t0, 10_000 + WEB_START_LIMIT.as_millis() as u64 - 1000));
    assert_eq!(w.state, WebState::Starting, "上限の起点は終了の時（作った時のままだと赤）");
    w.on(0, WebEvent::ProcessGone, ms(t0, 75_000));
    assert!(matches!(&w.state, WebState::Recoverable(why) if why.contains("twice")), "{:?}", w.state);
    let g = w.retry(ms(t0, 76_000));
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 77_000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 78_000));
    assert_eq!(w.state, WebState::Live);
}

#[test]
fn w12w1b_load_that_stops_without_finish_is_recoverable_after_the_grace() {
    let t0 = Instant::now();
    let grace = WEB_STOP_GRACE.as_millis() as u64;
    // 止まって 1 frame 後に Finished が着く: Live（猶予の中）
    let mut a = WebLife::new(XL, false, t0);
    a.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 1000));
    a.on(0, WebEvent::Progress { loading: false, progress: 0.9 }, ms(t0, 2000));
    a.tick(ms(t0, 2000));
    a.on(0, WebEvent::Finished(XL.into()), ms(t0, 2016));
    a.tick(ms(t0, 2000 + grace * 2));
    assert_eq!(a.state, WebState::Live);
    a.on(0, WebEvent::Progress { loading: false, progress: 1.0 }, ms(t0, 5000));
    a.tick(ms(t0, 5000 + grace * 4));
    assert_eq!(a.state, WebState::Live, "finish の後の loading=false・progress 1.0 は止まりではない");
    // 止まったまま Finished も題名も無い: 猶予の間は Loading・過ぎたら Recoverable（wry が didFail を渡さない代わり）
    let mut b = WebLife::new(XL, false, t0);
    b.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 1000));
    b.on(0, WebEvent::Progress { loading: false, progress: 0.3 }, ms(t0, 2000));
    b.tick(ms(t0, 2000 + grace / 2));
    assert_eq!(b.state, WebState::Loading);
    b.on(0, WebEvent::Progress { loading: false, progress: 0.3 }, ms(t0, 2000 + grace + 100));
    b.tick(ms(t0, 2000 + grace + 100));
    assert!(matches!(&b.state, WebState::Recoverable(why) if why.contains("stopped partway")), "{:?}", b.state);
    // provisional のうちに止まる（DNS・policy で失敗）: progress が 0.1 より上に行かなくても、Starting で 60 秒待たずに猶予の後 Recoverable
    let mut c = WebLife::new(XL, false, t0);
    c.on(0, WebEvent::Progress { loading: true, progress: 0.1 }, ms(t0, 0));
    c.on(0, WebEvent::Progress { loading: false, progress: 0.1 }, ms(t0, 500));
    c.tick(ms(t0, 500 + grace + 100));
    assert!(matches!(&c.state, WebState::Recoverable(why) if why.contains("stopped partway")), "{:?}", c.state);
    // 一度も loading を見ていない false は止まりと数えない（作った直後の 1 frame）
    let mut d = WebLife::new(XL, false, t0);
    d.on(0, WebEvent::Progress { loading: false, progress: 0.0 }, ms(t0, 0));
    d.tick(ms(t0, grace * 5));
    assert_eq!(d.state, WebState::Starting);
}

#[test]
fn w12w1b_navigation_started_but_no_page_is_recoverable_after_the_load_limit() {
    // navigation は始まった（progress が 0.1 より上）が頁が届かない（Started も題名も無い・loading は true のまま）: 開始から WEB_LOAD_LIMIT で Recoverable
    let t0 = Instant::now();
    let mut w = WebLife::new(XL, false, t0);
    w.on(0, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 40_000));
    w.tick(ms(t0, 40_000 + WEB_LOAD_LIMIT.as_millis() as u64));
    assert_eq!(w.state, WebState::Loading, "起点は開始の 40 秒（作った時ではない）");
    w.tick(ms(t0, 41_000 + WEB_LOAD_LIMIT.as_millis() as u64));
    assert!(matches!(&w.state, WebState::Recoverable(why) if why.contains("hadn't arrived")), "{:?}", w.state);
    // 試験の失敗の手の env: start-once だけ
    for (v, want) in [(Some("start-once"), Some(WebFault::StartOnce)), (Some("start"), None), (None, None)] {
        match v {
            Some(v) => std::env::set_var(WEB_FAULT_ENV, v),
            None => std::env::remove_var(WEB_FAULT_ENV),
        }
        assert_eq!(WebFault::from_env(), want, "{v:?}");
    }
}

#[test]
fn w12w1b_test_fault_is_the_real_start_limit_once() {
    // 試験の側の失敗の手（SHIBUBU_WEB_FAULT=start-once）: generation 0 の最初の tick で、本物の起動の上限と同じ 1 行の Recoverable・retry の後は本物
    let t0 = Instant::now();
    let mut real = WebLife::new(XL, false, t0);
    real.tick(ms(t0, (WEB_START_LIMIT.as_secs() + 1) * 1000));
    let mut w = WebLife::new(XL, false, t0);
    w.fault = Some(WebFault::StartOnce);
    w.tick(t0);
    assert!(matches!(&w.state, WebState::Recoverable(_)), "{:?}", w.state);
    assert_eq!(w.state, real.state, "理由の 1 行は本物と同じ");
    let g = w.retry(ms(t0, 1000));
    w.tick(ms(t0, 2000));
    assert_eq!(w.state, WebState::Starting, "retry の後は fault を当てない");
    w.on(g, WebEvent::Progress { loading: true, progress: 0.3 }, ms(t0, 3000));
    w.on(g, WebEvent::Finished(XL.into()), ms(t0, 4000));
    assert_eq!(w.state, WebState::Live);
}

#[test]
#[cfg(not(feature = "webview"))] // feature の build では窓が開いて終わらないので、既定の build だけで見る
fn web_without_the_feature_hands_off_to_the_browser() {
    // 既定の build（feature webview 無し）: WebView を作らず、本人の browser へ（KANGAE_OPEN_BIN の偽物に URL が渡る）
    let dir = std::env::temp_dir().join(format!("kangae-portal-test-{}-webcli", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let args = dir.join("open-args");
    let open = dir.join("open.sh");
    std::fs::write(&open, format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", args.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_kangae-portal"))
        .args(["web", "--youtube", "M7lc1UVf-VE"])
        .env("KANGAE_OPEN_BIN", &open)
        .env_remove("KANGAE_PORTAL_LOG")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("This build doesn't include the web view"), "{out}");
    // 中身が揃うまで待つ: shell の `>` は file を先に作ってから書くので、在るだけで読むと空を読む（高負荷で left "" になっていた）
    let deadline = Instant::now() + Duration::from_secs(10);
    let read = || std::fs::read_to_string(&args).unwrap_or_default();
    while read().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(read(), "https://www.youtube.com/embed/M7lc1UVf-VE");
    // 動画の id が壊れていれば何も開かない
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_kangae-portal")).args(["web", "--youtube", "../../x"]).env("KANGAE_OPEN_BIN", &open).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
#[cfg(not(feature = "webview"))]
fn handoff_refuses_non_web_schemes() {
    // 本人の browser へ渡すのは http / https だけ。頁が window.open で選べる URL から local の app・SMB・設定を起こさせない
    let dir = std::env::temp_dir().join(format!("kangae-portal-test-{}-scheme", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let args = dir.join("open-args");
    let open = dir.join("open.sh");
    std::fs::write(&open, format!("#!/bin/sh\nprintf '%s' \"$1\" >> '{}'\n", args.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    for url in ["file://localhost/System/Applications/Calculator.app", "smb://attacker.example/share", "x-apple.systempreferences://com.apple.preference.security", "HTTPS-x://a.example/"] {
        let o = std::process::Command::new(env!("CARGO_BIN_EXE_kangae-portal")).args(["web", url]).env("KANGAE_OPEN_BIN", &open).env_remove("KANGAE_PORTAL_LOG").output().unwrap();
        assert_ne!(o.status.code(), Some(0), "{url}: {}", String::from_utf8_lossy(&o.stdout));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(std::fs::read_to_string(&args).unwrap_or_default(), "", "偽の open に渡ってはいけない");
    // http / https は渡る（大文字の scheme も）
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_kangae-portal")).args(["web", "HTTPS://example.com/"]).env("KANGAE_OPEN_BIN", &open).env_remove("KANGAE_PORTAL_LOG").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
}

/// W2c: Lifecycle（12）× 面の種類 → 枠の 9 状態の表（docs/diagrams.md §28）を全部の組で見る。
#[test]
fn w2c_portal_state_maps_every_lifecycle_and_backend() {
    use Lifecycle as L;
    use PortalState as P;
    let native = AppSurfaceSpec::native_direct("docs", reaper());
    let mut mirror = native.clone();
    mirror.surface_backend = SurfaceBackend::InteractiveMirror;
    mirror.set_readiness(Layer::Input, Actor::Human, Readiness::Ready { evidence: "relay".into(), at_ms: 1.0 });
    let mut view_only = mirror.clone();
    view_only.set_readiness(Layer::Input, Actor::Human, Readiness::Unsupported { reason: "入力の転送が無い".into() });
    let mut stale = native.clone();
    stale.sync_mode = SyncMode::Stale;
    let mut external = native.clone();
    external.surface_backend = SurfaceBackend::ExternalHandoff;
    let mut web = native.clone();
    web.surface_backend = SurfaceBackend::WebView;
    // 列: native・mirror・view_only・stale・external・web
    let table: [(L, [Option<P>; 6]); 12] = [
        (L::Unresolved, [Some(P::Undiscovered); 6]),
        (L::Discovered, [Some(P::Attaching); 6]),
        (L::PermissionRequired, [Some(P::PermissionRequired); 6]),
        (L::Attaching, [Some(P::Attaching); 6]),
        (L::Active, [Some(P::NativeDirect), Some(P::Mirror), Some(P::ViewOnly), Some(P::SyncDelayed), Some(P::SyncDelayed), Some(P::NativeDirect)]),
        (L::Hidden, [Some(P::NativeDirect), Some(P::Mirror), Some(P::ViewOnly), Some(P::SyncDelayed), Some(P::SyncDelayed), Some(P::NativeDirect)]),
        (L::Resizing, [Some(P::NativeDirect), Some(P::Mirror), Some(P::ViewOnly), Some(P::SyncDelayed), Some(P::SyncDelayed), Some(P::NativeDirect)]),
        (L::SourceUnavailable, [Some(P::SourceUnavailable); 6]),
        (L::Degraded, [Some(P::IntegrationFailed); 6]),
        (L::SwitchingBackend, [Some(P::Attaching); 6]),
        (L::Detached, [Some(P::SyncDelayed); 6]),
        (L::Closed, [None; 6]),
    ];
    let specs = [&native, &mirror, &view_only, &stale, &external, &web];
    for (life, want) in table {
        for (spec, w) in specs.iter().zip(want) {
            assert_eq!(portal_state(&life, spec), w, "{life:?} × {:?} sync={:?}", spec.surface_backend, spec.sync_mode);
        }
    }
    // 接続済みの見た目（NativeDirect / Mirror）は Active / Hidden / Resizing の時だけ
    for life in [L::Unresolved, L::Discovered, L::PermissionRequired, L::Attaching, L::SourceUnavailable, L::Degraded, L::SwitchingBackend, L::Detached, L::Closed] {
        for spec in specs {
            assert!(!matches!(portal_state(&life, spec), Some(P::NativeDirect | P::Mirror)), "{life:?} で接続済みに見せた");
        }
    }
}

/// G90（operator の実機 07: YouTube の面が `Couldn't open www.youtube.com here · Sign in with your own browser`）: 頁が届いた後の sign-in の読み込み
/// （YouTube が裏で読む sub-frame の accounts.google.com/ServiceLogin・WKWebView の実測）はその読み込みだけ止めて面は開いたまま。頁が届く前に main が sign-in へ飛んだら今のまま面ごと handoff。
/// 負の対照（commit message）: Blocked{handoff: true} を常に Recoverable にする → 赤
#[test]
fn g90_a_sign_in_subframe_after_the_page_arrived_does_not_hand_the_surface_off() {
    let t0 = Instant::now();
    let at = |s: u64| t0 + Duration::from_secs(s);
    let signin = || WebEvent::Blocked { url: "https://accounts.google.com/ServiceLogin".into(), handoff: true, why: "Sign in with your own browser. Shibubu doesn't sign in for you".into() };
    let mut yt = WebLife::new("https://www.youtube.com/", true, t0);
    yt.on(0, WebEvent::Started("https://www.youtube.com/".into()), at(1));
    yt.on(0, signin(), at(2));
    assert!(!matches!(yt.state, WebState::Recoverable(_)), "sub-frame の sign-in で面ごと handoff した: {:?}", yt.state);
    assert_eq!(yt.handoff_url, None);
    assert!(yt.detail.as_deref().is_some_and(|d| d.contains("Sign in")), "止めた事を帯に言わない");
    yt.on(0, WebEvent::Finished("https://www.youtube.com/".into()), at(3));
    assert_eq!(yt.state, WebState::Live);
    // 頁が届く前に main が sign-in へ飛んだ（Docs の login の redirect）→ 面ごと handoff
    let mut docs = WebLife::new("https://docs.google.com/document/d/x", false, t0);
    docs.on(0, signin(), at(1));
    assert!(matches!(docs.state, WebState::Recoverable(_)));
    assert_eq!(docs.handoff_url.as_deref(), Some("https://accounts.google.com/ServiceLogin"));
}

