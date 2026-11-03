//! 静かな queue（AGT-006 / AGT-013 の最小）: 受け入れ（重複・非表示・mute・snooze・上限）、見える物の順位、snooze 明け、期限切れ。
//! rail に出すのは `visible` の上から MAX_SHOWN 件だけ。popup / toast / focus の要求はここには無い（rail が読むだけ）。
use super::candidate::Category;
use super::domain::suggestion::{Suggestion, SuggestionStatus};
use super::feedback::Feedback;
use super::grounding::Grounding;
use super::lifecycle;

/// rail に同時に出す最大（DESIGN §5.1 / PROACTIVE §5）。ui::agent_rail::MAX_SHOWN と同じ値。
pub const MAX_SHOWN: usize = 3;
/// app ごとの未決の提案の上限。event の洪水で queue が膨らまない（AGT-013）。
pub const CAP_PER_APP: usize = 20;

/// 未決 = まだ利用者の判断か適用を待っている。重複の門と上限はこれだけを数える。
/// verified / failed は遷移表では終端ではない（→ reverted がある）が、提案としては決着済み:
/// verified は取り消しのために、failed は記録を閉じるためにだけ残す（domain の `is_terminal` とは別の物差し）。
pub fn is_open(status: SuggestionStatus) -> bool {
    !status.is_terminal() && !matches!(status, SuggestionStatus::Verified | SuggestionStatus::Failed)
}

/// queue の中の同じ提案 = 同じ app の同じ id。
fn same_id(x: &Tracked, s: &Suggestion) -> bool {
    x.suggestion.app_id == s.app_id && x.suggestion.id == s.id
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tracked {
    pub suggestion: Suggestion,
    pub category: Category,
    /// ranker::dedupe_key。feedback の非表示 / snooze はこの鍵で持続する。
    pub key: String,
    pub score: f32,
    pub grounding: Grounding,
    /// snoozed の間だけ Some（RFC3339 UTC）。
    pub resume_at: Option<String>,
    pub created_at: String,
    /// 最後に落ちた理由（stale / 失敗 / 非表示）。rail の文言に使う。
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intake {
    Queued,
    /// 同じ id を適用済み（verified）の記録が持つ（取り消しが id で引く）ので、版つきの id「id@vN」で入れた。以後の操作はこの id で。
    QueuedAs(String),
    /// 同じ鍵の未決の提案（か、全く同じ変更で落ちた提案）、または同じ app の同じ id の未決の提案が既に在る。
    Duplicate,
    /// 非表示にされた鍵（持続）。
    Hidden,
    Muted,
    /// snooze 中の鍵: queue には入るが resume_at まで見えない。
    Snoozed(String),
    Capped,
    /// candidate / queued 以外の status で持ち込まれた。
    BadStatus(SuggestionStatus),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    items: Vec<Tracked>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn items(&self) -> &[Tracked] {
        &self.items
    }
    /// 提案は (app_id, id) で 1 つ。heuristic の id（`category.target`）は app を含まないので、id だけで引くと別の app の
    /// 同じ id の提案を掴む（queue は app を切り替えても 1 つ）。
    pub fn get(&self, app_id: &str, id: &str) -> Option<&Tracked> {
        self.items.iter().find(|t| t.suggestion.app_id == app_id && t.suggestion.id == id)
    }
    pub fn get_mut(&mut self, app_id: &str, id: &str) -> Option<&mut Tracked> {
        self.items.iter_mut().find(|t| t.suggestion.app_id == app_id && t.suggestion.id == id)
    }
    /// 上限に数える未決の提案の数（適用済み・落ちた提案は数えない）。
    pub fn active_count(&self, app_id: &str) -> usize {
        self.items.iter().filter(|t| t.suggestion.app_id == app_id && is_open(t.suggestion.status)).count()
    }

    /// candidate → validated → queued（表を通す）。重複 / 非表示 / mute / 上限は入れない。
    pub fn intake(&mut self, mut t: Tracked, feedback: &Feedback, now: &str) -> Intake {
        use SuggestionStatus::{Failed, Verified};
        if feedback.is_hidden(&t.key) {
            return Intake::Hidden;
        }
        if feedback.is_muted(t.category) {
            return Intake::Muted;
        }
        // 意味が同じ未決の提案は 1 つだけ。適用済みの提案は止めない（同じ form の次の項目の修正など、鍵が同じ別の変更）。
        // 落ちた提案と全く同じ変更は入れない: auto が毎 frame 作り直して落ち直さない（記録を閉じるか履歴の削除まで）
        let same_failed = |x: &Tracked| x.suggestion.status == Failed && x.suggestion.change.operations == t.suggestion.change.operations;
        if self.items.iter().any(|x| x.key == t.key && (is_open(x.suggestion.status) || same_failed(x))) {
            return Intake::Duplicate;
        }
        // 以下の id の比較は全部同じ app の中だけ（別の app の同じ id を止めない・消さない）。
        // 適用済み（verified）の記録が持つ id は取り消し（Agent::revert が id で引く）のために残す: 新しい提案は版つきの id で入れる
        let renamed = self.items.iter().any(|x| same_id(x, &t.suggestion) && x.suggestion.status == Verified);
        if renamed {
            t.suggestion.id = format!("{}@v{}", t.suggestion.id, t.suggestion.app_version);
        }
        // 同じ id の未決の提案は差し替えない（プレビューで見せた物と違う変更を同じ id で適用させない）。適用済みの記録も差し替えない。
        // 作り直せるのは決着済み（rejected / reverted / 別の変更で落ちた failed）の id だけ
        if self.items.iter().any(|x| same_id(x, &t.suggestion) && (is_open(x.suggestion.status) || x.suggestion.status == Verified)) {
            return Intake::Duplicate;
        }
        match t.suggestion.status {
            SuggestionStatus::Candidate => {
                if lifecycle::validate(&mut t).is_err() || lifecycle::queue(&mut t).is_err() {
                    return Intake::BadStatus(t.suggestion.status);
                }
            }
            SuggestionStatus::Queued => {}
            other => return Intake::BadStatus(other),
        }
        if self.active_count(&t.suggestion.app_id) >= CAP_PER_APP {
            return Intake::Capped;
        }
        // 同じ app の同じ id の決着済み（stale で退けた・別の変更で落ちた等）は置き換える
        self.items.retain(|x| !same_id(x, &t.suggestion));
        if let Some(until) = feedback.snoozed_until(&t.key, now) {
            let until = until.to_owned();
            if lifecycle::snooze(&mut t, &until).is_ok() {
                self.items.push(t);
                return Intake::Snoozed(until);
            }
        }
        let id = t.suggestion.id.clone();
        self.items.push(t);
        if renamed {
            Intake::QueuedAs(id)
        } else {
            Intake::Queued
        }
    }

    /// 見える物（queued / shown / previewed）を score の高い順（同点は id 順）。rail は上から MAX_SHOWN 件を切る。
    pub fn visible(&self, app_id: &str) -> Vec<&Tracked> {
        let mut v: Vec<&Tracked> = self
            .items
            .iter()
            .filter(|t| {
                t.suggestion.app_id == app_id
                    && matches!(
                        t.suggestion.status,
                        SuggestionStatus::Queued | SuggestionStatus::Shown | SuggestionStatus::Previewed
                    )
            })
            .collect();
        v.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.suggestion.id.cmp(&b.suggestion.id)));
        v
    }

    /// snooze 明け（resume_at ≤ now）を queued に戻す。戻した数。
    pub fn wake(&mut self, now: &str) -> usize {
        let mut n = 0;
        for t in &mut self.items {
            if t.suggestion.status == SuggestionStatus::Snoozed
                && t.resume_at.as_deref().is_some_and(|r| r.as_bytes() <= now.as_bytes())
                && lifecycle::requeue(t).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// expires_at を過ぎた未終了の提案を退ける。
    pub fn expire(&mut self, now: &str) -> usize {
        let mut n = 0;
        for t in &mut self.items {
            if !t.suggestion.status.is_terminal()
                && t.suggestion.expires_at.as_deref().is_some_and(|e| e.as_bytes() < now.as_bytes())
                && lifecycle::reject(t, "Expired").is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// 構成が進んだ（version が違う）未終了の提案を退ける（§11「構成が更新されています」）。鍵は非表示にしない: refresh で作り直せる。
    pub fn retire_stale(&mut self, app_id: &str, version: u32) -> usize {
        self.retire_if(app_id, |s| s.app_version != version, "The setup was updated")
    }

    /// まだ決まっていない（rejected へ進める）提案のうち `gone` が真の物を退ける。適用後の記録（applied / verified / failed）は触らない。
    pub fn retire_if(&mut self, app_id: &str, gone: impl Fn(&Suggestion) -> bool, note: &str) -> usize {
        let mut n = 0;
        for t in &mut self.items {
            if t.suggestion.app_id == app_id
                && t.suggestion.status.can_transition(SuggestionStatus::Rejected)
                && gone(&t.suggestion)
                && lifecycle::reject(t, note).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// 履歴の削除: 決着済みの提案（rejected / reverted / failed）を落とす。落ちた変更もこれで出し直せる。
    /// verified は残す（取り消しが id で引く。重複の門も上限も数えないので何も止めない）。
    pub fn drop_settled(&mut self) {
        self.items.retain(|t| is_open(t.suggestion.status) || t.suggestion.status == SuggestionStatus::Verified);
    }
}
