//! autonomy の policy（AGT-008）: suggest_only（既定・最小権限・A0）/ auto_local_reversible（A1）/ ask_before_external_effects（A2）。
//! 判定は autonomy と独立に実効 risk と scope の grant と承認（ConsentReceipt）で縛る:
//! 外部に効く変更（external_write / external_send / destructive）は提案だけでは絶対に進まず、
//! 見せた payload の digest 付きの承認と押下が要る。auto で当たるのは取り消せる UI patch だけで、
//! A1 はローカル（scope 不要）の物、A2 はそれに加えて grant 済み scope の読み取り（external_read）まで。
//! どの段でも、rail が「承認が必要」「要接続」と言う物（`needs_approval` / `needs_connection`）は自動で当てない。
use std::fmt;

use super::domain::action::{ActionDefinition, ActionReadiness};
use super::domain::app::{AppDefinitionV3, AutonomyLevel};
use super::domain::capability::CapabilityReadiness;
use super::domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject, PermissionGrant};
use super::domain::suggestion::{Risk, Suggestion, SuggestionKind};
use super::grounding::{risk_of_effect, risk_rank, Grounding};

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub autonomy: AutonomyLevel,
    /// app の agent.suggestions_enabled。
    pub enabled: bool,
    /// ユーザーの「Agent を一時停止」。
    pub paused: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { autonomy: AutonomyLevel::SuggestOnly, enabled: true, paused: false }
    }
}

/// schema の 3 値 → AGENT_NATIVE_RUNTIME §3.2 の段（CD-03 E）。A3 / A4 は MVP に無い。
pub const fn a_level(level: AutonomyLevel) -> &'static str {
    match level {
        AutonomyLevel::SuggestOnly => "A0",
        AutonomyLevel::AutoLocalReversible => "A1",
        AutonomyLevel::AskBeforeExternalEffects => "A2",
    }
}

pub const fn autonomy_wording(level: AutonomyLevel) -> &'static str {
    match level {
        AutonomyLevel::SuggestOnly => "A0 Suggestions only (you press Apply every time)",
        AutonomyLevel::AutoLocalReversible => "A1 Applies only local UI changes that can be undone",
        AutonomyLevel::AskBeforeExternalEffects => "A2 Local changes and allowed reads are automatic · always asks before writing or sending outside",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// 提案だけで当ててよい（auto_local_reversible のローカル UI patch）。
    AutoApply,
    /// ユーザーが押したので進める。
    Proceed,
    /// 押されていない（auto では当てない）。
    NeedsUserApproval,
    /// この scope の有効な grant が無い。
    NeedsScope(String),
    /// 外部に効く変更: 見せた内容への承認（digest 一致）が要る。
    NeedsConsent,
    Blocked(String),
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AutoApply => write!(f, "Can be applied automatically"),
            Self::Proceed => write!(f, "Can be applied"),
            Self::NeedsUserApproval => write!(f, "Won't be applied until you press Apply"),
            Self::NeedsScope(s) => write!(f, "Permission “{s}” is needed"),
            Self::NeedsConsent => write!(f, "This changes something outside. Check the contents and approve first"),
            Self::Blocked(r) => write!(f, "{r}"),
        }
    }
}

/// FNV-1a 64（std だけ）。payload の digest と revision の hash に使う。
// ponytail: 暗号学的な hash ではない。承認は suggestion id と組で照合し、queue の中の同じ id の中身は差し替わらない
// （queue::intake）ので process 内の照合には足りる。外から来た payload を承認と照合する時は sha256 に上げる
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// 承認の対象 = 見せた変更の正規化 JSON の digest: app・元の版・operations・実効 risk・権限・触る action。
/// 版が進んで作り直した同じ id・同じ operations の提案や、実効 risk / 権限が変わった提案には前の承認が当たらない
/// （§5.3「見せた exact な変更への承認」。operations だけだと別の版・別の効果に古い承認が流用されていた）。
pub fn payload_digest(s: &Suggestion, g: &Grounding) -> String {
    let shown = serde_json::json!({
        "app_id": s.app_id,
        "app_version": s.app_version,
        "operations": s.change.operations,
        "risk": g.effective_risk,
        "scopes": g.scopes,
        "actions": g.touched_actions,
    });
    format!("{:016x}", fnv1a64(shown.to_string().as_bytes()))
}

/// プレビューで見せた変更への承認の記録（Agent が explicit な適用の時にだけ作る。decision は常に Approved）。
pub fn consent_for(s: &Suggestion, g: &Grounding, now: &str) -> ConsentReceipt {
    let digest = payload_digest(s, g);
    ConsentReceipt {
        id: format!("consent.{}.{digest}", s.id),
        app_id: s.app_id.clone(),
        subject: ConsentSubject::Suggestion { id: s.id.clone() },
        scopes: g.scopes.clone(),
        payload_digest: Some(digest),
        decision: ConsentDecision::Approved,
        decided_at: now.to_owned(),
    }
}

pub fn has_consent(consents: &[ConsentReceipt], s: &Suggestion, g: &Grounding) -> bool {
    let digest = payload_digest(s, g);
    consents.iter().any(|c| {
        c.app_id == s.app_id
            && c.decision == ConsentDecision::Approved
            && matches!(&c.subject, ConsentSubject::Suggestion { id } if *id == s.id)
            && c.payload_digest.as_deref() == Some(digest.as_str())
    })
}

pub fn scope_granted(grants: &[PermissionGrant], app_id: &str, scope: &str, now: &str) -> bool {
    grants.iter().any(|g| g.app_id == app_id && g.scope == scope && g.active_at(now))
}

fn touched<'a>(def: &'a AppDefinitionV3, g: &'a Grounding) -> impl Iterator<Item = &'a ActionDefinition> + 'a {
    g.touched_actions.iter().filter_map(move |id| def.action(id))
}

/// 触る action のどれかが承認を要するか、外部に書く・送る・壊す（rail の「承認が必要」の一因・自動では当てない）。
/// 名前を変えるだけの提案でも、外部に書く action に繋がった node なら見える文言を根拠にした押下がすり替わるので、
/// [プレビュー] で見せてから当てる（SEC-A-R3-1。risk は local のまま: 新しく繋ぐ物ではない）。
pub fn needs_approval(def: &AppDefinitionV3, g: &Grounding) -> bool {
    touched(def, g).any(|a| a.requires_approval || risk_rank(risk_of_effect(a.effect)) >= risk_rank(Risk::ExternalWrite))
}

/// 接続の案内か、触る action のどれかが未接続（action が requires_connection か、capability が ready でない）。
/// rail の「要接続」と自動の判定が同じ物を見る。
pub fn needs_connection(def: &AppDefinitionV3, s: &Suggestion, g: &Grounding) -> bool {
    s.kind == SuggestionKind::ConnectionRequest
        || touched(def, g).any(|a| {
            a.readiness == ActionReadiness::RequiresConnection
                || a.capability_ref.as_deref().and_then(|c| def.capability(c)).is_some_and(|c| c.readiness != CapabilityReadiness::Ready)
        })
}

/// `explicit` = ユーザーがこの提案の「適用」を押した。`def` は今開いている app の定義（触る action の承認・接続を見る）。
#[allow(clippy::too_many_arguments)]
pub fn decide(
    policy: &Policy,
    def: &AppDefinitionV3,
    s: &Suggestion,
    g: &Grounding,
    grants: &[PermissionGrant],
    consents: &[ConsentReceipt],
    now: &str,
    explicit: bool,
) -> Decision {
    if !policy.enabled {
        return Decision::Blocked("This app has suggestions turned off".into());
    }
    if policy.paused {
        return Decision::Blocked("The agent is paused".into());
    }
    // 申告だけでなく、繋ぐ action が要する scope も（grounding が導いた和）
    if let Some(scope) = g.scopes.iter().find(|sc| !scope_granted(grants, &s.app_id, sc, now)) {
        return Decision::NeedsScope(scope.clone());
    }
    let risk = g.effective_risk;
    // 外部に書く・送る・壊す: autonomy に関係なく、見せた内容への承認（digest 一致）と押下の両方が要る。提案だけでは決して進まない
    if risk_rank(risk) >= risk_rank(Risk::ExternalWrite) {
        if !has_consent(consents, s, g) {
            return Decision::NeedsConsent;
        }
        return if explicit { Decision::Proceed } else { Decision::NeedsUserApproval };
    }
    if explicit {
        return Decision::Proceed;
    }
    // 提案だけで当ててよいのは取り消せる UI patch のうち、段が許す物（A1 ⊂ A2。上の段は下の段を含む）。
    // rail が「承認が必要」「要接続」と言っている物は段に関係なく押されるまで待つ（画面の言う事と動きを食い違わせない）
    let reversible_ui = s.kind == SuggestionKind::UiPatch && !s.change.operations.is_empty() && s.undoable;
    let marked = needs_approval(def, g) || needs_connection(def, s, g);
    let auto = match policy.autonomy {
        AutonomyLevel::SuggestOnly => false,
        AutonomyLevel::AutoLocalReversible => risk_rank(risk) <= risk_rank(Risk::LocalReversible) && g.scopes.is_empty(),
        // 許可済み scope の読み取りまで（scope は上で grant 済みと確かめた）
        AutonomyLevel::AskBeforeExternalEffects => risk_rank(risk) <= risk_rank(Risk::ExternalRead),
    };
    if reversible_ui && auto && !marked {
        Decision::AutoApply
    } else {
        Decision::NeedsUserApproval
    }
}
