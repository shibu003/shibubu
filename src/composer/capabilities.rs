//! CapabilityDiscovery（GEN-003）と local verb の契約。
//!
//! - `LOCAL_VERBS`: runtime が実装する local action（`local.<source>.<verb>`）の verb。**corpus（docs/examples/v3）の local action の
//!   verb だけ**（`update_<欄>` は corpus の `update_quantity` を欄の名前で一般化した物）。composer はこの表の verb しか出さず、
//!   BuildVerifier（verifier.rs）は ready な local action の verb がこの表に在る事を検査する（GEN-009「実装がある」）。
//! - `discover`: 接続が要る能力を registry（ComposeContext が渡す、Connections が知っている物）の **id の完全一致** で引く。
//!   connector を捏造しない: registry に無い物は `unknown`（unavailable）、名指しされた物は `requires_connection`（選択済み・未認証）、
//!   Kangae に組み込みの 3 つ（URL を OS に渡す handoff・editor の handoff・mic）だけ既定の readiness を持つ。
use serde::Serialize;

use super::domain::action::Effect;
use super::domain::capability::{CapabilityDescriptor, CapabilityReadiness};
use super::goal::ExternalNeed;

pub struct LocalVerb {
    pub verb: &'static str,
    pub effect: Effect,
    pub requires_approval: bool,
    /// 画面の既定の文言
    pub label: &'static str,
}

/// runtime の local action の契約（corpus の verb）。`update_` は前方一致（`update_<欄>`）。
pub const LOCAL_VERBS: &[LocalVerb] = &[
    LocalVerb { verb: "add", effect: Effect::LocalWrite, requires_approval: false, label: "追加する" },
    LocalVerb { verb: "update_", effect: Effect::LocalWrite, requires_approval: false, label: "変更を保存" },
    LocalVerb { verb: "delete", effect: Effect::Destructive, requires_approval: true, label: "削除する" },
    LocalVerb { verb: "filter", effect: Effect::LocalRead, requires_approval: false, label: "絞り込む" },
    LocalVerb { verb: "search", effect: Effect::LocalRead, requires_approval: false, label: "検索" },
    LocalVerb { verb: "import_csv", effect: Effect::LocalWrite, requires_approval: false, label: "CSV を読み込む" },
    LocalVerb { verb: "toggle_done", effect: Effect::LocalWrite, requires_approval: false, label: "済みにする" },
    LocalVerb { verb: "toggle", effect: Effect::LocalWrite, requires_approval: false, label: "開始 / 一時停止" },
    LocalVerb { verb: "reset", effect: Effect::LocalWrite, requires_approval: false, label: "リセット" },
    LocalVerb { verb: "save_settings", effect: Effect::LocalWrite, requires_approval: false, label: "設定を保存" },
    LocalVerb { verb: "create", effect: Effect::LocalWrite, requires_approval: false, label: "新しく作る" },
    LocalVerb { verb: "save", effect: Effect::LocalWrite, requires_approval: false, label: "保存" },
    LocalVerb { verb: "cite", effect: Effect::LocalWrite, requires_approval: false, label: "引用を入れる" },
    LocalVerb { verb: "open", effect: Effect::LocalWrite, requires_approval: false, label: "接続先を選ぶ" },
    LocalVerb { verb: "record", effect: Effect::LocalWrite, requires_approval: false, label: "録音する" },
];

/// source を持たない runtime 組み込みの namespace（`local.connection.open` は Connections を開く・`local.audio.record` は mic）。
pub const BUILTIN_NAMESPACES: &[&str] = &["connection", "audio"];

/// 組み込みの能力を通らなければ動かない verb（`record` は mic = OS の許可 `local.microphone`）。namespace を問わず、その capability を
/// 指していない action は ready を名乗れない（capability を外した録音を、承認も許可も要らない local_write として出さない）。
pub const GATED_VERBS: &[(&str, &str)] = &[("record", "local.microphone")];

/// Kangae に組み込みの能力（registry に無くても在る）。それ以外は registry か名指しでしか出ない。
pub const BUILTIN_CAPABILITIES: &[(&str, &str, CapabilityReadiness)] = &[
    // URL を OS に渡すだけ（corpus 09 / 10 の web.handoff と同じ）
    ("web.handoff", "handoff", CapabilityReadiness::Ready),
    // editor の候補はあるが、どれを使うかは未選択（corpus 05）
    ("editor.open", "handoff", CapabilityReadiness::Discoverable),
    // OS の mic 許可が要る（corpus 10）
    ("local.microphone", "local_device", CapabilityReadiness::RequiresConnection),
];

pub fn local_verb(verb: &str) -> Option<&'static LocalVerb> {
    LOCAL_VERBS.iter().find(|v| if v.verb.ends_with('_') { verb.len() > v.verb.len() && verb.starts_with(v.verb) } else { v.verb == verb })
}

/// `local.<namespace>.<verb>` を分ける（local でなければ None）。
pub fn split_local_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("local.")?;
    let (ns, verb) = rest.split_once('.')?;
    (!ns.is_empty() && !verb.is_empty() && !verb.contains('.')).then_some((ns, verb))
}

/// GEN-003 の 4 語（CD-02 の写像: available = discoverable ∨ ready / permission_needed = requires_connection ∨ connected_unverified /
/// unavailable = unknown ∨ degraded ∨ simulated / unsupported = unsupported ∨ revoked）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    PermissionNeeded,
    Unsupported,
}

impl Availability {
    pub fn of(r: CapabilityReadiness) -> Self {
        use CapabilityReadiness::*;
        match r {
            Ready | Discoverable => Self::Available,
            RequiresConnection | ConnectedUnverified => Self::PermissionNeeded,
            Unsupported | Revoked => Self::Unsupported,
            Unknown | Degraded | SimulatedInPreview => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Discovery {
    pub capability: String,
    pub availability: Availability,
    pub readiness: CapabilityReadiness,
    pub provider: Option<String>,
    pub adapter: Option<String>,
    /// どこで見つけたか（registry / builtin / named / none）
    pub found_in: &'static str,
}

/// id の完全一致だけ（名前の類似で引かない: CORE-002）。registry > 組み込み > 名指し > 無し（unknown）。
pub fn discover(need: &ExternalNeed, registry: &[CapabilityDescriptor]) -> Discovery {
    let mk = |readiness: CapabilityReadiness, provider: Option<String>, adapter: Option<String>, found_in: &'static str| Discovery {
        capability: need.capability.clone(),
        availability: Availability::of(readiness),
        readiness,
        provider,
        adapter,
        found_in,
    };
    if let Some(c) = registry.iter().find(|c| c.id == need.capability) {
        return mk(c.readiness, c.provider.clone().or_else(|| need.provider.clone()), c.adapter.clone().or_else(|| need.adapter.clone()), "registry");
    }
    if let Some((_, adapter, r)) = BUILTIN_CAPABILITIES.iter().find(|(id, _, _)| *id == need.capability) {
        return mk(*r, need.provider.clone(), Some((*adapter).into()), "builtin");
    }
    if need.named.is_some() {
        return mk(CapabilityReadiness::RequiresConnection, need.provider.clone(), need.adapter.clone(), "named");
    }
    mk(CapabilityReadiness::Unknown, None, None, "none")
}

/// registry / 組み込みが言う readiness（定義が自分で ready を名乗っていないかの照合用）。
pub fn known_readiness(id: &str, registry: &[CapabilityDescriptor]) -> Option<CapabilityReadiness> {
    registry
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.readiness)
        .or_else(|| BUILTIN_CAPABILITIES.iter().find(|(b, _, _)| *b == id).map(|(_, _, r)| *r))
}
