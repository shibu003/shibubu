//! 候補の生成（AGT-003）: LLM 無しのローカル heuristics だけ。読むのは定義（AppDefinitionV3）と呼び側が渡した観測（Observed）で、
//! 観測のうち許可の無い source（readiness が Local / Ready 以外）に属する物は捨てる（ungranted source を見に行かない）。
//! 出す物は agent_proposal_v1 の Suggestion そのもの（status: candidate）。action / component は定義に在る物しか参照しない。
//! 4 つ: 10 行超の table / list に検索を足す・空の一覧に CTA（既存の local_write action）を足す・
//! 動作の無い button に名前の近い action を繋ぐ・action の input error から form の修正案（または接続の案内）。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::domain::action::{ActionDefinition, ActionReadiness, Effect, InvocationError};
use super::domain::app::AppDefinitionV3;
use super::domain::binding::SourceReadiness;
use super::domain::patch::{AppPatch, PatchOp};
use super::domain::suggestion::{
    Change, Evidence, EvidenceKind, ProposalSchemaVersion, Risk, Suggestion, SuggestionKind, SuggestionStatus,
};
use super::domain::ui::UiNode;
use super::grounding::risk_of_effect;

/// これより多い行数の table / list に検索を提案する。
pub const SEARCH_ROWS_THRESHOLD: usize = 10;

/// 呼び側（renderer / runtime）が見た事。agent は自分で source を読まない。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Observed {
    /// source id → 件数。
    pub rows: BTreeMap<String, usize>,
    /// 直近の action error（同じ物が何度来ても提案は 1 つ）。
    pub errors: Vec<ActionError>,
    /// rail の入力（untrusted な文）。evidence の ref は `requests.<index>`。
    pub requests: Vec<String>,
}

impl Observed {
    /// grounding が解決できる、定義の外に在る ref（error と request）。
    pub fn known_refs(&self) -> BTreeSet<String> {
        let mut set: BTreeSet<String> = self.errors.iter().map(|e| format!("errors.{}", e.action_ref)).collect();
        set.extend((0..self.requests.len()).map(|i| format!("requests.{i}")));
        set
    }
    pub fn merge(&mut self, other: Observed) {
        self.rows.extend(other.rows);
        for e in other.errors {
            if !self.errors.contains(&e) {
                self.errors.push(e);
            }
        }
        self.requests.extend(other.requests);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionError {
    pub action_ref: String,
    pub error: InvocationError,
}

/// PROACTIVE_SUGGESTION_ENGINE §3 の category（open-ended）。dedupe key と feedback の mute の単位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    AddSearchOrFilter,
    CompleteMissingDependency,
    RepairBinding,
    FixInput,
    ConnectCandidateSource,
}

impl Category {
    pub const fn name(self) -> &'static str {
        match self {
            Category::AddSearchOrFilter => "add_search_or_filter",
            Category::CompleteMissingDependency => "complete_missing_dependency",
            Category::RepairBinding => "repair_binding",
            Category::FixInput => "fix_input",
            Category::ConnectCandidateSource => "connect_candidate_source",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub suggestion: Suggestion,
    pub category: Category,
}

/// 許可された source か（Local か Ready）。それ以外の source の観測は使わない（AGT-003 AC）。
pub fn source_granted(def: &AppDefinitionV3, source_ref: &str) -> bool {
    def.source(source_ref)
        .is_some_and(|s| matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready))
}

/// 定義と観測から候補を出す。順序は決定的（木の先行順 → error の順）。
pub fn generate(def: &AppDefinitionV3, observed: &Observed) -> Vec<Candidate> {
    let mut out = Vec::new();
    out.extend(add_search(def, observed, None));
    out.extend(empty_cta(def, observed));
    out.extend(bind_buttons(def));
    out.extend(from_errors(def, observed));
    out
}

/// rail の入力（untrusted）から。文は「どの heuristics を回すか」を選ぶだけで、scope / 権限 / action の実行には一切繋がらない（AGT-014）。
/// `request_index` は observed.requests の添字（evidence の ref）。
pub fn from_text(def: &AppDefinitionV3, observed: &Observed, request_index: usize) -> Vec<Candidate> {
    let text = observed.requests.get(request_index).map(|s| s.to_lowercase()).unwrap_or_default();
    let wants_search = ["検索", "絞り込", "フィルタ", "search", "filter"].iter().any(|w| text.contains(w));
    if wants_search {
        add_search(def, observed, Some(request_index))
    } else {
        Vec::new()
    }
}

/// 入力の文が対応している語（rail の返事に使う）。
pub const TEXT_COMMANDS: &[&str] = &["Add search"];

// ---------------------------------------------------------------------------

fn ev(kind: EvidenceKind, r: impl Into<String>) -> Evidence {
    Evidence { kind, r#ref: r.into(), observed_at: None, uncertain: None }
}

fn parent_of<'a>(root: &'a UiNode, key: &str) -> Option<(&'a UiNode, usize)> {
    root.nodes().into_iter().find_map(|n| n.children.iter().position(|c| c.key == key).map(|i| (n, i)))
}

fn name_of(n: &UiNode) -> String {
    n.label
        .clone()
        .or_else(|| n.props.get("label").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| n.key.clone())
}

fn referenced_actions(root: &UiNode) -> BTreeSet<String> {
    root.nodes().into_iter().filter_map(|n| n.action_ref.clone()).collect()
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn suggestion(
    def: &AppDefinitionV3,
    category: Category,
    target: &str,
    title: String,
    reason: String,
    effect: String,
    kind: SuggestionKind,
    risk: Risk,
    permission_scopes: Vec<String>,
    undoable: bool,
    evidence: Vec<Evidence>,
    summary: String,
    operations: Vec<AppPatch>,
) -> Suggestion {
    Suggestion {
        schema_version: ProposalSchemaVersion::V1,
        id: format!("{}.{}", category.name(), target),
        app_id: def.id.clone(),
        app_version: def.version,
        title,
        reason,
        evidence,
        target_ref: target.to_owned(),
        kind,
        effect,
        risk,
        permission_scopes,
        undoable,
        status: SuggestionStatus::Candidate,
        expires_at: None,
        change: Change { summary, operations },
    }
}

/// `key` の node の前（`after` なら後）に足す先: 上へ辿って最初の `column`（縦に積む容器）と、その中の位置。
/// split / row / tabs の子に足すと面や tab の数が変わるので、その外側の column に置く。column が無ければ直の親。
fn placement<'a>(root: &'a UiNode, key: &str, after: bool) -> Option<(&'a UiNode, usize)> {
    let direct = parent_of(root, key)?;
    let mut at = direct;
    while at.0.component != "column" {
        match parent_of(root, &at.0.key) {
            Some(up) => at = up,
            None => {
                at = direct;
                break;
            }
        }
    }
    Some((at.0, at.1 + usize::from(after)))
}

/// `container` の `index` の位置に `node` を足す op 列: insert（末尾に足す）→ index 以降の既存の子を元の順に末尾へ move。
/// 足した node 自体は move しない: validate_suggestion は今の木に無い key を target にした op を MissingTarget で落とすため、
/// op の target_key は全部今の木に在る key だけにする。
fn insert_at(container: &UiNode, index: usize, node: Value) -> Vec<AppPatch> {
    let mut ops = vec![AppPatch { op: PatchOp::Insert, target_key: container.key.clone(), value: Some(node) }];
    ops.extend(container.children.iter().skip(index).map(|c| AppPatch {
        op: PatchOp::Move,
        target_key: c.key.clone(),
        value: Some(json!({ "parent": container.key })),
    }));
    ops
}

/// その source を既に絞り込める UI が在るか（検索 role・`-search` の key・filter / query / search の path に bind された node）。
fn has_search(root: &UiNode, source_ref: &str) -> bool {
    root.nodes().into_iter().any(|n| {
        n.binding.as_ref().is_some_and(|b| b.source_ref == source_ref)
            && (n.role.as_deref() == Some("search")
                || n.key.ends_with("-search")
                || n.binding.as_ref().is_some_and(|b| matches!(b.path.as_str(), "filter" | "query" | "search")))
    })
}

/// 1. 10 行超の table / list に検索入力を足す（`request` が Some なら入力から: 件数の閾値は見ない）。
fn add_search(def: &AppDefinitionV3, observed: &Observed, request: Option<usize>) -> Vec<Candidate> {
    let root = &def.composition.root;
    let mut out = Vec::new();
    for node in root.nodes() {
        if node.component != "table" && node.component != "list" {
            continue;
        }
        let Some(binding) = &node.binding else { continue };
        if !source_granted(def, &binding.source_ref) || has_search(root, &binding.source_ref) {
            continue;
        }
        let Some((container, index)) = placement(root, &node.key, false) else { continue };
        let key = format!("{}-search", node.key);
        if root.find(&key).is_some() {
            continue;
        }
        let rows = observed.rows.get(&binding.source_ref).copied();
        let (evidence, reason) = match request {
            Some(i) => (
                vec![ev(EvidenceKind::UserGoal, format!("requests.{i}")), ev(EvidenceKind::AppState, format!("composition.{}", node.key))],
                "A change asked for in the input box".to_owned(),
            ),
            None => {
                let Some(rows) = rows else { continue };
                if rows <= SEARCH_ROWS_THRESHOLD {
                    continue;
                }
                (
                    vec![
                        ev(EvidenceKind::AppState, format!("composition.{}", node.key)),
                        ev(EvidenceKind::SourceRecord, format!("sources.{}", binding.source_ref)),
                    ],
                    format!("There are {rows} rows, too many to scan by eye"),
                )
            }
        };
        let name = name_of(node);
        let input = json!({
            "key": key,
            "component": "input",
            "role": "search",
            "label": "Search",
            "props": { "placeholder": "Filter" },
            "binding": { "source_ref": binding.source_ref, "path": "filter" }
        });
        out.push(Candidate {
            category: Category::AddSearchOrFilter,
            suggestion: suggestion(
                def,
                Category::AddSearchOrFilter,
                &node.key,
                format!("Add search to “{name}”"),
                reason,
                "Add a search box above the list".into(),
                SuggestionKind::UiPatch,
                Risk::LocalReversible,
                vec![],
                true,
                evidence,
                format!("Add one search box above “{name}”. The data doesn't change"),
                insert_at(container, index, input),
            ),
        });
    }
    out
}

/// 2. 空（0 件と観測された）一覧の直後に、定義に既に在る local_write の「追加」action を呼ぶ button を足す。action は作らない。
fn empty_cta(def: &AppDefinitionV3, observed: &Observed) -> Vec<Candidate> {
    let root = &def.composition.root;
    let referenced = referenced_actions(root);
    let mut out = Vec::new();
    for node in root.nodes() {
        if node.component != "table" && node.component != "list" {
            continue;
        }
        let Some(binding) = &node.binding else { continue };
        if !source_granted(def, &binding.source_ref) || observed.rows.get(&binding.source_ref) != Some(&0) {
            continue;
        }
        let Some((container, index)) = placement(root, &node.key, true) else { continue };
        let key = format!("{}-add", node.key);
        if root.find(&key).is_some() {
            continue;
        }
        let is_adder = |a: &&ActionDefinition| {
            a.effect == Effect::LocalWrite
                && a.readiness != ActionReadiness::Unsupported
                && !referenced.contains(&a.id)
                && tokens(&a.id).iter().any(|t| ["add", "create", "new", "import"].contains(&t.as_str()))
        };
        let Some(action) = def.actions.iter().find(is_adder) else { continue };
        let name = name_of(node);
        let button = json!({ "key": key, "component": "button", "label": "Add", "action_ref": action.id });
        out.push(Candidate {
            category: Category::CompleteMissingDependency,
            suggestion: suggestion(
                def,
                Category::CompleteMissingDependency,
                &node.key,
                format!("Show “Add” on the empty “{name}”"),
                "There are no rows and nothing on the screen says what to do next".into(),
                format!("Add a button that calls “{}” below the list", action.id),
                SuggestionKind::UiPatch,
                Risk::LocalReversible,
                action.required_scopes.clone(),
                true,
                vec![
                    ev(EvidenceKind::AppState, format!("composition.{}", node.key)),
                    ev(EvidenceKind::SourceRecord, format!("sources.{}", binding.source_ref)),
                    ev(EvidenceKind::AppState, format!("actions.{}", action.id)),
                ],
                format!("Add one “Add” ({}) right after “{name}”", action.id),
                insert_at(container, index, button),
            ),
        });
    }
    out
}

/// 3. action_ref の無い button に、どの node からも参照されていない action のうち名前（key / id の token）が重なる物を繋ぐ。
/// 外部に効く action なら risk をその効果で申告する（grounding が過少申告を落とす）。
fn bind_buttons(def: &AppDefinitionV3) -> Vec<Candidate> {
    let root = &def.composition.root;
    let referenced = referenced_actions(root);
    let mut out = Vec::new();
    for node in root.nodes() {
        if node.component != "button" || node.action_ref.is_some() {
            continue;
        }
        let words = tokens(&format!("{} {}", node.key, name_of(node)));
        let best = def
            .actions
            .iter()
            .filter(|a| a.readiness != ActionReadiness::Unsupported && !referenced.contains(&a.id))
            .map(|a| (tokens(&a.id).intersection(&words).count(), a))
            .filter(|(n, _)| *n >= 1)
            .max_by(|a, b| a.0.cmp(&b.0));
        let Some((_, action)) = best else { continue };
        let name = name_of(node);
        let risk = risk_of_effect(action.effect);
        out.push(Candidate {
            category: Category::RepairBinding,
            suggestion: suggestion(
                def,
                Category::RepairBinding,
                &node.key,
                format!("Connect “{name}” to “{}”", action.id),
                "The button does nothing yet and an action with a similar name already exists".into(),
                format!("Pressing it calls {} (effect: {:?})", action.id, action.effect),
                SuggestionKind::UiPatch,
                risk,
                action.required_scopes.clone(),
                true,
                vec![
                    ev(EvidenceKind::AppState, format!("composition.{}", node.key)),
                    ev(EvidenceKind::AppState, format!("actions.{}", action.id)),
                ],
                format!("Set the action of “{name}” to {}. The action itself doesn't change", action.id),
                vec![AppPatch {
                    op: PatchOp::Bind,
                    target_key: node.key.clone(),
                    value: Some(json!({ "action_ref": action.id })),
                }],
            ),
        });
    }
    out
}

/// 4. action の error から。入力の不足 / 型違いは同じ親の form の props に必須項目を足す案、
/// 未接続 / 未準備は接続の案内（patch 無し・apply 不可）。何処を直せばよいか分からない error は出さない。
fn from_errors(def: &AppDefinitionV3, observed: &Observed) -> Vec<Candidate> {
    let root = &def.composition.root;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for e in &observed.errors {
        if !seen.insert((e.action_ref.clone(), e.error.to_string())) {
            continue;
        }
        let Some(button) = root.nodes().into_iter().find(|n| n.action_ref.as_deref() == Some(&e.action_ref)) else { continue };
        let failure = ev(EvidenceKind::TestFailure, format!("errors.{}", e.action_ref));
        match &e.error {
            InvocationError::MissingInput { field } | InvocationError::InputType { field, .. } => {
                // action を持つ node が form ならそれ自身、button なら同じ親の form
                let form = if button.component == "form" {
                    Some(button)
                } else {
                    parent_of(root, &button.key).and_then(|(parent, _)| parent.children.iter().find(|c| c.component == "form"))
                };
                let Some(form) = form else { continue };
                let mut props: Map<String, Value> = form.props.clone();
                let mut required: Vec<Value> = props.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
                if !required.iter().any(|v| v.as_str() == Some(field)) {
                    required.push(Value::String(field.clone()));
                }
                props.insert("required".into(), Value::Array(required));
                if let InvocationError::InputType { expected, .. } = &e.error {
                    let mut types = props.get("types").and_then(Value::as_object).cloned().unwrap_or_default();
                    types.insert(field.clone(), Value::String(expected.clone()));
                    props.insert("types".into(), Value::Object(types));
                }
                if props == form.props {
                    continue; // 既に必須（・型つき）: 何も変えない提案は出さない
                }
                let name = name_of(form);
                out.push(Candidate {
                    category: Category::FixInput,
                    suggestion: suggestion(
                        def,
                        Category::FixInput,
                        &form.key,
                        format!("Make “{field}” required in “{name}”"),
                        format!("“{}” was stopped: {}", e.action_ref, e.error),
                        format!("Add {field} to the form's required fields and stop before sending"),
                        SuggestionKind::DataFix,
                        Risk::LocalReversible,
                        vec![],
                        true,
                        vec![failure, ev(EvidenceKind::AppState, format!("composition.{}", form.key))],
                        format!("Add {field} to the required fields of “{name}”. The data doesn't change"),
                        vec![AppPatch {
                            op: PatchOp::Update,
                            target_key: form.key.clone(),
                            value: Some(json!({ "props": Value::Object(props) })),
                        }],
                    ),
                });
            }
            InvocationError::NotReady { .. }
            | InvocationError::CapabilityNotReady { .. }
            | InvocationError::CapabilityMissing { .. } => {
                let scopes = def.action(&e.action_ref).map(|a| a.required_scopes.clone()).unwrap_or_default();
                let name = name_of(button);
                out.push(Candidate {
                    category: Category::ConnectCandidateSource,
                    suggestion: suggestion(
                        def,
                        Category::ConnectCandidateSource,
                        &button.key,
                        format!("“{name}” works once you connect it"),
                        format!("{}", e.error),
                        "Turns on when you connect in Connections. Nothing runs here".into(),
                        SuggestionKind::ConnectionRequest,
                        Risk::None,
                        scopes,
                        false,
                        vec![failure, ev(EvidenceKind::AppState, format!("composition.{}", button.key))],
                        "Doesn't change the setup (only guidance on connecting)".into(),
                        vec![],
                    ),
                });
            }
            InvocationError::UnknownAction(_) | InvocationError::InputNotObject => {}
        }
    }
    out
}
