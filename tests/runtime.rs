//! unit RT（TODO.md RUN-001 / 002 / 004 / 005 / 006 / 008・CORE-003 の呼び出し経路）の受け入れ test。
//! bin crate なので `#[path]` で domain / storage / runtime を取り込む（runtime は `super::domain` / `super::storage` を引く）。
//! store は memory（再起動の test だけ temp の file）。外部 adapter は test の中の偽物（数える・止まる・失敗する）。
#[path = "../src/domain/mod.rs"]
mod domain;
#[allow(dead_code, unused_imports)]
#[path = "../src/runtime/mod.rs"]
mod runtime;
#[allow(dead_code, unused_imports)]
#[path = "../src/storage/mod.rs"]
mod storage;

use domain::action::{Effect, InvocationOrigin};
use domain::app::AppDefinitionV3;
use domain::capability::CapabilityReadiness;
use domain::patch::AppRevision;
use domain::permission::PermissionGrant;
use domain::run::{ErrorCode, Outcome, Receipt, RunStatus};
use runtime::lease::LeaseKey;
use runtime::permission::{canonical, payload_hash, sha256_hex, GrantRecord};
use runtime::receipt::{self as rcpt, ApprovalRequest};
use runtime::registry::{ActionContract, ActionRegistry, Ctx, Handler, UI_PATCH};
use runtime::{rfc3339, AppRuntime, CancelResult, Invoked, Runtime};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use std::collections::BTreeSet;
use storage::{Store, StoreError, Table};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn corpus(name: &str) -> AppRevision {
    let text = std::fs::read_to_string(format!("{ROOT}/docs/examples/v3/{name}.json")).expect(name);
    AppRevision::new(serde_json::from_str(&text).expect(name)).expect(name)
}

fn ext(id: &str, effect: &str, scopes: &[&str]) -> Value {
    json!({ "id": id, "capability_ref": "mail.api", "effect": effect, "readiness": "ready", "input_schema": { "type": "object" },
            "required_scopes": scopes, "requires_approval": effect == "external_send" })
}
fn local(id: &str, effect: &str) -> Value {
    json!({ "id": id, "effect": effect, "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": [] })
}

/// 偽の外部 adapter と test 用の local action を宣言した app（id `test.mail`）。space は `metadata.space_id`。
fn test_app(space: &str) -> AppRevision {
    test_app_as("test.mail", space)
}
fn test_app_as(id: &str, space: &str) -> AppRevision {
    let def: AppDefinitionV3 = serde_json::from_value(json!({
        "schema_version": "3.0", "id": id, "version": 1, "title": "runtime test",
        "goal": { "literal_goal": "runtime の test", "outcomes": ["test"] },
        "composition": { "root": { "key": "root", "component": "column" } },
        "capabilities": [ { "id": "mail.api", "provider": "fake", "readiness": "ready", "adapter": "api" } ],
        "actions": [
            ext("mail.inbox.read", "external_read", &["mail.read"]),
            ext("mail.archive.read", "external_read", &["mail.archive"]),
            ext("mail.messages.send", "external_send", &["mail.send"]),
            local("test.probe.write", "local_write"),
            local("test.click.fake", "local_write"),
            local("test.plain.write", "local_write"),
            local("test.count.write", "local_write"),
            local("test.flaky.read", "local_read"),
            local("test.flaky.write", "local_write"),
            local("test.slow.write", "local_write"),
            local("test.lease.hold", "local_write"),
            local("test.lease.other", "local_write"),
            local("test.cancel.loop", "local_write"),
            local("test.cancel.race", "local_write"),
            local("test.lease.slow", "local_write"),
            local("test.lease.next", "local_write"),
            local("test.verify.fails", "local_write"),
            local("test.verify.rejects", "local_write"),
            local("test.lease.read", "local_read"),
            local("local.timer.save_settings", "local_write"),
            local("test.unregistered.op", "local_write")
        ],
        "sources": [],
        "agent": { "suggestions_enabled": true, "autonomy_level": "ask_before_external_effects", "entrypoint": "agent_rail" },
        "state": { "readiness": "ready", "execution_mode": "integrated" },
        "metadata": { "space_id": space }
    }))
    .unwrap();
    AppRevision::new(def).unwrap()
}

/// closure 2 本（execute / verify）の handler。
struct F<E, V>(E, V);
impl<E, V> Handler for F<E, V>
where
    E: Fn(&Ctx, &Value) -> Result<Value, ErrorCode> + Send + Sync,
    V: Fn(&Ctx, &Value, &Value) -> Outcome + Send + Sync,
{
    fn execute(&self, ctx: &Ctx, input: &Value) -> Result<Value, ErrorCode> {
        (self.0)(ctx, input)
    }
    fn verify(&self, ctx: &Ctx, input: &Value, accepted: &Value) -> Outcome {
        (self.1)(ctx, input, accepted)
    }
}
fn handler(
    exec: impl Fn(&Ctx, &Value) -> Result<Value, ErrorCode> + Send + Sync + 'static,
    verify: impl Fn(&Ctx, &Value, &Value) -> Outcome + Send + Sync + 'static,
) -> impl Handler {
    F(exec, verify)
}
/// verify を持たない handler（trait の既定 = 読み直す手段が無い）。
struct ExecOnly<E>(E);
impl<E: Fn(&Ctx, &Value) -> Result<Value, ErrorCode> + Send + Sync> Handler for ExecOnly<E> {
    fn execute(&self, ctx: &Ctx, input: &Value) -> Result<Value, ErrorCode> {
        (self.0)(ctx, input)
    }
}
fn exec_only(exec: impl Fn(&Ctx, &Value) -> Result<Value, ErrorCode> + Send + Sync + 'static) -> impl Handler {
    ExecOnly(exec)
}
fn observed(ctx: &Ctx, _: &Value, _: &Value) -> Outcome {
    Outcome::Verified { evidence: vec![ctx.observe("test", json!({ "seen": true })).unwrap()] }
}

/// local action ＋ test の偽物を登録した runtime（memory の store・偽の外部 adapter "mail" は ready）。
fn start(extra: impl FnOnce(&mut ActionRegistry)) -> (Arc<Runtime>, Arc<Mutex<Store>>) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let mut reg = ActionRegistry::with_local_actions();
    extra(&mut reg);
    let rt = Runtime::start(store.clone(), reg).unwrap();
    rt.registry().set_adapter_state("mail", CapabilityReadiness::Ready);
    (rt, store)
}

fn button(key: &str) -> InvocationOrigin {
    InvocationOrigin::Button { node_key: key.into() }
}
fn agent(suggestion: &str) -> InvocationOrigin {
    InvocationOrigin::Agent { suggestion_id: suggestion.into() }
}
fn done(r: Result<Invoked, ErrorCode>) -> Receipt {
    match r {
        Ok(Invoked::Done(receipt)) => receipt,
        other => panic!("Done を期待: {other:?}"),
    }
}
fn asks(r: Result<Invoked, ErrorCode>) -> ApprovalRequest {
    match r {
        Ok(Invoked::NeedsApproval(req)) => req,
        other => panic!("NeedsApproval を期待: {other:?}"),
    }
}
fn verified(receipt: &Receipt) -> bool {
    matches!(receipt.outcome, Outcome::Verified { .. })
}
fn doc(store: &Arc<Mutex<Store>>, session: &str, key: &str) -> Value {
    store.lock().unwrap().get_ui_state(session, key).unwrap().unwrap_or(Value::Null)
}
fn grant(id: &str, scope: &str, expires_at: Option<&str>) -> PermissionGrant {
    PermissionGrant {
        id: id.into(),
        app_id: "test.mail".into(),
        scope: scope.into(),
        granted_at: "2026-10-01T00:00:00.000Z".into(),
        expires_at: expires_at.map(String::from),
        consent_ref: None,
    }
}
fn status(rt: &Runtime, run_id: &str) -> RunStatus {
    rt.run(run_id).unwrap().unwrap().run.status
}
fn events(rt: &Runtime, run_id: &str) -> Vec<String> {
    rt.run_log(run_id).unwrap().into_iter().map(|l| l.event).collect()
}
fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = probe() {
            return v;
        }
        assert!(Instant::now() < end, "待ちきれない: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// RUN-001 / CORE-003: 未登録の action は button / ⌘K / Agent のどれからも同じ Err で、run も event も作らない。
#[test]
fn unregistered_action_is_refused_the_same_way_for_every_origin() {
    let (rt, _) = start(|_| {});
    let app = rt.open(test_app("work"), "s1").unwrap();
    for origin in [button("x"), InvocationOrigin::CommandPalette, agent("sg-1")] {
        // app は宣言しているが registry に無い / どちらにも無い
        assert_eq!(app.invoke("test.unregistered.op", json!({}), origin.clone()), Err(ErrorCode::NotReady));
        assert_eq!(app.invoke("nothing.declared.anywhere", json!({}), origin.clone()), Err(ErrorCode::NotReady));
        // registry に在っても app が宣言していなければ domain の validate_invocation が同じ答えを返す
        assert_eq!(app.invoke("local.timer.toggle", json!({}), origin), Err(ErrorCode::TargetNotFound));
    }
    assert!(app.runs().unwrap().is_empty(), "拒否した呼び出しで run を作った");
    // 裏: 同じ呼び方でも登録すれば通る（拒否は「未登録」に結び付いている）
    let (rt2, _) = start(|reg| reg.register("test.unregistered.op", ActionContract::local(Effect::LocalWrite), handler(|_, _| Ok(json!({})), observed)));
    let app2 = rt2.open(test_app("work"), "s1").unwrap();
    assert!(verified(&done(app2.invoke("test.unregistered.op", json!({}), button("x")))));
}

/// 偽の外部 adapter "mail" の読取 2 つ（呼ばれた数を数える）。要る scope は registry の契約が持つ（RT-SEC-R2-1）。
fn mail_reads(reg: &mut ActionRegistry, calls: &Arc<AtomicUsize>) {
    let reads: [(&str, &'static [&'static str]); 2] = [("mail.inbox.read", &["mail.read"]), ("mail.archive.read", &["mail.archive"])];
    for (id, scopes) in reads {
        let c = calls.clone();
        let read = ActionContract { adapter: "mail", scopes, ..ActionContract::local(Effect::ExternalRead) };
        reg.register(id, read, handler(move |_, _| { c.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
    }
}

/// RUN-002: Work の grant は Friends に漏れない。期限切れ・未宣言の scope も通さない。読取は承認の modal を出さない（CD-17 #2）。
/// app の space は store の定義が持つ（RT-SEC-2）: Work に置いた app を Friends として開き直す道は無く、Friends の app は別の app。
#[test]
fn grant_from_work_does_not_leak_into_friends() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let (rt, store) = start(move |reg| mail_reads(reg, &c));
    let work = rt.open(test_app("work"), "s-work").unwrap();
    assert_eq!(work.invoke("mail.inbox.read", json!({}), button("inbox")), Err(ErrorCode::PermissionDenied));
    work.grant(grant("g-read", "mail.read", None)).unwrap();
    assert!(verified(&done(work.invoke("mail.inbox.read", json!({}), button("inbox")))), "grant 済みの読取は承認を聞かずに実行する");
    // Work に置いた app を、Friends を名乗る定義で開き直して使う道は無い
    assert_eq!(rt.open(test_app("friends"), "s-friends").err(), Some(ErrorCode::PermissionDenied));
    // Friends に置いた app（同じ宣言・別の id）からは、同じ scope でも通らない
    let friends = rt.open(test_app_as("test.mail.friends", "friends"), "s-friends").unwrap();
    assert_eq!(friends.space_id(), "friends");
    assert_eq!(friends.invoke("mail.inbox.read", json!({}), button("inbox")), Err(ErrorCode::PermissionDenied));
    assert_eq!(friends.invoke("mail.inbox.read", json!({}), agent("sg-1")), Err(ErrorCode::PermissionDenied));
    // space だけが違う grant の行（同じ app・同じ scope・期限なし。Friends に置いた物）は Work の app に効かない（gate の space の比較）
    let stray = GrantRecord { space_id: "friends".into(), grant: grant("g-stray", "mail.archive", None) };
    store.lock().unwrap().put(Table::Permissions, "g-stray", Some("test.mail"), &serde_json::to_value(&stray).unwrap()).unwrap();
    assert_eq!(work.invoke("mail.archive.read", json!({}), button("archive")), Err(ErrorCode::PermissionDenied));
    // 期限の切れた grant は数えない・app が宣言していない scope の grant は作らない
    work.grant(grant("g-old", "mail.archive", Some("2000-01-01T00:00:00.000Z"))).unwrap();
    assert_eq!(work.invoke("mail.archive.read", json!({}), button("archive")), Err(ErrorCode::PermissionDenied));
    assert_eq!(work.grant(grant("g-any", "mail.everything", None)), Err(ErrorCode::PermissionDenied));
    assert_eq!(calls.load(SeqCst), 1);
    assert!(friends.runs().unwrap().is_empty());
}

/// CD-09 (i): handler が呼ばれた時には Attempt{outcome: None} が running で保存済み。終わると outcome と Receipt が埋まる。
#[test]
fn attempt_is_saved_before_dispatch_and_filled_after() {
    let seen: Arc<Mutex<Option<(RunStatus, usize, bool)>>> = Arc::default();
    let s = seen.clone();
    let (rt, _) = start(move |reg| {
        let probe = handler(
            move |ctx, _| {
                let rec = rcpt::load(&ctx.store(), &ctx.run_id).unwrap().unwrap();
                let attempts = &rec.run.steps[0].attempts;
                *s.lock().unwrap() = Some((rec.run.status, attempts.len(), attempts.last().is_some_and(|a| a.outcome.is_none())));
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.probe.write", ActionContract::local(Effect::LocalWrite), probe);
    });
    let app = rt.open(test_app("work"), "s1").unwrap();
    let rc = done(app.invoke("test.probe.write", json!({}), button("probe")));
    assert_eq!(*seen.lock().unwrap(), Some((RunStatus::Running, 1, true)), "dispatch の時点で Attempt{{None}} が保存されていない");
    let rec = rt.run(&rc.run_id).unwrap().unwrap();
    assert_eq!(rec.run.status, RunStatus::Verified);
    let attempt = &rec.run.steps[0].attempts[0];
    assert!(matches!(attempt.outcome, Some(Outcome::Verified { .. })) && attempt.finished_at.is_some(), "{attempt:?}");
    assert_eq!(rec.receipt.as_ref(), Some(&rc));
    assert!(!rc.observation_refs.is_empty());
    let ev = events(&rt, &rc.run_id);
    let at = |k: &str| ev.iter().position(|e| e == k).unwrap_or_else(|| panic!("{k} が無い: {ev:?}"));
    assert!(at("RunCreated") < at("ActionDispatched") && at("ActionDispatched") < at("DriverAccepted") && at("DriverAccepted") < at("VerificationSucceeded"), "{ev:?}");
}

/// RUN-004: 「押せた」（受理）だけで backend が変わっていなければ Unverified。読み直す手段の無い action も Verified にしない。
#[test]
fn accepted_without_backend_change_is_unverified() {
    let (rt, _) = start(|reg| {
        // 押したと言うが何も書かない偽の click。verify は backend（ui_state）を読み直す
        let click = handler(
            |_, _| Ok(json!({ "clicked": true })),
            |ctx, _, _| match ctx.store().get_ui_state(&ctx.session_id, "test.backend").unwrap() {
                Some(v) if v["saved"] == true => Outcome::Verified { evidence: Vec::new() },
                _ => Outcome::Unverified { reason: "backend に保存が無い".into() },
            },
        );
        reg.register("test.click.fake", ActionContract::local(Effect::LocalWrite), click);
        reg.register("test.plain.write", ActionContract::local(Effect::LocalWrite), exec_only(|_, _| Ok(json!({}))));
    });
    let app = rt.open(test_app("work"), "s1").unwrap();
    for id in ["test.click.fake", "test.plain.write"] {
        let rc = done(app.invoke(id, json!({}), button(id)));
        assert!(matches!(rc.outcome, Outcome::Unverified { .. }), "{id}: {:?}", rc.outcome);
        assert!(rc.observation_refs.is_empty());
        assert_eq!(status(&rt, &rc.run_id), RunStatus::Uncertain);
        let ev = events(&rt, &rc.run_id);
        let has = |k: &str| ev.iter().any(|e| e == k);
        assert!(has("DriverAccepted") && has("VerificationUnverified") && !has("VerificationSucceeded"), "{id}: {ev:?}");
    }
}

/// RUN-005: 同じ key の 2 回目は送り直さない（明示 key・承認の 2 度押し・Agent の同じ提案）。
/// 読取の再試行は契約の回数で止まり（暴走しない）、書込の Timeout は再試行せず EffectUnknown。
#[test]
fn idempotency_key_and_retry_limits_prevent_repeated_sends() {
    let (count, sends, flaky_read, flaky_write) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, s, fr, fw) = (count.clone(), sends.clone(), flaky_read.clone(), flaky_write.clone());
    let (rt, store) = start(move |reg| {
        reg.register("test.count.write", ActionContract::local(Effect::LocalWrite), handler(move |_, _| { c.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
        let send = ActionContract { adapter: "mail", scopes: &["mail.send"], ..ActionContract::local(Effect::ExternalSend) };
        reg.register("mail.messages.send", send, handler(move |_, _| { s.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
        let read = ActionContract { backoff: Duration::from_millis(1), ..ActionContract::local(Effect::LocalRead) };
        reg.register("test.flaky.read", read, handler(move |_, _| { fr.fetch_add(1, SeqCst); Err(ErrorCode::ConnectionLost) }, observed));
        reg.register("test.flaky.write", ActionContract::local(Effect::LocalWrite), handler(move |_, _| { fw.fetch_add(1, SeqCst); Err(ErrorCode::Timeout) }, observed));
    });
    let app = rt.open(test_app("work"), "s1").unwrap();
    // 明示 key: 2 回目は同じ Receipt。key が無ければ別の実行（button の 2 度押しは 2 回の意図）
    let first = done(app.invoke_keyed("test.count.write", json!({}), button("save"), Some("k-1")));
    assert_eq!(done(app.invoke_keyed("test.count.write", json!({}), button("save"), Some("k-1"))), first);
    assert_eq!(count.load(SeqCst), 1);
    // local の書込（中身は session の ui_state）の key は session ごと: 別 session は自分の run を作り、他の session の Receipt は返さない（RT-R2-3）
    let other_session = rt.open(test_app("work"), "s2").unwrap();
    let second = done(other_session.invoke_keyed("test.count.write", json!({}), button("save"), Some("k-1")));
    assert_ne!(second.run_id, first.run_id, "別 session に他の session の Receipt を返した");
    assert_eq!(done(other_session.invoke_keyed("test.count.write", json!({}), button("save"), Some("k-1"))), second);
    assert_eq!(count.load(SeqCst), 2);
    done(app.invoke("test.count.write", json!({}), button("save")));
    assert_eq!(count.load(SeqCst), 3);
    // 承認の 2 度押し: 送るのは 1 回。承認の前は 0 回
    app.grant(grant("g-send", "mail.send", None)).unwrap();
    let req = asks(app.invoke("mail.messages.send", json!({ "to": "a@example.com", "body": "hi" }), button("send")));
    assert_eq!(sends.load(SeqCst), 0, "承認の前に送った");
    let a = app.approve(&req.run_id, &req.payload_hash).unwrap();
    assert_eq!(app.approve(&req.run_id, &req.payload_hash).unwrap(), a);
    assert_eq!(sends.load(SeqCst), 1);
    // Agent の同じ提案（corpus 01 は suggest_only なので local の書込も聞く）: 承認待ちは同じ物、承認後は同じ Receipt
    let timer = rt.open(corpus("01_timer_local"), "s-timer").unwrap();
    let r1 = asks(timer.invoke("local.timer.toggle", json!({}), agent("sg-1")));
    assert_eq!(asks(timer.invoke("local.timer.toggle", json!({}), agent("sg-1"))), r1);
    let t1 = timer.approve(&r1.run_id, &r1.payload_hash).unwrap();
    assert!(verified(&t1));
    assert_eq!(done(timer.invoke("local.timer.toggle", json!({}), agent("sg-1"))), t1);
    assert_eq!(timer.runs().unwrap().len(), 1);
    assert_eq!(doc(&store, "s-timer", "local.timer")["running"], true, "start は 1 回だけ");
    // 読取: ConnectionLost を 2 回まで再試行して止まる。書込: Timeout は送り直さず EffectUnknown ＋ uncertain
    let rc = done(app.invoke("test.flaky.read", json!({}), button("refresh")));
    assert_eq!(rc.outcome, Outcome::Failed { error: ErrorCode::ConnectionLost });
    assert_eq!((flaky_read.load(SeqCst), rc.attempt_seq), (3, 3));
    let rc = done(app.invoke("test.flaky.write", json!({}), button("write")));
    assert!(matches!(rc.outcome, Outcome::EffectUnknown { .. }), "{:?}", rc.outcome);
    assert_eq!((flaky_write.load(SeqCst), rc.attempt_seq), (1, 1));
    assert_eq!(status(&rt, &rc.run_id), RunStatus::Uncertain);
}

/// RUN-008: cancel は lease の実行待ちと handler の checkpoint に届く。終わった副作用は取り消したと見せない（TooLate・状態はそのまま）。
#[test]
fn cancel_reaches_queued_and_cancellable_running_but_not_finished_effects() {
    let (entered_tx, entered_rx) = mpsc::channel::<String>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (entered_tx, release_rx) = (Arc::new(Mutex::new(entered_tx)), Mutex::new(release_rx));
    let (other_calls, writes) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (oc, w, e2) = (other_calls.clone(), writes.clone(), entered_tx.clone());
    let (rt, _) = start(move |reg| {
        let leased = ActionContract {
            leases: |input: &Value| vec![LeaseKey::Repo(input["repo"].as_str().unwrap_or_default().into())],
            timeout: Duration::from_secs(20),
            ..ActionContract::local(Effect::LocalWrite)
        };
        let hold = handler(
            move |ctx, _| {
                entered_tx.lock().unwrap().send(ctx.run_id.clone()).unwrap();
                let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.lease.hold", leased.clone(), hold);
        reg.register("test.lease.other", leased, handler(move |_, _| { oc.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
        let looping = handler(
            move |ctx, _| {
                e2.lock().unwrap().send(ctx.run_id.clone()).unwrap();
                let end = Instant::now() + Duration::from_secs(10);
                while !ctx.cancelled() && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if ctx.cancelled() {
                    return Err(ErrorCode::Cancelled); // 書く前の checkpoint
                }
                w.fetch_add(1, SeqCst);
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.cancel.loop", ActionContract { timeout: Duration::from_secs(20), ..ActionContract::local(Effect::LocalWrite) }, looping);
    });
    let app = Arc::new(rt.open(test_app("work"), "s1").unwrap());
    // 1. 実行待ち: hold が repo:x を持つ間に other を打つ → 待つ → cancel → dispatch 0 のまま cancelled
    let a = app.clone();
    let hold = std::thread::spawn(move || a.invoke("test.lease.hold", json!({ "repo": "x" }), button("hold")));
    let hold_run = entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let a = app.clone();
    let other = std::thread::spawn(move || a.invoke("test.lease.other", json!({ "repo": "x" }), button("other")));
    let other_run = wait_for("other が実行待ちになる", || {
        app.runs().unwrap().into_iter().find(|r| r.action_id() == "test.lease.other" && events(&rt, &r.run.id).iter().any(|e| e == "LeaseQueued")).map(|r| r.run.id)
    });
    assert_eq!(rt.cancel(&other_run), Ok(CancelResult::Requested));
    assert_eq!(other.join().unwrap(), Err(ErrorCode::Cancelled));
    assert_eq!(status(&rt, &hold_run), RunStatus::Running, "実行待ちの cancel が lease の持ち主の終わりを待った");
    assert_eq!(status(&rt, &other_run), RunStatus::Cancelled);
    assert_eq!(rt.run(&other_run).unwrap().unwrap().attempt_count(), 0);
    assert_eq!(other_calls.load(SeqCst), 0);
    // 2. 終わった副作用: hold を終わらせて verified → cancel は TooLate で状態はそのまま
    release_tx.send(()).unwrap();
    assert!(verified(&done(hold.join().unwrap())));
    assert_eq!(rt.cancel(&hold_run), Ok(CancelResult::TooLate(RunStatus::Verified)));
    assert_eq!(status(&rt, &hold_run), RunStatus::Verified);
    assert!(events(&rt, &hold_run).iter().any(|e| e == "CancelTooLate"));
    // 3. handler の中: checkpoint で flag を見て止まる → cancelled・書いていない
    let a = app.clone();
    let looping = std::thread::spawn(move || a.invoke("test.cancel.loop", json!({}), button("loop")));
    let loop_run = entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(rt.cancel(&loop_run), Ok(CancelResult::Requested));
    let rc = done(looping.join().unwrap());
    assert_eq!(rc.outcome, Outcome::Failed { error: ErrorCode::Cancelled });
    assert_eq!(status(&rt, &loop_run), RunStatus::Cancelled);
    assert_eq!(writes.load(SeqCst), 0);
}

/// 呼ばれた数を数え、入ったら知らせ、release まで止まる書込（process が止まる途中の代わり）。
fn slow_write(reg: &mut ActionRegistry, calls: Arc<AtomicUsize>, entered: mpsc::Sender<()>, release: Arc<Mutex<mpsc::Receiver<()>>>) {
    let entered = Mutex::new(entered);
    let contract = ActionContract { timeout: Duration::from_secs(60), ..ActionContract::local(Effect::LocalWrite) };
    let slow = handler(
        move |_, _| {
            calls.fetch_add(1, SeqCst);
            let _ = entered.lock().unwrap().send(());
            let _ = release.lock().unwrap().recv_timeout(Duration::from_secs(30));
            Ok(json!({}))
        },
        observed,
    );
    reg.register("test.slow.write", contract, slow);
}

/// CD-09 (ii)・DATA-AC-06: 書込の途中で process が止まった → 開き直すと EffectUnknown ＋ uncertain。自動で送り直さない（同じ key でも）。
#[test]
fn restart_turns_an_open_attempt_into_effect_unknown_without_resending() {
    let path = std::env::temp_dir().join(format!("kangae-rt-{}-{}.sqlite3", std::process::id(), storage::new_id()));
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let boot = |reg: ActionRegistry| Runtime::start(Arc::new(Mutex::new(Store::open(&path).unwrap())), reg).unwrap();

    let mut reg1 = ActionRegistry::with_local_actions();
    slow_write(&mut reg1, calls.clone(), entered_tx.clone(), release_rx.clone());
    let app1 = boot(reg1).open(test_app("work"), "s1").unwrap();
    let worker = std::thread::spawn(move || app1.invoke_keyed("test.slow.write", json!({}), button("save"), Some("k-crash")));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(); // handler の中 = Attempt{None} は保存済み

    // 同じ file を新しい接続で開いて start = 再起動した process（前の process の thread は止まったまま）
    let mut reg2 = ActionRegistry::with_local_actions();
    slow_write(&mut reg2, calls.clone(), entered_tx.clone(), release_rx.clone());
    let rt2 = boot(reg2);
    let app2 = rt2.open(test_app("work"), "s1").unwrap();
    let rec = app2.runs().unwrap().into_iter().find(|r| r.idempotency_key.as_deref() == Some("k-crash")).expect("run が無い");
    assert_eq!(rec.run.status, RunStatus::Uncertain);
    assert!(matches!(rec.run.steps[0].attempts[0].outcome, Some(Outcome::EffectUnknown { .. })), "{:?}", rec.run.steps[0].attempts);
    assert!(matches!(rec.receipt.as_ref().map(|r| &r.outcome), Some(Outcome::EffectUnknown { .. })));
    // FX7（FX-044）: reason は英語の事実の文（Runs の詳細に runtime の日本語の理由を出さない）。負の対照: recover の reason を日本語に戻す → 赤
    let reason = match &rec.run.steps[0].attempts[0].outcome { Some(Outcome::EffectUnknown { reason }) => reason.clone(), other => panic!("{other:?}") };
    assert!(reason.contains("not sent again") && reason.is_ascii(), "recover の reason が英語の事実でない: {reason}");
    assert!(events(&rt2, &rec.run.id).iter().any(|e| e == "RunNeedsReconciliation"));
    assert_eq!(calls.load(SeqCst), 1, "再起動で送り直した");
    // 同じ key で打ち直しても送らない（EffectUnknown の Receipt が返る）
    let again = done(app2.invoke_keyed("test.slow.write", json!({}), button("save"), Some("k-crash")));
    assert!(matches!(again.outcome, Outcome::EffectUnknown { .. }), "{:?}", again.outcome);
    assert_eq!(calls.load(SeqCst), 1);

    let _ = release_tx.send(());
    let _ = worker.join();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

/// ADR-005 (5): destructive は exact payload の hash に結び付いた承認を待つ。違う hash・承認の後に変わった入力では実行しない。
/// 待つ間の cancel は副作用の前。
#[test]
fn approval_is_bound_to_the_exact_payload_hash() {
    let (rt, store) = start(|_| {});
    let inv = rt.open(corpus("02_inventory"), "s-inv").unwrap();
    assert!(verified(&done(inv.invoke("local.inventory.add", json!({ "name": "りんご", "quantity": 3 }), button("inventory-add")))));
    let item = doc(&store, "s-inv", "local.inventory")["items"][0]["id"].as_str().unwrap().to_string();
    let input = json!({ "item_id": item });
    let req = asks(inv.invoke("local.inventory.delete", input.clone(), button("inventory-delete")));
    assert_eq!((req.effect, &req.input), (Effect::Destructive, &input));
    assert_eq!(req.payload_hash, payload_hash("local.inventory.delete", "example.v3.inventory", "default", &input, &[]));
    assert_eq!(status(&rt, &req.run_id), RunStatus::WaitingApproval);
    // 1 文字違う payload の hash では実行しない（待ちのまま）
    let other = payload_hash("local.inventory.delete", "example.v3.inventory", "default", &json!({ "item_id": format!("{item}x") }), &[]);
    assert_eq!(inv.approve(&req.run_id, &other), Err(ErrorCode::ApprovalDenied));
    assert_eq!(status(&rt, &req.run_id), RunStatus::WaitingApproval);
    assert_eq!(doc(&store, "s-inv", "local.inventory")["items"].as_array().unwrap().len(), 1);
    // 凍結した入力が後から変わった（保存を書き換えた）→ 見せた hash と合わないので実行しない
    let original = rt.run(&req.run_id).unwrap().unwrap();
    let mut changed = original.clone();
    changed.input = json!({ "item_id": "someone-else" });
    rcpt::save(&store.lock().unwrap(), &changed).unwrap();
    assert_eq!(inv.approve(&req.run_id, &req.payload_hash), Err(ErrorCode::ApprovalDenied));
    assert_eq!(events(&rt, &req.run_id).iter().filter(|e| *e == "ApprovalMismatch").count(), 2);
    rcpt::save(&store.lock().unwrap(), &original).unwrap();
    // 見せた物どおりなら実行・2 回目は同じ Receipt・ConsentReceipt に hash が残る
    let rc = inv.approve(&req.run_id, &req.payload_hash).unwrap();
    assert!(verified(&rc));
    assert_eq!(inv.approve(&req.run_id, &req.payload_hash), Ok(rc));
    assert!(doc(&store, "s-inv", "local.inventory")["items"].as_array().unwrap().is_empty());
    let consent = rt.run(&req.run_id).unwrap().unwrap().consent.unwrap();
    assert_eq!(consent.payload_digest.as_deref(), Some(req.payload_hash.as_str()));
    // 承認待ちの cancel は副作用の前: cancelled・後から approve しても動かない
    done(inv.invoke("local.inventory.add", json!({ "name": "みかん" }), button("inventory-add")));
    let item = doc(&store, "s-inv", "local.inventory")["items"][0]["id"].as_str().unwrap().to_string();
    let req = asks(inv.invoke("local.inventory.delete", json!({ "item_id": item }), button("inventory-delete")));
    assert_eq!(rt.cancel(&req.run_id), Ok(CancelResult::Cancelled));
    assert_eq!(inv.approve(&req.run_id, &req.payload_hash), Err(ErrorCode::Cancelled));
    assert_eq!(doc(&store, "s-inv", "local.inventory")["items"].as_array().unwrap().len(), 1);
}

/// RUN-006: run log の各行に 操作 / source / 時刻 / 対象 / 結果。secret は伏せる（本文そのものは利用者の data として保存する）。
#[test]
fn run_log_shows_operation_source_time_target_result_and_masks_secrets() {
    let (rt, store) = start(|_| {});
    let coding = rt.open(corpus("03_coding_workspace"), "s-code").unwrap();
    let secret = &format!("ghp_{}", "q".repeat(36)); // tests/storage.rs と同じく実行時に組む（file に token の形を置かない）
    let rc = done(coding.invoke("local.notes.save", json!({ "body": format!("メモ token={secret}") }), button("coding-notes-save")));
    assert!(verified(&rc));
    let log = rt.run_log(&rc.run_id).unwrap();
    assert!(log.len() >= 5, "{log:?}");
    for line in &log {
        assert_eq!((line.operation.as_str(), line.source.as_str(), line.target.as_str()), ("local.notes.save", "button:coding-notes-save", "local.notes"));
        assert!(line.at.ends_with('Z') && !line.result.is_empty(), "{line:?}");
    }
    assert_eq!(log.first().map(|l| l.event.as_str()), Some("RunCreated"));
    assert_eq!(log.last().map(|l| (l.event.as_str(), l.result.as_str())), Some(("VerificationSucceeded", "verified")));
    let text = serde_json::to_string(&log).unwrap();
    assert!(!text.contains(secret) && text.contains("[redacted]"), "{text}");
    assert!(doc(&store, "s-code", "local.notes")["body"].as_str().unwrap().contains(secret), "本文は利用者の data として残す");
}

/// 通し: corpus 01〜04 の local action が corpus と同じ id で invoke を通って verified になり、中身が binding の path に入る。
#[test]
fn corpus_local_actions_run_through_invoke() {
    let (rt, store) = start(|_| {});
    // 登録した id は corpus 01〜04 に在る id だけ（ui.patch は runtime 側）。composer の出す id とずれたら赤
    let mut corpus_ids = std::collections::BTreeSet::new();
    for n in ["01_timer_local", "02_inventory", "03_coding_workspace", "04_research_notes"] {
        corpus_ids.extend(corpus(n).definition().actions.iter().map(|a| a.id.clone()));
    }
    // 起動時に登録するのは corpus の id だけ。composer の一覧（読書記録・家計簿…）は app を開いた時に定義から導く（src/generated.rs の通しが見る）
    let registered = rt.registry().ids();
    assert_eq!(registered.len(), 17, "{registered:?}");
    for id in &registered {
        assert!(id == UI_PATCH || corpus_ids.contains(id), "corpus に無い id を登録した: {id}");
    }
    assert!(!corpus_ids.contains(UI_PATCH));
    let ok = |app: &AppRuntime, id: &str, input: Value| {
        let rc = done(app.invoke(id, input, button(id)));
        assert!(verified(&rc), "{id}: {:?}", rc.outcome);
    };

    // 01 timer（toggle = start / pause）
    let timer = rt.open(corpus("01_timer_local"), "s-timer").unwrap();
    ok(&timer, "local.timer.toggle", json!({}));
    let t = doc(&store, "s-timer", "local.timer");
    assert_eq!((&t["running"], &t["remaining_display"]), (&json!(true), &json!("25:00")));
    ok(&timer, "local.timer.toggle", Value::Null);
    assert_eq!(doc(&store, "s-timer", "local.timer")["running"], false);
    ok(&timer, "local.timer.save_settings", json!({ "duration_minutes": 10 }));
    let t = doc(&store, "s-timer", "local.timer");
    assert_eq!((&t["settings"]["duration_minutes"], &t["remaining_display"]), (&json!(10), &json!("10:00")));
    assert_eq!(timer.invoke("local.timer.save_settings", json!({ "duration_minutes": 0 }), button("timer-settings")), Err(ErrorCode::InvalidInput));
    assert_eq!(timer.invoke("local.timer.save_settings", json!({ "duration_minutes": 5, "extra": 1 }), button("timer-settings")), Err(ErrorCode::InvalidInput));
    ok(&timer, "local.timer.reset", json!({}));
    // ui.patch（domain の AppPatch）: 新しい revision が保存され、runtime の定義が入れ替わる。古い版に向けた patch は当てない
    ok(&timer, UI_PATCH, json!({ "operations": [ { "op": "update", "target_key": "timer-remaining", "value": { "label": "残り時間" } } ] }));
    assert_eq!(timer.revision().version(), 2);
    let latest = store.lock().unwrap().latest_revision("example.v3.timer_local").unwrap().unwrap();
    assert_eq!(latest.json["composition"]["root"]["children"][0]["label"], "残り時間");
    let stale = done(timer.invoke(UI_PATCH, json!({ "app_version": 1, "operations": [ { "op": "remove", "target_key": "timer-reset" } ] }), button("patch")));
    assert_eq!(stale.outcome, Outcome::Failed { error: ErrorCode::InvalidInput });
    assert_eq!(timer.revision().version(), 2);

    // 02 在庫（records の create / update / list）
    let inv = rt.open(corpus("02_inventory"), "s-inv").unwrap();
    ok(&inv, "local.inventory.add", json!({ "name": "りんご", "quantity": 3 }));
    ok(&inv, "local.inventory.add", json!({ "name": "バナナ" }));
    let items = doc(&store, "s-inv", "local.inventory")["items"].clone();
    let apple = items[0]["id"].as_str().unwrap().to_string();
    assert_eq!((&items[1]["quantity"], items.as_array().unwrap().len()), (&json!(0), 2));
    ok(&inv, "local.inventory.update_quantity", json!({ "item_id": apple, "quantity": 7 }));
    assert_eq!(doc(&store, "s-inv", "local.inventory")["items"][0]["quantity"], 7);
    ok(&inv, "local.inventory.filter", json!({ "query": "りん" }));
    assert_eq!(doc(&store, "s-inv", "local.inventory")["filter"]["ids"], json!([apple]));
    assert_eq!(inv.invoke("local.inventory.update_quantity", json!({ "item_id": apple, "quantity": -1 }), button("q")), Err(ErrorCode::InvalidInput));
    let missing = done(inv.invoke("local.inventory.update_quantity", json!({ "item_id": "nope", "quantity": 1 }), button("q")));
    assert_eq!(missing.outcome, Outcome::Failed { error: ErrorCode::TargetNotFound });
    assert_eq!(inv.invoke("local.inventory.import_csv", json!({ "file_ref": "f" }), button("inventory-import")), Err(ErrorCode::NotReady));

    // 03 coding（task・単一メモ・接続を開く。外部の issues.read は呼べない）
    let code = rt.open(corpus("03_coding_workspace"), "s-code").unwrap();
    ok(&code, "local.tasks.add", json!({ "title": "test を書く" }));
    let task = doc(&store, "s-code", "local.tasks")["items"][0]["id"].as_str().unwrap().to_string();
    ok(&code, "local.tasks.toggle_done", json!({ "task_id": task }));
    assert_eq!(doc(&store, "s-code", "local.tasks")["items"][0]["done"], true);
    ok(&code, "local.notes.save", json!({ "body": "今日の作業" }));
    assert_eq!(doc(&store, "s-code", "local.notes")["body"], "今日の作業");
    ok(&code, "local.connection.open", json!({ "capability_ref": "issues.read" }));
    assert_eq!(doc(&store, "s-code", "local.connection")["requested"], "issues.read");
    assert_eq!(code.invoke("issues.read", json!({ "repository": "a/b" }), button("coding-issues-refresh")), Err(ErrorCode::NotReady));

    // 04 研究ノート（資料・ノート・保存・引用・検索・削除）
    let notes = rt.open(corpus("04_research_notes"), "s-notes").unwrap();
    ok(&notes, "local.imports.add", json!({ "file_ref": "pick-1" }));
    let import = doc(&store, "s-notes", "local.imports")["items"][0]["id"].as_str().unwrap().to_string();
    ok(&notes, "local.notes.create", json!({ "title": "論文メモ" }));
    let note = doc(&store, "s-notes", "local.notes")["selected"]["id"].as_str().unwrap().to_string();
    ok(&notes, "local.notes.save", json!({ "note_id": note, "body": "結論は 3 章" }));
    ok(&notes, "local.notes.cite", json!({ "note_id": note, "import_id": import, "quote": "p.12" }));
    let d = doc(&store, "s-notes", "local.notes");
    assert_eq!((&d["selected"]["body"], &d["selected"]["citations"][0]["import_id"]), (&json!("結論は 3 章"), &json!(import)));
    let no_source = done(notes.invoke("local.notes.cite", json!({ "note_id": note, "import_id": "nowhere" }), button("research-cite")));
    assert_eq!(no_source.outcome, Outcome::Failed { error: ErrorCode::TargetNotFound });
    ok(&notes, "local.notes.search", json!({ "query": "論文" }));
    assert_eq!(doc(&store, "s-notes", "local.notes")["filter"]["note_ids"], json!([note]));
    let req = asks(notes.invoke("local.notes.delete", json!({ "note_id": note }), button("research-delete")));
    assert!(verified(&notes.approve(&req.run_id, &req.payload_hash).unwrap()));
    let d = doc(&store, "s-notes", "local.notes");
    assert_eq!((d["items"].as_array().unwrap().len(), &d["selected"]), (0, &Value::Null));
}

/// 承認の hash の土台: SHA-256 は NIST の例と一致し、正準形は key の順に依らない。時刻は RFC3339 UTC（ms）。
#[test]
fn sha256_matches_nist_vectors_and_canonical_json_ignores_key_order() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    let a: Value = serde_json::from_str(r#"{"b":1,"a":{"d":[1,2],"c":"x"}}"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"a":{"c":"x","d":[1,2]},"b":1}"#).unwrap();
    assert_eq!(canonical(&a), canonical(&b));
    assert_eq!(payload_hash("x.y", "app", "work", &a, &[]), payload_hash("x.y", "app", "work", &b, &[]));
    assert_ne!(payload_hash("x.y", "app", "work", &a, &[]), payload_hash("x.y", "app", "friends", &a, &[]));
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(951_782_400_123), "2000-02-29T00:00:00.123Z");
    assert_eq!(rfc3339(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
}

// ---- 7 層レビュー ラウンド 1 の確定指摘（RT-1〜6・RT-SEC-1 / 2）の再現。どれも直す前の実装で赤を見た ----

/// RT-1（CORE-006 / RUN-004）: 同じ app を 2 つの session で開いても ui.patch は互いの変更を消さない（両方の session が同じ revision を見る）。
/// store の最新に向けた app_version は通り、古い版に向けた patch は当てない。再起動して古い定義を渡して開いても、見えるのも土台も store の最新。
#[test]
fn ui_patch_from_two_sessions_of_the_same_app_keeps_both_changes() {
    let (rt, store) = start(|_| {});
    let a = rt.open(corpus("01_timer_local"), "s-a").unwrap();
    let b = rt.open(corpus("01_timer_local"), "s-b").unwrap();
    let insert = |key: &str| json!({ "operations": [ { "op": "insert", "target_key": "timer-root", "value": { "key": key, "component": "text", "label": key } } ] });
    let keys = || -> Vec<String> {
        let latest = store.lock().unwrap().latest_revision("example.v3.timer_local").unwrap().unwrap();
        latest.json["composition"]["root"]["children"].as_array().unwrap().iter().filter_map(|n| n["key"].as_str().map(String::from)).collect()
    };
    let has = |k: &[String], key: &str| k.iter().any(|x| x == key);
    assert!(verified(&done(a.invoke(UI_PATCH, insert("from-a"), button("patch")))));
    assert!(verified(&done(b.invoke(UI_PATCH, insert("from-b"), button("patch")))));
    let k = keys();
    assert!(has(&k, "from-a") && has(&k, "from-b"), "B の patch が A の変更を消した: {k:?}");
    assert_eq!((a.revision().version(), b.revision().version()), (3, 3));
    assert_eq!(a.revision(), b.revision());
    // store の最新（3）に向けた patch は通る。同じ 3 に向けた 2 つ目は古い版になったので当てない
    assert!(verified(&done(b.invoke(UI_PATCH, json!({ "app_version": 3, "operations": [ { "op": "remove", "target_key": "from-a" } ] }), button("patch")))));
    let stale = done(a.invoke(UI_PATCH, json!({ "app_version": 3, "operations": [ { "op": "remove", "target_key": "from-b" } ] }), button("patch")));
    assert_eq!(stale.outcome, Outcome::Failed { error: ErrorCode::InvalidInput });
    // 再起動（同じ store に新しい Runtime）で corpus の古い定義（version 1）を渡して開いても、見えるのは store の最新（4）
    let rt2 = Runtime::start(store.clone(), ActionRegistry::with_local_actions()).unwrap();
    let c = rt2.open(corpus("01_timer_local"), "s-c").unwrap();
    assert_eq!(c.revision().version(), 4, "開き直した app が store の最新でなく渡した定義を見ている");
    assert!(verified(&done(c.invoke(UI_PATCH, insert("from-c"), button("patch")))));
    // 同じ DB を開いたままの前の Runtime（a の写しは 4 のまま）が打っても、土台は store の最新（5）で c の変更を消さない
    assert!(verified(&done(a.invoke(UI_PATCH, insert("from-a2"), button("patch")))));
    let k = keys();
    assert!(has(&k, "from-b") && has(&k, "from-c") && has(&k, "from-a2") && !has(&k, "from-a"), "{k:?}");
    assert_eq!((c.revision().version(), a.revision().version()), (5, 6));
}

/// RT-2（RUN-008）: cancel が Cancelled（副作用の前に取り消した）と答えた run は、その後で dispatch されない。
/// 承認の要らない run は planning で保存しない（running と cancel flag を同じ lock の中で置く）ので、外から planning が見えない。
// ponytail: 窓を当てに行く試行の繰り返し（差し込める継ぎ目が無い）。直した後は planning が保存されないので常に緑。approve の道
// （running を保存してから flag を置くまで）は窓が lock 無しの 1 文で、test から当てられない
#[test]
fn cancel_that_answers_cancelled_is_never_overtaken_by_dispatch() {
    let executed: Arc<Mutex<Vec<String>>> = Arc::default();
    let e = executed.clone();
    let (rt, _) = start(move |reg| {
        let race = handler(
            move |ctx, _| {
                if ctx.cancelled() {
                    return Err(ErrorCode::Cancelled);
                }
                e.lock().unwrap().push(ctx.run_id.clone());
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.cancel.race", ActionContract::local(Effect::LocalWrite), race);
    });
    for i in 0..100 {
        let app = Arc::new(rt.open(test_app("work"), &format!("race-{i}")).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let (a, r, s) = (app.clone(), rt.clone(), stop.clone());
        // run が見えた瞬間に、見えた状態を控えて 1 回だけ cancel する
        let canceller = std::thread::spawn(move || loop {
            if let Some(run) = a.runs().unwrap().pop() {
                return Some((run.run.id.clone(), run.run.status, r.cancel(&run.run.id).unwrap()));
            }
            if s.load(SeqCst) {
                return None;
            }
        });
        let result = app.invoke("test.cancel.race", json!({}), button("race"));
        stop.store(true, SeqCst);
        if let Some((run_id, seen, answer)) = canceller.join().unwrap() {
            // planning は cancel が「副作用の前」と答える段。外から見えた時点で、その答えを executor の dispatch が覆いうる
            assert_ne!(seen, RunStatus::Planning, "試行 {i}: 承認の要らない run が planning で保存されていた（{answer:?}・{result:?}）");
            if answer == CancelResult::Cancelled {
                assert!(!executed.lock().unwrap().contains(&run_id), "試行 {i}: Cancelled と答えた run を dispatch した: {result:?}");
                assert_eq!(status(&rt, &run_id), RunStatus::Cancelled, "試行 {i}");
            }
        }
    }
}

/// RT-3（CD-19・RUN-005）: handler が時間切れになっても、捨てた thread が終わるまで lease を返さない（同じ window に 2 つの handler が同時に触らない）。
#[test]
fn timed_out_handler_keeps_its_lease_until_its_thread_ends() {
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
    let inside = Arc::new(AtomicBool::new(false));
    let (overlapped, next_calls) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let (i1, i2, o, n) = (inside.clone(), inside.clone(), overlapped.clone(), next_calls.clone());
    let (rt, _) = start(move |reg| {
        let window: fn(&Value) -> Vec<LeaseKey> = |input| vec![LeaseKey::Window(input["window"].as_str().unwrap_or_default().into())];
        let slow = handler(
            move |_, _| {
                i1.store(true, SeqCst);
                let _ = entered_tx.lock().unwrap().send(());
                let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
                i1.store(false, SeqCst);
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.lease.slow", ActionContract { leases: window, timeout: Duration::from_millis(100), ..ActionContract::local(Effect::LocalWrite) }, slow);
        let next = handler(
            move |_, _| {
                o.fetch_or(i2.load(SeqCst), SeqCst);
                n.fetch_add(1, SeqCst);
                Ok(json!({}))
            },
            observed,
        );
        reg.register("test.lease.next", ActionContract { leases: window, timeout: Duration::from_secs(20), ..ActionContract::local(Effect::LocalWrite) }, next);
    });
    let app = Arc::new(rt.open(test_app("work"), "s1").unwrap());
    let first = done(app.invoke("test.lease.slow", json!({ "window": "w1" }), button("slow")));
    assert!(matches!(first.outcome, Outcome::EffectUnknown { .. }), "{:?}", first.outcome);
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(); // 捨てた thread は w1 の中にいる
    let a = app.clone();
    let next = std::thread::spawn(move || a.invoke("test.lease.next", json!({ "window": "w1" }), button("next")));
    wait_for("次の run が実行待ちになる", || {
        app.runs().unwrap().into_iter().find(|r| r.action_id() == "test.lease.next" && events(&rt, &r.run.id).iter().any(|e| e == "LeaseQueued")).map(|_| ())
    });
    assert_eq!(next_calls.load(SeqCst), 0, "時間切れの thread が w1 を触っている間に次の handler が動いた");
    release_tx.send(()).unwrap();
    assert!(verified(&done(next.join().unwrap())));
    assert!(!overlapped.load(SeqCst), "同じ window に 2 つの handler が同時に触った");
    assert_eq!(next_calls.load(SeqCst), 1);
}

/// RT-4（RUN-004 / 005）: 受理された書込を verify が確かめられない（Failed）・拒否と読む（Rejected）→ uncertain。
/// failed にすると idempotency key が外れて、同じ Agent 提案がもう一度書く。
#[test]
fn accepted_write_whose_verify_fails_stays_uncertain_and_keeps_its_key() {
    let writes = Arc::new(AtomicUsize::new(0));
    let (w1, w2) = (writes.clone(), writes.clone());
    let (rt, _) = start(move |reg| {
        let fails = handler(move |_, _| { w1.fetch_add(1, SeqCst); Ok(json!({})) }, |_, _, _| Outcome::Failed { error: ErrorCode::Internal });
        reg.register("test.verify.fails", ActionContract::local(Effect::LocalWrite), fails);
        let rejects = handler(move |_, _| { w2.fetch_add(1, SeqCst); Ok(json!({})) }, |_, _, _| Outcome::Rejected { reason: "読み直すと拒否されていた".into() });
        reg.register("test.verify.rejects", ActionContract::local(Effect::LocalWrite), rejects);
    });
    let app = rt.open(test_app("work"), "s1").unwrap();
    for (n, id) in ["test.verify.fails", "test.verify.rejects"].into_iter().enumerate() {
        let first = done(app.invoke(id, json!({}), agent("sg-v")));
        assert!(matches!(first.outcome, Outcome::Unverified { .. }), "{id}: {:?}", first.outcome);
        assert_eq!(status(&rt, &first.run_id), RunStatus::Uncertain, "{id}");
        assert_eq!(done(app.invoke(id, json!({}), agent("sg-v"))), first, "{id}: 同じ提案で 2 回目を書いた");
        assert_eq!(writes.load(SeqCst), n + 1, "{id}");
    }
}

/// RT-5（RUN-001 / 002）: 承認を待つ間に adapter が revoke されたら、承認しても dispatch しない（invoke と同じ NotReady・承認待ちのまま）。戻れば承認できる。
#[test]
fn approve_rechecks_adapter_readiness() {
    let sends = Arc::new(AtomicUsize::new(0));
    let s = sends.clone();
    let (rt, _) = start(move |reg| {
        let send = ActionContract { adapter: "mail", scopes: &["mail.send"], ..ActionContract::local(Effect::ExternalSend) };
        reg.register("mail.messages.send", send, handler(move |_, _| { s.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
    });
    let app = rt.open(test_app("work"), "s1").unwrap();
    app.grant(grant("g-send", "mail.send", None)).unwrap();
    let req = asks(app.invoke("mail.messages.send", json!({ "to": "a@example.com", "body": "hi" }), button("send")));
    rt.registry().set_adapter_state("mail", CapabilityReadiness::Revoked);
    assert_eq!(app.approve(&req.run_id, &req.payload_hash), Err(ErrorCode::NotReady));
    assert_eq!(sends.load(SeqCst), 0, "revoke された adapter へ送った");
    assert_eq!(status(&rt, &req.run_id), RunStatus::WaitingApproval);
    rt.registry().set_adapter_state("mail", CapabilityReadiness::Ready);
    assert!(verified(&app.approve(&req.run_id, &req.payload_hash).unwrap()));
    assert_eq!(sends.load(SeqCst), 1);
}

/// RT-6（CORE-003）: schema に maximum の無い app が timer の長さに巨大な値を渡しても、掛け算が溢れて handler が落ちない（InvalidInput・書かない）。
#[test]
fn huge_timer_duration_is_invalid_input_not_a_panic() {
    let (rt, store) = start(|_| {});
    let app = rt.open(test_app("work"), "s1").unwrap();
    let rc = done(app.invoke("local.timer.save_settings", json!({ "duration_minutes": u64::MAX }), button("timer-settings")));
    assert_eq!(rc.outcome, Outcome::Failed { error: ErrorCode::InvalidInput });
    assert_eq!(status(&rt, &rc.run_id), RunStatus::Failed);
    assert_eq!(doc(&store, "s1", "local.timer"), Value::Null);
    // 裏: 掛けても溢れない値は通る
    assert!(verified(&done(app.invoke("local.timer.save_settings", json!({ "duration_minutes": 240 }), button("timer-settings")))));
}

/// RT-SEC-1（RUN-002）: approve / deny は run を作った app・session・space の AppRuntime からだけ効く。
/// 在庫 app の Agent 提案（timer の node を消す ui.patch）を timer の画面で承認しても timer に当たらない。
#[test]
fn approve_and_deny_only_work_from_the_runs_own_app_session_and_space() {
    let (rt, store) = start(|_| {});
    let timer = rt.open(corpus("01_timer_local"), "s-timer").unwrap();
    let inv = rt.open(corpus("02_inventory"), "s-inv").unwrap(); // suggest_only: Agent の書込は承認待ち
    let req = asks(inv.invoke(UI_PATCH, json!({ "operations": [ { "op": "remove", "target_key": "timer-reset" } ] }), agent("sg-x")));
    let inv_elsewhere = rt.open(corpus("02_inventory"), "s-inv-2").unwrap();
    for other in [&timer, &inv_elsewhere] {
        // 別 app（focus の移った timer の画面）・同じ app の別 session
        assert_eq!(other.approve(&req.run_id, &req.payload_hash), Err(ErrorCode::TargetNotFound), "{}", other.session_id());
        assert_eq!(other.deny(&req.run_id), Err(ErrorCode::TargetNotFound), "{}", other.session_id());
    }
    // 同じ提案（同じ key）を別 session で打っても、承認できない他人の ApprovalRequest は返さない（key は縛ったまま・2 つ目の run も作らない）
    let again = inv_elsewhere.invoke(UI_PATCH, json!({ "operations": [ { "op": "remove", "target_key": "timer-reset" } ] }), agent("sg-x"));
    assert_eq!(again, Err(ErrorCode::RateLimited));
    assert!(inv_elsewhere.runs().unwrap().is_empty());
    // 別 space・別 app の run: open は同じ app を別 space で開かず（RT-SEC-2）、session は 1 つの app の物なので、
    // 保存された run を書き換えて照合だけを測る（hash の照合より先に止まる = ApprovalDenied ではなく TargetNotFound）
    let original = rt.run(&req.run_id).unwrap().unwrap();
    let tampers: [fn(&mut rcpt::RunRecord); 2] = [|r| r.space_id = "friends".into(), |r| r.run.app_id = "example.v3.timer_local".into()];
    for tamper in tampers {
        let mut moved = original.clone();
        tamper(&mut moved);
        rcpt::save(&store.lock().unwrap(), &moved).unwrap();
        assert_eq!(inv.approve(&req.run_id, &req.payload_hash), Err(ErrorCode::TargetNotFound), "{moved:?}");
        assert_eq!(inv.deny(&req.run_id), Err(ErrorCode::TargetNotFound), "{moved:?}");
    }
    rcpt::save(&store.lock().unwrap(), &original).unwrap();
    assert_eq!(status(&rt, &req.run_id), RunStatus::WaitingApproval);
    assert!(timer.revision().definition().composition.root.find("timer-reset").is_some(), "在庫 app の承認が timer に当たった");
    assert_eq!(store.lock().unwrap().list_revisions("example.v3.timer_local").unwrap().len(), 1);
    // 自分の画面からなら承認できる（在庫に timer-reset は無いので patch は InvalidInput で failed。timer は変わらない）
    let rc = inv.approve(&req.run_id, &req.payload_hash).unwrap();
    assert_eq!(rc.outcome, Outcome::Failed { error: ErrorCode::InvalidInput });
    assert_eq!(store.lock().unwrap().list_revisions("example.v3.timer_local").unwrap().len(), 1);
}

/// RT-SEC-2（RUN-002・ADR-005）: open は渡された定義を正にしない。同じ id で宣言の違う定義（grant 済みの scope を別の action に
/// 付け替えた）・space を書き換えた定義・別 app の session は開けない（その id と space に置いた grant を継がない）。再起動した
/// process で偽物が先に開いても同じ。composition / version だけが違う定義（ui.patch の前の写し）は同じ app として開け、
/// revision と space は store の物を使う。
#[test]
fn open_refuses_another_definition_under_an_existing_app_id() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let (rt, store) = start(move |reg| mail_reads(reg, &c));
    let work = rt.open(test_app("work"), "s-work").unwrap();
    work.grant(grant("g-read", "mail.read", None)).unwrap();
    assert!(verified(&done(work.invoke("mail.inbox.read", json!({}), button("inbox")))));
    // 偽物: 同じ id・同じ space を名乗り、grant 済みの mail.read を archive の読取に付け替えた（元の宣言は mail.archive が要る）
    let mut def = test_app("work").definition().clone();
    def.actions.iter_mut().find(|a| a.id == "mail.archive.read").unwrap().required_scopes = vec!["mail.read".into()];
    let impostor = AppRevision::new(def).unwrap();
    assert_eq!(rt.open(impostor.clone(), "s-impostor").err(), Some(ErrorCode::PermissionDenied));
    // 再起動した process（同じ store に新しい Runtime）で偽物が先に開いても同じ（前は先に開いた定義がその id の revision になった）
    let mut reg = ActionRegistry::with_local_actions();
    mail_reads(&mut reg, &calls);
    let rt2 = Runtime::start(store.clone(), reg).unwrap();
    rt2.registry().set_adapter_state("mail", CapabilityReadiness::Ready);
    assert_eq!(rt2.open(impostor, "s-impostor").err(), Some(ErrorCode::PermissionDenied));
    // space の書き換え（Work の app を Friends として開く）と、別 app の session（s-work は test.mail の物）
    assert_eq!(rt2.open(test_app("friends"), "s-friends").err(), Some(ErrorCode::PermissionDenied));
    assert_eq!(rt2.open(corpus("01_timer_local"), "s-work").err(), Some(ErrorCode::PermissionDenied));
    assert_eq!(calls.load(SeqCst), 1, "偽物が work の grant で読んだ");
    assert!(store.lock().unwrap().get(Table::Sessions, "s-impostor").unwrap().is_none(), "拒否した open が session を作った");
    // 裏: ui.patch の後でも、composition / version だけが違う元の定義は同じ app として開ける。revision と space は store の物
    let insert = json!({ "operations": [ { "op": "insert", "target_key": "root", "value": { "key": "hello", "component": "text", "label": "受信箱" } } ] });
    assert!(verified(&done(work.invoke(UI_PATCH, insert, button("patch")))));
    let again = rt2.open(test_app("work"), "s-work").unwrap();
    assert_eq!((again.revision().version(), again.space_id()), (2, "work"));
    assert!(verified(&done(again.invoke("mail.inbox.read", json!({}), button("inbox")))));
    assert_eq!(calls.load(SeqCst), 2);
}

// ---- 7 層レビュー ラウンド 2 の確定指摘（RT-R2-1〜3・RT-SEC-R2-1）の再現。どれも直す前の実装で赤を見た ----

/// RT-R2-1（CD-19・RUN-005）: lease を持つ読取が時間切れで再試行する時、手を離した前の attempt の thread が終わるまで次の attempt を
/// 出さない（同じ run の attempt の間でも、同じ window に 2 つの handler が同時に触らない）。終われば再試行して verified。
#[test]
fn read_retry_waits_until_the_timed_out_attempt_leaves_the_lease() {
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
    let inside = Arc::new(AtomicBool::new(false));
    let (overlapped, calls) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let (i, o, n) = (inside.clone(), overlapped.clone(), calls.clone());
    let (rt, _) = start(move |reg| {
        let read = handler(
            move |_, _| {
                if n.fetch_add(1, SeqCst) == 0 {
                    // 1 回目: 時間切れになった後も window の中に居る
                    i.store(true, SeqCst);
                    let _ = entered_tx.lock().unwrap().send(());
                    let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
                    i.store(false, SeqCst);
                } else {
                    o.fetch_or(i.load(SeqCst), SeqCst);
                }
                Ok(json!({}))
            },
            observed,
        );
        let contract = ActionContract {
            leases: |input: &Value| vec![LeaseKey::Window(input["window"].as_str().unwrap_or_default().into())],
            timeout: Duration::from_secs(2),
            max_retries: 1,
            backoff: Duration::from_millis(1),
            ..ActionContract::local(Effect::LocalRead)
        };
        reg.register("test.lease.read", contract, read);
    });
    let app = Arc::new(rt.open(test_app("work"), "s1").unwrap());
    let a = app.clone();
    let reading = std::thread::spawn(move || a.invoke("test.lease.read", json!({ "window": "w1" }), button("read")));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let run_id = wait_for("1 回目が時間切れで再試行に回る", || {
        app.runs().unwrap().into_iter().find(|r| events(&rt, &r.run.id).iter().any(|e| e == "RetryScheduled")).map(|r| r.run.id)
    });
    std::thread::sleep(Duration::from_millis(300)); // backoff は 1ms: 待たない実装はここまでに 2 回目を出している
    assert_eq!(calls.load(SeqCst), 1, "時間切れの attempt が window の中に居る間に次の attempt を出した");
    release_tx.send(()).unwrap();
    let rc = done(reading.join().unwrap());
    assert!(verified(&rc), "{:?}", rc.outcome);
    assert_eq!((rc.attempt_seq, calls.load(SeqCst)), (2, 2));
    assert!(!overlapped.load(SeqCst), "同じ window に 2 つの handler が同時に触った");
    assert!(events(&rt, &run_id).iter().any(|e| e == "RetryWaiting"), "待った理由が run log に無い: {:?}", events(&rt, &run_id));
}

/// RT-R2-2（CD-09・CD-18）: 同じ DB を開いた別の process（別の接続）が同じ app の revision を積んでいる間も、ui.patch は読んだ
/// revision と書く revision の間に他の書き込みを入れない（読む・当てる・書くを 1 回の BEGIN IMMEDIATE の中で）。どの revision も
/// 1 つ前の revision の node を全部持つ（どちらの変更も消えない）。
// ponytail: 別の接続の writer を回し続けて窓を当てる（patch に差し込める継ぎ目が無い）。直す前の実装の patch の読みと書きの間に
// 50ms の sleep を入れて赤を見た（直した後は同じ sleep を transaction の中に置いても緑）
#[test]
fn ui_patch_does_not_lose_a_revision_written_by_another_connection() {
    const APP: &str = "example.v3.timer_local";
    const MINE: usize = 10;
    let path = std::env::temp_dir().join(format!("kangae-rt-{}-{}.sqlite3", std::process::id(), storage::new_id()));
    let store = Arc::new(Mutex::new(Store::open(&path).unwrap()));
    let rt = Runtime::start(store.clone(), ActionRegistry::with_local_actions()).unwrap();
    let app = rt.open(corpus("01_timer_local"), "s-p1").unwrap();
    let node = |key: String| json!({ "key": key, "component": "text", "label": "x" });
    // 別 process の代わり: 別の接続で「最新を読み、node を 1 つ足して書く」を回し続ける（こちらも transaction の中で）
    let other = Store::open(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let hammer = std::thread::spawn(move || {
        let mut n = 0;
        while !s.load(SeqCst) {
            other
                .immediate(|e| e, |st| {
                    let mut json = st.latest_revision(APP)?.expect("open が 1 行目を置く").json;
                    json["composition"]["root"]["children"].as_array_mut().expect("children").push(node(format!("other-{n}")));
                    st.put_revision(APP, &json, None).map(|_| ())
                })
                .map_err(|e: StoreError| e.to_string())
                .unwrap();
            n += 1;
            std::thread::sleep(Duration::from_millis(5)); // 書く lock を握り続けない（こちらの run の書込が busy_timeout を超えないように）
        }
        n
    });
    for i in 0..MINE {
        let insert = json!({ "operations": [ { "op": "insert", "target_key": "timer-root", "value": node(format!("mine-{i}")) } ] });
        let rc = done(app.invoke(UI_PATCH, insert, button("patch")));
        assert!(verified(&rc), "{i}: {:?}", rc.outcome);
    }
    stop.store(true, SeqCst);
    let others = hammer.join().unwrap();
    let keys = |json: &Value| -> BTreeSet<String> {
        json["composition"]["root"]["children"].as_array().unwrap().iter().filter_map(|c| c["key"].as_str().map(String::from)).collect()
    };
    let revs = store.lock().unwrap().list_revisions(APP).unwrap();
    for pair in revs.windows(2) {
        let (before, after) = (keys(&pair[0].json), keys(&pair[1].json));
        let lost: Vec<&String> = before.difference(&after).collect();
        assert!(lost.is_empty(), "revision {} が {} の node {lost:?} を消した（古い revision に当てて書いた）", pair[1].revision, pair[0].revision);
    }
    let last = keys(&revs.last().unwrap().json);
    for key in (0..MINE).map(|i| format!("mine-{i}")).chain((0..others).map(|n| format!("other-{n}"))) {
        assert!(last.contains(&key), "{key} が最新に無い");
    }
    drop(app);
    drop(rt);
    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

/// RT-R2-3（RUN-005・RUN-002）: idempotency key は効果が届く範囲で縛り、他の session の Receipt は返さない。local の書込（中身は
/// session の ui_state）は session ごと: 別 session が同じ key を打てば自分の ui_state に書く。外への送信は session に依らず 1 回:
/// 別 session は送り直さず、承認待ちの run も送った後の Receipt も受け取らない（RateLimited）。
#[test]
fn idempotency_key_never_hands_another_sessions_receipt() {
    let sends = Arc::new(AtomicUsize::new(0));
    let s = sends.clone();
    let (rt, store) = start(move |reg| {
        let send = ActionContract { adapter: "mail", scopes: &["mail.send"], ..ActionContract::local(Effect::ExternalSend) };
        reg.register("mail.messages.send", send, handler(move |_, _| { s.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
    });
    let items = |session: &str| doc(&store, session, "local.inventory")["items"].as_array().map_or(0, Vec::len);
    let a = rt.open(corpus("02_inventory"), "s-a").unwrap();
    let b = rt.open(corpus("02_inventory"), "s-b").unwrap();
    let add = json!({ "name": "x" });
    let ra = done(a.invoke_keyed("local.inventory.add", add.clone(), button("inventory-add"), Some("k")));
    let rb = done(b.invoke_keyed("local.inventory.add", add.clone(), button("inventory-add"), Some("k")));
    assert_ne!(rb.run_id, ra.run_id, "B に A の Receipt を返した（B では実行していない）");
    assert!(verified(&rb), "{:?}", rb.outcome);
    assert_eq!((items("s-a"), items("s-b")), (1, 1), "B の ui_state に書いていない");
    assert_eq!(b.runs().unwrap().len(), 1);
    // 同じ session の 2 回目は送り直さない（A には A の Receipt）
    assert_eq!(done(a.invoke_keyed("local.inventory.add", add, button("inventory-add"), Some("k"))), ra);
    assert_eq!(items("s-a"), 1);
    // 外への送信: 同じ key は app で 1 回
    let ma = rt.open(test_app("work"), "s-mail-a").unwrap();
    let mb = rt.open(test_app("work"), "s-mail-b").unwrap();
    ma.grant(grant("g-send", "mail.send", None)).unwrap();
    let msg = json!({ "to": "a@example.com", "body": "hi" });
    let req = asks(ma.invoke_keyed("mail.messages.send", msg.clone(), button("send"), Some("k-send")));
    assert_eq!(mb.invoke_keyed("mail.messages.send", msg.clone(), button("send"), Some("k-send")), Err(ErrorCode::RateLimited));
    let sent = ma.approve(&req.run_id, &req.payload_hash).unwrap();
    assert!(verified(&sent));
    assert_eq!(mb.invoke_keyed("mail.messages.send", msg.clone(), button("send"), Some("k-send")), Err(ErrorCode::RateLimited), "B に A の送信の Receipt を見せた");
    assert_eq!(done(ma.invoke_keyed("mail.messages.send", msg, button("send"), Some("k-send"))), sent);
    assert_eq!(sends.load(SeqCst), 1);
    assert!(mb.runs().unwrap().is_empty());
}

/// RT-SEC-R2-1（RUN-002・ADR-005）: 外部 action に要る scope は registry の契約が決める（app の宣言は足せるが減らせない）。
/// composer / LLM が mail の読取・送信に無害に見える scope（weather.read）を書き、利用者がそれだけに同意しても mail には触れない。
/// 契約に scope の無い外部 action（adapter の登録の誤り）は、app が scope を宣言して grant があっても呼べない。
#[test]
fn external_action_needs_the_scopes_its_registry_contract_names() {
    let (calls, sends) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, c2, s) = (calls.clone(), calls.clone(), sends.clone());
    let (rt, _) = start(move |reg| {
        mail_reads(reg, &c);
        let send = ActionContract { adapter: "mail", scopes: &["mail.send"], ..ActionContract::local(Effect::ExternalSend) };
        reg.register("mail.messages.send", send, handler(move |_, _| { s.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
        let bare = ActionContract { adapter: "mail", ..ActionContract::local(Effect::ExternalRead) };
        reg.register("mail.archive.read", bare, handler(move |_, _| { c2.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
    });
    // 偽装: mail の読取と送信の required_scopes を weather.read に書き換えた app（宣言された scope なので grant は作れる）
    let mut def = test_app_as("test.mail.disguised", "work").definition().clone();
    for action in def.actions.iter_mut().filter(|a| a.id == "mail.inbox.read" || a.id == "mail.messages.send") {
        action.required_scopes = vec!["weather.read".into()];
    }
    let disguised = rt.open(AppRevision::new(def).unwrap(), "s-disguised").unwrap();
    let as_disguised = |g: PermissionGrant| PermissionGrant { app_id: "test.mail.disguised".into(), ..g };
    disguised.grant(as_disguised(grant("g-weather", "weather.read", None))).unwrap();
    assert_eq!(disguised.invoke("mail.inbox.read", json!({}), button("inbox")), Err(ErrorCode::PermissionDenied));
    assert_eq!(disguised.invoke("mail.inbox.read", json!({}), agent("sg-1")), Err(ErrorCode::PermissionDenied));
    let msg = json!({ "to": "a@example.com", "body": "hi" });
    assert_eq!(disguised.invoke("mail.messages.send", msg.clone(), button("send")), Err(ErrorCode::PermissionDenied), "偽の scope で承認の問いを出した");
    assert_eq!(calls.load(SeqCst), 0, "関係の無い scope の grant で mail を読んだ");
    // 本当の scope は app が宣言していないので grant も作れない（接続の画面に本当の scope が出ない限り通らない）
    assert_eq!(disguised.grant(as_disguised(grant("g-read", "mail.read", None))), Err(ErrorCode::PermissionDenied));
    // 契約に scope の無い外部の読取は、app の宣言と grant があっても呼べない
    let honest = rt.open(test_app("work"), "s-honest").unwrap();
    honest.grant(grant("g-archive", "mail.archive", None)).unwrap();
    assert_eq!(honest.invoke("mail.archive.read", json!({}), button("archive")), Err(ErrorCode::PermissionDenied));
    // 裏: 正直な app は契約の scope の grant で読め、送信の承認の問いには契約の scope が出る
    honest.grant(grant("g-read", "mail.read", None)).unwrap();
    assert!(verified(&done(honest.invoke("mail.inbox.read", json!({}), button("inbox")))));
    honest.grant(grant("g-send", "mail.send", None)).unwrap();
    let req = asks(honest.invoke("mail.messages.send", msg, button("send")));
    assert_eq!(req.scopes, vec!["mail.send".to_string()]);
    assert_eq!((calls.load(SeqCst), sends.load(SeqCst)), (1, 0));
}

/// RT-R3-1（RT-SEC-R3-1 と同じ穴）: 外部 adapter の destructive も、契約に scope が無ければ通さない。is_external の 3 値だけを見ると、
/// 外部の消す操作を scope 無しで登録した時に、app が宣言した無関係な scope（weather.read）の grant と承認 1 回で消せた。
#[test]
fn external_destructive_needs_contract_scopes() {
    let deletes = Arc::new(AtomicUsize::new(0));
    let d = deletes.clone();
    let (rt, _) = start(move |reg| {
        let bare = ActionContract { adapter: "mail", ..ActionContract::local(Effect::Destructive) };
        reg.register("mail.messages.delete", bare, handler(move |_, _| { d.fetch_add(1, SeqCst); Ok(json!({})) }, observed));
    });
    let mut def = test_app_as("test.mail.delete", "work").definition().clone();
    def.actions.push(serde_json::from_value(ext("mail.messages.delete", "destructive", &["weather.read"])).unwrap());
    let app = rt.open(AppRevision::new(def).unwrap(), "s-delete").unwrap();
    app.grant(PermissionGrant { app_id: "test.mail.delete".into(), ..grant("g-weather", "weather.read", None) }).unwrap();
    assert_eq!(app.invoke("mail.messages.delete", json!({ "id": "m1" }), button("delete")), Err(ErrorCode::PermissionDenied), "契約に scope の無い外部の destructive が gate を通った");
    assert_eq!(deletes.load(SeqCst), 0);
}

/// RT-R3-2（CD-09: store が正）: 同じ DB を開いた別の process（ここでは別の Runtime）が ui.patch で revision を進めたら、
/// こちらの process で同じ app を開き直した時に最新の revision が見える（process の中の古い写しを返さない）。
#[test]
fn open_sees_the_revision_another_process_wrote() {
    let path = std::env::temp_dir().join(format!("kangae-rt-open-{}-{}.sqlite3", std::process::id(), storage::new_id()));
    let p1 = Runtime::start(Arc::new(Mutex::new(Store::open(&path).unwrap())), ActionRegistry::with_local_actions()).unwrap();
    let p2 = Runtime::start(Arc::new(Mutex::new(Store::open(&path).unwrap())), ActionRegistry::with_local_actions()).unwrap();
    let first = p1.open(corpus("01_timer_local"), "s-p1").unwrap();
    let v1 = first.revision().version();
    let other = p2.open(corpus("01_timer_local"), "s-p2").unwrap();
    let insert = json!({ "operations": [ { "op": "insert", "target_key": "timer-root", "value": { "key": "from-p2", "component": "text", "label": "x" } } ] });
    assert!(verified(&done(other.invoke(UI_PATCH, insert, button("patch")))));
    let again = p1.open(corpus("01_timer_local"), "s-p1-b").unwrap();
    assert!(again.revision().version() > v1, "別 process の ui.patch（v{}）が見えない: v{}", other.revision().version(), again.revision().version());
    assert!(again.revision().definition().composition.root.find("from-p2").is_some());
    drop((first, other, again, p1, p2));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
