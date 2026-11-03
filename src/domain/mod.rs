//! domain: v0.12 の契約を Rust の serde 型にした物（TODO.md CORE-001..008）。
//! 正本は docs/schemas/agent_native_app_v3.schema.json と agent_proposal_v1.schema.json。
//! field 名は schema そのまま（TODO の intent / ui_tree / data_sources / agent_policy / readiness は
//! schema では goal / composition / sources / agent / state。rename しない）。
//! 他 unit（runtime / composer / renderer / agent）が `crate::domain::*` で使う。
//! tests/contracts.rs は `#[path]` でこの mod.rs を取り込む（bin crate で lib target が無いため）ので、
//! この module 配下は serde / serde_json / std 以外に依存しない。
#![allow(dead_code)] // 他 unit が配線するまで main からの呼び出しが無い（配線後に外す）

pub mod capability;
pub mod icon;
pub mod migrate;
pub mod project;
pub mod terminal;
pub mod validate;
pub mod vault;

/// 画面に出る数と名詞（operator 10-29）: 1 だけ単数（`1 way`・`2 ways`・`0 ways`）。名詞は s を足すだけの物に使う。
/// 画面の文の層（ui・Engine の 1 行・generated）が共有する（どの試験 crate も domain は `#[path]` で取り込む）。src/llm の `plural` は llm だけの試験 crate のため別に持つ
pub fn plural(n: usize, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// RQ3 → HOME-ATTACH-1101: 画面の大きさ（`Empty`・`12 KB`・`2.1 MB`・`1.4 GB`）。Settings の data の行と Home の添付の長方形が共有する
/// （ui の試験 crate は generated を取り込まないので domain に置く）。
pub fn size_line(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    match b {
        0 => "Empty".into(),
        1..=1_048_575 => format!("{} KB", b.div_ceil(1024)),
        _ if (b as f64) < 1024.0 * MB => format!("{:.1} MB", b as f64 / MB),
        _ => format!("{:.1} GB", b as f64 / MB / 1024.0),
    }
}

#[cfg(test)]
#[test]
fn plural_is_singular_only_for_1() {
    assert_eq!((plural(0, "way"), plural(1, "way"), plural(2, "way")), ("0 ways".into(), "1 way".into(), "2 ways".into()));
}

// ---- action（旧 domain/action.rs）----
pub mod action {
    //! ActionDefinition（schema `actions[]`）と ActionInvocation（CORE-003）。
    //! button / command palette / Agent のどれから来ても `validate_invocation` 1 本を通る。見える UiNode とは別の型。
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};
    use std::fmt;

    use super::app::AppDefinitionV3;
    use super::capability::CapabilityReadiness;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ActionDefinition {
        pub id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub capability_ref: Option<String>,
        pub effect: Effect,
        pub readiness: ActionReadiness,
        pub input_schema: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub output_schema: Option<Map<String, Value>>,
        pub required_scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub verifier: Option<String>,
        #[serde(default)]
        pub requires_approval: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Effect {
        LocalRead,
        LocalWrite,
        ExternalRead,
        ExternalWrite,
        ExternalSend,
        Destructive,
    }

    impl Effect {
        /// 外に出る効果（docs/scripts/validate_agent_native_contracts.py の `effect.startswith('external')` と同じ範囲）。
        pub fn is_external(self) -> bool {
            matches!(self, Self::ExternalRead | Self::ExternalWrite | Self::ExternalSend)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ActionReadiness {
        Ready,
        PreviewOnly,
        RequiresConnection,
        Unsupported,
    }

    /// 「誰が・どの action を・何を入れて」呼んだか。origin が違っても検証は同じ。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ActionInvocation {
        pub action_ref: String,
        pub origin: InvocationOrigin,
        #[serde(default)]
        pub input: Value,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum InvocationOrigin {
        Button { node_key: String },
        CommandPalette,
        Agent { suggestion_id: String },
        /// L7-AGENT: Project の Terminal の中の agent（か人）が `shibubu project ask` で頼んだ（誰の編集か）。`terminal_id` = `TerminalSessionSpec::id`
        /// （子の `terminal::TERMINAL_ENV`・Terminal の外は空）・`agent` = 祖先の process の `terminal::agent_name`（無ければ None = shell で人が打った）。
        /// Wave: L7
        Terminal {
            terminal_id: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            agent: Option<String>,
        },
    }

    impl InvocationOrigin {
        /// L7-AGENT: 頼んだ者の人の言葉（承認の card・Runs・`shibubu project context`）。例: `claude in a terminal`・`a terminal`。
        pub fn who(&self) -> String {
            match self {
                Self::Button { .. } => "You pressed a button".into(),
                Self::CommandPalette => "You used ⌘K".into(),
                Self::Agent { .. } => "The assistant".into(),
                Self::Terminal { agent: Some(a), .. } => format!("{a} in a terminal"),
                Self::Terminal { agent: None, .. } => "A terminal".into(),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum InvocationError {
        UnknownAction(String),
        NotReady { action: String, readiness: ActionReadiness },
        CapabilityMissing { action: String, capability_ref: String },
        CapabilityNotReady { capability: String, readiness: CapabilityReadiness },
        InputNotObject,
        MissingInput { field: String },
        InputType { field: String, expected: String },
    }

    impl fmt::Display for InvocationError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::UnknownAction(id) => write!(f, "action '{id}' は定義に無い"),
                Self::NotReady { action, readiness } => write!(f, "action '{action}' は {readiness:?} で実行できない"),
                Self::CapabilityMissing { action, capability_ref } => {
                    write!(f, "action '{action}' の capability_ref '{capability_ref}' は定義に無い")
                }
                Self::CapabilityNotReady { capability, readiness } => {
                    write!(f, "capability '{capability}' は {readiness:?} で実行できない")
                }
                Self::InputNotObject => write!(f, "input は object でなければならない"),
                Self::MissingInput { field } => write!(f, "input に必須の '{field}' が無い"),
                Self::InputType { field, expected } => write!(f, "input の '{field}' は {expected} でなければならない"),
            }
        }
    }
    impl std::error::Error for InvocationError {}

    /// 唯一の入口。action id の完全一致 → readiness → capability の readiness → input_schema。
    /// 通れば実行してよい ActionDefinition を返す（実行そのものは runtime unit）。
    pub fn validate_invocation<'a>(
        def: &'a AppDefinitionV3,
        inv: &ActionInvocation,
    ) -> Result<&'a ActionDefinition, InvocationError> {
        let action = def
            .action(&inv.action_ref)
            .ok_or_else(|| InvocationError::UnknownAction(inv.action_ref.clone()))?;
        if action.readiness != ActionReadiness::Ready {
            return Err(InvocationError::NotReady { action: action.id.clone(), readiness: action.readiness });
        }
        if let Some(cap_ref) = &action.capability_ref {
            let cap = def.capability(cap_ref).ok_or_else(|| InvocationError::CapabilityMissing {
                action: action.id.clone(),
                capability_ref: cap_ref.clone(),
            })?;
            if cap.readiness != CapabilityReadiness::Ready {
                return Err(InvocationError::CapabilityNotReady { capability: cap.id.clone(), readiness: cap.readiness });
            }
        }
        check_input(&action.input_schema, &inv.input)?;
        Ok(action)
    }

    // ponytail: JSON Schema の部分集合（type: object / required / properties.type）だけ。full な検査は validator crate を足した時
    pub fn check_input(schema: &Map<String, Value>, input: &Value) -> Result<(), InvocationError> {
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Ok(());
        }
        let Some(obj) = input.as_object() else {
            return Err(InvocationError::InputNotObject);
        };
        for name in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
            let Some(name) = name.as_str() else { continue };
            if !obj.contains_key(name) {
                return Err(InvocationError::MissingInput { field: name.into() });
            }
        }
        for (name, prop) in schema.get("properties").and_then(Value::as_object).into_iter().flatten() {
            let (Some(expected), Some(value)) = (prop.get("type").and_then(Value::as_str), obj.get(name)) else {
                continue;
            };
            let ok = match expected {
                "string" => value.is_string(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                "null" => value.is_null(),
                _ => true,
            };
            if !ok {
                return Err(InvocationError::InputType { field: name.clone(), expected: expected.into() });
            }
        }
        Ok(())
    }
} // mod action

// ---- app（旧 domain/app.rs）----
pub mod app {
    //! AppDefinitionV3 — docs/schemas/agent_native_app_v3.schema.json を field 名そのまま serde 型にした物（CORE-001）。
    //! schema に無い field は足さない。`schema_version` が "3.0" 以外なら deserialize が明示 error になる。
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};
    use std::collections::BTreeSet;

    use super::action::ActionDefinition;
    use super::binding::Source;
    use super::capability::CapabilityDescriptor;
    use super::ui::UiNode;

    /// `"schema_version": "3.0"` の const。他の値は serde が `unknown variant` で落とす。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum SchemaVersion {
        #[serde(rename = "3.0")]
        V3,
    }

    /// TODO.md CORE-001 の概念名との対応: intent=goal / ui_tree=composition.root / data_sources=sources /
    /// agent_policy=agent / readiness=state.readiness / permissions=actions[].required_scopes + capabilities[].required_scopes + agent.policy_scope。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AppDefinitionV3 {
        pub schema_version: SchemaVersion,
        pub id: String,
        pub version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        pub goal: Goal,
        pub composition: Composition,
        #[serde(default)]
        pub capabilities: Vec<CapabilityDescriptor>,
        pub actions: Vec<ActionDefinition>,
        pub sources: Vec<Source>,
        #[serde(default)]
        pub workflows: Vec<Workflow>,
        pub agent: AgentPolicy,
        pub state: AppState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub metadata: Option<Map<String, Value>>,
    }

    impl AppDefinitionV3 {
        /// id の完全一致だけ。名前の類似で引かない（CORE-002 AC）。
        pub fn action(&self, id: &str) -> Option<&ActionDefinition> {
            self.actions.iter().find(|a| a.id == id)
        }
        pub fn source(&self, id: &str) -> Option<&Source> {
            self.sources.iter().find(|s| s.id == id)
        }
        pub fn capability(&self, id: &str) -> Option<&CapabilityDescriptor> {
            self.capabilities.iter().find(|c| c.id == id)
        }
        pub fn workflow(&self, id: &str) -> Option<&Workflow> {
            self.workflows.iter().find(|w| w.id == id)
        }
        /// この app が宣言している scope の全体（permission→scope の検査はこの集合に対して行う）:
        /// actions[].required_scopes ∪ capabilities[].required_scopes ∪ agent.policy_scope。
        pub fn declared_scopes(&self) -> BTreeSet<&str> {
            self.actions
                .iter()
                .flat_map(|a| a.required_scopes.iter())
                .chain(self.capabilities.iter().flat_map(|c| c.required_scopes.iter()))
                .chain(self.agent.policy_scope.iter())
                .map(String::as_str)
                .collect()
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Goal {
        pub literal_goal: String,
        pub outcomes: Vec<String>,
        #[serde(default)]
        pub assumptions: Vec<String>,
        #[serde(default)]
        pub constraints: Vec<String>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Composition {
        pub root: UiNode,
        #[serde(default)]
        pub view_modes: Vec<ViewMode>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ViewMode {
        Use,
        Build,
        Inspect,
        Run,
        Review,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Workflow {
        pub id: String,
        pub action_refs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub approval: Option<String>,
    }

    /// schema の `agent`。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AgentPolicy {
        pub suggestions_enabled: bool,
        pub autonomy_level: AutonomyLevel,
        pub entrypoint: String,
        #[serde(default)]
        pub policy_scope: Vec<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AutonomyLevel {
        SuggestOnly,
        AutoLocalReversible,
        AskBeforeExternalEffects,
    }

    /// schema の `state`。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AppState {
        pub readiness: AppReadiness,
        pub execution_mode: ExecutionMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub status_message: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AppReadiness {
        Preview,
        Partial,
        Ready,
        Blocked,
        Error,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ExecutionMode {
        LocalDemo,
        LocalReady,
        Integrated,
        Disabled,
    }
} // mod app

// ---- binding（旧 domain/binding.rs）----
pub mod binding {
    //! DataBinding と Source（CORE-004）。binding は node が「どの source のどの path を映すか」。
    //! 中身が無い・古い・未接続の時は BindingReadiness で言う。内容を作って埋めない。
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    use super::capability::DisplayTier;

    /// schema `$defs.uiNode.binding`（source_ref / path 必須。schema は追加 key を許すので extra に残す）。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DataBinding {
        pub source_ref: String,
        pub path: String,
        #[serde(flatten)]
        pub extra: Map<String, Value>,
    }

    impl DataBinding {
        /// 表示前の判定。`stale_before` は RFC3339 UTC（`...Z`）の文字列で、
        /// source.last_observed_at がそれより古い（または無い）ら Stale。None なら鮮度は見ない。
        // ponytail: RFC3339 "Z" 同士の辞書順比較。offset 付きが混ざるなら時刻型に上げる
        pub fn readiness(&self, sources: &[Source], stale_before: Option<&str>) -> BindingReadiness {
            use SourceReadiness::*;
            let Some(source) = sources.iter().find(|s| s.id == self.source_ref) else {
                return BindingReadiness::MissingSource(self.source_ref.clone());
            };
            match source.readiness {
                Local => BindingReadiness::Ready,
                Ready => match (stale_before, source.last_observed_at.as_deref()) {
                    (Some(limit), Some(seen)) if seen.as_bytes() >= limit.as_bytes() => BindingReadiness::Ready,
                    (Some(_), _) => BindingReadiness::Stale,
                    (None, _) => BindingReadiness::Ready,
                },
                RequiresConnection => BindingReadiness::RequiresConnection,
                ConnectedUnverified => BindingReadiness::Unverified,
                Degraded => BindingReadiness::Stale,
                Unsupported => BindingReadiness::Unsupported,
            }
        }
    }

    /// renderer はこれを UI-007 の状態語に写す（Ready 以外で中身を描かない）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum BindingReadiness {
        Ready,
        RequiresConnection,
        Unverified,
        Stale,
        Unsupported,
        MissingSource(String),
    }

    /// schema `sources[]`。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Source {
        pub id: String,
        pub kind: String,
        pub readiness: SourceReadiness,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub last_observed_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub provider: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum SourceReadiness {
        Local,
        RequiresConnection,
        ConnectedUnverified,
        Ready,
        Degraded,
        Unsupported,
    }

    impl SourceReadiness {
        /// source の readiness → 段（CD-02 (2)。local は接続が要らないので Ready）。
        pub const fn tier(self) -> DisplayTier {
            match self {
                Self::Local | Self::Ready => DisplayTier::Ready,
                Self::RequiresConnection => DisplayTier::Available,
                Self::ConnectedUnverified | Self::Degraded => DisplayTier::Connected,
                Self::Unsupported => DisplayTier::Unavailable,
            }
        }
    }
} // mod binding

// ---- patch（旧 domain/patch.rs）----
pub mod patch {
    //! AppRevision（不変・検証済み）と AppPatch（insert / remove / update / move / bind）、原子的な apply（CORE-006）。
    //! AppPatch の形は agent_proposal_v1 の `change.operations[]` そのもの（op / target_key / value）。
    //!
    //! op ごとの value:
    //! - insert: UiNode（target_key の children の末尾に足す。位置は move で）
    //! - remove: 無し（root は外せない）
    //! - update: node の field の部分 object（key / children は変えられない。null で Option を外す）
    //! - move:   {"parent": key, "index": n?}（index 無しは末尾。自分の子孫へは Cycle）
    //! - bind:   {"binding": DataBinding|null} と / または {"action_ref": id|null}
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::collections::HashSet;
    use std::fmt;

    use super::app::AppDefinitionV3;
    use super::ui::UiNode;
    use super::validate::{validate, ValidationError};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AppPatch {
        pub op: PatchOp,
        pub target_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub value: Option<Value>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PatchOp {
        Insert,
        Remove,
        Update,
        Move,
        Bind,
    }

    /// 検証を通った定義だけが revision になる。field は private で、`apply` が新しい revision を作る（元は変わらない）。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AppRevision {
        definition: AppDefinitionV3,
    }

    impl AppRevision {
        pub fn new(definition: AppDefinitionV3) -> Result<Self, Vec<ValidationError>> {
            validate(&definition)?;
            Ok(Self { definition })
        }
        pub fn definition(&self) -> &AppDefinitionV3 {
            &self.definition
        }
        pub fn version(&self) -> u32 {
            self.definition.version
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum PatchError {
        TargetNotFound { op: PatchOp, key: String },
        RootImmovable { op: PatchOp, key: String },
        DuplicateKey(String),
        InvalidValue { op: PatchOp, key: String, reason: String },
        Cycle { key: String, into: String },
        /// 全 patch を当てた後の定義が CORE-008 の検査に落ちた（参照切れ等）。
        Invalid(Vec<ValidationError>),
    }

    impl fmt::Display for PatchError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::TargetNotFound { op, key } => write!(f, "{op:?}: target_key '{key}' が無い"),
                Self::RootImmovable { op, key } => write!(f, "{op:?}: root '{key}' は remove / move できない"),
                Self::DuplicateKey(key) => write!(f, "insert: key '{key}' は既にある"),
                Self::InvalidValue { op, key, reason } => write!(f, "{op:?} '{key}': value が不正 — {reason}"),
                Self::Cycle { key, into } => write!(f, "move: '{key}' を自分の子孫 '{into}' の下へは動かせない"),
                Self::Invalid(errs) => {
                    write!(f, "patch 後の定義が不正:")?;
                    for e in errs {
                        write!(f, " [{e}]")?;
                    }
                    Ok(())
                }
            }
        }
    }
    impl std::error::Error for PatchError {}

    /// 全部当たるか、何も変わらないか。途中で 1 つでも落ちたら Err で、`base` は触られない（clone に当てて最後に検証）。
    pub fn apply(base: &AppRevision, patches: &[AppPatch]) -> Result<AppRevision, PatchError> {
        let mut def = base.definition.clone();
        for patch in patches {
            apply_one(&mut def.composition.root, patch)?;
        }
        def.version += 1;
        AppRevision::new(def).map_err(PatchError::Invalid)
    }

    /// 1 つの木に 1 つ当てる（検証はしない）。Project の Page の木にも同じ op を使う（project.rs）。
    pub(super) fn apply_one(root: &mut UiNode, patch: &AppPatch) -> Result<(), PatchError> {
        let op = patch.op;
        let key = patch.target_key.as_str();
        let not_found = |k: &str| PatchError::TargetNotFound { op, key: k.into() };
        let invalid = |reason: String| PatchError::InvalidValue { op, key: key.into(), reason };
        let value = || patch.value.as_ref().ok_or_else(|| invalid("value が無い".into()));
        match op {
            PatchOp::Insert => {
                let node: UiNode = serde_json::from_value(value()?.clone()).map_err(|e| invalid(e.to_string()))?;
                let existing: HashSet<&str> = root.nodes().iter().map(|n| n.key.as_str()).collect();
                if let Some(dup) = node.nodes().iter().find(|n| existing.contains(n.key.as_str())) {
                    return Err(PatchError::DuplicateKey(dup.key.clone()));
                }
                root.find_mut(key).ok_or_else(|| not_found(key))?.children.push(node);
            }
            PatchOp::Remove => {
                if root.key == key {
                    return Err(PatchError::RootImmovable { op, key: key.into() });
                }
                root.detach(key).ok_or_else(|| not_found(key))?;
            }
            PatchOp::Update => {
                let fields = value()?.as_object().ok_or_else(|| invalid("value は object".into()))?;
                if fields.contains_key("key") || fields.contains_key("children") {
                    return Err(invalid("key / children は update で変えない（insert / remove / move を使う）".into()));
                }
                let node = root.find_mut(key).ok_or_else(|| not_found(key))?;
                let mut json = serde_json::to_value(&*node).map_err(|e| invalid(e.to_string()))?;
                let obj = json.as_object_mut().ok_or_else(|| invalid("node が object でない".into()))?;
                for (k, v) in fields {
                    obj.insert(k.clone(), v.clone());
                }
                *node = serde_json::from_value(json).map_err(|e| invalid(e.to_string()))?;
            }
            PatchOp::Move => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct MoveTo {
                    parent: String,
                    #[serde(default)]
                    index: Option<usize>,
                }
                let to: MoveTo = serde_json::from_value(value()?.clone()).map_err(|e| invalid(e.to_string()))?;
                if root.key == key {
                    return Err(PatchError::RootImmovable { op, key: key.into() });
                }
                let target = root.find(key).ok_or_else(|| not_found(key))?;
                if target.nodes().iter().any(|n| n.key == to.parent) {
                    return Err(PatchError::Cycle { key: key.into(), into: to.parent });
                }
                let node = root.detach(key).ok_or_else(|| not_found(key))?;
                let parent = root.find_mut(&to.parent).ok_or_else(|| not_found(&to.parent))?;
                let at = to.index.unwrap_or(parent.children.len()).min(parent.children.len());
                parent.children.insert(at, node);
            }
            PatchOp::Bind => {
                let fields = value()?.as_object().ok_or_else(|| invalid("value は object".into()))?;
                if fields.is_empty() || fields.keys().any(|k| k != "binding" && k != "action_ref") {
                    return Err(invalid("value は binding / action_ref だけを持つ".into()));
                }
                let node = root.find_mut(key).ok_or_else(|| not_found(key))?;
                if let Some(b) = fields.get("binding") {
                    node.binding = serde_json::from_value(b.clone()).map_err(|e| invalid(e.to_string()))?;
                }
                if let Some(a) = fields.get("action_ref") {
                    node.action_ref = serde_json::from_value(a.clone()).map_err(|e| invalid(e.to_string()))?;
                }
            }
        }
        Ok(())
    }
} // mod patch

// ---- permission（旧 domain/permission.rs）----
pub mod permission {
    //! PermissionGrant と ConsentReceipt（CORE-007）。
    //! grant の scope は app が宣言した scope（AppDefinitionV3::declared_scopes）の 1 つ。検査は validate::validate_grant。
    //! 承認（ConsentReceipt）は「見せた payload / scope に対する決定」の記録。見せていない物の実行許可にはならない（DESIGN §5.3）。
    use serde::{Deserialize, Serialize};

    /// RQ-023: Engine の Automation の 3 択（Settings › Automation・既定 `AskBeforeWrite` = 無限定の自動を初期にしない）。Engine の承認の門
    /// （`capability_engine::EngineContext::automation`）が Execution の前に読む = 門は 1 つ（exact payload の承認・WF-006 の宛先の読み直しと同じ所）。
    /// 例: `EngineContext { automation: store.ui.automation, .. }`。保存の形は snake_case（`"ask_before_write"`）。
    /// 保証する: `Suggest` = 計画と操作の前の読みまでで止まり 1 手も送らない（Denied の 1 行）・`TrustedReadOnly` = 書く手を含む計画は 1 手も送らない
    /// （読むだけの計画は今まで通り走る）・`AskBeforeWrite` = 今まで通り。保証しない: 同時実行・タイムアウト・再試行の設定（既定の値のまま）。
    /// `AutonomyLevel`（生成した app の agent の方針）に足さない理由: あちらの 3 つ目は「元に戻せる手元の書き込みは自動」で、Engine の 3 択と 1 対 1 にならない。
    /// Wave: WF-E
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AutomationMode {
        /// 計画を見せるだけ（何も送らない）
        Suggest,
        /// 読む手はそのまま・書く手は見せた exact payload の承認の後（今まで通り）
        #[default]
        AskBeforeWrite,
        /// 読む手だけ（書く手を含む計画は 1 手も送らない）
        TrustedReadOnly,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PermissionGrant {
        pub id: String,
        pub app_id: String,
        pub scope: String,
        pub granted_at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub consent_ref: Option<String>,
    }

    impl PermissionGrant {
        /// `now` は RFC3339 UTC（`...Z`）。期限が無ければ有効。
        // ponytail: RFC3339 "Z" 同士の辞書順比較（`binding` と同じ前提）
        pub fn active_at(&self, now: &str) -> bool {
            self.expires_at.as_deref().is_none_or(|e| now.as_bytes() < e.as_bytes())
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ConsentReceipt {
        pub id: String,
        pub app_id: String,
        pub subject: ConsentSubject,
        /// 見せて決めてもらった scope。
        pub scopes: Vec<String>,
        /// 見せた payload の digest（外部送信・破壊的操作は exact payload の承認が要る）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub payload_digest: Option<String>,
        pub decision: ConsentDecision,
        pub decided_at: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum ConsentSubject {
        Suggestion { id: String },
        Action { action_ref: String, run_id: Option<String> },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConsentDecision {
        Approved,
        Denied,
    }
} // mod permission

// ---- run（旧 domain/run.rs）----
pub mod run {
    //! Run / Step / Attempt / Observation / Receipt / ErrorCode（CORE-007）と run 状態遷移の中央検証。
    //! 状態は DESIGN.md §4.5 の 10 個。許される遷移は RUN_TRANSITIONS の表 1 枚（docs/diagrams.md §6 と同じ物）。
    //! 時刻は schema に合わせて RFC3339 UTC の文字列。
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::fmt;

    use super::action::InvocationOrigin;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum RunStatus {
        Idle,
        Planning,
        WaitingConnection,
        WaitingApproval,
        Running,
        Verifying,
        Verified,
        Failed,
        Cancelled,
        Uncertain,
        /// WF0: Engine の手順の途中で人の判断（`capability_engine::StepKind::Decide`）を待っている。前の手は走った・後ろの手は送っていない。
        /// 再起動しても待ったまま（`receipt::recover` は触らない）・続きは `RunRecord::workflow` の cursor から（docs/diagrams.md §47）
        /// Wave: WF0
        WaitingHuman,
    }

    /// (from, to)。AGENT_NATIVE_RUNTIME §3.1 の形を §4.5 の名前で。
    /// cancelled は副作用前（idle / planning / waiting_* / running）からだけ。verifying 以降は「起きた事」なので取り消せない（RUN-008）。
    /// uncertain → verifying は再照合（DATA_CONTRACTS §7.1）。
    /// WF0: Engine の手順の途中の承認・判断（running → waiting_approval / waiting_human → running は cursor の続き・最初から走らせ直さない）。
    pub const RUN_TRANSITIONS: &[(RunStatus, RunStatus)] = {
        use RunStatus::*;
        &[
            (Idle, Planning),
            (Idle, Cancelled),
            (Planning, WaitingConnection),
            (Planning, WaitingApproval),
            (Planning, Running),
            (Planning, Failed),
            (Planning, Cancelled),
            (WaitingConnection, WaitingApproval),
            (WaitingConnection, Running),
            (WaitingConnection, Failed),
            (WaitingConnection, Cancelled),
            (WaitingApproval, Running),
            (WaitingApproval, Failed),
            (WaitingApproval, Cancelled),
            (Running, Verifying),
            (Running, Failed),
            (Running, Uncertain),
            (Running, Cancelled),
            (Verifying, Verified),
            (Verifying, Failed),
            (Verifying, Uncertain),
            (Uncertain, Verifying),
            (Running, WaitingApproval),
            (Running, WaitingHuman),
            (WaitingHuman, Running),
            (WaitingHuman, Cancelled),
        ]
    };

    /// WF0: Engine の手順の run の続き（runs 表の `RunRecord::workflow`）。前の手の出力は、読み直せる手（読む action）は digest だけを持ち
    /// 続ける前に元の情報源から読み直す（WF-004）・書いた手の出力は後ろの手が `$from` で使う時だけ値を持つ。凍結した手順と
    /// Engine の記録は `RunRecord::input`（`capability_engine::EngineRun`）。
    /// Wave: WF0
    /// 例: `WorkflowCursor { digest, plan: 0, next: 2, outputs, waiting: Some("pick".into()), seen }`（2 手が済み、3 手目の Decide で待っている）
    /// 保証する: 未知の欄は拒否・`waiting` が Some の時だけ run は `RunStatus::WaitingHuman` か `WaitingApproval`（人を待つ手の id）。
    /// 保証しない: 元データが今も同じ事（続ける前に `seen` と読み直した物を比べる・違えば 1 手も送らない）。
    #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct WorkflowCursor {
        /// 走り始めた時の保存した手順の file の sha256（無ければ空）。続ける時に違えば送らない（WF-002）
        pub digest: String,
        /// 今の操作（Engine の 1 一言の操作の番号）
        pub plan: usize,
        /// 次に走らせる手の番号（待っている間は待っている手）
        pub next: usize,
        /// 済んだ手の出力（手の id → payload か `{"$digest": sha256}`）。手元の sqlite だけに置く
        #[serde(default)]
        pub outputs: std::collections::BTreeMap<String, Value>,
        /// 人を待っている手の id
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub waiting: Option<String>,
        /// 止まった時に読んだ元データ（`Workflow::checks` の pointer の値）の sha256・同じ順。続ける前に読み直して比べる（WF-015）
        #[serde(default)]
        pub seen: Vec<String>,
    }

    impl RunStatus {
        pub fn can_transition(self, to: RunStatus) -> bool {
            RUN_TRANSITIONS.contains(&(self, to))
        }
        /// 唯一の遷移口。Run / Step はこれを呼ぶ。
        pub fn transition(&mut self, to: RunStatus) -> Result<(), TransitionError<RunStatus>> {
            *self = transition_in(RUN_TRANSITIONS, *self, to)?;
            Ok(())
        }
        pub fn is_terminal(self) -> bool {
            matches!(self, Self::Verified | Self::Failed | Self::Cancelled)
        }
    }

    /// run と提案で共用する遷移検査。表に無い (from, to) は Err。
    pub(super) fn transition_in<S: Copy + PartialEq>(table: &[(S, S)], from: S, to: S) -> Result<S, TransitionError<S>> {
        if table.contains(&(from, to)) {
            Ok(to)
        } else {
            Err(TransitionError { from, to })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TransitionError<S> {
        pub from: S,
        pub to: S,
    }

    impl<S: fmt::Debug> fmt::Display for TransitionError<S> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:?} から {:?} へは遷移できない", self.from, self.to)
        }
    }
    impl<S: fmt::Debug> std::error::Error for TransitionError<S> {}

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Run {
        pub id: String,
        pub app_id: String,
        pub app_version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub workflow_ref: Option<String>,
        pub origin: InvocationOrigin,
        pub status: RunStatus,
        pub steps: Vec<Step>,
        pub created_at: String,
        pub updated_at: String,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Step {
        pub id: String,
        pub action_ref: String,
        pub status: RunStatus,
        #[serde(default)]
        pub attempts: Vec<Attempt>,
    }

    /// 1 回の試行。「試みた」と「終わった」は別（RUN-004）: outcome が Verified の時だけ完了。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Attempt {
        pub seq: u32,
        pub started_at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub finished_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub outcome: Option<Outcome>,
    }

    /// DATA_CONTRACTS §2.1 の構造化 Result。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum Outcome {
        Verified { evidence: Vec<String> },
        Unverified { reason: String },
        Rejected { reason: String },
        Failed { error: ErrorCode },
        EffectUnknown { reason: String },
    }

    /// source から観測した事実。推論ではない。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Observation {
        pub id: String,
        pub source_ref: String,
        pub observed_at: String,
        pub payload: Value,
    }

    /// 副作用の証拠（RUN-004）。observation_refs が verified の根拠。
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Receipt {
        pub id: String,
        pub run_id: String,
        pub step_id: String,
        pub attempt_seq: u32,
        pub action_ref: String,
        pub outcome: Outcome,
        #[serde(default)]
        pub observation_refs: Vec<String>,
        pub recorded_at: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ErrorCode {
        NotReady,
        PermissionDenied,
        ApprovalDenied,
        InvalidInput,
        ConnectionLost,
        Timeout,
        RateLimited,
        TargetNotFound,
        TargetAmbiguous,
        DriverRefused,
        VerificationFailed,
        Cancelled,
        Internal,
    }
} // mod run

// ---- suggestion（旧 domain/suggestion.rs）----
pub mod suggestion {
    //! Suggestion — docs/schemas/agent_proposal_v1.schema.json を field 名そのまま serde 型にした物（CORE-007 の Suggestion）。
    //! 状態は schema の 12 値（TODO AGT-002 の鎖と同じ）。許される遷移は SUGGESTION_TRANSITIONS の表 1 枚（docs/diagrams.md §6）。
    //! DESIGN.md §5.3 の名前との対応: detected≈candidate / eligible≈validated→queued / presented≈shown / dismissed≈rejected。
    //! §5.3 の stale / blocked / withdrawn は schema に値が無いので表せない（caveat）。
    use serde::{Deserialize, Serialize};

    use super::patch::AppPatch;
    use super::run::{transition_in, TransitionError};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ProposalSchemaVersion {
        #[serde(rename = "1.0")]
        V1,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Suggestion {
        pub schema_version: ProposalSchemaVersion,
        pub id: String,
        pub app_id: String,
        pub app_version: u32,
        pub title: String,
        pub reason: String,
        pub evidence: Vec<Evidence>,
        pub target_ref: String,
        pub kind: SuggestionKind,
        /// 自由文（「検索入力を一覧の上に追加」）。action の Effect enum とは別物。
        pub effect: String,
        pub risk: Risk,
        pub permission_scopes: Vec<String>,
        pub undoable: bool,
        pub status: SuggestionStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<String>,
        pub change: Change,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Evidence {
        pub kind: EvidenceKind,
        pub r#ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub observed_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub uncertain: Option<bool>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum EvidenceKind {
        UserGoal,
        AppState,
        Event,
        SourceRecord,
        TestFailure,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum SuggestionKind {
        UiPatch,
        CapabilitySuggestion,
        WorkflowPatch,
        DataFix,
        ConnectionRequest,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Risk {
        None,
        LocalReversible,
        ExternalRead,
        ExternalWrite,
        ExternalSend,
        Destructive,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Change {
        pub summary: String,
        pub operations: Vec<AppPatch>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum SuggestionStatus {
        Candidate,
        Validated,
        Queued,
        Shown,
        Previewed,
        Approved,
        Rejected,
        Snoozed,
        Applied,
        Verified,
        Failed,
        Reverted,
    }

    /// (from, to)。AGT-002: candidate→validated→queued→shown→previewed→approved/rejected/snoozed→applied→verified/failed→reverted。
    /// snoozed は期限後に queued へ戻る。rejected は終端（AGT-007: すぐ再出現しない）。
    pub const SUGGESTION_TRANSITIONS: &[(SuggestionStatus, SuggestionStatus)] = {
        use SuggestionStatus::*;
        &[
            (Candidate, Validated),
            (Candidate, Rejected),
            (Validated, Queued),
            (Validated, Rejected),
            (Queued, Shown),
            (Queued, Rejected),
            (Queued, Snoozed),
            (Shown, Previewed),
            (Shown, Approved),
            (Shown, Rejected),
            (Shown, Snoozed),
            (Previewed, Approved),
            (Previewed, Rejected),
            (Previewed, Snoozed),
            (Snoozed, Queued),
            (Snoozed, Rejected),
            (Approved, Applied),
            (Approved, Failed),
            (Applied, Verified),
            (Applied, Failed),
            (Verified, Reverted),
            (Failed, Reverted),
        ]
    };

    impl SuggestionStatus {
        pub fn can_transition(self, to: SuggestionStatus) -> bool {
            SUGGESTION_TRANSITIONS.contains(&(self, to))
        }
        /// 唯一の遷移口。
        pub fn transition(&mut self, to: SuggestionStatus) -> Result<(), TransitionError<SuggestionStatus>> {
            *self = transition_in(SUGGESTION_TRANSITIONS, *self, to)?;
            Ok(())
        }
        pub fn is_terminal(self) -> bool {
            matches!(self, Self::Rejected | Self::Reverted)
        }
    }
} // mod suggestion

// ---- ui（旧 domain/ui.rs）----
pub mod ui {
    //! UiNode — schema `$defs.uiNode` を field 名そのまま再帰型にした物（CORE-005）。
    //! 見える要素（UiNode）と実行の定義（action::ActionDefinition）は別の型。node は `action_ref` で id を指すだけ。
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    use super::binding::DataBinding;

    /// renderer が描ける component（UI-005 の一覧 + examples / proposal が使う物 + DESIGN §4.4 の `source_surface` /
    /// §7 の `waveform` / corpus（docs/examples/v3）の `recorder` + 移行の placeholder `extension`）。
    /// validate はこの表に無い component を `UnsupportedComponent` にする。renderer unit が増やす時はここに足す。
    pub const BUILTIN_COMPONENTS: &[&str] = &[
        "text",
        "list",
        "table",
        "detail",
        "form",
        "chat",
        "calendar",
        "timeline",
        "media",
        "chart",
        "tabs",
        "split",
        "editor",
        "command_input",
        "run_progress",
        "button",
        "input",
        "column",
        "row",
        "recorder",
        "source_surface",
        "waveform",
        "extension",
        // W5: Project の Terminal の席（`props.terminal_ref` = `TerminalSessionSpec::id`）。照合は `project::validate_project`。
        // app の木（Project の外）に置いても指す先が無いので、描く側は「Project の Page にだけ置けます」の 1 行を出す。
        "terminal",
        // AI-COMPOSE-1101: AI が HTML / SVG で書く部品（props は `CanvasProps`・照合は `project::validate_project`）。
        "canvas",
    ];

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UiNode {
        pub key: String,
        pub component: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
        #[serde(default)]
        pub props: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub layout: Option<Map<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub binding: Option<DataBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub action_ref: Option<String>,
        #[serde(default)]
        pub children: Vec<UiNode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub states: Option<Map<String, Value>>,
        #[serde(default)]
        pub user_locked: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub visibility: Option<Map<String, Value>>,
    }

    impl UiNode {
        /// 自分を含む全 node を先行順で並べる。
        pub fn nodes(&self) -> Vec<&UiNode> {
            let mut out = Vec::new();
            fn walk<'a>(node: &'a UiNode, out: &mut Vec<&'a UiNode>) {
                out.push(node);
                for child in &node.children {
                    walk(child, out);
                }
            }
            walk(self, &mut out);
            out
        }
        /// key の完全一致で 1 つ引く。
        pub fn find(&self, key: &str) -> Option<&UiNode> {
            self.nodes().into_iter().find(|n| n.key == key)
        }
        pub fn find_mut(&mut self, key: &str) -> Option<&mut UiNode> {
            if self.key == key {
                return Some(self);
            }
            self.children.iter_mut().find_map(|c| c.find_mut(key))
        }
        /// key の node を木から外して返す（root 自身は外せない: None）。
        pub fn detach(&mut self, key: &str) -> Option<UiNode> {
            if let Some(i) = self.children.iter().position(|c| c.key == key) {
                return Some(self.children.remove(i));
            }
            self.children.iter_mut().find_map(|c| c.detach(key))
        }
    }

    /// AI-COMPOSE-1101: `canvas` の html の上限（byte）。超える部品は `validate_project` が拒否（Page の json と WebView に大きな物を積まない）。
    pub const CANVAS_HTML_MAX: usize = 64 * 1024;
    /// ENGINE-GENERIC-1101（owner 211）: きっかけに出来る部品の手 = (手を起こす部品の component, 手の名前)。`PartTrigger::act` = `<その部品の key>.<手の名前>`。
    /// 部品の手が起きた後に `Generated::part_event` が Project のきっかけの言葉と状況を Engine の 1 つの口へ渡す（何をするかは利用者の一言から会話の AI が
    /// 書いた言葉・製品は手の名前しか持たない）。`recorded` = 録音の帯が録り終えた。手を足す時はここと、その手を `part_event` に渡す 1 行を一緒に。
    /// 形を (component, 手) にした理由: 会話の AI の prompt に「どの部品のどの手か」を部品の種類から一般に並べ、保存の前の検査（`canvas_fault`）が
    /// 起きない手（帯でない部品の `recorded`）を拒否するため。AI-COMPOSE-1101 の `add`（Add take の前に当てる = 製品が take を入れる手を持つ道）は取り下げた。
    pub const TRIGGER_ACTS: &[(&str, &str)] = &[("recorder", "recorded")];

    /// AI-COMPOSE-1101（owner 206）: `canvas` の props = AI が HTML / SVG で書く Page の部品（CHAT-FREE 段 2 の閉じた WebView の card を Page に置いた物）。
    /// 部品の中の script が言えるのは `shibubu.read(app, what)`（app を読むだけ）・`shibubu.set(key, value)`（値を `values` に残す）・`shibubu.act(intent, value)`
    /// （Engine に頼む・承認の決まりは今のまま）の 3 つ（`surface::CanvasBridge::Page`）。`on` = 部品の手（`TRIGGER_ACTS`）が起きた後に Engine に頼むきっかけ。
    /// 新しい型の理由: 今の部品は描き方が決まった種類だけで、AI が中身を書く部品・その値・手の前に当てる約束を持つ物が無い（node の props の形だけを型にした・
    /// 保存は今の `UiNode::props` のまま = Project の json・`PatchOp` は増やさない）。
    /// 例（Hum to Guitar）: `{"component": "canvas", "props": {"html": "<select id=g></select><script>shibubu.read(app, 'guitar sounds').then(…)</script>",
    /// "values": {"guitar": "Acoustic Guitar"}, "on": [{"act": "hum.recorded", "key": "guitar", "intent": "Set the selected track's sound to {value}, then put the take on it"}]}}`
    /// ENGINE-GENERIC-1101: 欄の名前を `before` → `on`（手の前でなく後）。前の `before` の欄は読まない = 前の data のきっかけは起きない（`canvas_fault` も見ない）。
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct CanvasProps {
        pub html: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        pub values: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub on: Vec<PartTrigger>,
    }

    /// AI-COMPOSE-1101: 部品の手と Engine への一言を結ぶきっかけの 1 つ。`act` = `<手を持つ node の key>.<TRIGGER_ACTS の手>`（例 `hum.recorded` = 録り終えた）・
    /// `key` = 一言に入れる `CanvasProps::values` の key（空 = 値を使わない）・`intent` = 利用者の一言から会話の AI が書いた Engine への一言（`{value}` が値に替わる）。
    /// 新しい型の理由: 部品の値と別の部品の手を結ぶ物が今の木に無い（`action_ref` は node が自分の手を指すだけ）。
    /// ENGINE-GENERIC-1101: 手の後に走る（前は Add take の前）・`key` は空でよい（値を使わないきっかけ）。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PartTrigger {
        pub act: String,
        #[serde(default)]
        pub key: String,
        pub intent: String,
    }

    impl CanvasProps {
        /// node の props を読む（`canvas` でない・形が違う = None）。
        pub fn of(node: &UiNode) -> Option<Self> {
            (node.component == "canvas").then(|| serde_json::from_value(Value::Object(node.props.clone())).ok()).flatten()
        }
        /// `shibubu.set(key, value)` を残す 1 手（`AppPatch { op: Update, target_key: node の key, value: {"props": 全部} }`）。`Update` は node の欄を
        /// 丸ごと置き換える（`patch::apply_one`）ので、html と on も一緒に送る（他の props も保つ）。同じ値なら None（revision を足さない）。
        /// 例: `if let Some(p) = CanvasProps::set_patch(node, "guitar", json!("Acoustic Guitar")) { ops.push(ProjectPatchOp::Page { page_id, patch: p }) }`
        pub fn set_patch(node: &UiNode, key: &str, value: Value) -> Option<super::patch::AppPatch> {
            let mut props = node.props.clone();
            let values = props.entry("values").or_insert_with(|| Value::Object(Map::new())).as_object_mut()?;
            if values.get(key) == Some(&value) {
                return None;
            }
            values.insert(key.to_owned(), value);
            Some(super::patch::AppPatch { op: super::patch::PatchOp::Update, target_key: node.key.clone(), value: Some(serde_json::json!({ "props": props })) })
        }
    }

    impl PartTrigger {
        /// Engine に渡す一言（`fill_value`）。例: `t.goal(&json!("Acoustic Guitar"))` → `"Set … sound to Acoustic Guitar"`
        pub fn goal(&self, value: &Value) -> String {
            fill_value(&self.intent, value)
        }
    }

    /// AI-COMPOSE-1101: 一言の `{value}` を値に替える（文字はそのまま・他の値は JSON の 1 行）。きっかけ（`PartTrigger::goal`）と部品の `shibubu.act` が同じ物を使う。
    pub fn fill_value(intent: &str, value: &Value) -> String {
        let v = value.as_str().map_or_else(|| value.to_string(), str::to_owned);
        intent.replace("{value}", &v)
    }

    /// AI-COMPOSE-1101: canvas の node が保存できない理由（None = 保存できる）。html が在り上限の中・きっかけは同じ Page（`page_root`）の部品の手
    /// （その node の component と手が `TRIGGER_ACTS` に在る）を指し、intent が在り、key が在る時だけ intent が値の穴 `{value}` を持つ（key が空なら持たない）。
    /// `validate_project` と編集の道（`composer::project` の add_part）が同じ物を見る。
    /// 例: `if let Some(why) = canvas_fault(node, &page.root) { errs.push(format!("Page '{}': canvas '{}' {why}", page.id, node.key)) }`
    pub fn canvas_fault(node: &UiNode, page_root: &UiNode) -> Option<String> {
        let Some(c) = CanvasProps::of(node) else { return Some("の props が読めない（html が要る）".into()) };
        if c.html.trim().is_empty() || c.html.len() > CANVAS_HTML_MAX {
            return Some(format!("の html が空か {CANVAS_HTML_MAX} byte を超える"));
        }
        let bad = |t: &&PartTrigger| {
            let at = t.act.rsplit_once('.').filter(|(k, a)| page_root.find(k).is_some_and(|n| TRIGGER_ACTS.contains(&(n.component.as_str(), *a))));
            at.is_none() || t.intent.trim().is_empty() || t.key.trim().is_empty() == t.intent.contains("{value}")
        };
        c.on.iter().find(bad).map(|t| format!("のきっかけ '{}' の手がこの Page の部品に無いか、intent が空か、key と intent の {{value}} が合わない", t.act))
    }

    /// ENGINE-GENERIC-1101: 部品の手 `act`（例 `hum.recorded`）の後に走るきっかけを Page の木から集める（canvas の node の key・きっかけ・
    /// 今の値〈key が空・まだ選んでいない = None〉）。例（`Generated::part_event`）: `for (part, t, v) in triggers_on(&page.root, "hum.recorded") { … }`
    /// 保証する: 木の順・形の違う canvas は飛ばす（`validate_project` が保存の前に拒否する）。保証しない: 一言が app に当たる事（Engine の run）。
    pub fn triggers_on(root: &UiNode, act: &str) -> Vec<(String, PartTrigger, Option<Value>)> {
        root.nodes()
            .into_iter()
            .filter_map(|n| CanvasProps::of(n).map(|p| (n.key.clone(), p)))
            .flat_map(|(key, p)| p.on.iter().filter(|t| t.act == act).map(|t| (key.clone(), t.clone(), p.values.get(&t.key).cloned())).collect::<Vec<_>>())
            .collect()
    }
} // mod ui

// ---- chat（G104: Home の欄に添える物と考える深さ）----
pub mod chat {
    //! G104（CHAT-001・CHAT-005）: Home の欄の `+` の menu（HOME-ATTACH-1101 から `Add files or photos` の 1 行・窓に落とした・⌘V で貼った file）で添える物と、`Effort ▸` の段。
    //! 添えた物は欄の下の小さな角丸の長方形（× で外す）で見せ、値で `Intent::Generate` に乗り、`Generated::start_create` が provider の依頼
    //! （`llm::JsonRequest` の `files`・`effort`）に写す。llm は domain に依らない（tests/llm.rs が単独で取り込む）ので型はここに置き、llm の側は path と文字だけ。
    //! 新しい型の理由: 今の型に file の在りか・種類・深さの段を持つ物が無い（`Intent::Generate` も `JsonRequest` も文字だけ）。
    //! HOME-ATTACH-1101（owner 180 通目「fileやzip, folderは全て同じカテゴリーのfileとして扱うように」「file, link, videoの表示はいらない」）: 種類は写真と file の 2 つ
    //! （folder・zip も file・写真だけ縮小絵）。動画と link の道は消した。
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};

    /// G86（CHAT-0xx・owner 10-27 11:2x「自分のプロンプトの履歴が見えるようにchat式で」）: Project の会話の列の 1 つ。人の一言か、Shibubu の答え
    /// （作った・変えた・送って確かめた・止めた・失敗）。保存は `chat_turns` の 1 行の json そのまま（storage は domain を引かない）。
    /// 承認・同意・進めている間は保存しない（その間は `EngineView` の card・済んだ時の 1 行だけをここに足す）。
    /// 新しい型の理由: 今の記録（`project_revisions`・Engine の `engine-runs.jsonl`・rail の `engine_history` の文）は一言の文と答えの組を持たない
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Turn {
        pub who: Who,
        pub text: String,
        /// 答えの印（人の一言は None）
        #[serde(default)]
        pub mark: Mark,
        /// RFC 3339（hover で出す）
        pub at: String,
        /// 一言に添えた物の名前（`Attachment::name`・吹き出しの中の token）
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub attachments: Vec<String>,
        /// L2（109 通目 15）: 答えの中の選択肢（押すとその文が次の一言）。人の一言は空。CHAT-FREE-1101: 数と有無は AI が決める（前の最大 3 をやめた・`local_talk` は 3 つ）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub choices: Vec<String>,
        /// L2: 案の文（`Accept` で作る文）。None = まだ作らない（挨拶・短い一言・`Later` を押した）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub proposal: Option<String>,
        /// L2（111 通目・handoff 05 の Proposal → Preview → Accept）: `Preview plan` で開いた読むだけの行（何も書かない）。空 = まだ開いていない（`Accept` は出さない）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub preview: Vec<String>,
        /// TALK-1101（owner 190・191 通目）: `Preview plan` で開く見出しつきの節（見出し = 固定の English・`PLAN_SCREENS` 等／行 = AI がその人の言葉で書いた物）。
        /// 案の card の `Details` の場所に開いて出す（`agent_rail::Card::plan`）。空 = 名前の表の案・AI が読めなかった・まだ開いていない（`preview` の行だけ）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub sections: Vec<(String, Vec<String>)>,
        /// L2（DOGFOOD-X 周 5 #5）: 案が在っても、作る前に答えが要る問い（例「Which Numbers file and table should they go in?」）。model が
        /// `talk_schema` の `needs` に入れた物だけ（文の形から推さない = 細部を聞く問いは Accept を止めない・周 4）。Some の間、案の card の primary
        /// （`Preview plan`・`Accept`）は灰で、理由は `NEEDS_ANSWER` の 1 行（`pending_needs`）。人の一言・案の無い答えは None。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub needs: Option<String>,
    }

    /// L2: 作る前の問い（Home の会話に出す）。
    pub const TALK_ASK: &str = "What do you want to make, and how should it work?";
    /// L2: 案の card の見出しと手。TALK-1101（owner 191 通目）: 先頭は最初から `Build now`（primary）・`Change plan`・`Later`・`Preview plan`（中身を card の中に開くだけ・
    /// 開いた後は同じ場所が `Hide plan`）。AX の名前も同じ語。
    pub const PROPOSAL: &str = "Proposal";
    pub const PREVIEW_PLAN: &str = "Preview plan";
    pub const HIDE_PLAN: &str = "Hide plan";
    pub const CHANGE_PLAN: &str = "Change plan";
    pub const LATER: &str = "Later";
    /// CHAT-011 反証 Y2: 案の `Later` の後に会話に残す 1 行の頭（`{LATER_SAID} · {案の文}`・何も作っていない事と、案の文が会話に残る事）。
    pub const LATER_SAID: &str = "Saved for later · nothing was built";
    /// CHAT-011 反証 Y6: 案の `Build now` の灰の理由（AI が読んでいる・AI を止めている間）。CLI を確かめている間はその 1 行そのもの。
    pub const WAIT_READING: &str = "Wait until the AI finishes reading";
    pub const WAIT_STOPPING: &str = "Wait until the AI stops";
    /// TALK-1101（owner 191 通目「全て任せてすぐに作れるボタン」）: 案の card の先頭の手（前の `Accept` を一本化）。答えていない問いは Shibubu が決めて作る。
    pub const BUILD_NOW: &str = "Build now";
    /// TALK-1101: Build now で答えていない問いに Shibubu が選んだ答えの 1 行の頭（後ろは AI がその人の言葉で書いた選び・作った Project の会話の頭に残す）。
    pub const PICKED: &str = "I picked";
    /// TALK-1101: Preview plan の節の見出し（固定の English・中の行は AI がその人の言葉で書く）。画面ごとの見える物と押せる物・使う流れ・Mac の中で触る物と聞く許可・まだ決まっていない事。
    pub const PLAN_SCREENS: &str = "On screen";
    pub const PLAN_STEPS: &str = "How you use it";
    pub const PLAN_MAC: &str = "On your Mac";
    pub const PLAN_OPEN: &str = "Not decided yet";
    /// L2: 会話を閉じて Home の初めの形（Try と Continue）に戻る。
    pub const NEW_CHAT: &str = "New chat";
    /// L2: model が答えるまでの Shibubu の 1 行（案も選択肢も出さない = 待つ間に案の card を押させない）。
    pub const TALK_WAIT: &str = "Thinking…";
    /// L2: 会話の model への指示。利用者の規則・記憶・skill は入れない。
    /// DOGFOOD-X 周 4 #3: 前の「proposal: null while you still need to ask」で、一言目が作る物を言っていても細部を聞く間は案を出さなかった（AI 1 回多い）
    /// → 作る物を言った turn で案を出し、問いは細部を聞いてよい。周 5 #5: 答えが無いと作れない問いだけ `needs` に入れる（`Turn::needs`）。
    /// TALK-1101（owner 190・194 通目「送った言語に合わせて返す」）: 前の「Write every field in English」（CHAT-011・RULE-027）をやめ、最後の一言の言葉で書かせる
    /// （機械翻訳は挟まない・名前はそのまま）。English 固定は Shibubu の部品の文字だけ。案が出た後の問いは 1 回に 1 つ（Build now はずっと押せる）。
    /// B61-REPO: `TALK_REPO` の行は決まった repo（「this repo」なら聞き返さない）。B63: 別の repo を名指したら代用せず `needs` で聞く（Engine P0「名指しの対象が決まらなければ代用しない」）。
    /// CHAT-FREE-1101（owner 198 通目の 6「出力される形が常に一緒なの困る…それはこちら側が強制させるものではなくaiに考えさせること」）: 前の
    /// 「ask: a short friendly reply, one or two sentences, ending with one question」「choices: at most 3」の形の強制を外した。`ask` は AI が形を決めた
    /// Markdown（`ui::markdown` が描く物を並べるだけ・使えと言わない）・問いも選択肢も要る時だけ。製品が持つのは案（Build now の card）と `needs` だけ。
    /// key の名前（`ask`）は変えない（会話の記録・試験の偽の答えがこの名前で読む）。
    /// CHAT-FREE-1101 段 2（owner 201「AI の画用紙」）: 図・グラフ・画面の下書き・小さな form は ```html の 1 塊（chat の `Interactive view` の card・閉じた WebView）。
    /// 形の JSON を強いず（```chart はやめた）、中の button が言えるのは `shibubu.reply(text)` と `shibubu.build()` の 2 つだけ（`surface::canvas_message`）。
    /// TALK-HONEST-1101（＋owner 206 追記）: 会話の頭の `TALK_ABLE` の行（`talk_abilities`）だけが app に届く手段・確かめていない物を確かめ済みと言わない。
    /// 直接の口が無い app は断らず組み方（Page の部品 ＋ 押した時に Shibubu が app の画面を操作〈初めては見せる〉＋ 読み戻し・AI-COMPOSE-1101）を案に・
    /// 読めも押せもしない時だけ案を出さず理由と代わりを 1 行。repo の行は能力の行の後ろになったので「A first line」→「A line」。
    pub const TALK_INSTRUCTIONS: &str = "You are Shibubu, a Mac app that builds a project for the user. Before building anything, talk with them like a normal chat. Reply with the schema only. ask: your reply in Markdown, shaped by what you are saying: a single line for small talk, or headings, lists, numbered steps, tables, code blocks, quotes, links, a task list (- [ ] item) they can tick, and <details><summary>…</summary> to fold long logs, whenever that is clearer. To show a chart, a diagram, a mock-up of a screen or a small form, put one self-contained ```html block in ask (inline HTML, CSS, SVG and JavaScript only: it can't load anything from the internet or from files); a button in it can call shibubu.reply(text) to send that text as their reply, or shibubu.build() to press Build now. Ask a question only when you need an answer. choices: replies they could tap and send next, only when tapping is easier than typing; otherwise []. proposal: always null for a greeting, small talk, a question, or a single word. As soon as they have said what to make, even in their first message, proposal is one sentence of what to build that says only what they asked for, adding nothing, and your question can still ask about details. needs: null, unless it can't be built at all without their answer (for example which file or which account); then it is that question. Lines under \"What Shibubu can do on this Mac\" list how Shibubu can reach apps on this Mac right now, whether each was checked here, and the parts a Page can hold: never say it can already do something with their apps that isn't listed, or that something not checked here is checked. When what they want happens inside an app with no direct way listed, don't refuse: propose building it from Shibubu's parts, for example a picker on a Page filled by reading that app, and when they press it, Shibubu operates that app's screen for them (showing them the first time) and reads the app back to check it worked. Only when that app can be neither read nor pressed, proposal is null and ask says why in one line and what they can do instead. A line \"Their repo\" names the repo they already chose (the folder you run in is not it): \"this repo\" means that one, so never ask which. If they name a different repo, don't use that one instead: needs asks where the repo they named is. A line \"Their project\" means they are inside a project they already made: then proposal is one sentence of the change to make to that project (for example \"Add a Tests page\"), never a new project, and stays null while they only ask or chat. Lines under \"Other sessions in this project\" are what was said in that project's other chats: treat what was decided there as known and don't ask it again. Lines under \"Now on this page and in Shibubu\" are what their page and Shibubu show right now: the page, its parts and what was chosen in them, an error a part shows (a line with \"shows an error\"), what Shibubu is doing or last did and why it stopped, the latest of this chat, and open windows. When they ask about a mistake, an error, something not working, or what something is, look there first: name the part and the error it shows (or why the last run stopped), say what happened and the one thing that would fix it, and never answer about your own earlier wording instead. Write every field in the language of their latest message (if it is too short to tell, the language they have been writing in), and keep names they gave (apps, files, people, titles) exactly as written. Once there is a proposal, ask at most one question per reply: they can press Build now at any time, and Shibubu then picks the answers they have not given.";
    /// L2（DOGFOOD-X 周 5 #5）: 作る前に答えが要る問いが在る間の、案の card の 1 行の頭（後ろに問い）と、灰の `Build now` の hover の文（TALK-1101: `Answer this first: <問い>`）。
    pub const NEEDS_ANSWER: &str = "Answer this first";

    /// L2: model に渡す会話（`You: …` と `Shibubu: …` の行）。待つ間の 1 行は入れない。
    /// B61-REPO（録画の手 1）: `repo` = 利用者が決めた repo の folder の名前（app.rs の `workspace` = SHIBUBU_WORKSPACE_DIR）。Some なら 1 行目に
    /// `TALK_REPO: <名前>` = model が「this repo」を聞き返さない（claude の cwd は data dir で git の repo でない）。path は送らない（利用者の名前を含む・AI は tool 0 で読めない）。
    /// 例: `talk_history(&shell.talk, Some("shibubu-yc-demo"))` → `"Their repo (what \"this repo\" means): shibubu-yc-demo\nYou: …"`
    pub fn talk_history(turns: &[Turn], repo: Option<&str>) -> String {
        let said = turns.iter().filter(|t| !(t.who == Who::Shibubu && t.text == TALK_WAIT)).map(|t| format!("{}: {}", if t.who == Who::You { "You" } else { "Shibubu" }, t.text));
        repo.map(|r| format!("{TALK_REPO}: {r}")).into_iter().chain(said).collect::<Vec<_>>().join("\n")
    }
    /// B61-REPO: 会話の 1 行目の repo の頭（`talk_history`・`TALK_INSTRUCTIONS` がこの語で指す）。
    pub const TALK_REPO: &str = "Their repo (what \"this repo\" means)";
    /// CHAT-ONE-1101: Project の Chat の会話の 1 行目の頭（後ろに Project の名前と Page の並び・app.rs の `talk` が `talk_history` の前に足す）。
    /// `TALK_INSTRUCTIONS` がこの語で指す = 案はその Project への変更の 1 文（新しい Project にしない）。
    pub const TALK_PROJECT: &str = "Their project";
    /// SESSIONS-LEFT-1101: Home の会話を `chat_turns` に保存する時の key の頭（後ろに `storage::new_id`）。Project の会話の key は Project の id（重ならない）。
    pub const HOME_CHAT: &str = "home:";
    /// SESSIONS-LEFT-1101（owner 198 の 4）: Project の 2 つ目からの chat session の key の区切り（`{project}{SESSION}{id}`・Project の id に入らない字）。
    /// 1 つ目の session の key は Project の id のまま（前からの会話がそのまま 1 つ目）。
    pub const SESSION: &str = "#";
    /// SESSIONS-LEFT-1101（owner 198 の 4「そのprojectにある他のsessionも読めるし把握している」）: AI に渡す、同じ Project の他の session の要点の見出し。
    pub const TALK_OTHERS: &str = "Other sessions in this project";

    /// SESSIONS-LEFT-1101: 会話の key の Project（1 つ目の session = key そのもの・2 つ目から = `SESSION` の前）。Home の会話（`HOME_CHAT`）は None。
    /// 例: `session_project("project.app.a#k1") == Some("project.app.a")`・`session_project("home:k2") == None`。
    pub fn session_project(key: &str) -> Option<&str> {
        if key.starts_with(HOME_CHAT) {
            return None;
        }
        Some(key.split_once(SESSION).map_or(key, |(p, _)| p))
    }

    /// TALK-HONEST-1101: 会話の AI に渡す「この Mac で Shibubu が今出来る事」の頭（`TALK_INSTRUCTIONS` がこの語で指す）。
    pub const TALK_ABLE: &str = "What Shibubu can do on this Mac";
    /// TALK-HONEST-1101: 画面の手（`desktop.*`）を 1 行にまとめた時の名前（app ごとの手は Engine が初めての時に作る・AI-COMPOSE-1101）。
    const TALK_SCREEN: &str = "Any app's screen: read its windows, click, type, choose menu items, press keys";
    /// 直接の手段の行の上限（多い Mac で会話の頭を太らせない・残りは数だけ）
    const TALK_ABLE_MAX: usize = 20;

    /// TALK-HONEST-1101（owner 206 追記）: 会話の AI に渡す「この Mac で今出来る事」の行（`PageEngine::abilities` が作り、`talk()` が会話の前に置く）。
    /// `records` = `capability_engine::abilities`（registry の Adapter ＋ 保存した能力・Engine が見る物と同じ）・`mcp` = Engine が起こしてよい MCP の server の名前。
    /// 保証する: 確かめたかを能力ごとに言う（確かめていない物を確かめ済みと言わない）・`desktop.*` は 1 行（どれかが確かめ済みなら確かめ済み・全部使えなければ使えない）・
    /// 直接の手段は `TALK_ABLE_MAX` 行まで・Page に置ける部品は `ui::BUILTIN_COMPONENTS`（並べるだけの入れ物と移行の印を除く）・path も利用者の名前も持たない。
    /// 例: `talk_abilities(&records, &["playwright"])` → `"What Shibubu can do on this Mac:\n- git.status (local_core · not checked on this Mac yet)\n- Any app's screen: … (not checked on this Mac yet)\n- MCP server “playwright” (…)\nParts a Page can hold: …"`
    pub fn talk_abilities(records: &[super::capability::CapabilityRecord], mcp: &[&str]) -> String {
        use super::capability::Verification;
        let state = |v: &Verification| match v {
            Verification::Verified { .. } => "checked on this Mac".to_owned(),
            Verification::Unverified => "not checked on this Mac yet".to_owned(),
            Verification::Unsupported { reason } => format!("can't be used now: {reason}"),
        };
        let (screen, direct): (Vec<_>, Vec<_>) = records.iter().partition(|r| r.id.starts_with("desktop."));
        let mut lines = vec![format!("{TALK_ABLE}:")];
        lines.extend(direct.iter().take(TALK_ABLE_MAX).map(|r| format!("- {} ({} · {})", r.id, r.kind.as_str(), state(&r.verification))));
        if direct.len() > TALK_ABLE_MAX {
            lines.push(format!("- and {} more", direct.len() - TALK_ABLE_MAX));
        }
        let best = screen
            .iter()
            .find(|r| matches!(r.verification, Verification::Verified { .. }))
            .or_else(|| screen.iter().find(|r| r.verification == Verification::Unverified))
            .or(screen.first());
        if let Some(r) = best {
            lines.push(format!("- {TALK_SCREEN} ({})", state(&r.verification)));
        }
        lines.extend(mcp.iter().map(|n| format!("- MCP server “{n}” (its tools are read when a request names it)")));
        let parts: Vec<&str> = super::ui::BUILTIN_COMPONENTS
            .iter()
            // AI-COMPOSE-1101: `canvas` は描く側と Build now が出来るまで会話に出さない（本体で組み方の 1 行に替える）
            .filter(|c| !["column", "row", "split", "tabs", "extension", "canvas"].contains(c))
            .map(|c| if *c == "source_surface" { "an app's own window or a website placed on the Page" } else { c })
            .collect();
        lines.push(format!("Parts a Page can hold: {}", parts.join(", ")));
        lines.join("\n")
    }

    /// L2: model に渡す schema（新しい型は作らない。`Value` のまま）。
    pub fn talk_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "ask": {"type": "string"},
                "choices": {"type": "array", "items": {"type": "string"}},
                "proposal": {"type": ["string", "null"]},
                "needs": {"type": ["string", "null"]}
            },
            "required": ["ask", "choices", "proposal", "needs"]
        })
    }

    /// L2: model が無い・答えられなかった時の返事。短い一言・挨拶・問いは問いと 3 つの選択肢だけで、案は出さない。
    /// 「I'll describe it」は問いだけ。「Track my inventory」「Keep research notes」（Home の例の文 = model 無しで作れる）はその文を案にする。
    /// それより長い文は同じ問いと選択肢を残し、その文を案にする（作るのは案の card の `Build now` だけ）。
    pub fn local_talk(text: &str) -> (String, Vec<String>, Option<String>) {
        let t = text.trim();
        let open = vec!["Track my inventory".to_owned(), "Keep research notes".to_owned(), "I'll describe it".to_owned()];
        if t == "I'll describe it" {
            return (TALK_ASK.to_owned(), Vec::new(), None);
        }
        if open.iter().any(|c| c == t) {
            return (TALK_ASK.to_owned(), Vec::new(), Some(t.to_owned()));
        }
        let lower = t.to_ascii_lowercase();
        let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
        let greeting = words.len() <= 3 && words.first().is_some_and(|w| matches!(*w, "hi" | "hello" | "hey" | "yo" | "hiya" | "howdy" | "thanks" | "thank" | "good" | "morning" | "evening" | "こんにちは" | "こんばんは" | "おはよう" | "ありがとう" | "やあ" | "はじめまして"));
        // 空白で区切らない言葉（日本語）の 1 文は 1 語に数えるので、そちらは 4 文字までを短い一言にする
        let one_word = if t.is_ascii() { words.len() <= 1 } else { t.chars().count() <= 4 };
        let short = one_word || greeting || t.ends_with('?') || t.ends_with('？');
        if short {
            return (TALK_ASK.to_owned(), open, None);
        }
        (TALK_ASK.to_owned(), open, Some(t.to_owned()))
    }

    /// DOGFOOD-X 周 7 #1: AI が上限で止まっている時の Home の返事（`why` = `Generated::llm_stopped` の 1 行 = 止まった事と次の一手）。
    /// `local_talk` の問いと選択肢（Track my inventory 等・一言と関係が無い）は出さない。作る物を言った文はその文を案にし、Build now は名前の表で作る
    /// （`SIMPLE_ACCEPT`・app.rs の `create_app` が AI を呼ばずに `create_simple` へ）。短い一言・挨拶は止まった事だけ（案なし）。
    /// 名前の表でも作れない文（変更の文）の案と約束は呼ぶ側（app.rs の `no_ai_reply`）が落とす（REVIEW-R7 Y-a・composer を domain から呼ばない）。
    /// 例: `let (ask, choices, proposal) = match g.llm_stopped() { Some(why) => stopped_talk(&why, &text), None => local_talk(&text) };`
    /// Wave: L2
    pub fn stopped_talk(why: &str, text: &str) -> (String, Vec<String>, Option<String>) {
        let (_, _, proposal) = local_talk(text);
        let ask = if proposal.is_some() { format!("{why} · until then, {SIMPLE_ACCEPT}") } else { why.to_owned() };
        (ask, Vec::new(), proposal)
    }
    /// CHAT-ONE-1101: Project の Chat で model が無い・答えなかった・上限で止まった時の返事。挨拶・短い一言・問いは何を変えるかの問いだけ（Home の例の選択肢は出さない）・
    /// それ以外はその文を変更の案に（`Build now` = 今の変更の道 = 名前の表が読む）。`why` = 止まった事の 1 行（`Generated::llm_stopped`・在れば頭に）。
    /// 例: `let (ask, choices, proposal) = local_change(&text, g.llm_stopped().as_deref());`
    pub fn local_change(text: &str, why: Option<&str>) -> (String, Vec<String>, Option<String>) {
        let proposal = local_talk(text).2;
        let ask = if proposal.is_some() { CHANGE_READY } else { CHANGE_ASK };
        (why.map_or_else(|| ask.to_owned(), |w| format!("{w} · {ask}")), Vec::new(), proposal)
    }
    /// CHAT-ONE-1101: `local_change` の 2 つの返事（案が無い = 何を変えるか・案が在る = card の手の順）。
    pub const CHANGE_ASK: &str = "What should change in this project? For example, add a Tests page.";
    pub const CHANGE_READY: &str = "Here is the change. Build now applies it.";
    /// 周 7 #1 #2: AI が止まっている間の Build now（会話の返事と `Preview plan` の 1 行目の後ろ）。
    pub const SIMPLE_ACCEPT: &str = "Build now uses simple rules";

    /// L2: 最後の Shibubu の案。空なら案の card を出さない。
    pub fn pending_proposal(turns: &[Turn]) -> Option<&str> {
        turns.iter().rev().find(|t| t.who == Who::Shibubu).and_then(|t| t.proposal.as_deref()).filter(|p| !p.trim().is_empty())
    }

    /// L2（DOGFOOD-X 周 5 #5）: 最後の Shibubu の答えに案が在り、model が作る前に答えが要ると言った問い（`Turn::needs`）。在る間、案の card の
    /// `Build now` は灰で `plan_step` も作らない（TALK-1101: `Preview plan` は読める = 作らない）。人が答えると次の答えの `needs` で決め直す。
    /// 例: `if let Some(q) = pending_needs(&shell.talk) { format!("{NEEDS_ANSWER}: {q}") }`。保証しない: 問いに本当に答えたかの判定（model に任せる）。
    pub fn pending_needs(turns: &[Turn]) -> Option<&str> {
        pending_proposal(turns)?;
        turns.iter().rev().find(|t| t.who == Who::Shibubu).and_then(|t| t.needs.as_deref()).filter(|q| !q.trim().is_empty())
    }

    /// L2（DOGFOOD-X 周 2 回 1 #1）: 案の card に残す、まだ答えていない問いの頭。
    pub const OPEN_QUESTION: &str = "Not answered yet";
    /// L2: `Preview plan` の読むだけの行の終わり（何も書いていない）。
    pub const PLAN_UNMADE: &str = "Nothing is made until you press Build now.";
    /// L2: AI が案を読んでいる間の `Preview plan` の 1 行（Home はその間 `Build now` は灰・Project の chat は押せて、押すと `BUILD_QUEUED`）。
    pub const PLAN_READING: &str = "The AI is reading this plan…";
    /// OPEN-INPLACE-1101 の 5: Project の chat の案の Preview の読みの最中に `Build now` を押した時の 1 行（読み終えたらそのまま作る・`Generated::build_when_read`）。
    pub const BUILD_QUEUED: &str = "Building as soon as the plan is read";

    /// L2（DOGFOOD-X 周 2 回 1 #1）: 最後の Shibubu の答えが選択肢つきの問いで終わっていれば、その問いの 1 文（`?`・`？` で終わる最後の文）。
    /// 選択肢を押さずに `Build now` できる問いを案の card に残す（`home::talk_thread`）。選択肢が無い・問いで終わらない = None。
    /// ponytail: 文の区切りは `.` `!` `?` と全角の 3 つだけ（`e.g.` の中で切れる）。
    pub fn open_question(turns: &[Turn]) -> Option<&str> {
        let last = turns.iter().rev().find(|t| t.who == Who::Shibubu).filter(|t| !t.choices.is_empty())?;
        let t = last.text.trim_end();
        let body = t.strip_suffix('?').or_else(|| t.strip_suffix('？'))?;
        let start = body.char_indices().rev().find(|(_, c)| matches!(c, '.' | '!' | '?' | '。' | '！' | '？')).map_or(0, |(i, c)| i + c.len_utf8());
        Some(t[start..].trim_start())
    }

    #[cfg(test)]
    mod talk_shape {
        use super::*;

        /// TALK-HONEST-1101: 会話の頭は能力ごとに確かめたかを言い、画面の手は 1 行・MCP は名前・Page の部品を持つ。確かめていない物を確かめ済みと言わない。
        /// 負の対照: `state` の Unverified の腕を "checked on this Mac" にする → 1 つ目と画面の行の assert が赤。
        #[test]
        fn talk_honest_lists_what_this_mac_can_do_and_whether_it_was_checked() {
            use crate::domain::capability::{AdapterKind, CapabilityRecord, Origin, Reuse, Verification, CAPABILITY_SCHEMA};
            let rec = |id: &str, kind, verification| CapabilityRecord {
                schema: CAPABILITY_SCHEMA.into(),
                id: id.into(),
                reuse: Reuse::Shared,
                origin: Origin::Official,
                kind,
                verification,
                versions: vec![],
                permissions: vec![],
                evidence: vec![],
                constraints: vec![],
            };
            let checked = Verification::Verified { at: "2026-11-01T00:00:00Z".into(), evidence: "read back".into() };
            let records = [
                rec("git.status", AdapterKind::LocalCore, Verification::Unverified),
                rec("garageband.track.add", AdapterKind::Extension, checked.clone()),
                rec("desktop.ax.click", AdapterKind::MacGui, Verification::Unverified),
                rec("desktop.key", AdapterKind::MacGui, Verification::Unverified),
            ];
            assert!(TALK_INSTRUCTIONS.contains(&format!("\"{TALK_ABLE}\"")), "指示が能力の行の頭を名指していない");
            let got = talk_abilities(&records, &["playwright"]);
            let lines: Vec<&str> = got.lines().collect();
            assert_eq!(lines[0], format!("{TALK_ABLE}:"));
            assert_eq!(lines[1], "- git.status (local_core · not checked on this Mac yet)");
            assert_eq!(lines[2], "- garageband.track.add (extension · checked on this Mac)");
            assert_eq!(lines[3], format!("- {TALK_SCREEN} (not checked on this Mac yet)"), "画面の手が 1 行にまとまっていない・確かめ済みと言った");
            assert_eq!(lines[4], "- MCP server “playwright” (its tools are read when a request names it)");
            assert!(lines[5].starts_with("Parts a Page can hold: text, list, table") && lines[5].contains("an app's own window or a website placed on the Page"), "{}", lines[5]);
            assert!(!lines[5].contains("column") && !lines[5].contains("source_surface"), "{}", lines[5]);
            assert_eq!(lines.len(), 6, "{got}");
            // 画面の手のどれかを確かめた Mac は確かめ済み・どれも使えなければ使えない理由
            let one = [rec("desktop.ax.click", AdapterKind::MacGui, Verification::Unverified), rec("desktop.key", AdapterKind::MacGui, checked)];
            assert!(talk_abilities(&one, &[]).contains(&format!("- {TALK_SCREEN} (checked on this Mac)")));
            let off = [rec("desktop.ax.click", AdapterKind::MacGui, Verification::Unsupported { reason: "Accessibility is off".into() })];
            assert!(talk_abilities(&off, &[]).contains(&format!("- {TALK_SCREEN} (can't be used now: Accessibility is off)")));
            // 多い Mac は TALK_ABLE_MAX 行と残りの数
            let many: Vec<CapabilityRecord> = (0..TALK_ABLE_MAX + 3).map(|i| rec(&format!("x.a{i}"), AdapterKind::Extension, Verification::Unverified)).collect();
            let got = talk_abilities(&many, &[]);
            assert_eq!(got.lines().filter(|l| l.starts_with("- x.a")).count(), TALK_ABLE_MAX);
            assert!(got.contains("- and 3 more"), "{got}");
        }

        /// ENGINE-GENERIC-1101 追記 234 × TALK-HONEST 追記 232: chat と Engine が同じ書き方で読む状況の塊（頭・事実・file は名前だけ）と、chat の指示が
        /// その頭を名指して「エラーは何？」に Page の誤りから答える事。空の状況は塊を作らない（Engine の input は一言だけ）。
        /// 負の対照: `text` の file を path にする → 名前の assert が赤・`TALK_INSTRUCTIONS` の `NOW_HEAD` の語を替える → 名指しの assert が赤。
        #[test]
        fn now234_one_situation_block_for_chat_and_engine() {
            let s = Situation { facts: vec!["Page: Hum".into(), "“Guitar” shows an error: Couldn't read".into()], files: vec![PathBuf::from("/tmp/x/take-3.wav")] };
            assert_eq!(s.text().as_deref(), Some("Now on this page and in Shibubu:\n- Page: Hum\n- “Guitar” shows an error: Couldn't read\n- File “take-3.wav”"));
            assert_eq!(Situation::default().text(), None);
            assert!(TALK_INSTRUCTIONS.contains(&format!("\"{NOW_HEAD}\"")), "chat の指示が状況の塊の頭を名指していない");
            assert!(TALK_INSTRUCTIONS.contains("shows an error"), "chat の指示が部品の誤りの行を名指していない");
        }

        /// L2 の呼ぶ側: hello は問いと選択肢だけで案が無い。選択肢の文は案になる。古い json は新しい欄が空のまま読める。
        /// 負の対照: `local_talk` の短い一言の腕を外す → hello に案が付いて赤。
        #[test]
        fn hello_asks_without_a_proposal_and_old_turns_still_load() {
            let (ask, choices, proposal) = local_talk("hello");
            assert_eq!(ask, TALK_ASK);
            assert_eq!(choices, vec!["Track my inventory".to_owned(), "Keep research notes".to_owned(), "I'll describe it".to_owned()]);
            assert!(proposal.is_none(), "hello に案が付いた");
            assert_eq!(local_talk("I'll describe it").2, None);
            assert_eq!(local_talk("Keep research notes").2.as_deref(), Some("Keep research notes"));
            assert_eq!(local_talk("Track my inventory").2.as_deref(), Some("Track my inventory"));
            for said in ["Hello!", "hey there", "Good morning", "thanks a lot", "What can you do?", "Notes", "こんにちは", "家"] {
                assert_eq!(local_talk(said).2, None, "「{said}」に案が付いた");
            }
            assert_eq!(local_talk("在庫を管理したい").2.as_deref(), Some("在庫を管理したい"), "空白の無い日本語の依頼を 1 語と読んだ");
            let old = r#"{"who":"you","text":"hi","at":"t"}"#;
            let t: Turn = serde_json::from_str(old).unwrap();
            assert!(t.choices.is_empty() && t.proposal.is_none() && t.attachments.is_empty());
            let turn = |who, text: &str| Turn { who, text: text.into(), mark: Mark::None, at: "t".into(), attachments: vec![], choices: vec![], proposal: None, preview: vec![], sections: vec![], needs: None };
            let turns = [turn(Who::You, "hello"), turn(Who::Shibubu, TALK_ASK), turn(Who::You, "Track my inventory"), turn(Who::Shibubu, TALK_WAIT)];
            assert!(pending_proposal(&turns).is_none(), "待つ間に案が出た");
            assert_eq!(talk_history(&turns, None), format!("You: hello\nShibubu: {TALK_ASK}\nYou: Track my inventory"), "待つ 1 行が model に渡った");
            assert_eq!(talk_history(&turns[..1], Some("shibubu-yc-demo")), format!("{TALK_REPO}: shibubu-yc-demo\nYou: hello"), "repo の行が 1 行目に無い");
            // DOGFOOD-X 周 2 回 1 #1: 選択肢つきの問いで終わる答えは、その問いの 1 文が残る（選択肢が無い・問いで終わらない答えは残さない）
            let asked = |text: &str, choices: &[&str]| [Turn { choices: choices.iter().map(|c| c.to_string()).collect(), ..turn(Who::Shibubu, text) }];
            let q = "Should the summary be made for you, or do you write it?";
            assert_eq!(open_question(&asked(&format!("Nice, notes in TextEdit sound great. {q}"), &["Make it", "I'll write it"])), Some(q));
            assert_eq!(open_question(&asked("要約は自動にしますか？", &["自動"])), Some("要約は自動にしますか？"));
            assert_eq!(open_question(&asked(q, &[])), None, "選択肢の無い問いを残した");
            assert_eq!(open_question(&asked("Here is the plan.", &["Ok"])), None, "問いでない答えを残した");
            // DOGFOOD-X 周 5 #5: 答えが要る問いは案と一緒の時だけ止める・needs の無い答え（細部の問い = 周 4）は止めない
            let held = |proposal: Option<&str>, needs: Option<&str>| [Turn { proposal: proposal.map(str::to_owned), needs: needs.map(str::to_owned), ..turn(Who::Shibubu, "Which Numbers file?") }];
            assert_eq!(pending_needs(&held(Some("Copy the numbers"), Some("Which Numbers file?"))), Some("Which Numbers file?"));
            assert_eq!(pending_needs(&held(Some("Copy the numbers"), None)), None, "needs の無い答えで止めた");
            assert_eq!(pending_needs(&held(None, Some("Which Numbers file?"))), None, "案の無い答えで止めた");
            assert_eq!(pending_needs(&held(Some("Copy the numbers"), Some("  "))), None, "空の問いで止めた");
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Who {
        You,
        Shibubu,
    }

    /// 答えの印（`RegionState` の写し・domain は ui を引かない）。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Mark {
        #[default]
        None,
        /// 確かめた（`✓ Verified`）
        Verified,
        /// 送ったが確かめられない
        Partial,
        /// 止めた・送らなかった（拒否・承認しなかった）
        Stopped,
        Failed,
    }

    /// 添えた file の種類（長方形の形と、provider が受けるかの門）。HOME-ATTACH-1101: folder と zip も `File`。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum AttachKind {
        Photo,
        File,
    }

    /// 一言に添える 1 つ。
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Attachment {
        /// `attach` を通った file（絶対 path・選んだ時の大きさ）。HOME-ATTACH-1101: folder は `path` が folder で `bytes` が中身の和・
        /// zip は展開した folder（`$TMPDIR/shibubu-unzipped/…/<名前>.zip/`・名前は zip の名前のまま）で `bytes` が展開した中身の和
        File { path: PathBuf, kind: AttachKind, bytes: u64 },
        /// HOME-ATTACH-1101: Home の欄に貼った `PASTE_LINES` 行以上の文（`paste` を通った物・`lines` = 行の数〈毎 frame 数え直さない〉）。
        /// 欄には入れず、添付の長方形 1 つ（押すと読むだけの中身）。AI へは file と同じ道（`Generated` の `Asked::of` が `Pasted text.txt` に書いて `files` に）。
        /// 新しい型でなく variant の理由: 並び・× で外す・送る前の Cancel で欄に戻す・送る道が今の添付と同じ物だから。
        Pasted { text: String, lines: usize },
    }

    impl Attachment {
        /// 長方形に出す語（file・folder・zip の名前・`Pasted text · 412 lines`）。
        pub fn name(&self) -> String {
            match self {
                Attachment::File { path, .. } => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
                Attachment::Pasted { lines, .. } => format!("Pasted text · {lines} lines"),
            }
        }
    }

    /// ENGINE-GENERIC-1101（owner 211「userがその時に打ったプロンプトからshibubuが汎用的に状況を理解して対応できるengine」）: 一言と一緒に Engine の
    /// 1 つの口（`Generated::ask_engine`・`capability_engine::EngineContext::situation`）へ渡す今の状況。部品（録音の帯・Page の部品・chat）が知っている
    /// 事実だけで、app の名前も決まった機能（「take を入れる」等）も code に持たない（事実の行に出る名前は Project の data と利用者の言葉から来る）。
    /// 新しい型の理由: `EngineRequest` / `EngineContext` の target・document・document_file・repo は「Page の app 1 つ」の決まった欄で、録った file・部品で
    /// 選んだ値・直前に起きた事を、部品の種類を知らずに Engine へ渡す欄が無い。domain に置く理由: generated.rs は capability_engine の型を名指せない
    /// （tests/yc_demo_e2e.rs が generated.rs を #[path] で引く）。
    /// 例: `Situation { facts: vec!["Page: Hum to Guitar".into(), "Chosen in “Guitar”: Acoustic Guitar".into(), "Just happened: recorded a 12 s take".into()],
    ///       files: vec![takes_dir.join("take-3.wav")] }`
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Situation {
        /// 人の言葉の事実の 1 行ずつ（Engine の interpret と generate の AI が一言と一緒に読む）
        pub facts: Vec<String>,
        /// 一言が指してよい file（絶対 path・録った take・Page の面の書類）。手の input には Engine が `{"$from": "situation.files.<n>"}`（絶対 path）・
        /// `situation.urls.<n>`（`file://`・外へ出す所でだけ作る）で埋める。AI に渡すのは名前と `$from` の名前だけ（FX-111 = home の path も中身も渡さない）・
        /// B76 の読みの門（Project の repo の外を読まない）は変えない
        pub files: Vec<PathBuf>,
    }

    impl Situation {
        /// 事実も file も無い（Engine の AI の input は一言だけ = 今までと同じ字）
        pub fn is_empty(&self) -> bool {
            self.facts.is_empty() && self.files.is_empty()
        }

        /// 部品が渡した状況を Page の状況の後ろに足す（同じ事実・同じ file は重ねない）。`Generated::ask_engine` が使う。
        /// 例: `g.situation(&project).with(Situation { files: vec![take], ..Default::default() })`
        pub fn with(mut self, more: Situation) -> Situation {
            for f in more.facts {
                if !self.facts.contains(&f) {
                    self.facts.push(f);
                }
            }
            for f in more.files {
                if !self.files.contains(&f) {
                    self.files.push(f);
                }
            }
            self
        }

        /// ENGINE-GENERIC-1101 追記 234（owner「engineが把握することとchatが把握することは全て共有して互換性をもたせて」）: chat の AI と Engine の AI に
        /// 渡す状況の塊のただ 1 つの書き方（app.rs の `talk_in` と `capability_engine::interpret` がこれを呼ぶ = 同じ状況なら同じ字）。頭は `NOW_HEAD`・
        /// 事実は 1 行ずつ `- `・file は名前だけ（FX-111 = path を AI に渡さない）。空 = None（Engine の input は一言だけ = 今までと同じ字）。
        /// 例: `Situation { facts: vec!["Page: Hum".into()], files: vec![take] }.text()` → `Some("Now on this page and in Shibubu:\n- Page: Hum\n- File “take-3.wav”")`
        pub fn text(&self) -> Option<String> {
            if self.is_empty() {
                return None;
            }
            let name = |f: &PathBuf| f.file_name().map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
            let lines = self.facts.iter().map(|f| format!("- {f}")).chain(self.files.iter().map(|f| format!("- File “{}”", name(f))));
            Some(std::iter::once(format!("{NOW_HEAD}:")).chain(lines).collect::<Vec<_>>().join("\n"))
        }
    }

    /// ENGINE-GENERIC-1101 追記 234 × TALK-HONEST 追記 232: 状況の塊の頭（`Situation::text`・`TALK_INSTRUCTIONS` がこの語で指す）。
    pub const NOW_HEAD: &str = "Now on this page and in Shibubu";

    /// ENGINE-AUDIT W9（節 14「一言の種類・名指し・曖昧さを表せない」・P0 の代用の穴）: 一言を分けた操作 1 つの種類（`capability_engine::Operation::kind`・
    /// interpret の AI が決める）。act = 元の app・service・file への操作（Engine）・change = Shibubu の Page や部品を変える頼み（composer）・
    /// question = 質問・相談（会話の AI が run の記録で答える・何も操作しない）・nothing = 挨拶など（何もしない）。
    /// 新しい型の理由: 既存の enum（`Stage`・`StepKind`・`Effect`・`Who`）はどれも「一言の種類」を表さず、generated.rs と composer は capability_engine の型を名指せないので domain に置く。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum OperationKind {
        #[default]
        Act,
        Change,
        Question,
        Nothing,
    }

    /// ENGINE-AUDIT W9 本体: `PageEngine::classify` が返す操作 1 つ（interpret の `Operation` の写し = 種類・want・宛先・名指し・候補）。`route` が chat の振り分けに使う。
    /// 新しい型の理由: generated.rs と app.rs は capability_engine の `Operation` を名指せない（試験の crate が engine を持たない）・`OperationKind` だけでは宛先と候補を運べない
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Classified {
        pub kind: OperationKind,
        pub want: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target: Option<String>,
        #[serde(default)]
        pub named: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub candidates: Vec<String>,
    }

    /// W9 本体（spec「直した後の流れ B: interpret が種類と宛先を決める」・節 14・dogfood r1 / r6 = app の操作が composer に行って「部品を足した」で済んだ）: 一言の振り分け。
    /// act は Engine（`engine` = 一言のまま Engine に渡す・Engine が act だけを行う）・change と question は会話の AI（`talk`・案は composer へ）・
    /// act の宛先が決まらない（候補が 2 つ以上・名指したのに無い）= `ask` で 1 回だけ聞く（候補は選択肢・Engine は走らせない・答えの次の一言でまた分ける）。
    /// 混ざった頼み（act ＋ question）は両方（`hint` = 会話の AI に「app の方は Shibubu がしている」= 同じ事を案にしない）。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Route {
        /// 一言を Engine に渡す（act が 1 つ以上・聞く事が無い）
        pub engine: bool,
        /// 会話の AI を呼ぶ（question か change が在る・act が 1 つも無い = 分けられなかった時も）
        pub talk: bool,
        /// 1 回だけ聞く問いと選択肢（候補）。Some の時は engine も talk も false
        pub ask: Option<(String, Vec<String>)>,
        /// Engine がする act の want（会話の AI への 1 行の材料）
        pub acting: Vec<String>,
        /// B86: 候補が 2 つ以上だったが宛先が決まった（Page の app）時、聞かずに進む事の 1 行（`Using TextEdit for “…”`）。Engine に渡す時だけ
        pub using: Option<String>,
    }
    impl Route {
        /// 会話の AI の history に足す 1 行（Engine がしている事 = 同じ事を案にしない）。Engine に渡さない時は None
        pub fn hint(&self) -> Option<String> {
            (self.engine && !self.acting.is_empty()).then(|| format!("Shibubu is already doing this in the apps (don't propose it again): {}", self.acting.join(" · ")))
        }
    }
    /// 例: `let r = route(&classified); if let Some((q, choices)) = &r.ask { /* 問いの行 */ } else if r.engine { /* 一言を Engine へ */ } if r.talk { /* 会話の AI */ }`
    pub fn route(ops: &[Classified]) -> Route {
        let acts: Vec<&Classified> = ops.iter().filter(|o| o.kind == OperationKind::Act).collect();
        let talk = acts.is_empty() || ops.iter().any(|o| matches!(o.kind, OperationKind::Question | OperationKind::Change));
        let ask = acts.iter().find(|o| o.target.is_none() && (o.candidates.len() >= 2 || o.named)).map(|o| {
            if o.candidates.len() >= 2 {
                (format!("{WHICH_APP} “{}”?", o.want), o.candidates.clone())
            } else {
                (format!("Which app do you mean for “{}”? Name it as it appears in the Dock", o.want), Vec::new())
            }
        });
        let engine = !acts.is_empty() && ask.is_none();
        let using = acts.iter().find(|o| o.candidates.len() >= 2).and_then(|o| Some(format!("Using {} for “{}”", o.target.as_ref()?, o.want))).filter(|_| engine);
        Route { engine, talk: talk && ask.is_none(), acting: acts.iter().map(|o| o.want.clone()).collect(), ask, using }
    }

    /// W9 の候補が 2 つ以上の問いの頭（`route` の問い・`answered_ask` が見分ける）
    pub const WHICH_APP: &str = "Which app should Shibubu use for";

    /// B86: bundle id か origin の、人に見せる名前（chat の問いの選択肢と「Using … for」の 1 行・bundle id を画面に出さない）。
    /// 状況の窓の行（`capability_engine::open_windows` = `Windows open now: {名前} ({bundle id}) × n`）に在ればその名前・origin は host・無ければ最後の `.` の後ろ（頭を大文字）。
    /// 例: `app_label("com.stablyai.orca", &["Windows open now: TextEdit (com.apple.TextEdit) × 1 · Orca (com.stablyai.orca) × 2".into()]) == "Orca"`
    pub fn app_label(id: &str, facts: &[String]) -> String {
        let tag = format!(" ({id})");
        if let Some(name) = facts.iter().find_map(|f| f.find(&tag).map(|i| f[..i].rsplit([':', '·']).next().unwrap_or("").trim().to_owned())).filter(|n| !n.is_empty()) {
            return name;
        }
        if let Some((_, rest)) = id.split_once("://") {
            return rest.split(['/', '?', '#']).next().unwrap_or(rest).rsplit('@').next().unwrap_or(rest).to_owned();
        }
        let mut tail = id.rsplit('.').next().unwrap_or(id).chars();
        tail.next().map_or_else(String::new, |h| h.to_uppercase().chain(tail).collect())
    }

    /// B86: W9 の問い（`WHICH_APP`）の選択肢を押した（同じ名前を打った）一言 = 問いの前の人の一言（元の頼み）にその app を足した一言。押した名前だけを次の一言に
    /// すると頼みが消える（AI は「TextEdit」だけを分ける）。最後の Shibubu の行がその問いでない・選択肢に無い = None（今まで通り）。人の行は押した名前のまま。
    /// 例: `answered_ask(&[you("Change the README title"), ask], "TextEdit") == Some("Change the README title (use TextEdit)")`
    pub fn answered_ask(turns: &[Turn], said: &str) -> Option<String> {
        let at = turns.iter().rposition(|t| t.who == Who::Shibubu)?;
        let said = said.trim();
        if !turns[at].text.starts_with(WHICH_APP) || !turns[at].choices.iter().any(|c| c == said) {
            return None;
        }
        let asked = turns[..at].iter().rev().find(|t| t.who == Who::You)?;
        Some(format!("{} (use {said})", asked.text.trim()))
    }

    /// W9 本体の振り分けの決まり（caller の形）。負の対照（手で）: `route` の `ask` を None に → 2 本目が赤・`talk` の `acts.is_empty()` を外す → 3 本目が赤
    #[cfg(test)]
    mod w9_route_tests {
        use super::{route, Classified, OperationKind};
        fn act(target: Option<&str>, named: bool, candidates: &[&str]) -> Classified {
            Classified { kind: OperationKind::Act, want: "Open the guitar list".into(), target: target.map(str::to_owned), named, candidates: candidates.iter().map(|c| c.to_string()).collect() }
        }
        #[test]
        fn w9_route_sends_acts_to_the_engine_asks_once_and_leaves_the_rest_to_talk() {
            let r = route(&[act(Some("com.example.mixer"), true, &[]), Classified { kind: OperationKind::Question, want: "Is it open?".into(), ..Default::default() }]);
            assert!(r.engine && r.talk && r.ask.is_none() && r.hint().is_some_and(|h| h.contains("Open the guitar list")), "{r:?}");
            let r = route(&[act(None, false, &["com.example.mixer", "com.example.daw"])]);
            assert_eq!(r.ask.as_ref().map(|(_, c)| c.clone()), Some(vec!["com.example.mixer".to_string(), "com.example.daw".to_string()]));
            assert!(!r.engine && !r.talk, "{r:?}");
            let r = route(&[act(None, true, &[])]);
            assert!(r.ask.as_ref().is_some_and(|(q, c)| q.contains("Which app") && c.is_empty()) && !r.engine, "{r:?}");
            let r = route(&[Classified { kind: OperationKind::Change, want: "Add a button".into(), ..Default::default() }]);
            assert!(r.talk && !r.engine && r.ask.is_none() && r.hint().is_none(), "{r:?}");
            assert!(route(&[]).talk, "分けられなければ会話の AI");
        }
    }

    /// HOME-ATTACH-1101（owner 175 通目「300行以上のpateした文章…は chat を打つ下に長方形として小さくまとまるように」）: Home の欄にこの行数以上を
    /// 貼ると欄に入れず `Pasted` の添付にする（未満は今のまま欄に入る）。
    pub const PASTE_LINES: usize = 300;

    /// HOME-ATTACH-1101: 貼った文 → 添付。`PASTE_LINES` 行未満は None（欄にそのまま入る）。以上は確かめて Some（Err = 欄の下の 1 行・file と同じ順: 大きさ・秘密）。
    /// 今の AI が file を読めるかは見ない（owner 185 通目: AI が何であっても受け取って置く・読めるかは送る時の `recheck`）。
    /// 例（app.rs の貼り付けの受け口）: `if let Some(r) = chat::paste(&text) { line = r.and_then(|a| chat::add(&mut shell.attach, a)).err() }`
    pub fn paste(text: &str) -> Option<Result<Attachment, String>> {
        let lines = text.lines().count();
        (lines >= PASTE_LINES).then(|| pasted_ok(text).map(|()| Attachment::Pasted { text: text.to_owned(), lines }))
    }

    /// 貼った文を添えられるか（貼った時と送る直前の `recheck`）。大きさは file の上限・秘密は file と同じ門。
    fn pasted_ok(text: &str) -> Result<(), String> {
        let bytes = text.len() as u64;
        if bytes > FILE_MAX_BYTES {
            return Err(format!("Pasted text is too big ({} MB) · paste up to {} MB", bytes.div_ceil(1 << 20), FILE_MAX_BYTES >> 20));
        }
        if has_secret(text) {
            return Err("Pasted text looks like it has a password or key in it · remove it and paste again".into());
        }
        Ok(())
    }

    /// 考える深さ（`Effort ▸`）。`None` = Default = flag を付けない（利用者の CLI の既定）。語は claude 2.1.312 の `--help`
    /// （`--effort <level>`・low, medium, high, xhigh, max・2026-10-28 に G の pane で確かめた）。保存は `UiSettings::default_effort` に `flag` の語で。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Effort {
        Low,
        Medium,
        High,
        XHigh,
        Max,
    }

    impl Effort {
        pub const ALL: [Effort; 5] = [Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max];
        /// CLI の `--effort` に渡す語。
        pub const fn flag(self) -> &'static str {
            match self {
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High => "high",
                Effort::XHigh => "xhigh",
                Effort::Max => "max",
            }
        }
        /// menu の語。
        pub const fn label(self) -> &'static str {
            match self {
                Effort::Low => "Low",
                Effort::Medium => "Medium",
                Effort::High => "High",
                Effort::XHigh => "Extra high",
                Effort::Max => "Max",
            }
        }
        pub fn from_flag(s: &str) -> Option<Effort> {
            Effort::ALL.into_iter().find(|e| e.flag() == s)
        }
        /// 1 回の呼び出しの待ちの上限の倍率（High 2・Extra high 3・Max 5）。1 より大きい段の間は読んでいる 1 行に `Thinking longer`。
        pub const fn wait_factor(self) -> u32 {
            match self {
                Effort::Low | Effort::Medium => 1,
                Effort::High => 2,
                Effort::XHigh => 3,
                Effort::Max => 5,
            }
        }
    }

    /// G84: Claude Code の `Model ▸` の別名と menu の語。別名は claude 2.1.312 の `--help` の `--model` の行（`'fable', 'opus', or 'sonnet'`・2026-10-29 に G の pane で確かめた）。
    /// 出すのは CLI の `--help` に在った物だけ（`llm_cli::help_models` がこの表から `Accepts::models` の行を作る）。保存は `UiSettings::default_model` に別名の語で。
    /// PROVIDER-1101: Claude Code の表だけ。他の AI の一覧はその AI から読む（`Llm::models`）。
    // ponytail: 表は固定（help に新しい別名が出ても表に足すまで出さない）。help の語をそのまま全部出すなら表を help から作る
    pub const MODELS: [(&str, &str); 3] = [("fable", "Fable"), ("opus", "Opus"), ("sonnet", "Sonnet")];

    /// 今の provider が受けられる物（`Generated::accepts`）。受けられない項目は menu で押せない形と理由 1 行（`refusal`）。
    /// 契約（PROVIDER-1101 から・owner 180 通目「なぜollamaやgrokに変えるとmodelやeffortを切り替えられないの？」）: model と effort は provider ごとの一覧
    /// （前は Claude Code の固定の表の bit と `--effort` の有無だけ = Grok・Ollama では選べなかった）。Copy をやめた（一覧はその AI から読んだ String）。
    /// 呼ぶ側（chat の下の Provider / Model / Effort）: `for (id, label) in accepts.models() { … Intent::SetModel(Some(id.into())) }`・
    /// `for e in &accepts.efforts { … Intent::SetEffort(Some(*e)) }`・押せない時は `accepts.model_refusal()` / `accepts.refusal(None)` の 1 行（provider の名前で）。
    /// 例: `Accepts { ai: true, provider: "Grok".into(), efforts: vec![Effort::Low, Effort::High], models: vec![("grok-4".into(), "grok-4".into())], ..Default::default() }`
    /// 保証しない: 一覧が今の AI の答えと同じ事（読んだ時の写し・AI を替える / 確かめ直すと読み直す）。
    /// Wave: PROVIDER
    /// 契約（HOME-ATTACH-1101）: 添付は受けられなくても添えられ、送る時に `recheck` が断る（今の AI の名前で）。
    #[derive(Clone, Debug, PartialEq, Eq, Default)]
    pub struct Accepts {
        /// AI（CLI か動いている local LLM）が使える
        pub ai: bool,
        /// PROVIDER-1101: 今の provider の画面の名前（`Claude Code`・`Grok`・`Ollama` …・`Llm::provider_name`）。押せない理由の語に入れる。AI 無しは空
        pub provider: String,
        pub photos: bool,
        pub files: bool,
        /// PROVIDER-1101: 選べる考える深さ（`Effort::ALL` の中でその AI の flag が受ける段・`Llm::efforts`）。空 = 選べない（理由は `refusal(None)`）
        pub efforts: Vec<Effort>,
        /// PROVIDER-1101: 選べる model（渡す語, menu の語）= その AI から読んだ一覧（`Llm::models`）。空 = 選べない（理由は `model_refusal`）
        pub models: Vec<(String, String)>,
    }

    impl Accepts {
        /// 押せない理由の 1 行。`kind` = None は `Effort ▸`。受けられれば None。
        /// PROVIDER-1101: 語は今の provider の名前で（Claude Code は旗の無い古い版 = update の一手・他の AI はその AI が受けない事）。
        pub fn refusal(&self, kind: Option<AttachKind>) -> Option<String> {
            if !self.ai {
                return Some("Needs Claude Code on this Mac".into());
            }
            let (p, claude) = (self.provider.as_str(), self.provider == "Claude Code");
            match kind {
                Some(AttachKind::Photo) if !self.photos && claude => Some("Update Claude Code to attach pictures".into()),
                Some(AttachKind::Photo) if !self.photos => Some(format!("{p} doesn't take pictures here")),
                Some(AttachKind::File) if !self.files && claude => Some("Update Claude Code to attach files".into()),
                Some(AttachKind::File) if !self.files => Some(format!("{p} doesn't take files here")),
                None if self.efforts.is_empty() && claude => Some("This Claude Code has no effort setting · update it".into()),
                None if self.efforts.is_empty() => Some(format!("{p} has no effort setting")),
                _ => None,
            }
        }

        /// G84: `Model ▸` の行（渡す語, menu の語）。読んだ順（`Default` は呼ぶ側が頭に足す）。
        pub fn models(&self) -> impl Iterator<Item = (&str, &str)> {
            self.models.iter().map(|(id, label)| (id.as_str(), label.as_str()))
        }

        /// G84: 選んだ model の語 `id` を今の provider が受けるか（一覧に無い語は渡さない = AI の既定のまま）。
        pub fn model_ok(&self, id: &str) -> bool {
            self.models.iter().any(|(m, _)| m == id)
        }

        /// G84: `Model ▸` を押せない理由の 1 行（AI 無し・選べる model が 1 つも無い）。押せれば None。
        pub fn model_refusal(&self) -> Option<String> {
            if !self.ai {
                Some("Needs Claude Code on this Mac".into())
            } else if self.models.is_empty() && self.provider == "Claude Code" {
                Some("This Claude Code can't choose a model · update it".into())
            } else if self.models.is_empty() {
                Some(format!("{} has no models to choose", self.provider))
            } else {
                None
            }
        }
    }

    /// 添えられる数の上限。
    pub const MAX_ATTACHMENTS: usize = 5;
    /// 写真 1 枚の上限（5 MB）。
    pub const PHOTO_MAX_BYTES: u64 = 5 << 20;
    /// 写真でない file 1 つの上限（10 MB）。
    pub const FILE_MAX_BYTES: u64 = 10 << 20;

    /// HOME-ATTACH-1101: folder（と展開した zip）の中の file の数の上限（model に 1 つずつ名前を渡す）。
    pub const FOLDER_MAX_FILES: usize = 500;

    /// 1 つの file・folder・zip を確かめて添付にする（`+` で選んだ・窓に落とした・⌘V で貼った）。種類は形で決める（写真の拡張子なら写真・他は file）。
    /// 確かめる順: 在る・形式（写真 = png / jpg / jpeg / gif / webp・file = 文字か PDF・folder と zip = 中の file）・大きさ・秘密（`has_secret`）。
    /// 今の AI が読めるかは見ない（owner 185 通目「AI が何であっても Shibubu が受け取り chat bar に長方形として置く」）= 送る時の `recheck` が見る。
    /// HOME-ATTACH-1101（owner 180 通目）: folder は中身ごと（`folder_bytes` の門）・zip は `$TMPDIR` に展開した folder を同じ門に通してその folder を添える
    /// （`unzip`・名前は zip の名前のまま）。送る前の `recheck` は同じ関数 = 展開した folder を確かめ直す。
    /// Err = 欄の下の 1 行（何が・次の一手・file 名だけで path を出さない）。例: `let a = chat::attach(Path::new("/x/notes.md"))?;`
    pub fn attach(path: &Path) -> Result<Attachment, String> {
        let name = path.file_name().map_or_else(|| "That file".to_owned(), |n| n.to_string_lossy().into_owned());
        let meta = std::fs::metadata(path).map_err(|_| format!("{name} isn't there anymore · choose it again"))?;
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        // 反証 G-A: 形は拡張子と中身の先頭の byte の両方で決める（文字の file に .png を付けても写真にならない = 秘密の検査を飛ばさない）
        let mut head = [0u8; 12];
        let got = if meta.is_dir() { 0 } else { std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0) };
        let ext_photo = PHOTO_EXT.contains(&ext.as_str());
        let kind = if ext_photo && !meta.is_dir() { AttachKind::Photo } else { AttachKind::File };
        if kind == AttachKind::Photo && !is_picture(&head[..got]) {
            return Err(format!("{name} isn't a picture · attach a PNG, JPEG, GIF or WebP"));
        }
        let zip = ext == "zip" && head[..got].starts_with(b"PK");
        if meta.is_dir() || zip {
            let dir = if zip { unzip(path, &name, &meta)? } else { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) };
            let bytes = folder_bytes(&dir, &name)?;
            return Ok(Attachment::File { path: dir, kind, bytes });
        }
        let max = if kind == AttachKind::Photo { PHOTO_MAX_BYTES } else { FILE_MAX_BYTES };
        if meta.len() > max {
            return Err(format!("{name} is too big ({} MB) · attach files up to {} MB", meta.len().div_ceil(1 << 20), max >> 20));
        }
        if kind == AttachKind::File {
            let bytes = std::fs::read(path).map_err(|_| format!("{name} couldn't be read · check that it isn't locked"))?;
            // ponytail: PDF の中の文字は読まないので秘密も探さない（PDF を開く crate が要る）。写真の中の文字も同じ
            if !bytes.starts_with(b"%PDF") {
                let text = std::str::from_utf8(&bytes).map_err(|_| format!("{name} isn't text, a PDF or a picture"))?;
                if has_secret(text) {
                    return Err(format!("{name} looks like it has a password or key in it · remove it and attach again"));
                }
            }
        }
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Ok(Attachment::File { path, kind, bytes: meta.len() })
    }

    /// PNG・JPEG・GIF・WebP の先頭の byte。
    fn is_picture(h: &[u8]) -> bool {
        h.starts_with(b"\x89PNG") || h.starts_with(&[0xFF, 0xD8, 0xFF]) || h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") || (h.starts_with(b"RIFF") && h.get(8..12) == Some(&b"WEBP"[..]))
    }

    /// 写真と分かる拡張子（小文字）。
    const PHOTO_EXT: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

    /// HOME-ATTACH-1101: folder の中で入れない物の名前（隠し file・folder〈`.git`・`.env` など〉と、macOS の zip の資源の folder）。
    /// llm_cli の `copy_tree`（model に渡す写し）も同じ 2 つを入れない = 数えた物と渡す物が同じ。
    pub fn skipped_in_folder(name: &str) -> bool {
        name.starts_with('.') || name == "__MACOSX"
    }

    /// HOME-ATTACH-1101: folder の中の file（`skipped_in_folder` と symlink は入れない）の大きさの和。file の数は `FOLDER_MAX_FILES` まで・和は `FILE_MAX_BYTES` まで・
    /// 文字の file は秘密の門（`has_secret`）・文字でない file（写真・PDF・他）はそのまま。`name` = 1 行に出す名前（zip なら zip の名前）。
    /// Err の例: `src in project looks like it has a password or key in it · remove it and attach again`。
    fn folder_bytes(dir: &Path, name: &str) -> Result<u64, String> {
        let (mut bytes, mut files, mut todo) = (0u64, 0usize, vec![dir.to_path_buf()]);
        while let Some(d) = todo.pop() {
            let entries = std::fs::read_dir(&d).map_err(|_| format!("{name} couldn't be read · check that you can open it"))?;
            for e in entries.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if skipped_in_folder(&e.file_name().to_string_lossy()) || ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    todo.push(e.path());
                    continue;
                }
                files += 1;
                if files > FOLDER_MAX_FILES {
                    return Err(format!("{name} has more than {FOLDER_MAX_FILES} files · attach a smaller folder"));
                }
                let body = std::fs::read(e.path()).map_err(|_| format!("{name} couldn't be read · check that you can open it"))?;
                bytes += body.len() as u64;
                if bytes > FILE_MAX_BYTES {
                    return Err(format!("{name} is too big · attach up to {} MB", FILE_MAX_BYTES >> 20));
                }
                if std::str::from_utf8(&body).is_ok_and(has_secret) {
                    let inner = e.path().strip_prefix(dir).map_or_else(|_| e.file_name().to_string_lossy().into_owned(), |p| p.display().to_string());
                    return Err(format!("{inner} in {name} looks like it has a password or key in it · remove it and attach again"));
                }
            }
        }
        if files == 0 {
            return Err(format!("{name} has no files to attach"));
        }
        Ok(bytes)
    }

    /// HOME-ATTACH-1101: zip を `$TMPDIR/shibubu-unzipped/<zip の path・大きさ・時刻の hash>/<zip の名前>/` に展開して、その folder を返す（同じ zip は 1 回だけ展開）。
    /// 展開する前に目次（`zipinfo -t`）の展開した大きさの和を見る（`FILE_MAX_BYTES` を超えれば展開しない）。展開は macOS の `ditto -x -k`（別の名前に書いてから移す）。
    // ponytail: 目次の大きさを信じる（目次が嘘の zip は展開してから folder_bytes で止まる・disk には書く）。古い展開は OS の $TMPDIR の掃除に任せる
    fn unzip(zip: &Path, name: &str, meta: &std::fs::Metadata) -> Result<PathBuf, String> {
        use std::hash::{Hash, Hasher};
        let bad = || format!("{name} couldn't be opened · check that it's a working zip");
        let info = std::process::Command::new("/usr/bin/zipinfo").arg("-t").arg(zip).output().map_err(|_| bad())?;
        // 例: `4 files, 8 bytes uncompressed, 12 bytes compressed:  -50.0%`
        let text = String::from_utf8_lossy(&info.stdout);
        let words: Vec<&str> = text.split_whitespace().collect();
        let size = words.iter().position(|w| w.starts_with("uncompressed")).and_then(|i| words.get(i.checked_sub(2)?)?.parse::<u64>().ok());
        let size = size.filter(|_| info.status.success()).ok_or_else(bad)?;
        if size > FILE_MAX_BYTES {
            return Err(format!("{name} is too big unzipped ({} MB) · attach up to {} MB", size.div_ceil(1 << 20), FILE_MAX_BYTES >> 20));
        }
        let mut h = std::hash::DefaultHasher::new();
        (zip, meta.len(), meta.modified().ok()).hash(&mut h);
        let base = std::env::temp_dir().join("shibubu-unzipped").join(format!("{:016x}", h.finish()));
        let dir = base.join(name);
        if !dir.is_dir() {
            let part = base.join(".partial");
            let _ = std::fs::remove_dir_all(&part);
            std::fs::create_dir_all(&base).map_err(|_| bad())?;
            let ok = std::process::Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(zip).arg(&part).status().is_ok_and(|s| s.success());
            if !ok || std::fs::rename(&part, &dir).is_err() {
                let _ = std::fs::remove_dir_all(&part);
                return Err(bad());
            }
        }
        Ok(std::fs::canonicalize(&dir).unwrap_or(dir))
    }

    /// 添付の並びに 1 つ足す（同じ物は足さない・`MAX_ATTACHMENTS` まで）。Err = 欄の下の 1 行。
    pub fn add(list: &mut Vec<Attachment>, a: Attachment) -> Result<(), String> {
        let same = |x: &Attachment| match (x, &a) {
            (Attachment::File { path: p, .. }, Attachment::File { path: q, .. }) => p == q,
            (x, a) => x == a,
        };
        if list.iter().any(same) {
            return Err(format!("{} is already attached", a.name()));
        }
        if list.len() >= MAX_ATTACHMENTS {
            return Err(format!("You can attach up to {MAX_ATTACHMENTS} things · remove one first"));
        }
        list.push(a);
        Ok(())
    }

    /// 送る直前（Send・Preview plan・Accept）に確かめ直す。Err = 送らない理由の 1 行（欄と長方形はそのまま残る）。
    /// 順: 今の AI が読めるか（owner 185 通目: 添える時は AI を問わず受け、読めない時は送る時に今の AI の名前 `who` で言う・貼った文は file の道）→
    /// file が選んだ後に消えた・大きくなった・秘密を書き足した。例: `chat::recheck(&shell.attach, &generated.accepts(), &generated.ai_name())`
    pub fn recheck(list: &[Attachment], accepts: &Accepts, who: &str) -> Result<(), String> {
        for a in list {
            let kind = match a {
                Attachment::File { kind, .. } => *kind,
                Attachment::Pasted { .. } => AttachKind::File,
            };
            if let Some(why) = accepts.refusal(Some(kind)) {
                let name = if matches!(a, Attachment::Pasted { .. }) { "the pasted text".to_owned() } else { a.name() };
                let what = if kind == AttachKind::Photo { "pictures" } else { "files" };
                return Err(if accepts.ai { format!("{who} can't read {what} · remove {name} or switch the AI in Provider") } else { format!("Can't send {name} · {why}") });
            }
            match a {
                Attachment::File { path, .. } => {
                    attach(path)?;
                }
                Attachment::Pasted { text, .. } => pasted_ok(text)?,
            }
        }
        Ok(())
    }

    /// 文に秘密（鍵の頭・PEM の私有鍵・`password =` / `api_key:` の形）が入っているか。true なら送らない。
    /// 保証しない: 全ての秘密を見つける事（形の決まった物だけ・誤って止めるより送らない側に倒す）。
    pub fn has_secret(text: &str) -> bool {
        let low = text.to_lowercase();
        if low.contains("-----begin") && low.contains("private key-----") {
            return true;
        }
        // 鍵の頭の後に鍵の字が 16 字以上続く物（文書の中の頭の語だけは止めない）
        let tok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        // 反証 G-A: URL そのものが鍵の webhook と、`scheme://user:password@host` の password
        if ["hooks.slack.com/services/", "discord.com/api/webhooks/", "discordapp.com/api/webhooks/"].iter().any(|h| low.contains(h)) {
            return true;
        }
        for (at, _) in low.match_indices("://") {
            let authority = low[at + 3..].split(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace()).next().unwrap_or("");
            if let Some((user, _)) = authority.rsplit_once('@') {
                if user.split_once(':').is_some_and(|(_, pw)| pw.chars().count() >= 4 && !pw.starts_with(['<', '$', '{', '*'])) {
                    return true;
                }
            }
        }
        for head in ["sk-ant-", "sk-proj-", "sk-live-", "sk_live_", "sk_test_", "rk_live_", "ghp_", "gho_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "akia", "aiza"] {
            for (at, _) in low.match_indices(head) {
                if low[at + head.len()..].chars().take_while(|c| tok(*c)).count() >= 16 {
                    return true;
                }
            }
        }
        // `password = …`・`api_key: "…"` の形（値が 6 字以上で、`<…>`・`${…}` の置き場でも `input()`・`env["X"]` の code でもない）
        low.lines().any(|line| {
            ["password", "passwd", "secret", "api_key", "apikey", "api-key", "access_token", "auth_token", "client_secret", "private_key"].iter().any(|k| {
                line.match_indices(k).any(|(at, _)| {
                    // 反証 G-A: 鍵の名前の続き（`AWS_SECRET_ACCESS_KEY`・`SECRET_KEY` の `_access_key`）は、名前が key・secret・token・password で終わる時だけ読み飛ばす
                    // （空白は越えない = 文の中の「secret recipe:」は止めない・`api_key_rotation = weekly` も止めない）
                    let after = &line[at + k.len()..];
                    let rest = after.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                    let name_rest = &after[..after.len() - rest.len()];
                    if !name_rest.is_empty() && !["key", "secret", "token", "password", "passwd", "pwd"].iter().any(|e| name_rest.ends_with(e)) {
                        return false;
                    }
                    let rest = rest.trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
                    let Some(value) = rest.strip_prefix([':', '=']) else { return false };
                    let value = value.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
                    value.chars().count() >= 6 && !value.starts_with(['<', '$', '{', '*']) && !value.contains(['(', '[']) && !value.chars().all(|c| c == 'x' || c == '*')
                })
            })
        })
    }

    /// PASSWORD-1101 の 3（owner 252）: chat の一言が password らしいか。true なら app.rs が AI に送らず・会話に足さず・保存せずに `Shell::held` に持ち、
    /// card（`Save to {manager}` / `Delete`）を出す（打ちかけの保存 `snapshot_drafts` も書かない）。`has_secret`（鍵の頭・`password = …` の形）に 2 つを足す:
    /// ① 会話の言い方 = password の語（`password`・`passwd`・`passcode`・`pw`・`pin`・`パスワード`・`パスコード`・`暗証番号`）の後に `is`・`was`・`:`・`=`・`は`・`：`・`＝`、
    /// その後の 1 語が 4 字以上で数字か記号か大文字を含む（`my password is weak` は止めない）。
    /// ② 一言が空白の無い 1 語で 8 字以上・小文字・大文字・数字・記号の 4 種を全部含む（URL・mail・path・`#…` は除く）。
    /// 保証しない: 全ての password を見つける事（形で読む・小文字だけの一言は通る）・誤って止めない事（止めた時は Delete で消して言い直せる）。
    /// 例: `looks_like_password("my password is hunter2!x")` = true・`looks_like_password("Track my inventory")` = false
    /// Wave: PASSWORD-1101
    pub fn looks_like_password(text: &str) -> bool {
        if has_secret(text) {
            return true;
        }
        const KEYS: [&str; 8] = ["password", "passwd", "passcode", "pw", "pin", "パスワード", "パスコード", "暗証番号"];
        const JOINT: [char; 5] = [':', '=', '：', '＝', 'は'];
        let key = |w: &str| KEYS.contains(&w.to_lowercase().as_str());
        let strong = |v: &str| v.chars().count() >= 4 && v.chars().any(|c| c.is_ascii_digit() || c.is_uppercase() || c.is_ascii_punctuation());
        let words: Vec<&str> = text.split_whitespace().collect();
        let at = |i: usize| words.get(i).copied().unwrap_or("");
        // ① `password: X`・`パスワードはX`（語と値が 1 語）・`pw: X`・`password is X`・`the pin is X`
        let said = words.iter().enumerate().any(|(i, w)| match w.split_once(JOINT) {
            Some((head, tail)) if key(head) => strong(if tail.is_empty() { at(i + 1) } else { tail }),
            _ => key(*w) && ["is", "was", ":", "=", "は", "：", "＝"].contains(&at(i + 1).to_lowercase().as_str()) && strong(at(i + 2)),
        });
        // ② 1 語だけの強い文字列（URL・path・`~`・`#`・mail は除く）
        let alone = matches!(words.as_slice(), [w] if w.chars().count() >= 8
            && w.chars().any(char::is_lowercase)
            && w.chars().any(char::is_uppercase)
            && w.chars().any(|c| c.is_ascii_digit())
            && w.chars().any(|c| c.is_ascii_punctuation())
            && !w.contains("://")
            && !w.starts_with(['/', '~', '#'])
            && !(w.contains('@') && w.contains('.')));
        said || alone
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// PASSWORD-1101 の 3: 会話の言い方と 1 語の強い文字列を止め、普通の一言・password の語だけ・URL・mail・path・版の名前は止めない。
        /// 負の対照: ② の腕を外す → `Tr0ub4dor&3` が通って赤 / ① の腕を外す → `パスワードは Abc12345` が通って赤。
        #[test]
        fn password_1101_chat_text_that_looks_like_a_password() {
            for yes in [
                "my password is SHIBUBU-TEST-SECRET-7f3a9c2e",
                "My GitHub password is hunter2!x",
                "パスワードは Abc12345",
                "pw: Abc12345!",
                "the pin is 4821",
                "Tr0ub4dor&3",
                "SHIBUBU-TEST-SECRET-7f3a9c2e",
                "password = s3cr3t-value",
                "ghp_0123456789abcdefghij0123",
            ] {
                assert!(looks_like_password(yes), "止めない: {yes}");
            }
            for no in [
                "Track my inventory",
                "1Password を使って",
                "change my password",
                "my password is weak",
                "パスワードを変えたい",
                "the pin is in the drawer",
                "https://example.com/a?b=C1d!",
                "me@Example1.test",
                "/opt/Data/Project1!",
                "v1.2.3-beta",
                "iPhone15",
                "2026-11-02",
            ] {
                assert!(!looks_like_password(no), "止めた: {no}");
            }
        }

        fn dir(what: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("g104-chat-{what}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }
        fn cli() -> Accepts {
            Accepts { ai: true, provider: "Claude Code".into(), photos: true, files: true, efforts: Effort::ALL.to_vec(), ..Default::default() }
        }

        /// G104（CHAT-001）: 秘密（鍵・token・password）を含む file は添えられず理由 1 行・含まない file・文書の中の語だけ・置き場の値は添えられる。
        /// 負の対照: attach の `has_secret` の門を外す → 鍵の file が通って赤。
        #[test]
        fn g104_a_file_with_a_secret_stops_and_a_plain_one_attaches() {
            let d = dir("secret");
            let put = |name: &str, body: &str| {
                let p = d.join(name);
                std::fs::write(&p, body).unwrap();
                p
            };
            for (name, body) in [
                ("key.env", "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwx\n"),
                ("db.yml", "user: app\npassword: hunter2hunter2\n"),
                ("conf.json", "{\"client_secret\": \"9f8e7d6c5b4a\"}"),
                ("id_rsa.txt", "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"),
                ("gh.md", "Token: ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123\n"),
            ] {
                let e = attach(&put(name, body)).expect_err(name);
                assert_eq!(e, format!("{name} looks like it has a password or key in it · remove it and attach again"));
            }
            for (name, body) in [
                ("notes.md", "Meeting notes\n- ship the demo\n"),
                ("howto.md", "Keys start with ghp_ or sk-ant- · put your password in 1Password\n"),
                ("sample.env", "password: <your password>\napi_key=${API_KEY}\nsecret = os.environ[\"S\"]\n"),
                ("ünïcödé.md", "İİİ ghp_short\n"),
            ] {
                let a = attach(&put(name, body)).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert!(matches!(&a, Attachment::File { kind: AttachKind::File, .. }), "{a:?}");
                assert_eq!(a.name(), name);
            }
            let _ = std::fs::remove_dir_all(&d);
        }

        /// 形・大きさ・在る・text でない file の拒否。AI 無し・今の AI が読めない形は添えられて、送る時（recheck）に今の AI の名前で 1 行。
        #[test]
        fn g104_attach_says_why_a_file_cannot_go() {
            let d = dir("why");
            let notes = d.join("notes.md");
            std::fs::write(&notes, "hi").unwrap();
            let shot = d.join("Shot.PNG");
            std::fs::write(&shot, [0x89, b'P', b'N', b'G']).unwrap();
            let big = d.join("big.log");
            std::fs::File::create(&big).unwrap().set_len(FILE_MAX_BYTES + 1).unwrap();
            let bin = d.join("blob.bin");
            std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x81]).unwrap();
            let fake = d.join("notes.png");
            std::fs::write(&fake, "hi").unwrap();
            assert!(matches!(attach(&shot), Ok(Attachment::File { kind: AttachKind::Photo, .. })), "拡張子の大小は問わない");
            assert_eq!(attach(&fake).unwrap_err(), "notes.png isn't a picture · attach a PNG, JPEG, GIF or WebP");
            assert_eq!(attach(&big).unwrap_err(), "big.log is too big (11 MB) · attach files up to 10 MB");
            assert_eq!(attach(&bin).unwrap_err(), "blob.bin isn't text, a PDF or a picture");
            assert_eq!(attach(&d.join("gone.md")).unwrap_err(), "gone.md isn't there anymore · choose it again");
            // owner 185 通目: AI が無くても・今の AI が読めなくても添える（読めるかは送る時の recheck が今の AI の名前で言う）
            let (n, s) = (attach(&notes).expect("AI を問わず添える"), attach(&shot).unwrap());
            assert_eq!(recheck(&[n.clone()], &Accepts::default(), ""), Err("Can't send notes.md · Needs Claude Code on this Mac".into()));
            let ollama = Accepts { photos: false, files: false, ..cli() };
            assert_eq!(recheck(&[s], &ollama, "Ollama"), Err("Ollama can't read pictures · remove Shot.PNG or switch the AI in Provider".into()));
            assert_eq!(recheck(&[n], &ollama, "Ollama"), Err("Ollama can't read files · remove notes.md or switch the AI in Provider".into()));
            assert_eq!(Accepts { efforts: Vec::new(), ..cli() }.refusal(None).as_deref(), Some("This Claude Code has no effort setting · update it"));
            assert_eq!(cli().refusal(None), None);
            let _ = std::fs::remove_dir_all(&d);
        }

        /// HOME-ATTACH-1101（owner 180 通目「fileやzip, folderは全て同じカテゴリーのfileとして扱う」）: folder は中身ごと 1 つの file の添付
        /// （大きさ = 隠し file と symlink を除いた中身の和・中の秘密は中の名前つきの 1 行・空・上限）。zip は展開した folder（名前は zip のまま・中身が読める）。
        /// 送る前の確かめ直しも展開した folder を見る。負の対照: folder_bytes の has_secret の門を外す → src/conf.yml の鍵が通って赤・
        /// attach の zip の腕を外す → zip が「text でない」で赤。
        #[test]
        fn home_attach_folders_and_zips_are_files() {
            let d = dir("folder");
            let proj = d.join("project");
            std::fs::create_dir_all(proj.join("src")).unwrap();
            std::fs::create_dir_all(proj.join(".git")).unwrap();
            std::fs::write(proj.join("README.md"), "hello\n").unwrap();
            std::fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
            std::fs::write(proj.join(".env"), "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwx\n").unwrap();
            std::fs::write(proj.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            std::os::unix::fs::symlink("/etc/hosts", proj.join("hosts")).unwrap();
            let a = attach(&proj).unwrap();
            assert_eq!(a.name(), "project");
            assert!(matches!(a, Attachment::File { kind: AttachKind::File, bytes: 19, .. }), "隠し file・symlink を数えた: {a:?}");
            let zipped = d.join("project.zip");
            let ok = std::process::Command::new("/usr/bin/ditto").args(["-c", "-k", "--sequesterRsrc"]).arg(&proj).arg(&zipped).status().unwrap();
            assert!(ok.success());
            let z = attach(&zipped).unwrap();
            assert_eq!(z.name(), "project.zip", "zip の名前が残らない");
            let Attachment::File { path, kind: AttachKind::File, bytes: 19 } = &z else { panic!("{z:?}") };
            assert_eq!(std::fs::read_to_string(path.join("src/main.rs")).unwrap(), "fn main() {}\n", "展開した中身が読めない");
            assert_eq!(attach(&zipped), Ok(z.clone()), "同じ zip を 2 度展開して別の物になった");
            assert_eq!(recheck(&[a.clone(), z], &cli(), "Claude Code"), Ok(()));
            std::fs::write(proj.join("src/conf.yml"), "password: hunter2hunter2\n").unwrap();
            assert_eq!(attach(&proj).unwrap_err(), "src/conf.yml in project looks like it has a password or key in it · remove it and attach again");
            assert!(recheck(&[a], &cli(), "Claude Code").unwrap_err().contains("password or key"), "添えた後に書き足した秘密");
            let empty = d.join("empty");
            std::fs::create_dir_all(empty.join(".cache")).unwrap();
            assert_eq!(attach(&empty).unwrap_err(), "empty has no files to attach");
            let big = d.join("big");
            std::fs::create_dir_all(&big).unwrap();
            for i in 0..2 {
                std::fs::write(big.join(format!("{i}.log")), "x".repeat((FILE_MAX_BYTES / 2 + 1) as usize)).unwrap();
            }
            assert_eq!(attach(&big).unwrap_err(), "big is too big · attach up to 10 MB");
            let broken = d.join("broken.zip");
            std::fs::write(&broken, b"PK\x03\x04 not really").unwrap();
            assert_eq!(attach(&broken).unwrap_err(), "broken.zip couldn't be opened · check that it's a working zip");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// 反証 G-A の直し（G104）: 拡張子が写真でも中身が写真でなければ写真にしない（文字の key.png は秘密の検査を通る）・本物の PNG の頭は写真。
        /// 鍵の名前の続きは key 等で終わる時だけ秘密（`api_key_rotation = weekly` は止めない）。負の対照: is_picture の門を外す → key.png が写真で通って赤
        #[test]
        fn g_a_a_text_file_named_png_is_not_a_picture_and_its_key_is_caught() {
            let d = dir("gafix");
            let key = d.join("key.png");
            std::fs::write(&key, "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwx\n").unwrap();
            assert_eq!(attach(&key).unwrap_err(), "key.png isn't a picture · attach a PNG, JPEG, GIF or WebP");
            let renamed = d.join("key.txt");
            std::fs::rename(&key, &renamed).unwrap();
            assert_eq!(attach(&renamed).unwrap_err(), "key.txt looks like it has a password or key in it · remove it and attach again");
            let real = d.join("real.png");
            std::fs::write(&real, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
            assert!(matches!(attach(&real), Ok(Attachment::File { kind: AttachKind::Photo, .. })));
            assert!(!has_secret("api_key_rotation = weekly\nsecret recipe: grandma's\n"), "鍵でない設定と文を止めた");
            assert!(has_secret("AWS_SECRET_ACCESS_KEY=Zq7xWm2Kp9Lr4Tn6\n"));
            let _ = std::fs::remove_dir_all(&d);
        }

        /// 同じ物は 2 度足さない・5 つまで。送る前の確かめ直しは消えた・秘密を書き足した file を止める。
        /// 負の対照: recheck の中身を `Ok(())` にする → 消えた file が通って赤。
        #[test]
        fn g104_limits_and_the_recheck_before_sending() {
            let d = dir("recheck");
            let put = |name: &str| {
                let p = d.join(name);
                std::fs::write(&p, "plain").unwrap();
                p
            };
            let mut list = Vec::new();
            for i in 0..MAX_ATTACHMENTS {
                add(&mut list, attach(&put(&format!("{i}.md"))).unwrap()).unwrap();
            }
            assert_eq!(add(&mut list, attach(&put("0.md")).unwrap()).unwrap_err(), "0.md is already attached");
            assert_eq!(add(&mut list, attach(&put("9.md")).unwrap()).unwrap_err(), "You can attach up to 5 things · remove one first");
            assert_eq!(list.len(), MAX_ATTACHMENTS);

            let notes = put("notes.md");
            let a = vec![attach(&notes).unwrap()];
            assert_eq!(recheck(&a, &cli(), "Claude Code"), Ok(()));
            std::fs::write(&notes, "password=hunter2hunter2").unwrap();
            assert!(recheck(&a, &cli(), "Claude Code").unwrap_err().contains("password or key"), "添えた後に書き足した秘密");
            std::fs::remove_file(&notes).unwrap();
            assert_eq!(recheck(&a, &cli(), "Claude Code").unwrap_err(), "notes.md isn't there anymore · choose it again");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// HOME-ATTACH-1101: 300 行から貼った文は添付（名前 `Pasted text · 300 lines`）・299 行は None（欄に入る）・秘密は理由 1 行・AI 無しと読めない AI は送る時に 1 行・
        /// 同じ文は 2 度足さない・送る前の確かめ直しも同じ門。負の対照: paste の `>= PASTE_LINES` を `> PASTE_LINES` にする → 300 行が None で赤。
        #[test]
        fn home_attach_300_lines_become_one_attachment_and_299_do_not() {
            let text = |n: usize| (1..=n).map(|i| format!("line {i}\n")).collect::<String>();
            assert_eq!(paste(&text(299)), None, "299 行は欄に入る");
            let a = paste(&text(300)).expect("300 行が添付にならない").unwrap();
            assert_eq!(a.name(), "Pasted text · 300 lines");
            assert!(matches!(&a, Attachment::Pasted { text: t, lines: 300 } if *t == text(300)));
            let mut list = Vec::new();
            add(&mut list, a.clone()).unwrap();
            assert_eq!(add(&mut list, a.clone()).unwrap_err(), "Pasted text · 300 lines is already attached");
            assert_eq!(recheck(&list, &cli(), "Claude Code"), Ok(()));
            let leaked = format!("{}ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwx\n", text(300));
            assert_eq!(paste(&leaked), Some(Err("Pasted text looks like it has a password or key in it · remove it and paste again".into())));
            assert_eq!(recheck(&list, &Accepts::default(), "").unwrap_err(), "Can't send the pasted text · Needs Claude Code on this Mac");
            assert_eq!(recheck(&list, &Accepts { files: false, photos: false, ..cli() }, "Ollama").unwrap_err(), "Ollama can't read files · remove the pasted text or switch the AI in Provider");
        }
    }
} // mod chat
