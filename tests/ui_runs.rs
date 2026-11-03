//! Runs の一覧と詳細（src/ui/runs.rs・図 docs/diagrams.md §22）の test: 10 状態 × 5 結果（+ 結果なし）のすべてで宣伝文 0・
//! 次の一手は 0 か 1（終わった物は行の「開く」だけ）・内部の id を見せない・外に効いた run は相手と取り消しを出す・AX の名前・
//! 読み込み中の骨格・結果の色の cross-fade・reduce motion。入力は domain の RunStatus / Outcome / Attempt / Effect。
#![allow(dead_code)]
// ui::agent_rail は agent を crate root の sibling として参照する（tests/ui_motion.rs と同じ並び）
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

use common::{ctx, judge, rules, shapes, texts, SCREEN, SHINE_MID};
use domain::action::Effect;
use domain::run::{Attempt, ErrorCode, Outcome, RunStatus};
use eframe::egui::{self, accesskit, Context, FullOutput, Pos2, RawInput, Rect};
use ui::runs::{self, next_step, RunAction, RunEntry, RunResult, RunsAction, RunsView, Undo};

const STATUSES: [RunStatus; 10] = [
    RunStatus::Idle,
    RunStatus::Planning,
    RunStatus::WaitingConnection,
    RunStatus::WaitingApproval,
    RunStatus::Running,
    RunStatus::Verifying,
    RunStatus::Verified,
    RunStatus::Failed,
    RunStatus::Cancelled,
    RunStatus::Uncertain,
];
/// 「今」= 2026-10-18T12:00:00Z
const NOW: i64 = 1_792_324_800;

/// runtime が実際に入れる形の結果（evidence に id・URL、reason に debug 名を混ぜてある）。
fn outcomes() -> Vec<Option<Outcome>> {
    vec![
        Some(Outcome::Verified { evidence: vec!["drafts.get r-1234567890: 宛先・件名・本文が一致（送信していない）".into(), "rec_42".into(), "https://calendar.google.com/event?eid=abc".into()] }),
        Some(Outcome::Unverified { reason: "読み直しに失敗した（Timeout）".into() }),
        Some(Outcome::Rejected { reason: "相手の service が受け付けなかった".into() }),
        Some(Outcome::Failed { error: ErrorCode::ConnectionLost }),
        Some(Outcome::EffectUnknown { reason: "handler が途中で落ちた。届いたか分からない（自動で送り直さない）".into() }),
        None,
    ]
}

fn entry(id: &str, status: RunStatus, outcome: Option<Outcome>, effect: Effect, target: &str) -> RunEntry {
    let attempts = vec![Attempt { seq: 3, started_at: "2026-10-18T11:56:00Z".into(), finished_at: outcome.as_ref().map(|_| "2026-10-18T11:57:00Z".into()), outcome }];
    RunEntry { id: id.into(), what: "返信の下書きを作る".into(), app: "Gmail".into(), at: "2026-10-18T11:57:00Z".into(), status, effect, target: target.into(), undo: Undo::conservative(effect), attempts, connect: true, project: None, stopping: false, started_by: String::new() }
}

/// 10 × 6 の全部（id は内部の id らしい形）。
fn every_pair() -> Vec<RunEntry> {
    let mut v = Vec::new();
    for (i, s) in STATUSES.iter().enumerate() {
        for (j, o) in outcomes().into_iter().enumerate() {
            v.push(entry(&format!("run_{i}{j}_7f3a9c"), *s, o, Effect::ExternalSend, "チームの channel"));
        }
    }
    v
}

struct Pass {
    out: FullOutput,
    action: Option<RunsAction>,
}

fn run(c: &Context, t: f64, view: &RunsView, events: Vec<egui::Event>) -> Pass {
    let mut action = None;
    let input = RawInput { screen_rect: Some(SCREEN), time: Some(t), events, ..Default::default() };
    let out = c.run(input, |ctx| {
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| action = runs::show(ui, view));
    });
    Pass { out, action }
}

fn tree(p: &Pass) -> &accesskit::TreeUpdate {
    p.out.platform_output.accesskit_update.as_ref().expect("accesskit を有効にした pass は木を出す")
}

fn button<'a>(t: &'a accesskit::TreeUpdate, label: &str) -> Option<&'a accesskit::Node> {
    t.nodes.iter().map(|(_, n)| n).find(|n| n.role() == accesskit::Role::Button && n.label() == Some(label))
}

fn center(n: &accesskit::Node) -> Pos2 {
    let b = n.bounds().expect("bounds");
    Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

/// `label` の button を押す（乗せる → 押す → 離す）。離した pass を返す。
fn click(c: &Context, t: f64, view: &RunsView, label: &str) -> Pass {
    let p = run(c, t, view, vec![]);
    let pos = center(button(tree(&p), label).unwrap_or_else(|| panic!("button「{label}」が無い")));
    let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    run(c, t + 0.02, view, vec![egui::Event::PointerMoved(pos)]);
    run(c, t + 0.04, view, vec![press(true)]);
    run(c, t + 0.06, view, vec![press(false)])
}

fn row_label(e: &RunEntry) -> String {
    format!("{} · {}", e.what, e.result().wording())
}

/// 1 件だけの一覧で、その行を押して詳細を開いた pass（reduce motion・AX 付き）。
fn opened(e: &RunEntry) -> (Context, Pass) {
    let c = ctx(true);
    c.enable_accesskit();
    let list = [e.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    click(&c, 1.0, &view, &row_label(e));
    let p = run(&c, 1.2, &view, vec![]);
    (c, p)
}

fn all_text(p: &Pass) -> Vec<String> {
    let mut v: Vec<String> = texts(&p.out).into_iter().map(|(t, _, _)| t).collect();
    if let Some(t) = &p.out.platform_output.accesskit_update {
        for (_, n) in &t.nodes {
            v.extend(n.label().map(String::from));
            v.extend(n.value().map(String::from));
        }
    }
    v
}

/// FX2（FX-007・caller の形・本体の前は todo!() で赤）: `open_run` の次の `show` は、その run の行を押したのと同じ開いた形。一覧に無い id は何もしない。
#[test]
fn fx2_open_run_opens_that_row_like_a_press() {
    let sorted = |p: &Pass| {
        let mut v = all_text(p);
        v.sort();
        v
    };
    let e = entry("run_gen_1", RunStatus::Cancelled, None, Effect::LocalWrite, "");
    let (_, pressed) = opened(&e);
    let c = ctx(true);
    c.enable_accesskit();
    let list = [e.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    let closed = sorted(&run(&c, 1.0, &view, vec![]));
    assert_ne!(closed, sorted(&pressed), "閉じた形と開いた形の字が同じ（この試験が何も見ていない）");
    runs::open_run(&c, "no-such-run");
    assert_eq!(sorted(&run(&c, 1.1, &view, vec![])), closed, "一覧に無い id は何もしない");
    runs::open_run(&c, "run_gen_1");
    run(&c, 1.2, &view, vec![]);
    assert_eq!(sorted(&run(&c, 1.4, &view, vec![])), sorted(&pressed), "行を押したのと同じに開く");
}

#[test]
fn every_pair_has_a_word_and_zero_or_one_next_step() {
    for e in every_pair() {
        let r = e.result();
        assert!(!r.wording().is_empty());
        // 一手は押して効く物だけ（止める = runtime の cancel・接続する = Connections）。承認・照合・やり直しの先はまだ無いので出さない
        let stoppable = matches!(r, RunResult::Waiting | RunResult::Planning | RunResult::Running | RunResult::NeedsApproval);
        assert_eq!(e.next_step() == Some(RunAction::Cancel), stoppable, "{:?} × {:?} → {r:?}", e.status, e.outcome());
        assert!(matches!(e.next_step(), None | Some(RunAction::Cancel | RunAction::Connect | RunAction::CheckAgain)), "{r:?}");
    }
    // 入ったか分からない → 記録の読み直し（FX7・送り直す一手は出さない）・承認待ち → 止める（待ったまま出口が無い、にしない）
    let unknown = entry("a", RunStatus::Uncertain, Some(Outcome::EffectUnknown { reason: "届いたか分からない".into() }), Effect::ExternalSend, "x");
    assert_eq!((unknown.result(), unknown.next_step()), (RunResult::Unknown, Some(RunAction::CheckAgain)));
    let approval = entry("b", RunStatus::WaitingApproval, None, Effect::ExternalSend, "x");
    assert_eq!((approval.result(), approval.next_step()), (RunResult::NeedsApproval, Some(RunAction::Cancel)));
    let denied = Outcome::Failed { error: ErrorCode::ApprovalDenied };
    assert_eq!(next_step(RunResult::Failed, Some(&denied)), None, "人が承認しなかった物に一手を出さない");
}

/// 10 × 6 の全部で、行と開いた詳細の文字・AX の名前に宣伝文が 0。語の表（結果・一手・失敗・取り消し）も名前として通す。
#[test]
fn no_catch_copy_in_any_pair() {
    let r = rules();
    let mut names: Vec<&str> = [
        RunResult::Waiting,
        RunResult::Planning,
        RunResult::NeedsConnection,
        RunResult::NeedsApproval,
        RunResult::Running,
        RunResult::Verifying,
        RunResult::Verified,
        RunResult::Unverified,
        RunResult::Rejected,
        RunResult::Failed,
        RunResult::Unknown,
        RunResult::Cancelled,
    ]
    .iter()
    .map(|r| r.wording())
    .collect();
    names.extend([RunAction::Cancel, RunAction::Connect, RunAction::CheckAgain, RunAction::OpenProject].map(RunAction::label));
    names.extend([Undo::NotNeeded, Undo::InApp, Undo::InSource, Undo::Never, Undo::Unknown].map(Undo::wording));
    use ErrorCode as E;
    for e in [E::NotReady, E::PermissionDenied, E::ApprovalDenied, E::InvalidInput, E::ConnectionLost, E::Timeout, E::RateLimited, E::TargetNotFound, E::TargetAmbiguous, E::DriverRefused, E::VerificationFailed, E::Cancelled, E::Internal] {
        names.push(runs::error_word(e));
    }
    for n in names {
        assert!(judge(&r, n, true).is_empty(), "{n}: {:?}", judge(&r, n, true));
    }
    for e in every_pair() {
        let (_, p) = opened(&e);
        let shown = all_text(&p);
        assert!(shown.iter().any(|t| t.contains("What was checked") || t == "What"), "{:?}: 詳細が開いていない", e.status);
        for t in shown {
            assert!(judge(&r, &t, false).is_empty(), "{:?} × {:?}: {t} {:?}", e.status, e.outcome(), judge(&r, &t, false));
        }
    }
}

/// 開いた詳細の button は 行の 1 つ + 次の一手 0 / 1（名前は一手の語）。押すとその run の id と一手が返る。
#[test]
fn detail_has_exactly_its_next_step() {
    for e in every_pair() {
        let (c, p) = opened(&e);
        let t = tree(&p);
        // RQ1: `Copy details` は次の一手とは別に必ず 1 つ（次の一手の数には入れない）
        let copy = t.nodes.iter().filter(|(_, n)| n.role() == accesskit::Role::Button && n.label() == Some("Copy details")).count();
        assert_eq!(copy, 1, "{:?} × {:?}: Copy details が 1 つでない", e.status, e.outcome());
        let others: Vec<&str> = t.nodes.iter().map(|(_, n)| n).filter(|n| n.role() == accesskit::Role::Button && n.label() != Some(row_label(&e).as_str()) && n.label() != Some("Filter") && n.label() != Some("Copy details")).filter_map(|n| n.label()).collect();
        match e.next_step() {
            None => assert!(others.is_empty(), "{:?} × {:?}: 一手が無いはずが {others:?}", e.status, e.outcome()),
            Some(step) => {
                assert_eq!(others, [step.label()], "{:?} × {:?}", e.status, e.outcome());
                let list = [e.clone()];
                let view = RunsView { entries: &list, loading: false, now_unix: NOW };
                let got = click(&c, 2.0, &view, step.label()).action;
                assert_eq!(got, Some(RunsAction { run_id: e.id.clone(), action: step }));
            }
        }
    }
}

/// run の id・試行の番号・証拠の id / URL・reason の debug 名を、描いた文字にも AX にも出さない。
#[test]
fn no_internal_ids_on_screen() {
    let banned = ["run_", "7f3a9c", "r-1234567890", "rec_42", "eid=", "https://", "drafts.get", "Timeout", "seq", "handler", "service"];
    for e in every_pair() {
        let (_, p) = opened(&e);
        for t in all_text(&p) {
            assert!(!banned.iter().any(|b| t.contains(b)), "{:?} × {:?}: 内部の語が出た: {t}", e.status, e.outcome());
        }
    }
    let (_, p) = opened(&every_pair()[36]); // verified × Verified
    let shown = all_text(&p);
    assert!(shown.iter().any(|t| t == "宛先・件名・本文が一致（送信していない）"), "読み戻した証拠の中身は出す: {shown:?}");
}

/// 根拠は結果の語と食い違わない（status と outcome がずれた組でも）。reason の英語混じりは結果の語の表の 1 行に替える。
#[test]
fn evidence_matches_the_result() {
    for e in every_pair() {
        let (r, ev) = (e.result(), e.evidence());
        let ctx = format!("{:?} × {:?} → {:?}: {ev:?}", e.status, e.outcome(), r);
        if r != RunResult::Verified {
            assert!(!ev.iter().any(|l| l.contains("一致") || l.contains("matched")), "確かめていないのに一致と言った: {ctx}");
        }
        match r {
            RunResult::Unknown if e.outcome().is_some() => assert_eq!(ev, vec!["Can't tell if it arrived"], "{ctx}"),
            RunResult::Rejected => assert_eq!(ev, vec!["The app didn't accept it"], "{ctx}"),
            RunResult::Failed => assert!(ev.iter().all(|l| l == "Connection lost"), "{ctx}"),
            RunResult::Cancelled => assert!(ev.is_empty(), "{ctx}"),
            _ => {}
        }
    }
}

/// 外に効いた run（送る・書く・消す）は相手（空なら未記録）と取り消せるかを必ず出す。読んだだけの run には出さない。
#[test]
fn external_runs_show_target_and_undo() {
    let done = || Some(Outcome::Verified { evidence: vec!["送った本文を読み戻して一致".into()] });
    let cases = [
        (Effect::ExternalSend, "", Some("Not recorded"), Some("Can't be undone")),
        (Effect::Destructive, "予定「定例」", Some("予定「定例」"), Some("Can't be undone")),
        (Effect::ExternalWrite, "REAPER の track", Some("REAPER の track"), Some("Undo it in the app")),
        (Effect::LocalRead, "", None, None),
    ];
    for (effect, target, want_target, want_undo) in cases {
        let e = entry("run_x", RunStatus::Verified, done(), effect, target);
        let (_, p) = opened(&e);
        let shown = all_text(&p);
        let has = |s: &str| shown.iter().any(|t| t == s);
        assert_eq!(has("With"), want_target.is_some(), "{effect:?}: 相手の行");
        assert_eq!(has("Undo"), want_undo.is_some(), "{effect:?}: 取り消しの行");
        for v in want_target.into_iter().chain(want_undo) {
            assert!(has(v), "{effect:?}: 「{v}」が無い（{shown:?}）");
        }
    }
}

/// 行は名前の付いた button（何をしたか · 結果）・詳細は名前付きの Group・値は選べる label（cua の elements に出る）。
#[test]
fn ax_names_for_rows_and_details() {
    use accesskit::{Action, Role};
    let e = &every_pair()[36];
    let (_, p) = opened(e);
    let t = tree(&p);
    assert!(button(t, &row_label(e)).is_some(), "行の button「{}」が無い", row_label(e));
    let group = format!("Details of {}", e.what);
    assert!(t.nodes.iter().any(|(_, n)| n.role() == Role::Group && n.label() == Some(group.as_str())), "詳細の名前が無い");
    for v in ["返信の下書きを作る", "チームの channel", "Verified", "Can't be undone"] {
        assert!(t.nodes.iter().any(|(_, n)| n.role() == Role::Label && n.value() == Some(v) && n.supports_action(Action::Click)), "値「{v}」が選べる label に無い");
    }
}

/// 行を押すと下に詳細が開き、もう一度押すと閉じる。
#[test]
fn row_toggles_its_detail() {
    let c = ctx(true);
    c.enable_accesskit();
    let list = [entry("run_a", RunStatus::WaitingApproval, None, Effect::ExternalSend, "山田さん")];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    let has_detail = |p: &Pass| all_text(p).iter().any(|t| t == "What");
    assert!(!has_detail(&run(&c, 1.0, &view, vec![])), "押す前から開いている");
    assert!(has_detail(&click(&c, 1.1, &view, &row_label(&list[0]))) || has_detail(&run(&c, 1.3, &view, vec![])), "押しても開かない");
    click(&c, 1.5, &view, &row_label(&list[0]));
    assert!(!has_detail(&run(&c, 1.8, &view, vec![])), "もう一度押しても閉じない");
}

/// 一覧の行: 何をしたか · app · 何分前 · 結果の 1 語。空なら事実 1 行。
#[test]
fn list_rows_say_what_when_and_result() {
    let c = ctx(true);
    let list = [entry("run_a", RunStatus::Uncertain, Some(Outcome::EffectUnknown { reason: "届いたか分からない".into() }), Effect::ExternalSend, "x")];
    let shown: Vec<String> = texts(&run(&c, 1.0, &RunsView { entries: &list, loading: false, now_unix: NOW }, vec![]).out).into_iter().map(|t| t.0).collect();
    for want in ["返信の下書きを作る", "Gmail · 3 min ago", "Sent, not confirmed"] {
        assert!(shown.iter().any(|t| t == want), "「{want}」が無い（{shown:?}）");
    }
    let c = ctx(true);
    let empty: Vec<String> = texts(&run(&c, 1.0, &RunsView { entries: &[], loading: false, now_unix: NOW }, vec![]).out).into_iter().map(|t| t.0).collect();
    // 35.B の B3: Runs の空は表の `No runs yet`（FINAL:391・rail の History と同じ語・`Nothing here yet` と 2 語にしない）
    assert!(empty.iter().any(|t| t.contains("No runs yet")), "{empty:?}");
    assert!(!empty.iter().any(|t| t.contains("Nothing here yet")), "{empty:?}");
}

/// `area` の中の骨格の帯（1px の区切り線は数えない）と光の mesh。
fn bands(out: &FullOutput, area: Rect) -> (usize, usize) {
    use egui::{Color32, Shape};
    let mut n = (0, 0);
    for s in shapes(out) {
        match s {
            Shape::Rect(r) if r.fill != Color32::TRANSPARENT && r.rect.height() > ui::theme::HAIRLINE + 0.5 && area.intersects(r.rect) => n.0 += 1,
            Shape::Circle(c) if c.fill != Color32::TRANSPARENT && area.contains(c.center) => n.0 += 1,
            Shape::Mesh(m) if area.intersects(m.calc_bounds()) => n.1 += 1,
            _ => {}
        }
    }
    n
}

/// 見出しの下（骨格・行）の領域。
/// 検索の欄と `Filter`（FX-040）の下から。欄の背景は行の背景ではない。
fn below_filter(p: &Pass) -> Rect {
    let bar = texts(&p.out).into_iter().find(|(t, _, _)| t == "Filter").map_or_else(|| body(p).top(), |t| t.1.bottom() + 8.0);
    Rect::from_min_max(Pos2::new(SCREEN.left(), bar), SCREEN.max)
}

fn body(p: &Pass) -> Rect {
    let head = texts(&p.out).into_iter().find(|(t, _, _)| t == "Runs").expect("見出し").1;
    Rect::from_min_max(Pos2::new(SCREEN.left(), head.bottom() + 2.0), SCREEN.max)
}

/// 読み込み中: 120ms 未満は描かない → それ以降は行の骨格と光（文字 0）→ 読み込んだら骨格から行へ cross-fade。
#[test]
fn loading_shows_row_skeleton_then_cross_fades() {
    let c = ctx(false);
    let list = every_pair()[30..33].to_vec();
    let loading = RunsView { entries: &[], loading: true, now_unix: NOW };
    let first = run(&c, 1.0, &loading, vec![]);
    assert_eq!(bands(&first.out, body(&first)), (0, 0), "読み込みを始めてすぐ骨格が出た");
    let sk = run(&c, SHINE_MID, &loading, vec![]);
    let (rects, meshes) = bands(&sk.out, body(&sk));
    assert!(rects >= 6 && meshes >= 1, "骨格の帯 {rects}・光 {meshes}");
    assert!(texts(&sk.out).iter().all(|(_, r, _)| !body(&sk).intersects(*r)), "骨格に文字");
    let loaded = RunsView { entries: &list, loading: false, now_unix: NOW };
    run(&c, SHINE_MID + 0.01, &loaded, vec![]);
    let mid = run(&c, SHINE_MID + 0.12, &loaded, vec![]);
    let end = run(&c, SHINE_MID + 0.5, &loaded, vec![]);
    assert!(bands(&mid.out, body(&mid)).0 >= 1 && texts(&mid.out).iter().any(|t| t.0 == "返信の下書きを作る"), "fade の途中に骨格と行が両方ない");
    // 行の背景（hover・選択）は無いので、残る色付きの rect は行の区切り線だけ
    assert_eq!(bands(&end.out, below_filter(&end)).0, 0, "fade の後も骨格が残る");
}

/// reduce motion: 光を流さず、読み込んだ frame で即時（骨格を重ねない）。
#[test]
fn reduce_motion_is_immediate() {
    let c = ctx(true);
    let loading = RunsView { entries: &[], loading: true, now_unix: NOW };
    run(&c, 1.0, &loading, vec![]);
    let sk = run(&c, SHINE_MID, &loading, vec![]);
    assert!(bands(&sk.out, body(&sk)).0 >= 6);
    assert_eq!(bands(&sk.out, body(&sk)).1, 0, "reduce motion で光を流した");
    let list = every_pair()[30..33].to_vec();
    let p = run(&c, SHINE_MID + 0.01, &RunsView { entries: &list, loading: false, now_unix: NOW }, vec![]);
    assert_eq!(bands(&p.out, below_filter(&p)).0, 0, "reduce motion で骨格を重ねた");
}

/// 同じ run の状態が替わると結果の色を前の色から寄せる。
#[test]
fn result_color_cross_fades_when_the_status_changes() {
    let c = ctx(false);
    let t = ui::theme::LIGHT;
    let running = [entry("run_a", RunStatus::Running, None, Effect::ExternalSend, "x")];
    let verified = [entry("run_a", RunStatus::Verified, Some(Outcome::Verified { evidence: vec![] }), Effect::ExternalSend, "x")];
    run(&c, 1.0, &RunsView { entries: &running, loading: false, now_unix: NOW }, vec![]);
    run(&c, 2.0, &RunsView { entries: &verified, loading: false, now_unix: NOW }, vec![]);
    let color = |p: &Pass| texts(&p.out).into_iter().find(|x| x.0 == "Verified").map(|x| x.2).expect("結果の語");
    let mid = run(&c, 2.11, &RunsView { entries: &verified, loading: false, now_unix: NOW }, vec![]);
    let end = run(&c, 2.5, &RunsView { entries: &verified, loading: false, now_unix: NOW }, vec![]);
    let (from, to) = (RunResult::Running.region().color(&t), RunResult::Verified.region().color(&t));
    assert!(color(&mid) != from && color(&mid) != to, "色が寄っていない");
    assert_eq!(color(&end), to);
}

/// G94 ②: `Stop` の結果の 1 行は押した行の詳細の下に出る（他の行の詳細には出ない・行を閉じると消える）。
/// 負の対照: `show` の `if let Some(line) = ctx.data(..note_id..)` の描画を外す → 赤。
#[test]
fn g94_stop_result_is_shown_under_the_pressed_row() {
    let e = entry("run_stop_1", RunStatus::Running, None, Effect::ExternalSend, "チームの channel");
    let (c, before) = opened(&e);
    let line = "Stopped before anything changed.";
    assert!(!all_text(&before).iter().any(|t| t.contains(line)), "押す前に結果");
    runs::note(&c, &e.id, line);
    let list = [e.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    let after = run(&c, 1.4, &view, vec![]);
    assert!(all_text(&after).iter().any(|t| t.contains(line)), "押した行の下に結果が出ない");
    runs::note(&c, "some_other_run", "Too late to stop.");
    assert!(!all_text(&run(&c, 1.5, &view, vec![])).iter().any(|t| t.contains("Too late to stop.")), "他の行の結果が出た");
    click(&c, 2.0, &view, &row_label(&e));
    assert!(!all_text(&run(&c, 2.2, &view, vec![])).iter().any(|t| t.contains(line)), "閉じても結果が残る");
}

/// FX7（FX-037）: 属する Project を持つ行は、他に次の一手が無い時 `Open project` が 1 つの一手（押すと run の id と一手が返る）。
/// 走っている行は `Stop`・送ったか分からない行は `Check again` のまま（次の一手は 0 か 1）。project の無い行には出さない。
/// 負の対照（commit message）: `RunEntry::next_step` の `or_else` を外す → 1 つ目の assert が赤。
#[test]
fn fx7_open_project_is_the_one_step_of_a_row_that_has_a_project() {
    let mut e = entry("a", RunStatus::Verified, Some(Outcome::Verified { evidence: vec![] }), Effect::LocalWrite, "x");
    assert_eq!(e.next_step(), None, "project の無い行に Open project");
    e.project = Some("project.x".into());
    assert_eq!(e.next_step(), Some(RunAction::OpenProject));
    let (c, _) = opened(&e);
    let list = [e.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    assert_eq!(click(&c, 2.0, &view, "Open project").action, Some(RunsAction { run_id: e.id.clone(), action: RunAction::OpenProject }));
    let mut running = entry("b", RunStatus::Running, None, Effect::LocalWrite, "x");
    running.project = Some("project.x".into());
    assert_eq!(running.next_step(), Some(RunAction::Cancel));
    let mut unknown = entry("c", RunStatus::Uncertain, Some(Outcome::EffectUnknown { reason: "x".into() }), Effect::ExternalSend, "x");
    unknown.project = Some("project.x".into());
    assert_eq!(unknown.next_step(), Some(RunAction::CheckAgain));
}

/// G102 FX-102: 英語の reason・証拠は 1 文目を出す。内部の語・id・URL・Debug の混じる物は出さず、結果の語の表の 1 行に替える。
#[test]
fn g102_plain_english_reasons_show_and_internal_ones_dont() {
    let shown = |o: Outcome, s: RunStatus| entry("a", s, Some(o), Effect::ExternalSend, "x").evidence();
    let unknown = |r: &str| shown(Outcome::EffectUnknown { reason: r.into() }, RunStatus::Uncertain);
    assert_eq!(unknown("Discord closed before the reply came back. It won't be sent again"), vec!["Discord closed before the reply came back"]);
    for bad in [
        "The handler crashed",
        "Couldn't read the window (Timeout)",
        "Stale revision 3, now 4",
        "Err(Io { kind: NotFound })",
        "lease_expired",
        "See https://example.com/x for details",
    ] {
        assert_eq!(unknown(bad), vec!["Can't tell if it arrived"], "内部の語を出した: {bad}");
    }
    let verified = shown(Outcome::Verified { evidence: vec!["drafts.get r-1234567890: To, subject and body matched (not sent)".into(), "rec_42".into(), "https://calendar.google.com/event?eid=abc".into()] }, RunStatus::Verified);
    assert_eq!(verified, vec!["To, subject and body matched (not sent)"]);
}

/// FX7（FX-039）: 止めるよう頼んだ走っている run は `Stopping`（`Stop` はもう押せない・まだ終わっていない物として根拠を出さない）。止まった（Cancelled）run は `Undone` のまま。
/// 負の対照（commit message）: `RunEntry::result` の `stopping` の腕を外す → 赤。
#[test]
fn fx7_a_run_asked_to_stop_reads_stopping_and_offers_no_second_stop() {
    let mut e = entry("a", RunStatus::Running, None, Effect::LocalWrite, "x");
    assert_eq!((e.result(), e.next_step()), (RunResult::Running, Some(RunAction::Cancel)));
    e.stopping = true;
    assert_eq!((e.result(), e.result().wording(), e.next_step()), (RunResult::Stopping, "Stopping", None), "止めると頼んだ run に 2 本目の Stop");
    assert!(e.result().in_progress() && e.evidence().is_empty());
    let done = RunEntry { status: RunStatus::Cancelled, ..e };
    assert_eq!(done.result(), RunResult::Cancelled, "止まった run は Stopping のまま");
}

/// FX7（FX-040）: 開いた詳細は「Effect」の行で、この手が何をするか（外へ送る・この Mac を変える…）を言う。6 つの effect 全部に語がある。
/// 負の対照（commit message）: 詳細の `Effect` の行を外す → 赤。
#[test]
fn fx7_detail_says_what_kind_of_step_it_was() {
    for (effect, word) in [(Effect::ExternalSend, "Sends outside"), (Effect::LocalWrite, "Changes something on this Mac"), (Effect::LocalRead, "Reads on this Mac")] {
        let e = entry("a", RunStatus::Verified, Some(Outcome::Verified { evidence: vec![] }), effect, "x");
        let (_, p) = opened(&e);
        assert!(all_text(&p).iter().any(|t| t == word), "{effect:?}: 「{word}」が詳細に無い");
    }
    for effect in [Effect::LocalRead, Effect::LocalWrite, Effect::ExternalRead, Effect::ExternalWrite, Effect::ExternalSend, Effect::Destructive] {
        assert!(!runs::effect_word(effect).is_empty());
    }
}

/// FX7（FX-040）: 開いた詳細は「Started by」の行で起点（誰が始めた手か）を言う。記録に無ければ行を出さない。
/// 負の対照（commit message）: 詳細の `Started by` の行を外す → 赤。
#[test]
fn fx7_detail_says_who_started_it_when_known() {
    let mut e = entry("a", RunStatus::Verified, Some(Outcome::Verified { evidence: vec![] }), Effect::LocalWrite, "x");
    let (_, p) = opened(&e);
    assert!(!all_text(&p).iter().any(|t| t == "Started by"), "起点が空なのに行を出した");
    e.started_by = "You pressed a button".into();
    let (_, p) = opened(&e);
    assert!(all_text(&p).iter().any(|t| t == "You pressed a button"), "起点の行が詳細に無い");
}

/// FX7（FX-040）: Runs の絞り込み。閉じている間は検索の欄と `Filter` の button だけで、状態の語の項目は AX に無い（rehearse の seen は role なしの部分一致）。
/// 語で絞ると `Showing {n} of {m}` と `Clear`・Clear で戻る。`Filter` で状態を絞ると合う行だけ。
/// 同じ ctx の 2 pass 目以降は `accesskit_update` が None になるので、Clear は絞った pass の文字の中心を pointer で押す（`click` の `tree()` は使わない）。
/// 負の対照（commit message）: `RunFilter::matches` の状態の腕を常に true にする → 赤。Clear の press を外すと `Showing` が残って赤。
#[test]
fn fx7_runs_filter_closed_shows_only_search_and_filter_and_narrows_with_a_way_back() {
    let c = ctx(true);
    let ok = |id: &str| entry(id, RunStatus::Verified, Some(Outcome::Verified { evidence: vec![] }), Effect::LocalWrite, "x");
    let unknown = entry("u", RunStatus::Uncertain, Some(Outcome::EffectUnknown { reason: "x".into() }), Effect::ExternalSend, "x");
    let list = [ok("a"), unknown.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    let texts = |p: &Pass| all_text(p);
    let first = run(&c, 1.0, &view, vec![]);
    assert!(texts(&first).iter().any(|t| t == "Filter") || first.out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).any(|(_, n)| n.label() == Some("Filter")), "Filter の button が無い");
    let ax_labels = |p: &Pass| p.out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).filter_map(|(_, n)| n.label().map(str::to_owned)).collect::<Vec<_>>();
    assert!(!ax_labels(&first).iter().any(|l| l.starts_with("Show ")), "閉じている間に絞り込みの項目が AX に在る: {:?}", ax_labels(&first));
    assert!(!ax_labels(&first).iter().any(|l| l.starts_with("Showing")), "絞っていないのに Showing の行");
    runs::set_filter(&c, "", runs::RunFilter::NotConfirmed);
    let narrowed = run(&c, 1.5, &view, vec![]);
    let seen = |p: &Pass| texts(p).into_iter().chain(ax_labels(p)).collect::<Vec<_>>();
    assert!(seen(&narrowed).iter().any(|l| l == "Showing 1 of 2"), "絞った件数の行が無い: {:?}", seen(&narrowed));
    assert!(seen(&narrowed).iter().any(|l| l == "Clear"), "戻り道が無い");
    assert!(!ax_labels(&narrowed).iter().any(|l| l.starts_with(&row_label(&ok("a")))), "合わない行が残る");
    let clear_at = common::texts(&narrowed.out).into_iter().find(|(s, _, _)| s == "Clear").map(|(_, r, _)| r.center()).expect("Clear の文字の位置が無い");
    let press = |pressed| egui::Event::PointerButton { pos: clear_at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    run(&c, 2.02, &view, vec![egui::Event::PointerMoved(clear_at)]);
    run(&c, 2.04, &view, vec![press(true)]);
    run(&c, 2.06, &view, vec![press(false)]);
    let after = run(&c, 2.5, &view, vec![]);
    assert!(!seen(&after).iter().any(|l| l.starts_with("Showing")), "Clear しても絞りが残る: {:?}", seen(&after));
}

/// RQ1（N11）: 詳細の `Copy details` を押すと、その 1 件の `details_text` が clipboard に入り（相手の path は伏せる）、行の下に `Details copied`。
/// 次の一手（RunsAction）は返さない = 押しても run には何もしない。負の対照: `detail` の `copy_text` を外す → 赤。
#[test]
fn rq1_copy_details_copies_the_masked_detail() {
    let e = entry("run_copy_1", RunStatus::Failed, Some(Outcome::Failed { error: ErrorCode::Timeout }), Effect::ExternalWrite, "/Users/you/Music/take.wav");
    let (c, _) = opened(&e);
    let list = [e.clone()];
    let view = RunsView { entries: &list, loading: false, now_unix: NOW };
    let p = click(&c, 2.0, &view, "Copy details");
    assert_eq!(p.action, None, "Copy details が run の一手を返した");
    let copied: Vec<&String> = p.out.platform_output.commands.iter().filter_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t),
        _ => None,
    }).collect();
    assert_eq!(copied, [&e.details_text()], "押した 1 件の詳細が copy されない");
    assert!(copied[0].contains("With: [path]") && !copied[0].contains("/Users"), "path が伏せられていない: {}", copied[0]);
    assert!(all_text(&run(&c, 2.2, &view, vec![])).iter().any(|t| t.contains("Details copied")), "押した後の 1 行が無い");
}
