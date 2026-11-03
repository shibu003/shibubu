//! 下の panel の Chat の列（L1-NORAIL・右の rail は消した）: 提案の行（在る時だけ・Chat の列の頭）→ 会話（`home::talk_thread`）→ 今の card → 返事の 1 行 → 底に chat bar（`home::chat_bar`・CHAT-ONE-1101）。
//! 提案の中身は `AgentRailSource`（unit A の `agent::Agent` が実装）から読むだけで、ここでは作らない。
//! §5.2 の行（14 semibold 題・12/18 の 1 行説明・文脈 label・`[プレビュー]` or `[適用]` + `後で` / `非表示`・展開で
//! 根拠 / 変更点 / 必要権限 / 取り消し）と、§5.4 の 1 つの diff 面（`draw_preview`。主領域に app.rs が出す）。
//! 列は focus を要求しない（AGT-006: 入力中の focus を奪わない・欄の focus は ⇧⌘P と Chat の tab で app.rs が置く）。

use super::motion;
use super::shell::{AppRow, Intent, Shell};
use super::states::{self, RegionState};
use super::theme::{self, Type, Variant, SEMIBOLD};
use eframe::egui::{self, FontFamily, FontId, Vec2};

/// §5.2 の 1 行。`labels` は `提案` `要接続` `承認が必要` `読み取りのみ` などの文脈 label。
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub title: String,
    pub reason: String,
    pub labels: Vec<String>,
    /// 主 action は `プレビュー` か `適用` のどちらか 1 つ（§5.2 #4）
    pub primary: ProposalAct,
    /// 展開（§5.2 #5）: 根拠の行
    pub evidence: Vec<String>,
    /// 変更点（1 op 1 行）
    pub changes: Vec<String>,
    /// 必要権限（空なら「なし」）
    pub scopes: Vec<String>,
    /// 取り消しの文
    pub undo: String,
    /// 信頼度は意味のある時だけ（§5.2 #5）
    pub confidence: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalAct {
    Preview,
    Apply,
    Later,
    Hide,
    /// 適用済みを元に戻す（§5.4 `取り消す`）
    Revert,
    /// diff 面を閉じる
    Close,
}

impl ProposalAct {
    pub const fn label(self) -> &'static str {
        match self {
            ProposalAct::Preview => "Preview",
            ProposalAct::Apply => "Apply",
            ProposalAct::Later => "Later",
            ProposalAct::Hide => "Hide",
            ProposalAct::Revert => "Undo",
            ProposalAct::Close => "Close",
        }
    }
    /// agent 側の操作に写す（app.rs: `agent.act(&id, act.to_act(), &now)`）。
    pub const fn to_act(self) -> super::super::agent::Act {
        use super::super::agent::Act;
        match self {
            ProposalAct::Preview => Act::Preview,
            ProposalAct::Apply => Act::Apply,
            ProposalAct::Later => Act::Later,
            ProposalAct::Hide => Act::Hide,
            ProposalAct::Revert => Act::Revert,
            ProposalAct::Close => Act::Close,
        }
    }
}

/// CHAT-011（121 通目・O21③「AI が自由な HTML や任意の code を作らず、Shibubu の用意した部品に内容と Action を割り当てる」）: 会話の card の 1 つの型。
/// 中身と手を割り当てるのは今の作り手 = 会話の model（`chat::Turn` の proposal / preview / needs → `Card::plan`）・提案の agent（`Proposal` →
/// `Card::suggestion`）。案と提案を描くのは `draw_card` 1 つ（見出し・本文の 1 文・読むだけの行・手の行・展開の節・下の hairline。上の hairline は呼び手 =
/// Home は会話との間に 1 本・提案の列は前の card の下の線）。**まだ別に描いている所が 3 つ**（CHAT-011 反証 P1）: Engine の card（`engine_card` →
/// `engine_view::draw`・hairline の枠）・Preview の面（`draw_preview`・自前の Apply / Later / Hide / Close）・`Recently applied` の Undo（`next_tab`）= 次の段で同じ型に写す。
/// 手は `CardAct` だけで、押せない手は理由の 1 行つきで灰（`why_not`）。空の理由は型では止まらない（`Option<String>`）ので `draw_card` の debug_assert が赤にする（P2）・
/// primary は作る 2 つの関数が先頭の 1 つにだけ立てる（型は数えない）。文は English（RULE-027）。AX の名前（Proposal・Preview plan・Build now・Change plan・Later・
/// Apply・Hide）は前の画面と同じ・`Suggestion` は CHAT-011 で足した見出し。labels.py の needle と録画の台本（short.md・full.md）はこの名前を使っていない（P3）。
/// 新しい型の理由: card を描く所が 3 つ（`home::talk_thread`・提案の行・`engine_view::draw`）に分かれ、灰の理由・primary の数・AX の名前が別々に決まる。
/// 置き場は描く側（ui）: 中身の正本は今の型のまま（`Turn` は会話と保存・`Proposal` は agent の queue・`EngineView` は run）で、Card は毎 frame それから作る。
/// 例（home.rs）: `if let Some(card) = Card::plan(&shell.talk, busy) { if let Some(i) = draw_card(ui, &card) { intent = Some(i) } }`
/// 保証しない（131・11/03 以降）: design profile・大きさの制約・推奨の密度（この型に field を足す）・model が card の種類を選ぶ事。
/// Wave: CHAT-011
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub kind: CardKind,
    /// CHAT-011 反証 Y7: card の鍵（提案 = 提案の id・案 = 案の文）。`Details` の開閉はこれで覚える（前は題と理由 = 同じ題と理由の 2 件が一緒に開いた）
    pub key: String,
    /// 本文の 1 文（案の文・提案の題）
    pub title: String,
    /// 読むだけの行（答えが要る問い・答えていない問い・Preview の行・提案の理由と文脈 label）
    pub lines: Vec<String>,
    /// 手（primary は 1 つだけ・先頭）
    pub acts: Vec<CardAct>,
    /// `Details` で開く節（見出し・行）。空なら `Details` を出さない（提案の Why / What changes / Needs access to / How to undo / Confidence・
    /// TALK-1101: 案の `Preview plan` の節 = `Turn::sections`・見出しは保存した会話から読むので String）
    pub details: Vec<(String, Vec<String>)>,
}

/// CHAT-011: card の種類 = 見出し（AX の名前も同じ語）。
/// Wave: CHAT-011
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// L2 の案（見出し `chat::PROPOSAL`）
    Plan,
    /// 提案の agent の 1 件（見出し `Suggestion`）
    Suggestion,
    /// PASSWORD-1101: password manager（見出し `vault::HEADING`）= chat で止めた password の保存か消す・名指しした manager を繋ぐ
    Password,
}

impl CardKind {
    /// 見出しの語（AX の名前）。
    /// Wave: CHAT-011
    pub const fn heading(self) -> &'static str {
        match self {
            CardKind::Plan => crate::domain::chat::PROPOSAL,
            CardKind::Suggestion => "Suggestion",
            CardKind::Password => crate::domain::vault::HEADING,
        }
    }

    /// TALK-1101: 節（`Card::details`）を開閉する手の語（閉じている時, 開いている時）と、最初に開いているか。案 = `Preview plan` / `Hide plan` で最初から開く
    /// （押して読んだ中身を隠さない・owner 191 通目「押してみるまでわからない」）・提案 = `Details` / `Hide details` で閉じている
    /// （NOTIFY-1101 追記・owner 192「chat の中の Close の文字の button はやめ」= 節を畳む手は Close と言わない）。
    /// Wave: TALK-1101
    pub const fn fold(self) -> (&'static str, &'static str, bool) {
        match self {
            CardKind::Plan => (crate::domain::chat::PREVIEW_PLAN, crate::domain::chat::HIDE_PLAN, true),
            CardKind::Suggestion => ("Details", "Hide details", false),
            CardKind::Password => ("Details", "Hide details", false),
        }
    }
}

/// CHAT-011: card の手 1 つ。`why_not` が Some の間は灰で、hover にその 1 行（押しても intent を返さない）。
/// Wave: CHAT-011
#[derive(Clone, Debug, PartialEq)]
pub struct CardAct {
    /// button の文（AX の名前）
    pub label: String,
    pub intent: Intent,
    pub primary: bool,
    pub why_not: Option<String>,
}

impl Card {
    /// L2 の案の card（会話の最後の Shibubu の答えに案が在る時だけ・`chat::pending_proposal`）。`busy` = 今の待ちの理由の 1 行（AI が読んでいる = `chat::WAIT_READING`・
    /// CLI を確かめている = その 1 行・AI を止めている = `chat::WAIT_STOPPING`・待ちが無ければ None。CHAT-011 反証 Y6: 前は bool で、何の待ちでも「build を待て」と言った）。
    /// TALK-1101（owner 191 通目）: 手 = 最初から `Build now`（primary・前の `Accept` を一本化）・`Change plan`・`Later`（ghost）・読む前だけ `Preview plan`（ghost）。
    /// `Build now` の灰 = 答えが要る問い（`chat::pending_needs`）の間（理由 `Answer this first: 問い`）か `busy` の間（その理由）だけ。`Preview plan` は needs の間だけ灰。
    /// 行 = `NEEDS_ANSWER: 問い`（DOGFOOD-X 周 5 #5）か、無ければ答えていない問い（`chat::open_question`）の `OPEN_QUESTION: 問い`。その後に Preview の行（`Turn::preview`）。
    /// 読んだ後の節（`Turn::sections`）は `details` = card の中に最初から開き、同じ場所の `Preview plan` / `Hide plan` で開閉（`CardKind::fold`）。
    /// Wave: CHAT-011
    pub fn plan(turns: &[crate::domain::chat::Turn], busy: Option<&str>) -> Option<Card> {
        use crate::domain::chat::{open_question, pending_needs, pending_proposal, Who, BUILD_NOW, CHANGE_PLAN, LATER, NEEDS_ANSWER, OPEN_QUESTION, PREVIEW_PLAN};
        let sentence = pending_proposal(turns)?;
        let ask = pending_needs(turns).map(|q| format!("{NEEDS_ANSWER}: {q}"));
        let (preview, details) = turns.iter().rev().find(|t| t.who == Who::Shibubu).map(|t| (t.preview.clone(), t.sections.clone())).unwrap_or_default();
        // DOGFOOD-X 周 2 回 1 #1: 選択肢を押さずに Build now できる問いは card に残す・周 5 #5: 答えが要る問いが在る間はそれを先に
        let mut lines: Vec<String> = ask.clone().or_else(|| open_question(turns).map(|q| format!("{OPEN_QUESTION}: {q}"))).into_iter().collect();
        let read = !preview.is_empty();
        lines.extend(preview);
        let act = |label: &str, intent, primary, why_not| CardAct { label: label.to_owned(), intent, primary, why_not };
        let mut acts = vec![
            act(BUILD_NOW, Intent::AcceptPlan, true, ask.clone().or(busy.map(str::to_owned))),
            act(CHANGE_PLAN, Intent::ChangePlan, false, None),
            act(LATER, Intent::LaterPlan, false, None),
        ];
        if !read {
            acts.push(act(PREVIEW_PLAN, Intent::PreviewPlan, false, ask));
        }
        Some(Card { kind: CardKind::Plan, key: sentence.to_owned(), title: sentence.to_owned(), lines, acts, details })
    }
    /// 提案の agent の 1 件。行 = 理由と文脈 label（` · ` で 1 行）・手 = 主 action（`Preview` か `Apply`・primary）・`Later`・`Hide`・
    /// 展開 = Why（根拠）・What changes（空 = Pages stay as they are）・Needs access to（空 = None）・How to undo・Confidence（在る時だけ）。
    /// Wave: CHAT-011
    pub fn suggestion(p: &Proposal) -> Card {
        let mut lines = vec![p.reason.clone()];
        if !p.labels.is_empty() {
            lines.push(p.labels.join(" · "));
        }
        let or = |v: &[String], empty: &str| if v.is_empty() { vec![empty.to_owned()] } else { v.to_vec() };
        let mut details = vec![("Why".to_owned(), p.evidence.clone()), ("What changes".into(), or(&p.changes, "Pages stay as they are")), ("Needs access to".into(), or(&p.scopes, "None")), ("How to undo".into(), vec![p.undo.clone()])];
        if let Some(c) = &p.confidence {
            details.push(("Confidence".into(), vec![c.clone()]));
        }
        let act = |act: ProposalAct, primary| CardAct { label: act.label().to_owned(), intent: Intent::Proposal { id: p.id.clone(), act }, primary, why_not: None };
        Card { kind: CardKind::Suggestion, key: p.id.clone(), title: p.title.clone(), lines, acts: vec![act(p.primary, true), act(ProposalAct::Later, false), act(ProposalAct::Hide, false)], details }
    }
    /// PASSWORD-1101 の 1・3: password manager の card。`Shell::held`（chat に打った password を止めた）が先、無ければ `Shell::vault_ask`（名指しした manager を繋ぐ）。
    /// held: 題 `vault::HELD`・行 `vault::HELD_NEXT`・手 `Save to {m}`（primary・`Connection { row: m.row(), act: Add }`）・`Delete`（同じ row の `Remove`）。値は card に載せない。
    /// vault_ask（見つかった）: 題 `Connect {m}`・行 = 待ちか誤りの 1 行（空なら無し）・手 `Connect`（primary・待つ間 = 行が `vault::WAIT_TOUCH_ID` の間は灰でその理由）・
    /// `Not now`（`Disconnect`）／（見つからない）: 題 `{m} isn't on this Mac`・行 = 入れ方（1Password = `vault::OP_SETUP`）・手 `Check again`（`CheckAgain`・primary）・`Not now`。
    /// 描く所: Home の会話（`home.rs`・会話が空でも出す）と Project の Chat の列（`chat_column`）= どちらも `draw_card`。
    /// 例: `if let Some(card) = Card::password(shell) { if let Some(i) = draw_card(ui, &card) { intent = Some(i) } }`
    /// Wave: PASSWORD-1101
    pub fn password(shell: &Shell) -> Option<Card> {
        use super::connections::ConnAct;
        use crate::domain::vault::{Manager, BW_INSTALL, HELD, HELD_NEXT, OP_INSTALL, WAIT_TOUCH_ID};
        let act = |label: String, m: Manager, what: ConnAct, primary: bool, why_not: Option<String>| CardAct { label, intent: Intent::Connection { row: m.row(), act: what }, primary, why_not };
        if let Some((_, m)) = &shell.held {
            let acts = vec![act(format!("Save to {}", m.label()), *m, ConnAct::Add, true, None), act("Delete".into(), *m, ConnAct::Remove, false, None)];
            return Some(Card { kind: CardKind::Password, key: "password:held".into(), title: HELD.into(), lines: vec![HELD_NEXT.into()], acts, details: Vec::new() });
        }
        let (m, found, line) = shell.vault_ask.as_ref()?;
        let m = *m;
        let (title, lines, acts) = if *found {
            let waiting = (line == WAIT_TOUCH_ID).then(|| line.clone());
            let lines = if line.is_empty() { Vec::new() } else { vec![line.clone()] };
            (format!("Connect {}", m.label()), lines, vec![act("Connect".into(), m, ConnAct::Connect, true, waiting), act("Not now".into(), m, ConnAct::Disconnect, false, None)])
        } else {
            let how = match m {
                Manager::OnePassword => vec![OP_INSTALL.to_owned()],
                Manager::Bitwarden => vec![BW_INSTALL.to_owned()],
                Manager::Keychain => Vec::new(),
            };
            (format!("{} isn't on this Mac", m.label()), how, vec![act("Check again".into(), m, ConnAct::CheckAgain, true, None), act("Not now".into(), m, ConnAct::Disconnect, false, None)])
        };
        Some(Card { kind: CardKind::Password, key: format!("password:{}", m.scheme()), title, lines, acts, details: Vec::new() })
    }
}

/// CHAT-011: card を描く 1 か所。押せる手を押した時だけその intent を返す（灰の手は hover に `why_not`）。primary は `Variant::Primary`・他は ghost。
/// 例（agent_rail の Chat の列）: `for p in source.proposals() { if let Some(i) = draw_card(ui, &Card::suggestion(&p)) { intent = Some(i) } }`
/// Wave: CHAT-011
pub fn draw_card(ui: &mut egui::Ui, card: &Card) -> Option<Intent> {
    debug_assert!(card.acts.iter().all(|a| a.why_not.as_deref() != Some("")), "理由の無い灰の手: {:?}", card.acts);
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    ui.add_space(theme::SP_3);
    theme::label(ui, Type::BodyStrong, card.kind.heading(), t.text_primary);
    ui.add_space(theme::SP_1);
    ui.add(egui::Label::new(Type::Body.text(card.title.as_str()).color(t.text_primary)).wrap());
    for line in &card.lines {
        ui.add_space(theme::SP_1);
        ui.add(egui::Label::new(Type::Small.text(line.as_str()).color(t.text_secondary)).wrap());
    }
    ui.add_space(theme::SP_3);
    let open_id = egui::Id::new(("kangae-card-open", card.kind.heading(), &card.key));
    // TALK-1101: 案の節は最初から開く（覚えるのは既定から替えたか）
    let (shut, opened, first) = card.kind.fold();
    let flipped = toggle_flag(ui, open_id);
    let open = !card.details.is_empty() && flipped != first;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SP_2;
        for act in &card.acts {
            let variant = if act.primary { Variant::Primary } else { Variant::Ghost };
            let mut pressed = ui.add_enabled_ui(act.why_not.is_none(), |ui| theme::button(ui, &act.label, variant)).inner;
            if let Some(why) = &act.why_not {
                pressed = pressed.on_disabled_hover_text(why.as_str());
            }
            if pressed.clicked() {
                intent = Some(act.intent.clone());
            }
        }
        if !card.details.is_empty() && theme::button(ui, if open { opened } else { shut }, Variant::Ghost).clicked() {
            set_flag(ui, open_id, !flipped);
        }
    });
    if open {
        ui.add_space(theme::SP_2);
        for (head, lines) in &card.details {
            theme::label(ui, Type::Helper, head, t.text_tertiary);
            for line in lines {
                theme::label(ui, Type::Helper, line, t.text_primary);
            }
            ui.add_space(theme::SP_1);
        }
    }
    ui.add_space(theme::SP_3);
    theme::rule(ui);
    intent
}

/// rail が読む元。unit A の `agent::Agent` がこれを実装する。
pub trait AgentRailSource {
    /// 表示する提案（順位順）。3 件までは rail が切る。
    fn proposals(&self) -> Vec<Proposal>;
    /// `このアプリについて` の (項目, 値)。
    fn about(&self) -> Vec<(String, String)>;
    /// `実行履歴`（新しい順）。
    fn history(&self) -> Vec<String>;
    /// 検討中なら `提案を検討中`（§11）。完了は約束しない。
    fn planning(&self) -> bool {
        false
    }
    /// 取り消せる適用 (id, 題)。§5.4 の `取り消す` の affordance。
    fn revertible(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// 提案 engine が無い間の実装: 何も捏造しない。
pub struct NoAgent;
impl AgentRailSource for NoAgent {
    fn proposals(&self) -> Vec<Proposal> {
        Vec::new()
    }
    fn about(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn history(&self) -> Vec<String> {
        Vec::new()
    }
}

impl AgentRailSource for super::super::agent::Agent {
    fn proposals(&self) -> Vec<Proposal> {
        self.rail_rows(&super::super::agent::now_rfc3339())
            .into_iter()
            .map(|r| Proposal {
                id: r.id,
                title: r.title,
                reason: r.reason,
                labels: r.labels,
                primary: if r.primary_apply { ProposalAct::Apply } else { ProposalAct::Preview },
                evidence: r.evidence,
                changes: r.changes,
                scopes: r.scopes,
                undo: r.undo,
                confidence: r.confidence,
            })
            .collect()
    }
    fn about(&self) -> Vec<(String, String)> {
        self.about_rows()
    }
    fn history(&self) -> Vec<String> {
        self.history_lines().iter().rev().cloned().collect()
    }
    fn revertible(&self) -> Vec<(String, String)> {
        self.undo_rows()
    }
}

/// rail に同時に出す提案の最大（DESIGN §5.1）。値の正本は agent の queue（`mark_shown` と同じ数）。
pub use super::super::agent::MAX_SHOWN;

/// CHAT-ONE-1101（L1-NORAIL の席・owner 184「chatbarは１つに統合」）: 下の panel の Chat の tab（`terminal_panel::draw` の `chat` から app.rs が呼ぶ・`ui` = 帯の下の残り）。
/// 列（owner 191: panel の幅いっぱい・左右は `Tier::gutter` だけ）に、上から 提案の行（提案か取り消せる適用が在る時だけ）→ 会話（`home::talk_thread` = Home と同じ描き方・
/// 案の card〈`Preview plan` / `Accept`〉も同じ）→ 今の Engine の card（承認・決める・結果 = `engine_view::draw` と同じ button の名前）→ 返事の 1 行（`shell.chat_note`）→
/// 底に chat bar（`home::chat_bar` = Home と同じ部品・欄の AX の名前も同じ `What to create`）。結果の 1 行（済み・止めた・失敗）は card にしない（済み・止めたは会話の 1 行・失敗は右上の toast・NOTIFY-1101 追記・owner 192）。
/// `turns` = 保存した会話（app.rs が答えを待つ間の `chat::TALK_WAIT` を後ろに足して渡す）・`wait` = `Accept` を灰にする待ちの理由。
/// Terminal の tab と畳んだ panel では呼ばれない（grid が panel の底まで = 欄は Chat の tab にだけ在る）。返す = 押した手。
/// CHAT-SIDE-1101: 右の sidebar（`chat_side` の中の同じ panel の Chat の tab）でも同じ関数（会話・下書きは同じ物）。`side` = 右の sidebar の中（追記 220: 左右の余白は panel の分だけ =
/// `Tier::gutter` を足さない・HOME-CHATBAR-1101 追記 222: chat bar の下の行を 2 段）。
/// 例（app.rs）: `generated.draw_terminal_panel(ctx, &project, setup, Some(&mut |ui| if let Some(i) = chat_column(ui, &mut shell, &turns, card.as_ref(), wait, &agent, &rows, busy, side) { said = Some(i) }), None)`
pub fn chat_column(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    turns: &[crate::domain::chat::Turn],
    card: Option<&super::engine_view::EngineView>,
    wait: Option<&str>,
    source: &dyn AgentRailSource,
    apps: &[AppRow],
    busy: bool,
    side: bool,
) -> Option<Intent> {
    // NOTIFY-1101 追記（owner 192）: 結果の 1 行は card にしない（済み・止めたは会話の 1 行・失敗は toast）= card は人が今答える物だけ
    let card = card.filter(|c| !matches!(c, super::engine_view::EngineView::Line { .. }));
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    let all = ui.available_rect_before_wrap();
    let gutter = if side { 0.0 } else { super::shell::Tier::current(ui.ctx()).gutter };
    let w = (all.width() - 2.0 * gutter).max(0.0);
    let (left, right) = (all.center().x - w / 2.0, all.center().x + w / 2.0);
    // chat bar の高さは前の pass で測った値（変わった pass は捨てて測り直す・egui の request_discard）
    let bar_id = egui::Id::new("kangae.chat.bar-h");
    let bar_h = ui.ctx().data(|d| d.get_temp::<f32>(bar_id)).unwrap_or(super::home::BAR_H);
    let bar_top = (all.bottom() - theme::SP_3 - bar_h).max(all.top());
    let note_h = if shell.chat_note.is_some() { Type::Helper.line_height() + theme::SP_2 } else { 0.0 };
    let column = egui::Rect::from_min_max(egui::pos2(left, all.top()), egui::pos2(right, (bar_top - note_h).max(all.top())));
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Min)));
    // PASSWORD-1101: password manager の card（chat で止めた値・名指しした manager）は会話の下
    let vault = Card::password(shell);
    egui::ScrollArea::vertical().id_salt("kangae-chat-column").max_height(column.height()).auto_shrink([false, false]).stick_to_bottom(true).show(&mut inner, |ui| {
        ui.set_width(w);
        if !source.proposals().is_empty() || !source.revertible().is_empty() {
            if let Some(i) = next_tab(ui, source) {
                intent = Some(i);
            }
        }
        if turns.is_empty() && card.is_none() && vault.is_none() {
            ui.add_space(theme::SP_6);
            theme::label(ui, Type::Helper, "Nothing here yet. Ask, or say what to change, in the field below", t.text_secondary);
        }
        if let Some(i) = super::home::talk_thread(ui, turns, wait, false) {
            intent = Some(i);
        }
        if let Some(i) = vault.as_ref().and_then(|c| draw_card(ui, c)) {
            intent = Some(i);
        }
        if let Some(card) = card {
            ui.add_space(theme::SP_3);
            if let Some(i) = engine_card(ui, card) {
                intent = Some(i);
            }
        }
        ui.add_space(theme::SP_3);
    });
    // 返事の 1 行が替わったら下から slide（toast・§15.2-1）
    let t0 = motion::changed_at(ui.ctx(), egui::Id::new("kangae.chat.note"), &shell.chat_note);
    if let Some(note) = &shell.chat_note {
        let at = egui::Rect::from_min_max(egui::pos2(left, bar_top - note_h), egui::pos2(right, bar_top));
        let mut line = ui.new_child(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)));
        motion::toast_in(&mut line, "kangae.chat.note", t0, |ui| theme::label(ui, Type::Helper, note, t.text_tertiary));
    }
    let bar = egui::Rect::from_min_max(egui::pos2(left, bar_top), egui::pos2(right, all.bottom()));
    let mut at = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::top_down(egui::Align::Min)));
    let (i, rect) = super::home::chat_bar(&mut at, shell, apps, busy, None, side);
    if i.is_some() {
        intent = i;
    }
    if (rect.height() - bar_h).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(bar_id, rect.height()));
        ui.ctx().request_discard("chat bar size");
        ui.ctx().request_repaint();
    }
    ui.advance_cursor_after_rect(all);
    intent
}

/// CHAT-SIDE-1101: 右の sidebar の幅（境を引いて変える範囲・既定）。owner 204 の spec「幅 320〜560pt」。
pub const SIDE_W_MIN: f32 = 320.0;
pub const SIDE_W_MAX: f32 = 560.0;
pub const SIDE_W: f32 = 380.0;

/// CHAT-SIDE-1101（owner 204 の 2「右sidebar復活。右でchatもできるように。したか右かを選べるように」）・追記 219〜221: 右の sidebar = 下の panel と同じ部品の置き場。
/// 右の panel（幅 `SIDE_W_MIN..=SIDE_W_MAX`・境を引いて変える = egui が覚える・地と左右の余白は下の panel と同じ）の中で `body`（`Generated::draw_terminal_panel` の
/// `terminal_panel::draw` = Chat と Terminal の tab の横 scroll の列・会話か grid）を呼ぶ。上の「Chat」の字・畳む ›・畳んだ帯の ‹ は無い（追記 219・220。出し入れは header の ◨・帯の左端の開閉は下の panel だけ = PANEL-SYNC-1101 で右の ⇩ を外した）。
/// header の後・CentralPanel より先に呼ぶ（右の sidebar は header の下から窓の下まで・Page はその分だけ狭い）。
/// `width` = 保存した幅（`UiSettings::chat_width`・起動の後の最初の frame の幅 = egui の memory が空の時）。返す = 今の幅（境を引いている間は None = 離してから保存する）。
/// 例（generated.rs）: `if let Some(w) = agent_rail::chat_side(ctx, *saved, body) { *saved = w }`
/// Wave: CHAT-SIDE-1101
pub fn chat_side(ctx: &egui::Context, width: f32, body: impl FnOnce(&mut egui::Ui)) -> Option<f32> {
    let t = theme::tokens(ctx);
    let frame = egui::Frame::new().fill(t.surface_1).inner_margin(egui::Margin::symmetric(super::terminal_panel::INSET_X as i8, 0));
    let shown = egui::SidePanel::right("kangae-chat-side")
        .resizable(true)
        .default_width(width.clamp(SIDE_W_MIN, SIDE_W_MAX))
        .width_range(SIDE_W_MIN..=SIDE_W_MAX)
        .show_separator_line(true)
        .frame(frame)
        .show(ctx, body);
    // 境を引いている間（pointer を押している間）は保存しない = 離した frame の幅を 1 回
    (!ctx.input(|i| i.pointer.any_down())).then(|| shown.response.rect.width())
}

/// 今の Engine の card 1 枚（hairline の枠・`engine_view::draw` と同じ button の名前）。Chat の tab の会話の最後に描く。
fn engine_card(ui: &mut egui::Ui, card: &super::engine_view::EngineView) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .stroke(theme::hairline(&t))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_MD))
        .inner_margin(egui::Margin::same(theme::SP_3 as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            super::engine_view::draw(ui, card).map(|act| Intent::Page(super::shell::PageIntent::Engine(act)))
        })
        .inner
}

fn toggle_flag(ui: &egui::Ui, id: egui::Id) -> bool {
    ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false)
}
fn set_flag(ui: &egui::Ui, id: egui::Id, value: bool) {
    ui.ctx().data_mut(|d| d.insert_temp(id, value));
}

fn next_tab(ui: &mut egui::Ui, source: &dyn AgentRailSource) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    if source.planning() {
        theme::label(ui, Type::Helper, "Looking for suggestions", t.text_secondary);
        ui.add_space(theme::SP_2);
    }
    let proposals = source.proposals();
    let revertible = source.revertible();
    let show_all_id = egui::Id::new("kangae-rail-show-all");
    let show_all = toggle_flag(ui, show_all_id);
    let limit = if show_all { proposals.len() } else { MAX_SHOWN };
    for p in proposals.iter().take(limit) {
        if let Some(i) = draw_card(ui, &Card::suggestion(p)) {
            intent = Some(i);
        }
    }
    if proposals.len() > MAX_SHOWN {
        ui.add_space(theme::SP_2);
        let label = if show_all { "Show fewer".to_string() } else { format!("Show {} more", proposals.len() - MAX_SHOWN) };
        if theme::button_compact(ui, &label, Variant::Ghost).clicked() {
            set_flag(ui, show_all_id, !show_all);
        }
    }
    if !revertible.is_empty() {
        ui.add_space(theme::SP_4);
        theme::label(ui, Type::Helper, "Recently applied", t.text_tertiary);
        for (id, title) in revertible {
            ui.add_space(theme::SP_1);
            theme::label(ui, Type::Body, title, t.text_primary);
            if theme::button_compact(ui, ProposalAct::Revert.label(), Variant::Ghost).clicked() {
                intent = Some(Intent::Proposal { id, act: ProposalAct::Revert });
            }
            ui.add_space(theme::SP_2);
            theme::rule(ui);
        }
    }
    intent
}

fn detail_block(ui: &mut egui::Ui, head: &str, lines: &[String], empty: &str) {
    let t = theme::tokens(ui.ctx());
    theme::label(ui, Type::Helper, head, t.text_tertiary);
    if lines.is_empty() {
        theme::label(ui, Type::Helper, empty, t.text_secondary);
    }
    for line in lines {
        theme::label(ui, Type::Helper, line, t.text_primary);
    }
    ui.add_space(theme::SP_1);
}

/// §5.4 の 1 つの diff 面（主領域に出す。新しい app session にはしない）。main が 900 以上なら before / after を並べ、
/// 狭ければ切り替え。戻り値は押された操作（適用 / 後で / 非表示 / 閉じる）。
pub fn draw_preview(ui: &mut egui::Ui, p: &super::super::agent::Preview) -> Option<Intent> {
    use super::super::agent::{grounding::risk_wording, PreviewCta};
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    let cta = p.cta.label();
    theme::label(ui, Type::HeadingMd, format!("Preview: {}", p.title), t.text_primary);
    theme::label(ui, Type::Helper, &p.summary, t.text_secondary);
    theme::label(ui, Type::Helper, format!("v{} → v{}", p.version_from, p.version_to), t.text_tertiary);
    ui.add_space(theme::SP_4);
    let side_by_side = ui.available_width() >= 900.0;
    if side_by_side {
        ui.columns(2, |cols| {
            diff_column(&mut cols[0], "Before", &p.before);
            diff_column(&mut cols[1], "After", &p.after);
        });
    } else {
        let after_id = egui::Id::new(("kangae-preview-after", &p.id));
        let after = toggle_flag(ui, after_id);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::SP_2;
            if theme::chip(ui, if after { "Show before" } else { "Show after" }).clicked() {
                set_flag(ui, after_id, !after);
            }
        });
        ui.add_space(theme::SP_2);
        if after {
            diff_column(ui, "After", &p.after);
        } else {
            diff_column(ui, "Before", &p.before);
        }
    }
    ui.add_space(theme::SP_4);
    let affected = [("UI", &p.affected_nodes), ("Action", &p.affected_actions), ("Workflow", &p.affected_workflows)]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}: {}", v.join(", ")))
        .collect::<Vec<_>>();
    detail_block(ui, "Effect", &affected, "None");
    detail_block(ui, "Risk", std::slice::from_ref(&risk_wording(p.risk).to_owned()), "");
    let scopes = if p.scopes.is_empty() { vec!["None".to_owned()] } else { p.scopes.clone() };
    detail_block(ui, "Needs access to", &scopes, "");
    detail_block(ui, "How to undo", std::slice::from_ref(&p.rollback), "");
    if let Some(c) = &p.confidence {
        detail_block(ui, "Confidence", std::slice::from_ref(c), "");
    }
    ui.add_space(theme::SP_4);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SP_2;
        match &p.cta {
            PreviewCta::Apply | PreviewCta::Approve => {
                if theme::button(ui, cta, Variant::Primary).clicked() {
                    intent = Some(Intent::Proposal { id: p.id.clone(), act: ProposalAct::Apply });
                }
            }
            PreviewCta::Blocked(reason) => {
                ui.add_enabled_ui(false, |ui| {
                    theme::button(ui, cta, Variant::Primary).on_disabled_hover_text(reason);
                });
                super::states::inline(ui, super::states::RegionState::Blocked, reason);
            }
        }
        for act in [ProposalAct::Later, ProposalAct::Hide, ProposalAct::Close] {
            if theme::button(ui, act.label(), Variant::Ghost).clicked() {
                intent = Some(Intent::Proposal { id: p.id.clone(), act });
            }
        }
    });
    intent
}

fn diff_column(ui: &mut egui::Ui, head: &str, lines: &[String]) {
    let t = theme::tokens(ui.ctx());
    theme::label(ui, Type::Helper, head, t.text_tertiary);
    egui::Frame::new()
        .fill(t.surface_2)
        .stroke(theme::hairline(&t))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(egui::Margin::same(theme::SP_3 as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if lines.is_empty() {
                theme::label(ui, Type::Mono, "(empty)", t.text_secondary);
            }
            for line in lines {
                theme::label(ui, Type::Mono, line, t.text_primary);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::super::shell::View;
    use super::*;

    struct Many(usize);
    impl AgentRailSource for Many {
        fn proposals(&self) -> Vec<Proposal> {
            (0..self.0)
                .map(|i| Proposal {
                    id: format!("p{i}"),
                    title: format!("提案 {i}"),
                    reason: "理由".into(),
                    labels: vec!["提案".into()],
                    primary: ProposalAct::Apply,
                    evidence: vec!["アプリの状態: composition.x".into()],
                    changes: vec!["+ x-search（input）を root の下に".into()],
                    scopes: Vec::new(),
                    undo: "適用後に「取り消す」で v1 の構成に戻せます".into(),
                    confidence: None,
                })
                .collect()
        }
        fn about(&self) -> Vec<(String, String)> {
            Vec::new()
        }
        fn history(&self) -> Vec<String> {
            Vec::new()
        }
    }

    /// CHAT-011: 案の card と提案の card は同じ型で、primary は 1 つ・押せない手は理由つきで灰・読む前の案の card に節は無い・提案の card は Details の 4 節。
    /// TALK-1101: 案の先頭は最初から `Build now`（needs の間は `Answer this first: 問い` で灰）・`Preview plan` は読む前だけ・読んだ節は details。
    /// 負の対照（束の arm）: `Card::plan` の needs の灰を外す → 「答える前の primary が灰でない」で赤。
    #[test]
    fn chat011_plan_and_suggestion_are_one_card_type() {
        use crate::domain::chat::{Mark, Turn, Who, BUILD_NOW, CHANGE_PLAN, LATER, NEEDS_ANSWER, PREVIEW_PLAN, PROPOSAL};
        let turn = |who, text: &str| Turn { who, text: text.into(), mark: Mark::None, at: "t".into(), attachments: Vec::new(), choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None };
        let plan = "Copy the 2022 numbers into my budget table in Numbers";
        let asked = vec![turn(Who::You, "Copy the 2022 numbers into my budget table"), Turn { proposal: Some(plan.into()), needs: Some("Which Numbers file?".into()), ..turn(Who::Shibubu, "Sure. Which Numbers file?") }];
        let card = Card::plan(&asked, None).expect("案の card が無い");
        assert_eq!((card.kind.heading(), card.title.as_str()), (PROPOSAL, plan));
        assert_eq!(card.lines.first().cloned(), Some(format!("{NEEDS_ANSWER}: Which Numbers file?")));
        assert_eq!(card.acts.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), [BUILD_NOW, CHANGE_PLAN, LATER, PREVIEW_PLAN]);
        assert_eq!(card.acts.iter().filter(|a| a.primary).count(), 1, "primary は 1 つ");
        assert_eq!(card.acts[0].why_not, Some(format!("{NEEDS_ANSWER}: Which Numbers file?")), "答える前の Build now が灰でない・理由に問いが無い");
        assert!(card.details.is_empty(), "案の card に Details");
        let mut answered = asked.clone();
        answered[1].needs = None;
        answered[1].preview = vec!["2 pages: Budget, Sources".into()];
        answered[1].sections = vec![(crate::domain::chat::PLAN_STEPS.into(), vec!["1. Open Budget".into()])];
        let card = Card::plan(&answered, Some(crate::domain::chat::WAIT_READING)).unwrap();
        assert_eq!((card.acts[0].label.as_str(), &card.acts[0].intent), (BUILD_NOW, &Intent::AcceptPlan));
        assert!(!card.acts.iter().any(|a| a.label == PREVIEW_PLAN), "読んだ後に Preview plan の手（開閉は details の手）");
        assert_eq!(card.details, answered[1].sections, "読んだ節が card に無い");
        // CHAT-011 反証 Y6: 灰の理由はその時の待ち（前は何の待ちでも「Wait for the current build to finish」）
        assert_eq!(card.acts[0].why_not.as_deref(), Some(crate::domain::chat::WAIT_READING), "読んでいる間の Build now が押せる・理由が今の待ちでない");
        assert!(card.lines.contains(&"2 pages: Budget, Sources".to_owned()), "Preview の行が無い: {:?}", card.lines);
        assert!(Card::plan(&answered[..1], None).is_none(), "案の無い会話に card");
        let p = Many(1).proposals().remove(0);
        let card = Card::suggestion(&p);
        assert_eq!((card.kind.heading(), card.title.as_str()), ("Suggestion", "提案 0"));
        assert_eq!(card.acts.iter().map(|a| a.intent.clone()).collect::<Vec<_>>(), [ProposalAct::Apply, ProposalAct::Later, ProposalAct::Hide].map(|act| Intent::Proposal { id: "p0".into(), act }));
        assert!(card.acts.iter().all(|a| a.why_not.is_none()) && card.acts[0].primary);
        assert_eq!(card.details.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["Why", "What changes", "Needs access to", "How to undo"]);
    }

    /// 1440 幅の主領域に開いた Chat の列（会話は空）を 2 frame 描き、AccessKit の木（cua-driver が読むのと同じ物）に出た文字を返す
    /// （button は label、文字の Label は value に入る）。
    fn drawn_labels(source: &dyn AgentRailSource) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut shell = Shell::new(View::Project { project: "p".into(), page: "x".into() });
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0));
        let mut labels = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| chat_column(ui, &mut shell, &[], None, None, source, &[], false, false));
            });
            labels = out
                .platform_output
                .accesskit_update
                .map(|u| u.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(String::from)).collect())
                .unwrap_or_default();
        }
        labels
    }

    #[test]
    fn rail_draws_at_most_three_proposal_rows_and_puts_the_rest_under_see_all() {
        let labels = drawn_labels(&Many(5));
        let rows = labels.iter().filter(|l| *l == "Hide").count();
        assert_eq!(rows, MAX_SHOWN, "{labels:?}");
        assert!(labels.iter().any(|l| l == "提案 2") && !labels.iter().any(|l| l == "提案 3"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Show 2 more"), "{labels:?}");
        // 負の対照: 3 件以下なら全部出て「すべて見る」は無い
        let labels = drawn_labels(&Many(2));
        assert_eq!(labels.iter().filter(|l| *l == "Hide").count(), 2, "{labels:?}");
        assert!(!labels.iter().any(|l| l.starts_with("Show ") && l.ends_with(" more")), "{labels:?}");
        // L1-NORAIL: 0 件は何も出さない（Chat の列の頭に行を出すのは提案が在る時だけ）
        let labels = drawn_labels(&Many(0));
        assert!(!labels.iter().any(|l| l == "No new suggestions" || l == "Hide"), "{labels:?}");
    }

    /// 本物の `agent::Agent` を source にして描く（AgentRailSource の実装）: 題・説明・文脈 label・[適用] と 後で / 非表示。
    #[test]
    fn rail_draws_the_real_agents_rows() {
        use crate::agent::{Agent, Observed};
        use crate::domain::patch::AppRevision;
        let def = serde_json::from_value(serde_json::json!({
            "schema_version": "3.0", "id": "rail.test", "version": 1,
            "goal": { "literal_goal": "在庫を整理したい", "outcomes": ["商品を一覧で見る"] },
            "composition": { "root": { "key": "root", "component": "column", "children": [
                { "key": "items", "component": "table", "label": "商品", "binding": { "source_ref": "local.items", "path": "items" } }
            ] } },
            "actions": [],
            "sources": [ { "id": "local.items", "kind": "local_collection", "readiness": "local" } ],
            "agent": { "suggestions_enabled": true, "autonomy_level": "suggest_only", "entrypoint": "rail" },
            "state": { "readiness": "ready", "execution_mode": "local_ready" }
        }))
        .unwrap();
        let mut agent = Agent::new();
        agent.set_app(AppRevision::new(def).unwrap());
        agent.observe(Observed { rows: [("local.items".to_owned(), 25)].into(), ..Default::default() });
        agent.refresh("2026-10-17T09:00:00Z");
        let labels = drawn_labels(&agent);
        for want in ["Suggestion", "Add search to “商品”", "There are 25 rows, too many to scan by eye", "Suggested", "Apply", "Later", "Hide", "Details"] {
            assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
        }
        assert!(!labels.iter().any(|l| l == "No new suggestions"), "{labels:?}");
    }

    /// draw_preview を主領域（CentralPanel・1440 幅）に描き、AccessKit の木の (文字, disabled) を返す。
    fn drawn_preview(p: &crate::agent::Preview) -> Vec<(String, bool)> {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0));
        let mut nodes = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_preview(ui, p);
                });
            });
            nodes = out
                .platform_output
                .accesskit_update
                .map(|u| u.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(|l| (l.to_owned(), n.is_disabled()))).collect())
                .unwrap_or_default();
        }
        nodes
    }

    /// §5.4: 変更前 / 変更後を 1 つの面に並べて見せ、塞がれた提案の [Apply] は押せない。外部に効く変更は「Approve and apply」。
    #[test]
    fn preview_pane_shows_before_and_after_and_never_offers_a_blocked_apply() {
        use crate::agent::{Preview, PreviewCta};
        use crate::domain::suggestion::Risk;
        let preview = |cta| Preview {
            id: "p".into(),
            title: "「商品」に検索を追加".into(),
            summary: "検索欄を 1 つ足す".into(),
            before: vec!["root (column)".into(), "  items (table)".into()],
            after: vec!["root (column)".into(), "  items-search (input)".into(), "  items (table)".into()],
            affected_nodes: vec!["items".into()],
            affected_actions: Vec::new(),
            affected_workflows: Vec::new(),
            risk: Risk::LocalReversible,
            scopes: Vec::new(),
            undoable: true,
            confidence: None,
            rollback: "適用後に「取り消す」で v1 の構成に戻せます".into(),
            version_from: 1,
            version_to: 2,
            cta,
        };
        let has = |nodes: &[(String, bool)], text: &str| nodes.iter().any(|(l, _)| l == text);
        let apply = |nodes: &[(String, bool)], text: &str| nodes.iter().find(|(l, _)| l == text).map(|(_, disabled)| *disabled);
        let nodes = drawn_preview(&preview(PreviewCta::Apply));
        for want in ["Before", "After", "  items-search (input)", "v1 → v2", "Local · can be undone"] {
            assert!(has(&nodes, want), "{want}: {nodes:?}");
        }
        assert_eq!(apply(&nodes, "Apply"), Some(false), "{nodes:?}");
        let nodes = drawn_preview(&preview(PreviewCta::Blocked("権限 'mail.send' の許可が必要です".into())));
        assert_eq!(apply(&nodes, "Apply"), Some(true), "塞がれた [Apply] は disabled: {nodes:?}");
        let nodes = drawn_preview(&preview(PreviewCta::Approve));
        assert_eq!(apply(&nodes, "Approve and apply"), Some(false), "{nodes:?}");
    }

    struct Talk(Option<(Vec<crate::domain::chat::Turn>, Option<super::super::engine_view::EngineView>)>);
    impl AgentRailSource for Talk {
        fn proposals(&self) -> Vec<Proposal> {
            Vec::new()
        }
        fn about(&self) -> Vec<(String, String)> {
            Vec::new()
        }
        fn history(&self) -> Vec<String> {
            Vec::new()
        }
    }

    /// 開いた Chat の列を 2 frame 描いた AX の (id, 名前)。`press` の名前が在れば 3 frame 目に AX の Click で押し、返った Intent も返す。
    fn chat_ax(source: &Talk, press: Option<&str>) -> (Vec<(egui::accesskit::NodeId, String)>, Option<Intent>) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut shell = Shell::new(View::Project { project: "p".into(), page: "x".into() });
        let (turns, card) = source.0.clone().unwrap_or_default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0));
        let mut nodes: Vec<(egui::accesskit::NodeId, String)> = Vec::new();
        let mut got = None;
        for n in 0..3 {
            let events = match (n, press) {
                (2, Some(label)) => {
                    let target = nodes.iter().find(|(_, l)| l == label).unwrap_or_else(|| panic!("{label} が無い: {nodes:?}")).0;
                    vec![egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target, data: None })]
                }
                _ => Vec::new(),
            };
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(i) = chat_column(ui, &mut shell, &turns, card.as_ref(), None, source, &[], false, false) {
                        got = Some(i);
                    }
                });
            });
            if n < 2 {
                // 選べる label（card の 1 行）は文の選択のために同じ文の TextRun を子に持つ = 1 つの描画として数える
                nodes = out.platform_output.accesskit_update.map(|u| u.nodes.iter().filter(|(_, x)| x.role() != egui::accesskit::Role::TextRun).filter_map(|(id, x)| x.label().or(x.value()).map(|l| (*id, l.to_owned()))).collect()).unwrap_or_default();
            }
        }
        (nodes, got)
    }

    /// G86a: Chat の列 = 人の一言・Shibubu の答え・最後に今の card。card の `Approve and send` は窓に 1 つで、押すと Page の 1 行と同じ `EngineAct::Approve`
    /// （`PageIntent::Engine`）。結果の 1 行は card にせず会話の 1 行だけ（NOTIFY-1101 追記 = 2 度出さない）。
    /// 負の対照（G86M）: card = card を描かない → Approve and send が無い / `card.filter` の Line を外す → 結果の文が 2 つ
    #[test]
    fn g86_the_chat_tab_draws_what_was_said_and_one_approve_button() {
        use super::super::engine_view::{ApprovalStep, EngineAct, EngineView, APPROVE_LABEL};
        use crate::domain::chat::{Mark, Turn, Who};
        let turn = |who, text: &str, mark| Turn { who, text: text.into(), mark, at: "2026-10-28T11:00:00Z".into(), attachments: Vec::new(), choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None };
        let turns = vec![turn(Who::You, "Make a dev environment", Mark::None), turn(Who::Shibubu, "Made Dev environment: Dev, Tests", Mark::None), turn(Who::You, "Change the README title", Mark::None)];
        let step = ApprovalStep { action: "desktop.ax.type_text".into(), input: serde_json::json!({ "app_name": "TextEdit", "text": "# Shibubu Demo" }), digest: "d".into(), scopes: vec![] };
        let approve = Talk(Some((turns.clone(), Some(EngineView::Approve { goal: "Change the README title".into(), steps: vec![step], line: None }))));
        let (nodes, got) = chat_ax(&approve, Some(APPROVE_LABEL));
        for want in ["Make a dev environment", "Made Dev environment: Dev, Tests", "Change the README title"] {
            assert!(nodes.iter().any(|(_, l)| l == want), "{want}: {nodes:?}");
        }
        assert_eq!(nodes.iter().filter(|(_, l)| l == APPROVE_LABEL).count(), 1, "{nodes:?}");
        assert_eq!(got, Some(Intent::Page(super::super::shell::PageIntent::Engine(EngineAct::Approve))));
        let done = "Done. Checked the saved file";
        let mut said = turns.clone();
        said.push(turn(Who::Shibubu, done, Mark::Verified));
        let (nodes, _) = chat_ax(&Talk(Some((said, Some(EngineView::Line { state: RegionState::Verified, line: done.into() })))), None);
        assert_eq!(nodes.iter().filter(|(_, l)| l == done).count(), 1, "結果の文を 2 度描いた: {nodes:?}");
    }

    /// CHAT-ONE-1101: Chat の tab の chat bar は Home と同じ部品 = 欄 1 つ（AX の名前 `What to create`）・会話・`+`（`Attach`）・送る丸（`Send`）。
    /// Rename と ⌘K の間は欄を描かず 1 行の文（欄は header に在る = 窓に欄は 1 つ）。提案の題・About・Runs の tab は描かない。
    #[test]
    fn chat_one_the_chat_tab_holds_the_home_chat_bar() {
        use crate::domain::chat::{Mark, Turn, Who};
        let said = vec![Turn { who: Who::You, text: "Add a Tests page".into(), mark: Mark::None, at: "t".into(), attachments: Vec::new(), choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None }];
        let run = |renaming: bool| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut shell = Shell::new(View::Project { project: "p".into(), page: "x".into() });
            if renaming {
                shell.start_rename("p".into(), "Shop".into());
            }
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0));
            let mut nodes = Vec::new();
            for _ in 0..3 {
                let out = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| chat_column(ui, &mut shell, &said, None, None, &NoAgent, &[], false, false));
                });
                nodes = out.platform_output.accesskit_update.map(|u| u.nodes).unwrap_or_default();
            }
            let inputs: Vec<String> = nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::TextInput).filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
            let named = |name: &str| nodes.iter().any(|(_, n)| n.label().or(n.value()) == Some(name));
            (inputs, named("Add a Tests page"), named("Attach") && named("Send"), ["Suggestions", "About", "Runs"].iter().any(|n| named(n)))
        };
        assert_eq!(run(false), (vec!["What to create".to_owned()], true, true, false), "Chat の tab: Home と同じ欄 1 つ・会話・+ と送る丸");
        assert_eq!(run(true).0, Vec::<String>::new(), "Rename の間に Chat の欄を描いた（header の欄と 2 つ）");
    }
}
