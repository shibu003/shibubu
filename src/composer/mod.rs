//! composer（unit C）: 一言の目的 → 動く AppDefinitionV3（DESIGN §0-2: Goal → Capability → Data/Action → UI → Verification）。
//!
//! `compose(goal, ctx)` = interpret（goal.rs: job × 物 × 修飾。固定 template を選ばない）→ planner（2〜3 案から選ぶ・
//! data と action を UI より先に）→ layout（DESIGN §7）→ compiler（AppDefinitionV3）→ verifier（未完成を ready にしない）。
//! `ctx.model` が在る時だけ model に案を頼み、返った JSON が下書きの外（id・接続先・local でない action）を足した・変えた・消したか、
//! domain の検査と BuildVerifier を通らなければ捨てて rule-based を使う（GEN-012）。
//! 編集は patch.rs の `edit` / `apply_edit` / `undo`（GEN-011 / GEN-014）。
//!
//! bin crate の module だが tests/ から `#[path]` で取り込むので、中で `crate::` を書かない（domain は `super::domain`）。
#![allow(dead_code)] // 統合役が app に配線するまで main から呼ばれない

pub mod capabilities;
pub mod compiler;
pub mod goal;
pub mod layout;
pub mod patch;
pub mod planner;
pub mod project;
pub mod verifier;

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;

use super::domain;
use domain::action::ActionDefinition;
use domain::app::AppDefinitionV3;
use domain::capability::CapabilityDescriptor;
use domain::validate::{validate, validate_generated};

pub use goal::{validate_plan, GoalPlan};
#[allow(unused_imports)] // 統合役と tests が使う口
pub use patch::{apply_edit, edit, undo, Edit, EditError};
pub use verifier::{verify, Verdict};

/// 外部 model の口（GEN-012）。実装は `src/llm/`（pane p-google）で、統合役が繋ぐ。既定は無し（`ComposeContext::model = None`）。
/// 返すのは構造化 JSON（`schema` は agent_native_app_v3.schema.json）。composer は返った物を信用せず、検査を通った時だけ使う。
/// W13: 製品の実装は `generated.rs` が `llm::Llm`（利用者の CLI = `llm_cli::CliLlm`）を包んだ物。`schema` の `title` を `JsonRequest::name` に使う
/// （`project::plan_schema` / `edit_schema` は title を持つ）。Err の String は画面に出せる 1 行（`LlmError` の Display）。
pub trait ModelProvider {
    fn complete_json(&self, prompt: &str, schema: &Value) -> Result<Value, String>;
}

#[derive(Default)]
pub struct ComposeContext<'a> {
    /// 生成する app の id（無ければ `generated.<物>_<job>` を決定的に作る）
    pub app_id: Option<String>,
    /// Connections が知っている capability（id の完全一致で引く。空 = 何も繋いでいない）
    pub registry: Vec<CapabilityDescriptor>,
    /// 既定 off（DESIGN §12.3: 合意した設定なしに外部 provider へ送らない）
    pub model: Option<&'a dyn ModelProvider>,
    /// TALK-1101（owner 191 通目「全て任せてすぐに作れる」）: Home の会話で答えずに残った問い（`chat::open_question`）。`plan_project` だけが読み、
    /// model に「この問いはあなたが決めて作り、選んだ答えを `preview.picked` に書く」と渡す。None = 問いが無い・会話の外の一言
    pub unanswered: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComposeError {
    Empty,
    /// 目的そのものが読めない。plan に blocking な question が 1 つ（CD-05 規則 6）
    Clarify(Box<GoalPlan>),
    /// 今ある app への変更（edit を使う）
    EditRequest(String),
    /// 生成物が検査に落ちた（composer の bug。何が落ちたかを返す）
    Invalid(Vec<String>),
    /// W13: model が答えなかった・答えが Plan の決まりに合わない（`project::plan_project`）。String は画面に出せる 1 行の理由。
    /// 何も書いていない。呼ぶ側は model 無しの生成（`compose_project` の名前の表 → `compose`）で続け、この理由を 1 行出す
    Model(String),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "目的が空"),
            Self::Clarify(p) => write!(f, "目的を 1 つ決める: {}", p.questions.first().map(|q| q.text.as_str()).unwrap_or("")),
            Self::EditRequest(s) => write!(f, "'{s}' は今ある app への変更（edit を使う）"),
            Self::Invalid(errs) => write!(f, "生成物が検査に落ちた: {}", errs.join(" | ")),
            Self::Model(why) => write!(f, "AI を使えなかった: {why}"),
        }
    }
}
impl std::error::Error for ComposeError {}

/// 点検できる Build Plan（GEN-002: なぜこの形か）。`into_app` で検査済みの AppDefinitionV3 を渡す。
#[derive(Debug, Clone, Serialize)]
pub struct BuildPlan {
    pub goal_plan: GoalPlan,
    pub reading: goal::Reading,
    pub solutions: Vec<planner::Solution>,
    pub chosen: usize,
    pub rejected: Vec<planner::Rejected>,
    pub blueprint: planner::Blueprint,
    pub verdict: Verdict,
    /// "rule_based" か "model"
    pub interpreter: String,
    /// model の案を捨てた理由など（GEN-010）
    pub errors: Vec<String>,
    app: AppDefinitionV3,
}

impl BuildPlan {
    pub fn app(&self) -> &AppDefinitionV3 {
        &self.app
    }
    pub fn into_app(self) -> AppDefinitionV3 {
        self.app
    }
}

const APP_SCHEMA: &str = include_str!("../../docs/schemas/agent_native_app_v3.schema.json");

/// 唯一の入口。決定的（時刻・乱数を使わない）。
pub fn compose(goal: &str, ctx: &ComposeContext) -> Result<BuildPlan, ComposeError> {
    let reading = match goal::interpret(goal) {
        goal::Interpretation::Goal(r) => *r,
        goal::Interpretation::Unclear(plan) => return Err(ComposeError::Clarify(plan)),
        goal::Interpretation::Edit => return Err(ComposeError::EditRequest(goal.trim().into())),
        goal::Interpretation::Empty => return Err(ComposeError::Empty),
    };
    let solutions = planner::solutions(&reading);
    let (chosen, rejected) = planner::choose(&solutions);
    let app_id = ctx.app_id.clone().unwrap_or_else(|| planner::app_id(&reading));
    let bp = planner::blueprint(&reading, &ctx.registry, app_id, solutions[chosen].strategy);
    let scene = layout::scene(&reading, &bp);
    let goal_plan = planner::goal_plan(&reading, &bp, scene.intents.clone());
    validate_plan(&goal_plan).map_err(|errs| ComposeError::Invalid(errs.iter().map(ToString::to_string).collect()))?;
    let parts = compiler::Parts { reading: &reading, blueprint: &bp, scene: &scene, goal_plan: &goal_plan, solutions: &solutions, rejected: &rejected };
    let mut app = compiler::assemble(&parts);
    let mut verdict = verify(&app, &ctx.registry);
    let blockers = verdict.blockers();
    if !blockers.is_empty() {
        return Err(ComposeError::Invalid(blockers.iter().map(|f| format!("{} {}: {}", f.check, f.subject, f.message)).collect()));
    }
    let mut errors = Vec::new();
    let mut interpreter = "rule_based";
    if let Some(model) = ctx.model {
        match model_draft(model, &app, &parts, &ctx.registry) {
            Ok((m, v)) => {
                app = m;
                verdict = v;
                interpreter = "model";
            }
            Err(e) => {
                errors.push(e);
                // 捨てた理由を残す。返す app と verdict を取った app を同じ物にする
                compiler::set_provenance(&mut app, &parts, interpreter, &errors);
                verdict = verify(&app, &ctx.registry);
            }
        }
    }
    Ok(BuildPlan { goal_plan, reading, solutions, chosen, rejected, blueprint: bp, verdict, interpreter: interpreter.into(), errors, app })
}

/// model に案を頼む。採るのは (1) AppDefinitionV3 として読めて (2) 下書きの外を変えておらず（`drift_from_draft`）
/// (3) 理由（metadata.composer）を書いた後の app が domain の validate / validate_generated を通り (4) BuildVerifier の Blocker が 0 の物だけ。
/// 検査するのは返す app そのもの（検査の後で metadata を書き換えない）。どれかで落ちたら理由を返し、呼ぶ側は rule-based の app を使う。
fn model_draft(model: &dyn ModelProvider, base: &AppDefinitionV3, parts: &compiler::Parts, registry: &[CapabilityDescriptor]) -> Result<(AppDefinitionV3, Verdict), String> {
    let schema: Value = serde_json::from_str(APP_SCHEMA).map_err(|e| format!("schema を読めない: {e}"))?;
    // mic の録音の様に組み込みの能力を通る verb は、local の action として足させない（verifier が capability を外した物を止める）
    let verbs: Vec<&str> = capabilities::LOCAL_VERBS.iter().map(|v| v.verb).filter(|v| !capabilities::GATED_VERBS.iter().any(|(g, _)| g == v)).collect();
    let fixed: Vec<&str> = base.actions.iter().filter(|a| !is_local(a, base)).map(|a| a.id.as_str()).collect();
    let prompt = format!(
        "利用者の目的: {goal}\n\
         下の AppDefinition v3 は規則どおりに作った下書き。目的により合うように直した 1 つの AppDefinition v3 を JSON だけで返す。\n\
         - 直してよいのは画面（composition）・題名・local の action だけ。id・version・goal.literal_goal・agent・workflows・capabilities・sources と、local でない action {fixed:?} は下書きのまま（足す・変える・消すのどれをした案も捨てる）。\n\
         - local の action id は local.<source>.<verb>。source は下書きの物、verb は次だけ: {verbs:?}（update_ は update_<欄名>）。削除は requires_approval: true。\n\
         - KPI / metric の部品を置かない。chart を置くなら metadata.composer.rationale にその node の key で理由を書く。\n\
         下書き:\n{draft}",
        goal = parts.reading.literal,
        draft = serde_json::to_string(base).unwrap_or_default(),
    );
    let reply = model.complete_json(&prompt, &schema).map_err(|e| format!("model: {e}"))?;
    let mut app: AppDefinitionV3 = serde_json::from_value(reply).map_err(|e| format!("model の返答は AppDefinition v3 として読めない: {e}"))?;
    let drift = drift_from_draft(&app, base);
    if !drift.is_empty() {
        return Err(format!("model の返答は下書きの外を変えた（幻覚の id・変えてはいけない所）: {}", drift.join(", ")));
    }
    compiler::set_provenance(&mut app, parts, "model", &[]);
    if let Err(errs) = validate(&app).and_then(|_| validate_generated(&app)) {
        return Err(format!("model の返答は検査に落ちた: {}", errs.iter().map(ToString::to_string).collect::<Vec<_>>().join(" | ")));
    }
    let v = verify(&app, registry);
    let blockers = v.blockers();
    if !blockers.is_empty() {
        return Err(format!("model の返答は BuildVerifier に落ちた: {}", blockers.iter().map(|f| format!("{} {}", f.subject, f.message)).collect::<Vec<_>>().join(" | ")));
    }
    Ok((app, v))
}

/// local の action: capability を通らず外へ出ず、scope を要らず、`local.<source>.<verb>` の source がこの app の local source・verb が LOCAL_VERBS。
/// scope を持つ物は local と見ない（案が required_scopes を足すと app の宣言 scope が広がり、その grant と提案が通る・SEC-C3-01）。
/// model が足し引きしてよいのはこれだけ（effect と承認は verifier が LOCAL_VERBS と突き合わせる）。組み込みの namespace（mic の audio・
/// Connections の connection）は source を持たないので入らない: runtime の物で、model は足すことも変えることも消すこともできない。
fn is_local(a: &ActionDefinition, app: &AppDefinitionV3) -> bool {
    a.capability_ref.is_none()
        && !a.effect.is_external()
        && a.required_scopes.is_empty()
        && capabilities::split_local_id(&a.id).map(|(ns, verb)| capabilities::local_verb(verb).is_some() && app.source(&format!("local.{ns}")).is_some()).unwrap_or(false)
}

/// model が変えてはいけない所（GEN-012・CD-01）を下書きと比べ、外れた所を返す（空 = 採ってよい）。
/// 何の app か（id・version・利用者の文）・agent の権限・workflows（承認の形）・接続先（capability・source）・local でない action（effect・承認・
/// capability）は下書きのまま: 足さない・変えない・消さない（消した案を採ると、目的に要る取得元を落とした app が「接続は要らない」Ready になる）。
/// action が local かどうかは下書きの定義で決める（案の側で決めると、capability_ref を外すだけで mic の録音が照合を抜ける）。
/// 下書きに無い id は幻覚（registry に在っても、この目的に discover した物でなければ足させない）。
fn drift_from_draft(app: &AppDefinitionV3, base: &AppDefinitionV3) -> Vec<String> {
    let mut out = Vec::new();
    if app.id != base.id {
        out.push(format!("app id '{}'（下書きは '{}'）", app.id, base.id));
    }
    if app.version != base.version {
        out.push(format!("version {}（下書きは {}）", app.version, base.version));
    }
    if app.goal.literal_goal != base.goal.literal_goal {
        out.push("goal.literal_goal（利用者の文）".into());
    }
    if app.agent != base.agent {
        out.push("agent の権限".into());
    }
    if app.workflows != base.workflows {
        out.push("workflows（承認の形）".into());
    }
    let mut check = |kind: &str, id: &str, same: Option<bool>| match same {
        None => out.push(format!("{kind} '{id}' は下書きに無い")),
        Some(false) => out.push(format!("{kind} '{id}' が下書きと違う")),
        Some(true) => {}
    };
    for c in &app.capabilities {
        check("capability", &c.id, base.capability(&c.id).map(|b| b == c));
    }
    for s in &app.sources {
        check("source", &s.id, base.source(&s.id).map(|b| b == s));
    }
    for a in &app.actions {
        let b = base.action(&a.id);
        if !(is_local(a, base) && b.map_or(true, |b| is_local(b, base))) {
            check("action", &a.id, b.map(|b| b == a));
        }
    }
    // 逆向き: 下書きに在る接続先と local でない action を、案が消していないか
    for c in base.capabilities.iter().filter(|c| app.capability(&c.id).is_none()) {
        out.push(format!("capability '{}' を消した", c.id));
    }
    for s in base.sources.iter().filter(|s| app.source(&s.id).is_none()) {
        out.push(format!("source '{}' を消した", s.id));
    }
    for b in base.actions.iter().filter(|b| !is_local(b, base) && app.action(&b.id).is_none()) {
        out.push(format!("action '{}' を消した", b.id));
    }
    out
}

/// test・診断用: 生成物の「形」（component の集合）。
pub fn component_set(app: &AppDefinitionV3) -> BTreeSet<String> {
    app.composition.root.nodes().into_iter().map(|n| n.component.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::domain::action::ActionReadiness;
    use super::{compose, drift_from_draft, is_local, ComposeContext};

    /// C-R2-01: 下書きで capability を通る action は、id が自分の source の下（`local.<source>.<verb>`）でも、案が capability を外せば照合に掛かる
    /// （組み込みの namespace を local から外しただけでは塞がらない道）。
    #[test]
    fn r2_01b_locality_is_decided_by_the_draft_definition() {
        let mut base = compose("鼻歌を録音したい", &ComposeContext::default()).unwrap().into_app();
        base.actions.iter_mut().find(|a| a.id == "local.audio.record").expect("前提: 録音").id = "local.recordings.record".into();
        let mut app = base.clone();
        let rec = app.actions.iter_mut().find(|a| a.id == "local.recordings.record").unwrap();
        rec.capability_ref = None;
        rec.readiness = ActionReadiness::Ready;
        let drift = drift_from_draft(&app, &base);
        assert!(drift.iter().any(|d| d.contains("local.recordings.record")), "{drift:?}");
        assert!(drift_from_draft(&base, &base).is_empty());
    }

    /// SEC-C3-01: 案が local action に required_scopes（mail.send）を足すと、app の宣言 scope（declared_scopes）が広がり、その scope の
    /// grant と agent の提案が通るようになる。照合を飛ばしてよい local action は scope を持たない物だけ。
    #[test]
    fn sec_c3_01_a_local_action_that_gains_scopes_is_checked_against_the_draft() {
        let base = compose("在庫を整理したい", &ComposeContext::default()).unwrap().into_app();
        let mut app = base.clone();
        let a = app.actions.iter_mut().find(|a| is_local(a, &base)).expect("前提: local action");
        a.required_scopes = vec!["mail.send".into()];
        let id = a.id.clone();
        let drift = drift_from_draft(&app, &base);
        assert!(drift.iter().any(|d| d.contains(&id)), "local action に足した scope を照合しなかった: {drift:?}");
    }
}
