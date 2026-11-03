//! adapters（unit AD・TODO CON-001 / 004 / 005 / 006 / 010）: connections の backend。
//! 6 つの adapter（GitHub・MacGui〈Shibubu Bridge の AX〉・Web JSON・信頼済 CLI・Google・git〈W12a・Project の repo を読む〉）を AdapterRegistry に登録し、domain の
//! CapabilityDescriptor / ActionDefinition / Source として見せ、gate（登録・readiness・承認 = app・run・action・scope・exact payload・未使用）を通してから実行する。
//! composer / renderer は domain の型しか見ないので、adapter を足しても変わらない（CON-001 の AC）。足し方は docs/connectors/README.md。
//! この module 配下は `crate::` を書かない（tests/adapters.rs が `#[path]` で main.rs と同じ並びに取り込む）。
#![allow(dead_code)] // main から呼ぶのは git の resource の読み（app.rs の with_local_adapters）と Connections の行（desktop）だけ・他の adapter は統合役が配線するまで呼び出しが無い
pub mod git;
pub mod github;
pub mod google;
pub mod mac_gui; // GUI 操作（bridge の ax_tree / ax_press / ax_set_value / ax_menu / key_to_pid）
pub mod recipe; // W19-1: 利用者の data の recipe を Adapter にする（Core は add_recipes だけを呼ぶ）
pub mod sdk;
pub mod trusted_cli;
pub mod web_json;

use super::domain::{
    action::{ActionDefinition, Effect},
    binding::Source,
    capability::{CapabilityDescriptor, CapabilityReadiness, CapabilityRecord, DisplayTier, Verification},
    permission::{ConsentDecision, ConsentReceipt, ConsentSubject},
    project::{ProjectResource, ResourceReader},
    run::{ErrorCode, Observation, Outcome},
};
use super::google::rfc3339_utc;
use super::runtime::registry::{ActionContract, ActionRegistry};
use sdk::{Adapter, CallRunner, RouteChoice};
use serde_json::{Map, Value};
use std::{collections::{HashMap, HashSet}, fmt, path::Path};

/// registry が出す event（CON-001 の events）。統合役が storage::event_bus へ `adapter.readiness` / `adapter.invoked` で流す
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    Readiness { action: String, from: CapabilityReadiness, to: CapabilityReadiness },
    Invoked { action: String, outcome: Outcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError { DuplicateAdapter(String), DuplicateAction(String), BadId(String), BadSchema(String) }
impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateAdapter(id) => write!(f, "adapter '{id}' は登録済み"),
            Self::DuplicateAction(id) => write!(f, "action '{id}' は別の adapter が持っている"),
            Self::BadId(id) => write!(f, "action id '{id}' は dot を含む小文字の名前でなければならない（CD-08）"),
            Self::BadSchema(id) => write!(f, "action '{id}' の input schema が JSON object ではない"),
        }
    }
}

/// CD-08: action / capability の id は dot を 1 つ以上含む小文字の名前（`github.issues.list`）。名前の似た別物を引かない
fn namespaced(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 128 && parts.len() >= 2 && parts[0].starts_with(|c: char| c.is_ascii_lowercase())
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'))
}
/// 実行を頼んだ側。承認の記録の app_id / run_id と照合する（RT は Run の app_id と id を渡す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller<'a> { pub app_id: &'a str, pub run_id: Option<&'a str> }

/// 承認の記録が「この app・run・action・scope・exact payload」への、まだ使っていない承認か。足りない物を 1 つ返す
/// （DESIGN §5.3「見せていない物の実行許可にはならない」・ADR-005 §5）。run_id の無い承認はその app のどの run でも 1 回
pub(crate) fn approval_gap(c: Option<&ConsentReceipt>, caller: Caller, action: &str, scopes: &[String], input: &Value, used: &HashSet<String>) -> Option<&'static str> {
    let Some(c) = c else { return Some("承認の記録が無い") };
    let ConsentSubject::Action { action_ref, run_id } = &c.subject else { return Some("この action への承認ではない") };
    if action_ref != action { return Some("この action への承認ではない") }
    if c.decision != ConsentDecision::Approved { return Some("却下されている") }
    if c.app_id != caller.app_id || run_id.as_deref().is_some_and(|r| caller.run_id != Some(r)) { return Some("別の app / run への承認") }
    if !scopes.iter().all(|s| c.scopes.contains(s)) { return Some("scope が足りない") }
    if c.payload_digest.as_deref() != Some(sdk::payload_digest(action, input).as_str()) { return Some("承認した内容と違います（payload の digest が一致しない）") }
    if used.contains(&c.id) { return Some("この承認は使用済み（1 回の承認で 1 回だけ実行する）") }
    None
}

/// r10 ①: adapter が何もしていないと言った結果（送っていない・押していない）= 承認を使用済みにしない（`AdapterRegistry::invoke`）。
fn untouched(o: &Outcome) -> bool {
    matches!(o, Outcome::Rejected { .. } | Outcome::Failed { error: ErrorCode::TargetNotFound | ErrorCode::TargetAmbiguous | ErrorCode::ApprovalDenied })
}

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn Adapter>>,
    /// action → (その試行の時刻 unix 秒, 結果)。時刻は records の Verified の at
    last: HashMap<String, (u64, Outcome)>,
    events: Vec<AdapterEvent>,
    /// adapter へ渡した承認の id（1 回の承認 = 1 回の実行）。cleanup でも消さない
    // ponytail: memory だけで再起動すると消える。永続の消費簿は RT / storage の consumed（ADR-005 §5）に移す
    used: HashSet<String>,
}

impl AdapterRegistry {
    /// W12a: 利用者の account・鍵・承認が要らない Adapter（今は `git::Git` だけ）を登録した registry。Core はこの型と
    /// `ResourceReader` だけを使う（`adapters::git` を直に呼ばない・scripts/check_app_agnostic.py の direct）。
    /// 例: `generated.set_resource_reader(Box::new(AdapterRegistry::with_local_adapters()));`（app.rs・起動の時に 1 回）
    /// 保証する: 登録の誤り（id の重複等）は起きない（固定の表・起きたら panic で build の試験が赤）・ここでは git を起こさない（check は最初の read で）。
    /// Wave: W12a
    pub fn with_local_adapters() -> Self {
        let mut r = Self::default();
        r.register(Box::new(git::Git::new())).expect("固定の表の登録は通る");
        r
    }

    /// R-G2: 画面の操作の adapter（`mac_gui::MacGui::new()`・provider "desktop"）だけを登録した registry。これを使う Core は Connections の行
    /// （`app.rs` の `gui_status`: `check` → `tier("desktop.ax.click")`）と、W19-1 から旧 Desktop パネルの ①②（`desktop.windows.list`・`desktop.window.state`）と
    /// ③ `execute`（`desktop.ax.click` / `desktop.ax.type_text`）。operator 2026-10-23 の既定: 許可・bridge が無い時は Adapter の 1 行（`not_ready_line`）・
    /// 旧 lane の「確認して実行」1 回 = 1 つの ConsentReceipt・app_name の無い旧 binding は MacGui の invoke が拒否の 1 行。
    /// 使い方: `let mut r = AdapterRegistry::desktop(); r.check(); let tier = r.tier("desktop.ax.click");`
    ///         `let (o, obs) = r.invoke(Caller { app_id: &session_id, run_id: None }, "desktop.windows.list", &json!({}), None);`
    /// 保証: 他の adapter を登録しない（`check` が GitHub / Google の通信をしない）・bridge は `check` / `invoke` まで起こさない。
    /// 保証しない: readiness（作った直後は ConnectedUnverified。読む前に `check` が要る）・承認（`desktop.ax.*`・`desktop.key` は ConsentReceipt が無ければ Rejected）。
    /// Wave: R-G2
    /// W19-2 の配線（図 §33）: Engine が Page の依頼で使う registry = 手元の Adapter（git）と画面の操作の Adapter（MacGui）。Engine は run ごとに 1 つ作り、
    /// 前に作った Adapter（`add_recipes`）を足して `check` してから `capability_engine::run` に渡す。Core はこの fn だけを呼ぶ（Adapter の module を名指さない）。
    /// 使い方: `let mut r = AdapterRegistry::with_engine_adapters(); r.add_recipes(&dir.join(EXTENSIONS_DIR), &mut runner); r.check();`
    /// 保証する: 登録の誤り（id の重複）は起きない（固定の表）・bridge と git は `check` / `invoke` まで起こさない・鍵の要る Adapter（GitHub・Google）を入れない。
    /// 保証しない: readiness（`check` の後に読む）・承認（`desktop.ax.*`・`desktop.key` は ConsentReceipt が無ければ Rejected）。
    /// Wave: W19-2
    pub fn with_engine_adapters() -> Self {
        let mut r = Self::with_local_adapters();
        r.register(Box::new(mac_gui::MacGui::new())).expect("git と desktop の action id は namespaced で重ならない");
        r
    }

    pub fn desktop() -> Self {
        let mut r = Self::default();
        r.register(Box::new(mac_gui::MacGui::new())).expect("mac_gui の action id は namespaced で互いに重ならない");        r
    }
    pub fn register(&mut self, adapter: Box<dyn Adapter>) -> Result<(), RegisterError> {
        if self.adapters.iter().any(|a| a.id() == adapter.id()) { return Err(RegisterError::DuplicateAdapter(adapter.id().into())) }
        for (i, s) in adapter.specs().iter().enumerate() {
            if !namespaced(s.id) { return Err(RegisterError::BadId(s.id.into())) }
            if self.owner(s.id).is_some() || adapter.specs()[..i].iter().any(|o| o.id == s.id) { return Err(RegisterError::DuplicateAction(s.id.into())) }
            if serde_json::from_str::<Map<String, Value>>(s.input).is_err() { return Err(RegisterError::BadSchema(s.id.into())) }
        }
        self.adapters.push(adapter);
        Ok(())
    }
    fn owner(&self, action: &str) -> Option<usize> { self.adapters.iter().position(|a| a.specs().iter().any(|s| s.id == action)) }
    pub fn capabilities(&self) -> Vec<CapabilityDescriptor> { self.adapters.iter().flat_map(|a| a.capabilities()).collect() }
    pub fn actions(&self) -> Vec<ActionDefinition> { self.adapters.iter().flat_map(|a| a.actions()).collect() }
    /// W19-2: 登録した Adapter の id（Engine が作る Adapter の名前が、手書きの Adapter・利用者が入れた recipe と重ならないかを見る）。
    /// Wave: W19-2
    #[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から呼ぶ
    pub fn adapter_ids(&self) -> Vec<&'static str> { self.adapters.iter().map(|a| a.id()).collect() }
    /// W19-3b G2: 全部の Adapter に進行の 1 行の受け口を渡す（`Adapter::set_progress`）。Engine は run ごとに registry を作った直後に 1 回（`engine_page::CoreEngine::run`）。
    /// Wave: W19-3b
    pub fn set_progress(&mut self, progress: sdk::Progress) { for a in &mut self.adapters { a.set_progress(progress.clone()) } }
    /// ENGINE-ADJUST-1102 ①: 人の Stop の印を全部の Adapter に配る（`Adapter::set_stop`・`set_progress` と同じ形・Engine の run ごとに `CoreEngine::run` が 1 回）
    pub fn set_stop(&mut self, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) { for a in &mut self.adapters { a.set_stop(stop.clone()) } }
    /// G74: `Adapter::recheck` を action の持ち主へ渡す（読むだけ・gate も承認の消費も通さない）。登録されていない action は None。
    /// Wave: M2
    pub fn recheck(&mut self, action: &str, input: &Value) -> Option<Outcome> { let i = self.owner(action)?; self.adapters[i].recheck(action, input) }
    /// WF-006: `Adapter::prechecks` を action の持ち主に聞く（登録されていない action は false）。
    /// Wave: WF-E
    pub fn prechecks(&self, action: &str) -> bool { self.owner(action).is_some_and(|i| self.adapters[i].prechecks(action)) }
    pub fn sources(&self) -> Vec<Source> { self.adapters.iter().flat_map(|a| a.sources()).collect() }
    /// 画面の段（DESIGN §10）。最後の試行が Verified で、今も Ready なら Verified
    pub fn tier(&self, action: &str) -> DisplayTier {
        self.owner(action).map_or(DisplayTier::Unavailable, |i| sdk::tier_with(self.adapters[i].readiness(action), self.last.get(action).map(|(_, o)| o)))
    }
    /// provider（"github" / "google" …）に使える経路を CON-006 の順で 1 つ選ぶ。理由は RouteChoice.considered に全部残る
    pub fn route_for(&self, provider: &str) -> RouteChoice {
        let candidates: Vec<_> = self.adapters.iter().filter(|a| a.provider() == provider).map(|a| {
            let ready = a.specs().iter().any(|s| a.readiness(s.id) == CapabilityReadiness::Ready);
            (a.route(), if ready { CapabilityReadiness::Ready } else { a.specs().first().map_or(CapabilityReadiness::Unknown, |s| a.readiness(s.id)) })
        }).collect();
        sdk::choose_route(provider, &candidates)
    }
    fn snapshot(&self, i: usize) -> Vec<(&'static str, CapabilityReadiness)> {
        self.adapters[i].specs().iter().map(|s| (s.id, self.adapters[i].readiness(s.id))).collect()
    }
    fn diff(&mut self, i: usize, before: Vec<(&'static str, CapabilityReadiness)>) {
        for (action, from) in before {
            let to = self.adapters[i].readiness(action);
            if to != from { self.events.push(AdapterEvent::Readiness { action: action.into(), from, to }) }
        }
    }
    /// 全 adapter の auth と疎通を確かめる（Connections を開いた時・Run の前に 1 回: ADR-003 C11）
    pub fn check(&mut self) {
        for i in 0..self.adapters.len() {
            let before = self.snapshot(i);
            self.adapters[i].check();
            self.diff(i, before);
        }
    }
    /// 唯一の実行口。登録（id の完全一致）→ readiness が Ready → 承認が要る物は承認の記録（approval_gap）→ adapter。
    /// 止めた時は送っていない（Rejected）。承認は adapter が送ったかもしれない時に使用済みにする（結果が失敗でも再利用させない）。
    /// r10 ①: adapter が何もしていないと言った（`untouched` = Rejected・対象が無い・1 つに絞れない・押す直前に承認の要る手と分けた）時は使用済みにしない =
    /// 同じ run の同じ内容のやり直し（W5 の探す段の後に同じ手から）に同じ承認が効く。送った後の 2 回目は今まで通り使用済み
    pub fn invoke(&mut self, caller: Caller, action: &str, input: &Value, consent: Option<&ConsentReceipt>) -> (Outcome, Option<Observation>) {
        let reject = |reason: String| (Outcome::Rejected { reason }, None);
        let Some(i) = self.owner(action) else { return reject(format!("action '{action}' は登録されていない")) };
        let Some(spec) = self.adapters[i].specs().iter().find(|s| s.id == action).copied() else { return reject(format!("action '{action}' は登録されていない")) };
        let readiness = self.adapters[i].readiness(action);
        // 確かめた後も Ready でない時は、何を入れれば使えるかを言える Adapter の 1 行をそのまま出す（operator 2026-10-23 (B)・D-e）。
        // 確かめる前（ConnectedUnverified）は定型の 1 行（まだ何も分かっていないので Adapter の理由を言わない）
        if readiness != CapabilityReadiness::Ready {
            let line = (readiness != CapabilityReadiness::ConnectedUnverified).then(|| self.adapters[i].not_ready_line()).flatten();
            return reject(line.map_or_else(|| format!("'{action}' は {readiness:?} のため実行しない（接続と確認が先）"), str::to_owned))
        }
        if spec.approval {
            let scopes = self.adapters[i].scopes(&spec, Some(input));
            if let Some(why) = approval_gap(consent, caller, action, &scopes, input, &self.used) { return reject(format!("'{action}' は承認が要る（{why}）")) }
        }
        let before = self.snapshot(i);
        let t = sdk::now();
        let (outcome, observation) = self.adapters[i].invoke(action, input);
        if let (true, Some(c), false) = (spec.approval, consent, untouched(&outcome)) { self.used.insert(c.id.clone()); }
        // 権限の拒否は「この対象への権限不足（403）」か「token 自体の拒否（401）」か outcome から分からない。auth の確認をやり直して、
        // token 自体が拒まれた時だけ Revoked にする（1 つの対象への 403 で、token の要らない読み取りまで止めない: AD-R2-3）
        if outcome == (Outcome::Failed { error: ErrorCode::PermissionDenied }) { self.adapters[i].check() }
        self.last.insert(action.into(), (t, outcome.clone()));
        self.events.push(AdapterEvent::Invoked { action: action.into(), outcome: outcome.clone() });
        self.diff(i, before);
        (outcome, observation)
    }
    /// runtime への橋（統合 2 の merge 条件 3・CD-01・RT-SEC-R2-1）: action の Spec を runtime の ActionContract に写す。runtime の gate が見る
    /// scope は app の文書ではなくこの契約（Spec.scopes = adapter が本当に要る物）から取る。adapter 名は provenance の id（sync_readiness と同じ key）。
    /// 入力で決まる scope（信頼済 CLI の exec:<feature_id>）は static の契約に載らないので空になり、runtime の gate が拒否する（閉じる側）
    // ponytail: CLI を runtime から呼ぶ時に、契約の scope を入力の関数（leases と同じ形）にする
    pub fn contract(&self, action: &str) -> Option<ActionContract> {
        let i = self.owner(action)?;
        let spec = self.adapters[i].specs().iter().find(|s| s.id == action)?;
        Some(ActionContract { adapter: self.adapters[i].id(), scopes: spec.scopes, ..ActionContract::local(spec.effect) })
    }
    /// adapter ごとの今の readiness を runtime の registry に写す（runtime は adapter 単位で dispatch を止める）。action のどれかが Ready なら Ready
    pub fn sync_readiness(&self, runtime: &ActionRegistry) {
        for a in &self.adapters {
            let states: Vec<CapabilityReadiness> = a.specs().iter().map(|s| a.readiness(s.id)).collect();
            let best = if states.contains(&CapabilityReadiness::Ready) { CapabilityReadiness::Ready } else { states.first().copied().unwrap_or(CapabilityReadiness::Unknown) };
            runtime.set_adapter_state(a.id(), best);
        }
    }
    pub fn drain_events(&mut self) -> Vec<AdapterEvent> { std::mem::take(&mut self.events) }
    /// 後始末: 各 adapter の認証と記録を捨てる（CON-001 の cleanup）
    pub fn cleanup(&mut self) {
        for i in 0..self.adapters.len() {
            let before = self.snapshot(i);
            self.adapters[i].cleanup();
            self.diff(i, before);
        }
        self.last.clear();
    }

    /// W19-1: Adapter を外す（Core を変えずに能力を足す・外す）。外した後のその action の `invoke` は「登録されていない」の Rejected・`tier` は Unavailable。
    /// Wave: W19-1
    /// 保証する: 使用済みの承認の記録は消さない（外して入れ直しても同じ承認で 2 回実行させない）・None = その id は登録されていない。
    pub fn unregister(&mut self, adapter_id: &str) -> Option<Box<dyn Adapter>> {
        let i = self.adapters.iter().position(|a| a.id() == adapter_id)?;
        let gone = self.adapters.remove(i);
        for s in gone.specs() { self.last.remove(s.id); }
        Some(gone)
    }

    /// W19-1: 登録した全 Adapter の能力を Capability Registry の形で（`Adapter::records` を並べる）。Engine の Capability Discovery が読む。
    /// Wave: W19-1
    /// 保証する: Adapter の records のうち Unverified の物は、この registry での最後の試行が証拠つきの `Outcome::Verified` の時だけ
    /// `Verified { at: その時刻, evidence: その根拠 }` に上げる（受理だけ・拒否・失敗・根拠の無い Verified〈自己申告〉では上げない・
    /// 調査しただけの能力は Ready にならない。規則は `sdk::evidence_of`）。
    /// 保証しない: 今も同じ結果になる事（`capability_engine::needs_reverify`）。
    pub fn records(&self) -> Vec<CapabilityRecord> {
        self.adapters.iter().flat_map(|a| a.records()).map(|mut r| {
            if let (Verification::Unverified, Some((t, o))) = (&r.verification, self.last.get(&r.id)) {
                if let Some(evidence) = sdk::evidence_of(o) { r.verification = Verification::Verified { at: rfc3339_utc(*t), evidence } }
            }
            r
        }).collect()
    }

    /// W19-1: `dir`（`<data dir>/extensions`・`domain::capability::EXTENSIONS_DIR`）の recipe を Adapter にして登録する。Core は recipe の module を
    /// 名指さず、これだけを呼ぶ（Adapter を足すのに Core の diff 0）。
    /// Wave: W19-1
    /// 例: `let lines = registry.add_recipes(&data_dir.join(EXTENSIONS_DIR), &mut || capability_engine::call_runner(mcp.clone(), data_dir.clone()));`
    /// 保証する: 読めない・載らない recipe は登録せず、画面に出せる 1 行ずつで返す（他の recipe は登録する）・同じ id は `DuplicateAdapter` の 1 行・dir が無ければ空。
    /// 保証しない: 登録した recipe が今動く事（readiness は各 action の verification と `check`）。
    pub fn add_recipes(&mut self, dir: &Path, runner: &mut dyn FnMut() -> CallRunner) -> Vec<String> {
        recipe::load_dir(dir).into_iter().filter_map(|r| match r.and_then(|r| recipe::RecipeAdapter::new(r, runner())) {
            Err(e) => Some(e.to_string()),
            Ok(a) => self.register(Box::new(a)).err().map(|e| format!("追加の機能: {e}")),
        }).collect()
    }
}

/// W12a: resource を読む唯一の道。`resource.kind` を `Adapter::reads` に持つ最初の Adapter を引き、readiness が Ready でなければ
/// その Adapter の `check` を 1 回してから、`invoke`（gate を通る唯一の実行口）を input `{"uri": resource.uri}` で呼ぶ。
/// 保証する: 読む Adapter が無い・uri が無い → `Rejected{reason: 画面に出せる 1 行}`（action id・内部の語を入れない）・check の後も Ready でない →
/// その Adapter の `not_ready_line`（何を入れれば読めるか）が在れば `Rejected{reason: その 1 行}`・無ければ `Failed{NotReady}`
/// （どれも Observation は None・何も起こさない）・承認の要る action は呼ばない（`reads` の約束・載っていても Rejected）。保証しない: 結果の中身の形（その Adapter の doc）。
impl ResourceReader for AdapterRegistry {
    fn read(&mut self, resource: &ProjectResource) -> (Outcome, Option<Observation>) {
        let kind = resource.kind.as_str();
        let reject = |reason: String| (Outcome::Rejected { reason }, None);
        let Some(uri) = resource.uri.as_deref() else { return reject(format!("この {kind} には読む場所がありません")) };
        let found = self.adapters.iter().enumerate().find_map(|(i, a)| a.reads().iter().find(|(k, _)| *k == kind).map(|&(_, action)| (i, action)));
        // reads に載せてよいのは承認の要らない読みだけ（sdk の約束）。載っていても書く・送る物は呼ばない（次の Adapter も探さない）
        let read_only = |&(i, action): &(usize, &str)| self.adapters[i].specs().iter().find(|s| s.id == action).is_some_and(|s| !s.approval && matches!(s.effect, Effect::LocalRead | Effect::ExternalRead));
        let Some((i, action)) = found.filter(read_only) else { return reject(format!("この版では {kind} を読めません")) };
        if self.adapters[i].readiness(action) != CapabilityReadiness::Ready {
            let before = self.snapshot(i);
            self.adapters[i].check();
            self.diff(i, before);
            if self.adapters[i].readiness(action) != CapabilityReadiness::Ready {
                return match self.adapters[i].not_ready_line() {
                    Some(line) => reject(line.to_owned()),
                    None => (Outcome::Failed { error: ErrorCode::NotReady }, None),
                };
            }
        }
        // 承認の要らない読みなので caller は承認の照合に使われない（resource の id を provenance に残すだけ）
        self.invoke(Caller { app_id: "resource", run_id: None }, action, &serde_json::json!({ "uri": uri }), None)
    }
}
