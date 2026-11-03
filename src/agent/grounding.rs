//! 根拠の検査（AGT-005）: evidence の ref が実在する物を指す・reason / effect / summary が空でない・
//! domain の validate_suggestion（app / version / target / scope）を通る・触る node / action が分かる・
//! risk は operations から導いた実効 risk 以上を申告している・新しく繋ぐ action の scope を申告している・undoable の嘘が無い・
//! 手で固定した（user_locked）部品を変えない（CD-29）。
//! 落ちた提案は queue にも apply にも進めない。信頼度は evidence の `uncertain` から計算する（schema に field が無いので別 struct）。
use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

use super::domain::action::Effect;
use super::domain::app::AppDefinitionV3;
use super::domain::binding::SourceReadiness;
use super::domain::patch::PatchOp;
use super::domain::suggestion::{Evidence, EvidenceKind, Risk, Suggestion, SuggestionKind};
use super::domain::ui::UiNode;
use super::domain::validate::{validate_suggestion, ValidationError};

/// 検査を通った提案に付く「分かった事」。
#[derive(Debug, Clone, PartialEq)]
pub struct Grounding {
    /// evidence のうち uncertain でない物の割合（0..1）。model の自信ではない。
    pub confidence: f32,
    pub uncertain: bool,
    /// target_ref と各 op が触る node（insert の部分木・move の先を含む）。
    pub touched_nodes: Vec<String>,
    /// 触る action: insert の部分木・bind / update の value で新しく UI に繋ぐ物と、update / remove する node が今持つ物
    /// （プレビューの「影響」・承認の digest・「承認が必要」「要接続」の印に使う）。
    pub touched_actions: Vec<String>,
    /// 申告 risk と、新しく繋ぐ action の効果から導いた risk の大きい方。
    pub effective_risk: Risk,
    /// 申告した scope と、新しく繋ぐ action の required_scopes の和（申告の順 → 足りなかった物）。
    /// policy の判定・rail / プレビューの「必要権限」・承認の記録はこれを読む（申告だけを信じない）。
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundingError {
    NoEvidence,
    UnresolvedEvidence(String),
    EmptyField(&'static str),
    Invalid(ValidationError),
    RiskUnderstated { declared: Risk, effective: Risk },
    /// 新しく繋ぐ action が要する scope を permission_scopes に申告していない。
    ScopeUnderstated { missing: Vec<String> },
    /// patch の無い提案（connection_request 等）に undoable: true。
    FalseUndo,
    /// 手で固定した部品（`key`）か、それを含む部分木を変える・運ぶ・消す・その中へ足す（CD-29）。
    Locked { key: String },
    /// update / insert の value が `user_locked` を書く（固定の付け外しは利用者の直接編集だけ・CD-29 (2)）。
    LockWrite { key: String },
}

impl fmt::Display for GroundingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEvidence => write!(f, "There's no evidence"),
            Self::UnresolvedEvidence(r) => write!(f, "Evidence '{r}' points at something that doesn't exist"),
            Self::EmptyField(name) => write!(f, "'{name}' is empty"),
            Self::Invalid(e) => write!(f, "{e}"),
            Self::RiskUnderstated { declared, effective } => {
                write!(f, "The declared risk {declared:?} is lower than the effective risk {effective:?}")
            }
            Self::ScopeUnderstated { missing } => {
                write!(f, "The permission {} that the connecting action needs wasn't declared", missing.join(", "))
            }
            Self::FalseUndo => write!(f, "It says undoable but has no patch"),
            Self::Locked { key } => write!(f, "“{key}” was pinned by hand, so it isn't changed"),
            Self::LockWrite { key } => write!(f, "Pinning and unpinning “{key}” is done by hand, so suggestions don't change it"),
        }
    }
}
impl std::error::Error for GroundingError {}

pub fn risk_rank(r: Risk) -> u8 {
    match r {
        Risk::None => 0,
        Risk::LocalReversible => 1,
        Risk::ExternalRead => 2,
        Risk::ExternalWrite => 3,
        Risk::ExternalSend => 4,
        Risk::Destructive => 5,
    }
}

pub fn risk_of_effect(e: Effect) -> Risk {
    match e {
        Effect::LocalRead | Effect::LocalWrite => Risk::LocalReversible,
        Effect::ExternalRead => Risk::ExternalRead,
        Effect::ExternalWrite => Risk::ExternalWrite,
        Effect::ExternalSend => Risk::ExternalSend,
        Effect::Destructive => Risk::Destructive,
    }
}

pub fn is_external(r: Risk) -> bool {
    risk_rank(r) >= risk_rank(Risk::ExternalRead)
}

pub const fn risk_wording(r: Risk) -> &'static str {
    match r {
        Risk::None => "None",
        Risk::LocalReversible => "Local · can be undone",
        Risk::ExternalRead => "Reads from outside",
        Risk::ExternalWrite => "Writes to outside",
        Risk::ExternalSend => "Sends to outside",
        Risk::Destructive => "Destructive (can't be undone)",
    }
}

/// `known` は定義の外に在る ref（`errors.<action>` / `requests.<n>` / `events.<id>`）。
pub fn check(def: &AppDefinitionV3, s: &Suggestion, known: &BTreeSet<String>) -> Result<Grounding, Vec<GroundingError>> {
    let mut errs = Vec::new();
    if s.evidence.is_empty() {
        errs.push(GroundingError::NoEvidence);
    }
    for e in &s.evidence {
        if !resolve_ref(def, known, e) {
            errs.push(GroundingError::UnresolvedEvidence(e.r#ref.clone()));
        }
    }
    for (name, value) in [("title", &s.title), ("reason", &s.reason), ("effect", &s.effect), ("change.summary", &s.change.summary)] {
        if value.trim().is_empty() {
            errs.push(GroundingError::EmptyField(name));
        }
    }
    if let Err(v) = validate_suggestion(def, s) {
        errs.extend(v.into_iter().map(GroundingError::Invalid));
    }
    errs.extend(lock_errors(def, s));
    let Touched { nodes: touched_nodes, actions: touched_actions, wired, sources } = touched(def, s);
    // 新しく繋ぐ binding の取得元のうち外部の物（local でない）: 描いた時に読み取りが走るので external_read とその scope（SEC-A-R3-2）
    let outside: Vec<&super::domain::binding::Source> =
        sources.iter().filter_map(|id| def.source(id)).filter(|src| src.readiness != SourceReadiness::Local).collect();
    // 実効 risk は新しく UI から呼べるようになる action の効果だけから導く。update / remove する node が今持つ action は
    // 影響として見せるが risk には入れない: 名前を変える・必須項目を足す・外すは UI の変更で、その action は押された時に
    // action 側の門（承認・接続・権限）を通る（DESIGN §5.4 の distinct permission levels）
    let derived = wired
        .iter()
        .filter_map(|id| def.action(id))
        .map(|a| risk_of_effect(a.effect))
        .chain(outside.iter().map(|_| Risk::ExternalRead))
        .max_by_key(|r| risk_rank(*r))
        .unwrap_or(Risk::None);
    if risk_rank(derived) > risk_rank(s.risk) {
        errs.push(GroundingError::RiskUnderstated { declared: s.risk, effective: derived });
    }
    // scope は「新しく UI から呼べるようになる action」の分だけ要る（外す・名前を変えるだけの node の action は要らない）
    let mut missing: Vec<String> = Vec::new();
    let source_scopes = outside.iter().filter_map(|src| src.scope.as_ref());
    for scope in wired.iter().filter_map(|id| def.action(id)).flat_map(|a| a.required_scopes.iter()).chain(source_scopes) {
        if !s.permission_scopes.contains(scope) && !missing.contains(scope) {
            missing.push(scope.clone());
        }
    }
    if !missing.is_empty() {
        errs.push(GroundingError::ScopeUnderstated { missing: missing.clone() });
    }
    if s.undoable && (s.kind == SuggestionKind::ConnectionRequest || s.change.operations.is_empty()) {
        errs.push(GroundingError::FalseUndo);
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let n = s.evidence.len() as f32;
    let uncertain_n = s.evidence.iter().filter(|e| e.uncertain == Some(true)).count() as f32;
    let mut scopes = s.permission_scopes.clone();
    scopes.extend(missing);
    Ok(Grounding {
        confidence: if n == 0.0 { 0.0 } else { (n - uncertain_n) / n },
        uncertain: uncertain_n > 0.0,
        touched_nodes,
        touched_actions,
        effective_risk: if risk_rank(derived) > risk_rank(s.risk) { derived } else { s.risk },
        scopes,
    })
}

/// evidence の ref が指す物が在るか。source_record は許可された source（Local / Ready）だけ解決する（AGT-003）。
pub fn resolve_ref(def: &AppDefinitionV3, known: &BTreeSet<String>, e: &Evidence) -> bool {
    let r = e.r#ref.as_str();
    match e.kind {
        EvidenceKind::UserGoal => {
            r == "goal.literal_goal"
                || indexed(r, "goal.outcomes", def.goal.outcomes.len())
                || indexed(r, "goal.assumptions", def.goal.assumptions.len())
                || indexed(r, "goal.constraints", def.goal.constraints.len())
                || known.contains(r)
        }
        EvidenceKind::AppState => {
            if let Some(key) = r.strip_prefix("composition.") {
                def.composition.root.find(key).is_some()
            } else if let Some(id) = r.strip_prefix("actions.") {
                def.action(id).is_some()
            } else if let Some(id) = r.strip_prefix("sources.") {
                def.source(id).is_some()
            } else if let Some(id) = r.strip_prefix("capabilities.") {
                def.capability(id).is_some()
            } else if let Some(id) = r.strip_prefix("workflows.") {
                def.workflow(id).is_some()
            } else {
                false
            }
        }
        EvidenceKind::SourceRecord => r
            .strip_prefix("sources.")
            .and_then(|id| def.source(id))
            .is_some_and(|s| matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready)),
        EvidenceKind::Event | EvidenceKind::TestFailure => known.contains(r),
    }
}

fn indexed(r: &str, prefix: &str, len: usize) -> bool {
    r.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('['))
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|i| i.parse::<usize>().ok())
        .is_some_and(|i| i < len)
}

/// CD-29: 手で固定した（user_locked）部品を提案で変えない。op の対象かその祖先が固定 / remove・move の対象の子孫が固定 /
/// move の行き先かその祖先が固定（insert の行き先は対象そのもの）→ `Locked`。update / insert の value が `user_locked` を
/// 書く（true でも false でも）→ `LockWrite`。suggestion の target_ref だけを指す op の無い提案（接続の案内）は何も変えないので見ない。
// ponytail: agent の門だけ（CD-29 (4) の inflight）。composer の再生成など別の writer は素通りする。
// W4-0 で domain の patch::apply（PatchError::Locked）と validate_suggestion（LockedTarget）に寄せたら、ここは domain を呼ぶ形にする
pub fn lock_errors(def: &AppDefinitionV3, s: &Suggestion) -> Vec<GroundingError> {
    let root = &def.composition.root;
    let locks: Vec<&UiNode> = root.nodes().into_iter().filter(|n| n.user_locked).collect();
    // key の node を部分木に持つ固定（自分自身か祖先）
    let covering = |key: &str| locks.iter().find(|l| l.find(key).is_some()).map(|l| l.key.clone());
    let mut errs = Vec::new();
    for op in &s.change.operations {
        let mut hit = covering(&op.target_key);
        if matches!(op.op, PatchOp::Remove | PatchOp::Move) {
            let inner = root.find(&op.target_key).and_then(|t| t.nodes().into_iter().find(|n| n.user_locked));
            hit = hit.or_else(|| inner.map(|n| n.key.clone()));
        }
        if op.op == PatchOp::Move {
            hit = hit.or_else(|| op.value.as_ref().and_then(|v| v.get("parent")).and_then(Value::as_str).and_then(covering));
        }
        if let Some(key) = hit {
            errs.push(GroundingError::Locked { key });
        }
        if matches!(op.op, PatchOp::Insert | PatchOp::Update) {
            if let Some(key) = op.value.as_ref().and_then(|v| writes_lock(v, &op.target_key)) {
                errs.push(GroundingError::LockWrite { key });
            }
        }
    }
    errs
}

/// value（update の部分 object か insert の部分木）の中で `user_locked` を書く node の key（無ければ op の対象）。
fn writes_lock(v: &Value, fallback: &str) -> Option<String> {
    if v.get("user_locked").is_some() {
        return Some(v.get("key").and_then(Value::as_str).unwrap_or(fallback).to_owned());
    }
    v.get("children")?.as_array()?.iter().find_map(|c| writes_lock(c, fallback))
}

/// 提案が触る物。`wired` は `actions` のうち、適用すると新しく UI から呼べるようになる action
/// （insert の部分木の action_ref・bind / update の value の action_ref）。domain の patch でこの 3 つ以外に action_ref を書く口は無い。
/// `sources` は同じ 3 つの口で新しく node に繋ぐ binding の取得元（`binding.source_ref`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Touched {
    pub nodes: Vec<String>,
    pub actions: Vec<String>,
    pub wired: Vec<String>,
    pub sources: Vec<String>,
}

/// 触る node と action（preview の「影響」と policy の実効 risk / scope に使う）。
pub fn touched(def: &AppDefinitionV3, s: &Suggestion) -> Touched {
    let mut t = Touched { nodes: vec![s.target_ref.clone()], ..Touched::default() };
    let push = |v: &mut Vec<String>, x: &str| {
        if !v.iter().any(|y| y == x) {
            v.push(x.to_owned());
        }
    };
    for op in &s.change.operations {
        push(&mut t.nodes, &op.target_key);
        match op.op {
            PatchOp::Insert => {
                if let Some(node) = op.value.as_ref().and_then(|v| serde_json::from_value::<UiNode>(v.clone()).ok()) {
                    for n in node.nodes() {
                        push(&mut t.nodes, &n.key);
                        if let Some(a) = &n.action_ref {
                            push(&mut t.actions, a);
                            push(&mut t.wired, a);
                        }
                        if let Some(b) = &n.binding {
                            push(&mut t.sources, &b.source_ref);
                        }
                    }
                }
            }
            PatchOp::Move => {
                if let Some(p) = op.value.as_ref().and_then(|v| v.get("parent")).and_then(Value::as_str) {
                    push(&mut t.nodes, p);
                }
            }
            // update / remove は今の node の action にも効く（外す・名前を変える）。影響として数えるだけで、繋ぎはしない
            PatchOp::Remove | PatchOp::Update => {
                if let Some(a) = def.composition.root.find(&op.target_key).and_then(|n| n.action_ref.as_deref()) {
                    push(&mut t.actions, a);
                }
            }
            PatchOp::Bind => {}
        }
        // bind と update の value は node の action_ref を書き換えられる（domain::patch::apply の update は key / children 以外の
        // field を全部上書きする）。どちらも新しく繋ぐ action として数える（update で繋ぎ替えると risk の検査を素通りしていた）
        if matches!(op.op, PatchOp::Bind | PatchOp::Update) {
            if let Some(a) = op.value.as_ref().and_then(|v| v.get("action_ref")).and_then(Value::as_str) {
                push(&mut t.actions, a);
                push(&mut t.wired, a);
            }
            if let Some(src) = op.value.as_ref().and_then(|v| v.get("binding")).and_then(|b| b.get("source_ref")).and_then(Value::as_str) {
                push(&mut t.sources, src);
            }
        }
    }
    t
}
