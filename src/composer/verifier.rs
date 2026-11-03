//! BuildVerifier（GEN-009）: 参照・action の実装・権限・UI の状態・responsiveness・runtime smoke を検査し、
//! app が名乗ってよい readiness を返す。Blocker が 1 つでも在れば **ready を名乗らせない**（未完成の app を ready にしない）。
//!
//! 1. 参照と README の規則: domain の `validate` ＋ `validate_generated`（規則は 1 か所: domain/validate.rs）
//! 2. 実装: ready な local action は `local.<namespace>.<verb>` で、namespace は app の local source か組み込み、verb は LOCAL_VERBS
//!    （effect と承認も表と同じ。mic の録音の様な GATED_VERBS はその組み込みの capability を指す）。外部の action を ready と言えるのは
//!    registry / 組み込みが ready と言った capability だけ
//! 3. 権限: 宣言した capability の readiness が registry / 組み込みの言う値より良くない（自分で ready を名乗らない）。
//!    組み込み以外の capability を通る action は local_read / local_write を名乗らない（承認の gate は effect から導く: CD-01）
//! 4. responsiveness: split の ratio は子の数と合い・合計 1・主役 ≥ 60%（DESIGN §7）。layout に座標や px を書かない
//! 5. runtime smoke: ready な action は schema から作った入力で `validate_invocation` を通り、ready でない action は通らない
//!
//! W14b 使える検査（DESIGN §16.3 規則 8・27 通目）: 上の 5 つは「定義が正しいか」。作った物を初めての人が使えるかは、面（席）ごとに 3 問で決める ──
//! ① 届く ② 主たる 1 手が通る（local と read だけ・外に効く手は `LocalStep` が作れず「要承認」）③ 外で読み戻せる（local DB・disk・AX の値）。
//! 3 問を通った面だけ `Usable::ready`。測るのは `Probe`（製品は generated.rs・試験は偽物）で、ここは順番と判定だけ（IO を持たない）。図: docs/diagrams.md §33。
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::capabilities::{known_readiness, local_verb, split_local_id, BUILTIN_CAPABILITIES, BUILTIN_NAMESPACES, GATED_VERBS};
use super::domain::action::{validate_invocation, ActionInvocation, ActionReadiness, Effect, InvocationError, InvocationOrigin};
use super::domain::app::{AppDefinitionV3, AppReadiness};
use super::domain::capability::{CapabilityDescriptor, CapabilityReadiness};
use super::domain::project::{ProjectDefinitionV1, SurfaceBackendKind, SurfaceTarget};
use super::project::SeatV1;
use super::domain::ui::UiNode;
use super::domain::validate::{validate, validate_generated};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// ready を名乗れない
    Blocker,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub check: &'static str,
    pub subject: String,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    /// この app が名乗ってよい readiness（Blocker が在れば Ready にならない）
    pub readiness: AppReadiness,
    pub findings: Vec<Finding>,
    pub ready_actions: usize,
    pub total_actions: usize,
    /// smoke を通った ready な action
    pub smoke: Vec<String>,
}

impl Verdict {
    pub fn blockers(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.severity == Severity::Blocker).collect()
    }
    pub fn is_ready(&self) -> bool {
        self.readiness == AppReadiness::Ready
    }
}

const LAYOUT_ABSOLUTE: &[&str] = &["x", "y", "left", "top", "width", "height", "px", "color", "font_size", "css"];

fn rank(r: CapabilityReadiness) -> u8 {
    use CapabilityReadiness::*;
    match r {
        Ready => 3,
        ConnectedUnverified | Degraded => 2,
        Discoverable | RequiresConnection | SimulatedInPreview => 1,
        Unknown | Unsupported | Revoked => 0,
    }
}

/// required の欄に型どおりの最小値を入れる（runtime smoke 用）。
pub fn sample_input(schema: &Map<String, Value>) -> Value {
    let props = schema.get("properties").and_then(Value::as_object);
    let mut obj = Map::new();
    for name in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        let p = props.and_then(|p| p.get(name));
        let v = match p.and_then(|p| p.get("type")).and_then(Value::as_str).unwrap_or("string") {
            "integer" => json!(p.and_then(|p| p.get("minimum")).and_then(Value::as_i64).unwrap_or(0).max(1)),
            "number" => json!(1.0),
            "boolean" => json!(false),
            "array" => json!([]),
            "object" => json!({}),
            _ => p.and_then(|p| p.get("enum")).and_then(Value::as_array).and_then(|e| e.first().cloned()).unwrap_or_else(|| json!("sample")),
        };
        obj.insert(name.into(), v);
    }
    Value::Object(obj)
}

pub(super) fn walk(n: &UiNode, out: &mut Vec<Finding>) {
    if let Some(layout) = &n.layout {
        for k in layout.keys().filter(|k| LAYOUT_ABSOLUTE.contains(&k.as_str())) {
            out.push(Finding { check: "responsiveness", subject: n.key.clone(), message: format!("layout に絶対値 '{k}' を書かない（renderer が決める）"), severity: Severity::Blocker });
        }
        if let Some(ratio) = layout.get("ratio").and_then(Value::as_array) {
            let vals: Vec<f64> = ratio.iter().filter_map(Value::as_f64).collect();
            let sum: f64 = vals.iter().sum();
            if vals.len() != n.children.len() || (sum - 1.0).abs() > 0.01 {
                out.push(Finding { check: "responsiveness", subject: n.key.clone(), message: format!("split の ratio {vals:?} が子の数 {} と合わない・合計が 1 でない", n.children.len()), severity: Severity::Blocker });
            } else if vals.iter().cloned().fold(0.0, f64::max) < 0.6 {
                out.push(Finding { check: "responsiveness", subject: n.key.clone(), message: "split の主役に 60% 以上を渡す（DESIGN §7）".into(), severity: Severity::Blocker });
            }
        }
    }
    for c in &n.children {
        walk(c, out);
    }
}

fn origin_for(def: &AppDefinitionV3, action: &str) -> InvocationOrigin {
    match def.composition.root.nodes().into_iter().find(|n| n.action_ref.as_deref() == Some(action)) {
        Some(n) => InvocationOrigin::Button { node_key: n.key.clone() },
        None => InvocationOrigin::CommandPalette,
    }
}

/// 唯一の入口。registry は Connections が知っている capability（id の完全一致で照合）。
pub fn verify(def: &AppDefinitionV3, registry: &[CapabilityDescriptor]) -> Verdict {
    let mut f: Vec<Finding> = Vec::new();
    let mut blocker = |check: &'static str, subject: &str, message: String| f.push(Finding { check, subject: subject.into(), message, severity: Severity::Blocker });

    // 1. 参照と README の規則
    let structural = validate(def).is_ok();
    if let Err(errs) = validate_generated(def) {
        for e in errs {
            blocker("contract", &def.id, e.to_string());
        }
    }
    // 3. 宣言した capability が registry / 組み込みより良い readiness を名乗らない
    for c in &def.capabilities {
        let known = known_readiness(&c.id, registry).unwrap_or(CapabilityReadiness::Unknown);
        // 接続済み以上（ready / 確認中 / 一部）を名乗れるのは registry / 組み込みがそう言った時だけ。候補（discoverable / requires_connection）は名乗ってよい
        let fabricated = (c.readiness == CapabilityReadiness::Ready && known != CapabilityReadiness::Ready) || (rank(c.readiness) == 2 && rank(known) < 2);
        if fabricated {
            blocker("permissions", &c.id, format!("capability '{}' は {:?} を名乗るが registry / 組み込みは {:?}（捏造した readiness）", c.id, c.readiness, known));
        }
    }
    // 2. 実装
    let local_namespaces: Vec<&str> = def.sources.iter().filter_map(|s| s.id.strip_prefix("local.")).collect();
    for a in def.actions.iter().filter(|a| a.readiness == ActionReadiness::Ready) {
        match split_local_id(&a.id) {
            Some((ns, verb)) => {
                if !local_namespaces.contains(&ns) && !BUILTIN_NAMESPACES.contains(&ns) {
                    blocker("implementation", &a.id, format!("namespace '{ns}' の local source が無い（runtime は何に書くか分からない）"));
                }
                match local_verb(verb) {
                    None => blocker("implementation", &a.id, format!("runtime に無い local verb '{verb}'（LOCAL_VERBS）→ ready を名乗れない")),
                    Some(lv) => {
                        if lv.effect != a.effect {
                            blocker("implementation", &a.id, format!("verb '{verb}' の effect は {:?}（定義は {:?}）", lv.effect, a.effect));
                        }
                        if lv.requires_approval && !a.requires_approval {
                            blocker("permissions", &a.id, format!("verb '{verb}' は承認が要る（requires_approval: true）"));
                        }
                        // capability を外した録音は、OS の mic 許可（local.microphone）を通らずに ready を名乗る
                        if let Some((_, gate)) = GATED_VERBS.iter().find(|(v, _)| *v == verb) {
                            if a.capability_ref.as_deref() != Some(*gate) {
                                blocker("permissions", &a.id, format!("verb '{verb}' は capability '{gate}'（OS の許可）を通る（定義は {:?}）", a.capability_ref));
                            }
                        }
                    }
                }
            }
            None => {
                let cap = a.capability_ref.as_deref();
                let ok = cap.and_then(|c| known_readiness(c, registry)) == Some(CapabilityReadiness::Ready);
                if !ok {
                    blocker("implementation", &a.id, format!("外部の action が ready だが、registry / 組み込みで ready な capability を指していない（{cap:?}）"));
                }
            }
        }
    }
    // 外の能力（組み込み以外）を通る action が local の effect を名乗ると、effect から導く承認の gate（CD-01）を素通りする
    for a in &def.actions {
        let Some(c) = a.capability_ref.as_deref() else { continue };
        if matches!(a.effect, Effect::LocalRead | Effect::LocalWrite) && !BUILTIN_CAPABILITIES.iter().any(|(b, _, _)| *b == c) {
            blocker("permissions", &a.id, format!("capability '{c}' を通る action が {:?} を名乗る（外へ出る action は external_* か destructive。local を名乗ると承認の gate を通らない）", a.effect));
        }
    }
    // 4. responsiveness
    walk(&def.composition.root, &mut f);
    // 5. runtime smoke（参照が壊れていない時だけ）
    let mut smoke = Vec::new();
    if structural {
        for a in &def.actions {
            let inv = ActionInvocation { action_ref: a.id.clone(), origin: origin_for(def, &a.id), input: sample_input(&a.input_schema) };
            match (a.readiness, validate_invocation(def, &inv)) {
                (ActionReadiness::Ready, Ok(_)) => smoke.push(a.id.clone()),
                (ActionReadiness::Ready, Err(e)) => f.push(Finding { check: "smoke", subject: a.id.clone(), message: format!("validate_invocation が落ちた: {e}"), severity: Severity::Blocker }),
                (_, Ok(_)) => f.push(Finding { check: "smoke", subject: a.id.clone(), message: "ready でないのに実行できてしまう".into(), severity: Severity::Blocker }),
                (_, Err(InvocationError::NotReady { .. })) => {}
                (_, Err(e)) => f.push(Finding { check: "smoke", subject: a.id.clone(), message: format!("ready でない action が NotReady 以外で止まった: {e}"), severity: Severity::Warning }),
            }
        }
    }
    let total = def.actions.len();
    let ready = def.actions.iter().filter(|a| a.readiness == ActionReadiness::Ready).count();
    let blockers = f.iter().any(|x| x.severity == Severity::Blocker);
    let readiness = if !structural {
        AppReadiness::Error
    } else if blockers {
        AppReadiness::Blocked
    } else {
        def.state.readiness
    };
    Verdict { readiness, findings: f, ready_actions: ready, total_actions: total, smoke }
}

// ============================== W14b 使える検査（DESIGN §16.3 規則 8・27 通目） ==============================

/// W14b: 面（席）の種類。① 届くの測り方が種類ごとに違う（測るのは `Probe`）。
/// Wave: W14b
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    /// Project の Terminal（PTY が起きて prompt が返る）
    Terminal,
    /// 本物の app の窓を Page に重ねる面（app が入っていて、Shibubu に AX の許可が在る）
    NativeApp,
    /// 元の窓を映す面（映像が 1 枚来るか、許可の 1 行）
    Mirror,
    /// Web の面（URL が読み込まれる）
    Web,
    /// 生成した app（主役の部品が描かれ、入力欄が focus を取る）
    Generated,
    /// W13b: 一言が名指した app がこの Mac のどこにも無かった席（`project::installed_apps` の一覧にも全部にも無い・Spotlight・面は置いていない・Page の中に 1 問）。
    /// ① はいつも `Fail{Install, 1 行}`（測らない・`Need::Install` は利用者の手が要るので測り直さない）・②③ は Skipped。
    /// 1 行（`Usable::line` =「入れる: …」）が問いで、その下に答えの button（`renderer::usable_line` の `choices`）が並ぶ。
    /// Wave: W13b
    MissingApp,
}

/// W14b: ③ で何を見れば 1 手が通ったと言えるか。
/// Wave: W14b
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// 1 手で入れた印（`LocalStep::mark`）が、1 手の後の外の値に在り、前の値に無い（1 件入れる・1 行打つ・1 文字入れて保存）
    Mark,
    /// 外の値が 1 手の前と後で違う（start / stop で経過が残る。印を入れる欄が無い手）
    Changed,
}

/// W14b: その面の主たる 1 手（job ごとに 1 手）。effect と承認は action の定義（生成 app）か面の種類（元の app を動かす手 = 外への書き込み）から決まる。
/// Wave: W14b
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FirstAction {
    /// 画面の 1 行に出す手の名前（「1 件入れる」「1 行打つ」）
    pub what: String,
    /// 生成 app の手は action の id（`local.<source>.<verb>`）。Terminal と元の app の手は None
    pub action: Option<String>,
    pub effect: Effect,
    pub requires_approval: bool,
    pub expect: Expect,
    /// 1 手で押す回数（start / stop = 2・他は 1）。`Probe::act` が同じ手をこの回数だけ続けて押す
    pub presses: u8,
}

/// W14b: 使える検査の 1 席。`app_seat` / `project_seats` が定義から決定的に作る。id は terminal_specs・surface_specs・app の id（AX の `usable:<id>`）。
/// Wave: W14b
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Seat {
    pub id: String,
    /// 画面の名前（app の名前・host・「Terminal」・app の題名）。bundle id・URL の scheme・内部の id は入れない
    pub name: String,
    pub kind: SeatKind,
    /// None = 主たる 1 手が決まらない（② は Retry の 1 行）
    pub first: Option<FirstAction>,
}

/// W14b: 通らない時に利用者がする次の一手の種類（画面の 1 行の頭の語）。
/// Wave: W14b
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    /// 要接続: 繋ぐ・開き直すと届く（bridge が答えない・Page を開くと読み込む）
    Connect,
    /// 要承認: 外に効く手（元の app の中身を変える・送る・消す）なので評価では呼ばない
    Approve,
    /// 許可が要る: OS の許可（アクセシビリティ・画面収録）
    Permission,
    /// 入れる: app が入っていない・この版に部品が入っていない
    Install,
    /// 直しても通らない（PTY が答えない・1 手が落ちた・読み戻せない）。作り直すか開き直す
    Retry,
}

impl Need {
    /// 1 行の頭の語。
    pub const fn word(self) -> &'static str {
        match self {
            Need::Connect => "Needs a connection",
            Need::Approve => "Needs approval",
            Need::Permission => "Needs permission",
            Need::Install => "Install",
            Need::Retry => "Rebuild",
        }
    }
}

/// W14b: 3 問のそれぞれの答え。
/// Wave: W14b
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "line", rename_all = "snake_case")]
pub enum Check {
    /// 通った（何で確かめたか 1 行）
    Pass(String),
    Fail { need: Need, line: String },
    /// まだ測れていない（裏で測っている・Page を開くと測る）
    Wait(String),
    /// 前の問いが通らなかったので測っていない
    Skipped,
}

/// W14b: 評価が動かしてよい 1 手。作れるのは local（`LocalRead`・`LocalWrite`）と読むだけ（`ExternalRead`）で、承認の要らない手だけ。
/// 外に効く手（`ExternalWrite`・`ExternalSend`・`Destructive`）と承認の要る手は `new` が `Err(Need::Approve)` を返す ── `Probe::act` に渡す値が作れない。
/// field は private（`new` を通らずに作れない）。
/// 例: `let step = LocalStep::new(&first, "usable-1a2b")?; probe.act(&seat, &step)`
/// Wave: W14b
#[derive(Debug, Clone, PartialEq)]
pub struct LocalStep {
    effect: Effect,
    action: Option<String>,
    mark: String,
}

impl LocalStep {
    pub fn new(first: &FirstAction, mark: &str) -> Result<LocalStep, Need> {
        let safe = matches!(first.effect, Effect::LocalRead | Effect::LocalWrite | Effect::ExternalRead);
        if !safe || first.requires_approval {
            return Err(Need::Approve);
        }
        Ok(LocalStep { effect: first.effect, action: first.action.clone(), mark: mark.into() })
    }
    pub fn effect(&self) -> Effect {
        self.effect
    }
    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }
    /// 1 手で入れる印（③ はこれを外の値で探す）
    pub fn mark(&self) -> &str {
        &self.mark
    }
}

/// W14b: 読み戻した場所（③）。画面の文字・action の返り（Receipt）・API の返りはここに無い ＝ 自己申告から ready を作れない。
/// Wave: W14b
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outside {
    /// Shibubu の local の DB（ui_state）
    LocalDb,
    /// disk の file
    Disk,
    /// 元の app の AX の値
    AxValue,
}

/// W14b: `Probe::read_back` の答え。
/// Wave: W14b
#[derive(Debug, Clone, PartialEq)]
pub enum Readback {
    /// 1 手とは別の道で読んだ今の値（全体を文字列で）
    Read { via: Outside, value: String },
    /// 読めなかった・まだ無い（理由 1 行）
    Missing(String),
}

/// W14b: 3 問を測る口（製品は generated.rs・試験は偽物）。呼ぶ順は `usable` が決める: reach → read_back（前）→ act → read_back（後）。
/// Wave: W14b
pub trait Probe {
    /// ① 届く
    fn reach(&mut self, seat: &Seat) -> Check;
    /// ② 主たる 1 手。渡るのは `LocalStep` だけ（外に効く手はここに来ない）
    fn act(&mut self, seat: &Seat, step: &LocalStep) -> Check;
    /// ③ 1 手の結果を 1 手とは別の道で読む（local DB・disk・AX の値）。1 手の返り値・画面の文字を返さない
    fn read_back(&mut self, seat: &Seat, step: &LocalStep) -> Readback;
}

/// W14b: 1 席の判定（面ごとの verdict）。
/// Wave: W14b
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Usable {
    pub seat: Seat,
    pub reach: Check,
    pub first_action: Check,
    pub readback: Check,
    /// 直し（測り直し）を 1 回使った
    pub fixed: bool,
}

impl Usable {
    /// 3 問とも Pass の時だけ。
    pub fn ready(&self) -> bool {
        [&self.reach, &self.first_action, &self.readback].iter().all(|c| matches!(c, Check::Pass(_)))
    }
    /// AX の `usable:<seat>` の値: `ready` / `checking`（どれかが Wait）/ `not_ready`。
    pub fn value(&self) -> &'static str {
        if self.ready() {
            "ready"
        } else if [&self.reach, &self.first_action, &self.readback].iter().any(|c| matches!(c, Check::Wait(_))) {
            "checking"
        } else {
            "not_ready"
        }
    }
    /// 画面の 1 行（理由と次の一手）。ready =「<名前> は使えます（<何で読み戻したか>）」・通らない = 頭に `Need::word` ・Wait =「確かめています」。
    /// 成功の語（YC_ACCEPTANCE §4: 済み・完了・✓・保存しました…）を含めない。
    pub fn line(&self) -> String {
        if self.ready() {
            let how = match &self.readback {
                Check::Pass(how) => how.as_str(),
                _ => "",
            };
            return format!("{} works ({how})", self.seat.name);
        }
        for c in [&self.reach, &self.first_action, &self.readback] {
            match c {
                Check::Fail { need, line } => return format!("{}: {line}", need.word()),
                Check::Wait(line) => return format!("Checking: {line}"),
                _ => {}
            }
        }
        format!("{} hasn't been checked yet", self.seat.name)
    }
    /// BuildVerifier の findings の形（check = "usable"・subject = 席の id・message = `line`・Warning）。ready の面は None。
    pub fn finding(&self) -> Option<Finding> {
        (!self.ready()).then(|| Finding { check: "usable", subject: self.seat.id.clone(), message: self.line(), severity: Severity::Warning })
    }
}

/// ③ の場所の画面の語。
fn via_word(via: Outside) -> &'static str {
    match via {
        Outside::LocalDb => "Shibubu's database",
        Outside::Disk => "a file on disk",
        Outside::AxValue => "the value shown in the app",
    }
}

/// ③: 1 手の前と後の外の値から決める（`usable` の中だけ）。
fn judge(seat: &Seat, first: &FirstAction, step: &LocalStep, before: &Readback, after: &Readback) -> Check {
    let retry = |line: String| Check::Fail { need: Need::Retry, line };
    let (via, value) = match after {
        Readback::Read { via, value } => (*via, value),
        Readback::Missing(why) => return retry(format!("Can't read the result of {} from outside after “{}” ({why}). Rebuild it", seat.name, first.what)),
    };
    let done = format!("Read back “{}” from {}", first.what, via_word(via));
    match first.expect {
        Expect::Mark => {
            let mark = step.mark();
            let had = matches!(before, Readback::Read { value, .. } if !mark.is_empty() && value.contains(mark));
            if mark.is_empty() || !value.contains(mark) {
                retry(format!("After “{}”, {} has no mark of what was added. Rebuild it", first.what, via_word(via)))
            } else if had {
                retry(format!("{} already had the mark before “{}” (the read-back didn't see that step). Rebuild it", via_word(via), first.what))
            } else {
                Check::Pass(done)
            }
        }
        Expect::Changed => match before {
            Readback::Read { value: b, .. } if b == value => retry(format!("{} has the same value before and after “{}”. Rebuild it", via_word(via), first.what)),
            _ => Check::Pass(done),
        },
    }
}

/// 承認が要る 1 手の帯の文（頭に `Need::word`〈Needs approval〉が付く）。事実: この手は対象の中身を変える・事前には動かさない・使う時に確かめる。
/// 手の名前が対象の名前を含む時（`Edit excalidraw.com` の excalidraw.com）は対象を 2 度言わない。
/// 保証する: 内部の語（evaluation）を含まない。保証しない: 手の名前の長さ。
/// 例: `approval_line("Edit README.md", "TextEdit")`（手の名前は利用者の頼みの語。Shibubu が試す手の内部の説明〈1 文字打つ〉は出さない）
/// Wave: W15-c1
pub fn approval_line(what: &str, target: &str) -> String {
    if what.to_lowercase().contains(&target.to_lowercase()) {
        format!("“{what}” is a real change, so it's checked only when you use it")
    } else {
        format!("“{what}” changes {target}, so Shibubu checks it only when you use it")
    }
}

/// W14b: 唯一の判定の入口。決定的（時刻・乱数を使わない・印は呼ぶ側が渡す）。
/// 順: ① `reach` が Pass でなければ ②③ は Skipped。② `LocalStep::new` が Err なら Fail{Approve}（probe を呼ばない）・`first` が None なら Fail{Retry}。
/// ③ `read_back` を 1 手の前と後に呼び、`Expect::Mark` は後の値に印が在り前の値に無い・`Expect::Changed` は前と後の値が違う時だけ Pass。Missing と Wait は Pass にしない。
/// 直し（run ごとに 1 回・W14-067 の上限の中＝model を呼ばない）: `fixes` が 1 以上で ① か ② が Fail{Retry} の時だけ、その問いを 1 回測り直して `fixes` を 1 減らす
/// （`fixed = true`・後の答えを残す）。要接続・要承認・許可・入れるは利用者の手が要るので測り直さない。
/// 例: `let mut fixes = 1; let list: Vec<Usable> = seats.iter().map(|s| usable(s, &mut probe, &mark, &mut fixes)).collect();`
/// Wave: W14b
pub fn usable(seat: &Seat, probe: &mut dyn Probe, mark: &str, fixes: &mut u32) -> Usable {
    let mut out = Usable { seat: seat.clone(), reach: Check::Skipped, first_action: Check::Skipped, readback: Check::Skipped, fixed: false };
    let mut fix = |c: &Check, out_fixed: &mut bool| {
        let again = matches!(c, Check::Fail { need: Need::Retry, .. }) && *fixes > 0;
        if again {
            *fixes -= 1;
            *out_fixed = true;
        }
        again
    };
    out.reach = probe.reach(seat);
    if fix(&out.reach, &mut out.fixed) {
        out.reach = probe.reach(seat);
    }
    if !matches!(out.reach, Check::Pass(_)) {
        return out;
    }
    let Some(first) = &seat.first else {
        out.first_action = Check::Fail { need: Need::Retry, line: format!("Can't tell the main step of {}. Add your purpose and rebuild it", seat.name) };
        return out;
    };
    let step = match LocalStep::new(first, mark) {
        Ok(s) => s,
        Err(need) => {
            out.first_action = Check::Fail { need, line: approval_line(&first.what, &seat.name) };
            return out;
        }
    };
    let before = probe.read_back(seat, &step);
    out.first_action = probe.act(seat, &step);
    if fix(&out.first_action, &mut out.fixed) {
        out.first_action = probe.act(seat, &step);
    }
    if !matches!(out.first_action, Check::Pass(_)) {
        return out;
    }
    let after = probe.read_back(seat, &step);
    out.readback = judge(seat, first, &step, &before, &after);
    out
}

/// W14b: 生成 app の席 1 つ（id = app の id・名前 = 題名）。主たる 1 手は `metadata.composer.job` で決める:
/// 記録（log・visualize・memorize）→ add「1 件入れる」・整理（organize・monitor・plan）→ add「1 行足す」・計る（measure）→ toggle「始めて止める」（Changed）・
/// 書く（author）→ save「1 文字入れて保存する」。action は画面の node が指す `local.<source>.<verb>`（effect と承認は定義のまま）。job が無い・その verb が無い時は None。
/// Wave: W14b
pub fn app_seat(def: &AppDefinitionV3) -> Seat {
    use Effect::{ExternalSend, ExternalWrite};
    let job = def.metadata.as_ref().and_then(|m| m.get("composer")).and_then(|c| c.get("job")).and_then(Value::as_str).unwrap_or("");
    // (local の verb, 手の名前, Expect, 押す回数) か、外に効く手（その job の主役は外の物を動かす: 送る・mic・端末・外の editor）
    let local = match job {
        "log" | "visualize" | "memorize" => Some(("add", "Add one item", Expect::Mark, 1)),
        "organize" | "monitor" | "plan" => Some(("add", "Add one row", Expect::Mark, 1)),
        "measure" => Some(("toggle", "Start and stop", Expect::Changed, 2)),
        "author" => Some(("save", "Type one character and save", Expect::Mark, 1)),
        _ => None,
    };
    let outward = match job {
        "communicate" => Some(("Send a reply", ExternalSend)),
        "record" => Some(("Record from the microphone", ExternalWrite)),
        "mirror" => Some(("Control a device", ExternalWrite)),
        "edit_media" => Some(("Edit in an editor", ExternalWrite)),
        _ => None,
    };
    let on_screen: Vec<&str> = def.composition.root.nodes().into_iter().filter_map(|n| n.action_ref.as_deref()).collect();
    let first = match (local, outward) {
        (Some((verb, what, expect, presses)), _) => def
            .actions
            .iter()
            .filter(|a| split_local_id(&a.id).is_some_and(|(_, v)| v == verb))
            .min_by_key(|a| !on_screen.contains(&a.id.as_str())) // 画面の node が指す手を先に
            .map(|a| FirstAction { what: what.into(), action: Some(a.id.clone()), effect: a.effect, requires_approval: a.requires_approval, expect, presses }),
        (None, Some((what, effect))) => Some(FirstAction { what: what.into(), action: None, effect, requires_approval: true, expect: Expect::Mark, presses: 1 }),
        (None, None) => None,
    };
    Seat { id: def.id.clone(), name: def.title.clone().unwrap_or_else(|| def.goal.literal_goal.clone()), kind: SeatKind::Generated, first }
}

/// W14b: Project の席（`terminal_specs` の順 → `surface_specs` の順）。Terminal の 1 手は「1 行打つ」（LocalWrite・Mark）。
/// 本物の app と Web の面の 1 手は元の中身を変える（file に書く・元の app を動かす）ので ExternalWrite・承認あり ＝ ② はいつも「要承認」で呼ばない。
/// 種類は backend で決める（native_direct → NativeApp・mirror → Mirror・webview → Web）。app の名前で分けない（check_app_agnostic）。
/// W13b: その後に、Page の順・木の先行順で `project::SeatV1::asked_in` が Some の node ごとに
/// `Seat{id: その Page の route の名前（/ を落とした slug = 答えで置く面の SurfaceSpec::id）, name: 席の app_name, kind: MissingApp, first: None}`。
/// 答えた後（面を置いた・無しで続けた）の node は 1 問を持たないので席にならない。
/// Wave: W14b, W13b
pub fn project_seats(p: &ProjectDefinitionV1) -> Vec<Seat> {
    let mut out: Vec<Seat> = p
        .terminal_specs
        .iter()
        .map(|t| Seat {
            id: t.id.clone(),
            name: "Terminal".into(),
            kind: SeatKind::Terminal,
            first: Some(FirstAction { what: "Type one line".into(), action: None, effect: Effect::LocalWrite, requires_approval: false, expect: Expect::Mark, presses: 1 }),
        })
        .collect();
    for s in &p.surface_specs {
        // MIRROR-FRAME-1101: 中身の無い Mirror の枠は席にしない（app が無い = 1 手が無い・名前の無い行を使える検査に出さない）
        if s.blank_frame() {
            continue;
        }
        let (name, what) = match &s.target {
            SurfaceTarget::App { bundle_id, file } => {
                let name = bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_string();
                let what = match file {
                    Some(f) => format!("Edit {f}"),
                    None => format!("Change the contents of {name}"),
                };
                (name, what)
            }
            SurfaceTarget::Url { url } => {
                // host だけ（scheme・userinfo・path を落とす）
                let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
                let host = rest.split(['/', '?', '#']).next().unwrap_or(rest).rsplit('@').next().unwrap_or(rest).to_string();
                let what = format!("Edit {host}");
                (host, what)
            }
        };
        let kind = match s.backend {
            SurfaceBackendKind::NativeDirect => SeatKind::NativeApp,
            SurfaceBackendKind::Mirror => SeatKind::Mirror,
            SurfaceBackendKind::WebView => SeatKind::Web,
        };
        out.push(Seat { id: s.id.clone(), name, kind, first: Some(FirstAction { what, action: None, effect: Effect::ExternalWrite, requires_approval: true, expect: Expect::Mark, presses: 1 }) });
    }
    for page in &p.pages {
        for n in page.root.nodes() {
            let Some(asked) = SeatV1::asked_in(n) else { continue };
            // G88a: 1 Page に 1 問が複数でも席ごと（node の key `<席の slug>-next`）。前の版の 1 席 1 Page は route の名前と同じ
            let id = n.key.strip_suffix("-next").unwrap_or(page.route.trim_start_matches('/')).to_string();
            out.push(Seat { id, name: asked.app_name.unwrap_or_default(), kind: SeatKind::MissingApp, first: None });
        }
    }
    out
}

#[cfg(test)]
mod w14b_interface {
    use super::*;

    fn first(effect: Effect, requires_approval: bool) -> FirstAction {
        FirstAction { what: "1 件入れる".into(), action: Some("local.books.add".into()), effect, requires_approval, expect: Expect::Mark, presses: 1 }
    }

    #[test]
    fn w14b_signatures() {
        let _: fn(&Seat, &mut dyn Probe, &str, &mut u32) -> Usable = usable;
        let _: fn(&AppDefinitionV3) -> Seat = app_seat;
        let _: fn(&ProjectDefinitionV1) -> Vec<Seat> = project_seats;
    }

    /// 型の縛り: 外に効く手と承認の要る手は LocalStep にならない（`Probe::act` に渡せない）。
    #[test]
    fn w14b_local_step_refuses_outward_and_approval() {
        for e in [Effect::LocalRead, Effect::LocalWrite, Effect::ExternalRead] {
            let s = LocalStep::new(&first(e, false), "m").unwrap_or_else(|n| panic!("{e:?} は評価で動かしてよい: {n:?}"));
            assert_eq!((s.effect(), s.action(), s.mark()), (e, Some("local.books.add"), "m"));
        }
        for e in [Effect::ExternalWrite, Effect::ExternalSend, Effect::Destructive] {
            assert_eq!(LocalStep::new(&first(e, false), "m"), Err(Need::Approve), "{e:?} を評価で動かせてしまう");
        }
        assert_eq!(LocalStep::new(&first(Effect::LocalWrite, true), "m"), Err(Need::Approve), "承認の要る local の手を評価で動かせてしまう");
    }
}

/// W14b の判定の試験（偽の Probe）。3 問それぞれの負の対照は、この試験を赤にする実装の形で取る（docs/YC_WAVES.md の W14b の行）:
/// (b) 外に効く手を評価で呼ぶ実装 → `w14b_outward_first_action_is_never_called` が赤 ・ (c) 読み戻し無しで ready → `w14b_ready_needs_an_outside_readback_with_the_mark` が赤。
/// (a) 自己申告だけで ready（1 手の返りを読み戻しにする probe）は製品の probe の試験（generated.rs の `w14b_app_probe_reads_the_db_not_the_receipt`）。
#[cfg(test)]
mod w14b_tests {
    use super::*;
    use std::collections::VecDeque;

    /// W15-c1（r20 の AX: 承認の帯が長く内部の語 `evaluation` を言い、excalidraw.com を 2 度言う）: 帯は短く・事実（中身を変える・事前に動かさない・使う時に確かめる）は残し・
    /// 手の名前が対象を含む時は対象を重ねない。負の対照: 元の文に戻す → 1 行目〈evaluation・長さ〉が赤・重ねる腕に戻す → 3 行目が赤。
    #[test]
    fn w15c1_approval_band_is_short_plain_and_names_the_target_once() {
        for (what, target) in [("Edit README.md", "TextEdit"), ("Edit excalidraw.com", "excalidraw.com")] {
            let line = format!("{}: {}", Need::Approve.word(), approval_line(what, target));
            assert!(!line.to_lowercase().contains("evaluation") && line.chars().count() <= 120, "{line} ({})", line.chars().count());
            assert!(line.starts_with("Needs approval: “") && line.contains("only when you use it"), "承認が要る事と使う時に確かめる事は残す: {line}");
            assert_eq!(line.matches(target).count(), 1, "対象と動作が同じ語で重なる: {line}");
        }
        assert!(approval_line("Edit README.md", "TextEdit").contains("changes TextEdit"), "対象を言う");
    }

    /// 呼ばれた順を記録し、答えを順に返す偽の Probe。
    #[derive(Default)]
    struct Fake {
        reach: VecDeque<Check>,
        act: VecDeque<Check>,
        reads: VecDeque<Readback>,
        calls: Vec<String>,
    }

    impl Probe for Fake {
        fn reach(&mut self, seat: &Seat) -> Check {
            self.calls.push(format!("reach {}", seat.id));
            self.reach.pop_front().unwrap_or(Check::Pass("届いた".into()))
        }
        fn act(&mut self, seat: &Seat, step: &LocalStep) -> Check {
            self.calls.push(format!("act {} {:?}", seat.id, step.effect()));
            self.act.pop_front().unwrap_or(Check::Pass("押した".into()))
        }
        fn read_back(&mut self, seat: &Seat, _step: &LocalStep) -> Readback {
            self.calls.push(format!("read {}", seat.id));
            self.reads.pop_front().unwrap_or(Readback::Missing("無い".into()))
        }
    }

    fn seat(effect: Effect, approval: bool, expect: Expect) -> Seat {
        Seat {
            id: "s".into(),
            name: "読書記録".into(),
            kind: SeatKind::Generated,
            first: Some(FirstAction { what: "1 件入れる".into(), action: Some("local.books.add".into()), effect, requires_approval: approval, expect, presses: 1 }),
        }
    }

    fn db(v: &str) -> Readback {
        Readback::Read { via: Outside::LocalDb, value: v.into() }
    }

    /// (b) 外に効く手（送る・外への書き込み・消す・承認の要る手）は probe.act に届かず「要承認」・読み戻しも呼ばない。
    #[test]
    fn w14b_outward_first_action_is_never_called() {
        for (e, approval) in [(Effect::ExternalSend, true), (Effect::ExternalWrite, true), (Effect::Destructive, false), (Effect::ExternalWrite, false), (Effect::LocalWrite, true)] {
            let mut p = Fake::default();
            let u = usable(&seat(e, approval, Expect::Mark), &mut p, "m-1", &mut 1);
            assert_eq!(p.calls, ["reach s"], "{e:?}: 外に効く手を評価で呼んだ・読んだ: {:?}", p.calls);
            assert!(matches!(u.first_action, Check::Fail { need: Need::Approve, .. }), "{e:?}: {u:?}");
            assert!(!u.ready() && u.value() == "not_ready" && u.line().starts_with("Needs approval: "), "{e:?}: {}", u.line());
            assert_eq!(u.readback, Check::Skipped);
        }
    }

    /// (c) ready は ③ が外の値（Read）で印を見つけた時だけ。読み戻しが無い・印が無い・前から在る は ready にしない。
    #[test]
    fn w14b_ready_needs_an_outside_readback_with_the_mark() {
        let run = |reads: Vec<Readback>| {
            let mut p = Fake { reads: reads.into(), ..Fake::default() };
            let u = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m-1", &mut 0);
            (u, p.calls)
        };
        let (u, calls) = run(vec![Readback::Missing("まだ無い".into()), db(r#"{"items":[{"title":"m-1"}]}"#)]);
        assert!(u.ready(), "{u:?}");
        assert_eq!(calls, ["reach s", "read s", "act s LocalWrite", "read s"], "前 → 1 手 → 後 の順に読む");
        assert_eq!(u.line(), "読書記録 works (Read back “1 件入れる” from Shibubu's database)");
        assert!(u.finding().is_none());
        for (why, reads) in [
            ("読み戻し無し", vec![Readback::Missing("無い".into()), Readback::Missing("読めない".into())]),
            ("印の無い値", vec![Readback::Missing("無い".into()), db(r#"{"items":[]}"#)]),
            ("前から印が在る", vec![db("m-1"), db("m-1")]),
        ] {
            let (u, _) = run(reads);
            assert!(!u.ready(), "{why} で ready: {u:?}");
            assert!(matches!(u.readback, Check::Fail { need: Need::Retry, .. }), "{why}: {u:?}");
            let f = u.finding().expect("通らない面は finding");
            assert_eq!((f.check, f.subject.as_str(), f.severity), ("usable", "s", Severity::Warning));
            assert!(f.message.starts_with("Rebuild: "), "{why}: {}", f.message);
        }
        // Changed: 前と後が違えば通る・同じなら通らない
        let changed = |b: Readback, a: Readback| {
            let mut p = Fake { reads: vec![b, a].into(), ..Fake::default() };
            usable(&seat(Effect::LocalWrite, false, Expect::Changed), &mut p, "", &mut 0).ready()
        };
        assert!(changed(db(r#"{"running":false}"#), db(r#"{"running":false,"remaining_ms":1499}"#)));
        assert!(changed(Readback::Missing("無い".into()), db("{}")));
        assert!(!changed(db("{}"), db("{}")), "前と後が同じで ready");
        assert!(!changed(db("{}"), Readback::Missing("無い".into())), "後が読めないのに ready");
    }

    /// ① で止まった面は ②③ を測らない。Wait は checking。
    #[test]
    fn w14b_reach_failure_and_wait_stop_the_rest() {
        let mut p = Fake { reach: vec![Check::Fail { need: Need::Install, line: "TextEdit がこの Mac に入っていません".into() }].into(), ..Fake::default() };
        let u = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m", &mut 1);
        assert_eq!(p.calls, ["reach s"], "入れるは測り直さない: {:?}", p.calls);
        assert_eq!((u.first_action.clone(), u.readback.clone(), u.fixed), (Check::Skipped, Check::Skipped, false));
        assert_eq!(u.line(), "Install: TextEdit がこの Mac に入っていません");
        let mut p = Fake { reach: vec![Check::Wait("この Page を開くと読み込みます".into())].into(), ..Fake::default() };
        let u = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m", &mut 1);
        assert_eq!((u.value(), u.line().as_str()), ("checking", "Checking: この Page を開くと読み込みます"));
        let none = Seat { first: None, ..seat(Effect::LocalWrite, false, Expect::Mark) };
        let u = usable(&none, &mut Fake::default(), "m", &mut 1);
        assert!(matches!(u.first_action, Check::Fail { need: Need::Retry, .. }), "{u:?}");
    }

    /// 直しは run ごとに 1 回: Retry の ① か ② を 1 回だけ測り直す（2 席目は直さない）。
    #[test]
    fn w14b_one_fix_per_run() {
        let flaky = || Check::Fail { need: Need::Retry, line: "prompt が返りません".into() };
        let mut p = Fake { reach: vec![flaky(), Check::Pass("2 回目".into()), flaky()].into(), reads: vec![Readback::Missing("無い".into()), db("m"), Readback::Missing("無い".into()), db("m")].into(), ..Fake::default() };
        let mut fixes = 1;
        let a = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m", &mut fixes);
        assert!(a.ready() && a.fixed, "1 回目の直しで通る: {a:?}");
        let b = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m", &mut fixes);
        assert!(!b.ready() && !b.fixed && fixes == 0, "2 回目は直さない: {b:?}");
        assert_eq!(p.calls.iter().filter(|c| c.starts_with("reach")).count(), 3);
        // ② の Retry も同じ予算
        let mut p = Fake { act: vec![flaky(), Check::Pass("押した".into())].into(), reads: vec![Readback::Missing("無い".into()), db("m")].into(), ..Fake::default() };
        let u = usable(&seat(Effect::LocalWrite, false, Expect::Mark), &mut p, "m", &mut 1);
        assert!(u.ready() && u.fixed, "{u:?}");
    }

    /// 席の作り方: 生成 app は job の 1 手（画面が指す action）・Project は Terminal → 面の順・面の 1 手は外への書き込み（要承認）。
    #[test]
    fn w14b_seats_from_definitions() {
        use super::super::{compose, ComposeContext};
        let book = compose("読書記録をつけたい", &ComposeContext::default()).unwrap().into_app();
        let s = app_seat(&book);
        let f = s.first.clone().expect("記録の 1 手");
        assert!(f.action.as_deref().is_some_and(|a| a.ends_with(".add")) && f.effect == Effect::LocalWrite && !f.requires_approval && f.expect == Expect::Mark, "{f:?}");
        assert_eq!((s.kind, s.id.as_str()), (SeatKind::Generated, book.id.as_str()));
        let timer = compose("タイマーを作って", &ComposeContext::default()).unwrap().into_app();
        let f = app_seat(&timer).first.expect("計るの 1 手");
        assert!(f.action.as_deref().is_some_and(|a| a.ends_with(".toggle")) && f.expect == Expect::Changed && f.presses == 2, "{f:?}");
        let p = super::super::project::compose_project("この repo の開発環境を作って", &ComposeContext::default(), Some(std::path::Path::new("/tmp")))
            .unwrap()
            .expect("主デモ")
            .project;
        let seats = project_seats(&p);
        let got: Vec<(&str, SeatKind, &str)> = seats.iter().map(|s| (s.id.as_str(), s.kind, s.name.as_str())).collect();
        // M3: Docs の TextEdit の席は feature `mirror` の時は Mirror（`project::APP_SEAT_BACKEND`）・無い時は NativeDirect
        let app_seat = if cfg!(feature = "mirror") { SeatKind::Mirror } else { SeatKind::NativeApp };
        assert_eq!(got[1..], [("docs", app_seat, "TextEdit"), ("diagram", SeatKind::Web, "excalidraw.com")]);
        assert_eq!((got[0].1, got[0].2), (SeatKind::Terminal, "Terminal"));
        assert!(seats[1..].iter().all(|s| s.first.as_ref().is_some_and(|f| LocalStep::new(f, "m") == Err(Need::Approve))), "面の 1 手は評価で呼べない");
        // W15-c3: 帯が言う手の名前は利用者の頼みの語（Shibubu が試す手〈1 文字打って保存〉の内部の説明を出さない）。負の対照: 元の `Type one character into {f} and save` に戻す → 赤
        for s in &seats[1..] {
            let f = s.first.as_ref().unwrap();
            let line = approval_line(&f.what, &s.name);
            assert!(f.what.starts_with("Edit ") && !line.contains("character") && !line.contains("and save"), "帯に試しの手の説明: {line}");
        }
        assert!(LocalStep::new(seats[0].first.as_ref().unwrap(), "m").is_ok(), "Terminal の 1 手は local");
    }
}
