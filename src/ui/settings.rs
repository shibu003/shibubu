//! RQ1: Settings の画面（監査 C: macOS の app menu `Settings…`・⌘,・⌘K → `Settings` で主領域がこの画面。左に小さな節の list・右に form・card なし）。
//! 置き場: `ui/` の下だが `ui/mod.rs` の module にしない（`model::UiSettings` と `llm::Budget` を読むので、ui だけを `#[path]` で引く試験の crate〈tests/renderer.rs ほか〉では
//! compile できない）→ `engine_page` と同じく `src/main.rs` の `#[path = "ui/settings.rs"] mod settings;`。
//! Connections は「接続の作成・修復」、Settings は「既定値・安全ポリシー」（監査 C）。接続の行はここに写さない（list の `Connections` の行で Connections の画面へ移る）。

use crate::domain::icon::Icon;
use crate::llm::Budget;
use crate::model::{ChatPlace, ThemeChoice, UiSettings};
use crate::ui::connections::Connection;
use crate::ui::shell::{self, DataKind, Intent, SettingsSection, View};
use crate::ui::theme::{self, Type, Variant};
use eframe::egui::{self, Vec2};
use std::time::Duration;

/// RQ1: Settings の主領域を描く（app.rs の `View::Settings(section)` の腕の `draw_app_region` の中から毎 frame）。
/// 引数: `section` = 今の節・`settings` = `store.ui`（form が直に書く・画面の値は保存した物そのもの）・`ai` = `Generated::ai_rows()`・`used` = `Generated::budget_used()`・
/// `about` = `Generated::diagnostics(&connections_model().rows)`（About の行と `Copy diagnostics` の文が同じ）。
/// 左の list（上から）: `General`・`AI & Models`・`Connections`（→ `Intent::Go(View::Connections)`・節ではない）・`Terminal`・`About`（押すと `Intent::Go(View::Settings(節))`・今の節は選んだ地）。
/// 段 2 作り直し: 引数 `setup`（app の `agent_setup`）を足した。Terminal の節 = `terminal_section`（Install CLI & skill の状態と Remove）。
/// 右の form（英語・sentence case・env の名前は出さない）:
/// - General: `Appearance`（Match system / Light / Dark → ⌘K の Appearance の行と同じ `Intent::SetTheme`・保存と ctx の theme は app.rs の 1 か所）・`Reduce motion`（Match system / On / Off → `reduce_motion` = None / Some(true) / Some(false)）・
///   `When Shibubu opens`（Home / Last project → `reopen_last_project`）・`Text size`（`−` `{n}%` `+`・80〜150 を 10 刻み → `text_zoom`・横に `⌘+ ⌘− ⌘0`）・
///   `Mirror smoothness`（MIRROR-SPEED-1101・feature mirror の build だけ: Auto / Smooth / Saver → `mirror_smoothness`・数を打つ欄は作らない）・
///   `Shortcuts`（読むだけの表: ⌘K・⌘N・⌘,・⌘↵・⌘Z・⇧⌘Z・⌃⌘S・⇧⌘P・⌃`・⌃⇧`）
/// - AI & Models: `ai` の各行（名前・状態の記号と語・detail）・`Execution limit`（回数 0〜100・秒 0〜3600 → `llm_max_calls` / `llm_max_secs` = Some(..)・`Use default` で両方 None）と
///   その下の 1 行（`used` と `Budget::resolve(settings.llm_max_calls, settings.llm_max_secs)` から `3 of 10 calls · 42 of 600 seconds used by your last request`・上限に着いたら
///   `Stopped at 10 calls · raise Calls, or send your next request`・`used` が None なら `No AI calls yet`）。TALK-1101 追記 2: 上限は 1 つの頼みごと（一言で数え直す）。`Default model` / `Default effort` は描かない
///   （選ぶ所は Home の `+` の `Model ▸` / `Effort ▸` の 1 か所・G84 / G104）。AI が使えない時は `ai` の行の理由と次の一手だけ（押せない欄を並べない）
/// - About: `about` の (項目, 値) の行・`Copy usage report`（`Intent::CopyUsageReport`）・`Copy diagnostics`（`Intent::CopyDiagnostics`）
/// 契約（RQ2・本体は RQ2 の実装の片）: AI & Models の上に `Accounts` = `Claude Code`（利用者の Claude Code の login・PROVIDER-1101 で System default から改名）と `settings.ai_accounts` の各行
/// （`Plan` = 名前〈ACCOUNTS-1101 2 段目: 付けた名前 → email の @ の前 → `Claude 2`・email そのものは出さない〉・`Key` = `API key ····{last4}`・`EnvKey` = `Key from environment ····{llm_cli::env_key_last4}`）。使っている行に `In use`・
/// 他の行は `Use`（`Intent::UseAiAccount`）・`Re-authenticate`（Plan だけ・`Intent::ReauthAiAccount`）・`Remove`（`Intent::RemoveAiAccount`）。下に `Add account`
/// （`Intent::AddAiAccount`）・`Paste API key`（`Intent::PasteApiKey`・clipboard を読む・入力欄は作らない・鍵は billed per use と 1 行）・env に鍵が在る時だけ `Use key from environment`
/// （`Intent::UseEnvKey`）。その下に `usage`（(1 行, 隣の button の名前と Intent)・app.rs の `usage_view` = `llm::usage_line` と `llm_cli::usage_step`）。`usage` が None = 行を描かない。
/// 契約（RQ-USAGE）: `usage` を使用量の答えから「1 行と button」に替えた（下の帯と同じ物を app.rs の 1 か所で作る・読めた = `Refresh`・読めない = `Try again`・login が切れた = `Sign in`）。
/// 契約（RQ-N・RQ-ACCT）: 引数 `busy`（browser の sign in の途中の 1 行・Some の間は `Add account` と `Re-authenticate` を描かず、この 1 行を Accounts の下に）と
/// `emails`（(account の id, email)・app.rs が `llm_cli::account_email` で開いている間だけ読む・画面だけ）を足した。
/// 契約（ACCOUNTS-1101）: 引数 `unsigned`（足した CLI の account のうち login がまだの id・`llm_cli::cli_account_signed_in` が Some(false)）を足した。Accounts は
/// `accounts`（Connections の AI providers と同じ部品）。
/// 保証する（RQ2）: 鍵の値を描かない・AX の名前に出さない（末尾 4 文字だけ）。
/// 戻り値: form の値をこの frame で変えたら `Some(Intent::SettingsChanged)`（app.rs が副作用を当て直して保存）・list の行・copy の button はその intent・他は None。
/// 保証する: card・modal・2 つ目の言葉の欄を作らない・押せる物は全部効く（効かない物は描かないか押せない形と理由 1 行）・AX の名前は見える語と同じ。
/// 保証しない: 変えた値の取り消し（今の値を同じ所で戻すだけ・Undo の対象にしない）・節の中の scroll の位置。
/// 契約（RQ3・本体は RQ3 の実装の片）: 引数 `data`（`Generated::data_summary()`・None = まだ読んでいない = `Checking what Shibubu keeps` の 1 行だけ）と
/// `confirm`（`Shell::data_confirm`・行内の確認）を足した。list の `Connections` の行と `About` の間に `Data & Recovery`（`SettingsSection::Data`）。form（上から・card なし）:
/// - `Saved in` = `data.location`（home は `~`・path を変える欄は作らない）と `Reveal in Finder`（`Intent::RevealInFinder(location)`）
/// - Backups: `Back up now`（`Intent::BackUpNow`）・`data.backups` の各行（`2 h ago · Daily · 2.1 MB` と `Restore`（AX `Restore {行}`） = `Intent::RestoreBackup(Some(id))`・
///   壊れて隔離した世代は `Damaged · set aside` と `Try restoring`）・予約が在れば `Restores when Shibubu reopens` と `Cancel`（`Intent::RestoreBackup(None)`）
/// - Projects: `data.projects` の (名前, 大きさ) の行（読むだけ・書き出しは各 Project の `Export…`）と `Import`: `data.imports` の file ごとに `Import {file 名}`
///   （`Intent::ImportProject`）・0 件なら `Put a .shibubu file in Downloads to import it` の 1 行
/// - What Shibubu keeps: `DataKind::ALL` の行（`label`・`data.kept` の大きさ・保存期間 = `Generated::prune_records` の doc の語）と `Delete…`（`*confirm = Some(kind)`）・
///   `confirm` の行の下に `theme::confirm_inline`（`DataKind::effect`・`Delete` = `Intent::DeleteMyData(kind)`・`Keep` = None）。`History` の行に
///   `Keep for` の 3 択（7 / 30 / 90 days → `settings.history_days`・`Intent::SettingsChanged`）。下に clipboard の 1 行（`Copied diagnostics and usage reports
///   stay on your clipboard until you copy something else`）
/// 保証する（RQ3）: 押せる button は全部効く（Restore は次の起動・その事を行が言う）・env の名前を出さない・Delete は 2 回押さないと消えない。
/// 契約（ACCOUNTS-1101 2 段目・作る物 6）: 引数 `own`（帯の `OwnPanel` = `own_usage::Counter::panel`・None = 数えていない起動）を足した。AI & Models の
/// `AI on this Mac` の下に `Used in Shibubu`（`shell::own_usage_breakdown` と `own_usage_footer` = 帯の panel と同じ数字・同じ部品）。
/// Wave: RQ1, RQ2, RQ3, ACCOUNTS-1101
#[allow(clippy::too_many_arguments)] // 節ごとに要る物を app.rs が節の間だけ作って渡す（About の rows・AI の rows と同じ形）
pub fn draw(
    ui: &mut egui::Ui,
    section: SettingsSection,
    settings: &mut UiSettings,
    ai: &[Connection],
    used: Option<(u32, Duration)>,
    usage: Option<(&str, Option<(&'static str, Intent)>)>,
    busy: Option<&str>,
    emails: &[(String, String)],
    unsigned: &[String],
    about: &[(String, String)],
    data: Option<&crate::generated::DataSummary>,
    confirm: &mut Option<DataKind>,
    allowed: &[String],
    mirror: &[String],
    setup: Option<&mut crate::terminal::AgentSetup>,
    own: Option<&shell::OwnPanel>,
) -> Option<Intent> {
    // UI-SWEEP-1101 追記（owner 209）: button の枠は Settings・Connections・Runs の中だけ
    let _framed = theme::framed(ui.ctx());
    let mut out = None;
    shell::content_opening(ui, "Settings", "", None);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(LIST_W);
            for (label, to) in [
                (SettingsSection::General.label(), View::Settings(SettingsSection::General)),
                (SettingsSection::AiModels.label(), View::Settings(SettingsSection::AiModels)),
                ("Connections", View::Connections),
                (SettingsSection::Terminal.label(), View::Settings(SettingsSection::Terminal)),
                (SettingsSection::Data.label(), View::Settings(SettingsSection::Data)),
                (SettingsSection::Privacy.label(), View::Settings(SettingsSection::Privacy)),
                (SettingsSection::About.label(), View::Settings(SettingsSection::About)),
            ] {
                let here = to == View::Settings(section);
                if list_row(ui, label, here) && !here {
                    out = Some(Intent::Go(to));
                }
            }
        });
        ui.add_space(theme::SP_7);
        ui.vertical(|ui| {
            ui.set_max_width(FORM_W.min(ui.available_width()));
            let picked = match section {
                SettingsSection::General => general(ui, settings),
                SettingsSection::AiModels => ai_models(ui, settings, ai, used, usage, busy, emails, unsigned, own),
                SettingsSection::Terminal => {
                    terminal_section(ui, setup);
                    None
                }
                SettingsSection::Data => data_section(ui, settings, data, confirm),
                SettingsSection::Privacy => privacy_section(ui, allowed, mirror),
                SettingsSection::About => about_section(ui, about),
            };
            out = picked.or(out.take());
        });
    });
    out
}

/// 左の list の幅と、右の form の幅の上限（読む行が長くなり過ぎない）。
const LIST_W: f32 = 168.0;
const FORM_W: f32 = 560.0;

/// 左の list の 1 行（sidebar の行と同じ地: 選んだ行は `surface_2` と左の線・AX は選んだか付きの Button）。押されたか。
fn list_row(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let t = theme::tokens(ui.ctx());
    // SIZES-1101 / 226: sidebar の NAV の行と同じ 28（前は 32）
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::NAV_ROW_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    if ui.is_rect_visible(rect) {
        theme::row_background(ui, rect, response.hovered(), selected);
        theme::focus_ring(ui, &response, rect, egui::CornerRadius::same(theme::RADIUS_SM));
        let color = if selected { t.text_primary } else { t.text_secondary };
        let g = theme::truncated(ui, label, Type::Nav.font(), color, rect.width() - 2.0 * theme::SP_3);
        ui.painter().galley(egui::pos2(rect.left() + theme::SP_3, rect.center().y - g.size().y / 2.0), g, color);
    }
    response.clicked()
}

/// form の 1 段の見出し（群の題 = `Type::Group` 17 太字・UI-SWEEP-1101）と、その下の 1 行の説明（空なら無し）。
fn field_heading(ui: &mut egui::Ui, title: &str, help: &str) {
    let t = theme::tokens(ui.ctx());
    // SIZES-1101 / 226: 見出しの上下を 1 段詰める（16 → 12・8 → 6）
    ui.add_space(theme::SP_3);
    theme::heading(ui, Type::Group, title, t.text_primary);
    if !help.is_empty() {
        theme::label(ui, Type::Helper, help, t.text_secondary);
    }
    ui.add_space(theme::SP_1_5);
}

/// 選べる値の並び（1 つだけ選ぶ・AX は RadioButton と選んだか）。押した値を返す（今の値を押しても None）。
fn choices<T: Copy + PartialEq>(ui: &mut egui::Ui, now: T, options: &[(&str, T)]) -> Option<T> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SP_1;
        for (label, value) in options {
            let on = *value == now;
            let r = theme::button_compact(ui, label, if on { Variant::Selected } else { Variant::Ghost });
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, *label));
            if r.clicked() && !on {
                picked = Some(*value);
            }
        }
    });
    picked
}

/// 段 2 作り直し（owner 142）: Terminal = 見出し `CLI & skill for agents`・説明 1 行・`generated::setup_line(.., true)`（状態の文と `Remove` だけ・button は panel の 1 行目）。
/// `setup` が None（Project の口が無い起動 = 保存先を開けなかった）なら理由と次の一手の 1 行だけ（押せる物を置かない）。
fn terminal_section(ui: &mut egui::Ui, setup: Option<&mut crate::terminal::AgentSetup>) {
    let t = theme::tokens(ui.ctx());
    field_heading(
        ui,
        "CLI & skill for agents",
        "Agents you start in a Shibubu terminal (claude, codex, gemini, grok) can read and change the project with the shibubu command. Shibubu adds a short skill only to the agents you pick",
    );
    match setup {
        Some(setup) => crate::generated::setup_line(ui, setup, true),
        None => {
            theme::label(ui, Type::Helper, "Shibubu couldn't open its saved projects in this launch, so agents can't use the shibubu command · reopen Shibubu", t.text_secondary);
        }
    }
}

/// General: Appearance・Reduce motion・When Shibubu opens・Text size・Mirror smoothness（feature mirror）・Shortcuts。
fn general(ui: &mut egui::Ui, settings: &mut UiSettings) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    field_heading(ui, "Appearance", "");
    // 既にある ⌘K の theme の行と同じ intent（ctx の theme と保存を 1 か所で）
    let options = [("Match system", ThemeChoice::System), ("Light", ThemeChoice::Light), ("Dark", ThemeChoice::Dark)];
    if let Some(c) = choices(ui, settings.theme, &options) {
        out = Some(Intent::SetTheme(match c {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        }));
    }
    // CHAT-SIDE-1101（owner 204 の 2）: Project の chat の置き場（chat の帯の ⇨ / ⇩ と同じ intent・右で畳んでいても Right）
    field_heading(ui, "Chat", "Where you talk to Shibubu in a project · ⇧⌘P shows or hides it");
    if let Some(p) = choices(ui, settings.chat_place, &[("Bottom", ChatPlace::Bottom), ("Right sidebar", ChatPlace::Right)]) {
        out = Some(Intent::ChatPlace(p));
    }
    field_heading(ui, "Reduce motion", "Shorter fades and no sliding · Match system follows your Mac's Accessibility setting");
    if let Some(v) = choices(ui, settings.reduce_motion, &[("Match system", None), ("On", Some(true)), ("Off", Some(false))]) {
        settings.reduce_motion = v;
        out = Some(Intent::SettingsChanged);
    }
    field_heading(ui, "When Shibubu opens", "");
    if let Some(v) = choices(ui, settings.reopen_last_project, &[("Home", false), ("Last project", true)]) {
        settings.reopen_last_project = v;
        out = Some(Intent::SettingsChanged);
    }
    field_heading(ui, "Text size", "");
    let now = text_zoom(settings);
    ui.horizontal(|ui| {
        let step = |ui: &mut egui::Ui, icon: Icon, name: &str, to: u16, can: bool| {
            ui.add_enabled_ui(can, |ui| theme::icon_button(ui, icon, name)).inner.clicked().then_some(to)
        };
        let smaller = step(ui, Icon::Minus, "Smaller text", now.saturating_sub(10), now > ZOOM_MIN);
        theme::label(ui, Type::Body, format!("{now}%"), t.text_primary);
        let larger = step(ui, Icon::Plus, "Larger text", now + 10, now < ZOOM_MAX);
        ui.add_space(theme::SP_3);
        theme::label(ui, Type::Small, "⌘+ ⌘− ⌘0", t.text_tertiary);
        if let Some(v) = smaller.or(larger) {
            let v = v.clamp(ZOOM_MIN, ZOOM_MAX);
            settings.text_zoom = (v != 100).then_some(v);
            out = Some(Intent::SettingsChanged);
        }
    });
    // MIRROR-SPEED-1101（owner 205「秒/枚数の方法でいくならそれ設定で変えられるように」）: Mirror の映像の枚数（helper へは app.rs の当て直し）
    #[cfg(feature = "mirror")]
    {
        use crate::model::MirrorSmoothness;
        field_heading(ui, "Mirror smoothness", "Auto lowers the frame rate when your Mac is busy · Smooth shows 60 frames a second · Saver shows 15 to save battery and heat");
        let options = [("Auto", MirrorSmoothness::Auto), ("Smooth", MirrorSmoothness::Smooth), ("Saver", MirrorSmoothness::Saver)];
        if let Some(v) = choices(ui, settings.mirror_smoothness, &options) {
            settings.mirror_smoothness = v;
            out = Some(Intent::SettingsChanged);
        }
    }
    field_heading(ui, "Shortcuts", "");
    for (keys, what) in SHORTCUTS {
        ui.horizontal(|ui| {
            theme::column_label(ui, keys, Type::Body, t.text_secondary, 72.0);
            theme::label(ui, Type::Body, what, t.text_primary);
        });
    }
    out
}

/// Text size の範囲（％）。外の値は読む時に詰める（`model::UiSettings::text_zoom`）。
pub const ZOOM_MIN: u16 = 80;
pub const ZOOM_MAX: u16 = 150;

/// 保存した文字の大きさ（％・範囲に詰めた値）。app.rs の当て直しと画面が同じ値を読む。
pub fn text_zoom(settings: &UiSettings) -> u16 {
    settings.text_zoom.unwrap_or(100).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// 読むだけの鍵の表（app.rs の `keyboard` が取る鍵と同じ並び）。
const SHORTCUTS: [(&str, &str); 10] = [
    ("⌘K", "Open a command or project"),
    ("⌘N", "New project"),
    ("⌘,", "Settings"),
    ("⌘↵", "Create or change from the field"),
    ("⌘Z", "Undo"),
    ("⇧⌘Z", "Redo"),
    ("⌃⌘S", "Show or hide the sidebar"),
    ("⇧⌘P", "Show chat"),
    // L1-NORAIL: ⌃` は下の panel の今の tab（既定 = Chat）の開閉・Terminal を足すのは ⌃⇧`（terminal_panel の `apply`）
    ("⌃`", "Show or hide the panel (Chat or terminal)"),
    ("⌃⇧`", "New terminal"),
];

/// AI & Models: Accounts（RQ2）・見つかった AI の行・plan の使用量（RQ2）・Execution limit。
fn ai_models(
    ui: &mut egui::Ui,
    settings: &mut UiSettings,
    ai: &[Connection],
    used: Option<(u32, Duration)>,
    usage: Option<(&str, Option<(&'static str, Intent)>)>,
    busy: Option<&str>,
    emails: &[(String, String)],
    unsigned: &[String],
    own: Option<&shell::OwnPanel>,
) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut out = accounts(ui, settings, busy, emails, unsigned);
    field_heading(ui, "AI on this Mac", "");
    theme::rule(ui);
    for c in ai {
        crate::ui::connections::row(ui, c);
    }
    // RQ-USAGE: plan の % と戻る時刻（model を呼ばない・読んでいない間は行を描かない）・隣に次の一手
    if let Some((line, step)) = usage {
        ui.add_space(theme::SP_1_5);
        ui.horizontal(|ui| {
            theme::label(ui, Type::Helper, line, t.text_secondary);
            if let Some((name, intent)) = step {
                if theme::button_compact(ui, name, Variant::Ghost).clicked() {
                    out = Some(intent);
                }
            }
        });
    }
    // ACCOUNTS-1101 2 段目（作る物 6）: Shibubu の中で使った量（帯の panel と同じ部品 = 同じ数字・account ごとの内訳）
    if let Some(own) = own {
        ui.add_space(theme::SP_3);
        shell::own_usage_breakdown(ui, own);
        if let Some(i) = shell::own_usage_footer(ui, own) {
            out = Some(i);
        }
    }
    field_heading(ui, "Execution limit", "The limit for one request · it starts over each time you send something · the AI stops at either limit");
    let budget = Budget::resolve(settings.llm_max_calls, settings.llm_max_secs);
    let mut calls = budget.max_calls.min(100);
    let mut secs = budget.max_total.as_secs().min(3600);
    egui::Grid::new("kangae.settings.limit").num_columns(2).spacing(Vec2::new(theme::SP_3, theme::SP_2)).show(ui, |ui| {
        theme::label(ui, Type::Body, "Calls", t.text_primary);
        if theme::ax_name(ui.add(egui::DragValue::new(&mut calls).range(0..=100)), "Calls").changed() {
            settings.llm_max_calls = Some(calls);
            out = Some(Intent::SettingsChanged);
        }
        ui.end_row();
        theme::label(ui, Type::Body, "Seconds", t.text_primary);
        if theme::ax_name(ui.add(egui::DragValue::new(&mut secs).range(0..=3600)), "Seconds").changed() {
            settings.llm_max_secs = Some(secs);
            out = Some(Intent::SettingsChanged);
        }
        ui.end_row();
    });
    // 戻す値が在る時だけ押せる形で出す（押しても何も変わらない button を置かない）
    if settings.llm_max_calls.is_some() || settings.llm_max_secs.is_some() {
        ui.add_space(theme::SP_1_5);
        if theme::button_compact(ui, "Use default", Variant::Ghost).clicked() {
            (settings.llm_max_calls, settings.llm_max_secs) = (None, None);
            out = Some(Intent::SettingsChanged);
        }
    }
    ui.add_space(theme::SP_1_5);
    theme::label(ui, Type::Helper, usage_line(used, Budget::resolve(settings.llm_max_calls, settings.llm_max_secs)), t.text_secondary);
    out
}

/// RQ2（RQ-ACCT・RQ-API）: Accounts の節。`Claude Code`（None）と `settings.ai_accounts` の行（使っている行に `In use`・他は `Use`・plan の行に `Re-authenticate`・
/// 足した行に `Remove`〈行内の確認〉）、下に `Add account`・`Paste API key` と、env に鍵が在る時だけ `Use key from environment`。鍵の値は描かない（末尾 4 文字だけ）。
/// `busy` の間は `Add account` / `Re-authenticate` の代わりにその 1 行（2 本目の sign in を起こさない・押して何も起きない button を置かない）。
/// 契約（ACCOUNTS-1101・owner 203）: AI ごとの塊にし、Connections の AI providers からも同じ物を呼ぶ（2 か所で違う見た目にしない・spec の 4）。上から
/// Claude（Claude Code の sign in = System default・plan・API key・環境の鍵）→ 見つかった AI（`AiAccount::Found` の順・各塊に `Found` の行 = System default と、その AI の足した
/// account = `Plan { id: "<provider>-<n>" }`）→ 見つかっていない AI の足した account（CLI を消した Mac・Remove できるように）。塊の見出し = AI の名前と `+ Add account`
/// （`Intent::AddAiAccount(provider)`・Claude は今までの browser の sign in・`busy` の間は出さない）か、分けられない AI の理由の 1 行（`llm_cli::one_account_line`）。
/// 行: 使っている行に `In use`（緑）・`unsigned` と sign in が要る Found の行は赤の `Needs sign-in` と `Sign in`（`Intent::UseAiAccount` = 下の terminal の sign in）・
/// 他は `Use`・Claude の plan に `Re-authenticate`・足した行に `Remove`（行内の確認）。各塊の先頭（AI 自身の sign in）の行の名は `System default`（Orca と同じ語・
/// 見出しと同じ名を 2 回並べない）・AX は AI の名で（`Use Grok`・塊が違っても名が重ならない）。例: `settings::accounts(ui, &store.ui, busy, &emails, &unsigned)`。
/// Wave: RQ2, ACCOUNTS-1101
pub fn accounts(ui: &mut egui::Ui, settings: &UiSettings, busy: Option<&str>, emails: &[(String, String)], unsigned: &[String]) -> Option<Intent> {
    use crate::model::AiAccount;
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    field_heading(ui, "Accounts", "The AI uses the account marked In use · a switch applies from the next request");
    let confirm_id = egui::Id::new("kangae.settings.remove_account");
    // 確認の印は種類と id の組（`plan/env`・id だけだと重なった 2 つの行が両方確認を出す）
    let confirming: Option<String> = ui.data(|d| d.get_temp(confirm_id));
    let env_last4 = crate::llm_cli::env_key_last4();
    let active = settings.active_account();
    // ACCOUNTS-1101: AI ごとの塊の順 = Claude → 見つかった AI（列の順）→ 見つかっていない AI の足した account
    let mut providers: Vec<&str> = vec!["claude"];
    for a in settings.ai_accounts.iter().filter(|a| matches!(a, AiAccount::Found { .. })).chain(&settings.ai_accounts) {
        if !providers.contains(&a.provider()) {
            providers.push(a.provider());
        }
    }
    for provider in providers {
        let claude = provider == "claude";
        let addable = claude || crate::llm_cli::account_env(provider, std::path::Path::new("")).is_some();
        ui.add_space(theme::SP_2);
        ui.horizontal(|ui| {
            // TITLEBAR-ICONS-1101 の 3: 塊の見出しの左に AI の icon（Terminal の tab と同じ表 = `agent_kind`・PROVIDER-ICONS-1101 で logo → Mac の app → 頭文字 = `theme::provider_icon`・Connections の行と同じ大きさ）
            let title = if claude { "Claude" } else { crate::llm_cli::found_name(provider) };
            let (_, at) = ui.allocate_space(Vec2::splat(theme::ICON_UI));
            theme::provider_icon(ui, at.center(), provider, title, t.text_primary);
            theme::label(ui, Type::Helper, title, t.text_tertiary);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Claude の Add account は browser の sign in の途中は出さない（2 本目を起こさない）
                if addable && (!claude || busy.is_none()) {
                    let name = if claude { "Claude".to_owned() } else { crate::llm_cli::found_name(provider).to_owned() };
                    if theme::ax_name(theme::button_compact(ui, "+ Add account", Variant::Ghost), &format!("Add {name} account")).clicked() {
                        out = Some(Intent::AddAiAccount(provider.to_owned()));
                    }
                }
            });
        });
        if let Some(line) = crate::llm_cli::one_account_line(provider) {
            theme::label(ui, Type::Helper, line, t.text_tertiary);
        }
        theme::rule(ui);
        // (account・名前・説明・sign in が要る)。Claude の塊の先頭は Claude Code（None）
        let mut rows: Vec<(Option<&AiAccount>, String, String, bool)> = Vec::new();
        if claude {
            rows.push((None, "Claude Code".into(), "Claude Code's sign-in on this Mac · uses your plan".into(), false));
        }
        for a in settings.ai_accounts.iter().filter(|a| a.provider() == provider) {
            let needs = unsigned.iter().any(|id| id == a.id());
            let (name, help): (String, String) = match a {
                // ACCOUNTS-1101 2 段目（作る物 5）: 名前 = 付けた名前 → email の @ の前 → `Claude 2`（email そのものは出さない・前は `Claude plan · {email}`）
                AiAccount::Plan { id } => {
                    let email = emails.iter().find(|(i, _)| i == id).map(|(_, e)| e.as_str());
                    let name = crate::llm_cli::account_name(a, settings.account_name(a), email).or_else(|| crate::llm_cli::numbered_name(a)).unwrap_or_else(|| id.clone());
                    let help = if claude {
                        "Uses that plan"
                    } else if needs {
                        "Sign in needed"
                    } else {
                        "Signed in · kept by Shibubu"
                    };
                    (name, help.into())
                }
                AiAccount::Key { last4, .. } => (format!("API key ····{last4}"), "Billed per request by Anthropic".into()),
                AiAccount::EnvKey => (
                    format!("Key from environment ····{}", env_last4.as_deref().unwrap_or("····")),
                    match env_last4 {
                        Some(_) => "Billed per request by Anthropic",
                        // 在るのに末尾が読めない = Claude Code で断った鍵（`llm_cli::child_env` は渡さない・反証の直し）
                        None if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.trim().is_empty()) => "This key was declined in Claude Code · your plan is used instead",
                        None => "The key isn't in the environment anymore · Remove it",
                    }
                    .into(),
                ),
                AiAccount::Found { name, id, needs_sign_in } => (
                    name.clone(),
                    if *needs_sign_in {
                        "Sign in needed".into()
                    } else if crate::llm_cli::LOCAL_LLMS.iter().any(|(i, _)| *i == id.as_str()) {
                        "Running on this Mac".into()
                    } else {
                        "Its own sign-in on this Mac".into()
                    },
                ),
            };
            rows.push((Some(a), name, help, needs || matches!(a, AiAccount::Found { needs_sign_in: true, .. })));
        }
        // G84（operator 10-29 ②）: 使っている行と Use は account そのもの（種類と id の組）で（id だけだと手で書いた Plan{id:"env"} と EnvKey が重なる）
        for (acct, name, help, needs) in rows {
            let key = acct.map(|a| format!("{}/{}", a.kind(), a.id()));
            let in_use = active == acct;
            ui.horizontal(|ui| {
                ui.add_space(theme::SP_3);
                ui.vertical(|ui| {
                    // 塊の先頭 = AI 自身の sign in（Claude Code・見つかった CLI / local LLM）は `System default`（operator 11-01・Orca と同じ語）
                    let shown = if matches!(acct, None | Some(AiAccount::Found { .. })) { "System default" } else { name.as_str() };
                    // ACCOUNTS-1101 2 段目: 足した account（Plan）の名前は押すと行内で付け替える
                    match (acct, &key) {
                        (Some(a @ AiAccount::Plan { .. }), Some(k)) => {
                            if let Some(i) = rename_cell(ui, k, &name, a) {
                                out = Some(i);
                            }
                        }
                        _ => {
                            theme::label(ui, Type::Body, shown, t.text_primary);
                        }
                    }
                    theme::label(ui, Type::Helper, &help, if needs { t.danger } else { t.text_secondary });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let (Some(a), Some(key)) = (acct, &key) {
                        // 見つかった CLI と local LLM は利用者の login を消さない（Remove / Re-authenticate を出さない）
                        if !matches!(a, AiAccount::Found { .. }) && theme::ax_name(theme::button_compact(ui, "Remove", Variant::Ghost), &format!("Remove {name}")).clicked() {
                            ui.data_mut(|d| d.insert_temp(confirm_id, key.clone()));
                        }
                        if claude && matches!(a, AiAccount::Plan { .. }) && busy.is_none() && theme::ax_name(theme::button_compact(ui, "Re-authenticate", Variant::Ghost), &format!("Re-authenticate {name}")).clicked() {
                            out = Some(Intent::ReauthAiAccount(a.clone()));
                        }
                    }
                    // ACCOUNTS-1101: sign in が要る行は赤の `Needs sign-in` と `Sign in`（下の terminal で・使っている行でも）・使っている行は緑の `In use`・他は `Use`
                    if needs {
                        if theme::ax_name(theme::button_compact(ui, "Sign in", Variant::Primary), &format!("Sign in to {name}")).clicked() {
                            out = Some(Intent::UseAiAccount(acct.cloned()));
                        }
                        theme::label(ui, Type::Small, "Needs sign-in", t.danger);
                    } else if in_use {
                        theme::label(ui, Type::Small, "In use", t.success);
                    } else if theme::ax_name(theme::button_compact(ui, "Use", Variant::Ghost), &format!("Use {name}")).clicked() {
                        out = Some(Intent::UseAiAccount(acct.cloned()));
                    }
                });
            });
            if let (Some(a), true) = (acct, key.is_some() && confirming == key) {
                let whom = if claude { "Anthropic" } else { crate::llm_cli::found_name(provider) };
                let what = if claude { "Shibubu forgets it" } else { "Shibubu deletes its sign-in" };
                match theme::confirm_inline(ui, &format!("Remove {name}? {what} · nothing changes at {whom}")) {
                    Some(true) => {
                        out = Some(Intent::RemoveAiAccount(a.clone()));
                        ui.data_mut(|d| d.remove::<String>(confirm_id));
                    }
                    Some(false) => ui.data_mut(|d| d.remove::<String>(confirm_id)),
                    None => {}
                }
            }
            theme::rule(ui);
        }
        if !claude {
            continue;
        }
        // Claude の塊の下: browser の sign in の途中の 1 行・鍵の 2 つの手
        ui.add_space(theme::SP_1_5);
        if let Some(line) = busy {
            crate::ui::states::inline(ui, crate::ui::states::RegionState::Loading, line);
        }
        ui.horizontal(|ui| {
            if theme::button_compact(ui, "Paste API key", Variant::Ghost).clicked() {
                out = Some(Intent::PasteApiKey);
            }
            let has_env = settings.ai_accounts.iter().any(|a| *a == AiAccount::EnvKey);
            if env_last4.is_some() && !has_env && theme::button_compact(ui, "Use key from environment", Variant::Ghost).clicked() {
                out = Some(Intent::UseEnvKey);
            }
        });
        theme::label(ui, Type::Helper, "Paste API key adds the Anthropic key you copied · your plan is not used for it", t.text_tertiary);
    }
    out
}

/// ACCOUNTS-1101 2 段目（作る物 5・owner 203）: 足した account の行の名前。押すと行内の欄（今の名前が入る・40 字まで）・Enter で `Intent::RenameAiAccount`
/// （空 = 既定の名前に戻す）・Esc と外を押すと取り消し。打ちかけは `ui.data` の (行の key, 字)。AX = Button `Rename {name}`・欄 = `Name for {name}`。
fn rename_cell(ui: &mut egui::Ui, key: &str, name: &str, acct: &crate::model::AiAccount) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let edit_id = egui::Id::new("kangae.settings.rename_account");
    let field_id = egui::Id::new(("kangae.settings.rename_field", key));
    let editing: Option<(String, String)> = ui.data(|d| d.get_temp(edit_id));
    let Some((_, mut text)) = editing.filter(|(k, _)| k == key) else {
        let r = ui.add(egui::Label::new(Type::Body.text(name).color(t.text_primary)).sense(egui::Sense::click()));
        let r = theme::ax_name(r, &format!("Rename {name}")).on_hover_text("Click to rename · Enter keeps it");
        if r.clicked() {
            ui.data_mut(|d| d.insert_temp(edit_id, (key.to_owned(), name.to_owned())));
            ui.memory_mut(|m| m.request_focus(field_id));
        }
        return None;
    };
    let r = ui.add(egui::TextEdit::singleline(&mut text).id(field_id).font(Type::Body.font()).desired_width(240.0).char_limit(40));
    let r = theme::ax_name(r, &format!("Name for {name}"));
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(edit_id, (key.to_owned(), text.clone())));
    }
    if !r.lost_focus() {
        return None;
    }
    ui.data_mut(|d| d.remove::<(String, String)>(edit_id));
    ui.input(|i| i.key_pressed(egui::Key::Enter)).then(|| Intent::RenameAiAccount(acct.clone(), text))
}

/// Execution limit の下の 1 行（使った分と上限・上限に着いたら止まった理由と次の一手）。止まったかを先に見る
/// （上限 0 は `Budgeted::send` が 1 回目の前に止めるので、使った分が 0 のまま「止まった」になる）。
pub fn usage_line(used: Option<(u32, Duration)>, budget: Budget) -> String {
    // TALK-1101 追記 2: 上限は 1 つの頼みごと = 次の一言で数え直す（上限 0 は数え直しても呼ばないので言わない）
    let or_next = |limit: u64| if limit == 0 { "" } else { ", or send your next request" };
    match used {
        None => "No AI calls yet".into(),
        Some((n, _)) if n >= budget.max_calls => format!("Stopped at {} calls · raise Calls{}", budget.max_calls, or_next(budget.max_calls.into())),
        Some((_, d)) if d >= budget.max_total => format!("Stopped at {} seconds · raise Seconds{}", budget.max_total.as_secs(), or_next(budget.max_total.as_secs())),
        Some((0, _)) => "No AI calls yet".into(),
        Some((n, d)) => format!("{n} of {} calls · {} of {} seconds used by your last request", budget.max_calls, d.as_secs(), budget.max_total.as_secs()),
    }
}

/// Privacy（RQ6・RQ-022）: `mirror` = Mirror に映してよいと押した app の bundle id（`Generated::mirror_consents`）。`allowed` = AI に任せてよいと押した app の bundle id（`surface::act_consents`・名前順）。1 行 1 app（`AI may control it`）と `Revoke`
/// （AX `Revoke {bundle id}` = `Intent::RevokeConsent`）。0 件は `No app is allowed yet · Shibubu asks the first time AI needs one`。
fn privacy_section(ui: &mut egui::Ui, allowed: &[String], mirror: &[String]) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    field_heading(ui, "Apps you allowed", "AI works only in the apps listed here · Revoke takes the permission back");
    if allowed.is_empty() && mirror.is_empty() {
        theme::label(ui, Type::Helper, "No app is allowed yet · Shibubu asks the first time AI needs one", t.text_secondary);
    }
    // RQ6: Mirror に映してよいと押した app も同じ 1 か所で（AX 名は `Revoke {bundle id} in Mirror`）
    for bundle in mirror {
        ui.horizontal(|ui| {
            theme::label(ui, Type::Body, format!("{bundle} · Mirror may show it and send your typing"), t.text_primary);
            if theme::ax_name(theme::button_compact(ui, "Revoke", Variant::Ghost), &format!("Revoke {bundle} in Mirror")).clicked() {
                out = Some(Intent::RevokeMirrorConsent(bundle.clone()));
            }
        });
    }
    for bundle in allowed {
        ui.horizontal(|ui| {
            theme::label(ui, Type::Body, format!("{bundle} · AI may control it"), t.text_primary);
            if theme::ax_name(theme::button_compact(ui, "Revoke", Variant::Ghost), &format!("Revoke {bundle}")).clicked() {
                out = Some(Intent::RevokeConsent(bundle.clone()));
            }
        });
    }
    // RQ6（RQ-025）: AI に渡る物を先に言う（見せ方だけ・承認の門ではない）。言う事は `MacGui::read` の除外と一致させる
    field_heading(ui, "What AI sees", "When you ask for something in an app you allowed");
    for line in ["Your request, in your words", "The names and text of the controls in the window you chose", "Not password fields, and not the windows of password managers", "Never your keys or sign-ins"] {
        theme::label(ui, Type::Body, line, t.text_secondary);
    }
    out
}

/// About: 版・OS・Mac・AI・checks・数の行と、2 つの copy。
fn about_section(ui: &mut egui::Ui, about: &[(String, String)]) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    ui.add_space(theme::SP_3);
    for (k, v) in about {
        ui.horizontal(|ui| {
            theme::column_label(ui, k, Type::Helper, t.text_tertiary, 176.0);
            ui.add(egui::Label::new(Type::Body.text(v.as_str()).color(t.text_primary)).selectable(true).wrap());
        });
        ui.add_space(theme::SP_1);
    }
    ui.add_space(theme::SP_3);
    theme::label(ui, Type::Helper, "Copies to the clipboard only · paths, emails and keys are hidden · nothing is sent", t.text_secondary);
    ui.add_space(theme::SP_1_5);
    ui.horizontal(|ui| {
        if theme::button_compact(ui, "Copy usage report", Variant::Ghost).clicked() {
            out = Some(Intent::CopyUsageReport);
        }
        if theme::button_compact(ui, "Copy diagnostics", Variant::Ghost).clicked() {
            out = Some(Intent::CopyDiagnostics);
        }
    });
    out
}

/// RQ3: Data & Recovery の form（`draw` の doc の「契約（RQ3）」の並び）。押した物の Intent・`History` の保存期間を変えたら `SettingsChanged`。
fn data_section(ui: &mut egui::Ui, settings: &mut UiSettings, data: Option<&crate::generated::DataSummary>, confirm: &mut Option<DataKind>) -> Option<Intent> {
    use crate::domain::size_line;
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    let Some(d) = data else {
        field_heading(ui, "Saved in", "");
        theme::label(ui, Type::Body, "Checking what Shibubu keeps", t.text_secondary);
        return None;
    };
    field_heading(ui, "Saved in", "Everything Shibubu keeps on this Mac is in this folder");
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let shown = match home.as_deref().and_then(|h| d.location.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => d.location.display().to_string(),
    };
    // 長い path は行の幅で切る（button を押し出さないよう button は次の行）
    ui.add(egui::Label::new(Type::Body.text(shown).color(t.text_primary)).selectable(true).truncate());
    if theme::button_compact(ui, "Reveal in Finder", Variant::Ghost).clicked() {
        out = Some(Intent::RevealInFinder(d.location.clone()));
    }
    // RQ3 反証の直し: 毎日の写しは開いた時に 1 回（前の daily から 24 時間）・更新の前の写しは保存の形が変わる更新の時だけ（`Generated::open` の ②④）
    field_heading(ui, "Backups", "A copy is kept when you open Shibubu each day and before an update changes how projects are saved · a restore happens when Shibubu reopens");
    if theme::button_compact(ui, "Back up now", Variant::Ghost).clicked() {
        out = Some(Intent::BackUpNow);
    }
    ui.add_space(theme::SP_1_5);
    if let Some(id) = &d.restoring {
        let what = d.backups.iter().find(|(b, _)| b == id).map_or(id.as_str(), |(_, line)| line.as_str());
        ui.horizontal(|ui| {
            theme::label(ui, Type::Body, format!("Restores when Shibubu reopens · {what}"), t.text_primary);
            if theme::ax_name(theme::button_compact(ui, "Cancel", Variant::Ghost), "Cancel the restore").clicked() {
                out = Some(Intent::RestoreBackup(None));
            }
        });
    }
    if d.backups.is_empty() {
        theme::label(ui, Type::Helper, "No backups yet · Back up now makes the first one", t.text_secondary);
    }
    for (id, line) in &d.backups {
        ui.horizontal(|ui| {
            theme::label(ui, Type::Body, line, t.text_primary);
            let damaged = id.ends_with("-damaged") || id.starts_with("sessions.json.corrupt-");
            let label = if damaged { "Try restoring" } else { "Restore" };
            if d.restoring.as_deref() != Some(id.as_str()) && theme::ax_name(theme::button_compact(ui, label, Variant::Ghost), &format!("{label} {line}")).clicked() {
                out = Some(Intent::RestoreBackup(Some(id.clone())));
            }
        });
    }
    field_heading(ui, "Projects", "Export a project from its ⌄ menu or ••• on Home · the file leaves out keys, sign-ins and the apps it uses");
    for (name, bytes) in &d.projects {
        size_row(ui, name, &size_line(*bytes));
    }
    ui.add_space(theme::SP_1_5);
    if d.imports.is_empty() {
        theme::label(ui, Type::Helper, "Put a .shibubu file in Downloads to import it", t.text_secondary);
    }
    for file in &d.imports {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if theme::button_compact(ui, &format!("Import {name}"), Variant::Ghost).clicked() {
            out = Some(Intent::ImportProject(file.clone()));
        }
    }
    field_heading(ui, "What Shibubu keeps", "Delete removes it from this Mac · your folders, files and other apps are not touched");
    theme::rule(ui);
    for kind in DataKind::ALL {
        let bytes = d.kept.iter().find(|(k, _)| *k == kind).map_or(0, |(_, b)| *b);
        let kept = match kind {
            DataKind::History => format!("Kept {} days · up to 10 MB per log", settings.history_days.filter(|d| [7, 30, 90].contains(d)).unwrap_or(30)),
            DataKind::ScreenRecords => "Kept 7 days · up to 10 MB per log".into(),
            DataKind::Backups => "7 daily · 10 made by you · 3 before updates and restores".into(),
            _ => "Kept until you delete it".into(),
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                theme::label(ui, Type::Body, kind.label(), t.text_primary);
                theme::label(ui, Type::Helper, format!("{} · {kept}", size_line(bytes)), t.text_secondary);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ax_name(theme::button_compact(ui, "Delete…", Variant::Ghost), &format!("Delete {}…", kind.label())).clicked() {
                    *confirm = Some(kind);
                }
            });
        });
        if kind == DataKind::History {
            if let Some(v) = choices(ui, settings.history_days.unwrap_or(30), &[("7 days", 7), ("30 days", 30), ("90 days", 90)]) {
                settings.history_days = (v != 30).then_some(v);
                out = Some(Intent::SettingsChanged);
            }
        }
        if *confirm == Some(kind) {
            match theme::confirm_inline(ui, kind.effect()) {
                Some(true) => {
                    out = Some(Intent::DeleteMyData(kind));
                    *confirm = None;
                }
                Some(false) => *confirm = None,
                None => {}
            }
        }
        theme::rule(ui);
    }
    ui.add_space(theme::SP_3);
    theme::label(ui, Type::Helper, "Copied diagnostics and usage reports stay on your clipboard until you copy something else", t.text_tertiary);
    out
}

/// RQ3: 名前と大きさの 1 行（読むだけ）。
fn size_row(ui: &mut egui::Ui, name: &str, size: &str) {
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        theme::label(ui, Type::Body, name, t.text_primary);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::label(ui, Type::Helper, size, t.text_secondary));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RQ1: Execution limit の下の 1 行。止まった理由は着いた方の上限で言う。負の対照: 回数の腕を外す → 2 つ目が赤。
    #[test]
    fn rq1_usage_line_says_used_and_stopped() {
        let b = Budget { max_calls: 10, max_total: Duration::from_secs(600) };
        assert_eq!(usage_line(None, b), "No AI calls yet");
        assert_eq!(usage_line(Some((10, Duration::from_secs(5))), b), "Stopped at 10 calls · raise Calls, or send your next request");
        assert_eq!(usage_line(Some((2, Duration::from_secs(600))), b), "Stopped at 600 seconds · raise Seconds, or send your next request");
        assert_eq!(usage_line(Some((3, Duration::from_secs(42))), b), "3 of 10 calls · 42 of 600 seconds used by your last request");
        assert_eq!(usage_line(Some((0, Duration::ZERO)), b), "No AI calls yet");
        // 反証の直し: 上限 0（保存した 0 = 呼ばない）は 1 回目の前に止まる = 使った分は 0 のまま。「まだ呼んでいない」ではなく止まった理由を言う
        let zero_calls = Budget { max_calls: 0, max_total: Duration::from_secs(600) };
        assert_eq!(usage_line(Some((0, Duration::ZERO)), zero_calls), "Stopped at 0 calls · raise Calls");
        let zero_secs = Budget { max_calls: 10, max_total: Duration::ZERO };
        assert_eq!(usage_line(Some((0, Duration::ZERO)), zero_secs), "Stopped at 0 seconds · raise Seconds");
    }

    /// SKILL-KEYS: Shortcuts の表が今の鍵を言う（⌃` = 今の tab の開閉で既定は Chat・⌃⇧` = Terminal を足す）。文が言う事を本物の panel の `apply` で確かめる。
    /// Settings › Terminal の「まだ入っていない」の文は Install の行への道（表と同じ ⌃⇧`）を言う。負の対照: ⌃` の文を「Show or hide the terminal」に戻す → 赤。
    #[test]
    fn skill_keys_shortcuts_say_panel_and_new_terminal() {
        use crate::ui::terminal_panel::{PanelAct, TerminalPanel};
        let what = |k: &str| SHORTCUTS.iter().find(|(key, _)| *key == k).map(|(_, w)| *w);
        assert_eq!(what("⌃`"), Some("Show or hide the panel (Chat or terminal)"));
        assert_eq!(what("⌃⇧`"), Some("New terminal"));
        let mut p = TerminalPanel::default();
        p.apply(PanelAct::Toggle, || unreachable!("既定の ⌃` が Terminal を足した"), |_| false);
        assert!(p.open && p.chat && p.tabs.is_empty(), "既定の ⌃` が Chat を開かない");
        p.apply(PanelAct::Add, || ("t1".into(), "/".into()), |_| false);
        assert!(p.open && !p.chat && p.tabs.len() == 1, "⌃⇧` が Terminal の tab を足して開かない");
        assert!(crate::generated::SETUP_NOT_YET.contains("(+ › New terminal or ⌃⇧`)"), "Settings › Terminal の文が Install の行への道を言わない");
    }
}

