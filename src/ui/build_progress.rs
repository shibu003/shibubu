//! DESIGN §16.4 の生成中の画面（一言目 → 生成 → 評価 → 直す の間と、その結果）。図は docs/diagrams.md §24。
//! - 生成中: 上端に状態 1 行（読む・調べる・組む・確かめる・直す の今どれか）とキャンセル、下に作られる app の形の骨格
//!   （予定の component ごとの `skeleton::region_skeleton`）。120ms 未満なら骨格を描かない
//! - 通った: 骨格から app へ cross-fade（この画面は行も button も足さない）
//! - 通らない: 最良案の上に状態 1 行・残った理由 1〜3 行（人の言葉）・次の一手 1 つ。最良案が無ければそれだけ
//! - 周回の数字・loss・経過時間は持たない（入力の struct に field が無い = 描けない）。経路の説明は rail の「このアプリについて」へ `about_rows`
//! - 入力は refine の `--json`（p-engine `src/refine/mod.rs` の `Outcome`・未 merge）を写した最小の形。refine が main に入ったら統合役が寄せる
//! - FX2（図 §45）: Home の生成（`Generated::model_progress`）も同じ部品で出す = 欄の下の 1 行の段・待ちの理由・経過・`Stop`（`Build::Running`）、
//!   止めている間（`Build::Stopping`・押せる手 0）、終わった 1 行と次の一手 1 つ（`Build::Ended`）。経過は生成の job の時計（周回の数字・loss・% は今も持たない）

use super::motion;
use super::skeleton::{self, Skeleton};
use super::states::{self, first_sentence, has_ja, RegionState};
use super::theme::{self, Type, Variant};
use eframe::egui::{self, accesskit, Id, Ui, Vec2};
use serde::Deserialize;
use std::hash::Hash;
use std::time::Duration;

/// 生成の今の段（§16.4 の状態 1 行）。
/// FX2: Home の生成（model の job）が出すのは `Read` だけ（`installed_apps` と CLI の呼び出しの間・`save_plan` は UI の thread で同じ frame に終わる）。
/// `Investigate`・`Compose`・`Verify`・`Fix` は refine の段で、生成の job は出さない（見えない・起きていない段を出さない = `Generated::model_progress`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 一言目から検査の項目（target）を作る
    Read,
    /// Integration Planner（在る software と経路を調べる）
    Investigate,
    /// composer が app を組む
    Compose,
    /// 評価（検査・空打ち・§15 の規則）
    Verify,
    /// 2 周目からの直し
    Fix,
}

impl Phase {
    pub const ALL: [Phase; 5] = [Phase::Read, Phase::Investigate, Phase::Compose, Phase::Verify, Phase::Fix];

    pub const fn wording(self) -> &'static str {
        match self {
            Phase::Read => "Reading your request",
            Phase::Investigate => "Checking what's on this Mac",
            Phase::Compose => "Putting the pages together",
            Phase::Verify => "Running the checks",
            Phase::Fix => "Fixing failed checks",
        }
    }
}

/// refine の止まった理由（`refine::Stop` と同じ 4 つ・JSON の語も同じ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    Passed,
    NoImprovement,
    Deadline,
    NotBuilt,
}

impl Stop {
    /// 通らなかった時の状態 1 行。
    pub const fn wording(self) -> &'static str {
        match self {
            Stop::Passed => "Checks passed",
            Stop::NoImprovement => "Some checks still fail",
            Stop::Deadline => "Didn't pass in time",
            Stop::NotBuilt => "Couldn't build it",
        }
    }

    /// refine が理由を 1 つも返さなかった時の事実 1 行（期限が最初の評価より先に来た時など）。
    pub const fn fallback_reason(self) -> &'static str {
        match self {
            Stop::Passed => "",
            Stop::NoImprovement => "Fixes didn't help",
            Stop::Deadline => "Ran out of time before checking",
            Stop::NotBuilt => "Couldn't tell what to build",
        }
    }

    pub const fn region(self) -> RegionState {
        match self {
            Stop::Passed => RegionState::Verified,
            Stop::NoImprovement | Stop::Deadline => RegionState::Partial,
            Stop::NotBuilt => RegionState::Error,
        }
    }
}

/// 操作 1 つの経路の決定（`refine::plan::Decision` から画面に要る物だけ）。
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Decision {
    pub software: String,
    pub action_id: String,
    #[serde(default)]
    pub chosen: Option<String>,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

/// 口の無かった操作（`refine::plan::Codegen`）。
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Codegen {
    pub software: String,
    pub operation: String,
    #[serde(default)]
    pub why: String,
}

/// refine の `--json` の最小の写し。`loss`・`trace`・`elapsed_ms`・`target` は持たない（serde が捨てる = 画面に出せない）。
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct RefineOutcome {
    pub passed: bool,
    pub stop: Stop,
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub codegen: Vec<Codegen>,
    /// 最良の app が在るか（中身は renderer が別に持つ）
    #[serde(default)]
    app: Option<serde::de::IgnoredAny>,
}

impl RefineOutcome {
    pub fn from_json(s: &str) -> Result<RefineOutcome, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    pub fn has_best_app(&self) -> bool {
        self.app.is_some()
    }
}

/// 次の一手（呼ぶ側が rail・Connections・prompt・元の app へ繋ぐ）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BuildAction {
    /// 生成中の `Stop`（FX2: Home の生成では `Intent::Build(Cancel)` → `Generated::stop_model_job`）
    Cancel,
    /// その software の接続（OAuth など）を開く
    AllowConnection(String),
    /// その software を開いて人が続ける
    OpenInSource(String),
    /// prompt に戻って言い直す
    Rephrase,
    Retry,
    /// FX2（FX-006）: AI の失敗の後、利用者が選んで名前の表（`compose_project` → `create`）で作る。作った後の 1 行が AI を使わなかった事を言う
    /// （表 copy.en.md の Home「Built … with simple rules」）。処理（app.rs）: 欄の今の文で `create_now`。
    /// Wave: FX2
    UseSimpleRules,
    /// FX2（FX-007）: 生成中・終わった 1 行そのもの（文の部分）を押すと、Runs のその生成の run を開いた所へ（`Generated::model_run_id` → `ui::runs::open_run` → `Go(View::Runs)`）。
    /// 次の一手（button）とは別の手で、`action()` は返さない（1 行の次の一手は 1 つのまま）。
    /// Wave: FX2
    OpenRun,
}

impl BuildAction {
    pub const fn label(&self) -> &'static str {
        match self {
            BuildAction::Cancel => "Stop",
            BuildAction::AllowConnection(_) => "Allow this connection",
            BuildAction::OpenInSource(_) => "Open in the original app",
            BuildAction::Rephrase => "Say it another way",
            BuildAction::Retry => "Try again",
            BuildAction::UseSimpleRules => "Use simple rules",
            // 表 copy.en.md の `Open Runs`（Runs へ移る他の 1 行と同じ語）
            BuildAction::OpenRun => "Open Runs",
        }
    }

    pub const fn variant(&self) -> Variant {
        match self {
            BuildAction::Cancel | BuildAction::OpenRun => Variant::Ghost,
            _ => Variant::Primary,
        }
    }
}

/// 通らなかった時の次の一手（通った時は None = app だけ）。順は図 §24 の NS。
pub fn next_step(o: &RefineOutcome) -> Option<BuildAction> {
    if o.passed {
        return None;
    }
    if o.stop == Stop::NotBuilt {
        return Some(BuildAction::Rephrase);
    }
    let connection = |n: &String| n.starts_with("oauth_scope:") || n.starts_with("entitlement:");
    if let Some(d) = o.decisions.iter().find(|d| d.chosen.is_some() && d.needs.iter().any(connection)) {
        return Some(BuildAction::AllowConnection(d.software.clone()));
    }
    if let Some(d) = o.decisions.iter().find(|d| d.chosen.is_none() && d.fallback.is_some()) {
        return Some(BuildAction::OpenInSource(d.software.clone()));
    }
    Some(if o.stop == Stop::Deadline { BuildAction::Retry } else { BuildAction::Rephrase })
}

/// refine の `Eval::reasons()`（p-engine src/refine/mod.rs の `reasons`・未 merge）の接頭辞 → 人の言葉。
/// (接頭辞, 置く語, 後ろの中身を残すか)。置く語が空なら接頭辞を落とすだけ。
/// 接頭辞は表（docs/ui-copy/additions.md の `検査に落ちる: → Fails the checks:` ほか 8 行）の EN（UI-1 holdout の直し: 英語が既定の製品で refine の理由は
/// 画面に出る〈当たらない理由は first_sentence のまま出る〉ので、refine は表の EN の接頭辞で出す。refine を merge する時の約束・sendback 28）。
const REASON_PREFIX: &[(&str, &str, bool)] = &[
    ("Fails the checks: ", "", true),
    // 「Marketing copy rules [id]: 文（部品）」の文は宣伝文そのものなので写さない
    ("Marketing copy rules ", "Leftover description text", false),
    ("No loading skeleton: ", "A part has no loading state", false),
    ("Crashes on empty input: ", "A test press failed", true),
    ("Unrequested effect: ", "An outward action you didn't ask for", true),
    ("Connection not named: ", "A connection you didn't ask for", true),
    ("Note: ", "", true),
    ("Part not requested: ", "A part you didn't ask for", true),
];
/// 理由は何行まで出すか（§16.4・人が読むのは 1〜3 行）。
pub const MAX_REASONS: usize = 3;

/// 中身が id（日本語を含まず . _ を含む）なら落とす。「Slack」のような名前は残す。
fn looks_like_id(s: &str) -> bool {
    !has_ja(s) && s.contains(['.', '_'])
}

/// refine の理由 1 つ → 人の言葉 1 行。
pub fn human_reason(raw: &str) -> String {
    let raw = raw.trim();
    for &(prefix, word, keep) in REASON_PREFIX {
        let Some(rest) = raw.strip_prefix(prefix) else { continue };
        let rest = first_sentence(rest);
        let rest = if keep && !rest.is_empty() && !looks_like_id(rest) { rest } else { "" };
        return match (word.is_empty(), rest.is_empty()) {
            (true, _) => rest.to_string(),
            (false, true) => word.to_string(),
            (false, false) => format!("{word}: {rest}"),
        };
    }
    first_sentence(raw).to_string()
}

/// 画面に出す理由 1〜3 行（人の言葉・重複なし）。refine が理由を返さなかった時は stop の事実 1 行。
pub fn reasons(o: &RefineOutcome) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in o.remaining.iter().map(|r| human_reason(r)) {
        if !r.is_empty() && !out.contains(&r) && out.len() < MAX_REASONS {
            out.push(r);
        }
    }
    if out.is_empty() {
        out.push(o.stop.fallback_reason().to_string());
    }
    out
}

/// 経路の id → 人の言葉（refine::plan の ROUTES・registry の schema の route の全部）。内部の id はそのまま返さない。
pub fn route_word(route: Option<&str>) -> &str {
    match route {
        None => "Not reachable",
        Some("api") => "API",
        Some("local") => "On this Mac",
        Some("mcp") => "MCP",
        Some("native_handoff") => "Open in the original app",
        Some("web") => "On the web",
        Some("native_docking") => "Beside the app",
        Some("mirror") => "Mirrored",
        Some("repo") => "Code from a repo, isolated",
        // 撤去した画面の操作の経路（W18-1・release の binary にその語を残さない＝W18-2）と知らない語は、内部の id を出さず 1 つの言葉にする
        Some(_) => "Other",
    }
}

/// fallback（「native_handoff（reaper.app）」の形）の経路の語。
fn fallback_word(f: &str) -> &str {
    route_word(Some(f.split('（').next().unwrap_or(f).trim()))
}

/// rail の「このアプリについて」の行（`AgentRailSource::about()` の形）。操作 1 つに 1 行: 項目 = software、値 = 経路 · 何をするか（· 代わり）。
/// decisions に無い codegen の候補も 1 行ずつ足す。`requires_connection` などの内部の語（why の 2 文目）は出さない。
pub fn about_rows(o: &RefineOutcome) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for d in &o.decisions {
        // why は「経路（口）: 何をするか。状態の語」の形。経路の語は route_word が出すので「: 」より後ろだけ
        let what = first_sentence(d.why.split_once(": ").map_or(d.why.as_str(), |(_, w)| w));
        let mut value = route_word(d.chosen.as_deref()).to_string();
        if !what.is_empty() {
            value.push_str(" · ");
            value.push_str(what);
        }
        if let Some(f) = &d.fallback {
            value.push_str(" · Instead: ");
            value.push_str(fallback_word(f));
        }
        rows.push((d.software.clone(), value));
    }
    for c in o.codegen.iter().filter(|c| !o.decisions.iter().any(|d| d.software == c.software && d.action_id == c.operation)) {
        let why = first_sentence(&c.why);
        rows.push((c.software.clone(), if why.is_empty() { route_word(None).to_string() } else { format!("{} · {why}", route_word(None)) }));
    }
    rows
}

/// 画面に渡す 1 frame ぶん。
/// FX2: Home の生成は `Generated::model_progress` が `Running` → `Stopping` → `Ended` を返し、app.rs が `home::draw` に渡す（`Done` は refine だけ）。
/// 例: `let b = Build::Running { phase: Phase::Read, planned: &[], waiting: None, elapsed: job.started.elapsed() }; build_progress::show(ui, "home", &b, |_| {})`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Build<'a> {
    /// 生成中。`planned` は予定の component（`domain::ui::BUILTIN_COMPONENTS` の名前）。まだ分からなければ空。
    /// FX2: `waiting` = 待ちの理由の 1 行（CLI を確かめている `Checking for AI on this Mac` など・無ければ None）・`elapsed` = 生成の job を始めてからの時間
    /// （1 秒未満は出さない・`m:ss`）。状態の 1 行 = 状態の印 ＋ `phase.wording()`（＋ ` · ` 待ちの理由）（＋ ` · ` 経過）・次の一手は `Stop`。
    /// Wave: FX2
    Running { phase: Phase, planned: &'a [&'a str], waiting: Option<&'a str>, elapsed: Duration },
    Done(&'a RefineOutcome),
    /// FX2（FX-005）: `Stop` を押した後、子の process が回収されるまで（`Generated::stop_model_job` → `LlmError::Cancelled` が届くまで）。
    /// 状態の 1 行 = 状態の印 ＋ `Stopping the AI…`（＋ ` · ` 経過 = 押してからの時間）・次の一手 0（押せる物を出さない）・呼ぶ側は欄の `Create` も押せない形のまま。
    /// Wave: FX2
    Stopping { elapsed: Duration },
    /// FX2（FX-005・FX-006）: 生成の job が終わった 1 行（止めた = `Stopped. Nothing was added to Projects.`・失敗 = 本当の理由）と次の一手 0 か 1
    /// （失敗の種類ごと = `generated::failure_step`・止めた時は None）。`state` は 1 行の色と印（止めた = Ready・失敗 = Error）。骨格は描かない。
    /// `action` は借りる（`Build` は Copy のまま・`BuildAction` は String を持つので Copy でない = 持ち主は `Generated` の終わった 1 行）。
    /// Wave: FX2
    Ended { state: RegionState, line: &'a str, action: Option<&'a BuildAction> },
}

impl Build<'_> {
    /// 上端の状態 1 行（通った時は None = 行を出さない）。
    /// FX2: `Running` は待ちの理由と経過を足す・`Stopping` は `Stopping the AI…`・`Ended` は渡された 1 行（ICONS-1101 追記 238 から文だけ = 印は `draw` が頭に描く・
    /// 生成中は回る印）。
    pub fn status_line(&self) -> Option<String> {
        match self {
            Build::Running { phase, waiting, elapsed, .. } => {
                let mut line = phase.wording().to_string();
                if let Some(w) = waiting {
                    line.push_str(&format!(" · {w}"));
                }
                Some(line + &clock(*elapsed))
            }
            Build::Done(o) if o.passed => None,
            Build::Done(o) => Some(o.stop.wording().to_string()),
            Build::Stopping { elapsed } => Some(format!("Stopping the AI…{}", clock(*elapsed))),
            Build::Ended { line, .. } => Some(line.to_string()),
        }
    }

    /// 上端の次の一手（生成中はキャンセル・通らない時は `next_step`・通った時は None）。
    /// FX2: `Stopping` は None（子が消えるまで押せる物 0）・`Ended` は渡された `action`。`OpenRun` はここで返さない（1 行の文を押した時だけ `show` が返す）。
    pub fn action(&self) -> Option<BuildAction> {
        match self {
            Build::Running { .. } => Some(BuildAction::Cancel),
            Build::Done(o) => next_step(o),
            Build::Stopping { .. } => None,
            Build::Ended { action, .. } => action.cloned(),
        }
    }

    fn region(&self) -> Option<RegionState> {
        match self {
            Build::Running { .. } | Build::Stopping { .. } => Some(RegionState::Loading),
            Build::Done(o) if o.passed => None,
            Build::Done(o) => Some(o.stop.region()),
            Build::Ended { state, .. } => Some(*state),
        }
    }

    /// FX2 反証の直し: 1 行の文が `Open Runs` の link になる形 = その行に Runs の行（Home の生成の run）が在る形だけ（図 §45 の RN・ST・E0・F）。
    /// 文だけの形（押しても `OpenRun` を返さない）: CLI を確かめている間（`Running` の `waiting` が Some = job はまだ無い）・Page の変更を読んでいる間の
    /// Home の 1 行（`Ended { Loading }` = 編集の job は Runs に行を足さない）・`Done`（refine）。待ちの理由を持つ生成の job を作る時はここを直す。
    fn opens_run(&self) -> bool {
        match self {
            Build::Running { waiting, .. } => waiting.is_none(),
            Build::Stopping { .. } => true,
            Build::Ended { state, .. } => *state != RegionState::Loading,
            Build::Done(_) => false,
        }
    }

    /// FX2: 経過の時計を持つ（Home の生成）。refine の `Running`（`elapsed` = 0）と `Done`・`Ended` は持たない。
    fn ticking(&self) -> Option<Duration> {
        match self {
            Build::Running { elapsed, .. } | Build::Stopping { elapsed } if !elapsed.is_zero() => Some(*elapsed),
            _ => None,
        }
    }
}

/// FX2: 経過の ` · m:ss`（1 秒未満は出さない = 空）。
fn clock(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s == 0 {
        return String::new();
    }
    format!(" · {}:{:02}", s / 60, s % 60)
}

#[derive(Clone)]
struct Mem {
    running: bool,
    /// 最後に生成を始めた時刻（`with_skeleton_by` の読み込みの key・通った後も持つ = 骨格から app へ cross-fade）
    started_at: f64,
    /// 最後に見た予定の component（Done の frame で後ろに薄く出す骨格の形）
    planned: Vec<String>,
}

/// 生成の始まりと予定の形を覚える（画面 1 つに temp 1 件）。初めて見た Done は骨格を一度も出さない。
fn remember(ctx: &egui::Context, id: Id, build: &Build) -> Mem {
    let now = ctx.input(|i| i.time);
    let key = id.with("kangae.build.mem");
    let prev: Option<Mem> = ctx.data(|d| d.get_temp(key));
    let mem = match (prev, build) {
        (prev, Build::Running { planned, .. }) => {
            let started_at = prev.as_ref().filter(|m| m.running).map_or(now, |m| m.started_at);
            let planned = if planned.is_empty() { prev.filter(|m| m.running).map(|m| m.planned).unwrap_or_default() } else { planned.iter().map(|s| s.to_string()).collect() };
            Mem { running: true, started_at, planned }
        }
        // FX2: Stopping・Ended も Done と同じ（骨格は描かない・次の Running で始まりを取り直す）
        (Some(m), _) => Mem { running: false, ..m },
        (None, _) => Mem { running: false, started_at: f64::NEG_INFINITY, planned: Vec::new() },
    };
    ctx.data_mut(|d| d.insert_temp(key, mem.clone()));
    mem
}

/// 作られる app の形の骨格（予定の component ごと）。まだ分からない時は見出し 1 本 + 本文の塊。
fn planned_skeleton(ui: &mut Ui, planned: &[String]) {
    if planned.is_empty() {
        Skeleton::Line.show(ui);
        Skeleton::Block(240.0).show(ui);
        return;
    }
    for c in planned {
        skeleton::region_skeleton(ui, c);
        ui.add_space(theme::SP_6);
    }
}

/// 生成中の画面を `ui` の残りに描く。`app` は通った時の app・通らない時の最良案を描く（renderer）。
/// 戻り値はこの frame で押された次の一手。`id_salt` は同じ親の中の画面ごとに変える。
/// 契約（FX2・署名は同じ）: Home の欄の下（`home::draw`）からも呼ぶ（すぐ終わる処理で出さない 120ms の待ちは `home::draw` が持つ）。
/// `Stopping` は行だけ（button 0）・`Ended` は行と次の一手だけ（骨格・app を描かない）。経過の時計を持つ間（Home の `Running`・`Stopping`）は
/// 次の秒の替わり目に frame を頼む（idle の窓でも `m:ss` が進む）。
/// Home の生成の行のうち Runs の行が在る形（`Build::opens_run`）の文の部分（印と語）は押せる link で、押すと `Some(BuildAction::OpenRun)`
/// （AX の名前は `Open Runs`・値は 1 行の文）。CLI を確かめている間と Page の変更を読んでいる間の 1 行は文だけ（FX2 反証の直し）。
/// 次の一手の button は `line_with_action` と同じ形（右端の compact button）。
/// 保証しない: Home 以外（Page の欄の編集）の生成の行（FX2 の外・Page は今の `Reading your change` の 1 行のまま）。
/// Wave: FX2
pub fn show(ui: &mut Ui, id_salt: impl Hash, build: &Build, app: impl FnOnce(&mut Ui)) -> Option<BuildAction> {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let id = ui.id().with(("kangae.build", id_salt));
    let mem = remember(&ctx, id, build);
    let area = ui.available_rect_before_wrap();
    let mut pressed = None;

    if let (Some(line), Some(region)) = (build.status_line(), build.region()) {
        ctx.accesskit_node_builder(id, |n| {
            n.set_role(accesskit::Role::Group);
            n.set_label(line.clone());
            n.set_bounds(accesskit::Rect { x0: area.min.x.into(), y0: area.min.y.into(), x1: area.max.x.into(), y1: area.max.y.into() });
        });
        if let Some(e) = build.ticking() {
            ctx.request_repaint_after(Duration::from_secs(1) - Duration::from_nanos(u64::from(e.subsec_nanos())));
        }
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, theme::LIST_ROW_H), egui::Sense::hover());
        let color = motion::cross_fade(&ctx, id.with("kangae.build.color"), region.color(&t));
        // ICONS-1101 追記 238: 頭の区間は印の席（状態の色）・印は Label の頭に描く（生成中 = Loading は回る）
        let size = Type::Helper.size();
        let mut job = egui::text::LayoutJob::default();
        job.append("", states::mark_room(region, size), states::helper_format(color));
        job.append(&line, 0.0, states::helper_format(t.text_primary));
        // `line_with_action` の形（右端に button 0 か 1・残りに 1 行）。button が無い時も同じ行の高さと位置
        let mut row = ui.new_child(egui::UiBuilder::new().id_salt(id.with("kangae.build.row")).max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if let Some(action) = build.action() {
            if theme::button_compact(&mut row, action.label(), action.variant()).clicked() {
                pressed = Some(action);
            }
            row.add_space(theme::SP_3);
        }
        let rest = row.available_rect_before_wrap();
        let mut text = row.new_child(egui::UiBuilder::new().max_rect(rest).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let said = text.add(egui::Label::new(job).selectable(true).truncate());
        states::paint_mark(ui.painter(), region, egui::pos2(said.rect.left() + size / 2.0, said.rect.center().y), size, color);
        if build.opens_run() {
            ctx.accesskit_node_builder(said.id, |n| n.set_label(BuildAction::OpenRun.label()));
            if said.clicked() {
                pressed = Some(BuildAction::OpenRun);
            }
        }
    }

    let running = matches!(build, Build::Running { .. });
    skeleton::with_skeleton_by(ui, "build", !running, mem.started_at, |ui| planned_skeleton(ui, &mem.planned), |ui| {
        let Build::Done(o) = build else { return };
        if !o.passed {
            reasons_block(ui, o, &t);
        }
        if o.passed || o.has_best_app() {
            if !o.passed {
                ui.add_space(theme::SP_4);
                theme::rule(ui);
                ui.add_space(theme::SP_4);
            }
            app(ui);
        }
    });
    pressed
}

/// 残った理由 1〜3 行（1 行ずつ切り詰める・選べる label = cua の elements に出る）。
fn reasons_block(ui: &mut Ui, o: &RefineOutcome, t: &theme::Tokens) {
    for r in reasons(o) {
        ui.add(egui::Label::new(Type::Helper.text(format!("· {r}")).color(t.text_secondary)).selectable(true).truncate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(stop: Stop, remaining: &[&str], decisions: Vec<Decision>) -> RefineOutcome {
        RefineOutcome { passed: stop == Stop::Passed, stop, remaining: remaining.iter().map(|s| s.to_string()).collect(), decisions, codegen: Vec::new(), app: None }
    }

    fn decision(software: &str, chosen: Option<&str>, fallback: Option<&str>, needs: &[&str]) -> Decision {
        Decision { software: software.into(), action_id: format!("{}.read", software.to_lowercase()), chosen: chosen.map(String::from), why: String::new(), fallback: fallback.map(String::from), needs: needs.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn reasons_become_plain_words_without_ids() {
        assert_eq!(human_reason("Marketing copy rules [dekimasu]: 下の入力から直接頼めます（hero_1）"), "Leftover description text");
        assert_eq!(human_reason("No loading skeleton: list_2"), "A part has no loading state");
        assert_eq!(human_reason("Crashes on empty input: local.items.add"), "A test press failed");
        assert_eq!(human_reason("Connection not named: Slack"), "A connection you didn't ask for: Slack");
        assert_eq!(human_reason("Note: 送る前に宛先ごとに承認が要る。承認は 1 通ずつ"), "送る前に宛先ごとに承認が要る");
        assert_eq!(human_reason("「検索」の操作が無い"), "「検索」の操作が無い");
    }

    #[test]
    fn at_most_three_reasons_and_never_none() {
        let many = outcome(Stop::NoImprovement, &["No loading skeleton: a_1", "No loading skeleton: b_2", "「検索」の操作が無い", "「並べ替え」の操作が無い", "「共有」の操作が無い"], vec![]);
        assert_eq!(reasons(&many), ["A part has no loading state", "「検索」の操作が無い", "「並べ替え」の操作が無い"]);
        for stop in [Stop::NoImprovement, Stop::Deadline, Stop::NotBuilt] {
            assert_eq!(reasons(&outcome(stop, &[], vec![])), [stop.fallback_reason()], "{stop:?}");
        }
    }

    #[test]
    fn next_step_follows_the_order() {
        let gmail = decision("Gmail", Some("api"), Some("web（gmail.web）"), &["oauth_scope:gmail.readonly"]);
        let reaper = decision("REAPER", None, Some("native_handoff（reaper.app）"), &[]);
        let spotify = decision("Spotify", None, None, &[]);
        assert_eq!(next_step(&outcome(Stop::Passed, &[], vec![gmail.clone()])), None);
        assert_eq!(next_step(&outcome(Stop::NotBuilt, &[], vec![gmail.clone()])), Some(BuildAction::Rephrase));
        assert_eq!(next_step(&outcome(Stop::NoImprovement, &[], vec![reaper.clone(), gmail])), Some(BuildAction::AllowConnection("Gmail".into())));
        assert_eq!(next_step(&outcome(Stop::Deadline, &[], vec![spotify.clone(), reaper])), Some(BuildAction::OpenInSource("REAPER".into())));
        assert_eq!(next_step(&outcome(Stop::Deadline, &[], vec![spotify.clone()])), Some(BuildAction::Retry));
        assert_eq!(next_step(&outcome(Stop::NoImprovement, &[], vec![spotify])), Some(BuildAction::Rephrase));
    }

    /// FX2（FX-004〜006・caller の形・本体の前は todo!() で赤）: Home の生成の 3 つの形の状態 1 行と次の一手。
    /// 1 行 = 文（印は draw が頭に描く・Running は段の語 · 待ちの理由 · m:ss・1 秒未満の経過は出さない）・Stopping は押せる物 0・Ended は渡した 1 行と 0 か 1 の次の一手。
    #[test]
    fn fx2_home_build_lines_and_one_next_step() {
        let quick = Build::Running { phase: Phase::Read, planned: &[], waiting: None, elapsed: Duration::from_millis(400) };
        assert_eq!(quick.status_line(), Some("Reading your request".to_string()), "1 秒未満の経過は出さない");
        let waiting = Build::Running { phase: Phase::Read, planned: &[], waiting: Some("Checking for AI on this Mac"), elapsed: Duration::from_secs(72) };
        assert_eq!(waiting.status_line(), Some("Reading your request · Checking for AI on this Mac · 1:12".to_string()));
        assert_eq!(waiting.action(), Some(BuildAction::Cancel));
        let stopping = Build::Stopping { elapsed: Duration::from_secs(2) };
        assert_eq!(stopping.status_line(), Some("Stopping the AI… · 0:02".to_string()));
        assert_eq!(stopping.action(), None, "子が消えるまで押せる物 0");
        let stopped = Build::Ended { state: RegionState::Ready, line: "Stopped. Nothing was added to Projects.", action: None };
        assert_eq!(stopped.status_line(), Some("Stopped. Nothing was added to Projects.".to_string()));
        assert_eq!(stopped.action(), None);
        let failed = Build::Ended { state: RegionState::Error, line: "AI didn't answer within 120 s", action: Some(&BuildAction::Retry) };
        assert_eq!(failed.action(), Some(BuildAction::Retry), "失敗の次の一手は 1 つ");
        assert_eq!((BuildAction::UseSimpleRules.label(), BuildAction::OpenRun.label()), ("Use simple rules", "Open Runs"));
        for b in [quick, waiting, stopping, stopped, failed] {
            assert_ne!(b.action(), Some(BuildAction::OpenRun), "Open Runs は次の一手にしない（1 行の文を押した時だけ）: {b:?}");
        }
    }

    #[test]
    fn about_rows_drop_internal_words() {
        let mut d = decision("REAPER", Some("local"), Some("native_handoff（reaper.app）"), &[]);
        d.why = "local（reaper.cli）: wav を REAPER の track に入れる（InsertMedia）。要る物を確かめるまで requires_connection".into();
        let mut o = outcome(Stop::Passed, &[], vec![d]);
        o.codegen.push(Codegen { software: "Spotify".into(), operation: "spotify.playback".into(), why: "この Mac に無く、registry にも記録が無い".into() });
        assert_eq!(
            about_rows(&o),
            [("REAPER".to_string(), "On this Mac · wav を REAPER の track に入れる · Instead: Open in the original app".to_string()), ("Spotify".to_string(), "Not reachable · この Mac に無く、registry にも記録が無い".to_string())]
        );
    }
}
