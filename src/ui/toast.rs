//! NOTIFY-1101（owner 185 通目の 1「toastでnotificationcardが出るようにするだけでいい…そのcardは右上に罰がありそこを押すと消えるように」）:
//! 知らせるだけで手の要らない事の右上の card。振り分け（spec の決まり）: ① 人の手が今要る物 = chat の中の card ② 知らせるだけ = ここ
//! ③ いつも同じ事しか言わない説明 = 出さない（icon・tab・形で示す）。
//! 追記 213（owner「むだなnotificationいらない。garageband showedとかいらない」）: ここに出すのは 3 種だけ = (a) 失敗した・思った事が起きなかった
//! (b) 利用者の手が要る（button 付き）(c) 利用者が始めた事が画面の外で済んだ。画面を見れば分かる事（面の状態の移り変わり・app を開いた・映った・
//! 押した手の成功）は出さない。
//! 積み方: 上の帯の下・右上から縦に・最大 `MAX` 枚（溢れたら一番古いのを落とす）・同じ文は 2 枚にしない。
//! 消え方: 失敗と手の button の在る card は × まで残る・済んだ知らせは `DONE_SECS` 秒。AX: card = role Alert（文は中の Label）・× = Button `Dismiss`。
//! × は card の右上の角に固定（追記 213「バツをcardの右上に」・文の長さと button の有無で動かない）= `card`。帯の状態の全文（shell.rs）も同じ `card`。
//! 置き場は egui の Context の temp data = Shell を持たない所（renderer・generated の Terminal の panel）からも `post` できる。描くのは app.rs の 1 か所（`draw`）。
//! 例: `toast::post(ctx, "Saved", false, None)`・`toast::post_once(ctx, "agent-setup", SETUP_INTRO, false, Some("Install"))`・
//! app.rs の update: `if let Some(a) = ui::toast::draw(ctx, ctx.available_rect().top()) { … }`
use super::theme::{self, Type, Variant};
use eframe::egui;

/// 済んだ知らせが消えるまでの秒。
pub const DONE_SECS: f64 = 6.0;
/// 同時に出す枚数。
pub const MAX: usize = 3;
/// card の幅。
const WIDTH: f32 = 320.0;

/// 右上の知らせ 1 枚。
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    /// 失敗（× まで残る・印は danger の色）
    pub error: bool,
    /// 押せる 1 つの button の語（押すと `draw` がこの語を返し、card は消える）。例: `Some("Install")`
    pub action: Option<&'static str>,
    /// 初めて描いた時刻（`ctx.input(|i| i.time)`）。None = まだ描いていない
    since: Option<f64>,
}

impl Toast {
    /// × まで残るか（失敗・手の button が在る）。
    fn stays(&self) -> bool {
        self.error || self.action.is_some()
    }
}

#[derive(Clone, Default)]
struct Store {
    shown: Vec<Toast>,
    /// `post_once` で出した key（この起動の間）
    once: Vec<String>,
}

fn store_id() -> egui::Id {
    egui::Id::new("kangae.toasts")
}

/// 1 枚積む（同じ文が出ていれば積まない・`MAX` を超えたら一番古いのを落とす）。
pub fn post(ctx: &egui::Context, text: impl Into<String>, error: bool, action: Option<&'static str>) {
    let text = text.into();
    ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_default::<Store>(store_id());
        if s.shown.iter().any(|t| t.text == text) {
            return;
        }
        s.shown.push(Toast { text, error, action, since: None });
        if s.shown.len() > MAX {
            s.shown.remove(0);
        }
    });
    ctx.request_repaint();
}

/// `key` ごとにこの起動の間 1 回だけ積む（毎 frame 呼んでよい・× で消した後も出し直さない）。
// ponytail: 起動の間 1 回（Project を開き直すたびには出さない）。開くたびに要るなら go の時に once から消す
pub fn post_once(ctx: &egui::Context, key: &str, text: impl Into<String>, error: bool, action: Option<&'static str>) {
    let first = ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_default::<Store>(store_id());
        let first = !s.once.iter().any(|k| k == key);
        if first {
            s.once.push(key.to_owned());
        }
        first
    });
    if first {
        post(ctx, text, error, action);
    }
}

/// 失敗の文か（app.rs の `message` は種類を持たないので語で決める）。
// ponytail: 語の表（couldn't・can't・failed・isn't・error・not ）。message に種類を持たせたら置き換える
pub fn sounds_failed(text: &str) -> bool {
    let low = text.to_lowercase();
    ["couldn't", "can't", "cannot", "failed", "isn't", "error", "not "].iter().any(|w| low.contains(w))
}

/// 浮く card 1 枚（右上の知らせ・帯の状態の全文が同じ形）。× は card の右上の角に固定し、中身は × の分を右に空けて上から折り返す
/// （前は × が文の横に並び、短い文・button の在る card では × が左へ寄った）。返り値 = (card の矩形, × が押された)。
/// 例: `let (_, closed) = toast::card(ui, 420.0, |ui| { ui.add(egui::Label::new(text).wrap()); });`
pub fn card(ui: &mut egui::Ui, width: f32, body: impl FnOnce(&mut egui::Ui)) -> (egui::Rect, bool) {
    let tok = theme::tokens(ui.ctx());
    let mut closed = false;
    let rect = egui::Frame::new()
        .fill(tok.surface_1)
        .stroke(theme::hairline(&tok))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_MD))
        .shadow(ui.style().visuals.popup_shadow)
        .inner_margin(theme::SP_3)
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * theme::SP_3);
            let inner = ui.max_rect();
            ui.allocate_ui_with_layout(egui::vec2(inner.width() - theme::BUTTON_H - theme::SP_2, 0.0), egui::Layout::top_down(egui::Align::Min), body);
            let corner = egui::Rect::from_min_size(egui::pos2(inner.right() - theme::BUTTON_H, inner.top()), egui::Vec2::splat(theme::BUTTON_H));
            closed = ui.put(corner, |ui: &mut egui::Ui| theme::icon_button(ui, crate::domain::icon::Icon::X, "Dismiss")).clicked();
        })
        .response
        .rect;
    (rect, closed)
}

/// 右上に縦に積んで描く（app.rs の update で上の帯の後に 1 回）。`top` = 上の帯の下端（card はその `SP_2` 下）。
/// 返り値 = この frame に押された手の語（押した card は消える）。× は card を消すだけ。
pub fn draw(ctx: &egui::Context, top: f32) -> Option<&'static str> {
    let now = ctx.input(|i| i.time);
    let mut s = ctx.data(|d| d.get_temp::<Store>(store_id())).unwrap_or_default();
    s.shown.retain(|t| t.stays() || t.since.is_none_or(|t0| now - t0 < DONE_SECS));
    for t in &mut s.shown {
        t.since.get_or_insert(now);
    }
    // 次に消える済んだ知らせの時に描き直す（idle の frame を増やさない）
    if let Some(left) = s.shown.iter().filter(|t| !t.stays()).filter_map(|t| t.since).map(|t0| DONE_SECS - (now - t0)).reduce(f64::min) {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left.max(0.0)));
    }
    let (mut gone, mut pressed) = (None, None);
    if !s.shown.is_empty() {
        let tok = theme::tokens(ctx);
        let x = ctx.screen_rect().right() - theme::SP_4 - WIDTH;
        egui::Area::new(egui::Id::new("kangae.toasts.area")).order(egui::Order::Foreground).fixed_pos(egui::pos2(x, top + theme::SP_2)).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = theme::SP_2;
            for (i, t) in s.shown.iter().enumerate() {
                let id = egui::Id::new(("kangae.toast", i));
                ctx.accesskit_node_builder(id, |n| n.set_role(egui::accesskit::Role::Alert));
                let (rect, closed) = ctx.with_accessibility_parent(id, || {
                    card(ui, WIDTH, |ui| {
                        let ink = if t.error { tok.danger } else { tok.text_primary };
                        ui.add(egui::Label::new(Type::Body.text(t.text.as_str()).color(ink)).wrap());
                        if let Some(a) = t.action {
                            if theme::button_compact(ui, a, Variant::Ghost).clicked() {
                                (pressed, gone) = (Some(a), Some(i));
                            }
                        }
                    })
                });
                if closed {
                    gone = Some(i);
                }
                ctx.accesskit_node_builder(id, |n| {
                    n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
                });
            }
        });
    }
    if let Some(i) = gone {
        s.shown.remove(i);
    }
    ctx.data_mut(|d| d.insert_temp(store_id(), s));
    pressed
}
