//! run の記録（RUN-004 / 006・CD-09）: runs 表の 1 行 = RunRecord（domain の Run ＋ 凍結した入力・承認・Receipt）、
//! run_events に段ごとの 1 行（DATA_CONTRACTS §7 の名前。storage の append_run_event が redact して保存）。
//! 再起動の後始末（recover）もここ: outcome の無い Attempt は EffectUnknown ＋ uncertain にして、自動で送り直さない。
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::domain::action::{Effect, InvocationOrigin};
use super::domain::app::AppDefinitionV3;
use super::domain::permission::ConsentReceipt;
use super::domain::run::{ErrorCode, Outcome, Receipt, Run, RunStatus, Step, WorkflowCursor};
use super::lease::LeaseKey;
use super::storage::{new_id, Store, Table};
use super::{internal, now_rfc3339};

/// 承認の問い。exact payload（凍結した入力）とその hash を見せて決めてもらう（ADR-005 (5)）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub run_id: String,
    pub action_id: String,
    pub effect: Effect,
    /// 実行する入力そのもの。承認の後に変えられない。
    pub input: Value,
    pub scopes: Vec<String>,
    pub space_id: String,
    /// `permission::payload_hash`。approve はこれを返してもらう。
    pub payload_hash: String,
    pub requested_at: String,
}

/// runs 表の json。domain の Run は `run`（Runs 画面は `run.status` と `run.steps[0].attempts` を読む）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub run: Run,
    pub session_id: String,
    pub space_id: String,
    /// 凍結した入力。実行するのはこれだけ（後で app の定義が変わっても変わらない: DATA-AC-05）。生の値で、run log とは別。
    pub input: Value,
    /// run log の「対象」。
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ConsentReceipt>,
    /// 最後の attempt の結果（dispatch した run だけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    /// WF0: Engine の手順の run の続き（Some = Engine の run・`run.steps` は手順の手ごと〈操作の番号.手の id〉・`input` は Engine の凍結した手順と記録）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowCursor>,
}

impl RunRecord {
    /// 新しい run（idle → planning）。step は 1 つ（action 1 回の呼び出し。workflow は RUN-003）。
    pub fn new(def: &AppDefinitionV3, session_id: &str, space_id: &str, action_id: &str, origin: InvocationOrigin, input: Value) -> Result<Self, ErrorCode> {
        let now = now_rfc3339();
        let step = Step { id: new_id(), action_ref: action_id.into(), status: RunStatus::Idle, attempts: Vec::new() };
        let run = Run {
            id: new_id(),
            app_id: def.id.clone(),
            app_version: def.version,
            workflow_ref: None,
            origin,
            status: RunStatus::Idle,
            steps: vec![step],
            created_at: now.clone(),
            updated_at: now,
        };
        let mut rec = Self {
            run,
            session_id: session_id.into(),
            space_id: space_id.into(),
            input,
            target: String::new(),
            idempotency_key: None,
            approval: None,
            consent: None,
            receipt: None,
            workflow: None,
        };
        set_status(&mut rec, RunStatus::Planning)?;
        Ok(rec)
    }

    /// WF1: Engine の手順の run（idle → planning → running・手順の手ごとに Step〈id = 操作の番号.手の id〉・`workflow` に cursor）。
    /// run の id は Engine の `Caller::run_id`（承認の receipt が縛る id と同じ）・app は Project の app（id と版）・space = Project の id。
    /// Wave: WF1
    /// 例: `let rec = RunRecord::engine(&run_id, (&app_id, 3), session, project_id, &[("0.read".into(), "git.file.read".into())], frozen, cursor)?`
    /// 保証する: 手 1 つに Step 1 つ（attempt 0）・`input` = Engine の凍結した手順と記録・origin = CommandPalette（一言から）。
    pub fn engine(run_id: &str, app: (&str, u32), session_id: &str, space_id: &str, steps: &[(String, String)], input: Value, cursor: WorkflowCursor) -> Result<Self, ErrorCode> {
        let now = now_rfc3339();
        let run = Run {
            id: run_id.into(),
            app_id: app.0.into(),
            app_version: app.1,
            workflow_ref: None,
            origin: InvocationOrigin::CommandPalette,
            status: RunStatus::Idle,
            steps: steps.iter().map(|(id, action)| Step { id: id.clone(), action_ref: action.clone(), status: RunStatus::Idle, attempts: Vec::new() }).collect(),
            created_at: now.clone(),
            updated_at: now,
        };
        let mut rec = Self {
            run,
            session_id: session_id.into(),
            space_id: space_id.into(),
            input,
            target: String::new(),
            idempotency_key: None,
            approval: None,
            consent: None,
            receipt: None,
            workflow: Some(cursor),
        };
        set_status(&mut rec, RunStatus::Planning)?;
        set_status(&mut rec, RunStatus::Running)?;
        Ok(rec)
    }

    /// 今の手: attempt を持つ最後の手（Engine の run は手が複数で、手は順に走る）。どの手もまだ走っていなければ先頭の手。
    /// receipt・run log の operation・attempt の数はこの手で言う（先頭の読む手の名前で「届いたか分からない」を出さない・INTEG wa2）。
    pub fn step(&self) -> Option<&Step> {
        self.run.steps.iter().rev().find(|s| !s.attempts.is_empty()).or(self.run.steps.first())
    }

    pub fn action_id(&self) -> &str {
        self.step().map_or("", |s| s.action_ref.as_str())
    }

    pub fn attempt_count(&self) -> usize {
        self.step().map_or(0, |s| s.attempts.len())
    }
}

/// run と step の状態を domain の表（RUN_TRANSITIONS）で動かす。表に無い遷移は runtime の誤り = Internal。
pub fn set_status(rec: &mut RunRecord, to: RunStatus) -> Result<(), ErrorCode> {
    rec.run.status.transition(to).map_err(internal)?;
    for step in &mut rec.run.steps {
        step.status = to;
    }
    rec.run.updated_at = now_rfc3339();
    Ok(())
}

/// 1 回の put（status・attempt・receipt が同じ 1 文で入る）。
pub fn save(store: &Store, rec: &RunRecord) -> Result<(), ErrorCode> {
    let json = serde_json::to_value(rec).map_err(internal)?;
    store.put(Table::Runs, &rec.run.id, Some(&rec.session_id), &json).map_err(internal)
}

pub fn load(store: &Store, run_id: &str) -> Result<Option<RunRecord>, ErrorCode> {
    match store.get(Table::Runs, run_id).map_err(internal)? {
        Some(row) => serde_json::from_value(row.json).map(Some).map_err(internal),
        None => Ok(None),
    }
}

/// session の run（作った順）。RunRecord の形でない行は飛ばす。
pub fn list(store: &Store, session_id: &str) -> Result<Vec<RunRecord>, ErrorCode> {
    let rows = store.list(Table::Runs, Some(session_id)).map_err(internal)?;
    Ok(rows.into_iter().filter_map(|r| serde_json::from_value(r.json).ok()).collect())
}

/// 同じ app で同じ idempotency key をまだ縛っている run の全部（Receipt がある・承認待ち・実行中。どの session の物も）。
/// どの session の run を数えるかは呼ぶ側（invoke_keyed）が効果の届く範囲で決める。failed / cancelled は副作用が無かった run なので
/// key を放す（打ち直せる）。EffectUnknown は uncertain なので縛り続ける。
// ponytail: 全 run を毎回なめる O(n)。払うのは key を持つ呼び出し（Agent・明示 key）だけ。run が数千になったら index
pub fn find_by_key(store: &Store, app_id: &str, key: &str) -> Result<Vec<RunRecord>, ErrorCode> {
    let rows = store.list(Table::Runs, None).map_err(internal)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| serde_json::from_value::<RunRecord>(r.json).ok())
        .filter(|r| {
            r.run.app_id == app_id && r.idempotency_key.as_deref() == Some(key) && !matches!(r.run.status, RunStatus::Failed | RunStatus::Cancelled)
        })
        .collect())
}

/// run_events に 1 行。どの行も operation / source / target / result を持つ（RUN-006）。payload は storage が redact する。
pub fn event(store: &Store, rec: &RunRecord, kind: &str, result: &str, extra: Value) -> Result<(), ErrorCode> {
    let mut payload = json!({
        "operation": rec.action_id(),
        "source": source_label(&rec.run.origin),
        "target": rec.target,
        "result": result,
    });
    if let (Some(p), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        p.extend(extra);
    }
    store.append_run_event(&rec.run.id, kind, &payload).map(|_| ()).map_err(internal)
}

pub fn source_label(origin: &InvocationOrigin) -> String {
    match origin {
        InvocationOrigin::Button { node_key } => format!("button:{node_key}"),
        InvocationOrigin::CommandPalette => "command_palette".into(),
        InvocationOrigin::Agent { suggestion_id } => format!("agent:{suggestion_id}"),
        InvocationOrigin::Terminal { terminal_id, agent } => format!("terminal:{terminal_id}:{}", agent.as_deref().unwrap_or("shell")),
    }
}

/// run log の「対象」: lease があればその key、無ければ action の source（id の最後の段を除いた物）と record id。
pub fn target(action_id: &str, input: &Value, leases: &[LeaseKey]) -> String {
    if !leases.is_empty() {
        return leases.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
    }
    let source = action_id.rsplit_once('.').map_or(action_id, |(s, _)| s);
    match ["item_id", "note_id", "task_id"].iter().find_map(|k| input.get(*k).and_then(Value::as_str)) {
        Some(id) => format!("{source}/{id}"),
        None => source.into(),
    }
}

/// 最後の attempt の Receipt。observation_refs は Verified の evidence。
pub fn receipt_for(rec: &RunRecord, outcome: &Outcome) -> Receipt {
    let step = rec.step();
    Receipt {
        id: new_id(),
        run_id: rec.run.id.clone(),
        step_id: step.map(|s| s.id.clone()).unwrap_or_default(),
        attempt_seq: step.and_then(|s| s.attempts.last()).map_or(0, |a| a.seq),
        action_ref: rec.action_id().into(),
        outcome: outcome.clone(),
        observation_refs: match outcome {
            Outcome::Verified { evidence } => evidence.clone(),
            _ => Vec::new(),
        },
        recorded_at: now_rfc3339(),
    }
}

/// RUN-006 の 1 行。`detail` は redact 済みの payload 全体（入力・hash・error など）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunLogLine {
    pub seq: i64,
    pub at: String,
    pub event: String,
    pub operation: String,
    pub source: String,
    pub target: String,
    pub result: String,
    pub detail: Value,
}

pub fn run_log(store: &Store, run_id: &str) -> Result<Vec<RunLogLine>, ErrorCode> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let events = store.list_run_events(run_id).map_err(internal)?;
    Ok(events
        .into_iter()
        .map(|e| RunLogLine {
            seq: e.seq,
            at: e.recorded_at,
            event: e.kind,
            operation: text(&e.payload, "operation"),
            source: text(&e.payload, "source"),
            target: text(&e.payload, "target"),
            result: text(&e.payload, "result"),
            detail: e.payload,
        })
        .collect())
}

/// 再起動の後始末（CD-09 ii・DATA-AC-06）。`Runtime::start` が 1 回だけ呼ぶ（動いている run がある時に呼ばない）。
/// - outcome の無い Attempt = dispatch の後で止まった → EffectUnknown ＋ uncertain ＋ Receipt（送ったか分からない。自動で再送しない）
/// - attempt の無い planning / running = dispatch の前で止まった → failed（副作用なし。key を放す）
///
/// 返り値は EffectUnknown にした数。表に無い遷移になる行は触らない。
pub fn recover(store: &Store) -> Result<usize, ErrorCode> {
    let mut reconciled = 0;
    for row in store.list(Table::Runs, None).map_err(internal)? {
        let Ok(mut rec) = serde_json::from_value::<RunRecord>(row.json) else { continue };
        // WF1: Engine の run は手が複数（どの手の attempt でも結果の無い物 = 送った後に止まった）
        let open = rec.run.steps.iter().position(|s| s.attempts.last().is_some_and(|a| a.outcome.is_none()));
        if let Some(i) = open {
            let outcome = Outcome::EffectUnknown { reason: "Shibubu closed while this was being sent. It may have arrived, so it was not sent again".into() };
            if let Some(attempt) = rec.run.steps[i].attempts.last_mut() {
                attempt.outcome = Some(outcome.clone());
                attempt.finished_at = Some(now_rfc3339());
            }
            if set_status(&mut rec, RunStatus::Uncertain).is_err() {
                continue;
            }
            rec.receipt = Some(receipt_for(&rec, &outcome));
            save(store, &rec)?;
            event(store, &rec, "RunNeedsReconciliation", "May have arrived · Shibubu closed while sending · not sent again", json!({ "outcome": outcome }))?;
            reconciled += 1;
        } else if matches!(rec.run.status, RunStatus::Planning | RunStatus::Running) && set_status(&mut rec, RunStatus::Failed).is_ok() {
            save(store, &rec)?;
            // WF1: Engine の run は手の間で止まった（済んだ手は走った・残りは送っていない）= 副作用なしとは言わない
            let done = rec.run.steps.iter().filter(|s| !s.attempts.is_empty()).count();
            let why = if rec.workflow.is_some() && done > 0 { format!("再起動: 手順の途中で止まった（済んだ手 {done}・残りは送っていない）") } else { "再起動: dispatch の前に止まった（副作用なし）".to_string() };
            event(store, &rec, "RunFailed", &why, json!({}))?;
        }
    }
    Ok(reconciled)
}
