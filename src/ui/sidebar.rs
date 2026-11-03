//! Sidebar 232 / 216（§1.1 / §1.2・UI-4b で畳める）: 上から 検索（SESSIONS-LEFT-1101）・`New chat`・nav 3 行（28 高・icon 16・active だけ surface.2）・
//! Sessions（Home の session）・Your projects（行 28・SIZES-1101）。見出しと Project の行は hover・Tab の focus で右端から `⌄ ••• ＋`（追記 244）。card にしない（§3）。
//! 使用量と状態の 1 行は窓の一番下の帯（`shell::status_bar`・STATUSBAR-1101）。

use super::motion::{self, MotionMode};
use super::shell::{AppRow, Intent, Shell, Tier, View};
use super::theme::{self, Type, Variant, SEMIBOLD};
use crate::domain::chat::{session_project, HOME_CHAT};
use crate::domain::icon::Icon;
use crate::domain::terminal::AgentKind;
use eframe::egui::{self, FontFamily, FontId, Rect, Vec2};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// SESSIONS-LEFT-1101: 上の検索の欄の AX の名前（一覧の filter = 言葉の欄に数えない・app.rs の試験の `word_fields` も外す）。
pub const SEARCH: &str = "Search sessions and projects";
/// SESSIONS-LEFT-1101 追記 244: 畳める見出しの鍵（`Intent::FoldHeading::heading`・`UiSettings::folded_headings` の値）。
pub const HEADING_SESSIONS: &str = "sessions";
pub const HEADING_PROJECTS: &str = "projects";

/// 契約（NOTIFY-1101 で引数 `status_line`・STATUSBAR-1101 で `usage` を消した）: 直近の message と使用量は窓の一番下の帯（`shell::status_bar`）。最下行は無い。
/// 契約（UI-4b）: 幅は `motion::travel(.., if shell.sidebar_open { tier.sidebar } else { 0.0 }, motion::RAIL)`（180ms）で寄せ（UI-6a: Reduce Motion は
/// 幅を動かさず中身の不透明度だけ `REDUCE_FADE` で寄せる・畳む時は薄れ切るまで開いた幅のまま = 答え ①）、0.5 未満の frame は SidePanel を描かない（0 幅の区切りの線を残さない・作業の場所が窓の左の gutter まで広がる・欄の rect は動かない）。
/// 中身は開いた幅（`tier.sidebar`）で組んで panel の幅で切る（寄せる間に中身が折り返さず、panel が中身の最小幅で止まらない）。
/// `Workspace` の見出しは消した（G14 C3・失う物なし）。畳んだ間の行き先 = title line の `Shibubu`（Home）と ⌘K の `Go to …`・`Open {name}`（h_reach_when_folded）。
/// 契約（SESSIONS-LEFT-1101・owner 181「session履歴は全て左に」と 198 の 1〜5）: 上から
/// 1. 検索の欄（虫眼鏡・AX `SEARCH`・`shell.sidebar_query`）= session の題・Project の名前・Page の題を大小文字を見ずに絞る（並べ替えは無い = 新しい順だけ）。
///    絞っている間は当たった Project を開いた形で描き、中は当たった物だけ（Project の名前が当たれば中は全部）。
/// 2. `New chat`（owner 191 の枠の button・幅いっぱい・`Intent::NewChat` = どの View からも Home の空の会話へ・前の会話は Sessions に残る）。
///    RQ の `Create project`（= `Go(View::Home)`）はこの行に替えた（⌘N と ⌘K の `New project` は今まで通り）。
/// 3. nav 3 行（Home・Runs・Connections）。
/// 4. `Sessions` = Home の session（`chats` の key が `HOME_CHAT`・Project を作る前の会話・新しい順・題と時刻 `runs::ago`）。押すと `Intent::OpenSession(key)`。無ければ見出しごと描かない。
///    RQ2 の Home の `Continue`（All projects・Search · ⌘K・Name / Recent）はここと 5 に移した（owner 198 の 1・5）。
///    追記 244: 見出しの行に `⌄`（`Intent::FoldHeading` の `HEADING_SESSIONS`）と `＋`（AX `New chat` = `Intent::NewChat`）。
/// 5. `Your projects` = pin が先（`project_order`）。追記 244（owner 244・209・210 の「`+` を常に出す」を替えた）: 見出しの行と各 Project の行は、pointer・Tab の focus・
///    `•••` の menu の間だけ右端から左へ `⌄ ••• ＋` が滑り出す（`row_actions`・検索の間は出さない）。
///    見出し: `⌄` = 下の Project を全部畳む（`HEADING_PROJECTS`）・`•••` = `Show archived · {n}` / `Hide archived · {n}`（`shell.show_archived`・0 なら押せない
///    `No archived projects`）・`＋` = `Intent::Go(View::Home)`（AX `New project` = ⌘N と同じ）。
///    Project の行: `⌄` = 中の開閉だけ（`Intent::FoldProject`・`shell.open_projects` に覚える・画面は移らない）・`•••`（AX `More for {name}`）と右 click =
///    `header::project_actions`（Rename → `Intent::StartRename`・Duplicate・Export…・Show folder in Finder・Change folder…・Pin / Unpin・Archive / Unarchive・
///    `Delete…` → 行の下に `Shell::confirm_delete`〈その Project を開いている間は Project の頭の確認に任せる〉）・`＋` = `Intent::NewSession`（AX `New session in {name}`）。
///    行を押すと、今開いている Project なら中の開閉・他の Project なら `Intent::Go`（移った先の Project は app.rs の `go` が開く）。Project の行か中の行に pointer の間は
///    塊の全体に薄い地。開いた Project の下に、その Project の session（新しい順・題と時刻・押して作ったまだ一言の無い session は一番上に `New session`・
///    `Intent::OpenSession`）→ `pages[project]` の Page（`Intent::Go(View::Project)`）→ Terminal。
/// `chats` = `Generated::chat_list`（key・題・時刻）・`pages` = app.rs が開いた Project（と検索の間は全部）だけ引いた Page（id・題・Mac の app の bundle id か web の URL = `Generated::page_list`）・
/// `terms` = 同じ Project の Terminal（tab の id・名前・前面の agent = `Generated::term_rows`・追記 224）・`session` = 今開いている session の key
/// （Project = `Generated::session_key`・Home = `Shell::talk_id`）。消した Project の session は描かない（Project の行が無い）。
/// 例: `sidebar::draw(ctx, &mut self.shell, tier, &rows, self.generated.chat_list(), &pages, &terms, session.as_deref())`（app.rs・下の帯 `shell::status_bar` の次）。
/// 保証しない: 窓ごとの記憶（窓は 1 つ）・中身の文での検索（題と名前だけ）。
/// Wave: UI-4a, UI-4b, RQ2, SESSIONS-LEFT-1101
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: &egui::Context,
    shell: &mut Shell,
    tier: Tier,
    apps: &[AppRow],
    chats: &[(String, String, String)],
    pages: &BTreeMap<String, Vec<(String, String, Option<String>)>>,
    terms: &BTreeMap<String, Vec<(String, String, Option<&'static AgentKind>)>>,
    session: Option<&str>,
) -> Option<Intent> {
    let t = theme::tokens(ctx);
    let mut intent = None;
    // 開閉（UI-4b）と幅の段の替わり目（232 / 216）は 180ms で寄せる（§15.2-1 rail / drawer の開閉と同じ時間）
    // UI-6a: 幅（大きさ）は travel = Reduce Motion で即時。中身の不透明度は animate（Reduce Motion の短い fade）。畳む時は薄れ切るまで開いた幅のまま
    let id = egui::Id::new("kangae.sidebar.width");
    let fade = motion::animate(ctx, id.with("fade"), if shell.sidebar_open { 1.0 } else { 0.0 }, motion::RAIL);
    let still = motion::motion_mode(ctx) != MotionMode::Full;
    let width = motion::travel(ctx, id, if shell.sidebar_open || (still && fade > 0.0) { tier.sidebar } else { 0.0 }, motion::RAIL);
    if width < 0.5 {
        return None;
    }
    egui::SidePanel::left("kangae-sidebar")
        .exact_width(width)
        .resizable(false)
        .show_separator_line(true)
        .frame(egui::Frame::new().fill(t.surface_1))
        .show(ctx, |panel| {
            // 開いた幅の箱で組む（panel の clip で切れる）。親の ui を広げない child なので panel の幅は `width` のまま
            let r = panel.max_rect();
            let body = egui::Rect::from_min_size(r.min, Vec2::new(tier.sidebar, r.height())).shrink2(Vec2::new(theme::SP_3, theme::SP_4));
            let mut child = panel.new_child(egui::UiBuilder::new().id_salt("kangae-sidebar-body").max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
            let ui = &mut child;
            if still {
                ui.multiply_opacity(fade);
            }
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
            // SIZES-1101 / 226: 間を 1 段詰める（12 → 8・16 → 12）
            search_field(ui, &mut shell.sidebar_query);
            ui.add_space(theme::SP_2);
            if super::home::new_chat_button(ui) {
                intent = Some(Intent::NewChat);
            }
            ui.add_space(theme::SP_3);
            for view in View::NAV.iter() {
                let active = &shell.view == view;
                if nav_row(ui, view, active).clicked() {
                    intent = Some(Intent::Go(view.clone()));
                }
            }
            ui.add_space(theme::SP_3);
            theme::rule(ui);
            ui.add_space(theme::SP_2);
            // 最下行は無い（使用量と直近の message は窓の一番下の帯 = STATUSBAR-1101）
            let list_h = ui.available_height();
            let q = shell.sidebar_query.trim().to_lowercase();
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            egui::ScrollArea::vertical().id_salt("kangae-sidebar-apps").max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                let mut shown = 0;
                let searching = !q.is_empty();
                // 4. Sessions（Home の session = Project の外）・追記 244: 見出しの ⌄ ＋
                let home: Vec<&(String, String, String)> = chats.iter().filter(|(k, title, _)| k.starts_with(HOME_CHAT) && hits(&q, title)).collect();
                if !home.is_empty() {
                    let (open, acts) = heading_row(ui, "Sessions", HEADING_SESSIONS, &shell.folded_headings, searching, None, "New chat");
                    if acts.fold {
                        intent = Some(Intent::FoldHeading { heading: HEADING_SESSIONS.into(), open: !open });
                    }
                    if acts.plus {
                        intent = Some(Intent::NewChat);
                    }
                    for (key, title, at) in home.into_iter().filter(|_| open) {
                        let active = shell.view == View::Home && session == Some(key.as_str());
                        if sub_row(ui, title, &super::runs::ago(at, now), active, 0.0, (None, None, Some(Icon::MessageSquare), "Chat")).clicked() {
                            intent = Some(Intent::OpenSession(key.clone()));
                        }
                        shown += 1;
                    }
                    ui.add_space(theme::SP_3);
                }
                // 5. Your projects（pin が先・Archive した行は見出しの ••• の `Show archived · {n}` で後ろに）・追記 244: 見出しの ⌄ ••• ＋
                let archived = apps.iter().filter(|a| a.archived).count();
                let more = match (archived, shell.show_archived) {
                    (0, _) => "No archived projects".to_owned(),
                    (n, true) => format!("Hide archived · {n}"),
                    (n, false) => format!("Show archived · {n}"),
                };
                let (open, acts) = heading_row(ui, "Your projects", HEADING_PROJECTS, &shell.folded_headings, searching, Some(more.as_str()), "New project");
                if acts.fold {
                    intent = Some(Intent::FoldHeading { heading: HEADING_PROJECTS.into(), open: !open });
                }
                if acts.plus {
                    intent = Some(Intent::Go(View::Home));
                }
                let toggle = acts.more.as_ref().and_then(|m| theme::menu_popup(ui, m, |ui| ui.add_enabled_ui(archived > 0, |ui| theme::button_menu_item(ui, &more, "", Variant::Ghost).clicked()).inner.then_some(())));
                if toggle.is_some() {
                    shell.show_archived = !shell.show_archived;
                }
                for i in project_order(apps, shell.show_archived).into_iter().filter(|_| open) {
                    let (drawn, picked) = project_block(ui, shell, &apps[i], chats, pages, terms, session, &q, now);
                    shown += usize::from(drawn);
                    if picked.is_some() {
                        intent = picked;
                    }
                }
                if searching && shown == 0 {
                    theme::label(ui, Type::Helper, format!("Nothing matches “{}”", shell.sidebar_query.trim()), t.text_tertiary);
                }
                // 時刻の語（Just now → 1 min ago → 1 h ago）が替わる時だけ描き直す（idle の frame を増やさない）
                if let Some(s) = chats.iter().filter_map(|(_, _, at)| super::runs::ago_changes_in(at, now)).min() {
                    ui.ctx().request_repaint_after(Duration::from_secs(s.max(1) as u64));
                }
            });
        });
    intent
}

/// SESSIONS-LEFT-1101 追記 244: 見出しの下を描くか（検索の間は畳みを無視 = 当たった物を隠さない）。純関数。
fn heading_open(folded: &[String], key: &str, searching: bool) -> bool {
    searching || !folded.iter().any(|h| h == key)
}

/// SESSIONS-LEFT-1101 追記 244: 畳める見出しの行（`HIT_MIN` 高・字は Helper の `text_tertiary`・字は押せない）。右端から `⌄`（`more` が在れば `•••`）`＋` が出る（`row_actions`）。
/// 検索の間は出さず開いた形。`more` = ••• の menu の 1 項目の語（None = ••• を出さない）・`plus` = ＋ の AX の名前。返す = (下を描くか, 押した手)。
fn heading_row(ui: &mut egui::Ui, label: &str, key: &str, folded: &[String], searching: bool, more: Option<&str>, plus: &str) -> (bool, RowActs) {
    let t = theme::tokens(ui.ctx());
    let open = heading_open(folded, key, searching);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::HIT_MIN), egui::Sense::hover());
    let hot = ui.rect_contains_pointer(rect);
    let acts = if searching { RowActs::default() } else { row_actions(ui, rect, egui::Id::new(("kangae.sidebar.heading", key)), hot, open, label, more.is_some(), Some(plus)) };
    let galley = theme::truncated(ui, label, Type::Helper.font(), t.text_tertiary, rect.width() - acts.taken - theme::SP_1);
    ui.painter().galley(egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, t.text_tertiary);
    (open, acts)
}

/// SESSIONS-LEFT-1101 追記 244: `row_actions` の返り。新しい型の理由: 見出しと Project の行の 2 か所が同じ 4 つを受ける（4 つ組の tuple は読み違える）。
#[derive(Default)]
struct RowActs {
    /// `⌄` を押した
    fold: bool,
    /// `•••` の Response（呼ぶ側が `theme::menu_popup` に渡す）・None = ••• の無い行
    more: Option<egui::Response>,
    /// `＋` を押した
    plus: bool,
    /// 行の右端から出た分の幅（名前はその手前で切る・隠れている間は 0）
    taken: f32,
}

/// SESSIONS-LEFT-1101 追記 244（owner「カーソルを合わせると^の下向きと・・・、+を選べるように出てきて右から左に出るように」・出し方は Orca の
/// WorktreeCard の `opacity-0` → `group-hover/worktree-card:opacity-100 group-focus-within/worktree-card:opacity-100` = pointer か中の focus で出る）:
/// 行の右端から左へ `⌄ ••• ＋`（左からこの順・各 `ACT` 四方・`more` / `plus` が無ければ詰める）を滑り出させる。
/// 出る = `hot`（行か Project の塊に pointer）・3 つのどれかに keyboard の focus・••• の menu が開いている。寄せは `motion::travel`（`motion::HOVER`・Reduce Motion = その場に出る）。
/// 3 つは毎 frame interact する（押せる範囲は行の中だけ = 隠れている間は押せないが Tab の鎖には居る = focus で出る）。`⌄` は畳んだ時 › に回る。
/// AX = `Collapse {name}` / `Expand {name}`・`More for {name}`・`plus`。
#[allow(clippy::too_many_arguments)]
fn row_actions(ui: &mut egui::Ui, rect: Rect, id: egui::Id, hot: bool, open: bool, name: &str, more: bool, plus: Option<&str>) -> RowActs {
    let t = theme::tokens(ui.ctx());
    let ids = [id.with("fold"), id.with("more"), id.with("plus")];
    let on = [true, more, plus.is_some()];
    let held = ui.memory(|m| ids.iter().zip(on).any(|(i, o)| o && m.has_focus(*i)) || m.is_popup_open(ids[1].with("kangae-menu")));
    let s = motion::travel(ui.ctx(), id.with("slide"), if hot || held { 1.0 } else { 0.0 }, motion::HOVER);
    let turn = motion::travel(ui.ctx(), id.with("turn"), if open { 0.0 } else { 1.0 }, motion::HOVER);
    let w = on.iter().filter(|o| **o).count() as f32 * ACT + theme::SP_1;
    let name = name.trim();
    let fold_name = format!("{} {name}", if open { "Collapse" } else { "Expand" });
    let more_name = format!("More for {name}");
    let names = [fold_name.as_str(), more_name.as_str(), plus.unwrap_or("")];
    let painter = ui.painter_at(rect);
    let mut acts = RowActs { taken: w * s, ..RowActs::default() };
    let mut x = rect.right() - w * s;
    for (i, icon) in [Icon::ChevronDown, Icon::Ellipsis, Icon::Plus].into_iter().enumerate() {
        if !on[i] {
            continue;
        }
        let hit = Rect::from_min_size(egui::pos2(x, rect.center().y - ACT / 2.0), Vec2::splat(ACT));
        x += ACT;
        let r = ui.interact(hit.intersect(rect), ids[i], egui::Sense::click());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, names[i]));
        if s > 0.0 {
            let face = hit.shrink(2.0);
            if r.hovered() {
                painter.rect_filled(face, egui::CornerRadius::same(theme::RADIUS_SM), t.surface_3);
            }
            theme::focus_ring(ui, &r, face, egui::CornerRadius::same(theme::RADIUS_SM));
            let angle = if i == 0 { -std::f32::consts::FRAC_PI_2 * turn } else { 0.0 };
            theme::paint_icon(&painter, icon, face.center(), theme::ICON_UI, if r.hovered() { t.text_primary } else { t.text_secondary }, angle);
        }
        let r = r.on_hover_text(names[i]);
        match i {
            0 => acts.fold = r.clicked(),
            1 => acts.more = Some(r),
            _ => acts.plus = r.clicked(),
        }
    }
    acts
}

/// SESSIONS-LEFT-1101: 検索の当たり（空 = 全部・大小文字を見ない・`q` は小文字にした物）。
fn hits(q: &str, text: &str) -> bool {
    q.is_empty() || text.to_lowercase().contains(q)
}

/// SESSIONS-LEFT-1101（RQ2 の Home の `continue_order` を移した・Name / Recent は無い）: Your projects に描く行の index（`apps` の中）を描く順に。純関数。
/// 保証する: pin の行が先・同じ印の中は `apps` の順（作った新しい順・stable）・`show_archived` が false なら `archived` の行を出さない（true なら後ろに足す）。
fn project_order(apps: &[AppRow], show_archived: bool) -> Vec<usize> {
    let mut live: Vec<usize> = (0..apps.len()).filter(|&i| !apps[i].archived).collect();
    live.sort_by_key(|&i| !apps[i].pinned);
    if show_archived {
        live.extend((0..apps.len()).filter(|&i| apps[i].archived));
    }
    live
}

/// SESSIONS-LEFT-1101: Your projects の 1 つ（行〈名前・hover で `⌄ ••• ＋`〉と、開いていれば中の session・Page・Terminal）。返す = (描いたか, 押した手)。
/// 検索の間は当たった Project だけ・開いた形で（中は当たった物だけ・名前が当たれば全部・行の `⌄ ••• ＋` は出さない）。
/// 追記 244: 行と中の行を 1 つの塊に = 前の frame の塊の rect に pointer の間は、塊の全体に `RADIUS_SM` の薄い地（1 つの行の地はその上）・行の `⌄ ••• ＋` も出す。
#[allow(clippy::too_many_arguments)]
fn project_block(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    row: &AppRow,
    chats: &[(String, String, String)],
    pages: &BTreeMap<String, Vec<(String, String, Option<String>)>>,
    terms: &BTreeMap<String, Vec<(String, String, Option<&'static AgentKind>)>>,
    session: Option<&str>,
    q: &str,
    now: i64,
) -> (bool, Option<Intent>) {
    let mut intent = None;
    let project = row.project_id();
    let name_hit = hits(q, &row.name);
    let child = |text: &str| name_hit || hits(q, text);
    let mine: Vec<&(String, String, String)> = match project.as_deref() {
        Some(p) => chats.iter().filter(|(k, title, _)| session_project(k) == Some(p) && child(title)).collect(),
        None => Vec::new(),
    };
    let list: Vec<&(String, String, Option<String>)> = project.as_deref().and_then(|p| pages.get(p)).map_or_else(Vec::new, |l| l.iter().filter(|(_, title, _)| child(title)).collect());
    let shells: Vec<&(String, String, Option<&'static AgentKind>)> = project.as_deref().and_then(|p| terms.get(p)).map_or_else(Vec::new, |l| l.iter().filter(|(_, name, _)| child(name)).collect());
    if !name_hit && mine.is_empty() && list.is_empty() && shells.is_empty() {
        return (false, None);
    }
    // その Project を開いているか・開いている Page（中の行の active と、行の下の確認を Project の頭に任せるか）
    let (viewing, page_now) = match (&shell.view, project.as_deref()) {
        (View::Project { project: v, page }, Some(p)) if v == p => (true, Some(page.clone())),
        _ => (false, None),
    };
    let active = shell.view == row.view || viewing;
    let open = project.as_ref().is_some_and(|p| !q.is_empty() || shell.open_projects.contains(p));
    // 追記 244: 塊の地は行より先に描く場所を取り、塊の rect（前の frame）で pointer を見る
    let t = theme::tokens(ui.ctx());
    let key = egui::Id::new(("kangae.sidebar.project", &row.view));
    let block_hot = ui.ctx().data(|d| d.get_temp::<Rect>(key.with("block"))).is_some_and(|b| ui.rect_contains_pointer(b));
    let ground = ui.painter().add(egui::Shape::Noop);
    let (r, acts) = project_row(ui, row, key, active, open, block_hot, project.is_some() && q.is_empty());
    // owner 210 の 6: 今開いている Project の行 = 中の開閉・他は移る（移った先の Project は app.rs の `go` が開く）。⌄ は開閉だけ（追記 244）
    if r.clicked() {
        intent = Some(match &project {
            Some(p) if viewing => Intent::FoldProject { project: p.clone(), open: !open },
            _ => Intent::Go(row.view.clone()),
        });
    }
    let Some(p) = project else { return (true, intent) };
    if acts.fold {
        intent = Some(Intent::FoldProject { project: p.clone(), open: !open });
    }
    if acts.plus {
        intent = Some(Intent::NewSession(p.clone()));
    }
    // 追記 244: ••• と右 click は同じ menu（Project の頭の `⌄` と同じ `project_actions` に Pin・Archive を足す）
    let extra = [
        (if row.pinned { "Unpin" } else { "Pin" }, Intent::PinProject { project: p.clone(), pinned: !row.pinned }),
        (if row.archived { "Unarchive" } else { "Archive" }, Intent::ArchiveProject { project: p.clone(), archived: !row.archived }),
    ];
    let menu = |ui: &mut egui::Ui| super::header::project_actions(ui, &p, &extra);
    let mut picked = acts.more.as_ref().and_then(|m| theme::menu_popup(ui, m, menu));
    r.context_menu(|ui| {
        if let Some(a) = menu(ui) {
            picked = Some(a);
            ui.close_menu();
        }
    });
    if let Some(a) = picked {
        let name = row.name.trim();
        intent = match a {
            super::header::ProjectAct::Rename => Some(Intent::StartRename { project: p.clone(), name: name.to_owned() }),
            a => a.apply(shell, &p, name).or(intent),
        };
    }
    if !viewing {
        if let Some(i) = shell.confirm_delete(ui, &p, || row.name.trim().to_owned()) {
            intent = Some(i);
        }
    }
    if !open {
        ui.ctx().data_mut(|d| d.insert_temp(key.with("block"), r.rect));
        return (true, intent);
    }
    for (chat, title, at) in mine {
        let on = viewing && session == Some(chat.as_str());
        if sub_row(ui, title, &super::runs::ago(at, now), on, INDENT, (None, None, Some(Icon::MessageSquare), "Chat")).clicked() {
            intent = Some(Intent::OpenSession(chat.clone()));
        }
    }
    for (page, title, app) in list {
        let on = page_now.as_deref() == Some(page.as_str());
        // Mac の app を置いた Page = その app の icon（読めなければ Shibubu の app の面の icon）
        if sub_row(ui, title, "", on, INDENT, (None, app.as_deref(), Some(Icon::AppWindow), "Page")).clicked() {
            intent = Some(Intent::Go(View::Project { project: p.clone(), page: page.clone() }));
        }
    }
    // 追記 224: 開いた Project の Terminal（panel の tab の並び・名前は 217 の自動の名前）。押す = その Project を開き、その tab を選ぶ
    for (tab, name, kind) in shells {
        let icon = kind.map_or((None, None, Some(Icon::SquareTerminal), "Terminal"), |k| (k.logo, k.app, None, k.label));
        if sub_row(ui, name, "", false, INDENT, icon).clicked() {
            intent = Some(Intent::OpenTerminal { project: p.clone(), tab: tab.clone() });
        }
    }
    let block = Rect::from_min_max(r.rect.left_top(), egui::pos2(r.rect.right(), ui.min_rect().bottom()));
    ui.ctx().data_mut(|d| d.insert_temp(key.with("block"), block));
    if block_hot {
        ui.painter().set(ground, egui::Shape::rect_filled(block.shrink2(Vec2::new(theme::SP_1, 0.0)), egui::CornerRadius::same(theme::RADIUS_SM), t.surface_2.gamma_multiply(0.5)));
    }
    ui.add_space(theme::SP_1);
    (true, intent)
}

/// SESSIONS-LEFT-1101: 開いた Project の中の行の字下げ（追記 224 で Project の行の ◉ ▦ の箱を外した = 1 段だけ右へ）。
const INDENT: f32 = theme::SP_2;
/// SESSIONS-LEFT-1101 追記 244: 行の右端から出る `⌄ ••• ＋` の 1 つの押せる四方（`HIT_MIN` = 28・hover の地は 2 内）。owner 209・210 の `PLUS_HIT` を 3 つに広げた。
const ACT: f32 = theme::HIT_MIN;

/// SESSIONS-LEFT-1101: 上の検索の欄（地 `surface_2`・枠 `border`・左に虫眼鏡〈ICONS-1101: `Icon::Search`〉・hint `Search`・Esc で空に）。
fn search_field(ui: &mut egui::Ui, query: &mut String) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::BUTTON_H), egui::Sense::hover());
    let p = ui.painter();
    p.rect(rect, egui::CornerRadius::same(theme::RADIUS_MD), t.surface_2, theme::hairline(&t), egui::StrokeKind::Inside);
    let c = egui::pos2(rect.left() + theme::SP_2 + 5.0, rect.center().y);
    theme::paint_icon(p, Icon::Search, c + Vec2::new(1.0, 0.0), 14.0, t.text_tertiary, 0.0);
    let text = Rect::from_min_max(egui::pos2(c.x + 11.0, rect.top()), egui::pos2(rect.right() - theme::SP_2, rect.bottom()));
    let edit = egui::TextEdit::singleline(query).hint_text("Search").frame(false).font(Type::Nav.font()).vertical_align(egui::Align::Center).desired_width(text.width());
    let r = theme::ax_name(ui.put(text, edit), SEARCH);
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        query.clear();
    }
}

/// nav 行 28 高・icon 16・click target 行の幅 × 28（`NAV_ROW_H`・`ICON_NAV`・SIZES-1101）。active だけ surface.2（hover の地と同じく左右 `SP_1` 内・icon と字は地から 8 内）。
fn nav_row(ui: &mut egui::Ui, view: &View, active: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::NAV_ROW_H), egui::Sense::click());
    let label = view.nav_label();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label));
    // active の背景と文字色は View の切替で cross-fade（§15.2-1 状態の変化）。見えない間も寄せる（出た時に古い色を出さない）
    let bg = motion::cross_fade(ui.ctx(), response.id.with("bg"), if active { t.surface_2 } else { egui::Color32::TRANSPARENT });
    let color = motion::cross_fade(ui.ctx(), response.id.with("fg"), if active { t.text_primary } else { t.text_secondary });
    if ui.is_rect_visible(rect) {
        // operator の決め（226 の但し書き 1）: 字と icon は地の左端から 8 内 = 地の矩形から測る
        let rect = theme::row_background(ui, rect, response.hovered(), false);
        ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_SM), bg);
        theme::focus_ring(ui, &response, rect, egui::CornerRadius::same(theme::RADIUS_SM));
        let icon_center = rect.left_center() + Vec2::new(theme::SP_2 + theme::ICON_NAV / 2.0, 0.0);
        theme::paint_icon(ui.painter(), view.nav_icon(), icon_center, theme::ICON_NAV, color, 0.0);
        let font = if active { FontId::new(Type::Nav.size(), FontFamily::Name(SEMIBOLD.into())) } else { Type::Nav.font() };
        let text_x = rect.left() + theme::SP_2 + theme::ICON_NAV + theme::SP_2 + 3.0;
        let galley = theme::truncated(ui, label, font, color, rect.right() - text_x - theme::SP_2);
        ui.painter().galley(egui::pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, color);
    }
    response.on_hover_text(label)
}

/// YOUR APPS の行 28 高（`APP_ROW_H`・ATTACH の f53b9166〈226・233〉で 44 → 28・名前だけ・追記 224〈owner「projectの横のアイコン要らない」〉で ◉ ▦ の箱を外した）。
/// SESSIONS-LEFT-1101 追記 244: `acts` なら右端から `⌄ ••• ＋`（`row_actions`・`key` が id の元・行か塊〈`block_hot`〉に pointer の間・名前はその手前で切る）。
/// 行の地は行の上に pointer の間（右の 3 つの上でも消えない）。返す = (行, 右の 3 つ)。
fn project_row(ui: &mut egui::Ui, row: &AppRow, key: egui::Id, active: bool, open: bool, block_hot: bool, acts: bool) -> (egui::Response, RowActs) {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::APP_ROW_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &row.name));
    let inside = ui.rect_contains_pointer(rect);
    let ground = theme::row_background(ui, rect, inside, active);
    theme::focus_ring(ui, &response, ground, egui::CornerRadius::same(theme::RADIUS_SM));
    let name = row.name.trim();
    let plus = format!("New session in {name}");
    let acts = if acts { row_actions(ui, rect, key, block_hot || inside, open, name, true, Some(plus.as_str())) } else { RowActs::default() };
    let color = if active { t.text_primary } else { t.text_secondary };
    let text_x = ground.left() + theme::SP_2;
    let galley = theme::truncated(ui, &row.name, Type::Nav.font(), color, (ground.right() - theme::SP_2).min(rect.right() - acts.taken - theme::SP_1) - text_x);
    ui.painter().galley(egui::pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, color);
    let tip = if row.purpose.is_empty() { row.status.clone() } else { format!("{} · {}", row.purpose, row.status) };
    (response.on_hover_text(tip), acts)
}

/// SESSIONS-LEFT-1101: session・Page・Terminal の行（`BUTTON_H` 高・`indent` だけ右から始める〈Project の中 = `INDENT`・Home の Sessions = 0〉・題は Nav の字・右に小さく `right`〈時刻〉）。
/// hover・focus の輪・active の地は Project の行と同じ作り。AX = Button・名前は題（空の題は `Untitled session`）。
/// 追記 224: 題の左に種類の icon（`kind` = (同梱の logo の key, Mac の app の bundle id か web の URL, Phosphor, 種類の名前) = `theme::kind_icon` の出所の順・16）・
/// hover = 種類の名前と題。PROVIDER-ICONS-1101 で logo を足した（Terminal の行の agent = `AgentKind::logo`）。
fn sub_row(ui: &mut egui::Ui, title: &str, right: &str, active: bool, indent: f32, kind: (Option<&str>, Option<&str>, Option<Icon>, &str)) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let title = if title.is_empty() { "Untitled session" } else { title };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::BUTTON_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, title));
    if ui.is_rect_visible(rect) {
        let rect = theme::row_background(ui, rect, response.hovered(), active);
        theme::focus_ring(ui, &response, rect, egui::CornerRadius::same(theme::RADIUS_SM));
        let color = if active { t.text_primary } else { t.text_secondary };
        let when = theme::truncated(ui, right, Type::Helper.font(), t.text_tertiary, rect.width() * 0.4);
        let when_w = when.size().x;
        let right_x = rect.right() - theme::SP_2 - when_w;
        ui.painter().galley(egui::pos2(right_x, rect.center().y - when.size().y / 2.0), when, t.text_tertiary);
        let icon_x = rect.left() + indent + theme::SP_2;
        theme::kind_icon(ui, egui::pos2(icon_x + theme::ICON_UI / 2.0, rect.center().y), kind.0, kind.1, kind.2, kind.3, color);
        let x = icon_x + theme::ICON_UI + theme::SP_2;
        let galley = theme::truncated(ui, title, Type::Nav.font(), color, (right_x - theme::SP_2 - x).max(24.0));
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    }
    if right.is_empty() { response.on_hover_text(format!("{} · {title}", kind.3)) } else { response.on_hover_text(format!("{} · {title} · {right}", kind.3)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SESSIONS-LEFT-1101（RQ2 の Continue の並びの試験を移した）: pin の行が先・同じ印の中は作った新しい順・Archive した行は開いた時だけ後ろに。
    /// 負の対照: `project_order` の `sort_by_key(|&i| !apps[i].pinned)` を外す → pin の行が先に来ず赤。
    #[test]
    fn sessions_left_project_order_pins_first_and_archived_last() {
        let row = |name: &str, pinned: bool, archived: bool| AppRow {
            view: View::App(name.into()),
            name: name.into(),
            purpose: String::new(),
            status: String::new(),
            running: 0,
            needs: 0,
            pinned,
            archived,
            opened_at: None,
            sample: false,
        };
        let apps = [row("banana", false, false), row("Cherry", true, false), row("apple", false, false), row("Zed", false, true)];
        assert_eq!(project_order(&apps, false), vec![1, 0, 2], "pin → 作った新しい順");
        assert_eq!(project_order(&apps, true), vec![1, 0, 2, 3], "Archive した行は開いた時だけ後ろに");
        assert!(hits("", "x") && hits("chat", "My Chat about cats") && !hits("dog", "My Chat"), "検索は大小文字を見ない部分一致");
    }

    /// SESSIONS-LEFT-1101 追記 244: 畳んだ見出しは閉じ・他の見出しは開き・検索の間は畳みを無視して開く（当たった物を隠さない）。
    /// 負の対照（未実行・owner 186 / 187）: `heading_open` の `searching ||` を外す → 3 つ目の assert が赤。
    #[test]
    fn sessions_left_244_heading_open() {
        let folded = vec![HEADING_PROJECTS.to_owned()];
        assert!(heading_open(&[], HEADING_PROJECTS, false), "何も畳んでいないのに閉じた");
        assert!(!heading_open(&folded, HEADING_PROJECTS, false), "畳んだ見出しが開いた");
        assert!(heading_open(&folded, HEADING_PROJECTS, true), "検索の間に畳んだ見出しが当たった物を隠した");
        assert!(heading_open(&folded, HEADING_SESSIONS, false), "別の見出しまで閉じた");
    }
}
