//! G95: Project の下の panel（VS Code / Orca の terminal の帯と同じ位置）。下 = Shibubu と話す Chat の tab（L1-NORAIL）と自分の Terminal・真ん中 = 成果物の Page。
//! CHAT-SIDE-1101 追記 219〜221: 置き場（`UiSettings::chat_place`）が右なら、この panel ごと右の sidebar（`agent_rail::chat_side`）に描く（同じ tab の列・同じ pty・下には描かない）。
//! 利用者が足す Terminal は Page の面でなく、この panel の tab に並ぶ（Page を替えても同じ panel・cwd = Project の folder）。AI が plan で Page に置いた Terminal は Page の面のまま。
//! この file は状態（`TerminalPanel`・保存する）と帯の描き（tab・`+`・split・開閉・境の drag）だけを持つ。PTY と grid は持たない:
//! 描く側（`generated::Generated::draw_terminal_panel`）が tab ごとの本体を `body` で描き、押された手（`PanelAct`）を `Generated::panel_act` に渡す。
//! ui は terminal を取り込まない test crate にも入るので、`crate::domain` と `crate::ui` だけを引く。
use crate::domain::icon::Icon;
use crate::domain::terminal::{AgentState, AGENTS};
use crate::ui::theme::{self, Type, Variant};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// MIRROR-FIT-1101（owner 198 の 8「下のchatを上に引き上げると中のappを小さくするのではなくかぶるように」）: この frame で開いた panel が Page の上に
/// 被さっている矩形（egui の points）。被せていない（畳んだ・本物の app も Web の面も無い Page = 今まで通り Page を縮める）・この frame に panel を描いていない = None。
/// `Generated::draw_terminal_panel` が CentralPanel より先に毎 frame 置き、Page の側が読む: Mirror の面は被さった間 本物を Shibubu の下へ戻し、
/// WebView の面は被さった所を切って見せない（WebView の大きさは替えない = 頁を組み直さない）。
/// 例: `if terminal_panel::covering(ui.ctx()).is_some_and(|c| c.intersects(video)) { /* 本物を下へ */ }`
/// 保証する: 前の frame に置いた矩形を返さない（置いた pass の番号を見る）。
/// Wave: MIRROR-FIT-1101
pub fn covering(ctx: &egui::Context) -> Option<egui::Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, egui::Rect)>(egui::Id::new(COVER_ID))).filter(|(at, _)| *at == pass).map(|(_, r)| r)
}

const COVER_ID: &str = "kangae-panel-cover";

/// MIRROR-FIT-1101: `covering` の矩形を置く（`draw_terminal_panel` が毎 frame・被せない frame は None）。
/// Wave: MIRROR-FIT-1101
pub fn set_covering(ctx: &egui::Context, rect: Option<egui::Rect>) {
    let (id, pass) = (egui::Id::new(COVER_ID), ctx.cumulative_pass_nr());
    match rect {
        Some(r) => ctx.data_mut(|d| d.insert_temp(id, (pass, r))),
        None => ctx.data_mut(|d| d.remove::<(u64, egui::Rect)>(id)),
    }
}

/// 畳んだ帯の高さ（tab の名前と状態の点の 1 行）。UI-SWEEP-1101 追記（owner 210「orca真似して」）: Orca の terminal の帯と同じ 32（tab は帯の高さいっぱい・字 12）
pub const STRIP_H: f32 = 32.0;
/// owner 210: panel の左右の内側の余白。会話（`chat`）だけがこの内側に収まり、帯と Terminal の本体は panel の端から端まで（描く側の `Frame` の inner_margin と同じ値）
pub const INSET_X: f32 = theme::SP_2;
/// owner 210: 帯の tab の左右の内側（Orca の tab の px-1.5）と、tab の中の `×` の四方（Orca の w-4 h-4）
const TAB_PAD_X: f32 = 6.0;
const TAB_CLOSE: f32 = 16.0;
/// TITLEBAR-ICONS-1101 の 4: tab の名前の最大の幅（長い題は … で切り、hover で全文）
const TAB_NAME_MAX: f32 = 180.0;
/// TITLEBAR-ICONS-1101 の 2: tab の icon の角の状態の点の半径
const TAB_DOT: f32 = 3.0;
/// 開いた時の既定の高さ（主領域に対する割合）
pub const DEFAULT_FRACTION: f32 = 0.35;
/// 境の drag で取れる割合の範囲（TERM-FEEL-1101・owner 182 通目「一番上まで引き上げられない」: 上は主領域の全部）
pub const MIN_FRACTION: f32 = 0.15;
pub const MAX_FRACTION: f32 = 1.0;
/// 境をここ（主領域の 92%）より上まで drag すると最大（主領域の全部）に吸い付く・下げれば drag した所に戻る。Wave: TERM-FEEL-1101
pub const SNAP_FRACTION: f32 = 0.92;
/// 開いた panel の最小の高さ（帯・状態の 1 行・grid の `TERM_MIN_ROWS` 行・入力欄が入る）
pub const OPEN_MIN_H: f32 = 240.0;

/// G95: Project 1 つの panel の今。開閉・高さ・tab の名前と cwd と並びは保存する（`ui_state` の `panel.{project}` / `terminal_panel`）。
/// process は持ち越せない: 再起動の後の tab は同じ session id の新しい shell で起き、終わる前に保存した画面をその上に描く（TERM-RESTORE-1101・`generated::ensure_spec`）。
/// 新しい型の理由: Project の定義（revision・Undo の対象）に入れると開閉や並べ替えのたびに revision が増える。利用者の席の状態で、Page の木とは別。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalPanel {
    pub open: bool,
    /// 開いた時の高さ（主領域に対する割合・`MIN_FRACTION..SNAP_FRACTION`・最大の間は吸い付く前の割合を持つ）
    pub fraction: f32,
    /// 最大 = 主領域の全部（境の double-click で入る / 戻る・境を `SNAP_FRACTION` より上まで drag で入る / 下げて戻る）
    pub max: bool,
    pub tabs: Vec<PanelTab>,
    /// 選んでいる tab の id（左）
    pub active: Option<String>,
    /// 割った時の右（`down` なら下）の tab の id
    pub split: Option<String>,
    /// L7-ORCA #9: 割る向き（false = 左右・true = 上下）。grid の右 click の `Split Down` で true・⌘D と `Split Right` で false
    pub down: bool,
    /// L1-NORAIL: Chat の tab を選んでいる（false = Terminal の tab のどれか）。既定 true・`chat` の無い古い保存も true（`#[serde(default)]` は `Default` の値で埋める）
    pub chat: bool,
    /// 名前を替えている tab の id と打ちかけ（保存しない）
    #[serde(skip)]
    pub renaming: Option<(String, String)>,
    /// `Terminate "{name}"?` を出している tab の id（保存しない）
    #[serde(skip)]
    pub confirm_close: Option<String>,
    /// SESSIONS-LEFT-1101（owner 210 の 4「どの会話を見ているか」）: 今の chat session の題（`Generated::draw_terminal_panel` が毎 frame 入れる・保存しない）。
    /// Some なら帯の Chat の tab は `Chat · {題}`（長い題は切る・AX の名前は `Chat` のまま）。None = 一言の無い 1 つ目の session（`Chat` だけ）
    #[serde(skip)]
    pub chat_title: Option<String>,
    /// TERM-ONE-1101（owner 183「appにterminalを呼ぶのと下にterminalがある違いを作る意味がわからない」）: plan の Terminal（Project の `terminal_specs`）のうち
    /// tab にした id（`PanelAct::Adopt`）。閉じた tab もここに残る = 毎 frame の採用で戻さない。古い保存（欄が無い）は空 = 次の frame で全部 tab になる。
    pub planned: Vec<String>,
}

/// SESSIONS-LEFT-1101（owner 210 の 4）: 帯の Chat の tab に出す session の題の字数（それより長い題は切って `…`）
const CHAT_TITLE_MAX: usize = 24;

/// PANEL-SYNC-1101（owner 248「下にしまった時は下のおびにtabを置いてscrollできるように」）: 畳んだ panel の tab を窓の一番下の帯（`shell::status_bar`）に並べる材料。
/// 作り手 = `Generated::folded`（開いている間は None = 同じ tab を 2 か所に出さない）。押した手は `Intent::OpenBelow`（下に開く = 右の sidebar は畳まれる）。
/// 新しい型の理由: `StatusBar` は AI と使用量の欄だけで panel の tab を持つ物が無く、`TerminalPanel` ごと渡すと帯が状態の点と名前の打ちかけまで読む（帯は icon と名前だけ）。
/// 例: `Folded { chat: Some(chat_label(None)), tabs: g.term_rows(&project), agents: vec!["claude"], browser: true }`
#[derive(Debug, Clone, PartialEq)]
pub struct Folded {
    /// Chat の tab の語（`chat_label`）・None = Chat の無い panel（Home の Terminal）
    pub chat: Option<String>,
    /// (tab の id, 名前, 前面の agent) = `Generated::term_rows` と同じ並び
    pub tabs: Vec<(String, String, Option<&'static crate::domain::terminal::AgentKind>)>,
    /// 帯の ＋ の menu に出す入っている agent（panel の帯の ＋ と同じ）
    pub agents: Vec<&'static str>,
    /// ＋ の menu に `New browser tab` を出すか（Project の panel だけ）
    pub browser: bool,
}

/// SESSIONS-LEFT-1101（owner 210 の 4）・PANEL-SYNC-1101: Chat の tab の語（今の session の題が在れば `Chat · {題}`・長い題は `CHAT_TITLE_MAX` で切る）。panel の帯と窓の一番下の帯が同じ語を使う。
pub fn chat_label(title: Option<&str>) -> String {
    title.map_or_else(
        || "Chat".to_owned(),
        |t| {
            let short: String = t.chars().take(CHAT_TITLE_MAX).collect();
            format!("Chat · {short}{}", if t.chars().count() > CHAT_TITLE_MAX { "…" } else { "" })
        },
    )
}

/// G95: panel の tab 1 つ。`id` = `TerminalSessionSpec::id`（= session id・再起動をまたいで同じ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelTab {
    pub id: String,
    pub name: String,
    pub cwd: String,
    /// L7-ORCA #12: 起こした shell に最初に打つ agent の名前（`AGENTS` の 1 つ・`+` の menu の agent の行）。None = shell だけ（保存の形は今まで通り）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// TITLEBAR-ICONS-1101 の 4: 利用者が付けた名前（`Rename`）。true の間は自動の名前（`Retitle`）で替えない（前の版の保存 = false = 自動）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub named: bool,
}

/// G95: 帯で押された手と鍵（`PageIntent::TerminalPanel` で app.rs から・帯から直に `Generated::panel_act` へ）。
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAct {
    /// L1-NORAIL: 帯の `Chat` の tab・⇧⌘P・新しい card: Chat を選んで開く（Terminal は起こさない）
    Chat,
    /// ⌃` と帯の左端の開閉（`Show chat` / `Hide chat`・`Show terminal` / `Hide terminal`）: 選んでいる tab の開閉（Terminal を選んでいて tab が無ければ 1 つ足して開く）
    Toggle,
    /// `•••` の `Show terminal`（Page に Terminal が無い時）: 開く（tab が無ければ 1 つ足す）
    Open,
    /// Page の `＋` の `Add terminal`・帯の `+` の `New terminal`・⌃⇧`: 新しい tab を足して開く
    Add,
    /// TERM-ONE-1101: plan の Terminal（`terminal_specs` の 1 つ・`id` = spec の id = session id）を tab にする（`Generated::draw_terminal_panel` が毎 frame・
    /// `planned` に無い spec だけ）。名前は `{name} 1`・cwd は `fresh` の cwd（今の Project の folder）。開閉と選んでいる tab は変えない（focus を奪わない）・
    /// 選んでいる tab が無ければこれを選ぶ。1 回だけ: `planned` に在る id は何もしない（閉じた tab を戻さない）。Page には Terminal を描かない。
    /// 例: `panel.apply(PanelAct::Adopt { id: spec.id.clone(), name: "Dev".into() }, || (String::new(), terminal_cwd(&p)), busy)` → `Some(Spawn(tab))` が 1 回。
    Adopt { id: String, name: String },
    /// L7-ORCA #12: 帯の `+` の agent の行: 新しい tab で shell を起こし、その agent を打つ（`AGENTS` に無い名前は何もしない = 外から来た文字を shell に打たない）
    AddAgent { agent: String },
    /// SIGNIN-1101（owner 193「認証できる画面まで自動で飛ばすように」）: Provider / Connections の「sign in first」の AI。新しい tab（名前 = `name {n}`・`Kimi Code 1`）で
    /// shell を起こし、`command`（app.rs が `llm_cli::FOUND_CLIS` から引いた sign in の command・`kimi login` 等）を打って開く。表の外の command を打たない守りは
    /// `generated::panel_spec`（`llm_cli::sign_in_command` が表に無い物を None にする・ui は llm_cli を引けない = tests/ui_portal.rs が `#[path]` で ui を取り込む）。
    /// 終わりを見て切り替えるのは app.rs（`Generated::probe_tab`）
    SignIn { name: String, command: String },
    /// L7-ORCA #12: 帯の `+` の `New browser tab`: 今の Page に Browser を置く。panel は何もしない（`Generated::draw_terminal_panel` が `PageIntent::AddBrowser` に替える）
    NewBrowserTab,
    Select(String),
    /// ⌘1〜⌘9（0 始まり）
    SelectNth(usize),
    Rename { tab: String, name: String },
    /// TITLEBAR-ICONS-1101 の 4: tab の自動の名前（`domain::terminal::tab_title`・`Generated::panel_status` が題か前面の process が替わった時だけ）。
    /// 利用者が付けた名前（`named`）の tab と空の名前は替えない。保存は他の手と同じ（再起動で戻る = TERM-RESTORE と同じ置き場）
    Retitle { tab: String, name: String },
    /// tab の `×` と右 click の `Terminate`: 子が動いていれば `Terminate "{name}"?` を出すだけ・動いていなければ止めて外す（隠すのは `Toggle`）
    Close { tab: String },
    /// 確認の `Terminate`
    ConfirmClose { tab: String },
    /// 確認の `Keep`
    KeepOpen,
    /// drag で並べ替え（`to` = 移した後の位置）
    Move { tab: String, to: usize },
    /// ⌘D: 左右に割る / 戻す（tab が 1 つなら 2 つ目を足して右に）
    Split,
    /// L7-ORCA #9: grid の右 click の `Split Right` / `Split Down`: その tab を左（上）に、新しい Terminal を右（下）に割る（Orca の Split と同じく新しく起こす）
    SplitRight { tab: String },
    SplitDown { tab: String },
    /// 境の drag の終わり（`SNAP_FRACTION` 以上 = 最大に吸い付く・`TerminalPanel::drag_to`）
    Resize { fraction: f32 },
    /// 境の double-click
    ToggleMax,
    /// L1-NORAIL R1（独立 review 9e63f032）: 新しい card（承認・決める・同意・Ask・結果）が出た = 開くだけ。選んでいる tab はそのまま（Terminal の focus を落とさない）・
    /// Terminal の入力欄へ focus も送らない（欄の focus も奪わない）。CHAT-ONE-1101 から card は Chat の tab の会話の中だけ（Terminal の tab の下に出さない）。
    // ponytail: Terminal の tab を見ている間に出た card は Chat の tab を開くまで見えない。要るなら帯の `Chat` に印（未読の点）を
    Reveal,
}

/// G95: `TerminalPanel::apply` の後に PTY の持ち主（`Generated`）がする事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEffect {
    /// 足した tab の shell を起こす
    Spawn(PanelTab),
    /// 閉じた tab の shell を止める（`TerminalManager::close`）
    Stop(String),
}

/// G95: 帯が描く tab 1 つの今（`Generated` が manager と `AgentWatch` から毎 frame 作る）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabStatus {
    pub agent: Option<AgentState>,
    /// TITLEBAR-ICONS-1101 の 2: 今の前面の agent（`AGENT_KINDS` の行・shell に戻れば None = Terminal の icon）。`agent` の Done は次の agent まで残るが、icon はこちらで替える
    pub kind: Option<&'static crate::domain::terminal::AgentKind>,
    /// shell が動いている（false = 終わった・前の起動の tab）
    pub running: bool,
}

impl Default for TerminalPanel {
    fn default() -> Self {
        TerminalPanel { open: false, fraction: DEFAULT_FRACTION, max: false, tabs: Vec::new(), active: None, split: None, renaming: None, confirm_close: None, chat_title: None, down: false, chat: true, planned: Vec::new() }
    }
}

impl TerminalPanel {
    /// 主領域の高さ `main_h` のうち panel が取る高さ。畳む = `STRIP_H`（CHAT-ONE-1101: 底の欄の行は無い・打つのは Chat の tab の chat bar）・開く = `main_h × fraction`
    /// （`OPEN_MIN_H` 以上）・最大 = `main_h` = 主領域の全部（TERM-FEEL-1101）。どれも `main_h` を超えず `STRIP_H` 以上。
    pub fn height(&self, main_h: f32) -> f32 {
        let h = match (self.open, self.max) {
            (false, _) => return STRIP_H,
            (true, true) => main_h,
            (true, false) => (main_h * self.fraction).max(OPEN_MIN_H).min(main_h),
        };
        h.max(STRIP_H)
    }

    /// 境の drag の割合を当てる（drag の間の `draw` と、離した時の `Resize` の 1 本の規則）。`MIN_FRACTION..=MAX_FRACTION` に丸め、
    /// `SNAP_FRACTION` 以上なら最大に吸い付く（`fraction` は吸い付く前のまま = double-click で戻る高さ）・それより下は drag した所。
    fn drag_to(&mut self, fraction: f32) {
        let f = fraction.clamp(MIN_FRACTION, MAX_FRACTION);
        self.max = f >= SNAP_FRACTION;
        if !self.max {
            self.fraction = f;
        }
    }

    /// 手を 1 つ当てる。`fresh` = 新しい tab の (id, cwd)（足す時だけ呼ぶ）・`busy` = その tab の前面に shell 以外の job が居るか（閉じる確認）。
    /// 例: `let effect = panel.apply(act, || (storage::new_id(), cwd.clone()), |id| busy(id));`
    pub fn apply(&mut self, act: PanelAct, fresh: impl FnOnce() -> (String, String), busy: impl Fn(&str) -> bool) -> Option<PanelEffect> {
        match act {
            PanelAct::Chat => (self.chat, self.open) = (true, true),
            PanelAct::Toggle if !self.chat && self.tabs.is_empty() => return self.apply(PanelAct::Add, fresh, busy),
            PanelAct::Toggle => self.open = !self.open,
            PanelAct::Open if self.tabs.is_empty() => return self.apply(PanelAct::Add, fresh, busy),
            PanelAct::Open => (self.chat, self.open) = (false, true),
            PanelAct::Add => return Some(self.add(fresh(), "Terminal", None)),
            PanelAct::Adopt { id, name } => {
                if self.planned.contains(&id) {
                    return None;
                }
                self.planned.push(id.clone());
                if self.index(&id).is_none() {
                    let tab = self.push((id, fresh().1), &name, None);
                    self.active.get_or_insert_with(|| tab.id.clone());
                    return Some(PanelEffect::Spawn(tab));
                }
            }
            PanelAct::AddAgent { agent } => {
                if let Some(agent) = AGENTS.into_iter().find(|a| *a == agent) {
                    return Some(self.add(fresh(), agent, Some(agent)));
                }
            }
            PanelAct::SignIn { name, command } => return Some(self.add(fresh(), &name, Some(&command))),
            PanelAct::NewBrowserTab => {}
            PanelAct::Select(id) => {
                if self.index(&id).is_some() {
                    self.active = Some(id);
                    (self.chat, self.open) = (false, true);
                }
            }
            PanelAct::SelectNth(n) => {
                if let Some(t) = self.tabs.get(n) {
                    self.active = Some(t.id.clone());
                    (self.chat, self.open) = (false, true);
                }
            }
            PanelAct::Rename { tab, name } => {
                let name = name.trim();
                if let (Some(i), false) = (self.index(&tab), name.is_empty()) {
                    self.tabs[i].name = name.to_owned();
                    self.tabs[i].named = true;
                }
                self.renaming = None;
            }
            PanelAct::Retitle { tab, name } => {
                if let (Some(i), false) = (self.index(&tab), name.trim().is_empty()) {
                    if !self.tabs[i].named {
                        self.tabs[i].name = name;
                    }
                }
            }
            PanelAct::Close { tab } if busy(&tab) => self.confirm_close = Some(tab),
            PanelAct::Close { tab } | PanelAct::ConfirmClose { tab } => {
                self.confirm_close = None;
                return self.remove(&tab).then_some(PanelEffect::Stop(tab));
            }
            PanelAct::KeepOpen => self.confirm_close = None,
            PanelAct::Move { tab, to } => {
                if let Some(i) = self.index(&tab) {
                    let t = self.tabs.remove(i);
                    self.tabs.insert(to.min(self.tabs.len()), t);
                }
            }
            PanelAct::Split if self.split.is_some() => self.split = None,
            PanelAct::Split => {
                let left = self.active_tab().map(|t| t.id.clone());
                (self.chat, self.open) = (false, true);
                self.down = false;
                let right = self.tabs.iter().find(|t| Some(&t.id) != left.as_ref()).map(|t| t.id.clone());
                match right {
                    Some(right) => self.split = Some(right),
                    None => {
                        let tab = self.push(fresh(), "Terminal", None);
                        self.active = left.or_else(|| Some(tab.id.clone()));
                        self.split = Some(tab.id.clone());
                        return Some(PanelEffect::Spawn(tab));
                    }
                }
            }
            PanelAct::Resize { fraction } => self.drag_to(fraction),
            PanelAct::ToggleMax => {
                self.max = !self.max;
                self.open = true;
            }
            PanelAct::Reveal => self.open = true,
            PanelAct::SplitRight { tab } | PanelAct::SplitDown { tab } if self.index(&tab).is_none() => {}
            PanelAct::SplitRight { tab } => return Some(self.split_new(tab, false, fresh())),
            PanelAct::SplitDown { tab } => return Some(self.split_new(tab, true, fresh())),
        }
        None
    }

    /// 選んでいる tab（無ければ先頭）
    pub fn active_tab(&self) -> Option<&PanelTab> {
        self.active.as_deref().and_then(|id| self.tabs.iter().find(|t| t.id == id)).or(self.tabs.first())
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// `tab` を左（上）に、新しい tab を右（下）に置いて開く
    fn split_new(&mut self, tab: String, down: bool, fresh: (String, String)) -> PanelEffect {
        let new = self.push(fresh, "Terminal", None);
        (self.active, self.split, self.down, self.open, self.chat) = (Some(tab), Some(new.id.clone()), down, true, false);
        PanelEffect::Spawn(new)
    }

    /// 足して選び、開く（Chat から Terminal の tab に移る）
    fn add(&mut self, fresh: (String, String), base: &str, command: Option<&str>) -> PanelEffect {
        let tab = self.push(fresh, base, command);
        self.active = Some(tab.id.clone());
        (self.chat, self.open) = (false, true);
        PanelEffect::Spawn(tab)
    }

    /// 末尾に足す（名前は使っていない一番小さい `{base} {n}` = `Terminal 1`・`claude 1`）
    fn push(&mut self, (id, cwd): (String, String), base: &str, command: Option<&str>) -> PanelTab {
        let n = (1..).find(|n| !self.tabs.iter().any(|t| t.name == format!("{base} {n}"))).unwrap_or(1);
        let tab = PanelTab { id, name: format!("{base} {n}"), cwd, command: command.map(str::to_owned), named: false };
        self.tabs.push(tab.clone());
        tab
    }

    /// 外す: 選んでいた tab なら隣（前、無ければ次）を選ぶ・右に割っていた tab なら戻す・最後の 1 つなら Chat に戻る（開閉はそのまま・L1-NORAIL）。無い id は false。
    fn remove(&mut self, id: &str) -> bool {
        let Some(i) = self.index(id) else { return false };
        self.tabs.remove(i);
        if self.split.as_deref() == Some(id) {
            self.split = None;
        }
        if self.active.as_deref() == Some(id) {
            self.active = self.tabs.get(i.saturating_sub(1)).map(|t| t.id.clone());
        }
        if self.split.is_some() && self.split == self.active {
            self.split = None;
        }
        if self.tabs.is_empty() {
            self.chat = true;
            self.active = None;
        }
        true
    }
}

/// panel の中（帯 + 開いていれば本体）を描く。`main_h` = 主領域の高さ（境の drag の割合）・None = 右の sidebar の中（CHAT-SIDE-1101 追記 219〜221・上端の高さの境は無い）・`status` = tab の id → 今・
/// `agents` = 帯の `+` の menu に出す入っている agent（`llm_cli::agent_clis`）・`browser` = 帯の `+` に `New browser tab` を出すか（HOME-TERM-1101: Home の Terminal は
/// Page が無い = false・押して何も起きない項目を置かない）・`body` = tab 1 つの本体（grid と入力欄）・
/// `chat` = Chat の tab（CHAT-ONE-1101: 会話と chat bar・Chat を選んで開いている時だけ呼ぶ）。None = Chat の無い panel（HOME-TERM-1101 の Home の Terminal）
/// （帯に `Chat` の tab・`+` の `Chat with Shibubu` を出さず、`panel.chat` は Terminal の席として読む）。畳んでいる時は帯だけ（PANEL-SYNC-1101 から製品は畳んだ panel を
/// 呼ばない = 呼ぶ側 `Generated::draw_terminal_panel` が描かず、tab は窓の一番下の帯の `folded_tabs`。畳んだ帯の形は試験の口として残す）・Terminal の tab で開いている時は
/// grid が panel の下端まで（owner 184「terminalとしたのchatの間の空間がいらない」= 底の欄・card・その間の空白を描かない）。
/// 押された手を返す（名前替えの打ちかけ・確認の開閉はここで `panel` に書く）。
/// 例（`Generated::draw_terminal_panel`）: `terminal_panel::draw(ui, panel, Some(main_h), &status, &agents, true, &mut |ui, tab| grid(ui, tab), Some(&mut |ui| column(ui)))`
pub fn draw(
    ui: &mut egui::Ui,
    panel: &mut TerminalPanel,
    main_h: Option<f32>,
    status: &BTreeMap<String, TabStatus>,
    agents: &[&str],
    browser: bool,
    body: &mut dyn FnMut(&mut egui::Ui, &PanelTab),
    chat: Option<&mut dyn FnMut(&mut egui::Ui)>,
) -> Option<PanelAct> {
    let mut act = None;
    let whole = ui.max_rect();
    // Chat の無い panel（Home の Terminal）は Chat の席を Terminal の席として読む
    let here = chat.is_some();
    if !here {
        panel.chat = false;
    }
    // 境（panel の上端の 5px・開いている時だけ・右の sidebar には無い）: drag で高さ（離した時に保存・上端の近くで最大に吸い付く）・double-click で最大 / 戻す
    if let (true, Some(main_h)) = (panel.open, main_h) {
        let edge = egui::Rect::from_min_max(whole.left_top(), egui::pos2(whole.right(), whole.top() + 5.0));
        let r = ui.interact(edge, ui.id().with("terminal-panel-edge"), egui::Sense::click_and_drag());
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if r.double_clicked() {
            act = Some(PanelAct::ToggleMax);
        } else if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                panel.drag_to((whole.bottom() - p.y) / main_h.max(1.0));
            }
        } else if r.drag_stopped() {
            act = Some(PanelAct::Resize { fraction: if panel.max { MAX_FRACTION } else { panel.fraction } });
        }
    }
    // 帯: 左端に開閉（小さな icon 1 つ・追記 215・畳んでも開いても同じ所）・常設の `Chat` の tab（L1-NORAIL・× 無し）・Terminal の tab（状態の記号と名前・× は tab の中）・tab の右に `+`（menu）・右端に split
    // （TERM-ONE-1101: `•••` の `Input field (for IME)` は消した = terminal ごとの欄は無い・変換中の字は grid の cursor の所に出る）
    // （L7-ORCA #7・owner 10-31「show terminal の場所が見つけづらい」: 右端の ▴ の icon だけでは見つからない）
    // UI-SWEEP-1101 追記（owner 210「terminalとかの枠マジでいらない。ここら辺とかorca真似して」）: tab は枠なし（`strip_tab`）・`+` は文字なしの icon 1 つ・
    // 帯は panel の端から端まで（`INSET_X` の外まで）で、下に 1px の `border`（Orca の帯の下の線）
    let gap = ui.spacing().item_spacing.x;
    let n = if panel.tabs.is_empty() { 0.0 } else { 1.0 };
    let group_w = n * (theme::BUTTON_H + gap) + TAB_PAD_X;
    let band = egui::Rect::from_min_size(egui::pos2(whole.left() - INSET_X, ui.cursor().top()), egui::vec2(whole.width() + 2.0 * INSET_X, STRIP_H));
    let mut strip = ui.new_child(egui::UiBuilder::new().id_salt("panel-strip").max_rect(band).layout(egui::Layout::left_to_right(egui::Align::Center)));
    strip.set_clip_rect(band.intersect(ui.clip_rect()));
    {
        let ui = &mut strip;
        // 選んでいる tab の開閉（Chat を選んでいる間は `Show chat` / `Hide chat`）
        let name = match (panel.chat, panel.open) {
            (true, true) => "Hide chat",
            (true, false) => "Show chat",
            (false, true) => "Hide terminal",
            (false, false) => "Show terminal",
        };
        // UI-SWEEP-1101 追記 215（owner「showterminalとhideterminalを小さく。文字ではなくアイコンで」）: 小さな icon 1 つ（下の panel の絵・開いている間は ⌄ = 畳む・
        // 畳んでいる間は ⌃ = 開く）。AX の名前は今の語のまま・hover は名前と鍵
        // PANEL-SYNC-1101（owner 248「右sidebarの下矢印いらない」）: 開閉の icon は下の panel だけ（右の sidebar は ◨ で閉じる・置き場の切り替えは ◨ と窓の一番下の帯の tab）
        if main_h.is_some() {
            let hint = if !panel.chat && panel.open && !panel.tabs.is_empty() { format!("{name} · shells keep running · ⌃`") } else { format!("{name} · ⌃`") };
            let toggle = theme::ax_name(theme::icon_button(ui, if panel.open { Icon::PanelBottomClose } else { Icon::PanelBottomOpen }, &hint), name);
            if toggle.clicked() {
                act = Some(PanelAct::Toggle);
            }
        }
        let (_, room) = ui.allocate_space(egui::vec2((ui.available_width() - group_w - gap).max(0.0), STRIP_H));
        // tab の並びと、その右の `+`（tab が溢れても `+` は room の右端に残る = その分を tab の並びから引く）。
        // UI-SWEEP-1101 追記 215（owner「orcaの上みたいにterminal, chatのtabを横にscrollでもしたくさんtab増えても移動できるように」）: 並びは横に scroll する
        // （trackpad の横・shift＋wheel = egui が横に替える・scroll bar は出さない・tab の drag は並べ替えのまま）。選んだ tab・tab の数・Chat の席が替わった frame だけ
        // 選んだ tab が見える所まで寄せ（毎 frame 寄せると手で動かせない）、溢れている側に薄い影
        let tabs_room = egui::Rect::from_min_max(room.min, egui::pos2((room.right() - theme::BUTTON_H - gap).max(room.left()), room.bottom()));
        let mut row = ui.new_child(egui::UiBuilder::new().id_salt("panel-tabs-row").max_rect(tabs_room).layout(egui::Layout::left_to_right(egui::Align::Center)));
        row.set_clip_rect(tabs_room.intersect(ui.clip_rect()));
        let seen_id = ui.id().with("panel-tabs-seen");
        let now = (panel.chat, panel.active.clone(), panel.tabs.len());
        let reveal = ui.ctx().data(|d| d.get_temp::<(bool, Option<String>, usize)>(seen_id)).as_ref() != Some(&now);
        ui.ctx().data_mut(|d| d.insert_temp(seen_id, now));
        let scrolled = egui::ScrollArea::horizontal()
            .id_salt("panel-tabs-scroll")
            .max_width(tabs_room.width())
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .drag_to_scroll(false)
            .show(&mut row, |row| {
                row.horizontal(|row| {
                    row.spacing_mut().item_spacing.x = 0.0; // tab は隙間なく並ぶ（Orca）
                    let mut act = None;
                    if here {
                        // SESSIONS-LEFT-1101（owner 210 の 4）: 今の session の題を tab に（長い題は切る・AX の名前は Chat のまま）
                        let label = chat_label(panel.chat_title.as_deref());
                        let (chat_tab, _) = strip_tab(row, &label, panel.chat, None, false, STRIP_H);
                        if reveal && panel.chat {
                            chat_tab.scroll_to_me(None);
                        }
                        if theme::ax_name(chat_tab, "Chat").on_hover_text("Talk to Shibubu · ⇧⌘P").clicked() {
                            act = Some(PanelAct::Chat);
                        }
                    }
                    tab_row(row, panel, status, reveal).or(act)
                })
                .inner
            });
        let seen = scrolled.inner_rect;
        let (offset, full) = (scrolled.state.offset.x, scrolled.content_size.x);
        edge_shadows(ui, seen, offset > 0.5, offset + seen.width() < full - 0.5);
        if let Some(a) = scrolled.inner {
            act = Some(a);
        }
        let plus_at = egui::pos2(seen.right().min(tabs_room.right()) + gap, room.center().y - theme::BUTTON_H / 2.0);
        // L7-ORCA #12: 新しい Terminal・入っている agent（無ければ行を出さない）・今の Page に Browser・常設の Chat の tab を選ぶ（L1-NORAIL）
        let new = ui.put(egui::Rect::from_min_size(plus_at, egui::Vec2::splat(theme::BUTTON_H)), |ui: &mut egui::Ui| theme::icon_button(ui, Icon::Plus, "New terminal, agent, or browser · ⌃⇧` for a terminal"));
        let new = theme::ax_name(new, "New tab");
        let picked = strip_menu(ui, &new, |ui| new_menu(ui, agents, browser, here));
        // `put` は横の並びの cursor を `+` の後ろへ戻すので、右端の icon は room の右から置き直す
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(egui::pos2(room.right() + gap, band.top()), egui::pos2(band.right() - TAB_PAD_X, band.bottom()))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if !panel.tabs.is_empty() && theme::icon_button(&mut right, Icon::Split, "Split · ⌘D").clicked() {
            act = Some(PanelAct::Split);
        }
        if picked.is_some() {
            act = picked;
        }
    }
    if panel.open && !panel.chat {
        let line = egui::Rect::from_min_max(egui::pos2(band.left(), band.bottom() - theme::HAIRLINE), band.right_bottom());
        ui.painter().with_clip_rect(band.intersect(ui.clip_rect())).rect_filled(line, egui::CornerRadius::ZERO, theme::tokens(ui.ctx()).border);
    }
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(egui::pos2(whole.left(), band.top()), egui::pos2(whole.right(), band.bottom())));
    // CHAT-ONE-1101: 畳んでいる = 帯だけ・Chat を選んで開いている = 会話と chat bar（grid は描かない）
    if !panel.open {
        return act;
    }
    if panel.chat {
        if let Some(chat) = chat {
            chat(ui);
        }
        return act;
    }
    // Terminal の tab で開いている = grid が panel の下端まで（欄・card・間の空白は無い）。
    // owner 210: Terminal の本体は panel の端から端まで（会話の `INSET_X` の外まで・枠も角の丸みも無い = `components::terminal_grid` の panel の腕）
    let all = ui.available_rect_before_wrap();
    let room = egui::Rect::from_min_max(egui::pos2(all.left() - INSET_X, all.top()), egui::pos2(all.right() + INSET_X, all.bottom()));
    let mut grid = ui.new_child(egui::UiBuilder::new().id_salt("panel-tabs").max_rect(room).layout(egui::Layout::top_down(egui::Align::Min)));
    grid.set_clip_rect(room.intersect(ui.clip_rect()));
    tabs_body(&mut grid, panel, body);
    ui.advance_cursor_after_rect(all);
    act
}

/// L7-ORCA #12・PANEL-SYNC-1101: 帯の `+` の menu の中身（panel の帯と窓の一番下の帯が同じ）= 新しい Terminal・入っている agent（無ければ行を出さない）・
/// 今の Page に Browser（`browser`）・常設の Chat の tab を選ぶ（`chat`・L1-NORAIL）。
/// PROVIDER-ICONS-1101: agent の行 = その印と表の名前（`AgentKind::label` = `Codex`・AX の名前も同じ・押した手は cmd のまま）。
fn new_menu(ui: &mut egui::Ui, agents: &[&str], browser: bool, chat: bool) -> Option<PanelAct> {
    let mut picked = None;
    if theme::button_menu_item(ui, "New terminal", "⌃⇧`", Variant::Ghost).clicked() {
        picked = Some(PanelAct::Add);
    }
    for agent in agents {
        let label = crate::domain::terminal::agent_kind(agent).map_or(*agent, |k| k.label);
        if theme::provider_menu_item(ui, agent, label, "Agent").clicked() {
            picked = Some(PanelAct::AddAgent { agent: (*agent).to_owned() });
        }
    }
    if browser && theme::button_menu_item(ui, "New browser tab", "On this page", Variant::Ghost).clicked() {
        picked = Some(PanelAct::NewBrowserTab);
    }
    if chat && theme::button_menu_item(ui, "Chat with Shibubu", "⇧⌘P", Variant::Ghost).clicked() {
        picked = Some(PanelAct::Chat);
    }
    picked
}

/// PANEL-SYNC-1101: 窓の一番下の帯の畳んだ tab の並びの幅の上限（帯の残りの幅に対する割合・右の使用量の席を残す）。
const FOLDED_MAX: f32 = 0.5;

/// PANEL-SYNC-1101（owner 248「一番したのおびにわかりやすく入れておくように下にしまった時は下のおびにtabを置いてscrollできるように」）: 畳んだ panel の tab を
/// 窓の一番下の帯（`shell::status_bar`）の左寄りに描く。Chat（在れば・吹き出しの icon）と各 Terminal（前面の agent の icon か >_）= icon と名前・枠なし・帯の高さ `STATUS_H`
/// （panel の帯の tab と同じ `strip_tab`）。並びの幅は `FOLDED_MAX` まで・溢れたら横に scroll（scroll bar なし・溢れた側に薄い影）・最後に `+`（panel の帯と同じ menu・AX `New tab`）。
/// 状態の点は描かない（帯は icon と名前だけ・状態は開いた panel の tab と通知）。返す = 押した手（呼ぶ側が `Intent::OpenBelow` に包む = 下に開く）。
/// 例（`shell::status_bar`）: `if let Some(a) = bar.folded.as_ref().and_then(|f| terminal_panel::folded_tabs(ui, f)) { intent = Some(Intent::OpenBelow(a)); }`
pub fn folded_tabs(ui: &mut egui::Ui, f: &Folded) -> Option<PanelAct> {
    let t = theme::tokens(ui.ctx());
    let h = theme::STATUS_H;
    let room = (ui.available_width() * FOLDED_MAX - h).max(0.0);
    let scrolled = egui::ScrollArea::horizontal()
        .id_salt("status-folded-tabs")
        .max_width(room)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .drag_to_scroll(false)
        .show(ui, |row| {
            row.horizontal(|row| {
                row.spacing_mut().item_spacing.x = 0.0;
                let mut act = None;
                if let Some(label) = &f.chat {
                    let (r, _) = strip_tab(row, label, false, None, true, h);
                    let ink = if r.hovered() { t.text_primary } else { t.text_secondary };
                    theme::kind_icon(row, egui::pos2(r.rect.left() + TAB_PAD_X + theme::ICON_UI / 2.0, r.rect.center().y), None, None, Some(Icon::MessageSquare), "Chat", ink);
                    if theme::ax_name(r, "Chat").on_hover_text("Open the chat below").clicked() {
                        act = Some(PanelAct::Chat);
                    }
                }
                for (id, name, kind) in &f.tabs {
                    let (r, _) = strip_tab(row, name, false, None, true, h);
                    tab_icon(row, &r, false, Some(&TabStatus { agent: None, kind: *kind, running: true }));
                    if r.on_hover_text(format!("{name} · Open below")).clicked() {
                        act = Some(PanelAct::Select(id.clone()));
                    }
                }
                act
            })
            .inner
        });
    let seen = scrolled.inner_rect;
    let (offset, full) = (scrolled.state.offset.x, scrolled.content_size.x);
    edge_shadows(ui, seen, offset > 0.5, offset + seen.width() < full - 0.5);
    let mut act = scrolled.inner;
    // `+`（帯の高さの四方・icon_button の 28 は帯〈24〉に入らない = 同じ塗りの決まりで帯の高さに描く）
    let (rect, new) = ui.allocate_exact_size(egui::vec2(h, h), egui::Sense::click());
    new.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New tab"));
    if ui.is_rect_visible(rect) {
        if new.hovered() {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_SM), t.surface_3);
        }
        theme::focus_ring(ui, &new, rect, egui::CornerRadius::same(theme::RADIUS_SM));
        theme::paint_icon(ui.painter(), Icon::Plus, rect.center(), theme::ICON_UI, if new.hovered() { t.text_primary } else { t.text_secondary }, 0.0);
    }
    let new = new.on_hover_text("New terminal, agent, or browser · ⌃⇧` for a terminal");
    if let Some(a) = strip_menu(ui, &new, |ui| new_menu(ui, &f.agents, f.browser, f.chat.is_some())) {
        act = Some(a);
    }
    ui.add_space(theme::SP_2);
    act
}

/// 本体: 選んだ tab（割っていれば左右か上下に 2 つ）。body は渡した ui の max_rect の下端までに grid を収める
fn tabs_body(ui: &mut egui::Ui, panel: &TerminalPanel, body: &mut dyn FnMut(&mut egui::Ui, &PanelTab)) {
    let left = panel.active_tab().cloned();
    let right = panel.split.as_ref().and_then(|id| panel.tabs.iter().find(|t| &t.id == id)).filter(|r| left.as_ref().is_none_or(|l| l.id != r.id)).cloned();
    match (left, right) {
        (Some(l), Some(r)) if panel.down => {
            let all = ui.available_rect_before_wrap();
            let gap = ui.spacing().item_spacing.y;
            let h = ((all.height() - gap) / 2.0).max(0.0);
            let top = egui::Rect::from_min_size(all.min, egui::vec2(all.width(), h));
            let under = egui::Rect::from_min_size(egui::pos2(all.left(), top.bottom() + gap), egui::vec2(all.width(), h));
            // 半分ずつ（境の drag は無い）。窓が低くて grid の床（`TERM_MIN_ROWS`）が半分に入らない時も、上の grid が下に重ならないよう枠で切る
            for (half, tab) in [(top, &l), (under, &r)] {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(half).layout(egui::Layout::top_down(egui::Align::Min)));
                child.set_clip_rect(half.intersect(ui.clip_rect()));
                body(&mut child, tab);
            }
            ui.advance_cursor_after_rect(all);
        }
        (Some(l), Some(r)) => {
            // ponytail: 左右は半分ずつ（境の drag は無い）。要れば split の割合を TerminalPanel に足す
            ui.columns(2, |cols| {
                body(&mut cols[0], &l);
                body(&mut cols[1], &r);
            });
        }
        (Some(l), None) => body(ui, &l),
        _ => {}
    }
}

/// 帯の button（`+`）の menu（窓の下半分の button = 下の panel の帯は上に・上半分 = 右の sidebar の帯〈CHAT-SIDE-1101 追記 220〉は下に開く）。`opener` を押すたびに開閉し、
/// Esc・外の click・選んだ時に閉じる。幅 240 の rect を、窓の右半分の button は `opener` の右端に・左半分の button（tab の右の `+`・owner 210）は左端に揃えて渡す（窓の外へはみ出さない）。`Memory::popup` = WebView を隠す規則に入る。
/// STATUSBAR-1101: 窓の下の帯（`shell::status_bar` の AI の menu）も同じ口で開く。
pub(super) fn strip_menu<R>(ui: &mut egui::Ui, opener: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui) -> Option<R>) -> Option<R> {
    let menu = opener.id.with("strip-menu");
    if opener.clicked() {
        ui.memory_mut(|m| m.toggle_popup(menu));
    }
    let mut anchor = opener.clone();
    let left = if opener.rect.center().x < ui.ctx().screen_rect().center().x { opener.rect.left() } else { opener.rect.right() - 240.0 };
    anchor.rect = egui::Rect::from_min_size(egui::pos2(left, opener.rect.top()), egui::vec2(240.0, opener.rect.height()));
    let side = if opener.rect.center().y > ui.ctx().screen_rect().center().y { egui::AboveOrBelow::Above } else { egui::AboveOrBelow::Below };
    let picked = egui::popup::popup_above_or_below_widget(ui, menu, &anchor, side, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        add_contents(ui)
    })
    .flatten();
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

/// UI-SWEEP-1101 追記 215: tab の並びが溢れて隠れている側に薄い影（`seen` = 見えている並び・幅 12 で `scrim` から透明へ）。Orca の上の tab の列と同じ合図。
fn edge_shadows(ui: &egui::Ui, seen: egui::Rect, left: bool, right: bool) {
    let dark = theme::tokens(ui.ctx()).scrim.gamma_multiply(0.35);
    let w = 12.0_f32.min(seen.width() / 2.0);
    let mut mesh = egui::Mesh::default();
    for (_, outer, inner) in [(left, seen.left(), seen.left() + w), (right, seen.right(), seen.right() - w)].into_iter().filter(|(on, ..)| *on) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(outer, seen.top()), dark);
        mesh.colored_vertex(egui::pos2(outer, seen.bottom()), dark);
        mesh.colored_vertex(egui::pos2(inner, seen.top()), egui::Color32::TRANSPARENT);
        mesh.colored_vertex(egui::pos2(inner, seen.bottom()), egui::Color32::TRANSPARENT);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 2, i + 3);
    }
    if !mesh.is_empty() {
        ui.painter().with_clip_rect(seen).add(mesh);
    }
}

/// UI-SWEEP-1101 追記（owner 210「terminalとかの枠マジでいらない。ここら辺とかorca真似して」）: 帯の tab 1 つ = Orca の terminal の帯の tab と同じ形。
/// 枠なし・帯の高さいっぱい（`STRIP_H`）・字 12（`Type::Helper`）・左右 `TAB_PAD_X`。`selected` = 選んでいる（AX に選びの状態・追記 215 で開閉は icon になり文字の button の腕は消した）。
/// 選んだ tab = 面 `surface_2`（panel の `surface_1` より 1 段）＋ 字 `text_primary` ＋ 下に `TAB_INDICATOR` の線・他 = 字 `text_secondary`（tab の上に pointer が在る間は `text_primary`）。
/// `close` = Some(AX 名) の時だけ tab の中の右に `×`（`TAB_CLOSE` 四方・選んだ tab と pointer の在る tab だけ描く・押せる範囲は帯の高さ）。
/// 返り値 = (tab の response = click と drag〈並べ替え〉, この frame で `×` を押したか)。新しい fn の理由: Page の tab（`theme::tab`）は 5 回目の形のまま（owner 209）で、× を中に持たない。
/// `lead` = 名前の左に icon の席（`ICON_UI` ＋ `SP_1`・TITLEBAR-ICONS-1101 の 2・描くのは呼ぶ側 = `tab_icon`）。
/// `height` = 帯の高さ（panel の帯 = `STRIP_H`・窓の一番下の帯 = `theme::STATUS_H`・PANEL-SYNC-1101）。
/// 例: `let (r, closed) = strip_tab(ui, "Terminal 1", true, Some("Terminate Terminal 1"), true, STRIP_H);`
fn strip_tab(ui: &mut egui::Ui, label: &str, selected: bool, close: Option<&str>, lead: bool, height: f32) -> (egui::Response, bool) {
    let t = theme::tokens(ui.ctx());
    let enabled = ui.is_enabled();
    let on = selected;
    let galley = theme::truncated(ui, label, Type::Helper.font(), egui::Color32::PLACEHOLDER, TAB_NAME_MAX);
    let close_w = if close.is_some() { theme::SP_1 + TAB_CLOSE } else { 0.0 };
    let lead_w = if lead { theme::ICON_UI + theme::SP_1 } else { 0.0 };
    // tab は drag で並べ替える
    let (rect, response) = ui.allocate_exact_size(egui::vec2(2.0 * TAB_PAD_X + lead_w + galley.size().x + close_w, height), egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, label));
    // × は tab の後に取る = 重なった所は × が押される（egui は後に取った widget を上とみなす）。押せる範囲 = × から tab の右端まで・帯の高さ
    let close_face = egui::Rect::from_center_size(egui::pos2(rect.right() - TAB_PAD_X - TAB_CLOSE / 2.0, rect.center().y), egui::Vec2::splat(TAB_CLOSE));
    let x = close.map(|name| {
        let hit = egui::Rect::from_min_max(egui::pos2(close_face.left(), rect.top()), rect.right_bottom());
        let x = ui.interact(hit, response.id.with("close"), egui::Sense::click());
        x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
        x
    });
    if ui.is_rect_visible(rect) {
        let over = enabled && ui.rect_contains_pointer(rect);
        if let (true, Some(c)) = (over, ui.visuals().interact_cursor) {
            ui.ctx().set_cursor_icon(c);
        }
        if on {
            ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, t.surface_2);
            let mark = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - theme::TAB_INDICATOR), rect.right_bottom());
            ui.painter().rect_filled(mark, egui::CornerRadius::ZERO, t.text_primary);
        }
        let ink = if on || over { t.text_primary } else { t.text_secondary };
        ui.painter().galley(egui::pos2(rect.left() + TAB_PAD_X + lead_w, rect.center().y - galley.size().y / 2.0), galley, ink);
        if let Some(x) = x.as_ref().filter(|_| on || over) {
            if x.hovered() {
                ui.painter().rect_filled(close_face, egui::CornerRadius::same(theme::RADIUS_SM / 2), t.surface_3);
            }
            let glyph = if x.hovered() { t.text_primary } else { t.text_secondary };
            theme::paint_icon(ui.painter(), Icon::X, close_face.center(), Type::Helper.size(), glyph, 0.0);
        }
        theme::focus_ring(ui, &response, rect, egui::CornerRadius::ZERO);
    }
    (response, x.is_some_and(|x| x.clicked()))
}

/// TITLEBAR-ICONS-1101 の 2: tab の名前の左の icon（前面の agent = `theme::kind_icon` の出所の順・shell だけ = Terminal の >_）と、角の状態の点
/// （Working = 回る印〈ICONS-1101 追記 238 の `states::paint_mark` の Running・青の点にしない〉・Needs you = `warning`・Done = `success`〈終わりの番号が 0 でなければ `danger`〉・
/// shell が終わった = `text_tertiary`・何も無ければ点なし）。
/// 前の字の印（▶ ● ✓ ○）は点に替えた（owner 230・231 = 字の記号を icon にしない）。
fn tab_icon(ui: &egui::Ui, tab: &egui::Response, on: bool, st: Option<&TabStatus>) {
    if !ui.is_rect_visible(tab.rect) {
        return;
    }
    let t = theme::tokens(ui.ctx());
    let ink = if on || tab.hovered() { t.text_primary } else { t.text_secondary };
    let center = egui::pos2(tab.rect.left() + TAB_PAD_X + theme::ICON_UI / 2.0, tab.rect.center().y);
    let kind = st.and_then(|s| s.kind);
    let rect = match kind {
        Some(k) => theme::kind_icon(ui, center, k.logo, k.app, None, k.label, ink),
        None => theme::kind_icon(ui, center, None, None, Some(Icon::SquareTerminal), "Terminal", ink),
    };
    let at = rect.right_bottom() - egui::vec2(TAB_DOT, TAB_DOT);
    // 地の色の輪で icon から切り離す（選んだ tab の地 = surface_2）
    let ring = if on { t.surface_2 } else { t.surface_1 };
    let dot = match (st.and_then(|s| s.agent.as_ref()), st) {
        (Some(AgentState::Working), _) => {
            ui.painter().circle_filled(at, TAB_DOT + 2.0, ring);
            crate::ui::states::paint_mark(ui.painter(), crate::ui::states::RegionState::Running, at, 2.0 * (TAB_DOT + 1.0), t.text_secondary);
            None
        }
        (Some(AgentState::NeedsYou), _) => Some(t.warning),
        (Some(AgentState::Done { code: Some(c) }), _) if *c != 0 => Some(t.danger),
        (Some(AgentState::Done { .. }), _) => Some(t.success),
        (None, Some(s)) if !s.running => Some(t.text_tertiary),
        _ => None,
    };
    if let Some(color) = dot {
        ui.painter().circle_filled(at, TAB_DOT + 1.0, ring);
        ui.painter().circle_filled(at, TAB_DOT, color);
    }
}

/// 帯の tab の並び。押す = 選ぶ・double-click = 名前替えの欄（Enter で確定・Esc でやめる）・drag = 並べ替え・右 click = `Rename` / `Terminate` の menu・
/// `×` = `Terminate`（確認は tab の横に 1 行）。`reveal` = 選んだ tab を見える所まで寄せる frame（追記 215・横の scroll の中）。
fn tab_row(ui: &mut egui::Ui, panel: &mut TerminalPanel, status: &BTreeMap<String, TabStatus>, reveal: bool) -> Option<PanelAct> {
    let t = theme::tokens(ui.ctx());
    let mut act = None;
    let active = panel.active_tab().map(|t| t.id.clone());
    let mut rects: Vec<(String, egui::Rect)> = Vec::new();
    let mut dropped: Option<(String, f32)> = None;
    for tab in panel.tabs.clone() {
        if let Some((_, text)) = panel.renaming.as_mut().filter(|(id, _)| *id == tab.id) {
            let field = ui.add(egui::TextEdit::singleline(text).desired_width(140.0).font(Type::Small.font()));
            let field = theme::ax_name(field, "Terminal name");
            if field.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    panel.renaming = None;
                } else {
                    act = Some(PanelAct::Rename { tab: tab.id.clone(), name: text.clone() });
                }
            } else if !field.has_focus() {
                field.request_focus();
            }
            continue;
        }
        let st = status.get(&tab.id);
        let agent = st.and_then(|s| s.agent.as_ref());
        // TITLEBAR-ICONS-1101 の 4: 1 行目 = 名前の全文（帯では切れることがある）・2 行目 = agent の名前と状態か Exited・始めた folder
        let what = match (agent, st) {
            (Some(a), Some(s)) => format!("{} · {} · ", s.kind.map_or("Agent", |k| k.label), a.label()),
            (None, Some(s)) if !s.running => "Exited · ".to_owned(),
            _ => String::new(),
        };
        let hover = format!("{}\n{what}{}", tab.name, tab.cwd);
        let on = !panel.chat && (active.as_deref() == Some(tab.id.as_str()) || panel.split.as_deref() == Some(tab.id.as_str()));
        // owner 210: × は tab の中（確認の 1 行が出ている間は出さない = 横の `Terminate` / `Keep` が答える）
        let asking = panel.confirm_close.as_deref() == Some(tab.id.as_str());
        let close_name = format!("Terminate {}", tab.name);
        let (r, closed) = strip_tab(ui, &tab.name, on, (!asking).then_some(close_name.as_str()), true, STRIP_H);
        tab_icon(ui, &r, on, st);
        if closed {
            act = Some(PanelAct::Close { tab: tab.id.clone() });
        }
        let r = r.on_hover_text(hover);
        if reveal && !panel.chat && active.as_deref() == Some(tab.id.as_str()) {
            r.scroll_to_me(None);
        }
        rects.push((tab.id.clone(), r.rect));
        if r.double_clicked() {
            panel.renaming = Some((tab.id.clone(), tab.name.clone()));
        } else if r.clicked() {
            act = Some(PanelAct::Select(tab.id.clone()));
        }
        if r.drag_stopped() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                dropped = Some((tab.id.clone(), p.x));
            }
        }
        // L7: 右 click の menu（`Rename`・`Terminate`）。`Memory::popup` で開く = WebView を隠す規則に入る（`theme::menu_popup` と同じ・`Response::context_menu` は入らない）
        let menu = ui.id().with(("terminal-tab-menu", &tab.id));
        if r.secondary_clicked() {
            ui.memory_mut(|m| m.toggle_popup(menu));
        }
        let picked = egui::popup::popup_below_widget(ui, menu, &r, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if theme::button_menu_item(ui, "Rename", "", Variant::Ghost).clicked() {
                return Some(true);
            }
            theme::button_menu_item(ui, "Terminate", "", Variant::Danger).clicked().then_some(false)
        })
        .flatten();
        match picked {
            Some(true) => panel.renaming = Some((tab.id.clone(), tab.name.clone())),
            Some(false) => act = Some(PanelAct::Close { tab: tab.id.clone() }),
            None => {}
        }
        if picked.is_some() {
            ui.memory_mut(|m| m.close_popup());
        }
        // 止める手は `Terminate` と名乗る（帯の `Hide terminal` は隠すだけ・05「Hidden panel ≠ Stopped process」）
        if asking {
            ui.add_space(TAB_PAD_X);
            theme::label(ui, Type::Small, format!("Terminate “{}”? What runs in it stops", tab.name), t.text_primary);
            ui.add_space(TAB_PAD_X);
            if theme::button_compact(ui, "Terminate", Variant::Danger).clicked() {
                act = Some(PanelAct::ConfirmClose { tab: tab.id.clone() });
            }
            ui.add_space(theme::SP_1);
            if theme::button_compact(ui, "Keep", Variant::Ghost).clicked() {
                act = Some(PanelAct::KeepOpen);
            }
        }
    }
    if let Some((id, x)) = dropped {
        let to = rects.iter().filter(|(rid, r)| *rid != id && r.center().x < x).count();
        act = Some(PanelAct::Move { tab: id, to });
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CHAT-SIDE-1101 追記 219〜221: 右の sidebar の中（`main_h` = None）も下と同じ帯 = `Chat` の tab・開閉の icon・`New tab`（同じ部品）。追記 212: `Chat` の右に ⇨ は無い。
    /// Chat の無い panel（Home の Terminal・`chat` = None）は `Chat` の tab が無く、Chat を選んでいた panel は Terminal の席として読む（`Show terminal`・`panel.chat` = false）。
    /// 負の対照: 右の間に `chat` を渡さない形に戻す → 右に Chat の tab が無く赤 / `if !here { panel.chat = false }` を外す → Home で `Show chat` のままで赤。
    #[test]
    fn chat_side_219_the_right_sidebar_draws_the_same_strip() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let labels = |ctx: &egui::Context, p: &mut TerminalPanel, main_h: Option<f32>, chat: bool| -> Vec<String> {
            let status = BTreeMap::new();
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut column = |_: &mut egui::Ui| {};
                    draw(ui, p, main_h, &status, &[], true, &mut |_: &mut egui::Ui, _: &PanelTab| {}, chat.then_some(&mut column as &mut dyn FnMut(&mut egui::Ui)));
                });
            });
            out.platform_output.accesskit_update.expect("AccessKit の木が無い").nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect()
        };
        for main_h in [Some(600.0), None] {
            let mut p = TerminalPanel::default();
            labels(&ctx, &mut p, main_h, true);
            let got = labels(&ctx, &mut p, main_h, true);
            assert!(got.iter().any(|l| l == "Chat") && got.iter().any(|l| l == "New tab"), "{main_h:?} の帯: {got:?}");
            // PANEL-SYNC-1101（owner 248「右sidebarの下矢印いらない」）: 開閉の icon（Show chat / Hide chat）は下の panel だけ。負の対照: `if main_h.is_some()` を外す → 右にも出て赤
            assert_eq!(got.iter().any(|l| l == "Show chat"), main_h.is_some(), "{main_h:?} の開閉の icon: {got:?}");
            assert!(!got.iter().any(|l| l.starts_with("Move chat")), "⇨ が在る（追記 212 で消した）: {got:?}");
        }
        let mut p = TerminalPanel::default();
        labels(&ctx, &mut p, Some(600.0), false);
        let home = labels(&ctx, &mut p, Some(600.0), false);
        assert!(!home.iter().any(|l| l == "Chat" || l == "Show chat"), "Chat の無い panel に Chat の tab: {home:?}");
        assert!(home.iter().any(|l| l == "Show terminal") && !p.chat, "Chat の無い panel が Chat の席のまま: {home:?}");
    }

    fn add(p: &mut TerminalPanel, id: &str) -> Option<PanelEffect> {
        p.apply(PanelAct::Add, || (id.to_owned(), "/tmp".to_owned()), |_| false)
    }

    fn ids(p: &TerminalPanel) -> Vec<&str> {
        p.tabs.iter().map(|t| t.id.as_str()).collect()
    }

    /// 900×600 の窓で panel を 1 frame 描き、返った手と AX の button（名前 → 枠）を返す
    fn strip(ctx: &egui::Context, p: &mut TerminalPanel, events: Vec<egui::Event>) -> (Option<PanelAct>, BTreeMap<String, egui::Rect>) {
        strip_at(ctx, p, events, false)
    }

    /// `bottom` = 製品と同じく窓の下端の `TopBottomPanel`（高さ = `height(600)`）に描く
    fn strip_at(ctx: &egui::Context, p: &mut TerminalPanel, events: Vec<egui::Event>, bottom: bool) -> (Option<PanelAct>, BTreeMap<String, egui::Rect>) {
        let mut act = None;
        let status = BTreeMap::new();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), events, ..Default::default() };
        let h = p.height(600.0);
        let out = ctx.run(input, |ctx| {
            let mut body = |ui: &mut egui::Ui| act = draw(ui, p, Some(600.0), &status, &["codex"], true, &mut |_: &mut egui::Ui, _: &PanelTab| {}, Some(&mut |_: &mut egui::Ui| {}));
            if bottom {
                egui::TopBottomPanel::bottom("strip-test").exact_height(h).frame(egui::Frame::new()).show(ctx, |ui| body(ui));
            } else {
                egui::CentralPanel::default().show(ctx, |ui| body(ui));
            }
        });
        let tree = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
        let buttons = tree
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == egui::accesskit::Role::Button)
            .filter_map(|(_, n)| Some((n.label()?.to_owned(), n.bounds().map(|b| egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))?)))
            .collect();
        (act, buttons)
    }

    /// G95: 畳む `STRIP_H`（owner 210 で 32・CHAT-ONE-1101: 底の欄の行は無い）・開く 35%・最大は主領域の全部（TERM-FEEL-1101）・境の drag は範囲に丸める。
    /// 変異: `height` の畳んだ腕を `main_h * fraction` にする → 畳んだ高さでなく赤・`drag_to` の clamp を外す → 0.05 が残り赤・
    /// 畳んだ腕に欄の行（32 ＋ 8）を戻す → 赤。
    #[test]
    fn g95_panel_height_folds_opens_and_maxes() {
        let mut p = TerminalPanel::default();
        assert_eq!(p.height(800.0), STRIP_H, "既定は Chat の畳んだ帯だけ（CHAT-ONE-1101: 底の欄の行は無い）");
        add(&mut p, "a");
        assert!(p.open, "足すと開く");
        assert_eq!(p.height(800.0), 800.0 * DEFAULT_FRACTION);
        p.apply(PanelAct::ToggleMax, || unreachable!(), |_| false);
        assert_eq!(p.height(800.0), 800.0, "最大 = 主領域の全部（TERM-FEEL-1101）");
        p.apply(PanelAct::ToggleMax, || unreachable!(), |_| false);
        p.apply(PanelAct::Resize { fraction: 0.05 }, || unreachable!(), |_| false);
        assert_eq!(p.fraction, MIN_FRACTION);
        p.apply(PanelAct::Toggle, || unreachable!(), |_| false);
        assert_eq!(p.height(800.0), STRIP_H, "⌃` で畳んだ Terminal の tab に帯の他が残った");
        assert!(p.height(60.0) >= STRIP_H, "低い窓でも帯は残る");
    }

    /// TERM-FEEL-1101（owner 182 通目「appのchatやterminalを一番上まで引き上げられない」）: 境を上端まで drag すると panel が主領域の全部・
    /// 下げると drag した所に戻る・吸い付く手前は drag した所のまま・吸い付いた後の double-click は吸い付く前の高さへ。
    /// 負の対照: `MAX_FRACTION` を 0.85 に戻す → 上端の drag が 0.85 に丸まり吸い付かず `p.max` が false で赤。
    #[test]
    fn term_feel_drag_to_the_top_fills_the_main_area_and_comes_back() {
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        let drag = |p: &mut TerminalPanel, f: f32| p.apply(PanelAct::Resize { fraction: f }, || unreachable!(), |_| false);
        drag(&mut p, 0.75);
        assert_eq!((p.max, p.height(800.0)), (false, 600.0), "drag した所");
        drag(&mut p, 1.0);
        assert!(p.max, "上端まで drag すると最大");
        assert_eq!(p.height(800.0), 800.0, "上端まで drag すると主領域の全部");
        drag(&mut p, 0.5);
        assert_eq!((p.max, p.height(800.0)), (false, 400.0), "下げると drag した所に戻る");
        drag(&mut p, SNAP_FRACTION - 0.02);
        assert!(!p.max, "吸い付く手前は drag した所のまま");
        assert!((p.height(800.0) - 800.0 * (SNAP_FRACTION - 0.02)).abs() < 0.01);
        drag(&mut p, SNAP_FRACTION + 0.01);
        assert!(p.max, "上端の近く（SNAP_FRACTION より上）で吸い付く");
        p.apply(PanelAct::ToggleMax, || unreachable!(), |_| false);
        assert!(!p.max);
        assert!((p.fraction - (SNAP_FRACTION - 0.02)).abs() < 1e-6, "吸い付いても前の高さを覚えている（double-click で戻る）");
    }

    /// TERM-ONE-1101: plan の Terminal は 1 回だけ tab になる（2 回当てても 1 つ・閉じた後に当てても戻らない）・id は spec の id・cwd は今の folder・
    /// 開閉と選んでいる tab は変えない。負の対照: `planned` の確かめを外す → 閉じた後に当てると戻って赤。
    #[test]
    fn term_one_plan_terminal_becomes_a_tab_once() {
        let mut p = TerminalPanel::default();
        let adopt = |p: &mut TerminalPanel| p.apply(PanelAct::Adopt { id: "app.dev".into(), name: "Dev".into() }, || (String::new(), "/tmp/a".into()), |_| false);
        let Some(PanelEffect::Spawn(tab)) = adopt(&mut p) else { panic!("plan の Terminal が tab にならない") };
        assert_eq!((tab.id.as_str(), tab.name.as_str(), tab.cwd.as_str()), ("app.dev", "Dev 1", "/tmp/a"));
        assert_eq!((p.open, p.chat, p.active.as_deref()), (false, true, Some("app.dev")), "開閉と Chat の席は変えない・選ぶ tab が無ければこれ");
        assert_eq!(adopt(&mut p), None, "2 回目は何もしない");
        assert_eq!(p.tabs.len(), 1);
        p.apply(PanelAct::Close { tab: "app.dev".into() }, || unreachable!(), |_| false);
        assert_eq!(adopt(&mut p), None, "閉じた tab を戻した");
        assert!(p.tabs.is_empty());
    }

    /// G95: tab の追加・rename・並べ替え・⌘N の選択・止める確認（子が動いていれば確認だけ・Terminate で止める）。
    /// 変異: `Close` の busy の分岐を外す → 確認を出さずに閉じて赤・`Move` の挿し直しを外す → 並びが変わらず赤。
    #[test]
    fn g95_tabs_add_rename_move_and_ask_before_stopping() {
        let mut p = TerminalPanel::default();
        assert!(matches!(add(&mut p, "a"), Some(PanelEffect::Spawn(t)) if t.name == "Terminal 1" && t.cwd == "/tmp"));
        add(&mut p, "b");
        add(&mut p, "c");
        assert_eq!(p.active.as_deref(), Some("c"), "足した tab を選ぶ");
        p.apply(PanelAct::Rename { tab: "b".into(), name: "  claude  ".into() }, || unreachable!(), |_| false);
        assert_eq!(p.tabs[1].name, "claude");
        p.apply(PanelAct::Rename { tab: "b".into(), name: " ".into() }, || unreachable!(), |_| false);
        assert_eq!(p.tabs[1].name, "claude", "空の名前にしない");
        p.apply(PanelAct::Move { tab: "c".into(), to: 0 }, || unreachable!(), |_| false);
        assert_eq!(ids(&p), ["c", "a", "b"]);
        p.apply(PanelAct::SelectNth(2), || unreachable!(), |_| false);
        assert_eq!(p.active.as_deref(), Some("b"));
        assert_eq!(p.apply(PanelAct::Close { tab: "b".into() }, || unreachable!(), |id| id == "b"), None, "動いている子は確認だけ");
        assert_eq!((ids(&p).len(), p.confirm_close.as_deref()), (3, Some("b")));
        p.apply(PanelAct::KeepOpen, || unreachable!(), |_| true);
        assert_eq!(p.confirm_close, None);
        assert_eq!(p.apply(PanelAct::ConfirmClose { tab: "b".into() }, || unreachable!(), |_| true), Some(PanelEffect::Stop("b".into())));
        assert_eq!(ids(&p), ["c", "a"]);
        assert_eq!(p.active.as_deref(), Some("a"), "閉じた tab の隣を選ぶ");
        assert_eq!(p.apply(PanelAct::Close { tab: "c".into() }, || unreachable!(), |_| false), Some(PanelEffect::Stop("c".into())), "動いていなければすぐ閉じる");
        p.apply(PanelAct::Close { tab: "a".into() }, || unreachable!(), |_| false);
        assert!(p.tabs.is_empty() && p.chat && p.open, "最後の tab を閉じたら Chat に戻る（L1-NORAIL）");
    }

    /// G95: ⌘D で左右に割る（1 つなら 2 つ目を足す）・もう 1 回で戻す・右の tab を閉じても戻る。
    /// 変異: `Split` の 2 つ目を足す腕を外す → split が None のまま赤・閉じる時の split の後始末を外す → 消えた tab を指して赤。
    #[test]
    fn g95_split_side_by_side_and_back() {
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        assert!(matches!(p.apply(PanelAct::Split, || ("b".into(), "/tmp".into()), |_| false), Some(PanelEffect::Spawn(t)) if t.id == "b"));
        assert_eq!((p.active.as_deref(), p.split.as_deref()), (Some("a"), Some("b")), "左 = 今の tab・右 = 足した tab");
        p.apply(PanelAct::Split, || unreachable!(), |_| false);
        assert_eq!(p.split, None, "もう 1 回で戻す");
        p.apply(PanelAct::Split, || unreachable!(), |_| false);
        assert_eq!(p.split.as_deref(), Some("b"), "2 つ在れば足さずに隣を右へ");
        p.apply(PanelAct::Close { tab: "b".into() }, || unreachable!(), |_| false);
        assert_eq!(p.split, None, "右を閉じたら戻る");
    }

    /// L7（05「Hidden panel ≠ Stopped process」・07「explicit Terminate in context menu」）: 隠す手と止める手は名前で分かれる。
    /// 帯は `Hide terminal`（隠すだけ）・tab の `×` は `Terminate Terminal 1`（`Close` と名乗らない）・右 click の menu に `Rename` と `Terminate`。
    /// menu の `Terminate` は `×` と同じ `Close` を返す = 動いている子は確認の 1 行（`Terminate` / `Keep`）を出すだけで止めない。
    /// 変異: `secondary_clicked` で開く行を外す → menu の Terminate が無く赤・`×` の名前を `Close {}` に戻す → 赤。
    #[test]
    fn l7_hide_and_terminate_are_named_apart_and_the_tab_menu_asks_first() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        let frame = |p: &mut TerminalPanel, events: Vec<egui::Event>| strip(&ctx, p, events);
        // 押す frame と離す frame（離した frame に手が返る）
        let click = |p: &mut TerminalPanel, at: egui::Pos2, button: egui::PointerButton| {
            let modifiers = egui::Modifiers::default();
            frame(p, vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button, pressed: true, modifiers }]);
            frame(p, vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers }]).0
        };
        frame(&mut p, vec![]);
        let (_, b) = frame(&mut p, vec![]);
        assert!(b.contains_key("Hide terminal") && b.contains_key("Terminate Terminal 1"), "帯の Hide terminal と tab の Terminate が無い: {:?}", b.keys());
        assert!(!b.keys().any(|k| k.starts_with("Close")), "止める手が Close と名乗った: {:?}", b.keys());
        assert!(!b.contains_key("Terminate"), "開く前から menu が出ている");
        click(&mut p, b["Terminal 1"].center(), egui::PointerButton::Secondary);
        let (_, b) = frame(&mut p, vec![]);
        assert!(b.contains_key("Rename") && b.contains_key("Terminate"), "右 click の menu に Rename / Terminate が無い: {:?}", b.keys());
        let act = click(&mut p, b["Terminate"].center(), egui::PointerButton::Primary);
        assert_eq!(act, Some(PanelAct::Close { tab: "a".into() }), "menu の Terminate が × と同じ手を返さない");
        assert_eq!(p.apply(act.unwrap(), || unreachable!(), |_| true), None, "動いている子は確認だけ（止めない）");
        let (_, b) = frame(&mut p, vec![]);
        assert!(b.contains_key("Terminate") && b.contains_key("Keep") && !b.contains_key("Rename"), "確認の 1 行（Terminate / Keep）が無いか menu が残った: {:?}", b.keys());
        assert_eq!(ids(&p), ["a"], "確認の前に tab を外した");
    }

    /// L7-ORCA #7（owner 10-31「show terminal の場所が見つけづらい」2 回目）: 開閉は帯の左端の button。
    /// tab が無くても在り・畳んでも開いても同じ左端・tab と `+` より左。AX 名は `Show terminal` / `Hide terminal`（BG の driver の labels.py と同じ）。
    /// UI-SWEEP-1101 追記 215（owner「showterminalとhideterminalを小さく。文字ではなくアイコンで」）: 文字の無い小さな icon 1 つ（幅 = `BUTTON_H`）。
    /// 変異: 開閉を帯の右端に戻す → 一番左でなく赤・文字つきの `strip_tab` に戻す → 幅が icon より広く赤。
    #[test]
    fn l7_show_terminal_is_a_named_button_at_the_strip_left() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut p = TerminalPanel::default();
        let shown = |p: &mut TerminalPanel, name: &str| {
            strip(&ctx, p, vec![]);
            let (_, b) = strip(&ctx, p, vec![]);
            let at = *b.get(name).unwrap_or_else(|| panic!("{name} が無い: {:?}", b.keys()));
            assert!(b.iter().all(|(k, r)| k == name || r.left() >= at.right()), "{name} が帯の一番左でない: {b:?}");
            assert!(at.width() <= theme::BUTTON_H + 0.5, "{name} が文字つきの button のまま（追記 215 = 小さな icon 1 つ）: {at:?}");
            (at, b)
        };
        // L1-NORAIL: tab の無い既定は Chat を選んでいる = `Show chat`（同じ左端の 1 つの button）
        let (empty, b) = shown(&mut p, "Show chat");
        assert!(b.contains_key("New tab") && b.contains_key("Chat"), "tab の無い帯に + か Chat の tab が無い");
        add(&mut p, "a");
        let (open, _) = shown(&mut p, "Hide terminal");
        p.apply(PanelAct::Toggle, || unreachable!(), |_| false);
        let (folded, b) = shown(&mut p, "Show terminal");
        assert!(b.contains_key("Terminal 1"), "畳んだ帯に tab が無い");
        assert!((empty.left() - folded.left()).abs() < 0.5 && (open.left() - folded.left()).abs() < 0.5, "開閉で左端が動いた: {empty:?} {open:?} {folded:?}");
        let modifiers = egui::Modifiers::default();
        let at = folded.center();
        strip(&ctx, &mut p, vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers }]);
        let (act, _) = strip(&ctx, &mut p, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers }]);
        assert_eq!(act, Some(PanelAct::Toggle), "Show terminal が開かない");
    }

    /// UI-SWEEP-1101 追記 215（owner「tabを横にscrollでもしたくさんtab増えても移動できるように」）: tab が帯に入りきらなくても、足した tab（選んだ tab）は
    /// `+` の左の見える所まで寄り（1 枚目は左へ隠れる）、shift＋wheel で横に戻すと 1 枚目に届く。`+` は帯の中に残る。
    /// 変異: tab_row の `scroll_to_me` を外す → 14 枚目が `+` の右（帯の外）で赤・ScrollArea を切り抜くだけの行に戻す → 寄らず wheel でも動かず赤。
    #[test]
    fn ui_sweep_215_many_tabs_scroll_sideways_and_the_picked_one_shows() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut p = TerminalPanel::default();
        for i in 0..14 {
            add(&mut p, &format!("t{i}"));
        }
        let frames = |p: &mut TerminalPanel, n: usize, events: Vec<egui::Event>| {
            let mut b = strip(&ctx, p, events).1;
            for _ in 0..n {
                b = strip(&ctx, p, vec![]).1;
            }
            b
        };
        let b = frames(&mut p, 30, vec![]); // scroll_to_me は数 frame かけて寄る（egui の scroll の animation）
        let (toggle, plus) = (b["Hide terminal"], b["New tab"]);
        assert!(plus.right() <= 900.0, "+ が帯の外: {plus:?}");
        let last = b["Terminal 14"];
        assert!(last.left() >= toggle.right() - 0.5 && last.right() <= plus.left() + 0.5, "足した 14 枚目が見える所に寄っていない: {last:?} toggle {toggle:?} + {plus:?}");
        assert!(b["Terminal 1"].right() <= toggle.right() + 0.5, "1 枚目が見えたまま = 溢れていない（試験の前提が崩れた）: {:?}", b["Terminal 1"]);
        let at = egui::pos2((toggle.right() + plus.left()) / 2.0, plus.center().y);
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let wheel = vec![egui::Event::PointerMoved(at), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 5000.0), modifiers: shift }];
        let b = frames(&mut p, 60, wheel);
        let first = b["Terminal 1"];
        assert!(first.left() >= toggle.right() - 0.5, "shift＋wheel で 1 枚目に戻らない: {first:?} toggle {toggle:?}");
        assert_eq!(p.active.as_deref(), Some("t13"), "scroll で選んだ tab が替わった");
    }

    /// L7-ORCA #9: grid の右 click の `Split Right` / `Split Down` は、その tab を左（上）に新しい Terminal を右（下）に置く（新しく起こす）。
    /// 上下の時は本体を上と下の半分ずつに描く。⌘D（Split）は左右に戻す。無い tab は何もしない。
    /// 変異: SplitDown の `down` を false にする → 向きが左右で赤・draw の上下の腕を外す → 左右に並んで赤。
    #[test]
    fn l7_split_right_and_down_add_a_new_terminal_beside_or_below() {
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        assert!(matches!(p.apply(PanelAct::SplitDown { tab: "a".into() }, || ("b".into(), "/tmp".into()), |_| false), Some(PanelEffect::Spawn(t)) if t.id == "b"));
        assert_eq!((p.active.as_deref(), p.split.as_deref(), p.down), (Some("a"), Some("b"), true));
        // 上下に描く: body が受けた枠（tab → rect）
        let ctx = egui::Context::default();
        let status = BTreeMap::new();
        let mut at: BTreeMap<String, egui::Rect> = BTreeMap::new();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(
                    ui,
                    &mut p,
                    Some(600.0),
                    &status,
                    &[],
                    true,
                    &mut |ui: &mut egui::Ui, tab: &PanelTab| {
                        at.insert(tab.id.clone(), ui.max_rect());
                    },
                    Some(&mut |_: &mut egui::Ui| {}),
                );
            });
        });
        let (a, b) = (at["a"], at["b"]);
        assert!(a.bottom() <= b.top() && (a.left() - b.left()).abs() < 0.5 && (a.width() - b.width()).abs() < 0.5, "上下に分かれない: a {a:?} b {b:?}");
        assert!(matches!(p.apply(PanelAct::SplitRight { tab: "b".into() }, || ("c".into(), "/tmp".into()), |_| false), Some(PanelEffect::Spawn(t)) if t.id == "c"));
        assert_eq!((p.active.as_deref(), p.split.as_deref(), p.down), (Some("b"), Some("c"), false));
        assert_eq!(p.apply(PanelAct::SplitDown { tab: "zz".into() }, || unreachable!(), |_| false), None, "無い tab で割った");
    }

    /// L1-NORAIL: Chat は下の panel の常設の tab（tab 0 でも在る・× 無し・既定で選んでいる）。Chat を選んでいる間の開閉は Terminal を起こさない・
    /// Terminal を足す / 選ぶと Chat は外れる・最後の Terminal を閉じると Chat に戻る・`chat` の無い古い保存は Chat を選んだ形で読む。
    /// 本体は Chat を選んで開いている間だけ `chat` の closure を呼び（CHAT-ONE-1101: 畳んだら呼ばない = 底の欄は無い）、grid（`body`）を呼ばない。
    /// 帯の `+` の menu の `Chat with Shibubu` の行（L7-ORCA #12 の menu に足した 1 行）も Chat を選ぶ。
    /// 変異: `Toggle` の `!self.chat &&` を外す → Chat の開閉で Terminal を起こして赤・draw の Chat の腕を外す → grid を描いて赤・menu の Chat の行を外す → 赤。
    #[test]
    fn l1_chat_is_a_permanent_tab_of_the_bottom_panel() {
        let mut p = TerminalPanel::default();
        assert!(p.chat && !p.open, "既定は Chat を選んで畳んだ形");
        assert_eq!(p.apply(PanelAct::Toggle, || unreachable!("Chat の開閉で Terminal を起こした"), |_| false), None);
        assert!(p.chat && p.open, "Show chat で開かない");
        add(&mut p, "a");
        assert!(!p.chat && p.open, "Terminal を足しても Chat のまま");
        p.apply(PanelAct::Chat, || unreachable!(), |_| false);
        assert!(p.chat && p.open && p.tabs.len() == 1, "Chat の tab で Chat に戻らない（Terminal は残る）");
        p.apply(PanelAct::Select("a".into()), || unreachable!(), |_| false);
        assert!(!p.chat, "Terminal の tab で Chat が外れない");
        let old: TerminalPanel = serde_json::from_str(r#"{"open":true,"tabs":[]}"#).unwrap();
        assert!(old.chat, "chat の無い古い保存が Chat を選ばない");
        p.apply(PanelAct::Chat, || unreachable!(), |_| false);
        let ctx = egui::Context::default();
        let status = BTreeMap::new();
        let (mut grids, mut chats) = (0, Vec::new());
        for open in [true, false] {
            p.open = open;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw(ui, &mut p, Some(600.0), &status, &[], true, &mut |_: &mut egui::Ui, _: &PanelTab| grids += 1, Some(&mut |_: &mut egui::Ui| chats.push(open)));
                });
            });
        }
        assert_eq!((grids, chats), (0, vec![true]), "Chat を選んでいる間に grid を描いたか、畳んだ時に Chat を描いた");
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        strip(&ctx, &mut p, vec![]);
        let (_, b) = strip(&ctx, &mut p, vec![]);
        assert!(b.contains_key("Chat") && b.contains_key("Show chat") && !b.contains_key("Terminate Chat"), "帯に Chat の tab か Show chat が無い・Chat に × がある: {:?}", b.keys());
        assert!(b["Chat"].right() <= b["Terminal 1"].left(), "Chat の tab が Terminal の tab の左に無い: {b:?}");
        // + の menu の Chat の行（Terminal を選んで開いている時に Chat へ戻る道）
        let click = |p: &mut TerminalPanel, at: egui::Pos2| {
            let modifiers = egui::Modifiers::default();
            strip_at(&ctx, p, vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers }], true);
            strip_at(&ctx, p, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers }], true).0
        };
        p.apply(PanelAct::Select("a".into()), || unreachable!(), |_| false);
        strip_at(&ctx, &mut p, vec![], true);
        let (_, b) = strip_at(&ctx, &mut p, vec![], true);
        click(&mut p, b["New tab"].center());
        let (_, m) = strip_at(&ctx, &mut p, vec![], true);
        let row = *m.get("Chat with Shibubu").unwrap_or_else(|| panic!("+ の menu に Chat の行が無い: {:?}", m.keys()));
        assert_eq!(click(&mut p, row.center()), Some(PanelAct::Chat), "+ の menu の Chat の行が Chat を選ばない");
    }

    /// CHAT-ONE-1101（owner 184「terminalとしたのchatの間の空間がいらない」）: Terminal の tab で開いている間は `chat` を呼ばず、grid の枠の下端と panel の下端の差は
    /// 1 行の高さ未満・畳むと何も描かない。新しい card の `Reveal` は開くだけで tab を替えず（Terminal なら Terminal のまま）、Terminal を起こさない（R1）。
    /// 変異: draw の Terminal の腕で底に欄の行（`COMMAND_H + 2 × SP_1`）を取ってから grid を描く → 差が 1 行を越えて赤・`Reveal` を `Chat` と同じにする → 赤。
    #[test]
    fn chat_one_the_terminal_tab_grid_reaches_the_panel_bottom() {
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        p.open = false;
        assert_eq!(p.apply(PanelAct::Reveal, || unreachable!("card で Terminal を起こした"), |_| false), None);
        assert!(p.open && !p.chat && p.active.as_deref() == Some("a"), "card で Terminal の tab から Chat に替えた: {p:?}");
        let ctx = egui::Context::default();
        let status = BTreeMap::new();
        let (mut grids, mut chats, mut bottom) = (Vec::new(), 0, 0.0);
        for open in [true, false] {
            p.open = open;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                    bottom = ui.max_rect().bottom();
                    let grid = &mut |ui: &mut egui::Ui, _: &PanelTab| grids.push(ui.max_rect().bottom());
                    draw(ui, &mut p, Some(600.0), &status, &[], true, grid, Some(&mut |_: &mut egui::Ui| chats += 1));
                });
            });
        }
        assert_eq!(chats, 0, "Terminal の tab の間か畳んだ時に Chat（欄）を描いた");
        assert_eq!(grids.len(), 1, "開いた Terminal の tab で grid を描かないか、畳んで描いた: {grids:?}");
        let line = Type::Body.line_height();
        assert!(bottom - grids[0] < line, "grid の下端 {} と panel の下端 {bottom} の間が 1 行（{line}）以上空いた", grids[0]);
        // Chat を選んで畳んでいる時の Reveal は Chat を開く（会話の列に card）
        p.apply(PanelAct::Chat, || unreachable!(), |_| false);
        p.open = false;
        p.apply(PanelAct::Reveal, || unreachable!(), |_| false);
        assert!(p.open && p.chat, "Chat の tab の card で開かない: {p:?}");
    }

    /// L7-ORCA #12・UI-SWEEP-1101 追記（owner 210「orca真似して」）: `+` は tab の並びのすぐ右の icon 1 つ（AX `New tab`・`BUTTON_H` 四方・L7-ORCA #11 の文字つき `+` をやめた）で、
    /// 押すと上に menu: `New terminal`・入っている agent（`Agent`）・`New browser tab`（`On this page`）。agent の行は新しい tab（名前 `codex 1`）で shell を起こしてその agent を打つ。
    /// `AGENTS` に無い名前は何もしない。
    /// 変異: `+` を文字つきの `+ New` に戻す → 幅が icon でなく赤・右端の群へ戻す → tab のすぐ右でなく赤・agent の行を外す → `Codex` が無く赤・
    /// AddAgent の `AGENTS` の照合を外す → 知らない名前で tab を足して赤。
    #[test]
    fn l7_plus_new_opens_a_menu_of_terminal_agents_and_browser() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut p = TerminalPanel::default();
        let click = |p: &mut TerminalPanel, at: egui::Pos2| {
            let modifiers = egui::Modifiers::default();
            strip_at(&ctx, p, vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers }], true);
            strip_at(&ctx, p, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers }], true).0
        };
        strip_at(&ctx, &mut p, vec![], true);
        let (_, b) = strip_at(&ctx, &mut p, vec![], true);
        let new = *b.get("New tab").unwrap_or_else(|| panic!("帯に +（New tab）が無い: {:?}", b.keys()));
        assert!((new.width() - theme::BUTTON_H).abs() < 0.5 && (new.height() - theme::BUTTON_H).abs() < 0.5, "+ が icon 1 つの大きさでない（文字つきの + New のまま?）: {new:?}");
        assert!(new.left() >= b["Chat"].right() && new.left() - b["Chat"].right() < 2.0 * theme::SP_2 && new.right() <= b["Terminal options"].left(), "+ が tab の並びのすぐ右に無い: {b:?}");
        assert!(!b.contains_key("New terminal"), "押す前から menu が出ている");
        let wanted = ["New terminal", "Codex", "New browser tab"];
        for (name, want) in wanted.iter().zip([PanelAct::Add, PanelAct::AddAgent { agent: "codex".into() }, PanelAct::NewBrowserTab]) {
            click(&mut p, new.center());
            let (_, b) = strip_at(&ctx, &mut p, vec![], true);
            let rows: Vec<egui::Rect> = wanted.iter().map(|w| *b.get(*w).unwrap_or_else(|| panic!("+ の menu に {w} が無い: {:?}", b.keys()))).collect();
            assert!(rows.iter().all(|r| r.bottom() <= new.top() + 0.5), "menu が帯の上に開かない: {rows:?}");
            assert_eq!(click(&mut p, b[*name].center()), Some(want), "{name} の手");
            let (_, b) = strip_at(&ctx, &mut p, vec![], true);
            assert!(!b.contains_key("New terminal"), "選んでも menu が閉じない");
        }
        assert!(matches!(p.apply(PanelAct::AddAgent { agent: "codex".into() }, || ("x".into(), "/tmp".into()), |_| false), Some(PanelEffect::Spawn(t)) if t.name == "codex 1" && t.command.as_deref() == Some("codex")));
        assert_eq!(p.apply(PanelAct::AddAgent { agent: "rm -rf ~".into() }, || unreachable!(), |_| false), None, "AGENTS に無い名前で tab を足した");
        // SIGNIN-1101: sign in の tab は渡された command を持つ（表の外を打たない守りは generated の panel_spec）
        assert!(matches!(p.apply(PanelAct::SignIn { name: "Kimi Code".into(), command: "kimi login".into() }, || ("k".into(), "/tmp".into()), |_| false), Some(PanelEffect::Spawn(t)) if t.name == "Kimi Code 1" && t.command.as_deref() == Some("kimi login")));
        p.apply(PanelAct::Close { tab: "k".into() }, || unreachable!(), |_| false);
        assert_eq!(p.apply(PanelAct::NewBrowserTab, || unreachable!(), |_| false), None, "Browser は panel の tab にしない");
        assert_eq!(ids(&p), ["x"]);
        let back: TerminalPanel = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back.tabs[0].command.as_deref(), Some("codex"), "agent の tab を保存しない");
        assert!(!serde_json::to_string(&PanelTab { id: "a".into(), name: "Terminal 1".into(), cwd: "/tmp".into(), command: None, named: false }).unwrap().contains("command"), "shell だけの tab の保存の形が変わった");
    }

    /// UI-SWEEP-1101 追記（owner 210「terminalとかの枠マジでいらない。ここら辺とかorca真似して」）: 帯の tab は枠なしで帯の高さ（`STRIP_H` = Orca の 32）いっぱい・
    /// 選んだ tab だけ面（`surface_2`）・`×` は tab の中（`Terminate …` の枠が tab の枠の中）・`+` は tab の右の icon・帯の中の rect に線 0。
    /// 変異: `strip_tab` を `button_compact` に戻す → 高さが 24 で赤・`×` を tab の後の icon_button に戻す → tab の中に無く赤・選んだ tab の面を外す → surface_2 が無く赤・
    /// 選んでいない tab にも面を塗る → 赤。
    #[test]
    fn ui_sweep_210_strip_tabs_are_frameless_like_orca() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        add(&mut p, "b");
        strip(&ctx, &mut p, vec![]);
        let (_, b) = strip(&ctx, &mut p, vec![]);
        for name in ["Chat", "Terminal 1", "Terminal 2"] {
            let r = *b.get(name).unwrap_or_else(|| panic!("{name} が無い: {:?}", b.keys()));
            assert!((r.height() - STRIP_H).abs() < 0.5, "{name} が帯の高さいっぱいでない: {r:?}");
        }
        let (one, two) = (b["Terminal 1"], b["Terminal 2"]);
        assert!(one.contains_rect(b["Terminate Terminal 1"]) && two.contains_rect(b["Terminate Terminal 2"]), "× が tab の中に無い: {b:?}");
        let status = BTreeMap::new();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &mut p, Some(600.0), &status, &[], true, &mut |_: &mut egui::Ui, _: &PanelTab| {}, Some(&mut |_: &mut egui::Ui| {}));
            });
        });
        let t = theme::tokens(&ctx);
        let rects: Vec<egui::epaint::RectShape> = out.shapes.iter().filter_map(|c| match &c.shape {
            egui::Shape::Rect(r) if r.rect.bottom() <= two.bottom() + 0.5 => Some(r.clone()),
            _ => None,
        }).collect();
        assert!(rects.iter().all(|r| r.stroke.width == 0.0 || r.stroke.color.a() == 0), "帯に枠の線: {rects:?}");
        let face = |tab: egui::Rect| rects.iter().any(|r| r.fill == t.surface_2 && (r.rect.left() - tab.left()).abs() < 0.5 && (r.rect.width() - tab.width()).abs() < 0.5);
        assert!(face(two) && !face(one), "選んだ tab（Terminal 2）だけに surface_2 の面でない: {rects:?}");
    }

    /// G95: 保存するのは開閉・高さ・tab の名前と cwd と並び（打ちかけの名前と確認は残さない）。
    /// 変異: `renaming` の `#[serde(skip)]` を外す → 読み戻しに打ちかけが残り赤。
    #[test]
    fn g95_panel_keeps_tabs_but_not_the_typing() {
        let mut p = TerminalPanel::default();
        add(&mut p, "a");
        p.renaming = Some(("a".into(), "half".into()));
        p.confirm_close = Some("a".into());
        let back: TerminalPanel = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!((back.open, back.tabs.clone(), back.active.clone()), (true, p.tabs.clone(), Some("a".into())));
        assert_eq!((back.renaming, back.confirm_close), (None, None));
        let old: TerminalPanel = serde_json::from_value(serde_json::json!({ "open": true })).unwrap();
        assert_eq!(old.fraction, DEFAULT_FRACTION, "欄の足りない古い行も読める");
    }
}
