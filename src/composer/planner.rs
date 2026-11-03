//! Planner（GEN-002 / GEN-004 / GEN-005 / GEN-007 / GEN-008）: UI より先に data と action を決める。
//!
//! 1. 解を 2〜3 個作り、utility / risk / 接続の手間 / 待ち時間 / 戻せる度合い で点を付けて選ぶ（落とした案と理由も残す）。
//! 2. 選んだ解で entity → source（`local.<id>`）、verb → local action（`local.<source>.<verb>`。verb は capabilities::LOCAL_VERBS だけ）、
//!    接続が要る能力 → capability（discover の readiness のまま）＋ action（ready でなければ requires_connection）。
//!    local の collection には画面に出すかどうかに関わらず filter / search を宣言する（agent も同じ action で探せる・後から「検索を追加」できる）。
//! 3. GoalPlan（goal_plan_v1）を書く。質問は「無いと使えない・危ない」時だけ 1 つまで（CD-05）。local で済む目的は 0 問。
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::capabilities::{discover, local_verb, Discovery};
use super::domain::action::{ActionDefinition, ActionReadiness, Effect};
use super::domain::app::Workflow;
use super::domain::binding::{Source, SourceReadiness};
use super::domain::capability::{AdapterKind, CapabilityDescriptor, CapabilityReadiness};
use super::goal::{
    CandidateSource, Entity, EntityKind, ExternalNeed, ExternalRole, FieldKind, FieldSpec, GoalPlan, Job, Necessity, Need, NeedEffect, NeedStatus, Origin, PlanGoal,
    PlanSchemaVersion, Question, QuestionReason, Reading, Requirement, SourceEvidence, SourcePolicy, UiIntent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// 端末内で全部動かし、接続は後から足せる候補として置く
    LocalFirst,
    /// 主な流れだけ（取り込み・副の一覧を置かない）
    Minimal,
    /// 取得元を繋いでから使う（接続が要る物が在る時だけ候補になる）
    ConnectedFirst,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scores {
    pub utility: f64,
    pub risk: f64,
    pub connection_cost: f64,
    pub latency: f64,
    pub reversibility: f64,
}

impl Scores {
    pub fn total(&self) -> f64 {
        self.utility * 0.4 - self.risk * 0.2 - self.connection_cost * 0.15 - self.latency * 0.1 + self.reversibility * 0.15
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Solution {
    pub strategy: Strategy,
    pub summary: String,
    pub scores: Scores,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rejected {
    pub strategy: Strategy,
    pub reason: String,
}

/// GEN-002: 少なくとも 2 案。点の付け方は全 goal で同じ式（Scores::total）。
pub fn solutions(r: &Reading) -> Vec<Solution> {
    let external_core = matches!(r.job, Job::Communicate | Job::Mirror);
    let writes = r.externals.iter().any(|x| matches!(x.effect, Effect::ExternalWrite | Effect::ExternalSend));
    let destructive = r.job != Job::Measure && r.primary().is_some();
    let mut out = vec![
        Solution {
            strategy: Strategy::LocalFirst,
            summary: if r.is_local_only() { "Run everything on this Mac · no connection needed".into() } else { "Make the parts on this Mac usable first and keep what needs a connection as suggestions".into() },
            scores: Scores {
                utility: if external_core { 0.6 } else { 0.6 + 0.3 * r.confidence },
                risk: 0.05 + if destructive { 0.05 } else { 0.0 },
                connection_cost: 0.0,
                latency: 0.1,
                reversibility: 0.9,
            },
            total: 0.0,
        },
        Solution {
            strategy: Strategy::Minimal,
            summary: "Only the main flow (no imports, side lists or suggestions)".into(),
            scores: Scores { utility: 0.55, risk: 0.04, connection_cost: 0.0, latency: 0.05, reversibility: 0.95 },
            total: 0.0,
        },
    ];
    if !r.externals.is_empty() {
        out.push(Solution {
            strategy: Strategy::ConnectedFirst,
            summary: "Connect a source first and make its data the main part (empty until connected)".into(),
            scores: Scores { utility: if external_core { 0.7 } else { 0.4 }, risk: if writes { 0.45 } else { 0.25 }, connection_cost: 0.8, latency: 0.6, reversibility: 0.4 },
            total: 0.0,
        });
    }
    for s in &mut out {
        s.total = (s.scores.total() * 1000.0).round() / 1000.0;
    }
    out
}

pub fn choose(solutions: &[Solution]) -> (usize, Vec<Rejected>) {
    let best = solutions.iter().enumerate().max_by(|a, b| a.1.total.total_cmp(&b.1.total)).map(|(i, _)| i).unwrap_or(0);
    let rejected = solutions
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != best)
        .map(|(_, s)| Rejected {
            strategy: s.strategy,
            reason: match s.strategy {
                Strategy::Minimal => format!("score {:.3} < {:.3}: missing actions would have to be added later (undoable, but extra work)", s.total, solutions[best].total),
                Strategy::ConnectedFirst => format!("score {:.3} < {:.3}: nothing works until it is connected · more risk of writing outside and more waiting", s.total, solutions[best].total),
                Strategy::LocalFirst => format!("score {:.3} < {:.3}", s.total, solutions[best].total),
            },
        })
        .collect();
    (best, rejected)
}

/// local の entity と、それに付いた action（verb → id）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Planned {
    pub entity: Entity,
    pub source: String,
    pub verbs: Vec<(String, String)>,
}

impl Planned {
    pub fn action(&self, verb: &str) -> Option<&str> {
        self.verbs.iter().find(|(v, _)| v == verb).map(|(_, id)| id.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedExternal {
    pub need: ExternalNeed,
    pub readiness: ActionReadiness,
}

/// UI より先に決まる物（GEN-005）。layout と compiler はこれだけを見る。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blueprint {
    pub app_id: String,
    pub strategy: Strategy,
    pub entities: Vec<Planned>,
    pub externals: Vec<PlannedExternal>,
    pub sources: Vec<Source>,
    pub capabilities: Vec<CapabilityDescriptor>,
    pub actions: Vec<ActionDefinition>,
    pub workflows: Vec<Workflow>,
    pub discovery: Vec<Discovery>,
}

impl Blueprint {
    pub fn entity(&self, kind: EntityKind) -> Option<&Planned> {
        self.entities.iter().find(|p| p.entity.kind == kind)
    }
    pub fn action(&self, id: &str) -> Option<&ActionDefinition> {
        self.actions.iter().find(|a| a.id == id)
    }
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }
    pub fn external(&self, role: ExternalRole) -> Option<&PlannedExternal> {
        self.externals.iter().find(|x| x.need.role == role)
    }
    pub fn connection_action(&self) -> Option<&str> {
        self.action("local.connection.open").map(|a| a.id.as_str())
    }
}

fn prop(f: &FieldSpec) -> Value {
    match f.kind {
        FieldKind::Text | FieldKind::Multiline if f.title => json!({ "type": "string", "minLength": 1 }),
        FieldKind::Text | FieldKind::Multiline => json!({ "type": "string" }),
        FieldKind::Select => json!({ "type": "string", "enum": f.options }),
        FieldKind::Integer => json!({ "type": "integer", "minimum": 0 }),
        FieldKind::Number => json!({ "type": "number" }),
        FieldKind::Date => json!({ "type": "string", "format": "date" }),
        FieldKind::DateTime => json!({ "type": "string", "format": "date-time" }),
        FieldKind::Boolean => json!({ "type": "boolean" }),
    }
}

pub fn object_schema(required: &[&str], props: Vec<(String, Value)>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!("object"));
    if !required.is_empty() {
        m.insert("required".into(), json!(required));
    }
    if !props.is_empty() {
        m.insert("properties".into(), Value::Object(props.into_iter().collect()));
    }
    m.insert("additionalProperties".into(), json!(false));
    m
}

const FILE_REF: &str = "利用者が picker で選んだ file の参照 id。path は書かない";

fn item_id() -> (String, Value) {
    ("item_id".into(), json!({ "type": "string" }))
}

/// verb ごとの input_schema（entity の欄から作る）。
fn input_schema(e: &Entity, verb: &str) -> Map<String, Value> {
    let entry: Vec<&FieldSpec> = e.fields.iter().filter(|f| !f.auto && f.kind != FieldKind::Boolean).collect();
    match verb {
        "add" if e.picker => object_schema(&["file_ref"], vec![("file_ref".into(), json!({ "type": "string", "description": FILE_REF }))]),
        "add" => {
            // 必須は見出し（無ければ最初の数: 家計の金額のように「それが無いと記録にならない」欄）
            let req: Vec<&str> = e.title_field().or_else(|| e.numeric_field()).map(|f| vec![f.name.as_str()]).unwrap_or_default();
            object_schema(&req, entry.iter().map(|f| (f.name.clone(), prop(f))).collect())
        }
        "import_csv" => object_schema(&["file_ref"], vec![("file_ref".into(), json!({ "type": "string", "description": FILE_REF }))]),
        "filter" | "search" => object_schema(&[], vec![("query".into(), json!({ "type": "string" }))]),
        "delete" | "toggle_done" => object_schema(&["item_id"], vec![item_id()]),
        "toggle" | "reset" => object_schema(&[], vec![]),
        "save_settings" => object_schema(&["duration_minutes"], vec![("duration_minutes".into(), json!({ "type": "integer", "minimum": 1, "maximum": 1440 }))]),
        "create" => object_schema(&[], e.title_field().map(|f| vec![(f.name.clone(), json!({ "type": "string" }))]).unwrap_or_default()),
        "save" => {
            let body = e.fields.iter().find(|f| f.kind == FieldKind::Multiline).map(|f| f.name.clone()).unwrap_or_else(|| "body".into());
            object_schema(&["item_id", body.as_str()], vec![item_id(), (body.clone(), json!({ "type": "string" }))])
        }
        "cite" => object_schema(&["item_id", "import_id"], vec![item_id(), ("import_id".into(), json!({ "type": "string" }))]),
        v => match v.strip_prefix("update_").and_then(|name| e.field(name)) {
            Some(f) => object_schema(&["item_id", f.name.as_str()], vec![item_id(), (f.name.clone(), prop(f))]),
            None => object_schema(&[], vec![]),
        },
    }
}

/// entity の種類 × job → verb の列（全部 LOCAL_VERBS の中）。
fn verbs_for(e: &Entity, job: Job, research: bool, strategy: Strategy) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut push = |s: &str| v.push(s.to_string());
    match e.kind {
        EntityKind::Primary => match job {
            Job::Measure => ["toggle", "reset", "save_settings"].iter().for_each(|s| push(s)),
            Job::Author => {
                ["search", "create", "save", "delete"].iter().for_each(|s| push(s));
                if research {
                    push("cite");
                }
            }
            _ => {
                push("filter");
                push("add");
                for f in e.fields.iter().filter(|f| f.mutable && f.kind != FieldKind::Boolean) {
                    push(&format!("update_{}", f.name));
                }
                if e.fields.iter().any(|f| f.mutable && f.kind == FieldKind::Boolean) {
                    push("toggle_done");
                }
                push("delete");
                if e.bulk && strategy != Strategy::Minimal {
                    push("import_csv");
                }
            }
        },
        EntityKind::Imports => ["filter", "add"].iter().for_each(|s| push(s)),
        EntityKind::Checklist => ["filter", "add", "toggle_done", "delete"].iter().for_each(|s| push(s)),
        EntityKind::Drafts => push("save"),
        EntityKind::Clips => ["filter", "add", "delete"].iter().for_each(|s| push(s)),
        EntityKind::Markers => ["add", "delete"].iter().for_each(|s| push(s)),
        EntityKind::Recordings => ["filter", "delete"].iter().for_each(|s| push(s)),
        EntityKind::Candidates | EntityKind::Pairing => {}
    }
    v
}

fn local_action(id: String, verb: &str, schema: Map<String, Value>) -> ActionDefinition {
    let lv = local_verb(verb).expect("verbs_for は LOCAL_VERBS の verb だけを出す");
    ActionDefinition {
        id,
        capability_ref: None,
        effect: lv.effect,
        readiness: ActionReadiness::Ready,
        input_schema: schema,
        output_schema: (verb == "import_csv").then(|| {
            let mut m = Map::new();
            m.insert("type".into(), json!("object"));
            m.insert("properties".into(), json!({ "imported": { "type": "integer" }, "skipped": { "type": "integer" } }));
            m
        }),
        required_scopes: vec![],
        verifier: Some("local_state".into()),
        requires_approval: lv.requires_approval,
    }
}

fn action_readiness(c: CapabilityReadiness) -> ActionReadiness {
    match c {
        CapabilityReadiness::Ready => ActionReadiness::Ready,
        CapabilityReadiness::Unsupported => ActionReadiness::Unsupported,
        _ => ActionReadiness::RequiresConnection,
    }
}

fn source_readiness(c: CapabilityReadiness) -> SourceReadiness {
    match c {
        CapabilityReadiness::Ready => SourceReadiness::Ready,
        CapabilityReadiness::ConnectedUnverified => SourceReadiness::ConnectedUnverified,
        CapabilityReadiness::Degraded => SourceReadiness::Degraded,
        CapabilityReadiness::Unsupported => SourceReadiness::Unsupported,
        _ => SourceReadiness::RequiresConnection,
    }
}

/// app id: 決定的（時刻も乱数も使わない）。`generated.<主役の id>_<job>`。
pub fn app_id(r: &Reading) -> String {
    let job = serde_json::to_value(r.job).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let what = r.primary().map(|e| e.id.clone()).or_else(|| r.entities.first().map(|e| e.id.clone())).unwrap_or_else(|| "app".into());
    format!("generated.{what}_{job}")
}

pub fn blueprint(r: &Reading, registry: &[CapabilityDescriptor], app_id: String, strategy: Strategy) -> Blueprint {
    let minimal = strategy == Strategy::Minimal;
    let mut entities = Vec::new();
    let mut sources: Vec<Source> = Vec::new();
    let mut actions: Vec<ActionDefinition> = Vec::new();
    for e in &r.entities {
        if minimal && !matches!(e.kind, EntityKind::Primary | EntityKind::Drafts | EntityKind::Recordings | EntityKind::Clips | EntityKind::Pairing) {
            continue;
        }
        let source = format!("local.{}", e.id);
        let kind = if e.id == "timer" || matches!(e.kind, EntityKind::Candidates | EntityKind::Pairing) { "local_state" } else { "local_collection" };
        sources.push(Source { id: source.clone(), kind: kind.into(), readiness: SourceReadiness::Local, scope: Some("app".into()), last_observed_at: None, provider: None });
        let mut verbs = Vec::new();
        for verb in verbs_for(e, r.job, r.research, strategy) {
            let id = format!("{source}.{verb}");
            actions.push(local_action(id.clone(), &verb, input_schema(e, &verb)));
            verbs.push((verb, id));
        }
        entities.push(Planned { entity: e.clone(), source, verbs });
    }
    // 接続が要る能力: discover の readiness のまま（ready を名乗るのは registry が ready と言った物だけ）
    let mut capabilities: Vec<CapabilityDescriptor> = Vec::new();
    let mut externals = Vec::new();
    let mut discovery = Vec::new();
    for x in &r.externals {
        let d = discover(x, registry);
        if !capabilities.iter().any(|c| c.id == x.capability) {
            capabilities.push(CapabilityDescriptor {
                id: x.capability.clone(),
                provider: d.provider.clone(),
                readiness: d.readiness,
                required_scopes: vec![x.capability.clone()],
                adapter: d.adapter.clone(),
            });
        }
        if let Some(sid) = &x.source {
            match sources.iter_mut().find(|s| &s.id == sid) {
                // 同じ source の能力が複数ある時は、読める能力の状態を source の状態にする
                Some(s) if x.effect == Effect::ExternalRead => s.readiness = source_readiness(d.readiness),
                Some(_) => {}
                None => sources.push(Source {
                    id: sid.clone(),
                    kind: x.source_kind.clone(),
                    readiness: source_readiness(d.readiness),
                    scope: None,
                    last_observed_at: None,
                    provider: x.provider.clone(),
                }),
            }
        }
        let readiness = action_readiness(d.readiness);
        actions.push(ActionDefinition {
            id: x.action.clone(),
            capability_ref: Some(x.capability.clone()),
            effect: x.effect,
            readiness,
            input_schema: object_schema(
                &x.input.iter().filter(|f| f.name == "account_id" || f.name == "device_id" || f.name == "recording_id").map(|f| f.name.as_str()).collect::<Vec<_>>(),
                x.input.iter().map(|f| (f.name.clone(), prop(f))).collect(),
            ),
            output_schema: None,
            required_scopes: vec![x.capability.clone()],
            verifier: Some(
                match x.effect {
                    Effect::ExternalRead => "read_back",
                    Effect::ExternalSend => "source_receipt",
                    Effect::LocalWrite if x.role == ExternalRole::Mic => "local_state",
                    _ => "state_compare",
                }
                .into(),
            ),
            requires_approval: x.approval,
        });
        externals.push(PlannedExternal { need: x.clone(), readiness });
        discovery.push(d);
    }
    if entities.iter().any(|p| matches!(p.entity.kind, EntityKind::Candidates | EntityKind::Pairing)) || externals.iter().any(|x| x.need.role == ExternalRole::Editor) {
        actions.push(local_action("local.connection.open".into(), "open", object_schema(&[], vec![("capability_ref".into(), json!({ "type": "string" }))])));
    }
    // 承認つきの送信 / 書き込みは workflow に置き、承認の形を明示する（V20）
    let mut workflows = Vec::new();
    if let (Some(save), Some(send)) = (entities.iter().find(|p| p.entity.kind == EntityKind::Drafts).and_then(|p| p.action("save")), externals.iter().find(|x| x.need.role == ExternalRole::Send)) {
        workflows.push(Workflow { id: "reply_with_review".into(), action_refs: vec![save.to_string(), send.need.action.clone()], approval: Some("exact_payload".into()) });
    }
    if let Some(insert) = externals.iter().find(|x| x.need.action.ends_with(".insert_media")) {
        if let Some(rec) = externals.iter().find(|x| x.need.role == ExternalRole::Mic) {
            workflows.push(Workflow { id: "record_then_insert".into(), action_refs: vec![rec.need.action.clone(), insert.need.action.clone()], approval: Some("per_insert".into()) });
        }
    }
    Blueprint { app_id, strategy, entities, externals, sources, capabilities, actions, workflows, discovery }
}

fn plan_slug(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false) {
        out
    } else {
        format!("n_{out}")
    }
}

/// GoalPlan（goal_plan_v1）を書く。質問は CD-05（blocking ≤1・local_only は 0）。
pub fn goal_plan(r: &Reading, bp: &Blueprint, ui_intents: Vec<UiIntent>) -> GoalPlan {
    let mut needs: Vec<Need> = Vec::new();
    for p in &bp.entities {
        if p.verbs.is_empty() {
            continue;
        }
        let effect = p.verbs.iter().filter_map(|(_, id)| bp.action(id)).map(|a| NeedEffect::from_effect(a.effect)).fold(NeedEffect::Pure, NeedEffect::max);
        needs.push(Need {
            id: plan_slug(&format!("{}_local", p.entity.id)),
            description: format!("{}を端末内で扱う（{}）", p.entity.label, p.verbs.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(" / ")),
            effect,
            status: NeedStatus::PlannedLocal,
        });
    }
    for x in &bp.externals {
        let id = plan_slug(&x.need.action);
        if needs.iter().any(|n| n.id == id) {
            continue;
        }
        needs.push(Need {
            id,
            description: format!("{}（{}）", x.need.label, x.need.source_label),
            effect: NeedEffect::from_effect(x.need.effect),
            status: match x.readiness {
                ActionReadiness::Ready => NeedStatus::PlannedLocal,
                ActionReadiness::Unsupported => NeedStatus::Unsupported,
                _ if x.need.role == ExternalRole::Mic => NeedStatus::NeedsPermission,
                _ => NeedStatus::NeedsConnection,
            },
        });
    }
    // planned_local は外への書き込みに使わない（python 規則）: ready な外部書き込みは needs_permission のまま（承認は実行時）
    for n in needs.iter_mut() {
        if n.effect == NeedEffect::ExternalWrite && n.status == NeedStatus::PlannedLocal {
            n.status = NeedStatus::NeedsPermission;
        }
    }
    if needs.is_empty() {
        needs.push(Need { id: "view".into(), description: "表示だけ".into(), effect: NeedEffect::Pure, status: NeedStatus::Proposed });
    }
    let source_policy = if r.externals.is_empty() {
        SourcePolicy::LocalOnly
    } else if r.named_services() {
        SourcePolicy::RequestedConnection
    } else {
        SourcePolicy::OptionalConnections
    };
    let mut candidate_sources: Vec<CandidateSource> = Vec::new();
    for x in r.externals.iter().filter(|x| x.named.is_some()) {
        let id = plan_slug(x.source.as_deref().unwrap_or(&x.capability));
        if candidate_sources.iter().any(|c| c.id == id) {
            continue;
        }
        candidate_sources.push(CandidateSource {
            id,
            label: x.named.clone().unwrap_or_default(),
            // 語の写し方は domain の 1 か所（CD-07・統合 2 の merge 条件 4）。adapter の無い名指しは api
            kind: x.adapter.as_deref().and_then(|a| a.parse().ok()).unwrap_or(AdapterKind::Api),
            evidence: SourceEvidence::UserNamed,
        });
    }
    // 接続先が名指しされておらず、それが無いと主役が空のまま（会話・端末）の時だけ 1 問。既定つき = preview を止めない
    let mut questions = Vec::new();
    if let Some(x) = bp.externals.iter().find(|x| x.need.necessity == Necessity::Essential && x.need.named.is_none() && x.readiness != ActionReadiness::Ready && x.need.role != ExternalRole::Mic) {
        questions.push(Question {
            id: "choose_source".into(),
            text: format!("どの{}を繋ぎますか？", x.need.source_label),
            blocking_need_id: plan_slug(&x.need.action),
            reason: if x.need.role == ExternalRole::Screen { QuestionReason::Privacy } else { QuestionReason::Identity },
            suggested_default: Some("今は繋がず、画面と端末内の部分だけ作る".into()),
        });
    }
    let requirements: Vec<Requirement> = r
        .outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| Requirement {
            id: format!("outcome_{}", i + 1),
            statement: o.clone(),
            origin: if i == 0 { Origin::Explicit } else { Origin::Inferred },
            necessity: if i == 0 { Necessity::Essential } else { Necessity::Useful },
            confidence: if i == 0 { r.confidence } else { (r.confidence * 0.9 * 100.0).round() / 100.0 },
        })
        .collect();
    let mut verification_plan = vec![
        "ready な action は全部 validate_invocation の smoke を通る".to_string(),
        "ready でない action を指す node は states.disabled に理由がある".to_string(),
        "state は BuildVerifier の判定（ready / partial）のまま。接続していない物を ready と言わない".to_string(),
    ];
    if !r.externals.is_empty() {
        verification_plan.push("繋ぐまで外部の data を読まない・送らない（registry で ready の物だけ実行できる）".into());
    }
    GoalPlan {
        schema_version: PlanSchemaVersion::V1,
        id: plan_slug(bp.app_id.strip_prefix("generated.").unwrap_or(&bp.app_id)),
        prompt: r.literal.clone(),
        goal: PlanGoal { objective: r.objective.clone(), success_signal: r.outcomes.first().cloned().unwrap_or_else(|| r.objective.clone()) },
        requirements,
        needs,
        source_policy,
        candidate_sources,
        ui_intents,
        questions,
        verification_plan,
    }
}
