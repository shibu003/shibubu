//! PermissionGate（RUN-002・CD-01・ADR-005）: effect の 6 値（domain の Effect のまま）と scope × space × app × 期限で決める。
//! 承認は exact payload の sha256 に結び付ける（承認の後に中身・宛先 app・space・scope が変われば hash が変わる）。
//! user の軸は無い（1 台 1 人の local app）。resource は scope の `provider.resource.verb` で表す。
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::domain::action::{ActionDefinition, Effect, InvocationOrigin};
use super::domain::app::{AppDefinitionV3, AutonomyLevel};
use super::domain::permission::PermissionGrant;
use super::domain::run::ErrorCode;
use super::internal;
use super::registry::{ActionContract, LOCAL, UI_PATCH};
use super::storage::{Store, Table};

/// permissions 表の 1 行: domain の PermissionGrant に space を添える（grant は space + app ごと・RUN-002・CD-16）。
/// domain の PermissionGrant に `space_id` を足すと tests/contracts.rs:358 の struct literal が compile できなくなるので、ここで持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRecord {
    pub space_id: String,
    pub grant: PermissionGrant,
}

/// この app の grant（他の形の行は読まない = 許可しない側に倒す）。
pub fn grants(store: &Store, app_id: &str) -> Result<Vec<GrantRecord>, ErrorCode> {
    let rows = store.list(Table::Permissions, Some(app_id)).map_err(internal)?;
    Ok(rows.into_iter().filter_map(|r| serde_json::from_value(r.json).ok()).collect())
}

/// action が要る scope = registry の契約の scope（adapter が本当に要る物・正）∪ app の宣言（action と capability の
/// required_scopes）。sort 済み。app の宣言は足せるが減らせない: composer / LLM が無害に見える scope に書き換えても、契約の scope の
/// grant が無ければ通らない（RT-SEC-R2-1）。validate_grant は app が宣言した scope しか grant にしないので、契約の scope を宣言しない
/// app はその action を呼べない（閉じる側に倒す）。
pub fn required_scopes(def: &AppDefinitionV3, action: &ActionDefinition, contract: &ActionContract) -> Vec<String> {
    let mut scopes: Vec<String> = contract.scopes.iter().map(|s| (*s).to_string()).collect();
    scopes.extend(action.required_scopes.iter().cloned());
    if let Some(cap) = action.capability_ref.as_deref().and_then(|id| def.capability(id)) {
        scopes.extend(cap.required_scopes.iter().cloned());
    }
    scopes.sort();
    scopes.dedup();
    scopes
}

/// 全ての scope に、この app・この space の期限内の grant が在るか。別 space・別 app の grant は数えない。
pub fn scopes_granted(scopes: &[String], app_id: &str, space_id: &str, grants: &[GrantRecord], now: &str) -> bool {
    scopes.iter().all(|scope| {
        grants.iter().any(|g| g.space_id == space_id && g.grant.app_id == app_id && &g.grant.scope == scope && g.grant.active_at(now))
    })
}

/// gate。Ok(true) = 承認が要る（ApprovalRequest を返して止まる）、Ok(false) = すぐ実行、Err = 拒否。
/// - 見る scope は `required_scopes`（registry の契約 ∪ app の宣言）。外へ出る effect と外部 adapter の action は契約の scope が必須
/// - local_read / local_write: すぐ実行（`requires_approval: true` なら確認する）
/// - external_read: grant（接続の時の同意 1 回）があればすぐ実行。run ごとの確認は出さない（CD-17 #2 の仮置き）
/// - external_write / external_send / destructive: 毎回、exact payload の承認（`allow always` は無い）
/// - Agent 由来は app の自律度（CD-03 E）: suggest_only は local_read 以外を聞く、auto_local_reversible は ui.patch 以外の書込と外部を聞く
pub fn gate(
    def: &AppDefinitionV3,
    action: &ActionDefinition,
    contract: &ActionContract,
    origin: &InvocationOrigin,
    space_id: &str,
    grants: &[GrantRecord],
    now: &str,
) -> Result<bool, ErrorCode> {
    // 外へ出る操作と外部 adapter の操作は、要る scope を registry の契約が持つ（app の宣言だけだと、関係の無い scope の grant で通る道が
    // できる）。契約に無いのは adapter の登録の誤りなので、app が scope を宣言していても通さない。effect の 3 値（is_external）だけだと、
    // 一番重い外部の destructive が scope 無しの登録で通った（RT-R3-1）
    if (action.effect.is_external() || contract.adapter != LOCAL) && contract.scopes.is_empty() {
        return Err(ErrorCode::PermissionDenied);
    }
    let scopes = required_scopes(def, action, contract);
    if !scopes_granted(&scopes, &def.id, space_id, grants, now) {
        return Err(ErrorCode::PermissionDenied);
    }
    let by_effect = matches!(action.effect, Effect::ExternalWrite | Effect::ExternalSend | Effect::Destructive) || action.requires_approval;
    let by_autonomy = matches!(origin, InvocationOrigin::Agent { .. })
        && match def.agent.autonomy_level {
            AutonomyLevel::SuggestOnly => action.effect != Effect::LocalRead,
            // ponytail: 戻せる local の書込 = ui.patch（undo は前の revision）だけと見る。record の undo が出来たら足す
            AutonomyLevel::AutoLocalReversible => !(action.effect == Effect::LocalRead || action.id == UI_PATCH),
            AutonomyLevel::AskBeforeExternalEffects => false, // external_write 以上は by_effect が聞く
        };
    Ok(by_effect || by_autonomy)
}

/// 承認の対象（この app・space で、この action に、この入力を、この scope で）の sha256。
pub fn payload_hash(action_id: &str, app_id: &str, space_id: &str, input: &Value, scopes: &[String]) -> String {
    digest(&json!({ "action_id": action_id, "app_id": app_id, "space_id": space_id, "input": input, "scopes": scopes }))
}

/// 正準形 JSON の sha256（承認の hash・入力の digest・revision の digest・app の指紋が同じ作り方を使う）。
pub fn digest(v: &Value) -> String {
    sha256_hex(canonical(v).as_bytes())
}

/// 正準形の JSON。serde_json の Map は `preserve_order` が無いので key が sort 済みで、`to_string` は空白を入れない
/// （`cargo tree -e features -i serde_json` = default / std だけ）。preserve_order が入ると key の順だけ違う 2 つの hash が
/// 割れる（tests/runtime.rs の canonical の test が赤になる）。
pub fn canonical(v: &Value) -> String {
    v.to_string()
}

/// SHA-256（FIPS 180-4）。std に無く、crate を足さない規約なので手で書いた。NIST の例で tests/runtime.rs が測る。
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
        0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
        0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
