//! プレビュー（AGT-010）と適用の transaction（AGT-011）。
//! preview = 元の revision を変えずに patch を当てた結果の before / after（node の outline）と影響・risk・権限・取り消し。
//! transaction = revision の照合 → approved → domain::patch::apply（原子的）→ 追加の検査（renderer 等の hook）→ applied → verified。
//! 途中で落ちたら status は failed になり、返す revision は元のまま（呼び側の保存は新しい revision が返った時だけ）。
//! 取り消しは適用前の revision の snapshot を version+1 で戻す（inverse patch ではない）。
use std::collections::BTreeSet;
use std::fmt;

use super::domain::app::AppDefinitionV3;
use super::domain::patch::{apply as patch_apply, AppRevision, PatchError, PatchOp};
use super::domain::run::TransitionError;
use super::domain::suggestion::{Risk, SuggestionKind, SuggestionStatus};
use super::domain::ui::UiNode;
use super::grounding::GroundingError;
use super::lifecycle;
use super::policy::{fnv1a64, Decision};
use super::queue::Tracked;

/// §5.4 の 1 つの diff 面に出す物。
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// 影響する部分木の outline（1 行 1 node）。
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub affected_nodes: Vec<String>,
    pub affected_actions: Vec<String>,
    pub affected_workflows: Vec<String>,
    pub risk: Risk,
    pub scopes: Vec<String>,
    pub undoable: bool,
    /// 根拠に uncertain が混じる時だけ Some（「信頼度 N%」）。
    pub confidence: Option<String>,
    pub rollback: String,
    pub version_from: u32,
    pub version_to: u32,
    pub cta: PreviewCta,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewCta {
    Apply,
    /// 外部に効く変更: 押すとこのプレビューで見せた変更（digest）への承認として記録して適用する（§5.3）。
    Approve,
    /// 適用できない理由（要接続 / 承認が必要 / 停止中）。button は disabled でこの文。
    Blocked(String),
}

impl PreviewCta {
    pub const fn label(&self) -> &'static str {
        match self {
            PreviewCta::Approve => "Approve and apply",
            PreviewCta::Apply | PreviewCta::Blocked(_) => "Apply",
        }
    }
}

/// 適用の結果。呼び側はこれを保存する（`Store::put_revision(app_id, json, Some(base_hash))`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub suggestion_id: String,
    pub title: String,
    pub revision: AppRevision,
    pub base_version: u32,
    /// 元にした revision の定義（JSON）の hash。保存側の照合用。
    pub base_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyError {
    /// 提案の app_version が今の revision と違う（§11「構成が更新されています」）。
    Stale { expected: u32, found: u32 },
    NotGrounded(Vec<GroundingError>),
    Policy(Decision),
    /// patch の無い提案（接続の案内など）。
    NotApplicable(String),
    Patch(PatchError),
    /// 適用後の追加検査（renderer 等）が落ちた。
    Check(String),
    Transition(TransitionError<SuggestionStatus>),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale { expected, found } => {
                write!(f, "The setup was updated (this suggestion is for v{found}, now v{expected}). Rebuilding the suggestion")
            }
            Self::NotGrounded(errs) => {
                write!(f, "Can't apply it because the evidence is missing:")?;
                for e in errs {
                    write!(f, " [{e}]")?;
                }
                Ok(())
            }
            Self::Policy(d) => write!(f, "{d}"),
            Self::NotApplicable(s) => write!(f, "{s}"),
            Self::Patch(e) => write!(f, "Couldn't apply it (the setup is unchanged): {e}"),
            Self::Check(s) => write!(f, "The check after applying failed (the setup is unchanged): {s}"),
            Self::Transition(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ApplyError {}

impl From<TransitionError<SuggestionStatus>> for ApplyError {
    fn from(e: TransitionError<SuggestionStatus>) -> Self {
        ApplyError::Transition(e)
    }
}

pub fn base_hash(rev: &AppRevision) -> String {
    format!("{:016x}", fnv1a64(serde_json::to_string(rev.definition()).unwrap_or_default().as_bytes()))
}

/// 元の revision は触らない。patch が当たらない提案は Err（その理由がプレビューの代わり）。
/// 構成を変えない提案（接続の案内）は before = after で、適用の代わりに理由を出す（押せる faux button を出さない）。
pub fn preview(base: &AppRevision, t: &Tracked, cta: PreviewCta) -> Result<Preview, ApplyError> {
    let s = &t.suggestion;
    let no_change = s.kind == SuggestionKind::ConnectionRequest || s.change.operations.is_empty();
    let (next, cta) = if no_change {
        (base.clone(), PreviewCta::Blocked("Doesn't change the setup. Connect in Connections to make it usable".into()))
    } else {
        (patch_apply(base, &s.change.operations).map_err(ApplyError::Patch)?, cta)
    };
    let def = base.definition();
    let anchors = anchors(def, t);
    let affected_actions = t.grounding.touched_actions.clone();
    let affected_workflows: Vec<String> = def
        .workflows
        .iter()
        .filter(|w| w.action_refs.iter().any(|a| affected_actions.contains(a)))
        .map(|w| w.id.clone())
        .collect();
    Ok(Preview {
        id: s.id.clone(),
        title: s.title.clone(),
        summary: s.change.summary.clone(),
        before: outline(&def.composition.root, &anchors),
        after: outline(&next.definition().composition.root, &anchors),
        affected_nodes: t.grounding.touched_nodes.clone(),
        affected_actions,
        affected_workflows,
        risk: t.grounding.effective_risk,
        scopes: t.grounding.scopes.clone(),
        undoable: s.undoable,
        confidence: confidence_wording(t),
        rollback: rollback_wording(s.undoable, base.version()),
        version_from: base.version(),
        version_to: next.version(),
        cta,
    })
}

pub fn confidence_wording(t: &Tracked) -> Option<String> {
    t.grounding.uncertain.then(|| format!("{:.0}% (some of the evidence is unconfirmed)", t.grounding.confidence * 100.0))
}

pub fn rollback_wording(undoable: bool, base_version: u32) -> String {
    if undoable {
        format!("After applying, Undo returns the setup to v{base_version}")
    } else {
        "Can't be undone. Check the contents before you apply".into()
    }
}

/// transaction。`checks` は patch 後の定義への追加検査（renderer の描画可否など。無ければ `|_| Ok(())`）。
/// 戻り値が Ok の時だけ構成が進む。Err の時 `base` はそのまま（domain の apply が clone に当てるため）。
pub fn transaction(
    base: &AppRevision,
    t: &mut Tracked,
    checks: &dyn Fn(&AppDefinitionV3) -> Result<(), String>,
) -> Result<Applied, ApplyError> {
    let s = &t.suggestion;
    if s.app_id != base.definition().id || s.app_version != base.version() {
        return Err(ApplyError::Stale { expected: base.version(), found: s.app_version });
    }
    if s.kind == SuggestionKind::ConnectionRequest || s.change.operations.is_empty() {
        return Err(ApplyError::NotApplicable("This suggestion doesn't change the setup (it is guidance on connecting)".into()));
    }
    if !matches!(s.status, SuggestionStatus::Approved) {
        lifecycle::approve(t)?;
    }
    let ops = t.suggestion.change.operations.clone();
    let next = match patch_apply(base, &ops) {
        Ok(next) => next,
        Err(e) => {
            let _ = lifecycle::fail(t, &e.to_string());
            return Err(ApplyError::Patch(e));
        }
    };
    lifecycle::apply(t)?;
    // 再検査: domain の validate は patch_apply が済ませた。ここでは対象がまだ在るか・追加の検査。
    let post = post_checks(next.definition(), t).and_then(|()| checks(next.definition()));
    if let Err(reason) = post {
        let _ = lifecycle::fail(t, &reason);
        return Err(ApplyError::Check(reason));
    }
    lifecycle::verify(t)?;
    Ok(Applied {
        suggestion_id: t.suggestion.id.clone(),
        title: t.suggestion.title.clone(),
        revision: next,
        base_version: base.version(),
        base_hash: base_hash(base),
    })
}

fn post_checks(def: &AppDefinitionV3, t: &Tracked) -> Result<(), String> {
    for op in &t.suggestion.change.operations {
        let key = match op.op {
            PatchOp::Remove => continue,
            PatchOp::Insert => op.value.as_ref().and_then(|v| v.get("key")).and_then(|k| k.as_str()).unwrap_or(&op.target_key),
            _ => op.target_key.as_str(),
        };
        if def.composition.root.find(key).is_none() {
            return Err(format!("Node '{key}' wasn't found after applying"));
        }
    }
    if def.version != t.suggestion.app_version + 1 {
        return Err(format!("The version is {}, not {}", t.suggestion.app_version + 1, def.version));
    }
    Ok(())
}

/// before / after に出す部分木の根: insert は target、他は target の親（無ければ target）、move は移動先も。
fn anchors(def: &AppDefinitionV3, t: &Tracked) -> BTreeSet<String> {
    let root = &def.composition.root;
    let parent_of = |key: &str| -> Option<String> {
        root.nodes().into_iter().find(|n| n.children.iter().any(|c| c.key == key)).map(|n| n.key.clone())
    };
    let mut set = BTreeSet::new();
    for op in &t.suggestion.change.operations {
        match op.op {
            PatchOp::Insert => {
                set.insert(op.target_key.clone());
            }
            PatchOp::Move => {
                set.insert(parent_of(&op.target_key).unwrap_or_else(|| op.target_key.clone()));
                if let Some(p) = op.value.as_ref().and_then(|v| v.get("parent")).and_then(|p| p.as_str()) {
                    set.insert(p.to_owned());
                }
            }
            _ => {
                set.insert(parent_of(&op.target_key).unwrap_or_else(|| op.target_key.clone()));
            }
        }
    }
    if set.is_empty() {
        // 構成を変えない提案でも、何についての提案かは見せる
        set.insert(t.suggestion.target_ref.clone());
    }
    // 子孫の anchor は親に含まれるので落とす（同じ node を 2 回出さない）
    let keys: Vec<String> = set.iter().cloned().collect();
    for k in &keys {
        if let Some(n) = root.find(k) {
            for d in n.nodes().iter().skip(1) {
                set.remove(&d.key);
            }
        }
    }
    set
}

/// 1 行 1 node: `key (component) ⇐ source.path → action`。
pub fn outline(root: &UiNode, anchors: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for key in anchors {
        match root.find(key) {
            Some(n) => walk(n, 0, &mut out),
            None => out.push(format!("({key} is missing)")),
        }
    }
    out
}

fn walk(n: &UiNode, depth: usize, out: &mut Vec<String>) {
    let mut line = format!("{}{} ({})", "  ".repeat(depth), n.key, n.component);
    if let Some(l) = &n.label {
        line.push_str(&format!(" “{l}”"));
    }
    if let Some(b) = &n.binding {
        line.push_str(&format!(" ⇐ {}.{}", b.source_ref, b.path));
    }
    if let Some(a) = &n.action_ref {
        line.push_str(&format!(" → {a}"));
    }
    out.push(line);
    for c in &n.children {
        walk(c, depth + 1, out);
    }
}
