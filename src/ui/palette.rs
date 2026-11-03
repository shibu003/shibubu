//! `⌘K` の最小 command palette（UI-008）: 作る / 開く / 移動 / 提案 rail。閉じたら focus を元に戻す。

use super::shell::{AppRow, Intent, SettingsSection, Shell, View};
use super::theme::{self, Type};
use eframe::egui::{self, Rect, Vec2};

/// ⌘K の状態（`Shell::palette`）。
/// 契約（FX4〈FX-016〉）: ⌘K は 2 つ目の欄も `egui::Modal` も出さない。打つ所は 1 つの欄（`header::command_field`・Id `kangae-command`・
/// Home は主領域の 52%・他は header の中央）で、`open` の間はその欄の文（`Shell::command`）が絞り込みの語・欄のすぐ下に list が落ちる（`draw`）。
/// 絞り込みの語は 1 つの欄の文（`Shell::command`）だけが持つ（前の `query` は消した）。
/// field `stash`（FX4 で足した）: 開いた時に欄に在った打ちかけ（Home の作る文・Project の変更文）。開く時に `Shell::command` から取り出して欄を空にし、
/// 閉じる時（Esc・行を選んだ・`Shell::go`）に欄へ戻す = ⌘K で打った語が打ちかけ（`Shell::drafts`）に混ざらず、打ちかけは消えない。
/// field `recent`（FX4 で足した・UIUX N17「local history opt-in, dedup」）: `None` = 覚えない（既定）・`Some(v)` = 覚える（`v` は選んだ行の label・
/// 新しい順・同じ label は 1 つ・`RECENT_MAX` まで）。書き手 = `draw`（行を選んだ時・`Generate` の行は覚えない）と `Intent::RememberCommands`。
/// 再起動を跨ぐ保存は `model::UiSettings::recent_commands`（`Shell::drafts` ↔ `UiSettings::drafts` と同じ写し方・app.rs の `open` と `snapshot_drafts`）。
/// 保証しない: 保存期間と消す規則（RQ-014）・Settings の切り替えの行（RQ1 の General が merge された後に足す双子・今は ⌘K の行だけ）。
/// Wave: UI-4a, FX4
#[derive(Debug, Default)]
pub struct Palette {
    pub open: bool,
    selected: usize,
    restore_focus: Option<egui::Id>,
    pub(crate) stash: Option<String>,
    pub recent: Option<Vec<String>>,
}

/// ⌘K の list の頭に出す最近の行の数（UIUX N17「⌘KのRecent 3」）。
/// Wave: FX4
pub const RECENT_MAX: usize = 3;

impl Palette {
    /// ⌘K を開く: `command`（欄の打ちかけ）を `stash` へ退けて欄を空にし、1 つの欄（Id `kangae-command`）に focus を置く（新しい欄は作らない）。
    /// 例（app.rs の ⌘K）: `self.shell.palette.open(ctx, &mut self.shell.command)`。
    /// Wave: UI-4a, FX4
    pub fn open(&mut self, ctx: &egui::Context, command: &mut String) {
        self.open = true;
        self.selected = 0;
        self.stash = Some(std::mem::take(command));
        self.restore_focus = ctx.memory(|m| m.focused());
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("kangae-command")));
    }
    /// overlay を閉じて `stash` を `command` へ戻し（⌘K で打った語は捨てる）、focus を元に戻す（UI-008 の AC）。
    /// 行を選んで閉じた時も同じ（戻した後に呼ぶ側が intent を当てる = `Shell::go` が打ちかけを正しい場所の `drafts` へ預ける）。
    /// Wave: UI-4a, FX4
    pub fn close(&mut self, ctx: &egui::Context, command: &mut String) {
        self.open = false;
        if let Some(before) = self.stash.take() {
            *command = before;
        }
        if let Some(id) = self.restore_focus.take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub label: String,
    pub hint: String,
    pub intent: Intent,
}

/// canvas が 1 つの時の削除の行の label（押せない形で理由を言う・78。Page の `A project needs at least one page` と同じ型）。
const ONE_CANVAS: &str = "You need at least one canvas";

/// 押せない行（label が理由の文）。`Command` に field を足さず label で見分ける（行は `commands` が作る 1 種だけ）。
/// palette の Enter と click・header の欄の Enter（`Shell::enter_intent`）の両方がこれで飛ばす。
pub(crate) fn disabled(c: &Command) -> bool {
    c.label == ONE_CANVAS
}

/// query で絞った command 一覧。大文字小文字を無視した部分一致。
/// 何にも当たらない非空の query は「その一言でアプリを作る」になる（create が一等）。
/// 契約（UI-4a から・署名は変えない）: `New project`（hint `⌘N`）は `Go(View::Home)`（Home の欄に focus・sendback 17 / 35.10）・旧 canvas は新しい行
/// `New blank canvas`（copy.en.md:40・`Intent::NewApp`）だけ。`Go to Apps` は `View::NAV` から消える（G14 A2 / sendback 35.1）。hint の記号は `⇧⌘P`・`↵`
/// （G14 B6・copy.en.md:16）。空の時の 1 行は表どおり `No projects match`（copy.en.md:47。G14 C7 の `No matches` は表に無い = sendback 70）。
/// 1 つの欄の Enter（Runs・Connections・CompatCanvas・App）も同じ一覧の頭の行を使う（`Shell::enter_intent`・最後の `Generate` の行を除く）。
/// 契約（UI-4b・引数 `sidebar_open` は UI-4b の Interface の段で足した）: rail の行の次に `Hide sidebar`（`sidebar_open` が true）/
/// `Show sidebar`（false）の 1 行（hint `⌃⌘S`・`Intent::ToggleSidebar`・表 additions.md:302-303 の ` · ` の前と後）。sidebar を畳んでも Home・各 Project・
/// Runs・Connections の行はこの一覧に残る（owner-addendum ②「畳んでも ⌘K から行ける」・h_reach_when_folded）。
/// 例（UI-4b まで）: `commands(&palette.query, apps, shell.rail_open, shell.sidebar_open)`。保証しない: 欄の list（⌘K は今の modal の palette のまま・後）。
/// 契約（UI-6a・本体で足す = sendback 73・76・78 で header の `•••` を消した行き先）: Appearance の 3 行（`Intent::SetTheme(System | Light | Dark)`）を常に、
/// 旧 canvas の `Duplicate canvas`（`Intent::CloneApp`）と削除（`Intent::DeleteApp`）を互換 view の間だけ足す。canvas が 1 つの時の削除の行は押せない形で
/// label を理由の文にする（UI-5a の `A project needs at least one page` と同じ型・`Command` に field を足さない・候補 `You need at least one canvas`）。
/// 行の語は表の持ち主として `docs/ui-copy/additions.md` に足す（operator ⑤）。
/// 引数 `canvases`（UI-6a の Interface の段で足した・UI-4b の `sidebar_open` と同じ形）: `Some(n)` = 今が互換 view で旧 canvas が n 個（`Shell::canvases`）・
/// `None` = 互換 view の外（旧 canvas の行を出さない）。
/// 例: `commands(&palette.query, apps, shell.rail_open, shell.sidebar_open, shell.canvases)`。
/// 引数 `pages`（FX3〈FX-008〉の Interface の段で足した・`canvases` と同じ形）: 今いる Project の Page の行（`Generated::page_commands` = 全 Page の
/// `Go to {Page}`・Project の外と読めない Project は空）。旧 canvas の行の後・`Open {Project}` の行の前に、渡された順のまま足す（表の節の順 Pages → Projects）。
/// 打った文で絞るのは他の行と同じ（label か hint の部分一致）。例（app.rs）: `commands(q, apps, .., shell.canvases, &self.generated.page_commands(&project))`。
/// 保証する: 帯に 5 つしか見えない Project でも、どの Page にも ⌘K から行ける（帯の `More` と同じ行き先 = `Go(View::Project{..})`）。保証しない: 他の Project の Page の行。
/// 署名（FX4 の Interface の段で替えた）: `rail_open`・`sidebar_open`・`canvases` の 3 つを `shell: &Shell` に（3 つとも `Shell` の field の写しで、FX4 の `recent` も
/// `shell.palette.recent` から読む = 引数を 1 つずつ足し続けない）。`query` は残す（`shell.command` = 欄の文を渡す・`Shell::enter_intent` は trim した欄の文）。
/// 例: `commands(&shell, &shell.command, &rows, &pages)`。
/// 契約（FX4）: ① FX-017 `New blank canvas`（`Intent::NewApp`）の行を出さない（互換 canvas は FX-059 で消す・旧 session の行 `Open {name}` は残す）。
/// ② FX-016 `query` が空で `shell.palette.recent` が `Some` の時、頭に `recent` の label と同じ行を `recent` の順で最大 `RECENT_MAX`（今の一覧に無い label は飛ばす・
/// 同じ行を 2 度出さない = 下の元の位置から外す）。③ 覚える / 覚えないの行を 1 つ（`recent` が `None` = `Remember recent commands` →
/// `Intent::RememberCommands(true)`・`Some` = `Forget recent commands` → `RememberCommands(false)`・Appearance の行の後）。語は本体で docs/ui-copy/additions.md に足す。
/// 保証しない: 見出し（`Recent`・copy.en.md の section headings）を描く事（`draw` が描く）・打った語の候補（Page の入力候補 = N17 の後半は入れない）。
/// Wave: UI-4a, UI-4b, UI-6a, FX3, FX4
/// 契約（RQ1・署名は変えない・行は本体で足す）: `Go to {nav}` の行の次に `Settings`（hint `⌘,`・`Intent::Go(View::Settings(SettingsSection::General))`）を常に 1 行。
/// `Appearance · …` の 3 行と `Copy usage report` は残す（Settings の General / About と同じ intent・2 か所から同じ値を書く）。保証しない: 節ごとの行（⌘K から直に About へ等）。
/// 契約（RQ2・署名は変えない）: Project の検索はここ（Home の Continue に欄を置かない・`Search · ⌘K` は palette を開くだけ）。`AppRow::archived` の行も `Open {name}` に残す
/// （Archive は消さない・ここから開いて `Unarchive` で戻せる）。保証しない: 印（`needs`・`pinned`・`sample`）を行の語に入れる事（hint は今の purpose のまま）。
/// Wave: UI-4a, UI-4b, UI-6a, RQ1, RQ2
pub fn commands(shell: &Shell, query: &str, apps: &[AppRow], pages: &[Command]) -> Vec<Command> {
    let (sidebar_open, canvases) = (shell.sidebar_open, shell.canvases);
    let mut all = vec![
        Command { label: "New project".into(), hint: "⌘N".into(), intent: Intent::Go(View::Home) },
    ];
    for v in View::NAV {
        all.push(Command { label: format!("Go to {}", v.nav_label()), hint: String::new(), intent: Intent::Go(v) });
    }
    all.push(Command { label: "Settings".into(), hint: "⌘,".into(), intent: Intent::Go(View::Settings(SettingsSection::General)) });
    // L1-NORAIL: 右の rail は無い。Project の間だけ下の panel の Chat を開く行（Project の外の会話は Home の欄）
    if matches!(shell.view, View::Project { .. }) {
        all.push(Command { label: "Show chat".into(), hint: "⇧⌘P".into(), intent: Intent::OpenChat });
    }
    all.push(Command { label: if sidebar_open { "Hide sidebar" } else { "Show sidebar" }.into(), hint: "⌃⌘S".into(), intent: Intent::ToggleSidebar });
    // UI-6a（sendback 73・76・78）: header の `•••` の Appearance と旧 canvas の項目の行き先
    for (word, pref) in [("Match system", egui::ThemePreference::System), ("Light", egui::ThemePreference::Light), ("Dark", egui::ThemePreference::Dark)] {
        all.push(Command { label: format!("Appearance · {word}"), hint: String::new(), intent: Intent::SetTheme(pref) });
    }
    // FX4（FX-016）: 最近の行を覚える / 覚えない（既定は覚えない）
    let remembering = shell.palette.recent.is_some();
    all.push(Command {
        label: if remembering { "Forget recent commands" } else { "Remember recent commands" }.into(),
        hint: String::new(),
        intent: Intent::RememberCommands(!remembering),
    });
    // USAGE-1: 使った記録を clipboard へ（送らない・貼るのは利用者）
    all.push(Command { label: "Copy usage report".into(), hint: String::new(), intent: Intent::CopyUsageReport });
    if let Some(n) = canvases {
        all.push(Command { label: "Duplicate canvas".into(), hint: String::new(), intent: Intent::CloneApp });
        all.push(Command { label: if n > 1 { "Delete canvas…" } else { ONE_CANVAS }.into(), hint: String::new(), intent: Intent::DeleteApp });
    }
    // FX3（FX-008）: 今いる Project の全 Page（app.rs が `Generated::page_commands` を渡す・Project の外では空）
    all.extend(pages.iter().cloned());
    for a in apps {
        all.push(Command { label: format!("Open {}", a.name), hint: a.purpose.clone(), intent: Intent::Go(a.view.clone()) });
    }
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        // 覚えている時だけ、選んだ行を頭へ（今の一覧に無い label は飛ばす・同じ行は 1 回）
        let mut head: Vec<Command> = Vec::new();
        for label in shell.palette.recent.iter().flatten() {
            if head.len() < RECENT_MAX {
                if let Some(i) = all.iter().position(|c| &c.label == label) {
                    head.push(all.remove(i));
                }
            }
        }
        head.append(&mut all);
        return head;
    }
    let mut hits: Vec<Command> = all.into_iter().filter(|c| c.label.to_lowercase().contains(&q) || c.hint.to_lowercase().contains(&q)).collect();
    hits.push(Command { label: format!("Create project: “{}”", query.trim()), hint: "↵".into(), intent: Intent::Generate(query.trim().to_owned(), Vec::new()) });
    hits
}

/// ⌘K の palette（modal）。行は `commands(&palette.query, apps, rail_open, sidebar_open, canvases)`。
/// 契約（UI-4b・引数 `sidebar_open` は UI-4b の Interface の段で足した）: `sidebar_open` は `commands` にそのまま渡すだけ（`Hide sidebar` / `Show sidebar` の行の語）。
/// 契約（UI-6a・引数 `canvases` は UI-6a の Interface の段で足した）: `canvases` も `commands` にそのまま渡す。label が理由の文の行（canvas が 1 つの時の削除）は
/// 押せない形（`theme::button_disabled` と同じ地なし・`text_tertiary`）で描き、Enter と click で intent を返さない。
/// 例（app.rs）: `palette::draw(ctx, &mut self.shell.palette, &rows, self.shell.rail_open, self.shell.sidebar_open, self.shell.canvases)`。保証しない: 開いている間の WebView の扱い（今のまま全部隠す）。
/// 契約（FX3・引数 `pages` は FX3 の Interface の段で足した）: `pages` も `commands` にそのまま渡す（Project の画面の時だけ `Generated::page_commands`・他は空）。
/// 署名（FX4 の Interface の段で替えた）: `palette`・`rail_open`・`sidebar_open`・`canvases` を `shell: &mut Shell` に（欄の文 `Shell::command` を読み・閉じる時に
/// `stash` を欄へ戻し・`recent` を書くため。4 つとも `Shell` の field）。例（app.rs）: `palette::draw(ctx, &mut self.shell, &rows, &pages)`。
/// 契約（FX4〈FX-016〉）: `egui::Modal` と 2 つ目の `TextEdit` を描かない（AX の `Commands` の欄は 0・言葉の欄は窓に 1 つ = `d1_one_field`）。
/// list は 1 つの欄がこの pass に置いた rect（`skeleton::note_command_field`）のすぐ下・同じ左端と幅の `egui::Area`（Order::Foreground・地の scrim なし）。
/// 行は `commands(shell, &shell.command, apps, pages)`・最大 8 行の高さで scroll・空の時は `No projects match`・`recent` の行の上に見出し `Recent`。
/// ↑↓ と Enter は `open` の間だけ list が取る（`header::command_field` は `open` の間 Enter を送らない）・Enter / click で選んだ行は
/// `recent` が `Some` なら `Generate` の行を除いて先頭へ（同じ label を消して入れ、`RECENT_MAX` で切る）・`close(ctx, &mut shell.command)` の後に intent を返す。
/// list の外の click・Esc（app.rs の連鎖）・`Shell::go` で閉じる。保証する: 押せない行（理由の文）は Enter でも click でも intent を返さない（今と同じ）。
/// 保証しない: 開いている間の WebView の扱い（今のまま `cover` で全部隠す）・欄が見えない時（Home を scroll して欄が header の下に隠れた時）は欄を見える所へ scroll しない。
/// Wave: UI-4b, UI-6a, FX3, FX4
pub fn draw(ctx: &egui::Context, shell: &mut Shell, apps: &[AppRow], pages: &[Command]) -> Option<Intent> {
    if !shell.palette.open {
        return None;
    }
    let t = theme::tokens(ctx);
    // 1 つの欄が置いた rect（無ければ header の中央）の真下・同じ左端と幅
    let field = super::skeleton::noted_command_field(ctx)
        .unwrap_or_else(|| Rect::from_center_size(egui::pos2(ctx.screen_rect().center().x, theme::HEADER_H / 2.0), Vec2::new(super::shell::Tier::current(ctx).input_w, theme::COMMAND_H)));
    let list = commands(shell, &shell.command, apps, pages);
    let recent_rows = if shell.command.trim().is_empty() {
        let recent = shell.palette.recent.as_deref().unwrap_or_default();
        list.iter().take(RECENT_MAX).take_while(|c| recent.contains(&c.label)).count()
    } else {
        0
    };
    let (down, up, enter) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::Enter)));
    let palette = &mut shell.palette;
    if down {
        palette.selected = (palette.selected + 1).min(list.len().saturating_sub(1));
    }
    if up {
        palette.selected = palette.selected.saturating_sub(1);
    }
    palette.selected = palette.selected.min(list.len().saturating_sub(1));
    let selected = palette.selected;
    let mut out = None;
    let edge = theme::hairline(&t);
    let area = egui::Area::new(egui::Id::new("kangae-palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(field.left() - edge.width, field.bottom() + theme::SP_1))
        .show(ctx, |ui| {
            egui::Frame::new().fill(t.surface_1).stroke(edge).corner_radius(egui::CornerRadius::same(theme::RADIUS_LG)).show(ui, |ui| {
                ui.set_width(field.width());
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                egui::ScrollArea::vertical().id_salt("kangae-palette-list").max_height(theme::LIST_ROW_H * 8.0).show(ui, |ui| {
                    if list.is_empty() {
                        theme::label(ui, Type::Helper, "No projects match", t.text_tertiary);
                    }
                    for (i, c) in list.iter().enumerate() {
                        if i == 0 && recent_rows > 0 {
                            theme::label(ui, Type::Helper, "Recent", t.text_tertiary);
                        }
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::LIST_ROW_H), egui::Sense::click());
                        let off = disabled(c);
                        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, !off, i == selected, &c.label));
                        if ui.is_rect_visible(rect) {
                            // 押せない行は地を塗らず text_tertiary（theme::button_disabled と同じ）
                            theme::row_background(ui, rect, response.hovered() && !off, i == selected && !off);
                            let hint_w = if c.hint.is_empty() { 0.0 } else { 160.0 };
                            let ink = if off { t.text_tertiary } else { t.text_primary };
                            let galley = theme::truncated(ui, &c.label, Type::Body.font(), ink, rect.width() - hint_w - theme::SP_3 * 2.0);
                            ui.painter().galley(egui::pos2(rect.left() + theme::SP_3, rect.center().y - galley.size().y / 2.0), galley, ink);
                            if !c.hint.is_empty() {
                                let hint = theme::truncated(ui, &c.hint, Type::Helper.font(), t.text_tertiary, hint_w);
                                ui.painter().galley(egui::pos2(rect.right() - theme::SP_3 - hint.size().x, rect.center().y - hint.size().y / 2.0), hint, t.text_tertiary);
                            }
                        }
                        if response.clicked() && !off {
                            out = Some(c);
                        }
                    }
                });
            });
        });
    // 変換中の Enter と確定の frame の Enter は header の欄が入力から外す（`take_enter_while_composing`）ので、ここでは `enter` が立たない
    if enter {
        if let Some(c) = list.get(selected).filter(|c| !disabled(c)) {
            out = Some(c);
        }
    }
    // list と欄の外の click で閉じる
    let outside = ctx.input(|i| i.pointer.any_pressed()).then(|| ctx.input(|i| i.pointer.interact_pos())).flatten().is_some_and(|p| !area.response.rect.contains(p) && !field.contains(p));
    let chosen = out.cloned();
    if let Some(c) = &chosen {
        // 覚えている時だけ先頭へ（作る行と、覚える / 覚えない行そのものは覚えない）
        if let (Some(recent), false) = (shell.palette.recent.as_mut(), matches!(c.intent, Intent::Generate(..) | Intent::RememberCommands(_))) {
            recent.retain(|l| *l != c.label);
            recent.insert(0, c.label.clone());
            recent.truncate(RECENT_MAX);
        }
    }
    if chosen.is_some() || outside {
        shell.palette.close(ctx, &mut shell.command);
    }
    chosen.map(|c| c.intent)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// sidebar・旧 canvas の数だけを替えた Shell（FX4 で `commands` が `&Shell` を取るようになった・View は Runs）。
    fn sh(_: bool, sidebar_open: bool, canvases: Option<usize>) -> Shell {
        let mut s = Shell::new(View::Runs);
        (s.sidebar_open, s.canvases) = (sidebar_open, canvases);
        s
    }
    fn apps() -> Vec<AppRow> {
        vec![AppRow { view: View::CompatCanvas(7), name: "在庫メモ".into(), purpose: "在庫を整理したい".into(), status: "2 パーツ".into(), running: 0, needs: 0, pinned: false, archived: false, opened_at: None, sample: false }]
    }
    #[test]
    fn empty_query_lists_create_nav_rail_and_apps() {
        let list = commands(&sh(false, true, None), "", &apps(), &[]);
        // UI-4a: `New project` は Home の欄へ・Apps の行は無い（FX4: `New blank canvas` も無い）
        assert_eq!((list[0].label.as_str(), list[0].hint.as_str(), &list[0].intent), ("New project", "⌘N", &Intent::Go(View::Home)));
        assert!(!list.iter().any(|c| c.label == "Go to Apps"));
        assert!(list.iter().any(|c| c.intent == Intent::Go(View::Runs)));
        assert!(!list.iter().any(|c| c.intent == Intent::OpenChat), "Project の外に Show chat の行（L1-NORAIL: 右の rail は無い）");
        assert!(list.iter().any(|c| c.intent == Intent::Go(View::Connections)));
        assert!(list.iter().any(|c| c.intent == Intent::Go(View::CompatCanvas(7))));
        // RQ1: `Go to …` の行の次に `Settings`（⌘,）が 1 行・打って探せる。負の対照: push を外す → 赤
        let last_go = list.iter().rposition(|c| c.label.starts_with("Go to ")).unwrap();
        assert_eq!((list[last_go + 1].label.as_str(), list[last_go + 1].hint.as_str(), &list[last_go + 1].intent), ("Settings", "⌘,", &Intent::Go(View::Settings(SettingsSection::General))));
        assert_eq!(list.iter().filter(|c| c.label == "Settings").count(), 1);
        assert_eq!(commands(&sh(false, true, None), "settings", &apps(), &[])[0].label, "Settings", "打って探せる");
        // UI-4b: Settings の行の次に sidebar の行（開いている間は Hide・hint ⌃⌘S）・L1-NORAIL: Project の間は Settings と sidebar の間に Show chat（⇧⌘P）
        assert_eq!((list[last_go + 2].label.as_str(), list[last_go + 2].hint.as_str(), &list[last_go + 2].intent), ("Hide sidebar", "⌃⌘S", &Intent::ToggleSidebar));
        let mut in_project = sh(false, true, None);
        in_project.view = View::Project { project: "p".into(), page: "x".into() };
        let list = commands(&in_project, "", &apps(), &[]);
        let at = list.iter().position(|c| c.intent == Intent::OpenChat).expect("Project の ⌘K に Show chat が無い");
        assert_eq!((list[at].label.as_str(), list[at].hint.as_str(), &list[at + 1].intent), ("Show chat", "⇧⌘P", &Intent::ToggleSidebar));
    }
    /// UI-4b（h_reach_when_folded の試験の半分）: 畳んだ間も ⌘K に Home・Runs・Connections・各 Project の行が残り、sidebar の行は `Show sidebar`。
    #[test]
    fn folded_sidebar_keeps_every_place_in_the_palette() {
        let list = commands(&sh(false, false, None), "", &apps(), &[]);
        for want in [Intent::Go(View::Home), Intent::Go(View::Runs), Intent::Go(View::Connections), Intent::Go(View::CompatCanvas(7))] {
            assert!(list.iter().any(|c| c.intent == want), "{want:?} の行が無い");
        }
        assert!(list.iter().any(|c| c.intent == Intent::ToggleSidebar && c.label == "Show sidebar"));
        assert_eq!(commands(&sh(false, false, None), "sidebar", &apps(), &[])[0].intent, Intent::ToggleSidebar, "打って探せる");
    }
    /// UI-6a（sendback 73・76・78）: header の `•••` の項目の行き先。Appearance の 3 行は常に・旧 canvas の Duplicate と削除は互換 view の間だけ・
    /// canvas が 1 つなら削除の行は理由の文で押せない（Enter でも intent にならない）。負の対照（commit message）: `ONE_CANVAS` の腕を外す → 3 本目が赤。
    #[test]
    fn appearance_and_canvas_rows_moved_from_the_header_menu() {
        let outside = commands(&sh(false, true, None), "", &apps(), &[]);
        for pref in [egui::ThemePreference::System, egui::ThemePreference::Light, egui::ThemePreference::Dark] {
            assert!(outside.iter().any(|c| c.intent == Intent::SetTheme(pref) && c.label.starts_with("Appearance · ")), "{pref:?} の行が無い");
        }
        assert!(!outside.iter().any(|c| matches!(c.intent, Intent::CloneApp | Intent::DeleteApp)), "互換 view の外に旧 canvas の行");
        let two = commands(&sh(false, true, Some(2)), "", &apps(), &[]);
        assert!(two.iter().any(|c| c.intent == Intent::CloneApp && c.label == "Duplicate canvas"));
        assert!(two.iter().any(|c| c.intent == Intent::DeleteApp && c.label == "Delete canvas…" && !disabled(c)));
        let one = commands(&sh(false, true, Some(1)), "", &apps(), &[]);
        let del = one.iter().find(|c| c.intent == Intent::DeleteApp).expect("削除の行");
        assert!(disabled(del) && del.label == "You need at least one canvas", "{del:?}");
    }
    #[test]
    fn query_filters_and_offers_generate_last() {
        let list = commands(&sh(true, true, None), "在庫", &apps(), &[]);
        assert_eq!(list[0].intent, Intent::Go(View::CompatCanvas(7)));
        assert_eq!(list.last().unwrap().intent, Intent::Generate("在庫".into(), vec![]));
        assert_eq!(list.last().unwrap().hint, "↵");
        let none = commands(&sh(true, true, None), "zzz", &[], &[]);
        assert_eq!(none.len(), 1, "only the generate fallback");
    }
    /// FX4（FX-017）: ⌘K から互換 canvas の入口 `New blank canvas` を外す（空の語でも `canvas` と打っても出ない）。旧 session の `Open {name}` は残す。
    #[test]
    fn fx4_palette_has_no_new_blank_canvas_row() {
        for q in ["", "canvas", "blank"] {
            let list = commands(&sh(false, true, None), q, &apps(), &[]);
            assert!(!list.iter().any(|c| c.intent == Intent::NewApp || c.label == "New blank canvas"), "「{q}」で互換 canvas の入口が出た");
        }
        assert!(commands(&sh(false, true, None), "", &apps(), &[]).iter().any(|c| c.intent == Intent::Go(View::CompatCanvas(7))), "旧 session の行が消えた");
    }
    /// FX4（FX-016・UIUX N17）: 最近の行は覚える時（`recent` が Some）だけ・空の語の時に頭へ `RECENT_MAX` まで・同じ行は 1 回・今の一覧に無い label は飛ばす。
    /// 覚える / 覚えないの行は 1 つ（None = `Remember recent commands`・Some = `Forget recent commands`）。
    #[test]
    fn fx4_recent_rows_lead_only_when_remembered() {
        let off = commands(&sh(false, true, None), "", &apps(), &[]);
        assert_eq!(off[0].label, "New project", "覚えていないのに頭が替わった");
        assert!(off.iter().any(|c| c.intent == Intent::RememberCommands(true) && c.label == "Remember recent commands"), "覚える行が無い");
        let mut s = sh(false, true, None);
        s.palette.recent = Some(vec!["Go to Runs".into(), "Open 在庫メモ".into(), "No such row".into(), "Copy usage report".into(), "Go to Home".into()]);
        let on = commands(&s, "", &apps(), &[]);
        let head: Vec<&str> = on.iter().take(RECENT_MAX).map(|c| c.label.as_str()).collect();
        assert_eq!(head, ["Go to Runs", "Open 在庫メモ", "Copy usage report"], "頭の最近の行");
        assert_eq!(on.iter().filter(|c| c.label == "Go to Runs").count(), 1, "最近の行が下にも残った");
        assert!(on.iter().any(|c| c.intent == Intent::RememberCommands(false) && c.label == "Forget recent commands"), "覚えない行が無い");
        assert_eq!(commands(&s, "runs", &apps(), &[])[0].label, "Go to Runs", "打った語がある時は今の絞り方のまま");
    }
}
