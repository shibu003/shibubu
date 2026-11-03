//! W16-1: Native Direct の面（TextEdit で README.md）を Docs の Page の枠に重ねる口。feature `native_direct` の時だけ入る
//! （W15 の .app は feature を切り、面は W3 の `surface-status` の 1 行のまま）。図は docs/diagrams.md §30。
//!
//! 流れ（呼ぶ側の使い方）:
//! - `Generated` が `surfaces: NativeSurfaces` を持つ（保存しない・再起動したら空から）
//! - `draw_page` が描く前に `view(id)` を `Host` に渡し、renderer が `portal_frame::show` で枠を描いて `surface_placed` で枠の画面座標を返す。
//!   描いた後に `draw_page` がそれを `on_frame` へ流す（Host は `&AppRuntime` を借りているので `&mut NativeSurfaces` を渡さない）
//! - Page を離れる `save_view_state` → `hide_page`（HostHidden）・入る `activate` → `show_page`（HostShown）
//! - 公開 API だけ: 窓の位置は AX（bridge の `set_frame`）・前後は `orderWindow:relativeTo:`（`surface::order_host_below` / `order_host_above`）。
//!   非公開 API・CuaDriver・Orca は使わない。前面を取る手（activate・`open` で app を起こす）は打たない
use crate::domain::project::{SurfaceBackendKind, SurfaceReceipt, SurfaceSpec, SurfaceTarget};
use crate::surface::{self, AgentAction, AppIdentity, Bridge, BridgeError, Dock, Frame, Host, LifeEvent, Lifecycle, Msg, PortalState, Reconnect, WindowInfo};
use crate::ui::portal_frame::{Mode, PortalAction, PortalView};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// bridge の ready を待つ上限（UI の thread で 1 回だけ待つ・app.rs の create_project と同じ 5 秒）。
const READY_LIMIT: Duration = Duration::from_secs(5);

/// 重ねた記録の file の名前（data dir の直下・1 行 1 件の JSONL・追記だけ）。
pub const OVERLAY_LOG: &str = "overlay.jsonl";

/// 重ねた記録の file の場所。
/// 例: `NativeSurfaces::new(Some(native::record_path(dir)))`（`Generated::open(dir)` の dir = `SHIBUBU_DATA_DIR`）。試験の側はこの file を読んで oracle と照合する。
pub fn record_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OVERLAY_LOG)
}

/// W16-3: 同じ窓へ繋ぎ直すための identity の JSONL（data dir の直下・1 行 1 件・追記だけ）。重ねた記録（`OVERLAY_LOG`）とは別（あちらは照合の相手で、pid を再起動の後の正にしない）。
/// Wave: W16-3
pub const IDENTITY_LOG: &str = "overlay-identity.jsonl";

/// `OverlayIdentity.schema` の値。未知の欄は拒否（`deny_unknown_fields`）。
/// Wave: W16-3
pub const IDENTITY_SCHEMA: &str = "shibubu.native.identity/1";

/// identity の file の場所（`record_path` と同じ data dir）。
/// 例: `NativeSurfaces::new(Some(record_path(dir)))` の呼ぶ側は変えない。restore は `identity_path(dir)` を読む。
/// Wave: W16-3
pub fn identity_path(data_dir: &Path) -> PathBuf {
    data_dir.join(IDENTITY_LOG)
}

/// 重ねた記録の出来事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayEvent {
    /// 窓を決めて枠へ合わせ始めた（Lifecycle が active に入った）
    Attached,
    /// 面の Page に入った（`activate` → HostShown）。窓を Shibubu の真上に戻した後
    HostShown,
    /// 面の Page を離れた（`save_view_state` → HostHidden）。Shibubu を窓の真上に置いた後
    HostHidden,
    /// 重なっていない（枠から外れた・答えない・許可が外れた）
    Degraded,
    /// 元の app が終了した・窓が消えた
    SourceGone,
    /// W16-3: Shibubu を起こし直した後、同じ pid・window_id の窓に繋ぎ直した（新しい窓への Attached ではない）
    Reconnected,
}

/// 重ねた記録 1 件（製品の自己申告。正は試験の側の `oracle/windows.swift` で、これは照合の相手）。
/// JSON の欄の名前は試験の側との契約: `surface_id`・`page_id`・`event`・`at_ms`・`bundle_id`・`pid`・`window_id`・`frame{x,y,w,h}`。
/// 例: `{"surface_id":"docs","page_id":"page.docs","event":"host_shown","at_ms":1760000000000.0,"bundle_id":"com.apple.TextEdit","pid":123,"window_id":456,"frame":{"x":10,"y":80,"w":800,"h":600}}`
/// 保証する: `window_id` は bridge が CGWindowList で 1 つに決められた時だけ（決められなければ null・作らない）・`frame` は送った枠（global の point・Retina の倍率は掛けない）・`at_ms` は UNIX epoch の ms。
/// 保証しない: 窓が本当にそこに在った事（試験の側の標本で確かめる）。
/// Wave: W16-1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayRecord {
    pub surface_id: String,
    pub page_id: String,
    pub event: OverlayEvent,
    pub at_ms: f64,
    pub bundle_id: String,
    pub pid: Option<u32>,
    pub window_id: Option<i64>,
    pub frame: Option<Frame>,
}

impl OverlayRecord {
    /// file の末尾に 1 行足す（無ければ作る）。書けなければ Err（呼ぶ側は記録できない事を `note` に 1 行出し、重ねるのは止めない）。
    pub fn append(&self, path: &Path) -> std::io::Result<()> {
        let line = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{line}")
    }
}

/// W16-3: 重ねた窓の identity（Shibubu の再起動をまたいで同じ窓へ繋ぐ。TextEdit の pid・window_id は Shibubu を kill しても変わらない）。
/// JSON の欄の名前は契約: `schema`・`project_id`・`surface_id`・`page_id`・`bundle_id`・`pid`・`window_id`・`title`。
/// 例: `if let Some(id) = g.surfaces.identity(&p.id, "docs") { id.append(&native::identity_path(dir))?; }`
/// 保証する: `identity()` が Some を返すのは Lifecycle が Active / Hidden / Resizing で pid と window_id が両方在る時だけ（終了した app・決めていない窓は None＝「接続済み」「直接操作中」の根拠にしない）。
/// 保証しない: 書いた行の窓が今も生きている事（`restore` が bridge で確かめる）。
/// Wave: W16-3
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayIdentity {
    pub schema: String,
    pub project_id: String,
    pub surface_id: String,
    pub page_id: String,
    pub bundle_id: String,
    pub pid: u32,
    pub window_id: i64,
    pub title: String,
}

impl OverlayIdentity {
    /// file の末尾に 1 行足す（無ければ作る）。書けなければ Err（呼ぶ側は繋ぎ直しを止めない）。
    pub fn append(&self, path: &Path) -> std::io::Result<()> {
        let line = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")
    }
}

/// W16-3: 元の窓を今の Space の枠に重ねられない理由（非公開 API で連れて来ない。degraded と「外で開く」の 1 行）。
/// Wave: W16-3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffSpaceKind {
    /// Shibubu が全画面（元の窓は前の Space に残る）
    HostFullscreen,
    /// 元の窓が全画面
    SourceFullscreen,
    /// 元の窓は今の Space に無い（最小化はこれではない）
    DifferentSpace,
}

impl OffSpaceKind {
    /// 画面に出す理由の短い句（`NativeError::OffSpace` の括弧の中）。成功の語を含まない。
    /// 例: `format!("{} を重ねられません（{}）。外で開くで使ってください", app, kind.line())`
    /// 保証する: 3 種とも 1 行・次の一手は呼ぶ側の `NativeError` Display が足す。保証しない: 今の Space へ窓を移す事。
    /// 契約（UI-2 から・署名は変えない）: 表の EN（additions.md）`Shibubu is full screen. The original window stays in the previous Space`・
    /// `The original window is full screen`・`The original window isn't in this Space`。`NativeError` の Display も同じ表の EN（括弧の句はこの line）。
    /// Wave: W16-3, UI-2
    pub fn line(self) -> &'static str {
        match self {
            OffSpaceKind::HostFullscreen => "Shibubu is full screen. The original window stays in the previous Space",
            OffSpaceKind::SourceFullscreen => "The original window is full screen",
            OffSpaceKind::DifferentSpace => "The original window isn't in this Space",
        }
    }
}

/// 面を重ねられない理由（Display = 画面に出す 1 行と次の一手・D-e。成功の語〈接続済み・済み・完了 等〉を含まない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeError {
    /// spec が Native Direct の App ではない（Mirror・WebView・Url）
    NotNative,
    /// bridge（`kangae-surface-bridge`）が見つからない・起きない・止まった
    Bridge(String),
    /// 元の app が起動していない。製品は app を起こさない（`open -g` でも TextEdit は前面を取る・W17-1/2）
    NotRunning { app: String },
    /// Shibubu にアクセシビリティの許可が無い
    NoAccessibility,
    /// 起動中の app に、その file を開いた窓が無い
    NoWindow { app: String, file: String },
    /// W16-2: 読み戻す面がまだ Page に重なっていない（窓が決まっていないので AX の文書を読めない）
    NotAttached { app: String },
    /// W16-2: 面の file（Project の working dir からの相対）を disk で読めない（spec に file が無い・repo の場所が無い・無い・dir・権限）
    FileUnreadable { file: String, why: String },
    /// W16-2: 重ねた窓に文書の欄（AXTextArea）が無い（保存前の中身は読めない。保存した中身は disk で読む）
    NoTextArea { app: String },
    /// W16-3: 元の窓を今の Space の枠に重ねられない（全画面・別 Space）。view は IntegrationFailed。
    /// Wave: W16-3
    OffSpace { app: String, kind: OffSpaceKind },
    /// W16-3: 外から app が終了した・identity の pid が生きていない。view は SourceUnavailable（「接続済み」「直接操作中」と出さない）。
    /// Wave: W16-3
    Gone { app: String },
    /// W16-3: identity の pid は生きているが、保存した window_id の窓は無い（元の窓が閉じた。終了した app の Gone ではない。別の窓にすり替えない）。
    /// Wave: W16-3
    WindowClosed { app: String, file: String },
    /// W16-3: 同じ題名の窓が複数あり、どれを重ねるか決められない（すり替えない・L4）。
    /// Wave: W16-3
    AmbiguousWindow { app: String, file: String },
    /// W16-3: 人が操作中なので Agent のその一手はしない（PORTAL-09・`ControlLease::agent_may`）。
    /// Wave: W16-3
    HumanHasControl { app: String, action: AgentAction },
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeError::NotNative => write!(f, "This screen can't be placed over a page"),
            NativeError::Bridge(why) => write!(f, "The part that overlays windows isn't running ({why}). Reopen Shibubu to try again"),
            NativeError::NotRunning { app } => write!(f, "{app} isn't running. Open {app} first, then come back to this page"),
            NativeError::NoAccessibility => write!(f, "Accessibility access is needed. Turn Shibubu on in System Settings, then come back to this page"),
            NativeError::NoWindow { app, file } => write!(f, "{file} isn't open in {app}. Open {file} in {app}, then come back to this page"),
            NativeError::NotAttached { app } => write!(f, "{app}'s window isn't over this page yet. Open a document in {app}, then come back to this page"),
            NativeError::FileUnreadable { file, why } => write!(f, "Couldn't read {file} ({why}). In the repo, {file} {}", crate::domain::project::FILE_UNREADABLE_NEXT),
            NativeError::NoTextArea { app } => write!(f, "No document field found in {app}'s window. Unsaved content can't be read; save in {app} and Shibubu reads it back from disk"),
            NativeError::OffSpace { app, kind } => write!(f, "Couldn't overlay {app} ({}). Use Open in {app}", kind.line()),
            NativeError::Gone { app } => write!(f, "{app} has quit. Reopen {app}, then come back to this page"),
            NativeError::WindowClosed { app, file } => write!(f, "{app}'s original window was closed. Reopen {file} in {app}, then come back to this page"),
            NativeError::AmbiguousWindow { app, file } => write!(f, "{file} is open in several {app} windows and Shibubu can't tell which to cover. Keep only the one you want, then come back to this page"),
            NativeError::HumanHasControl { app, action } => {
                let what = match action {
                    AgentAction::Input => "type",
                    AgentAction::Focus => "move focus",
                    AgentAction::MoveWindow => "move the window",
                };
                write!(f, "You are working in {app}, so the agent won't {what}. Press Hand over to let it")
            }
        }
    }
}

impl From<BridgeError> for NativeError {
    fn from(e: BridgeError) -> Self {
        NativeError::Bridge(e.to_string())
    }
}

/// W16-2: 面の Page を描いている間に読み戻す間隔（`Generated::draw_page` が前の読みからこれだけ経った frame で `readback_native` を 1 回）。
/// 他に、Page を離れる `save_view_state` の前と、戻った `activate` の後に 1 回ずつ（A2: 移る前と戻った後の文書）。
pub const READBACK_EVERY: Duration = Duration::from_secs(2);

/// ax_text の応答を UI の thread で待つ上限。越えたら待たずに戻り、応答は後の pump で拾う（その面には応答が来るまで 2 つ目を送らない）。
/// 実測（2026-10-21 23:35・load 93 / 8 CPU）: 背景の TextEdit の ax_text が 16 秒。同期で待つと UI が止まり、2 秒ごとに送ると bridge の列が伸び続けた。
const AX_BUDGET: Duration = Duration::from_millis(300);
/// 応答の来ない ax_text をあきらめて送り直すまで（bridge が要求を落とした時に読み戻しが止まったままにしない）。
const AX_STALE: Duration = Duration::from_secs(60);
/// W16-3: 今の Space・全画面・最小化（`refresh_space`）を読み直す間隔。応答は待たず後の pump で拾い、応答待ちの間は送らない。
/// 反証の直し: on_frame ごとに AX の `frame` を同期で待つと、nap の TextEdit（W16-2 実機で AX 13〜31 秒）で UI が frame ごとに止まる。
/// 間隔は読み戻しと同じ（bridge は 1 本の列で、`frame` は元の窓の AX を約 10 回読む。短くすると nap の間 set_frame と ax_text の前に居続ける。
/// OffSpace は帯の 1 行なので 2 秒遅れてよい）。
const SPACE_EVERY: Duration = READBACK_EVERY;

/// bridge の ax_text の応答 → 窓の文書か理由。
fn ax_reply(app: &str, r: Result<serde_json::Value, BridgeError>) -> Result<String, NativeError> {
    let v = r.map_err(|e| match e {
        BridgeError::Failed { code, .. } if code == "not_trusted" => NativeError::NoAccessibility,
        e => e.into(),
    })?;
    match (v["text"].as_str(), v["reason"].as_str()) {
        (Some(t), _) => Ok(t.to_string()),
        (None, Some("ax_no_text_area")) => Err(NativeError::NoTextArea { app: app.to_string() }),
        (None, why) => Err(NativeError::Bridge(format!("Couldn't read the {app} document ({})", why.unwrap_or("No reason given")))),
    }
}

/// verified の oracle（重ねた窓の文書の sha と disk の sha が同じ・その前の読みから disk が変わった）。
const SAVED: &str = "file_sha+ax_text";

/// 窓の文書 `text` が disk の sha `disk` と同じ中身か（末尾の改行 1 つの差は同じと見る: TextEdit の AXValue と保存した file で末尾の `\n` の有無が違う）。
fn same_as_disk(text: &str, disk: &str) -> bool {
    let sha = |s: &str| crate::runtime::permission::sha256_hex(s.as_bytes());
    sha(text) == disk || sha(&format!("{text}\n")) == disk || text.strip_suffix('\n').is_some_and(|t| sha(t) == disk)
}

/// spec の file を repo の場所に足した path の sha256。読めなければ `FileUnreadable`（理由は画面に出す日本語・io の英語の原文を出さない）。
fn disk_sha(workspace: Option<&Path>, file: Option<&str>) -> Result<String, NativeError> {
    let unreadable = |why: &str| NativeError::FileUnreadable { file: file.unwrap_or("Document").to_string(), why: why.to_string() };
    let file = file.ok_or_else(|| unreadable("This screen has no file name"))?;
    let path = workspace.ok_or_else(|| unreadable("This project has no repo location"))?.join(file);
    if path.is_dir() {
        return Err(unreadable("It's a folder"));
    }
    match std::fs::read(&path) {
        Ok(bytes) => Ok(crate::runtime::permission::sha256_hex(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(unreadable("Not found")),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(unreadable("No permission to read it")),
        Err(_) => Err(unreadable("Can't open it")),
    }
}

/// W16-2: Native Direct の面を 1 回読み戻した物（PORTAL-03・04・14）。操作（人が窓の中で打つ・保存する・Undo）とは別の道 2 本で読む。
/// 計画の名前は `surface::readback(spec)` だが、surface は domain（`SurfaceSpec`・`SurfaceReceipt`）を引けないので、W2c の `project::readback` と同じ理由でここに置く。
/// 保存しない（`receipt` で `SurfaceReceipt` にして `surface_receipts` へ書く。pid と窓 id は `NativeSurfaces::window`・重ねた記録の側）。
/// 例: `let (before, now) = g.surfaces.readback(&p.id, spec, ws)?; if let Some(r) = now.receipt(before.as_ref(), &last, "README.md") { r.validate()?; /* put_surface_receipt */ }`
/// 保証する: 値は読めた物だけ（読めなければ Err と理由・空の文字や前の値で埋めない）。保証しない: 誰が書いたか（それは `receipt` が窓の文書と disk を比べて決める）・受け入れの A1（正は試験の側）。
#[derive(Debug, Clone, PartialEq)]
pub struct Readback {
    /// spec の `file` を Project の working dir に足した path の中身の sha256（小文字 16 進 64 字・`runtime::permission::sha256_hex`）。
    /// Err = `FileUnreadable`（spec に file が無い・repo の場所が無い・無い・dir・権限）。
    pub disk_hash: Result<String, NativeError>,
    /// 重ねた窓の最初の AXTextArea の AXValue（保存前の編集を含む・`KangaeMirror.swift` の `ax-text` と同じ読み）。
    /// Err = `NotAttached`（窓が決まっていない）・`NoTextArea`（文書の欄が無い app）・`Bridge`（bridge が答えない・AX の error の番号）。
    pub ax_value: Result<String, NativeError>,
    /// 読んだ時刻（`runtime::now_rfc3339` の形 `YYYY-MM-DDTHH:MM:SS.mmmZ`・UTC）。receipt の `verified_at` にそのまま入る。
    pub verified_at: String,
}

impl Readback {
    /// 前の読み `before`（同じ面の 1 つ前の `readback`）から今の読みへの変化を receipt 1 件にする。書く必要が無ければ None。
    /// `last` は その面の最新の receipt（`Generated::surface_receipts`。無ければ `project::unavailable`）で、surface_id・page_id・kind・app_id・app_version・os_version をそこから写す。
    /// `file` は operation の文に出す名前（`README.md`）。
    /// 決め方（上から最初に当たった物）:
    /// 1. verified: `before` と今の `disk_hash` が両方 Ok で違い、今の `ax_value` の UTF-8 の sha256 が今の `disk_hash` と同じ（末尾の改行 1 つの差は同じと見る）
    ///    = 重ねた窓の文書が disk に保存された。oracle `file_sha+ax_text`・operation `<file> の保存を disk で読み戻した`・evidence_ref `file_sha256:<今の disk_hash>`
    /// 2. unverified・oracle = `domain::project::NOT_ON_DISK`: 両方 Ok で disk は変わらず、窓の文書の sha256 が disk と違う（保存していない編集・Undo の後）
    /// 2b. unverified・oracle = `domain::project::CHANGED_OUTSIDE`: 両方 Ok で disk が変わり、窓の文書が disk と違う（外の process が file を書き換えた。保存で上書きさせる一手は出さない）。W16-2-b の決定。
    ///    `before` が無い時（kill → 再起動）は `last.evidence_ref` の `file_sha256:` を直前の disk とする（無いと外で書き換わった disk を NOT_ON_DISK にし、窓が新しい disk に追いついても古い verified が残る）。
    /// 3. unavailable・oracle = `disk_hash` の Err の 1 行そのまま: disk を読めない（renderer は `domain::project::FILE_UNREADABLE_NEXT` で終わる 1 行をそのまま出す）
    /// 4. unverified・oracle = `domain::project::IN_SYNC`: `last` が NOT_ON_DISK で、窓の文書が disk と同じに戻った（保存せずに Undo した。古い「保存していない変更」を残さない）
    ///    ＝ 74ec7fb の口から変えた所（74ec7fb は None）。計画の決定待ち（YC_WAVES の未決 W16-2-a）
    /// 5. それ以外（変わらない・AX だけ読めない）と、`last` と同じ state・oracle・evidence_ref になる時は None（同じ行を毎回足さない。保存 2 回目は sha が違うので書く）
    ///    ＝ evidence_ref を足したのは 74ec7fb の口から変えた所（74ec7fb は state と oracle）。同じく W16-2-a
    /// 読んだ時刻（`self.verified_at`）は、どの receipt の `verified_at` にもそのまま入る（74ec7fb の口）。
    /// `before` は比べる相手: `Generated::readback_native` は窓の文書を最後に読めた読み（`NativeSurfaces::seen`）を渡す（AX の応答待ちの読みで進めない）。
    /// 保証する: 返す receipt は必ず `validate` を通る（`last` の app_version か os_version が空なら 1 は unverified に下げ、oracle にその理由）・成功の語は verified の operation の外に出さない。
    /// 保証しない: 1 の保存が窓の中の人の手だった事（A1 の経路は試験の側の `oracle/windows` の標本で見る。食い違えば L1）。
    pub fn receipt(&self, before: Option<&Readback>, last: &SurfaceReceipt, file: &str) -> Option<SurfaceReceipt> {
        use crate::domain::project::{ReceiptState, CHANGED_OUTSIDE, IN_SYNC, NOT_ON_DISK};
        let mut r = SurfaceReceipt {
            state: ReceiptState::Unverified,
            oracle: String::new(),
            operation: String::new(),
            verified_at: self.verified_at.clone(),
            evidence_ref: String::new(),
            ..last.clone()
        };
        // この起動の before が空なら、前の起動の verified の disk（evidence_ref）を直前とする
        let prior = before
            .and_then(|b| b.disk_hash.as_ref().ok())
            .map(String::as_str)
            .or_else(|| last.evidence_ref.strip_prefix("file_sha256:"));
        match (&self.disk_hash, &self.ax_value) {
            (Err(e), _) => {
                r.state = ReceiptState::Unavailable;
                r.oracle = e.to_string();
            }
            (Ok(disk), Ok(text)) if same_as_disk(text, disk) => {
                let changed = prior.is_some_and(|b| b != disk);
                if !changed {
                    if last.oracle != NOT_ON_DISK && last.oracle != CHANGED_OUTSIDE {
                        return None;
                    }
                    r.oracle = IN_SYNC.into();
                } else if last.app_version.trim().is_empty() || last.os_version.trim().is_empty() {
                    r.oracle = format!("{SAVED}: couldn't read the app or OS version, so a save read back isn't marked confirmed");
                } else {
                    r.state = ReceiptState::Verified;
                    r.oracle = SAVED.into();
                    r.operation = format!("Read back the save of {file} from disk");
                    r.evidence_ref = format!("file_sha256:{disk}");
                }
            }
            (Ok(disk), Ok(_)) => {
                let changed = prior.is_some_and(|b| b != disk);
                r.oracle = if changed {
                    CHANGED_OUTSIDE.into()
                } else {
                    NOT_ON_DISK.into()
                };
            }
            (Ok(_), Err(_)) => return None,
        }
        let same = (r.state, r.oracle.as_str(), r.evidence_ref.as_str()) == (last.state, last.oracle.as_str(), last.evidence_ref.as_str());
        (!same).then_some(r)
    }
}

/// app の名前を聞く前の呼び名: bundle id の最後の段（com.apple.TextEdit → TextEdit）。
fn short_name(bundle_id: &str) -> String {
    bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_string()
}

/// 面の file（Project の working dir からの相対）の名前の部分（docs/README.md → README.md）。窓の題名で選ぶ時と、理由・operation の 1 行に使う。
pub fn file_name_of(file: &str) -> &str {
    Path::new(file).file_name().and_then(|n| n.to_str()).unwrap_or(file)
}

/// 起動中の app の窓から、`file_name`（README.md）を開いた窓を 1 つ選ぶ（title が file 名と同じか、file 名で始まる標準の窓）。
/// 保証する: 候補が 0 なら Err（`NoWindow` の元）・2 つ以上なら main の窓、それも無ければ Err（すり替えない・L4）。
/// Wave: W16-1
pub fn pick_document_window(list: &[WindowInfo], file_name: &str) -> Result<WindowInfo, String> {
    // 「README.md」と「README.md — 編集済み」は取り、「README.md.bak」は取らない
    let named = |w: &&WindowInfo| w.title == file_name || w.title.strip_prefix(file_name).is_some_and(|rest| rest.starts_with(' '));
    let hits: Vec<&WindowInfo> = list.iter().filter(|w| w.standard() && !w.minimized).filter(named).collect();
    let w = match hits.as_slice() {
        [] => return Err(format!("No window has {file_name} open")),
        [one] => *one,
        many => match many.iter().filter(|w| w.main).collect::<Vec<_>>().as_slice() {
            [main] => **main,
            _ => return Err(format!("{file_name} is open in {} windows, so Shibubu can't tell which to cover", many.len())),
        },
    };
    if w.fullscreen {
        return Err(format!("“{}” is full screen", w.title_text()));
    }
    if !w.movable || w.frame.is_none() {
        return Err(format!("Can't move or resize “{}” from outside", w.title_text()));
    }
    Ok(w.clone())
}

/// W16-3: 保存した identity と今動いている窓を突き合わせる。同じ pid・window_id の 1 つだけを取る（題名だけで `pick_document_window` しない・L4）。
/// 同じ題名が複数でも、保存した window_id が 1 つに決まればそれを返す。pid が居なければ `Gone`。pid は居て window_id が無いのは `WindowClosed`（終了してはいない）。同じ題名が複数なら `AmbiguousWindow`。
fn match_identity(
    saved: &OverlayIdentity,
    running_bundle: &str,
    running_pid: Option<u32>,
    list: &[WindowInfo],
) -> Result<WindowInfo, NativeError> {
    let app = short_name(&saved.bundle_id);
    if running_bundle != saved.bundle_id || running_pid != Some(saved.pid) {
        return Err(NativeError::Gone { app });
    }
    if let Some(w) = list
        .iter()
        .find(|w| w.pid == saved.pid && w.cg_id == Some(saved.window_id))
    {
        return Ok(w.clone());
    }
    match surface::pick_reconnect(&saved.bundle_id, &saved.title, running_bundle, list) {
        Reconnect::Ask(_) => Err(NativeError::AmbiguousWindow {
            app,
            file: saved.title.clone(),
        }),
        _ => Err(NativeError::WindowClosed {
            app,
            file: saved.title.clone(),
        }),
    }
}

/// 面の鍵: (project_id, surface_id)。composer は Project ごとに同じ surface id（"docs"）を作るので、surface id だけでは別の Project の面と混ざる。
type Key = (String, String);

fn key(project_id: &str, surface_id: &str) -> Key {
    (project_id.to_string(), surface_id.to_string())
}

/// Native Direct の面ごとの今（保存しない）。`Generated.surfaces`。
/// Wave: W16-1
pub struct NativeSurfaces {
    /// 常駐の bridge（最初に面の Page を描いた時に `Bridge::spawn`。起きなければ None のまま理由を `errors` に）
    bridge: Option<Bridge>,
    /// (project_id, surface_id) → 面の Dock と、その面を置いた Page の id
    docks: BTreeMap<Key, (String, Dock)>,
    /// (project_id, surface_id) → 重ねられない理由（`view` が枠の 1 行にする）
    errors: BTreeMap<Key, NativeError>,
    /// 重ねた記録の file（None = 書かない・単体の試験）
    record: Option<PathBuf>,
    /// (project_id, surface_id) → 元の app の名前（理由の 1 行の枠に出す。重ねる前に分かった分だけ）
    names: BTreeMap<Key, String>,
    /// W16-2: (project_id, surface_id) → 前の読み戻しとその時刻（`readback` の before・`readback_due`）。保存しない
    reads: BTreeMap<Key, (Instant, Readback)>,
    /// 反証の直し: (project_id, surface_id) → 窓の文書を最後に読めた読み（`ax_value` が Ok）。`receipt` の比べる相手（`seen`）。保存しない
    seen: BTreeMap<Key, Readback>,
    /// 反証の直し: bundle id → find_app で聞けた版（`app_version` が 2 度目を送らない）
    versions: BTreeMap<String, String>,
    /// W16-2: 応答を待っている ax_text（id と送った時刻）。面ごとに 1 つまで
    ax_wait: BTreeMap<Key, (u64, Instant)>,
    /// W16-2: 待たずに戻った後に pump が拾った ax_text の応答（次の readback が使う）
    ax_got: BTreeMap<Key, Result<String, NativeError>>,
    /// W16-2: ax_text を UI の thread で待つ上限（製品は AX_BUDGET・試験は混雑で偽の bridge が遅いので長くする）
    ax_budget: Duration,
    /// Shibubu の窓を元の窓の真上 / 真下へ（製品は `surface::order_host_above` / `order_host_below`・試験は偽物）
    order_above: fn(u32) -> Result<(), String>,
    order_below: fn(u32) -> Result<(), String>,
    /// W16-3: restore で繋ぎ直した面。Attaching → Active の記録は Attached ではなく Reconnected
    reconnected: BTreeSet<Key>,
    /// W16-3: 応答を待っている `refresh_space` の読み（要求の id → 面と cmd）・面ごとに最後に送った時刻・cg_windows を待つ間の frame の応答
    space_wait: BTreeMap<u64, (Key, &'static str)>,
    space_at: BTreeMap<Key, Instant>,
    space_frame: BTreeMap<Key, serde_json::Value>,
    /// W16-3: 見えていないので隠した面（別の Page へ移った・枠が全部隠れた）。`ensure` は戻さず、枠が見えた `on_frame` か `show_page` で戻す
    unseen: BTreeSet<Key>,
}

/// 状態の替わり目のうち、記録に残す物（`on_frame` と `show_page` の bridge の event の後）。
fn edge(before: Lifecycle, after: Lifecycle) -> Option<OverlayEvent> {
    match (before, after) {
        // 1 回の pump の中で Attached の後に HostMoved（枠が動いた）や最小化が続くと Resizing / Hidden で終わる。どちらも Attached を通らないと来ない
        (Lifecycle::Attaching, Lifecycle::Active | Lifecycle::Resizing | Lifecycle::Hidden) => Some(OverlayEvent::Attached),
        (b, Lifecycle::Degraded) if b != Lifecycle::Degraded => Some(OverlayEvent::Degraded),
        (b, Lifecycle::SourceUnavailable) if b != Lifecycle::SourceUnavailable => Some(OverlayEvent::SourceGone),
        _ => None,
    }
}

/// 窓の (pid, CG の窓番号)。
fn window_of(d: &Dock) -> Option<(u32, Option<i64>)> {
    d.window.as_ref().map(|w| (w.pid, w.cg_id))
}

/// `refresh_space` の応答の組を窓に書く。`frame`（AX）は全画面と最小化・`cg_windows`（CG）は今の Space に在るか（無ければ on_screen=false）。
/// 組で書く: 片方だけ書くと、最小化した・戻した間の 1 frame に on_screen=false と minimized=false が並び、最小化を別 Space と読む。
fn apply_space(
    w: &mut WindowInfo,
    frame: Option<&serde_json::Value>,
    cg: Option<&serde_json::Value>,
) {
    if let Some(v) = frame {
        if let Some(fs) = v["fullscreen"].as_bool() {
            w.fullscreen = fs;
        }
        if let Some(m) = v["minimized"].as_bool() {
            w.minimized = m;
        }
    }
    let row = w.cg_id.zip(cg).and_then(|(id, v)| {
        v["windows"]
            .as_array()?
            .iter()
            .find(|x| x["id"].as_i64() == Some(id))
    });
    if let Some(on) = row.and_then(|r| r["on_screen"].as_bool()) {
        w.on_screen = Some(on);
    }
}

impl NativeSurfaces {
    /// 重ねた面（Dock）が在るか（在る間は毎 frame の poll が要る: bridge の event・枠の追従・app.rs `next_repaint`）。
    pub fn wants_frames(&self) -> bool {
        !self.docks.is_empty()
    }

    /// 空で作る（bridge はまだ起こさない）。
    pub fn new(record: Option<PathBuf>) -> Self {
        NativeSurfaces {
            bridge: None,
            docks: BTreeMap::new(),
            errors: BTreeMap::new(),
            record,
            names: BTreeMap::new(),
            reads: BTreeMap::new(),
            seen: BTreeMap::new(),
            versions: BTreeMap::new(),
            ax_wait: BTreeMap::new(),
            ax_got: BTreeMap::new(),
            ax_budget: AX_BUDGET,
            order_above: surface::order_host_above,
            order_below: surface::order_host_below,
            reconnected: BTreeSet::new(),
            space_wait: BTreeMap::new(),
            space_at: BTreeMap::new(),
            space_frame: BTreeMap::new(),
            unseen: BTreeSet::new(),
        }
    }

    /// 生きている bridge（無い・止まっていれば起こし直す。起きなければ Err）。
    fn bridge(&mut self) -> Result<&mut Bridge, NativeError> {
        if !self.bridge.as_ref().is_some_and(Bridge::alive) {
            self.bridge = None;
            // 新しい bridge の要求の id は 1 から。前の bridge に送った ax_text の id と取り違えない
            self.ax_wait.clear();
            self.ax_got.clear();
            self.space_wait.clear();
            self.space_frame.clear();
            self.bridge = Some(surface::find_bridge().and_then(|p| Bridge::spawn(&p, READY_LIMIT))?);
        }
        Ok(self.bridge.as_mut().expect("今入れた"))
    }

    /// 記録を 1 件足す（`win` は窓が消える前に取った物・`frame` は送った枠。None なら今の Dock の物）。書けなければ Dock の metrics に 1 行（重ねるのは止めない）。
    fn record(&mut self, k: &Key, event: OverlayEvent, win: Option<(u32, Option<i64>)>, frame: Option<Frame>) {
        let Some(path) = self.record.clone() else { return };
        let Some((page_id, d)) = self.docks.get_mut(k) else { return };
        let r = OverlayRecord {
            surface_id: k.1.clone(),
            page_id: page_id.clone(),
            event,
            at_ms: surface::now_ms(),
            bundle_id: d.spec.app_identity.bundle_id.clone(),
            pid: win.map(|w| w.0).or(d.app_pid()),
            window_id: win.and_then(|w| w.1),
            frame: frame.or(d.tracker.last_target),
        };
        if let Err(e) = r.append(&path) {
            d.metrics.note(format!("Couldn't save the record of the covered window ({}: {e})", path.display()));
        }
    }

    /// bridge に届いた物を全部の面へ配る（応答は id で、event は handle と pid で各 Dock が自分の物だけ拾う）→ 替わり目を記録。
    /// `host` は今描いた面の枠（その面だけ `on_host`）。
    fn pump(&mut self, host: Option<(&Key, Host)>, now: Instant) {
        let Some(b) = self.bridge.as_mut() else { return };
        let msgs = b.poll();
        // W16-2: 待たずに戻った ax_text の応答を拾う（Dock は自分の id の応答しか見ない）
        for m in &msgs {
            if let Msg::Reply { id, result } = m {
                // W16-3: 待たずに戻った refresh_space の読み。frame を取っておき、後に送った cg_windows が来たら組で窓に書く（次の on_frame の off_space が見る）
                if let Some((k, cmd)) = self.space_wait.remove(id) {
                    if cmd == "frame" {
                        if let Ok(v) = result {
                            self.space_frame.insert(k, v.clone());
                        }
                        continue;
                    }
                    let frame = self.space_frame.remove(&k);
                    if let Some(w) = self.docks.get_mut(&k).and_then(|(_, d)| d.window.as_mut()) {
                        apply_space(w, frame.as_ref(), result.as_ref().ok());
                    }
                    continue;
                }
                if let Some(k) = self.ax_wait.iter().find(|(_, (w, _))| w == id).map(|(k, _)| k.clone()) {
                    self.ax_wait.remove(&k);
                    let app = self.names.get(&k).cloned().unwrap_or_else(|| "the original app".into());
                    self.ax_got.insert(k, ax_reply(&app, result.clone()));
                }
            }
        }
        let mut edges = Vec::new();
        for (sid, (_, d)) in self.docks.iter_mut() {
            let (before, win) = (d.life, window_of(d));
            for m in &msgs {
                d.on_msg(b, m.clone(), now);
            }
            if let Some((_, at)) = host.filter(|(k, _)| *k == sid) {
                d.on_host(b, at, now);
            }
            d.tick(b, now);
            if let Some(ev) = edge(before, d.life) {
                edges.push((sid.clone(), ev, window_of(d).or(win)));
            }
        }
        for (sid, ev, win) in edges {
            let ev = if ev == OverlayEvent::Attached && self.reconnected.remove(&sid) {
                OverlayEvent::Reconnected
            } else {
                ev
            };
            self.record(&sid, ev, win, None);
            if matches!(
                ev,
                OverlayEvent::Attached | OverlayEvent::Reconnected | OverlayEvent::HostHidden
            ) {
                let _ = self.persist_identity(&sid.0, &sid.1);
            }
        }
    }

    /// 枠の帯で押された次の一手（人の操作で 1 回）。どの一手も何かをする（L2: 押しても何も起きない を作らない）。
    fn act(&mut self, k: &Key, action: PortalAction) {
        match action {
            // 見つからない・終了・同期の遅れ: 今の面を捨てて、次の frame の ensure で探し直す
            PortalAction::Rediscover | PortalAction::Retry | PortalAction::CheckUpdate => {
                self.errors.remove(k);
                if let (Some((_, mut d)), Some(b)) = (self.docks.remove(k), self.bridge.as_mut()) {
                    d.close(b);
                }
            }
            PortalAction::OpenPermission => {
                self.errors.remove(k);
                if let Ok(b) = self.bridge() {
                    let lim = b.limit();
                    let _ = b.call("permission", json!({ "prompt": true }), lim);
                }
            }
            // 人が押した時だけ元の app を前に出す（外で開く。製品が勝手に前面を取るのではない）
            PortalAction::Detach | PortalAction::ContinueInSource | PortalAction::OpenSourceWindow => {
                if let (Some((_, d)), Some(b)) = (self.docks.get_mut(k), self.bridge.as_mut()) {
                    if let Err(e) = d.handoff(b) {
                        d.reason = e;
                    }
                }
            }
            PortalAction::Cancel => {
                if let (Some((_, d)), Some(b)) = (self.docks.get_mut(k), self.bridge.as_mut()) {
                    d.close(b);
                }
            }
        }
    }

    /// 面の Page を描く前に 1 回: 窓が無ければ bridge を起こし、`spec` の app の `file`（Project の working dir からの相対・`workspace` に足す）を開いた窓を探して重ね始める。
    /// 例: `draw_page` で `for s in native_specs { let _ = g.surfaces.ensure(&p.id, page_id, s, ws); }`
    /// 保証する: 既に重ねている面は何もしない（同じ窓 id のまま・2 回目の attach をしない）・app を起動しない・前面を取らない（activate を送らない）・
    /// 面は (project_id, surface id) ごと（別の Project の同じ id の面を使い回さない）・描く Page の面が隠れたまま（移るのを断られた等）なら前へ戻す（`show_page` と同じ手）・
    /// Err は `errors` にも残し `view` の 1 行にする。保証しない: README.md を TextEdit で開く事（人が先に開いた時だけ通す。製品が開くのは後の Wave）。
    pub fn ensure(
        &mut self,
        project_id: &str,
        page_id: &str,
        spec: &SurfaceSpec,
        workspace: Option<&Path>,
    ) -> Result<(), NativeError> {
        let (SurfaceBackendKind::NativeDirect, SurfaceTarget::App { bundle_id, file }) = (spec.backend, &spec.target) else { return Err(NativeError::NotNative) };
        let k = key(project_id, &spec.id);
        if let Some((_, d)) = self.docks.get(&k) {
            // 描いている Page の面が隠れたまま = Page を離れる手が途中で断られた。窓を Shibubu の後ろに残して「直接操作中」と出さない
            // 見えていないので隠した面（unseen）は戻さない: 戻すのは枠が見えた on_frame（戻すと毎 frame 出し入れする）
            if d.life == Lifecycle::Hidden && !self.unseen.contains(&k) {
                self.reveal(page_id);
            }
            return Ok(());
        }
        // 前に断った面は、人が帯の一手を押すか Page に入り直すまで聞き直さない（毎 frame bridge を待たない）
        if let Some(e) = self.errors.get(&k) {
            return Err(e.clone());
        }
        // ponytail: 窓は題名（file 名）で選ぶ。同じ名前の別の dir の README.md を区別できない（bridge が AXDocument を返さない）。
        // 2 つの Project の README.md が同じ題名なら同じ窓に 2 つの Dock が付く（隠れている方は窓を動かさない）。区別が要れば bridge の windows に AXDocument を足して workspace と照らす
        let _ = workspace;
        let r = self.attach(page_id, &k, bundle_id, file.as_deref());
        if let Err(e) = &r {
            self.errors.insert(k, e.clone());
        }
        r
    }

    /// `ensure` の中身: 起動中か → 許可 → file の窓 → Dock を作って重ね始める。app を起こさない・activate を送らない。
    fn attach(&mut self, page_id: &str, k: &Key, bundle_id: &str, file: Option<&str>) -> Result<(), NativeError> {
        let running = self.running_apps(k, bundle_id)?;
        let Some((first, _, _)) = running.first() else { return Err(NativeError::NotRunning { app: short_name(bundle_id) }) };
        let file_name = file.map(file_name_of);
        // 同じ bundle の process が複数（`open -n`）なら、その file の窓を持つ process を取る（先に起きた方に決め打ちしない）
        let found = running.iter().find_map(|(name, pid, list)| {
            let picked = match file_name {
                Some(f) => pick_document_window(list, f),
                None => surface::pick_main_window(list),
            };
            picked.ok().map(|w| (name.clone(), *pid, w))
        });
        let Some((name, pid, w)) = found else { return Err(NativeError::NoWindow { app: first.clone(), file: file_name.unwrap_or("Document").to_string() }) };
        self.bind(page_id, k, bundle_id, &name, pid, w)
    }

    /// 起動中の `bundle_id` の app の名前・pid と窓の一覧（`attach` の初回と `reconnect_one`）。`open -n` で同じ bundle の process が複数なら全部（bridge の apps の順）。起動していなければ空。
    /// 許可を窓より先に聞く: 無ければ窓を聞かず `NoAccessibility`（聞くと bridge の not_trusted が「許可を開く」の 1 行を隠す）。
    fn running_apps(&mut self, k: &Key, bundle_id: &str) -> Result<Vec<(String, u32, Vec<WindowInfo>)>, NativeError> {
        self.names.insert(k.clone(), short_name(bundle_id));
        let b = self.bridge()?;
        let lim = b.limit();
        let apps = b.call("apps", json!({ "bundle_id": bundle_id, "regular_only": false }), lim)?;
        let mut procs = Vec::new();
        for app in apps["apps"].as_array().into_iter().flatten().filter(|x| x["bundle_id"].as_str() == Some(bundle_id)) {
            let name = app["name"].as_str().filter(|n| !n.is_empty()).map_or_else(|| short_name(bundle_id), str::to_string);
            let pid = app["pid"].as_u64().map(|p| p as u32).ok_or_else(|| NativeError::Bridge(format!("Can't read the process ID of {name}")))?;
            procs.push((name, pid));
        }
        let Some((first, _)) = procs.first().cloned() else { return Ok(Vec::new()) };
        let trusted = b.call("permission", json!({}), lim)?["trusted"].as_bool().unwrap_or(false);
        if !trusted {
            self.names.insert(k.clone(), first);
            return Err(NativeError::NoAccessibility);
        }
        let mut running = Vec::new();
        for (name, pid) in procs {
            let list = surface::parse_windows(&b.call("windows", json!({ "pid": pid }), lim)?);
            running.push((name, pid, list));
        }
        self.names.insert(k.clone(), first);
        Ok(running)
    }

    /// 選んだ窓に Dock を付ける（`attach` の初回と `restore` の同じ窓）。app を起こさない・activate を送らない。
    fn bind(&mut self, page_id: &str, k: &Key, bundle_id: &str, name: &str, pid: u32, w: WindowInfo) -> Result<(), NativeError> {
        let order_below = self.order_below;
        let b = self.bridge.as_mut().ok_or_else(|| NativeError::Bridge("The part that overlays windows isn't running".into()))?;
        let mut dock = Dock::new(AppIdentity { bundle_id: bundle_id.to_string(), name: name.to_string(), pid: Some(pid), ..AppIdentity::default() }, std::process::id());
        dock.spec.surface_id = k.1.clone();
        dock.front_on_attach = false;
        dock.host_order = Some(order_below);
        dock.attach_to(b, w, Host::default());
        self.names.insert(k.clone(), name.to_string());
        self.docks.insert(k.clone(), (page_id.to_string(), dock));
        Ok(())
    }

    /// 1 frame ごと（描いた後）: renderer が返した枠の画面座標を Dock に渡し（`Dock::on_host`）、bridge の event を流す（`on_msg`・`tick`）。
    /// `action` は枠の帯で押された次の一手（再試行・許可を開く・切り離す）。
    /// 保証する: 状態が Active / Attaching / Resizing の時だけ窓を動かす（`Lifecycle::touches_window`）・Attached / Degraded / SourceGone に入った frame に記録を 1 件足す。
    pub fn on_frame(&mut self, project_id: &str, surface_id: &str, at: Host, action: Option<PortalAction>, now: Instant) {
        let k = key(project_id, surface_id);
        if let Some(a) = action {
            self.act(&k, a);
        }
        // W16-3: 枠が全部隠れるまで scroll した面（slot が None）の窓を Page の上に浮かせない（hide_unseen と同じ）。また見えたら同じ窓で戻す
        if at.slot.is_none() {
            let hidden = self.hide(|x, _, d| *x == k && matches!(d.life, Lifecycle::Active | Lifecycle::Resizing));
            self.unseen.extend(hidden);
            return self.pump(None, now);
        }
        if self.unseen.contains(&k) {
            self.reveal_where(|x, _| *x == k);
        }
        self.refresh_space(&k, now);
        match self.off_space(project_id, surface_id, &at) {
            Ok(()) => self.pump(Some((&k, at)), now),
            Err(e) => {
                self.errors.insert(k.clone(), e.clone());
                let mut off_edge = None;
                if let Some((_, d)) = self.docks.get_mut(&k) {
                    let before = d.life;
                    d.life = d
                        .life
                        .next(LifeEvent::FrameOff)
                        .unwrap_or(Lifecycle::Degraded);
                    d.reason = e.to_string();
                    if let Some(ev) = edge(before, d.life) {
                        off_edge = Some((ev, window_of(d)));
                    }
                }
                if let Some((ev, win)) = off_edge {
                    self.record(&k, ev, win, None);
                }
                self.pump(None, now);
            }
        }
    }

    /// Page を離れる（`Generated::save_view_state` から）: その Page の面ごとに Shibubu の窓を元の窓の真上へ（`order_host_above`・前面は変えない）→ HostHidden → 記録。
    /// 保証する: 窓を閉じない・動かさない（戻った時に同じ窓 id）。重ねていない面は何もしない。
    pub fn hide_page(&mut self, page_id: &str) {
        self.hide(|_, pg, _| pg == page_id);
    }

    /// `pick` に当たる面ごとに Shibubu の窓を元の窓の真上へ → HostHidden → 記録（`hide_page` と `hide_unseen`・枠が隠れた `on_frame`）。返すのは隠せた面。
    fn hide(&mut self, pick: impl Fn(&Key, &str, &Dock) -> bool) -> Vec<Key> {
        let order_above = self.order_above;
        let mut hidden = Vec::new();
        for (sid, (_, d)) in self.docks.iter_mut().filter(|(k, (pg, d))| pick(k, pg, d)) {
            let Some((pid, cg)) = window_of(d) else { continue };
            let Some(cg) = cg else {
                d.metrics.note("The window has no number, so Shibubu can't be placed above it. The original stays visible");
                continue;
            };
            if let Err(e) = order_above(cg as u32) {
                // 隠せていないので HostHidden を記録しない（記録だけの偽の「隠した」を作らない）
                d.metrics.note(format!("{:.0} Couldn't place Shibubu above the original window ({e})", surface::now_ms()));
                continue;
            }
            match d.life.next(LifeEvent::HostHidden) {
                Ok(to) => {
                    d.life = to;
                    hidden.push((sid.clone(), Some((pid, Some(cg)))));
                }
                Err(e) => d.metrics.note(e),
            }
        }
        for (sid, win) in &hidden {
            self.record(sid, OverlayEvent::HostHidden, *win, None);
            let _ = self.persist_identity(&sid.0, &sid.1);
        }
        hidden.into_iter().map(|(k, _)| k).collect()
    }

    /// W16-3: 今の Page（`page_id`）で見えていない面の窓を Shibubu の後ろへ（`hide_page` と同じ手・HostHidden・記録 host_hidden）。毎 frame `draw_page` から。
    /// 見えていない = その Project の別の Page へ移った面（「TextEdit を右へ」の後・Undo の後）。枠が全部隠れるまで scroll した面は `on_frame` が同じ手で隠す。
    /// 実機（W16-3 pane N）: 「TextEdit を右へ」の後、README の窓が Docs の時の位置に残り、Diagram の Excalidraw の上に浮いたまま帯は「直接操作中」だった。
    /// 保証する: 重なっている（Active / Resizing）面だけ・窓は閉じない・動かさない（同じ pid・window_id）。隠した面は、枠が見えた frame の `on_frame` か `show_page` で戻す。
    /// Wave: W16-3
    pub fn hide_unseen(&mut self, project_id: &str, page_id: &str) {
        let hidden = self.hide(|k, pg, d| k.0 == project_id && pg != page_id && matches!(d.life, Lifecycle::Active | Lifecycle::Resizing));
        self.unseen.extend(hidden);
    }

    /// Page に入る（`Generated::activate` から）: その Page の面ごとに Shibubu の窓を元の窓の真下へ（`order_host_below`）→ HostShown → 記録。
    /// 保証する: 隠した時と同じ窓（窓 id と pid）を使う・新しく attach しない。窓が消えていたら SourceGone と 1 行（偽の「重ねて表示中」を出さない）。
    pub fn show_page(&mut self, page_id: &str) {
        // Page に入り直した = 前に断った面を聞き直す・閉じた面を作り直す
        self.errors.clear();
        self.docks.retain(|_, (_, d)| d.life != Lifecycle::Closed);
        self.pump(None, Instant::now());
        self.reveal(page_id);
    }

    /// `page_id` の隠れた面を、隠した時と同じ窓のまま Shibubu の真下へ戻す → HostShown → 記録（`show_page` と、隠れたまま描かれた `ensure` から）。
    fn reveal(&mut self, page_id: &str) {
        self.reveal_where(|_, pg| pg == page_id);
    }

    /// `pick` に当たる隠れた面を同じ窓のまま Shibubu の真下へ戻す（`reveal` と、枠がまた見えた `on_frame`）。
    fn reveal_where(&mut self, pick: impl Fn(&Key, &str) -> bool) {
        let order_below = self.order_below;
        let Some(b) = self.bridge.as_mut() else { return };
        let mut done = Vec::new();
        for (sid, (_, d)) in self.docks.iter_mut().filter(|(k, (pg, d))| pick(k, pg) && d.life == Lifecycle::Hidden) {
            self.unseen.remove(sid);
            let Some((pid, Some(cg))) = window_of(d) else { continue };
            let lim = b.limit();
            // 隠した時の窓（同じ CG の窓番号と pid）がまだ在るか。無ければ繋ぎ直さない（別の窓にすり替えない）
            let there = b
                .call("cg_windows", json!({ "pid": pid, "on_screen_only": false }), lim)
                .map(|v| v["windows"].as_array().is_some_and(|a| a.iter().any(|w| w["id"].as_i64() == Some(cg) && w["pid"].as_u64() == Some(pid as u64))));
            let (win, sent) = (Some((pid, Some(cg))), d.tracker.last_target);
            match there {
                Ok(true) => {}
                Ok(false) => {
                    d.life = d.life.next(LifeEvent::SourceGone).unwrap_or(Lifecycle::SourceUnavailable);
                    d.reason = format!("{}'s window was closed. Reopen it in {}, then press Try again", d.spec.app_identity.name, d.spec.app_identity.name);
                    d.window = None;
                    done.push((sid.clone(), OverlayEvent::SourceGone, win, sent));
                    continue;
                }
                Err(e) => {
                    d.metrics.note(format!("Couldn't check whether the window exists: {e}"));
                }
            }
            d.life = d.life.next(LifeEvent::HostShown).unwrap_or(Lifecycle::Active);
            if let Err(e) = order_below(cg as u32) {
                // 元の窓が Shibubu の後ろに残っている: 「直接操作中」と出さない
                d.life = d.life.next(LifeEvent::FrameOff).unwrap_or(Lifecycle::Degraded);
                d.reason = format!("Couldn't bring {name}'s window back to the front ({e}). Press Open in {name}", name = d.spec.app_identity.name);
                done.push((sid.clone(), OverlayEvent::Degraded, win, sent));
                continue;
            }
            // 隠れている間に人が窓を動かしていても、次の frame で枠へ送り直す
            d.tracker.reset();
            done.push((sid.clone(), OverlayEvent::HostShown, win, sent));
        }
        for (sid, ev, win, sent) in done {
            self.record(&sid, ev, win, sent);
        }
    }

    /// 枠に描く今（renderer の `ActionHost::surface_portal` へ）。重ねる対象でない面・まだ `ensure` していない面は None（W3 の 1 行のまま）。
    /// 保証する: 状態語は `PortalState::wording`（成功の語を含まない）・Err の面は `NativeError` の 1 行を detail に。
    /// detail は Shibubu の言葉（`shown_detail`）: Dock の理由（kangae-portal の言葉: 旧名・「真の子 View」・無い button）をそのまま出さない。
    pub fn view(&self, project_id: &str, surface_id: &str) -> Option<PortalView> {
        let k = key(project_id, surface_id);
        if let Some((_, d)) = self.docks.get(&k) {
            let state = surface::portal_state(&d.life, &d.spec)?;
            return Some(PortalView {
                app_name: d.spec.app_identity.name.clone(),
                state,
                sync: d.spec.sync_mode,
                mode: (d.life == Lifecycle::Detached).then_some(Mode::External),
                detail: shown_detail(d, state),
            });
        }
        let e = self.errors.get(&k)?;
        let state = match e {
            NativeError::NotNative => return None,
            NativeError::NoAccessibility => PortalState::PermissionRequired,
            // 一手は「再探索」（起こし直す・探し直す）。W16-2 の 3 つは readback だけが返し `errors` に入らないが、入っても探し直す
            NativeError::Bridge(_)
            | NativeError::NotRunning { .. }
            | NativeError::NoWindow { .. }
            | NativeError::NotAttached { .. }
            | NativeError::FileUnreadable { .. }
            | NativeError::NoTextArea { .. }
            | NativeError::AmbiguousWindow { .. }
            | NativeError::WindowClosed { .. }
            | NativeError::HumanHasControl { .. } => PortalState::Undiscovered,
            NativeError::OffSpace { .. } => PortalState::IntegrationFailed,
            NativeError::Gone { .. } => PortalState::SourceUnavailable,
        };
        Some(PortalView {
            app_name: self.names.get(&k).cloned().unwrap_or_else(|| "the original app".into()),
            state,
            sync: surface::SyncMode::Unsupported,
            mode: None,
            detail: e.to_string(),
        })
    }

    /// W16-3: 今重ねている窓の identity（Shibubu の再起動後に同じ窓へ繋ぐ）。終了した app・決めていない窓は None。
    /// 例: `let id = g.surfaces.identity(&p.id, "docs").expect("重ねている"); assert_eq!(id.window_id, 500);`
    /// 保証する: Some は Lifecycle が Active / Hidden / Resizing で pid と CG の窓番号が両方在る時だけ（SourceUnavailable / Degraded / Closed は None＝「直接操作中」の根拠にしない）。
    /// 保証しない: 返した pid・window_id が今の Space に在る事（それは `off_space`）。
    /// Wave: W16-3
    pub fn identity(&self, project_id: &str, surface_id: &str) -> Option<OverlayIdentity> {
        let (page_id, d) = self.docks.get(&key(project_id, surface_id))?;
        if !matches!(
            d.life,
            Lifecycle::Active | Lifecycle::Hidden | Lifecycle::Resizing
        ) {
            return None;
        }
        let w = d.window.as_ref()?;
        Some(OverlayIdentity {
            schema: IDENTITY_SCHEMA.into(),
            project_id: project_id.into(),
            surface_id: surface_id.into(),
            page_id: page_id.clone(),
            bundle_id: d.spec.app_identity.bundle_id.clone(),
            pid: w.pid,
            window_id: w.cg_id?,
            title: w.title.clone(),
        })
    }

    /// W16-3: 今の identity を `overlay-identity.jsonl` に追記する（Page を離れる・Shibubu を終了する前）。
    /// 例: `let _ = g.surfaces.persist_identity(&p.id, "docs");`
    /// 保証する: `identity()` が None なら何も書かない（終了した app の行を残して「接続済み」の根拠にしない）。保証しない: 書いた行の窓が次の起動まで生きている事。
    /// Wave: W16-3
    pub fn persist_identity(&self, project_id: &str, surface_id: &str) -> std::io::Result<()> {
        let Some(id) = self.identity(project_id, surface_id) else {
            return Ok(());
        };
        let Some(path) = self.identity_file() else {
            return Ok(());
        };
        id.append(&path)
    }

    /// identity の JSONL（`overlay.jsonl` と同じ dir・`NativeSurfaces::new` の署名は変えない）。
    fn identity_file(&self) -> Option<PathBuf> {
        self.record.as_ref().map(|p| match p.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => identity_path(dir),
            _ => p.with_file_name(IDENTITY_LOG),
        })
    }

    /// W16-3: Shibubu を起こし直した後、`overlay-identity.jsonl` の pid・window_id が生きていれば同じ窓に繋ぎ直す。
    /// 例: `Generated::open` の後に `g.surfaces.restore(&project_id)?;`（新しい窓を `pick_document_window` で取りに行かない）。
    /// 保証する: 同じ bundle + 同じ pid + 同じ window_id の窓だけ（pid が居なければ `Gone`・pid は居て window_id が無ければ `WindowClosed`・同じ題名が複数なら `AmbiguousWindow`・別の窓にすり替えない）。返す数は繋ぎ直した面の数。
    /// 保証しない: Space / 全画面で重ねられる事（それは `off_space` → `OffSpace`）。
    /// Wave: W16-3
    pub fn restore(&mut self, project_id: &str) -> Result<usize, NativeError> {
        let Some(path) = self.identity_file() else {
            return Ok(0);
        };
        if !path.exists() {
            return Ok(0);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| NativeError::Bridge(format!("Can't read the identity ({e})")))?;
        let mut last = BTreeMap::<String, OverlayIdentity>::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(id) = serde_json::from_str::<OverlayIdentity>(line) else {
                continue;
            };
            if id.schema != IDENTITY_SCHEMA || id.project_id != project_id {
                continue;
            }
            last.insert(id.surface_id.clone(), id);
        }
        let mut n = 0;
        for id in last.into_values() {
            let k = key(&id.project_id, &id.surface_id);
            if self.docks.contains_key(&k) {
                continue;
            }
            match self.reconnect_one(&id) {
                Ok(()) => n += 1,
                Err(e @ NativeError::Bridge(_)) => return Err(e),
                Err(e) => {
                    self.names
                        .entry(k.clone())
                        .or_insert_with(|| short_name(&id.bundle_id));
                    self.errors.insert(k, e);
                }
            }
        }
        Ok(n)
    }

    /// 1 面を同じ pid・window_id に繋ぎ直す。題名だけで窓を取りに行かない。
    fn reconnect_one(&mut self, id: &OverlayIdentity) -> Result<(), NativeError> {
        let k = key(&id.project_id, &id.surface_id);
        let running = self.running_apps(&k, &id.bundle_id)?;
        // 同じ bundle の process が複数なら保存した pid の process（居なければ先頭 → match_identity が Gone）
        let Some((name, pid, list)) = running.iter().find(|(_, p, _)| *p == id.pid).or(running.first()).cloned() else {
            return Err(NativeError::Gone {
                app: short_name(&id.bundle_id),
            });
        };
        let w = match_identity(id, &id.bundle_id, Some(pid), &list)?;
        self.reconnected.insert(k.clone());
        self.bind(&id.page_id, &k, &id.bundle_id, &name, id.pid, w)
    }

    /// W16-3: 「TextEdit を右へ」（`MoveSurface`）の後、Dock の page_id を行き先の Page に付け替える（窓は動かさない・同じ pid・window_id のまま）。
    /// 例: `g.surfaces.retarget_page(&p.id, "docs", &to_page);`
    /// 保証する: 重ねている窓はそのまま（hide / show しない）。無い面は何もしない。保証しない: Project の木が既に MoveSurface 済みである事（呼ぶ側）。
    /// Wave: W16-3
    pub fn retarget_page(&mut self, project_id: &str, surface_id: &str, page_id: &str) {
        if let Some((pg, _)) = self.docks.get_mut(&key(project_id, surface_id)) {
            *pg = page_id.to_string();
        }
        let _ = self.persist_identity(project_id, surface_id);
    }

    /// W16-3: Agent がその面に `action` をしてよいか（PORTAL-09）。人が持っている間は `HumanHasControl`（何もしない）。
    /// 例: `g.surfaces.agent_may(&p.id, "docs", AgentAction::Input)?;`
    /// 保証する: 人が操作中なら Err で元の app に何も送らない。保証しない: Agent の一手の中身（それは Adapter）。
    /// Wave: W16-3
    pub fn agent_may(
        &self,
        project_id: &str,
        surface_id: &str,
        action: AgentAction,
    ) -> Result<(), NativeError> {
        let k = key(project_id, surface_id);
        let app = self
            .names
            .get(&k)
            .cloned()
            .unwrap_or_else(|| "the original app".into());
        let Some((_, d)) = self.docks.get(&k) else {
            return Err(NativeError::NotAttached { app });
        };
        d.lease
            .agent_may(action)
            .map_err(|_| NativeError::HumanHasControl { app, action })
    }

    /// W16-3: 今の Host と元の窓が、この Space の枠に重ねられるか。だめなら `OffSpace`（degraded・「外で開く」の 1 行）。
    /// 例: `g.surfaces.off_space(&p.id, "docs", &host)?;`
    /// 保証する: Shibubu が全画面・元の窓が全画面・元の窓が今の Space に無い時は Err。偽の NativeDirect を出さない。
    /// 保証しない: 元の窓を今の Space へ連れて来る事（非公開 API は使わない）。
    /// Wave: W16-3
    pub fn off_space(
        &self,
        project_id: &str,
        surface_id: &str,
        host: &Host,
    ) -> Result<(), NativeError> {
        let Some((_, d)) = self.docks.get(&key(project_id, surface_id)) else {
            return Ok(());
        };
        let app = d.spec.app_identity.name.clone();
        if host.fullscreen {
            return Err(NativeError::OffSpace {
                app,
                kind: OffSpaceKind::HostFullscreen,
            });
        }
        if let Some(w) = &d.window {
            if w.fullscreen {
                return Err(NativeError::OffSpace {
                    app,
                    kind: OffSpaceKind::SourceFullscreen,
                });
            }
            // 最小化は Dock の minimized が degraded にする。CG の on_screen=false を別 Space にしない
            if w.on_screen == Some(false) && !w.minimized {
                return Err(NativeError::OffSpace {
                    app,
                    kind: OffSpaceKind::DifferentSpace,
                });
            }
        }
        Ok(())
    }

    /// 今の Space / 全画面 / 最小化を Dock の WindowInfo に書く（attach 時の snapshot のままにしない）。
    /// on_screen は CG（`cg_windows`・今の Space に無ければ false）。fullscreen / minimized は AX（`frame`・AXFullScreen）。
    /// `space_changed` の event を待たず、`SPACE_EVERY` ごとに送る。応答は待たず pump が `apply_space` で書く（その次の frame で OffSpace になる）。
    /// 応答待ちの間は送らない（遅い app の AX で bridge の 1 本の列を伸ばさない）。`AX_STALE` 来なければ送り直す。
    fn refresh_space(&mut self, k: &Key, now: Instant) {
        let Some((_, d)) = self.docks.get(k) else {
            return;
        };
        if matches!(
            d.life,
            // Degraded も読まない（W16-3 pane N の実機: nap の TextEdit に 2 秒ごとに frame を読みに行き、繋ぎ直しに要る windows / set_frame の前に bridge の 1 本の列を占めた）
            Lifecycle::Closed | Lifecycle::SourceUnavailable | Lifecycle::Detached | Lifecycle::Degraded
        ) {
            return;
        }
        let Some(w) = d.window.as_ref() else {
            return;
        };
        let waiting = self.space_wait.values().any(|(wk, _)| wk == k);
        let every = if waiting { AX_STALE } else { SPACE_EVERY };
        if self
            .space_at
            .get(k)
            .is_some_and(|at| now.saturating_duration_since(*at) < every)
        {
            return;
        }
        let Some(b) = self.bridge.as_mut() else {
            return;
        };
        self.space_wait.retain(|_, (wk, _)| wk != k);
        self.space_frame.remove(k);
        let reads = [
            ("frame", json!({ "handle": w.handle })),
            (
                "cg_windows",
                json!({ "pid": w.pid, "on_screen_only": false }),
            ),
        ];
        for (cmd, args) in reads {
            if let Ok(id) = b.send(cmd, args) {
                self.space_wait.insert(id, (k.clone(), cmd));
            }
        }
        self.space_at.insert(k.clone(), now);
    }

    /// 試験: 重ねた Dock を捨てて restore の入口にする（bridge と identity の file はそのまま）。
    #[cfg(test)]
    pub fn forget_docks(&mut self) {
        self.docks.clear();
        self.errors.clear();
        self.names.clear();
        self.reconnected.clear();
        self.space_wait.clear();
        self.space_at.clear();
        self.space_frame.clear();
        self.unseen.clear();
    }

    /// 重ねた窓の今（試験と記録の照合用）: (pid, 窓 id)。重ねていなければ None。
    #[cfg(test)] // 製品の画面は使わない（試験と記録の照合だけ）
    pub fn window(&self, project_id: &str, surface_id: &str) -> Option<(u32, Option<i64>)> {
        self.docks.get(&key(project_id, surface_id)).and_then(|(_, d)| window_of(d))
    }

    /// W16-2: 面を 1 回読み戻す。返すのは (前の読み, 今の読み)。前の読みは同じ (project_id, surface id) の 1 つ前の呼び出し（この起動の中・保存しない・初回は None）で、今の読みに置き換える。
    /// disk: `workspace.join(spec の file)` を読んで sha256（書かない）。AX: 重ねた窓（`ensure` が決めた handle）に bridge の `ax_text` を 1 回。
    /// bridge の口（W16-2 で `KangaeSurfaceBridge.swift` に足す・読むだけ）: 要求 `{"cmd":"ax_text","handle":"<WindowInfo.handle>"}` →
    /// 応答 `{"handle":"…","text":"…"|null,"reason":"ok"|"ax_no_text_area"|"ax_value_error_<AXError の番号>"|"value_type_<CFTypeID>"}`（`KangaeMirror.swift` の `ax-text` と同じ形・
    /// 窓の AX の子を幅優先で辿った最初の AXTextArea の AXValue・NSAttributedString は string にする・大きさの上限は置かない）。許可が無ければ他の口と同じ `not_trusted` の error。
    /// 例: `let (before, now) = self.surfaces.readback(project_id, spec, workspace.as_deref())?;`
    /// 保証する: 読むだけ（AX の set・押す・activate・raise・`open_app` を送らない・窓を動かさない。人の直接操作の道を Agent の道と混ぜない）・
    /// 重ねていない面は `ax_value = Err(NotAttached)` で disk だけ読む（窓を探し直さない・attach しない）・Hidden（別の Page）の間も読む。
    /// Err は spec が Native Direct の App でない時（`NotNative`）だけ。保証しない: 読んだ値が人の手の結果である事（`Readback::receipt` と試験の側）。
    pub fn readback(&mut self, project_id: &str, spec: &SurfaceSpec, workspace: Option<&Path>) -> Result<(Option<Readback>, Readback), NativeError> {
        let (SurfaceBackendKind::NativeDirect, SurfaceTarget::App { bundle_id, file }) = (spec.backend, &spec.target) else { return Err(NativeError::NotNative) };
        let k = key(project_id, &spec.id);
        // 重ねる前は app の名前をまだ聞いていない: attach と同じ呼び名
        let app = self.names.get(&k).cloned().unwrap_or_else(|| short_name(bundle_id));
        let now = Readback { disk_hash: disk_sha(workspace, file.as_deref()), ax_value: self.ax_text(&k, app), verified_at: crate::runtime::now_rfc3339() };
        if now.ax_value.is_ok() {
            self.seen.insert(k.clone(), now.clone());
        }
        let before = self.reads.insert(k, (Instant::now(), now.clone())).map(|(_, r)| r);
        Ok((before, now))
    }

    /// 反証の直し（50dda73 の待たない道）: 窓の文書を最後に読めた読み（`ax_value` が Ok・この起動の中）。無ければ None。
    /// `Generated::readback_native` が `readback` の前に取り、`receipt` の比べる相手にする。`readback` の前の読みは応答待ち（Err）の読みでも進むので、
    /// それと比べると、待っている間（実測 13〜31 秒）の保存は応答を使う読みで「disk は変わっていない」になり verified にならない。
    pub fn seen(&self, project_id: &str, surface_id: &str) -> Option<Readback> {
        self.seen.get(&key(project_id, surface_id)).cloned()
    }

    /// W16-2: `draw_page` が今 frame で読み戻すか（その面をまだ読んでいない・前の読みから `READBACK_EVERY` 経った）。
    pub fn readback_due(&self, project_id: &str, surface_id: &str, now: Instant) -> bool {
        self.reads.get(&key(project_id, surface_id)).is_none_or(|(at, _)| now.saturating_duration_since(*at) >= READBACK_EVERY)
    }

    /// W16-2: 起きている bridge に app の版を聞く（`find_app`・読むだけ・bridge を起こさない）。読めなければ None。
    /// `readback_native` が、版の無い最新の receipt（作った時に bridge が起きなかった `project::unavailable`）の版を埋めるのに使う。
    /// 反証の直し: bridge は要求を 1 本の列で順に答えるので、ax_text の応答を待っている間は送らず None（同期で送ると UI の thread がその後ろで止まる）。
    /// 聞けた版は覚えて 2 度目を送らない（ponytail: 起動の間は覚えたまま。app を更新したら Shibubu を開き直すまで前の版）。
    pub fn app_version(&mut self, bundle_id: &str) -> Option<String> {
        if let Some(v) = self.versions.get(bundle_id) {
            return Some(v.clone());
        }
        if !self.ax_wait.is_empty() {
            return None;
        }
        let b = self.bridge.as_mut().filter(|b| b.alive())?;
        let lim = b.limit();
        let v = b.call("find_app", json!({ "bundle_id": bundle_id }), lim).ok()?;
        let v = v["version"].as_str().filter(|s| !s.trim().is_empty())?.to_string();
        self.versions.insert(bundle_id.to_string(), v.clone());
        Some(v)
    }

    /// 重ねた窓の文書（bridge の `ax_text`）。重ねていない（Dock が無い・窓が決まっていない・重なっていない状態）は `NotAttached`。
    /// 応答を `ax_budget` まで待ち、越えたら待たずに Err（待っている）で戻る。その面には応答が来るまで 2 つ目を送らず、pump が拾った応答を次の読みで使う。
    fn ax_text(&mut self, k: &Key, app: String) -> Result<String, NativeError> {
        let handle = self
            .docks
            .get(k)
            .filter(|(_, d)| matches!(d.life, Lifecycle::Active | Lifecycle::Resizing | Lifecycle::Hidden))
            .and_then(|(_, d)| d.window.as_ref())
            .map(|w| w.handle.clone());
        let Some((handle, b)) = handle.zip(self.bridge.as_mut().filter(|b| b.alive())) else {
            self.ax_wait.remove(k);
            self.ax_got.remove(k);
            return Err(NativeError::NotAttached { app });
        };
        if let Some(got) = self.ax_got.remove(k) {
            return got;
        }
        let waiting = || NativeError::Bridge(format!("Waiting for {app} to answer the document read"));
        if let Some((_, at)) = self.ax_wait.get(k) {
            if at.elapsed() < AX_STALE {
                return Err(waiting());
            }
            self.ax_wait.remove(k);
        }
        let id = b.send("ax_text", json!({ "handle": handle }))?;
        match b.wait(id, "ax_text", self.ax_budget) {
            Err(BridgeError::Timeout { .. }) => {
                self.ax_wait.insert(k.clone(), (id, Instant::now()));
                Err(waiting())
            }
            r => ax_reply(&app, r),
        }
    }
}

/// 枠の帯の detail（Shibubu の画面の 1 行）。重なっている間は空（状態語と renderer の説明の 1 行で足りる）・外で開いている間は本当に在る帯の一手を名指す・
/// 他は理由（native.rs が書いた物はそのまま、Dock が書いた物は旧名を Shibubu に）。
fn shown_detail(d: &Dock, state: PortalState) -> String {
    match d.life {
        Lifecycle::Active | Lifecycle::Resizing | Lifecycle::Hidden | Lifecycle::Attaching | Lifecycle::Discovered => String::new(),
        Lifecycle::Detached => format!("{} is open in its original window. Press {} to cover the frame again", d.spec.app_identity.name, state.next_step().label()),
        _ => d.reason.replace("Kangae", "Shibubu"),
    }
}

impl Drop for NativeSurfaces {
    /// Shibubu を閉じる: 今の identity を残してから、重ねた窓を元の位置に戻す（窓は閉じない）。
    fn drop(&mut self) {
        let keys: Vec<Key> = self.docks.keys().cloned().collect();
        for k in keys {
            let _ = self.persist_identity(&k.0, &k.1);
        }
        if let Some(b) = self.bridge.as_mut() {
            for (_, d) in self.docks.values_mut() {
                d.close(b);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::project::SurfaceBackendKind;
    use std::sync::atomic::{AtomicU32, Ordering};

    const SLOT: Frame = Frame { x: 40.0, y: 120.0, w: 700.0, h: 440.0 };

    fn order_ok(_: u32) -> Result<(), String> {
        Ok(())
    }
    fn order_fails(_: u32) -> Result<(), String> {
        Err("偽の失敗".into())
    }
    static ABOVE: AtomicU32 = AtomicU32::new(0);
    fn order_above_seen(cg: u32) -> Result<(), String> {
        ABOVE.store(cg, Ordering::SeqCst);
        Ok(())
    }
    /// 隠れたまま描かれた面を前に戻した回数（この static はその試験だけが使う）
    static BELOW_REVEAL: AtomicU32 = AtomicU32::new(0);
    fn order_below_counted(_: u32) -> Result<(), String> {
        BELOW_REVEAL.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 偽の bridge（tests/fake-surface-bridge.py）を `env`（`export A=B` の行）で起こし、受け取った要求の log と記録の file の場所を返す。
    pub(crate) fn rig(name: &str, env: &[(&str, &str)]) -> (NativeSurfaces, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("shibubu-w16-1-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (log, rec) = (dir.join("requests.log"), dir.join(OVERLAY_LOG));
        let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py");
        let mut sh = format!("#!/bin/sh\nexport FAKE_LOG='{}'\n", log.display());
        for (k, v) in env {
            sh.push_str(&format!("export {k}='{v}'\n"));
        }
        sh.push_str(&format!("exec python3 '{}'\n", py.display()));
        let path = dir.join("bridge.sh");
        std::fs::write(&path, sh).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = NativeSurfaces::new(Some(rec.clone()));
        // 高負荷で python の起動が遅い（project::tests と同じ 30 秒）
        s.bridge = Some(Bridge::spawn(&path, Duration::from_secs(30)).unwrap());
        s.order_above = order_ok;
        s.order_below = order_ok;
        // 混雑で偽の bridge の ax_text も遅い。待たずに戻る道は w16_2_a_slow_ax_text_* が AX_BUDGET を短くして通す
        s.ax_budget = Duration::from_secs(30);
        (s, log, rec)
    }

    pub(crate) const TEXTEDIT: [(&str, &str); 3] = [("FAKE_APP_BUNDLE", "com.apple.TextEdit"), ("FAKE_APP_NAME", "TextEdit"), ("FAKE_TITLES", "README.md")];

    /// `TEXTEDIT` に `extra` を足した env（`rig` に渡す）。
    pub(crate) fn textedit<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        TEXTEDIT.iter().copied().chain(extra.iter().copied()).collect()
    }

    /// W16-2 の試験の repo: `.git` と `README.md`（"# demo\n"）を置いた dir と、偽の bridge が窓の文書として読む file（`.git/ax.txt`・中身 `ax`）。
    pub(crate) fn repo(name: &str, ax: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("shibubu-w16-2-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("README.md"), "# demo\n").unwrap();
        let ax_file = dir.join(".git/ax.txt");
        std::fs::write(&ax_file, ax).unwrap();
        (dir, ax_file)
    }

    fn docs() -> SurfaceSpec {
        SurfaceSpec {
            id: "docs".into(),
            backend: SurfaceBackendKind::NativeDirect,
            target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("README.md".into()) },
            keep_alive: true,
            frame: None,
            browser: false,
        }
    }

    fn requests(log: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
    fn count(log: &Path, cmd: &str) -> usize {
        requests(log).iter().filter(|r| r["cmd"] == cmd).count()
    }
    pub(crate) fn records(rec: &Path) -> Vec<OverlayRecord> {
        std::fs::read_to_string(rec).unwrap_or_default().lines().map(|l| serde_json::from_str(l).expect("契約の欄だけ")).collect()
    }

    /// 枠を渡し続けて、`project` の docs の面が重なる（Active）まで frame を回す。
    pub(crate) fn settle_in(s: &mut NativeSurfaces, project: &str) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            s.on_frame(project, "docs", Host { slot: Some(SLOT), ..Host::default() }, None, Instant::now());
            if life(s, project, "docs") == Some(Lifecycle::Active) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("20 秒で重ならない: {:?}", s.docks.get(&key(project, "docs")).map(|(_, d)| (d.life, d.reason.clone())));
    }
    /// 枠を SLOT に描いた 1 frame（project "p" の面 "docs"）。
    fn frame(s: &mut NativeSurfaces) {
        s.on_frame(
            "p",
            "docs",
            Host {
                slot: Some(SLOT),
                ..Host::default()
            },
            None,
            Instant::now(),
        );
    }
    fn settle(s: &mut NativeSurfaces) {
        settle_in(s, "p")
    }
    /// on_frame を `done` まで回す（10 秒で止める）。`refresh_space` の読みは待たずに送るので、効くのは応答を拾った後の frame。
    fn frames_until(s: &mut NativeSurfaces, done: impl Fn(&NativeSurfaces) -> bool) {
        let t = Instant::now();
        while !done(s) && t.elapsed() < Duration::from_secs(10) {
            frame(s);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// 面の今と、その面を置いた Page（試験の側の読み取り）。
    pub(crate) fn life(s: &NativeSurfaces, project: &str, sid: &str) -> Option<Lifecycle> {
        s.docks.get(&key(project, sid)).map(|(_, d)| d.life)
    }
    fn dock_page(s: &NativeSurfaces, project: &str) -> Option<String> {
        s.docks.get(&key(project, "docs")).map(|(pg, _)| pg.clone())
    }

    fn win(name: &str, title: &str, main: bool) -> WindowInfo {
        WindowInfo {
            handle: name.into(),
            pid: 1,
            title: title.into(),
            subrole: "AXStandardWindow".into(),
            main,
            movable: true,
            frame: Some(SLOT),
            cg_id: Some(9),
            ..WindowInfo::default()
        }
    }

    #[test]
    fn picks_the_file_window_and_never_substitutes_another() {
        let list = [win("a", "notes.txt", true), win("b", "README.md", false), win("c", "README.md.bak", false)];
        assert_eq!(pick_document_window(&list, "README.md").unwrap().handle, "b");
        assert_eq!(pick_document_window(&[win("b", "README.md — 編集済み", false)], "README.md").unwrap().handle, "b");
        // README.md が無ければ main の notes.txt にすり替えない（L4）
        assert!(pick_document_window(&list[..1], "README.md").is_err());
        assert!(pick_document_window(&[win("c", "README.md.bak", true)], "README.md").is_err());
        // 2 つ在れば main、main が無ければ決めない
        assert_eq!(pick_document_window(&[win("x", "README.md", false), win("y", "README.md", true)], "README.md").unwrap().handle, "y");
        assert!(pick_document_window(&[win("x", "README.md", false), win("y", "README.md", false)], "README.md").is_err());
        let mut min = win("m", "README.md", true);
        min.minimized = true;
        assert!(pick_document_window(&[min], "README.md").is_err());
    }

    /// 通し（偽の bridge）: 重ねる → 記録 attached → 別の Page で隠す（host_hidden）→ 戻ると同じ窓（host_shown）。前面を取る要求を 1 つも送らない。
    #[test]
    fn overlays_hides_and_returns_to_the_same_window_without_taking_front() {
        let (mut s, log, rec) = rig("pass", &TEXTEDIT);
        s.order_above = order_above_seen;
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        assert_eq!(count(&log, "windows"), 1, "2 回目の ensure は窓を探し直さない（同じ窓 id のまま）");
        settle(&mut s);
        assert_eq!(s.window("p", "docs"), Some((222, Some(500))));
        let id = s.identity("p", "docs").expect("重ねている窓");
        assert_eq!(
            (
                id.schema.as_str(),
                id.pid,
                id.window_id,
                id.page_id.as_str()
            ),
            (IDENTITY_SCHEMA, 222, 500, "page.docs")
        );
        let v = s.view("p", "docs").expect("枠を出す");
        assert_eq!((v.app_name.as_str(), v.state), ("TextEdit", PortalState::NativeDirect));
        let set = requests(&log).into_iter().filter(|r| r["cmd"] == "set_frame").last().expect("枠へ送った");
        assert_eq!((set["x"].as_f64(), set["y"].as_f64(), set["w"].as_f64(), set["h"].as_f64()), (Some(SLOT.x), Some(SLOT.y), Some(SLOT.w), Some(SLOT.h)));

        s.hide_page("page.other");
        assert_eq!(life(&s, "p", "docs").unwrap(), Lifecycle::Active, "別の Page の hide は触らない");
        s.hide_page("page.docs");
        assert_eq!((life(&s, "p", "docs").unwrap(), ABOVE.load(Ordering::SeqCst)), (Lifecycle::Hidden, 500));
        assert_eq!(s.identity("p", "docs").map(|i| (i.pid, i.window_id)), Some((222, 500)), "隠しても同じ窓");
        s.show_page("page.docs");
        assert_eq!(life(&s, "p", "docs").unwrap(), Lifecycle::Active);
        assert_eq!(s.window("p", "docs"), Some((222, Some(500))), "戻ると同じ窓");
        assert_eq!(count(&log, "windows"), 1, "戻っても attach し直さない");

        let r = records(&rec);
        let events: Vec<OverlayEvent> = r.iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached, OverlayEvent::HostHidden, OverlayEvent::HostShown]);
        for x in &r {
            assert_eq!((x.surface_id.as_str(), x.page_id.as_str(), x.bundle_id.as_str(), x.pid, x.window_id), ("docs", "page.docs", "com.apple.TextEdit", Some(222), Some(500)));
            assert_eq!(x.frame, Some(SLOT));
        }
        // 前面を取る手（activate つきの raise・app を起こす）は 1 つも送らない
        for q in requests(&log) {
            assert!(!(q["cmd"] == "raise" && q["activate"] == true), "前面を取った: {q}");
            assert_ne!(q["cmd"], "open_app", "app を起こした: {q}");
        }
    }

    /// 実機 2 回目（279f420）: egui の枠は最初の数 frame で少し動く。最初の set_frame の応答（Attached）と次の送り（HostMoved）が
    /// 同じ pump に入ると Attaching → Resizing になり、attached の記録が抜けていた。
    #[test]
    fn attached_is_recorded_even_when_the_frame_moves_in_the_same_pump() {
        let (mut s, _log, rec) = rig("moving", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        let moved = Frame { x: SLOT.x + 3.0, ..SLOT };
        let t = Instant::now();
        let mut i = 0;
        while !matches!(life(&s, "p", "docs"), Some(Lifecycle::Active | Lifecycle::Resizing)) && t.elapsed() < Duration::from_secs(20) {
            let slot = if i % 2 == 0 { SLOT } else { moved };
            s.on_frame("p", "docs", Host { slot: Some(slot), ..Host::default() }, None, Instant::now());
            i += 1;
            std::thread::sleep(Duration::from_millis(20));
        }
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached], "重なった時に attached を 1 件（今: {:?}）", life(&s, "p", "docs"));
    }

    /// 実機 3 回目: 背景の TextEdit は最初の set_frame を途中の大きさで止める事がある（同じ枠をもう 1 回送ると合った 6/6）。
    /// 1 回の食い違いは送り直して重ねる・2 回続けば degraded（合わない枠で回り続けない）。
    #[test]
    fn a_partial_first_resize_is_sent_again_once_and_then_overlays() {
        let (mut s, log, rec) = rig("partial1", &textedit(&[("FAKE_PARTIAL_SETS", "1")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        assert_eq!(count(&log, "set_frame"), 2, "同じ枠をもう 1 回だけ送った");
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached]);

        let (mut s, log, rec) = rig("partial2", &textedit(&[("FAKE_PARTIAL_SETS", "2")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        let t = Instant::now();
        while life(&s, "p", "docs") != Some(Lifecycle::Degraded) && t.elapsed() < Duration::from_secs(20) {
            frame(&mut s);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Degraded), "2 回続けて合わなければ degraded");
        assert_eq!(count(&log, "set_frame"), 2, "送り直しは 1 回だけ");
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Degraded]);
    }

    /// W16-3（pane N の実機）: set_frame の直後の actual が途中の値でも、落ち着いた窓が枠と合えば送り直さずに重ねる（すぐ送り直すと動いている途中の窓に重なり、落ち着いた先がまたずれた）
    #[test]
    fn a_lagging_resize_is_read_again_after_it_settles_and_not_sent_twice() {
        let (mut s, log, rec) = rig("lagged", &textedit(&[("FAKE_LAGGED_SETS", "1")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        assert_eq!(count(&log, "set_frame"), 1, "落ち着いた窓が枠と合っていれば送り直さない");
        assert!(count(&log, "frame") >= 1, "落ち着いた後に窓を読み直す");
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached]);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3（pane N の実機）: 重ねに失敗した面（Degraded）には今の Space の読みを送らない（nap の TextEdit への frame が、繋ぎ直しに要る windows / set_frame の前に bridge の 1 本の列を占める）
    #[test]
    fn a_degraded_face_is_not_read_for_the_space() {
        let (mut s, log, rec) = rig("degraded-no-space", &textedit(&[("FAKE_PARTIAL_SETS", "2")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        frames_until(&mut s, |s| life(s, "p", "docs") == Some(Lifecycle::Degraded));
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Degraded));
        let before = requests(&log).len();
        let t = Instant::now();
        while t.elapsed() < SPACE_EVERY + Duration::from_secs(1) {
            frame(&mut s);
            std::thread::sleep(Duration::from_millis(20));
        }
        let reads = requests(&log)[before..].iter().filter(|r| r["cmd"] == "frame").count();
        assert_eq!(reads, 0, "degraded の面に Space の読みを送った");
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3: 同じ bundle の process が 2 つ（`open -n`・先に起きた 333 は README の窓を持たない）なら README の窓を持つ 222 に重ね、起こし直した後も保存した pid の 222 に繋ぐ
    #[test]
    fn the_process_that_holds_the_file_is_picked_and_reconnected() {
        let (mut s, _log, rec) = rig("two-procs", &textedit(&[("FAKE_OTHER_PID", "333")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        assert_eq!(s.window("p", "docs"), Some((222, Some(500))), "先に起きた 333 に決め打ちしない");
        s.persist_identity("p", "docs").unwrap();
        s.forget_docks();
        assert_eq!(s.restore("p").unwrap(), 1);
        settle(&mut s);
        assert_eq!(s.identity("p", "docs").map(|i| (i.pid, i.window_id)), Some((222, 500)));
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3（pane N の実機）: 別の Page へ移った面（「TextEdit を右へ」）は今の Page で窓を Shibubu の後ろへ隠し、移った先の Page に入ると同じ窓で戻す
    #[test]
    fn a_face_moved_to_another_page_is_hidden_and_shown_on_its_page() {
        let (mut s, _log, rec) = rig("moved-hide", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        s.retarget_page("p", "docs", "page.diagram");
        s.hide_unseen("p", "page.docs");
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Hidden), "別の Page へ移った面の窓を今の Page に浮かせない");
        s.hide_unseen("p", "page.docs");
        s.show_page("page.diagram");
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Active));
        assert_eq!(s.window("p", "docs"), Some((222, Some(500))), "同じ窓");
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached, OverlayEvent::HostHidden, OverlayEvent::HostShown], "隠すのは 1 回だけ（毎 frame 記録しない）");
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3（pane N の実機）: 枠が全部隠れるまで scroll した面（slot が None）は窓を Shibubu の後ろへ隠し、ensure で戻さず、枠がまた見えた frame で同じ窓に戻す
    #[test]
    fn a_scrolled_out_face_hides_and_comes_back_when_seen() {
        let (mut s, _log, rec) = rig("scrolled-out", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let out = Host { slot: None, ..Host::default() };
        s.on_frame("p", "docs", out, None, Instant::now());
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Hidden), "見えていない枠の窓を Page の上に浮かせない");
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        s.on_frame("p", "docs", out, None, Instant::now());
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Hidden), "隠れている間に ensure で戻して毎 frame 出し入れしない");
        frame(&mut s);
        // 戻した直後は枠へ送り直している（reveal が tracker を戻す）
        assert!(matches!(life(&s, "p", "docs"), Some(Lifecycle::Active | Lifecycle::Resizing)), "枠が見えたら戻す: {:?}", life(&s, "p", "docs"));
        settle(&mut s);
        assert_eq!(s.window("p", "docs"), Some((222, Some(500))));
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached, OverlayEvent::HostHidden, OverlayEvent::HostShown]);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    #[test]
    fn a_window_closed_while_hidden_is_source_gone_not_shown() {
        let gone = std::env::temp_dir().join(format!("shibubu-w16-1-closed-{}", std::process::id()));
        let _ = std::fs::remove_file(&gone);
        let (mut s, _log, rec2) = rig("gone", &textedit(&[("FAKE_GONE_FILE", gone.to_str().unwrap())]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        s.hide_page("page.docs");
        std::fs::write(&gone, "").unwrap();
        s.show_page("page.docs");
        let _ = std::fs::remove_file(&gone);
        assert_eq!(life(&s, "p", "docs").unwrap(), Lifecycle::SourceUnavailable);
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::SourceUnavailable, "消えた窓を「直接操作中」と出さない");
        assert_eq!(s.identity("p", "docs"), None, "終了した app の identity は残さない");
        let events: Vec<OverlayEvent> = records(&rec2).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached, OverlayEvent::HostHidden, OverlayEvent::SourceGone]);
        assert_eq!(records(&rec2)[2].window_id, Some(500), "消えた窓の id を残す");
    }

    #[test]
    fn a_failed_hide_is_not_recorded_as_hidden() {
        let (mut s, _log, rec) = rig("hidefail", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        s.order_above = order_fails;
        s.hide_page("page.docs");
        assert_eq!(life(&s, "p", "docs").unwrap(), Lifecycle::Active);
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached]);
    }

    /// 反証 1: composer は Project ごとに同じ surface id "docs" を作る。A の Docs → B の Docs で、B は A の隠れた Dock を受け継がず、自分の窓を探して重なる。
    #[test]
    fn a_second_project_gets_its_own_dock_not_the_first_projects_hidden_one() {
        let (mut s, log, rec) = rig("twoprojects", &TEXTEDIT);
        s.ensure("project.a", "page.a.docs", &docs(), None).unwrap();
        settle_in(&mut s, "project.a");
        s.hide_page("page.a.docs");
        s.show_page("page.b.docs");
        s.ensure("project.b", "page.b.docs", &docs(), None).unwrap();
        assert_eq!(count(&log, "windows"), 2, "B の窓を探す");
        assert_eq!(dock_page(&s, "project.b").as_deref(), Some("page.b.docs"));
        settle_in(&mut s, "project.b");
        assert_eq!(life(&s, "project.a", "docs"), Some(Lifecycle::Hidden), "A は隠れたまま");
        assert_eq!(s.view("project.b", "docs").unwrap().state, PortalState::NativeDirect);
        let pages: Vec<(String, OverlayEvent)> = records(&rec).into_iter().map(|r| (r.page_id, r.event)).collect();
        assert_eq!(
            pages,
            [("page.a.docs".into(), OverlayEvent::Attached), ("page.a.docs".into(), OverlayEvent::HostHidden), ("page.b.docs".into(), OverlayEvent::Attached)]
        );
    }

    /// 反証 2・4: 隠した後に Page を離れる手が断られ、同じ Page がまた描かれた（ensure が来た）→ 窓を Shibubu の真下へ戻して HostShown を記録する。
    /// 戻さずに on_host が Hidden → Active にすると、窓は Shibubu の後ろなのに「直接操作中」と出る。
    #[test]
    fn a_hidden_surface_drawn_again_is_put_back_in_front_of_shibubu() {
        let (mut s, _log, rec) = rig("reveal", &TEXTEDIT);
        s.order_below = order_below_counted;
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let before = BELOW_REVEAL.load(Ordering::SeqCst);
        s.hide_page("page.docs");
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        assert_eq!(BELOW_REVEAL.load(Ordering::SeqCst), before + 1, "窓を Shibubu の真下（= 前）へ戻した");
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Active));
        let events: Vec<OverlayEvent> = records(&rec).iter().map(|r| r.event).collect();
        assert_eq!(events, [OverlayEvent::Attached, OverlayEvent::HostHidden, OverlayEvent::HostShown], "host_hidden に対の host_shown");
    }

    /// 反証 5: 帯の detail は Shibubu の言葉。重なっている間は旧名・「真の子 View」を出さず、外で開いている間は本当に在る帯の一手を名指す。
    #[test]
    fn the_band_detail_is_shibubu_wording_and_names_a_real_button() {
        let (mut s, _log, _rec) = rig("detail", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let v = s.view("p", "docs").unwrap();
        for bad in ["Kangae", "true child", "child view"] {
            assert!(!v.detail.contains(bad), "{}", v.detail);
        }
        s.on_frame("p", "docs", Host { slot: Some(SLOT), ..Host::default() }, Some(PortalAction::Detach), Instant::now());
        // 切り離す → 元の位置へ戻す応答 → Detached（外で開いている）
        let t = Instant::now();
        while life(&s, "p", "docs") != Some(Lifecycle::Detached) && t.elapsed() < Duration::from_secs(20) {
            frame(&mut s);
            std::thread::sleep(Duration::from_millis(20));
        }
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::SyncDelayed);
        assert!(!v.detail.contains("Kangae"), "{}", v.detail);
        assert!(v.detail.contains(v.state.next_step().label()), "帯に在る一手を名指す: {}", v.detail);
    }

    /// 起動していない・許可が無い・README.md の窓が無い・面が Native でない: それぞれの 1 行。断った面は毎 frame 聞き直さず、Page に入り直すと聞き直す。
    #[test]
    fn refusals_are_one_line_and_not_reasked_every_frame() {
        let (mut s, log, rec) = rig("notrunning", &[]);
        let e = s.ensure("p", "page.docs", &docs(), None).unwrap_err();
        assert_eq!(e, NativeError::NotRunning { app: "TextEdit".into() });
        for _ in 0..5 {
            assert_eq!(s.ensure("p", "page.docs", &docs(), None), Err(e.clone()));
        }
        assert_eq!(count(&log, "apps"), 1, "断った面を毎 frame 聞き直さない");
        let v = s.view("p", "docs").unwrap();
        assert_eq!((v.state, v.detail.as_str()), (PortalState::Undiscovered, e.to_string().as_str()));
        s.show_page("page.docs");
        let _ = s.ensure("p", "page.docs", &docs(), None);
        assert_eq!(count(&log, "apps"), 2, "Page に入り直すと聞き直す");
        // 帯の一手（再探索）でも聞き直す
        s.on_frame("p", "docs", Host::default(), Some(PortalAction::Rediscover), Instant::now());
        let _ = s.ensure("p", "page.docs", &docs(), None);
        assert_eq!(count(&log, "apps"), 3);
        assert!(records(&rec).is_empty(), "重ねていない面は記録しない");

        let (mut s, _, _) = rig("untrusted", &textedit(&[("FAKE_MODE", "untrusted")]));
        assert_eq!(s.ensure("p", "page.docs", &docs(), None), Err(NativeError::NoAccessibility));
        assert_eq!(s.view("p", "docs").unwrap().state, PortalState::PermissionRequired);

        let (mut s, _, _) = rig("nowindow", &[TEXTEDIT[0], TEXTEDIT[1], ("FAKE_TITLES", "notes.txt")]);
        assert_eq!(s.ensure("p", "page.docs", &docs(), None), Err(NativeError::NoWindow { app: "TextEdit".into(), file: "README.md".into() }));

        let url = SurfaceSpec { id: "diagram".into(), backend: SurfaceBackendKind::WebView, target: SurfaceTarget::Url { url: "https://excalidraw.com".into() }, keep_alive: true, frame: None, browser: false };
        assert_eq!(s.ensure("p", "page.diagram", &url, None), Err(NativeError::NotNative));
        assert_eq!(s.view("p", "diagram"), None, "重ねない面は W3 の 1 行のまま");
    }

    /// 試験の側が読む欄の名前（契約）。名前を変えたらここが赤になる。
    #[test]
    fn overlay_record_field_names_are_the_contract() {
        let r = OverlayRecord {
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            event: OverlayEvent::HostShown,
            at_ms: 1.0,
            bundle_id: "com.apple.TextEdit".into(),
            pid: Some(123),
            window_id: None,
            frame: Some(Frame {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            }),
        };
        let v = serde_json::to_value(&r).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "at_ms",
                "bundle_id",
                "event",
                "frame",
                "page_id",
                "pid",
                "surface_id",
                "window_id"
            ]
        );
        assert_eq!(v["event"], "host_shown");
        assert_eq!(
            v["window_id"],
            serde_json::Value::Null,
            "決められない窓 id は null で残す（作らない）"
        );
        assert_eq!(serde_json::from_value::<OverlayRecord>(v).unwrap(), r);
        for (e, s) in [
            (OverlayEvent::Attached, "attached"),
            (OverlayEvent::HostHidden, "host_hidden"),
            (OverlayEvent::Degraded, "degraded"),
            (OverlayEvent::SourceGone, "source_gone"),
            (OverlayEvent::Reconnected, "reconnected"),
        ] {
            assert_eq!(serde_json::to_value(e).unwrap(), s);
        }
    }

    // ---- W16-2: 読み戻し ----

    fn sha(s: &str) -> String {
        crate::runtime::permission::sha256_hex(s.as_bytes())
    }

    /// disk の中身 `disk` と窓の文書 `ax`（None = 重なっていない）の 1 回の読み。
    fn read(disk: &str, ax: Option<&str>) -> Readback {
        Readback {
            disk_hash: Ok(sha(disk)),
            ax_value: ax.map(str::to_string).ok_or(NativeError::NotAttached { app: "TextEdit".into() }),
            verified_at: "2026-10-21T00:00:00.000Z".into(),
        }
    }

    /// W3 の create_project が書く最新の receipt（入っている・版つき・操作の効果はまだ読んでいない）。
    fn w3_last() -> SurfaceReceipt {
        use crate::domain::project::{ReceiptKind, ReceiptState};
        SurfaceReceipt {
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            kind: ReceiptKind::NativeApp,
            state: ReceiptState::Unverified,
            oracle: "find_app: 入っている（操作の効果はまだ読んでいない）".into(),
            app_id: "com.apple.TextEdit".into(),
            app_version: "1.19".into(),
            os_version: "14.5 (23F79)".into(),
            operation: String::new(),
            verified_at: String::new(),
            evidence_ref: String::new(),
        }
    }

    /// 決め方の表（`Readback::receipt` の 1〜5）。返した receipt は全部 validate を通り、verified 以外は成功の語を持たない。
    #[test]
    fn w16_2_a_save_is_verified_only_when_the_disk_changed_and_the_window_shows_it() {
        use crate::domain::project::{ReceiptState as S, IN_SYNC, NOT_ON_DISK};
        let last = w3_last();
        let saved = "# demo\nn-1\n";
        // 1. 保存: disk が変わり、窓の文書（末尾の改行なし）が disk と同じ → verified
        let r = read(saved, Some("# demo\nn-1")).receipt(Some(&read("# demo\n", Some("# demo\n"))), &last, "README.md").expect("保存を書く");
        assert_eq!((r.state, r.oracle.as_str(), r.evidence_ref.clone()), (S::Verified, SAVED, format!("file_sha256:{}", sha(saved))));
        assert_eq!((r.operation.as_str(), r.verified_at.as_str()), ("Read back the save of README.md from disk", "2026-10-21T00:00:00.000Z"));
        assert_eq!((r.surface_id.as_str(), r.page_id.as_str(), r.app_id.as_str(), r.app_version.as_str()), ("docs", "page.docs", "com.apple.TextEdit", "1.19"));
        assert!(r.validate().is_ok());
        // 窓の文書の方に末尾の改行が 1 つ多い時も同じ中身と見る
        assert_eq!(read("a", Some("a\n")).receipt(Some(&read("b", Some("b"))), &last, "README.md").map(|r| r.state), Some(S::Verified));
        // 保存 2 回目（sha が違う）は last が verified でも書く
        let again = read("# demo\nn-2\n", Some("# demo\nn-2\n")).receipt(Some(&read(saved, Some(saved))), &r, "README.md").expect("2 回目の保存");
        assert_eq!(again.state, S::Verified);
        assert_ne!(again.evidence_ref, r.evidence_ref);
        // 5. 初めての読み・disk が変わらない → 書かない（窓を開いただけで verified にしない）
        assert_eq!(read(saved, Some(saved)).receipt(None, &last, "README.md"), None);
        assert_eq!(read(saved, Some(saved)).receipt(Some(&read(saved, Some(saved))), &last, "README.md"), None);
        // 2. 窓の文書が disk と違う（打っただけ・保存の後の Undo）→ NOT_ON_DISK。同じ行は続けて書かない
        let typed = read("# demo\n", Some("# demo\nn-1\n")).receipt(Some(&read("# demo\n", Some("# demo\n"))), &last, "README.md").expect("保存していない変更");
        assert_eq!((typed.state, typed.oracle.as_str(), typed.evidence_ref.as_str()), (S::Unverified, NOT_ON_DISK, ""));
        assert_eq!(read("# demo\n", Some("# demo\nn-1 more\n")).receipt(None, &typed, "README.md"), None);
        // 4. 保存せずに Undo して窓が disk と同じに戻った → NOT_ON_DISK を残さず unverified（disk は変わっていないので verified にしない）。
        // 反証の直し: 「この間に保存は無い」とは言わない（AX の応答待ち・再起動の後は間に保存が在っても分からない）
        let back = read("# demo\n", Some("# demo\n")).receipt(Some(&read("# demo\n", Some("# demo\nn-1\n"))), &typed, "README.md").expect("古い 1 行を消す");
        assert_eq!((back.state, back.oracle.as_str()), (S::Unverified, IN_SYNC));
        assert!(!IN_SYNC.contains("保存はありません"), "{IN_SYNC}");
        // 3. disk を読めない → unavailable・oracle は Err の 1 行そのまま（74ec7fb の口・頭を足さない）。同じ理由は続けて書かない
        let unreadable = NativeError::FileUnreadable { file: "README.md".into(), why: "ありません".into() };
        let gone = Readback { disk_hash: Err(unreadable.clone()), ..read("x", Some("x")) };
        let u = gone.receipt(Some(&read("x", Some("x"))), &last, "README.md").expect("読めない");
        assert_eq!((u.state, u.oracle.clone()), (S::Unavailable, unreadable.to_string()));
        assert_eq!(gone.receipt(None, &u, "README.md"), None);
        // 74ec7fb の口: 読んだ時刻は verified 以外の receipt の verified_at にもそのまま入る
        for r in [&typed, &back, &u] {
            assert_eq!(r.verified_at, "2026-10-21T00:00:00.000Z", "{r:?}");
        }
        // 5. AX だけ読めない（重なっていない）→ disk が変わっていても書かない（誰の保存か分からない）
        assert_eq!(read(saved, None).receipt(Some(&read("# demo\n", None)), &last, "README.md"), None);
        // 版の無い last（作った時に bridge が起きなかった）は verified に下げず unverified と理由（validate を通る）
        for missing in [SurfaceReceipt { app_version: String::new(), ..last.clone() }, SurfaceReceipt { os_version: " ".into(), ..last.clone() }] {
            let r = read(saved, Some(saved)).receipt(Some(&read("# demo\n", Some("# demo\n"))), &missing, "README.md").unwrap();
            assert_eq!(r.state, S::Unverified, "{r:?}");
            assert!(r.oracle.starts_with(SAVED) && r.validate().is_ok(), "{}", r.oracle);
        }
        for r in [&typed, &back, &u] {
            assert!(r.validate().is_ok());
            for w in ["接続済み", "保存しました", "保存済み", "完了", "✓"] {
                assert!(!r.oracle.contains(w) && !r.operation.contains(w), "{r:?} に {w}");
            }
        }
    }

    /// 偽の bridge の上で: 重なる前は disk だけ（ax_text を送らない）・重なった後は ax_text を 1 回だけ（set・押す・前に出す・app を起こすを送らない）・
    /// 前の読みを返す・Page で隠れている間も読む・文書の欄が無い / disk を読めない / Native でない の各理由。
    #[test]
    fn w16_2_readback_reads_the_disk_and_the_overlaid_window_and_sends_nothing_else() {
        let (dir, ax) = repo("readback", "# demo\nn-1");
        let (mut s, log, _rec) = rig("readback", &textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap())]));
        let (before, now) = s.readback("p", &docs(), Some(&dir)).unwrap();
        assert_eq!((before, now.disk_hash.clone()), (None, Ok(sha("# demo\n"))));
        assert_eq!(now.ax_value, Err(NativeError::NotAttached { app: "TextEdit".into() }), "重なる前は窓を読まない");
        assert_eq!(count(&log, "ax_text") + count(&log, "windows"), 0, "読み戻しは窓を探さない・attach しない");
        assert!(now.verified_at.ends_with('Z') && now.verified_at.len() == "2026-10-21T00:00:00.000Z".len(), "{}", now.verified_at);

        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let sent = requests(&log).len();
        let (before2, now2) = s.readback("p", &docs(), Some(&dir)).unwrap();
        assert_eq!(before2, Some(now), "前の読みを返す");
        assert_eq!(now2.ax_value.as_deref(), Ok("# demo\nn-1"));
        // 最後の frame の Dock が送った読み（cg_windows）は偽の bridge の log に後から載る事がある。readback の分は ax_text 1 つで、書く・動かす要求は 0
        let after: Vec<String> = requests(&log)[sent..].iter().map(|r| r["cmd"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(after.iter().filter(|c| *c == "ax_text").count(), 1, "{after:?}");
        assert!(!after.iter().any(|c| ["set_frame", "raise", "open_app", "windows", "observe", "set_value", "permission"].contains(&c.as_str())), "読むだけ: {after:?}");
        assert!(!s.readback_due("p", "docs", Instant::now()), "読んだばかり");
        assert!(s.readback_due("p", "docs", Instant::now() + READBACK_EVERY), "READBACK_EVERY の後");
        assert!(s.readback_due("p", "other", Instant::now()), "読んでいない面");

        s.hide_page("page.docs");
        assert_eq!(s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value.as_deref(), Ok("# demo\nn-1"), "隠れている間も読む");
        std::fs::remove_file(&ax).unwrap();
        assert_eq!(s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value, Err(NativeError::NoTextArea { app: "TextEdit".into() }));
        let no_repo = s.readback("p", &docs(), None).unwrap().1.disk_hash;
        assert_eq!(no_repo, Err(NativeError::FileUnreadable { file: "README.md".into(), why: "This project has no repo location".into() }));
        std::fs::remove_file(dir.join("README.md")).unwrap();
        assert_eq!(s.readback("p", &docs(), Some(&dir)).unwrap().1.disk_hash, Err(NativeError::FileUnreadable { file: "README.md".into(), why: "Not found".into() }));
        let url = SurfaceSpec { id: "diagram".into(), backend: SurfaceBackendKind::WebView, target: SurfaceTarget::Url { url: "https://excalidraw.com".into() }, keep_alive: true, frame: None, browser: false };
        assert_eq!(s.readback("p", &url, Some(&dir)), Err(NativeError::NotNative));
        assert_eq!(s.app_version("com.apple.TextEdit").as_deref(), Some("1.19"), "起きている bridge に版を聞く");
        for q in requests(&log) {
            for bad in ["set_value", "open_app", "activate"] {
                assert_ne!(q["cmd"], bad, "{q}");
            }
            assert!(!(q["cmd"] == "raise" && q["activate"] == true), "前面を取った: {q}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 実測（load 93 で背景の TextEdit の ax_text が 16 秒）: 遅い ax_text は UI の thread で ax_budget だけ待って戻り、応答が来るまで同じ面に 2 つ目を送らない
    /// （2 秒ごとに送って bridge の列を伸ばさない）。pump が拾った応答を次の読みで使い、その後にまた送る。
    #[test]
    fn w16_2_a_slow_ax_text_is_not_waited_on_and_not_sent_twice() {
        let (dir, ax) = repo("slow", "# demo\n");
        let (mut s, log, _rec) = rig("slowax", &textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap()), ("FAKE_AX_SLOW", "2")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        s.ax_budget = Duration::from_millis(100);
        let t = Instant::now();
        let first = s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value;
        assert!(t.elapsed() < Duration::from_millis(1500), "待ちすぎた: {:?}", t.elapsed());
        assert!(matches!(&first, Err(NativeError::Bridge(m)) if m.contains("Waiting for")), "{first:?}");
        assert!(s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value.is_err());
        // 応答が来るまで frame を回す（pump が拾う）
        let t = Instant::now();
        while s.ax_got.is_empty() && t.elapsed() < Duration::from_secs(30) {
            frame(&mut s);
            std::thread::sleep(Duration::from_millis(50));
        }
        // 偽の bridge は 1 本の列で、log は要求を読んだ時に書く: 待つ間に 2 つ目を送っていれば、1 つ目の応答の直後に 2 行目が載る
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(count(&log, "ax_text"), 1, "応答が来るまで 2 つ目を送らない");
        // 次の読みは拾った応答を使い、送らない
        assert_eq!(s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value.as_deref(), Ok("# demo\n"));
        let _ = s.readback("p", &docs(), Some(&dir));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count(&log, "ax_text"), 2, "使った後はまた送る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反証の直し（50dda73 の待たない道）: bridge は要求を 1 本の列で順に答えるので、ax_text の応答を待っている間に find_app を同期で送ると
    /// UI の thread がその後ろで止まる。待っている間は版を聞かず None・聞けた版は覚えて 2 度目を送らない。
    #[test]
    fn w16_2_app_version_does_not_wait_behind_a_pending_ax_text_and_is_asked_once() {
        let (dir, ax) = repo("version", "# demo\n");
        let (mut s, log, _rec) = rig("version", &textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap()), ("FAKE_AX_SLOW", "3")]));
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        s.ax_budget = Duration::from_millis(100);
        assert!(s.readback("p", &docs(), Some(&dir)).unwrap().1.ax_value.is_err(), "応答を待っている");
        let t = Instant::now();
        assert_eq!(s.app_version("com.apple.TextEdit"), None, "待っている間は聞かない");
        assert!(t.elapsed() < Duration::from_millis(1000), "ax_text の後ろで待った: {:?}", t.elapsed());
        let t = Instant::now();
        while !s.ax_wait.is_empty() && t.elapsed() < Duration::from_secs(30) {
            frame(&mut s);
            std::thread::sleep(Duration::from_millis(50));
        }
        let asked = count(&log, "find_app");
        assert_eq!(s.app_version("com.apple.TextEdit").as_deref(), Some("1.19"), "列が空けば聞く");
        assert_eq!(s.app_version("com.apple.TextEdit").as_deref(), Some("1.19"));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count(&log, "find_app"), asked + 1, "聞けた版は覚えて 2 度目を送らない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 試験の道具: ax_text を待つ上限（rig は混雑に合わせて 30 秒。待たない道を通す generated の試験が短くする）。
    pub(crate) fn set_ax_budget(s: &mut NativeSurfaces, d: Duration) {
        s.ax_budget = d;
    }

    /// 理由の 1 行に成功の語（YC_ACCEPTANCE §4 L1 の表）が無く、次の一手がある。
    #[test]
    fn errors_are_one_line_without_success_words() {
        let all = [
            NativeError::NotNative,
            NativeError::Bridge("x".into()),
            NativeError::NotRunning {
                app: "TextEdit".into(),
            },
            NativeError::NoAccessibility,
            NativeError::NoWindow {
                app: "TextEdit".into(),
                file: "README.md".into(),
            },
            NativeError::NotAttached { app: "TextEdit".into() },
            NativeError::FileUnreadable { file: "README.md".into(), why: "ありません".into() },
            NativeError::NoTextArea { app: "REAPER".into() },
            NativeError::OffSpace { app: "TextEdit".into(), kind: OffSpaceKind::HostFullscreen },
            NativeError::OffSpace { app: "TextEdit".into(), kind: OffSpaceKind::SourceFullscreen },
            NativeError::OffSpace { app: "TextEdit".into(), kind: OffSpaceKind::DifferentSpace },
            NativeError::Gone { app: "TextEdit".into() },
            NativeError::WindowClosed { app: "TextEdit".into(), file: "README.md".into() },
            NativeError::AmbiguousWindow { app: "TextEdit".into(), file: "README.md".into() },
            NativeError::HumanHasControl { app: "TextEdit".into(), action: AgentAction::Input },
        ];
        for e in all {
            let s = e.to_string();
            assert!(!s.contains('\n'), "{s}");
            for w in [
                "接続済み",
                "保存しました",
                "保存済み",
                "済み",
                "完了",
                "✓",
                "Connected",
                "Saved",
                "Done",
            ] {
                assert!(!s.contains(w), "{s} に {w}");
            }
        }
        let off = NativeError::OffSpace {
            app: "TextEdit".into(),
            kind: OffSpaceKind::DifferentSpace,
        }
        .to_string();
        assert!(
            off.contains("Open in") && off.contains("isn't in this Space"),
            "{off}"
        );
        let gone = NativeError::Gone {
            app: "TextEdit".into(),
        }
        .to_string();
        assert!(
            gone.contains("has quit") && gone.contains("Reopen"),
            "{gone}"
        );
    }

    /// W16-3: identity の JSON 欄の名前は契約。終了した app の error は SourceUnavailable（接続済みと出さない）。Space は IntegrationFailed。
    #[test]
    fn w16_3_identity_schema_and_error_views() {
        let id = OverlayIdentity {
            schema: IDENTITY_SCHEMA.into(),
            project_id: "p".into(),
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            bundle_id: "com.apple.TextEdit".into(),
            pid: 222,
            window_id: 500,
            title: "README.md".into(),
        };
        let v = serde_json::to_value(&id).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "bundle_id",
                "page_id",
                "pid",
                "project_id",
                "schema",
                "surface_id",
                "title",
                "window_id"
            ]
        );
        assert_eq!(v["schema"], IDENTITY_SCHEMA);
        assert_eq!(serde_json::from_value::<OverlayIdentity>(v).unwrap(), id);
        let dir = std::env::temp_dir().join(format!("shibubu-w16-3-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = identity_path(&dir);
        id.append(&path).unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        assert!(
            line.contains(IDENTITY_SCHEMA) && line.contains("\"pid\":222"),
            "{line}"
        );
        let _ = std::fs::remove_dir_all(&dir);

        let mut s = NativeSurfaces::new(None);
        s.errors.insert(
            key("p", "docs"),
            NativeError::Gone {
                app: "TextEdit".into(),
            },
        );
        s.names.insert(key("p", "docs"), "TextEdit".into());
        let v = s.view("p", "docs").unwrap();
        assert_eq!(
            v.state,
            PortalState::SourceUnavailable,
            "終了した app を直接操作中と出さない"
        );
        assert!(
            !v.detail.contains("Connected") && v.detail.contains("has quit"),
            "{}",
            v.detail
        );
        s.errors.insert(
            key("p", "docs"),
            NativeError::OffSpace {
                app: "TextEdit".into(),
                kind: OffSpaceKind::HostFullscreen,
            },
        );
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::IntegrationFailed);
        assert!(
            v.detail.contains("Open in") && v.detail.contains("full screen"),
            "{}",
            v.detail
        );
        s.errors.insert(
            key("p", "docs"),
            NativeError::AmbiguousWindow {
                app: "TextEdit".into(),
                file: "README.md".into(),
            },
        );
        assert_eq!(
            s.view("p", "docs").unwrap().state,
            PortalState::Undiscovered
        );
        s.errors.insert(
            key("p", "docs"),
            NativeError::WindowClosed {
                app: "TextEdit".into(),
                file: "README.md".into(),
            },
        );
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::Undiscovered);
        assert!(
            v.detail.contains("was closed") && !v.detail.contains("has quit"),
            "{}",
            v.detail
        );
    }

    /// W16-3: identity の JSONL は重ねた記録（overlay.jsonl）ではない。schema も欄も違う。
    #[test]
    fn w16_3_identity_log_is_not_overlay_record() {
        assert_ne!(IDENTITY_LOG, OVERLAY_LOG);
        let dir = std::env::temp_dir().join(format!("shibubu-w16-3-idlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ident(222, 500).append(&identity_path(&dir)).unwrap();
        OverlayRecord {
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            event: OverlayEvent::Attached,
            at_ms: 1.0,
            bundle_id: "com.apple.TextEdit".into(),
            pid: Some(222),
            window_id: Some(500),
            frame: Some(SLOT),
        }
        .append(&record_path(&dir))
        .unwrap();
        let id_line = std::fs::read_to_string(identity_path(&dir)).unwrap();
        let rec_line = std::fs::read_to_string(record_path(&dir)).unwrap();
        assert!(
            serde_json::from_str::<OverlayRecord>(id_line.trim()).is_err(),
            "{id_line}"
        );
        assert!(
            serde_json::from_str::<OverlayIdentity>(rec_line.trim()).is_err(),
            "{rec_line}"
        );
        assert!(id_line.contains(IDENTITY_SCHEMA) && !rec_line.contains(IDENTITY_SCHEMA));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ident(pid: u32, window_id: i64) -> OverlayIdentity {
        OverlayIdentity {
            schema: IDENTITY_SCHEMA.into(),
            project_id: "p".into(),
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            bundle_id: "com.apple.TextEdit".into(),
            pid,
            window_id,
            title: "README.md".into(),
        }
    }

    fn twin(pid: u32, cg: i64) -> WindowInfo {
        WindowInfo {
            handle: format!("h{cg}"),
            pid,
            title: "README.md".into(),
            subrole: "AXStandardWindow".into(),
            main: true,
            movable: true,
            frame: Some(SLOT),
            cg_id: Some(cg),
            on_screen: Some(true),
            ..WindowInfo::default()
        }
    }

    /// W16-3: 同じ pid・window_id だけを取る。題名が複数でも id が 1 つならそれを返す。pid が違うのは Gone。pid は居て window_id が無い・題名が 1 つは WindowClosed（終了してはいない）。題名が複数で id が無いのは AmbiguousWindow。
    #[test]
    fn w16_3_match_identity_uses_pid_and_window_id_not_the_title() {
        let saved = ident(222, 500);
        let a = twin(222, 500);
        let b = twin(222, 501);
        assert_eq!(
            match_identity(
                &saved,
                "com.apple.TextEdit",
                Some(222),
                &[a.clone(), b.clone()]
            )
            .unwrap()
            .cg_id,
            Some(500),
            "同じ題名が複数でも window_id が 1 つならそれを取る"
        );
        assert!(matches!(
            match_identity(&saved, "com.apple.TextEdit", Some(999), &[a.clone()]),
            Err(NativeError::Gone { .. })
        ));
        let missing = ident(222, 999);
        assert!(matches!(
            match_identity(&missing, "com.apple.TextEdit", Some(222), &[a.clone(), b]),
            Err(NativeError::AmbiguousWindow { .. })
        ));
        let closed = match_identity(&missing, "com.apple.TextEdit", Some(222), &[a]).unwrap_err();
        assert!(
            matches!(closed, NativeError::WindowClosed { .. }),
            "{closed:?}"
        );
        let line = closed.to_string();
        assert!(
            line.contains("was closed") && !line.contains("has quit"),
            "{line}"
        );
    }

    /// W16-3: persist → Dock を捨てる → restore で同じ pid・window_id。記録は Reconnected（新しい Attached ではない）。pid が死んでいれば Gone・identity は None。
    #[test]
    fn w16_3_restore_reconnects_the_same_window_and_gone_is_not_connected() {
        let (mut s, _log, rec) = rig("w16-3-restore", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let win = s.window("p", "docs");
        assert_eq!(win, Some((222, Some(500))));
        s.persist_identity("p", "docs").unwrap();
        let path = rec.with_file_name(IDENTITY_LOG);
        let line = std::fs::read_to_string(&path).unwrap();
        assert!(
            line.contains(IDENTITY_SCHEMA) && line.contains("\"pid\":222"),
            "{line}"
        );
        s.forget_docks();
        assert_eq!(s.identity("p", "docs"), None);
        assert_eq!(s.restore("p").unwrap(), 1);
        settle(&mut s);
        assert_eq!(s.window("p", "docs"), win);
        assert_eq!(s.identity("p", "docs").unwrap().pid, 222);
        let events: Vec<_> = records(&rec).into_iter().map(|r| r.event).collect();
        assert!(
            events.contains(&OverlayEvent::Reconnected),
            "繋ぎ直しは Reconnected: {events:?}"
        );

        let gone = ident(999, 500);
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&gone).unwrap()),
        )
        .unwrap();
        s.forget_docks();
        assert_eq!(s.restore("p").unwrap(), 0);
        assert!(matches!(
            s.errors.get(&key("p", "docs")),
            Some(NativeError::Gone { .. })
        ));
        assert_eq!(
            s.identity("p", "docs"),
            None,
            "終了した app の identity は残さない"
        );
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::SourceUnavailable);
        assert!(
            !v.detail.contains("接続済み") && !v.detail.contains("直接操作中"),
            "{}",
            v.detail
        );
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3 反証の直し: 許可を外して Shibubu を起こし直すと、restore は「許可を開く」の 1 行（NoAccessibility）で止まる。
    /// bridge の not_trusted を Err で返して Project を開く 1 行を bridge の理由にしない（窓より先に許可を聞く）。
    #[test]
    fn w16_3_restore_without_accessibility_says_permission_not_bridge() {
        let (mut s, _log, rec) = rig("w16-3-untrusted", &textedit(&[("FAKE_MODE", "untrusted")]));
        ident(222, 500)
            .append(&rec.with_file_name(IDENTITY_LOG))
            .unwrap();
        assert_eq!(s.restore("p").unwrap(), 0);
        assert!(
            matches!(
                s.errors.get(&key("p", "docs")),
                Some(NativeError::NoAccessibility)
            ),
            "{:?}",
            s.errors.get(&key("p", "docs"))
        );
        assert_eq!(
            s.view("p", "docs").unwrap().state,
            PortalState::PermissionRequired
        );
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3: 20 回 Page を離れても同じ窓。retarget は page_id だけ。人が持っている間は HumanHasControl。Space / 全画面は OffSpace。
    #[test]
    fn w16_3_retarget_agent_may_off_space_and_twenty_hides_keep_the_window() {
        let (mut s, _log, rec) = rig("w16-3-ops", &TEXTEDIT);
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        let win = s.window("p", "docs");
        for _ in 0..20 {
            s.hide_page("page.docs");
            assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Hidden));
            s.show_page("page.docs");
            assert_eq!(s.window("p", "docs"), win);
        }
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Active));

        s.retarget_page("p", "docs", "page.diagram");
        assert_eq!(
            s.identity("p", "docs").unwrap().page_id.as_str(),
            "page.diagram"
        );
        assert_eq!(s.window("p", "docs"), win, "retarget は窓を動かさない");

        let e = s.agent_may("p", "docs", AgentAction::Input).unwrap_err();
        assert!(matches!(
            e,
            NativeError::HumanHasControl {
                action: AgentAction::Input,
                ..
            }
        ));
        assert!(e.to_string().contains("Hand over"), "{e}");
        s.docks
            .get_mut(&key("p", "docs"))
            .unwrap()
            .1
            .lease
            .hand_to_agent(1.0);
        s.agent_may("p", "docs", AgentAction::Input).unwrap();

        let host_fs = Host {
            fullscreen: true,
            slot: Some(SLOT),
            ..Host::default()
        };
        assert!(matches!(
            s.off_space("p", "docs", &host_fs),
            Err(NativeError::OffSpace {
                kind: OffSpaceKind::HostFullscreen,
                ..
            })
        ));
        s.docks
            .get_mut(&key("p", "docs"))
            .unwrap()
            .1
            .window
            .as_mut()
            .unwrap()
            .fullscreen = true;
        assert!(matches!(
            s.off_space("p", "docs", &Host::default()),
            Err(NativeError::OffSpace {
                kind: OffSpaceKind::SourceFullscreen,
                ..
            })
        ));
        {
            let w = s
                .docks
                .get_mut(&key("p", "docs"))
                .unwrap()
                .1
                .window
                .as_mut()
                .unwrap();
            w.fullscreen = false;
            w.on_screen = Some(false);
        }
        assert!(matches!(
            s.off_space("p", "docs", &Host::default()),
            Err(NativeError::OffSpace {
                kind: OffSpaceKind::DifferentSpace,
                ..
            })
        ));
        s.on_frame("p", "docs", host_fs, None, Instant::now());
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Degraded));
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::IntegrationFailed);
        assert!(
            v.detail.contains("Open in") && !v.detail.contains("Connected"),
            "{}",
            v.detail
        );
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-2-b / W16-3: disk が変わり窓が古い → CHANGED_OUTSIDE（保存させる 1 行にしない）。disk 同じ＋窓が違う → NOT_ON_DISK。
    #[test]
    fn w16_3_receipt_2b_changed_outside_is_not_a_save() {
        use crate::domain::project::{ReceiptState as S, CHANGED_OUTSIDE, NOT_ON_DISK};
        let last = w3_last();
        let outside = read("# demo\noutside\n", Some("# demo\n"))
            .receipt(
                Some(&read("# demo\n", Some("# demo\n"))),
                &last,
                "README.md",
            )
            .expect("外で書き換わった");
        assert_eq!(
            (
                outside.state,
                outside.oracle.as_str(),
                outside.evidence_ref.as_str()
            ),
            (S::Unverified, CHANGED_OUTSIDE, "")
        );
        assert!(
            !outside.operation.contains("save") && outside.validate().is_ok(),
            "{outside:?}"
        );
        let typed = read("# demo\n", Some("# demo\nn-1\n"))
            .receipt(
                Some(&read("# demo\n", Some("# demo\n"))),
                &last,
                "README.md",
            )
            .expect("打っただけ");
        assert_eq!(typed.oracle.as_str(), NOT_ON_DISK);

        let mut saved = w3_last();
        saved.state = S::Verified;
        saved.oracle = SAVED.into();
        saved.evidence_ref = format!("file_sha256:{}", sha("# demo\n"));
        let restarted = read("# demo\noutside\n", Some("# demo\n"))
            .receipt(None, &saved, "README.md")
            .expect("再起動後も外で書き換わった");
        assert_eq!(
            (restarted.state, restarted.oracle.as_str()),
            (S::Unverified, CHANGED_OUTSIDE)
        );
        assert!(
            !restarted.operation.contains("save") && restarted.validate().is_ok(),
            "{restarted:?}"
        );
        let caught = read("# demo\noutside\n", Some("# demo\noutside\n"))
            .receipt(None, &saved, "README.md")
            .expect("窓が新しい disk に追いついた");
        assert_eq!(caught.state, S::Verified);
        assert_eq!(
            caught.evidence_ref,
            format!("file_sha256:{}", sha("# demo\noutside\n"))
        );
    }

    /// W16-3: on_frame は Dock の WindowInfo を試験が書かなくても、今の cg_windows / frame で OffSpace になる。最小化は別 Space にしない。
    #[test]
    fn w16_3_off_space_on_frame_reads_live_cg_not_the_dock_snapshot() {
        let mark = std::env::temp_dir().join(format!("shibubu-w16-3-off-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let (mut s, _log, rec) = rig(
            "w16-3-live-space",
            &textedit(&[("FAKE_OFF_SCREEN_FILE", mark.to_str().unwrap())]),
        );
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Active));
        std::fs::write(&mark, "1").unwrap();
        frames_until(&mut s, |s| {
            life(s, "p", "docs") == Some(Lifecycle::Degraded)
        });
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Degraded));
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::IntegrationFailed);
        assert_ne!(v.state, PortalState::NativeDirect);
        assert!(
            v.detail.contains("Open in") && v.detail.contains("isn't in this Space"),
            "{}",
            v.detail
        );
        let _ = std::fs::remove_file(&mark);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3: 元の窓の全画面も attach 時の snapshot ではなく、今の frame（AXFullScreen）で OffSpace。
    #[test]
    fn w16_3_source_fullscreen_on_frame_is_off_space_not_native_direct() {
        let mark = std::env::temp_dir().join(format!("shibubu-w16-3-fs-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let (mut s, _log, rec) = rig(
            "w16-3-live-fs",
            &textedit(&[("FAKE_FULLSCREEN_FILE", mark.to_str().unwrap())]),
        );
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        std::fs::write(&mark, "1").unwrap();
        frames_until(&mut s, |s| {
            life(s, "p", "docs") == Some(Lifecycle::Degraded)
        });
        assert_eq!(life(&s, "p", "docs"), Some(Lifecycle::Degraded));
        let v = s.view("p", "docs").unwrap();
        assert_eq!(v.state, PortalState::IntegrationFailed);
        assert!(
            v.detail.contains("Open in") && v.detail.contains("full screen"),
            "{}",
            v.detail
        );
        let _ = std::fs::remove_file(&mark);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3 反証の直し: 元の app の AX が遅い（nap の TextEdit・W16-2 実機で 13〜31 秒）間も、on_frame は今の Space の読みを待たない（UI の thread を止めない）。
    /// 応答が来たら、その後の frame で読みが効く（応答待ちの間は 2 つ目を送らない）。
    #[test]
    fn w16_3_on_frame_does_not_wait_for_a_slow_space_read() {
        let mark = std::env::temp_dir().join(format!("shibubu-w16-3-slow-{}", std::process::id()));
        let off =
            std::env::temp_dir().join(format!("shibubu-w16-3-slow-off-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let _ = std::fs::remove_file(&off);
        let (mut s, log, rec) = rig(
            "w16-3-slow-frame",
            &textedit(&[
                ("FAKE_FRAME_SLOW_FILE", mark.to_str().unwrap()),
                ("FAKE_OFF_SCREEN_FILE", off.to_str().unwrap()),
            ]),
        );
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        std::fs::write(&mark, "1").unwrap();
        std::fs::write(&off, "1").unwrap();
        let sent_before = requests(&log).len();
        let t = Instant::now();
        let mut slowest = Duration::ZERO;
        // SPACE_EVERY を越えて回す（遅い読みを少なくとも 1 つ送らせる）
        while t.elapsed() < SPACE_EVERY + Duration::from_secs(2) {
            let one = Instant::now();
            frame(&mut s);
            slowest = slowest.max(one.elapsed());
            std::thread::sleep(Duration::from_millis(20));
        }
        // 偽の bridge は frame の応答の前に 3 秒待つ（待てば 1 回で約 3 秒）。混んだ機械の揺れは 1.5 秒の手前で吸う
        assert!(
            slowest < Duration::from_millis(1500),
            "遅い frame の読みを on_frame が待った: {slowest:?}"
        );
        let frames = requests(&log)[sent_before..]
            .iter()
            .filter(|r| r["cmd"] == "frame")
            .count();
        // 遅い読みは 1 つ送ったら応答（3 秒後）まで送らない。応答の後の次の間隔で 2 つ目まで
        assert!(
            (1..=2).contains(&frames),
            "遅い frame を送っていない・応答待ちの間に送り続けた: {frames}"
        );
        let _ = std::fs::remove_file(&mark);
        frames_until(&mut s, |s| {
            life(s, "p", "docs") == Some(Lifecycle::Degraded)
        });
        assert_eq!(
            s.view("p", "docs").unwrap().state,
            PortalState::IntegrationFailed,
            "応答が来た後の frame で別 Space が効く"
        );
        let _ = std::fs::remove_file(&off);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }

    /// W16-3: 最小化は on_screen=false でも DifferentSpace にしない。
    #[test]
    fn w16_3_minimized_is_not_different_space() {
        let mark = std::env::temp_dir().join(format!("shibubu-w16-3-min-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let (mut s, _log, rec) = rig(
            "w16-3-min",
            &textedit(&[("FAKE_MINIMIZED_FILE", mark.to_str().unwrap())]),
        );
        s.ensure("p", "page.docs", &docs(), None).unwrap();
        settle(&mut s);
        std::fs::write(&mark, "1").unwrap();
        // 読みの組（frame の minimized と cg の on_screen=false）が窓に書かれるまで回し、その後の frame も回す
        frames_until(&mut s, |s| {
            s.docks
                .get(&key("p", "docs"))
                .and_then(|(_, d)| d.window.as_ref())
                .is_some_and(|w| w.minimized && w.on_screen == Some(false))
        });
        frame(&mut s);
        assert!(
            s.off_space("p", "docs", &Host::default()).is_ok(),
            "最小化を別 Space にしない: {:?}",
            s.off_space("p", "docs", &Host::default())
        );
        let v = s.view("p", "docs").unwrap();
        assert!(!v.detail.contains("isn't in this Space"), "{}", v.detail);
        let _ = std::fs::remove_file(&mark);
        let _ = std::fs::remove_dir_all(rec.parent().unwrap());
    }
}
