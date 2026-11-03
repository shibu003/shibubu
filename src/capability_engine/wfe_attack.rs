//! WF-E の反証（review の pane・branch yc/review-wfe・trunk には入れない）: WF-E が守ると言っている事を AC の外から破りに行く攻撃。
//! 偽の model と偽の Adapter だけ（本物の AI・外へ送る手・owner の login を使わない）。赤 = 主張と実物の食い違い（直しは operator が起こす pane）。
//! 緑の攻撃は、変異の build で守りを 1 か所外して赤を見る（armed-tests）。

use super::*;
use crate::adapters::sdk::{Adapter, Route, Spec};
use crate::domain::binding::Source;
use crate::domain::permission::AutomationMode;
use crate::llm::{fake::Fake, LlmError};
use std::sync::{Arc, Mutex};

const OBJ: &str = r#"{"type":"object"}"#;
const APP_ID: &str = "com.example.Notes";

/// 書いた字（input の `text` か `title`）を action の名前ごとに覚え、`attk.state.read` で全部を返す偽の Adapter。
/// 外へ効く手は 2 つ: `attk.ok.post` は送る直前に宛先を読み直すと宣言する・`attk.raw.post` は宣言しない（`Adapter::prechecks`）
struct Attk {
    state: Arc<Mutex<serde_json::Map<String, Value>>>,
    calls: Arc<Mutex<Vec<String>>>,
}
static ATTK: [Spec; 7] = [
    Spec { id: "attk.state.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    Spec { id: "attk.a.set", effect: Effect::LocalWrite, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    Spec { id: "attk.b.set", effect: Effect::LocalWrite, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    Spec { id: "attk.title.set", effect: Effect::LocalWrite, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    Spec { id: "attk.ok.post", effect: Effect::ExternalWrite, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    Spec { id: "attk.raw.post", effect: Effect::ExternalWrite, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
    // 読めない読み（答えない app と同じ = 接続の切れ）: 操作の前の読みを止める
    Spec { id: "attk.gone.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: OBJ, verifier: "read_back" },
];

impl Adapter for Attk {
    fn id(&self) -> &'static str {
        "attk"
    }
    fn kind(&self) -> AdapterKind {
        AdapterKind::LocalCore
    }
    fn route(&self) -> Route {
        Route::OfficialApi
    }
    fn provider(&self) -> &'static str {
        "attk"
    }
    fn specs(&self) -> &'static [Spec] {
        &ATTK
    }
    fn readiness(&self, _action: &str) -> CapabilityReadiness {
        CapabilityReadiness::Ready
    }
    fn sources(&self) -> Vec<Source> {
        vec![]
    }
    fn check(&mut self) {}
    fn cleanup(&mut self) {}
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        self.calls.lock().unwrap().push(action.into());
        if action == "attk.gone.read" {
            return (Outcome::Failed { error: ErrorCode::ConnectionLost }, None);
        }
        let mut state = self.state.lock().unwrap();
        if action == "attk.state.read" {
            return (Outcome::Verified { evidence: vec!["attk".into()] }, Some(sdk::observe("attk", sdk::now(), Value::Object(state.clone()))));
        }
        let put = input.get("text").or_else(|| input.get("title")).cloned().unwrap_or(Value::Null);
        state.insert(action.into(), put);
        (Outcome::Unverified { reason: "wrote".into() }, Some(sdk::observe("attk", sdk::now(), json!({ "wrote": true }))))
    }
    fn prechecks(&self, action: &str) -> bool {
        action == "attk.ok.post"
    }
}

/// 窓の一覧を読む偽の Adapter: 読みが接続の切れで落ちると、その後の readiness は Degraded（`SourceSync::record` の Offline と同じ = 接続が切れた直後）
struct Flaky {
    lost: bool,
}
static FLAKY: [Spec; 1] = [Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: OBJ, verifier: "read_back" }];

impl Adapter for Flaky {
    fn id(&self) -> &'static str {
        "desktop"
    }
    fn kind(&self) -> AdapterKind {
        AdapterKind::LocalCore
    }
    fn route(&self) -> Route {
        Route::OfficialApi
    }
    fn provider(&self) -> &'static str {
        "desktop"
    }
    fn specs(&self) -> &'static [Spec] {
        &FLAKY
    }
    fn readiness(&self, _action: &str) -> CapabilityReadiness {
        if self.lost {
            CapabilityReadiness::Degraded
        } else {
            CapabilityReadiness::Ready
        }
    }
    fn sources(&self) -> Vec<Source> {
        vec![]
    }
    fn check(&mut self) {}
    fn cleanup(&mut self) {}
    fn invoke(&mut self, _action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
        self.lost = true;
        (Outcome::Failed { error: ErrorCode::ConnectionLost }, None)
    }
}

fn attk() -> (AdapterRegistry, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut reg = AdapterRegistry::default();
    reg.register(Box::new(Attk { state: Default::default(), calls: calls.clone() })).unwrap();
    reg.check();
    (reg, calls)
}

fn step(id: &str, cap: &str, input: Value) -> WorkflowStep {
    WorkflowStep { kind: StepKind::Action, id: id.into(), capability: cap.into(), input }
}

/// `attk.state.read` の `/<書いた action>` を読み戻す check
fn read(cap: &str, expect: Expect) -> ReadBack {
    ReadBack { action: "attk.state.read".into(), input: json!({}), pointer: format!("/{cap}"), expect }
}

/// 前の run が保存した手順（目的 `goal` の操作 `op`）。`split` は AI が分けた操作の id（1 操作・欄の無い古い保存は空）
fn saved(dir: &Path, goal: &str, op: &str, steps: Vec<WorkflowStep>, checks: Vec<ReadBack>, split: &[&str]) {
    let op = Operation { id: op.into(), want: String::new(), success: String::new(), target: None, ..Default::default() };
    save_workflow(
        &dir.join(WORKFLOWS_DIR),
        &Workflow { proven: None, fails: 0, schema: WORKFLOW_SCHEMA.into(), split: split.iter().map(|s| (*s).to_string()).collect(), id: workflow_id(goal, &op), goal: goal.into(), steps, ui: None, checks },
    )
    .unwrap();
}

fn go(reg: &mut AdapterRegistry, dir: &Path, llm: Option<&dyn Llm>, automation: AutomationMode, goal: &str) -> Result<EngineRecord, EngineError> {
    let (mcp, mut an) = (McpConfig::default(), Vec::new());
    let mut cx = w19_2_body::cx(reg, dir, llm, &mcp, &[], &mut an);
    cx.automation = automation;
    run(goal, &mut cx)
}

fn interp(target: Option<&str>) -> Result<Value, LlmError> {
    Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Set a", "success": "a is set", "target": target }] }))
}

/// 前の AI の run の分け方: 1 つの目的を 2 つの操作に
fn two_ops() -> Result<Value, LlmError> {
    Ok(json!({ "operations": [
        { "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "Set a", "success": "a is set", "target": null },
        { "kind": "act", "named": false, "candidates": [], "id": "op2", "want": "Set b", "success": "b is set", "target": null },
    ] }))
}

fn none(why: &str) -> Result<Value, LlmError> {
    Ok(json!({ "none_why": why, "workflow_steps": [], "checks": [], "recipe": null, "trial": null }))
}

fn input(fake: &Fake, k: usize) -> Value {
    serde_json::from_str(&fake.requests.lock().unwrap()[k].input).unwrap()
}

fn has_ja(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}'))
}

/// 撃つ所 1（WF-006）: 「宣言の無い外へ効く手が在れば 1 手も送らない（nothing was sent）」は run 全体で真か。
/// 2 つの操作の run: 1 つ目 = 宛先を読み直すと宣言した外へ効く手・2 つ目 = 宣言の無い外へ効く手。門は `run_steps`（操作ごと）なので
/// 1 つ目は送られ、`execute` の Unchecked の腕は前の操作で走った手を見ずに「nothing was sent」を返すかを見る。
#[test]
fn wfe_x1_a_later_action_without_a_recheck_never_says_nothing_was_sent_after_an_earlier_one_went_out() {
    let goal = "Post a, then post b";
    let d = w19_2_body::tmp("wfe-x1");
    saved(&d, goal, "op1", vec![step("p1", "attk.ok.post", json!({ "text": "a" }))], vec![read("attk.ok.post", Expect::Equals { value: json!("a") })], &["op1", "op2"]);
    saved(&d, goal, "op2", vec![step("p2", "attk.raw.post", json!({ "text": "b" }))], vec![read("attk.raw.post", Expect::Equals { value: json!("b") })], &["op1", "op2"]);
    let (mut reg, calls) = attk();
    let fake = Fake::new(vec![two_ops()]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), goal);
    let sent: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".post")).cloned().collect();
    assert!(!sent.iter().any(|c| c == "attk.raw.post"), "宣言の無い手は送らない（前提）: {sent:?}");
    let Err(e @ EngineError::Denied(_)) = &r else { panic!("宣言の無い手で止まらない（前提）: {r:?}") };
    let line = crate::engine_page::failure_line(e);
    assert!(line.contains("“p2”"), "2 つ目の操作の手で止まった（前提）: {line}");
    assert!(
        !(line.contains("nothing was sent") && !sent.is_empty()),
        "1 つ目の操作の外へ効く手を送った後で「nothing was sent」: 送った手 = {sent:?}・画面の 1 行 = {line}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 2（WF-007）: 「書く手の Verified は頼まれた差分の読み戻しだけ（TODO: 全ての書く手で）」。`text` 以外の key（Quillpad の `title` の形）で
/// 字を置く書く手を、含むだけの読み戻しで済みにしないか（`unasked` は input の `text` だけを見る）。対照: `text` の手は Partial（規則が効いている）。
#[test]
fn wfe_x2_a_write_that_puts_text_under_another_key_is_not_done_on_a_contains_check() {
    let goal = "Rename the doc";
    for (key, cap) in [("text", "attk.a.set"), ("title", "attk.title.set")] {
        let d = w19_2_body::tmp("wfe-x2");
        let mut put = json!({});
        put[key] = json!("Shibubu Demo");
        saved(&d, goal, "op1", vec![step("set", cap, put)], vec![read(cap, Expect::Contains { text: "Demo".into() })], &[]);
        let (mut reg, calls) = attk();
        let r = go(&mut reg, &d, None, AutomationMode::default(), goal).unwrap_or_else(|e| panic!("{key}: {e:?}"));
        assert!(calls.lock().unwrap().iter().any(|c| c == cap), "{key}: 書く手が走った（前提）");
        assert!(r.partial.is_some(), "input の `{key}` に字を置く書く手を、含むだけの読み戻しで済みにした: verified = {:?}", r.verified);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// 撃つ所 3（WF-010）: 2 回とも手順 0 → 依頼は 3 回（interpret 1・計画 2）で止まり、model の none_why の文字列は画面の 1 行にも tried にも出ない。
/// 4 つ目の答えは走れる計画（上限が外れていれば使われ、依頼の数が赤になる）。
#[test]
fn wfe_x3_two_empty_plans_stop_after_one_rebuild_and_keep_the_model_reason_off_screen() {
    let d = w19_2_body::tmp("wfe-x3");
    let (mut reg, calls) = attk();
    let plan = Ok(json!({ "none_why": null, "workflow_steps": [{ "id": "s1", "capability": "attk.a.set", "input_json": json!({ "text": "a" }).to_string() }],
        "checks": [{ "action": "attk.state.read", "input_json": "{}", "pointer": "/attk.a.set", "expect": "equals", "value_json": "\"a\"" }], "recipe": null, "trial": null }));
    let fake = Fake::new(vec![interp(None), none("WHY-ONE /Users/someone/notes.txt"), none("WHY-TWO"), plan]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Set a");
    assert_eq!(fake.requests.lock().unwrap().len(), 3, "作り直しは 1 回だけ（3 つ目の計画を聞かない）: {r:?}");
    let Err(e @ EngineError::NoMeans { tried, .. }) = &r else { panic!("2 回とも手順 0 なら手段が無い: {r:?}") };
    let line = crate::engine_page::failure_line(e);
    assert!(!line.contains("WHY-") && tried.iter().all(|t| !t.contains("WHY-")), "model の none_why が画面か tried に出た: {line} / {tried:?}");
    assert!(calls.lock().unwrap().iter().all(|c| c == "attk.state.read"), "何も書いていない: {:?}", calls.lock().unwrap());
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 4（WF-008・009・011）: 宛先の窓の読みが接続の切れで落ちた直後（Adapter の readiness はもう Degraded）に、計画の input の
/// `registry_actions` が窓を読む action を「ready」と渡さないか（`generate` は readiness を `facts` の前に読む）。
#[test]
fn wfe_x4_a_window_read_that_lost_the_connection_is_not_passed_to_the_plan_as_ready() {
    let d = w19_2_body::tmp("wfe-x4");
    let mut reg = AdapterRegistry::default();
    reg.register(Box::new(Flaky { lost: false })).unwrap();
    reg.check();
    let fake = Fake::new(vec![interp(Some(APP_ID)), none("no window"), none("still no window")]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Rename the note");
    assert!(matches!(r, Err(EngineError::NoMeans { .. })), "{r:?}");
    let i = input(&fake, 1);
    assert_eq!(i["windows_fact"]["status"], json!("read_failed"), "窓の読みが落ちた（前提）: {}", i["windows_fact"]);
    let now = reg.actions().into_iter().find(|a| a.id == "desktop.windows.list").map(|a| json!(a.readiness));
    assert_ne!(now, Some(json!("ready")), "読みの後の Adapter の readiness（前提）");
    let a = i["registry_actions"].as_array().unwrap().iter().find(|a| a["id"] == "desktop.windows.list").cloned().unwrap_or_default();
    assert_ne!(a["readiness"], json!("ready"), "窓の読みが接続の切れで落ちたのに、計画の input は窓を読む action を ready と渡した（今の readiness = {now:?}）: {a}");
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 6（RQ-026）: 承認の card の 2 行目（範囲）が実際の手と食い違わないか。README の 1 か所と Notes の書類の全文を替える 2 手の card で、
/// 「全文を替える」の宛先が README になり、Notes が出ない形にならないか（`scope_line` は宛先を最初の type_text から取る）。
#[test]
fn wfe_x5a_the_scope_line_names_the_document_whose_text_is_all_replaced() {
    use crate::ui::engine_view::{scope_line, ApprovalStep};
    let s = |action: &str, input: Value| ApprovalStep { action: action.into(), input, digest: String::new(), scopes: vec![] };
    let readme = json!({ "app_name": "TextEdit", "document": "README.md", "replace": "# old", "text": "# new" });
    let notes = json!({ "app_name": "Notes", "document": "Shopping", "text": "milk" });
    assert_eq!(scope_line(&[s("desktop.ax.type_text", readme.clone())]).as_deref(), Some("Changes 1 place in TextEdit's “README.md”; the rest stays the same"), "対照: 1 つの書類の 1 か所");
    let line = scope_line(&[s("desktop.ax.type_text", readme), s("desktop.ax.type_text", notes)]).unwrap_or_default();
    assert!(
        line.contains("Notes") && !line.contains("Replaces all the text in TextEdit"),
        "README は 1 か所だけ・全文を替えるのは Notes の “Shopping” なのに: {line}"
    );
}

/// 撃つ所 6（RQ-026）: 画面の手（desktop.*）が押すだけで、同じ card の他の手が字を送る時、2 行目が「nothing is typed」と言わないか。
#[test]
fn wfe_x5b_the_scope_line_does_not_say_nothing_is_typed_when_another_step_sends_text() {
    use crate::ui::engine_view::{scope_line, ApprovalStep};
    let s = |action: &str, input: Value| ApprovalStep { action: action.into(), input, digest: String::new(), scopes: vec![] };
    let save = json!({ "app_name": "TextEdit", "document": "README.md", "path": ["File", "Save"] });
    let post = json!({ "text": "Hello from Shibubu" });
    let line = scope_line(&[s("desktop.ax.menu", save), s("attk.raw.post", post)]).unwrap_or_default();
    assert!(!line.contains("nothing is typed"), "同じ card の 2 つ目の手が字を送るのに: {line}");
}

/// 撃つ所 5（WF-036）: AI が時間切れの run は AI 無しの道（目的をそのまま 1 つの操作 = op1）に戻る。前の AI の run が 2 つの操作に分けて保存した目的で、
/// op1 の保存した手順だけが走って済み（Ok）にならないか（`workflow_id` は目的と操作の id から作る）。前提: AI が答える run は 2 つとも走る。
#[test]
fn wfe_x6_without_ai_a_goal_saved_as_two_actions_is_not_done_after_only_the_first() {
    let goal = "Set a and b";
    let d = w19_2_body::tmp("wfe-x6");
    saved(&d, goal, "op1", vec![step("a", "attk.a.set", json!({ "text": "a" }))], vec![read("attk.a.set", Expect::Equals { value: json!("a") })], &["op1", "op2"]);
    saved(&d, goal, "op2", vec![step("b", "attk.b.set", json!({ "text": "b" }))], vec![read("attk.b.set", Expect::Equals { value: json!("b") })], &["op1", "op2"]);
    let (mut reg, calls) = attk();
    let fake = Fake::new(vec![two_ops()]);
    go(&mut reg, &d, Some(&fake), AutomationMode::default(), goal).unwrap_or_else(|e| panic!("AI の run（前提）: {e:?}"));
    assert!(["attk.a.set", "attk.b.set"].iter().all(|w| calls.lock().unwrap().iter().any(|c| c == w)), "AI の run は 2 つとも走る（前提）: {:?}", calls.lock().unwrap());

    let (mut reg, calls) = attk();
    let fake = Fake::new(vec![Err(LlmError::Timeout { ms: 60_000 })]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), goal);
    match &r {
        Ok(rec) => {
            assert!(rec.without_ai.is_some(), "AI 無しの道に戻った（前提）: {rec:?}");
            let ran_b = calls.lock().unwrap().iter().any(|c| c == "attk.b.set");
            assert!(ran_b, "AI 無しの道で 1 つ目の操作の手順だけを走らせて済みにした: 走った手 = {:?}・Runs の行 = {:?}", calls.lock().unwrap(), crate::engine_page::history_lines(rec));
        }
        // 段 1b の形（operator と決めた）: op1 も走らせずに止め、WF-036 の「Nothing was sent」の 1 行 = 止めた run は 1 手も送っていない事
        Err(e) => {
            let sent: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
            assert!(sent.is_empty(), "AI 無しの道で止めたのに手を送った（後で頼み直すと同じ手をもう一度送る）: {sent:?}・{}", crate::engine_page::failure_line(e));
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 5 の 2（WF-036・段 1b の x6 の直しの範囲の外）: model が初めから無い run（CLI が無い・login が切れた = `llm: None`）も、目的をそのまま op1 にする。
/// 前の AI の run が 2 つの操作に分けて保存した目的で、op1 の保存した手順だけが走って済み（Ok）にならないか（直しの門は `without_ai` = AI が失敗した時だけ）。
#[test]
fn wfe_x6b_with_no_model_at_all_a_goal_saved_as_two_actions_is_not_done_after_only_the_first() {
    let goal = "Set a and b";
    let d = w19_2_body::tmp("wfe-x6b");
    saved(&d, goal, "op1", vec![step("a", "attk.a.set", json!({ "text": "a" }))], vec![read("attk.a.set", Expect::Equals { value: json!("a") })], &["op1", "op2"]);
    saved(&d, goal, "op2", vec![step("b", "attk.b.set", json!({ "text": "b" }))], vec![read("attk.b.set", Expect::Equals { value: json!("b") })], &["op1", "op2"]);
    let (mut reg, calls) = attk();
    let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
    let ran: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
    assert!(
        r.is_err() || ran.iter().any(|c| c == "attk.b.set"),
        "model が無い run で 1 つ目の操作の手順だけを走らせて済みにした: 走った手 = {ran:?}・Runs の行 = {:?}",
        r.as_ref().map(crate::engine_page::history_lines)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 5 の 3（WF-036）: 前の AI の run が目的を 2 つに分け、op1 の手順を保存した後に op2 の操作の前の読みで止まる。
/// 保存は op1 だけだが `split` は op1+op2。その後の model が無い run が、op1 だけを走らせて済みにしないか。
#[test]
fn wfe_x6c_a_goal_whose_second_action_stopped_before_its_save_is_not_done_after_only_the_first_without_ai() {
    let goal = "Set a and b";
    let d = w19_2_body::tmp("wfe-x6c");
    let plan = |cap: &str, text: &str, check: &str| -> Result<Value, LlmError> {
        Ok(json!({ "none_why": null, "workflow_steps": [{ "id": "s1", "capability": cap, "input_json": json!({ "text": text }).to_string() }],
            "checks": [{ "action": check, "input_json": "{}", "pointer": format!("/{cap}"), "expect": "equals", "value_json": json!(text).to_string() }], "recipe": null, "trial": null }))
    };
    let (mut reg, calls) = attk();
    let fake = Fake::new(vec![two_ops(), plan("attk.a.set", "a", "attk.state.read"), plan("attk.b.set", "b", "attk.gone.read")]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), goal);
    assert!(matches!(&r, Err(EngineError::Denied(_))), "AI の run は op2 の操作の前の読みで止まる（前提）: {r:?}");
    assert!(!calls.lock().unwrap().iter().any(|c| c.ends_with(".set")), "AI の run は何も書いていない（前提）: {:?}", calls.lock().unwrap());
    let kept: Vec<_> = std::fs::read_dir(d.join(WORKFLOWS_DIR)).map_or(Vec::new(), |e| e.filter_map(|f| f.ok()).map(|f| f.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect());
    assert_eq!(kept.len(), 1, "保存は op1 の手順だけ（前提）");
    let wf: Workflow = serde_json::from_slice(&std::fs::read(&kept[0]).unwrap()).unwrap();
    assert_eq!(wf.split, ["op1".to_string(), "op2".to_string()], "AI の run が分けた id を op1 の保存に残す（前提）: {:?}", wf.split);

    let (mut reg, calls) = attk();
    let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
    let ran: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
    assert!(
        r.is_err() || ran.iter().any(|c| c == "attk.b.set"),
        "AI が 2 つに分けた目的を、model が無い run が 1 つ目の操作だけで済みにした: 走った手 = {ran:?}・Runs の行 = {:?}",
        r.as_ref().map(crate::engine_page::history_lines)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 5 の 4（WF-036）: `split` の欄が無い古い保存（空は書き出さない = 前の版の file）。
/// 1 操作だけの古い保存は今まで通り走る。同じ目的の 2 つ目の file が欄無しで残っていれば、model 無しの run は 1 つ目だけで済みにしない
/// （門が split の中だけを数えて、保存した手順の id を落とす穴）。
#[test]
fn wfe_x6d_an_old_save_with_no_split_field_is_not_done_after_only_the_first_when_another_action_remains() {
    let goal = "Set a and b";
    let plant = |label: &str, op: &str, cap: &str, text: &str| {
        let d = w19_2_body::tmp(label);
        saved(&d, goal, op, vec![step(op, cap, json!({ "text": text }))], vec![read(cap, Expect::Equals { value: json!(text) })], &[]);
        let raw = std::fs::read_dir(d.join(WORKFLOWS_DIR)).unwrap().find_map(|e| e.ok()).map(|e| std::fs::read_to_string(e.path()).unwrap()).unwrap();
        assert!(!raw.contains("\"split\""), "空の split を書き出している（古い保存の形でない）: {raw}");
        d
    };
    let d = plant("wfe-x6d-one", "op1", "attk.a.set", "a");
    let (mut reg, calls) = attk();
    let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
    let ran: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
    assert!(r.is_ok() && ran == ["attk.a.set"], "split の無い 1 操作の古い保存を model 無しの run が止めたり別の手を送った: 走った手 = {ran:?}・{r:?}");
    let _ = std::fs::remove_dir_all(&d);

    let d = plant("wfe-x6d-two", "op1", "attk.a.set", "a");
    saved(&d, goal, "op2", vec![step("b", "attk.b.set", json!({ "text": "b" }))], vec![read("attk.b.set", Expect::Equals { value: json!("b") })], &[]);
    let (mut reg, calls) = attk();
    let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
    let ran: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
    assert!(
        r.is_err() || ran.iter().any(|c| c == "attk.b.set"),
        "split の無い古い保存が 2 操作分残っているのに、1 つ目だけで済みにした: 走った手 = {ran:?}・Runs の行 = {:?}",
        r.as_ref().map(crate::engine_page::history_lines)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 5 の 5（WF-036）: 保存した手順の id が、どの `split` の一覧にも無い。
/// op1 の一覧は op1 だけ、op2 の一覧は op1 だけ（op2 は一覧に無い）。門が split だけを見て file の id を落とすと、op1 だけ走って済みになる。
#[test]
fn wfe_x6e_a_saved_action_whose_id_is_absent_from_split_is_not_done_after_only_the_first() {
    let goal = "Set a and b";
    let d = w19_2_body::tmp("wfe-x6e");
    saved(&d, goal, "op1", vec![step("a", "attk.a.set", json!({ "text": "a" }))], vec![read("attk.a.set", Expect::Equals { value: json!("a") })], &["op1"]);
    saved(&d, goal, "op2", vec![step("b", "attk.b.set", json!({ "text": "b" }))], vec![read("attk.b.set", Expect::Equals { value: json!("b") })], &["op1"]);
    let (mut reg, calls) = attk();
    let r = go(&mut reg, &d, None, AutomationMode::default(), goal);
    let ran: Vec<String> = calls.lock().unwrap().iter().filter(|c| c.ends_with(".set")).cloned().collect();
    assert!(
        r.is_err() || ran.iter().any(|c| c == "attk.b.set"),
        "split に無い操作の id を落として、1 つ目だけで済みにした: 走った手 = {ran:?}・Runs の行 = {:?}",
        r.as_ref().map(crate::engine_page::history_lines)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 6（RQ-023）: Suggest は「計画と操作の前の読みまでで 1 手も送らない」。Automation の門（Execution の前）より先の Verification の段で、
/// 生成した Adapter の試験（書く手を MCP の server へ・写しの上で）が走り、Adapter が確かめ済みで保存されないか。
#[test]
fn wfe_x7_suggest_runs_no_generated_adapter_before_the_automation_gate() {
    use w19_2_body::{quill_answer, quill_cfg, title_on_disk, write_doc, QUILL_APP};
    let d = w19_2_body::tmp("wfe-x7");
    let log = d.join("mcp.jsonl");
    let real = d.join("real").join("notes.quill.json");
    write_doc(&real, "Old");
    let o = Operation { id: "op1".into(), want: "x".into(), success: "y".into(), target: Some(QUILL_APP.into()), ..Default::default() };
    let tdir = trial_dir(&d, &o);
    let mcp = quill_cfg("", &log);
    let goal = format!("Rename the Quillpad doc {} to Shibubu Demo", real.display());
    let interp = Ok(json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": goal, "success": "The title is Shibubu Demo", "target": QUILL_APP }] }));
    let fake = Fake::new(vec![interp, Ok(quill_answer(&real, &tdir.join("notes.quill.json"), "Shibubu Demo"))]);
    let (mut reg, mut an) = (AdapterRegistry::default(), Vec::new());
    let mut cx = w19_2_body::cx(&mut reg, &d, Some(&fake), &mcp, &[], &mut an);
    cx.automation = AutomationMode::Suggest;
    let r = run(&goal, &mut cx);
    assert!(matches!(&r, Err(EngineError::Denied(x)) if x.why.contains("Suggest")), "Suggest の門で止まった（前提）: {r:?}");
    assert_eq!(title_on_disk(&real), "Old", "本物は変えない（前提）");
    let copy = tdir.join("notes.quill.json");
    let tried = copy.is_file() && title_on_disk(&copy) == "Shibubu Demo";
    let kept = d.join(CAPABILITIES_DIR).join("quillpad.doc.title.set.json").is_file();
    assert!(
        !tried && !kept,
        "Suggest なのに生成した Adapter の書く手を MCP の server へ送り（写しの題 = Shibubu Demo: {tried}）、確かめ済みの能力として保存した（{kept}）・mcp の記録 = {} 行",
        std::fs::read_to_string(&log).unwrap_or_default().lines().count()
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 5（WF-036）: 不正な JSON（AI の答えの手の input が JSON でない）で AI 無しの道に戻った時の 1 行は画面の言葉（英語・UI-2）か。
/// Engine の Model の error の原文（日本語）を括弧の前まで画面に出さないか（`ai_why` は括弧を落とすだけ・`plain` は日本語を通す）。
#[test]
fn wfe_x8_the_without_ai_line_from_a_bad_json_answer_is_in_the_screen_language() {
    let d = w19_2_body::tmp("wfe-x8");
    let (mut reg, calls) = attk();
    let bad = Ok(json!({ "none_why": null, "workflow_steps": [{ "id": "s1", "capability": "attk.a.set", "input_json": "{" }], "checks": [], "recipe": null, "trial": null }));
    let fake = Fake::new(vec![interp(None), bad]);
    let r = go(&mut reg, &d, Some(&fake), AutomationMode::default(), "Set a");
    let Err(e @ EngineError::NoMeans { tried, .. }) = &r else { panic!("AI 無しの道でも手段が無い（前提）: {r:?}") };
    assert!(tried.iter().any(|t| t.starts_with(AI_NOT_USED)), "AI 無しの道に戻った（前提）: {tried:?}");
    assert!(calls.lock().unwrap().is_empty(), "何も送っていない（前提）");
    let line = crate::engine_page::failure_line(e);
    assert!(!has_ja(&line), "画面の 1 行に Engine の error の原文（日本語）が出た: {line}");
    let _ = std::fs::remove_dir_all(&d);
}

/// 撃つ所 9 の材料（app の試験 `src/app_wfe_attack.rs` から・本物の `CoreEngine` に渡す）: 前の run が保存した 1 手の手順
/// （目的 `goal` の op1 = `attk.a.set` に "a"・読み戻しは equals）と、run ごとに registry を作る口（送った手は `calls` に）
pub(crate) fn nine(dir: &Path, goal: &str, calls: Arc<Mutex<Vec<String>>>) -> Arc<dyn Fn() -> AdapterRegistry + Send + Sync> {
    std::fs::create_dir_all(dir.join(WORKFLOWS_DIR)).unwrap();
    saved(dir, goal, "op1", vec![step("a", "attk.a.set", json!({ "text": "a" }))], vec![read("attk.a.set", Expect::Equals { value: json!("a") })], &[]);
    Arc::new(move || {
        let mut reg = AdapterRegistry::default();
        reg.register(Box::new(Attk { state: Default::default(), calls: calls.clone() })).unwrap();
        reg
    })
}
