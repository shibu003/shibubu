//! DESIGN.md §2 の token を 1 か所に（UI-001 / UI-003 / UI-004）。
//! 描画側は hex や magic number を書かず、ここの定数・`tokens()`・`Type` から引く。
//! Light が既定、Dark は system theme に連動（`install` で両方の `egui::Style` を登録し、egui が切り替える）。

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, StrokeKind, Theme,
    ThemePreference, Vec2,
};
use std::sync::Arc;

use crate::domain::icon::Icon;

// ---------------------------------------------------------------------------
// §2.4 / §2.5 色（17 token）・UI-3 で 3 つ足し、青の 3 つ（interactive_primary・interactive_hover・on_primary）を消した（SPEC §12・決定 ④）
// ---------------------------------------------------------------------------

/// 色の token（Light と Dark で同じ field）。描く側は hex を書かず `tokens(ctx)` で今の theme の物を引く。
/// Wave: UI-3 — 足した 3 つ: `surface_press`（押している間の塗り）・`highlight`（「変わった所」の tint）・`border_input`（入力欄の 1px の枠）。
/// `focus_ring` は `text_primary` と同じ値・`interactive_primary`・`interactive_hover`・`on_primary` は field ごと消した（operator の決定 ④・sendback 36.1）。青が残るのは `working` だけ。
/// 呼ぶ例: `let t = theme::tokens(ui.ctx()); let fill = if pressed { t.surface_press } else if hovered { t.surface_3 } else { Color32::TRANSPARENT };`
/// 保証する: Light / Dark の両方で `text_primary` / `surface_press` ≥ 4.5・`border_input` / `bg_app` ≥ 3.0・`focus_ring` / `bg_app` ≥ 3.0（WCAG の相対輝度の比・UI-3 Done 1）。
/// 保証しない: `ansi` の 16 色の比（Terminal の地 `surface_2` の上の組で UI-3 は触らない）・`scrim` の値（premultiplied で、比の検査の外）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub bg_app: Color32,
    pub surface_1: Color32,
    pub surface_2: Color32,
    /// hover の塗り（button・icon button・行）。
    pub surface_3: Color32,
    /// 押している間の塗り（`surface_3` の 1 段濃い灰・owner の決定 2・SPEC §12）。button・icon button・素の button の press に使う。
    pub surface_press: Color32,
    /// 「変わった所」の tint（SPEC §8・§12。hover と press より濃いので、変わった tab を hover と読み違えない）。UI-3 では文字の選択の地（`selection.bg_fill`）に使い、tint を描くのは UI-6。
    pub highlight: Color32,
    /// 区切り線（`hairline`・`rule`）。button には使わない（UI-3）。
    pub border: Color32,
    pub border_strong: Color32,
    /// 入力欄（TextEdit・checkbox・ComboBox）の休みと hover の 1px の枠（SPEC §10「入力欄だけが枠を持つ」）。Dark は `border_strong` と同じ値。button には使わない。
    pub border_input: Color32,
    /// ink。UI-3 から primary の塗り・tab の印・行の縦棒・link・文字の cursor・focus の輪にも使う。
    pub text_primary: Color32,
    /// 事実の文字。UI-3 から primary の hover と press の塗りにも使う。
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub success: Color32,
    /// 「動いている」の色。UI-3 の後、青はこれだけ。ICONS-1101 追記 238 から `RegionState` は使わない（Running / Loading は回る印・語は `text_secondary`）=
    /// 残るのは Terminal の tab の agent の Working の点だけ。状態でない物（波形など）に使わない。
    pub working: Color32,
    /// keyboard focus の 2px の輪。UI-3 から `text_primary` と同じ値（SPEC §12 の change）。
    pub focus_ring: Color32,
    pub scrim: Color32,
    /// W10: Terminal の 16 色（SGR 30〜37 / 90〜97・`TermColor::Indexed(0..16)`）。地の色は surface_2・既定の文字は text_primary。
    /// Light は GitHub Light、Dark は GitHub Dark の terminal の色（どちらも surface_2 の上で読める組）。
    pub ansi: [Color32; 16],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Light の token。Wave: UI-3 — `surface_press`・`highlight`・`border_input` は SPEC §12 の値・`focus_ring` は `text_primary`（#202731）。
pub const LIGHT: Tokens = Tokens {
    bg_app: rgb(0xF6F7F8),
    surface_1: rgb(0xFFFFFF),
    surface_2: rgb(0xF3F5F7),
    surface_3: rgb(0xEAEDF1),
    surface_press: rgb(0xDFE3E8),
    highlight: rgb(0xCFD6DF),
    border: rgb(0xE2E6EA),
    border_strong: rgb(0xB9C1CB),
    border_input: rgb(0x8190A2),
    text_primary: rgb(0x202731),
    text_secondary: rgb(0x515D6B),
    text_tertiary: rgb(0x6C7682),
    danger: rgb(0xAB271E),
    warning: rgb(0x8A5700),
    success: rgb(0x17633A),
    working: rgb(0x2359CE),
    focus_ring: rgb(0x202731),
    // #20273180 を premultiplied で
    scrim: Color32::from_rgba_premultiplied(16, 20, 25, 128),
    ansi: [
        rgb(0x24292F), rgb(0xCF222E), rgb(0x116329), rgb(0x4D2D00), rgb(0x0969DA), rgb(0x8250DF), rgb(0x1B7C83), rgb(0x6E7781),
        rgb(0x57606A), rgb(0xA40E26), rgb(0x1A7F37), rgb(0x633C01), rgb(0x218BFF), rgb(0xA475F9), rgb(0x3192AA), rgb(0x8C959F),
    ],
};

/// Dark の token。Wave: UI-3 → THEME-DARK-1101（owner 223「darkcolorの色がダサい。もっと黒く。orcaと同じぐらい」）: 青みの黒を Orca の段の中立の黒に
/// （Orca の app.asar の `.dark` = background #0A0A0A・card #171717・muted #262626・accent #404040・字 #FAFAFA / #A1A1A1 の値だけ使う）。
/// `border_input` は `border_strong` と同じ値で、`bg_app` の上で 3:1（WCAG 1.4.11）に届く一番暗い中立の灰 #5E5E5E（spec の #404040 は 1.9:1 で欄の枠が見えない）。
/// `text_tertiary` は #8A8A8A（#737373 は `bg_app` の上で 4.2:1 で AA に届かない）。`focus_ring` は `text_primary`・状態の色と `ansi` は替えない。
pub const DARK: Tokens = Tokens {
    bg_app: rgb(0x0A0A0A),
    surface_1: rgb(0x171717),
    surface_2: rgb(0x1F1F1F),
    surface_3: rgb(0x262626),
    surface_press: rgb(0x333333),
    highlight: rgb(0x404040),
    border: rgb(0x262626),
    border_strong: rgb(0x5E5E5E),
    border_input: rgb(0x5E5E5E),
    text_primary: rgb(0xFAFAFA),
    text_secondary: rgb(0xA1A1A1),
    text_tertiary: rgb(0x8A8A8A),
    danger: rgb(0xFF9D94),
    warning: rgb(0xE7BD70),
    success: rgb(0x7DDAA5),
    working: rgb(0x91B5FF),
    focus_ring: rgb(0xFAFAFA),
    // #00000099
    scrim: Color32::from_rgba_premultiplied(0, 0, 0, 153),
    ansi: [
        rgb(0x484F58), rgb(0xFF7B72), rgb(0x3FB950), rgb(0xD29922), rgb(0x58A6FF), rgb(0xBC8CFF), rgb(0x39C5CF), rgb(0xB1BAC4),
        rgb(0x6E7681), rgb(0xFFA198), rgb(0x56D364), rgb(0xE3B341), rgb(0x79C0FF), rgb(0xD2A8FF), rgb(0x56D4DD), rgb(0xFFFFFF),
    ],
};

/// TERM-XTERM-1101: xterm.js の頁の見た目（地 = `surface_1`〈下の panel の地〉・字 = `text_primary`・16 色 = `ansi`・選び = `selection` の地・字の大きさ = `Type::Mono`）。
/// 例（`Generated::term_sync`）: `let theme = theme::term_theme(&theme::tokens(ctx)); if theme != last { view.set_theme(&theme) }`
pub fn term_theme(t: &Tokens) -> crate::surface::TermTheme {
    let rgb = |c: Color32| [c.r(), c.g(), c.b()];
    let f = t.focus_ring;
    crate::surface::TermTheme {
        background: rgb(t.surface_1),
        foreground: rgb(t.text_primary),
        cursor: rgb(t.text_primary),
        selection: [f.r(), f.g(), f.b(), 0x66],
        ansi: t.ansi.map(rgb),
        font_size: Type::Mono.size(),
    }
}

/// W10: xterm の 256 色の 1 つ（0〜15 = `t.ansi`・16〜231 = 6×6×6 の立方体・232〜255 = 灰色 24 段。xterm の 256colres.h と同じ値）。
pub fn ansi256(t: &Tokens, i: u8) -> Color32 {
    let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    match i {
        0..=15 => t.ansi[i as usize],
        16..=231 => {
            let c = i - 16;
            Color32::from_rgb(level(c / 36), level(c / 6 % 6), level(c % 6))
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            Color32::from_rgb(g, g, g)
        }
    }
}

/// 今効いている theme の token。
pub fn tokens(ctx: &egui::Context) -> Tokens {
    match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

// ---------------------------------------------------------------------------
// §2.1 文字（9 段）
// ---------------------------------------------------------------------------

/// weight 600 の face を入れる font family 名（`install` が必ず登録する。無い環境では regular を入れる）。
pub const SEMIBOLD: &str = "kangae-semibold";
/// 太字の face を入れる font family 名（page と群の題 = `Type::Title`・`Type::Group`。Latin = Helvetica Neue Bold〈face 1〉・CJK = Hiragino W6）。
/// `install` が必ず登録する（無い環境では regular を入れる）。Wave: UI-SWEEP-1101
pub const BOLD: &str = "kangae-bold";

/// 文字の段。Wave: UI-3 — SPEC §10 の 5 段（`Title` 15/20 600・`Body` 13/20・`BodyStrong` 13/20 600・`Small` 12/16・`Mono` 12/18）。
/// 古い段は消さず値を寄せた（`Display`・`HeadingLg`・`HeadingMd` → `Title`・`HeadingSm` → `BodyStrong`・`Nav` → `Body`・`Helper` → `Small`）。
/// 消すのは INTEG-UI-b の後の R-ui の片（UI-3・UI-6a の後の Refactor 段は見送った: 呼ぶ所 136〈yc/ui-5a の新しい code も古い段を使う〉・`check_ui_copy.py` の hero の判定が `Type::Display` の字面を読む・2 列が合流する前に名前を替えると merge が割れる。それまで `Title` を作る所は 0）。
/// 呼ぶ例: `theme::label(ui, Type::Small, "⌘K", t.text_secondary)`・`Type::Title.text("Runs")`。
/// 保証する: どの段も 12px 以上で、行の高さ > 文字の大きさ（`g_min_font`）。保証しない: face（Latin = Helvetica Neue・CJK = Hiragino・`fonts`。Inter は OFL で承認待ち・sendback 5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Display,
    HeadingLg,
    HeadingMd,
    HeadingSm,
    Body,
    BodyStrong,
    Nav,
    Helper,
    Mono,
    /// page の題（Connections・Settings・Runs・中身の無い Project の Page = `shell::content_opening`）。24 / 30・太字（`BOLD`）。
    /// Wave: UI-3（15 / 20 600）→ UI-SWEEP-1101（owner 196 通目の 7「見出しの Connections をもっと大きく太文字」）
    Title,
    /// shortcut・時刻・数・footer・受け取り。12 / 16（下限・これより小さい段は作らない）。Wave: UI-3
    Small,
    /// 群の題（Connections の AI providers・Settings の form の段・Home の Continue・Project の Page の heading の文）。17 / 22・太字（`BOLD`）・`Title` より小さく本文より大きい。
    /// Wave: UI-SWEEP-1101（owner 196 通目の 8「その下の題目も大きく太文字・ただし Connections よりは小さく」）
    Group,
}

impl Type {
    /// SIZES-1101 の出典: 本文 13 = HIG の Body 13・小さい字 12 / 16 = Orca `text-xs`（HIG の Callout 12）・群の題 17 / 22 = HIG の Title 2。
    pub const fn size(self) -> f32 {
        match self {
            Type::Title => 24.0,
            Type::Group => 17.0,
            Type::Display | Type::HeadingLg | Type::HeadingMd => 15.0,
            Type::HeadingSm | Type::Body | Type::BodyStrong | Type::Nav => 13.0,
            Type::Helper | Type::Small | Type::Mono => 12.0,
        }
    }
    pub const fn line_height(self) -> f32 {
        match self {
            Type::Title => 30.0,
            Type::Group => 22.0,
            Type::Display | Type::HeadingLg | Type::HeadingMd | Type::HeadingSm | Type::Body | Type::BodyStrong | Type::Nav => 20.0,
            Type::Helper | Type::Small => 16.0,
            Type::Mono => 18.0,
        }
    }
    pub fn font(self) -> FontId {
        match self {
            Type::Title | Type::Group => FontId::new(self.size(), FontFamily::Name(BOLD.into())),
            Type::Display | Type::HeadingLg | Type::HeadingMd | Type::HeadingSm | Type::BodyStrong => {
                FontId::new(self.size(), FontFamily::Name(SEMIBOLD.into()))
            }
            Type::Mono => FontId::monospace(self.size()),
            _ => FontId::proportional(self.size()),
        }
    }
    /// 色は呼び出し側が token から選ぶ（既定は text.primary）。
    pub fn text(self, s: impl Into<String>) -> RichText {
        RichText::new(s).font(self.font()).line_height(Some(self.line_height()))
    }
}

/// `ui.add(Label)` の短縮。色は token を渡す。
pub fn label(ui: &mut egui::Ui, kind: Type, text: impl Into<String>, color: Color32) -> egui::Response {
    ui.add(egui::Label::new(kind.text(text).color(color)).wrap())
}

/// 見出し（画面・節・region の題）。文字を選べる label にする = AccessKit の Click を持つ。
/// cua-driver の `elements` は操作できる行だけを並べるので、選べない label は木に在っても名前で探せない。
/// 名前は AccessKit が label の value から取る（入力欄の `labelled_by` の元にもなる）。Tab では止まらない。
pub fn heading(ui: &mut egui::Ui, kind: Type, text: impl Into<String>, color: Color32) -> egui::Response {
    ui.add(egui::Label::new(kind.text(text).color(color)).wrap().selectable(true))
}

/// 見える label の無い入力欄に AX の名前（AccessKit の label）を付ける。placeholder は名前にならない（§4.2）。
pub fn ax_name(response: egui::Response, name: &str) -> egui::Response {
    response.ctx.accesskit_node_builder(response.id, |n| n.set_label(name));
    response
}

/// 裏の thread で 1 回だけ作る texture（`key` ごと・初めて描く frame に起こし、出来るまで・作れなかった時は None = 呼ぶ側は代わりの印）。UI の thread で decode しない。
/// 例: `theme::texture_once(ctx, &format!("attach-thumb {}", p.display()), move || decode(&p))`（HOME-ATTACH-1101 の写真の縮小絵・TITLEBAR-ICONS-1101 の app の icon）
pub fn texture_once(ctx: &egui::Context, key: &str, load: impl FnOnce() -> Option<egui::ColorImage> + Send + 'static) -> Option<egui::TextureHandle> {
    use std::sync::OnceLock;
    #[derive(Clone)]
    enum Slot {
        Reading(Arc<OnceLock<Option<egui::ColorImage>>>),
        Done(Option<egui::TextureHandle>),
    }
    let id = egui::Id::new(("kangae.texture_once", key));
    let next = match ctx.data(|d| d.get_temp::<Slot>(id)) {
        Some(Slot::Done(tex)) => return tex,
        Some(Slot::Reading(slot)) => match slot.get() {
            Some(img) => Slot::Done(img.clone().map(|img| ctx.load_texture(key, img, egui::TextureOptions::LINEAR))),
            None => return None,
        },
        None => {
            let slot = Arc::new(OnceLock::new());
            let (s, c) = (slot.clone(), ctx.clone());
            // 起こせなければ Reading のまま = 代わりの印で描く
            let _ = std::thread::Builder::new().name("texture-once".into()).spawn(move || {
                let _ = s.set(load());
                c.request_repaint();
            });
            Slot::Reading(slot)
        }
    };
    let tex = match &next {
        Slot::Done(tex) => tex.clone(),
        Slot::Reading(_) => None,
    };
    ctx.data_mut(|d| d.insert_temp(id, next));
    tex
}

/// TITLEBAR-ICONS-1101 の 2・SESSIONS-LEFT-1101 追記 224: Mac の app の icon（`audio::surface::app_icon_png` を裏で読み 64 px に・bundle id ごとに 1 回）。
/// 入っていない app・読めない icon は None。
pub fn app_icon(ctx: &egui::Context, bundle_id: &str) -> Option<egui::TextureHandle> {
    let id = bundle_id.to_owned();
    texture_once(ctx, &format!("app-icon {bundle_id}"), move || {
        let png = (crate::ui::STARTUP.get()?.app_icon)(&id)?;
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?.resize(64, 64, image::imageops::FilterType::Triangle).to_rgba8();
        Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
    })
}

/// PROVIDER-ICONS-1101: 同梱の logo（`domain::icon::logo_png`）を key ごとに 1 回 texture に（64 px・`mono` は呼ぶ側が字の色で塗る）。表に無い key は None。
fn logo_texture(ctx: &egui::Context, key: &str) -> Option<(egui::TextureHandle, bool)> {
    let (png, mono) = crate::domain::icon::logo_png(key)?;
    let tex = texture_once(ctx, &format!("logo {key}"), move || {
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
        Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
    })?;
    Some((tex, mono))
}

/// PROVIDER-ICONS-1101 追記 252（owner「未知のsoftwareのiconは自動で…保存して使いまわせるように」）: web の service の icon（`audio::surface::site_icon_png` を裏で
/// 読み 64 px に・URL の host ごとに 1 回・溜めた file が在れば網に出ない）。取れない site は None（呼ぶ側は Phosphor の印）。
pub fn site_icon(ctx: &egui::Context, url: &str) -> Option<egui::TextureHandle> {
    // 同じ host の Page が何枚在っても取りに行くのは 1 回（溜める file も host ごと）
    let host = url.split_once("://").map_or(url, |(_, r)| r.split(['/', '?', '#']).next().unwrap_or(r));
    let key = format!("site-icon {host}");
    let url = url.to_owned();
    texture_once(ctx, &key, move || {
        let png = (crate::ui::STARTUP.get()?.site_icon)(&url)?;
        let img = image::load_from_memory(&png).ok()?.resize(64, 64, image::imageops::FilterType::Triangle).to_rgba8();
        Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
    })
}

/// TITLEBAR-ICONS-1101 の 2（owner 216「claudeならclaudeのアイコン…orcaを真似して」）・SESSIONS-LEFT-1101 追記 224（行の題の左の種類の icon）: 相手の種類の icon 1 つを
/// `center` に `ICON_UI` 四方で描き、描いた矩形を返す（状態の点を角に置く）。
/// PROVIDER-ICONS-1101 で出所の順を替えた = 同梱の brand の logo（`logo` = `AgentKind::logo` の key・`mono` は `color` で塗る）→ Mac の app の icon か web の service の icon
/// （`app` = bundle id・`http://` / `https://` で始まれば site の URL = 追記 252・裏で読む・読めるまでは次）→ Phosphor（`icon`）→ `name` の頭文字の丸。
/// 例: `theme::kind_icon(ui, c, k.logo, k.app, None, k.label, ink)`・Terminal = `theme::kind_icon(ui, c, None, None, Some(Icon::SquareTerminal), "Terminal", ink)`
pub fn kind_icon(ui: &egui::Ui, center: egui::Pos2, logo: Option<&str>, app: Option<&str>, icon: Option<Icon>, name: &str, color: Color32) -> egui::Rect {
    let rect = egui::Rect::from_center_size(center, Vec2::splat(ICON_UI));
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let picture = |src: &str| if src.starts_with("http://") || src.starts_with("https://") { site_icon(ui.ctx(), src) } else { app_icon(ui.ctx(), src) };
    if let Some((tex, mono)) = logo.and_then(|key| logo_texture(ui.ctx(), key)) {
        ui.painter().image(tex.id(), rect, uv, if mono { color } else { Color32::WHITE });
    } else if let Some(tex) = app.and_then(picture) {
        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
    } else if let Some(icon) = icon {
        paint_icon(ui.painter(), icon, center, ICON_UI, color, 0.0);
    } else {
        ui.painter().circle_stroke(center, ICON_UI / 2.0 - 1.0, egui::Stroke::new(1.0, color));
        let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        ui.painter().text(center, egui::Align2::CENTER_CENTER, letter, FontId::proportional(ICON_UI * 0.6), color);
    }
    rect
}

/// PROVIDER-ICONS-1101（owner 248「usageのところにもそれぞれのproviderのicon入れて。connectionも。」）: provider / agent の key（`AgentKind::cmd` か
/// `AiAccount::provider` の id = `claude`・`grok`・`kimi`・`opencode`・`cursor-agent`・`ollama`・`lm-studio`）の印 1 つを `center` に `ICON_UI` 四方で描く
/// （`domain::terminal::agent_kind` で引いて `kind_icon` = logo → Mac の app → `name` の頭文字の丸）。表に無い key は `name` の頭文字の丸。
/// 使う所（11 か所・spec の一覧）: 一番下の帯の AI の選びと使用量・Settings / Connections の account の塊と行・Used in Shibubu の内訳・chat bar の Provider・
/// panel の ＋ の menu・Settings › Terminal の agent。Terminal の tab と左の行は `kind_icon` に `AgentKind` を直に渡す。
/// 例: `theme::provider_icon(ui, c, acct.map_or("claude", |a| a.provider()), &label, ink)`
/// Wave: PROVIDER-ICONS-1101
pub fn provider_icon(ui: &egui::Ui, center: egui::Pos2, key: &str, name: &str, color: Color32) -> egui::Rect {
    let kind = crate::domain::terminal::agent_kind(key);
    kind_icon(ui, center, kind.and_then(|k| k.logo), kind.and_then(|k| k.app), None, name, color)
}

/// 1 行で切り詰める（label は通常 ellipsize・§2.1）。
pub fn truncated(ui: &egui::Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.fonts(|f| f.layout_job(job))
}

// ---------------------------------------------------------------------------
// §2.2 間隔・UI-3 で 4・8・12・16・24・32・48 の 7 段に（4 の倍数 = HIG・Orca `--spacing: .25rem`・SIZES-1101 で 6 = `SP_1_5` を足した・SPEC §10・名前は残す。`SP_5` 20・`SP_8` 40・`SP_10` 64 は消した・呼ぶ所 0 の `SP_0` は UI-3 の後の Refactor 段で消した）
// ---------------------------------------------------------------------------

pub const SP_1: f32 = 4.0;
/// SIZES-1101（owner 233・226「全体を詰める」）: 4 と 8 の間の 6（Orca `gap-1.5`）。行の間・見出しの下を 8 から 1 段詰める所だけ。
pub const SP_1_5: f32 = 6.0;
pub const SP_2: f32 = 8.0;
pub const SP_3: f32 = 12.0;
pub const SP_4: f32 = 16.0;
pub const SP_6: f32 = 24.0;
pub const SP_7: f32 = 32.0;
pub const SP_9: f32 = 48.0;

// ---------------------------------------------------------------------------
// §2.3 geometry と §1.1 / §8 の殻の寸法
// ---------------------------------------------------------------------------

/// 小さい物（行の hover の地・button・tab）の角（Orca `rounded-sm` 6）。
pub const RADIUS_SM: u8 = 6;
/// 欄と card の角（Orca `rounded-md` 8 = 一番多い角）。
pub const RADIUS_MD: u8 = 8;
/// 大きい panel・窓・palette の角（Orca `rounded-lg` 10・SIZES-1101 で 12 → 10）。
pub const RADIUS_LG: u8 = 10;
/// 枠と区切りの線 1 px（Orca の `border` 1 px・色は `Tokens::border`）。
pub const HAIRLINE: f32 = 1.0;
/// button の描く高さ 28（owner の決定 5「もっと小さくミニマムに」・SPEC §12 `button-h`・HIG Buttons の Mini 28・Orca `h-7`）。`icon_button` の辺・素の button の `interact_size.y` もこれ。Wave: UI-3
/// 呼ぶ例: `theme::button(ui, "Create", Variant::Primary)` が 28 高で描く。`ui-owner-check` の `d5_btn_size` は `= 28.0` の数字の字面を読む（式で書かない）。
pub const BUTTON_H: f32 = 28.0;
/// 32 px の帯の中の button の高さ 24（SPEC §12 `button-compact-h`・Orca `h-6`）。押せる範囲は `HIT_MIN` まで広げる。Wave: UI-3
pub const BUTTON_COMPACT_H: f32 = 24.0;
/// `BUTTON_H` の button の文字の横の inset 8（owner の決定 4「文字との感覚が内側に少なく」・SPEC の `button-inset`・Orca `px-2`）。Wave: UI-3
/// 名前は `BUTTON_PAD_X` のまま（operator の決定・sendback 36: `d5_btn_inset` がこの名前を読む。YC_WAVES の口の `BUTTON_INSET` は使わない）。
pub const BUTTON_PAD_X: f32 = 8.0;
/// compact の横 inset 6（SPEC の `button-inset-compact`・owner の数）。名前は `BUTTON_COMPACT_PAD_X` のまま（sendback 36）。Wave: UI-3
pub const BUTTON_COMPACT_PAD_X: f32 = 6.0;
/// 描く大きさが 28 より小さい物（compact の 24・token の 22・欄の中の 24 の button）の押せる範囲の下限。Wave: UI-3
/// 呼ぶ例: `let hit = rect.expand2(Vec2::new(0.0, ((HIT_MIN - rect.height()) / 2.0).max(0.0)));` を `ui.interact` に渡す（描く rect は変えない）。
/// 保証する: helper（`button*`・`icon_button`・`chip`）の interact の rect の高さ ≥ 28（UI-3 Done 3）。保証しない: helper を通らない素の egui の widget（`interact_size.y` = `BUTTON_H` が持つ）。
pub const HIT_MIN: f32 = 28.0;
/// 1 行の入力欄の高さ 32（`Send` の button もこの高さで呼ぶ・Orca `h-8`）。Wave: UI-3 — operator の決定 ②・sendback 36.2: SPEC の 30 は採らない・UI-4 の欄も 32。
pub const INPUT_H: f32 = 32.0;
/// 入力欄の角（= `RADIUS_MD`）。Wave: UI-3（描くのは UI-4 の 1 つの欄）。
pub const INPUT_RADIUS: u8 = 8;
/// 1 つの欄（title line の中央・全 View で同じ場所）の高さ（Orca `h-8`）。契約（UI-4a から）: 36 → 32 にした（owner の決定 6「38 → 32」・operator の決定 ②・sendback 36.2。
/// SPEC の 30 は採らない）。名前は `COMMAND_H` のまま・値は数の literal（`ui-owner-check` の `d1_field_h` がこの名前の literal を読む。`INPUT_H` に寄せる式にすると読めず赤）。
/// 呼ぶ側: header の欄の rect の高さと `input_margin(ui, Type::Nav, COMMAND_H)`・起動の骨格の欄の帯。保証しない: 幅（`Tier::input_w`）・`COMMAND_MAX_W`（renderer の form の幅で欄には使わない）。
/// Wave: UI-4a
pub const COMMAND_H: f32 = 32.0;
pub const COMMAND_MAX_W: f32 = 480.0;
/// 左の sidebar の NAV の行と Settings の左の一覧の行の高さ 28（Orca `h-7`・HIG の Mini の段・SIZES-1101 / 226 で 38 → 28）。
pub const NAV_ROW_H: f32 = 28.0;
/// list の 1 行の高さ 32（SPEC §12 `list-row-h`・Orca `h-8`。Runs・⌘K・Connections の一覧の行。Connections の行は中に 28 の button が入るので、SIZES-1101 の sidebar の行 28〈`h-7`〉より 1 段高い）。Wave: UI-3
pub const LIST_ROW_H: f32 = 32.0;
/// Git の欄の file の行の高さ（SPEC §12 `file-row-h`）。Wave: UI-3（描くのは Dev の Page の作り直しの Wave）。
pub const FILE_ROW_H: f32 = 28.0;
/// 左の sidebar の Project の行の高さ 28（Orca `h-7`・SIZES-1101 / 226 で 44 → 28 = 中の session・Page の行〈`BUTTON_H`〉と同じ）。
pub const APP_ROW_H: f32 = 28.0;
/// tab の**行**の高さ 32（Orca `h-8` = 帯の `STRIP_H` と同じ・SIZES-1101 で 36 → 32）。tab の文字の inset は `BUTTON_PAD_X`（8）。tab の箱は UI-5（sendback 36.3）。
pub const TAB_H: f32 = 32.0;
/// 選択中の tab の印の太さ。色は `text_primary`（ink・UI-3）。
pub const TAB_INDICATOR: f32 = 2.0;
/// 欄の中の folder の token の高さ（SPEC §12 `token-h`・chip の替わり）。Wave: UI-3（描くのは UI-7）。
pub const TOKEN_H: f32 = 22.0;
/// inline の icon 16（HIG・Orca の `size-4`）。
pub const ICON_UI: f32 = 16.0;
/// sidebar の NAV の行・`icon_button` の icon 16（SIZES-1101 で 18 → 16 = `ICON_UI` と同じ段）。
pub const ICON_NAV: f32 = 16.0;
pub const SCROLLBAR_W: f32 = 6.0;
pub const MODAL_MAX_W: f32 = 560.0;
pub const FOCUS_RING_W: f32 = 2.0;
pub const FOCUS_RING_OFFSET: f32 = 2.0;
/// 上の行（header）の高さ。FX4（FX-018・UIUX H01・DESIGN「上の行 44」）で 56 → 44。読み手は全部この名前（header の panel・起動の骨格 `skeleton::paint_shell`・
/// `w15c2_…` の試験）なので値だけを替える。保証しない: 録画の frame の見た目が前と同じ事（見せ方が変わる = owner の ok 94 通目）。
/// 契約（TITLEBAR-ICONS-1101 の 1・owner 216「赤黄緑の browser のところにおいて。その無駄なスペースを削減」）: header は macOS の題の帯そのもの
/// （窓の中身を題の帯の下まで伸ばし・題の字を隠し・帯を透明に = main.rs の `native_options`・FX4 の「native の title bar のまま」は終わり）。44 → 36（Orca の
/// `.titlebar-left { height: 36px }`）。赤黄緑は `surface::AppKitOp::TrafficLights` が帯の縦の中央へ寄せる（左端 `TRAFFIC_X`）。
/// Wave: FX4, TITLEBAR-ICONS-1101
pub const HEADER_H: f32 = 36.0;
/// TITLEBAR-ICONS-1101: 題の帯の左の赤黄緑の席の幅（Orca の `.titlebar-traffic-light-pad { width: 80px }`）。header の左端の最初の物（◧）はこの右から。
/// 全画面の間（`ViewportInfo::fullscreen`）は赤黄緑が消えるので席を取らず `SP_2`。
pub const TRAFFIC_PAD: f32 = 80.0;
/// TITLEBAR-ICONS-1101: 赤黄緑の左端の x（Orca の `trafficLightPosition: { x: 16 }`）。縦は Orca の y 12 ではなく `HEADER_H` の中央（帯の他の物と揃える）。
pub const TRAFFIC_X: f32 = 16.0;
const _: () = assert!(TAB_H <= HEADER_H && BUTTON_H <= HEADER_H && TRAFFIC_X < TRAFFIC_PAD);
pub const SIDEBAR_W: f32 = 232.0;
pub const SIDEBAR_W_MID: f32 = 216.0;
/// 畳んだ panel の帯・tab の帯の高さ 32（Orca `h-8`・owner 210・`TAB_H` と同じ）。
pub const STRIP_H: f32 = 32.0;
/// STATUSBAR-1101: 窓の一番下の細い帯（AI・使用量・↻・状態の 1 行）の高さ（Orca `h-6` 24・字は `Type::Small` = 12 の下限）。
pub const STATUS_H: f32 = 24.0;
pub const GUTTER_WIDE: f32 = 28.0;
pub const GUTTER_MID: f32 = 20.0;
pub const MAX_TEXT_COLUMN: f32 = 1080.0;
pub const DRAWER_W: f32 = 296.0;

pub fn hairline(t: &Tokens) -> Stroke {
    Stroke::new(HAIRLINE, t.border)
}

/// 入力欄の inner margin: 横 12、縦は「指定の高さ − 文字の行高」の半分（floor）。
/// Hiragino は行高が大きく、縦 8 固定だと TextEdit が 36 / 38 を超えて伸びる（§2.3 の高さを守る）。
pub fn input_margin(ui: &egui::Ui, kind: Type, height: f32) -> Margin {
    let row = ui.fonts(|f| f.row_height(&kind.font()));
    Margin::symmetric(SP_3 as i8, ((height - row) / 2.0).floor().max(0.0) as i8)
}

/// 幅いっぱいの 1px 区切り線（§3「heading + spacing、要るなら 1px divider」）。
pub fn rule(ui: &mut egui::Ui) {
    let t = tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HAIRLINE), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.border);
}

// ---------------------------------------------------------------------------
// egui::Style を token から組む
// ---------------------------------------------------------------------------

/// `fill` = `bg_fill`（checkbox の箱・scrollbar の handle が読む）・`weak` = `weak_bg_fill`（素の button と ComboBox の頭が読む）。
fn widget(fill: Color32, weak: Color32, stroke: Stroke, text: Color32) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: weak,
        bg_stroke: stroke,
        corner_radius: CornerRadius::same(RADIUS_MD),
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

fn style(t: &Tokens, dark: bool) -> egui::Style {
    let mut s = egui::Style::default();
    s.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let v = &mut s.visuals;
    v.dark_mode = dark;
    v.override_text_color = None;
    v.panel_fill = t.surface_1;
    v.window_fill = t.surface_1;
    v.extreme_bg_color = t.surface_3;
    v.faint_bg_color = t.surface_2;
    v.code_bg_color = t.surface_2;
    v.window_stroke = hairline(t);
    v.window_corner_radius = CornerRadius::same(RADIUS_LG);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MD);
    v.window_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.scrim.gamma_multiply(0.25) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 12, spread: 0, color: t.scrim.gamma_multiply(0.2) };
    // UI-3: 青を外す（決定 ④）。link・選択の線・文字の cursor は ink、選択の地は highlight
    v.hyperlink_color = t.text_primary;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.danger;
    v.selection.bg_fill = t.highlight;
    v.selection.stroke = Stroke::new(HAIRLINE, t.text_primary);
    v.text_cursor.stroke = Stroke::new(2.0, t.text_primary);
    v.striped = false;
    v.button_frame = true;
    v.window_highlight_topmost = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    // 素の button と ComboBox の頭は weak_bg_fill（休み透明・hover surface_3・press surface_press）。入力欄（TextEdit・checkbox・ComboBox）の
    // 休みと hover の枠は border_input。button も同じ bg_stroke を読むので、素の button は `native_buttons` の scope か helper で描く（D-b）
    let input = Stroke::new(HAIRLINE, t.border_input);
    v.widgets.noninteractive = widget(t.surface_1, t.surface_1, hairline(t), t.text_primary);
    v.widgets.inactive = widget(t.surface_1, Color32::TRANSPARENT, input, t.text_primary);
    v.widgets.hovered = widget(t.surface_3, t.surface_3, input, t.text_primary);
    v.widgets.active = widget(t.surface_press, t.surface_press, Stroke::NONE, t.text_primary);
    v.widgets.open = widget(t.surface_2, t.surface_2, Stroke::NONE, t.text_primary);

    let sp = &mut s.spacing;
    sp.item_spacing = Vec2::new(SP_2, SP_2);
    sp.button_padding = Vec2::new(BUTTON_PAD_X, (BUTTON_H - Type::Body.line_height()) / 2.0);
    sp.window_margin = Margin::same(SP_6 as i8);
    // SIZES-1101 / 226: menu の内側の余白 12 → 8（Orca の menu の `p-1`〜`p-2`・項目は `button_menu_item` の 28）
    sp.menu_margin = Margin::same(SP_2 as i8);
    sp.indent = SP_4;
    sp.interact_size = Vec2::new(SP_9, BUTTON_H);
    sp.icon_width = ICON_UI;
    sp.tooltip_width = 400.0;
    sp.text_edit_width = 280.0;
    sp.combo_height = 320.0;
    sp.scroll = egui::style::ScrollStyle::thin();
    sp.scroll.bar_width = SCROLLBAR_W;
    sp.scroll.dormant_handle_opacity = 0.3;
    sp.scroll.active_handle_opacity = 0.6;
    sp.scroll.interact_handle_opacity = 0.6;

    // 既定の TextStyle も 12 未満にならないよう token へ寄せる（§2.1「12 px 未満は禁止」）。
    s.text_styles = [
        (egui::TextStyle::Small, Type::Helper.font()),
        (egui::TextStyle::Body, Type::Body.font()),
        (egui::TextStyle::Button, Type::Body.font()),
        (egui::TextStyle::Heading, Type::HeadingLg.font()),
        (egui::TextStyle::Monospace, Type::Mono.font()),
    ]
    .into();
    // TEXT-SELECT-1101（owner 185 通目の 3「shibubu上にある全ての文字はselectできるように」）: label は全部 drag で選び ⌘C（Mac の app と同じ）。
    // button・tab・行（painter で字を描く）は押す物なので選ばない。label を click で押す物にした所は `.selectable(false)`（header の `Shibubu`）。
    // 選べる label は click と drag を取る = 字の上の click は下の部品へ行かない（行は字を painter で描くか、行の interact を字の後に置く）・AccessKit の Click を持つ
    s.interaction.selectable_labels = true;
    s
}

/// 幅の決まった列の 1 語（Settings・Runs の詳細の key の列）。選べる label（TEXT-SELECT-1101）・幅を超えたら … で切り hover で全文。
/// 例: `theme::column_label(ui, k, Type::Helper, t.text_tertiary, 176.0)`。前は `truncated` の galley を painter で描いていた（選べない）。
pub fn column_label(ui: &mut egui::Ui, text: &str, kind: Type, color: Color32, width: f32) {
    ui.allocate_ui_with_layout(Vec2::new(width, Type::Body.line_height()), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        ui.add(egui::Label::new(kind.text(text).color(color)).truncate());
    });
}

/// COPY-SCROLL-1101（owner 204 通目の 3「selectしてcopyしたままscrollしてscrollしながらcopyできるように」）: chat の会話の 1 つの文（折り返す・選べる）。
/// egui 0.31 の `Label` は scroll で枠の外に出た時に選びの state を通さず、選びの端が外へ出ると選びを消す（`LabelSelectionState::end_pass`）。
/// ここは見えなくても通す = 選んだまま scroll でき、見えていない所も ⌘C に入る・吹き出しをまたいで選べる（間は改行 1 つ = egui の copy）。
/// ⌘A（`chat_keys`）の間は文の後ろを選びの色で塗る。例: `theme::chat_text(ui, Type::Body.text(&turn.text).color(t.text_primary))`
pub fn chat_text(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    chat_label(ui, egui::Label::new(text).wrap()).2
}

/// CHAT-FREE-1101: `chat_text` の中身。折り返すか（表の枡・code は折り返さない）は `label` が決め、字の位置と galley も返す（`markdown` の link の当たり）。
/// 例: `let (pos, galley, r) = theme::chat_label(ui, egui::Label::new(job).wrap())`
pub fn chat_label(ui: &mut egui::Ui, label: egui::Label) -> (egui::Pos2, Arc<egui::Galley>, egui::Response) {
    let behind = ui.painter().add(egui::Shape::Noop);
    let (pos, galley, response) = label.selectable(true).layout_in_ui(ui);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), galley.text()));
    if ui.ctx().data(|d| d.get_temp::<bool>(chat_all_id())).unwrap_or(false) {
        ui.painter().set(behind, egui::epaint::RectShape::filled(response.rect, CornerRadius::same(2), ui.visuals().selection.bg_fill));
    }
    egui::text_selection::LabelSelectionState::label_text_selection(ui, &response, pos, galley.clone(), ui.visuals().text_color(), Stroke::NONE);
    (pos, galley, response)
}

fn chat_all_id() -> egui::Id {
    egui::Id::new("kangae.chat.select-all")
}

/// COPY-SCROLL-1101: chat の ⌘A（会話の全部を選ぶ・欄や Terminal に focus が在る時はそちらの ⌘A）と、その間の ⌘C（`all` = 会話の全部の文）。
/// 押す・Esc で ⌘A の選びは終わる。会話を描く前に 1 回呼ぶ（同じ frame の `chat_text` が塗る）。例: `theme::chat_keys(ui, || turns.join("\n"))`
pub fn chat_keys(ui: &mut egui::Ui, all: impl FnOnce() -> String) {
    let id = chat_all_id();
    let was = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let free = ui.memory(|m| m.focused().is_none());
    let on = if free && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
        // 引いて選んだ所は消す（2 つの選びの copy が重ならない）
        let mut s = egui::text_selection::LabelSelectionState::load(ui.ctx());
        s.clear_selection();
        s.store(ui.ctx());
        true
    } else {
        was && !ui.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape))
    };
    if on && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
        ui.ctx().copy_text(all());
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, on));
}

/// COPY-SCROLL-1101: 文を引いて選んでいる間に pointer が今の scroll の枠（`ui.clip_rect()`）の上 / 下の外へ出たら、その向きへ流す（外れた 1px ごとに 12px/秒・40〜1600）。
/// ScrollArea の中身の ui で毎 frame 呼ぶ。`active` = この枠の中で今選んでいる（chat = `selecting_text`・Terminal = grid を drag 中）。流している間は次の frame を頼む。
pub fn edge_scroll(ui: &mut egui::Ui, active: bool) {
    let clip = ui.clip_rect();
    let Some(p) = ui.input(|i| i.pointer.latest_pos()).filter(|_| active) else { return };
    let past = if p.y < clip.top() {
        clip.top() - p.y
    } else if p.y > clip.bottom() {
        clip.bottom() - p.y
    } else {
        return;
    };
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    ui.scroll_with_delta_animation(Vec2::new(0.0, (past.abs() * 12.0).clamp(40.0, 1600.0) * past.signum() * dt), egui::style::ScrollAnimation::none());
    ui.ctx().request_repaint();
}

/// COPY-SCROLL-1101: 今の scroll の枠の中で押して、文の選びを引いている最中か（`edge_scroll` の chat の `active`）。
pub fn selecting_text(ui: &egui::Ui) -> bool {
    let clip = ui.clip_rect();
    ui.input(|i| i.pointer.primary_down() && i.pointer.press_origin().is_some_and(|o| clip.contains(o))) && egui::text_selection::LabelSelectionState::load(ui.ctx()).has_selection()
}

fn fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let load = |paths: &[&str]| paths.iter().find_map(|p| std::fs::read(p).ok());
    // 日本語は Hiragino 優先。"Hiragino Sans GB" は簡体字 face で fallback のみ。
    let regular = load(&[
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ]);
    let semibold = load(&[
        "/System/Library/Fonts/ヒラギノ角ゴシック W6.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    ]);
    // nav の ⌂ ▦ ↳ と ✦ は egui 同梱の font にも Hiragino にも無く tofu（□）になるので、記号 font を最後の fallback に
    // （Apple Symbols = 矢印・幾何学図形・⌂、Zapf Dingbats = ✦。どちらも macOS 標準。無い OS では tofu のまま: test が macOS でだけ守る）。
    let symbols: Vec<(&str, Option<Vec<u8>>)> = vec![
        ("KangaeSymbols", load(&["/System/Library/Fonts/Apple Symbols.ttf", "/System/Library/Fonts/Supplemental/Apple Symbols.ttf"])),
        ("KangaeDingbats", load(&["/System/Library/Fonts/Supplemental/Zapf Dingbats.ttf", "/System/Library/Fonts/ZapfDingbats.ttf"])),
    ];
    // Latin は Helvetica Neue（face 0 = Regular・10 = Medium）を先頭に、CJK だけ Hiragino へ落とす（G33・sendback 74・75）。
    // Hiragino が先頭だと行の箱が Hiragino の 1.5em（line gap 0.5em を含む）になり、箱の中央に置いた Latin が 3pt 上に寄り・`…` が中央の三点リーダになる。
    // ponytail: face の番号は macOS 14 の HelveticaNeue.ttc の並び。並びが変わったら name table で Medium を探す
    let latin = load(&["/System/Library/Fonts/HelveticaNeue.ttc"]);
    // 後ろに落とした Hiragino の ink を Latin の baseline へ下げる: (A − H/2)_Latin − (A − H/2)_Hiragino（em）= (0.952 − 1.193/2) − (0.88 − 1.5/2) = 0.225
    // （egui は後ろの face の字を先頭の face の行の箱の中央に置く。Monospace の Hack でも 0.216 = 12pt で 0.1pt の差）
    let jp = egui::FontTweak { y_offset_factor: if latin.is_some() { 0.225 } else { 0.0 }, ..Default::default() };
    let mut semibold_family: Vec<String> = Vec::new();
    let mut bold_family: Vec<String> = Vec::new();
    if let Some(data) = regular {
        f.font_data.insert("KangaeJP".into(), egui::FontData::from_owned(data).tweak(jp).into());
        // Monospace は egui の Hack を先に、CJK だけ Hiragino へ落とす。
        f.families.entry(FontFamily::Proportional).or_default().insert(0, "KangaeJP".into());
        f.families.entry(FontFamily::Monospace).or_default().push("KangaeJP".into());
    }
    if let Some(data) = semibold {
        f.font_data.insert("KangaeJPSemibold".into(), egui::FontData::from_owned(data).tweak(jp).into());
        semibold_family.push("KangaeJPSemibold".into());
        bold_family.push("KangaeJPSemibold".into());
    }
    if let Some(data) = latin {
        // face 1 = Bold（UI-SWEEP-1101 の題）・10 = Medium
        f.font_data.insert("KangaeLatinBold".into(), egui::FontData { index: 1, ..egui::FontData::from_owned(data.clone()) }.into());
        f.font_data.insert("KangaeLatinMedium".into(), egui::FontData { index: 10, ..egui::FontData::from_owned(data.clone()) }.into());
        f.font_data.insert("KangaeLatin".into(), egui::FontData::from_owned(data).into());
        f.families.entry(FontFamily::Proportional).or_default().insert(0, "KangaeLatin".into());
        semibold_family.insert(0, "KangaeLatinMedium".into());
        bold_family.insert(0, "KangaeLatinBold".into());
    }
    for (name, data) in symbols {
        let Some(data) = data else { continue };
        f.font_data.insert(name.into(), egui::FontData::from_owned(data).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            f.families.entry(family).or_default().push(name.into());
        }
    }
    // ICONS-1101: icon の字（egui-phosphor 0.9.0 の crate に同梱の Phosphor Bold）。`paint_icon` は icon だけの family（自分の em の箱で真ん中に置く）・
    // 字の文の中の icon（menu の `•••`・Project の名前の後の caret・menu の行の check）は Proportional の 2 番目から引く（egui-phosphor の `add_to_fonts` と同じ席 =
    // Latin の次・Hiragino と egui 同梱の emoji-icon-font〈同じ私用の番号を持つ〉より前。Phosphor は私用の番号だけなので他の字は先頭の face のまま）
    f.font_data.insert(ICON_FAMILY.into(), egui_phosphor::Variant::Bold.font_data().into());
    let at = f.families.get(&FontFamily::Proportional).map_or(0, |l| l.len().min(1));
    f.families.entry(FontFamily::Proportional).or_default().insert(at, ICON_FAMILY.into());
    // Name family が無いと egui が panic するので、face が無くても必ず登録する（regular で代用）。
    let proportional = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    semibold_family.extend(proportional.iter().cloned());
    bold_family.extend(proportional);
    f.families.insert(FontFamily::Name(SEMIBOLD.into()), semibold_family);
    f.families.insert(FontFamily::Name(BOLD.into()), bold_family);
    f.families.insert(FontFamily::Name(ICON_FAMILY.into()), vec![ICON_FAMILY.into()]);
    ctx.set_fonts(f);
}

/// 起動時に 1 回。Light / Dark 両方の Style を登録し、preference（System 既定）を egui に渡す。
/// Wave: UI-3 — Style に青が無い: 文字の選択の地 = `highlight`・focus 中の入力欄の枠と選択の線 = 1px の `text_primary`・文字の cursor = 2px の `text_primary`・link = `text_primary`。
/// 素の egui の button: `weak_bg_fill` は休み透明・hover `surface_3`・press `surface_press`、`active` と `open` の線なし、`button_padding` = (`BUTTON_PAD_X`, (`BUTTON_H` − `Type::Body` の行の高さ) / 2)、`interact_size.y` = `BUTTON_H`。
/// 入力欄（TextEdit・checkbox・ComboBox）の休みと hover の枠 = `border_input`（egui は button と同じ `widgets.inactive/hovered.bg_stroke` を読むので、素の button は `native_buttons` の scope か helper に換える・旧 canvas を含む = D-b）。
/// 呼ぶ例: `theme::install(&cc.egui_ctx, ThemePreference::System)`。保証しない: 今の theme の外の色（`src/bin` の開発用の bin の直書き）。
pub fn install(ctx: &egui::Context, preference: ThemePreference) {
    fonts(ctx);
    ctx.set_style_of(Theme::Light, style(&LIGHT, false));
    ctx.set_style_of(Theme::Dark, style(&DARK, true));
    ctx.set_theme(preference);
}

// ---------------------------------------------------------------------------
// §4.1 button / chip（token だけで描く。hover・press・focus ring・disabled を 1 か所で）
// UI-SWEEP-1101（owner 196 通目の 3・4「checkagain などはボタンにして・どこを押せばいいのか分かるように」）→ 追記（owner 209 通目「settingやconnectionなどの中だけでいいよ。
// 他は元に戻して。chatbarとかappの中とか」）: 枠は `framed` の範囲（Settings・Connections・Runs の本体）の中だけ。中: primary は ink の面・ほかは 1px の `border_strong` の枠・
// 押せない button は薄い `border` の枠と text_tertiary の字。外（chat bar・Home・sidebar・Page の tab と帯・app の中・通知・menu）は 5 回目の build（50b9b91c）の線なし。
// 塗り（hover は surface_3・press は surface_press）はどこでも同じ。枠の token は `frame_of` だけが読む。図 docs/diagrams.md §37・§59。
// ---------------------------------------------------------------------------

fn framed_id() -> egui::Id {
    egui::Id::new("kangae.theme.framed")
}

/// UI-SWEEP-1101 追記（owner 209 通目）: button に枠を付ける範囲の印。Settings・Connections・Runs の描く fn の頭で立てる（他の画面は立てない）。
/// 範囲の中の `button_sized`（`button`・`button_compact`・`chip`）・`button_disabled`・`tab`・`native_buttons` の scope は 1px の枠、外は線なし。
/// 印は ctx の 1 つの値（同じ pass の中で描く popup にも効く）で、`Framed` が落ちる時（fn の終わり・途中の return）に前の値へ戻す。
/// `ui-owner-check` の `d2_btn_framed` が 3 つの file の呼び出しを、`d2_btn_no_stroke` が他の file で呼ばない事を読む。
/// 例: `let _framed = theme::framed(ui.ctx());`（`_` に捨てるとその場で閉じる = 名前を付けて持つ）
pub fn framed(ctx: &egui::Context) -> Framed {
    let was = ctx.data(|d| d.get_temp::<bool>(framed_id())).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(framed_id(), true));
    Framed { ctx: ctx.clone(), was }
}

/// `framed` の範囲（落ちると閉じる）。
#[must_use = "名前を付けて持つ（`_` に捨てるとその場で範囲が閉じる）"]
pub struct Framed {
    ctx: egui::Context,
    was: bool,
}

impl Drop for Framed {
    fn drop(&mut self) {
        let was = self.was;
        self.ctx.data_mut(|d| d.insert_temp(framed_id(), was));
    }
}

/// button の枠の線: `framed` の範囲の中だけ 1px（押せる = `border_strong`・押せない = `border`）・外は `Stroke::NONE`。
/// button の描画 fn は枠の token をここからだけ読む（本体に `.border*` を書かない = `d2_btn_no_stroke`）。
fn frame_of(ui: &egui::Ui, enabled: bool) -> Stroke {
    if !ui.ctx().data(|d| d.get_temp::<bool>(framed_id())).unwrap_or(false) {
        return Stroke::NONE;
    }
    let t = tokens(ui.ctx());
    Stroke::new(HAIRLINE, if enabled { t.border_strong } else { t.border })
}

/// button の形。Wave: UI-3 → UI-SWEEP-1101 — `Primary` だけ面（ink）で枠なし・ほかは `framed` の範囲の中だけ 1px の `border_strong` の枠（休み・hover・press とも・外は線なし）。塗り（休み / hover / press）と文字:
/// `Primary` = `text_primary` / `text_secondary` / `text_secondary`・文字 `surface_1`（1 画面に 1 つ）。`Selected` = `surface_2` / `surface_3` / `surface_press`・文字 `text_primary`。
/// `Ghost` = 透明 / `surface_3` / `surface_press`・文字 `text_primary`。`Danger` = `Ghost` と同じ塗り・文字 `danger`。押せない時 = 透明・文字 `text_tertiary`（範囲の中は枠 `border`）。
/// UI-3 の後の Refactor 段で `Secondary`（`Ghost` と同じ形）を `Ghost` に寄せて消した（生成 app の props の `"secondary"` と知らない値も `Ghost`）。
/// 呼ぶ例: `theme::button_compact(ui, &page.title, if active { Variant::Selected } else { Variant::Ghost })`。保証しない: 1 画面に primary が 1 つである事（呼ぶ側の責任・人の目 `h_primary_one`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Ghost,
    Danger,
    /// 選択中の tab と開いている toggle（Page の tab・`▤ Terminal`）。primary（1 画面に 1 つの ink）と別の形で「今ここ」を示す。休み `surface_2`・hover `surface_3`・press `surface_press`・線なし。Wave: UI-3
    Selected,
}

/// `BUTTON_H` の button。Wave: UI-3 — 28 高・横 inset `BUTTON_PAD_X`（8）・文字 `Type::Body`・線なし（形は `Variant`）。
/// 呼ぶ例: `if theme::button(ui, "Create", Variant::Primary).clicked() { … }`。保証する: 押せる範囲 ≥ `HIT_MIN`・cursor は指（Style の `interact_cursor`）。
pub fn button(ui: &mut egui::Ui, label: &str, variant: Variant) -> egui::Response {
    button_sized(ui, label, variant, None, BUTTON_H)
}

/// 32 px の帯の中の button。Wave: UI-3 — 描く高さ 24・横 inset `BUTTON_COMPACT_PAD_X`（6）・文字 `Type::Small`・押せる範囲は `HIT_MIN`（28）まで縦に広げる。
/// 呼ぶ例: `theme::button_compact(ui, "Undo", Variant::Ghost)`。
pub fn button_compact(ui: &mut egui::Ui, label: &str, variant: Variant) -> egui::Response {
    button_sized(ui, label, variant, None, BUTTON_COMPACT_H)
}

/// 描く高さから文字の段と横 inset（`height < BUTTON_H` なら compact の `Type::Small` と 6・ほかは `Type::Body` と 8）。`button_sized` と `button_disabled` が同じ式を使う。
fn button_face(height: f32) -> (FontId, f32) {
    if height < BUTTON_H { (Type::Small.font(), BUTTON_COMPACT_PAD_X) } else { (Type::Body.font(), BUTTON_PAD_X) }
}

/// `width` を渡すとその幅（sidebar の全幅 button など）・`height` は描く高さ。Wave: UI-3 → UI-SWEEP-1101 — 枠は `Variant` の doc の表（`frame_of` = `framed` の範囲の中だけ・`d2_btn_framed` が本体の `frame_of(` を読む）。
/// 塗りは `Variant` の doc の表。press = `response.is_pointer_button_down_on()`・hover = `response.hovered()`（どちらも押せる時だけ）。`height < BUTTON_H` なら compact の inset と `Type::Small`。
/// 描く rect は `height` のまま、押せる範囲（interact の rect）だけを `HIT_MIN` 四方まで広げる（id は `allocate_space` の id = 前の `allocate_exact_size` と同じ・label を id に使わない）。
/// AX の枠（accesskit の bounds）は描いた rect に戻す（VoiceOver の枠と見える button を揃え、親の面の外へはみ出させない）。
/// 塗りの式（`surface_3`・`is_pointer_button_down_on`）はこの fn の本体に字面で書く（`ui-owner-check` の `d2_btn_fill` が本体を読む・helper に括らない）。
/// 押せない ui（`add_enabled_ui(false)`）の中では `button_disabled` と同じ形（地なし・範囲の中は薄い `border` の枠・`text_tertiary`）で、egui の fade を掛けない
/// （fade は半透明の文字を明るい地より明るくし、Dev の帯の押せない `Back` が見えなくなっていた・UI-3 の BG の写真）。
/// 呼ぶ例: `theme::button_sized(ui, "Send", Variant::Primary, None, theme::INPUT_H)`。保証しない: 押せない理由の 1 行（呼ぶ側の `on_disabled_hover_text`・UI-5）。
pub fn button_sized(ui: &mut egui::Ui, label: &str, variant: Variant, width: Option<f32>, height: f32) -> egui::Response {
    let t = tokens(ui.ctx());
    let (font, pad_x) = button_face(height);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = Vec2::new(width.unwrap_or(galley.size().x + 2.0 * pad_x), height);
    let (id, slot) = ui.allocate_space(size);
    let rect = ui.layout().align_size_within_rect(size, slot);
    let hit = rect.expand2(Vec2::new(((HIT_MIN - rect.width()) / 2.0).max(0.0), ((HIT_MIN - rect.height()) / 2.0).max(0.0)));
    let response = ui.interact(hit, id, egui::Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    // widget_info が bounds を hit に書くので、この 2 行は widget_info の後に置く（前に動かすと AX の枠が hit に戻る）
    let r = egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() };
    ui.ctx().accesskit_node_builder(id, |n| n.set_bounds(r));
    if ui.is_rect_visible(rect) {
        let pressed = enabled && response.is_pointer_button_down_on();
        let hovered = enabled && response.hovered();
        point(ui, hovered);
        let (fill, text) = match variant {
            _ if !enabled => (Color32::TRANSPARENT, t.text_tertiary),
            Variant::Primary => (if pressed || hovered { t.text_secondary } else { t.text_primary }, t.surface_1),
            Variant::Selected => (if pressed { t.surface_press } else if hovered { t.surface_3 } else { t.surface_2 }, t.text_primary),
            Variant::Ghost | Variant::Danger => (
                if pressed { t.surface_press } else if hovered { t.surface_3 } else { Color32::TRANSPARENT },
                if variant == Variant::Danger { t.danger } else { t.text_primary },
            ),
        };
        // UI-SWEEP-1101 追記（owner 209）: 枠は Settings・Connections・Runs の中だけ（`frame_of`）・primary は ink の面だけ
        let frame = if enabled && variant == Variant::Primary { Stroke::NONE } else { frame_of(ui, enabled) };
        let mut painter = ui.painter().clone();
        if !enabled && ui.is_visible() {
            painter.set_fade_to_color(None);
        }
        painter.rect(rect, CornerRadius::same(RADIUS_MD), fill, frame, StrokeKind::Inside);
        focus_ring(ui, &response, rect, CornerRadius::same(RADIUS_MD));
        painter.galley(rect.center() - galley.size() / 2.0, galley, text);
    }
    response
}

/// 押せない button（前は `renderer::components` の private な `disabled_button`）。Wave: UI-3
/// 地は無く、`framed` の範囲の中だけ薄い `border` の 1px の枠（UI-SWEEP-1101: button の形は残し、押せる物の `border_strong` と区別する）、文字は `text_tertiary`・inset と文字の段は `button_sized` と同じ式（`button_face`）・`Sense::hover`（click を取らない）・AX は Button / disabled・hover で `reason`。
/// egui の `add_enabled_ui(false)` は描いた物を白へ寄せる fade をかけ `button_sized` の文字が消えるので、押せない形は token から描く（DESIGN §12.1「読めない disabled は不可」）。
/// 呼ぶ例: `theme::button_disabled(ui, "Send", theme::INPUT_H, "Type something first")`。
/// 保証しない: keyboard focus で理由を出す事（SPEC §12 の P8・押せない形の作り直しは UI-5）。
pub fn button_disabled(ui: &mut egui::Ui, label: &str, height: f32, reason: &str) -> egui::Response {
    let t = tokens(ui.ctx());
    let (font, pad_x) = button_face(height);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, t.text_tertiary);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(galley.size().x + 2.0 * pad_x, height), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, false, label));
    if ui.is_rect_visible(rect) {
        let frame = frame_of(ui, false);
        if frame != Stroke::NONE {
            ui.painter().rect_stroke(rect, CornerRadius::same(RADIUS_MD), frame, StrokeKind::Inside);
        }
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text_tertiary);
    }
    response.on_hover_text(reason)
}

/// 素の egui の button（`ui.button`・`menu_button`・`ComboBox` の頭）を、この `ui` の中だけ helper の button と同じ形にする: `framed` の範囲（Settings・Connections・Runs）の中は
/// 1px の `border_strong` の枠・外は線なし（入力欄の `border_input` を読ませない）。
/// Wave: UI-3（`flat_buttons` = 線なし）→ UI-SWEEP-1101（`framed_buttons` = どこでも枠）→ 追記（owner 209・範囲の中だけ枠・名前を `native_buttons` に）。
/// `widgets.inactive / hovered / active / open` の `bg_stroke` を `frame_of` に・塗りは Style の `weak_bg_fill`（休み透明・hover `surface_3`・press `surface_press`）のまま。
/// 呼ぶ例: `ui.scope(|ui| { theme::native_buttons(ui); ui.menu_button("Filter", |ui| { … }); });`（Runs の `Filter`・Home の `+`・旧 canvas の Add part・同意の行・web の失敗の帯）。
/// 保証しない: 入力欄の枠（TextEdit・checkbox も同じ `bg_stroke` を読むので、入力欄を含む `ui` に掛けると枠が `border_input` でなくなる）。operator の決定（sendback 36）: Style 一括でなく scope。
pub fn native_buttons(ui: &mut egui::Ui) {
    let frame = frame_of(ui, true);
    let w = &mut ui.style_mut().visuals.widgets;
    for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        v.bg_stroke = frame;
    }
}

/// 例 prompt などの chip（§2.3）。Wave: UI-3 — `button_compact(ui, label, Variant::Ghost)` と同じ形（24・inset 6・`framed` の範囲の中だけ `border_strong` の枠〈UI-SWEEP-1101〉・文字 `text_primary` = 押せる物は黒）。`CHIP_H`・`CHIP_INSET_X` は消した。
/// 呼ぶ例: `if theme::chip(ui, "Show before / after").clicked() { … }`。
pub fn chip(ui: &mut egui::Ui, label: &str) -> egui::Response {
    button_compact(ui, label, Variant::Ghost)
}

/// icon-only button（`BUTTON_H` 四方 = 28×28 = `HIT_MIN`）。`name` は accessible name と tooltip（§9「icon-only は必ず名前」）。
/// Wave: UI-3 — 線なし・休み透明・hover `surface_3`・press `surface_press`・glyph は `ICON_NAV`（SIZES-1101 で 16）。
/// UI-SWEEP-1101 でも枠を付けない（記号そのものが押せる物の形 = 字と読み違えない・header の帯と Page の帯に枠の箱が並ばない）。
/// 塗りの式は `button_sized` に委譲せずこの本体に書く（`d2_btn_fill` が本体を読む）。
/// ICONS-1101（owner 227「なんでこんな細いの選んだの？」）: 第 2 引数は `Icon`（font の記号の字・手描きの線を押せる物にする道を型で塞ぐ）。描くのは `paint_icon`（Phosphor Bold・`ICON_UI`・字の色 `text_primary`）。
/// 呼ぶ例: `theme::icon_button(ui, Icon::Undo, "Undo · ⌘Z")`
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, name: &str) -> egui::Response {
    const _: () = assert!(BUTTON_H >= HIT_MIN);
    let t = tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if ui.is_rect_visible(rect) {
        let fill = if response.is_pointer_button_down_on() { t.surface_press } else if response.hovered() { t.surface_3 } else { Color32::TRANSPARENT };
        point(ui, ui.is_enabled() && response.hovered());
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_MD), fill);
        focus_ring(ui, &response, rect, CornerRadius::same(RADIUS_MD));
        paint_icon(ui.painter(), icon, rect.center(), ICON_UI, t.text_primary, 0.0);
    }
    response.on_hover_text(name)
}

/// ICONS-1101: icon の字の family の名（Phosphor Bold の face 1 つだけ・`fonts` が入れる）。
pub const ICON_FAMILY: &str = "ShibubuIcons";

/// ICONS-1101（owner 227・230・231）: `Icon` → egui-phosphor 0.9.0 の Bold の字（Regular と同じ番号）。Icon → Phosphor の名前の表はここだけ
/// （Design System の Icons の組はこの右の名前の Bold の SVG を上げる）。`PanelRight` は `PanelLeft` と同じ字（`paint_icon` が左右に返す）。純関数。
/// 呼ぶ側の大半は `icon_button`（押せる）と `paint_icon`（押せない）。字の文の中に置く時（`menu_button` の字・button の語の後）はこの字をそのまま文に入れる
/// （`fonts` が Proportional の 2 番目にも積む・試験の素の `Context` では □）。
/// Wave: ICONS-1101
pub fn icon_glyph(icon: Icon) -> &'static str {
    use egui_phosphor::bold as b;
    match icon {
        Icon::Ellipsis => b::DOTS_THREE,
        Icon::X => b::X,
        Icon::Undo => b::ARROW_U_UP_LEFT,
        Icon::Redo => b::ARROW_U_UP_RIGHT,
        Icon::Check => b::CHECK,
        Icon::Refresh => b::ARROWS_CLOCKWISE,
        Icon::CornerDownLeft => b::ARROW_ELBOW_DOWN_LEFT,
        Icon::ArrowRight => b::ARROW_RIGHT,
        Icon::ArrowLeft => b::ARROW_LEFT,
        Icon::ChevronLeft => b::CARET_LEFT,
        Icon::ChevronRight => b::CARET_RIGHT,
        Icon::ChevronUp => b::CARET_UP,
        Icon::ChevronDown => b::CARET_DOWN,
        Icon::Plus => b::PLUS,
        Icon::Minus => b::MINUS,
        Icon::ArrowUp => b::ARROW_UP,
        Icon::ArrowDown => b::ARROW_DOWN,
        Icon::Folder => b::FOLDER,
        Icon::Sparkles => b::SPARKLE,
        Icon::Settings => b::GEAR,
        Icon::Split => b::SQUARE_SPLIT_HORIZONTAL,
        Icon::PanelLeft | Icon::PanelRight => b::SIDEBAR_SIMPLE,
        Icon::PanelBottomOpen => b::ARROW_LINE_UP,
        Icon::PanelBottomClose => b::ARROW_LINE_DOWN,
        Icon::House => b::HOUSE,
        Icon::Plug => b::PLUG,
        Icon::History => b::CLOCK_COUNTER_CLOCKWISE,
        Icon::AppWindow => b::APP_WINDOW,
        Icon::Play => b::PLAY,
        Icon::Search => b::MAGNIFYING_GLASS,
        Icon::SquareTerminal => b::TERMINAL_WINDOW,
        Icon::MessageSquare => b::CHAT_CIRCLE,
        Icon::Image => b::IMAGE,
        Icon::File => b::FILE,
        Icon::Circle => b::CIRCLE,
        Icon::Loader => b::CIRCLE_NOTCH,
        Icon::GithubLogo => b::GITHUB_LOGO,
        Icon::Globe => b::GLOBE,
        Icon::MousePointerClick => b::CURSOR_CLICK,
        Icon::Monitor => b::MONITOR,
        Icon::Accessibility => b::PERSON_ARMS_SPREAD,
        Icon::Mic => b::MICROPHONE,
    }
}

/// ICONS-1101: 押せない icon（状態の印・行の左の相手の icon・送る丸の中）を `center` を中心に `size`（既定 `ICON_UI` = 16）で `color` に描き、icon の em の矩形を返す。
/// 置くのは字の墨（ink）の中心 = `center`（字の箱の余白の偏りで真ん中からずれない）・`angle`（radian・時計回り）はその中心で回す = 回る印（追記 238 の Running / Loading・
/// 角度を作るのは `states::paint_mark` だけ）も中心がぶれない。`PanelRight` は `PanelLeft` の字を半回転（上下に対称な形なので左右に返したのと同じ）。
/// 押せる icon は `icon_button`（本体はこれを呼ぶ）。`ICON_FAMILY` がまだ無い ctx（`install` の前・試験の素の `Context`）は Proportional で描く（panic しない・字は □）。
/// 例: `theme::paint_icon(ui.painter(), Icon::SquareTerminal, row.left_center() + egui::vec2(8.0, 0.0), theme::ICON_UI, t.text_secondary, 0.0)`
/// Wave: ICONS-1101（追記 238 で `angle`）
pub fn paint_icon(painter: &egui::Painter, icon: Icon, center: egui::Pos2, size: f32, color: Color32, angle: f32) -> egui::Rect {
    let family = FontFamily::Name(ICON_FAMILY.into());
    let family = if painter.fonts(|f| f.families().contains(&family)) { family } else { FontFamily::Proportional };
    let galley = painter.layout_no_wrap(icon_glyph(icon).to_owned(), FontId::new(size, family), color);
    let angle = angle + if icon == Icon::PanelRight { std::f32::consts::PI } else { 0.0 };
    // TextShape は pos（字の箱の左上）で回る: 墨の中心 m（字の箱の中の位置）が center に来る pos = center − R(angle)·m
    let ink = galley.mesh_bounds.center().to_vec2();
    let pos = center - egui::emath::Rot2::from_angle(angle) * ink;
    let rect = egui::Rect::from_center_size(center, galley.size());
    painter.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(angle));
    rect
}

/// Page の帯の文字の tab（UI-5a・SPEC §2 Pages・ui-5/bar.md §2・sendback 36.3）。行 `TAB_H`（36）の中に描く: 地も枠も無く、選んでいない tab は `text_secondary`・`Type::Body`、
/// 選んでいる tab は `text_primary`・`Type::BodyStrong` と、行の下端の `TAB_INDICATOR`（2px）の ink（`text_primary`）の下線（青なし）。28 高（`BUTTON_H`）の箱
/// （角 `RADIUS_SM`・横 inset `BUTTON_PAD_X`）は `framed` の範囲の中だけ休みも 1px の枠（`frame_of`・UI-SWEEP-1101 → 追記 owner 209 で Page の帯は線なしに戻した）・hover で `surface_3` の塗りと字が `text_primary` に・
/// 押している間は `surface_press`。字は 14 文字を超えたら `…`。行の下の hairline は呼ぶ側が `rule` で引く（本体に線の token を書かない = `d2_btn_no_stroke`）。
/// AX は `WidgetInfo::selected(Button, true, selected, label)`（toggled が付くので macOS では AXCheckBox・BG の driver は tab を AXCheckBox の名前で押す）。
/// 呼ぶ例: `if theme::tab(ui, Some(Icon::File), &pg.title, pg.id == page.id, 0.0).clicked() && pg.id != page.id { intent = Some(Intent::Go(View::Project { .. })); }`
/// 保証しない: 状態の記号（`Diagram ▣`・写像は UI-6a）・⌘1〜⌘9（UI-5b）。
/// 契約（FX3〈FX-011〉・引数 `tint` は FX3 の Interface の段で足した）: `tint` = 変わった印の濃さ 0..=1（DESIGN §3「変化の印は青でない `highlight` の tint を 600ms で消す」）。
/// 0 より大きい時、字と下線の下に `face`（hover の箱と同じ形）を `highlight` に `tint` を掛けた不透明度で塗る（hover・press の塗りはその上）。AX の名前・大きさ・押せる範囲は変えない。
/// 濃さは呼ぶ側が `1.0 - motion::progress(ctx, 変わった時刻, motion::TINT)` で作る（`dur` を通さない = Reduce Motion でも 600ms の色の fade・Cut の試験では呼ぶ側が 0）。
/// 消す確認の間の tab は 0.0。
/// 署名（TITLEBAR-ICONS-1101 の 2'・案 A の Interface の段で足した）: `icon` = 字の左の小さな icon（`Some(Icon::File)` = Page の印・`ICON_UI` で `paint_icon`・
/// 字との間 `SP_1_5`・色は字と同じ）。None は字だけ。AX の名前は `label` のまま（icon は飾り）。帯の `More`（FX-008）は消した（Page は全部 tab・帯は横に scroll）。
/// 例: `theme::tab(&mut row, Some(Icon::File), &pg.title, pg.id == page.id, tint_of(&pg.id))`（`Generated::draw_page_band`）
/// Wave: UI-5a, FX3, TITLEBAR-ICONS-1101
pub fn tab(ui: &mut egui::Ui, icon: Option<Icon>, label: &str, selected: bool, tint: f32) -> egui::Response {
    let t = tokens(ui.ctx());
    let enabled = ui.is_enabled();
    let shown: String = if label.chars().count() > 14 { label.chars().take(14).chain(['…']).collect() } else { label.to_owned() };
    let font = if selected { Type::BodyStrong.font() } else { Type::Body.font() };
    let galley = ui.painter().layout_no_wrap(shown, font, Color32::PLACEHOLDER);
    // TITLEBAR-ICONS-1101: 字の左の icon の席（icon ＋ 字との間）
    let lead = if icon.is_some() { ICON_UI + SP_1_5 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(lead + galley.size().x + 2.0 * BUTTON_PAD_X, TAB_H), egui::Sense::click());
    // AX の名前は切り詰めない全部の名前（BG の driver は tab を名前で押す）
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label));
    if ui.is_rect_visible(rect) {
        let pressed = enabled && response.is_pointer_button_down_on();
        let hovered = enabled && response.hovered();
        let face = egui::Rect::from_center_size(rect.center(), Vec2::new(rect.width(), BUTTON_H));
        // FX3（FX-011）: 変わった印は青でない highlight の tint（hover・press の塗りはその上）
        if tint > 0.0 {
            ui.painter().rect_filled(face, CornerRadius::same(RADIUS_SM), t.highlight.gamma_multiply(tint.min(1.0)));
        }
        if pressed || hovered {
            ui.painter().rect_filled(face, CornerRadius::same(RADIUS_SM), if pressed { t.surface_press } else { t.surface_3 });
        }
        // UI-SWEEP-1101 追記（owner 209）: 枠は Settings・Connections・Runs の中の tab だけ（Page の帯は 5 回目の線なし）
        let frame = frame_of(ui, enabled);
        if frame != Stroke::NONE {
            ui.painter().rect_stroke(face, CornerRadius::same(RADIUS_SM), frame, StrokeKind::Inside);
        }
        point(ui, hovered);
        let text = if selected || hovered { t.text_primary } else { t.text_secondary };
        // icon と字をまとめて真ん中に（icon は字と同じ色）
        let x0 = rect.center().x - (lead + galley.size().x) / 2.0;
        if let Some(icon) = icon {
            paint_icon(ui.painter(), icon, egui::pos2(x0 + ICON_UI / 2.0, rect.center().y), ICON_UI, text, 0.0);
        }
        ui.painter().galley(egui::pos2(x0 + lead, rect.center().y - galley.size().y / 2.0), galley, text);
        if selected {
            let mark = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - TAB_INDICATOR), rect.right_bottom());
            ui.painter().rect_filled(mark, CornerRadius::ZERO, t.text_primary);
        }
        focus_ring(ui, &response, face, CornerRadius::same(RADIUS_SM));
    }
    response
}

/// TITLEBAR-ICONS-1101 の 1'（owner 218「hum to guiter^ってなっているけどこの^が小さすぎる。下向きの^ね」）: 1 つの menu を開く button =
/// `icon`（`Some(Icon::Folder)` = Project）・`label`（`Type::Nav`・`text_primary`）・下向きの ⌄（`Icon::ChevronDown` を `ICON_UI` で `paint_icon`・`text_secondary`・
/// 名前との間 `SP_1`）を 1 つの押せる箱に（高さ `BUTTON_H`・横 inset `BUTTON_COMPACT_PAD_X`・休みは透明・hover `surface_3`・press `surface_press`・角 `RADIUS_SM`）。
/// ⌄ を字の文に混ぜない（`{name} ⌄` の 1 つの文字列だと ⌄ が本文の大きさの小さな点に見えた = 218）。spec の「painter の線」は owner 230・231（既製の icon だけ・手描きをやめる）の後なので
/// ICONS-1101 の表の `ChevronDown`（Phosphor Bold の CARET_DOWN）で描く。AX は `Button`・名前は `label`（呼ぶ側が `ax_name` で替える = Project は `Project options`）。
/// Project と Page を見分ける形: Project = この 1 つの menu・Page = 題の帯に並んだ `tab`（⌄ を付けない）。
/// 例: `let opener = theme::ax_name(theme::dropdown(ui, Some(Icon::Folder), &shown), "Project options");`
/// 保証しない: 長い名前の切り詰め（呼ぶ側が字数で切る）。
/// Wave: TITLEBAR-ICONS-1101
pub fn dropdown(ui: &mut egui::Ui, icon: Option<Icon>, label: &str) -> egui::Response {
    let t = tokens(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), Type::Nav.font(), Color32::PLACEHOLDER);
    let lead = if icon.is_some() { ICON_UI + SP_1_5 } else { 0.0 };
    let w = 2.0 * BUTTON_COMPACT_PAD_X + lead + galley.size().x + SP_1 + ICON_UI;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, BUTTON_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let fill = if response.is_pointer_button_down_on() { t.surface_press } else if response.hovered() { t.surface_3 } else { Color32::TRANSPARENT };
        point(ui, ui.is_enabled() && response.hovered());
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_SM), fill);
        focus_ring(ui, &response, rect, CornerRadius::same(RADIUS_SM));
        let (mut x, cy) = (rect.left() + BUTTON_COMPACT_PAD_X, rect.center().y);
        if let Some(icon) = icon {
            paint_icon(ui.painter(), icon, egui::pos2(x + ICON_UI / 2.0, cy), ICON_UI, t.text_secondary, 0.0);
            x += lead;
        }
        let text_w = galley.size().x;
        ui.painter().galley(egui::pos2(x, cy - galley.size().y / 2.0), galley, t.text_primary);
        paint_icon(ui.painter(), Icon::ChevronDown, egui::pos2(x + text_w + SP_1 + ICON_UI / 2.0, cy), ICON_UI, t.text_secondary, 0.0);
    }
    response
}

/// 開いた menu の最初の押せる項目へ keyboard focus を置く印（`menu_popup` が開いた pass に立て、最初の `button_menu_item` が取る）。同時に開ける popup は 1 つなので 1 つの Id。
fn menu_focus_id() -> egui::Id {
    egui::Id::new("kangae-menu-focus-first")
}

/// popup の menu の 1 行（UI-5a・ui-5/menu.md §1）。高さ `BUTTON_H`（28 = `HIT_MIN`）・幅いっぱい・字は左寄せの `Type::Body`・右端に `hint`（鍵・空なら描かない）を `text_tertiary` で・
/// 線なし・休み透明・hover `surface_3`・press `surface_press`。形は今の `Variant` で選ぶ: `Ghost` = 普通・`Danger` = 字が `danger`（`Delete page…`）。
/// 押せない項目（`A project needs at least one page`）は呼ぶ側が `ui.add_enabled_ui(false, …)` の中で呼ぶ（`button_sized` と同じく地なし・`text_tertiary`・egui の fade を掛けない）。
/// AX は Button と `label`。popup の中で素の `ui.button(` を使わない（枠の付いた egui の button が menu に並ばない）。UI-SWEEP-1101 でも枠を付けない
/// （menu の行は popup の枠の中の行 = macOS の menu と同じ形・hover の塗りで押せる行と分かる）。
/// 呼ぶ例: `for c in &options { if theme::button_menu_item(ui, &c.label, &c.hint, Variant::Ghost).clicked() { picked = Some(c.intent.clone()); } }`
/// 保証する: 1 行の高さは 28・`hint` が空なら右端に何も描かない。保証しない: 項目の並びと押した後に閉じる事（`page_menus` と `menu_popup` が持つ）。
/// Wave: UI-5a
pub fn button_menu_item(ui: &mut egui::Ui, label: &str, hint: &str, variant: Variant) -> egui::Response {
    menu_item(ui, None, label, hint, variant)
}

/// PROVIDER-ICONS-1101（owner 248「usageのところにもそれぞれのproviderのicon入れて」）: provider / agent を選ぶ menu の 1 行 = `button_menu_item` の字の左に
/// その印（`provider_icon`・`key` = `AgentKind::cmd` か `AiAccount::provider`・字と同じ色）。AX の名前は `label` のまま。
/// 例: `theme::provider_menu_item(ui, "codex", "Codex", "Agent")`・帯の AI の選び・chat bar の Provider の dropdown・panel の ＋ の agent の行
/// Wave: PROVIDER-ICONS-1101
pub fn provider_menu_item(ui: &mut egui::Ui, key: &str, label: &str, hint: &str) -> egui::Response {
    menu_item(ui, Some(key), label, hint, Variant::Ghost)
}

/// `button_menu_item` と `provider_menu_item` の本体（`lead` = 字の左の印の provider の key）。
fn menu_item(ui: &mut egui::Ui, lead: Option<&str>, label: &str, hint: &str, variant: Variant) -> egui::Response {
    let t = tokens(ui.ctx());
    let enabled = ui.is_enabled();
    let galley = ui.painter().layout_no_wrap(label.to_owned(), Type::Body.font(), Color32::PLACEHOLDER);
    let hint_g = (!hint.is_empty()).then(|| ui.painter().layout_no_wrap(hint.to_owned(), Type::Small.font(), Color32::PLACEHOLDER));
    let hint_w = hint_g.as_ref().map_or(0.0, |g| g.size().x + SP_3);
    let lead_w = if lead.is_some() { ICON_UI + SP_2 } else { 0.0 };
    let width = ui.available_width().max(lead_w + galley.size().x + hint_w + 2.0 * BUTTON_PAD_X);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, BUTTON_H), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if enabled && ui.ctx().data_mut(|d| d.remove_temp::<bool>(menu_focus_id())).unwrap_or(false) {
        response.request_focus();
    }
    if ui.is_rect_visible(rect) {
        let pressed = enabled && response.is_pointer_button_down_on();
        let hovered = enabled && response.hovered();
        let fill = if pressed { t.surface_press } else if hovered { t.surface_3 } else { Color32::TRANSPARENT };
        point(ui, hovered);
        let text = if !enabled { t.text_tertiary } else if variant == Variant::Danger { t.danger } else { t.text_primary };
        // 押せない項目（理由の文がそのまま字）は egui の fade を掛けない（button_sized と同じ・地より明るくしない）
        let mut painter = ui.painter().clone();
        if !enabled && ui.is_visible() {
            painter.set_fade_to_color(None);
        }
        painter.rect_filled(rect, CornerRadius::same(RADIUS_SM), fill);
        focus_ring(ui, &response, rect, CornerRadius::same(RADIUS_SM));
        if let Some(key) = lead {
            provider_icon(ui, egui::pos2(rect.left() + BUTTON_PAD_X + ICON_UI / 2.0, rect.center().y), key, label, text);
        }
        painter.galley(egui::pos2(rect.left() + BUTTON_PAD_X + lead_w, rect.center().y - galley.size().y / 2.0), galley, text);
        if let Some(g) = hint_g {
            painter.galley(egui::pos2(rect.right() - BUTTON_PAD_X - g.size().x, rect.center().y - g.size().y / 2.0), g, t.text_tertiary);
        }
    }
    response
}

/// `opener`（帯の `＋`・`•••`）の下に開く menu（UI-5a・operator の決定: `egui::popup::popup_below_widget` ＋ `Memory::toggle_popup`・sendback 6 = native の menu の crate は足さない）。
/// `opener` を押すたびに開閉し、Esc・外の click・`add_contents` が `Some` を返した時に閉じる。開いた frame に最初の項目へ keyboard focus を置き、閉じたら focus を `opener` へ戻す。
/// `Memory::popup` を使うので、開いている間 `Memory::any_popup_open()` が真 = app.rs の「popup の間は WebView を隠す」規則（cover）に入る
/// （`ui.menu_button` と `Response::context_menu` はそこに入らず、Diagram の Excalidraw の下に menu が隠れる・menu.md §0）。同時に開ける menu は 1 つ。
/// 呼ぶ例: `let more = theme::icon_button(ui, Icon::Ellipsis, "Page options"); if let Some(i) = theme::menu_popup(ui, &more, |ui| { … }) { intent = Some(i); }`
/// 保証しない: submenu（popup の中の popup は親を閉じる）・右端の opener から右へはみ出さない位置（本体で決める）。
/// Wave: UI-5a
pub fn menu_popup<R>(ui: &mut egui::Ui, opener: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui) -> Option<R>) -> Option<R> {
    const MENU_W: f32 = 240.0;
    let id = opener.id.with("kangae-menu");
    if opener.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
        if ui.memory(|m| m.is_popup_open(id)) {
            ui.ctx().data_mut(|d| d.insert_temp(menu_focus_id(), true));
        }
    }
    let was_open = ui.memory(|m| m.is_popup_open(id));
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // popup_below_widget は渡した rect の左下から右へ開き、幅はその rect 以上。右端の ••• から右へはみ出さないよう、幅 MENU_W の rect を opener の右端に揃えて渡す
    // （開閉の判定〈外の click〉は opener の hover と interact_rect のまま = clone の rect だけを替える）
    let mut anchor = opener.clone();
    anchor.rect = egui::Rect::from_min_max(egui::pos2(opener.rect.right() - MENU_W, opener.rect.top()), opener.rect.right_bottom());
    let picked = egui::popup::popup_below_widget(ui, id, &anchor, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        add_contents(ui)
    })
    .flatten();
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
    }
    if was_open && (picked.is_some() || esc) {
        opener.request_focus();
    }
    picked
}

/// 同じ場所の 1 行の消す確認（UI-5a・owner の答え ③「今のまま」・SPEC §5.13・sendback 11）。`question`・`Delete`（danger の字・塗りなし）・`Keep` の順に横へ描く。
/// 開いた frame に `Keep` へ keyboard focus を置く（Enter は focus の在る `Keep` を押す = 消さない）。`Delete` と `Keep` の語はこの本体の literal
/// （`ui-owner-check` の `a3_confirm_inline` が同じ block の 2 つの button を数える・引数で渡さない）。2 つ目の語は `Keep`（operator の決定）。
/// 返り値: `Some(true)` = `Delete` を押した・`Some(false)` = `Keep` を押した・`None` = どちらも押していない。Esc は呼ぶ側（app.rs の Esc の連鎖）が閉じる。
/// 呼ぶ例: `match theme::confirm_inline(ui, &format!("Delete “{}”? ⌘Z brings it back.", page.title)) { Some(true) => intent = Some(Intent::Page(PageIntent::Delete { page: page.id.clone() })), Some(false) => self.pending_delete = None, None => {} }`
/// 使う所: Page の tab の横（`Generated::draw_page`）・旧 canvas の canvas と部品の削除（app.rs の `delete_prompts` の 2 つの `Window` を置き換える・G14 B8）。保証しない: 消した後の受け取りの 1 行（UI-6a）。
/// Wave: UI-5a
pub fn confirm_inline(ui: &mut egui::Ui, question: &str) -> Option<bool> {
    ui.horizontal(|ui| {
        ui.label(Type::Body.text(question).color(tokens(ui.ctx()).text_primary));
        let delete = button(ui, "Delete", Variant::Danger);
        let keep = button(ui, "Keep", Variant::Ghost);
        confirm_answer(ui, &delete, &keep)
    })
    .inner
}

/// `confirm_inline` の確かめる button の語を替えた物（RQ4: Connections の `Remove`・Trusted command を足す前の `Allow`）。`Keep` と focus の規則は同じ。
pub fn confirm_inline_with(ui: &mut egui::Ui, question: &str, yes: &str) -> Option<bool> {
    ui.horizontal(|ui| {
        ui.label(Type::Body.text(question).color(tokens(ui.ctx()).text_primary));
        let yes = button(ui, yes, Variant::Danger);
        let keep = button(ui, "Keep", Variant::Ghost);
        confirm_answer(ui, &yes, &keep)
    })
    .inner
}

/// 行内の確認の答え: 開いたばかり（前の pass に描いていない）なら `Keep` に focus（Enter は focus の在る Keep を押す = 何もしない）。
fn confirm_answer(ui: &egui::Ui, yes: &egui::Response, keep: &egui::Response) -> Option<bool> {
    let seen = keep.id.with("kangae-confirm-seen");
    let now = ui.ctx().cumulative_pass_nr();
    let last = ui.data(|d| d.get_temp::<u64>(seen));
    ui.data_mut(|d| d.insert_temp(seen, now));
    if last.is_none_or(|l| l + 1 < now) {
        keep.request_focus();
    }
    if yes.clicked() {
        Some(true)
    } else if keep.clicked() {
        Some(false)
    } else {
        None
    }
}

/// FX3（FX-009・UIUX B02・B09）: tab の席に出す 1 行の名前の欄（`confirm_inline` の隣・modal 0・FINAL SPEC §5.0 の浮く部品 ⑦「名前の欄」）。
/// `text` を中身に 1 行の欄を 1 つ（幅は `TAB_H` の行の中・`INPUT_H` 未満にしない・角 `RADIUS_SM`・`border_input`）描き、`name` を placeholder と AX の名前にする
/// （`ax_name` + `ax_text_input` = BG の driver が AX の Click と SetValue で打てる・⌘Z は欄に文字が在る間は欄の undo）。開いた frame に欄へ focus と全選択（今の名前を打ち替える形）。
/// 返り値: `Some(true)` = 欄に focus が在る間の Return（確定）・`Some(false)` = Esc（破棄）・`None` = 打っている途中（`text` は打った通り）。Enter と Esc で `text` を変えない
/// （確定した名前を送るのも、破棄して閉じるのも呼ぶ側）。外の click で focus が外れただけでは閉じない（`None` のまま・閉じるのは Enter・Esc・Page を移る）。
/// 例（`Generated::draw_page`）: `match theme::rename_inline(ui, &mut draft, "Page name") { Some(true) => intent = Some(Intent::Page(PageIntent::Rename { page, title: draft.clone() })), Some(false) => self.pending_rename = None, None => {} }`
/// 保証しない: 名前の検査（空・別の Page と同じ名前は `router::rename_page` の Err を `page_intent` が欄の右の 1 行に出す）・Project の名前替え（同じ部品を `Project name` で使える形だが、FX3 では呼ばない）。
/// 本体の約束: 欄は言葉の欄（header の 1 つ・`d1_one_field`）ではない ⑦ なので、`scripts/qa/ui-owner-check.py` の `c_one_field` に「この fn の中の 1 つ」を除く行を同じ commit で足す（数え方を緩めるのはこの 1 つだけ）。
/// Wave: FX3
pub fn rename_inline(ui: &mut egui::Ui, text: &mut String, name: &str) -> Option<bool> {
    use super::ax::AxTextInput;
    const NAME_W: f32 = 160.0;
    let t = tokens(ui.ctx());
    let edit = egui::TextEdit::singleline(text)
        .hint_text(Type::Body.text(name).color(t.text_tertiary))
        .font(Type::Body.font())
        .desired_width(NAME_W)
        .margin(input_margin(ui, Type::Body, INPUT_H));
    let r = ax_name(ui.add(edit), name).ax_text_input(ui, text);
    // 開いた frame に focus と全選択（今の名前を打ち替える形）
    if focus_when_shown(ui, &r) {
        if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), r.id) {
            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()))));
            st.store(ui.ctx(), r.id);
        }
    }
    // egui は Enter と Esc で欄の focus を外す（Esc は pass の頭で外す）= その frame の lost_focus と key で読む
    if !r.lost_focus() {
        return None;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Some(false);
    }
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        // 断られたら（同じ名前・空）打ち直せるよう focus を戻す（通れば欄は次の frame で消え、focus も消える）
        r.request_focus();
        return Some(true);
    }
    None
}

/// 前の pass に描いていない（= 出たばかり）widget に keyboard focus を置く（`confirm_inline`・`quit_inline` の失わない側・`rename_inline` の欄）。true = 置いた（出たばかり）。
fn focus_when_shown(ui: &egui::Ui, r: &egui::Response) -> bool {
    let seen = r.id.with("kangae-confirm-seen");
    let now = ui.ctx().cumulative_pass_nr();
    let last = ui.data(|d| d.get_temp::<u64>(seen));
    ui.data_mut(|d| d.insert_temp(seen, now));
    let shown = last.is_none_or(|l| l + 1 < now);
    if shown {
        r.request_focus();
    }
    shown
}

/// FX1（FX-001）: 終わる前の保存に失敗した時の同じ場所の 1 行（`confirm_inline` の隣・modal 0）。`Couldn't save your changes · {reason}`・`Quit anyway`
/// （danger の字・塗りなし）・`Keep open` の順に横へ描き、開いた frame に `Keep open` へ focus（Enter は失わない側）。語はこの本体の literal（引数で渡さない）。
/// 返り値: `Some(true)` = `Quit anyway`・`Some(false)` = `Keep open`・`None` = どちらも押していない。Esc は呼ぶ側（app.rs の Esc の連鎖 = `Keep open` と同じ）。
/// 例: `match theme::quit_inline(ui, why) { Some(true) => intents.push(Intent::Quit { anyway: true }), Some(false) => self.quit_failed = None, None => {} }`
/// 保証しない: 理由の文の言い換え（`save_now` の Err の文をそのまま出す）・保存し直す button（`Keep open` の後に人が直して ⌘Q し直す）。
/// Wave: FX1
pub fn quit_inline(ui: &mut egui::Ui, reason: &str) -> Option<bool> {
    let t = tokens(ui.ctx());
    let mut answer = None;
    ui.horizontal(|ui| {
        ui.label(Type::Body.text(format!("Couldn't save your changes · {reason}")).color(t.text_primary));
        let quit = button(ui, "Quit anyway", Variant::Danger);
        let keep = button(ui, "Keep open", Variant::Ghost);
        focus_when_shown(ui, &keep); // Enter は失わない側
        if quit.clicked() {
            answer = Some(true);
        }
        if keep.clicked() {
            answer = Some(false);
        }
    });
    answer
}

/// hover 中の押せる helper の cursor を Style の `interact_cursor`（指）にする。egui 0.31 でこれを読むのは素の `Button::ui`（widgets/button.rs:396）だけで、
/// `ui.interact` / `allocate_exact_size` で自前に描く `button_sized` と `icon_button` は自分で置く（holdout UI-3 h2 / h6 の Hover）。
fn point(ui: &egui::Ui, hovered: bool) {
    if let (true, Some(c)) = (hovered, ui.visuals().interact_cursor) {
        ui.ctx().set_cursor_icon(c);
    }
}

/// 2px outline + 2px offset（§2.4 focus.ring）。focus が在り、最後の入力が key（⌘ / Ctrl / Alt なし）の時だけ（mouse で開いた menu の先頭・確認の `Keep` には出さない・sendback 79）。
/// 最後の入力 = この pass の event の最後の key か pointer の押し下げ（無い pass は前の値）。focus を置く所（`menu_popup`・`confirm_inline`）は変えない（key の人の道）。
pub fn focus_ring(ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, radius: CornerRadius) {
    let last = ui.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::Key { pressed: true, modifiers, .. } if !(modifiers.command || modifiers.ctrl || modifiers.alt) => Some(true),
            egui::Event::PointerButton { pressed: true, .. } => Some(false),
            _ => None,
        })
    });
    let by_key = egui::Id::new("kangae-focus-by-key");
    let by_key = ui.ctx().data_mut(|d| {
        let v = d.get_temp_mut_or(by_key, false);
        *v = last.unwrap_or(*v);
        *v
    });
    if by_key && response.has_focus() {
        let t = tokens(ui.ctx());
        ui.painter().rect_stroke(rect.expand(FOCUS_RING_OFFSET), radius, Stroke::new(FOCUS_RING_W, t.focus_ring), StrokeKind::Outside);
    }
}

/// 選択 / hover の行背景（list row・nav row。§4.3）。選択は左 2px bar 付き。
/// Wave: UI-3 — 選択の縦棒は `text_primary`（ink・青を 2 番目の色に使わない・owner の決定 4）。地は今のまま `surface_2`。
/// SIZES-1101 / 226（owner「グレーになる範囲…デカすぎる」）: 地と縦棒は行の左右から `SP_1` 内（Orca の一覧の行 = 端に付かない角 6 の箱）。押せる範囲は行のまま。
/// 返す = 地の矩形（operator の決め: 行の字と icon は地の左右から `SP_2` 内 = 地の中の余白 8・Orca の px-2 の中の px-2）。字を行の `SP_3` 内に置く行は返り値を使わなくてよい。
/// 呼ぶ例: `let rect = theme::row_background(ui, rect, resp.hovered(), selected);` → 字は `rect.left() + theme::SP_2`。
pub fn row_background(ui: &egui::Ui, rect: egui::Rect, hovered: bool, selected: bool) -> egui::Rect {
    let t = tokens(ui.ctx());
    let rect = rect.shrink2(Vec2::new(SP_1, 0.0));
    if selected || hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_SM), t.surface_2);
    }
    if selected {
        let bar = egui::Rect::from_min_size(rect.left_top(), Vec2::new(2.0, rect.height()));
        ui.painter().rect_filled(bar, CornerRadius::same(1), t.text_primary);
    }
    rect
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// ICONS-1101: どの `Icon` も Phosphor Bold の字を 1 つ持つ（□ にならない）・別の icon と字が重ならない（`PanelRight` だけ `PanelLeft` の字を返して描く）。
    /// 負の対照（commit message・未実行）: 腕の 1 つを別の icon と同じ字にする → 重なりで赤・`ICON_FAMILY` を `fonts` で入れない → has_glyph で赤・
    /// Proportional の 2 番目に積まない → 字の文の has_glyph で赤
    #[test]
    fn icons1101_every_icon_has_its_own_glyph() {
        let ctx = egui::Context::default();
        install(&ctx, ThemePreference::Light);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let font = FontId::new(ICON_UI, FontFamily::Name(ICON_FAMILY.into()));
        let mut seen = std::collections::HashMap::new();
        for i in Icon::ALL {
            let g = icon_glyph(i);
            let c = g.chars().next().unwrap_or_default();
            assert!(g.chars().count() == 1 && ('\u{E000}'..='\u{F8FF}').contains(&c), "{i:?} の字が Phosphor の番号でない: {g:?}");
            assert!(ctx.fonts(|f| f.has_glyph(&font, c)), "{i:?} の字が Phosphor Bold に無い（□）");
            assert!(ctx.fonts(|f| f.has_glyph(&Type::Body.font(), c)), "{i:?} の字が字の文（Proportional）から引けない（menu の ••• が □）");
            if i != Icon::PanelRight {
                if let Some(o) = seen.insert(g, i) {
                    panic!("{i:?} と {o:?} が同じ字");
                }
            }
        }
    }

    #[test]
    fn light_and_dark_share_geometry_and_differ_in_color() {
        assert_ne!(LIGHT.bg_app, DARK.bg_app);
        assert_eq!(LIGHT.working, rgb(0x2359CE));
        assert_eq!(DARK.working, rgb(0x91B5FF));
    }

    /// WCAG 2.x の相対輝度の比（sRGB を線形にしてから重みを掛ける）。
    pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
        fn lum(c: Color32) -> f32 {
            let lin = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// UI-3 Done 1: 足した 3 token は SPEC §12 の値（Dark は THEME-DARK-1101 の中立の黒）・press は hover と別の灰・focus の輪は ink・比は WCAG の下限以上（Light と Dark）。
    /// 負の対照（commit message）: LIGHT の `surface_press` を `surface_3` と同じ値にする → 赤。
    #[test]
    fn ui3_new_tokens_have_spec_values_and_contrast() {
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01, "比の関数の検算（白と黒 = 21）");
        for (name, t, want) in [("Light", LIGHT, [0xDFE3E8, 0xCFD6DF, 0x8190A2]), ("Dark", DARK, [0x333333, 0x404040, 0x5E5E5E])] {
            assert_eq!([t.surface_press, t.highlight, t.border_input], want.map(rgb), "{name}: SPEC §12 の値");
            assert_ne!(t.surface_press, t.surface_3, "{name}: press と hover が同じ灰（押した事が見えない）");
            assert_eq!(t.focus_ring, t.text_primary, "{name}: focus の輪は ink");
            for (what, a, b, min) in [
                ("text_primary / surface_press", t.text_primary, t.surface_press, 4.5),
                ("border_input / bg_app", t.border_input, t.bg_app, 3.0),
                ("focus_ring / bg_app", t.focus_ring, t.bg_app, 3.0),
            ] {
                let r = contrast(a, b);
                assert!(r >= min, "{name} {what}: {r:.2} < {min}");
            }
        }
        assert_eq!(DARK.border_input, DARK.border_strong);
    }

    /// THEME-DARK-1101（owner 223「もっと黒く。orcaと同じぐらい」）: Dark の地は青みの無い中立の灰（r = g = b）で、字 3 段は地 3 段（`bg_app`・`surface_1`・`surface_2`）の
    /// どれの上でも 4.5:1 以上（AA）。負の対照: `text_tertiary` を spec の候補 #737373 にする → bg_app の上で 4.2 で赤・`bg_app` を前の #11151B に戻す → 中立で赤。
    #[test]
    fn theme_dark_is_neutral_black_and_text_reads() {
        let t = DARK;
        for (name, c) in [("bg_app", t.bg_app), ("surface_1", t.surface_1), ("surface_2", t.surface_2), ("surface_3", t.surface_3), ("border", t.border), ("border_input", t.border_input)] {
            assert!(c.r() == c.g() && c.g() == c.b(), "{name} に色み: {c:?}");
        }
        for (what, ink) in [("text_primary", t.text_primary), ("text_secondary", t.text_secondary), ("text_tertiary", t.text_tertiary)] {
            for (on, bg) in [("bg_app", t.bg_app), ("surface_1", t.surface_1), ("surface_2", t.surface_2)] {
                let r = contrast(ink, bg);
                assert!(r >= 4.5, "{what} / {on}: {r:.2} < 4.5");
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Pointer {
        Rest,
        Hover,
        Press,
    }

    /// headless の egui（Style は `install` と同じ・font は egui の同梱）で `draw` を 3 frame 描く: 1 frame 目で置き、2 frame 目で pointer を
    /// 置き（Rest は画面の隅・Hover と Press は 1 frame 目の rect の中心・Press は押したまま）、3 frame 目の Response と平らにした shape を返す。
    fn paint(dark: bool, pointer: Pointer, draw: impl FnMut(&mut egui::Ui) -> egui::Response) -> (egui::Response, Vec<egui::Shape>) {
        let (r, shapes, _) = paint_full(dark, pointer, draw);
        (r, shapes)
    }

    /// `paint` と同じ 3 frame で、3 frame 目の cursor（`platform_output.cursor_icon`）も返す。
    fn paint_full(dark: bool, pointer: Pointer, mut draw: impl FnMut(&mut egui::Ui) -> egui::Response) -> (egui::Response, Vec<egui::Shape>, egui::CursorIcon) {
        let ctx = egui::Context::default();
        // system font は読まない（速さ）。Style の見出しの段が引く SEMIBOLD の family だけ同梱の face で登録する（無いと egui が panic）
        let mut f = egui::FontDefinitions::default();
        let prop = f.families[&FontFamily::Proportional].clone();
        f.families.insert(FontFamily::Name(SEMIBOLD.into()), prop);
        ctx.set_fonts(f);
        ctx.set_style_of(Theme::Light, style(&LIGHT, false));
        ctx.set_style_of(Theme::Dark, style(&DARK, true));
        ctx.set_theme(if dark { ThemePreference::Dark } else { ThemePreference::Light });
        let (mut at, mut last) = (egui::Pos2::ZERO, None);
        for frame in 0..3 {
            let mut events = Vec::new();
            if frame == 1 {
                let pos = if matches!(pointer, Pointer::Rest) { egui::pos2(990.0, 790.0) } else { at };
                events.push(egui::Event::PointerMoved(pos));
                if matches!(pointer, Pointer::Press) {
                    events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
                }
            }
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))), events, ..Default::default() };
            let mut resp = None;
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| resp = Some(draw(ui)));
            });
            let r = resp.expect("描いていない");
            at = r.rect.center();
            fn flat(s: egui::Shape, out: &mut Vec<egui::Shape>) {
                match s {
                    egui::Shape::Vec(v) => v.into_iter().for_each(|s| flat(s, out)),
                    s => out.push(s),
                }
            }
            let cursor = out.platform_output.cursor_icon;
            let mut shapes = Vec::new();
            out.shapes.into_iter().for_each(|c| flat(c.shape, &mut shapes));
            last = Some((r, shapes, cursor));
        }
        last.unwrap()
    }

    /// `hit`（押せる範囲）の中の Rect と Text の shape。
    fn inside(shapes: &[egui::Shape], hit: egui::Rect) -> (Vec<egui::epaint::RectShape>, Vec<egui::epaint::TextShape>) {
        let area = hit.expand(0.5);
        let rects = shapes.iter().filter_map(|s| match s {
            egui::Shape::Rect(r) if area.contains_rect(r.rect) => Some(r.clone()),
            _ => None,
        });
        let texts = shapes.iter().filter_map(|s| match s {
            egui::Shape::Text(t) if area.contains(t.pos) => Some(t.clone()),
            _ => None,
        });
        (rects.collect(), texts.collect())
    }

    /// UI-3 Done 2（owner の決定 4）→ UI-SWEEP-1101（owner 196）→ 追記（owner 209「settingやconnectionなどの中だけでいいよ。他は元に戻して」）:
    /// `framed` の範囲の中は primary が ink の面で枠なし・ほかの variant は休み・hover・press のどれでも 1px の `border_strong` の枠、範囲の外はどの variant も線なし。
    /// 塗りは範囲に依らず `Variant` の表（primary は ink → text_secondary・選択中は surface_2・ほかは 透明 → surface_3 → surface_press）で、文字は primary だけ surface_1。
    /// icon button は枠なしのまま。28 と帯の中の 24・Light と Dark。負の対照: `frame_of` の範囲の印を見る 3 行を消す → 範囲の外に枠が出て赤・`button_sized` の `frame` を NONE → 範囲の中が赤。
    #[test]
    fn ui_sweep_buttons_framed_only_in_framed_and_fill_by_state() {
        let states = [Pointer::Rest, Pointer::Hover, Pointer::Press];
        for (dark, t) in [(false, LIGHT), (true, DARK)] {
            let clear = Color32::TRANSPARENT;
            let table = [
                (Variant::Primary, [t.text_primary, t.text_secondary, t.text_secondary], t.surface_1),
                (Variant::Selected, [t.surface_2, t.surface_3, t.surface_press], t.text_primary),
                (Variant::Ghost, [clear, t.surface_3, t.surface_press], t.text_primary),
                (Variant::Danger, [clear, t.surface_3, t.surface_press], t.danger),
            ];
            for (variant, fills, text) in table {
                for (pointer, want) in states.into_iter().zip(fills) {
                    for h in [BUTTON_H, BUTTON_COMPACT_H] {
                        for within in [false, true] {
                            let what = format!("dark={dark} {variant:?} {pointer:?} {h} framed={within}");
                            let (r, shapes) = paint(dark, pointer, |ui| {
                                let _framed = within.then(|| framed(ui.ctx()));
                                button_sized(ui, "Create", variant, None, h)
                            });
                            let (rects, texts) = inside(&shapes, r.rect);
                            let face = rects.iter().find(|s| (s.rect.height() - h).abs() < 0.01).unwrap_or_else(|| panic!("{what}: 塗りの rect が無い {rects:?}"));
                            assert_eq!(face.fill, want, "{what}: 塗り");
                            let frame = if variant == Variant::Primary || !within { Stroke::NONE } else { Stroke::new(HAIRLINE, t.border_strong) };
                            assert_eq!(face.stroke, frame, "{what}: 枠");
                            assert!(texts.iter().any(|s| s.fallback_color == text), "{what}: 文字の色");
                        }
                    }
                }
            }
            for (pointer, want) in states.into_iter().zip([clear, t.surface_3, t.surface_press]) {
                let (r, shapes) = paint(dark, pointer, |ui| icon_button(ui, Icon::X, "Close"));
                let (rects, _) = inside(&shapes, r.rect);
                let face = rects.iter().find(|s| s.rect == r.rect).unwrap_or_else(|| panic!("icon {pointer:?}: 塗りの rect が無い"));
                assert_eq!(face.fill, want, "icon dark={dark} {pointer:?}");
                assert!(rects.iter().all(|s| s.stroke.width == 0.0 || s.stroke.color.a() == 0), "icon {pointer:?}: 線を描いた");
            }
        }
    }

    /// UI-3 Done 2 の cursor（口「cursor は指」・holdout h2 / h6 の Hover）: 押せる helper（`button`・`button_compact`・`chip`・`icon_button`）は
    /// hover で Style の `interact_cursor`（指）・休みは Default・押せない ui の中では指にしない。Light と Dark。
    /// egui 0.31 の `interact_cursor` を読むのは素の `Button::ui` だけで、自前で描く helper は自分で置く。負の対照（commit message）: `button_sized` の cursor の行を消す → 赤。
    #[test]
    fn ui3_buttons_point_on_hover() {
        use egui::CursorIcon::{Default, PointingHand};
        let cases: [(&str, fn(&mut egui::Ui) -> egui::Response); 7] = [
            ("primary", |ui| button(ui, "Create", Variant::Primary)),
            ("ghost", |ui| button(ui, "Create", Variant::Ghost)),
            ("danger", |ui| button(ui, "Create", Variant::Danger)),
            ("selected", |ui| button(ui, "Create", Variant::Selected)),
            ("compact", |ui| button_compact(ui, "Undo", Variant::Ghost)),
            ("chip", |ui| chip(ui, "Research notes")),
            ("icon", |ui| icon_button(ui, Icon::X, "Close")),
        ];
        for dark in [false, true] {
            for (name, draw) in cases {
                assert_eq!(paint_full(dark, Pointer::Hover, draw).2, PointingHand, "dark={dark} {name}: hover");
                assert_eq!(paint_full(dark, Pointer::Rest, draw).2, Default, "dark={dark} {name}: rest");
            }
            let (_, _, c) = paint_full(dark, Pointer::Hover, |ui| ui.add_enabled_ui(false, |ui| button(ui, "Back", Variant::Ghost)).inner);
            assert_eq!(c, Default, "dark={dark}: 押せない ui の中の button が指");
        }
    }

    /// UI-3 Done 3（owner の決定 4・5）: button の描く高さ 28・帯の中 24・文字の横の inset 8 / 6・押せる範囲（interact の rect）≥ `HIT_MIN` 28。
    /// chip は compact と同じ形・icon は 28 四方・押せない button は同じ inset で地も線も無い。負の対照（commit message）: hit を描く rect のままにする → 赤。
    #[test]
    fn ui3_button_size_inset_and_hit_area() {
        assert_eq!((BUTTON_H, BUTTON_COMPACT_H, BUTTON_PAD_X, BUTTON_COMPACT_PAD_X, HIT_MIN), (28.0, 24.0, 8.0, 6.0, 28.0));
        let cases: [(&str, f32, f32, fn(&mut egui::Ui) -> egui::Response); 4] = [
            ("button", BUTTON_H, BUTTON_PAD_X, |ui| button(ui, "Create", Variant::Primary)),
            ("compact", BUTTON_COMPACT_H, BUTTON_COMPACT_PAD_X, |ui| button_compact(ui, "Undo", Variant::Ghost)),
            ("chip", BUTTON_COMPACT_H, BUTTON_COMPACT_PAD_X, |ui| chip(ui, "Research notes")),
            ("send", INPUT_H, BUTTON_PAD_X, |ui| button_sized(ui, "Send", Variant::Primary, None, INPUT_H)),
        ];
        for (name, h, pad, draw) in cases {
            let (r, shapes) = paint(false, Pointer::Hover, draw);
            let (rects, texts) = inside(&shapes, r.rect);
            let face = rects.iter().find(|s| (s.rect.height() - h).abs() < 0.01).unwrap_or_else(|| panic!("{name}: 高さ {h} の塗りが無い {rects:?}"));
            let text = texts.first().unwrap_or_else(|| panic!("{name}: 文字が無い"));
            assert!((text.pos.x - face.rect.left() - pad).abs() < 0.01, "{name}: inset {} ≠ {pad}", text.pos.x - face.rect.left());
            assert!(r.rect.height() >= HIT_MIN && r.interact_rect.height() >= HIT_MIN, "{name}: 押せる範囲 {:?}", r.interact_rect);
            assert!(r.rect.width() >= HIT_MIN, "{name}: 押せる幅 {:?}", r.rect);
        }
        let (r, _) = paint(false, Pointer::Hover, |ui| icon_button(ui, Icon::X, "Close"));
        assert_eq!(r.rect.size(), Vec2::splat(BUTTON_H));
        let (r, shapes) = paint(false, Pointer::Hover, |ui| button_disabled(ui, "Send", INPUT_H, "Type something first"));
        let (rects, texts) = inside(&shapes, r.rect);
        assert_eq!(r.rect.height(), INPUT_H);
        assert!(rects.is_empty(), "範囲の外の押せない button が地か線を描いた（owner 209 = 5 回目の形） {rects:?}");
        assert!((texts[0].pos.x - r.rect.left() - BUTTON_PAD_X).abs() < 0.01 && texts[0].fallback_color == LIGHT.text_tertiary);
        let (r, shapes) = paint(false, Pointer::Hover, |ui| {
            let _framed = framed(ui.ctx());
            button_disabled(ui, "Send", INPUT_H, "Type something first")
        });
        let (rects, _) = inside(&shapes, r.rect);
        assert!(!rects.is_empty() && rects.iter().all(|s| s.fill.a() == 0 && s.stroke.color == LIGHT.border), "範囲の中の押せない button は地なし・薄い border の枠（UI-SWEEP-1101） {rects:?}");
    }

    /// 押せない ui（`add_enabled_ui(false)`・generated.rs の `Back`・app.rs の `Get data` ほか・agent_rail の Primary）の helper の button は
    /// `button_disabled` と同じ形: 地なし・薄い `border` の枠（UI-SWEEP-1101）・文字は `text_tertiary` そのもの（egui の fade で地より明るくならない）。負の対照（commit message）: fade を外す 3 行を消す → 赤。
    #[test]
    fn ui3_button_in_a_disabled_ui_reads_like_button_disabled() {
        for (dark, t) in [(false, LIGHT), (true, DARK)] {
            for (variant, h) in [(Variant::Ghost, BUTTON_COMPACT_H), (Variant::Primary, BUTTON_H), (Variant::Danger, BUTTON_H)] {
                let what = format!("dark={dark} {variant:?} {h}");
                let (r, shapes) = paint(dark, Pointer::Hover, |ui| ui.add_enabled_ui(false, |ui| button_sized(ui, "Back", variant, None, h)).inner);
                let (rects, texts) = inside(&shapes, r.rect);
                assert!(rects.iter().all(|s| s.fill.a() == 0 && (s.stroke.width == 0.0 || s.stroke.color == t.border)), "{what}: 地か濃い枠を描いた {rects:?}");
                assert!(!texts.is_empty() && texts.iter().all(|s| s.fallback_color == t.text_tertiary), "{what}: 文字が text_tertiary でない {:?}", texts.iter().map(|s| s.fallback_color).collect::<Vec<_>>());
            }
        }
    }

    /// `native_buttons` の scope の中の素の button は helper と同じ形（`framed` の範囲の中 = `border_strong` の枠・外 = 線なし〈owner 209〉）で、
    /// scope の外の入力欄は `border_input` の枠のまま（Style 一括でない・sendback 36）。
    #[test]
    fn ui_sweep_native_buttons_follow_the_framed_scope() {
        let native = |ui: &mut egui::Ui| ui.scope(|ui| { native_buttons(ui); ui.button("Open in browser") }).inner;
        let (r, shapes) = paint(false, Pointer::Hover, |ui| {
            let _framed = framed(ui.ctx());
            native(ui)
        });
        let (rects, _) = inside(&shapes, r.rect);
        assert!(rects.iter().any(|s| s.stroke == Stroke::new(HAIRLINE, LIGHT.border_strong)), "範囲の中の素の button に border_strong の枠が無い {rects:?}");
        assert!(rects.iter().any(|s| s.fill == LIGHT.surface_3), "素の button の hover が surface_3 でない {rects:?}");
        let (r, shapes) = paint(false, Pointer::Hover, native);
        let (rects, _) = inside(&shapes, r.rect);
        assert!(!rects.is_empty() && rects.iter().all(|s| s.stroke.width == 0.0 || s.stroke.color.a() == 0), "範囲の外の素の button に線（owner 209 = 5 回目の形） {rects:?}");
        let input_framed = |shapes: &[egui::Shape]| shapes.iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.color == LIGHT.border_input && r.stroke.width > 0.0));
        let mut text = String::new();
        let (_, outside) = paint(false, Pointer::Rest, |ui| {
            ui.scope(|ui| native_buttons(ui));
            ui.text_edit_singleline(&mut text)
        });
        assert!(input_framed(&outside), "scope の外の入力欄の枠（border_input）が消えた");
        // doc の「保証しない」: 入力欄を含む ui に掛けると枠が border_input でなくなる（上の assert が赤になれる事の対照）
        let (_, within) = paint(false, Pointer::Rest, |ui| ui.scope(|ui| { native_buttons(ui); ui.text_edit_singleline(&mut text) }).inner);
        assert!(!input_framed(&within), "scope の中の入力欄が border_input のまま（native_buttons が効いていない）");
    }
    #[test]
    fn no_type_below_twelve_px() {
        for k in [Type::Display, Type::HeadingLg, Type::HeadingMd, Type::HeadingSm, Type::Body, Type::BodyStrong, Type::Nav, Type::Helper, Type::Mono, Type::Title, Type::Small, Type::Group] {
            assert!(k.size() >= 12.0, "{k:?}");
            assert!(k.line_height() > k.size(), "{k:?}");
        }
    }
    /// 殻が字のまま描く記号（key の字と区切り）が、起動時に積む font のどれかに在る（無いと □ の tofu）。icon は Phosphor（`icons1101_every_icon_has_its_own_glyph`）。
    /// system font を読むので macOS でだけ走る。
    #[test]
    #[cfg(target_os = "macos")]
    fn shell_glyphs_exist_in_loaded_fonts() {
        let ctx = egui::Context::default();
        fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let missing: Vec<char> = "⌘⇧⌃⌥·…".chars().filter(|&c| !ctx.fonts(|f| f.has_glyph(&Type::Body.font(), c))).collect();
        assert!(missing.is_empty(), "tofu になる glyph: {missing:?}");
    }
    /// ICONS-1101: `icon_button` は Phosphor の字 1 つを 28 の箱の真ん中に描く（font の記号・painter の線を描かない）。`paint_icon` は字の墨の中心で回す
    /// （追記 238 の回る印がぶれない・`PanelRight` は半回転）。負の対照（未実行）: 回す中心を字の箱の左上に戻す → 墨の中心が center から外れて赤
    #[test]
    fn icons1101_icon_button_paints_one_phosphor_glyph_and_turns_on_its_ink_center() {
        let (_, shapes) = paint(false, Pointer::Rest, |ui| icon_button(ui, Icon::Undo, "Undo · ⌘Z"));
        let texts: Vec<&str> = shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = s { Some(t.galley.text()) } else { None }).collect();
        assert_eq!(texts, [icon_glyph(Icon::Undo)], "字が Phosphor の 1 つでない");
        assert!(!shapes.iter().any(|s| matches!(s, egui::Shape::Path(_) | egui::Shape::LineSegment { .. })), "手描きの線が残る");
        let c = egui::pos2(40.0, 30.0);
        for (icon, angle) in [(Icon::Loader, 0.0), (Icon::Loader, 1.0), (Icon::Loader, 4.0), (Icon::PanelRight, 0.0)] {
            let (_, shapes) = paint(false, Pointer::Rest, |ui| {
                paint_icon(ui.painter(), icon, c, ICON_UI, LIGHT.text_primary, angle);
                ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
            });
            let t = shapes.iter().find_map(|s| if let egui::Shape::Text(t) = s { (t.galley.text() == icon_glyph(icon)).then_some(t) } else { None }).unwrap_or_else(|| panic!("{icon:?}: 字の形が無い"));
            let ink = t.pos + egui::emath::Rot2::from_angle(t.angle) * t.galley.mesh_bounds.center().to_vec2();
            assert!(ink.distance(c) < 0.01, "{icon:?} {angle}: 墨の中心 {ink:?} が {c:?} に無い（回るとぶれる）");
        }
    }
    #[test]
    fn style_text_styles_come_from_tokens() {
        let s = style(&LIGHT, false);
        assert_eq!(s.text_styles[&egui::TextStyle::Small].size, 12.0);
        assert_eq!(s.text_styles[&egui::TextStyle::Body].size, 13.0);
        assert_eq!(s.visuals.panel_fill, LIGHT.surface_1);
        assert_eq!(s.visuals.widgets.noninteractive.bg_stroke.color, LIGHT.border);
        assert_eq!(style(&DARK, true).visuals.panel_fill, DARK.surface_1);
        // UI-3: 素の button の塗りは 休み透明 / hover surface_3 / press surface_press・press と open は線なし・入力欄の休みと hover の枠は border_input
        let w = &s.visuals.widgets;
        assert_eq!([w.inactive.weak_bg_fill, w.hovered.weak_bg_fill, w.active.weak_bg_fill], [Color32::TRANSPARENT, LIGHT.surface_3, LIGHT.surface_press]);
        assert_eq!([w.inactive.bg_stroke.color, w.hovered.bg_stroke.color], [LIGHT.border_input; 2]);
        assert_eq!([w.active.bg_stroke, w.open.bg_stroke], [Stroke::NONE; 2]);
        assert_eq!((s.visuals.selection.bg_fill, s.visuals.hyperlink_color, s.visuals.text_cursor.stroke.color), (LIGHT.highlight, LIGHT.text_primary, LIGHT.text_primary));
    }
}
