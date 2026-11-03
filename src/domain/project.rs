//! ProjectDefinitionV1 — 1 つの Project が複数の Page を持つ形（YCM-010・SPEC v15 §6）。`AppDefinitionV3` は変えず、Page が `app_ref` で指して包む。
//! 差分は `ProjectPatchOp`（Page の中は `domain::patch` の AppPatch をそのまま使う）。当て方は全部か無しか（`patch::apply` と同じ流儀）。
//! hash（`base_hash`）はここでは計算しない（domain は serde / std だけ）。照合は保存する側（generated.rs）が `runtime::permission::digest` で行う。
//! 図: docs/diagrams.md §25。
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

use super::app::AppDefinitionV3;
use super::patch::{apply_one, AppPatch, PatchError, PatchOp};
use super::run::{Observation, Outcome};
use super::ui::{canvas_fault, UiNode, BUILTIN_COMPONENTS};
use super::validate::ValidationError;

/// Wave: W1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDefinitionV1 {
    pub id: String,
    pub name: String,
    pub goal: String,
    #[serde(default)]
    pub app_refs: Vec<String>,
    pub pages: Vec<PageDefinition>,
    #[serde(default)]
    pub resources: Vec<ProjectResource>,
    #[serde(default)]
    pub terminal_specs: Vec<TerminalSessionSpec>,
    /// W2c: Page に組み込む本物の app / Web の起こし方。`#[serde(default)]` なので W1 の保存（この欄が無い json）もそのまま読める。
    #[serde(default)]
    pub surface_specs: Vec<SurfaceSpec>,
    pub revision: u64,
}

/// Wave: W1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDefinition {
    pub id: String,
    pub route: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ref: Option<String>,
    pub root: UiNode,
    pub revision: u64,
}

/// Project が参照する外の物（repo・file・URL）。中身は持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResource {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// W12a: Project の resource を読む口。Core（generated.rs）はこの trait だけを見て、読み方（git 等）を知らない。
/// 実装は Adapter の registry（`adapters::AdapterRegistry`: `Adapter::reads` の resource の kind で Adapter を引き、input `{"uri": uri}` で invoke）と試験の偽物。
/// trait を domain に置くのは、`tests/yc_demo_e2e.rs` が `generated.rs` を `mod adapters`（→ `mod cua`）無しで compile するため（D-a）。
///
/// 例: `generated.set_resource_reader(Box::new(crate::adapters::AdapterRegistry::with_local_adapters()));`（app.rs・起動の時に 1 回）
///
/// 保証する（実装に求める）: 読むだけ（resource にも利用者の他の道具〈git の index.lock 等〉にも書かない）・承認の要る action は呼ばない・
/// 読めた時だけ `Outcome::Verified` と `Some(Observation)`（payload = その Adapter が doc に書いた形・source_ref は provenance）・
/// 読めない時は `Outcome::Failed{ErrorCode}`（道具が無い = NotReady・場所が無い / 読める形でない = TargetNotFound・uri の形が違う = InvalidInput・
/// 時間切れ = Timeout）か、その kind を読む Adapter が無い時の `Outcome::Rejected{reason: 画面に出せる 1 行}` で、Observation は None。
/// 保証しない: 速さ（呼ぶ側の UI の thread で走る・Adapter の時間切れまで待つ）・読んだ後に resource が変わらない事。
/// Wave: W12a
pub trait ResourceReader {
    fn read(&mut self, resource: &ProjectResource) -> (Outcome, Option<Observation>);
}

/// W12a: resource を読んだ中身の置き場の名前 = 殻の app の `sources[].id` = binding の `source_ref` = `ui_state` の key（session `main.{app}`）。
/// 例: `resource_source_id("repo") == "resource.repo"`。`local.*`（local action）・`page:*`・`project:drawer` と重ならない。
pub fn resource_source_id(resource_id: &str) -> String {
    format!("resource.{resource_id}")
}

/// Terminal の起こし方だけ（PTY の handle は保存しない・SPEC §6）。中身は W4。
/// W5: Page の `terminal` node（`props.terminal_ref` = この id）が指す。id は `SessionKey::session_id` でもあり、再起動をまたいで同じ
/// （pid は起動ごとに変わる・前の起動の行は Unavailable）。足すのは `ProjectPatchOp::AddTerminal` の 1 か所（W3 の生成も同じ op を出す）。
/// 例: `TerminalSessionSpec { id: storage::new_id(), project_id: p.id.clone(), shell: "/bin/zsh".into(), working_directory: home, keep_alive_on_page_switch: true }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSessionSpec {
    pub id: String,
    pub project_id: String,
    pub shell: String,
    pub working_directory: String,
    pub keep_alive_on_page_switch: bool,
    /// L7-AGENT: 起こした shell に最初に打つ 1 行（`claude`・`grok` 等の agent の起動）。None = shell だけ（今まで通り・保存の形も hash も変わらない）。
    /// 打つのは `TerminalManager::spawn`（shell の 1 行として = agent が終わっても shell は残る）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// W12W-B: Page の中の面の位置と大きさ（points・Page の内容の左上が原点）。無ければ今の木の並び。
/// MIRROR-HYBRID-1101 追記 246: Mirror の面は、x・y = Page の縦の流れの並びの所から右・下へのずれ・w・h = 人が四隅で選んだ枠（題の帯を含む）の大きさ
/// （Page の上で浮かせず縦の流れのまま = 後ろの部品は下へどく）。I33b: x・y は負でもよい（並びの所より左・上 = 前の部品に重ねた枠・Page の列の中）。
/// 例: `SurfaceFrame { x: 24.0, y: 80.0, w: 640.0, h: 480.0 }`
/// 保証する: 保存して再起動の後も同じ。保証しない: その矩形に頁が出ている事・Mirror の本物の app がその大きさを受ける事。
/// Wave: W12W-B
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFrame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl SurfaceFrame {
    /// 有限で幅・高さが 1 以上。
    pub fn valid(self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            && self.w >= 1.0
            && self.h >= 1.0
    }
}

/// W2c: Page の `source_surface` node（`props.surface_ref` = この id）が指す本物の app / Web の起こし方。
/// 例: `SurfaceSpec { id: "docs".into(), backend: SurfaceBackendKind::NativeDirect, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("notes.txt".into()) }, keep_alive: true, frame: None, browser: false }`
/// 保証する: 保存するのは起こし方だけ（pid・窓 id・WebView の handle は保存しない）。保証しない: target の app / URL が今在る事（それは `SurfaceReceipt` の読み戻しで見る）。
/// Wave: W2c
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSpec {
    pub id: String,
    pub backend: SurfaceBackendKind,
    pub target: SurfaceTarget,
    /// Page を移っても閉じない（Terminal の `keep_alive_on_page_switch` と同じ意味）。
    pub keep_alive: bool,
    /// W12W-B: Page の中の位置と大きさ。無ければ今の木の並び。W1 の保存（この欄が無い json）は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<SurfaceFrame>,
    /// W12W-B: Browser の面（host の allowlist 無し・上の欄で URL を替える）。既存の Diagram は false。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub browser: bool,
}

impl SurfaceSpec {
    /// MIRROR-FRAME-1101（owner 247）: 中身の無い Mirror の枠（Page の ＋ の `Add mirror frame`）= Mirror の面で `SurfaceTarget::App` の bundle id が空。
    /// 窓を入れると（`ProjectPatchOp::SetSurfaceWindow` の `bundle_id`）false になり、今の Mirror の面と同じ物になる。新しい variant を足さない理由: 保存の形
    /// （`{"kind":"app","bundle_id":""}`）を変えず、SurfaceTarget を作る 78 か所を触らない。
    /// 読む所: `MirrorPages::show`（窓を探さず `Choose a window` を描く）・`composer::verifier::project_seats`（席にしない）・`composer::project::surface_names`
    /// （名前の無い面を一言の候補にしない）・`generated::mirror_surfaces_on`（映していない枠で `Stop mirroring` を出さない）。
    /// 例: `if spec.blank_frame() { return pages.blank(ui, rect, spec) }`
    /// Wave: MIRROR-FRAME-1101
    pub fn blank_frame(&self) -> bool {
        self.backend == SurfaceBackendKind::Mirror && matches!(&self.target, SurfaceTarget::App { bundle_id, .. } if bundle_id.trim().is_empty())
    }
}

/// 保存する描画の方式は 3 つだけ。`surface::SurfaceBackend`（7 種・process を持つ module）は domain から引けないので、
/// 名前（snake_case）を揃えた 3 種をここに持つ（`native_direct` / `mirror` / `webview`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceBackendKind {
    NativeDirect,
    Mirror,
    #[serde(rename = "webview")]
    WebView,
}

/// 何を起こすか。App = bundle id（と開く file。Project の working dir からの相対）・Url = Web。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceTarget {
    App {
        bundle_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
    },
    Url {
        url: String,
    },
}

/// `project::readback` が「この Mac に app が入っていない」と書く oracle の原文。renderer はこれと一致した時だけ「入っていません」と言う。
pub const NOT_INSTALLED: &str = "find_app: この Mac に入っていません";

/// W14（YCM-062 の不許可）: app は入っているが bridge の `permission` が trusted = false と答えた時の oracle の原文。renderer はこれと一致した時だけ許可の 1 行を出す。
pub const NOT_TRUSTED: &str = "permission: Shibubu にアクセシビリティの許可がありません";

/// W16-2: `project::native::Readback::receipt` が「重ねた窓の文書が disk の中身と違う（保存していない編集・Undo の後。disk は変わっていない）」と書く oracle の原文。
/// renderer はこれと一致した時だけ「保存していない変更があります。<app> で保存すると読み戻します」と言う（D-e の次の一手）。
/// 外の process が file を書き換えた（disk が変わり窓は古い）は `CHANGED_OUTSIDE`（W16-2-b / W16-3）。
pub const NOT_ON_DISK: &str = "ax_text: 窓の文書が disk の中身と違います（保存していない変更）";

/// W16-2: disk を読めない 1 行（`NativeError::FileUnreadable` の Display）の終わり。`Readback::receipt` は unavailable の oracle にその 1 行をそのまま入れる（74ec7fb の口・頭を足さない）。
/// renderer は unavailable の oracle がこれで終わる時だけ、その 1 行（file の名前・理由・次の一手）をそのまま出す。
pub const FILE_UNREADABLE_NEXT: &str = "が在るか確かめてから、この Page に戻ってください";

/// W16-2: `Readback::receipt` が NOT_ON_DISK の後に窓の文書が disk と同じに戻った時（保存せずに Undo した等）に書く unverified の oracle の原文。
/// 「この間に保存は無い」とは言わない（AX の応答待ち・再起動の後は間の保存を見ていない）。renderer はこれと一致した時だけ「窓の文書は disk の中身と同じです」と言う。
pub const IN_SYNC: &str = "file_sha+ax_text: 窓の文書が disk の中身と同じです（保存を読み戻した物ではないので verified にしません）";

/// W16-3 / W16-2-b: `Readback::receipt` が「disk が変わり、重ねた窓の文書が disk と違う（外の process が file を書き換えた。窓はまだ古い中身）」と書く oracle の原文。
/// renderer はこれと一致した時だけ「文書が外で書き換わりました。<app> で読み直すか、外で開くで確かめてください」と言う。
/// 保存で上書きさせる一手は出さない（`NOT_ON_DISK` とは分ける）。本体が `receipt` にこの oracle を書くのは W16-3 の実装の段。
/// 例: `if r.oracle == CHANGED_OUTSIDE { /* 保存させる 1 行に落とさない */ }`
/// 保証する: この原文と一致した時だけ renderer が「外で書き換わった」の 1 行を出す。保証しない: 誰が disk を書き換えたか。
/// Wave: W16-3
pub const CHANGED_OUTSIDE: &str = "ax_text: 窓の文書が disk の中身と違います（外で書き換わった）";

/// 読み戻しの対象の種類（YC_ACCEPTANCE §2 の kind ごとの値）。`Local` は verified を取らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    NativeApp,
    Web,
    Terminal,
    Local,
}

impl ReceiptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NativeApp => "native_app",
            Self::Web => "web",
            Self::Terminal => "terminal",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Verified,
    Unverified,
    Unavailable,
    Ended,
}

impl ReceiptState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Unavailable => "unavailable",
            Self::Ended => "ended",
        }
    }
}

/// W2c: surface を読み戻した結果 1 件（表 `surface_receipts` の 1 行・5 通目の「対象 app・OS・版・成功した操作・日時・証拠」の 6 欄）。
/// 例: 保存の前に `receipt.validate()?` → `store.put_surface_receipt(project_id, &r.surface_id, &r.page_id, r.kind.as_str(), r.state.as_str(), &serde_json::to_value(&r)?)`。
/// 保証する: `validate` を通った verified は 6 つの欄（app_id・app_version・os_version・operation・verified_at・evidence_ref）が空でなく、kind が Local でない。保証しない: 欄の値が本当か（正は試験の側の oracle・`tests/common/readback.rs`）。
/// Wave: W2c
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceReceipt {
    pub surface_id: String,
    pub page_id: String,
    pub kind: ReceiptKind,
    pub state: ReceiptState,
    /// 読んだ道の名前（例 `ax_value`・`file_sha`・`github_api`）。
    pub oracle: String,
    /// NativeApp = bundle id・Web = origin・Terminal = shell の path。
    pub app_id: String,
    /// NativeApp = `CFBundleShortVersionString`・Web = WebKit の `CFBundleVersion`・Terminal = `--version` の 1 行。
    pub app_version: String,
    /// `sw_vers -productVersion` と build。
    pub os_version: String,
    /// 実際に成功した操作（1 行）。
    pub operation: String,
    /// RFC3339（UTC）。
    pub verified_at: String,
    /// `docs/evidence/<Wave>/…` の path か、DB の evidence の id。W16-2 の Native Direct の保存の読み戻しは `file_sha256:<読んだ disk の sha256>`。
    pub evidence_ref: String,
}

/// `SurfaceReceipt::validate` の拒否。画面と log に出す 1 行は Display。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptError {
    /// verified なのに欄が空（欄の名前）。
    MissingField(&'static str),
    /// `Local` は verified を取らない。
    LocalVerified,
}

impl fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(n) => write!(f, "確認済みにするには '{n}' が要ります。読み戻し直してください"),
            Self::LocalVerified => write!(f, "生成した部品だけの面は確認済みにできません"),
        }
    }
}
impl std::error::Error for ReceiptError {}

impl SurfaceReceipt {
    /// 保存の前の照合。verified 以外はいつも Ok（空の欄を許す）。
    pub fn validate(&self) -> Result<(), ReceiptError> {
        if self.state != ReceiptState::Verified {
            return Ok(());
        }
        if self.kind == ReceiptKind::Local {
            return Err(ReceiptError::LocalVerified);
        }
        for (name, v) in [
            ("app_id", &self.app_id),
            ("app_version", &self.app_version),
            ("os_version", &self.os_version),
            ("operation", &self.operation),
            ("verified_at", &self.verified_at),
            ("evidence_ref", &self.evidence_ref),
        ] {
            if v.trim().is_empty() {
                return Err(ReceiptError::MissingField(name));
            }
        }
        Ok(())
    }
}

/// revision を作った物。復元と Undo は snapshot、差分の表示は ops を使う。
/// W8: `Undo` は書かない（予約・migration の CHECK には残す）。Undo / Redo は行を足さず、`projects` の行を前後の行の snapshot に置き換えるだけ（`UndoRedo`）。
/// Wave: W1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionOrigin {
    Generate,
    NlEdit,
    DirectEdit,
    Undo,
}

impl RevisionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::NlEdit => "nl_edit",
            Self::DirectEdit => "direct_edit",
            Self::Undo => "undo",
        }
    }
}

/// W8: Project の Undo / Redo の行き先（revision の番号）。None はその向きに戻れない（ボタンを出さない・L2）。
/// 保存形式（新しい表・migration は無し）: `project_revisions` の行は `1..=最後` で欠けなく並び、今（head）= `projects` の json の `revision`。
/// `undo = head - 1`（1 以上・行が在る時）、`redo = head + 1`（行が在る時）。Undo / Redo は `projects` の行をその行の snapshot に置き換えるだけで、
/// `project_revisions` は書かない・消さない。Undo の後に別の編集を当てると、`apply_project` が `revision > head` の行（Redo の行き先）を消してから書く。
///
/// 使い方: `let ur = g.undo_redo(&project)?; if ur.undo.is_some() { /* 「↶ 元に戻す」を出す */ }`
/// 保証する: DB だけから決まる（再起動をまたいでも同じ行き先）。保証しない: `list_project_revisions(..).last()` が今の Project である事
/// （Undo の後の最後の行は Redo の行き先。今は `projects` の行の `revision` で見る）。
/// Wave: W8
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UndoRedo {
    pub undo: Option<u64>,
    pub redo: Option<u64>,
}

/// W8: Undo / Redo の拒否（どれも何も書かず今の Page に残る）。古い base_hash と store の失敗は `apply_project` と同じ 1 行を使う（ここに持たない）。
/// Wave: W8
#[derive(Debug, Clone, PartialEq)]
pub enum UndoError {
    /// head が revision 1（生成のまま・まだ編集していない・包んだだけの Project）。
    NothingToUndo,
    /// head が最後の行（Undo していない・Undo の後に別の編集をした）。
    NothingToRedo,
    /// 行き先の行が無い・読めない（行の欠け・後の版の JSON）。
    Unreadable { revision: u64, reason: String },
}

impl fmt::Display for UndoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingToUndo => write!(f, "元に戻せる変更はありません"),
            Self::NothingToRedo => write!(f, "やり直せる変更はありません。元に戻した後に別の変更をすると、やり直しは消えます"),
            Self::Unreadable { revision, reason } => write!(
                f,
                "保存した版 {revision} を読めません（{reason}）。開き直してからもう一度押してください"
            ),
        }
    }
}
impl std::error::Error for UndoError {}

/// Wave: W1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectPatchOp {
    /// Page の木に AppPatch を 1 つ当てる。
    Page {
        page_id: String,
        patch: AppPatch,
    },
    /// index 無しは末尾。
    AddPage {
        page: PageDefinition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemovePage {
        page_id: String,
    },
    RenamePage {
        page_id: String,
        title: String,
    },
    MovePage {
        page_id: String,
        index: usize,
    },
    /// surface_key は全 Page から探す（Page ごとの key は重なりうるので、0 件も 2 件以上も拒否）。index 無しは末尾。
    MoveSurface {
        surface_key: String,
        to_page: String,
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// W5: Terminal を Page に置く。`spec` を `terminal_specs` に足し、`page_id` の `parent` の下（index 無しは末尾）に
    /// `{ key: node_key, component: "terminal", props: { terminal_ref: spec.id } }` を入れる。両方当たるか、何も変わらない。
    /// 例: `ProjectPatchOp::AddTerminal { spec, page_id: page.id.clone(), parent: page.root.key.clone(), node_key: format!("terminal.{}", spec.id), index: None }`
    /// 保証する: spec の id が重なれば `DuplicateTerminal`・Page / parent が無ければ `PageNotFound` / `Page{TargetNotFound}`・node の key の重なりは
    /// 最後の `validate_project` が拒否。保証しない: shell が起こせる事（起こすのは `Generated::ensure_terminal`）。
    /// Wave: W5
    AddTerminal {
        spec: TerminalSessionSpec,
        page_id: String,
        parent: String,
        node_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// W16M-3: 面（`SurfaceSpec`・Mirror / Native Direct / WebView）を Page に置く。`spec` を `surface_specs` に足し、`page_id` の `parent` の下
    /// （index 無しは末尾）に `{ key: node_key, component: "source_surface", props: { surface_ref: spec.id } }` を入れる。両方当たるか、何も変わらない。
    /// 面を足す op はこれ 1 つ（W12W-B の「＋ Browser」も同じ op を使う）。
    /// Wave: W16M-3
    /// 例（「notes.md を Mirror で見る Page を足して」）: `[AddPage { page, index: None }, AddSurface { spec: SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror,
    /// target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false }, page_id: page.id.clone(),
    /// parent: page.root.key.clone(), node_key: "surface.notes".into(), index: None }]`
    /// 保証する: spec の id が重なれば `DuplicateSurface`・Page / parent が無ければ `PageNotFound` / `Page{TargetNotFound}`・node の key の重なりは `validate_project`。
    /// 保証しない: 面が起こせる事（同意・画面収録の許可・app の起動は描く時の `MirrorPages` / `NativeSurfaces` / `PageWebViews`）。
    AddSurface {
        spec: SurfaceSpec,
        page_id: String,
        parent: String,
        node_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// W12W-B: Browser の上の欄で URL を替える。`surface_id` は `surface_specs` の id。Url の面だけ。
    /// 例: `SetSurfaceUrl { surface_id: "browser.1".into(), url: "https://www.youtube.com/watch?v=x".into() }`
    /// 保証する: 無い id は `SurfaceNotFound`・Url でない面は `NotUrlSurface`・同じ URL なら Project は変わらない（revision を足さない）。
    /// 保証しない: 頁が読める事（読むのは `PageWebViews`）。
    /// Wave: W12W-B
    SetSurfaceUrl {
        surface_id: String,
        url: String,
    },
    /// W12W-B: 面の端 / 題の帯の drag の終わりで位置と大きさを残す。`frame.valid()` でなければ何も変わらない。
    /// 例: `SetSurfaceFrame { surface_id: "browser.1".into(), frame: SurfaceFrame { x: 40.0, y: 80.0, w: 720.0, h: 480.0 } }`
    /// MIRROR-HYBRID-1101 追記 246: Mirror の面の四隅の引き終わりも同じ op（並びの所からのずれと枠の大きさ・`SurfaceFrame` の doc）。
    /// 保証する: 無い id は `SurfaceNotFound`・同じ frame なら revision を足さない。保証しない: Native Direct の自由な配置（列 N・M）。
    /// Wave: W12W-B
    SetSurfaceFrame {
        surface_id: String,
        frame: SurfaceFrame,
    },
    /// RQ4（RQ-019・`Choose window…`）: app の面が映す窓を選び直す = `SurfaceTarget::App::file`（窓の題に含む語・Mirror の `MirrorTarget::find` の `title_has`）を替える。
    /// `title` が None なら `file` を外す（app の前の窓に任せる）。`Generated::page_intent` の `PageIntent::ChooseWindow` が DirectEdit で 1 回当てる（Undo で前の窓に戻る）。
    /// 例: `SetSurfaceWindow { surface_id: "notes".into(), title: Some("todo.md".into()) }`。保存の形: `{"op": "set_surface_window", "surface_id": "notes", "title": "todo.md"}`
    /// （None は `title` を書かない）。
    /// 保証する: 無い id は `SurfaceNotFound`・Url の面は `Invalid`（`Only an app's surface has windows`）・空白だけの題は None と同じ・同じ値なら revision を足さない。
    /// 保証しない: その題の窓が今在る事（読むのは Mirror）・書類の path としての正しさ（題の語として使うだけ）。
    /// MIRROR-FRAME-1101: `bundle_id` が Some なら app も替える（空の枠に全部の app の窓の一覧・引いて入れた窓・`Open an app…` で入れる = 1 つの revision・⌘Z で空の枠へ）。
    /// None なら app はそのまま（今までの保存は欄が無い = None）。空白だけの bundle id は `Invalid`（`Choose an app`）。
    /// 例: `SetSurfaceWindow { surface_id: "frame.1".into(), title: Some("Docs".into()), bundle_id: Some("com.google.Chrome".into()) }`
    /// Wave: RQ4, MIRROR-FRAME-1101
    SetSurfaceWindow {
        surface_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bundle_id: Option<String>,
    },
    /// RQ2（RQ-005・Rename）: Project の名前（`ProjectDefinitionV1::name`）を替える。Page・id・route は変えない。`Generated::rename_project` が `apply_project(.., DirectEdit)` で
    /// 1 回当てる（Project の revision が 1 つ増え、⌘Z の Undo で前の名前に戻る）。例: `RenameProject { name: "Shop inventory".into() }`
    /// 保証する: 前後の空白を落として書く・空は `Invalid` で何も変わらない。保証しない: 同じ名前の Project が他に無い事（同じ名前を許す・`Generated::rename_project` も拒まない）・
    /// 生成した app の題（`AppDefinitionV3::title`）を替える事（画面の名前を Project の名前から読むのは本体）。
    /// Wave: RQ2
    RenameProject {
        name: String,
    },
    /// G99c: Project の repo の場所を `dir`（絶対 path）にする。`resources` の id "repo" を `file://<dir>` に替え（無ければ足す）、`terminal_specs` の
    /// 全部の `working_directory` を `dir` に替える（次に起こす shell から効く・今の session は動かさない）。Page の木は触らない（「No folder」の行を Terminal に
    /// 替えるのは呼ぶ側が同じ ops に `Page{Remove}` と `AddTerminal` を並べる・`Generated` の `workspace_ops`）。
    /// 例: `SetWorkspace { dir: "/Users/you/code/shibubu".into() }`
    /// 保証する: 絶対 path でなければ `Invalid`。保証しない: dir が在る事・git の repo である事（Git の欄は `read_resources` が読む）。
    SetWorkspace {
        dir: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectPatchError {
    PageNotFound(String),
    DuplicatePage(String),
    LastPage(String),
    SurfaceNotFound(String),
    AmbiguousSurface(String),
    /// W5: 同じ id の Terminal が既に `terminal_specs` に在る（AddTerminal）。
    DuplicateTerminal(String),
    /// W16M-3: 同じ id の面が既に `surface_specs` に在る（AddSurface）。
    DuplicateSurface(String),
    /// W12W-B: `SetSurfaceUrl` の先が Url の面ではない。
    NotUrlSurface(String),
    /// FX3（FX-009）: 名前替えの先の名前を、同じ Project の別の Page が持っている（前後の空白を除き大文字小文字を見ない）。値は打った名前（trim 後）。
    /// 作る所: `router::rename_page` だけ（足す Page の title の重なりは今まで通り許す = `router::new_page` の doc）。Display は表（copy.en.md の `There's already {what}.`）の英語。
    /// 例: `Err(ProjectPatchError::DuplicateTitle("Tests".into()))` → `There's already a page named “Tests”.`
    /// 保証しない: 他の Project の Page との重なり（名前は Project の中だけで見分ける）。
    /// Wave: FX3
    DuplicateTitle(String),
    Page {
        page_id: String,
        error: PatchError,
    },
    /// 当てた後の Project が形の検査に落ちた（理由を 1 つずつ）。
    Invalid(Vec<String>),
}

impl fmt::Display for ProjectPatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PageNotFound(id) => write!(
                f,
                "Page '{id}' がありません。Page の一覧を開き直してから当て直してください"
            ),
            Self::DuplicatePage(id) => {
                write!(f, "Page '{id}' は既にあります。別の名前で足してください")
            }
            // id は出さない（1 枚しか無い時だけ起きる・画面に内部の語を出さない）
            Self::LastPage(_) => write!(
                f,
                "最後の Page は消せません。先に別の Page を足してください"
            ),
            Self::SurfaceNotFound(k) => write!(
                f,
                "部品 '{k}' がどの Page にもありません。開き直してから当て直してください"
            ),
            Self::AmbiguousSurface(k) => write!(
                f,
                "部品 '{k}' が複数の Page にあります。動かす Page の部品を選び直してください"
            ),
            // id は出さない（乱数の内部の語）
            Self::DuplicateTerminal(_) => write!(
                f,
                "この Terminal は既に置いてあります。Page の一覧を開き直してください"
            ),
            // id は出さない（AddTerminal と同じ）
            Self::DuplicateSurface(_) => write!(
                f,
                "この面は既に置いてあります。Page の一覧を開き直してください"
            ),
            Self::NotUrlSurface(_) => write!(
                f,
                "この面は頁の URL を持てません。Page の「＋ Browser」で開き直してください"
            ),
            // FX3: 英語（表 copy.en.md の `There's already {what}.`・他の腕の日本語は G97 の持ち物で触らない）
            Self::DuplicateTitle(title) => write!(f, "There's already a page named “{title}”."),
            Self::Page { page_id, error } => write!(f, "Page '{page_id}': {error}"),
            Self::Invalid(errs) => write!(
                f,
                "当てた後の Project が検査に落ちました（{}）。変更を見直してください",
                errs.join(" / ")
            ),
        }
    }
}
impl std::error::Error for ProjectPatchError {}

impl ProjectDefinitionV1 {
    /// 互換ラップ（YCM-011）: Project を持たない app を Page 1 枚の Project として読む。id は app から決まる（開くたびに同じ hash）。
    /// ponytail: Page の root は app の木の写し。app 側の ui.patch とは同期しない（Page を描く W2 で app_ref から引き直すか決める）。
    pub fn wrap(app: &AppDefinitionV3) -> Self {
        let title = app
            .title
            .clone()
            .unwrap_or_else(|| app.goal.literal_goal.clone());
        ProjectDefinitionV1 {
            id: format!("project.{}", app.id),
            name: title.clone(),
            goal: app.goal.literal_goal.clone(),
            app_refs: vec![app.id.clone()],
            pages: vec![PageDefinition {
                id: format!("page.{}.1", app.id),
                route: "/".into(),
                title,
                app_ref: Some(app.id.clone()),
                root: app.composition.root.clone(),
                revision: 1,
            }],
            resources: Vec::new(),
            terminal_specs: Vec::new(),
            surface_specs: Vec::new(),
            revision: 1,
        }
    }

    pub fn page(&self, id: &str) -> Option<&PageDefinition> {
        self.pages.iter().find(|p| p.id == id)
    }

    fn page_index(&self, id: &str) -> Result<usize, ProjectPatchError> {
        self.pages
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| ProjectPatchError::PageNotFound(id.into()))
    }

    fn surface_index(&self, id: &str) -> Result<usize, ProjectPatchError> {
        self.surface_specs
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| ProjectPatchError::SurfaceNotFound(id.into()))
    }

    /// `props.surface_ref` が surface_id の node を持つ Page の id（面の op が revision を進める Page）。
    fn pages_showing<'a>(&'a self, surface_id: &'a str) -> impl Iterator<Item = String> + 'a {
        self.pages
            .iter()
            .filter(move |pg| {
                pg.root.nodes().iter().any(|n| {
                    n.props.get("surface_ref").and_then(|v| v.as_str()) == Some(surface_id)
                })
            })
            .map(|pg| pg.id.clone())
    }
}

/// 形の検査: Page が 1 枚以上・Page の id と route が重ならない・Page ごとに node の key が重ならない・部品は表に在る物だけ。
/// W2c: `surface_specs` の id が空でなく重ならない・`props.surface_ref` を持てるのは `source_surface` だけで、指す先は `surface_specs` に在る物だけ（`ValidationError::MissingSurface`）。
/// W5: `terminal_specs` も同じ（id が空でなく重ならない・`project_id` がこの Project・`props.terminal_ref` を持てるのは `terminal` だけで指す先は在る物だけ
/// ＝ `ValidationError::MissingTerminal`）。Page に置かれていない spec は許す（Page から外しても shell は止めない・SPEC §4）。
/// 参照の検査（`patch::apply` の MissingAction / MissingSource と同じ）: app_ref を持つ Page の action_ref / binding はその app の
/// actions / sources に在る物だけ（`apps` に指す app が無ければ照合できないので拒否）。app を指さない Page は action_ref / binding を持てない。
pub fn validate_project(
    p: &ProjectDefinitionV1,
    apps: &[&AppDefinitionV3],
) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    if p.id.is_empty() {
        errs.push("id が空".into());
    }
    if p.pages.is_empty() {
        errs.push("Page が 0 枚".into());
    }
    let mut surface_ids = HashSet::new();
    for s in &p.surface_specs {
        if s.id.is_empty() || !surface_ids.insert(s.id.as_str()) {
            errs.push(format!("surface の id '{}' が空か重複", s.id));
        }
    }
    let mut terminal_ids = HashSet::new();
    for t in &p.terminal_specs {
        if t.id.is_empty() || !terminal_ids.insert(t.id.as_str()) {
            errs.push(format!("Terminal の id '{}' が空か重複", t.id));
        }
        if t.project_id != p.id {
            errs.push(format!(
                "Terminal '{}' の project_id '{}' がこの Project ではない",
                t.id, t.project_id
            ));
        }
    }
    let (mut ids, mut routes) = (HashSet::new(), HashSet::new());
    for page in &p.pages {
        if page.id.is_empty() || !ids.insert(page.id.as_str()) {
            errs.push(format!("Page の id '{}' が空か重複", page.id));
        }
        if !routes.insert(page.route.as_str()) {
            errs.push(format!("route '{}' が重複", page.route));
        }
        let app = match page.app_ref.as_deref() {
            Some(r) => match apps.iter().find(|a| a.id == r) {
                Some(a) => Some(*a),
                None => {
                    errs.push(format!("Page '{}' の app '{r}' が開いていません", page.id));
                    continue;
                }
            },
            None => None,
        };
        let mut keys = HashSet::new();
        for node in page.root.nodes() {
            let missing = match (app, &node.action_ref, &node.binding) {
                (None, None, None) => None,
                (None, ..) => Some(format!(
                    "node '{}' の action_ref / binding は、app を指さない Page では行き先が無い",
                    node.key
                )),
                (Some(a), Some(r), _) if a.action(r).is_none() => Some(
                    ValidationError::MissingAction {
                        key: node.key.clone(),
                        action_ref: r.clone(),
                    }
                    .to_string(),
                ),
                (Some(a), _, Some(b)) if a.source(&b.source_ref).is_none() => Some(
                    ValidationError::MissingSource {
                        key: node.key.clone(),
                        source_ref: b.source_ref.clone(),
                    }
                    .to_string(),
                ),
                _ => None,
            };
            if let Some(e) = missing {
                errs.push(format!("Page '{}': {e}", page.id));
            }
            if let Some(r) = node.props.get("surface_ref").and_then(|v| v.as_str()) {
                if node.component != "source_surface" || !surface_ids.contains(r) {
                    errs.push(format!(
                        "Page '{}': {}",
                        page.id,
                        ValidationError::MissingSurface {
                            key: node.key.clone(),
                            surface_ref: r.into(),
                        }
                    ));
                }
            }
            let terminal_ref = node.props.get("terminal_ref").and_then(|v| v.as_str());
            let bad_terminal = match (node.component == "terminal", terminal_ref) {
                (true, Some(r)) => !terminal_ids.contains(r),
                (true, None) => true,
                (false, r) => r.is_some(),
            };
            if bad_terminal {
                errs.push(format!(
                    "Page '{}': {}",
                    page.id,
                    ValidationError::MissingTerminal {
                        key: node.key.clone(),
                        terminal_ref: terminal_ref.unwrap_or_default().into(),
                    }
                ));
            }
            if node.key.is_empty() || !keys.insert(node.key.as_str()) {
                errs.push(format!(
                    "Page '{}' の key '{}' が空か重複",
                    page.id, node.key
                ));
            }
            if !BUILTIN_COMPONENTS.contains(&node.component.as_str()) {
                errs.push(format!(
                    "Page '{}' の '{}' は unsupported な部品 '{}'",
                    page.id, node.key, node.component
                ));
            }
            // AI-COMPOSE-1101: AI が書く部品は html が在り上限の中・きっかけは同じ Page の手（`TRIGGER_ACTS`）を指し、intent が値の穴 `{value}` を持つ
            if let Some(why) = (node.component == "canvas").then(|| canvas_fault(node, &page.root)).flatten() {
                errs.push(format!("Page '{}': canvas '{}' {why}", page.id, node.key));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// 全部当たるか、何も変わらないか。当たった Project は revision が 1 進み、触った Page の revision も 1 進む。
/// `apps` は Page が app_ref で指す app（参照の照合に使う。`validate_project`）。
/// Wave: W1
pub fn apply(
    base: &ProjectDefinitionV1,
    ops: &[ProjectPatchOp],
    apps: &[&AppDefinitionV3],
) -> Result<ProjectDefinitionV1, ProjectPatchError> {
    let p = apply_unchecked(base, ops)?;
    validate_project(&p, apps).map_err(ProjectPatchError::Invalid)?;
    Ok(p)
}

/// `apply` の `validate_project` を除いた物（op ごとの拒否は同じ）。編集の手を続けて組む途中の写し（`composer::project::edit_project_by_model`）用で、
/// 保存する前には必ず `apply` で本物の app と照合する（途中の写しを、Page の action・source を持たない app で照合して落とさないため）。
pub fn apply_unchecked(
    base: &ProjectDefinitionV1,
    ops: &[ProjectPatchOp],
) -> Result<ProjectDefinitionV1, ProjectPatchError> {
    use ProjectPatchError as E;
    let mut p = base.clone();
    let mut touched: HashSet<String> = HashSet::new();
    for op in ops {
        match op {
            ProjectPatchOp::Page { page_id, patch } => {
                let i = p.page_index(page_id)?;
                apply_one(&mut p.pages[i].root, patch).map_err(|error| E::Page {
                    page_id: page_id.clone(),
                    error,
                })?;
                touched.insert(page_id.clone());
            }
            ProjectPatchOp::AddPage { page, index } => {
                if p.page(&page.id).is_some() {
                    return Err(E::DuplicatePage(page.id.clone()));
                }
                let at = index.unwrap_or(p.pages.len()).min(p.pages.len());
                p.pages.insert(at, page.clone());
            }
            ProjectPatchOp::RemovePage { page_id } => {
                let i = p.page_index(page_id)?;
                if p.pages.len() == 1 {
                    return Err(E::LastPage(page_id.clone()));
                }
                p.pages.remove(i);
                touched.remove(page_id);
            }
            ProjectPatchOp::RenamePage { page_id, title } => {
                let i = p.page_index(page_id)?;
                p.pages[i].title = title.clone();
                touched.insert(page_id.clone());
            }
            ProjectPatchOp::MovePage { page_id, index } => {
                let i = p.page_index(page_id)?;
                let page = p.pages.remove(i);
                let at = (*index).min(p.pages.len());
                p.pages.insert(at, page);
            }
            ProjectPatchOp::MoveSurface {
                surface_key,
                to_page,
                parent,
                index,
            } => {
                let hits: Vec<usize> = (0..p.pages.len())
                    .filter(|&i| p.pages[i].root.find(surface_key).is_some())
                    .collect();
                let from = match hits.as_slice() {
                    [] => return Err(E::SurfaceNotFound(surface_key.clone())),
                    [i] => *i,
                    _ => return Err(E::AmbiguousSurface(surface_key.clone())),
                };
                let to = p.page_index(to_page)?;
                let from_id = p.pages[from].id.clone();
                let move_err = |error| E::Page {
                    page_id: from_id.clone(),
                    error,
                };
                if from == to {
                    let patch = AppPatch {
                        op: PatchOp::Move,
                        target_key: surface_key.clone(),
                        value: Some(serde_json::json!({ "parent": parent, "index": index })),
                    };
                    apply_one(&mut p.pages[from].root, &patch).map_err(move_err)?;
                } else {
                    if p.pages[from].root.key == *surface_key {
                        return Err(move_err(PatchError::RootImmovable {
                            op: PatchOp::Move,
                            key: surface_key.clone(),
                        }));
                    }
                    let node = p.pages[from]
                        .root
                        .detach(surface_key)
                        .ok_or_else(|| E::SurfaceNotFound(surface_key.clone()))?;
                    let dest = p.pages[to].root.find_mut(parent).ok_or_else(|| E::Page {
                        page_id: to_page.clone(),
                        error: PatchError::TargetNotFound {
                            op: PatchOp::Move,
                            key: parent.clone(),
                        },
                    })?;
                    let at = index
                        .unwrap_or(dest.children.len())
                        .min(dest.children.len());
                    dest.children.insert(at, node);
                }
                touched.insert(from_id);
                touched.insert(to_page.clone());
            }
            ProjectPatchOp::AddTerminal {
                spec,
                page_id,
                parent,
                node_key,
                index,
            } => {
                if p.terminal_specs.iter().any(|t| t.id == spec.id) {
                    return Err(E::DuplicateTerminal(spec.id.clone()));
                }
                let i = p.page_index(page_id)?;
                let dest = p.pages[i].root.find_mut(parent).ok_or_else(|| E::Page {
                    page_id: page_id.clone(),
                    error: PatchError::TargetNotFound {
                        op: PatchOp::Insert,
                        key: parent.clone(),
                    },
                })?;
                let mut node = UiNode {
                    key: node_key.clone(),
                    component: "terminal".into(),
                    ..UiNode::default()
                };
                node.props
                    .insert("terminal_ref".into(), serde_json::json!(spec.id));
                let at = index
                    .unwrap_or(dest.children.len())
                    .min(dest.children.len());
                dest.children.insert(at, node);
                p.terminal_specs.push(spec.clone());
                touched.insert(page_id.clone());
            }
            ProjectPatchOp::AddSurface {
                spec,
                page_id,
                parent,
                node_key,
                index,
            } => {
                if p.surface_specs.iter().any(|s| s.id == spec.id) {
                    return Err(E::DuplicateSurface(spec.id.clone()));
                }
                let i = p.page_index(page_id)?;
                let dest = p.pages[i].root.find_mut(parent).ok_or_else(|| E::Page {
                    page_id: page_id.clone(),
                    error: PatchError::TargetNotFound {
                        op: PatchOp::Insert,
                        key: parent.clone(),
                    },
                })?;
                let mut node = UiNode {
                    key: node_key.clone(),
                    component: "source_surface".into(),
                    ..UiNode::default()
                };
                node.props
                    .insert("surface_ref".into(), serde_json::json!(spec.id));
                if spec.browser {
                    node.props
                        .insert("display".into(), serde_json::json!("embedded"));
                }
                let at = index
                    .unwrap_or(dest.children.len())
                    .min(dest.children.len());
                dest.children.insert(at, node);
                p.surface_specs.push(spec.clone());
                touched.insert(page_id.clone());
            }
            ProjectPatchOp::SetSurfaceUrl { surface_id, url } => {
                let i = p.surface_index(surface_id)?;
                let SurfaceTarget::Url { url: cur } = &mut p.surface_specs[i].target else {
                    return Err(E::NotUrlSurface(surface_id.clone()));
                };
                if *cur != *url {
                    *cur = url.clone();
                    touched.extend(p.pages_showing(surface_id));
                }
            }
            ProjectPatchOp::SetSurfaceFrame { surface_id, frame } => {
                if !frame.valid() {
                    continue;
                }
                let i = p.surface_index(surface_id)?;
                if p.surface_specs[i].frame != Some(*frame) {
                    p.surface_specs[i].frame = Some(*frame);
                    touched.extend(p.pages_showing(surface_id));
                }
            }
            ProjectPatchOp::SetSurfaceWindow { surface_id, title, bundle_id } => {
                let i = p.surface_index(surface_id)?;
                let SurfaceTarget::App { bundle_id: app, file } = &mut p.surface_specs[i].target else {
                    return Err(E::Invalid(vec!["Only an app's surface has windows".into()]));
                };
                let title = title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
                // MIRROR-FRAME-1101: bundle id が在れば app も替える（空の枠に窓を入れる）・無ければ今の app のまま
                let next = match bundle_id.as_deref().map(str::trim) {
                    Some("") => return Err(E::Invalid(vec!["Choose an app".into()])),
                    Some(b) => b.to_owned(),
                    None => app.clone(),
                };
                if *file != title || *app != next {
                    (*file, *app) = (title, next);
                    touched.extend(p.pages_showing(surface_id));
                }
            }
            ProjectPatchOp::RenameProject { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(E::Invalid(vec!["A project needs a name".into()]));
                }
                p.name = name.to_owned();
            }
            ProjectPatchOp::SetWorkspace { dir } => {
                if !dir.starts_with('/') {
                    return Err(E::Invalid(vec![format!("The folder isn't a full path: {dir}")]));
                }
                let uri = Some(format!("file://{dir}"));
                match p.resources.iter_mut().find(|r| r.id == "repo") {
                    Some(r) => r.uri = uri,
                    None => p.resources.push(ProjectResource { id: "repo".into(), kind: "repo".into(), uri }),
                }
                for t in &mut p.terminal_specs {
                    t.working_directory = dir.clone();
                }
            }
        }
    }
    for page in p.pages.iter_mut().filter(|pg| touched.contains(&pg.id)) {
        page.revision += 1;
    }
    p.revision += 1;
    Ok(p)
}

/// W16M-3: AddSurface の保存形式と apply（AddTerminal と同じ形・source_surface / surface_ref）。
#[cfg(test)]
mod w16m3_interface {
    use super::*;
    fn notes_spec() -> SurfaceSpec {
        SurfaceSpec {
            id: "notes".into(),
            backend: SurfaceBackendKind::Mirror,
            target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) },
            keep_alive: true,
            frame: None,
            browser: false,
        }
    }
    #[test]
    fn w16m3_add_surface_round_trips_and_rejects_unknown_fields() {
        let op = ProjectPatchOp::AddSurface {
            spec: notes_spec(),
            page_id: "page.notes".into(),
            parent: "root".into(),
            node_key: "surface.notes".into(),
            index: None,
        };
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!(v["op"], "add_surface", "{v}");
        assert!(v.get("index").is_none(), "index 無しは書かない: {v}");
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(v.clone()).unwrap(), op);
        let mut extra = v;
        extra["frame"] = serde_json::json!({});
        assert!(serde_json::from_value::<ProjectPatchOp>(extra).is_err(), "未知の欄は拒否");
        assert!(!ProjectPatchError::DuplicateSurface("notes".into()).to_string().contains("notes"), "id を画面の文に出さない");
    }

    /// RQ4 interface（caller の形・本体の前は todo!() で赤）: 窓を選び直すと面の `file` だけが替わり、同じ値は revision を足さず、Url の面と無い id は断る。保存の形も往復する。
    #[test]
    fn rq4_set_surface_window_changes_only_the_title_word() {
        let op = ProjectPatchOp::SetSurfaceWindow { surface_id: "notes".into(), title: Some("todo.md".into()), bundle_id: None };
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!(v["op"], "set_surface_window", "{v}");
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(v).unwrap(), op);
        let p = ProjectDefinitionV1 {
            id: "project.t".into(),
            name: "t".into(),
            goal: "t".into(),
            app_refs: vec!["app.t".into()],
            pages: vec![PageDefinition { id: "page.t.1".into(), route: "/".into(), title: "t".into(), app_ref: Some("app.t".into()), root: UiNode { key: "root".into(), component: "column".into(), ..UiNode::default() }, revision: 1 }],
            resources: vec![],
            terminal_specs: vec![],
            surface_specs: vec![notes_spec(), SurfaceSpec { id: "web".into(), target: SurfaceTarget::Url { url: "https://example.com".into() }, ..notes_spec() }],
            revision: 1,
        };
        let next = apply_unchecked(&p, &[op.clone()]).expect("窓を選び直せる");
        assert_eq!(next.surface_specs[0].target, SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("todo.md".into()) });
        assert_eq!(next.surface_specs[1], p.surface_specs[1], "他の面を変えた");
        let again = apply_unchecked(&next, &[op]).unwrap();
        assert_eq!(again.surface_specs, next.surface_specs);
        assert!(apply_unchecked(&p, &[ProjectPatchOp::SetSurfaceWindow { surface_id: "web".into(), title: None, bundle_id: None }]).is_err(), "Url の面に窓を選べた");
        assert!(apply_unchecked(&p, &[ProjectPatchOp::SetSurfaceWindow { surface_id: "ghost".into(), title: None, bundle_id: None }]).is_err());
    }

    /// MIRROR-FRAME-1101 interface（caller の形・本体の前は ignore）: 空の枠（bundle id が空の Mirror の面）に窓を入れると app と題の語が 1 回で入り、
    /// 空の枠でなくなる。bundle id を渡さない手は app を替えない（今までの保存の形 = 欄が無い）。空白だけの bundle id は断る。
    /// 負の対照: 本体の `bundle_id` を当てる 3 行を外す → 1 つ目の assert が赤。
    #[test]
    fn mirror_frame_1101_a_blank_frame_takes_an_app_and_a_title() {
        let blank = SurfaceSpec { id: "frame.1".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: String::new(), file: None }, keep_alive: true, frame: Some(SurfaceFrame { x: 0.0, y: 0.0, w: 640.0, h: 400.0 }), browser: false };
        assert!(blank.blank_frame() && !notes_spec().blank_frame());
        let p = ProjectDefinitionV1 {
            id: "project.t".into(),
            name: "t".into(),
            goal: "t".into(),
            app_refs: vec!["app.t".into()],
            pages: vec![PageDefinition { id: "page.t.1".into(), route: "/".into(), title: "t".into(), app_ref: Some("app.t".into()), root: UiNode { key: "root".into(), component: "column".into(), ..UiNode::default() }, revision: 1 }],
            resources: vec![],
            terminal_specs: vec![],
            surface_specs: vec![blank.clone()],
            revision: 1,
        };
        let op = ProjectPatchOp::SetSurfaceWindow { surface_id: "frame.1".into(), title: Some("Docs".into()), bundle_id: Some("com.google.Chrome".into()) };
        let next = apply_unchecked(&p, &[op.clone()]).expect("空の枠に窓を入れられる");
        assert_eq!(next.surface_specs[0].target, SurfaceTarget::App { bundle_id: "com.google.Chrome".into(), file: Some("Docs".into()) });
        assert!(!next.surface_specs[0].blank_frame());
        assert_eq!(next.surface_specs[0].frame, blank.frame, "枠の大きさが替わった");
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!((v["bundle_id"].as_str(), serde_json::from_value::<ProjectPatchOp>(v.clone()).unwrap()), (Some("com.google.Chrome"), op));
        let old = serde_json::to_value(ProjectPatchOp::SetSurfaceWindow { surface_id: "frame.1".into(), title: None, bundle_id: None }).unwrap();
        assert!(old.get("bundle_id").is_none(), "None を保存に書いた: {old}");
        let kept = apply_unchecked(&next, &[ProjectPatchOp::SetSurfaceWindow { surface_id: "frame.1".into(), title: Some("Mail".into()), bundle_id: None }]).unwrap();
        assert_eq!(kept.surface_specs[0].target, SurfaceTarget::App { bundle_id: "com.google.Chrome".into(), file: Some("Mail".into()) }, "bundle id の無い手が app を替えた");
        assert!(apply_unchecked(&p, &[ProjectPatchOp::SetSurfaceWindow { surface_id: "frame.1".into(), title: None, bundle_id: Some("  ".into()) }]).is_err(), "空白の app を入れた");
    }

    #[test]
    fn w16m3_add_surface_places_source_surface_and_refuses_duplicate_id() {
        let p = ProjectDefinitionV1 {
            id: "project.t".into(),
            name: "t".into(),
            goal: "t".into(),
            app_refs: vec!["app.t".into()],
            pages: vec![PageDefinition {
                id: "page.t.1".into(),
                route: "/".into(),
                title: "t".into(),
                app_ref: Some("app.t".into()),
                root: UiNode { key: "root".into(), component: "column".into(), ..UiNode::default() },
                revision: 1,
            }],
            resources: vec![],
            terminal_specs: vec![],
            surface_specs: vec![],
            revision: 1,
        };
        let op = |id: &str, page: &str| ProjectPatchOp::AddSurface {
            spec: SurfaceSpec { id: id.into(), ..notes_spec() },
            page_id: page.into(),
            parent: "root".into(),
            node_key: format!("surface.{id}"),
            index: None,
        };
        let next = apply_unchecked(&p, &[op("notes", "page.t.1")]).expect("AddSurface が当たる");
        assert_eq!(next.surface_specs, [notes_spec()]);
        let n = next.pages[0].root.nodes().into_iter().find(|n| n.component == "source_surface").expect("source_surface");
        assert_eq!(n.key, "surface.notes");
        assert_eq!(n.props.get("surface_ref").and_then(|v| v.as_str()), Some("notes"));
        assert!(matches!(apply_unchecked(&next, &[op("notes", "page.t.1")]), Err(ProjectPatchError::DuplicateSurface(_))));
        assert!(matches!(apply_unchecked(&p, &[op("notes", "nope")]), Err(ProjectPatchError::PageNotFound(_))));
    }
}

/// W12W-B の op の保存形式（serde の往復と未知の欄の拒否）と apply の形。
#[cfg(test)]
mod w12wb_interface {
    use super::*;
    fn browser() -> SurfaceSpec {
        SurfaceSpec {
            id: "browser.1".into(),
            backend: SurfaceBackendKind::WebView,
            target: SurfaceTarget::Url { url: "https://www.google.com/".into() },
            keep_alive: true,
            frame: Some(SurfaceFrame { x: 24.0, y: 80.0, w: 640.0, h: 480.0 }),
            browser: true,
        }
    }
    #[test]
    fn w12wb_spec_round_trips_frame_and_browser_and_old_json_still_reads() {
        let spec = browser();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["browser"], true, "{v}");
        assert_eq!(v["frame"]["w"], 640.0, "{v}");
        assert_eq!(serde_json::from_value::<SurfaceSpec>(v.clone()).unwrap(), spec);
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("frame");
        old.as_object_mut().unwrap().remove("browser");
        let back = serde_json::from_value::<SurfaceSpec>(old).unwrap();
        assert_eq!((back.frame, back.browser), (None, false), "W12W-1 の json は frame 無し・Browser ではない");
        let mut extra = v;
        extra["pid"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SurfaceSpec>(extra).is_err(), "handle は持てない");
        assert!(!SurfaceFrame { x: 0.0, y: 0.0, w: 0.0, h: 10.0 }.valid(), "幅 0 は置かない");
    }
    #[test]
    fn w12wb_set_url_and_frame_round_trip_and_reject_unknown_fields() {
        let url = ProjectPatchOp::SetSurfaceUrl { surface_id: "browser.1".into(), url: "https://www.youtube.com/watch?v=x".into() };
        let frame = ProjectPatchOp::SetSurfaceFrame { surface_id: "browser.1".into(), frame: SurfaceFrame { x: 8.0, y: 16.0, w: 400.0, h: 300.0 } };
        let vu = serde_json::to_value(&url).unwrap();
        let vf = serde_json::to_value(&frame).unwrap();
        assert_eq!(vu["op"], "set_surface_url", "{vu}");
        assert_eq!(vf["op"], "set_surface_frame", "{vf}");
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(vu.clone()).unwrap(), url);
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(vf.clone()).unwrap(), frame);
        let mut extra = vu;
        extra["allow"] = serde_json::json!([]);
        assert!(serde_json::from_value::<ProjectPatchOp>(extra).is_err(), "未知の欄は拒否");
        assert!(!ProjectPatchError::NotUrlSurface("browser.1".into()).to_string().contains("browser"), "id を画面の文に出さない");
    }

    /// AI-COMPOSE-1101: AI が書く部品は html が要り上限の中・値の 1 手は props を全部送り同じ値は送らない。ENGINE-GENERIC-1101: きっかけ（`on`）は同じ Page の
    /// 部品の手（`TRIGGER_ACTS` の component と手）を指し、key が在る時だけ intent が {value} を持つ・手の後に走る物を木から集める・前の `before` の欄は読まない。
    /// 負の対照（未実行）: validate の canvas の腕を外す → bad の assert で赤・`set_patch` の同じ値の判定を外す → None の assert で赤・
    /// `canvas_fault` の component の照合を外す → `title.recorded` の bad が通って赤。
    #[test]
    fn ai_compose_canvas_part_keeps_values_and_triggers() {
        use crate::domain::ui::{triggers_on, CanvasProps, PartTrigger, CANVAS_HTML_MAX};
        use serde_json::json;
        let canvas = |props: serde_json::Value| UiNode { key: "guitar".into(), component: "canvas".into(), props: props.as_object().unwrap().clone(), ..UiNode::default() };
        let take = UiNode { key: "take".into(), component: "recorder".into(), ..UiNode::default() };
        let title = UiNode { key: "title".into(), component: "text".into(), ..UiNode::default() };
        let with = |c: UiNode| {
            let mut p = blank();
            p.pages[0].root.children = vec![c, take.clone(), title.clone()];
            p
        };
        let trig = json!([{"act": "take.recorded", "key": "guitar", "intent": "Set the track's sound to {value}"}]);
        let ok = with(canvas(json!({"html": "<select></select>", "values": {"guitar": "Clean"}, "on": trig})));
        assert_eq!(validate_project(&ok, &[]), Ok(()));
        // 値を使わないきっかけ（key が空・{value} 無し）と、前の data の `before`（読まない = 起きない・拒否もしない）
        let plain = json!([{"act": "take.recorded", "intent": "Put the take on the selected track"}]);
        assert_eq!(validate_project(&with(canvas(json!({"html": "x", "on": plain}))), &[]), Ok(()));
        assert_eq!(validate_project(&with(canvas(json!({"html": "x", "before": [{"act": "take.add", "key": "guitar", "intent": "{value}"}]}))), &[]), Ok(()));
        for bad in [
            json!({"html": "  "}),
            json!({"html": "x".repeat(CANVAS_HTML_MAX + 1)}),
            json!({"on": []}),
            json!({"html": "x", "on": [{"act": "nope.recorded", "key": "guitar", "intent": "{value}"}]}),
            json!({"html": "x", "on": [{"act": "take.press", "key": "guitar", "intent": "{value}"}]}),
            json!({"html": "x", "on": [{"act": "take.add", "key": "guitar", "intent": "{value}"}]}),
            json!({"html": "x", "on": [{"act": "title.recorded", "key": "guitar", "intent": "{value}"}]}),
            json!({"html": "x", "on": [{"act": "take.recorded", "key": "guitar", "intent": "no hole"}]}),
            json!({"html": "x", "on": [{"act": "take.recorded", "key": "", "intent": "Use {value}"}]}),
            json!({"html": "x", "on": [{"act": "take.recorded", "key": "", "intent": "  "}]}),
        ] {
            assert!(validate_project(&with(canvas(bad.clone())), &[]).is_err(), "通した: {bad}");
        }
        let node = &ok.pages[0].root.children[0];
        assert_eq!(CanvasProps::set_patch(node, "guitar", json!("Clean")), None, "同じ値で revision を足す手を作った");
        let patch = CanvasProps::set_patch(node, "guitar", json!("Acoustic Guitar")).expect("値の 1 手");
        let mut root = ok.pages[0].root.clone();
        apply_one(&mut root, &patch).unwrap();
        let after = CanvasProps::of(root.find("guitar").unwrap()).unwrap();
        assert_eq!(after.values.get("guitar"), Some(&json!("Acoustic Guitar")));
        assert_eq!((after.html.as_str(), after.on.len()), ("<select></select>", 1), "html ときっかけを落とした");
        let got = triggers_on(&root, "take.recorded");
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].2.clone()), ("guitar", Some(json!("Acoustic Guitar"))));
        assert_eq!(got[0].1.goal(got[0].2.as_ref().unwrap()), "Set the track's sound to Acoustic Guitar");
        assert!(triggers_on(&root, "take.discard").is_empty());
        let fresh = with(canvas(json!({"html": "x", "on": trig})));
        assert_eq!(triggers_on(&fresh.pages[0].root, "take.recorded")[0].2, None, "まだ選んでいない値を作った");
        assert_eq!(PartTrigger { act: "a.recorded".into(), key: "k".into(), intent: "n={value}".into() }.goal(&json!(3)), "n=3");
    }

    fn blank() -> ProjectDefinitionV1 {
        ProjectDefinitionV1 {
            id: "project.t".into(),
            name: "t".into(),
            goal: "g".into(),
            app_refs: vec![],
            pages: vec![PageDefinition {
                id: "page.a".into(),
                route: "/".into(),
                title: "A".into(),
                app_ref: None,
                root: UiNode { key: "root".into(), component: "column".into(), ..UiNode::default() },
                revision: 1,
            }],
            resources: vec![],
            terminal_specs: vec![],
            surface_specs: vec![],
            revision: 1,
        }
    }

    fn add_browser() -> ProjectPatchOp {
        ProjectPatchOp::AddSurface {
            spec: SurfaceSpec {
                id: "b1".into(),
                backend: SurfaceBackendKind::WebView,
                target: SurfaceTarget::Url { url: "https://www.google.com/".into() },
                keep_alive: true,
                frame: None,
                browser: true,
            },
            page_id: "page.a".into(),
            parent: "root".into(),
            node_key: "surface.b1".into(),
            index: None,
        }
    }

    #[test]
    fn w12wb_add_surface_set_url_set_frame_apply_and_undo_shape() {
        let p = apply(&blank(), &[add_browser()], &[]).expect("Browser を置ける");
        assert_eq!(p.surface_specs.len(), 1);
        assert!(p.surface_specs[0].browser);
        assert_eq!(p.pages[0].root.children[0].component, "source_surface");
        assert_eq!(p.pages[0].root.children[0].props["surface_ref"], "b1");
        let q = apply(&p, &[ProjectPatchOp::SetSurfaceUrl { surface_id: "b1".into(), url: "https://www.youtube.com/watch?v=x".into() }], &[]).expect("URL を替えられる");
        match &q.surface_specs[0].target {
            SurfaceTarget::Url { url } => assert_eq!(url, "https://www.youtube.com/watch?v=x"),
            other => panic!("{other:?}"),
        }
        let r = apply(&q, &[ProjectPatchOp::SetSurfaceFrame { surface_id: "b1".into(), frame: SurfaceFrame { x: 10.0, y: 20.0, w: 400.0, h: 300.0 } }], &[]).expect("frame を残せる");
        assert_eq!(r.surface_specs[0].frame, Some(SurfaceFrame { x: 10.0, y: 20.0, w: 400.0, h: 300.0 }));
        // 同じ URL / 同じ frame は中身が変わらない（revision は apply が 1 進める）
        let same = apply(&r, &[ProjectPatchOp::SetSurfaceUrl { surface_id: "b1".into(), url: "https://www.youtube.com/watch?v=x".into() }], &[]).unwrap();
        assert_eq!(same.surface_specs[0].target, r.surface_specs[0].target);
        let bad = apply(&r, &[ProjectPatchOp::SetSurfaceFrame { surface_id: "b1".into(), frame: SurfaceFrame { x: 0.0, y: 0.0, w: 0.0, h: 10.0 } }], &[]).unwrap();
        assert_eq!(bad.surface_specs[0].frame, r.surface_specs[0].frame, "valid でない frame は残さない");
        assert!(matches!(apply(&r, &[ProjectPatchOp::SetSurfaceUrl { surface_id: "nope".into(), url: "https://example.com/".into() }], &[]), Err(ProjectPatchError::SurfaceNotFound(_))));
        let mut app = r.clone();
        app.surface_specs[0].target = SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: None };
        assert!(matches!(apply(&app, &[ProjectPatchOp::SetSurfaceUrl { surface_id: "b1".into(), url: "https://example.com/".into() }], &[]), Err(ProjectPatchError::NotUrlSurface(_))));
        assert!(matches!(apply(&p, &[add_browser()], &[]), Err(ProjectPatchError::DuplicateSurface(_))));
    }
}
