//! v0.14 §7.2 の Portal の枠（src/ui/portal_frame.rs・図 docs/diagrams.md §20）の test: 9 状態すべてで 宣伝文 0（docs/ui-copy-rules.json の表）・
//! 次の一手ちょうど 1 つ（押すとその一手が返る）・AX の名前・card に入れない・接続中の skeleton・状態の替わり目の cross-fade と reduce motion。
//! ui は bin crate の module なので `#[path]` で丸ごと取り込む（ui_motion.rs と同じ）。時間は `RawInput::time` で進める。
#![allow(dead_code)]
// ui は agent（rail）と domain（renderer・connections）を crate root の sibling として引く（src/main.rs と同じ並び）
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
mod common;

use eframe::egui::{self, accesskit, Context, FullOutput, Pos2, RawInput, Shape};
use ui::portal_frame::{self as portal, Mode, PortalFrame, PortalResponse, PortalState, SyncMode};
use ui::theme;
use common::{bars_in, ctx, judge, rules, shapes, texts, SCREEN, SHINE_MID};

/// 9 状態それぞれの、本物に近い事実（呼ぶ側 = p-portal / p-mirror が渡す detail の見本）。
fn sample(state: PortalState) -> PortalFrame<'static> {
    use PortalState as S;
    use SyncMode as Y;
    let (sync, mode, detail) = match state {
        S::Undiscovered => (Y::Unsupported, None, "起動していません"),
        S::PermissionRequired => (Y::Unsupported, None, "アクセシビリティ · 窓の位置と大きさを枠に合わせるため"),
        S::Attaching => (Y::Unsupported, None, ""),
        S::NativeDirect => (Y::VisualLive, None, ""),
        S::Mirror => (Y::VisualLive, None, "入力: クリックとキー"),
        S::ViewOnly => (Y::VisualLive, None, ""),
        S::SourceUnavailable => (Y::Stale, None, "最終確認 12:03"),
        S::SyncDelayed => (Y::SemanticPolled, Some(Mode::Manual), "最終確認 12:03 · 根拠: 窓の題名"),
        S::IntegrationFailed => (Y::Unsupported, None, "窓が枠から外れました"),
    };
    PortalFrame { app_name: "REAPER", state, sync, mode, detail }
}

struct Pass {
    out: FullOutput,
    resp: PortalResponse,
    /// 呼ぶ側の body が呼ばれた時の opacity（呼ばれなければ None）
    body: Option<f32>,
}

fn run(c: &Context, t: f64, f: &PortalFrame, events: Vec<egui::Event>) -> Pass {
    let (mut resp, mut body) = (None, None);
    let input = RawInput { screen_rect: Some(SCREEN), time: Some(t), events, ..Default::default() };
    let out = c.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            resp = Some(portal::show(ui, "portal", f, |ui| body = Some(ui.opacity())));
        });
    });
    Pass { out, resp: resp.expect("show が 1 回は走る"), body }
}

/// 9 状態すべてで、描いた文字と名前（状態語・次の一手・Manual / Mirror / External・同期の確度）に宣伝文が 0。
#[test]
fn no_catch_copy_in_any_state() {
    let r = rules();
    let mut names: Vec<&str> = PortalState::ALL.iter().flat_map(|s| [s.wording(), s.next_step().label()]).collect();
    names.extend(SyncMode::ALL.map(portal::sync_label));
    for n in names {
        assert!(judge(&r, n, true).is_empty(), "{n}: {:?}", judge(&r, n, true));
    }
    for s in PortalState::ALL {
        let c = ctx(true);
        let shown = texts(&run(&c, SHINE_MID, &sample(s), vec![]).out);
        assert!(shown.len() >= 2, "{s:?}: 帯の 1 行と button の文字が無い（{shown:?}）");
        for (text, _, _) in &shown {
            assert!(judge(&r, text, false).is_empty(), "{s:?}: {text} {:?}", judge(&r, text, false));
        }
    }
}

// ---------------------------------------------------------------------------
// 次の一手と AX の名前（cua の AX 経路で探す物）
// ---------------------------------------------------------------------------

fn ax(c: &Context, t: f64, f: &PortalFrame) -> accesskit::TreeUpdate {
    c.enable_accesskit();
    run(c, t, f, vec![]).out.platform_output.accesskit_update.expect("accesskit を有効にした pass は木を出す")
}

fn buttons(t: &accesskit::TreeUpdate) -> Vec<&accesskit::Node> {
    t.nodes.iter().map(|(_, n)| n).filter(|n| n.role() == accesskit::Role::Button).collect()
}

fn center(n: &accesskit::Node) -> Pos2 {
    let b = n.bounds().expect("button の bounds");
    Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

/// どの状態でも button はちょうど 1 つで、名前は次の一手。押すとその一手が返る。
#[test]
fn exactly_one_next_step_in_every_state() {
    for s in PortalState::ALL {
        let c = ctx(true);
        let f = sample(s);
        let tree = ax(&c, 1.0, &f);
        let b = buttons(&tree);
        // RQ4: 元の app へ行く 2 つ目の手（Open original・`alt_step`）が在る状態は 2 つ。次の一手はちょうど 1 つ
        assert_eq!(b.len(), 1 + usize::from(s.alt_step().is_some()), "{s:?}: button が {} 個", b.len());
        let next = b.iter().copied().filter(|n| n.label() == Some(s.next_step().label())).collect::<Vec<_>>();
        assert_eq!(next.len(), 1, "{s:?}: 次の一手の button が 1 つでない");
        let pos = center(next[0]);
        let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        assert_eq!(run(&c, 1.02, &f, vec![egui::Event::PointerMoved(pos)]).resp.action, None, "{s:?}: 乗せただけで返った");
        assert_eq!(run(&c, 1.04, &f, vec![press(true)]).resp.action, None, "{s:?}: 押し下げで返った");
        assert_eq!(run(&c, 1.06, &f, vec![press(false)]).resp.action, Some(s.next_step()), "{s:?}: 押しても次の一手が返らない");
    }
}

/// RQ6（RQ-024）: 帯の今の活動の 1 行は、言う事がある間だけ押せない静的な文（AX 名 `Activity`）が 1 つ増える。button の数・名前と帯の 1 行（値）は変わらない。
/// 負の対照: `set_activity` を見ずに常に描く → 言わない時にも `Activity` が出て赤・描かない → 言う時に無くて赤。
#[test]
fn rq6_activity_is_one_extra_static_label_and_changes_nothing_else() {
    use accesskit::Role;
    for s in PortalState::ALL {
        let c = ctx(true);
        let f = sample(s);
        let named = |t: &accesskit::TreeUpdate| t.nodes.iter().filter(|(_, n)| n.label() == Some("Activity")).count();
        portal::set_activity(&c, None);
        let before = ax(&c, 1.0, &f);
        assert_eq!(named(&before), 0, "{s:?}: 言う事が無いのに Activity が出た");
        portal::set_activity(&c, Some("Waiting for you to finish typing in REAPER".into()));
        let after = ax(&c, 1.1, &f);
        assert_eq!(named(&after), 1, "{s:?}: 言う事が在るのに Activity が出ない");
        let names = |t: &accesskit::TreeUpdate| {
            let mut v: Vec<(Role, String)> = t.nodes.iter().filter(|(_, n)| n.label() != Some("Activity")).map(|(_, n)| (n.role(), n.label().or(n.value()).unwrap_or("").to_owned())).collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        };
        let buttons_of = |t: &accesskit::TreeUpdate| buttons(t).iter().filter_map(|n| n.label().map(str::to_owned)).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(buttons_of(&before), buttons_of(&after), "{s:?}: button が替わった");
        assert!(names(&after).iter().any(|(_, l)| *l == f.status_line()), "{s:?}: 帯の 1 行が消えた");
        let _ = names(&before);
    }
}

/// 枠に名前（app 名 · 状態語）・帯の 1 行は選べる label（Click を持つ = cua の elements に出る）で値は status_line。
#[test]
fn ax_names_in_every_state() {
    use accesskit::{Action, Role};
    for s in PortalState::ALL {
        let c = ctx(true);
        let f = sample(s);
        let tree = ax(&c, 1.0, &f);
        let group = if s.wording().is_empty() { "REAPER".to_owned() } else { format!("REAPER · {}", s.wording()) };
        assert!(tree.nodes.iter().any(|(_, n)| n.role() == Role::Group && n.label() == Some(group.as_str())), "{s:?}: 枠の名前「{group}」が無い");
        let line = f.status_line();
        let node = tree.nodes.iter().find(|(_, n)| n.role() == Role::Label && n.value() == Some(line.as_str()));
        assert!(node.is_some_and(|(_, n)| n.supports_action(Action::Click)), "{s:?}: 帯の 1 行「{line}」が選べる label に無い");
        assert!(line.contains("REAPER") && line.contains(s.wording()), "{s:?}: {line}");
    }
}

/// 状態語と同期の確度（英語）が帯の 1 行に出る（Manual / Mirror / External の語は UI-1 で帯から外した・表に英語が無い）。
#[test]
fn live_states_tell_manual_or_mirror_and_freshness() {
    let line = |s| sample(s).status_line();
    assert_eq!(line(PortalState::NativeDirect), "REAPER · Live view");
    assert_eq!(line(PortalState::Mirror), "REAPER · Mirroring · Live view · 入力: クリックとキー");
    assert_eq!(line(PortalState::ViewOnly), "REAPER · View only · Live view");
    assert_eq!(line(PortalState::SyncDelayed), "REAPER · Behind · Checked periodically · 最終確認 12:03 · 根拠: 窓の題名");
    assert_eq!(line(PortalState::SourceUnavailable), "REAPER · Not available · May be out of date");
}

// ---------------------------------------------------------------------------
// 1 つの大きな作業領域（card に入れない）
// ---------------------------------------------------------------------------

/// slot は available の全幅・帯の下から下端まで。button は帯の中（元の窓が slot を覆っても押せる）。slot を囲む角丸の面が無い。
#[test]
fn portal_is_one_big_area_not_a_card() {
    for s in PortalState::ALL {
        let c = ctx(true);
        let f = sample(s);
        let tree = ax(&c, 1.0, &f);
        let pass = run(&c, 1.0, &f, vec![]);
        let slot = pass.resp.slot;
        assert!(slot.width() >= SCREEN.width() - 2.0 * theme::HAIRLINE, "{s:?}: slot の幅 {}", slot.width());
        assert!(slot.height() >= SCREEN.height() - theme::STRIP_H - theme::HAIRLINE, "{s:?}: slot の高さ {}", slot.height());
        assert_eq!(slot.top(), SCREEN.top() + theme::STRIP_H, "{s:?}: slot が帯の真下から始まらない");
        let b = buttons(&tree)[0].bounds().unwrap();
        assert!((b.y1 as f32) <= slot.top(), "{s:?}: button が slot に入っている（元の窓に覆われる）");
        for shape in shapes(&pass.out) {
            if let Shape::Rect(r) = shape {
                let rounded = r.corner_radius != egui::CornerRadius::ZERO;
                // 接続中の骨格の帯（角丸 6）は slot と同じ大きさなので、slot より外へ広がる面だけを card とみなす
                assert!(!(rounded && r.rect.expand(0.5).contains_rect(slot) && r.rect != slot), "{s:?}: slot を囲む角丸の面 {:?}", r.rect);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 接続中の skeleton・替わり目の cross-fade・reduce motion
// ---------------------------------------------------------------------------

/// 接続中: 120ms 未満は描かない → それ以降は slot に帯と光（文字 0）。
#[test]
fn attaching_shows_skeleton_after_120ms_with_no_text_in_slot() {
    let c = ctx(false);
    let f = sample(PortalState::Attaching);
    let first = run(&c, 1.0, &f, vec![]);
    assert_eq!(bars_in(&first.out, first.resp.slot), (0, 0), "接続中に入ってすぐ骨格が出た（ちらつく）");
    let p = run(&c, SHINE_MID, &f, vec![]);
    let (rects, meshes) = bars_in(&p.out, p.resp.slot);
    assert!(rects >= 1 && meshes >= 1, "骨格の帯 {rects}・光 {meshes}");
    assert!(texts(&p.out).iter().all(|(_, r, _)| !p.resp.slot.intersects(*r)), "骨格の slot に文字");
    assert_eq!(p.body, None, "接続中に呼ぶ側の body を呼んだ");
}

/// 接続中 → Native Direct 中: 骨格から本物へ 220ms で cross-fade（途中は骨格も本物も薄く在る）。
#[test]
fn attaching_to_native_direct_cross_fades_from_the_skeleton() {
    let c = ctx(false);
    run(&c, 1.0, &sample(PortalState::Attaching), vec![]);
    run(&c, SHINE_MID, &sample(PortalState::Attaching), vec![]);
    let live = sample(PortalState::NativeDirect);
    let start = run(&c, SHINE_MID + 0.01, &live, vec![]);
    let mid = run(&c, SHINE_MID + 0.12, &live, vec![]);
    let end = run(&c, SHINE_MID + 0.5, &live, vec![]);
    let (a, b, z) = (start.body.unwrap(), mid.body.unwrap(), end.body.unwrap());
    assert!(a < 0.2 && a < b && b < 1.0 && z == 1.0, "body の opacity {a} → {b} → {z}");
    assert!(bars_in(&mid.out, mid.resp.slot).0 >= 1, "fade の途中で骨格が消えた");
    assert_eq!(bars_in(&end.out, end.resp.slot).0, 0, "fade の後も骨格が残る");
}

/// 映像の無い状態どうし（未発見 → 繋げなかった）: 前の見出しが薄れて新しい見出しが濃くなる。帯の頭（印の席）の色も寄せる。
/// ICONS-1101 追記 238 から Not found と Not available は同じ `text_secondary`（印も無い）なので、色の替わる Couldn't attach（Error = 赤）で見る。
#[test]
fn state_change_cross_fades_the_fact_and_the_color() {
    let c = ctx(false);
    let t = theme::LIGHT;
    let (from, to) = (sample(PortalState::Undiscovered), sample(PortalState::IntegrationFailed));
    run(&c, 1.0, &from, vec![]);
    run(&c, 2.0, &to, vec![]);
    let mid = run(&c, 2.11, &to, vec![]);
    let end = run(&c, 2.5, &to, vec![]);
    let has = |p: &Pass, s: &str| texts(&p.out).iter().any(|(x, r, _)| x.contains(s) && p.resp.slot.intersects(*r));
    assert!(has(&mid, "Not found") && has(&mid, "Couldn't attach"), "fade の途中に前と今の見出しが両方ない");
    assert!(!has(&end, "Not found") && has(&end, "Couldn't attach"), "fade の後も前の見出しが残る");
    let strip = |p: &Pass| texts(&p.out).into_iter().find(|(x, _, _)| x == &to.status_line()).map(|(_, _, c)| c).expect("帯の 1 行");
    let target = PortalState::IntegrationFailed.region().color(&t);
    let before = PortalState::Undiscovered.region().color(&t);
    assert!(strip(&mid) != target && strip(&mid) != before, "帯の glyph の色が寄っていない {:?}", strip(&mid));
    assert_eq!(strip(&end), target);
}

/// reduce motion: 替わった frame で即時（前の見出しも骨格も重ねない・body は opacity 1）。
#[test]
fn reduce_motion_switches_states_at_once() {
    let c = ctx(true);
    let t = theme::LIGHT;
    run(&c, 1.0, &sample(PortalState::Undiscovered), vec![]);
    let to = sample(PortalState::SourceUnavailable);
    let p = run(&c, 2.0, &to, vec![]);
    assert!(texts(&p.out).iter().all(|(x, _, _)| !x.contains("見つかりません")), "reduce motion で前の見出しを重ねた");
    let strip = texts(&p.out).into_iter().find(|(x, _, _)| x == &to.status_line()).unwrap().2;
    assert_eq!(strip, PortalState::SourceUnavailable.region().color(&t));

    let c = ctx(true);
    run(&c, 1.0, &sample(PortalState::Attaching), vec![]);
    let sk = run(&c, SHINE_MID, &sample(PortalState::Attaching), vec![]);
    assert_eq!(bars_in(&sk.out, sk.resp.slot).1, 0, "reduce motion で光を流した");
    let live = run(&c, SHINE_MID + 0.01, &sample(PortalState::NativeDirect), vec![]);
    assert_eq!(live.body, Some(1.0));
    assert_eq!(bars_in(&live.out, live.resp.slot).0, 0, "reduce motion で骨格を重ねた");
}
