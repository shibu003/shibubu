//! 中央の Action Registry（RUN-001）: 登録された id だけが呼べる（完全一致）。id ごとに side-effect class（domain の Effect）、
//! timeout / retry / lease の契約（CD-04 (4) の ActionContract）と handler を持つ。adapter ごとの今の状態は CapabilityReadiness。
//! v3 の定義（AppDefinitionV3）には runtime の field を足さない: 契約はここに置く（CD-04 (4)）。
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use super::domain::action::{ActionDefinition, ActionReadiness, Effect};
use super::domain::capability::CapabilityReadiness;
use super::domain::patch::AppRevision;
use super::domain::run::{ErrorCode, Observation, Outcome};
use super::lease::LeaseKey;
use super::storage::{new_id, redact_value, Store, Table};
use super::{internal, lock, now_rfc3339};

/// runtime 側の action: domain の AppPatch を当てる（app が宣言しなくても呼べる）。
pub const UI_PATCH: &str = "ui.patch";
/// 端末の中だけで完結する action の adapter 名（常に ready）。
pub const LOCAL: &str = "local";

/// 1 つの action の実行契約。
#[derive(Clone)]
pub struct ActionContract {
    /// 本当の副作用の大きさ。app の宣言（actions[].effect）がこれと違えば invoke は拒否する。
    pub effect: Effect,
    /// adapter 名（`set_adapter_state` の key）。
    pub adapter: &'static str,
    /// この action が本当に要る scope（`provider.resource.verb`）。gate は app の宣言に足して見る（registry が正・RT-SEC-R2-1）。
    /// 外へ出る effect と local でない adapter は空にできない（空なら adapter の登録の誤りとして invoke を拒否する）。
    pub scopes: &'static [&'static str],
    /// 1 回の execute / verify の上限。過ぎたら待つのをやめる（書込なら EffectUnknown）。
    pub timeout: Duration,
    /// 再試行の回数。読取（local_read / external_read）だけに効く。書込は照合するまで送り直さない。
    pub max_retries: u32,
    /// 再試行の待ちの基準（attempt ごとに 2 倍・30 秒で頭打ち）。
    pub backoff: Duration,
    /// 押さえる対象（同じ window / repo / CLI / browser profile に同時に 2 つ触らない）。入力から決める。
    pub leases: fn(&Value) -> Vec<LeaseKey>,
}

impl ActionContract {
    /// local の既定: timeout 5 秒・読取だけ 2 回まで再試行・lease なし。外部 adapter は `..ActionContract::local(e)` で上書きする。
    pub fn local(effect: Effect) -> Self {
        Self {
            effect,
            adapter: LOCAL,
            scopes: &[],
            timeout: Duration::from_secs(5),
            max_retries: if is_read(effect) { 2 } else { 0 },
            backoff: Duration::from_millis(100),
            leases: |_| Vec::new(),
        }
    }
}

pub fn is_read(effect: Effect) -> bool {
    matches!(effect, Effect::LocalRead | Effect::ExternalRead)
}

/// handler は別 thread で動く（timeout のため）。store の lock を持ったまま待たない（executor が結果を書けなくなる）。
pub trait Handler: Send + Sync {
    /// 副作用を起こす。Ok は「受理された」で完了ではない（RUN-004）。書く直前に `ctx.cancelled()` を見る（RUN-008）。
    fn execute(&self, ctx: &Ctx, input: &Value) -> Result<Value, ErrorCode>;
    /// 対象を読み直して確かめる。既定は確かめる手段が無い = Unverified（受理を Verified にしない）。
    fn verify(&self, ctx: &Ctx, input: &Value, accepted: &Value) -> Outcome {
        let _ = (ctx, input, accepted);
        Outcome::Unverified { reason: "この action には読み直す手段が無い".into() }
    }
}

/// handler に渡す実行の文脈。
#[derive(Clone)]
pub struct Ctx {
    pub run_id: String,
    pub session_id: String,
    /// この attempt の期限（外部 adapter は自分の I/O の timeout に使う）。
    pub deadline: Instant,
    pub(super) store: Arc<Mutex<Store>>,
    pub(super) app: Arc<Mutex<AppRevision>>,
    pub(super) cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }
    /// 今の app revision（ui.patch が入れ替える）。lock の順は app → store（逆にしない）。
    pub fn app(&self) -> MutexGuard<'_, AppRevision> {
        lock(&self.app)
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    /// 観測した事実を evidence 表に残して id を返す（Outcome::Verified の根拠・Receipt の observation_refs）。payload は redact する。
    pub fn observe(&self, source_ref: &str, payload: Value) -> Result<String, ErrorCode> {
        let obs = Observation { id: new_id(), source_ref: source_ref.into(), observed_at: now_rfc3339(), payload: redact_value(&payload) };
        let json = serde_json::to_value(&obs).map_err(internal)?;
        self.store().put(Table::Evidence, &obs.id, Some(&self.run_id), &json).map_err(internal)?;
        Ok(obs.id)
    }
}

pub struct Registered {
    pub contract: ActionContract,
    pub handler: Arc<dyn Handler>,
}

#[derive(Default)]
pub struct ActionRegistry {
    /// 起動時の登録（`register`）に、app を開いた時に定義から導く local action（`register_if_absent`）が足される
    actions: RwLock<HashMap<String, Arc<Registered>>>,
    adapters: Mutex<HashMap<String, CapabilityReadiness>>,
}

impl ActionRegistry {
    /// corpus 01〜04 の local action と ui.patch を登録した物（local_actions.rs）。
    pub fn with_local_actions() -> Self {
        let mut reg = Self::default();
        super::local_actions::register(&mut reg);
        reg
    }

    /// 同じ id の 2 回目は置き換える（外部 adapter が local の仮実装を差し替える時）。
    pub fn register(&mut self, id: &str, contract: ActionContract, handler: impl Handler + 'static) {
        let actions = self.actions.get_mut().unwrap_or_else(|e| e.into_inner());
        actions.insert(id.into(), Arc::new(Registered { contract, handler: Arc::new(handler) }));
    }

    /// 開いた app の定義から導いた action を足す。同じ id が既に在れば置き換えない（起動時の登録と先に導いた物が正）。足したら true。
    pub fn register_if_absent(&self, id: &str, contract: ActionContract, handler: impl Handler + 'static) -> bool {
        let mut actions = self.actions.write().unwrap_or_else(|e| e.into_inner());
        if actions.contains_key(id) {
            return false;
        }
        actions.insert(id.into(), Arc::new(Registered { contract, handler: Arc::new(handler) }));
        true
    }

    /// 完全一致だけ。名前の似た物を引かない。
    pub fn get(&self, id: &str) -> Option<Arc<Registered>> {
        self.actions.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.actions.read().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        ids.sort_unstable();
        ids
    }

    /// adapter の今の状態（接続・失効・劣化）。外部 adapter が接続の確認や revoke の時に書く。
    pub fn set_adapter_state(&self, adapter: &str, state: CapabilityReadiness) {
        lock(&self.adapters).insert(adapter.into(), state);
    }

    /// local は常に ready。他は set されるまで Unknown（= 呼べない）。
    pub fn adapter_ready(&self, adapter: &str) -> bool {
        let fallback = if adapter == LOCAL { CapabilityReadiness::Ready } else { CapabilityReadiness::Unknown };
        let state = lock(&self.adapters).get(adapter).copied().unwrap_or(fallback);
        matches!(state, CapabilityReadiness::Ready | CapabilityReadiness::Degraded)
    }
}

/// app が宣言しなくても呼べる runtime 側の action の定義（今は ui.patch だけ）。invoke はこれを足した定義で
/// app の action と同じ validate_invocation に通す。入力 = agent_proposal_v1 の `change.operations[]`。
pub fn builtin_definition(id: &str) -> Option<ActionDefinition> {
    if id != UI_PATCH {
        return None;
    }
    let schema = json!({
        "type": "object",
        "required": ["operations"],
        "properties": { "operations": { "type": "array" }, "app_version": { "type": "integer", "minimum": 1 } },
        "additionalProperties": false
    });
    Some(ActionDefinition {
        id: UI_PATCH.into(),
        capability_ref: None,
        effect: Effect::LocalWrite,
        readiness: ActionReadiness::Ready,
        input_schema: schema.as_object().cloned().unwrap_or_default(),
        output_schema: None,
        required_scopes: Vec::new(),
        verifier: Some("local_state".into()),
        requires_approval: false,
    })
}
