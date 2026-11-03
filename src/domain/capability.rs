//! CapabilityDescriptor — schema `capabilities[]`（CORE-002）。
//! 引くのは `AppDefinitionV3::capability(id)` の完全一致だけ。provider / adapter の名前の似ている物を実行可能にしない。
//! TODO の input_schema / output_schema / effect_class / rate_limits / fallback は schema v3 に無いので持たない（action 側にある）。
use super::action::Effect;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDescriptor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub readiness: CapabilityReadiness,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
}

/// AGENT_NATIVE_RUNTIME §4.1 の状態 + preview 用の simulated_in_preview。実行できるのは Ready だけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReadiness {
    Unknown,
    Discoverable,
    RequiresConnection,
    ConnectedUnverified,
    Ready,
    Degraded,
    Unsupported,
    Revoked,
    SimulatedInPreview,
}

/// DESIGN §10 の段（CD-02 (2)）。Unavailable は CD-02 の「段なし」。画面（ui::connections）も adapter の registry もこの 1 つを使う。
/// Verified は readiness からは出ない（最後の結果が Verified の時だけ: adapters::sdk::tier_with）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayTier {
    Unavailable,
    Available,
    Connected,
    Ready,
    Verified,
}

impl DisplayTier {
    /// §10 の語をそのまま（翻訳しない）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "Unavailable",
            Self::Available => "Available",
            Self::Connected => "Connected",
            Self::Ready => "Ready",
            Self::Verified => "Verified",
        }
    }
}

impl CapabilityReadiness {
    /// readiness → 段（CD-02 (2) の表）。
    pub const fn tier(self) -> DisplayTier {
        match self {
            Self::Discoverable | Self::RequiresConnection => DisplayTier::Available,
            Self::ConnectedUnverified | Self::Degraded => DisplayTier::Connected,
            Self::Ready => DisplayTier::Ready,
            Self::Unknown | Self::Unsupported | Self::Revoked | Self::SimulatedInPreview => DisplayTier::Unavailable,
        }
    }
}

/// CD-07 / CD-48: adapter の種類はこの 1 つだけ（composer の GoalPlan の候補・adapters の SDK・画面が同じ物を使う）。
/// GoalPlan v1 の 9 値（canonical）+ undecided。他文書・corpus の名は alias で受ける（FromStr）。
/// capability の `adapter` は schema どおり文字列のまま（V7 の正規化は validate.rs の注記どおり後で）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    LocalCore,
    Api,
    Mcp,
    /// AI の GUI 操作（Shibubu Bridge の AX・YC W17-2 #12）。保存は "mac_gui"・W17-2 より前の保存と GoalPlan の "cua" も読む
    #[serde(alias = "cua")]
    MacGui,
    Repo,
    OriginalSurface,
    Extension,
    Device,
    Manual,
    Undecided,
}

impl AdapterKind {
    pub const ALL: [AdapterKind; 10] =
        [Self::LocalCore, Self::Api, Self::Mcp, Self::MacGui, Self::Repo, Self::OriginalSurface, Self::Extension, Self::Device, Self::Manual, Self::Undecided];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalCore => "local_core",
            Self::Api => "api",
            Self::Mcp => "mcp",
            Self::MacGui => "mac_gui",
            Self::Repo => "repo",
            Self::OriginalSurface => "original_surface",
            Self::Extension => "extension",
            Self::Device => "device",
            Self::Manual => "manual",
            Self::Undecided => "undecided",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKind(pub String);
impl fmt::Display for UnknownKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "adapter の種類 '{}' は canonical 9 値・undecided・alias のどれでもない", self.0)
    }
}

impl FromStr for AdapterKind {
    type Err = UnknownKind;
    /// CD-07 の表。2 語（corpus 07 / 10・統合 2 の merge 条件 4・CD-48）:
    /// `os_automation`（AppleScript / ReaScript）→ api: app が公開している scripting の口で、GUI を押す物ではない。
    ///   Cua（CuaDriver）は GUI の最終手段（SPEC:104）なので写さない。
    /// `local_device`（mic）→ local_core: 同じ process が cpal で読む local の hardware（許可は TCC の mic）。device は電話などの外の機器。
    fn from_str(s: &str) -> Result<Self, UnknownKind> {
        use AdapterKind::*;
        Ok(match s {
            "local_core" | "local_state" | "local_collection" | "local" | "entity" | "local_device" => LocalCore,
            "api" | "API" | "external_api" | "public_web" | "web_fetch" | "os_automation" => Api,
            "mcp" | "MCP" => Mcp,
            "mac_gui" | "MacGui" | "cua" | "Cua" | "cua_browser_tab" => MacGui,
            "repo" | "Repo" | "trusted_adapter" => Repo,
            "original_surface" | "source_surface" | "provider_surface" | "embed" | "Embed" | "handoff" => OriginalSurface,
            "extension" | "safe_extension" => Extension,
            "device" | "ADB" | "companion" => Device,
            "manual" | "Manual" | "manual_handoff" => Manual,
            "undecided" | "external_optional" => Undecided,
            other => return Err(UnknownKind(other.into())),
        })
    }
}

// ---- W19-1: Capability Registry の保存形式と、利用者の data に置く Adapter（recipe）の保存形式（owner 2026-10-21 18・19 通目・docs/ANALYSIS_BACKEND.md §5）----

/// `CapabilityRecord::schema` の値。
/// Wave: W19-1
pub const CAPABILITY_SCHEMA: &str = "shibubu.capability/1";
/// `Recipe::schema` の値。
/// Wave: W19-1
pub const RECIPE_SCHEMA: &str = "shibubu.recipe/1";
/// 利用者の data dir の下の置き場: 確かめた能力（`CapabilityRecord` 1 件 1 file `<id>.json`）。
/// Wave: W19-1
pub const CAPABILITIES_DIR: &str = "capabilities";
/// 利用者の data dir の下の置き場: Engine が作った・利用者が入れた Adapter（`<id>/recipe.json`・生成した code は `<id>/vN/`）。
/// repo の `extensions/` も同じ形（製品に同梱する Adapter・Core の code ではない）。
/// Wave: W19-1
pub const EXTENSIONS_DIR: &str = "extensions";
/// `EXTENSIONS_DIR/<id>/` の中の recipe の file 名。
/// Wave: W19-1
pub const RECIPE_FILE: &str = "recipe.json";

/// 能力が誰の物か（19 通目 §5「アプリ固有の Capability / 複数アプリで再利用できる Capability」）。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reuse {
    /// 1 つの app / service に固有（app = bundle id か origin）
    AppSpecific { app: String },
    /// 複数の app で使える（file の変換・検索・data の加工）
    Shared,
}

/// 能力がどこから来たか（19 通目 §5「公式連携 / 解析から発見した候補 / Engine が生成した機能」）。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum Origin {
    /// app / service が公開している連携（API・SDK・CLI・MCP・plugin・app の script）。src/adapters/ の Adapter もここ
    Official,
    /// Analysis Backend（REA など・W19-3）の解析から見つけた候補。report = `intel::analysis::AnalysisReport::id`
    Analysis { backend: String, report: String },
    /// Engine が生成した物（recipe・code・Workflow）。artifact = `EXTENSIONS_DIR` の下の id
    Generated { artifact: String },
}

/// 確かめたか（19 通目 §5「実機検証済み / 未検証・非対応」）。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Verification {
    Unverified,
    /// 元の app / service / file の実状態を、操作とは別の道で読んで確かめた（at = RFC3339 UTC・evidence = 何をどの道で読んだかの 1 行）
    Verified { at: String, evidence: String },
    /// 使えない（理由の 1 行。解析できても安全で許された連携が無い時もここに書く・W19-3 の合格の条件）
    Unsupported { reason: String },
}

/// Capability Registry の 1 件（利用者の data の `CAPABILITIES_DIR/<id>.json`）。`CapabilityDescriptor`（app の定義の中の能力）に、
/// 誰の物か・どこから来たか・確かめたか・版・権限・根拠・制約を足した物。
/// Wave: W19-1
/// 使い方: `let r: CapabilityRecord = serde_json::from_str(&text)?; if r.readiness() == CapabilityReadiness::Ready { /* 選んでよい */ }`
/// 保証する: 未知の欄は拒否・`schema` は `CAPABILITY_SCHEMA`・Core は `kind`（CD-07 の 1 つ）で手段を見る（API・SDK・app の script = Api・CLI = Repo・
/// MCP = Mcp・AX = MacGui・DOM / 埋め込み = OriginalSurface・plugin = Extension・生成した code = Origin::Generated の LocalCore）。
/// 保証しない: 実体の Adapter が今も在る事（registry の `check`）・版が今の環境と同じ事（`capability_engine::needs_reverify`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRecord {
    pub schema: String,
    /// action id と同じ名前（`<provider>.<…>`・CD-08）
    pub id: String,
    pub reuse: Reuse,
    pub origin: Origin,
    pub kind: AdapterKind,
    pub verification: Verification,
    /// 確かめた対象の版（app の CFBundleShortVersionString・service の origin・OS の版）
    #[serde(default)]
    pub versions: Vec<String>,
    /// 要る権限（scope・TCC の名前）
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 根拠（file:line・URL・解析の report の id）
    #[serde(default)]
    pub evidence: Vec<String>,
    /// 分かっている制約（内部の実装に依る・版が限られる・許された連携が無い）
    #[serde(default)]
    pub constraints: Vec<String>,
}

impl CapabilityRecord {
    /// 画面と registry が使う readiness。
    /// Wave: W19-1
    /// 保証する: Ready は `Verification::Verified` の時だけ（調査しただけ・生成しただけの能力を Ready にしない・19 通目 §5）・
    /// Unverified は Discoverable・Unsupported は Unsupported。保証しない: 今この Mac で動く事（`needs_reverify`）。
    pub fn readiness(&self) -> CapabilityReadiness {
        match self.verification {
            Verification::Verified { .. } => CapabilityReadiness::Ready,
            Verification::Unverified => CapabilityReadiness::Discoverable,
            Verification::Unsupported { .. } => CapabilityReadiness::Unsupported,
        }
    }
}

/// 利用者の data に置く Adapter の保存形式（`EXTENSIONS_DIR/<id>/RECIPE_FILE`）。Engine が作る・利用者が入れる。Core に app 専用の code を足さずに
/// app 固有の能力（CLI・MCP・AX の手順・DOM・生成した code・HTTP の API）を registry に載せる（owner 16・18 通目）。
/// Wave: W19-1
/// 使い方: `let n = registry.add_recipes(&data_dir.join(EXTENSIONS_DIR), &mut || capability_engine::call_runner(mcp.clone(), data_dir.clone()));`
/// 保証する: 未知の欄は拒否・`schema` は `RECIPE_SCHEMA`・鍵と token の値を書かない（auth は registry の Adapter の道）。
/// 保証しない: 手順が今も動く事（各 action の `verification` と registry の `check`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: String,
    /// adapter の id（provenance と event の名前・registry で重ならない）
    pub id: String,
    /// capability の provider（CON-006 の key）
    pub provider: String,
    pub reuse: Reuse,
    pub origin: Origin,
    #[serde(default)]
    pub versions: Vec<String>,
    pub actions: Vec<RecipeAction>,
}

/// recipe の action 1 つ（`adapters::sdk::Spec` の data 版）。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeAction {
    /// action id（dot を含む小文字・CD-08）
    pub id: String,
    pub effect: Effect,
    /// 実行の前に人の承認が要る（external_write 以上と、他 app を戻せない形で変える local_write）
    pub approval: bool,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// input の JSON Schema（object）
    pub input: Value,
    /// 検証の仕方（CD-04 (4) の語: read_back / state_compare / source_receipt / human_review）
    pub verifier: String,
    pub call: RecipeCall,
    pub verification: Verification,
}

/// recipe の action の呼び方（手段は交換・組み合わせができる: 19 通目 §7 W19-1）。input は `{{input.<key>}}` で入る。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipeCall {
    /// CLI（argv をそのまま・shell を通さない）
    Cli { argv: Vec<String> },
    /// MCP の tool（server は `mcp.json` の名前）
    Mcp { server: String, tool: String, arguments: Value },
    /// AI の GUI 操作の手順（1 手 = `gui::GuiAction` の JSON に、element を探す `find: {role, label}` を足した物）
    Gui { steps: Vec<Value> },
    /// WebView の頁で走らせる JavaScript（`origin` の頁だけ・`surface::may_evaluate`）
    Dom { origin: String, script: String },
    /// Engine が生成した code（`codegen` の隔離した JXA・artifact は `EXTENSIONS_DIR/<id>/vN/`）
    Code { artifact: String },
    /// 公開の HTTP API（鍵を recipe に書かない）
    Http {
        method: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
}

/// W19-1 の保存形式（serde の往復・未知の欄の拒否）と enum の網羅、`readiness` の規則。
#[cfg(test)]
mod w19_1_interface {
    use super::*;
    use serde_json::json;

    /// 19 通目 §5「調査しただけの能力を Ready として扱ってはいけない」: Ready は Verified だけ。解析の候補・生成物・公式の連携でも、
    /// 確かめていなければ Discoverable（registry の gate が実行しない）・非対応は Unsupported。origin・reuse は readiness を変えない。
    #[test]
    fn w19_1_only_verified_records_are_ready() {
        let verified = Verification::Verified { at: "2026-10-23T00:00:00Z".into(), evidence: "disk の sha".into() };
        let unsupported = Verification::Unsupported { reason: "安全で許された連携が無い".into() };
        for origin in [Origin::Official, Origin::Analysis { backend: "rea".into(), report: "a1".into() }, Origin::Generated { artifact: "x".into() }] {
            for reuse in [Reuse::Shared, Reuse::AppSpecific { app: "org.example.app".into() }] {
                assert_eq!(record(reuse.clone(), origin.clone(), Verification::Unverified).readiness(), CapabilityReadiness::Discoverable, "{origin:?}");
                assert_eq!(record(reuse.clone(), origin.clone(), verified.clone()).readiness(), CapabilityReadiness::Ready, "{origin:?}");
                assert_eq!(record(reuse.clone(), origin.clone(), unsupported.clone()).readiness(), CapabilityReadiness::Unsupported, "{origin:?}");
            }
        }
    }

    fn record(reuse: Reuse, origin: Origin, verification: Verification) -> CapabilityRecord {
        CapabilityRecord {
            schema: CAPABILITY_SCHEMA.into(),
            id: "example.doc.title.set".into(),
            reuse,
            origin,
            kind: AdapterKind::MacGui,
            verification,
            versions: vec!["1.19".into()],
            permissions: vec!["accessibility".into()],
            evidence: vec!["read_back".into()],
            constraints: vec![],
        }
    }

    #[test]
    fn w19_1_readiness_signature() {
        let _: fn(&CapabilityRecord) -> CapabilityReadiness = CapabilityRecord::readiness;
    }

    #[test]
    fn w19_1_capability_record_round_trips_every_variant_and_rejects_unknown_fields() {
        let reuses = [Reuse::AppSpecific { app: "org.example.app".into() }, Reuse::Shared];
        let origins = [Origin::Official, Origin::Analysis { backend: "rea".into(), report: "a1".into() }, Origin::Generated { artifact: "x".into() }];
        let checks = [
            Verification::Unverified,
            Verification::Verified { at: "2026-10-21T00:00:00Z".into(), evidence: "disk の sha".into() },
            Verification::Unsupported { reason: "安全で許された連携が無い".into() },
        ];
        // 網羅: 区別を足したら、ここと CapabilityRecord::readiness を同じ commit で直す
        for r in &reuses {
            match r {
                Reuse::AppSpecific { .. } | Reuse::Shared => {}
            }
        }
        for o in &origins {
            match o {
                Origin::Official | Origin::Analysis { .. } | Origin::Generated { .. } => {}
            }
        }
        for c in &checks {
            match c {
                Verification::Unverified | Verification::Verified { .. } | Verification::Unsupported { .. } => {}
            }
        }
        for r in &reuses {
            for o in &origins {
                for c in &checks {
                    let rec = record(r.clone(), o.clone(), c.clone());
                    let v = serde_json::to_value(&rec).unwrap();
                    assert_eq!(serde_json::from_value::<CapabilityRecord>(v).unwrap(), rec);
                }
            }
        }
        let mut v = serde_json::to_value(record(Reuse::Shared, Origin::Official, Verification::Unverified)).unwrap();
        v["ready"] = json!(true);
        assert!(serde_json::from_value::<CapabilityRecord>(v).is_err(), "未知の欄（自己申告の ready）は拒否");
    }

    #[test]
    fn w19_1_recipe_round_trips_every_call_and_rejects_unknown_fields() {
        let calls = vec![
            RecipeCall::Cli { argv: vec!["tool".into(), "{{input.path}}".into()] },
            RecipeCall::Mcp { server: "s".into(), tool: "t".into(), arguments: json!({}) },
            RecipeCall::Gui { steps: vec![json!({ "op": "press", "find": { "role": "AXButton", "label": "Save" } })] },
            RecipeCall::Dom { origin: "https://example.com".into(), script: "1".into() },
            RecipeCall::Code { artifact: "example/v1".into() },
            RecipeCall::Http { method: "GET".into(), url: "https://example.com/api".into(), body: None },
        ];
        for c in &calls {
            match c {
                RecipeCall::Cli { .. } | RecipeCall::Mcp { .. } | RecipeCall::Gui { .. } | RecipeCall::Dom { .. } | RecipeCall::Code { .. } | RecipeCall::Http { .. } => {}
            }
        }
        let recipe = Recipe {
            schema: RECIPE_SCHEMA.into(),
            id: "example".into(),
            provider: "example".into(),
            reuse: Reuse::AppSpecific { app: "org.example.app".into() },
            origin: Origin::Generated { artifact: "example".into() },
            versions: vec![],
            actions: calls
                .into_iter()
                .enumerate()
                .map(|(i, call)| RecipeAction {
                    id: format!("example.a{i}"),
                    effect: Effect::LocalRead,
                    approval: false,
                    scopes: vec![],
                    input: json!({ "type": "object" }),
                    verifier: "read_back".into(),
                    call,
                    verification: Verification::Unverified,
                })
                .collect(),
        };
        let v = serde_json::to_value(&recipe).unwrap();
        assert_eq!(serde_json::from_value::<Recipe>(v.clone()).unwrap(), recipe);
        let mut extra = v;
        extra["token"] = json!("x");
        assert!(serde_json::from_value::<Recipe>(extra).is_err(), "未知の欄（鍵の値など）は拒否");
    }
}
