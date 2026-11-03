//! Action Runtime（unit RT・TODO.md RUN-001 / 002 / 004 / 005 / 006 / 008、CORE-003 の呼び出し経路）。
//! button / command palette / Agent は `AppRuntime::invoke` の 1 本を通る。判定は permission.rs の gate 1 か所、
//! 実行は executor.rs（lease・write-ahead・timeout・retry・cancel）、記録は receipt.rs（runs 表の RunRecord と run_events）。
//! 型は domain の物を使う（Effect / Outcome / RunStatus / Receipt / ErrorCode / ConsentReceipt / Observation）。
//! 中では `crate::` を使わない（tests/runtime.rs が `#[path]` で取り込む）。domain と storage は親の兄弟として `super::` で引く。
//! 図: docs/diagrams.md §21。正本: CONTRACT_DECISIONS CD-01 / 03 / 04 / 09 / 16 / 17、ADR-005。
#![allow(dead_code)] // 統合役が app / agent に配線するまで main からの呼び出しが無い

pub mod executor;
pub mod lease;
pub mod local_actions;
pub mod permission;
pub mod receipt;
pub mod registry;

use super::domain;
use super::storage;

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use domain::action::{validate_invocation, ActionInvocation, InvocationError, InvocationOrigin};
use domain::app::AppDefinitionV3;
use domain::patch::AppRevision;
use domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject, PermissionGrant};
use domain::run::{ErrorCode, Receipt, RunStatus};
use lease::LeaseTable;
use permission::GrantRecord;
use receipt::{ApprovalRequest, RunLogLine, RunRecord};
use registry::ActionRegistry;
use storage::{Store, Table};

/// app の `metadata.space_id` が無い時の space（ADR-005 (4)）。
pub const DEFAULT_SPACE: &str = "default";

/// invoke の Ok。承認待ちを入れる field が domain の Receipt に無いので 2 つに分けた。
#[derive(Debug, Clone, PartialEq)]
pub enum Invoked {
    /// dispatch した（attempt が 1 つ以上ある）。結果は Receipt の outcome（受理だけなら Verified にならない）。
    Done(Receipt),
    /// 何も dispatch していない。exact payload を見せて `approve(run_id, payload_hash)` を待つ。
    NeedsApproval(ApprovalRequest),
}

/// cancel の結果。終わった副作用を取り消したとは言わない（RUN-008）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelResult {
    /// 副作用の前（idle / planning / waiting_*）だったので cancelled にした。
    Cancelled,
    /// running（lease の実行待ちか handler の中）。flag を立てた。止まれば cancelled、間に合わなければ結果のまま。
    Requested,
    /// verifying 以降。状態は変えない。
    TooLate(RunStatus),
}

/// process に 1 つ。store・registry・lease・開いた app の revision・実行中の run の cancel flag を持つ。
pub struct Runtime {
    store: Arc<Mutex<Store>>,
    registry: ActionRegistry,
    leases: Arc<LeaseTable>,
    /// app id → 今の revision。同じ app を別 session で開いても 1 つを共有する（ui.patch がどの session にも同じに見える）。
    apps: Mutex<HashMap<String, Arc<Mutex<AppRevision>>>>,
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Runtime {
    /// 起動時に 1 回。前の process が残した outcome の無い Attempt を EffectUnknown + uncertain にする（自動で再送しない・CD-09 ii）。
    pub fn start(store: Arc<Mutex<Store>>, registry: ActionRegistry) -> Result<Arc<Self>, ErrorCode> {
        receipt::recover(&lock(&store))?;
        Ok(Arc::new(Self { store, registry, leases: Arc::default(), apps: Mutex::default(), running: Mutex::default() }))
    }

    /// app を 1 つ開く。store が正（CD-09）: 初めての id なら渡した定義を入れ（apps と app_revisions の 1 行目）、2 回目からは
    /// store の最新 revision を使う。渡した定義は「同じ app か」の照合にだけ使う: 指紋（`identity`。ui.patch が変えられない
    /// 所の全部 = actions・capabilities・sources・agent・metadata.space_id …）が store の定義と違えば PermissionDenied
    /// （同じ id を名乗る別の定義が、その id と space に置いた grant を継がない・RT-SEC-2）。別 app の session も開かない
    /// （sessions の親が違う = ui_state と run が別 app と混ざる）。sessions の行が無ければ足す（runs・ui_state の FK の親）。
    /// revision は app ごとに 1 つ（同じ app の AppRuntime が共有する。別 session の ui.patch が見える）、space は store の定義の物。
    pub fn open(self: &Arc<Self>, app: AppRevision, session_id: &str) -> Result<AppRuntime, ErrorCode> {
        let app_id = app.definition().id.clone();
        let claimed = identity(app.definition())?;
        let mut apps = lock(&self.apps); // 同じ id の open が同時に来ても入れるのは 1 回（lock の順は apps → store）
        // 読む・入れる・照合するを 1 回の DB transaction の中で: 同じ DB を開いた別の process が同じ新しい id を同時に開いても、
        // 1 行目を入れるのは 1 つで、後の方はそれと照合される（拒否した open は何も残さない）
        let current = self.store().immediate(internal, |store| {
            let current = match store.latest_revision(&app_id).map_err(internal)? {
                Some(rev) => revision_of(rev.json)?,
                None => {
                    // 初めて開く app。apps の行だけ先に在れば（runtime の外が置いた）それを 1 行目にする
                    let first = match store.get(Table::Apps, &app_id).map_err(internal)? {
                        Some(row) => revision_of(row.json)?,
                        None => app,
                    };
                    let json = serde_json::to_value(first.definition()).map_err(internal)?;
                    store.put(Table::Apps, &app_id, None, &json).map_err(internal)?;
                    store.put_revision(&app_id, &json, None).map_err(internal)?;
                    first
                }
            };
            if identity(current.definition())? != claimed {
                return Err(ErrorCode::PermissionDenied);
            }
            match store.get(Table::Sessions, session_id).map_err(internal)? {
                None => store.put(Table::Sessions, session_id, Some(&app_id), &json!({ "app_id": app_id })).map_err(internal)?,
                Some(row) if row.parent_id.as_deref() == Some(app_id.as_str()) => {}
                Some(_) => return Err(ErrorCode::PermissionDenied),
            }
            Ok(current)
        })?;
        let space_id = space_of(current.definition());
        // 一覧の汎用の local action（add・filter・update_<欄>・toggle_done・delete）を定義から導く（entity ごとに手書きしない）
        local_actions::derive(&self.registry, current.definition());
        // store が正（CD-09）: この process の写しより store が新しければ（別の process が ui.patch で進めた）写しを入れ替える。
        // 古い写しを返すと描く側は別 process の変更を見ず、版付きの提案は毎回 stale になる（RT-R3-2）。lock の順は apps → app
        let shared = apps.entry(app_id.clone()).or_insert_with(|| Arc::new(Mutex::new(current.clone()))).clone();
        {
            let mut cached = lock(&shared);
            if current.version() > cached.version() {
                *cached = current;
            }
        }
        Ok(AppRuntime { rt: self.clone(), app: shared, app_id, session_id: session_id.into(), space_id })
    }

    pub fn registry(&self) -> &ActionRegistry {
        &self.registry
    }

    /// RQ2（RQ-005・Delete）: 消した app の写しを忘れる（同じ id で作り直した時に、消した前の revision を返さない）。store の行は呼ぶ側が消す。
    pub fn forget(&self, app_id: &str) {
        lock(&self.apps).remove(app_id);
    }

    /// run id だけで効く（どの app の run でも）。
    pub fn cancel(&self, run_id: &str) -> Result<CancelResult, ErrorCode> {
        let store = self.store();
        let mut rec = receipt::load(&store, run_id)?.ok_or(ErrorCode::TargetNotFound)?;
        match rec.run.status {
            RunStatus::Idle | RunStatus::Planning | RunStatus::WaitingConnection | RunStatus::WaitingApproval => {
                receipt::set_status(&mut rec, RunStatus::Cancelled)?;
                receipt::save(&store, &rec)?;
                receipt::event(&store, &rec, "RunCancelled", "副作用の前に取り消した", json!({}))?;
                Ok(CancelResult::Cancelled)
            }
            RunStatus::Running => {
                if let Some(flag) = lock(&self.running).get(run_id) {
                    flag.store(true, Ordering::SeqCst);
                }
                receipt::event(&store, &rec, "CancelRequested", "取り消しを頼んだ（副作用の前なら止まる）", json!({}))?;
                Ok(CancelResult::Requested)
            }
            status => {
                receipt::event(&store, &rec, "CancelTooLate", "副作用の後なので取り消さない", json!({ "status": status }))?;
                Ok(CancelResult::TooLate(status))
            }
        }
    }

    pub fn run(&self, run_id: &str) -> Result<Option<RunRecord>, ErrorCode> {
        receipt::load(&self.store(), run_id)
    }

    /// RUN-006: 操作 / source / 時刻 / 対象 / 結果。payload は storage が redact して保存した物。
    pub fn run_log(&self, run_id: &str) -> Result<Vec<RunLogLine>, ErrorCode> {
        receipt::run_log(&self.store(), run_id)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    /// run を running で保存したのと同じ store の lock の中で呼ぶ: cancel（store の lock の中で状態を見る）は running を見たら
    /// 必ず flag を見つける。外すのは executor（結果を保存した後）。lock の順は store → running（cancel と同じ）。
    /// ENGINE-ADJUST-1102 ①: Engine の run（`Generated::start_engine_with`）も同じ表に flag を置く = Runs の Stop（`cancel`）が runs 表に行が出来た後に同じ印を立てる。
    /// 外すのは `untrack`（Engine の答えが届いた時）。
    pub(crate) fn track(&self, run_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        lock(&self.running).insert(run_id.into(), flag.clone());
        flag
    }

    /// ENGINE-ADJUST-1102 ①: `track` の逆（Engine の run が終わった時・`Generated::poll_engine`）。executor は結果を保存した後に自分で外す（`executor::execute`）。
    pub(crate) fn untrack(&self, run_id: &str) {
        lock(&self.running).remove(run_id);
    }
}

/// 開いた app 1 つ（今の revision・session・space）。invoke の唯一の入口。
pub struct AppRuntime {
    rt: Arc<Runtime>,
    /// 同じ app の AppRuntime で共有する（Runtime::open）。
    app: Arc<Mutex<AppRevision>>,
    /// `app` の定義の id（patch でも変わらない）。承認の持ち主を照合する時に app の lock を取らない（lock の順は app → store）。
    app_id: String,
    session_id: String,
    space_id: String,
}

impl AppRuntime {
    /// button / command palette / Agent の共通の入口（CORE-003）。入力の無い action は `{}` か `null` を渡す。
    pub fn invoke(&self, action_id: &str, input: Value, origin: InvocationOrigin) -> Result<Invoked, ErrorCode> {
        self.invoke_keyed(action_id, input, origin, None)
    }

    /// `key` は二重送信を止める idempotency key。None なら Agent 由来だけ `agent:<app>:<suggestion>:<action>` を使う
    /// （同じ提案は 1 回しか実行しない）。button の連打は key を渡さない限り別の実行。key は効果が届く範囲で縛る: local の action
    /// （中身は session の ui_state）は session ごと、外部と ui.patch（app の revision）は app ごと。他の session の Receipt は返さない。
    pub fn invoke_keyed(&self, action_id: &str, input: Value, origin: InvocationOrigin, key: Option<&str>) -> Result<Invoked, ErrorCode> {
        let reg = self.rt.registry.get(action_id).ok_or(ErrorCode::NotReady)?; // RUN-001: 未登録は呼べない
        if !self.rt.registry.adapter_ready(reg.contract.adapter) {
            return Err(ErrorCode::NotReady);
        }
        let def = self.definition_for(action_id);
        let input = if input.is_null() { json!({}) } else { input };
        let inv = ActionInvocation { action_ref: action_id.into(), origin, input };
        let action = validate_invocation(&def, &inv).map_err(invocation_error)?.clone();
        check_bounds(&action.input_schema, &inv.input)?;
        // app の宣言で effect を軽くできない（composer / LLM が destructive を local_read と書いても registry が正）。
        if action.effect != reg.contract.effect {
            return Err(ErrorCode::PermissionDenied);
        }
        let key = key.map(String::from).or_else(|| match &inv.origin {
            InvocationOrigin::Agent { suggestion_id } => Some(format!("agent:{}:{suggestion_id}:{action_id}", def.id)),
            _ => None,
        });
        let target = receipt::target(action_id, &inv.input, &(reg.contract.leases)(&inv.input));
        let store = self.rt.store();
        let grants = permission::grants(&store, &def.id)?;
        let needs_approval = permission::gate(&def, &action, &reg.contract, &inv.origin, &self.space_id, &grants, &now_rfc3339())?;
        if let Some(k) = &key {
            // 探すのと作るのを同じ lock の中で（同じ key の 2 つが同時に来ても run は 1 つ）
            let bound = receipt::find_by_key(&store, &def.id, k)?;
            // Receipt と承認の問いを返すのは run を作った session にだけ（own_run と同じ: 他の画面の run は見せない）
            if let Some(prev) = bound.iter().find(|r| r.session_id == self.session_id) {
                return match (&prev.receipt, &prev.approval, prev.run.status) {
                    (Some(done), _, _) => Ok(Invoked::Done(done.clone())),
                    (None, Some(req), RunStatus::WaitingApproval) => Ok(Invoked::NeedsApproval(req.clone())),
                    _ => Err(ErrorCode::RateLimited), // 同じ key の run が実行中
                };
            }
            // 別 session の run だけ: 効果が session の中に留まる action（local の ui_state）なら、この session には何も起きていないので
            // 自分の run を作る。session に依らず効く action（外部・ui.patch の revision）は済んだ / 進んでいるので、送り直さず見せもしない
            if !bound.is_empty() && !(reg.contract.adapter == registry::LOCAL && action_id != registry::UI_PATCH) {
                return Err(ErrorCode::RateLimited);
            }
        }
        let mut rec = RunRecord::new(&def, &self.session_id, &self.space_id, action_id, inv.origin, inv.input)?;
        rec.idempotency_key = key;
        rec.target = target;
        let request = if needs_approval {
            let scopes = permission::required_scopes(&def, &action, &reg.contract);
            let req = ApprovalRequest {
                run_id: rec.run.id.clone(),
                action_id: action_id.into(),
                effect: action.effect,
                input: rec.input.clone(),
                payload_hash: permission::payload_hash(action_id, &def.id, &self.space_id, &rec.input, &scopes),
                scopes,
                space_id: self.space_id.clone(),
                requested_at: now_rfc3339(),
            };
            receipt::set_status(&mut rec, RunStatus::WaitingApproval)?;
            rec.approval = Some(req.clone());
            Some(req)
        } else {
            // planning を保存しない: cancel が planning を見て Cancelled と答えた後で executor が running に上書きして dispatch する窓を作らない
            receipt::set_status(&mut rec, RunStatus::Running)?;
            None
        };
        receipt::save(&store, &rec)?;
        let digest = permission::digest(&rec.input);
        receipt::event(&store, &rec, "RunCreated", "created", json!({ "input": rec.input, "input_digest": digest, "app_version": rec.run.app_version }))?;
        if let Some(req) = request {
            receipt::event(&store, &rec, "ApprovalRequested", "承認待ち", json!({ "effect": req.effect, "payload_hash": req.payload_hash }))?;
            return Ok(Invoked::NeedsApproval(req));
        }
        receipt::event(&store, &rec, "PolicyChecked", "allowed", json!({ "effect": action.effect }))?;
        let flag = self.rt.track(&rec.run.id);
        drop(store);
        executor::execute(self, rec, &reg, flag).map(Invoked::Done)
    }

    /// 見せた ApprovalRequest の payload_hash で承認する。凍結した入力の hash と違えば実行しない（ADR-005 (5)）。
    /// 2 回目の承認は 1 回目の Receipt を返す（送り直さない）。run を作った app・session・space の AppRuntime からだけ効く（他は TargetNotFound）。
    pub fn approve(&self, run_id: &str, payload_hash: &str) -> Result<Receipt, ErrorCode> {
        let (rec, reg, flag) = {
            let store = self.rt.store();
            let mut rec = self.own_run(&store, run_id)?;
            if let Some(done) = &rec.receipt {
                return Ok(done.clone());
            }
            match rec.run.status {
                RunStatus::WaitingApproval => {}
                RunStatus::Cancelled => return Err(ErrorCode::Cancelled),
                RunStatus::Running | RunStatus::Verifying => return Err(ErrorCode::RateLimited),
                _ => return Err(ErrorCode::ApprovalDenied),
            }
            let req = rec.approval.clone().ok_or(ErrorCode::Internal)?;
            let frozen = permission::payload_hash(&req.action_id, &rec.run.app_id, &rec.space_id, &rec.input, &req.scopes);
            if payload_hash != req.payload_hash || frozen != req.payload_hash {
                receipt::event(&store, &rec, "ApprovalMismatch", "承認した内容と違うので実行しない", json!({ "approved": payload_hash, "frozen": frozen }))?;
                return Err(ErrorCode::ApprovalDenied);
            }
            // 承認は scope の代わりにならない。待つ間に grant が切れていたら実行しない。
            let grants = permission::grants(&store, &rec.run.app_id)?;
            if !permission::scopes_granted(&req.scopes, &rec.run.app_id, &rec.space_id, &grants, &now_rfc3339()) {
                receipt::set_status(&mut rec, RunStatus::Failed)?;
                receipt::save(&store, &rec)?;
                receipt::event(&store, &rec, "RunFailed", "承認を待つ間に権限が無くなった", json!({}))?;
                return Err(ErrorCode::PermissionDenied);
            }
            let reg = self.rt.registry.get(&req.action_id).ok_or(ErrorCode::NotReady)?;
            // 承認は接続の代わりにもならない。待つ間に adapter が revoke / 失効していたら invoke と同じ NotReady で dispatch しない。
            // run は承認待ちのまま（接続し直してから同じ承認を押せる）
            if !self.rt.registry.adapter_ready(reg.contract.adapter) {
                receipt::event(&store, &rec, "ApprovalBlocked", "接続が使えないので実行しない（承認待ちのまま）", json!({ "adapter": reg.contract.adapter }))?;
                return Err(ErrorCode::NotReady);
            }
            rec.consent = Some(consent(&rec, &req, ConsentDecision::Approved));
            // lock の中で running にする: 同時に来た 2 回目の approve は running を見て止まる
            receipt::set_status(&mut rec, RunStatus::Running)?;
            receipt::save(&store, &rec)?;
            receipt::event(&store, &rec, "ApprovalGranted", "承認された", json!({ "payload_hash": req.payload_hash }))?;
            let flag = self.rt.track(&rec.run.id);
            (rec, reg, flag)
        };
        executor::execute(self, rec, &reg, flag)
    }

    /// 承認しない。何も dispatch せずに failed（key は放す）。
    pub fn deny(&self, run_id: &str) -> Result<(), ErrorCode> {
        let store = self.rt.store();
        let mut rec = self.own_run(&store, run_id)?;
        if rec.run.status != RunStatus::WaitingApproval {
            return Err(ErrorCode::ApprovalDenied);
        }
        let req = rec.approval.clone().ok_or(ErrorCode::Internal)?;
        rec.consent = Some(consent(&rec, &req, ConsentDecision::Denied));
        receipt::set_status(&mut rec, RunStatus::Failed)?;
        receipt::save(&store, &rec)?;
        receipt::event(&store, &rec, "ApprovalDenied", "承認されなかった（実行していない）", json!({}))
    }

    /// 接続の同意（scope の grant）を、この app・この space に置く。別 space へは漏れない（RUN-002）。
    pub fn grant(&self, grant: PermissionGrant) -> Result<(), ErrorCode> {
        let def = lock(&self.app).definition().clone();
        domain::validate::validate_grant(&def, &grant).map_err(|_| ErrorCode::PermissionDenied)?;
        let rec = GrantRecord { space_id: self.space_id.clone(), grant };
        let json = serde_json::to_value(&rec).map_err(internal)?;
        self.rt.store().put(Table::Permissions, &rec.grant.id, Some(&rec.grant.app_id), &json).map_err(internal)
    }

    /// この session の run（作った順）。Runs 画面の元。
    pub fn runs(&self) -> Result<Vec<RunRecord>, ErrorCode> {
        receipt::list(&self.rt.store(), &self.session_id)
    }

    /// 今の revision（ui.patch で入れ替わる）。
    pub fn revision(&self) -> AppRevision {
        lock(&self.app).clone()
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.rt
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn space_id(&self) -> &str {
        &self.space_id
    }

    /// 画面に出す source の中身（この session の ui_state を `local_actions::view` に通した物: 未保存の timer は既定の長さ・
    /// 動いている timer の残りは今の時刻から）。描く側（app.rs の Host）が binding の path をこの上で引く。
    pub fn ui_state(&self, source: &str) -> Result<Value, ErrorCode> {
        let doc = self.rt.store().get_ui_state(&self.session_id, source).map_err(internal)?;
        Ok(local_actions::view(source, doc, storage::now_ms()))
    }

    /// 画面の中身が時刻だけで次に替わるまで（動いている timer の秒の替わり目・`local_actions::next_change_ms`）。None = 入力か run でしか替わらない。
    pub fn next_change(&self) -> Option<std::time::Duration> {
        local_actions::next_change_ms(&self.ui_state(local_actions::TIMER_SOURCE).ok()?).map(std::time::Duration::from_millis)
    }

    /// この app・session・space の run。他の画面（別 app・別 session・別 space）の run は無い物として扱う: 承認の hash が
    /// 縛る app / space と、効果が当たる先（Ctx の session と revision = self）をずらさない。Receipt も他の画面には見せない。
    fn own_run(&self, store: &Store, run_id: &str) -> Result<RunRecord, ErrorCode> {
        receipt::load(store, run_id)?
            .filter(|r| r.run.app_id == self.app_id && r.session_id == self.session_id && r.space_id == self.space_id)
            .ok_or(ErrorCode::TargetNotFound)
    }

    /// 検査に使う定義。app が宣言していない runtime 側の action（ui.patch）は登録の定義を足して同じ検査に通す。
    fn definition_for(&self, action_id: &str) -> AppDefinitionV3 {
        let mut def = lock(&self.app).definition().clone();
        if def.action(action_id).is_none() {
            def.actions.extend(registry::builtin_definition(action_id));
        }
        def
    }
}

fn consent(rec: &RunRecord, req: &ApprovalRequest, decision: ConsentDecision) -> ConsentReceipt {
    ConsentReceipt {
        id: storage::new_id(),
        app_id: rec.run.app_id.clone(),
        subject: ConsentSubject::Action { action_ref: req.action_id.clone(), run_id: Some(rec.run.id.clone()) },
        scopes: req.scopes.clone(),
        payload_digest: Some(req.payload_hash.clone()),
        decision,
        decided_at: now_rfc3339(),
    }
}

fn space_of(def: &AppDefinitionV3) -> String {
    def.metadata.as_ref().and_then(|m| m.get("space_id")).and_then(Value::as_str).unwrap_or(DEFAULT_SPACE).into()
}

/// 同じ app かを決める指紋（RT-SEC-2）: ui.patch が変えられる composition と apply が上げる version を除いた定義の digest
/// （CD-18 (5): patch は composition.root しか触らない）。除く物を名指すので、定義に field が増えても指紋に入る側に倒れる。
fn identity(def: &AppDefinitionV3) -> Result<String, ErrorCode> {
    let mut v = serde_json::to_value(def).map_err(internal)?;
    if let Some(m) = v.as_object_mut() {
        m.remove("composition");
        m.remove("version");
    }
    Ok(permission::digest(&v))
}

/// apps / app_revisions の json → 検証済みの AppRevision。読めない・検証に落ちる行は使わない（その上に patch も当てない・Internal）。
fn revision_of(json: Value) -> Result<AppRevision, ErrorCode> {
    let def: AppDefinitionV3 = serde_json::from_value(json).map_err(internal)?;
    AppRevision::new(def).map_err(|errs| internal(format!("保存された定義が検証に落ちる: {errs:?}")))
}

fn invocation_error(e: InvocationError) -> ErrorCode {
    match e {
        InvocationError::UnknownAction(_) => ErrorCode::TargetNotFound,
        InvocationError::NotReady { .. } | InvocationError::CapabilityMissing { .. } | InvocationError::CapabilityNotReady { .. } => {
            ErrorCode::NotReady
        }
        InvocationError::InputNotObject | InvocationError::MissingInput { .. } | InvocationError::InputType { .. } => ErrorCode::InvalidInput,
    }
}

/// domain の check_input（type / required）が見ない所: minimum / maximum / minLength と additionalProperties: false。
// ponytail: 1 段目の properties だけ。入れ子の schema は validator crate を足す時
fn check_bounds(schema: &Map<String, Value>, input: &Value) -> Result<(), ErrorCode> {
    let props = schema.get("properties").and_then(Value::as_object);
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        let declared = |k: &String| props.is_some_and(|p| p.contains_key(k));
        if input.as_object().is_some_and(|o| !o.keys().all(declared)) {
            return Err(ErrorCode::InvalidInput);
        }
    }
    for (name, prop) in props.into_iter().flatten() {
        let Some(v) = input.get(name) else { continue };
        let limit = |k: &str, out: fn(f64, f64) -> bool| prop.get(k).and_then(Value::as_f64).zip(v.as_f64()).is_some_and(|(l, x)| out(x, l));
        let short = prop.get("minLength").and_then(Value::as_u64).zip(v.as_str()).is_some_and(|(n, s)| (s.chars().count() as u64) < n);
        if limit("minimum", |x, l| x < l) || limit("maximum", |x, l| x > l) || short {
            return Err(ErrorCode::InvalidInput);
        }
    }
    Ok(())
}

/// poison しても store は SQLite の 1 文ごとに整合しているので、中身を取り出して続ける。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// storage / serde の失敗は ErrorCode に詳細を持てないので、原文を stderr に残して Internal にする。
fn internal(e: impl std::fmt::Display) -> ErrorCode {
    eprintln!("kangae runtime: {e}");
    ErrorCode::Internal
}

/// RFC3339 UTC（ms）。domain の時刻（Attempt / Receipt / grant の期限）は全部この形で、辞書順で比べられる。
pub fn now_rfc3339() -> String {
    rfc3339(storage::now_ms())
}

/// epoch ms → `YYYY-MM-DDTHH:MM:SS.mmmZ`（SQLite の strftime('%Y-%m-%dT%H:%M:%fZ') と同じ形）。
/// 算法は src/google/mod.rs の rfc3339_utc と同じ civil_from_days（あちらは秒精度で adapter 側にあるので引かない）。
pub fn rfc3339(ms: u64) -> String {
    let (secs, milli) = (ms / 1000, ms % 1000);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
