//! goal: GoalPlan（`docs/schemas/goal_plan_v1.schema.json` と 1 対 1・CD-10）と GoalInterpreter（GEN-001）。
//!
//! 解釈は prompt ごとの template を選ばない（docs/ACCEPTANCE.md §1.1「何を頼まれても対応させる」）。文を独立な 3 軸に分けて組む:
//! - **job**（述語）: 覚える / 記録する / 見える化する / 計画する / 整理する / 把握する / 書く / 計る、と
//!   接続が前提の領域（会話・端末の画面・動画の編集・録音）。述語は文末に来るので、当たった語のうち一番後ろを採る。
//! - **物**（名詞句）: 利用者の言葉をそのまま label にし、「物の性質」の表（お金 → 金額・分類・日付 など）で欄を決める。
//!   複合語は後ろが主（「研究ノート」→ ノート）。表に無い物は job の既定の欄（記録 → 日付・内容・メモ）に落ちる。
//! - **修飾**: 毎朝 などの頻度・名指しされたサービス・出典の要求（研究・論文）・準備（持ち物）。
//!
//! サービス・端末は名指しされた時だけ provider に残す（DESIGN §0-1・TEST-004）。目的が読めない時だけ 1 問返す（CD-05 規則 6）。
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

use super::domain::action::Effect;
use super::domain::capability::AdapterKind;

// ============================== GoalPlan（schema と 1 対 1） ==============================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanSchemaVersion {
    #[serde(rename = "1.0")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPlan {
    pub schema_version: PlanSchemaVersion,
    pub id: String,
    pub prompt: String,
    pub goal: PlanGoal,
    pub requirements: Vec<Requirement>,
    pub needs: Vec<Need>,
    pub source_policy: SourcePolicy,
    pub candidate_sources: Vec<CandidateSource>,
    pub ui_intents: Vec<UiIntent>,
    pub questions: Vec<Question>,
    pub verification_plan: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanGoal {
    pub objective: String,
    pub success_signal: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub statement: String,
    pub origin: Origin,
    pub necessity: Necessity,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Explicit,
    Inferred,
    Confirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Necessity {
    Essential,
    Useful,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Need {
    pub id: String,
    pub description: String,
    pub effect: NeedEffect,
    pub status: NeedStatus,
}

/// plan の effect（schema の 5 値）。runtime の 6 値（domain の Effect）への写像は CD-01。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedEffect {
    Pure,
    Read,
    LocalWrite,
    ExternalWrite,
    Execute,
}

impl NeedEffect {
    /// CD-01: action の effect → need の effect（need は bind する action の中で一番強い物）。
    pub fn from_effect(e: Effect) -> Self {
        match e {
            Effect::LocalRead | Effect::ExternalRead => Self::Read,
            Effect::LocalWrite | Effect::Destructive => Self::LocalWrite,
            Effect::ExternalWrite | Effect::ExternalSend => Self::ExternalWrite,
        }
    }
    fn rank(self) -> u8 {
        match self {
            Self::Pure => 0,
            Self::Read => 1,
            Self::LocalWrite => 2,
            Self::ExternalWrite => 3,
            Self::Execute => 4,
        }
    }
    pub fn max(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// planner だけの状態（CD-02: runtime の readiness に写さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedStatus {
    Proposed,
    PlannedLocal,
    NeedsConnection,
    NeedsPermission,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePolicy {
    LocalOnly,
    OptionalConnections,
    RequestedConnection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSource {
    pub id: String,
    pub label: String,
    pub kind: AdapterKind,
    pub evidence: SourceEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEvidence {
    UserNamed,
    CatalogCandidate,
    UserAuthorizedDiscovery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiIntent {
    pub id: String,
    pub purpose: String,
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub blocking_need_id: String,
    pub reason: QuestionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_default: Option<String>,
}

impl Question {
    /// CD-05 規則 1: 既定で進められない question だけが preview を止める。
    pub fn is_blocking(&self) -> bool {
        self.suggested_default.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionReason {
    Identity,
    Privacy,
    ExternalEffect,
    Payment,
    IrreduciblePreference,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    BadId { what: &'static str, id: String },
    Duplicate { what: &'static str, id: String },
    Confidence { id: String },
    Empty(&'static str),
    PrimaryIntents(usize),
    BlockingQuestions(usize),
    UnknownBlockingNeed { question: String, need: String },
    LocalOnlyHasSources,
    LocalOnlyHasQuestions,
    LocalOnlyNeedStatus { need: String },
    PretendsLocal { need: String },
    RequestedWithoutSource,
    NamedSourceNotInPrompt { source: String },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId { what, id } => write!(f, "{what} id '{id}' は ^[a-z][a-z0-9_]{{1,79}}$ に合わない"),
            Self::Duplicate { what, id } => write!(f, "{what} id '{id}' が重複"),
            Self::Confidence { id } => write!(f, "requirement '{id}' の confidence は 0〜1"),
            Self::Empty(what) => write!(f, "'{what}' が空か短すぎる"),
            Self::PrimaryIntents(n) => write!(f, "primary の ui_intent は 1 つ（{n} 個）"),
            Self::BlockingQuestions(n) => write!(f, "既定の無い（blocking な）question は 1 つまで（{n} 個）"),
            Self::UnknownBlockingNeed { question, need } => write!(f, "question '{question}' の blocking_need_id '{need}' が needs に無い"),
            Self::LocalOnlyHasSources => write!(f, "local_only なのに candidate_sources がある"),
            Self::LocalOnlyHasQuestions => write!(f, "local_only なのに question がある"),
            Self::LocalOnlyNeedStatus { need } => write!(f, "local_only なのに need '{need}' が接続 / 許可待ち"),
            Self::PretendsLocal { need } => write!(f, "need '{need}' は外への書き込み / 実行なのに planned_local"),
            Self::RequestedWithoutSource => write!(f, "requested_connection なのに候補の取得元も接続時の question も無い"),
            Self::NamedSourceNotInPrompt { source } => write!(f, "user_named の候補 '{source}' は prompt に書かれていない"),
        }
    }
}
impl std::error::Error for PlanError {}

/// goal_plan_v1 の id pattern `^[a-z][a-z0-9_]{1,79}$`（v3 の id とは別: CD-08）。
pub fn plan_id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=80).contains(&b.len()) && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// python `validate_goal_plans.py` の 6 規則 ＋ CD-05 の規則 1〜5（V15 の GoalPlan 部分）。全部集めて返す。
pub fn validate_plan(p: &GoalPlan) -> Result<(), Vec<PlanError>> {
    use PlanError::*;
    let mut errs = Vec::new();
    fn ids(what: &'static str, list: Vec<&str>, errs: &mut Vec<PlanError>) {
        let mut seen = HashSet::new();
        for id in list {
            if !plan_id_ok(id) {
                errs.push(PlanError::BadId { what, id: id.into() });
            }
            if !seen.insert(id) {
                errs.push(PlanError::Duplicate { what, id: id.into() });
            }
        }
    }
    ids("plan", vec![p.id.as_str()], &mut errs);
    ids("requirement", p.requirements.iter().map(|r| r.id.as_str()).collect(), &mut errs);
    ids("need", p.needs.iter().map(|n| n.id.as_str()).collect(), &mut errs);
    ids("candidate_source", p.candidate_sources.iter().map(|s| s.id.as_str()).collect(), &mut errs);
    ids("ui_intent", p.ui_intents.iter().map(|u| u.id.as_str()).collect(), &mut errs);
    ids("question", p.questions.iter().map(|q| q.id.as_str()).collect(), &mut errs);
    let short = |s: &str| s.chars().count() < 2;
    if p.prompt.trim().is_empty() || p.prompt.chars().count() > 4000 {
        errs.push(Empty("prompt"));
    }
    if short(&p.goal.objective) {
        errs.push(Empty("goal.objective"));
    }
    if short(&p.goal.success_signal) {
        errs.push(Empty("goal.success_signal"));
    }
    if p.verification_plan.is_empty() || p.verification_plan.iter().any(|v| short(v)) {
        errs.push(Empty("verification_plan"));
    }
    for r in &p.requirements {
        if !(0.0..=1.0).contains(&r.confidence) {
            errs.push(Confidence { id: r.id.clone() });
        }
    }
    let primaries = p.ui_intents.iter().filter(|u| u.primary).count();
    if primaries != 1 {
        errs.push(PrimaryIntents(primaries));
    }
    for q in &p.questions {
        if !p.needs.iter().any(|n| n.id == q.blocking_need_id) {
            errs.push(UnknownBlockingNeed { question: q.id.clone(), need: q.blocking_need_id.clone() });
        }
    }
    let blocking = p.questions.iter().filter(|q| q.is_blocking()).count();
    if blocking > 1 {
        errs.push(BlockingQuestions(blocking));
    }
    for n in &p.needs {
        if matches!(n.effect, NeedEffect::ExternalWrite | NeedEffect::Execute) && n.status == NeedStatus::PlannedLocal {
            errs.push(PretendsLocal { need: n.id.clone() });
        }
    }
    let prompt = p.prompt.to_lowercase();
    for s in &p.candidate_sources {
        if s.evidence == SourceEvidence::UserNamed && !prompt.contains(&s.label.to_lowercase()) {
            errs.push(NamedSourceNotInPrompt { source: s.id.clone() });
        }
    }
    match p.source_policy {
        SourcePolicy::LocalOnly => {
            if !p.candidate_sources.is_empty() {
                errs.push(LocalOnlyHasSources);
            }
            if !p.questions.is_empty() {
                errs.push(LocalOnlyHasQuestions);
            }
            for n in &p.needs {
                if matches!(n.status, NeedStatus::NeedsConnection | NeedStatus::NeedsPermission) {
                    errs.push(LocalOnlyNeedStatus { need: n.id.clone() });
                }
            }
        }
        SourcePolicy::RequestedConnection => {
            // CD-05 規則 4・5: 接続時に出す question = 接続 / 許可待ちの need を塞ぐ identity / privacy / 外部効果 / 支払いの question
            let deferred = p.questions.iter().any(|q| {
                !matches!(q.reason, QuestionReason::IrreduciblePreference)
                    && p.needs.iter().any(|n| n.id == q.blocking_need_id && matches!(n.status, NeedStatus::NeedsConnection | NeedStatus::NeedsPermission))
            });
            if p.candidate_sources.is_empty() && !deferred {
                errs.push(RequestedWithoutSource);
            }
        }
        SourcePolicy::OptionalConnections => {}
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

// ============================== GoalInterpreter ==============================

/// 利用者が何をするか（述語）。DESIGN §7 の「job」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// 時間を計る（状態が 1 つ）
    Measure,
    /// 一覧を作って直す（CRUD + 絞り込み）
    Organize,
    /// 本文を書いて残す
    Author,
    /// 状態のある物を 1 画面で見る
    Monitor,
    /// 1 枚ずつ覚える
    Memorize,
    /// 数を記録して傾向を見る（chart が正当化される唯一の job）
    Visualize,
    /// 日付つきで記録し続ける（入力の頻度が高い）
    Log,
    /// 日付の上に並べて組み立てる
    Plan,
    /// 外のアカウントの会話を読む・送る（接続が前提）
    Communicate,
    /// 端末の画面を映す・操作する（接続が前提）
    Mirror,
    /// 素材をまとめ、編集は既存の editor へ渡す
    EditMedia,
    /// mic から録る
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Multiline,
    Integer,
    Number,
    Date,
    DateTime,
    Select,
    Boolean,
}

/// entity の欄（form の fields・action の input_schema・table の columns の元）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldSpec {
    pub name: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// 作った後に変える欄（状態・数量・本文）→ `update_<name>`（boolean は toggle_done）
    pub mutable: bool,
    /// 一覧の見出し（題名・名前・単語）
    pub title: bool,
    /// runtime が入れる時刻（利用者は入力しない）
    pub auto: bool,
}

impl FieldSpec {
    pub fn is_numeric(&self) -> bool {
        matches!(self.kind, FieldKind::Integer | FieldKind::Number)
    }
    pub fn is_date(&self) -> bool {
        matches!(self.kind, FieldKind::Date | FieldKind::DateTime) && !self.auto
    }
    /// form の `input`（corpus の語: text / multiline / integer / number / date / datetime / select / boolean）。
    pub fn input(&self) -> &'static str {
        match self.kind {
            FieldKind::Text => "text",
            FieldKind::Multiline => "multiline",
            FieldKind::Integer => "integer",
            FieldKind::Number => "number",
            FieldKind::Date => "date",
            FieldKind::DateTime => "datetime",
            FieldKind::Select => "select",
            FieldKind::Boolean => "boolean",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// 主役の collection（timer は local_state）
    Primary,
    /// 取り込んだ資料（出典）
    Imports,
    /// 持ち物・準備
    Checklist,
    /// 返信の下書き
    Drafts,
    /// 繋げる候補（runtime が Connections から埋める。composer は列挙しない）
    Candidates,
    /// 端末の繋ぎ方の候補
    Pairing,
    /// 動画の素材
    Clips,
    /// 切る位置
    Markers,
    /// 録音
    Recordings,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entity {
    /// source は `local.<id>`（ASCII）
    pub id: String,
    /// 利用者の言葉（「英単語」「家計」）
    pub label: String,
    pub kind: EntityKind,
    pub fields: Vec<FieldSpec>,
    /// CSV でまとめて入れる意味がある
    pub bulk: bool,
    /// add は picker で選んだ file の参照（file_ref）
    pub picker: bool,
}

impl Entity {
    pub fn title_field(&self) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.title)
    }
    pub fn field(&self, name: &str) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.name == name)
    }
    pub fn date_field(&self) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.is_date())
    }
    pub fn numeric_field(&self) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.is_numeric())
    }
    /// 分類の欄（select か、名前が category / tag / kind の text）。
    pub fn category_field(&self) -> Option<&FieldSpec> {
        self.fields
            .iter()
            .find(|f| !f.title && (f.kind == FieldKind::Select || matches!(f.name.as_str(), "category" | "tag" | "kind")) && f.name != "status")
    }
    pub fn status_field(&self) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.kind == FieldKind::Select && f.mutable)
    }
}

/// 接続先の action が画面のどこに座るか（layout が消費する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalRole {
    /// 会話を読む（Communicate の主役）
    Threads,
    /// 確認して送る
    Send,
    /// 端末の画面
    Screen,
    /// 端末へ入力
    Input,
    /// 既存の editor を開く（handoff）
    Editor,
    /// mic
    Mic,
    /// 状況の取得元（Monitor の任意の取得元）
    MonitorRead,
    /// 名指しされたサービスのその他の能力
    Extra,
}

/// 接続が要る能力。provider は名指しされた時だけ（決め打ちしない）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExternalNeed {
    pub action: String,
    pub capability: String,
    pub label: String,
    pub effect: Effect,
    pub approval: bool,
    pub role: ExternalRole,
    /// bind する source（無ければ node は action だけ持つ）
    pub source: Option<String>,
    pub source_kind: String,
    pub source_label: String,
    pub provider: Option<String>,
    pub adapter: Option<String>,
    /// 利用者が書いたサービス名（candidate_sources の label。名指しの時だけ）
    pub named: Option<String>,
    pub necessity: Necessity,
    pub input: Vec<FieldSpec>,
}

/// GEN-001 の出力: outcome / actors / objects / flows / constraints / confidence / unknowns。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub literal: String,
    /// 利用者の名詞句（「英単語」「旅行の計画」）。頻度の前置きは落とす
    pub subject: String,
    pub title: String,
    pub job: Job,
    pub cadence: Option<Cadence>,
    /// 出典が要る（研究・論文・資料）
    pub research: bool,
    /// 物の性質の表に当たったか（外れなら job の既定の欄）
    pub known_thing: bool,
    pub entities: Vec<Entity>,
    pub externals: Vec<ExternalNeed>,
    pub objective: String,
    pub outcomes: Vec<String>,
    pub actors: Vec<String>,
    pub constraints: Vec<String>,
    pub assumptions: Vec<String>,
    pub unknowns: Vec<String>,
    pub confidence: f64,
    /// どの語でそう読んだか（説明用）
    pub evidence: Vec<String>,
}

impl Reading {
    pub fn primary(&self) -> Option<&Entity> {
        self.entities.iter().find(|e| e.kind == EntityKind::Primary)
    }
    pub fn entity(&self, kind: EntityKind) -> Option<&Entity> {
        self.entities.iter().find(|e| e.kind == kind)
    }
    pub fn is_local_only(&self) -> bool {
        self.externals.is_empty()
    }
    pub fn named_services(&self) -> bool {
        self.externals.iter().any(|x| x.named.is_some())
    }
}

pub enum Interpretation {
    Goal(Box<Reading>),
    /// 目的そのものが読めない（CD-05 規則 6: blocking 1 問 ＋ 候補の ui_intents）
    Unclear(Box<GoalPlan>),
    /// 今ある app への変更（「検索を追加」）。新しい app を作らない
    Edit,
    Empty,
}

// ---------- 語彙の表（物の性質・述語・頻度・サービス）。prompt ごとの行は無い ----------

#[derive(Clone, Copy)]
struct F {
    name: &'static str,
    label: &'static str,
    kind: FieldKind,
    options: &'static [&'static str],
    mutable: bool,
    title: bool,
    auto: bool,
}

const fn fld(name: &'static str, label: &'static str, kind: FieldKind) -> F {
    F { name, label, kind, options: &[], mutable: false, title: false, auto: false }
}
const fn head(name: &'static str, label: &'static str) -> F {
    F { title: true, ..fld(name, label, FieldKind::Text) }
}
const fn long(name: &'static str, label: &'static str) -> F {
    F { mutable: true, ..fld(name, label, FieldKind::Multiline) }
}
const fn pick(name: &'static str, label: &'static str, options: &'static [&'static str], mutable: bool) -> F {
    F { options, mutable, ..fld(name, label, FieldKind::Select) }
}
const fn flag(name: &'static str, label: &'static str) -> F {
    F { mutable: true, ..fld(name, label, FieldKind::Boolean) }
}
const fn changes(f: F) -> F {
    F { mutable: true, ..f }
}
const fn stamp(name: &'static str, label: &'static str) -> F {
    F { auto: true, ..fld(name, label, FieldKind::DateTime) }
}

impl F {
    fn spec(self) -> FieldSpec {
        FieldSpec {
            name: self.name.into(),
            label: self.label.into(),
            kind: self.kind,
            options: self.options.iter().map(|s| (*s).to_string()).collect(),
            mutable: self.mutable,
            title: self.title,
            auto: self.auto,
        }
    }
}

use FieldKind::{Date, Integer, Number, Text};

/// 物の性質の表。terms は小文字で contains（ASCII の語は語境界つき）。job は述語が無い時の既定。
struct Thing {
    id: &'static str,
    terms: &'static [&'static str],
    job: Job,
    fields: &'static [F],
    bulk: bool,
    checklist: bool,
    picker: bool,
}

const TIMER_FIELDS: &[F] = &[fld("duration_minutes", "Minutes", Integer)];
const NOTE_FIELDS: &[F] = &[head("title", "Title"), long("body", "Body"), stamp("updated_at", "Updated")];

const THINGS: &[Thing] = &[
    Thing { id: "timer", terms: &["タイマー", "ストップウォッチ", "カウントダウン", "ポモドーロ", "timer", "stopwatch", "countdown", "pomodoro"], job: Job::Measure, fields: TIMER_FIELDS, bulk: false, checklist: false, picker: false },
    Thing {
        id: "ledger",
        terms: &["家計", "支出", "収入", "出費", "予算", "お金", "費用", "経費", "金額", "売上", "貯金", "家賃", "小遣い", "支払", "買い物", "レシート", "expense", "budget", "money", "spending"],
        job: Job::Log,
        fields: &[fld("date", "Date", Date), pick("category", "Category", &["Food", "Household", "Transport", "Housing", "Leisure", "Income", "Other"], false), fld("amount", "Amount", Number), fld("memo", "Note", Text)],
        bulk: true,
        checklist: false,
        picker: false,
    },
    Thing { id: "inventory", terms: &["在庫", "備品", "棚卸", "ストック", "商品", "inventory", "stock"], job: Job::Organize, fields: &[head("name", "Name"), changes(fld("quantity", "Quantity", Integer)), stamp("updated_at", "Updated")], bulk: true, checklist: false, picker: false },
    Thing { id: "words", terms: &["英単語", "単語", "語彙", "フレーズ", "漢字", "熟語", "用語", "例文", "vocabulary", "flashcard", "word"], job: Job::Memorize, fields: &[head("term", "Word"), fld("meaning", "Meaning", Text), flag("done", "Learned")], bulk: true, checklist: false, picker: false },
    Thing {
        id: "books",
        terms: &["読書", "書籍", "蔵書", "本棚", "小説", "漫画", "マンガ", "絵本", "book", "reading", "novel"],
        job: Job::Log,
        fields: &[head("title", "Title"), fld("author", "Author", Text), pick("status", "Status", &["Want to read", "Reading", "Read"], true), fld("finished_on", "Finished on", Date), long("note", "Thoughts")],
        bulk: false,
        checklist: false,
        picker: false,
    },
    Thing { id: "watched", terms: &["映画", "ドラマ", "アニメ", "movie", "film"], job: Job::Log, fields: &[head("title", "Title"), pick("status", "Status", &["Want to watch", "Watching", "Watched"], true), fld("watched_on", "Watched on", Date), long("note", "Thoughts")], bulk: false, checklist: false, picker: false },
    Thing {
        id: "workouts",
        terms: &["ジム", "筋トレ", "トレーニング", "ワークアウト", "ウエイト", "workout", "gym"],
        job: Job::Log,
        fields: &[fld("date", "Date", Date), head("exercise", "Exercise"), fld("weight", "Weight (kg)", Number), fld("reps", "Reps", Integer), fld("sets", "Sets", Integer)],
        bulk: false,
        checklist: false,
        picker: false,
    },
    Thing {
        id: "activities",
        terms: &["ランニング", "ジョギング", "ウォーキング", "マラソン", "サイクリング", "水泳", "散歩", "running", "jogging", "walking"],
        job: Job::Log,
        fields: &[fld("date", "Date", Date), head("kind", "Kind"), fld("distance_km", "Distance (km)", Number), fld("duration_minutes", "Minutes", Integer), fld("memo", "Note", Text)],
        bulk: false,
        checklist: false,
        picker: false,
    },
    Thing { id: "measurements", terms: &["体重", "睡眠", "血圧", "体温", "歩数", "体調", "気分", "健康", "sleep"], job: Job::Log, fields: &[fld("date", "Date", Date), fld("value", "Value", Number), fld("memo", "Note", Text)], bulk: false, checklist: false, picker: false },
    Thing {
        id: "itinerary",
        terms: &["旅行", "旅", "出張", "観光", "旅程", "遠征", "キャンプ", "trip", "travel", "vacation"],
        job: Job::Plan,
        fields: &[fld("date", "Date", Date), fld("time", "Time", Text), head("place", "Place or activity"), fld("memo", "Note", Text)],
        bulk: false,
        checklist: true,
        picker: false,
    },
    Thing {
        id: "events",
        terms: &["予定", "スケジュール", "会議", "イベント", "打ち合わせ", "ミーティング", "カレンダー", "schedule", "meeting", "calendar", "event"],
        job: Job::Plan,
        fields: &[head("title", "Title"), fld("starts_at", "Starts", FieldKind::DateTime), fld("ends_at", "Ends", FieldKind::DateTime), fld("place", "Place", Text)],
        bulk: false,
        checklist: false,
        picker: false,
    },
    Thing {
        id: "reports",
        terms: &["バグ", "不具合", "障害", "エラー", "クラッシュ", "問い合わせ", "クレーム", "bug", "defect", "incident", "crash"],
        job: Job::Organize,
        fields: &[head("title", "Title"), pick("severity", "Severity", &["High", "Medium", "Low"], false), pick("status", "Status", &["Open", "In progress", "Resolved"], true), long("detail", "Details and steps to reproduce"), stamp("reported_at", "Reported")],
        bulk: true,
        checklist: false,
        picker: false,
    },
    Thing {
        id: "tasks",
        terms: &["仕事", "業務", "タスク", "やること", "やる事", "todo", "to-do", "プロジェクト", "作業", "案件", "宿題", "課題", "task", "project"],
        job: Job::Organize,
        fields: &[head("title", "Title"), pick("status", "Status", &["Not started", "In progress", "Waiting", "Done"], true), fld("due", "Due", Date), stamp("updated_at", "Updated")],
        bulk: false,
        checklist: false,
        picker: false,
    },
    Thing {
        id: "people",
        terms: &["顧客", "連絡先", "名簿", "メンバー", "友達", "友人", "人脈", "会員", "生徒", "取引先", "customer", "contact", "member"],
        job: Job::Organize,
        fields: &[head("name", "Name"), fld("group", "Group", Text), fld("contact", "Contact", Text), changes(fld("last_contact", "Last contacted", Date)), long("memo", "Note")],
        bulk: true,
        checklist: false,
        picker: false,
    },
    Thing { id: "notes", terms: &["ノート", "メモ", "日記", "議事録", "記事", "ブログ", "原稿", "下書き", "レポート", "論文", "note", "memo", "diary", "journal", "blog", "article"], job: Job::Author, fields: NOTE_FIELDS, bulk: false, checklist: false, picker: false },
    Thing { id: "recipes", terms: &["レシピ", "料理", "献立", "作り置き", "recipe", "cooking"], job: Job::Organize, fields: &[head("title", "Recipe"), fld("tag", "Category", Text), long("ingredients", "Ingredients"), long("steps", "Steps")], bulk: false, checklist: false, picker: false },
    Thing { id: "habits", terms: &["習慣", "日課", "ルーティン", "habit", "routine"], job: Job::Log, fields: &[fld("date", "Date", Date), head("title", "Habit"), flag("done", "Done")], bulk: false, checklist: false, picker: false },
    Thing { id: "files", terms: &["ファイル", "写真", "画像", "pdf", "資料", "スクリーンショット", "file", "photo", "image"], job: Job::Organize, fields: &[head("name", "Name"), fld("kind", "Kind", Text), stamp("added_at", "Added")], bulk: false, checklist: false, picker: true },
];

/// 述語 → job。文末に近い（一番後ろの）語を採る。
const JOB_WORDS: &[(Job, &[&str])] = &[
    (Job::Memorize, &["覚え", "暗記", "記憶", "復習", "習得", "身につけ", "勉強", "学習", "memorize", "learn", "study"]),
    (Job::Visualize, &["見える化", "可視化", "分析", "推移", "グラフ", "傾向", "visualize", "visualise", "chart", "analyze", "trend"]),
    (Job::Plan, &["計画", "立て", "段取り", "旅程", "プラン", "準備", "plan", "itinerary"]),
    (Job::Log, &["記録", "をつけ", "残し", "残す", "残せ", "ログ", "日誌", "続け", "練習", "log", "track"]),
    (Job::Monitor, &["把握", "状況", "進捗", "見守", "様子", "モニタ", "overview", "monitor", "progress", "status"]),
    (Job::Organize, &["整理", "管理", "まとめ", "片付", "集め", "一覧", "リスト", "整え", "organize", "organise", "manage", "collect"]),
    (Job::Author, &["書き", "書く", "書け", "執筆", "write", "draft"]),
    (Job::Measure, &["計る", "測る", "計り", "測り", "計測", "measure"]),
];

const CADENCE: &[(Cadence, &[&str])] = &[
    (Cadence::Daily, &["毎朝", "毎日", "毎晩", "日々", "毎夜", "daily", "every day", "every morning", "every night"]),
    (Cadence::Weekly, &["毎週", "週に", "weekly", "every week"]),
    (Cadence::Monthly, &["毎月", "月に", "monthly", "every month"]),
];
const CADENCE_PREFIXES: &[&str] = &["毎朝の", "毎日の", "毎晩の", "毎週の", "毎月の", "日々の", "今日の", "毎朝", "毎日", "毎晩", "毎週", "毎月", "日々"];
const CONTAINER_SUFFIXES: &[&str] = &["のアプリ", "アプリ", "のツール", "ツール", "の仕組み", "app", "tool"];
const RESEARCH: &[&str] = &["研究", "論文", "調査", "文献", "出典", "引用", "資料", "research", "paper", "citation"];
const CHECKLIST: &[&str] = &["準備", "持ち物", "チェックリスト", "checklist", "packing"];
const VAGUE: &[&str] = &[
    "便利", "いい感じ", "良い感じ", "何か", "なにか", "何", "すごい", "かっこいい", "楽", "簡単", "色々", "いろいろ", "適当", "おすすめ", "もの", "こと", "それ", "これ", "あれ", "全部", "something", "anything", "it",
];
const EDIT_WORDS: &[&str] = &["追加", "足して", "消して", "削除", "外して", "並び替え", "並べ替え", "ソート", "可能に", "できるように", "見やすく", "add ", "remove ", "sort", "make it"];
/// 述語（job の語）の直後に付けば目的の言い方（「記録できるようにしたい」「管理可能にしたい」）で、変更の語ではない。
const MODAL_EDIT: &[&str] = &["可能に", "できるように"];
/// 英語の文頭の動詞・冠詞（名詞句を取り出す時に落とす）。
const EN_LEAD: &[&str] = &["i want to ", "i want ", "help me ", "please ", "make ", "create ", "build ", "track ", "organize ", "plan ", "keep ", "log ", "a ", "an ", "the ", "my "];
const DEVICE: &[&str] = &["スマホ", "スマートフォン", "iphone", "android", "アンドロイド", "端末", "タブレット", "ipad", "phone"];
const MIRROR: &[&str] = &["画面", "操作", "ミラー", "映し", "映す", "mirror", "screen", "remote", "リモート"];
const AUDIO: &[&str] = &["録音", "鼻歌", "ボイスメモ", "音声メモ", "作曲", "音楽制作", "曲作り", "曲を作", "音楽を作", "宅録", "record audio", "voice memo"];
const VIDEO: &[&str] = &["動画", "映像", "ビデオ", "クリップ", "video", "footage"];
const VIDEO_EDIT: &[&str] = &["編集", "カット", "切り", "作りたい", "作って", "edit"];
const MAIL: &[&str] = &["メール", "mail", "inbox", "受信箱"];
const CONVERSATION: &[&str] = &["チャット", "メッセージ", "メール", "返信", "受信箱", "dm", "chat", "message", "mail", "inbox"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Messages,
    Mail,
    Calendar,
    Issues,
    Files,
    Music,
    Daw,
    Video,
    Social,
    Web,
}

struct Service {
    key: &'static str,
    provider: &'static str,
    terms: &'static [&'static str],
    family: Family,
}

/// 名指しされた時だけ provider に残るサービス（表に無い物は候補にも出さない = 捏造しない）。
const SERVICES: &[Service] = &[
    Service { key: "slack", provider: "slack", terms: &["slack", "スラック"], family: Family::Messages },
    Service { key: "discord", provider: "discord", terms: &["discord", "ディスコード"], family: Family::Messages },
    Service { key: "line", provider: "line", terms: &["line", "ライン"], family: Family::Messages },
    Service { key: "teams", provider: "microsoft", terms: &["teams"], family: Family::Messages },
    Service { key: "chatwork", provider: "chatwork", terms: &["chatwork", "チャットワーク"], family: Family::Messages },
    Service { key: "gmail", provider: "google", terms: &["gmail", "gメール"], family: Family::Mail },
    Service { key: "outlook", provider: "microsoft", terms: &["outlook"], family: Family::Mail },
    Service { key: "google_calendar", provider: "google", terms: &["google calendar", "googleカレンダー", "google カレンダー", "gcal"], family: Family::Calendar },
    Service { key: "github", provider: "github", terms: &["github", "ギットハブ"], family: Family::Issues },
    Service { key: "jira", provider: "atlassian", terms: &["jira"], family: Family::Issues },
    Service { key: "trello", provider: "atlassian", terms: &["trello"], family: Family::Issues },
    Service { key: "asana", provider: "asana", terms: &["asana"], family: Family::Issues },
    Service { key: "linear", provider: "linear", terms: &["linear"], family: Family::Issues },
    Service { key: "notion", provider: "notion", terms: &["notion", "ノーション"], family: Family::Files },
    Service { key: "google_drive", provider: "google", terms: &["google drive", "googleドライブ", "グーグルドライブ"], family: Family::Files },
    Service { key: "dropbox", provider: "dropbox", terms: &["dropbox"], family: Family::Files },
    Service { key: "apple_music", provider: "apple", terms: &["apple music", "applemusic", "アップルミュージック"], family: Family::Music },
    Service { key: "spotify", provider: "spotify", terms: &["spotify"], family: Family::Music },
    Service { key: "youtube", provider: "youtube", terms: &["youtube", "yotube", "ユーチューブ"], family: Family::Video },
    Service { key: "reaper", provider: "reaper", terms: &["reaper"], family: Family::Daw },
    Service { key: "linkedin", provider: "linkedin", terms: &["linkedin"], family: Family::Social },
    Service { key: "claude_ai", provider: "anthropic", terms: &["claude.ai"], family: Family::Web },
    Service { key: "chatgpt", provider: "openai", terms: &["chatgpt"], family: Family::Web },
];

/// ASCII の語は語境界つき（"line" が "deadline" に、"dm" が "admin" に当たらない）。日本語は contains。
fn find_term(text: &str, term: &str, last: bool) -> Option<usize> {
    if !term.is_ascii() {
        return if last { text.rfind(term) } else { text.find(term) };
    }
    let b = text.as_bytes();
    let mut hits = text.match_indices(term).map(|(i, _)| i).filter(|&i| {
        let before = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let end = i + term.len();
        let after = end >= b.len() || !b[end].is_ascii_alphanumeric() || term.ends_with(' ');
        before && after
    });
    if last {
        hits.last()
    } else {
        hits.next()
    }
}

fn has_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|t| find_term(text, t, false).is_some())
}

/// 変更の語が在るか。「できるように / 可能に」は job の語の直後なら数えない（「並び替え可能に」「検索できるように」は変更のまま）。
fn asks_edit(text: &str) -> bool {
    let after_job = |i: usize| JOB_WORDS.iter().any(|(_, terms)| terms.iter().any(|t| text[..i].ends_with(t)));
    EDIT_WORDS.iter().any(|w| if MODAL_EDIT.contains(w) { text.match_indices(w).any(|(i, _)| !after_job(i)) } else { find_term(text, w, false).is_some() })
}

/// 物の性質。複合語は後ろが主なので、当たった term の終わりが一番後ろの物を採る。返すのは (物, 当たった語)。
fn find_thing(text: &str) -> Option<(&'static Thing, String)> {
    let mut best: Option<(usize, &'static Thing, &'static str)> = None;
    for thing in THINGS {
        for term in thing.terms {
            if let Some(pos) = find_term(text, term, true) {
                let end = pos + term.len();
                if best.map(|(e, _, t)| end > e || (end == e && term.len() > t.len())).unwrap_or(true) {
                    best = Some((end, thing, term));
                }
            }
        }
    }
    best.map(|(_, t, term)| (t, term.to_string()))
}

fn last_job_word(text: &str) -> Option<(Job, &'static str)> {
    let mut best: Option<(usize, Job, &'static str)> = None;
    for (job, terms) in JOB_WORDS {
        for term in *terms {
            if let Some(pos) = find_term(text, term, true) {
                if best.map(|(p, _, _)| pos > p).unwrap_or(true) {
                    best = Some((pos, *job, term));
                }
            }
        }
    }
    best.map(|(_, j, t)| (j, t))
}

/// 接続が前提の領域は述語より強い（「チャットをまとめたい」は会話の app で、一覧の app ではない）。
fn special_domain(text: &str) -> Option<(Job, &'static str)> {
    if has_any(text, DEVICE) && has_any(text, MIRROR) {
        return Some((Job::Mirror, "端末 + 画面 / 操作"));
    }
    if has_any(text, AUDIO) {
        return Some((Job::Record, "録音 / 作曲"));
    }
    if has_any(text, VIDEO) && has_any(text, VIDEO_EDIT) {
        return Some((Job::EditMedia, "動画 + 編集"));
    }
    if has_any(text, CONVERSATION) {
        return Some((Job::Communicate, "会話 / メール"));
    }
    None
}

fn cadence_of(text: &str) -> Option<Cadence> {
    CADENCE.iter().find(|(_, terms)| has_any(text, terms)).map(|(c, _)| *c)
}

/// 利用者の名詞句: 「を」の前（無ければ最初の助詞 / 述語の前）。頻度の前置きと器の語（アプリ）を落とす。
/// English（ASCII）は利用者の書いた大文字小文字のまま（DOGFOOD-X 周 7 #4: 名前が一言を小文字にした全文だった）。
/// 6 語を超える文は最初の句（`,` `;` ` and ` ` then ` の前）だけ・60 文字まで（語の切れ目で `…`）= 名前に一言の全文を使わない。
fn subject_of(literal: &str) -> String {
    let t = literal.trim().trim_end_matches(['。', '.', '!', '！', '?', '？']);
    let mut s = match t.find('を') {
        Some(i) => t[..i].to_string(),
        None if t.is_ascii() => {
            // ASCII なので小文字にしても byte の位置は同じ = 頭の語は小文字で比べて、利用者の書いた形から切る
            let mut rest = t;
            while let Some(p) = EN_LEAD.iter().find(|p| rest.to_lowercase().starts_with(*p)) {
                rest = &rest[p.len()..];
            }
            short_name(rest)
        }
        None => {
            let cut = ["が", "は", "に", "で", "作っ", "作り", "作る", "して", "したい"]
                .iter()
                .filter_map(|m| t.find(m).filter(|&i| i > 0))
                .min()
                .unwrap_or(t.len());
            t[..cut].to_string()
        }
    };
    for p in CADENCE_PREFIXES {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.to_string();
            break;
        }
    }
    for suf in CONTAINER_SUFFIXES {
        if let Some(rest) = s.strip_suffix(suf) {
            s = rest.to_string();
            break;
        }
    }
    s.trim().trim_matches(['、', ',', ' ', '「', '」']).to_string()
}

/// 周 7 #4: 長い English の文（6 語を超える）を名前の長さに。最初の句だけ・60 文字まで（語の切れ目で `…`）。短い文はそのまま。
fn short_name(s: &str) -> String {
    let s = s.trim();
    if s.split_whitespace().count() <= 6 {
        return s.to_owned();
    }
    let lower = s.to_lowercase();
    let cut = [",", ";", " and ", " then "].iter().filter_map(|m| lower.find(m)).filter(|&i| i > 0).min().unwrap_or(s.len());
    let clause = s[..cut].trim();
    if clause.len() <= 60 {
        return clause.to_owned();
    }
    let end = clause[..60].rfind(' ').unwrap_or(60);
    format!("{}…", clause[..end].trim_end())
}

fn is_vague(subject: &str) -> bool {
    let s = subject.trim().to_lowercase();
    s.is_empty() || VAGUE.contains(&s.as_str()) || ["この", "その", "あの", "これ", "それ"].iter().any(|p| s.starts_with(p))
}

const LOG_FIELDS: &[F] = &[fld("date", "Date", Date), head("title", "Entry"), fld("memo", "Note", Text)];
const PLAN_FIELDS: &[F] = &[fld("date", "Date", Date), head("title", "Plan"), fld("memo", "Note", Text)];
const CARD_FIELDS: &[F] = &[head("prompt", "Question"), fld("answer", "Answer", Text), flag("done", "Learned")];
const RECORD_FIELDS: &[F] = &[fld("date", "Date", Date), fld("category", "Category", Text), fld("value", "Value", Number), fld("memo", "Note", Text)];
const MONITOR_FIELDS: &[F] = &[head("title", "Title"), pick("status", "Status", &["Not started", "In progress", "Done"], true), stamp("updated_at", "Updated")];
const ITEM_FIELDS: &[F] = &[head("name", "Name"), long("memo", "Note"), stamp("updated_at", "Updated")];

/// 物の表に無い時の、job の既定の欄。
fn generic_fields(job: Job) -> (&'static str, &'static [F]) {
    match job {
        Job::Log => ("entries", LOG_FIELDS),
        Job::Plan => ("plans", PLAN_FIELDS),
        Job::Memorize => ("cards", CARD_FIELDS),
        Job::Visualize => ("records", RECORD_FIELDS),
        Job::Monitor => ("items", MONITOR_FIELDS),
        Job::Author => ("notes", NOTE_FIELDS),
        Job::Measure => ("timer", TIMER_FIELDS),
        _ => ("items", ITEM_FIELDS),
    }
}

/// job が要る欄を足す（物の表の欄を job に合わせる。例: 見える化には数と日付、把握には状態）。
fn fit_fields(job: Job, fields: &mut Vec<FieldSpec>) {
    let has = |fields: &Vec<FieldSpec>, p: &dyn Fn(&FieldSpec) -> bool| fields.iter().any(p);
    match job {
        Job::Log | Job::Plan => {
            if !has(fields, &|f| f.is_date()) {
                fields.insert(0, fld("date", "Date", Date).spec());
            }
        }
        Job::Visualize => {
            if !has(fields, &|f| f.is_date()) {
                fields.insert(0, fld("date", "Date", Date).spec());
            }
            if !has(fields, &|f| f.is_numeric()) {
                fields.push(fld("value", "Value", Number).spec());
            }
        }
        Job::Memorize => {
            if !has(fields, &|f| !f.title && matches!(f.kind, FieldKind::Text | FieldKind::Multiline)) {
                fields.push(fld("answer", "Answer", Text).spec());
            }
            if !has(fields, &|f| f.kind == FieldKind::Boolean) {
                fields.push(flag("done", "Learned").spec());
            }
        }
        Job::Monitor => {
            if !has(fields, &|f| f.kind == FieldKind::Select && f.mutable) {
                fields.push(pick("status", "Status", &["Not started", "In progress", "Done"], true).spec());
            }
        }
        Job::Author => {
            if !has(fields, &|f| f.kind == FieldKind::Multiline) {
                fields.push(long("body", "Body").spec());
            }
        }
        _ => {}
    }
}

fn input(fields: &[F]) -> Vec<FieldSpec> {
    fields.iter().map(|f| f.spec()).collect()
}

struct Cap {
    suffix: &'static str,
    label: &'static str,
    effect: Effect,
    approval: bool,
    role: ExternalRole,
    input: &'static [F],
}

const ACCOUNT: F = fld("account_id", "Account", Text);

const MESSAGES_CAPS: &[Cap] = &[
    Cap { suffix: "messages.read", label: "会話を読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Threads, input: &[ACCOUNT, fld("since", "Since", Text)] },
    Cap { suffix: "messages.send", label: "確認して送る", effect: Effect::ExternalSend, approval: true, role: ExternalRole::Send, input: &[ACCOUNT, fld("thread_id", "Conversation", Text), fld("draft_id", "Draft", Text)] },
];
const MAIL_CAPS: &[Cap] = &[
    Cap { suffix: "mail.read", label: "メールを読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Threads, input: &[ACCOUNT, fld("query", "Search", Text)] },
    Cap { suffix: "mail.send", label: "確認して送る", effect: Effect::ExternalSend, approval: true, role: ExternalRole::Send, input: &[ACCOUNT, fld("thread_id", "Thread", Text), fld("draft_id", "Draft", Text)] },
];
const CALENDAR_CAPS: &[Cap] = &[
    Cap { suffix: "events.read", label: "予定を読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[ACCOUNT, fld("from", "Since", Text)] },
    Cap { suffix: "events.create", label: "予定を入れる", effect: Effect::ExternalWrite, approval: true, role: ExternalRole::Extra, input: &[ACCOUNT, fld("title", "Title", Text), fld("start", "Starts", Text)] },
];
const ISSUES_CAPS: &[Cap] = &[Cap { suffix: "issues.read", label: "Issue を読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[fld("repository", "Repository", Text)] }];
const FILES_CAPS: &[Cap] = &[Cap { suffix: "files.read", label: "ファイルを読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[fld("folder", "Place", Text)] }];
const MUSIC_CAPS: &[Cap] = &[
    Cap { suffix: "library.read", label: "ライブラリを読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[] },
    Cap { suffix: "playback", label: "再生する", effect: Effect::ExternalWrite, approval: false, role: ExternalRole::Extra, input: &[fld("track_id", "Track", Text)] },
];
const DAW_CAPS: &[Cap] = &[Cap { suffix: "insert_media", label: "録音を入れる", effect: Effect::ExternalWrite, approval: true, role: ExternalRole::Extra, input: &[fld("recording_id", "Recording", Text)] }];
const VIDEO_CAPS: &[Cap] = &[Cap { suffix: "search", label: "検索する", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[fld("query", "Search", Text)] }];
const SOCIAL_CAPS: &[Cap] = &[Cap { suffix: "connections.read", label: "つながりを読み込む", effect: Effect::ExternalRead, approval: false, role: ExternalRole::Extra, input: &[] }];
const WEB_CAPS: &[Cap] = &[Cap { suffix: "open", label: "開く", effect: Effect::LocalWrite, approval: false, role: ExternalRole::Extra, input: &[] }];

fn family_caps(f: Family) -> &'static [Cap] {
    match f {
        Family::Messages => MESSAGES_CAPS,
        Family::Mail => MAIL_CAPS,
        Family::Calendar => CALENDAR_CAPS,
        Family::Issues => ISSUES_CAPS,
        Family::Files => FILES_CAPS,
        Family::Music => MUSIC_CAPS,
        Family::Daw => DAW_CAPS,
        Family::Video => VIDEO_CAPS,
        Family::Social => SOCIAL_CAPS,
        Family::Web => WEB_CAPS,
    }
}

fn source_kind(f: Family) -> &'static str {
    match f {
        Family::Music | Family::Daw => "os_automation",
        Family::Web => "public_web",
        _ => "external_api",
    }
}

fn named_externals(text: &str, literal: &str) -> Vec<ExternalNeed> {
    let mut out = Vec::new();
    for s in SERVICES {
        let Some((pos, term)) = s.terms.iter().find_map(|t| find_term(text, t, false).map(|p| (p, *t))) else { continue };
        // 利用者が書いた形のまま label にする（validate_plan の user_named 規則: label は prompt の中に在る）
        let named: String = literal.chars().skip(text[..pos].chars().count()).take(term.chars().count()).collect();
        for cap in family_caps(s.family) {
            let web = s.family == Family::Web;
            out.push(ExternalNeed {
                action: format!("{}.{}", s.key, cap.suffix),
                capability: if web { "web.handoff".into() } else { format!("{}.{}", s.key, cap.suffix) },
                label: cap.label.into(),
                effect: cap.effect,
                approval: cap.approval,
                role: cap.role,
                source: (!web).then(|| s.key.to_string()),
                source_kind: source_kind(s.family).into(),
                source_label: named.clone(),
                provider: Some(s.provider.into()),
                adapter: Some(if web { "handoff".into() } else if s.family == Family::Music || s.family == Family::Daw { "os_automation".into() } else { "api".into() }),
                named: Some(named.clone()),
                necessity: Necessity::Essential,
                input: input(cap.input),
            });
        }
    }
    out
}

fn external(action: &str, capability: &str, label: &str, effect: Effect, role: ExternalRole, source: Option<&str>, source_label: &str, necessity: Necessity, fields: &[F]) -> ExternalNeed {
    ExternalNeed {
        action: action.into(),
        capability: capability.into(),
        label: label.into(),
        effect,
        approval: matches!(effect, Effect::ExternalSend | Effect::Destructive) || (effect == Effect::ExternalWrite && role != ExternalRole::Extra),
        role,
        source: source.map(Into::into),
        source_kind: if source.map(|s| s.starts_with("optional.")).unwrap_or(false) { "external_optional".into() } else { "local_device".into() },
        source_label: source_label.into(),
        provider: None,
        adapter: None,
        named: None,
        necessity,
        input: input(fields),
    }
}

fn entity(id: &str, label: &str, kind: EntityKind, fields: &[F], picker: bool) -> Entity {
    Entity { id: id.into(), label: label.into(), kind, fields: input(fields), bulk: false, picker }
}

/// 器（候補の一覧）は接続が要る物が在る時だけ置く。
fn candidates() -> Entity {
    entity("connection_candidates", "繋げる候補", EntityKind::Candidates, &[head("name", "Name")], false)
}

/// 「便利にして」のような目的そのものが読めない文: blocking 1 問 ＋ 候補 3 つ（CD-05 規則 6）。
fn unclear_plan(literal: &str) -> GoalPlan {
    let intent = |id: &str, purpose: &str, primary: bool, hint: &str| UiIntent { id: id.into(), purpose: purpose.into(), primary, representation_hint: Some(hint.into()) };
    GoalPlan {
        schema_version: PlanSchemaVersion::V1,
        id: "unclear_goal".into(),
        prompt: literal.into(),
        goal: PlanGoal { objective: "何を作るかを 1 つに決める".into(), success_signal: "作る物が 1 文で言える".into() },
        requirements: vec![Requirement { id: "objective".into(), statement: literal.into(), origin: Origin::Explicit, necessity: Necessity::Essential, confidence: 0.2 }],
        needs: vec![Need { id: "objective".into(), description: "目的（何を・どう扱うか）".into(), effect: NeedEffect::Pure, status: NeedStatus::Proposed }],
        // local_only は question 0 が条件（CD-05 規則 3）。何を繋ぐかもまだ決まっていない
        source_policy: SourcePolicy::OptionalConnections,
        candidate_sources: vec![],
        ui_intents: vec![
            intent("ask_objective", "何を作りたいかを 1 行で聞く", true, "question"),
            intent("suggest_list", "例: 何かの一覧を作って探す（在庫・やる事・相手）", false, "list"),
            intent("suggest_log", "例: 毎日の記録をつける（体重・読書・支出）", false, "timeline"),
        ],
        questions: vec![Question {
            id: "objective".into(),
            text: "何を作りたいですか？（例: 在庫を整理したい・読書記録をつけたい・タイマーを作って）".into(),
            blocking_need_id: "objective".into(),
            reason: QuestionReason::IrreduciblePreference,
            suggested_default: None,
        }],
        verification_plan: vec!["目的が決まるまで app を作らない（質問は 1 つ）".into()],
    }
}

/// 1 文字ずつ小文字にする。小文字が 2 文字以上になる文字（'İ' は "i\u{307}"）は元のまま: text と literal の文字位置を 1 対 1 に保ち、
/// text で見つけた位置で literal から利用者の書いた形を切り出せるようにする（C-R3-01）。
fn lower_1to1(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// 唯一の入口。固定の panel を選ばず、job × 物 × 修飾 を返す。
pub fn interpret(prompt: &str) -> Interpretation {
    let literal = prompt.trim().to_string();
    if literal.is_empty() {
        return Interpretation::Empty;
    }
    let text = lower_1to1(&literal);
    let subject = subject_of(&literal);
    let thing = find_thing(&subject.to_lowercase()).or_else(|| find_thing(&text));
    let domain = special_domain(&text);
    let verb = last_job_word(&text);
    // 名詞句が名指しのサービスだけ（「Slack と GitHub をまとめたい」）→ 物はサービスの data そのもの。会話が在れば会話、無ければ把握
    let named_only = {
        let named = named_externals(&text, &literal);
        let mut rest = subject.to_lowercase();
        for n in named.iter().filter_map(|x| x.named.as_ref()) {
            rest = rest.replace(&n.to_lowercase(), "");
        }
        let rest = rest.replace("and", "");
        (!named.is_empty() && rest.chars().all(|c| c.is_whitespace() || "と、,&や・+".contains(c)))
            .then(|| if named.iter().any(|x| x.role == ExternalRole::Threads) { Job::Communicate } else { Job::Monitor })
    };
    let thing = if named_only.is_some() { None } else { thing };
    // 物も領域も無く変更の語だけ在る文（「一覧に検索を追加して」）は、新しい app ではなく今の app への編集
    if thing.is_none() && domain.is_none() && asks_edit(&text) {
        return Interpretation::Edit;
    }
    let mut evidence = Vec::new();
    let job = if let Some((j, why)) = domain {
        evidence.push(format!("接続が前提の領域: {why}"));
        j
    } else if let Some(j) = named_only {
        evidence.push("名指しのサービスだけをまとめる".into());
        j
    } else if let Some((j, w)) = verb {
        evidence.push(format!("述語「{w}」"));
        j
    } else if let Some((t, w)) = &thing {
        evidence.push(format!("物「{w}」の既定"));
        t.job
    } else if !is_vague(&subject) {
        evidence.push(format!("物「{subject}」は表に無い → 一覧の既定"));
        Job::Organize
    } else {
        return Interpretation::Unclear(Box::new(unclear_plan(&literal)));
    };
    let cadence = cadence_of(&text);
    let research = has_any(&text, RESEARCH);
    let checklist = has_any(&text, CHECKLIST) || thing.as_ref().map(|(t, _)| t.checklist).unwrap_or(false);
    let title = if is_vague(&subject) { thing.as_ref().map(|(_, w)| w.clone()).unwrap_or_else(|| literal.chars().take(16).collect()) } else { subject.clone() };
    // 物の label: 当たった語を利用者の書いた形で（無ければ名詞句から job の名詞を落とした物）
    let word = match &thing {
        Some((_, w)) => text.find(w.as_str()).map(|p| literal.chars().skip(text[..p].chars().count()).take(w.chars().count()).collect()).unwrap_or_else(|| w.clone()),
        None => {
            let mut s = subject.clone();
            for suf in ["の記録", "記録", "の計画", "計画", "の状況", "状況", "の管理", "管理", "の一覧", "一覧", "の見える化"] {
                if let Some(rest) = s.strip_suffix(suf) {
                    s = rest.to_string();
                    break;
                }
            }
            if is_vague(&s) || named_only.is_some() {
                "項目".into()
            } else {
                s
            }
        }
    };

    let mut entities = Vec::new();
    let mut externals = named_externals(&text, &literal);
    match job {
        Job::Communicate => {
            if !externals.iter().any(|x| x.role == ExternalRole::Threads) {
                let mail = has_any(&text, MAIL);
                for cap in family_caps(if mail { Family::Mail } else { Family::Messages }) {
                    let essential = cap.role == ExternalRole::Threads;
                    let mut x = external(cap.suffix, cap.suffix, cap.label, cap.effect, cap.role, Some("optional.accounts"), "繋いだアカウント", if essential { Necessity::Essential } else { Necessity::Useful }, cap.input);
                    x.approval = cap.approval;
                    externals.push(x);
                }
            }
            entities.push(entity("drafts", "下書き", EntityKind::Drafts, &[fld("thread_id", "Conversation", Text), long("body", "Body")], false));
        }
        Job::Mirror => {
            externals.push(external("device.screen.read", "device.bridge", "画面を映す", Effect::ExternalRead, ExternalRole::Screen, Some("optional.device"), "端末", Necessity::Essential, &[fld("device_id", "Device", Text)]));
            externals.push(external("device.input.send", "device.bridge", "入力を送る", Effect::ExternalWrite, ExternalRole::Input, None, "端末", Necessity::Useful, &[fld("device_id", "Device", Text), fld("input", "Input", Text)]));
            entities.push(entity("pairing_methods", "繋ぎ方の候補", EntityKind::Pairing, &[head("name", "Pairing")], false));
        }
        Job::EditMedia => {
            entities.push(entity("clips", "素材", EntityKind::Clips, &[head("name", "Name"), fld("duration_seconds", "Length (s)", Integer), stamp("added_at", "Added")], true));
            entities.push(entity("markers", "切る位置", EntityKind::Markers, &[fld("clip_id", "Clip", Text), fld("position_seconds", "Position (s)", Integer), head("label", "Note")], false));
            externals.push(external("editor.open", "editor.open", "editor で開く", Effect::LocalWrite, ExternalRole::Editor, None, "editor", Necessity::Useful, &[fld("clip_id", "Clip", Text)]));
        }
        Job::Record => {
            entities.push(entity("recordings", "録音", EntityKind::Recordings, &[head("name", "Name"), fld("duration_seconds", "Length (s)", Integer), stamp("recorded_at", "Recorded")], false));
            let mut mic = external("local.audio.record", "local.microphone", "録音する", Effect::LocalWrite, ExternalRole::Mic, None, "mic", Necessity::Essential, &[fld("max_seconds", "Max length (s)", Integer)]);
            mic.adapter = Some("local_device".into());
            externals.push(mic);
        }
        _ => {
            let (id, fields) = match (&thing, job) {
                (_, Job::Measure) => ("timer", TIMER_FIELDS),
                (Some((t, _)), _) => (t.id, t.fields),
                (None, j) => generic_fields(j),
            };
            let mut fields = input(fields);
            if id == "measurements" {
                // 「体重を見える化」の値は「体重」と呼ぶ
                fields.iter_mut().filter(|f| f.name == "value").for_each(|f| f.label = word.clone());
            }
            if job != Job::Measure {
                fit_fields(job, &mut fields);
            }
            let (bulk, picker) = thing.as_ref().map(|(t, _)| (t.bulk, t.picker)).unwrap_or((false, false));
            entities.push(Entity { id: id.into(), label: word.clone(), kind: EntityKind::Primary, fields, bulk: bulk && job != Job::Measure, picker });
            if job == Job::Author && research {
                entities.push(entity("imports", "取り込んだ資料", EntityKind::Imports, &[head("name", "File name"), stamp("imported_at", "Imported")], true));
            }
            if job == Job::Plan && checklist {
                entities.push(entity("checklist", "持ち物・準備", EntityKind::Checklist, &[head("title", "Item"), flag("done", "Done")], false));
            }
            if job == Job::Monitor && !externals.iter().any(|x| x.named.is_some()) {
                externals.push(external(&format!("{id}.read"), &format!("{id}.read"), "取得元から読み込む", Effect::ExternalRead, ExternalRole::MonitorRead, Some(&format!("optional.{id}")), &format!("取得元の{word}"), Necessity::Optional, &[]));
            }
        }
    }
    if has_any(&text, DEVICE) && job != Job::Mirror {
        externals.push(external("device.files.read", "device.bridge", "端末から読み込む", Effect::ExternalRead, ExternalRole::Extra, Some("optional.device"), "端末", Necessity::Optional, &[fld("device_id", "Device", Text)]));
    }
    // 候補の一覧は「繋ぐ先を選ぶ」必要が在る時だけ（Mirror は繋ぎ方の候補を自分で持つ）
    if job != Job::Mirror && externals.iter().any(|x| x.named.is_some() || x.source.as_deref().is_some_and(|s| s.starts_with("optional."))) {
        entities.push(candidates());
    }

    // ---- 言葉（outcome / objective / 前提 / 制約 / 不明点）----
    let primary = entities.iter().find(|e| e.kind == EntityKind::Primary);
    let label = word.as_str();
    let mut outcomes: Vec<String> = Vec::new();
    let objective = match job {
        Job::Measure => {
            outcomes.extend(["開始・一時停止・リセットができる".into(), "設定した時間（分）が、閉じて開き直しても残る".into()]);
            "指定した時間を計って知らせる".to_string()
        }
        Job::Organize => {
            outcomes.push(format!("{label}を一覧で見て、絞り込める"));
            if let Some(f) = primary.and_then(|e| e.fields.iter().find(|f| f.mutable && f.kind != FieldKind::Multiline)) {
                outcomes.push(format!("{}をその場で変えられる", f.label));
            }
            outcomes.push("閉じて開き直しても残る".into());
            if primary.map(|e| e.bulk).unwrap_or(false) {
                outcomes.push("手元の CSV から読み込める".into());
            }
            format!("{label}の確認と更新を簡単にする")
        }
        Job::Log => {
            outcomes.push(if title.contains("記録") { format!("{title}をその場でつけられる") } else { format!("{title}をその場で記録できる") });
            outcomes.push("日付ごとに振り返れる".into());
            outcomes.push("閉じて開き直しても残る".into());
            format!("{label}を続けて記録し、あとから振り返る")
        }
        Job::Visualize => {
            let by = primary.and_then(|e| e.category_field()).map(|f| f.label.clone()).unwrap_or_else(|| "Date".into());
            outcomes.push(format!("{label}を記録すると、{by}ごとの合計がグラフで見える"));
            outcomes.push("同じ数字を表でも確かめられる（グラフだけに頼らない）".into());
            outcomes.push("閉じて開き直しても残る".into());
            format!("{label}の流れをひと目で分かるようにする")
        }
        Job::Plan => {
            outcomes.push("日付ごとの予定を並べて見られる".into());
            if checklist {
                outcomes.push("持ち物・準備を確かめて、済んだ物に印を付けられる".into());
            }
            outcomes.push("閉じて開き直しても残る".into());
            format!("{title}を日付の上で組み立てる")
        }
        Job::Memorize => {
            outcomes.push(format!("{label}を足して、1 枚ずつ復習できる"));
            outcomes.push("覚えた物は復習から外れる".into());
            if cadence.is_some() {
                outcomes.push("開いた時に今日の残りが分かる".into());
            }
            format!("{label}を少しずつ確実に覚える")
        }
        Job::Monitor => {
            outcomes.push(format!("{label}の状態を 1 画面で見られる"));
            outcomes.push("取得元を繋いだ時だけ、その一覧が加わる（今は何も読まない）".into());
            format!("{label}の状況を 1 画面で把握する")
        }
        Job::Author => {
            outcomes.push(format!("{label}を書いて、一覧と検索ができる"));
            if research {
                outcomes.push("取り込んだ資料に出典（file 名・取り込み日時）が残り、引用から辿れる".into());
            }
            format!("{label}を書いて探せるように残す")
        }
        Job::Communicate => {
            outcomes.push("繋いだアカウントの会話が 1 つの一覧に、出所の印つきで並ぶ".into());
            outcomes.push("返信の下書きは端末内に残り、送るのは宛先と本文を確認した後".into());
            "選んだアカウントの会話を 1 つの画面で読む".to_string()
        }
        Job::Mirror => {
            outcomes.push("繋いだ端末の画面が見える".into());
            outcomes.push("入力の送信は 1 回ずつ承認する".into());
            "端末の画面を手元で見て操作する".to_string()
        }
        Job::EditMedia => {
            outcomes.push("素材の一覧と切る位置が手元に残る".into());
            outcomes.push("編集そのものは選んだ editor で行う".into());
            "素材と編集の手順を 1 つにまとめる".to_string()
        }
        Job::Record => {
            outcomes.push("録音して、波形で見て、手元に残せる".into());
            "音を録って手元に残す".to_string()
        }
    };
    let mut assumptions = Vec::new();
    let mut constraints = vec!["利用者の承認なく外部のデータを読まない・送らない".to_string()];
    let mut unknowns = Vec::new();
    if externals.is_empty() {
        assumptions.push("端末内だけで動く。外部アカウント・スマホ・web login は使わない".into());
    }
    if externals.iter().any(|x| x.named.is_none() && x.source.as_deref().map(|s| s.starts_with("optional.")).unwrap_or(false)) {
        assumptions.push("取得元は未選択。候補は Connections に並ぶ。中身は繋ぐまで読まない".into());
        unknowns.push("どの取得元・アカウントを繋ぐか（未選択）".into());
    }
    if let Some(c) = cadence {
        assumptions.push(format!("{}開いた時に今日の分が出る（通知は出さない）", match c { Cadence::Daily => "毎日", Cadence::Weekly => "毎週", Cadence::Monthly => "毎月" }));
    }
    let known_thing = thing.is_some() || matches!(job, Job::Measure | Job::Communicate | Job::Mirror | Job::EditMedia | Job::Record);
    if !known_thing {
        if let Some(e) = primary {
            let names: Vec<&str> = e.fields.iter().map(|f| f.label.as_str()).collect();
            unknowns.push(format!("「{label}」の欄は推定（{}）。足りなければ編集で足す", names.join("・")));
        }
    }
    if primary.map(|e| matches!(job, Job::Organize | Job::Log | Job::Visualize | Job::Plan | Job::Memorize | Job::Monitor | Job::Author) && e.kind == EntityKind::Primary).unwrap_or(false) {
        constraints.push("削除は確認の後（DESIGN §4.1）".into());
    }
    if externals.iter().any(|x| x.effect == Effect::ExternalSend) {
        constraints.push("送信は 1 件ずつ承認する。一括送信・自動送信をしない".into());
    }
    if job == Job::Visualize {
        constraints.push("グラフだけに頼らず、同じ数字を表でも出す（DESIGN §9）".into());
    }
    if research {
        constraints.push("出典の無い引用を作らない".into());
    }
    if job == Job::Record {
        assumptions.push("mic は OS の許可を得てから使う。参考曲の音声は取り込まない".into());
    }
    if job == Job::EditMedia {
        assumptions.push("Shibubu は editor を作らない。編集は選んだ editor で行う（DESIGN §12.2）".into());
    }
    let confidence = match (domain.is_some(), verb.is_some(), thing.is_some()) {
        (true, _, _) | (_, true, true) => 0.9,
        (false, false, true) => 0.75,
        (false, true, false) => 0.6,
        _ => 0.4,
    };
    if let Some((_, w)) = &thing {
        evidence.push(format!("物「{w}」"));
    }
    Interpretation::Goal(Box::new(Reading {
        literal,
        subject,
        title,
        job,
        cadence,
        research: research && job == Job::Author,
        known_thing,
        entities,
        externals,
        objective,
        outcomes,
        actors: vec!["利用者".into()],
        constraints,
        assumptions,
        unknowns,
        confidence,
        evidence,
    }))
}
