//! DESIGN.md v2.0 の殻（frontend の骨）。
//! - `theme`: §2 の token（色・文字・間隔・geometry）と `egui::Style`。描画側は hex を書かない
//! - `shell`: View / rail / 3 段の幅 / Intent / 主領域の差し込み口 `draw_app_region`
//! - `header` `sidebar` `agent_rail`: 殻の 3 面
//! - `home` `runs` `connections`: nav の 3 画面（Project の一覧は Home と sidebar・UI-4a で Apps を消した）
//! - `states`: §11 の region 状態の文言（renderer も使う）
//! - `build_progress`: §16.4 の生成中の画面（骨格 + 状態 1 行・通らない時の理由と次の一手・rail の経路の行）
//! - `palette`: ⌘K
//! - `portal_frame`: v0.14 §7.2 の Portal の枠（9 状態・次の一手 1 つ・同期の確度）
//! - `ax`: 入力欄が外の AX（VoiceOver・cua-driver）から受ける操作の契約（W2b）
//! - `mirror_view`: Page の Mirror の矩形に元の app の窓の映像を描く（feature `mirror`・W16M-1）
//! - `renderer`: AppDefinition v3 の UiNode 木を描く（`View::App(id)` の主領域・unit R）
//! - `toast`: 知らせるだけの右上の card（NOTIFY-1101・手の要る物は chat の card・いつも同じ説明は出さない）
//! 旧 canvas（v0.4）は app.rs の `compat_canvas` が `View::CompatCanvas` の時だけ主領域に描く。

pub mod agent_rail;

/// 起動の時に main が渡す物のうち ui が読む物（ui は `#[path]` で試験の crate・kangae-portal にも入り、そこに audio と main の開く panel は無い）。
/// main は `generated::set_startup` の 1 回で渡す（その中で `STARTUP` に入る）。入っていない間（試験の crate・kangae-portal）は app と web の icon 無し
/// （呼ぶ側の Phosphor の印か頭文字）・空の Mirror の枠の `Open an app…` は取り消しと同じ（None）。
#[derive(Clone, Copy)]
pub struct Startup {
    /// TITLEBAR-ICONS / PROVIDER-ICONS: Mac の app の icon の PNG（bundle id → `audio::surface::app_icon_png`・`theme::app_icon` が裏で読む）
    pub app_icon: fn(&str) -> Option<Vec<u8>>,
    /// 同: web の icon の PNG（URL → `audio::surface::site_icon_png`・`theme::site_icon`）
    pub site_icon: fn(&str) -> Option<Vec<u8>>,
    /// MIRROR-FRAME-1101: 空の枠の `Open an app…`（`MirrorPages::pick_app`）= main の `choose_app`（macOS の開く panel で .app を選び `CFBundleIdentifier` を読む・
    /// modal = 閉じるまで frame は止まる）。取り消し = None・.app でない・読めない = Some(Err(1 行))
    #[cfg(feature = "mirror")]
    pub pick_app: fn() -> Option<Result<String, String>>,
}

/// `Startup` の置き場（入れるのは `generated::set_startup` だけ・2 回目は無視）
pub static STARTUP: std::sync::OnceLock<Startup> = std::sync::OnceLock::new();
pub mod ax;
#[allow(dead_code)] // Home の生成の行（`Build`・`BuildAction`・`show`）は FX2 から app.rs が呼ぶ。refine の型（`RefineOutcome`・`about_rows` ほか）は統合役が配線するまで（図 §24）
pub mod build_progress;
pub mod connections;
/// PV1: Home の欄の下の成果物の候補と道具の行（何も入れない・中身は project::acquire が作る）
pub mod discover;
/// W19-2 の配線: Page の Engine の 1 行（進めている・承認・済み・止めた・失敗）。Engine の型を持たない（文は main.rs の engine_page）
pub mod engine_view;
pub mod header;
pub mod home;
/// CHAT-FREE-1101: chat の答えの Markdown（見出し・表・code・チェックリスト・折りたたみ）。Home の会話と Project の会話が同じ物を通す
pub mod markdown;
/// W16M-1: Page の Mirror の矩形に映像を描く（feature `mirror` の時だけ）
#[cfg(feature = "mirror")]
pub mod mirror_view;
pub mod motion;
pub mod palette;
#[allow(dead_code)] // 統合役が app.rs から呼ぶまで（図 §20）。src/surface が main に入ったら型を寄せる
pub mod portal_frame;
pub mod renderer;
pub mod runs;
pub mod shell;
pub mod sidebar;
pub mod skeleton;
pub mod states;
/// G95: Project の下の Terminal の panel（tab・split・開閉・agent の状態の点）
pub mod terminal_panel;
pub mod theme;
/// NOTIFY-1101: 知らせるだけの右上の card（× で消える・最大 3 枚）。renderer・generated・app.rs が `post` し、app.rs が `draw`
pub mod toast;
