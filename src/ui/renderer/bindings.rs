//! 定義と runtime の状態 → §11 の状態語（UI-007）と、button を押せるか（CD-02 の matrix）。表はここだけ（inspector も同じ物を出す）。
//! 状態語は `states::RegionState` の wording だけを使う。補足の文は node.states（empty / requires_connection / disabled ＝定義の作者の文）を先に、
//! 無い時だけここの既定の 1 文（CD-02 の「画面の語」）。

use super::ActionHost;
use crate::domain::action::{check_input, ActionReadiness, InvocationError};
use crate::domain::app::{AppDefinitionV3, AppReadiness, ExecutionMode};
use crate::domain::binding::BindingReadiness;
use crate::domain::capability::CapabilityReadiness;
use crate::domain::run::RunStatus;
use crate::domain::ui::UiNode;
use crate::ui::states::RegionState;
use serde_json::Value;

/// app 全体の readiness → 見出しの下の 1 行。
pub fn app_state(r: AppReadiness) -> RegionState {
    match r {
        AppReadiness::Preview | AppReadiness::Partial => RegionState::Partial,
        AppReadiness::Ready => RegionState::Ready,
        AppReadiness::Blocked => RegionState::Blocked,
        AppReadiness::Error => RegionState::Error,
    }
}

/// node.states の文（key = empty / requires_connection / disabled）。
pub fn state_text<'a>(node: &'a UiNode, key: &str) -> Option<&'a str> {
    node.states.as_ref()?.get(key)?.as_str()
}

/// 中身を描いてよい状態（それ以外は状態語だけを出す）。
pub fn shows_content(state: RegionState) -> bool {
    matches!(state, RegionState::Ready | RegionState::Partial | RegionState::Stale | RegionState::Running | RegionState::Verified)
}

/// bind 先を持つ入力（form / input / editor）を打ってよい状態。読み込み中・未接続・失敗・古い・実行中 等は、
/// どの record への入力か決められない（または上書きしてよいか分からない）ので打たせず、form は送らせない（R2-01）。
pub fn editable(state: RegionState) -> bool {
    matches!(state, RegionState::Ready | RegionState::Empty | RegionState::Partial | RegionState::Verified)
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// 入力の部品（form / input / editor）の region・補足・bind 先の record・打ってよいか。runtime が Error と言う時（保存・送信の失敗）は
/// bind 先の record を失ったのではないので、record を出したまま打ち直して送り直せる（R3-01・DESIGN.md:213 の retry・CD-03 の retryable）。
/// 取得元が使えない Error（対応外・定義に無い）は record が無いので打てない（R2-01 のまま）。list / table は失敗の間も行を描かない（region のまま）。
pub fn input_region(app: &AppDefinitionV3, node: &UiNode, host: &dyn ActionHost) -> (RegionState, String, Option<Value>, bool) {
    let (state, detail, data) = region(app, node, host);
    if state == RegionState::Error && host.region(&node.key).is_some() {
        let record = node.binding.as_ref().and_then(|b| host.data(b)).filter(|v| !is_empty(v));
        let can_type = record.is_some();
        return (state, detail, record, can_type);
    }
    (state, detail, data, editable(state))
}

/// region の状態・補足・描いてよい data。順: runtime が知っている状態 → binding の readiness → data の有無。
pub fn region(app: &AppDefinitionV3, node: &UiNode, host: &dyn ActionHost) -> (RegionState, String, Option<Value>) {
    let empty = || state_text(node, "empty").unwrap_or("").to_owned();
    let data = || node.binding.as_ref().and_then(|b| host.data(b)).filter(|v| !is_empty(v));
    if let Some((state, detail)) = host.region(&node.key) {
        let data = if shows_content(state) { data() } else { None };
        // runtime が Ready（idle の run 等）と言っても中身が無ければ、空白を残さず「まだデータがありません」（UI-007）。
        // runtime の補足（取り消した run の「取り消しました」等）が在ればそれを残し、無ければ作者の空の文。
        if state == RegionState::Ready && data.is_none() {
            return (RegionState::Empty, if detail.is_empty() { empty() } else { detail }, None);
        }
        return (state, detail, data);
    }
    let Some(binding) = &node.binding else {
        return (RegionState::Empty, empty(), None);
    };
    match binding.readiness(&app.sources, None) {
        BindingReadiness::Ready => match data() {
            Some(v) => (RegionState::Ready, String::new(), Some(v)),
            None => (RegionState::Empty, empty(), None),
        },
        BindingReadiness::RequiresConnection => (RegionState::Disconnected, state_text(node, "requires_connection").unwrap_or("").to_owned(), None),
        BindingReadiness::Unverified => (RegionState::Loading, "Checking the connection".to_owned(), None),
        BindingReadiness::Stale => (RegionState::Stale, "Some sources aren't available".to_owned(), None),
        BindingReadiness::Unsupported => (RegionState::Error, "This data source isn't supported".to_owned(), None),
        BindingReadiness::MissingSource(id) => (RegionState::Error, format!("The data source “{id}” is missing from the project"), None),
    }
}

/// 押せない理由。`state` は §11 の状態が当てはまる時だけ（接続が必要・実行中 等）、`reason` は人に見せる 1 文。
#[derive(Clone, Debug, PartialEq)]
pub struct Gate {
    pub state: Option<RegionState>,
    pub reason: String,
}

impl Gate {
    /// hover と inspector に出す文（理由が空なら状態語）。
    pub fn text(&self) -> String {
        match (&self.state, self.reason.is_empty()) {
            (Some(s), true) => s.wording().to_owned(),
            (Some(s), false) => format!("{} · {}", s.wording(), self.reason),
            (None, _) => self.reason.clone(),
        }
    }
}

/// action を今押せるか（CD-02 の matrix）。Ok は invoke する action id。
/// 順: 実行モード → 割り当て → 定義 → runtime の状態（実行中 等・form は bind 先が打てない状態）→ readiness → local_demo の外部効果 → capability → 入力。
/// 入力は呼ぶ側が組んで押した時に送る物（form の宣言した欄・composer の文・recorder の秒数・それ以外は {}）で、
/// runtime の `validate_invocation` と同じ `check_input` で見る（落とされる input を送る button を押せる形で出さない・DESIGN §4.1）。
/// 他の node の値（editor の文・選んだ行）は詰めない: v3 の UiNode にはどの値を input にするかを書く field が無い。
/// 契約（UI-6a = sendback 60.1）: runtime の状態の段で `host.locked(&node.key)` が true の node も押せなくする
/// （理由の状態と文は `host.region` の物 = Host は EffectUnknown を `Stale` と今の文 `Can't tell if it arrived · {reason} · Open Runs` で入れる）。押せない事と状態の語を分ける:
/// Running・Loading・Blocked は今まで通り押せない・`Stale` の他の region（disk と違う窓など）は押せるまま。
/// Wave: UI-6a
pub fn gate(app: &AppDefinitionV3, node: &UiNode, host: &dyn ActionHost, input: &Value) -> Result<String, Gate> {
    let because = |state: Option<RegionState>, fallback: &str| Gate {
        state,
        reason: state_text(node, "disabled").unwrap_or(fallback).to_owned(),
    };
    if app.state.execution_mode == ExecutionMode::Disabled {
        return Err(because(None, "Actions are turned off for this project"));
    }
    let Some(id) = node.action_ref.as_deref() else {
        return Err(because(None, "No action is set for this button"));
    };
    let Some(action) = app.action(id) else {
        return Err(because(None, &format!("The action “{id}” isn't defined")));
    };
    // 押し直すと同じ手が 2 回届く node（locked）も押せない。状態の語は region の物のまま（Stale でも押せない・sendback 60.1）
    let region = host.region(&node.key);
    if host.locked(&node.key) || region.as_ref().is_some_and(|(s, _)| matches!(s, RegionState::Running | RegionState::Loading | RegionState::Blocked)) {
        let (state, reason) = region.unwrap_or((RegionState::Stale, String::new()));
        return Err(Gate { state: Some(state), reason });
    }
    // form の入力は bind 先の record の欄から組む: 打てない状態（未接続・失敗・古い 等）の間は送らせない。
    // 理由はその状態（打てない欄を指して「欄名を入れると押せます」とは言わない）
    if node.component == "form" && node.binding.is_some() {
        let (state, detail, _, can_type) = input_region(app, node, host);
        if !can_type {
            return Err(Gate { state: Some(state), reason: detail });
        }
    }
    match host.readiness(id).unwrap_or(action.readiness) {
        ActionReadiness::Ready => {}
        ActionReadiness::PreviewOnly => return Err(because(None, "Not run in preview")),
        ActionReadiness::RequiresConnection => return Err(because(Some(RegionState::Disconnected), "")),
        ActionReadiness::Unsupported => return Err(because(None, "This action isn't supported")),
    }
    // validator（V4 PhantomReady）が先に落とす形。描く側でも外へは出さない。
    if app.state.execution_mode == ExecutionMode::LocalDemo && action.effect.is_external() {
        return Err(because(None, "Not sent outside in preview"));
    }
    if let Some(cap_ref) = &action.capability_ref {
        let Some(cap) = app.capability(cap_ref) else {
            return Err(because(None, &format!("The required capability “{cap_ref}” isn't defined")));
        };
        if let Some((state, fallback)) = capability_gate(cap.readiness) {
            return Err(because(state, fallback));
        }
    }
    if let Err(e) = check_input(&action.input_schema, input) {
        return Err(Gate { state: None, reason: input_reason(node, &e) });
    }
    Ok(action.id.clone())
}

/// 入力が通らない理由。form の欄なら欄の名前で次の一手を、画面に欄の無い入力は名前をそのまま（定義の穴なので作者と Agent が直せるように）。
fn input_reason(node: &UiNode, e: &InvocationError) -> String {
    let label = |field: &str| {
        let f = node.props.get("fields")?.as_array()?.iter().find(|f| f.get("name").and_then(Value::as_str) == Some(field))?;
        Some(f.get("label").and_then(Value::as_str).unwrap_or(field).to_owned())
    };
    match e {
        InvocationError::MissingInput { field } => match label(field) {
            Some(l) => format!("Enter “{l}” to enable this"),
            None => format!("No field on this screen sets the input “{field}”"),
        },
        InvocationError::InputType { field, .. } => format!("The value for “{}” has the wrong format", label(field).unwrap_or_else(|| field.clone())),
        other => other.to_string(),
    }
}

/// capability の readiness → 押せない時の状態と既定の文（CD-02 の表の「画面の語」）。Ready は None。
pub fn capability_gate(r: CapabilityReadiness) -> Option<(Option<RegionState>, &'static str)> {
    use CapabilityReadiness::*;
    Some(match r {
        Ready => return None,
        Unknown => (None, "Not confirmed as usable"),
        Discoverable | RequiresConnection => (Some(RegionState::Disconnected), ""),
        ConnectedUnverified => (None, "Checking the connection"),
        Degraded => (None, "Partly unavailable"),
        Unsupported => (None, "This action isn't supported"),
        Revoked => (Some(RegionState::Disconnected), "Connect"),
        SimulatedInPreview => (None, "Not run in preview"),
    })
}

/// run の状態（DESIGN.md:211 の 10 語 = domain の RunStatus）→ 状態語と既定の補足。
/// `uncertain` は「送信済みか未確認」（§11）で確認待ちに置く。`verified` だけが検証済み（success 色）。
pub fn run_state(status: RunStatus) -> (RegionState, &'static str) {
    match status {
        RunStatus::Idle => (RegionState::Ready, ""),
        RunStatus::Planning => (RegionState::Loading, "Getting ready"),
        RunStatus::WaitingConnection => (RegionState::Disconnected, ""),
        RunStatus::WaitingApproval => (RegionState::Blocked, ""),
        RunStatus::Running => (RegionState::Running, ""),
        RunStatus::Verifying => (RegionState::Running, "Checking the result"),
        RunStatus::Verified => (RegionState::Verified, ""),
        RunStatus::Failed => (RegionState::Error, ""),
        RunStatus::Cancelled => (RegionState::Ready, "Undone"),
        RunStatus::Uncertain => (RegionState::Stale, "Not confirmed as sent"),
        RunStatus::WaitingHuman => (RegionState::Blocked, "Waiting for your choice"),
    }
}

/// data の `status` を RunStatus として読む（schema の snake_case。読めない語は None）。
pub fn parse_run_status(v: &Value) -> Option<RunStatus> {
    serde_json::from_value(v.get("status")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_verified_run_is_verified_and_uncertain_is_never_done() {
        for s in [
            RunStatus::Idle,
            RunStatus::Planning,
            RunStatus::WaitingConnection,
            RunStatus::WaitingApproval,
            RunStatus::Running,
            RunStatus::Verifying,
            RunStatus::Verified,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Uncertain,
        ] {
            let (state, detail) = run_state(s);
            assert_eq!(state == RegionState::Verified, s == RunStatus::Verified, "{s:?}");
            assert!(!["完了", "成功", "Done", "Success"].iter().any(|w| detail.contains(w)), "{s:?}: {detail}");
        }
        assert_eq!(run_state(RunStatus::Uncertain), (RegionState::Stale, "Not confirmed as sent"), "承認の無い行に ▣ Needs approval を出さない（UI-1 反証）");
    }
    #[test]
    fn only_ready_capability_lets_a_button_through() {
        use CapabilityReadiness::*;
        for r in [Unknown, Discoverable, RequiresConnection, ConnectedUnverified, Ready, Degraded, Unsupported, Revoked, SimulatedInPreview] {
            assert_eq!(capability_gate(r).is_none(), r == Ready, "{r:?}");
        }
    }
}
