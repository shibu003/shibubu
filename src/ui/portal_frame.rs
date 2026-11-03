//! v0.14 §7.1・§7.2 の Portal の枠（元の app の画面が生成 app の中に入る時の境界）。図は docs/diagrams.md §20。
//! - 枠は 1 つの大きな作業領域（available の残り全部・card に入れない・角丸 0・外周 1px）。上の帯 32px に 1 行の label と次の一手 1 つ
//! - 1 行 = glyph + app 名 · 状態語 · 同期の確度（`sync_label`・copy.en.md の英語）。飾りの badge は置かない。
//!   mode（Manual / Mirror / External）は表に英語の語が無いので帯に出さない（UI-1・sendback UI-1-c。`PortalFrame::mode` は残す）
//! - 帯の下の slot は呼ぶ側に返す（p-portal が元の窓を重ね、p-mirror が映像を描く）。button は slot に置かない（元の窓が覆う）
//! - 接続中は slot に skeleton（`with_skeleton`）・状態の変化は cross-fade・reduce motion で 0（DESIGN §15.2-1・2）
//! - 型は p-portal の `src/surface`（未 merge）と名前を合わせた最小の物。`SyncMode` は variant 名も語も同じ。他の対応は図 §20 の表

use super::motion;
use super::skeleton::{self, Skeleton};
use super::states::{self, RegionState};
use super::theme::{self, Type, Variant};
use eframe::egui::{self, accesskit, pos2, Align, Color32, Id, Layout, Rect, Sense, StrokeKind, Ui, UiBuilder, Vec2};
use std::hash::Hash;

/// W2c: 9 状態（PortalState）と同期の確度（SyncMode）の正本は `src/surface`（serde だけの module）。ここは描き方（語・glyph・次の一手）だけを足す。
/// Lifecycle（12 状態）からの写しは `surface::portal_state`。
pub use crate::surface::{PortalState, SyncMode};

/// 枠の画面座標: egui の rect（窓の中身の左上からの point）に窓の中身の左上（global の point）を足す。Retina の倍率は掛けない。
/// 例: renderer が `surface_placed(id, Host { slot: slot_on_screen(ui.ctx(), resp.slot), .. }, action)` で返す。
/// 保証しない: viewport の位置がまだ来ていない最初の frame は None を返す。
#[cfg_attr(not(any(feature = "native_direct", feature = "mirror")), allow(dead_code))]
pub fn slot_on_screen(ctx: &egui::Context, rect: Rect) -> Option<crate::surface::Frame> {
    let inner = ctx.input(|i| i.viewport().inner_rect)?;
    let in_host = crate::surface::Frame { x: rect.min.x as f64, y: rect.min.y as f64, w: rect.width() as f64, h: rect.height() as f64 };
    Some(crate::surface::PortalSlot { id: String::new(), in_host }.on_screen(inner.min.x as f64, inner.min.y as f64))
}

impl PortalState {
    /// 帯と slot に出す状態語（初めての人の言葉。§7.2 の名前は doc に）。Native Direct（release の Web の面の Live）は空 =
    /// 状態語を出さない（copy.en.md: `直接操作中 → (removed with Native Direct)`・帯は「state only when not Ready」）。
    pub const fn wording(self) -> &'static str {
        match self {
            PortalState::Undiscovered => "Not found",
            PortalState::PermissionRequired => "Needs permission",
            PortalState::Attaching => "Attaching",
            PortalState::NativeDirect => "",
            PortalState::Mirror => "Mirroring",
            PortalState::ViewOnly => "View only",
            // B23（DOGFOOD-X 周 3: 動いている Zoom を「Not running · Zoom's window is hidden」と言った）: 窓が無い・閉じた・隠れているも同じ状態なので、
            // app が動いているかを言わない語
            PortalState::SourceUnavailable => "Not available",
            PortalState::SyncDelayed => "Behind",
            PortalState::IntegrationFailed => "Couldn't attach",
        }
    }

    /// glyph と色は §11 の region 状態から借りる（色だけで状態を示さない・font の glyph の test も同じ表）。
    pub const fn region(self) -> RegionState {
        match self {
            PortalState::Undiscovered => RegionState::Empty,
            PortalState::PermissionRequired => RegionState::Blocked,
            PortalState::Attaching => RegionState::Loading,
            PortalState::NativeDirect | PortalState::Mirror => RegionState::Ready,
            PortalState::ViewOnly => RegionState::Partial,
            PortalState::SourceUnavailable => RegionState::Disconnected,
            PortalState::SyncDelayed => RegionState::Stale,
            PortalState::IntegrationFailed => RegionState::Error,
        }
    }

    /// 状態ごとにちょうど 1 つの次の一手。
    pub const fn next_step(self) -> PortalAction {
        match self {
            PortalState::Undiscovered => PortalAction::Rediscover,
            PortalState::PermissionRequired => PortalAction::OpenPermission,
            PortalState::Attaching => PortalAction::Cancel,
            PortalState::NativeDirect => PortalAction::Detach,
            PortalState::Mirror | PortalState::ViewOnly => PortalAction::OpenSourceWindow,
            PortalState::SourceUnavailable => PortalAction::Retry,
            PortalState::SyncDelayed => PortalAction::CheckUpdate,
            PortalState::IntegrationFailed => PortalAction::ContinueInSource,
        }
    }

    /// RQ4（RQ-018・UIUX C04・N06）: 帯の右に次の一手と並べる 2 つ目の手（元の app を手で使い続ける道）。`ContinueInSource`（本体で文字を `Open original` に替える）を、
    /// 次の一手が既に元の窓を開く手（`OpenSourceWindow`・`ContinueInSource`）の状態と、元の app がまだ見つかっていない `Undiscovered` を除く全部の状態で返す
    /// （`PermissionRequired` = `Open settings` と `Open original` の 2 つ = 許可を拒んでも元の app へ行ける）。
    /// 例（本体の `strip_line`）: `if let Some(alt) = frame.state.alt_step() { /* 次の一手の左に Ghost の button */ }`
    /// 保証する: 次の一手と同じ手を返さない。保証しない: 権限を最初の起動でまとめて求めない事（呼ぶ側の約束 = bridge の `permission {prompt:true}` は人が押した
    /// `Open settings` と Mirror の許可の button からだけ）。
    /// Wave: RQ4
    pub const fn alt_step(self) -> Option<PortalAction> {
        match (self, self.next_step()) {
            (PortalState::Undiscovered, _) | (_, PortalAction::OpenSourceWindow | PortalAction::ContinueInSource) => None,
            _ => Some(PortalAction::ContinueInSource),
        }
    }

    /// 元の app の画面（重ねた窓・Mirror の映像）が slot に在る状態。他は slot に事実 1 行（偽の画面で埋めない・DESIGN §14.3）。
    pub const fn has_visual(self) -> bool {
        matches!(self, PortalState::NativeDirect | PortalState::Mirror | PortalState::ViewOnly | PortalState::SyncDelayed)
    }
}

impl SyncMode {
    const fn is_live(self) -> bool {
        matches!(self, SyncMode::VisualLive | SyncMode::SemanticLive)
    }
}

/// 人が元の app をどう操作しているか（§7.1 の `Manual / Mirror / External`）。
/// Manual = NativeDirect × HumanDirect・Mirror = InteractiveMirror × HumanForwarded・External = ExternalHandoff。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Manual,
    Mirror,
    External,
}

/// 同期の確度の画面の語（copy.en.md の「ライブ画面 / 状態連携中 / …」の行）。`SyncMode::label` は surface の日本語のまま（kangae-portal の行が読む）。
pub const fn sync_label(s: SyncMode) -> &'static str {
    match s {
        SyncMode::VisualLive => "Live view",
        SyncMode::SemanticLive => "Live state",
        SyncMode::SemanticPolled => "Checked periodically",
        SyncMode::InferredUnverified => "Guessed",
        SyncMode::Stale => "May be out of date",
        SyncMode::Unsupported => "Can't read its state",
    }
}

/// 次の一手（呼ぶ側が p-portal / p-mirror の操作へ繋ぐ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortalAction {
    Rediscover,
    OpenPermission,
    Cancel,
    /// 重ねるのをやめ、元の窓を外に残す（External Handoff）
    Detach,
    OpenSourceWindow,
    Retry,
    CheckUpdate,
    ContinueInSource,
}

impl PortalAction {
    pub const fn label(self) -> &'static str {
        match self {
            PortalAction::Rediscover => "Find again",
            PortalAction::OpenPermission => "Open settings",
            PortalAction::Cancel => "Cancel",
            PortalAction::Detach => "Stop mirroring",
            PortalAction::OpenSourceWindow => "Open window",
            PortalAction::Retry => "Try again",
            PortalAction::CheckUpdate => "Check now",
            PortalAction::ContinueInSource => "Open original",
        }
    }

    /// 直す一手は Primary・作業中の面から抜ける一手は Ghost（帯の上で操作中の画面より目立たせない）。
    pub const fn variant(self) -> Variant {
        match self {
            PortalAction::Cancel | PortalAction::Detach | PortalAction::OpenSourceWindow => Variant::Ghost,
            _ => Variant::Primary,
        }
    }
}

/// 枠に渡す 1 frame ぶんの事実。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalFrame<'a> {
    pub app_name: &'a str,
    pub state: PortalState,
    pub sync: SyncMode,
    /// 同期遅延の時だけ読む（External で外に出した後もここ）。Native Direct 中・Mirror 中・閲覧専用は状態で決まり、他は出さない。
    pub mode: Option<Mode>,
    /// 状態の事実 1 行（許可待ち = 権限と目的・同期遅延 = 最終確認の時刻と根拠・一体化失敗 = 理由・Mirror 中 = 入力の範囲）。空なら出さない。
    pub detail: &'a str,
}

impl PortalFrame<'_> {
    /// 帯に出す同期の確度。遅延と live は並べない・終了した app の情報は古い。
    pub fn shown_sync(&self) -> Option<SyncMode> {
        match self.state {
            PortalState::NativeDirect | PortalState::Mirror | PortalState::ViewOnly => Some(self.sync),
            PortalState::SyncDelayed if self.sync.is_live() => Some(SyncMode::Stale),
            PortalState::SyncDelayed => Some(self.sync),
            PortalState::SourceUnavailable => Some(SyncMode::Stale),
            _ => None,
        }
    }

    /// 帯で状態語の後ろに並べる語（同期の確度）。
    fn meta(&self) -> Option<&'static str> {
        self.shown_sync().map(sync_label)
    }

    /// 帯の末尾の detail。映像の無い状態の detail は slot の中央に出す（帯と 2 度書かない）。
    fn strip_detail(&self) -> &str {
        if self.state.has_visual() {
            self.detail.trim()
        } else {
            ""
        }
    }

    /// 帯の 1 行の文字（cua の elements に出る値と同じ）。
    pub fn status_line(&self) -> String {
        let mut parts: Vec<&str> = vec![self.app_name];
        parts.extend(Some(self.state.wording()).filter(|w| !w.is_empty()));
        parts.extend(self.meta());
        parts.extend(Some(self.strip_detail()).filter(|d| !d.is_empty()));
        parts.join(" · ")
    }
}

/// W16-1: `PortalFrame` の持ち主つきの写し（`ActionHost::surface_portal` が返す・frame をまたいで持てる）。
/// 例: `if let Some(v) = cx.host.surface_portal("docs") { let r = portal_frame::show(ui, "docs", &v.frame(), |_| {}); }`
/// 保証する: `frame()` は同じ値の `PortalFrame` を借りて返すだけ（語・glyph は `PortalFrame` と同じ）。
#[derive(Clone, Debug, PartialEq)]
pub struct PortalView {
    pub app_name: String,
    pub state: PortalState,
    pub sync: SyncMode,
    pub mode: Option<Mode>,
    pub detail: String,
}

impl PortalView {
    pub fn frame(&self) -> PortalFrame<'_> {
        PortalFrame { app_name: &self.app_name, state: self.state, sync: self.sync, mode: self.mode, detail: &self.detail }
    }
}

/// `show` が返す物。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalResponse {
    /// 帯の下の作業領域（p-portal の PortalSlot に渡す rect）。
    pub slot: Rect,
    /// この frame で押された次の一手。
    pub action: Option<PortalAction>,
}

#[derive(Clone, Copy)]
struct Seen {
    state: PortalState,
    prev: PortalState,
    changed_at: f64,
    /// 最後に接続中へ入った時刻（`with_skeleton` の読み込みの key・接続中を抜けても持つ = 骨格から本物へ cross-fade）
    attach_started: f64,
}

/// 状態の替わり目を覚える（枠 1 つに temp 1 件）。初めて見た状態は動かさない。
fn remember(ctx: &egui::Context, id: Id, state: PortalState) -> Seen {
    let now = ctx.input(|i| i.time);
    let key = id.with("kangae.portal.seen");
    let attach = |s: PortalState, keep: f64| if s == PortalState::Attaching { now } else { keep };
    let seen = match ctx.data(|d| d.get_temp::<Seen>(key)) {
        None => Seen { state, prev: state, changed_at: f64::NEG_INFINITY, attach_started: attach(state, f64::NEG_INFINITY) },
        Some(s) if s.state == state => s,
        Some(s) => Seen { state, prev: s.state, changed_at: now, attach_started: attach(state, s.attach_started) },
    };
    ctx.data_mut(|d| d.insert_temp(key, seen));
    seen
}

/// 枠の egui の id（`show` が帯・slot・AX の node に使う）。同じ `ui`・`id_salt` で `show` の AX の node を引く（renderer の Native Direct の枠が identifier を足す）。
pub fn frame_id(ui: &Ui, id_salt: impl Hash) -> Id {
    ui.id().with(("kangae.portal", id_salt))
}

/// Portal の枠を `ui` の残り全部に描く。`id_salt` は同じ親の中の枠ごとに変える。
/// `body` は映像のある状態（`has_visual`）の時だけ slot の中で呼ぶ（Mirror の映像など。Native Direct は元の窓が上に重なる）。
pub fn show(ui: &mut Ui, id_salt: impl Hash, frame: &PortalFrame, body: impl FnOnce(&mut Ui)) -> PortalResponse {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let id = frame_id(ui, id_salt);
    let seen = remember(&ctx, id, frame.state);
    let outer = ui.available_rect_before_wrap();
    ui.allocate_rect(outer, Sense::hover());
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(accesskit::Role::Group);
        n.set_label(match frame.state.wording() {
            "" => frame.app_name.to_owned(),
            w => format!("{} · {w}", frame.app_name),
        });
        n.set_bounds(accesskit::Rect { x0: outer.min.x.into(), y0: outer.min.y.into(), x1: outer.max.x.into(), y1: outer.max.y.into() });
    });

    // 境界: 外周 1px と帯の下の 1px だけ（塗らない・角丸 0 = card にしない）
    let painter = ui.painter_at(outer);
    painter.rect_stroke(outer, 0.0, theme::hairline(&t), StrokeKind::Inside);
    let strip = Rect::from_min_size(outer.min, Vec2::new(outer.width(), theme::STRIP_H));
    painter.rect_filled(Rect::from_min_max(pos2(strip.left(), strip.bottom() - theme::HAIRLINE), strip.max), 0.0, t.border);
    let slot = Rect::from_min_max(pos2(outer.left() + theme::HAIRLINE, strip.bottom()), outer.max - Vec2::splat(theme::HAIRLINE));

    let color = motion::cross_fade(&ctx, id.with("kangae.portal.color"), frame.state.region().color(&t));
    let action = strip_line(ui, id, strip, frame, color);

    let mut slot_ui = ui.new_child(UiBuilder::new().id_salt(id.with("kangae.portal.slot")).max_rect(slot).layout(Layout::top_down(Align::Min)));
    slot_ui.set_clip_rect(slot.intersect(ui.clip_rect()));
    let loaded = frame.state != PortalState::Attaching;
    skeleton::with_skeleton(&mut slot_ui, "slot", loaded, seen.attach_started, Skeleton::Block(slot.height()), |ui| {
        // 接続中から抜けた時は with_skeleton が骨格から cross-fade する。他の替わり目はここで前の状態から寄せる
        let fading = seen.prev != seen.state && seen.prev != PortalState::Attaching;
        let p = if fading { motion::ease_out(motion::progress(&ctx, seen.changed_at, motion::dur(&ctx, motion::PAGE))) } else { 1.0 };
        let base = ui.opacity();
        let area = ui.available_rect_before_wrap();
        ui.multiply_opacity(p);
        if frame.state.has_visual() {
            body(ui);
        } else {
            fact(ui, area, frame, color);
        }
        if p < 1.0 && !seen.prev.has_visual() {
            ghost(ui, area, seen.prev, base * (1.0 - p));
        }
    });
    PortalResponse { slot, action }
}

/// RQ6（RQ-024）: 帯の今の活動の 1 行（`Typing in {app}` / `Shibubu is …` / `Waiting for you to finish typing in {app}`）の置き場。app.rs が毎 frame 入れる（None で消す）。
/// 帯の他の要素（題・状態語・次の一手・`Open original`）は変えず、言う事がある間だけ押せない静的な文が 1 つ（AX 名 `Activity`）増える。
/// Wave: RQ6
pub fn set_activity(ctx: &egui::Context, line: Option<String>) {
    let id = Id::new("kangae.portal.activity");
    ctx.data_mut(|d| match line {
        Some(l) => d.insert_temp(id, l),
        None => {
            d.remove::<String>(id);
        }
    });
}

/// 帯: 右端に次の一手 1 つ、残りの幅に 1 行（選べる label = cua の elements に出る）。
fn strip_line(ui: &mut Ui, id: Id, strip: Rect, frame: &PortalFrame, color: Color32) -> Option<PortalAction> {
    let t = theme::tokens(ui.ctx());
    let inner = Rect::from_min_max(pos2(strip.left() + theme::SP_3, strip.top()), pos2(strip.right() - theme::SP_1, strip.bottom() - theme::HAIRLINE));
    // status_line と同じ並び（状態語は状態の色・app 名は primary・mode と sync は secondary・detail は tertiary）。ICONS-1101 追記 238: 頭の区間は印の席
    // （状態の色・印の無い状態は 0）・印は帯の左端の 1 行目の中央に描く（Label は帯の左端から置く = `line_with_action` と下の row の左）
    let format = states::helper_format;
    let (region, size) = (frame.state.region(), Type::Helper.size());
    let mut job = egui::text::LayoutJob::default();
    job.append("", states::mark_room(region, size), format(color));
    states::paint_mark(ui.painter(), region, pos2(inner.left() + size / 2.0, inner.center().y), size, color);
    job.append(frame.app_name, 0.0, format(t.text_primary));
    if !frame.state.wording().is_empty() {
        job.append(&format!(" · {}", frame.state.wording()), 0.0, format(color));
    }
    if let Some(m) = frame.meta() {
        job.append(&format!(" · {m}"), 0.0, format(t.text_secondary));
    }
    let detail = frame.strip_detail();
    if !detail.is_empty() {
        job.append(&format!(" · {detail}"), 0.0, format(t.text_tertiary));
    }
    let step = frame.state.next_step();
    let activity = ui.ctx().data(|d| d.get_temp::<String>(Id::new("kangae.portal.activity")));
    let alt = frame.state.alt_step();
    if alt.is_none() && activity.is_none() {
        return states::line_with_action(ui, id.with("kangae.portal.strip"), inner, job, step.label(), step.variant()).then_some(step);
    }
    // RQ4（RQ-018）: 次の一手の左に元の app へ行く 2 つ目の手（Ghost・許可を拒んだ面でも押せる = `alt_step`）。並びは line_with_action と同じ
    let mut row = ui.new_child(UiBuilder::new().id_salt(id.with("kangae.portal.strip")).max_rect(inner).layout(Layout::right_to_left(Align::Center)));
    let mut out = theme::button_compact(&mut row, step.label(), step.variant()).clicked().then_some(step);
    row.add_space(theme::SP_2);
    if let Some(alt) = alt {
        if theme::button_compact(&mut row, alt.label(), Variant::Ghost).clicked() {
            out = Some(alt);
        }
        row.add_space(theme::SP_2);
    }
    // RQ6: 今の活動（押せない静的な文・ボタンの左・他の要素の AX 名と値は変えない）
    if let Some(line) = activity {
        theme::ax_name(row.add(egui::Label::new(Type::Helper.text(line.as_str()).color(t.text_primary)).truncate()), "Activity");
        row.add_space(theme::SP_3);
    }
    row.add_space(theme::SP_3);
    let rest = row.available_rect_before_wrap();
    row.new_child(UiBuilder::new().max_rect(rest).layout(Layout::left_to_right(Align::Center))).add(egui::Label::new(job).selectable(true).truncate());
    out
}

/// 映像の無い状態の slot: 中央に glyph + 状態語（見出し）と detail 1 行。偽の画面・架空の data で埋めない。
fn fact(ui: &mut Ui, area: Rect, frame: &PortalFrame, color: Color32) {
    let t = theme::tokens(ui.ctx());
    let detail = frame.detail.trim();
    let top = fact_top(area, !detail.is_empty());
    ui.add_space((top - area.top()).max(0.0));
    ui.vertical_centered(|ui| {
        let format = |color: Color32| egui::text::TextFormat { font_id: Type::HeadingSm.font(), color, line_height: Some(Type::HeadingSm.line_height()), ..Default::default() };
        let mut job = egui::text::LayoutJob::default();
        job.append(frame.state.wording(), 0.0, format(t.text_primary));
        // ICONS-1101 追記 238: 印は見出しの頭（状態の色・印の無い状態は語だけ）
        states::marked(ui, frame.state.region(), job, color, |l| l.selectable(true));
        if !detail.is_empty() {
            ui.add_space(theme::SP_1);
            theme::label(ui, Type::Helper, detail, t.text_secondary);
        }
    });
}

/// 見出しの行の上端（見出し + detail の塊を slot の縦の中央に）。
fn fact_top(area: Rect, has_detail: bool) -> f32 {
    let h = Type::HeadingSm.line_height() + if has_detail { theme::SP_1 + Type::Helper.line_height() } else { 0.0 };
    area.center().y - h / 2.0
}

/// 替わる前の状態の見出しを painter だけで薄く重ねる（widget にしない = AX と focus に出ない）。
fn ghost(ui: &Ui, area: Rect, prev: PortalState, opacity: f32) {
    if opacity <= 0.0 || prev == PortalState::Attaching {
        return;
    }
    let t = theme::tokens(ui.ctx());
    let mut painter = ui.painter_at(area);
    painter.set_opacity(opacity);
    let y = fact_top(area, false) + Type::HeadingSm.line_height() / 2.0;
    // ICONS-1101 追記 238: `fact` と同じ並び（印の席 + 語を真ん中に）
    let (region, size) = (prev.region(), Type::HeadingSm.size());
    let galley = painter.layout_no_wrap(prev.wording().to_owned(), Type::HeadingSm.font(), t.text_primary);
    let room = states::mark_room(region, size);
    let left = area.center().x - (room + galley.size().x) / 2.0;
    states::paint_mark(&painter, region, pos2(left + size / 2.0, y), size, region.color(&t));
    painter.galley(pos2(left + room, y - galley.size().y / 2.0), galley, t.text_primary);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(state: PortalState, sync: SyncMode, mode: Option<Mode>) -> PortalFrame<'static> {
        PortalFrame { app_name: "REAPER", state, sync, mode, detail: "" }
    }

    #[test]
    fn sync_labels_are_the_v14_table() {
        let labels: Vec<_> = SyncMode::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels, ["Live view", "Live state", "Checked periodically", "Guessed", "May be out of date", "Can't read its state"]);
    }

    #[test]
    fn delayed_and_ended_never_show_live() {
        for sync in SyncMode::ALL {
            for s in [PortalState::SyncDelayed, PortalState::SourceUnavailable] {
                let shown = frame(s, sync, None).shown_sync().expect("同期の確度を出す");
                assert!(!shown.is_live(), "{s:?} × {sync:?} → {shown:?}");
            }
        }
        for s in [PortalState::Undiscovered, PortalState::PermissionRequired, PortalState::Attaching, PortalState::IntegrationFailed] {
            assert_eq!(frame(s, SyncMode::VisualLive, None).shown_sync(), None, "{s:?}");
        }
    }

    #[test]
    fn every_state_has_its_own_wording() {
        let mut seen = std::collections::HashSet::new();
        for s in PortalState::ALL {
            assert!(seen.insert(s.wording()), "{s:?}");
        }
    }

    /// RQ4 interface（caller の形・本体の前は赤）: 許可を拒まれた面にも元の app への道が在り（`Open settings` と `Open original`）、2 つ目の手は次の一手と重ならない。
    #[test]
    fn rq4_every_state_but_not_found_keeps_a_way_to_the_original_app() {
        assert_eq!(PortalState::PermissionRequired.alt_step(), Some(PortalAction::ContinueInSource));
        assert_eq!(PortalState::Undiscovered.alt_step(), None);
        for s in PortalState::ALL {
            let opens = matches!(s.next_step(), PortalAction::OpenSourceWindow | PortalAction::ContinueInSource);
            assert_eq!(s.alt_step().is_some(), !opens && s != PortalState::Undiscovered, "{s:?}");
            assert_ne!(s.alt_step(), Some(s.next_step()), "{s:?}");
        }
        assert_eq!(PortalAction::ContinueInSource.label(), "Open original");
    }

    #[test]
    fn status_line_names_app_state_and_sync() {
        let f = PortalFrame { detail: "Clicks and keys", ..frame(PortalState::Mirror, SyncMode::VisualLive, Some(Mode::External)) };
        assert_eq!(f.status_line(), "REAPER · Mirroring · Live view · Clicks and keys");
        let f = PortalFrame { detail: "Not running", ..frame(PortalState::Undiscovered, SyncMode::Unsupported, None) };
        assert_eq!(f.status_line(), "REAPER · Not found");
    }
}
