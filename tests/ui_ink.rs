//! G33（G28 の B1・B2・B6 = sendback 74・75・79）: theme の font で描いた字の ink の上下と、focus の輪が key の時だけ出る事。
//! 数は glyph の ink の箱（`glyph.pos` ＋ `uv_rect.offset` / `size`・tessellate と同じく 1 px に丸める）から。`native_pixels_per_point` = 2（retina・G28 の写真と同じ）。
//! macOS の system font（Hiragino・Helvetica Neue）を読むので macOS でだけ。`--nocapture` で前後の数を出す（report の数はこれ）。
//! ui は bin crate の module なので `#[path]` で丸ごと取り込む（ui_motion.rs と同じ）。
#![cfg(target_os = "macos")]
#![allow(dead_code)]
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/agent/mod.rs"]
mod agent;
#[allow(dead_code)]
#[path = "../src/surface/mod.rs"]
mod surface;
// shell の Intent が model::AiAccount を運ぶ。model は時刻だけ google を、google は crypto を引く（adapters と同じ 3 つ）
#[allow(dead_code)]
#[path = "../src/crypto.rs"]
mod crypto;
#[allow(dead_code)]
#[path = "../src/google/mod.rs"]
mod google;
#[allow(dead_code)]
#[path = "../src/model.rs"]
mod model;
#[path = "../src/ui/mod.rs"]
mod ui;

use eframe::egui::{self, epaint::ClippedShape, Color32, Context, Event, FullOutput, Galley, Key, Modifiers, Pos2, RawInput, Rect, Shape, ViewportId, ViewportInfo};
use domain::icon::Icon;
use ui::theme::{self, Type, Variant};

const SCREEN: Rect = Rect { min: Pos2::ZERO, max: Pos2::new(800.0, 600.0) };
const PPP: f32 = 2.0;

fn ctx() -> Context {
    let c = Context::default();
    theme::install(&c, egui::ThemePreference::Light);
    c
}

fn frame(c: &Context, events: Vec<Event>, mut draw: impl FnMut(&mut egui::Ui)) -> FullOutput {
    let mut input = RawInput { screen_rect: Some(SCREEN), events, ..Default::default() };
    input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(PPP), ..Default::default() });
    c.run(input, |c| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(c, |ui| draw(ui));
    })
}

/// 1 字の ink の上端と下端（画面の point・tessellate と同じく物理 px に丸める）。ink の無い字（空白）は None。
fn ink(pos: Pos2, galley: &Galley, want: impl Fn(char) -> bool) -> Option<(f32, f32)> {
    let round = |v: f32| (v * PPP).round() / PPP;
    let glyphs = galley.rows.iter().flat_map(|r| &r.glyphs).filter(|g| want(g.chr) && g.uv_rect.size.y > 0.0);
    glyphs.fold(None, |acc, g| {
        let top = round(pos.y) + round(g.pos.y + g.uv_rect.offset.y); // tessellate は text の pos と glyph の左上を別々に丸める
        let bottom = top + g.uv_rect.size.y;
        Some(acc.map_or((top, bottom), |(t, b): (f32, f32)| (t.min(top), b.max(bottom))))
    })
}

fn layout(c: &Context, text: &str, ty: Type) -> std::sync::Arc<Galley> {
    frame(c, vec![], |_| {}); // font を読み込ませる
    c.fonts(|f| f.layout_no_wrap(text.into(), ty.font(), Color32::BLACK))
}

/// B1: `Create` の primary の pill の中で、ink の上の余白と下の余白の差 ≤ 1pt（= 中央から ±1 retina px）。
/// G28 の写真（UI-3/01）は上 12・下 24 retina px（差 6pt）。根 = Proportional の先頭の Hiragino の行の箱（line gap 0.5em を含む 1.5em）の中央に置いた Latin が上に寄る。
#[test]
fn create_ink_is_centered_in_the_primary_pill() {
    let c = ctx();
    frame(&c, vec![], |ui| {
        theme::button(ui, "Create", Variant::Primary);
    });
    let out = frame(&c, vec![], |ui| {
        theme::button(ui, "Create", Variant::Primary);
    });
    let (mut pill, mut text) = (None, None);
    for ClippedShape { shape, .. } in &out.shapes {
        match shape {
            Shape::Rect(r) if r.fill == theme::LIGHT.text_primary => pill = Some(r.rect),
            Shape::Text(t) if t.galley.job.text == "Create" => text = ink(t.pos, &t.galley, |_| true),
            _ => {}
        }
    }
    let (pill, (top, bottom)) = (pill.expect("primary の塗り"), text.expect("Create の字"));
    let (above, below) = (top - pill.top(), pill.bottom() - bottom);
    eprintln!("B1 Create: pill {:.1}×{:.1}pt・ink の上 {:.0} / 下 {:.0} retina px（差 {:.0}）", pill.width(), pill.height(), above * PPP, below * PPP, (above - below) * PPP);
    assert!((above - below).abs() <= 1.0, "Create の ink が pill の中央に無い: 上 {above}pt・下 {below}pt");
}

/// B2: `Delete page…` の `…` が baseline の 3 点（ink の下端が `D` の下端 = baseline から 1pt 以内）。Hiragino の `…` は中央の三点リーダ（下端が 0.32em 上）。
#[test]
fn ellipsis_sits_on_the_baseline() {
    let c = ctx();
    let g = layout(&c, "Delete page…", Type::Body);
    let (_, dots) = ink(Pos2::ZERO, &g, |ch| ch == '…').expect("… の ink");
    let (_, base) = ink(Pos2::ZERO, &g, |ch| ch == 'D').expect("D の ink");
    eprintln!("B2 …: ink の下端が baseline から {:.0} retina px 上", (base - dots) * PPP);
    assert!((base - dots).abs() <= 1.0, "… が baseline に無い: 下端 {dots}・D の下端 {base}");
}

/// JP の利用者の data（Project の題・Page の名前）: 日本語が tofu にならず、CJK の ink の下端が同じ行の Latin の baseline から 1.5pt 以内
/// （Hiragino 自身の和欧の並び = 漢字 / 仮名の下端は baseline の 0.04〜0.07em 下）。本文と semibold（選んでいる tab・見出し）の 2 つ。
#[test]
fn japanese_shares_the_latin_baseline() {
    let c = ctx();
    let jp = |ch: char| ch > '\u{3000}';
    for ty in [Type::Body, Type::BodyStrong, Type::Title] {
        let text = "Page 日本語のページ";
        let g = layout(&c, text, ty);
        assert!(c.fonts(|f| f.has_glyphs(&ty.font(), text)), "{ty:?}: tofu");
        let (_, latin) = ink(Pos2::ZERO, &g, |ch| ch == 'P').expect("P の ink");
        let (_, cjk) = ink(Pos2::ZERO, &g, jp).expect("CJK の ink");
        eprintln!("JP {ty:?}: CJK の ink の下端が Latin の baseline から {:+.0} retina px（+ = 下）・幅 {:.1}pt", (cjk - latin) * PPP, g.size().x);
        assert!((cjk - latin).abs() <= 1.5, "{ty:?}: CJK の baseline が Latin とずれる: CJK の下端 {cjk}・P の下端 {latin}");
    }
}

/// 測るだけ（assert なし・report の数）: `icon_button` の icon（ICONS-1101 から Phosphor Bold・`paint_icon` が墨の中心を置く）の ink の中心と 28 四方の中心の差（+ = 下）。
#[test]
fn icon_glyph_offsets() {
    let c = ctx();
    for icon in [Icon::Ellipsis, Icon::Plus, Icon::ChevronLeft, Icon::X, Icon::Sparkles] {
        let glyph = theme::icon_glyph(icon);
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let draw = |ui: &mut egui::Ui| rect.set(theme::icon_button(ui, icon, "x").rect);
        frame(&c, vec![], draw);
        let out = frame(&c, vec![], draw);
        let text = out.shapes.iter().find_map(|s| match &s.shape {
            Shape::Text(t) if t.galley.job.text == glyph => ink(t.pos, &t.galley, |_| true),
            _ => None,
        });
        let (top, bottom) = text.expect("記号の ink");
        eprintln!("icon {glyph}: ink の中心が button の中心から {:+.0} retina px", ((top + bottom) / 2.0 - rect.get().center().y) * PPP);
    }
}

/// focus の輪（`FOCUS_RING_W` の `focus_ring` 色の線）の数。
fn rings(out: &FullOutput) -> usize {
    let t = theme::LIGHT;
    out.shapes.iter().filter(|s| matches!(&s.shape, Shape::Rect(r) if r.stroke.width == theme::FOCUS_RING_W && r.stroke.color == t.focus_ring)).count()
}

fn click_far() -> Vec<Event> {
    let pos = Pos2::new(SCREEN.right() - 4.0, SCREEN.bottom() - 4.0);
    [true, false].map(|pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }).into()
}

fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
    vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]
}

/// B6: focus の在る button に輪が出るのは、最後の入力が key（⌘ / Ctrl / Alt なし）の時だけ。mouse の後は出ない・⌘ の shortcut では替わらない。
#[test]
fn focus_ring_only_after_a_key() {
    let c = ctx();
    let draw = |ui: &mut egui::Ui| theme::button(ui, "Keep", Variant::Ghost).request_focus();
    frame(&c, vec![], draw);
    let mut seen = vec![];
    for (name, events) in [
        ("mouse", click_far()),
        ("A", key(Key::A, Modifiers::NONE)),
        ("⌘Z の後も key", key(Key::Z, Modifiers::COMMAND)),
        ("mouse に戻る", click_far()),
        ("⌘Z の後も mouse", key(Key::Z, Modifiers::COMMAND)),
    ] {
        frame(&c, events, draw);
        seen.push((name, rings(&frame(&c, vec![], draw))));
    }
    assert_eq!(seen, [("mouse", 0), ("A", 1), ("⌘Z の後も key", 1), ("mouse に戻る", 0), ("⌘Z の後も mouse", 0)]);
}

/// B6: mouse で開いた消す確認は `Keep` に focus を置く（Enter = 消さない・key の人の道）が、輪は出さない。key で動かすと出る。
#[test]
fn confirm_opened_by_mouse_focuses_keep_without_a_ring() {
    let c = ctx();
    let draw = |ui: &mut egui::Ui| {
        theme::confirm_inline(ui, "Delete “Notes”? ⌘Z brings it back.");
    };
    frame(&c, click_far(), draw);
    let out = frame(&c, vec![], draw);
    assert!(c.memory(|m| m.focused()).is_some(), "Keep に focus が無い");
    assert_eq!(rings(&out), 0, "mouse で開いた確認に輪");
    frame(&c, key(Key::A, Modifiers::NONE), draw);
    assert_eq!(rings(&frame(&c, vec![], draw)), 1, "key の後に輪が無い");
}
