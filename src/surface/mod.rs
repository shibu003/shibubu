//! Native App Portal の型と状態（v0.14 §5.1〜§5.4・§13）と、macOS bridge（§5.5・`bridge/macos-surface`）の client。
//! - 型: SurfaceBackend / SyncMode / Capability を別の型に持つ（表示・操作・意味の同期を混ぜない・§2）・窓（`WindowInfo`）・枠の 9 状態（`PortalState`）
//! - 状態: `ControlLease`（§5.3・Mirror の人と AI の取り合い）・app ごとの同意（Mirror と AI の操作）
//! - bridge の client（`Bridge`・1 行 1 JSON）・WebView の面の判定の純関数・AppKit の口（`appkit`）
//! - 重ね（Native Direct Docking の `Dock`・`Lifecycle`・.rpp の証拠）は `portal.rs`（常に重ねる方式は撤回・G91 の Mirror の面が `Dock` で本物をしまう / 前に出すので file は残す）
//! **真の子 View ではない**: 元の窓は別の窓のまま重ねるだけ。非公開 API（SkyLight / CGS）は使わない。図は docs/diagrams.md §18。
//! serde / serde_json だけに依る（eframe 無し）—— bin 4 本・tests の 9 本・audio/surface.rs・intel が `#[path]` で取り込む（この file が根）。
#![allow(dead_code)] // 統合役が app に配線するまで、本体の bin からは未使用

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// WebView の面（feature `webview` の時だけ wry 0.57・CD-24）。判定の純関数はこの file の「WebView の面」に在る。
#[cfg(all(feature = "webview", target_os = "macos"))]
#[path = "webview.rs"]
pub mod webview;

/// Interactive Mirror の人の口・自動の口・app ごとの同意（feature `mirror` の時だけ・YC W16M-0・W15 の .app では切る）。
#[cfg(feature = "mirror")]
#[path = "mirror.rs"]
pub mod mirror;

/// 重ね（Native Direct Docking）の Dock と証拠。G91 の Mirror の面が本物の窓をしまう・前に出すのにも使う。呼び元は `surface::<名前>` のまま引く。
#[path = "portal.rs"]
mod portal;
#[allow(unused_imports)]
pub use self::portal::*; // ponytail: 互換（ui/mirror_view.rs・project/native.rs・bin/kangae-portal.rs・tests/portal.rs・generated.rs の cfg）

/// TERM-XTERM-1101 の直し: この file は bin・tests・audio・intel が `#[path]` で取り込む根で `crate::` を引けない（`crate::crypto` を持たない bin の
/// kangae-surface・kangae-intel の test の compile が赤だった）。base64 は crypto.rs（std だけ・`crate::` 無し）を中に取り込んで使う。
/// ponytail: main の crate では crate::crypto と同じ中身がもう 1 つ compile される（数 KB）。
#[path = "../crypto.rs"]
mod crypto;

/// 人の操作で 1 回だけ呼ぶ要求（attach・外で開く・閉じる）の上限。混んだ機械では AX が遅いので長め。
pub const CALL_LIMIT: Duration = Duration::from_secs(5);
/// bridge の window_list は窓を 3 つの AX の属性（kAXWindows・MainWindow・FocusedWindow）で別々に読み、窓の欄の読みも含めて
/// 合計 この数 × 今の ax_timeout（起動時か config・G31b）で打ち切る（KangaeSurfaceBridge.swift の windowListReads と同じ数・W12W-1c）。
/// 読み手は `Bridge::window_list_limit` で待つ。要求の `budget_s`（任意）が短ければ bridge はそちらで打ち切る（`MacGui::patiently` の読み直しは 30 秒）。
pub const WINDOW_LIST_AX_READS: f64 = 3.0;
/// bridge の実行 file の名前（build-mac.sh が .app の Contents/Helpers に入れる・MIRROR-QUIET-1101）。
pub const BRIDGE_NAME: &str = "kangae-surface-bridge";

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

// ---- 型（§5.1・§5.4）----

/// 描画の方式（§5.1 surface_backend）。描画方式は型で宣言し、実機で確かめた能力だけ Capability で公開する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceBackend {
    #[serde(rename = "webview")]
    WebView,
    NativeDirect,
    NativePortalExperimental,
    InteractiveMirror,
    CooperativePlugin,
    ExternalHandoff,
    GeneratedUi,
}

/// 同期の確度（§2.2）。`ui::portal_frame::SyncMode` はこの型の再輸出（W2c で 1 つにした）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    VisualLive,
    SemanticLive,
    SemanticPolled,
    InferredUnverified,
    Stale,
    Unsupported,
}

impl SyncMode {
    pub const ALL: [SyncMode; 6] = [
        SyncMode::VisualLive,
        SyncMode::SemanticLive,
        SyncMode::SemanticPolled,
        SyncMode::InferredUnverified,
        SyncMode::Stale,
        SyncMode::Unsupported,
    ];

    /// §2.2 の表示ラベル
    pub const fn label(self) -> &'static str {
        match self {
            SyncMode::VisualLive => "Live view",
            SyncMode::SemanticLive => "Live state",
            SyncMode::SemanticPolled => "Checked periodically",
            SyncMode::InferredUnverified => "Guessed",
            SyncMode::Stale => "May be out of date",
            SyncMode::Unsupported => "Can't read its state",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusOwner {
    Human,
    Agent,
    None,
}

/// 3 層（§2）。どれが Ready かを別々に持つ（3 つを勝手に Ready にしない・§5.2 active）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// 表示
    Visual,
    /// 操作
    Input,
    /// 意味の同期
    Semantic,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::Visual => "Display",
            Layer::Input => "Control",
            Layer::Semantic => "State",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Readiness {
    /// 証拠つきで確かめた
    Ready { evidence: String, at_ms: f64 },
    /// 方式としては在るが、まだ確かめていない
    Unverified { reason: String },
    /// できない
    Unsupported { reason: String },
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, Readiness::Ready { .. })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Readiness::Ready { .. } => "Confirmed",
            Readiness::Unverified { .. } => "Checked when you open Connections",
            Readiness::Unsupported { .. } => "Not supported",
        }
    }
}

/// 層 × 誰の操作 ごとの能力（§5.1 capabilities）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub layer: Layer,
    pub actor: Actor,
    pub readiness: Readiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Accessibility,
    ScreenCapture,
    Microphone,
    ProviderOauth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    NotRequested,
    Granted,
    Denied,
}

/// 要る権限と、その目的（§5.2 permission_required・§8.1 用途別）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Permission {
    pub kind: PermissionKind,
    pub purpose: String,
    pub state: PermissionState,
}

/// 窓の位置と大きさ（points・画面の左上が原点。AX と CGWindowList と egui の viewport が同じ座標）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    /// layout_tracking_error_px: 4 つの差の最大（points）
    pub fn error_px(&self, o: &Frame) -> f64 {
        [(self.x - o.x).abs(), (self.y - o.y).abs(), (self.w - o.w).abs(), (self.h - o.h).abs()].into_iter().fold(0.0, f64::max)
    }
    pub fn from_json(v: &Value) -> Option<Frame> {
        let f = Frame { x: v["x"].as_f64()?, y: v["y"].as_f64()?, w: v["w"].as_f64()?, h: v["h"].as_f64()? };
        f.valid().then_some(f)
    }
    pub fn json(&self) -> Value {
        json!({ "x": self.x, "y": self.y, "w": self.w, "h": self.h })
    }
    pub fn valid(&self) -> bool {
        [self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite()) && self.w >= 1.0 && self.h >= 1.0
    }
    pub fn text(&self) -> String {
        format!("{:.0},{:.0} {:.0}×{:.0}", self.x, self.y, self.w, self.h)
    }
}

/// Kangae の画面の中で元の窓を置く予約の矩形（§3.2-2）。in_host は Kangae の窓の中身の左上からの points。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalSlot {
    pub id: String,
    pub in_host: Frame,
}

impl PortalSlot {
    /// 画面座標: Kangae の窓の中身の左上（画面座標）+ in_host
    pub fn on_screen(&self, host_x: f64, host_y: f64) -> Frame {
        Frame { x: host_x + self.in_host.x, y: host_y + self.in_host.y, w: self.in_host.w, h: self.in_host.h }
    }
}

/// 元の窓の識別子と、再同定の手がかり（§5.1 source_window）。handle は bridge が生きている間だけ有効。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WindowInfo {
    pub handle: String,
    pub pid: u32,
    pub title: String,
    pub subrole: String,
    pub minimized: bool,
    pub main: bool,
    pub fullscreen: bool,
    /// 位置と大きさを AX で変えられる
    pub movable: bool,
    pub frame: Option<Frame>,
    /// CG の窓番号（同じ pid で frame が一致する窓が 1 つだけの時だけ。非公開 API で引かない）
    pub cg_id: Option<i64>,
    pub on_screen: Option<bool>,
}

impl WindowInfo {
    pub fn from_json(v: &Value) -> Option<WindowInfo> {
        Some(WindowInfo {
            handle: v["handle"].as_str()?.to_string(),
            pid: v["pid"].as_u64()? as u32,
            title: v["title"].as_str().unwrap_or("").to_string(),
            subrole: v["subrole"].as_str().unwrap_or("").to_string(),
            minimized: v["minimized"].as_bool().unwrap_or(false),
            main: v["main"].as_bool().unwrap_or(false),
            fullscreen: v["fullscreen"].as_bool().unwrap_or(false),
            movable: v["position_settable"].as_bool().unwrap_or(false) && v["size_settable"].as_bool().unwrap_or(false),
            frame: Frame::from_json(&v["frame"]),
            cg_id: v["cg_id"].as_i64(),
            on_screen: v["on_screen"].as_bool(),
        })
    }
    /// 普通の文書の窓（dialog・浮いている palette ではない）
    pub fn standard(&self) -> bool {
        self.subrole == "AXStandardWindow"
    }
    pub fn title_text(&self) -> &str {
        if self.title.trim().is_empty() {
            "（題名なし）"
        } else {
            &self.title
        }
    }
}

/// bridge の `windows` の result から窓の一覧。
pub fn parse_windows(result: &Value) -> Vec<WindowInfo> {
    result["windows"].as_array().map(|a| a.iter().filter_map(WindowInfo::from_json).collect()).unwrap_or_default()
}

/// 再接続の判定（PORTAL-11）。
#[derive(Debug, Clone, PartialEq)]
pub enum Reconnect {
    /// 同じ app・同じ題名の普通の窓が 1 つだけ。これに繋いでよい
    One(WindowInfo),
    /// 候補はあるが決められない（人に選ばせる。先頭の窓に繋がない）
    Ask(String),
    /// まだ候補が無い
    None(String),
}

/// app を再起動した後に繋ぎ直す窓を選ぶ。bundle id が違えば繋がない。題名が完全に一致する普通の窓が 1 つだけの時だけ One。
pub fn pick_reconnect(saved_bundle: &str, saved_title: &str, app_bundle: &str, candidates: &[WindowInfo]) -> Reconnect {
    if saved_bundle != app_bundle {
        return Reconnect::None(format!("It's a different app ({app_bundle}), so Shibubu won't connect"));
    }
    if saved_title.trim().is_empty() {
        return Reconnect::Ask("The previous window had no title, so Shibubu can't tell if it's the same one. Choose the window to cover again".into());
    }
    let same: Vec<&WindowInfo> = candidates.iter().filter(|w| w.standard() && !w.minimized && w.title == saved_title).collect();
    match same.as_slice() {
        [one] => Reconnect::One((*one).clone()),
        [] if candidates.iter().any(|w| w.standard()) => Reconnect::Ask(format!(
            "No window titled “{saved_title}” ({} other windows). Open the same project, or choose the window to cover again",
            candidates.iter().filter(|w| w.standard()).count()
        )),
        [] => Reconnect::None(format!("No window titled “{saved_title}” yet")),
        many => Reconnect::Ask(format!("{} windows are titled “{saved_title}” and Shibubu can't tell which. Choose the window to cover again", many.len())),
    }
}

// ---- 枠の 9 状態（v0.14 §7.2）と Lifecycle からの写し（W2c）----

/// §7.2 の 9 状態。枠の描き方（語・glyph・次の一手）は `ui::portal_frame` が同じ型に足す（ui はこの型を再輸出する）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalState {
    /// 未発見
    Undiscovered,
    /// 許可待ち
    PermissionRequired,
    /// 接続中
    Attaching,
    /// Native Direct 中（元の窓を slot に重ね、元の app が入力を直接受ける）
    NativeDirect,
    /// Mirror 中（映像 + 入力の転送）
    Mirror,
    /// 閲覧専用（映像だけ）
    ViewOnly,
    /// ソース終了
    SourceUnavailable,
    /// 同期遅延
    SyncDelayed,
    /// 一体化失敗
    IntegrationFailed,
}

impl PortalState {
    pub const ALL: [PortalState; 9] = [
        PortalState::Undiscovered,
        PortalState::PermissionRequired,
        PortalState::Attaching,
        PortalState::NativeDirect,
        PortalState::Mirror,
        PortalState::ViewOnly,
        PortalState::SourceUnavailable,
        PortalState::SyncDelayed,
        PortalState::IntegrationFailed,
    ];
}

// ---- Mirror の映像の事実（W2c の名前・W16M-1 で欄を足した）----

/// Mirror の映像 1 frame の事実（pixel は持たない。pixel は `mirror::MirrorFrame::rgba`）。helper の tag 1 の header JSON と同じ欄
/// （`seq`・`w` → width・`h` → height・`captured_ms`・`content`・`factor`）。
/// 例: `if let Some(f) = mirror.stream()? { assert_eq!(f.rgba.len(), f.meta.width as usize * f.meta.height as usize * 4) }`。保証する: 値は helper が書いた header のまま。保証しない: 今の元の窓の大きさ（frame は過去の 1 枚）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MirrorFrameMeta {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// 撮った時刻（SCStreamFrameInfo.displayTime を unix ms に直した物）
    pub captured_ms: f64,
    /// SCStreamFrameInfo.contentRect（frame の point の座標・x, y, w, h）。0 の時は frame 全体が中身
    pub content: [f64; 4],
    /// SCStreamFrameInfo.scaleFactor（frame の pixel / point・Retina = 2）
    pub factor: f64,
}

/// W2c の契約の `MirrorSurface{stream, relay_human}` は W16M-0 の `mirror::MirrorSurface`（YC_WAVES §5.4。W2c で作り直さない）。
/// 人の入力の型は `mirror::MirrorInput`（W2c の `HumanInput` は W16M-1 で消した: helper の 1 行と同じ形の W16M-0 の型が先に在る）。
#[cfg(feature = "mirror")]
#[allow(unused_imports)]
pub use mirror::{MirrorFrame, MirrorInput, MirrorSurface};

// ---- app ごとの同意（W16M-0 の「映す」・W19-1 の「AI が操作」で同じ形。YC_DECISIONS 11 通目: 口は別・既定は無し）----

/// 同意の記録の版。
pub const CONSENT_SCHEMA_V: u32 = 1;

/// 同意の記録 1 つ（Mirror は `ui_state` の json・AI の操作は `ACT_CONSENT_FILE` の 1 行）。人が同意の button を押した時だけ作る。
/// W19-1 で `mirror.rs` から移した（feature `mirror` の外の AI の操作も同じ型を使う。`mirror::ConsentRecord` は再び出す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRecord {
    pub v: u32,
    pub bundle_id: String,
    pub granted_at_ms: f64,
}

/// AI の操作の同意の file を指す環境変数（試験の側が run ごとに置き場を決める・`gui::ACT_LOG_ENV` と同じ扱い）。
/// Wave: W19-1
pub const ACT_CONSENT_ENV: &str = "SHIBUBU_GUI_ACT_CONSENTS";
/// data dir の下の AI の操作の同意の file（追記だけの JSONL・1 行 1 `ConsentRecord`）。Mirror の同意（`ui_state`）と別の口。
/// `ui_state` に置かないのは、sessions → apps の FK で Shibubu の app に縛られる（その app を消すと同意も消える）ため。AI の同意は宛先の app ごと。
/// Wave: W19-1
pub const ACT_CONSENT_FILE: &str = "gui-act-consents.jsonl";

/// FX1（FX-002）: Shibubu の data dir を決める 1 つの関数。`SHIBUBU_DATA_DIR` → `KANGAE_DATA_DIR`（release の launcher が入れる）→
/// `$HOME/Library/Application Support/Shibubu`（launcher と同じ場所 = Finder から開いた .app と `cargo run` が同じ dir）。
/// この file に置くのは、bin 4 本・tests の 9 本・audio・intel が `#[path]` で取り込む根で、`crate::` を使わずに全員が呼べるから。
/// 呼ぶ所: `app.rs` の `ShibubuApp::new`・`act_consent_path`・`mirror::relay_log_path`・`adapters::mac_gui` の `act_log_path`・kangae-portal の証拠の dir
/// （各 `*_ENV` の上書きは残す）。`media::youtube` の `count_file` は同じ順を写す（media は surface を持たない crate〈tests/media.rs〉でも取り込まれる）。
/// 例: `let dir = surface::data_dir(); let consents = dir.join(ACT_CONSENT_FILE);`
/// 保証する: 返す path は絶対（相対の env は今の cwd で絶対に・`llm_cli::CliConfig::new` の直しが要らなくなる）・空の env は無いのと同じ・
/// 試験の build（`cfg!(test)`）は HOME の代わりに `std::env::temp_dir()` を渡す（利用者の本物の data に書かない）。
/// 保証しない: dir が在る・書ける事（作るのは書く側・書けない時は `ShibubuApp::save_now` の Err）・試験が起こす本物の bin（その試験は env で dir を渡す）。
/// Wave: FX1
pub fn data_dir() -> PathBuf {
    let home = if cfg!(test) { Some(std::env::temp_dir().into_os_string()) } else { std::env::var_os("HOME") };
    data_dir_from(std::env::var_os("SHIBUBU_DATA_DIR"), std::env::var_os("KANGAE_DATA_DIR"), home)
}

/// `data_dir` の中身（env は process 全体の物なので、試験はこちらを値で呼ぶ・`project::context::target_from` と同じ形）。
/// 引数は順に `SHIBUBU_DATA_DIR`・`KANGAE_DATA_DIR`・`HOME` の値。どれも無い・空なら今の cwd の `.shibubu-data`（HOME の無い process だけ・Finder の .app は HOME を持つので通らない）。
/// 例: `assert_eq!(data_dir_from(None, None, Some("/h".into())), PathBuf::from("/h/Library/Application Support/Shibubu"))`
/// Wave: FX1
pub fn data_dir_from(
    shibubu: Option<std::ffi::OsString>,
    kangae: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    let set = |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = set(shibubu)
        .or_else(|| set(kangae))
        .or_else(|| set(home).map(|h| h.join("Library/Application Support/Shibubu")))
        .unwrap_or_else(|| PathBuf::from(".shibubu-data"));
    std::path::absolute(&dir).unwrap_or(dir)
}

/// FX1（FX-002 の残り）: FX1 の前の既定（今の cwd の `.shibubu-data`・`.kangae-data`）に保存が在り、新しい data dir（`data_dir()`）に無ければ、
/// 旧 dir の中身を新 dir へ写してから新 dir を返す（写してから読む）。写せなければ旧 dir を返す（その起動は旧 dir を読み書きする・次の起動でもう 1 回試す）。
/// 呼ぶのは `ShibubuApp::new` だけ・env（`SHIBUBU_DATA_DIR` / `KANGAE_DATA_DIR`）で dir を決めた起動では呼ばない（portal・relay・act の記録の bin も呼ばない）。
/// 写す物: 旧 dir の下の file 全部（sub dir も・SQLite の `-shm` は開く時に作り直されるので除く）。保存の印の 2 つ（`sessions.json`・`kangae.sqlite3`）は
/// 他を写した後に `<名前>.moving` へ写してから名前を替える（途中で止まっても新 dir は「保存が無い」のまま = 次も試す）。失敗したら新 dir の印を消す。
/// 例: `let dir = adopt_old_data(&data_dir(), &[cwd.join(".shibubu-data"), cwd.join(".kangae-data")]);`
/// 保証する: 旧 dir の file を消さない・書き換えない・新 dir に保存（どちらかの印）が在れば何もしない（混ぜない）・写す物が無ければ新 dir を作らない。
/// 保証しない: 旧 dir を同時に書いている別の Shibubu が居る時の写しの揃い（ponytail: file の写し。要るなら SQLite の `VACUUM INTO`）・旧 dir を消す事（利用者が消す）。
/// Wave: FX1
pub fn adopt_old_data(new: &Path, old: &[PathBuf]) -> PathBuf {
    const MARKS: [&str; 2] = ["sessions.json", "kangae.sqlite3"];
    let saved = |d: &Path| MARKS.iter().any(|m| d.join(m).is_file());
    let Some(old) = old.iter().find(|d| saved(d)) else { return new.to_path_buf() };
    if saved(new) {
        return new.to_path_buf();
    }
    fn copy_tree(from: &Path, to: &Path, skip: &dyn Fn(&Path) -> bool) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if e.file_type()?.is_dir() {
                copy_tree(&src, &dst, skip)?;
            } else if !skip(&src) {
                std::fs::copy(&src, &dst)?;
            }
        }
        Ok(())
    }
    let top = |p: &Path, n: &str| p.parent() == Some(old.as_path()) && p.file_name().is_some_and(|f| f == n);
    let copied = copy_tree(old, new, &|p| top(p, "kangae.sqlite3-shm") || MARKS.iter().any(|m| top(p, m))).and_then(|()| {
        for m in MARKS.iter().filter(|m| old.join(m).is_file()) {
            let moving = new.join(format!("{m}.moving"));
            std::fs::copy(old.join(m), &moving)?;
            std::fs::rename(&moving, new.join(m))?;
        }
        Ok(())
    });
    match copied {
        Ok(()) => new.to_path_buf(),
        Err(e) => {
            for m in MARKS {
                let _ = std::fs::remove_file(new.join(m));
                let _ = std::fs::remove_file(new.join(format!("{m}.moving")));
            }
            eprintln!("shibubu: couldn't move your data from {} · {e} · using it where it is", old.display());
            old.clone()
        }
    }
}

/// AI の操作の同意の置き場: `ACT_CONSENT_ENV` → `data_dir()`（FX1）の `ACT_CONSENT_FILE`。
/// Wave: W19-1
pub fn act_consent_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ACT_CONSENT_ENV) {
        return PathBuf::from(p);
    }
    data_dir().join(ACT_CONSENT_FILE)
}

/// ENGINE-AUDIT W7 本体 3（engine-audit 20「同意と Always allow の記録が平文で誰でも足せる」・operator 11-03 の決め 4）: 同意の file の行に付ける署名の鍵の
/// Keychain の key 名（service は `storage::secret_store::KEYCHAIN_SERVICE` = 機械全体で 1 つの名前の既に在る 1 か所・data dir ごとに分けない）。
/// `secret_store` の key の決まり（英数字と `. _ - : / @`）に合う字だけ。
/// ponytail: 鍵は `/usr/bin/security` で置く = 同じ user の process は `security` で読める（行を足すだけの agent を止める柵・境界ではない）。
/// 境界にするなら署名した .app の SecItem（ACL をその app だけに）。
pub const CONSENT_KEY_NAME: &str = "shibubu.consent-key";

/// W7 本体 3: 署名の鍵（32 byte・Keychain には hex）。process の中で Keychain に聞くのは鍵が見つかるまで（見つかった鍵・作った鍵は覚える）。
/// Ok(None) = まだ無い（初めて同意する前・`cfg!(test)` = 本物の Keychain を読まない）→ 読む側は平文の行を数える（今まで通り）。
/// Err = 読めない（`security` の失敗・10 秒の時間切れ・初回の macOS の Allow を待つ間）→ 読む側は 1 行も数えない（閉じる側・覚えない = 次にもう 1 回聞く）。
/// 本体 3 の取り込み方: この file は `crate::storage` も `crate::crypto` も持たない bin・tests の根（`#[path]`）なので、ui-1101 の d0ec6ed5（TERM-XTERM の直し =
/// `#[path = "../crypto.rs"] mod crypto;`）と同じ型で `storage/secret_store.rs`（std だけ）を中に取り込み、HMAC は `crypto::hmac_sha256`（`crate::crypto::` は使わない）。
fn consent_key() -> Result<Option<Vec<u8>>, String> {
    todo!("ENGINE-AUDIT W7 本体 3")
}

/// W7 本体 3: 書く側（`grant_act_consent`・`always_allow`）だけが呼ぶ。鍵が無ければ作って Keychain に置き、その時 `path` に在る読める行を 1 回だけ署名し直す
/// （TOFU = 鍵を作る前に人が押した同意は信じる・作った後に足された署名の無い行は数えない）。他の data dir の署名の無い行は数えなくなる（その app でもう 1 回同意を聞く）。
/// Err（鍵を作れない・置けない）は書く側の Err の 1 行（同意を書かない = 閉じる側）。`cfg!(test)` は Ok(None)（署名しない）。
fn consent_key_made(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let _ = path;
    todo!("ENGINE-AUDIT W7 本体 3")
}

/// W7 本体 3: 行（記録 1 つの JSON）に署名を付ける = 末尾の `}` の前に `,"mac":"<その JSON の HMAC-SHA256 の hex 64 字>"`。`key` が None なら付けない。
/// 新しい型・欄を作らない理由: 署名は file の行の性質で記録の欄ではない（`ConsentRecord` は Mirror の `ui_state` と同じ型・欄を足すと Mirror にも意味の無い欄が出る）。
/// 例: `sealed(r#"{"v":1}"#.into(), Some(&k))` → `{"v":1,"mac":"…"}`
fn sealed(json: String, key: Option<&[u8]>) -> String {
    let _ = (json, key);
    todo!("ENGINE-AUDIT W7 本体 3")
}

/// W7 本体 3: 同意の file の 1 行を、数えてよければ署名を外した JSON で返す（`sealed` の逆）。
/// `key` が Some = 署名が在ってその JSON の HMAC と合う行だけ Some（無い・合わない = 足された行 = None = 数えない）。None = どの行も Some（署名が在れば確かめずに外す =
/// 鍵を消した後も読める）。読む所: `act_consents`・`always_allowed`（`consent_key` の鍵で）・`drop_consents`（None で照合だけ外し、残す行は元の行のまま書き戻す =
/// 署名を落とさない・署名の合わない行も取り消しで外せる）。
/// 例: `text.lines().filter_map(|l| unsealed(l, key)).filter_map(|j| serde_json::from_str::<ConsentRecord>(&j).ok())`
fn unsealed(line: &str, key: Option<&[u8]>) -> Option<String> {
    let _ = (line, key);
    todo!("ENGINE-AUDIT W7 本体 3")
}

/// AI に任せてよい app の bundle id（file が無い = 0 件）。
/// 保証する: 読めない行・別の版の行は同意に数えない（閉じる側）。W7 本体 3 から: 鍵が在れば署名の合う行だけ（`unsealed`）・鍵を読めなければ 0 件。
/// 保証しない: その app が今動いている事。
/// Wave: W19-1
pub fn act_consents(path: &Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<ConsentRecord>(l).ok())
        .filter(|r| r.v == CONSENT_SCHEMA_V && !r.bundle_id.is_empty())
        .map(|r| r.bundle_id)
        .collect()
}

/// 人が「AI に任せる」を押した時だけ呼ぶ。置き場の dir が無ければ作り、1 行を追記する。
/// 例: `grant_act_consent(&act_consent_path(), "com.apple.TextEdit", now_ms())?`
/// Wave: W19-1
pub fn grant_act_consent(path: &Path, bundle_id: &str, now_ms: f64) -> Result<ConsentRecord, String> {
    if bundle_id.trim().is_empty() {
        return Err("Can't tell which app this window belongs to".into());
    }
    let r = ConsentRecord { v: CONSENT_SCHEMA_V, bundle_id: bundle_id.to_string(), granted_at_ms: now_ms };
    append_consent(path, &r)?;
    Ok(r)
}

/// 同意の file に 1 行足す（`ConsentRecord` と `AlwaysAllowed`・置き場の dir が無ければ作る）
fn append_consent(path: &Path, row: &impl Serialize) -> Result<(), String> {
    let line = serde_json::to_string(row).map_err(|e| e.to_string())?;
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("Can't create the folder for your consent, {} ({e})", d.display()))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}").and_then(|_| f.flush()))
        .map_err(|e| format!("Can't write your consent to {} ({e})", path.display()))
}

/// 同意の file から `drop` に当たる行を外す（読めない行は残す・file が無ければ何もしない）。外した行が在れば true
fn drop_consents(path: &Path, drop: impl Fn(&str) -> bool) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(false) };
    let keep: Vec<&str> = text.lines().filter(|l| !drop(l)).collect();
    if keep.len() == text.lines().count() {
        return Ok(false);
    }
    let body = if keep.is_empty() { String::new() } else { format!("{}\n", keep.join("\n")) };
    std::fs::write(path, body).map_err(|e| format!("Can't write your consent to {} ({e})", path.display()))?;
    Ok(true)
}

/// RQ6（RQ-022・Settings › Privacy の `Revoke`）: AI の操作の同意を 1 つ取り消す（その app の行を file から外す）。取り消した行が在れば true。
/// 次の run から `act_consents` に入らないので、AI の操作はその app で拒まれる。読めない行はそのまま残す。
/// OPEN-INPLACE-1101: その app の `AlwaysAllowed` の行も外す（`revoke_always` の doc）。
/// 保証する: 他の app の行を変えない・file が無ければ何も作らない。
/// Wave: RQ6
pub fn revoke_act_consent(path: &Path, bundle_id: &str) -> Result<bool, String> {
    drop_consents(path, |l| {
        serde_json::from_str::<ConsentRecord>(l).is_ok_and(|r| r.bundle_id == bundle_id) || serde_json::from_str::<AlwaysAllowed>(l).is_ok_and(|a| a.bundle_id == bundle_id)
    })
}

/// OPEN-INPLACE-1101 の 2（owner 240「Need approve には always allow を聞くように」）: 「Always allow」で決めた 1 つ = この app（`bundle_id`）の
/// この種類の手（`action` = registry の action の id・例 `desktop.ax.click`）は承認の card を出さずに打つ（読み戻しは省かない = engine P0）。
/// 置き場は AI の操作の同意と同じ file（`ACT_CONSENT_FILE`・追記だけの JSONL・新しい store を作らない）の 1 行。`app_name` は Connections の一覧の字。
/// 新しい型の理由: `ConsentRecord` は Mirror の同意（`ui_state`）と同じ型で app ごとの 1 行だけを持ち、手の種類を持てない（欄を足すと Mirror の記録にも
/// 意味の無い欄が出る）。`act_consents` は欄の違うこの行を同意に数えない（同意は Always allow の時に `grants` が別に書く）。
/// 例: `{"v":1,"bundle_id":"com.example.app","app_name":"Example","action":"desktop.ax.click","granted_at_ms":1.0}`
/// Wave: OPEN-INPLACE-1101
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlwaysAllowed {
    pub v: u32,
    pub bundle_id: String,
    pub app_name: String,
    pub action: String,
    pub granted_at_ms: f64,
}

impl AlwaysAllowed {
    /// Connections の Screen control の行の一覧の字（`{app} · {手の種類}`・種類は action の id の最後の語 = click → Click・type_text → Typing・
    /// menu → Menu・key → Keys・他はその語の頭を大文字に）。app の名前で枝を作らない。
    /// 例: `AlwaysAllowed { app_name: "Example".into(), action: "desktop.ax.click".into(), .. }.label()` → `"Example · Click"`
    pub fn label(&self) -> String {
        let kind = match self.action.rsplit('.').next().unwrap_or_default() {
            "click" => "Click".to_owned(),
            "type_text" => "Typing".into(),
            "menu" => "Menu".into(),
            "key" => "Keys".into(),
            k => k.chars().take(1).flat_map(char::to_uppercase).chain(k.chars().skip(1)).collect::<String>().replace('_', " "),
        };
        format!("{} · {kind}", self.app_name)
    }
}

/// OPEN-INPLACE-1101 の 2: 「Always allow」を押した時だけ呼ぶ（`CoreEngine::grant` が bundle を決めてから）。同じ app と手の行が在れば書かない。
/// 例: `always_allow(&act_consent_path(), "com.example.app", "Example", "desktop.ax.click", now_ms())?`
/// Wave: OPEN-INPLACE-1101
pub fn always_allow(path: &Path, bundle_id: &str, app_name: &str, action: &str, now_ms: f64) -> Result<AlwaysAllowed, String> {
    if bundle_id.trim().is_empty() || action.trim().is_empty() {
        return Err("Can't tell which app or which kind of step to always allow".into());
    }
    if let Some(have) = always_allowed(path).into_iter().find(|a| a.bundle_id == bundle_id && a.action == action) {
        return Ok(have);
    }
    let r = AlwaysAllowed { v: CONSENT_SCHEMA_V, bundle_id: bundle_id.to_owned(), app_name: app_name.trim().to_owned(), action: action.to_owned(), granted_at_ms: now_ms };
    append_consent(path, &r)?;
    Ok(r)
}

/// OPEN-INPLACE-1101 の 2: 今の「Always allow」の一覧（書いた順・同じ app と手は 1 つ・読めない行と別の版の行は数えない = 閉じる側）。
/// W7 本体 3 から: 同意と同じく鍵が在れば署名の合う行だけ（`unsealed`）。`grant_act_consent` と `always_allow` は鍵が在れば署名して書く（`consent_key_made`）。
/// 読む所: `Generated::poll_engine`（承認の card を出す前に、止まった手が全部この一覧に在るか）と Connections の Screen control の行。
/// Wave: OPEN-INPLACE-1101
pub fn always_allowed(path: &Path) -> Vec<AlwaysAllowed> {
    let mut out: Vec<AlwaysAllowed> = Vec::new();
    for a in std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<AlwaysAllowed>(l).ok()) {
        if a.v == CONSENT_SCHEMA_V && !a.bundle_id.is_empty() && !out.iter().any(|o| o.bundle_id == a.bundle_id && o.action == a.action) {
            out.push(a);
        }
    }
    out
}

/// OPEN-INPLACE-1101 の 2（Settings › Connections の Screen control の行の Remove）: その app のその手の行を file から外す。外した行が在れば true。
/// 保証する: 同意の行（`ConsentRecord`）と他の app・他の手の行を変えない・file が無ければ何も作らない。`revoke_act_consent`（app の同意を消す）は
/// その app の Always allow の行も外す（同意の無い app に承認を聞かずに打つ決まりを残さない）。
/// Wave: OPEN-INPLACE-1101
pub fn revoke_always(path: &Path, bundle_id: &str, action: &str) -> Result<bool, String> {
    drop_consents(path, |l| serde_json::from_str::<AlwaysAllowed>(l).is_ok_and(|a| a.bundle_id == bundle_id && a.action == action))
}

// ---- Control Lease（§5.3・PORTAL-09）----

/// Agent が元の app に対してしようとする事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAction {
    Input,
    Focus,
    MoveWindow,
}

/// 手動の Surface では Agent が入力・focus・窓の移動を奪わない。人はいつでも取り戻せる。
#[derive(Debug, Clone, PartialEq)]
pub struct ControlLease {
    pub holder: FocusOwner,
    pub since_ms: f64,
}

impl ControlLease {
    /// attach した時: 人が持つ（human_direct）
    pub fn human(now_ms: f64) -> ControlLease {
        ControlLease { holder: FocusOwner::Human, since_ms: now_ms }
    }
    /// 人が操作した（元の app が前に来た・Kangae で操作した）。いつでも通る
    pub fn human_input(&mut self, now_ms: f64) {
        if self.holder != FocusOwner::Human {
            self.holder = FocusOwner::Human;
            self.since_ms = now_ms;
        }
    }
    /// 人が「任せる」を押した時だけ Agent に渡る
    pub fn hand_to_agent(&mut self, now_ms: f64) {
        self.holder = FocusOwner::Agent;
        self.since_ms = now_ms;
    }
    pub fn human_has_control(&self) -> bool {
        self.holder == FocusOwner::Human
    }
    /// Agent の要求を通してよいか。人が持っている間は Err（何もしない）
    pub fn agent_may(&self, action: AgentAction) -> Result<(), String> {
        if self.human_has_control() {
            let what = match action {
                AgentAction::Input => "type",
                AgentAction::Focus => "move focus",
                AgentAction::MoveWindow => "move the window",
            };
            return Err(format!("You are working, so the agent won't {what}. Press Hand over to let it"));
        }
        Ok(())
    }
    /// backend を切り替えてよいか: 人が mouse の button を押している間（drag の途中）は Err
    pub fn may_switch_backend(&self, mouse_buttons: u64) -> Result<(), String> {
        if mouse_buttons != 0 {
            return Err("Not switching while you drag. Release the mouse, then press again".into());
        }
        Ok(())
    }
}

// ---- bridge の client（子 process・1 行 1 JSON）----

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeError {
    /// 実行 file が無い
    NotFound(String),
    Spawn(String),
    Timeout { cmd: String, ms: u64 },
    /// 壊れた JSON・形の違う応答
    Broken(String),
    /// 子 process が終わった（stderr の最後）
    Gone(String),
    /// bridge が明示の error を返した
    Failed { code: String, message: String },
}

impl BridgeError {
    pub fn code(&self) -> &str {
        match self {
            BridgeError::NotFound(_) => "bridge_not_found",
            BridgeError::Spawn(_) => "bridge_spawn",
            BridgeError::Timeout { .. } => "timeout",
            BridgeError::Broken(_) => "broken",
            BridgeError::Gone(_) => "bridge_gone",
            BridgeError::Failed { code, .. } => code,
        }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::NotFound(s) => write!(f, "{s}"),
            BridgeError::Spawn(s) => write!(f, "Couldn't start the bridge: {s}"),
            BridgeError::Timeout { cmd, ms } => write!(f, "The bridge didn't answer within {:.1} s ({cmd})", *ms as f64 / 1000.0),
            BridgeError::Broken(s) => write!(f, "Can't read the bridge's answer: {s}"),
            BridgeError::Gone(s) => write!(f, "The bridge stopped{}", if s.is_empty() { String::new() } else { format!(": {s}") }),
            BridgeError::Failed { message, .. } => write!(f, "{message}"),
        }
    }
}

/// bridge の 1 行の意味。
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Reply { id: u64, result: Result<Value, BridgeError> },
    Event(Value),
    Broken(String),
    Gone(String),
}

fn cut(s: &str) -> String {
    s.chars().take(300).collect()
}

/// stdout の 1 行を読む（純関数）。
pub fn parse_line(line: &str) -> Msg {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Msg::Broken(cut(line)) };
    if v.get("event").and_then(Value::as_str).is_some() {
        return Msg::Event(v);
    }
    let failed = || BridgeError::Failed {
        code: v["error"]["code"].as_str().unwrap_or("unknown").to_string(),
        message: v["error"]["message"].as_str().unwrap_or("Failed with no reason given").to_string(),
    };
    match (v.get("id").and_then(Value::as_u64), v.get("ok").and_then(Value::as_bool)) {
        (Some(id), Some(true)) => Msg::Reply { id, result: Ok(v.get("result").cloned().unwrap_or(Value::Null)) },
        (Some(id), Some(false)) => Msg::Reply { id, result: Err(failed()) },
        (None, Some(false)) => Msg::Broken(format!("The bridge couldn't read the request: {}", failed())),
        _ => Msg::Broken(cut(line)),
    }
}

/// bridge の実行 file を探す: `KANGAE_SURFACE_BRIDGE` → 自分の実行 file の隣（.app の Contents/MacOS）→ ../Helpers →
/// ../bridge（target/debug → target/bridge）→ ../../bridge（target/<triple>/debug）。無ければ build の仕方つきの Err。
pub fn find_bridge() -> Result<PathBuf, BridgeError> {
    if let Some(p) = std::env::var_os("KANGAE_SURFACE_BRIDGE") {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(BridgeError::NotFound(format!("The file {} named by KANGAE_SURFACE_BRIDGE doesn't exist", p.display()))) };
    }
    let exe = std::env::current_exe().map_err(|e| BridgeError::NotFound(format!("Can't read its own location: {e}")))?;
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let candidates = [dir.join(BRIDGE_NAME), dir.join("../Helpers").join(BRIDGE_NAME), dir.join("../bridge").join(BRIDGE_NAME), dir.join("../../bridge").join(BRIDGE_NAME)];
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        BridgeError::NotFound(format!(
            "Can't find {BRIDGE_NAME} (looked next to {}, in ../Helpers and in ../bridge). Build it with swiftc -O -swift-version 5 bridge/macos-surface/KangaeSurfaceBridge.swift -o target/bridge/{BRIDGE_NAME}, or set the location in KANGAE_SURFACE_BRIDGE",
            dir.display()
        ))
    })
}

enum Raw {
    Line(String),
    Closed,
}

/// 子 process の bridge。`send` と `poll` は止まらない（UI の 1 frame から呼ぶ）。`call` は応答を待つ（人の操作で 1 回の物だけ）。
pub struct Bridge {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Raw>,
    backlog: VecDeque<Msg>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
    closed: bool,
    pub path: PathBuf,
    /// 起動時の ready event（version・pid・trusted）
    pub hello: Value,
    /// MIRROR-QUIET-1101: app の出来事（`"event":"app_…"`）が着いた時に UI を起こす口（`wake_with`）。無い間は UI が次の frame で `poll` するのを待つ
    wake: Arc<Mutex<Option<std::task::Waker>>>,
}

/// 起動直後の bridge の ready を待つ既定の上限（混んだ機械で swift の binary が立つまで）。`Bridge::spawn` を呼ぶ側はこれを渡す。
pub const BRIDGE_READY_LIMIT: Duration = Duration::from_secs(20);

impl Bridge {
    /// 起こして ready event を待つ（ready_limit まで）。
    pub fn spawn(path: &Path, ready_limit: Duration) -> Result<Bridge, BridgeError> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BridgeError::Spawn(format!("{}: {e}", path.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| BridgeError::Spawn("Can't open stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| BridgeError::Spawn("Can't open stdout".into()))?;
        let mut err_pipe = child.stderr.take().ok_or_else(|| BridgeError::Spawn("Can't open stderr".into()))?;
        let (tx, rx) = mpsc::channel();
        let wake: Arc<Mutex<Option<std::task::Waker>>> = Arc::default();
        let woke = Arc::clone(&wake);
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Raw::Closed);
                        break;
                    }
                    Ok(_) => {
                        let l = line.trim_end().to_string();
                        // 起こすのは app の出来事だけ（frame ごとの要求の応答で UI と bridge が起こし合う loop を作らない・G97 の背景の描き直し）
                        let app = l.contains("\"event\":\"app_");
                        if tx.send(Raw::Line(l)).is_err() {
                            break;
                        }
                        if let Some(w) = app.then(|| woke.lock().ok().and_then(|g| g.clone())).flatten() {
                            w.wake();
                        }
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = err_pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = sink.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if s.len() > 4000 {
                        let keep = s.len() - 2000;
                        let at = (keep..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
                        s.drain(..at);
                    }
                }
            }
        });
        let mut b = Bridge { child, stdin, rx, backlog: VecDeque::new(), stderr, next_id: 1, closed: false, path: path.to_path_buf(), hello: Value::Null, wake };
        let deadline = Instant::now() + ready_limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(BridgeError::Timeout { cmd: "ready".into(), ms: ready_limit.as_millis() as u64 });
            }
            match b.rx.recv_timeout(left) {
                Ok(Raw::Line(l)) => match parse_line(&l) {
                    Msg::Event(v) if v["event"] == "ready" => {
                        b.hello = v;
                        return Ok(b);
                    }
                    Msg::Broken(s) => return Err(BridgeError::Broken(s)),
                    other => b.backlog.push_back(other),
                },
                Ok(Raw::Closed) | Err(RecvTimeoutError::Disconnected) => return Err(BridgeError::Gone(b.gone_reason())),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn gone_reason(&mut self) -> String {
        self.closed = true;
        // 終わり方と stderr の最後の行を拾うため、子が終わるのを少しだけ待つ（error の道だけ）
        let t = Instant::now();
        while matches!(self.child.try_wait(), Ok(None)) && t.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(50));
        let status = self.child.try_wait().ok().flatten().map(|s| format!("({s})")).unwrap_or_default();
        let err = self.stderr.lock().map(|s| s.trim().lines().last().unwrap_or("").to_string()).unwrap_or_default();
        format!("{status}{err}").trim().to_string()
    }

    pub fn alive(&self) -> bool {
        !self.closed
    }

    /// 要求 1 つを待つ上限: bridge の AX の timeout（ready の ax_timeout）+ 2 秒。CALL_LIMIT より短くしない。
    /// AX の読みが 1 回の要求の分（window_list は window_list_limit。windows・frame も窓ごとに読みが増えるので遅い app では超え得る）
    pub fn limit(&self) -> Duration {
        CALL_LIMIT.max(Duration::from_secs_f64(self.ax_timeout() + 2.0))
    }

    /// window_list を待つ上限: bridge の打ち切り（WINDOW_LIST_AX_READS × ax_timeout）+ 2 秒。CALL_LIMIT より短くしない
    pub fn window_list_limit(&self) -> Duration {
        CALL_LIMIT.max(Duration::from_secs_f64(WINDOW_LIST_AX_READS * self.ax_timeout() + 2.0))
    }

    fn ax_timeout(&self) -> f64 {
        self.hello["ax_timeout"].as_f64().filter(|t| t.is_finite() && *t > 0.0).unwrap_or(3.0)
    }

    /// 要求を送る（応答は poll で受け取る）。戻り値は id
    pub fn send(&mut self, cmd: &str, args: Value) -> Result<u64, BridgeError> {
        if self.closed {
            return Err(BridgeError::Gone(self.gone_reason()));
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut req = match args {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        req.insert("id".into(), json!(id));
        req.insert("cmd".into(), json!(cmd));
        let line = Value::Object(req).to_string();
        if writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush()).is_err() {
            return Err(BridgeError::Gone(self.gone_reason()));
        }
        Ok(id)
    }

    /// MIRROR-QUIET-1101: app の起動・前に来た・隠した等（`"event":"app_…"`）が着いたら `make` の Waker を起こす（背景で止まっている UI の描き直し）。
    /// 作るのは最初の 1 回だけ（毎 frame 呼んでよい）。応答と他の出来事では起こさない。
    /// 例: `b.wake_with(|| Waker::from(Arc::new(Repaint(ctx.clone()))))`
    pub fn wake_with(&self, make: impl FnOnce() -> std::task::Waker) {
        if let Ok(mut w) = self.wake.lock() {
            w.get_or_insert_with(make);
        }
    }

    /// 届いている応答と event を全部（止まらない）
    pub fn poll(&mut self) -> Vec<Msg> {
        let mut out: Vec<Msg> = self.backlog.drain(..).collect();
        loop {
            match self.rx.try_recv() {
                Ok(Raw::Line(l)) => out.push(parse_line(&l)),
                Ok(Raw::Closed) | Err(TryRecvError::Disconnected) => {
                    if !self.closed {
                        let why = self.gone_reason();
                        out.push(Msg::Gone(why));
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        out
    }

    /// 送って応答を待つ。待つ間に来た他の応答と event は次の poll に回す
    pub fn call(&mut self, cmd: &str, args: Value, limit: Duration) -> Result<Value, BridgeError> {
        let id = self.send(cmd, args)?;
        self.wait(id, cmd, limit)
    }

    /// `send` で送った `id`（`cmd`）の応答を `limit` まで待つ。待つ間に来た他の応答と event は次の poll に回す。
    /// Timeout でも要求は bridge に残り、応答は後の `poll` に `Msg::Reply` で来る（W16-2 の ax_text はそれを拾って、同じ面に 2 つ目を送らない）。
    pub fn wait(&mut self, id: u64, cmd: &str, limit: Duration) -> Result<Value, BridgeError> {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(BridgeError::Timeout { cmd: cmd.into(), ms: limit.as_millis() as u64 });
            }
            match self.rx.recv_timeout(left) {
                Ok(Raw::Line(l)) => match parse_line(&l) {
                    Msg::Reply { id: rid, result } if rid == id => return result,
                    other => self.backlog.push_back(other),
                },
                Ok(Raw::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    let why = self.gone_reason();
                    self.backlog.push_back(Msg::Gone(why.clone()));
                    return Err(BridgeError::Gone(why));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Kangae の窓の今（egui の viewport から）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Host {
    /// 枠の画面座標
    pub slot: Option<Frame>,
    pub minimized: bool,
    pub fullscreen: bool,
}

// ---- Kangae 自身の窓の前後（AppKit・公開 API。unsafe はこの 1 か所だけ）----

/// AppKit に触る口（公開 API だけ・unsafe はこの `appkit` 1 つに閉じる）。
#[cfg(target_os = "macos")]
pub enum AppKitOp {
    /// Kangae 自身の窓（key → main → 最初の窓）を CG の窓番号の窓の真下へ（`orderWindow:NSWindowBelow relativeTo:`・v0.14 P3）
    OrderHostBelow(u32),
    /// 同じ窓を CG の窓番号の窓の真上へ（`orderWindow:NSWindowAbove relativeTo:`・W16-1 で Page を離れた時に元の窓を隠す）
    OrderHostAbove(u32),
    /// L5-STICK ①・SINGLE-INSTANCE: pid の app を前へ（`NSRunningApplication runningApplicationWithProcessIdentifier:` → `activateWithOptions:` IgnoringOtherApps）。
    /// 他の process の窓には触らない（前に出すかは系と app が決める）。Err = その pid の app が無い・系が断った（NO）
    Activate(u32),
    /// 標準の Edit menu（Undo / Redo / Cut / Copy / Paste / Select All を first responder へ）を main menu に足す（子の WebView は Cmd+キーを自分で処理しない）
    InstallEditMenu,
    /// `view`（WKWebView）を親の view（AccessKit の subclass）の accessibilityChildren に足す（AX の木に WebView を出す）
    AxAddChild(*mut std::ffi::c_void),
    /// `view` が AX の木に足した WebView なら外す（WebSurface の Drop。Retry の後に孤児の WebView を見せない）
    AxDropChild(*mut std::ffi::c_void),
    /// first responder が `view` の中か（autofocus で WebView に移った時に egui の focus を外すため）
    FirstResponderWithin(*mut std::ffi::c_void),
    /// TERM-XTERM-1101: `view`（WKWebView）を窓の first responder にする（`makeFirstResponder:`・Show terminal / New terminal の後にそのまま打てる）。
    /// 頁の中の欄（xterm の textarea）への focus は頁の script（`term.focus()`）が持つ。Err = view か窓が無い。
    FocusWeb(*mut std::ffi::c_void),
    /// `view`（WKWebView）の窓の first responder を `view` の親（winit / egui の view）にする（`makeFirstResponder:`・egui の欄が focus を取った frame・`focus_fix`）
    FirstResponderToHost(*mut std::ffi::c_void),
    /// MIRROR-FIT-1101（owner 198 の 8「panel は被せる」）: `view`（WKWebView）を切り抜きの箱（`clipsToBounds` の NSView・identifier `shibubu-web-clip`）に入れ、
    /// 頁を縮めずに見えている所だけ見せる（下の panel が被さった・scroll で外に出た）。`shown` = 箱の矩形 [x, y, w, h]（egui の view の中・左上が原点の point。
    /// winit の view は flipped）・None = 箱の隠す / 見せるを揃えるだけ（箱が無ければ何もしない）。箱の隠す / 見せるは毎回 `view` の `isHidden` に揃える
    /// （隠した WebView の空の箱が egui の押しを取らない）。WebView は箱の中に wry の `set_bounds`（箱から見た左上が原点・wry が箱の高さで裏返す）で置く。
    /// 呼ぶ側の例（`WebSurface::frame`）: `appkit(AppKitOp::WebClip { view: wk, shown: Some([s.x, s.y, s.w, s.h]) })` の Ok の後に
    /// `view.set_bounds(Frame { x: slot.x - s.x, y: slot.y - s.y, w: slot.w, h: slot.h })`・Err は今まで通り `shown` に縮めて置く。
    /// 保証する: 箱は WebView ごとに 1 つ（2 度目からは作らない）・`AxAddChild` と `FirstResponderToHost` は箱の外の egui の view を親に使う。
    /// Err = view が無い・親の view が無い・親が flipped でない。
    WebClip { view: *mut std::ffi::c_void, shown: Option<[f64; 4]> },
    /// MIRROR-FIT-1101: `WebClip` の箱を窓から外す（`WebSurface` の Drop。wry の Drop は WebView を箱から外すだけで、空の箱が egui の押しを取り続ける）。箱に入っていなければ何もしない。
    WebUnclip(*mut std::ffi::c_void),
    /// MIRROR-FIT-1101 追記 210（owner 210 の 2「extraldrawが右上にあるとnotificationが隠れてしまう」）: egui が WebView の上に描く物（toast・menu・popup・palette・
    /// tooltip・承認の窓）の矩形 `holes`（egui の view の中・左上が原点の point = `shown` と同じ空間）を、`WebClip` の箱に穴として抜く。
    /// 見た目 = 箱の layer の mask（CAShapeLayer 1 つ・path = 箱の bounds ＋ 穴・even-odd。穴は箱の中に切り詰め、重なった穴は先に外接の矩形へまとめる）。
    /// 押し = 箱（class `ShibubuWebClipView`）の `hitTest:` が mask の path の外（= 穴）で nil を返す（click と scroll は下の egui の view が受ける）。
    /// `shown` = 箱の今の矩形（`WebClip` に渡した物）・`holes` が空なら mask を外す。
    /// 呼ぶ側の例（`WebSurface::frame`・穴が替わった frame だけ）: `appkit(AppKitOp::WebHoles { view: wk, shown: [s.x, s.y, s.w, s.h], holes })`・Err なら今まで通り隠す。
    /// 保証する: 箱を作り直さない・穴の組は箱ごと（mask の path が正本 = 別の WebView の穴と混ざらない）。
    /// Err = 箱が無い（縮めて置いた・まだ置いていない）・箱が穴を抜ける class でない（`WebClip` が `ShibubuWebClipView` を作れなかった）。
    /// 保証しない: 穴の影（egui の影は area の矩形の外 = WebView の下）・1 frame の遅れ（area の矩形は前の frame の物）・WKWebView の別 process の層に mask が効く事（実機で見る）。
    WebHoles { view: *mut std::ffi::c_void, shown: [f64; 4], holes: Vec<[f64; 4]> },
    /// RQ1（RQ-001）: macOS の app menu（main menu の先頭の submenu = `Shibubu`）の `About` と区切りの次に `Settings…`（key equivalent `,` = ⌘,）を足す。
    /// 押すと（menu か ⌘,）`take_settings_request` が次に 1 回 true を返す印を立て、渡した `wake`（例 `move || ctx.request_repaint()`）を 1 回呼ぶ
    /// （menu の action は egui の event を作らないので、起こさないと次の入力まで Settings へ移らない）。action の受け手は Rust の関数（押せるのに動かない項目にしない）。
    /// 呼ぶ側の例（app.rs の起動の 1 回・WebView の有無に依らない）: `let c = cc.egui_ctx.clone(); let _ = appkit(AppKitOp::InstallSettingsItem(Box::new(move || c.request_repaint())));`
    /// 保証する: 2 度呼んでも項目は 1 つ（既に在れば足さず Ok の 1 行）・⌘, の key は menu が受けるので egui の `keyboard()` には来ない。Err は記録の 1 行（menu が無い等・起動は止めない）。
    /// 保証しない: 窓が無い process（bin の kangae-portal 等）で効く事。
    /// Wave: RQ1
    InstallSettingsItem(Box<dyn Fn() + Send + Sync>),
    /// TITLEBAR-ICONS-1101 の 1（owner 216）: 窓の赤黄緑（`standardWindowButton:` の閉じる・しまう・拡大）を、透明にした題の帯（高さ `bar_h` = `theme::HEADER_H`）の
    /// 縦の中央へ・閉じるの左端を `x`（`theme::TRAFFIC_X`）へ寄せる（Electron の `trafficLightPosition` と同じ手: 赤黄緑の親の親 = NSTitlebarContainerView の
    /// 高さを `bar_h`・上端を窓の上端に・3 つの間は今の間〈しまう.x − 閉じる.x〉のまま）。座標は AppKit の view の中（原点は左下・winit の view と違い flipped でない）。
    /// AppKit は窓の大きさの変化と全画面の出入りで既定の位置に戻すので、呼ぶ側（app.rs の毎 frame・全画面の間は呼ばない）は毎 frame 打つ: 既に寄っていれば
    /// 何も書かず Ok（frame・値は KVC の `valueForKey:@"frame"` → NSValue の `getValue:size:` で読む = x86_64 の struct の戻り値〈objc_msgSend_stret〉を使わない）。
    /// 呼ぶ側の例: `let _ = appkit(AppKitOp::TrafficLights { x: f64::from(theme::TRAFFIC_X), bar_h: f64::from(theme::HEADER_H) });`
    /// Err = 窓が無い・赤黄緑が無い（題の帯の無い窓）→ 呼ぶ側は既定の位置のまま続ける（記録しない = 毎 frame の Err で stderr を埋めない）。
    /// 保証しない: 右から左の言語の並び（赤黄緑が右に来る）・macOS 15 より後で NSTitlebarContainerView の作りが替わる事。
    /// Wave: TITLEBAR-ICONS-1101
    TrafficLights { x: f64, bar_h: f64 },
    /// TITLEBAR-ICONS-1101 の 1（operator 24 回目の前）: 系の設定「題の帯を double-click した時」（Desktop & Dock・`AppleActionOnDoubleClick`）を読む。
    /// `NSUserDefaults.standardUserDefaults` の `stringForKey:`（global domain も入っている = 系の設定がそのまま読める）。Ok = 値の字（`Maximize`・`Fill`・`Minimize`・`None`・
    /// 設定していない = 空）。呼ぶ側の例（header の帯の double-click）: `header::double_click(&appkit(AppKitOp::DoubleClickAction).unwrap_or_default(), zoomed)`。
    /// 保証しない: 古い macOS の `AppleMiniaturizeOnDoubleClick`（bool）を見る事。
    /// Wave: TITLEBAR-ICONS-1101
    DoubleClickAction,
}

/// RQ1（RQ-001）: app menu の `Settings…` か ⌘, が押されたか（押されていれば印を下ろして true・1 回押せば 1 回だけ true）。macOS 以外は常に false。
/// 呼ぶ側の例（app.rs の毎 frame の頭）: `if surface::take_settings_request() { self.apply(ctx, Intent::Go(View::Settings(SettingsSection::General))); }`
/// 保証する: 印は main thread に 1 つ（窓は 1 つ・AppKit の menu の action と eframe の update は同じ main thread = winit の macOS の約束）・読むと下ろす（同じ押しで 2 度移らない）。
/// 保証しない: 押した順（2 回押しても 1 回の true にまとまる事がある）。
/// Wave: RQ1
pub fn take_settings_request() -> bool {
    SETTINGS_REQUEST.with(|r| r.replace(false))
}

/// RQ1: `Settings…` の印を立て、`InstallSettingsItem` で渡された wake を呼ぶ（menu の action の本体・試験は直に呼ぶ）。
pub(crate) fn request_settings() {
    SETTINGS_REQUEST.with(|r| r.set(true));
    if let Some(wake) = SETTINGS_WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        wake();
    }
}

thread_local! {
    // thread の印にした理由: 試験は thread ごとに別の app を回す（process の印だと隣の試験の frame が拾って Settings へ移る）。製品は main thread 1 本で同じ
    static SETTINGS_REQUEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static SETTINGS_WAKE: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
static AX_CHILD: std::sync::atomic::AtomicPtr<std::ffi::c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
#[cfg(target_os = "macos")]
static AX_ORIGINAL: std::sync::atomic::AtomicPtr<std::ffi::c_void> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// AppKit の手当てを 1 つ（戻り値は記録の 1 行 / FirstResponderWithin と FirstResponderToHost は "1" か "0"）。
#[cfg(target_os = "macos")]
pub fn appkit(op: AppKitOp) -> Result<String, String> {
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::atomic::Ordering;
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        fn object_getClass(obj: *mut c_void) -> *mut c_void;
        fn class_getInstanceMethod(cls: *mut c_void, sel: *mut c_void) -> *mut c_void;
        fn method_setImplementation(m: *mut c_void, imp: *mut c_void) -> *mut c_void;
        fn class_getName(cls: *mut c_void) -> *const c_char;
    }
    type Id = *mut c_void;
    // MIRROR-FIT-1101 追記 210: 穴を抜ける箱（`ShibubuWebClipView`）と mask の path
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_allocateClassPair(superclass: *mut c_void, name: *const c_char, extra: usize) -> *mut c_void;
        fn objc_registerClassPair(cls: *mut c_void);
        fn class_addMethod(cls: *mut c_void, sel: *mut c_void, imp: *mut c_void, types: *const c_char) -> i8;
        fn class_getMethodImplementation(cls: *mut c_void, sel: *mut c_void) -> *mut c_void;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPathCreateMutable() -> *mut c_void;
        fn CGPathAddRect(path: *mut c_void, m: *const c_void, rect: CgRect);
        fn CGPathRelease(path: *mut c_void);
        fn CGPathContainsPoint(path: *const c_void, m: *const c_void, point: CgPoint, eo_fill: bool) -> bool;
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
    }
    // 値で渡すだけ（Rust の側では読まない）
    #[repr(C)]
    #[allow(dead_code)]
    struct CgRect(f64, f64, f64, f64);
    // `ShibubuWebClipView` の hitTest:（point は親 = egui の view の座標）: 箱の layer の mask の path の外（= 穴）なら nil = 押しと scroll は下の egui の view へ。
    // それ以外は NSView の hitTest:（KVO の subclass でも回らないよう、自分の class の親ではなく NSView の物を呼ぶ）
    unsafe extern "C" fn web_clip_hit_test(this: Id, cmd: Id, p: CgPoint) -> Id {
        let msg = objc_msgSend as unsafe extern "C" fn();
        let send0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg);
        let send_bool1: unsafe extern "C" fn(Id, Id, Id) -> i8 = std::mem::transmute(msg);
        let send_point: unsafe extern "C" fn(Id, Id, CgPoint, Id) -> CgPoint = std::mem::transmute(msg);
        let sel = |s: &CStr| sel_registerName(s.as_ptr());
        let layer = send0(this, sel(c"layer"));
        let mask = if layer.is_null() { std::ptr::null_mut() } else { send0(layer, sel(c"mask")) };
        if !mask.is_null() && send_bool1(mask, sel(c"respondsToSelector:"), sel(c"path")) != 0 {
            let path = send0(mask, sel(c"path"));
            if !path.is_null() {
                let at = send_point(this, sel(c"convertPoint:fromView:"), p, send0(this, sel(c"superview")));
                if !CGPathContainsPoint(path, std::ptr::null(), at, true) {
                    return std::ptr::null_mut();
                }
            }
        }
        let base: unsafe extern "C" fn(Id, Id, CgPoint) -> Id = std::mem::transmute(class_getMethodImplementation(objc_getClass(c"NSView".as_ptr()), cmd));
        base(this, cmd, p)
    }
    // AccessKit の subclass の accessibilityChildren を包む: 元の答え（egui の木）の後ろに WebView を 1 つ足す
    unsafe extern "C" fn ax_children(this: Id, cmd: Id) -> Id {
        let original = AX_ORIGINAL.load(Ordering::SeqCst);
        let child = AX_CHILD.load(Ordering::SeqCst);
        let msg = objc_msgSend as unsafe extern "C" fn();
        let orig: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(original);
        let base = orig(this, cmd);
        if child.is_null() {
            return base;
        }
        let send1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg);
        let send_void1: unsafe extern "C" fn(Id, Id, Id) = std::mem::transmute(msg);
        let list = send1(objc_getClass(c"NSMutableArray".as_ptr()), sel_registerName(c"arrayWithArray:".as_ptr()), base);
        send_void1(list, sel_registerName(c"addObject:".as_ptr()), child);
        list
    }
    // 理由: crate を足さずに AppKit の公開 method を呼ぶため、objc_msgSend を引数の型ごとの C 関数として呼ぶ。
    // 前提: 引数が pointer と整数だけなので x86_64 / arm64 とも C の呼び出し規約（src/main.rs の AccessKit と同じ）・AppKit の main thread・
    // 例外は `WebClip` の setFrame: の NSRect（double 4 つを値で渡す: x86_64 は stack〈MEMORY の分類〉・arm64 は d0〜d3〈HFA〉= `#[repr(C)]` の struct と同じ・戻り値の struct は読まない）・
    // 触るのは自分の process の物だけ（他の process の窓には触らない・`Activate` は他の app に前へ来るよう頼むだけ）。NSWindowBelow = -1、relativeTo は CGWindowID と同じ windowNumber。
    unsafe {
        let msg = objc_msgSend as unsafe extern "C" fn();
        let send0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg);
        let send1: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(msg);
        let send_bool1: unsafe extern "C" fn(Id, Id, Id) -> i8 = std::mem::transmute(msg);
        let send_item: unsafe extern "C" fn(Id, Id, Id, Id, Id) -> Id = std::mem::transmute(msg);
        let sel = |s: &CStr| sel_registerName(s.as_ptr());
        let nsstring = |s: &CStr| send1(objc_getClass(c"NSString".as_ptr()), sel(c"stringWithUTF8String:"), s.as_ptr() as Id);
        let send_order: unsafe extern "C" fn(Id, Id, isize, isize) = std::mem::transmute(msg);
        let app = || {
            let class = objc_getClass(c"NSApplication".as_ptr());
            (!class.is_null()).then(|| send0(class, sel(c"sharedApplication")))
        };
        // MIRROR-FIT-1101: `WebClip` の箱（identifier で見分ける）と、箱の外の親（egui の view）。箱の中の WebView の superview は箱なので、
        // AX の木と first responder の親は 1 つ上（箱を親にすると AxAddChild が NSView の class ごと accessibilityChildren を差し替える・egui が key を受けなくなる）
        let send_bool0: unsafe extern "C" fn(Id, Id) -> i8 = std::mem::transmute(msg);
        let is_clip = |v: Id| {
            let name = if v.is_null() { std::ptr::null_mut() } else { send0(v, sel(c"identifier")) };
            !name.is_null() && send_bool1(name, sel(c"isEqualToString:"), nsstring(c"shibubu-web-clip")) != 0
        };
        let host_of = |v: Id| {
            let p = send0(v, sel(c"superview"));
            if is_clip(p) { send0(p, sel(c"superview")) } else { p }
        };
        match op {
            AppKitOp::Activate(pid) => {
                let class = objc_getClass(c"NSRunningApplication".as_ptr());
                if class.is_null() {
                    return Err("AppKit isn't loaded (a process with no windows)".into());
                }
                let send_pid: unsafe extern "C" fn(Id, Id, i32) -> Id = std::mem::transmute(msg);
                let running = send_pid(class, sel(c"runningApplicationWithProcessIdentifier:"), pid as i32);
                if running.is_null() {
                    return Err(format!("No app is running as process {pid}"));
                }
                // NSApplicationActivateIgnoringOtherApps = 1 << 1（macOS 14 から協調の activate・系が断ると NO）
                let send_opts: unsafe extern "C" fn(Id, Id, usize) -> i8 = std::mem::transmute(msg);
                if send_opts(running, sel(c"activateWithOptions:"), 1 << 1) == 0 {
                    return Err(format!("macOS didn't bring process {pid} to the front"));
                }
                Ok(format!("Asked process {pid} to come to the front"))
            }
            AppKitOp::OrderHostBelow(target) | AppKitOp::OrderHostAbove(target) => {
                // NSWindowBelow = -1・NSWindowAbove = 1
                let (place, said) = if matches!(op, AppKitOp::OrderHostAbove(_)) { (1, "Placed directly above") } else { (-1, "Placed directly below") };
                let app = app().ok_or("AppKit isn't loaded (a process with no windows)")?;
                let mut win = send0(app, sel(c"keyWindow"));
                if win.is_null() {
                    win = send0(app, sel(c"mainWindow"));
                }
                if win.is_null() {
                    win = send0(send0(app, sel(c"windows")), sel(c"firstObject"));
                }
                if win.is_null() {
                    return Err("The Shibubu window wasn't found".into());
                }
                send_order(win, sel(c"orderWindow:relativeTo:"), place, target as isize);
                Ok(said.into())
            }
            AppKitOp::InstallEditMenu => {
                let app = app().ok_or("AppKit isn't loaded (a process with no windows)")?;
                let main = send0(app, sel(c"mainMenu"));
                if main.is_null() {
                    return Err("The main menu wasn't found".into());
                }
                if !send1(main, sel(c"itemWithTitle:"), nsstring(c"Edit")).is_null() {
                    return Ok("The Edit menu already exists".into());
                }
                let menu = send1(send0(objc_getClass(c"NSMenu".as_ptr()), sel(c"alloc")), sel(c"initWithTitle:"), nsstring(c"Edit"));
                for (title, action, key) in [
                    (c"Undo", c"undo:", c"z"),
                    (c"Redo", c"redo:", c"Z"),
                    (c"Cut", c"cut:", c"x"),
                    (c"Copy", c"copy:", c"c"),
                    (c"Paste", c"paste:", c"v"),
                    (c"Select All", c"selectAll:", c"a"),
                ] {
                    let item = send_item(send0(objc_getClass(c"NSMenuItem".as_ptr()), sel(c"alloc")), sel(c"initWithTitle:action:keyEquivalent:"), nsstring(title), sel(action), nsstring(key));
                    send1(menu, sel(c"addItem:"), item);
                }
                let holder = send0(send0(objc_getClass(c"NSMenuItem".as_ptr()), sel(c"alloc")), sel(c"init"));
                send1(holder, sel(c"setTitle:"), nsstring(c"Edit"));
                send1(holder, sel(c"setSubmenu:"), menu);
                send1(main, sel(c"addItem:"), holder);
                Ok("Added the Edit menu".into())
            }
            AppKitOp::AxAddChild(view) => {
                if view.is_null() {
                    return Err("The web view wasn't found".into());
                }
                let parent = host_of(view);
                if parent.is_null() {
                    return Err("The web view has no parent view".into());
                }
                // AccessKit は最初の accessibilityChildren の問い合わせで winit の view を subclass にする（src/main.rs と同じ）。先に 1 回問う
                send0(parent, sel(c"accessibilityChildren"));
                let class = object_getClass(parent);
                let name = CStr::from_ptr(class_getName(class)).to_string_lossy().into_owned();
                AX_CHILD.store(view, Ordering::SeqCst);
                if AX_ORIGINAL.load(Ordering::SeqCst).is_null() {
                    let method = class_getInstanceMethod(class, sel(c"accessibilityChildren"));
                    if method.is_null() {
                        return Err(format!("{name} has no accessible children"));
                    }
                    let previous = method_setImplementation(method, ax_children as *mut c_void);
                    AX_ORIGINAL.store(previous, Ordering::SeqCst);
                }
                Ok(format!("Added the web view to {name}'s accessibilityChildren"))
            }
            AppKitOp::AxDropChild(view) => Ok(match AX_CHILD.compare_exchange(view, std::ptr::null_mut(), Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => "Removed the web view from the accessibility tree".into(),
                Err(_) => "The accessibility tree points to a different web view, so it is left alone".into(),
            }),
            AppKitOp::FirstResponderWithin(view) => {
                if view.is_null() {
                    return Ok("0".into());
                }
                let window = send0(view, sel(c"window"));
                if window.is_null() {
                    return Ok("0".into());
                }
                let responder = send0(window, sel(c"firstResponder"));
                if responder.is_null() || send_bool1(responder, sel(c"isKindOfClass:"), objc_getClass(c"NSView".as_ptr())) == 0 {
                    return Ok("0".into());
                }
                Ok(if send_bool1(responder, sel(c"isDescendantOf:"), view) != 0 { "1" } else { "0" }.into())
            }
            AppKitOp::FocusWeb(view) => {
                if view.is_null() {
                    return Err("The web view wasn't found".into());
                }
                let window = send0(view, sel(c"window"));
                if window.is_null() {
                    return Err("The web view has no window".into());
                }
                Ok(if send_bool1(window, sel(c"makeFirstResponder:"), view) != 0 { "1" } else { "0" }.into())
            }
            AppKitOp::FirstResponderToHost(view) => {
                if view.is_null() {
                    return Err("The web view wasn't found".into());
                }
                let (window, host) = (send0(view, sel(c"window")), host_of(view));
                if window.is_null() || host.is_null() {
                    return Err("The web view has no window or parent view".into());
                }
                Ok(if send_bool1(window, sel(c"makeFirstResponder:"), host) != 0 { "1" } else { "0" }.into())
            }
            AppKitOp::WebClip { view, shown } => {
                // 値で渡すだけ（Rust の側では読まない）
                #[repr(C)]
                #[allow(dead_code)]
                struct NsRect(f64, f64, f64, f64);
                let send_rect: unsafe extern "C" fn(Id, Id, NsRect) = std::mem::transmute(msg);
                let send_flag: unsafe extern "C" fn(Id, Id, i8) = std::mem::transmute(msg);
                let send_place: unsafe extern "C" fn(Id, Id, Id, isize, Id) = std::mem::transmute(msg);
                if view.is_null() {
                    return Err("The web view wasn't found".into());
                }
                let mut clip = send0(view, sel(c"superview"));
                if clip.is_null() {
                    return Err("The web view has no parent view".into());
                }
                let mut said = "Placed the web view's clipping view";
                if !is_clip(clip) {
                    if shown.is_none() {
                        return Ok("The web view isn't clipped yet".into());
                    }
                    // 箱の矩形は親の座標のまま渡す = 親（winit の view）は左上が原点（flipped）の時だけ
                    if send_bool0(clip, sel(c"isFlipped")) == 0 {
                        return Err("The window's view isn't flipped, so the web view is shrunk instead of clipped".into());
                    }
                    let host = clip;
                    // MIRROR-FIT-1101 追記 210: 箱は NSView の subclass（穴で hitTest: が nil）・作れなければ NSView のまま（`WebHoles` が Err = 今まで通り隠す）
                    let name = c"ShibubuWebClipView";
                    let mut class = objc_getClass(name.as_ptr());
                    if class.is_null() {
                        class = objc_allocateClassPair(objc_getClass(c"NSView".as_ptr()), name.as_ptr(), 0);
                        if !class.is_null() {
                            class_addMethod(class, sel(c"hitTest:"), web_clip_hit_test as *mut c_void, c"@@:{CGPoint=dd}".as_ptr());
                            objc_registerClassPair(class);
                        }
                    }
                    let class = if class.is_null() { objc_getClass(c"NSView".as_ptr()) } else { class };
                    clip = send0(send0(class, sel(c"alloc")), sel(c"init"));
                    if clip.is_null() {
                        return Err("Couldn't make the web view's clipping view".into());
                    }
                    send1(clip, sel(c"setIdentifier:"), nsstring(c"shibubu-web-clip"));
                    send_flag(clip, sel(c"setWantsLayer:"), 1);
                    let layer = send0(clip, sel(c"layer"));
                    if !layer.is_null() {
                        send_flag(layer, sel(c"setMasksToBounds:"), 1);
                    }
                    // macOS 14 から clipsToBounds の既定は NO（13 までは常に切る・この selector が無い）
                    if send_bool1(clip, sel(c"respondsToSelector:"), sel(c"setClipsToBounds:")) != 0 {
                        send_flag(clip, sel(c"setClipsToBounds:"), 1);
                    }
                    // WebView の今の前後の所（NSWindowAbove = 1）に箱を置いて WebView を移す（addSubview: は前の親から外す）・箱は親が持つ（alloc の 1 つを放す）
                    send_place(host, sel(c"addSubview:positioned:relativeTo:"), clip, 1, view);
                    send1(clip, sel(c"addSubview:"), view);
                    send0(clip, sel(c"release"));
                    said = "Put the web view in a clipping view";
                }
                if let Some([x, y, w, h]) = shown {
                    send_rect(clip, sel(c"setFrame:"), NsRect(x, y, w, h));
                }
                // 隠した WebView の空の箱は egui の押しを取る（NSView の hitTest は中身が無くても自分を返す）= 箱の隠す / 見せるは WebView に揃える
                send_flag(clip, sel(c"setHidden:"), send_bool0(view, sel(c"isHidden")));
                Ok(said.into())
            }
            AppKitOp::WebUnclip(view) => {
                let clip = if view.is_null() { std::ptr::null_mut() } else { send0(view, sel(c"superview")) };
                if !is_clip(clip) {
                    return Ok("The web view isn't clipped".into());
                }
                // WebView を先に箱から外す（wry の Drop の removeFromSuperview は親が無ければ何もしない）→ 箱を窓から外す（親が持つ 1 つが最後 = 箱は消える）
                send0(view, sel(c"removeFromSuperview"));
                send0(clip, sel(c"removeFromSuperview"));
                Ok("Removed the web view's clipping view".into())
            }
            AppKitOp::WebHoles { view, shown, holes } => {
                let send_rect: unsafe extern "C" fn(Id, Id, CgRect) = std::mem::transmute(msg);
                let send_point: unsafe extern "C" fn(Id, Id, CgPoint, Id) -> CgPoint = std::mem::transmute(msg);
                let clip = if view.is_null() { std::ptr::null_mut() } else { send0(view, sel(c"superview")) };
                if !is_clip(clip) {
                    return Err("The web view isn't clipped, so it can't have holes".into());
                }
                let class = objc_getClass(c"ShibubuWebClipView".as_ptr());
                if class.is_null() || send_bool1(clip, sel(c"isKindOfClass:"), class) == 0 {
                    return Err("The web view's clipping view can't pass clicks through".into());
                }
                let layer = send0(clip, sel(c"layer"));
                if layer.is_null() {
                    return Err("The web view's clipping view has no layer".into());
                }
                let cut = hole_rects(shown, &holes);
                if cut.is_empty() {
                    send1(layer, sel(c"setMask:"), std::ptr::null_mut());
                    return Ok("Removed the holes in the web view".into());
                }
                // 穴は親（egui の view・flipped）の座標 → 箱の座標（flipped でない = 左下が原点）へ AppKit に直させる（hitTest: と同じ変換）。
                // convertRect: は struct の戻り値が 32 byte（x86_64 は _stret）なので、16 byte の点を 2 つ
                let host = send0(clip, sel(c"superview"));
                let local = |x: f64, y: f64| send_point(clip, sel(c"convertPoint:fromView:"), CgPoint { x, y }, host);
                let path = CGPathCreateMutable();
                if path.is_null() {
                    return Err("Couldn't make the path for the web view's holes".into());
                }
                CGPathAddRect(path, std::ptr::null(), CgRect(0.0, 0.0, shown[2], shown[3]));
                for &[x, y, w, h] in &cut {
                    let (a, b) = (local(x, y), local(x + w, y + h));
                    CGPathAddRect(path, std::ptr::null(), CgRect(a.x.min(b.x), a.y.min(b.y), (b.x - a.x).abs(), (b.y - a.y).abs()));
                }
                let mask = send0(objc_getClass(c"CAShapeLayer".as_ptr()), sel(c"layer"));
                if mask.is_null() {
                    CGPathRelease(path);
                    return Err("Couldn't make the mask for the web view's holes".into());
                }
                send_rect(mask, sel(c"setFrame:"), CgRect(0.0, 0.0, shown[2], shown[3]));
                // CAShapeLayer は path を持つ（retain）→ 作った 1 つを放す
                send1(mask, sel(c"setPath:"), path);
                CGPathRelease(path);
                send1(mask, sel(c"setFillRule:"), nsstring(c"even-odd"));
                send1(layer, sel(c"setMask:"), mask);
                Ok(format!("Cut {} hole(s) in the web view", cut.len()))
            }
            AppKitOp::InstallSettingsItem(wake) => {
                #[link(name = "objc")]
                unsafe extern "C" {
                    fn objc_allocateClassPair(superclass: *mut c_void, name: *const c_char, extra: usize) -> *mut c_void;
                    fn objc_registerClassPair(cls: *mut c_void);
                    fn class_addMethod(cls: *mut c_void, sel: *mut c_void, imp: *mut c_void, types: *const c_char) -> i8;
                }
                unsafe extern "C" fn open_settings(_this: Id, _cmd: Id, _sender: Id) {
                    request_settings();
                }
                let send_n: unsafe extern "C" fn(Id, Id) -> isize = std::mem::transmute(msg);
                let send_at: unsafe extern "C" fn(Id, Id, isize) -> Id = std::mem::transmute(msg);
                let send_insert: unsafe extern "C" fn(Id, Id, Id, isize) = std::mem::transmute(msg);
                *SETTINGS_WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
                let app = app().ok_or("AppKit isn't loaded (a process with no windows)")?;
                let main = send0(app, sel(c"mainMenu"));
                // itemAtIndex: は範囲の外で例外を投げる → 数を先に見る
                if main.is_null() || send_n(main, sel(c"numberOfItems")) < 1 {
                    return Err("The app menu wasn't found".into());
                }
                let menu = send0(send_at(main, sel(c"itemAtIndex:"), 0), sel(c"submenu"));
                if menu.is_null() {
                    return Err("The app menu wasn't found".into());
                }
                if !send1(menu, sel(c"itemWithTitle:"), nsstring(c"Settings…")).is_null() {
                    return Ok("The Settings… item already exists".into());
                }
                let name = c"ShibubuSettingsTarget";
                let mut class = objc_getClass(name.as_ptr());
                if class.is_null() {
                    class = objc_allocateClassPair(objc_getClass(c"NSObject".as_ptr()), name.as_ptr(), 0);
                    if class.is_null() {
                        return Err("Couldn't make the Settings menu target".into());
                    }
                    class_addMethod(class, sel(c"openSettings:"), open_settings as *mut c_void, c"v@:@".as_ptr());
                    objc_registerClassPair(class);
                }
                // menu の target は弱い参照 → init の 1 つを放さない（process の間 1 つ・項目は 1 度しか足さない）
                let target = send0(send0(class, sel(c"alloc")), sel(c"init"));
                let item = send_item(send0(objc_getClass(c"NSMenuItem".as_ptr()), sel(c"alloc")), sel(c"initWithTitle:action:keyEquivalent:"), nsstring(c"Settings…"), sel(c"openSettings:"), nsstring(c","));
                send1(item, sel(c"setTarget:"), target);
                // winit の既定の並び（About · 区切り · Services …）の About と区切りの次。項目が 2 つより少なければ末尾
                let at = send_n(menu, sel(c"numberOfItems")).min(2);
                send_insert(menu, sel(c"insertItem:atIndex:"), item, at);
                send_insert(menu, sel(c"insertItem:atIndex:"), send0(objc_getClass(c"NSMenuItem".as_ptr()), sel(c"separatorItem")), at + 1);
                Ok("Added Settings… (⌘,) to the app menu".into())
            }
            AppKitOp::TrafficLights { x, bar_h } => {
                // Electron の WindowButtonsProxy::RedoLayout と同じ手（読んだ所は commit message）。NSWindowCloseButton 0・MiniaturizeButton 1・ZoomButton 2
                let send_n: unsafe extern "C" fn(Id, Id) -> usize = std::mem::transmute(msg);
                let send_at: unsafe extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(msg);
                let send_rect: unsafe extern "C" fn(Id, Id, CgRect) = std::mem::transmute(msg);
                let send_origin: unsafe extern "C" fn(Id, Id, CgPoint) = std::mem::transmute(msg);
                let send_get: unsafe extern "C" fn(Id, Id, *mut c_void, usize) = std::mem::transmute(msg);
                // frame（NSRect）を KVC の NSValue で読む = struct の戻り値を objc_msgSend で受けない（x86_64 は stret が要る）
                let frame_of = |v: Id| {
                    let mut r = [0.0f64; 4];
                    let value = send1(v, sel(c"valueForKey:"), nsstring(c"frame"));
                    if !value.is_null() {
                        send_get(value, sel(c"getValue:size:"), r.as_mut_ptr().cast(), std::mem::size_of::<[f64; 4]>());
                    }
                    r
                };
                let app = app().ok_or("AppKit isn't loaded (a process with no windows)")?;
                // 赤黄緑を持つ窓（key / main が無い時 = 他の app が前でも探せる・IME の panel 等の題の帯の無い窓は飛ばす）
                let windows = send0(app, sel(c"windows"));
                let count = if windows.is_null() { 0 } else { send_n(windows, sel(c"count")) };
                let win = (0..count)
                    .map(|i| send_at(windows, sel(c"objectAtIndex:"), i))
                    .find(|w| !send_at(*w, sel(c"standardWindowButton:"), 0).is_null())
                    .ok_or("The Shibubu window has no window buttons")?;
                let buttons: Vec<Id> = (0..3).map(|k| send_at(win, sel(c"standardWindowButton:"), k)).filter(|b| !b.is_null()).collect();
                // 赤黄緑の親 = NSTitlebarView・その親 = NSTitlebarContainerView（窓の上端の帯）
                let bar = send0(send0(buttons[0], sel(c"superview")), sel(c"superview"));
                if bar.is_null() {
                    return Err("The title bar wasn't found".into());
                }
                let win_h = frame_of(win)[3];
                let [bx, by, bw, bh] = frame_of(bar);
                let first = frame_of(buttons[0]);
                // 3 つの間は今の間のまま（寄せた後も同じ = 毎 frame 読み直しても揺れない）
                let step = buttons.get(1).map_or(20.0, |b| frame_of(*b)[0] - first[0]);
                let (top, y) = (win_h - bar_h, ((bar_h - first[3]) / 2.0).round());
                let near = |a: f64, b: f64| (a - b).abs() < 0.5;
                if near(bh, bar_h) && near(by, top) && near(first[0], x) && near(first[1], y) {
                    return Ok("The window buttons are already in place".into());
                }
                send_rect(bar, sel(c"setFrame:"), CgRect(bx, top, bw, bar_h));
                for (i, b) in buttons.iter().enumerate() {
                    send_origin(*b, sel(c"setFrameOrigin:"), CgPoint { x: x + step * i as f64, y });
                }
                Ok(format!("Moved the window buttons to the middle of the {bar_h} pt title bar"))
            }
            AppKitOp::DoubleClickAction => {
                let defaults = send0(objc_getClass(c"NSUserDefaults".as_ptr()), sel(c"standardUserDefaults"));
                let value = if defaults.is_null() { std::ptr::null_mut() } else { send1(defaults, sel(c"stringForKey:"), nsstring(c"AppleActionOnDoubleClick")) };
                let utf8 = if value.is_null() { std::ptr::null() } else { send0(value, sel(c"UTF8String")) as *const c_char };
                Ok(if utf8.is_null() { String::new() } else { CStr::from_ptr(utf8).to_string_lossy().into_owned() })
            }
        }
    }
}

/// MIRROR-FIT-1101 追記 210: `AppKitOp::WebHoles` の穴（[x, y, w, h]・親の座標）を箱 `shown` の中に切り詰め、重なる物を外接の矩形にまとめる
/// （even-odd の path で重なった所が見え直さない）。箱の外の穴・幅か高さが 0 の穴は捨てる。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn hole_rects(shown: [f64; 4], holes: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let [sx, sy, sw, sh] = shown;
    let mut out: Vec<[f64; 4]> = holes
        .iter()
        .map(|&[x, y, w, h]| {
            let (x0, y0) = (x.max(sx), y.max(sy));
            [x0, y0, (x + w).min(sx + sw) - x0, (y + h).min(sy + sh) - y0]
        })
        .filter(|h| h[2] > 0.0 && h[3] > 0.0)
        .collect();
    let meets = |a: &[f64; 4], b: &[f64; 4]| a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
    // ponytail: 外接の矩形にまとめる = L 字に重なると少し余分に抜く・穴は toast・menu・tooltip の数（10 未満）なので総当たりの繰り返しで足りる
    while let Some((i, j)) = (0..out.len()).flat_map(|i| (i + 1..out.len()).map(move |j| (i, j))).find(|&(i, j)| meets(&out[i], &out[j])) {
        let (a, b) = (out[i], out.swap_remove(j));
        let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
        out[i] = [x0, y0, (a[0] + a[2]).max(b[0] + b[2]) - x0, (a[1] + a[3]).max(b[1] + b[3]) - y0];
    }
    out
}

#[cfg(test)]
mod web_holes_tests {
    use super::hole_rects;

    /// MIRROR-FIT-1101 追記 210: 穴は箱の中に切り詰め、重なる穴は 1 つにまとめ（鎖で重なる 3 つも 1 つ）、離れた穴はそのまま・箱の外の穴は捨てる。
    /// 負の対照: `meets` を常に false → 重なった 2 つが残って赤・切り詰めを外す → 右にはみ出した幅のままで赤。
    #[test]
    fn mirror_fit_210_holes_are_cut_to_the_box_and_merged() {
        let shown = [100.0, 50.0, 400.0, 300.0];
        assert_eq!(hole_rects(shown, &[[450.0, 60.0, 100.0, 40.0]]), vec![[450.0, 60.0, 50.0, 40.0]], "箱の右にはみ出した toast を切り詰めない");
        assert_eq!(hole_rects(shown, &[[100.0, 50.0, 10.0, 10.0], [105.0, 55.0, 10.0, 10.0]]), vec![[100.0, 50.0, 15.0, 15.0]]);
        assert_eq!(hole_rects(shown, &[[100.0, 50.0, 10.0, 10.0], [200.0, 50.0, 5.0, 5.0]]).len(), 2, "離れた穴をまとめた");
        assert_eq!(hole_rects(shown, &[[100.0, 50.0, 10.0, 10.0], [108.0, 50.0, 10.0, 10.0], [116.0, 50.0, 10.0, 10.0]]), vec![[100.0, 50.0, 26.0, 10.0]]);
        assert!(hole_rects(shown, &[[600.0, 60.0, 50.0, 50.0], [100.0, 50.0, 0.0, 10.0]]).is_empty(), "箱の外・幅 0 の穴を残した");
    }
}

// ---- WebView の面（v0.14 §3.1・CD-24〜26。WebKit に依らない判定だけ。WebView そのものは webview.rs）----

/// W12W-0: この build に WebView（wry 0.57 の `WebSurface`）が入っているか。release（`scripts/release/build-mac.sh`）は
/// `--features webview` で build するので true・既定の `cargo build` / `cargo test` は false（WebKit の crate を入れない）。
/// 例: `if !surface::WEBVIEW_BUILT { r.state = ReceiptState::Unavailable; r.oracle = surface::WEBVIEW_MISSING.into(); }`
/// 保証する: compile 時の事実だけ（`cfg!(all(feature = "webview", target_os = "macos"))`）。保証しない: WKWebView が実際に開ける・頁が読める事（W12W-1 の実機）。
/// Wave: W12W-0
pub const WEBVIEW_BUILT: bool = cfg!(all(feature = "webview", target_os = "macos"));

/// W12W-0: WebView の無い build で Web の面（`SurfaceTarget::Url`）に出す 1 行（D-e・偽の「接続済み」を出さない）。
/// `project::readback` の oracle と kangae-portal の handoff の log が同じ文言を使う（`kangae-portal.rs` の feature 無しの `web_run`）。
/// 保証しない: 次の一手が使える事（外の browser で開く button は renderer の `source_surface` が持つ）。
/// 契約（UI-2 から・名前と型は変えない）: 値は表の EN `This build doesn't include the web view (cargo feature webview)`（additions.md）。
/// oracle と log の照合は値ではなくこの定数で引く（文で分岐しない）。
/// Wave: W12W-0, UI-2
pub const WEBVIEW_MISSING: &str = "This build doesn't include the web view (cargo feature webview)";

/// W15: その描画の方式がこの build に入っているか（WebView = feature `webview`・Native Direct = `native_direct`・Mirror = `mirror`・他は true）。
/// W15 の 3 回と録画の .app は `native_direct` と `mirror` を切って build する（YC_WAVES の W15 の行・W16 より前の範囲を測る）。
/// Wave: W15
/// 例: `if !surface::built(SurfaceBackend::NativeDirect) { r.state = ReceiptState::Unavailable; r.oracle = surface::missing_line(SurfaceBackend::NativeDirect, &app_name).unwrap_or_default(); }`
/// 保証する: compile 時の事実だけ（`cfg!`）。保証しない: 入っている方式が実機で動く事（W16・W16M・W12W の受け入れ）。
pub fn built(backend: SurfaceBackend) -> bool {
    match backend {
        SurfaceBackend::WebView => WEBVIEW_BUILT,
        SurfaceBackend::NativeDirect => cfg!(feature = "native_direct"),
        SurfaceBackend::InteractiveMirror => cfg!(feature = "mirror"),
        _ => true,
    }
}

/// W15: この build に無い方式の面に出す 1 行（D-e・接続済みと出さない）。形は「この版では <app> を Page に置けません。<app> で外で開いてください」。
/// 保証する: 入っている方式（`built` が true）には None・次の一手（<app> で外で開く）を含む・feature の名前と bundle id を文に出さない。
/// 保証しない: 外で開く button が在る事（renderer の `source_surface` が持つ）。
/// 契約（UI-2 から・署名は変えない）: 英語の 1 行。Mirror は表の EN `This version can't mirror apps · Open in {App}`（copy.en.md）。
/// 表に EN の無い方式（Native Direct・WebView）は表に無い文を作らず sendback に足し、その間は日本語のまま `docs/ui-copy/ja-allowlist.md`。
/// Wave: W15, UI-2
pub fn missing_line(backend: SurfaceBackend, app_name: &str) -> Option<String> {
    (!built(backend)).then(|| format!("This version can't place {app_name} on a page. Open it outside, in {app_name}"))
}

/// YouTube の埋め込みで名乗る Referer（CD-26: bundle id app.kangae.desktop を origin に）。
pub const KANGAE_REFERER: &str = "https://app.kangae.desktop";
/// W12W-1b: 作った（か retry した）時から navigation の開始（`WebEvent::Progress` の `progress > WEB_PENDING_PROGRESS`・`Started`・題名）までの上限（暫定・実測で直す）。
/// 過ぎたら `Recoverable`。混んだ機械では WebKit の WebContent の起動だけで 35 秒かかった（run 5・owner 13 通目）ので、読み込みの上限と分ける。
/// Wave: W12W-1b
pub const WEB_START_LIMIT: Duration = Duration::from_secs(60);
/// navigation の開始から、頁が届く（wry の `Started` = didCommitNavigation・または題名）までの上限。過ぎたら `Recoverable`。
/// W12W-1b: 起点は navigation の開始（作った時ではない・作った時から開始までは `WEB_START_LIMIT`）。
pub const WEB_LOAD_LIMIT: Duration = Duration::from_secs(20);
/// navigation の開始から、使える（finished か題名が出る）までの上限。重い頁は finished が遅い（実測 2026-10-18: load 200 の YouTube の
/// embed は 20 秒で finished が来ないまま動画の題名が出て再生できた）ので、題名でも使えるとみなし、こちらは長めに。
/// W12W-1b: 起点は navigation の開始。
pub const WEB_FINISH_LIMIT: Duration = Duration::from_secs(60);
/// W12W-1b: 読み込みの止まり（`Progress` の `loading` が true を見た後に false）が、`Finished` も題名も無いままこれだけ続いたら `Recoverable`
/// （wry は didFail を渡さないので、これで失敗を取る）。同じ generation の `Finished` が 1 frame 遅れて着くのを待つための猶予。
pub const WEB_STOP_GRACE: Duration = Duration::from_secs(1);
/// W12W-1b: load を頼んだ直後（WebContent の起動の前）に WKWebView が返す `estimatedProgress`。WebKit は pending の API request の間 `isLoading` を true・
/// `estimatedProgress` を `initialProgressValue` = 0.1 で返す（WebKit `Source/WebKit/UIProcess/PageLoadState.cpp` の `estimatedProgress(const Data&)`・2026-10-21 main で確認）。
/// だから navigation の開始はこれより大きい値（`didChangeProgress`）で数える。`> 0` で数えると 35 秒の WebContent の起動が `Loading` の 20 秒に入って run 5 の欠陥が残る。
pub const WEB_PENDING_PROGRESS: f64 = 0.1;
/// W12W-1b: 試験の側がわざと `Recoverable` を起こす手（owner の network・Wi-Fi・proxy・他の process に触らない）。値は `start-once` だけ。
/// WebContent を kill する手は採らない: WebContent は ppid 1 で、どの app の物かは root の `launchctl procinfo` でしか読めず、owner の Safari を落とし得る（2026-10-21 実測）。
pub const WEB_FAULT_ENV: &str = "SHIBUBU_WEB_FAULT";
/// YouTube の題名が動画の題名に変わるのを待つ上限。「YouTube」のままなら埋め込みの拒否（Error 153）とみなす（PoC §9）。
pub const YOUTUBE_TITLE_LIMIT: Duration = Duration::from_secs(10);

/// (space, app) ごとの WebKit の data store の id（16 byte・CD-25 (1)）。同じ組なら毎回同じ・違えば違う（FNV-1a 64 を 2 本・版を越えて変わらない）。
pub fn store_identifier(space_id: &str, software_id: &str) -> [u8; 16] {
    let fnv = |seed: u64| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
        for b in space_id.bytes().chain([0u8]).chain(software_id.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&fnv(0).to_be_bytes());
    id[8..].copy_from_slice(&fnv(0x6b61_6e67_6165).to_be_bytes());
    id
}

/// W12W-2: Browser の面（W12W-B）の website data（cookie と storage）は Shibubu 全体で 1 つの永続の store に置く（YC_WAVES §3・login は Shibubu で 1 回）。
/// 組は `store_identifier(space, software)` に渡す (space, software)。W12W-3 の GitHub の login もこの store に残る。
/// G72: Browser でない Web の面（Diagram の excalidraw.com など）は Project ごとの store（`project::web::store_of`・この組の space を Project の id に替える）。
/// Wave: W12W-2, W12W-3
/// 使い方: `project::web::web_spec` が `WebSpec { space_id: SHARED_WEB_STORE.0.into(), software_id: SHARED_WEB_STORE.1.into(), .. }` を作り、`PageWebViews::place` が `store_of` で決め直す。
/// 保証する: 値を版を越えて変えない（変えると利用者の login が消える）・run の初期化と録画の初期化でこの store を消さない。
/// 保証しない: CD-25 (1)「data store は (space, app) ごと」との両立（§3 の決定で Browser は 1 つに寄せる。`store_identifier` は残す）・
/// G72 の前に Diagram で描いた図を Project の store へ移す事（この store に残り、Browser で excalidraw.com を開けば見える）。
pub const SHARED_WEB_STORE: (&str, &str) = ("shibubu", "web");

/// W12W-2: `WebSurface::evaluate` を今の頁で走らせてよいか（許可した origin の頁だけ・第三者の頁に script を入れない CD-25）。純関数。
/// Wave: W12W-2
/// 例: `surface::may_evaluate(&今の URL, &spec.allow_hosts)?; web.evaluate(js, done)?`
/// 保証する: `decide_navigation` が Allow でない URL（sign-in・http・許可の外・読めない URL）と `about:` の頁は Err の 1 行（何も走らせない）。
/// `allow_hosts` が空（W12W-B の Browser の面・`decide_navigation` では host の制限無し）も Err: script は明示した host の頁だけで走らせ、
/// Browser の「空 = 制限無し」を継がない（利用者が開いた任意の第三者の頁に script を入れない・CD-25）。判定の順は docs/diagrams.md §18 の W12W-2。
/// 保証しない: 返った値が正しい事（製品の自己申告なので receipt は unverified 止まり・YC_ACCEPTANCE §2）。
pub fn may_evaluate(current_url: &str, allow_hosts: &[String]) -> Result<(), String> {
    // about: は decide_navigation が Allow にする（sub-frame の空の頁）ので先に止める
    if current_url.starts_with("about:") {
        return Err("The page hasn't loaded yet, so the script wasn't run".into());
    }
    if allow_hosts.iter().all(|h| h.trim().is_empty()) {
        return Err("This browser has no fixed host, so the script wasn't run".into());
    }
    match decide_navigation(current_url, allow_hosts) {
        WebNav::Allow => Ok(()),
        WebNav::Deny { why, .. } => Err(why),
    }
}

/// YouTube の埋め込みの URL（video id は `^[A-Za-z0-9_-]{11}$` だけ・CD-25 (7)）。
pub fn youtube_embed_url(video_id: &str) -> Result<String, String> {
    let ok = video_id.len() == 11 && video_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
    if !ok {
        return Err(format!("A YouTube video ID is 11 letters, digits, _ or - (“{video_id}”)"));
    }
    Ok(format!("https://www.youtube.com/embed/{video_id}"))
}

/// URL の host（小文字・port は外す）。scheme が無い・壊れた URL は None。
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let host = auth.rsplit('@').next().unwrap_or(auth);
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or("").to_string() };
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// 1 frame の focus の持ち主の直し（egui の欄と WKWebView・`WebSurface::frame` が毎 frame 使う）。
/// Wave: W12W-3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusFix {
    /// 何もしない（first responder が WebView の外）
    Keep,
    /// egui の focus を外す（WebView が自分で first responder を取った・autofocus か人の click。focus の枠を二重に出さない）
    EguiLetsGo,
    /// 窓の first responder を WebView の親（egui の view）へ戻す（egui の欄がこの frame に focus を取った）
    ToHost,
}

/// first responder が WebView の中（`web_has_responder`）の時、どちらに focus を渡すか。`egui_took_focus` = egui の focus がこの frame に別の widget へ移った。
/// 例: `match focus_fix(within, now != last && now.is_some()) { FocusFix::ToHost => appkit(AppKitOp::FirstResponderToHost(wk)), .. }`
/// 保証する: egui が focus を取った frame は ToHost（AX の AXPress / AXFocused は AppKit の first responder を動かさないので、戻さないと打った key は WebView の中の
/// 第三者の頁の欄に届く・W12W-3 run-1 の 19）・それ以外で WebView の中なら EguiLetsGo（前と同じ）。保証しない: AppKit が first responder を受け取る事（呼ぶ側が "1" を見る）。
/// Wave: W12W-3
pub fn focus_fix(web_has_responder: bool, egui_took_focus: bool) -> FocusFix {
    match (web_has_responder, egui_took_focus) {
        (false, _) => FocusFix::Keep,
        (true, true) => FocusFix::ToHost,
        (true, false) => FocusFix::EguiLetsGo,
    }
}

/// navigation の判定（wry の navigation handler は sub-frame でも呼ばれ、frame を区別できない・PoC §9・§11）。
#[derive(Debug, Clone, PartialEq)]
pub enum WebNav {
    Allow,
    /// その読み込みだけ止める（handoff = true なら面ごと本人の browser へ）
    Deny { handoff: bool, why: String },
}

/// sign-in・OAuth の頁か（CD-25 (6): 埋め込みで login させない）。
pub fn is_sign_in(url: &str) -> bool {
    let host = host_of(url).unwrap_or_default();
    let lower = url.to_lowercase();
    host == "accounts.google.com"
        || host.ends_with(".accounts.google.com")
        || host == "appleid.apple.com"
        || host == "login.microsoftonline.com"
        || ["/o/oauth2/", "/oauth/authorize", "/oauth2/authorize", "/oauth2/v2.0/authorize"].iter().any(|p| lower.contains(p))
}

/// url を WebView の中で読んでよいか。about:blank / about:srcdoc（sub-frame の空の頁）は通す。https だけ・許可の host（とその sub domain）だけ。
/// http は 127.0.0.1 / localhost（自前の頁）だけ。sign-in / OAuth は handoff。
/// W12W-B: `allow_hosts` が空なら host の制限は無い（Browser）。https / localhost-http / sign-in の規則は残す。
pub fn decide_navigation(url: &str, allow_hosts: &[String]) -> WebNav {
    if url == "about:blank" || url == "about:srcdoc" {
        return WebNav::Allow;
    }
    if is_sign_in(url) {
        return WebNav::Deny { handoff: true, why: "Sign in with your own browser. Shibubu doesn't sign in for you".into() };
    }
    let Some(host) = host_of(url) else {
        return WebNav::Deny { handoff: false, why: format!("Can't read this URL ({})", url.chars().take(80).collect::<String>()) };
    };
    let scheme = url.split_once("://").map(|(s, _)| s.to_lowercase()).unwrap_or_default();
    let local = host == "127.0.0.1" || host == "localhost";
    if !(scheme == "https" || (scheme == "http" && local)) {
        return WebNav::Deny { handoff: false, why: format!("{host} isn't https, so it won't open") };
    }
    if allow_hosts.is_empty() {
        return WebNav::Allow;
    }
    let allowed = allow_hosts.iter().any(|a| {
        let a = a.trim().to_lowercase();
        !a.is_empty() && (host == a || host.ends_with(&format!(".{a}")))
    });
    if allowed {
        WebNav::Allow
    } else {
        WebNav::Deny { handoff: false, why: format!("{host} won't open (use Open in browser to go to your own browser)") }
    }
}

/// W12W-B: Browser の上の欄の文を開く URL にする。空は None。
/// `https://…` / `http://…` はそのまま・`example.com` は `https://example.com`・それ以外（空白を含む言葉）は Google 検索。
/// 例: `bar_input_to_url("猫 動画") == Some("https://www.google.com/search?q=%E7%8C%AB+%E5%8B%95%E7%94%BB")`
/// 保証する: 返す URL は `https:` か `http:` で始まる。保証しない: その頁が読める事（`decide_navigation` が http と sign-in を止める）。
/// Wave: W12W-B
pub fn bar_input_to_url(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(s.to_owned());
    }
    let host_like = !s.chars().any(char::is_whitespace)
        && (s.contains('.') || lower == "localhost" || lower.starts_with("localhost:"));
    if host_like {
        return Some(format!("https://{s}"));
    }
    Some(format!("https://www.google.com/search?q={}", encode_query(s)))
}

/// W12W-B: Live の頁の URL が spec と違う時だけ保存する（同じなら毎 frame 書かない）。
/// 例: `if let Some(u) = persistable_url(&spec.url, &life.url, matches!(life.state, WebState::Live)) { apply SetSurfaceUrl }`
/// 保証する: 空と spec と同じ URL は None。末尾の `/` と scheme の大文字小文字だけの差も同じと見る
/// （WKWebView は `https://excalidraw.com` を `https://excalidraw.com/` にする。INTEG-3b: Diagram を開くだけで ↶ が 1 つ増えた）。
/// 保証しない: その頁が読める事。
/// Wave: W12W-B
pub fn persistable_url(spec_url: &str, life_url: &str, live: bool) -> Option<String> {
    let norm = |u: &str| {
        let u = u.trim_end_matches('/');
        match u.split_once("://") {
            Some((scheme, rest)) => format!("{}://{rest}", scheme.to_ascii_lowercase()),
            None => u.to_owned(),
        }
    };
    (live && !life_url.is_empty() && norm(life_url) != norm(spec_url)).then(|| life_url.to_owned())
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// WebView の面を開く時の事実（feature に依らない。開くのは webview.rs）。
#[derive(Debug, Clone, Default)]
pub struct WebSpec {
    /// 開く URL（YouTube なら空にして `youtube` に video id）
    pub url: String,
    pub youtube: Option<String>,
    /// YouTube の負の対照だけ: Referer を付けない（CD-26 の検査）
    pub no_referer: bool,
    /// app（software）の id・space の id（data store を分ける）
    pub software_id: String,
    pub space_id: String,
    /// 読んでよい host（registry の official_origin。sub-frame の host も要る: wry は frame を区別できない）
    pub allow_hosts: Vec<String>,
    /// W12W-1: download を受ける一時の dir（絶対 path）。None = 今まで通り download は止める（`WebEvent::Download`）。
    /// Some = wry の download_started で `staged_download_path(dir, 提案の path)` に落とし、`WebEvent::DownloadStaged` を送る。
    /// 名前と repo の中の置き場は Shibubu の inline の欄で利用者が決める（`project::web::save_download`・NSSavePanel は出さない）。
    /// 例: `WebSpec { download_dir: Some(data_dir.join(project::web::STAGING_DIR)), ..spec }`。
    /// 保証しない: Excalidraw の書き出し（`<a download>` の blob）が wry の handler に届く事（W12W-1 の probe 3・未測）。
    pub download_dir: Option<PathBuf>,
    /// G29（B3）: 頁の文書の開始で `REPORT_VISIBLE_JS` を入れ、隠れていても「見えている」と答えさせる。`project::web::web_spec` が keep_alive で Browser でない面（Diagram）だけ true。
    /// 根: Excalidraw は `document.hidden` の間、図を localStorage に保存しない（excalidraw-app/data/LocalData.ts の `isSavePaused`）。WKWebView は Shibubu の窓が
    /// 別の app の後ろ（BG）か別の Page に居る時に頁を hidden にするので、BG で描いた図が再起動で消えた（docs/evidence/W12W-2/log.md:87）。
    pub report_visible: bool,
}

/// G29: `WebSpec::report_visible` の面に文書の開始で入れる script（main frame だけ）。`document` の 2 つの getter を上書きするだけで、頁の外には触らない。
/// 保証しない: WebKit が隠れた頁の timer を間引くのは止めない（保存の debounce 300ms が遅れるだけ）。
pub const REPORT_VISIBLE_JS: &str = r#"Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });"#;

// ---- chat の ```html の閉じた WebView（CHAT-FREE-1101 段 2・owner 198 の 6 / 201 の「AI の画用紙」）。WebView そのものは webview.rs の `Canvas` ----

/// chat の ```html の文書の CSP: 外の network・file・frame・form の送り先を全部断り、AI が書いた inline の script / style と data: / blob: の絵と字だけ許す。
/// 後から足した CSP は緩められない（頁の script が meta を消しても効いたまま）。
pub const CANVAS_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; form-action 'none'; base-uri 'none'";

/// 返事の長さの上限（字）。超える物は送らない（AI の頁が会話を埋めない）。
pub const CANVAS_REPLY_MAX: usize = 2000;

/// chat の ```html の中の button が Shibubu に言える 2 つ（`window.shibubu.reply(text)` / `window.shibubu.build()`）と、AI-COMPOSE-1101 の Page の部品
/// （`canvas`）が言える 3 つ（`shibubu.read(app, what)` / `shibubu.set(key, value)` / `shibubu.act(intent, value)`）。どちらの口を開けるかは `CanvasBridge`。
/// 新しくした理由: 既存の `WebEvent` は頁の読み込みと navigation の事実で、頁から Shibubu への頼みを持つ物が無い（Browser の面は IPC を登録しない）。
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasMsg {
    /// この文を本人の返事として送る（trim 済み・空でない・`CANVAS_REPLY_MAX` 字まで）
    Reply(String),
    /// 案の Build now を押す
    Build,
    /// AI-COMPOSE-1101: app を読むだけの頼み（`id` = 頁の Promise の番号・答えは `canvas_answer_js`）。読むのは `PageEngine::read`（書く手は 1 手も無い）
    Read { id: u64, app: String, what: String },
    /// AI-COMPOSE-1101: 部品の値を Project に残す（`ui::CanvasProps::set_patch`）
    Set { key: String, value: Value },
    /// AI-COMPOSE-1101: Engine に頼む（`intent` の `{value}` を値に・承認の決まりは今のまま = 書く手は承認の card）
    Act { intent: String, value: Value },
    /// TALK-HONEST-1101 追記 232: 読めなかった read の後に頁の bridge が出した「Let Shibubu open it」を押した（一言は頁から受けず、Shibubu が覚えた
    /// その部品の `ReadMiss::open` を使う = `Generated::open_part`）
    Open,
    /// OPEN-INPLACE-1101 の 1: 部品の所の承認の card（`PartRun::Approve`）で「Always allow」（`always`）か「Allow once」を押した
    /// （`{allow:"always"}` / `{allow:"once"}`・app.rs が `Generated::engine_act(AlwaysAllow / Approve)` へ）
    Allow { always: bool },
    /// OPEN-INPLACE-1101 の 1: 部品の所の承認の card で「Cancel」（`{allow:"cancel"}`・`engine_act(Cancel)` = 何も送らない）
    Cancel,
}

/// OPEN-INPLACE-1101 の 1（owner 240「let shibubu open it を押したら勝手に動くように」）: 「Let Shibubu open it」で始めた run の今を、押した部品の所に出す 1 つ
/// （`Generated::part_run` が作り `canvas_run_js` が頁の bridge へ渡す）。済み（✓）は無い = その部品を読み直す（`take_rereads`）。
/// 新しい型の理由: 頁に渡すのは 1 行と押せる物だけ（`EngineView` は egui の card の型で goal・手・digest を持ち、頁に渡す物ではない）。
/// Wave: OPEN-INPLACE-1101
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartRun {
    /// 進めている（Loader と、何をしているかの 1 行 = Engine の `progress` か `Working on “{一言}”`）
    Working(String),
    /// 承認が要る（「どの app で何を押すか」の 1 行 = `engine_view::approval_line`）。頁は Always allow / Allow once / Cancel を出す。
    /// ENGINE-AUDIT W7（欄を足した）: 2 つ目 = 毎回聞く理由の 1 行（`engine_view::asks_every_time` = 押す手だけの承認）。Some の時、頁は Always allow を出さずにこの 1 行を出す
    Approve(String, Option<String>),
    /// 止まった（理由と次の一手の 1 行・押す物は無い。次の read で消える）
    Stopped(String),
}

/// AI-COMPOSE-1101: 閉じた文書の口の向き。`Chat` = 会話の答えの中（reply / build の 2 つ）・`Page` = Page に置いた `canvas` の部品（read / set / act の 3 つと前に残した値）。
/// 片方の口をもう片方で開けない（Chat の頁は app を読めず、Page の部品は Build now を押せない）。
/// 新しい型の理由: 同じ閉じた文書（`canvas_document`）を 2 か所に置き、置き場で開ける口だけを替える物が要る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasBridge {
    Chat,
    Page,
}

/// 頁の script が読む `window.shibubu`（Chat の 2 つの口）
const CHAT_BRIDGE_JS: &str = "window.shibubu=Object.freeze({reply:function(t){window.ipc.postMessage(JSON.stringify({reply:String(t)}))},\
build:function(){window.ipc.postMessage(JSON.stringify({build:true}))}});";
/// 頁の script が読む `window.shibubu`（Page の 3 つの口・`values` = 前に残した値・`read` は Promise で `canvas_answer_js` が答える）。`__VALUES__` を JSON に替えて使う
/// TALK-HONEST-1101 追記 232: 読めなかった答えが開けば読める物（`canvas_answer_js` の `open`）なら頁の最後に「Let Shibubu open it」の button を 1 つ（押すと `{open:true}`）
/// OPEN-INPLACE-1101 の 1: `__shibubuRun`（`canvas_run_js`）が button を外して同じ所に run の今の枠を 1 つ（承認は 3 つの button・止まった時は button を出し直す）
const PAGE_BRIDGE_JS: &str = "(function(){var n=0,w={};\
var offer=function(){if(document.getElementById('shibubu-open'))return;var b=document.createElement('button');b.id='shibubu-open';b.type='button';\
b.textContent='Let Shibubu open it';b.style.marginTop='8px';b.onclick=function(){post({open:true})};document.body.appendChild(b)};\
window.__shibubuAnswer=function(i,ok,v,o){var p=w[i];if(!p)return;delete w[i];if(ok)p[0](v);else{p[1](new Error(v));if(o)offer()}};\
var post=function(m){window.ipc.postMessage(JSON.stringify(m))};\
window.__shibubuRun=function(s,t,a){var f=document.getElementById('shibubu-run');if(!s){if(f)f.remove();return}\
var o=document.getElementById('shibubu-open');if(o)o.remove();\
if(!f){f=document.createElement('div');f.id='shibubu-run';f.setAttribute('role','status');\
f.style.cssText='margin-top:8px;display:flex;flex-wrap:wrap;align-items:center;gap:6px';document.body.appendChild(f)}\
f.textContent='';if(s==='working'){var k=document.createElement('span');\
k.style.cssText='width:10px;height:10px;border:2px solid currentColor;border-top-color:transparent;border-radius:50%;display:inline-block';f.appendChild(k);\
if(k.animate)k.animate([{transform:'rotate(0deg)'},{transform:'rotate(360deg)'}],{duration:800,iterations:Infinity})}\
var l=document.createElement('span');l.textContent=t;f.appendChild(l);\
var btn=function(x,m){var b=document.createElement('button');b.type='button';b.textContent=x;b.onclick=function(){f.textContent='';post(m)};f.appendChild(b)};\
if(s==='approve'){if(a){var e=document.createElement('span');e.textContent=a;f.appendChild(e)}else btn('Always allow',{allow:\"always\"});btn('Allow once',{allow:\"once\"});btn('Cancel',{allow:\"cancel\"})}\
if(s==='stopped')offer()};\
window.shibubu=Object.freeze({values:Object.freeze(__VALUES__),\
read:function(a,q){return new Promise(function(r,j){var i=++n;w[i]=[r,j];post({read:{id:i,app:String(a),what:String(q)}})})},\
set:function(k,v){post({set:{key:String(k),value:v===undefined?null:v}})},\
act:function(t,v){post({act:{intent:String(t),value:v===undefined?null:v}})}});})();";

/// AI の html を閉じた文書に包む: 先頭に Shibubu の head（charset・`CANVAS_CSP`・`window.shibubu` の口・地の字）を置き、AI の html は body に入れる。
/// AI の doctype の前に meta を置かない（quirks mode になる）・AI の html / head / body の入れ子は HTML の parser が畳む。純関数。
/// `values` は `CanvasBridge::Page` の時だけ `shibubu.values` に入れる（`<` は `\u003c` に替える = 値の中の `</script>` で head を閉じさせない）。Chat は見ない。
/// 例: `canvas_document("<svg>…</svg>", CanvasBridge::Chat, &Map::new())` → `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" …>…</head><body><svg>…</svg></body></html>`
/// 保証しない: AI の html が正しく描ける事（壊れた html は WebKit が描ける所まで描く）。
pub fn canvas_document(html: &str, bridge: CanvasBridge, values: &serde_json::Map<String, Value>) -> String {
    let script = match bridge {
        CanvasBridge::Chat => CHAT_BRIDGE_JS.to_owned(),
        CanvasBridge::Page => PAGE_BRIDGE_JS.replace("__VALUES__", &Value::Object(values.clone()).to_string().replace('<', "\\u003c")),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{CANVAS_CSP}\">\
         <meta name=\"color-scheme\" content=\"light dark\"><style>html,body{{margin:0}}body{{padding:12px;font:13px -apple-system,system-ui,sans-serif}}</style>\
         <script>{script}</script></head><body>{html}</body></html>"
    )
}

/// 頁から届いた 1 通（wry の IPC の body）→ `bridge` で開けた口のどれか。他の口・他の形・空の文・長すぎる物は None（黙って捨てる）。純関数。
/// 例: `canvas_message(r#"{"reply":"Blue"}"#, CanvasBridge::Chat) == Some(CanvasMsg::Reply("Blue".into()))`・
/// `canvas_message(r#"{"read":{"id":1,"app":"GarageBand","what":"guitar sounds"}}"#, CanvasBridge::Page) == Some(CanvasMsg::Read { id: 1, app: "GarageBand".into(), what: "guitar sounds".into() })`
/// 保証する: 文（reply・app・what・key・intent）は trim して空でなく `CANVAS_REPLY_MAX` 字まで・値（set・act）は JSON の 1 行で `CANVAS_REPLY_MAX` byte まで・
/// read の id は 0 より大きい整数。
/// OPEN-INPLACE-1101: Page の `{allow:"always"}` → `Allow { always: true }`・`{allow:"once"}` → `Allow { always: false }`・`{allow:"cancel"}` → `Cancel`（他の字は None）。
pub fn canvas_message(body: &str, bridge: CanvasBridge) -> Option<CanvasMsg> {
    let v: Value = serde_json::from_str(body).ok()?;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty() && t.chars().count() <= CANVAS_REPLY_MAX).map(str::to_owned);
    let value = |v: Option<&Value>| v.cloned().filter(|x| x.to_string().len() <= CANVAS_REPLY_MAX);
    match bridge {
        CanvasBridge::Chat if v.get("build") == Some(&Value::Bool(true)) => Some(CanvasMsg::Build),
        CanvasBridge::Chat => text(v.get("reply")).map(CanvasMsg::Reply),
        CanvasBridge::Page => {
            if let Some(r) = v.get("read") {
                let id = r.get("id").and_then(Value::as_u64).filter(|i| *i > 0)?;
                return Some(CanvasMsg::Read { id, app: text(r.get("app"))?, what: text(r.get("what"))? });
            }
            if let Some(s) = v.get("set") {
                return Some(CanvasMsg::Set { key: text(s.get("key"))?, value: value(s.get("value"))? });
            }
            if v.get("open") == Some(&Value::Bool(true)) {
                return Some(CanvasMsg::Open);
            }
            if let Some(a) = v.get("allow") {
                return match a.as_str()? {
                    "always" => Some(CanvasMsg::Allow { always: true }),
                    "once" => Some(CanvasMsg::Allow { always: false }),
                    "cancel" => Some(CanvasMsg::Cancel),
                    _ => None,
                };
            }
            let a = v.get("act")?;
            Some(CanvasMsg::Act { intent: text(a.get("intent"))?, value: value(a.get("value"))? })
        }
    }
}

/// OPEN-INPLACE-1101 の 1: 部品の所の run の今を頁へ渡す script（`Canvas::run` が `evaluate_script` に渡す・None = 消す）。頁の bridge
/// （`PAGE_BRIDGE_JS` の `window.__shibubuRun`）は「Let Shibubu open it」の button の所（無ければ body の最後）に 1 つの枠を置き、Working = 回る印と 1 行・
/// Approve = 1 行と 3 つの button（`Always allow`・`Allow once`・`Cancel` = `{allow:"always"|"once"|"cancel"}`・ENGINE-AUDIT W7: 毎回聞く 1 行が在れば
/// Always allow の代わりにその 1 行 = 3 つ目の引数）・Stopped = 1 行を出す。純関数。
/// 例: `canvas_run_js(Some(&PartRun::Working("Opening the list".into())))` → `window.__shibubuRun&&window.__shibubuRun("working","Opening the list")`・
/// `canvas_run_js(None)` → `window.__shibubuRun&&window.__shibubuRun(null,"")`（枠を消す）
/// 保証する: 字は JSON の文字列で渡す（`canvas_answer_js` と同じ）・頁の script が AI の html の要素を書き換えない（枠は bridge が作った 1 つだけ）。
/// Wave: OPEN-INPLACE-1101
pub fn canvas_run_js(run: Option<&PartRun>) -> String {
    let js = |t: &str| Value::String(t.to_owned()).to_string().replace('<', "\\u003c");
    let (state, line, every) = match run {
        None => ("null", "", None),
        Some(PartRun::Working(l)) => ("\"working\"", l.as_str(), None),
        Some(PartRun::Approve(l, every)) => ("\"approve\"", l.as_str(), every.as_deref()),
        Some(PartRun::Stopped(l)) => ("\"stopped\"", l.as_str(), None),
    };
    // ENGINE-AUDIT W7: 毎回聞く承認は 3 つ目の引数（頁は Always allow の代わりにその 1 行）
    let every = every.map_or_else(String::new, |a| format!(",{}", js(a)));
    format!("window.__shibubuRun&&window.__shibubuRun({state},{}{every})", js(line))
}

/// AI-COMPOSE-1101: `shibubu.read` の答えを頁へ返す script（`Canvas::answer` が `evaluate_script` に渡す）。Ok = 読めた値・Err = 読めなかった理由の 1 行（Promise は reject）。純関数。
/// TALK-HONEST-1101 追記 232（引数を足した）: `open` = Shibubu が開けば読める（頁の bridge が「Let Shibubu open it」を出す → 押すと `CanvasMsg::Open`）。
/// 例: `canvas_answer_js(3, &Ok(json!(["Acoustic Guitar", "Clean Guitar"])), false)` → `window.__shibubuAnswer&&window.__shibubuAnswer(3,true,["Acoustic Guitar","Clean Guitar"],false)`
pub fn canvas_answer_js(id: u64, got: &Result<Value, String>, open: bool) -> String {
    let (ok, v) = match got {
        Ok(v) => (true, v.clone()),
        Err(why) => (false, Value::String(why.clone())),
    };
    format!("window.__shibubuAnswer&&window.__shibubuAnswer({id},{ok},{},{})", v.to_string().replace('<', "\\u003c"), open && !ok)
}

/// chat の ```html の WebView が移ってよい所 = `about:`（最初の文書は `about:blank`・srcdoc の frame）だけ。link・form・`location` で外へ出る手は全部止める。
/// wry の navigation handler は初めの読み込み（loadHTMLString）にも全 frame にも呼ばれる（wry-0.57.0/src/wkwebview/navigation.rs:50）ので、about: を断ると何も描かれない。
pub fn canvas_may_navigate(url: &str) -> bool {
    url.starts_with("about:")
}

// ---- Terminal の描き手（TERM-XTERM-1101・owner 204 の 4「普通のterminalをshibubuで使えないの？orcaみたいに」）。WebView そのものは webview.rs の `TermView` ----
// shell は今までの portable-pty の session（`terminal::TerminalManager`）・裏の読み手は alacritty_terminal（AI が読む・TERM-RESTORE の保存）のまま。
// 描くのだけを Orca・VS Code と同じ xterm.js（WebGL の描き手）にする。xterm.js の file は vendor/xterm（版と出所は vendor/xterm/PACKAGES.md）を binary に入れる = CDN を読まない。

/// 1 回に頁へ送る出力の上限（byte）。超える分は次の frame へ回す（大きな出力で 1 frame の main thread を止めない）。
pub const TERM_WRITE_MAX: usize = 256 * 1024;
/// 頁から届く打鍵 1 通の上限（byte・貼り付けもこれまで）。超える通は捨てる（頁の script の暴走で PTY を埋めない）。
pub const TERM_INPUT_MAX: usize = 1 << 20;

/// TERM-XTERM-1101: xterm.js の頁が Shibubu に言う事（wry の IPC の body を `term_message` が通した物だけ・8 つ）。
/// 新しくした理由: `CanvasMsg` は chat の頁の 2 つの口（返事・Build now）で、打鍵・大きさ・link・menu・遅れを持たない。
#[derive(Debug, Clone, PartialEq)]
pub enum TermMsg {
    /// 頁が xterm を作り終えた。これより前に出力を送らない（読み込みの前の evaluate は捨てられる）。来たら `vt::seed` の画面を 1 回送ってから流す
    Ready,
    /// 打った字・貼り付け・IME の確定・mouse の報告（xterm の onData / onBinary）→ PTY へそのまま（人の入力 = `write_as(Human, ..)`）
    Input(Vec<u8>),
    /// xterm が枠に合わせた大きさ（fit addon）→ `TerminalManager::resize`（1..=1000）
    Size { cols: u16, rows: u16 },
    /// link を押した（http / https だけ）→ 既定の browser（`open_in_browser`）
    Open(String),
    /// 右 click（枠の左上からの point）とその時の選びの字（無ければ空）→ grid と同じ egui の menu（Copy・Copy all・Paste・Clear・Split・Terminate）
    Menu { x: f32, y: f32, selection: String },
    /// 打ってから画面に出るまで（ms・0..=60000）→ `TermLatency`
    Shown(u32),
    /// 頁に focus が在る間に押した Shibubu の short cut（⌘ の key・⌃`）。頁が持つ ⌘C・⌘V・⌘X・⌘A・⌘Z・⌘F は来ない。
    /// → 次の frame の egui の入力（app.rs の `raw_input_hook`）= ⌘K・⌘T・⌃` などが Terminal の中からも効く（first responder が WebView の間 egui に key が来ない）
    Shortcut { key: eframe::egui::Key, modifiers: eframe::egui::Modifiers },
}

/// 頁から届いた 1 通（JSON・`{"t": 種類, ..}`）→ `TermMsg`。形の違う通・範囲の外・http(s) でない link・`TERM_INPUT_MAX` を超える打鍵は None（黙って捨てる）。純関数。
/// 種類: `ready`・`in`（`d` = 字）・`bin`（`d` = 1 字 1 byte の字列・xterm の onBinary）・`size`（`cols` `rows`）・`open`（`url`）・`menu`（`x` `y` `sel`）・`shown`（`ms`）・
/// `key`（`code` = KeyboardEvent.code・`command` `ctrl` `shift` `alt`）。
/// 例: `term_message(r#"{"t":"in","d":"ls\r"}"#) == Some(TermMsg::Input(b"ls\r".to_vec()))`・`term_message(r#"{"t":"size","cols":80,"rows":24}"#) == Some(TermMsg::Size { cols: 80, rows: 24 })`
pub fn term_message(body: &str) -> Option<TermMsg> {
    let v: Value = serde_json::from_str(body).ok()?;
    let text = |k: &str| v.get(k).and_then(Value::as_str);
    let dim = |k: &str| v.get(k)?.as_u64().filter(|n| (1..=1000).contains(n)).map(|n| n as u16);
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let input = |b: Vec<u8>| (b.len() <= TERM_INPUT_MAX).then_some(TermMsg::Input(b));
    match text("t")? {
        "ready" => Some(TermMsg::Ready),
        "in" => input(text("d")?.as_bytes().to_vec()),
        // xterm の onBinary は 1 字 = 1 byte（U+0000〜U+00FF）の字列
        "bin" => input(text("d")?.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect::<Option<Vec<u8>>>()?),
        "size" => Some(TermMsg::Size { cols: dim("cols")?, rows: dim("rows")? }),
        "open" => text("url").filter(|u| u.starts_with("https://") || u.starts_with("http://")).map(|u| TermMsg::Open(u.to_owned())),
        "menu" => Some(TermMsg::Menu { x: v.get("x")?.as_f64()? as f32, y: v.get("y")?.as_f64()? as f32, selection: text("sel").unwrap_or_default().to_owned() }),
        "shown" => v.get("ms")?.as_u64().filter(|ms| *ms <= 60_000).map(|ms| TermMsg::Shown(ms as u32)),
        "key" => {
            let code = text("code")?;
            let name = match code {
                "BracketLeft" => "[",
                "BracketRight" => "]",
                c => c.strip_prefix("Key").unwrap_or(c),
            };
            let (command, ctrl) = (flag("command"), flag("ctrl"));
            // 頁が持つ key と、修飾の無い key は short cut にしない（打った字は `in` で来る）
            let key = eframe::egui::Key::from_name(name).filter(|k| (command || ctrl) && !(command && [eframe::egui::Key::C, eframe::egui::Key::V, eframe::egui::Key::X, eframe::egui::Key::A, eframe::egui::Key::Z, eframe::egui::Key::F].contains(k)))?;
            Some(TermMsg::Shortcut { key, modifiers: eframe::egui::Modifiers { alt: flag("alt"), ctrl, shift: flag("shift"), mac_cmd: command, command } })
        }
        _ => None,
    }
}

/// TERM-XTERM-1101: xterm の見た目（Shibubu の theme の token から `ui::theme::term_theme` が作る・Dark / Light を替えたら `TermView::set_theme`）。
/// 字は macOS の等幅（`ui-monospace` = SF Mono → Menlo）を WebKit が CoreText で描く（egui の灰色の anti-alias ではない）。
/// 新しくした理由: `Tokens` は egui の色（`Color32`）で、頁へ渡す形（rgb の列と字の大きさ）とは別。
#[derive(Debug, Clone, PartialEq)]
pub struct TermTheme {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub cursor: [u8; 3],
    /// 選びの地（rgba）
    pub selection: [u8; 4],
    /// 16 色（0〜7 = 普通・8〜15 = 明るい）。egui の grid と同じ `Tokens::ansi`
    pub ansi: [[u8; 3]; 16],
    /// 字の大きさ（point・`Type::Mono` と同じ）
    pub font_size: f32,
}

/// TERM-XTERM-1101: xterm の頁 1 枚（`TermView::open` が `with_html` で読む）。head に vendor/xterm の css と js（xterm・webgl・fit・search・web-links・unicode11）を
/// そのまま入れ、Shibubu の糊（`window.ipc` へ `term_message` の形で言う・`w(base64)` で書く・⌘F の検索の欄・右 click を menu へ）を足す。外の file・CDN は読まない。純関数。
/// 例: `term_document(&theme)` → `<!doctype html><html><head><meta charset="utf-8"><style>/* xterm.css */…</style><script>/* xterm.js */…</script>…</head><body><div id="t"></div>…`
/// 保証する: WebGL が使えない（context を作れない・失った）時は xterm の DOM の描き手に落ちる（字は描ける）。
/// 保証しない: 頁の中の検索の欄の文言を ui-copy の外で変えない事（文言は docs/ui-copy/additions.md）。
pub fn term_document(theme: &TermTheme) -> String {
    const CSS: &str = include_str!("../../vendor/xterm/xterm.css");
    const JS: [&str; 6] = [
        include_str!("../../vendor/xterm/xterm.js"),
        include_str!("../../vendor/xterm/addon-webgl.js"),
        include_str!("../../vendor/xterm/addon-fit.js"),
        include_str!("../../vendor/xterm/addon-search.js"),
        include_str!("../../vendor/xterm/addon-web-links.js"),
        include_str!("../../vendor/xterm/addon-unicode11.js"),
    ];
    const GLUE: &str = include_str!("term.js");
    // 頁の中の枠（Orca の pane と同じ内側 4）と ⌘F の欄（右上）。字の色は theme（`--find-fg`）
    const PAGE_CSS: &str = "html,body{margin:0;height:100%;overflow:hidden}#t{position:absolute;inset:4px}\
        #find{position:absolute;top:6px;right:14px;z-index:10;display:flex;gap:4px;align-items:center;padding:4px 6px;border-radius:6px;\
        background:rgba(60,60,60,.92);font:12px -apple-system,system-ui,sans-serif;color:var(--find-fg,#eee)}#find[hidden],#none[hidden]{display:none}\
        #find input{width:160px;font:inherit;color:inherit;background:rgba(0,0,0,.35);border:1px solid rgba(255,255,255,.18);border-radius:4px;padding:2px 6px;outline:none}\
        #find button{font:inherit;color:inherit;background:none;border:0;padding:0 4px;cursor:pointer}#none{opacity:.7}";
    let mut doc = format!(
        "<!doctype html><html style=\"background:{bg};--find-fg:{fg}\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{TERM_CSP}\"><style>{CSS}\n{PAGE_CSS}</style>",
        bg = hex(theme.background),
        fg = hex(theme.foreground),
    );
    for js in JS {
        doc.push_str("<script>");
        doc.push_str(js);
        doc.push_str("</script>");
    }
    doc.push_str(&format!(
        "</head><body><div id=\"t\"></div><div id=\"find\" hidden><input id=\"q\" placeholder=\"Find\" aria-label=\"Find\" spellcheck=\"false\">\
         <span id=\"none\" hidden>No results</span><button id=\"prev\" aria-label=\"Previous match\">↑</button><button id=\"next\" aria-label=\"Next match\">↓</button>\
         <button id=\"close\" aria-label=\"Close find\">×</button></div><script>const SHIBUBU_THEME={};\n{GLUE}</script></body></html>",
        theme_json(theme)
    ));
    doc
}

/// Terminal の頁の CSP: 外の network・file・frame・form を全部断り、中の script と style だけ（xterm.js は inline・字は Mac の font）。
const TERM_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'";

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// xterm の `ITheme` と字の大きさ（`{"fontSize":12,"colors":{"background":"#…",…}}`）
fn theme_json(t: &TermTheme) -> String {
    const NAMES: [&str; 16] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
    ];
    let [r, g, b, a] = t.selection;
    let mut colors = serde_json::Map::new();
    for (k, c) in [("background", t.background), ("foreground", t.foreground), ("cursor", t.cursor), ("cursorAccent", t.background)] {
        colors.insert(k.into(), json!(hex(c)));
    }
    colors.insert("selectionBackground".into(), json!(format!("rgba({r},{g},{b},{:.3})", f32::from(a) / 255.0)));
    for (k, c) in NAMES.iter().zip(t.ansi) {
        colors.insert((*k).into(), json!(hex(c)));
    }
    json!({ "fontSize": t.font_size, "colors": colors }).to_string()
}

/// TERM-XTERM-1101: Dark / Light を替えた時に頁へ送る script（`term_document` と同じ見た目の JSON を `setTheme` に）。純関数。
pub fn term_theme_script(theme: &TermTheme) -> String {
    format!("setTheme({})", theme_json(theme))
}

/// TERM-XTERM-1101: PTY の出力（byte のまま・UTF-8 の途中で切れていてよい = xterm が次の write と繋ぐ）を頁の xterm に書く script（`w("<base64>")`）。純関数。
/// 例: `term_write_script(b"hi\r\n") == r#"w("aGkNCg==")"#`
pub fn term_write_script(bytes: &[u8]) -> String {
    format!("w(\"{}\")", crypto::base64(bytes, false))
}

/// TERM-XTERM-1101: 打ってから画面に出るまで（`TermMsg::Shown`）の 1 分ごとの中央値。`add` が 1 分ごとに 1 行を返し、呼ぶ側が stderr（= app.log）に書く。
/// 例: `if let Some(line) = latency.add(ms, Instant::now()) { eprintln!("{line}"); }`
/// → `terminal: typed → shown median 9 ms · p95 21 ms · 143 keys in the last minute`
/// 新しくした理由: 既存の速さの記録（Mirror の FOLLOW_MS）は閾値を超えた 1 回ずつの行で、中央値を持たない。
#[derive(Debug, Default)]
pub struct TermLatency {
    since: Option<Instant>,
    samples: Vec<u32>,
}

impl TermLatency {
    /// 1 つ足す。窓の最初の 1 つから 1 分経っていれば、その窓の中央値・95% 点・数の 1 行を返して窓を空にする（打たない間は何も書かない）。
    pub fn add(&mut self, ms: u32, now: Instant) -> Option<String> {
        let since = *self.since.get_or_insert(now);
        self.samples.push(ms);
        if now.duration_since(since) < Duration::from_secs(60) {
            return None;
        }
        self.since = None;
        let mut s = std::mem::take(&mut self.samples);
        s.sort_unstable();
        let p95 = s[((s.len() - 1) as f64 * 0.95).round() as usize];
        Some(format!("terminal: typed → shown median {} ms · p95 {p95} ms · {} keys in the last minute", s[(s.len() - 1) / 2], s.len()))
    }
}

impl WebSpec {
    /// 実際に開く URL（YouTube は /embed/<id>）
    pub fn target(&self) -> Result<String, String> {
        match &self.youtube {
            Some(id) => youtube_embed_url(id),
            None => Ok(self.url.clone()),
        }
    }
}

/// WebView の面に起きた事（wry の handler から channel で届く・WebKit に依らない形）。
#[derive(Debug, Clone, PartialEq)]
pub enum WebEvent {
    Started(String),
    Finished(String),
    Title(String),
    /// navigation を止めた（sub-frame かもしれない）
    Blocked { url: String, handoff: bool, why: String },
    /// 新しい窓の要求（deny して handoff）
    NewWindow(String),
    /// download を止めた
    Download(String),
    /// W12W-1: download を `WebSpec::download_dir` の中の `staged` に落とし始めた（wry の download_started で path を差し替えて許した）。
    DownloadStaged { url: String, staged: PathBuf },
    /// W12W-1: download が終わった（wry の download_completed）。macOS では wry が path を渡さない（`wry-0.57.0/src/lib.rs:1376`）ので、
    /// file は前の `DownloadStaged` の `staged`。`ok` = WebKit が成功と言った（disk の中身は呼ぶ側が読んで確かめる）。
    DownloadDone { url: String, ok: bool },
    ProcessGone,
    /// W12W-1b: WKWebView の `isLoading` と `estimatedProgress` を `WebSurface::frame` が毎 frame 読んで送る（wry は didStartProvisional も didFail も渡さない）。
    /// 例: `life.on(gen, WebEvent::Progress { loading: wk.isLoading(), progress: wk.estimatedProgress() }, now)`。
    /// 判定（`WebLife` の本体）: navigation の開始 = `Starting` の間に `progress > WEB_PENDING_PROGRESS`（0.1。load を頼んだ直後、WebContent の起動の前から
    /// WebKit は `loading` = true・`progress` = 0.1 を返すので、どちらも開始と数えない）。止まり = `loading` が true → false で、`Finished` も題名も無いまま
    /// `WEB_STOP_GRACE`（`Starting` でも `Loading` でも）。`progress` は finish の後も 1.0 のまま。
    Progress { loading: bool, progress: f64 },
}

/// W12W-1b: `SHIBUBU_WEB_FAULT`（`WEB_FAULT_ENV`）の値。試験の側の BG の run が、owner の network に触らずに 1 回だけ `Recoverable` を起こす。
/// 例: `open -g --env SHIBUBU_WEB_FAULT=start-once …` → Diagram の Page が「起動の上限」の `Recoverable` → 「もう一度」か Page の行き来で `Live`。
/// 保証する: generation 0 だけ（retry の後は本物の上限）・理由の 1 行は本物の `WEB_START_LIMIT` の物と同じ。保証しない: 他の失敗（止まり・process の終了）を起こす事。
/// Wave: W12W-1b
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebFault {
    /// generation 0 の最初の `tick` で、`WEB_START_LIMIT` を過ぎた時と同じ `Recoverable` にする
    StartOnce,
}

impl WebFault {
    /// `SHIBUBU_WEB_FAULT` を読む（`start-once` だけ Some・他の値と無しは None）。`WebSurface::open` が 1 回呼んで `WebLife::fault` に入れる。
    pub fn from_env() -> Option<WebFault> {
        (std::env::var(WEB_FAULT_ENV).ok().as_deref() == Some("start-once")).then_some(WebFault::StartOnce)
    }
}

/// W12W-1: WebView が一時の dir に落とした file 1 つ（`WebLife::download`）。
/// 例: `if let Some(WebDownload { staged, done: Some(true), .. }) = life.download.take() { project::web::save_download(&staged, repo, &name)? }`。
/// 保証する: `staged` は `WebSpec::download_dir` の中の絶対 path。保証しない: `done == Some(true)` でも file の中身が正しい事（読むのは呼ぶ側と試験の側）。
#[derive(Debug, Clone, PartialEq)]
pub struct WebDownload {
    pub url: String,
    pub staged: PathBuf,
    /// None = まだ落としている・Some(ok) = 終わった
    pub done: Option<bool>,
}

/// W12W-1: download の提案の名前（repo に保存する欄の既定・一時の file の名前の元）。最後の部分だけ（`..`・`/`・空は `download`）・先頭の `.` を落とす（隠し file にしない）・
/// 末尾の「 (数字)」を落とす。wry は提案の名前を owner の ~/Downloads の file と比べて番号を足してから download_started を呼ぶ
/// （`wry-0.57.0/src/wkwebview/download.rs:57-72`）ので、落とさないと repo の名前が無関係な ~/Downloads で変わる。一時の dir の番号もここで落ちる。
/// 例: `download_name(Path::new("/Users/x/Downloads/Untitled (1).excalidraw")) == "Untitled.excalidraw"`。
// ponytail: 頁が本当に「x (2).excalidraw」と提案した名前も「x.excalidraw」になる。困ったら DownloadStaged に wry の前の提案の名前を持たせる
pub fn download_name(suggested: &Path) -> String {
    let name = suggested.file_name().and_then(|n| n.to_str()).map(|n| n.trim_start_matches('.')).filter(|n| !n.is_empty()).unwrap_or("download");
    let (stem, ext) = name.split_once('.').map_or((name, ""), |(s, _)| (s, &name[s.len()..]));
    let stem = stem
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(s, n)| !s.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(stem, |(s, _)| s);
    format!("{stem}{ext}")
}

/// W12W-1: download_started で wry に返す一時の path（`dir` の中・提案の file 名の最後の部分だけを使い、`..` や `/` は落とす・同じ名前が在れば番号を足す）。
/// 例: `.with_download_started_handler(move |url, path| match &dir { Some(d) => { *path = staged_download_path(d, path); true } None => false })`。
/// 保証する: 戻り値は `dir` の直下（`dir` の外を指さない）。保証しない: 書けるか（disk の空き・権限は WebKit が `DownloadDone{ok:false}` で言う）。
pub fn staged_download_path(dir: &Path, suggested: &Path) -> PathBuf {
    let name = download_name(suggested);
    let name = name.as_str();
    let (stem, ext) = name.split_once('.').map(|(s, e)| (s, format!(".{e}"))).unwrap_or((name, String::new()));
    let mut path = dir.join(name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// WebView の面の状態（`ui::portal_frame` の 9 状態へは呼ぶ側が写す: Starting・Loading = 接続中・Live = 直接操作中・Recoverable = 一体化に失敗）。
/// W12W-1b（owner 13 通目）: 終端の状態は無い。`Recoverable` も `WebLife::retry` で `Starting` に戻る（前の `HandedOff`・`Gone` はここに入った）。
/// 例: `match views.state(&key) { Some(WebState::Recoverable(why)) => { 理由 1 行・「もう一度」→ views.retry(&key)・「browser で開く」 } … }`。
/// Wave: W12W-1b
#[derive(Debug, Clone, PartialEq)]
pub enum WebState {
    /// W12W-1b: 作った・retry した・1 回目の process の終了〜navigation の開始（`Progress` の `progress > WEB_PENDING_PROGRESS`・wry の `Started` / `Finished`・題名）。上限は `WEB_START_LIMIT`
    Starting,
    /// navigation の開始の後〜使える（`Finished` か題名）。上限は `WEB_LOAD_LIMIT`・`WEB_FINISH_LIMIT`（navigation の開始から）
    Loading,
    Live,
    /// W12W-1b: 読めなかった・本人の browser へ渡すべき（理由 1 行）。上限を過ぎた・止まった・2 回目の process の終了・新しい窓・sign-in・YouTube の 153・
    /// 開けなかった。同じ generation の event では抜けない（遅れて着いた `Finished` で戻らない）・`retry` でだけ `Starting` へ
    Recoverable(String),
}

/// WebView の面の判定（純関数の状態機械・図 §18 の WebView の段）。時計は引数の `now` で注入する（試験は `Instant` を足して進める）。
/// 保証する: event は generation ごと（`on` は今の generation の物だけを読む）・`Recoverable` は終端でない。保証しない: `Live` が頁が使える事（自己申告・読み戻しは試験の側）。
/// Wave: W12W-1b
#[derive(Debug, Clone)]
pub struct WebLife {
    pub state: WebState,
    pub youtube: bool,
    pub title: String,
    /// 帯に出す事実 1 行（止めた頁など）
    pub detail: Option<String>,
    /// 「browser で開く」の URL（新しい窓・sign-in はその URL・他は開いていた URL）。`Recoverable` になった時に入る
    pub handoff_url: Option<String>,
    /// 今の URL（最後に読み込みが始まった・終わった物）。retry はこの URL を読み直す
    pub url: String,
    /// WebContent の process の終了の数（retry で 0 に戻さない: 自動の読み直しは 1 回目だけ・2 回目からは `Recoverable` で人が「もう一度」）
    pub crashes: u32,
    /// 1 回目の process の終了で retry を頼む（`WebSurface::frame` が `WebSurface::retry` を呼んで false に戻す）。その時 state は `Starting`（同じ generation・時計の起点は終了の時）
    pub want_reload: bool,
    /// W12W-1: 最後の download（`DownloadStaged` で Some・`DownloadDone` で `done` が埋まる）。呼ぶ側が取り出したら None に戻す。
    pub download: Option<WebDownload>,
    /// W12W-1b: 試験の側が起こす失敗（`WebFault::from_env`）。None = 本物の上限だけ
    pub fault: Option<WebFault>,
    generation: u64,
    /// 上限の時計の起点（作った時・retry した時・1 回目の process の終了の時）
    opened_at: Instant,
    /// navigation の開始（`Progress` の 0.1 より上・`Started`・`Finished`・題名の最初）。`WEB_LOAD_LIMIT` / `WEB_FINISH_LIMIT` の起点
    nav_at: Option<Instant>,
    /// wry の `Started`（= didCommitNavigation）が来た = 頁が届いた
    committed: bool,
    /// この generation で `Progress` の `loading` が true を見た（止まりは true → false でだけ数える）
    loading_seen: bool,
    /// `loading` が true → false になった時（また true になれば None）
    stopped_at: Option<Instant>,
    finished_at: Option<Instant>,
    first_title_at: Option<Instant>,
}

impl WebLife {
    /// 作った時（`Starting`・generation 0）。`now` が `WEB_START_LIMIT` の起点。
    pub fn new(url: &str, youtube: bool, now: Instant) -> WebLife {
        WebLife {
            state: WebState::Starting,
            youtube,
            title: String::new(),
            detail: None,
            handoff_url: None,
            url: url.to_string(),
            crashes: 0,
            want_reload: false,
            download: None,
            fault: None,
            generation: 0,
            opened_at: now,
            nav_at: None,
            committed: false,
            loading_seen: false,
            stopped_at: None,
            finished_at: None,
            first_title_at: None,
        }
    }

    /// W12W-1b: 今の generation（作った時 0・`retry` ごとに 1 つ進む）。`WebSurface` は wry の handler が送る event にこの値を付け、`on` に渡す。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// W12W-1b: 読み直す（「もう一度」・Recoverable の面の Page に戻った時・1 回目の process の終了）。どの状態からでも呼べる（いつ呼ぶかは呼ぶ側が決める）。
    /// 例: `let gen = life.retry(now); gen_counter.store(gen, SeqCst); view.load_url(&life.url)`（`reload()` は何も読んでいない WKWebView では何もしない）。
    /// 保証する: generation を 1 つ進めて返す・`Starting` に戻す・上限の時計の起点を `now` にする（前の generation の開始・finished・題名の時刻は捨てる）・
    /// `crashes` は残す。保証しない: 読めるようになる事（また上限を過ぎれば `Recoverable`）。
    pub fn retry(&mut self, now: Instant) -> u64 {
        self.restart(now, self.generation + 1);
        self.generation
    }

    /// `Starting` に戻す（url・youtube・crashes・download・fault は残す・時計の起点は `now`）
    fn restart(&mut self, now: Instant, generation: u64) {
        let (url, youtube, crashes, download, fault) = (std::mem::take(&mut self.url), self.youtube, self.crashes, self.download.take(), self.fault);
        *self = WebLife { crashes, download, fault, generation, ..WebLife::new(&url, youtube, now) };
    }

    /// navigation が始まった（最初の 1 回の時刻を起点にする・`Starting` なら `Loading` へ）
    fn navigation_started(&mut self, now: Instant) {
        self.nav_at.get_or_insert(now);
        if self.state == WebState::Starting {
            self.state = WebState::Loading;
        }
    }

    fn recoverable(&mut self, why: String, url: Option<String>) {
        if !matches!(self.state, WebState::Recoverable(_)) {
            self.handoff_url = Some(url.unwrap_or_else(|| self.url.clone()));
            self.state = WebState::Recoverable(why);
        }
    }

    /// event 1 つ（`generation` = その event を送った時の `WebLife::generation`）。前の generation の event は捨てる（retry の前の読み込みの `Finished` が新しい状態を上書きしない）。
    /// 例: `while let Ok((gen, ev)) = rx.try_recv() { life.on(gen, ev, now) }`。
    pub fn on(&mut self, generation: u64, ev: WebEvent, now: Instant) {
        if generation != self.generation || matches!(self.state, WebState::Recoverable(_)) {
            return;
        }
        match ev {
            WebEvent::Started(u) => {
                self.url = u;
                self.committed = true;
                self.navigation_started(now);
            }
            WebEvent::Finished(u) => {
                self.url = u;
                self.nav_at.get_or_insert(now);
                if self.finished_at.is_none() {
                    self.finished_at = Some(now);
                }
                self.state = WebState::Live;
            }
            WebEvent::Title(t) => {
                if self.first_title_at.is_none() && !t.trim().is_empty() {
                    self.first_title_at = Some(now);
                    self.navigation_started(now);
                }
                // 題名が出た = 頁は描けている（YouTube の「YouTube」だけは player がまだか 153 なので数えない）
                if matches!(self.state, WebState::Starting | WebState::Loading) && !t.trim().is_empty() && !(self.youtube && t.trim() == "YouTube") {
                    self.state = WebState::Live;
                }
                self.title = t;
            }
            // G90: 面ごと本人の browser へ回すのは、頁が届く前（main の読み込みが sign-in へ飛んだ）か今の頁が sign-in の時だけ。
            // 頁が届いた後の sign-in は sub-frame（YouTube が裏で読む accounts.google.com/ServiceLogin・16:15 の実測）か頁の中の link なので、その読み込みだけ止める
            WebEvent::Blocked { url, handoff: true, why } if !self.committed || is_sign_in(&self.url) => self.recoverable(why, Some(url)),
            WebEvent::Blocked { why, .. } => self.detail = Some(why),
            WebEvent::NewWindow(url) => self.recoverable("New windows open in your own browser".into(), Some(url)),
            WebEvent::Download(url) => self.detail = Some(format!("Download blocked ({})", host_of(&url).unwrap_or_default())),
            WebEvent::DownloadStaged { url, staged } => self.download = Some(WebDownload { url, staged, done: None }),
            // 今の download の終わりだけ（取り出した後・別の URL の終わりで pending を作らない）
            WebEvent::DownloadDone { url, ok } => {
                if let Some(d) = self.download.as_mut().filter(|d| d.url == url && d.done.is_none()) {
                    d.done = Some(ok);
                }
                if !ok {
                    self.detail = Some(format!("Download failed ({}). Export again", host_of(&url).unwrap_or_default()));
                }
            }
            WebEvent::ProcessGone => {
                self.crashes += 1;
                if self.crashes >= 2 {
                    self.recoverable("The content process quit twice".into(), None);
                } else {
                    // 中は空白なので Live と言わない。generation は進めない（読み直す WebSurface::frame の retry が進める）
                    self.restart(now, self.generation);
                    self.want_reload = true;
                }
            }
            WebEvent::Progress { loading, progress } => {
                if progress > WEB_PENDING_PROGRESS {
                    self.navigation_started(now);
                }
                if loading {
                    self.loading_seen = true;
                    self.stopped_at = None;
                } else if self.loading_seen {
                    self.stopped_at.get_or_insert(now);
                }
            }
        }
    }

    /// 時間で決まる事（どれも `Recoverable` へ）: navigation が始まらない（作った / retry した時から `WEB_START_LIMIT`）・始まって頁が届かない（`WEB_LOAD_LIMIT`）/
    /// 使えない（`WEB_FINISH_LIMIT`。どちらも navigation の開始から）・止まった（`WEB_STOP_GRACE`）・YouTube が 10 秒たっても「YouTube」のまま（153）・`fault`（generation 0 だけ）。
    pub fn tick(&mut self, now: Instant) {
        let start = || format!("Loading hadn't started after {} s", WEB_START_LIMIT.as_secs());
        if self.state == WebState::Starting && (now.duration_since(self.opened_at) > WEB_START_LIMIT || (self.generation == 0 && self.fault == Some(WebFault::StartOnce))) {
            return self.recoverable(start(), None);
        }
        let nothing_shown = self.finished_at.is_none() && self.first_title_at.is_none();
        if matches!(self.state, WebState::Starting | WebState::Loading) && nothing_shown && self.stopped_at.is_some_and(|t| now.duration_since(t) > WEB_STOP_GRACE) {
            return self.recoverable("Loading stopped partway".into(), None);
        }
        if let Some(nav) = self.nav_at.filter(|_| self.state == WebState::Loading) {
            let since_nav = now.duration_since(nav);
            if !self.committed && nothing_shown && since_nav > WEB_LOAD_LIMIT {
                return self.recoverable(format!("Loading started, but the page hadn't arrived after {} s", WEB_LOAD_LIMIT.as_secs()), None);
            }
            if since_nav > WEB_FINISH_LIMIT {
                return self.recoverable(format!("Loading didn't finish in {} s", WEB_FINISH_LIMIT.as_secs()), None);
            }
        }
        let anchor = match (self.finished_at, self.first_title_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if self.youtube && matches!(self.state, WebState::Starting | WebState::Loading | WebState::Live) && self.title.trim() == "YouTube" && anchor.is_some_and(|a| now.duration_since(a) > YOUTUBE_TITLE_LIMIT) {
            self.recoverable("YouTube doesn't play this embed (Error 153)".into(), None);
        }
    }
}

/// 本人の既定の browser で開く（handoff・`open <url>`。`KANGAE_OPEN_BIN` で差し替え = test）。終わりは待たない。
pub fn open_in_browser(url: &str) -> Result<(), String> {
    // http / https だけ（頁が window.open で選べる URL から file:// の app・smb:// の mount・設定を起こさせない）
    let scheme = url.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) || host_of(url).is_none() {
        return Err(format!("Can't open this URL: {url}"));
    }
    let bin = std::env::var_os("KANGAE_OPEN_BIN").filter(|b| !b.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/bin/open"));
    let mut child = Command::new(&bin).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Couldn't start {}: {e}", bin.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// W12W-2: script を走らせてよい頁（図 §18 の W12W-2 の順）。
#[cfg(test)]
mod w12w2 {
    use super::may_evaluate;
    fn hosts(h: &[&str]) -> Vec<String> {
        h.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn w12w2_may_evaluate_only_on_the_allowed_https_page() {
        let ex = hosts(&["excalidraw.com"]);
        assert_eq!(may_evaluate("https://excalidraw.com/#room=1", &ex), Ok(()));
        assert_eq!(may_evaluate("https://json.excalidraw.com/api", &ex), Ok(()), "許可の host の sub-domain");
        for url in ["about:blank", "about:srcdoc", "https://evil.example/", "http://excalidraw.com/", "https://accounts.google.com/o/oauth2/auth", "excalidraw.com", "https://notexcalidraw.com/"] {
            assert!(may_evaluate(url, &ex).is_err(), "{url} で走らせない");
        }
    }

    #[test]
    fn w12w2_may_evaluate_never_on_a_browser_surface() {
        // W12W-B の Browser（allow_hosts が空）は decide_navigation では host の制限が無いが、script は入れない（CD-25）
        assert!(super::decide_navigation("https://excalidraw.com/", &[]) == super::WebNav::Allow);
        for h in [hosts(&[]), hosts(&[""]), hosts(&["  "])] {
            let e = may_evaluate("https://excalidraw.com/", &h).expect_err("Browser の面");
            assert!(e.contains("browser"), "{e}");
        }
    }

    #[test]
    fn w12w2_shared_store_is_one_and_fixed() {
        // 値を変えると利用者の login（WebKit の website data）が消える（§3・保証する）
        assert_eq!(super::SHARED_WEB_STORE, ("shibubu", "web"));
        // WebKit に渡す 16 byte（disk の ~/Library/WebKit/<bundle id>/WebsiteDataStore/b813ce58-a550-c2d3-bd06-4d7df53634c0）を固定する。
        // seed・FNV の素数・区切りの 0 を変えると全利用者の store が移るので、ここが赤になる。
        assert_eq!(
            super::store_identifier(super::SHARED_WEB_STORE.0, super::SHARED_WEB_STORE.1),
            [0xb8, 0x13, 0xce, 0x58, 0xa5, 0x50, 0xc2, 0xd3, 0xbd, 0x06, 0x4d, 0x7d, 0xf5, 0x36, 0x34, 0xc0]
        );
    }
}

#[cfg(test)]
mod w12w3_focus {
    use super::{focus_fix, FocusFix};

    #[test]
    fn w12w3_focus_goes_back_to_egui_when_its_field_takes_focus() {
        // run-1: AX で上の欄に focus → first responder は WKWebView のまま → 打った文字が Google の検索欄へ。egui が focus を取った frame は host へ戻す
        assert_eq!(focus_fix(true, true), FocusFix::ToHost);
        // WebView が自分で取った（人の click・autofocus）: 前と同じく egui が手放す
        assert_eq!(focus_fix(true, false), FocusFix::EguiLetsGo);
        for took in [false, true] {
            assert_eq!(focus_fix(false, took), FocusFix::Keep, "first responder が WebView の外なら触らない");
        }
    }
}

/// CHAT-FREE-1101 段 2: chat の ```html の文書は CSP が AI の html より先・頁が言えるのは返事と Build now の 2 つだけ・about: の外へは移れない。
/// 負の対照（commit message・未実行）: `canvas_document` の head を body の後ろに回す → 1 つ目で赤・`canvas_message` の空の返事の判定を外す → 空白の返事で赤・
/// `canvas_may_navigate` を常に true に → https で赤。
#[cfg(test)]
mod term_xterm_tests {
    use super::{term_document, term_message, term_write_script, TermLatency, TermMsg, TermTheme, TERM_INPUT_MAX};
    use std::time::{Duration, Instant};

    fn theme() -> TermTheme {
        TermTheme { background: [0, 0, 0], foreground: [230, 230, 230], cursor: [230, 230, 230], selection: [80, 120, 200, 120], ansi: [[1, 2, 3]; 16], font_size: 12.0 }
    }

    /// 頁の口は 8 つだけ・形の外は捨てる（打鍵の上限・http(s) でない link・0 の大きさ・頁が持つ ⌘ の key）
    #[test]
    fn term_xterm_1101_the_page_speaks_only_in_its_eight_words() {
        assert_eq!(term_message(r#"{"t":"ready"}"#), Some(TermMsg::Ready));
        assert_eq!(term_message(r#"{"t":"in","d":"ls\r"}"#), Some(TermMsg::Input(b"ls\r".to_vec())));
        assert_eq!(term_message(r#"{"t":"in","d":"日本語"}"#), Some(TermMsg::Input("日本語".as_bytes().to_vec())), "IME の確定は UTF-8 のまま");
        assert_eq!(term_message(r#"{"t":"bin","d":"\u001b[M ÿ"}"#), Some(TermMsg::Input(vec![0x1b, b'[', b'M', b' ', 0xff])), "onBinary は 1 字 1 byte");
        assert_eq!(term_message(r#"{"t":"size","cols":80,"rows":24}"#), Some(TermMsg::Size { cols: 80, rows: 24 }));
        assert_eq!(term_message(r#"{"t":"open","url":"https://example.com/a"}"#), Some(TermMsg::Open("https://example.com/a".into())));
        assert_eq!(term_message(r#"{"t":"menu","x":10,"y":20.5,"sel":"abc"}"#), Some(TermMsg::Menu { x: 10.0, y: 20.5, selection: "abc".into() }));
        assert_eq!(term_message(r#"{"t":"shown","ms":12}"#), Some(TermMsg::Shown(12)));
        let cmd = eframe::egui::Modifiers { mac_cmd: true, command: true, ..Default::default() };
        assert_eq!(term_message(r#"{"t":"key","code":"KeyK","command":true}"#), Some(TermMsg::Shortcut { key: eframe::egui::Key::K, modifiers: cmd }), "⌘K は palette へ");
        assert_eq!(term_message(r#"{"t":"key","code":"Backquote","ctrl":true}"#), Some(TermMsg::Shortcut { key: eframe::egui::Key::Backtick, modifiers: eframe::egui::Modifiers::CTRL }), "⌃` は panel へ");
        assert_eq!(term_message(r#"{"t":"key","code":"Digit2","command":true}"#), Some(TermMsg::Shortcut { key: eframe::egui::Key::Num2, modifiers: cmd }));
        let huge = format!(r#"{{"t":"in","d":"{}"}}"#, "a".repeat(TERM_INPUT_MAX + 1));
        for bad in [
            r#"{"t":"size","cols":0,"rows":24}"#,
            r#"{"t":"size","cols":80,"rows":1001}"#,
            r#"{"t":"open","url":"javascript:alert(1)"}"#,
            r#"{"t":"open","url":"file:///etc/passwd"}"#,
            r#"{"t":"bin","d":"あ"}"#,
            r#"{"t":"shown","ms":-1}"#,
            r#"{"t":"run","cmd":"rm"}"#,
            r#"{"t":"in"}"#,
            r#"{"t":"key","code":"KeyZ","command":true}"#,
            r#"{"t":"key","code":"KeyC","command":true}"#,
            r#"{"t":"key","code":"KeyK"}"#,
            "ls",
            huge.as_str(),
        ] {
            assert_eq!(term_message(bad), None, "口の外の形を通した: {}", &bad[..bad.len().min(60)]);
        }
    }

    /// 出力は byte のまま base64 で頁へ（UTF-8 の途中で切れた byte も壊さない）
    #[test]
    fn term_xterm_1101_output_reaches_the_page_byte_for_byte() {
        assert_eq!(term_write_script(b"hi\r\n"), r#"w("aGkNCg==")"#);
        let cut = &"あ".as_bytes()[..2];
        let script = term_write_script(cut);
        let inner = script.trim_start_matches("w(\"").trim_end_matches("\")");
        assert_eq!(super::crypto::base64_decode(inner).as_deref(), Some(cut));
    }

    /// 頁は vendor の xterm を中に持ち、外から何も読まない（CDN・file・http の src が無い）
    #[test]
    fn term_xterm_1101_the_page_carries_xterm_and_loads_nothing_from_outside() {
        let doc = term_document(&theme());
        assert!(doc.starts_with("<!doctype html>"), "{}", &doc[..doc.len().min(80)]);
        for has in ["new Terminal(", "WebglAddon", "FitAddon", "SearchAddon", "WebLinksAddon", "Unicode11Addon", "window.ipc.postMessage", "function w("] {
            assert!(doc.contains(has), "頁に {has} が無い");
        }
        for never in ["<script src", "<link ", "cdn.", "unpkg", "jsdelivr"] {
            assert!(!doc.contains(never), "頁が外の物を読む: {never}");
        }
    }

    /// 1 分ごとに中央値の 1 行・打たない間は何も書かない
    #[test]
    fn term_xterm_1101_typed_to_shown_is_one_median_line_a_minute() {
        let t0 = Instant::now();
        let mut l = TermLatency::default();
        for (i, ms) in [5, 9, 7, 30, 8].into_iter().enumerate() {
            assert_eq!(l.add(ms, t0 + Duration::from_secs(i as u64)), None, "1 分の前に書いた");
        }
        let line = l.add(9, t0 + Duration::from_secs(61)).expect("1 分で 1 行");
        assert!(line.contains("median 8 ms") && line.contains("6 keys"), "{line}");
        assert_eq!(l.add(4, t0 + Duration::from_secs(62)), None, "書いた後は新しい窓");
    }
}

#[cfg(test)]
mod chat_canvas {
    use super::{canvas_answer_js, canvas_document, canvas_message, canvas_may_navigate, CanvasBridge, CanvasMsg, CANVAS_CSP, CANVAS_REPLY_MAX};
    use serde_json::{json, Map};

    #[test]
    fn chat_free_canvas_is_closed_and_speaks_only_two_words() {
        let page = "<!doctype html><html><body><button onclick=\"shibubu.reply('Blue')\">Blue</button></body></html>";
        let doc = canvas_document(page, CanvasBridge::Chat, &Map::new());
        assert!(doc.starts_with("<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\""), "CSP が文書の先頭に無い: {doc}");
        let (csp, ai) = (doc.find(CANVAS_CSP).expect("CSP が無い"), doc.find(page).expect("AI の html が無い"));
        assert!(csp < ai && doc.find("window.shibubu=").is_some_and(|b| b < ai), "CSP と口が AI の html より後: {doc}");
        assert!(CANVAS_CSP.starts_with("default-src 'none'") && !CANVAS_CSP.contains("http") && !CANVAS_CSP.contains('*'), "外へ出る口を許した: {CANVAS_CSP}");
        let chat = |b: &str| canvas_message(b, CanvasBridge::Chat);
        assert_eq!(chat(r#"{"reply":"  Blue  "}"#), Some(CanvasMsg::Reply("Blue".into())));
        assert_eq!(chat(r#"{"build":true}"#), Some(CanvasMsg::Build));
        let long = format!(r#"{{"reply":"{}"}}"#, "a".repeat(CANVAS_REPLY_MAX + 1));
        for bad in [r#"{"reply":"   "}"#, r#"{"open":"https://example.com"}"#, r#"{"build":"yes"}"#, r#"{"reply":3}"#, "Blue", long.as_str()] {
            assert_eq!(chat(bad), None, "2 つの口の外の形を通した: {bad}");
        }
        assert!(canvas_may_navigate("about:blank") && canvas_may_navigate("about:srcdoc"));
        for url in ["https://example.com", "http://localhost:8080", "file:///etc/hosts", "data:text/html,x", "shibubu://open"] {
            assert!(!canvas_may_navigate(url), "外へ移れた: {url}");
        }
    }

    /// AI-COMPOSE-1101: Page の部品の口は read / set / act の 3 つだけで、chat の 2 つと交わらない・前の値は head の中で閉じたまま・read の答えは番号で返る。
    /// 負の対照（未実行）: `canvas_message` の Chat の腕で `read` を読む → 1 つ目の for で赤・`<` の置き換えを外す → `</script>` の assert で赤。
    #[test]
    fn ai_compose_page_canvas_speaks_only_read_set_act() {
        for b in [r#"{"read":{"id":1,"app":"GarageBand","what":"guitar sounds"}}"#, r#"{"set":{"key":"guitar","value":"Clean"}}"#, r#"{"act":{"intent":"x {value}","value":1}}"#] {
            assert_eq!(canvas_message(b, CanvasBridge::Chat), None, "chat の頁が Page の口を使えた: {b}");
        }
        let page = |b: &str| canvas_message(b, CanvasBridge::Page);
        for b in [r#"{"reply":"Blue"}"#, r#"{"build":true}"#] {
            assert_eq!(page(b), None, "Page の部品が chat の口を使えた: {b}");
        }
        assert_eq!(
            page(r#"{"read":{"id":7,"app":" GarageBand ","what":"guitar sounds"}}"#),
            Some(CanvasMsg::Read { id: 7, app: "GarageBand".into(), what: "guitar sounds".into() })
        );
        assert_eq!(page(r#"{"set":{"key":"guitar","value":"Acoustic Guitar"}}"#), Some(CanvasMsg::Set { key: "guitar".into(), value: json!("Acoustic Guitar") }));
        assert_eq!(page(r#"{"set":{"key":"n","value":null}}"#), Some(CanvasMsg::Set { key: "n".into(), value: json!(null) }));
        assert_eq!(page(r#"{"act":{"intent":"Use {value}","value":["a"]}}"#), Some(CanvasMsg::Act { intent: "Use {value}".into(), value: json!(["a"]) }));
        let big = format!(r#"{{"set":{{"key":"k","value":"{}"}}}}"#, "a".repeat(CANVAS_REPLY_MAX));
        for bad in [r#"{"read":{"id":0,"app":"A","what":"b"}}"#, r#"{"read":{"id":1,"app":"","what":"b"}}"#, r#"{"set":{"value":1}}"#, r#"{"act":{"intent":"  "}}"#, big.as_str()] {
            assert_eq!(page(bad), None, "形の違う頼みを通した: {bad}");
        }
        let mut values = Map::new();
        values.insert("guitar".into(), json!("</script><script>alert(1)</script>"));
        let doc = canvas_document("<p>x</p>", CanvasBridge::Page, &values);
        assert!(doc.contains("shibubu=Object.freeze({values:") && doc.contains("read:function") && !doc.contains("reply:function"), "{doc}");
        assert_eq!(doc.matches("</script>").count(), 1, "値の中の </script> が head を閉じた: {doc}");
        assert!(canvas_document("<p>x</p>", CanvasBridge::Chat, &values).find("alert(1)").is_none(), "chat の頁に値が入った");
        assert_eq!(canvas_answer_js(3, &Ok(json!(["Acoustic Guitar"])), false), r#"window.__shibubuAnswer&&window.__shibubuAnswer(3,true,["Acoustic Guitar"],false)"#);
        assert_eq!(canvas_answer_js(4, &Err("<no>".into()), false), r#"window.__shibubuAnswer&&window.__shibubuAnswer(4,false,"<no>",false)"#);
        // TALK-HONEST-1101 追記 232: 開ければ読める誤りだけ頁の bridge が button を出す（読めた答えに open を付けても出さない）・押すと {open:true} = Open
        assert_eq!(canvas_answer_js(5, &Err("Looked for x".into()), true), r#"window.__shibubuAnswer&&window.__shibubuAnswer(5,false,"Looked for x",true)"#);
        assert!(canvas_answer_js(6, &Ok(json!([])), true).ends_with(",false)"), "読めた答えに button を出した");
        assert_eq!(page(r#"{"open":true}"#), Some(CanvasMsg::Open));
        assert_eq!(canvas_message(r#"{"open":true}"#, CanvasBridge::Chat), None, "chat の頁が開く手を言えた");
        assert!(doc.contains("Let Shibubu open it") && doc.contains("open:true"), "頁の bridge に開く button が無い: {doc}");
    }
}

#[cfg(test)]
mod w15_interface {
    use super::SurfaceBackend;
    #[test]
    fn w15_built_and_missing_line_signatures() {
        let _: fn(SurfaceBackend) -> bool = super::built;
        let _: fn(SurfaceBackend, &str) -> Option<String> = super::missing_line;
    }

    #[test]
    fn w15_built_follows_features_and_missing_line_only_for_absent() {
        use SurfaceBackend::*;
        assert_eq!(super::built(NativeDirect), cfg!(feature = "native_direct"));
        assert_eq!(super::built(InteractiveMirror), cfg!(feature = "mirror"));
        assert_eq!(super::built(WebView), super::WEBVIEW_BUILT);
        for b in [NativeDirect, InteractiveMirror, WebView] {
            let line = super::missing_line(b, "TextEdit");
            assert_eq!(line.is_some(), !super::built(b), "{b:?}: 入っている方式に「置けません」を出さない・無い方式には必ず出す");
            if let Some(l) = line {
                assert_eq!(l, "This version can't place TextEdit on a page. Open it outside, in TextEdit");
                assert!(!l.contains("feature") && !l.contains("com.apple"), "feature 名と bundle id を文に出さない");
            }
        }
        for b in [ExternalHandoff, GeneratedUi, CooperativePlugin, NativePortalExperimental] {
            assert!(super::built(b) && super::missing_line(b, "X").is_none(), "{b:?} は feature で切らない");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod ax_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn ax_drop_child_clears_only_its_own_view() {
        let (mine, other) = (0x10 as *mut std::ffi::c_void, 0x20 as *mut std::ffi::c_void);
        AX_CHILD.store(mine, Ordering::SeqCst);
        appkit(AppKitOp::AxDropChild(other)).unwrap();
        assert_eq!(AX_CHILD.load(Ordering::SeqCst), mine, "別の WebView の Drop で今の WebView を外さない");
        appkit(AppKitOp::AxDropChild(mine)).unwrap();
        assert!(AX_CHILD.load(Ordering::SeqCst).is_null(), "自分の Drop で外れる");
    }
}

/// FX1（FX-002）: data dir の順と既定（呼ぶ側の形・本体の前は `todo!()` で赤）。
/// 負の対照（本体の段）: HOME の枝を `.shibubu-data` に戻す → 2 本目の assert が赤・KANGAE を SHIBUBU より先に読む → 1 本目が赤。
#[cfg(test)]
mod fx1_interface {
    use super::data_dir_from;
    use std::path::PathBuf;
    #[test]
    fn fx1_data_dir_order_and_default() {
        let s = |v: &str| Some(std::ffi::OsString::from(v));
        assert_eq!(data_dir_from(s("/a"), s("/b"), s("/h")), PathBuf::from("/a"), "SHIBUBU_DATA_DIR が先");
        assert_eq!(data_dir_from(None, None, s("/h")), PathBuf::from("/h/Library/Application Support/Shibubu"), "env が無ければ launcher と同じ場所");
        assert_eq!(data_dir_from(s(""), s("/b"), s("/h")), PathBuf::from("/b"), "空の値は無いのと同じ");
        let rel = data_dir_from(s("rel-dir"), None, s("/h"));
        assert!(rel.is_absolute() && rel.ends_with("rel-dir"), "相対の env は今の cwd で絶対に: {rel:?}");
        assert!(super::data_dir().is_absolute(), "試験の build でも絶対の path");
        let env_set = std::env::var_os("SHIBUBU_DATA_DIR").is_some() || std::env::var_os("KANGAE_DATA_DIR").is_some();
        let home_lib = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library");
        assert!(env_set || !super::data_dir().starts_with(home_lib), "試験の build が利用者の本物の data dir を返した");
    }
}

#[cfg(test)]
mod rq1_settings_request_tests {
    /// RQ1: menu の `Settings…`（action の本体 = `request_settings`）を 1 回押すと、`take_settings_request` は 1 回だけ true（読むと下ろす）。
    /// 負の対照: `take_settings_request` を `get` にする（下ろさない）→ 2 回目も true = 赤。
    #[test]
    fn rq1_one_press_is_one_request() {
        assert!(!super::take_settings_request());
        super::request_settings();
        assert!(super::take_settings_request());
        assert!(!super::take_settings_request(), "同じ押しで 2 度移る");
    }
}

#[cfg(test)]
mod rq6_tests {
    use super::*;

    /// RQ6（RQ-022）: 取り消した app だけが同意から外れ、他の app の行・読めない行は残る。無い app・無い file は false で何も作らない。
    /// 負の対照: `revoke_act_consent` の filter を `true` にする（何も外さない）→ `act_consents` に TextEdit が残って赤。
    #[test]
    fn rq6_revoke_removes_only_that_app_and_leaves_the_rest() {
        let dir = std::env::temp_dir().join(format!("rq6-revoke-{}-{:?}", std::process::id(), std::thread::current().id())); // この module は複数の path で取り込まれ、同じ process の別 thread で同時に走る
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("consents.jsonl");
        assert_eq!(revoke_act_consent(&path, "com.apple.TextEdit"), Ok(false));
        assert!(!path.exists(), "無い file を作った");
        grant_act_consent(&path, "com.apple.TextEdit", 1.0).unwrap();
        grant_act_consent(&path, "com.hnc.Discord", 2.0).unwrap();
        assert_eq!(revoke_act_consent(&path, "com.apple.TextEdit"), Ok(true));
        let left = act_consents(&path);
        assert!(!left.contains("com.apple.TextEdit") && left.contains("com.hnc.Discord"), "{left:?}");
        assert_eq!(revoke_act_consent(&path, "com.apple.TextEdit"), Ok(false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod open_inplace_tests {
    use super::*;
    use serde_json::Map;

    /// OPEN-INPLACE-1101 の 2: Always allow は同意と同じ file の別の形の行（新しい store を作らない）・同じ app と手は 1 つ・app の同意には数えない・
    /// 外すとその行だけ消える・app の同意を消すとその app の Always allow も消える。label は Connections の一覧の字。
    /// 負の対照: `always_allowed` の重ねない処理を外す → 一覧の assert が赤・`revoke_act_consent` が Always allow の行を残す → 最後の assert が赤。
    #[test]
    fn open_inplace_always_allow_lives_beside_the_consent_and_can_be_removed() {
        let dir = std::env::temp_dir().join(format!("open-inplace-always-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("consents.jsonl");
        let labels = || always_allowed(&path).iter().map(AlwaysAllowed::label).collect::<Vec<_>>();
        assert!(labels().is_empty() && !path.exists(), "無い file を作った");
        grant_act_consent(&path, "com.example.mixer", 1.0).unwrap();
        let a = always_allow(&path, "com.example.mixer", "Mixer", "desktop.ax.click", 2.0).unwrap();
        assert_eq!((a.label(), a.bundle_id.as_str(), a.action.as_str()), ("Mixer · Click".to_owned(), "com.example.mixer", "desktop.ax.click"));
        always_allow(&path, "com.example.mixer", "Mixer", "desktop.ax.click", 3.0).unwrap();
        always_allow(&path, "com.example.mixer", "Mixer", "desktop.ax.type_text", 4.0).unwrap();
        always_allow(&path, "com.example.pad", "Pad", "desktop.ax.menu", 5.0).unwrap();
        always_allow(&path, "com.example.pad", "Pad", "desktop.key", 6.0).unwrap();
        assert_eq!(labels(), ["Mixer · Click", "Mixer · Typing", "Pad · Menu", "Pad · Keys"]);
        assert_eq!(act_consents(&path).into_iter().collect::<Vec<_>>(), ["com.example.mixer"], "Always allow の行を app の同意に数えた");
        assert_eq!(revoke_always(&path, "com.example.mixer", "desktop.ax.click"), Ok(true));
        assert_eq!(labels(), ["Mixer · Typing", "Pad · Menu", "Pad · Keys"]);
        assert!(act_consents(&path).contains("com.example.mixer"), "Always allow を外して app の同意も消した");
        assert_eq!(revoke_always(&path, "com.example.mixer", "desktop.ax.click"), Ok(false));
        assert_eq!(revoke_act_consent(&path, "com.example.mixer"), Ok(true));
        assert_eq!(labels(), ["Pad · Menu", "Pad · Keys"], "app の同意を消しても Always allow が残った");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENGINE-AUDIT W7 本体 3（engine-audit 20）: 鍵が在る時、同意の file に足しただけの行（署名の無い・別の鍵の署名）は数えない。鍵が無い時は全部の行を
    /// 署名を外して読む（鍵を消した後も読める）。署名は行の末尾の `"mac"` で、外した JSON は記録の型（deny_unknown_fields）で読める。
    /// 取り消し（`drop_consents`）は残す行を元の行のまま書き戻す（署名を落とすと、鍵の在る次の読みで残した同意が全部消える）。
    /// 負の対照: `unsealed` の HMAC の照合を外す → 鍵の在る読みに forged が入って赤・`sealed` が None でも付ける → 平文の assert が赤・
    /// `drop_consents` が署名を外した JSON を書き戻す → 取り消しの後の鍵の在る読みが空で赤。
    #[test]
    fn w7_a_line_added_without_the_key_does_not_count_once_a_key_exists() {
        let dir = std::env::temp_dir().join(format!("w7-sealed-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("consents.jsonl");
        let (key, other) = ([7u8; 32], [8u8; 32]);
        let row = |b: &str| serde_json::to_string(&ConsentRecord { v: CONSENT_SCHEMA_V, bundle_id: b.into(), granted_at_ms: 1.0 }).unwrap();
        assert_eq!(sealed(row("com.example.plain"), None), row("com.example.plain"), "鍵が無いのに署名を付けた");
        let signed = sealed(row("com.example.signed"), Some(&key));
        assert!(signed.starts_with(&row("com.example.signed")[..row("com.example.signed").len() - 1]) && signed.ends_with("\"}"), "{signed}");
        assert_eq!(unsealed(&signed, Some(&key)), Some(row("com.example.signed")), "署名を外した JSON が書いた JSON と違う");
        assert_eq!(unsealed(&signed, Some(&other)), None, "別の鍵の署名を数えた");
        assert_eq!(unsealed(&row("com.example.plain"), Some(&key)), None, "鍵が在るのに署名の無い行を数えた");
        assert_eq!(unsealed(&row("com.example.plain"), None), Some(row("com.example.plain")));
        assert_eq!(unsealed(&signed, None), Some(row("com.example.signed")), "鍵が無い時に署名の在る行を読めない");
        let always = sealed(serde_json::to_string(&AlwaysAllowed { v: CONSENT_SCHEMA_V, bundle_id: "com.example.signed".into(), app_name: "Signed".into(), action: "desktop.ax.menu".into(), granted_at_ms: 2.0 }).unwrap(), Some(&key));
        let body = [row("com.example.plain"), signed.clone(), sealed(row("com.example.forged"), Some(&other)), sealed(row("com.example.gone"), Some(&key)), always].join("\n");
        std::fs::write(&path, format!("{body}\n")).unwrap();
        let read = |key: Option<&[u8]>| std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(|l| unsealed(l, key)).collect::<Vec<_>>();
        let bundles = |key: Option<&[u8]>| read(key).iter().filter_map(|l| serde_json::from_str::<ConsentRecord>(l).ok()).map(|r| r.bundle_id).collect::<Vec<_>>();
        assert_eq!(bundles(Some(&key)), ["com.example.signed", "com.example.gone"], "署名の無い・別の鍵の行を数えた");
        assert_eq!(bundles(None), ["com.example.plain", "com.example.signed", "com.example.forged", "com.example.gone"]);
        let kept = read(Some(&key)).iter().filter_map(|l| serde_json::from_str::<AlwaysAllowed>(l).ok()).map(|a| a.action).collect::<Vec<_>>();
        assert_eq!(kept, ["desktop.ax.menu"], "署名した Always allow の行を読めない");
        assert_eq!(revoke_act_consent(&path, "com.example.gone"), Ok(true));
        assert_eq!(bundles(Some(&key)), ["com.example.signed"], "取り消しで残した行の署名を落とした");
        assert!(std::fs::read_to_string(&path).unwrap().lines().any(|l| l == signed), "残した行を書き換えた");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPEN-INPLACE-1101 の 1: 部品の所の承認の 3 つ（Page の頁だけ・chat の頁は言えない）と、run の今を頁へ渡す script。
    /// 負の対照: `canvas_message` の allow の腕を外す → 1 つ目が赤・bridge の `__shibubuRun` を外す → doc の assert が赤。
    #[test]
    fn open_inplace_the_part_shows_its_run_and_answers_allow_or_cancel() {
        let page = |b: &str| canvas_message(b, CanvasBridge::Page);
        assert_eq!(page(r#"{"allow":"always"}"#), Some(CanvasMsg::Allow { always: true }));
        assert_eq!(page(r#"{"allow":"once"}"#), Some(CanvasMsg::Allow { always: false }));
        assert_eq!(page(r#"{"allow":"cancel"}"#), Some(CanvasMsg::Cancel));
        for bad in [r#"{"allow":"yes"}"#, r#"{"allow":true}"#, r#"{"allow":""}"#] {
            assert_eq!(page(bad), None, "形の違う承認を通した: {bad}");
        }
        assert_eq!(canvas_message(r#"{"allow":"always"}"#, CanvasBridge::Chat), None, "chat の頁が承認できた");
        assert_eq!(canvas_run_js(Some(&PartRun::Working("Opening the list".into()))), r#"window.__shibubuRun&&window.__shibubuRun("working","Opening the list")"#);
        assert_eq!(canvas_run_js(Some(&PartRun::Approve("Press View › Library in Mixer".into(), None))), r#"window.__shibubuRun&&window.__shibubuRun("approve","Press View › Library in Mixer")"#);
        // ENGINE-AUDIT W7: 押す手だけの承認 = 毎回聞く 1 行を 3 つ目に（頁はそれがあると Always allow を描かない）
        assert_eq!(
            canvas_run_js(Some(&PartRun::Approve("Press “Delete” in Mail".into(), Some("Asks every time before pressing “Delete”".into())))),
            r#"window.__shibubuRun&&window.__shibubuRun("approve","Press “Delete” in Mail","Asks every time before pressing “Delete”")"#
        );
        assert_eq!(canvas_run_js(Some(&PartRun::Stopped("a\"b".into()))), r#"window.__shibubuRun&&window.__shibubuRun("stopped","a\"b")"#);
        assert_eq!(canvas_run_js(None), r#"window.__shibubuRun&&window.__shibubuRun(null,"")"#);
        let doc = canvas_document("<p>x</p>", CanvasBridge::Page, &Map::new());
        for want in ["__shibubuRun", "Always allow", "Allow once", "Cancel", r#"allow:"always""#, r#"allow:"once""#, r#"allow:"cancel""#, "else btn('Always allow'"] {
            assert!(doc.contains(want), "頁の bridge に {want} が無い: {doc}");
        }
        assert!(!canvas_document("<p>x</p>", CanvasBridge::Chat, &Map::new()).contains("__shibubuRun"), "chat の頁に run の枠を置いた");
    }
}
