//! DESIGN.md §11 の region 状態（UI-007）。文言と描き方を 1 か所に。renderer（unit R）もここを使う。
//! 色だけで状態を示さない（§2.4「shape と text を必ず付ける」= 語は全部の状態に在る）。状態の色が替わる時は cross-fade（§15.2-1）。
//! ICONS-1101 追記 238（owner「empty,partial,stale disconnected, blocked,はアイコンいらない。errorは赤、readyは緑runnningはloadingのように丸の中を回る定番のやつに回る演出。loadingも。verifyも緑。」）:
//! 印は `RegionState::icon` の 3 つだけ（Error と Ready の丸・Verified の check・Running と Loading の回る印）を `paint_mark` が描く（文字の記号にしない）。

use super::motion;
use crate::domain::icon::Icon;
use super::theme::{self, Tokens, Type, Variant};
use eframe::egui::{self, Color32};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionState {
    Loading,
    Empty,
    Partial,
    Stale,
    Disconnected,
    Blocked,
    Error,
    Ready,
    Running,
    Verified,
}

impl RegionState {
    pub const ALL: [RegionState; 10] = [
        RegionState::Loading,
        RegionState::Empty,
        RegionState::Partial,
        RegionState::Stale,
        RegionState::Disconnected,
        RegionState::Blocked,
        RegionState::Error,
        RegionState::Ready,
        RegionState::Running,
        RegionState::Verified,
    ];

    /// §11 の文言。曖昧な「完了」「成功」は使わない（検証済みだけが `verified`）。
    /// 契約（UI-1 から・型と署名は変えない）: 返すのは `docs/ui-copy/copy.en.md` の「Region states」の EN から glyph を除いた語
    /// （Loading・Nothing here yet・Partly shown・May be out of date・Not connected・Needs approval・Failed・Ready・Running・Verified）。
    /// Blocked（▣ Needs approval）は承認・許可の button が同じ行に在る時だけ使う（UI-1 反証: 確かめていない面は Ready・disk と違う窓は Stale）。
    /// 呼ぶ側: 文の 1 行は語だけ（印は `marked` / `paint_mark` が頭に描く・ICONS-1101）。保証する: 全 variant で英語・sentence case・句点なし・8 語以下・日本語 0。
    /// 保証しない: 何を読んでいるかの語（`Loading {thing}` の {thing} は呼ぶ側が足す）・Ready を描かない事（UI-6）・ja の locale。
    /// Wave: UI-1
    pub const fn wording(self) -> &'static str {
        match self {
            RegionState::Loading => "Loading",
            RegionState::Empty => "Nothing here yet",
            RegionState::Partial => "Partly shown",
            RegionState::Stale => "May be out of date",
            RegionState::Disconnected => "Not connected",
            RegionState::Blocked => "Needs approval",
            RegionState::Error => "Failed",
            RegionState::Ready => "Ready",
            RegionState::Running => "Running",
            RegionState::Verified => "Verified",
        }
    }

    /// ICONS-1101 追記 238: 状態の印（色は `color`）。Empty・Partial・Stale・Disconnected・Blocked は None（語だけ）・Error と Ready = `Circle`（赤 / 緑）・
    /// Verified = `Check`（緑）・Running と Loading = `Loader`（`paint_mark` が回す）。描くのは `paint_mark` か `marked`（文字の記号にしない）。
    /// Wave: ICONS-1101
    pub const fn icon(self) -> Option<Icon> {
        match self {
            RegionState::Error | RegionState::Ready => Some(Icon::Circle),
            RegionState::Verified => Some(Icon::Check),
            RegionState::Running | RegionState::Loading => Some(Icon::Loader),
            RegionState::Empty | RegionState::Partial | RegionState::Stale | RegionState::Disconnected | RegionState::Blocked => None,
        }
    }

    /// 238: 赤 = Error・緑 = Ready と Verified・ほか（印の無い 5 つ・回る印の 2 つ）は `text_secondary`（動いているは青でなく回る印で出す = UI-3 の「青は動いている状態だけ」を置き換えた）。
    pub fn color(self, t: &Tokens) -> Color32 {
        match self {
            RegionState::Error => t.danger,
            RegionState::Ready | RegionState::Verified => t.success,
            RegionState::Loading | RegionState::Empty | RegionState::Partial | RegionState::Stale | RegionState::Disconnected | RegionState::Blocked | RegionState::Running => t.text_secondary,
        }
    }
}

/// ICONS-1101 追記 238: 回る印の 1 回転の秒（定番の spinner・0.8〜1 秒）。
const SPIN_S: f64 = 0.9;

/// 238: 状態の印（`RegionState::icon`）を `center` に `size` で `color` に描く（印の無い状態は何もしない）。Running・Loading は回す（回っている間だけ次の frame を頼む・
/// 動きを減らす設定〈`motion::reduce_motion`〉では止めて描く）。回す角度を渡すのは `theme::paint_icon` の 1 か所。
/// 例: `states::paint_mark(ui.painter(), RegionState::Running, at.center(), Type::Small.size(), t.text_secondary)`
/// Wave: ICONS-1101
pub fn paint_mark(painter: &egui::Painter, state: RegionState, center: egui::Pos2, size: f32, color: Color32) {
    let Some(icon) = state.icon() else { return };
    let ctx = painter.ctx();
    let angle = if icon == Icon::Loader && !motion::reduce_motion(ctx) {
        ctx.request_repaint();
        (ctx.input(|i| i.time) / SPIN_S).fract() as f32 * std::f32::consts::TAU
    } else {
        0.0
    };
    theme::paint_icon(painter, icon, center, size, color, angle);
}

/// 238: 文の頭の印の席の幅（印の大きさ `size` と隙間 `SP_1`・印の無い状態は 0）。`marked` と、印を自分で置く行（Runs の結果の語・Portal の帯・生成の 1 行）が使う。
pub fn mark_room(state: RegionState, size: f32) -> f32 {
    if state.icon().is_some() {
        size + theme::SP_1
    } else {
        0.0
    }
}

/// 238: `job` の頭に印の席（`mark_room`）を空けた Label を置き、その席に印を描く（大きさ = 頭の区間の字の大きさ・縦は 1 行目の中央）。
/// `label` で Label の形（`truncate` / `wrap` / `selectable`）を決める。返すのは Label の Response（AX の名前と値は語だけ）。
/// 例: `states::marked(ui, RegionState::Error, job, color, |l| l.truncate())`
/// Wave: ICONS-1101
pub fn marked(ui: &mut egui::Ui, state: RegionState, mut job: egui::text::LayoutJob, color: Color32, label: impl FnOnce(egui::Label) -> egui::Label) -> egui::Response {
    let font = job.sections.first().map_or_else(|| Type::Helper.font(), |s| s.format.font_id.clone());
    let line_h = job.sections.first().and_then(|s| s.format.line_height).unwrap_or_else(|| ui.fonts(|f| f.row_height(&font)));
    let size = font.size;
    if let Some(s) = job.sections.first_mut() {
        s.leading_space += mark_room(state, size);
    }
    let r = ui.add(label(egui::Label::new(job)));
    if ui.is_rect_visible(r.rect) {
        let y = r.rect.top() + line_h.min(r.rect.height()) / 2.0;
        paint_mark(ui.painter(), state, egui::pos2(r.rect.left() + size / 2.0, y), size, color);
    }
    r
}

/// 1 行の状態表示（印 + 文言 + 補足・印は ICONS-1101 追記 238 の 3 つの形だけ）。行の末尾・表の列・strip で使う。
/// 1 つの Label に 2 色で組む（親が right_to_left でも語順が崩れない）。
/// 契約（UI-6a・`state_line` が持つ = owner の問題 3・SPEC §8 / §14 Q8「Ready は何も出さない」）:
/// - `Ready` は記号と語（`● Ready`）を描かない。`detail` が空なら何も描かず AX の node も作らない（`if state == RegionState::Ready && detail.is_empty() { return; }`）。
///   `detail` が在れば事実の 1 行だけ（記号・語・` · ` 無し・Helper の `text_secondary`）= 拒否の理由（`page_line` の RunTests・③ C2 の文の拒否）を消さない（L2・D-e）
/// - 同じ `(state, detail)` は 1 pass に 1 回: `ctx.cumulative_pass_nr()` と組にした記録を ctx の temp data に置き、同じ pass の 2 回目は何も描かない（AX の node も 1 つ）。
///   別の pass（次の frame・`request_discard` の描き直し）では描く。数えるのは Page の上に積む状態の行（`page_line`・`Host::line`・使える検査の席・Home の 1 行）だけで、
///   `each_its_own` の中（renderer の node の中身 = 欄・button・行ごとの事実）は数えない（反証の直し: Git の欄の branch と SHA が同じ文で、2 つ目の欄が空に見えた）
/// 呼ぶ側の例: `states::inline(ui, RegionState::Error, &line)`（同じ文を 2 か所から呼んでも画面には 1 行）。
/// 保証しない: 違う文の重複（`Terminal` の席が 2 つで文が違えば 2 行）・`region()` の大きな空の見出し（記録の外）・題の行への集約（UI-6b）。
/// Wave: UI-6a
pub fn inline(ui: &mut egui::Ui, state: RegionState, detail: &str) {
    state_line(ui, state, detail, false);
}

/// `inline` と同じ 1 行を、幅が足りなければ切らずに折り返す。次の一手が文の後ろに在る単独の行（AI の状態の 1 行）に使う（D-e: 切ると次の一手が見えない）。
/// 契約（UI-6a）: Ready と 1 pass に 1 回の規則は `inline` と同じ（同じ `state_line` を通る・記録は折り返しの有無を問わず `(state, detail)` で 1 つ）。
/// Wave: UI-6a
pub fn inline_wrap(ui: &mut egui::Ui, state: RegionState, detail: &str) {
    state_line(ui, state, detail, true);
}

fn own_key() -> egui::Id {
    egui::Id::new("kangae.state.own")
}

/// G88a（h_primary_one・operator 10-27 の決定）: 面の次の一手の button の形。1 pass（= 1 画面）で最初に描いた次の一手だけ `Primary`、2 つ目から `Ghost`
/// （名前と AX は同じ・形だけ）。読む順 = 描く順（区画の main → side → bottom、同じ区画の中は上から）。app 別の分岐は無い。
/// 例: `theme::button_compact(ui, "Mirror here", states::next_step_variant(ui))`
/// 保証する: 同じ pass で `Primary` を返すのは 1 回だけ。保証しない: 次の一手でない primary（header の `Create`）を数える事。
/// Wave: M2
pub fn next_step_variant(ui: &egui::Ui) -> theme::Variant {
    let pass = ui.ctx().cumulative_pass_nr();
    let id = egui::Id::new("kangae.next_step");
    let first = ui.ctx().data_mut(|d| d.get_temp::<u64>(id) != Some(pass) && { d.insert_temp(id, pass); true });
    if first { theme::Variant::Primary } else { theme::Variant::Ghost }
}

/// UI-6a（反証の直し）: `add` の中で描く状態の 1 行を「1 pass に 1 回」に数えない（同じ文でも別の欄・別の button・別の行の事実）。
/// renderer の `draw_node` が node ごとに 1 回通す（入れ子でよい・抜ける時に前の値へ戻す）。
/// 例: `states::each_its_own(ui, |ui| components::draw(ui, node, cx))`。保証しない: `add` の中の同じ文の重なりを消す事（node の中は呼ぶ側が 1 回だけ描く）。
/// Wave: UI-6a
pub fn each_its_own<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let before = ctx.data_mut(|d| std::mem::replace(d.get_temp_mut_or_default::<bool>(own_key()), true));
    let r = add(ui);
    ctx.data_mut(|d| d.insert_temp(own_key(), before));
    r
}

fn state_line(ui: &mut egui::Ui, state: RegionState, detail: &str, wrap: bool) {
    // Ready は記号と語を出さない・言う事が無ければ何も描かない（AX の node も作らない）
    if state == RegionState::Ready && detail.is_empty() {
        return;
    }
    // 同じ (state, detail) は 1 pass に 1 回（呼ぶ側が重ねても画面と AX に 1 行）。記録は pass が替われば空にする（ctx に 1 つ・溜めない）。
    // node の中身（each_its_own の中）は数えない
    let ctx = ui.ctx().clone();
    let pass = ctx.cumulative_pass_nr();
    let key = egui::Id::new((state, detail));
    let own = ctx.data(|d| d.get_temp::<bool>(own_key())).unwrap_or(false);
    let first = own || ctx.data_mut(|d| {
        let (at, seen) = d.get_temp_mut_or_default::<(u64, Vec<egui::Id>)>(egui::Id::new("kangae.state.pass"));
        if *at != pass {
            (*at, *seen) = (pass, Vec::new());
        }
        let first = !seen.contains(&key);
        if first {
            seen.push(key);
        }
        first
    });
    if !first {
        return;
    }
    let t = theme::tokens(ui.ctx());
    let mut job = egui::text::LayoutJob::default();
    if state == RegionState::Ready {
        // 事実の 1 行だけ（拒否の理由を消さない = L2・D-e）
        job.append(detail, 0.0, helper_format(t.text_secondary));
        let label = egui::Label::new(job);
        ui.add(if wrap { label.wrap() } else { label.truncate() });
        return;
    }
    // id は次に置く Label の位置の id（同じ場所の状態が替わった時だけ寄せる）
    let color = motion::cross_fade(ui.ctx(), ui.next_auto_id().with("kangae.state"), state.color(&t));
    // ICONS-1101 追記 238: 印は頭に描く（印の無い状態は語だけ）
    job.append(state.wording(), 0.0, helper_format(color));
    if !detail.is_empty() {
        job.append(&format!(" · {detail}"), 0.0, helper_format(t.text_tertiary));
    }
    marked(ui, state, job, color, |l| if wrap { l.wrap() } else { l.truncate() });
}

/// 日本語（かな・漢字）を含むか。日本語を含まない断片は id・内部の語とみなして画面から落とす時に。
pub fn has_ja(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}'))
}

/// 1 文目だけ（「。」「！」は落とし、問いの「？」は残す）・末尾の「（ascii の id）」を落とす。
pub fn first_sentence(s: &str) -> &str {
    let s = match s.char_indices().find(|(_, c)| "。？?！!".contains(*c)) {
        Some((i, c @ ('？' | '?'))) => &s[..i + c.len_utf8()],
        Some((i, _)) => &s[..i],
        None => s,
    }
    .trim();
    match s.rfind('（') {
        Some(i) if s.ends_with('）') && !has_ja(&s[i..]) => s[..i].trim_end(),
        _ => s,
    }
}

/// 補助の文字（Helper 12/18）の 1 区間の書式。色を変えて 1 つの LayoutJob に並べる時に。
pub fn helper_format(color: Color32) -> egui::text::TextFormat {
    egui::text::TextFormat { font_id: Type::Helper.font(), color, line_height: Some(Type::Helper.line_height()), ..Default::default() }
}

/// 状態の 1 行と次の一手 1 つ（右端の compact button）を `rect` に描く。押された frame で true。
/// 1 行は選べる label（cua の elements に名前で出る）で、幅が足りなければ切り詰める。Portal の枠の帯と生成中の画面の上端が使う。
pub fn line_with_action(ui: &mut egui::Ui, id: egui::Id, rect: egui::Rect, job: egui::text::LayoutJob, action: &str, variant: Variant) -> bool {
    let mut row = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let clicked = theme::button_compact(&mut row, action, variant).clicked();
    row.add_space(theme::SP_3);
    let rest = row.available_rect_before_wrap();
    let mut line = row.new_child(egui::UiBuilder::new().max_rect(rest).layout(egui::Layout::left_to_right(egui::Align::Center)));
    line.add(egui::Label::new(job).selectable(true).truncate());
    clicked
}

/// region 全体の状態（§1.1 empty state: 見出し 1 行 + 説明 1 行 + CTA 最大 1 つ）。
/// 戻り値は CTA が押されたか。KPI も card も置かない。
pub fn region(ui: &mut egui::Ui, state: RegionState, detail: &str, cta: Option<&str>) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut clicked = false;
    ui.vertical(|ui| {
        ui.add_space(theme::SP_6);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::SP_2;
            let color = motion::cross_fade(ui.ctx(), ui.next_auto_id().with("kangae.state"), state.color(&t));
            // ICONS-1101 追記 238: 印は見出しの字の大きさ（印の無い状態は語だけ）
            if state.icon().is_some() {
                let size = Type::HeadingSm.size();
                let (_, at) = ui.allocate_space(egui::Vec2::splat(size));
                paint_mark(ui.painter(), state, at.center(), size, color);
            }
            theme::heading(ui, Type::HeadingSm, state.wording(), t.text_primary);
        });
        if !detail.is_empty() {
            ui.add_space(theme::SP_1);
            theme::label(ui, Type::Helper, detail, t.text_secondary);
        }
        if let Some(label) = cta {
            ui.add_space(theme::SP_3);
            let variant = match state {
                RegionState::Error => Variant::Ghost,
                _ => Variant::Primary,
            };
            if theme::button(ui, label, variant).clicked() {
                clicked = true;
            }
        }
        ui.add_space(theme::SP_6);
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_state_has_distinct_unambiguous_wording() {
        let mut seen = std::collections::HashSet::new();
        for s in RegionState::ALL {
            let w = s.wording();
            assert!(!w.is_empty() && !w.contains("完了") || s == RegionState::Ready, "{s:?}: {w}");
            assert!(!w.contains("成功"), "{s:?}: {w}");
            assert!(seen.insert(w), "duplicate wording {w}");
        }
    }
    /// 1 pass ずつ描いた文字（pass ごとの Vec）。動きは試験の即時（OS を読まない）。
    fn passes(draws: &[&dyn Fn(&mut egui::Ui)]) -> Vec<Vec<String>> {
        fn walk(s: &egui::epaint::Shape, acc: &mut Vec<String>) {
            match s {
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
                egui::epaint::Shape::Text(t) => acc.push(t.galley.text().to_owned()),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        motion::set_motion_mode(&ctx, motion::MotionMode::Cut);
        draws
            .iter()
            .map(|draw| {
                let out = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| draw(ui));
                });
                let mut acc = Vec::new();
                out.shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
                acc
            })
            .collect()
    }

    /// UI-6a（owner の問題 3・SPEC §8 / §14 Q8）: Ready は記号と語を描かず、言う事が無ければ何も描かない・理由の文は残す。
    /// 同じ (state, 文) は 1 pass に 1 回（`inline` と `inline_wrap` を混ぜても）・違う文は描く・次の pass ではまた描く。
    /// node の中身（`each_its_own` の中・入れ子も）は数えない。
    /// 負の対照（commit message）: `state_line` の Ready の早い抜けを外す → 1 本目が赤・pass の記録を外す → 3 本目が赤・`own ||` を外す → 5 本目が赤。
    #[test]
    fn ready_is_silent_and_the_same_line_is_drawn_once_per_pass() {
        let shown = passes(&[
            &|ui| inline(ui, RegionState::Ready, ""),
            &|ui| inline(ui, RegionState::Ready, "Run tests from the Tests page"),
            &|ui| {
                inline(ui, RegionState::Error, "x");
                inline_wrap(ui, RegionState::Error, "x");
                inline(ui, RegionState::Disconnected, "x");
                inline(ui, RegionState::Error, "y");
            },
            &|ui| inline(ui, RegionState::Error, "x"),
            &|ui| {
                each_its_own(ui, |ui| inline(ui, RegionState::Error, "Not read yet"));
                each_its_own(ui, |ui| each_its_own(ui, |ui| inline(ui, RegionState::Error, "Not read yet")));
                inline(ui, RegionState::Error, "Not read yet");
                inline(ui, RegionState::Error, "Not read yet");
            },
        ]);
        assert!(shown[0].is_empty(), "Ready を描いた: {:?}", shown[0]);
        assert_eq!(shown[1], ["Run tests from the Tests page"], "Ready の理由の文は記号も語も無しで残す");
        // ICONS-1101 追記 238: 語の後に印（Failed = 丸）を描く・Not connected は印が無い
        let o = theme::icon_glyph(Icon::Circle);
        assert_eq!(shown[2], ["Failed · x", o, "Not connected · x", "Failed · y", o], "同じ 1 行を 1 pass に 2 回描いた・印の有る無しが 238 と違う");
        assert_eq!(shown[3], ["Failed · x", o], "次の pass で描かない");
        assert_eq!(shown[4], ["Failed · Not read yet", o, "Failed · Not read yet", o, "Failed · Not read yet", o], "別々の node の同じ文を消した（node の外の重なりは 1 回）");
    }
    /// ICONS-1101 追記 238: 緑は Ready と Verified・赤は Error・印は 3 つの状態の形だけ（印の無い 5 つ・回るのは Running と Loading）。
    /// 負の対照（未実行）: Blocked に印を戻す → 印の表で赤・Running を `working` の青に戻す → 色の表で赤
    #[test]
    fn icons1101_238_ready_and_verified_are_green_and_only_three_states_have_a_mark() {
        use RegionState as R;
        let t = theme::LIGHT;
        for s in RegionState::ALL {
            assert_eq!(s.color(&t) == t.success, matches!(s, R::Ready | R::Verified), "{s:?} の緑");
            assert_eq!(s.color(&t) == t.danger, s == R::Error, "{s:?} の赤");
            assert_ne!(s.color(&t), t.working, "{s:?} が青（動いているは回る印で出す）");
            let want = match s {
                R::Error | R::Ready => Some(Icon::Circle),
                R::Verified => Some(Icon::Check),
                R::Running | R::Loading => Some(Icon::Loader),
                _ => None,
            };
            assert_eq!(s.icon(), want, "{s:?} の印");
            assert_eq!(mark_room(s, 12.0) > 0.0, want.is_some(), "{s:?} の席");
        }
    }
}
