//! v1 / v2（docs/schemas/app_definition_v1/v2.schema.json）から v3 への移行（CORE-001・LEG-006）。
//! 入口は `to_v3`: schema_version で振り分け、"3.0" 以外の未知は UnknownSchemaVersion。
//! 方針: 元の文書は丸ごと metadata.legacy に残す（可逆）。移行した action / capability は ready にしない
//! （接続も実行も未検証）。見える node の元の kind / type_id は props に残す。
//! 対応表: v1 dataset→binding は dataset.produced_by の capability の source へ。v2 entity は v3 の source（kind "entity"）に、
//! v2 の namespaced type_id は BUILTIN_COMPONENTS へ写し、知らない物（surface.* / extension.* 等）は placeholder `extension`。
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fmt;

use super::action::{ActionDefinition, ActionReadiness, Effect};
use super::app::{AgentPolicy, AppDefinitionV3, AppReadiness, AppState, AutonomyLevel, Composition, ExecutionMode, Goal, SchemaVersion, ViewMode, Workflow};
use super::binding::{DataBinding, Source, SourceReadiness};
use super::capability::{CapabilityDescriptor, CapabilityReadiness};
use super::ui::UiNode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    MissingSchemaVersion,
    UnknownSchemaVersion(String),
    Parse { schema_version: String, error: String },
    Mapping { schema_version: String, detail: String },
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSchemaVersion => write!(f, "schema_version が無い"),
            Self::UnknownSchemaVersion(v) => write!(f, "schema_version '{v}' は知らない（1.0 / 2.0 / 3.0）"),
            Self::Parse { schema_version, error } => write!(f, "schema_version {schema_version} として読めない: {error}"),
            Self::Mapping { schema_version, detail } => write!(f, "v{schema_version} → v3 の移行で参照が解けない: {detail}"),
        }
    }
}
impl std::error::Error for MigrateError {}

/// どの版の app 定義でも v3 にして返す。検証（validate::validate）は呼ぶ側で（移行結果も同じ検査を通す）。
pub fn to_v3(value: &Value) -> Result<AppDefinitionV3, MigrateError> {
    let ver = value
        .get("schema_version")
        .and_then(Value::as_str)
        .ok_or(MigrateError::MissingSchemaVersion)?;
    let parse = |e: serde_json::Error| MigrateError::Parse { schema_version: ver.into(), error: e.to_string() };
    match ver {
        "3.0" => serde_json::from_value(value.clone()).map_err(parse),
        "2.0" => from_v2(serde_json::from_value(value.clone()).map_err(parse)?, value),
        "1.0" => from_v1(serde_json::from_value(value.clone()).map_err(parse)?, value),
        other => Err(MigrateError::UnknownSchemaVersion(other.into())),
    }
}

fn object_schema() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!("object"));
    m
}

fn default_agent() -> AgentPolicy {
    AgentPolicy {
        suggestions_enabled: true,
        autonomy_level: AutonomyLevel::SuggestOnly,
        entrypoint: "agent_rail".into(),
        policy_scope: vec![],
    }
}

fn legacy_metadata(version: &str, raw: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("migrated_from".into(), json!(version));
    m.insert("legacy".into(), raw.clone());
    m
}

fn mapping(version: &str, detail: String) -> MigrateError {
    MigrateError::Mapping { schema_version: version.into(), detail }
}

// ---------- v1 ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1 {
    schema_version: String,
    id: String,
    name: String,
    purpose: String,
    sources: Vec<V1Source>,
    capabilities: Vec<V1Capability>,
    datasets: Vec<V1Dataset>,
    view: V1View,
    workflows: Vec<V1Workflow>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Source {
    id: String,
    name: String,
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Capability {
    id: String,
    source_id: String,
    name: String,
    kind: String,
    requires_confirmation: bool,
    verify_as: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Dataset {
    id: String,
    produced_by: String,
    record_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1View {
    pattern: String,
    regions: Vec<V1Region>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Region {
    key: String,
    role: String,
    components: Vec<V1Component>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Component {
    key: String,
    kind: String,
    label: String,
    importance: String,
    dataset_id: Option<String>,
    action_id: Option<String>,
    workflow_id: Option<String>,
    visible_in: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Workflow {
    id: String,
    name: String,
    steps: Vec<V1Step>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Step {
    id: String,
    capability_id: String,
    after: Vec<String>,
}

fn v1_component(kind: &str) -> &'static str {
    match kind {
        "heading" | "text" | "status" => "text",
        "list" => "list",
        "data_table" => "table",
        "detail" => "detail",
        "form" => "form",
        "select" => "input",
        "button" | "workflow_launcher" => "button",
        "run_progress" => "run_progress",
        "calendar" => "calendar",
        "timeline" | "workflow_graph" => "timeline",
        "diff_view" => "editor",
        "media_controls" => "media",
        _ => "extension",
    }
}

fn from_v1(v: V1, raw: &Value) -> Result<AppDefinitionV3, MigrateError> {
    const VER: &str = "1.0";
    let capability = |id: &str| v.capabilities.iter().find(|c| c.id == id).ok_or_else(|| mapping(VER, format!("capability '{id}' が無い")));
    let source = |id: &str| v.sources.iter().find(|s| s.id == id).ok_or_else(|| mapping(VER, format!("source '{id}' が無い")));

    let sources = v
        .sources
        .iter()
        .map(|s| Source {
            id: s.id.clone(),
            kind: s.kind.clone(),
            readiness: SourceReadiness::RequiresConnection,
            scope: None,
            last_observed_at: None,
            provider: Some(s.name.clone()),
        })
        .collect();
    let capabilities = v
        .capabilities
        .iter()
        .map(|c| {
            let src = source(&c.source_id)?;
            Ok(CapabilityDescriptor {
                id: c.id.clone(),
                provider: Some(src.name.clone()),
                readiness: CapabilityReadiness::RequiresConnection,
                required_scopes: vec![],
                adapter: Some(src.kind.clone()),
            })
        })
        .collect::<Result<Vec<_>, MigrateError>>()?;
    // v1 の component.action_id は capability id を指すので、capability 1 つにつき同じ id の action を作る。
    let actions = v
        .capabilities
        .iter()
        .map(|c| ActionDefinition {
            id: c.id.clone(),
            capability_ref: Some(c.id.clone()),
            effect: if c.kind == "read" { Effect::ExternalRead } else { Effect::ExternalWrite },
            readiness: ActionReadiness::RequiresConnection,
            input_schema: object_schema(),
            output_schema: None,
            required_scopes: vec![],
            verifier: Some(c.verify_as.clone()),
            requires_approval: c.requires_confirmation,
        })
        .collect();

    let component_node = |c: &V1Component| -> Result<UiNode, MigrateError> {
        let binding = match &c.dataset_id {
            Some(d) => {
                let ds = v.datasets.iter().find(|x| x.id == *d).ok_or_else(|| mapping(VER, format!("dataset '{d}' が無い")))?;
                let cap = capability(&ds.produced_by)?;
                Some(DataBinding { source_ref: cap.source_id.clone(), path: ds.id.clone(), extra: Map::new() })
            }
            None => None,
        };
        if let Some(a) = &c.action_id {
            capability(a)?;
        }
        let mut props = Map::new();
        props.insert("importance".into(), json!(c.importance));
        props.insert("v1_kind".into(), json!(c.kind));
        if let Some(w) = &c.workflow_id {
            props.insert("workflow_ref".into(), json!(w));
        }
        let visibility = c.visible_in.as_ref().map(|states| {
            let mut m = Map::new();
            m.insert("visible_in".into(), json!(states));
            m
        });
        Ok(UiNode {
            key: c.key.clone(),
            component: v1_component(&c.kind).into(),
            label: Some(c.label.clone()),
            props,
            binding,
            action_ref: c.action_id.clone(),
            visibility,
            ..Default::default()
        })
    };
    let regions = v
        .view
        .regions
        .iter()
        .map(|r| {
            Ok(UiNode {
                key: r.key.clone(),
                component: "column".into(),
                role: Some(r.role.clone()),
                children: r.components.iter().map(component_node).collect::<Result<Vec<_>, _>>()?,
                ..Default::default()
            })
        })
        .collect::<Result<Vec<_>, MigrateError>>()?;
    let mut root_props = Map::new();
    root_props.insert("pattern".into(), json!(v.view.pattern));
    let root = UiNode { key: format!("{}.root", v.id), component: "column".into(), props: root_props, children: regions, ..Default::default() };

    let workflows = v
        .workflows
        .iter()
        .map(|w| Workflow { id: w.id.clone(), action_refs: w.steps.iter().map(|s| s.capability_id.clone()).collect(), approval: None })
        .collect();

    Ok(AppDefinitionV3 {
        schema_version: SchemaVersion::V3,
        id: v.id.clone(),
        version: 1,
        title: Some(v.name.clone()),
        goal: Goal { literal_goal: v.purpose.clone(), outcomes: vec![v.purpose.clone()], assumptions: vec![], constraints: vec![] },
        composition: Composition { root, view_modes: vec![ViewMode::Use, ViewMode::Build, ViewMode::Review] },
        capabilities,
        actions,
        sources,
        workflows,
        agent: default_agent(),
        state: AppState {
            readiness: AppReadiness::Partial,
            execution_mode: ExecutionMode::LocalDemo,
            status_message: Some("v1 から移行。接続と実行は未検証".into()),
        },
        metadata: Some(legacy_metadata(VER, raw)),
    })
}

// ---------- v2 ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2 {
    schema_version: String,
    id: String,
    name: String,
    purpose: String,
    sources: Vec<V2Source>,
    capabilities: Vec<V2Capability>,
    entities: Vec<V2Entity>,
    actions: Vec<V2Action>,
    views: Vec<V2View>,
    workflows: Vec<V2Workflow>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Source {
    id: String,
    display_name: String,
    adapter_kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Capability {
    id: String,
    source_id: String,
    catalog_ref: String,
    effect: String,
    verification: String,
    approval: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Entity {
    id: String,
    identity_key: String,
    fields: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Action {
    id: String,
    capability_id: String,
    target_entity_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Predicate {
    state_key: String,
    equals: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Node {
    key: String,
    type_id: String,
    label: String,
    priority: Option<String>,
    role: Option<String>,
    data_ref: Option<String>,
    action_ref: Option<String>,
    when: Option<V2Predicate>,
    props: Option<Map<String, Value>>,
    children: Option<Vec<V2Node>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2View {
    id: String,
    label: String,
    when: Option<V2Predicate>,
    root: V2Node,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Workflow {
    id: String,
    label: String,
    trigger: String,
    steps: Vec<V2Step>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Step {
    id: String,
    action_id: String,
    after: Vec<String>,
}

fn v2_component(type_id: &str) -> &'static str {
    match type_id.split('@').next().unwrap_or(type_id) {
        "layout.stack" | "layout.column" => "column",
        "layout.row" | "ui.toolbar" => "row",
        "layout.split" => "split",
        "layout.tabs" | "ui.tabs" => "tabs",
        "ui.text" | "ui.notice" | "ui.heading" | "ui.status" => "text",
        "ui.list" => "list",
        "ui.table" => "table",
        "ui.detail" => "detail",
        "ui.form" => "form",
        "ui.chat" => "chat",
        "ui.calendar" => "calendar",
        "ui.timeline" => "timeline",
        "ui.chart" => "chart",
        "ui.editor" => "editor",
        "ui.button" => "button",
        "ui.input" | "ui.select" => "input",
        "ui.command" => "command_input",
        "ui.run_progress" => "run_progress",
        "ui.player" | "ui.recorder" | "ui.media" => "media",
        _ => "extension",
    }
}

fn predicate_visibility(p: &V2Predicate) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("state_key".into(), json!(p.state_key));
    m.insert("equals".into(), p.equals.clone());
    m
}

fn v2_node(n: &V2Node) -> UiNode {
    let mut props = n.props.clone().unwrap_or_default();
    props.insert("type_id".into(), json!(n.type_id));
    if let Some(p) = &n.priority {
        props.insert("priority".into(), json!(p));
    }
    UiNode {
        key: n.key.clone(),
        component: v2_component(&n.type_id).into(),
        role: n.role.clone(),
        label: Some(n.label.clone()),
        props,
        binding: n.data_ref.as_ref().map(|d| DataBinding { source_ref: d.clone(), path: d.clone(), extra: Map::new() }),
        action_ref: n.action_ref.clone(),
        children: n.children.iter().flatten().map(v2_node).collect(),
        visibility: n.when.as_ref().map(predicate_visibility),
        ..Default::default()
    }
}

fn from_v2(v: V2, raw: &Value) -> Result<AppDefinitionV3, MigrateError> {
    const VER: &str = "2.0";
    let source = |id: &str| v.sources.iter().find(|s| s.id == id).ok_or_else(|| mapping(VER, format!("source '{id}' が無い")));
    let capability = |id: &str| v.capabilities.iter().find(|c| c.id == id).ok_or_else(|| mapping(VER, format!("capability '{id}' が無い")));
    let is_local = |source_id: &str| source(source_id).map(|s| s.adapter_kind == "local");

    let mut sources: Vec<Source> = v
        .sources
        .iter()
        .map(|s| Source {
            id: s.id.clone(),
            kind: s.adapter_kind.clone(),
            readiness: if s.adapter_kind == "local" { SourceReadiness::Local } else { SourceReadiness::RequiresConnection },
            scope: None,
            last_observed_at: None,
            provider: Some(s.display_name.clone()),
        })
        .collect();
    // entity は v3 では source（node.binding.source_ref は sources にしか向けられない）。作り手の action → capability → source で readiness を決める。
    for e in &v.entities {
        if sources.iter().any(|s| s.id == e.id) {
            return Err(mapping(VER, format!("entity '{}' が source と同じ id", e.id)));
        }
        let producer = v
            .actions
            .iter()
            .find(|a| a.target_entity_id.as_deref() == Some(&e.id))
            .map(|a| capability(&a.capability_id))
            .transpose()?;
        let (readiness, provider) = match producer {
            Some(cap) => (
                if is_local(&cap.source_id)? { SourceReadiness::Local } else { SourceReadiness::RequiresConnection },
                Some(cap.source_id.clone()),
            ),
            None => (SourceReadiness::RequiresConnection, None),
        };
        sources.push(Source { id: e.id.clone(), kind: "entity".into(), readiness, scope: None, last_observed_at: None, provider });
    }

    let capabilities = v
        .capabilities
        .iter()
        .map(|c| {
            let src = source(&c.source_id)?;
            Ok(CapabilityDescriptor {
                id: c.id.clone(),
                provider: Some(c.catalog_ref.clone()),
                readiness: if src.adapter_kind == "local" { CapabilityReadiness::Discoverable } else { CapabilityReadiness::RequiresConnection },
                required_scopes: vec![],
                adapter: Some(src.adapter_kind.clone()),
            })
        })
        .collect::<Result<Vec<_>, MigrateError>>()?;

    let actions = v
        .actions
        .iter()
        .map(|a| {
            let cap = capability(&a.capability_id)?;
            let local = is_local(&cap.source_id)?;
            let effect = match (cap.effect.as_str(), local) {
                ("read", true) => Effect::LocalRead,
                ("read", false) => Effect::ExternalRead,
                ("external_write", _) => Effect::ExternalWrite,
                (_, true) => Effect::LocalWrite,
                (_, false) => Effect::ExternalWrite,
            };
            Ok(ActionDefinition {
                id: a.id.clone(),
                capability_ref: Some(cap.id.clone()),
                effect,
                readiness: if local { ActionReadiness::PreviewOnly } else { ActionReadiness::RequiresConnection },
                input_schema: object_schema(),
                output_schema: None,
                required_scopes: vec![],
                verifier: Some(cap.verification.clone()),
                requires_approval: cap.approval != "none",
            })
        })
        .collect::<Result<Vec<_>, MigrateError>>()?;

    let view_root = |view: &V2View| {
        let mut node = v2_node(&view.root);
        node.props.insert("view_id".into(), json!(view.id));
        node.props.insert("view_label".into(), json!(view.label));
        if let Some(w) = &view.when {
            node.visibility = Some(predicate_visibility(w));
        }
        node
    };
    let root = match v.views.as_slice() {
        [] => return Err(mapping(VER, "views が空".into())),
        [only] => view_root(only),
        many => UiNode {
            key: format!("{}.views", v.id),
            component: "tabs".into(),
            children: many.iter().map(view_root).collect(),
            ..Default::default()
        },
    };

    let workflows = v
        .workflows
        .iter()
        .map(|w| Workflow {
            id: w.id.clone(),
            action_refs: w.steps.iter().map(|s| s.action_id.clone()).collect(),
            approval: Some(w.trigger.clone()),
        })
        .collect();
    let all_local = sources.iter().all(|s| s.readiness == SourceReadiness::Local);

    Ok(AppDefinitionV3 {
        schema_version: SchemaVersion::V3,
        id: v.id.clone(),
        version: 1,
        title: Some(v.name.clone()),
        goal: Goal { literal_goal: v.purpose.clone(), outcomes: vec![v.purpose.clone()], assumptions: vec![], constraints: vec![] },
        composition: Composition { root, view_modes: vec![ViewMode::Use, ViewMode::Build, ViewMode::Review] },
        capabilities,
        actions,
        sources,
        workflows,
        agent: default_agent(),
        state: AppState {
            readiness: if all_local { AppReadiness::Preview } else { AppReadiness::Partial },
            execution_mode: ExecutionMode::LocalDemo,
            status_message: Some("v2 から移行。接続と実行は未検証".into()),
        },
        metadata: Some(legacy_metadata(VER, raw)),
    })
}
