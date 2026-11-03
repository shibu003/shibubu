//! DESIGN.md §15.2-2 の全画面 skeleton loading。
//! - 帯は灰色（Light = surface.3 / Dark = surface.2）。その上を明るい帯（Light = surface.2 / Dark = surface.3）が
//!   左から右へ 1.2s で流れる。位相は画面幅で揃える = 画面の全部の帯を 1 本の光が横切る
//! - 骨格は text を 1 つも描かない（rect と mesh だけ）。reduce motion なら光を流さず repaint も頼まない
//! - 中身が来たら `with_skeleton` が帯から本物へ cross-fade。120ms 未満で読み終わる時は骨格を描かない（ちらつき防止）
//! - 起動直後は `install_startup` が殻の骨格を最前面に置いて本物へ fade する（最初の frame が 120ms 以上かかった時だけ）
//! - まだ bin から呼ばれない物（`#[allow(dead_code)]`）: `Skeleton`・`view_skeleton`・`region_skeleton`・`with_skeleton`・`with_skeleton_by`・`shell_skeleton`。
//!   View の中身は手元の data で 120ms 未満に出るので骨格の出番が無く、renderer（data の取得・生成中）と統合役が呼ぶ

use super::motion;
use super::shell::{Tier, View};
use super::theme;
use eframe::egui::{self, pos2, Color32, Context, Id, LayerId, Mesh, Order, Painter, Pos2, Rect, Sense, Shape, Theme, Ui, UiBuilder, Vec2};

/// 光が左端から右端まで渡る時間。
const SHIMMER_PERIOD: f64 = 1.2;
/// これより早く読み終わる時は骨格を描かない。
pub const SHOW_AFTER: f64 = 0.120;
/// 1 行の text の代わりに描く帯の高さ。
pub const LINE_H: f32 = 12.0;
const SHINE_W: f32 = 240.0;
/// 行ごとの帯の長さ（同じ長さが並ぶと骨格に見えないので、決まった順でばらす）。
const WIDTHS: [f32; 5] = [0.92, 0.74, 0.86, 0.62, 0.8];

fn width_at(i: usize) -> f32 {
    WIDTHS[i % WIDTHS.len()]
}

/// 骨格の部品。`show` が場所を取って描く。
#[allow(dead_code)] // renderer の region・with_skeleton が呼ぶまで
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Skeleton {
    /// text 1 行（幅の 60%）。
    Line,
    /// 全幅 × 高さ。
    Block(f32),
    /// 直径。
    Circle(f32),
    /// n 行（icon の丸 + 本文 + 右端の短い帯）。
    List(usize),
    /// rows × cols（見出しの行 + 区切り線 + 本文の行）。
    Table(usize, usize),
    /// 枠付きの card（見出し + 2 行）。
    Card,
    /// n 欄（label + 入力欄）+ 送信 button。
    Form(usize),
}

#[allow(dead_code)]
impl Skeleton {
    pub fn show(self, ui: &mut Ui) -> Rect {
        let w = ui.available_width();
        match self {
            Skeleton::Line => {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover());
                bar(ui, Rect::from_min_size(pos2(rect.left(), rect.center().y - LINE_H / 2.0), Vec2::new(w * 0.6, LINE_H)));
                rect
            }
            Skeleton::Block(h) => {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                bar(ui, rect);
                rect
            }
            Skeleton::Circle(d) => {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(d), Sense::hover());
                circle(ui.painter(), rect.center(), d / 2.0, now(ui.ctx()));
                rect
            }
            Skeleton::List(n) => rows(ui, n, theme::LIST_ROW_H, |ui, r, i| {
                circle(ui.painter(), pos2(r.left() + 12.0, r.center().y), 12.0, now(ui.ctx()));
                bar(ui, line_in(r, r.left() + 36.0, (r.width() - 36.0) * 0.55 * width_at(i)));
                bar(ui, line_in(r, r.right() - r.width() * 0.12, r.width() * 0.12));
            }),
            Skeleton::Table(n, cols) => {
                let cols = cols.max(1);
                let top = ui.cursor().top();
                let head = rows(ui, 1, 32.0, |ui, r, _| cells(ui, r, cols, |_| 0.45, 10.0));
                hairline(ui, head.left(), head.right(), head.bottom());
                rows(ui, n, theme::LIST_ROW_H, |ui, r, i| cells(ui, r, cols, |c| width_at(i + c), LINE_H));
                Rect::from_min_max(pos2(head.left(), top), pos2(head.right(), ui.cursor().top()))
            }
            Skeleton::Card => {
                let t = theme::tokens(ui.ctx());
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 120.0), Sense::hover());
                ui.painter().rect(rect, theme::RADIUS_LG, t.surface_1, theme::hairline(&t), egui::StrokeKind::Inside);
                let inner = rect.shrink(theme::SP_4);
                bar(ui, Rect::from_min_size(inner.min, Vec2::new(inner.width() * 0.4, 16.0)));
                for (i, y) in [inner.top() + 36.0, inner.top() + 60.0].into_iter().enumerate() {
                    bar(ui, Rect::from_min_size(pos2(inner.left(), y), Vec2::new(inner.width() * width_at(i), LINE_H)));
                }
                rect
            }
            Skeleton::Form(n) => {
                let top = ui.cursor().top();
                for _ in 0..n {
                    let (label, _) = ui.allocate_exact_size(Vec2::new(w, 14.0), Sense::hover());
                    bar(ui, Rect::from_min_size(label.min, Vec2::new(96.0_f32.min(w), 10.0)));
                    Skeleton::Block(theme::INPUT_H).show(ui);
                }
                let (btn, _) = ui.allocate_exact_size(Vec2::new(w, theme::BUTTON_H), Sense::hover());
                bar(ui, Rect::from_min_size(btn.min, Vec2::new(96.0_f32.min(w), theme::BUTTON_H)));
                Rect::from_min_max(pos2(btn.left(), top), btn.max)
            }
        }
    }
}

fn now(ctx: &Context) -> f64 {
    ctx.input(|i| i.time)
}

fn bar(ui: &Ui, rect: Rect) {
    paint_shimmer(ui.painter(), rect, now(ui.ctx()));
}

fn line_in(row: Rect, left: f32, width: f32) -> Rect {
    Rect::from_min_size(pos2(left, row.center().y - LINE_H / 2.0), Vec2::new(width.max(0.0), LINE_H))
}

fn hairline(ui: &Ui, left: f32, right: f32, y: f32) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(Rect::from_min_max(pos2(left, y - theme::HAIRLINE), pos2(right, y)), 0.0, t.border);
}

/// n 行を高さ `h` で取り、行ごとに `paint(ui, 行の rect, index)`。全体の rect を返す。
fn rows(ui: &mut Ui, n: usize, h: f32, mut paint: impl FnMut(&Ui, Rect, usize)) -> Rect {
    let w = ui.available_width();
    let top = ui.cursor().min;
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut last = Rect::from_min_size(top, Vec2::new(w, 0.0));
    for i in 0..n {
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        paint(ui, r, i);
        last = r;
    }
    ui.spacing_mut().item_spacing.y = spacing;
    ui.add_space(spacing);
    Rect::from_min_max(top, pos2(top.x + w, last.bottom()))
}

/// 1 行を cols 等分し、各欄に `frac(col)` の長さの帯。
fn cells(ui: &Ui, row: Rect, cols: usize, frac: impl Fn(usize) -> f32, h: f32) {
    let cw = row.width() / cols as f32;
    for c in 0..cols {
        let left = row.left() + cw * c as f32;
        bar(ui, Rect::from_min_size(pos2(left, row.center().y - h / 2.0), Vec2::new((cw - theme::SP_4) * frac(c), h)));
    }
}

fn colors(ctx: &Context) -> (Color32, Color32) {
    let t = theme::tokens(ctx);
    match ctx.theme() {
        Theme::Dark => (t.surface_2, t.surface_3),
        Theme::Light => (t.surface_3, t.surface_2),
    }
}

/// 灰色の帯 1 本と、その上を流れる光。`t` は ctx の time（秒）。
pub fn paint_shimmer(painter: &Painter, rect: Rect, t: f64) {
    let (base, _) = colors(painter.ctx());
    let r = (rect.height() / 2.0).min(theme::RADIUS_SM as f32);
    painter.rect_filled(rect, r, base);
    // 角の丸みに光がはみ出さないよう、左右を丸みの分だけ削った所にだけ流す
    shine(painter, rect.shrink2(Vec2::new(r, 0.0)), t);
}

fn circle(painter: &Painter, center: Pos2, radius: f32, t: f64) {
    let (base, _) = colors(painter.ctx());
    painter.circle_filled(center, radius, base);
    // 内接する正方形にだけ流す
    shine(painter, Rect::from_center_size(center, Vec2::splat(radius * std::f32::consts::SQRT_2)), t);
}

fn shine(painter: &Painter, clip: Rect, t: f64) {
    let ctx = painter.ctx();
    if clip.width() <= 0.0 || motion::reduce_motion(ctx) {
        return;
    }
    ctx.request_repaint();
    let screen = ctx.screen_rect();
    let phase = (t / SHIMMER_PERIOD).rem_euclid(1.0) as f32;
    let cx = screen.left() - SHINE_W + (screen.width() + 2.0 * SHINE_W) * phase;
    let (l, r) = (cx - SHINE_W / 2.0, cx + SHINE_W / 2.0);
    if r < clip.left() || l > clip.right() {
        return;
    }
    let (_, light) = colors(ctx);
    let mut mesh = Mesh::default();
    for (x, c) in [(l, Color32::TRANSPARENT), (cx, light), (r, Color32::TRANSPARENT)] {
        mesh.colored_vertex(pos2(x, clip.top()), c);
        mesh.colored_vertex(pos2(x, clip.bottom()), c);
    }
    for k in [0, 2] {
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.with_clip_rect(clip.intersect(painter.clip_rect())).add(Shape::mesh(mesh));
}

/// `shell_skeleton` が描いた 3 面（header の高さ・sidebar の幅を test と統合役が読む）。
#[allow(dead_code)] // bin は描くだけで読まない
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellRects {
    pub header: Rect,
    pub sidebar: Rect,
    pub body: Rect,
    /// 本文の列（見出しと行の帯が並ぶ幅）。本物の draw_app_region がこの pass か直前の pass に列を
    /// `note_main_column` で置いていればその x 範囲（縦 scrollbar の分を引いた後）、無ければ式で出す。
    pub column: Rect,
    /// 1 つの欄の帯。本物の `header::command_field` がこの pass か直前の pass に置いた rect（Home = 主領域の縦の中央・G83）、無ければ header の中央。
    pub field: Rect,
}

#[derive(Clone, Copy)]
struct MainColumn {
    pass: u64,
    left: f32,
    right: f32,
}

fn main_column_id() -> Id {
    Id::new("kangae.skeleton.main_column")
}

/// draw_app_region が本文の列を置いた `ui`（幅を決めた後）の中で呼ぶ。本物がこの pass で実際に使った列の x 範囲を覚える。
/// 縦 scrollbar（theme の thin = 6px）が出るかは中身の長さと前の frame で決まり、出る間は列がその分だけ動くので、
/// 骨格は式で当てずに本物の値を読む（cross-fade で列が横に跳ばない）。
pub fn note_main_column(ui: &Ui) {
    let r = ui.max_rect();
    let ctx = ui.ctx();
    let col = MainColumn { pass: ctx.cumulative_pass_nr(), left: r.left(), right: r.right() };
    ctx.data_mut(|d| d.insert_temp(main_column_id(), col));
}

/// この pass か直前の pass に本物が置いた列で、`body` の中に収まる物だけ（View が変わった・rail を開いた後の古い値は使わない）。
fn noted_main_column(ctx: &Context, body: Rect) -> Option<(f32, f32)> {
    let c: MainColumn = ctx.data(|d| d.get_temp(main_column_id()))?;
    let fresh = c.pass + 1 >= ctx.cumulative_pass_nr();
    let inside = c.left >= body.left() - 0.5 && c.right <= body.right() + 0.5 && c.right >= c.left;
    (fresh && inside).then_some((c.left, c.right))
}

/// `header::command_field` が欄を置いた rect を覚える（G83: Home は主領域・他は header。骨格の欄の帯をそこへ置く）。
pub fn note_command_field(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new("kangae.skeleton.command_field"), (pass, rect)));
}

/// この pass か直前の pass に本物が置いた欄の rect。
pub fn noted_command_field(ctx: &Context) -> Option<Rect> {
    let (pass, rect): (u64, Rect) = ctx.data(|d| d.get_temp(Id::new("kangae.skeleton.command_field")))?;
    (pass + 1 >= ctx.cumulative_pass_nr()).then_some(rect)
}

/// 殻の骨格を `screen` に描く（painter だけ = Area の外からも重ねられる）。header 56・sidebar（nav 3 行〈`View::NAV`〉+ app 3 行）・本文（見出し 1 + 行 5）。
/// gutter は本物と同じ `Tier::for_width`。
/// 本文の右端は `ctx.available_rect()`（pin した rail の分だけ狭い）。本文の列は本物が `note_main_column` で置いた列
/// （縦 scrollbar の分まで同じ）。本物がまだ描かれていない時（起動直後）は draw_app_region と同じ式の中央寄せ。
/// 契約（UI-4b）: sidebar の帯の右端は本物の sidebar が残した `ctx.available_rect().left()`（rail の右端と同じ読み方・
/// `sidebar::draw` の SidePanel の幅 = 覚えた畳みと窓の幅の既定を解いた後の値）。畳んで SidePanel を描かない時は帯 0（本文が窓の左から）＝ UI-4 の Done 7。
/// 起動の骨格（`install_startup` の on_end_pass）は app の update の後に描くので、`ShibubuApp::new` の前に入れても覚えた状態を知る（store を骨格に渡す口は作らない）。
/// 帯の中身は nav 3 行 + app 3 行（64 の icon の列の分岐は消した）を開いた幅（`tier.sidebar`）で組み、帯の幅で切る（開閉の 180ms の途中も本物と同じ見え方）。例: `let r = paint_shell(&painter, ctx.screen_rect(), t); r.sidebar.width()`。
/// 保証しない: 本物の sidebar を描かない frame で `tier.sidebar` の帯を出す事（試験が骨格だけを描く時は帯 0。帯を見る試験は SidePanel を先に置く）。
/// 契約（G83）: 欄の帯は本物の欄が置いた rect（`note_command_field`・起動の Home なら主領域の縦の中央）・無ければ header の中央。欄が本文の中なら本文の帯はその下から。
/// Wave: UI-4b, M2
pub fn paint_shell(painter: &Painter, screen: Rect, t: f64) -> ShellRects {
    let tk = theme::tokens(painter.ctx());
    let tier = Tier::for_width(screen.width());
    let header = Rect::from_min_size(screen.min, Vec2::new(screen.width(), theme::HEADER_H));
    // G89: clamp でなく max → min（裏返った screen〈幅が負〉で f32::clamp が panic しない）
    let side_right = painter.ctx().available_rect().left().max(screen.left()).min(screen.right());
    let sidebar = Rect::from_min_max(pos2(screen.left(), header.bottom()), pos2(side_right, screen.bottom()));
    // pin した rail（SidePanel::right）がある時は本物の central と同じく右端が rail の分だけ狭くなる
    let right = screen.right().min(painter.ctx().available_rect().right()).max(sidebar.right());
    let body = Rect::from_min_max(pos2(sidebar.right(), header.bottom()), pos2(right, screen.bottom()));
    let rail = Rect::from_min_max(pos2(right, header.bottom()), screen.max);
    painter.rect_filled(header, 0.0, tk.surface_1);
    painter.rect_filled(body, 0.0, tk.bg_app);
    let line = |r: Rect| painter.rect_filled(r, 0.0, tk.border);
    line(Rect::from_min_max(pos2(header.left(), header.bottom() - theme::HAIRLINE), header.max));
    let bar = |r: Rect| paint_shimmer(painter, r, t);
    let mid = |r: Rect, h: f32| r.center().y - h / 2.0;

    // header: 左に名前・中央に command・右に丸
    bar(Rect::from_min_size(pos2(header.left() + theme::SP_6, mid(header, 16.0)), Vec2::new(96.0, 16.0)));
    // 本物の 1 つの欄と同じ rect（本物が置いた所 = Home は主領域の縦の中央・G83。無ければ header の中央・幅 `Tier::input_w`・UI-4a 決定 1）
    let field = noted_command_field(painter.ctx()).unwrap_or_else(|| Rect::from_center_size(header.center(), Vec2::new(tier.input_w, theme::COMMAND_H)));
    bar(field);
    circle(painter, pos2(header.right() - theme::SP_6 - 14.0, header.center().y), 14.0, t);

    // sidebar（畳んでいれば帯 0 で描かない）: nav 行（`View::NAV`）→ 区切り → app 3 行。開いた幅で組んで帯の幅で切る
    if sidebar.width() > 0.0 {
        let sp = painter.with_clip_rect(sidebar.intersect(painter.clip_rect()));
        sp.rect_filled(sidebar, 0.0, tk.surface_1);
        sp.rect_filled(Rect::from_min_max(pos2(sidebar.right() - theme::HAIRLINE, sidebar.top()), sidebar.max), 0.0, tk.border);
        let sbar = |r: Rect| paint_shimmer(&sp, r, t);
        let (pad, full) = (theme::SP_3, tier.sidebar);
        let mut y = sidebar.top() + theme::SP_4;
        for i in 0..View::NAV.len() {
            let row = Rect::from_min_size(pos2(sidebar.left() + pad, y), Vec2::new(full - 2.0 * pad, theme::NAV_ROW_H));
            sbar(Rect::from_min_size(pos2(row.left() + theme::SP_2, mid(row, theme::ICON_NAV)), Vec2::splat(theme::ICON_NAV)));
            sbar(Rect::from_min_size(pos2(row.left() + theme::SP_2 + theme::ICON_NAV + theme::SP_3, mid(row, LINE_H)), Vec2::new(row.width() * 0.45 * width_at(i), LINE_H)));
            y += theme::NAV_ROW_H;
        }
        y += theme::SP_6;
        sbar(Rect::from_min_size(pos2(sidebar.left() + pad + theme::SP_2, y), Vec2::new(64.0, 10.0)));
        y += theme::SP_6;
        for i in 0..3 {
            let row = Rect::from_min_size(pos2(sidebar.left() + pad, y), Vec2::new(full - 2.0 * pad, theme::APP_ROW_H));
            circle(&sp, pos2(row.left() + theme::SP_2 + 14.0, row.center().y), 14.0, t);
            sbar(Rect::from_min_size(pos2(row.left() + 48.0, mid(row, LINE_H)), Vec2::new((row.width() - 56.0) * width_at(i + 1), LINE_H)));
            y += theme::APP_ROW_H;
        }
    }

    // 本文: 見出し 1 本 + 行 5 本（主領域の列と同じ gutter・上余白）
    // 本物が置いた列があればそれ（scrollbar が出ている時はその分だけ狭く左へ寄る）。
    // 無ければ shell.rs draw_app_region と同じ式（gutter・最大 1080・中央寄せ・scrollbar なし）
    let (left, width) = match noted_main_column(painter.ctx(), body) {
        Some((l, r)) => (l, r - l),
        None => {
            let width = (body.width() - 2.0 * tier.gutter).max(0.0).min(theme::MAX_TEXT_COLUMN);
            (body.left() + ((body.width() - width) / 2.0).max(tier.gutter), width)
        }
    };
    // 欄が本文の中（Home）なら帯はその下から・欄と同じ左端と幅（本物の Try と Continue の所・G83b）
    let home = body.contains_rect(field);
    let mut y = if home { field.bottom() + theme::SP_6 } else { body.top() + theme::SP_7 };
    let (bl, bw) = if home { (field.left(), field.width()) } else { (left, width) };
    bar(Rect::from_min_size(pos2(bl, y), Vec2::new(280.0_f32.min(bw), 28.0)));
    y += 28.0 + theme::SP_6;
    for i in 0..5 {
        let row = Rect::from_min_size(pos2(bl, y), Vec2::new(bw, theme::LIST_ROW_H));
        bar(Rect::from_min_size(pos2(bl, mid(row, LINE_H)), Vec2::new(bw * width_at(i), LINE_H)));
        y += theme::LIST_ROW_H;
    }
    // rail: tab の帯 + 行 3 本
    if rail.width() > 0.0 {
        painter.rect_filled(rail, 0.0, tk.surface_1);
        line(Rect::from_min_max(rail.min, pos2(rail.left() + theme::HAIRLINE, rail.bottom())));
        let inner = rail.shrink(theme::SP_4);
        bar(Rect::from_min_size(inner.min, Vec2::new(inner.width() * 0.7, 16.0)));
        for i in 0..3 {
            bar(Rect::from_min_size(pos2(inner.left(), inner.top() + 48.0 + 28.0 * i as f32), Vec2::new(inner.width() * width_at(i), LINE_H)));
        }
    }
    let column = Rect::from_min_max(pos2(left, body.top()), pos2(left + width, body.bottom()));
    ShellRects { header, sidebar, body, column, field }
}

/// 起動直後の全画面の骨格。`ui` の残りを全部取る（CentralPanel に余白 0 で置く）。
#[allow(dead_code)] // 統合役が app の中で読み込みを待つ時
pub fn shell_skeleton(ui: &mut Ui) -> ShellRects {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    paint_shell(ui.painter(), rect, now(ui.ctx()))
}

/// View を切り替えた直後の主領域の骨格（その画面の最終的な形）。
#[allow(dead_code)] // View の中身を待つ物が出来た時（今は手元の data で 120ms 未満に出る）
pub fn view_skeleton(ui: &mut Ui, view: &View) {
    // 見出し（Display / HeadingLg 相当の太い帯）
    let w = ui.available_width();
    let (h, _) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::hover());
    bar(ui, Rect::from_min_size(h.min, Vec2::new(w.min(280.0), 28.0)));
    ui.add_space(theme::SP_4);
    match view {
        View::Home => {
            ui.horizontal(|ui| {
                for _ in 0..3 {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(88.0, theme::BUTTON_COMPACT_H), Sense::hover());
                    bar(ui, r);
                }
            });
            ui.add_space(theme::SP_6);
            Skeleton::Line.show(ui);
            Skeleton::List(3).show(ui);
        }
        View::Runs => {
            Skeleton::Table(6, 4).show(ui);
        }
        View::Connections => {
            for _ in 0..3 {
                Skeleton::Card.show(ui);
            }
        }
        View::Settings(_) => {
            Skeleton::List(4).show(ui);
        }
        View::App(_) | View::Project { .. } | View::CompatCanvas(_) => {
            Skeleton::Line.show(ui);
            Skeleton::Table(5, 3).show(ui);
        }
    }
}

/// renderer の component ごとの骨格（`domain::ui::BUILTIN_COMPONENTS` の名前）。data を持たない部品・知らない名前は block。
#[allow(dead_code)] // renderer が呼ぶまで
pub fn region_skeleton(ui: &mut Ui, component_id: &str) {
    let w = ui.available_width();
    match component_id {
        "list" => {
            Skeleton::List(5).show(ui);
        }
        "table" => {
            Skeleton::Table(5, 4).show(ui);
        }
        "form" => {
            Skeleton::Form(4).show(ui);
        }
        "detail" => {
            Skeleton::Line.show(ui);
            rows(ui, 5, theme::LIST_ROW_H, |ui, r, i| {
                bar(ui, line_in(r, r.left(), r.width() * 0.22));
                bar(ui, line_in(r, r.left() + r.width() * 0.3, r.width() * 0.55 * width_at(i)));
            });
        }
        "chat" => {
            // 相手（左）と自分（右）の吹き出しが交互
            rows(ui, 4, 56.0, |ui, r, i| {
                let bw = r.width() * if i % 2 == 0 { 0.6 } else { 0.45 };
                let left = if i % 2 == 0 { r.left() } else { r.right() - bw };
                bar(ui, Rect::from_min_size(pos2(left, r.top() + 6.0), Vec2::new(bw, 44.0)));
            });
            Skeleton::Block(theme::INPUT_H).show(ui);
        }
        "media" => {
            Skeleton::Block((w * 9.0 / 16.0).min(360.0)).show(ui);
            Skeleton::Line.show(ui);
        }
        "timeline" => {
            let t = theme::tokens(ui.ctx());
            let r = rows(ui, 5, theme::LIST_ROW_H, |ui, r, i| {
                circle(ui.painter(), pos2(r.left() + 6.0, r.center().y), 6.0, now(ui.ctx()));
                bar(ui, line_in(r, r.left() + 24.0, (r.width() - 24.0) * 0.6 * width_at(i)));
            });
            let x = r.left() + 6.0;
            ui.painter().rect_filled(Rect::from_min_max(pos2(x - 0.5, r.top() + 20.0), pos2(x + 0.5, r.bottom() - 20.0)), 0.0, t.border);
        }
        "calendar" => {
            Skeleton::Line.show(ui);
            let cw = w / 7.0;
            rows(ui, 5, 64.0, |ui, r, _| {
                for c in 0..7 {
                    bar(ui, Rect::from_min_size(pos2(r.left() + cw * c as f32 + 2.0, r.top() + 2.0), Vec2::new(cw - 4.0, 60.0)));
                }
            });
        }
        "editor" => {
            Skeleton::Block(32.0).show(ui);
            rows(ui, 8, 22.0, |ui, r, i| bar(ui, line_in(r, r.left(), r.width() * width_at(i * 3))));
        }
        "chart" => {
            Skeleton::Block(200.0).show(ui);
        }
        _ => {
            Skeleton::Block(120.0).show(ui);
        }
    }
}

#[derive(Clone, Copy)]
struct LoadState {
    /// この状態が属する読み込みの started_at（bit）。違う読み込みが来たら作り直す = region 1 つにつき temp は 1 件
    started_at: u64,
    shown: bool,
    loaded_at: Option<f64>,
}

/// 読み込み中は骨格・中身が来たら帯から本物へ cross-fade。`id_salt` は呼び出し位置ごとに変える（同じ親の中の region を分ける）。
/// `started_at` は読み込みを始めた ctx の time（秒）で、読み込みごとに変える（同じ値の間は同じ読み込みとみなす）。
/// 120ms 未満で読み終わったら骨格を描かず本物をそのまま出す。読み込み中は `None`。
/// どの状態でも同じ形の scope 1 つの中に描く = 本物の中の id と、後ろに並ぶ兄弟の auto id が状態で変わらない。
#[allow(dead_code)] // renderer の data region が呼ぶまで
pub fn with_skeleton<R>(ui: &mut Ui, id_salt: impl std::hash::Hash, loaded: bool, started_at: f64, skeleton: Skeleton, real: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    with_skeleton_by(ui, id_salt, loaded, started_at, |ui: &mut Ui| { skeleton.show(ui); }, real)
}

/// `with_skeleton` の骨格を部品 1 つでなく描く関数で渡す形（生成中の画面 = 予定の component ごとの `region_skeleton` を並べる）。
/// `draw` は読み込み中と cross-fade の後ろの薄い骨格の 2 か所で呼ぶ。
#[allow(dead_code)] // build_progress・portal_frame が app.rs から呼ばれるまで
pub fn with_skeleton_by<R>(ui: &mut Ui, id_salt: impl std::hash::Hash, loaded: bool, started_at: f64, draw: impl Fn(&mut Ui), real: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let ctx = ui.ctx().clone();
    let id = ui.id().with(("kangae.skeleton", id_salt));
    // 状態は region の id 1 つに置く（started_at を key に入れると読み込みの回数だけ temp に積まれ、egui は temp を消さない）
    let fresh = LoadState { started_at: started_at.to_bits(), shown: false, loaded_at: None };
    let mut st = ctx.data(|d| d.get_temp::<LoadState>(id)).filter(|s| s.started_at == fresh.started_at).unwrap_or(fresh);
    let t = now(&ctx);
    ui.scope_builder(UiBuilder::new().id_salt(id), |ui| {
        if !loaded {
            let wait = SHOW_AFTER - (t - started_at);
            if wait > 0.0 {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.min(SHOW_AFTER)));
            } else {
                draw(ui);
                if !st.shown {
                    st.shown = true;
                    ctx.data_mut(|d| d.insert_temp(id, st));
                }
            }
            return None;
        }
        let loaded_at = *st.loaded_at.get_or_insert(t);
        ctx.data_mut(|d| d.insert_temp(id, st));
        let p = if st.shown { motion::ease_out(motion::progress(&ctx, loaded_at, motion::dur(&ctx, motion::PAGE))) } else { 1.0 };
        let base = ui.opacity();
        let start = ui.available_rect_before_wrap();
        ui.multiply_opacity(p);
        let r = real(ui);
        if p < 1.0 {
            // 本物の後ろに置く = 本物の id の系列に触らない
            let mut ghost = ui.new_child(UiBuilder::new().id_salt("kangae.skeleton.ghost").max_rect(start));
            ghost.set_opacity(base * (1.0 - p));
            draw(&mut ghost);
        }
        Some(r)
    })
    .inner
}

/// `KANGAE_DEV_SKELETON` で重ねられる最長の秒数（1 時間）。
pub const DEV_HOLD_MAX: f64 = 3600.0;

/// `KANGAE_DEV_SKELETON` の値 → 骨格を重ねる秒数。`1` は 1.5s・それ以外の `DEV_HOLD_MAX` 以下の正の数はその秒数・他は `None`（見本を出さない）。
pub fn dev_hold(raw: Option<&str>) -> Option<f64> {
    match raw.map(str::trim)? {
        "1" => Some(1.5),
        s => match s.parse::<f64>() {
            // inf・NaN・1 時間を超える数は弾く。Duration に入る数でも 9.2e18 秒を超えると
            // eframe の `Instant::now() + delay` が溢れて panic する（reduce motion の時に request_repaint_after が届く）
            Ok(v) if v > 0.0 && v <= DEV_HOLD_MAX => Some(v),
            _ => None,
        },
    }
}

/// 起動直後の殻の骨格（§15.2-2 起動直後）。eframe は最初の frame を描くまで窓を隠すので、app を組む間（font・store の読み込み）は
/// 骨格も描けない。最初の pass の時刻（eframe の開始からの秒）が 120ms 以上なら、その pass で殻の骨格を最前面に置き、
/// そこから 220ms で本物へ fade する（待たせる hold は置かない）。120ms 未満・reduce motion なら描かない。
/// `KANGAE_DEV_SKELETON` の値（`dev_hold`）があれば、その秒数だけ骨格を止めてから fade する（写真を撮る時の見本）。
pub fn install_startup(ctx: &Context) {
    let hold = dev_hold(std::env::var("KANGAE_DEV_SKELETON").ok().as_deref());
    let start_id = Id::new("kangae.startup_skeleton.start");
    ctx.on_end_pass(
        "kangae-startup-skeleton",
        std::sync::Arc::new(move |ctx: &Context| {
            let t = now(ctx);
            let start = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(start_id, || t));
            if hold.is_none() && start < SHOW_AFTER {
                return;
            }
            let hold = hold.unwrap_or(0.0);
            let alpha = if t - start < hold {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(hold - (t - start)));
                1.0
            } else {
                1.0 - motion::ease_out(motion::progress(ctx, start + hold, motion::dur(ctx, motion::PAGE)))
            };
            if alpha <= 0.0 {
                return;
            }
            let mut painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("kangae.startup_skeleton")));
            painter.multiply_opacity(alpha);
            paint_shell(&painter, ctx.screen_rect(), t);
        }),
    );
}

/// main.rs が呼ぶ名前（main.rs は統合役の持ち場なので名前だけ残す。中身は本番の道 `install_startup`）。
#[allow(unused_imports)] // tests/ui_motion.rs は install_startup を直に呼ぶ
pub use install_startup as install_dev_overlay;
