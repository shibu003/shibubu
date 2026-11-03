//! Runs（実行履歴・DESIGN §4.5・§11・§15）。図は docs/diagrams.md §22。外部側の記録ではなくこの端末の記録。
//! - 一覧: 行 = button 1 つ（押すと下に詳細）。何をしたか · app · 何分前 · 結果の 1 語。run の id は key にだけ使い画面に出さない
//! - 詳細: 何をしたか・相手・結果・根拠（読み戻した証拠）・取り消し + 次の一手 0 / 1。外に効いた run は相手と取り消せるかを必ず出す
//! - 入力は domain の RunStatus / Outcome / Attempt / Effect。runtime の RunRecord が main に入ったら統合役が寄せる（図 §22 の対応）
//! - 読み込み中は行の骨格・行は 30ms ずつずらして出す・結果の色は cross-fade・reduce motion で 0
//! - app.rs の View::Runs は `show` を呼ぶ。行は src/generated.rs の `run_entries`（runtime の RunRecord）、押した一手は `run_action`

use super::motion;
use super::shell;
use super::skeleton::{self, Skeleton};
use super::states::{self, first_sentence, has_ja, RegionState};
use super::theme::{self, Type, Variant};
use crate::domain::action::Effect;
use crate::domain::run::{Attempt, ErrorCode, Outcome, RunStatus};
use eframe::egui::{self, accesskit, Id, Vec2};

/// 結果の 1 語（status と最後の attempt の outcome から決める・図 §22 の W）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunResult {
    Waiting,
    Planning,
    NeedsConnection,
    NeedsApproval,
    Running,
    Verifying,
    /// FX7（FX-039）: 止めるよう頼んだが、まだ止まったか分からない（`CancelRequested` を記録した走っている run）。止まれば `Undone`・間に合わなければ結果のまま
    Stopping,
    Verified,
    Unverified,
    Rejected,
    Failed,
    /// 入ったか分からない（送り直さない）
    Unknown,
    Cancelled,
}

impl RunResult {
    /// verified の status でも、最後の outcome が Verified でなければ「確かめた」と言わない（RUN-004）。
    pub fn of(status: RunStatus, outcome: Option<&Outcome>) -> RunResult {
        use RunResult as R;
        use RunStatus as S;
        match (status, outcome) {
            (S::Idle, _) => R::Waiting,
            (S::Planning, _) => R::Planning,
            (S::WaitingConnection, _) => R::NeedsConnection,
            // WF0: Engine の判断待ちは Runs の行に出していない（`run_entries` が Engine の run を除く）。FX7 が Runs に出す時に語を足す
            (S::WaitingApproval | S::WaitingHuman, _) => R::NeedsApproval,
            (S::Running, _) => R::Running,
            (S::Verifying, _) => R::Verifying,
            (S::Cancelled, _) => R::Cancelled,
            (_, Some(Outcome::Unverified { .. })) => R::Unverified,
            (S::Uncertain, _) | (_, Some(Outcome::EffectUnknown { .. })) => R::Unknown,
            (S::Verified, Some(Outcome::Verified { .. })) => R::Verified,
            (S::Verified, None) => R::Unverified,
            (_, Some(Outcome::Rejected { .. })) => R::Rejected,
            _ => R::Failed,
        }
    }

    pub const fn wording(self) -> &'static str {
        match self {
            RunResult::Waiting => "Queued",
            RunResult::Planning => "Preparing",
            RunResult::NeedsConnection => "Not connected",
            RunResult::NeedsApproval => "Needs approval",
            RunResult::Running => "Running",
            RunResult::Verifying => "Checking",
            RunResult::Stopping => "Stopping",
            RunResult::Verified => "Verified",
            RunResult::Unverified => "Not verified",
            RunResult::Rejected => "Declined",
            RunResult::Failed => "Failed",
            RunResult::Unknown => "Sent, not confirmed",
            RunResult::Cancelled => "Undone",
        }
    }

    /// glyph と色（緑は確かめた物だけ・色だけで示さない）。
    pub const fn region(self) -> RegionState {
        match self {
            RunResult::Waiting => RegionState::Ready,
            RunResult::Planning => RegionState::Loading,
            RunResult::NeedsConnection => RegionState::Disconnected,
            RunResult::NeedsApproval => RegionState::Blocked,
            RunResult::Running | RunResult::Verifying | RunResult::Stopping => RegionState::Running,
            RunResult::Verified => RegionState::Verified,
            RunResult::Unverified => RegionState::Partial,
            RunResult::Rejected | RunResult::Failed => RegionState::Error,
            RunResult::Unknown => RegionState::Stale,
            RunResult::Cancelled => RegionState::Empty,
        }
    }

    /// まだ終わっていない（status が進んでいる途中）。
    pub const fn in_progress(self) -> bool {
        matches!(self, RunResult::Waiting | RunResult::Planning | RunResult::NeedsConnection | RunResult::NeedsApproval | RunResult::Running | RunResult::Verifying | RunResult::Stopping)
    }

    /// 根拠が記録に無い時の事実 1 行。
    const fn fallback_evidence(self) -> &'static str {
        match self {
            RunResult::Verified => "Read back and matched",
            RunResult::Unverified => "Couldn't read it back",
            RunResult::Rejected => "The app didn't accept it",
            RunResult::Unknown => "Can't tell if it arrived",
            _ => "",
        }
    }
}

/// 詳細の次の一手（呼ぶ側が runtime の cancel・Connections・記録の読み直しへ繋ぐ）。押しても何もしない一手は持たない
/// （承認の画面・相手の側を読み直す照合・やり直しは、先が出来た時に足す）。
/// FX7（FX-037）: `CheckAgain` = 「送ったか分からない」run の記録を読み直し、まだ分からなければ相手の app で確かめる道を 1 行で言う（送り直さない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunAction {
    Cancel,
    Connect,
    CheckAgain,
    /// FX7（FX-037）: その run が属する Project を開く（`RunEntry::project` が在る時だけ・他に次の一手が無い時の 1 つ）。
    OpenProject,
}

impl RunAction {
    pub const fn label(self) -> &'static str {
        match self {
            RunAction::Cancel => "Stop",
            RunAction::Connect => "Connect",
            RunAction::CheckAgain => "Check again",
            RunAction::OpenProject => "Open project",
        }
    }

    const fn variant(self) -> Variant {
        match self {
            RunAction::Cancel => Variant::Ghost,
            RunAction::Connect => Variant::Primary,
            RunAction::CheckAgain | RunAction::OpenProject => Variant::Ghost,
        }
    }
}

/// 状態ごとの次の一手（0 か 1・図 §22 の NX）。副作用の前（承認待ちも）は止める・接続が要る物は接続する、他は無し。
/// 確かめ中は取り消せない（RUN-008）ので無し。
pub fn next_step(result: RunResult, outcome: Option<&Outcome>) -> Option<RunAction> {
    use RunResult as R;
    match result {
        R::Waiting | R::Planning | R::Running | R::NeedsApproval => Some(RunAction::Cancel),
        R::NeedsConnection => Some(RunAction::Connect),
        R::Failed => match outcome {
            Some(Outcome::Failed { error: ErrorCode::PermissionDenied | ErrorCode::ConnectionLost | ErrorCode::NotReady }) => Some(RunAction::Connect),
            _ => None,
        },
        // FX7: 送ったか分からない run は Runs の先頭に置き、記録の読み直しを出す（送り直す一手は出さない）
        R::Unknown => Some(RunAction::CheckAgain),
        R::Stopping | R::Unverified | R::Verifying | R::Verified | R::Rejected | R::Cancelled => None,
    }
}

/// 失敗の理由の語（ErrorCode 13 個）。
pub const fn error_word(e: ErrorCode) -> &'static str {
    match e {
        ErrorCode::NotReady => "Not set up yet",
        ErrorCode::PermissionDenied => "Needs permission",
        ErrorCode::ApprovalDenied => "Not approved",
        ErrorCode::InvalidInput => "Input not accepted",
        ErrorCode::ConnectionLost => "Connection lost",
        ErrorCode::Timeout => "Took too long",
        ErrorCode::RateLimited => "Limit reached",
        ErrorCode::TargetNotFound => "Couldn't find the app",
        ErrorCode::TargetAmbiguous => "Couldn't tell which app",
        ErrorCode::DriverRefused => "The app refused the action",
        ErrorCode::VerificationFailed => "The check failed",
        ErrorCode::Cancelled => "Undone",
        ErrorCode::Internal => "Something went wrong in Shibubu",
    }
}

/// 取り消せるか（詳細の「取り消し」の行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Undo {
    /// 読んだだけ（変わった物が無い）
    NotNeeded,
    /// この Mac の中で前の版に戻せる（runtime が undo を持つ時だけ渡す。今の runtime は持たないので作られない）
    #[allow(dead_code)]
    InApp,
    /// 相手の app・service の側で戻す
    InSource,
    /// 送った・消した物は戻らない
    Never,
    Unknown,
}

impl Undo {
    pub const fn wording(self) -> &'static str {
        match self {
            Undo::NotNeeded => "Nothing changed",
            Undo::InApp => "Can be undone on this Mac",
            Undo::InSource => "Undo it in the app",
            Undo::Never => "Can't be undone",
            Undo::Unknown => "Not sure it can be undone",
        }
    }

    /// effect だけから言える控えめな値（戻せると約束しない）。
    pub const fn conservative(effect: Effect) -> Undo {
        match effect {
            Effect::LocalRead | Effect::ExternalRead => Undo::NotNeeded,
            Effect::LocalWrite => Undo::Unknown,
            Effect::ExternalWrite => Undo::InSource,
            Effect::ExternalSend | Effect::Destructive => Undo::Never,
        }
    }
}

/// 詳細の「Effect」の行の語（この手が何をするか）。
pub const fn effect_word(effect: Effect) -> &'static str {
    match effect {
        Effect::LocalRead => "Reads on this Mac",
        Effect::LocalWrite => "Changes something on this Mac",
        Effect::ExternalRead => "Reads from another app",
        Effect::ExternalWrite => "Changes another app",
        Effect::ExternalSend => "Sends outside",
        Effect::Destructive => "Deletes something",
    }
}

/// 外に効く（送る・書く・消す）。相手と取り消しを必ず出す。
pub const fn external(effect: Effect) -> bool {
    matches!(effect, Effect::ExternalWrite | Effect::ExternalSend | Effect::Destructive)
}

/// 一覧の 1 件。
#[derive(Clone, Debug, PartialEq)]
pub struct RunEntry {
    /// run の id（行の key と押した一手の宛先にだけ使う・画面に出さない）
    pub id: String,
    /// 何をしたか（action の名前）
    pub what: String,
    pub app: String,
    /// 最後に状態が変わった時刻（RFC3339・`run.updated_at`）
    pub at: String,
    pub status: RunStatus,
    pub effect: Effect,
    /// 相手（宛先・対象。空なら「未記録」）
    pub target: String,
    pub undo: Undo,
    /// 最後の step の試行（`run.steps[0].attempts`）
    pub attempts: Vec<Attempt>,
    /// FX2 反証の直し: 次の一手に `Connect`（Connections へ）を出せるか = Connections にこの run の直す先の行が在る。runtime の run は true・
    /// Home の生成の行は false（Connections に AI の行が無い = 押しても直す先が無い・G99 で AI の行が出来たら true に）。失敗の語（`error_word`）は変えない。
    pub connect: bool,
    /// FX7（FX-037）: この run が属する Project の id（`Open project` の行き先）。runtime の run は app を包んだ Project・Home の生成の行は None（Project がまだ無い）。
    pub project: Option<String>,
    /// FX7（FX-039）: 止めるよう頼んだ（`CancelRequested`）走っている run。`result()` が `Stopping` になり、`Stop` はもう押せない
    pub stopping: bool,
    /// FX7（FX-040）: 起点（誰が始めた手か）。空 = 記録に無い（詳細に出さない）
    pub started_by: String,
}

impl RunEntry {
    /// 最後の attempt の結果（試行中なら None）。
    pub fn outcome(&self) -> Option<&Outcome> {
        self.attempts.last().and_then(|a| a.outcome.as_ref())
    }

    pub fn result(&self) -> RunResult {
        match RunResult::of(self.status, self.outcome()) {
            RunResult::Running if self.stopping => RunResult::Stopping,
            r => r,
        }
    }

    pub fn next_step(&self) -> Option<RunAction> {
        next_step(self.result(), self.outcome())
            .filter(|a| self.connect || *a != RunAction::Connect)
            // 他に次の一手が無い行は、属する Project へ戻る一手（次の一手は 0 か 1 のまま）
            .or_else(|| self.project.as_ref().map(|_| RunAction::OpenProject))
    }

    /// RQ1（RQ-004・UIUX N11「Runsの詳細に`Copy details`（秘密除去）・error code, user action, evidence」）: 開いた 1 件の詳細の `Copy details` が clipboard に入れる文。
    /// 中身は画面の詳細と同じ行（`What`・`With`・`Result`・`What was checked`・`Undo`）を `"{項目}: {値}"` で改行に繋ぎ、最後に `Attempts: {n}` と失敗の語（`Failed` の時だけ・
    /// `ErrorCode` の表の語）。run の id・URL・内部の語は `evidence` と同じく落とし、`With` の値の file path（`/…`・`~/…`）と email は `[path]`・`[email]` に伏せる。
    /// 呼ぶ側（`detail` の本体）: 行の下に Ghost の `Copy details` を常に 1 つ（次の一手の button とは別・押すと `ui.ctx().copy_text(e.details_text())` と `note(ctx, &e.id, "Details copied")`・
    /// Intent を足さない = Connections の `Open settings` と同じ形）。保証する: 何も書かない・送らない。保証しない: 画面に出していない記録（step の入出力の値）を入れる事。
    /// Wave: RQ1
    pub fn details_text(&self) -> String {
        let mut lines: Vec<String> = self.rows(self.result().wording().to_owned()).into_iter().map(|(k, v)| format!("{k}: {}", mask_personal(&v))).collect();
        lines.push(format!("Attempts: {}", self.attempts.len()));
        if let (RunResult::Failed, Some(Outcome::Failed { error })) = (self.result(), self.outcome()) {
            lines.push(format!("Error: {}", error_word(*error)));
        }
        lines.join("\n")
    }

    /// 詳細の行（画面の `detail` と `details_text` が同じ並びを使う）。`result` = Result の欄の文（画面は印つき・copy は語だけ）。
    fn rows(&self, result: String) -> Vec<(&'static str, String)> {
        let mut items: Vec<(&str, String)> = vec![("What", self.what.clone())];
        if self.shows_target() {
            items.push(("With", if self.target.trim().is_empty() { "Not recorded".into() } else { self.target.clone() }));
        }
        if !self.started_by.is_empty() {
            items.push(("Started by", self.started_by.clone()));
        }
        items.push(("Effect", effect_word(self.effect).into()));
        items.push(("Result", result));
        items.extend(self.evidence().into_iter().map(|line| ("What was checked", line)));
        if self.shows_undo() {
            items.push(("Undo", self.undo.wording().into()));
        }
        items
    }

    /// 根拠の行（読み戻した証拠・理由・失敗の語）。id・URL・内部の語は落とす。まだ終わっていない run には出さない。
    pub fn evidence(&self) -> Vec<String> {
        let result = self.result();
        if result.in_progress() {
            return Vec::new();
        }
        // 根拠は result と同じ種類の outcome からだけ取る（(Failed, Verified) で「一致」と言わない）。ずれた組は結果の語の表へ
        use RunResult as R;
        let lines: Vec<String> = match (result, self.outcome()) {
            (R::Verified, Some(Outcome::Verified { evidence })) => evidence.iter().filter_map(|e| evidence_line(e)).map(String::from).collect(),
            (R::Unverified, Some(Outcome::Unverified { reason }))
            | (R::Rejected, Some(Outcome::Rejected { reason }))
            | (R::Unknown, Some(Outcome::EffectUnknown { reason })) => {
                // 内部の語（handler・service・revision など）や Debug の混じる reason は出さず、結果の語の表の 1 行に替える
                plain(sentence(reason)).map(String::from).into_iter().collect()
            }
            (R::Failed, Some(Outcome::Failed { error })) => vec![error_word(*error).to_string()],
            _ => Vec::new(),
        };
        match (lines.is_empty(), result.fallback_evidence()) {
            (true, f) if !f.is_empty() && self.outcome().is_some() => vec![f.to_string()],
            _ => lines,
        }
    }

    fn shows_target(&self) -> bool {
        external(self.effect) || !self.target.trim().is_empty()
    }

    fn shows_undo(&self) -> bool {
        external(self.effect) || self.undo != Undo::NotNeeded
    }
}

/// RQ1: 人に渡す文（Runs の `Copy details`・Settings › About の `Copy diagnostics`・AI の行）から、家の dir を `~` にした上で
/// file path（`/…`・`~/…`・`file://…` で始まる語）を `[path]`、email（`x@y.z`）を `[email]` に伏せる。語の前後の括弧と句読点は残す。
/// 空白を含む path（`Application Support`・`My Song.wav`）: 後ろの語を ` · `・改行・句読点で終わる語・括弧の手前まで見て、`/` か拡張子を持つ最後の語までを 1 つの path にする
/// （`… take.wav for a@b.co` の `for` から先は残る）。読み違えたら伏せる側に倒れる。
pub fn mask_personal(s: &str) -> String {
    const OPEN: [char; 6] = ['(', '"', '\'', '[', '<', '「'];
    const CLOSE: [char; 10] = ['.', ',', ';', ':', ')', '"', '\'', ']', '>', '」'];
    let s = match std::env::var("HOME") {
        Ok(h) if h.len() > 1 => s.replace(h.as_str(), "~"),
        _ => s.to_owned(),
    };
    let pieces: Vec<&str> = s.split_inclusive(char::is_whitespace).collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < pieces.len() {
        let piece = pieces[i];
        let body = piece.trim_end();
        let lead = body.len() - body.trim_start_matches(OPEN).len();
        let core = body[lead..].trim_end_matches(CLOSE);
        let (mask, end) = if (core.starts_with('/') && core.len() > 1) || core.starts_with("~/") || core.starts_with("file://") {
            let mut end = (i, lead + core.len()); // path の終わり（piece の番号・その piece の中の byte）
            let mut k = i;
            loop {
                let b = pieces[k].trim_end();
                let c = b.trim_end_matches(CLOSE);
                let ext = c.rsplit_once('.').is_some_and(|(a, e)| !a.is_empty() && !e.is_empty() && e.chars().all(|ch| ch.is_ascii_alphanumeric()));
                if k > i && !c.contains('@') && (c.contains('/') || ext) {
                    end = (k, c.len());
                }
                let next_stops = pieces.get(k + 1).map_or(true, |n| {
                    let nb = n.trim_end();
                    nb.is_empty() || nb.starts_with('·') || nb.starts_with(OPEN)
                });
                if c.len() < b.len() || !pieces[k].ends_with(' ') || next_stops {
                    break;
                }
                k += 1;
            }
            ("[path]", end)
        } else if core == "~" {
            ("[path]", (i, lead + core.len()))
        } else if core.split_once('@').is_some_and(|(user, host)| !user.is_empty() && host.contains('.') && !host.starts_with('.')) {
            ("[email]", (i, lead + core.len()))
        } else {
            out.push_str(piece);
            i += 1;
            continue;
        };
        out.push_str(&body[..lead]);
        out.push_str(mask);
        out.push_str(&pieces[end.0][end.1..]);
        i = end.0 + 1;
    }
    out
}

/// 「drafts.get <id>: To and subject matched」の前置き（呼んだ口と id）を落とし、1 文目だけ。id・URL だけの証拠は出さない。
/// 「drafts.get <id>: 宛先・件名が一致」の前置き（日本語の無い部分 = 呼んだ口と id）を落とし、1 文目だけ。日本語の無い証拠（id・URL）は出さない。
fn evidence_line(e: &str) -> Option<&str> {
    let e = e.trim();
    let e = match e.split_once(": ") {
        Some((head, rest)) if !has_ja(head) && plain(head).is_none() => rest,
        _ => e,
    };
    plain(sentence(e))
}

/// 1 文目（英語の「. 」でも切り、末尾の「.」は落とす）
fn sentence(s: &str) -> &str {
    first_sentence(s.split_once(". ").map_or(s, |(a, _)| a)).trim_end_matches('.')
}

/// 人に見せてよい 1 文か（G102 FX-102）。日本語は英字の混じらない物だけ。英語は語が 2 つ以上で、内部の語・id・URL・Debug（`(Timeout)`・`{..}`・`::`）を持たない物だけ。
fn plain(s: &str) -> Option<&str> {
    const INTERNAL: [&str; 11] = ["revision", "lease", "receipt", "handler", "service", "digest", "payload", "seq", "token", "stderr", "json"];
    let en = !has_ja(s);
    let words: Vec<&str> = s.split(|c: char| !c.is_ascii_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let ok = if en {
        s.contains(' ')
            && !s.contains(['_', '{', '}', '=', '`']) && !s.contains("::") && !s.contains("://")
            && !words.iter().any(|w| INTERNAL.contains(&w.to_ascii_lowercase().as_str()))
            // id の形（`drafts.get`・`r-1234567890`・`example.com`）: 語の中の「.」・英字と数字の混じり・5 桁以上の数字
            && !s.split_whitespace().map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric())).any(|t| {
                t.contains('.') || t.contains(|c: char| c.is_ascii_digit()) && t.contains(|c: char| c.is_ascii_alphabetic()) || t.chars().filter(char::is_ascii_digit).count() >= 5
            })
            && !s.split('(').skip(1).any(|p| p.split(')').next().is_some_and(|t| !t.contains(' ') && t.starts_with(|c: char| c.is_ascii_uppercase())))
    } else {
        !s.chars().any(|c| c.is_ascii_alphabetic())
    };
    ok.then_some(s)
}

/// RFC3339（`2026-10-18T05:03:00Z`・`+09:00` も）→ unix 秒。読めなければ None。
pub(crate) fn unix_secs(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let (y, m, d, hh, mm, ss) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    // 秒の小数を飛ばして時差へ
    let rest = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "z" | "" => 0,
        r if r.len() == 6 && (r.starts_with('+') || r.starts_with('-')) => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            sign * (r.get(1..3)?.parse::<i64>().ok()? * 3600 + r.get(4..6)?.parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    // days_from_civil（Howard Hinnant）
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

/// `ago` の言い方が次に替わるまでの秒（W2b-AX 直し: idle の窓は frame を頼まないので `show` がこの時の frame を頼む）。
/// 日付だけになった（1 日より前）・読めない時刻は替わらない = None。
pub fn ago_changes_in(at: &str, now_unix: i64) -> Option<i64> {
    let d = (now_unix - unix_secs(at)?).max(0);
    let step = match d {
        0..=3599 => 60,
        3600..=86399 => 3600,
        _ => return None,
    };
    Some(step - d % step)
}

/// 何分前（時差に依らない相対の言い方。1 日より前は日付だけ = copy.en.md の「{Oct 23}」）。読めない時刻は空。
pub fn ago(at: &str, now_unix: i64) -> String {
    let Some(t) = unix_secs(at) else { return String::new() };
    let d = (now_unix - t).max(0);
    match d {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => {
            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let month = at.get(5..7).and_then(|m| m.parse::<usize>().ok()).and_then(|m| MONTHS.get(m.wrapping_sub(1)));
            let day = at.get(8..10).and_then(|d| d.parse::<u32>().ok());
            month.zip(day).map_or(String::new(), |(m, d)| format!("{m} {d}"))
        }
    }
}

/// FX7（FX-040）: Runs の絞り込み（`Filter` の menu で選ぶ）。状態の語は menu を開いた時だけ画面と AX に出る（rehearse の `seen()` は role を渡さず部分一致で探すので、常に出す chip は置かない）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunFilter {
    #[default]
    All,
    NotConfirmed,
    Failed,
    InProgress,
    Verified,
}

impl RunFilter {
    const ALL: [RunFilter; 5] = [RunFilter::All, RunFilter::NotConfirmed, RunFilter::Failed, RunFilter::InProgress, RunFilter::Verified];

    const fn menu_label(self) -> &'static str {
        match self {
            RunFilter::All => "Show all runs",
            RunFilter::NotConfirmed => "Show not confirmed",
            RunFilter::Failed => "Show failed",
            RunFilter::InProgress => "Show in progress",
            RunFilter::Verified => "Show verified",
        }
    }

    /// 検索の語（何をしたか・app・相手に含まれる・大文字小文字を見ない）と、絞り込みの両方に合う行か。
    pub fn matches(self, e: &RunEntry, text: &str) -> bool {
        let r = e.result();
        let by_state = match self {
            RunFilter::All => true,
            RunFilter::NotConfirmed => r == RunResult::Unknown,
            RunFilter::Failed => matches!(r, RunResult::Failed | RunResult::Rejected),
            RunFilter::InProgress => r.in_progress(),
            RunFilter::Verified => r == RunResult::Verified,
        };
        let q = text.trim().to_lowercase();
        by_state && (q.is_empty() || [&e.what, &e.app, &e.target].iter().any(|s| s.to_lowercase().contains(&q)))
    }
}

fn filter_key() -> Id {
    Id::new("kangae.runs.filter")
}

/// 検索の語と絞り込みを置く（試験と、別の画面から Runs を絞って開く時）。
pub fn set_filter(ctx: &egui::Context, text: &str, kind: RunFilter) {
    ctx.data_mut(|d| d.insert_temp(filter_key(), (text.to_owned(), kind)));
}

/// 画面に渡す 1 frame ぶん。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunsView<'a> {
    pub entries: &'a [RunEntry],
    pub loading: bool,
    /// 今の unix 秒（「何分前」の基準・test は固定する）
    pub now_unix: i64,
}

impl RunsView<'_> {
    pub fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
    }
}

/// 押された次の一手と、その run の id。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunsAction {
    pub run_id: String,
    pub action: RunAction,
}

#[derive(Clone)]
struct Mem {
    loading: bool,
    started_at: f64,
    open: Option<String>,
}

fn mem_key(id: Id) -> Id {
    id.with("kangae.runs.mem")
}

/// 読み込みの始まりと開いている 1 件を覚える。初めて見た「読み込み済み」は骨格を一度も出さない。
fn remember(ctx: &egui::Context, id: Id, loading: bool) -> Mem {
    let now = ctx.input(|i| i.time);
    let prev: Option<Mem> = ctx.data(|d| d.get_temp(mem_key(id)));
    let mem = match prev {
        Some(m) if loading && !m.loading => Mem { loading, started_at: now, ..m },
        Some(m) => Mem { loading, ..m },
        None => Mem { loading, started_at: if loading { now } else { f64::NEG_INFINITY }, open: None },
    };
    ctx.data_mut(|d| d.insert_temp(mem_key(id), mem.clone()));
    mem
}

fn set_open(ctx: &egui::Context, id: Id, open: Option<String>) {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(mem_key(id), || Mem { loading: false, started_at: f64::NEG_INFINITY, open: None }).open = open);
}

/// G94: 押した次の一手（`Stop`）の結果の 1 行を、押した行の詳細の下に出す（その行を閉じるまで）。呼ぶ側が `show` の戻り値を受けて `run_action` の答えを渡す。
/// 例: `if let Some(a) = pressed { ui::runs::note(ctx, &a.run_id, &line) }`。保証する: 他の行には出ない・窓の下の帯（message）に出さない。
pub fn note(ctx: &egui::Context, run_id: &str, line: &str) {
    ctx.data_mut(|d| d.insert_temp(Id::new(("kangae.runs.note", run_id)), line.to_owned()));
}

/// FX2（FX-007）: 次に `show` が描く時に、その run の行を開いた形（詳細を出した 1 件）にして見える所まで送る（`note` と同じく run の id の key で ctx に置く）。
/// 呼ぶ側: app.rs の `Intent::Build(BuildAction::OpenRun)` = `if let Some(id) = g.model_run_id() { ui::runs::open_run(ctx, id); self.go(View::Runs) }`。
/// 保証する: 1 回だけ効く（開いた後は利用者の開け閉めのまま）・他の行を閉じる（開く 1 件は今と同じ 1 つ）・一覧に無い id なら何もしない（Runs は今の一覧のまま）。
/// 保証しない: 再起動の後の生成の run（`Generated::run_entries` の生成の行はこの起動の間だけ）。
/// Wave: FX2
pub fn open_run(ctx: &egui::Context, run_id: &str) {
    ctx.data_mut(|d| d.insert_temp(open_next_key(), run_id.to_owned()));
}

fn open_next_key() -> Id {
    Id::new("kangae.runs.open_next")
}

/// 実行履歴の一覧と、押した 1 件の詳細。戻り値はこの frame で押された次の一手。
pub fn show(ui: &mut egui::Ui, view: &RunsView) -> Option<RunsAction> {
    // UI-SWEEP-1101 追記（owner 209）: button の枠は Settings・Connections・Runs の中だけ
    let _framed = theme::framed(ui.ctx());
    shell::content_opening(ui, "Runs", "", None);
    ui.spacing_mut().item_spacing.y = 0.0;
    theme::rule(ui);
    let ctx = ui.ctx().clone();
    let id = ui.id().with("kangae.runs");
    let mut mem = remember(&ctx, id, view.loading);
    // FX2（open_run）: 頼まれた run を 1 回だけ開く（押したのと同じ = 他の行は閉じる）・一覧に無い id は捨てて何もしない
    let reveal = ctx.data_mut(|d| d.remove_temp::<String>(open_next_key())).filter(|r| view.entries.iter().any(|e| &e.id == r));
    if let Some(r) = &reveal {
        set_open(&ctx, id, Some(r.clone()));
        ctx.data_mut(|d| d.remove::<String>(Id::new(("kangae.runs.note", r))));
        mem.open = Some(r.clone());
    }
    if let Some(s) = view.entries.iter().filter_map(|e| ago_changes_in(&e.at, view.now_unix)).min() {
        ctx.request_repaint_after(std::time::Duration::from_secs(s as u64));
    }
    let mut pressed = None;
    // FX-040: 検索の欄 1 つと `Filter` の menu 1 つだけ（行が在る時だけ）。絞っている間は `Showing {n} of {m} · Clear` の 1 行（何も出ない時の理由と戻り道）
    let (mut text, mut kind): (String, RunFilter) = ctx.data(|d| d.get_temp(filter_key())).unwrap_or_default();
    if !view.entries.is_empty() {
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Search history").desired_width(220.0));
            theme::ax_name(edit, "Search history");
            ui.scope(|ui| {
                theme::native_buttons(ui);
                ui.menu_button("Filter", |ui| {
                    for k in RunFilter::ALL {
                        if ui.selectable_label(kind == k, k.menu_label()).clicked() {
                            kind = k;
                            ui.close_menu();
                        }
                    }
                });
            });
        });
    }
    let shown: Vec<&RunEntry> = view.entries.iter().filter(|e| kind.matches(e, &text)).collect();
    let filtering = !text.trim().is_empty() || kind != RunFilter::All;
    if filtering && !view.entries.is_empty() {
        ui.horizontal(|ui| {
            theme::label(ui, Type::Helper, format!("Showing {} of {}", shown.len(), view.entries.len()), theme::tokens(&ctx).text_secondary);
            if theme::button_compact(ui, "Clear", Variant::Ghost).clicked() {
                (text, kind) = (String::new(), RunFilter::All);
            }
        });
    }
    ctx.data_mut(|d| d.insert_temp(filter_key(), (text.clone(), kind)));
    skeleton::with_skeleton(ui, "runs", !view.loading, mem.started_at, Skeleton::List(6), |ui| {
        if view.entries.is_empty() {
            empty_row(ui);
            return;
        }
        // 行は上から 30ms ずつずらして fade + slide（§15.2-1 list の stagger・起点は View の替わり目）
        let t0 = shell::view_since(&ctx);
        for (i, e) in shown.iter().copied().enumerate() {
            let open = mem.open.as_deref() == Some(e.id.as_str());
            motion::fade_slide_in(ui, ("kangae.run_row", &e.id), t0 + motion::stagger(&ctx, i) as f64, |ui| {
                let note_id = Id::new(("kangae.runs.note", &e.id));
                let row = entry_row(ui, id, e, open, view.now_unix);
                if reveal.as_deref() == Some(e.id.as_str()) {
                    row.scroll_to_me(None);
                }
                if row.clicked() {
                    set_open(&ctx, id, (!open).then(|| e.id.clone()));
                    ctx.data_mut(|d| d.remove::<String>(note_id));
                }
                if open {
                    if let Some(action) = detail(ui, id, e) {
                        pressed = Some(RunsAction { run_id: e.id.clone(), action });
                    }
                    if let Some(line) = ctx.data(|d| d.get_temp::<String>(note_id)) {
                        ui.add_space(theme::SP_2);
                        theme::label(ui, Type::Helper, &line, theme::tokens(&ctx).text_secondary);
                    }
                }
            });
        }
    });
    pressed
}

/// 空の 1 行は表の `No runs yet`（35.B の B3・FINAL:391・rail の History と同じ語）を `text_secondary` で（ICONS-1101 追記 238: Empty は印が無い）。
fn empty_row(ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::LIST_ROW_H), egui::Sense::hover());
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui.add_space(theme::SP_3);
        theme::label(ui, Type::Helper, "No runs yet", t.text_secondary);
    });
}

/// 1 件の行（button 1 つ = 開く / 閉じる）。何をしたか · app · 何分前 と、右端に結果の 1 語。
fn entry_row(ui: &mut egui::Ui, id: Id, e: &RunEntry, open: bool, now: i64) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let result = e.result();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::LIST_ROW_H), egui::Sense::click());
    let name = format!("{} · {}", e.what, result.wording());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    if ui.is_rect_visible(rect) {
        theme::row_background(ui, rect, response.hovered(), open);
        theme::focus_ring(ui, &response, rect, egui::CornerRadius::same(theme::RADIUS_SM));
        let color = motion::cross_fade(ui.ctx(), id.with(("kangae.run_color", &e.id)), result.region().color(&t));
        let state_w = 160.0;
        let x0 = rect.left() + theme::SP_3;
        let mid = |g: &egui::Galley| rect.center().y - g.size().y / 2.0;
        // ICONS-1101 追記 238: 結果の印は語の左（Running は回る・印の無い状態は語だけ）
        let size = Type::Helper.size();
        let state = theme::truncated(ui, result.wording(), Type::Helper.font(), color, state_w - states::mark_room(result.region(), size));
        let state_x = rect.right() - theme::SP_3 - state.size().x;
        ui.painter().galley(egui::pos2(state_x, mid(&state)), state.clone(), color);
        states::paint_mark(ui.painter(), result.region(), egui::pos2(state_x - theme::SP_1 - size / 2.0, rect.center().y), size, color);
        let room = (rect.right() - theme::SP_3 - state_w - theme::SP_3 - x0).max(40.0);
        let what = theme::truncated(ui, &e.what, Type::Body.font(), t.text_primary, room * 0.6);
        let what_w = what.size().x;
        ui.painter().galley(egui::pos2(x0, mid(&what)), what, t.text_primary);
        let meta: Vec<String> = [e.app.clone(), ago(&e.at, now)].into_iter().filter(|s| !s.is_empty()).collect();
        let meta = theme::truncated(ui, &meta.join(" · "), Type::Helper.font(), t.text_tertiary, (room - what_w - theme::SP_3).max(0.0));
        ui.painter().galley(egui::pos2(x0 + what_w + theme::SP_3, mid(&meta)), meta, t.text_tertiary);
        let line = egui::Rect::from_min_size(rect.left_bottom() - Vec2::new(0.0, theme::HAIRLINE), Vec2::new(rect.width(), theme::HAIRLINE));
        ui.painter().rect_filled(line, 0.0, t.border);
    }
    response
}

/// 開いた 1 件: (項目, 値) の行と次の一手 0 / 1。card にしない（行の下に字下げで並べるだけ）。
fn detail(ui: &mut egui::Ui, id: Id, e: &RunEntry) -> Option<RunAction> {
    let t = theme::tokens(ui.ctx());
    let result = e.result();
    let items = e.rows(result.wording().to_string());
    let top = ui.cursor().min;
    let mut clicked = None;
    ui.indent(id.with(("kangae.run_detail", &e.id)), |ui| {
        ui.add_space(theme::SP_3);
        for (k, v) in &items {
            ui.horizontal(|ui| {
                theme::column_label(ui, k, Type::Helper, t.text_tertiary, 96.0);
                ui.add(egui::Label::new(Type::Body.text(v.as_str()).color(t.text_primary)).selectable(true).wrap());
            });
            ui.add_space(theme::SP_1);
        }
        if let Some(step) = e.next_step() {
            ui.add_space(theme::SP_2);
            if theme::button(ui, step.label(), step.variant()).clicked() {
                clicked = Some(step);
            }
        }
        // RQ1: 人に渡す詳細（path と email は伏せる・何も送らない）。次の一手とは別の Ghost
        ui.add_space(theme::SP_2);
        if theme::button(ui, "Copy details", Variant::Ghost).clicked() {
            ui.ctx().copy_text(e.details_text());
            note(ui.ctx(), &e.id, "Details copied");
        }
        ui.add_space(theme::SP_4);
    });
    let rect = egui::Rect::from_min_max(top, ui.cursor().min.max(top) + Vec2::new(ui.available_width(), 0.0));
    ui.ctx().accesskit_node_builder(id.with(("kangae.run_detail_ax", &e.id)), |n| {
        n.set_role(accesskit::Role::Group);
        n.set_label(format!("Details of {}", e.what));
        n.set_bounds(accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
    theme::rule(ui);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RQ1: 伏せるのは path と email だけ（家の dir は `~` を経て `[path]`）。語の前後の記号と、path に似た分数・単独の `/` は残す。
    #[test]
    fn rq1_mask_personal_hides_paths_and_emails_only() {
        let home = std::env::var("HOME").unwrap_or_default();
        let s = format!("Saved to {home}/Music/a.wav, sent (mail me@example.com). claude-cli/2.1 · 1/2 · a / b · ~/notes");
        assert_eq!(mask_personal(&s), "Saved to [path], sent (mail [email]). claude-cli/2.1 · 1/2 · a / b · [path]");
        assert_eq!(mask_personal("Open /Applications/REAPER.app\nthen"), "Open [path]\nthen");
        assert_eq!(mask_personal("no @ here · x@y"), "no @ here · x@y");
        // 反証の直し: 空白を含む path（.app の data dir・曲の名前）と file:// も最後まで伏せる。path の後の語（for・or ask）と ` · ` の先は残す
        assert_eq!(mask_personal(&format!("Not found at {home}/Library/Application Support/Shibubu/bridge.sock · mail x")), "Not found at [path] · mail x");
        assert_eq!(mask_personal("Saved to /Users/you/Music/My Secret Song.wav"), "Saved to [path]");
        assert_eq!(mask_personal("Saved to /Users/you/Music/My Song.wav. Then open it"), "Saved to [path]. Then open it");
        assert_eq!(mask_personal(&format!("Open file://{home}/Documents/plan.md")), "Open [path]");
        assert_eq!(mask_personal("Open file:///Users/you/Documents/plan.md"), "Open [path]");
        assert_eq!(mask_personal("Not found: /Users/you/My Tools/srv (the program wasn't found)"), "Not found: [path] (the program wasn't found)");
        assert_eq!(mask_personal("see /Users/you/.local/bin or ask me@example.com"), "see [path] or ask [email]");
        assert_eq!(mask_personal("/Users/you/take.wav for a@b.co"), "[path] for [email]");
    }

    /// RQ1: `details_text` は画面の詳細と同じ行 + 試行の数（失敗なら語）。相手の path と email は伏せる・run の id は出さない。
    #[test]
    fn rq1_details_text_is_the_detail_rows_masked() {
        let e = RunEntry {
            id: "run_7f3a9c".into(),
            what: "Save the take".into(),
            app: "REAPER".into(),
            at: "2026-10-18T11:57:00Z".into(),
            status: RunStatus::Failed,
            effect: Effect::ExternalWrite,
            target: "/Users/you/Music/take.wav for a@b.co".into(),
            undo: Undo::InSource,
            attempts: vec![Attempt { seq: 1, started_at: "2026-10-18T11:57:00Z".into(), finished_at: None, outcome: Some(Outcome::Failed { error: ErrorCode::Timeout }) }],
            connect: false,
            project: None,
            stopping: false,
            started_by: String::new(),
        };
        let text = e.details_text();
        assert_eq!(
            text,
            format!(
                "What: Save the take\nWith: [path] for [email]\nEffect: {}\nResult: {}\nWhat was checked: {w}\nUndo: Undo it in the app\nAttempts: 1\nError: {w}",
                effect_word(Effect::ExternalWrite),
                RunResult::Failed.wording(),
                w = error_word(ErrorCode::Timeout)
            )
        );
        assert!(!text.contains("run_7f3a9c"));
    }

    #[test]
    fn verified_status_alone_is_not_verified() {
        let ok = Outcome::Verified { evidence: vec![] };
        assert_eq!(RunResult::of(RunStatus::Verified, Some(&ok)), RunResult::Verified);
        assert_eq!(RunResult::of(RunStatus::Verified, None), RunResult::Unverified);
        assert_eq!(RunResult::of(RunStatus::Uncertain, Some(&ok)), RunResult::Unknown);
        assert_eq!(RunResult::of(RunStatus::Failed, Some(&ok)), RunResult::Failed);
    }

    #[test]
    fn ago_reads_rfc3339_with_offsets() {
        let now = unix_secs("2026-10-18T12:00:00Z").unwrap();
        assert_eq!(ago("2026-10-18T11:59:30Z", now), "Just now");
        assert_eq!(ago("2026-10-18T11:57:00.123Z", now), "3 min ago");
        assert_eq!(ago("2026-10-18T20:00:00+09:00", now), "1 h ago");
        assert_eq!(ago("2026-10-16T12:00:00Z", now), "Oct 16");
        assert_eq!(ago("2026-08-01T00:00:00Z", now), "Aug 1");
        assert_eq!(ago("きのう", now), "");
        assert_eq!(unix_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_secs("2000-03-01T00:00:00Z"), Some(951868800));
    }

    /// W2b-AX 直し: `ago_changes_in` は `ago` の言い方が替わる最初の秒（その 1 秒前までは同じ言い方）。日付だけになった後は None。
    #[test]
    fn w2b_ax_ago_changes_in_is_the_first_second_the_wording_changes() {
        let at = "2026-10-18T12:00:00Z";
        let t = unix_secs(at).unwrap();
        // 0 秒・境目の前後・各段の中・1 日の手前（毎秒見ると多いので 61 秒刻み〈60 と 3600 と素なので段の中の端数を一巡する〉+ 境目）
        let nows = (0..86_400).step_by(61).chain([59, 60, 3599, 3600, 86399]).map(|d| t + d);
        for now in nows {
            let s = ago_changes_in(at, now).unwrap_or_else(|| panic!("1 日の手前で None: {}", now - t));
            assert!(s > 0, "{}", now - t);
            assert_eq!(ago(at, now + s - 1), ago(at, now), "{} 秒後より前に替わった（{} 秒経った時）", s, now - t);
            assert_ne!(ago(at, now + s), ago(at, now), "{} 秒後に替わらない（{} 秒経った時）", s, now - t);
        }
        assert_eq!(ago_changes_in(at, t + 86_400), None, "日付だけになった後も frame を頼む");
        assert_eq!(ago_changes_in("きのう", t), None);
    }

    #[test]
    fn evidence_drops_ids_and_urls() {
        assert_eq!(evidence_line("drafts.get r-1234567890: 宛先・件名・本文が一致（送信していない）"), Some("宛先・件名・本文が一致（送信していない）"));
        assert_eq!(evidence_line("rec_42"), None);
        assert_eq!(evidence_line("https://calendar.google.com/event?eid=abc"), None);
        assert_eq!(evidence_line("events.get ev_9: status=cancelled"), None);
    }

    #[test]
    fn conservative_undo_never_promises() {
        assert_eq!(Undo::conservative(Effect::ExternalSend), Undo::Never);
        assert_eq!(Undo::conservative(Effect::Destructive), Undo::Never);
        assert_eq!(Undo::conservative(Effect::ExternalWrite), Undo::InSource);
        assert_eq!(Undo::conservative(Effect::LocalWrite), Undo::Unknown);
        assert_eq!(Undo::conservative(Effect::LocalRead), Undo::NotNeeded);
    }
}
