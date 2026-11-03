//! DESIGN §16.4 の生成中の画面（src/ui/build_progress.rs・図 docs/diagrams.md §24）の test: 状態ごとに宣伝文 0・周回の数字 / loss / % が出ない・
//! 次の一手ちょうど 1 つ（通った時は 0 = app だけ）・AX の名前・生成中の骨格（予定の component の形）・通った時の cross-fade・reduce motion。
//! 入力は refine の `--json` の形の fixture（tests/fixtures/build_progress/・not_built.json は実物の出力）。
#![allow(dead_code)]
// ui::agent_rail は agent を、ui::renderer と ui::connections は domain を crate root の sibling として参照する（tests/ui_motion.rs と同じ並び）
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

use common::{bars_in, ctx, judge, rules, texts, SCREEN, SHINE_MID};
use eframe::egui::{self, accesskit, Context, FullOutput, Pos2, RawInput, Rect};
use ui::build_progress::{self as build, about_rows, next_step, Build, BuildAction, Phase, RefineOutcome, Stop};

const FIXTURES: [&str; 5] = ["no_improvement", "handoff", "deadline", "not_built", "passed"];
/// data の文に数字を入れていない fixture（loss・elapsed_ms・trace の round だけが数字）。not_built は実物なので「1 つ」を含む
const DIGIT_FREE: [&str; 4] = ["no_improvement", "handoff", "deadline", "passed"];
const PLANNED: [&str; 2] = ["list", "form"];

fn raw(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/build_progress/{name}.json", env!("CARGO_MANIFEST_DIR"))).expect("fixture")
}

fn fixture(name: &str) -> RefineOutcome {
    RefineOutcome::from_json(&raw(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

struct Pass {
    out: FullOutput,
    action: Option<BuildAction>,
    /// 呼ぶ側の app が呼ばれた時の (opacity, 上端)
    app: Option<(f32, f32)>,
}

fn run(c: &Context, t: f64, b: &Build, events: Vec<egui::Event>) -> Pass {
    let (mut action, mut app) = (None, None);
    let input = RawInput { screen_rect: Some(SCREEN), time: Some(t), events, ..Default::default() };
    let out = c.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            action = build::show(ui, "build", b, |ui| {
                app = Some((ui.opacity(), ui.cursor().top()));
                ui.label("app の本文");
            });
        });
    });
    Pass { out, action, app }
}

fn running(phase: Phase) -> Build<'static> {
    Build::Running { phase, planned: &PLANNED, waiting: None, elapsed: std::time::Duration::ZERO }
}

/// 生成中の 5 段と、fixture の結果 5 つ。
fn every_state(outcomes: &[RefineOutcome]) -> Vec<Build<'_>> {
    Phase::ALL.iter().map(|&p| running(p)).chain(outcomes.iter().map(Build::Done)).collect()
}

/// 状態の行の下（骨格・理由・app）の領域。
fn below_row() -> Rect {
    Rect::from_min_max(Pos2::new(SCREEN.left(), SCREEN.top() + ui::theme::LIST_ROW_H), SCREEN.max)
}

#[test]
fn fixtures_are_read_as_refine_json() {
    let o = fixture("no_improvement");
    assert!(!o.passed && o.stop == Stop::NoImprovement && o.has_best_app() && o.decisions.len() == 2);
    assert!(raw("no_improvement").contains("\"loss\": 37") && raw("no_improvement").contains("\"elapsed_ms\": 98765"), "数字の test の前提（fixture に loss と経過が在る）");
    assert_eq!((fixture("not_built").stop, fixture("not_built").has_best_app()), (Stop::NotBuilt, false));
    assert_eq!((fixture("deadline").stop, fixture("deadline").has_best_app()), (Stop::Deadline, false));
    assert!(fixture("passed").passed);
}

/// 状態ごとに、描いた文字・rail の行・名前（段の語・止まった語・既定の理由・次の一手・経路の語）に宣伝文が 0。
/// route の語（撤去した `cuadriver` を含む・前は種の registry の schema の enum から取った。種の registry は owner 10-23 で製品から外した）と
/// 知らない語は、内部の id のまま画面に出さない（W18-1 反証: 撤去した経路の語が素通りした）。
const ROUTES: [&str; 9] = ["local", "api", "mcp", "web", "native_handoff", "native_docking", "mirror", "cuadriver", "repo"];

#[test]
fn route_word_never_shows_a_raw_route_id() {
    let r = rules();
    for id in ROUTES.iter().map(|s| s.to_string()).chain(["something_new".to_string()]) {
        let w = build::route_word(Some(&id));
        assert_ne!(w, id, "{id} が生のまま");
        // 表の EN は語の並び（空白を含む）か API / MCP / Mirrored / Other（表の 1 語）。内部の id は空白を含まない
        assert!(w.contains(' ') || ["API", "MCP", "Mirrored", "Other"].contains(&w), "{id} → {w}: 内部の語");
        assert!(judge(&r, w, true).is_empty(), "{id} → {w}: {:?}", judge(&r, w, true));
    }
}

#[test]
fn no_catch_copy_in_any_state() {
    let r = rules();
    let mut names: Vec<String> = Phase::ALL.iter().map(|p| p.wording().to_string()).collect();
    for s in [Stop::NoImprovement, Stop::Deadline, Stop::NotBuilt] {
        names.extend([s.wording().to_string(), s.fallback_reason().to_string()]);
    }
    let any = String::from("X");
    for a in [BuildAction::Cancel, BuildAction::AllowConnection(any.clone()), BuildAction::OpenInSource(any), BuildAction::Rephrase, BuildAction::Retry] {
        names.push(a.label().to_string());
    }
    for route in [None, Some("api"), Some("local"), Some("mcp"), Some("native_handoff"), Some("web"), Some("cuadriver")] {
        names.push(build::route_word(route).to_string());
    }
    for n in &names {
        assert!(judge(&r, n, true).is_empty(), "{n}: {:?}", judge(&r, n, true));
    }
    let outcomes: Vec<RefineOutcome> = FIXTURES.iter().map(|f| fixture(f)).collect();
    for b in every_state(&outcomes) {
        let c = ctx(true);
        let shown = texts(&run(&c, SHINE_MID, &b, vec![]).out);
        assert!(!shown.is_empty(), "{b:?}: 何も描いていない");
        for (text, _, _) in &shown {
            assert!(judge(&r, text, false).is_empty(), "{:?}: {text} {:?}", b.status_line(), judge(&r, text, false));
        }
    }
    for (f, o) in FIXTURES.iter().zip(&outcomes) {
        for (k, v) in about_rows(o) {
            for text in [k, v] {
                assert!(judge(&r, &text, false).is_empty(), "{f} の rail: {text} {:?}", judge(&r, &text, false));
            }
        }
    }
}

/// 周回の数字・loss・経過時間・% を画面にも rail にも出さない（fixture に loss 37・elapsed 98765・round 0〜3 が在っても）。
#[test]
fn no_round_loss_or_percent_in_ui() {
    let banned = ["周", "回目", "loss", "%", "％", "秒", "round"];
    let outcomes: Vec<RefineOutcome> = DIGIT_FREE.iter().map(|f| fixture(f)).collect();
    let mut seen = Vec::new();
    for b in every_state(&outcomes) {
        let c = ctx(true);
        seen.extend(texts(&run(&c, SHINE_MID, &b, vec![]).out).into_iter().map(|(t, _, _)| t));
    }
    for o in &outcomes {
        seen.extend(about_rows(o).into_iter().flat_map(|(k, v)| [k, v]));
    }
    assert!(seen.len() > 10, "見た文字が少なすぎる（{seen:?}）");
    for text in &seen {
        assert!(!text.chars().any(|c| c.is_ascii_digit()), "数字が出た: {text}");
        assert!(!banned.iter().any(|w| text.contains(w)), "周回・loss の語が出た: {text}");
    }
}

fn ax(c: &Context, b: &Build) -> accesskit::TreeUpdate {
    c.enable_accesskit();
    run(c, 1.0, b, vec![]).out.platform_output.accesskit_update.expect("accesskit を有効にした pass は木を出す")
}

fn buttons(t: &accesskit::TreeUpdate) -> Vec<&accesskit::Node> {
    t.nodes.iter().map(|(_, n)| n).filter(|n| n.role() == accesskit::Role::Button).collect()
}

/// 生成中・通らない時は button ちょうど 1 つ（押すとその一手）。通った時は 0（app だけ）。
#[test]
fn exactly_one_next_step_while_building_or_failing() {
    let outcomes: Vec<RefineOutcome> = FIXTURES.iter().map(|f| fixture(f)).collect();
    for b in every_state(&outcomes) {
        let c = ctx(true);
        let tree = ax(&c, &b);
        let bs = buttons(&tree);
        let Some(want) = b.action() else {
            assert!(bs.is_empty(), "通った時に button が {} 個", bs.len());
            continue;
        };
        assert_eq!(bs.len(), 1, "{:?}: button が {} 個", b.status_line(), bs.len());
        assert_eq!(bs[0].label(), Some(want.label()));
        let r = bs[0].bounds().unwrap();
        let pos = Pos2::new(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32);
        let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&c, 1.02, &b, vec![egui::Event::PointerMoved(pos)]);
        run(&c, 1.04, &b, vec![press(true)]);
        assert_eq!(run(&c, 1.06, &b, vec![press(false)]).action, Some(want), "{:?}", b.status_line());
    }
    let want = |f: &str| next_step(&fixture(f));
    assert_eq!(want("no_improvement"), Some(BuildAction::AllowConnection("Gmail".into())));
    assert_eq!(want("handoff"), Some(BuildAction::OpenInSource("REAPER".into())));
    assert_eq!(want("deadline"), Some(BuildAction::Retry));
    assert_eq!(want("not_built"), Some(BuildAction::Rephrase));
    assert_eq!(want("passed"), None);
}

/// 画面に名前（状態 1 行）・状態の行と理由は選べる label（Click を持つ = cua の elements に出る）。
#[test]
fn ax_names_for_the_row_and_reasons() {
    use accesskit::{Action, Role};
    let outcomes: Vec<RefineOutcome> = FIXTURES.iter().map(|f| fixture(f)).collect();
    for b in every_state(&outcomes) {
        let Some(line) = b.status_line() else { continue };
        let c = ctx(true);
        let tree = ax(&c, &b);
        assert!(tree.nodes.iter().any(|(_, n)| n.role() == Role::Group && n.label() == Some(line.as_str())), "{line}: 画面の名前が無い");
        let click = |v: &str| tree.nodes.iter().any(|(_, n)| n.role() == Role::Label && n.value() == Some(v) && n.supports_action(Action::Click));
        assert!(click(&line), "{line}: 状態の行が選べる label に無い");
        if let Build::Done(o) = b {
            for r in build::reasons(o) {
                assert!(click(&format!("· {r}")), "{line}: 理由「{r}」が選べる label に無い");
            }
        }
    }
}

/// FX2 反証の直し: Home の 1 行の文が `Open Runs` の link になるのは、その行に Runs の行（Home の生成の run）が在る形だけ（図 §45 の RN・ST・E0・F）。
/// CLI を確かめている間（`Running` の `waiting` が Some = job はまだ無い）と、Page の変更を読んでいる間の Home の 1 行（`Ended { Loading }` = 編集の job は
/// Runs に行を足さない）は文だけ（AX の名前は文・押しても `OpenRun` を返さない = 何も開かない Runs へ移らない）。
/// 負の対照（commit message）: `show` の link の条件を前の `!matches!(build, Build::Done(_))` に戻す → 2 つの形で `Open Runs` が出て赤。
#[test]
fn fx2_open_runs_link_only_on_rows_with_a_run() {
    use std::time::Duration;
    use ui::states::RegionState;
    let press_line = |b: &Build| -> (bool, Option<BuildAction>) {
        let c = ctx(true);
        let tree = ax(&c, b);
        let line = b.status_line().expect("1 行");
        let linked = tree.nodes.iter().any(|(_, n)| n.label() == Some("Open Runs"));
        let r = tree.nodes.iter().find(|(_, n)| n.role() == accesskit::Role::Label && n.value() == Some(line.as_str())).and_then(|(_, n)| n.bounds()).expect("1 行の文の label");
        let pos = Pos2::new(r.x0 as f32 + 8.0, ((r.y0 + r.y1) / 2.0) as f32);
        let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&c, 1.02, b, vec![egui::Event::PointerMoved(pos)]);
        run(&c, 1.04, b, vec![press(true)]);
        (linked, run(&c, 1.06, b, vec![press(false)]).action)
    };
    let failed = BuildAction::Retry;
    let with_run = [
        Build::Running { phase: Phase::Read, planned: &[], waiting: None, elapsed: Duration::from_secs(2) },
        Build::Stopping { elapsed: Duration::from_secs(1) },
        Build::Ended { state: RegionState::Ready, line: "Stopped. Nothing was added to Projects.", action: None },
        Build::Ended { state: RegionState::Error, line: "AI wasn't available · x", action: Some(&failed) },
    ];
    for b in with_run {
        assert_eq!(press_line(&b), (true, Some(BuildAction::OpenRun)), "run の在る行は文を押すと Open Runs: {:?}", b.status_line());
    }
    let without_run = [
        Build::Running { phase: Phase::Read, planned: &[], waiting: Some("Checking for AI on this Mac"), elapsed: Duration::from_secs(2) },
        Build::Ended { state: RegionState::Loading, line: "Reading your change", action: None },
    ];
    for b in without_run {
        assert_eq!(press_line(&b), (false, None), "run の無い行が Open Runs の link: {:?}", b.status_line());
    }
}

/// 理由は人の言葉で 1〜3 行（id・規則の名前・宣伝文そのものを写さない）。
#[test]
fn reasons_are_one_to_three_plain_lines() {
    let lines = build::reasons(&fixture("no_improvement"));
    assert_eq!(lines, ["Leftover description text", "A part has no loading state", "「未読」の一覧が無い"]);
    assert_eq!(build::reasons(&fixture("not_built")), ["目的を 1 つ決める: 何を作りたいですか？"]);
    assert_eq!(build::reasons(&fixture("deadline")), [Stop::Deadline.fallback_reason()]);
    for f in FIXTURES {
        let o = fixture(f);
        let n = build::reasons(&o).len();
        assert!(o.passed || (1..=build::MAX_REASONS).contains(&n), "{f}: {n} 行");
    }
}

/// 通らない時: 状態の行・理由・最良案の順（理由は最良案の上）。最良案が無ければ app を呼ばない。
#[test]
fn failure_puts_reasons_above_the_best_app() {
    let o = fixture("no_improvement");
    let c = ctx(true);
    let p = run(&c, 1.0, &Build::Done(&o), vec![]);
    let (_, app_top) = p.app.expect("最良案を描く");
    let shown = texts(&p.out);
    let bottom = |s: &str| shown.iter().find(|(t, _, _)| t == s).map(|(_, r, _)| r.bottom()).unwrap_or_else(|| panic!("{s} が無い"));
    for r in build::reasons(&o) {
        assert!(bottom(&format!("· {r}")) <= app_top, "理由「{r}」が最良案より下");
    }
    assert!(bottom(&Build::Done(&o).status_line().unwrap()) <= app_top);
    let d = fixture("deadline");
    assert_eq!(run(&ctx(true), 1.0, &Build::Done(&d), vec![]).app, None, "最良案が無いのに app を呼んだ");
}

/// 生成中: 120ms 未満は骨格を描かない → それ以降は行の下に帯と光（文字 0）。
#[test]
fn building_shows_the_app_shaped_skeleton_after_120ms() {
    let c = ctx(false);
    let b = running(Phase::Compose);
    let first = run(&c, 1.0, &b, vec![]);
    assert_eq!(bars_in(&first.out, below_row()), (0, 0), "始めてすぐ骨格が出た");
    let p = run(&c, SHINE_MID, &b, vec![]);
    let (rects, meshes) = bars_in(&p.out, below_row());
    assert!(rects >= 5 && meshes >= 1, "骨格の帯 {rects}・光 {meshes}");
    assert!(texts(&p.out).iter().all(|(_, r, _)| !below_row().intersects(*r)), "骨格に文字");
    assert_eq!(p.app, None, "生成中に app を呼んだ");
}

/// 骨格の形は予定の component で決まる（table と chat で帯の数が違う）。
#[test]
fn planned_components_shape_the_skeleton() {
    let bars = |planned: &'static [&'static str]| {
        let c = ctx(true);
        let b = Build::Running { phase: Phase::Compose, planned, waiting: None, elapsed: std::time::Duration::ZERO };
        run(&c, 1.0, &b, vec![]);
        let p = run(&c, 1.5, &b, vec![]);
        bars_in(&p.out, below_row()).0
    };
    let (table, chat) = (bars(&["table"]), bars(&["chat"]));
    assert!(table >= 1 && chat >= 1 && table != chat, "table {table}・chat {chat}");
}

/// 生成中 → 通った: 行が消え、骨格から app へ 220ms で cross-fade（途中は骨格も app も薄く在る）。
#[test]
fn passing_cross_fades_from_the_skeleton_to_the_app() {
    let c = ctx(false);
    let b = running(Phase::Verify);
    run(&c, 1.0, &b, vec![]);
    run(&c, SHINE_MID, &b, vec![]);
    let o = fixture("passed");
    let done = Build::Done(&o);
    let start = run(&c, SHINE_MID + 0.01, &done, vec![]);
    let mid = run(&c, SHINE_MID + 0.12, &done, vec![]);
    let end = run(&c, SHINE_MID + 0.5, &done, vec![]);
    let (a, m, z) = (start.app.unwrap().0, mid.app.unwrap().0, end.app.unwrap().0);
    assert!(a < 0.2 && a < m && m < 1.0 && z == 1.0, "app の opacity {a} → {m} → {z}");
    assert!(bars_in(&mid.out, SCREEN).0 >= 1, "fade の途中で骨格が消えた");
    assert_eq!(bars_in(&end.out, SCREEN).0, 0, "fade の後も骨格が残る");
    assert!(texts(&end.out).iter().all(|(t, _, _)| !t.contains(Phase::Verify.wording())), "通った後も状態の行が残る");
}

/// reduce motion: 光を流さず、通った frame で即時（骨格を重ねない・app は opacity 1）。
#[test]
fn reduce_motion_is_immediate() {
    let c = ctx(true);
    let b = running(Phase::Fix);
    run(&c, 1.0, &b, vec![]);
    let sk = run(&c, SHINE_MID, &b, vec![]);
    assert!(bars_in(&sk.out, below_row()).0 >= 1);
    assert_eq!(bars_in(&sk.out, below_row()).1, 0, "reduce motion で光を流した");
    let o = fixture("passed");
    let p = run(&c, SHINE_MID + 0.01, &Build::Done(&o), vec![]);
    assert_eq!(p.app.map(|a| a.0), Some(1.0));
    assert_eq!(bars_in(&p.out, SCREEN).0, 0, "reduce motion で骨格を重ねた");
}

/// rail の「このアプリについて」: 操作 1 つに 1 行（decisions に無い codegen も 1 行）・内部の語を出さない。
#[test]
fn about_rows_one_per_operation() {
    let rows = about_rows(&fixture("no_improvement"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("Gmail".to_string(), "API · thread と返信（件数・差出人・本文）を読む · Instead: On the web".to_string()));
    let deadline = about_rows(&fixture("deadline"));
    assert_eq!(deadline.len(), 2, "decisions 1 + decisions に無い codegen 1: {deadline:?}");
    for f in FIXTURES {
        for (_, v) in about_rows(&fixture(f)) {
            assert!(!v.contains("requires_connection") && !v.contains("discoverable") && !v.contains("codegen"), "{f}: {v}");
        }
    }
}
