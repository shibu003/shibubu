//! Shibubu Engine の pipeline（owner 2026-10-21 18・19 通目・docs/ANALYSIS_BACKEND.md §3）: 利用者の目的を操作に分け、既存の能力を発見・再利用し、
//! 足りない所だけ生成し、元の app・service・file の実状態で確かめてから使い、成功した物を registry に残して 2 回目は作らずに使う。
//! Goal Interpretation → Capability Discovery →（足りなければ）Deep Software Analysis → Integration Planning → Dynamic Generation →
//! Verification → Execution → Registry / Reuse。
//! Wave: W19-2
//! ここは Core の部品をつなぐだけ: 発見は `adapters::AdapterRegistry`（registry だけ・Adapter を名指さない）と `mcp`（tools/list）と保存した能力、
//! 生成は recipe（`domain::capability::Recipe`・利用者の data）と Workflow、model は `llm::Llm`（利用者が login 済みの CLI・W13・
//! 特定の LLM の製品や plan を唯一の道にしない）。app 専用の code をここに書かない（`scripts/check_app_agnostic.py`）。
//! 人が本物の app を直接操作する道（Native Direct・Mirror）はここを通らない（Agent の道と独立・18 通目 §4）。
//! 保存形式: `<data dir>/capabilities/<id>.json`（`CapabilityRecord` 1 件 1 file・`save_record`）・`<data dir>/extensions/<id>/recipe.json`（生成した Adapter・`save_recipe`）・
//! `<data dir>/workflows/<id>.json`（生成した手順と成功の確かめ方・`save_workflow`）・`<data dir>/workflows/trials/<手順の id>.json`（確かめた試験と Adapter の id・
//! 版・OS・権限が変わった時に作り直さずに流し直す）・`<data dir>/trials/<操作>/`（隔離した試験の写し・run ごとに作り直す）・
//! Engine の記録 `EngineRecord`（`ENGINE_SCHEMA`・run ごとに 1 つ・evidence に残す）。
#![allow(dead_code)] // W19-2 の配線（図 §33）で Page の欄から `run` を呼ぶ。discover・choose・needs_reverify 等の単体の口は試験と W19-3 からだけ呼ばれる

use crate::adapters::sdk::{self, CallRunner};
use crate::adapters::{AdapterRegistry, Caller};
use crate::domain::action::{ActionDefinition, Effect};
use crate::domain::capability::{
    AdapterKind, CapabilityReadiness, CapabilityRecord, DisplayTier, Origin, Recipe, RecipeAction, RecipeCall, Reuse, Verification, CAPABILITIES_DIR, CAPABILITY_SCHEMA,
    EXTENSIONS_DIR, RECIPE_FILE, RECIPE_SCHEMA,
};
use crate::domain::chat::{OperationKind, Situation};
use crate::domain::permission::{ConsentReceipt, ConsentSubject};
use crate::domain::run::{ErrorCode, Observation, Outcome, WorkflowCursor};
use crate::intel::analysis::{
    self, AnalysisBackend, AnalysisConsent, AnalysisError, AnalysisReport, AnalysisRequest, AnalysisTarget, Availability, FindingStatus, Placement, ReaBackend, ToolProbe,
    ANALYSES_DIR, REA_SERVER,
};
use crate::llm::{short, JsonRequest, Llm, LlmError};
use crate::mcp::{self, McpConfig, McpError, McpTool, McpToolHints};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// 段ごとの部品（子は `use super::*` でこの file の名前を引き、外へは下の `pub use` で今までどおり `capability_engine::X` に出す）
mod deep_analysis; // W19-3: Deep Software Analysis（REA などの Analysis Backend の結果を Capability の候補にする）
mod recipe; // 呼ぶ・隔離した写しで試す・model に作らせる・読み戻しで確かめる・置き場
mod workflow; // W19-2b: 複数の service の能力をつないだ Workflow
pub use deep_analysis::*;
pub use recipe::*;
pub use workflow::*;

/// `EngineRecord::schema` の値。
/// Wave: W19-2
pub const ENGINE_SCHEMA: &str = "shibubu.engine-run/1";

/// pipeline の段（19 通目 §3 の 8 つ・この順でしか進まない）。
/// Wave: W19-2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    GoalInterpretation,
    CapabilityDiscovery,
    /// 通常の発見で足りない時だけ（REA などの Analysis Backend・W19-3）
    DeepAnalysis,
    IntegrationPlanning,
    /// 既存の機能で足りない部分だけ（recipe・code・UI・Workflow）
    DynamicGeneration,
    Verification,
    Execution,
    Registry,
}

impl Stage {
    /// 進む順（`run` はこの順で `EngineRecord::stages` に 1 行ずつ足す）。
    pub const ALL: [Stage; 8] = [
        Stage::GoalInterpretation,
        Stage::CapabilityDiscovery,
        Stage::DeepAnalysis,
        Stage::IntegrationPlanning,
        Stage::DynamicGeneration,
        Stage::Verification,
        Stage::Execution,
        Stage::Registry,
    ];
}

/// 目的を分けた操作 1 つ（Goal Interpretation の出力）。
/// Wave: W19-2
/// ENGINE-AUDIT W9（節 14・P0 の代用の穴）: `kind`・`named`・`candidates` を足した（serde default = 前の記録と保存した手順はそのまま読める）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    /// 何をするか（利用者の言葉に近い 1 行）
    pub want: String,
    /// 成功の条件（元の app・service・file の何が、どう変われば成功か）
    pub success: String,
    /// 対象の app / service（bundle id か origin）。Page の面から決まる時は面の target
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// W9: 操作の種類（act = Engine・change = composer・question = 会話・nothing）。振り分けは呼ぶ側（本体 = `run_stages` は act だけ走らせる）
    #[serde(default)]
    pub kind: OperationKind,
    /// W9: 目的が app・service を名指したか。名指したのに `target` が決まらない（null）時は Page の app で埋めない（P0「名指しの対象を代用しない」）
    #[serde(default)]
    pub named: bool,
    /// W9: 宛先が 2 つ以上あり得る時の候補（bundle id か origin）。決まる時は空。2 つ以上なら Engine が 1 回だけ聞く（`StepKind::Decide`）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
}

/// Integration Planning の比べ方（19 通目 §3 の 6 つ）。どれも 0〜3（大きいほど良い）。
/// Wave: W19-2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    pub feasibility: u8,
    pub fidelity: u8,
    pub permission: u8,
    pub stability: u8,
    pub cost: u8,
    pub environment: u8,
}

/// 操作ごとに選んだ能力と理由（Engine の記録の「選んだ理由」）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub operation: String,
    /// `CapabilityRecord::id`
    pub capability: String,
    pub reason: String,
    pub score: Score,
}

/// 段 1 つの結果。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum StageOutcome {
    Done { note: String },
    /// その段が要らなかった（2 回目で生成 0 回・発見で足りたので解析しない等）
    Skipped { why: String },
    Failed { why: String },
}

/// Wave: W19-2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageLog {
    pub stage: Stage,
    pub outcome: StageOutcome,
    pub ms: u64,
    /// ENGINE-AUDIT W1（254「どの段で何を見たかを 1 行ずつ」・節 16）: 段の数字（JSON の object・無ければ None = 前の記録）。key は段ごと:
    /// GoalInterpretation `{ai}`（AI の回数）・CapabilityDiscovery `{found, verified, mcp}`・DynamicGeneration `{ai, retries, elements, shown, windows}`
    /// （AI の回数・作り直し・読んだ要素の数・AI に見せた行の数・窓の数）・Verification `{reads}`・Execution `{steps, readback_ms}`。
    /// 既存の型に足した理由: 段の ms と並べて 1 行で読む物で、`EngineRecord` の外に別の表を作らない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<Value>,
}

/// Engine の記録（W19-2 の受け入れ: 分けた操作・見つけた手段・選んだ理由・作った物・検証）。run ごとに 1 つ・`docs/evidence/W19-2/` に残す。
/// Wave: W19-2
/// 使い方: `let rec = capability_engine::run(goal, &mut cx)?; serde_json::to_writer(file, &rec)?`
/// 保証する: `stages` は `Stage::ALL` の順・2 回目の同じ目的は `generated` が空で `reused = true`（registry の成果物を優先）・`verified` は `verify` の読み戻しの行だけ・
/// `receipts` は走らせた手順ごとに 1 つ（W19-2b: 受け渡した値は digest だけ）。
/// 保証しない: 受け入れの verified（正は試験の側の oracle。ここの `verified` は製品の記録）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRecord {
    pub schema: String,
    pub goal: String,
    pub operations: Vec<Operation>,
    /// 見つけた手段（公式・解析の候補・生成した物・既存の Adapter）
    pub discovered: Vec<CapabilityRecord>,
    pub choices: Vec<Choice>,
    /// 作った物の id（recipe・code の artifact・Workflow・UI の Project）
    pub generated: Vec<String>,
    /// 実状態で確かめた事の 1 行（何を・どの道で読んだか）
    pub verified: Vec<String>,
    /// W19-2b: Execution で走らせた手順ごとの記録（各手の Outcome と、service の間で受け渡した値の digest）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<WorkflowReceipt>,
    pub stages: Vec<StageLog>,
    /// registry の成果物を使い、生成しなかった
    pub reused: bool,
    /// WF-007: 走って読み戻しも合ったが、字を置く書く手の「頼んだ差分」を読み戻していない（`Expect::Replaced` か置いた字との `Equals` が無い）=
    /// 済みと出さない理由の 1 行（Page は Partial・runs 表は uncertain）。None = 頼んだ差分まで確かめた
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// WF-036: Engine の AI が使えず（不在・時間切れ・上限・答えの形）、AI 無しの道（目的をそのまま 1 つの操作・確かめ済みの能力の 1 手）で
    /// 走った理由の 1 行（run と画面に出す）。None = AI を使った・初めから AI が無い
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub without_ai: Option<String>,
    /// ENGINE-AUDIT W7（spec 261「既に選ばれていれば『もうそうなっています』」）: 手を 1 つも打たなかった理由 = どの操作も押す手 1 つで、手の前の読みで Core が手から
    /// 作った読み戻し（`recipe::click_checks` = 押す行が選ばれている）がもう合っていた時の 1 行（`recipe::already`・何が・どの app で）。Some の時 `receipts` は空・
    /// `verified` はその読み。押す手が 2 つ以上の手順は打つ（前の手が文脈を変える = 別の track を選んでから音色）。
    /// Page は「Already like that」の 1 行（✓ でも Partial でもない・ATTACH）。None = 手を打った・打てなかった
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub already: Option<String>,
}

/// WF-036: AI 無しの道に戻った印（`EngineError::NoMeans::tried` の 1 行の頭・`engine_page::failure_line` が拾って 1 行にする）。
/// Wave: WF-E
pub const AI_NOT_USED: &str = "AI not used: ";

/// 再検証が要るかを決める今の環境（19 通目 §3「版・環境・権限が変わったら再検証」）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvFacts {
    /// 対象の app の版（Info.plist の CFBundleShortVersionString）か service の origin
    #[serde(default)]
    pub target_version: Option<String>,
    pub os_version: String,
    /// 今ある権限（TCC の名前・scope）
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// Engine の誤り。Display は画面に出せる 1 行（理由と次の一手・D-e）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// どの手段も無い・使えない（調べた手段の 1 行ずつ）。AX で届かない事だけを理由にしない（API・SDK・CLI・plugin・DOM・source を見た後に出す）。
    /// ENGINE-AUDIT W5: `searched` = 探した跡（何を探し・何が見えたか・次の一手 = `failure_line` の 1 行の後ろ・探していなければ `Searched::default()`）
    NoMeans { operation: String, tried: Vec<String>, searched: Searched },
    /// 人が操作中（lease）・同意・許可・送る直前の宛先の読み直し（WF-006）が無い。止めた手とその後ろは送っていない。前に走った手は手元を読む手だけ、
    /// ただし WF-006 の Unchecked で前の操作の外へ効く手が走った時は、`why` が走った手の数を言う（WF-E x1・nothing was sent と言わない）。
    /// 承認の要る手で止まった時は `Denial::pending` に、その手の承認に要る物（受け渡した後の exact input と digest）
    Denied(Denial),
    /// model が答えない・答えが決まりに合わない
    Model(String),
    /// 生成した物が隔離した試験に通らない
    Generation(String),
    /// 実状態の読み戻しが成功の条件に合わない（成功と出さない）
    Verification(String),
    /// G74: 送る手（書く手の後ろの書く手）の直前に、前の書く手が置いた物を確かめられず、送る手を走らせなかった（`WorkflowError::Held`）。
    /// 置く手は走った・送る手とその後ろは送っていない
    NotSent(String),
    /// G74: 書く手を送った後に、結果を確かめられなかった（送れたかが分からない）。済みと出さず、自動で送り直さない
    Unconfirmed(String),
    /// G74c（owner 10-26 14:4x の 3 つ目の文）: 手順の途中の手 `step` が送る前に止められた（`WorkflowError::Rejected`）が、その前の手（読むだけでない手）は
    /// 走った。`step` とその後ろは送っていない（消す・送る手は打っていない）・前の `ran` 手は走った。`why` = 止めた Adapter の 1 行（何が見えて止めたか）
    Stopped { step: String, why: String, ran: usize },
    /// 利用者の data（capabilities / extensions）を読めない・書けない
    Store(String),
    /// WF0: 手順の途中の Decide の手で人の判断を待つ（前の手は走った・その手と後ろは送っていない・run は runs 表に `waiting_human` で残る）
    /// Wave: WF0
    Decide(Decision),
    /// WF0: 止まっている間に元データ（読む手の出力・`Workflow::checks` の値）か保存した手順が変わった・読めないので、古い判断・承認で 1 手も送らなかった。
    /// `why` = 理由の 1 行・`asked` = 出し直す card（run は待ったまま・もう一度押すと今の元データで続ける）
    /// Wave: WF0
    Changed { why: String, asked: Asked },
}

/// WF0: 人を待っている run が出す card（runs 表の Engine の run の記録に残し、再起動の後も同じ物を出す）。
/// Wave: WF0
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Asked {
    /// 送る手の承認（`Denial::pending` と `later`）
    Approve { steps: Vec<PendingApproval> },
    Decide(Decision),
}

/// WF0: 再起動の後に人を待っている Engine の run（`waiting_runs`）。`space_id` = Project の id。
/// Wave: WF0
#[derive(Debug, Clone, PartialEq)]
pub struct PausedRun {
    pub space_id: String,
    pub run_id: String,
    pub goal: String,
    pub asked: Asked,
}

/// WF1: Engine の run を残す runs 表（`EngineContext::runs`）と、新しい run の行の親（Project の app の session・版・space = Project の id）。
/// run の id = `Caller::run_id`・app の id = `Caller::app_id`。
/// Wave: WF1
pub struct RunsTable<'a> {
    pub store: &'a std::sync::Mutex<crate::storage::Store>,
    pub session_id: &'a str,
    pub space_id: &'a str,
    pub app_version: u32,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMeans { operation, tried, .. } => write!(f, "No steps came back for “{operation}” (looked at: {}). Do it in the app yourself", tried.join(", ")),
            Self::Denied(why) => write!(f, "Didn't act ({why})"),
            Self::Model(why) => write!(f, "Couldn't use the AI's answer ({why}). Reword your request and ask again"),
            Self::Generation(why) => write!(f, "Couldn't build the missing ability ({why}). Do it in the app yourself"),
            Self::Verification(why) => write!(f, "Couldn't check the result in the app ({why}). Not marked done"),
            Self::NotSent(why) => write!(f, "Shibubu didn't do the send step ({why})"),
            Self::Unconfirmed(why) => write!(f, "Couldn't confirm whether it was sent ({why}). It won't be sent again"),
            Self::Stopped { step, why, ran } => write!(f, "Stopped before the step “{step}” ({why}). It and the later steps didn't run; the {} before it ran", crate::domain::plural(*ran, "step")),
            Self::Store(why) => write!(f, "Can't read or write the saved ability ({why})"),
            Self::Decide(d) => write!(f, "Waiting for your choice: {}", d.question),
            Self::Changed { why, .. } => f.write_str(why),
        }
    }
}
impl std::error::Error for EngineError {}

/// `EngineError::Denied` の中身: 止めた理由（`Deref` で文字列として読める）と、承認待ちで止まった時の承認に要る物。
/// Wave: W19-2b
/// 例: `if let Err(EngineError::Denied(d)) = &r { if let Some(p) = &d.pending { /* p.input を利用者に見せ、承認なら payload_digest = p.digest の ConsentReceipt */ } }`
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Denial {
    pub why: String,
    /// 止めた手が承認の要る手の時だけ（承認だけで通るとは限らない・人の lease などは `why`）
    pub pending: Option<PendingApproval>,
    /// W19-2 の配線（図 §33）: `pending` の手より後ろで承認が要り、input が今決まる手（`$from` が走った手だけを指す）。
    /// 画面は `pending` と一緒に 1 行で見せ、1 回の承認で全部の `ConsentReceipt` を作って同じ一言を run し直す（手ごとに止めない・26 通目）。
    /// 保証しない: 書く手の出力を受け取る後ろの手（走らせるまで input が決まらない）はここに入らず、run し直した時にそこで止まる（`Verification`）。
    pub later: Vec<PendingApproval>,
    /// ENGINE-AUDIT W5: 探した跡（探し尽くして止めた時・`failure_line` が「何を探し・何が見えたか・次の一手」に）。探していなければ空（`Searched::default()`）
    pub searched: Searched,
    /// r10 ②（P0「送っていない / 送ったが続けられなかった」を分ける）: 止めるまでにこの run で走った手の数（書く手が 1 つでも走った時だけ・0 = 何も送っていない）。
    /// `failure_line` は 0 の時だけ「Nothing was done (nothing was sent)」と言い、1 以上なら「N steps ran before Shibubu stopped」。今は探す段の止め（`execute` の Unfound の腕）が入れる
    /// interface に足した理由（1 行）: 探す段の止めは探した跡（`searched`）を持つ `Denied` のまま走った数を言う = `Stopped` に替えると探した跡が落ちる
    pub ran: usize,
}

/// 承認待ちの 1 手。`ConsentReceipt` の action = `action`・`payload_digest` = `digest` で、この input の 1 回だけ通る。
/// 受け渡した値は走らせた時にしか決まらないので、保存した手順の input（`$from`）ではなく Engine が埋めた後の物を返す。記録（`EngineRecord`）には残さない。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingApproval {
    pub step: String,
    pub action: String,
    /// 受け渡した後の exact input（利用者に見せる値）
    pub input: Value,
    /// `sdk::payload_digest(action, input)`
    pub digest: String,
    /// W19-2 の配線: action の required_scopes（承認の `ConsentReceipt::scopes` に入れる・registry の gate が照合する）
    pub scopes: Vec<String>,
}

impl std::ops::Deref for Denial {
    type Target = str;
    fn deref(&self) -> &str {
        &self.why
    }
}

/// ENGINE-AUDIT W5（節 8「止まった時の次の一手」・TALK-HONEST 232）: 探した跡 = 止まった時の 1 行の中身。`engine_page::failure_line` が「何を探し・何が見えたか・次の一手」を出し、
/// ATTACH が `next` を button にする（押せば同じ Project の run）。`NoMeans`（AI が手順 0）と `Denied`（探し尽くした）の両方が持つ・`RunPlan::searched` に積む。
/// 新しい型の理由: 探した語・近い名前・次の一手を一緒に持つ型が Core に無い（`ReadMiss` は UI の read だけ・`Denial` は理由と承認・`Decision` は人の選択）。
/// 例: `Denial { why, searched: Searched { words: vec!["guitar".into()], seen: vec!["AXButton: Library".into()], named: 212, next: Some("Open the Library in GarageBand".into()) }, ..Default::default() }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Searched {
    /// 探した語（`gui::words_of` = 頼みの言葉・押す名前の語）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// 近い名前（`role: 名前`・多くて 3 つ・語の一部が当たる物、無ければ押せる物の先頭）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seen: Vec<String>,
    /// 名前の付いた要素の数（窓の全部・0 = 読めなかった）
    #[serde(default)]
    pub named: usize,
    /// 次の一手 = 人の言葉の 1 行（その app で何をすれば名前が出るか・AI の none_why から・ATTACH が行や button にする）。無ければ None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// W5 本体で足した: 探す段で足して走らせた手（「desktop.ax.click Guitar」の 1 行ずつ・AI に「同じ手を繰り返さない」と渡す・止まった 1 行の「試した事」）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tried: Vec<String>,
}
impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.why)
    }
}
impl From<String> for Denial {
    fn from(why: String) -> Self {
        Self { why, pending: None, later: Vec::new(), searched: Searched::default(), ran: 0 }
    }
}
impl From<&str> for Denial {
    fn from(why: &str) -> Self {
        why.to_string().into()
    }
}

/// Engine が 1 回の run で使う Core の部品。
/// Wave: W19-2
/// 例: `let mut cx = EngineContext { registry: &mut reg, data_dir: &dir, llm: generated.llm(), mcp: &mcp, env, analysis: capability_engine::analysis_backends(&mcp).as_mut_slice(),
///       caller: Caller { app_id: &project_id, run_id: Some(&run_id) }, consents: &shown, readback: READBACK_WAIT, target: req.target.as_deref(), document: req.document.as_deref(),
///       document_file: req.document_file.as_deref(), repo: req.repo.as_deref(), progress: Some(&*req.progress), trace: std::env::var(QA_ENV).is_ok_and(|v| v == "1"),
///       situation: &req.situation, stop: &req.stop, staged: Some(&push) };`
/// 呼ぶ前に `registry.add_recipes(&dir.join(EXTENSIONS_DIR), …)`（前に作った Adapter）と `registry.check()`（公式の Adapter の readiness）を 1 回。
pub struct EngineContext<'a> {
    pub registry: &'a mut AdapterRegistry,
    /// 実行を頼んだ側（承認の記録の app_id / run_id と照合する・`run_workflow` と同じ）
    pub caller: Caller<'a>,
    /// 利用者がこの run で見て決めた承認（exact payload・1 回の承認で 1 回）。承認の要る action は、合う物が在る時だけ registry が通す（無ければ Rejected → `Denied`）
    pub consents: &'a [ConsentReceipt],
    /// 利用者の data dir（`capabilities/`・`extensions/`・`workflows/`・`trials/` の親）
    pub data_dir: &'a Path,
    /// 利用者の LLM の CLI（None = model 無しで、既存の能力の発見・同じ目的の保存した手順の再利用・確かめ済みの能力が 1 つだけの時の入力なしの 1 手〈読み戻しが無いので済みとは出さない〉まで）
    pub llm: Option<&'a dyn Llm>,
    /// Engine が起こしてよい MCP の server（`mcp::load_engine_config`: 利用者が Shibubu に入れた物だけ・他の app の設定の server は起こさない）
    pub mcp: &'a McpConfig,
    pub env: EnvFacts,
    /// W19-3: Deep Software Analysis に使える Backend（空 = 解析しない・REA は入っていない Mac が普通で、無くても他の段は同じに動く）
    pub analysis: &'a mut [Box<dyn AnalysisBackend>],
    /// W19-2 の配線（W19a2 の実機）: Execution の後の読み戻しが条件に合うまで読み直す上限（`READBACK_STEP` おき・0 = 1 回だけ読む）。製品は `READBACK_WAIT`。
    /// 背景の app の保存は disk に遅れて届く（W17-2 の実測 2〜33 秒・W19a2 の実機で約 30 秒）ので、1 回目の読みで「確かめられない」と言わない
    pub readback: Duration,
    /// M2 G-a: 利用者が今見ている Page に置いた app の bundle id（`generated::EngineRequest::target`）。Goal Interpretation の全部の操作の target が null
    /// （一言が app を名指さない・model 無しの 1 操作）の時だけ、全部の操作の宛先にする（1 つでも宛先を決めた操作が在れば触らない = 受け渡しの操作を Page の app に縛らない）
    pub target: Option<&'a str>,
    /// G73: `target` と同じ Page の、その app の面の書類の file 名（`generated::EngineRequest::document`）。Dynamic Generation の前の宛先の窓の読み
    /// （`desktop.windows.list` の `main`）に渡し、Adapter は窓の役割ではなく書類で窓を決める（役割で選ぶのは書類の無い app = REAPER・Zoom・Discord だけ）
    pub document: Option<&'a str>,
    /// G81: `document` の書類の disk の場所（`generated::EngineRequest::document_file`・`file://` + 絶対 path）。在れば、名指しの書類に字を置く手の済みを
    /// 窓（G75）に加えてこの file の中身（registry の `git.file.read`）でも確かめる = 保存が disk に着くまで「済み」と出さない（r25 #20: 窓だけで ≈ 17 秒早く済み）
    pub document_file: Option<&'a str>,
    /// MIRROR-PIN-1102: `generated::EngineRequest::window`（今の Page の Mirror に映っている `target` の窓 = (pid, CG の窓番号)・決められない時は Err の 1 行）。
    /// 宛先が `target` と同じ操作にだけ効く: Ok = その窓だけを読み・押す（無ければ代用せず Denied）・Err = 窓も読まず何も送らずに Denied
    pub window: Option<&'a Result<(u32, u32), String>>,
    /// B74（録画の手 7）: Project の repo の場所（`generated::EngineRequest::repo`・`file://` + 絶対 path・repo が無ければ None）。model は disk の場所を知らない（FX-111）ので、
    /// 自分で足す書類の読み（`SAVED_READ`）の uri を repo からの相対（`README.md`）で書く → Engine がこの下の絶対に直す（`in_repo`・repo の外 = `..` は直さない）。
    /// B76: model の読みはこの下だけ（外を指せば 1 つも読まず `OUTSIDE_REPO` で止まる・None なら Engine 自身の書類の読みの他は全部外）
    pub repo: Option<&'a str>,
    /// G81: Execution の後の読み戻しを待つ間の Page の 1 行（`generated::EngineRequest::progress` と同じ口・None = 出さない）。
    /// 窓の読みは合ったが保存した file がまだ合わない間だけ `CHECKING_SAVED` を出す
    pub progress: Option<&'a (dyn Fn(Option<String>) + Send + Sync)>,
    /// G76: 段ごとの trace を `<data_dir>/engine-trace.jsonl`（`TRACE_FILE`）に書く（QA の run だけ・製品は env `QA_ENV` = "1" の時だけ true・既定は書かない）
    pub trace: bool,
    /// WF1: run を残す runs 表（Execution の段の前に 1 行・手ごとに write-ahead・承認と判断で止まった所から `resume`）。None = 残さない
    /// （止まった run は今まで通り同じ一言を最初から run し直す）
    pub runs: Option<RunsTable<'a>>,
    /// RQ-023: Automation の 3 択（`AutomationMode`・Execution の前の承認の門が読む・既定 Ask before write）
    pub automation: crate::domain::permission::AutomationMode,
    /// ENGINE-GENERIC-1101（owner 211）: 一言と一緒に渡された今の状況（`domain::chat::Situation`・`generated::EngineRequest::situation`）。部品は app も
    /// 決まった機能も知らずに一言と状況だけを渡し、Engine がその場で app の口を読んで手を組む。状況の無い呼び手は `&NO_SITUATION`。
    /// interpret と generate の AI は事実の行と file の名前を読み（`Situation::text` = chat と同じ塊・generate の input の `situation`）、手の input の file は
    /// `{"$from": "situation.files.<n>"}` を Execution が埋める（`situation_payload`・保存した手順にも path を書かない = 次の take でも同じ手順）。
    pub situation: &'a Situation,
    /// ENGINE-ADJUST-1102 ①（owner 256・operator「Engine の run に人の Stop が無い」）: 人の Stop の印。runtime の `running` の flag（`Runtime::track`・Runs の Stop が
    /// 立てる）と Running の card の Stop（`Generated::stop_engine`）が同じ `AtomicBool` を立て、`generated::EngineRequest::stop` でここへ来る。Engine は段の間・
    /// AI の前後（`complete_json_cancellable`）・手の前（`run_steps`）・読み戻しの待ち・Adapter の読みの続き（`AdapterRegistry::set_stop` → bridge の more の前）で見て、
    /// 立っていれば副作用の手を打たず、読みかけを使わず、1 手も送っていなければ `Denied`（`STOPPED_LINE`）・手が走った後なら `Stopped`（`STOPPED_WHY`）で閉じる。
    /// 印を持たない呼び手（試験・`run_workflow`）は `&NO_STOP`。
    pub stop: &'a AtomicBool,
    /// ENGINE-AUDIT W1（節 16「記録に段の時間と AI が見た物が無い」）: 段が終わるたびに呼ぶ口（`progress` と同じ形）。止まった run でも段の記録が残る
    /// （`EngineError` は段を持たないので、`CoreEngine::run` がここで集めて `engine-runs.jsonl` の error の行に `stages` を書く）。None = 集めない（試験・`run_workflow`）
    pub staged: Option<&'a (dyn Fn(&StageLog) + Send + Sync)>,
    /// r7（operator 11-03 ②）: この run で generate が AI を呼んだ数（上限 `recipe::GENERATE_CALLS_MAX` = 2。`run_stages` / `plan` の頭で 0・文脈の手の後の
    /// generate のし直しでも 0 に戻す = 尽きたら手順 0 として止める）。dogfood r7 は 2 操作で 3 回 150 秒
    pub generate_calls: u8,
}

/// ENGINE-GENERIC-1101: 状況の無い run の `EngineContext::situation`（試験・状況を集めない呼び手）。Vec を持つので const の参照は 'static にならず static。
pub static NO_SITUATION: Situation = Situation { facts: Vec::new(), files: Vec::new() };

/// ENGINE-ADJUST-1102 ①: 人の Stop を持たない呼び手の `EngineContext::stop`（立つ事が無い）。
pub static NO_STOP: AtomicBool = AtomicBool::new(false);
/// ENGINE-ADJUST-1102 ①: 人の Stop で 1 手も送らずに閉じた時の 1 行（`EngineError::Denied`・Page は Error でなく理由の文・Runs は Cancelled）。
pub const STOPPED_LINE: &str = "Stopped by you · nothing more was sent";
/// ENGINE-ADJUST-1102 ①: 手が走った後の人の Stop の理由（`EngineError::Stopped::why`・文は「Stopped before the step … (stopped by you). It and the later steps didn't run; the N before it ran」）。
pub const STOPPED_WHY: &str = "stopped by you";
/// ENGINE-ADJUST-1102 ①: 書いた後の読み戻しの最中の人の Stop（`EngineError::Unconfirmed`・送り直さない・済みと出さない・row は Uncertain）。
pub const STOPPED_UNCHECKED: &str = "Stopped by you before the result was checked · it won't be sent again";

/// ENGINE-ADJUST-1102 ①: 人の Stop の印（`EngineContext::stop`）が立っていれば、1 手も送らない Stop の誤り（`STOPPED_LINE`・承認待ちの手は無い = card を出さない）。
/// 段の間（`run_stages`）・AI の前（`generate` の facts の後と作り直しの前）・承認の続きの前（`resume`）で呼ぶ。
fn check_stop(cx: &EngineContext) -> Result<(), EngineError> {
    if cx.stop.load(Ordering::SeqCst) {
        return Err(stopped());
    }
    Ok(())
}

/// ENGINE-AUDIT W1（節 6「待つ間に何をしているか見えない」）: 段の 1 行を Page（Running の card の下・部品の Working の行）に出す。`cx.progress` が無ければ何もしない。
/// 例: `say(cx, "Asking the AI what you meant… (up to 2 min)")`
fn say(cx: &EngineContext, line: impl Into<String>) {
    if let Some(p) = cx.progress {
        p(Some(line.into()));
    }
}

/// ENGINE-AUDIT W1: 段の数字を足し合わせる（`Generated::counts` を操作ごとに DynamicGeneration の counts へ・数の key は足し、配列の key は繋ぐ・他は見ない）。
fn add_counts(total: &mut Value, part: &Value) {
    let (Some(t), Some(p)) = (total.as_object_mut(), part.as_object()) else { return };
    for (k, v) in p {
        if let Some(n) = v.as_u64() {
            let was = t.get(k).and_then(Value::as_u64).unwrap_or(0);
            t.insert(k.clone(), json!(was + n));
        } else if let Some(more) = v.as_array() {
            // r9 (3): 配列（ms_ai_each・retry_why）は操作の順に繋ぐ
            let mut was = t.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
            was.extend(more.iter().cloned());
            t.insert(k.clone(), Value::Array(was));
        }
    }
}

/// ENGINE-AUDIT W2: Core が名指す「押す」手の action の id（Adapter の `mac_gui` の spec と同じ字・`OPEN_DOCUMENT_ACTION` と同じ理由で文字列）。
/// 再利用の前の名前の照合（`unseen_label`）と、失敗の hint の「押した名前」に使う。
pub const CLICK_ACTION: &str = "desktop.ax.click";
/// ENGINE-AUDIT W7（spec 261・operator 11-03 の決め 1〜3）: 承認の要らない「押す」手（`mac_gui` の spec `desktop.ax.reveal`・input は click と同じ・Effect は ExternalWrite のまま =
/// read-only の automation では走らない）。どちらにするかは AI でなく Core が決める（`recipe::reveal_or_click` = `gui::asks` に当たらない押す手は reveal・当たれば click = 毎回 card・
/// Always allow でも飛ばさない）。送る直前に Adapter が解決した要素で `gui::asks` に当たれば押さずに `Failed { ApprovalDenied }` → `run_steps` が `WorkflowError::Asks` →
/// `execute` がその手を click に書き換えて card。`CLICK_ACTION` を見る所（押した名前・unseen_label・探す段）は両方を見る（`presses`）
pub const REVEAL_ACTION: &str = "desktop.ax.reveal";
/// W7: 押す手か（click か reveal）。例: `w.steps.iter().filter(|s| presses(&s.capability))`
pub(crate) fn presses(capability: &str) -> bool {
    capability == CLICK_ACTION || capability == REVEAL_ACTION
}
/// ENGINE-AUDIT W3: Core が名指す「menu bar を読む」手の action の id（`mac_gui` の spec `desktop.menu.read` と同じ字）。generate の facts が当たり 0 の時に読み、AI の input の `menu_bar` に
pub const MENU_READ_ACTION: &str = "desktop.menu.read";
/// ENGINE-AUDIT W4: Core が名指す「app の口の棚を読む」手の action の id（`mac_gui` の spec `desktop.app.surfaces`）。discover の頭（`surface_records`）と generate の facts（input の `app_surfaces`）と
/// `CoreEngine::read`（AX の前）が読む
pub const SURFACES_ACTION: &str = "desktop.app.surfaces";

/// ENGINE-AUDIT W4（節 4）: 口の棚（`gui::AppSurfaces`）を Capability Discovery の候補にする（`Reuse::AppSpecific`・`Origin::Official`・`Verification::Unverified` = 一覧に在るだけ・
/// generate の model が読んで recipe を組む・選ぶのは Verification を通った物だけ）。1 件 = 口 1 つ: `app.<bundle>.url`（URL scheme・evidence に scheme）・`app.<bundle>.documents`（書類の型）・
/// `app.<bundle>.dictionary`（辞書の suite・evidence に suite の名前）・`app.<bundle>.data.<ext>`（同梱の data・evidence に数）。何も無ければ空。
/// 例: `p.found.extend(surface_records(&s))`
pub fn surface_records(s: &crate::gui::AppSurfaces) -> Vec<CapabilityRecord> {
    let app = s.bundle_id.clone();
    let rec = |id: String, kind: AdapterKind, evidence: Vec<String>| CapabilityRecord {
        reuse: Reuse::AppSpecific { app: app.clone() },
        evidence,
        constraints: vec![SURFACE_NOTE.into()],
        ..base_record(id, kind)
    };
    let head = format!("app.{}", slug(&s.bundle_id));
    let mut out = Vec::new();
    if !s.url_schemes.is_empty() {
        out.push(rec(format!("{head}.url"), AdapterKind::Api, s.url_schemes.iter().map(|u| format!("URL scheme {u}:// (open with the `open` CLI: open \"{u}://…\")")).collect()));
    }
    if !s.document_types.is_empty() {
        out.push(rec(format!("{head}.documents"), AdapterKind::LocalCore, vec![format!("Opens documents: {}", s.document_types.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" "))]));
    }
    if !s.dictionary.is_empty() {
        out.push(rec(format!("{head}.dictionary"), AdapterKind::Api, vec![format!("AppleScript dictionary suites: {} (read or change with an osascript recipe)", s.dictionary.join(" · "))]));
    }
    for g in &s.resources {
        out.push(rec(format!("{head}.data.{}", slug(&g.ext)), AdapterKind::LocalCore, vec![format!("{} .{} files under Contents/Resources (read-only; names are in app_surfaces)", g.count, g.ext)]));
    }
    out
}

/// W4: 口の棚から来た候補の印（constraints の 1 行・`choose` は Verified でないので選ばない・generate の model が読む）。
const SURFACE_NOTE: &str = "読むだけで分かった口（bundle の中身）。呼べるか・結果が元の物に残るかは未確認";

/// W4: 宛先の app の口の棚を読んで候補にする（`SURFACES_ACTION` が無い・読めない = 空・run は止めない）。
fn surface_records_of(cx: &mut EngineContext, app: &str) -> Vec<CapabilityRecord> {
    match cx.registry.invoke(cx.caller, SURFACES_ACTION, &json!({ "bundle_id": app }), None) {
        (Outcome::Verified { .. } | Outcome::Unverified { .. }, Some(obs)) => serde_json::from_value::<crate::gui::AppSurfaces>(obs.payload).map(|s| surface_records(&s)).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// ENGINE-AUDIT W2（「proven なら interpret を飛ばす」）: この一言の保存した手順が全部 proven（本物で読み戻しが通った・失敗 0）なら、その操作を返す（AI 0 回）。
/// 1 つでも下書き・失敗した物が在れば None（今まで通り interpret）。操作の id は手順の id の末尾（`workflow_id` と同じ付け方）、順は `split` が在ればその順。
/// 宛先は手順の最初の手の `bundle_id`（無ければ None = Page の app）。
fn proven_ops(goal: &str, data_dir: &Path) -> Option<Vec<Operation>> {
    let head = format!("wf-{}-", &crate::crypto::sha256_hex(goal.trim().as_bytes())[..12]);
    let saved = load_workflows(&data_dir.join(WORKFLOWS_DIR)).ok()?;
    let mine: Vec<&Workflow> = saved.iter().filter(|w| w.id.starts_with(&head) && w.goal.trim() == goal.trim()).collect();
    if mine.is_empty() || mine.iter().any(|w| w.proven.is_none() || w.fails > 0) {
        return None;
    }
    let op_of = |w: &Workflow| Operation {
        id: w.id[head.len()..].to_owned(),
        want: w.goal.clone(),
        success: String::new(),
        target: w.steps.iter().find_map(|s| s.input["bundle_id"].as_str().map(str::to_owned)),
        ..Default::default()
    };
    let split = mine.iter().find(|w| !w.split.is_empty()).map(|w| w.split.clone()).unwrap_or_default();
    let mut ops: Vec<Operation> = Vec::new();
    for id in &split {
        if let Some(w) = mine.iter().find(|w| w.id == format!("{head}{}", slug(id))) {
            ops.push(op_of(w));
        }
    }
    for w in &mine {
        if !ops.iter().any(|o| o.id.as_str() == &w.id[head.len()..]) {
            ops.push(op_of(w));
        }
    }
    (!ops.is_empty() && ops.len() <= MAX_OPS).then_some(ops)
}

/// ENGINE-AUDIT W2: run の終わり方が「手順が悪かった」と言える物か（`Workflow::fails` を足す・次は作り直す）。承認待ち・判断・元データの変化・人の Stop・
/// 送れたか分からない（Unconfirmed）・AI と保存の失敗は数えない。
/// 11-03（DEMO-FREEZE の読み）: 人の側で止めた拒否（AI に任せる同意が無い `gui::ACT_CONSENT_MARK`・人が前面に居る `gui::HUMAN_LEASE_LINE`）も数えない。
/// 承認の要る手で止まれば `pending` が在って元から数えないが、承認の要らない手（W7 の reveal）で止まると `pending` が無く、同意や lease の後の run が
/// 正しい保存の手順を捨てて「Make different steps」（`failed_before`）で作り直していた。
fn counts_as_failure(e: &EngineError) -> bool {
    if is_stop(e) {
        return false;
    }
    match e {
        EngineError::Verification(_) | EngineError::NotSent(_) | EngineError::Generation(_) | EngineError::NoMeans { .. } | EngineError::Stopped { .. } => true,
        EngineError::Denied(d) => d.pending.is_none() && ![crate::gui::ACT_CONSENT_MARK, crate::gui::HUMAN_LEASE_LINE].iter().any(|m| d.why.contains(m)),
        _ => false,
    }
}

/// ENGINE-AUDIT W2: 保存した手順の file を 1 つ読み替えて書く（無い・読めない = 何もしない・`alone` の手順は file を持たない）。
fn amend_workflow(data_dir: &Path, id: &str, change: impl FnOnce(&mut Workflow)) {
    let dir = data_dir.join(WORKFLOWS_DIR);
    let Ok(all) = load_workflows(&dir) else { return };
    if let Some(mut w) = all.into_iter().find(|w| w.id == id) {
        change(&mut w);
        let _ = save_workflow(&dir, &w);
    }
}

/// ENGINE-AUDIT W2: 失敗の hint（generate の `rejected_plan` の 1 行・前に保存した手順が押した名前つき）。
fn failed_before(w: &Workflow) -> String {
    let pressed: Vec<&str> = w.steps.iter().filter(|s| presses(&s.capability)).filter_map(|s| s.input["label"].as_str()).collect();
    let what = if pressed.is_empty() { String::new() } else { format!(" (they pressed: {})", pressed.join(", ")) };
    format!("The steps saved for this request before failed {}{what}. Make different steps: first open or select what is needed, and press only names in window_controls", crate::domain::plural(w.fails as usize, "time"))
}

/// ENGINE-ADJUST-1102 ①: 1 手も送らずに閉じる Stop の誤り。
fn stopped() -> EngineError {
    EngineError::Denied(Denial::from(STOPPED_LINE))
}

/// ENGINE-ADJUST-1102 ①: AI の呼び出しの誤りを Engine の誤りに（人の Stop で止めた = `stopped`・それ以外は今まで通り `Model` = AI 無しの道へ）。
fn ai_error(e: LlmError) -> EngineError {
    match e {
        LlmError::Cancelled { .. } => stopped(),
        e => EngineError::Model(e.to_string()),
    }
}

/// ENGINE-ADJUST-1102 ①: 人の Stop で閉じた誤りか（`close_row` が row を Cancelled にする・card の Mark::Stopped の元）。
/// 例: `if is_stop(&e) { /* Cancelled */ }`
pub fn is_stop(e: &EngineError) -> bool {
    match e {
        EngineError::Denied(d) => d.why == STOPPED_LINE,
        EngineError::Stopped { why, .. } => why == STOPPED_WHY,
        EngineError::Unconfirmed(w) => w == STOPPED_UNCHECKED,
        _ => false,
    }
}

/// ENGINE-GENERIC-1101: 手の `{"$from": "situation.files.<n>"}`（絶対 path）・`situation.urls.<n>`（`file://` の URL）が指す値（Execution の前に `outputs` に入れる・
/// 手の id には使えない = `workflow::validate`）。例: `outputs.insert(SITUATION.into(), situation_payload(cx.situation))`
fn situation_payload(s: &Situation) -> Value {
    json!({
        "files": s.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
        "urls": s.files.iter().map(|f| format!("file://{}", f.display())).collect::<Vec<_>>(),
    })
}

/// ENGINE-GENERIC-1101: AI に見せる状況（interpret は一言の後ろの行・generate は input の `situation`）。file は名前と `$from` の名前だけ
/// （FX-111 = home の path を AI に渡さない）。空 = None（AI の input は今までと同じ字）。
fn situation_files(s: &Situation) -> Vec<Value> {
    let name = |f: &Path| f.file_name().map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
    s.files.iter().enumerate().map(|(i, f)| json!({ "name": name(f), "path": { "$from": format!("{SITUATION}.files.{i}") }, "url": { "$from": format!("{SITUATION}.urls.{i}") } })).collect()
}

/// ENGINE-GENERIC-1101（operator の決め 2「開いている app と窓」）: 今 窓を開いている app（名前・bundle id・窓の数）を状況の 1 行に（`desktop.windows.list` を 1 回・
/// 窓の題は渡さない = 他の app の書類の名前や件名を AI に出さない）。action が無い・Ready でない・読めない・0 個 → None（run は止めない）。
/// 追記 234: 呼ぶのは `CoreEngine::windows` だけ（chat の thread と Engine の thread が `generated::with_windows` で同じ 1 行を足す）。
/// 例: `situation.facts.extend(open_windows(&mut reg, caller))` → `Windows open now: Mixer (com.example.mixer) × 1`
pub fn open_windows(registry: &mut AdapterRegistry, caller: Caller) -> Option<String> {
    const LIST: &str = "desktop.windows.list";
    if !registry.actions().iter().any(|a| a.id == LIST && a.readiness == crate::domain::action::ActionReadiness::Ready) {
        return None;
    }
    let (Outcome::Verified { .. } | Outcome::Unverified { .. }, Some(obs)) = registry.invoke(caller, LIST, &json!({}), None) else { return None };
    let mut apps: Vec<(String, String, usize)> = Vec::new();
    for w in serde_json::from_value::<Vec<crate::gui::WindowRef>>(obs.payload).ok()? {
        match apps.iter_mut().find(|(_, b, _)| *b == w.bundle_id) {
            Some(a) => a.2 += 1,
            None => apps.push((w.app_name, w.bundle_id, 1)),
        }
    }
    (!apps.is_empty()).then(|| format!("Windows open now: {}", apps.iter().map(|(n, b, k)| format!("{n} ({b}) × {k}")).collect::<Vec<_>>().join(" · ")))
}

/// G76: 段ごとの trace の file（`EngineContext::data_dir` の下・1 行 = 1 段: {run_id, stage, result: ok | stop, stop_reason?, ms, …段の中身}）。
/// 段 = ask → interpret → target → plan → validate → approval（run の最後の行）。題名・本文・home の path・鍵らしい物は書かない（`scrub`）。
/// Wave: M2
pub const TRACE_FILE: &str = "engine-trace.jsonl";
/// G76: この env が "1" の app だけ trace を書く（QA の driver が app を起こす時に付ける）。
pub const QA_ENV: &str = "SHIBUBU_QA_LLM";
/// G76: trace の plan 行に置く model の生の答えの上限（byte・字の途中で切らない）。構造の field は切らない
const TRACE_CAP: usize = 4096;

/// 製品の読み戻しの待ちの上限（`EngineContext::readback`）。背景の TextEdit の保存が disk に届くまで W17-2 で最長 33 秒・W19a2 の実機で 30〜41 秒
/// （45 秒の build で 41 秒 = 余り 4 秒だった）。合えば待たずに返るので、上限は余りを大きく取る。
/// Wave: W19-2
pub const READBACK_WAIT: Duration = Duration::from_secs(90);
/// 読み戻しを読み直す最初の間隔（読む action だけを読み直す・書く手はもう一度送らない）。W7 から 1 → 2 → 4 秒の backoff（4 倍で頭打ち）
const READBACK_STEP: Duration = Duration::from_secs(1);

/// 一言の操作の数の上限（Goal Interpretation の答えを確かめる）。
const MAX_OPS: usize = 8;
/// MCP の server の tools/list の待ちの上限（1 server ごと）。
const MCP_TIMEOUT: Duration = Duration::from_secs(10);
/// recipe の手段 1 回（CLI・MCP・HTTP）の待ちの上限。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// CLI の stdout を読む上限。
const OUT_CAP: u64 = 4 * 1024 * 1024;
/// `JsonRequest::name`（`llm-calls.jsonl` で何の呼び出しかを数える）。
const INTERPRET_NAME: &str = "engine_interpret";
const GENERATE_NAME: &str = "engine_generate";
/// 保存した能力の記録と今の環境が違う時に、発見した能力の constraints に付ける 1 行（この印の付いた能力は確かめ直すまで使わない）。
const REVERIFY: &str = "版・OS・権限が確かめた時と違うので、使う前に確かめ直す";

/// 目的 → 実行 → 記録（pipeline の全段）。
/// Wave: W19-2
/// 保証する: 段は `Stage::ALL` の順・registry に検証済みの能力と同じ目的の保存した手順が在れば生成しない（`reused`）・人の lease の間・承認の無い書き込みは
/// `Denied` で何も送らない（受け渡した値を外の service へ渡す手も、読む action でも承認が要る）・成功は実状態の読み戻しが条件に合った時だけ（`Verification`）・新しく確かめた能力だけを `save_record` で残す。
/// 段の中身: 足りない操作は `generate`（作った recipe は Unverified）→ Verification の段で本物の file を `trials/<操作>/` に写し、`trial` を `call_runner` で
/// その写しに流して `verify`（通った action だけ Verified）→ `AdapterRegistry::add_recipes` で registry に載せ、`save_recipe`・`save_workflow`（承認を待つ 2 回目も
/// 同じ手順を使える）→ 本物の宛先の `checks` を読む（操作の前の値）→ Execution は `run_workflow(&wf, cx.registry, cx.caller, cx.consents)`（registry の gate を通る
/// 唯一の道・止められた手より前と、前の操作で走った手が手元を読む手〈LocalRead〉だけなら `Denied`〈元の物は何も変わらず外へも送っていない・承認の要る手なら
/// `Denial::pending` に受け渡した後の exact input と digest〉・それ以外の手が走っていれば `Verification`）→ もう一度読んで、
/// 期待する値の `$from` を手の出力で埋めて `verify`（W19-2b: 受け渡した値を受け取った側の実状態で確かめる）→ `save_record`。
/// 人の lease は registry / Adapter の gate（W19-1 `gui::human_holds`）が Rejected で止め、Engine はそれを `Denied` に写すだけ（自分で前面や入力を見ない）。
/// MCP の server は run ごとに 1 回だけ起こして tools/list を読む（操作の数だけ起こさない）。保存した手順が版・OS・権限の違いで使えない時は、作り直す前に
/// 保存した試験（`workflows/trials/`）を写しで流し直して確かめ直す（model を呼ばない・通らなければその試験を捨てて Generation・次の run は作り直す）。
/// W19-3 の Deep Analysis の段（19 通目 §3 の 3「通常の探索で足りない場合」・§4）: 解析は通常の探索で足りない時だけ。対象（`op.target`）の決まった操作で、
/// 同じ目的の保存した手順が無く、解析の Backend が在る時、前に同じ Backend・対象・版を解析した結果（`analyses/`）が在れば読み、利用者の承認（action
/// `analysis.<backend>`・exact payload）が在れば解析する。承認がまだ無い操作は何もせず先へ進み、Dynamic Generation で生成の model が手段無し（`NoMeans`）と
/// 答えた時に初めて、Ready の道具が在れば何も解析せず `Denied`（`Denial::pending` に解析の承認）。model 無しの時と、手段が在る時は解析を求めない。
/// 結果の候補は Unverified で found に足し、生成の model が読む（選ぶのは Verification を通った物だけ）。解析の誤りは段の Failed の 1 行で、解析せずに既存の道で続ける。
/// 保証しない: 対象の app を前もって知っている事（app 専用の code は持たない・見本の app でも同じ道）。
/// G76: `cx.trace` の時だけ段ごとの 1 行を `TRACE_FILE` に（ask と、run の最後の approval の行はここ・interpret は `run_stages`・target / plan / validate は `generate`）。
pub fn run(goal: &str, cx: &mut EngineContext) -> Result<EngineRecord, EngineError> {
    run_with(goal, None, cx)
}

/// OPEN-INPLACE-1101 の 5: `run` と同じ。`planned` = 同じ一言の `plan` が分けた操作（Some なら interpret を呼ばない = chat の案に見せた手と同じ手・
/// 保存した手順を `usable` で拾うので生成も 0 回）。
/// 例（`CoreEngine::run`）: `engine::run_with(&req.goal, planned(&req), &mut cx)`
/// Wave: OPEN-INPLACE-1101
pub fn run_with(goal: &str, planned: Option<Vec<Operation>>, cx: &mut EngineContext) -> Result<EngineRecord, EngineError> {
    let t = Instant::now();
    trace(cx, "ask", t, None, json!({ "goal_chars": goal.trim().chars().count(), "page_target": cx.target, "page_document": cx.document }));
    // WF-036: AI が失敗した run は途中から AI を外す（`cx.llm = None`）ので、終わったら呼ぶ側の model に戻す
    let llm = cx.llm;
    let got = run_stages(goal, planned, cx);
    cx.llm = llm;
    let pending = match &got {
        Err(EngineError::Denied(d)) => usize::from(d.pending.is_some()) + d.later.len(),
        _ => 0,
    };
    trace(cx, "approval", t, got.as_ref().err(), json!({ "pending": pending }));
    got
}

/// WF2: 止まった run（`run_id`・runs 表の `waiting_approval` / `waiting_human` の行）を cursor の続きから走らせる。interpret を呼び直さず、
/// 済んだ手を走らせ直さない（WF-012）。続ける前に読む手を元の情報源から読み直し、`Workflow::checks` の値と保存した手順の digest を止まった時と比べる
/// （WF-002・WF-015）: Decide の時は読む手の出力も、どれか違えば 1 手も送らず `Changed`（card を出し直す・run は待ったまま）。承認の時の読む手の出力の違いは
/// registry の exact payload の照合が拒む（今まで通り承認の card を出し直す）。`choice` = Decide で選んだ choice の id（承認の時は None で、承認は `cx.consents`）。
/// Wave: WF2
/// 例: `capability_engine::resume(goal, &run_id, Some("b"), &mut cx)`（`cx.runs` = 同じ runs 表・`cx.caller.run_id` = `run_id`）
/// 保証する: Decide を選んでも承認・同意の receipt を作らない・選ばなかった choice の値はどの手にも渡らない・止まった行が無い（Engine の段の前の承認＝解析の承認・
/// 前の版の run）時は、承認なら同じ一言を `run` し直す（今まで通り）・選んだ答えなら 1 手も送らずに理由。
/// 同じ run に resume が 2 本同時に来ても（2 つの窓・二度押し・別の process）、待っている行を Running にできた 1 本だけが送る（INTEG wa1）。
/// 保証しない: 書いた手の結果が今も残っている事（済んだ手は送り直さず、最後の `checks` の読み戻しで確かめる）。
pub fn resume(goal: &str, run_id: &str, choice: Option<&str>, cx: &mut EngineContext) -> Result<EngineRecord, EngineError> {
    use crate::domain::run::RunStatus;
    let t = Instant::now();
    let gone = || EngineError::Denied(Denial::from("This is no longer waiting, so nothing was sent. Ask again"));
    let Some(store) = cx.runs.as_ref().map(|r| r.store) else { return if choice.is_some() { Err(gone()) } else { run(goal, cx) } };
    let found = crate::runtime::receipt::load(&store.lock().unwrap_or_else(|e| e.into_inner()), run_id).ok().flatten();
    let Some(rec) = found else {
        // 止まった行が無い（Engine の段の前の承認 = 解析の承認・行を残さなかった run）: 承認なら同じ一言を run し直す（今まで通り）・選んだ答えなら送らない
        return if choice.is_some() { Err(gone()) } else { run(goal, cx) };
    };
    let want = if choice.is_some() { RunStatus::WaitingHuman } else { RunStatus::WaitingApproval };
    let (Some(mut cursor), true) = (rec.workflow.clone(), rec.run.status == want) else { return Err(gone()) };
    let loaded = cursor.clone();
    let mut run: EngineRun = serde_json::from_value(rec.input.clone()).map_err(|e| EngineError::Store(format!("Can't read the waiting run ({e})")))?;
    let mut row = Some(rec);
    // WF-002: 止まっている間に保存した手順が書き直された・作り直された → 古い手順の続きを走らせない
    if saved_digest(cx.data_dir, &run.plans) != cursor.digest {
        hold(&run, &cursor, &mut row, Some(store), Some(RunStatus::Cancelled), None)?;
        return Err(EngineError::Denied(Denial::from("The saved procedure changed while this was waiting, so nothing was sent. Ask again")));
    }
    let i = cursor.plan;
    let Some(wf) = run.plans.get(i).map(|p| p.workflow.clone()) else { return Err(gone()) };
    let step = cursor.waiting.clone().unwrap_or_default();
    let actions = cx.registry.actions();
    let reads = |s: &WorkflowStep| s.is_action() && actions.iter().any(|a| a.id == s.capability && is_read(a.effect));
    let unread = |why: String, asked: Option<Asked>| asked.map_or_else(gone, |asked| EngineError::Changed { why, asked });
    // WF-004: 読み直せる手（行には digest だけ）は元の情報源から読み直し、止まった時と比べる。後ろで書く手が済んだ読みの digest は、止まる時に
    // 読み直した物（`settled` = 自分の書き込みの後の元データ）なので、自分の書き込みを止まっている間の変化と読まない
    // ENGINE-GENERIC-1101: 状況の file（`situation.files.<n>`）は止まる前と同じ物（`Generated::situation` が同じ順で集め直す）
    let mut outputs: HashMap<String, Value> = HashMap::from([(SITUATION.to_owned(), situation_payload(cx.situation))]);
    let mut moved = false;
    for s in wf.steps.iter().take(cursor.next) {
        let kept = cursor.outputs.get(&s.id).cloned().unwrap_or(Value::Null);
        let Some(was) = kept.get("$digest").and_then(Value::as_str).filter(|_| reads(s)).map(str::to_owned) else {
            outputs.insert(s.id.clone(), kept);
            continue;
        };
        let input = resolve(&s.input, &outputs, &s.id, "", &mut Vec::new()).map_err(EngineError::Verification)?;
        match cx.registry.invoke(cx.caller, &s.capability, &input, None) {
            (Outcome::Verified { .. } | Outcome::Unverified { .. }, Some(o)) => {
                moved |= digest_of(&o.payload) != was;
                outputs.insert(s.id.clone(), o.payload);
            }
            (o, _) => return Err(unread(format!("Couldn't read the original again ({}), so nothing was sent. Try again", outcome_word(&o)), run.asked.clone())),
        }
    }
    // WF-015: 止まった時に見た元データ（checks の値）と今を比べる。違えば古い判断・承認で 1 手も送らず、今の元データの card を出し直す（待ったまま）
    let now = read_checks(&wf.checks, cx).map_err(|e| {
        let why = if let EngineError::Verification(w) = e { w } else { e.to_string() };
        unread(format!("Couldn't read the original again ({why}), so nothing was sent. Try again"), run.asked.clone())
    })?;
    let seen = seen_of(&wf.checks, &now);
    if moved || seen != cursor.seen {
        let (fresh, why) = match &run.asked {
            Some(Asked::Decide(_)) => (Asked::Decide(decision(&wf, &step, &outputs).map_err(EngineError::Generation)?), "Changed since last checked · Review the choices again. The original changed while this was waiting, so nothing was sent"),
            _ => {
                let first = pending(&wf, &step, &outputs, &actions);
                let steps = first.into_iter().chain(later(&wf, &step, &outputs, &actions)).collect();
                // RQ-027（UIUX-N02）: 書く前に元データの digest を照合した結果の語は `Changed since last checked · Review`（照合の本体は WF-015・2 本目を作らない）
                (Asked::Approve { steps }, "Changed since last checked · Review the steps and approve again. The original changed while this was waiting, so the earlier approval wasn't used and nothing was sent")
            }
        };
        // 前に書いた手が無ければ、操作の前の読みも今に（Changed・Fewer は今の元データと比べる）
        if !wf.steps.iter().take(cursor.next).any(|s| s.is_action() && !reads(s)) {
            run.plans[i].before = now;
        }
        wait_at(&mut cursor, &wf, &step, &outputs, &actions, seen);
        run.asked = Some(fresh.clone());
        hold(&run, &cursor, &mut row, Some(store), None, None)?;
        return Err(EngineError::Changed { why: why.into(), asked: fresh });
    }
    match (&run.asked, choice) {
        (Some(Asked::Decide(_)), Some(id)) => {
            let d = decision(&wf, &step, &outputs).map_err(EngineError::Generation)?;
            let Some(c) = d.choices.iter().find(|c| c.id == id) else { return Err(EngineError::Denied(Denial::from("That isn't one of the choices, so nothing was sent. Choose again"))) };
            // 選んだ値だけを後ろの手へ（`<手>.value`）。選ばなかった値は outputs に入れない
            outputs.insert(step.clone(), json!({ "choice": c.id, "value": c.value }));
        }
        (Some(Asked::Approve { .. }), None) => {}
        _ => return Err(gone()),
    }
    // RQ-023: 待っている間に Automation が替わっていれば、その門で止める（何も送らない・待ったまま）
    automation_gate(&run, cx)?;
    // ENGINE-ADJUST-1102 ①: 承認・判断の続きを始める前に人の Stop が押されていれば、1 手も送らず行を Cancelled に（card は出し直さない・状態が替えられなければ待ったまま）
    if cx.stop.load(Ordering::SeqCst) {
        let _ = hold(&run, &cursor, &mut row, Some(store), Some(RunStatus::Cancelled), None);
        return Err(stopped());
    }
    (cursor.waiting, run.asked) = (None, None);
    // INTEG wa1: 行が読んだ時のまま待っている事の確かめと Running の書き込みを 1 つの BEGIN IMMEDIATE に（同じ run に resume が 2 本同時に来ても
    // 〈2 つの窓・別の process でも〉送るのは先に書いた 1 本だけ・後の 1 本は 1 手も送らずに理由）
    if !hold(&run, &cursor, &mut row, Some(store), Some(RunStatus::Running), Some((want, &loaded)))? {
        return Err(gone());
    }
    let got = execute(&mut run, &mut cursor, outputs, &mut row, cx);
    close_row(row, &got, run.record.partial.is_some(), cx);
    // r10 ③（dogfood r10 の承認の後の行は stages が []）: 続きの段は止まっても `cx.staged` に渡す（記録の行に残る）・走った手と探した手の数
    let outcome = match &got {
        Ok(ran) => done(format!("Continued from the step “{step}” · {}", ran.join("・"))),
        Err(e) => execution_stopped(e),
    };
    let s = StageLog { stage: Stage::Execution, outcome, ms: t.elapsed().as_millis() as u64, counts: Some(execution_counts(&run)) };
    if let Some(f) = cx.staged {
        f(&s);
    }
    got?;
    let mut rec = run.record;
    rec.stages.push(s);
    Ok(rec)
}

/// r10 ③: 止まった Execution の段の終わり方（承認・判断を待つ = Skipped・それ以外 = Failed）。`run_stages` と `resume` の 2 か所。
fn execution_stopped(e: &EngineError) -> StageOutcome {
    match e {
        EngineError::Denied(d) if d.pending.is_some() => StageOutcome::Skipped { why: format!("Waiting for approval · {}", d.why) },
        EngineError::Decide(_) => StageOutcome::Skipped { why: "Waiting for your choice".into() },
        e => StageOutcome::Failed { why: e.to_string() },
    }
}

/// r10 ③: Execution の段の数字（走った手〈探す段の手も〉・探す段で足した手・読み戻しの ms）。
fn execution_counts(run: &EngineRun) -> Value {
    json!({
        "steps": run.plans.iter().map(|p| p.done.len()).sum::<usize>(),
        "searches": run.plans.iter().map(|p| p.searches).sum::<usize>(),
        "readback_ms": run.plans.iter().map(|p| p.readback_ms).sum::<u64>(),
    })
}

/// WF2: 再起動の後に、人を待っている Engine の run（runs 表の `waiting_human` / `waiting_approval` で cursor を持つ行・作った順）。
/// Wave: WF2
/// 例: `for p in capability_engine::waiting_runs(&store) { /* p.space_id の Project に p.asked の card */ }`
/// 保証する: registry も model も使わない（止まった時に残した card をそのまま返す）・形の読めない行は飛ばす。
pub fn waiting_runs(store: &crate::storage::Store) -> Vec<PausedRun> {
    use crate::domain::run::RunStatus;
    let Ok(rows) = store.list(crate::storage::Table::Runs, None) else { return Vec::new() };
    rows.into_iter()
        .filter_map(|r| {
            let rec: crate::runtime::receipt::RunRecord = serde_json::from_value(r.json).ok()?;
            let waits = rec.workflow.is_some() && matches!(rec.run.status, RunStatus::WaitingHuman | RunStatus::WaitingApproval);
            let run: EngineRun = serde_json::from_value(rec.input).ok().filter(|_| waits)?;
            Some(PausedRun { space_id: rec.space_id, run_id: rec.run.id, goal: run.record.goal, asked: run.asked? })
        })
        .collect()
}

/// CHAT-013: Page の app（`cx.target`）の名指しの書類（`cx.document`・`cx.document_file`）が閉じている時（`gui::is_closed`）、model を呼ぶ前に
/// 「開く」手（`mac_gui::OPEN_DOCUMENT`・承認つき）を `Denied` の `pending` で出す。承認が在れば registry の gate を通して開き（Adapter が窓を待つ）、
/// Ok で先へ（同じ一言の残りの段 = 書類の窓が在る道）。開いている・窓を読めない・開く手が registry に無い・書類の場所が無い → Ok（何もしない = 今まで通り）。
/// 保証する: 開く承認の無い間は何も開かない・Project の repo の書類だけを開く（別の folder の同じ名前の書類で代用しない）。
fn open_named(cx: &mut EngineContext, app: &str, document: &str) -> Result<(), EngineError> {
    let Some(file) = cx.document_file else { return Ok(()) };
    let actions = cx.registry.actions();
    let ready = |id: &str| actions.iter().find(|a| a.id == id && a.readiness == crate::domain::action::ActionReadiness::Ready);
    let (Some(open), Some(_)) = (ready(OPEN_DOCUMENT_ACTION), ready("desktop.windows.list")) else { return Ok(()) };
    let windows = match cx.registry.invoke(cx.caller, "desktop.windows.list", &json!({ "bundle_id": app }), None) {
        (Outcome::Verified { .. } | Outcome::Unverified { .. }, Some(obs)) => match serde_json::from_value::<Vec<crate::gui::WindowRef>>(obs.payload) {
            Ok(ws) => ws,
            Err(_) => return Ok(()),
        },
        _ => return Ok(()),
    };
    if !crate::gui::is_closed(app, document, file, &windows) {
        return Ok(());
    }
    // 見せる app の名前: 窓 → 起動中の app → bundle id の最後の語（起動していない app）
    let name = windows.iter().find(|w| w.bundle_id == app).map(|w| w.app_name.clone()).or_else(|| match cx.registry.invoke(cx.caller, "desktop.apps.list", &json!({ "bundle_id": app }), None) {
        (_, Some(obs)) => obs.payload.as_array().and_then(|a| a.first()).and_then(|a| a["name"].as_str()).filter(|n| !n.is_empty()).map(str::to_owned),
        _ => None,
    });
    let name = name.unwrap_or_else(|| app.rsplit('.').next().unwrap_or(app).to_owned());
    let input = json!({ "bundle_id": app, "app_name": name, "document": document, "document_file": file });
    let Some(consent) = workflow::consent_for(cx.consents, OPEN_DOCUMENT_ACTION, &input) else {
        let pending = PendingApproval { step: "open".into(), action: OPEN_DOCUMENT_ACTION.into(), digest: sdk::payload_digest(OPEN_DOCUMENT_ACTION, &input), input, scopes: open.required_scopes.clone() };
        return Err(EngineError::Denied(Denial { why: format!("“{document}” isn't open in {name}. Shibubu can open this project's {document} first"), pending: Some(pending), later: Vec::new(), searched: Searched::default(), ran: 0 }));
    };
    match cx.registry.invoke(cx.caller, OPEN_DOCUMENT_ACTION, &input, Some(consent)) {
        (Outcome::Verified { .. }, _) => Ok(()),
        (o, _) => Err(EngineError::Denied(format!("“{document}” couldn't be opened in {name} ({}). Nothing was changed", workflow::why_of(&o)).into())),
    }
}

/// CHAT-013: 名指しの書類を開く手の action（Adapter の `mac_gui::OPEN_DOCUMENT` と同じ id・Core は Adapter の型を持たない = 字で）。
pub const OPEN_DOCUMENT_ACTION: &str = "desktop.document.open";

/// `run` の段の本体（doc は `run`）。
/// OPEN-INPLACE-1101 の 5: 前に分けた操作（`run_with` の `planned`）の時の GoalInterpretation の段の字（AI は呼ばない）。
/// r8 (b)（operator 11-03）: W9 から `planned` は Preview の plan だけでなく chat の振り分け（`CoreEngine::classify` の `PLANNED`）からも来る =
/// 「Preview」だけの字は宛先の app（Apple の Preview）と読み違えられた
const FROM_PLAN: &str = "split earlier for this request, by the chat or Preview · no AI";

/// OPEN-INPLACE-1101 の 5（234・owner 240「chat が約束した事と Engine の手が違う」）: 一言の手順を、何も送らずに作る。`run_stages` と同じ順の前半だけ
/// （interpret → 宛先の補い → discover → 保存した手順の再利用 → 足りない操作だけ generate → repo の外の門 → 保存）。解析・試験・手の前の読み・実行はしない。
/// 保存した手順は、返した操作を渡した `run_with` が `usable` で拾う（生成 0 回）。Adapter を新しく作る手順（recipe）は試験を流せないので保存しない
/// （Build の時に作り直す = generate が 1 回多い）。呼ぶ側は `cx.document` を None にする（`generate` の頭が書類を開く手を打たない）・承認は空。
/// 例（`CoreEngine::plan`）: `let (ops, workflows) = engine::plan(&req.goal, &mut cx)?;`
/// Wave: OPEN-INPLACE-1101
pub fn plan(goal: &str, cx: &mut EngineContext) -> Result<(Vec<Operation>, Vec<Workflow>), EngineError> {
    let goal = goal.trim().to_string();
    cx.generate_calls = 0;
    // run_stages と同じ順（宛先の補い・MCP の server の名指し・分けた一覧・保存する手順の持ち主）
    let mut ops = interpret(&goal, cx.situation, cx.llm, cx.stop)?;
    // ENGINE-AUDIT W9 本体: 名指したのに宛先が決まらない操作は Page の app で埋めない（代用しない）
    if ops.iter().all(|op| op.target.is_none()) && cx.target.is_some() {
        ops.iter_mut().filter(|op| !op.named).for_each(|op| op.target = cx.target.map(str::to_owned));
    }
    let said: Vec<&str> = std::iter::once(goal.as_str()).chain(ops.iter().flat_map(|o| [o.want.as_str(), o.target.as_deref().unwrap_or("")])).collect();
    let tools = mcp_records(cx.mcp, &said);
    let saved = load_workflows(&cx.data_dir.join(WORKFLOWS_DIR)).map_err(|e| EngineError::Store(e.to_string()))?;
    let split: Vec<String> = if ops.len() > 1 { ops.iter().map(|o| o.id.clone()).collect() } else { Vec::new() };
    let mut out = Vec::new();
    for op in &ops {
        let mut found = discover_with(cx, &tools);
        // ENGINE-AUDIT W4: Preview も同じ候補（口の棚）で作る
        if let Some(app) = op.target.clone() {
            found.extend(surface_records_of(cx, &app));
        }
        let id = workflow_id(&goal, op);
        // 使える保存した手順・保存した試験で確かめ直す手順は run が使う（作り直さない・上書きしない）
        if let Some(w) = saved.iter().find(|w| w.id == id && (usable(w, &found, cx.registry) || load_trial(cx.data_dir, &w.id).is_some())) {
            out.push(w.clone());
            continue;
        }
        // ENGINE-AUDIT W2: 本物で失敗した手順は Preview でも使わず、前の失敗を hint に作り直す
        let hint = saved.iter().find(|w| w.id == id && w.fails > 0).map(failed_before);
        let g = generate(op, &found, cx, hint.as_deref())?;
        let mut wf = g.workflow;
        (wf.id, wf.goal) = (id, goal.clone());
        let reads = wf.steps.iter_mut().filter(|s| s.capability == SAVED_READ).map(|s| &mut s.input);
        for i in reads.chain(wf.checks.iter_mut().filter(|c| c.action == SAVED_READ).map(|c| &mut c.input)) {
            in_repo(i, cx.repo);
        }
        let outside = |i: &Value| outside_repo(i, cx.repo, cx.document_file);
        if wf.steps.iter().any(|s| s.capability == SAVED_READ && outside(&s.input)) || wf.checks.iter().any(|c| c.action == SAVED_READ && outside(&c.input)) {
            return Err(EngineError::Denied(Denial::from(OUTSIDE_REPO)));
        }
        if g.recipe.is_none() && !g.context {
            let keep = Workflow { proven: None, fails: 0, split: split.clone(), ui: Some(format!("project.{}", cx.caller.app_id)), ..wf.clone() };
            save_workflow(&cx.data_dir.join(WORKFLOWS_DIR), &keep).map_err(|e| EngineError::Store(e.to_string()))?;
        }
        out.push(wf);
    }
    Ok((ops, out))
}

fn run_stages(goal: &str, planned: Option<Vec<Operation>>, cx: &mut EngineContext) -> Result<EngineRecord, EngineError> {
    let goal = goal.trim().to_string();
    cx.generate_calls = 0;
    let mut stages = Vec::new();
    // ENGINE-AUDIT W1: 段の記録は record に積み、`cx.staged`（止まった run の記録の口）にも同じ物を渡す。counts = 段の数字（None = 無し）
    let staged = cx.staged;
    let mut log = |stage: Stage, outcome: StageOutcome, since: Instant, counts: Option<Value>| {
        let s = StageLog { stage, outcome, ms: since.elapsed().as_millis() as u64, counts };
        if let Some(f) = staged {
            f(&s);
        }
        stages.push(s);
    };

    let t = Instant::now();
    // ENGINE-ADJUST-1102 ①: 人の Stop の印は段の間で見る（立っていれば次の段に入らず、AI も Adapter も呼ばずに `STOPPED_LINE`）
    check_stop(cx)?;
    // WF-036: AI が失敗した（不在・時間切れ・上限・答えの形）→ この run の残りは AI 無しの道（目的をそのまま 1 つの操作・保存した手順・確かめ済みの能力の 1 手）
    let mut without_ai: Option<String> = None;
    // ENGINE-AUDIT W2: この一言の手順が全部 proven なら interpret を飛ばす（AI 0 回・2 回目は速く確か）
    let proven = planned.is_none().then(|| proven_ops(&goal, cx.data_dir)).flatten();
    let proven_used = proven.is_some();
    let planned = planned.or(proven);
    let from_plan = planned.is_some();
    // ENGINE-AUDIT W1: AI を呼ぶ段は 1 行を出し、回数を段の数字に
    let ai_used = planned.is_none() && cx.llm.is_some();
    if ai_used {
        say(cx, "Asking the AI what you meant… (up to 2 min)");
    }
    let mut ops = match planned.map_or_else(|| interpret(&goal, cx.situation, cx.llm, cx.stop), Ok) {
        Err(EngineError::Model(why)) if cx.llm.is_some() => {
            trace(cx, "interpret", t, Some(&EngineError::Model(why.clone())), json!({ "without_ai": true }));
            cx.llm = None;
            without_ai = Some(ai_why(&why));
            interpret(&goal, cx.situation, None, cx.stop)?
        }
        got => got.inspect_err(|e| trace(cx, "interpret", t, Some(e), json!({})))?,
    };
    check_stop(cx)?;
    let filled = fill_page_target(&mut ops, cx.target);
    // W9 本体: Engine が行うのは act だけ（question は会話の AI・change は composer = chat の振り分け `domain::chat::route`）。act が 1 つも無ければ 1 行で止める
    let kinds: Vec<OperationKind> = ops.iter().map(|o| o.kind).collect();
    ops.retain(|o| o.kind == OperationKind::Act);
    if ops.is_empty() {
        let why = if kinds.contains(&OperationKind::Change) {
            "This asks to change Shibubu's own page or parts, not an app, so the Engine didn't run. Ask for the change in the chat"
        } else if kinds.contains(&OperationKind::Question) {
            "This is a question, not something to do in an app, so nothing was sent. Ask it in the chat"
        } else {
            "There's nothing to do in an app here, so nothing was sent"
        };
        return Err(EngineError::Denied(Denial::from(why)));
    }
    // W9 本体: 候補が 2 つ以上・名指したのに宛先が無い = 代用せず 1 行で止める（chat の振り分けは Engine に来る前に 1 回聞く = ここは Build now 等の直の道）
    if let Some(o) = ops.iter().find(|o| o.target.is_none() && (o.candidates.len() >= 2 || o.named)) {
        let why = if o.candidates.len() >= 2 {
            format!("Which app should this use for “{}”? More than one could fit, so nothing was sent. Say which, then ask again", o.want)
        } else {
            format!("Shibubu couldn't tell which app you meant by “{}”, so nothing was sent. Name the app as it appears in the Dock, then ask again", o.want)
        };
        return Err(EngineError::Denied(Denial::from(why)));
    }
    let said: Vec<Value> = ops.iter().map(|o| json!({ "id": o.id, "target": o.target, "want": scrub(&o.want), "success": scrub(&o.success) })).collect();
    trace(cx, "interpret", t, None, json!({ "page_filled": filled, "operations": said, "situation": { "facts": cx.situation.facts.len(), "files": cx.situation.files.len() } }));
    let how = if proven_used {
        "proven procedures (no AI)".to_string()
    } else if from_plan {
        FROM_PLAN.to_string()
    } else {
        cx.llm.map_or_else(|| "model 無し: 目的をそのまま 1 つの操作に".to_string(), |l| l.label())
    };
    log(Stage::GoalInterpretation, done(format!("{} 個の操作に分けた（{how}）", ops.len())), t, Some(json!({ "ai": u8::from(ai_used), "operations": ops.len() })));

    let t = Instant::now();
    say(cx, "Looking for ways to do it on this Mac…");
    // MCP の server は run ごとに 1 回だけ。L4: 起こすのは一言・操作が名指した server だけ・前に答えた server は覚えた答えを使う（`mcp_records`）
    let said: Vec<&str> = std::iter::once(goal.as_str()).chain(ops.iter().flat_map(|o| [o.want.as_str(), o.target.as_deref().unwrap_or("")])).collect();
    let tools = mcp_records(cx.mcp, &said);
    let mut plans: Vec<Plan> = Vec::new();
    for op in ops {
        let mut found = discover_with(cx, &tools);
        // ENGINE-AUDIT W4: 宛先の app の口の棚（bundle を読むだけ）を候補に足す（discover の頭・app の名前の枝は無い）
        if let Some(app) = op.target.clone() {
            found.extend(surface_records_of(cx, &app));
        }
        plans.push(Plan { context: false, op, found, choice: None, workflow: None, recipe: None, trial: None, reused: false, recheck: false, alone: false, before: Vec::new(), evidence: Vec::new(), ask: None, hint: None });
    }
    let all: usize = plans.iter().map(|p| p.found.len()).sum();
    let sure: usize = plans.iter().map(|p| p.found.iter().filter(|r| is_verified(r)).count()).sum();
    log(Stage::CapabilityDiscovery, done(format!("{all} 件（確かめ済み {sure} 件・registry・保存した能力・MCP の tools）")), t, Some(json!({ "found": all, "verified": sure, "mcp": tools.len() })));
    check_stop(cx)?;

    // W19-3: 解析は通常の探索で足りない時だけ。ここでは前に解析した結果を読むか、利用者が承認した操作だけを解析し、結果を候補（Unverified）として found に足す。
    // 承認がまだ無い操作は何もせず先へ進み、Dynamic Generation で手段が無いと分かった時に初めて解析の承認を求める（`analysis_ask`）。
    // 解析の失敗は既存の道で続ける（解析を必須にしない）
    let t = Instant::now();
    let saved = load_workflows(&cx.data_dir.join(WORKFLOWS_DIR)).map_err(|e| EngineError::Store(e.to_string()))?;
    let (mut notes, mut ran, mut failed) = (Vec::<String>::new(), 0, 0);
    for p in plans.iter_mut() {
        let (note, cands) = match analysis_plan(&goal, p, &saved, cx) {
            AnalysisPlan::Skip(why) => (why, Vec::new()),
            AnalysisPlan::Saved(note, c) => (note, c),
            AnalysisPlan::IfShort(target) => {
                p.ask = Some(target);
                ("通常の探索で足りるかを先に試す（手段が無ければ解析の承認を求める）".to_string(), Vec::new())
            }
            AnalysisPlan::Run(id, req) => match analysis::load_report(&cx.data_dir.join(ANALYSES_DIR), &id) {
                // 同じ対象の前の操作がこの run で解析した（同じ app を 2 回解析しない）
                Ok(r) => ("同じ app の前の操作で解析した結果を使った".to_string(), candidates_from(&r)),
                Err(_) => match deep_analysis(&req, cx) {
                    Ok(d) => {
                        ran += 1;
                        (analyzed_note(&d), d.candidates)
                    }
                    Err(e) => {
                        failed += 1;
                        (e.to_string(), Vec::new())
                    }
                },
            },
        };
        for c in cands {
            if !p.found.iter().any(|r| r.id == c.id) {
                p.found.push(c);
            }
        }
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    let why = notes.join("・");
    log(
        Stage::DeepAnalysis,
        match (ran, failed) {
            (0, 0) => StageOutcome::Skipped { why },
            (0, _) => StageOutcome::Failed { why },
            _ => done(why),
        },
        t,
        Some(json!({ "ran": ran, "failed": failed })),
    );
    check_stop(cx)?;

    let t = Instant::now();
    for p in &mut plans {
        let id = workflow_id(&goal, &p.op);
        let same = saved.iter().find(|w| w.id == id);
        p.workflow = same.filter(|w| usable(w, &p.found, cx.registry)).cloned();
        p.reused = p.workflow.is_some();
        // ENGINE-AUDIT W7: 保存した手順（W7 の前の click だけの手順も）の押す手も Core が reveal / click を決め直す（file は書き換えない）
        if let Some(w) = p.workflow.as_mut() {
            recipe::reveal_where_known(&mut w.steps, &cx.registry.actions());
        }
        // ENGINE-AUDIT W2: 本物で失敗した手順は使わず、前の失敗を hint に作り直す
        if let Some(w) = same.filter(|w| w.fails > 0) {
            p.hint = Some(failed_before(w));
        }
        // W2: 再利用の前に、最初の押す名前を今の木で照らす（無ければ再利用せず作り直す = owner の Mac の "Sounds" の手順を使わない）。窓を読めない時は今まで通り
        if let Some(w) = p.workflow.as_ref().filter(|w| w.steps.iter().any(|s| presses(&s.capability))) {
            if let Ok(f) = recipe::facts(&p.op, cx) {
                // W3: 照合は全部の名前（`Facts::tree`・r9 から押す側と同じ数え方）と
                if let Some(why) = recipe::unseen_label(&w.steps, f.tree()) {
                    p.hint = Some(format!("The steps saved for this request can't be used now: {why}. Make different steps: first open or select what is needed, and press only names in window_controls"));
                    (p.workflow, p.reused) = (None, false);
                }
            }
        }
        // 使えなくなった保存した手順（版・OS・権限が変わった・Adapter が読み込まれていない）は、作り直す前に保存した試験で確かめ直す
        if let (false, Some(w), Some((rid, trial))) = (p.reused, same, same.and_then(|w| load_trial(cx.data_dir, &w.id))) {
            if let Ok(Some(r)) = load_recipe(&cx.data_dir.join(EXTENSIONS_DIR), &rid) {
                if matches!(r.origin, Origin::Generated { .. }) {
                    (p.workflow, p.recipe, p.trial, p.recheck) = (Some(w.clone()), Some(r), Some(trial), true);
                }
            }
        }
        p.choice = match (&p.workflow, p.recheck) {
            (Some(w), false) => Some(choice_of(&p.op, w, &p.found, format!("保存した手順「{}」を再利用（生成 0 回）", w.id))),
            (Some(w), true) => Some(choice_of(&p.op, w, &p.found, format!("保存した手順「{}」を、保存した試験で確かめ直してから使う（版・OS・権限が変わった・生成 0 回）", w.id))),
            (None, _) => choose(&p.op, &p.found),
        };
    }
    // WF-036（WF-E x6・x6b・x6c）: model 無しの道（AI が失敗した・初めから無い）は目的をそのまま 1 つの操作にする。前の AI の run がこの目的を別の操作にも分けて
    // いれば、model 無しで分かるのはその一部だけ = 1 つ目だけ走らせて済みにしない（後で頼み直すと走った手をもう一度送る）→ 1 手も送らずに止める。
    // 分けた操作 = 保存した手順の id と、各手順が覚えている分けた一覧（`Workflow::split`・op1 だけ保存して op2 の前で止まった目的も = x6c）の和
    let here: Vec<String> = plans.iter().map(|p| workflow_id(&goal, &p.op)).collect();
    let mut more: Vec<String> = Vec::new();
    for w in saved.iter().filter(|w| w.goal == goal) {
        let split = w.split.iter().map(|id| workflow_id(&goal, &Operation { id: id.clone(), want: String::new(), success: String::new(), target: None, ..Default::default() }));
        for id in std::iter::once(w.id.clone()).chain(split) {
            if !here.contains(&id) && !more.contains(&id) {
                more.push(id);
            }
        }
    }
    let more = more.len();
    if cx.llm.is_none() && more > 0 {
        return Err(match without_ai {
            Some(_) => no_ai(EngineError::NoMeans { operation: goal.clone(), tried: Vec::new(), searched: Searched::default() }, without_ai.as_deref()),
            // 初めから model が無い = 理由と次の一手（AI の道具を足す所）
            None => EngineError::Denied(Denial::from(format!("This goal has {} actions and needs an AI tool · Add one in Settings › AI & Models", plans.len() + more))),
        });
    }
    let reuse = plans.iter().filter(|p| p.reused).count();
    let again = plans.iter().filter(|p| p.recheck).count();
    let fit = plans.iter().filter(|p| p.workflow.is_none() && p.choice.is_some()).count();
    log(
        Stage::IntegrationPlanning,
        done(format!("再利用 {reuse}・確かめ直す {again}・確かめ済みの能力が在る {fit}・手順を作る {}", plans.len() - reuse - again)),
        t,
        Some(json!({ "reused": reuse, "recheck": again, "fit": fit })),
    );

    check_stop(cx)?;
    let t = Instant::now();
    let mut generated = Vec::new();
    let mut alone_n = 0;
    // ENGINE-AUDIT W1: 操作ごとの generate の数字（AI の回数・作り直し・読んだ要素・見せた行・窓）を足し合わせる
    let mut gen_counts = json!({ "ai": 0, "retries": 0, "elements": 0, "shown": 0, "windows": 0 });
    for p in plans.iter_mut().filter(|p| p.workflow.is_none()) {
        // ENGINE-ADJUST-1102 ①: 操作ごとの AI の前にも人の Stop を見る（generate の中は facts の後と作り直しの前・AI の待ちは cancellable）
        check_stop(cx)?;
        // WF-036: 計画の AI が失敗した → この run の残りは AI を外し、確かめ済みの能力の 1 手に戻る（無ければ AI を使わなかった事を tried に）
        let failed = if cx.llm.is_none() {
            None
        } else {
            match generate(&p.op, &p.found, cx, p.hint.as_deref()) {
                Err(EngineError::Model(why)) => {
                    cx.llm = None;
                    without_ai.get_or_insert(ai_why(&why));
                    None
                }
                got => Some(got),
            }
        };
        let Some(got) = failed else {
            p.workflow = Some(alone(&goal, &p.op, &p.found, &cx.registry.actions()).map_err(|e| no_ai(e, without_ai.as_deref()))?);
            p.alone = true;
            alone_n += 1;
            continue;
        };
        let g = match got {
            // 通常の探索で手段が無い: 解析してよい対象で、使える解析の道具が在る時だけ、ここで初めて解析の承認を求める（何も解析していない・何も走らせていない。
            // 承認の後の run が Deep Analysis の段で解析する）。断られたら手段が無いまま（既存の道はもう試した）
            Err(EngineError::NoMeans { operation, tried, searched }) => {
                return Err(match p.ask.as_ref().and_then(|t| analysis_ask(&p.op, t, cx.analysis)) {
                    Some(a) => EngineError::Denied(Denial { why: ANALYSIS_WAIT.into(), pending: Some(a), later: Vec::new(), searched: Searched::default(), ran: 0 }),
                    None => EngineError::NoMeans { operation, tried, searched },
                });
            }
            other => other?,
        };
        add_counts(&mut gen_counts, &g.counts);
        let mut wf = g.workflow;
        (wf.id, wf.goal) = (workflow_id(&goal, &p.op), goal.clone());
        if let Some(r) = &g.recipe {
            generated.push(r.id.clone());
        }
        generated.push(wf.id.clone());
        (p.workflow, p.recipe, p.trial, p.context) = (Some(wf), g.recipe, g.trial, g.context);
    }
    log(
        Stage::DynamicGeneration,
        match (generated.is_empty(), alone_n) {
            (true, 0) => StageOutcome::Skipped { why: "保存した手順を使った（生成 0 回）".into() },
            (true, _) => StageOutcome::Skipped { why: "model 無し: 確かめ済みの能力を入力なしの 1 手にした（生成 0 回）".into() },
            (false, _) => done(format!("Made: {}", generated.join("・"))),
        },
        t,
        Some(gen_counts),
    );

    check_stop(cx)?;
    let t = Instant::now();
    let mut notes = Vec::new();
    // WF-E x6c: AI が目的を 2 つ以上に分けた run は、保存する手順ごとに分けた一覧を残す（後の model 無しの run の門が数える）
    let split: Vec<String> = if plans.len() > 1 { plans.iter().map(|p| p.op.id.clone()).collect() } else { Vec::new() };
    // operator 11-03: 読み戻しが全部「手の前からもう合う contains」だった操作は 1 回だけ作り直して同じ操作をこの段の頭からやり直す（`k` を進めない）
    let mut redone = vec![false; plans.len()];
    // その作り直しの generate の数字（AI の回数・ms_ai 等・DynamicGeneration の段は記録済みなので Verification の段の counts の rebuilt に）
    let mut rebuilt = json!({});
    let mut k = 0;
    while k < plans.len() {
        let p = &mut plans[k];
        if let Some(w) = p.workflow.as_mut() {
            // B74（録画の手 7・cu.GkMp 11:43:22Z）: model が自分で足した書類の読み（`SAVED_READ`・読み戻しにも読む手にも）の repo からの相対の uri（"README.md"）は
            // repo の下の絶対に直す（前は Adapter が InvalidInput で手の前の読みが止まった）。repo の外（`..`）・repo が無い時は直さない = Adapter が止める（repo の外は読まない）。
            // 試験の checks（`Trial`）は直さない（写しを読む物・本物の repo を指させない）
            let reads = w.steps.iter_mut().filter(|s| s.capability == SAVED_READ).map(|s| &mut s.input);
            for i in reads.chain(w.checks.iter_mut().filter(|c| c.action == SAVED_READ).map(|c| &mut c.input)) {
                in_repo(i, cx.repo);
            }
            // B76（安全・Orca task_e1fa3405c76e）: model の書いた読み（読み戻しにも読む手にも）は Project の repo の下だけ。Adapter は「.. が無い・祖先に .git」しか見ないので、
            // 承認の要らない読みでこの Mac の別の repo の file（ignore された .env も disk には在る）が読めた → 外は 1 つも読まず送らずに止める（symlink は解いた先で見る）。
            // Engine 自身の書類の読み（`saved_checks` = `document_file`・前の run で保存した手順に残る物を含む）は縛らない。
            // 読んだ中身は model には渡らない（generate は実行の前・interpret は一言と状況の事実だけ）。後の手へは `$from` で渡り得るが、書く手と external_* へ渡す手は埋めた後の exact payload の承認が要る。
            // uri が `$from`（文字列でない）の読む手もここで止める（fail closed）。埋めた後の uri は `run_steps` がもう一度見る（計画の段を通らない resume も）
            let outside = |i: &Value| outside_repo(i, cx.repo, cx.document_file);
            if w.steps.iter().any(|s| s.capability == SAVED_READ && outside(&s.input)) || w.checks.iter().any(|c| c.action == SAVED_READ && outside(&c.input)) {
                return Err(EngineError::Denied(Denial::from(OUTSIDE_REPO)));
            }
            // G81: 書類の disk の場所が在れば、名指しの書類に字を置く手の済みを保存した file の中身にも縛る（作った手順にも、G81 の前に保存した手順にも・重ねない）
            if let (Some(file), Some(d)) = (cx.document_file, cx.document) {
                let saved = saved_checks(&w.steps, d, file);
                // B74: Engine が同じ file を正しい形（/text・頼んだ差分）で読む時は、model の同じ file の読みを落とす（重ねない・model の pointer は当て推量 = 実例 /content）。
                // B76: document_file（generated.rs・面の file を正規化しない = `<repo>/./README.md` も在る）と `in_repo` の答えは `same_file` で同じ形にして比べる
                if !saved.is_empty() {
                    w.checks.retain(|c| c.action != SAVED_READ || !c.input["uri"].as_str().is_some_and(|u| same_file(u, file)) || saved.contains(c));
                }
                for c in saved {
                    if !w.checks.contains(&c) {
                        w.checks.push(c);
                    }
                }
            }
        }
        let mut wf = p.workflow.clone().expect("Integration Planning か Dynamic Generation が手順を決めた");
        if let (Some(recipe), Some(trial)) = (p.recipe.as_mut(), p.trial.as_ref()) {
            // RQ-023（WF-E x7）: 生成した Adapter の試し（写しへ書く・MCP の server へ送る手を含む）も送る手 = Suggest はその前で止める（他の 2 つは今まで通り）
            if cx.automation == crate::domain::permission::AutomationMode::Suggest {
                return Err(EngineError::Denied(Denial::from(format!("{SUGGEST}, not even the try on a copy that comes first. {SUGGEST_RUN}"))));
            }
            let dir = trial_dir(cx.data_dir, &p.op);
            prepare_trial(&dir, &wf)?;
            let lines = match run_trial(recipe, trial, &dir, &wf, cx) {
                Ok(lines) => lines,
                Err(EngineError::Generation(why)) if p.recheck => {
                    // 確かめ直しに通らない保存した試験は捨てる（次の run は作り直す・同じ所で止まり続けない）
                    let _ = std::fs::remove_file(saved_trial_path(cx.data_dir, &wf.id));
                    return Err(EngineError::Generation(format!("The saved adapter was checked again and didn't pass ({why}). Ask again and it will be rebuilt")));
                }
                Err(e) => return Err(e),
            };
            mark_verified(recipe, trial, &lines.join(" / "));
            // 同じ id の前の Adapter（同じ app の前の操作で作った物）で確かめた action は、同じ定義なら確かめを残す（前の目的の手順を作り直しにしない）
            if let Ok(Some(old)) = load_recipe(&cx.data_dir.join(EXTENSIONS_DIR), &recipe.id) {
                keep_verified(recipe, &old);
            }
            if let Some(a) = recipe.actions.iter().find(|a| !is_verified_action(a) && uses(&wf, &a.id)) {
                return Err(EngineError::Generation(format!("Won't run a procedure that uses {} for real without a test in isolation", a.id)));
            }
            register(recipe, &dir, cx)?;
            save_recipe(&cx.data_dir.join(EXTENSIONS_DIR), recipe)?;
            save_trial(cx.data_dir, &wf.id, &recipe.id, trial)?;
            // 試験で確かめた能力は Execution の前に残す（承認を待って止まった run の後も、次の run が作らずに使える）。前の確かめを残しただけの action は書き直さない
            let ran = used_in(trial);
            for a in recipe.actions.iter().filter(|a| is_verified_action(a) && ran.contains(a.id.as_str())) {
                save_record(&cx.data_dir.join(CAPABILITIES_DIR), &record_of(recipe, a, &cx.env, lines.clone()))?;
            }
            notes.push(format!("{} ({})", recipe.id, lines.join(" / ")));
            p.evidence.extend(lines);
        }
        // G74d: 手を走らせる前の読みが読めない（前の run の menu が開いたまま・app が答えない）= 何も打っていない → Denied（Adapter の 1 行・
        // 「確かめられない」Verification の文にしない）
        p.before = wf
            .checks
            .iter()
            .map(|c| {
                read_check(c, cx).map_err(|o| {
                    // B75: 形の違う入力（手順の書き方の誤り = 利用者には直せない）は理由の語（Input missing or wrong）でなく次の一手（この run は手順を保存していない = 頼み直せば書き直す）
                    let why = match o {
                        Outcome::Failed { error: ErrorCode::InvalidInput } => UNREADABLE_CHECK.to_owned(),
                        o => format!("Nothing was sent, because the original couldn't be read first: {}", outcome_word(&o)),
                    };
                    let e = EngineError::Denied(Denial::from(why));
                    // G77: QA の trace だけ、読めなかった読み（input は key だけ）と宛先の app の今の窓の正体（どの pid の・どれが名指しの書類か）
                    if cx.trace {
                        let detail = json!({ "op": p.op.id, "action": c.action, "input_keys": c.input.as_object().map(|o| o.keys().collect::<Vec<_>>()), "windows": windows_now(c, cx) });
                        trace(cx, "before", t, Some(&e), detail);
                    }
                    e
                })
            })
            .collect::<Result<_, _>>()?;
        // operator 11-03（r10 の check）: 書く手の在る手順で、手の前の読みでもう合う contains は押した証拠にしない = その check と手の前の読みを外す（保存する手順からも）。
        // 残りが 0 = 済みの根拠が無い計画 → Unconfirmed まで送らず、理由（`recipe::STALE_ONLY`）を hint に AI 1 回で作り直す（保存した手順もこの run で作り直す）。
        // 作り直しても 0・AI が無い → 送らない（Generation の 1 行）
        let actions_now = cx.registry.actions();
        let acts = wf.steps.iter().any(|s| s.is_action() && actions_now.iter().any(|a| a.id == s.capability && !is_read(a.effect)));
        let stale: Vec<bool> = wf.checks.iter().zip(&p.before).map(|(c, b)| acts && recipe::already_contains(c, b)).collect();
        if !stale.is_empty() && stale.iter().all(|s| *s) {
            if redone[k] || cx.llm.is_none() {
                log(Stage::Verification, StageOutcome::Failed { why: recipe::STALE_ONLY.into() }, t, Some(json!({ "rebuilt": rebuilt })));
                return Err(EngineError::Generation(recipe::STALE_ONLY.into()));
            }
            redone[k] = true;
            cx.generate_calls = recipe::GENERATE_CALLS_MAX - 1;
            let g = recipe::generate(&p.op, &p.found, cx, Some(recipe::STALE_ONLY))?;
            add_counts(&mut rebuilt, &g.counts);
            notes.push(format!("Made the steps for {} again: every check already passed before any step ran", p.op.id));
            let mut w = g.workflow;
            (w.id, w.goal) = (workflow_id(&goal, &p.op), goal.clone());
            for id in g.recipe.iter().map(|r| r.id.clone()).chain([w.id.clone()]) {
                if !generated.contains(&id) {
                    generated.push(id);
                }
            }
            (p.workflow, p.recipe, p.trial, p.context, p.reused, p.recheck, p.before) = (Some(w), g.recipe, g.trial, g.context, false, false, Vec::new());
            continue;
        }
        if stale.contains(&true) {
            let mut gone = stale.iter();
            wf.checks.retain(|_| !gone.next().copied().unwrap_or(false));
            let mut gone = stale.iter();
            p.before.retain(|_| !gone.next().copied().unwrap_or(false));
            p.workflow = Some(wf.clone());
        }
        // r7 ①: 文脈の手だけの手順（context）は目当ての手順ではないので保存しない
        if !p.reused && !p.alone && !p.context {
            // RQ3（RQ-012）: 保存する手順は、この run を呼んだ Project の物（`Workflow::ui`・Export がこれで引く・caller の app id → `project.<app>` は Project の id の決め方）
            let wf = Workflow { proven: None, fails: 0, split: split.clone(), ui: Some(format!("project.{}", cx.caller.app_id)), ..wf };
            save_workflow(&cx.data_dir.join(WORKFLOWS_DIR), &wf).map_err(|e| EngineError::Store(e.to_string()))?;
        }
        k += 1;
    }
    let read: usize = plans.iter().map(|p| p.before.len()).sum();
    notes.push(format!("Read {} from before the action", crate::domain::plural(read, "value")));
    let trials = plans.iter().filter(|p| p.recipe.is_some() && p.trial.is_some()).count();
    log(Stage::Verification, done(notes.join("・")), t, Some(json!({ "reads": read, "trials": trials, "rebuilt": rebuilt })));
    // ENGINE-ADJUST-1102 ①: Execution に入る前（row を作る前 = 送らない run は記録しない）
    check_stop(cx)?;

    // Execution の前の記録（見つけた手段・選んだ理由は Execution で変わらない）と、操作ごとの手順・操作の前の読み。
    // WF1: 行が在れば runs 表に凍結し、判断・承認で止まった所から `resume` が続ける
    let mut discovered: Vec<CapabilityRecord> = Vec::new();
    for r in plans.iter().flat_map(|p| p.found.iter()) {
        if !discovered.iter().any(|d| d.id == r.id) {
            discovered.push(r.clone());
        }
    }
    let choices = plans
        .iter()
        .map(|p| match (&p.recipe, p.workflow.as_ref()) {
            (_, Some(w)) if p.recheck => p.choice.clone().unwrap_or_else(|| choice_of(&p.op, w, &p.found, "Check the saved procedure again, then use it".into())),
            (Some(r), Some(w)) if !p.reused => {
                let mut found = p.found.clone();
                found.extend(r.actions.iter().map(|a| record_of(r, a, &cx.env, vec![])));
                choice_of(&p.op, w, &found, format!("The existing abilities weren't enough, so adapter “{}” was built and checked with an isolated test and a read-back from the real thing", r.id))
            }
            (None, Some(w)) if !p.reused => choice_of(&p.op, w, &p.found, "Built a procedure that combines checked abilities".into()),
            (_, Some(w)) => p.choice.clone().unwrap_or_else(|| choice_of(&p.op, w, &p.found, "A procedure that uses checked abilities".into())),
            (_, None) => p.choice.clone().unwrap_or_else(|| Choice { operation: p.op.id.clone(), capability: String::new(), reason: "No procedure".into(), score: Score::default() }),
        })
        .collect();
    let record = EngineRecord {
        schema: ENGINE_SCHEMA.into(),
        goal,
        reused: generated.is_empty(),
        operations: plans.iter().map(|p| p.op.clone()).collect(),
        discovered,
        choices,
        generated,
        verified: Vec::new(),
        receipts: Vec::new(),
        stages: Vec::new(),
        partial: None,
        without_ai,
        already: None,
    };
    let frozen = plans.iter().map(|p| RunPlan { workflow: p.workflow.clone().expect("Verification の段を通った"), before: p.before.clone(), lines: Vec::new(), done: Vec::new(), readback_ms: 0, searched: Searched::default(), searches: 0, tree: String::new(), context: p.context }).collect();
    let mut run = EngineRun { record, plans: frozen, asked: None };
    // ENGINE-AUDIT W7（spec 261「既にそうなら打たない」）: どの操作も押す手 1 つで、Core が作った読み戻し（行の selected）が手の前の読みでもう合う = 1 手も送らない
    // （row も承認の card も作らない・Automation の門の前 = 何も送らないので Suggest でも答えは同じ）。`verified` はその読み・`already` は 1 行
    let already: Option<Vec<String>> = run.plans.iter().map(|p| recipe::already(&p.workflow, &p.before)).collect();
    if let Some(lines) = already.filter(|l| !l.is_empty()) {
        let t = Instant::now();
        log(Stage::Execution, StageOutcome::Skipped { why: lines.join("・") }, t, Some(execution_counts(&run)));
        let record = EngineRecord { verified: lines.clone(), already: Some(lines.join(" · ")), ..run.record };
        return Ok(EngineRecord { stages, ..record });
    }
    // RQ-023: Automation の門（Execution の前・runs 表に行を作る前 = 送らない run は記録しない）
    automation_gate(&run, cx)?;

    let t = Instant::now();
    let mut cursor = WorkflowCursor { digest: saved_digest(cx.data_dir, &run.plans), ..Default::default() };
    let mut row = open_row(&run, &cursor, cx)?;
    let got = execute(&mut run, &mut cursor, HashMap::new(), &mut row, cx);
    close_row(row, &got, run.record.partial.is_some(), cx);
    // ENGINE-AUDIT W2: 手順が悪かった終わり方なら fails を足す（次は再利用せず作り直す）・読み戻しが通ったら proven で保存し直す
    match &got {
        Err(e) if counts_as_failure(e) => {
            for rp in &run.plans {
                amend_workflow(cx.data_dir, &rp.workflow.id, |w| w.fails += 1);
            }
        }
        Ok(_) => {
            let now = crate::runtime::now_rfc3339();
            for (p, rp) in plans.iter().zip(&run.plans) {
                // r7 ①: 文脈の手だけで終わった手順（regenerate の後は context = false）は proven にしない
                if !p.alone && !rp.context {
                    let note = format!("{now} · {} read back", crate::domain::plural(rp.lines.len(), "value"));
                    amend_workflow(cx.data_dir, &rp.workflow.id, |w| (w.proven, w.fails) = (Some(note.clone()), 0));
                }
            }
        }
        Err(_) => {}
    }
    // r10 ③: 止まった Execution も段に残す（`cx.staged` = 止まった run の記録の行・承認と判断の待ちは Skipped）
    if let Err(e) = &got {
        log(Stage::Execution, execution_stopped(e), t, Some(execution_counts(&run)));
    }
    let lines = got?;
    let counts = json!({ "steps": run.record.receipts.iter().map(|r| r.steps.len()).sum::<usize>(), "readback_ms": run.plans.iter().map(|p| p.readback_ms).sum::<u64>() });
    log(Stage::Execution, done(lines.join("・")), t, Some(counts));

    let t = Instant::now();
    let mut kept = 0;
    for (p, rp) in plans.iter().zip(&run.plans) {
        // 本物の読み戻しの行を根拠に足して書き直す（試験の行だけの記録は Verification の段で書いた・この run の試験で流した action だけ）
        let ran = p.trial.as_ref().map(used_in).unwrap_or_default();
        if let Some(r) = &p.recipe {
            let evidence: Vec<String> = p.evidence.iter().chain(&rp.lines).cloned().collect();
            for a in r.actions.iter().filter(|a| is_verified_action(a) && ran.contains(a.id.as_str())) {
                save_record(&cx.data_dir.join(CAPABILITIES_DIR), &record_of(r, a, &cx.env, evidence.clone()))?;
                kept += 1;
            }
        }
    }
    log(
        Stage::Registry,
        if kept == 0 {
            StageOutcome::Skipped { why: "No newly checked ability (used the saved one)".into() }
        } else {
            done(format!("Kept {kept} checked abilities, adapters and procedures (next time they are used instead of rebuilt)"))
        },
        t,
        Some(json!({ "kept": kept })),
    );
    Ok(EngineRecord { stages, ..run.record })
}

/// WF1: runs 表の Engine の run の凍結（`RunRecord::input`）: Execution の前までの記録・操作ごとの手順と操作の前の読み・済んだ手・人に出している card。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineRun {
    record: EngineRecord,
    plans: Vec<RunPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked: Option<Asked>,
}

/// 操作 1 つの凍結した手順と、操作の前の読み（`Workflow::checks` と同じ順・WF-004: 前の値で比べる checks〈Changed・Fewer〉の物だけ中身を持つ）・
/// 読み戻しの行・済んだ手の Receipt（止まる前の手も）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunPlan {
    workflow: Workflow,
    before: Vec<Observation>,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    done: Vec<StepReceipt>,
    /// ENGINE-AUDIT W1: 読み戻しが合うまで（か諦めるまで）の ms（Execution の段の数字 `readback_ms`）
    #[serde(default)]
    readback_ms: u64,
    /// ENGINE-AUDIT W5: 探す段の跡（この操作で探した語・近い名前・次の一手 = 止まる時の `Denial::searched`）と、AI に頼んだ次の 1 手の数（`SEARCH_STEPS_MAX` の天井）
    #[serde(default)]
    searched: Searched,
    #[serde(default)]
    searches: usize,
    /// W5: 最後に読んだ木の印（`Facts::names` の hash・同じ木が 2 回続いた = 進んでいない → 止める）。空 = まだ探していない
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tree: String,
    /// r7 ①: 今の手順が文脈の手だけ（`Generated::context`）= 読み戻しが通ったら `regenerate`（同じ操作を読み直して generate し直す）・proven で保存しない
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    context: bool,
}

/// `RunRecord::run.steps` の判断の手の action_ref（registry の action ではない）。
const DECIDE_REF: &str = "decide";

/// WF-002: 走り始めた時の保存した手順の file（`workflows/<id>.json`・操作の順）の sha256（無い手順〈model 無しの 1 手〉は数えない・全部無ければ空）。
fn saved_digest(data_dir: &Path, plans: &[RunPlan]) -> String {
    let files: Vec<Vec<u8>> = plans.iter().filter_map(|p| std::fs::read(data_dir.join(WORKFLOWS_DIR).join(format!("{}.json", p.workflow.id))).ok()).collect();
    if files.is_empty() {
        return String::new();
    }
    crate::crypto::sha256_hex(&files.concat())
}

/// 行の中身（WF-004: 前の値で比べない checks の操作の前の読みは捨てる・card の値は secret の key を伏せる）。
fn frozen(run: &EngineRun) -> Result<Value, EngineError> {
    let mut run = run.clone();
    for p in &mut run.plans {
        for (c, o) in p.workflow.checks.iter().zip(p.before.iter_mut()) {
            if !matches!(c.expect, Expect::Changed | Expect::Fewer { .. } | Expect::Replaced { .. }) {
                o.payload = Value::Null;
            }
        }
    }
    let mut v = serde_json::to_value(&run).map_err(|e| EngineError::Store(e.to_string()))?;
    if let Some(a) = v.get_mut("asked") {
        *a = crate::storage::redact_value(a);
    }
    Ok(v)
}

/// runs 表に 1 回の put。
fn keep(store: &std::sync::Mutex<crate::storage::Store>, rec: &crate::runtime::receipt::RunRecord) -> Result<(), EngineError> {
    let st = store.lock().unwrap_or_else(|e| e.into_inner());
    crate::runtime::receipt::save(&st, rec).map_err(|e| EngineError::Store(format!("Can't record the run ({e:?})")))
}

/// WF1: Execution の前に runs 表に 1 行（running・手ごとの Step〈操作の番号.手の id〉・凍結した手順と記録・cursor）。書けなければ 1 手も送らない（Store）。
/// `cx.runs` が無ければ None（今まで通り残さない）。
fn open_row(run: &EngineRun, cursor: &WorkflowCursor, cx: &EngineContext) -> Result<Option<crate::runtime::receipt::RunRecord>, EngineError> {
    let Some(runs) = cx.runs.as_ref() else { return Ok(None) };
    let steps: Vec<(String, String)> = run
        .plans
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.workflow.steps.iter().map(move |s| (format!("{i}.{}", s.id), if s.is_action() { s.capability.clone() } else { DECIDE_REF.into() })))
        .collect();
    let id = cx.caller.run_id.map_or_else(crate::storage::new_id, str::to_owned);
    let rec = crate::runtime::receipt::RunRecord::engine(&id, (cx.caller.app_id, runs.app_version), runs.session_id, runs.space_id, &steps, frozen(run)?, cursor.clone())
        .map_err(|e| EngineError::Store(format!("Can't record the run before sending ({e:?})")))?;
    keep(runs.store, &rec)?;
    // ENGINE-AUDIT W1（節 16「Engine の run には run_events が 0 行」）: runtime の run と同じ RunCreated を書く（書けなくても run は止めない）
    {
        let st = runs.store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crate::runtime::receipt::event(&st, &rec, "RunCreated", "engine", json!({ "steps": steps.len(), "operations": run.plans.len() }));
    }
    Ok(Some(rec))
}

/// WF1: 行を結果の状態にして保存する（止まって待つ run は `hold` が待つ状態で保存済み = 触らない・行が無ければ何もしない）。
/// 済み → verifying → verified・送れたか確かめられない → uncertain・確かめられない → verifying → failed・それ以外 → failed。保存できなくても答えは変えない
/// （行は running のまま残り、次の起動の `receipt::recover` が後始末する）。
fn close_row(row: Option<crate::runtime::receipt::RunRecord>, got: &Result<Vec<String>, EngineError>, partial: bool, cx: &EngineContext) {
    use crate::domain::run::RunStatus as S;
    let (Some(mut rec), Some(runs)) = (row, cx.runs.as_ref()) else { return };
    if matches!(rec.run.status, S::WaitingHuman | S::WaitingApproval) {
        return;
    }
    let path: &[S] = match got {
        // WF-007: 読み戻しは合ったが頼んだ差分を読んでいない = 確かめ切れていない
        Ok(_) if partial => &[S::Verifying, S::Uncertain],
        Ok(_) => &[S::Verifying, S::Verified],
        // ENGINE-ADJUST-1102 ①: 人の Stop = 取り消した（runtime の cancel と同じ Cancelled）。書いた後の読み戻しの途中の Stop だけ Uncertain（送れたかが分からない）
        Err(EngineError::Unconfirmed(w)) if w == STOPPED_UNCHECKED => &[S::Uncertain],
        Err(e) if is_stop(e) => &[S::Cancelled],
        Err(EngineError::Unconfirmed(_)) => &[S::Uncertain],
        Err(EngineError::Verification(_)) => &[S::Verifying, S::Failed],
        Err(_) => &[S::Failed],
    };
    for to in path {
        if crate::runtime::receipt::set_status(&mut rec, *to).is_err() {
            break;
        }
    }
    let _ = keep(runs.store, &rec);
    // ENGINE-AUDIT W1: 結末の run_event（runtime の RunVerified / RunFailed / RunCancelled と同じ名・理由は scrub した 1 行）
    let kind = match rec.run.status {
        S::Verified => "RunVerified",
        S::Cancelled => "RunCancelled",
        S::Uncertain => "RunUncertain",
        _ => "RunFailed",
    };
    let why = got.as_ref().err().map(|e| scrub(&e.to_string()));
    let st = runs.store.lock().unwrap_or_else(|e| e.into_inner());
    let _ = crate::runtime::receipt::event(&st, &rec, kind, "engine", json!({ "why": why, "partial": partial }));
}

/// WF-007（P65⑧）: 字を置く書く手（input の `PUT_KEYS` のどれかが文字列）のうち、頼んだ差分の読み戻し（`Replaced` の `to` か `Equals` の値が置いた字）が無い
/// 最初の手の 1 行（`EngineRecord::partial`）。字を置かない書く手（押す・保存）と、受け渡した値を置く手（`$from`）は見ない。
fn unasked(wf: &Workflow, actions: &[ActionDefinition]) -> Option<String> {
    let writes = |s: &WorkflowStep| s.is_action() && actions.iter().any(|a| a.id == s.capability && !is_read(a.effect));
    let asked = |t: &str| {
        wf.checks.iter().any(|c| match &c.expect {
            Expect::Replaced { to, .. } => to == t,
            Expect::Equals { value } => value.as_str() == Some(t),
            _ => false,
        })
    };
    let s = wf.steps.iter().filter(|s| writes(s)).find(|s| PUT_KEYS.iter().filter_map(|k| s.input[*k].as_str()).any(|t| !t.trim().is_empty() && !asked(t)))?;
    Some(format!("what the step “{}” put wasn't read back as the exact change you asked for", s.id))
}

/// WF-007（WF-E x2）: 書く手が人の読む字を置く input の key（本文・題・件名・説明 = 今の Adapter の schema と生成した Adapter の `title`）。
/// 宛先（app_name・document・to・repo）と押す物（label・key・path）は数えない。
const PUT_KEYS: [&str; 6] = ["text", "title", "body", "subject", "summary", "description"];

/// 読み直せる値の digest（読むたびに変わる AX の ref は落とす）。
fn digest_of(v: &Value) -> String {
    crate::crypto::sha256_hex(without_refs(v).to_string().as_bytes())
}

/// 止まった時の元データの digest（`Workflow::checks` の pointer の値・同じ順・値が無い check は空）。
fn seen_of(checks: &[ReadBack], obs: &[Observation]) -> Vec<String> {
    checks.iter().zip(obs).map(|(c, o)| o.payload.pointer(&c.pointer).map_or_else(String::new, digest_of)).collect()
}

/// WF-015: 止まる手 `step` より前で、後ろ（止まる手まで）に書く手が済んだ読む手を、止まる時に元の情報源から読み直した出力（続ける前に比べる digest の元 =
/// 自分の書き込みの後の元データ）。読めない手と、それ以外の手は走った時の出力のまま（読めなければ続ける時に違って見え、送らない側に倒れる）。
fn settled(wf: &Workflow, step: &str, outputs: &HashMap<String, Value>, cx: &mut EngineContext) -> HashMap<String, Value> {
    let upto = wf.steps.iter().position(|s| s.id == step).unwrap_or(0);
    let actions = cx.registry.actions();
    let reads = |s: &WorkflowStep| s.is_action() && actions.iter().any(|a| a.id == s.capability && is_read(a.effect));
    let mut now = outputs.clone();
    for (k, s) in wf.steps.iter().enumerate().take(upto) {
        if !reads(s) || !wf.steps[k + 1..upto].iter().any(|t| t.is_action() && !reads(t)) {
            continue;
        }
        let Ok(input) = resolve(&s.input, outputs, &s.id, "", &mut Vec::new()) else { continue };
        if let (Outcome::Verified { .. } | Outcome::Unverified { .. }, Some(o)) = cx.registry.invoke(cx.caller, &s.capability, &input, None) {
            now.insert(s.id.clone(), o.payload);
        }
    }
    now
}

/// WF-004: 止まる時に行に残す済んだ手の出力: 読む手は `{"$digest": …}` だけ（続ける前に元の情報源から読み直す）・書いた手は後ろ（`upto` から先の手・
/// 判断の選択肢・checks の期待する値）が `$from` で指す時だけ値（secret の key は伏せる）・判断の手の答えは値。
fn kept_outputs(wf: &Workflow, upto: usize, outputs: &HashMap<String, Value>, actions: &[ActionDefinition]) -> std::collections::BTreeMap<String, Value> {
    let mut refs = Vec::new();
    for s in &wf.steps[upto..] {
        from_refs(&s.input, &mut refs);
        if let StepKind::Decide { choices, .. } = &s.kind {
            choices.iter().for_each(|c| from_refs(&c.value, &mut refs));
        }
    }
    for c in &wf.checks {
        if let Expect::Equals { value } = &c.expect {
            from_refs(value, &mut refs);
        }
    }
    let used: HashSet<&str> = refs.iter().map(|r| r.split('.').next().unwrap_or("")).collect();
    wf.steps[..upto]
        .iter()
        .filter_map(|s| {
            let v = outputs.get(&s.id)?;
            let read = actions.iter().any(|a| a.id == s.capability && is_read(a.effect));
            let plain = !s.is_action() || (!read && used.contains(s.id.as_str()));
            Some((s.id.clone(), if plain { crate::storage::redact_value(v) } else { json!({ "$digest": digest_of(v) }) }))
        })
        .collect()
}

/// WF2: 手 `step`（`wf` の）で止まる cursor にする（待つ手・済んだ手の出力・止まった時の元データ）。
fn wait_at(cursor: &mut WorkflowCursor, wf: &Workflow, step: &str, outputs: &HashMap<String, Value>, actions: &[ActionDefinition], seen: Vec<String>) {
    let at = wf.steps.iter().position(|s| s.id == step).unwrap_or(0);
    (cursor.next, cursor.waiting, cursor.seen) = (at, Some(step.to_owned()), seen);
    cursor.outputs = kept_outputs(wf, at, outputs, actions);
}

/// WF1・WF2: 凍結した run と cursor を行に保存する（`to` = 次の状態・None は状態を変えない）。行が無ければ何もしない。
/// `was` = Some((状態, cursor)) の時は、行が今もその状態・その cursor である事の読みと書き込みを 1 つの `BEGIN IMMEDIATE` で行い（別の接続・別の process の
/// 書き手も間に入れない）、違えば書かずに `Ok(false)`（INTEG wa1: 同じ run の resume の 2 本目は送らない）。
fn hold(
    run: &EngineRun,
    cursor: &WorkflowCursor,
    row: &mut Option<crate::runtime::receipt::RunRecord>,
    store: Option<&std::sync::Mutex<crate::storage::Store>>,
    to: Option<crate::domain::run::RunStatus>,
    was: Option<(crate::domain::run::RunStatus, &WorkflowCursor)>,
) -> Result<bool, EngineError> {
    let (Some(rec), Some(store)) = (row.as_mut(), store) else { return Ok(true) };
    rec.input = frozen(run)?;
    rec.workflow = Some(cursor.clone());
    if let Some(to) = to {
        crate::runtime::receipt::set_status(rec, to).map_err(|e| EngineError::Store(format!("{e:?}")))?;
    }
    let Some((status, seen)) = was else { return keep(store, rec).map(|_| true) };
    let err = |e: String| EngineError::Store(format!("Can't record the run ({e})"));
    let st = store.lock().unwrap_or_else(|e| e.into_inner());
    st.immediate(
        |e| err(format!("{e:?}")),
        |st| {
            let now = crate::runtime::receipt::load(st, &rec.run.id).map_err(|e| err(format!("{e:?}")))?;
            if !now.is_some_and(|r| r.run.status == status && r.workflow.as_ref() == Some(seen)) {
                return Ok(false);
            }
            crate::runtime::receipt::save(st, rec).map_err(|e| err(format!("{e:?}")))?;
            Ok(true)
        },
    )
}

/// Execution の段の本体（新しい run と `resume` の両方）: `cursor` の所（操作 `plan` の手 `next`）から走らせる。`outputs` = 今の操作の済んだ手の出力
/// （`resume` が読み直した物・新しい run は空）。行が在れば手ごとに write-ahead（送る前に attempt を保存・結果を保存）し、判断で止まる（Decide）・
/// 承認を待つ（その exact payload の承認が無い・前の書く手が走っていても cursor から続けられる）時は cursor と card を保存して待つ状態にする（WF1・WF2）。
/// 行が無い時の止まり方は今まで通り（前の手が手元を読むだけなら Denied・書いた手が在れば Stopped）。戻り = 操作ごとの 1 行。
fn execute(run: &mut EngineRun, cursor: &mut WorkflowCursor, mut outputs: HashMap<String, Value>, row: &mut Option<crate::runtime::receipt::RunRecord>, cx: &mut EngineContext) -> Result<Vec<String>, EngineError> {
    use crate::domain::run::{Attempt, RunStatus};
    let actions = cx.registry.actions();
    // 手元を読むだけの action（外の service を読む手は、問い合わせ自体が外へ出るので数えない）。止められた手より前と、前の操作で走った手が
    // 全部これの時だけ、元の物は何も変わっておらず外へも何も送っていない = Denied
    let local_reads: HashSet<&str> = actions.iter().filter(|a| a.effect == Effect::LocalRead).map(|a| a.id.as_str()).collect();
    let store = cx.runs.as_ref().map(|r| r.store);
    let mut ran = Vec::new();
    while cursor.plan < run.plans.len() {
        let i = cursor.plan;
        let wf = run.plans[i].workflow.clone();
        // ENGINE-GENERIC-1101: 手の `{"$from": "situation.…"}` が指す状況の file（操作ごとに outputs を空にした後も）
        outputs.insert(SITUATION.to_owned(), situation_payload(cx.situation));
        // 次の操作が止められた時に「何も変えていない」と言わないため、前の操作で走った手を持ち越す
        let earlier: Vec<&WorkflowStep> = run.plans[..i].iter().flat_map(|p| &p.workflow.steps).filter(|s| s.is_action()).collect();
        let (ran_reads, changed) = (earlier.len(), earlier.iter().any(|s| !local_reads.contains(s.capability.as_str())));
        let off: usize = run.plans[..i].iter().map(|p| p.workflow.steps.len()).sum();
        let had = run.plans[i].done.len();
        let receipts = &mut run.plans[i].done;
        let (repo, own) = (cx.repo, cx.document_file);
        let outside = |i: &Value| outside_repo(i, repo, own);
        // ENGINE-AUDIT W1: 手ごとの 1 行（送る直前 = write-ahead の時）。closure は cx を借りないよう progress だけを写す
        let (progress, total) = (cx.progress, wf.steps.len());
        let got = run_steps(&wf, &mut *cx.registry, cx.caller, cx.consents, &mut outputs, cursor.next, &outside, cx.stop, &mut |k, r| {
            if let (None, Some(p)) = (r, progress) {
                p(Some(format!("Sending step {} of {total}…", k + 1)));
            }
            // 走った手（Verified / Unverified）だけを済んだ手に。gate に止められた手（Rejected）は送っていないので、続きはその手から・記録に残さない
            let ran = r.filter(|r| matches!(r.outcome, Outcome::Verified { .. } | Outcome::Unverified { .. }));
            cursor.next = k + usize::from(ran.is_some());
            if let Some(r) = ran {
                receipts.push(r.clone());
            }
            let (Some(rec), Some(store)) = (row.as_mut(), store) else { return Ok(()) };
            let now = crate::runtime::now_rfc3339();
            // r7 ①: 文脈の手の後に作り直した手順は、run を開いた時の記録の手の列より多くなり得る = 記録に無い手は書かない（panic しない）
            if let Some(step) = rec.run.steps.get_mut(off + k) {
                match r {
                    None => step.attempts.push(Attempt { seq: step.attempts.len() as u32 + 1, started_at: now, finished_at: None, outcome: None }),
                    Some(r) => {
                        if let Some(a) = step.attempts.last_mut() {
                            (a.outcome, a.finished_at) = (Some(r.outcome.clone()), Some(now));
                        }
                    }
                }
            }
            rec.workflow = Some(cursor.clone());
            keep(store, rec).map_err(|e| e.to_string())
        });
        // ENGINE-AUDIT W7: reveal の手を Adapter が送る直前に承認の要る手と分けた（押していない）= その手を `CLICK_ACTION` に書き換えて（`run.plans[i]` = 待つ時に
        // 保存する手順・承認の後の resume はその click から）、`Rejected` の腕と同じ承認の card（`pending` = 書き換えた手・`later`）。row が無ければ今まで通りの止め方。
        // その手より後ろの reveal も click に（`reveal_where_known` の「1 手でも承認が要れば全部 click」を送る時にも = 承認の間に閉じる menu の項目を
        // 2 枚目の card にしない・sheet の中の popup の開く手が当たった時は、その menu の項目も同じ 1 枚の card に入る）
        let (wf, got) = match got {
            Err(WorkflowError::Asks { step, why }) => {
                for s in run.plans[i].workflow.steps.iter_mut().skip_while(|s| s.id != step).filter(|s| s.capability == REVEAL_ACTION) {
                    s.capability = CLICK_ACTION.into();
                }
                (run.plans[i].workflow.clone(), Err(WorkflowError::Rejected { step, why }))
            }
            got => (wf, got),
        };
        let receipt = match got {
            Ok(r) => r,
            Err(WorkflowError::Waiting { step }) => {
                let d = decision(&wf, &step, &outputs).map_err(EngineError::Generation)?;
                // 止まった時の元データ（続ける前に読み直して比べる）。読めなければ操作の前の読み
                let seen = read_checks(&wf.checks, cx).map_or_else(|_| seen_of(&wf.checks, &run.plans[i].before), |now| seen_of(&wf.checks, &now));
                let kept = settled(&wf, &step, &outputs, cx);
                wait_at(cursor, &wf, &step, &kept, &actions, seen);
                run.asked = Some(Asked::Decide(d.clone()));
                hold(run, cursor, row, store, Some(RunStatus::WaitingHuman), None)?;
                return Err(EngineError::Decide(d));
            }
            Err(WorkflowError::Rejected { step, why }) => {
                let ran_before: Vec<&WorkflowStep> = wf.steps.iter().take_while(|s| s.id != step).filter(|s| s.is_action()).collect();
                let wrote = changed || ran_before.iter().any(|s| !local_reads.contains(s.capability.as_str()));
                let n = ran_reads + ran_before.len();
                // ENGINE-ADJUST-1102 ①: 人の Stop で止めた手（`run_steps` が送る前に見た・その手と後ろは送っていない）。承認の card は出さない・row は `close_row` が Cancelled に。
                // 書く手が走った後なら G74c と同じ文で走った数を言い（`Stopped`）、読む手だけ・0 手なら 1 手も送っていない 1 行
                if why == STOPPED_WHY {
                    return Err(if wrote { EngineError::Stopped { step, why, ran: n } } else { stopped() });
                }
                let (first, rest) = (pending(&wf, &step, &outputs, &actions), later(&wf, &step, &outputs, &actions));
                // WF2: 承認の要る手で、その exact payload の承認がまだ無い = 承認を待つ（承認したのに止められた = 人の lease・許可は今まで通り）
                let asks = first.as_ref().filter(|p| !consent_for(cx.consents, &p.action, &p.input).is_some_and(|c| c.payload_digest.as_deref() == Some(p.digest.as_str())));
                if let (Some(p), true) = (asks.cloned(), row.is_some()) {
                    let seen = if wrote { read_checks(&wf.checks, cx).map_or_else(|_| seen_of(&wf.checks, &run.plans[i].before), |now| seen_of(&wf.checks, &now)) } else { seen_of(&wf.checks, &run.plans[i].before) };
                    let kept = settled(&wf, &step, &outputs, cx);
                    wait_at(cursor, &wf, &step, &kept, &actions, seen);
                    run.asked = Some(Asked::Approve { steps: std::iter::once(p.clone()).chain(rest.iter().cloned()).collect() });
                    hold(run, cursor, row, store, Some(RunStatus::WaitingApproval), None)?;
                    let why = if wrote || n == 0 { why } else { format!("{why} · the first {} only read, and left the originals unchanged", crate::domain::plural(n, "step")) };
                    return Err(EngineError::Denied(Denial { why, pending: Some(p), later: rest, searched: Searched::default(), ran: 0 }));
                }
                if wrote {
                    // G74c: 走った手の後で止めた = 3 つ目の文（その手と後ろは送っていない・前の手は走った）。確かめられない（Verification）とは言わない
                    return Err(EngineError::Stopped { step, why, ran: n });
                }
                let why = if n == 0 { why } else { format!("{why} · the first {} only read, and left the originals unchanged", crate::domain::plural(n, "step")) };
                return Err(EngineError::Denied(Denial { why, pending: first, later: rest, searched: Searched::default(), ran: 0 }));
            }
            // ENGINE-AUDIT W5（節 1・8・11・owner 206「探してから聞く」・spec 189）: 押す名前が今の木に無い・1 つに絞れない（Adapter は押していない）= run を終えず探す段へ
            // （読み直す → 梯子 → AI の次の 1 手 → 走らせる → 目当てを今の木で照らす）。Ok = 同じ手（`cursor.next`）からやり直す。止めるのは `search` の 3 つ
            Err(WorkflowError::Unfound { step, label, ambiguous }) => {
                // r10 ②（P0）: 探す段が止めた時、この run で走った手（前の操作の手・この操作の済んだ手・探す段で足した手）が在れば「何も送っていない」と言わない =
                // `Rejected` の腕と同じ数え方。書く手が走っていれば `Denial::ran`（探した跡は残す）・読む手だけなら「the first N only read」・承認の card（pending）はそのまま
                if let Err(e) = search(run, i, &step, &label, ambiguous, &mut outputs, row, cx) {
                    let p = &run.plans[i];
                    let writes = |r: &StepReceipt| r.step.starts_with(SEARCH_STEP_PREFIX) || p.workflow.steps.iter().any(|s| s.id == r.step && !local_reads.contains(s.capability.as_str()));
                    let n = ran_reads + p.done.len();
                    let wrote = changed || p.done.iter().any(writes);
                    return Err(match e {
                        EngineError::Denied(d) if d.pending.is_none() && wrote => EngineError::Denied(Denial { ran: n, ..d }),
                        EngineError::Denied(d) if d.pending.is_none() && n > 0 => {
                            EngineError::Denied(Denial { why: format!("{} · the first {} only read, and left the originals unchanged", d.why, crate::domain::plural(n, "step")), ..d })
                        }
                        e => e,
                    });
                }
                continue;
            }
            Err(WorkflowError::Missing { capability, .. }) => {
                let want = run.record.operations.get(i).map_or_else(String::new, |o| o.want.clone());
                return Err(EngineError::NoMeans { operation: want, tried: vec![format!("{capability} (can't be used now)")], searched: Searched::default() });
            }
            // G74: 送った手の結果が分からない = 「送れたか確かめられない・送り直さない」の文
            Err(WorkflowError::Step { step, why }) if why.starts_with(workflow::SENT_UNKNOWN) => return Err(EngineError::Unconfirmed(format!("Stopped at the step “{step}”: {why}"))),
            Err(WorkflowError::Step { step, why }) => return Err(EngineError::Verification(format!("Stopped at the step “{step}”: {why}"))),
            // 送る直前に読んで見えた事（Adapter の 1 行）を持たせる（Page の 1 行が「別の場所」等を言う・FX-115）
            Err(WorkflowError::Held { why, .. }) => return Err(EngineError::NotSent(why)),
            Err(WorkflowError::Invalid(why)) => return Err(EngineError::Generation(why)),
            Err(WorkflowError::Store(why)) => return Err(EngineError::Store(why)),
            // B76: 読む手が `$from` を埋めた後に repo の外を指した = その手と後ろは走らせていない。1 手も走っていなければ計画の段と同じ 1 行・走った手が在れば数を言う
            Err(WorkflowError::Outside { step }) => {
                let n = ran_reads + wf.steps.iter().take(cursor.next).filter(|s| s.is_action()).count();
                if n == 0 {
                    return Err(EngineError::Denied(Denial::from(OUTSIDE_REPO)));
                }
                return Err(EngineError::Stopped { step, why: OUTSIDE_STEP.into(), ran: n });
            }
            // WF-006: 外へ効く手の宛先を送る直前に読み直せない Adapter = その手から後ろは 1 手も送っていない。門は操作ごと = 前の操作（と resume の前）で
            // 外へ効く手が走っていれば「nothing was sent」と言わず、走った手の数を言う（WF-E x1・Rejected の腕と同じ数え方）
            Err(WorkflowError::Unchecked { step, why }) => {
                let ran_here: Vec<&WorkflowStep> = wf.steps.iter().take(cursor.next).filter(|s| s.is_action()).collect();
                if !changed && ran_here.iter().all(|s| local_reads.contains(s.capability.as_str())) {
                    return Err(EngineError::Denied(Denial::from(why)));
                }
                let n = ran_reads + ran_here.len();
                let ran = if n == 1 { "Only the first step ran".to_string() } else { format!("Only the first {n} steps ran") };
                let why = format!("Shibubu has no way to check where the step “{step}” would go right before sending it, so it and the steps after it weren't sent. {ran}. Look inside the original app");
                return Err(EngineError::Denied(Denial::from(why)));
            }
            // W7: 上の `match`（`run.plans[i]` を click に書き換える所）が `Rejected` に替えたので、ここには来ない。型の網羅のための腕（panic しない・押していない 1 行）
            Err(WorkflowError::Asks { why, .. }) => return Err(EngineError::Denied(Denial::from(why))),
        };
        if wf.checks.is_empty() {
            let caps: Vec<&str> = wf.steps.iter().filter(|s| s.is_action()).map(|s| s.capability.as_str()).collect();
            return Err(EngineError::Verification(format!("Ran {}, but Shibubu has no way to read the result back, so it can't tell if it worked. Check in the app yourself", caps.join("・"))));
        }
        // 条件に合うまで読む action だけを読み直す（`readback` まで・書く手はもう一度送らない）。合わないまま時間が来たら最後の読みの理由で Verification
        // r7 ①: 文脈の手だけの手順は、窓が替わらなければ 90 秒待たずに短く諦める（替わらない = 同じ木 → 次の周で止まる）
        let deadline = Instant::now() + if run.plans[i].context { cx.readback.min(CONTEXT_READBACK) } else { cx.readback };
        let mut gap = READBACK_STEP;
        // この操作に書く手が在るか（読み戻しの途中で止める時の文を分ける: 書いた後 = 送れたか分からない・読むだけ = 1 手も送っていない）
        let wrote_any = wf.steps.iter().any(|s| actions.iter().any(|a| a.id == s.capability && !is_read(a.effect)));
        let mut saying = false;
        // ENGINE-AUDIT W1: 読み戻しの段の 1 行（合えばすぐ消える）と、合うまでの ms（Execution の数字）
        let started = Instant::now();
        if let Some(say) = cx.progress {
            let app = wf.checks.iter().find_map(|c| c.input["app_name"].as_str()).map(|a| format!(" in {a}")).unwrap_or_default();
            say(Some(format!("Checking the result{app}… (up to {} s)", cx.readback.as_secs())));
            saying = true;
        }
        let lines = loop {
            // G74 戻し: 読めない（read の失敗）も合わないと同じに時間まで読み直す（書いた後なら Unconfirmed）
            let each = read_checks(&wf.checks, cx).map(|after| {
                wf.checks
                    .iter()
                    .zip(&run.plans[i].before)
                    .zip(&after)
                    .map(|((c, b), a)| expected(c, &outputs).map_err(EngineError::Verification).and_then(|c| verify(&c, Some(b), a)))
                    .collect::<Vec<_>>()
            });
            // G81: 窓（他の読み）は合ったが保存した file の読みだけがまだ合わない = 保存が disk に着くのを待っている間の 1 行（済みとは出さない）
            if let (Some(say), Ok(each)) = (cx.progress, &each) {
                if each.iter().any(Result::is_err) && wf.checks.iter().zip(each).all(|(c, r)| r.is_ok() || c.action == SAVED_READ) {
                    let app = wf.checks.iter().find_map(|c| c.input["app_name"].as_str()).unwrap_or("the app");
                    say(Some(format!("Changed in {app} · checking the saved file…")));
                    saying = true;
                }
            }
            let got: Result<Vec<String>, EngineError> = each.and_then(|each| each.into_iter().collect());
            let late = Instant::now() >= deadline;
            if let (true, Some(say)) = (saying && (got.is_ok() || late), cx.progress) {
                say(None);
            }
            match got {
                Ok(lines) => break lines,
                // G74: 書く手を送った後に読み戻しが合わない = 送れたか確かめられない（送り直さない）。読むだけの手順は今まで通り
                Err(EngineError::Verification(why)) if late && wrote_any => return Err(EngineError::Unconfirmed(why)),
                Err(e) if late => return Err(e),
                // ENGINE-ADJUST-1102 ①: 読み戻しの待ちの間の人の Stop（読み直さない・書いた後なら送れたかが分からない = 送り直さない・読むだけなら 1 手も送っていない）
                Err(_) if cx.stop.load(Ordering::SeqCst) => return Err(if wrote_any { EngineError::Unconfirmed(STOPPED_UNCHECKED.into()) } else { stopped() }),
                // ENGINE-AUDIT W7（spec 261「読み戻しは backoff」）: 1 → 2 → 4 秒（4 秒で頭打ち・時間の終わりを越えて眠らない = 終わりにもう 1 回読む）
                Err(_) => {
                    std::thread::sleep(gap.min(deadline.saturating_duration_since(Instant::now())));
                    gap = (gap * 2).min(READBACK_STEP * 4);
                }
            }
        };
        run.plans[i].readback_ms = started.elapsed().as_millis() as u64;
        // WF-007: 字を置く書く手の「頼んだ差分」を読み戻していなければ Partial（読み戻しは合ったが済みと出さない）
        if run.record.partial.is_none() {
            run.record.partial = unasked(&wf, &actions);
        }
        let steps: Vec<StepReceipt> = run.plans[i].done[..had].iter().cloned().chain(receipt.steps).collect();
        let handed: usize = steps.iter().map(|s| s.handoffs.len()).sum();
        ran.push(format!("{}: {} steps · handed over {handed}", receipt.workflow, steps.len()));
        run.record.verified.extend(lines.iter().cloned());
        run.plans[i].lines = lines;
        run.record.receipts.push(WorkflowReceipt { workflow: receipt.workflow, steps });
        // r7 ①（梯子の段 0）: 文脈の手だけの手順が通った = 窓が替わった → 同じ操作を読み直して generate し直し、同じ i を続ける（止めるのは同じ木 2 回・天井 SEARCH_STEPS_MAX・AI が手を返さない）
        if run.plans[i].context {
            regenerate(run, i, cx)?;
            (cursor.next, cursor.waiting) = (0, None);
            cursor.outputs.clear();
            outputs = HashMap::new();
            continue;
        }
        (cursor.plan, cursor.next, cursor.waiting) = (i + 1, 0, None);
        cursor.outputs.clear();
        outputs = HashMap::new();
    }
    Ok(ran)
}

/// r7（operator 11-03 ①・梯子の段 0）: 文脈の手（`Generated::context` の 1 手）が走って読み戻しが通った後、同じ操作を読み直して generate し直す（`execute` の中）。
/// 読み直した木が前と同じなら止める（`RunPlan::tree`）・`RunPlan::searches` の天井 `SEARCH_STEPS_MAX`・generate の AI の枠は改めて `GENERATE_CALLS_MAX`。
/// 新しい手順の読み戻しの前の値（`before`）はここで読む・`done` の Receipt（文脈の手）は残す・記録の手の列は増やさない（`execute` の mark が無い手を書かない）。
/// Err = generate の NoMeans（`Searched` つき）・Denied（同じ木・天井）・人の Stop
fn regenerate(run: &mut EngineRun, i: usize, cx: &mut EngineContext) -> Result<(), EngineError> {
    check_stop(cx)?;
    let op = run.record.operations.get(i).cloned().ok_or_else(|| EngineError::Generation(format!("The plan {i} has no operation")))?;
    let app = op.target.clone().unwrap_or_else(|| "the app".into());
    let p = &mut run.plans[i];
    p.searches += 1;
    if p.searches > SEARCH_STEPS_MAX {
        return Err(EngineError::Denied(Denial { why: format!("Looked for a way to “{}” in {app} with {SEARCH_STEPS_MAX} extra steps without finding it, so Shibubu stopped", op.want), searched: p.searched.clone(), ..Default::default() }));
    }
    say(cx, "Reading the window again after that step…");
    let f = recipe::facts(&op, cx)?;
    let tree = tree_id(&f.names);
    let p = &mut run.plans[i];
    if !p.tree.is_empty() && p.tree == tree {
        return Err(EngineError::Denied(Denial { why: format!("Looked for a way to “{}” in {app}, but the window didn't change after that step, so Shibubu stopped", op.want), searched: p.searched.clone(), ..Default::default() }));
    }
    p.tree = tree;
    // 押した文脈の手を覚える（AI に「同じ行を繰り返さない」・止まった 1 行の「試した事」）
    let pressed: Vec<String> = p.workflow.steps.iter().map(|s| format!("{} {}", s.capability, s.input["label"].as_str().or_else(|| s.input["path"].as_array().and_then(|a| a.last()).and_then(Value::as_str)).unwrap_or_default()).trim().to_owned()).collect();
    let next = p.searched.next.take();
    let mut tried = std::mem::take(&mut p.searched.tried);
    tried.extend(pressed);
    p.searched = recipe::searched_from(&f.names, &f.words, next);
    p.searched.tried = tried;
    let hint = format!("Already pressed to reveal more (don't press these again; pick a different row or path): {}", p.searched.tried.join(" · "));
    // 候補は discover と同じ（registry・保存した能力・名指した MCP の tools）
    let said: Vec<&str> = std::iter::once(run.record.goal.as_str()).chain([op.want.as_str(), op.target.as_deref().unwrap_or("")]).collect();
    let found = discover_with(cx, &mcp_records(cx.mcp, &said));
    cx.generate_calls = 0;
    let g = recipe::generate(&op, &found, cx, Some(&hint))?;
    let mut wf = g.workflow;
    (wf.id, wf.goal) = (workflow_id(&run.record.goal, &op), run.record.goal.clone());
    let before = recipe::read_checks(&wf.checks, cx)?;
    let p = &mut run.plans[i];
    p.before = before;
    p.context = g.context;
    p.workflow = wf;
    p.lines.clear();
    Ok(())
}

/// ENGINE-AUDIT W5: 探す段（`execute` の `Unfound` の腕・spec の「探しながら進む loop」と「探しの梯子」の 1〜3 段。4 = 口の棚は W4 で generate の input に・5 = 絵の OCR は W8）。
/// 1 回の呼び出し = 1 周: ① 宛先の窓を読み直す（`recipe::facts`・`Facts::names` = 今の木の全部の名前）② 目当て（`label`）が今の木にちょうど 1 つ在れば Ok（何も足さず同じ手からやり直す）
/// ③ 無ければ梯子 = 語と役割で引き直す（`gui::matches`・同じ名前が複数なら道〈`Control::path`〉で 1 つに = `within`）→ 押し方を替える（bridge の `ax_press` の `ladder`）→
/// menu bar（`MENU_READ_ACTION`）の順 ④ それでも無ければ AI に次の 1 手だけ（`recipe::next_step` = 既に在る generate の答えの形で steps 1 個）→ その手を 1 手の手順として
/// `run_steps` で走らせる（承認・同意・act の記録は本物の手と同じ・`cursor` は動かさない・走った手の Receipt は `RunPlan::done` に積む）→ Ok（呼ぶ側が同じ手からやり直す）。
/// 止めるのは 3 つ（spec 189）: 人の Stop（`stopped()`・走った後なら `Stopped`）・同じ木が 2 回続いた（`RunPlan::tree` と同じ印 = 進んでいない）・AI が手を返さない。
/// どれも `Denied` に `Searched`（探した語・近い名前 3 つ・名前の付いた数・押せる次の一手）を付けて止める。天井（ponytail: 木が毎回変わり続ける app で回り続けない）= `SEARCH_STEPS_MAX`。
/// 保証する: 目当てが在る時は何も送らない・AI の手が承認の要る手で承認が無ければ `Denied { pending }`（card・その手は送らない）・app の名前の枝は無い・読む手も押す手も Adapter の門を通る。
/// 保証しない: 目当てが出た後の押しが効く事（それは `execute` の読み戻し）。
/// 例（`execute`）: `Err(WorkflowError::Unfound { step, label, ambiguous }) => { search(run, i, &step, &label, ambiguous, &mut outputs, row, cx)?; continue }`
fn search(run: &mut EngineRun, i: usize, step: &str, label: &str, ambiguous: bool, outputs: &mut HashMap<String, Value>, row: &mut Option<crate::runtime::receipt::RunRecord>, cx: &mut EngineContext) -> Result<(), EngineError> {
    // 記録の手（`RunRecord::run.steps`）は増やさない: 足した手の Receipt は `RunPlan::done` に積む・resume は探す段の途中を持たない（承認は同じ一言の run のし直し）
    let _ = row;
    check_stop(cx)?;
    let op = run.record.operations.get(i).cloned().ok_or_else(|| EngineError::Generation(format!("The plan {i} has no operation")))?;
    say(cx, format!("Looking for “{label}”… reading the window again"));
    // ① 読み直す（窓の読みは Adapter の門を通る読む手・何も送らない）
    let f = recipe::facts(&op, cx)?;
    let app = f.windows.first().map_or_else(|| op.target.clone().unwrap_or_else(|| "the app".into()), |w| w.app_name.clone());
    let p = &mut run.plans[i];
    let tried = std::mem::take(&mut p.searched.tried);
    p.searched = recipe::searched_from(&f.names, &crate::gui::words_of(&[label]), p.searched.next.take());
    p.searched.tried = tried;
    let denied = |p: &RunPlan, why: String| EngineError::Denied(Denial { why, searched: p.searched.clone(), ..Default::default() });
    // 止め 2: 同じ木が 2 回続いた = 前の手で何も進んでいない（印は名前の全部の hash）
    let tree = tree_id(&f.names);
    if !p.tree.is_empty() && p.tree == tree {
        return Err(denied(p, format!("Looked for “{label}” in {app}, but the window didn't change after the last step, so Shibubu stopped")));
    }
    p.tree = tree;
    // ② 目当てが今の木に在るか（押せる形 = AXPress か一覧の行・押せない形 = 梯子で押す・2 つ以上 = 道で絞る）
    let want = label.to_lowercase();
    let hits: Vec<&crate::gui::Control> = f.state.as_ref().map(|s| s.controls.iter().filter(|c| c.label.to_lowercase().contains(&want)).collect()).unwrap_or_default();
    let pressable = hits.iter().filter(|c| crate::gui::pressable(&c.role, &c.actions, "AXPress")).count();
    match (pressable, hits.len()) {
        // 今は在ってちょうど 1 つ（前の手で出た・絞れた）= そのままやり直す（Adapter の数え方と違えば Unfound → 同じ木で止まる）
        (1, _) => return Ok(()),
        // ③ 梯子: 名前は在るが押せる形でない（見出し・子の字に名前がある button・自前の tile）= bridge の梯子で押す（1 回だけ・効かなければ同じ木で止まる）
        (0, n) if n >= 1 && !ambiguous => {
            if let Some(s) = p.workflow.steps.iter_mut().find(|s| s.id == step).filter(|s| s.input["ladder"] != true) {
                s.input["ladder"] = json!(true);
                return Ok(());
            }
        }
        _ => {}
    }
    // 天井
    if p.searches >= SEARCH_STEPS_MAX {
        return Err(denied(p, format!("Looked for “{label}” in {app} with {SEARCH_STEPS_MAX} extra steps without finding it, so Shibubu stopped")));
    }
    // ④ AI に次の 1 手だけ（既に在る generate の答えの形・門は `next_step`）。r10 ①: この操作で走った手も tried に（AI が同じ手を押し直す手を返さない）
    let mut searched = p.searched.clone();
    let shown_of = |s: &WorkflowStep| format!("{} {}", s.capability, s.input["label"].as_str().unwrap_or_default()).trim().to_owned();
    let ran_here: Vec<&WorkflowStep> = p.workflow.steps.iter().filter(|s| p.done.iter().any(|r| r.step == s.id)).collect();
    for line in ran_here.iter().map(|s| shown_of(*s)) {
        if !searched.tried.contains(&line) {
            searched.tried.push(line);
        }
    }
    let next = recipe::next_step(&op, &[], &f, label, &mut searched, cx)?;
    p.searched = searched;
    // r10 ①（dogfood r10: 探す段の AI が走った手〈track を選ぶ〉をもう一度返し、その承認は使用済みで止まった）: 同じ action・同じ input の手は承認した「1 回」の外 =
    // 送らずに止める（押し直しても窓は同じ・承認の card も出さない）。押す手は click と reveal を同じ手と見る（W7 で承認の要否だけが違う）
    let same = |s: &WorkflowStep, x: &WorkflowStep| (s.capability == x.capability || presses(&s.capability) && presses(&x.capability)) && s.input == x.input;
    if let Some(x) = next.as_ref().filter(|x| ran_here.iter().any(|s| same(*s, *x))) {
        let again = x.input["label"].as_str().unwrap_or_default().to_owned();
        return Err(denied(p, format!("Looked for “{label}” in {app}: the only step left was pressing “{again}” again, which already ran, so Shibubu stopped")));
    }
    let Some(mut x) = next else {
        let why = if hits.len() >= 2 {
            format!("Looked for “{label}” in {app}, but {} things in the window have that name and Shibubu couldn't tell which, so nothing was pressed", hits.len())
        } else {
            format!("Looked for “{label}” in {app}, but it isn't in the window and nothing on the screen leads to it, so nothing was pressed")
        };
        return Err(denied(p, why));
    };
    // 同じ名前を道や役割で絞った手 = 元の手の input を置き換えてやり直す（手は足さない）
    if presses(&x.capability) && x.input["label"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(label)) {
        if let Some(s) = p.workflow.steps.iter_mut().find(|s| s.id == step) {
            s.input = x.input;
        }
        p.searches += 1;
        return Ok(());
    }
    // 足す手を 1 手の手順として走らせる（承認・同意・act の記録・人の Stop は本物の手と同じ門・`cursor` は動かさない）
    p.searches += 1;
    x.id = format!("{SEARCH_STEP_PREFIX}{}", p.searches);
    let shown = x.input["label"].as_str().or_else(|| x.input["path"].as_array().and_then(|a| a.last()).and_then(Value::as_str)).unwrap_or_default().to_owned();
    p.searched.tried.push(format!("{} {shown}", x.capability).trim().to_owned());
    let wf1 = Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: format!("{}.search", p.workflow.id), goal: p.workflow.goal.clone(), ui: None, steps: vec![x.clone()], checks: Vec::new() };
    let actions = cx.registry.actions();
    let (repo, own) = (cx.repo, cx.document_file);
    let outside = |i: &Value| outside_repo(i, repo, own);
    let progress = cx.progress;
    let mut receipts: Vec<StepReceipt> = Vec::new();
    let got = run_steps(&wf1, &mut *cx.registry, cx.caller, cx.consents, outputs, 0, &outside, cx.stop, &mut |_, r| {
        match r {
            None => {
                if let Some(say) = progress {
                    say(Some(format!("Pressing “{shown}” to find “{label}”…")));
                }
            }
            Some(r) => receipts.push(r.clone()),
        }
        Ok(())
    });
    p.done.extend(receipts.into_iter().filter(|r| matches!(r.outcome, Outcome::Verified { .. } | Outcome::Unverified { .. })));
    match got {
        Ok(_) => Ok(()),
        // 止め 1: 人の Stop（走った手が在れば数を言う）
        Err(WorkflowError::Rejected { why, .. }) if why == STOPPED_WHY => Err(if p.done.is_empty() { stopped() } else { EngineError::Stopped { step: x.id, why, ran: p.done.len() } }),
        // 承認の要る手で承認が無い = card（run は同じ一言のし直し）・人の lease 等は理由の 1 行に探した跡を添える
        Err(WorkflowError::Rejected { why, .. }) => {
            let pend = pending(&wf1, &x.id, outputs, &actions).filter(|pa| !consent_for(cx.consents, &pa.action, &pa.input).is_some_and(|c| c.payload_digest.as_deref() == Some(pa.digest.as_str())));
            Err(EngineError::Denied(Denial { why, pending: pend, later: Vec::new(), searched: p.searched.clone(), ran: 0 }))
        }
        // W7: 探す手（reveal）が送る直前に承認の要る手と分かった（押していない）= click としての承認の card（承認の後の run は `run_steps` が同じ input の click の承認で送る）
        Err(WorkflowError::Asks { why, .. }) => {
            let wf1 = Workflow { steps: vec![WorkflowStep { capability: CLICK_ACTION.into(), ..x.clone() }], ..wf1 };
            Err(EngineError::Denied(Denial { why, pending: pending(&wf1, &x.id, outputs, &actions), later: Vec::new(), searched: p.searched.clone(), ran: 0 }))
        }
        Err(WorkflowError::Unfound { label: l, .. }) => Err(denied(p, format!("Looked for “{label}” in {app}: the step to reveal it (“{l}”) isn't in the window either, so Shibubu stopped"))),
        Err(WorkflowError::Missing { capability, .. }) => Err(denied(p, format!("Looked for “{label}” in {app}, but the step to reveal it needs {capability}, which can't be used now"))),
        Err(e) => Err(EngineError::Verification(format!("Stopped while looking for “{label}” in {app}: {e}"))),
    }
}
/// W5: 探す段の天井 = 1 つの操作で AI に頼む次の 1 手の数（超えたら `Denied` に `Searched`）
const SEARCH_STEPS_MAX: usize = 8;
/// r7 ①: 文脈の手だけの手順の読み戻しの上限（主窓の要素が変わるのを待つ時間・製品の 90 秒より短く）
const CONTEXT_READBACK: Duration = Duration::from_secs(10);
/// W5: 探す段で足した手の id の頭（本物の手の id と重ねない・`RunPlan::done` の Receipt で見分ける）
const SEARCH_STEP_PREFIX: &str = "search-";
/// W5: 木の印（`Facts::names` の hash・同じ木が 2 回続いたら止める）
fn tree_id(names: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 1 つの操作の途中の状態（`run` の中だけ）。
struct Plan {
    /// r7 ①: 文脈の手だけの手順（`Generated::context`）= 保存しない・execute が走らせた後に `regenerate`
    context: bool,
    op: Operation,
    found: Vec<CapabilityRecord>,
    choice: Option<Choice>,
    workflow: Option<Workflow>,
    recipe: Option<Recipe>,
    trial: Option<Trial>,
    reused: bool,
    /// 保存した手順・Adapter・試験を、保存した試験で確かめ直して使う（版・OS・権限が変わった・生成しない）
    recheck: bool,
    /// model 無しの手元の組み合わせ（`alone`・保存しない・読み戻しが無いので済みと出さない）
    alone: bool,
    /// `workflow.checks` を操作の前に読んだ物（同じ順）
    before: Vec<Observation>,
    /// 確かめた根拠（試験と本物の読み戻しの行）。`save_record` の evidence
    evidence: Vec<String>,
    /// W19-3: 解析してよい対象（承認がまだ無い）。生成の model が手段無しと答えた時だけ、これの解析の承認を求める
    ask: Option<AnalysisTarget>,
    /// ENGINE-AUDIT W2: 作り直す理由の 1 行（保存した手順が本物で失敗した・最初の押す名前が今の木に無い）。generate の `hint` → input の `rejected_plan`
    hint: Option<String>,
}

fn done(note: String) -> StageOutcome {
    StageOutcome::Done { note }
}

/// G76: `cx.trace` の時だけ、段の 1 行を `<data_dir>/TRACE_FILE` に足す（{at〈RQ3・RFC3339〉, run_id, stage, result: "ok" | "stop", stop_reason?: "<EngineError の kind>: <scrub した 1 行>",
/// ms〈`since` から〉} ＋ `detail` の key・構造の field は切らない）。`detail` の字（利用者の言葉・model の答え）は呼ぶ側が `scrub` / `trace_raw` を通す。
/// 書けなくても run は止めない（QA の記録）。
/// Wave: M2
/// 例: `trace(cx, "target", t, None, json!({ "windows_status": "ready", "windows_count": 1 }));`
fn trace(cx: &EngineContext, stage: &str, since: Instant, stop: Option<&EngineError>, detail: Value) {
    if !cx.trace {
        return;
    }
    // RQ3: `at` = 書いた時刻（History の保存期間で古い行から消す・`generated::prune_jsonl`）
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut row = json!({ "at": crate::agent::rfc3339_from_secs(now), "run_id": cx.caller.run_id, "stage": stage, "result": if stop.is_some() { "stop" } else { "ok" }, "ms": since.elapsed().as_millis() as u64 });
    if let Some(e) = stop {
        // kind = 腕の名前（Debug の頭）・1 行 = 画面の文（Display）を scrub
        let kind = format!("{e:?}");
        row["stop_reason"] = json!(format!("{}: {}", kind.split(['(', ' ', '{']).next().unwrap_or_default(), scrub(&e.to_string())));
    }
    if let (Some(r), Value::Object(d)) = (row.as_object_mut(), detail) {
        r.extend(d);
    }
    // ponytail: run ごとに file を開き直す（段は 6 つ・QA の時だけ）
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(cx.data_dir.join(TRACE_FILE)).and_then(|mut f| f.write_all(format!("{row}\n").as_bytes()));
}

/// 鍵らしい物を伏せた時に書く字（trace）。
const HIDDEN: &str = "(hidden: looks like a key or token)";

/// home の path を `~` に（trace に利用者の名前を書かない）。
fn home_less(s: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if h.len() > 1 => s.replace(&h, "~"),
        _ => s.to_string(),
    }
}

/// G76: trace に書く 1 行から、鍵らしい物（`looks_secret`）・home の path・“” で包んだ物（窓の題名・書類の名前・利用者の字 = Adapter と Engine の文の決まり）
/// と "" で包んだ物（serde の誤りの値）を外す（中は字数だけ残す）。
fn scrub(s: &str) -> String {
    if looks_secret(s) {
        return HIDDEN.into();
    }
    let mask = |s: &str, open: char, close: char| {
        let (mut out, mut inside) = (String::new(), None);
        for c in s.chars() {
            match inside {
                None if c == open => {
                    out.push(c);
                    inside = Some(0);
                }
                None => out.push(c),
                Some(n) if c == close => {
                    out.push_str(&format!("{n} chars{close}"));
                    inside = None;
                }
                Some(n) => inside = Some(n + 1),
            }
        }
        if let Some(n) = inside {
            out.push_str(&format!("{n} chars"));
        }
        out
    };
    mask(&mask(&home_less(s), '“', '”'), '"', '"')
}

/// G76: 窓の題名の代わりの匿名 id（trace の target 行・owner の review 3 回目）: 起動ごとの塩（trace に書かない・`crypto::random`）と題名の sha256 の頭 8 字。
/// 同じ起動の中なら run をまたいでも同じ題名 = 同じ id（突き合わせられる）・塩が無いので辞書で題名に戻せない。題名が空（読めない）なら None。
fn title_id(title: &str) -> Option<String> {
    static SALT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let salt = SALT.get_or_init(|| crate::crypto::random(16).unwrap_or_else(|_| crate::model::new_id().to_le_bytes().to_vec()));
    (!title.trim().is_empty()).then(|| crate::crypto::sha256_hex(&[salt.as_slice(), title.as_bytes()].concat())[..8].to_string())
}

/// G76: model の生の答え（trace の plan 行）: 鍵らしければ伏せ・home を `~` に・`TRACE_CAP` byte まで（字の途中で切らない）。→ (書く字, 切ったか)
fn trace_raw(s: &str) -> (String, bool) {
    if looks_secret(s) {
        return (HIDDEN.into(), false);
    }
    let s = home_less(s);
    let mut end = s.len().min(TRACE_CAP);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), end < s.len())
}

/// 保存した手順の id（同じ目的・同じ操作の id なら同じ）。2 回目はこれで引く。
fn workflow_id(goal: &str, op: &Operation) -> String {
    format!("wf-{}-{}", &crate::crypto::sha256_hex(goal.trim().as_bytes())[..12], slug(&op.id))
}

/// id・dir の名前にできる形（小文字の英数字と `_` `-`。他は `_`）。
pub(crate) fn slug(s: &str) -> String {
    let t: String = s.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect();
    if t.is_empty() {
        "x".into()
    } else {
        t
    }
}

/// 確かめ済みか（Capability Registry の readiness が Ready = W19-1 の規則 1 つに従う）。
fn is_verified(r: &CapabilityRecord) -> bool {
    r.readiness() == CapabilityReadiness::Ready
}

fn is_verified_action(a: &RecipeAction) -> bool {
    matches!(a.verification, Verification::Verified { .. })
}

fn is_read(e: Effect) -> bool {
    matches!(e, Effect::LocalRead | Effect::ExternalRead)
}

/// 隔離した試験で流してよい効果（外へ書く・送る・消す物は写しで試せない）。
fn trialable(e: Effect) -> bool {
    matches!(e, Effect::LocalRead | Effect::LocalWrite | Effect::ExternalRead)
}

fn uses(wf: &Workflow, action: &str) -> bool {
    wf.steps.iter().any(|s| s.capability == action) || wf.checks.iter().any(|c| c.action == action)
}

/// 保存した手順を今使えるか: 手順と読み戻しの action が全部 registry で Ready（作った Adapter の action は確かめ済みの時だけ Ready）で、
/// 環境が変わって確かめ直しの印が付いた物が無い。
/// ENGINE-AUDIT W2: 本物で失敗した手順（`Workflow::fails` ≥ 1）は使わない（作り直す・前の失敗を hint に）。
fn usable(w: &Workflow, found: &[CapabilityRecord], registry: &AdapterRegistry) -> bool {
    let stale: HashSet<&str> = found.iter().filter(|r| r.constraints.iter().any(|c| c == REVERIFY)).map(|r| r.id.as_str()).collect();
    w.fails == 0
        && !w.checks.is_empty()
        && w.steps.iter().filter(|s| s.is_action()).map(|s| s.capability.as_str()).chain(w.checks.iter().map(|c| c.action.as_str())).all(|cap| {
            matches!(registry.tier(cap), DisplayTier::Ready | DisplayTier::Verified) && !stale.contains(cap)
        })
}

/// 手順の主な能力（最後の手 = 効果を持つ手）を選んだ理由つきで。
fn choice_of(op: &Operation, w: &Workflow, found: &[CapabilityRecord], reason: String) -> Choice {
    let cap = w.steps.iter().rfind(|s| s.is_action()).map_or_else(String::new, |s| s.capability.clone());
    let score = found.iter().find(|r| r.id == cap).map(|r| score(op, r)).unwrap_or_default();
    Choice { operation: op.id.clone(), capability: cap, reason, score }
}

/// model 無しの時の手元の組み合わせ（W13 の「AI 無しの手元の推定で続ける」の Engine 版）: 確かめ済みの能力がちょうど 1 つで（選ぶ曖昧さが無い）、入力が要らず
/// （input の schema に required が無い）、読むだけか承認の要る物（送る前に利用者が exact payload を見る）だけを、入力 `{}` の 1 手にする。
/// 承認の要らない書く操作は model 無しでは選ばない。読み戻しの仕方は決められないので checks は空（`run` は走らせた後も済みとは出さない）。
fn alone(goal: &str, op: &Operation, found: &[CapabilityRecord], actions: &[ActionDefinition]) -> Result<Workflow, EngineError> {
    // 反証 G-A（G100「見た手段の数」）: tried は見た能力だけ。model 無しで選べない理由は数に入れず log へ
    let no = |why: String| {
        eprintln!("Engine（model 無し）: {why}");
        // W5（節 8）: tried はこの操作に効く候補だけ（relevant）= git.* 等の直接の口を数えず、正直な文の腕（画面の手だけ・何も無い）に届く
        EngineError::NoMeans { operation: op.want.clone(), tried: tried(&found.iter().filter(|r| relevant(op, r)).cloned().collect::<Vec<_>>()), searched: Searched::default() }
    };
    let ready: Vec<&CapabilityRecord> = found.iter().filter(|r| is_verified(r)).collect();
    let [one] = ready.as_slice() else { return Err(no(format!("model が無いので、確かめ済みの能力 {} 件から選べない", ready.len()))) };
    let Some(a) = actions.iter().find(|a| a.id == one.id) else { return Err(no(format!("{} は今の registry に無い", one.id))) };
    if a.input_schema.get("required").and_then(Value::as_array).is_some_and(|r| !r.is_empty()) {
        return Err(no(format!("{} の入力を決める model が無い", a.id)));
    }
    if !(a.requires_approval || is_read(a.effect)) {
        return Err(no(format!("{} は承認の要らない書く操作なので、model 無しでは選ばない", a.id)));
    }
    Ok(Workflow { proven: None, fails: 0,
        schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
        id: workflow_id(goal, op),
        goal: goal.into(),
        steps: vec![WorkflowStep { kind: StepKind::Action, id: "s1".into(), capability: a.id.clone(), input: json!({}) }],
        ui: None,
        checks: vec![],
    })
}

/// RQ-023: Automation の 3 択の門（Execution の前・承認の門〈exact payload〉と WF-006 の宛先の読み直しの手前 = 同じ 1 つの門の段）。
/// Suggest = 計画と操作の前の読みまでで 1 手も送らない・Trusted read-only = 書く手が 1 つでも在れば 1 手も送らない・Ask before write = 今まで通り。
fn automation_gate(run: &EngineRun, cx: &EngineContext) -> Result<(), EngineError> {
    use crate::domain::permission::AutomationMode as M;
    let actions = cx.registry.actions();
    let writes = run.plans.iter().flat_map(|p| &p.workflow.steps).find(|s| s.is_action() && actions.iter().any(|a| a.id == s.capability && !is_read(a.effect)));
    match (cx.automation, writes) {
        (M::Suggest, _) => Err(EngineError::Denied(Denial::from(format!("{SUGGEST}. {SUGGEST_RUN}")))),
        (M::TrustedReadOnly, Some(s)) => Err(EngineError::Denied(Denial::from(format!(
            "Automation is set to Trusted read-only, and the step “{}” would change something, so nothing was sent. To run it, set Automation to Ask before write",
            s.id
        )))),
        _ => Ok(()),
    }
}

/// RQ-023: Suggest の門の 1 行（`automation_gate` と、生成した Adapter の試しの前の門）。
const SUGGEST: &str = "Automation is set to Suggest, so Shibubu only made the plan and sent nothing";
/// B75: 手を走らせる前の読みが形の違う入力（`ErrorCode::InvalidInput` = model の書いた手順の誤り）で読めない時の Denied の 1 行（何も送っていない・次の一手）
pub const UNREADABLE_CHECK: &str = "Nothing was sent, because a check in the steps was written in a form Shibubu can't read. Ask again to have the steps written again";
/// B76: model の書いた読みが Project の repo の外の file を指す時の Denied の 1 行（1 つも読んでいない・何も送っていない・次の一手）
pub const OUTSIDE_REPO: &str = "Nothing was read or sent, because the steps would read a file outside this Project's repo. Ask again to have the steps written again";
/// B76: 走った手の後で、読む手が `$from` を埋めた後に repo の外を指した時の理由（`EngineError::Stopped` の括弧の中）
pub const OUTSIDE_STEP: &str = "it would read a file outside this Project's repo; ask again to have the steps written again";
const SUGGEST_RUN: &str = "To run it, set Automation to Ask before write";

/// WF-036: AI の失敗の 1 行（error の原文の括弧から後ろを落とす = serde の細部を画面に出さない）。この 1 行は画面の 3 か所（Page の 1 行・済みの 1 行の
/// 「Without AI (…)」・Runs の行）にそのまま出る = 日本語の原文（計画の答えの形を確かめる Engine の文）は、答えが合わなかった事だけの英語に（WF-E x8・UI-2）。
fn ai_why(why: &str) -> String {
    let w = why.split(" (").next().and_then(|w| w.split('（').next()).unwrap_or(why).trim();
    if w.chars().any(|c| matches!(c, '\u{3000}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}')) {
        return "the AI's answer didn't fit".into();
    }
    w.to_string()
}

/// WF-036: AI 無しの道でも手段が無い時、tried に AI を使わなかった理由の印（`AI_NOT_USED`）を足す。
fn no_ai(e: EngineError, why: Option<&str>) -> EngineError {
    match (e, why) {
        (EngineError::NoMeans { operation, mut tried, searched }, Some(w)) => {
            tried.push(format!("{AI_NOT_USED}{w}"));
            EngineError::NoMeans { operation, tried, searched }
        }
        (e, _) => e,
    }
}

/// M2 G-a: 一言が app を名指さない（どの操作も宛先を決めなかった）時だけ、Page の app（`EngineContext::target`・`EngineRequest::target`）を宛先に。埋めたら true。
/// ENGINE-AUDIT W9 本体（節 14・P0 の代用の穴）: 名指したのに宛先が決まらない操作は埋めない（別の app で代用しない）。
/// B86: run と chat の振り分け（`CoreEngine::classify`）が同じこれを通す = run なら埋めて進む物を、chat が先に「Which app」と聞かない
pub fn fill_page_target(ops: &mut [Operation], target: Option<&str>) -> bool {
    let filled = ops.iter().all(|op| op.target.is_none()) && target.is_some();
    if filled {
        ops.iter_mut().filter(|op| !op.named).for_each(|op| op.target = target.map(str::to_owned));
    }
    filled
}

/// 見た能力ごとに 1 行（候補が 0 件なら空 = Page の 1 行は「nothing on this Mac can do it yet」）。
fn tried(found: &[CapabilityRecord]) -> Vec<String> {
    found
        .iter()
        .map(|r| match &r.verification {
            Verification::Verified { .. } => format!("{}（確かめ済み・この操作の手順が無い）", r.id),
            Verification::Unverified => format!("{}（未確認）", r.id),
            Verification::Unsupported { reason } => format!("{}（使えない: {reason}）", r.id),
        })
        .collect()
}

const INTERPRET_INSTRUCTIONS: &str = "あなたは Shibubu Engine の Goal Interpretation の段です。利用者の目的を、元の app・service・file に対する操作に分けます。\
各操作は id（op1, op2…・重ならない）・want（何をするか 1 行・目的と同じ言語で書く〈英語の目的なら英語〉・file の path や値は利用者の言葉のまま入れる）・\
success（元の app・service・file の何がどう変われば成功か。操作とは別に読み戻して確かめられる形）・\
target（対象の app の bundle id か service の origin。分からなければ null）を持ちます。操作は少なく（多くて 8 つ）、分けられない時は 1 つにします。\
ある app・service で読んだ値を別の app・service に渡す時は、分けずに 1 つの操作にします（受け渡しは 1 つの手順の中で行い、target は null）。\
目的の後ろに Situation の行が在れば、それは利用者が目的を言った時の画面の事実（Page・選んだ値・録った file・窓を開いている app）です。\
目的の「これ」「録った物」「選んだ物」が指す物はそこから埋め、target はそこに在る app の bundle id から決めて構いません。Situation に無い物は作りません。\
各操作に kind を付けます: act = 元の app・service・file への操作（押す・打つ・開く・読む・送る）・change = Shibubu 自身の Page や部品を足す・変える頼み・\
question = 質問・相談・確認（何も操作しない）・nothing = 挨拶など何もしない。app の操作を change にしません（Page に欄や button を足すのは、利用者がそう言った時だけ）。\
named = 目的が app・service を名前で指したか（true / false）。名指したのに target を決められない時は target を null のままにします（別の app で代用しない）。\
candidates = 宛先が 2 つ以上あり得る時（Situation に app が 2 つ在って目的がどちらとも読める等）、その bundle id か origin の候補。決まる時は空の配列。";

fn interpret_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["operations"],
        "properties": { "operations": { "type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["id", "want", "success", "target", "kind", "named", "candidates"],
            "properties": {
                "id": { "type": "string" }, "want": { "type": "string" }, "success": { "type": "string" }, "target": { "type": ["string", "null"] },
                "kind": { "type": "string", "enum": ["act", "change", "question", "nothing"] }, "named": { "type": "boolean" },
                "candidates": { "type": "array", "items": { "type": "string" } }
            }
        } } }
    })
}

/// Goal Interpretation: 目的を操作と成功の条件に分ける（model が在れば model・無ければ 1 操作 = 目的そのまま）。
/// Wave: W19-2
/// 保証する: 空の目的は `Denied`（何も呼ばない）・model の答えは schema で確かめ、操作 0 個・9 個以上・id の空と重なりは `Model`。
/// ENGINE-ADJUST-1102 ①: `stop` = 人の Stop の印（`EngineContext::stop`）。model の呼び出しは `complete_json_cancellable`（立てば CLI の子も止まり `Denied`〈`STOPPED_LINE`〉=
/// AI 無しの道に落ちない）。
pub fn interpret(goal: &str, situation: &Situation, llm: Option<&dyn Llm>, stop: &AtomicBool) -> Result<Vec<Operation>, EngineError> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(EngineError::Denied("Nothing was asked. Write what you want to do in one line".into()));
    }
    let Some(llm) = llm else {
        return Ok(vec![Operation { id: "op1".into(), want: goal.into(), success: String::new(), target: None, ..Default::default() }]);
    };
    // ENGINE-GENERIC-1101: 状況が在れば一言の後ろに事実の行（無ければ一言だけ = 今までと同じ字）・追記 234: chat と同じ書き方（`Situation::text`）
    let said = situation.text().map_or_else(|| goal.to_owned(), |s| format!("{goal}\n\n{s}"));
    let v = llm.complete_json_cancellable(JsonRequest::new(INTERPRET_NAME, INTERPRET_INSTRUCTIONS, &said, interpret_schema()), stop).map_err(ai_error)?;
    let ops: Vec<Operation> = serde_json::from_value(v["operations"].clone()).map_err(|e| EngineError::Model(format!("The action is in the wrong shape ({e})")))?;
    if ops.is_empty() || ops.len() > MAX_OPS {
        return Err(EngineError::Model(format!("There are {} actions (it must be 1 to {MAX_OPS})", ops.len())));
    }
    let mut seen = HashSet::new();
    if let Some(op) = ops.iter().find(|op| op.id.trim().is_empty() || op.want.trim().is_empty() || !seen.insert(op.id.as_str())) {
        return Err(EngineError::Model(format!("An action “{}” has no name or goal, or two actions share a name", op.id)));
    }
    Ok(ops)
}

/// Capability Discovery: 操作 1 つに使えそうな能力を集める（registry の Adapter・保存した `CapabilityRecord`・MCP の server の tools）。
/// Wave: W19-2
/// 保証する: 調べた手段は全部返す（使えない物も verification つきで・選ぶのは `choose`、どれを使うかを決めるのは `generate` の model）・MCP の tools は
/// `Origin::Official`・`Verification::Unverified`（一覧に在るだけで Ready にしない・19 通目 §5）・起こせない server は `Unsupported` の 1 件・
/// 保存した記録と環境が違う確かめ済みの能力は Unverified に下げ、constraints に確かめ直しの 1 行・保存した能力のうち今の registry に無い物（解析の候補・
/// 読み込まれていない Adapter）も返し、確かめ済みでも今は呼べないので Unverified に下げて 1 行・解析（W19-3）と app の Identity（intel）はここでは走らせない。
/// L4: MCP の server は、操作が名指した物（`want`・`target` の語）と、この process で前に答えた物だけ（`mcp_records`）。
pub fn discover(op: &Operation, cx: &mut EngineContext) -> Vec<CapabilityRecord> {
    discover_with(cx, &mcp_records(cx.mcp, &[op.want.as_str(), op.target.as_deref().unwrap_or("")]))
}

/// `discover` の中身（MCP の tools は呼ぶ側が読んだ物: `run` は run ごとに 1 回だけ server を起こし、操作ごとに使い回す）。
fn discover_with(cx: &EngineContext, tools: &[CapabilityRecord]) -> Vec<CapabilityRecord> {
    let mut found = abilities(cx.registry, cx.data_dir, &cx.env);
    found.extend(tools.iter().cloned());
    found
}

/// TALK-HONEST-1101: MCP の tools を除いた Capability Discovery（registry の Adapter ＋ 保存した能力を `discover` と同じ形で重ねる）。
/// 会話の AI に「この Mac で今出来る事」を渡す口（`PageEngine::abilities`）も同じ物を読む = Engine が見る手段と会話が言う手段がずれない。
/// 保証する: 何も起こさない（MCP の server・bridge・git を起こさない・L4）・保存した能力が読めない時は照合しない（registry の分だけ）。
pub fn abilities(registry: &AdapterRegistry, data_dir: &Path, env: &EnvFacts) -> Vec<CapabilityRecord> {
    let saved = load_records(&data_dir.join(CAPABILITIES_DIR)).unwrap_or_default(); // 読めない時は照合しない（Registry の段の書き込みが Store で止まる）
    with_saved(registry.records(), saved, env)
}

/// 前に確かめた能力が、今の registry に Adapter として無い時に constraints に付ける 1 行。
const NOT_LOADED: &str = "前に確かめたが、今の registry にこの能力の Adapter が無い（読み込むか作り直すまで使わない）";

/// registry の能力に保存した能力を重ねる: 環境が違えば確かめ直しの印・registry に無い保存した能力は候補に足す（確かめ済みなら今は呼べないので Unverified に下げる）。
fn with_saved(registry: Vec<CapabilityRecord>, saved: Vec<CapabilityRecord>, env: &EnvFacts) -> Vec<CapabilityRecord> {
    let mut found: Vec<CapabilityRecord> = registry
        .into_iter()
        .map(|mut r| {
            if is_verified(&r) && saved.iter().any(|s| s.id == r.id && needs_reverify(s, env)) {
                r.verification = Verification::Unverified;
                r.constraints.push(REVERIFY.into());
            }
            r
        })
        .collect();
    for mut s in saved {
        if found.iter().any(|r| r.id == s.id) {
            continue;
        }
        if is_verified(&s) {
            s.verification = Verification::Unverified;
            s.constraints.push(NOT_LOADED.into());
        }
        found.push(s);
    }
    found
}

fn base_record(id: String, kind: AdapterKind) -> CapabilityRecord {
    CapabilityRecord {
        schema: CAPABILITY_SCHEMA.into(),
        id,
        reuse: Reuse::Shared,
        origin: Origin::Official,
        kind,
        verification: Verification::Unverified,
        versions: vec![],
        permissions: vec![],
        evidence: vec![],
        constraints: vec![],
    }
}

/// MCP の設定の server ごとに tools/list を 1 回読み、tool 1 つ = 候補 1 件にする（server と tool の名前・inputSchema を evidence に入れて model に渡す）。
/// L4（DOGFOOD-X 周 8: Accept の 1〜2 分後に load1 > 100・設定の 10 本を一度に起こしていた）: 起こすのは `said`（一言・操作の文）の語に名前が出た server だけ
/// （語は英数字と - _ の並び・大文字小文字は見ない）。名指さない server は、この process で前に答えた物（Connections の `Check again`・前の run）を覚えた答えから
/// 使い、起こさない。名指さず答えてもいない server は候補にも出さない（起こしていないので使えるかを言えない）。起こすのは `mcp::list_tools_all`（2 本ずつ・答えなかった子は孫ごと止める）。
/// interface（L4）: 引数 `said` を足した = 起こしてよい server を run の一言から決めるため。
fn mcp_records(cfg: &McpConfig, said: &[&str]) -> Vec<CapabilityRecord> {
    let named = |name: &str| said.iter().flat_map(|t| t.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))).any(|w| w.eq_ignore_ascii_case(name));
    let mut out = Vec::new();
    // 解析の道具（REA）の tool は対象の app の能力ではない（解析は Deep Analysis の段が承認を取ってから・生成した Adapter から呼ばせない）。key が rea の server は
    // 起こさない。別の key で入れた解析の道具も tool の名前の組で見分け、使えない印の 1 件だけにする（`check_generated` がこの印で呼ばせない）
    // RQ4（RQ-016）: server は同時に起こし、答えは process の中に覚える（2 回目の run では起こし直さない・`mcp::list_tools_all`）。止めた server は起こさない
    let targets = McpConfig {
        mcp_servers: cfg
            .mcp_servers
            .iter()
            .filter(|(n, s)| n.as_str() != REA_SERVER && (named(n) || matches!(mcp::known_tools(n, s), Some(Ok(_)))))
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect(),
    };
    for (name, answer) in mcp::list_tools_all(&targets, MCP_TIMEOUT) {
        let name = name.as_str();
        match answer {
            Ok(tools) if analysis::looks_like_rea(tools.iter().map(|(t, _)| t.name.as_str())) => {
                out.push(CapabilityRecord { verification: Verification::Unsupported { reason: MCP_ANALYSIS.into() }, ..base_record(format!("mcp.{}", slug(name)), AdapterKind::Mcp) })
            }
            Ok(tools) => out.extend(tools.iter().map(|(t, h)| mcp_record(name, t, *h))),
            Err(e) => out.push(CapabilityRecord { verification: Verification::Unsupported { reason: e.to_string() }, ..base_record(format!("mcp.{}", slug(name)), AdapterKind::Mcp) }),
        }
    }
    out
}

/// tools/list の annotations の印（`evidence` の行・server の申告。Engine は model の申告より厳しい側にだけ使う・`means_problem`）。
const MCP_READ_ONLY: &str = "tools/list の annotations: readOnlyHint = true（server が読むだけと言う）";
const MCP_DESTRUCTIVE: &str = "tools/list の annotations: destructiveHint = true（server が消す・戻せないと言う）";
/// tools/list に解析の道具（REA）の tool の名前が在る server の印（`mcp.<server>` の Unsupported の理由・`check_generated` が見る）。
const MCP_ANALYSIS: &str = "解析の道具（tools/list に REA の tool の名前が在る）: Deep Analysis の段だけが承認を取って呼ぶ・作る Adapter からは呼ばない";

fn mcp_record(server: &str, t: &McpTool, hints: McpToolHints) -> CapabilityRecord {
    let mut evidence = vec![
        format!("MCP の server「{server}」の tools/list: {} — {}", t.name, t.description),
        format!("call: {}", json!({ "via": "mcp", "server": server, "tool": t.name, "arguments": {} })),
        format!("inputSchema: {}", t.input_schema),
    ];
    evidence.extend([(hints.read_only, MCP_READ_ONLY), (hints.destructive, MCP_DESTRUCTIVE)].into_iter().filter(|(on, _)| *on).map(|(_, m)| m.to_string()));
    CapabilityRecord {
        evidence,
        constraints: vec!["tools/list に在るだけ（呼べるか・結果が元の物に残るかは未確認）".into()],
        ..base_record(mcp_id(server, &t.name), AdapterKind::Mcp)
    }
}

/// MCP の tool の候補の id（`mcp.<server>.<tool>`）。
fn mcp_id(server: &str, tool: &str) -> String {
    format!("mcp.{}.{}", slug(server), slug(tool))
}

/// 手段から分かる効果と model の申告が食い違うか（承認と試してよいかは申告の effect で決まるので、申告が手段より弱い答えはここで止める）。
/// HTTP は GET と HEAD だけが読む・CLI は書く扱い（program が何をするか Engine は知らない）・MCP は tools/list で readOnlyHint が true の tool だけが読み、
/// destructiveHint が true の tool は destructive でなければならない（どちらも server の申告で、model の申告より厳しい側にだけ使う）。
fn means_problem(a: &RecipeAction, found: &[CapabilityRecord]) -> Option<String> {
    let read = is_read(a.effect);
    match &a.call {
        RecipeCall::Http { method, .. } if read && !matches!(method.to_ascii_uppercase().as_str(), "GET" | "HEAD") => {
            Some(format!("{} は HTTP の {method} なので読むだけにできません", a.id))
        }
        // HTTP の宛先は外の service（url は model が書く）。手元を読む手にすると、受け渡した値を外へ渡す手の承認（`run_steps`）を素通りする
        RecipeCall::Http { .. } if a.effect == Effect::LocalRead => Some(format!("{} は HTTP の手なので local_read にできません（外の service を読む = external_read）", a.id)),
        RecipeCall::Cli { .. } if read => Some(format!("{} は CLI の手なので読むだけにできません（CLI は書く扱いで承認が要る・読み戻しには使えない）", a.id)),
        RecipeCall::Mcp { server, tool, .. } => {
            let id = mcp_id(server, tool);
            let said = |m: &str| found.iter().any(|r| r.id == id && r.kind == AdapterKind::Mcp && r.origin == Origin::Official && r.evidence.iter().any(|e| e == m));
            if read && !said(MCP_READ_ONLY) {
                Some(format!("{} の tool {tool} は、MCP の server が読むだけ（readOnlyHint）と言っていないので読むだけにできません", a.id))
            } else if said(MCP_DESTRUCTIVE) && a.effect != Effect::Destructive {
                Some(format!("{} の tool {tool} は、MCP の server が消す・戻せない（destructiveHint）と言っています", a.id))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Integration Planning: 候補から操作ごとに 1 つ選ぶ（実現性・操作の忠実さ・権限・安定性・cost・実行環境）。
/// Wave: W19-2
/// 保証する: 確かめ済み（Verified = Ready）でない能力を選ばない（解析の候補・未検証の生成物・MCP の一覧に在るだけの tool は先に Verification を通す）・
/// 対象（`op.target`）の決まった操作では他の app 専用の能力を選ばない・target の無い操作では複数の app で使える能力だけ・同点は公式の連携・それでも同点は id の若い方。None = 手段が無い。
pub fn choose(op: &Operation, found: &[CapabilityRecord]) -> Option<Choice> {
    found
        .iter()
        .filter(|r| is_verified(r) && relevant(op, r))
        .map(|r| (r, score(op, r)))
        .max_by(|(a, sa), (b, sb)| {
            (total(sa), matches!(a.origin, Origin::Official)).cmp(&(total(sb), matches!(b.origin, Origin::Official))).then_with(|| b.id.cmp(&a.id))
        })
        .map(|(r, s)| Choice {
            operation: op.id.clone(),
            capability: r.id.clone(),
            reason: format!("{} · Verified", origin_word(&r.origin)),
            score: s,
        })
}

fn relevant(op: &Operation, r: &CapabilityRecord) -> bool {
    match (&op.target, &r.reuse) {
        (_, Reuse::Shared) => true,
        (Some(t), Reuse::AppSpecific { app }) => app == t,
        (None, Reuse::AppSpecific { .. }) => false,
    }
}

fn score(op: &Operation, r: &CapabilityRecord) -> Score {
    use AdapterKind::*;
    Score {
        feasibility: match (&op.target, &r.reuse) {
            (Some(t), Reuse::AppSpecific { app }) if app == t => 3,
            (_, Reuse::Shared) => 2,
            _ => 1,
        },
        fidelity: match r.kind {
            Api | Mcp | Repo | LocalCore | Extension => 3,
            OriginalSurface => 2,
            MacGui => 1,
            Device | Manual | Undecided => 0,
        },
        permission: 3u8.saturating_sub(r.permissions.len().min(3) as u8),
        stability: match r.origin {
            Origin::Official => 3,
            Origin::Generated { .. } => 2,
            Origin::Analysis { .. } => 1,
        },
        cost: match r.kind {
            LocalCore | Repo => 3,
            Api | Mcp | Extension => 2,
            OriginalSurface | MacGui => 1,
            Device | Manual | Undecided => 0,
        },
        environment: if r.constraints.is_empty() { 3 } else { 1 },
    }
}

fn total(s: &Score) -> u32 {
    [s.feasibility, s.fidelity, s.permission, s.stability, s.cost, s.environment].iter().map(|&x| u32::from(x)).sum()
}

fn origin_word(o: &Origin) -> &'static str {
    match o {
        Origin::Official => "Official connection",
        Origin::Analysis { .. } => "Analysis candidate",
        Origin::Generated { .. } => "Generated ability",
    }
}

/// 保存した能力をこの環境で使う前に確かめ直すか（版・OS・権限のどれかが記録と違えば true）。
/// Wave: W19-2
/// 保証する: 確かめ済みでない記録は true・`env.target_version` が在って記録の `versions` に無い・`env.os_version` が在って記録に無い・
/// 記録の `permissions`（確かめた時に在った権限）のどれかが今無い → true。保証しない: 版が同じなら動く事（Execution の後の `verify`）。
pub fn needs_reverify(rec: &CapabilityRecord, env: &EnvFacts) -> bool {
    !is_verified(rec)
        || env.target_version.as_ref().is_some_and(|v| !rec.versions.contains(v))
        || (!env.os_version.is_empty() && !rec.versions.contains(&env.os_version))
        || !rec.permissions.iter().all(|p| env.permissions.contains(p))
}

/// W19-2 の署名を compile で縛り、保存形式（serde の往復と未知の欄の拒否）と段の網羅だけを走らせる。
#[cfg(test)]
mod w19_2_interface {
    use super::*;

    #[test]
    fn w19_2_pipeline_signatures() {
        let _: fn(&str, &mut EngineContext) -> Result<EngineRecord, EngineError> = run;
        // ENGINE-GENERIC-1101 で situation・ENGINE-ADJUST-1102 ① で人の Stop の印が足された（前は 2 引数の pin のまま = 試験の build が通らなかった）
        let _: fn(&str, &Situation, Option<&dyn Llm>, &AtomicBool) -> Result<Vec<Operation>, EngineError> = interpret;
        let _: fn(&Operation, &mut EngineContext) -> Vec<CapabilityRecord> = discover;
        let _: fn(&Operation, &[CapabilityRecord]) -> Option<Choice> = choose;
        let _: fn(&CapabilityRecord, &EnvFacts) -> bool = needs_reverify;
        let _: fn(McpConfig, PathBuf) -> CallRunner = call_runner;
        let _: fn(&Path, &CapabilityRecord) -> Result<PathBuf, EngineError> = save_record;
        let _: fn(&Path) -> Result<Vec<CapabilityRecord>, EngineError> = load_records;
        let _: fn(&Operation, &[CapabilityRecord], &mut EngineContext, Option<&str>) -> Result<Generated, EngineError> = generate;
        let _: fn(&ReadBack, Option<&Observation>, &Observation) -> Result<String, EngineError> = verify;
        let _: fn(&Path, &Recipe) -> Result<PathBuf, EngineError> = save_recipe;
        // EngineContext は Execution に caller と consents を持つ（run_workflow と同じ gate の材料）
        fn gate<'a>(cx: &EngineContext<'a>) -> (Caller<'a>, &'a [ConsentReceipt]) {
            (cx.caller, cx.consents)
        }
        let _ = gate;
    }

    #[test]
    fn w19_2_generated_round_trips_and_rejects_unknown_fields() {
        use crate::domain::capability::{Origin, RecipeAction, RecipeCall, Reuse, Verification, RECIPE_SCHEMA};
        use serde_json::json;
        let check = |expect| ReadBack { action: "desktop.window.state".into(), input: json!({ "app_name": "Example" }), pointer: "/controls/0/value".into(), expect };
        let expects = [Expect::Equals { value: json!("新しい題") }, Expect::Contains { text: "題".into() }, Expect::Changed, Expect::Fewer { by: 1 }, Expect::Replaced { from: "旧".into(), to: "新".into() }];
        // 網羅: 条件を足したら、ここと verify を同じ commit で直す
        for e in &expects {
            match e {
                Expect::Equals { .. } | Expect::Contains { .. } | Expect::Changed | Expect::Fewer { .. } | Expect::Replaced { .. } => {}
            }
        }
        let step = |input| WorkflowStep { kind: StepKind::Action, id: "set".into(), capability: "example.title.set".into(), input };
        let g = Generated {
            workflow: Workflow { proven: None, fails: 0,
                schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
                id: "wf1".into(),
                goal: "題を変えて保存して".into(),
                steps: vec![step(json!({ "path": "/real/doc.txt" }))],
                ui: None,
                checks: vec![check(Expect::Equals { value: json!("新しい題") })],
            },
            recipe: Some(Recipe {
                schema: RECIPE_SCHEMA.into(),
                id: "example".into(),
                provider: "example".into(),
                reuse: Reuse::AppSpecific { app: "org.example.app".into() },
                origin: Origin::Generated { artifact: "example".into() },
                versions: vec![],
                actions: vec![RecipeAction {
                    id: "example.title.set".into(),
                    effect: Effect::LocalWrite,
                    approval: true,
                    scopes: vec![],
                    input: json!({ "type": "object" }),
                    verifier: "read_back".into(),
                    call: RecipeCall::Cli { argv: vec!["tool".into(), "{{input.path}}".into()] },
                    verification: Verification::Unverified,
                }],
            }),
            trial: Some(Trial { steps: vec![step(json!({ "path": "/data/trials/r1/doc.txt" }))], checks: vec![check(Expect::Changed)] }),
            counts: Value::Null,
            context: false,
        };
        for e in expects {
            let mut g = g.clone();
            g.workflow.checks = vec![check(e)];
            let v = serde_json::to_value(&g).unwrap();
            assert_eq!(serde_json::from_value::<Generated>(v).unwrap(), g);
        }
        let mut extra = serde_json::to_value(&g).unwrap();
        extra["verified"] = json!(true);
        assert!(serde_json::from_value::<Generated>(extra).is_err(), "未知の欄（model の自己申告）は拒否");
        let mut extra = serde_json::to_value(&g).unwrap();
        extra["workflow"]["checks"][0]["expect"]["ok"] = json!(true);
        assert!(serde_json::from_value::<Generated>(extra).is_err(), "条件の未知の欄は拒否");
        let bare = json!({ "workflow": { "schema": WORKFLOW_SCHEMA, "id": "w", "goal": "g", "steps": [] } });
        let g2: Generated = serde_json::from_value(bare).unwrap();
        assert_eq!((g2.recipe, g2.trial, g2.workflow.checks.len()), (None, None, 0), "recipe・trial・checks は省ける（W19-2b の前の保存も読める）");
    }

    #[test]
    fn w19_2_stages_are_every_stage_once_in_order() {
        for (i, s) in Stage::ALL.iter().enumerate() {
            // 網羅: 段を足したら ALL と、この表を同じ commit で直す
            let at = match s {
                Stage::GoalInterpretation => 0,
                Stage::CapabilityDiscovery => 1,
                Stage::DeepAnalysis => 2,
                Stage::IntegrationPlanning => 3,
                Stage::DynamicGeneration => 4,
                Stage::Verification => 5,
                Stage::Execution => 6,
                Stage::Registry => 7,
            };
            assert_eq!(at, i, "{s:?}");
        }
    }

    #[test]
    fn w19_2_engine_record_round_trips_and_rejects_unknown_fields() {
        let rec = EngineRecord {
            schema: ENGINE_SCHEMA.into(),
            goal: "README の題を変えて保存して".into(),
            operations: vec![Operation { id: "op1".into(), want: "題を変える".into(), success: "disk の 1 行目が新しい題".into(), target: Some("org.example.app".into()), ..Default::default() }],
            discovered: vec![],
            choices: vec![Choice { operation: "op1".into(), capability: "example.doc.title.set".into(), reason: "公式の script".into(), score: Score { feasibility: 3, fidelity: 3, permission: 2, stability: 2, cost: 3, environment: 3 } }],
            generated: vec![],
            verified: vec!["disk の sha が変わった".into()],
            receipts: vec![WorkflowReceipt { workflow: "wf".into(), steps: vec![] }],
            stages: Stage::ALL
                .iter()
                .zip([StageOutcome::Done { note: "n".into() }, StageOutcome::Skipped { why: "w".into() }, StageOutcome::Failed { why: "f".into() }].iter().cycle())
                .map(|(&stage, o)| StageLog { stage, outcome: o.clone(), ms: 1, counts: None })
                .collect(),
            reused: true,
            partial: None,
            without_ai: None,
            already: None,
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(serde_json::from_value::<EngineRecord>(v.clone()).unwrap(), rec);
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("receipts");
        assert_eq!(serde_json::from_value::<EngineRecord>(old).unwrap().receipts, vec![], "W19-2b の前の記録（receipts の無い json）も読める");
        let mut extra = v;
        extra["verified_by_product"] = serde_json::json!(true);
        assert!(serde_json::from_value::<EngineRecord>(extra).is_err(), "未知の欄は拒否");
        let env = EnvFacts { target_version: Some("1.19".into()), os_version: "15.1".into(), permissions: vec!["accessibility".into()] };
        assert_eq!(serde_json::from_value::<EnvFacts>(serde_json::to_value(&env).unwrap()).unwrap(), env);
    }
}

/// W19-2b の署名を compile で縛り、保存形式（serde の往復と未知の欄の拒否）だけを走らせる。
#[cfg(test)]
mod w19_2b_interface {
    use super::*;
    use crate::domain::run::Outcome;
    use serde_json::json;

    #[test]
    fn w19_2b_workflow_signatures() {
        let _: fn(&Workflow) -> Result<(), WorkflowError> = validate;
        let _: fn(&Workflow, &mut AdapterRegistry, crate::adapters::Caller, &[crate::domain::permission::ConsentReceipt]) -> Result<WorkflowReceipt, WorkflowError> = run_workflow;
        let _: fn(&Path, &Workflow) -> Result<PathBuf, WorkflowError> = save_workflow;
        let _: fn(&Path) -> Result<Vec<Workflow>, WorkflowError> = load_workflows;
    }

    #[test]
    fn w19_2b_workflow_and_receipt_round_trip_and_reject_unknown_fields() {
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "wf1".into(),
            goal: "repo の状態を別の service に残して".into(),
            steps: vec![
                WorkflowStep { kind: StepKind::Action, id: "read".into(), capability: "git.status.read".into(), input: json!({ "uri": "file:///tmp/repo" }) },
                WorkflowStep { kind: StepKind::Action, id: "post".into(), capability: "example.comment.create".into(), input: json!({ "body": { "$from": "read.branch" } }) },
            ],
            ui: Some("project.wf1".into()),
            checks: vec![],
        };
        let v = serde_json::to_value(&wf).unwrap();
        assert_eq!(serde_json::from_value::<Workflow>(v.clone()).unwrap(), wf);
        let mut extra = v;
        extra["auto_run"] = json!(true);
        assert!(serde_json::from_value::<Workflow>(extra).is_err(), "未知の欄は拒否");
        let rc = WorkflowReceipt {
            workflow: "wf1".into(),
            steps: vec![StepReceipt {
                step: "read".into(),
                outcome: Outcome::Verified { evidence: vec!["git.status · observation 1".into()] },
                observation: Some("1".into()),
                handoffs: vec![Handoff { from_step: "read".into(), to_step: "post".into(), field: "body".into(), digest: "ab12".into() }],
            }],
        };
        assert_eq!(serde_json::from_value::<WorkflowReceipt>(serde_json::to_value(&rc).unwrap()).unwrap(), rc);
    }
}

/// W19-3 の Engine の側の署名を compile で縛る。
#[cfg(test)]
mod w19_3_interface {
    use super::*;
    #[test]
    fn w19_3_candidates_from_signature() {
        let _: fn(&AnalysisReport) -> Vec<CapabilityRecord> = candidates_from;
        let _: fn(&AnalysisRequest, &mut EngineContext) -> Result<DeepAnalysis, AnalysisError> = deep_analysis;
        let _: fn(&McpConfig) -> Vec<Box<dyn AnalysisBackend>> = analysis_backends;
    }
}

/// W19-2・W19-2b の本体の試験。外の service・owner の quota は使わない: model は `llm::fake::Fake`（`complete_json` の schema の検査と答えの検証は本物と同じ）、
/// MCP は試験の側で起こした local の server（tests/fake-mcp-server.py・架空の app Quillpad・製品の code はこの名前を知らない）。gate の単体の試験の Adapter は
/// registry に `register` した試験の偽物（alpha / beta）。`run` の通しは W19-1 の本体（`records`・`add_recipes`・`RecipeAdapter`）の上で走り、W19-2b の通しは
/// 本物の git（手書きの公式の Adapter と、試験の側で作った repo）と Quillpad の 2 つの service の実状態を試験の側で直に読む。
#[cfg(test)]
mod w19_2_body {
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::domain::permission::ConsentDecision;
    use crate::llm::fake::Fake;
    use crate::llm::LlmError;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    pub(super) const CALLER: Caller<'static> = Caller { app_id: "proj", run_id: None };
    pub(super) const QUILL_APP: &str = "org.example.quillpad";

    pub(super) fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-w19-2-{name}-{}-{}", std::process::id(), crate::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn obs(payload: Value) -> Observation {
        Observation { id: "7".into(), source_ref: "t".into(), observed_at: "2026-10-23T00:00:00Z".into(), payload }
    }

    fn rb(pointer: &str, expect: Expect) -> ReadBack {
        ReadBack { action: "probe.text.read".into(), input: json!({}), pointer: pointer.into(), expect }
    }

    pub(super) fn rec(id: &str, reuse: Reuse, origin: Origin, verification: Verification) -> CapabilityRecord {
        CapabilityRecord { reuse, origin, verification, ..base_record(id.into(), AdapterKind::Mcp) }
    }

    pub(super) fn verified() -> Verification {
        Verification::Verified { at: "2026-10-23T00:00:00Z".into(), evidence: "読み戻した".into() }
    }

    fn op(target: Option<&str>) -> Operation {
        Operation { id: "op1".into(), want: "文書の題を変える".into(), success: "題が新しい値".into(), target: target.map(str::to_string), ..Default::default() }
    }

    pub(super) fn consent(action: &str, input: &Value) -> ConsentReceipt {
        ConsentReceipt {
            id: format!("c-{}", crate::model::new_id()),
            app_id: CALLER.app_id.into(),
            subject: ConsentSubject::Action { action_ref: action.into(), run_id: None },
            scopes: vec![],
            payload_digest: Some(sdk::payload_digest(action, input)),
            decision: ConsentDecision::Approved,
            decided_at: "2026-10-23T00:00:00Z".into(),
        }
    }

    /// 試験の偽の Adapter（service 1 つ: `<p>.text.read`・`<p>.text.write`〈承認が要る〉）。text を持ち、呼ばれた action を数える。lease = 人が操作中（書く手を Rejected）。
    /// `….list` と `….state` の action は text を JSON として返す（`desktop.windows.list`・`desktop.window.state` の偽物・JSON でなければ Timeout）。
    struct Probe {
        id: &'static str,
        specs: &'static [Spec],
        text: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        lease: bool,
        /// false = 接続の要る Adapter（readiness が Ready でない・G76 の (A)）
        ready: bool,
    }
    const fn specs(read: &'static str, write: &'static str) -> [Spec; 2] {
        [
            Spec { id: read, effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: write, effect: Effect::LocalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        ]
    }
    static ALPHA: [Spec; 2] = specs("alpha.text.read", "alpha.text.write");
    static BETA: [Spec; 2] = specs("beta.text.read", "beta.text.write");

    fn probe(id: &'static str, specs: &'static [Spec], text: &str, lease: bool) -> (Probe, Arc<Mutex<String>>, Arc<Mutex<Vec<String>>>) {
        let (t, c) = (Arc::new(Mutex::new(text.to_string())), Arc::new(Mutex::new(Vec::new())));
        (Probe { id, specs, text: t.clone(), calls: c.clone(), lease, ready: true }, t, c)
    }

    impl Adapter for Probe {
        fn id(&self) -> &'static str {
            self.id
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::LocalCore
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            self.id
        }
        fn specs(&self) -> &'static [Spec] {
            self.specs
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            if self.ready {
                CapabilityReadiness::Ready
            } else {
                CapabilityReadiness::RequiresConnection
            }
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            self.calls.lock().unwrap().push(action.into());
            if action.ends_with(".write") {
                if self.lease {
                    return (Outcome::Rejected { reason: "人が操作中です（lease）".into() }, None);
                }
                *self.text.lock().unwrap() = input["text"].as_str().unwrap_or("").into();
                return (Outcome::Unverified { reason: "書いた".into() }, Some(sdk::observe(self.id, sdk::now(), json!({ "written": true }))));
            }
            let t = self.text.lock().unwrap().clone();
            if action.ends_with(".list") || action.ends_with(".state") {
                // text が JSON でない = 答えない app（AX の時間切れ・operator が混雑の Zoom で測った形）
                return match serde_json::from_str::<Value>(&t) {
                    Ok(v) => (Outcome::Verified { evidence: vec![self.id.into()] }, Some(sdk::observe(self.id, sdk::now(), v))),
                    Err(_) => (Outcome::Failed { error: ErrorCode::Timeout }, None),
                };
            }
            (Outcome::Verified { evidence: vec![self.id.into()] }, Some(sdk::observe(self.id, sdk::now(), json!({ "text": t, "len": t.len() }))))
        }
        fn cleanup(&mut self) {}
        fn records(&self) -> Vec<CapabilityRecord> {
            vec![]
        }
        /// WF-006: 外へ効く手（desktop の偽物）は MacGui と同じに送る直前に宛先を読み直す扱い
        fn prechecks(&self, _action: &str) -> bool {
            true
        }
    }

    pub(super) fn cx<'a>(
        reg: &'a mut AdapterRegistry,
        dir: &'a Path,
        llm: Option<&'a dyn Llm>,
        mcp: &'a McpConfig,
        consents: &'a [ConsentReceipt],
        analysis: &'a mut Vec<Box<dyn AnalysisBackend>>,
    ) -> EngineContext<'a> {
        EngineContext { generate_calls: 0, registry: reg, caller: CALLER, consents, data_dir: dir, llm, mcp, env: EnvFacts::default(), analysis: analysis.as_mut_slice(), readback: Duration::ZERO, target: None, document: None, document_file: None, window: None, repo: None, progress: None, trace: false, runs: None, automation: Default::default(), situation: &NO_SITUATION, stop: &NO_STOP, staged: None }
    }

    pub(super) fn quill_cfg(mode: &str, log: &Path) -> McpConfig {
        connected(McpConfig { mcp_servers: [("quill".to_string(), crate::mcp::w19_2_body::quill(mode, log))].into_iter().collect() })
    }

    /// 利用者が前に Connections で確かめた（`Check again`）server にする = Accept の発見が起こさずに覚えた答えを使う（L4・REA の役は起こさない）。
    pub(crate) fn connected(cfg: McpConfig) -> McpConfig {
        let mine = McpConfig { mcp_servers: cfg.mcp_servers.iter().filter(|(n, _)| n.as_str() != REA_SERVER).map(|(n, s)| (n.clone(), s.clone())).collect() };
        let _ = mcp::list_tools_all(&mine, std::time::Duration::from_secs(20));
        cfg
    }

    /// L4（DOGFOOD-X 周 8: Accept の 1〜2 分後に load1 > 100・Engine が ~/.shibubu/mcp.json の 10 本を一度に起こしていた）: Accept の発見は、
    /// 一言・操作が名指さず、前に答えてもいない server を起こさない（候補にも出さない）。名指せば起こし、答えを覚えた後は名指さなくても起こさずに使う。
    /// 起こした回数は sh が 1 行書いてから exec で数える。負の対照: 名指し・覚えの門を外す → 名指さない 1 回目で起こして赤。
    #[test]
    fn l4_accept_does_not_start_an_mcp_server_nobody_named_or_connected() {
        let d = tmp("l4-accept-mcp");
        let starts = d.join("starts.log");
        let q = crate::mcp::w19_2_body::quill("", &d.join("calls.jsonl"));
        let mut args = vec!["-c".to_string(), "echo start >> \"$0\"; exec \"$@\"".into(), starts.display().to_string(), q.command.clone()];
        args.extend(q.args);
        let cfg = McpConfig { mcp_servers: [("notes-mcp".to_string(), crate::mcp::McpServerConfig { command: "/bin/sh".into(), args, env: q.env, ..Default::default() })].into_iter().collect() };
        let n = || std::fs::read_to_string(&starts).map(|t| t.lines().count()).unwrap_or(0);
        let ids = |f: &[CapabilityRecord]| f.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        let mine = format!("mcp.{}.", slug("notes-mcp"));
        let found = mcp_records(&cfg, &["Make a chart of my sales", "com.apple.iWork.Numbers"]);
        assert_eq!((n(), ids(&found)), (0, vec![]), "名指さず確かめてもいない server を起こした / 候補にした");
        let found = mcp_records(&cfg, &["Use notes-mcp to rename the doc"]);
        assert_eq!(n(), 1, "名指した server を起こさない");
        assert!(found.iter().any(|r| r.id.starts_with(&mine)), "{:?}", ids(&found));
        let found = mcp_records(&cfg, &[]);
        assert_eq!(n(), 1, "答えを覚えた server を起こし直した");
        assert!(found.iter().any(|r| r.id.starts_with(&mine)), "前に答えた server を使わない: {:?}", ids(&found));
        let _ = std::fs::remove_dir_all(&d);
    }

    pub(super) fn write_doc(p: &Path, title: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, json!({ "title": title, "body": "本文" }).to_string()).unwrap();
    }

    /// 試験の側の oracle: Engine と MCP の server を通らず disk の JSON を直に読む。
    pub(super) fn title_on_disk(p: &Path) -> String {
        serde_json::from_str::<Value>(&std::fs::read_to_string(p).unwrap()).unwrap()["title"].as_str().unwrap().to_string()
    }

    /// model の答えの手 1 つ（`generate_schema` の step の形・input は JSON の文字列）。
    pub(super) fn step_answer(id: &str, cap: &str, input: Value) -> Value {
        json!({ "id": id, "capability": cap, "input_json": input.to_string() })
    }

    /// model の答え（`generate_schema` の形）: Quillpad の MCP の tool 2 つを呼ぶ Adapter・本物の doc の題を変える手順・題の読み戻し・試験の置き場の写しでの試験。
    pub(super) fn quill_answer(real: &Path, trial: &Path, title: &str) -> Value {
        let call = |tool: &str, args: Value| json!({ "via": "mcp", "server": "quill", "tool": tool, "arguments": args }).to_string();
        let c = |path: &Path| json!({ "action": "quillpad.doc.read", "input_json": json!({ "path": path }).to_string(), "pointer": "/title", "expect": "equals", "value_json": json!(title).to_string() });
        json!({
            "none_why": null, "workflow_steps": [step_answer("set", "quillpad.doc.title.set", json!({ "path": real, "title": title }))],
            "checks": [c(real)],
            "recipe": { "id": "quillpad", "provider": "quillpad", "app": QUILL_APP, "actions": [
                { "id": "quillpad.doc.title.set", "effect": "local_write", "input_schema_json": json!({ "type": "object" }).to_string(),
                  "call_json": call("quill_set_title", json!({ "path": "{{input.path}}", "title": "{{input.title}}" })) },
                { "id": "quillpad.doc.read", "effect": "local_read", "input_schema_json": json!({ "type": "object" }).to_string(),
                  "call_json": call("quill_read", json!({ "path": "{{input.path}}" })) },
            ] },
            "trial": { "steps": [step_answer("set", "quillpad.doc.title.set", json!({ "path": trial, "title": title }))], "checks": [c(trial)] },
        })
    }

    #[test]
    fn w19_2_verify_reads_back_and_never_passes_a_mismatch() {
        let after = obs(json!({ "text": "新しい題", "n": 2 }));
        assert!(verify(&rb("/text", Expect::Equals { value: json!("新しい題") }), None, &after).unwrap().contains("/text を読み戻した"));
        assert!(matches!(verify(&rb("/text", Expect::Equals { value: json!("古い題") }), None, &after), Err(EngineError::Verification(_))));
        assert!(verify(&rb("/text", Expect::Contains { text: "題".into() }), None, &after).is_ok());
        assert!(matches!(verify(&rb("/n", Expect::Contains { text: "2".into() }), None, &after), Err(EngineError::Verification(_))), "文字列でない値は含まない");
        assert!(matches!(verify(&rb("/none", Expect::Equals { value: json!(null) }), None, &after), Err(EngineError::Verification(_))), "pointer の先が無い");
        assert!(verify(&rb("", Expect::Equals { value: json!({ "text": "新しい題", "n": 2 }) }), None, &after).is_ok(), "\"\" は payload 全体");
        let changed = rb("/text", Expect::Changed);
        assert!(matches!(verify(&changed, None, &after), Err(EngineError::Verification(_))), "前の値が無ければ変わったと言わない");
        assert!(matches!(verify(&changed, Some(&obs(json!({ "text": "新しい題" }))), &after), Err(EngineError::Verification(_))), "同じ値は変化でない");
        assert!(verify(&changed, Some(&obs(json!({ "text": "古い題" }))), &after).is_ok());
        assert!(verify(&changed, Some(&obs(json!({}))), &after).is_ok(), "前に無かった値が出来たのも変化");
    }

    /// operator 11-03（r10 の AI の check = /controls を contains・前は配列を見ずに必ず合わない = 押せても 90 秒後に Unconfirmed）: 配列はどれかの項目の文字列で合う。
    /// 項目が物なら名前か値だけ（役割・押し方・道・ref の字では合わない）・数は字として見ない・配列でない物は今まで通り。負の対照: `has_text` の Array の腕を false に →
    /// 1 つ目が赤・Object の腕を全部の値（`m.values()`）に → 役割・道・ref の assert が赤
    #[test]
    fn r10_contains_on_a_list_passes_when_one_item_has_the_text() {
        let after = obs(json!({ "controls": [{ "element": "e3.2", "label": "Tracks", "role": "AXStaticText" }, { "label": "Strum", "value": "Echo Strum", "role": "AXRow", "actions": ["AXPress"], "path": ["Library"] }], "n": [2] }));
        let has = |at: &str, text: &str| verify(&rb(at, Expect::Contains { text: text.into() }), None, &after);
        assert!(has("/controls", "Echo Strum").is_ok(), "一覧の 2 つ目の行の値");
        assert!(has("/controls", "Tracks").is_ok(), "一覧の 1 つ目の行の名前");
        assert!(matches!(has("/controls", "Natural Flat Pick"), Err(EngineError::Verification(_))), "どの行にも無い名前");
        assert!(matches!(has("/n", "2"), Err(EngineError::Verification(_))), "数の配列は含まない");
        for (text, why) in [("e3.2", "読むたびに変わる ref"), ("AXRow", "役割"), ("Press", "押し方"), ("Library", "道（祖先の名前）")] {
            assert!(matches!(has("/controls", text), Err(EngineError::Verification(_))), "{why}の字で合わせた");
        }
        assert!(matches!(has("/controls/0", "Tracks"), Err(EngineError::Verification(_))), "配列でも文字列でもない値は今まで通り含まない");
    }

    /// M2 G-b: 窓の要素の読み（`desktop.window.state`）は読むたびに ref（`element` = `e<snapshot>.<n>`）が変わる。Changed は ref を外して比べ、
    /// 途中までの読み（complete = false・混雑で 98 → 77 に切れた読み G31 log）では変わったと言わない。
    /// 負の対照: ref だけ違う 2 つの読み → 変わっていない／要素が 1 つ増えた（track を足した）→ 変わった／前か後が途中まで → 変わったと言わない。
    #[test]
    fn m2_changed_ignores_per_read_refs_and_partial_reads() {
        let control = |r: &str, label: &str| json!({ "element": r, "label": label, "role": "AXButton", "value": null, "actions": ["AXPress"], "value_settable": false });
        let state = |complete: bool, controls: Vec<Value>| obs(json!({ "pid": 222, "window_id": 300, "app_name": "REAPER", "complete": complete, "controls": controls }));
        let before = state(true, vec![control("e1.1", "Track 1")]);
        let same = state(true, vec![control("e2.1", "Track 1")]);
        let added = state(true, vec![control("e2.1", "Track 1"), control("e2.2", "Track 2")]);
        let changed = rb("/controls", Expect::Changed);
        assert!(matches!(verify(&changed, Some(&before), &same), Err(EngineError::Verification(_))), "ref だけ変わった読みは変化でない");
        assert!(matches!(verify(&rb("/controls/0", Expect::Changed), Some(&before), &same), Err(EngineError::Verification(_))), "要素 1 つでも同じ");
        assert!(matches!(verify(&rb("", Expect::Changed), Some(&before), &same), Err(EngineError::Verification(_))), "payload 全体でも同じ");
        assert!(verify(&changed, Some(&before), &added).is_ok(), "track が 1 本増えた");
        assert!(matches!(verify(&changed, Some(&state(false, vec![])), &added), Err(EngineError::Verification(_))), "前が途中まで");
        assert!(matches!(verify(&changed, Some(&before), &state(false, vec![control("e2.1", "Track 1"), control("e2.2", "Track 2")])), Err(EngineError::Verification(_))), "後が途中まで");
        // ref の形でない `element`（他の Adapter の data）は比べる値のまま
        let data = |e: &str| obs(json!({ "element": e }));
        assert!(verify(&rb("", Expect::Changed), Some(&data("hydrogen")), &data("helium")).is_ok(), "ref でない element の変化は変化");
    }

    /// G74b: `Fewer` は操作の前の読みの数からちょうど `by` 少ない時だけ（消した message の行が 1 つ減った）。
    /// 負の対照: 減っていない・2 つ減った（他の行も消えた）・前の読みが無い・前か後が途中まで・数えられない値 → 済みと言わない
    #[test]
    fn g74b_fewer_counts_against_the_read_before() {
        let row = |label: &str| json!({ "label": label, "role": "AXGroup", "value": null, "actions": ["AXShowMenu"], "value_settable": false });
        let state = |complete: bool, labels: &[&str]| obs(json!({ "pid": 222, "window_id": 300, "app_name": "Discord", "complete": complete, "controls": labels.iter().map(|l| row(l)).collect::<Vec<_>>() }));
        let fewer = rb("/controls", Expect::Fewer { by: 1 });
        let before = state(true, &["a", "b", "c"]);
        assert!(verify(&fewer, Some(&before), &state(true, &["a", "c"])).unwrap().contains("3 → 2"), "1 つ減った");
        assert!(matches!(verify(&fewer, Some(&before), &before), Err(EngineError::Verification(_))), "減っていない");
        assert!(matches!(verify(&fewer, Some(&before), &state(true, &["a"])), Err(EngineError::Verification(_))), "2 つ減った = 他の行も消えた");
        assert!(matches!(verify(&fewer, None, &state(true, &["a", "c"])), Err(EngineError::Verification(_))), "前の読みが無い");
        assert!(matches!(verify(&fewer, Some(&state(false, &["a", "b", "c"])), &state(true, &["a", "c"])), Err(EngineError::Verification(_))), "前が途中まで");
        assert!(matches!(verify(&fewer, Some(&before), &state(false, &["a", "c"])), Err(EngineError::Verification(_))), "後が途中まで");
        assert!(verify(&rb("/n", Expect::Fewer { by: 2 }), Some(&obs(json!({ "n": 5 }))), &obs(json!({ "n": 3 }))).is_ok(), "数なら値");
        assert!(matches!(verify(&rb("/t", Expect::Fewer { by: 1 }), Some(&obs(json!({ "t": "ab" }))), &obs(json!({ "t": "a" }))), Err(EngineError::Verification(_))), "文字列は数えない");
    }

    #[test]
    fn w19_2_needs_reverify_when_version_os_or_permission_changes() {
        let mut r = rec("quillpad.doc.read", Reuse::Shared, Origin::Generated { artifact: "quillpad".into() }, verified());
        (r.versions, r.permissions) = (vec!["2.1".into(), "14.5".into()], vec!["accessibility".into()]);
        let env = EnvFacts { target_version: Some("2.1".into()), os_version: "14.5".into(), permissions: vec!["accessibility".into(), "screen".into()] };
        assert!(!needs_reverify(&r, &env), "同じ版・OS・権限は確かめ直さない");
        assert!(needs_reverify(&r, &EnvFacts { target_version: Some("2.2".into()), ..env.clone() }), "app の版が違う");
        assert!(needs_reverify(&r, &EnvFacts { os_version: "15.0".into(), ..env.clone() }), "OS が違う");
        assert!(needs_reverify(&r, &EnvFacts { permissions: vec!["screen".into()], ..env.clone() }), "確かめた時の権限が無い");
        assert!(needs_reverify(&CapabilityRecord { verification: Verification::Unverified, ..r }, &env), "確かめていない記録は確かめる");
    }

    #[test]
    fn w19_2_choose_takes_only_verified_and_prefers_the_target_then_official() {
        let gen = Origin::Generated { artifact: "quillpad".into() };
        let quill = Reuse::AppSpecific { app: QUILL_APP.into() };
        // 調べただけ（MCP の一覧に在るだけ・解析の候補）の能力は、点が高くても選ばない（19 通目 §5・負の対照の要）
        let only_found = [
            rec("mcp.quill.quill_set_title", Reuse::Shared, Origin::Official, Verification::Unverified),
            rec("rea.quill.set", quill.clone(), Origin::Analysis { backend: "rea".into(), report: "a1".into() }, Verification::Unverified),
        ];
        assert_eq!(choose(&op(Some(QUILL_APP)), &only_found), None);
        let mut found = only_found.to_vec();
        found.push(rec("quillpad.doc.title.set", quill.clone(), gen.clone(), verified()));
        found.push(rec("other.doc.title.set", Reuse::AppSpecific { app: "org.example.other".into() }, Origin::Official, verified()));
        let c = choose(&op(Some(QUILL_APP)), &found).unwrap();
        assert_eq!((c.capability.as_str(), c.score.feasibility, c.score.stability), ("quillpad.doc.title.set", 3, 2), "{c:?}");
        assert_eq!(choose(&op(None), &found), None, "対象の無い操作に app 専用の能力を当てない");
        found.push(rec("quillpad.doc.title.official", quill, Origin::Official, verified()));
        assert_eq!(choose(&op(Some(QUILL_APP)), &found).unwrap().capability, "quillpad.doc.title.official", "同じ対象なら公式の連携");
    }

    #[test]
    fn w19_2_store_round_trips_and_refuses_bad_names() {
        let d = tmp("store");
        let caps = d.join(CAPABILITIES_DIR);
        let r = rec("quillpad.doc.read", Reuse::Shared, Origin::Generated { artifact: "quillpad".into() }, verified());
        assert_eq!(save_record(&caps, &r).unwrap(), caps.join("quillpad.doc.read.json"));
        std::fs::write(caps.join("broken.json"), "{").unwrap();
        std::fs::write(caps.join("other.json"), serde_json::to_string(&CapabilityRecord { schema: "shibubu.capability/0".into(), ..r.clone() }).unwrap()).unwrap();
        assert_eq!(load_records(&caps).unwrap(), vec![r.clone()], "壊れた file と schema の違う file は使わない");
        assert_eq!(load_records(&d.join("none")).unwrap(), vec![], "dir が無ければ空");
        assert!(matches!(save_record(&caps, &CapabilityRecord { id: "../x".into(), ..r.clone() }), Err(EngineError::Store(_))));
        let names: Vec<String> = std::fs::read_dir(&caps).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.contains(".tmp-")), "一時 file を残さない: {names:?}");
        let recipe = Recipe {
            schema: RECIPE_SCHEMA.into(),
            id: "quillpad".into(),
            provider: "quillpad".into(),
            reuse: Reuse::Shared,
            origin: Origin::Generated { artifact: "quillpad".into() },
            versions: vec![],
            actions: vec![],
        };
        let ext = d.join(EXTENSIONS_DIR);
        let p = save_recipe(&ext, &recipe).unwrap();
        assert_eq!(p, ext.join("quillpad").join(RECIPE_FILE));
        assert_eq!(serde_json::from_slice::<Recipe>(&std::fs::read(&p).unwrap()).unwrap(), recipe);
        assert!(matches!(save_recipe(&ext, &Recipe { id: "..".into(), ..recipe.clone() }), Err(EngineError::Store(_))));
        assert!(matches!(save_recipe(&ext, &Recipe { schema: "x".into(), ..recipe }), Err(EngineError::Store(_))));
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "wf-1".into(),
            goal: "g".into(),
            steps: vec![WorkflowStep { kind: StepKind::Action, id: "a".into(), capability: "alpha.text.read".into(), input: json!({}) }],
            ui: None,
            checks: vec![rb("/text", Expect::Changed)],
        };
        let wdir = d.join(WORKFLOWS_DIR);
        save_workflow(&wdir, &wf).unwrap();
        assert!(matches!(save_workflow(&wdir, &Workflow { steps: vec![], ..wf.clone() }), Err(WorkflowError::Invalid(_))), "形の違う手順は書かない");
        std::fs::write(wdir.join("bad.json"), json!({ "schema": WORKFLOW_SCHEMA, "id": "b", "goal": "", "steps": [] }).to_string()).unwrap();
        assert_eq!(load_workflows(&wdir).unwrap(), vec![wf], "validate に通らない保存は使わない・再起動の後も読める");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_2_interpret_splits_with_the_model_and_falls_back_without_it() {
        let fake = Fake::new(vec![Ok(json!({ "operations": [
            { "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "題を変える", "success": "題が新しい値", "target": QUILL_APP },
            { "kind": "act", "named": false, "candidates": [], "id": "op2", "want": "本文を読む", "success": "本文が読める", "target": null },
        ] }))]);
        let ops = interpret("Quillpad の題を変えて本文を読んで", &NO_SITUATION, Some(&fake), &NO_STOP).unwrap();
        assert_eq!(ops.iter().map(|o| (o.id.as_str(), o.target.as_deref())).collect::<Vec<_>>(), [("op1", Some(QUILL_APP)), ("op2", None)]);
        assert_eq!(fake.requests.lock().unwrap()[0].name, INTERPRET_NAME, "schema は本物の complete_json の送る前の検査に通った");
        assert_eq!(interpret("  題を変えて ", &NO_SITUATION, None, &NO_STOP).unwrap(), vec![Operation { id: "op1".into(), want: "題を変えて".into(), success: String::new(), target: None, ..Default::default() }]);
        assert!(matches!(interpret(" ", &NO_SITUATION, None, &NO_STOP), Err(EngineError::Denied(_))));
        let dup = Fake::new(vec![Ok(json!({ "operations": [
            { "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "a", "success": "", "target": null }, { "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "b", "success": "", "target": null } ] }))]);
        assert!(matches!(interpret("x", &NO_SITUATION, Some(&dup), &NO_STOP), Err(EngineError::Model(_))), "id の重なり");
        let down = Fake::new(vec![Err(LlmError::PlanLimit { detail: "resets 3pm".into() })]);
        assert!(matches!(interpret("x", &NO_SITUATION, Some(&down), &NO_STOP), Err(EngineError::Model(m)) if m.contains("resets 3pm")));
    }

    #[test]
    fn w19_2_generate_stamps_generated_unverified_and_refuses_unsafe_answers() {
        let d = tmp("gen");
        let real = d.join("real").join("notes.quill.json");
        let o = op(Some(QUILL_APP));
        let tdir = trial_dir(&d, &o);
        let ok = quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo");
        let mut reg = AdapterRegistry::default();
        let (mcp, mut an) = (quill_cfg("", &d.join("mcp.jsonl")), Vec::new());
        let found = mcp_records(&mcp, &[]);
        let gen_with = |reg: &mut AdapterRegistry, an: &mut Vec<Box<dyn AnalysisBackend>>, answer: Value| {
            let fake = Fake::new(vec![Ok(answer)]);
            let mut c = cx(reg, &d, Some(&fake), &mcp, &[], an);
            generate(&o, &found, &mut c, None)
        };
        let g = gen_with(&mut reg, &mut an, ok.clone()).unwrap();
        let r = g.recipe.as_ref().unwrap();
        assert_eq!(r.origin, Origin::Generated { artifact: "quillpad".into() });
        assert_eq!(r.reuse, Reuse::AppSpecific { app: QUILL_APP.into() });
        assert!(r.actions.iter().all(|a| a.verification == Verification::Unverified), "作っただけの能力は未検証（Engine が付ける）");
        assert_eq!(r.actions.iter().map(|a| (a.id.as_str(), a.approval)).collect::<Vec<_>>(), [("quillpad.doc.title.set", true), ("quillpad.doc.read", false)], "書く action は承認が要る");
        assert_eq!(g.workflow.steps[0].input, json!({ "path": real, "title": "Shibubu Demo" }));
        assert_eq!(g.workflow.checks[0].expect, Expect::Equals { value: json!("Shibubu Demo") });
        let refuse = |reg: &mut AdapterRegistry, an: &mut Vec<Box<dyn AnalysisBackend>>, edit: &dyn Fn(&mut Value)| {
            let mut a = ok.clone();
            edit(&mut a);
            gen_with(reg, an, a)
        };
        let gen_err = |r: Result<Generated, EngineError>| matches!(r, Err(EngineError::Generation(_)));
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["checks"][0]["action"] = json!("quillpad.doc.title.set"))), "書く action で読み戻さない");
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["workflow_steps"][0]["capability"] = json!("quillpad.doc.delete"))), "無い action");
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["trial"] = json!(null))), "試験の無い Adapter");
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["checks"] = json!([]))), "読み戻しの無い手順");
        let real_s = json!({ "path": real, "title": "x" }).to_string();
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["trial"]["steps"][0]["input_json"] = json!(real_s))), "試験が本物の file を指す");
        let escape = json!({ "path": format!("{}/../../real/notes.quill.json", tdir.display()), "title": "x" }).to_string();
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["trial"]["steps"][0]["input_json"] = json!(escape))), "試験の置き場から .. で出る");
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["recipe"]["actions"][0]["effect"] = json!("external_send"))), "外へ送る action は試せない");
        let gui = json!({ "via": "gui", "steps": [] }).to_string();
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["recipe"]["actions"][0]["call_json"] = json!(gui))), "recipe から AX を直に送らない");
        assert!(gen_err(refuse(&mut reg, &mut an, &|a| a["workflow_steps"][0]["input_json"] = json!(r#"{"token":"ghp_0123456789abcdef"}"#))), "鍵らしい値");
        assert!(matches!(refuse(&mut reg, &mut an, &|a| a["workflow_steps"][0]["input_json"] = json!("{")), Err(EngineError::Model(_))), "input が JSON でない");
        assert!(matches!(refuse(&mut reg, &mut an, &|a| a["recipe"]["actions"][0]["call_json"] = json!(r#"{"via":"mcp"}"#)), Err(EngineError::Model(_))), "call の形");
        // 既存の registry の action だけで足りる答え（recipe と trial が null）は通る
        let (a, _, _) = probe("alpha", &ALPHA, "", false);
        reg.register(Box::new(a)).unwrap();
        let only_registry = json!({
            "none_why": null, "workflow_steps": [{ "id": "w", "capability": "alpha.text.write", "input_json": json!({ "text": "x" }).to_string() }],
            "checks": [{ "action": "alpha.text.read", "input_json": "{}", "pointer": "/text", "expect": "equals", "value_json": "\"x\"" }],
            "recipe": null, "trial": null,
        });
        let g2 = gen_with(&mut reg, &mut an, only_registry).unwrap();
        assert_eq!((g2.recipe, g2.trial), (None, None));
        let mut c = cx(&mut reg, &d, None, &mcp, &[], &mut an);
        assert!(matches!(generate(&o, &[], &mut c, None), Err(EngineError::Model(_))), "model が無ければ作れない");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn integ_demo_c_generate_gives_the_model_only_the_target_apps_windows() {
        // demo #20 と owner 10-25「TextEdit だけでなく実際の desktop app（REAPER・Zoom・Discord）」: 手順の pid・window_id を model に推測させない。
        // 渡すのは宛先の app の今の窓だけ（題名 = 文書名・Discord の channel 名は個人の情報）。窓の形は operator が本物の app で測った物（pane-INTEG-demo-c の追記）
        static DESK: [Spec; 1] =
            [Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
        let w = |pid: u32, id: i64, app: &str, bundle: &str, title: &str| json!({ "pid": pid, "window_id": id, "app_name": app, "title": title, "bundle_id": bundle });
        let textedit = w(41, 7001, "TextEdit", "com.apple.TextEdit", "README.md");
        // REAPER: 窓 3（主窓・評価版の窓・Mixer）・AXDocument 無し
        let reaper = [
            w(52, 8101, "REAPER", "com.cockos.reaper", "untitled project - REAPER v7.81"),
            w(52, 8102, "REAPER", "com.cockos.reaper", "REAPER Evaluation License"),
            w(52, 8103, "REAPER", "com.cockos.reaper", "Mixer"),
        ];
        let discord = w(63, 9001, "Discord", "com.hnc.Discord", "#general | 友だちのサーバー");
        let d = tmp("gen-windows");
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        // 緩い Adapter: 頼んだ bundle の外の窓も混ぜて返す（Core の絞りだけを測る）
        let listing = json!([textedit, reaper[0], reaper[1], reaper[2], discord]).to_string();
        let (desk, list, calls) = probe("desktop", &DESK, &listing, false);
        let (alpha, _, _) = probe("alpha", &ALPHA, "", false);
        let answer = json!({
            "none_why": null, "workflow_steps": [{ "id": "w", "capability": "alpha.text.write", "input_json": json!({ "text": "x" }).to_string() }],
            "checks": [{ "action": "alpha.text.read", "input_json": "{}", "pointer": "/text", "expect": "equals", "value_json": "\"x\"" }],
            "recipe": null, "trial": null,
        });
        // model が受け取った input（Fake が読み戻す）。答えは毎回同じ = 窓が読めても読めなくても generate は止まらない
        let sent = |reg: &mut AdapterRegistry, an: &mut Vec<Box<dyn AnalysisBackend>>, target: Option<&str>| -> Value {
            let fake = Fake::new(vec![Ok(answer.clone())]);
            let g = generate(&op(target), &[], &mut cx(reg, &d, Some(&fake), &mcp, &[], an), None).unwrap();
            assert_eq!(g.workflow.steps.len(), 1);
            let input = fake.requests.lock().unwrap()[0].input.clone();
            serde_json::from_str(&input).unwrap()
        };
        let none = |v: Value| v.get("windows").is_none();
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(alpha)).unwrap();
        assert!(none(sent(&mut reg, &mut an, Some("com.apple.TextEdit"))), "desktop の action が registry に無い = 今までと同じ input");
        reg.register(Box::new(desk)).unwrap();
        assert_eq!(sent(&mut reg, &mut an, Some("com.apple.TextEdit"))["windows"], json!([textedit]), "宛先の app の窓だけ（Discord の channel 名を渡さない）");
        assert_eq!(sent(&mut reg, &mut an, Some("com.cockos.reaper"))["windows"], json!(reaper), "Core は Adapter が返した宛先の app の窓を全部渡す（主窓への絞りは Adapter の main = MacGui の pick_by_role・この偽の Adapter は絞らない）");
        assert_eq!(sent(&mut reg, &mut an, Some("com.hnc.Discord"))["windows"], json!([discord]));
        assert!(none(sent(&mut reg, &mut an, Some("us.zoom.xos"))), "宛先の app の窓が 0 = key を入れない");
        calls.lock().unwrap().clear();
        assert!(none(sent(&mut reg, &mut an, None)), "宛先が分からない = 渡さない");
        assert!(calls.lock().unwrap().is_empty(), "宛先の無い操作では窓の一覧を読みもしない");
        // Zoom: 混雑で AX が答えない = 窓が無いとは言わず、key を入れないだけ（generate は続く）
        *list.lock().unwrap() = "(no answer)".into();
        assert!(none(sent(&mut reg, &mut an, Some("us.zoom.xos"))));
        // Discord: 起動直後に自分を更新して pid と窓の id が変わる = generate ごとに読み直す（前の pid を渡さない）
        let updated = w(64, 9002, "Discord", "com.hnc.Discord", "#general | 友だちのサーバー");
        *list.lock().unwrap() = json!([updated]).to_string();
        assert_eq!(sent(&mut reg, &mut an, Some("com.hnc.Discord"))["windows"], json!([updated]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn g76_the_qa_trace_tells_where_a_run_stopped_without_titles_or_bodies() {
        // G76（r18 #20: 本物の AI で NoMeans・遺物からは (A) 窓が model に渡らない / (B) 渡ったが手順 0 / (C) interpret の宛先が Page の app と違う を分けられない）:
        // QA の run（`EngineContext::trace`）だけ段ごとの 1 行を `<data>/engine-trace.jsonl` に書き、どの段で何を入れて何が返ったかを 1 本の file で読む
        static DESK: [Spec; 1] =
            [Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
        let w = |pid: u32, id: i64, app: &str, bundle: &str, title: &str| json!({ "pid": pid, "window_id": id, "app_name": app, "title": title, "bundle_id": bundle });
        // 緩い Adapter（頼んだ bundle の外の窓も返す）: Discord の窓の題名（個人の情報）が trace に出ない事も測る
        let both = json!([w(41, 7001, "TextEdit", "com.apple.TextEdit", "README.md"), w(63, 9001, "Discord", "com.hnc.Discord", "#general | 友だちのサーバー")]).to_string();
        let none = json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let goal = "README の題を「Shibubu Demo」にして保存して";
        // 1 回の run（Page = TextEdit の README.md の面・r18 の Architecture の Page）→ (答え, trace の行, file の字, 窓の一覧を読んだ回数)
        let ask = |listing: &str, target: Value, ready: bool, trace: bool| {
            let d = tmp("g76");
            let mut reg = AdapterRegistry::default();
            let (mut desk, _, calls) = probe("desktop", &DESK, listing, false);
            desk.ready = ready;
            reg.register(Box::new(desk)).unwrap();
            let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "README の題を「Shibubu Demo」にして保存する", "success": "README の題が Shibubu Demo", "target": target }] });
            // WF-010: 手順 0 は理由つきで 1 回作り直す = 0 手の答えを 2 つ
            let fake = Fake::new(vec![Ok::<Value, LlmError>(ops), Ok(none.clone()), Ok(none.clone())]);
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let r = run(goal, &mut EngineContext { target: Some("com.apple.TextEdit"), document: Some("README.md"), trace, ..cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an) });
            let text = std::fs::read_to_string(d.join(TRACE_FILE)).unwrap_or_default();
            let rows: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
            let n = calls.lock().unwrap().len();
            let _ = std::fs::remove_dir_all(&d);
            (r, rows, text, n)
        };
        // 同じ段が 2 行（WF-010 の作り直しの plan）なら最後の行
        let stage = |rows: &[Value], s: &str| rows.iter().rev().find(|r| r["stage"] == s).cloned().unwrap_or(Value::Null);
        let no_means = |r: &Result<EngineRecord, EngineError>| matches!(r, Err(EngineError::NoMeans { .. }));

        // 1. r18 の形（(B) の再現）: 窓は渡った・model が手順 0 → 今の NoMeans。plan 行に steps 0・windows_given true・渡した action
        let (r, rows, text, _) = ask(&both, Value::Null, true, true);
        assert!(no_means(&r), "{r:?}");
        assert_eq!(rows.iter().map(|r| r["stage"].as_str().unwrap_or("?")).collect::<Vec<_>>(), ["ask", "interpret", "target", "plan", "plan", "approval"], "{text}");
        assert!(rows.iter().all(|r| r.get("run_id").is_some() && r["ms"].is_u64() && (r["result"] == "ok" || r["result"] == "stop")), "{text}");
        let (k, i, t, p, a) = (stage(&rows, "ask"), stage(&rows, "interpret"), stage(&rows, "target"), stage(&rows, "plan"), stage(&rows, "approval"));
        assert_eq!((&k["goal_chars"], &k["page_target"], &k["page_document"]), (&json!(goal.chars().count()), &json!("com.apple.TextEdit"), &json!("README.md")));
        assert_eq!((&i["page_filled"], &i["operations"][0]["target"], &i["operations"][0]["want"]), (&json!(true), &json!("com.apple.TextEdit"), &json!("README の題を「Shibubu Demo」にして保存する")));
        assert_eq!((&t["same_as_page"], &t["document"], &t["desktop"]), (&json!(true), &json!("README.md"), &json!("ready")));
        assert_eq!((&t["windows_status"], &t["listed"], &t["windows_count"]), (&json!("ready"), &json!(2), &json!(1)), "Adapter が返した 2 つ・Core が宛先の app に絞って 1 つ");
        let win = &t["windows"][0];
        assert_eq!((&win["bundle_id"], &win["document"], t["windows"].as_array().map(Vec::len)), (&json!("com.apple.TextEdit"), &json!("match"), Some(1)), "{t}");
        let id = win["title_id"].as_str().unwrap_or_default().to_string();
        assert!(id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()), "題名の代わりの匿名 id: {t}");
        assert_ne!(id, crate::crypto::sha256_hex(b"README.md")[..8], "塩の無い hash = 辞書で題名に戻せる");
        assert_eq!((&p["result"], &p["windows_given"], &p["steps"], &p["checks"], &p["recipe"]), (&json!("stop"), &json!(true), &json!(0), &json!(0), &json!(false)));
        assert_eq!((&p["actions"], &p["actions_count"]), (&json!(["desktop.windows.list"]), &json!(1)));
        assert_eq!((&p["answer"], &p["answer_truncated"]), (&json!(none.to_string()), &json!(false)), "生の答え");
        assert!(p["stop_reason"].as_str().is_some_and(|s| s.starts_with("NoMeans: ")), "{p}");
        assert_eq!((&a["result"], &a["stop_reason"]), (&json!("stop"), &p["stop_reason"]), "run の最後の行 = 止まった所の理由");
        // 題名・利用者の一言そのもの・“” の中は書かない（NoMeans の文の “操作” は字数だけ）
        assert!(!text.contains("友だち") && !text.contains("#general") && !text.contains(goal), "{text}");
        assert!(p["stop_reason"].as_str().is_some_and(|s| s.contains("“") && !s.contains("Shibubu Demo")), "{p}");

        // 2. (A) の再現: desktop.windows.list が Ready でない → 窓を読まない・target 行に readiness・plan の windows_given false（同じ NoMeans に潰れる）
        let (r, rows, text, n) = ask(&both, Value::Null, false, true);
        assert!(no_means(&r), "{r:?}");
        let (t, p) = (stage(&rows, "target"), stage(&rows, "plan"));
        assert_eq!((&t["windows_status"], &t["desktop"], &t["windows_count"]), (&json!("not_ready"), &json!("requires_connection"), &json!(0)), "{text}");
        assert_eq!((&p["windows_given"], &p["steps"]), (&json!(false), &json!(0)));
        assert_eq!(n, 0, "Ready でない action は呼ばない");

        // (A) の他の 2 つ: 答えない（read_failed）・読めたが窓の形でない（decode_failed・serde の理由を残す = 今の code は黙って空）
        let (_, rows, text, _) = ask("(no answer)", Value::Null, true, true);
        assert_eq!((&stage(&rows, "target")["windows_status"], &stage(&rows, "plan")["windows_given"]), (&json!("read_failed"), &json!(false)), "{text}");
        let (_, rows, text, _) = ask(r#"{"windows": []}"#, Value::Null, true, true);
        let t = stage(&rows, "target");
        assert_eq!((&t["windows_status"], &stage(&rows, "plan")["windows_given"]), (&json!("decode_failed"), &json!(false)), "{text}");
        assert!(t["decode_error"].as_str().is_some_and(|e| !e.is_empty()), "{t}");

        // 3. (C) の再現: interpret が bundle id でない宛先を返す → G-a が Page の app で埋めない・書類を渡さない・宛先の窓 0 → Denied でなく同じ NoMeans
        // （G76 で見つけた穴。直すのは本物の trace で (A)(B)(C) が決まってから）
        let (r, rows, text, _) = ask(&both, json!("README.md"), true, true);
        assert!(no_means(&r), "{r:?}");
        let (i, t, p) = (stage(&rows, "interpret"), stage(&rows, "target"), stage(&rows, "plan"));
        assert_eq!((&i["page_filled"], &i["operations"][0]["target"]), (&json!(false), &json!("README.md")), "{text}");
        // WF-008: 読めたが宛先の app の窓が 0 個 = empty（ready と分ける）
        assert_eq!((&t["same_as_page"], &t["document"], &t["windows_status"], &t["windows_count"]), (&json!(false), &Value::Null, &json!("empty"), &json!(0)));
        assert_eq!(p["windows_given"], json!(false));

        // 4. 既定（QA でない run）は file を作らない
        let (r, rows, text, _) = ask(&both, Value::Null, true, false);
        assert!(no_means(&r) && rows.is_empty() && text.is_empty(), "{text}");

        // 窓の匿名 id と名指しの書類との一致（owner の review 3 回目）: 同じ app の窓 3 つ = 一致 / 不一致 / 題名が読めない。
        // 同じ起動の中の別の run でも同じ題名 = 同じ id（run をまたいで突き合わせる）・題名そのものは書かない
        let three = json!([
            w(41, 7001, "TextEdit", "com.apple.TextEdit", "README.md"),
            w(41, 7002, "TextEdit", "com.apple.TextEdit", "private-notes.md"),
            w(41, 7003, "TextEdit", "com.apple.TextEdit", ""),
        ])
        .to_string();
        let (_, rows, text, _) = ask(&three, Value::Null, true, true);
        let ws = stage(&rows, "target")["windows"].clone();
        let judged: Vec<&Value> = ws.as_array().into_iter().flatten().map(|w| &w["document"]).collect();
        assert_eq!(judged, [&json!("match"), &json!("mismatch"), &json!("unreadable")], "{text}");
        assert_eq!((&ws[0]["title_id"], &ws[2]["title_id"]), (&json!(id), &Value::Null), "前の run の README.md と同じ id・題名が空なら id 無し");
        assert!(ws[1]["title_id"].is_string() && ws[1]["title_id"] != ws[0]["title_id"]);
        assert!(!text.contains("private-notes"), "{text}");

        // 字の門: home の path・鍵らしい物・“” の中
        let home = std::env::var("HOME").unwrap_or_default();
        assert!(home.len() > 1);
        assert_eq!(scrub(&format!("{home}/notes “README.md” isn't open")), "~/notes “9 chars” isn't open");
        assert!(!scrub("token Bearer abc").contains("abc"));
        let (raw, cut) = trace_raw(&format!("{home}/{}", "あ".repeat(2000)));
        assert!(cut && raw.len() <= TRACE_CAP && raw.starts_with("~/") && raw.ends_with('あ'), "{} {cut}", raw.len());
        assert!(!trace_raw("{\"x\":\"sk-ant-1\"}").0.contains("sk-ant-"));
    }

    #[test]
    fn g77_the_qa_trace_names_the_windows_when_the_read_before_fails() {
        // G77（operator の追記 ①・r19 #20: 手を走らせる前の読みが「3 windows of TextEdit … can't tell which」で止まったが、3 つの正体〈どの pid の・どれが
        // 名指しの書類か〉はどこにも残らなかった）: 前読みが読めない時、QA の trace に before 行 = 読めなかった読みの action・input の key（値は書かない）・
        // その宛先の app の今の窓の全部（main で絞らない・全 pid の pid・窓番号・題名の匿名 id・書類との一致）
        static DESK: [Spec; 1] =
            [Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
        let te = "com.apple.TextEdit";
        let w = |pid: u32, id: i64, title: &str| json!({ "pid": pid, "window_id": id, "app_name": "TextEdit", "title": title, "bundle_id": te });
        let three = json!([w(41, 7001, "README.md"), w(41, 7003, ""), w(42, 7002, "private-notes.md")]).to_string();
        let ask = |trace: bool| {
            let d = tmp("g77");
            let mut reg = AdapterRegistry::default();
            let (desk, _, calls) = probe("desktop", &DESK, &three, false);
            let (alpha, _, _) = probe("alpha", &ALPHA, "", false);
            // 読めない読み（接続の要る Adapter = registry が Rejected）
            let (mut beta, _, _) = probe("beta", &BETA, "", false);
            beta.ready = false;
            for a in [desk, alpha, beta] {
                reg.register(Box::new(a)).unwrap();
            }
            let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "w", "success": "s", "target": null }] });
            let answer = json!({
                "none_why": null, "workflow_steps": [{ "id": "w", "capability": "alpha.text.write", "input_json": json!({ "text": "x" }).to_string() }],
                "checks": [{ "action": "beta.text.read", "input_json": json!({ "bundle_id": te, "main": true }).to_string(), "pointer": "/text", "expect": "equals", "value_json": "\"x\"" }],
                "recipe": null, "trial": null,
            });
            let fake = Fake::new(vec![Ok::<Value, LlmError>(ops), Ok(answer)]);
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let r = run("README を直して", &mut EngineContext { target: Some(te), document: Some("README.md"), trace, ..cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an) });
            let text = std::fs::read_to_string(d.join(TRACE_FILE)).unwrap_or_default();
            let n = calls.lock().unwrap().len();
            let _ = std::fs::remove_dir_all(&d);
            (r, text, n)
        };
        let (r, text, n) = ask(true);
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.why.contains("couldn't be read first")), "{r:?}");
        let rows: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.iter().map(|r| r["stage"].as_str().unwrap_or("?")).collect::<Vec<_>>(), ["ask", "interpret", "target", "plan", "validate", "before", "approval"], "{text}");
        let b = &rows[5];
        assert_eq!((&b["result"], &b["action"], &b["input_keys"]), (&json!("stop"), &json!("beta.text.read"), &json!(["bundle_id", "main"])), "{b}");
        let seen: Vec<(Value, Value, Value)> = b["windows"].as_array().into_iter().flatten().map(|w| (w["pid"].clone(), w["window_id"].clone(), w["document"].clone())).collect();
        assert_eq!(seen, [(json!(41), json!(7001), json!("match")), (json!(41), json!(7003), json!("unreadable")), (json!(42), json!(7002), json!("mismatch"))], "{b}");
        assert!(!text.contains("private-notes"), "{text}");
        // QA でない run は before の窓を読まない（窓の一覧を読む回数 = 生成の前の 1 回だけ）
        let (r, text, plain) = ask(false);
        assert!(matches!(r, Err(EngineError::Denied(_))) && text.is_empty(), "{text}");
        assert_eq!((plain, n), (1, 2), "QA の run だけ before で 1 回多く読む");
    }

    /// G79（r20 #20: 本物の計画 = cmd+↑・cmd+shift+→・全文の type_text・cmd+s）: 書類の窓が 1 つに決まれば本文の頭（先頭 40 行・2 KB）と本文の欄の role・label を
    /// plan の input の document_text に。計画に修飾 key 付きの desktop.key が在れば validate で落とし、理由を rejected_plan に入れて計画だけ 1 回作り直す
    /// （窓は読み直さない・何も送らない）。2 回目も在れば Denied = 人には「送っていない」の 1 行。trace の plan 行は書類の頭を字数と hash だけ・答えの replace も。
    /// 負の対照（G79_MUT=gate = 門を外す）→ 1 回目の計画が承認待ちまで進み赤
    #[test]
    fn g79_a_plan_with_shortcuts_is_rebuilt_once_and_the_document_head_is_given() {
        static DESK: [Spec; 3] = [
            Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.key", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.ax.type_text", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        ];
        static STATE: [Spec; 1] =
            [Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
        let te = "com.apple.TextEdit";
        let readme = json!([{ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "title": "README.md", "bundle_id": te }]).to_string();
        let state = |body: &str| {
            let c = |role: &str, value: Option<&str>, settable: bool| json!({ "label": "", "role": role, "value": value, "actions": [], "value_settable": settable });
            json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "complete": true, "controls": [c("AXTextArea", Some(body), true), c("AXButton", None, false)] }).to_string()
        };
        let key = |id: &str, k: &str, mods: &[&str]| step_answer(id, "desktop.key", json!({ "pid": 41, "app_name": "TextEdit", "key": k, "mods": mods }));
        let typed = |extra: Value| {
            let mut i = json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "label": "", "role": "AXTextArea", "text": "# Shibubu Demo" });
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            step_answer("s3", "desktop.ax.type_text", i)
        };
        let check = json!([{ "action": "alpha.text.read", "input_json": "{}", "pointer": "/text", "expect": "changed", "value_json": null }]);
        let r20 = json!({ "none_why": null, "workflow_steps": [key("s1", "up", &["cmd"]), key("s2", "right", &["cmd", "shift"]), typed(json!({})), key("s4", "s", &["cmd"])], "checks": check, "recipe": null, "trial": null });
        let fixed = json!({ "none_why": null, "workflow_steps": [typed(json!({ "replace": "# README" }))], "checks": check, "recipe": null, "trial": null });
        let lines = format!("# README\n{}", (2..=45).map(|n| format!("line {n}\n")).collect::<String>());
        let ask = |second: &Value, body: &str| {
            let d = tmp("g79");
            let mut reg = AdapterRegistry::default();
            let (desk, _, calls) = probe("desktop", &DESK, &readme, false);
            let (st, _, reads) = probe("deskstate", &STATE, &state(body), false);
            let (alpha, _, _) = probe("alpha", &ALPHA, "", false);
            for a in [desk, st, alpha] {
                reg.register(Box::new(a)).unwrap();
            }
            let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "w", "success": "s", "target": null }] });
            let fake = Fake::new(vec![Ok::<Value, LlmError>(ops), Ok(r20.clone()), Ok(second.clone())]);
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let r = run("README の題を直して保存して", &mut EngineContext { target: Some(te), document: Some("README.md"), trace: true, ..cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an) });
            let text = std::fs::read_to_string(d.join(TRACE_FILE)).unwrap_or_default();
            let gens: Vec<Value> = fake.requests.lock().unwrap().iter().filter(|q| q.name == GENERATE_NAME).map(|q| serde_json::from_str(&q.input).unwrap()).collect();
            let sent = (calls.lock().unwrap().clone(), reads.lock().unwrap().len());
            let _ = std::fs::remove_dir_all(&d);
            (r, text, gens, sent)
        };
        let stages = |text: &str| text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["stage"].as_str().unwrap_or("?").to_string()).collect::<Vec<_>>();
        // 2 回目も shortcut → 止まる・何も送っていない
        let (r, text, gens, (calls, reads)) = ask(&r20, &lines);
        let Err(e @ EngineError::Denied(_)) = &r else { panic!("{r:?}") };
        let line = crate::engine_page::failure_line(e);
        assert!(line.contains("shortcut keys (cmd+up) in TextEdit") && line.contains("nothing was sent"), "{line}");
        assert_eq!(stages(&text), ["ask", "interpret", "target", "plan", "validate", "plan", "validate", "approval"], "{text}");
        assert_eq!((calls, reads), (vec!["desktop.windows.list".to_string()], 1), "窓の一覧と書類の欄を 1 回ずつ読むだけ・key は送らない・作り直しで読み直さない");
        assert_eq!(gens.len(), 2, "計画だけ 1 回作り直す");
        let given = &gens[0]["document_text"];
        assert_eq!((&given["role"], &given["label"], &given["document"]), (&json!("AXTextArea"), &json!(""), &json!("README.md")), "{given}");
        let head = given["head"].as_str().unwrap_or_default();
        assert!(head.starts_with("# README\nline 2\n") && head.lines().count() == 40, "先頭 40 行: {head:?}");
        assert!(gens[0].get("rejected_plan").is_none() && gens[1]["rejected_plan"].as_str().is_some_and(|w| w.contains("s1") && w.contains("replace")), "{}", gens[1]);
        assert_eq!(gens[1]["document_text"], gens[0]["document_text"], "作り直しにも同じ書類の頭");
        let rows: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let plans: Vec<&Value> = rows.iter().filter(|r| r["stage"] == "plan").collect();
        assert_eq!(plans.iter().map(|p| p["attempt"].clone()).collect::<Vec<_>>(), [json!(1), json!(2)]);
        assert_eq!(plans[0]["document_text"]["chars"], json!(head.chars().count()), "{}", plans[0]);
        assert!(plans[0]["document_text"]["hash"].as_str().is_some_and(|h| h.len() == 8), "{}", plans[0]);
        assert!(!text.contains("line 2") && !text.contains("# README"), "書類の本文を trace に書いた: {text}");
        // 2 回目が shortcut の無い計画 → 承認待ちまで進む・replace（書類の字）は trace に字数と hash だけ・長い 1 行の頭は 2 KB で字の途中で切らない
        let wide = "あ".repeat(1000);
        let (r, text, gens, (calls, _)) = ask(&fixed, &wide);
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.pending.is_some()), "{r:?}");
        assert_eq!(stages(&text), ["ask", "interpret", "target", "plan", "validate", "plan", "validate", "approval"], "{text}");
        assert_eq!(calls, ["desktop.windows.list"], "承認の前に何も送らない");
        assert_eq!(gens[0]["document_text"]["head"].as_str().map(str::len), Some(2046), "2 KB まで・字の途中で切らない");
        assert!(!text.contains("# README") && text.contains("Shibubu Demo"), "{text}");
    }

    /// G80（r22 #20: plan の document_text が null → model は空の計画 → NoMeans）: 本物の TextEdit の書類の窓（r23 の README・bridge の ax_tree の実測）は
    /// 本文の AXTextArea の隣に AXScrollBar 2 つを持ち、その AXValue は値を置ける数（"0" 等）= 値を置ける字の在る欄が 3 つで頭を渡していなかった。
    /// 本文の欄は AXTextArea だけで数える。渡せない時は trace の plan 行の document_text_why に理由（窓 / invoke の Outcome の種類 / decode / fields:N / secret・本文は出さない）。
    /// 負の対照: G80_MUT=role（値を置ける欄を全部数える）→ 実測の形で頭が渡らず赤 / G80_MUT=why（理由を書かない）→ 理由の表が赤
    #[test]
    fn g80_the_document_field_is_the_text_area_and_the_trace_says_why_it_is_not_given() {
        static DESK: [Spec; 2] = [
            Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.ax.type_text", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        ];
        static STATE: [Spec; 1] =
            [Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];
        let te = "com.apple.TextEdit";
        let readme = json!([{ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "title": "README.md", "bundle_id": te }]).to_string();
        let c = |role: &str, value: Option<&str>, settable: bool| json!({ "label": "", "role": role, "value": value, "actions": [], "value_settable": settable });
        let state = |controls: Vec<Value>| json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "complete": true, "controls": controls }).to_string();
        // 本物の TextEdit の書類の窓の形（窓そのものを除いた ax_tree の並び・scroll bar の値は実測と同じ数の字）
        let real = |area: &str| {
            vec![
                c("AXScrollArea", None, false),
                c("AXButton", None, false),
                c("AXStaticText", Some("README.md"), false),
                c("AXTextArea", Some(area), true),
                c("AXScrollBar", Some("0.4285714285714286"), true),
                c("AXScrollBar", Some("0"), true),
                c("AXGroup", None, false),
            ]
        };
        let body = "# shibubu-yc-demo\n\nThe repo of this run\n";
        let typed = json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "label": "", "role": "AXTextArea", "replace": "# shibubu-yc-demo", "text": "# Shibubu Demo" });
        let check = json!([{ "action": "alpha.text.read", "input_json": "{}", "pointer": "/text", "expect": "changed", "value_json": null }]);
        let plan = json!({ "none_why": null, "workflow_steps": [step_answer("s1", "desktop.ax.type_text", typed)], "checks": check, "recipe": null, "trial": null });
        // → (生成に渡した input, trace の plan 行)
        let ask = |shape: &str| {
            let d = tmp("g80");
            let mut reg = AdapterRegistry::default();
            let (desk, _, _) = probe("desktop", &DESK, &readme, false);
            let (st, _, _) = probe("deskstate", &STATE, shape, false);
            let (alpha, _, _) = probe("alpha", &ALPHA, "", false);
            for a in [desk, st, alpha] {
                reg.register(Box::new(a)).unwrap();
            }
            let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "w", "success": "s", "target": null }] });
            let fake = Fake::new(vec![Ok::<Value, LlmError>(ops), Ok(plan.clone())]);
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let r = run("Change the README title to “Shibubu Demo” and save it", &mut EngineContext { target: Some(te), document: Some("README.md"), trace: true, ..cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an) });
            assert!(matches!(&r, Err(EngineError::Denied(p)) if p.pending.is_some()), "どの形でも計画は承認待ちまで進む（頭が無くても止めない）: {r:?}");
            let text = std::fs::read_to_string(d.join(TRACE_FILE)).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&d);
            assert!(!text.contains("The repo of this run") && !text.contains("Bearer"), "書類の本文を trace に書いた: {text}");
            let input: Value = fake.requests.lock().unwrap().iter().find(|q| q.name == GENERATE_NAME).map(|q| serde_json::from_str(&q.input).unwrap()).unwrap();
            let row = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|r| r["stage"] == "plan").unwrap();
            (input, row)
        };
        // 実測の形 → 本文の欄は AXTextArea の 1 つ → 頭が渡る・理由は null
        let (input, row) = ask(&state(real(body)));
        assert_eq!((&input["document_text"]["head"], &input["document_text"]["role"]), (&json!(body), &json!("AXTextArea")), "{}", input["document_text"]);
        assert_eq!((&row["document_text"]["chars"], &row["document_text_why"]), (&json!(body.chars().count()), &Value::Null), "{row}");
        // 渡せない時は理由だけを trace に（本文は出さない）
        for (shape, why) in [
            (state(real("")), "fields:0"),
            (state(vec![c("AXTextArea", Some(body), false), c("AXScrollBar", Some("0"), true)]), "fields:0"),
            (state(vec![c("AXTextArea", Some(body), true), c("AXTextArea", Some("The other pane"), true)]), "fields:2"),
            (state(real("# shibubu-yc-demo\nBearer abc\n")), "secret"),
            (json!([{ "role": "AXTextArea" }]).to_string(), "decode"),
            ("the app didn't answer".to_string(), "invoke:failed:timeout"),
        ] {
            let (input, row) = ask(&shape);
            assert!(input.get("document_text").is_none(), "{why}: {}", input["document_text"]);
            assert_eq!((&row["document_text"], &row["document_text_why"]), (&Value::Null, &json!(why)), "{row}");
        }
    }

    /// G81 の試験の偽の TextEdit（書類の窓 1 つ・本文の欄 1 つ）。File › Save（`desktop.ax.menu`）は `save` の後に押した時の本文を `file` に書く
    /// （背景の app の保存が disk に遅れて届く・r25 #20 で ≈ 17 秒）・None = disk に何も書かない（保存が届かない）。
    struct Doc {
        body: Arc<Mutex<String>>,
        file: PathBuf,
        save: Option<Duration>,
        calls: Arc<Mutex<Vec<(String, Value)>>>,
    }
    const WINDOW: &str = r#"{"type":"object","properties":{"pid":{"type":"integer"},"window_id":{"type":"integer"},"app_name":{"type":"string"},"bundle_id":{"type":"string"},"document":{"type":"string"}}}"#;
    static DOC: [Spec; 4] = [
        Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: WINDOW, verifier: "read_back" },
        Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: WINDOW, verifier: "read_back" },
        Spec { id: "desktop.ax.type_text", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: WINDOW, verifier: "read_back" },
        Spec { id: "desktop.ax.menu", effect: Effect::ExternalWrite, approval: true, scopes: &[], input: WINDOW, verifier: "read_back" },
    ];
    impl Adapter for Doc {
        fn id(&self) -> &'static str {
            "desktop"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::MacGui
        }
        fn route(&self) -> Route {
            Route::OsAutomation
        }
        fn provider(&self) -> &'static str {
            "desktop"
        }
        fn specs(&self) -> &'static [Spec] {
            &DOC
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            self.calls.lock().unwrap().push((action.into(), input.clone()));
            let body = self.body.lock().unwrap().clone();
            let payload = match action {
                "desktop.windows.list" => json!([{ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "title": "README.md", "bundle_id": "com.apple.TextEdit" }]),
                "desktop.window.state" => json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "complete": true,
                    "controls": [{ "label": "", "role": "AXTextArea", "value": body, "actions": [], "value_settable": true }] }),
                "desktop.ax.type_text" => {
                    let (from, to) = (input["replace"].as_str().unwrap_or_default(), input["text"].as_str().unwrap_or_default());
                    *self.body.lock().unwrap() = body.replacen(from, to, 1);
                    return (Outcome::Unverified { reason: "置いた".into() }, Some(sdk::observe("desktop", sdk::now(), json!({ "set": true }))));
                }
                _ => {
                    if let Some(after) = self.save {
                        let file = self.file.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(after);
                            std::fs::write(file, body).unwrap();
                        });
                    }
                    return (Outcome::Unverified { reason: "押した".into() }, Some(sdk::observe("desktop", sdk::now(), json!({ "pressed": input["path"] }))));
                }
            };
            (Outcome::Verified { evidence: vec!["読んだ".into()] }, Some(sdk::observe("desktop", sdk::now(), payload)))
        }
        fn cleanup(&mut self) {}
        /// WF-006: MacGui の偽物（送る直前に宛先を読み直す扱い）
        fn prechecks(&self, _action: &str) -> bool {
            true
        }
    }

    /// G81（r25 #20: 本物の AI の計画 = 見出しの 1 行の replace ＋ File › Save・checks は窓の本文と窓の題名だけ → 「✓ Verified」が disk の保存より ≈ 17 秒早い）:
    /// Page の書類の disk の場所（`document_file`）が在れば、Engine が名指しの書類に字を置く手の済みを保存した file の中身（`git.file.read` の /text が置いた字を含む）
    /// にも縛る。保存が遅れて届く → 届いた後にだけ済み（その間 Page に「Changed in TextEdit · checking the saved file…」・送り直さない）/ 届かない → 時間で
    /// Unconfirmed（送ったが確かめられない・送り直さない）/ G81 の前に保存した手順（disk の check 無し）を使い直しても同じ。
    /// 負の対照: G81_MUT=add（disk の check を足さない = 前の形）→ 保存の前に済み・保存が届かなくても済みで赤。
    /// CU 1a655d7c #9（本物の AI の計画は手にも checks にも自分で document を書く）: Adapter に届く document の在る手・読みは全部 document_file を持つ。
    /// 負の対照: recipe の `ours` を「document が無い物だけ」に戻す → desktop.window.state に書類の場所が無くて赤
    #[test]
    fn g81_verified_only_after_the_saved_file_on_disk_has_the_text() {
        let te = "com.apple.TextEdit";
        let at = json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "bundle_id": te, "document": "README.md" });
        let with = |extra: Value| {
            let mut i = at.clone();
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            i
        };
        // r25 の計画の形（pid・窓番号は偽の 41・7001・見出しの字は試験の字）
        let plan = json!({
            "none_why": null, "workflow_steps": [
                step_answer("s1", "desktop.ax.type_text", with(json!({ "label": "", "role": "AXTextArea", "replace": "# shibubu-yc-demo", "text": "# Shibubu Demo" }))),
                step_answer("s2", "desktop.ax.menu", with(json!({ "path": ["File", "Save"] }))),
            ],
            "checks": [
                { "action": "desktop.window.state", "input_json": with(json!({ "role": "AXTextArea", "contains": "# Shibubu Demo" })).to_string(), "pointer": "/controls/0/value", "expect": "contains", "value_json": "\"# Shibubu Demo\"" },
                { "action": "desktop.windows.list", "input_json": json!({ "bundle_id": te, "main": true }).to_string(), "pointer": "/0/title", "expect": "contains", "value_json": "\"README.md\"" },
            ],
            "recipe": null, "trial": null,
        });
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Change the README title to “Shibubu Demo” and save it", "target": null,
            "success": "Reading the README file back from disk shows its title is exactly “Shibubu Demo”" }] });
        let body = "# shibubu-yc-demo\n\nThe repo of this run\n";
        // → (2 回目の run の答え, 2 回目の run の秒, 押した menu の数, Page の 1 行の列, disk の README)
        let ask = |save: Option<Duration>, wait: Duration, strip: bool| {
            let d = tmp("g81");
            let repo = d.join("repo");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::write(repo.join("README.md"), body).unwrap();
            let uri = format!("file://{}", repo.join("README.md").display());
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut reg = AdapterRegistry::with_local_adapters();
            reg.register(Box::new(Doc { body: Arc::new(Mutex::new(body.into())), file: repo.join("README.md"), save, calls: calls.clone() })).unwrap();
            let fake = Fake::new(vec![Ok::<Value, LlmError>(ops.clone()), Ok(plan.clone()), Ok(ops.clone()), Ok(plan.clone())]);
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let data = d.join("data");
            let said = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
            let heard = said.clone();
            let say = move |l: Option<String>| heard.lock().unwrap().push(l);
            let mut go = |ok: &[ConsentReceipt]| {
                let mut c = EngineContext { target: Some(te), document: Some("README.md"), document_file: Some(&uri), progress: Some(&say), readback: wait, ..cx(&mut reg, &data, Some(&fake), &mcp, ok, &mut an) };
                run("Change the README title to “Shibubu Demo” and save it", &mut c)
            };
            // 1 回目: 承認待ち（2 手とも）・何も送っていない
            let Err(EngineError::Denied(den)) = go(&[]) else { panic!("承認待ちで止まらない") };
            let ok: Vec<ConsentReceipt> = den.pending.iter().chain(&den.later).map(|p| consent(&p.action, &p.input)).collect();
            assert_eq!(ok.len(), 2, "置き換えと保存の 2 手");
            // G81 の前に保存した手順（disk の check 無し）を使い直す場面
            if strip {
                for f in std::fs::read_dir(data.join(WORKFLOWS_DIR)).unwrap().flatten().filter(|f| f.path().extension().is_some_and(|e| e == "json")) {
                    let mut w: Value = serde_json::from_slice(&std::fs::read(f.path()).unwrap()).unwrap();
                    w["checks"].as_array_mut().unwrap().retain(|c| c["action"] != "git.file.read");
                    std::fs::write(f.path(), w.to_string()).unwrap();
                }
            }
            let t = Instant::now();
            let r = go(&ok);
            let took = t.elapsed();
            let menus = calls.lock().unwrap().iter().filter(|(a, _)| a == "desktop.ax.menu").count();
            // CU 1a655d7c #9: model が自分で document を書いた手・読みにも Engine が書類の場所を足す（無いと同じ名前の別の README で Adapter が決められない）
            for (a, i) in calls.lock().unwrap().iter().filter(|(_, i)| i.get("document").is_some()) {
                assert_eq!(i["document_file"].as_str(), Some(uri.as_str()), "{a} に書類の場所が無い: {i}");
            }
            let disk = std::fs::read_to_string(repo.join("README.md")).unwrap();
            let lines = said.lock().unwrap().clone();
            let _ = std::fs::remove_dir_all(&d);
            (r, took, menus, lines, disk)
        };
        let checking = Some("Changed in TextEdit · checking the saved file…".to_string());
        // 保存が 1.5 秒遅れて届く → 届いた後にだけ済み・その間は確かめ中の 1 行・menu は 1 回だけ
        let (r, took, menus, lines, disk) = ask(Some(Duration::from_millis(1500)), Duration::from_secs(10), false);
        let rec = r.unwrap_or_else(|e| panic!("{e:?}"));
        assert!(disk.starts_with("# Shibubu Demo\n") && took >= Duration::from_millis(1500), "保存の前に済みと言った（{took:?}・disk {disk:?}）");
        // WF-007: 保存した file も頼んだ差分だけ（見出しの 1 か所）・#20 は Verified のまま（Partial にならない）
        assert!(rec.verified.iter().any(|v| v.starts_with("git.file.read の /text") && v.contains("→「# Shibubu Demo」")), "{:?}", rec.verified);
        assert_eq!(rec.partial, None, "{:?}", rec.verified);
        assert_eq!(menus, 1, "読み直しで File › Save を送り直さない");
        assert_eq!((lines.first(), lines.last()), (Some(&checking), Some(&None)), "{lines:?}");
        // 保存が届かない → 時間で Unconfirmed（送ったが確かめられない・送り直さない）
        let (r, _, menus, lines, disk) = ask(None, Duration::from_secs(2), false);
        let Err(e @ EngineError::Unconfirmed(why)) = &r else { panic!("保存が届かないのに: {r:?}") };
        assert!(why.contains("git.file.read の /text"), "{why}");
        assert!(crate::engine_page::failure_line(e).starts_with("Couldn't confirm whether it was sent, so it isn't marked done. It won't be sent again"), "{e}");
        assert_eq!((menus, disk.as_str()), (1, body));
        assert!(lines.contains(&checking) && lines.last() == Some(&None), "{lines:?}");
        // G81 の前に保存した手順を使い直しても、保存の前に済みと言わない
        let (r, _, _, _, _) = ask(None, Duration::from_secs(2), true);
        assert!(matches!(&r, Err(EngineError::Unconfirmed(_))), "保存した手順の窓だけの checks で済みにした: {r:?}");
    }

    /// B74 の場面（録画の手 7・cu.GkMp 11:43:22Z の本物の計画の形）: 見出しの置き換え ＋ File › Save・checks = 窓の本文 ＋ model が自分で足した書類の読み
    /// （`git.file.read` を `model_uri` と当て推量の pointer /content で）。Page の書類 = repo の README.md・repo の外に `../id_key`。
    /// B76: `model_uri` と `doc`（Page の書類の場所 = `document_file`）の `{d}` は試験の dir（その下に repo と、.git と .env を持つ別の repo の other）。
    /// → (承認の前の run, 保存した手順の git.file.read の読み, 承認した後の run〈承認待ちで止まった時だけ〉, 送った書く手の数)
    /// B76: `then` = 保存の後に足す読む手 s3（`git.file.read`）の input（None = 足さない）。
    fn b74(model_uri: &str, doc: &str, then: Option<Value>) -> (Result<EngineRecord, EngineError>, Vec<Value>, Option<Result<EngineRecord, EngineError>>, usize) {
        let d = tmp("b74");
        let model_uri = model_uri.replace("{d}", &d.display().to_string());
        let te = "com.apple.TextEdit";
        let at = json!({ "pid": 41, "window_id": 7001, "app_name": "TextEdit", "bundle_id": te, "document": "README.md" });
        let with = |extra: Value| {
            let mut i = at.clone();
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            i
        };
        let mut plan = json!({
            "none_why": null, "workflow_steps": [
                // 手順の読む手も repo からの相対（schema が相対でよいと言う = 読み戻しと同じに直す）
                step_answer("s0", "git.file.read", json!({ "uri": "README.md" })),
                step_answer("s1", "desktop.ax.type_text", with(json!({ "label": "", "role": "AXTextArea", "replace": "# shibubu-yc-demo", "text": "# Shibubu Demo" }))),
                step_answer("s2", "desktop.ax.menu", with(json!({ "path": ["File", "Save"] }))),
            ],
            "checks": [
                { "action": "desktop.window.state", "input_json": with(json!({ "role": "AXTextArea" })).to_string(), "pointer": "/controls/0/value", "expect": "contains", "value_json": "\"# Shibubu Demo\"" },
                { "action": "git.file.read", "input_json": json!({ "uri": model_uri }).to_string(), "pointer": "/content", "expect": "contains", "value_json": "\"# Shibubu Demo\\n\"" },
            ],
            "recipe": null, "trial": null,
        });
        if let Some(i) = then {
            plan["workflow_steps"].as_array_mut().unwrap().push(step_answer("s3", "git.file.read", i));
        }
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Change the README title to “Shibubu Demo” and save it", "target": null, "success": "The README's title is “Shibubu Demo”" }] });
        let body = "# shibubu-yc-demo\n\nThe repo of this run\n";
        let repo = d.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("README.md"), body).unwrap();
        std::fs::write(d.join("id_key"), "secret").unwrap();
        std::fs::create_dir_all(d.join("other/.git")).unwrap();
        std::fs::write(d.join("other/.env"), "TOKEN=secret\n").unwrap();
        let (uri, repo_uri) = (doc.replace("{d}", &d.display().to_string()), format!("file://{}", repo.display()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::with_local_adapters();
        reg.register(Box::new(Doc { body: Arc::new(Mutex::new(body.into())), file: repo.join("README.md"), save: Some(Duration::ZERO), calls: calls.clone() })).unwrap();
        let fake = Fake::new(vec![Ok::<Value, LlmError>(ops.clone()), Ok(plan.clone()), Ok(ops), Ok(plan)]);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let data = d.join("data");
        let mut go = |ok: &[ConsentReceipt]| {
            let mut c = EngineContext { target: Some(te), document: Some("README.md"), document_file: Some(&uri), repo: Some(&repo_uri), readback: Duration::from_secs(5), ..cx(&mut reg, &data, Some(&fake), &mcp, ok, &mut an) };
            run("Change the README title to “Shibubu Demo” and save it", &mut c)
        };
        let first = go(&[]);
        let reads: Vec<Value> = std::fs::read_dir(data.join(WORKFLOWS_DIR))
            .map(|r| r.flatten().filter(|f| f.path().extension().is_some_and(|e| e == "json")).collect::<Vec<_>>())
            .unwrap_or_default()
            .iter()
            .flat_map(|f| serde_json::from_slice::<Value>(&std::fs::read(f.path()).unwrap()).unwrap()["checks"].as_array().cloned().unwrap_or_default())
            .filter(|c| c["action"] == SAVED_READ)
            .collect();
        let second = match &first {
            Err(EngineError::Denied(den)) if den.pending.is_some() => {
                let ok: Vec<ConsentReceipt> = den.pending.iter().chain(&den.later).map(|p| consent(&p.action, &p.input)).collect();
                Some(go(&ok))
            }
            _ => None,
        };
        let sent = calls.lock().unwrap().iter().filter(|(a, _)| a == "desktop.ax.type_text" || a == "desktop.ax.menu").count();
        let _ = std::fs::remove_dir_all(&d);
        (first, reads, second, sent)
    }

    /// B74（Orca task_b334b9d3e2fa・録画の手 7）: 本物の AI が自分で足した書類の読みを repo からの相対（{"uri":"README.md"}）と当て推量の pointer（/content）で
    /// 書いても、Engine が repo の下の絶対に直し、同じ書類の読みは Engine の読み（/text・頼んだ差分）に任せる → 承認まで進み、承認の後は保存した file で済み。
    /// 前は Adapter が InvalidInput で手の前の読みが止まった（「Nothing was sent, because the original couldn't be read first: Input missing or wrong」・5 回中 1 回）。
    /// 手順の読む手（s0・`git.file.read` を相対で）も直す = 承認の後に走らせても止まらない。
    /// 負の対照（束の arm）: `in_repo` を呼ばない → 承認の前に止まって赤 / 読む手だけ直さない → 承認の後に済みにならず赤 / 同じ file の model の読みを落とさない →
    /// /content の読みが手順に残って赤。
    #[test]
    fn b74_a_relative_readme_check_from_the_model_reaches_approval_and_is_verified_on_disk() {
        let (first, reads, second, sent) = b74("README.md", "file://{d}/repo/README.md", None);
        let Err(EngineError::Denied(den)) = &first else { panic!("承認を頼まない: {first:?}") };
        assert!(den.pending.is_some(), "承認の前に止まった（相対の README.md の読み）: {}", den.why);
        assert!(
            !reads.is_empty() && reads.iter().all(|c| c["pointer"] == "/text" && c["input"]["uri"].as_str().is_some_and(|u| u.starts_with("file:///") && u.ends_with("/repo/README.md"))),
            "model の当て推量の読み（相対・/content）が手順に残った: {reads:?}"
        );
        let rec = second.expect("承認した run が無い").unwrap_or_else(|e| panic!("承認した後に済みにならない: {e:?}"));
        assert!(rec.verified.iter().any(|v| v.starts_with("git.file.read の /text")), "保存した file で確かめていない: {:?}", rec.verified);
        assert_eq!(sent, 2, "置き換えと保存を 1 回ずつ");
    }

    /// B74: repo の外を指す相対（`../id_key`）は直さず、手の前の読みで止まる = repo の外は読まない・承認を頼まない・何も送らない。
    /// B75: 止まった 1 行は理由の語（Input missing or wrong）でなく次の一手（`UNREADABLE_CHECK`・頼み直せば手順を書き直す）。
    /// 負の対照（束の arm）: `in_repo` が `..` を飛ばして直す → repo の中の別の file を読んで承認まで進み赤 / B75 を戻す → 理由の語の 1 行で赤。
    #[test]
    fn b74_a_relative_path_outside_the_repo_stops_before_anything_is_sent() {
        let (first, _, second, sent) = b74("../id_key", "file://{d}/repo/README.md", None);
        let Err(EngineError::Denied(den)) = &first else { panic!("止まらない: {first:?}") };
        assert!(den.pending.is_none() && second.is_none(), "repo の外の相対 path で承認まで進んだ（repo の外を repo の中に読み替えた）: {}", den.why);
        assert_eq!(den.why, UNREADABLE_CHECK, "止まった 1 行が次の一手でない");
        assert_eq!(sent, 0, "止まったのに書く手を送った");
    }

    /// B76（安全・Orca task_e1fa3405c76e）: model が別の repo の file（.git の在る other の .env）を `file://` + 絶対 path で読もうとしたら、1 つも読まず送らずに止まる
    /// （Adapter は .git の在る所なら読めてしまう・承認の要らない読み）。1 行は `OUTSIDE_REPO`。
    /// 負の対照（束の arm）: 縛りを外す → .env を読んで承認まで進み赤。
    #[test]
    fn b76_a_model_read_of_another_repo_stops_before_anything_is_read() {
        let (first, reads, second, sent) = b74("file://{d}/other/.env", "file://{d}/repo/README.md", None);
        let Err(EngineError::Denied(den)) = &first else { panic!("止まらない: {first:?}") };
        assert!(den.pending.is_none() && second.is_none(), "別の repo の file を読んで承認まで進んだ: {}", den.why);
        assert_eq!((den.why.as_str(), sent, reads.len()), (OUTSIDE_REPO, 0, 0), "止まった 1 行が違う・送った・手順を保存した");
    }

    /// B76: Page の書類の場所が `<repo>/./README.md` の形（generated.rs は面の file を正規化しない）でも、model の同じ書類の読み（相対の README.md）を落とす
    /// （Engine の読み = /text だけが残り、承認の後に保存した file で済み）。
    /// 負の対照（束の arm）: 比べる前に揃えない（文字列の ==）→ model の /content の読みが残って赤。
    #[test]
    fn b76_a_dot_slash_document_file_still_drops_the_models_duplicate_read() {
        let (first, reads, second, _) = b74("README.md", "file://{d}/repo/./README.md", None);
        let Err(EngineError::Denied(den)) = &first else { panic!("承認を頼まない: {first:?}") };
        assert!(den.pending.is_some(), "承認の前に止まった: {}", den.why);
        assert!(!reads.is_empty() && reads.iter().all(|c| c["pointer"] == "/text"), "./ の形で model の重複した読み（/content）が残った: {reads:?}");
        let rec = second.expect("承認した run が無い").unwrap_or_else(|e| panic!("承認した後に済みにならない: {e:?}"));
        assert!(rec.verified.iter().any(|v| v.starts_with("git.file.read の /text")), "{:?}", rec.verified);
    }

    /// B76（sub operator の抜け道）: model の読む手の uri が文字列でない（`{"$from":"s0.text"}` = 前の手の出力で埋まる・埋まる先は計画の時に分からない）なら、
    /// 計画の段で 1 つも読まず送らずに `OUTSIDE_REPO` で止まる（fail closed）。
    /// 負の対照（束の arm）: 文字列でない uri を「外ではない」に戻す → 門を素通りして承認まで進み赤。
    #[test]
    fn b76_a_read_step_whose_uri_comes_from_another_step_stops_before_anything_is_read() {
        let (first, reads, second, sent) = b74("README.md", "file://{d}/repo/README.md", Some(json!({ "uri": { "$from": "s0.text" } })));
        assert!(matches!(&first, Err(EngineError::Denied(den)) if den.why == OUTSIDE_REPO && den.pending.is_none()), "文字列でない uri（$from）の読む手が門を通った: {first:?}");
        assert_eq!((second.is_none(), sent, reads.len()), (true, 0, 0), "承認を頼んだ・送った・手順を保存した");
    }

    /// B76: 読む手の uri が `$from` で前の手の出力（ここでは別の repo の .env の場所）から埋まる時も、埋めた後の uri で repo の外を読まない
    /// （`run_steps` の門 = Engine の Execution と `run_workflow` の共通の口・計画の段を通らない resume も覆う）。
    /// 負の対照（束の arm）: 埋めた後の門を外す → 別の repo の .env を読んで済みになり赤。
    #[test]
    fn b76_a_read_step_filled_from_an_earlier_step_does_not_read_outside() {
        let d = tmp("b76f");
        std::fs::create_dir_all(d.join("other/.git")).unwrap();
        std::fs::write(d.join("other/.env"), "TOKEN=secret\n").unwrap();
        let mut reg = AdapterRegistry::with_local_adapters();
        let (a, _, _) = probe("alpha", &ALPHA, &format!("file://{}", d.join("other/.env").display()), false);
        reg.register(Box::new(a)).unwrap();
        let step = |id: &str, cap: &str, input: Value| WorkflowStep { kind: StepKind::Action, id: id.into(), capability: cap.into(), input };
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: "leak".into(), goal: String::new(), ui: None, checks: Vec::new(),
            steps: vec![step("s0", "alpha.text.read", json!({})), step("s1", SAVED_READ, json!({ "uri": { "$from": "s0.text" } }))],
        };
        let r = run_workflow(&wf, &mut reg, CALLER, &[]);
        let _ = std::fs::remove_dir_all(&d);
        assert!(matches!(&r, Err(WorkflowError::Outside { step }) if step == "s1"), "埋めた後に repo の外を指す読む手を走らせた: {r:?}");
    }

    #[test]
    fn w19_2_call_runner_runs_each_means_and_never_reports_a_dead_one_as_done() {
        let d = tmp("runner");
        let log = d.join("mcp.jsonl");
        let doc = d.join("a.quill.json");
        write_doc(&doc, "Old");
        let mut run = call_runner(quill_cfg("", &log), d.clone());
        let failed = |o: (Outcome, Option<Observation>)| match o {
            (Outcome::Failed { error }, None) => Some(error),
            _ => None,
        };
        // CLI: shell を通さない・置き場は 1 つの argv（空白を含んでも割れない）・stdout の JSON が payload
        let cli = RecipeCall::Cli { argv: vec!["/usr/bin/printf".into(), r#"{"n":%s,"who":"%s"}"#.into(), "{{input.n}}".into(), "{{input.who}}".into()] };
        let (o, ob) = run(&cli, &json!({ "n": 2, "who": "a b; rm -rf ~" }));
        assert!(matches!(o, Outcome::Unverified { .. }), "{o:?}");
        assert_eq!(ob.unwrap().payload, json!({ "n": 2, "who": "a b; rm -rf ~" }));
        assert_eq!(failed(run(&cli, &json!({ "n": 2 }))), Some(ErrorCode::InvalidInput), "無い key は呼ばない");
        assert_eq!(failed(run(&RecipeCall::Cli { argv: vec!["/usr/bin/false".into()] }, &json!({}))), Some(ErrorCode::Internal), "0 以外の終了は成功でない");
        assert_eq!(failed(run(&RecipeCall::Cli { argv: vec!["/nonexistent/tool".into()] }, &json!({}))), Some(ErrorCode::NotReady));
        // MCP: local の server を起こして tool を呼ぶ・payload は structuredContent
        let read = RecipeCall::Mcp { server: "quill".into(), tool: "quill_read".into(), arguments: json!({ "path": "{{input.path}}" }) };
        let (o, ob) = run(&read, &json!({ "path": doc }));
        assert!(matches!(o, Outcome::Unverified { .. }));
        assert_eq!(ob.unwrap().payload["title"], "Old");
        assert_eq!(failed(run(&RecipeCall::Mcp { server: "none".into(), tool: "t".into(), arguments: json!({}) }, &json!({}))), Some(ErrorCode::NotReady), "設定に無い server");
        assert_eq!(failed(run(&read, &json!({ "path": d.join("none.json") }))), Some(ErrorCode::DriverRefused), "tool の isError");
        // HTTP: 127.0.0.1 の使い捨ての server だけ（外に送らない）
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let body = r#"{"ok":true}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let (o, ob) = run(&RecipeCall::Http { method: "get".into(), url: format!("http://127.0.0.1:{port}/items/{{{{input.id}}}}"), body: None }, &json!({ "id": 42 }));
        assert!(matches!(o, Outcome::Unverified { .. }), "{o:?}");
        assert_eq!(ob.unwrap().payload, json!({ "ok": true }));
        assert!(srv.join().unwrap().starts_with("GET /items/42 "), "url の置き場を埋めた");
        assert_eq!(failed(run(&RecipeCall::Http { method: "GET".into(), url: "file:///etc/hosts".into(), body: None }, &json!({}))), Some(ErrorCode::InvalidInput));
        for c in [RecipeCall::Gui { steps: vec![] }, RecipeCall::Dom { origin: "https://example.com".into(), script: "1".into() }, RecipeCall::Code { artifact: "a/v1".into() }] {
            assert_eq!(failed(run(&c, &json!({}))), Some(ErrorCode::NotReady), "この版で使えない手段は成功と返さない");
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2, "MCP の tool を呼んだのは 2 回だけ");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_2b_validate_refuses_bad_shapes() {
        let s = |id: &str, input: Value| WorkflowStep { kind: StepKind::Action, id: id.into(), capability: "alpha.text.read".into(), input };
        let wf = |steps, checks| Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: "w".into(), goal: String::new(), steps, ui: None, checks };
        assert!(validate(&wf(vec![s("a", json!({})), s("b", json!({ "x": { "$from": "a.text" } }))], vec![])).is_ok());
        let invalid = |w: Workflow| matches!(validate(&w), Err(WorkflowError::Invalid(_)));
        assert!(invalid(wf(vec![], vec![])), "手順 0 個");
        assert!(invalid(wf(vec![s("a", json!({})), s("a", json!({}))], vec![])), "id の重なり");
        assert!(invalid(wf(vec![s("a", json!({ "x": { "$from": "b.text" } })), s("b", json!({}))], vec![])), "後ろの手を指す");
        assert!(invalid(wf(vec![s("a", json!({ "x": [{ "$from": "a" }] }))], vec![])), "自分を指す");
        assert!(invalid(wf(vec![s("a", json!({}))], vec![ReadBack { input: json!({ "p": { "$from": "a.x" } }), ..rb("", Expect::Changed) }])), "読み戻しが手を指す");
        // W19-2b: 期待する値は手の出力を指してよい・在らない手は拒否（書く手より前の手かは効果を知る run_workflow と generate が見る: w19_2b_expected_value_comes_from_before_the_first_write）
        let from = |r: &str| rb("/text", Expect::Equals { value: json!({ "$from": r }) });
        assert!(validate(&wf(vec![s("a", json!({})), s("b", json!({}))], vec![from("a.text"), from("b.text")])).is_ok());
        assert!(invalid(wf(vec![s("a", json!({}))], vec![from("z.text")])), "期待する値が在らない手を指す");
        assert!(invalid(Workflow { schema: "shibubu.workflow/0".into(), ..wf(vec![s("a", json!({}))], vec![]) }));
    }

    #[test]
    fn w19_2b_run_workflow_hands_data_between_services_through_the_gate() {
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "copy".into(),
            goal: "alpha の文を beta に写して".into(),
            steps: vec![
                WorkflowStep { kind: StepKind::Action, id: "read".into(), capability: "alpha.text.read".into(), input: json!({}) },
                WorkflowStep { kind: StepKind::Action, id: "write".into(), capability: "beta.text.write".into(), input: json!({ "text": { "$from": "read.text" } }) },
            ],
            ui: None,
            checks: vec![],
        };
        let setup = |lease: bool| {
            let mut reg = AdapterRegistry::default();
            let (a, _, ac) = probe("alpha", &ALPHA, "alpha の文", false);
            let (b, bt, bc) = probe("beta", &BETA, "", lease);
            reg.register(Box::new(a)).unwrap();
            reg.register(Box::new(b)).unwrap();
            (reg, ac, bt, bc)
        };
        let ok = consent("beta.text.write", &json!({ "text": "alpha の文" }));
        // 承認が無い: 書く手は gate が止め、beta は変わらない
        let (mut reg, _, bt, bc) = setup(false);
        assert!(matches!(run_workflow(&wf, &mut reg, CALLER, &[]), Err(WorkflowError::Rejected { step, .. }) if step == "write"));
        assert_eq!((bt.lock().unwrap().as_str(), bc.lock().unwrap().len()), ("", 0), "beta の adapter は呼ばれていない");
        // 違う内容への承認: exact payload が合わないので止める
        let other = consent("beta.text.write", &json!({ "text": "別の文" }));
        assert!(matches!(run_workflow(&wf, &mut reg, CALLER, &[other]), Err(WorkflowError::Rejected { why, .. }) if why.contains("digest")));
        // 人が beta を操作中（lease）: 承認が在っても adapter の gate が断る
        let (mut leased, _, lt, _) = setup(true);
        assert!(matches!(run_workflow(&wf, &mut leased, CALLER, std::slice::from_ref(&ok)), Err(WorkflowError::Rejected { why, .. }) if why.contains("lease")));
        assert_eq!(lt.lock().unwrap().as_str(), "", "lease の間は書かない");
        // 承認が在る: alpha の出力が beta に渡り、渡した値は digest で残る
        let rc = run_workflow(&wf, &mut reg, CALLER, std::slice::from_ref(&ok)).unwrap();
        assert_eq!(bt.lock().unwrap().as_str(), "alpha の文", "beta の実状態（adapter の持つ text）");
        assert_eq!(
            rc.steps[1].handoffs,
            vec![Handoff { from_step: "read".into(), to_step: "write".into(), field: "text".into(), digest: crate::crypto::sha256_hex(json!("alpha の文").to_string().as_bytes()) }]
        );
        assert!(matches!(run_workflow(&wf, &mut reg, CALLER, std::slice::from_ref(&ok)), Err(WorkflowError::Rejected { why, .. }) if why.contains("使用済み")), "1 回の承認で 1 回");
        // 能力が無い: 1 手も走らせない
        let (mut lone, ac, _, _) = {
            let mut r = AdapterRegistry::default();
            let (a, _, ac) = probe("alpha", &ALPHA, "x", false);
            r.register(Box::new(a)).unwrap();
            (r, ac, (), ())
        };
        assert!(matches!(run_workflow(&wf, &mut lone, CALLER, &[]), Err(WorkflowError::Missing { capability, .. }) if capability == "beta.text.write"));
        assert!(ac.lock().unwrap().is_empty(), "前の手も走らせない");
    }

    #[test]
    fn w19_2_trial_runs_the_generated_adapter_on_a_copy_and_never_touches_the_real_doc() {
        let d = tmp("trial");
        let log = d.join("mcp.jsonl");
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = op(Some(QUILL_APP));
        let tdir = trial_dir(&d, &o);
        let mcp = quill_cfg("", &log);
        // 発見: MCP の tools は一覧に在るだけ = 未確認（Ready にならないので choose は選ばない）
        let found = mcp_records(&mcp, &[]);
        assert_eq!(found.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["mcp.quill.quill_read", "mcp.quill.quill_set_title"]);
        assert!(found.iter().all(|r| r.verification == Verification::Unverified && r.evidence[1].contains(r#""server":"quill""#)));
        assert_eq!(choose(&o, &found), None);
        let (mut reg, mut an) = (AdapterRegistry::default(), Vec::new());
        let mut answer = quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo");
        // 試験で流さない 3 つ目の action（Verified にならない事を見る・反証の指摘）
        let mut unused = answer["recipe"]["actions"][0].clone();
        unused["id"] = json!("quillpad.doc.body.set");
        answer["recipe"]["actions"].as_array_mut().unwrap().push(unused);
        let fake = Fake::new(vec![Ok(answer)]);
        let mut c = cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
        let g = generate(&o, &found, &mut c, None).unwrap();
        assert!(fake.requests.lock().unwrap()[0].input.contains("quill_set_title"), "model に発見した tool を渡した");
        let (mut recipe, trial) = (g.recipe.unwrap(), g.trial.unwrap());
        prepare_trial(&tdir, &g.workflow).unwrap();
        assert_eq!(title_on_disk(&tdir.join("notes.quill.json")), "Old", "本物の写し");
        let lines = run_trial(&recipe, &trial, &tdir, &g.workflow, &mut c).unwrap();
        assert!(lines[0].contains("/title") && lines[0].contains("Shibubu Demo"), "{lines:?}");
        assert_eq!(title_on_disk(&tdir.join("notes.quill.json")), "Shibubu Demo", "写しは変わった（試験の側で disk を読む）");
        assert_eq!(title_on_disk(&real), "Old", "本物の文書は試験で変わらない");
        mark_verified(&mut recipe, &trial, &lines.join(" / "));
        let states = |r: &Recipe| r.actions.iter().map(|a| (a.id.clone(), is_verified_action(a))).collect::<Vec<_>>();
        let want = [("quillpad.doc.title.set".to_string(), true), ("quillpad.doc.read".to_string(), true), ("quillpad.doc.body.set".to_string(), false)];
        assert_eq!(states(&recipe), want, "試験で流した 2 つの action だけ確かめ済み・流していない action は未確認のまま");
        let p = save_recipe(&d.join(EXTENSIONS_DIR), &recipe).unwrap();
        assert_eq!(states(&serde_json::from_slice::<Recipe>(&std::fs::read(p).unwrap()).unwrap()), want);
        // 負の対照: server が違う値を書く → 試験で合わず、確かめ済みにしない
        let wrong = quill_cfg("wrong", &log);
        let mut c2 = cx(&mut reg, &d, None, &wrong, &[], &mut an);
        prepare_trial(&tdir, &g.workflow).unwrap();
        assert!(matches!(run_trial(&recipe, &trial, &tdir, &g.workflow, &mut c2), Err(EngineError::Generation(m)) if m.contains("合いません")));
        // 負の対照: 発見した server が設定から消えた → 呼べず、確かめ済みにしない
        let gone = McpConfig::default();
        let mut c3 = cx(&mut reg, &d, None, &gone, &[], &mut an);
        assert!(matches!(run_trial(&recipe, &trial, &tdir, &g.workflow, &mut c3), Err(EngineError::Generation(m)) if m.contains("Not connected")));
        assert_eq!(title_on_disk(&real), "Old");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_2_discover_keeps_saved_capabilities_but_never_as_ready_when_they_cannot_run_now() {
        let gen = Origin::Generated { artifact: "quillpad".into() };
        let mut stale = rec("quillpad.doc.read", Reuse::Shared, gen.clone(), verified());
        stale.versions = vec!["14.5".into()];
        let in_registry = vec![
            rec("quillpad.doc.read", Reuse::Shared, gen.clone(), verified()),
            rec("quillpad.doc.title.set", Reuse::Shared, gen.clone(), verified()),
        ];
        let saved = vec![
            stale,
            CapabilityRecord { versions: vec!["15.0".into()], ..rec("quillpad.doc.title.set", Reuse::Shared, gen.clone(), verified()) },
            rec("rea.quill.export", Reuse::Shared, Origin::Analysis { backend: "rea".into(), report: "a1".into() }, Verification::Unverified),
            rec("old.adapter.write", Reuse::Shared, gen.clone(), verified()),
        ];
        let env = EnvFacts { target_version: None, os_version: "15.0".into(), permissions: vec![] };
        let found = with_saved(in_registry, saved, &env);
        let state = |id: &str| found.iter().find(|r| r.id == id).map(|r| (r.verification.clone(), r.constraints.clone()));
        assert_eq!(found.len(), 4, "registry の 2 件 + registry に無い保存した 2 件（同じ id は重ねない）");
        assert_eq!(state("quillpad.doc.read").unwrap(), (Verification::Unverified, vec![REVERIFY.to_string()]), "OS が変わった");
        assert!(matches!(state("quillpad.doc.title.set").unwrap().0, Verification::Verified { .. }), "確かめた時と同じ OS の保存なら確かめ済みのまま");
        assert_eq!(state("rea.quill.export").unwrap().0, Verification::Unverified, "解析の候補は候補のまま返す");
        assert_eq!(state("old.adapter.write").unwrap(), (Verification::Unverified, vec![NOT_LOADED.to_string()]), "今の registry に無い物は呼べない");
    }

    #[test]
    fn w19_2_without_a_model_runs_only_one_unambiguous_read_or_approved_action() {
        let mut reg = AdapterRegistry::default();
        let (a, _, _) = probe("alpha", &ALPHA, "x", false);
        reg.register(Box::new(a)).unwrap();
        let actions = reg.actions();
        let o = op(None);
        let v = |id: &str| rec(id, Reuse::Shared, Origin::Official, verified());
        let wf = alone("alpha を読んで", &o, &[v("alpha.text.read"), rec("mcp.q.t", Reuse::Shared, Origin::Official, Verification::Unverified)], &actions).unwrap();
        assert_eq!((wf.steps.len(), wf.steps[0].capability.as_str(), &wf.steps[0].input, wf.checks.len()), (1, "alpha.text.read", &json!({}), 0));
        assert!(validate(&wf).is_ok());
        assert!(alone("書いて", &o, &[v("alpha.text.write")], &actions).is_ok(), "承認の要る書き込みは選べる（gate が exact payload の承認を求める）");
        let no = |r: Result<Workflow, EngineError>| matches!(r, Err(EngineError::NoMeans { .. }));
        assert!(no(alone("x", &o, &[v("alpha.text.read"), v("alpha.text.write")], &actions)), "2 つから選ばない（曖昧）");
        assert!(no(alone("x", &o, &[], &actions)), "確かめ済みが無い");
        assert!(no(alone("x", &o, &[v("beta.text.read")], &actions)), "registry に無い");
        let mut specs = reg.actions();
        specs[0].input_schema.insert("required".into(), json!(["path"]));
        assert!(no(alone("x", &o, &[v("alpha.text.read")], &specs)), "入力が要る");
        specs[1].requires_approval = false;
        assert!(no(alone("x", &o, &[v("alpha.text.write")], &specs)), "承認の要らない書く操作は model 無しで選ばない");
    }

    /// 反証 A・B・E（W19-2 の反証の直し）: 試験の手が本物に届き得る答えは、何も呼ばずに Generation。case ごとに数えて、通った case を並べる。
    #[test]
    fn w19_2_generate_refuses_a_trial_that_could_reach_the_real_doc() {
        let d = tmp("iso");
        let log = d.join("mcp.jsonl");
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = op(Some(QUILL_APP));
        let copy = trial_dir(&d, &o).join("notes.quill.json");
        let mcp = quill_cfg("", &log);
        let found = mcp_records(&mcp, &[]);
        let (mut reg, mut an) = (AdapterRegistry::default(), Vec::new());
        let mut gen = |edit: &dyn Fn(&mut Value)| {
            let mut a = quill_answer(&real, &copy, "Shibubu Demo");
            edit(&mut a);
            let fake = Fake::new(vec![Ok(a)]);
            let mut c = cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
            generate(&o, &found, &mut c, None)
        };
        assert!(gen(&|_| {}).is_ok(), "正の対照: 写しだけを指す試験は通る");
        let call = |tool: &str, args: Value| json!(json!({ "via": "mcp", "server": "quill", "tool": tool, "arguments": args }).to_string());
        let input = |v: Value| json!(v.to_string());
        let path_case = |p: String| move |a: &mut Value| a["trial"]["steps"][0]["input_json"] = json!(json!({ "path": p, "title": "x" }).to_string());
        let (out, unreached, real_flow) = ("試験の置き場の外を指し得ます", "写しの path が届いていません", "本物の手順が試験の置き場");
        let cases: Vec<(&str, &str, Box<dyn Fn(&mut Value) + '_>)> = vec![
            ("A: call に本物の path を直に書き、試験の input に path が無い", out, Box::new(|a: &mut Value| {
                a["recipe"]["actions"][0]["call_json"] = call("quill_set_title", json!({ "path": real, "title": "{{input.title}}" }));
                a["recipe"]["actions"][1]["call_json"] = call("quill_read", json!({ "path": real }));
                a["trial"]["steps"][0]["input_json"] = input(json!({ "title": "Shibubu Demo" }));
                a["trial"]["checks"][0]["input_json"] = json!("{}");
            })),
            ("path を取らない tool（写しの path が call に届かない書く手）", unreached, Box::new(|a: &mut Value| a["recipe"]["actions"][0]["call_json"] = call("quill_set_title", json!({ "title": "{{input.title}}" })))),
            ("B: 相対 path（MCP の server の cwd は試験の置き場でない）", out, Box::new(path_case("real/notes.quill.json".into()))),
            ("B: ~/", out, Box::new(path_case("~/notes.quill.json".into()))),
            ("B: file://（写しの置き場でも）", out, Box::new(path_case(format!("file://{}", copy.display())))),
            ("B: ../", out, Box::new(path_case("../real/notes.quill.json".into()))),
            ("E: 本物の手順の読み戻しが試験の写しを読む", real_flow, Box::new(|a: &mut Value| a["checks"][0]["input_json"] = input(json!({ "path": copy })))),
        ];
        // 拒んだ理由まで見る（別の検査で拒まれても、この case の規則を外した事にはならない）
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(name, why, edit)| match gen(edit.as_ref()) {
                Err(EngineError::Generation(m)) if m.contains(why) => None,
                other => Some(format!("{name}: {:?}", other.map(|_| "通した"))),
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert!(!log.exists(), "generate は tool を呼ばない");
        assert_eq!(title_on_disk(&real), "Old");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証 A・B（runtime）: 試験は写しの置き場を cwd にし、走らせる直前にも宛先を調べ、本物の file や写しが変わったら済みにしない。scenario ごとに数える。
    #[test]
    fn w19_2_trial_checks_each_call_and_never_passes_when_the_real_doc_or_the_copy_changes() {
        let d = tmp("iso-run");
        let log = d.join("mcp.jsonl");
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = op(Some(QUILL_APP));
        let tdir = trial_dir(&d, &o);
        let copy = tdir.join("notes.quill.json");
        let mcp = quill_cfg("", &log);
        let found = mcp_records(&mcp, &[]);
        let (mut reg, mut an) = (AdapterRegistry::default(), Vec::new());
        let mut make = |a: Value| {
            let fake = Fake::new(vec![Ok(a)]);
            let mut c = cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
            let g = generate(&o, &found, &mut c, None).unwrap();
            (g.workflow, g.recipe.unwrap(), g.trial.unwrap())
        };
        let mut sed = quill_answer(&real, &copy, "Shibubu Demo");
        sed["recipe"]["actions"][0]["call_json"] = json!(json!({ "via": "cli", "argv": ["/usr/bin/sed", "-i", "", "s/Old/Shibubu Demo/", "{{input.path}}", "{{input.also}}"] }).to_string());
        sed["trial"]["steps"][0]["input_json"] = json!(json!({ "path": copy, "also": "real/notes.quill.json" }).to_string());
        let (sed_wf, sed_recipe, sed_trial) = make(sed);
        let (wf, recipe, good) = make(quill_answer(&real, &copy, "Shibubu Demo"));
        let mut outside = good.clone();
        outside.steps[0].input = json!({ "path": real, "title": "X" });
        let mut both = quill_cfg("both", &log);
        both.mcp_servers.get_mut("quill").unwrap().env.insert("FAKE_MCP_OPEN_DOC".into(), real.display().to_string());
        let lying = quill_cfg("lying_read", &log);
        let mut try_trial = |m: &McpConfig, wf: &Workflow, recipe: &Recipe, trial: &Trial| {
            write_doc(&real, "Old");
            let _ = std::fs::remove_file(&log);
            prepare_trial(&tdir, wf).unwrap();
            let mut c = cx(&mut reg, &d, None, m, &[], &mut an);
            let r = run_trial(recipe, trial, &tdir, wf, &mut c);
            (r, title_on_disk(&real), std::fs::read_to_string(&log).unwrap_or_default())
        };
        let mut wrong = Vec::new();
        // CLI の試験は写しの置き場を cwd にする（相対の名前は写しの側に解ける・前は利用者の data dir で、本物の文書を書き換えた）
        let (r, title, _) = try_trial(&mcp, &sed_wf, &sed_recipe, &sed_trial);
        if title != "Old" || !matches!(r, Err(EngineError::Generation(_))) {
            wrong.push(format!("CLI の相対の名前: {r:?}・本物の題 {title}"));
        }
        // 走らせる直前の検査（generate を通らない試験でも）: 写しの外を指す手は呼ばない
        let (r, title, calls) = try_trial(&mcp, &wf, &recipe, &outside);
        if title != "Old" || calls.contains("quill_set_title") || !matches!(r, Err(EngineError::Generation(_))) {
            wrong.push(format!("写しの外を指す手: {r:?}・本物の題 {title}"));
        }
        // 開いている文書にも効く tool（写しも本物も変える）: 写しの読み戻しは合っても、本物が変わったので済みにしない
        let (r, _, _) = try_trial(&both, &wf, &recipe, &good);
        if !matches!(&r, Err(EngineError::Generation(m)) if m.contains("The real")) {
            wrong.push(format!("本物も変える tool: {r:?}"));
        }
        // 読むだけと言う tool が写しを書き換える: 済みにしない
        let (r, _, _) = try_trial(&lying, &wf, &recipe, &good);
        if !matches!(&r, Err(EngineError::Generation(m)) if m.contains("写しを変え")) {
            wrong.push(format!("読むと言って書く tool: {r:?}"));
        }
        // 正の対照: 同じ試験が正しい server では通り、本物は変わらない
        let (r, title, _) = try_trial(&mcp, &wf, &recipe, &good);
        if r.is_err() || title != "Old" {
            wrong.push(format!("正の対照: {r:?}・本物の題 {title}"));
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証 C・C2（W19-2 の反証の直し）: 承認と試してよいかを model の申告だけで決めない。手段から分かる効果と食い違えば Generation。case ごとに数える。
    #[test]
    fn w19_2_generate_takes_the_effect_from_the_means_not_from_the_model() {
        let d = tmp("means");
        let real = d.join("real").join("notes.quill.json");
        let o = op(Some(QUILL_APP));
        let copy = trial_dir(&d, &o).join("notes.quill.json");
        let mcp = quill_cfg("", &d.join("mcp.jsonl"));
        let found = mcp_records(&mcp, &[]);
        let (mut reg, mut an) = (AdapterRegistry::default(), Vec::new());
        let mut gen = |found: &[CapabilityRecord], edit: &dyn Fn(&mut Value)| {
            let mut a = quill_answer(&real, &copy, "X");
            edit(&mut a);
            let fake = Fake::new(vec![Ok(a)]);
            let mut c = cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
            generate(&o, found, &mut c, None)
        };
        let g = gen(&found, &|_| {}).unwrap();
        assert_eq!(g.recipe.unwrap().actions.iter().map(|a| a.approval).collect::<Vec<_>>(), [true, false], "正の対照: readOnlyHint の tool だけが承認なしの読む action");
        let http = json!(json!({ "via": "http", "method": "DELETE", "url": "https://example.com/items/{{input.id}}" }).to_string());
        let cli = json!(json!({ "via": "cli", "argv": ["/bin/cat", "{{input.path}}"] }).to_string());
        let mut destructive = found.clone();
        let set = destructive.iter_mut().find(|r| r.id == "mcp.quill.quill_set_title").expect("発見した tool");
        let tool = McpTool { name: "quill_set_title".into(), description: String::new(), input_schema: json!({}) };
        *set = mcp_record("quill", &tool, McpToolHints { read_only: false, destructive: true, not_destructive: false });
        let none: &[CapabilityRecord] = &[];
        let cases: Vec<(&str, &str, &[CapabilityRecord], Box<dyn Fn(&mut Value) + '_>)> = vec![
            ("C: 書く tool を local_read と申告", "readOnlyHint", &found[..], Box::new(|a: &mut Value| a["recipe"]["actions"][0]["effect"] = json!("local_read"))),
            ("readOnlyHint を見ていない（発見していない）tool を読むと申告", "readOnlyHint", none, Box::new(|_: &mut Value| {})),
            ("C2: HTTP の DELETE を external_read と申告", "HTTP の DELETE", &found[..], Box::new(|a: &mut Value| {
                a["recipe"]["actions"][1]["effect"] = json!("external_read");
                a["recipe"]["actions"][1]["call_json"] = http.clone();
            })),
            ("CLI の手を読むだけと申告（CLI は書く扱い）", "CLI の手", &found[..], Box::new(|a: &mut Value| a["recipe"]["actions"][1]["call_json"] = cli.clone())),
            ("server が消すと言う tool を local_write と申告", "destructiveHint", &destructive[..], Box::new(|_: &mut Value| {})),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(name, why, f, edit)| match gen(f, edit.as_ref()) {
                Err(EngineError::Generation(m)) if m.contains(why) => None,
                other => Some(format!("{name}: {:?}", other.map(|_| "通した"))),
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証 D（W19-2 の反証の直し）: 作る Adapter の名前と action の id は、既に在る Adapter・action と重ねない（register は同じ id の前の物を外すので、
    /// 公式の Adapter を外して置き換える事になる）。前に作った Adapter を広げる時だけ、同じ id で前の action を全部含める。
    #[test]
    fn w19_2_generate_never_takes_an_existing_adapter_or_action_name() {
        let d = tmp("ids");
        let real = d.join("real").join("notes.quill.json");
        let o = op(Some(QUILL_APP));
        let copy = trial_dir(&d, &o).join("notes.quill.json");
        let mcp = quill_cfg("", &d.join("mcp.jsonl"));
        let found = mcp_records(&mcp, &[]);
        let mut reg = AdapterRegistry::default();
        let (p, _, _) = probe("alpha", &ALPHA, "x", false);
        reg.register(Box::new(p)).unwrap();
        let mut an = Vec::new();
        let mut gen = |edit: &dyn Fn(&mut Value)| {
            let mut a = quill_answer(&real, &copy, "X");
            edit(&mut a);
            let fake = Fake::new(vec![Ok(a)]);
            let mut c = cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
            generate(&o, &found, &mut c, None)
        };
        let refused = |r: &Result<Generated, EngineError>, why: &str| matches!(r, Err(EngineError::Generation(m)) if m.contains(why));
        let base = gen(&|_| {}).unwrap().recipe.unwrap();
        let with_extra = |a: &mut Value| {
            let mut more = a["recipe"]["actions"][0].clone();
            more["id"] = json!("quillpad.doc.body.set");
            a["recipe"]["actions"].as_array_mut().unwrap().push(more);
        };
        let mut wrong = Vec::new();
        let names: Vec<(&str, &str, Box<dyn Fn(&mut Value)>)> = vec![
            ("登録済みの Adapter の名前", "既に在る Adapter の名前", Box::new(|a: &mut Value| a["recipe"]["id"] = json!("alpha"))),
            ("登録済みの action の id", "既に在る action の id", Box::new(|a: &mut Value| {
                a["recipe"]["actions"][1]["id"] = json!("alpha.text.read");
                a["checks"][0]["action"] = json!("alpha.text.read");
                a["trial"]["checks"][0]["action"] = json!("alpha.text.read");
            })),
        ];
        for (name, why, edit) in names {
            let r = gen(edit.as_ref());
            if !refused(&r, why) {
                wrong.push(format!("{name}: {:?}", r.map(|_| "通した")));
            }
        }
        // 前に作った Adapter（extensions/quillpad・確かめ済み）: 前の action を落とす答えは、前の目的の手順を壊すので拒む・全部含めれば広げてよい
        let ext = d.join(EXTENSIONS_DIR);
        let extra = RecipeAction { id: "quillpad.doc.body.set".into(), ..base.actions[0].clone() };
        let mut old = Recipe { actions: base.actions.iter().cloned().chain([extra.clone()]).collect(), ..base.clone() };
        old.actions.iter_mut().for_each(|a| a.verification = verified());
        save_recipe(&ext, &old).unwrap();
        let r = gen(&|_| {});
        if !refused(&r, "が無い答え") {
            wrong.push(format!("前に作った Adapter の action を落とす: {:?}", r.map(|_| "通した")));
        }
        match gen(&with_extra) {
            Ok(g) => {
                let mut grown = g.recipe.unwrap();
                mark_verified(&mut grown, &g.trial.unwrap(), "試験");
                keep_verified(&mut grown, &old);
                if !grown.actions.iter().all(is_verified_action) {
                    wrong.push("前と同じ定義の流していない action の確かめが残らない".into());
                }
            }
            Err(e) => wrong.push(format!("前の action を全部含めて広げる答えを拒んだ: {e}")),
        }
        let mut other = Recipe { actions: vec![RecipeAction { call: RecipeCall::Cli { argv: vec!["/usr/bin/true".into()] }, verification: Verification::Unverified, ..extra }], ..old.clone() };
        keep_verified(&mut other, &old);
        if other.actions[0].verification != Verification::Unverified {
            wrong.push("定義が違う action に前の確かめを付けた".into());
        }
        // 利用者が入れた（生成でない）同じ名前の recipe は置き換えない
        save_recipe(&ext, &Recipe { origin: Origin::Official, ..old.clone() }).unwrap();
        let r = gen(&with_extra);
        if !refused(&r, "利用者が入れた") {
            wrong.push(format!("生成でない recipe と同じ名前: {:?}", r.map(|_| "通した")));
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証（prepare_trial の同じ名前）: 本物の手順が同じ名前の別の file を 2 つ指すと写しが重なるので、試験を作らない。
    #[test]
    fn w19_2_prepare_trial_refuses_two_real_files_with_the_same_name() {
        let d = tmp("dup");
        let (a, b) = (d.join("a").join("notes.json"), d.join("b").join("notes.json"));
        write_doc(&a, "A");
        write_doc(&b, "B");
        let wf = |p: &Path, q: &Path| Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "w".into(),
            goal: String::new(),
            steps: vec![WorkflowStep { kind: StepKind::Action, id: "s".into(), capability: "x.write".into(), input: json!({ "path": p }) }],
            ui: None,
            checks: vec![ReadBack { input: json!({ "path": q }), ..rb("/title", Expect::Changed) }],
        };
        let t = d.join("t");
        assert!(matches!(prepare_trial(&t, &wf(&a, &b)), Err(EngineError::Generation(_))), "a/notes.json と b/notes.json");
        assert!(prepare_trial(&t, &wf(&a, &a)).is_ok(), "同じ file を 2 回指すのは重ならない");
        assert_eq!(title_on_disk(&t.join("notes.json")), "A");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証（版・環境・権限が変わったら確かめ直す）: 確かめた試験を手順と組で残し、作り直さずに写しで流し直せるようにする。
    #[test]
    fn w19_2_saved_trial_round_trips_next_to_its_workflow() {
        let d = tmp("saved-trial");
        let trial = Trial {
            steps: vec![WorkflowStep { kind: StepKind::Action, id: "s".into(), capability: "quillpad.doc.title.set".into(), input: json!({ "path": "/t/x", "title": "T" }) }],
            checks: vec![rb("/title", Expect::Changed)],
        };
        assert_eq!(load_trial(&d, "wf-1"), None, "無ければ None");
        let p = save_trial(&d, "wf-1", "quillpad", &trial).unwrap();
        assert!(p.starts_with(d.join(WORKFLOWS_DIR)) && p != d.join(WORKFLOWS_DIR).join("wf-1.json"), "手順の file とは別: {p:?}");
        assert_eq!(load_trial(&d, "wf-1"), Some(("quillpad".to_string(), trial.clone())));
        assert_eq!(load_workflows(&d.join(WORKFLOWS_DIR)).unwrap(), vec![], "試験の file を手順として読まない");
        assert!(matches!(save_trial(&d, "../x", "quillpad", &trial), Err(EngineError::Store(_))));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 通し（goal → 発見 → 生成 → 試験 → 承認つきの実行 → 本物の読み戻し → 記録 → 2 回目は生成 0 回）。
    #[test]
    fn w19_2_run_discovers_generates_verifies_and_reuses_without_regenerating() {
        let d = tmp("run");
        let log = d.join("mcp.jsonl");
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = op(Some(QUILL_APP));
        let tdir = trial_dir(&d, &o);
        let mcp = quill_cfg("", &log);
        let goal = format!("{} の Quillpad の文書の題を『Shibubu Demo』にして保存して", real.display());
        let interp = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "文書の題が Shibubu Demo", "target": QUILL_APP }] }));
        let fake = Fake::new(vec![interp(), Ok(quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo")), interp()]);
        let write_input = json!({ "path": real, "title": "Shibubu Demo" });
        let mut an = Vec::new();
        let mut reg = AdapterRegistry::default();
        // 承認が無い: 生成と試験は通るが、本物は変えない（Denied）。手順と Adapter は残る
        let denied = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        assert!(matches!(denied, Err(EngineError::Denied(_))), "{denied:?}");
        assert_eq!(title_on_disk(&real), "Old");
        let ok = [consent("quillpad.doc.title.set", &write_input)];
        let first = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an)).unwrap();
        assert_eq!(title_on_disk(&real), "Shibubu Demo", "試験の側の oracle（disk を直に読む）");
        assert!(first.reused && first.generated.is_empty(), "承認を待った手順を再利用した");
        assert!(first.verified[0].contains("/title"));
        assert_eq!(first.stages.iter().map(|s| s.stage).collect::<Vec<_>>(), Stage::ALL);
        let names: Vec<String> = fake.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, [INTERPRET_NAME, GENERATE_NAME, INTERPRET_NAME], "生成は 1 回だけ");
        assert!(d.join(CAPABILITIES_DIR).join("quillpad.doc.title.set.json").is_file());
        // 版・OS が変わった: 作り直さず（model 無しでも）保存した試験を写しで流し直して使う（反証の指摘・YC_WAVES の「再検証」）
        let ok2 = [consent("quillpad.doc.title.set", &write_input)];
        let mut moved = cx(&mut reg, &d, None, &mcp, &ok2, &mut an);
        moved.env.os_version = "99.0".into();
        let again = run(&goal, &mut moved).unwrap();
        assert!(again.reused && again.generated.is_empty(), "OS が変わっても作り直さない: {again:?}");
        assert!(again.stages.iter().any(|s| s.stage == Stage::Verification && matches!(&s.outcome, StageOutcome::Done { note } if note.contains("隔離した試験"))), "写しで確かめ直した");
        assert_eq!(fake.requests.lock().unwrap().len(), 3, "model を呼んでいない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 試験の側で git の repo を作る（commit 1 つ・branch = `branch`）。
    fn git_repo(dir: &Path, branch: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("README.md"), "w19-2b\n").unwrap();
        git_out(dir, &["init", "-q", "-b", branch]);
        git_out(dir, &["add", "README.md"]);
        git_out(dir, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "first"]);
    }

    /// 試験の側の git の oracle（Engine・registry の Git の Adapter を通らず git を直に打つ・利用者の git の設定は読まない）。
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git").arg("-C").arg(dir).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// model の答え（W19-2b）: git（registry に在る手書きの公式の Adapter）で repo の今の branch を読み、Quillpad（MCP・事前実装なし）の文書の題に渡す手順。
    /// 読み戻しは 2 つの service の実状態: Quillpad の題 = 受け渡した branch・git の HEAD = 読んだ時の HEAD（読むだけで変えていない）。期待する値は手の出力（`$from`）。
    fn handoff_answer(repo: &Path, real: &Path, trial: &Path) -> Value {
        let mut a = quill_answer(real, trial, "試験の題");
        let c = |action: &str, input: Value, pointer: &str, from: &str| {
            json!({ "action": action, "input_json": input.to_string(), "pointer": pointer, "expect": "equals", "value_json": json!({ "$from": from }).to_string() })
        };
        let uri = format!("file://{}", repo.display());
        a["workflow_steps"] = json!([
            step_answer("read", "git.status.read", json!({ "uri": uri })),
            step_answer("set", "quillpad.doc.title.set", json!({ "path": real, "title": { "$from": "read.head.branch" } })),
        ]);
        a["checks"] = json!([
            c("quillpad.doc.read", json!({ "path": real }), "/title", "read.head.branch"),
            c("git.status.read", json!({ "uri": uri }), "/head/sha", "read.head.sha"),
        ]);
        a
    }

    /// W19-2b の通し: 一言 → git（既存の公式の Adapter）と Quillpad（事前実装の無い local の MCP の app）を発見 → Quillpad の Adapter と、git の出力を Quillpad に
    /// 渡す手順を作る → 写しで試す → 承認つきで走らせる → 2 つの service の実状態を試験の側で直に読む → 再起動（registry を data から作り直す）の後、branch が
    /// 変わっても生成 0 回で同じ手順が新しい値を渡して確かめる（期待する値を決め打ちにしていない）→ 違う値を書く server は済みにしない。
    #[test]
    fn w19_2b_run_hands_data_between_two_services_and_reuses_after_restart() {
        let d = tmp("2b");
        let log = d.join("mcp.jsonl");
        let repo = d.join("repo");
        git_repo(&repo, "topic-alpha");
        let head = git_out(&repo, &["rev-parse", "HEAD"]);
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let success = "文書の題が repo の今の branch の名前";
        let tdir = trial_dir(&d, &Operation { id: "op1".into(), want: String::new(), success: success.into(), target: None, ..Default::default() });
        let mcp = quill_cfg("", &log);
        let goal = format!("{} の repo の今の branch の名前を、{} の Quillpad の文書の題にして保存して", repo.display(), real.display());
        let interp = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": success, "target": null }] }));
        let fake = Fake::new(vec![interp(), Ok(handoff_answer(&repo, &real, &tdir.join("notes.quill.json"))), interp()]);
        // 起動の時の registry（手書きの Adapter → 前に作った Adapter〈extensions/〉→ check）。GitHub・Google は足さない（外へ通信しない）
        let boot = |mcp: &McpConfig| {
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(crate::adapters::git::Git::new())).unwrap();
            let (m, data) = (mcp.clone(), d.clone());
            assert_eq!(reg.add_recipes(&d.join(EXTENSIONS_DIR), &mut || call_runner(m.clone(), data.clone())), Vec::<String>::new());
            reg.check();
            reg
        };
        let mut reg = boot(&mcp);
        assert_eq!(reg.tier("git.status.read"), DisplayTier::Ready, "git が在る Mac（試験の前提）");
        let mut an = Vec::new();
        // 承認が無い: 作って写しで試すが、git を読んだだけで Quillpad は変えない（Denied・前の手は読むだけ）
        let denied = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        assert!(matches!(&denied, Err(EngineError::Denied(m)) if m.contains("only read")), "{denied:?}");
        assert_eq!(title_on_disk(&real), "Old");
        // 承認（受け渡した後の exact payload）は Denied が返した承認待ちの手から作る（試験が自分の oracle の値を写さない）。見せる値 = git の branch
        let approve = |r: &Result<EngineRecord, EngineError>| match r {
            Err(EngineError::Denied(Denial { pending: Some(p), .. })) => {
                assert_eq!((p.action.as_str(), p.digest.clone()), ("quillpad.doc.title.set", sdk::payload_digest(&p.action, &p.input)));
                (p.input["title"].clone(), [consent(&p.action, &p.input)])
            }
            other => panic!("承認待ちの手が返らない: {other:?}"),
        };
        let (shown, ok) = approve(&denied);
        assert_eq!(shown, json!("topic-alpha"), "見せる値 = git の実状態の branch（試験の側の oracle と同じ）");
        let first = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an)).unwrap();
        // 試験の側の oracle: Quillpad = disk の JSON を直に・git = git を直に（Engine・registry・MCP の server を通らない）
        assert_eq!(title_on_disk(&real), "topic-alpha", "Quillpad の実状態");
        let git_state = || (git_out(&repo, &["rev-parse", "HEAD"]), git_out(&repo, &["branch", "--show-current"]), git_out(&repo, &["status", "--porcelain"]));
        assert_eq!(git_state(), (head.clone(), "topic-alpha".to_string(), String::new()), "git の実状態（読んだだけ）");
        let handoffs: Vec<&Handoff> = first.receipts.iter().flat_map(|r| &r.steps).flat_map(|s| &s.handoffs).collect();
        let digest = crate::crypto::sha256_hex(json!("topic-alpha").to_string().as_bytes());
        assert_eq!(handoffs, [&Handoff { from_step: "read".into(), to_step: "set".into(), field: "title".into(), digest }], "受け渡しの記録（値は digest だけ）");
        assert_eq!(first.verified.len(), 2, "{:?}", first.verified);
        assert!(first.verified[0].starts_with("quillpad.doc.read の /title") && first.verified[0].contains("topic-alpha"), "{:?}", first.verified);
        assert!(first.verified[1].starts_with("git.status.read の /head/sha"), "{:?}", first.verified);
        assert!(first.reused && first.generated.is_empty(), "承認を待った手順を再利用した");
        let saved = load_workflows(&d.join(WORKFLOWS_DIR)).unwrap();
        assert_eq!(saved.iter().map(|w| w.steps.iter().map(|s| s.capability.as_str()).collect::<Vec<_>>()).collect::<Vec<_>>(), [["git.status.read", "quillpad.doc.title.set"]]);
        // RQ3 反証の直し（RQ-012）: 保存した手順は呼んだ Project の物（Project の Export が `ui` で引く。前は製品の全経路で None = 書き出しが常に空）
        assert_eq!(saved[0].ui.as_deref(), Some(format!("project.{}", CALLER.app_id).as_str()), "手順に呼んだ Project を付けていない");
        assert_eq!(fake.requests.lock().unwrap().iter().filter(|r| r.name == GENERATE_NAME).count(), 1, "生成は 1 回だけ");
        // 再起動: registry を data から作り直す（前に作った Adapter は extensions/ から）。その間に repo の branch が変わった
        drop(reg);
        git_out(&repo, &["checkout", "-q", "-b", "topic-beta"]);
        let mut reg = boot(&mcp);
        let (shown, ok2) = approve(&run(&goal, &mut cx(&mut reg, &d, None, &mcp, &[], &mut an)));
        assert_eq!(shown, json!("topic-beta"));
        let again = run(&goal, &mut cx(&mut reg, &d, None, &mcp, &ok2, &mut an)).unwrap();
        assert!(again.reused && again.generated.is_empty(), "{again:?}");
        assert_eq!(title_on_disk(&real), "topic-beta", "保存した手順が新しい値を渡した");
        assert!(again.verified[0].contains("topic-beta"), "期待する値は手の出力（決め打ちの topic-alpha ではない）: {:?}", again.verified);
        assert_eq!(fake.requests.lock().unwrap().len(), 3, "model を呼んでいない");
        // 負の対照: Quillpad の server が違う値を書く → 製品は済みと言わない（書いた後の読み戻しが合わない = Unconfirmed・送り直さない・G74）。本物が変わった事は試験の側が見る
        drop(reg);
        git_out(&repo, &["checkout", "-q", "-b", "topic-gamma"]);
        let wrong = quill_cfg("wrong", &log);
        let mut reg = boot(&wrong);
        let (_, ok3) = approve(&run(&goal, &mut cx(&mut reg, &d, None, &wrong, &[], &mut an)));
        let bad = run(&goal, &mut cx(&mut reg, &d, None, &wrong, &ok3, &mut an));
        assert!(matches!(&bad, Err(EngineError::Unconfirmed(m)) if m.contains("/title")), "{bad:?}");
        assert_eq!(title_on_disk(&real), "topic-gamma?");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証（W19-2b・高）: 読み戻しの期待する値の `$from` が書く手より後の手を指すと、書いた後の自分の読みと比べるだけになる（違う値を書いても「等しい」）。
    /// 走らせる前（`run_workflow`）と生成の検査（本物の手順・試験）で拒む。
    #[test]
    fn w19_2b_expected_value_comes_from_before_the_first_write() {
        let mut reg = AdapterRegistry::default();
        let (a, _, ac) = probe("alpha", &ALPHA, "alpha の文", false);
        let (b, bt, bc) = probe("beta", &BETA, "", false);
        reg.register(Box::new(a)).unwrap();
        reg.register(Box::new(b)).unwrap();
        let step = |id: &str, cap: &str, input: Value| WorkflowStep { kind: StepKind::Action, id: id.into(), capability: cap.into(), input };
        let wf = |from: &str| Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "copy".into(),
            goal: String::new(),
            steps: vec![step("read", "alpha.text.read", json!({})), step("write", "beta.text.write", json!({ "text": { "$from": "read.text" } })), step("readb", "beta.text.read", json!({}))],
            ui: None,
            checks: vec![ReadBack { action: "beta.text.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Equals { value: json!({ "$from": from }) } }],
        };
        let ok = consent("beta.text.write", &json!({ "text": "alpha の文" }));
        let r = run_workflow(&wf("readb.text"), &mut reg, CALLER, std::slice::from_ref(&ok));
        assert!(matches!(&r, Err(WorkflowError::Invalid(m)) if m.contains("書く手より後")), "{r:?}");
        assert_eq!((ac.lock().unwrap().len(), bc.lock().unwrap().len(), bt.lock().unwrap().as_str()), (0, 0, ""), "1 手も走らせない");
        assert!(run_workflow(&wf("read.text"), &mut reg, CALLER, std::slice::from_ref(&ok)).is_ok(), "正の対照: 書く前の手の出力なら走る");
        // 生成の検査: git → Quillpad の答えに、書いた後に Quillpad を読む手を足し、期待する値をその出力にする（本物の手順・試験の両方）
        let d = tmp("taut");
        let (repo, real) = (d.join("repo"), d.join("real").join("notes.quill.json"));
        git_repo(&repo, "topic-alpha");
        write_doc(&real, "Old");
        let o = Operation { id: "op1".into(), want: "w".into(), success: "s".into(), target: None, ..Default::default() };
        let copy = trial_dir(&d, &o).join("notes.quill.json");
        let mcp = quill_cfg("", &d.join("mcp.jsonl"));
        let found = mcp_records(&mcp, &[]);
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(crate::adapters::git::Git::new())).unwrap();
        reg.check();
        let mut an = Vec::new();
        let mut gen = |edit: &dyn Fn(&mut Value)| {
            let mut a = handoff_answer(&repo, &real, &copy);
            edit(&mut a);
            let fake = Fake::new(vec![Ok(a)]);
            generate(&o, &found, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an), None)
        };
        assert!(gen(&|_| {}).is_ok(), "正の対照: 期待する値が書く前の読む手（git）の出力");
        let readq = |path: &Path| json!({ "id": "readq", "capability": "quillpad.doc.read", "input_json": json!({ "path": path }).to_string() });
        let taut = json!(json!({ "$from": "readq.title" }).to_string());
        let real_wf = gen(&|a| {
            a["workflow_steps"].as_array_mut().unwrap().push(readq(&real));
            a["checks"][0]["value_json"] = taut.clone();
        });
        assert!(matches!(&real_wf, Err(EngineError::Generation(m)) if m.contains("書く手より後")), "{real_wf:?}");
        let trial = gen(&|a| {
            a["trial"]["steps"].as_array_mut().unwrap().push(readq(&copy));
            a["trial"]["checks"][0]["value_json"] = taut.clone();
        });
        assert!(matches!(&trial, Err(EngineError::Generation(m)) if m.contains("書く手より後")), "{trial:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 試験の側の HTTP の宛先（loopback・外へは出ない）: 受けた request の 1 行目を全部残し、{"ok":true} を返す。
    fn listener() -> (u16, Arc<Mutex<Vec<String>>>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                g.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string());
                let body = r#"{"ok":true}"#;
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (port, got)
    }

    /// 反証（W19-2b・高）: ある service で読んだ利用者の data（git の HEAD）を、受け渡し（`$from`）で外の service へ渡す手は、承認の要らない読む action
    /// （HTTP の GET）でも、受け渡した後の exact payload への承認が無ければ送らない（19 通目 §3 の 7・§4）。承認に要る物は Denied の pending から作る。
    /// 承認の前に走る試験では受け渡した値を外へ渡す手を作らない・HTTP の手を手元を読む手（local_read）と申告させない（gate を素通りする）。
    #[test]
    fn w19_2b_handing_user_data_to_an_outside_service_needs_approval() {
        let d = tmp("exfil");
        let repo = d.join("repo");
        git_repo(&repo, "topic-alpha");
        let head = git_out(&repo, &["rev-parse", "HEAD"]);
        let (port, got) = listener();
        let uri = format!("file://{}", repo.display());
        let lookup = |q: &str| json!({ "action": "websvc.lookup", "input_json": json!({ "q": q }).to_string(), "pointer": "/ok", "expect": "equals", "value_json": "true" });
        let answer = json!({
            "none_why": null, "workflow_steps": [step_answer("read", "git.status.read", json!({ "uri": uri })), step_answer("send", "websvc.lookup", json!({ "q": { "$from": "read.head.sha" } }))],
            "checks": [lookup("probe")],
            "recipe": { "id": "websvc", "provider": "websvc", "app": null, "actions": [
                { "id": "websvc.lookup", "effect": "external_read", "input_schema_json": json!({ "type": "object" }).to_string(),
                  "call_json": json!({ "via": "http", "method": "GET", "url": format!("http://127.0.0.1:{port}/?q={{{{input.q}}}}") }).to_string() },
            ] },
            "trial": { "steps": [step_answer("t", "websvc.lookup", json!({ "q": "trial" }))], "checks": [lookup("probe")] },
        });
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(crate::adapters::git::Git::new())).unwrap();
        reg.check();
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        {
            let o = Operation { id: "op0".into(), want: "w".into(), success: "s".into(), target: None, ..Default::default() };
            let mut gen = |edit: &dyn Fn(&mut Value)| {
                let mut a = answer.clone();
                edit(&mut a);
                let fake = Fake::new(vec![Ok(a)]);
                generate(&o, &[], &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an), None)
            };
            assert!(gen(&|_| {}).is_ok(), "正の対照");
            let t = gen(&|a| a["trial"]["steps"].as_array_mut().unwrap().push(step_answer("t2", "websvc.lookup", json!({ "q": { "$from": "t.ok" } }))));
            assert!(matches!(&t, Err(EngineError::Generation(m)) if m.contains("外の service へ渡す")), "{t:?}");
            let local = gen(&|a| a["recipe"]["actions"][0]["effect"] = json!("local_read"));
            assert!(matches!(&local, Err(EngineError::Generation(m)) if m.contains("local_read")), "{local:?}");
        }
        assert!(got.lock().unwrap().is_empty(), "生成の検査では呼ばない");
        let goal = format!("{} の repo の HEAD を問い合わせて", repo.display());
        let fake = Fake::new(vec![Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "問い合わせた", "target": null }] })), Ok(answer)]);
        let sent = || got.lock().unwrap().iter().filter(|l| l.contains(&head)).count();
        let denied = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(why)) = &denied else { panic!("{denied:?}") };
        assert_eq!(sent(), 0, "承認 0 件では HEAD を外へ送らない: {:?}", got.lock().unwrap());
        let p = why.pending.as_ref().expect("承認に要る物");
        assert_eq!((p.action.as_str(), &p.input, p.digest.clone()), ("websvc.lookup", &json!({ "q": head }), sdk::payload_digest("websvc.lookup", &json!({ "q": head }))));
        let ok = [consent(&p.action, &p.input)];
        let done = run(&goal, &mut cx(&mut reg, &d, None, &mcp, &ok, &mut an)).unwrap();
        assert!(done.reused, "{done:?}");
        assert_eq!(sent(), 1, "承認した内容を 1 回だけ送った: {:?}", got.lock().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証（W19-2b・中）: 2 つ目の操作が承認待ちで止まっても、前の操作が本物を書いた後なら Denied（何も変えていない）と言わない。
    #[test]
    fn w19_2b_a_later_operation_stopped_after_an_earlier_one_wrote_is_not_denied() {
        let d = tmp("ops");
        let mut reg = AdapterRegistry::default();
        let (a, at, _) = probe("alpha", &ALPHA, "old", false);
        let (b, bt, _) = probe("beta", &BETA, "", false);
        reg.register(Box::new(a)).unwrap();
        reg.register(Box::new(b)).unwrap();
        let check = |action: &str, value: Value| json!({ "action": action, "input_json": "{}", "pointer": "/text", "expect": "equals", "value_json": value.to_string() });
        let ops = json!({ "operations": [
            { "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "alpha の文を new に", "success": "alpha が new", "target": null },
            { "kind": "act", "named": false, "candidates": [], "id": "op2", "want": "alpha の文を beta に写す", "success": "beta が alpha と同じ", "target": null },
        ] });
        let fake = Fake::new(vec![
            Ok(ops),
            Ok(json!({ "none_why": null, "workflow_steps": [step_answer("w", "alpha.text.write", json!({ "text": "new" }))], "checks": [check("alpha.text.read", json!("new"))], "recipe": null, "trial": null })),
            Ok(json!({
                "none_why": null, "workflow_steps": [step_answer("r", "alpha.text.read", json!({})), step_answer("w", "beta.text.write", json!({ "text": { "$from": "r.text" } }))],
                "checks": [check("beta.text.read", json!({ "$from": "r.text" }))], "recipe": null, "trial": null,
            })),
        ]);
        let ok = [consent("alpha.text.write", &json!({ "text": "new" }))];
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let r = run("alpha の文を new にして、それを beta に写して", &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an));
        assert_eq!((at.lock().unwrap().as_str(), bt.lock().unwrap().as_str()), ("new", ""), "1 つ目の操作は本物を書き、2 つ目の書く手は送っていない");
        // G74c: 走った手（1 つ目の操作の書く手と 2 つ目の読む手）の後で止めた = Stopped（Verification の定型文にしない）
        assert!(matches!(&r, Err(EngineError::Stopped { step, ran: 2, .. }) if step == "w"), "{r:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 書いた結果が遅れて届く service（背景の app の保存は disk に 2〜33 秒遅れる）の試験の偽物: write は `delay` 後に read へ現れる。
    struct Lag {
        text: Arc<Mutex<(String, Option<(String, Instant)>)>>,
        delay: Duration,
    }
    static LAG: [Spec; 2] = specs("lag.text.read", "lag.text.write");
    impl Adapter for Lag {
        fn id(&self) -> &'static str {
            "lag"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::LocalCore
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            "lag"
        }
        fn specs(&self) -> &'static [Spec] {
            &LAG
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            let mut t = self.text.lock().unwrap();
            if action == "lag.text.write" {
                t.1 = Some((input["text"].as_str().unwrap_or("").into(), Instant::now() + self.delay));
                return (Outcome::Unverified { reason: "送った（届くのは後）".into() }, None);
            }
            if let Some((v, due)) = t.1.clone() {
                if Instant::now() >= due {
                    *t = (v, None);
                }
            }
            (Outcome::Verified { evidence: vec!["lag".into()] }, Some(sdk::observe("lag", sdk::now(), json!({ "text": t.0 }))))
        }
        fn cleanup(&mut self) {}
    }

    /// W19a2 の実機で見つけた（直し）: 書いた結果が遅れて届く時、読み戻しは `readback` まで読み直し、届いたら済み（書く手はもう一度送らない）。
    /// 待たない（`readback` = 0）と同じ run が Verification（負の対照）。
    #[test]
    fn w19_2_readback_waits_for_a_late_write_and_never_resends_it() {
        let d = tmp("lag");
        let check = json!({ "action": "lag.text.read", "input_json": "{}", "pointer": "/text", "expect": "equals", "value_json": json!("new").to_string() });
        let answer = json!({ "none_why": null, "workflow_steps": [step_answer("w", "lag.text.write", json!({ "text": "new" }))], "checks": [check], "recipe": null, "trial": null });
        let ops = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "lag を new に", "success": "lag が new", "target": null }] }));
        let ok = [consent("lag.text.write", &json!({ "text": "new" }))];
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        for (wait, pass) in [(Duration::ZERO, false), (Duration::from_secs(5), true)] {
            let state = Arc::new(Mutex::new(("old".to_string(), None)));
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(Lag { text: state.clone(), delay: Duration::from_millis(1500) })).unwrap();
            let fake = Fake::new(vec![ops(), Ok(answer.clone())]);
            let dir = d.join(format!("w{}", wait.as_secs()));
            let r = run("lag を new にして", &mut EngineContext { readback: wait, ..cx(&mut reg, &dir, Some(&fake), &mcp, &ok, &mut an) });
            if pass {
                let rec = r.unwrap();
                assert_eq!(rec.receipts[0].steps.len(), 1, "書く手は 1 回だけ（読み直しで送り直さない）");
                assert!(rec.verified[0].contains("/text"), "{rec:?}");
            } else {
                assert!(matches!(&r, Err(EngineError::Unconfirmed(_))), "待たないと済みにしない・書いた後なので「送れたか確かめられない」: {r:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// G74 戻し（owner 12:2x の ④「送れたか確かめられない時は unconfirmed のまま」）: 書く手を送った後の読み戻しの read 自体が失敗する（app が答えない）時も
    /// 「送れたか確かめられない」（Unconfirmed）・書く手は 1 回だけ。負の対照（手で取る）: 読み戻しの read の失敗を `?` で返す（前の形）→ Verification
    #[test]
    fn g74_a_read_that_fails_after_a_write_is_unconfirmed() {
        static GAMMA: [Spec; 2] = specs("gamma.text.list", "gamma.text.write");
        let d = tmp("g74-read");
        let mut reg = AdapterRegistry::default();
        let (a, at, calls) = probe("gamma", &GAMMA, "[]", false);
        reg.register(Box::new(a)).unwrap();
        // 前の読みは JSON（読める）・書いた後の text は JSON でない = .list が答えない（Timeout）
        let check = json!({ "action": "gamma.text.list", "input_json": "{}", "pointer": "", "expect": "equals", "value_json": "[]" });
        let ops = Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "gamma を new に", "success": "gamma が new", "target": null }] }));
        let fake = Fake::new(vec![ops, Ok(json!({ "none_why": null, "workflow_steps": [step_answer("w", "gamma.text.write", json!({ "text": "new" }))], "checks": [check], "recipe": null, "trial": null }))]);
        let ok = [consent("gamma.text.write", &json!({ "text": "new" }))];
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let r = run("gamma を new にして", &mut EngineContext { readback: Duration::ZERO, ..cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an) });
        assert!(matches!(&r, Err(EngineError::Unconfirmed(m)) if m.contains("gamma.text.list")), "{r:?}");
        assert_eq!(at.lock().unwrap().as_str(), "new");
        assert_eq!(calls.lock().unwrap().iter().filter(|c| c.ends_with(".write")).count(), 1, "送り直していない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W19-2 の配線: 承認の要る手が 2 つ（書いて保存）の手順は、1 回目の Denied が 2 手とも返し（`pending` と `later`）、その 2 つの承認で run し直すと
    /// 止まらずに通る（手ごとに止めない）。`pending` だけの承認では 1 手目が書いた後に止まる（負の対照）。input が決まらない後ろの手は `later` に入れない。
    #[test]
    fn w19_2_denied_returns_every_approval_the_run_can_know_now() {
        let d = tmp("later");
        let mut reg = AdapterRegistry::default();
        let (a, at, _) = probe("alpha", &ALPHA, "old", false);
        let (b, bt, _) = probe("beta", &BETA, "", false);
        reg.register(Box::new(a)).unwrap();
        reg.register(Box::new(b)).unwrap();
        let check = |action: &str, value: Value| json!({ "action": action, "input_json": "{}", "pointer": "/text", "expect": "equals", "value_json": value.to_string() });
        let ops = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "alpha と beta を new に", "success": "両方 new", "target": null }] }));
        let answer = json!({
            "none_why": null, "workflow_steps": [step_answer("w1", "alpha.text.write", json!({ "text": "new" })), step_answer("w2", "beta.text.write", json!({ "text": "new" }))],
            "checks": [check("alpha.text.read", json!("new")), check("beta.text.read", json!("new"))], "recipe": null, "trial": null,
        });
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let goal = "alpha と beta を new にして";
        let fake = Fake::new(vec![ops(), Ok(answer)]);
        let r = run(goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(why)) = &r else { panic!("{r:?}") };
        let p = why.pending.clone().expect("1 手目");
        assert_eq!(p.action, "alpha.text.write");
        assert_eq!(why.later.iter().map(|l| (l.action.as_str(), l.input.clone())).collect::<Vec<_>>(), [("beta.text.write", json!({ "text": "new" }))]);
        assert_eq!(why.later[0].digest, sdk::payload_digest("beta.text.write", &json!({ "text": "new" })));
        // 負の対照: pending だけを承認 → 1 手目が書いた後に 2 手目で止まる（Denied ではない）
        let only = [consent(&p.action, &p.input)];
        let half = run(goal, &mut cx(&mut reg, &d, None, &mcp, &only, &mut an));
        assert!(matches!(&half, Err(EngineError::Stopped { step, ran: 1, .. }) if step == "w2"), "{half:?}");
        *at.lock().unwrap() = "old".into();
        // 2 つとも承認 → 止まらずに通る
        let both = [consent(&p.action, &p.input), consent(&why.later[0].action, &why.later[0].input)];
        let done = run(goal, &mut cx(&mut reg, &d, None, &mcp, &both, &mut an)).unwrap();
        assert_eq!(done.receipts[0].steps.len(), 2);
        assert_eq!((at.lock().unwrap().as_str(), bt.lock().unwrap().as_str()), ("new", "new"));
        // 後ろの手の input が書く手の出力を指す（走らせるまで決まらない）→ later に入れない
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: "wf-x".into(),
            goal: goal.into(),
            steps: vec![
                WorkflowStep { kind: StepKind::Action, id: "w1".into(), capability: "alpha.text.write".into(), input: json!({ "text": "new" }) },
                WorkflowStep { kind: StepKind::Action, id: "w2".into(), capability: "beta.text.write".into(), input: json!({ "text": { "$from": "w1.written" } }) },
            ],
            ui: None,
            checks: vec![],
        };
        assert!(later(&wf, "w1", &HashMap::new(), &reg.actions()).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// W19-3 の本体の試験。解析の道具は試験の側の偽物（tests/fake-mcp-server.py の REA の役・tool の名前と evidence の記録の形だけを真似た物）を汎用の MCP の
/// client で呼ぶ（本物の REA は install も実行もしない・owner の ok 待ち）。見本の app Quillpad の束は試験の置き場の下に作り（/Applications に置かない）、
/// 公式の MCP は読むだけ（quill_read）・題を書く道は束の中の CLI（tests/fake-quillctl.py）にしか無い。model は `llm::fake::Fake`。
#[cfg(test)]
pub(crate) mod w19_3_body {
    use super::w19_2_body::{connected, consent, cx, quill_answer, quill_cfg, step_answer, title_on_disk, tmp, write_doc, QUILL_APP};
    use super::*;
    use crate::intel::analysis::{Finding, FindingKind, ANALYSIS_SCHEMA, REA_NOTE};
    use crate::llm::fake::Fake;
    use std::sync::{Mutex, MutexGuard};

    /// 解析する試験を 1 本ずつ（`ANALYZING` は process に 1 つ・並べると片方が Overloaded になる）。
    static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.example.quillpad</string><key>CFBundleShortVersionString</key><string>1.0</string><key>CFBundleExecutable</key><string>Quillpad</string></dict></plist>"#;

    /// 見本の app の束: Info.plist（bundle id・版・主の実行 file）・Contents/MacOS/Quillpad（`/usr/bin/true` の写し: 本物の REA の `open_binary` は
    /// CFBundleExecutable が Mach-O を指す束しか開かない・起動はしない）・Contents/MacOS/quillctl（tests/fake-quillctl.py の写し・実行できる）。
    pub(crate) fn quill_app(d: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let app = d.join("Quillpad.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), PLIST).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/Quillpad")).unwrap();
        let ctl = app.join("Contents/MacOS/quillctl");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-quillctl.py"), &ctl).unwrap();
        std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        app
    }

    /// Engine の MCP の設定: Quillpad の公式の MCP（読むだけ）と、REA の役の偽物（`rea_mode` = "" か "empty"）。
    pub(crate) fn mcp_cfg(d: &Path, rea_mode: &str) -> McpConfig {
        let quill = crate::mcp::w19_2_body::quill("readonly", &d.join("quill.jsonl"));
        let mut rea = crate::mcp::w19_2_body::quill(rea_mode, &d.join("rea.jsonl"));
        rea.env.insert("FAKE_MCP_ROLE".into(), "rea".into());
        rea.env.insert("FAKE_MCP_START_LOG".into(), d.join(REA_STARTS).display().to_string());
        connected(McpConfig { mcp_servers: [("quill".to_string(), quill), (REA_SERVER.to_string(), rea)].into_iter().collect() })
    }

    /// REA（偽物と、本物を包む sh）が起こされた回数を書く file（`<d>/rea-starts.log`・1 回 1 行）。
    const REA_STARTS: &str = "rea-starts.log";

    /// REA が起こされた回数（W19-3-fix: 1 run で 1 回）。
    pub(crate) fn rea_starts(d: &Path) -> usize {
        std::fs::read_to_string(d.join(REA_STARTS)).unwrap_or_default().lines().count()
    }

    /// 進行の 1 行の受け口と、受けた物（Some = 1 行・None = 消した）。
    pub(crate) fn recorder() -> (analysis::Progress, std::sync::Arc<Mutex<Vec<Option<String>>>>) {
        let got: std::sync::Arc<Mutex<Vec<Option<String>>>> = Default::default();
        let g = got.clone();
        (std::sync::Arc::new(move |x| g.lock().unwrap().push(x)), got)
    }

    /// REA の役の tools/call（tools/list は数えない）。
    pub(crate) fn rea_calls(d: &Path) -> Vec<Value> {
        std::fs::read_to_string(d.join("rea.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// model の答え: 解析で見つけた束の中の CLI（quillctl set-title）で題を書き、公式の MCP（quill_read・読むだけ）で読み戻す Adapter・本物の手順・写しの試験。
    pub(crate) fn cli_answer(app: &Path, real: &Path, trial: &Path, title: &str) -> Value {
        let ctl = app.join("Contents/MacOS/quillctl");
        let c = |path: &Path| json!({ "action": "quillpad.doc.read", "input_json": json!({ "path": path }).to_string(), "pointer": "/title", "expect": "equals", "value_json": json!(title).to_string() });
        let schema = json!({ "type": "object" }).to_string();
        json!({
            "none_why": null, "workflow_steps": [step_answer("set", "quillpad.doc.title.set", json!({ "path": real, "title": title }))],
            "checks": [c(real)],
            "recipe": { "id": "quillpad", "provider": "quillpad", "app": app.display().to_string(), "actions": [
                { "id": "quillpad.doc.title.set", "effect": "local_write", "input_schema_json": schema,
                  "call_json": json!({ "via": "cli", "argv": [ctl, "set-title", "{{input.path}}", "{{input.title}}"] }).to_string() },
                { "id": "quillpad.doc.read", "effect": "local_read", "input_schema_json": schema,
                  "call_json": json!({ "via": "mcp", "server": "quill", "tool": "quill_read", "arguments": { "path": "{{input.path}}" } }).to_string() },
            ] },
            "trial": { "steps": [step_answer("set", "quillpad.doc.title.set", json!({ "path": trial, "title": title }))], "checks": [c(trial)] },
        })
    }

    /// model の答え: 今の能力では行えない（手順 0・作る Adapter も無い = 通常の探索で手段が無い）。
    pub(crate) fn no_means() -> Value {
        json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null })
    }

    fn request(t: AnalysisTarget) -> AnalysisRequest {
        AnalysisRequest {
            target: t,
            question: "題を書く道".into(),
            consent: AnalysisConsent { granted_by: "試験".into(), at: "2026-10-24T00:00:00Z".into(), scope: "読むだけ".into() },
            placement: Placement::Local,
            timeout_ms: 20_000,
        }
    }

    /// 通し ①〜⑦: 一言 → 通常の探索（公式の MCP は読むだけ）で生成の model が手段無しと答える → 解析の承認の 1 行（何も解析しない）→ 承認つきで REA を MCP で 1 回 →
    /// 根拠つきの候補（Unverified）→ 候補を model に渡して Adapter を作る → 写しで試す → 書く手の承認 → 本物を変えて公式の MCP で読み戻す →
    /// 再起動の後は解析も生成もせずに同じ手順で確かめる。
    #[test]
    fn w19_3_run_analyzes_after_consent_generates_from_the_finding_verifies_and_reuses() {
        let _one = serial();
        let d = tmp("w19-3-run");
        let app = quill_app(&d);
        let target = app.display().to_string();
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let tdir = trial_dir(&d, &Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() });
        let mcp = mcp_cfg(&d, "");
        let goal = format!("{} の Quillpad の文書の題を『Shibubu Demo』にして保存して", real.display());
        let interp = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "文書の題が Shibubu Demo", "target": target }] }));
        let fake = Fake::new(vec![interp(), Ok(no_means()), Ok(no_means()), interp(), Ok(cli_answer(&app, &real, &tdir.join("notes.quill.json"), "Shibubu Demo")), interp(), interp()]);
        let names = || fake.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        let mut reg = AdapterRegistry::default();
        assert_eq!(analysis_backends(&mcp).len(), 1, "MCP の設定の rea だけが解析の Backend");
        assert!(analysis_backends(&quill_cfg("", &d.join("x.jsonl"))).is_empty(), "rea が無ければ解析しない");

        // ① の前: 通常の探索を先に試す（生成の model が手段無しと答える）→ 解析の承認が無いので REA の tool を呼ばず、承認の 1 行
        let mut an = analysis_backends(&mcp);
        let r1 = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(d1)) = &r1 else { panic!("{r1:?}") };
        let p1 = d1.pending.clone().expect("解析の承認待ち");
        assert_eq!((p1.action.as_str(), p1.input["app_name"].as_str(), p1.input["version"].as_str()), ("analysis.rea", Some("Quillpad"), Some("1.0")), "{p1:?}");
        assert_eq!((&p1.input["by"], &p1.input["tools"]), (&json!("REA"), &json!(["open_binary", "inspect_artifact"])), "呼ぶ tool を承認の範囲に（道具を起こさずに決まる）: {p1:?}");
        assert!(d1.later.is_empty());
        assert!(rea_calls(&d).is_empty(), "承認の前に解析しない（tools/list で道具を確かめただけ）");
        assert_eq!(rea_starts(&d), 1, "①: 解析の承認を求める前に道具を確かめるのに 1 回だけ起こす");
        assert_eq!(names(), [INTERPRET_NAME, GENERATE_NAME, GENERATE_NAME], "解析を求める前に、通常の探索で作れるかを生成の model で試した（0 手の作り直し 1 回・WF-010）");
        assert!(!d.join(ANALYSES_DIR).exists());
        assert_eq!(title_on_disk(&real), "Old");
        let line = crate::engine_page::failure_line(&EngineError::Denied(d1.clone()));
        assert!(line.contains("found no way") && line.contains("waiting for permission to analyze") && line.contains("nothing has been analyzed or sent yet"), "{line}");

        // ①〜⑤: 解析の承認つき → REA を 1 回 → 候補 → 生成 → 写しの試験 → 書く手の承認待ち（本物は変えない）
        let ok_an = [consent(&p1.action, &p1.input)];
        let mut an = analysis_backends(&mcp);
        let (progress, lines) = recorder();
        an[0].set_progress(progress);
        let r2 = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok_an, &mut an));
        let Err(EngineError::Denied(d2)) = &r2 else { panic!("{r2:?}") };
        assert_eq!(d2.pending.as_ref().map(|p| p.action.as_str()), Some("quillpad.doc.title.set"), "{d2:?}");
        assert_eq!(rea_starts(&d), 2, "②: 承認つきの run は道具を確かめた session で解析する（1 run で REA を 1 回だけ起こす・W19-3-fix (a)）");
        // W19-3-fix (b): 起こす間と読む間だけ 1 行・どちらも終われば消す（期限は env と probe の実測で決まるので頭だけを見る）
        let lines = lines.lock().unwrap().clone();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!((lines[0].as_deref(), &lines[1], &lines[3]), (Some("Starting the analysis tool (up to 90 s)"), &None, &None), "{lines:?}");
        assert!(lines[2].as_deref().is_some_and(|l| l.starts_with("The analysis tool is reading Quillpad (up to ") && l.ends_with(" s)")), "{lines:?}");
        assert_eq!(
            rea_calls(&d),
            [json!({ "tool": "open_binary", "arguments": { "path": target } }), json!({ "tool": "inspect_artifact", "arguments": {} })],
            "1 本の解析 = 1 つの session で対象の束を開いて 1 回読む（本物の REA 6.7.0 の契約）"
        );
        let reports: Vec<PathBuf> = std::fs::read_dir(d.join(ANALYSES_DIR)).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(reports.len(), 1, "{reports:?}");
        let id = reports[0].file_stem().unwrap().to_string_lossy().into_owned();
        let report = analysis::load_report(&d.join(ANALYSES_DIR), &id).unwrap();
        assert_eq!((report.backend.as_str(), report.target.bundle_id.as_deref(), report.target.version.as_deref()), (REA_SERVER, Some(QUILL_APP), Some("1.0")));
        assert!(report.findings.iter().any(|f| f.claim == "束の中の file: Contents/MacOS/quillctl（resource）"), "束の中の CLI（本物の REA と同じく file を並べるだけ）: {report:?}");
        assert!(report.findings.iter().all(|f| f.status == FindingStatus::Analyzed), "解析の事実まで（推測・呼べる・検証済みを名乗らない）");
        assert!(report.findings[0].evidence.iter().any(|e| e.contains("REA 6.7.0 の inspect_artifact")), "道具の版と tool が根拠に: {:?}", report.findings[0].evidence);
        assert!(report.constraints.iter().any(|c| c == REA_NOTE), "{:?}", report.constraints);
        let cands: Vec<CapabilityRecord> = load_records(&d.join(CAPABILITIES_DIR)).unwrap().into_iter().filter(|r| matches!(r.origin, Origin::Analysis { .. })).collect();
        assert_eq!(cands.len(), report.findings.len());
        assert!(cands.iter().all(|c| c.verification == Verification::Unverified && c.readiness() != CapabilityReadiness::Ready && c.versions == ["1.0"]), "{cands:?}");
        assert!(cands.iter().all(|c| c.constraints.iter().any(|x| x == NOT_CALLED)), "呼べるか未確認と書く");
        let gen = fake.requests.lock().unwrap().iter().rev().find(|r| r.name == GENERATE_NAME).map(|r| r.input.clone()).expect("解析の後の生成");
        assert!(gen.contains("Contents/MacOS/quillctl") && gen.contains(&cands[0].id), "解析の候補が生成の model に届いた: {gen}");
        assert_eq!(title_on_disk(&tdir.join("notes.quill.json")), "Shibubu Demo", "写しで試した");
        assert_eq!(title_on_disk(&real), "Old", "本物は書く手の承認まで変えない");

        // ⑥: 書く手の承認つき → 保存した手順で本物を変え、公式の MCP（読むだけ）で読み戻す。解析し直さない
        let write = json!({ "path": real, "title": "Shibubu Demo" });
        let ok_w = [consent("quillpad.doc.title.set", &write)];
        let mut an = analysis_backends(&mcp);
        let first = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok_w, &mut an)).unwrap();
        assert_eq!(title_on_disk(&real), "Shibubu Demo", "試験の側の oracle（disk を直に読む）");
        assert!(first.verified[0].contains("quillpad.doc.read") && first.verified[0].contains("/title"), "{:?}", first.verified);
        let da = first.stages.iter().find(|s| s.stage == Stage::DeepAnalysis).unwrap();
        assert!(matches!(&da.outcome, StageOutcome::Skipped { why } if why.contains("saved procedure")), "{da:?}");
        assert!(first.discovered.iter().any(|r| matches!(r.origin, Origin::Analysis { .. }) && r.verification == Verification::Unverified), "解析の候補は記録に残るが Ready にならない");
        assert!(first.discovered.iter().all(|r| !r.id.starts_with("mcp.rea")), "解析の道具の tool を能力の候補にしない");
        assert_eq!(rea_calls(&d).len(), 2);
        assert_eq!(rea_starts(&d), 2, "保存した手順の run は REA を起こさない");

        // ⑦: 再起動（registry を data から作り直す）→ 解析も生成もせずに同じ手順で、もう一度本物を変えて確かめる
        write_doc(&real, "Old");
        let mut fresh = AdapterRegistry::default();
        let (m, data) = (mcp.clone(), d.clone());
        assert_eq!(fresh.add_recipes(&d.join(EXTENSIONS_DIR), &mut || call_runner(m.clone(), data.clone())), Vec::<String>::new());
        let ok_w2 = [consent("quillpad.doc.title.set", &write)];
        let mut an = analysis_backends(&mcp);
        let again = run(&goal, &mut cx(&mut fresh, &d, Some(&fake), &mcp, &ok_w2, &mut an)).unwrap();
        assert_eq!(title_on_disk(&real), "Shibubu Demo");
        assert!(again.reused && again.generated.is_empty(), "{again:?}");
        assert_eq!(rea_calls(&d).len(), 2, "解析し直さない");
        assert_eq!(rea_starts(&d), 2, "再起動の後も REA を起こさない");
        assert_eq!(names(), [INTERPRET_NAME, GENERATE_NAME, GENERATE_NAME, INTERPRET_NAME, GENERATE_NAME, INTERPRET_NAME, INTERPRET_NAME], "手段無しの答え 2 回（0 手の作り直し 1 回・WF-010）と解析の後の生成 1 回だけ");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 安全で許された連携が見つからない時: その制約を report と、使えない候補 1 件（Unsupported・理由 = 制約）として残す（解析した事にも、使える事にもしない）。
    /// 同時に 1 本: 別の解析が走っている間は REA を呼ばない。
    #[test]
    fn w19_3_analysis_that_finds_no_means_records_the_limitation() {
        let _one = serial();
        let d = tmp("w19-3-none");
        let app = quill_app(&d);
        let mcp = mcp_cfg(&d, "empty");
        let mut reg = AdapterRegistry::default();
        let t = analysis_target(&app.display().to_string()).expect("Info.plist を読む");
        assert_eq!((t.bundle_id.as_deref(), t.version.as_deref(), t.path.as_str()), (Some(QUILL_APP), Some("1.0"), app.to_str().unwrap()));
        assert_eq!(analysis_target(&d.join("none.app").display().to_string()), None, "無い path は解析しない");
        assert_eq!(analysis_target("Quillpad"), None, "bundle id でも path でもない名前は解析しない");
        let req = request(t);
        let held = Busy::take().expect("試験は 1 本ずつ");
        let mut an = analysis_backends(&mcp);
        assert!(matches!(deep_analysis(&req, &mut cx(&mut reg, &d, None, &mcp, &[], &mut an)), Err(AnalysisError::Overloaded(_))));
        assert!(rea_calls(&d).is_empty(), "混んでいる間は呼ばない");
        drop(held);
        let got = deep_analysis(&req, &mut cx(&mut reg, &d, None, &mcp, &[], &mut an)).unwrap();
        assert_eq!(rea_calls(&d).len(), 2, "open_binary と inspect_artifact を 1 回ずつ");
        assert!(got.report.findings.is_empty());
        let c = &got.report.constraints;
        assert!(c.iter().any(|x| x.contains("外から呼べる口が見つからない")) && c.iter().any(|x| x.contains("coverage")) && c.iter().any(|x| x == REA_NOTE), "{c:?}");
        let [none] = got.candidates.as_slice() else { panic!("{:?}", got.candidates) };
        assert!(none.id.ends_with(".none") && matches!(&none.verification, Verification::Unsupported { reason } if reason.contains("外から呼べる口が見つからない")), "{none:?}");
        assert_eq!(analysis::load_report(&d.join(ANALYSES_DIR), &got.report.id).unwrap(), got.report, "制約を disk に残した");
        assert_eq!(load_records(&d.join(CAPABILITIES_DIR)).unwrap(), got.candidates);
        let t = tried(&got.candidates);
        assert!(t[0].contains("使えない: ") && t[0].contains("外から呼べる口が見つからない"), "手段が無い時の 1 行に制約が出る: {t:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// REA が入っていない（設定に在るが起こせない）時: 解析の段は理由つきで飛ばし、W19-2 の見本（Quillpad の公式の MCP）は同じに通る。rea の tool は候補にしない。
    #[test]
    fn w19_3_without_a_usable_rea_the_w19_2_path_still_works() {
        let _one = serial();
        let d = tmp("w19-3-missing");
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = Operation { id: "op1".into(), want: String::new(), success: String::new(), target: Some(QUILL_APP.into()), ..Default::default() };
        let tdir = trial_dir(&d, &o);
        let mut mcp = quill_cfg("", &d.join("mcp.jsonl"));
        mcp.mcp_servers.insert(REA_SERVER.into(), crate::mcp::McpServerConfig { command: d.join("no-rea").display().to_string(), ..Default::default() });
        let goal = format!("{} の Quillpad の文書の題を『Shibubu Demo』にして保存して", real.display());
        let interp = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "文書の題が Shibubu Demo", "target": QUILL_APP }] }));
        let fake = Fake::new(vec![interp(), Ok(quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo")), interp()]);
        let mut reg = AdapterRegistry::default();
        let mut an = analysis_backends(&mcp);
        assert!(matches!(an[0].availability(), Availability::Missing { .. }));
        let denied = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(dn)) = &denied else { panic!("{denied:?}") };
        assert_eq!(dn.pending.as_ref().map(|p| p.action.as_str()), Some("quillpad.doc.title.set"), "解析の承認を求めない: {dn:?}");
        let ok = [consent("quillpad.doc.title.set", &json!({ "path": real, "title": "Shibubu Demo" }))];
        let mut an = analysis_backends(&mcp);
        let rec = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an)).unwrap();
        assert_eq!(title_on_disk(&real), "Shibubu Demo");
        let da = rec.stages.iter().find(|s| s.stage == Stage::DeepAnalysis).unwrap();
        assert!(matches!(&da.outcome, StageOutcome::Skipped { .. }), "{da:?}");
        assert!(rec.discovered.iter().all(|r| !r.id.starts_with("mcp.rea")), "起こせない rea を候補の 1 件にもしない");
        assert!(!d.join(ANALYSES_DIR).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証の直し（指摘 1・2・19 通目 §3 の 3「通常の探索で足りない場合」・§4）: REA が Ready でも、通常の探索（W19-2 の見本 = 公式の MCP で書いて読む）で
    /// 足りる時は解析の承認を求めず、REA を呼ばない。逆向き: その app 専用の確かめ済みの能力が在っても、別の目的で生成の model が手段無しと答えたら解析の承認を求める。
    #[test]
    fn w19_3_analysis_is_asked_only_after_the_usual_means_fall_short() {
        let _one = serial();
        let d = tmp("w19-3-usual");
        let app = quill_app(&d);
        let target = app.display().to_string();
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let tdir = trial_dir(&d, &Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() });
        let mut mcp = mcp_cfg(&d, "");
        // 公式の MCP が題を書く tool も持つ（W19-2 の見本の quill("")）
        mcp.mcp_servers.insert("quill".into(), crate::mcp::w19_2_body::quill("", &d.join("quill.jsonl")));
        let mcp = connected(mcp);
        let goal = format!("{} の Quillpad の文書の題を『Shibubu Demo』にして保存して", real.display());
        let other = format!("{} の Quillpad の文書に表を足して", real.display());
        let interp = |g: &str| Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": g, "success": "元の文書に残る", "target": target }] }));
        let mut gen = quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo");
        gen["recipe"]["app"] = json!(target); // 作る Adapter はこの app 専用（下の逆向きの前提: その app 専用の確かめ済みの能力）
        let fake = Fake::new(vec![interp(&goal), Ok(gen), interp(&goal), interp(&other), Ok(no_means()), Ok(no_means())]);
        let names = || fake.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        let mut reg = AdapterRegistry::default();
        let mut an = analysis_backends(&mcp);
        let r1 = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(d1)) = &r1 else { panic!("{r1:?}") };
        assert_eq!(d1.pending.as_ref().map(|p| p.action.as_str()), Some("quillpad.doc.title.set"), "解析の承認でなく、公式の MCP で作った手順の書く手の承認: {d1:?}");
        assert!(rea_calls(&d).is_empty() && !d.join(ANALYSES_DIR).exists(), "解析しない");
        let ok = [consent("quillpad.doc.title.set", &json!({ "path": real, "title": "Shibubu Demo" }))];
        let mut an = analysis_backends(&mcp);
        let done = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok, &mut an)).unwrap();
        assert_eq!(title_on_disk(&real), "Shibubu Demo", "試験の側の oracle（disk を直に読む）");
        assert!(done.discovered.iter().all(|r| !matches!(r.origin, Origin::Analysis { .. })) && rea_calls(&d).is_empty(), "REA を入れても W19-2 の道は解析なしで通る");

        // 逆向き: この app 専用の確かめ済みの能力（上で作った Adapter）が在っても、別の目的で手段が無ければ解析の承認を求める
        assert!(reg.records().iter().any(|r| is_verified(r) && r.reuse == Reuse::AppSpecific { app: target.clone() }), "前提: この app 専用の確かめ済みの能力");
        let mut an = analysis_backends(&mcp);
        let r3 = run(&other, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let Err(EngineError::Denied(d3)) = &r3 else { panic!("{r3:?}") };
        let p3 = d3.pending.as_ref().expect("解析の承認待ち");
        assert_eq!((p3.action.as_str(), p3.input["app_name"].as_str()), ("analysis.rea", Some("Quillpad")), "{p3:?}");
        assert!(rea_calls(&d).is_empty(), "承認の前は解析しない");
        assert_eq!(names(), [INTERPRET_NAME, GENERATE_NAME, INTERPRET_NAME, INTERPRET_NAME, GENERATE_NAME, GENERATE_NAME]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証の直し（指摘 4）: 解析の道具を mcp.json の別の key（rea-agents）で入れても、tool の名前の組で見分け、能力の候補にせず（使えない印の 1 件だけ）、
    /// 作る Adapter から呼ばせない（解析は承認を取ってから Deep Analysis の段だけ）。
    #[test]
    fn w19_3_an_analysis_tool_under_another_key_is_neither_a_capability_nor_callable() {
        let d = tmp("w19-3-key");
        let app = quill_app(&d);
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let mut cfg = mcp_cfg(&d, "");
        let rea = cfg.mcp_servers.remove(REA_SERVER).unwrap();
        cfg.mcp_servers.insert("rea-agents".into(), rea);
        let found = mcp_records(&cfg, &["rea-agents"]);
        let ids: Vec<&str> = found.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.iter().all(|id| !id.starts_with("mcp.rea-agents.")), "REA の tool を能力の候補にしない: {ids:?}");
        assert!(found.iter().any(|r| r.id == "mcp.rea-agents" && matches!(&r.verification, Verification::Unsupported { reason } if reason.contains("解析の道具"))), "{ids:?}");
        assert!(ids.iter().any(|id| id.starts_with("mcp.quill.")), "他の server は今まで通り候補: {ids:?}");
        let o = Operation { id: "op1".into(), want: "題を変える".into(), success: "題".into(), target: Some(app.display().to_string()), ..Default::default() };
        let mut a = cli_answer(&app, &real, &trial_dir(&d, &o).join("notes.quill.json"), "T");
        a["recipe"]["actions"][0]["call_json"] = json!(json!({ "via": "mcp", "server": "rea-agents", "tool": "inspect_artifact", "arguments": { "path": "{{input.path}}" } }).to_string());
        let fake = Fake::new(vec![Ok(a)]);
        let mut reg = AdapterRegistry::default();
        let mut an = Vec::new();
        let got = generate(&o, &found, &mut cx(&mut reg, &d, Some(&fake), &cfg, &[], &mut an), None);
        assert!(matches!(&got, Err(EngineError::Generation(why)) if why.contains("解析の道具")), "{got:?}");
        assert!(rea_calls(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 反証の直し（指摘 5）の W19-3-real の版: REA の tool が壊さない（readOnlyHint = true か destructiveHint = false の明示）と申告しなければ解析の道具として
    /// 使わない（MCP の既定は destructive・申告は REA の物）。本物の REA 6.7.0 の解析の tool は readOnlyHint = false・destructiveHint = false（偽物は本物の一覧の写し）。
    /// allowlist に在っても申告が壊すと言う tool（open_binary が destructive の偽物）は呼ばない = 第 2 の検査。確かめていない版の REA は使わない。
    #[test]
    fn w19_3_rea_tools_that_do_not_say_non_destructive_or_other_versions_are_not_used() {
        let _one = serial();
        let d = tmp("w19-3-hint");
        let app = quill_app(&d);
        let mut an = analysis_backends(&mcp_cfg(&d, "unhinted"));
        let [b] = an.as_mut_slice() else { panic!("rea の Backend 1 つ") };
        assert!(matches!(b.availability(), Availability::Missing { why } if why.contains("won't break anything")), "annotations の無い REA の tool は使わない");
        let mut ok = analysis_backends(&mcp_cfg(&d, ""));
        assert!(matches!(ok[0].availability(), Availability::Ready { version, .. } if version == "6.7.0"), "正の対照: 本物と同じ申告の REA 6.7.0 は使える");
        let mut other = mcp_cfg(&d, "");
        other.mcp_servers.get_mut(REA_SERVER).unwrap().env.insert("FAKE_REA_VERSION".into(), "6.8.0".into());
        let mut newer = analysis_backends(&other);
        assert!(matches!(newer[0].availability(), Availability::Missing { why } if why.contains("6.8.0") && why.contains("6.7.0")), "確かめていない版は使わない（次の一手に確かめた版）");
        let cfg = mcp_cfg(&d, "destructive_open");
        let mut an = analysis_backends(&cfg);
        let t = analysis_target(&app.display().to_string()).unwrap();
        let req = request(t);
        let got = deep_analysis(&req, &mut cx(&mut AdapterRegistry::default(), &d, None, &cfg, &[], &mut an));
        assert!(matches!(&got, Err(AnalysisError::Unavailable(why)) if why.contains("open_binary")), "allowlist に在っても壊すと申告した tool は呼ばない: {got:?}");
        assert!(rea_calls(&d).is_empty(), "道具を確かめるだけで tool を呼ばない");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn report(findings: Vec<Finding>) -> AnalysisReport {
        AnalysisReport {
            schema: ANALYSIS_SCHEMA.into(),
            id: "an-pad-0123".into(),
            backend: REA_SERVER.into(),
            target: AnalysisTarget { bundle_id: Some(QUILL_APP.into()), path: "/x/Quillpad.app".into(), version: Some("1.0".into()) },
            findings,
            constraints: vec![REA_NOTE.into()],
            at: "2026-10-24T00:00:00Z".into(),
        }
    }

    fn finding(status: FindingStatus) -> Finding {
        Finding { kind: FindingKind::Binary, claim: "usage: padctl".into(), evidence: vec!["場所: Contents/MacOS/padctl".into()], confidence: 0.7, status }
    }

    /// 候補はどれも Ready にならない（Backend が検証済みと言っても）・根拠・版・制約を持つ・呼べるか未確認と推測を constraints で分ける。
    #[test]
    fn w19_3_candidates_are_never_ready_and_say_what_is_unconfirmed() {
        let all = [FindingStatus::Analyzed, FindingStatus::Inferred, FindingStatus::Callable, FindingStatus::Verified];
        let c = candidates_from(&report(all.iter().map(|&s| finding(s)).collect()));
        assert_eq!(c.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["analysis.an-pad-0123.1", "analysis.an-pad-0123.2", "analysis.an-pad-0123.3", "analysis.an-pad-0123.4"]);
        for r in &c {
            assert_eq!(r.verification, Verification::Unverified, "{r:?}");
            assert_ne!(r.readiness(), CapabilityReadiness::Ready);
            assert_eq!((&r.origin, &r.reuse, r.versions.as_slice()), (&Origin::Analysis { backend: REA_SERVER.into(), report: "an-pad-0123".into() }, &Reuse::AppSpecific { app: QUILL_APP.into() }, ["1.0".to_string()].as_slice()));
            assert!(r.evidence.iter().any(|e| e.contains("usage: padctl")) && r.evidence.iter().any(|e| e.contains("Contents/MacOS/padctl")) && r.constraints.iter().any(|x| x == REA_NOTE), "{r:?}");
        }
        let has = |i: usize, x: &str| c[i].constraints.iter().any(|y| y == x);
        assert!(has(0, NOT_CALLED) && has(1, NOT_CALLED) && !has(2, NOT_CALLED) && !has(3, NOT_CALLED));
        assert!(!has(0, INFERRED) && has(1, INFERRED) && !has(2, INFERRED));
        assert!(c[1].evidence[0].contains("Guess") && c[3].evidence[0].contains("Shibubu hasn't checked"));
        let mut bare = report(vec![]);
        bare.target.bundle_id = None;
        let none = candidates_from(&bare);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].reuse, Reuse::AppSpecific { app: "/x/Quillpad.app".into() }, "bundle id が無ければ path");
        assert!(matches!(&none[0].verification, Verification::Unsupported { reason } if reason == REA_NOTE));
    }

    /// 対象と違う物の結果・検証済みを名乗る結果は何も残さない。
    #[test]
    fn w19_3_deep_analysis_refuses_reports_that_claim_too_much() {
        struct Liar(AnalysisReport);
        impl AnalysisBackend for Liar {
            fn id(&self) -> &str {
                "liar"
            }
            fn availability(&mut self) -> Availability {
                Availability::Ready { version: "1".into(), probe_ms: 0 }
            }
            fn analyze(&mut self, _req: &AnalysisRequest) -> Result<AnalysisReport, AnalysisError> {
                Ok(self.0.clone())
            }
        }
        let _one = serial();
        let d = tmp("w19-3-liar");
        let mcp = McpConfig::default();
        let mut reg = AdapterRegistry::default();
        let honest = report(vec![finding(FindingStatus::Analyzed)]);
        let req = request(honest.target.clone());
        let mut lying = honest.clone();
        lying.findings[0].status = FindingStatus::Verified;
        let mut other = honest.clone();
        other.target.path = "/x/Other.app".into();
        for bad in [lying, other] {
            let mut an: Vec<Box<dyn AnalysisBackend>> = vec![Box::new(Liar(bad))];
            assert!(matches!(deep_analysis(&req, &mut cx(&mut reg, &d, None, &mcp, &[], &mut an)), Err(AnalysisError::Backend(_))));
        }
        assert!(!d.join(ANALYSES_DIR).exists() && !d.join(CAPABILITIES_DIR).exists(), "何も残さない");
        let mut an: Vec<Box<dyn AnalysisBackend>> = vec![Box::new(Liar(honest))];
        let ok = deep_analysis(&req, &mut cx(&mut reg, &d, None, &mcp, &[], &mut an)).unwrap();
        assert_eq!((ok.report.backend.as_str(), ok.report.id.clone()), ("liar", analysis_id("liar", &req.target)), "id と backend は Engine が付け直す");
        let mut none: Vec<Box<dyn AnalysisBackend>> = vec![];
        assert!(matches!(deep_analysis(&req, &mut cx(&mut reg, &d, None, &mcp, &[], &mut none)), Err(AnalysisError::Unavailable(_))));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 生成した Adapter から解析の道具を呼ばせない（解析は承認を取ってから Deep Analysis の段だけ）。
    #[test]
    fn w19_3_generated_adapters_cannot_call_the_analysis_tool() {
        let d = tmp("w19-3-gen");
        let app = quill_app(&d);
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let o = Operation { id: "op1".into(), want: "題を変える".into(), success: "題".into(), target: Some(app.display().to_string()), ..Default::default() };
        let mut a = cli_answer(&app, &real, &trial_dir(&d, &o).join("notes.quill.json"), "T");
        a["recipe"]["actions"][0]["call_json"] = json!(json!({ "via": "mcp", "server": REA_SERVER, "tool": "inspect_artifact", "arguments": { "path": "{{input.path}}" } }).to_string());
        let fake = Fake::new(vec![Ok(a)]);
        let mcp = mcp_cfg(&d, "");
        let mut reg = AdapterRegistry::default();
        let mut an = Vec::new();
        let got = generate(&o, &[], &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an), None);
        assert!(matches!(&got, Err(EngineError::Generation(why)) if why.contains("解析の道具")), "{got:?}");
        assert!(rea_calls(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W19-3-fix (c): 解析 1 回の待ちの上限 = env `SHIBUBU_ANALYSIS_TIMEOUT_S`（秒・既定 120）と、道具を確かめた実測 × 3 の大きい方。承認つきの対象の
    /// 解析の依頼（`analysis_plan` の `Run`）にその値が入る。負の対照（手で取る）: `analysis_timeout` の × 3 を外す → 赤
    #[test]
    fn w19_3_fix_analysis_timeout_grows_with_the_probe() {
        let s = Duration::from_secs;
        assert_eq!(analysis_timeout(None, s(10)), s(120), "既定 120 秒（実測 × 3 = 30 秒より大きい）");
        assert_eq!(analysis_timeout(None, s(50)), s(150), "混んで起こすのが遅い = 読むのも遅い（実測 × 3）");
        assert_eq!(analysis_timeout(Some(" 300 "), s(50)), s(300), "env の秒");
        assert_eq!(analysis_timeout(Some("30"), s(1)), s(30), "env で縮める");
        assert_eq!(analysis_timeout(Some("30"), s(40)), s(120), "env より実測 × 3 が大きい");
        for bad in ["0", "-5", "abc", "", "1.5"] {
            assert_eq!(analysis_timeout(Some(bad), s(1)), s(120), "{bad:?} は既定");
        }
        assert_eq!(analysis_timeout(Some("99999999999999"), s(1)), s(86_400), "1 日で頭打ち（Instant に足して溢れない）");
        // 配線: 承認つきの対象 → `Run` の依頼の期限が probe の実測から決まる
        struct Probed;
        impl AnalysisBackend for Probed {
            fn id(&self) -> &str {
                REA_SERVER
            }
            fn availability(&mut self) -> Availability {
                Availability::Ready { version: "6.7.0".into(), probe_ms: 50_000 }
            }
            fn analyze(&mut self, _req: &AnalysisRequest) -> Result<AnalysisReport, AnalysisError> {
                Err(AnalysisError::Backend("呼ばない".into()))
            }
        }
        let d = tmp("w19-3-fix-timeout");
        let app = quill_app(&d);
        let t = analysis_target(&app.display().to_string()).unwrap();
        let mut an: Vec<Box<dyn AnalysisBackend>> = vec![Box::new(Probed)];
        let ok = [consent(&analysis_action(REA_SERVER), &analysis_input(an[0].as_ref(), &t))];
        let op = Operation { id: "op1".into(), want: "題を変える".into(), success: "題".into(), target: Some(app.display().to_string()), ..Default::default() };
        let p = Plan { op, found: vec![], choice: None, workflow: None, recipe: None, trial: None, reused: false, recheck: false, alone: false, before: vec![], evidence: vec![], ask: None, hint: None, context: false };
        let (mcp, mut reg) = (McpConfig::default(), AdapterRegistry::default());
        let AnalysisPlan::Run(_, req) = analysis_plan("題を変える", &p, &[], &mut cx(&mut reg, &d, None, &mcp, &ok, &mut an)) else { panic!("承認つきの対象は解析する") };
        let want = analysis_timeout(std::env::var(ANALYSIS_TIMEOUT_ENV).ok().as_deref(), s(50));
        assert!(req.timeout_ms >= 150_000 && u128::from(req.timeout_ms) == want.as_millis(), "probe 50 秒 → 期限 150 秒以上: {}", req.timeout_ms);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 本物の REA（npm rea-agents@6.7.0 を worktree の外に入れた物・既定 `$HOME/kangae-rust-mvp-wt/.tools/rea`・`SHIBUBU_REA_MCP` で rea.mjs を替えられる）の
    /// MCP の設定。REA の HOME と TMPDIR は試験の空の dir（REA が利用者の HOME に何も書かない事を後で見る）。
    fn real_rea(d: &Path) -> (crate::mcp::McpServerConfig, PathBuf, PathBuf) {
        let script = std::env::var("SHIBUBU_REA_MCP").unwrap_or_else(|_| format!("{}/kangae-rust-mvp-wt/.tools/rea/node_modules/rea-agents/scripts/rea.mjs", std::env::var("HOME").unwrap_or_default()));
        assert!(Path::new(&script).is_file(), "本物の REA が無い（{script}）: npm i --prefix ~/kangae-rust-mvp-wt/.tools/rea rea-agents@{}", analysis::REA_VERSION);
        let (home, tmpd) = (d.join("rea-home"), d.join("rea-tmp"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&tmpd).unwrap();
        let env = [("HOME".to_string(), home.display().to_string()), ("TMPDIR".to_string(), tmpd.display().to_string())].into_iter().collect();
        // 起こされた回数を数える（W19-3-fix: 1 run で 1 回）: sh が 1 行書いてから node に exec（同じ pid のまま・stdio はそのまま）
        let args = vec!["-c".into(), "echo start >> \"$0\"; exec node \"$1\" mcp".into(), d.join(REA_STARTS).display().to_string(), script];
        (crate::mcp::McpServerConfig { command: "/bin/sh".into(), args, env, ..Default::default() }, home, tmpd)
    }

    fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                let bytes = std::fs::read(&p).unwrap_or_default();
                out.push((p, bytes));
            }
        }
        out.sort();
        out
    }

    /// この試験の process の子孫（REA の node・偽物の MCP・quillctl）。
    fn descendants(root: u32) -> Vec<u32> {
        let (mut all, mut todo) = (Vec::new(), vec![root]);
        while let Some(p) = todo.pop() {
            let out = Command::new("/usr/bin/pgrep").args(["-P", &p.to_string()]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            for c in out.split_whitespace().filter_map(|x| x.parse::<u32>().ok()) {
                all.push(c);
                todo.push(c);
            }
        }
        all
    }

    /// 試験の間だけ 2 秒おきに、子孫の network の接続を `lsof -n -P -i -a -p <pid…>` で取る（operator 10-24・(経過秒, 子孫の数, lsof の出力)）。
    fn watch_net(stop: std::sync::Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<(u64, usize, String)>> {
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut samples = Vec::new();
            loop {
                let pids = descendants(std::process::id());
                let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                let out = if pids.is_empty() {
                    String::new()
                } else {
                    Command::new("/usr/sbin/lsof").args(["-n", "-P", "-i", "-a", "-p", &list]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
                };
                samples.push((start.elapsed().as_secs(), pids.len(), out));
                let next = Instant::now() + Duration::from_secs(2);
                while Instant::now() < next {
                    if stop.load(Ordering::SeqCst) {
                        return samples;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
    }

    /// 本物の REA 6.7.0 で ①〜③（W19-3-real・画面なし・試験）: 本物の tools/list が fixture と同じ（名前と申告）→ 通常の探索で手段が無い → 解析の承認の 1 行
    /// （REA を呼ばない）→ 承認つきで本物の REA を 1 つの session で 1 回（open_binary → inspect_artifact）→ 根拠つきの候補（Unverified）が生成の model に届く。
    /// 試験の間ずっと子孫の network の接続を数え（0 件でなければ赤）、対象の束の中身・REA の HOME に何も書かれていない事と、束に置いた秘密が report と候補に
    /// 入らない事（holdout t5 の観点）を見る。model は偽物（本物の LLM は使わない）・対象は架空の束 Quillpad（主の実行 file は /usr/bin/true の写し・起動しない）。
    #[test]
    #[ignore = "本物の REA を起こす（~/kangae-rust-mvp-wt/.tools/rea・REA の起動だけで 20〜40 秒 × 4）。cargo test -- --ignored w19_3_real"]
    fn w19_3_real_rea_analyzes_after_consent_and_gives_candidates_with_evidence() {
        let _one = serial();
        let d = tmp("w19-3-real");
        let app = quill_app(&d);
        let (ghp, akia, body) = (format!("ghp_{}", "Q1w2E3r4T5y6U7i8O9p0A1s2D3f4G5"), format!("AKIA{}", "W193REALTESTKEYX"), "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::fs::write(app.join("Contents/Resources/settings.env"), format!("GITHUB_TOKEN={ghp}\nAWS_ACCESS_KEY_ID={akia}\n-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY-----\n")).unwrap();
        let before = files_under(&app);
        let target = app.display().to_string();
        let real = d.join("real").join("notes.quill.json");
        write_doc(&real, "Old");
        let tdir = trial_dir(&d, &Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() });
        let (rea, home, tmpd) = real_rea(&d);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let net = watch_net(stop.clone());
        let t0 = Instant::now();

        // 本物の一覧が fixture（tests/fixtures/rea/tools-list-6.7.0.json）と同じ: 名前と申告（偽物はこの写しを出す）
        let mut probe = crate::mcp::McpSession::open(&rea, analysis::REA_PROBE_TIMEOUT).expect("本物の REA を起こす");
        let tools = probe.tools().expect("本物の REA の tools/list");
        let version = probe.version().to_string();
        probe.close();
        let t_probe = t0.elapsed().as_secs();
        assert_eq!(rea_starts(&d), 1);
        assert_eq!(version, analysis::REA_VERSION);
        let fx: Value = serde_json::from_str(include_str!("../../tests/fixtures/rea/tools-list-6.7.0.json")).unwrap();
        let hints = |a: &Value| (a["readOnlyHint"] == true, a["destructiveHint"] == true, a["readOnlyHint"] == true || a["destructiveHint"] == false);
        let want: Vec<(String, (bool, bool, bool))> = fx["tools"].as_array().unwrap().iter().map(|t| (t["name"].as_str().unwrap().to_string(), hints(&t["annotations"]))).collect();
        let got: Vec<(String, (bool, bool, bool))> = tools.iter().map(|(t, h)| (t.name.clone(), (h.read_only, h.destructive, h.not_destructive))).collect();
        assert_eq!(got, want, "本物の REA 6.7.0 の一覧が fixture と違う（fixture を取り直す）");

        let mut mcp = mcp_cfg(&d, "");
        mcp.mcp_servers.insert(REA_SERVER.into(), rea);
        let goal = format!("{} の Quillpad の文書の題を『Shibubu Demo』にして保存して", real.display());
        let interp = || Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "文書の題が Shibubu Demo", "target": target }] }));
        let fake = Fake::new(vec![interp(), Ok(no_means()), Ok(no_means()), interp(), Ok(cli_answer(&app, &real, &tdir.join("notes.quill.json"), "Shibubu Demo"))]);
        let mut reg = AdapterRegistry::default();

        // ①: 通常の探索で手段が無い → 解析の承認の 1 行（本物の REA は tools/list で確かめただけ）
        let mut an = analysis_backends(&mcp);
        let r1 = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an));
        let t_ask = t0.elapsed().as_secs();
        let Err(EngineError::Denied(d1)) = &r1 else { panic!("{r1:?}") };
        let p1 = d1.pending.clone().expect("解析の承認待ち");
        assert_eq!((p1.action.as_str(), &p1.input["by"], &p1.input["tools"]), ("analysis.rea", &json!("REA"), &json!(["open_binary", "inspect_artifact"])), "{p1:?}");
        assert!(!d.join(ANALYSES_DIR).exists(), "承認の前に解析しない");
        assert_eq!(rea_starts(&d), 2, "①: 道具を確かめるのに 1 回だけ起こす");

        // ②③: 承認つき → 本物の REA を 1 つの session で 1 回 → 根拠つきの候補 → 生成の model に届く（写しの試験の後、書く手の承認待ち）
        let ok_an = [consent(&p1.action, &p1.input)];
        let mut an = analysis_backends(&mcp);
        // 進行の 1 行と、出した・消した時刻（run の頭からの ms）
        let seen: std::sync::Arc<Mutex<Vec<(u128, Option<String>)>>> = Default::default();
        let (s, t2) = (seen.clone(), Instant::now());
        an[0].set_progress(std::sync::Arc::new(move |x| s.lock().unwrap().push((t2.elapsed().as_millis(), x))));
        let r2 = run(&goal, &mut cx(&mut reg, &d, Some(&fake), &mcp, &ok_an, &mut an));
        let t_run = t0.elapsed().as_secs();
        let seen = seen.lock().unwrap().clone();
        println!("W19-3-real-fix: ②③ の進行の 1 行（ms・1 行）: {seen:?}");
        assert_eq!(rea_starts(&d), 3, "②③: 承認つきの run で本物の REA を起こすのは 1 回（確かめた session で解析する）");
        let lines: Vec<Option<&str>> = seen.iter().map(|(_, l)| l.as_deref()).collect();
        assert_eq!(lines.len(), 4, "{seen:?}");
        assert_eq!((lines[0], lines[1], lines[3]), (Some("Starting the analysis tool (up to 90 s)"), None, None), "{seen:?}");
        assert!(lines[2].is_some_and(|l| l.starts_with("The analysis tool is reading Quillpad (up to ")), "{seen:?}");
        stop.store(true, Ordering::SeqCst);
        let samples = net.join().unwrap();
        let Err(EngineError::Denied(d2)) = &r2 else { panic!("{r2:?}") };
        assert_eq!(d2.pending.as_ref().map(|p| p.action.as_str()), Some("quillpad.doc.title.set"), "{d2:?}");
        let reports: Vec<PathBuf> = std::fs::read_dir(d.join(ANALYSES_DIR)).unwrap().map(|e| e.unwrap().path()).collect();
        let [path] = reports.as_slice() else { panic!("{reports:?}") };
        let report = analysis::load_report(&d.join(ANALYSES_DIR), &path.file_stem().unwrap().to_string_lossy()).unwrap();
        let claims: Vec<&str> = report.findings.iter().map(|f| f.claim.as_str()).collect();
        assert!(claims.contains(&"束の中の file: Contents/MacOS/quillctl（resource）") && claims.contains(&"束の中の file: Contents/MacOS/Quillpad（executable）"), "{claims:?}");
        assert!(report.findings.iter().all(|f| f.status == FindingStatus::Analyzed && f.evidence[0].starts_with("REA 6.7.0 の inspect_artifact・evidence_id ev_")), "{:?}", report.findings);
        let c = &report.constraints;
        assert!(c.iter().any(|x| x.starts_with("REA が見ていない所: ")) && c.iter().any(|x| x.contains("untrusted") || x.contains("命令として扱わない")) && c.last().map(String::as_str) == Some(REA_NOTE), "{c:?}");
        let cands: Vec<CapabilityRecord> = load_records(&d.join(CAPABILITIES_DIR)).unwrap().into_iter().filter(|r| matches!(r.origin, Origin::Analysis { .. })).collect();
        assert_eq!(cands.len(), report.findings.len());
        assert!(cands.iter().all(|c| c.verification == Verification::Unverified && c.readiness() != CapabilityReadiness::Ready && c.versions == ["1.0"] && c.constraints.iter().any(|x| x == NOT_CALLED)), "{cands:?}");
        let gen = fake.requests.lock().unwrap().iter().rev().find(|r| r.name == GENERATE_NAME).map(|r| r.input.clone()).expect("解析の後の生成");
        assert!(gen.contains("Contents/MacOS/quillctl") && gen.contains(&cands[0].id), "解析の候補が生成の model に届いた");

        // 読むだけ・外へ出ない・秘密を写さない
        let conns: Vec<&(u64, usize, String)> = samples.iter().filter(|s| !s.2.trim().is_empty()).collect();
        println!(
            "W19-3-real: probe {t_probe}s・①の承認まで {t_ask}s・②③まで {t_run}s・接続の標本 {} 回（2 秒おき・子孫は最大 {} 個）・接続のあった標本 {} 回・REA の TMPDIR の file {} 個",
            samples.len(),
            samples.iter().map(|s| s.1).max().unwrap_or(0),
            conns.len(),
            files_under(&tmpd).len()
        );
        for s in &conns {
            println!("W19-3-real: 接続 {}s: {}", s.0, s.2.trim());
        }
        assert!(samples.len() >= 2 && samples.iter().any(|s| s.1 > 0), "標本が取れていない（見張りが測っていない）: {samples:?}");
        assert!(conns.is_empty(), "本物の REA（か子孫）が network に繋いだ: {conns:?}");
        assert_eq!(files_under(&app), before, "対象の束の中身が変わった（読むだけでない）");
        assert!(files_under(&home).is_empty(), "REA が HOME に書いた: {:?}", files_under(&home).iter().map(|f| &f.0).collect::<Vec<_>>());
        let text = format!("{}{}", serde_json::to_string(&report).unwrap(), serde_json::to_string(&cands).unwrap());
        for s in [ghp.as_str(), akia.as_str(), body] {
            assert!(!text.contains(s), "束に置いた秘密が report か候補に入った");
        }
        assert!(text.contains("Contents/Resources/settings.env"), "file の名前は残る（何も写さない実装で通らないように）");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// WF-D（WF0〜WF2・docs/diagrams.md §47）: Decide で止まる・runs 表に残る・再起動の後に cursor の続きから・止まっている間に変わったら送らない。
/// 偽の app（Desk: 手元の note・下書き・送った文の台帳）を試験の側の oracle が直に読む。外へは何も送らない。engine_page の Page の試験も使う。
#[cfg(test)]
pub(crate) mod wf_d {
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::domain::permission::ConsentDecision;
    use crate::domain::run::RunStatus;
    use crate::llm::fake::Fake;
    use crate::runtime::receipt::{self, RunRecord};
    use crate::storage::{Store, Table};
    use std::sync::{Arc, Mutex, MutexGuard};

    pub(crate) const APP: &str = "app.desk";
    const SESSION: &str = "s-desk";
    const PROJECT: &str = "project.desk";
    /// 通しの一言（保存した手順 `titled` の目的）
    pub(crate) const GOAL: &str = "Pick a title for my note and set it";
    const SEND_GOAL: &str = "Send the short title of my note";

    /// 偽の app の実状態。`calls` = 受けた手（action と input）・`crash` = この action を受けて記録した後に落ちる（送った後・結果を返す前に process が死んだ形）。
    #[derive(Debug, Default)]
    pub(crate) struct Desk {
        pub note: String,
        /// note を読むと一緒に返る秘密（WF-004: runs 表に残さない）
        pub secret: String,
        pub sent: Vec<String>,
        pub calls: Vec<(String, Value)>,
        pub crash: Option<&'static str>,
    }
    impl Desk {
        pub(crate) fn count(&self, action: &str) -> usize {
            self.calls.iter().filter(|(a, _)| a == action).count()
        }
    }

    static DESK: [Spec; 4] = [
        Spec { id: "desk.note.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desk.draft.make", effect: Effect::LocalWrite, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desk.note.set", effect: Effect::LocalWrite, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desk.note.send", effect: Effect::LocalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
    ];

    pub(crate) struct DeskAdapter(pub Arc<Mutex<Desk>>);
    impl Adapter for DeskAdapter {
        fn id(&self) -> &'static str {
            "desk"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::LocalCore
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            "desk"
        }
        fn specs(&self) -> &'static [Spec] {
            &DESK
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            let mut d = self.0.lock().unwrap_or_else(|e| e.into_inner());
            d.calls.push((action.into(), input.clone()));
            if d.crash == Some(action) {
                drop(d);
                panic!("試験: {action} を送った後に落ちた");
            }
            let text = input["text"].as_str().unwrap_or("").to_string();
            let payload = match action {
                "desk.note.read" => json!({ "text": d.note, "token": d.secret, "last": d.sent.last() }),
                "desk.draft.make" => {
                    let from = input["from"].as_str().unwrap_or("");
                    json!({ "short": format!("{from} (short)"), "long": format!("{from}: the long title") })
                }
                "desk.note.set" => {
                    d.note = text;
                    json!({ "written": true })
                }
                "desk.note.send" => {
                    d.sent.push(text);
                    json!({ "sent": true })
                }
                _ => return (Outcome::Failed { error: ErrorCode::Timeout }, None),
            };
            let o = if action.ends_with(".read") { Outcome::Verified { evidence: vec!["desk".into()] } } else { Outcome::Unverified { reason: "書いた".into() } };
            (o, Some(sdk::observe("desk", sdk::now(), payload)))
        }
        fn cleanup(&mut self) {}
    }

    pub(crate) fn desk_registry(desk: &Arc<Mutex<Desk>>) -> AdapterRegistry {
        let mut r = AdapterRegistry::default();
        r.register(Box::new(DeskAdapter(desk.clone()))).unwrap();
        r.check();
        r
    }

    fn step(id: &str, cap: &str, input: Value) -> WorkflowStep {
        WorkflowStep { kind: StepKind::Action, id: id.into(), capability: cap.into(), input }
    }

    fn op1() -> Operation {
        Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() }
    }

    /// 判断の手: 下書きの 2 つの題のどちらか（証拠 = その題の文字と 1 行の得失）
    fn pick() -> WorkflowStep {
        let c = |id: &str, label: &str, from: &str, note: &str| DecideChoice { id: id.into(), label: label.into(), value: json!({ "$from": from }), note: note.into() };
        let choices = vec![c("short", "Use the short title", "draft.short", "Fits the list"), c("long", "Use the long title", "draft.long", "Says more")];
        WorkflowStep { kind: StepKind::Decide { question: "Which title?".into(), choices }, id: "pick".into(), capability: String::new(), input: Value::Null }
    }

    /// 通しの保存した手順: note を読む → 手元に下書きを書く → 判断（2 手が済んだ後）→ 選んだ題を note に。読み戻し = note が選んだ題。
    pub(crate) fn titled(goal: &str) -> Workflow {
        Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: workflow_id(goal, &op1()),
            goal: goal.into(),
            steps: vec![step("read", "desk.note.read", json!({})), step("draft", "desk.draft.make", json!({ "from": { "$from": "read.text" } })), pick(), step("set", "desk.note.set", json!({ "text": { "$from": "pick.value" } }))],
            ui: None,
            checks: vec![ReadBack { action: "desk.note.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Equals { value: json!({ "$from": "pick.value" }) } }],
        }
    }

    /// 承認の要る手の手順: note を読む → 手元に下書きを書く（承認の要らない書く手）→ 短い題を送る（承認が要る）。
    fn sending(goal: &str) -> Workflow {
        Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(), split: Vec::new(),
            id: workflow_id(goal, &op1()),
            goal: goal.into(),
            steps: vec![step("read", "desk.note.read", json!({})), step("draft", "desk.draft.make", json!({ "from": { "$from": "read.text" } })), step("send", "desk.note.send", json!({ "text": { "$from": "draft.short" } }))],
            ui: None,
            checks: vec![ReadBack { action: "desk.note.read".into(), input: json!({}), pointer: "/last".into(), expect: Expect::Contains { text: " (short)".into() } }],
        }
    }

    /// 1 つの data dir・runs 表（memory か file の sqlite）・偽の app。
    struct Rig {
        dir: PathBuf,
        db: Option<PathBuf>,
        desk: Arc<Mutex<Desk>>,
        store: Mutex<Store>,
    }

    fn open(db: Option<&Path>) -> Store {
        let s = db.map_or_else(|| Store::open_in_memory().unwrap(), |p| Store::open(p).unwrap());
        s.put(Table::Apps, APP, None, &json!({})).unwrap();
        s.put(Table::Sessions, SESSION, Some(APP), &json!({})).unwrap();
        s
    }

    fn rig(name: &str, note: &str, file: bool) -> Rig {
        let dir = w19_2_body::tmp(name);
        for wf in [titled(GOAL), sending(SEND_GOAL)] {
            save_workflow(&dir.join(WORKFLOWS_DIR), &wf).unwrap();
        }
        let db = file.then(|| dir.join("kangae.sqlite3"));
        let store = Mutex::new(open(db.as_deref()));
        Rig { dir, db, desk: Arc::new(Mutex::new(Desk { note: note.into(), ..Default::default() })), store }
    }

    impl Rig {
        fn desk(&self) -> MutexGuard<'_, Desk> {
            self.desk.lock().unwrap_or_else(|e| e.into_inner())
        }
        fn row(&self, run_id: &str) -> RunRecord {
            receipt::load(&self.store.lock().unwrap(), run_id).unwrap().expect("runs 表に行")
        }
        /// Shibubu を閉じて開く: store を捨てて同じ sqlite を開き直し、起動の後始末（`receipt::recover`）を 1 回。戻り = 後始末した数
        fn reopen(&mut self) -> usize {
            self.store = Mutex::new(open(self.db.as_deref()));
            receipt::recover(&self.store.lock().unwrap()).unwrap()
        }
        /// 毎回新しい registry（起動ごとに作り直す製品と同じ）で、この run の Engine の部品を作って `f` に渡す
        fn go<T>(&self, run_id: &str, consents: &[ConsentReceipt], llm: Option<&dyn Llm>, f: impl FnOnce(&mut EngineContext) -> T) -> T {
            let mut reg = desk_registry(&self.desk);
            let mcp = McpConfig::default();
            let mut an = Vec::new();
            let mut cx = w19_2_body::cx(&mut reg, &self.dir, llm, &mcp, consents, &mut an);
            cx.caller = Caller { app_id: APP, run_id: Some(run_id) };
            cx.runs = Some(RunsTable { store: &self.store, session_id: SESSION, space_id: PROJECT, app_version: 1 });
            f(&mut cx)
        }
    }

    /// 承認の card を押した時の承認（Engine が返した承認待ちの手の exact payload・この run の id）
    fn approve(p: &PendingApproval, run_id: &str) -> ConsentReceipt {
        ConsentReceipt {
            id: format!("c-{}", crate::model::new_id()),
            app_id: APP.into(),
            subject: ConsentSubject::Action { action_ref: p.action.clone(), run_id: Some(run_id.into()) },
            scopes: p.scopes.clone(),
            payload_digest: Some(p.digest.clone()),
            decision: ConsentDecision::Approved,
            decided_at: "2026-10-28T00:00:00Z".into(),
        }
    }

    fn interp() -> Result<Value, crate::llm::LlmError> {
        Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": SEND_GOAL, "success": "送った", "target": null }] }))
    }

    /// WF0: 旧い手順の file（kind の無い手）はそのまま読めて Action・書き直しても同じ byte（既定の kind は書かない）。Decide は tag 付きで往復する。
    /// 判断の選択肢の `$from` が後ろの手を指す手順は拒む。
    #[test]
    fn wf0_old_steps_read_as_actions_and_write_back_the_same_bytes() {
        let old = r#"{"id":"read","capability":"desk.note.read","input":{}}"#;
        let s: WorkflowStep = serde_json::from_str(old).unwrap();
        assert_eq!(s.kind, StepKind::Action);
        assert_eq!(serde_json::to_string(&s).unwrap(), old, "旧い file を書き直しても同じ byte");
        let p = pick();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"]["type"], json!("decide"));
        assert_eq!(serde_json::from_value::<WorkflowStep>(v).unwrap(), p);
        assert!(serde_json::from_value::<WorkflowStep>(json!({ "id": "x", "kind": { "type": "decide", "question": "?", "choices": [], "when": "a" } })).is_err(), "未知の欄は拒否");
        validate(&titled(GOAL)).unwrap();
        let mut early = titled(GOAL);
        early.steps.swap(1, 2);
        assert!(matches!(validate(&early), Err(WorkflowError::Invalid(m)) if m.contains("pick")), "下書きより前の判断は下書きの値を指せない");
        let mut none = titled(GOAL);
        none.steps[2] = WorkflowStep { kind: StepKind::Decide { question: "Which?".into(), choices: vec![] }, ..pick() };
        assert!(matches!(validate(&none), Err(WorkflowError::Invalid(_))), "選択肢の無い判断");
        let _: fn(&str, &str, Option<&str>, &mut EngineContext) -> Result<EngineRecord, EngineError> = resume;
        let _: fn(&Store) -> Vec<PausedRun> = waiting_runs;
    }

    /// WF-017 (1)・WF-016: 2 手（読む・手元に下書きを書く）が済んだ後の判断で `waiting_human` に止まり、後ろの手は送らない。選んだ道だけが cursor の続きから走る
    /// （前の書く手は走り直さない・選ばなかった値はどの手にも渡らない・判断は承認・同意の receipt を作らない）。
    /// 負の対照: `run_steps` の Decide の腕で止まらずに進ませると set が選ぶ前に走って赤・選んだ値の代わりに 1 つ目を渡すと note の oracle が赤
    #[test]
    fn wf017_1_waits_after_two_steps_and_runs_only_the_chosen_path() {
        let r = rig("wf017-1", "Notes", false);
        let got = r.go("run-1", &[], None, |cx| run(GOAL, cx));
        let Err(EngineError::Decide(d)) = &got else { panic!("判断で止まらない: {got:?}") };
        assert_eq!((d.step.as_str(), d.question.as_str()), ("pick", "Which title?"));
        assert_eq!(d.choices.iter().map(|c| c.value.clone()).collect::<Vec<_>>(), [json!("Notes (short)"), json!("Notes: the long title")], "証拠 = 前の手の出力で埋めた値");
        let d0 = r.desk();
        assert_eq!((d0.count("desk.draft.make"), d0.count("desk.note.set"), d0.note.as_str()), (1, 0, "Notes"), "2 手が済み、後ろの手は送っていない: {:?}", d0.calls);
        drop(d0);
        let rec = r.row("run-1");
        assert_eq!((rec.run.status, rec.space_id.as_str(), rec.run.app_id.as_str()), (RunStatus::WaitingHuman, PROJECT, APP));
        let c = rec.workflow.clone().unwrap();
        assert_eq!((c.plan, c.next, c.waiting.as_deref()), (0, 2, Some("pick")));
        assert_eq!(rec.run.steps.iter().map(|s| (s.id.as_str(), s.attempts.len())).collect::<Vec<_>>(), [("0.read", 1), ("0.draft", 1), ("0.pick", 0), ("0.set", 0)], "走った手だけ attempt");
        let paused = waiting_runs(&r.store.lock().unwrap());
        assert_eq!(paused.iter().map(|p| (p.space_id.as_str(), p.run_id.as_str(), p.goal.as_str())).collect::<Vec<_>>(), [(PROJECT, "run-1", GOAL)]);
        assert!(matches!(&paused[0].asked, Asked::Decide(x) if x == d), "{:?}", paused[0].asked);
        let done = r.go("run-1", &[], None, |cx| resume(GOAL, "run-1", Some("long"), cx)).unwrap();
        let d1 = r.desk();
        assert_eq!(d1.note, "Notes: the long title", "試験の側の oracle: 選んだ値");
        assert_eq!((d1.count("desk.draft.make"), d1.count("desk.note.set")), (1, 1), "下書きは走り直さない・set は 1 回: {:?}", d1.calls);
        assert!(d1.calls.iter().all(|(_, i)| !i.to_string().contains("(short)")), "選ばなかった値はどの手にも渡らない: {:?}", d1.calls);
        drop(d1);
        assert!(done.verified.iter().any(|l| l.contains("Notes: the long title")), "{:?}", done.verified);
        let rec = r.row("run-1");
        assert_eq!(rec.run.status, RunStatus::Verified);
        assert!(rec.approval.is_none() && rec.consent.is_none(), "判断は承認・同意の receipt を作らない");
        assert!(waiting_runs(&r.store.lock().unwrap()).is_empty());
        // もう待っていない run を選んでも送らない
        let again = r.go("run-1", &[], None, |cx| resume(GOAL, "run-1", Some("short"), cx));
        assert!(matches!(&again, Err(EngineError::Denied(_))), "{again:?}");
        assert_eq!(r.desk().count("desk.note.set"), 1);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-017 (2)・WF-016: 待ったまま閉じて（registry も store も捨てる）同じ data dir と sqlite で開き直すと、同じ判断が同じ Project に戻り、
    /// 済んだ書き込み（下書き）を再送しない。起動の後始末（`receipt::recover`）は待っている run を触らない。
    /// 負の対照: recover が待っている run も failed にすると（running と同じ扱い）waiting_runs が空で赤
    #[test]
    fn wf017_2_a_restart_brings_back_the_same_choice_and_does_not_resend() {
        let mut r = rig("wf017-2", "Plan", true);
        let Err(EngineError::Decide(d)) = r.go("run-2", &[], None, |cx| run(GOAL, cx)) else { panic!("判断で止まらない") };
        assert_eq!(r.reopen(), 0, "待っている run は後始末しない");
        let paused = waiting_runs(&r.store.lock().unwrap());
        assert_eq!(paused.len(), 1);
        assert_eq!((paused[0].space_id.as_str(), &paused[0].asked), (PROJECT, &Asked::Decide(d)), "同じ判断が同じ Project に");
        r.go("run-2", &[], None, |cx| resume(GOAL, "run-2", Some("short"), cx)).unwrap();
        let d = r.desk();
        assert_eq!(d.note, "Plan (short)");
        assert_eq!((d.count("desk.draft.make"), d.count("desk.note.set")), (1, 1), "開き直した後も下書きを再送しない: {:?}", d.calls);
        drop(d);
        assert_eq!(r.row("run-2").run.status, RunStatus::Verified);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-017 (3)・WF-015: 止まっている間に元データ（note）が変わったら、古い判断・承認で 1 手も送らず理由 1 行と出し直しの card（run は待ったまま）。
    /// もう一度押すと今の元データで続ける。承認の run も同じ（古い承認を使わない）。保存した手順が書き直されたら送らない（WF-002）。
    /// 負の対照: `resume` の読み直しの比較を外すと、変わった後の古い判断で set が走って赤
    #[test]
    fn wf017_3_a_changed_source_sends_nothing_and_asks_again() {
        // 判断
        let r = rig("wf017-3", "Notes", false);
        let Err(EngineError::Decide(_)) = r.go("run-3", &[], None, |cx| run(GOAL, cx)) else { panic!("判断で止まらない") };
        r.desk().note = "Notes, edited by hand".into();
        let got = r.go("run-3", &[], None, |cx| resume(GOAL, "run-3", Some("long"), cx));
        let Err(EngineError::Changed { why, asked: Asked::Decide(again) }) = &got else { panic!("変わったのに送った・理由が無い: {got:?}") };
        assert!(why.contains("nothing was sent"), "{why}");
        assert!(why.starts_with("Changed since last checked · Review"), "RQ-027 の語: {why}");
        assert_eq!(again.step, "pick", "同じ判断を出し直す");
        let d = r.desk();
        assert_eq!((d.count("desk.draft.make"), d.count("desk.note.set"), d.note.as_str()), (1, 0, "Notes, edited by hand"), "1 手も送らない: {:?}", d.calls);
        drop(d);
        assert_eq!(r.row("run-3").run.status, RunStatus::WaitingHuman, "待ったまま");
        r.go("run-3", &[], None, |cx| resume(GOAL, "run-3", Some("long"), cx)).expect("見直して押すと今の元データで続ける");
        assert_eq!(r.desk().count("desk.note.set"), 1);
        // 承認: 下書き（承認の要らない書く手）の後の送る手で止まる → 元データが変わる → 古い承認では送らない → 出し直した承認で 1 回だけ送る
        let Err(EngineError::Denied(Denial { pending: Some(p), .. })) = r.go("run-3b", &[], None, |cx| run(SEND_GOAL, cx)) else { panic!("承認で止まらない") };
        assert_eq!(r.row("run-3b").run.status, RunStatus::WaitingApproval);
        r.desk().note = "Changed".into();
        let old = [approve(&p, "run-3b")];
        let got = r.go("run-3b", &old, None, |cx| resume(SEND_GOAL, "run-3b", None, cx));
        let Err(EngineError::Changed { asked: Asked::Approve { steps }, .. }) = &got else { panic!("古い承認で送った: {got:?}") };
        assert!(r.desk().sent.is_empty(), "古い承認を使わない");
        let fresh = [approve(&steps[0], "run-3b")];
        r.go("run-3b", &fresh, None, |cx| resume(SEND_GOAL, "run-3b", None, cx)).unwrap();
        let d = r.desk();
        assert_eq!((d.sent.len(), d.count("desk.draft.make")), (1, 2), "送るのは 1 回・下書きはこの run で 1 回だけ（前の run の 1 回と合わせて 2）: {:?}", d.calls);
        drop(d);
        // WF-002: 止まっている間に保存した手順が書き直された → 送らない
        let Err(EngineError::Decide(_)) = r.go("run-3c", &[], None, |cx| run(GOAL, cx)) else { panic!("判断で止まらない") };
        let mut wf = titled(GOAL);
        wf.checks[0].pointer = "/token".into();
        save_workflow(&r.dir.join(WORKFLOWS_DIR), &wf).unwrap();
        let sets = r.desk().count("desk.note.set");
        let got = r.go("run-3c", &[], None, |cx| resume(GOAL, "run-3c", Some("short"), cx));
        assert!(matches!(&got, Err(EngineError::Denied(d)) if d.contains("procedure")), "{got:?}");
        assert_eq!(r.desk().count("desk.note.set"), sets, "書き直された手順の続きを走らせない");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-015: 判断の前に、読んだ元（note）へ自分で書いた手順でも、開き直して続けられる（自分の書き込みを「止まっている間の変化」と読まない）。
    /// 書いた後に人が変えたら、止まった時の checks の値と違うので 1 手も送らない。
    /// 負の対照: 止まる時の読み直し（`settled`）を外す（走った時の digest と比べる）と、自分の書き込みで毎回 Changed になり赤
    #[test]
    fn wf015_a_write_to_the_read_source_before_the_choice_is_not_a_change() {
        const STAMP_GOAL: &str = "Stamp my note, then pick its title";
        let r = rig("wf015", "Notes", false);
        let mut wf = titled(STAMP_GOAL);
        wf.steps.insert(2, step("stamp", "desk.note.set", json!({ "text": { "$from": "draft.short" } })));
        save_workflow(&r.dir.join(WORKFLOWS_DIR), &wf).unwrap();
        let Err(EngineError::Decide(_)) = r.go("run-15", &[], None, |cx| run(STAMP_GOAL, cx)) else { panic!("判断で止まらない") };
        assert_eq!(r.desk().note, "Notes (short)", "判断の前に note へ書いた");
        r.go("run-15", &[], None, |cx| resume(STAMP_GOAL, "run-15", Some("long"), cx)).expect("自分の書き込みは止まっている間の変化ではない");
        assert_eq!(r.desk().note, "Notes: the long title");
        let Err(EngineError::Decide(_)) = r.go("run-15b", &[], None, |cx| run(STAMP_GOAL, cx)) else { panic!("判断で止まらない") };
        r.desk().note = "edited by hand".into();
        let sets = r.desk().count("desk.note.set");
        let got = r.go("run-15b", &[], None, |cx| resume(STAMP_GOAL, "run-15b", Some("short"), cx));
        assert!(matches!(&got, Err(EngineError::Changed { .. })), "書いた後に人が変えたのに送った: {got:?}");
        assert_eq!(r.desk().count("desk.note.set"), sets, "1 手も送らない");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-012: 承認の card を押すと cursor の続き（承認待ちの手）から走らせ、承認の要らない書く手（下書き）を走らせ直さず、interpret を呼び直さない。
    /// 負の対照: 承認の後に `run` し直す（今までの道）と下書きが 2 回・interpret が 2 回で赤
    #[test]
    fn wf012_an_approval_continues_from_the_cursor_without_asking_the_model_again() {
        let r = rig("wf012", "Notes", false);
        let fake = Fake::new(vec![interp()]);
        let Err(EngineError::Denied(Denial { pending: Some(p), .. })) = r.go("run-12", &[], Some(&fake), |cx| run(SEND_GOAL, cx)) else { panic!("承認で止まらない") };
        assert_eq!((p.action.as_str(), &p.input["text"]), ("desk.note.send", &json!("Notes (short)")));
        assert!(r.desk().sent.is_empty());
        let ok = [approve(&p, "run-12")];
        r.go("run-12", &ok, Some(&fake), |cx| resume(SEND_GOAL, "run-12", None, cx)).unwrap();
        let d = r.desk();
        assert_eq!((d.sent.as_slice(), d.count("desk.draft.make")), (["Notes (short)".to_string()].as_slice(), 1), "承認の要らない書く手は 1 回: {:?}", d.calls);
        drop(d);
        assert_eq!(fake.requests.lock().unwrap().iter().filter(|q| q.name == INTERPRET_NAME).count(), 1, "interpret を呼び直さない");
        assert_eq!(r.row("run-12").run.status, RunStatus::Verified);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-015・WF-012: 承認で止まる前に、読み戻す元（note の本文）へ自分で書いた手順（BG の通しの形）。承認の resume は止まった時の checks の値
    /// （書いた後）と、止まる時に読み直した読む手の digest（`settled`）と比べるので、自分の書き込みを変化と読まずに 1 回だけ送る。
    /// 負の対照: 承認の pause の seen を操作の前の読み（`before`）にすると Changed で赤
    #[test]
    fn wf015_an_approval_after_writing_the_read_source_sends_once() {
        const STAMP_SEND: &str = "Stamp my note, then send the long title";
        let r = rig("wf015b", "Notes", false);
        let mut wf = sending(STAMP_SEND);
        wf.steps.insert(2, step("stamp", "desk.note.set", json!({ "text": { "$from": "draft.short" } })));
        wf.steps[3].input = json!({ "text": { "$from": "draft.long" } });
        // 読み戻しの期待する値は最初の書く手（下書き）を `$from` で指せない（`expected_before_write`）ので、本文に短い題の印が在る事を読む
        wf.checks = vec![ReadBack { action: "desk.note.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Contains { text: " (short)".into() } }];
        save_workflow(&r.dir.join(WORKFLOWS_DIR), &wf).unwrap();
        let Err(EngineError::Denied(Denial { pending: Some(p), .. })) = r.go("run-15c", &[], None, |cx| run(STAMP_SEND, cx)) else { panic!("承認で止まらない") };
        assert_eq!((r.desk().note.as_str(), r.row("run-15c").run.status), ("Notes (short)", RunStatus::WaitingApproval), "承認の前に本文へ書いた");
        let ok = [approve(&p, "run-15c")];
        r.go("run-15c", &ok, None, |cx| resume(STAMP_SEND, "run-15c", None, cx)).expect("自分の書き込みは止まっている間の変化ではない");
        let d = r.desk();
        assert_eq!((d.sent.as_slice(), d.count("desk.note.set")), (["Notes: the long title".to_string()].as_slice(), 1), "{:?}", d.calls);
        drop(d);
        assert_eq!(r.row("run-15c").run.status, RunStatus::Verified);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-012・WF-014: 判断の後ろに承認の要る手 → 選んだ値の exact payload で承認を求め（cursor は判断の後ろ）、承認すると選んだ値を 1 回だけ送る。
    /// 下書きは走り直さない。負の対照: 判断の答えを cursor の outputs に残さない（`kept_outputs` が判断の手を落とす）と、承認の後の resolve が赤
    #[test]
    fn wf012_a_choice_then_an_approval_sends_the_chosen_value_once() {
        const PICK_SEND: &str = "Pick a title and send it";
        let r = rig("wf012b", "Notes", false);
        let mut wf = titled(PICK_SEND);
        wf.steps[3] = step("send", "desk.note.send", json!({ "text": { "$from": "pick.value" } }));
        wf.checks[0].pointer = "/last".into();
        save_workflow(&r.dir.join(WORKFLOWS_DIR), &wf).unwrap();
        let Err(EngineError::Decide(_)) = r.go("run-12b", &[], None, |cx| run(PICK_SEND, cx)) else { panic!("判断で止まらない") };
        let got = r.go("run-12b", &[], None, |cx| resume(PICK_SEND, "run-12b", Some("long"), cx));
        let Err(EngineError::Denied(Denial { pending: Some(p), .. })) = &got else { panic!("承認で止まらない: {got:?}") };
        assert_eq!((p.action.as_str(), &p.input["text"]), ("desk.note.send", &json!("Notes: the long title")), "選んだ値の exact payload");
        let rec = r.row("run-12b");
        assert_eq!((rec.run.status, rec.workflow.as_ref().and_then(|c| c.waiting.as_deref())), (RunStatus::WaitingApproval, Some("send")));
        assert!(r.desk().sent.is_empty());
        let ok = [approve(p, "run-12b")];
        r.go("run-12b", &ok, None, |cx| resume(PICK_SEND, "run-12b", None, cx)).unwrap();
        let d = r.desk();
        assert_eq!((d.sent.as_slice(), d.count("desk.draft.make")), (["Notes: the long title".to_string()].as_slice(), 1), "{:?}", d.calls);
        drop(d);
        assert_eq!(r.row("run-12b").run.status, RunStatus::Verified);
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-003・WF1: 書く手（set）を送った後・結果の前に落ちた → 開き直した後始末で `uncertain`（届いたか分からない）・待っている run に出さない・
    /// 続けても送らない（自動で送り直さない）。偽の app が受けた set は 1 回。
    /// receipt と run の名前は切れた手を指す（INTEG wa2）。
    /// 負の対照: 送る前の保存（write-ahead）を外すと、行は running のまま attempt が無く、後始末が failed（副作用なし）にして赤・
    /// `RunRecord::step` を先頭の手に戻すと receipt が「0.read」で赤
    #[test]
    fn wf003_a_crash_while_writing_reopens_uncertain_and_never_sends_again() {
        let mut r = rig("wf003", "Notes", true);
        let Err(EngineError::Decide(_)) = r.go("run-c", &[], None, |cx| run(GOAL, cx)) else { panic!("判断で止まらない") };
        r.desk().crash = Some("desk.note.set");
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.go("run-c", &[], None, |cx| resume(GOAL, "run-c", Some("short"), cx))));
        assert!(died.is_err(), "落ちた形");
        r.desk().crash = None;
        assert_eq!(r.reopen(), 1, "途中の書く手を 1 つ後始末");
        let rec = r.row("run-c");
        assert_eq!(rec.run.status, RunStatus::Uncertain);
        let set = rec.run.steps.iter().find(|s| s.id == "0.set").unwrap();
        assert!(matches!(set.attempts.last().and_then(|a| a.outcome.as_ref()), Some(Outcome::EffectUnknown { .. })), "{set:?}");
        // INTEG wa2: 「届いたか分からない」の receipt と run の名前は切れた手（set）を指す（先頭の読む手でない）
        let got = rec.receipt.as_ref().expect("後始末の receipt");
        assert_eq!((got.step_id.as_str(), got.action_ref.as_str(), rec.action_id()), ("0.set", "desk.note.set", "desk.note.set"));
        assert!(waiting_runs(&r.store.lock().unwrap()).is_empty(), "届いたか分からない run を判断待ちに出さない");
        let again = r.go("run-c", &[], None, |cx| resume(GOAL, "run-c", Some("short"), cx));
        assert!(matches!(&again, Err(EngineError::Denied(_))), "{again:?}");
        assert_eq!(r.desk().count("desk.note.set"), 1, "自動で送り直さない");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// WF-004: runs 表（sqlite の行）に読む手の出力を残さない（読み直せる手は digest だけ）・secret の key の値は伏せる。再起動の後は元の app から
    /// 読み直して同じ値で続ける。負の対照: 読む手の出力を digest にせず cursor に入れると秘密が行に出て赤
    #[test]
    fn wf004_the_runs_table_keeps_no_plain_secret() {
        let mut r = rig("wf004", "Body", true);
        r.desk().secret = "sekret-wf004-value".into();
        let Err(EngineError::Decide(_)) = r.go("run-s", &[], None, |cx| run(GOAL, cx)) else { panic!("判断で止まらない") };
        let raw = r.store.lock().unwrap().get(Table::Runs, "run-s").unwrap().unwrap().json.to_string();
        assert!(!raw.contains("sekret-wf004-value"), "秘密の平文が runs 表に: {raw}");
        let rec = r.row("run-s");
        let read = &rec.workflow.as_ref().unwrap().outputs["read"];
        assert!(read.get("$digest").is_some() && read.get("text").is_none(), "読む手は digest だけ: {read}");
        r.reopen();
        r.go("run-s", &[], None, |cx| resume(GOAL, "run-s", Some("short"), cx)).unwrap();
        assert_eq!(r.desk().note, "Body (short)", "読み直した同じ値で続く");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    /// INTEG 反証（module WF-D・AC の外）
    mod review_wf {
        use super::*;
        use crate::adapters::sdk::{Adapter, Route, Spec};
        use crate::domain::binding::Source;
        use std::sync::atomic::AtomicUsize;

        /// 偽の app の前に置く関所: この registry の 2 回目の `desk.note.read`（resume の `read_checks` = Running を書く直前の最後の読み）で、もう 1 本の resume も来るまで（上限 30 秒・全 suite の混雑では 3 秒だと 2 本目が間に合わず先に進んで緑になった）待つ。
        /// 2 本とも止まった時と同じ元データを見た後に先へ進ませる（1 回目の読みで待ち合わせると、先の 1 本が set まで済ませ、後の 1 本の `read_checks` が変化を見て Changed で止まる = 同時にならず緑・ed2f97b で実測） = 二度押し・2 つの窓の同時の選びを決定的に作る。
        struct Gate {
            inner: DeskAdapter,
            arrived: Arc<AtomicUsize>,
            reads: usize,
        }
        impl Adapter for Gate {
            fn id(&self) -> &'static str { self.inner.id() }
            fn kind(&self) -> AdapterKind { self.inner.kind() }
            fn route(&self) -> Route { self.inner.route() }
            fn provider(&self) -> &'static str { self.inner.provider() }
            fn specs(&self) -> &'static [Spec] { self.inner.specs() }
            fn readiness(&self, action: &str) -> CapabilityReadiness { self.inner.readiness(action) }
            fn sources(&self) -> Vec<Source> { self.inner.sources() }
            fn check(&mut self) { self.inner.check() }
            fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
                if action == "desk.note.read" {
                    self.reads += 1;
                }
                // 読んだ後で待ち合わせる（読む前に待つと、先に放たれた 1 本が set まで済ませ、後の 1 本の読みが変化を見て Changed で止まる = 98c78fb の緑）
                let got = self.inner.invoke(action, input);
                if action == "desk.note.read" && self.reads == 2 {
                    self.arrived.fetch_add(1, Ordering::SeqCst);
                    let t = Instant::now();
                    while self.arrived.load(Ordering::SeqCst) < 2 && t.elapsed() < Duration::from_secs(30) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                got
            }
            fn cleanup(&mut self) { self.inner.cleanup() }
        }

        /// wa1（WF2 WF-012）: 待っている同じ run に resume が 2 本同時に来る（判断の card の二度押し・2 つの窓）→ 選んだ手（set）は 1 回だけ送る。
        /// 落ちると読んだ理由: `resume` は行を読んで `WaitingHuman` を見てから `Running` を書くまで store の lock を持たない（間に読み直しの invoke を挟む）
        /// → 2 本とも待っている行を見て進み、set を 2 回送る。
        #[test]
        fn wa1_two_resumes_at_once_send_the_chosen_step_once() {
            let r = rig("review-wa1", "Notes", false);
            let Err(EngineError::Decide(_)) = r.go("run-w", &[], None, |cx| run(GOAL, cx)) else { panic!("前提: 判断で止まらない") };
            let arrived = Arc::new(AtomicUsize::new(0));
            let one = |choice: &'static str| {
                let mut reg = AdapterRegistry::default();
                reg.register(Box::new(Gate { inner: DeskAdapter(r.desk.clone()), arrived: arrived.clone(), reads: 0 })).unwrap();
                reg.check();
                let mcp = McpConfig::default();
                let mut an = Vec::new();
                let mut cx = w19_2_body::cx(&mut reg, &r.dir, None, &mcp, &[], &mut an);
                cx.caller = Caller { app_id: APP, run_id: Some("run-w") };
                cx.runs = Some(RunsTable { store: &r.store, session_id: SESSION, space_id: PROJECT, app_version: 1 });
                resume(GOAL, "run-w", Some(choice), &mut cx).map(|_| ())
            };
            let (a, b) = std::thread::scope(|s| {
                let a = s.spawn(|| one("short"));
                let b = s.spawn(|| one("long"));
                (a.join().unwrap(), b.join().unwrap())
            });
            let sets = r.desk().count("desk.note.set");
            eprintln!("wa1: a = {a:?} · b = {b:?} · arrived = {} · calls = {:?}", arrived.load(Ordering::SeqCst), r.desk().calls.iter().map(|(x, _)| x.as_str()).collect::<Vec<_>>());
            assert_eq!(sets, 1, "同じ run の判断が 2 回走った（a = {a:?}・b = {b:?}・note = {}）", r.desk().note);
            let _ = std::fs::remove_dir_all(&r.dir);
        }

        /// wa2（WF1 WF-003 × Runs の 1 行）: set を送った後に落ちた run を開き直した時の receipt は、届いたか分からない手（`0.set`・`desk.note.set`）を指す。
        /// 落ちると読んだ理由: `receipt_for` と `RunRecord::action_id` は steps の先頭（`0.read`・読む手）を見る → Runs の「届いたか分からない」が読む手の名前で出る。
        #[test]
        fn wa2_an_uncertain_write_is_named_by_the_step_that_was_cut() {
            let mut r = rig("review-wa2", "Notes", true);
            let Err(EngineError::Decide(_)) = r.go("run-u", &[], None, |cx| run(GOAL, cx)) else { panic!("前提: 判断で止まらない") };
            r.desk().crash = Some("desk.note.set");
            let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.go("run-u", &[], None, |cx| resume(GOAL, "run-u", Some("short"), cx))));
            assert!(died.is_err(), "前提: 落ちた形");
            r.desk().crash = None;
            assert_eq!(r.reopen(), 1, "前提: 途中の書く手を 1 つ後始末");
            let rec = r.row("run-u");
            assert_eq!(rec.run.status, RunStatus::Uncertain, "前提");
            let got = receipt::receipt_for(&rec, &Outcome::EffectUnknown { reason: "test".into() });
            assert_eq!((got.step_id.as_str(), got.action_ref.as_str(), rec.action_id()), ("0.set", "desk.note.set", "desk.note.set"), "届いたか分からない手でない手の名前で言う");
            let _ = std::fs::remove_dir_all(&r.dir);
        }
    }
}

/// WF-E（Engine の事実と安全）の呼ぶ側の形: 計画の入力の事実（WF-008・009・011）・0 手の作り直し（WF-010）・外へ効く手の宛先の読み直し（WF-006）・
/// 頼んだ差分の読み戻し（WF-007）・AI が失敗した時の AI 無しの道（WF-036）・Automation の 3 択（RQ-023）・承認の card の範囲（RQ-026）。偽の model と偽の Adapter だけ。
#[cfg(test)]
mod wf_e {
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::domain::permission::AutomationMode;
    use crate::llm::{fake::Fake, LlmError};
    use std::sync::{Arc, Mutex};

    const APP_ID: &str = "com.example.Notes";

    /// 宛先の窓の一覧だけを持つ偽の Adapter（`ready` = false で readiness が接続待ち・`payload` = None で答えない）
    struct Win {
        ready: bool,
        payload: Option<Value>,
    }
    static WIN: [Spec; 1] = [Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" }];

    /// 外へ効く手を持つ偽の Adapter（`out.text.post` = ExternalWrite・承認なし）。`checks` = 送る直前に宛先を読み直すと宣言する（`Adapter::prechecks`）
    struct Outbox {
        text: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        checks: bool,
    }
    static OUT: [Spec; 2] = [
        Spec { id: "out.text.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "out.text.post", effect: Effect::ExternalWrite, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
    ];

    macro_rules! plain_adapter {
        ($t:ty, $id:literal, $specs:expr) => {
            fn id(&self) -> &'static str {
                $id
            }
            fn kind(&self) -> AdapterKind {
                AdapterKind::LocalCore
            }
            fn route(&self) -> Route {
                Route::OfficialApi
            }
            fn provider(&self) -> &'static str {
                $id
            }
            fn specs(&self) -> &'static [Spec] {
                &$specs
            }
            fn sources(&self) -> Vec<Source> {
                vec![]
            }
            fn check(&mut self) {}
            fn cleanup(&mut self) {}
        };
    }

    impl Adapter for Win {
        plain_adapter!(Win, "desktop", WIN);
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            if self.ready {
                CapabilityReadiness::Ready
            } else {
                CapabilityReadiness::RequiresConnection
            }
        }
        fn invoke(&mut self, _action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
            match &self.payload {
                Some(v) => (Outcome::Verified { evidence: vec!["desktop".into()] }, Some(sdk::observe("desktop", sdk::now(), v.clone()))),
                None => (Outcome::Failed { error: ErrorCode::Timeout }, None),
            }
        }
    }

    impl Adapter for Outbox {
        plain_adapter!(Outbox, "out", OUT);
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            self.calls.lock().unwrap().push(action.into());
            if action == "out.text.post" {
                *self.text.lock().unwrap() = input["text"].as_str().unwrap_or("").into();
                return (Outcome::Unverified { reason: "送った".into() }, Some(sdk::observe("out", sdk::now(), json!({ "posted": true }))));
            }
            let t = self.text.lock().unwrap().clone();
            (Outcome::Verified { evidence: vec!["out".into()] }, Some(sdk::observe("out", sdk::now(), json!({ "text": t }))))
        }
        fn prechecks(&self, action: &str) -> bool {
            self.checks && action == "out.text.post"
        }
    }

    fn outbox(checks: bool, text: &str) -> (AdapterRegistry, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Outbox { text: Arc::new(Mutex::new(text.into())), calls: calls.clone(), checks })).unwrap();
        reg.check();
        (reg, calls)
    }

    fn step(id: &str, cap: &str, input: Value) -> WorkflowStep {
        WorkflowStep { kind: StepKind::Action, id: id.into(), capability: cap.into(), input }
    }

    fn read(expect: Expect) -> ReadBack {
        ReadBack { action: "out.text.read".into(), input: json!({}), pointer: "/text".into(), expect }
    }

    /// 保存した手順（model 無しの run と、AI 無しに戻った run が再利用する）
    fn saved(dir: &Path, goal: &str, steps: Vec<WorkflowStep>, checks: Vec<ReadBack>) {
        let op = Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() };
        save_workflow(&dir.join(WORKFLOWS_DIR), &Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: workflow_id(goal, &op), goal: goal.into(), steps, ui: None, checks }).unwrap();
    }

    fn go(reg: &mut AdapterRegistry, dir: &Path, llm: Option<&dyn Llm>, automation: AutomationMode, goal: &str) -> Result<EngineRecord, EngineError> {
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let mut cx = w19_2_body::cx(reg, dir, llm, &mcp, &[], &mut an);
        cx.automation = automation;
        run(goal, &mut cx)
    }

    fn interp(target: Option<&str>) -> Result<Value, LlmError> {
        Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Rename the note", "success": "The note has the new name", "target": target }] }))
    }

    fn none(why: &str) -> Result<Value, LlmError> {
        Ok(json!({ "none_why": why, "workflow_steps": [], "checks": [], "recipe": null, "trial": null }))
    }

    /// 窓の一覧を読むだけの計画（窓の一覧が `[]` の時に読み戻しが合う）
    fn list_plan() -> Result<Value, LlmError> {
        let list = json!({ "id": "r", "capability": "desktop.windows.list", "input_json": "{}" });
        let check = json!({ "action": "desktop.windows.list", "input_json": "{}", "pointer": "", "expect": "equals", "value_json": "[]" });
        Ok(json!({ "none_why": null, "workflow_steps": [list], "checks": [check], "recipe": null, "trial": null }))
    }

    /// 偽の model が受けた k 番目の依頼の input（JSON）
    fn input(fake: &Fake, k: usize) -> Value {
        serde_json::from_str(&fake.requests.lock().unwrap()[k].input).unwrap()
    }

    /// WF-008・WF-011: 窓の取得の状態（宛先なし・接続待ち・答えない・形が違う・0 件・取れた）が計画の input の `windows_fact` に事実として入る。
    /// WF-009: registry の action に readiness・確かめ方・scope が入る。負の対照: `windows_fact` を入れないと 6 つとも赤・readiness を外すと赤
    #[test]
    fn wf008_the_plan_is_told_why_there_are_no_windows_and_what_each_action_needs() {
        let w = |bundle: &str| json!({ "pid": 41, "window_id": 7001, "app_name": "Notes", "title": "a", "bundle_id": bundle });
        let cases: [(Option<&str>, bool, Option<Value>, Value); 6] = [
            (None, true, Some(json!([])), json!({ "status": "no_target" })),
            (Some(APP_ID), false, Some(json!([])), json!({ "status": "not_ready", "why": "requires_connection" })),
            (Some(APP_ID), true, None, json!("read_failed")),
            (Some(APP_ID), true, Some(json!({ "windows": [] })), json!("decode_failed")),
            (Some(APP_ID), true, Some(json!([w("com.example.Other")])), json!({ "status": "empty" })),
            (Some(APP_ID), true, Some(json!([w(APP_ID)])), json!({ "status": "ready", "n": 1 })),
        ];
        for (target, ready, payload, want) in cases {
            let d = w19_2_body::tmp("wf008");
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(Win { ready, payload })).unwrap();
            reg.check();
            let fake = Fake::new(vec![interp(target), none("no window"), none("still no window")]);
            let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Rename the note");
            assert!(matches!(r, Err(EngineError::NoMeans { .. })), "{r:?}");
            let i = input(&fake, 1);
            match want.as_str() {
                Some(status) => assert_eq!(i["windows_fact"]["status"], json!(status), "{i}"),
                None => assert_eq!(i["windows_fact"], want, "{target:?} {ready}"),
            }
            let a = &i["registry_actions"][0];
            assert_eq!((&a["id"], &a["verifier"], &a["required_scopes"]), (&json!("desktop.windows.list"), &json!("read_back"), &json!([])), "{a}");
            assert!(a["readiness"].is_string(), "readiness が無い: {a}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// WF-010: model が 0 手と答えたら、理由を input の `previous_none` に入れて計画だけを 1 回作り直す（何も送っていない）。2 回目も 0 手なら NoMeans で、3 回目は聞かない。
    /// 負の対照: 作り直しを外すと 1 本目が NoMeans で赤・回数の上限を外すと 2 本目の依頼の数が赤
    #[test]
    fn wf010_a_plan_with_no_steps_is_asked_again_once_with_its_reason() {
        let d = w19_2_body::tmp("wf010");
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Win { ready: true, payload: Some(json!([])) })).unwrap();
        reg.check();
        let fake = Fake::new(vec![interp(Some(APP_ID)), none("The windows list was empty"), list_plan()]);
        let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Rename the note");
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(fake.requests.lock().unwrap().len(), 3);
        assert_eq!((input(&fake, 1).get("previous_none"), &input(&fake, 2)["previous_none"]), (None, &json!("The windows list was empty")));

        let fake = Fake::new(vec![interp(Some(APP_ID)), none("a"), none("b"), list_plan()]);
        let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Rename the note again");
        assert!(matches!(r, Err(EngineError::NoMeans { .. })), "{r:?}");
        assert_eq!(fake.requests.lock().unwrap().len(), 3, "作り直しは 1 回だけ");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// WF-006: 外へ効く手は、Adapter が送る直前に宛先を読み直すと宣言していなければ、adapter を問わず Engine が 1 手も送らない（理由の 1 行）。
    /// 宣言していれば今まで通り送る。負の対照: `run_steps` の宣言の検査を外すと 1 本目で post が送られて赤
    #[test]
    fn wf006_an_outside_step_without_a_recheck_before_sending_is_not_sent() {
        let goal = "Post hi";
        for (checks, sent) in [(false, false), (true, true)] {
            let d = w19_2_body::tmp("wf006");
            saved(&d, goal, vec![step("post", "out.text.post", json!({ "text": "hi" }))], vec![read(Expect::Equals { value: json!("hi") })]);
            let (mut reg, calls) = outbox(checks, "");
            let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
            let posted = calls.lock().unwrap().iter().any(|c| c == "out.text.post");
            assert_eq!(posted, sent, "{r:?} {:?}", calls.lock().unwrap());
            if !sent {
                let Err(e @ EngineError::Denied(_)) = &r else { panic!("{r:?}") };
                let line = crate::engine_page::failure_line(e);
                assert!(line.contains("nothing was sent") && line.contains("“post”"), "{line}");
            } else {
                assert!(r.is_ok(), "{r:?}");
            }
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// WF-007: 字を置く書く手の済みは「頼んだ差分」を読み戻した時だけ。含むだけの読み戻しは Partial（run は済みと出さない）・置いた字と等しい読み戻しは済み。
    /// `Replaced` は前の値の 1 か所を替えた物と今の値を比べる（他の行が消えたら赤）。負の対照: Partial の規則を外すと 1 本目の partial が None で赤
    #[test]
    fn wf007_a_write_is_done_only_when_the_asked_change_is_read_back() {
        let goal = "Post hi there";
        for (expect, partial) in [(Expect::Contains { text: "hi".into() }, true), (Expect::Equals { value: json!("hi there") }, false)] {
            let d = w19_2_body::tmp("wf007");
            saved(&d, goal, vec![step("post", "out.text.post", json!({ "text": "hi there" }))], vec![read(expect)]);
            let (mut reg, _) = outbox(true, "");
            let r = go(&mut reg, &d, None, AutomationMode::default(), goal).unwrap();
            assert_eq!(r.partial.is_some(), partial, "{:?}", r.partial);
            let _ = std::fs::remove_dir_all(&d);
        }
        let o = |t: &str| Observation { id: "1".into(), source_ref: "t".into(), observed_at: "2026-10-28T00:00:00Z".into(), payload: json!({ "text": t }) };
        let rb = |from: &str, to: &str| ReadBack { action: "out.text.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Replaced { from: from.into(), to: to.into() } };
        let doc = "# old\n\nbody\n";
        assert!(verify(&rb("# old", "# new"), Some(&o(doc)), &o("# new\n\nbody")).is_ok(), "1 か所だけ替わった（末尾の改行は比べない）");
        assert!(verify(&rb("# old", "# new"), Some(&o(doc)), &o("# new\r\n\r\nbody\u{200b}\n")).is_ok(), "改行の形と幅 0 の字は比べない");
        assert!(verify(&rb("# old", "# new"), Some(&o(doc)), &o("# new\n")).is_err(), "他の行が消えた");
        assert!(verify(&rb("# old", "# new"), Some(&o(doc)), &o(doc)).is_err(), "替わっていない");
        assert!(verify(&rb("x", "y"), Some(&o("x x")), &o("y x")).is_err(), "前の値に 2 か所 = どれを頼んだか決まらない");
        assert!(verify(&rb("", "all new"), Some(&o(doc)), &o("all new\n")).is_ok(), "from が空 = 全文");
        assert!(verify(&rb("# old", "# new"), None, &o("# new\n\nbody")).is_err(), "前の値が無い");
        let saved = saved_checks(&[step("set", "desktop.ax.type_text", json!({ "document": "README.md", "replace": "# old", "text": "# new" }))], "README.md", "file:///x/README.md");
        assert_eq!(saved[0].expect, Expect::Replaced { from: "# old".into(), to: "# new".into() }, "保存した file の読み戻しも頼んだ差分");
    }

    /// WF-036: model の呼び出しが失敗したら、AI 無しの道（目的をそのまま 1 つの操作・保存した手順・確かめ済みの能力）に戻り、その事を run と画面に 1 行。
    /// 戻れない時は AI が使えなかった事を言う（「言い直して」の嘘の 1 行にしない）。負の対照: 戻りを外すと 1 本目が Model で赤
    #[test]
    fn wf036_when_the_ai_fails_the_run_goes_on_without_it_and_says_so() {
        let goal = "Read the note";
        let d = w19_2_body::tmp("wf036");
        saved(&d, goal, vec![step("r", "out.text.read", json!({}))], vec![read(Expect::Equals { value: json!("hi") })]);
        let (mut reg, _) = outbox(true, "hi");
        let fake = Fake::new(vec![Err(LlmError::Timeout { ms: 60_000 })]);
        let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), goal).unwrap();
        assert!(r.without_ai.is_some(), "{r:?}");
        assert_eq!(fake.requests.lock().unwrap().len(), 1, "失敗した後は AI を呼ばない");

        // 確かめ済みの能力が無い registry（前の run の読みで Verified になった物を使わない）
        let (mut reg, _) = outbox(true, "hi");
        let fake = Fake::new(vec![interp(None), Err(LlmError::Timeout { ms: 60_000 })]);
        let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Rename the note");
        let Err(e @ EngineError::NoMeans { tried, .. }) = &r else { panic!("{r:?}") };
        assert!(tried.iter().any(|t| t.starts_with(AI_NOT_USED)), "{tried:?}");
        let line = crate::engine_page::failure_line(e);
        assert!(line.contains("AI") && !line.contains("Rephrase"), "{line}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// WF-036（WF-E x6b）: model が初めから無い run も、前の AI の run が 2 つの操作に分けて保存した目的を 1 つ目だけ走らせて済みにしない
    /// （1 手も送らない・理由と次の一手の 1 行）。負の対照: 門を AI が失敗した時だけに戻すと op1 の post が送られて赤
    #[test]
    fn wf036_with_no_model_a_goal_saved_as_two_actions_sends_nothing() {
        let goal = "Post hi and bye";
        let d = w19_2_body::tmp("wf036b");
        saved(&d, goal, vec![step("post", "out.text.post", json!({ "text": "hi" }))], vec![read(Expect::Equals { value: json!("hi") })]);
        let op2 = Operation { id: "op2".into(), want: String::new(), success: String::new(), target: None, ..Default::default() };
        let steps = vec![step("post", "out.text.post", json!({ "text": "bye" }))];
        let wf = Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: workflow_id(goal, &op2), goal: goal.into(), steps, ui: None, checks: vec![read(Expect::Equals { value: json!("bye") })] };
        save_workflow(&d.join(WORKFLOWS_DIR), &wf).unwrap();
        let (mut reg, calls) = outbox(true, "");
        let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
        assert!(!calls.lock().unwrap().iter().any(|c| c == "out.text.post"), "{r:?} {:?}", calls.lock().unwrap());
        let Err(e) = &r else { panic!("{r:?}") };
        assert_eq!(crate::engine_page::failure_line(e), "This goal has 2 actions and needs an AI tool · Add one in Settings › AI & Models");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// WF-036（WF-E x6c）: 前の AI の run が 2 つに分けて op1 だけ保存した目的（`Workflow::split` が分けた一覧を覚えている）も、model 無しの run は 1 手も送らない
    /// （x6b と同じ 1 行・N = 分けた数）。一覧の無い前の版の保存は今まで通り走る。負の対照: 門が split を数えるのを外すと 1 本目の post が送られて赤
    #[test]
    fn wf036_with_no_model_a_goal_saved_only_up_to_its_first_action_sends_nothing() {
        let goal = "Post hi and bye";
        for (split, sent) in [(vec!["op1".to_string(), "op2".to_string()], false), (Vec::new(), true)] {
            let d = w19_2_body::tmp("wf036c");
            let op1 = Operation { id: "op1".into(), want: String::new(), success: String::new(), target: None, ..Default::default() };
            let steps = vec![step("post", "out.text.post", json!({ "text": "hi" }))];
            let wf = Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split, id: workflow_id(goal, &op1), goal: goal.into(), steps, ui: None, checks: vec![read(Expect::Equals { value: json!("hi") })] };
            save_workflow(&d.join(WORKFLOWS_DIR), &wf).unwrap();
            let (mut reg, calls) = outbox(true, "");
            let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
            assert_eq!(calls.lock().unwrap().iter().any(|c| c == "out.text.post"), sent, "{r:?}");
            if !sent {
                let Err(e) = &r else { panic!("{r:?}") };
                assert_eq!(crate::engine_page::failure_line(e), "This goal has 2 actions and needs an AI tool · Add one in Settings › AI & Models");
            }
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// RQ-023: Suggest は計画まで（1 手も送らない）・Trusted read-only は書く手を含む計画を送らない（読むだけの計画は走る）・Ask before write は今まで通り。
    /// 負の対照: 門を外すと Suggest と Trusted read-only の post が送られて赤
    #[test]
    fn rq023_automation_suggest_and_read_only_send_nothing_that_writes() {
        let goal = "Post hi";
        let d = w19_2_body::tmp("rq023");
        saved(&d, goal, vec![step("post", "out.text.post", json!({ "text": "hi" }))], vec![read(Expect::Equals { value: json!("hi") })]);
        saved(&d, "Read the note", vec![step("r", "out.text.read", json!({}))], vec![read(Expect::Equals { value: json!("") })]);
        for (mode, sent) in [(AutomationMode::Suggest, false), (AutomationMode::TrustedReadOnly, false), (AutomationMode::AskBeforeWrite, true)] {
            let (mut reg, calls) = outbox(true, "");
            let r = go(&mut reg, &d, None, mode, goal);
            assert_eq!(calls.lock().unwrap().iter().any(|c| c == "out.text.post"), sent, "{mode:?} {r:?}");
            assert_eq!(matches!(r, Err(EngineError::Denied(_))), !sent, "{mode:?} {r:?}");
        }
        let (mut reg, _) = outbox(true, "");
        assert!(go(&mut reg, &d, None, AutomationMode::TrustedReadOnly, "Read the note").is_ok(), "読むだけは走る");
        let (mut reg, calls) = outbox(true, "");
        assert!(go(&mut reg, &d, None, AutomationMode::Suggest, "Read the note").is_err(), "Suggest は読む手も走らせない");
        assert_eq!(calls.lock().unwrap().iter().filter(|c| *c == "out.text.read").count(), 1, "操作の前の読み 1 回だけ（手の read は走らない）");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// RQ-026: 承認の card の 2 行目 = 範囲（何か所・全文・押す数）。AX 名と 1 行目は変えない。負の対照: 全文と 1 か所を取り違えると赤
    #[test]
    fn rq026_the_approval_card_says_how_much_will_change() {
        use crate::ui::engine_view::{scope_line, ApprovalStep};
        let s = |action: &str, input: Value| ApprovalStep { action: action.into(), input, digest: String::new(), scopes: vec![] };
        let doc = json!({ "app_name": "TextEdit", "document": "README.md", "replace": "# old", "text": "# new" });
        assert_eq!(scope_line(&[s("desktop.ax.type_text", doc.clone())]).as_deref(), Some("Changes 1 place in TextEdit's “README.md”; the rest stays the same"));
        let all = json!({ "app_name": "TextEdit", "document": "README.md", "text": "Shibubu Demo" });
        let save = json!({ "app_name": "TextEdit", "document": "README.md", "path": ["File", "Save"] });
        assert_eq!(scope_line(&[s("desktop.ax.type_text", all), s("desktop.ax.menu", save.clone())]).as_deref(), Some("Replaces all the text in TextEdit's “README.md” · then presses 1 thing"));
        assert_eq!(scope_line(&[s("desktop.ax.menu", save)]).as_deref(), Some("Presses 1 thing in TextEdit; nothing is typed"));
        assert_eq!(scope_line(&[s("desk.note.send", json!({ "text": "x" }))]), None, "画面の手でない手は 2 行目を出さない");
    }
}

/// ENGINE-ADJUST-1102 ①（owner 256「止めるのは人の Stop だけ」・operator「Engine の run に人の Stop が無い」）: 人の Stop の印（`EngineContext::stop`）を Engine が守る、
/// の呼ぶ側の形（interface の commit で書いた・本体の前は 3 本とも赤 = 印を見ずに走り切る）。負の対照（本体の後・手で）: 段の前の見張りを外す → 1 本目が NoMeans で赤・
/// interpret の後の見張りを外す → 2 本目で generate の AI が呼ばれて赤・`run_steps` の手の前の見張りを外す → 3 本目で 2 手目が走って赤。
#[cfg(test)]
mod engine_adjust_stop_interface {
    use super::w19_2_body::{cx, tmp, CALLER};
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::domain::permission::ConsentDecision;
    use crate::llm::LlmError;
    use std::sync::{Arc, Mutex};

    /// 試験の偽の Adapter: 読む手 `trip.text.read`（承認なし）と書く手 `trip.text.write`（承認あり）。`arm` = 書く手が走った時に人の Stop の印を立てる（手の間の Stop）。
    struct Trip {
        calls: Arc<Mutex<Vec<String>>>,
        arm: Option<Arc<AtomicBool>>,
    }
    static TRIP: [Spec; 2] = [
        Spec { id: "trip.text.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "trip.text.write", effect: Effect::LocalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
    ];
    impl Adapter for Trip {
        fn id(&self) -> &'static str {
            "trip"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::LocalCore
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            "trip"
        }
        fn specs(&self) -> &'static [Spec] {
            &TRIP
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
            self.calls.lock().unwrap().push(action.into());
            if action.ends_with(".write") {
                if let Some(f) = &self.arm {
                    f.store(true, Ordering::SeqCst);
                }
                return (Outcome::Unverified { reason: "wrote".into() }, Some(sdk::observe("trip", sdk::now(), json!({ "written": true }))));
            }
            let n = self.calls.lock().unwrap().len();
            (Outcome::Verified { evidence: vec!["trip".into()] }, Some(sdk::observe("trip", sdk::now(), json!({ "text": n }))))
        }
        fn cleanup(&mut self) {}
    }

    pub(super) fn trip(arm: Option<Arc<AtomicBool>>) -> (AdapterRegistry, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Trip { calls: calls.clone(), arm })).unwrap();
        reg.check();
        (reg, calls)
    }

    /// AI が答える間に人が Stop を押した形: 1 回目（interpret）で印を立てて答えを返す。それ以外の呼び出し（generate）は Stop の後 = 呼ばれたら Err。
    struct Tripwire {
        flag: Arc<AtomicBool>,
        calls: Mutex<Vec<String>>,
    }
    impl Llm for Tripwire {
        fn label(&self) -> String {
            "tripwire".into()
        }
        fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
            self.calls.lock().unwrap().push(req.name.clone());
            if req.name != INTERPRET_NAME {
                return Err(LlmError::Malformed { detail: "Stop の後に AI を呼んだ".into() });
            }
            self.flag.store(true, Ordering::SeqCst);
            Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Open the guitar list", "success": "the list is shown", "target": null }] }))
        }
    }

    /// 印が立ってから頼まれた run は、AI も Adapter も 1 回も呼ばずに `STOPPED_LINE`（承認待ちの手は無い）で閉じる。
    #[test]
    fn stop_already_set_sends_nothing_and_calls_nothing() {
        let d = tmp("stop-before");
        let (mut reg, calls) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let stop = AtomicBool::new(true);
        let r = run("Open the guitar list", &mut EngineContext { stop: &stop, ..cx(&mut reg, &d, None, &mcp, &[], &mut an) });
        assert!(matches!(&r, Err(EngineError::Denied(dn)) if dn.why == STOPPED_LINE && dn.pending.is_none()), "{r:?}");
        assert!(calls.lock().unwrap().is_empty(), "Stop の後に Adapter を呼んだ: {:?}", calls.lock().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// AI が答えを返す間に Stop が押された（interpret の中で印が立つ）→ 次の段に進まず、generate の AI も Adapter も呼ばない。
    #[test]
    fn stop_set_while_the_ai_answers_stops_before_the_next_stage() {
        let d = tmp("stop-interpret");
        let (mut reg, calls) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let stop = Arc::new(AtomicBool::new(false));
        let ai = Tripwire { flag: stop.clone(), calls: Mutex::new(Vec::new()) };
        let r = run("Open the guitar list", &mut EngineContext { stop: &stop, ..cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an) });
        assert!(matches!(&r, Err(EngineError::Denied(dn)) if dn.why == STOPPED_LINE), "{r:?}");
        assert_eq!(*ai.calls.lock().unwrap(), [INTERPRET_NAME], "Stop の後に AI を呼んだ");
        assert!(calls.lock().unwrap().is_empty(), "Stop の後に Adapter を呼んだ: {:?}", calls.lock().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 手の間の Stop: 1 手目（書く手）が走った後に印が立つ → 2 手目は送らず `Stopped { step: "s2", why: STOPPED_WHY, ran: 1 }`（走った手の数を言う・読み戻しもしない）。
    /// 保存した手順の再利用（AI 無し・承認は exact digest）で、手の間の見張りだけを通す。
    #[test]
    fn stop_between_steps_sends_no_later_step_and_counts_the_ran_ones() {
        let d = tmp("stop-between");
        let stop = Arc::new(AtomicBool::new(false));
        let (mut reg, calls) = trip(Some(stop.clone()));
        let goal = "Write a twice";
        let op = Operation { id: "op1".into(), want: goal.into(), success: String::new(), target: None, ..Default::default() };
        let step = |id: &str, text: &str| WorkflowStep { kind: StepKind::Action, id: id.into(), capability: "trip.text.write".into(), input: json!({ "text": text }) };
        let wf = Workflow { proven: None, fails: 0,
            schema: WORKFLOW_SCHEMA.into(),
            split: Vec::new(),
            id: workflow_id(goal, &op),
            goal: goal.into(),
            ui: None,
            steps: vec![step("s1", "a"), step("s2", "b")],
            checks: vec![ReadBack { action: "trip.text.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Changed }],
        };
        std::fs::create_dir_all(d.join(WORKFLOWS_DIR)).unwrap();
        save_workflow(&d.join(WORKFLOWS_DIR), &wf).unwrap();
        let ok = |text: &str| ConsentReceipt {
            id: format!("c-{}", crate::model::new_id()),
            app_id: CALLER.app_id.into(),
            subject: ConsentSubject::Action { action_ref: "trip.text.write".into(), run_id: None },
            scopes: vec![],
            payload_digest: Some(sdk::payload_digest("trip.text.write", &json!({ "text": text }))),
            decision: ConsentDecision::Approved,
            decided_at: "2026-11-02T00:00:00Z".into(),
        };
        let consents = [ok("a"), ok("b")];
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let seen = Mutex::new(Vec::new());
        let push = |s: &StageLog| seen.lock().unwrap().push(s.clone());
        let r = run(goal, &mut EngineContext { stop: &stop, staged: Some(&push), ..cx(&mut reg, &d, None, &mcp, &consents, &mut an) });
        assert!(matches!(&r, Err(EngineError::Stopped { step, why, ran: 1 }) if step == "s2" && why == STOPPED_WHY), "{r:?}");
        assert_eq!(*calls.lock().unwrap(), ["trip.text.read", "trip.text.write"], "手の前の読みと 1 手目だけ（2 手目と読み戻しは無し）");
        // r10 ③: 止まった Execution も段に残る（走った手の数つき）。負の対照: run_stages の `execution_stopped` の log を外す → 赤
        let got = seen.lock().unwrap();
        assert!(matches!(got.last(), Some(StageLog { stage: Stage::Execution, outcome: StageOutcome::Failed { .. }, counts: Some(c), .. }) if c["steps"] == 1), "{got:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// ENGINE-AUDIT W1（節 16「記録に段の時間と AI が見た物が無い」・254）: 段の記録の呼ぶ側の形（interface の commit で書いた・counts は本体の前は None = 赤）。
#[cfg(test)]
mod engine_audit_w1_interface {
    use super::engine_adjust_stop_interface::trip;
    use super::w19_2_body::{cx, tmp};
    use super::*;
    use crate::llm::fake::Fake;
    use std::sync::Mutex;

    /// 段が終わるたびに `EngineContext::staged` が呼ばれ、止まった run（手段が無い = NoMeans）でも段が順に・ms つきで残る。GoalInterpretation の counts に AI の回数。
    /// 負の対照（手で）: `run_stages` の `log` から `staged` の呼びを外す → 0 件で赤・GoalInterpretation の counts を None に戻す → 赤。
    #[test]
    fn w1_stages_are_handed_out_in_order_even_when_the_run_stops() {
        let d = tmp("w1-stages");
        let (mut reg, _) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let seen = Mutex::new(Vec::new());
        let push = |s: &StageLog| seen.lock().unwrap().push(s.clone());
        let said = Mutex::new(Vec::new());
        let say = |l: Option<String>| said.lock().unwrap().push(l);
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Open the guitar list", "success": "the list is shown", "target": null }] });
        let none = json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(ops), Ok(none.clone()), Ok(none)]);
        let r = run("Open the guitar list", &mut EngineContext { staged: Some(&push), progress: Some(&say), ..cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an) });
        assert!(matches!(r, Err(EngineError::NoMeans { .. })), "{r:?}");
        let got = seen.into_inner().unwrap();
        let order: Vec<Stage> = got.iter().map(|s| s.stage).collect();
        assert_eq!(order, [Stage::GoalInterpretation, Stage::CapabilityDiscovery, Stage::DeepAnalysis, Stage::IntegrationPlanning, Stage::DynamicGeneration], "止まった run でも段が順に残る: {got:?}");
        assert_eq!(got[0].counts.as_ref().and_then(|c| c["ai"].as_u64()), Some(1), "GoalInterpretation の AI の回数: {:?}", got[0]);
        // r9 (2)(3)（operator 11-03: 止まった run の記録が integration_planning で終わっていた）: 止まった生成の段も数字と時間の内訳と作り直しの理由を持つ。
        // 負の対照（手で）: `generate` の Err の時の `cx.staged` の 1 行を外す → 段が 4 つで上の assert が赤
        let gen = got[4].counts.clone().unwrap_or_default();
        assert!(matches!(&got[4].outcome, StageOutcome::Failed { why } if why == "0 steps"), "{:?}", got[4]);
        assert_eq!((gen["ai"].as_u64(), gen["retries"].as_u64(), gen["ms_ai_each"].as_array().map(Vec::len)), (Some(2), Some(1), Some(2)), "AI の回数と 1 回ごとの時間: {gen}");
        assert!(["ms_open", "ms_read", "ms_menu", "ms_input", "ms_ai", "chars_input"].iter().all(|k| gen[k].is_u64()), "時間の内訳・字数が無い: {gen}");
        assert_eq!(gen["retry_why"], json!(["No reason was given"]), "作り直しの理由: {gen}");
        // W1 本体: AI を呼ぶ段と探す段の 1 行が Page に出る（Running の card の下）
        let lines: Vec<String> = said.into_inner().unwrap().into_iter().flatten().collect();
        assert!(lines.iter().any(|l| l.starts_with("Asking the AI what you meant")) && lines.iter().any(|l| l.starts_with("Looking for ways")), "段の 1 行: {lines:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// ENGINE-AUDIT W9（節 14・interface）: interpret が操作ごとの種類・名指し・候補を返す（呼ぶ側の形）。
#[cfg(test)]
mod engine_audit_w9_interface {
    use super::*;
    use crate::llm::fake::Fake;

    /// AI の答えの kind / named / candidates が `Operation` に乗る。前の形（kind 等の無い保存した記録）は serde default で読める（kind = act・named = false・candidates 空）。
    #[test]
    fn w9_interpret_carries_kind_named_and_candidates() {
        let ops = json!({ "operations": [
            { "id": "op1", "want": "Make it a bit louder", "success": "louder", "target": null, "kind": "act", "named": false, "candidates": ["com.example.daw", "com.example.player"] },
            { "id": "op2", "want": "What did you do last time?", "success": "", "target": null, "kind": "question", "named": false, "candidates": [] },
        ] });
        let fake = Fake::new(vec![Ok(ops)]);
        let got = interpret("Make it a bit louder and tell me what you did last time", &NO_SITUATION, Some(&fake), &NO_STOP).unwrap();
        assert_eq!((got[0].kind, got[0].named, got[0].candidates.as_slice()), (OperationKind::Act, false, &["com.example.daw".to_string(), "com.example.player".to_string()][..]));
        assert_eq!((got[1].kind, got[1].candidates.len()), (OperationKind::Question, 0));
        let old: Operation = serde_json::from_value(json!({ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "w", "success": "s", "target": null })).unwrap();
        assert_eq!((old.kind, old.named, old.candidates.len()), (OperationKind::Act, false, 0), "前の記録はそのまま読める");
        let v = serde_json::to_value(&old).unwrap();
        assert!(v.get("candidates").is_none() && v["kind"] == "act", "空の候補は書かない・種類は snake_case: {v}");
    }
}

/// ENGINE-AUDIT W2（節 2「確かめていない計画を覚え、失敗しても捨てない」・owner 211 §2）: 確かめた手順だけ覚える、の呼ぶ側の形（interface の commit・本体の前は 2〜4 が赤）。
#[cfg(test)]
mod engine_audit_w2_interface {
    use super::engine_adjust_stop_interface::trip;
    use super::w19_2_body::{cx, tmp, CALLER};
    use super::*;
    use crate::domain::permission::ConsentDecision;
    use crate::llm::fake::Fake;

    /// 保存した手順（書く手 1 つ ＋ 読み戻し Changed・AI 無しで再利用できる形）を置く。`fails` と `proven` は呼ぶ側が決める。
    fn saved(d: &Path, goal: &str, proven: Option<&str>, fails: u32) -> Workflow {
        let op = Operation { id: "op1".into(), want: goal.into(), success: String::new(), target: None, ..Default::default() };
        let wf = Workflow {
            proven: proven.map(str::to_owned),
            fails,
            schema: WORKFLOW_SCHEMA.into(),
            split: Vec::new(),
            id: workflow_id(goal, &op),
            goal: goal.into(),
            ui: None,
            steps: vec![WorkflowStep { kind: StepKind::Action, id: "s1".into(), capability: "trip.text.write".into(), input: json!({ "text": "a" }) }],
            checks: vec![ReadBack { action: "trip.text.read".into(), input: json!({}), pointer: "/text".into(), expect: Expect::Changed }],
        };
        std::fs::create_dir_all(d.join(WORKFLOWS_DIR)).unwrap();
        save_workflow(&d.join(WORKFLOWS_DIR), &wf).unwrap();
        wf
    }

    fn ok(text: &str) -> ConsentReceipt {
        ConsentReceipt {
            id: format!("c-{}", crate::model::new_id()),
            app_id: CALLER.app_id.into(),
            subject: ConsentSubject::Action { action_ref: "trip.text.write".into(), run_id: None },
            scopes: vec![],
            payload_digest: Some(sdk::payload_digest("trip.text.write", &json!({ "text": text }))),
            decision: ConsentDecision::Approved,
            decided_at: "2026-11-02T00:00:00Z".into(),
        }
    }

    /// 1. 保存の形: 前の版の file（proven・fails 無し）はそのまま読め（None・0）、proven と fails は往復し、0 と None は書かない。
    #[test]
    fn w2_workflow_keeps_proven_and_fails_and_reads_old_files() {
        let d = tmp("w2-shape");
        let wf = saved(&d, "Write a", Some("2026-11-02T00:00:00Z · 1 value read back"), 2);
        let got = load_workflows(&d.join(WORKFLOWS_DIR)).unwrap();
        assert_eq!(got, vec![wf.clone()]);
        let v = serde_json::to_value(&Workflow { proven: None, fails: 0, ..wf }).unwrap();
        assert!(v.get("proven").is_none() && v.get("fails").is_none(), "下書きは proven も fails も書かない: {v}");
        let old: Workflow = serde_json::from_value(json!({ "schema": WORKFLOW_SCHEMA, "id": "w", "goal": "g", "steps": [{ "id": "s1", "capability": "x", "input": {} }] })).unwrap();
        assert_eq!((old.proven, old.fails), (None, 0), "前の版の file");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 2. 本物で失敗した手順（fails ≥ 1）は再利用せず、失敗の 1 行を hint に AI で作り直す（AI が手順 0 なら NoMeans・Adapter は 1 つも呼ばない）。
    /// 負の対照（本体の後・手で）: `usable` の fails の条件を外す → 再利用されて Adapter が呼ばれ、赤。
    #[test]
    fn w2_a_failed_procedure_is_rebuilt_not_reused() {
        let d = tmp("w2-fails");
        let goal = "Write a";
        saved(&d, goal, None, 1);
        let (mut reg, calls) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "written", "target": null }] });
        let none = json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(ops), Ok(none.clone()), Ok(none)]);
        let consents = [ok("a")];
        let r = run(goal, &mut EngineContext { ..cx(&mut reg, &d, Some(&ai), &mcp, &consents, &mut an) });
        assert!(matches!(r, Err(EngineError::NoMeans { .. })), "{r:?}");
        let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert!(names.iter().any(|n| n == GENERATE_NAME), "失敗した手順を作り直さずに使った: {names:?}");
        assert!(ai.requests.lock().unwrap().iter().any(|r| r.name == GENERATE_NAME && r.input.contains("rejected_plan")), "前の失敗の 1 行を AI に渡していない");
        assert!(calls.lock().unwrap().is_empty(), "失敗した手順の手を送った: {:?}", calls.lock().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 3. 読み戻しが通った手順は proven（いつ・何を確かめたか）を持って保存し直される。
    /// 負の対照（手で）: 本体の proven の書き込みを外す → 赤。
    #[test]
    fn w2_a_procedure_that_read_back_is_saved_as_proven() {
        let d = tmp("w2-proven");
        let goal = "Write a";
        saved(&d, goal, None, 0);
        let (mut reg, _) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let consents = [ok("a")];
        let r = run(goal, &mut EngineContext { ..cx(&mut reg, &d, None, &mcp, &consents, &mut an) });
        assert!(r.is_ok(), "{r:?}");
        let got = load_workflows(&d.join(WORKFLOWS_DIR)).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].proven.as_deref().is_some_and(|p| !p.trim().is_empty()) && got[0].fails == 0, "通った手順に proven が無い: {:?}", got[0]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 4. proven の手順が在る一言は interpret を飛ばす（AI 0 回・2 回目は速く確か）。AI が在っても呼ばない。
    /// 負の対照（手で）: `run_stages` の proven の近道を外す → interpret が呼ばれ（答えが無いので Malformed → AI 無しの道）、requests が 1 件で赤。
    #[test]
    fn w2_a_proven_procedure_skips_interpret() {
        let d = tmp("w2-skip");
        let goal = "Write a";
        saved(&d, goal, Some("2026-11-02T00:00:00Z · 1 value read back"), 0);
        let (mut reg, _) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let ai = Fake::new(vec![]);
        let consents = [ok("a")];
        let r = run(goal, &mut EngineContext { ..cx(&mut reg, &d, Some(&ai), &mcp, &consents, &mut an) });
        assert!(r.is_ok(), "{r:?}");
        assert!(ai.requests.lock().unwrap().is_empty(), "proven の手順が在るのに AI を呼んだ: {:?}", ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// WF-E の反証（review の pane・yc/review-wfe）: AC の外の攻撃（新しい file・撃つ所 9 の材料を app の試験 `src/app_wfe_attack.rs` が使う）
#[cfg(test)]
pub(crate) mod wfe_attack;

/// ENGINE-AUDIT W5（節 1・8・11・spec 189「探しながら進む loop」・owner 206「探してから聞く」）の呼ぶ側の形（interface の commit で書いた・本体の前は赤 = `search` が todo!()）。
/// 負の対照（本体の後・手で）: `execute` の `Unfound` の腕を `Denied` に戻す → 1 本目が Err で赤・`run_steps` の `Unfound` の写しを `Step` に戻す → 同じく赤。
#[cfg(test)]
mod engine_audit_w5_interface {
    use super::w19_2_body::{cx, tmp};
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::llm::fake::Fake;
    use std::sync::{Arc, Mutex};

    /// 偽の desktop: 窓 1 つ。Library の button は最初から在る・押すと Guitar の行が出る・Guitar を押すと Acoustic Guitar の行が出る（GarageBand の Library の形）。
    /// 無い名前を押す手は `Failed { TargetNotFound }`（押していない = W5 の契約）。click は試験なので承認なし
    struct Desk {
        calls: Arc<Mutex<Vec<String>>>,
        opened: bool,
        category: bool,
        selected: bool,
    }
    static DESK: [Spec; 3] = [
        Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.ax.click", effect: Effect::LocalWrite, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "state_compare" },
    ];
    impl Adapter for Desk {
        fn id(&self) -> &'static str {
            "desktop"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::MacGui
        }
        fn route(&self) -> Route {
            Route::OsAutomation
        }
        fn provider(&self) -> &'static str {
            "desktop"
        }
        fn specs(&self) -> &'static [Spec] {
            &DESK
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            let label = input["label"].as_str().unwrap_or_default().to_owned();
            self.calls.lock().unwrap().push(if label.is_empty() { action.to_owned() } else { format!("{action} {label}") });
            let obs = |v: Value| Some(sdk::observe("desktop", sdk::now(), v));
            let row = |label: &str, selected: bool| json!({ "label": label, "role": "AXRow", "actions": ["AXPress"], "selected": selected, "path": ["Library"] });
            match action {
                "desktop.windows.list" => (Outcome::Verified { evidence: vec!["desk".into()] }, obs(json!([{ "pid": 7, "window_id": 1, "app_name": "Mixer", "title": "Untitled", "bundle_id": "com.example.mixer", "document": "" }]))),
                "desktop.window.state" => {
                    let mut controls = vec![json!({ "label": "Library", "role": "AXButton", "actions": ["AXPress"] }), json!({ "label": "Tracks", "role": "AXStaticText" })];
                    if self.opened {
                        controls.push(row("Guitar", self.category));
                    }
                    if self.category {
                        controls.push(row("Acoustic Guitar", self.selected));
                    }
                    (Outcome::Verified { evidence: vec!["desk".into()] }, obs(json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "complete": true, "controls": controls })))
                }
                _ => {
                    // 11-03 DEMO-FREEZE: 押す前に Adapter の門が断る名前（Busy = 人の lease・Twice = 手順のせいの拒否）
                    match label.as_str() {
                        "Busy" => return (Outcome::Rejected { reason: crate::gui::HUMAN_LEASE_LINE.into() }, None),
                        "Twice" => return (Outcome::Rejected { reason: "“Twice” is in 2 places in the window, so nothing was pressed".into() }, None),
                        _ => {}
                    }
                    let shown = match label.as_str() {
                        "Library" => true,
                        "Guitar" => self.opened,
                        "Acoustic Guitar" => self.category,
                        _ => false,
                    };
                    if !shown {
                        return (Outcome::Failed { error: ErrorCode::TargetNotFound }, None);
                    }
                    match label.as_str() {
                        "Library" => self.opened = true,
                        "Guitar" => self.category = true,
                        _ => self.selected = true,
                    }
                    (Outcome::Unverified { reason: "pressed".into() }, obs(json!({ "pressed": label })))
                }
            }
        }
        fn cleanup(&mut self) {}
    }

    /// 探す loop の 1 周: 2 手目の名前が無い → Adapter は押さず `Unfound` → Engine は run を終えず、窓を読み直して AI に次の 1 手（Guitar を押す）を頼み、走らせ、
    /// 同じ手（Acoustic Guitar）をやり直して済む（読み戻し = controls が変わった）。AI は interpret・generate・next_step の 3 回だけ。
    #[test]
    fn w5_an_unfound_name_is_searched_for_with_one_more_step_then_retried() {
        let d = tmp("w5-search");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Desk { calls: calls.clone(), opened: false, category: false, selected: false })).unwrap();
        reg.check();
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let goal = "Set the guitar to Acoustic Guitar";
        let click = |id: &str, label: &str| json!({ "id": id, "capability": "desktop.ax.click", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "label": label, "role": "", "match": "contains" }).to_string() });
        let check = json!({ "action": "desktop.window.state", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer" }).to_string(), "pointer": "/controls", "expect": "changed", "value_json": null });
        let ops = json!({ "operations": [{ "kind": "act", "named": true, "candidates": [], "id": "op1", "want": goal, "success": "Acoustic Guitar is selected", "target": "com.example.mixer" }] });
        let plan = json!({ "none_why": null, "workflow_steps": [click("s1", "Library"), click("s2", "Acoustic Guitar")], "checks": [check], "recipe": null, "trial": null });
        let next = json!({ "none_why": null, "workflow_steps": [click("x1", "Guitar")], "checks": [], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(ops), Ok(plan), Ok(next)]);
        let r = run(goal, &mut cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an));
        assert!(r.is_ok(), "探さずに止めた: {r:?}");
        let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, [INTERPRET_NAME, GENERATE_NAME, recipe::NEXT_STEP_NAME], "AI は 3 回（次の 1 手は 1 回）");
        let seen = calls.lock().unwrap().clone();
        let clicks: Vec<&str> = seen.iter().filter_map(|c| c.strip_prefix("desktop.ax.click ")).collect();
        assert_eq!(clicks, ["Library", "Acoustic Guitar", "Guitar", "Acoustic Guitar"], "無い名前 → 次の 1 手 → 同じ手をやり直す: {seen:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// operator 11-03（r10）: 手の前からもう合う contains（窓に初めから在る名前 Tracks）だけの計画は押した証拠にならない → 1 手も送らずに AI 1 回で作り直し
    /// （hint = `recipe::STALE_ONLY`）、作り直した計画（押すと現れる行 Guitar を contains）で走る。作り直しても同じ → 送らずに Generation。
    /// 負の対照（手で）: Verification の段の `stale` を全部 false に → 1 本目は AI が 2 回で赤・2 本目は Library を押して赤
    #[test]
    fn r10_a_contains_that_already_passed_before_the_steps_makes_the_plan_again() {
        let goal = "Show the guitars";
        let click = json!({ "id": "s1", "capability": "desktop.ax.click", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "label": "Library", "role": "", "match": "contains" }).to_string() });
        let has = |text: &str| json!({ "action": "desktop.window.state", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer" }).to_string(), "pointer": "/controls", "expect": "contains", "value_json": json!(text).to_string() });
        let plan = |text: &str| json!({ "none_why": null, "workflow_steps": [click.clone()], "checks": [has(text)], "recipe": null, "trial": null });
        let ops = json!({ "operations": [{ "kind": "act", "named": true, "candidates": [], "id": "op1", "want": goal, "success": "The guitars are listed", "target": "com.example.mixer" }] });
        for (second, ok) in [("Guitar", true), ("Tracks", false)] {
            let d = tmp("r10-stale");
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(Desk { calls: calls.clone(), opened: false, category: false, selected: false })).unwrap();
            reg.check();
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let ai = Fake::new(vec![Ok(ops.clone()), Ok(plan("Tracks")), Ok(plan(second))]);
            let r = run(goal, &mut cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an));
            let asked: Vec<(String, String)> = ai.requests.lock().unwrap().iter().map(|q| (q.name.clone(), q.input.clone())).collect();
            assert_eq!(asked.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), [INTERPRET_NAME, GENERATE_NAME, GENERATE_NAME], "{second}: 作り直しは 1 回: {r:?}");
            assert!(asked[2].1.contains("Every check already passed before any step ran"), "{second}: 作り直しに理由を渡していない");
            let clicks: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.starts_with("desktop.ax.click")).cloned().collect();
            if ok {
                let rec = r.unwrap_or_else(|e| panic!("作り直した計画で走らない: {e:?}"));
                assert_eq!(clicks, ["desktop.ax.click Library"]);
                assert!(rec.verified.iter().any(|v| v.contains("「Guitar」を含む")), "{:?}", rec.verified);
                let v = rec.stages.iter().find(|s| s.stage == Stage::Verification).expect("Verification の段");
                assert_eq!(v.counts.as_ref().map(|c| c["rebuilt"]["ai"].clone()), Some(json!(1)), "作り直しの AI が段の数字に無い: {v:?}");
            } else {
                assert!(matches!(&r, Err(EngineError::Generation(why)) if why == recipe::STALE_ONLY), "作り直しても初めから合う check だけなのに: {r:?}");
                assert!(clicks.is_empty(), "済みの根拠の無い計画で押した: {clicks:?}");
            }
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// 11-03（DEMO-FREEZE の読み）: 承認の要らない押す手（W7 の reveal と同じ = 止まっても `pending` 無し）が人の lease で断られた run は、保存した手順の
    /// 失敗に数えない（fails 0 のまま = 次の run も再利用し、「Make different steps」で作り直さない）。対照: 手順のせいの拒否（名前が 2 つ）は今まで通り fails 1。
    /// 負の対照（手で）: `counts_as_failure` の mark の除外を外す → Busy が fails 1 で赤・除外を全部の Denied に広げる → Twice が fails 0 で赤
    #[test]
    fn demo_freeze_a_lease_refusal_does_not_mark_the_saved_steps_failed() {
        let goal = "Show the guitars";
        for (label, fails) in [("Busy", 0), ("Twice", 1)] {
            let d = tmp("lease-fails");
            let saved = Workflow {
                proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), ui: None, goal: goal.into(),
                id: workflow_id(goal, &Operation { id: "op1".into(), ..Default::default() }),
                steps: vec![WorkflowStep { kind: StepKind::Action, id: "s1".into(), capability: "desktop.ax.click".into(), input: json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "label": label, "role": "" }) }],
                checks: vec![ReadBack { action: "desktop.window.state".into(), input: json!({ "pid": 7, "window_id": 1, "app_name": "Mixer" }), pointer: "/controls".into(), expect: Expect::Contains { text: "Guitar".into() } }],
            };
            save_workflow(&d.join(WORKFLOWS_DIR), &saved).unwrap();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(Desk { calls: calls.clone(), opened: false, category: false, selected: false })).unwrap();
            reg.check();
            let (mcp, mut an) = (McpConfig::default(), Vec::new());
            let ops = json!({ "operations": [{ "kind": "act", "named": true, "candidates": [], "id": "op1", "want": goal, "success": "The guitars are listed", "target": "com.example.mixer" }] });
            let ai = Fake::new(vec![Ok(ops)]);
            let r = run(goal, &mut cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an));
            assert!(matches!(&r, Err(EngineError::Denied(dn)) if dn.pending.is_none()), "{label}: 押す前に断られる（承認の要らない手 = pending 無し）: {r:?}");
            let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|q| q.name.clone()).collect();
            assert_eq!(names, [INTERPRET_NAME], "{label}: 保存した手順を使う（generate 0 回）");
            let now = load_workflows(&d.join(WORKFLOWS_DIR)).unwrap().into_iter().find(|w| w.id == saved.id).map(|w| w.fails);
            assert_eq!(now, Some(fails), "{label}: 保存した手順の fails");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// `run_steps` の契約: 押す名前が無い（`Failed { TargetNotFound }`）は `WorkflowError::Unfound { step, label, ambiguous: false }`（前の手は走った・その手は送っていない）。
    /// 1 つに絞れない（`TargetAmbiguous`）は ambiguous = true
    #[test]
    fn w5_run_steps_turns_target_not_found_into_unfound() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Desk { calls: calls.clone(), opened: false, category: false, selected: false })).unwrap();
        reg.check();
        let step = |id: &str, label: &str| WorkflowStep { kind: StepKind::Action, id: id.into(), capability: "desktop.ax.click".into(), input: json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "label": label, "role": "" }) };
        let wf = Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: Vec::new(), id: "w".into(), goal: "g".into(), ui: None, steps: vec![step("s1", "Library"), step("s2", "Acoustic Guitar")], checks: vec![ReadBack { action: "desktop.window.state".into(), input: json!({ "pid": 7, "window_id": 1 }), pointer: "/controls".into(), expect: Expect::Changed }] };
        let mut outputs = HashMap::new();
        let got = run_steps(&wf, &mut reg, w19_2_body::CALLER, &[], &mut outputs, 0, &|_| false, &NO_STOP, &mut |_, _| Ok(()));
        assert_eq!(got, Err(WorkflowError::Unfound { step: "s2".into(), label: "Acoustic Guitar".into(), ambiguous: false }), "{got:?}");
        assert_eq!(calls.lock().unwrap().iter().filter(|c| c.starts_with("desktop.ax.click")).count(), 2, "1 手目は走り、2 手目は押していない: {:?}", calls.lock().unwrap());
    }
}

/// ENGINE-AUDIT W9 本体（節 14・26「名指しの対象を代用しない」）の Engine 側の決まり: act だけを行う（question / change だけなら 1 行で止める・generate も Adapter も呼ばない）・
/// 名指したのに宛先が無ければ Page の app で埋めず 1 行で止める・候補が 2 つ以上も 1 行（chat の振り分けは Engine に来る前に 1 回聞く）・名指さない act は今まで通り Page の app で埋める。
/// 負の対照（手で）: `run_stages` の retain を外す → 1 本目で generate が呼ばれて赤・fill の `!op.named` を外す → 3 本目で代用して赤。
#[cfg(test)]
mod engine_audit_w9_body {
    use super::engine_adjust_stop_interface::trip;
    use super::w19_2_body::{cx, tmp};
    use super::*;
    use crate::llm::fake::Fake;

    fn op(kind: &str, want: &str, target: Option<&str>, named: bool, candidates: &[&str]) -> Value {
        json!({ "id": format!("op-{}", want.len()), "want": want, "success": "", "target": target, "kind": kind, "named": named, "candidates": candidates })
    }
    /// → (run の結果, AI の呼び名の列, generate の input に Page の app が宛先として入った数)
    fn run_with_ops(ops: Vec<Value>, target: Option<&str>) -> (Result<EngineRecord, EngineError>, Vec<String>, usize) {
        let d = tmp("w9-body");
        let (mut reg, _calls) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let none = json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(json!({ "operations": ops })), Ok(none.clone()), Ok(none)]);
        let r = run("Do it", &mut EngineContext { target, ..cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an) });
        let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        let filled = ai.requests.lock().unwrap().iter().filter(|r| r.name == GENERATE_NAME && r.input.contains("\"target\":\"com.example.page\"")).count();
        let _ = std::fs::remove_dir_all(&d);
        (r, names, filled)
    }

    #[test]
    fn w9_the_engine_does_only_acts_and_never_substitutes_a_named_target() {
        let (r, names, _) = run_with_ops(vec![op("question", "Is it open?", None, false, &[])], Some("com.example.page"));
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.why.contains("question")), "question だけ = 1 行: {r:?}");
        assert_eq!(names, [INTERPRET_NAME], "question なのに先へ進んだ: {names:?}");
        let (r, _, _) = run_with_ops(vec![op("change", "Add a button", None, false, &[])], Some("com.example.page"));
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.why.contains("Shibubu's own page")), "change だけ = 1 行: {r:?}");
        let (r, names, filled) = run_with_ops(vec![op("act", "Open the guitar list in GarageBand", None, true, &[])], Some("com.example.page"));
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.why.contains("which app you meant")), "名指しで宛先無し = 1 行: {r:?}");
        assert!(filled == 0 && names == [INTERPRET_NAME], "名指しの操作を Page の app で代用した: {names:?} / {filled}");
        let (r, _, _) = run_with_ops(vec![op("act", "Make it louder", None, false, &["com.example.a", "com.example.b"])], None);
        assert!(matches!(&r, Err(EngineError::Denied(d)) if d.why.contains("More than one could fit")), "候補 2 つ = 1 行: {r:?}");
        let (_, names, filled) = run_with_ops(vec![op("act", "Open the list", None, false, &[]), op("question", "Is it open?", None, false, &[])], Some("com.example.page"));
        assert!(names.contains(&GENERATE_NAME.to_string()) && filled >= 1, "名指さない act が Page の app に行かない: {names:?} / {filled}");
    }
}

/// r7（operator 11-03 ②）: generate の AI は 1 run で 2 回まで = 2 操作の run で 1 つ目が手順を作れば（1 回）、2 つ目は作り直しをせずに 1 回で手順 0 → NoMeans（前は 3 回・150 秒）。
/// 負の対照（手で）: `generate` の `cx.generate_calls >= GENERATE_CALLS_MAX` の門を外す → generate が 3 回で赤。
#[cfg(test)]
mod r7_generate_budget {
    use super::engine_adjust_stop_interface::trip;
    use super::w19_2_body::{cx, tmp};
    use super::*;
    use crate::llm::fake::Fake;

    #[test]
    fn r7_generate_asks_the_ai_at_most_twice_per_run() {
        let d = tmp("r7-budget");
        let (mut reg, _calls) = trip(None);
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let op = |id: &str, want: &str| json!({ "id": id, "want": want, "success": "", "target": null, "kind": "act", "named": false, "candidates": [] });
        let ops = json!({ "operations": [op("op1", "Write a"), op("op2", "Write b")] });
        let plan = json!({
            "none_why": null,
            "workflow_steps": [{ "id": "s1", "capability": "trip.text.write", "input_json": json!({ "text": "a" }).to_string() }],
            "checks": [{ "action": "trip.text.read", "input_json": "{}", "pointer": "/text", "expect": "changed", "value_json": null }],
            "recipe": null, "trial": null,
        });
        let none = json!({ "none_why": "Nothing on this Mac writes b", "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(ops), Ok(plan), Ok(none.clone()), Ok(none)]);
        let r = run("Write a and b", &mut cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an));
        assert!(matches!(&r, Err(EngineError::NoMeans { searched, .. }) if searched.next.as_deref() == Some("Nothing on this Mac writes b")), "{r:?}");
        let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.iter().filter(|n| *n == GENERATE_NAME).count(), 2, "1 run で generate を 3 回呼んだ: {names:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// r7（operator 11-03 ①・梯子の段 0・一般の読み「一覧は選択で中身が替わる」）: 目当ての語が今の面に無く AI が手順 0 の時、目当てに関係する見える行を 1 つ選んで読み直す
/// 文脈の手（context）を走らせ、窓が替わったら同じ操作を generate し直す。偽の desktop = Tracks の行「Drummer」「Software Instrument」と Library の行「Rock」。
/// 「Software Instrument」を押すと Library が「Acoustic Guitar」に替わる（GarageBand の Library の形・app の名前は無い）。
/// 負の対照（手で）: generate の context の枝を外す → NoMeans で赤・execute の `context` の continue を外す → 文脈の手だけで Done になり clicks が 1 つで赤。
#[cfg(test)]
mod r7_context_step {
    use super::w19_2_body::{cx, tmp};
    use super::*;
    use crate::adapters::sdk::{Adapter, Route, Spec};
    use crate::domain::binding::Source;
    use crate::llm::fake::Fake;
    use std::sync::{Arc, Mutex};

    struct Desk {
        calls: Arc<Mutex<Vec<String>>>,
        track: bool,
        selected: bool,
    }
    static DESK: [Spec; 3] = [
        Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.ax.click", effect: Effect::LocalWrite, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "state_compare" },
    ];
    impl Adapter for Desk {
        fn id(&self) -> &'static str {
            "desktop"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::MacGui
        }
        fn route(&self) -> Route {
            Route::OsAutomation
        }
        fn provider(&self) -> &'static str {
            "desktop"
        }
        fn specs(&self) -> &'static [Spec] {
            &DESK
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            let label = input["label"].as_str().unwrap_or_default().to_owned();
            self.calls.lock().unwrap().push(if label.is_empty() { action.to_owned() } else { format!("{action} {label}") });
            let obs = |v: Value| Some(sdk::observe("desktop", sdk::now(), v));
            let row = |label: &str, path: &str, selected: bool| json!({ "label": label, "role": "AXRow", "actions": ["AXPress"], "selected": selected, "path": [path] });
            match action {
                "desktop.windows.list" => (Outcome::Verified { evidence: vec!["desk".into()] }, obs(json!([{ "pid": 7, "window_id": 1, "app_name": "Mixer", "title": "Song", "bundle_id": "com.example.mixer", "document": "" }]))),
                "desktop.window.state" => {
                    let mut controls = vec![json!({ "label": "Record", "role": "AXButton", "actions": ["AXPress"] }), row("Drummer", "Tracks", !self.track), row("Software Instrument", "Tracks", self.track)];
                    controls.push(if self.track { row("Acoustic Guitar", "Library", self.selected) } else { row("Rock", "Library", false) });
                    (Outcome::Verified { evidence: vec!["desk".into()] }, obs(json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "complete": true, "controls": controls })))
                }
                _ => {
                    let shown = match label.as_str() {
                        "Software Instrument" | "Drummer" => true,
                        "Acoustic Guitar" => self.track,
                        _ => false,
                    };
                    if !shown {
                        return (Outcome::Failed { error: ErrorCode::TargetNotFound }, None);
                    }
                    match label.as_str() {
                        "Software Instrument" => self.track = true,
                        "Acoustic Guitar" => self.selected = true,
                        _ => {}
                    }
                    (Outcome::Unverified { reason: "pressed".into() }, obs(json!({ "pressed": label })))
                }
            }
        }
        fn cleanup(&mut self) {}
    }

    #[test]
    fn r7_a_context_step_reveals_the_target_and_the_operation_is_generated_again() {
        let d = tmp("r7-context");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Desk { calls: calls.clone(), track: false, selected: false })).unwrap();
        reg.check();
        let (mcp, mut an) = (McpConfig::default(), Vec::new());
        let goal = "Set the guitar to Acoustic Guitar";
        let click = |id: &str, label: &str| json!({ "id": id, "capability": "desktop.ax.click", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer", "label": label, "role": "", "match": "contains" }).to_string() });
        let check = json!({ "action": "desktop.window.state", "input_json": json!({ "pid": 7, "window_id": 1, "app_name": "Mixer" }).to_string(), "pointer": "/controls", "expect": "changed", "value_json": null });
        let ops = json!({ "operations": [{ "kind": "act", "named": true, "candidates": [], "id": "op1", "want": goal, "success": "Acoustic Guitar is selected", "target": "com.example.mixer" }] });
        let none = json!({ "none_why": "No Guitar row is visible; the Library shows drum kits", "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let context = json!({ "none_why": null, "workflow_steps": [click("x1", "Software Instrument")], "checks": [], "recipe": null, "trial": null });
        let plan = json!({ "none_why": null, "workflow_steps": [click("s1", "Acoustic Guitar")], "checks": [check], "recipe": null, "trial": null });
        let ai = Fake::new(vec![Ok(ops), Ok(none.clone()), Ok(none), Ok(context), Ok(plan)]);
        let r = run(goal, &mut cx(&mut reg, &d, Some(&ai), &mcp, &[], &mut an));
        assert!(r.is_ok(), "文脈の手の後に generate し直していない: {r:?}");
        let names: Vec<String> = ai.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, [INTERPRET_NAME, GENERATE_NAME, GENERATE_NAME, recipe::NEXT_STEP_NAME, GENERATE_NAME], "手順 0 × 2 → 文脈の手 1 → 読み直して generate 1");
        let seen = calls.lock().unwrap().clone();
        let clicks: Vec<&str> = seen.iter().filter_map(|c| c.strip_prefix("desktop.ax.click ")).collect();
        assert_eq!(clicks, ["Software Instrument", "Acoustic Guitar"], "文脈の手 → 目当て: {seen:?}");
        // 文脈の手だけの手順は保存しない（目当ての手順だけ）
        let saved = load_workflows(&d.join(WORKFLOWS_DIR)).unwrap();
        assert!(saved.iter().all(|w| w.steps.iter().all(|s| s.input["label"] != "Software Instrument")), "文脈の手を手順として保存した: {saved:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
