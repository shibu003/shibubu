//! Adapter SDK（CON-001）。1 つの trait で capability discovery（Spec → domain の CapabilityDescriptor / ActionDefinition）・
//! auth と status（readiness）・action schema（input_schema）・read / write の意味（Effect）・provenance（domain の Observation）・
//! cleanup を持つ。events は registry（mod.rs）が readiness の差と実行結果から出す。
//! 型は domain の物を使い（AdapterKind も domain の capability.rs・CD-07 / CD-48）、domain に無い物だけここに置く: Route（CON-006）・SourceSync / Conflict（CON-010）。
//! 画面の段（DisplayTier と readiness の tier()・CD-02）は domain の capability.rs / `domain::binding` に在り、ここは「最後の結果」を足す tier_with だけ。
use super::super::domain::{
    action::{ActionDefinition, ActionReadiness, Effect},
    binding::{Source, SourceReadiness},
    capability::{CapabilityDescriptor, CapabilityReadiness, CapabilityRecord, DisplayTier, Origin, RecipeCall, Reuse, Verification, CAPABILITY_SCHEMA},
    run::{ErrorCode, Observation, Outcome},
};
pub use super::super::domain::capability::AdapterKind;
use super::super::google::rfc3339_utc;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// W19-3b G2: 裏の thread で長く待つ間の進行の 1 行の受け口（`Adapter::set_progress`）。Some = 待っている事の 1 行・None = 待ち終わった。
/// generated.rs の `EngineRequest::progress` と同じ形（generated.rs は adapters を持たない試験の crate にも入るので、名前でなく形で揃える）。
pub type Progress = std::sync::Arc<dyn Fn(Option<String>) + Send + Sync>;

/// 今の UNIX 秒（SourceSync の時計。表示と Observation は rfc3339_utc で文字列にする）
pub fn now() -> u64 { super::super::google::now() }

/// 1 つの action の静的な定義。action id = capability id（corpus と同じ 1 対 1）
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub id: &'static str,
    pub effect: Effect,
    /// 実行の前に人の承認が要る（external_write 以上と、他 app を戻せない形で変える local_write）
    pub approval: bool,
    /// 承認が束ねる scope。対象が入力で決まる action は Adapter::scopes で上書きする（信頼済 CLI の exec:<feature_id>:<指紋>）
    pub scopes: &'static [&'static str],
    /// input の JSON Schema（object）。parse できる事は register が確かめる
    pub input: &'static str,
    /// 検証の仕方（CD-04 (4) の語: read_back / state_compare / source_receipt / human_review）
    pub verifier: &'static str,
}

pub trait Adapter {
    /// provenance と event に出す名前（"github"）
    fn id(&self) -> &'static str;
    fn kind(&self) -> AdapterKind;
    fn route(&self) -> Route;
    /// capability の provider。CON-006 の制約表（CONSTRAINTS）の key にもなる
    fn provider(&self) -> &'static str;
    fn specs(&self) -> &'static [Spec];
    /// その時点の readiness（設定と、check / invoke の記録から）。registry は Ready の時だけ invoke を通す
    fn readiness(&self, action: &str) -> CapabilityReadiness;
    /// source ごとの状態（CON-010）。鮮度は domain の DataBinding::readiness(sources, stale_before) が last_observed_at から測る
    fn sources(&self) -> Vec<Source>;
    /// auth と疎通の確認（重い・明示して呼ぶ）。対象に依らない口を叩き、結果は SourceSync::record_check で readiness に効く。
    /// 操作が権限の拒否を返した後にも registry が呼ぶ（token 自体の拒否か、対象への 403 かを確かめる）
    fn check(&mut self);
    /// 実行。gate（登録・readiness・承認）は registry が済ませてから呼ぶ。読んだ物は Observation（どの source から・いつ）
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>);
    /// 後始末: 持っている認証と記録を捨てて初期に戻す
    fn cleanup(&mut self);
    /// 承認が束ねる scope。input が None なら宣言（取り得る全部: capabilities / actions に出す）、Some ならその入力の実行に要る物
    /// （registry の gate が見る）。既定は Spec の scopes
    fn scopes(&self, spec: &Spec, _input: Option<&Value>) -> Vec<String> { strings(spec.scopes) }
    /// W12a: この Adapter が読める Project の resource（`ProjectResource.kind`）と、その時に呼ぶ action の id の組。registry の
    /// `ResourceReader::read` が kind で引き、input `{"uri": resource.uri}` で invoke する（Core は kind も action も名指ししない）。
    /// 載せてよいのは `approval: false` で effect が LocalRead / ExternalRead の Spec だけ。既定は無し（resource を読まない Adapter）。
    fn reads(&self) -> &'static [(&'static str, &'static str)] { &[] }
    /// W12a: `check` の後も Ready でない時に、利用者が何を入れれば読めるようになるかの 1 行（画面に出せる文・action id を入れない・D-e）。
    /// registry の `ResourceReader::read` がこれを `Rejected{reason}` で返す（Core は Adapter の名を知らないので、入れる物を言えるのは Adapter だけ）。
    /// 既定は無し（`Failed{NotReady}` のまま・Core の一般の 1 行になる）。
    fn not_ready_line(&self) -> Option<&'static str> { None }

    fn capabilities(&self) -> Vec<CapabilityDescriptor> {
        self.specs().iter().map(|s| CapabilityDescriptor {
            id: s.id.into(),
            provider: Some(self.provider().into()),
            readiness: self.readiness(s.id),
            required_scopes: self.scopes(s, None),
            adapter: Some(self.kind().as_str().into()),
        }).collect()
    }
    fn actions(&self) -> Vec<ActionDefinition> {
        self.specs().iter().map(|s| ActionDefinition {
            id: s.id.into(),
            capability_ref: Some(s.id.into()),
            effect: s.effect,
            readiness: action_readiness(self.readiness(s.id)),
            input_schema: serde_json::from_str(s.input).unwrap_or_default(),
            output_schema: None,
            required_scopes: self.scopes(s, None),
            verifier: Some(s.verifier.into()),
            requires_approval: s.approval,
        }).collect()
    }

    /// W19-1: この Adapter の能力を Capability Registry の形で（registry の `records` が並べ、Engine の Capability Discovery が読む）。
    /// 既定（src/adapters/ の手書きの Adapter）は `official_records(self, Reuse::AppSpecific { app: provider })`。複数の app に使う手書きの Adapter
    /// （mac_gui の AX の能力）は `official_records(self, Reuse::Shared)` で上書きする。recipe の Adapter は recipe の reuse・origin・versions・verification を返す。
    /// Wave: W19-1
    /// 既定は全部 `Unverified`（Adapter は最後の結果を持たない。Verified に上げるのは registry の `records` が最後の証拠つきの結果を見て行う）。
    fn records(&self) -> Vec<CapabilityRecord> {
        official_records(self, Reuse::AppSpecific { app: self.provider().into() })
    }
    /// W19-3b G2: 長く待つ間の進行の 1 行を渡す口（registry の `set_progress` が run ごとに配る）。既定は捨てる（長く待たない Adapter）。
    /// MacGui は答えない app を読み直す間だけ「X が答えないので待っています（最長 N 秒）」を渡し、終わったら None（経路を問わず）。
    /// Wave: W19-3b
    fn set_progress(&mut self, _progress: Progress) {}
    /// ENGINE-ADJUST-1102 ①（owner 256・人の Stop）: 人の Stop の印を渡す口（`AdapterRegistry::set_stop` が run ごとに配る・`set_progress` と同じ形）。
    /// 長く読む・待つ Adapter（MacGui = bridge の more の続き・答えない app の読み直し・人の lease の待ち）は続きを頼む前に見て、立っていれば続きを頼まず
    /// `Rejected` の 1 行（読みかけは使わない・送っていない）。既定は捨てる（長く待たない Adapter）。
    fn set_stop(&mut self, _stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {}
    /// G74: 書く手が置いた物を、次の書く手（送る・押す・保存）の直前にもう一度読む口（送らない・承認も act の記録も要らない読み）。
    /// `capability_engine` の `run_steps` が、書く手の後ろの書く手を registry の `invoke` に渡す前に、前の書く手の input で呼ぶ。
    /// Some(Verified) = 今も置いた通り（何を読んで一致と言ったかの行つき）・Some(それ以外) = 違う・読めない → 後ろの手は走らせない・
    /// None = この action は読み直す口を持たない（今まで通り走る）。既定は None。
    /// Wave: M2
    fn recheck(&mut self, _action: &str, _input: &Value) -> Option<Outcome> { None }
    /// WF-006: 外へ効く action（`Effect::ExternalWrite`・`ExternalSend`・`Destructive`）を送る直前に、この Adapter が `invoke` の中で宛先を
    /// 読み直し（どの app・窓・channel・相手か）、違えば送らない、と宣言する（true）。Engine の `run_steps` は宣言の無い外へ効く手を 1 手も走らせない
    /// （adapter を問わない門・`WorkflowError::Unchecked`）。既定は false（宣言しない = 送らない側）。手元の書き込み（LocalWrite）は見ない。
    /// Wave: WF-E
    fn prechecks(&self, _action: &str) -> bool { false }
}

/// 手書きの Adapter の records（`Adapter::records` の既定の本体）: Spec ごとに `Origin::Official`・`reuse`・Adapter の kind・`Unverified`・Spec の scope。
/// Wave: W19-1
pub fn official_records<A: Adapter + ?Sized>(a: &A, reuse: Reuse) -> Vec<CapabilityRecord> {
    a.specs().iter().map(|s| CapabilityRecord {
        schema: CAPABILITY_SCHEMA.into(),
        id: s.id.into(),
        reuse: reuse.clone(),
        origin: Origin::Official,
        kind: a.kind(),
        verification: Verification::Unverified,
        versions: vec![],
        permissions: a.scopes(s, None),
        evidence: vec![],
        constraints: vec![],
    }).collect()
}
fn strings(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

/// W19-1: recipe の action を手段（CLI・MCP・AX・DOM・code・HTTP）ごとに実際に呼ぶ口。Core の配線が作り（`capability_engine::call_runner`）、
/// registry の `add_recipes` が recipe の Adapter に渡す。Adapter は手段の中身を知らずにこれを呼ぶ（手段は交換・組み合わせができる）。
/// Wave: W19-1
/// 保証する（作る側の約束）: gate（登録・readiness・承認）は registry が済ませた後にだけ呼ばれる・読んだ物は Observation で返す・
/// 手段が使えない時は `Outcome::Failed { error: NotReady }`（成功と返さない）。
pub type CallRunner = Box<dyn FnMut(&RecipeCall, &Value) -> (Outcome, Option<Observation>)>;

/// 旧 lane の文字列の口（`app.rs` の `execute` が `Result<String, String>` で受ける）: Verified / Unverified → Ok(1 行)・Rejected / EffectUnknown → Err(理由)・
/// Failed → Err(code の 1 行)。W19-1 で `mac_gui` から移した（Core は registry と sdk だけを呼ぶ・check_app_agnostic の direct）。
/// 保証: Unverified を Err にしない・Ok の文に「未検証」を残す（`app.rs` の `result_line` が「未検証」の語で「完了」にしない）。
pub fn legacy(o: Outcome) -> Result<String, String> {
    match o {
        Outcome::Verified { evidence } => Ok(evidence.join(" / ")),
        Outcome::Unverified { reason } if reason.contains("未検証") => Ok(reason),
        Outcome::Unverified { reason } => Ok(format!("{reason}（未検証）")),
        Outcome::Rejected { reason } | Outcome::EffectUnknown { reason } => Err(reason),
        Outcome::Failed { error } => Err(match error {
            ErrorCode::TargetNotFound => "対象の操作が窓に見つかりません。② で読み直して再登録してください",
            ErrorCode::Timeout => "画面の操作の応答がありません。元の app が応答しているか確かめてからもう一度",
            ErrorCode::NotReady => "画面の操作の部品（Shibubu Bridge）を起こせません。Shibubu を開き直してください",
            _ => "画面の操作に失敗しました。画面を読み直してからもう一度",
        }
        .into()),
    }
}

/// capability の readiness → action の readiness。Ready 以外を Ready にしない（domain validate の CapabilityNotReady を起こさない）
pub fn action_readiness(r: CapabilityReadiness) -> ActionReadiness {
    match r {
        CapabilityReadiness::Ready => ActionReadiness::Ready,
        CapabilityReadiness::Unsupported => ActionReadiness::Unsupported,
        CapabilityReadiness::SimulatedInPreview => ActionReadiness::PreviewOnly,
        _ => ActionReadiness::RequiresConnection,
    }
}

/// CON-006 の経路。並びがそのまま優先（official API > 許可された OS automation > licensed embed > external handoff）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Route { OfficialApi, OsAutomation, Embed, Handoff }
pub const ROUTES: [Route; 4] = [Route::OfficialApi, Route::OsAutomation, Route::Embed, Route::Handoff];
/// source ごとに使わない経路と理由（規約）。出典は docs/connectors/README.md の表
pub const CONSTRAINTS: &[(&str, Route, &str)] = &[
    ("claude.ai", Route::OsAutomation, "Anthropic Consumer Terms §3(7) が claude.ai / Claude Desktop の自動操作を禁じる（API key か handoff だけ）"),
    ("linkedin", Route::OsAutomation, "LinkedIn User Agreement 8.2(2)(13) の自動取得に当たる。使うかは owner の判断（既定は使わない）"),
    ("ycombinator", Route::OfficialApi, "公式 API は無い。無文書の endpoint は Terms of Use の scraping 禁止に当たる"),
    ("ycombinator", Route::OsAutomation, "Terms of Use の data mining / robots / scraping 禁止に当たる（既定は handoff）"),
];
/// 選んだ経路と、4 経路それぞれを採った / 採らなかった理由（CON-006 AC: inspectable）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteChoice { pub chosen: Option<Route>, pub considered: Vec<(Route, String)> }
/// 経路を 1 つ選ぶ。候補は (経路, readiness)。優先の高い順に、禁止でなく Ready な最初の物。どれも無ければ None（手で続ける）
pub fn choose_route(provider: &str, candidates: &[(Route, CapabilityReadiness)]) -> RouteChoice {
    let mut chosen = None;
    let considered = ROUTES.iter().map(|&route| {
        let best = candidates.iter().filter(|(r, _)| *r == route).map(|(_, x)| *x).max_by_key(|x| *x == CapabilityReadiness::Ready);
        let why = match (CONSTRAINTS.iter().find(|(p, r, _)| *p == provider && *r == route), best) {
            (Some((_, _, reason)), _) => format!("使わない: {reason}"),
            (None, None) => "adapter が無い".to_string(),
            (None, Some(r)) if r != CapabilityReadiness::Ready => format!("{r:?} のため使えない"),
            (None, Some(_)) if chosen.is_some() => "上の経路を採った".to_string(),
            (None, Some(_)) => { chosen = Some(route); "採用".to_string() }
        };
        (route, why)
    }).collect();
    RouteChoice { chosen, considered }
}

/// 段（domain の DisplayTier・CD-02）に最後の結果を足す: Ready で、最後の試行が証拠つきの Verified の時だけ Verified
/// （DESIGN §10「Verified = action produced evidence」）。readiness だけの段は CapabilityReadiness::tier
pub fn tier_with(r: CapabilityReadiness, last: Option<&Outcome>) -> DisplayTier {
    match (r.tier(), last.and_then(evidence_of)) {
        (DisplayTier::Ready, Some(_)) => DisplayTier::Verified,
        (t, _) => t,
    }
}

/// 証拠つきの Verified の根拠（空白でない項を " / " で繋ぐ）。根拠の無い Verified（自己申告）と他の結果は None
/// （YC_WAVES §1 の追加「製品の自己申告では verified にしない」）。`tier_with` と registry の `records` が同じ規則で読む。
/// Wave: W19-1
pub(super) fn evidence_of(o: &Outcome) -> Option<String> {
    let Outcome::Verified { evidence } = o else { return None };
    let e: Vec<&str> = evidence.iter().map(String::as_str).filter(|s| !s.trim().is_empty()).collect();
    (!e.is_empty()).then(|| e.join(" / "))
}

/// 1 本の接続（adapter の認証と疎通）の記録と、その接続で読む source ごとの鮮度（CON-010）。試行の Outcome を入れると
/// readiness（domain の 2 種）と再試行の時刻が出る。readiness とは別軸の鮮度（stale）は source ごとの last_observed_at を
/// domain の DataBinding::readiness に渡して測る（CD-02 (5)）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceSync {
    /// 最後に成功した試行（読み・書き・check のどれでも）。readiness の Ready はこれで決まる = 認証と疎通の証拠
    last_ok: Option<u64>,
    fault: Option<Fault>,
    failures: u32,
    last_fail: u64,
    /// source_ref → 最後に読めた時刻（Observation の observed_at）。Observation を返した読み取りだけが進める
    seen: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fault {
    /// 通信が切れた・時間切れ・利用上限。前の data と時刻は残す
    Offline,
    /// 認証を拒まれた（再接続する）
    Revoked,
    /// 実行先が無い・動いていない・版が違う（接続が必要）
    Missing,
}
impl SourceSync {
    /// 操作の試行 1 回を記録する。read = その試行が返した Observation（読み取りの時だけ）。書き込みの受領・CLI の exit 0・click・check の疎通は
    /// 接続の証拠（Ready）にはなるが、data を読んだ事にはならないので鮮度を進めない。
    /// 権限の拒否（PermissionDenied）はここでは状態を変えない: 403 は対象（repo・予定表・scope の要る操作）への権限不足で、token は他の操作に効く
    /// （AD-R2-3）。token 自体の拒否かは registry が check をやり直して確かめる（record_check が Revoked にする）
    pub fn record(&mut self, now: u64, o: &Outcome, read: Option<&Observation>) {
        let fault = match o {
            Outcome::Verified { .. } | Outcome::Unverified { .. } => {
                (self.last_ok, self.fault, self.failures, self.last_fail) = (Some(now), None, 0, 0);
                if let Some(obs) = read { self.seen.insert(obs.source_ref.clone(), obs.observed_at.clone()); }
                return;
            }
            Outcome::Failed { error: ErrorCode::NotReady } => Fault::Missing,
            Outcome::Failed { error: ErrorCode::ConnectionLost | ErrorCode::Timeout | ErrorCode::RateLimited } | Outcome::EffectUnknown { .. } => Fault::Offline,
            // 権限の拒否・gate で止めた・入力・対象・実行先自身の失敗は source の接続と関係ない
            _ => return,
        };
        self.fail(now, fault)
    }
    /// auth と疎通の確認（check）の結果。check は対象に依らない口（GitHub の /rate_limit・Gmail の profile・Calendar の primary）を叩くので、
    /// ここでの権限の拒否は token 自体の拒否 = Revoked（人が再接続するまで叩かない）
    pub fn record_check(&mut self, now: u64, o: &Outcome) {
        match o {
            Outcome::Failed { error: ErrorCode::PermissionDenied } => self.fail(now, Fault::Revoked),
            o => self.record(now, o, None),
        }
    }
    /// 実行先が無い（版が違う）と分かった。人が直して check するまで RequiresConnection
    pub fn missing(&mut self, now: u64) { self.fail(now, Fault::Missing) }
    fn fail(&mut self, now: u64, fault: Fault) {
        self.fault = Some(fault);
        self.failures = self.failures.saturating_add(1);
        self.last_fail = now;
    }
    pub fn capability(&self, configured: bool) -> CapabilityReadiness {
        use CapabilityReadiness::*;
        match (configured, self.fault) {
            (false, _) | (_, Some(Fault::Missing)) => RequiresConnection,
            (_, Some(Fault::Revoked)) => Revoked,
            (_, Some(Fault::Offline)) => Degraded,
            _ if self.last_ok.is_some() => Ready,
            _ => ConnectedUnverified,
        }
    }
    pub fn source(&self, configured: bool) -> SourceReadiness {
        match self.capability(configured) {
            CapabilityReadiness::Ready => SourceReadiness::Ready,
            CapabilityReadiness::Degraded => SourceReadiness::Degraded,
            CapabilityReadiness::ConnectedUnverified => SourceReadiness::ConnectedUnverified,
            _ => SourceReadiness::RequiresConnection,
        }
    }
    /// その source を最後に読めた時刻（RFC3339 UTC）。オフラインでも消さない = 前の表示に「いつの data か」を添えられる
    pub fn last_observed_at(&self, source: &str) -> Option<String> { self.seen.get(source).cloned() }
    /// 自動で check（読み取り）をやり直してよい最も早い時刻。オフラインの時だけ: 5 秒 × 2^(連続失敗-1)、上限 5 分。
    /// 認証の拒否・実行先が無い時は None（人が直すまで叩かない）。書き込みはここでは再送しない（DESIGN §11）
    pub fn retry_at(&self) -> Option<u64> {
        (self.fault == Some(Fault::Offline)).then(|| self.last_fail + (5u64 << self.failures.saturating_sub(1).min(6)).min(300))
    }
}
/// domain の Source を 1 つ作る（readiness と last_observed_at は SourceSync から。id は読み取りの Observation.source_ref と同じ名）
pub fn source(id: &str, kind: AdapterKind, provider: &str, sync: &SourceSync, configured: bool) -> Source {
    Source { id: id.into(), kind: kind.as_str().into(), readiness: sync.source(configured), scope: None, last_observed_at: sync.last_observed_at(id), provider: Some(provider.into()) }
}

/// 承認が束ねる exact payload の digest（ADR-005 §5・DESIGN §5.3）。action id と入力の正準 JSON の sha256（serde_json の Map は
/// key が sort 済みで to_string は空白を入れない。preserve_order は無効）。承認の記録（ConsentReceipt.payload_digest）を作る側もこれで作る
pub fn payload_digest(action: &str, input: &Value) -> String {
    super::super::crypto::sha256_hex(json!({"action_id": action, "input": input}).to_string().as_bytes())
}

/// 同じ物についての 2 つの観測が食い違った（CON-010）。両方の根拠（payload）と時刻（observed_at）を持つ。どちらを採るかは人が決める
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict { pub ours: Observation, pub theirs: Observation }
/// 2 つの観測を突き合わせる。中身が同じなら新しい方を返し、違えば黙って上書きせず Conflict を返す
pub fn reconcile(ours: Observation, theirs: Observation) -> Result<Observation, Conflict> {
    if ours.payload != theirs.payload { return Err(Conflict { ours, theirs }) }
    Ok(if theirs.observed_at >= ours.observed_at { theirs } else { ours })
}

/// 読んだ物の provenance（どの source から・いつ）。id は domain の約束どおり new_id の 10 進
pub fn observe(source: &str, now: u64, payload: Value) -> Observation {
    Observation { id: super::super::model::new_id().to_string(), source_ref: source.into(), observed_at: rfc3339_utc(now), payload }
}
/// 読み取り 1 回の結果。読めたら Verified（根拠 = 何を・いつ・どの observation）と Observation、だめなら Failed{code}
pub fn read_result(r: Result<Value, ErrorCode>, source: &str, what: &str, now: u64) -> (Outcome, Option<Observation>) {
    match r {
        Ok(v) => {
            let obs = observe(source, now, v);
            (Outcome::Verified { evidence: vec![format!("{what} · {} · observation {}", obs.observed_at, obs.id)] }, Some(obs))
        }
        Err(code) => (Outcome::Failed { error: code }, None),
    }
}
