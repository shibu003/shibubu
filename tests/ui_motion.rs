//! DESIGN.md §15.2-1・2 / §15.2-5 (b)(c) の test: src/ui/motion.rs と src/ui/skeleton.rs と、殻への当て方（View の切替・rail・起動直後）・
//! cua の AX 経路で探す名前（入力欄・header の欄・見出し）を egui の headless `Context::run` で。
//! ui は bin crate の module なので `#[path]` で丸ごと取り込む（lib target が無い）。時間は `RawInput::time` で進める。
#![allow(dead_code)]
// ui::agent_rail は agent を、ui::renderer と ui::connections は domain を crate root の sibling として参照する（w3/agent・w3/renderer・w3/adapters）。bin の main.rs と同じ並びで取り込む
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/agent/mod.rs"]
mod agent;
#[allow(dead_code)] // ui::portal_frame が PortalState / SyncMode を再輸出する（W2c）
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

use eframe::egui::{self, epaint::ClippedShape, Context, FullOutput, Id, Pos2, RawInput, Rect, Shape, Vec2};
use ui::motion::{self, MotionMode};
use ui::shell::{Tier, View};
use ui::skeleton::{self, Skeleton};

const SCREEN: Rect = Rect { min: Pos2::ZERO, max: Pos2::new(1440.0, 900.0) };

fn frame(ctx: &Context, t: f64, mut draw: impl FnMut(&mut egui::Ui)) -> FullOutput {
    let input = RawInput { screen_rect: Some(SCREEN), time: Some(t), ..Default::default() };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| draw(ui));
    })
}

/// 動きの段を明示した ctx（env と OS を読まない = test が機械の設定に左右されない）。true = 試験だけの即時（Cut）・false = Full。
fn ctx(reduce: bool) -> Context {
    let ctx = Context::default();
    motion::set_motion_mode(&ctx, if reduce { MotionMode::Cut } else { MotionMode::Full });
    ctx
}

/// Reduce Motion（OS の設定と同じ段 = `set_reduce_motion(true)`）の ctx。
fn fade_ctx() -> Context {
    let ctx = Context::default();
    motion::set_reduce_motion(&ctx, true);
    ctx
}

#[derive(Default, Debug)]
struct Count {
    text: usize,
    /// 色の付いた rect と円（骨格の帯）
    rect: usize,
    mesh: usize,
}

fn count(out: &FullOutput) -> Count {
    fn walk(s: &Shape, c: &mut Count) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, c)),
            Shape::Text(_) => c.text += 1,
            // CentralPanel の Frame::NONE も透明な rect を 1 つ置くので、色の付いた rect だけ数える
            Shape::Rect(r) if r.fill != egui::Color32::TRANSPARENT => c.rect += 1,
            Shape::Circle(r) if r.fill != egui::Color32::TRANSPARENT => c.rect += 1,
            Shape::Mesh(_) => c.mesh += 1,
            _ => {}
        }
    }
    let mut c = Count::default();
    out.shapes.iter().for_each(|ClippedShape { shape, .. }| walk(shape, &mut c));
    c
}

/// 試験だけの Cut は全部の時間が 0（1 frame で終点）。Full は定数のまま。
#[test]
fn cut_makes_every_duration_zero() {
    let on = ctx(true);
    for base in [motion::PAGE, motion::RAIL, motion::STAGGER, motion::HOVER, motion::TOAST, motion::NUMBER, motion::DIFF] {
        assert_eq!(motion::dur(&on, base), 0.0, "{base}");
    }
    assert_eq!(motion::stagger(&on, 7), 0.0);
    let off = ctx(false);
    assert_eq!(motion::dur(&off, motion::PAGE), 0.220);
    assert_eq!(motion::dur(&off, motion::NUMBER), 0.300);
}

/// UI-6a（owner-addendum ① = 答え ①）: Reduce Motion は 0 でなく短い fade。時間は `REDUCE_FADE`（120）まで詰め、それより短い hover は そのまま・
/// stagger は 0（並びの時間は動きではない）・`reduce_motion` は true・egui 自身の開閉と scroll も travel しない。
/// 負の対照（commit message）: `dur` の Fade の腕を `0.0` に戻す → 1 本目の assert が赤。
#[test]
fn reduce_motion_is_a_short_fade_not_zero() {
    let c = fade_ctx();
    assert_eq!(motion::motion_mode(&c), MotionMode::Fade);
    assert!(motion::reduce_motion(&c));
    for base in [motion::PAGE, motion::RAIL, motion::TOAST, motion::NUMBER, motion::DIFF] {
        assert_eq!(motion::dur(&c, base), motion::REDUCE_FADE, "{base}");
    }
    assert_eq!(motion::dur(&c, motion::HOVER), motion::HOVER, "100 は詰めない");
    assert!(motion::REDUCE_FADE > 0.0 && motion::REDUCE_FADE <= motion::PAGE);
    assert_eq!(motion::stagger(&c, 7), 0.0);
    assert_eq!(c.style().animation_time, 0.0, "egui の開閉が travel する");
    motion::set_reduce_motion(&c, false);
    assert_eq!(motion::motion_mode(&c), MotionMode::Full);
    assert!(c.style().animation_time > 0.0, "Full に戻した後も egui の開閉が即時");
}

/// UI-6a Done 9 の芯: Reduce Motion で `fade_slide_in` の中身は最初の frame から終わりの位置（y が動かない）で、不透明度だけが 0 → 1。
/// Full は今まで通り 8px 下から上がる。負の対照（commit message）: `slide_in` の段の分岐を外す（dy を常に `SLIDE_PX * (1 - p)`）→ Fade の y の assert が赤。
#[test]
fn reduce_motion_fades_without_moving() {
    let c = fade_ctx();
    let probe = |t: f64| {
        let (mut y, mut alpha) = (f32::NAN, f32::NAN);
        frame(&c, t, |ui| {
            let start = ui.cursor().top();
            motion::fade_slide_in(ui, "x", 0.0, |ui| (y, alpha) = (ui.cursor().top() - start, ui.opacity()));
        });
        (y, alpha)
    };
    let (y0, a0) = probe(0.0);
    let (y_mid, a_mid) = probe(0.06);
    let (y_end, a_end) = probe(1.0);
    assert_eq!((y0, y_mid, y_end), (0.0, 0.0, 0.0), "Reduce Motion で位置が動いた");
    assert!(a0 < 0.05 && a_mid > a0 && a_mid < 1.0 && a_end == 1.0, "不透明度が 0 → 1 に寄らない: {a0} {a_mid} {a_end}");
}

/// UI-6a: 位置と大きさの `travel` は Full だけ寄せ、Fade と Cut は最初の frame で目標。同じ間の `animate`（薄れる量）は Fade でも寄る。
#[test]
fn travel_jumps_under_reduce_motion_while_animate_fades() {
    let (id, fade) = (Id::new("w"), Id::new("w.fade"));
    for (c, moves) in [(ctx(false), true), (fade_ctx(), false), (ctx(true), false)] {
        let mode = motion::motion_mode(&c);
        frame(&c, 0.0, |ui| {
            motion::travel(ui.ctx(), id, 0.0, motion::RAIL);
            motion::animate(ui.ctx(), fade, 0.0, motion::RAIL);
        });
        let (mut w, mut a) = (0.0, 0.0);
        frame(&c, 0.05, |ui| (w, a) = (motion::travel(ui.ctx(), id, 232.0, motion::RAIL), motion::animate(ui.ctx(), fade, 1.0, motion::RAIL)));
        frame(&c, 0.1, |ui| (w, a) = (motion::travel(ui.ctx(), id, 232.0, motion::RAIL), motion::animate(ui.ctx(), fade, 1.0, motion::RAIL)));
        if moves {
            assert!(w > 0.0 && w < 232.0, "{mode:?}: 幅が寄らない {w}");
        } else {
            assert_eq!(w, 232.0, "{mode:?}: 幅が travel した");
        }
        let fades = mode == MotionMode::Fade;
        assert!(!fades || (a > 0.0 && a < 1.0), "{mode:?}: 薄れる量が寄らない {a}");
    }
}

/// UI-6a Done 9（holdout h2・反証の直し）: Reduce Motion を切った最初の frame で目標が替わっても、Fade の間の最後の値から滑る。
/// 替わらなければ動かない（Fade の前の古い from から滑らない）。負の対照（commit message）: `travel` の Fade の腕で記録を捨てる（前の形）→ 1 本目が赤。
#[test]
fn travel_slides_from_the_last_reduced_value_when_reduce_motion_is_turned_off() {
    let id = Id::new("w");
    let c = ctx(false);
    frame(&c, 0.0, |ui| _ = motion::travel(ui.ctx(), id, 232.0, motion::RAIL));
    motion::set_motion_mode(&c, MotionMode::Fade);
    frame(&c, 1.0, |ui| _ = motion::travel(ui.ctx(), id, 0.0, motion::RAIL));
    motion::set_motion_mode(&c, MotionMode::Full);
    let mut w = f32::NAN;
    frame(&c, 2.0, |ui| w = motion::travel(ui.ctx(), id, 100.0, motion::RAIL));
    frame(&c, 2.05, |ui| w = motion::travel(ui.ctx(), id, 100.0, motion::RAIL));
    assert!(w > 0.0 && w < 100.0, "Full に戻した frame の 0 → 100 が寄らない: {w}");
    // 替わらない目標は動かない（Fade の前の 232 から滑らない）
    let id = Id::new("w2");
    frame(&c, 3.0, |ui| _ = motion::travel(ui.ctx(), id, 232.0, motion::RAIL));
    motion::set_motion_mode(&c, MotionMode::Fade);
    frame(&c, 4.0, |ui| _ = motion::travel(ui.ctx(), id, 0.0, motion::RAIL));
    motion::set_motion_mode(&c, MotionMode::Full);
    frame(&c, 5.0, |ui| w = motion::travel(ui.ctx(), id, 0.0, motion::RAIL));
    assert_eq!(w, 0.0, "Fade の前の値から滑った");
}

/// UI-6a: 状態の色の cross-fade は `DIFF`（160）で終わる（前は `PAGE` 220）。
#[test]
fn cross_fade_settles_by_diff() {
    let c = ctx(false);
    let id = Id::new("diff");
    let (a, b) = (egui::Color32::from_rgb(0, 0, 0), egui::Color32::from_rgb(200, 100, 0));
    frame(&c, 0.0, |ui| {
        motion::cross_fade(ui.ctx(), id, a);
    });
    let mut got = a;
    frame(&c, 1.0, |ui| got = motion::cross_fade(ui.ctx(), id, b));
    frame(&c, 1.0 + f64::from(motion::DIFF) + 0.01, |ui| got = motion::cross_fade(ui.ctx(), id, b));
    assert_eq!(got, b, "160ms で終わらない");
}

#[test]
fn reduce_motion_number_jumps_and_does_not_repaint() {
    let c = ctx(true);
    let id = Id::new("n");
    frame(&c, 0.0, |ui| {
        motion::animated_number(ui.ctx(), id, 0.0);
    });
    let mut got = -1.0;
    let out = frame(&c, 0.01, |ui| got = motion::animated_number(ui.ctx(), id, 100.0));
    assert_eq!(got, 100.0);
    assert!(out.viewport_output.values().all(|v| v.repaint_delay > std::time::Duration::from_secs(1)), "reduce motion で repaint を頼んだ");
}

#[test]
fn env_overrides_os_setting() {
    assert!(motion::detect(Some("1"), || false));
    assert!(!motion::detect(Some("0"), || true));
    assert!(motion::detect(None, || true));
    assert!(!motion::detect(Some(""), || false));
}

#[test]
fn stagger_is_proportional_to_index() {
    let c = ctx(false);
    for i in 0..10 {
        assert!((motion::stagger(&c, i) - 0.030 * i as f32).abs() < 1e-6, "{i}");
    }
}

#[test]
fn animated_number_converges_and_stops_repainting() {
    let c = ctx(false);
    let id = Id::new("count");
    frame(&c, 0.0, |ui| {
        motion::animated_number(ui.ctx(), id, 0.0);
    });
    let mut seen = vec![];
    let mut last = FullOutput::default();
    // 1 回 request_repaint すると egui は次の frame も描くので、止まってから 2 frame 先まで回す
    for k in 1..=10 {
        last = frame(&c, k as f64 * 0.05, |ui| seen.push(motion::animated_number(ui.ctx(), id, 100.0)));
    }
    // 目標が変わった frame は前の値・次から 0 と 100 の間で単調に増え、300ms を過ぎたら 100 ちょうど
    assert_eq!(seen[0], 0.0, "{seen:?}");
    assert!(seen[1] > 0.0 && seen[1] < 100.0, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), 100.0);
    assert!(last.viewport_output.values().all(|v| v.repaint_delay > std::time::Duration::from_secs(1)), "止まった後も repaint を頼んでいる");
}

#[test]
fn cross_fade_moves_between_colors() {
    let c = ctx(false);
    let id = Id::new("state");
    let (a, b) = (egui::Color32::from_rgb(0, 0, 0), egui::Color32::from_rgb(200, 100, 0));
    frame(&c, 0.0, |ui| {
        motion::cross_fade(ui.ctx(), id, a);
    });
    let mut mid = a;
    frame(&c, 0.05, |ui| mid = motion::cross_fade(ui.ctx(), id, b));
    frame(&c, 0.10, |ui| mid = motion::cross_fade(ui.ctx(), id, b));
    assert!(mid != a && mid != b, "{mid:?}");
    let mut end = a;
    frame(&c, 1.0, |ui| end = motion::cross_fade(ui.ctx(), id, b));
    assert_eq!(end, b);
}

#[test]
fn fade_slide_in_starts_low_and_settles() {
    let c = ctx(false);
    let top = |t: f64| {
        let mut y = 0.0;
        frame(&c, t, |ui| {
            let start = ui.cursor().top();
            motion::fade_slide_in(ui, "x", 0.0, |ui| y = ui.cursor().top() - start);
        });
        y
    };
    assert!((top(0.0) - motion::SLIDE_PX).abs() < 1e-3);
    assert_eq!(top(1.0), 0.0);
}

fn assert_skeleton_only(out: &FullOutput, what: &str) {
    let c = count(out);
    assert_eq!(c.text, 0, "{what}: 骨格が text を描いた {c:?}");
    assert!(c.rect >= 1, "{what}: 帯が 1 本も無い {c:?}");
}

#[test]
fn every_skeleton_draws_bars_and_no_text() {
    let c = ctx(false);
    let out = frame(&c, 0.3, |ui| {
        skeleton::shell_skeleton(ui);
    });
    assert_skeleton_only(&out, "shell");
    assert!(count(&out).mesh >= 1, "shimmer の光が流れていない");
    for v in [View::Home, View::Runs, View::Connections, View::App("a".into()), View::CompatCanvas(1)] {
        let out = frame(&c, 0.3, |ui| skeleton::view_skeleton(ui, &v));
        assert_skeleton_only(&out, &format!("{v:?}"));
    }
    for id in ["list", "table", "detail", "chat", "media", "form", "timeline", "calendar", "editor", "chart", "unknown"] {
        let out = frame(&c, 0.3, |ui| skeleton::region_skeleton(ui, id));
        assert_skeleton_only(&out, id);
    }
    for s in [Skeleton::Line, Skeleton::Block(40.0), Skeleton::Circle(24.0), Skeleton::List(3), Skeleton::Table(3, 4), Skeleton::Card, Skeleton::Form(2)] {
        let out = frame(&c, 0.3, |ui| {
            s.show(ui);
        });
        assert_skeleton_only(&out, &format!("{s:?}"));
    }
}

/// 負の対照: 同じ数え方で本物の画面（Home）は text を数える = 「text 0 件」は数え損ないではない。
#[test]
fn negative_control_real_screen_has_text() {
    let c = ctx(false);
    ui::theme::install(&c, egui::ThemePreference::Light);
    let mut shell = ui::shell::Shell::new(View::Home);
    let out = frame(&c, 0.0, |ui| {
        ui::home::draw(ui, None, &[], &mut shell, None, None);
    });
    assert!(count(&out).text > 0, "{:?}", count(&out));
}

#[test]
fn dark_skeleton_uses_dark_tokens() {
    let c = ctx(false);
    c.set_theme(egui::Theme::Dark);
    let out = frame(&c, 0.3, |ui| {
        Skeleton::Block(40.0).show(ui);
    });
    let mut fills = vec![];
    for ClippedShape { shape, .. } in &out.shapes {
        if let Shape::Rect(r) = shape {
            fills.push(r.fill);
        }
    }
    assert!(fills.contains(&ui::theme::DARK.surface_2), "{fills:?}");
}

#[test]
fn reduce_motion_skeleton_is_static() {
    let c = ctx(true);
    // egui は最初の frame に自分で repaint を頼むので 1 frame 回してから測る
    frame(&c, 0.0, |ui| {
        skeleton::shell_skeleton(ui);
    });
    let out = frame(&c, 0.3, |ui| {
        skeleton::shell_skeleton(ui);
    });
    assert_eq!(count(&out).mesh, 0, "reduce motion で光が流れた");
    assert!(out.viewport_output.values().all(|v| v.repaint_delay > std::time::Duration::from_secs(1)));
}

/// 骨格を、先に置いた panel（header `HEADER_H`・幅 `side` の sidebar〈0 = 置かない = 畳んだ〉）の後で描く（UI-4b: 帯の右端は本物の SidePanel が残した左端）。
fn skeleton_after_panels(c: &Context, w: f32, side: f32) -> skeleton::ShellRects {
    let mut rects = None;
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, 800.0))), time: Some(0.0), ..Default::default() };
    let _ = c.run(input, |ctx| {
        egui::TopBottomPanel::top("h").exact_height(ui::theme::HEADER_H).frame(egui::Frame::NONE).show_separator_line(false).show(ctx, |_| {});
        if side > 0.0 {
            egui::SidePanel::left("s").exact_width(side).resizable(false).frame(egui::Frame::NONE).show_separator_line(false).show(ctx, |_| {});
        }
        // 起動の骨格と同じく、panel の後で窓の全面に描く（install_startup の on_end_pass）
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("sk")));
        rects = Some(skeleton::paint_shell(&painter, ctx.screen_rect(), 0.0));
    });
    rects.unwrap()
}

#[test]
fn shell_skeleton_header_is_header_h_and_sidebar_232() {
    let c = ctx(false);
    let mut rects = None;
    frame(&c, 0.0, |ui| rects = Some(skeleton::shell_skeleton(ui)));
    let r = rects.unwrap();
    assert_eq!(r.header.height(), ui::theme::HEADER_H);
    assert_eq!(r.header.width(), SCREEN.width());
    let r = skeleton_after_panels(&c, 1440.0, Tier::for_width(1440.0).sidebar);
    assert_eq!(r.sidebar.width(), 232.0);
    assert_eq!(r.sidebar.top(), ui::theme::HEADER_H);
    assert_eq!(r.body.left(), 232.0);
}

/// UI-4 Done 7（UI-4b の分）: 骨格の sidebar の帯は本物の sidebar の状態どおり — 畳んで SidePanel が無ければ幅 0（本文が窓の左から）・開いていれば `tier.sidebar`。
/// 負の対照（commit message）: `paint_shell` の帯の右端を `screen.left() + tier.sidebar` に戻す → 畳んだ時に 216 / 232 の帯を描いて赤。
#[test]
fn shell_skeleton_sidebar_follows_the_folded_state() {
    let c = ctx(false);
    for w in [900.0, 1200.0, 1440.0] {
        let folded = skeleton_after_panels(&c, w, 0.0);
        assert_eq!((folded.sidebar.width(), folded.body.left()), (0.0, 0.0), "{w}: 畳んだのに骨格が sidebar の帯を描いた");
        let side = Tier::for_width(w).sidebar;
        let open = skeleton_after_panels(&c, w, side);
        assert_eq!((open.sidebar.width(), open.body.left()), (side, side), "{w}: 開いた sidebar の幅と骨格の帯が違う");
    }
}

#[test]
fn with_skeleton_skips_bars_when_load_is_fast() {
    let c = ctx(false);
    // 0.05s: まだ読み込み中だが 120ms 前なので何も描かない
    let out = frame(&c, 0.05, |ui| {
        skeleton::with_skeleton(ui, "r", false, 0.0, Skeleton::List(5), |ui| ui.label("本物"));
    });
    assert_eq!(count(&out).rect, 0, "120ms 前に骨格を描いた");
    // 0.10s で読み終わった: 骨格を挟まず本物だけ
    let out = frame(&c, 0.10, |ui| {
        skeleton::with_skeleton(ui, "r", true, 0.0, Skeleton::List(5), |ui| ui.label("本物"));
    });
    let n = count(&out);
    assert_eq!(n.rect, 0, "速い読み込みで骨格が出た {n:?}");
    assert!(n.text > 0);
}

#[test]
fn with_skeleton_shows_bars_then_cross_fades() {
    let c = ctx(false);
    let started = 10.0;
    // 負の対照: 120ms を過ぎても読み込み中なら骨格を描く（上の test の 0 件が「何も描けない」ではない事）
    let out = frame(&c, started + 0.2, |ui| {
        skeleton::with_skeleton(ui, "r", false, started, Skeleton::List(5), |ui| ui.label("本物"));
    });
    let n = count(&out);
    assert!(n.rect >= 5 && n.text == 0, "{n:?}");
    // 読み終わった frame（進み 0）→ 110ms 後は本物と骨格の両方（cross-fade 中）
    frame(&c, started + 0.25, |ui| {
        skeleton::with_skeleton(ui, "r", true, started, Skeleton::List(5), |ui| ui.label("本物"));
    });
    let out = frame(&c, started + 0.36, |ui| {
        skeleton::with_skeleton(ui, "r", true, started, Skeleton::List(5), |ui| ui.label("本物"));
    });
    let n = count(&out);
    assert!(n.text > 0 && n.rect >= 5, "cross-fade 中に両方が無い {n:?}");
    // 220ms 後は本物だけ
    let out = frame(&c, started + 0.6, |ui| {
        skeleton::with_skeleton(ui, "r", true, started, Skeleton::List(5), |ui| ui.label("本物"));
    });
    let n = count(&out);
    assert!(n.text > 0 && n.rect == 0, "{n:?}");
}

/// 本物の殻と同じ段で骨格の sidebar も狭くなる（cross-fade で sidebar の幅が跳ねない）。UI-4b: 1200 未満も開けば 216（64 の icon の列は消した）。
#[test]
fn shell_skeleton_follows_the_width_tier() {
    let c = ctx(false);
    for (w, side) in [(1300.0, 216.0), (960.0, 216.0)] {
        assert_eq!(Tier::for_width(w).sidebar, side, "{w}");
        let r = skeleton_after_panels(&c, w, Tier::for_width(w).sidebar);
        assert_eq!(r.sidebar.width(), side, "{w}");
        assert_eq!(r.header.height(), ui::theme::HEADER_H);
    }
}

/// M-1: 読み込み中・fade 中・fade 後で、本物の中の id と後ろに並ぶ兄弟の auto id が変わらない
/// （変わると ScrollArea の位置・TextEdit の focus・CollapsingHeader の開閉が fade の終わりに切れる）。
#[test]
fn with_skeleton_keeps_ids_through_the_fade() {
    let c = ctx(false);
    let started = 10.0;
    let mut text = String::new();
    let mut real_ids = vec![];
    let mut after_ids = vec![];
    let mut focused_after = false;
    // 120ms 前・骨格・読み終わり・fade 中（ここで欄に focus）・fade 後・その次
    for (t, loaded) in [(10.05, false), (10.2, false), (10.25, true), (10.36, true), (10.6, true), (10.7, true)] {
        frame(&c, t, |ui| {
            skeleton::with_skeleton(ui, "r", loaded, started, Skeleton::List(5), |ui| {
                real_ids.push((t, ui.id(), ui.next_auto_id(), ui.make_persistent_id("scroll")));
                let r = ui.text_edit_singleline(&mut text);
                if t == 10.36 {
                    r.request_focus();
                }
                if t == 10.7 {
                    focused_after = r.has_focus();
                }
            });
            after_ids.push((t, ui.next_auto_id()));
        });
    }
    assert!(after_ids.windows(2).all(|w| w[0].1 == w[1].1), "後ろの兄弟の auto id が変わった {after_ids:?}");
    assert!(real_ids.windows(2).all(|w| (w[0].1, w[0].2, w[0].3) == (w[1].1, w[1].2, w[1].3)), "本物の中の id が変わった {real_ids:?}");
    assert!(focused_after, "fade 中に focus した欄が fade の終わりに focus を失った");
}

/// M-2: 同じ started_at を共有する 2 つの region が状態を取り合わない。
#[test]
fn with_skeleton_regions_sharing_started_at_are_independent() {
    let c = ctx(false);
    let started = 10.0;
    let (mut oa, mut ob) = (f32::NAN, f32::NAN);
    let mut run = |t: f64, a_loaded: bool| {
        frame(&c, t, |ui| {
            skeleton::with_skeleton(ui, "a", a_loaded, started, Skeleton::Block(40.0), |ui| oa = ui.opacity());
            skeleton::with_skeleton(ui, "b", true, started, Skeleton::List(5), |ui| ob = ui.opacity());
        });
        (oa, ob)
    };
    // B は 50ms で読み終わり骨格を挟まない。A は 120ms を過ぎて骨格を出す → B に骨格が戻って来てはいけない
    assert_eq!(run(10.05, false).1, 1.0);
    assert_eq!(run(10.2, false).1, 1.0, "A の骨格が B に fade し直させた");
    assert_eq!(run(10.25, false).1, 1.0, "A の骨格が B に fade し直させた");
    // A が読み終わったら A 自身の時刻から cross-fade する（B の古い時刻でいきなり切り替わらない）
    run(10.3, true);
    let (a, b) = run(10.4, true);
    assert!(a > 0.0 && a < 1.0, "A が cross-fade しなかった {a}");
    assert_eq!(b, 1.0);
}

/// 本物の殻（header `HEADER_H`・sidebar・必要なら pinned の rail・CentralPanel の draw_app_region）を組み、
/// draw_app_region の列の rect と、同じ frame の end で描いた殻の骨格の rect を返す。
/// `installed` = 実際の app と同じ theme::install（thin の縦 scrollbar が 6px 取る）・`tall` = 中身が画面より長い。
fn real_and_skeleton_with(w: f32, rail: bool, installed: bool, tall: bool) -> (Rect, Rect, skeleton::ShellRects) {
    let c = ctx(false);
    if installed {
        ui::theme::install(&c, egui::ThemePreference::Light);
    }
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, 900.0));
    let tier = Tier::for_width(w);
    let (mut column, mut central, mut rects) = (Rect::NOTHING, Rect::NOTHING, None);
    // show_scroll は前の pass の値・bar の幅は animate_bool で伸びるので、時間を進めて数 pass 回す
    for k in 0..4 {
        let input = RawInput { screen_rect: Some(screen), time: Some(k as f64), ..Default::default() };
        let _ = c.run(input, |ctx| {
            egui::TopBottomPanel::top("h").exact_height(ui::theme::HEADER_H).frame(egui::Frame::NONE).show_separator_line(false).show(ctx, |_| {});
            egui::SidePanel::left("s").exact_width(tier.sidebar).resizable(false).frame(egui::Frame::NONE).show_separator_line(false).show(ctx, |_| {});
            // 右に場所を取る panel（L1-NORAIL で rail は消えた・互換 view の inspector と同じ形）が在っても骨格の列が合う
            if rail {
                egui::SidePanel::right("r").exact_width(296.0).resizable(false).frame(egui::Frame::NONE).show_separator_line(false).show(ctx, |_| {});
            }
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                central = ui.max_rect();
                ui::shell::draw_app_region(ui, &mut |ui| {
                    column = ui.max_rect();
                    if tall {
                        ui.add_space(3000.0);
                    }
                });
            });
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("sk")));
            rects = Some(skeleton::paint_shell(&painter, screen, ctx.input(|i| i.time)));
        });
    }
    (column, central, rects.unwrap())
}

fn real_and_skeleton(w: f32, rail: bool) -> (Rect, Rect, skeleton::ShellRects) {
    real_and_skeleton_with(w, rail, false, false)
}

/// M-3: 骨格の本文の列が本物の draw_app_region と同じ所に来る（cross-fade で横に跳ばない）。rail を pin した時も。
#[test]
fn shell_skeleton_column_matches_the_real_region() {
    for (w, rail) in [(960.0, false), (1300.0, false), (1300.0, true), (1440.0, false), (1440.0, true), (1920.0, false), (1920.0, true)] {
        let (column, central, r) = real_and_skeleton(w, rail);
        assert_eq!(r.body.right(), central.right(), "{w} rail={rail}: 本文の右端");
        assert!((r.column.left() - column.left()).abs() < 0.5, "{w} rail={rail}: 列の左 骨格 {} 本物 {}", r.column.left(), column.left());
        assert!((r.column.width() - column.width()).abs() < 0.5, "{w} rail={rail}: 列の幅 骨格 {} 本物 {}", r.column.width(), column.width());
    }
}

/// M-R2-1: 実際の app の Style（thin の scrollbar）で中身が画面より長い時も、短い時も、骨格の列が本物の列と同じ所に来る。
#[test]
fn shell_skeleton_column_matches_the_real_region_with_the_scrollbar() {
    for tall in [true, false] {
        for (w, rail) in [(960.0, false), (1300.0, true), (1440.0, false), (1920.0, true)] {
            let (column, central, r) = real_and_skeleton_with(w, rail, true, tall);
            assert_eq!(r.body.right(), central.right(), "{w} rail={rail} tall={tall}: 本文の右端");
            assert!((r.column.left() - column.left()).abs() < 0.5, "{w} rail={rail} tall={tall}: 列の左 骨格 {} 本物 {}", r.column.left(), column.left());
            assert!((r.column.width() - column.width()).abs() < 0.5, "{w} rail={rail} tall={tall}: 列の幅 骨格 {} 本物 {}", r.column.width(), column.width());
        }
    }
}

/// M-4: KANGAE_DEV_SKELETON の値。無限大・Duration に入らない数・負・数でない物は弾く（弾かないと起動直後に panic）。
#[test]
fn dev_hold_rejects_values_duration_cannot_hold() {
    assert_eq!(skeleton::dev_hold(Some("1")), Some(1.5));
    assert_eq!(skeleton::dev_hold(Some(" 3 ")), Some(3.0));
    assert_eq!(skeleton::dev_hold(Some("600")), Some(600.0));
    // M-R2-2: 上限は 1 時間。9.3e18〜1.8e19 は Duration には入るが eframe の Instant::now()+delay が溢れて落ちる
    assert_eq!(skeleton::dev_hold(Some("3600")), Some(3600.0));
    for bad in ["inf", "Infinity", "1e20", "1e19", "9.3e18", "3601", "NaN", "0", "-1", "abc", ""] {
        assert_eq!(skeleton::dev_hold(Some(bad)), None, "{bad}");
    }
    assert_eq!(skeleton::dev_hold(None), None);
}

/// M-R3-1: 目標が NaN（0 除算など）でも毎 frame 動き直さない = idle で repaint を頼み続けない。
/// 普通の数に戻ったらその値ちょうど（NaN から lerp しない）。無限大も同じ（inf × 0 の NaN を出さない）。
#[test]
fn animated_number_with_non_finite_target_stops_repainting() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let c = ctx(false);
        let id = Id::new("non-finite");
        frame(&c, 0.0, |ui| {
            motion::animated_number(ui.ctx(), id, 10.0);
        });
        let mut seen = vec![];
        let mut last = FullOutput::default();
        for k in 1..=10 {
            last = frame(&c, k as f64 * 0.05, |ui| seen.push(motion::animated_number(ui.ctx(), id, bad)));
        }
        assert!(seen.iter().all(|v| v.to_bits() == bad.to_bits()), "{bad}: {seen:?}");
        assert!(last.viewport_output.values().all(|v| v.repaint_delay > std::time::Duration::from_secs(1)), "{bad}: 目標が有限でない間 repaint を頼み続けた");
        let mut back = vec![];
        for k in 0..3 {
            frame(&c, 1.0 + k as f64 * 0.05, |ui| back.push(motion::animated_number(ui.ctx(), id, 42.0)));
        }
        assert_eq!(back, vec![42.0; 3], "{bad}: 有限に戻った後に NaN か古い値を出した");
    }
}

/// M-R3-2: 読み込みのたびに started_at が変わっても、region 1 つの状態は ctx の temp に 1 件だけ
/// （数秒ごとに読み直す画面を開いたままでも temp が増え続けない）。読み込みごとに骨格 → cross-fade をやり直す。
#[test]
fn with_skeleton_keeps_one_state_per_region_across_loads() {
    let c = ctx(false);
    let mut len_after = vec![];
    for load in 0..40 {
        let started = 10.0 + load as f64;
        let mut fading = f32::NAN;
        for (dt, loaded) in [(0.05, false), (0.2, false), (0.25, true), (0.36, true), (0.6, true)] {
            frame(&c, started + dt, |ui| {
                skeleton::with_skeleton(ui, "r", loaded, started, Skeleton::List(3), |ui| {
                    if dt == 0.36 {
                        fading = ui.opacity();
                    }
                });
            });
        }
        assert!(fading > 0.0 && fading < 1.0, "読み込み {load}: 骨格から本物へ cross-fade しなかった {fading}");
        len_after.push(c.data(|d| d.len()));
    }
    assert!(len_after[1..].iter().all(|&n| n == len_after[1]), "読み込みのたびに temp が増えた {len_after:?}");
    // 速い 2 回目（120ms 未満）は前の読み込みの「骨格を出した」を引き継がない = cross-fade しない
    let started = 100.0;
    for (dt, loaded) in [(0.2, false), (0.25, true), (0.6, true)] {
        frame(&c, started + dt, |ui| {
            skeleton::with_skeleton(ui, "r", loaded, started, Skeleton::List(3), |_| ());
        });
    }
    let started = 101.0;
    let mut o = f32::NAN;
    frame(&c, started + 0.05, |ui| {
        skeleton::with_skeleton(ui, "r", true, started, Skeleton::List(3), |ui| o = ui.opacity());
    });
    assert_eq!(o, 1.0, "速い読み込みで前の読み込みの骨格を引き継いだ");
}

/// AccessKit を有効にした 1 pass の木。`draw` は ctx 全体に描く（header の TopBottomPanel も置ける）。
fn ax_tree(mut draw: impl FnMut(&Context)) -> egui::accesskit::TreeUpdate {
    let c = ctx(false);
    // header の太字は theme の font family（kangae-semibold）を引くので、実際の app と同じく install する
    ui::theme::install(&c, egui::ThemePreference::Light);
    c.enable_accesskit();
    let out = c.run(RawInput { screen_rect: Some(SCREEN), ..Default::default() }, |ctx| draw(ctx));
    out.platform_output.accesskit_update.expect("accesskit を有効にした pass は木を出す")
}

/// 開いてすぐの殻（header + Home）の木。
fn home_ax_tree() -> egui::accesskit::TreeUpdate {
    let mut shell = ui::shell::Shell::new(View::Home);
    ax_tree(|ctx| {
        ui::header::draw(ctx, &mut shell, "Home", None, &[], false, None, &mut |_| None);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui::home::draw(ui, None, &[], &mut shell, None, None);
        });
    })
}

fn text_inputs(t: &egui::accesskit::TreeUpdate) -> Vec<&egui::accesskit::Node> {
    use egui::accesskit::Role;
    t.nodes.iter().map(|(_, n)| n).filter(|n| matches!(n.role(), Role::TextInput | Role::MultilineTextInput)).collect()
}

/// qa-01 s1 → UI-4a 決定 1: Home の言葉の欄は 1 つだけ（G83 から header でなく主領域の縦の中央・textarea を描かない）・1 行（TextInput）・名前は表の「What to create」（§4.2 placeholder は名前にならない）。
/// 負の対照: home.rs に textarea を戻す → 2 つで赤。
#[test]
fn home_has_one_word_field_named_what_to_create() {
    let t = home_ax_tree();
    let fields = text_inputs(&t);
    assert_eq!(fields.len(), 1, "Home の言葉の欄が 1 つでない");
    assert_eq!((fields[0].role(), fields[0].label()), (egui::accesskit::Role::TextInput, Some("What to create")));
}

/// 1 つの欄（Home は主領域・G83）にも AX の名前（見える label が無いので AccessKit の label で）。
#[test]
fn header_command_field_has_a_name() {
    let t = home_ax_tree();
    assert!(text_inputs(&t).iter().any(|n| n.label() == Some("What to create")), "header の欄に名前が無い");
}

/// cua-driver の `elements` は操作できる行（AX の action を持つ物）だけを並べる。見出しと現在地は Click を持ち、名前で探せる。
/// 負の対照: 説明の label（選べない）は Click を持たない = この test は「全部の node が Click を持つ」で緑になっていない。
#[test]
fn headings_are_listed_by_cua() {
    use egui::accesskit::{Action, Role};
    let t = home_ax_tree();
    let click = |v: &str| t.nodes.iter().find(|(_, n)| n.role() == Role::Label && n.value() == Some(v)).map(|(_, n)| n.supports_action(Action::Click));
    assert_eq!(click("Home"), Some(true), "header の現在地");
    assert!(text_inputs(&t).iter().all(|n| n.supports_action(Action::Click)), "欄に AX の Click（focus）が無い");
    assert!(t.nodes.iter().any(|(_, n)| n.role() == Role::Label && !n.supports_action(Action::Click)), "選べない label が 1 つも無い（負の対照が成り立たない）");
}

fn at(t: f64, w: f32) -> RawInput {
    RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, 900.0))), time: Some(t), ..Default::default() }
}

/// §15.2-2 起動直後（本番の道）: 最初の pass が 120ms 以上かかった時だけ殻の骨格を最前面に置き、そこから 220ms で消える。
/// 速い起動・reduce motion では 1 枚も描かない（ちらつかせない）。
#[test]
fn startup_skeleton_only_when_the_first_frame_was_slow() {
    let frames = |reduce: bool, first: f64| -> Vec<usize> {
        let c = ctx(reduce);
        skeleton::install_startup(&c);
        [0.0, 0.11, 0.3].iter().map(|dt| count(&c.run(at(first + dt, 1440.0), |_| {})).rect).collect()
    };
    let slow = frames(false, 0.5);
    assert!(slow[0] > 0 && slow[1] > 0, "120ms 以上かかった起動で骨格を描かなかった {slow:?}");
    assert_eq!(slow[2], 0, "220ms 経っても骨格が残った {slow:?}");
    assert_eq!(frames(false, 0.05), vec![0, 0, 0], "120ms 未満の起動で骨格を描いた（ちらつき）");
    assert_eq!(frames(true, 0.5), vec![0, 0, 0], "reduce motion で骨格の fade を描いた");
}

/// 殻（header + 主領域）を 1 pass 描き、draw_app_region の中身の (opacity, 上端) を返す。
fn region_after(c: &Context, shell: &mut ui::shell::Shell, t: f64) -> (f32, f32) {
    let mut seen = (f32::NAN, f32::NAN);
    let _ = c.run(at(t, 1440.0), |ctx| {
        ui::header::draw(ctx, shell, "x", None, &[], false, None, &mut |_| None);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui::shell::draw_app_region(ui, &mut |ui| seen = (ui.opacity(), ui.max_rect().top()));
        });
    });
    seen
}

/// §15.2-1 画面の切替: View が替わった frame から中身が opacity 0 / 8px 下 → 220ms で 1 / 元の位置。起動直後の View は動かさない。
#[test]
fn view_switch_fades_and_slides_the_main_region() {
    let c = ctx(false);
    ui::theme::install(&c, egui::ThemePreference::Light);
    let mut shell = ui::shell::Shell::new(View::Home);
    let (o0, top0) = region_after(&c, &mut shell, 1.0);
    assert_eq!(o0, 1.0, "起動直後の View を fade させた");
    shell.go(View::Runs);
    let (o1, top1) = region_after(&c, &mut shell, 2.0);
    assert_eq!(o1, 0.0, "View が替わった frame で中身が出ている（fade しない）");
    assert!((top1 - top0 - motion::SLIDE_PX).abs() < 0.01, "8px 下から始まらない {top0} → {top1}");
    let (o2, top2) = region_after(&c, &mut shell, 2.11);
    assert!(o2 > 0.0 && o2 < 1.0 && top2 > top0 && top2 < top1, "途中の frame が無い o={o2} top={top2}");
    assert_eq!(region_after(&c, &mut shell, 2.5), (1.0, top0), "220ms で元に戻らない");
    // reduce motion なら即時
    let r = ctx(true);
    ui::theme::install(&r, egui::ThemePreference::Light);
    let mut shell = ui::shell::Shell::new(View::Home);
    region_after(&r, &mut shell, 1.0);
    shell.go(View::Runs);
    assert_eq!(region_after(&r, &mut shell, 2.0).0, 1.0, "reduce motion で fade した");
}


/// 殻の 1 つの欄を 1 pass 描いて返った Intent（窓は前面・入力は `events`）。Home の欄は主領域（`home::draw`・G83）・Project は下の panel の Chat の tab の
/// 底の chat bar（`agent_rail::chat_column`・CHAT-ONE-1101 で Home と同じ部品・`ChatRoom` と `CHAT_FIELD_H` は消えた = 残りの領域に列ごと描く）・他は header。
fn field_pass(c: &Context, shell: &mut ui::shell::Shell, t: f64, events: Vec<egui::Event>, apps: &[ui::shell::AppRow]) -> Option<ui::shell::Intent> {
    let mut got = None;
    let _ = c.run(RawInput { screen_rect: Some(SCREEN), time: Some(t), focused: true, events, ..Default::default() }, |ctx| {
        got = ui::header::draw(ctx, shell, "x", None, apps, false, None, &mut |_| None);
        if shell.view == View::Home {
            egui::CentralPanel::default().show(ctx, |ui| got = got.take().or(ui::home::draw(ui, None, apps, shell, None, None)));
        } else if matches!(shell.view, View::Project { .. }) {
            egui::CentralPanel::default().show(ctx, |ui| {
                got = got.take().or(ui::agent_rail::chat_column(ui, shell, &[], None, None, &ui::agent_rail::NoAgent, apps, false, false));
            });
        }
    });
    got
}

fn enter(modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// UI-4a Done 2 / Done 5 / L2: 1 つの欄の Enter は場所で決まる（Home = 会話・Project = 変更・他 = 頭の command）・⌘↵ はどこでも作る・
/// 打っている間は欄が文字を取る（palette は開かない・`k` も文字）。Talk と Generate は欄を空にし、Page の変更は通った合図まで残す。
/// 負の対照: header の ⌘↵ の `consume_key` を外す → Project の ⌘↵ が Generate にならず赤。`enter_intent` の Project の腕を Generate にする → Page(Edit) の assert で赤。
#[test]
fn ui4a_header_enter_goes_by_the_place() {
    use ui::shell::{Intent, Shell};
    let c = ctx(false);
    ui::theme::install(&c, egui::ThemePreference::Light);
    // Home: 開いた時に欄に focus（⌘N と同じ）・打った文字は欄へ・Enter は会話（作らない）
    let mut shell = Shell::new(View::Home);
    field_pass(&c, &mut shell, 0.0, vec![], &[]);
    field_pass(&c, &mut shell, 0.1, vec![egui::Event::Text("Keep research notes k".into())], &[]);
    assert_eq!(shell.command, "Keep research notes k");
    assert!(!shell.palette.open, "打っている間に palette が開いた");
    assert_eq!(field_pass(&c, &mut shell, 0.2, vec![enter(egui::Modifiers::NONE)], &[]), Some(Intent::Talk("Keep research notes k".into())));
    assert_eq!(shell.command, "", "会話の一言を送った後も欄に残る");
    // Project: Enter は Page(Edit)・欄は残す（空にするのは通った合図 = app.rs）。⌘↵ も Page(Edit)（L1-NORAIL）・欄は下の panel の Chat の欄の行
    let project = View::Project { project: "p1".into(), page: "a".into() };
    let mut shell = Shell::new(project);
    shell.focus_command = true;
    field_pass(&c, &mut shell, 1.0, vec![], &[]);
    field_pass(&c, &mut shell, 1.1, vec![egui::Event::Text("add a Tests page".into())], &[]);
    let got = field_pass(&c, &mut shell, 1.2, vec![enter(egui::Modifiers::NONE)], &[]);
    assert!(matches!(&got, Some(Intent::Page(ui::shell::PageIntent::Edit { text })) if text == "add a Tests page"), "{got:?}");
    assert_eq!(shell.command, "add a Tests page", "Page の変更を送っただけで欄を空にした（拒否で直せない）");
    shell.focus_command = true;
    field_pass(&c, &mut shell, 1.3, vec![], &[]);
    assert!(matches!(field_pass(&c, &mut shell, 1.4, vec![enter(egui::Modifiers::COMMAND)], &[]), Some(Intent::Page(ui::shell::PageIntent::Edit { .. }))), "Project の ⌘↵ が Enter と同じでない");
    // Runs: 頭の command（Go）・何にも当たらなければ何もしない（作るのは ⌘↵ だけ）
    let mut shell = Shell::new(View::Runs);
    shell.focus_command = true;
    field_pass(&c, &mut shell, 2.0, vec![], &[]);
    field_pass(&c, &mut shell, 2.1, vec![egui::Event::Text("connections".into())], &[]);
    assert_eq!(field_pass(&c, &mut shell, 2.2, vec![enter(egui::Modifiers::NONE)], &[]), Some(Intent::Go(View::Connections)));
}

/// G89: 窓が潰れた・clip の無い Ui でも Home は有限の rect だけを描き、骨格が引く欄の rect（`paint_shell` の field）も有限で幅 ≥ 0。
/// 高さ 0 / 1 / 負・幅 0 / 負・clip が無限（`Rect::EVERYTHING`）の 6 つ。
/// 負の対照（commit message）: home.rs の見えている高さの `is_finite` を外す → clip 無限で赤・skeleton.rs の帯の右端を `clamp` に戻す → 幅 負で panic。
#[test]
fn g89_home_draws_only_finite_rects_when_the_window_collapses() {
    let wh = |w: f32, h: f32| Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
    // 負 = min と max が裏返った rect（その軸だけ）
    let flipped = |w: f32, h: f32| Rect::from_min_max(Pos2::new((-w).max(0.0), (-h).max(0.0)), Pos2::new(w.max(0.0), h.max(0.0)));
    let cases = [("高さ 0", wh(1440.0, 0.0), false), ("高さ 1", wh(1440.0, 1.0), false), ("高さ 負", flipped(1440.0, -900.0), false), ("幅 0", wh(0.0, 900.0), false), ("幅 負", flipped(-1440.0, 900.0), false), ("clip 無限", SCREEN, true)];
    for (name, screen, unclipped) in cases {
        let c = ctx(false);
        ui::theme::install(&c, egui::ThemePreference::Light);
        let mut shell = ui::shell::Shell::new(View::Home);
        let mut field = None;
        let out = c.run(RawInput { screen_rect: Some(screen), time: Some(0.0), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if unclipped {
                    ui.set_clip_rect(Rect::EVERYTHING);
                }
                ui::home::draw(ui, None, &[], &mut shell, None, None);
            });
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("sk")));
            field = Some(skeleton::paint_shell(&painter, ctx.screen_rect(), 0.0).field);
        });
        let field = field.unwrap();
        assert!(field.is_finite() && field.width() >= 0.0, "{name}: 骨格の欄の rect {field:?}");
        for ClippedShape { shape, .. } in &out.shapes {
            // 空の欄の文（galley が空）は egui の側で外接が NaN になるので、置いた位置だけを見る
            let r = match shape {
                Shape::Text(t) if t.galley.text().is_empty() => Rect::from_min_size(t.pos, Vec2::ZERO),
                s => s.visual_bounding_rect(),
            };
            assert!(r == Rect::NOTHING || r.is_finite(), "{name}: 有限でない形 {r:?}");
        }
    }
}
