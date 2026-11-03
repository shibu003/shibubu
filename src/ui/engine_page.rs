//! W19-2 の配線（図 §33）: `Generated` の `PageEngine` の製品の実装（`capability_engine::run`）と、Engine の答えを人の言葉にする所
//! （拒否・失敗の 1 行・rail の実行履歴）。file は src/ui に置く（check_ui_copy の対象）が、main.rs の mod（ui は Engine を持たない試験の crate にも入る）。
//! 画面に出すのは人の言葉だけ: 段の名前・path・action の id・error の原文は出さない。

use crate::adapters::{AdapterRegistry, Caller};
use crate::capability_engine::{self as engine, Asked, Decision, EngineContext, EngineError, EngineRecord, EnvFacts, PendingApproval, RunsTable};
use crate::domain::capability::{AdapterKind, DisplayTier, Verification, EXTENSIONS_DIR};
use crate::domain::project::ProjectResource;
use crate::generated::{EngineAnswer, EngineReply, EngineRequest, EngineWaiting, PageEngine, ReadMiss, RunStart, CLASSIFY_NONE};
use crate::mcp::McpConfig;
use crate::storage::Store;
use crate::surface::AppIdentity;
use crate::ui::engine_view::{ApprovalStep, DecideOption};
use crate::ui::states;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 製品の Page の Engine: run ごとに registry を作り（`add_recipes(extensions/)` → `check`）、`capability_engine::run` の答えを `EngineAnswer` にする。
/// Wave: W19-2
/// 例: `g.set_engine(EngineSetup { engine: Arc::new(CoreEngine { registry: Arc::new(AdapterRegistry::with_engine_adapters), data_dir: dir.clone(),
///       mcp: mcp::load_engine_config().unwrap_or_default(), env, readback: capability_engine::READBACK_WAIT, act_consents: surface::act_consent_path() }), data_dir: dir })`（app.rs）
pub struct CoreEngine {
    /// run ごとに registry を作る口（Engine は `&mut` を持つ）。製品は `AdapterRegistry::with_engine_adapters`
    pub registry: Arc<dyn Fn() -> AdapterRegistry + Send + Sync>,
    /// 利用者の data dir（`capabilities/`・`extensions/`・`workflows/`・`trials/` の親）
    pub data_dir: PathBuf,
    /// Engine が起こしてよい MCP の server（`mcp::load_engine_config`）
    pub mcp: McpConfig,
    pub env: EnvFacts,
    /// `EngineContext::readback`（製品は `READBACK_WAIT`）
    pub readback: Duration,
    /// app ごとの AI の同意の file（製品は `surface::act_consent_path`・`MacGui` の門と旧 Desktop の画面が読む 1 つの鍵）。
    /// `EngineRequest::grants` の宛先をここに書く（W19-1 の残り・operator 2026-10-24）
    pub act_consents: PathBuf,
}

/// OPEN-INPLACE-1101 の 5: Preview で作った plan の操作（data dir・Project の app・一言・操作）。`CoreEngine::plan` が書き、Build の `run` が 1 回だけ取り出す。
/// ponytail: process の中だけ・8 件まで（再起動の後の Build と、Project の app が開いていない時の承認の後の run は interpret を今まで通り 1 回呼ぶ）
static PLANNED: std::sync::Mutex<Vec<(PathBuf, String, String, Vec<engine::Operation>)>> = std::sync::Mutex::new(Vec::new());
const PLANNED_MAX: usize = 8;

/// OPEN-INPLACE-1101 の 5: plan の手順を chat の案の読むだけの行に。送る手は承認の card と同じ字（`engine_view::step_line`）・読む手は何を読むか・
/// 選ぶ手は問い・同じ行は 1 つ・最後に読み戻しの 1 行（確かめ方が在れば）。
fn plan_lines(workflows: &[engine::Workflow], actions: &[crate::domain::action::ActionDefinition]) -> Vec<String> {
    use crate::domain::action::Effect;
    let reads = |id: &str| actions.iter().any(|a| a.id == id && matches!(a.effect, Effect::LocalRead | Effect::ExternalRead));
    let mut lines: Vec<String> = Vec::new();
    for s in workflows.iter().flat_map(|w| &w.steps) {
        let app = s.input["app_name"].as_str().filter(|a| !a.trim().is_empty()).unwrap_or("the app");
        let line = match &s.kind {
            engine::StepKind::Decide { question, .. } => format!("Ask you: {question}"),
            _ if reads(&s.capability) => format!("Read {app}"),
            // W7: 承認の要らない押す手（reveal）も押す手の字（「Press “…” in …」）
            _ => {
                let action = if s.capability == engine::REVEAL_ACTION { engine::CLICK_ACTION.to_owned() } else { s.capability.clone() };
                crate::ui::engine_view::step_line(&ApprovalStep { action, input: s.input.clone(), digest: String::new(), scopes: Vec::new() })
            }
        };
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if workflows.iter().any(|w| !w.checks.is_empty()) {
        lines.push("Then Shibubu reads it back to check it worked".into());
    }
    lines
}

impl PageEngine for CoreEngine {
    fn run(&self, req: EngineRequest) -> EngineAnswer {
        // 前に作った Adapter（利用者の data の recipe）を載せてから readiness を読む（EngineContext の doc の順）
        let mut reg = self.registry_with_recipes();
        reg.check();
        // 答えない app を待つ間の 1 行を Page へ（MacGui の読み直し・承認の宛先の決め直しも同じ registry を通る・W19-3b G2）
        reg.set_progress(req.progress.clone());
        // ENGINE-ADJUST-1102 ①: 人の Stop の印を Adapter にも（MacGui = bridge の more の続きの前に見る）
        reg.set_stop(req.stop.clone());
        self.grant(&mut reg, &req);
        // W19-3: 利用者が Shibubu の MCP の設定に入れた解析の道具（REA）。無ければ空で、解析の段は飛ばす
        let mut analysis = engine::analysis_backends(&self.mcp);
        // REA を起こす・読む間の 1 行を Page へ（W19-3-fix・G2 の `reg.set_progress` と同じ口）
        for b in analysis.iter_mut() {
            b.set_progress(req.progress.clone());
        }
        let caller = Caller { app_id: &req.app_id, run_id: Some(&req.run_id) };
        // ENGINE-GENERIC-1101 追記 234: 状況（窓を開いている app の 1 行も）は `Generated` が chat と同じ塊にして渡す（`with_windows` → `windows`）
        let situation = req.situation.clone();
        // ENGINE-AUDIT W1: 段の記録を集める（止まった run でも `engine-runs.jsonl` の error の行に `stages` を書く）
        let stages = std::sync::Mutex::new(Vec::new());
        let push = |s: &engine::StageLog| stages.lock().unwrap_or_else(|e| e.into_inner()).push(s.clone());
        let mut cx = EngineContext {
            registry: &mut reg,
            caller,
            consents: &req.consents,
            data_dir: &self.data_dir,
            llm: req.llm.as_deref(),
            mcp: &self.mcp,
            env: self.env.clone(),
            analysis: analysis.as_mut_slice(),
            readback: self.readback,
            target: req.target.as_deref(),
            document: req.document.as_deref(),
            document_file: req.document_file.as_deref(),
            window: req.window.as_ref(),
            repo: req.repo.as_deref(),
            progress: Some(&*req.progress),
            generate_calls: 0,
            trace: std::env::var(engine::QA_ENV).is_ok_and(|v| v == "1"),
            // WF1: run を Project の app の session の runs 表に（手ごとに write-ahead・止まった所から `resume`）
            runs: req.runs.as_ref().map(|r| RunsTable { store: &r.store, session_id: &r.session_id, space_id: &r.space_id, app_version: r.app_version }),
            automation: req.automation,
            // ENGINE-GENERIC-1101: 一言と一緒の今の状況（Page の状況 ＋ 部品が渡した物 ＋ 窓を開いている app）
            situation: &situation,
            // ENGINE-ADJUST-1102 ①: 人の Stop の印（段の間・AI の前後・手の前・読み戻しの待ちで見る）
            stop: &req.stop,
            staged: Some(&push),
        };
        // WF2: 止まった run の続き（承認か選んだ答え）は cursor から・interpret を呼び直さない
        let got = match req.runs.as_ref().map(|r| &r.start) {
            Some(RunStart::Resume { choice }) => engine::resume(&req.goal, &req.run_id, choice.as_deref(), &mut cx),
            // OPEN-INPLACE-1101 の 5: 同じ一言の Preview の plan が在ればその操作で（interpret を呼ばない・保存した手順を拾う = 生成 0 回）
            _ => engine::run_with(&req.goal, self.planned(&req.app_id, &req.goal), &mut cx),
        };
        let mut a = answer(got);
        // ENGINE-AUDIT W1: 止まった run の行にも段（順・ms・数字）を残す（済んだ run は record の `stages` に在る）
        if a.log.0 == "error" {
            a.log.1["stages"] = serde_json::to_value(stages.into_inner().unwrap_or_else(|e| e.into_inner())).unwrap_or_default();
        }
        a
    }

    /// OPEN-INPLACE-1101 の 5（`PageEngine::plan` の doc）: `capability_engine::plan` で手順を作って保存し（何も送らない・書類を開く手も打たない = document は渡さない）、
    /// 分けた操作を Build の `run` のために覚え、手順を chat の案の読むだけの行にする（`plan_lines`）。
    fn plan(&self, req: EngineRequest) -> Result<Vec<String>, String> {
        let mut reg = self.registry_with_recipes();
        reg.check();
        let goal = req.goal.trim();
        let got = {
            let mut cx = EngineContext {
                registry: &mut reg,
                caller: Caller { app_id: &req.app_id, run_id: None },
                consents: &[],
                data_dir: &self.data_dir,
                llm: req.llm.as_deref(),
                mcp: &self.mcp,
                env: self.env.clone(),
                analysis: &mut [],
                readback: self.readback,
                target: req.target.as_deref(),
                document: None,
                document_file: req.document_file.as_deref(),
                window: req.window.as_ref(),
                repo: req.repo.as_deref(),
                progress: Some(&*req.progress),
                generate_calls: 0,
                trace: false,
                runs: None,
                automation: req.automation,
                situation: &req.situation,
                stop: &req.stop,
                staged: None,
            };
            engine::plan(goal, &mut cx)
        };
        let (ops, workflows) = got.map_err(|e| failure_line(&e))?;
        let lines = plan_lines(&workflows, &reg.actions());
        let mut all = PLANNED.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|(d, a, g, _)| !(*d == self.data_dir && *a == req.app_id && g == goal));
        all.push((self.data_dir.clone(), req.app_id.clone(), goal.to_owned(), ops));
        if all.len() > PLANNED_MAX {
            all.remove(0);
        }
        Ok(lines)
    }

    /// ENGINE-AUDIT W9（`PageEngine::classify` の doc）: interpret だけ（AI 1 回・窓は読まない・何も送らない）。分けた操作は `PLANNED` に覚え、同じ一言の次の
    /// `run` が `planned` で拾う（interpret を呼び直さない）。AI が無ければ Err（AI 無しの 1 操作 = 種類は分からない）。
    fn classify(&self, req: EngineRequest) -> Result<Vec<crate::domain::chat::Classified>, String> {
        let Some(llm) = req.llm.as_deref() else { return Err(CLASSIFY_NONE.into()) };
        let goal = req.goal.trim();
        let mut ops = engine::interpret(goal, &req.situation, Some(llm), &req.stop).map_err(|e| failure_line(&e))?;
        // B86: run と同じ決まりで Page の app（名指しの書類の Page の app も = `EngineRequest::target`）を宛先に（run なら埋めて進む物を先に聞かない）
        engine::fill_page_target(&mut ops, req.target.as_deref());
        // W9 本体: 種類・want・宛先・名指し・候補を chat に渡す（chat が候補 2 つ以上・名指しで宛先無しを 1 回聞く）。
        // B86: chat に渡す宛先と候補は人に見せる app の名前（bundle id を画面に出さない・覚えた操作〈`PLANNED`〉は bundle id のまま）
        let name = |id: &str| crate::domain::chat::app_label(id, &req.situation.facts);
        let kinds = ops
            .iter()
            .map(|o| crate::domain::chat::Classified { kind: o.kind, want: o.want.clone(), target: o.target.as_deref().map(name), named: o.named, candidates: o.candidates.iter().map(|c| name(c)).collect() })
            .collect();
        let mut all = PLANNED.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|(d, a, g, _)| !(*d == self.data_dir && *a == req.app_id && g == goal));
        all.push((self.data_dir.clone(), req.app_id.clone(), goal.to_owned(), ops));
        if all.len() > PLANNED_MAX {
            all.remove(0);
        }
        Ok(kinds)
    }

    /// OPEN-INPLACE-1101 の 2: `act_consents` の file の Always allow の行（`surface::always_allowed`）で決める（`PageEngine::skips_approval` の doc）
    /// ENGINE-AUDIT W7 本体 1 から（operator 11-03 の決め 3 = Always allow は消さずに狭める）: `gui::asks` に当たる手は Always allow が在っても毎回 card
    /// = ① 押す手（`CLICK_ACTION`）は飛ばさない（W7 から承認の要る押す手は `gui::asks` に当たった手だけ・戻せる押す手は reveal で承認が要らない）
    /// ② 打つ手の欄の名前（`label`）・menu の道（`path` の項目のどれか）が `gui::asks` に当たれば飛ばさない。key の手（名前が無い）は今まで通り。
    /// ATTACH の button と型は変えない（押す手の card の Always allow は行を書くが、その手を飛ばす事には使わない）
    fn skips_approval(&self, steps: &[crate::ui::engine_view::ApprovalStep]) -> bool {
        let always = crate::surface::always_allowed(&self.act_consents);
        // W7: 消す・送る・買う系の名前の手（打つ欄の名前・menu の道の項目）は Always allow でも毎回聞く
        let asks = |s: &ApprovalStep| {
            let role = s.input["role"].as_str().unwrap_or_default();
            let names = s.input["label"].as_str().into_iter().chain(s.input["path"].as_array().into_iter().flatten().filter_map(Value::as_str));
            names.into_iter().any(|n| crate::gui::asks(n, role, false).is_some())
        };
        let covered = |s: &ApprovalStep| {
            let (app, bundle) = (s.input["app_name"].as_str().unwrap_or_default(), s.input["bundle_id"].as_str().filter(|b| !b.is_empty()));
            s.action != engine::CLICK_ACTION && !asks(s) && !app.is_empty() && always.iter().any(|a| a.action == s.action && a.app_name == app && bundle.is_none_or(|b| b == a.bundle_id))
        };
        !steps.is_empty() && steps.iter().all(covered)
    }

    /// WF2: 再起動の後に、人を待って止まった run（runs 表）の card。registry も model も使わない
    fn waiting(&self, store: &Store) -> Vec<EngineWaiting> {
        engine::waiting_runs(store)
            .into_iter()
            .map(|p| EngineWaiting {
                project_id: p.space_id,
                run_id: p.run_id,
                goal: p.goal,
                reply: match p.asked {
                    Asked::Decide(d) => decide_reply(&d, None),
                    Asked::Approve { steps } => EngineReply::Denied { steps: steps.iter().map(step_of).collect(), line: String::new() },
                },
            })
            .collect()
    }

    /// RQ3（RQ-012・Export）: この Project の Workflow（`workflows/` の `ui` が `project_id` の物）と、step の capability の `<provider>` ごとの接続の参照
    /// （種類 = registry の Adapter の種類・この Mac に無い provider は `unknown`・Shibubu の中だけで動く `local_core` は入れない）。鍵・token・URL は持たない。
    fn export_workflows(&self, project_id: &str) -> (Vec<serde_json::Value>, Vec<ProjectResource>) {
        let wfs: Vec<engine::Workflow> = engine::load_workflows(&self.data_dir.join(engine::WORKFLOWS_DIR)).unwrap_or_default().into_iter().filter(|w| w.ui.as_deref() == Some(project_id)).collect();
        let records = self.registry_with_recipes().records();
        let mut conns: Vec<ProjectResource> = Vec::new();
        for s in wfs.iter().flat_map(|w| &w.steps) {
            let provider = s.capability.split('.').next().unwrap_or_default();
            let kind = records.iter().find(|r| r.id == s.capability).map(|r| r.kind);
            if provider.is_empty() || kind == Some(AdapterKind::LocalCore) || conns.iter().any(|c| c.id == provider) {
                continue;
            }
            conns.push(ProjectResource { id: provider.into(), kind: kind.map_or("unknown", AdapterKind::as_str).into(), uri: None });
        }
        (wfs.iter().filter_map(|w| serde_json::to_value(w).ok()).collect(), conns)
    }

    /// RQ3（RQ-012・Import）: Workflow を `ui` = `project_id` にして保存し（id が既に在れば `-2`…・検査を通らない物は保存しない）、使う provider のうち
    /// この Mac で Ready の action が 1 つも無い物を返す。
    /// ponytail: 足りない接続を知るのに registry の `check`（gh の auth 等の疎通）を UI の thread で 1 回走らせる。Import は手で押す 1 回なので待たせる・重いなら裏の thread へ
    fn import_workflows(&self, project_id: &str, workflows: &[serde_json::Value]) -> Result<Vec<ProjectResource>, String> {
        let dir = self.data_dir.join(engine::WORKFLOWS_DIR);
        let have: Vec<String> = engine::load_workflows(&dir).unwrap_or_default().into_iter().map(|w| w.id).collect();
        let (mut bad, mut providers) = (0, Vec::<String>::new());
        for v in workflows {
            let Ok(mut wf) = serde_json::from_value::<engine::Workflow>(v.clone()) else {
                bad += 1;
                continue;
            };
            wf.ui = Some(project_id.to_owned());
            let base = wf.id.clone();
            for n in 2.. {
                if !have.contains(&wf.id) && !dir.join(format!("{}.json", wf.id)).exists() {
                    break;
                }
                wf.id = format!("{base}-{n}");
            }
            if engine::save_workflow(&dir, &wf).is_err() {
                bad += 1;
                continue;
            }
            for s in &wf.steps {
                let p = s.capability.split('.').next().unwrap_or_default().to_owned();
                if !p.is_empty() && !providers.contains(&p) {
                    providers.push(p);
                }
            }
        }
        if bad > 0 {
            return Err(format!("{bad} of the project's steps couldn't be read"));
        }
        if providers.is_empty() {
            return Ok(Vec::new());
        }
        let mut reg = self.registry_with_recipes();
        reg.check();
        let records = reg.records();
        Ok(providers
            .into_iter()
            .filter(|p| !records.iter().any(|r| r.id.split('.').next() == Some(p.as_str()) && matches!(reg.tier(&r.id), DisplayTier::Ready | DisplayTier::Verified)))
            .map(|p| {
                let kind = records.iter().find(|r| r.id.split('.').next() == Some(p.as_str())).map_or("unknown", |r| r.kind.as_str());
                ProjectResource { id: p, kind: kind.into(), uri: None }
            })
            .collect())
    }

    /// TALK-HONEST-1101: run と同じ registry（recipe を載せた物）と保存した能力（`engine::abilities`）・Engine が起こしてよい MCP の名前。
    /// `check` も MCP の tools/list もしない（何も起こさない・L4）= 確かめたかは保存した記録の通り。
    fn abilities(&self) -> String {
        let mcp: Vec<&str> = self.mcp.mcp_servers.keys().map(String::as_str).collect();
        crate::domain::chat::talk_abilities(&engine::abilities(&self.registry_with_recipes(), &self.data_dir, &self.env), &mcp)
    }

    /// ENGINE-GENERIC-1101 追記 234: 今 窓を開いている app の 1 行（run と同じ registry の読み = `open_windows`・題は渡さない）。chat の thread と Engine の
    /// thread が `generated::with_windows` で呼ぶ（前は run の中だけ = chat は窓を知らなかった）。読めなければ None。
    /// ponytail: 呼ぶたびに registry を作り `check` する（run の check と 2 回）。chat の答えが遅くなれば窓の読みを数秒の写しに
    fn windows(&self) -> Option<String> {
        let mut reg = (self.registry)();
        reg.check();
        engine::open_windows(&mut reg, Caller { app_id: "shibubu.situation", run_id: None })
    }

    /// AI-COMPOSE-1101（owner 206）: 部品の `shibubu.read(app, what)`。名指しの app の主窓を AX で読み（registry の読む 3 手 = `desktop.apps.list` →
    /// `desktop.windows.list`（main）→ `desktop.window.state`・どれも LocalRead で承認も AI の同意も要らない = 書く手・送る手は 1 手も無い）、見えている要素の
    /// role・label・value の行を AI に 1 回渡して `what` の形の JSON にする（窓に無ければ当てずに Err）。Err は頁に返す人の 1 行（`ReadMiss::line`）と、
    /// app が開いていない・窓が無い・窓に無い時だけ Shibubu が開けば読める一言（`ReadMiss::open` = `Open {探した物} in {app}`・頁の「Let Shibubu open it」）。
    /// 新しくした理由（interface の `EngineRequest::reads_only` の run を捨てた）: Engine の Execution は読み戻しの check と期待値が前提（check の無い計画は
    /// Verification の Err・合うまで 90 秒読み直す）で、app と what が決まった read には interpret・generate の AI 2 回も要らない。
    /// ponytail: AX の主窓の読みだけ（見えている物しか読めない）。recipe・MCP・CLI で読む道は、要る app が出た時に Engine の読むだけの run として足す
    fn read(&self, app: &str, what: &str, llm: Option<Arc<dyn crate::llm::Llm>>) -> Result<Value, ReadMiss> {
        use crate::domain::run::Outcome;
        let Some(llm) = llm else { return Err(READ_NEEDS_AI.into()) };
        let mut reg = self.registry_with_recipes();
        reg.check();
        if !reg.records().iter().any(|r| r.id == "desktop.window.state") {
            return Err(format!("Shibubu can't read {app} on this Mac").into());
        }
        let caller = Caller { app_id: "canvas.read", run_id: None };
        let schema = json!({
            "type": "object", "additionalProperties": false, "required": ["found", "json", "looked_for", "shows", "next"],
            "properties": {
                "found": { "type": "boolean", "description": "true only when the lines show what was asked" },
                "json": { "type": "string", "description": "the answer as JSON text in the shape asked (empty when not found)" },
                "looked_for": { "type": "string", "description": "what was asked, in a few plain words (for example: the guitar list)" },
                "shows": { "type": "string", "description": "when not found: what the lines show instead, in a few words (empty when found)" },
                "next": { "type": "string", "description": "when not found: the one thing the person can do in that app so the window shows it, as one short instruction (empty when found)" }
            }
        });
        let answer = |got: &Value| {
            let text = got["json"].as_str().unwrap_or_default().trim();
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_owned()))
        };
        // TALK-HONEST-1101 追記 232: 誤りの文 = 何を探して・窓に何が見えていたか・次の一手 1 つ（app の名前は文に入るが code の枝にしない）。
        // 探した物は部品の `what` の形の前（composer が `… as a JSON array of strings` の形で書かせる）。探す語もここから（形の語 json・array は探さない）
        let sought = what.split(" as ").next().unwrap_or(what).trim();
        let words = crate::gui::words_of(&[sought]);
        // ENGINE-AUDIT W4（話 1・Done の 1 本目）: AX の前に app の口の棚（bundle を読むだけ = 同梱の data の名前・辞書・URL scheme）。頼みの語に当たればその行を AI に渡して答える
        // = 窓を読まない・押す手 0・app が閉じていてもよい。当たりが無い・AI が「無い」と言った時は今まで通り窓を読む
        let shelf = match reg.invoke(caller, engine::SURFACES_ACTION, &json!({ "app_name": app }), None) {
            (_, Some(o)) => serde_json::from_value::<crate::gui::AppSurfaces>(o.payload).ok(),
            _ => None,
        };
        if let Some(s) = shelf.as_ref().filter(|s| !s.hits(&words).is_empty()) {
            let lines = s.lines(&words, READ_CHARS);
            let input = format!("app: {app}\nwhat: {what}\nfrom the app's bundle, not its window ({} lines · url / opens / dictionary / <kind> (count): names / strings):\n{}", lines.len(), lines.join("\n"));
            let got = llm.complete_json(crate::llm::JsonRequest::new(READ_NAME, READ_INSTRUCTIONS, &input, schema.clone())).map_err(|e| e.to_string())?;
            if got["found"] == true {
                return Ok(answer(&got));
            }
        }
        let mut get = |action: &str, input: Value| match reg.invoke(caller, action, &input, None) {
            (_, Some(o)) => Ok(o.payload),
            (Outcome::Rejected { reason }, None) => Err(reason),
            (_, None) => Err(format!("Couldn't read {app} · try again")),
        };
        // operator の決め: 開いていない・窓が無い・窓に無いは Shibubu が開けば読める = 文の後ろに「or let Shibubu open it」と、押した時の Engine への一言（`open`）
        let miss = |looked: &str, line: String| ReadMiss { line: format!("{line}, or let Shibubu open it"), open: Some(format!("Open {looked} in {app}")) };
        let apps = get("desktop.apps.list", json!({}))?;
        let Some(found) = apps.as_array().into_iter().flatten().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(app.trim()))) else {
            return Err(miss(sought, format!("Looked for {sought} in {app}, but {app} isn't open · Open {app}")));
        };
        let filter = match found["bundle_id"].as_str().filter(|b| !b.is_empty()) {
            Some(bundle) => json!({ "bundle_id": bundle, "main": true }),
            None => json!({ "pid": found["pid"], "main": true }),
        };
        let windows = get("desktop.windows.list", filter)?;
        let Some(w) = windows.as_array().and_then(|w| w.first()).cloned() else {
            return Err(miss(sought, format!("Looked for {sought} in {app}, but it has no window open · Open a window in {app}")));
        };
        let state = get("desktop.window.state", json!({ "pid": w["pid"], "window_id": w["window_id"], "app_name": app }))?;
        // ENGINE-AUDIT W3: generate と同じ行（`WindowState::lines` = 頼みの語の当たりを先頭に・残りは字の予算まで・入らない分は「and N more」）。読む数の上限は無い
        let state: crate::gui::WindowState = serde_json::from_value(state).map_err(|_| format!("Couldn't read {app} · try again"))?;
        let lines = state.lines(&words, READ_CHARS);
        let input = format!("app: {app}\nwhat: {what}\nwindow ({} lines · role: name = value · in path · selected · press · ×same):\n{}", lines.len(), lines.join("\n"));
        let got = llm.complete_json(crate::llm::JsonRequest::new(READ_NAME, READ_INSTRUCTIONS, &input, schema)).map_err(|e| e.to_string())?;
        if got["found"] != true {
            let say = |k: &str| got[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.trim_end_matches('.').to_owned());
            let looked = say("looked_for").unwrap_or_else(|| sought.to_owned());
            let shows = say("shows").unwrap_or_else(|| "something else".to_owned());
            let next = say("next").unwrap_or_else(|| format!("Show it in {app}"));
            return Err(miss(looked.as_str(), format!("Looked for {looked} in {app}, but its window shows {shows} now · {next}")));
        }
        Ok(answer(&got))
    }
}

/// AI-COMPOSE-1101: 部品の read の AI の呼び名（`llm-calls.jsonl`・`own_usage::purpose` は「Work in apps」）。
pub const READ_NAME: &str = "engine_read";
/// AI-COMPOSE-1101 → ENGINE-AUDIT W3（owner 256 の 2）: read で AI に渡す窓の行の字の予算（読む数の上限ではない）。頼みの語の当たりは予算を見ずに全部、残りは木の順で
/// ここまで、入らない分は「and N more」の 1 行（`gui::WindowState::lines` = generate と同じ関数）。
const READ_CHARS: usize = 12_000;
/// AI-COMPOSE-1101: AI が無い時の read の 1 行（Engine の「needs an AI tool」と同じ行き先）。
const READ_NEEDS_AI: &str = "Reading an app needs an AI tool · Add one in Settings › AI & Models";
const READ_INSTRUCTIONS: &str = "You get the visible elements of one app window, one per line (role: label = value), and what a page part asked to read. \
Answer only from those lines: found = true only when they show it, and json = the answer as JSON text in the shape asked (for example a JSON array of strings). \
looked_for = what was asked in a few plain words. When it isn't there, shows = what the window shows instead (a few words from its lines) and next = the one thing \
the person can do in that app so the window shows it, as a short instruction; both empty when found. Don't guess, add or translate names. \
When the lines come from the app's bundle instead of its window (url / opens / dictionary / <kind> (count): names / strings), the names are the data the app ships with: \
answer from them when they are what was asked (a list of names, whether something exists); when they aren't, found = false with shows and next empty.";

/// WF2: 判断の card（選択肢ごとに button の名前と証拠の行 = 1 行の得失・選んだ時に後ろの手へ渡す値）。
fn decide_reply(d: &Decision, line: Option<String>) -> EngineReply {
    let shown = |v: &Value| match v {
        Value::String(s) => format!("“{s}”"),
        Value::Null => String::new(),
        other => crate::llm::short(other),
    };
    let choices = d
        .choices
        .iter()
        .map(|c| DecideOption { id: c.id.clone(), label: c.label.clone(), lines: [c.note.clone(), shown(&c.value)].into_iter().filter(|l| !l.trim().is_empty()).collect() })
        .collect();
    EngineReply::Decide { question: d.question.clone(), choices, line }
}

impl CoreEngine {
    /// run と同じ registry（利用者の data の recipe を載せた物）。readiness はまだ読んでいない（`check` は呼ぶ側）。
    /// OPEN-INPLACE-1101 の 5: この data dir・Project の app・一言の Preview の plan の操作を 1 回だけ取り出す（Build の run が使う・次の run は interpret から）
    fn planned(&self, app: &str, goal: &str) -> Option<Vec<engine::Operation>> {
        let mut all = PLANNED.lock().unwrap_or_else(|e| e.into_inner());
        let at = all.iter().position(|(d, a, g, _)| *d == self.data_dir && a == app && g == goal.trim())?;
        Some(all.remove(at).3)
    }

    fn registry_with_recipes(&self) -> AdapterRegistry {
        let mut reg = (self.registry)();
        let (mcp, dir) = (self.mcp.clone(), self.data_dir.clone());
        reg.add_recipes(&self.data_dir.join(EXTENSIONS_DIR), &mut || engine::call_runner(mcp.clone(), dir.clone()));
        reg
    }
    /// 承認した画面の操作の宛先（`EngineRequest::grants`）を、app ごとの AI の同意に書く（既に在る bundle は書かない）。
    /// 宛先の app は registry の `desktop.apps.list`（bridge の `apps` = `MacGui::op` の門の `app_of` と同じ読み）で決める。AX の窓は読まない =
    /// 答えない app を待たず、送る時の窓の読みと食い違わない（G70・r7: grant の `window_list` だけが時間切れで黙って書かず、act は needs_consent）。
    /// input に `bundle_id` が在ればその bundle の起動中の app（保存した手順の pid は古いかもしれない・W19-3b / G31b）、無ければ pid の app。
    /// bundle id は呼ぶ側の値でなく bridge の読み。app 名が承認の 1 行で見せた `app_name` と違えば書かない（見せた app にだけ同意する）。
    /// 書かない・書けない時は理由を stderr に 1 行（その手は `MacGui` の門で needs_consent になり、Page に「X を AI に任せる」が出る）。
    /// OPEN-INPLACE-1101 の 2: 宛先に `"always": [action の id]`（承認の card の Always allow）が在れば、同じ読みで決めた bundle で
    /// `surface::always_allow` を action ごとに 1 行（同意と同じ file・同じ条件 = 見せた app 名と bridge の読みが合う時だけ）。
    fn grant(&self, reg: &mut AdapterRegistry, req: &EngineRequest) {
        let mut have = crate::surface::act_consents(&self.act_consents);
        for to in &req.grants {
            let app = to["app_name"].as_str().unwrap_or_default();
            let filter = match (to["bundle_id"].as_str().filter(|b| !b.trim().is_empty()), to["pid"].as_u64()) {
                (Some(bundle), _) => json!({ "bundle_id": bundle }),
                (None, Some(pid)) => json!({ "pid": pid }),
                (None, None) => {
                    eprintln!("G70: {app} を AI に任せる同意を書かない（宛先に pid も bundle id も無い）");
                    continue;
                }
            };
            let caller = Caller { app_id: &req.app_id, run_id: Some(&req.run_id) };
            let (outcome, obs) = reg.invoke(caller, "desktop.apps.list", &filter, None);
            let apps: Vec<AppIdentity> = obs.and_then(|o| serde_json::from_value(o.payload).ok()).unwrap_or_default();
            let Some(bundle) = apps.iter().find(|a| !a.bundle_id.is_empty() && a.name == app).map(|a| a.bundle_id.clone()) else {
                let seen: Vec<String> = apps.iter().map(|a| format!("{} ({})", a.name, a.bundle_id)).collect();
                eprintln!("G70: {app} を AI に任せる同意を書かない（{filter} の起動中の app: [{}]・{outcome:?}）", seen.join(", "));
                continue;
            };
            if have.insert(bundle.clone()) {
                if let Err(e) = crate::surface::grant_act_consent(&self.act_consents, &bundle, crate::surface::now_ms()) {
                    eprintln!("G70: {app} を AI に任せる同意を書けない: {e}");
                }
            }
            // OPEN-INPLACE-1101 の 2: 承認の card の Always allow（同意が前から在っても書く・同じ行は surface が重ねない）
            for action in to["always"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if let Err(e) = crate::surface::always_allow(&self.act_consents, &bundle, app, action, crate::surface::now_ms()) {
                    eprintln!("OPEN-INPLACE: {app} の {action} を Always allow に書けない: {e}");
                }
            }
        }
    }
}

/// Engine の答え → 画面の答えと記録の 1 行。
fn answer(got: Result<EngineRecord, EngineError>) -> EngineAnswer {
    match got {
        Ok(rec) => EngineAnswer {
            // G81: `verify` の行は「{action} の …」= 保存した file（`SAVED_READ`）を読んで確かめた行を数える
            reply: EngineReply::Done {
                verified: rec.verified.len(),
                saved: rec.verified.iter().filter(|v| v.strip_prefix(engine::SAVED_READ).is_some_and(|r| r.starts_with(" の "))).count(),
                history: history_lines(&rec),
                partial: rec.partial.clone(),
                without_ai: rec.without_ai.clone(),
                already: rec.already.clone(),
            },
            log: ("record", serde_json::to_value(&rec).unwrap_or_default()),
        },
        // 宛先の app に AI に任せる同意が無くて送らなかった（`MacGui` の門の 1 行・承認の後でしか届かないので `pending` は見ない）→ Page の「X を AI に任せる」
        Err(EngineError::Denied(d)) if d.why.contains(crate::gui::ACT_CONSENT_MARK) => {
            let log = json!({ "kind": "needs_consent", "why": d.why });
            EngineAnswer { reply: EngineReply::NeedsConsent { line: failure_line(&EngineError::Denied(d)) }, log: ("error", log) }
        }
        Err(EngineError::Denied(d)) => {
            let all: Vec<&PendingApproval> = d.pending.iter().chain(&d.later).collect();
            let log = json!({ "kind": "denied", "why": d.why, "ran": d.ran, "pending": all.iter().map(|p| json!({ "step": p.step, "action": p.action, "digest": p.digest })).collect::<Vec<_>>() });
            let steps = if d.pending.is_some() { all.iter().map(|p| step_of(p)).collect() } else { Vec::new() };
            let line = failure_line(&EngineError::Denied(d));
            EngineAnswer { reply: EngineReply::Denied { steps, line }, log: ("error", log) }
        }
        // WF2: 判断で止まった（run は waiting_human・card は選択肢ごとの証拠つき）
        Err(EngineError::Decide(d)) => {
            let log = json!({ "kind": "decide", "step": d.step, "choices": d.choices.iter().map(|c| c.id.as_str()).collect::<Vec<_>>() });
            EngineAnswer { reply: decide_reply(&d, None), log: ("error", log) }
        }
        // WF2: 止まっている間に元データが変わった → 1 手も送らず、理由の 1 行と出し直しの card（判断か承認）
        Err(EngineError::Changed { why, asked }) => {
            let line = failure_line(&EngineError::Changed { why: why.clone(), asked: asked.clone() });
            let reply = match asked {
                Asked::Decide(d) => decide_reply(&d, Some(line)),
                Asked::Approve { steps } => EngineReply::Recheck { steps: steps.iter().map(step_of).collect(), line },
            };
            EngineAnswer { reply, log: ("error", json!({ "kind": "changed", "why": why })) }
        }
        Err(e) => EngineAnswer { log: ("error", json!({ "kind": "failed", "why": e.to_string() })), reply: EngineReply::Failed { line: failure_line(&e) } },
    }
}

/// 承認待ちの手 → 画面と承認の記録に要る物。
fn step_of(p: &PendingApproval) -> ApprovalStep {
    ApprovalStep { action: p.action.clone(), input: p.input.clone(), digest: p.digest.clone(), scopes: p.scopes.clone() }
}

/// 次に開く括弧（全角か半角の早い方）の位置と、閉じる字。
fn next_open(s: &str, pairs: [(char, char); 2]) -> Option<(usize, char)> {
    pairs.iter().filter_map(|&(o, c)| s.find(o).map(|i| (i, c))).min()
}

/// 「（pid 21415・窓番号 1234）」「 (process 21415 · window number 1234)」のような、pid を含む括弧を落とす（Adapter の拒否の 1 行は人の言葉だが、窓の指し方の数を持つ・W19a2 の実機・英語は UI-2b）。
fn without_ids(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((i, close)) = next_open(rest, [('（', '）'), ('(', ')')]) {
        let Some(j) = rest[i..].find(close).map(|j| i + j + close.len_utf8()) else { break };
        if rest[i..j].contains("pid") || rest[i..j].contains("process ") {
            out.push_str(rest[..i].trim_end_matches(' '));
        } else {
            out.push_str(&rest[..j]);
        }
        rest = &rest[j..];
    }
    out.push_str(rest);
    out
}

/// 理由の文が人の言葉だけか（日本語か英語の文〈英字の語 3 つ以上〉で、path・引用符・英字の識別子〈`a.b` の形〉を含まない）。
/// 「」“” の中は人が見ている名前（文書の題名・app の名前）なので見ない（「README.md」の `.md` を識別子と読まない・W19-3b の実機で決め直せない理由が定型文に消えた）。
/// 語の中の `'`（can't・Shibubu's）は引用符と読まない（UI-2b: Adapter の拒否が英語になった）。
fn plain(s: &str) -> bool {
    let mut outside = String::new();
    let mut rest = s;
    while let Some((i, close)) = next_open(rest, [('「', '」'), ('“', '”')]) {
        outside.push_str(&rest[..i]);
        match rest[i..].find(close) {
            Some(j) => rest = &rest[i + j + close.len_utf8()..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    outside.push_str(rest);
    let s = outside.as_str();
    let dotted = s
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .any(|tok| tok.split('.').filter(|p| !p.is_empty()).count() >= 2 && tok.chars().any(|c| c.is_ascii_alphabetic()));
    let chars: Vec<char> = s.chars().collect();
    let quoted = (0..chars.len()).any(|i| chars[i] == '\'' && !(i > 0 && chars[i - 1].is_alphabetic() && chars.get(i + 1).is_some_and(|c| c.is_alphabetic())));
    let english = s.split_whitespace().filter(|w| w.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2).count() >= 3;
    (states::has_ja(s) || english) && !dotted && !quoted && !s.contains(['/', '`', '{', '\\'])
}

/// 失敗・拒否の 1 行（理由と次の一手）。
/// Wave: W19-2
/// 保証する: `Denied` の理由は人の言葉の 1 行（Adapter の拒否: 人が操作中・許可が無い）ならそのまま、action の id・path・英字の識別子を含めば定型文・
/// どの腕も「何も送っていない／済みとは出さない」と次の一手を持つ・段の名前と error の原文を出さない。
pub fn failure_line(e: &EngineError) -> String {
    match e {
        // r10 ②（P0）: 止めるまでに手が走った（`Denial::ran`）= 「何も送っていない」と言わない。何手走ったか ＋ 止めた理由（人の言葉の時だけ）＋ 探した跡
        EngineError::Denied(d) if d.ran > 0 => {
            let seen = if plain(&d.why) { format!("{}. ", without_ids(&d.why)) } else { String::new() };
            let ran = crate::domain::plural(d.ran, "step");
            with_search(format!("{seen}{ran} ran before Shibubu stopped, and the steps after that didn't run (nothing they would delete or send was done). Look inside the original app"), &d.searched)
        }
        // ENGINE-AUDIT W5（節 8）: 探した跡（何を探し・何が見えたか・次の一手）を 1 行の後ろに
        EngineError::Denied(d) if plain(&d.why) => with_search(without_ids(&d.why), &d.searched),
        EngineError::Denied(d) => with_search("Nothing was done (nothing was sent). Check the state of the original app, then ask again".into(), &d.searched),
        // WF-036: AI が使えず、AI 無しの道でも手段が無い = 「言い直して」と言わない（AI の失敗を利用者の言い方のせいにしない）
        EngineError::NoMeans { operation, tried, .. } if tried.iter().any(|t| t.starts_with(engine::AI_NOT_USED)) => {
            let why = tried.iter().find_map(|t| t.strip_prefix(engine::AI_NOT_USED)).filter(|w| plain(w)).map_or_else(String::new, |w| format!(" ({w})"));
            let what = if plain(operation) { format!("“{operation}”") } else { "what you asked".into() };
            format!("The AI couldn't be used{why}, and without it there's no safe way to do {what}. Nothing was sent. Continue in the original app, or ask again later")
        }
        // TALK-HONEST-1101（spec 3・前の G100 FX-101「No steps came back · N ways … looked at」をやめた）: なぜ止めたかを利用者の言葉で、頼んだ言葉の言語で。
        // 画面の手（`desktop.*`）しか無く、どれも確かめていない = 直接の口が無く画面の手も未確認・何も無い = この Mac にまだ口が無い・
        // 画面の手が全部使えない = 直接の口が無く画面の手も今は使えない・他 = 手段を見たが手順が作れない。手段の id と数は出さない
        EngineError::NoMeans { operation, tried, searched } => {
            let ja = states::has_ja(operation);
            let what = match (plain(operation), ja) {
                (true, true) => format!("「{operation}」"),
                (true, false) => format!("“{operation}”"),
                (false, true) => "頼まれた事".into(),
                (false, false) => "what you asked".into(),
            };
            let screen: Vec<&String> = tried.iter().filter(|t| t.starts_with("desktop.")).collect();
            let direct = tried.len() - screen.len();
            let all = |end: &str| !screen.is_empty() && screen.iter().all(|t| t.contains(end));
            let line = match (direct, ja) {
                (0, true) if all("（未確認）") => format!("{what}は、外から直接する口がこの Mac に無く、画面を押す手もまだ確かめていないので止めました（続きは元の app で）"),
                (0, false) if all("（未確認）") => format!("There's no direct way to do {what} on this Mac, and pressing its screen hasn't been checked here yet, so Shibubu stopped. Continue in the original app"),
                (0, true) if all("（使えない") => format!("{what}は、外から直接する口がこの Mac に無く、画面を押す手も今は使えないので止めました（続きは元の app で）"),
                (0, false) if all("（使えない") => format!("There's no direct way to do {what} on this Mac, and its screen can't be pressed right now, so Shibubu stopped. Continue in the original app"),
                (0, true) if screen.is_empty() => format!("{what}をする口がこの Mac にまだ無いので止めました（続きは元の app で）"),
                (0, false) if screen.is_empty() => format!("Nothing on this Mac can do {what} yet, so Shibubu stopped. Continue in the original app"),
                (_, true) => format!("{what}をする手段を見ましたが、手順を作れなかったので止めました（続きは元の app で）"),
                (_, false) => format!("Shibubu looked at the ways to do {what} here but couldn't make steps for it, so it stopped. Continue in the original app"),
            };
            with_search(line, searched)
        }
        EngineError::Model(_) => "Couldn't use the AI's answer. Rephrase and ask again".into(),
        EngineError::Generation(_) => "Couldn't build the missing feature (nothing was sent). Continue in the original app".into(),
        EngineError::Verification(_) => "Couldn't check the result in the original app, so it isn't marked done. Look inside the original app".into(),
        // G74（owner 12:2x）: 送る手を打っていないと言える時と、打った後に確かめられない時の 2 つの文を混ぜない
        // G100 FX-101: 打った字が即時に同期される app では「Nothing was sent」が偽になりうる = 送る手を打っていない事だけを言う
        // FX-115: 送る直前に見えた事（`…: the window now shows another place …`）の最後の節が人の言葉なら添える（channel の名前は Adapter が書かない）
        EngineError::NotSent(why) => {
            let seen = why.rsplit(": ").next().filter(|s| plain(s)).map(|s| format!(" · {}", without_ids(s))).unwrap_or_default();
            format!("Couldn't confirm what was put in the original app, so Shibubu didn't do the send step{seen}. Check it in the original app, then ask again")
        }
        EngineError::Unconfirmed(_) => "Couldn't confirm whether it was sent, so it isn't marked done. It won't be sent again. Look inside the original app".into(),
        // G74c（owner 14:4x）: 走った手の後で次の手を止めた = 3 つ目の文（次の手を打っていない・何が見えて止めたかは Adapter の 1 行・前に走った手の数）
        EngineError::Stopped { why, ran, .. } => {
            let seen = if plain(why) { format!("{}. ", without_ids(why)) } else { String::new() };
            let ran = if *ran == 1 { "Only the first step ran".to_string() } else { format!("Only the first {ran} steps ran") };
            format!("{seen}Shibubu stopped at that step, so it and the steps after it didn't run (nothing they would delete or send was done). {ran}. Look inside the original app")
        }
        EngineError::Store(_) => "Can't read or write the saved features. Check free space and permissions where they're stored, then ask again".into(),
        // WF2: card の上の 1 行（Decide は card が問いと選択肢を持つ・この 1 行は記録と、card を出せない時だけ）
        EngineError::Decide(_) => "Waiting for your choice. The steps after it haven't run".into(),
        EngineError::Changed { why, .. } if plain(why) => why.clone(),
        EngineError::Changed { .. } => "Something changed while this was waiting, so nothing was sent. Check it again, then choose or approve".into(),
    }
}

/// ENGINE-AUDIT W5（節 8「止まった時の 1 行に、押せる次の一手が無い」）: 止まった 1 行の後ろに探した跡を添える = 窓に何が見えたか（近い名前・名前の付いた数）・試した手・
/// 次の一手（AI の none_why = その app で何をすれば名前が出るか・ATTACH が行や button にする）。探していなければそのまま。名前は画面の字（role・id は出さない）
fn with_search(line: String, s: &engine::Searched) -> String {
    let mut out = line;
    if !s.seen.is_empty() {
        let names: Vec<&str> = s.seen.iter().map(|n| n.split_once(": ").map_or(n.as_str(), |(_, x)| x)).collect();
        let named = if s.named > names.len() { format!(" (of {} named things)", s.named) } else { String::new() };
        out.push_str(&format!(" · The window shows {}{named}", names.join(", ")));
    }
    if !s.tried.is_empty() {
        let tried: Vec<&str> = s.tried.iter().map(|t| t.split_once(' ').map_or(t.as_str(), |(_, x)| x)).collect();
        out.push_str(&format!(" · Tried {}", tried.join(", ")));
    }
    if let Some(n) = s.next.as_deref().filter(|n| !n.trim().is_empty()) {
        out.push_str(&format!(" · Next: {}", without_ids(n.trim_end_matches('.'))));
    }
    out
}

/// rail の「実行履歴」に足す行（1 run = この順の数行）: 頼んだ事・分けた操作・見つけた手段・選んだ理由・作った物・確かめた事。
/// Wave: W19-2
/// 保証する: 内部の id（手順・Adapter・action）・path・段の名前を出さない（数と人の言葉だけ）・確かめた事が 0 件なら「済み」と書かない。
pub fn history_lines(rec: &EngineRecord) -> Vec<String> {
    let wants: Vec<&str> = rec.operations.iter().map(|o| o.want.as_str()).filter(|w| plain(w)).collect();
    let sure = rec.discovered.iter().filter(|r| matches!(r.verification, Verification::Verified { .. })).count();
    // 選んだ理由は Engine の記録の reason（手順・Adapter の id を含み、文言は訳で変わる）ではなく、記録の事実から人の言葉で:
    // 作った物のうち手順（`wf-…`）でない物 = 足りない機能（recipe）を作った（capability_engine の `generated` は recipe の id と手順の id だけを積む）
    let why = match (rec.reused, rec.generated.iter().any(|g| !g.starts_with("wf-"))) {
        (true, _) => "Reused the earlier procedure (not rebuilt)",
        (false, true) => "Built the missing feature, tried it on a copy, then used it",
        (false, false) => "Built a procedure that combines checked features",
    };
    let sent: usize = rec.receipts.iter().map(|r| r.steps.len()).sum();
    let handed: usize = rec.receipts.iter().flat_map(|r| &r.steps).map(|s| s.handoffs.len()).sum();
    // WF-036: AI を使わなかった run は、その事を 1 行（済みの 1 行の前）
    let no_ai = rec.without_ai.as_ref().map(|w| format!("Without AI: {w} (used the saved or checked steps)"));
    let plural = crate::domain::plural;
    let mut lines = vec![
        format!("You asked: {}", rec.goal),
        format!("Split into {}{}", plural(rec.operations.len(), "action"), if wants.is_empty() { String::new() } else { format!(" ({})", wants.join(" · ")) }),
        format!("Found {} · {sure} checked", plural(rec.discovered.len(), "way")),
        format!("Why this one: {why}"),
        if rec.generated.is_empty() { "Built: nothing (reused earlier work)".to_string() } else { format!("Built: {} (a procedure and, if needed, the missing feature)", rec.generated.len()) },
        if rec.verified.is_empty() {
            format!("Checked: 0 (not marked done) · steps sent {sent}")
        } else {
            format!("Checked: {} in the original app's real state · steps sent {sent} · handed over {handed}", rec.verified.len())
        },
    ];
    lines.extend(no_ai);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_engine::Denial;

    /// ENGINE-AUDIT W9（節 14・interface）: `classify` は interpret だけで操作の種類を返し（AI 1 回・窓を読まない・Adapter を呼ばない）、同じ一言の次の `run` は
    /// interpret を呼び直さない（AI の呼び出しは classify の 1 回 ＋ generate の分だけ）。負の対照（手で）: classify の `PLANNED` への push を外す → interpret が 2 回で赤。
    #[test]
    fn w9_classify_reads_the_kind_once_and_the_run_reuses_it() {
        use crate::domain::chat::OperationKind;
        use crate::llm::fake::Fake;
        let dir = std::env::temp_dir().join(format!("shibubu-w9-classify-{}", crate::storage::new_id()));
        let e = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: McpConfig::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let ops = json!({ "operations": [
            { "id": "op1", "want": "Open the guitar list in the library", "success": "the list is shown", "target": null, "kind": "act", "named": true, "candidates": [] },
            { "id": "op2", "want": "Is the library open?", "success": "", "target": null, "kind": "question", "named": false, "candidates": [] },
        ] });
        let none = json!({ "none_why": null, "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        let fake = Arc::new(Fake::new(vec![Ok(ops), Ok(none.clone()), Ok(none)]));
        let req = |llm: Option<Arc<dyn crate::llm::Llm>>| EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "Open the guitar list and tell me if the library is open".into(), consents: vec![], grants: vec![], llm, target: None, document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        let kinds = e.classify(req(Some(fake.clone()))).expect("分けられる");
        assert_eq!(kinds.iter().map(|c| (c.kind, c.want.clone())).collect::<Vec<_>>(), [(OperationKind::Act, "Open the guitar list in the library".to_string()), (OperationKind::Question, "Is the library open?".to_string())]);
        assert!(kinds[0].named && kinds[0].target.is_none() && kinds[0].candidates.is_empty(), "名指し・宛先・候補も渡す: {:?}", kinds[0]);
        let a = e.run(req(Some(fake.clone())));
        assert_eq!(a.log.0, "error", "手段が無いので止まる: {:?}", a.log);
        let names: Vec<String> = fake.requests.lock().unwrap().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.iter().filter(|n| *n == "engine_interpret").count(), 1, "classify の後の run が interpret を呼び直した: {names:?}");
        assert!(e.classify(req(None)).is_err(), "AI が無ければ分けられない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B86（38 回目の take 5: 「Which app should Shibubu use for …?」を bundle id の button で聞き、押すと bundle id だけが次の一言になって録画の手 7・9 が止まった）:
    /// ① 候補が 2 つでも Page の app（`EngineRequest::target`）が在れば run と同じく埋めて聞かずに進み、1 行で「Using TextEdit for …」
    /// ② Page の app が無ければ聞くが、選択肢は人に見せる名前（窓の行の名前）・押すと元の頼みにその app を足した一言で分け直す。
    /// 負の対照（手で・未実行）: classify の `fill_page_target` を外す → 1 つ目が ask で赤・`app_label` を通さない → 選択肢が bundle id で赤・
    /// `answered_ask` が None を返す → 元の頼みが消えて赤。
    #[test]
    fn b86_the_page_app_answers_which_app_and_a_pressed_choice_keeps_the_request() {
        use crate::domain::chat::{answered_ask, route, Mark, Situation, Turn, Who};
        use crate::llm::fake::Fake;
        let dir = std::env::temp_dir().join(format!("shibubu-b86-{}", crate::storage::new_id()));
        let e = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: McpConfig::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let said = "Change the README title to “Shibubu Demo” and save it";
        let ops = json!({ "operations": [{ "id": "op1", "want": said, "success": "The README title is Shibubu Demo", "target": null, "kind": "act", "named": false, "candidates": ["com.apple.TextEdit", "com.stablyai.orca"] }] });
        let situation = Situation { facts: vec!["Windows open now: TextEdit (com.apple.TextEdit) × 1 · Orca (com.stablyai.orca) × 2".into()], files: vec![] };
        let req = |target: Option<&str>| EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: said.into(), consents: vec![], grants: vec![], llm: Some(Arc::new(Fake::new(vec![Ok(ops.clone())]))), target: target.map(str::to_owned), document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: situation.clone(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        let on_page = route(&e.classify(req(Some("com.apple.TextEdit"))).expect("分けられる"));
        assert!(on_page.engine && on_page.ask.is_none() && on_page.using.as_deref().is_some_and(|u| u.starts_with("Using TextEdit for")), "Page の app で聞かずに進む: {on_page:?}");
        let off_page = route(&e.classify(req(None)).expect("分けられる"));
        let (question, choices) = off_page.ask.clone().expect("Page の app が無ければ聞く");
        assert_eq!(choices, ["TextEdit", "Orca"], "bundle id を見せない");
        assert!(!off_page.engine && off_page.using.is_none(), "{off_page:?}");
        let turn = |who: Who, text: &str, choices: Vec<String>| Turn { who, text: text.into(), mark: Mark::None, at: String::new(), attachments: vec![], choices, proposal: None, preview: vec![], sections: vec![], needs: None };
        let chat = [turn(Who::You, said, vec![]), turn(Who::Shibubu, &question, choices), turn(Who::You, "TextEdit", vec![])];
        assert_eq!(answered_ask(&chat, "TextEdit"), Some(format!("{said} (use TextEdit)")), "押した後も元の頼みが続く");
        assert_eq!(answered_ask(&chat, "Excel"), None, "選択肢に無い一言は今まで通り");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENGINE-AUDIT W4（節 4・話 1「ギターの音色の一覧を出して」= Done の 1 本目）: 部品の `read` は AX の前に app の口の棚（`desktop.app.surfaces`）を引き、
    /// 語の当たり（同梱の data の名前）が在ればそれを AI に渡して答える（app が閉じていても読める・窓は読まない・押す手 0）。
    /// 負の対照（本体の後・手で）: read の棚の読みを外す → apps.list に行き「isn't open」で赤。
    #[test]
    fn w4_read_answers_from_the_shelf_before_touching_the_window() {
        use crate::adapters::sdk::{Adapter, Route, Spec};
        use crate::domain::action::Effect;
        use crate::domain::binding::Source;
        use crate::domain::capability::CapabilityReadiness;
        use crate::domain::run::{Observation, Outcome};
        use crate::gui::{AppSurfaces, ResourceGroup};
        use crate::llm::fake::Fake;
        use std::sync::Mutex;
        struct Shelf(Arc<Mutex<Vec<String>>>);
        static SHELF: [Spec; 4] = [
            Spec { id: "desktop.apps.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "desktop.app.surfaces", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        ];
        impl Adapter for Shelf {
            fn id(&self) -> &'static str {
                "desktop"
            }
            fn kind(&self) -> AdapterKind {
                AdapterKind::MacGui
            }
            fn route(&self) -> Route {
                Route::OsAutomation
            }
            fn provider(&self) -> &'static str {
                "desktop"
            }
            fn specs(&self) -> &'static [Spec] {
                &SHELF
            }
            fn readiness(&self, _action: &str) -> CapabilityReadiness {
                CapabilityReadiness::Ready
            }
            fn sources(&self) -> Vec<Source> {
                vec![]
            }
            fn check(&mut self) {}
            fn invoke(&mut self, action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
                self.0.lock().unwrap().push(action.into());
                let payload = match action {
                    "desktop.app.surfaces" => json!(AppSurfaces {
                        bundle_id: "com.example.mixer".into(),
                        name: "Mixer".into(),
                        resources: vec![ResourceGroup { ext: "patch".into(), count: 3, names: vec!["Guitar/Acoustic".into(), "Guitar/Clean".into(), "Drums/Kit".into()] }],
                        ..Default::default()
                    }),
                    "desktop.apps.list" => json!([]),
                    _ => json!([]),
                };
                (Outcome::Verified { evidence: vec!["shelf".into()] }, Some(crate::adapters::sdk::observe("desktop", crate::adapters::sdk::now(), payload)))
            }
            fn cleanup(&mut self) {}
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let dir = std::env::temp_dir().join(format!("shibubu-w4-read-{}", crate::storage::new_id()));
        let e = CoreEngine { registry: Arc::new(move || { let mut r = AdapterRegistry::default(); r.register(Box::new(Shelf(c.clone()))).unwrap(); r }), data_dir: dir.clone(), mcp: McpConfig::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let fake = Arc::new(Fake::new(vec![Ok(json!({ "found": true, "json": "[\"Acoustic\",\"Clean\"]", "looked_for": "the guitar list", "shows": "", "next": "" }))]));
        let got = e.read("Mixer", "the guitar sounds as a JSON array of strings", Some(fake.clone())).expect("棚から読める");
        assert_eq!(got, json!(["Acoustic", "Clean"]));
        let input = fake.requests.lock().unwrap()[0].input.clone();
        assert!(input.contains("Guitar/Acoustic") && input.contains("Guitar/Clean") && !input.contains("Drums"), "棚の当たりだけを AI に: {input}");
        let seen = calls.lock().unwrap().clone();
        assert!(seen.contains(&"desktop.app.surfaces".to_string()) && !seen.contains(&"desktop.window.state".to_string()), "棚で答えられるのに窓を読んだ: {seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENGINE-AUDIT W1（節 16）: 止まった run の `engine-runs.jsonl` の行（`EngineAnswer::log` の error）にも段（順・ms）が残る = どこで何秒かかったかが 1 行で読める。
    /// 負の対照（手で）: `CoreEngine::run` の `a.log.1["stages"]` の 1 行を外す → 赤。
    #[test]
    fn w1_a_stopped_run_logs_its_stages_in_the_error_row() {
        let dir = std::env::temp_dir().join(format!("shibubu-w1-stages-{}", crate::storage::new_id()));
        let e = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: McpConfig::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let req = EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "Open the guitar list".into(), consents: vec![], grants: vec![], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        let a = e.run(req);
        assert_eq!(a.log.0, "error", "{:?}", a.log);
        let stages = a.log.1["stages"].as_array().cloned().unwrap_or_default();
        assert_eq!(stages.iter().map(|s| s["stage"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["goal_interpretation", "capability_discovery", "deep_analysis", "integration_planning"], "{:?}", a.log.1);
        assert!(stages.iter().all(|s| s["ms"].is_u64()), "段ごとの ms: {stages:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// r10 ②（P0・dogfood r10 の最後の 1 行が「Nothing was done (nothing was sent)」なのに手 1 は送っていた）: 走った手が在る `Denied`（`Denial::ran`）は
    /// 何も送っていないと言わず、何手走ったかと探した跡を言う（理由が内部の語でも）。走った手が無ければ今まで通り。
    /// 負の対照: `failure_line` の `d.ran > 0` の腕を外す → 1 つ目の assert が赤。
    #[test]
    fn r10_a_stop_after_a_step_ran_never_says_nothing_was_sent() {
        let s = engine::Searched { words: vec!["guitar".into()], seen: vec!["AXRow: Library".into()], named: 232, next: None, tried: vec!["desktop.ax.click Brit and Clean".into()] };
        let internal = "'desktop.ax.click' は承認が要る（この承認は使用済み）";
        let l = failure_line(&EngineError::Denied(engine::Denial { why: internal.into(), searched: s.clone(), ran: 1, ..Default::default() }));
        assert!(!l.contains("nothing was sent") && !l.contains("Nothing was done") && l.contains("1 step ran") && l.contains("Library") && !l.contains("desktop.ax"), "{l}");
        let human = "Looked for “Acoustic Guitar” in Mixer, but it isn't in the window and nothing on the screen leads to it, so nothing was pressed";
        let l = failure_line(&EngineError::Denied(engine::Denial { why: human.into(), searched: s.clone(), ran: 2, ..Default::default() }));
        assert!(l.starts_with(human) && l.contains("2 steps ran"), "{l}");
        let l = failure_line(&EngineError::Denied(engine::Denial { why: internal.into(), searched: s, ..Default::default() }));
        assert!(l.starts_with("Nothing was done (nothing was sent)"), "走った手の無い止めの 1 行を変えた: {l}");
    }

    /// ENGINE-AUDIT W5（節 8「止まった時の 1 行に、押せる次の一手が無い」）: 止まった 1 行は「何を探し・何が見えたか・次の一手」を言う（`Denied` と `NoMeans` の両方・`engine::Searched`）。
    /// interface の commit で書いた（本体の前は赤）。負の対照（本体の後・手で）: `failure_line` の `searched` の節を外す → 赤。
    #[test]
    fn w5_failure_line_says_what_was_looked_for_what_was_seen_and_the_next_move() {
        let s = engine::Searched { words: vec!["acoustic".into(), "guitar".into()], seen: vec!["AXButton: Library".into(), "AXRow: Guitar".into()], named: 212, next: Some("Open the Library in Mixer".into()), tried: vec![] };
        let d = failure_line(&EngineError::Denied(engine::Denial { why: "Looked for “Acoustic Guitar” in Mixer, but it isn't in the window now".into(), searched: s.clone(), ..Default::default() }));
        for t in ["Acoustic Guitar", "Library", "Guitar", "Open the Library in Mixer"] {
            assert!(d.contains(t), "{t} が無い: {d}");
        }
        let n = failure_line(&EngineError::NoMeans { operation: "Set the guitar to Acoustic Guitar".into(), tried: vec!["desktop.ax.click（確かめ済み・この操作の手順が無い）".into()], searched: s });
        for t in ["Library", "Open the Library in Mixer"] {
            assert!(n.contains(t), "{t} が無い: {n}");
        }
        assert!(!d.contains("desktop.") && !n.contains("desktop."), "action の id を出した: {d} / {n}");
    }

    /// RQ3（RQ-012）: Project の Workflow は Export で持ち出し（接続は provider の名前と種類だけ・URL も鍵も無い）、Import で新しい Project の id に付け替えて
    /// 保存する（同じ id が在れば `-2`・元は変えない）・この Mac で使えない provider を返す（Page の `… isn't connected on this Mac` の 1 行）。
    /// 負の対照（commit message）: `import_workflows` で `wf.ui` を替えない → 付け替えの assert が赤。
    #[test]
    fn rq3_workflows_travel_with_the_project_and_missing_connections_are_named() {
        let dir = std::env::temp_dir().join(format!("shibubu-rq3-wf-{}", crate::storage::new_id()));
        let wf_dir = dir.join(engine::WORKFLOWS_DIR);
        let e = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: McpConfig::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let step = engine::WorkflowStep { id: "s1".into(), capability: "github.issue.create".into(), input: json!({ "title": "x" }), kind: engine::StepKind::Action };
        let wf = engine::Workflow { proven: None, fails: 0, schema: engine::WORKFLOW_SCHEMA.into(), id: "post-issue".into(), goal: "Post an issue".into(), steps: vec![step], ui: Some("project.a".into()), checks: vec![], split: vec![] };
        engine::save_workflow(&wf_dir, &wf).unwrap();
        engine::save_workflow(&wf_dir, &engine::Workflow { id: "other".into(), ui: Some("project.z".into()), ..wf.clone() }).unwrap();
        let (wfs, conns) = e.export_workflows("project.a");
        assert_eq!(wfs.len(), 1, "他の Project の Workflow を入れた・入れていない");
        assert_eq!(conns.iter().map(|c| (c.id.as_str(), c.kind.as_str(), c.uri.is_none())).collect::<Vec<_>>(), vec![("github", "unknown", true)]);
        let missing = e.import_workflows("project.b", &wfs).unwrap();
        assert_eq!(missing.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["github"], "使えない接続を言わない");
        let saved = engine::load_workflows(&wf_dir).unwrap();
        let got = saved.iter().find(|w| w.id == "post-issue-2").expect("同じ id の Workflow を上書きした・保存していない");
        assert_eq!((got.ui.as_deref(), got.steps.len()), (Some("project.b"), 1), "新しい Project に付け替えていない");
        assert!(saved.iter().any(|w| w.id == "post-issue" && w.ui.as_deref() == Some("project.a")), "元を変えた");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 署名を compile で縛る。
    #[test]
    fn w19_2_engine_page_signatures() {
        let _: fn(&EngineError) -> String = failure_line;
        let _: fn(&EngineRecord) -> Vec<String> = history_lines;
        fn page(e: CoreEngine) -> Arc<dyn PageEngine> {
            Arc::new(e)
        }
        let _ = page;
    }

    /// rail の実行履歴は 5 項目（分けた操作・見つけた手段・選んだ理由・作った物・確かめた事）を人の言葉で・内部の id を出さない・確かめ 0 件なら「済みとは出しません」。
    #[test]
    fn w19_2_history_lines_say_the_five_items_without_internal_ids() {
        use crate::capability_engine::{Choice, Handoff, Operation, Score, StepReceipt, WorkflowReceipt, ENGINE_SCHEMA};
        use crate::domain::run::Outcome;
        let rec = EngineRecord {
            schema: ENGINE_SCHEMA.into(),
            goal: "README に 1 行足して保存して".into(),
            operations: vec![Operation { id: "op1".into(), want: "README に 1 行足す".into(), success: "保存した".into(), target: None, ..Default::default() }],
            discovered: vec![],
            choices: vec![Choice { operation: "op1".into(), capability: "desktop.ax.menu".into(), reason: "保存した手順「wf-0123456789ab-op1」を再利用（生成 0 回）".into(), score: Score::default() }],
            generated: vec![],
            verified: vec!["desktop.window.state の /controls/2/value を読み戻した".into()],
            receipts: vec![WorkflowReceipt {
                workflow: "wf-0123456789ab-op1".into(),
                steps: vec![StepReceipt { step: "set".into(), outcome: Outcome::Verified { evidence: vec![] }, observation: None, handoffs: vec![Handoff { from_step: "read".into(), to_step: "set".into(), field: "text".into(), digest: "d".into() }] }],
            }],
            stages: vec![],
            reused: true,
            partial: None,
            without_ai: None,
            already: None,
        };
        let lines = history_lines(&rec);
        let all = lines.join("\n");
        for want in ["You asked: README に 1 行足して保存して", "Split into 1 action (README に 1 行足す)", "Found 0 ways", "Why this one: Reused the earlier procedure", "Built: nothing", "Checked: 1 in the original app's real state · steps sent 1 · handed over 1"] {
            assert!(all.contains(want), "{want} が無い: {all}");
        }
        for internal in ["wf-", "op1", "desktop.", "/controls"] {
            assert!(!all.contains(internal), "{internal} を出さない: {all}");
        }
        // operator 10-29: 1 だけ単数（`domain::plural`）。負の対照: plural の単数の腕を外す → 1 の 2 行が「1 actions」「1 ways」で赤（2 の行は緑のまま）
        use crate::domain::capability::{CapabilityRecord, Origin, Reuse, Verification, CAPABILITY_SCHEMA};
        let way = CapabilityRecord {
            schema: CAPABILITY_SCHEMA.into(),
            id: "out.text.read".into(),
            reuse: Reuse::Shared,
            origin: Origin::Official,
            kind: crate::domain::capability::AdapterKind::LocalCore,
            verification: Verification::Unverified,
            versions: vec![],
            permissions: vec![],
            evidence: vec![],
            constraints: vec![],
        };
        let one = history_lines(&EngineRecord { discovered: vec![way.clone()], ..rec.clone() });
        assert!(one.iter().any(|l| l == "Found 1 way · 0 checked"), "{one:?}");
        let op2 = Operation { id: "op2".into(), want: "保存する".into(), success: String::new(), target: None, ..Default::default() };
        let two = history_lines(&EngineRecord { operations: vec![rec.operations[0].clone(), op2], discovered: vec![way.clone(), way], ..rec.clone() });
        assert!(two.iter().any(|l| l == "Split into 2 actions (README に 1 行足す · 保存する)"), "{two:?}");
        assert!(two.iter().any(|l| l == "Found 2 ways · 0 checked"), "{two:?}");
        let none = history_lines(&EngineRecord { verified: vec![], ..rec });
        assert!(none.join("\n").contains("Checked: 0 (not marked done)"));
    }

    /// UI-3（sendback 42・G20）: 「選んだ理由」の腕は記録の事実（作った物に手順でない物 = 足りない機能が在るか）で選ぶ。reason の文の語（`Adapter`）は
    /// 訳で変わる（yc/ui-2b の英語の reason は小文字の adapter）ので鍵にしない。負の対照（commit message）: 腕を `reason.contains("Adapter")` に戻す → 赤。
    #[test]
    fn ui3_why_line_follows_what_was_built_not_the_reason_words() {
        use crate::capability_engine::{Choice, Operation, Score, ENGINE_SCHEMA};
        let rec = |generated: &[&str]| EngineRecord {
            schema: ENGINE_SCHEMA.into(),
            goal: "Add a line to the README".into(),
            operations: vec![Operation { id: "op1".into(), want: "Add a line".into(), success: "Saved".into(), target: None, ..Default::default() }],
            discovered: vec![],
            choices: vec![Choice { operation: "op1".into(), capability: "desktop.ax.menu".into(), reason: "The existing abilities weren't enough, so adapter “x” was built".into(), score: Score::default() }],
            generated: generated.iter().map(|s| s.to_string()).collect(),
            verified: vec![],
            receipts: vec![],
            stages: vec![],
            reused: false,
            partial: None,
            without_ai: None,
            already: None,
        };
        let built = history_lines(&rec(&["recipe-textedit-append", "wf-0123456789ab-op1"])).join("\n");
        assert!(built.contains("Why this one: Built the missing feature"), "{built}");
        let combined = history_lines(&rec(&["wf-0123456789ab-op1"])).join("\n");
        assert!(combined.contains("Why this one: Built a procedure that combines checked features"), "{combined}");
    }

    /// W19-3: 画面が解析の承認を見分ける頭（engine_view は Engine の型を持たない）が、Engine の承認の action の頭と合う。
    #[test]
    fn w19_3_analysis_step_prefix_matches_the_engine() {
        assert_eq!(crate::ui::engine_view::ANALYSIS_STEP, format!("{}.", engine::ANALYSIS_ACTION));
    }

    /// TALK-HONEST-1101（spec 3）: NoMeans はなぜ止めたかを利用者の言葉で、頼んだ言葉の言語で 1 行（手段の id・数・「No steps came back」・中黒を出さない）。
    /// 負の対照: `ja` を常に false にする → 日本語の 3 行が赤・`all("（未確認）")` の腕を外す → GarageBand の行が「手段を見ましたが」になって赤。
    #[test]
    fn talk_honest_no_means_says_why_it_stopped_in_their_language() {
        let line = |op: &str, tried: &[&str]| failure_line(&EngineError::NoMeans { operation: op.into(), tried: tried.iter().map(|t| t.to_string()).collect(), searched: Default::default() });
        let screen = ["desktop.ax.click（未確認）", "desktop.ax.menu（未確認）", "desktop.key（未確認）"];
        assert_eq!(
            line("GarageBand の楽器をギターに替える", &screen),
            "「GarageBand の楽器をギターに替える」は、外から直接する口がこの Mac に無く、画面を押す手もまだ確かめていないので止めました（続きは元の app で）"
        );
        assert_eq!(
            line("Switch the GarageBand instrument to guitar", &screen),
            "There's no direct way to do “Switch the GarageBand instrument to guitar” on this Mac, and pressing its screen hasn't been checked here yet, so Shibubu stopped. Continue in the original app"
        );
        let off = ["desktop.ax.click（使えない: Accessibility is off）"];
        assert!(line("Add a line to README", &off).contains("its screen can't be pressed right now"));
        assert_eq!(line("README に 1 行足す", &[]), "「README に 1 行足す」をする口がこの Mac にまだ無いので止めました（続きは元の app で）");
        assert_eq!(line("desktop.ax.click", &[]), "Nothing on this Mac can do what you asked yet, so Shibubu stopped. Continue in the original app");
        let mixed = ["mcp.quill.read（未確認）", "desktop.ax.click（未確認）"];
        assert_eq!(line("README に 1 行足す", &mixed), "「README に 1 行足す」をする手段を見ましたが、手順を作れなかったので止めました（続きは元の app で）");
        for l in [line("x y z", &mixed), line("x y z", &screen), failure_line(&EngineError::NotSent("held".into()))] {
            assert!(!l.contains("No steps came back") && !l.contains("Nothing was sent") && !l.contains("mcp.") && !l.contains("desktop.") && !l.contains('・') && !l.chars().any(|c| c.is_ascii_digit()), "{l}");
        }
    }

    #[test]
    fn w19_2_failure_line_keeps_human_reasons_and_hides_internal_words() {
        let lease = "人が対象の app を操作中です（その app が前面）。何も送っていません。Shibubu に戻してから、もう一度頼んでください";
        assert!(failure_line(&EngineError::Denied(Denial::from(lease))).contains(lease));
        let gone = failure_line(&EngineError::Denied(Denial::from("TextEdit の窓（pid 21415・窓番号 1234）が見つかりません。窓を開き直して再登録してください")));
        assert_eq!(gone, "TextEdit の窓が見つかりません。窓を開き直して再登録してください", "pid の括弧だけを落とす");
        let raw = failure_line(&EngineError::Denied(Denial::from("'desktop.ax.type_text' は承認が要る（承認の記録が無い）")));
        assert!(!raw.contains("desktop.") && raw.contains("nothing was sent"), "{raw}");
        // W19-3b: 決め直せない理由は「」の中の題名（.md）があっても人の言葉としてそのまま出す・「」の外の識別子は今まで通り定型文
        let moved = "TextEdit で「draft.md」を開いた窓が見つかりません。開いてから、もう一度頼んでください（読めた TextEdit の窓 1 個: 「README.md」）";
        assert_eq!(failure_line(&EngineError::Denied(Denial::from(moved))), moved);
        let slow = "TextEdit が答えないので「draft.md」を開いた窓を探せませんでした。少し待ってから、もう一度頼んでください（読めた TextEdit の窓 0 個・読めなかった物: TextEdit（窓の一覧を読めません: app が応答しません（忙しい・dialog が開いている）））";
        assert_eq!(failure_line(&EngineError::Denied(Denial::from(slow))), slow);
        assert!(!failure_line(&EngineError::Denied(Denial::from("「draft.md」は desktop.ax.menu で送れません"))).contains("desktop."));
        // UI-2b: Adapter の拒否が英語になった。英語の文もそのまま・“” の中の題名は見ない・語の中の ' は引用符と読まない・半角の pid の括弧も落とす
        let lease = "Someone is using the app in front, so nothing was sent. Go back to Shibubu, then ask again";
        assert_eq!(failure_line(&EngineError::Denied(Denial::from(lease))), lease);
        let moved = "TextEdit isn't answering, so the window with “draft.md” couldn't be found. Wait a moment, then ask again (1 TextEdit window was read: “README.md”)";
        assert_eq!(failure_line(&EngineError::Denied(Denial::from(moved))), moved);
        let gone = failure_line(&EngineError::Denied(Denial::from("TextEdit's window (process 21415 · window number 1234) wasn't found. Reopen the window and add the step again")));
        assert_eq!(gone, "TextEdit's window wasn't found. Reopen the window and add the step again", "pid の括弧だけを落とす");
        for internal in ["'desktop.ax.type_text' needs approval", "“draft.md” can't be sent with desktop.ax.menu", "Not sent: see /Users/x/log", "ok"] {
            let l = failure_line(&EngineError::Denied(Denial::from(internal)));
            assert!(!l.contains("desktop.") && !l.contains("/Users") && l != "ok", "{internal} → {l}");
        }
        for e in [
            EngineError::NoMeans { operation: "README に 1 行足す".into(), tried: vec!["mcp.quill.read（今は使えない）".into()], searched: Default::default() },
            EngineError::Model("schema に合わない".into()),
            EngineError::Generation("/Users/x/trials/op1 の外".into()),
            EngineError::Verification("/controls/3/value が違う".into()),
            EngineError::Store("Permission denied (os error 13)".into()),
            EngineError::Stopped { step: "item".into(), why: "'desktop.ax.click' は /controls を schema で読めない".into(), ran: 1 },
        ] {
            let l = failure_line(&e);
            for internal in ["mcp.", "schema", "/Users", "/controls", "os error"] {
                assert!(!l.contains(internal), "{internal} を出さない: {l}");
            }
            assert!(l.contains("again") || l.contains("original app") || l.contains("元の app"), "次の一手か済みと出さない事を言う: {l}");
        }
        // G74c（owner 14:4x）: 走った手の後で次の手を止めた = 3 つ目の文。Adapter の 1 行（何が見えて止めたか）＋ その手と後ろは走っていない ＋ 前に走った手の数。
        // Verification の定型文（「Couldn't check the result」= 確かめられない）にしない
        let why = "No menu is open in Discord, so “Delete Message” wasn't pressed (nothing in the window was pressed)";
        let l = failure_line(&EngineError::Stopped { step: "item".into(), why: why.into(), ran: 1 });
        assert!(l.starts_with(why) && l.contains("didn't run") && l.contains("delete or send") && l.contains("Only the first step ran") && !l.contains("Couldn't check"), "{l}");
        let l = failure_line(&EngineError::Stopped { step: "w2".into(), why: why.into(), ran: 2 });
        assert!(l.contains("Only the first 2 steps ran"), "{l}");
    }
}

/// W19-2 の配線（図 §33）の試験: Page の欄 → `finish_edit` の Unsupported → 裏の thread の `capability_engine::run` → 承認の 1 行 → 承認 → run し直し → 実状態。
/// 外の service・owner の quota は使わない: model は `llm::fake::Fake`、元の app は試験の偽の Adapter（pad: 文書の文字と保存した文字を持つ）、git は本物（試験の側で作った repo）。
#[cfg(test)]
mod w19_2_engine_wiring {
    use super::CoreEngine;
    use crate::generated::{EngineSetup, Generated, ModelDone, ENGINE_RUNS_FILE};
    use crate::runtime::registry::ActionRegistry;
    use crate::storage::{self, Store};
    use crate::ui::shell::{PageIntent, View};
    use crate::ui::states::RegionState;
    use crate::domain::run::Outcome;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use crate::adapters::sdk::{self, Adapter, Route, Spec};
    use crate::adapters::AdapterRegistry;
    use crate::capability_engine::EnvFacts;
    use crate::domain::action::Effect;
    use crate::domain::binding::Source;
    use crate::domain::capability::{AdapterKind, CapabilityReadiness};
    use crate::domain::run::Observation;
    use crate::llm::fake::Fake;
    use crate::ui::engine_view::{EngineAct, EngineView};
    use serde_json::json;

    const DEMO: &str = "この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい";
    const ASK: &str = "README に 1 行足して保存して";
    const LEASE: &str = "人が対象の app を操作中です（その app が前面）。何も送っていません。Shibubu に戻してから、もう一度頼んでください";

    /// 偽の文書の app の実状態（試験の側の oracle が直に読む）。calls = Adapter が受けた手（送った手の記録）。lease = 人が操作中。
    #[derive(Clone, Default)]
    struct World {
        text: Arc<Mutex<String>>,
        saved: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        lease: Arc<Mutex<bool>>,
    }
    impl World {
        fn writes(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().filter(|c| !c.ends_with(".read")).cloned().collect()
        }
    }

    static PAD: [Spec; 3] = [
        Spec { id: "pad.text.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "pad.text.set", effect: Effect::LocalWrite, approval: true, scopes: &[], input: r#"{"type":"object","required":["text"],"properties":{"text":{"type":"string"}}}"#, verifier: "read_back" },
        Spec { id: "pad.save", effect: Effect::LocalWrite, approval: true, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
    ];

    struct Pad(World);
    impl Adapter for Pad {
        fn id(&self) -> &'static str {
            "pad"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::LocalCore
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            "pad"
        }
        fn specs(&self) -> &'static [Spec] {
            &PAD
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            let w = &self.0;
            if action != "pad.text.read" && *w.lease.lock().unwrap() {
                return (Outcome::Rejected { reason: LEASE.into() }, None);
            }
            w.calls.lock().unwrap().push(action.into());
            match action {
                "pad.text.set" => *w.text.lock().unwrap() = input["text"].as_str().unwrap_or("").into(),
                "pad.save" => *w.saved.lock().unwrap() = w.text.lock().unwrap().clone(),
                _ => {}
            }
            let payload = json!({ "text": *w.text.lock().unwrap(), "saved": *w.saved.lock().unwrap() });
            (Outcome::Verified { evidence: vec!["pad".into()] }, Some(sdk::observe("pad", sdk::now(), payload)))
        }
        fn cleanup(&mut self) {}
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-w19-2-wire-{name}-{}-{}", std::process::id(), storage::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn step(id: &str, cap: &str, input: Value) -> Value {
        json!({ "id": id, "capability": cap, "input_json": input.to_string() })
    }

    fn check(action: &str, input: Value, pointer: &str, expect: &str, value: Value) -> Value {
        json!({ "action": action, "input_json": input.to_string(), "pointer": pointer, "expect": expect, "value_json": value.to_string() })
    }

    /// model の答え: Project の編集として読めない（unsupported）→ 1 操作 → pad に文字を入れて保存する手順（2 手とも承認が要る）。後ろの 2 つは承認の後に
    /// 新しい run になる時の interpret（WF-012: 止まった run の続きは model を呼ばないので、続きの道では余る）。
    fn answers(generate: Value) -> Vec<Result<Value, crate::llm::LlmError>> {
        let edit = json!({ "steps": [{ "verb": "unsupported", "name": ASK }] });
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "README に 1 行足して保存する", "success": "保存した文字に新しい行が在る", "target": null }] });
        vec![Ok(edit), Ok(ops.clone()), Ok(generate), Ok(ops.clone()), Ok(ops)]
    }

    fn pad_answer(text: &str) -> Value {
        json!({
            "none_why": null, "workflow_steps": [step("set", "pad.text.set", json!({ "text": text })), step("save", "pad.save", json!({}))],
            // WF-007: 字を置く手の済みは置いた字そのものの読み戻し（contains だけなら Partial）
            "checks": [check("pad.text.read", json!({}), "/saved", "equals", json!(text))],
            "recipe": null, "trial": null,
        })
    }

    /// Project を 1 つ作り、model と Engine（registry = 渡した物を run ごとに）を入れた Generated。
    fn wired(llm: Fake, registry: impl Fn() -> AdapterRegistry + Send + Sync + 'static, dir: &std::path::Path) -> (Generated, String) {
        wired_with(llm, registry, dir, Default::default())
    }

    /// `wired` の Engine の MCP の設定を渡せる形（W19-3: 解析の道具と見本の app の公式の MCP）。
    fn wired_with(llm: Fake, registry: impl Fn() -> AdapterRegistry + Send + Sync + 'static, dir: &std::path::Path, mcp: crate::mcp::McpConfig) -> (Generated, String) {
        wired_in(llm, registry, dir, mcp, None)
    }

    /// `wired_with` の Project に repo（workspace）を持たせ、読み戻しを `readback` まで待つ形（G81: 書類の保存を disk で確かめる）
    fn wired_in(llm: Fake, registry: impl Fn() -> AdapterRegistry + Send + Sync + 'static, dir: &std::path::Path, mcp: crate::mcp::McpConfig, repo: Option<(&std::path::Path, Duration)>) -> (Generated, String) {
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, repo.map(|(r, _)| r), None).unwrap().unwrap() else { panic!() };
        g.set_llm(Ok(Box::new(llm)));
        let readback = repo.map_or(Duration::ZERO, |(_, w)| w);
        let engine = CoreEngine { registry: Arc::new(registry), data_dir: dir.to_path_buf(), mcp, env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        g.set_engine(EngineSetup { engine: Arc::new(engine), data_dir: dir.to_path_buf() });
        (g, project)
    }

    fn pad_registry(w: &World) -> impl Fn() -> AdapterRegistry + Send + Sync + 'static {
        let w = w.clone();
        move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(Pad(w.clone()))).unwrap();
            r
        }
    }

    /// 混雑でも緑にする期限: 1 分 load を 4 で割った倍率（1〜8 倍）を掛ける。待つ対象が来ない（壊れている）なら伸ばしても赤のまま
    /// （期限の上限を伸ばすだけで性質は変えない）。load は `sysctl -n vm.loadavg`（`{ 1.23 4.56 7.89 }`）。読めなければ 1 倍。
    fn patient(base: std::time::Duration) -> std::time::Duration {
        let load = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().find_map(|w| w.parse::<f64>().ok()))
            .unwrap_or(0.0);
        base.mul_f64((load / 4.0).clamp(1.0, 8.0))
    }

    /// 裏の thread の答えが届くまで（最長 20 秒・混雑なら patient 倍）。
    fn settle(g: &mut Generated) {
        let end = std::time::Instant::now() + patient(Duration::from_secs(20));
        while std::time::Instant::now() < end {
            if g.poll_engine() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("Engine の答えが 20 秒（混雑なら伸ばした分）届かない");
    }

    fn runs(dir: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join(ENGINE_RUNS_FILE)).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// F1 の形: 一言 → 承認の 1 行（2 手を 1 行で・何も送っていない）→ 承認 1 回 → 2 手が順に送られ、保存した文字に新しい行。承認しなければ 0 件。
    #[test]
    fn w19_2_page_request_runs_through_one_approval_and_changes_the_real_state() {
        let dir = tmp("f1");
        let w = World::default();
        *w.text.lock().unwrap() = "# README".into();
        let (mut g, project) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&w), &dir);
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).expect("編集として読めない文は Engine へ（拒否の 1 行にしない）");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Running { .. })), "{:?}", g.engine_view(&project));
        settle(&mut g);
        let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の 1 行: {:?}", g.engine_view(&project)) };
        assert_eq!(steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), ["pad.text.set", "pad.save"], "2 手を 1 回の承認で（手ごとに止めない）");
        assert!(w.writes().is_empty(), "承認の前は何も送らない: {:?}", w.writes());
        assert_eq!(*w.saved.lock().unwrap(), "");
        g.engine_act(&project, EngineAct::Approve);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Running { .. })));
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(w.writes(), ["pad.text.set", "pad.save"], "書いてから保存（順）");
        assert_eq!(*w.saved.lock().unwrap(), "# README\nnonce-w19", "試験の側の oracle: 保存した文字");
        let hist = g.engine_history();
        assert!(hist.iter().any(|l| l.contains(ASK)), "{hist:?}");
        for internal in ["pad.", "wf-", "op1", "/saved"] {
            assert!(hist.iter().all(|l| !l.contains(internal)), "{internal} を履歴に出さない: {hist:?}");
        }
        let r = runs(&dir);
        assert_eq!(r.len(), 2, "止めた run と通った run の 2 行: {r:?}");
        assert!(r[0].get("error").is_some() && r[1]["record"]["receipts"][0]["steps"].as_array().unwrap().len() == 2, "{r:?}");
        // 承認しない → 何も送らない（負の対照の側の道）
        let w2 = World::default();
        let (mut g2, p2) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&w2), &tmp("f1-cancel"));
        g2.page_intent(&p2, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g2);
        g2.engine_act(&p2, EngineAct::Cancel);
        assert!(g2.engine_view(&p2).is_none());
        assert!(w2.writes().is_empty() && w2.saved.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19-3 の画面の道: 一言 → 通常の探索（生成の model が手段無しと答える）→ 解析の承認の 1 行（解析の手 1 つ・送る手ではない・承認の前は REA を呼ばない）→
    /// 承認 1 回 → 解析・生成・写しの試験 → 書く手の承認の 1 行 → 承認 → 本物が変わる。Page が作る承認の記録（run_id つき）が、Engine の解析の承認としてそのまま通る事を見る。
    #[test]
    fn w19_3_page_asks_to_analyze_then_to_write_and_changes_the_real_state() {
        use crate::capability_engine::w19_3_body::{cli_answer, mcp_cfg, no_means, quill_app, rea_calls, rea_starts, serial};
        let _one = serial();
        let dir = tmp("w19-3");
        let app = quill_app(&dir);
        let real = dir.join("real").join("notes.quill.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, json!({ "title": "Old", "body": "本文" }).to_string()).unwrap();
        let title = || serde_json::from_str::<Value>(&std::fs::read_to_string(&real).unwrap()).unwrap()["title"].clone();
        let edit = json!({ "steps": [{ "verb": "unsupported", "name": ASK }] });
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "文書の題を変える", "success": "題が Shibubu Demo", "target": app.display().to_string() }] });
        let gen = cli_answer(&app, &real, &dir.join("trials").join("op1").join("notes.quill.json"), "Shibubu Demo");
        let fake = Fake::new(vec![Ok(edit), Ok(ops.clone()), Ok(no_means()), Ok(no_means()), Ok(ops.clone()), Ok(gen), Ok(ops)]);
        let (mut g, project) = wired_with(fake, AdapterRegistry::default, &dir, mcp_cfg(&dir, ""));
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("解析の承認の 1 行: {:?}", g.engine_view(&project)) };
        assert_eq!(steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), ["analysis.rea"]);
        let line = crate::ui::engine_view::approval_line(&goal, &steps);
        assert!(line.contains("No existing connection can do") && line.contains("using 2 tools REA says won't break anything") && line.contains("(Quillpad, version 1.0) using") && line.contains("Nothing is sent") && !line.contains("will send"), "{line}");
        assert!(rea_calls(&dir).is_empty(), "承認の前は解析しない");
        assert_eq!(rea_starts(&dir), 1, "道具を確かめるのに 1 回だけ起こす");
        assert_eq!(title(), "Old");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert_eq!(rea_starts(&dir), 2, "承認の後の run も REA を 1 回だけ起こす（確かめた session で解析する・W19-3-fix）");
        let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("書く手の承認の 1 行: {:?}", g.engine_view(&project)) };
        assert_eq!(steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), ["quillpad.doc.title.set"]);
        assert_eq!(rea_calls(&dir).len(), 2, "Page の承認の後に 1 回だけ解析した（1 つの session で open_binary と inspect_artifact）");
        assert_eq!(title(), "Old", "書く手の承認まで本物を変えない");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(title(), "Shibubu Demo", "試験の側の oracle（disk を直に読む）");
        assert_eq!(rea_calls(&dir).len(), 2);
        assert_eq!(rea_starts(&dir), 2, "保存した手順の run は REA を起こさない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19-3-fix（通し）: 一言 → 通常の探索で手段が無い → 解析の承認を求める前に REA を確かめる間、Page の「進めています」の下に
    /// 「REA を起こしています（最長 90 秒）」（AX の AXStaticText の値も全文）→ 起こし終わると消え → 解析の承認の 1 行に替わる。この run で REA は 1 回だけ起きる。
    /// 偽の REA は即答するので、Engine の thread を受け口の Some の所と run の終わりで止めて Page を読む（G2 の `Hold` と同じ）。
    /// 負の対照（手で取る）: `poll_engine` の写しを外す・`CoreEngine::run` の `set_progress` を外す → 1 行が出ずに赤
    #[test]
    fn w19_3_fix_page_shows_the_rea_line_while_it_starts() {
        use crate::capability_engine::w19_3_body::{mcp_cfg, no_means, quill_app, rea_starts, serial};
        use crate::generated::{EngineAnswer, EngineRequest, PageEngine};
        use eframe::egui;
        use std::sync::mpsc;
        struct Hold(CoreEngine, Arc<Mutex<mpsc::Receiver<()>>>);
        impl PageEngine for Hold {
            fn run(&self, mut req: EngineRequest) -> EngineAnswer {
                let (page, rx) = (req.progress.clone(), self.1.clone());
                req.progress = Arc::new(move |line| {
                    let some = line.is_some();
                    page(line);
                    if some {
                        let _ = rx.lock().unwrap().recv();
                    }
                });
                let a = self.0.run(req);
                let _ = self.1.lock().unwrap().recv();
                a
            }
        }
        let _one = serial();
        let dir = tmp("w19-3-fix");
        let app = quill_app(&dir);
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "文書の題を変える", "success": "題が Shibubu Demo", "target": app.display().to_string() }] });
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_llm(Ok(Box::new(Fake::new(vec![Ok(ops), Ok(no_means()), Ok(no_means())]))));
        let core = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: mcp_cfg(&dir, ""), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
        let (go, rx) = mpsc::channel();
        g.set_engine(EngineSetup { engine: Arc::new(Hold(core, Arc::new(Mutex::new(rx)))), data_dir: dir.clone() });
        assert!(g.start_engine(&project, ASK, vec![], None));
        let until = |g: &mut Generated, want: Option<&str>| {
            for _ in 0..2000 {
                g.poll_engine();
                match g.engine_view(&project) {
                    Some(EngineView::Running { waiting, .. }) if waiting.as_deref() == want => return,
                    Some(EngineView::Running { .. }) => std::thread::sleep(Duration::from_millis(10)),
                    other => panic!("待つ間に Running でなくなった: {other:?}"),
                }
            }
            panic!("Page の 1 行が {want:?} にならない: {:?}", g.engine_view(&project));
        };
        // Page の Engine の行を描いた AX の木の値（Label = AXStaticText の値）
        let ax = |g: &Generated| -> Vec<String> {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let view = g.engine_view(&project).cloned().expect("Engine の行");
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::ui::engine_view::draw(ui, &view);
                });
            });
            out.platform_output.accesskit_update.expect("AccessKit の木").nodes.iter().filter_map(|(_, n)| n.value().map(str::to_owned)).collect()
        };
        let line = "Starting the analysis tool (up to 90 s)";
        until(&mut g, Some(line));
        assert!(ax(&g).iter().any(|v| v == line), "AX の値に全文が無い: {:?}", ax(&g));
        assert_eq!(rea_starts(&dir), 0, "1 行は起こす前に出る（待つ間ずっと見える）");
        go.send(()).unwrap(); // 起こして tools/list を読む
        until(&mut g, None);
        assert!(!ax(&g).iter().any(|v| v.contains("Starting the analysis tool")), "起こし終わった後も 1 行が残る: {:?}", ax(&g));
        go.send(()).unwrap(); // 答えを Page へ
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { steps, .. }) if steps[0].action == "analysis.rea"), "解析の承認の 1 行に替わる: {:?}", g.engine_view(&project));
        assert_eq!(rea_starts(&dir), 1, "この run で REA は 1 回だけ起きる");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19a2 の実機で見つけた（直し）: 裏の thread の答え（model の一言・編集・Engine）が届いた後に画面を起こす。idle の frame は 0 なので、
    /// 起こさないと「AI が一言を読んでいます」のまま次の入力まで描かれない（背景の run で 2 回読んでも変わらず、別の click の後に Project が開いた）。
    #[test]
    fn w19a2_background_answers_wake_the_screen() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // 起こされた回数を数える ctx（egui は描くまで次の request_repaint を 1 つにまとめるので、段ごとに新しい ctx にする）
        let counted = || {
            let ctx = eframe::egui::Context::default();
            let n = Arc::new(AtomicUsize::new(0));
            let m = n.clone();
            ctx.set_request_repaint_callback(move |_| {
                m.fetch_add(1, Ordering::SeqCst);
            });
            (ctx, n)
        };
        let wait = |n: &AtomicUsize, what: &str| {
            let end = std::time::Instant::now() + patient(Duration::from_secs(5));
            while std::time::Instant::now() < end {
                if n.load(Ordering::SeqCst) > 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("{what} の答えで画面を起こさない");
        };
        let world = World::default();
        let dir = tmp("wake");
        // 一言（Home の「作る」）: 答えの形が違っても届いたら起こす（起きた frame で手元の推定に落ちる）
        let (mut g0, _) = wired(Fake::new(vec![Ok(json!({}))]), pad_registry(&world), &dir);
        let (ctx0, n0) = counted();
        g0.wake_on_terminal(&ctx0);
        n0.store(0, Ordering::SeqCst);
        assert!(g0.start_create("メモを書きたい", None, &[]));
        wait(&n0, "model の一言");
        let (mut g, project) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&world), &dir);
        let (ctx1, n1) = counted();
        g.wake_on_terminal(&ctx1);
        n1.store(0, Ordering::SeqCst);
        assert!(g.start_edit(&project, ASK), "model の編集を裏で始める");
        wait(&n1, "model の編集");
        let (ctx2, n2) = counted();
        g.wake_on_terminal(&ctx2);
        n2.store(0, Ordering::SeqCst);
        assert!(matches!(g.poll_model_job(|| None), Some(ModelDone::Edited { .. })), "起きた frame で答えを当てる");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Running { .. })), "編集として読めない文は Engine へ");
        wait(&n2, "Engine");
        assert!(g.poll_engine(), "起きた frame で Engine の答えを当てる");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F2: 人が操作中の間は、承認しても送らず拒否の 1 行（承認の 1 行に戻さない）・送った手 0。
    #[test]
    fn w19_2_human_lease_refuses_with_one_line_and_sends_nothing() {
        let dir = tmp("f2");
        let w = World::default();
        let (mut g, project) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&w), &dir);
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g);
        *w.lease.lock().unwrap() = true;
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("拒否の 1 行: {:?}", g.engine_view(&project)) };
        // G73: 承認では通らない拒否の行に承認の button は無い = ▣ Needs approval にしない（UI-6a・理由の文だけの Ready）
        assert_eq!(state, RegionState::Ready);
        assert!(line.contains("操作中"), "{line}");
        assert!(w.writes().is_empty(), "送った手 0: {:?}", w.writes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 26 通目: 1 つの一言で git（既存の公式の Adapter・本物の repo）→ pad へ `$from` で渡す Workflow を、承認 1 回の後は利用者の手なしで 1 本の run で通す。
    /// Handoff と各手の Outcome が記録に残る。branch の先を進めて同じ一言をもう一度 → 生成 0 回で新しい値（固定の値を渡す実装なら赤）。
    #[test]
    fn w19_2_one_request_hands_data_across_two_tools_in_one_run() {
        let dir = tmp("handoff");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(&repo).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
            assert!(o.status.success(), "git {args:?}");
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        std::fs::write(repo.join("README.md"), "w19-2\n").unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "README.md"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "first"]);
        let uri = format!("file://{}", repo.display());
        let answer = json!({
            "none_why": null, "workflow_steps": [
                step("read", "git.status.read", json!({ "uri": uri })),
                step("set", "pad.text.set", json!({ "text": { "$from": "read.head.sha" } })),
                step("save", "pad.save", json!({})),
            ],
            "checks": [check("pad.text.read", json!({}), "/saved", "equals", json!({ "$from": "read.head.sha" }))],
            "recipe": null, "trial": null,
        });
        let w = World::default();
        let pw = w.clone();
        let both = move || {
            let mut r = AdapterRegistry::with_local_adapters();
            r.register(Box::new(Pad(pw.clone()))).unwrap();
            r
        };
        // 1 回目: 編集として読めない・分ける・作る。2 回目: 編集として読めない・分ける（どちらも承認の後は止まった所から続け、model を呼ばない・WF-012）
        let r = answers(answer);
        let replies = vec![r[0].clone(), r[1].clone(), r[2].clone(), r[0].clone(), r[1].clone()];
        let (mut g, project) = wired(Fake::new(replies), both, &dir);
        let ask = "repo の今の commit を pad に書いて保存して";
        g.page_intent(&project, PageIntent::Edit { text: ask.into() }).unwrap();
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { steps, .. }) if steps.len() == 2), "{:?}", g.engine_view(&project));
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        let head = git(&["rev-parse", "HEAD"]);
        assert_eq!(*w.saved.lock().unwrap(), head, "試験の側の oracle: pad に保存した値 = repo の HEAD");
        let rec = runs(&dir).pop().unwrap()["record"].clone();
        let steps = rec["receipts"][0]["steps"].as_array().unwrap().clone();
        assert_eq!(steps.len(), 3, "1 本の run で 3 手: {rec}");
        assert_eq!(steps[1]["handoffs"][0]["from_step"], "read", "{rec}");
        assert_eq!(steps[1]["handoffs"][0]["digest"], crate::crypto::sha256_hex(json!(head).to_string().as_bytes()));
        // branch の先を進めて同じ一言 → 生成 0 回・新しい値（前の手の出力を使わず固定の値を渡す実装なら、ここで古い値になって赤）
        std::fs::write(repo.join("README.md"), "w19-2 2\n").unwrap();
        git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qam", "second"]);
        g.page_intent(&project, PageIntent::Edit { text: ask.into() }).unwrap();
        settle(&mut g);
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let head2 = git(&["rev-parse", "HEAD"]);
        assert_ne!(head, head2);
        assert_eq!(*w.saved.lock().unwrap(), head2);
        assert_eq!(runs(&dir).pop().unwrap()["record"]["generated"], json!([]), "2 回目は生成 0 回");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19-1 の残り（operator 2026-10-24「同意の一本化」）: Page の「承認して送る」は宛先の app の AI に任せる同意を、旧 Desktop の画面と同じ
    /// 1 つの鍵（`surface::act_consents` の file の bundle id）に書く → 旧 Desktop の画面も同意済（「AI に任せる」を出さない）。同意を消すと、
    /// 承認つきの Page の act でも `MacGui` の門で送らず needs_consent（act の記録に 1 行・送った数は増えない）・Page は「REAPER を AI に任せる」→
    /// 押すと同意を書いて同じ手を送る。偽の bridge（tests/fake-surface-bridge.py・REAPER = com.cockos.reaper・pid 222）・本物の MacGui と Engine。
    #[cfg(unix)]
    #[test]
    fn w19_1_page_approval_writes_the_one_ai_consent_key_and_without_it_the_act_needs_consent() {
        use crate::adapters::mac_gui::MacGui;
        use crate::generated::{EngineAnswer, EngineRequest, PageEngine};
        use crate::gui::{ActLog, NotSent, WindowRef, ACT_LOG_FILE};
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        /// 承認で同意を書かない Page の Engine（同意を消した後の act を MacGui の門まで届ける試験の道具）
        struct NoGrant(CoreEngine, Arc<AtomicBool>);
        impl PageEngine for NoGrant {
            fn run(&self, mut req: EngineRequest) -> EngineAnswer {
                if self.1.load(Ordering::SeqCst) {
                    req.grants.clear();
                }
                self.0.run(req)
            }
        }
        let dir = tmp("consent");
        let (consents, acts, sh) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE), dir.join("bridge.sh"));
        std::fs::write(&sh, format!("#!/bin/sh\nexec python3 '{}/tests/fake-surface-bridge.py'\n", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (c, a) = (consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(sh.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        let typed = json!({ "pid": 222, "window_id": 300, "app_name": "REAPER", "label": "本文", "role": "AXTextArea", "text": "nonce-w19" });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("set", "desktop.ax.type_text", typed)],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls/2/value", "equals", json!("nonce-w19"))],
            "recipe": null, "trial": null,
        });
        let r = answers(gen);
        // 1 本目: 読めない・分ける・作る（承認の後は止まった所から = model を呼ばない・WF-012）。2 本目: 読めない・分ける／承認の後（同意なし）も
        // 止まった所から／「AI に任せる」の後: 止まった run は無い（同意の 1 行は待つ run にしない）ので新しい run で分ける
        let replies = vec![r[0].clone(), r[1].clone(), r[2].clone(), r[0].clone(), r[1].clone(), r[1].clone()];
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_llm(Ok(Box::new(Fake::new(replies))));
        let drop_grants = Arc::new(AtomicBool::new(false));
        let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::ZERO, act_consents: consents.clone() };
        g.set_engine(EngineSetup { engine: Arc::new(NoGrant(core, drop_grants.clone())), data_dir: dir.clone() });
        // 旧 Desktop の画面（① の窓の下の 1 行）が「AI に任せる」を出すか
        let panel_asks = || {
            let ctx = eframe::egui::Context::default();
            ctx.enable_accesskit();
            let w = WindowRef { pid: 222, window_id: 300, app_name: "REAPER".into(), title: String::new(), bundle_id: "com.cockos.reaper".into(), document: String::new() };
            let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(800.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    crate::app::act_consent_row(ui, &w, &consents);
                });
            });
            out.platform_output.accesskit_update.expect("AccessKit の木").nodes.iter().any(|(_, n)| n.author_id() == Some("gui-act-consent:com.cockos.reaper"))
        };
        let log = || if acts.exists() { ActLog::read(&acts).unwrap() } else { vec![] };
        let sent = || log().iter().filter(|r| r.not_sent.is_none()).count();
        let asked = || log().iter().filter(|r| r.not_sent == Some(NotSent::NeedsConsent)).count();
        assert!(panel_asks(), "同意の前は旧 Desktop の画面が「AI に任せる」を出す");
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の 1 行: {:?}", g.engine_view(&project)) };
        let line = crate::ui::engine_view::approval_line(&goal, &steps);
        assert!(line.contains("once, and hands REAPER to the AI)"), "承認の 1 行が同意も書く事を言う: {line}");
        assert_eq!((sent(), asked()), (0, 0), "承認の前は何も送らない");
        // Page で承認 → 同意の鍵に REAPER・送って読み戻す・旧 Desktop の画面も同意済
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert!(crate::surface::act_consents(&consents).contains("com.cockos.reaper"), "Page の承認が同意の鍵に書いていない");
        assert!(!panel_asks(), "Page で承認したのに旧 Desktop の画面が同意を聞く（鍵が 2 つ）");
        assert_eq!((sent(), asked()), (1, 0));
        // 同意を消す → 承認つきの Page の act でも送らず needs_consent・Page に「REAPER を AI に任せる」
        std::fs::write(&consents, "").unwrap();
        assert!(panel_asks());
        drop_grants.store(true, Ordering::SeqCst);
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g);
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let Some(EngineView::Consent { steps, line, .. }) = g.engine_view(&project).cloned() else { panic!("同意の 1 行: {:?}", g.engine_view(&project)) };
        assert!(line.contains("REAPER isn't allowed to be controlled by the AI yet") && line.contains("nothing was sent"), "{line}");
        assert_eq!(crate::ui::engine_view::consent_label(&steps), "Hand REAPER to the AI");
        assert_eq!((sent(), asked()), (1, 1), "同意が無い間は送らず act の記録に needs_consent の 1 行");
        // 「REAPER を AI に任せる」を押す → 同意を書いて同じ手を送る
        drop_grants.store(false, Ordering::SeqCst);
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert!(crate::surface::act_consents(&consents).contains("com.cockos.reaper") && !panel_asks());
        assert_eq!((sent(), asked()), (2, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19-3b（operator 2026-10-24）: 一言 → 承認 → 送って読み戻す（F1）→ Page の移動 → 自然言語の編集（「TextEdit を右へ」）→ Shibubu の再起動
    /// （同じ store と data dir で Generated を作り直す）→ 元の app も起こし直して pid が変わる → 同じ一言は保存した手順を使い（生成 0 回）、
    /// 手順の input の bundle id と文書の題名で宛先を今の窓に決め直して送り、読み戻しが揃う（act の記録の宛先は新しい pid）。
    /// 負の対照（手で取る）: `MacGui::invoke` の決め直しを外すと前の pid を指して止まる。許可が無い時は tests/adapters.rs の W19-3b の試験。
    #[cfg(unix)]
    #[test]
    fn w19_3b_saved_steps_retarget_by_bundle_after_page_move_nl_edit_and_restart() {
        use crate::adapters::mac_gui::MacGui;
        use crate::gui::{ActLog, ACT_LOG_FILE};
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("w19-3b");
        let (consents, acts, sh, db) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE), dir.join("bridge.sh"), dir.join("shibubu.db"));
        // 偽の bridge の TextEdit（README.md の窓 1 つ）。pid は起こし直すと変わる
        let bridge = |pid: u32| {
            let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
            std::fs::write(&sh, format!("#!/bin/sh\nexport FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_APP_PID={pid}\nexec python3 '{py}'\n")).unwrap();
            std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        bridge(222);
        let (s, c, a) = (sh.clone(), consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(s.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        let at = json!({ "pid": 222, "window_id": 300, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "README.md" });
        let mut typed = at.clone();
        typed["label"] = json!("本文");
        typed["role"] = json!("AXTextArea");
        typed["text"] = json!("nonce-3b");
        let gen = json!({
            "none_why": null, "workflow_steps": [step("set", "desktop.ax.type_text", typed)],
            "checks": [check("desktop.window.state", at, "/controls/2/value", "equals", json!("nonce-3b"))],
            "recipe": null, "trial": null,
        });
        let r = answers(gen);
        let wire = || {
            let mut g = Generated::start(Store::open(&db).unwrap(), ActionRegistry::with_local_actions(), None);
            let core = CoreEngine { registry: Arc::new(registry.clone()), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::ZERO, act_consents: consents.clone() };
            g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
            g
        };
        let sent = || ActLog::read(&acts).map(|v| v.into_iter().filter(|r| r.not_sent.is_none()).map(|r| r.receipt.target).collect::<Vec<_>>()).unwrap_or_default();
        let approve = |g: &mut Generated, project: &str| {
            g.page_intent(project, PageIntent::Edit { text: ASK.into() }).unwrap();
            settle(g);
            assert!(matches!(g.engine_view(project), Some(EngineView::Approve { .. })), "{:?}", g.engine_view(project));
            g.engine_act(project, EngineAct::Approve);
            settle(g);
            assert!(matches!(g.engine_view(project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(project));
        };
        // 1 回目: 読めない・分ける・作る／承認の後: 分ける。その後の自然言語の編集
        let mut g = wire();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_llm(Ok(Box::new(Fake::new(vec![r[0].clone(), r[1].clone(), r[2].clone(), r[1].clone(), Ok(json!({ "steps": [{ "verb": "move_right", "name": "textedit" }] }))]))));
        approve(&mut g, &project);
        assert_eq!(sent().iter().map(|t| (t.pid, t.window_id)).collect::<Vec<_>>(), [(222, Some(300))]);
        // Page の移動（足して戻る）と自然言語の編集
        g.page_intent(&project, PageIntent::Add).unwrap();
        g.page_intent(&project, PageIntent::Back).unwrap();
        g.page_intent(&project, PageIntent::Edit { text: "TextEdit を右へ".into() }).expect("自然言語の編集が当たる");
        // Shibubu の再起動（同じ store と data dir）・元の app も起こし直す（pid が変わる）・同意も消す（2 回目の承認が今の pid の app に書き直す）
        drop(g);
        bridge(333);
        assert!(crate::surface::act_consents(&consents).contains("com.apple.TextEdit"), "1 回目の承認が同意を書いた");
        std::fs::write(&consents, "").unwrap();
        let mut g = wire();
        g.open_project(&project, None).unwrap();
        g.set_llm(Ok(Box::new(Fake::new(vec![r[0].clone(), r[1].clone(), r[1].clone()]))));
        approve(&mut g, &project);
        let to = sent();
        assert_eq!(to.iter().map(|t| (t.pid, t.window_id)).collect::<Vec<_>>(), [(222, Some(300)), (333, Some(300))], "2 回目は今の pid へ: {to:?}");
        assert!(to.iter().all(|t| t.bundle_id == "com.apple.TextEdit"));
        assert!(crate::surface::act_consents(&consents).contains("com.apple.TextEdit"), "古い pid の手の承認でも、決め直した窓の app に同意を書く");
        assert_eq!(runs(&dir).pop().unwrap()["record"]["generated"], json!([]), "保存した手順を使った（生成 0 回）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W19-3b G2 の口: Adapter の進行の受け口（`sdk::Progress`）と Page の Engine の口（`EngineRequest::progress`）は同じ形・Page の 1 行は `Running::waiting`。
    #[test]
    fn w19_3b_progress_signatures() {
        let _: fn(&mut AdapterRegistry, sdk::Progress) = AdapterRegistry::set_progress;
        let p: sdk::Progress = Arc::new(|_| {});
        let req = crate::generated::EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "g".into(), consents: vec![], grants: vec![], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: p, runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        (req.progress)(None);
        let _ = EngineView::Running { goal: "g".into(), waiting: Some("TextEdit が答えないので待っています（最長 32 秒）".into()), stopping: false };
    }

    /// W19-3b G2（通し）: 手の宛先の app（偽の bridge の TextEdit）が AX に答えず Engine が待つ間、Page の「進めています」の下に
    /// 「TextEdit が答えないので待っています（最長 32 秒）」が出て（AX の AXStaticText の値も全文）、読み直しが終わると消え、結果の 1 行に替わる。
    /// 偽の bridge は即答するので、Engine の thread を受け口（`EngineRequest::progress`）の Some の所と run の終わりで止めて Page を読む（`Hold`）。
    /// 負の対照（手で取る）: `CoreEngine::run` の `set_progress` を外すと 1 行が出ずに赤・`poll_engine` の写しを外しても赤
    #[cfg(unix)]
    #[test]
    fn w19_3b_page_shows_the_waiting_line_until_the_slow_app_answers() {
        use crate::adapters::mac_gui::MacGui;
        use crate::generated::{EngineAnswer, EngineRequest, PageEngine};
        use eframe::egui;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc;
        struct Hold(CoreEngine, Arc<Mutex<mpsc::Receiver<()>>>);
        impl PageEngine for Hold {
            fn run(&self, mut req: EngineRequest) -> EngineAnswer {
                let (page, rx) = (req.progress.clone(), self.1.clone());
                req.progress = Arc::new(move |line| {
                    let some = line.is_some();
                    page(line);
                    if some {
                        let _ = rx.lock().unwrap().recv();
                    }
                });
                let a = self.0.run(req);
                let _ = self.1.lock().unwrap().recv();
                a
            }
        }
        let dir = tmp("w19-3b-g2");
        let sh = dir.join("bridge.sh");
        std::fs::write(&sh, format!("#!/bin/sh\nexport FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_WL_WAKE=10\nexec python3 '{}/tests/fake-surface-bridge.py'\n", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = sh.clone();
        let core = CoreEngine {
            registry: Arc::new(move || {
                let mut r = AdapterRegistry::default();
                r.register(Box::new(MacGui::at(s.clone()))).unwrap();
                r
            }),
            data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE),
        };
        let (go, rx) = mpsc::channel();
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Hold(core, Arc::new(Mutex::new(rx)))), data_dir: dir.clone() });
        // 読む手（承認は要らない）の宛先を bundle id と文書の題名で今の窓に決め直す = 答えない app を待つ道
        // （G70 の前は承認の同意の宛先 = `CoreEngine::grant` も窓を読んでこの道を通った・G70 からは起動中の app だけを読み、窓を待たない）
        let at = json!({ "pid": 222, "window_id": 300, "app_name": "TextEdit", "bundle_id": "com.apple.TextEdit", "document": "README.md" });
        let gen = json!({ "none_why": null, "workflow_steps": [step("look", "desktop.window.state", at.clone())], "checks": [check("desktop.window.state", at, "/complete", "equals", json!(true))], "recipe": null, "trial": null });
        let r = answers(gen);
        g.set_llm(Ok(Box::new(Fake::new(vec![r[1].clone(), r[2].clone()]))));
        assert!(g.start_engine(&project, ASK, vec![], None));
        let line = "Waiting because TextEdit isn't answering (up to 32 s)";
        let until = |g: &mut Generated, want: Option<&str>| {
            for _ in 0..2000 {
                g.poll_engine();
                match g.engine_view(&project) {
                    Some(EngineView::Running { waiting, .. }) if waiting.as_deref() == want => return,
                    Some(EngineView::Running { .. }) => std::thread::sleep(Duration::from_millis(10)),
                    other => panic!("待つ間に Running でなくなった: {other:?}"),
                }
            }
            panic!("Page の 1 行が {want:?} にならない: {:?}", g.engine_view(&project));
        };
        // Page の Engine の行を描いた AX の木の値（Label = AXStaticText の値）
        let ax = |g: &Generated| -> Vec<String> {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let view = g.engine_view(&project).cloned().expect("Engine の行");
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::ui::engine_view::draw(ui, &view);
                });
            });
            out.platform_output.accesskit_update.expect("AccessKit の木").nodes.iter().filter_map(|(_, n)| n.value().map(str::to_owned)).collect()
        };
        until(&mut g, Some(line));
        assert!(ax(&g).iter().any(|v| v == line), "AX の値に全文が無い: {:?}", ax(&g));
        go.send(()).unwrap(); // 読み直しへ（偽の bridge は 30 秒の上限で答える）
        until(&mut g, None);
        assert!(!ax(&g).iter().any(|v| v.contains("isn't answering")), "読み直しの後も 1 行が残る: {:?}", ax(&g));
        go.send(()).unwrap(); // 答えを Page へ
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { .. })), "結果の 1 行に替わる: {:?}", g.engine_view(&project));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同意を書く宛先は承認の 1 行で見せた app だけ: 窓の app 名（bridge の読み）が input の `app_name` と違えば書かない・同じなら書く。
    #[cfg(unix)]
    #[test]
    fn w19_1_grant_writes_only_the_app_shown_in_the_approval_line() {
        use crate::adapters::mac_gui::MacGui;
        use crate::generated::EngineRequest;
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("grant-name");
        let (consents, sh) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join("bridge.sh"));
        std::fs::write(&sh, format!("#!/bin/sh\nexec python3 '{}/tests/fake-surface-bridge.py'\n", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = sh.clone();
        let core = CoreEngine {
            registry: Arc::new(move || {
                let mut r = AdapterRegistry::default();
                r.register(Box::new(MacGui::at(s.clone()))).unwrap();
                r
            }),
            data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: consents.clone(),
        };
        let grant = |app: &str| {
            let req = EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "g".into(), consents: vec![], grants: vec![json!({ "pid": 222, "app_name": app })], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
            let mut reg = (core.registry)();
            reg.check();
            core.grant(&mut reg, &req);
            crate::surface::act_consents(&consents)
        };
        assert!(grant("Notes").is_empty(), "見せた app（Notes）と違う app（REAPER）に同意を書いた");
        assert!(grant("REAPER").contains("com.cockos.reaper"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPEN-INPLACE-1101 の 2（owner 240「Need approve には always allow を聞くように」）: 承認の card の Always allow（宛先に `"always": [action の id]`）は、
    /// 同意と同じ読み（bridge の起動中の app・見せた app 名と合う時だけ）で決めた bundle で Always allow の行を書き、app の同意も書く。
    /// `skips_approval` は同じ app・同じ action の手だけ true（別の種類の手・別の app・空は聞く）。ENGINE-AUDIT W7 から押す手（click）は Always allow の行が在っても
    /// 飛ばさない（行は書く・飛ばす範囲は `w7_always_allow_never_skips_a_step_that_deletes_sends_or_buys`）。`always` の無い宛先は同意だけ。
    /// 偽の bridge（tests/fake-surface-bridge.py・REAPER = com.cockos.reaper・pid 222）。
    /// 負の対照: grant の always の腕を外す → 3 つ目が赤・名前の照合の前に書く → 2 つ目が赤・`skips_approval` の CLICK_ACTION の門を外す → 押す手の assert が赤。
    #[cfg(unix)]
    #[test]
    fn open_inplace_always_allow_is_kept_beside_the_consent_for_the_shown_app_and_kind_only() {
        use crate::adapters::mac_gui::MacGui;
        use crate::generated::{EngineRequest, PageEngine};
        use crate::ui::engine_view::ApprovalStep;
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("always");
        let (consents, sh) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join("bridge.sh"));
        std::fs::write(&sh, format!("#!/bin/sh\nexec python3 '{}/tests/fake-surface-bridge.py'\n", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = sh.clone();
        let core = CoreEngine {
            registry: Arc::new(move || {
                let mut r = AdapterRegistry::default();
                r.register(Box::new(MacGui::at(s.clone()))).unwrap();
                r
            }),
            data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: consents.clone(),
        };
        let grant = |to: Value| {
            let req = EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "g".into(), consents: vec![], grants: vec![to], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
            let mut reg = (core.registry)();
            reg.check();
            core.grant(&mut reg, &req);
            crate::surface::always_allowed(&consents).into_iter().map(|a| (a.bundle_id, a.app_name, a.action)).collect::<Vec<_>>()
        };
        assert!(grant(json!({ "pid": 222, "app_name": "REAPER" })).is_empty(), "Always allow を押していないのに覚えた");
        assert!(grant(json!({ "pid": 222, "app_name": "Notes", "always": ["desktop.ax.click"] })).is_empty(), "見せた app（Notes）と違う app（REAPER）に Always allow を書いた");
        let got = grant(json!({ "pid": 222, "app_name": "REAPER", "always": ["desktop.ax.click"] }));
        assert_eq!(got, [("com.cockos.reaper".to_owned(), "REAPER".to_owned(), "desktop.ax.click".to_owned())]);
        assert!(crate::surface::act_consents(&consents).contains("com.cockos.reaper"), "Always allow で app の同意を書かない");
        let step = |action: &str, app: &str| ApprovalStep { action: action.into(), input: json!({ "pid": 222, "app_name": app, "label": "Library" }), digest: "d".into(), scopes: vec![] };
        // W7: 承認の要る押す手は gui::asks に当たった手だけ = Always allow で飛ばさない（覚えた手で飛ばすのは下の試験の menu・打つ手）
        assert!(!core.skips_approval(&[step("desktop.ax.click", "REAPER"), step("desktop.ax.click", "REAPER")]), "W7: 押す手の承認を Always allow で飛ばした");
        assert!(!core.skips_approval(&[step("desktop.ax.click", "REAPER"), step("desktop.ax.type_text", "REAPER")]), "覚えていない種類の手（打つ）を聞かずに送る");
        assert!(!core.skips_approval(&[step("desktop.ax.click", "Notes")]), "別の app の手を聞かずに送る");
        assert!(!core.skips_approval(&[]), "手の無い承認を聞かずに送る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENGINE-AUDIT W7 本体 1（operator 11-03 の決め 3）: Always allow は消さずに狭める = 消す・送る・買う系の名前の手（`gui::asks`）は Always allow が在っても毎回 card。
    /// 押す手は飛ばさない・menu の道と打つ欄の名前が当たれば飛ばさない・当たらない menu / 打つ手は今まで通り飛ばす。bridge は要らない（file を読むだけ）。
    /// 負の対照: skips_approval の CLICK_ACTION の門を外す → 1 つ目が赤・menu の道を gui::asks に渡さない → 「Delete」の assert が赤。
    #[test]
    fn w7_always_allow_never_skips_a_step_that_deletes_sends_or_buys() {
        use crate::generated::PageEngine;
        use crate::ui::engine_view::ApprovalStep;
        let dir = tmp("w7-always");
        let consents = dir.join(crate::surface::ACT_CONSENT_FILE);
        for action in [crate::capability_engine::CLICK_ACTION, "desktop.ax.menu", "desktop.ax.type_text"] {
            crate::surface::always_allow(&consents, "com.example.mixer", "Mixer", action, 1.0).unwrap();
        }
        let core = CoreEngine { registry: Arc::new(AdapterRegistry::default), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: consents };
        let step = |action: &str, input: Value| ApprovalStep { action: action.into(), input, digest: "d".into(), scopes: vec![] };
        let menu = |path: &[&str]| step("desktop.ax.menu", json!({ "pid": 1, "app_name": "Mixer", "path": path }));
        let typed = |label: &str| step("desktop.ax.type_text", json!({ "pid": 1, "app_name": "Mixer", "label": label, "text": "x" }));
        assert!(!core.skips_approval(&[step(crate::capability_engine::CLICK_ACTION, json!({ "pid": 1, "app_name": "Mixer", "label": "Delete Track" }))]), "消す押す手を飛ばした");
        assert!(!core.skips_approval(&[menu(&["Edit", "Delete"])]), "menu の Delete を飛ばした");
        assert!(!core.skips_approval(&[typed("Send to")]), "送る欄に打つ手を飛ばした");
        assert!(core.skips_approval(&[menu(&["Track", "New Track"]), typed("Track Name")]), "当たらない menu と打つ手を W7 の前の通りに飛ばさない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G70（27 手の #20・r7）: 承認の同意の宛先は起動中の app（bridge の `apps`）で決め、窓の読み（AX）を待たない。
    /// ① r7: 承認の直後に TextEdit が窓の一覧に答えない（`window_list` が Rust の上限 5 秒を越える = r7 の grant は Timeout で skipped・
    /// patient の読み直しは無い）間も、input の bundle の起動中の app が承認の 1 行の app 名なら書く（r7 は書かずに act が needs_consent）。
    /// ② G31b: 起こし直した app（保存した手順の pid 222 → 今は 333・document の無い menu の手）も bundle で書く。③ 同じ bundle でも名前が違えば書かない。
    /// 負の対照（手で取る）: grant を窓の読み（`desktop.windows.list` + `gui::retarget`）に戻すと ①② が書かない。
    #[cfg(unix)]
    #[test]
    fn g70_grant_reads_the_running_app_not_its_windows() {
        use crate::adapters::mac_gui::MacGui;
        use crate::generated::EngineRequest;
        use std::os::unix::fs::PermissionsExt;
        let grant = |name: &str, knob: &str, to: Value| {
            let dir = tmp(name);
            let (consents, sh) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join("bridge.sh"));
            std::fs::write(&sh, format!("#!/bin/sh\nexport {knob}\nexec python3 '{}/tests/fake-surface-bridge.py'\n", env!("CARGO_MANIFEST_DIR"))).unwrap();
            std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
            let s = sh.clone();
            let core = CoreEngine {
                registry: Arc::new(move || {
                    let mut r = AdapterRegistry::default();
                    r.register(Box::new(MacGui::at(s.clone()))).unwrap();
                    r
                }),
                data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts::default(), readback: Duration::ZERO, act_consents: consents.clone(),
            };
            let req = EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "g".into(), consents: vec![], grants: vec![to], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: Arc::new(|_| {}), runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
            let mut reg = (core.registry)();
            reg.check();
            core.grant(&mut reg, &req);
            let got = crate::surface::act_consents(&consents);
            let _ = std::fs::remove_dir_all(&dir);
            got
        };
        let te = "FAKE_APP_BUNDLE=com.apple.TextEdit FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_APP_PID=78301 FAKE_WL_SLOW=6";
        let readme = |app: &str| json!({ "pid": 78301, "app_name": app, "bundle_id": "com.apple.TextEdit", "document": "README.md" });
        assert!(grant("g70-r7", te, readme("TextEdit")).contains("com.apple.TextEdit"), "① 窓の一覧が答えない間に承認の同意を書かない（r7 の #20）");
        let to = json!({ "pid": 222, "app_name": "REAPER", "bundle_id": "com.cockos.reaper" });
        assert!(grant("g70-g31b", "FAKE_APP_PID=333", to).contains("com.cockos.reaper"), "② 起こし直した app に承認の同意を書かない（G31b）");
        assert!(grant("g70-name", te, readme("Notes")).is_empty(), "③ 見せた app（Notes）と違う名前の app（TextEdit）に同意を書いた");
    }

    /// M2（G-a・G-b）: REAPER の Page で "Add a new track"（一言は app を名指さない = model の target は null）→ 生成の model に Page の app と
    /// その窓が届く（G-a）→ 承認 → `ax_menu` Track › Insert new track → 窓の要素を読み戻し、`/controls` が本当に変わった時だけ「済み」（G-b）。
    /// 偽の bridge（tests/fake-surface-bridge.py・REAPER・pid 222・窓 300）は読むたびに要素の ref（`e<snapshot>.<n>`）を変える。
    /// 負の対照: 押しても木が変わらない bridge（FAKE_MENU_ADDS 無し）は「済み」にしない（G-b の前は ref の違いだけで Verified）・
    /// app の面の無い Page（Dev）は宛先 null で窓も渡さない・model が宛先を決めた操作は Page の app で上書きしない。
    /// G-i（本物の REAPER の 1 回目 = 評価版の About を model の役が選んだ）: 生成の model に渡る窓は Adapter が窓の役割で主窓 1 つに絞った物
    /// （About = main だが動かせない・主窓 = 動かせる）。同じ役割の窓が 2 つで決められない時は model を呼ばず Adapter の 1 行で止める（何も送らない）。
    #[cfg(unix)]
    #[test]
    fn m2_page_app_is_the_target_and_only_a_real_change_is_done() {
        let menu = json!({ "pid": 222, "app_name": "REAPER", "path": ["Track", "Insert new track"] });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("add", "desktop.ax.menu", menu)],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls", "changed", Value::Null)],
            "recipe": null, "trial": null,
        });
        // 一言を送って承認の 1 行まで（`knob` = 偽の bridge の env に足す物・`reaper` = REAPER の Mirror の Page を足して移ってから・`target` = Goal Interpretation の
        // model の答えの target）。返すのは生成の model が受け取った input（生成の前に止まった = None）
        let ask = |name: &str, knob: &str, reaper: bool, target: Value| {
            let env = format!("FAKE_MENU='Track>Insert new track' FAKE_TITLES='untitled project - REAPER v7.81'{knob}");
            let (g, project, req, acts, dir) = m2_ask(name, &env, reaper.then_some((REAPER, NO_FILE)), ADD, &gen, target);
            (g, project, req.map(|r| serde_json::from_str::<Value>(&r.input).unwrap()), acts, dir)
        };
        let adds = " FAKE_MENU_ADDS=1";
        // G-a: 宛先は Page の app・その窓だけが model に届く
        let (mut g, project, input, acts, dir) = ask("m2-add", adds, true, Value::Null);
        let input = input.expect("生成の model を呼んだ");
        assert_eq!(input["operation"]["target"], json!(REAPER));
        assert!(input["windows"].as_array().is_some_and(|w| !w.is_empty() && w.iter().all(|w| w["bundle_id"] == REAPER)), "{}", input["windows"]);
        // G-b: 承認 → menu を 1 回押す → track が 1 本増えた読みで「済み」
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(m2_sent(&acts), 1);
        let _ = std::fs::remove_dir_all(&dir);
        // 負の対照 G-b: 押しても木が変わらない（読みの ref だけが変わる）= 「済み」にしない
        let (mut g, project, _, acts, dir) = ask("m2-noop", "", true, Value::Null);
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(!matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "変わっていないのに済み: {:?}", g.engine_view(&project));
        assert_eq!(m2_sent(&acts), 1, "押したが、確かめられない");
        let _ = std::fs::remove_dir_all(&dir);
        // 負の対照 G-a: app の面の無い Page（Dev）は宛先 null・窓を渡さない／model が決めた宛先は替えない
        let (_, _, input, _, dir) = ask("m2-dev", adds, false, Value::Null);
        let input = input.expect("生成の model を呼んだ");
        assert_eq!((&input["operation"]["target"], input.get("windows")), (&Value::Null, None));
        let _ = std::fs::remove_dir_all(&dir);
        let (_, _, input, _, dir) = ask("m2-named", adds, true, json!("com.apple.TextEdit"));
        assert_eq!(input.expect("生成の model を呼んだ")["operation"]["target"], json!("com.apple.TextEdit"), "model が決めた宛先");
        let _ = std::fs::remove_dir_all(&dir);
        // G-i: 評価版の About（main・動かせない）と主窓（main でない・動かせる）= 生成の model には主窓だけ（model の題名の判断に頼らない）
        let about = r#" FAKE_WL='[[{"title":"About REAPER v7.81","main":true,"movable":false},{"title":"REAPER v7.81 - EVALUATION LICENSE","main":false}]]'"#;
        let (_, _, input, _, dir) = ask("m2-about", about, true, Value::Null);
        let titles = |v: &Value| v["windows"].as_array().map(|w| w.iter().map(|w| w["title"].clone()).collect::<Vec<_>>());
        assert_eq!(titles(&input.clone().expect("生成の model を呼んだ")), Some(vec![json!("REAPER v7.81 - EVALUATION LICENSE")]), "{input:?}");
        let _ = std::fs::remove_dir_all(&dir);
        // G-i の止まり: 同じ役割の窓が 2 つ（どちらも動かせて main でない）= 推測で手順を作らず Adapter の 1 行で止める（生成の model を呼ばない・何も送らない）
        let two = r#" FAKE_WL='[[{"title":"a - REAPER v7.81","main":false},{"title":"b - REAPER v7.81","main":false}]]'"#;
        let (g, project, input, acts, dir) = ask("m2-two", two, true, Value::Null);
        assert!(input.is_none(), "決められない窓の 1 つで手順を作った: {:?}", input.as_ref().and_then(titles));
        let view = format!("{:?}", g.engine_view(&project));
        assert!(view.contains("2 windows of REAPER are open and Shibubu can't tell which"), "{view}");
        // G73: 承認の button の無い拒否の行は ▣ Needs approval にしない（UI-6a・理由の文だけの Ready）
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Ready, .. })), "{view}");
        assert_eq!(m2_sent(&acts), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M2b（G-c・G-d・G-e）: Discord の Page で "Open the general channel" → 生成の model は窓の中身を受け取らず（channel 名を CLI に出さない）、
    /// click の label に利用者の言葉・match = contains（G-c）と、窓の題名の contains の読み戻し（G-d）を選べる（指示と schema に在る）→
    /// 承認の 1 行は「Press “general” in Discord」・1 step（G-e）→ 承認 → label に general を含む押せる要素がちょうど 1 つなので押す →
    /// 窓の題名が #general になった読みで「済み」。偽の bridge = Discord（pid 222・窓 300）の窓に channel の行（AXLink「<名前> (text channel)」）。
    /// 負の対照: general を含む行が 2 つ・0 個 = 押さずに探す段（ENGINE-AUDIT W5: 窓を読み直し、AI に次の 1 手を聞く = 偽は手 0）→「2 things … have that name」/「isn't in the window」の
    /// 1 行に何が見えたかと次の一手（AI の none_why）・題名が空（画面収録の許可が無い）= 押したが「済み」にしない。
    #[cfg(unix)]
    #[test]
    fn m2b_discord_presses_the_one_channel_that_matches_and_reads_the_title() {
        let click = json!({ "pid": 222, "window_id": 300, "app_name": "Discord", "label": "general", "role": "", "match": "contains" });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("open", "desktop.ax.click", click)],
            "checks": [check("desktop.windows.list", json!({ "bundle_id": DISCORD, "main": true }), "/0/title", "contains", json!("general"))],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_ROW_TITLE='#{{}} | Test Server'");
        let ask = |name: &str, rows: &str, title: &str| m2_ask(name, &format!("{base} FAKE_AX_ROWS='{rows}' FAKE_WL='[[{{\"title\":\"{title}\"}}]]'"), Some((DISCORD, NO_FILE)), OPEN, &gen, Value::Null);
        let (mut g, project, req, acts, dir) = ask("m2b-open", "general|random", "Test Server");
        let req = req.expect("生成の model を呼んだ");
        let input: Value = serde_json::from_str(&req.input).unwrap();
        assert_eq!(input["operation"]["target"], json!(DISCORD), "G-a: Page の app");
        assert!(!req.input.contains("random") && !req.input.contains("text channel"), "窓の中身（channel 名）を model に渡さない: {}", req.input);
        let click_schema = input["registry_actions"].as_array().unwrap().iter().find(|a| a["id"] == "desktop.ax.click").map(|a| a["input_schema"].to_string()).unwrap_or_default();
        assert!(click_schema.contains("contains"), "G-c: click の schema に match = contains: {click_schema}");
        assert!(req.instructions.contains("match = \"contains\"") && req.instructions.contains("/0/title"), "G-c・G-d: model が選べる様に指示に在る");
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("{:?}", g.engine_view(&project)) };
        assert_eq!(crate::ui::engine_view::approval_line(&goal, &steps), "To do “Open the general channel”, Shibubu will send 1 step: Press “general” in Discord (approving sends exactly this, once, and hands Discord to the AI)");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(m2_sent(&acts), 1);
        let _ = std::fs::remove_dir_all(&dir);
        // 負の対照: 押せない（2 個・0 個）は何も押さず探す段 → AI が手 0（none）→ 1 行に何を探し・何が見えたか・次の一手・題名が読めないと「済み」にしない
        let none = json!({ "none_why": "Open the channel list in the server first", "workflow_steps": [], "checks": [], "recipe": null, "trial": null });
        for (name, rows, title, want) in [
            ("m2b-two", "general|general-2", "Test Server", Some("2 things in the window have that name")),
            ("m2b-none", "random", "Test Server", Some("isn't in the window and nothing on the screen leads to it")),
            ("m2b-untitled", "general", "", None),
        ] {
            let (mut g, project, _, acts, dir) = m2_ask_gens(name, &format!("{base} FAKE_AX_ROWS='{rows}' FAKE_WL='[[{{\"title\":\"{title}\"}}]]'"), Some((DISCORD, NO_FILE)), OPEN, std::slice::from_ref(&gen), Value::Null, None, std::slice::from_ref(&none));
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let view = format!("{:?}", g.engine_view(&project));
            assert!(!matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{name}: 済みにした: {view}");
            match want {
                Some(line) => {
                    assert!(view.contains(line), "{name}: {view}");
                    assert!(view.contains("The window shows") && view.contains("Open the channel list"), "{name}: W5 = 何が見えたか・次の一手: {view}");
                    assert_eq!(m2_sent(&acts), 0, "{name}: 押した");
                }
                None => {
                    assert_eq!(m2_sent(&acts), 1, "{name}: 押したが題名が読めない");
                    assert!(view.contains("isn't marked done"), "{name}: G-d は Verification の 1 行: {view}");
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G73（27 手の #20・r14）: TextEdit の窓が README.md・notes.md・一時の小窓（題名なし）の 3 つで、Page の面の書類が README.md → 生成の前の宛先の窓の読みは
    /// 窓の役割でなく書類で決める = 生成の model に README の窓だけが届く（r14: 役割で決められず「3 windows of TextEdit … can't tell which」で Denied・
    /// その行に承認の button の無い ▣ Needs approval）。main が notes の時も README（書類が先・役割で選ぶと別の書類の窓を model に渡す）。
    /// 書類の窓が 2 つ（2 つの instance で同じ README）なら役割で選ばず、書類の 1 行で止める（model を呼ばない・何も送らない・拒否の行は Ready = 理由の文だけ）。
    /// 書類の無い Page（scene 2 の REAPER・Discord の席 = file: None）は今まで通り役割（m2・m2b の試験が持つ）。書類の窓が 0 個で止まるのは G75 の試験。
    /// 負の対照（手で取る）: Adapter の書類で決める枝を外す → tie は Denied・notes-main は model に notes の窓・twins は main の窓で手順を作る。
    #[cfg(unix)]
    #[test]
    fn g73_the_page_document_picks_the_window_before_the_role() {
        let set = json!({ "pid": 222, "window_id": 300, "app_name": "TextEdit", "bundle_id": TEXTEDIT, "document": "README.md", "label": "", "role": "AXTextArea", "text": "Shibubu Demo" });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("set", "desktop.ax.type_text", set)],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls", "changed", Value::Null)],
            "recipe": null, "trial": null,
        });
        let ask = |name: &str, second: &str, notes_main: bool| {
            let wl = format!(r#"[[{{"title":"README.md","main":false}},{{"title":"{second}","main":{notes_main}}},{{"title":"","main":false}}]]"#);
            let (g, project, req, acts, dir) = m2_ask(name, &format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_WL='{wl}'"), Some((TEXTEDIT, "README.md")), RETITLE, &gen, Value::Null);
            (g, project, req.map(|r| serde_json::from_str::<Value>(&r.input).unwrap()), acts, dir)
        };
        let titles = |v: &Value| v["windows"].as_array().map(|w| w.iter().map(|w| w["title"].clone()).collect::<Vec<_>>());
        for (name, notes_main) in [("g73-tie", false), ("g73-notes-main", true)] {
            let (_, _, input, acts, dir) = ask(name, "notes.md", notes_main);
            let input = input.unwrap_or_else(|| panic!("{name}: 生成の model を呼ばずに止めた"));
            assert_eq!(input["operation"]["target"], json!(TEXTEDIT), "{name}: G-a の Page の app");
            assert_eq!(titles(&input), Some(vec![json!("README.md")]), "{name}: 書類の窓だけ");
            assert_eq!(m2_sent(&acts), 0, "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        let (g, project, input, acts, dir) = ask("g73-twins", "README.md — Edited", true);
        assert!(input.is_none(), "同じ書類の窓 2 つから役割で選んで手順を作った: {:?}", input.as_ref().and_then(titles));
        let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("拒否の 1 行: {:?}", g.engine_view(&project)) };
        assert!(line.contains("2 windows of TextEdit have “README.md” open"), "書類の 1 行: {line}");
        assert_eq!(state, RegionState::Ready, "承認の button の無い拒否の行に ▣ Needs approval（UI-6a）: {line}");
        assert_eq!(m2_sent(&acts), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// act の記録のうち送った手の種類（set = 欄に置いた・key = key・other = 押す・menu）
    fn g74_kinds(acts: &std::path::Path) -> Vec<&'static str> {
        use crate::gui::GuiAction;
        let log = crate::gui::ActLog::read(acts).unwrap_or_default();
        log.iter()
            .filter(|r| r.not_sent.is_none())
            .map(|r| match &r.receipt.action {
                GuiAction::SetValue { .. } => "set",
                GuiAction::Key { .. } => "key",
                _ => "other",
            })
            .collect()
    }

    /// G74（owner 12:2x・P0・③-8 = 1117961: Discord の入力欄に在った書きかけ 6 字が、置き換えられないまま return で投稿された）:
    /// Discord の Page で「この channel に試験の文を送って」→ 承認 1 回 → type_text（入力欄）→ key return の手順。偽の bridge の欄は
    /// FAKE_SET_LASTS（置いた道は「合う」と言うが、欄の中身は n 回の読みの後に元へ戻る・0 = 初めから変わらない = rich text の欄）。
    /// rich（0）: type_text の別の読みが違う → 送る直前の再確認でも違う → return を送らずに止まる・1 行は「送る手を打っていない」の文
    /// reverts（1）: type_text は別の読みで合った（Verified）が、送る直前の再確認で戻っている → 同じく return を送らない
    /// draft: 打つ前の欄に人の字 = 置かずに止まる（置く手も return も 0・Adapter の人の言葉の 1 行）
    /// sent: 欄が空で置けた → return を 1 回 → 読み戻し（投稿されると欄が空になる）が合わない = 「送れたか確かめられない・送り直さない」の文・
    ///   もう一度承認を押しても return は 1 回のまま（自動で送り直さない）
    /// 負の対照（手で取る）: 送る直前の再確認を外す → rich・reverts で return を送る・下書きの止めを外す → draft で置いて return・
    ///   Held を Verification に写す → 1 行が「確かめられない」の文になる
    #[cfg(unix)]
    #[test]
    fn g74_the_send_step_runs_only_after_the_field_is_confirmed() {
        let typed = json!({ "pid": 222, "window_id": 300, "app_name": "Discord", "label": "本文", "role": "AXTextArea", "text": "shibubu-test-g74" });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("type", "desktop.ax.type_text", typed), step("send", "desktop.key", json!({ "pid": 222, "app_name": "Discord", "key": "return" }))],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls/2/value", "equals", json!(""))],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord'");
        for (name, env, want, sent) in [
            ("g74-rich", "FAKE_SET_LASTS=0", "so Shibubu didn't do the send step", vec!["set"]),
            ("g74-reverts", "FAKE_SET_LASTS=1", "so Shibubu didn't do the send step", vec!["set"]),
            ("g74-draft", "FAKE_AX_VALUE=書きかけ", "already has text", vec![]),
            ("g74-sent", "", "Couldn't confirm whether it was sent", vec!["set", "key"]),
        ] {
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), SEND, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            assert!(line.contains(want), "{name}: {line}");
            assert_ne!(state, RegionState::Verified, "{name}: 済みにした: {line}");
            assert!(!(line.contains("didn't run") && line.contains("whether it was sent")), "{name}: 2 つの文を混ぜた: {line}");
            assert_eq!(g74_kinds(&acts), sent, "{name}: 送った手");
            // もう一度承認を押しても、待っている答えが無い = 何も送り直さない（run が始まれば Running に変わる・poll_engine は走り出した直後も false なので見ない）
            g.engine_act(&project, EngineAct::Approve);
            assert!(matches!(g.engine_view(&project), Some(EngineView::Line { line: again, .. }) if *again == line), "{name}: 承認をもう一度押したら run が始まった: {:?}", g.engine_view(&project));
            assert_eq!(g74_kinds(&acts), sent, "{name}: 送り直した");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G74 ①（27 手の #20 を止めない）: TextEdit の Page（書類 README.md）で、人の字が在る README の本文を「題を Shibubu Demo に」→ model の手は
    /// 窓の pid・window_id だけ（本物の model の形・bundle_id と document を写さない）→ 生成が Page の書類の窓を指す手に bundle_id と document を足す
    /// → Adapter は書類で窓を決め直し、頼みの中身が書類そのものなので置く → 保存の menu の前の再確認も合う → 済み。
    /// Discord の Page（書類の題名の窓が無い）は足さない = g74 の draft が止まる。負の対照（手で取る）: 足す所を外す → 書きかけで止まる
    #[cfg(unix)]
    #[test]
    fn g74_the_page_document_may_be_replaced() {
        let set = json!({ "pid": 222, "window_id": 300, "app_name": "TextEdit", "label": "本文", "role": "AXTextArea", "text": "Shibubu Demo" });
        let gen = json!({
            "none_why": null, "workflow_steps": [step("set", "desktop.ax.type_text", set), step("save", "desktop.ax.menu", json!({ "pid": 222, "app_name": "TextEdit", "path": ["ファイル", "保存"] }))],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls/2/value", "equals", json!("Shibubu Demo"))],
            "recipe": null, "trial": null,
        });
        let env = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_AX_VALUE=人の書きかけ");
        let (mut g, project, _, acts, dir) = m2_ask("g74-doc", &env, Some((TEXTEDIT, "README.md")), RETITLE, &gen, Value::Null);
        let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("{:?}", g.engine_view(&project)) };
        assert_eq!((steps[0].input["document"].clone(), steps[0].input["bundle_id"].clone()), (json!("README.md"), json!(TEXTEDIT)), "書類の窓を指す手に書類を足す");
        assert_eq!(steps[1].input["document"], json!("README.md"), "G75: 保存の menu にも足す = 書類の窓が主窓の時だけ送る");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(g74_kinds(&acts), ["set", "other"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G74b（owner 12:2x「削除が shibubu からできるかテスト」）: Discord の Page で「誤って投稿した message を消して」→ 承認 1 回 → 行の context menu
    /// （press = AXShowMenu）→ Delete Message → 確認の dialog の Delete（only_if = 消す本文）→ 読み戻し 2 つ（その本文の行が無い・行が 1 つ減った = fewer）で済み。
    /// 偽の bridge の FAKE_MESSAGES（本物の Discord の role と label は未測）。違う場面（FAKE_MESSAGES_OFF=1 = app が別の行の menu を開いた）: dialog は
    /// 別の本文を見せる → Delete を押さない（押した手は 2 つ）・済みにしない。負の対照（手で取る）: only_if を窓全体で探す → 違う場面で 3 つ押して別の行を消す
    #[cfg(unix)]
    #[test]
    fn g74b_the_page_deletes_only_the_message_the_dialog_shows() {
        let at = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "app_name": "Discord" });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let rows = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "subrole": "AXDocumentArticle" });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let gen = json!({
            "none_why": null, "workflow_steps": [
                step("menu", "desktop.ax.click", at(json!({ "label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu" }))),
                step("item", "desktop.ax.click", at(json!({ "label": "Delete Message", "role": "AXMenuItem", "match": "contains" }))),
                step("delete", "desktop.ax.click", at(json!({ "label": "Delete", "role": "AXButton", "only_if": ["typo-six"] }))),
            ],
            "checks": [
                check("desktop.window.state", rows(json!({ "contains": "typo-six" })), "/controls", "equals", json!([])),
                check("desktop.window.state", rows(json!({})), "/controls", "fewer", json!(1)),
            ],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord' FAKE_MESSAGES='hello-row|typo-six|later-row'");
        for (name, env, done, pressed) in [("g74b-match", "", true, 3), ("g74b-other", "FAKE_MESSAGES_OFF=1", false, 2)] {
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), DELETE, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            assert_eq!(state == RegionState::Verified, done, "{name}: {line}");
            if done {
                assert!(line.contains("Checked 2 items"), "{name}: {line}");
            }
            assert_eq!(g74_kinds(&acts).len(), pressed, "{name}: 押した手");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G74c（owner 14:4x「文字そのものを長押しすると出てくる…ボタンなどではない」・Discord 15:0x の実測の形 = menu は AXGroup ＋ AXMenuItem・数百 ms 遅れて
    /// 描かれ、出ている間は行が木から消える）: Discord の Page で「誤って投稿した message を消して」→ 承認 1 回。
    /// late: menu が 2 回の読みの間まだ無い → 2 手目（role = AXMenuItem）が待って Delete Message → dialog の Delete → 済み（3 つ押す）
    /// missing: menu に Delete Message が無く、AXCancel でも閉じない → 2 手目は何も押さない。Page の 1 行は「Couldn't check the result…」でなく、
    ///   何が見えて止めたか（menu に無い・開いたまま・Esc）＋ その手と後ろは走っていない ＋ 前の 1 手だけ走った（送ったのは AXShowMenu と AXCancel の 2 つ）
    /// lingering: model の手が menu を開く 1 手だけ・確かめは「その本文の行が無い」だけ → menu の間は行が木から消えるが、消えたと読まない（済みにしない）
    /// 負の対照（G74C_MUT）: nowait = 読み直さない → late が止まる / line = 3 つ目の文を Verification の定型文に戻す → missing が「Couldn't check」/
    /// state = menu の間も window.state を読む → lingering が済み
    #[cfg(unix)]
    #[test]
    fn g74c_the_page_says_what_ran_when_it_stops_at_the_menu() {
        let at = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "app_name": "Discord" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let rows = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "subrole": "AXDocumentArticle" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let menu = step("menu", "desktop.ax.click", at(json!({ "label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu" })));
        let item = step("item", "desktop.ax.click", at(json!({ "label": "Delete Message", "role": "AXMenuItem", "match": "contains" })));
        let delete = step("delete", "desktop.ax.click", at(json!({ "label": "Delete", "role": "AXButton", "only_if": ["typo-six"] })));
        let gone = check("desktop.window.state", rows(json!({ "contains": "typo-six" })), "/controls", "equals", json!([]));
        let fewer = check("desktop.window.state", rows(json!({})), "/controls", "fewer", json!(1));
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord' FAKE_MESSAGES='hello-row|typo-six|later-row'");
        let three = vec![menu.clone(), item, delete];
        for (name, env, steps, checks, pressed) in [
            ("g74c-late", "FAKE_MENU_LATE=2", three.clone(), vec![gone.clone(), fewer], 3),
            ("g74c-missing", "FAKE_MENU_ITEMS='Reply|Copy Text' FAKE_MENU_STICKY=1", three, vec![gone.clone()], 2),
            ("g74c-lingering", "", vec![menu], vec![gone], 1),
        ] {
            let gen = json!({ "none_why": null, "workflow_steps": steps, "checks": checks, "recipe": null, "trial": null });
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), DELETE, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            match name {
                "g74c-late" => assert!(state == RegionState::Verified && line.contains("Checked 2 items"), "{name}: 2 手目が menu を待たなかった: {line}"),
                "g74c-missing" => assert!(
                    state != RegionState::Verified && line.contains("“Delete Message”") && line.contains("still open") && line.contains("Esc") && line.contains("didn't run") && line.contains("Only the first step ran") && !line.contains("Couldn't check"),
                    "{name}: 3 つ目の文でない: {line}"
                ),
                _ => assert!(state != RegionState::Verified, "{name}: menu の間の行 0 を消えたと読んだ: {line}"),
            }
            assert_eq!(g74_kinds(&acts).len(), pressed, "{name}: 送った手");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G74e（本物 17:10 の 3 回目: Delete Message を押したが Discord は確認の dialog を出さず、Page は「Couldn't check the result…」= 送っていない 3 手目と
    /// 確かめられないが混ざった）: dead = 項目を押しても何も起きない → 3 手目は何も押さず止まる。Page の 1 行は 3 つ目の文（確認が出なかった・“Delete” は
    /// 押していない・menu が開いたまま Esc ＋ 前の 2 手だけ走った）・送った手 2。late = dialog が 2 回の読みの間まだ無い → 待って押す → 済み（3 つ押す）
    /// 負の対照（G74E_MUT）: arm → dead が「Couldn't check」/ dialogwait → late が止まる
    #[cfg(unix)]
    #[test]
    fn g74e_the_page_says_no_confirmation_appeared() {
        let at = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "app_name": "Discord" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let rows = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "subrole": "AXDocumentArticle" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let gen = json!({
            "none_why": null, "workflow_steps": [
                step("menu", "desktop.ax.click", at(json!({ "label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu" }))),
                step("item", "desktop.ax.click", at(json!({ "label": "Delete Message", "role": "AXMenuItem", "match": "contains" }))),
                step("delete", "desktop.ax.click", at(json!({ "label": "Delete", "role": "AXButton", "only_if": ["typo-six"] }))),
            ],
            "checks": [
                check("desktop.window.state", rows(json!({ "contains": "typo-six" })), "/controls", "equals", json!([])),
                check("desktop.window.state", rows(json!({})), "/controls", "fewer", json!(1)),
            ],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord' FAKE_MESSAGES='hello-row|typo-six|later-row'");
        for (name, env, pressed) in [("g74e-dead", "FAKE_ITEM_DEAD=1", 2), ("g74e-late", "FAKE_DIALOG_LATE=2", 3)] {
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), DELETE, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            match name {
                "g74e-dead" => assert!(
                    state != RegionState::Verified && line.contains("No confirmation") && line.contains("“Delete” wasn't pressed") && line.contains("Esc") && line.contains("Only the first 2 steps ran") && !line.contains("Couldn't check"),
                    "{name}: 3 つ目の文でない: {line}"
                ),
                _ => assert!(state == RegionState::Verified && line.contains("Checked 2 items"), "{name}: dialog を待たなかった: {line}"),
            }
            assert_eq!(g74_kinds(&acts).len(), pressed, "{name}: 送った手");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G74f（本物 18:37 = ⌘H で隠れた Discord への AXShowMenu は rc 0 で答えたが menu は出なかった・18:45 = 見せた後に「Delete Message」へ AXFocused → AXPress で
    /// 確認の dialog が出た）: hidden = 隠れた app には承認の後も何も送らない（どの app でも・勝手に見せない・前に出さない）→ Page は「Discord is hidden, so nothing
    /// was sent. Show its window …」・送った手 0。focus = menu の項目は focus を付けて押す → focus が要る app でも dialog の Delete まで通る → 済み（3 つ押す）
    /// 負の対照（G74F_MUT）: hidden → AXShowMenu が送られる / focus → 項目が効かず「No confirmation」
    /// ENGINE-AUDIT W8a（spec 262・r8b）: 隠れた app は送る前に見せる（app_unhide・前面は取らない）= g74f-unhide は見せてから 3 つ押して済み。
    /// 見せられない（FAKE_UNHIDE_STUCK）時だけ今まで通り「Discord is hidden…」で 0 手（隠れたままの app への AXShowMenu は届いても効かない）。
    /// 負の対照（本体の後・手で）: invoke の shown_for_act を外す → g74f-unhide が「Discord is hidden」で赤
    #[cfg(unix)]
    #[test]
    fn g74f_a_hidden_app_gets_nothing_and_a_menu_item_is_focused_first() {
        let at = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "app_name": "Discord" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let rows = |extra: Value| {
            let mut v = json!({ "pid": 222, "window_id": 300, "subrole": "AXDocumentArticle" });
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let gen = json!({
            "none_why": null, "workflow_steps": [
                step("menu", "desktop.ax.click", at(json!({ "label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu" }))),
                step("item", "desktop.ax.click", at(json!({ "label": "Delete Message", "role": "AXMenuItem", "match": "contains" }))),
                step("delete", "desktop.ax.click", at(json!({ "label": "Delete", "role": "AXButton", "only_if": ["typo-six"] }))),
            ],
            "checks": [
                check("desktop.window.state", rows(json!({ "contains": "typo-six" })), "/controls", "equals", json!([])),
                check("desktop.window.state", rows(json!({})), "/controls", "fewer", json!(1)),
            ],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord' FAKE_MESSAGES='hello-row|typo-six|later-row'");
        for (name, env, pressed) in [("g74f-hidden", "FAKE_HIDDEN=1 FAKE_UNHIDE_STUCK=1", 0), ("g74f-unhide", "FAKE_HIDDEN=1 FAKE_ITEM_FOCUS=1", 3), ("g74f-focus", "FAKE_ITEM_FOCUS=1", 3)] {
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), DELETE, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            match name {
                "g74f-hidden" => assert!(
                    state != RegionState::Verified && line.starts_with("Discord is hidden, so nothing was sent. Show its window (it can stay behind other windows), then ask again") && !line.contains("Couldn't check"),
                    "{name}: 隠れた app の文でない: {line}"
                ),
                _ => assert!(state == RegionState::Verified && line.contains("Checked 2 items"), "{name}: 項目に focus を付けずに押した: {line}"),
            }
            assert_eq!(g74_kinds(&acts).len(), pressed, "{name}: 送った手");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G74d（G74c の後続・owner の「送る手を打っていない / 確かめられない」を混ぜない）: 前の run が Discord の menu を開いたまま残した（FAKE_MENU_AT）所で
    /// 「誤って投稿した message を消して」→ 手を走らせる前の読み（その本文の行）が menu に隠されて読めない → 承認を出さず、何も送らずに止める。
    /// Page の 1 行は「Couldn't check the result…」でなく、何も送っていない ＋ Adapter の 1 行（menu が開いている・閉じてからもう一度）・送った手 0。
    /// 負の対照（G74D_MUT=denied）: 前の読みの失敗を今までの Verification に戻す → 「Couldn't check」
    #[cfg(unix)]
    #[test]
    fn g74d_a_read_before_any_step_that_fails_says_nothing_was_sent() {
        let at = json!({ "pid": 222, "window_id": 300, "app_name": "Discord" });
        let rows = json!({ "pid": 222, "window_id": 300, "subrole": "AXDocumentArticle", "contains": "typo-six" });
        let mut menu = at.clone();
        menu.as_object_mut().unwrap().extend(json!({ "label": "typo-six", "role": "", "match": "contains", "press": "AXShowMenu" }).as_object().unwrap().clone());
        let gen = json!({
            "none_why": null, "workflow_steps": [step("menu", "desktop.ax.click", menu)],
            "checks": [check("desktop.window.state", rows, "/controls", "equals", json!([]))],
            "recipe": null, "trial": null,
        });
        let env = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord' FAKE_MESSAGES='hello-row|typo-six|later-row' FAKE_MENU_AT=1");
        let (g, project, req, acts, dir) = m2_ask_any("g74d-leftover", &env, Some((DISCORD, NO_FILE)), DELETE, &gen, Value::Null);
        assert!(req.is_some(), "生成まで進まなかった");
        let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("承認を出した・前の読みで止めていない: {:?}", g.engine_view(&project)) };
        assert!(state != RegionState::Verified && line.contains("Nothing was sent") && line.contains("A menu is open in Discord") && !line.contains("Couldn't check"), "{line}");
        assert_eq!(g74_kinds(&acts).len(), 0, "送った手");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G75（owner 10-26 12:1x・P0 = 名指しの対象を代用しない・Target Resolution → Identity Verification → Execution → Result Verification）:
    /// TextEdit の Page（書類 README.md）で「README の題を Shibubu Demo にして保存して」。model の手 = 書類の窓に字を置く type_text と保存の menu・
    /// model の checks = 「窓の controls が変わった」だけ（demo #20 の git の changed と同じ弱さ）。
    /// open: README の窓 1 つ → 置いて保存 → 済み（確かめは model の 1 つ + 書類の中身の 1 つ = 2）
    /// closed: notes.md だけ → model を呼ばず「“README.md” isn't open in TextEdit・Nothing was changed」で止まる（notes に何もしない）
    /// twins: README が 2 つ → model を呼ばず区別できない 1 行
    /// behind: notes.md が主窓・README は後ろ → model に README の窓だけ・README に置く・保存の menu は README が主窓でないので送らない（notes を保存しない）
    /// moved: 生成の後に窓の番号が入れ替わる（README 300 → 301）→ 置く手は今の README（301）へ（実行の直前の読み直し）
    /// faded: 置いた字が保存の後に消える（FAKE_SET_LASTS=2・戻し先は空）→ 「変わった」は通るが書類の中身に字が無い = 済みにしない
    /// elsewhere: model が README でない窓（notes 301）に置く手を作った → 承認の 1 行を出さず止める（送らない）
    /// 負の対照（G75_MUT）: role = 0 個で役割に戻す → closed が notes に / reread = 実行の直前の読み直しを外す → moved が 300 に /
    /// checks = 書類の中身の確かめを足さない → faded が済み / main = menu の主窓の確かめを外す → behind が notes を保存 / elsewhere = 別の窓の手を通す
    #[cfg(unix)]
    #[test]
    fn g75_the_named_document_is_never_replaced() {
        let gen = |window: u64, save: bool| {
            let set = json!({ "pid": 222, "window_id": window, "app_name": "TextEdit", "label": "本文", "role": "AXTextArea", "text": "Shibubu Demo" });
            let mut steps = vec![step("set", "desktop.ax.type_text", set)];
            if save {
                steps.push(step("save", "desktop.ax.menu", json!({ "pid": 222, "app_name": "TextEdit", "path": ["ファイル", "保存"] })));
            }
            json!({
                "none_why": null, "workflow_steps": steps,
                "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": window }), "/controls", "changed", Value::Null)],
                "recipe": null, "trial": null,
            })
        };
        let base = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_AX_VALUE=元の題");
        let windows = |t: &[(&str, bool)]| format!("[[{}]]", t.iter().map(|(title, main)| format!(r#"{{"title":"{title}","main":{main}}}"#)).collect::<Vec<_>>().join(","));
        let wins = |acts: &std::path::Path| crate::gui::ActLog::read(acts).unwrap_or_default().into_iter().filter(|r| r.not_sent.is_none()).map(|r| r.receipt.target.window_id).collect::<Vec<_>>();
        let line = |g: &Generated, project: &str| match g.engine_view(project) {
            Some(EngineView::Line { state, line }) => (state.clone(), line.clone()),
            other => panic!("{other:?}"),
        };
        let approve = |g: &mut Generated, project: &str| {
            g.engine_act(project, EngineAct::Approve);
            settle(g);
        };
        // open
        let (mut g, project, _, acts, dir) = m2_ask("g75-open", &format!("{base} FAKE_TITLES=README.md"), Some((TEXTEDIT, "README.md")), RETITLE, &gen(300, true), Value::Null);
        approve(&mut g, &project);
        let (state, opened) = line(&g, &project);
        assert_eq!(state, RegionState::Verified, "open: {opened}");
        assert_eq!(g74_kinds(&acts), ["set", "other"], "open");
        let _ = std::fs::remove_dir_all(&dir);
        // closed
        let (g, project, req, acts, dir) = m2_ask_any("g75-closed", &format!("{base} FAKE_WL='{}'", windows(&[("notes.md", true)])), Some((TEXTEDIT, "README.md")), RETITLE, &gen(300, true), Value::Null);
        assert!(req.is_none(), "closed: notes の窓で手順を作った: {:?}", req.map(|r| r.input));
        let (state, l) = line(&g, &project);
        assert!(l.contains("“README.md” isn't open in TextEdit") && l.contains("Nothing was changed"), "closed: {l}");
        assert_eq!(state, RegionState::Ready, "closed: 押せる答えの無い拒否の行: {l}");
        assert_eq!(m2_sent(&acts), 0, "closed");
        let _ = std::fs::remove_dir_all(&dir);
        // twins
        let (g, project, req, acts, dir) = m2_ask_any("g75-twins", &format!("{base} FAKE_WL='{}'", windows(&[("README.md", true), ("README.md — Edited", false)])), Some((TEXTEDIT, "README.md")), RETITLE, &gen(300, true), Value::Null);
        assert!(req.is_none(), "twins: どちらかで手順を作った");
        assert!(line(&g, &project).1.contains("can't tell which"), "twins: {:?}", line(&g, &project));
        assert_eq!(m2_sent(&acts), 0, "twins");
        let _ = std::fs::remove_dir_all(&dir);
        // behind
        let (mut g, project, req, acts, dir) = m2_ask("g75-behind", &format!("{base} FAKE_WL='{}'", windows(&[("notes.md", true), ("README.md", false)])), Some((TEXTEDIT, "README.md")), RETITLE, &gen(301, true), Value::Null);
        let input: Value = serde_json::from_str(&req.expect("behind: 生成まで進む").input).unwrap();
        assert_eq!(input["windows"].as_array().map(|w| w.iter().map(|w| w["title"].clone()).collect::<Vec<_>>()), Some(vec![json!("README.md")]), "behind: model に README の窓だけ");
        approve(&mut g, &project);
        assert_ne!(line(&g, &project).0, RegionState::Verified, "behind: {:?}", line(&g, &project));
        assert_eq!(g74_kinds(&acts), ["set"], "behind: README に置き、主窓が notes なので保存の menu は送らない");
        assert_eq!(wins(&acts), [Some(301)], "behind: 置いたのは README の窓");
        let _ = std::fs::remove_dir_all(&dir);
        // moved
        let wl = format!(r#"[[{{"title":"README.md","main":true}},{{"title":"notes.md","main":false}}],[{{"title":"notes.md","main":false}},{{"title":"README.md","main":true}}]]"#);
        let (mut g, project, _, acts, dir) = m2_ask("g75-moved", &format!("{base} FAKE_WL='{wl}'"), Some((TEXTEDIT, "README.md")), RETITLE, &gen(300, false), Value::Null);
        approve(&mut g, &project);
        assert_eq!(wins(&acts), [Some(301)], "moved: 生成の後に README は 301 へ = 置く直前に読み直して 301 に置く（300 は今は notes）");
        let _ = std::fs::remove_dir_all(&dir);
        // faded
        let (mut g, project, _, acts, dir) = m2_ask("g75-faded", &format!("{base} FAKE_TITLES=README.md FAKE_SET_LASTS=2 FAKE_SET_BACK="), Some((TEXTEDIT, "README.md")), RETITLE, &gen(300, true), Value::Null);
        approve(&mut g, &project);
        let (state, l) = line(&g, &project);
        assert_ne!(state, RegionState::Verified, "faded: 書類に字が無いのに済み: {l}");
        assert_eq!(g74_kinds(&acts), ["set", "other"], "faded: 置いて保存した（読み戻しで止まる）");
        let _ = std::fs::remove_dir_all(&dir);
        // open の確かめの数は faded の後に見る（checks の負の対照で faded の赤を先に見るため）
        assert!(opened.contains("Checked 2 items"), "open: 書類の中身の確かめを足した: {opened}");
        // elsewhere
        let (g, project, req, acts, dir) = m2_ask_any("g75-elsewhere", &format!("{base} FAKE_WL='{}'", windows(&[("README.md", true), ("notes.md", false)])), Some((TEXTEDIT, "README.md")), RETITLE, &gen(301, true), Value::Null);
        assert!(req.is_some(), "elsewhere: 生成まで進む");
        let (state, l) = line(&g, &project);
        assert!(l.contains("another TextEdit window") && l.contains("“README.md”"), "elsewhere: {l}");
        assert_eq!(state, RegionState::Ready, "elsewhere: {l}");
        assert_eq!(m2_sent(&acts), 0, "elsewhere");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G77（r19 #20・trace で決まった根）: 本物の model の答え（r19 の engine-trace.jsonl の plan 行の answer・起動ごとの pid 1987 と窓番号 3834 だけ
    /// 偽の bridge の 222 と 300 に替えた）の checks の 2 つ目は `desktop.windows.list {"bundle_id": TextEdit, "main": true}`（document 無し =
    /// GENERATE_INSTRUCTIONS の窓の題名の確かめの形）。TextEdit が 2 つ起動している（pid 222 = README.md と題名の無い小窓・223 = notes.md）と、書類で
    /// 決めない読みは役割で決められない（main が 2 つ）= 手を走らせる前の読みが「3 windows of TextEdit … can't tell which」で Denied（r19 の止まり方）。
    /// 生成が Page の書類の窓を指す checks にも document と bundle_id を足す → 前読みは 2 つの起動の窓から README の 1 つに決まり、承認の 1 行まで進む（何も送っていない）。
    /// 負の対照（G77_MUT）: checks = checks に足さない → 前読みで Denied「3 windows」/ bundle = pid の一致だけ（bundle_id だけの読みに足さない）→ 同じ
    /// G79: r19 の手 1・2・4（修飾 key 付きの desktop.key）は validate で落ちるようになったので、手だけ G79 の形（type_text の replace・File › Save）に替えた
    /// （G77 が見るのは checks の 2 つ目）
    #[cfg(unix)]
    #[test]
    fn g77_a_check_without_the_document_reads_the_page_document_window() {
        let r19 = r##"{"checks":[
            {"action":"desktop.window.state","expect":"changed","input_json":"{\"pid\":222,\"window_id\":300,\"app_name\":\"TextEdit\",\"bundle_id\":\"com.apple.TextEdit\",\"document\":\"README.md\",\"role\":\"AXTextArea\",\"contains\":\"# Shibubu Demo\"}","pointer":"/controls","value_json":null},
            {"action":"desktop.windows.list","expect":"contains","input_json":"{\"bundle_id\":\"com.apple.TextEdit\",\"main\":true}","pointer":"/0/title","value_json":"\"README.md\""}],
          "recipe":null,"trial":null,
          "none_why": null, "workflow_steps":[
            {"capability":"desktop.ax.type_text","id":"s3","input_json":"{\"pid\":222,\"window_id\":300,\"app_name\":\"TextEdit\",\"bundle_id\":\"com.apple.TextEdit\",\"document\":\"README.md\",\"label\":\"\",\"role\":\"AXTextArea\",\"replace\":\"元の題\",\"text\":\"# Shibubu Demo\"}"},
            {"capability":"desktop.ax.menu","id":"s4","input_json":"{\"pid\":222,\"app_name\":\"TextEdit\",\"bundle_id\":\"com.apple.TextEdit\",\"document\":\"README.md\",\"path\":[\"File\",\"Save\"]}"}]}"##;
        let gen: Value = serde_json::from_str(r19).unwrap();
        let wl = r#"[[{"title":"README.md","main":true,"pid":222},{"title":"","main":false,"pid":222},{"title":"notes.md","main":true,"pid":223}]]"#;
        let env = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_APP_MORE_PID=223 FAKE_AX_VALUE=元の題 FAKE_WL='{wl}'");
        let (g, project, req, acts, dir) = m2_ask_any("g77-r19", &env, Some((TEXTEDIT, "README.md")), RETITLE, &gen, Value::Null);
        let input: Value = serde_json::from_str(&req.expect("生成まで進む").input).unwrap();
        assert_eq!(input["windows"].as_array().map(|w| w.iter().map(|w| (w["pid"].clone(), w["title"].clone())).collect::<Vec<_>>()), Some(vec![(json!(222), json!("README.md"))]), "2 つの起動から書類の窓だけを model に");
        let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("前読みで止まった: {:?}", g.engine_view(&project)) };
        assert_eq!(steps.len(), 2, "置き換えと保存の 2 手");
        assert_eq!(m2_sent(&acts), 0, "承認の前に何も送らない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G79（r20 #20: 本物の計画 = cmd+↑・cmd+shift+→・全文の type_text・cmd+s = 承認すると README が 1 行に置き換わり、後ろの TextEdit に shortcut は
    /// 届かず保存もされない）: 1 回目の計画（r20 の形）は validate で落ち、計画だけ作り直す。2 回目（type_text の replace・File › Save）が承認の 1 行に →
    /// 承認 → bridge には題の行だけの置き換え（ax_replace_text）と menu だけ（全文の ax_set_value も key も送らない）→ 済み。
    /// 生成の model には書類の頭（偽の bridge の本文）と本文の欄の role・label が届く。
    /// 負の対照: G79_MUT=gate → 1 回目の 4 手が承認の 1 行に / G79_MUT=all → bridge に全文の ax_set_value
    /// G80: 偽の bridge の窓は本物の TextEdit と同じく本文の隣に AXScrollBar 2 つ（値を置ける数）を持つ（r22 の document_text null の形）。G80_MUT=role → 頭が渡らず赤
    #[cfg(unix)]
    #[test]
    fn g79_the_r20_plan_is_rebuilt_and_only_the_title_line_is_replaced() {
        let at = |extra: Value| {
            let mut i = json!({ "pid": 222, "app_name": "TextEdit" });
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            i
        };
        let key = |id: &str, k: &str, mods: &[&str]| step(id, "desktop.key", at(json!({ "key": k, "mods": mods })));
        let typed = |extra: Value| {
            let mut i = at(json!({ "window_id": 300, "label": "本文", "role": "AXTextArea", "text": "# Shibubu Demo" }));
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            step("s3", "desktop.ax.type_text", i)
        };
        let checks = json!([check("desktop.window.state", at(json!({ "window_id": 300, "role": "AXTextArea", "contains": "# Shibubu Demo" })), "/controls/0/value", "contains", json!("# Shibubu Demo"))]);
        let r20 = json!({ "none_why": null, "workflow_steps": [key("s1", "up", &["cmd"]), key("s2", "right", &["cmd", "shift"]), typed(json!({})), key("s4", "s", &["cmd"])], "checks": checks, "recipe": null, "trial": null });
        let fixed = json!({ "none_why": null, "workflow_steps": [typed(json!({ "replace": "# README" })), step("s4", "desktop.ax.menu", at(json!({ "path": ["File", "Save"] })))], "checks": checks, "recipe": null, "trial": null });
        let readme = "# README\n\nline two\nline three";
        let log = tmp("g79-log").join("bridge.log");
        let env = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_MENU='File>Save' FAKE_AX_SCROLLBARS=1 FAKE_LOG='{}' FAKE_AX_VALUE='{readme}'", log.display());
        let (mut g, project, reqs, acts, dir) = m2_ask_gens("g79-r20", &env, Some((TEXTEDIT, "README.md")), RETITLE, &[r20, fixed], Value::Null, None, &[]);
        assert_eq!(reqs.len(), 2, "計画だけ 1 回作り直す");
        let input: Value = serde_json::from_str(&reqs[0].input).unwrap();
        assert_eq!((&input["document_text"]["head"], &input["document_text"]["role"], &input["document_text"]["label"]), (&json!(readme), &json!("AXTextArea"), &json!("本文")), "{}", input["document_text"]);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("{:?}", g.engine_view(&project)) };
        assert_eq!(steps.iter().map(|s| (s.action.as_str(), s.input["replace"].as_str())).collect::<Vec<_>>(), [("desktop.ax.type_text", Some("# README")), ("desktop.ax.menu", None)]);
        let said = crate::ui::engine_view::approval_line(&goal, &steps);
        assert!(said.contains("will send 2 steps: In TextEdit's “README.md”, replace “# README” with “# Shibubu Demo” · Press File › Save in TextEdit's “README.md” ("), "録画の承認の 1 行: {said}");
        assert_eq!(m2_sent(&acts), 0, "承認の前に何も送らない");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{:?}", g.engine_view(&project)) };
        assert_eq!(state, RegionState::Verified, "{line}");
        let sent = |cmd: &str| std::fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.contains(&format!(r#""cmd":"{cmd}""#))).count();
        assert_eq!((sent("ax_replace_text"), sent("ax_set_value"), sent("key_to_pid"), sent("ax_menu")), (1, 0, 0, 1), "題の行だけを置き換えて File › Save");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    /// G81（r25 #20: 本物の AI で初めて ok・但し「✓ Verified」が disk の保存より ≈ 17 秒早い）: Project の repo の README.md の Page で、r25 の形の計画
    /// （見出しの 1 行の replace ＋ File › Save・model の checks は窓の本文だけ）を承認 → 偽の bridge の File › Save が 1.5 秒遅れて disk に書く →
    /// その間 Page は確かめ中の 1 行（Running の waiting）・済みは disk に書いた後にだけで、済みの 1 行が保存した file も確かめた事を言う / 書かない（FAKE_SAVE_TO 無し）→
    /// 済みにせず「送ったが確かめられない」の 1 行 / Project に repo が無い（書類の disk の場所が分からない）→ 窓だけで済み・済みの 1 行は窓の語だけで保存を言わない。
    /// 負の対照: G81_MUT=wire（Page の書類の disk の場所を Engine に渡さない）→ 保存の前に済み・書かなくても済みで赤 / copy（いつも保存を言う）→ repo 無しで赤 /
    /// quiet（保存を言わない）→ 保存した場面で赤
    #[cfg(unix)]
    #[test]
    fn g81_the_page_says_verified_only_after_the_save_reaches_disk() {
        let at = |extra: Value| {
            let mut i = json!({ "pid": 222, "app_name": "TextEdit" });
            i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            i
        };
        let gen = json!({
            "none_why": null, "workflow_steps": [
                step("s3", "desktop.ax.type_text", at(json!({ "window_id": 300, "label": "本文", "role": "AXTextArea", "replace": "# README", "text": "# Shibubu Demo" }))),
                step("s4", "desktop.ax.menu", at(json!({ "path": ["File", "Save"] }))),
            ],
            "checks": [check("desktop.window.state", at(json!({ "window_id": 300, "role": "AXTextArea", "contains": "# Shibubu Demo" })), "/controls/0/value", "contains", json!("# Shibubu Demo"))],
            "recipe": null, "trial": null,
        });
        let readme = "# README\n\nline two";
        let checking = "Changed in TextEdit · checking the saved file…";
        // 済みの 1 行の数は「1 item」「2 items」（1 だけ単数・operator 10-29）
        let is_items = |s: &str| s.split_once(' ').is_some_and(|(n, w)| n.parse::<usize>().is_ok_and(|n| w == if n == 1 { "item" } else { "items" }));
        for (name, has_repo, save) in [("g81-saved", true, true), ("g81-unsaved", true, false), ("g81-no-repo", false, false)] {
            let repo = tmp(&format!("{name}-repo"));
            // Page の書類 = repo からの相対（`to_app_page` の README.md は `mirror_page_ops` が docs/README.md にする）
            let file = repo.join("docs/README.md");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(repo.join("docs")).unwrap();
            std::fs::write(&file, readme).unwrap();
            let to = if save { format!(" FAKE_SAVE_TO='{}' FAKE_SAVE_MS=1500", file.display()) } else { String::new() };
            let env = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_TITLES=README.md FAKE_MENU='File>Save' FAKE_AX_VALUE='{readme}'{to}");
            let wait = Duration::from_secs(if save { 10 } else { 3 });
            let (mut g, project, _, _, dir) = m2_ask_gens(name, &env, Some((TEXTEDIT, "README.md")), RETITLE, &[gen.clone()], Value::Null, Some((repo.as_path(), wait)).filter(|_| has_repo), &[]);
            assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { .. })), "{name}: {:?}", g.engine_view(&project));
            let t = std::time::Instant::now();
            g.engine_act(&project, EngineAct::Approve);
            // 窓は変わったが保存した file がまだ = 確かめ中の 1 行（済みとは出さない）
            let mut seen = false;
            while !g.poll_engine() && t.elapsed() < patient(Duration::from_secs(20)) {
                seen |= matches!(g.engine_view(&project), Some(EngineView::Running { waiting: Some(w), .. }) if w == checking);
                std::thread::sleep(Duration::from_millis(10));
            }
            let took = t.elapsed();
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            let disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(seen, has_repo, "{name}: 確かめ中の 1 行（{took:?}）");
            if !has_repo {
                // 書類の disk の場所が分からない = 窓だけ確かめた = 済みの 1 行は窓の語だけ（保存を言わない）
                assert_eq!(state, RegionState::Verified, "{name}: {line}");
                assert!(line.starts_with("Done. Checked ") && is_items(line.trim_start_matches("Done. Checked ").trim_end_matches(" in the original app")) && !line.contains("saved"), "{name}: {line}");
                assert_eq!(disk, readme, "{name}");
            } else if save {
                assert_eq!(state, RegionState::Verified, "{name}: {line}");
                assert!(line.starts_with("Done. Checked ") && is_items(line.trim_start_matches("Done. Checked ").trim_end_matches(" in the original app and the saved file")), "{name}: {line}");
                assert!(disk.starts_with("# Shibubu Demo\n") && took >= Duration::from_millis(1500), "{name}: 保存の前に済みと言った（{took:?}・disk {disk:?}）");
            } else {
                assert_ne!(state, RegionState::Verified, "{name}: 保存が届かないのに済み: {line}");
                assert!(line.starts_with("Couldn't confirm whether it was sent, so it isn't marked done. It won't be sent again"), "{name}: {line}");
                assert_eq!(disk, readme, "{name}");
            }
            for x in [dir, repo] {
                let _ = std::fs::remove_dir_all(x);
            }
        }
    }

    const REAPER: &str = "com.cockos.reaper";
    const DISCORD: &str = "com.hnc.Discord";
    const TEXTEDIT: &str = "com.apple.TextEdit";
    /// 27 手の #20 の一言（TextEdit の Page で打つ・app を名指さない）
    const RETITLE: &str = "README の題を「Shibubu Demo」にして保存して";
    /// M2 の台本の一言（REAPER の Page で打つ・app を名指さない・track に名前は付けない = G-h）
    const ADD: &str = "Add a new track";
    /// M2b の台本の一言（Discord の Page で打つ・送らない・押すだけ）
    const OPEN: &str = "Open the general channel";
    /// M2b ③-8 の台本の一言（Discord の Page で打つ・開いている channel に試験の文を 1 回送る）
    const SEND: &str = "Post a short test message in this channel";
    /// G74b の一言（Discord の Page で打つ・誤って投稿した 1 件を消す）
    const DELETE: &str = "Delete the message I posted by mistake";
    /// 書類の無い app の Page（`to_app_page`・scene 2 の席と同じ file: None）
    const NO_FILE: &str = "";

    /// 試験の側から model に届いた request を読む（`set_llm` に渡した `Fake` は読めない）
    struct Seen(Arc<Fake>);
    impl crate::llm::Llm for Seen {
        fn label(&self) -> String {
            crate::llm::Llm::label(&*self.0)
        }
        fn send(&self, req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
            crate::llm::Llm::send(&*self.0, req)
        }
    }

    /// act の記録のうち送った手の数
    fn m2_sent(acts: &std::path::Path) -> usize {
        crate::gui::ActLog::read(acts).map(|v| v.iter().filter(|r| r.not_sent.is_none()).count()).unwrap_or(0)
    }

    /// M2 の偽の bridge の 1 回: `env` = 偽の bridge の env（`export` の後ろ）・`page` = 一言の前に足して移る Mirror の Page の (app, file)（None = 移らない）・
    /// `say` = Page の欄の一言・`gen` = 生成の model の答え・`target` = Goal Interpretation の model の答えの target。一言を送って承認の 1 行まで進め、
    /// 生成の model が受け取った request（生成の前に止まった = None）と act の記録の path を返す（生成まで進んだ時は承認の 1 行に居る事を見る）
    #[cfg(unix)]
    fn m2_ask(name: &str, env: &str, page: Option<(&str, &str)>, say: &str, gen: &Value, target: Value) -> (Generated, String, Option<crate::llm::JsonRequest>, std::path::PathBuf, std::path::PathBuf) {
        let (g, project, req, acts, dir) = m2_ask_any(name, env, page, say, gen, target);
        if req.is_some() {
            assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { .. })), "{name}: {:?}", g.engine_view(&project));
        }
        (g, project, req, acts, dir)
    }

    /// `m2_ask` の本体（生成まで進んだ後に止まる場面も見る G75 用・承認の 1 行に居る事を見ない）
    #[cfg(unix)]
    fn m2_ask_any(name: &str, env: &str, page: Option<(&str, &str)>, say: &str, gen: &Value, target: Value) -> (Generated, String, Option<crate::llm::JsonRequest>, std::path::PathBuf, std::path::PathBuf) {
        let (g, project, reqs, acts, dir) = m2_ask_gens(name, env, page, say, std::slice::from_ref(gen), target, None, &[]);
        (g, project, reqs.into_iter().next(), acts, dir)
    }

    /// `m2_ask_any` の生成の model の答えを順に `gens`（G79: 計画を作り直す場面）。生成の model が受け取った request を全部返す。
    /// `repo` = G81: Project の repo（workspace）と読み戻しの待ち（registry に repo の Adapter も入れる = 書類の file を読む）
    #[cfg(unix)]
    fn m2_ask_gens(
        name: &str,
        env: &str,
        page: Option<(&str, &str)>,
        say: &str,
        gens: &[Value],
        target: Value,
        repo: Option<(&std::path::Path, Duration)>,
        after: &[Value],
    ) -> (Generated, String, Vec<crate::llm::JsonRequest>, std::path::PathBuf, std::path::PathBuf) {
        use crate::adapters::mac_gui::MacGui;
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp(name);
        let (sh, acts) = (dir.join("bridge.sh"), dir.join(crate::gui::ACT_LOG_FILE));
        let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
        std::fs::write(&sh, format!("#!/bin/sh\nexport {env}\nexec python3 '{py}'\n")).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (s, c, a) = (sh.clone(), dir.join(crate::surface::ACT_CONSENT_FILE), acts.clone());
        let local = repo.is_some();
        let registry = move || {
            let mut r = if local { AdapterRegistry::with_local_adapters() } else { AdapterRegistry::default() };
            r.register(Box::new(MacGui::at(s.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        let (mut g, project) = wired_in(Fake::new(vec![]), registry, &dir, Default::default(), repo);
        if let Some((app, file)) = page {
            to_app_page(&mut g, &project, app, file);
        }
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": say, "success": say, "target": target }] });
        // `after` = 承認の後の run（同じ一言のし直し・interpret の後）で AI が受ける答え（ENGINE-AUDIT W5: 探す段の次の 1 手 = 手 0 なら止まる）
        let answers = [vec![Ok(json!({ "steps": [{ "verb": "unsupported", "name": say }] })), Ok(ops.clone())], gens.iter().cloned().map(Ok).collect(), vec![Ok(ops)], after.iter().cloned().map(Ok).collect()].concat();
        let fake = Arc::new(Fake::new(answers));
        g.set_llm(Ok(Box::new(Seen(fake.clone()))));
        g.page_intent(&project, PageIntent::Edit { text: say.into() }).unwrap();
        settle(&mut g);
        let reqs = fake.requests.lock().unwrap().iter().filter(|r| r.name == "engine_generate").cloned().collect();
        (g, project, reqs, acts, dir)
    }

    /// M2: Project に app の Mirror の Page（`file`）を足して移る（Page の app = `app`）。`file` が `NO_FILE` なら書類の無い app の席の面
    /// （scene 2 の REAPER・Discord と同じ `APP_SEAT_BACKEND`・file: None = Page の書類が無い = 名指しが無い = 役割で主窓・G75）。
    fn to_app_page(g: &mut Generated, project: &str, app_id: &str, file: &str) {
        use crate::domain::project::{ProjectPatchOp, SurfaceTarget};
        let app = project.strip_prefix("project.").expect("project.<app>");
        let (p, hash) = g.project_of(app).unwrap().expect("Project");
        let mut ops = crate::composer::project::mirror_page_ops(&p, app_id, if file == NO_FILE { "app" } else { file }).unwrap();
        for op in ops.iter_mut().filter(|_| file == NO_FILE) {
            if let ProjectPatchOp::AddSurface { spec, .. } = op {
                spec.backend = crate::composer::project::APP_SEAT_BACKEND;
                spec.target = SurfaceTarget::App { bundle_id: app_id.into(), file: None };
            }
        }
        let (next, _) = g.apply_project(app, &hash, &ops, crate::domain::project::RevisionOrigin::DirectEdit).unwrap();
        g.navigate_page(project, &next.pages.last().unwrap().id).unwrap();
    }

    /// M2a の本物の bridge の 1 回（試験の側が REAPER を `open -g -j -a REAPER --args -new` で起こしてから打ち、終わったら自分の pid だけ終了）:
    /// `M2_BRIDGE=$PWD/target/bridge/kangae-surface-bridge cargo test --bin shibubu m2_real_reaper -- --ignored --nocapture`。
    /// 製品の道（Page の欄 → Engine → 承認 → `MacGui` → 本物の bridge の `ax_menu` → 本物の窓の読み戻し）を、model の役だけ下の決まった答え（`Stub`・従量の API を使わない）で通す。
    /// 済みは読み戻しが本当に変わった時だけ（G-b）・試験の側の oracle は同じ bridge で `Track number` の AXButton を数える（前後の数は docs/evidence/M2/log.md）。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "本物の bridge（M2_BRIDGE）と起動中の REAPER が要る"]
    fn m2_real_reaper_add_track() {
        use crate::adapters::mac_gui::MacGui;
        use crate::gui::{ActLog, WindowState, ACT_LOG_FILE};
        use crate::llm::{JsonRequest, Llm, LlmError};
        let bridge = std::path::PathBuf::from(std::env::var("M2_BRIDGE").expect("M2_BRIDGE = 本物の bridge の path"));
        /// model の役（決まった答え）: 編集ではない → 1 操作（宛先 null = Page の app へ）→ 生成は input の窓から主窓を題名で選び（"About" を含まない "REAPER v"・
        /// 主窓は `REAPER v7.81 - EVALUATION LICENSE`（2026-10-25 23:58 実測）・起動直後に評価版の About の窓が出る回があり、1 回目の run はそちらを読んで
        /// 読み戻しが変わらなかった・無ければ input の窓を出して止まる）、menu を押して窓の要素が変わったかを読み戻す手順
        static PICKED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);
        struct Stub;
        impl Llm for Stub {
            fn label(&self) -> String {
                "m2-stub".into()
            }
            fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
                Ok(match req.name.as_str() {
                    "engine_interpret" => json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "track を 1 本足す", "success": "REAPER の窓の track が 1 本増える", "target": null }] }),
                    "engine_generate" => {
                        let input: Value = serde_json::from_str(&req.input).unwrap();
                        let ws = input["windows"].as_array().cloned().unwrap_or_default();
                        let main = |t: &str| t.contains("REAPER v") && !t.contains("About");
                        let w = ws.iter().find(|w| w["title"].as_str().is_some_and(main)).unwrap_or_else(|| panic!("REAPER の主窓が input に無い（G-a）: {ws:?}"));
                        eprintln!("M2 real: 生成の input の窓 = {:?}・選んだ窓 = {}", ws.iter().map(|w| (w["window_id"].clone(), w["title"].clone())).collect::<Vec<_>>(), w["window_id"]);
                        PICKED.store(w["window_id"].as_i64().unwrap_or(-1), std::sync::atomic::Ordering::SeqCst);
                        json!({
                            "none_why": null, "workflow_steps": [step("add", "desktop.ax.menu", json!({ "pid": w["pid"], "app_name": "REAPER", "path": ["Track", "Insert new track"] }))],
                            "checks": [check("desktop.window.state", json!({ "pid": w["pid"], "window_id": w["window_id"] }), "/controls", "changed", Value::Null)],
                            "recipe": null, "trial": null,
                        })
                    }
                    _ => json!({ "steps": [{ "verb": "unsupported", "name": ADD }] }),
                })
            }
        }
        let dir = tmp("m2-real");
        let (consents, acts) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE));
        let (b, c, a) = (bridge.clone(), consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(b.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        // 試験の側の oracle: REAPER の主窓の窓番号・要素の数・`Track number` の数・complete（Engine と別の registry と bridge で読む）。主窓は題名でなく
        // 製品の役割の規則（`pick_by_role` = 動かせる普通の窓・評価版の About は動かせない）で決める: 今の窓に無い (pid, window_id) と bundle_id を渡すと retarget が役割で選び直す
        let read = || -> (i64, usize, usize, bool) {
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(MacGui::at(bridge.clone()))).unwrap();
            reg.check();
            let caller = crate::adapters::Caller { app_id: "m2-oracle", run_id: None };
            let (outcome, obs) = reg.invoke(caller, "desktop.window.state", &json!({ "bundle_id": REAPER, "app_name": "REAPER", "pid": 0, "window_id": 0 }), None);
            let s: WindowState = serde_json::from_value(obs.unwrap_or_else(|| panic!("REAPER の主窓を役割で決められない: {outcome:?}")).payload).unwrap();
            (s.window_id, s.controls.len(), s.controls.iter().filter(|c| c.label.starts_with("Track number")).count(), s.complete)
        };
        let before = read();
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::from_secs(20), act_consents: consents };
        g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
        to_app_page(&mut g, &project, REAPER, NO_FILE);
        g.set_llm(Ok(Box::new(Stub)));
        g.page_intent(&project, PageIntent::Edit { text: ADD.into() }).unwrap();
        settle(&mut g);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の 1 行: {:?}", g.engine_view(&project)) };
        eprintln!("M2 real: 承認の 1 行 = {}", crate::ui::engine_view::approval_line(&goal, &steps));
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let after = read();
        let sent = ActLog::read(&acts).map(|v| v.iter().filter(|r| r.not_sent.is_none()).count()).unwrap_or(0);
        eprintln!("M2 real: 前 (主窓, 要素, Track number, complete) = {before:?} → 後 {after:?}・送った手 {sent}・Page = {:?}", g.engine_view(&project));
        assert_eq!(PICKED.load(std::sync::atomic::Ordering::SeqCst), before.0, "model の役が題名で選んだ窓 = 製品の役割の規則の主窓");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        assert_eq!((after.2, sent), (before.2 + 1, 1), "track が 1 本増え、menu を 1 回押した");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M2b の本物の bridge の 1 回（Discord の主窓の text channel を 1 つ押すだけ・送らない・入力欄 / DM / 設定に触らない・server を切り替えない / 作らない）:
    /// owner 10-26 04:3x「誰もいない自分自身だけのチャンネルならいいよ」= 押してよいのは member が owner 1 人だけの server の text channel だけ。
    /// 試験の側が先に bridge で窓を 2000 まで読み、member の見出しの合計が 1 で、押してよい text channel（今の題名に無い・名前を含む押せる要素が 1 つ）が
    /// ちょうど 1 つの時だけ `M2_CHANNEL_FILE` にその名前を書く（それ以外は何も押さずに未測）→
    /// `M2_BRIDGE=… M2_CHANNEL_FILE=… cargo test --bin shibubu m2b_real_discord -- --ignored --nocapture`。製品の道（Page の欄 → Engine → 承認 → `MacGui` の
    /// match = contains → 本物の bridge の `ax_press` → 主窓の題名の読み戻し）を、model の役だけ決まった答え（`Stub`・従量の API を使わない）で通す。
    /// channel の名前は記録に残さない: 出す文字は名前を `*` に替える（数と長さだけ）・act の記録の置き場は assert の前に消す。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "本物の bridge（M2_BRIDGE）・起動中の Discord・選んだ channel（M2_CHANNEL_FILE）が要る"]
    fn m2b_real_discord_open_channel() {
        use crate::adapters::mac_gui::MacGui;
        use crate::gui::{ActLog, WindowRef, ACT_LOG_FILE};
        use crate::llm::{JsonRequest, Llm, LlmError};
        let bridge = std::path::PathBuf::from(std::env::var("M2_BRIDGE").expect("M2_BRIDGE = 本物の bridge の path"));
        let name = std::fs::read_to_string(std::env::var("M2_CHANNEL_FILE").expect("M2_CHANNEL_FILE = 選んだ channel の名前の file")).unwrap().trim().to_string();
        assert!(!name.is_empty(), "channel を選べていない");
        let hide = |t: &str| t.replace(name.as_str(), &"*".repeat(name.chars().count()));
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        NAME.set(name.clone()).unwrap();
        /// model の役（決まった答え）: 1 操作（宛先 null = Page の app へ）→ 生成は input の窓（G-i で主窓 1 つ）に channel の名前の contains の click と題名の読み戻し
        struct Stub;
        impl Llm for Stub {
            fn label(&self) -> String {
                "m2b-stub".into()
            }
            fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
                let name = NAME.get().unwrap();
                Ok(match req.name.as_str() {
                    "engine_interpret" => json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "channel を開く", "success": "Discord の窓の題名がその channel になる", "target": null }] }),
                    "engine_generate" => {
                        let input: Value = serde_json::from_str(&req.input).unwrap();
                        let ws = input["windows"].as_array().cloned().unwrap_or_default();
                        let [w] = ws.as_slice() else { panic!("G-i: 生成の input の窓は主窓 1 つ（{} 個）", ws.len()) };
                        json!({
                            "none_why": null, "workflow_steps": [step("open", "desktop.ax.click", json!({ "pid": w["pid"], "window_id": w["window_id"], "app_name": w["app_name"], "label": name, "role": "", "match": "contains" }))],
                            "checks": [check("desktop.windows.list", json!({ "bundle_id": DISCORD, "main": true }), "/0/title", "contains", json!(name))],
                            "recipe": null, "trial": null,
                        })
                    }
                    _ => json!({ "steps": [{ "verb": "unsupported", "name": OPEN }] }),
                })
            }
        }
        let dir = tmp("m2b-real");
        let (consents, acts) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE));
        let (b, c, a) = (bridge.clone(), consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(b.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        // 試験の側の oracle: 主窓の題名に名前が在るか・題名の長さ（Engine と別の registry と bridge で読む・主窓は製品と同じ main = pick_by_role）
        let title = || -> (bool, usize) {
            let mut reg = AdapterRegistry::default();
            reg.register(Box::new(MacGui::at(bridge.clone()))).unwrap();
            reg.check();
            let caller = crate::adapters::Caller { app_id: "m2b-oracle", run_id: None };
            let (outcome, obs) = reg.invoke(caller, "desktop.windows.list", &json!({ "bundle_id": DISCORD, "main": true }), None);
            let ws: Vec<WindowRef> = serde_json::from_value(obs.unwrap_or_else(|| panic!("Discord の主窓を読めない: {}", hide(&format!("{outcome:?}")))).payload).unwrap();
            let t = ws.first().map(|w| w.title.clone()).unwrap_or_default();
            (t.contains(name.as_str()), t.chars().count())
        };
        let before = title();
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::from_secs(20), act_consents: consents };
        g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
        to_app_page(&mut g, &project, DISCORD, NO_FILE);
        g.set_llm(Ok(Box::new(Stub)));
        g.page_intent(&project, PageIntent::Edit { text: OPEN.into() }).unwrap();
        settle(&mut g);
        // ENGINE-AUDIT W7: channel を開く手は戻せる手（`desktop.ax.reveal`）= 承認の card を出さずに押す（前は承認の 1 行「Press “名前” in Discord」）
        let asked = matches!(g.engine_view(&project), Some(EngineView::Approve { .. }));
        let after = title();
        let sent = ActLog::read(&acts).map(|v| v.iter().filter(|r| r.not_sent.is_none()).count()).unwrap_or(0);
        let view = g.engine_view(&project).cloned();
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!(
            "M2b real: channel の名前 {} 字・承認の card {asked}・題名 (名前を含む, 字数) 前 {before:?} → 後 {after:?}・送った手 {sent}・Page = {}",
            name.chars().count(),
            hide(&format!("{view:?}"))
        );
        assert!(!asked, "W7: 開く手に承認を聞いた");
        assert!(!before.0, "試験の側が選んだ channel は押す前の題名に無い");
        assert!(matches!(view, Some(EngineView::Line { state: RegionState::Verified, .. })), "済みにならなかった（上の Page の 1 行）");
        assert_eq!((after.0, sent), (true, 1), "押した channel が題名に出て、押したのは 1 回");
    }

    /// M2b ③-8 の本物の bridge の 1 回（owner 10-26 10:4x「２つ目作らなくていいよそのまま書いて誰もいないから」）: member が owner 1 人だけの server の、
    /// 開いている唯一の text channel に、個人の情報を持たない短い nonce を 1 回だけ送る。前段（試験の側・bridge で読むだけ）が題名に server の名前・member の
    /// 見出しの合計 1・AXLink の text channel が 1 つで開いている・入力欄が 1 つで空、を確かめた時だけ `M2_SEND_FILE` に channel の名前・入力欄の label と
    /// role・nonce を書く → ここで製品の道（Page の欄 → Engine → 承認 1 回 → `MacGui` の `desktop.ax.type_text` → `desktop.key` return）を 1 回 →
    /// 読み戻し（nonce が一覧に出た・入力欄が空に戻った）は後段（試験の側・2000 まで読む）。送り直さない・消さない・他を押さない。出す文字は nonce・数・長さだけ。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "本物の bridge（M2_BRIDGE）・起動中の Discord・前段の読み（M2_SEND_FILE）が要る"]
    fn m2b_real_discord_send_nonce() {
        use crate::adapters::mac_gui::MacGui;
        use crate::gui::{ActLog, ACT_LOG_FILE};
        use crate::llm::{JsonRequest, Llm, LlmError};
        let bridge = std::path::PathBuf::from(std::env::var("M2_BRIDGE").expect("M2_BRIDGE = 本物の bridge の path"));
        let pick: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("M2_SEND_FILE").expect("M2_SEND_FILE = 前段の読みの file")).unwrap()).unwrap();
        let s = |k: &str| pick[k].as_str().unwrap_or_default().to_string();
        let (name, nonce) = (s("name"), s("nonce"));
        assert!(!name.is_empty() && !nonce.is_empty() && !s("field_label").is_empty() && !s("field_role").is_empty(), "前段の読みが足りない");
        let hide = |t: &str| t.replace(name.as_str(), &"*".repeat(name.chars().count()));
        static PICK: std::sync::OnceLock<(String, String, String)> = std::sync::OnceLock::new();
        PICK.set((s("field_label"), s("field_role"), nonce.clone())).unwrap();
        /// model の役（決まった答え）: 1 操作（宛先 null = Page の app へ）→ 生成は input の窓（主窓 1 つ）の入力欄に nonce を置き、return を 1 回
        struct Stub;
        impl Llm for Stub {
            fn label(&self) -> String {
                "m2b8-stub".into()
            }
            fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
                let (label, role, nonce) = PICK.get().unwrap();
                Ok(match req.name.as_str() {
                    "engine_interpret" => json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "試験の文を 1 回送る", "success": "channel に試験の文が出る", "target": null }] }),
                    "engine_generate" => {
                        let input: Value = serde_json::from_str(&req.input).unwrap();
                        let ws = input["windows"].as_array().cloned().unwrap_or_default();
                        let [w] = ws.as_slice() else { panic!("G-i: 生成の input の窓は主窓 1 つ（{} 個）", ws.len()) };
                        json!({
                            "none_why": null, "workflow_steps": [
                                step("type", "desktop.ax.type_text", json!({ "pid": w["pid"], "window_id": w["window_id"], "app_name": w["app_name"], "label": label, "role": role, "text": nonce })),
                                step("send", "desktop.key", json!({ "pid": w["pid"], "app_name": w["app_name"], "key": "return" })),
                            ],
                            "checks": [check("desktop.window.state", json!({ "pid": w["pid"], "window_id": w["window_id"], "app_name": w["app_name"] }), "/controls", "changed", Value::Null)],
                            "recipe": null, "trial": null,
                        })
                    }
                    _ => json!({ "steps": [{ "verb": "unsupported", "name": SEND }] }),
                })
            }
        }
        let dir = tmp("m2b8-real");
        let (consents, acts) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE));
        let (b, c, a) = (bridge.clone(), consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(b.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::from_secs(20), act_consents: consents };
        g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
        to_app_page(&mut g, &project, DISCORD, NO_FILE);
        g.set_llm(Ok(Box::new(Stub)));
        g.page_intent(&project, PageIntent::Edit { text: SEND.into() }).unwrap();
        settle(&mut g);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の 1 行: {}", hide(&format!("{:?}", g.engine_view(&project)))) };
        let line = crate::ui::engine_view::approval_line(&goal, &steps);
        // 承認は 1 回だけ（送れたか分からなくても押し直さない = 読み戻しは後段）
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let log = ActLog::read(&acts).unwrap_or_default();
        let sent = log.iter().filter(|r| r.not_sent.is_none()).count();
        let view = g.engine_view(&project).cloned();
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!(
            "M2b8 real: nonce {nonce}・承認の 1 行 {} 字（手 {}）・act の記録 {} 行（送った手 {sent}）・Page = {}",
            line.chars().count(),
            steps.len(),
            log.len(),
            hide(&format!("{view:?}"))
        );
        assert_eq!(steps.len(), 2, "承認の手は入力欄に置く 1 手と return の 1 手");
        assert_eq!(sent, 2, "置いて return を 1 回ずつ送った（読み戻しは後段）");
    }

    /// G74b の本物の bridge の 1 回（owner 10-26 14:16 G の pane で「許可」・operator の「画面空き」の後）: member が owner 1 人だけの server の開いている
    /// channel で、11:57 に誤って投稿された 1 件（本文 6 字）を製品の道で消す。前段（試験の側・bridge で読むだけ）がその行がちょうど 1 つで、製品の
    /// `find_containing`（AXShowMenu）がその行 1 つに当たる事を確かめた時だけ `M2_DELETE_FILE` に本文・行の role と subrole・行の数を書く →
    /// ここで製品の道（Page の欄 → Engine → 承認 1 回 → `MacGui` の click 3 つ: 行の context menu → Delete Message → dialog が本文を見せている時だけ Delete）
    /// → Engine の読み戻し（その本文の行が無い・行が 1 つ減った）→ 後段（試験の側・読むだけ）。押し直さない・他の message に触れない。出す文字は数と長さだけ
    /// （本文は `*` に替える・act の記録の置き場は assert の前に消す）。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "本物の bridge（M2_BRIDGE）・起動中の Discord・前段の読み（M2_DELETE_FILE）が要る"]
    fn m2b_real_discord_delete() {
        use crate::adapters::mac_gui::MacGui;
        use crate::gui::{ActLog, ACT_LOG_FILE};
        use crate::llm::{JsonRequest, Llm, LlmError};
        let bridge = std::path::PathBuf::from(std::env::var("M2_BRIDGE").expect("M2_BRIDGE = 本物の bridge の path"));
        let pick: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("M2_DELETE_FILE").expect("M2_DELETE_FILE = 前段の読みの file")).unwrap()).unwrap();
        let s = |k: &str| pick[k].as_str().unwrap_or_default().to_string();
        let body = s("body");
        assert!(!body.trim().is_empty() && !s("row_role").is_empty() && !s("row_subrole").is_empty(), "前段の読みが足りない");
        let hide = |t: &str| t.replace(body.as_str(), &"*".repeat(body.chars().count()));
        static PICK: std::sync::OnceLock<(String, String, String)> = std::sync::OnceLock::new();
        PICK.set((body.clone(), s("row_role"), s("row_subrole"))).unwrap();
        /// model の役（決まった答え）: 1 操作（宛先 null = Page の app へ）→ 生成は input の窓（主窓 1 つ）で g74b の偽の bridge の試験と同じ 3 手と読み戻し 2 つ
        struct Stub;
        impl Llm for Stub {
            fn label(&self) -> String {
                "m2b-delete-stub".into()
            }
            fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
                let (body, role, subrole) = PICK.get().unwrap();
                Ok(match req.name.as_str() {
                    "engine_interpret" => json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "誤って投稿した 1 件を消す", "success": "その 1 件が一覧から消える", "target": null }] }),
                    "engine_generate" => {
                        let input: Value = serde_json::from_str(&req.input).unwrap();
                        let ws = input["windows"].as_array().cloned().unwrap_or_default();
                        let [w] = ws.as_slice() else { panic!("G-i: 生成の input の窓は主窓 1 つ（{} 個）", ws.len()) };
                        let at = |extra: Value| {
                            let mut v = json!({ "pid": w["pid"], "window_id": w["window_id"], "app_name": w["app_name"] });
                            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
                            v
                        };
                        json!({
                            "none_why": null, "workflow_steps": [
                                step("menu", "desktop.ax.click", at(json!({ "label": body, "role": "", "match": "contains", "press": "AXShowMenu" }))),
                                step("item", "desktop.ax.click", at(json!({ "label": "Delete Message", "role": "AXMenuItem", "match": "contains" }))),
                                step("delete", "desktop.ax.click", at(json!({ "label": "Delete", "role": "AXButton", "only_if": [body] }))),
                            ],
                            "checks": [
                                check("desktop.window.state", at(json!({ "role": role, "subrole": subrole, "contains": body })), "/controls", "equals", json!([])),
                                check("desktop.window.state", at(json!({ "role": role, "subrole": subrole })), "/controls", "fewer", json!(1)),
                            ],
                            "recipe": null, "trial": null,
                        })
                    }
                    _ => json!({ "steps": [{ "verb": "unsupported", "name": DELETE }] }),
                })
            }
        }
        let dir = tmp("m2b-delete-real");
        let (consents, acts) = (dir.join(crate::surface::ACT_CONSENT_FILE), dir.join(ACT_LOG_FILE));
        let (b, c, a) = (bridge.clone(), consents.clone(), acts.clone());
        let registry = move || {
            let mut r = AdapterRegistry::default();
            r.register(Box::new(MacGui::at(b.clone()).with_act_consents(c.clone()).with_act_log(a.clone()))).unwrap();
            r
        };
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::from_secs(20), act_consents: consents };
        g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
        to_app_page(&mut g, &project, DISCORD, NO_FILE);
        g.set_llm(Ok(Box::new(Stub)));
        g.page_intent(&project, PageIntent::Edit { text: DELETE.into() }).unwrap();
        settle(&mut g);
        let Some(EngineView::Approve { goal, steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の 1 行: {}", hide(&format!("{:?}", g.engine_view(&project)))) };
        let line = crate::ui::engine_view::approval_line(&goal, &steps);
        // 承認は 1 回だけ（消せたか分からなくても押し直さない）
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let log = ActLog::read(&acts).unwrap_or_default();
        let sent = log.iter().filter(|r| r.not_sent.is_none()).count();
        let view = g.engine_view(&project).cloned();
        // G74c: 止まった時の Engine の内側の理由（engine-runs の error・本文は * に替える）
        let inner = runs(&dir).last().map(|r| hide(&r["error"].to_string())).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!(
            "M2b delete real: 本文 {} 字・承認の 1 行 {} 字（手 {}）・act の記録 {} 行（送った手 {sent}・{:?}）・Page = {}・内側 = {inner}",
            body.chars().count(),
            line.chars().count(),
            steps.len(),
            log.len(),
            log.iter().map(|r| format!("{:?}", r.receipt.action).chars().take(40).collect::<String>()).collect::<Vec<_>>(),
            hide(&format!("{view:?}"))
        );
        assert_eq!(steps.len(), 3, "承認の手は context menu・Delete Message・Delete の 3 つ（W7: 承認の要る手の在る手順は押す手も全部 click = 走らせる前に 1 枚で聞く）");
        assert!(matches!(view, Some(EngineView::Line { state: RegionState::Verified, .. })), "済みにならなかった（上の Page の 1 行）");
        assert_eq!(sent, 3, "3 つを 1 回ずつ押した");
    }

    /// W19-3-fix の口: 解析の Backend の進行の受け口（`analysis::Progress`）と Page の Engine の口（`EngineRequest::progress`）は同じ形・Page の 1 行は `Running::waiting`
    /// （yc/gui の G2 ee29e07 と同じ名前と形・受け手は G2 の `w19_3b_progress_signatures` の registry と 2 つ）。
    #[test]
    fn w19_3_fix_progress_signatures() {
        use crate::intel::analysis::{AnalysisBackend, Progress};
        let _: fn(&mut dyn AnalysisBackend, Progress) = |b, p| b.set_progress(p);
        let p: Progress = Arc::new(|_| {});
        let req = crate::generated::EngineRequest { app_id: "a".into(), run_id: "r".into(), goal: "g".into(), consents: vec![], grants: vec![], llm: None, target: None, document: None, document_file: None, window: None, repo: None, progress: p, runs: None, automation: Default::default(), situation: Default::default(), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) };
        (req.progress)(None);
        let _ = EngineView::Running { goal: "g".into(), waiting: Some("REA を起こしています（最長 90 秒）".into()), stopping: false };
    }

    /// WF-017 (2) の Page の形・WF-016: Decide の card が出た Project を閉じて（Generated を捨てる）同じ sqlite と data dir で開き直すと、
    /// 同じ card（問い・選択肢の名前と証拠）が同じ Project に出て、選ぶと選んだ道だけが cursor の続きから走る。下書きは再送しない・選んでも承認の receipt は 0。
    /// 負の対照: `set_engine` の止まった run の読み（`PageEngine::waiting`）を外すと、開き直した Project に card が出ずに赤
    #[test]
    fn wf016_the_page_shows_the_same_choice_after_a_restart() {
        use crate::capability_engine::{save_workflow, wf_d, WORKFLOWS_DIR};
        let dir = tmp("wf016");
        let db = dir.join("kangae.sqlite3");
        let desk = Arc::new(Mutex::new(wf_d::Desk { note: "Notes".into(), ..Default::default() }));
        save_workflow(&dir.join(WORKFLOWS_DIR), &wf_d::titled(wf_d::GOAL)).unwrap();
        let wire = || {
            let mut g = Generated::start(Store::open(&db).unwrap(), ActionRegistry::with_local_actions(), None);
            let d = desk.clone();
            let registry = move || wf_d::desk_registry(&d);
            let core = CoreEngine { registry: Arc::new(registry), data_dir: dir.clone(), mcp: Default::default(), env: EnvFacts { os_version: "test".into(), ..Default::default() }, readback: Duration::ZERO, act_consents: dir.join(crate::surface::ACT_CONSENT_FILE) };
            g.set_engine(EngineSetup { engine: Arc::new(core), data_dir: dir.clone() });
            g
        };
        let mut g = wire();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert!(g.start_engine(&project, wf_d::GOAL, vec![], None));
        settle(&mut g);
        let Some(EngineView::Decide { question, choices, line: None, .. }) = g.engine_view(&project).cloned() else { panic!("判断の card: {:?}", g.engine_view(&project)) };
        assert_eq!(question, "Which title?");
        assert_eq!(choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Use the short title", "Use the long title"]);
        assert!(choices[1].lines.iter().any(|l| l.contains("Notes: the long title")) && choices[1].lines.iter().any(|l| l.contains("Says more")), "選択肢ごとの証拠: {choices:?}");
        assert_eq!(desk.lock().unwrap().count("desk.note.set"), 0, "選ぶ前は送らない");
        drop(g);
        let mut g = wire();
        g.open_project(&project, None).unwrap();
        let want = EngineView::Decide { goal: wf_d::GOAL.into(), question, choices, line: None };
        assert_eq!(g.engine_view(&project), Some(&want), "同じ card が同じ Project に");
        g.engine_act(&project, EngineAct::Choose(1));
        assert!(matches!(g.engine_view(&project), Some(EngineView::Running { .. })), "{:?}", g.engine_view(&project));
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g.engine_view(&project));
        let d = desk.lock().unwrap();
        assert_eq!(d.note, "Notes: the long title", "試験の側の oracle: 選んだ題");
        assert_eq!((d.count("desk.draft.make"), d.count("desk.note.set")), (1, 1), "下書きは再送しない: {:?}", d.calls);
        assert_eq!(runs(&dir).last().unwrap()["consents"], json!(0), "選んでも承認の receipt は 0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G101（FX-111・CHAT-013・RULE-005 の 4 場面 = P48-15b）: TextEdit の Page（書類 = この Project の repo の docs/README.md）で #20 の一言。窓は書類（AXDocument）で決める。
    /// open = この README だけ → 承認 1 回 → README（300）に置いて保存して済み
    /// twins = 同じ題名の README が 2 つ（300 = 別の folder・主窓でない / 301 = この Project・主窓）→ 301 だけに置く
    /// closed = notes.md だけが前面 → 生成の model を呼ばず「Approve and open」（開く手 1 つ・何も変えていない）→ 承認 → 偽の bridge が README を 301 に開く →
    ///   変える手の承認 → 301 だけに置いて済み（notes の 300 には何も送らない）
    /// other = 別の folder の README.md だけ（300）→ closed と同じく開く手 → 開いた 301 だけに置く
    /// どの場面も変わるのはこの Project の README だけ（別の folder の README は元のまま）。
    /// 負の対照: retarget の書類の絞り込みを外す → twins が「can't tell which」で止まり赤・generate の open_named を外す → closed が「isn't open」で止まり赤・
    ///   approval_line の opening の腕を外す → 「Send …」の 1 行で赤・model の窓の document を消す所を外す → home の path が model に届いて赤
    #[cfg(unix)]
    #[test]
    fn g101_only_this_projects_readme_changes_in_four_scenes() {
        use crate::gui::GuiAction;
        let gen = |window: u64| {
            let at = |extra: Value| {
                let mut i = json!({ "pid": 222, "app_name": "TextEdit" });
                i.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
                i
            };
            json!({
                "none_why": null, "workflow_steps": [
                    step("s3", "desktop.ax.type_text", at(json!({ "window_id": window, "label": "本文", "role": "AXTextArea", "replace": "# README", "text": "# Shibubu Demo" }))),
                    step("s4", "desktop.ax.menu", at(json!({ "path": ["File", "Save"] }))),
                ],
                "checks": [check("desktop.window.state", at(json!({ "window_id": window, "role": "AXTextArea", "contains": "# Shibubu Demo" })), "/controls/0/value", "contains", json!("# Shibubu Demo"))],
                "recipe": null, "trial": null,
            })
        };
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": RETITLE, "success": RETITLE, "target": null }] });
        let readme = "# README\n\nline two";
        let sets = |acts: &std::path::Path| crate::gui::ActLog::read(acts).unwrap_or_default().into_iter().filter(|r| r.not_sent.is_none() && matches!(r.receipt.action, GuiAction::SetValue { .. })).map(|r| r.receipt.target.window_id).collect::<Vec<_>>();
        let finish = |g: &mut Generated, project: &str| {
            g.engine_act(project, EngineAct::Approve);
            let t = std::time::Instant::now();
            while !g.poll_engine() && t.elapsed() < patient(Duration::from_secs(30)) {
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        for (name, opens) in [("g101-open", false), ("g101-twins", false), ("g101-closed", true), ("g101-other", true)] {
            let repo = tmp(&format!("{name}-repo"));
            let file = repo.join("docs/README.md");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(repo.join("docs")).unwrap();
            std::fs::write(&file, readme).unwrap();
            let elsewhere = tmp(&format!("{name}-elsewhere")).join("README.md");
            std::fs::write(&elsewhere, readme).unwrap();
            let (ours, theirs) = (format!("file://{}", file.display()), format!("file://{}", elsewhere.display()));
            let wl = match name {
                "g101-open" => format!(r#"[[{{"title":"README.md","document":"{ours}"}}]]"#),
                "g101-twins" => format!(r#"[[{{"title":"README.md","document":"{theirs}","main":false}},{{"title":"README.md","document":"{ours}","main":true}}]]"#),
                "g101-closed" => r#"[[{"title":"notes.md","document":"file:///nowhere/notes.md"}]]"#.to_string(),
                _ => format!(r#"[[{{"title":"README.md","document":"{theirs}"}}]]"#),
            };
            let window = if name == "g101-open" { 300 } else { 301 };
            let env = format!("FAKE_APP_BUNDLE={TEXTEDIT} FAKE_APP_NAME=TextEdit FAKE_MENU='File>Save' FAKE_AX_VALUE='{readme}' FAKE_SAVE_TO='{}' FAKE_SAVE_MS=300 FAKE_WL='{wl}' FAKE_OPEN_STATE='{}'", file.display(), repo.join(".fake-opened").display());
            let gens = if opens { vec![ops.clone(), gen(window)] } else { vec![gen(window)] };
            let (mut g, project, reqs, acts, dir) = m2_ask_gens(name, &env, Some((TEXTEDIT, "README.md")), RETITLE, &gens, Value::Null, Some((repo.as_path(), Duration::from_secs(10))), &[]);
            let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("{name}: 承認の 1 行に居ない: {:?}", g.engine_view(&project)) };
            if !opens {
                let input: Value = serde_json::from_str(&reqs[0].input).unwrap();
                assert!(input["windows"].as_array().is_some_and(|ws| !ws.is_empty() && ws.iter().all(|w| w.get("document").is_none_or(|d| d == ""))), "{name}: 書類の場所（home の path）を model に渡した: {}", input["windows"]);
            }
            if opens {
                assert!(reqs.is_empty(), "{name}: 開く前に生成の model を呼んだ");
                assert_eq!(steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), [crate::ui::engine_view::OPEN_STEP], "{name}: 開く手 1 つだけ");
                assert_eq!(steps[0].input["document_file"], json!(ours), "{name}: 開くのはこの Project の README");
                assert_eq!(crate::ui::engine_view::OPEN_STEP, crate::capability_engine::OPEN_DOCUMENT_ACTION, "Engine と同じ id");
                let line = crate::ui::engine_view::approval_line(RETITLE, &steps);
                assert!(line.contains("open this project's “README.md” in TextEdit") && line.contains("nothing is changed yet"), "{name}: {line}");
                assert_eq!(m2_sent(&acts), 0, "{name}: 開く承認の前に何も送らない");
                finish(&mut g, &project);
                assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { steps, .. }) if steps.iter().all(|s| s.action != crate::ui::engine_view::OPEN_STEP)), "{name}: 開いた後は変える手の承認: {:?}", g.engine_view(&project));
            }
            finish(&mut g, &project);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            assert_eq!(state, RegionState::Verified, "{name}: {line}");
            assert_eq!(sets(&acts), [Some(window as u32)], "{name}: 置いたのはこの Project の README の窓だけ");
            assert!(std::fs::read_to_string(&file).unwrap().contains("# Shibubu Demo"), "{name}: この Project の README が変わった");
            assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), readme, "{name}: 別の folder の README は元のまま");
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir_all(&repo);
            let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
        }
    }

    /// G101（FX-115・P49-2d）: Discord の Page で試験の文を送る手（type_text → return）。字を置いた後に窓の題名が別の channel に替わった
    /// （FAKE_TITLE_AFTER_SET）→ 送る直前の読み直しで宛先が違う → return を送らずに止まる（「送る手を打っていない」の文・channel の名前を 1 行に書かない）。
    /// 替わらなければ return を 1 回送る（g74-sent と同じ）。負の対照: recheck の題名の確かめを外す → return を送って赤。
    #[cfg(unix)]
    #[test]
    fn g101_the_send_step_stops_when_the_channel_changed() {
        let typed = json!({ "pid": 222, "window_id": 300, "app_name": "Discord", "label": "本文", "role": "AXTextArea", "text": "shibubu-test-g101" });
        let gen = json!({
            "none_why": null,
            "workflow_steps": [step("type", "desktop.ax.type_text", typed), step("send", "desktop.key", json!({ "pid": 222, "app_name": "Discord", "key": "return" }))],
            "checks": [check("desktop.window.state", json!({ "pid": 222, "window_id": 300 }), "/controls/2/value", "equals", json!(""))],
            "recipe": null, "trial": null,
        });
        let base = format!("FAKE_APP_BUNDLE={DISCORD} FAKE_APP_NAME=Discord FAKE_TITLES='#general | Test Server - Discord'");
        for (name, env, sent) in [("g101-moved", "FAKE_TITLE_AFTER_SET='#random | Test Server - Discord'", vec!["set"]), ("g101-stayed", "", vec!["set", "key"])] {
            let (mut g, project, req, acts, dir) = m2_ask(name, &format!("{base} {env}"), Some((DISCORD, NO_FILE)), SEND, &gen, Value::Null);
            assert!(req.is_some(), "{name}: 生成まで進まなかった");
            g.engine_act(&project, EngineAct::Approve);
            settle(&mut g);
            let Some(EngineView::Line { state, line }) = g.engine_view(&project).cloned() else { panic!("{name}: {:?}", g.engine_view(&project)) };
            assert_ne!(state, RegionState::Verified, "{name}: {line}");
            assert_eq!(g74_kinds(&acts), sent, "{name}: 送った手: {line}");
            if name == "g101-moved" {
                assert!(line.contains("Shibubu didn't do the send step") && line.contains("another place"), "{name}: {line}");
                assert!(!line.contains("#random") && !line.contains("#general"), "{name}: channel の名前を書いた: {line}");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// G86a: Engine に渡した依頼の一言は 1 回だけ会話に（承認して run し直しても足さない）・承認の間は答えを足さない（card だけ）・済んだ 1 行は Verified で残る。
    /// 承認しなかった（Cancel）は「Not sent」の Stopped の 1 行。負の対照（G86M）: line = poll_engine の say を外す → 済んだ 1 行が会話に無い
    #[test]
    fn g86_the_request_and_its_result_join_the_chat() {
        use crate::domain::chat::{Mark, Who};
        let dir = tmp("g86");
        let w = World::default();
        *w.text.lock().unwrap() = "# README".into();
        let (mut g, project) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&w), &dir);
        let before = g.chat(&project).len();
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { .. })), "{:?}", g.engine_view(&project));
        let now: Vec<(Who, String, Mark)> = g.chat(&project).into_iter().skip(before).map(|t| (t.who, t.text, t.mark)).collect();
        assert_eq!(now, [(Who::You, ASK.to_string(), Mark::None)], "承認の間は一言だけ");
        g.engine_act(&project, EngineAct::Approve);
        settle(&mut g);
        let Some(EngineView::Line { line, .. }) = g.engine_view(&project).cloned() else { panic!("{:?}", g.engine_view(&project)) };
        let now: Vec<(Who, String, Mark)> = g.chat(&project).into_iter().skip(before).map(|t| (t.who, t.text, t.mark)).collect();
        assert_eq!(now, [(Who::You, ASK.to_string(), Mark::None), (Who::Shibubu, line, Mark::Verified)], "承認の run し直しで一言を足さない");
        let w2 = World::default();
        let (mut g2, p2) = wired(Fake::new(answers(pad_answer("# README\nnonce-w19"))), pad_registry(&w2), &tmp("g86-cancel"));
        g2.page_intent(&p2, PageIntent::Edit { text: ASK.into() }).unwrap();
        settle(&mut g2);
        g2.engine_act(&p2, EngineAct::Cancel);
        let last = g2.chat(&p2).last().cloned().unwrap();
        assert_eq!((last.who, last.text.as_str(), last.mark), (Who::Shibubu, "Not sent. Nothing was changed", Mark::Stopped));
        assert!(w2.writes().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Running の card の Stop（`engine_act` が `stop_engine` で止める印を立てる）: 押すと押せない「Stopping…」に替わり（2 度目も同じ）、run が閉じたら
    /// 承認の card を出さず会話に「Stopped by you」の 1 行（Mark::Stopped）・何も送らない・後の Approve で送り直さない。走っていない時の Stop は何もしない。
    /// 止める前に済んだ run（Engine が Done で答えた）は済みの 1 行のまま = 止めたと言わない（Engine の止め方の時機に依らないよう、印を見ずに Done で答える Engine で）。
    /// 負の対照（未測）: `poll_engine` の by_you を外す → 承認の card が残って赤・by_you の Done の除外を外す → 済みが「Stopped by you」になって後半が赤・
    /// draw の stopping の腕を外す → 押した後も Stop が押せて AX が赤・`engine_act` の `stop_engine` の false で返る行を外す → 走っていない時は view が無いので緑のまま（等価・`stop_engine` の false は ENGINE の試験が縛る）。
    #[test]
    fn stop_on_the_running_card_closes_as_stopped_by_you_but_never_claims_a_finished_run() {
        use crate::domain::chat::{Mark, Who};
        use crate::generated::{EngineAnswer, EngineReply, EngineRequest, PageEngine};
        use crate::ui::engine_view::{STOPPED_LINE, STOPPING_LABEL, STOP_LABEL};
        use eframe::egui;
        // 描いた card の button（AX の名前・押せるか）
        let buttons = |g: &Generated, project: &str| -> Vec<(String, bool)> {
            let ctx = egui::Context::default();
            crate::ui::theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let view = g.engine_view(project).cloned().expect("Engine の card");
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::ui::engine_view::draw(ui, &view);
                });
            });
            let nodes = out.platform_output.accesskit_update.expect("AccessKit の木").nodes;
            nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| Some((n.label()?.to_owned(), !n.is_disabled()))).collect()
        };
        let dir = tmp("stop");
        let w = World::default();
        *w.text.lock().unwrap() = "# README".into();
        let (mut g, project) = wired(Fake::new(answers(pad_answer("# README\nnonce-stop"))), pad_registry(&w), &dir);
        g.engine_act(&project, EngineAct::Stop);
        assert_eq!(g.engine_view(&project), None, "走っていない時の Stop");
        g.page_intent(&project, PageIntent::Edit { text: ASK.into() }).unwrap();
        assert_eq!(buttons(&g, &project), [(STOP_LABEL.to_string(), true)]);
        g.engine_act(&project, EngineAct::Stop);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Running { stopping: true, .. })), "{:?}", g.engine_view(&project));
        assert_eq!(buttons(&g, &project), [(STOPPING_LABEL.to_string(), false)], "押した後は押せない Stopping…");
        g.engine_act(&project, EngineAct::Stop);
        settle(&mut g);
        assert_eq!(g.engine_view(&project), Some(&EngineView::Line { state: RegionState::Ready, line: STOPPED_LINE.into() }), "止めた run の承認の card");
        let last = g.chat(&project).last().cloned().unwrap();
        assert_eq!((last.who, last.text.as_str(), last.mark), (Who::Shibubu, STOPPED_LINE, Mark::Stopped));
        g.engine_act(&project, EngineAct::Approve);
        assert!(!matches!(g.engine_view(&project), Some(EngineView::Running { .. })), "止めた後の Approve で送り直した");
        assert!(w.writes().is_empty(), "{:?}", w.writes());
        // 止める前に済んだ run: 印を見ずに Done で答える Engine（送り終えて確かめた後に Stop が届いた形）は済みの 1 行（Stopped by you と言わない）
        struct DoneAnyway;
        impl PageEngine for DoneAnyway {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                EngineAnswer { reply: EngineReply::Done { verified: 1, saved: 0, history: Vec::new(), partial: None, without_ai: None, already: None }, log: ("done", json!({})) }
            }
        }
        let done_dir = tmp("stop-done");
        let (mut g2, p2) = wired(Fake::new(answers(pad_answer("# README\nnonce-stop"))), pad_registry(&World::default()), &done_dir);
        g2.set_engine(EngineSetup { engine: Arc::new(DoneAnyway), data_dir: done_dir });
        g2.page_intent(&p2, PageIntent::Edit { text: ASK.into() }).unwrap();
        g2.engine_act(&p2, EngineAct::Stop);
        assert!(matches!(g2.engine_view(&p2), Some(EngineView::Running { stopping: true, .. })), "{:?}", g2.engine_view(&p2));
        settle(&mut g2);
        assert!(matches!(g2.engine_view(&p2), Some(EngineView::Line { state: RegionState::Verified, .. })), "{:?}", g2.engine_view(&p2));
        assert!(!g2.chat(&p2).iter().any(|t| t.text == STOPPED_LINE), "済んだ run を止めたと言った");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPEN-INPLACE-1101 の 5 の試験の形: 案（`ASK`）を持つ Project の chat・Engine（pad）・AI の答え 4 つ（変更の読み = 読めない・interpret・generate・余り）。
    fn proposed(name: &str) -> (Generated, String, Arc<Fake>, World, std::path::PathBuf) {
        use crate::domain::chat::{Mark, Turn, Who};
        let dir = tmp(name);
        let w = World::default();
        *w.text.lock().unwrap() = "# README".into();
        let (mut g, project) = wired(Fake::new(vec![]), pad_registry(&w), &dir);
        let edit = json!({ "steps": [{ "verb": "unsupported", "name": ASK }] });
        let ops = json!({ "operations": [{ "kind": "act", "named": false, "candidates": [], "id": "op1", "want": "README に 1 行足して保存する", "success": "保存した文字に新しい行が在る", "target": null }] });
        let fake = Arc::new(Fake::new(vec![Ok(edit), Ok(ops.clone()), Ok(pad_answer("# README\nnonce-plan")), Ok(ops)]));
        g.set_llm(Ok(Box::new(Seen(fake.clone()))));
        let turn = Turn { who: Who::Shibubu, text: "I can do that".into(), mark: Mark::None, at: crate::runtime::now_rfc3339(), attachments: Vec::new(), choices: Vec::new(), proposal: Some(ASK.into()), preview: Vec::new(), sections: Vec::new(), needs: None };
        g.say_turn(&project, turn);
        (g, project, fake, w, dir)
    }

    /// 裏の model の job の答えが `done` の形で届くまで（最長 20 秒・混雑なら patient 倍）
    fn until_done(g: &mut Generated, done: fn(&ModelDone) -> bool, what: &str) {
        let end = std::time::Instant::now() + patient(Duration::from_secs(20));
        loop {
            if g.poll_model_job(|| None).as_ref().is_some_and(done) {
                return;
            }
            assert!(std::time::Instant::now() < end, "{what} が届かない");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn card_lines(g: &mut Generated, project: &str) -> Vec<String> {
        g.chat(project).last().map(|t| t.preview.clone()).unwrap_or_default()
    }

    /// OPEN-INPLACE-1101 の 5（owner 240「chat が約束した事と Engine の手が違う」・operator の決め 3）: Project の chat の案の Preview plan は Build now と
    /// 同じ読み（変更の読み → interpret → generate）を 1 回し、案の card に Engine の手の行（承認の card と同じ字）を置く・何も送らない。その後の
    /// Build now（`start_edit`）は AI を 1 回も呼ばずに同じ手の承認の card まで行く = Preview → Build の呼び出しは Build だけの時と同じ 3 回。
    /// 負の対照: `start_change_read` の覚えた読みの枝を外す → 4（変更の読みを 2 回）で赤・`CoreEngine::run` の `planned` を None にする → 4（interpret 2 回）で赤・
    /// `capability_engine::plan` の `save_workflow` を外す → 4（generate 2 回）で赤・`planned_read` の `preview_change` を外す → 行の assert が赤。
    #[test]
    fn open_inplace_preview_plan_lists_the_engines_steps_and_build_now_reuses_them() {
        use crate::domain::chat::PLAN_UNMADE;
        let (mut g, project, fake, w, dir) = proposed("plan-preview");
        let calls = || fake.requests.lock().unwrap().len();
        assert_eq!(g.preview_engine_plan(&project, ASK), Ok(()));
        until_done(&mut g, |d| matches!(d, ModelDone::Planned), "Preview の答え");
        let lines = card_lines(&mut g, &project);
        assert_eq!(lines.first().map(String::as_str), Some(crate::generated::PLAN_STEPS), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("nonce-plan")), "Engine の手の行が無い: {lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some(PLAN_UNMADE), "{lines:?}");
        assert!(w.writes().is_empty(), "Preview で送った: {:?}", w.writes());
        assert_eq!(calls(), 3, "Preview の呼び出し = 変更の読み・interpret・generate");
        // Build now（change_step の Accept と同じ道）
        assert!(g.start_edit(&project, ASK));
        until_done(&mut g, |d| matches!(d, ModelDone::Edited { .. }), "Build now の答え");
        settle(&mut g);
        let Some(EngineView::Approve { steps, .. }) = g.engine_view(&project).cloned() else { panic!("承認の card: {:?}", g.engine_view(&project)) };
        assert_eq!(steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), ["pad.text.set", "pad.save"], "Preview に見せた手と違う");
        assert_eq!(calls(), 3, "Build now で AI を呼び直した（Preview の読み・plan を使わない）");
        assert!(w.writes().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPEN-INPLACE-1101 の 5（operator の直し ①・owner 240「押したのに何も反応しない」）: Preview の読みの最中も案の card の Build now は押せて
    /// （`card_wait` = None）、押すと card に `BUILD_QUEUED`（Build now は灰・理由も同じ字）。読み終えたらそのまま同じ手の承認の card まで行く（AI は Preview の 3 回だけ）。
    /// 負の対照: `planned_read` の `build_queued` の枝を外す → 承認の card が来ずに赤・`card_wait` の previewing の腕を外す → 1 つ目の assert が赤・
    /// `build_when_read` の `preview_change` を外す → BUILD_QUEUED の行の assert が赤。
    #[test]
    fn open_inplace_build_now_pressed_while_the_plan_is_read_builds_once_it_is_read() {
        use crate::domain::chat::BUILD_QUEUED;
        let (mut g, project, fake, w, dir) = proposed("plan-queued");
        assert_eq!(g.preview_engine_plan(&project, ASK), Ok(()));
        assert_eq!(g.card_wait(&project), None, "Preview の読みの最中に Build now が灰");
        assert!(g.build_when_read(&project, ASK), "読みの最中の Build now を受けない");
        assert!(card_lines(&mut g, &project).iter().any(|l| l == BUILD_QUEUED), "押した事が card に出ない: {:?}", card_lines(&mut g, &project));
        assert_eq!(g.card_wait(&project), Some(BUILD_QUEUED), "押した後も Build now が押せる（2 回作る）");
        until_done(&mut g, |d| matches!(d, ModelDone::Planned), "Preview の答え");
        assert!(g.model_busy_line().is_some(), "読み終えても Build が始まらない");
        until_done(&mut g, |d| matches!(d, ModelDone::Edited { .. }), "Build now の答え");
        settle(&mut g);
        assert!(matches!(g.engine_view(&project), Some(EngineView::Approve { .. })), "{:?}", g.engine_view(&project));
        assert_eq!(fake.requests.lock().unwrap().len(), 3, "読み終えてからの Build で AI を呼び直した");
        assert_ne!(g.card_wait(&project), Some(BUILD_QUEUED), "作り始めた後も待ちの理由が残る");
        assert!(w.writes().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPEN-INPLACE-1101 の 5（operator の直し ②）: 同じ文・同じ Project の版の Preview の押し直しは前の読みの行を置き直すだけ（AI 0 回・job も起こさない）。
    /// 版が替わったら読み直す。
    /// 負の対照: `preview_engine_plan` の前の読みの枝を外す → 2 回目で job が始まり赤・`PlannedEdit::is` の hash を見ない → 版の替わった後に job が始まらず赤。
    #[test]
    fn open_inplace_preview_again_reuses_the_read_until_the_project_changes() {
        use crate::domain::patch::{AppPatch, PatchOp};
        use crate::domain::project::{ProjectPatchOp, RevisionOrigin};
        let (mut g, project, fake, _w, dir) = proposed("plan-again");
        assert_eq!(g.preview_engine_plan(&project, ASK), Ok(()));
        until_done(&mut g, |d| matches!(d, ModelDone::Planned), "Preview の答え");
        let first = card_lines(&mut g, &project);
        g.preview_change(&project, Vec::new()); // 開き直した card（行が消えて Preview plan が戻る）
        assert_eq!(g.preview_engine_plan(&project, ASK), Ok(()));
        assert!(g.model_busy_line().is_none(), "同じ文・同じ版で読み直した");
        assert_eq!(card_lines(&mut g, &project), first, "前の読みの行を置き直さない");
        assert_eq!(fake.requests.lock().unwrap().len(), 3, "押し直しで AI を呼んだ");
        // Project の版を替える（部品を 1 つ置く）→ 読み直す
        let app = project.strip_prefix("project.").expect("project.<app>").to_owned();
        let (p, hash) = g.project_of(&app).unwrap().expect("Project");
        let page = &p.pages[0];
        let node = json!({ "key": "part-notes", "component": "canvas", "label": "Notes", "props": { "html": "<p></p>" } });
        let op = ProjectPatchOp::Page { page_id: page.id.clone(), patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(node) } };
        g.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit).expect("部品を置けない");
        g.preview_change(&project, Vec::new());
        assert_eq!(g.preview_engine_plan(&project, ASK), Ok(()));
        assert!(g.model_busy_line().is_some(), "Project の版が替わったのに前の読みを使った");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// AI-COMPOSE-1101（owner 206）: 部品の `shibubu.read(app, what)` = `CoreEngine::read`。偽の desktop の Adapter（読む 3 手だけ・受けた手を残す）と `llm::fake::Fake`。
/// 負の対照: `read` の app の照合を外す（最初の app を読む）→ 「開いていない app」の assert が赤・`found` を見ない → 「窓に無い」の assert が赤。
#[cfg(test)]
mod ai_compose_read {
    use super::{CoreEngine, READ_NAME, READ_NEEDS_AI};
    use crate::adapters::sdk::{self, Adapter, Route, Spec};
    use crate::adapters::AdapterRegistry;
    use crate::capability_engine::EnvFacts;
    use crate::domain::action::Effect;
    use crate::domain::binding::Source;
    use crate::domain::capability::{AdapterKind, CapabilityReadiness};
    use crate::domain::run::{Observation, Outcome};
    use crate::generated::{PageEngine, ReadMiss};
    use crate::llm::fake::Fake;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    static DESK: [Spec; 3] = [
        Spec { id: "desktop.apps.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.windows.list", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        Spec { id: "desktop.window.state", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
    ];

    struct Desk(Arc<Mutex<Vec<(String, Value)>>>);
    impl Adapter for Desk {
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
            &DESK
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
            self.0.lock().unwrap().push((action.into(), input.clone()));
            let payload = match action {
                "desktop.apps.list" => json!([{ "pid": 7, "name": "GarageBand", "bundle_id": "com.apple.garageband10" }, { "pid": 8, "name": "Finder", "bundle_id": "com.apple.finder" }]),
                "desktop.windows.list" => json!([{ "pid": 7, "window_id": 41, "app_name": "GarageBand", "title": "Song", "bundle_id": "com.apple.garageband10", "document": "" }]),
                _ => json!({ "pid": 7, "window_id": 41, "app_name": "GarageBand", "complete": true, "controls": [
                    { "label": "Acoustic Guitar", "role": "AXStaticText", "value": null },
                    { "label": "", "role": "AXGroup", "value": null },
                    { "label": "Clean Electric", "role": "AXStaticText", "value": null },
                ] }),
            };
            (Outcome::Verified { evidence: vec!["desk".into()] }, Some(sdk::observe("desktop", sdk::now(), payload)))
        }
        fn cleanup(&mut self) {}
    }

    #[test]
    fn ai_compose_read_reads_the_named_apps_window_only_and_shapes_it_once() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let dir = std::env::temp_dir().join(format!("shibubu-ai-compose-read-{}", std::process::id()));
        let core = CoreEngine {
            registry: Arc::new(move || {
                let mut reg = AdapterRegistry::default();
                reg.register(Box::new(Desk(seen.clone()))).unwrap();
                reg
            }),
            data_dir: dir.clone(),
            mcp: Default::default(),
            env: EnvFacts { os_version: "test".into(), ..Default::default() },
            readback: Duration::ZERO,
            act_consents: dir.join("consents"),
        };
        let what = "guitar sound names as a JSON array of strings";
        let fake = Arc::new(Fake::new(vec![Ok(json!({ "found": true, "json": "[\"Acoustic Guitar\", \"Clean Electric\"]", "looked_for": "guitar sounds", "shows": "", "next": "" }))]));
        let got = core.read("garageband", what, Some(fake.clone() as Arc<dyn crate::llm::Llm>));
        assert_eq!(got, Ok(json!(["Acoustic Guitar", "Clean Electric"])));
        let acts: Vec<String> = calls.lock().unwrap().iter().map(|(a, _)| a.clone()).collect();
        assert_eq!(acts, ["desktop.apps.list", "desktop.windows.list", "desktop.window.state"], "読む 3 手の他を呼んだ");
        assert_eq!(calls.lock().unwrap()[1].1["bundle_id"], "com.apple.garageband10", "名指しの app の窓を読まない");
        let reqs = fake.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1, "AI は 1 回");
        assert!(reqs[0].name == READ_NAME && reqs[0].input.contains("Acoustic Guitar") && reqs[0].input.contains(what) && !reqs[0].input.contains("AXGroup"), "{}", reqs[0].input);
        drop(reqs);
        // 開いていない app・AI が無い・窓に無い（当てない）
        let none = Arc::new(Fake::new(vec![]));
        assert!(core.read("Logic Pro", what, Some(none.clone() as Arc<dyn crate::llm::Llm>)).is_err_and(|e| e.line.contains("Logic Pro isn't open")));
        assert!(none.requests.lock().unwrap().is_empty(), "開いていない app で AI を呼んだ");
        assert_eq!(core.read("GarageBand", what, None), Err(READ_NEEDS_AI.into()));
        // TALK-HONEST-1101 追記 232: 窓に無い時の 1 行 = 何を探して・窓に何が見えていたか・次の一手 1 つ（負の対照: `shows` を文に入れない → 赤）
        // ＋ operator の決め: 文の後ろは「{next}, or let Shibubu open it」・開ける一言（`open`）は探した物と app の名前（data）から（AI が無い時は持たない）
        let missing = Arc::new(Fake::new(vec![Ok(json!({ "found": false, "json": "[\"Steel\"]", "looked_for": "the guitar list", "shows": "Drummer", "next": "Select a software instrument track." }))]));
        let miss = |line: &str, open: &str| -> Result<Value, ReadMiss> { Err(ReadMiss { line: line.into(), open: Some(open.into()) }) };
        assert_eq!(
            core.read("GarageBand", what, Some(missing as Arc<dyn crate::llm::Llm>)),
            miss("Looked for the guitar list in GarageBand, but its window shows Drummer now · Select a software instrument track, or let Shibubu open it", "Open the guitar list in GarageBand"),
            "窓に無い物を当てた・何が見えていたかと次の一手を言わない・開ける一言が無い"
        );
        assert_eq!(
            core.read("Logic Pro", what, Some(none as Arc<dyn crate::llm::Llm>)),
            miss("Looked for guitar sound names in Logic Pro, but Logic Pro isn't open · Open Logic Pro, or let Shibubu open it", "Open guitar sound names in Logic Pro")
        );
        assert_eq!(core.read("GarageBand", what, None).map_err(|m| m.open), Err(None), "AI が無いのに開ける一言を持った");
    }
}
