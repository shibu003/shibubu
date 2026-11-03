//! dispatch（RUN-004 / 005 / 008・CD-09 (i)）: lease → Attempt{outcome: None} を保存 → handler（別 thread・timeout）
//! → 受理なら verifying で読み直す → Outcome を埋めて Receipt と同じ 1 回の保存で書く。
//! 再試行は読取だけ（Timeout / ConnectionLost / RateLimited・回数は契約）。時間切れの attempt の thread が lease を持っている間は
//! 次の attempt を出さない（契約の timeout まで待つ）。書込の timeout / 接続断 / 落ちた は EffectUnknown。
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::domain::run::{Attempt, ErrorCode, Outcome, Receipt, RunStatus};
use super::lease::{LeaseGuard, LeaseWait};
use super::receipt::{self, RunRecord};
use super::registry::{is_read, Ctx, Registered};
use super::{lock, now_rfc3339, AppRuntime};

/// lease の実行待ちの上限（DESIGN-runtime §7 Q5 の 30 分）。
const QUEUE_MAX: Duration = Duration::from_secs(30 * 60);

/// running の run を受けて最後まで動かす。invoke / approve が running の保存と同じ store の lock の中で `flag` を置いてから渡す
/// （cancel が running を見た時は必ず flag が在る。planning を見て Cancelled と答えた run をここで上書きしない）。外すのはここ。
/// Ok = attempt が 1 つ以上ある（Receipt）。Err = dispatch の前に終わった（cancel / lease の上限 / store の失敗）。
pub(super) fn execute(app: &AppRuntime, rec: RunRecord, reg: &Registered, flag: Arc<AtomicBool>) -> Result<Receipt, ErrorCode> {
    let run_id = rec.run.id.clone();
    let result = run(app, rec, reg, &flag);
    lock(&app.rt.running).remove(&run_id);
    result
}

fn run(app: &AppRuntime, mut rec: RunRecord, reg: &Registered, flag: &Arc<AtomicBool>) -> Result<Receipt, ErrorCode> {
    let rt = &app.rt;
    let mut keys = (reg.contract.leases)(&rec.input);
    keys.sort();
    keys.dedup();
    // lease は Arc で持ち、handler の thread にも 1 つずつ渡す: 時間切れで手を離した thread が終わるまで返らない（CD-19）
    let lease = if keys.is_empty() {
        None
    } else {
        let guard = match rt.leases.try_acquire(&keys, &rec.run.id) {
            Ok(guard) => guard,
            Err(owner) => {
                receipt::event(&rt.store(), &rec, "LeaseQueued", "実行待ち（対象を他の run が使っている）", json!({ "held_by": owner }))?;
                match rt.leases.acquire(&keys, &rec.run.id, flag, Instant::now() + QUEUE_MAX) {
                    Ok(guard) => guard,
                    Err(LeaseWait::Cancelled) => return stop(app, rec, RunStatus::Cancelled, ErrorCode::Cancelled, "実行待ちのまま取り消した（dispatch していない）"),
                    Err(LeaseWait::TimedOut) => return stop(app, rec, RunStatus::Failed, ErrorCode::Timeout, "実行待ちが上限を超えた（dispatch していない）"),
                }
            }
        };
        let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
        receipt::event(&rt.store(), &rec, "LeaseAcquired", "対象を押さえた", json!({ "leases": names }))?;
        Some(Arc::new(guard))
    };
    let read = is_read(reg.contract.effect);
    loop {
        if flag.load(Ordering::SeqCst) {
            return stop(app, rec, RunStatus::Cancelled, ErrorCode::Cancelled, "dispatch の前に取り消した");
        }
        let seq = rec.attempt_count() as u32 + 1;
        if let Some(step) = rec.run.steps.first_mut() {
            step.attempts.push(Attempt { seq, started_at: now_rfc3339(), finished_at: None, outcome: None });
        }
        {
            // write-ahead: 「試みた」を保存してから handler を呼ぶ。ここで落ちても再起動で EffectUnknown になる（CD-09 (i)(ii)）
            let store = rt.store();
            receipt::save(&store, &rec)?;
            receipt::event(&store, &rec, "ActionDispatched", "dispatch", json!({ "attempt": seq }))?;
        }
        let ctx = Ctx {
            run_id: rec.run.id.clone(),
            session_id: app.session_id.clone(),
            deadline: Instant::now() + reg.contract.timeout,
            store: rt.store.clone(),
            app: app.app.clone(),
            cancel: flag.clone(),
        };
        let (handler, c, input, held) = (reg.handler.clone(), ctx.clone(), rec.input.clone(), lease.clone());
        let result = match bounded(reg.contract.timeout, move || {
            let r = handler.execute(&c, &input);
            drop(held); // thread が終わった時に lease の持ち手を 1 つ返す（時間切れの後でも）
            r
        }) {
            Bounded::Done(r) => r,
            Bounded::TimedOut => Err(ErrorCode::Timeout),
            Bounded::Panicked if read => Err(ErrorCode::Internal),
            Bounded::Panicked => return conclude(app, rec, Outcome::EffectUnknown { reason: "handler が途中で落ちた。届いたか分からない（自動で送り直さない）".into() }),
        };
        let outcome = match result {
            Ok(accepted) => {
                // 受理 ≠ 完了（RUN-004）。verifying に進め、handler に対象を読み直させて決める。
                let late = flag.load(Ordering::SeqCst);
                receipt::set_status(&mut rec, RunStatus::Verifying)?;
                {
                    let store = rt.store();
                    receipt::save(&store, &rec)?;
                    let why = if late { "受理された（取り消しは間に合わなかった）" } else { "受理された（まだ確かめていない）" };
                    receipt::event(&store, &rec, "DriverAccepted", why, json!({ "attempt": seq }))?;
                }
                let (handler, c, input, held) = (reg.handler.clone(), ctx, rec.input.clone(), lease.clone());
                match bounded(reg.contract.timeout, move || {
                    let o = handler.verify(&c, &input, &accepted);
                    drop(held);
                    o
                }) {
                    // 受理の後の Failed（読み直せなかった）・Rejected（拒否と読んだ）は「副作用が無かった」ではない（CD-03: どちらも
                    // dispatch の前の語）。failed にすると idempotency key が外れて同じ書込をもう一度送れるので uncertain にする
                    Bounded::Done(Outcome::Failed { error }) => Outcome::Unverified { reason: format!("読み直しに失敗した（{error:?}）") },
                    Bounded::Done(Outcome::Rejected { reason }) => Outcome::Unverified { reason: format!("受理の後で拒否と読めた（届いたかは不明）: {reason}") },
                    Bounded::Done(outcome) => outcome,
                    Bounded::TimedOut => Outcome::Unverified { reason: "読み直しが時間内に終わらない".into() },
                    Bounded::Panicked => Outcome::Unverified { reason: "読み直しが途中で落ちた".into() },
                }
            }
            Err(ErrorCode::Cancelled) => Outcome::Failed { error: ErrorCode::Cancelled },
            Err(code) if read && retryable(code) && seq <= reg.contract.max_retries => {
                finish(&mut rec, Outcome::Failed { error: code });
                let wait = backoff(reg.contract.backoff, seq);
                {
                    let store = rt.store();
                    receipt::save(&store, &rec)?;
                    receipt::event(&store, &rec, "RetryScheduled", "読取を再試行する", json!({ "attempt": seq, "error": code, "backoff_ms": wait.as_millis() as u64 }))?;
                }
                sleep_unless_cancelled(flag, wait);
                // 時間切れで手を離した thread が lease を持ったまま対象を触っている間は、次の attempt を出さない（CD-19 は同じ run の
                // attempt の間でも効く）。契約の timeout まで待って返らなければ再試行をやめる（lease はその thread が終わった時に返る）
                if let Some(held) = lease.as_ref().filter(|l| Arc::strong_count(*l) > 1) {
                    receipt::event(&rt.store(), &rec, "RetryWaiting", "前の試みがまだ対象を触っている（終わるまで次の試みを出さない）", json!({ "attempt": seq }))?;
                    if !released(held, flag, Instant::now() + reg.contract.timeout) && !flag.load(Ordering::SeqCst) {
                        return conclude(app, rec, Outcome::Failed { error: code });
                    }
                }
                continue;
            }
            Err(code) if !read && matches!(code, ErrorCode::Timeout | ErrorCode::ConnectionLost) => {
                Outcome::EffectUnknown { reason: format!("{code:?}: 届いたか分からない（自動で送り直さない）") }
            }
            Err(code) => Outcome::Failed { error: code },
        };
        return conclude(app, rec, outcome);
    }
}

/// dispatch の前に終わる（attempt を足さない）。
fn stop(app: &AppRuntime, mut rec: RunRecord, to: RunStatus, code: ErrorCode, why: &str) -> Result<Receipt, ErrorCode> {
    receipt::set_status(&mut rec, to)?;
    let store = app.rt.store();
    receipt::save(&store, &rec)?;
    receipt::event(&store, &rec, if to == RunStatus::Cancelled { "RunCancelled" } else { "RunFailed" }, why, json!({ "error": code }))?;
    Err(code)
}

/// Attempt の outcome を埋め、状態を決め、Receipt を同じ 1 回の保存で書く。
fn conclude(app: &AppRuntime, mut rec: RunRecord, outcome: Outcome) -> Result<Receipt, ErrorCode> {
    finish(&mut rec, outcome.clone());
    let to = match (&outcome, rec.run.status) {
        (Outcome::Verified { .. }, _) => RunStatus::Verified,
        (Outcome::Unverified { .. } | Outcome::EffectUnknown { .. }, _) => RunStatus::Uncertain,
        (Outcome::Failed { error: ErrorCode::Cancelled }, RunStatus::Running) => RunStatus::Cancelled,
        _ => RunStatus::Failed,
    };
    receipt::set_status(&mut rec, to)?;
    let done = receipt::receipt_for(&rec, &outcome);
    rec.receipt = Some(done.clone());
    let (kind, result) = match &outcome {
        Outcome::Verified { .. } => ("VerificationSucceeded", "verified".to_string()),
        Outcome::Unverified { reason } => ("VerificationUnverified", format!("未確認: {reason}")),
        Outcome::EffectUnknown { reason } => ("RunNeedsReconciliation", format!("届いたか分からない: {reason}")),
        Outcome::Failed { error: ErrorCode::Cancelled } if to == RunStatus::Cancelled => ("RunCancelled", "副作用の前に止まった".to_string()),
        Outcome::Failed { error } => ("RunFailed", format!("{error:?}")),
        Outcome::Rejected { reason } => ("RunFailed", format!("拒否: {reason}")),
    };
    let store = app.rt.store();
    receipt::save(&store, &rec)?;
    receipt::event(&store, &rec, kind, &result, json!({ "outcome": outcome, "attempt": done.attempt_seq }))?;
    Ok(done)
}

/// 開いている（outcome の無い）最後の Attempt を埋める。埋まった Attempt は書き換えない（再試行をやめた時の conclude）。
fn finish(rec: &mut RunRecord, outcome: Outcome) {
    if let Some(attempt) = rec.run.steps.first_mut().and_then(|s| s.attempts.last_mut()).filter(|a| a.outcome.is_none()) {
        attempt.outcome = Some(outcome);
        attempt.finished_at = Some(now_rfc3339());
    }
}

/// lease の持ち手が run だけになるまで待つ（時間切れで手を離した attempt の thread が返したら true）。cancel か deadline で false。
fn released(lease: &Arc<LeaseGuard>, flag: &AtomicBool, deadline: Instant) -> bool {
    while Arc::strong_count(lease) > 1 {
        if flag.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// CD-03 A: 再試行してよいのは届いていない事が分かる失敗だけ（読取に限って使う）。
fn retryable(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::Timeout | ErrorCode::ConnectionLost | ErrorCode::RateLimited)
}

fn backoff(base: Duration, seq: u32) -> Duration {
    base.saturating_mul(1 << seq.saturating_sub(1).min(8)).min(Duration::from_secs(30))
}

fn sleep_unless_cancelled(flag: &AtomicBool, wait: Duration) {
    let end = Instant::now() + wait;
    while !flag.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(10)));
    }
}

enum Bounded<T> {
    Done(T),
    TimedOut,
    Panicked,
}

/// 別 thread で動かし、limit を過ぎたら待つのをやめる。thread は残って後で終わりうる（だから書込は EffectUnknown にする）。
fn bounded<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Bounded<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(v) => Bounded::Done(v),
        Err(RecvTimeoutError::Timeout) => Bounded::TimedOut,
        Err(RecvTimeoutError::Disconnected) => Bounded::Panicked,
    }
}
