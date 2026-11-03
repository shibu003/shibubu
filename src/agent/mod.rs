//! agent（TODO.md AGT-001..014 の unit A）: 提案の生成 → 根拠の検査 → 順位付け → 静かな queue → rail、
//! と適用の transaction（revision の照合 → scope → policy → domain::patch::apply → 再検査 → 落ちたら元のまま）。
//! LLM は呼ばない（ローカルの heuristics だけ）。domain にしか依存しない — tests/agent.rs が `#[path]` で取り込むので
//! `crate::` を書かない（domain は親 module の sibling）。egui との接点は src/ui/agent_rail.rs（`impl AgentRailSource for Agent`）。
//!
//! 入口は `Agent` 1 つ: app.rs が `Intent::Proposal { id, act }` を `Agent::act` に、`Intent::AgentAsk` を `Agent::ask` に渡し、
//! 返った `Applied.revision` を保存する。rail には `rail_rows` / `about_rows` / `history_lines` / `undo_rows` を読ませる。
use super::domain;

pub mod apply;
pub mod candidate;
pub mod grounding;
pub mod policy;
pub mod queue;

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use domain::action::InvocationError;
use domain::app::AutonomyLevel;
use domain::patch::{AppRevision, PatchError, PatchOp};
use domain::permission::{ConsentReceipt, PermissionGrant};
use domain::run::TransitionError;
use domain::suggestion::{EvidenceKind, Risk, Suggestion, SuggestionKind, SuggestionStatus};
use domain::validate::{validate_grant, ValidationError};

pub use apply::{Applied, ApplyError, Preview, PreviewCta};
pub use candidate::{ActionError, Candidate, Category, Observed};
pub use feedback::Feedback;
pub use grounding::GroundingError;
pub use lifecycle::Hold;
pub use policy::{Decision, Policy};
pub use queue::{Intake, Queue, Tracked, MAX_SHOWN};

/// rail の操作（ui::agent_rail::ProposalAct と 1:1。ui 側が写す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Preview,
    Apply,
    /// snooze（1 日）。
    Later,
    /// 非表示（持続）。
    Hide,
    /// 適用済みを元の構成に戻す。
    Revert,
    /// プレビュー面を閉じる（status は変えない）。
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Acted {
    Previewed(Preview),
    Applied(Applied),
    Snoozed { until: String },
    Hidden,
    /// 適用済みを戻した新しい revision（`version`）が `Agent::app()` に在る。呼び側はこれを保存する。
    Reverted { version: u32 },
    /// 落ちた（failed）適用の記録を閉じた。構成は変えていないので保存する物は無い（revision を足さない）。
    FailureClosed,
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    Unknown(String),
    NoApp,
    Apply(ApplyError),
    Transition(TransitionError<SuggestionStatus>),
    /// 取り消しの元の revision が今の構成と合わない（後の変更がある）。
    RevertConflict { expected: u32, found: u32 },
    NotRevertible(String),
    BadTime(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "There's no suggestion '{id}'"),
            Self::NoApp => write!(f, "No app is open"),
            Self::Apply(e) => write!(f, "{e}"),
            Self::Transition(e) => write!(f, "{e}"),
            Self::RevertConflict { expected, found } => {
                write!(f, "The setup has changed since then (undo works for v{expected}, but it is v{found} now)")
            }
            Self::NotRevertible(s) => write!(f, "{s}"),
            Self::BadTime(s) => write!(f, "Can't read the time '{s}' (RFC 3339 UTC)"),
        }
    }
}
impl std::error::Error for AgentError {}

impl From<ApplyError> for AgentError {
    fn from(e: ApplyError) -> Self {
        AgentError::Apply(e)
    }
}
impl From<TransitionError<SuggestionStatus>> for AgentError {
    fn from(e: TransitionError<SuggestionStatus>) -> Self {
        AgentError::Transition(e)
    }
}

/// rail の 1 行（§5.2）。ui::agent_rail が Proposal に写す。
#[derive(Debug, Clone, PartialEq)]
pub struct RailRow {
    pub id: String,
    pub title: String,
    pub reason: String,
    /// `提案` / `要接続` / `承認が必要` / `読み取りのみ`
    pub labels: Vec<String>,
    /// true = `[適用]`、false = `[プレビュー]`
    pub primary_apply: bool,
    pub evidence: Vec<String>,
    pub changes: Vec<String>,
    pub scopes: Vec<String>,
    pub undo: String,
    pub confidence: Option<String>,
}

struct Undo {
    suggestion_id: String,
    title: String,
    before: AppRevision,
    after_version: u32,
    /// 適用した直後の構成の hash（apply::base_hash）。同じ版のまま利用者が部品を固定した（set_app）後に、before で上書きしない（A-R3-1）
    after_hash: String,
}

/// 追加の検査（renderer が描けるか等）。patch 後の定義を見て Err(理由) なら適用は失敗して元に戻る。
pub type Check = Box<dyn Fn(&domain::app::AppDefinitionV3) -> Result<(), String>>;

pub struct Agent {
    app: Option<AppRevision>,
    grants: Vec<PermissionGrant>,
    consents: Vec<ConsentReceipt>,
    policy: Policy,
    feedback: Feedback,
    queue: Queue,
    observed: Observed,
    undo: Vec<Undo>,
    preview: Option<Preview>,
    history: Vec<String>,
    checks: Vec<Check>,
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent {
    pub fn new() -> Self {
        Agent {
            app: None,
            grants: Vec::new(),
            consents: Vec::new(),
            policy: Policy::default(),
            feedback: Feedback::default(),
            queue: Queue::new(),
            observed: Observed::default(),
            undo: Vec::new(),
            preview: None,
            history: Vec::new(),
            checks: Vec::new(),
        }
    }

    /// 保存してあった feedback（非表示 / snooze）から。
    pub fn with_feedback(feedback: Feedback) -> Self {
        Agent { feedback, ..Self::new() }
    }

    // ---- app / 観測 ------------------------------------------------------

    pub fn app(&self) -> Option<&AppRevision> {
        self.app.as_ref()
    }

    /// 今開いている app の id。queue の提案はこれと id の組で引く。
    fn app_id(&self) -> Option<String> {
        self.app.as_ref().map(|a| a.definition().id.clone())
    }

    /// 開いた app（検証済みの revision）。別の app に切り替えた時だけ autonomy をその app の宣言に戻し、観測と取り消しの記録を捨てる
    /// （同じ app の新しい revision ならユーザーが選んだ autonomy を保つ）。版が進んだ提案と、根拠（観測した error / 入力）が
    /// 消えた提案は退ける（§5.3 stale / withdrawn。refresh で今の構成から作り直す）。
    pub fn set_app(&mut self, rev: AppRevision) {
        let def = rev.definition();
        if self.app.as_ref().map(|a| a.definition().id.as_str()) != Some(def.id.as_str()) {
            self.policy.autonomy = def.agent.autonomy_level;
            self.policy.enabled = def.agent.suggestions_enabled;
            self.observed = Observed::default();
            self.undo.clear();
        }
        self.queue.retire_stale(&def.id, def.version);
        let known = self.observed.known_refs();
        self.queue.retire_if(&def.id, |s| grounding::check(def, s, &known).is_err(), "The evidence is gone");
        self.preview = None;
        self.app = Some(rev);
    }

    pub fn clear_app(&mut self) {
        self.app = None;
        self.preview = None;
    }

    /// renderer / runtime が見た事（件数・error・入力）。agent は自分で source を読まない。
    pub fn observe(&mut self, observed: Observed) {
        self.observed.merge(observed);
    }

    pub fn observe_error(&mut self, action_ref: &str, error: InvocationError) {
        self.observe(Observed { errors: vec![ActionError { action_ref: action_ref.to_owned(), error }], ..Default::default() });
    }

    pub fn add_check(&mut self, check: Check) {
        self.checks.push(check);
    }

    // ---- 候補 → queue -------------------------------------------------------

    /// heuristics を回して queue に入れる。戻り値は新しく queued になった数。
    pub fn refresh(&mut self, now: &str) -> usize {
        if !self.policy.enabled || self.policy.paused {
            return 0;
        }
        let Some(app) = &self.app else { return 0 };
        let candidates = candidate::generate(app.definition(), &self.observed);
        self.intake_all(candidates, now)
    }

    /// 外から来た提案（将来の LLM 等）の唯一の入口: 根拠の検査を通らなければ queue にも apply にも進まない（AGT-005）。
    pub fn submit(&mut self, suggestion: Suggestion, category: Category, now: &str) -> Result<Intake, Vec<GroundingError>> {
        let app = self.app.as_ref().ok_or_else(Vec::new)?;
        let t = self.track(app, Candidate { suggestion, category }, now)?;
        Ok(self.queue.intake(t, &self.feedback, now))
    }

    fn intake_all(&mut self, candidates: Vec<Candidate>, now: &str) -> usize {
        let Some(app) = self.app.clone() else { return 0 };
        let mut n = 0;
        for c in candidates {
            if let Ok(t) = self.track(&app, c, now) {
                if matches!(self.queue.intake(t, &self.feedback, now), Intake::Queued | Intake::QueuedAs(_)) {
                    n += 1;
                }
            }
        }
        n
    }

    fn track(&self, app: &AppRevision, c: Candidate, now: &str) -> Result<Tracked, Vec<GroundingError>> {
        let def = app.definition();
        let grounding = grounding::check(def, &c.suggestion, &self.observed.known_refs())?;
        let key = ranker::dedupe_key(&c.suggestion, c.category);
        let score = ranker::score(def, &c.suggestion, &grounding, c.category, self.feedback.preference(c.category));
        Ok(Tracked {
            suggestion: c.suggestion,
            category: c.category,
            key,
            score,
            grounding,
            resume_at: None,
            created_at: now.to_owned(),
            note: None,
        })
    }

    /// 毎 frame の頭で安く: snooze 明け・期限切れ。
    pub fn tick(&mut self, now: &str) {
        self.queue.wake(now);
        self.queue.expire(now);
        self.feedback.prune(now);
    }

    /// rail が実際に描いた時に呼ぶ（§5.3 presented は見えた時だけ）。上から `n` 件（MAX_SHOWN まで）を queued → shown（app.rs: Chat の tab = MAX_SHOWN・
    /// Terminal の tab で Engine の card が無い時の欄の上 = 1・CHAT-011 反証 Y1）。
    pub fn mark_shown(&mut self, n: usize) {
        let Some(app_id) = self.app_id() else { return };
        let ids: Vec<String> = self.visible().into_iter().take(n.min(MAX_SHOWN)).map(|t| t.suggestion.id.clone()).collect();
        for id in ids {
            if let Some(t) = self.queue.get_mut(&app_id, &id) {
                if t.suggestion.status == SuggestionStatus::Queued {
                    let _ = lifecycle::show(t);
                }
            }
        }
    }

    /// 今の app の、見える提案（queued / shown / previewed）を score 順に。rail は上から MAX_SHOWN 件。
    pub fn visible(&self) -> Vec<&Tracked> {
        match &self.app {
            Some(app) => self.queue.visible(&app.definition().id),
            None => Vec::new(),
        }
    }

    /// header の `提案` の数 = 見える提案の数（生成した候補の総数ではない。PROACTIVE §7）。
    pub fn eligible_count(&self) -> usize {
        self.visible().len()
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// rail の入力。文は untrusted: どの heuristics を回すかを選ぶだけで、権限・grant・action の実行には繋がらない（AGT-014）。
    pub fn ask(&mut self, text: &str, now: &str) -> String {
        let text = text.trim();
        if text.is_empty() {
            return "Write in one line what you want to change and how".into();
        }
        let Some(app) = self.app.clone() else {
            return "Open an app first, then ask (from Home or Apps)".into();
        };
        if self.policy.paused {
            return "The agent is paused (suggestions come back when you resume it)".into();
        }
        self.observed.requests.push(text.to_owned());
        let index = self.observed.requests.len() - 1;
        let candidates = candidate::from_text(app.definition(), &self.observed, index);
        let total = candidates.len();
        // 手で固定した部品を変える候補は根拠の検査で落ちる（CD-29）。全部がそれなら「同じ提案がある」とは言わない
        let pinned = total > 0 && candidates.iter().all(|c| !grounding::lock_errors(app.definition(), &c.suggestion).is_empty());
        let n = self.intake_all(candidates, now);
        if total == 0 {
            format!("No suggestion can be made from this request (what it can do now: {}). Permissions and sending are never changed from a sentence", candidate::TEXT_COMMANDS.join(" / "))
        } else if n == 0 && pinned {
            "This part was pinned by hand, so it isn't changed".into()
        } else if n == 0 {
            "That suggestion already exists (hidden ones come back when you delete the history)".into()
        } else {
            format!("Added {n} suggestions to “What you can do next”")
        }
    }

    // ---- 操作 -----------------------------------------------------------------

    pub fn act(&mut self, id: &str, act: Act, now: &str) -> Result<Acted, AgentError> {
        // 今開いている app の提案だけ（queue は (app, id) で引く: 別の app の提案を今の構成に当てたり、その差分を見せたりしない）
        let app_id = self.app_id().ok_or_else(|| AgentError::Unknown(id.into()))?;
        if self.queue.get(&app_id, id).is_none() {
            return Err(AgentError::Unknown(id.into()));
        }
        match act {
            Act::Close => {
                self.preview = None;
                Ok(Acted::Closed)
            }
            Act::Preview => {
                let app = self.app.clone().ok_or(AgentError::NoApp)?;
                let t = self.queue.get(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
                // 表に無い遷移はここで Err（写しで確かめるので状態は変えない）。既に previewed なら開き直すだけ
                lifecycle::preview(&mut t.clone())?;
                let cta = match self.decision(t, now, true) {
                    Decision::Proceed | Decision::AutoApply => PreviewCta::Apply,
                    // 外部に効く変更: このプレビューで見せた内容への承認として押してもらう（apply_one が digest 付きで記録）
                    Decision::NeedsConsent => PreviewCta::Approve,
                    other => PreviewCta::Blocked(other.to_string()),
                };
                // patch が当たらなければ Err で、見せていない提案を previewed にしない
                let preview = apply::preview(&app, t, cta)?;
                let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
                lifecycle::preview(t)?;
                self.preview = Some(preview.clone());
                Ok(Acted::Previewed(preview))
            }
            Act::Later => {
                let until = add_days(now, 1).ok_or_else(|| AgentError::BadTime(now.into()))?;
                let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
                lifecycle::snooze(t, &until)?;
                let (key, category, title) = (t.key.clone(), t.category, t.suggestion.title.clone());
                self.feedback.snooze(&key, category, &until, now);
                self.close_preview_of(id);
                self.history.push(format!("{now} Later (resumes at {until}): {title}"));
                Ok(Acted::Snoozed { until })
            }
            Act::Hide => {
                let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
                lifecycle::reject(t, "Hidden")?;
                let (key, category, title) = (t.key.clone(), t.category, t.suggestion.title.clone());
                self.feedback.hide(&key, category, now);
                self.close_preview_of(id);
                self.history.push(format!("{now} Hidden: {title}"));
                Ok(Acted::Hidden)
            }
            Act::Apply => self.apply_one(id, now, true).map(Acted::Applied),
            Act::Revert => self.revert(id, now),
        }
    }

    fn close_preview_of(&mut self, id: &str) {
        if self.preview.as_ref().is_some_and(|p| p.id == id) {
            self.preview = None;
        }
    }

    fn decision(&self, t: &Tracked, now: &str, explicit: bool) -> Decision {
        let Some(app) = &self.app else { return Decision::Blocked(AgentError::NoApp.to_string()) };
        policy::decide(&self.policy, app.definition(), &t.suggestion, &t.grounding, &self.grants, &self.consents, now, explicit)
    }

    /// 適用の transaction（AGT-011）。順: revision の照合 → 根拠 → policy（scope / 承認）→ approved → patch → 再検査 → verified。
    /// 途中で落ちたら構成は元のまま。stale な提案は退ける（refresh で作り直す）。
    fn apply_one(&mut self, id: &str, now: &str, explicit: bool) -> Result<Applied, AgentError> {
        let app = self.app.clone().ok_or(AgentError::NoApp)?;
        let def = app.definition();
        let app_id = def.id.clone();
        let known = self.observed.known_refs();
        let fresh = {
            let t = self.queue.get(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
            if lifecycle::hold(def, &t.suggestion, &self.grants, now) == Some(lifecycle::Hold::Stale) {
                let err = ApplyError::Stale { expected: def.version, found: t.suggestion.app_version };
                if let Some(t) = self.queue.get_mut(&app_id, id) {
                    let _ = lifecycle::reject(t, "The setup was updated");
                }
                return Err(err.into());
            }
            grounding::check(def, &t.suggestion, &known).map_err(ApplyError::NotGrounded)?
        };
        let consent = {
            let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
            // 判定は今の構成で導き直した根拠で（受け入れた時の物を使い回さない）
            t.grounding = fresh;
            if t.suggestion.kind == SuggestionKind::ConnectionRequest || t.suggestion.change.operations.is_empty() {
                return Err(ApplyError::NotApplicable("This suggestion doesn't change the setup (connect from Connections)".into()).into());
            }
            // §5.3: 外部に効く変更の approved は「見せた exact な変更への承認」だけ。プレビューを開いた（previewed）後に
            // ユーザーが押した時にだけ、その変更（版・operations・実効 risk・権限）の digest に結び付けて記録する。
            // auto（explicit = false）は決して記録しない。
            let shown = t.suggestion.status == SuggestionStatus::Previewed;
            let external = grounding::risk_rank(t.grounding.effective_risk) >= grounding::risk_rank(Risk::ExternalWrite);
            (explicit && shown && external && !policy::has_consent(&self.consents, &t.suggestion, &t.grounding))
                .then(|| policy::consent_for(&t.suggestion, &t.grounding, now))
        };
        if let Some(receipt) = consent {
            self.consent(receipt);
        }
        {
            let t = self.queue.get(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
            match self.decision(t, now, explicit) {
                Decision::AutoApply | Decision::Proceed => {}
                other => return Err(ApplyError::Policy(other).into()),
            }
        }
        let checks = &self.checks;
        let run_checks = |d: &domain::app::AppDefinitionV3| checks.iter().try_for_each(|c| c(d));
        let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
        let applied = apply::transaction(&app, t, &run_checks)?;
        let (key, category, title) = (t.key.clone(), t.category, t.suggestion.title.clone());
        self.undo.push(Undo {
            suggestion_id: id.to_owned(),
            title: title.clone(),
            before: app,
            after_version: applied.revision.version(),
            after_hash: apply::base_hash(&applied.revision),
        });
        self.app = Some(applied.revision.clone());
        self.queue.retire_stale(&app_id, applied.revision.version());
        self.feedback.accept(&key, category, now);
        self.preview = None;
        self.history.push(format!("{now} Applied v{}→v{}: {title}", applied.base_version, applied.revision.version()));
        // 版が進んで残りは stale として退けたので、今の構成から作り直す
        self.refresh(now);
        Ok(applied)
    }

    /// 自律度が許す物だけを提案だけで当てる（A1: ローカルで取り消せる UI patch / A2: それに加えて許可済み scope の読み取り）。
    /// 外部に書く・送る・壊す物は決して当たらない（AGT-008）。`user_busy`（入力中・drag 中・文字選択中）の間は何もしない:
    /// 焦点の部品が動かないよう patch は操作が終わるまで待つ（DESIGN §5.4 / AGT-006）。
    pub fn auto_apply(&mut self, now: &str, user_busy: bool) -> Vec<Applied> {
        let mut out = Vec::new();
        if user_busy {
            return out;
        }
        // 1 件当てるたびに apply_one が今の構成から作り直す。上限は queue の上限と同じ（提案同士が打ち消し合っても止まる）
        for _ in 0..queue::CAP_PER_APP {
            let next = self
                .visible()
                .into_iter()
                .find(|t| self.decision(t, now, false) == Decision::AutoApply)
                .map(|t| t.suggestion.id.clone());
            let Some(id) = next else { break };
            match self.apply_one(&id, now, false) {
                Ok(a) => out.push(a),
                Err(_) => break,
            }
        }
        out
    }

    /// 適用済みを元の構成に戻す（snapshot を version+1 で）→ `Acted::Reverted`（新しい revision を保存する）。
    /// failed の提案は記録を閉じるだけ → `Acted::FailureClosed`（構成は変えていないので保存しない）。
    pub fn revert(&mut self, id: &str, now: &str) -> Result<Acted, AgentError> {
        let current = self.app.clone().ok_or(AgentError::NoApp)?;
        let app_id = current.definition().id.clone();
        let status = self.queue.get(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?.suggestion.status;
        if status == SuggestionStatus::Failed {
            let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
            lifecycle::revert(t)?;
            // 落ちた提案を閉じた: 同じ物を作り直してまた落とさない
            let (key, category, title) = (t.key.clone(), t.category, t.suggestion.title.clone());
            self.feedback.revert(&key, category, now);
            self.history.push(format!("{now} Closed the record of the failed apply (the layout stays at v{}): {title}", current.version()));
            return Ok(Acted::FailureClosed);
        }
        if status != SuggestionStatus::Verified {
            return Err(AgentError::Transition(TransitionError { from: status, to: SuggestionStatus::Reverted }));
        }
        let pos = self
            .undo
            .iter()
            .rposition(|u| u.suggestion_id == id)
            .ok_or_else(|| AgentError::NotRevertible("There's no record of applying this suggestion".into()))?;
        if self.undo[pos].after_version != current.version() {
            return Err(AgentError::RevertConflict { expected: self.undo[pos].after_version, found: current.version() });
        }
        // 版が同じでも中身が適用の直後と違う（利用者の固定など）なら戻さない: before を丸ごと戻すと手で固定した部品と印が消える（CD-29）
        if apply::base_hash(&current) != self.undo[pos].after_hash {
            return Err(AgentError::NotRevertible("The setup changed after it was applied, so it isn't undone (to protect parts pinned by hand)".into()));
        }
        let mut def = self.undo[pos].before.definition().clone();
        def.version = current.version() + 1;
        let restored = AppRevision::new(def).map_err(|e| AgentError::Apply(ApplyError::Patch(PatchError::Invalid(e))))?;
        let t = self.queue.get_mut(&app_id, id).ok_or_else(|| AgentError::Unknown(id.into()))?;
        lifecycle::revert(t)?;
        let (key, category, title) = (t.key.clone(), t.category, t.suggestion.title.clone());
        let version = restored.version();
        self.undo.truncate(pos);
        self.queue.retire_stale(&app_id, version);
        self.app = Some(restored);
        self.feedback.revert(&key, category, now);
        self.preview = None;
        self.history.push(format!("{now} Undone v{}→v{version}: {title}", current.version()));
        self.refresh(now);
        Ok(Acted::Reverted { version })
    }

    // ---- 権限 / policy / feedback -------------------------------------------------

    /// grant は app が宣言した scope にしか付かない（validate_grant）。
    pub fn grant(&mut self, grant: PermissionGrant) -> Result<(), ValidationError> {
        if let Some(app) = &self.app {
            validate_grant(app.definition(), &grant)?;
        }
        self.grants.retain(|g| g.id != grant.id);
        self.grants.push(grant);
        Ok(())
    }
    pub fn grants(&self) -> &[PermissionGrant] {
        &self.grants
    }
    pub fn consent(&mut self, receipt: ConsentReceipt) {
        self.consents.retain(|c| c.id != receipt.id);
        self.consents.push(receipt);
    }
    pub fn consents(&self) -> &[ConsentReceipt] {
        &self.consents
    }
    pub fn policy(&self) -> &Policy {
        &self.policy
    }
    pub fn set_autonomy(&mut self, level: AutonomyLevel) {
        self.policy.autonomy = level;
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.policy.paused = paused;
    }
    pub fn mute(&mut self, category: Category) {
        self.feedback.mute(category);
    }
    pub fn unmute(&mut self, category: Category) {
        self.feedback.unmute(category);
    }
    /// 履歴の削除（AGT-007）: 非表示 / snooze / 好み / 決着済みの提案 / rail の履歴を消す。適用済みの構成と取り消しの記録はそのまま。
    pub fn clear_history(&mut self) {
        self.feedback.clear();
        self.queue.drop_settled();
        self.history.clear();
    }
    pub fn feedback(&self) -> &Feedback {
        &self.feedback
    }

    // ---- rail が読む物 ------------------------------------------------------------

    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }
    pub fn history_lines(&self) -> &[String] {
        &self.history
    }
    /// 取り消せる適用（最後の 1 つだけ: それより前は後の変更に重なる）。(id, title)。
    pub fn undo_rows(&self) -> Vec<(String, String)> {
        self.undo
            .last()
            .filter(|u| self.app.as_ref().is_some_and(|a| a.version() == u.after_version))
            .map(|u| vec![(u.suggestion_id.clone(), u.title.clone())])
            .unwrap_or_default()
    }

    /// §5.2 の文脈 label。「承認が必要」「要接続」は policy::decide が自動を止める条件と同じ物から導く
    /// （画面が「承認が必要」と言う提案を auto が黙って当てない）。`now` は decide に渡す時刻と同じ物（grant の期限を見る）。
    pub fn labels(&self, t: &Tracked, now: &str) -> Vec<String> {
        let mut labels = Vec::new();
        let s = &t.suggestion;
        let g = &t.grounding;
        let risk = g.effective_risk;
        let def = self.app.as_ref().map(|a| a.definition());
        if self.needs_connection(t) {
            labels.push("Needs a connection".to_owned());
        }
        // 権限は申告だけでなく、繋ぐ action が要する scope も（grounding が導いた和）。期限の切れた grant は数えない（decide と同じ述語）
        let ungranted = g.scopes.iter().any(|sc| !policy::scope_granted(&self.grants, &s.app_id, sc, now));
        let approval = grounding::risk_rank(risk) >= grounding::risk_rank(Risk::ExternalWrite)
            || ungranted
            || def.is_some_and(|d| policy::needs_approval(d, g));
        if approval {
            labels.push("Needs approval".to_owned());
        }
        if risk == Risk::ExternalRead {
            labels.push("Read only".to_owned());
        }
        if labels.is_empty() {
            labels.push("Suggested".to_owned());
        }
        labels
    }

    fn needs_connection(&self, t: &Tracked) -> bool {
        self.app.as_ref().is_some_and(|a| policy::needs_connection(a.definition(), &t.suggestion, &t.grounding))
    }

    /// `[適用]` を出せるか: ローカルで取り消せる UI patch で、権限も接続も要らない物だけ。それ以外は `[プレビュー]`。
    pub fn primary_is_apply(&self, t: &Tracked, now: &str) -> bool {
        let s = &t.suggestion;
        s.kind == SuggestionKind::UiPatch
            && !s.change.operations.is_empty()
            && s.undoable
            && grounding::risk_rank(t.grounding.effective_risk) <= grounding::risk_rank(Risk::LocalReversible)
            && !self.needs_connection(t)
            && !self.labels(t, now).iter().any(|l| l == "Needs approval")
    }

    /// rail の行。`now` は grant の期限を見る時刻（ui::agent_rail は描く時の `now_rfc3339()`）。
    pub fn rail_rows(&self, now: &str) -> Vec<RailRow> {
        self.visible().into_iter().map(|t| self.rail_row(t, now)).collect()
    }

    fn rail_row(&self, t: &Tracked, now: &str) -> RailRow {
        let s = &t.suggestion;
        let evidence = s
            .evidence
            .iter()
            .map(|e| {
                let kind = match e.kind {
                    EvidenceKind::UserGoal => "Purpose",
                    EvidenceKind::AppState => "App state",
                    EvidenceKind::Event => "Events",
                    EvidenceKind::SourceRecord => "Data",
                    EvidenceKind::TestFailure => "Failures",
                };
                let mut line = format!("{kind}: {}", e.r#ref);
                if let Some(at) = &e.observed_at {
                    line.push_str(&format!("({at})"));
                }
                if e.uncertain == Some(true) {
                    line.push_str(" · unconfirmed");
                }
                line
            })
            .collect();
        let changes = s
            .change
            .operations
            .iter()
            .map(|op| {
                let v = op.value.as_ref();
                let field = |k: &str| v.and_then(|v| v.get(k)).and_then(|x| x.as_str().map(String::from).or_else(|| Some(x.to_string())));
                match op.op {
                    PatchOp::Insert => format!(
                        "+ {} ({}) below {}",
                        field("key").unwrap_or_default(),
                        field("component").unwrap_or_default(),
                        op.target_key
                    ),
                    PatchOp::Remove => format!("− {}", op.target_key),
                    PatchOp::Update => format!(
                        "~ {}: {}",
                        op.target_key,
                        v.and_then(|v| v.as_object()).map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()
                    ),
                    PatchOp::Move => format!("→ {} to {}", op.target_key, field("parent").unwrap_or_default()),
                    PatchOp::Bind => format!(
                        "⇐ {}: {}",
                        op.target_key,
                        field("action_ref").map(|a| format!("action {a}")).or_else(|| field("binding").map(|b| format!("binding {b}"))).unwrap_or_default()
                    ),
                }
            })
            .collect();
        RailRow {
            id: s.id.clone(),
            title: s.title.clone(),
            reason: s.reason.clone(),
            labels: self.labels(t, now),
            primary_apply: self.primary_is_apply(t, now),
            evidence,
            changes,
            scopes: t.grounding.scopes.clone(),
            undo: apply::rollback_wording(s.undoable, s.app_version),
            confidence: apply::confidence_wording(t),
        }
    }

    /// `このアプリについて` の (項目, 値)。
    pub fn about_rows(&self) -> Vec<(String, String)> {
        let Some(app) = &self.app else { return Vec::new() };
        let def = app.definition();
        let nodes = def.composition.root.nodes().len();
        let visible = self.visible().len();
        let connected = def.sources.iter().filter(|s| candidate::source_granted(def, &s.id)).count();
        vec![
            ("App".into(), def.title.clone().unwrap_or_else(|| def.id.clone())),
            ("Purpose".into(), def.goal.literal_goal.clone()),
            (
                "Contents".into(),
                format!("v{} · {nodes} parts · {} actions · {} workflows", def.version, def.actions.len(), def.workflows.len()),
            ),
            ("Suggestions".into(), format!("{visible} items (showing up to {})", MAX_SHOWN)),
            ("Autonomy".into(), policy::autonomy_wording(self.policy.autonomy).to_owned()),
            ("Data".into(), format!("{connected} of {} sources are usable", def.sources.len())),
        ]
    }
}

// ---- 時刻（RFC3339 UTC "Z"。`domain::binding` / `domain::permission` と同じく文字列の辞書順で比べる）-----------------------------

pub fn now_rfc3339() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    rfc3339_from_secs(secs)
}

pub fn rfc3339_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn secs_from_rfc3339(s: &str) -> Option<i64> {
    let num = |a: usize, l: usize| s.get(a..a + l)?.parse::<i64>().ok();
    if s.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hh, mm, ss) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

pub fn add_days(rfc3339: &str, days: i64) -> Option<String> {
    secs_from_rfc3339(rfc3339).map(|s| rfc3339_from_secs(s + days * 86_400))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- feedback（旧 agent/feedback.rs）----
pub mod feedback {
    //! ユーザーの反応の台帳（AGT-007 / AGT-013 の最小）: 非表示（持続）・snooze（再開日つき）・category の mute・採択 / 却下の数。
    //! serde で丸ごと保存 / 復元できる（置き場は呼び側: Store の ui_state など）。1 回の採択から広い許可を推論しない —
    //! ここに在るのは順位付けの好みと抑制だけで、権限（grant / consent）は別の台帳。
    use std::collections::{BTreeMap, BTreeSet};

    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    use super::candidate::Category;

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Feedback {
        /// 非表示にした dedupe key。履歴の削除までずっと抑制する。
        pub hidden: BTreeSet<String>,
        /// dedupe key → 再開（RFC3339 UTC）。
        pub snoozed: BTreeMap<String, String>,
        /// mute した category（name）。
        pub muted: BTreeSet<String>,
        /// category name → 採択数 − 却下数。順位付けの好みにだけ使う。
        pub preference: BTreeMap<String, i32>,
        pub log: Vec<Entry>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Entry {
        pub key: String,
        pub category: String,
        /// hide / snooze / accept / revert
        pub decision: String,
        pub at: String,
    }

    impl Feedback {
        pub fn is_hidden(&self, key: &str) -> bool {
            self.hidden.contains(key)
        }
        pub fn is_muted(&self, category: Category) -> bool {
            self.muted.contains(category.name())
        }
        /// snooze 中なら再開日（過ぎていれば None。過ぎた物は読み出し時に捨てる）。
        pub fn snoozed_until(&self, key: &str, now: &str) -> Option<&str> {
            self.snoozed.get(key).map(String::as_str).filter(|until| until.as_bytes() > now.as_bytes())
        }
        pub fn preference(&self, category: Category) -> i32 {
            self.preference.get(category.name()).copied().unwrap_or(0)
        }

        pub fn hide(&mut self, key: &str, category: Category, at: &str) {
            self.hidden.insert(key.to_owned());
            self.snoozed.remove(key);
            *self.preference.entry(category.name().to_owned()).or_insert(0) -= 1;
            self.log(key, category, "hide", at);
        }
        pub fn snooze(&mut self, key: &str, category: Category, until: &str, at: &str) {
            self.snoozed.insert(key.to_owned(), until.to_owned());
            self.log(key, category, "snooze", at);
        }
        pub fn accept(&mut self, key: &str, category: Category, at: &str) {
            self.snoozed.remove(key);
            *self.preference.entry(category.name().to_owned()).or_insert(0) += 1;
            self.log(key, category, "accept", at);
        }
        /// 取り消し = 「要らなかった」: 非表示と同じく鍵を抑え、直後の refresh で同じ提案を出し直さない。
        pub fn revert(&mut self, key: &str, category: Category, at: &str) {
            self.hidden.insert(key.to_owned());
            *self.preference.entry(category.name().to_owned()).or_insert(0) -= 1;
            self.log(key, category, "revert", at);
        }
        pub fn mute(&mut self, category: Category) {
            self.muted.insert(category.name().to_owned());
        }
        pub fn unmute(&mut self, category: Category) {
            self.muted.remove(category.name());
        }
        /// 履歴の削除（AGT-007）: 非表示 / snooze / 好み / log を全部消す。mute は設定なので残す。
        pub fn clear(&mut self) {
            self.hidden.clear();
            self.snoozed.clear();
            self.preference.clear();
            self.log.clear();
        }
        /// 過ぎた snooze を落とす。
        pub fn prune(&mut self, now: &str) {
            self.snoozed.retain(|_, until| until.as_bytes() > now.as_bytes());
        }

        pub fn to_json(&self) -> Value {
            serde_json::to_value(self).unwrap_or(Value::Null)
        }
        pub fn from_json(v: &Value) -> Result<Self, serde_json::Error> {
            serde_json::from_value(v.clone())
        }

        fn log(&mut self, key: &str, category: Category, decision: &str, at: &str) {
            self.log.push(Entry { key: key.to_owned(), category: category.name().to_owned(), decision: decision.to_owned(), at: at.to_owned() });
        }
    }
} // mod feedback

// ---- lifecycle（旧 agent/lifecycle.rs）----
pub mod lifecycle {
    //! 提案の状態機械の門（AGT-002）。表は domain の `SUGGESTION_TRANSITIONS` 1 枚で、ここはそれを呼ぶ関数の集まり。
    //! 表に無い遷移は Err で状態は変わらない。queue / apply / Agent はこの関数以外で status を書かない。
    //! DESIGN §5.3 の stale / blocked / withdrawn は保存しない: `hold` が今の revision に validate_suggestion を当てて導く（CD-03 D）。
    use super::domain::app::AppDefinitionV3;
    use super::domain::permission::PermissionGrant;
    use super::domain::run::TransitionError;
    use super::domain::suggestion::{Suggestion, SuggestionStatus};
    use super::domain::validate::{validate_suggestion, ValidationError};
    use super::policy::scope_granted;
    use super::queue::Tracked;

    pub type Transition = Result<(), TransitionError<SuggestionStatus>>;

    /// §5.3 の側枝のうち schema に値が無い 3 つ。描くたびに計算し、status には書かない。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Hold {
        /// 構成が進んだ（app_version が違う）。§11「構成が更新されています」
        Stale,
        /// 宣言されていない scope か、有効な grant の無い scope が要る
        Blocked,
        /// 根拠か対象が消えた
        Withdrawn,
    }

    /// validate_suggestion の error は全部返るので、重なった時の優先は stale > blocked > withdrawn（rebase で他も変わりうる）。
    pub fn hold(def: &AppDefinitionV3, s: &Suggestion, grants: &[PermissionGrant], now: &str) -> Option<Hold> {
        let errs = validate_suggestion(def, s).err().unwrap_or_default();
        let any = |f: fn(&ValidationError) -> bool| errs.iter().any(f);
        if any(|e| matches!(e, ValidationError::VersionMismatch { .. })) {
            Some(Hold::Stale)
        } else if any(|e| matches!(e, ValidationError::UndeclaredScope(_)))
            || s.permission_scopes.iter().any(|sc| !scope_granted(grants, &s.app_id, sc, now))
        {
            Some(Hold::Blocked)
        } else if any(|e| matches!(e, ValidationError::NoEvidence(_) | ValidationError::MissingTarget { .. })) {
            Some(Hold::Withdrawn)
        } else {
            None
        }
    }

    pub fn advance(t: &mut Tracked, to: SuggestionStatus) -> Transition {
        t.suggestion.status.transition(to)
    }

    pub fn validate(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Validated)
    }

    pub fn queue(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Queued)
    }

    /// snooze 明け。
    pub fn requeue(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Queued)?;
        t.resume_at = None;
        Ok(())
    }

    /// rail に実際に出た時だけ（§5.3 presented）。
    pub fn show(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Shown)
    }

    /// queued のまま押された物は shown を経てから（表に queued→previewed は無い）。
    /// 既に previewed なら何もしない（閉じた・差し替えたプレビューを開き直す。状態は進めない）。
    pub fn preview(t: &mut Tracked) -> Transition {
        match t.suggestion.status {
            SuggestionStatus::Previewed => Ok(()),
            SuggestionStatus::Queued => {
                show(t)?;
                advance(t, SuggestionStatus::Previewed)
            }
            _ => advance(t, SuggestionStatus::Previewed),
        }
    }

    pub fn approve(t: &mut Tracked) -> Transition {
        if t.suggestion.status == SuggestionStatus::Queued {
            show(t)?;
        }
        advance(t, SuggestionStatus::Approved)
    }

    pub fn reject(t: &mut Tracked, note: &str) -> Transition {
        advance(t, SuggestionStatus::Rejected)?;
        t.note = Some(note.to_owned());
        Ok(())
    }

    pub fn snooze(t: &mut Tracked, until: &str) -> Transition {
        advance(t, SuggestionStatus::Snoozed)?;
        t.resume_at = Some(until.to_owned());
        Ok(())
    }

    pub fn apply(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Applied)
    }

    pub fn verify(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Verified)
    }

    pub fn fail(t: &mut Tracked, note: &str) -> Transition {
        advance(t, SuggestionStatus::Failed)?;
        t.note = Some(note.to_owned());
        Ok(())
    }

    pub fn revert(t: &mut Tracked) -> Transition {
        advance(t, SuggestionStatus::Reverted)
    }
} // mod lifecycle

// ---- ranker（旧 agent/ranker.rs）----
pub mod ranker {
    //! 順位付けと重複の鍵（AGT-004）。PROACTIVE_SUGGESTION_ENGINE §5 の式をローカルの入力だけで:
    //! score = 0.34 goal + 0.22 evidence + 0.14 time_saved + 0.12 urgency + 0.08 reversibility + 0.10 preference
    //!       − 0.30 permission_friction − 0.18 attention_cost − 0.30 uncertainty（入力は全部 0..1）。
    //! 重複の鍵は app_id | category | target | op の並び（意味的に同じ変更は 1 つだけ出る）。
    use std::collections::BTreeSet;

    use super::candidate::Category;
    use super::domain::app::AppDefinitionV3;
    use super::domain::suggestion::Suggestion;
    use super::grounding::{is_external, Grounding};

    pub fn dedupe_key(s: &Suggestion, category: Category) -> String {
        let mut sig: Vec<String> =
            s.change.operations.iter().map(|o| format!("{:?}:{}", o.op, o.target_key).to_lowercase()).collect();
        sig.sort();
        format!("{}|{}|{}|{}", s.app_id, category.name(), s.target_ref, sig.join(","))
    }

    /// 式の入力（全部 0..1）。test と rail の「なぜこの順か」のために公開。
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Inputs {
        pub goal_relevance: f32,
        pub verified_evidence: f32,
        pub expected_time_saved: f32,
        pub urgency: f32,
        pub reversibility: f32,
        pub user_preference: f32,
        pub permission_friction: f32,
        pub attention_cost: f32,
        pub uncertainty: f32,
    }

    impl Inputs {
        pub fn score(&self) -> f32 {
            0.34 * self.goal_relevance + 0.22 * self.verified_evidence + 0.14 * self.expected_time_saved + 0.12 * self.urgency
                + 0.08 * self.reversibility
                + 0.10 * self.user_preference
                - 0.30 * self.permission_friction
                - 0.18 * self.attention_cost
                - 0.30 * self.uncertainty
        }
    }

    /// `preference` は feedback の category ごとの採択数 − 却下数（−3..3 に切る）。
    pub fn inputs(def: &AppDefinitionV3, s: &Suggestion, g: &Grounding, category: Category, preference: i32) -> Inputs {
        let goal = bigrams(&format!("{} {}", def.goal.literal_goal, def.goal.outcomes.join(" ")));
        let text = bigrams(&format!("{} {}", s.title, s.effect));
        let goal_relevance = if goal.is_empty() { 0.0 } else { goal.intersection(&text).count() as f32 / goal.len() as f32 };
        let (time_saved, urgency) = match category {
            Category::AddSearchOrFilter => (0.7, 0.3),
            Category::CompleteMissingDependency => (0.5, 0.4),
            Category::RepairBinding => (0.6, 0.3),
            Category::FixInput => (0.8, 0.9),
            Category::ConnectCandidateSource => (0.4, 0.9),
        };
        let scopes = g.scopes.len().min(3) as f32 / 3.0;
        let permission_friction = (scopes + if is_external(g.effective_risk) { 0.5 } else { 0.0 }).min(1.0);
        let attention_cost = s.change.operations.len().min(5) as f32 / 5.0 * 0.5;
        let uncertainty = ((1.0 - g.confidence) + if g.uncertain { 0.25 } else { 0.0 }).min(1.0);
        Inputs {
            goal_relevance: goal_relevance.clamp(0.0, 1.0),
            verified_evidence: g.confidence.clamp(0.0, 1.0),
            expected_time_saved: time_saved,
            urgency,
            reversibility: if s.undoable { 1.0 } else { 0.0 },
            user_preference: (preference.clamp(-3, 3) + 3) as f32 / 6.0,
            permission_friction,
            attention_cost,
            uncertainty,
        }
    }

    pub fn score(def: &AppDefinitionV3, s: &Suggestion, g: &Grounding, category: Category, preference: i32) -> f32 {
        inputs(def, s, g, category, preference).score()
    }

    /// 日本語でも効く粗い類似: 空白を除いた文字の 2-gram。
    fn bigrams(s: &str) -> BTreeSet<(char, char)> {
        let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).map(|c| c.to_ascii_lowercase()).collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    }
} // mod ranker

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc3339_roundtrip_and_add_days() {
        for s in ["2026-10-16T22:45:00Z", "2024-02-29T00:00:00Z", "1970-01-01T00:00:00Z", "2000-03-01T12:34:56Z"] {
            assert_eq!(rfc3339_from_secs(secs_from_rfc3339(s).unwrap()), s);
        }
        assert_eq!(add_days("2026-10-16T22:45:00Z", 1).unwrap(), "2026-10-17T22:45:00Z");
        assert_eq!(add_days("2024-02-29T00:00:00Z", 1).unwrap(), "2024-03-01T00:00:00Z");
        assert_eq!(add_days("2026-12-31T23:59:59Z", 1).unwrap(), "2027-01-01T23:59:59Z");
        assert!(secs_from_rfc3339("2026-10-16").is_none());
        assert!(now_rfc3339().ends_with('Z'));
    }
}
