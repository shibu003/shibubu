//! Header = macOS の題の帯の 1 段（§1.1・FX4 で 56 → 44・TITLEBAR-ICONS-1101 で 36 にして赤黄緑と同じ段へ）: 赤黄緑の席・◧・Shibubu の印・現在地
//! （nav 13/20・最大 `Tier::location_w`・Project の間は `theme::dropdown` の menu）・中央 = Project の間は Page の帯（`band`・L1-NORAIL）・他は ⌘K の間だけ 1 つの欄
//! （Home 以外・Home は主領域の縦の中央 = G83・32 高・幅 `Tier::input_w`・右端に Enter の結果の 1 語）・右端の ◨。`•••` は FX4 で・`✦` は L1-NORAIL で消した
//! （提案は下の panel の Chat の列の頭・在る時だけ）。帯の空いた所は窓を動かす・double-click で拡大（`draw` の TITLEBAR-ICONS-1101 の契約）。

use crate::domain::icon::Icon;
use crate::ui::ax::AxTextInput;
use super::shell::{AppRow, Intent, PageIntent, SettingsSection, Shell, Tier, View};
use super::theme::{self, Type, Variant};
use eframe::egui::{self, Rect, Vec2};

/// `menu_extra`（`•••` の中に app.rs が足した互換 view の複製・削除）は FX4 の Interface の段で消した（下の「署名」）。
/// 契約（UI-4a から・引数 `apps`・`busy` は UI-4a の Interface の段で足した）: 言葉の欄はこの 1 つだけ（`TextEdit::singleline(&mut shell.command)` を
/// 字面で 1 回 = `command_field`。Home の textarea と Page の「Change this project」の欄は UI-4a で消した = `d1_one_field`）。置き場は Home 以外の View で header の中央
/// （`Rect::from_center_size(header の rect の中央, vec2(Tier::current(ctx).input_w, theme::COMMAND_H))`。header は sidebar より先に描く窓の全幅の panel なので、
/// 欄の x・y・幅は View と sidebar で動かない = 決定 1）。Home では header に欄を描かない（`home::draw` が主領域の縦の中央に描く・G83・owner 10-27）。placeholder = `shell.view.field_hint()`・AX の名前 = `field_name()`・右端 = `shell.enter_label(apps)`・
/// Enter / ⌘↵ / 右端の button = `shell.enter_intent(cmd, apps)`。`Generate` を送った時だけ欄を空にする（`Page(Edit)` は app.rs が `Generated::take_edit_applied` で空にする）。
/// `busy`（AI が一言 / 変更を読んでいる・CLI を確かめている = `model_busy_line().is_some() || waiting`）の間は欄を灰にして送らない。`shell.focus_command` なら
/// focus を置いて false に戻す。現在地の前の `○` は描かない（Ready は記号なし・G14 B5）。作る順（= Tab の順）: 現在地 → 欄 → 右端 → `✦` → `•••`。
/// 例（UI-4a〜UI-6a・FX4 の前）: `header::draw(ctx, &mut shell, &location, back.as_deref(), 0, &rows, busy, &mut extra)`。
/// 保証しない: 欄の list（⌘K は今の modal の palette）・IME の変換中の Enter の守り（透明な title bar は TITLEBAR-ICONS-1101 で入れた）。
/// 契約（UI-4b）: 左端（`Shibubu` の前・作る順 = Tab の頭）に sidebar を畳む / 戻す icon button（28 = `theme::icon_button`・`Icon::PanelLeft`）を 1 つ。
/// tooltip は `shell.sidebar_open` で `Hide sidebar · ⌃⌘S` / `Show sidebar · ⌃⌘S`（表 additions.md:302-303）・AX の名前は ` · ` の前の語・押すと `Intent::ToggleSidebar`。
/// 畳んでも header は窓の全幅の panel のままなので、button と欄の rect は sidebar の開閉で動かない（UI-4 の Done 9）。保証しない: 開閉で icon の形を替える事（1 つの icon）。
/// 契約（UI-6a = sendback 73・76・78・operator 21:4x の決定・表 copy.en.md:18 `(removed; items moved to menus)`）: 右端の `•••`
/// （egui の `menu_button`・Appearance の 3 つと `menu_extra` の旧 canvas の項目）を描かない。項目は ⌘K の palette の行へ（`palette::commands` の契約）。
/// `menu_extra` は呼ばれなくなる（引数を消すのは yc/ui-5a の `header::draw` の署名の変更〈`back`〉と同じ merge の後・2 つの列で同時に署名を変えない）。
/// これで Diagram の Page で menu が WebView の下に隠れる穴（73）と Project の画面の `•••` が 2 つ（76）が前提ごと消える。作る順（Tab）は `✦` で終わる。
/// 契約（UI-5a・INTEG-UI-b で UI-4a〜6a の署名に足した）: `back` は title line の `‹` の席（SPEC §2 の B4）: `Some(戻る先の Page の名前)`（`Generated::back_target`）なら
/// 場所の名前の左に `‹` の icon button（28・AX の名前 `Back to {Page}`）を描き、押すと `Intent::Page(PageIntent::Back)`。`None` でも席の幅 28 は取る
/// （場所の名前の x を動かさない・SPEC §2 Fixed edges）。席の分だけ場所の名前の幅（`Tier::location_w`）を詰める = 名前の右端と欄の間は UI-4a のまま。保証しない: ⌘[（UI-5b の鍵）。
/// 署名（FX4〈FX-019〉の Interface の段で替えた）: 引数 `menu_extra` を消した（UI-6a の契約の「yc/ui-5a の `back` の merge の後」が済んだ・旧 canvas の
/// `Duplicate canvas` / `Delete canvas…` は UI-6a から ⌘K の行 = `palette::commands`）。`•••` は FX4 の本体で消した。
/// 例: `header::draw(ctx, &mut shell, &location, back.as_deref(), 0, &rows, busy)`。
/// 契約（FX4）: ① FX-019 右端の `•••`（`menu_button`）を描かない = 右の群は `✦` だけ（Tab の順は `✦` で終わる・Appearance は ⌘K の
/// `Appearance · …` と RQ1 の General）。この doc の頭の「`•••`」と UI-6a の「描かない」を実際に合わせる。② FX-018 header の高さは `theme::HEADER_H`
/// （FX4 で 44・TITLEBAR-ICONS-1101 で 36 = 透明な題の帯の中）。欄は header の縦の中央（32 の欄・上下 2）。保証しない: 960 幅より狭い窓の並び。
/// Wave: UI-4a, UI-4b, UI-5a, UI-6a, M2, FX4
/// 契約（RQ2・署名は変えない・本体で描く）: `View::Project` の間、場所の名前は `{name} ⌄` の menu（AX の名前 `Project options`・幅は今の `location_w` の中）:
/// 上に全 Project（`apps` の Project の行・Archive を除く・今の Project は選んだ印・`needs` が 1 以上の行は右に `{n} needs you`・押すと `Intent::Go(row.view)`）、
/// 区切りの下に `Rename`（`shell.renaming` を立てて 1 つの欄へ）・`Duplicate`（`Intent::DuplicateProject`）・`Project settings`（L1-NORAIL: `Intent::Go(View::Settings(SettingsSection::About))` =
/// Settings › About の頭に `Generated::project_settings_rows`）・`Delete…`（`shell.project_confirm` を立てるだけ・確認の行は Project の頭）。場所の右に、今の Project 以外の
/// `needs` の合計が 1 以上なら静かな `{n} needs you`（押すと同じ menu を開く・0 なら描かない）。Home・Runs・Connections・Settings では `⌄` を描かない（今の場所の名前のまま）。
/// 保証しない: location の幅を超える名前の全文（今の tooltip のまま）。
/// RQ3: `{name} ⌄` の区切りの下の `Duplicate` の次に `Export…`（`Intent::ExportProject`・`project_menu` が描く。`menu_extra` は `•••` の中なので使わない）。
/// `Restore diagram from before`（`Intent::RestoreDiagrams`）は描かない（`PageWebViews::restore_diagrams` が未実装・押せて動かない行を出さない）。
/// Wave: UI-4a, UI-4b, UI-5a, UI-6a, M2, RQ2, RQ3
/// 署名（L1-NORAIL・diagrams.md の節）: `proposals`（✦ の dot）を消し、`band` を足した。置き場の決まり（窓に欄は 1 つ = `d1_one_field`）:
/// Project の間 = 中央に `band`（`Generated::draw_page_band` = Page の tab と ↶ ↷ ＋ •••）・欄は下の panel の Chat の tab（app.rs）・Rename の間だけ欄を場所の名前の席に
/// （Enter = `RenameProject`・Esc = 戻す）・⌘K の palette が開いている間だけ欄を中央に（list は欄に付く）。他の View = ⌘K の間だけ中央に欄
/// （UI-SWEEP-1101 で常の `Search commands and projects` の欄を消した・Connections の文の欄は行の下 = CONNECTIONS-1101・Home は描かない）。
/// 例（app.rs）: `header::draw(ctx, &mut shell, &location, back.as_deref(), &rows, busy, &mut |ui| generated.draw_page_band(ui, &project, &page))`。
/// 保証しない: 右の rail の入口（L1-NORAIL で rail ごと消した・会話は ⇧⌘P = `Intent::OpenChat`）。
/// 契約（CHAT-SIDE-1101 追記 210・owner 210 の 5「左と同じくアイコン欲しい」）: 引数 `chat_right` を足した。`Some(shown)` = Project の間（`shown` = chat を右の sidebar に
/// 出している = `ChatPlace::Right`）で、header の右端（Page の帯の ↶ ＋ ••• の右・作る順 = Tab の最後）に左の `Icon::PanelLeft` と同じ大きさ・形の左右反対
/// `Icon::PanelRight` を 1 つ。AX の名前 = `Show chat on the right`（`shown` = false）/ `Hide right sidebar`（true）・押すと `Intent::ChatPlace(Right / Bottom)`
/// （しまうと panel ごと下に戻る・追記 219: 右に置いて閉じた panel も「出していない」= 押すと右で開く）。`None`（Home・Runs・Connections・Settings = 右の chat が無い）は描かない。
/// 例（app.rs）: `header::draw(ctx, &mut shell, &location, back.as_deref(), &rows, busy, chat_right, &mut band)`。
/// ⇧⌘P は右に出している間だけ ◨ と同じ（下へ・追記 219）。下の間の ⇧⌘P は Chat の tab を開く。
/// 契約（TITLEBAR-ICONS-1101 の 1・1'・2'〈案 A〉・owner 216・218・署名は変えない）: header は macOS の題の帯の 1 段（`theme::HEADER_H` = 本体で 36・Orca の
/// `.titlebar-left`）。窓は中身を題の帯の下まで伸ばし題の字を隠す（main.rs）・赤黄緑は帯の縦の中央（`surface::AppKitOp::TrafficLights`・app.rs が毎 frame）。
/// 並び（= Tab の順）: 赤黄緑の席 `theme::TRAFFIC_PAD`（全画面の間は `SP_2`）→ ◧ → Shibubu の印（透明の icon 16 ＋「Shibubu」・押すと Home）→ ‹ の席 →
/// Project の間は `theme::dropdown(ui, Some(Icon::Folder), name)`（AX `Project options`・⌄ は 1' の icon）・他の View は場所の名前 → `band`（Page の tab ＋ ＋・
/// ↶ ↷ •••・`Generated::draw_page_band`）→ 右端の ◨。header の下に Page の帯の段は無い（中身はすぐ下から）。
/// 帯の空いた所（どの widget も載っていない所）を押して動かすと `ViewportCommand::StartDrag`（窓が動く）・button・tab・⌄・◧ の上で押した時は押す方が勝つ
/// （動かしても窓は動かない）。double-click は系の設定どおり（`double_click`・`AppKitOp::DoubleClickAction`）。帯の地の interact は子より先に 1 回（後に描く物が上）。
/// 保証しない: 赤黄緑を寄せられなかった時の縦のずれ（AppKit の既定の位置 = 28 の帯の中央のまま・4 px 上）・960 幅より狭い窓。
/// Wave: UI-4a, UI-4b, UI-5a, UI-6a, M2, FX4, TITLEBAR-ICONS-1101
pub fn draw(
    ctx: &egui::Context,
    shell: &mut Shell,
    location: &str,
    back: Option<&str>,
    apps: &[AppRow],
    busy: bool,
    chat_right: Option<bool>,
    band: &mut dyn FnMut(&mut egui::Ui) -> Option<Intent>,
) -> Option<Intent> {
    let t = theme::tokens(ctx);
    let tier = Tier::current(ctx);
    let mut intent = None;
    // 毎 frame 最初に描く panel = View の替わり目を主領域より先に知る所（app.rs は替わり目を渡さない）
    super::shell::note_view(ctx, &shell.view);
    // TITLEBAR-ICONS-1101: 左は赤黄緑の席（全画面の間は赤黄緑が消えるので詰める）
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    let pad = if fullscreen { theme::SP_2 } else { theme::TRAFFIC_PAD };
    egui::TopBottomPanel::top("kangae-header")
        .exact_height(theme::HEADER_H)
        .show_separator_line(true)
        .frame(egui::Frame::new().fill(t.surface_1).inner_margin(egui::Margin { left: pad as i8, right: theme::SP_2 as i8, top: 0, bottom: 0 }))
        .show(ctx, |ui| {
            // 帯の中は全部 inner の中に置く（欄・右の群は rect で置く）。item_spacing.y が残ると 2 つ目の組（右の群）の後ろで panel の中身が 56 を超え、
            // 超えた分は clip されて塗られないまま sidebar と主領域がその下から始まる = 窓の幅いっぱいの太い黒い線（W15-c2・r11 以後の全 View）
            ui.spacing_mut().item_spacing.y = 0.0;
            let inner = ui.max_rect();
            // TITLEBAR-ICONS-1101（owner 216）: 帯の地 = 窓の題の帯（赤黄緑の席も含む窓の幅いっぱい）。子より先に 1 回 = 後に描く button と tab が勝つ。
            // keyboard の focus は取らない（FOCUSABLE を外す = Tab の頭は ◧ のまま・AX の木に名の無い node を足さない）
            let full = Rect::from_x_y_ranges(ui.ctx().screen_rect().x_range(), inner.y_range());
            let bar_id = egui::Id::new("kangae-titlebar");
            let bar = ui.interact(full, bar_id, egui::Sense::CLICK | egui::Sense::DRAG);
            // 押した所の一番上の押せる物が地か（= 何も載っていない所）を押した frame に覚える。egui の hit test は click だけの button の下の地にも drag を
            // 渡す（button と ScrollArea の組の為）ので、button の上から動かすと地の drag_started が来る = これで見分ける
            if ui.input(|i| i.pointer.primary_pressed()) {
                let on_bar = ui.ctx().viewport(|v| v.hits.click.map(|w| w.id)) == Some(bar_id);
                ui.ctx().data_mut(|d| d.insert_temp(bar_id, on_bar));
            }
            // CHAT-SIDE-1101 追記 210: 右端の ◨ の席（Page の帯はその左まで）
            let edge = if chat_right.is_some() { inner.right() - theme::BUTTON_H - theme::SP_2 } else { inner.right() };
            // 作る順 = Tab の順: sidebar の button → 現在地 → 中央（欄 / Project は Page の帯）→ 右端
            let in_project = matches!(shell.view, View::Project { .. });
            let left = ui.horizontal_centered(|ui| {
                let tip = if shell.sidebar_open { "Hide sidebar · ⌃⌘S" } else { "Show sidebar · ⌃⌘S" };
                let toggle = theme::icon_button(ui, Icon::PanelLeft, tip);
                if theme::ax_name(toggle, tip.split(" · ").next().unwrap_or(tip)).clicked() {
                    intent = Some(Intent::ToggleSidebar);
                }
                ui.add_space(theme::SP_1);
                // TITLEBAR-ICONS-1101（owner 216「Home（Shibubu の印）」・Orca の logo 16 ＋ app の名前）: 透明の icon（icon は 2 つだけ = Home の chat bar の上と同じ物）と語。
                // どちらを押しても Home。AX は語の 1 つ（icon は飾り = 名前を持たない）
                let mark = super::home::brand_icon(ui.ctx()).map(|tex| {
                    let size = tex.size_vec2();
                    ui.add(egui::Image::from_texture((tex.id(), Vec2::new(theme::ICON_UI * size.x / size.y, theme::ICON_UI))).sense(egui::Sense::click()))
                });
                let brand = ui.add(egui::Label::new(Type::HeadingSm.text("Shibubu").color(t.text_primary)).sense(egui::Sense::click()).selectable(false)); // TEXT-SELECT-1101: Home へ行く button の字は選ばない
                if brand.clicked() || mark.is_some_and(|m| m.clicked()) {
                    intent = Some(Intent::Go(View::Home));
                }
                ui.add_space(theme::SP_4);
                // UI-5a: ‹ の席（戻る先が無くても幅 28 を取る = 場所の名前の x を動かさない）
                match back {
                    Some(page) => {
                        if theme::icon_button(ui, Icon::ChevronLeft, &format!("Back to {page}")).clicked() {
                            intent = Some(Intent::Page(PageIntent::Back));
                        }
                    }
                    // icon button と同じ取り方（28 四方 ＋ 後ろの item_spacing）。add_space は item_spacing を足さないので x がずれる
                    None => drop(ui.allocate_exact_size(Vec2::splat(theme::BUTTON_H), egui::Sense::hover())),
                }
                // 現在地は画面の題（見出しと同じく選べる = AX の elements に名前で出る）。長い名前は location_w で切る（欄と重ならない・‹ の席の分を詰める）
                let location_w = (tier.location_w - theme::BUTTON_H - ui.spacing().item_spacing.x).max(0.0);
                ui.allocate_ui(Vec2::new(location_w, ui.available_height()), |ui| {
                    // RQ2（RQ-005・RQ-006）: Project の間は `{name} ⌄` の menu（他の View は今の場所の名前のまま）
                    let View::Project { project, .. } = shell.view.clone() else {
                        ui.add(egui::Label::new(Type::Nav.text(location).color(t.text_secondary)).truncate().selectable(true));
                        return;
                    };
                    // L1-NORAIL: Rename の間は 1 つの欄を場所の名前の席に（下の panel の Chat の欄の行は 1 行の文になる）
                    if shell.renaming.is_some() && !shell.palette.open {
                        let at = Rect::from_min_size(egui::pos2(ui.cursor().left(), ui.max_rect().center().y - theme::COMMAND_H / 2.0), Vec2::new(location_w, theme::COMMAND_H));
                        if let Some(i) = command_field(ui, at, shell, apps, busy) {
                            intent = Some(i);
                        }
                        ui.advance_cursor_after_rect(at);
                        return;
                    }
                    ui.horizontal_centered(|ui| {
                        // ponytail: 長い名前は字数で切る（location_w の中の button・全文は menu の行と Settings で読める）
                        let shown: String = if location.chars().count() > 28 { location.chars().take(27).chain(['…']).collect() } else { location.to_owned() };
                        // TITLEBAR-ICONS-1101 の 1'（owner 218）: folder の icon ＋ 名前 ＋ 大きな ⌄（1 つの menu = Project・Page は右に並んだ tab）
                        let opener = theme::ax_name(theme::dropdown(ui, Some(Icon::Folder), &shown), "Project options");
                        // AX の名前は `Project options` のまま（BG の driver と試験が引く）・どの Project かは AX の help（VoiceOver が続けて読む）に
                        ui.ctx().accesskit_node_builder(opener.id, |n| n.set_description(location));
                        let others: usize = apps.iter().filter(|a| !a.archived && a.project_id().as_deref() != Some(project.as_str())).map(|a| a.needs).sum();
                        // `{n} needs you` は opener と 1 つの Response にして開く（別に toggle_popup すると、mouse の click は opener の外 = 同じ frame の
                        // CloseOnClickOutside で閉じ直されて何も起きない・AX の Click では起きないので BG の試験では見えなかった）
                        let opener = if others > 0 { opener.union(theme::button_compact(ui, &format!("{others} needs you"), Variant::Ghost)) } else { opener };
                        if let Some(i) = theme::menu_popup(ui, &opener, |ui| project_menu(ui, apps, &project)).and_then(|act| act.apply(shell, &project, location)) {
                            intent = Some(i);
                        }
                    });
                });
            })
            .response
            .rect;
            if in_project && !shell.palette.open {
                // L1-NORAIL: Project の中央は Page の帯（場所の名前の右から右端まで・縦は header の中央）
                let at = Rect::from_min_max(
                    egui::pos2(left.right() + theme::SP_4, inner.center().y - theme::TAB_H / 2.0),
                    egui::pos2(edge, inner.center().y + theme::TAB_H / 2.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    if let Some(i) = band(ui) {
                        intent = Some(i);
                    }
                });
            } else if shell.view != View::Home && shell.palette.open {
                // UI-SWEEP-1101（owner 196 通目の 11・12「一番上の chatbar 無くして・search command とか意味わからない」）: Runs・Connections・Settings・App では
                // 常の欄（Search commands and projects）を描かない。⌘K の list（欄に付く）の間だけ header の中央に出す（Connections の文の欄は行の下 = CONNECTIONS-1101）。
                // 置き場は前と同じ = sidebar で x・y・幅が動かない（決定 1）。Home は主領域の縦の中央（G83・home::draw）
                let field_rect = Rect::from_center_size(inner.center(), Vec2::new(tier.input_w, theme::COMMAND_H));
                if let Some(i) = command_field(ui, field_rect, shell, apps, busy) {
                    intent = Some(i);
                }
            }
            // CHAT-SIDE-1101 追記 210（owner 210 の 5）: 右端の ◨ = 左の ◧ の左右反対・右の sidebar（Chat）を出す / しまう（しまうと下へ）
            if let Some(shown) = chat_right {
                let at = Rect::from_center_size(egui::pos2(inner.right() - theme::BUTTON_H / 2.0, inner.center().y), Vec2::splat(theme::BUTTON_H));
                let name = if shown { "Hide right sidebar" } else { "Show chat on the right" };
                if ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| theme::icon_button(ui, Icon::PanelRight, name)).inner.clicked() {
                    use crate::model::ChatPlace;
                    intent = Some(Intent::ChatPlace(if shown { ChatPlace::Bottom } else { ChatPlace::Right }));
                }
            }
            // TITLEBAR-ICONS-1101（owner 216「空いた所を drag すると窓が動く・double-click で拡大」）: macOS の題の帯と同じ（winit の drag_window = performWindowDragWithEvent・
            // set_maximized = zoom:）。button の上の click は button が取る（地は下）・button の上から始めた drag は窓を動かさない（押した frame の印）
            if bar.drag_started_by(egui::PointerButton::Primary) && ui.ctx().data(|d| d.get_temp::<bool>(bar_id)).unwrap_or(false) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                #[cfg(target_os = "macos")]
                let action = crate::surface::appkit(crate::surface::AppKitOp::DoubleClickAction).unwrap_or_default();
                #[cfg(not(target_os = "macos"))]
                let action = String::new();
                let zoomed = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                if let Some(cmd) = double_click(&action, zoomed) {
                    ui.ctx().send_viewport_cmd(cmd);
                }
            }
        });
    intent
}

/// TITLEBAR-ICONS-1101 の 1: 題の帯の double-click の手（`action` = 系の設定 `AppleActionOnDoubleClick` の字・`zoomed` = 今拡大しているか）。
/// `Minimize` → Dock へしまう・`None` → 何もしない・`Maximize`・`Fill`・空（設定していない）・知らない字 → 拡大の切り替え（macOS の既定）。
/// 呼ぶ側の例（`draw` の帯）: `if let Some(cmd) = double_click(&action, zoomed) { ctx.send_viewport_cmd(cmd) }`。
/// 保証しない: `Fill`（macOS 15 の「画面を埋める」）を zoom と別の大きさにする事（winit の口が zoom: だけ）。
/// Wave: TITLEBAR-ICONS-1101
pub(crate) fn double_click(action: &str, zoomed: bool) -> Option<egui::ViewportCommand> {
    match action {
        "Minimize" => Some(egui::ViewportCommand::Minimized(true)),
        "None" => None,
        _ => Some(egui::ViewportCommand::Maximized(!zoomed)),
    }
}

/// RQ2: Project の menu（`{name} ⌄` と Home の行の `•••`）で選んだ物（Rename と Delete… は Shell の印を立てるだけ・`Close` は `⌄` の今の Project の行 =
/// menu を閉じるだけ・他は Intent）。
pub(super) enum ProjectAct {
    Rename,
    Delete,
    Close,
    Do(Intent),
}

impl ProjectAct {
    /// 選んだ物を当てる: Rename → 1 つの欄を名前の欄に（`name` = 今の名前）・Delete… → 行内の確認を立てる・Close → 何もしない・他は呼ぶ側へ返す Intent。
    pub(super) fn apply(self, shell: &mut Shell, project: &str, name: &str) -> Option<Intent> {
        match self {
            ProjectAct::Rename => shell.start_rename(project.to_owned(), name.to_owned()),
            ProjectAct::Delete => shell.project_confirm = Some(project.to_owned()),
            ProjectAct::Close => {}
            ProjectAct::Do(i) => return Some(i),
        }
        None
    }
}

/// RQ2・RQ3: Project の操作の項目 = Rename・Duplicate・Export…・Show folder in Finder・Change folder…（PROJECT-DIR-1101）・`extra`（呼ぶ側の項目）・Delete…（`⌄` と `•••` が同じ順・同じ AX の名前で描く）。
pub(super) fn project_actions(ui: &mut egui::Ui, project: &str, extra: &[(&str, Intent)]) -> Option<ProjectAct> {
    let item = |ui: &mut egui::Ui, label: &str, v: Variant| theme::button_menu_item(ui, label, "", v).clicked();
    let mut picked = None;
    if item(ui, "Rename", Variant::Ghost) {
        picked = Some(ProjectAct::Rename);
    }
    // PROJECT-DIR-1101: その Project の folder（Terminal が始まる所）を開く・選び直す
    let common = [
        ("Duplicate", Intent::DuplicateProject(project.to_owned())),
        ("Export…", Intent::ExportProject(project.to_owned())),
        ("Show folder in Finder", Intent::ShowProjectFolder(project.to_owned())),
        ("Change folder…", Intent::ChangeProjectFolder(project.to_owned())),
    ];
    for (label, i) in common.iter().chain(extra) {
        if item(ui, label, Variant::Ghost) {
            picked = Some(ProjectAct::Do(i.clone()));
        }
    }
    if item(ui, "Delete…", Variant::Danger) {
        picked = Some(ProjectAct::Delete);
    }
    picked
}

/// RQ2: `{name} ⌄` の menu の中身 = 全 Project（Archive を除く・今の Project は `Current`・`{n} needs you`）・区切り・`project_actions`（`extra` = Project settings）。
fn project_menu(ui: &mut egui::Ui, apps: &[AppRow], current: &str) -> Option<ProjectAct> {
    let mut picked = None;
    for row in apps.iter().filter(|a| !a.archived) {
        let Some(id) = row.project_id() else { continue };
        let here = id == current;
        let hint = match (here, row.needs) {
            (true, _) => "Current".to_owned(),
            (false, 0) => String::new(),
            (false, n) => format!("{n} needs you"),
        };
        if theme::button_menu_item(ui, row.name.trim(), &hint, Variant::Ghost).clicked() {
            // 今の Project の行は既に居る所 = menu を閉じるだけ（macOS の menu で印の付いた項目を選んだ時と同じ）
            picked = Some(if here { ProjectAct::Close } else { ProjectAct::Do(Intent::Go(row.view.clone())) });
        }
    }
    theme::rule(ui);
    // L1-NORAIL: Project settings は Settings › About（頭に今の Project の 3 行 = `UiSettings::last_project`）
    project_actions(ui, current, &[("Project settings", Intent::Go(View::Settings(SettingsSection::About)))]).or(picked)
}

/// 言葉の 1 つの欄を `rect` に描く（TextEdit・AX の名前 `shell.view.field_name()`・右端の `shell.enter_label(apps)`・Enter / ⌘↵ / 右端の button = `shell.enter_intent`）。
/// 契約（G83・`header::draw` の欄の部分を切り出した。中身は UI-4a のまま）: 窓に 1 つ = 毎 frame どこか 1 か所だけが呼ぶ（`TextEdit::singleline` の構築は
/// この関数の 1 回 = `d1_one_field`）。置き場は呼ぶ側が決める（Home 以外 = `header::draw` が header の中央・幅 `Tier::input_w`／Home = `home::draw` が
/// 主領域の縦の中央・owner 10-27）。Id は `kangae-command` の 1 つ（置き場で替わらない）・置いた rect を骨格へ（`skeleton::note_command_field`）。`busy` の間は灰にして送らない・
/// `shell.focus_command` なら focus を置いて false に戻す・`Generate` と `Talk`（L2）を送った時だけ欄を空にする（Home の添付は `Generate` の時だけ外す）・
/// Home の Enter・⌘↵・右端の `Send` は `Talk`（L2 = 会話・作らない・送った後も focus は欄）。例: `header::command_field(ui, rect, &mut shell, &rows, busy)`。
/// 保証しない: `rect` が `ui` の中に在る事（呼ぶ側）。
/// 契約（FX4・署名は変えない）: ① FX-016 `shell.palette.open` の間は Enter・⌘↵・右端の button で `enter_intent` を送らない（↑↓ と Enter は
/// ⌘K の list = `palette::draw` が取る）・placeholder は `Type a command or project name`（AX の名前 `field_name()` は変えない = labels.py の `goal` / `change`）・
/// 右端の 1 語は描かない（list の選んだ行が Enter の結果）。② FX-020 IME: この欄の Id の上で `Event::Ime(Preedit(空でない))` を見てから `Commit` / `Disabled` /
/// 空の `Preedit` までを「変換中」として ctx の temp に覚え、変換中の Enter と、`Commit` が来た frame の Enter（macOS は確定の Enter と Commit が同じ frame に来る）は
/// 送らない（⌘↵ も同じ）。③ FX-020 空の欄で Enter を押したら何も送らず focus を欄に残す（singleline が Enter で手放した focus を同じ frame で戻す・
/// 右端の button の空の押下は今も focus だけ）。例は今のまま。保証しない: 4 行までの伸び（FX-058）・添付のある欄の focus（G84）。
/// 契約（HOME-CHATBAR-1101・署名は変えない）: Home の欄は `rect`（card の文の場所・`home::draw`）いっぱいの複数行（枠なし・⇧Enter = 改行・修飾なしの Enter = 送る・
/// 溢れたら `rect` の中で縦に scroll）。右端の 1 語は描かない（送るのは card の下の行の丸 = `home::draw` が `submit` を呼ぶ）。AX は role も名前も 1 行の欄のまま
/// （TextInput = AXTextField・`field_name()` = labels.py の `goal`・録画の台本と rehearse が role と名前で探す）。Home の外は今の 1 行の欄のまま。
/// 契約（CHAT-ONE-1101）: Project の Chat の chat bar（`home::chat_bar`）の欄も Home と同じ複数行（名前の欄〈Rename〉と ⌘K の list の間は header の 1 行の欄のまま）。
/// Wave: M2, FX4, HOME-CHATBAR-1101, CHAT-ONE-1101
pub fn command_field(ui: &mut egui::Ui, rect: Rect, shell: &mut Shell, apps: &[AppRow], busy: bool) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    // chat bar の中の欄（Home・Project の Chat）か。Project の名前の欄と ⌘K は header の 1 行
    let home = match shell.view {
        View::Home => true,
        View::Project { .. } => shell.renaming.is_none() && !shell.palette.open,
        _ => false,
    };
    // ⌘K の list が開いている間は右端の 1 語を描かない（list の選んだ行が Enter の結果）・Home は card の丸が送る
    let full = if shell.palette.open || home { String::new() } else { shell.enter_label(apps) };
    // 欄の右端に入りきらない ` · ⌘↵ …` 付きの label は Enter の結果だけにして、全文は hover に（枠の外へはみ出さない・CU 所見 3）
    let label = shorten_tail(&full, |s| ui.painter().layout_no_wrap(s.to_owned(), Type::Small.font(), egui::Color32::PLACEHOLDER).size().x + 2.0 * theme::BUTTON_COMPACT_PAD_X, TAIL_MAX_W);
    let is_button = label.starts_with('↵') || label.starts_with("Create");
    // 右端の 1 語の幅（欄の右の margin は i8 なので 120 で切り、長い文は省略して tooltip に全文）
    let tail_w = if label.is_empty() {
        0.0
    } else {
        (ui.painter().layout_no_wrap(label.clone(), Type::Small.font(), egui::Color32::PLACEHOLDER).size().x + 2.0 * theme::BUTTON_COMPACT_PAD_X).min(TAIL_MAX_W)
    };
    let mut margin = theme::input_margin(ui, Type::Nav, theme::COMMAND_H);
    margin.right = margin.right.max((tail_w + 2.0 * theme::SP_1) as i8);
    // HOME-CHATBAR-1101: Home は card の中の文（左右 12〈追記 222 で 16 から 1 段詰めた = `home::text_h` と同じ幅〉・上 12 = 見本の placeholder の位置）
    if home {
        margin = egui::Margin { left: theme::SP_3 as i8, right: theme::SP_3 as i8, top: theme::SP_3 as i8, bottom: theme::SP_2 as i8 };
    }
    super::skeleton::note_command_field(ui.ctx(), rect);
    // 親の ui に依らない 1 つの Id（Home の主領域と header で同じ = Home の ⌘↵ で Project を開いても focus が欄に残る）
    let id = egui::Id::new(FIELD_ID);
    take_enter_while_composing(ui, id);
    let hint = if shell.palette.open { "Type a command or project name" } else { shell.field_hint() };
    // 窓が狭くて placeholder が右端の label の下に入り込む時は、最初の句（`,` の前）だけにする（960 幅の CU 所見: 重なって読めない）・Home の複数行は折り返す
    let text_w = rect.width() - f32::from(margin.left) - f32::from(margin.right);
    let hint = if home { hint } else { fit_hint(hint, |s| ui.painter().layout_no_wrap(s.to_owned(), Type::Nav.font(), egui::Color32::PLACEHOLDER).size().x, text_w) };
    ui.add_enabled_ui(!busy, |ui| {
        let hint = Type::Nav.text(hint).color(t.text_tertiary);
        let field = if home {
            let edit = egui::TextEdit::multiline(&mut shell.command)
                .id(id)
                .font(Type::Nav.font())
                .hint_text(hint)
                .margin(margin)
                .frame(false)
                .desired_width(f32::INFINITY)
                .desired_rows(1)
                .min_size(rect.size())
                .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                egui::ScrollArea::vertical().id_salt("kangae-command-scroll").max_height(rect.height()).auto_shrink([false, true]).show(ui, |ui| ui.add(edit)).inner
            })
            .inner
        } else {
            ui.put(rect, egui::TextEdit::singleline(&mut shell.command).id(id).font(Type::Nav.font()).hint_text(hint).margin(margin))
        };
        let field = theme::ax_name(field, shell.view.field_name()).ax_text_input(ui, &mut shell.command);
        if home {
            field.ctx.accesskit_node_builder(field.id, |n| n.set_role(egui::accesskit::Role::TextInput));
        }
        if std::mem::take(&mut shell.focus_command) {
            field.request_focus();
        }
        // Enter は singleline が focus を手放した frame・Home（複数行）は focus の間の修飾なしの Enter（⇧Enter は TextEdit が改行にする）・
        // ⌘↵ は focus を手放さない（egui の return_key は修飾なしの Enter だけ）ので focus の間に取る
        let enter = if home {
            field.has_focus() && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.is_none())))
        } else {
            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
        };
        let cmd = field.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        let mut pressed = false;
        if !label.is_empty() {
            let tail = Rect::from_min_size(
                egui::pos2(rect.right() - theme::SP_1 - tail_w, rect.center().y - theme::BUTTON_COMPACT_H / 2.0),
                Vec2::new(tail_w, theme::BUTTON_COMPACT_H),
            );
            if is_button {
                // `Create` で始まる label だけ primary（L2 から Home は `↵ Send` = ghost・決める primary は会話の案の card）。AX の名前は鍵の記号を外した語（Send・Change・Open “{name}”・Open・Run）
                let variant = if label.starts_with("Create") { Variant::Primary } else { Variant::Ghost };
                let name = label.trim_start_matches("↵ ").split(" · ").next().unwrap_or_default();
                let b = ui.put(tail, |ui: &mut egui::Ui| theme::button_sized(ui, &label, variant, Some(tail_w), theme::BUTTON_COMPACT_H));
                pressed = theme::ax_name(b.on_hover_text(full.as_str()), name).clicked();
            } else {
                ui.put(tail, egui::Label::new(Type::Small.text(label.as_str()).color(t.text_tertiary)).truncate()).on_hover_text(full.as_str());
            }
        }
        // ⌘K の list が開いている間の Enter は list が取る（`palette::draw`）。欄は focus を手放さない
        if shell.palette.open {
            if enter {
                field.request_focus();
            }
        } else if (enter || cmd || pressed) && !busy {
            intent = submit(ui, shell, apps, cmd, is_button && pressed);
        }
    });
    intent
}

/// 1 つの欄の egui の Id（Home の主領域と header で同じ）。
pub const FIELD_ID: &str = "kangae-command";

/// 欄の一言を送る（Enter・⌘↵・欄の右端の button・Home の card の送る丸が通る 1 つの道）。`cmd` = ⌘↵・`pressed` = button を押した。
/// 送った `Generate` / `Talk` は欄を空に（Home の添付は `Generate` の時だけ外す）・`Talk` と空の Enter は focus を欄へ・空の欄で button を押したら何を打つかの 1 行（`Intent::Notice`）。
/// 呼ぶ側: `command_field` と `home::draw` の送る丸（どちらも ⌘K の list の間と `busy` の間は呼ばない）。例: `header::submit(ui, shell, apps, false, true)`。
/// Wave: HOME-CHATBAR-1101（`command_field` の本体から切り出した・中身は FX4 のまま）
pub fn submit(ui: &egui::Ui, shell: &mut Shell, apps: &[AppRow], cmd: bool, pressed: bool) -> Option<Intent> {
    let focus = || ui.memory_mut(|m| m.request_focus(egui::Id::new(FIELD_ID)));
    match shell.enter_intent(cmd, apps) {
        Some(i) => {
            // Page の変更は通った時だけ空にする（app.rs が `Generated::take_edit_applied` で）。作るのは送った時に空にし、作れなければ app.rs が戻す
            if matches!(i, Intent::Generate(..) | Intent::Talk(..)) {
                shell.command.clear();
                // G104: Home の添付は作る一言に乗った時だけ外す（Talk は会話なので残す。Home の外の ⌘↵ には乗らない）
                if shell.view == View::Home && matches!(i, Intent::Generate(..)) {
                    (shell.attach, shell.attach_line) = (Vec::new(), None);
                }
            }
            // L2: 会話は送った後も続けて打つ（singleline が Enter で手放した focus を同じ frame で戻す）
            if matches!(i, Intent::Talk(..)) {
                focus();
            }
            Some(i)
        }
        // 空の欄で button を押した: 欄に focus と、何を打てばいいかの 1 行（押しても何も起きない button にしない）。
        // 空の Enter は button ではない（何も送らず、singleline が手放した focus を欄に戻す）
        None if pressed => {
            focus();
            let line = match (&shell.renaming, &shell.view) {
                (Some(_), _) => "A project needs a name · type one, or press Esc to keep the old one",
                (None, View::Project { .. }) => "Type a message first, e.g. add a Tests page",
                (None, _) => "Type what you want to create first",
            };
            Some(Intent::Notice(line.into()))
        }
        None => {
            focus();
            None
        }
    }
}

/// IME の変換中（`Preedit` が空でない間）と、確定（`Commit`）が来た frame の Enter・⌘↵ は、欄にも ⌘K の list にも渡さない（macOS は確定の Enter と `Commit` が
/// 同じ frame に来る）。欄の Id の上に focus が在る間だけ `Event::Ime` を見る。TextEdit が見る前に Enter の押下を入力から外す = 変換中の文字と focus を壊さない。
fn take_enter_while_composing(ui: &egui::Ui, id: egui::Id) {
    use egui::{Event, ImeEvent};
    let state_id = egui::Id::new("kangae.command.composing");
    let mut composing = ui.data(|d| d.get_temp::<bool>(state_id)).unwrap_or(false);
    let mut committed = false;
    if !ui.memory(|m| m.has_focus(id)) {
        // 欄が focus を手放したら変換は終わっている（`Disabled` は focus の無い間に来ても届かない）。残すと以後の Enter を全部捨てる
        composing = false;
    } else {
        for e in ui.input(|i| i.events.clone()) {
            if let Event::Ime(ime) = e {
                match ime {
                    ImeEvent::Preedit(text) => composing = !text.is_empty(),
                    ImeEvent::Commit(_) => (composing, committed) = (false, true),
                    ImeEvent::Disabled => composing = false,
                    ImeEvent::Enabled => {}
                }
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(state_id, composing));
    if composing || committed {
        ui.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { key: egui::Key::Enter, pressed: true, .. })));
    }
}

/// placeholder が `avail` に入りきらない時は最初の句（`,` の前）だけにする。入る時・句が 1 つだけの時はそのまま。
fn fit_hint<'a>(full: &'a str, width: impl Fn(&str) -> f32, avail: f32) -> &'a str {
    if width(full) > avail {
        full.split(',').next().unwrap_or(full).trim_end()
    } else {
        full
    }
}

/// 右端の label が `max` に入りきらず ` · ⌘↵ …`（FX-020 の付け足し）を持つ時は、Enter の結果の部分（` · ` の前）だけにする。AX の名前は元から ` · ` の前。
/// 入る時・付け足しの無い label（Home の `Create · ⌘↵`）・button でない文（`No command matches · …`）は変えない。
fn shorten_tail(full: &str, width: impl Fn(&str) -> f32, max: f32) -> String {
    if full.starts_with('↵') && full.contains(" · ⌘↵ ") && width(full) > max {
        full.split(" · ").next().unwrap_or(full).to_owned()
    } else {
        full.to_owned()
    }
}

/// 欄の右端の 1 語の最大幅（`TextEdit::margin` の右は i8 = 127 まで）。
const TAIL_MAX_W: f32 = 120.0;

#[cfg(test)]
mod tests {
    use super::*;
    use egui::accesskit::{Action, ActionRequest, Role};

    /// W15-c2（r11〜r20 の全写真: header の下に窓の幅いっぱいの太い黒い線）: header の下は隙間なく sidebar と主領域が始まる（panel を使い切った残りの上端 = HEADER_H）。
    /// 負の対照: `item_spacing.y = 0.0` の行を外す → 赤（隙間の分だけ上端が下がる）。
    #[test]
    fn w15c2_nothing_is_left_between_the_header_and_what_is_below_it() {
        for back in [None, Some("Docs")] {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            let mut shell = Shell::new(View::Home);
            let mut top = f32::NAN;
            for _ in 0..3 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() };
                ctx.run(input, |ctx| {
                    draw(ctx, &mut shell, "Dev", back, &[], false, None, &mut |_| None);
                    top = ctx.available_rect().top();
                });
            }
            assert_eq!(top, theme::HEADER_H, "header の下に隙間（{back:?}）");
        }
    }

    /// UI-5a: title line の `‹` の席。`Some(Page)` なら場所の名前の左に AX `Back to {Page}` の button が在り、AX の Click で `PageIntent::Back`。
    /// `None` は `‹` を描かず、場所の名前の x は `Some` と同じ（席の幅 28・SPEC §2 Fixed edges）。負の対照: None の腕の席の幅を外す → x がずれて赤。
    #[test]
    fn ui5a_back_seat_names_the_page_and_keeps_the_location_x() {
        let run = |back: Option<&str>, press: bool| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut shell = Shell::new(View::Home);
            let mut got = None;
            let mut out = None;
            let mut events = Vec::new();
            for _ in 0..3 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events: std::mem::take(&mut events), ..Default::default() };
                let o = ctx.run(input, |ctx| got = got.take().or(draw(ctx, &mut shell, "Dev", back, &[], false, None, &mut |_| None)));
                let tree = o.platform_output.accesskit_update.clone().expect("AccessKit の木");
                if press {
                    if let Some((id, _)) = tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Back to"))) {
                        events = vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: *id, data: None })];
                    }
                }
                out = Some(o);
            }
            let o = out.unwrap();
            let x = o.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Dev" => Some(t.pos.x),
                _ => None,
            });
            let names: Vec<String> = o.platform_output.accesskit_update.unwrap().nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
            (x.expect("場所の名前を描いていない"), names, got)
        };
        let (x_some, names, _) = run(Some("Page 1"), false);
        assert!(names.iter().any(|n| n == "Back to Page 1"), "‹ の AX の名前: {names:?}");
        let (x_none, names, _) = run(None, false);
        assert!(!names.iter().any(|n| n.starts_with("Back to")), "戻る先が無いのに ‹ を描いた: {names:?}");
        assert_eq!(x_some, x_none, "‹ の有無で場所の名前の x が動いた");
        let (_, _, got) = run(Some("Page 1"), true);
        assert_eq!(got, Some(Intent::Page(PageIntent::Back)));
    }

    /// FX4（FX-018・FX-019）: header は 44 高（下の sidebar と主領域は 44 から始まる）で、右端に `•••` の menu が無い。L1-NORAIL: `✦`（右の rail の入口）も無い。
    /// TITLEBAR-ICONS-1101 の 1: header は題の帯の 1 段 = 36 高（Orca の `.titlebar-left`）。負の対照: `HEADER_H` を 44 に戻す → 赤。
    #[test]
    fn fx4_titlebar_header_is_36_high_and_has_no_dots_menu() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut shell = Shell::new(View::Runs);
        let (mut top, mut names) = (f32::NAN, Vec::new());
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() };
            let o = ctx.run(input, |ctx| {
                draw(ctx, &mut shell, "Runs", None, &[], false, None, &mut |_| None);
                top = ctx.available_rect().top();
            });
            names = o.platform_output.accesskit_update.unwrap().nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
        }
        assert_eq!(top, 36.0, "header の高さ（題の帯の 1 段）");
        assert!(!names.iter().any(|n| n.contains("•••")), "header に ••• が残る: {names:?}");
        assert!(!names.iter().any(|n| n.starts_with("Chat") || n.starts_with("Suggestions")), "✦（右の rail の入口）が残る: {names:?}");
    }

    /// CHAT-SIDE-1101 追記 210（owner 210 の 5）: Project の header の右端に ◨（AX `Show chat on the right` / `Hide right sidebar`）・押すと ChatPlace::Right / Bottom。
    /// Project の外（`chat_right` = None）は描かない。負の対照: ◨ を描く腕を外す → 名前が無く赤・押した時の置き場を入れ替える → 赤。
    #[test]
    fn chat_side_210_right_edge_icon_shows_and_hides_the_right_chat() {
        use crate::model::ChatPlace;
        let run = |view: View, chat_right: Option<bool>, press: Option<&str>| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut shell = Shell::new(view);
            let (mut got, mut events, mut right, mut names) = (None, Vec::new(), None, Vec::new());
            for _ in 0..3 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events: std::mem::take(&mut events), ..Default::default() };
                let o = ctx.run(input, |ctx| got = got.take().or(draw(ctx, &mut shell, "Dev", None, &[], false, chat_right, &mut |_| None)));
                let tree = o.platform_output.accesskit_update.expect("AccessKit の木");
                names = tree.nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
                if let Some((id, n)) = tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l == "Show chat on the right" || l == "Hide right sidebar")) {
                    right = n.bounds().map(|b| b.x1);
                    if press.is_some_and(|p| n.label() == Some(p)) {
                        events = vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: *id, data: None })];
                    }
                }
            }
            (names, right, got)
        };
        let project = || View::Project { project: "project.app.a".into(), page: "page-1".into() };
        let (names, right, _) = run(project(), Some(false), None);
        assert!(names.iter().any(|n| n == "Show chat on the right"), "◨ の AX の名前: {names:?}");
        assert!(right.is_some_and(|x| x >= 1440.0 - f64::from(theme::SP_6) - 1.0), "◨ が header の右端に無い: {right:?}");
        let (names, _, _) = run(project(), Some(true), None);
        assert!(names.iter().any(|n| n == "Hide right sidebar"), "出している間の AX の名前: {names:?}");
        assert_eq!(run(project(), Some(false), Some("Show chat on the right")).2, Some(Intent::ChatPlace(ChatPlace::Right)));
        assert_eq!(run(project(), Some(true), Some("Hide right sidebar")).2, Some(Intent::ChatPlace(ChatPlace::Bottom)));
        let (names, _, _) = run(View::Runs, None, None);
        assert!(!names.iter().any(|n| n == "Show chat on the right" || n == "Hide right sidebar"), "Project の外に ◨: {names:?}");
    }

    /// TITLEBAR-ICONS-1101 の 1: 1 frame を描き、AX の名前と矩形・窓への頼み（`ViewportCommand`）を返す。`fullscreen` は eframe が入れる窓の状態の写し。
    fn titlebar_frame(ctx: &egui::Context, shell: &mut Shell, location: &str, fullscreen: bool, t: f64, events: Vec<egui::Event>) -> (Vec<(String, egui::accesskit::Rect)>, Vec<egui::ViewportCommand>) {
        let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), time: Some(t), focused: true, events, ..Default::default() };
        input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { fullscreen: Some(fullscreen), maximized: Some(false), ..Default::default() });
        let o = ctx.run(input, |ctx| drop(draw(ctx, shell, location, None, &[], false, None, &mut |_| None)));
        let tree = o.platform_output.accesskit_update.expect("AccessKit の木");
        let named = tree.nodes.iter().filter_map(|(_, n)| Some((n.label()?.to_owned(), n.bounds()?))).collect();
        (named, o.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default())
    }

    /// TITLEBAR-ICONS-1101 の 1（owner 216）: 一番上の 1 段の左端の物（◧）は赤黄緑の席（`TRAFFIC_PAD` = 80）の右から・全画面の間は赤黄緑が消えるので `SP_2` から。
    /// 負の対照: header の左の inner margin を前の `SP_6` に戻す → 1 つ目が赤・全画面で 80 を取ったまま → 2 つ目が赤。
    #[test]
    fn titlebar1101_the_first_item_starts_right_of_the_traffic_lights() {
        for (fullscreen, want) in [(false, theme::TRAFFIC_PAD), (true, theme::SP_2)] {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut shell = Shell::new(View::Runs);
            let mut named = Vec::new();
            for i in 0..3 {
                named = titlebar_frame(&ctx, &mut shell, "Runs", fullscreen, f64::from(i) * 0.1, vec![]).0;
            }
            let x0 = named.iter().find(|(l, _)| l == "Hide sidebar" || l == "Show sidebar").map(|(_, b)| b.x0).expect("◧ が AX に無い");
            assert!((x0 - f64::from(want)).abs() < 1.0, "全画面 {fullscreen}: ◧ の左端 {x0} が {want} でない");
        }
    }

    /// TITLEBAR-ICONS-1101 の 1（owner 216「空いた所を drag すると窓が動く・double-click で拡大」）: 帯の空いた所（Runs の真ん中 = 何も載っていない所）を
    /// 押して動かすと `StartDrag`・2 回続けて押すと系の設定どおりの手（`double_click`・既定 = `Maximized(true)`）。◧ を 2 回押しても窓に何もしない・
    /// ◧ の上で押して動かしても窓は動かない（押す方が勝つ・operator 24 回目の前）。
    /// 負の対照: 帯の地の interact を外す → 1・2 つ目が赤・地の interact を子の後に置く → ◧ の double-click で拡大して 3 つ目が赤・
    /// 押した frame の印（`hits.click` が地か）を見ない → 4 つ目が赤。
    #[test]
    fn titlebar1101_dragging_the_empty_bar_moves_the_window_and_a_double_click_zooms() {
        let run = |at: egui::Pos2, clicks: usize, drag: bool| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut shell = Shell::new(View::Runs);
            let (mut t, mut said) = (0.0, Vec::new());
            let mut step = |events: Vec<egui::Event>| {
                t += 0.05;
                said.extend(titlebar_frame(&ctx, &mut shell, "Runs", false, t, events).1);
            };
            let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            step(vec![]);
            step(vec![egui::Event::PointerMoved(at)]);
            for _ in 0..clicks {
                step(vec![button(true)]);
                if drag {
                    step(vec![egui::Event::PointerMoved(at + egui::vec2(40.0, 0.0))]);
                    step(vec![egui::Event::PointerMoved(at + egui::vec2(80.0, 0.0))]);
                }
                step(vec![button(false)]);
            }
            step(vec![]);
            said
        };
        let empty = egui::pos2(720.0, theme::HEADER_H / 2.0);
        assert!(run(empty, 1, true).iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag)), "空いた所の drag で窓が動かない");
        // 系の設定はこの Mac の物（Minimize / None の人もいる）= 同じ読みから期待を作る（読みの表は下の `double_click` の試験）
        #[cfg(target_os = "macos")]
        let action = crate::surface::appkit(crate::surface::AppKitOp::DoubleClickAction).unwrap_or_default();
        #[cfg(not(target_os = "macos"))]
        let action = String::new();
        let want = double_click(&action, false);
        let said = run(empty, 2, false);
        let window = |c: &&egui::ViewportCommand| matches!(c, egui::ViewportCommand::Maximized(_) | egui::ViewportCommand::Minimized(_));
        assert_eq!(said.iter().find(window), want.as_ref(), "空いた所の double-click が系の設定 {action:?} の手でない");
        let toggle = egui::pos2(theme::TRAFFIC_PAD + theme::BUTTON_H / 2.0, theme::HEADER_H / 2.0);
        assert!(!run(toggle, 2, false).iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag | egui::ViewportCommand::Maximized(_) | egui::ViewportCommand::Minimized(_))), "◧ を押して窓が動いた・拡大した");
        assert!(!run(toggle, 1, true).iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag)), "◧ の上で押して動かすと窓が動いた（押す方が勝たない）");
    }

    /// TITLEBAR-ICONS-1101 の 1（operator 24 回目の前）: 題の帯の double-click は系の設定（`AppleActionOnDoubleClick`）どおり。
    /// 負の対照: `double_click` を前の「常に `Maximized(!zoomed)`」に戻す → Minimize と None の行が赤。
    #[test]
    fn titlebar1101_double_click_follows_the_system_setting() {
        use egui::ViewportCommand as C;
        for (action, zoomed, want) in [
            ("Maximize", false, Some(C::Maximized(true))),
            ("Fill", true, Some(C::Maximized(false))),
            ("", false, Some(C::Maximized(true))),
            ("Minimize", false, Some(C::Minimized(true))),
            ("None", false, None),
        ] {
            assert_eq!(double_click(action, zoomed), want, "{action:?}（拡大中 {zoomed}）");
        }
    }

    /// TITLEBAR-ICONS-1101 の 1'（owner 218「^が小さすぎる。下向きの^ね」）: Project の名前は字だけで描き（`{name} ⌄` の 1 つの文字列にしない）、⌄ は別の icon。
    /// AX の名前は `Project options` のまま（`rq2_needs_you_…` が名前と description を見る）。負の対照: `dropdown` を前の `format!("{label} ⌄")` に戻す → 赤。
    #[test]
    fn titlebar1101_the_project_name_and_its_chevron_are_drawn_apart() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut shell = Shell::new(View::Project { project: "project.app.a".into(), page: "page.1".into() });
        let mut texts = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() };
            let o = ctx.run(input, |ctx| drop(draw(ctx, &mut shell, "Shop", None, &[], false, Some(false), &mut |_| None)));
            texts = o.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) => Some(t.galley.text().to_owned()), _ => None }).collect();
            let named: Vec<String> = o.platform_output.accesskit_update.unwrap().nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
            assert!(named.iter().any(|n| n == "Project options"), "AX の `Project options` が無い: {named:?}");
        }
        assert!(texts.iter().any(|t| t == "Shop"), "Project の名前を字だけで描いていない: {texts:?}");
        assert!(!texts.iter().any(|t| t.contains("Shop") && t.contains('⌄')), "名前と ⌄ が 1 つの文字列: {texts:?}");
    }

    /// 1 つの欄だけを headless で回す（focus は `shell.focus_command`）。frame ごとの戻り値と、最後に欄が focus を持つか。
    fn run_field(shell: &mut Shell, frames: Vec<Vec<egui::Event>>) -> (Vec<Option<Intent>>, bool) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        let mut got = Vec::new();
        for events in std::iter::once(Vec::new()).chain(frames).chain(std::iter::once(Vec::new())) {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), events, focused: true, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got.push(command_field(ui, Rect::from_min_size(egui::pos2(200.0, 300.0), Vec2::new(480.0, theme::COMMAND_H)), shell, &[], false)));
            });
        }
        (got, ctx.memory(|m| m.focused()) == Some(egui::Id::new("kangae-command")))
    }
    fn enter() -> egui::Event {
        egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// FX4（FX-020・UIUX M10）: IME の変換中の Enter と、確定（`Commit`）と同じ frame の Enter は送らない。確定の後の Enter だけが作る。
    /// 負の対照（本体の commit で取る）: `Commit` の frame を見る行を外す → 3 つ目の frame で `Generate` が出て赤。
    #[test]
    fn fx4_ime_enter_does_not_submit_while_composing() {
        use egui::{Event, ImeEvent};
        let mut shell = Shell::new(View::Home);
        let (got, _) = run_field(&mut shell, vec![
            vec![Event::Ime(ImeEvent::Enabled), Event::Ime(ImeEvent::Preedit("か".into()))],
            vec![enter()],
            vec![Event::Ime(ImeEvent::Commit("家".into())), enter()],
            vec![enter()],
        ]);
        assert_eq!(got[1..4], [None, None, None], "変換中・確定の frame の Enter で送った: {got:?}");
        assert_eq!(got[4], Some(Intent::Talk("家".into())), "確定の後の Enter が会話にならない: {got:?}");
    }

    /// FX4（FX-020・UIUX H05）: 空の欄で Enter を押しても何も送らず、focus は欄に残る（右端の button の Notice は Enter では出さない）。
    /// 負の対照: `None if is_button && pressed` を `(pressed || enter)` に戻す → 2 つ目の frame が Notice で赤。
    #[test]
    fn fx4_empty_enter_keeps_focus_in_the_field() {
        let mut shell = Shell::new(View::Home);
        let (got, focused) = run_field(&mut shell, vec![vec![enter()]]);
        assert!(got.iter().all(Option::is_none), "{got:?}");
        assert!(focused, "空の Enter で欄が focus を手放した");
    }

    /// FX4（FX-016）: ⌘K の list が開いている間、欄の Enter は欄の結果（Runs = 合う command）を送らない（list の選んだ行が Enter の結果）。
    #[test]
    fn fx4_enter_belongs_to_the_list_while_it_is_open() {
        let mut shell = Shell::new(View::Runs);
        shell.focus_command = true;
        shell.command = "go to home".into();
        shell.palette.open = true;
        let (got, _) = run_field(&mut shell, vec![vec![enter()]]);
        assert!(got.iter().all(Option::is_none), "list が開いているのに欄が送った: {got:?}");
    }

    /// FX4（CU 所見 3）: 欄の右端に入りきらない `↵ Change · ⌘↵ New project` は `↵ Change` だけにする（全文は hover）。入る時と付け足しの無い label は変えない。
    /// 負の対照（commit message）: `shorten_tail` の幅の比べを外す → 1 つ目の assert が赤。
    #[test]
    fn fx4_a_long_right_end_label_is_cut_to_the_enter_result() {
        let w = |s: &str| s.chars().count() as f32 * 7.0;
        assert_eq!(shorten_tail("↵ Change · ⌘↵ New project", w, 120.0), "↵ Change");
        assert_eq!(shorten_tail("↵ Change · ⌘↵ New project", w, 400.0), "↵ Change · ⌘↵ New project", "入るのに切った");
        assert_eq!(shorten_tail("Create · ⌘↵", w, 20.0), "Create · ⌘↵", "付け足しの無い label を切った");
        assert_eq!(shorten_tail("No command matches · ⌘↵ to create a project from this", w, 20.0), "No command matches · ⌘↵ to create a project from this", "button でない文は切らない（次の一手を言う）");
    }

    /// FX4（CU 所見 960 幅 ①）: 入りきらない placeholder `Change this project, e.g. add a Tests page` は `Change this project` だけにする。入る時と句が 1 つだけの時は変えない。
    /// 負の対照（commit message）: `fit_hint` の幅の比べを外す → 1 つ目の assert が赤。
    #[test]
    fn fx4_a_placeholder_that_does_not_fit_keeps_its_first_phrase() {
        let w = |s: &str| s.chars().count() as f32 * 8.0;
        let full = "Change this project, e.g. add a Tests page";
        assert_eq!(fit_hint(full, w, 200.0), "Change this project");
        assert_eq!(fit_hint(full, w, 600.0), full, "入るのに切った");
        assert_eq!(fit_hint("What do you want to create?", w, 20.0), "What do you want to create?", "句が 1 つだけなのに切った");
    }

    /// RQ2 反証の直し: 場所の右の `{n} needs you` を本物の mouse の 1 click（動かす → 押す → 離す・AX の Click ではない）で押すと Project の list が開く
    /// （AX の Click は pointer の click を立てないので、閉じ直しの穴は AX の試験では見えない）。今の Project の行は menu を閉じるだけ・`⌄` の AX は名前を help に持つ。
    /// 負の対照（commit message）: opener と `needs you` の union をやめて前の `toggle_popup` に戻す → mouse で開かず赤。
    #[test]
    fn rq2_needs_you_opens_the_project_list_with_a_mouse_click() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let here = View::Project { project: "project.app.a".into(), page: "page.1".into() };
        let row = |app: &str, name: &str, needs: usize| AppRow { view: View::App(app.into()), name: name.into(), purpose: String::new(), status: String::new(), running: 0, needs, pinned: false, archived: false, opened_at: None, sample: false };
        let apps = [row("app.a", "Shop", 0), row("app.b", "Harbor", 1)];
        let mut shell = Shell::new(here);
        let mut t = 0.0;
        let mut frame = |shell: &mut Shell, events: Vec<egui::Event>| {
            t += 0.1;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), time: Some(t), focused: true, events, ..Default::default() };
            let mut got = None;
            let o = ctx.run(input, |ctx| got = draw(ctx, shell, "Shop", None, &apps, false, None, &mut |_| None));
            (o.platform_output.accesskit_update.expect("AccessKit の木"), got)
        };
        let node = |tree: &egui::accesskit::TreeUpdate, name: &str| tree.nodes.iter().find(|(_, n)| n.label() == Some(name)).map(|(_, n)| n.clone());
        let mut tree = frame(&mut shell, vec![]).0;
        for _ in 0..2 {
            tree = frame(&mut shell, vec![]).0;
        }
        let opener = node(&tree, "Project options").expect("`⌄` が AX に無い");
        assert_eq!(opener.description(), Some("Shop"), "`⌄` の AX から今の Project の名前が消えた");
        let needs = node(&tree, "1 needs you").and_then(|n| n.bounds()).expect("`1 needs you` が AX に無い");
        let at = egui::pos2(((needs.x0 + needs.x1) / 2.0) as f32, ((needs.y0 + needs.y1) / 2.0) as f32);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&mut shell, vec![egui::Event::PointerMoved(at)]);
        frame(&mut shell, vec![button(true)]);
        frame(&mut shell, vec![button(false)]);
        let tree = frame(&mut shell, vec![]).0;
        assert!(ctx.memory(|m| m.any_popup_open()) && node(&tree, "Rename").is_some(), "mouse で `1 needs you` を押しても list が開かない");
        // 今の Project の行（`Current`）を押すと閉じるだけ（Intent を出さない）
        let current = tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Shop"))).map(|(id, _)| *id).expect("今の Project の行");
        let (_, got) = frame(&mut shell, vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: current, data: None })]);
        frame(&mut shell, vec![]);
        assert!(got.is_none() && !ctx.memory(|m| m.any_popup_open()), "今の Project の行で menu が閉じない・何かを送った: {got:?}");
    }

    /// RQ2 反証の直し（holdout h3）: 空の欄の右端の button（Project = `Change`・名前の欄 = `Rename`）は何を打てばいいかの 1 行（`Intent::Notice`）を出し、欄に focus を置く。
    /// 負の対照（commit message）: `command_field` の `None if is_button && pressed` の腕を消す → None で赤。
    #[test]
    fn rq2_empty_field_button_says_what_to_type() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut shell = Shell::new(View::Project { project: "project.app.a".into(), page: "page.1".into() });
        for (name, renaming) in [("Change", false), ("Rename", true)] {
            if renaming {
                shell.start_rename("project.app.a".into(), String::new());
            }
            let mut events = Vec::new();
            let mut got = None;
            for _ in 0..3 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events: std::mem::take(&mut events), ..Default::default() };
                // L1-NORAIL: Project の欄は下の panel の Chat の欄の行（ここでは主領域に置く）・Rename の間だけ header の場所の名前の席
                let o = ctx.run(input, |ctx| {
                    got = got.take().or(draw(ctx, &mut shell, "Shop", None, &[], false, None, &mut |_| None));
                    if shell.renaming.is_none() {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let at = Rect::from_min_size(egui::pos2(200.0, 300.0), Vec2::new(480.0, theme::COMMAND_H));
                            got = got.take().or(command_field(ui, at, &mut shell, &[], false));
                        });
                    }
                });
                let tree = o.platform_output.accesskit_update.expect("AccessKit の木");
                if let Some((id, _)) = tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(name)) {
                    events = vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: *id, data: None })];
                }
            }
            match got {
                Some(Intent::Notice(line)) => assert!(!line.is_empty(), "{name}"),
                other => panic!("空の欄で `{name}` を押して 1 行が出ない: {other:?}"),
            }
        }
    }

}
