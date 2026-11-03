//! W19-2 の配線（図 §33）: Page の「この Project への変更」の欄から Engine に渡した依頼の、Page の 1 行（進めている・承認・済み・止めた・失敗）。
//! 状態を持つのは `Generated`（Project ごとに 1 つ・保存しない）。ここは描くだけで、Engine の型を持たない（tests/renderer.rs などが ui を #[path] で引く）。
//! 文（理由・rail の実行履歴）は `engine_page` が Engine の答えから作る。画面に出すのは人の言葉だけ（src/ui は check_ui_copy の対象）。

use super::states::{self, RegionState};
use super::theme::{self, Variant};
use eframe::egui;
use serde_json::Value;

/// Page に出す Engine の状態（Project ごとに 1 つ・保存しない）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq)]
pub enum EngineView {
    /// 裏の thread が run している（同時に 1 つ）。waiting = 今長く待っている事の 1 行（答えない app を読み直す間の「X が答えないので待っています（最長 N 秒）」・
    /// REA を起こす・読む間の「REA を起こしています（最長 N 秒）」・「進めています」の下に折り返して全部出す = AX の AXStaticText の値。
    /// None = 待っていない・W19-3b G2・W19-3-fix）。stopping = 人が `STOP_LABEL` を押した（`EngineAct::Stop`）= button は押せない `STOPPING_LABEL` になり、
    /// run が閉じた時に Done 以外なら `STOPPED_LINE` の 1 行（Done なら済みの 1 行のまま = 止めたと言わない・`Generated::poll_engine`）
    Running { goal: String, waiting: Option<String>, stopping: bool },
    /// 承認待ち（Engine の `Denial::pending` と `later`）。「承認して送る」1 回で全部の手の ConsentReceipt を作り、止まった所から続ける
    /// （WF2: cursor の続き・止まった行が無い時だけ同じ一言を run し直す）。line = 出し直した理由の 1 行（WF2: 止まっている間に元データが変わった・None = 初めて出す）
    Approve { goal: String, steps: Vec<ApprovalStep>, line: Option<String> },
    /// 承認した手の宛先の app に AI に任せる同意が無くて送らなかった（W19-1 の残り）。`line` と「X を AI に任せる」（`consent_label`）・
    /// 押すと同じ手を `EngineAct::Approve` と同じく送り直す（宛先の app の同意も書く）
    Consent { goal: String, steps: Vec<ApprovalStep>, line: String },
    /// 終わった（済み・止めた・失敗）の 1 行。次の依頼で消える。card には描かない（NOTIFY-1101 追記・owner 192）:
    /// 済み・止めたは会話の 1 行（`Generated::poll_engine` の say）・失敗は右上の toast（`Generated::take_notices`）
    Line { state: RegionState, line: String },
    /// WF2: 手順の途中で人が選ぶ（Engine の `StepKind::Decide`・run は `waiting_human`）。選択肢ごとに button（label = AX の名前）と証拠の行・
    /// 押しても承認・同意の receipt は作らない。line = 出し直した理由の 1 行（止まっている間に元データが変わった・None = 初めて出す）。
    /// 再起動の後も同じ Project に同じ card（runs 表の記録から）
    /// Wave: WF0
    Decide { goal: String, question: String, choices: Vec<DecideOption>, line: Option<String> },
    /// L7-AGENT: Project の Terminal の agent が `shibubu project ask` で頼んだ一言（runs 表の承認待ちの行・`Generated::poll_asks`）。
    /// 承認（`EngineAct::Approve`）で初めて、人が欄に打ったのと同じ道で 1 回読む・Cancel は何も変えない。`who` = `InvocationOrigin::who`
    /// Wave: L7
    Ask { who: String, text: String },
}

/// WF2: Decide の card の選択肢 1 つ（Engine の `DecideChoice` から見せる物だけ）。
/// Wave: WF0
#[derive(Debug, Clone, PartialEq)]
pub struct DecideOption {
    /// choice の id（Engine に返す・画面には出さない）
    pub id: String,
    /// button の名前（AX の名前も同じ）
    pub label: String,
    /// 証拠の行（1 行の得失と、選んだ時に後ろの手へ渡す値の文字）
    pub lines: Vec<String>,
}

/// 承認待ちの 1 手（Engine の `PendingApproval` から、見せる値と承認の記録に要る物だけ）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalStep {
    /// action の id（承認の記録の action_ref・画面には出さない）
    pub action: String,
    /// 受け渡した後の exact input（見せる値）
    pub input: Value,
    /// `payload_digest(action, input)`
    pub digest: String,
    /// action の required_scopes（承認の記録の scopes）
    pub scopes: Vec<String>,
}

/// Page の Engine の行で押した物。
/// Wave: W19-2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineAct {
    /// 承認待ちの手を全部承認して送る（`EngineView::Approve` の「承認して送る」と `EngineView::Consent` の「X を AI に任せる」）
    Approve,
    /// 送らない・1 行を閉じる（何も送らない）
    Cancel,
    /// WF2: Decide の card の n 番目の選択肢を選ぶ（承認ではない・cursor の続きから走らせる）
    /// Wave: WF0
    Choose(usize),
    /// OPEN-INPLACE-1101 の 2（owner 240）: `Approve` と同じく今の手を全部送り、その手の app と種類（`surface::AlwaysAllowed`）を覚える
    /// = 以後その app のその種類の手は承認の card を出さずに打つ（読み戻しは省かない）。`Consent`・`Ask`・`Decide` の card には出さない（承認の card だけ）。
    /// Wave: OPEN-INPLACE-1101
    AlwaysAllow,
    /// Running の card の `STOP_LABEL`: 進めている run を止めるよう頼む（`Generated::engine_act` が `Generated::stop_engine` で止める印を立てる）。
    /// 止まるのを待つ間は `STOPPING_LABEL`（押せない）・閉じたら `STOPPED_LINE`（`Generated::poll_engine`）
    Stop,
}

/// 「承認して送る」の button（AX の名前も同じ・試験の道具が BG で element_token を引く）。
/// Wave: W19-2
pub const APPROVE_LABEL: &str = "Approve and send";
/// CHAT-013: 承認の手が書類を開く手だけの時の button（送る手ではないので「送る」と言わない・AX の名前も同じ）。
/// Wave: W19-2
pub const OPEN_LABEL: &str = "Approve and open";
/// OPEN-INPLACE-1101 の 2: 承認の card の左の button（`EngineAct::AlwaysAllow`・AX の名前も同じ）。承認の card（Page の Engine の欄・chat の列の card・
/// 部品の所 = 頁の bridge の同じ字）に出す。ENGINE-AUDIT W7 から押す手だけの card には出さない（`asks_every_time`）。右の 2 つ（`APPROVE_LABEL` / `OPEN_LABEL` / `ANALYZE_LABEL` と `Cancel`）の字は今のまま
/// （BG の試験の道具〈scripts/qa/bg〉が名前で引く）。部品の所の頁の 3 つ（Always allow・Allow once・Cancel）は頁の bridge の JS の字
/// （`surface::canvas_document`・const の JS に Rust の定数を入れられない・`surface` の試験 open_inplace が 3 つの字と `{allow:…}` を押さえる）。
/// Wave: OPEN-INPLACE-1101
pub const ALWAYS_LABEL: &str = "Always allow";
/// 「送らない」の button（承認待ちの時）。
/// Wave: W19-2
pub const CANCEL_LABEL: &str = "Cancel";
/// Running の card の止める button（`EngineAct::Stop`・AX の名前も同じ）。
pub const STOP_LABEL: &str = "Stop";
/// Stop を押した後、run が閉じるまでの押せない button（AX の名前も同じ）。
pub const STOPPING_LABEL: &str = "Stopping…";
/// 人が止めた run が閉じた時の会話の 1 行（Done で閉じた時は出さない）。
pub const STOPPED_LINE: &str = "Stopped by you";
/// L7-AGENT: Terminal の agent が頼んだ変更の承認の button（AX の名前も同じ・送る手ではないので「送る」と言わない）。
/// Wave: L7
pub const ASK_LABEL: &str = "Approve change";

/// L7-AGENT: agent が頼んだ変更の card の 1 行（誰が・何を・承認するまで何も変わらない事）。
/// 例: `ask_line("claude in a terminal", "add a Tests page")` → `claude in a terminal asks to change this project: “add a Tests page”. …`
pub fn ask_line(who: &str, text: &str) -> String {
    format!("{who} asks to change this project: “{text}”. Nothing changes until you approve. Approving reads it once, as if you typed it")
}
/// 承認の 1 行に出さない input の欄（窓・要素の指し方。宛先は `app_name` で言う・`bundle_id` は W19-3b の宛先の決め直しの鍵・`match` は M2 G-c の探し方・
/// `document_file` は FX-111 の書類の場所 = home の path）。
const HIDDEN_KEYS: [&str; 8] = ["app_name", "label", "role", "pid", "window_id", "bundle_id", "match", "document_file"];
/// W19-3: 解析の承認の手（Engine の `capability_engine::ANALYSIS_ACTION` + "."・この file は Engine の型を持たないので文字の頭で見分ける・
/// 頭が Engine と合う事は engine_page の試験が縛る）。送る手ではなく、対象の app の中を読んで調べる手。
/// Wave: W19-3
pub const ANALYSIS_STEP: &str = "analysis.";

/// CHAT-013: 名指しの書類を開く手（Engine の `OPEN_DOCUMENT` と同じ id・頭が合う事は engine_page の試験が縛る）。送る手ではなく、書類を開く手。
pub const OPEN_STEP: &str = "desktop.document.open";

fn opening(steps: &[ApprovalStep]) -> bool {
    !steps.is_empty() && steps.iter().all(|s| s.action == OPEN_STEP)
}

fn analysis(steps: &[ApprovalStep]) -> bool {
    !steps.is_empty() && steps.iter().all(|s| s.action.starts_with(ANALYSIS_STEP))
}

/// W19-3: 解析の承認の button（送る手ではないので「送る」と言わない・AX の名前も同じ・試験の道具が BG で element_token を引く）。
/// Wave: W19-3
pub const ANALYZE_LABEL: &str = "Look inside";
/// W19-3: 解析の承認を断る button（解析しない・既存の連携はもう試した）。
/// Wave: W19-3
pub const NO_ANALYSIS_LABEL: &str = "Cancel";

/// 承認の 1 行の 2 つの button の名前（AX の名前も同じ）: 解析の承認だけなら「承認して調べる」「調べない」、送る手が在れば「承認して送る」「送らない」。
fn labels(steps: &[ApprovalStep]) -> (&'static str, &'static str) {
    if analysis(steps) {
        (ANALYZE_LABEL, NO_ANALYSIS_LABEL)
    } else if opening(steps) {
        (OPEN_LABEL, CANCEL_LABEL)
    } else {
        (APPROVE_LABEL, CANCEL_LABEL)
    }
}

/// 送る手 1 つの人の言葉（承認の card の手の部分・OPEN-INPLACE-1101 の 5: chat の案の Preview の行も同じ字 = `CoreEngine::plan`）。
/// 例: `step_line(&ApprovalStep { action: "desktop.ax.click".into(), input: json!({ "app_name": "GarageBand", "label": "Sounds" }), .. })` == `"Press “Sounds” in GarageBand"`
pub fn step_line(s: &ApprovalStep) -> String {
    let to = s.input["app_name"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("the app");
    let label = s.input["label"].as_str().map(str::trim).filter(|l| !l.is_empty());
    let path: Vec<&str> = s.input["path"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    // G79: 画面の手の宛先 = 書類が在れば「TextEdit's “README.md”」（どの書類か）・無ければ app
    let doc = s.input["document"].as_str().filter(|d| !d.trim().is_empty());
    let at = doc.map_or_else(|| to.to_string(), |d| format!("{to}'s “{d}”"));
    let shown = |k: &str| if secret_value(&s.input, k) { "…".to_owned() } else { s.input[k].as_str().unwrap_or("").replace('\n', " ⏎ ") };
    match (s.action.as_str(), label) {
        // M2 G-e: 押す手は何を押すかを言う（click の label は利用者の言葉・menu は path）
        ("desktop.ax.click", Some(l)) => format!("Press “{l}” in {at}"),
        ("desktop.ax.menu", _) if !path.is_empty() => format!("Press {} in {at}", path.join(" › ")),
        // G79: 一部を置き換える type_text は「どこの何を何に」・replace 無しは書類なら全文を置き換えると言う・書類でない欄は打つ
        ("desktop.ax.type_text", _) if s.input["replace"].is_string() => format!("In {at}, replace “{}” with “{}”", shown("replace"), shown("text")),
        ("desktop.ax.type_text", _) if doc.is_some() => format!("In {at}, replace all the text with “{}”", shown("text")),
        ("desktop.ax.type_text", _) if s.input["text"].is_string() => format!("Type “{}” in {to}", shown("text")),
        ("desktop.key", _) if s.input["key"].is_string() => format!("Press {} in {at}", chord(&s.input)),
        _ => match texts(&s.input) {
            v if v.is_empty() => format!("Control {to}"),
            v => format!("Send “{}” to {to}", v.join(" · ")),
        },
    }
}

/// OPEN-INPLACE-1101 の 2: Always allow で覚えられる承認 = 全部の手が画面の操作（input に宛先の `pid` と `app_name`）。`CoreEngine::grant` が app を決めて書く
fn rememberable(steps: &[ApprovalStep]) -> bool {
    !steps.is_empty() && steps.iter().all(|s| s.input["pid"].is_u64() && s.input["app_name"].as_str().is_some_and(|a| !a.trim().is_empty()))
}

/// ENGINE-AUDIT W7（operator 11-03）: 押す手（`desktop.ax.click`）だけの承認 = Core が `gui::asks` に当たった押す手（消す・送る・買う系・確認の窓の中）にだけ出す card。
/// Always allow を押しても次も毎回聞く（`CoreEngine::skips_approval` は押す手を飛ばさない）ので、card は Always allow を出さず（効かない button を置かない）この 1 行を出す。
/// 打つ・menu・key の手が 1 つでも在れば None = 今まで通り Always allow（owner 240）。
/// 例: 押す手「Delete」だけ → `Some("Asks every time before pressing “Delete”")`・名前の無い押す手だけ → `Some("Asks every time before this press")`
pub fn asks_every_time(steps: &[ApprovalStep]) -> Option<String> {
    if steps.is_empty() || !steps.iter().all(|s| s.action == "desktop.ax.click") {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for l in steps.iter().filter_map(|s| s.input["label"].as_str()).map(str::trim).filter(|l| !l.is_empty()) {
        let q = format!("“{l}”");
        if !names.contains(&q) {
            names.push(q);
        }
    }
    Some(if names.is_empty() { "Asks every time before this press".to_owned() } else { format!("Asks every time before pressing {}", names.join(" · ")) })
}

/// Page の欄の下に Engine の 1 行を描く（Running = 進めている 1 行と `STOP_LABEL`〈押した後は押せない `STOPPING_LABEL`〉・Approve = 承認の 1 行と 3 つの button〈OPEN-INPLACE-1101: 左に `ALWAYS_LABEL`
/// = `EngineAct::AlwaysAllow`・続けて今の 2 つ〉・ENGINE-AUDIT W7: 押す手だけの承認は Always allow の代わりに `asks_every_time` の 1 行・Line = 描かない）。modal にしない。
/// Wave: W19-2
/// 例: `if let Some(v) = g.engine_view(project_id) { if let Some(act) = engine_view::draw(ui, v) { g.engine_act(project_id, act) } }`
pub fn draw(ui: &mut egui::Ui, view: &EngineView) -> Option<EngineAct> {
    let mut act = None;
    match view {
        // Loading の 1 行（model の「読んでいます」と同じ形）。答えが届くと裏の thread が egui を起こす（`Generated::wake_on_terminal` の ctx）
        EngineView::Running { goal, waiting, stopping } => {
            states::inline(ui, RegionState::Loading, &format!("Working on “{goal}”"));
            // 待っている事は inline（切り詰める）に混ぜず、折り返して全部出す（AX の AXStaticText の値も全文）
            if let Some(w) = waiting {
                ui.add(egui::Label::new(w.as_str()).wrap().selectable(true));
            }
            // 押した後は 2 度押せない（run が閉じるまで待つ）
            if *stopping {
                theme::button_disabled(ui, STOPPING_LABEL, theme::BUTTON_COMPACT_H, "Waiting for this request to end");
            } else if theme::ax_name(theme::button_compact(ui, STOP_LABEL, Variant::Ghost), STOP_LABEL).clicked() {
                act = Some(EngineAct::Stop);
            }
        }
        EngineView::Approve { goal, steps, line } => {
            let what = if analysis(steps) { "Approval needed before looking" } else if opening(steps) { "Approval needed before opening" } else { "Approval needed before sending" };
            states::inline(ui, RegionState::Blocked, what);
            if let Some(l) = line {
                ui.add(egui::Label::new(l.as_str()).wrap().selectable(true));
            }
            // 1 行は折り返して全部見せる（inline は切り詰める）。hover は exact payload の全部の値
            let full: Vec<String> = steps.iter().map(|s| texts(&s.input).join(" · ")).collect();
            ui.add(egui::Label::new(approval_line(goal, steps)).wrap().selectable(true)).on_hover_text(full.join("\n"));
            // RQ-026: 2 行目 = 効く範囲と大きさ（何か所・全文・押す数）。1 行目と button の AX 名は変えない
            if let Some(scope) = scope_line(steps) {
                ui.add(egui::Label::new(scope).wrap().selectable(true));
            }
            // ENGINE-AUDIT W7: 押す手だけの card は毎回聞く = Always allow の代わりに理由の 1 行
            let every = asks_every_time(steps);
            if let Some(why) = &every {
                ui.add(egui::Label::new(why.as_str()).wrap().selectable(true));
            }
            let (approve, cancel) = labels(steps);
            ui.horizontal_wrapped(|ui| {
                // OPEN-INPLACE-1101 の 2: 覚えられる手（画面の操作 = 宛先の pid と app 名を持つ手）だけの card に Always allow（覚えられない手・W7 の押す手だけの card には出さない）
                if rememberable(steps) && every.is_none() && theme::ax_name(theme::button_compact(ui, ALWAYS_LABEL, Variant::Ghost), ALWAYS_LABEL).clicked() {
                    act = Some(EngineAct::AlwaysAllow);
                }
                if theme::ax_name(theme::button_compact(ui, approve, Variant::Primary), approve).clicked() {
                    act = Some(EngineAct::Approve);
                }
                if theme::ax_name(theme::button_compact(ui, cancel, Variant::Ghost), cancel).clicked() {
                    act = Some(EngineAct::Cancel);
                }
            });
        }
        EngineView::Ask { who, text } => {
            states::inline(ui, RegionState::Blocked, "Approval needed before changing");
            ui.add(egui::Label::new(ask_line(who, text)).wrap().selectable(true));
            ui.horizontal_wrapped(|ui| {
                if theme::ax_name(theme::button_compact(ui, ASK_LABEL, Variant::Primary), ASK_LABEL).clicked() {
                    act = Some(EngineAct::Approve);
                }
                if theme::ax_name(theme::button_compact(ui, CANCEL_LABEL, Variant::Ghost), CANCEL_LABEL).clicked() {
                    act = Some(EngineAct::Cancel);
                }
            });
        }
        EngineView::Consent { steps, line, .. } => {
            states::inline(ui, RegionState::Blocked, "Needs your consent before sending");
            ui.add(egui::Label::new(line.as_str()).wrap().selectable(true));
            let label = consent_label(steps);
            ui.horizontal_wrapped(|ui| {
                if theme::ax_name(theme::button_compact(ui, &label, Variant::Primary), &label).clicked() {
                    act = Some(EngineAct::Approve);
                }
                if theme::ax_name(theme::button_compact(ui, CANCEL_LABEL, Variant::Ghost), CANCEL_LABEL).clicked() {
                    act = Some(EngineAct::Cancel);
                }
            });
        }
        // NOTIFY-1101 追記（owner 192「chat の中の Close の文字の button はやめ」）: 結果の 1 行は card にしない（会話の 1 行か toast・`agent_rail::chat_column` も渡さない）
        EngineView::Line { .. } => {}
        // WF2: 判断の card（承認の card の所に描く・G86 の会話の列と merge で寄せる）。押しても承認・同意の receipt は作らない
        EngineView::Decide { question, choices, line, .. } => {
            // 承認の button は無い（選ぶだけ）ので Blocked（▣ Needs approval）にしない（UI-1・G73: Needs approval は承認の button が同じ行に在る時だけ）
            states::inline(ui, RegionState::Ready, "Your choice is needed");
            if let Some(l) = line {
                ui.add(egui::Label::new(l.as_str()).wrap().selectable(true));
            }
            ui.add(egui::Label::new(question.as_str()).wrap().selectable(true));
            for (n, c) in choices.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if theme::ax_name(theme::button_compact(ui, &c.label, Variant::Ghost), &c.label).clicked() {
                        act = Some(EngineAct::Choose(n));
                    }
                    for l in &c.lines {
                        ui.add(egui::Label::new(l.as_str()).wrap().selectable(true));
                    }
                });
            }
            if theme::ax_name(theme::button_compact(ui, CANCEL_LABEL, Variant::Ghost), CANCEL_LABEL).clicked() {
                act = Some(EngineAct::Cancel);
            }
        }
    }
    act
}

/// G86: rail を開く card の鍵（`Shell::show_card`）。進めている間と結果の 1 行は None（承認・同意・問いが出た時だけ開く・NOTIFY-1101 追記）。
pub fn card_key(project_id: &str, view: &EngineView) -> Option<String> {
    match view {
        EngineView::Running { .. } | EngineView::Line { .. } => None,
        // WF2: 止まっている間に元データが変わって card を出し直した（line = Changed の理由）時も、畳んだ列をもう一度開く
        EngineView::Approve { goal, steps, line } => Some(format!("{project_id}:approve:{goal}:{}:{}", steps.len(), line.as_deref().unwrap_or(""))),
        EngineView::Decide { goal, question, line, .. } => Some(format!("{project_id}:decide:{goal}:{question}:{}", line.as_deref().unwrap_or(""))),
        EngineView::Consent { goal, .. } => Some(format!("{project_id}:consent:{goal}")),
        EngineView::Ask { text, .. } => Some(format!("{project_id}:ask:{text}")),
    }
}

/// 画面の操作の手（input に `pid`）の宛先の app 名（重ねない・出てきた順）。
fn gui_apps(steps: &[ApprovalStep]) -> Vec<&str> {
    let mut apps: Vec<&str> = Vec::new();
    for a in steps.iter().filter(|s| s.input["pid"].is_u64()).filter_map(|s| s.input["app_name"].as_str()).filter(|a| !a.trim().is_empty()) {
        if !apps.contains(&a) {
            apps.push(a);
        }
    }
    apps
}

/// 同意が無くて止めた時の button（AX の名前も同じ・app ごとの AI の同意〈`gui-act-consents.jsonl`〉の「X を AI に任せる」と同じ文）。宛先の app 名が無ければ「この app」。
/// Wave: W19-2
pub fn consent_label(steps: &[ApprovalStep]) -> String {
    let apps = gui_apps(steps);
    format!("Hand {} to the AI", if apps.is_empty() { "this app".to_string() } else { apps.join(", ") })
}

/// input の中の、人に見せる文字の値（`HIDDEN_KEYS` の欄と数を落とす・文字の配列は「›」でつなぐ・1 つずつ `screen_text`・`secret_value` は「{名前}: …」）。
/// B54: path と URL も落とさない（送り先の URL・保存先の path が見えないまま承認させない）
fn texts(input: &Value) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let home = home.trim_end_matches('/');
    let Some(o) = input.as_object() else { return Vec::new() };
    o.iter()
        .filter(|(k, _)| !HIDDEN_KEYS.contains(&k.as_str()))
        .filter_map(|(k, v)| {
            let shown = match v {
                Value::String(t) if !t.trim().is_empty() => screen_text(t, home),
                Value::Array(a) => {
                    let parts: Vec<String> = a.iter().filter_map(Value::as_str).map(|t| screen_text(t, home)).collect();
                    if parts.is_empty() {
                        return None;
                    }
                    parts.join(" › ")
                }
                _ => return None,
            };
            Some(if secret_value(input, k) { format!("{k}: …") } else { shown })
        })
        .collect()
}

/// 欄の名前が秘密を示す語（大文字小文字・空白・`-`・`_` を除いて含むか）。URL の `SECRET_PARAMS` より狭い: `key`・`code`・`auth`・`sig` は人の欄の名前
/// （Key・Postal code・Author・Design）にも在るので入れない。
const SECRET_NAMES: [&str; 10] = ["password", "passcode", "passwd", "secret", "token", "apikey", "privatekey", "credential", "authorization", "cookie"];

/// 欄の名前（input の key・打つ先の欄の AX の名前 = `label`〈AXTitle → AXDescription → 見出し〉）が秘密を示すか: `SECRET_NAMES` を含むか、語ごとに `pin`。
fn secret_name(name: &str) -> bool {
    let low = name.to_lowercase();
    let squeezed: String = low.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect();
    SECRET_NAMES.iter().any(|w| squeezed.contains(w)) || low.split(|c: char| !c.is_alphanumeric()).any(|w| w == "pin")
}

/// B54 の続き（operator 11-03・表示だけ・送る値は変えない）: 承認の 1 行と hover に出さない値か = input の key の名前が `secret_name`、
/// または打つ字（`text`・`replace`）で打つ先の欄の名前（`label`）が `secret_name`。普通の欄（書類の本文・検索の欄）の字は今のまま出す。
fn secret_value(input: &Value, key: &str) -> bool {
    secret_name(key) || (matches!(key, "text" | "replace") && input["label"].as_str().is_some_and(secret_name))
}

/// URL の query と fragment で、名前にこの語を含む値を `…` にする（名前は残す・大文字小文字は見ない）。
const SECRET_PARAMS: [&str; 7] = ["token", "key", "secret", "password", "auth", "sig", "code"];

/// 値 1 つを画面の字に（表示だけ・送る値は変えない）: HOME の下は `~/…`（画面に利用者の名前を出さない）・改行は ⏎。
/// B54 の続き（秘密を画面に出さない）: `://` を含む語は、どの `://` の後ろでも認証部の userinfo（`u:p@`）を落とし（`composer::verifier` の host の 1 行と同じ形・入れ子の URL も）、
/// 最初の `?` / `#` から先の `名前=値` は名前が `SECRET_PARAMS` の語を含めば値を `…` に。読み違えたら伏せる側に倒れる（`postcode`・`author` も伏せる）。
/// 例: `https://u:p@host/x?token=abc&q=1` → `https://host/x?token=…&q=1`
fn screen_text(t: &str, home: &str) -> String {
    let t = if home.is_empty() { t.to_owned() } else { t.replace(&format!("{home}/"), "~/") };
    let mut out = String::with_capacity(t.len());
    for word in t.split_inclusive(char::is_whitespace) {
        let body = word.trim_end();
        let Some(first) = body.find("://") else {
            out.push_str(word);
            continue;
        };
        let mut url = body.to_owned();
        let mut from = 0;
        while let Some(i) = url[from..].find("://") {
            let a = from + i + 3;
            let end = url[a..].find(['/', '?', '#']).map_or(url.len(), |e| a + e);
            if let Some(at) = url[a..end].rfind('@') {
                url.replace_range(a..=a + at, "");
            }
            from = a;
        }
        let q = url[first..].find(['?', '#']).map_or(url.len(), |e| first + e);
        out.push_str(&url[..q]);
        for part in url[q..].split_inclusive(['?', '#', '&', ';']) {
            let (pair, sep) = part.split_at(part.len() - usize::from(part.ends_with(['?', '#', '&', ';'])));
            match pair.split_once('=') {
                Some((name, _)) if SECRET_PARAMS.iter().any(|w| name.to_ascii_lowercase().contains(w)) => {
                    out.push_str(name);
                    out.push_str("=…");
                }
                _ => out.push_str(pair),
            }
            out.push_str(sep);
        }
        out.push_str(&word[body.len()..]);
    }
    out.replace('\n', " ⏎ ")
}

/// G79: key の手の人の形（⌘S・⇧⌘→・Return）。修飾 key は macOS の menu と同じ ⌃⌥⇧⌘ の順・矢印は記号・1 字は大文字・名前は頭だけ大文字。
fn chord(input: &Value) -> String {
    let mods: Vec<&str> = input["mods"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let marks: String = [("control", "⌃"), ("option", "⌥"), ("shift", "⇧"), ("cmd", "⌘")].iter().filter(|(m, _)| mods.contains(m)).map(|(_, s)| *s).collect();
    let key = match input["key"].as_str().unwrap_or("") {
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        k => k.chars().take(1).flat_map(char::to_uppercase).chain(k.chars().skip(1)).collect(),
    };
    format!("{marks}{key}")
}

/// RQ-026: 承認の card の 2 行目 = 効く範囲と大きさ（何か所・全文か・押す数）。approval_line（AX 名と試験が縛る文）は変えずに、同じ card の下に出す。
/// Wave: WF-E
/// 例: `scope_line(&steps)` → `Some("Changes 1 place in TextEdit's “README.md”; the rest stays the same")`・全文の置き換えは
/// `"Replaces all the text in …"`・押す手だけは `"Presses 2 things in …; nothing is typed"`。保証しない: 押した後の app の振る舞い。
pub fn scope_line(steps: &[ApprovalStep]) -> Option<String> {
    let desk: Vec<&ApprovalStep> = steps.iter().filter(|s| s.action.starts_with("desktop.")).collect();
    let first = desk.first()?;
    let app = |s: &ApprovalStep| s.input["app_name"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("the original app").to_string();
    let at = |s: &ApprovalStep| match s.input["document"].as_str().filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{}'s “{d}”", app(s)),
        None => app(s),
    };
    let typed: Vec<&ApprovalStep> = desk.iter().copied().filter(|s| s.action == "desktop.ax.type_text").collect();
    let replaces = |s: &ApprovalStep| s.input["replace"].as_str().is_some_and(|r| !r.is_empty());
    let things = |n: usize| crate::domain::plural(n, "thing");
    // 字を置く宛先（書類）ごとに 1 句・最初の手の順（WF-E x5a: 最初の手の宛先で全部を言わない = 全文を替える書類の名前を言う）
    let mut dests: Vec<String> = Vec::new();
    for d in typed.iter().map(|t| at(t)) {
        if !dests.contains(&d) {
            dests.push(d);
        }
    }
    let edit = (!dests.is_empty()).then(|| {
        let each: Vec<String> = dests
            .iter()
            .map(|d| {
                let here: Vec<&ApprovalStep> = typed.iter().copied().filter(|t| at(t) == *d).collect();
                let places = here.iter().filter(|t| replaces(t)).count();
                if here.iter().any(|t| !replaces(t) && t.input.get("document").is_some()) {
                    format!("Replaces all the text in {d}")
                } else if places > 0 {
                    format!("Changes {} in {d}; the rest stays the same", crate::domain::plural(places, "place"))
                } else {
                    format!("Types into {d}")
                }
            })
            .collect();
        each.join(" · ")
    });
    Some(match (edit, desk.len() - typed.len()) {
        (Some(e), 0) => e,
        (Some(e), n) => format!("{e} · then presses {}", things(n)),
        (None, n) if desk.len() == steps.len() => format!("Presses {} in {}; nothing is typed", things(n), app(first)),
        // WF-E x5b: 画面の手でない手（字を送りうる）が同じ card に在れば「nothing is typed」と言わない
        (None, n) => format!("Presses {} in {}", things(n), app(first)),
    })
}

/// 承認の 1 行: 頼んだ事と、送る手ごとに「どの app へ・何を」（input の文字の値）。
/// Wave: W19-2
/// 保証する: 手の数だけ「 · 」で区切って並ぶ（表の 1 行の中の並びの区切り・日本語の「・」を画面に出さない）・input の `app_name` を宛先に（無ければ「元の app」）・action の id・pid・窓番号は出さない（B54: 送り先の URL と保存先の path は出す・HOME の下は `~/…`）・
/// 送る中身は 1 字も省かない（G79 operator 01:3x: 承認 = この内容を 1 回だけ。W19a2 の 40 字で間を「…」に切る形は外した。B54 の続き: 秘密の欄の値と URL の鍵らしい値だけ `…`）・画面の操作の手が在れば、宛先の app を AI に任せる同意も書く事を言う
/// （app ごとの同意の 1 つの鍵 `gui-act-consents.jsonl`）。M2 G-e: 押す手は何を押すかを言う = click は「Press “{label}” in {App}」（label は利用者の言葉・`HIDDEN_KEYS` でも
/// click の label だけは出す）・menu は「Press {path} in {App}」・手が 1 つなら「1 step」。G79: 画面の手は人の語 = 宛先は書類が在れば「{App}'s “{書類}”」・
/// 置き換える type_text は「In {宛先}, replace “前” with “後”」・replace 無しは書類なら「replace all the text with」・書類でない欄は「Type “…” in {App}」・
/// key は「Press ⌘S in {宛先}」（input の内部の値〈key 名・mods・document〉を「Send “…”」に並べない）。
pub fn approval_line(goal: &str, steps: &[ApprovalStep]) -> String {
    // W19-3: 解析の承認は送る手ではない（既存の連携で手段が無かった時だけ出る・対象の app の中を読む）。operator 10-24 の形: 「<道具> の tool N 個
    // （壊さないと申告した物だけ・申告は <道具> のもの）で <対象> を読みます。送りません」。N と対象の path を出す（承認の input の `tools`・`target`）。
    // 「送りません」の根拠: Shibubu は送らない・REA 6.7.0 の allowlist の 5 つは network に出ない（source を読み通信を観測した・docs/evidence/W19-3/rea-facts.md）
    if analysis(steps) {
        let each: Vec<String> = steps
            .iter()
            .map(|s| {
                let by = s.input["by"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("the analysis tool");
                let tools = match s.input["tools"].as_array().map_or(0, Vec::len) {
                    0 => format!("only tools {by} says won't break anything"),
                    1 => format!("1 tool {by} says won't break anything"),
                    n => format!("{n} tools {by} says won't break anything"),
                };
                let name = s.input["app_name"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("the target app");
                let ver = s.input["version"].as_str().map_or_else(|| "version unknown".to_string(), |v| format!("version {v}"));
                let what = s.input["target"].as_str().filter(|t| !t.trim().is_empty()).map_or_else(|| format!("{name} ({ver})"), |t| format!("{t} ({name}, {ver})"));
                format!("{what} using {tools} (that claim is {by}'s)")
            })
            .collect();
        return format!(
            "No existing connection can do “{goal}”. Shibubu will read {}. Nothing is sent. Shibubu doesn't start or install the app. Anything it finds is tried on a copy and asks you again.",
            each.join(" · ")
        );
    }
    // CHAT-013: 開く手だけ = 何も変えない・開いた後に変える手をもう一度聞く
    if opening(steps) {
        let each: Vec<String> = steps
            .iter()
            .map(|s| format!("open this project's “{}” in {}", s.input["document"].as_str().unwrap_or("the file"), s.input["app_name"].as_str().unwrap_or("the app")))
            .collect();
        return format!("To do “{goal}”, Shibubu will first {} (approving opens exactly this file, once · nothing is changed yet · it asks you again before changing anything)", each.join(" · "));
    }
    let each: Vec<String> = steps.iter().map(step_line).collect();
    let apps = gui_apps(steps);
    let keep = if apps.is_empty() { String::new() } else { format!(", and hands {} to the AI", apps.join(", ")) };
    let n = steps.len();
    format!("To do “{goal}”, Shibubu will send {n} {}: {} (approving sends exactly this, once{keep})", if n == 1 { "step" } else { "steps" }, each.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(action: &str, input: Value) -> ApprovalStep {
        ApprovalStep { action: action.into(), digest: "d".into(), input, scopes: vec![] }
    }

    /// B54: 画面の手でない承認の手（MCP の tool・作った口）の 1 行と hover は、送り先の URL と保存先の path を落とさない（HOME の下は `~/…`・HOME の名前は出さない）。
    /// 負の対照（手で・未実行）: `texts` の前の filter（`/`・`~` で始まる値と `://` を含む値を落とす）を戻す → URL と path が無くて赤・`one` の HOME の縮めを外す → HOME が出て赤
    #[test]
    fn b54_the_card_shows_where_it_sends_and_where_it_saves() {
        let home = std::env::var("HOME").unwrap_or_default();
        let home = home.trim_end_matches('/');
        let saved = format!("{home}/Desktop/out.txt");
        let s = step("mcp.web.post", json!({ "url": "https://hooks.example.com/x", "path": saved, "also": ["~/notes.md", "body"], "body": "hi" }));
        let line = approval_line("Post it", std::slice::from_ref(&s));
        let shown_path = if home.is_empty() { saved.clone() } else { "~/Desktop/out.txt".to_string() };
        assert!(line.contains("https://hooks.example.com/x") && line.contains(&shown_path) && line.contains("~/notes.md › body") && line.contains("hi"), "{line}");
        if home.len() > 1 {
            assert!(!line.contains(&format!("{home}/")), "HOME の名前を出した: {line}");
        }
        assert_eq!(texts(&s.input).len(), 4, "hover も 4 つの値を全部: {:?}", texts(&s.input));
    }

    /// B54 の続き（operator 11-03）: 承認の 1 行・hover・chat の案の行（全部 `texts` を通る）に、URL の userinfo（入れ子の URL も）と、名前が鍵らしい query / fragment の値を出さない
    /// （名前と他の値は残す）。負の対照: `screen_text` の userinfo を落とす `replace_range` を外す → `u:p@` が出て赤・`SECRET_PARAMS` の腕を外す → `abc` が出て赤。
    #[test]
    fn b54_the_card_hides_the_secrets_in_a_url() {
        let s = step("mcp.web.post", json!({ "url": "https://u:p@host/x?token=abc&q=1#access_token=zz&state=s", "next": "go https://a/?r=https://v:w@b/c then" }));
        let line = approval_line("Post it", std::slice::from_ref(&s));
        assert!(line.contains("https://host/x?token=…&q=1#access_token=…&state=s"), "{line}");
        assert!(line.contains("go https://a/?r=https://b/c then"), "入れ子の URL の userinfo も落とす: {line}");
        for secret in ["u:p@", "abc", "zz", "v:w@"] {
            assert!(!line.contains(secret), "{secret} を出した: {line}");
        }
        assert!(texts(&s.input).iter().all(|t| !t.contains("abc") && !t.contains("u:p@")), "hover も伏せる: {:?}", texts(&s.input));
    }

    /// B54 の続き 2（operator 11-03）: 欄の名前（input の key・打つ先の欄の AX の名前 `label`）が秘密の語なら、1 行と hover にその値を出さない（名前は残す）。
    /// 普通の欄（録画の手 9 = TextEdit の書類の本文）の字は今のまま出す。
    /// 負の対照（手で・未実行）: `step_line` の `shown` の `secret_value` を外す → 「hunter2」が出て赤・`texts` の `secret_value` を外す → 「sk-live-1」が出て赤・
    /// `secret_name` を常に true に → 手 9 の字が消えて赤。
    #[test]
    fn b54_the_card_hides_what_goes_into_a_secret_field() {
        let pw = step("desktop.ax.type_text", json!({ "pid": 1, "window_id": 2, "app_name": "Mail", "label": "Password", "role": "AXTextField", "text": "hunter2" }));
        let key = step("mcp.api.call", json!({ "api_key": "sk-live-1", "q": "weather" }));
        let demo = step("desktop.ax.type_text", json!({ "pid": 3, "window_id": 4, "app_name": "TextEdit", "document": "Demo.md", "label": "", "role": "AXTextArea", "text": "# Shibubu Demo" }));
        let line = approval_line("g", &[pw.clone(), key.clone(), demo]);
        assert!(line.contains("Type “…” in Mail") && line.contains("api_key: …") && line.contains("weather"), "{line}");
        assert!(line.contains("replace all the text with “# Shibubu Demo”"), "普通の欄（手 9）は伏せない: {line}");
        let hover: Vec<String> = [&pw, &key].iter().flat_map(|s| texts(&s.input)).collect();
        for secret in ["hunter2", "sk-live-1"] {
            assert!(!line.contains(secret) && hover.iter().all(|t| !t.contains(secret)), "{secret} を出した: {line} / {hover:?}");
        }
        assert!(["API key", "Access Token", "PIN", "client_secret"].iter().all(|n| secret_name(n)), "秘密の名前を見落とした");
        assert!(!["Key", "Author", "Postal code", "Spinner", ""].iter().any(|n| secret_name(n)), "普通の名前を伏せた");
    }

    /// 署名を compile で縛る。
    #[test]
    fn w19_2_engine_view_signatures() {
        let _: fn(&mut egui::Ui, &EngineView) -> Option<EngineAct> = draw;
        let _: fn(&str, &[ApprovalStep]) -> String = approval_line;
        let _: fn(&[ApprovalStep]) -> String = consent_label;
    }

    /// OPEN-INPLACE-1101 の 2（owner 240「Need approve には always allow を聞くように」）: 承認の card は左から Always allow・今の承認の button・Cancel の 3 つ
    /// （右の 2 つの字は BG の道具が引くので今のまま）。押すと `AlwaysAllow`。判断（Decide）・同意（Consent）・agent の一言（Ask）の card には出さない。
    /// ENGINE-AUDIT W7（operator 11-03）: 押す手だけの card（`gui::asks` に当たった押す手 = 毎回聞く）は Always allow を出さず、押す名前を言う毎回聞く理由の 1 行。
    /// 打つ・menu・key の手が 1 つでも在る card は今まで通り Always allow（owner 240）。
    /// 負の対照（未測）: draw の Approve の腕から Always allow の button を外す → 1 つ目が赤・Consent の腕に足す → 同意の assert が赤・
    /// `every.is_none()` の門を外す → 押す手だけの card の assert が赤・理由の 1 行を描かない → 「Asks every time」の assert が赤。
    #[test]
    fn open_inplace_every_approval_card_offers_always_allow_first() {
        let buttons = |view: &EngineView, press: Option<&str>| {
            let ctx = egui::Context::default();
            crate::ui::theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut act = None;
            let mut frame = |events: Vec<egui::Event>| {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0))), events, ..Default::default() };
                let out = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if let Some(a) = draw(ui, view) {
                            act = Some(a);
                        }
                    });
                });
                out.platform_output.accesskit_update.expect("AccessKit の木").nodes
            };
            let nodes = frame(vec![]);
            let shown: Vec<String> = nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(String::from)).collect();
            let named: Vec<(String, egui::accesskit::NodeId, f64)> = nodes
                .iter()
                .filter(|(_, n)| n.role() == egui::accesskit::Role::Button)
                .filter_map(|(id, n)| Some((n.label()?.to_owned(), *id, n.bounds()?.x0)))
                .collect();
            if let Some(p) = press {
                let id = named.iter().find(|(l, ..)| l == p).map(|(_, id, _)| *id).expect("押す button が無い");
                frame(vec![egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target: id, data: None })]);
            }
            let mut by_x = named;
            by_x.sort_by(|a, b| a.2.total_cmp(&b.2));
            (by_x.into_iter().map(|(l, ..)| l).collect::<Vec<_>>(), act, shown)
        };
        let send = step("desktop.ax.menu", json!({ "pid": 4242, "window_id": 77, "app_name": "Example", "path": ["View", "Library"] }));
        let approve = EngineView::Approve { goal: "open the list".into(), steps: vec![send.clone()], line: None };
        assert_eq!(buttons(&approve, None).0, [ALWAYS_LABEL, APPROVE_LABEL, CANCEL_LABEL], "承認の card の button");
        assert_eq!(buttons(&approve, Some(ALWAYS_LABEL)).1, Some(EngineAct::AlwaysAllow));
        let consent = EngineView::Consent { goal: "open the list".into(), steps: vec![send], line: "x".into() };
        assert!(!buttons(&consent, None).0.iter().any(|l| l == ALWAYS_LABEL), "同意の card に Always allow");
        // 覚えられない手（宛先の app の無い手）だけの card には出さない
        let other = EngineView::Approve { goal: "save".into(), steps: vec![step("pad.save", json!({ "app_name": "Pad" }))], line: None };
        assert_eq!(buttons(&other, None).0, [APPROVE_LABEL, CANCEL_LABEL], "覚えられない手の card");
        // W7: 押す手だけ = 毎回聞く（Always allow は無い・押す名前を言う 1 行）
        let press = |label: &str| step("desktop.ax.click", json!({ "pid": 4242, "window_id": 77, "app_name": "Mail", "label": label, "role": "AXButton" }));
        let pressing = EngineView::Approve { goal: "clean up".into(), steps: vec![press("Delete"), press("Delete")], line: None };
        let (got, _, shown) = buttons(&pressing, None);
        assert_eq!(got, [APPROVE_LABEL, CANCEL_LABEL], "押す手だけの card に効かない Always allow");
        assert!(shown.iter().any(|l| l == "Asks every time before pressing “Delete”"), "毎回聞く理由の 1 行が無い（名前は 1 回）: {shown:?}");
        // 打つ手が 1 つでも在れば今まで通り
        let typed = step("desktop.ax.type_text", json!({ "pid": 4242, "app_name": "Mail", "label": "To", "text": "a@example.com" }));
        let mixed = EngineView::Approve { goal: "send it".into(), steps: vec![typed, press("Send")], line: None };
        assert_eq!(buttons(&mixed, None).0, [ALWAYS_LABEL, APPROVE_LABEL, CANCEL_LABEL], "打つ手の在る card");
        assert_eq!(asks_every_time(&[step("desktop.ax.click", json!({ "pid": 1, "app_name": "Mail" }))]).as_deref(), Some("Asks every time before this press"));
    }

    /// 承認の 1 行は宛先と送る文字を人の言葉で並べ、内部の欄（pid・窓番号・label・role・action の id・path）を出さない。
    #[test]
    fn w19_2_approval_line_shows_where_and_what_without_internal_words() {
        let set = step("desktop.ax.type_text", json!({ "pid": 4242, "window_id": 77, "app_name": "TextEdit", "label": "", "role": "AXTextArea", "text": "# README\nnonce-123" }));
        let save = step("desktop.ax.menu", json!({ "pid": 4242, "app_name": "TextEdit", "path": ["ファイル", "保存"] }));
        let line = approval_line("README に 1 行足して保存して", &[set, save]);
        assert!(line.contains("README に 1 行足して保存して"), "{line}");
        assert!(line.contains("TextEdit") && line.contains("nonce-123") && line.contains("ファイル › 保存"), "{line}");
        // 画面の操作の宛先は AI に任せる app として覚える事を言う（同じ app は 1 回）・画面の操作でない手（pid の無い手）は言わない
        assert!(line.contains("once, and hands TextEdit to the AI)") && line.matches("TextEdit to the AI").count() == 1, "{line}");
        assert!(!approval_line("x", &[step("pad.save", json!({ "app_name": "Pad" }))]).contains("to the AI"));
        assert_eq!(consent_label(&[step("desktop.ax.menu", json!({ "pid": 4242, "app_name": "TextEdit", "path": ["ファイル", "保存"] }))]), "Hand TextEdit to the AI");
        // 長い文（README の全文 + 足す 1 行）も 1 字も省かない（G79 operator 01:3x: 承認 = この内容を 1 回だけ・W19a2 の「…」は外した）
        let long = format!("# README\n{}\nnonce-456", "本文の行。".repeat(20));
        let l2 = approval_line("README に 1 行足して保存して", &[step("desktop.ax.type_text", json!({ "app_name": "TextEdit", "text": long }))]);
        assert!(l2.contains(&long.replace('\n', " ⏎ ")) && !l2.contains('…'), "{l2}");
        for internal in ["desktop.", "4242", "77", "AXTextArea", "pid", "window_id"] {
            assert!(!line.contains(internal), "{internal} を出さない: {line}");
        }
        // 反証の直し（UI-1）: 英語の一言と英語の値なら、手と値の区切りにも日本語の字（「・」U+30FB）が出ない
        let en = approval_line("add a line and save", &[
            step("desktop.ax.type_text", json!({ "app_name": "TextEdit", "text": "hello", "label": "body" })),
            step("desktop.ax.menu", json!({ "app_name": "TextEdit", "path": ["File", "Save"], "title": "Save" })),
        ]);
        let ja = en.chars().find(|c| matches!(*c, '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}'));
        assert_eq!(ja, None, "日本語の字が出る: {en}");
        assert!(en.contains("Type “hello” in TextEdit · Press File › Save in TextEdit"), "{en}");
    }

    /// G79（operator 01:3x・録画の文言）: 画面の手は人の語で、送る中身は 1 字も省かない。r20 の計画は「Send “README.md · up · cmd” to TextEdit」と
    /// 内部の値（document・key 名・mods）を並べていた → 宛先は「TextEdit's “README.md”」・置き換えは「replace “前” with “後”」・key は「⌘↑」
    #[test]
    fn g79_approval_steps_are_in_human_words_and_keep_every_character() {
        let doc = |extra: Value| {
            let mut i = json!({ "pid": 4242, "window_id": 77, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "README.md", "label": "", "role": "AXTextArea" });
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            i
        };
        let r20 = approval_line("g", &[
            step("desktop.key", doc(json!({ "key": "up", "mods": ["cmd"] }))),
            step("desktop.key", doc(json!({ "key": "right", "mods": ["shift", "cmd"] }))),
            step("desktop.ax.type_text", doc(json!({ "text": "# Shibubu Demo" }))),
            step("desktop.key", doc(json!({ "key": "s", "mods": ["cmd"] }))),
        ]);
        assert!(r20.contains("will send 4 steps: Press ⌘↑ in TextEdit's “README.md” · Press ⇧⌘→ in TextEdit's “README.md” · In TextEdit's “README.md”, replace all the text with “# Shibubu Demo” · Press ⌘S in TextEdit's “README.md” ("), "{r20}");
        let old = format!("# README {}", "長い題の続き".repeat(10));
        let g79 = approval_line("g", &[
            step("desktop.ax.type_text", doc(json!({ "replace": old, "text": "# Shibubu Demo\nby Shibubu" }))),
            step("desktop.ax.menu", doc(json!({ "path": ["File", "Save"] }))),
        ]);
        assert!(g79.contains(&format!("will send 2 steps: In TextEdit's “README.md”, replace “{old}” with “# Shibubu Demo ⏎ by Shibubu” · Press File › Save in TextEdit's “README.md” (")), "{g79}");
        for internal in ["Send “", "README.md ·", "cmd", "desktop.", "4242", "AXTextArea", "com.apple"] {
            assert!(!r20.contains(internal) && !g79.contains(internal), "{internal} を出さない: {r20} / {g79}");
        }
        // 書類でない欄に打つ手は「Type」・key だけの手も記号（Return）
        let field = approval_line("g", &[step("desktop.ax.type_text", json!({ "app_name": "Discord", "text": "hello" })), step("desktop.key", json!({ "app_name": "Discord", "key": "return" }))]);
        assert!(field.contains("Type “hello” in Discord · Press Return in Discord ("), "{field}");
    }

    /// M2 G-e: 押す手は何を押すかを言う（Discord の channel を見ずに承認しない）・menu も「Press」・1 手は単数形。
    #[test]
    fn m2_approval_line_says_what_is_pressed_and_counts_one_step() {
        let click = step("desktop.ax.click", json!({ "pid": 63, "window_id": 9001, "app_name": "Discord", "label": "general", "role": "", "match": "contains" }));
        let one = approval_line("Open the general channel", &[click.clone()]);
        assert_eq!(one, "To do “Open the general channel”, Shibubu will send 1 step: Press “general” in Discord (approving sends exactly this, once, and hands Discord to the AI)");
        let menu = step("desktop.ax.menu", json!({ "pid": 52, "app_name": "REAPER", "path": ["Track", "Insert new track"] }));
        assert!(approval_line("Add a new track", &[menu.clone()]).contains("will send 1 step: Press Track › Insert new track in REAPER ("), "{}", approval_line("Add a new track", &[menu.clone()]));
        let two = approval_line("g", &[menu, click]);
        assert!(two.contains("will send 2 steps: Press Track › Insert new track in REAPER · Press “general” in Discord ("), "{two}");
        for internal in ["contains", "9001", "AXLink", "desktop."] {
            assert!(!one.contains(internal), "{internal} を出さない: {one}");
        }
        // label の無い click は今まで通り何を押すかを言えない 1 行
        assert!(approval_line("g", &[step("desktop.ax.click", json!({ "pid": 1, "app_name": "Pad", "label": " ", "role": "AXButton" }))]).contains("Control Pad"));
    }

    /// W19-3: 解析の承認は「送ります」と言わない。operator 10-24 の形: 道具の tool の数（壊さないと申告した物だけ・申告は道具のもの）・対象の path・app の名前と版・
    /// 「送りません」（REA 6.7.0 の 5 つの tool の source を読み、通信を観測して確かめた後の約束・指摘 5 の時は確かめていなかったので書かなかった）。action の id は出さない。
    #[test]
    fn w19_3_approval_line_for_analysis_says_how_many_tools_what_it_reads_and_that_it_does_not_send() {
        let a = step("analysis.rea", json!({ "app_name": "Quillpad", "target": "/tmp/x/Quillpad.app", "version": "1.0", "scope": "壊さない", "by": "REA", "tools": ["open_binary", "inspect_artifact"] }));
        let line = approval_line("文書の題を変えて", &[a.clone()]);
        assert!(line.contains("No existing connection can do “文書の題を変えて”") && line.contains("Shibubu will read /tmp/x/Quillpad.app (Quillpad, version 1.0) using 2 tools REA says won't break anything (that claim is REA's). Nothing is sent"), "{line}");
        assert!(line.contains("doesn't start or install") && line.contains("asks you again"), "{line}");
        for internal in ["送ります:", "analysis.", "open_binary", "読むだけと申告"] {
            assert!(!line.contains(internal), "{internal} を出さない: {line}");
        }
        assert!(!line.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "rea"), "道具の id を出さない: {line}");
        let unknown = approval_line("g", &[step("analysis.rea", json!({ "app_name": "Pad", "target": "/x", "version": null }))]);
        assert!(unknown.contains("Shibubu will read /x (Pad, version unknown) using only tools the analysis tool says won't break anything (that claim is the analysis tool's)"), "{unknown}");
        let one = approval_line("g", &[step("analysis.rea", json!({ "app_name": "Pad", "by": "REA", "tools": ["open_binary"] }))]);
        assert!(one.contains("Shibubu will read Pad (version unknown) using 1 tool REA says"), "単数の形: {one}");
        // 送る手と混ざった承認（Engine は出さないが）は送る手の 1 行のまま（読むだけと言わない）
        let mixed = approval_line("g", &[a, step("desktop.ax.type_text", json!({ "app_name": "TextEdit", "text": "x" }))]);
        assert!(mixed.contains("will send"), "{mixed}");
    }

    /// 反証の直し（指摘 3）: 解析の承認の 2 つの button は「承認して調べる」「調べない」（AX の名前も同じ・BG の道具が引く名前）。送る手の承認は今まで通り。
    #[test]
    fn w19_3_analysis_buttons_say_look_not_send() {
        let a = step("analysis.rea", json!({ "app_name": "Quillpad" }));
        assert_eq!(labels(&[a.clone()]), ("Look inside", "Cancel"));
        assert_eq!(labels(&[step("desktop.ax.type_text", json!({ "app_name": "TextEdit", "text": "x" }))]), (APPROVE_LABEL, CANCEL_LABEL));
        assert_eq!(labels(&[a, step("desktop.ax.type_text", json!({}))]), (APPROVE_LABEL, CANCEL_LABEL), "送る手と混ざれば送る手の名前");
    }
}
