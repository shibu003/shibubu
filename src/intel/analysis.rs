//! W19-3: 交換できる Analysis Backend（owner 2026-10-21 19 通目 §2・§4・原文 docs/ANALYSIS_BACKEND.md）。公開の API・SDK・既存の Adapter で足りない時だけ、
//! 解析の許可のある software の内部構造と連携の可能性を調べる。REA（https://github.com/morluto/rea ・MIT・MCP と CLI）は 1 つ目の実装で、必須の依存にしない。
//! 解析の結果・推測・実行可能性・検証済みを `FindingStatus` で必ず分ける。解析の結果は Capability の候補（`Origin::Analysis`・Unverified）にしかならず、Ready にしない
//! （候補への変換と検証は Engine の `capability_engine::candidates_from` と Verification の段）。
//! 守る事（§4）: 権限のある software だけ（`AnalysisConsent`）・license・規約・第三者の権利・認証やアクセス制御の回避を標準の機能にしない・秘密と利用者の data を
//! 抜かない・送らない・解析の対象を起動 / install する処理は許可と隔離が要る・結果に版・根拠・信頼度・制約を書く・Mac に過大な負荷をかけない（`Placement::Remote`）。
//! self-contained（intel は tests/intel.rs と bin/kangae-intel.rs が `#[path]` で取り込む）: serde と std と `super::utc` だけ。MCP は `ToolCaller` と `ToolProber` を注入する
//! （本番は Core の配線 `capability_engine::analysis_backends` が `mcp::McpSession` の `open`・`tools`・`call_tool` を包む）。
//! Wave: W19-3
#![allow(dead_code)] // bin の kangae-intel と tests/intel.rs の #[path] の取り込みでは使わない（使うのは main の capability_engine）
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `AnalysisReport::schema` の値。
/// Wave: W19-3
pub const ANALYSIS_SCHEMA: &str = "shibubu.analysis/1";
/// 利用者の data dir の下の置き場: 解析の結果（`AnalysisReport` 1 件 1 file `<id>.json`）。`Origin::Analysis { report }` の根拠を後から読み戻せる。
/// Wave: W19-3
pub const ANALYSES_DIR: &str = "analyses";
/// REA の MCP の server の名前（`mcp.json` の key・利用者が入れた時だけ在る）と、REA の Backend の id（承認の action `analysis.rea`）。
/// 解析の道具かどうかは key で決めない（別の key で入れた REA も `looks_like_rea` で見分ける）。
/// Wave: W19-3
pub const REA_SERVER: &str = "rea";

/// 解析する対象。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisTarget {
    /// app の bundle id（無い software は None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// 解析する file か dir の絶対 path（.app・binary・asar・dll・repo）
    pub path: String,
    /// その版（Info.plist の CFBundleShortVersionString など・結果に必ず書く）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// 解析の許可（権限のある software だけを解析する・19 通目 §4）。無い依頼は `AnalysisError::NotPermitted`。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisConsent {
    /// 誰が許したか（利用者・試験の owner）
    pub granted_by: String,
    /// RFC3339 UTC
    pub at: String,
    /// 許した範囲（読むだけ・起動してよい・install してよい）の 1 行
    pub scope: String,
}

/// どこで解析するか（Mac に過大な負荷をかけない・重い解析は別の環境へ回せる口）。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case", deny_unknown_fields)]
pub enum Placement {
    /// この Mac（同時に 1 本だけ・解析・build・試験を重ねない）
    Local,
    /// 別の環境（endpoint は利用者が決める・未測なら未測と書く）
    Remote { endpoint: String },
}

/// 解析の依頼 1 件。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRequest {
    pub target: AnalysisTarget,
    /// 何を知りたいか（対象の機能・連携の可能性の 1 行）
    pub question: String,
    pub consent: AnalysisConsent,
    pub placement: Placement,
    pub timeout_ms: u64,
}

/// 見つけた物の種類（19 通目 §2 の調査の候補）。
/// Wave: W19-3
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// native の binary・関数・呼び出しの関係
    Binary,
    /// Electron の構成と IPC
    ElectronIpc,
    /// JavaScript・Web app の構造
    JavaScript,
    /// .NET の assembly
    DotNet,
    /// app の関連の file・resource（辞書・設定・plugin の置き場）
    Resource,
    /// 実行時の動作と入出力
    Runtime,
    /// 機能を作る module と依存
    Module,
    /// 上のどれでもない連携の手がかり
    Other,
}

/// 見つけた事の確からしさ（19 通目 §2「解析結果・推測・実行可能性・検証済みを必ず区別」）。
/// Wave: W19-3
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    /// 解析で読めた事実（symbol・file・IPC の channel 名）
    Analyzed,
    /// 事実からの推測
    Inferred,
    /// 外から呼べる事を確かめた（呼べても結果は未検証）
    Callable,
    /// 呼んだ結果を元の app の状態で確かめた（ここに来るのは Engine の Verification の後だけ・Backend は付けない）
    Verified,
}

/// 見つけた事 1 つ。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub kind: FindingKind,
    /// 何を見つけたか（1 行）
    pub claim: String,
    /// 根拠（symbol・offset・file・command の出力の要約。秘密と利用者の data を入れない）
    #[serde(default)]
    pub evidence: Vec<String>,
    /// 0.0〜1.0
    pub confidence: f64,
    pub status: FindingStatus,
}

/// 解析の結果 1 件（保存形式・`ANALYSIS_SCHEMA`）。Capability の候補の根拠（`Origin::Analysis { report: id }`）になる。
/// Wave: W19-3
/// 保証する: 対象の版・根拠・信頼度・制約を必ず持つ（無い時は空でなく「不明」と書く。対象の版が分からない時は `target` を依頼のまま〈`version: None`〉にし、
/// constraints に「対象の版が不明」と書く: Engine は依頼と同じ対象かを `target` の一致で見る）・`Verified` の finding を Backend は出さない。
/// 保証しない: 見つけた機能を外から呼べる事（`Callable` までが Backend の主張・それも Engine が確かめ直す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisReport {
    pub schema: String,
    /// Engine が付け直す（同じ Backend・対象・版なら同じ id = 2 回目は解析し直さずに読む）
    pub id: String,
    /// `AnalysisBackend::id`
    pub backend: String,
    pub target: AnalysisTarget,
    pub findings: Vec<Finding>,
    /// 分かった制約（内部の実装に依る・許された連携が無い・版が限られる）。安全で許された連携が無い時はそれを書く（それも W19-3 の合格の条件）
    #[serde(default)]
    pub constraints: Vec<String>,
    /// RFC3339 UTC
    pub at: String,
}

/// Backend が使えるか（REA は入っていない Mac が普通・必須の依存にしない）。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Availability {
    /// probe_ms = 道具を確かめるのに掛かった時間（起こす・`initialize`・tools/list。1 run の 1 回目の実測・Engine は解析の待ちの上限をこれで伸ばす・W19-3-fix）
    Ready { version: String, probe_ms: u64 },
    /// 入っていない・起こせない（理由の 1 行・Engine は解析の段を飛ばして既存の道で続ける）
    Missing { why: String },
}

/// 解析の誤り。Display は画面に出せる 1 行。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    /// Backend が無い・起こせない
    Unavailable(String),
    /// 解析の許可が無い・範囲の外（何も解析していない）
    NotPermitted(String),
    Timeout { ms: u64 },
    /// Mac が混んでいる（同時に 1 本の決まり）・別の環境へ回す
    Overloaded(String),
    /// Backend の誤り（壊れた応答・tool の誤り）
    Backend(String),
    /// 解析の結果（`ANALYSES_DIR`）を読めない・書けない・形が違う
    Store(String),
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "The analysis tool can't be used ({why}). Carrying on the existing way without analysis"),
            Self::NotPermitted(why) => write!(f, "Didn't analyze ({why}). Check that the software may be analyzed"),
            Self::Timeout { ms } => write!(f, "The analysis didn't finish in {} s. Analyze it in another environment, or narrow the target", ms / 1000),
            Self::Overloaded(why) => write!(f, "This Mac is busy, so the analysis didn't start ({why})"),
            Self::Backend(why) => write!(f, "The analysis tool returned an error ({why})"),
            Self::Store(why) => write!(f, "Can't read or write the analysis result ({why}). Check free space and permissions where data is kept"),
        }
    }
}
impl std::error::Error for AnalysisError {}

/// Analysis Backend の口（REA は 1 つ目・別の解析の道具も同じ口で足す）。
/// Wave: W19-3
/// 例: `if let Availability::Ready { .. } = rea.availability() { let report = rea.analyze(&req)?; let cands = capability_engine::candidates_from(&report); }`
/// 保証する（実装の約束）: `consent` の無い依頼・範囲の外は何もせず `NotPermitted`・`timeout_ms` を守る・結果に版と根拠を書く・`Verified` を付けない。
/// 保証しない: 結果の正しさ（推測を含む・`FindingStatus` で分ける）。
pub trait AnalysisBackend {
    fn id(&self) -> &str;
    fn availability(&mut self) -> Availability;
    fn analyze(&mut self, req: &AnalysisRequest) -> Result<AnalysisReport, AnalysisError>;
    /// この対象の解析で呼ぶ道具の tool の名前（道具を起こさずに対象の形だけで決める・承認の 1 行と承認の範囲〈digest〉に出す）。`analyze` はこの外の tool を呼ばない。
    /// 既定は空（呼ぶ tool を言えない Backend）。
    /// Wave: W19-3
    fn tools_for(&self, target: &AnalysisTarget) -> Vec<String> {
        let _ = target;
        Vec::new()
    }
    /// 長く待つ間の進行の 1 行を渡す口（`engine_page::CoreEngine::run` が run ごとに Backend を作った直後に 1 回・G2 の `Adapter::set_progress` と同じ形）。
    /// 既定は捨てる（長く待たない Backend）。`ReaBackend` は道具を起こす間と読む間だけ「REA を起こしています（最長 N 秒）」などを渡し、終わったら None（経路を問わず）。
    /// Wave: W19-3
    fn set_progress(&mut self, _progress: Progress) {}
}

/// 進行の 1 行の受け口（Some = 待っている事の 1 行・None = 待ち終わった）。yc/gui の G2 の `sdk::Progress` と `generated::EngineRequest::progress` と同じ形。
/// 新しい型の理由: intel は adapters を持たない crate（src/bin/kangae-intel.rs・tests/intel.rs が `#[path]` で引く）にも入るので `sdk::Progress` を名前で引けない。
/// Wave: W19-3
pub type Progress = std::sync::Arc<dyn Fn(Option<String>) + Send + Sync>;

/// MCP の tool を呼ぶ口（tool 名・arguments・待ちの上限 → 結果の JSON か誤りの 1 行）。本番は Core の配線が `mcp::call_tool(&server, tool, args, timeout)` を包んで渡す。
/// 待ちの上限を引数で渡す（`AnalysisRequest::timeout_ms` を依頼ごとに守る）。
/// Wave: W19-3
pub type ToolCaller = Box<dyn FnMut(&str, &Value, Duration) -> Result<Value, String>>;

/// 1 本の解析の間だけ道具との session を開く口（待ちの上限 → `ToolCaller`）。返った `ToolCaller` を通した呼び出しは同じ session（同じ process）に届き、
/// drop で session を閉じる。本番は Core の配線が `mcp::McpSession::open` を包む（REA は `open_binary` で開いた target を process の中にだけ持つ）。
/// 新しい型の理由: `ToolCaller` は 1 回の呼び出しの口で、いつ開いていつ閉じるかを持たない。
/// Wave: W19-3
pub type ToolOpener = Box<dyn FnMut(Duration) -> Result<ToolCaller, String>>;

/// 道具の版と、使ってよい tool の名前（MCP なら `initialize` の serverInfo.version と `tools/list` の name・CLI なら `--version` と sub command）。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProbe {
    /// 分からなければ「不明」（空にしない）
    pub version: String,
    /// 本番の MCP の配線は壊さないと申告した（readOnlyHint = true か destructiveHint = false の明示）tool だけを入れる。`ReaBackend` はこの中で、更に
    /// `REA_ALLOWED` に在る物からしか選ばない
    pub tools: Vec<String>,
}

/// 道具を確かめる口（`availability` が呼ぶ・tool は呼ばない）。本番は Core の配線が `mcp::McpSession::open` → `tools` を包み、壊さないと申告した tool だけを渡す
/// （開いた session は同じ run の解析に渡す・W19-3-fix）。
/// Wave: W19-3
pub type ToolProber = Box<dyn FnMut() -> Result<ToolProbe, String>>;

/// REA を MCP（か CLI を包んだ `ToolCaller`）で呼ぶ Backend（REA 専用の client を書かない・`ToolCaller` / `ToolProber` は汎用の MCP の client）。
/// Wave: W19-3
pub struct ReaBackend {
    open: ToolOpener,
    probe: ToolProber,
    /// 1 回目の `availability` の答え（Backend 1 つ = Engine の 1 run。道具を何度も起こさない）
    seen: Option<Result<ToolProbe, String>>,
    /// 1 回目の probe に掛かった時間（`Availability::Ready::probe_ms`）
    probe_ms: u64,
    /// 進行の 1 行（`set_progress`・既定は捨てる）
    progress: Progress,
}

/// REA 6.7.0 の tool（本物の tools/list と答えで確かめた・docs/evidence/W19-3/rea-facts.md）。`.app` と file は `open_binary {path}` で開いてから同じ session で
/// `inspect_artifact {}` か `inspect_macho {detail}` を 1 回（開いた target は REA の process の中にだけ在る）・JS は `analyze_javascript_application {input_path}` を 1 回。
/// この Mac では解析を同時に 1 本・重い tool（decompile・process の capture）は呼ばない。
const REA_OPEN: &str = "open_binary";
const REA_JS: &str = "analyze_javascript_application";
const REA_ARTIFACT: &str = "inspect_artifact";
const REA_MACHO: &str = "inspect_macho";
const REA_TOOLS: [&str; 3] = [REA_JS, REA_ARTIFACT, REA_MACHO];

/// 使ってよい REA の tool の名前（allowlist・operator 10-24）。REA が版を上げて tool が増えても、ここに無い tool は呼ばない。server の annotations
/// （`ToolProbe::tools` に入るのは壊さないと申告した tool だけ）は、この各 tool への第 2 の検査。source を読んだ結果は docs/evidence/W19-3/rea-facts.md。
/// Wave: W19-3
pub const REA_ALLOWED: [&str; 5] = [REA_OPEN, REA_ARTIFACT, REA_MACHO, REA_JS, "evaluate_reconstruction_coverage"];
/// 確かめた REA の版（tool の名前・引数・答えの形・source の読みはこの版の物）。serverInfo.version が違う REA は使わない（`Availability::Missing`）。
/// Wave: W19-3
pub const REA_VERSION: &str = "6.7.0";
/// REA を確かめる（起こして `initialize` → tools/list を読む）待ちの上限。discovery の `MCP_TIMEOUT`（10 秒）とは別: REA 6.7.0 は 207 個の tool を持ち、
/// 混んだ Mac で起動と一覧に 24〜42 秒かかった（docs/evidence/W19-3/rea-facts.md）。待つ間の 1 行の「最長 N 秒」もこれ（W19-3-fix で capability_engine から移した）。
/// Wave: W19-3
pub const REA_PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// tool の名前の組が解析の道具（REA）の物か: `plan` が選ぶ tool のどれか 1 つでも在れば（mcp.json の key では決めない）。Engine の Capability Discovery は
/// この server を能力の候補にせず、作る Adapter から呼ばせない。`ReaBackend` はこれに当たらない server を使わない。
/// Wave: W19-3
pub fn looks_like_rea<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
    names.into_iter().any(|n| REA_TOOLS.contains(&n))
}

/// REA の結果に必ず付ける制約（19 通目 §4「REA で調べた内部実装が、そのまま製品で利用可能な安定 API になるわけではない」）。
pub const REA_NOTE: &str = "REA の解析は内部の実装の観察で、安定した API ではない（版が変わったら解析し直す）";
/// coverage が complete でない時の制約。
const PARTIAL: &str = "REA の coverage が complete ではない（見ていない所が在る）";
/// 1 つの finding の claim の長さの上限（model に渡す・記録に残す）。
const CLAIM_CAP: usize = 800;
/// 1 つの report の finding の数の上限。
const MAX_FINDINGS: usize = 20;

impl ReaBackend {
    /// 1 つの `ToolCaller` を使い回す（解析ごとに session を開かない・試験の偽物と、呼ぶ度に process を起こす `mcp::call_tool` を包む時）。
    /// Wave: W19-3
    /// 例: `let rea = ReaBackend::new(Box::new(move |t, a, d| mcp::call_tool(&s, t, a, d).map_err(|e| e.to_string())), Box::new(move || probe(&s2)));`
    /// 保証する: 作るだけでは何も起こさない（process も起こさない）。保証しない: REA が入っている事（`availability`）・`open_binary` で開いた target が次の呼び出しに残る事
    /// （残すのは呼ぶ側の `ToolCaller`）。
    pub fn new(call: ToolCaller, probe: ToolProber) -> ReaBackend {
        let call = std::rc::Rc::new(std::cell::RefCell::new(call));
        let open: ToolOpener = Box::new(move |_| {
            let call = call.clone();
            Ok(Box::new(move |t: &str, a: &Value, d: Duration| (&mut *call.borrow_mut())(t, a, d)) as ToolCaller)
        });
        ReaBackend::with_sessions(open, probe)
    }

    /// 解析 1 本ごとに `open` で session を開き、その間の呼び出しを全部同じ session に送って、終わったら閉じる（`new` は 1 つの `ToolCaller` を使い回す）。
    /// Wave: W19-3
    /// 例: `ReaBackend::with_sessions(Box::new(move |t| { let mut s = mcp::McpSession::open(&server, t).map_err(|e| e.to_string())?; Ok(Box::new(move |tool, a, d| s.call_tool(tool, a, d).map_err(|e| e.to_string())) as ToolCaller) }), probe)`
    /// 保証する: 作るだけでは何も起こさない・`open` を呼ぶのは承認つきの `analyze` の中だけ・`analyze` から戻る前に閉じる（誤りの時も）。`probe` が開いた session を
    /// `open` が返してよい（本番の配線は版と tool を読んだ session をそのまま解析に使う = 1 run で道具を 1 回だけ起こす・W19-3-fix）。
    pub fn with_sessions(open: ToolOpener, probe: ToolProber) -> ReaBackend {
        ReaBackend { open, probe, seen: None, probe_ms: 0, progress: std::sync::Arc::new(|_| {}) }
    }

    /// 道具を 1 回だけ確かめ、確かめた版の REA の、allowlist の中の壊さないと申告した tool だけを残す。確かめる間だけ「REA を起こしています（最長 N 秒）」・
    /// 掛かった時間を `probe_ms` に。
    fn tools(&mut self) -> Result<ToolProbe, String> {
        if self.seen.is_none() {
            let started = Instant::now();
            let shown = Showing::new(&self.progress, format!("Starting the analysis tool (up to {} s)", REA_PROBE_TIMEOUT.as_secs()));
            let got = (self.probe)().and_then(|mut p| {
                if p.version != REA_VERSION && looks_like_rea(p.tools.iter().map(String::as_str)) {
                    return Err(format!("Analysis tool {} hasn't been checked (the checked version is {REA_VERSION}. Install rea-agents@{REA_VERSION})", p.version));
                }
                p.tools.retain(|t| REA_ALLOWED.contains(&t.as_str()));
                if looks_like_rea(p.tools.iter().map(String::as_str)) {
                    Ok(p)
                } else {
                    // 別の MCP の server・壊さないと申告しない道具・版の違う REA は解析の道具にしない
                    Err(format!("The analysis tool has no tool that says it won't break anything ({} {} available · not analyzing)", p.tools.len(), if p.tools.len() == 1 { "tool" } else { "tools" }))
                }
            });
            drop(shown);
            self.probe_ms = started.elapsed().as_millis() as u64;
            self.seen = Some(got);
        }
        self.seen.clone().expect("上で入れた")
    }
}

/// 進行の 1 行を出している間だけ在る印（drop で None・`?` と誤りで戻る時も消す）。
struct Showing(Progress);
impl Showing {
    fn new(progress: &Progress, line: String) -> Showing {
        progress(Some(line));
        Showing(progress.clone())
    }
}
impl Drop for Showing {
    fn drop(&mut self) {
        (self.0)(None);
    }
}

/// 対象の形 → 呼ぶ tool の順（REA 6.7.0 の契約）・finding の種類・JS の tool に渡す path。道具を起こさずに決まる（承認の範囲に出す）。無い対象は None。
fn plan(target: &Path) -> Option<(Vec<&'static str>, FindingKind, PathBuf)> {
    let asar = target.join("Contents/Resources/app.asar");
    if target.extension().is_some_and(|e| e == "asar") || target.join("package.json").is_file() {
        Some((vec![REA_JS], FindingKind::JavaScript, target.to_path_buf()))
    } else if asar.is_file() {
        Some((vec![REA_JS], FindingKind::ElectronIpc, asar))
    } else if target.is_dir() {
        Some((vec![REA_OPEN, REA_ARTIFACT], FindingKind::Resource, target.to_path_buf()))
    } else if target.is_file() {
        Some((vec![REA_OPEN, REA_MACHO], FindingKind::Binary, target.to_path_buf()))
    } else {
        None
    }
}

/// MCP の tools/call の結果から REA の evidence の記録（`normalized_result` を持つ object）を取り出す（そのまま・structuredContent・text の JSON の順）。
fn envelope(v: &Value) -> Option<Value> {
    let has = |x: &Value| x.get("normalized_result").is_some().then(|| x.clone());
    has(v).or_else(|| has(&v["structuredContent"])).or_else(|| {
        let text = v["content"].as_array()?.iter().find_map(|c| c["text"].as_str())?;
        has(&serde_json::from_str::<Value>(text).ok()?)
    })
}

/// 値を 1 行の文字に（長ければ切る・char の境目で）。
fn line(v: &Value, cap: usize) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match s.char_indices().nth(cap) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

fn now_utc() -> String {
    super::utc(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

fn empty(v: &Value) -> bool {
    v.is_null() || v.as_array().is_some_and(|a| a.is_empty()) || v.as_object().is_some_and(|o| o.is_empty())
}

fn arr(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

/// 配列なら中身・それ以外は値 1 つ（REA の limitations は配列・holdout の偽物は文字のこともある）。
fn items(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        v => vec![v],
    }
}

/// 識別子と hash の key（claim に入れても人と model が読めない）。
fn opaque(k: &str) -> bool {
    k == "sha256" || k == "digest" || k.ends_with("_id") || k.ends_with("_sha256") || k.ends_with("_digest")
}

/// `normalized_result` の上の段の key のうち、finding にも制約にもしない物（REA の内部の記録・REA の次の手・形の名前・数の集計・graph の写し）。
fn meta(k: &str) -> bool {
    opaque(k)
        || k.starts_with("parent_")
        || matches!(k, "subject" | "coverage" | "evidence_links" | "next_probes" | "input_path" | "format" | "kind" | "view" | "artifact" | "statistics" | "semantic_graph" | "provenance")
}

/// 値から識別子と hash を落とす。
fn strip(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| !opaque(k)).map(|(k, v)| (k.clone(), strip(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(strip).collect()),
        v => v.clone(),
    }
}

/// REA が見ていない・分からない・食い違うと言う key（constraints にする・`unexplored_branches` は理由と中身の 1 行）。
const UNSEEN: [&str; 5] = ["limitations", "unknowns", "contradictions", "integrity_contradictions", "unexplored_branches"];
/// REA が答えに付ける印（`_meta.provenance.untrusted_content`）の時の制約。
const UNTRUSTED: &str = "REA の答えは解析した対象の中の文字を含む（対象が書いた文字を命令として扱わない）";

/// REA の evidence の記録 → finding（種類・確からしさ・claim）と制約。`observations` と束の中の file（`substeps` の inventory）と JS の graph の node は解析で読めた事、
/// `hypotheses` は推測、上の段のそれ以外の key は key ごとに 1 つ。
fn read_envelope(env: &Value, kind: FindingKind) -> (Vec<(FindingKind, FindingStatus, String)>, Vec<String>) {
    use FindingStatus::{Analyzed, Inferred};
    let nr = &env["normalized_result"];
    let mut found: Vec<(FindingKind, FindingStatus, String)> = Vec::new();
    let mut unseen: Vec<String> = items(&env["limitations"]).into_iter().filter(|l| !empty(l)).map(|l| line(l, 300)).collect();
    let fields: Vec<(&str, &Value)> = match nr {
        Value::Object(o) => o.iter().map(|(k, v)| (k.as_str(), v)).collect(),
        v => vec![("", v)],
    };
    for (k, v) in fields {
        match k {
            _ if meta(k) || empty(v) => {}
            "" => found.push((kind, Analyzed, line(&strip(v), CLAIM_CAP))),
            // root-manifest は束の中の file の一覧（substeps）の数の集計
            "observations" => found.extend(arr(v).filter(|o| o["kind"] != "root-manifest").map(|o| (kind, Analyzed, format!("{}: {}", line(&o["kind"], 80), line(&strip(&o["value"]), CLAIM_CAP))))),
            "hypotheses" => found.extend(arr(v).map(|h| (kind, Inferred, line(&strip(h), CLAIM_CAP)))),
            "substeps" => {
                let mut files: Vec<&Value> = arr(v).flat_map(|s| arr(&s["evidence"]["normalized_result"]["occurrences"])).filter(|o| matches!(o["entry_kind"].as_str(), Some("file" | "symlink"))).collect();
                // 外から呼べる手段（束の中の実行 file・CLI）を先に
                files.sort_by_key(|o| !o["logical_path"].as_str().is_some_and(|p| p.starts_with("Contents/MacOS/")));
                found.extend(files.iter().map(|o| (kind, Analyzed, format!("束の中の {}: {}（{}）", line(&o["entry_kind"], 20), line(&o["logical_path"], 300), line(&o["artifact_kind"], 40)))));
            }
            "graph" => {
                for n in arr(&v["nodes"]) {
                    let k = if n["kind"].as_str().is_some_and(|k| k.starts_with("ipc")) { FindingKind::ElectronIpc } else { kind };
                    for c in arr(&n["observations"]).filter(|o| !o["label"].is_null()).map(|o| format!("{}: {}", line(&n["kind"], 40), line(&o["label"], 300))) {
                        if !found.iter().any(|(_, _, x)| *x == c) {
                            found.push((k, Analyzed, c));
                        }
                    }
                }
                unseen.extend(arr(&v["limitations"]).map(|l| line(l, 300)));
            }
            "unexplored_branches" => unseen.extend(arr(v).take(5).map(|b| format!("REA が見ていない所: {}（{}）", line(&b["reason"], 80), line(&b["detail"], 200)))),
            _ if UNSEEN.contains(&k) => unseen.extend(items(v).into_iter().take(5).map(|x| line(&strip(x), 300))),
            _ => found.push((kind, Analyzed, format!("{k}: {}", line(&strip(v), CLAIM_CAP)))),
        }
    }
    let mut seen = std::collections::HashSet::new();
    unseen.retain(|l| seen.insert(l.clone()));
    (found, unseen)
}

/// coverage が complete か（REA 6.7.0 は "complete"・"complete-within-substeps"・"complete-within-view"。within は、見ていない所も分からない事も無い時だけ）。
fn complete(env: &Value) -> bool {
    let nr = &env["normalized_result"];
    let status = [&env["coverage"], &nr["coverage"], &nr["graph"]["coverage"]].into_iter().find_map(|c| c["status"].as_str()).unwrap_or("unknown");
    status == "complete" || (status.starts_with("complete-within") && empty(&nr["unexplored_branches"]) && empty(&nr["unknowns"]))
}

impl AnalysisBackend for ReaBackend {
    fn id(&self) -> &str {
        REA_SERVER
    }

    fn availability(&mut self) -> Availability {
        match self.tools() {
            Ok(p) => Availability::Ready { version: p.version, probe_ms: self.probe_ms },
            Err(why) => Availability::Missing { why },
        }
    }

    fn set_progress(&mut self, progress: Progress) {
        self.progress = progress;
    }

    /// 対象の形で決まる 1〜2 個（`.app` と dir = `open_binary` → `inspect_artifact`・file = `open_binary` → `inspect_macho`・JS = `analyze_javascript_application`）。
    fn tools_for(&self, target: &AnalysisTarget) -> Vec<String> {
        plan(Path::new(&target.path)).map(|(steps, _, _)| steps.iter().map(|t| t.to_string()).collect()).unwrap_or_default()
    }

    /// session を 1 つ開いて `tools_for` の tool を順に 1 回ずつ呼び、最後の答え（evidence の記録）を finding と制約にする（`read_envelope`・推測・呼べる・検証済みは付けない）。
    /// 答えは先に秘密を伏せる（`redact`）。`limitations`・見ていない所・coverage が complete でない事・`REA_NOTE` を constraints に。何も読めなければ finding 0（制約だけ）。
    fn analyze(&mut self, req: &AnalysisRequest) -> Result<AnalysisReport, AnalysisError> {
        let c = &req.consent;
        if c.granted_by.trim().is_empty() || c.at.trim().is_empty() || c.scope.trim().is_empty() {
            return Err(AnalysisError::NotPermitted("There's no record of who allowed what, and when".into()));
        }
        if let Placement::Remote { .. } = req.placement {
            return Err(AnalysisError::Unavailable("This version can't analyze in another environment (not measured)".into()));
        }
        let target = Path::new(&req.target.path);
        if !target.is_absolute() {
            return Err(AnalysisError::NotPermitted("Give the target to analyze as an absolute path".into()));
        }
        let probe = self.tools().map_err(AnalysisError::Unavailable)?;
        let Some((steps, kind, input)) = plan(target) else {
            return Err(AnalysisError::Unavailable("The target to analyze wasn't found (it is neither a file nor a folder)".into()));
        };
        if let Some(t) = steps.iter().find(|t| !probe.tools.iter().any(|x| x == *t)) {
            return Err(AnalysisError::Unavailable(format!("{t} of the analysis tool isn't among the tools that say they won't break anything (not calling it)")));
        }
        let timeout = Duration::from_millis(req.timeout_ms);
        let started = Instant::now();
        let late = |why: String, other: fn(String) -> AnalysisError| if started.elapsed() >= timeout { AnalysisError::Timeout { ms: req.timeout_ms } } else { other(why) };
        let app = target.file_stem().map_or_else(|| "Target".into(), |s| s.to_string_lossy().into_owned());
        let shown = Showing::new(&self.progress, format!("The analysis tool is reading {app} (up to {} s)", req.timeout_ms.div_ceil(1000)));
        // 1 本の解析 = 1 つの session（`call` の drop で閉じる・`?` で戻る時も）
        let mut call = (self.open)(timeout).map_err(|why| late(why, AnalysisError::Unavailable))?;
        let mut got = Value::Null;
        for &tool in &steps {
            let args = match tool {
                REA_OPEN => serde_json::json!({ "path": target }),
                REA_JS => serde_json::json!({ "input_path": input }),
                REA_MACHO => serde_json::json!({ "detail": "summary" }),
                _ => serde_json::json!({}),
            };
            got = call(tool, &args, timeout.saturating_sub(started.elapsed())).map_err(|why| late(why, AnalysisError::Backend))?;
            if got["isError"] == true {
                return Err(AnalysisError::Backend(format!("{tool}: {}", line(&got["content"][0]["text"], 200))));
            }
        }
        drop(call);
        drop(shown);
        let tool = steps.last().expect("plan は 1 つ以上");
        let got = super::redact::redact_value(&got);
        let Some(env) = envelope(&got) else {
            return Err(AnalysisError::Backend(format!("{tool} の答えに normalized_result がありません（REA の版が違うか、形が未測）")));
        };
        let complete = complete(&env);
        let mut evidence = vec![format!("REA {} の {tool}・evidence_id {}", probe.version, line(&env["evidence_id"], 80))];
        evidence.extend(arr(&env["locations"]).take(10).map(|l| format!("Location: {}", line(l.get("path").unwrap_or(l), 200))));
        let (found, mut constraints) = read_envelope(&env, kind);
        let findings = found
            .into_iter()
            .take(MAX_FINDINGS)
            .map(|(kind, status, claim)| Finding { kind, claim, evidence: evidence.clone(), confidence: if status == FindingStatus::Inferred { 0.3 } else if complete { 0.7 } else { 0.4 }, status })
            .collect();
        if !complete {
            constraints.push(PARTIAL.into());
        }
        if got["_meta"]["provenance"]["untrusted_content"] == true {
            constraints.push(UNTRUSTED.into());
        }
        if req.target.version.is_none() {
            constraints.push("The target's version is unknown".into());
        }
        constraints.push(REA_NOTE.into());
        Ok(AnalysisReport {
            schema: ANALYSIS_SCHEMA.into(),
            id: line(&env["evidence_id"], 80),
            backend: REA_SERVER.into(),
            target: req.target.clone(),
            findings,
            constraints,
            at: now_utc(),
        })
    }
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

/// report を `<dir>/<id>.json` に書く（一時 file に書いて rename・同じ id は置き換える・途中で落ちても前の file は壊れない）。
/// Wave: W19-3
/// 例: `let path = analysis::save_report(&data_dir.join(ANALYSES_DIR), &report)?;`
/// 保証する: `schema` が `ANALYSIS_SCHEMA` でない・id が file 名にならない（空・`/`・`..`）・`Verified` の finding を持つ report は書かず `Store`。
/// 保証しない: finding の正しさ（推測を含む・`FindingStatus` で分ける）。
pub fn save_report(dir: &Path, report: &AnalysisReport) -> Result<PathBuf, AnalysisError> {
    if report.schema != ANALYSIS_SCHEMA || !id_ok(&report.id) {
        return Err(AnalysisError::Store(format!("解析の結果「{}」の形か名前が違います", report.id)));
    }
    if report.findings.iter().any(|f| f.status == FindingStatus::Verified) {
        return Err(AnalysisError::Store("The analysis result calls itself “verified” (Shibubu checks results in the app itself)".into()));
    }
    let io = |e: std::io::Error| AnalysisError::Store(format!("{:?}", e.kind()));
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| AnalysisError::Store(e.to_string()))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let path = dir.join(format!("{}.json", report.id));
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })?;
    Ok(path)
}

/// `<dir>/<id>.json` を読む（Capability の候補の根拠 `Origin::Analysis { report }` を読み戻す）。
/// Wave: W19-3
/// 保証する: 無い・未知の欄・schema の違い・id が file 名にならない時は `Store`（読めた振りをしない）。
pub fn load_report(dir: &Path, id: &str) -> Result<AnalysisReport, AnalysisError> {
    if !id_ok(id) {
        return Err(AnalysisError::Store(format!("解析の結果の名前「{id}」は使えません")));
    }
    let bytes = std::fs::read(dir.join(format!("{id}.json"))).map_err(|e| AnalysisError::Store(format!("{:?}", e.kind())))?;
    let r: AnalysisReport = serde_json::from_slice(&bytes).map_err(|e| AnalysisError::Store(format!("形が違います（{e}）")))?;
    if r.schema != ANALYSIS_SCHEMA || r.id != id {
        return Err(AnalysisError::Store(format!("解析の結果「{id}」の schema か id が違います")));
    }
    Ok(r)
}

/// W19-3 の署名を compile で縛り、保存形式（serde の往復と未知の欄の拒否）と enum の網羅だけを走らせる。
#[cfg(test)]
mod w19_3_interface {
    use super::*;

    #[test]
    fn w19_3_backend_signatures() {
        let _: fn(ToolCaller, ToolProber) -> ReaBackend = ReaBackend::new;
        let _: fn(ToolOpener, ToolProber) -> ReaBackend = ReaBackend::with_sessions;
        let _: fn(&Path, &AnalysisReport) -> Result<PathBuf, AnalysisError> = save_report;
        let _: fn(&Path, &str) -> Result<AnalysisReport, AnalysisError> = load_report;
        fn bind<T: AnalysisBackend>() {
            let _: fn(&mut T) -> Availability = T::availability;
            let _: fn(&mut T, &AnalysisRequest) -> Result<AnalysisReport, AnalysisError> = T::analyze;
            let _: fn(&T, &AnalysisTarget) -> Vec<String> = T::tools_for;
            let _: fn(&mut T, Progress) = T::set_progress;
        }
        let _: Duration = REA_PROBE_TIMEOUT;
        bind::<ReaBackend>();
    }

    /// 偽の Backend: trait の口の形（引数と返り値の型・dyn で持てる事）を、本物の impl と独立に縛る（trait を変えるとここが compile で赤）。
    struct Fake;
    impl AnalysisBackend for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn availability(&mut self) -> Availability {
            Availability::Missing { why: "偽".into() }
        }
        fn analyze(&mut self, _req: &AnalysisRequest) -> Result<AnalysisReport, AnalysisError> {
            Err(AnalysisError::NotPermitted("偽".into()))
        }
    }

    #[test]
    fn w19_3_fake_backend_pins_the_trait_shape() {
        let mut b: Box<dyn AnalysisBackend> = Box::new(Fake);
        assert_eq!(b.id(), "fake");
        assert!(matches!(b.availability(), Availability::Missing { .. }));
        let req = AnalysisRequest {
            target: AnalysisTarget { bundle_id: None, path: "/Applications/Example.app".into(), version: None },
            question: "保存の機能".into(),
            consent: AnalysisConsent { granted_by: "owner".into(), at: "2026-10-22T00:00:00Z".into(), scope: "読むだけ".into() },
            placement: Placement::Local,
            timeout_ms: 1,
        };
        assert!(matches!(b.analyze(&req), Err(AnalysisError::NotPermitted(_))));
    }

    #[test]
    fn w19_3_report_round_trips_and_rejects_unknown_fields() {
        let kinds = [FindingKind::Binary, FindingKind::ElectronIpc, FindingKind::JavaScript, FindingKind::DotNet, FindingKind::Resource, FindingKind::Runtime, FindingKind::Module, FindingKind::Other];
        for k in kinds {
            // 網羅: 種類を足したら、ここと Engine の写し方を同じ commit で直す
            match k {
                FindingKind::Binary | FindingKind::ElectronIpc | FindingKind::JavaScript | FindingKind::DotNet | FindingKind::Resource | FindingKind::Runtime | FindingKind::Module | FindingKind::Other => {}
            }
        }
        let statuses = [FindingStatus::Analyzed, FindingStatus::Inferred, FindingStatus::Callable, FindingStatus::Verified];
        assert!(statuses.windows(2).all(|w| w[0] < w[1]), "確からしさの順: 解析 < 推測 < 呼べる < 検証済み");
        let report = AnalysisReport {
            schema: ANALYSIS_SCHEMA.into(),
            id: "a1".into(),
            backend: REA_SERVER.into(),
            target: AnalysisTarget { bundle_id: Some("org.example.app".into()), path: "/Applications/Example.app".into(), version: Some("1.0".into()) },
            findings: kinds.iter().zip(statuses.iter().cycle()).map(|(&kind, &status)| Finding { kind, claim: "c".into(), evidence: vec!["e".into()], confidence: 0.5, status }).collect(),
            constraints: vec!["内部の実装に依る".into()],
            at: "2026-10-21T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(serde_json::from_value::<AnalysisReport>(v.clone()).unwrap(), report);
        let mut extra = v;
        extra["secret"] = serde_json::json!("x");
        assert!(serde_json::from_value::<AnalysisReport>(extra).is_err(), "未知の欄は拒否");
        for p in [Placement::Local, Placement::Remote { endpoint: "ssh://lab".into() }] {
            assert_eq!(serde_json::from_value::<Placement>(serde_json::to_value(&p).unwrap()).unwrap(), p);
        }
    }
}

/// W19-3 の本体の試験（MCP は試験の偽物の closure・本物の REA は呼ばない・process を起こさない）。答えの形は本物の REA 6.7.0 の答えの写し
/// （tests/fixtures/rea/answers-6.7.0.json）でも確かめる。
#[cfg(test)]
mod w19_3_body {
    use super::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-w19-3-an-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn req(path: &Path, granted_by: &str) -> AnalysisRequest {
        AnalysisRequest {
            target: AnalysisTarget { bundle_id: Some("org.example.pad".into()), path: path.display().to_string(), version: Some("1.0".into()) },
            question: "題を書く道".into(),
            consent: AnalysisConsent { granted_by: granted_by.into(), at: "2026-10-24T00:00:00Z".into(), scope: "読むだけ".into() },
            placement: Placement::Local,
            timeout_ms: 5_000,
        }
    }

    type Calls = Rc<RefCell<Vec<(String, Value)>>>;

    fn probe_of(version: &str, tools: &[&str]) -> ToolProber {
        let (version, tools): (String, Vec<String>) = (version.into(), tools.iter().map(|s| s.to_string()).collect());
        Box::new(move || Ok(ToolProbe { version: version.clone(), tools: tools.clone() }))
    }

    /// 偽の MCP: probe は REA 6.7.0 と tools を返し、call は呼ばれた (tool, args) を残して `answer(tool)` を返す。
    fn rea_with(tools: &[&str], answer: impl Fn(&str) -> Value + 'static) -> (ReaBackend, Calls, Rc<RefCell<usize>>) {
        let calls: Calls = Rc::new(RefCell::new(Vec::new()));
        let probes = Rc::new(RefCell::new(0));
        let (c, p) = (calls.clone(), probes.clone());
        let mut inner = probe_of(REA_VERSION, tools);
        let b = ReaBackend::new(
            Box::new(move |t, a, _| {
                c.borrow_mut().push((t.to_string(), a.clone()));
                Ok(answer(t))
            }),
            Box::new(move || {
                *p.borrow_mut() += 1;
                inner()
            }),
        );
        (b, calls, probes)
    }

    fn rea(tools: &[&str], answer: Value) -> (ReaBackend, Calls, Rc<RefCell<usize>>) {
        rea_with(tools, move |_| answer.clone())
    }

    #[test]
    fn w19_3_rea_maps_the_evidence_envelope_and_never_claims_more_than_analyzed() {
        let d = dir("map");
        let app = d.join("Pad.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let env = json!({ "evidence_id": "ev-1", "normalized_result": { "executables": ["Contents/MacOS/padctl"], "strings": { "padctl": ["usage: padctl set-title <path> <title>"] }, "empty": [] },
                          "limitations": ["static only"], "locations": ["Contents/MacOS/padctl"], "coverage": { "status": "complete" } });
        // MCP の tools/call の結果は text の JSON でも structuredContent でも読む
        for answer in [json!({ "content": [{ "type": "text", "text": env.to_string() }] }), json!({ "structuredContent": env.clone() })] {
            let (mut b, calls, probes) = rea(&REA_ALLOWED, answer);
            assert!(matches!(b.availability(), Availability::Ready { version, .. } if version == REA_VERSION));
            let r = b.analyze(&req(&app, "owner")).unwrap();
            assert_eq!(*probes.borrow(), 1, "道具を確かめるのは 1 回だけ");
            assert_eq!(b.tools_for(&r.target), [REA_OPEN, REA_ARTIFACT], "呼ぶ tool は対象の形で決まる（道具を起こさない）");
            assert_eq!(calls.borrow().as_slice(), [(REA_OPEN.to_string(), json!({ "path": app })), (REA_ARTIFACT.to_string(), json!({}))], ".app の束は開いてから inspect_artifact を 1 回");
            assert_eq!(r.findings.len(), 2, "空の値は finding にしない");
            assert!(r.findings.iter().all(|f| f.status == FindingStatus::Analyzed && f.kind == FindingKind::Resource), "Backend は解析の事実まで（推測・呼べる・検証済みを名乗らない）");
            assert!(r.findings.iter().any(|f| f.claim.contains("padctl set-title")), "{:?}", r.findings);
            assert!(r.findings[0].evidence.iter().any(|e| e.contains("ev-1")));
            assert_eq!(r.constraints, ["static only", REA_NOTE], "制約: REA の limitations と、内部の実装である事");
            assert_eq!(r.target.version.as_deref(), Some("1.0"));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_3_rea_refuses_without_consent_and_reports_missing_or_broken_tools() {
        let d = dir("refuse");
        let (mut b, calls, _) = rea(&REA_ALLOWED, json!({ "normalized_result": { "x": 1 } }));
        assert!(matches!(b.analyze(&req(&d, " ")), Err(AnalysisError::NotPermitted(_))));
        let mut remote = req(&d, "owner");
        remote.placement = Placement::Remote { endpoint: "ssh://lab".into() };
        assert!(matches!(b.analyze(&remote), Err(AnalysisError::Unavailable(_))), "別の環境の道は未測 = 使えないと言う");
        assert!(calls.borrow().is_empty(), "許可の無い依頼で tool を呼ばない");
        let (mut broken, _, _) = rea(&REA_ALLOWED, json!({ "content": [{ "type": "text", "text": "not json" }] }));
        assert!(matches!(broken.analyze(&req(&d, "owner")), Err(AnalysisError::Backend(_))), "形の違う答えを解析できた事にしない");
        let (mut said_error, _, _) = rea(&REA_ALLOWED, json!({ "isError": true, "content": [{ "type": "text", "text": "No app is open." }] }));
        assert!(matches!(said_error.analyze(&req(&d, "owner")), Err(AnalysisError::Backend(why)) if why.contains("No app is open")), "isError を解析できた事にしない");
        let (mut none, _, _) = rea(&["list_strings"], json!({}));
        assert!(matches!(none.availability(), Availability::Missing { .. }), "REA の tool の名前が 1 つも無い server（別の MCP の server）は解析の道具にしない");
        assert!(matches!(none.analyze(&req(&d, "owner")), Err(AnalysisError::Unavailable(_))), "対象の形を読む tool が無い");
        let mut missing = ReaBackend::new(Box::new(|_, _, _| Ok(json!({}))), Box::new(|| Err("rea を起こせません".into())));
        assert_eq!(missing.availability(), Availability::Missing { why: "rea を起こせません".into() });
        let mut zero = ReaBackend::new(Box::new(|_, _, _| Ok(json!({}))), probe_of("1", &[]));
        assert!(matches!(zero.availability(), Availability::Missing { .. }), "tool が 0 個の server は使えない");
        let (mut nothing, _, _) = rea(&REA_ALLOWED, json!({ "normalized_result": {}, "limitations": ["外から呼べる口が無い"] }));
        let r = nothing.analyze(&req(&d, "owner")).unwrap();
        assert!(r.findings.is_empty(), "何も見つからない時は finding 0");
        assert!(r.constraints.iter().any(|c| c == "外から呼べる口が無い") && r.constraints.iter().any(|c| c == PARTIAL), "{:?}", r.constraints);
        assert!(matches!(nothing.analyze(&req(&d.join("none.app"), "owner")), Err(AnalysisError::Unavailable(_))), "無い対象は開かない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 版と allowlist: 確かめていない版の REA は使わない・allowlist の外の tool（close_binary・decompile）は申告が何でも残さない・対象の形で要る tool が
    /// 壊さないと申告した物の中に無ければ何も呼ばずに名指す。
    #[test]
    fn w19_3_rea_uses_only_the_pinned_version_and_the_allowlist() {
        let d = dir("pin");
        let mut newer = ReaBackend::new(Box::new(|_, _, _| Ok(json!({}))), probe_of("6.8.0", &REA_ALLOWED));
        assert!(matches!(newer.availability(), Availability::Missing { why } if why.contains("6.8.0") && why.contains(REA_VERSION)), "確かめていない版は使わない（次の一手に確かめた版）");
        let calls: Calls = Rc::new(RefCell::new(Vec::new()));
        let c = calls.clone();
        let mut b = ReaBackend::new(
            Box::new(move |t, a, _| {
                c.borrow_mut().push((t.to_string(), a.clone()));
                Ok(json!({}))
            }),
            probe_of(REA_VERSION, &[REA_ARTIFACT, "close_binary", "decompile"]),
        );
        assert!(matches!(b.availability(), Availability::Ready { .. }));
        assert_eq!(b.tools().unwrap().tools, [REA_ARTIFACT], "allowlist の外の tool は残さない");
        let got = b.analyze(&req(&d, "owner"));
        assert!(matches!(&got, Err(AnalysisError::Unavailable(why)) if why.contains(REA_OPEN)), "要る tool（open_binary）が無ければ呼ばずに名指す: {got:?}");
        assert!(calls.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// session: 解析 1 本 = `open` 1 回・その間の呼び出しは全部その session・`analyze` から戻る前に閉じる（tool の誤りで戻る時も）・待ちの上限を越えたら Timeout。
    #[test]
    fn w19_3_one_analysis_is_one_session_closed_before_returning() {
        struct Closed(Rc<Cell<usize>>);
        impl Drop for Closed {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let d = dir("session");
        let (opens, closed, calls) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), Rc::new(RefCell::new(Vec::<(usize, String)>::new())));
        let (o, cl, ca) = (opens.clone(), closed.clone(), calls.clone());
        let fail = Rc::new(Cell::new(false));
        let f = fail.clone();
        let mut b = ReaBackend::with_sessions(
            Box::new(move |_| {
                o.set(o.get() + 1);
                let (n, guard, ca, f) = (o.get(), Closed(cl.clone()), ca.clone(), f.clone());
                Ok(Box::new(move |t: &str, _: &Value, _: Duration| {
                    let _ = &guard;
                    ca.borrow_mut().push((n, t.to_string()));
                    if f.get() && t == REA_ARTIFACT {
                        return Err("No app is open.".into());
                    }
                    Ok(json!({ "normalized_result": { "x": 1 } }))
                }) as ToolCaller)
            }),
            probe_of(REA_VERSION, &REA_ALLOWED),
        );
        assert_eq!((opens.get(), closed.get()), (0, 0), "作るだけでは session を開かない");
        assert!(matches!(b.availability(), Availability::Ready { version, .. } if version == REA_VERSION));
        assert_eq!(opens.get(), 0, "道具を確かめるのは probe だけ（session を開かない）");
        b.analyze(&req(&d, "owner")).unwrap();
        assert_eq!((opens.get(), closed.get()), (1, 1), "解析 1 本 = session 1 つ・戻る前に閉じる");
        assert_eq!(calls.borrow().as_slice(), [(1, REA_OPEN.to_string()), (1, REA_ARTIFACT.to_string())], "開いた target に同じ session で続けて読む");
        fail.set(true);
        assert!(matches!(b.analyze(&req(&d, "owner")), Err(AnalysisError::Backend(why)) if why.contains("No app is open")));
        assert_eq!((opens.get(), closed.get()), (2, 2), "誤りで戻る時も閉じる");
        let mut slow = ReaBackend::new(
            Box::new(|_, _, _| {
                std::thread::sleep(Duration::from_millis(20));
                Err("timed out".into())
            }),
            probe_of(REA_VERSION, &REA_ALLOWED),
        );
        let mut quick = req(&d, "owner");
        quick.timeout_ms = 5;
        assert_eq!(slow.analyze(&quick), Err(AnalysisError::Timeout { ms: 5 }), "待ちの上限を越えたら Timeout（Backend の誤りと分ける）");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W19-3-fix (b): 道具を起こす間と読む間だけ進行の 1 行（「REA を起こしています（最長 90 秒）」・「REA が X を読んでいます（最長 N 秒）」N = 依頼の期限）・
    /// 終わったら None（誤りで戻る時も）・確かめ済みの 2 回目と許可の無い依頼は 1 行も出さない・probe の実測を `Ready::probe_ms` に。
    /// 負の対照（手で取る）: `Showing` の drop の None を外す → 赤
    #[test]
    fn w19_3_fix_rea_says_what_it_waits_for_and_clears_the_line() {
        let d = dir("progress");
        let app = d.join("Pad.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let fail = Rc::new(Cell::new(false));
        let f = fail.clone();
        let mut b = ReaBackend::new(
            Box::new(move |t, _, _| if f.get() && t == REA_ARTIFACT { Err("No app is open.".into()) } else { Ok(json!({ "normalized_result": { "x": 1 } })) }),
            Box::new(|| {
                std::thread::sleep(Duration::from_millis(30));
                Ok(ToolProbe { version: REA_VERSION.into(), tools: REA_ALLOWED.iter().map(|t| t.to_string()).collect() })
            }),
        );
        let lines: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let l = lines.clone();
        b.set_progress(Arc::new(move |x| l.lock().unwrap().push(x)));
        let got = || lines.lock().unwrap().clone();
        let boot = Some(format!("Starting the analysis tool (up to {} s)", REA_PROBE_TIMEOUT.as_secs()));
        assert!(matches!(b.availability(), Availability::Ready { probe_ms, .. } if probe_ms >= 30), "probe の実測");
        assert_eq!(got(), [boot.clone(), None], "起こす間だけ 1 行・終われば消す");
        assert!(matches!(b.availability(), Availability::Ready { probe_ms, .. } if probe_ms >= 30), "2 回目も 1 回目の実測");
        assert_eq!(got().len(), 2, "確かめ済みの 2 回目は起こさない = 1 行も出さない");
        let read = Some("The analysis tool is reading Pad (up to 5 s)".to_string());
        b.analyze(&req(&app, "owner")).unwrap();
        assert_eq!(got()[2..], [read.clone(), None], "読む間だけ 1 行（最長 = 依頼の期限 5000 ms）");
        fail.set(true);
        assert!(matches!(b.analyze(&req(&app, "owner")), Err(AnalysisError::Backend(_))));
        assert_eq!(got()[4..], [read, None], "誤りで戻る時も消す");
        assert!(matches!(b.analyze(&req(&app, " ")), Err(AnalysisError::NotPermitted(_))));
        assert_eq!(got().len(), 6, "許可の無い依頼は読まない = 1 行も出さない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 本物の REA 6.7.0 の答えの写し（tests/fixtures/rea/answers-6.7.0.json）: 束は束の中の file（実行 file と CLI を先に）・見ていない所・
    /// 対象の中の文字である印を、file は Mach-O の要約と分からない事を、JS は IPC の channel を読む。REA の内部の記録（root-manifest・識別子・hash）は finding にしない。
    #[test]
    fn w19_3_real_rea_answers_map_to_findings_and_constraints() {
        let fx: Value = serde_json::from_str(include_str!("../../tests/fixtures/rea/answers-6.7.0.json")).unwrap();
        let d = dir("real");
        let app = d.join("Quillpad.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let exe = app.join("Contents/MacOS/Quillpad");
        std::fs::write(&exe, b"\xcf\xfa\xed\xfe").unwrap();
        let js = d.join("jsapp");
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(js.join("package.json"), "{}").unwrap();
        let fx2 = fx.clone();
        let (mut b, calls, _) = rea_with(&REA_ALLOWED, move |t| fx2[t]["result"].clone());

        let r = b.analyze(&req(&app, "owner")).unwrap();
        let claims: Vec<&str> = r.findings.iter().map(|f| f.claim.as_str()).collect();
        assert_eq!(claims[..2], ["束の中の file: Contents/MacOS/Quillpad（executable）", "束の中の file: Contents/MacOS/quillctl（resource）"], "実行 file と CLI を先に: {claims:?}");
        assert!(claims.contains(&"束の中の file: Contents/Info.plist（plist）") && claims.contains(&"束の中の symlink: Contents/Resources/link.txt（resource）"), "{claims:?}");
        assert_eq!(r.findings.len(), 4, "file 4 つだけ（root-manifest・識別子・hash は finding にしない）: {claims:?}");
        assert!(r.findings.iter().all(|f| f.status == FindingStatus::Analyzed && f.confidence == 0.4 && !f.claim.contains("ev_") && !f.claim.contains("art_")), "{:?}", r.findings);
        assert!(r.findings[0].evidence[0].starts_with("REA 6.7.0 の inspect_artifact・evidence_id ev_"), "{:?}", r.findings[0].evidence);
        let has = |c: &str| r.constraints.iter().any(|x| x.contains(c));
        assert!(has("REA が見ていない所: format-specific-analysis-required（open_binary was not run") && has("does not execute the artifact"), "{:?}", r.constraints);
        assert!(r.constraints.iter().any(|x| x == PARTIAL) && r.constraints.iter().any(|x| x == UNTRUSTED) && r.constraints.last().map(String::as_str) == Some(REA_NOTE), "{:?}", r.constraints);
        assert_eq!(r.constraints.iter().filter(|x| x.contains("does not execute the artifact")).count(), 1, "同じ制約を 2 度書かない（REA は上の段と normalized_result の両方に書く）");

        let r = b.analyze(&req(&exe, "owner")).unwrap();
        assert_eq!(calls.borrow()[3], (REA_MACHO.to_string(), json!({ "detail": "summary" })));
        assert!(r.findings.iter().any(|f| f.kind == FindingKind::Binary && f.claim.starts_with("summary: ") && f.claim.contains("mach-o")), "{:?}", r.findings);
        assert!(r.findings[0].evidence.iter().any(|e| e == "Location: /x/Quillpad.app/Contents/MacOS/Quillpad"), "{:?}", r.findings[0].evidence);
        assert!(r.constraints.iter().any(|x| x.contains("did not report exhaustive imports")) && r.constraints.iter().any(|x| x == PARTIAL), "分からない事が在れば complete-within-view でも一部: {:?}", r.constraints);

        let r = b.analyze(&req(&js, "owner")).unwrap();
        assert_eq!(calls.borrow()[4], (REA_JS.to_string(), json!({ "input_path": js })));
        assert!(r.findings.iter().any(|f| f.kind == FindingKind::ElectronIpc && f.claim == "ipc-handler: pad:set-title"), "{:?}", r.findings.iter().map(|f| &f.claim).collect::<Vec<_>>());
        assert!(r.findings.iter().all(|f| f.confidence == 0.7) && !r.constraints.iter().any(|x| x == PARTIAL), "graph の coverage が complete: {:?}", r.constraints);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 秘密: REA の答えの文字（対象の中の文字列）に混じった token・access key・秘密鍵の本文を report に入れない。害の無い symbol 名は残す。
    #[test]
    fn w19_3_secrets_in_rea_answers_do_not_reach_the_report() {
        let d = dir("secret");
        let (ghp, akia, body) = (format!("ghp_{}", "Z9yX8wV7uT6sR5qP4oN3mL2kJ1iH0g"), format!("AKIA{}", "W193TESTSECRETXX"), "b3BlbnNzaC1rZXktdjEAAAAABG5vbmU");
        let pem = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY-----");
        let env = json!({ "evidence_id": "ev-2", "normalized_result": { "strings": ["SaveDocumentSymbol", ghp, akia, pem], "observations": [{ "kind": "string", "value": format!("token {ghp}") }] },
                          "limitations": [format!("saw {akia}")], "coverage": { "status": "complete" } });
        let (mut b, _, _) = rea(&REA_ALLOWED, json!({ "structuredContent": env }));
        let text = serde_json::to_string(&b.analyze(&req(&d, "owner")).unwrap()).unwrap();
        for s in [ghp.as_str(), akia.as_str(), body] {
            assert!(!text.contains(s), "{s} が report に入った: {text}");
        }
        assert!(text.contains("SaveDocumentSymbol"), "何も写さない実装で通らないように: {text}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_3_reports_round_trip_on_disk_and_refuse_verified_or_bad_names() {
        let d = dir("store");
        let mut r = AnalysisReport {
            schema: ANALYSIS_SCHEMA.into(),
            id: "an-1".into(),
            backend: REA_SERVER.into(),
            target: AnalysisTarget { bundle_id: None, path: "/x.app".into(), version: Some("2".into()) },
            findings: vec![Finding { kind: FindingKind::Binary, claim: "c".into(), evidence: vec![], confidence: 0.5, status: FindingStatus::Inferred }],
            constraints: vec![REA_NOTE.into()],
            at: "2026-10-24T00:00:00Z".into(),
        };
        let p = save_report(&d, &r).unwrap();
        assert_eq!(p, d.join("an-1.json"));
        assert_eq!(load_report(&d, "an-1").unwrap(), r);
        assert!(std::fs::read_dir(&d).unwrap().all(|e| e.unwrap().path() == p), "一時 file を残さない");
        for bad in ["", "..", "a/b"] {
            assert!(matches!(save_report(&d, &AnalysisReport { id: bad.into(), ..r.clone() }), Err(AnalysisError::Store(_))), "{bad:?}");
            assert!(matches!(load_report(&d, bad), Err(AnalysisError::Store(_))), "{bad:?}");
        }
        assert!(matches!(load_report(&d, "none"), Err(AnalysisError::Store(_))));
        r.findings[0].status = FindingStatus::Verified;
        assert!(matches!(save_report(&d, &AnalysisReport { id: "an-2".into(), ..r.clone() }), Err(AnalysisError::Store(_))), "検証済みを名乗る report は書かない");
        std::fs::write(d.join("an-3.json"), r#"{"schema":"other"}"#).unwrap();
        assert!(matches!(load_report(&d, "an-3"), Err(AnalysisError::Store(_))));
        let _ = std::fs::remove_dir_all(&d);
    }
}
