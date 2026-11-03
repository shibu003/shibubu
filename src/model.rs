use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

pub fn new_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    nanos ^ SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PanelKind { GitHubIssues, GitHubPulls, WebService, Desktop, Feature, Workflow, Notes }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub id: u64,
    pub kind: PanelKind,
    pub title: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub hidden: bool,
}
impl Panel {
    pub fn new(kind: PanelKind, title: &str, column: usize, row: usize) -> Self {
        Self {id:new_id(),kind,title:title.into(),x:30.0+column as f32*350.0,
            y:24.0+row as f32*296.0,width:326.0,height:240.0,locked:false,hidden:false}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DesktopTool { Click, TypeText }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionKind {
    GitHubIssues { repo: String },
    GitHubPulls { repo: String },
    GitHubCreateIssue { repo: String, title: String, body: String },
    WebRead,
    /// app_name = 登録した時の app 名（YC W17-2・押す前に今の app 名と照合）。空 = W17-2 より前の binding（照合しない）
    Desktop { pid: i64, window_id: i64, label: String, role: String, tool: DesktopTool, text: String, #[serde(default)] app_name: String },
    TrustedCli { feature_id: u64 },
}
impl ActionKind {
    pub fn is_write(&self) -> bool {
        matches!(self, Self::GitHubCreateIssue{..}|Self::Desktop{..}|Self::TrustedCli{..})
    }
    pub fn window(&self) -> Option<(i64, i64)> {
        match self {Self::Desktop{pid,window_id,..}=>Some((*pid,*window_id)),_=>None}
    }
    pub fn exclusive_resource(&self) -> Option<String> {
        match self {
            Self::Desktop{pid,window_id,..}=>Some(format!("gui:{pid}:{window_id}")),
            Self::GitHubCreateIssue{repo,..}=>Some(format!("github-write:{repo}")),
            Self::TrustedCli{feature_id}=>Some(format!("trusted-cli:{feature_id}")),
            _=>None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {Self::GitHubIssues{..}=>"GitHub Issuesを読む",Self::GitHubPulls{..}=>"GitHub PRを読む",
            Self::GitHubCreateIssue{..}=>"GitHub Issueを作成",Self::WebRead=>"Web JSONを読む",
            Self::Desktop{..}=>"画面の操作でDesktopを操作",Self::TrustedCli{..}=>"信頼済CLIを実行"}
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {pub id:u64,pub name:String,pub kind:ActionKind}
impl Action {pub fn new(name:&str,kind:ActionKind)->Self{Self{id:new_id(),name:name.into(),kind}}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {pub id:u64,pub name:String,pub steps:Vec<u64>}
/// 旧 canvas の Desktop の手を足す先の Workflow の名前（FX4〈FX-017〉で足した・本体で `engine::generate` がこの名前で作る）。
/// 前の名前 `Desktopで処理する` は保存済みの sessions.json に残る（data は書き換えない）ので、探す側は名前でなく `Workflow::is_desktop` を使う。
/// Wave: FX4
pub const DESKTOP_WORKFLOW: &str = "Desktop steps";
impl Workflow {
    /// 旧 canvas の Desktop の手を足す先か（名前が `DESKTOP_WORKFLOW` か前の名前 `Desktopで処理する`）。
    /// 例（app.rs の Desktop の手・Trusted CLI の手）: `s.workflows.iter().position(Workflow::is_desktop)`。
    /// 保証する: 旧 session（前の名前）も新しい session も同じ Workflow に足す（片方だけ替えると `workflows.first_mut()` = `情報を集める` に落ちる）。
    /// 保証しない: 保存済みの名前を書き換える事（旧 session の data は読めるまま・互換 canvas は FX-059 で消す）。
    /// Wave: FX4
    pub fn is_desktop(&self)->bool{self.name==DESKTOP_WORKFLOW||self.name=="Desktopで処理する"}
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id:u64,pub name:String,pub prompt:String,pub repo:String,
    pub panels:Vec<Panel>,pub actions:Vec<Action>,pub workflows:Vec<Workflow>,pub history:Vec<String>,
    #[serde(default)] pub drafts:std::collections::HashMap<String,String>,
}
/// starter の名前（保存済みの sessions.json の名前と照合する鍵でもある・`Session::is_untouched_starter`）。
const STARTER_NAME: &str = "はじめての統合アプリ";
impl Session {
    pub fn duplicate(&self) -> Self {
        use std::collections::HashMap;
        let mut duplicate = self.clone();
        duplicate.id = new_id();
        duplicate.name = format!("{} のコピー", self.name);
        duplicate.history.clear();
        for panel in &mut duplicate.panels { panel.id = new_id(); }
        let mut mapping = HashMap::new();
        for action in &mut duplicate.actions {
            let old_id = action.id;
            action.id = new_id();
            mapping.insert(old_id, action.id);
        }
        for workflow in &mut duplicate.workflows {
            workflow.id = new_id();
            workflow.steps = workflow.steps.iter().filter_map(|id| mapping.get(id).copied()).collect();
        }
        duplicate
    }
    /// 契約（FX4〈FX-017〉・署名は変えない・本体で替える）: 最初の Notes の部品の名前 `ようこそ` を英語の事実の語 `Notes` に（保存済みの旧 session の名前は読めるまま・
    /// `is_untouched_starter` は部品の名前を見ないので starter の判定は変わらない）。`STARTER_NAME` は保存済みの名前と照合する鍵なので替えない。
    /// Wave: FX4
    pub fn starter() -> Self {
        Self {id:new_id(),name:STARTER_NAME.into(),prompt:String::new(),repo:String::new(),
            panels:vec![Panel::new(PanelKind::Notes,"Notes",0,0)],actions:vec![],workflows:vec![],history:vec![],drafts:std::collections::HashMap::new()}
    }
    pub fn find_action(&self,id:u64)->Option<&Action>{self.actions.iter().find(|a|a.id==id)}
    /// 手を付けていない `starter()` のままか（sidebar・Home・palette の行から外す・`App::rows`）。名前の照合は starter を作るこの file だけが持つ
    /// （UI-1: app.rs は画面の文の集合なので、保存済みの sessions.json の名前との照合をここへ移した）。
    /// Wave: UI-1
    pub fn is_untouched_starter(&self)->bool{
        self.name==STARTER_NAME&&self.prompt.is_empty()&&self.actions.is_empty()&&self.history.is_empty()&&self.panels.len()==1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoFeature {
    pub id:u64,pub owner:String,pub repo:String,pub subdir:String,pub commit:String,
    pub license:String,pub local_path:String,pub cli_path:String,pub cli_args:Vec<String>,pub enabled:bool,
    /// 「CLI接続を登録する」時の実行ファイルと引数の指紋（repos::cli_pin・ADR-002 硬化 (b)）。空 = 未登録（この版より前の登録も）
    #[serde(default)] pub cli_pin:String,
}

/// 殻の永続設定（DESIGN §2.5 の theme 強制・§1.1 の rail の pin）。無い旧 JSON は default。
/// LOOK-1101（owner 11-01「defaultでdark modeにして、設定のところで変えられるように」）: 既定は Dark（前は System）。利用者が選んでいない保存は
/// `UiSettings::theme_chosen` が false で、読む時に既定（Dark）にする（前の既定 System のままの保存も Dark で開く）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ThemeChoice { System, Light, #[default] Dark }
/// CHAT-SIDE-1101（owner 204 の 2「右sidebar復活。右でchatもできるように。したか右かを選べるように」）: Project の panel（Chat と Terminal の tab）の置き場（全 Project 共通・`UiSettings::chat_place`）。
/// `Bottom` = 窓の下（今まで・既定）・`Right` = 右の sidebar（追記 219〜221: 同じ panel・同じ tab の列・同じ pty を右に描き、下には描かない）。
/// 開閉は panel の `open`（右で閉じると何も描かない・header の ◨ か ⌃` で開く）。
/// 追記 219（owner「右上にsidebar出し入れのアイコンがあるのに常に右に<があるの？」）: 右で畳んだ `RightClosed` と右端の ‹ の帯は消した。前の保存の `"RightClosed"` は `Bottom` で読む。
/// 新しい型の理由: ThemeChoice と同じ 1 つの選択の値。
/// Wave: CHAT-SIDE-1101
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ChatPlace { #[default] #[serde(alias = "RightClosed")] Bottom, Right }
/// MIRROR-SPEED-1101（owner 205「秒/枚数の方法でいくならそれ設定で変えられるように」）: Mirror の映像の枚数の 3 択（Settings › General の `Mirror smoothness`）。
/// 新しい型の理由: 既にある enum（`ThemeChoice`・`AutomationMode`）はどれも別の設定の値で、この 3 つを入れる所が無い。数を打つ欄は作らない。
/// 保存の形: `"mirror_smoothness": "smooth" | "saver"`（既定の Auto は書かない = 前の版の file と同じ形）。
/// 読み手: app.rs の当て直し（起動の 1 回と `SettingsChanged`）が `surface::mirror::set_smoothness(s.arg())` に渡す（helper の `--smooth` と stdin の `{"cmd":"smoothness"}`）。
/// Wave: MIRROR-SPEED-1101
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MirrorSmoothness {
    /// 60 枚 / 秒で始め、機械が混んだら helper が 30・15 に下げる（既定）
    #[default]
    Auto,
    /// 60 枚 / 秒のまま
    Smooth,
    /// 15 枚 / 秒（電池と熱を減らす）
    Saver,
}

impl MirrorSmoothness {
    /// helper の `--smooth` と `{"cmd":"smoothness","mode":…}` の語（KangaeMirror.swift の頭）
    pub fn arg(self) -> &'static str {
        match self {
            MirrorSmoothness::Auto => "auto",
            MirrorSmoothness::Smooth => "smooth",
            MirrorSmoothness::Saver => "saver",
        }
    }
    /// 保存の形に書かない値（既定）
    pub fn is_auto(&self) -> bool {
        *self == MirrorSmoothness::Auto
    }
}
/// 窓全体の殻の設定（`Store.ui`・`sessions.json` に `Store` ごと保存・書き手は `self.store.ui.<field> = …; self.mark()` = debounce の保存）。
/// 契約（UI-4b から）: field `sidebar_hidden`・`drafts` を足した（どれも `#[serde(default)]` = 前の版の file も読める・前の版の build は知らない key を読み飛ばす）。
/// 保存の形（`sessions.json` の `"ui"`）: `{"theme": "System", "rail_pinned": false, "sidebar_hidden": null | true | false, "drafts": {"home": "…", "<project id>": "…"}}`。
/// 例: `let open = Tier::current(ctx).sidebar_open(store.ui.sidebar_hidden);`。失敗の種類は無い（読めない file は今の `Store::load` の隔離と通知のまま）。
/// 保証しない: 窓ごとの設定（窓は 1 つ・`main.rs` の ViewportBuilder。窓が増えたら窓の設定へ移す）。
/// 契約（FX4 で field `recent_commands` を足した・末尾 = 他の列〈RQ1・RQ3〉の足した field と別の行）。
/// 契約（RQ1 から・Settings の全節の保存はこの 1 つ・新しい設定の file を作らない）: field `reduce_motion`・`reopen_last_project`・`last_project`・`text_zoom`・
/// `default_model`・`default_effort`・`llm_max_calls`・`llm_max_secs` を足した（どれも `#[serde(default)]` = 前の版の file も読め、無い key は「OS に従う / Home / 100% / CLI の既定 / env か既定の上限」）。
/// 保存の形に足した key: `"reduce_motion": null | true | false, "reopen_last_project": false, "last_project": null | "<project id>", "text_zoom": null | 120,
/// "default_model": null | "sonnet", "default_effort": null | "high", "llm_max_calls": null | 10, "llm_max_secs": null | 600`。
/// この file は serde と std（と時刻の `crate::google::rfc3339_utc`）だけを使う（`crate::llm` を引かない: tests/adapters.rs が model.rs と google だけを `#[path]` で取り込む）ので、上限は `llm::Budget` でなく env と同じ 2 つの数で持つ。
/// 書き手: Settings の form（`crate::settings::draw` が `&mut` で書き `Intent::SettingsChanged`）と、`last_project` だけは app.rs の `go`（Project へ移った時）。
/// 契約（RQ3 から）: field `history_days` を足した（`#[serde(default)]`・無い key は 30 日）。
/// 契約（RQ4 から）: field `web_data_url`・`disconnected` を足した（`#[serde(default)]`・無い key は「Web data 無し」「切った接続 0」）。保存の形に足した key:
/// `"web_data_url": null | "https://api.example.com/items", "disconnected": ["github"]`。Web data の token・GitHub の token は Keychain だけ（ここに書かない）。
/// 読み手: app.rs の当て直し（起動の 1 回と `SettingsChanged`）。env（`KANGAE_REDUCE_MOTION`・`SHIBUBU_LLM_BUDGET`・`KANGAE_WEB_SERVICE_URL`）は試験の口として残し、ここに値が在ればこちらが勝つ。
/// 契約（LOOK-1101 から）: field `theme_chosen` を足した（`#[serde(default)]` = false）。false の保存は `theme` を読まず既定（Dark）で開く。
/// 書き手 = `Intent::SetTheme` の腕だけ（Settings › General › Appearance と ⌘K の Appearance の行が同じ intent）。保存の形に足した key: `"theme_chosen": false | true`。
/// 契約（MIRROR-SPEED-1101 から）: field `mirror_smoothness` を足した（`MirrorSmoothness`・`#[serde(default)]` = Auto・Auto は保存の形に書かない）。
/// 契約（SESSIONS-LEFT-1101 追記 244 から）: field `folded_headings` を足した（`#[serde(default)]` = 空 = sidebar の見出しは全部開く）。保存の形に足した key: `"folded_headings": ["projects"]`。
/// Wave: UI-4b, FX4, RQ1, RQ3, RQ4, LOOK, MIRROR-SPEED-1101
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct UiSettings {
    /// RQ5: 提案の Hide / Later の覚え（`agent::Feedback::to_json`・Null = 無い）。再起動しても出し直さない
    #[serde(default)] pub agent_feedback: serde_json::Value,
    #[serde(default)] pub theme: ThemeChoice,
    /// LOOK-1101: 利用者が Appearance を選んだ印（Match system / Light / Dark のどれでも・選んだら true のまま）。false = 選んでいない = 既定の Dark で開く。
    #[serde(default)] pub theme_chosen: bool,
    #[serde(default)] pub rail_pinned: bool,
    /// 人が sidebar を畳んだか（owner-addendum ②「畳んだ状態を覚える」）。`None` = 一度も畳む / 戻すをしていない = 窓の幅の既定（`Tier::sidebar_hidden`）に従う・
    /// `Some(true)` = 畳んだ・`Some(false)` = 戻した。書き手は `Intent::ToggleSidebar` の arm だけ（窓の幅の既定で畳んで始まっても書かない）。読み手 = `Tier::sidebar_open`。
    // ponytail: 窓は 1 つなので窓全体の 1 欄。窓が増えたら窓ごとの設定へ移す。
    #[serde(default)] pub sidebar_hidden: Option<bool>,
    /// 場所ごとの打ちかけ（`ui::shell::Shell::drafts` と同じ key = `"home"` か Project の id・空の文は持たない）。UI-4a の Done 6「store を開き直した後に両方とも戻る」。
    /// 書き手（UI-4b の本体）: app.rs が `Shell::drafts` と今の場所の `command` を写す（打った frame ごとに `mark()` しない・場所を替えた時の debounce の保存と `on_exit` の保存の前）。
    /// 読み手: 起動の 1 回（`Shell::drafts` に入れ、今の場所の分を `command` へ）。保証しない: 消えた Project の key を残す事（起動で無い Project の key は捨てる）・
    /// IME の変換中の文（確定した文だけ）。
    #[serde(default)] pub drafts: BTreeMap<String, String>,
    /// RQ-023: Engine の Automation の 3 択（既定 Ask before write）。書き手 = Settings › Automation の行（RQ1 の Settings の画面）・読み手 = app.rs が
    /// `Generated::automation` へ写す（Engine の依頼ごとに `EngineRequest::automation`）。前の版の file は key が無く既定で読む。
    #[serde(default)] pub automation: crate::domain::permission::AutomationMode,
    /// FX4（FX-016・UIUX N17「local history opt-in, dedup」）: ⌘K の最近の行（`ui::palette::Palette::recent` の写し）。`None` = 覚えない（既定・一度も選んでいない
    /// 旧い sessions.json も None）・`Some(v)` = 覚える（選んだ行の label・新しい順・重ならない・`palette::RECENT_MAX` まで）。
    /// 書き手（本体）: app.rs の `snapshot_drafts`（`Shell::drafts` と同じ時）。読み手: 起動の 1 回（`Palette::recent` へ・今の一覧に無い label は ⌘K が飛ばす）。
    /// 保証しない: 保存期間と消す規則（RQ-014）・打った文（欄の文は覚えない = `drafts` の持ち物）。
    #[serde(default)] pub recent_commands: Option<Vec<String>>,
    /// RQ1（General · Reduce motion）: `None` = OS の「視差効果を減らす」に従う（`motion::clear_motion_mode`）・`Some(b)` = 人が選んだ（`motion::set_reduce_motion`）。
    #[serde(default)] pub reduce_motion: Option<bool>,
    /// RQ1（General · When Shibubu opens）: false = Home（既定・外に効く操作は起こさない）・true = `last_project` の Project（無い・開けなければ Home と理由の 1 行）。
    #[serde(default)] pub reopen_last_project: bool,
    /// RQ1: 最後に開いた Project の id（`reopen_last_project` の行き先・書き手は app.rs の `go` だけ）。消えた Project の id は起動で読み捨てる。
    #[serde(default)] pub last_project: Option<String>,
    /// RQ1（General · Text size）: 文字の大きさ（％・egui の `zoom_factor` × 100）。`None` = 100。⌘+ / ⌘− / ⌘0（egui の既定の key）で変えた値もここへ写す。
    /// 範囲は 80〜150（外は読む時に詰める）。f32 にしないのは `Eq` を保つため（`Store` の比較）。
    #[serde(default)] pub text_zoom: Option<u16>,
    /// RQ1（AI & Models · Default model）: CLI の `--model` に渡す別名（`claude --help` の `fable`・`opus`・`sonnet`）。`None` = 付けない（CLI の既定）。
    // G84: 書き手 = `+` の `Model ▸`（`Intent::SetModel`・`domain::chat::MODELS` の語）・argv = `llm_cli::command_line` の `--model`（provider が受ける別名だけ）。
    // 選ぶ所は `+` の menu の 1 か所（Settings には描かない・同じ値を 2 か所で替えさせない）。
    #[serde(default)] pub default_model: Option<String>,
    /// RQ1（AI & Models · Default effort）: CLI の `--effort` に渡す段（`low`・`medium`・`high`・`xhigh`・`max`）。`None` = 付けない。持ち主と描く条件は `default_model` と同じ。
    /// G104 × RQ-N: `+` の `Effort ▸` も同じ席（書き手 = `Intent::SetEffort` の腕・読み手 = 起動の 1 回 → `Generated::effort` と `Shell::effort`）。
    /// 前の .app が書いた key `"effort"` も読む。両方在る時は `"default_effort"`（新しい key）を採り、書く時は `"default_effort"` だけ。
    #[serde(default)] pub default_effort: Option<String>,
    /// RQ1（AI & Models · Execution limit）: 起動ごとに数える AI の呼び出しの回数の上限（`llm::Budget::resolve` が env の `calls=` より先に使う）。`None` = env か既定（10 回）。
    #[serde(default)] pub llm_max_calls: Option<u32>,
    /// RQ1（AI & Models · Execution limit）: 呼び出しにかかった秒の合計の上限（env の `secs=` より先）。`None` = env か既定（600 秒）。
    #[serde(default)] pub llm_max_secs: Option<u64>,
    /// RQ2（RQ-007・Continue の pin）: 上に留める Project の id（押した順）。書き手 = `Intent::PinProject` の腕だけ・読み手 = app.rs が `AppRow::pinned` に写す。
    /// 消した Project の id は `Intent::DeleteProject` の腕と起動の読み込みで捨てる（`drafts` と同じ）。
    #[serde(default)] pub pinned_projects: Vec<String>,
    /// RQ2（RQ-005・Archive）: Home の Continue と sidebar から外した Project の id（消さない・⌘K の `Open {name}` には残る・`Unarchive` で戻る）。書き手 = `Intent::ArchiveProject` の腕だけ。
    #[serde(default)] pub archived_projects: Vec<String>,
    /// RQ2（RQ-009）: Home の `Try` から作った Project の id（行に `Sample` の印・AI を使わず外に書かない例だけ）。書き手 = `Intent::TrySample` の腕だけ。
    #[serde(default)] pub sample_projects: Vec<String>,
    /// RQ2（RQ-007・Continue の Recent）: Project の id → 最後に開いた時刻（unix 秒）。書き手 = app.rs の `go`（Project へ移った時・`last_project` と同じ所）。
    /// 保証しない: 別の Mac の時刻・Project を開かずに走った run の時刻（開いた時だけ）。
    #[serde(default)] pub opened_projects: BTreeMap<String, i64>,
    /// SESSIONS-LEFT-1101（owner 198 の 3「開いた状態は覚える」）: sidebar で session と Page を開いた Project の id（開いた順）。書き手 = `Intent::FoldProject` の腕・
    /// app.rs の `go`（Project へ移った時に足す・owner 210 の 6）と `Intent::DeleteProject` の腕（消す）。RQ2 の `continue_by_name`（Continue の並び）の席を替えた（前の版の保存の key は読み捨てる）。
    #[serde(default)] pub open_projects: Vec<String>,
    /// SESSIONS-LEFT-1101 追記 244（owner「projectsの中身はしまえるように畳めるようにしたい」）: sidebar で畳んだ見出しの鍵（`sidebar::HEADING_SESSIONS`・`HEADING_PROJECTS`）。
    /// 書き手 = `Intent::FoldHeading` の腕だけ。`open_projects` と逆向き（見出しは既定が開いた形 = 覚えるのは畳んだ物）。
    #[serde(default)] pub folded_headings: Vec<String>,
    /// CHAT-SIDE-1101: Project の panel（Chat と Terminal）の置き場（下 / 右）。書き手 = `Intent::ChatPlace` の腕だけ（header の ◨・右に出している間の ⇧⌘P・Settings › General › Chat）。
    /// 読み手 = app.rs が毎 frame（panel を下に描くか右の sidebar に描くか）。保存の形: `"chat_place": "Bottom" | "Right"`（前の `"RightClosed"` は `Bottom` で読む）。
    #[serde(default)] pub chat_place: ChatPlace,
    /// CHAT-SIDE-1101（operator・owner 189 = 開き直しても同じ）: 右の sidebar の幅（pt・`agent_rail::SIDE_W_MIN..=SIDE_W_MAX` に丸める）。`None` = 既定の `SIDE_W`。
    /// 書き手 = app.rs（境を引き終えて幅が替わった frame）。全 Project 共通（置き場と同じ）なので Project ごとの ui_state ではなくここ。
    #[serde(default)] pub chat_width: Option<u16>,
    /// RQ2（RQ-ACCT・RQ-API・AI & Models の Accounts）: Shibubu が足した AI の sign-in（足した順）。System default（その Mac の `claude` の login）は列に入れない（常に在る）。
    /// 保存しない物: email（画面に出すだけ・`claude auth status --json` を毎回読む）・API key の値（Keychain だけ・ここは末尾 4 文字）。
    #[serde(default)] pub ai_accounts: Vec<AiAccount>,
    /// RQ2（RQ-ACCT）: 次の AI の呼び出しに使う account の id（`AiAccount::id`）。`None` = System default（何も選ばない人 = 今のまま plan の login・鍵を外す）。
    /// 切り替えは次の呼び出しから（読んでいる途中の 1 回は前の account のまま・作業中の会話と Project を動かさない）。無い id は起動で `None` に戻す。
    #[serde(default)] pub ai_active: Option<String>,
    /// G84（operator 10-29 ②）: `ai_active` の account の種類（`AiAccount::kind`）。手で書いた設定で id が重なっても（`Plan{id:"env"}` と `EnvKey`）種類と id の組で引く。
    /// `None` = この field の前の設定（id だけ・列で最初の物）。書くのは `set_active_account`（id と一緒に）。
    #[serde(default, skip_serializing_if = "Option::is_none")] pub ai_active_kind: Option<String>,
    /// RQ3（RQ-014・Settings › Data & Recovery の `Run history and AI calls` の `Keep for`）: Run と Engine と AI の呼び出しの記録・開いた日を残す日数（7 / 30 / 90）。
    /// `None` = 30。読み手 = app.rs が起動の 1 回と `SettingsChanged` で `Generated::prune_records` に渡す。画面と入力の記録（7 日）・同意・Project は対象でない
    /// （`ui::shell::DataKind` の doc）。保存の形: `"history_days": null | 7 | 30 | 90`（他の数は読む時に 30 へ）。
    #[serde(default)] pub history_days: Option<u16>,
    /// RQ4（RQ-015・Connections › Web data の `Connect`）: Web data として読む 1 つの URL（https か loopback・`github::web_url_ok` が通る物だけを書く）。`None` = 未設定
    /// （env `KANGAE_WEB_SERVICE_URL` は試験の口・ここに値が在ればこちらが勝つ）。書き手 = `Intent::Connection { row: "web", act: Submit | Remove }` の腕だけ。
    /// 読み手 = app.rs が `adapters::web_json::WebJson::new(url, token)` に渡す（token は Keychain の `secret_store::connection_key("web", "token")`・要る時だけ）。
    // ponytail: 1 つだけ（`WebJson::id` が "web_json" の 1 つ）。2 つ目の Web data が要る時に Vec と adapter の id を行ごとにする。
    #[serde(default)] pub web_data_url: Option<String>,
    /// RQ4（RQ-015・`Disconnect` / `Reconnect`）: Shibubu が使うのをやめた接続の行の key（`"github"`・`"web"`）。設定と Keychain の秘密は残す（`Reconnect` で同じ設定に戻る）。
    /// MCP の server は `mcp::McpServerConfig::disabled`・Trusted command は `RepoFeature::enabled` が持つ（Engine が読む所に置く・ここには入れない）。
    /// 読み手 = app.rs（切った GitHub は token を渡さず public の読みだけ・切った Web data は adapter を作らない）と、外した接続を使う Project の席の 1 行（RQ-019 M26 =
    /// `verifier::Check::Fail(Need::Connect, "{name} is disconnected · Reconnect in Connections")`）。
    #[serde(default)] pub disconnected: Vec<String>,
    /// PASSWORD-1101: 繋いだ password manager（chat の card か Connections の `Connect` で sign in できた物・`Disconnect` で外す）。項目の参照と値は入れない
    /// （参照は Connections の行の秘密 = Keychain の `connection_key` の値・値は使う瞬間だけ `domain::vault::with_secret`）。空なら保存の形に書かない。
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub vaults: Vec<crate::domain::vault::Manager>,
    /// MIRROR-SPEED-1101: Mirror の映像の枚数（既定 Auto は保存の形に書かない）。書き手 = Settings › General の `Mirror smoothness`（feature mirror の build だけ描く）
    #[serde(default, skip_serializing_if = "MirrorSmoothness::is_auto")] pub mirror_smoothness: MirrorSmoothness,
    /// ACCOUNTS-1101 2 段目（作る物 5・owner 203）: 利用者が付けた account の名前（key = `AiAccount::key` = `<kind>/<id>`・足した account = `Plan` だけ）。
    /// 無い account の画面の名前は `llm_cli::account_name` の既定（sign in で分かった email の @ の前 → `<AI> <n>`）。書き手 = `Intent::RenameAiAccount` の腕
    /// （空 = 外す）と `Intent::RemoveAiAccount` の腕（外す）。email は保存しない。空なら保存の形に書かない（前の版は知らない key を読み捨てる）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub account_names: BTreeMap<String, String>,
}

/// `"effort"` と `"default_effort"` が両方在ると serde の alias は duplicate field で設定全部を隔離する。読む前に 1 つの key へ寄せる（両方在る時は新しい key）。
impl<'de> Deserialize<'de> for UiSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer).map_err(serde::de::Error::custom)?;
        if let Some(obj) = value.as_object_mut() {
            if obj.contains_key("default_effort") {
                obj.remove("effort");
            } else if let Some(old) = obj.remove("effort") {
                obj.insert("default_effort".to_owned(), old);
            }
        }
        let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
        let mut de = serde_json::Deserializer::from_str(&raw);
        let mut ui = UiSettingsDe::deserialize(&mut de).map_err(serde::de::Error::custom)?;
        // LOOK-1101: Appearance を選んでいない保存（前の既定 System のままの owner の Mac も）は既定の Dark で開く
        if !ui.theme_chosen {
            ui.theme = ThemeChoice::default();
        }
        Ok(ui)
    }
}

#[derive(Deserialize)]
#[serde(remote = "UiSettings")]
struct UiSettingsDe {
    #[serde(default)] agent_feedback: serde_json::Value,
    #[serde(default)] theme: ThemeChoice,
    #[serde(default)] theme_chosen: bool,
    #[serde(default)] rail_pinned: bool,
    #[serde(default)] sidebar_hidden: Option<bool>,
    #[serde(default)] drafts: BTreeMap<String, String>,
    #[serde(default)] automation: crate::domain::permission::AutomationMode,
    #[serde(default)] recent_commands: Option<Vec<String>>,
    #[serde(default)] reduce_motion: Option<bool>,
    #[serde(default)] reopen_last_project: bool,
    #[serde(default)] last_project: Option<String>,
    #[serde(default)] text_zoom: Option<u16>,
    #[serde(default)] default_model: Option<String>,
    #[serde(default)] default_effort: Option<String>,
    #[serde(default)] llm_max_calls: Option<u32>,
    #[serde(default)] llm_max_secs: Option<u64>,
    #[serde(default)] pinned_projects: Vec<String>,
    #[serde(default)] archived_projects: Vec<String>,
    #[serde(default)] sample_projects: Vec<String>,
    #[serde(default)] opened_projects: BTreeMap<String, i64>,
    #[serde(default)] open_projects: Vec<String>,
    #[serde(default)] folded_headings: Vec<String>,
    #[serde(default)] chat_place: ChatPlace,
    #[serde(default)] chat_width: Option<u16>,
    #[serde(default)] ai_accounts: Vec<AiAccount>,
    #[serde(default)] ai_active: Option<String>,
    #[serde(default)] ai_active_kind: Option<String>,
    #[serde(default)] history_days: Option<u16>,
    #[serde(default)] web_data_url: Option<String>,
    #[serde(default)] disconnected: Vec<String>,
    #[serde(default)] vaults: Vec<crate::domain::vault::Manager>,
    #[serde(default)] mirror_smoothness: MirrorSmoothness,
    #[serde(default)] account_names: BTreeMap<String, String>,
}

/// RQ2（RQ-ACCT・RQ-API）: Shibubu が足した AI の sign-in 1 つ（`UiSettings::ai_accounts` の 1 行）。子の `claude` をどう起こすかは `llm_cli::child_env` が決める。
/// 例: `settings.ai_accounts.push(AiAccount::Key { id: "key-1".into(), last4: "a1b2".into() }); settings.ai_active = Some("key-1".into());`
/// 保証する: 秘密を持たない（plan の login は `<data dir>/ai-accounts/<id>` の CLAUDE_CONFIG_DIR に Claude Code が書く・鍵の値は Keychain の `ai-key.<id>`）・
/// `id` は data dir の中で重ならない（`plan-<n>` / `key-<n>` / `env`）。保証しない: login がまだ効いている事（画面は `claude auth status --json` を読み直す）。
/// 新しくした理由: 既存の型に AI の sign-in を持つ物が無い（`ui::connections::Connection` は画面の行で serde が無い・`llm_cli::CliConfig` は起動ごとの子の起こし方で保存しない）。
/// この file は serde と std だけ（tests/adapters.rs が model.rs だけを取り込む）なので llm_cli には置かず、ここに置いて llm_cli と settings が読む。
/// Wave: RQ2
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiAccount {
    /// Shibubu が足した plan の login（`claude auth login --claudeai` を CLAUDE_CONFIG_DIR = `<data dir>/ai-accounts/<id>` で・`--console` は出さない）。子の env の鍵の変数は全部外す。
    /// ACCOUNTS-1101（owner 203「providerごとにadd accountで複数アカウント」）: id = `plan-<n>`（Claude・前から）か `<provider>-<n>`（grok・kimi・opencode の足した login・
    /// 置き場は同じ `<data dir>/ai-accounts/<id>` を各 CLI の置き場の変数で渡す = `llm_cli::account_env`）。どの AI かは `provider()`。
    /// 新しい variant にしない理由: 古い版の Shibubu が設定を読んだ時に知らない `kind` で `ai_accounts` ごと読めなくなる（並ぶ build を行き来する）。Plan のままなら古い版は読める。
    Plan { id: String },
    /// 利用者が `Paste API key` で渡した Anthropic の鍵（使った分の請求 = 選ぶ時と使っている間に `Billed per request by Anthropic`）。値は Keychain の `ai-key.<id>`・ここは末尾 4 文字だけ。
    Key { id: String, last4: String },
    /// 環境に既に在る `ANTHROPIC_API_KEY`（`Use the key from your environment`・打たせない・値は持たない）。利用者が Claude Code で断った鍵（`~/.claude.json` の rejected）は使わない。
    EnvKey,
    /// Mac で見つかった AI の CLI、または動いている local LLM（`llm_cli::found_rows`）。認証は聞き直さない。
    /// 新しくした理由: `Plan` / `Key` / `EnvKey` は Claude の login か API key で、grok・codex・Ollama・LM Studio の名前を持てない。
    /// PROVIDER-1101（owner 180 通目「機械から検知したものを全て表示させるようにして」）: 在るのに sign in していない CLI も行にする（`needs_sign_in`）。
    /// 選ぶと `Sign in in Terminal first` の 1 行と Terminal で sign in する手（`auth login` は Shibubu が起こさない）。保存の古い行（印無し）は sign in 済み。
    /// 例: `AiAccount::Found { id: "kimi".into(), name: "Kimi".into(), needs_sign_in: true }`
    Found {
        id: String,
        name: String,
        #[serde(default)]
        needs_sign_in: bool,
    },
}

/// 111 通目（docs 05「Provider states」・QA L-01・L-02）: Provider の行の本当の状態。PATH に在る・port が開いている事は `Ready` の根拠にならない
/// （`Ready` は選んで本当に確かめが通った AI だけ・`Detected` = 見つけただけ）。Composer の下の行は `Shell::provider_states` を読む（描画は L1）。
/// 新しくした理由: `llm::LlmError` は失敗の種類・`DisplayTier` は Connections の段で、Provider の「見つけた / sign in が要る / 使える / 使えない」を 1 語で持つ型が無い。
/// `Error`（呼んで失敗した）は `LlmError` の種類が AI の行まで届く時に足す（今は文でしか届かない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderState {
    Detected,
    NeedsSignIn,
    Ready,
    Unavailable,
}

#[allow(dead_code)] // 読み手（app）が無い試験の crate
impl ProviderState {
    pub const fn label(self) -> &'static str {
        match self {
            ProviderState::Detected => "Detected",
            ProviderState::NeedsSignIn => "Needs sign-in",
            ProviderState::Ready => "Ready",
            ProviderState::Unavailable => "Unavailable",
        }
    }

    /// `active` = 使っている行・`ready` = その AI の確かめが通った（`Generated::llm_ready`）・`missing` = 通らなかった理由の 1 行（`Generated::llm_line`）。
    /// 使っていない行は確かめていないので `Detected`。使っている行は確かめの答えだけで決める（答えがまだ無ければ `Detected`）。
    pub fn of(active: bool, ready: bool, missing: Option<&str>) -> Self {
        match (active, ready, missing) {
            (false, ..) => ProviderState::Detected,
            (true, true, _) => ProviderState::Ready,
            (true, false, Some(why)) if why.to_lowercase().contains("sign") => ProviderState::NeedsSignIn,
            (true, false, Some(_)) => ProviderState::Unavailable,
            (true, false, None) => ProviderState::Detected,
        }
    }
}

#[allow(dead_code)] // 読み手（llm_cli・settings）が無い試験の crate（tests/adapters.rs は model.rs だけを取り込む）
impl AiAccount {
    /// `UiSettings::ai_active` と data dir の置き場に使う id（`EnvKey` は `"env"`）。
    /// Wave: RQ2
    pub fn id(&self) -> &str {
        match self {
            AiAccount::Plan { id } | AiAccount::Key { id, .. } | AiAccount::Found { id, .. } => id,
            AiAccount::EnvKey => "env",
        }
    }

    /// G84（operator 10-29 ②）: 種類の語（serde の `kind` と同じ・`UiSettings::ai_active_kind`）。
    pub fn kind(&self) -> &'static str {
        match self {
            AiAccount::Plan { .. } => "plan",
            AiAccount::Key { .. } => "key",
            AiAccount::EnvKey => "env_key",
            AiAccount::Found { .. } => "found",
        }
    }

    /// ACCOUNTS-1101: その account の AI（`"claude"`・`llm_cli::FOUND_CLIS` / `LOCAL_LLMS` の id）。`Plan` は id の `<provider>-<数字>` の前半（`plan-<n>` と形の違う id は Claude）・
    /// `Key` / `EnvKey` は Claude・`Found` はその id。例: `Plan { id: "grok-2" }.provider()` → `"grok"`・`Plan { id: "plan-1" }.provider()` → `"claude"`。
    /// Wave: ACCOUNTS-1101
    pub fn provider(&self) -> &str {
        match self {
            AiAccount::Plan { id } => match id.rsplit_once('-') {
                Some((p, n)) if p != "plan" && !p.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => p,
                _ => "claude",
            },
            AiAccount::Key { .. } | AiAccount::EnvKey => "claude",
            AiAccount::Found { id, .. } => id,
        }
    }

    /// ACCOUNTS-1101 2 段目: 種類と id の組の 1 語 `<kind>/<id>`（`UiSettings::account_names` の key と Settings の行の印が同じ字・G84 の id の重なりを避ける）。
    /// 例: `Plan { id: "plan-1" }.key()` → `"plan/plan-1"`
    /// Wave: ACCOUNTS-1101
    pub fn key(&self) -> String {
        format!("{}/{}", self.kind(), self.id())
    }
}

#[allow(dead_code)] // 読み手（app・settings）が無い試験の crate（tests/adapters.rs は model.rs だけを取り込む）
impl UiSettings {
    /// RQ2 × G84: 次の呼び出しに使う account（`ai_active` を `ai_accounts` で引く・None = System default・列に無い id も None）。
    /// 種類が書いてあれば種類と id の組で引く（id だけで引くと、手で書いた `Plan{id:"env"}` を選んでも列で先の `EnvKey` = 従量の鍵が使われる）。
    pub fn active_account(&self) -> Option<&AiAccount> {
        let id = self.ai_active.as_deref()?;
        self.ai_accounts.iter().find(|a| a.id() == id && self.ai_active_kind.as_deref().map_or(true, |k| k == a.kind()))
    }

    /// L4: 見つかった AI の行（`AiAccount::Found`）を今の答えに揃える（消えた物を外し・新しい物を列の最後へ・他の account は動かさない）。変わった時 true。
    pub fn set_found(&mut self, rows: Vec<AiAccount>) -> bool {
        let before = self.ai_accounts.clone();
        self.ai_accounts.retain(|a| !matches!(a, AiAccount::Found { .. }) || rows.contains(a));
        for r in rows {
            if !self.ai_accounts.contains(&r) {
                self.ai_accounts.push(r);
            }
        }
        before != self.ai_accounts
    }

    /// 使う account を替える（id と種類を一緒に書く・None = System default）。戻り値 = 前の (id, 種類)（確かめが通らなければ app が書き戻す）。
    pub fn set_active_account(&mut self, a: Option<&AiAccount>) -> (Option<String>, Option<String>) {
        let (id, kind) = (a.map(|a| a.id().to_owned()), a.map(|a| a.kind().to_owned()));
        (std::mem::replace(&mut self.ai_active, id), std::mem::replace(&mut self.ai_active_kind, kind))
    }

    /// ACCOUNTS-1101 2 段目: 利用者が付けた名前（None = 付けていない = 呼ぶ側が `llm_cli::account_name` の既定）。例: `llm_cli::account_name(&a, ui.account_name(&a), email)`
    /// Wave: ACCOUNTS-1101
    pub fn account_name(&self, a: &AiAccount) -> Option<&str> {
        self.account_names.get(&a.key()).map(String::as_str)
    }

    /// ACCOUNTS-1101 2 段目: 名前を付ける。前後の空白を落として 40 字で切る・空 = 外す（既定の名前に戻る）・足した account（`Plan`）で列に在る物だけ。
    /// 戻り値 = 変わった（保存の印を立てる）。例: `if ui.set_account_name(&acct, "work") { self.mark() }`
    /// Wave: ACCOUNTS-1101
    pub fn set_account_name(&mut self, a: &AiAccount, name: &str) -> bool {
        if !matches!(a, AiAccount::Plan { .. }) || !self.ai_accounts.contains(a) {
            return false;
        }
        let cut: String = name.trim().chars().take(40).collect();
        let (key, name) = (a.key(), cut.trim_end().to_owned());
        if name.is_empty() {
            return self.account_names.remove(&key).is_some();
        }
        self.account_names.insert(key, name.clone()).as_deref() != Some(name.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {pub sessions:Vec<Session>,pub features:Vec<RepoFeature>,pub selected:u64,#[serde(default)] pub ui:UiSettings,
    /// FX1（FX-003）: この起動の `load` が壊れた file を隔離した先（RQ3 から `backups/<UTC>-damaged/<name>`・保存しない = `#[serde(skip)]`・次の起動では None）。
    /// 読み手: app.rs が作業の場所の頭に `load` の通知の 1 行と `Reveal in Finder`（`generated::run_open("/usr/bin/open", &["-R", path])`）を出す。
    /// 例: `if let Some(p) = &self.store.set_aside { if theme::button(ui, "Reveal in Finder", Variant::Ghost).clicked() { … } }`
    /// 保証する: Some なら、その path に壊れた file が在る（`fs::rename` が通った時だけ入れる）。隔離に失敗した時は None で、通知の 1 行だけが理由を言う。
    /// 保証しない: kangae.sqlite3（Project の本体）の隔離（開けない時は `Generated::store_lost`・backup / restore は RQ3）。索引は `Store::load` の行
    #[serde(skip)]
    pub set_aside:Option<PathBuf>,
}
impl Default for Store {
    fn default()->Self {let first=Session::starter();Self{selected:first.id,sessions:vec![first],features:vec![],ui:UiSettings::default(),set_aside:None}}
}
impl Store {
    /// 戻り値の Option は画面に出す通知。file 無し・正常は None、壊れていたら隔離して既定 store + 通知。
    /// FX1: 隔離できたら戻す store の `set_aside` に隔離した先を入れる（署名は今のまま・Reveal in Finder の path）。
    /// 契約（RQ3 から・署名は変えない・本体は RQ3 の実装の片）: 隔離の先を `path` の dir の `backups/<UTC の YYYYMMDDTHHMMSSZ>-damaged/sessions.json` に替え
    /// （Settings › Data & Recovery の Backups の list に `Damaged · set aside` で出る・`Generated::restore_backup` で選べる）、通知は
    /// `Your settings were damaged and set aside · Settings › Data & Recovery`。前の版が置いた `sessions.json.corrupt-<秒>` も list が読む（動かさない）。
    /// 保証する: 壊れた file を消さない。保証しない: 壊れた file から読める所だけを拾う事（全部か既定か）。
    /// Wave: FX1, RQ3
    pub fn load(path:&Path)->(Self,Option<String>) {
        let bytes=match fs::read(path){
            Ok(b)=>b,
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return (Self::default(),None),
            Err(_)=>return Self::quarantine(path),
        };
        match serde_json::from_slice(&bytes){Ok(store)=>(store,None),Err(_)=>Self::quarantine(path)}
    }
    fn quarantine(path:&Path)->(Self,Option<String>) {
        let secs=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let name=path.file_name().map(|n|n.to_string_lossy().into_owned()).unwrap_or_default();
        // 世代の id は generated::BACKUPS_DIR の doc の形（`<UTC>-damaged`・Settings › Data & Recovery の Backups に出て Restore で選べる）
        let set=path.with_file_name("backups").join(format!("{}-damaged",utc_stamp(secs)));
        let moved=fs::create_dir_all(&set).and_then(|()|fs::rename(path,set.join(&name)));
        let mut store=Self::default();
        let note=match moved{
            Ok(())=>{store.set_aside=Some(set.join(&name));"Your settings were damaged and set aside · Settings › Data & Recovery".to_owned()}
            Err(e)=>format!("Your settings were damaged and couldn't be set aside ({}) · they are replaced when Shibubu saves",e.kind()),
        };
        (store,Some(note))
    }
    pub fn save(&self,path:&Path)->Result<(),String> {
        if let Some(p)=path.parent(){fs::create_dir_all(p).map_err(|e|e.to_string())?}
        let tmp=path.with_extension("json.tmp");
        let data=serde_json::to_vec_pretty(self).map_err(|e|e.to_string())?;
        fs::write(&tmp,data).map_err(|e|e.to_string())?;
        fs::rename(&tmp,path).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn selected(&self)->Option<&Session>{self.sessions.iter().find(|s|s.id==self.selected)}
    pub fn selected_mut(&mut self)->Option<&mut Session>{self.sessions.iter_mut().find(|s|s.id==self.selected)}
}

/// RQ3: unix 秒 → 世代の id の頭 `YYYYMMDDTHHMMSSZ`（UTC・名前の順 = 時刻の順）。`generated` の世代と `Store::load` の隔離が同じ形を使う。
pub fn utc_stamp(secs:u64)->String{crate::google::rfc3339_utc(secs).replace(['-',':'],"")}

#[cfg(test)]
mod tests {
    /// 111 通目: Provider の状態は確かめの答えだけで決まる。使っていない行・まだ答えが無い行は `Ready` にならない（QA L-01）。
    /// 負の対照: `of` の先頭の `(false, ..)` の腕を外す → 使っていない行が Unavailable / Ready になって赤。
    #[test]
    fn provider_state_is_ready_only_after_a_real_check() {
        use ProviderState::*;
        assert_eq!(ProviderState::of(false, true, None), Detected, "使っていない行が Ready");
        assert_eq!(ProviderState::of(true, false, None), Detected, "確かめる前に Ready");
        assert_eq!(ProviderState::of(true, true, None), Ready);
        assert_eq!(ProviderState::of(true, false, Some("Grok isn't signed in")), NeedsSignIn);
        assert_eq!(ProviderState::of(true, false, Some("That AI isn't running on this Mac")), Unavailable);
        assert_eq!(ProviderState::of(false, false, Some("Grok isn't signed in")), Detected, "使っていない行に別の AI の失敗が移った");
    }

    /// L4: 見つかった AI の行は答えに揃う（消えた物は外れ・新しい物は足され・plan と鍵の account は動かない）。
    /// 負の対照: `set_found` の `retain` を外す → 消えた Ollama が残って赤。
    #[test]
    fn l4_set_found_follows_the_answer_and_keeps_other_accounts() {
        let f = |id: &str| AiAccount::Found { id: id.into(), name: id.into(), needs_sign_in: false };
        let mut ui = UiSettings::default();
        ui.ai_accounts = vec![AiAccount::Plan { id: "a".into() }, f("ollama")];
        assert!(!ui.set_found(vec![f("ollama")]), "同じ答えで変わったと言った");
        assert!(ui.set_found(vec![f("grok")]));
        assert_eq!(ui.ai_accounts, [AiAccount::Plan { id: "a".into() }, f("grok")]);
        ui.set_active_account(Some(&f("grok")));
        assert_eq!(ui.active_account(), Some(&f("grok")));
        ui.set_found(vec![]);
        assert_eq!(ui.active_account(), None, "消えた AI が使っている行のまま");
    }

 use super::*;
 fn tmp_dir(tag:&str)->std::path::PathBuf{let d=std::env::temp_dir().join(format!("kangae-store-{tag}-{}",new_id()));fs::create_dir_all(&d).unwrap();d}
 #[test] fn load_corrupt_json_quarantines_and_notifies(){
     let d=tmp_dir("bad");let p=d.join("sessions.json");fs::write(&p,"{not json").unwrap();
     let (store,note)=Store::load(&p);
     assert_eq!(store.sessions.len(),1);
     // RQ3: 隔離の先は backups/<UTC>-damaged/sessions.json。FX1 の Reveal は set_aside がその file を指す。
     let note=note.expect("message");assert_eq!(note,"Your settings were damaged and set aside · Settings › Data & Recovery");
     assert!(store.set_aside.as_deref().is_some_and(|a|a.exists()&&a.starts_with(d.join("backups"))),"FX1: 隔離した先が set_aside に無い: {:?}",store.set_aside);
     assert!(!p.exists());
     let sets:Vec<_>=fs::read_dir(d.join("backups")).unwrap().map(|e|e.unwrap().path()).collect();
     assert!(sets.len()==1&&sets[0].file_name().unwrap().to_string_lossy().ends_with("Z-damaged"),"{sets:?}");
     assert_eq!(fs::read_to_string(sets[0].join("sessions.json")).unwrap(),"{not json","壊れた file を消した・変えた");
     fs::remove_dir_all(&d).ok();
 }
 #[test] fn load_missing_file_has_no_message(){
     let d=tmp_dir("none");let (store,note)=Store::load(&d.join("sessions.json"));
     assert!(note.is_none());assert_eq!(store.sessions.len(),1);fs::remove_dir_all(&d).ok();
 }
 #[test] fn load_valid_json_is_kept(){
     let d=tmp_dir("ok");let p=d.join("sessions.json");let mut s=Store::default();s.sessions[0].name="keep-me".into();s.save(&p).unwrap();
     let (store,note)=Store::load(&p);
     assert!(note.is_none());assert_eq!(store.sessions[0].name,"keep-me");assert!(p.exists());fs::remove_dir_all(&d).ok();
 }
 #[test] fn storage_roundtrip(){let data=Store::default();let encoded=serde_json::to_string(&data).unwrap();let restored:Store=serde_json::from_str(&encoded).unwrap();assert_eq!(restored.sessions.len(),1);}
 #[test] fn old_v03_json_loads_without_new_fields(){
     let data=Store::default();
     let mut value=serde_json::to_value(&data).unwrap();
     let session=value["sessions"][0].as_object_mut().unwrap();
     session.remove("drafts");
     let panel=session.get_mut("panels").unwrap().as_array_mut().unwrap()[0].as_object_mut().unwrap();
     panel.remove("locked");panel.remove("hidden");
     value.as_object_mut().unwrap().remove("ui");
     let restored:Store=serde_json::from_value(value).unwrap();
     assert!(restored.sessions[0].drafts.is_empty());
     assert!(!restored.sessions[0].panels[0].locked);
     assert_eq!(restored.ui,UiSettings::default());
 }
 /// RQ1 interface（保存の形）: RQ1 の前の版が書いた `"ui"`（新しい key が無い）を読むと、RQ1 の欄は全部既定（OS に従う・Home・100%・CLI の既定・env か既定の上限）。
 /// 全部を埋めた値は書いて読み戻すと同じ。負の対照（commit message）: `reopen_last_project` の `#[serde(default)]` を外す → 赤。
 #[test] fn rq1_ui_settings_reads_the_old_file_and_round_trips(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"Dark","rail_pinned":true,"sidebar_hidden":null,"drafts":{}}"#).unwrap();
     assert_eq!(old,UiSettings{theme:ThemeChoice::Dark,rail_pinned:true,..UiSettings::default()});
     assert_eq!((old.reduce_motion,old.reopen_last_project,old.last_project.as_deref(),old.text_zoom,old.default_model.as_deref(),old.default_effort.as_deref(),old.llm_max_calls,old.llm_max_secs),(None,false,None,None,None,None,None,None));
     let full=UiSettings{reduce_motion:Some(true),reopen_last_project:true,last_project:Some("p.a".into()),text_zoom:Some(120),default_model:Some("sonnet".into()),
         default_effort:Some("high".into()),llm_max_calls:Some(0),llm_max_secs:Some(90),..UiSettings::default()};
     let back:UiSettings=serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
     assert_eq!(back,full);
 }
 /// G104 × RQ-N（merge の約束）: G104 の .app が書いた `"effort"` の key を `default_effort` で読み、書く時は `default_effort` の key だけ（古い key を書き戻さない）。
 /// 負の対照: `"effort"` だけの時に `default_effort` へ読み替えない → 1 つ目の assert が赤。
 #[test] fn g104_old_effort_key_reads_into_default_effort(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"System","rail_pinned":false,"sidebar_hidden":null,"drafts":{},"effort":"high"}"#).unwrap();
     assert_eq!(old.default_effort.as_deref(),Some("high"),"前の .app の effort を読めない");
     let text=serde_json::to_string(&old).unwrap();
     assert!(text.contains(r#""default_effort":"high""#) && !text.contains(r#""effort":"#),"{text}");
 }
 /// 古い key と新しい key が両方在る設定は隔離せず、新しい key を採る。知らない段の語も読める。書く時は新しい key だけ。
 /// 負の対照: 両方在る時に古い key を採る → 1 つ目の assert が赤。
 #[test] fn g104_both_effort_keys_keep_the_new_key(){
     let both=serde_json::from_str::<UiSettings>(r#"{"effort":"low","default_effort":"high"}"#);
     assert_eq!(both.as_ref().ok().and_then(|u|u.default_effort.as_deref()),Some("high"),"両方の key を読めない・新しい key を採らない: {both:?}");
     let d=tmp_dir("both-effort");let p=d.join("sessions.json");
     let mut v=serde_json::to_value(Store::default()).unwrap();
     v["ui"]["effort"]=serde_json::json!("low");v["ui"]["default_effort"]=serde_json::json!("high");
     v["ui"]["pinned_projects"]=serde_json::json!(["p.keep"]);
     fs::write(&p,serde_json::to_vec(&v).unwrap()).unwrap();
     let (store,note)=Store::load(&p);
     assert!(note.is_none(),"effort の key が 2 つ在るだけで設定を隔離した: {note:?}");
     assert_eq!((store.ui.default_effort.as_deref(),store.ui.pinned_projects.clone()),(Some("high"),vec!["p.keep".to_string()]));
     let odd=serde_json::from_str::<UiSettings>(r#"{"default_effort":"ultra","default_model":"--dangerously-skip-permissions"}"#).expect("知らない語で設定を読めない");
     let text=serde_json::to_string(&odd).unwrap();
     assert!(!text.contains(r#""effort":"#),"古い key を書き戻した: {text}");
     fs::remove_dir_all(&d).ok();
 }
 /// RQ2 interface（保存の形）: RQ1 の版が書いた `"ui"`（RQ2 の key が無い）を読むと、Project の印は全部空・Recent・System default。
 /// 全部を埋めた値は書いて読み戻すと同じで、account は `kind` の tag で書く（鍵の値も email も無い）。負の対照（commit message）: `ai_active` の `#[serde(default)]` を外す → 赤。
 #[test] fn rq2_ui_settings_reads_the_rq1_file_and_round_trips(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"System","rail_pinned":false,"sidebar_hidden":null,"drafts":{},"reduce_motion":null,"reopen_last_project":false,"last_project":null,"text_zoom":null,"default_model":null,"default_effort":null,"llm_max_calls":null,"llm_max_secs":null}"#).unwrap();
     assert_eq!(old,UiSettings::default());
     let full=UiSettings{pinned_projects:vec!["project.app.a".into()],archived_projects:vec!["project.app.b".into()],sample_projects:vec!["project.app.c".into()],
         opened_projects:BTreeMap::from([("project.app.a".to_owned(),1_761_600_000)]),open_projects:vec!["project.app.a".into()],
         ai_accounts:vec![AiAccount::Plan{id:"plan-1".into()},AiAccount::Key{id:"key-1".into(),last4:"a1b2".into()},AiAccount::EnvKey],ai_active:Some("key-1".into()),..UiSettings::default()};
     let text=serde_json::to_string(&full).unwrap();
     assert!(text.contains(r#"{"kind":"key","id":"key-1","last4":"a1b2"}"#),"account の保存の形: {text}");
     let back:UiSettings=serde_json::from_str(&text).unwrap();
     assert_eq!(back,full);
     assert_eq!(back.ai_accounts.iter().map(AiAccount::id).collect::<Vec<_>>(),["plan-1","key-1","env"]);
 }
 /// RQ3 interface（caller の形）: RQ2 の file（history_days の無い "ui"）も読め、保存期間は往復する。負の対照: `history_days` を `#[serde(skip)]` にする → 保存の形の assert が赤
 /// （`serde(default)` を外すだけでは赤くならない = Option の欄は serde が無い key を None で読む・等価な変異）。
 #[test] fn rq3_ui_settings_reads_the_rq2_file_and_keeps_history_days(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"System","rail_pinned":false,"pinned_projects":[],"ai_accounts":[],"ai_active":null}"#).unwrap();
     assert_eq!(old.history_days,None);
     let full=UiSettings{history_days:Some(7),..UiSettings::default()};
     let text=serde_json::to_string(&full).unwrap();
     assert!(text.contains(r#""history_days":7"#),"保存の形: {text}");
     assert_eq!(serde_json::from_str::<UiSettings>(&text).unwrap(),full);
 }
 /// SESSIONS-LEFT-1101 追記 244 interface（caller の形）: 前の版の file（`folded_headings` の無い "ui"）は空 = 見出しは全部開いた形で読め、畳んだ見出しは往復する。
 /// 負の対照（未実行・owner 186 / 187）: UiSettings の `folded_headings` を `#[serde(skip)]` にする → 保存の形の assert が赤。
 #[test] fn sessions_left_244_ui_settings_keeps_folded_headings(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"System","rail_pinned":false,"open_projects":["project.app.a"]}"#).unwrap();
     assert!(old.folded_headings.is_empty()&&old.open_projects==["project.app.a"]);
     let full=UiSettings{folded_headings:vec!["projects".into()],open_projects:vec!["project.app.a".into()],..UiSettings::default()};
     let text=serde_json::to_string(&full).unwrap();
     assert!(text.contains(r#""folded_headings":["projects"]"#),"保存の形: {text}");
     assert_eq!(serde_json::from_str::<UiSettings>(&text).unwrap(),full);
 }
 /// RQ4 interface（caller の形）: RQ3 の file（web_data_url・disconnected の無い "ui"）も読め、2 つは往復する。負の対照: `disconnected` を `#[serde(skip)]` にする → 保存の形の assert が赤。
 #[test] fn rq4_ui_settings_reads_the_rq3_file_and_keeps_connection_fields(){
     let old:UiSettings=serde_json::from_str(r#"{"theme":"System","rail_pinned":false,"history_days":30}"#).unwrap();
     assert_eq!((old.web_data_url.as_deref(),old.disconnected.len()),(None,0));
     let full=UiSettings{web_data_url:Some("https://api.example.com/items".into()),disconnected:vec!["github".into()],..UiSettings::default()};
     let text=serde_json::to_string(&full).unwrap();
     assert!(text.contains(r#""disconnected":["github"]"#)&&text.contains(r#""web_data_url":"https://api.example.com/items""#),"保存の形: {text}");
     assert_eq!(serde_json::from_str::<UiSettings>(&text).unwrap(),full);
 }
 #[test] fn duplication_rebinds_actions_without_crosstalk(){
     let mut original=Session::starter();
     let action=Action::new("read",ActionKind::GitHubIssues{repo:"a/b".into()});
     let old=action.id;
     original.workflows.push(Workflow{id:new_id(),name:"read".into(),steps:vec![old]});
     original.actions.push(action);
     let copy=original.duplicate();
     assert_ne!(copy.id,original.id);
     assert_ne!(copy.actions[0].id,old);
     assert_eq!(copy.workflows[0].steps[0],copy.actions[0].id);
 }
    /// MIRROR-SPEED-1101: Mirror smoothness は既定 Auto で保存の形に出ず、選んだ値は読み戻る・前の版の file（key 無し）は Auto・helper の語は 3 つ。
    /// 負の対照: `skip_serializing_if` を外す → 既定の保存に `mirror_smoothness` が出て赤。
    #[test]
    fn mirror_speed_smoothness_defaults_to_auto_and_round_trips() {
        let ui = UiSettings::default();
        assert_eq!(ui.mirror_smoothness, MirrorSmoothness::Auto);
        assert!(serde_json::to_value(&ui).unwrap().get("mirror_smoothness").is_none(), "既定の Auto を保存の形に書いた");
        let saver = UiSettings { mirror_smoothness: MirrorSmoothness::Saver, ..UiSettings::default() };
        let v = serde_json::to_value(&saver).unwrap();
        assert_eq!(v["mirror_smoothness"], "saver");
        assert_eq!(serde_json::from_value::<UiSettings>(v).unwrap().mirror_smoothness, MirrorSmoothness::Saver);
        assert_eq!(serde_json::from_str::<UiSettings>("{}").unwrap().mirror_smoothness, MirrorSmoothness::Auto, "前の版の file が Auto で読めない");
        assert_eq!([MirrorSmoothness::Auto, MirrorSmoothness::Smooth, MirrorSmoothness::Saver].map(MirrorSmoothness::arg), ["auto", "smooth", "saver"]);
    }
    /// ACCOUNTS-1101 2 段目（interface・呼ぶ側の形）: 名前は `<kind>/<id>` の key で付き、空白で外れ、足した account でない行と列に無い account には付かない・40 字で切る・
    /// 往復する・前の版の file は空。負の対照（commit message）: `set_account_name` の trim を外す → `"  work  "` のままで赤・`skip_serializing_if` を外す → 既定の保存に key が出て赤。
    #[test]
    fn accounts2_names_are_kept_by_kind_and_id_and_cleared_when_empty() {
        let plan = AiAccount::Plan { id: "plan-1".into() };
        let grok = AiAccount::Plan { id: "grok-2".into() };
        let mut ui = UiSettings { ai_accounts: vec![plan.clone(), grok.clone(), AiAccount::EnvKey], ..UiSettings::default() };
        assert!(serde_json::to_value(&ui).unwrap().get("account_names").is_none(), "名前の無い保存に key を書いた");
        assert!(ui.set_account_name(&plan, "  work  "));
        assert_eq!(ui.account_name(&plan), Some("work"));
        assert_eq!(ui.account_names.get("plan/plan-1").map(String::as_str), Some("work"), "key が種類と id の組でない");
        assert!(!ui.set_account_name(&plan, "work"), "同じ名前で変わったと言った");
        assert!(!ui.set_account_name(&AiAccount::EnvKey, "env"), "足した account でない行に名前を付けた");
        assert!(!ui.set_account_name(&AiAccount::Plan { id: "plan-9".into() }, "x"), "列に無い account に名前を付けた");
        assert!(ui.set_account_name(&grok, &"g".repeat(60)));
        assert_eq!(ui.account_name(&grok).map(|n| n.chars().count()), Some(40), "長い名前を切らない");
        let back: UiSettings = serde_json::from_str(&serde_json::to_string(&ui).unwrap()).unwrap();
        assert_eq!(back.account_names, ui.account_names, "名前が往復しない");
        assert!(ui.set_account_name(&plan, "   "));
        assert_eq!(ui.account_name(&plan), None, "空白で外れない");
        assert!(serde_json::from_str::<UiSettings>("{}").unwrap().account_names.is_empty());
    }
 #[test] fn write_gate(){assert!(!ActionKind::GitHubIssues{repo:"a/b".into()}.is_write());assert!(ActionKind::Desktop{pid:1,window_id:2,label:"OK".into(),role:"button".into(),tool:DesktopTool::Click,text:"".into(),app_name:String::new()}.is_write());}
}
