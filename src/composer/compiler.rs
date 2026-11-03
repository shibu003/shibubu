//! Compiler: Reading + Blueprint + Scene → AppDefinitionV3。
//! state は action / source の実際の readiness から決める（README 規則 2・V14。接続していない物を ready と言わない）。
//! metadata.composer に解釈・GoalPlan・案と落とした理由・node ごとの理由・errors を残す（GEN-010 の最小: なぜその部品が在るか）。
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::domain::action::ActionReadiness;
use super::domain::app::{AgentPolicy, AppDefinitionV3, AppReadiness, AppState, AutonomyLevel, Composition, ExecutionMode, Goal, SchemaVersion, ViewMode};
use super::domain::binding::{Source, SourceReadiness};
use super::goal::{GoalPlan, Reading};
use super::layout::Scene;
use super::planner::{Blueprint, Rejected, Solution};

/// README 規則 2 / V14 と同じ式: ready = 全部 ready、partial = ready と未 ready が混在、blocked = ready が 0。
pub fn state_for(actions: &[super::domain::action::ActionDefinition], sources: &[Source]) -> AppState {
    let ready = actions.iter().filter(|a| a.readiness == ActionReadiness::Ready).count();
    let waiting = actions.len() - ready;
    let src_waiting = sources.iter().filter(|s| !matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready)).count();
    let integrated = sources.iter().any(|s| s.readiness == SourceReadiness::Ready);
    let readiness = if ready == 0 {
        AppReadiness::Blocked
    } else if waiting == 0 && src_waiting == 0 {
        AppReadiness::Ready
    } else {
        AppReadiness::Partial
    };
    // DOGFOOD-X 周 7 #4（RULE-027・画面は English だけ）: Page の上に出る 1 行
    let status_message = match readiness {
        AppReadiness::Ready if integrated => "Works with the connected sources and on this Mac".to_string(),
        AppReadiness::Ready => "Works on this Mac · no connection needed".to_string(),
        // 待っているのが取得元だけ（操作は 0）の時に「0 actions」と言わない
        AppReadiness::Partial if waiting == 0 => "The parts on this Mac work · the rest waits until a source is connected and allowed".to_string(),
        AppReadiness::Partial if waiting == 1 => "The parts on this Mac work · 1 action waits until a source is connected and allowed".to_string(),
        AppReadiness::Partial => format!("The parts on this Mac work · {waiting} actions wait until a source is connected and allowed"),
        _ => "Nothing works yet · connect a source to use it".to_string(),
    };
    AppState {
        readiness,
        execution_mode: if integrated { ExecutionMode::Integrated } else { ExecutionMode::LocalReady },
        status_message: Some(status_message),
    }
}

#[derive(Serialize)]
struct Provenance<'a> {
    interpreter: &'a str,
    job: Value,
    subject: &'a str,
    cadence: Value,
    confidence: f64,
    evidence: &'a [String],
    unknowns: &'a [String],
    strategy: Value,
    solutions: &'a [Solution],
    rejected: &'a [Rejected],
    discovery: Value,
    goal_plan: &'a GoalPlan,
    rationale: Map<String, Value>,
    errors: &'a [String],
}

pub struct Parts<'a> {
    pub reading: &'a Reading,
    pub blueprint: &'a Blueprint,
    pub scene: &'a Scene,
    pub goal_plan: &'a GoalPlan,
    pub solutions: &'a [Solution],
    pub rejected: &'a [Rejected],
}

pub fn assemble(p: &Parts) -> AppDefinitionV3 {
    let (r, bp) = (p.reading, p.blueprint);
    let state = state_for(&bp.actions, &bp.sources);
    let external = !r.externals.is_empty();
    let mut def = AppDefinitionV3 {
        schema_version: SchemaVersion::V3,
        id: bp.app_id.clone(),
        version: 1,
        title: Some(r.title.clone()),
        goal: Goal { literal_goal: r.literal.clone(), outcomes: r.outcomes.clone(), assumptions: r.assumptions.clone(), constraints: r.constraints.clone() },
        composition: Composition { root: p.scene.root.clone(), view_modes: vec![ViewMode::Use, ViewMode::Build, ViewMode::Review] },
        capabilities: bp.capabilities.clone(),
        actions: bp.actions.clone(),
        sources: bp.sources.clone(),
        workflows: bp.workflows.clone(),
        agent: AgentPolicy {
            suggestions_enabled: true,
            autonomy_level: if external { AutonomyLevel::AskBeforeExternalEffects } else { AutonomyLevel::SuggestOnly },
            entrypoint: "agent_rail".into(),
            policy_scope: vec!["local".into()],
        },
        state,
        metadata: None,
    };
    set_provenance(&mut def, p, "rule_based", &[]);
    def
}

/// metadata.composer を書き直す（model の案を採った時も同じ形）。node ごとの理由は、既に書いてある理由（model の案が自分の node に
/// 書いた物）を規則の理由より優先して残す。消すと、理由つきで検査を通った chart が理由なしになる（V18）。
pub fn set_provenance(def: &mut AppDefinitionV3, p: &Parts, interpreter: &str, errors: &[String]) {
    let mut rationale: Map<String, Value> = p.scene.rationale.iter().filter(|(_, w)| !w.is_empty()).map(|(k, w)| (k.clone(), json!(w))).collect();
    if let Some(had) = def.metadata.as_ref().and_then(|m| m.get("composer")).and_then(|c| c.get("rationale")).and_then(Value::as_object) {
        rationale.extend(had.iter().filter(|(_, w)| w.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)).map(|(k, w)| (k.clone(), w.clone())));
    }
    let prov = Provenance {
        interpreter,
        job: json!(p.reading.job),
        subject: &p.reading.subject,
        cadence: json!(p.reading.cadence),
        confidence: p.reading.confidence,
        evidence: &p.reading.evidence,
        unknowns: &p.reading.unknowns,
        strategy: json!(p.blueprint.strategy),
        solutions: p.solutions,
        rejected: p.rejected,
        discovery: json!(p.blueprint.discovery),
        goal_plan: p.goal_plan,
        rationale,
        errors,
    };
    let meta = def.metadata.get_or_insert_with(Map::new);
    meta.insert("composer".into(), serde_json::to_value(prov).unwrap_or(Value::Null));
}
