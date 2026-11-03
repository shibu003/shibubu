//! 相互参照の統一 validator（CORE-008）。serde が表せない schema の制約（minLength / minimum / uniqueItems）もここ。
//! button→action / binding→source / action→capability / workflow の action_refs→action / permission→scope。
//! error は「何が悪いか」を id / key 入りで言う。全部集めて返す（最初の 1 つで止めない）。
//!
//! 2 段: `validate()` = 読み込み・patch の門（設計サンプル app_v3_* と移行物も通る緩い検査）。
//! `validate_generated()` = 生成物の門（`validate()` ＋ docs/examples/README.md の corpus 規則 = CD-14 の V5・V6・V8・V9・V11・V12・
//! V14・V16〜V20）。composer の出力と corpus（docs/examples/v3）はこちらを通る。設計サンプルは README §1 で「緩く検査」なので通さない。
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

use super::action::{ActionReadiness, Effect};
use super::app::{AppDefinitionV3, AppReadiness, ExecutionMode};
use super::binding::SourceReadiness;
use super::capability::CapabilityReadiness;
use super::patch::PatchOp;
use super::permission::PermissionGrant;
use super::suggestion::Suggestion;
use super::ui::{UiNode, BUILTIN_COMPONENTS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    EmptyField(String),
    VersionBelowOne(u32),
    DuplicateId { kind: &'static str, id: String },
    DuplicateKey(String),
    UnsupportedComponent { key: String, component: String },
    MissingAction { key: String, action_ref: String },
    MissingSource { key: String, source_ref: String },
    /// W2c: `source_surface` の `props.surface_ref` が Project の `surface_specs` に無い（または source_surface 以外が surface_ref を持つ）
    MissingSurface { key: String, surface_ref: String },
    /// W5: `terminal` の `props.terminal_ref` が Project の `terminal_specs` に無い（または terminal 以外が terminal_ref を持つ）
    MissingTerminal { key: String, terminal_ref: String },
    MissingCapability { action: String, capability_ref: String },
    CapabilityNotReady { action: String, capability: String, readiness: CapabilityReadiness },
    ExternalWithoutCapability { action: String },
    /// execution_mode が local_demo なのに external 系の action が ready（docs/scripts/validate_agent_native_contracts.py と同じ規則）。
    PhantomReady { action: String },
    MissingWorkflowAction { workflow: String, action_ref: String },
    AppMismatch { expected: String, found: String },
    VersionMismatch { expected: u32, found: u32 },
    NoEvidence(String),
    MissingTarget { suggestion: String, target: String },
    UndeclaredScope(String),
    // ---- validate_generated（CD-14）----
    /// V5: external_send / destructive は requires_approval: true
    ApprovalRequired { action: String },
    /// V6: props に型付き field / 行データの key
    ForbiddenProp { key: String, prop: String },
    /// V6: 絶対 path
    AbsolutePath { key: String },
    /// V8: id の pattern（CD-08）
    BadId { kind: &'static str, id: String },
    /// V9: 深さ ≤24・node ≤500
    TreeTooLarge { depth: usize, nodes: usize },
    /// V9: children は容器だけ
    ChildrenOnLeaf { key: String, component: String },
    /// V9: binding は葉だけ
    BindingOnContainer { key: String },
    /// V11: visibility は {"when": "has_selection"} だけ
    UnknownVisibility { key: String },
    /// V12: workflow の action_refs は 1〜20
    WorkflowSize { workflow: String, count: usize },
    /// V14: state が action / source の readiness と合わない
    StateMismatch(String),
    /// V14 / V16: preview_only / local_demo / preview は生成物で使わない
    NotForGenerated(String),
    /// V16: requires_connection なのに capability が無い・ready
    ConnectionLie { action: String },
    /// V16: unsupported なのに capability が unsupported でない
    UnsupportedLie { action: String },
    /// V16: 未接続の source に last_observed_at（捏造した鮮度）
    StaleClaim { source: String },
    /// V16: readiness local なのに kind が local でない
    LocalKindMismatch { source: String },
    /// V17: states.disabled / empty / requires_connection の文が無い
    MissingState { key: String, state: &'static str },
    /// V18: card は props.selectable: true の時だけ
    CardNotSelectable { key: String },
    /// V18: chart は metadata.composer.rationale に理由が要る（生成時だけ）
    ChartWithoutRationale { key: String },
    /// V19: button / form / command_input / recorder は action_ref 必須
    MissingActionRef { key: String, component: String },
    /// V19: source_surface は props.display ∈ native / embedded / handoff / mirror / composite
    BadSurfaceDisplay { key: String },
    /// V20: 送信 / 削除を含む workflow は approval（none 不可）
    WorkflowWithoutApproval { workflow: String },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(path) => write!(f, "'{path}' が空"),
            Self::VersionBelowOne(v) => write!(f, "version は 1 以上（{v}）"),
            Self::DuplicateId { kind, id } => write!(f, "{kind} id '{id}' が重複"),
            Self::DuplicateKey(key) => write!(f, "UI key '{key}' が重複"),
            Self::UnsupportedComponent { key, component } => write!(f, "node '{key}' の component '{component}' は描けない"),
            Self::MissingAction { key, action_ref } => write!(f, "node '{key}' の action_ref '{action_ref}' が actions に無い"),
            Self::MissingSource { key, source_ref } => write!(f, "node '{key}' の binding.source_ref '{source_ref}' が sources に無い"),
            Self::MissingSurface { key, surface_ref } => write!(f, "node '{key}' の surface_ref '{surface_ref}' が surface_specs に無い（source_surface だけが持てる）"),
            Self::MissingTerminal { key, terminal_ref } => write!(f, "node '{key}' の terminal_ref '{terminal_ref}' が terminal_specs に無い（terminal だけが持てる）"),
            Self::MissingCapability { action, capability_ref } => {
                write!(f, "action '{action}' の capability_ref '{capability_ref}' が capabilities に無い")
            }
            Self::CapabilityNotReady { action, capability, readiness } => {
                write!(f, "action '{action}' は ready だが capability '{capability}' は {readiness:?}")
            }
            Self::ExternalWithoutCapability { action } => write!(f, "action '{action}' は external で ready だが capability_ref が無い"),
            Self::PhantomReady { action } => write!(f, "local_demo では external 系の action '{action}' を ready にできない"),
            Self::MissingWorkflowAction { workflow, action_ref } => {
                write!(f, "workflow '{workflow}' の action_ref '{action_ref}' が actions に無い")
            }
            Self::AppMismatch { expected, found } => write!(f, "app_id が違う: 期待 '{expected}'・実際 '{found}'"),
            Self::VersionMismatch { expected, found } => write!(f, "app_version が違う（stale）: 期待 {expected}・実際 {found}"),
            Self::NoEvidence(id) => write!(f, "提案 '{id}' に evidence が無い"),
            Self::MissingTarget { suggestion, target } => write!(f, "提案 '{suggestion}' の対象 '{target}' が UI に無い"),
            Self::UndeclaredScope(scope) => write!(f, "scope '{scope}' は app が宣言していない"),
            Self::ApprovalRequired { action } => write!(f, "action '{action}' は送信 / 削除なので requires_approval: true が要る"),
            Self::ForbiddenProp { key, prop } => write!(f, "node '{key}' の props に '{prop}' を書かない（型付き field と binding に置く）"),
            Self::AbsolutePath { key } => write!(f, "node '{key}' の props に絶対 path がある"),
            Self::BadId { kind, id } => write!(f, "{kind} id '{id}' は CD-08 の pattern に合わない"),
            Self::TreeTooLarge { depth, nodes } => write!(f, "UI の木が大きすぎる（深さ {depth} / node {nodes}。上限 24 / 500）"),
            Self::ChildrenOnLeaf { key, component } => write!(f, "node '{key}'（{component}）は容器でないのに children を持つ"),
            Self::BindingOnContainer { key } => write!(f, "node '{key}' は children を持つのに binding がある（binding は葉だけ）"),
            Self::UnknownVisibility { key } => write!(f, "node '{key}' の visibility は {{\"when\": \"has_selection\"}} だけ"),
            Self::WorkflowSize { workflow, count } => write!(f, "workflow '{workflow}' の action_refs は 1〜20（{count}）"),
            Self::StateMismatch(why) => write!(f, "state が readiness と合わない: {why}"),
            Self::NotForGenerated(what) => write!(f, "{what} は生成物では使わない（ready か requires_connection か unsupported）"),
            Self::ConnectionLie { action } => write!(f, "action '{action}' は requires_connection なのに、ready でない capability を指していない"),
            Self::UnsupportedLie { action } => write!(f, "action '{action}' は unsupported なのに capability が unsupported でない"),
            Self::StaleClaim { source } => write!(f, "未接続の source '{source}' に last_observed_at（捏造した鮮度）"),
            Self::LocalKindMismatch { source } => write!(f, "source '{source}' は readiness local なのに kind が local でない"),
            Self::MissingState { key, state } => write!(f, "node '{key}' に states.{state} の文が無い（DESIGN §4.1 / §11）"),
            Self::CardNotSelectable { key } => write!(f, "node '{key}' の card は props.selectable: true の時だけ（DESIGN §3）"),
            Self::ChartWithoutRationale { key } => write!(f, "node '{key}' の chart に理由（metadata.composer.rationale）が無い（DESIGN §3: KPI 0）"),
            Self::MissingActionRef { key, component } => write!(f, "node '{key}'（{component}）に action_ref が無い"),
            Self::BadSurfaceDisplay { key } => write!(f, "node '{key}' の source_surface は props.display が native / embedded / handoff / mirror / composite"),
            Self::WorkflowWithoutApproval { workflow } => write!(f, "workflow '{workflow}' は送信 / 削除を含むのに approval が無い"),
        }
    }
}
impl std::error::Error for ValidationError {}

fn dups<'a>(kind: &'static str, ids: impl Iterator<Item = &'a str>, errs: &mut Vec<ValidationError>) {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            errs.push(ValidationError::DuplicateId { kind, id: id.into() });
        }
    }
}

pub fn validate(def: &AppDefinitionV3) -> Result<(), Vec<ValidationError>> {
    use ValidationError::*;
    let mut errs = Vec::new();
    if def.id.is_empty() {
        errs.push(EmptyField("id".into()));
    }
    if def.version < 1 {
        errs.push(VersionBelowOne(def.version));
    }
    if def.goal.literal_goal.is_empty() {
        errs.push(EmptyField("goal.literal_goal".into()));
    }
    if def.goal.outcomes.is_empty() {
        errs.push(EmptyField("goal.outcomes".into()));
    }
    dups("action", def.actions.iter().map(|a| a.id.as_str()), &mut errs);
    dups("source", def.sources.iter().map(|s| s.id.as_str()), &mut errs);
    dups("capability", def.capabilities.iter().map(|c| c.id.as_str()), &mut errs);
    dups("workflow", def.workflows.iter().map(|w| w.id.as_str()), &mut errs);
    let modes: Vec<String> = def.composition.view_modes.iter().map(|m| format!("{m:?}").to_lowercase()).collect();
    dups("view_mode", modes.iter().map(String::as_str), &mut errs);
    for (i, a) in def.actions.iter().enumerate() {
        if a.id.is_empty() {
            errs.push(EmptyField(format!("actions[{i}].id")));
        }
    }
    for (i, s) in def.sources.iter().enumerate() {
        if s.id.is_empty() {
            errs.push(EmptyField(format!("sources[{i}].id")));
        }
    }

    let mut keys = HashSet::new();
    for node in def.composition.root.nodes() {
        if node.key.is_empty() {
            errs.push(EmptyField("composition node .key".into()));
        }
        if !keys.insert(node.key.as_str()) {
            errs.push(DuplicateKey(node.key.clone()));
        }
        if node.component.is_empty() {
            errs.push(EmptyField(format!("node '{}'.component", node.key)));
        } else if !BUILTIN_COMPONENTS.contains(&node.component.as_str()) {
            errs.push(UnsupportedComponent { key: node.key.clone(), component: node.component.clone() });
        }
        if let Some(a) = &node.action_ref {
            if def.action(a).is_none() {
                errs.push(MissingAction { key: node.key.clone(), action_ref: a.clone() });
            }
        }
        if let Some(b) = &node.binding {
            if def.source(&b.source_ref).is_none() {
                errs.push(MissingSource { key: node.key.clone(), source_ref: b.source_ref.clone() });
            }
        }
    }

    for a in &def.actions {
        let ready = a.readiness == ActionReadiness::Ready;
        match &a.capability_ref {
            Some(c) => match def.capability(c) {
                None => errs.push(MissingCapability { action: a.id.clone(), capability_ref: c.clone() }),
                Some(cap) if ready && cap.readiness != CapabilityReadiness::Ready => errs.push(CapabilityNotReady {
                    action: a.id.clone(),
                    capability: cap.id.clone(),
                    readiness: cap.readiness,
                }),
                Some(_) => {}
            },
            None if ready && a.effect.is_external() => errs.push(ExternalWithoutCapability { action: a.id.clone() }),
            None => {}
        }
        if def.state.execution_mode == ExecutionMode::LocalDemo && a.effect.is_external() && ready {
            errs.push(PhantomReady { action: a.id.clone() });
        }
    }

    for w in &def.workflows {
        for r in &w.action_refs {
            if def.action(r).is_none() {
                errs.push(MissingWorkflowAction { workflow: w.id.clone(), action_ref: r.clone() });
            }
        }
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// 提案が今の定義に当てられるか: app / version の一致・evidence・target_ref と各 op の target_key・新 key の一意・scope の宣言。
/// 落ちた提案は apply に進めない（AGT-005）。
pub fn validate_suggestion(def: &AppDefinitionV3, s: &Suggestion) -> Result<(), Vec<ValidationError>> {
    use ValidationError::*;
    let mut errs = Vec::new();
    if s.app_id != def.id {
        errs.push(AppMismatch { expected: def.id.clone(), found: s.app_id.clone() });
    }
    if s.app_version != def.version {
        errs.push(VersionMismatch { expected: def.version, found: s.app_version });
    }
    if s.reason.is_empty() {
        errs.push(EmptyField("reason".into()));
    }
    if s.evidence.is_empty() {
        errs.push(NoEvidence(s.id.clone()));
    }
    let root = &def.composition.root;
    let missing = |target: &str| MissingTarget { suggestion: s.id.clone(), target: target.into() };
    if root.find(&s.target_ref).is_none() {
        errs.push(missing(&s.target_ref));
    }
    for op in &s.change.operations {
        if root.find(&op.target_key).is_none() {
            errs.push(missing(&op.target_key));
        }
        if op.op == PatchOp::Insert {
            if let Some(key) = op.value.as_ref().and_then(|v| v.get("key")).and_then(|k| k.as_str()) {
                if root.find(key).is_some() {
                    errs.push(DuplicateKey(key.into()));
                }
            }
        }
    }
    let declared = def.declared_scopes();
    for scope in &s.permission_scopes {
        if !declared.contains(scope.as_str()) {
            errs.push(UndeclaredScope(scope.clone()));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// permission→scope: grant はその app が宣言した scope にしか付かない。
pub fn validate_grant(def: &AppDefinitionV3, grant: &PermissionGrant) -> Result<(), ValidationError> {
    if grant.app_id != def.id {
        return Err(ValidationError::AppMismatch { expected: def.id.clone(), found: grant.app_id.clone() });
    }
    if !def.declared_scopes().contains(grant.scope.as_str()) {
        return Err(ValidationError::UndeclaredScope(grant.scope.clone()));
    }
    Ok(())
}

// ============================== validate_generated（生成物の門・CD-14） ==============================

/// V9 の容器（COMPILER:40 の 7 種 ＋ tabs）。
pub const CONTAINERS: &[&str] = &["column", "row", "split", "tabs", "stack", "overlay", "scroll"];
/// bind した時に states.empty が要る component（README 規則 5）。
const COLLECTIONS: &[&str] = &["list", "table", "calendar", "timeline", "chat", "detail"];
/// V6: props に置かない key（型付き field と binding が持つ物・行データ）。
const FORBIDDEN_PROPS: &[&str] = &["readiness", "effect", "status", "action_ref", "binding", "items", "rows", "data", "records", "messages", "events", "threads", "contacts", "replies"];
const SURFACE_DISPLAY: &[&str] = &["native", "embedded", "handoff", "mirror", "composite"];
const NEEDS_ACTION: &[&str] = &["button", "form", "command_input", "recorder"];
/// V11 の閉じた語彙（CD-11 (4)）。足す時は renderer と同じ commit で。
const VISIBILITY_WHEN: &[&str] = &["has_selection"];

/// CD-08: `^[a-z][a-z0-9_-]*(\.[a-z0-9][a-z0-9_-]*)*$`、≤128。`need_dot` は action / capability（namespace 必須）。
pub fn dotted_id_ok(id: &str, need_dot: bool) -> bool {
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    let seg = |s: &str, first: bool| {
        let b = s.as_bytes();
        !b.is_empty()
            && (b[0].is_ascii_lowercase() || (!first && b[0].is_ascii_digit()))
            && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
    };
    let mut parts = id.split('.');
    let first_ok = parts.next().map(|s| seg(s, true)).unwrap_or(false);
    let rest: Vec<&str> = parts.collect();
    first_ok && rest.iter().all(|s| seg(s, false)) && (!need_dot || !rest.is_empty())
}

/// CD-08: node key `^[a-z0-9][a-z0-9_-]{0,79}$`（egui の Id は key から作る）。
pub fn node_key_ok(key: &str) -> bool {
    let b = key.as_bytes();
    (1..=80).contains(&b.len()) && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

fn has_text(map: Option<&serde_json::Map<String, Value>>, k: &str) -> bool {
    map.and_then(|m| m.get(k)).and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false)
}

fn absolute_path(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains("/Users/") || s.contains("/home/") || s.starts_with("C:\\"),
        Value::Array(a) => a.iter().any(absolute_path),
        Value::Object(o) => o.values().any(absolute_path),
        _ => false,
    }
}

fn walk_generated(def: &AppDefinitionV3, n: &UiNode, depth: usize, count: &mut usize, max_depth: &mut usize, errs: &mut Vec<ValidationError>) {
    use ValidationError::*;
    *count += 1;
    *max_depth = (*max_depth).max(depth);
    let key = n.key.clone();
    let comp = n.component.as_str();
    if !node_key_ok(&n.key) {
        errs.push(BadId { kind: "node key", id: key.clone() });
    }
    for p in n.props.keys() {
        if FORBIDDEN_PROPS.contains(&p.as_str()) {
            errs.push(ForbiddenProp { key: key.clone(), prop: p.clone() });
        }
    }
    if n.props.values().any(absolute_path) {
        errs.push(AbsolutePath { key: key.clone() });
    }
    if !n.children.is_empty() && !CONTAINERS.contains(&comp) {
        errs.push(ChildrenOnLeaf { key: key.clone(), component: comp.into() });
    }
    if !n.children.is_empty() && n.binding.is_some() {
        errs.push(BindingOnContainer { key: key.clone() });
    }
    if let Some(vis) = &n.visibility {
        let ok = vis.len() == 1 && vis.get("when").and_then(Value::as_str).map(|w| VISIBILITY_WHEN.contains(&w)).unwrap_or(false);
        if !ok {
            errs.push(UnknownVisibility { key: key.clone() });
        }
    }
    let states = n.states.as_ref();
    if let Some(a) = n.action_ref.as_deref().and_then(|id| def.action(id)) {
        if a.readiness != ActionReadiness::Ready && !has_text(states, "disabled") {
            errs.push(MissingState { key: key.clone(), state: "disabled" });
        }
    }
    if let Some(b) = &n.binding {
        if COLLECTIONS.contains(&comp) && !has_text(states, "empty") {
            errs.push(MissingState { key: key.clone(), state: "empty" });
        }
        if let Some(s) = def.source(&b.source_ref) {
            if !matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready) && !has_text(states, "requires_connection") {
                errs.push(MissingState { key: key.clone(), state: "requires_connection" });
            }
        }
    }
    if comp == "card" && n.props.get("selectable") != Some(&Value::Bool(true)) {
        errs.push(CardNotSelectable { key: key.clone() });
    }
    if comp == "chart" {
        let why = def.metadata.as_ref().and_then(|m| m.get("composer")).and_then(|c| c.get("rationale")).and_then(|r| r.get(&n.key)).and_then(Value::as_str);
        if why.map(|w| w.trim().is_empty()).unwrap_or(true) {
            errs.push(ChartWithoutRationale { key: key.clone() });
        }
    }
    if NEEDS_ACTION.contains(&comp) && n.action_ref.is_none() {
        errs.push(MissingActionRef { key: key.clone(), component: comp.into() });
    }
    if comp == "source_surface" && !n.props.get("display").and_then(Value::as_str).map(|d| SURFACE_DISPLAY.contains(&d)).unwrap_or(false) {
        errs.push(BadSurfaceDisplay { key });
    }
    for c in &n.children {
        walk_generated(def, c, depth + 1, count, max_depth, errs);
    }
}

/// 生成物の門: `validate()` ＋ README の corpus 規則（CD-14 V5・V6・V8・V9・V11・V12・V14・V16〜V20）。全部集めて返す。
/// V7（AdapterKind の正規化）は AD 待ちの 2 語（CD-07）が決まるまで入れない。V15（GoalPlan）は composer::goal::validate_plan。
pub fn validate_generated(def: &AppDefinitionV3) -> Result<(), Vec<ValidationError>> {
    use ValidationError::*;
    let mut errs = validate(def).err().unwrap_or_default();
    // V8
    if !dotted_id_ok(&def.id, false) {
        errs.push(BadId { kind: "app", id: def.id.clone() });
    }
    for a in &def.actions {
        if !dotted_id_ok(&a.id, true) {
            errs.push(BadId { kind: "action", id: a.id.clone() });
        }
    }
    for c in &def.capabilities {
        if !dotted_id_ok(&c.id, true) {
            errs.push(BadId { kind: "capability", id: c.id.clone() });
        }
    }
    for s in &def.sources {
        if !dotted_id_ok(&s.id, false) {
            errs.push(BadId { kind: "source", id: s.id.clone() });
        }
    }
    for w in &def.workflows {
        if !dotted_id_ok(&w.id, false) {
            errs.push(BadId { kind: "workflow", id: w.id.clone() });
        }
    }
    // V5・V16（action）
    for a in &def.actions {
        let cap = a.capability_ref.as_deref().and_then(|c| def.capability(c));
        if matches!(a.effect, Effect::ExternalSend | Effect::Destructive) && !a.requires_approval {
            errs.push(ApprovalRequired { action: a.id.clone() });
        }
        match a.readiness {
            ActionReadiness::PreviewOnly => errs.push(NotForGenerated(format!("action '{}' の preview_only", a.id))),
            ActionReadiness::RequiresConnection => {
                let not_ready = cap.map(|c| {
                    use CapabilityReadiness::*;
                    // README 規則 2「requires_connection なら capability が未 ready」。simulated_in_preview（preview の中だけ）も未 ready
                    matches!(c.readiness, Unknown | Discoverable | RequiresConnection | ConnectedUnverified | Degraded | Revoked | SimulatedInPreview)
                });
                if not_ready != Some(true) {
                    errs.push(ConnectionLie { action: a.id.clone() });
                }
            }
            ActionReadiness::Unsupported => {
                if cap.map(|c| c.readiness) != Some(CapabilityReadiness::Unsupported) {
                    errs.push(UnsupportedLie { action: a.id.clone() });
                }
            }
            ActionReadiness::Ready => {}
        }
        // ready の時は validate() が ExternalWithoutCapability を出す。ここは ready でない時も（README 規則 2）
        if a.effect.is_external() && a.capability_ref.is_none() && a.readiness != ActionReadiness::Ready {
            errs.push(ExternalWithoutCapability { action: a.id.clone() });
        }
    }
    // V16（source）
    for s in &def.sources {
        if s.readiness == SourceReadiness::Local && !s.kind.starts_with("local") {
            errs.push(LocalKindMismatch { source: s.id.clone() });
        }
        if !matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready) && s.last_observed_at.is_some() {
            errs.push(StaleClaim { source: s.id.clone() });
        }
    }
    // V12・V20
    for w in &def.workflows {
        if w.action_refs.is_empty() || w.action_refs.len() > 20 {
            errs.push(WorkflowSize { workflow: w.id.clone(), count: w.action_refs.len() });
        }
        let gated = w.action_refs.iter().filter_map(|r| def.action(r)).any(|a| matches!(a.effect, Effect::ExternalSend | Effect::Destructive));
        if gated && w.approval.as_deref().map(|s| s.trim().is_empty() || s == "none").unwrap_or(true) {
            errs.push(WorkflowWithoutApproval { workflow: w.id.clone() });
        }
    }
    // V14
    let ready = def.actions.iter().filter(|a| a.readiness == ActionReadiness::Ready).count();
    let waiting = def.actions.len() - ready;
    let src_waiting = def.sources.iter().filter(|s| !matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready)).count();
    let ext_ready = def.sources.iter().any(|s| s.readiness == SourceReadiness::Ready);
    match def.state.execution_mode {
        ExecutionMode::Integrated if !ext_ready => errs.push(StateMismatch("integrated なのに ready な外部 source が無い".into())),
        ExecutionMode::LocalReady if ready == 0 || ext_ready => errs.push(StateMismatch("local_ready は ready な local action が在り、ready な外部 source が無い時だけ".into())),
        ExecutionMode::LocalDemo | ExecutionMode::Disabled => errs.push(NotForGenerated(format!("execution_mode {:?}", def.state.execution_mode))),
        _ => {}
    }
    match def.state.readiness {
        AppReadiness::Ready if waiting > 0 || src_waiting > 0 => errs.push(StateMismatch(format!("ready なのに ready でない action {waiting} / source {src_waiting}"))),
        AppReadiness::Partial if ready == 0 || (waiting == 0 && src_waiting == 0) => errs.push(StateMismatch("partial は ready と未 ready が混ざる時だけ".into())),
        AppReadiness::Blocked if ready > 0 => errs.push(StateMismatch("blocked なのに ready な action が在る".into())),
        AppReadiness::Preview | AppReadiness::Error => errs.push(NotForGenerated(format!("state.readiness {:?}", def.state.readiness))),
        _ => {}
    }
    // V6・V8（key）・V9・V11・V17〜V19
    let (mut count, mut depth) = (0, 0);
    walk_generated(def, &def.composition.root, 1, &mut count, &mut depth, &mut errs);
    if depth > 24 || count > 500 {
        errs.push(TreeTooLarge { depth, nodes: count });
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}
