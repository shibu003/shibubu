//! W19-2b: 複数の service の能力を組み合わせた Workflow（owner 19 通目 §7 W19-2b・§1「異なるソフトウェアの機能を組み合わせて新しい作業環境を構成する」）
use super::*;

/// `Workflow::schema` の値。
/// Wave: W19-2b
pub const WORKFLOW_SCHEMA: &str = "shibubu.workflow/1";
/// 利用者の data dir の下の置き場（`<id>.json`・再起動の後も使える）。
/// Wave: W19-2b
pub const WORKFLOWS_DIR: &str = "workflows";

/// 異なる app・service の能力をつないだ作業の手順（Engine が生成し、利用者の data に残す）。生成した UI は Project（Page と面）で、`ui` がその id。
/// Wave: W19-2b
/// 例: `Workflow { proven: None, fails: 0, steps: vec![WorkflowStep { kind: StepKind::Action, id: "read".into(), capability: "git.status".into(), input: json!({"uri": repo}) },
///       WorkflowStep { kind: StepKind::Action, id: "post".into(), capability: "<service>.comment".into(), input: json!({"body": {"$from": "read.payload.summary"}}) }], .. }`
/// 保証する: 未知の欄は拒否・step の input の `{"$from": "<step id>.<path>"}` は前の step の出力だけを指す（後ろ・自分・無い step は `validate` が拒否）・
/// `checks` の input は step を指せない（操作の前にも読むため）。
/// 保証しない: 各 service が今も使える事（各 step の capability の readiness と registry の gate）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub schema: String,
    pub id: String,
    /// 利用者の目的（Engine の記録の goal と同じ）
    pub goal: String,
    pub steps: Vec<WorkflowStep>,
    /// 手順の持ち主の Project の id（`project.<app>`）。Engine の run が保存する時に呼んだ Project を入れ（RQ3）、Import は取り込んだ先の Project に替える。
    /// Project の Export はこの値で引く。Engine の外で作った手順は None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<String>,
    /// W19-2: 成功の確かめ方（各 service の実状態を読む action と条件・Execution の前と後に読む）。Engine が生成した手順は 1 つ以上持ち、再利用の時も同じ物で確かめる
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<ReadBack>,
    /// WF-E x6c: AI がこの目的を 2 つ以上の操作に分けた run の、分けた操作の id の一覧（この手順の操作も含む）。model 無しの道（目的をそのまま 1 つの操作）が、
    /// 保存が 1 つ目だけで止まった目的を 1 つ目だけ走らせて済みにしないため（`run_stages` の model 無しの門が数える）。空 = 1 つの操作か前の版の保存（今の振る舞い）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split: Vec<String>,
    /// ENGINE-AUDIT W2（節 2「確かめていない計画を覚え、失敗しても捨てない」・owner 211 §2「確かめた手は data」）: 読み戻しが通った時の印（いつ・何を確かめたかの 1 行）。
    /// None = 下書き（Verification の段と `plan` が保存した・まだ本物で通っていない）。本体: 再利用の前に最初の押す名前を今の木で照らし、proven なら interpret を飛ばす
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proven: Option<String>,
    /// W2: 本物で失敗した回数（手が見つからない・読み戻しが合わない・硬い拒否）。1 以上は再利用せず、理由を `rejected_plan` に入れて作り直す（承認待ち・人の Stop・判断は数えない）
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub fails: u32,
}

fn u32_is_zero(n: &u32) -> bool {
    *n == 0
}

/// Workflow の 1 手（registry の action を 1 回呼ぶ・WF0 から人が選ぶ手も）。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    pub id: String,
    /// registry の action id（`CapabilityRecord::id`）。`StepKind::Decide` の手は空
    #[serde(default)]
    pub capability: String,
    /// 入力。前の step の出力は `{"$from": "<step id>.<path>"}`（path は Observation の payload の中の `.` 区切り・配列は番号）
    #[serde(default)]
    pub input: serde_json::Value,
    /// WF0: 手の種類（既定 = `Action` = 今の 1 手・既定の時は書き出さないので、旧い手順の file はそのまま読めて、書き直しても同じ byte）
    #[serde(default, skip_serializing_if = "StepKind::is_action")]
    pub kind: StepKind,
}

/// WF0: 手の種類（review §1 の `kind`・O4 §5①「既存のAction Stepをそのまま残し、制御フローを追加情報として持たせる」）。
/// Wave: WF0
/// 例: `WorkflowStep { id: "pick".into(), capability: String::new(), input: Value::Null, kind: StepKind::Decide { question: "Which title?".into(), choices } }`
/// 保証する: `Decide` で run は `waiting_human` に止まり、選んだ choice の `value`（`$from` を前の手の出力で埋めた物）が `outputs["<手>"]`
/// = `{"choice": id, "value": …}` に入って後ろの手が `$from: "<手>.value"` で使う（選ばなかった choice の値はどの手にも渡らない）。
/// 保証しない: 道ごとに別の手を走らせる分岐（`when`・WF3）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepKind {
    /// registry の action を 1 回呼ぶ（今の 1 手）
    #[default]
    Action,
    /// 人が選ぶ（approval / consent の receipt は作らない・送る手の承認は今まで通り Approve）
    Decide { question: String, choices: Vec<DecideChoice> },
}

impl StepKind {
    pub fn is_action(&self) -> bool {
        matches!(self, Self::Action)
    }
}

/// WF0: `StepKind::Decide` の選択肢 1 つ。
/// Wave: WF0
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideChoice {
    pub id: String,
    /// button の名前（AX の名前も同じ・人の言葉）
    pub label: String,
    /// 選んだ時に後ろの手へ渡す値 = card に出す証拠（`{"$from": "<前の手>.<path>"}` は前の手の出力で埋める・WF-014 の差分・試験の結果）
    #[serde(default)]
    pub value: Value,
    /// 1 行の得失（card に値の上に出す）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// WF0: Decide の card に出す物（`$from` を埋めた選択肢）。run の記録（runs 表）に残し、再起動の後も同じ card を出す。
/// Wave: WF0
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// 待っている手の id
    pub step: String,
    pub question: String,
    pub choices: Vec<DecideChoice>,
}

impl WorkflowStep {
    /// registry の action を呼ぶ手か（Decide の手は readiness・承認・読む / 書くの数に入れない）。
    pub fn is_action(&self) -> bool {
        self.kind.is_action()
    }
}

/// WF0: 待っている Decide の手の card（choice の `value` の `$from` を `outputs` で埋める・`step` が Decide でなければ Err）。
/// Wave: WF0
pub fn decision(wf: &Workflow, step: &str, outputs: &HashMap<String, Value>) -> Result<Decision, String> {
    let s = wf.steps.iter().find(|s| s.id == step).ok_or_else(|| format!("手順に「{step}」がありません"))?;
    let StepKind::Decide { question, choices } = &s.kind else { return Err(format!("手順「{step}」は判断の手ではありません")) };
    let choices = choices.iter().map(|c| Ok(DecideChoice { value: resolve(&c.value, outputs, step, &c.id, &mut Vec::new())?, ..c.clone() })).collect::<Result<_, String>>()?;
    Ok(Decision { step: step.into(), question: question.clone(), choices })
}

/// service の間で受け渡した data 1 つの記録（何を・どこから・どこへ・中身の digest）。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub from_step: String,
    pub to_step: String,
    /// 受け取った側の input の欄（入れ子は `.` 区切り）
    pub field: String,
    /// 渡した値の sha256（値そのものは記録に入れない・秘密を残さない）
    pub digest: String,
}

/// 1 手の結果（registry の `invoke` の Outcome と、読んだ Observation の id）。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReceipt {
    pub step: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
    #[serde(default)]
    pub handoffs: Vec<Handoff>,
}

/// Workflow を 1 回走らせた記録（各 service の実状態の読み戻しは step の Outcome と試験の側の oracle）。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReceipt {
    pub workflow: String,
    pub steps: Vec<StepReceipt>,
}

/// Workflow の誤り。Display は画面に出せる 1 行。
/// Wave: W19-2b
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowError {
    /// 形が違う（`$from` が前の step を指さない・id の重なり・step 0 個）
    Invalid(String),
    /// step の能力が registry に無い・Ready でない（どの step も走らせていない）
    Missing { step: String, capability: String },
    /// W19-2: registry の gate が止めた（承認が無い・合わない・人の lease・登録や readiness）。この step は送っていない（前の step は走った）
    Rejected { step: String, why: String },
    /// step が失敗した（後ろの step は走らせない）
    Step { step: String, why: String },
    /// G74: 書く手 `step` を送る直前に、前の書く手 `after` が置いた物を読み直して確かめられなかった（`AdapterRegistry::recheck`）。
    /// `step` とその後ろは送っていない（`after` までは走った）
    Held { step: String, after: String, why: String },
    /// 利用者の data（workflows）を読めない・書けない
    Store(String),
    /// WF0: `StepKind::Decide` の手 `step` で人の判断を待つ（前の手は走った・`step` と後ろは走らせていない）
    Waiting { step: String },
    /// WF-006: 外へ効く手 `step` の Adapter が、送る直前に宛先を読み直すと宣言していない（`Adapter::prechecks`）。どの手も走らせていない
    /// （`run_steps` の最初の検査・`Missing` と同じ所）
    /// Wave: WF-E
    Unchecked { step: String, why: String },
    /// B76: 読む手 `step`（`SAVED_READ`）の `$from` を埋めた後の uri が Project の repo の外（`outside_repo`）。`step` と後ろは走らせていない（前の手は走った）
    Outside { step: String },
    /// ENGINE-AUDIT W5: 手 `step` の押す名前 `label` が今の木に無い（`ambiguous` = false・Adapter の `Failed { TargetNotFound }`）か 1 つに絞れない（true・`TargetAmbiguous`）。
    /// その手は送っていない（前の手は走った）。`execute` の探す段がここから始まる（run を終えない）
    Unfound { step: String, label: String, ambiguous: bool },
    /// ENGINE-AUDIT W7: 承認の要らない押す手 `step`（`REVEAL_ACTION`）を、Adapter が送る直前に解決した要素で承認の要る手と分かった（`gui::asks` = 消す・送る・買う系の名前・
    /// 確認の窓の中・Adapter の `Failed { ApprovalDenied }`）。その手は送っていない（前の手は走った）。`why` = `gui::asks` の 1 行。`execute` がその手を
    /// `CLICK_ACTION` に書き換えて承認の card を出す（run は待ったまま・承認の後はその click から続ける）。既存の `Rejected` に混ぜない = 書き換える手を取り違えない
    Asks { step: String, why: String },
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(why) => write!(f, "The saved procedure isn't in the expected form ({why}). Rebuild it"),
            Self::Missing { step, capability } => write!(f, "The ability {capability} used by the step “{step}” isn't available now. Check the connection"),
            Self::Rejected { step, why } => write!(f, "The step “{step}” was stopped ({why}). This procedure was not sent"),
            Self::Step { step, why } => write!(f, "Stopped at the step “{step}” ({why}). The later steps were not run"),
            Self::Held { step, after, why } => write!(f, "The step “{step}” didn't run, because what “{after}” put couldn't be confirmed right before it ({why}). Nothing more was sent"),
            Self::Store(why) => write!(f, "Can't read or write the saved work procedure ({why})"),
            Self::Waiting { step } => write!(f, "Waiting for your choice at the step “{step}”. The later steps were not run"),
            Self::Unchecked { why, .. } => write!(f, "{why}"),
            Self::Outside { step } => write!(f, "The step “{step}” wasn't run, because it would read a file outside this Project's repo. The later steps were not run"),
            Self::Unfound { step, label, ambiguous } => write!(f, "The step “{step}” {} “{label}” in the window, so it wasn't pressed", if *ambiguous { "found more than one" } else { "couldn't find" }),
            Self::Asks { step, why } => write!(f, "The step “{step}” needs your OK first ({why}), so it wasn't pressed. The later steps were not run"),
        }
    }
}
impl std::error::Error for WorkflowError {}

/// ENGINE-GENERIC-1101: 一言と一緒の状況（`EngineContext::situation`）を指す `$from` の頭（`situation.files.<n>` = 絶対 path・`situation.urls.<n>` = `file://`）。
/// 手より前に在る値として `validate` が指してよい物に数え、Execution が `outputs` に入れる（手の id には使えない）。
pub const SITUATION: &str = "situation";

/// input の中の `{"$from": …}`（key がそれ 1 つの object）を集める。
pub(super) fn from_refs<'v>(v: &'v Value, out: &mut Vec<&'v str>) {
    match v {
        Value::Object(o) => match (o.len(), o.get("$from").and_then(Value::as_str)) {
            (1, Some(r)) => out.push(r),
            _ => o.values().for_each(|x| from_refs(x, out)),
        },
        Value::Array(a) => a.iter().for_each(|x| from_refs(x, out)),
        _ => {}
    }
}

/// `v` の中の `$from` のうち、`before`（指してよい手の id）の手を指さない最初の物。`before` が空なら `$from` が 1 つでも在れば Some。
pub(super) fn stray_from<'v>(v: &'v Value, before: &[&str]) -> Option<&'v str> {
    let mut refs = Vec::new();
    from_refs(v, &mut refs);
    refs.into_iter()
        .find(|r| !before.contains(&r.split('.').next().unwrap_or("")))
}

/// `{"$from": "<step>.<path>"}` を前の手の出力（payload）で埋め、渡した値を `Handoff` に digest で残す。
pub(super) fn resolve(v: &Value, outputs: &HashMap<String, Value>, to_step: &str, field: &str, handoffs: &mut Vec<Handoff>) -> Result<Value, String> {
    let join = |k: &str| if field.is_empty() { k.to_string() } else { format!("{field}.{k}") };
    Ok(match v {
        Value::Object(o) => match (o.len(), o.get("$from").and_then(Value::as_str)) {
            (1, Some(r)) => {
                let (step, path) = r.split_once('.').unwrap_or((r, ""));
                let out = outputs.get(step).ok_or_else(|| format!("前の手順「{step}」の出力がありません"))?;
                let got = if path.is_empty() {
                    Some(out)
                } else {
                    path.split('.').try_fold(out, |x, k| x.get(k).or_else(|| k.parse::<usize>().ok().and_then(|i| x.get(i))))
                };
                let got = got.ok_or_else(|| format!("前の手順「{step}」の出力に {path} がありません"))?.clone();
                handoffs.push(Handoff { from_step: step.into(), to_step: to_step.into(), field: field.into(), digest: crate::crypto::sha256_hex(got.to_string().as_bytes()) });
                got
            }
            _ => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), resolve(x, outputs, to_step, &join(k), handoffs)?))).collect::<Result<_, String>>()?),
        },
        Value::Array(a) => Value::Array(a.iter().enumerate().map(|(i, x)| resolve(x, outputs, to_step, &join(&i.to_string()), handoffs)).collect::<Result<_, _>>()?),
        other => other.clone(),
    })
}

/// 形の検査（走らせる前・保存の前）。
/// Wave: W19-2b
/// 保証する: schema・id が空でない・step が 1 つ以上・step の id が空でなく重ならない・`$from` が前の step か状況（`SITUATION`）だけを指す・`checks` の input に `$from` が無い・
/// `checks` の `Expect::Equals` の値の `$from` は在る step を指す（最初の書く手より前の手か、は action の効果を知る `run_workflow` と `generate` が見る）。
pub fn validate(wf: &Workflow) -> Result<(), WorkflowError> {
    let bad = |why: String| Err(WorkflowError::Invalid(why));
    if wf.schema != WORKFLOW_SCHEMA {
        return bad(format!("schema が {WORKFLOW_SCHEMA} ではありません"));
    }
    if wf.id.trim().is_empty() || wf.steps.is_empty() {
        return bad("id が空か、手順が 0 個です".into());
    }
    // ENGINE-GENERIC-1101: 状況（`SITUATION`）は最初の手より前に在る値（同じ id の手は重なりで拒否）
    let mut before: Vec<&str> = vec![SITUATION];
    for s in &wf.steps {
        if s.id.trim().is_empty() || before.contains(&s.id.as_str()) {
            return bad(format!("手順の id「{}」が空か重なっています", s.id));
        }
        if let Some(r) = stray_from(&s.input, &before) {
            return bad(format!("手順「{}」の $from「{r}」は前の手順を指していません", s.id));
        }
        // WF0: 判断の手は問いと選択肢（id が空でなく重ならない・名前が在る）を持ち、選択肢の値の `$from` は前の手だけを指す
        if let StepKind::Decide { question, choices } = &s.kind {
            let mut ids: Vec<&str> = Vec::new();
            if question.trim().is_empty() || choices.is_empty() {
                return bad(format!("判断「{}」に問いか選択肢がありません", s.id));
            }
            for c in choices {
                if c.id.trim().is_empty() || c.label.trim().is_empty() || ids.contains(&c.id.as_str()) {
                    return bad(format!("判断「{}」の選択肢「{}」の id が空か重なっている・名前がありません", s.id, c.id));
                }
                if let Some(r) = stray_from(&c.value, &before) {
                    return bad(format!("判断「{}」の選択肢「{}」の $from「{r}」は前の手順を指していません", s.id, c.id));
                }
                ids.push(c.id.as_str());
            }
        }
        before.push(&s.id);
    }
    for c in &wf.checks {
        if stray_from(&c.input, &[]).is_some() {
            return bad(format!("読み戻し {} の input は手順の出力を指せません（操作の前にも読むため）", c.action));
        }
        if let Expect::Equals { value } = &c.expect {
            if let Some(r) = stray_from(value, &before) {
                return bad(format!("読み戻し {} の期待する値の $from「{r}」は手順を指していません", c.action));
            }
        }
    }
    Ok(())
}

/// 読み戻しの期待する値の `$from` が、最初の書く手（`reads` が false の手）より前の手だけを指すか。後ろの手の出力は、書いた後に読んだ値で、
/// それと読み戻しを比べても自分自身と比べるだけになる（違う値を書いても「等しい」）。`validate` は action の効果を知らないので、効果の分かる所
/// （`run_steps`・`check_generated`）で呼ぶ。
pub(super) fn expected_before_write(wf: &Workflow, reads: impl Fn(&str) -> bool) -> Result<(), String> {
    let first = wf.steps.iter().position(|s| s.is_action() && !reads(&s.capability)).unwrap_or(wf.steps.len());
    // ENGINE-GENERIC-1101: 状況（`SITUATION`）は一言の時の値なので、書いた後に読んだ自分の値ではない
    let mut before: Vec<&str> = std::iter::once(SITUATION).chain(wf.steps[..first].iter().map(|s| s.id.as_str())).collect();
    // WF0: 判断の手の値は人が選んだ物（書いた後に読んだ自分の値ではない）ので、読み戻しはそれと比べてよい
    before.extend(wf.steps.iter().filter(|s| !s.is_action()).map(|s| s.id.as_str()));
    for c in &wf.checks {
        let Expect::Equals { value } = &c.expect else { continue };
        if let Some(r) = stray_from(value, &before) {
            return Err(format!("読み戻し {} の期待する値の $from「{r}」は書く手より後の手を指しています（書いた後に読んだ値と比べても確かめになりません）", c.action));
        }
    }
    Ok(())
}

/// 受け渡した値（`$from`）を外の service に渡す手か（承認の要らない読む action でも、利用者の data を外へ出すので exact payload の承認が要る）。
fn hands_out(a: &ActionDefinition, handoffs: &[Handoff]) -> bool {
    a.effect.is_external() && !handoffs.is_empty()
}

/// 手 1 つの承認に要る物（`run_steps` が gate に渡すのと同じ、受け渡した後の exact input）。Ok(None) = 承認が要らない・Err = input が今決まらない
/// （`$from` がまだ走っていない手を指す）か action が無い。
fn approval_of(s: &WorkflowStep, outputs: &HashMap<String, Value>, actions: &[ActionDefinition]) -> Result<Option<PendingApproval>, ()> {
    let mut handoffs = Vec::new();
    let input = resolve(&s.input, outputs, &s.id, "", &mut handoffs).map_err(|_| ())?;
    let a = actions.iter().find(|a| a.id == s.capability).ok_or(())?;
    Ok((a.requires_approval || hands_out(a, &handoffs))
        .then(|| PendingApproval { step: s.id.clone(), action: a.id.clone(), digest: sdk::payload_digest(&a.id, &input), input, scopes: a.required_scopes.clone() }))
}

/// 止められた手が承認の要る手なら、その手の承認に要る物。
pub(super) fn pending(wf: &Workflow, step: &str, outputs: &HashMap<String, Value>, actions: &[ActionDefinition]) -> Option<PendingApproval> {
    approval_of(wf.steps.iter().find(|s| s.id == step)?, outputs, actions).ok().flatten()
}

/// 止められた手より後ろで承認の要る手のうち、input が今決まる物（`Denial::later`）。input が決まらない手で止める（その後ろは見ない）。
pub(super) fn later(wf: &Workflow, step: &str, outputs: &HashMap<String, Value>, actions: &[ActionDefinition]) -> Vec<PendingApproval> {
    wf.steps
        .iter()
        .skip_while(|s| s.id != step)
        .skip(1)
        .map(|s| approval_of(s, outputs, actions))
        .take_while(Result::is_ok)
        .filter_map(|r| r.ok().flatten())
        .collect()
}

/// 読み戻しの期待する値の `$from` を手の出力で埋める（受け渡した値を受け取った側の実状態で確かめる・`Expect` の W19-2b）。`$from` の無い物はそのまま。
pub(super) fn expected(c: &ReadBack, outputs: &HashMap<String, Value>) -> Result<ReadBack, String> {
    let Expect::Equals { value } = &c.expect else { return Ok(c.clone()) };
    let value = resolve(value, outputs, "", "", &mut Vec::new()).map_err(|why| format!("読み戻し {} の期待する値: {why}", c.action))?;
    Ok(ReadBack { expect: Expect::Equals { value }, ..c.clone() })
}

/// 承認の記録から、この action とこの入力（exact payload）への物を選ぶ。合う物が無ければ同じ action の物（registry が「内容が違う」と断る）。
pub(super) fn consent_for<'c>(consents: &'c [ConsentReceipt], action: &str, input: &Value) -> Option<&'c ConsentReceipt> {
    let digest = sdk::payload_digest(action, input);
    let mine = |c: &&ConsentReceipt| matches!(&c.subject, ConsentSubject::Action { action_ref, .. } if action_ref == action);
    consents.iter().filter(mine).find(|c| c.payload_digest.as_deref() == Some(digest.as_str())).or_else(|| consents.iter().find(mine))
}

/// 前から順に 1 手ずつ registry の `invoke`（gate を通る唯一の口）で走らせ、`$from` を前の Observation から埋める。
/// Wave: W19-2b
/// 保証する: `validate` を先に通す・走らせる前に全 step の能力が registry で Ready か見る（無ければ `Missing` で 1 手も走らせない）・1 手でも Verified / Unverified でなければ
/// そこで止める（gate の拒否は `Rejected`・失敗は `Step`・後ろは走らせない）・渡した値は `Handoff` に digest で残す・承認の要る step は `consents` に exact payload の承認が
/// 無ければ registry が Rejected（送らない）・受け渡した値を外の service へ渡す手（効果が external_*）は承認の要らない読む action でも exact payload の
/// 承認が無ければ Rejected（送らない）・読み戻しの期待する値の `$from` が最初の書く手より後の手を指す手順は `Invalid`（1 手も走らせない）・
/// 書く手の後ろの書く手は、前の書く手が置いた物を送る直前に `AdapterRegistry::recheck` で読み直し、確かめられなければ `Held`（その手から後ろは送らない・G74）。
/// B76: 読む手（`SAVED_READ`）は `$from` を埋めた後の uri で見て、repo を知らないこの口では `file://` の絶対 path を全部外とする（`Outside`・fail closed）。
/// 保証しない: 走らせた結果が元の service に残った事（それは試験の側の oracle と `Workflow::checks` の読み戻し）・読み直す口を持たない手（`recheck` が None）の後ろを止める事。
pub fn run_workflow(
    wf: &Workflow,
    registry: &mut AdapterRegistry,
    caller: Caller,
    consents: &[ConsentReceipt],
) -> Result<WorkflowReceipt, WorkflowError> {
    run_steps(wf, registry, caller, consents, &mut HashMap::new(), 0, &|i: &Value| outside_repo(i, None, None), &NO_STOP, &mut |_, _| Ok(()))
}

/// 手ごとの記録の口（WF1・`run_steps`）: 手 `k`（`Workflow::steps` の番号）を送る直前に `None`（write-ahead・Err なら送らない = `Store`）、結果の後に
/// `Some(その手の Receipt)`（保存できなくても続ける = 次の手の write-ahead が止める）。
pub(super) type Mark<'m> = &'m mut dyn FnMut(usize, Option<&StepReceipt>) -> Result<(), String>;

/// 送った後に結果が分からない手の理由の頭（`run` が「送れたか確かめられない」の文に分ける・G74）。
pub(super) const SENT_UNKNOWN: &str = "Sent, but the result is unknown";

/// `recheck` が Verified でない時の理由の 1 行。
pub(super) fn why_of(o: &Outcome) -> String {
    match o {
        Outcome::Unverified { reason } | Outcome::Rejected { reason } | Outcome::EffectUnknown { reason } => reason.clone(),
        Outcome::Failed { error } => error_word(*error).into(),
        Outcome::Verified { .. } => String::new(),
    }
}

/// `run_workflow` の中身。走った手の出力（payload）を `outputs` に残す（止まった時も: `run` が読み戻しの期待する値の `$from` と、承認待ちの手の exact input を埋める）。
/// WF1・WF2: `from` = 走らせ始める手（前の手の出力は `outputs` に在る）・判断の手は `outputs` に答えが在れば越え、無ければ `Waiting`（その手と後ろは走らせない）・
/// 手ごとに `mark`。B76: 読む手（`SAVED_READ`）は `$from` を埋めた後の input が `out_of_repo`（Project の repo の外）なら走らせない（`Outside`）。
/// ENGINE-ADJUST-1102 ①: `stop` = 人の Stop の印（`EngineContext::stop`）。引数にした理由: 手の間で見る所はこの loop の中だけ（本体で手を送る前に見て、立っていれば
/// `Rejected { why: STOPPED_WHY }` = その手と後ろは送らない・`run` が走った手の数で `Denied` / `Stopped` に分ける）。`run_workflow` は `&NO_STOP`。
pub(super) fn run_steps(wf: &Workflow, registry: &mut AdapterRegistry, caller: Caller, consents: &[ConsentReceipt], outputs: &mut HashMap<String, Value>, from: usize, out_of_repo: &dyn Fn(&Value) -> bool, stop: &AtomicBool, mark: Mark) -> Result<WorkflowReceipt, WorkflowError> {
    validate(wf)?;
    if let Some(s) = wf.steps.iter().skip(from).filter(|s| s.is_action()).find(|s| !matches!(registry.tier(&s.capability), DisplayTier::Ready | DisplayTier::Verified)) {
        return Err(WorkflowError::Missing { step: s.id.clone(), capability: s.capability.clone() });
    }
    let actions = registry.actions();
    let action = |cap: &str| actions.iter().find(|a| a.id == cap);
    // WF-006（P49-3a）: 外へ効く手は、Adapter が送る直前に宛先を読み直すと宣言している時だけ（adapter を問わない門・宣言の無い手が 1 つでも在れば 1 手も走らせない）
    let outside = |cap: &str| action(cap).is_some_and(|a| matches!(a.effect, Effect::ExternalWrite | Effect::ExternalSend | Effect::Destructive));
    if let Some(s) = wf.steps.iter().skip(from).filter(|s| s.is_action()).find(|s| outside(&s.capability) && !registry.prechecks(&s.capability)) {
        let why = format!("Shibubu has no way to check where the step “{}” would go right before sending it, so nothing was sent. Do it in the original app", s.id);
        return Err(WorkflowError::Unchecked { step: s.id.clone(), why });
    }
    expected_before_write(wf, |cap| action(cap).is_some_and(|a| is_read(a.effect))).map_err(WorkflowError::Invalid)?;
    let mut steps: Vec<StepReceipt> = Vec::new();
    // G74: 最後に走った書く手と、その受け渡した後の input（後ろの書く手の直前に `recheck` で読み直す）
    let mut placed: Option<(&WorkflowStep, Value)> = None;
    for (k, s) in wf.steps.iter().enumerate().skip(from) {
        if !s.is_action() {
            if outputs.contains_key(&s.id) {
                continue;
            }
            return Err(WorkflowError::Waiting { step: s.id.clone() });
        }
        let mut handoffs = Vec::new();
        let input = resolve(&s.input, outputs, &s.id, "", &mut handoffs).map_err(|why| WorkflowError::Step { step: s.id.clone(), why })?;
        // B76（安全）: 読む手の uri は前の手の出力（README・窓の中の別の repo の鍵の file の場所）で埋まり得る = 埋めた後に見る（承認の要らない読み・計画の段を通らない resume も）
        if s.capability == SAVED_READ && out_of_repo(&input) {
            return Err(WorkflowError::Outside { step: s.id.clone() });
        }
        // ENGINE-AUDIT W7: 承認の要らない押す手（reveal）が、人が同じ input の click として承認した手（送る直前の 2 回目の門で card になった手の承認の後の run）なら click で送る
        // （同じ一言のし直し・探す段・保存した手順で reveal に戻っても card を繰り返さない）
        let clicked = |c: &&ConsentReceipt| c.payload_digest.as_deref() == Some(sdk::payload_digest(super::CLICK_ACTION, &input).as_str());
        let cap = if s.capability == super::REVEAL_ACTION && consent_for(consents, super::CLICK_ACTION, &input).filter(clicked).is_some() { super::CLICK_ACTION } else { s.capability.as_str() };
        let consent = consent_for(consents, cap, &input);
        // 受け渡した値を外の service へ渡す手は、registry が承認を見ない読む action でも exact payload の承認を見てから（19 通目 §3 の 7・§4）
        // ponytail: 承認の使用済みは registry の承認の要る action だけが記録する。この道の承認は同じ内容なら次の run でも通る（registry に使用済みの口が要る）
        if let Some(a) = action(&s.capability).filter(|a| !a.requires_approval && hands_out(a, &handoffs)) {
            if let Some(why) = crate::adapters::approval_gap(consent, caller, &a.id, &a.required_scopes, &input, &HashSet::new()) {
                return Err(WorkflowError::Rejected { step: s.id.clone(), why: format!("'{}' は承認が要る（受け渡した値を外の service へ渡す・{why}）", a.id) });
            }
        }
        // G74（owner 12:2x・③-8）: 書く手の後ろの書く手（送る・押す・保存）は、前の書く手が置いた物を送る直前に読み直し（`AdapterRegistry::recheck`）、
        // 今もその通りと確かめられた時だけ送る。違う・読めない → この手から後ろは送らない（Held）。読み直す口を持たない手（click・menu・key = None）は今まで通り
        let writes = action(&s.capability).is_some_and(|a| !is_read(a.effect));
        if let Some((p, put)) = placed.as_ref().filter(|_| writes) {
            match registry.recheck(&p.capability, put) {
                None => {}
                Some(Outcome::Verified { evidence }) => {
                    if let Some(StepReceipt { outcome: Outcome::Verified { evidence: e }, .. }) = steps.iter_mut().rev().find(|r| r.step == p.id) {
                        e.extend(evidence);
                    }
                }
                Some(o) => return Err(WorkflowError::Held { step: s.id.clone(), after: p.id.clone(), why: why_of(&o) }),
            }
        }
        // ENGINE-ADJUST-1102 ①: 手を送る直前に人の Stop の印を見る（立っていればこの手と後ろは送らない・`run` が走った手の数で Denied / Stopped に分ける）
        if stop.load(Ordering::SeqCst) {
            return Err(WorkflowError::Rejected { step: s.id.clone(), why: STOPPED_WHY.into() });
        }
        mark(k, None).map_err(WorkflowError::Store)?;
        let (outcome, obs) = registry.invoke(caller, cap, &input, consent);
        let receipt = StepReceipt { step: s.id.clone(), outcome, observation: obs.as_ref().map(|o| o.id.clone()), handoffs };
        let _ = mark(k, Some(&receipt));
        match &receipt.outcome {
            Outcome::Verified { .. } | Outcome::Unverified { .. } => {}
            Outcome::Rejected { reason } => return Err(WorkflowError::Rejected { step: s.id.clone(), why: reason.clone() }),
            // ENGINE-AUDIT W7: 承認の要らない押す手を、Adapter が押す直前に解決した要素で承認の要る手と分けた（押していない）= `execute` が click に書き換えて card。
            // 理由は計画の段と同じ `gui::asks`（名前で当たらなければ確認の窓の中）
            Outcome::Failed { error: crate::domain::run::ErrorCode::ApprovalDenied } if cap == super::REVEAL_ACTION => {
                let (label, role) = (input["label"].as_str().unwrap_or_default(), input["role"].as_str().unwrap_or_default());
                let why = crate::gui::asks(label, role, false).or_else(|| crate::gui::asks(label, role, true)).unwrap_or_default();
                return Err(WorkflowError::Asks { step: s.id.clone(), why });
            }
            // ENGINE-AUDIT W5: 画面の手（input に押す名前 `label` が在る）の名前が無い・1 つに絞れない（Adapter は押していない）= 探す段へ（`execute` の `Unfound` の腕）。
            // label の無い手（HTTP の 404・MCP の無い対象）は今まで通り `Step`（窓を読み直しても意味が無い）
            Outcome::Failed { error: e @ (crate::domain::run::ErrorCode::TargetNotFound | crate::domain::run::ErrorCode::TargetAmbiguous) }
                if input["label"].as_str().is_some_and(|l| !l.trim().is_empty()) =>
            {
                let label = input["label"].as_str().unwrap_or_default().trim().to_owned();
                return Err(WorkflowError::Unfound { step: s.id.clone(), label, ambiguous: *e == crate::domain::run::ErrorCode::TargetAmbiguous });
            }
            Outcome::Failed { error } => return Err(WorkflowError::Step { step: s.id.clone(), why: error_word(*error).into() }),
            Outcome::EffectUnknown { reason } => return Err(WorkflowError::Step { step: s.id.clone(), why: format!("{SENT_UNKNOWN} ({reason})") }),
        }
        if writes {
            placed = Some((s, input.clone()));
        }
        outputs.insert(s.id.clone(), obs.map_or(Value::Null, |o| o.payload));
        steps.push(receipt);
    }
    Ok(WorkflowReceipt { workflow: wf.id.clone(), steps })
}

/// `<dir>/<id>.json` に書く（`validate` を通った物だけ・一時 file から rename）。
/// Wave: W19-2b
/// 保証する: `validate` に通らない・id が 1 つの file の名前にならない → 書かない（`Invalid` / `Store`）・同じ id は置き換える。
pub fn save_workflow(dir: &Path, wf: &Workflow) -> Result<PathBuf, WorkflowError> {
    validate(wf)?;
    if !dir_name_ok(&wf.id) {
        return Err(WorkflowError::Invalid(format!("id「{}」は file の名前にできません", wf.id)));
    }
    let path = dir.join(format!("{}.json", wf.id));
    write_json(&path, wf, &format!("Procedure “{}”", wf.id)).map_err(WorkflowError::Store)?;
    Ok(path)
}

/// `<dir>/*.json` を全部読む（再起動の後も使える）。
/// Wave: W19-2b
/// 保証する: dir が無ければ空・読めない・形の違う・`validate` に通らない file は飛ばす（使わない）・dir 自体を読めない時だけ `Store`。
pub fn load_workflows(dir: &Path) -> Result<Vec<Workflow>, WorkflowError> {
    let all: Vec<Workflow> = load_json_dir(dir).map_err(|e| WorkflowError::Store(format!("Can't read the place where procedures are saved ({})", e.kind())))?;
    Ok(all.into_iter().filter(|w| validate(w).is_ok()).collect())
}
