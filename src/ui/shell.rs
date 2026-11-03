//! 殻（DESIGN.md §1.1 / §8）: View・Agent rail の開閉・幅の 3 段・主領域の差し込み口・実行 strip。
//! UI-002（殻の寸法）/ UI-010（View の枠）/ UI-011（提案の入口は header の `✦`）。
//! 他 unit が使う公開 API: `View`, `Tier::for_width`, `Shell`, `Intent`, `AppRow`, `draw_app_region`。

use crate::domain::icon::Icon;
use super::agent_rail::ProposalAct;
use crate::domain::chat::{Accepts, Attachment, Effort};
use super::motion;
use super::palette::Palette;
use super::theme::{self, Type};
use eframe::egui;
use std::collections::BTreeMap;

/// 主領域に何を出すか。`App(id)` は AppDefinition v3 の id（renderer unit R が描く）。W2 からは古い入口: `Intent::Go(App(id))` は
/// `Generated::project_view_of(id)` で包んだ Project の先頭 Page（`Project{..}`）に変換してから開く。
/// `Project{project, page}` は Project の id（`ProjectDefinitionV1::id`）と、今いる Page の id（変わらない id・route ではない）。
/// `CompatCanvas(session_id)` は旧 canvas（v0.4 の Session）を互換 view として出す。
/// 契約（UI-4a から）: `Apps` は消した（variant・`NAV` の 1 つ〈`[View; 3]` = Home・Runs・Connections〉・`nav_label` / `nav_icon` の腕・`SHIBUBU_START` の `apps`・
/// 起動の骨格の腕）。`Go(View::Apps)` だった行き先（消えた Project / app の画面・旧 canvas の消えた画面）は `Go(View::Home)`、button の文は表の `Go to Home`
/// （copy.en.md:87・:497・additions.md:63・G14 B8 の 63）。Project の一覧は Home の `All projects · {n}` と ⌘K。保証しない: 一覧の絞り込み（Apps の欄は消えた・⌘K が代わり）。
/// 契約（RQ1 から）: variant `Settings(SettingsSection)` を足した（主領域を Settings の画面にする・`crate::settings::draw` が描く）。入口は 3 つで、どれも
/// `Intent::Go(View::Settings(SettingsSection::General))`: macOS の app menu の `Settings…`（`surface::AppKitOp::InstallSettingsItem`）・⌘,・⌘K の `Settings` の行
/// （`palette::commands`）。`NAV` には入れない（sidebar の nav は 3 行のまま・監査 C「既存Home・Runs・Connectionsの主要navを増やさない」）。
/// 例: `shell.go(View::Settings(SettingsSection::About))`。保証する: 1 つの欄・placeholder・AX の名前・Enter の結果は Runs と同じ（command を探す欄）・
/// `place()` は `"search"`（打ちかけは Runs と同じ探す欄の key）。保証しない: 節の中の scroll の位置を覚える事（毎回頭から）。
/// Wave: UI-4a, RQ1
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Home,
    Runs,
    Connections,
    App(String),
    Project { project: String, page: String },
    CompatCanvas(u64),
    Settings(SettingsSection),
}

/// RQ1: Settings の左の list の節（監査 C の S1 General・S2 AI & Models・S7 About。主領域の左に小さな list・右に form・card なし）。
/// `View::Settings` の中身で、list の行を押すと `Intent::Go(View::Settings(節))`。list の 4 行目 `Connections` は節でなく `Intent::Go(View::Connections)`
/// （接続の作成・修復は Connections の画面・Settings と二重にしない = 監査 C）。後の Wave が variant を足す: RQ3 = `Data`（Data & Recovery）・
/// RQ6 = `Privacy`・`Automation`（7 行は RQ6 の後に揃う）・PV-022 = Software & Permissions。まだ作っていない節は variant も行も無い（押しても何も無い行を出さない）。
/// 契約（RQ3 から）: variant `Data`（label `Data & Recovery`・監査 S6）を足し、`ALL` は 4 つ（list の上から General・AI & Models・〈Connections の行〉・Data & Recovery・About）。
/// `crate::settings::draw` の list は `Connections` の行と `About` の間に `Data & Recovery` の行（RQ3 の本体で足した）。
/// 例: `for s in SettingsSection::ALL { if row(ui, s.label(), s == now) { out = Some(Intent::Go(View::Settings(s))) } }`
/// 保証する: label は英語・sentence case（表の語）。保証しない: 節の並びの保存（毎回 `ALL` の順）。
/// 新しくした理由: 既存の型に Settings の節の並びを持つ物が無い（`View` は場所・`RailTab` は右の列の tab で、どちらにも入れると別の画面の match が増える）。
/// Wave: RQ1, RQ3
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)] // 作り手の `crate::settings` は ui だけを `#[path]` で引く試験の crate に入らない（bin では全部使う）
pub enum SettingsSection {
    General,
    AiModels,
    /// 段 2 作り直し（owner 142）: Install CLI & skill の状態と `Remove`（`crate::settings::draw` の doc・Terminal の panel の 1 行目と同じ部品）。
    Terminal,
    /// RQ3（RQ-010〜RQ-014）: 保存先・Back up / Restore・Import・Shibubu が残す物と保存期間・Delete my data（`crate::settings::draw` の doc）。
    Data,
    /// RQ6（RQ-022）: AI に任せてよいと押した app の一覧と `Revoke`（`crate::settings::draw` の doc）。
    Privacy,
    About,
}

#[allow(dead_code)] // 同上（読み手の `crate::settings` が無い試験の crate）
impl SettingsSection {
    /// list の上からの順（`Connections` の行は `AiModels` と `Terminal` の間に `crate::settings::draw` が置く）。
    /// Wave: RQ1, RQ3
    pub const ALL: [SettingsSection; 6] = [SettingsSection::General, SettingsSection::AiModels, SettingsSection::Terminal, SettingsSection::Data, SettingsSection::Privacy, SettingsSection::About];
    /// list の行の語（AX の名前も同じ）。
    /// Wave: RQ1
    pub const fn label(self) -> &'static str {
        match self {
            SettingsSection::General => "General",
            SettingsSection::AiModels => "AI & Models",
            SettingsSection::Terminal => "Terminal",
            SettingsSection::Data => "Data & Recovery",
            SettingsSection::Privacy => "Privacy",
            SettingsSection::About => "About",
        }
    }
}

/// RQ3（RQ-013 Delete my data・RQ-014 Data retention）: Shibubu がこの Mac に残す物の種類。Settings › Data & Recovery の「What Shibubu keeps」の 1 行ずつ
/// （語・大きさ・保存期間・`Delete…` = 行内の確認 = `Shell::data_confirm`）と、`Intent::DeleteMyData` の宛先。
/// 例: `for k in DataKind::ALL { row(ui, k.label(), size_of(k)); if confirm == Some(k) { confirm_inline(ui, k.effect()) } }`
/// 各種類が持つ物（消す物 = `Generated::delete_my_data` の doc・保存期間 = `Generated::prune_records` の doc）:
/// - `Projects`: Project と Page と版（DB の projects・apps〈CASCADE〉・terminal_sessions・run_receipts）・面の中身・Project ごとの web の store・録音（`takes`）
/// - `History`: Run と Engine と AI の呼び出しの記録・開いた日（DB の runs と evidence・`engine-runs.jsonl`・`engine-trace.jsonl`・`llm-calls.jsonl`・`usage.jsonl`）
/// - `ScreenRecords`: 画面と入力の記録（`mirror-relay.jsonl`・`gui-acts.jsonl`・`overlay.jsonl`・`overlay-identity.jsonl`。打った字そのものは元から持たない）
/// - `Consents`: AI に任せてよいと押した app（`gui-act-consents.jsonl`）と Mirror の同意（`ui_state` の同意の行）。保存期間で消さない（消すのは利用者だけ）
/// - `Connections`: Shibubu が Keychain に置いた鍵（`ai-key.*`）・足した AI の login（`ai-accounts/`）・Browser の面の共有の web の store（sign-in と、その store の
///   site の data = G72 より前に Diagram で描いた図も。`effect` がそう言う）。GitHub の token は Shibubu が持たない（env）ので入らない
/// - `Backups`: `backups/` の世代（`Generated::back_up_now` の doc）
/// 保証する: label は英語・sentence case・利用者の語（file の名前や env を出さない）。保証しない: Shibubu の外に在る物（利用者の folder と file・元の app の data・
/// Claude Code の `~/.claude`・site 側に置いた物）を種類に入れる事（消さない・`effect` が言う）。
/// 新しくした理由: 既存の型は DB の表だけ（`storage::Table`）で、file の記録・Keychain・web の store・同意の file を 1 つの宛先として持つ型が無い。
/// Intent に載るので ui に置く（`crate::storage` は ui だけを引く試験の crate に無い）。
/// Wave: RQ3
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)] // 読み手の `crate::settings` と generated が無い試験の crate（ui だけを `#[path]` で引く）
pub enum DataKind {
    Projects,
    History,
    ScreenRecords,
    Consents,
    Connections,
    Backups,
}

#[allow(dead_code)] // 同上
impl DataKind {
    /// 「What Shibubu keeps」の上からの順。
    /// Wave: RQ3
    pub const ALL: [DataKind; 6] = [DataKind::Projects, DataKind::History, DataKind::ScreenRecords, DataKind::Consents, DataKind::Connections, DataKind::Backups];
    /// 行の語（AX の名前も同じ・`Delete…` の AX は `Delete {label}…`）。
    /// Wave: RQ3
    pub const fn label(self) -> &'static str {
        match self {
            DataKind::Projects => "Projects and pages",
            DataKind::History => "Run history and AI calls",
            DataKind::ScreenRecords => "Screen and input records",
            DataKind::Consents => "Apps you allowed",
            DataKind::Connections => "Sign-ins and keys",
            DataKind::Backups => "Backups",
        }
    }
    /// 行内の確認に出す影響の 1 行（何が消え・何が残り・後で何をやり直すか）。`theme::confirm_inline` の本文。
    /// Wave: RQ3
    pub const fn effect(self) -> &'static str {
        match self {
            DataKind::Projects => "Every project, its pages and history are deleted. Your folders and files stay",
            DataKind::History => "Runs, AI calls and the usage report start empty. Projects stay",
            DataKind::ScreenRecords => "Records of what Shibubu saw and sent to apps are deleted. Projects stay",
            DataKind::Consents => "You'll be asked again before AI or Mirror works in each app",
            DataKind::Connections => "You'll sign in again and paste keys again. What Browser pages kept is deleted too, including diagrams drawn before each project had its own. Your Claude Code sign-in stays",
            DataKind::Backups => "You can't restore from them afterwards. Your current data stays",
        }
    }
}

impl View {
    /// sidebar の nav 3 行（順序は §1.2・UI-4a で Apps を消した）。
    pub const NAV: [View; 3] = [View::Home, View::Runs, View::Connections];

    pub fn nav_label(&self) -> &'static str {
        match self {
            View::Home => "Home",
            View::Runs => "Runs",
            View::Connections => "Connections",
            View::Settings(_) => "Settings",
            View::App(_) | View::Project { .. } | View::CompatCanvas(_) => "App",
        }
    }
    /// ICONS-1101: 左の nav の icon（`sidebar::nav_row` が `theme::paint_icon` で描く）。
    pub fn nav_icon(&self) -> Icon {
        match self {
            View::Home => Icon::House,
            View::Runs => Icon::History,
            View::Connections => Icon::Plug,
            View::Settings(_) => Icon::Settings,
            View::App(_) | View::Project { .. } | View::CompatCanvas(_) => Icon::AppWindow,
        }
    }
    pub fn is_app(&self) -> bool {
        matches!(self, View::App(_) | View::Project { .. } | View::CompatCanvas(_))
    }
    /// 1 つの欄の placeholder（場所だけで決まる・表 docs/ui-copy/copy.en.md:26・:28・:30・G14 B4）。呼ぶ側: header の欄の `hint_text(shell.view.field_hint())`。
    /// 保証する: Home = `What do you want to create?`・Project = `Ask, or say what to change, e.g. add a Tests page`（CHAT-ONE-1101: 会話の chat bar）・Runs / Connections / CompatCanvas / App =
    /// `Search commands and projects`。英語・表の EN そのまま。保証しない: 打った文や busy で変わる事（busy の 1 行は Home と Page の上の今の行のまま）。
    /// Wave: UI-4a
    pub fn field_hint(&self) -> &'static str {
        match self {
            View::Home => "What do you want to create?",
            View::Project { .. } => "Ask, or say what to change, e.g. add a Tests page",
            View::Runs | View::Connections | View::Settings(_) | View::App(_) | View::CompatCanvas(_) => "Search commands and projects",
        }
    }
    /// 1 つの欄の AX の名前（表 copy.en.md:27・:29。BG の driver は labels.py の `goal` / `change` で引く）。呼ぶ側: `theme::ax_name(field, shell.view.field_name())`。
    /// 保証する: Home と Project = `What to create`（CHAT-ONE-1101: chat bar は 1 つの部品 = 欄の名前も Home と同じ・labels.py の `goal` / `change`）・
    /// Runs / Connections / CompatCanvas / App = `Search commands and projects`（表に名前の行が無いので placeholder と同じ）。
    /// 保証しない: AX の role（singleline の TextEdit = AXTextField。labels.py の `goal` の role は UI-4a で AXTextArea → AXTextField にした）。
    /// Wave: UI-4a
    pub fn field_name(&self) -> &'static str {
        match self {
            View::Home | View::Project { .. } => "What to create",
            View::Runs | View::Connections | View::Settings(_) | View::App(_) | View::CompatCanvas(_) => "Search commands and projects",
        }
    }
    /// `Shell::drafts` の key = 欄ごとに 1 つ（Home の欄 = `"home"`・Project の Chat の欄 = Project の id・
    /// 他の View の `Search commands and projects` = `"search"`）。DOGFOOD-X 周 2 回 8: Home と探す欄が同じ key で、Connections の欄に Home の未送信の頼みが入り、
    /// 打った語がその後ろに付いた → 欄ごとに分けた。
    pub(crate) fn place(&self) -> &str {
        match self {
            View::Home => "home",
            View::Project { project, .. } => project,
            View::Runs | View::Connections | View::Settings(_) | View::App(_) | View::CompatCanvas(_) => "search",
        }
    }
}

/// §8 の 3 段（窓の最小幅が 960 なので 960 未満の drawer 段は持たない）。
/// UI-4a で field `input_w`（1 つの欄の幅）・`location_w`（場所の名前の最大幅）を足した。例: `let t = Tier::current(ctx); t.input_w`。
/// 保証する: `input_w`・`location_w` は段の中では同じ値（窓の幅が段の中で動いても欄の rect は動かない）。
/// UI-4b で field `sidebar_hidden`（その幅の畳みの既定）と `Tier::sidebar_open` を足した。例: `let open = Tier::current(ctx).sidebar_open(store.ui.sidebar_hidden);`。
/// 契約（UI-4b）: `sidebar` は開いた時の幅 232 / 216 / 216（< 1200 も `SIDEBAR_W_MID`）。64 の icon の列（`sidebar_compact`・`theme::SIDEBAR_W_COMPACT`）は消した
/// （畳む口が出来たので狭い窓の icon の列は要らない・sidebar.md §2）。
/// L1-NORAIL で右の rail の幅（`rail`・`rail_overlay`）を消した（会話は下の panel の Chat の tab）。保証しない: title line の Git の幅（透明な title bar と一緒に後）。
/// Wave: UI-4a, UI-4b
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tier {
    /// 開いた時の sidebar の幅（畳んだ時の 0 は `sidebar::draw` が `motion::animate` で寄せる・この値は変えない）
    pub sidebar: f32,
    /// この幅で初めて開いた時に sidebar を畳んでおくか（owner-addendum ②「狭い窓は畳んで始まる」）: ≥1200 は false・< 1200 は true。
    /// 人が一度でも畳む / 戻すと `model::UiSettings::sidebar_hidden` が `Some` になり、この既定は読まれない（`Tier::sidebar_open`）。
    pub sidebar_hidden: bool,
    pub gutter: f32,
    /// 1 つの欄の幅（Project・Runs・Connections で同じ = 決定 1。Home は主領域の縦の中央でこれ以上の幅・G83）。段の下限の窓の幅 W に SPEC §2 の式
    /// `min(input-max-w, 2·(W/2 − 78 − 28 − location_w − 8))`（input-max-w 420 / 480 / 560・78 = 窓の button・28 = sidebar を畳む button〈owner-addendum ②・sendback 9〉）
    /// を当てた 276（960）/ 396（1200）/ 516（1440）。段の中では同じ値（x は窓の中央なので幅が同じなら rect も同じ）。
    /// 呼ぶ側: header の `Rect::from_center_size(header の rect の中央, vec2(tier.input_w, theme::COMMAND_H))`・Home の欄の幅の下限（`home::draw`）。
    /// 保証しない: 窓の button の実の inset（UI-4a は今の 56 の header の panel で、透明な title bar ではない・78 は後で title bar にしても幅が変わらないよう式に残す）。
    pub input_w: f32,
    /// title line の左の場所の名前の最大幅（SPEC §12 の location-max-w 200 / 260 / 320）。長い名前は切り、全文は tooltip。欄の幅の式に入るので欄と重ならない。
    pub location_w: f32,
}

impl Tier {
    pub fn for_width(width: f32) -> Tier {
        if width >= 1440.0 {
            Tier { sidebar: theme::SIDEBAR_W, sidebar_hidden: false, gutter: theme::GUTTER_WIDE, input_w: 516.0, location_w: 320.0 }
        } else if width >= 1200.0 {
            Tier { sidebar: theme::SIDEBAR_W_MID, sidebar_hidden: false, gutter: theme::GUTTER_WIDE, input_w: 396.0, location_w: 260.0 }
        } else {
            Tier { sidebar: theme::SIDEBAR_W_MID, sidebar_hidden: true, gutter: theme::GUTTER_MID, input_w: 276.0, location_w: 200.0 }
        }
    }
    pub fn current(ctx: &egui::Context) -> Tier {
        Tier::for_width(ctx.screen_rect().width())
    }
    /// この frame で sidebar を開いて描くか。`saved` = `model::UiSettings::sidebar_hidden`（`None` = 人が一度も畳む / 戻すをしていない）。純関数。
    /// 例（app.rs の update の頭・毎 frame）: `self.shell.sidebar_open = Tier::current(ctx).sidebar_open(self.store.ui.sidebar_hidden);`
    /// 保証する: `Some(b)` なら `!b`（人の選択を窓の幅より先に覚える・広げ縮めしても追従しない）・`None` なら `!self.sidebar_hidden`
    /// （幅 900 で初めて起動すると畳む・窓を 1100 に縮めると畳む・1200 以上に広げると開く）。
    /// 保証しない: 起動の `ShibubuApp::new` の時に正しい事（その時の ctx は窓の幅を知らない = `screen_rect` が既定の 10000 幅 → 必ず毎 frame の頭で読む）・
    /// `None` に戻す口（作らない・誰も要らない設定）。
    /// Wave: UI-4b
    pub fn sidebar_open(self, saved: Option<bool>) -> bool {
        !saved.unwrap_or(self.sidebar_hidden)
    }
}

/// sidebar / Home / Apps / palette が共有する 1 行分のアプリ情報（Session から app.rs が作る）。
/// 契約（RQ2 から）: field `needs`・`pinned`・`archived`・`opened_at`・`sample` を足した（Project を管理する印・作る所は `Generated::rows` と app.rs の `rows`）。
/// 例: `AppRow { needs: 0, pinned: false, archived: false, opened_at: None, sample: false, ..row }`。保証する: 印は全部写し（正は `UiSettings` と run の記録）・旧 canvas の行は全部既定。
/// 保証しない: 並びの意味（並べるのは `sidebar::project_order`）。
/// Wave: RQ2
#[derive(Clone, Debug, PartialEq)]
pub struct AppRow {
    /// 開く先（旧 Session なら `CompatCanvas(id)`、v3 なら `App(id)`）
    pub view: View,
    pub name: String,
    /// 1 行の目的（prompt）。空なら名前だけ。
    pub purpose: String,
    /// readiness の文言（states.rs の wording か、パーツ数などの実状態）
    pub status: String,
    pub running: usize,
    /// RQ2（RQ-006・`1 needs you`）: この Project で人の手を待つ数（承認待ち・送ったか確かめられない・接続が要る run と Engine の承認待ち）。
    /// 書き手 = `Generated::rows`（`run_entries` の `RunResult::{NeedsApproval, Unknown, NeedsConnection}` と `engine_views` の承認待ちを Project ごとに数える）・旧 canvas は 0。
    /// 読み手 = `⌄` の list の印と location の右の `{n} needs you`（今の Project 以外の合計・0 なら描かない）。
    pub needs: usize,
    /// RQ2（RQ-007）: Continue の上に留めた（`UiSettings::pinned_projects` の写し・app.rs が詰める・2 つ目の真実にしない）。
    pub pinned: bool,
    /// RQ2（RQ-005）: Archive した（`UiSettings::archived_projects` の写し）。Continue と sidebar は既定で描かない・⌘K は描く。
    pub archived: bool,
    /// RQ2（RQ-007）: 最後に開いた時刻（unix 秒・`UiSettings::opened_projects` の写し）。None = この Mac で開いた記録が無い（作った順の後ろ）。
    pub opened_at: Option<i64>,
    /// RQ2（RQ-009）: Home の `Try` から作った（`UiSettings::sample_projects` の写し）。行の status の前に `Sample`。
    pub sample: bool,
}

impl AppRow {
    /// 行の Project の id（生成した Project の行だけ・旧 canvas の行は None = `⌄` の list と `•••` に出さない）。SESSIONS-LEFT-1101: app.rs も引く（sidebar の Page）。
    pub fn project_id(&self) -> Option<String> {
        match &self.view {
            View::App(app) => Some(format!("project.{app}")),
            View::Project { project, .. } => Some(project.clone()),
            _ => None,
        }
    }
}

/// 画面の部品が app.rs に返す「やりたいこと」。app.rs が `apply` で状態を変える。
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Go(View),
    /// 空のアプリを 1 つ作って互換 view で開く。契約（UI-4a から・sendback 17 / 35.10）: 作る口は ⌘K の `New blank canvas` の行だけ
    /// （⌘N・⌘K の `New project` は `Go(View::Home)` = Home の欄に focus・sidebar の `New chat` は `NewChat`）。
    /// 契約（FX4〈FX-017〉）: 本体で ⌘K の `New blank canvas` の行を外すので作る口が 0 になる（`check_wired` の「作る側なし」+1）。variant と
    /// `new_blank_session` は互換 canvas ごと FX-059（RULE-040・Refactor の段）で消す（ここで消すと旧 canvas の消す / 複製の道と同時に触る）。
    /// Wave: FX4
    NewApp,
    /// 一言から composer で app を作って `View::App(id)` で開く。契約（UI-4a から）: 作る口 = どこでも ⌘↵・Home の `Try` の行の例・palette の fallback。
    /// L2（109 通目 15）: Home の Enter（名前が同じ Project が無い時）はここを通さない（`Talk`）。
    /// G104: 2 つ目は一言に添えた物（Home の欄の token・`Shell::attach` の写し）。Home の外の ⌘↵・例・palette は空
    Generate(String, Vec<Attachment>),
    /// L2（109 通目 15）: Home の Enter は Project を作らない。一言を Home の会話にする。
    /// 処理: 人の Turn と、答え（CHAT-FREE-1101: 形は AI が決める Markdown）・選択肢（AI が要ると思った時だけ・数は AI 次第）・案を `Shell::talk` に足す。作り手 = `Shell::enter_intent(false)` の Home の名前不一致。
    Talk(String),
    /// CHAT-FREE-1101: Home の会話の答えのチェックリストの 1 行を押した（turn の位置・文の頭からのチェックの番号）。
    /// 処理: app.rs が `markdown::toggle` で `Shell::talk[turn].text` の `[ ]` / `[x]` を替える（次の一言で AI も読む）。作り手 = `home::talk_thread`。
    CheckItem(usize, usize),
    /// L2（111 通目・handoff 05「Proposal → Preview → Build now → Build」）: 会話の案の card の手。どれも最後の Shibubu の `proposal` に効き、案が無ければ何もしない。作り手 = Home の会話の card。
    /// `PreviewPlan` = 読むだけの行を `Turn::preview` に入れる（何も書かない・AI も呼ばない）。`ChangePlan` = 案の文を欄に入れて focus（案は残す）。
    /// `LaterPlan` = 案を閉じる（`proposal` と `preview` を空に・会話は残す）。`AcceptPlan` = その文で `create_app` を通す（Home から作る唯一の道）。
    PreviewPlan,
    ChangePlan,
    LaterPlan,
    AcceptPlan,
    /// L2: 会話を閉じて Home の初めの形（Try）へ（読んでいる AI の答えは捨てる）。作り手 = 左の sidebar の一番上の `New chat`
    /// （SESSIONS-LEFT-1101: header の右から移した・どの View からも Home へ移る）。前の会話は保存したまま左の一覧に残る。
    NewChat,
    /// SESSIONS-LEFT-1101: 左の session の行を開く（key = `Generated::chat_list` の key）。Home の session（`chat::HOME_CHAT` の key）= Home に移り `Shell::talk` にその会話を読む・
    /// Project の session = その Project を開き、その session を今の session に（`Generated::pick_session`）、下の panel の Chat の tab を開く。作り手 = `sidebar::draw` の session の行。
    /// Wave: SESSIONS-LEFT-1101
    OpenSession(String),
    /// SESSIONS-LEFT-1101 追記 224（owner「projectsのtermianal, chat, appごとにアイコンを…」）: 左の sidebar の Project の中の Terminal の行 = その Project を開き、下の panel のその tab を選んで開く。
    /// 作り手 = `sidebar::draw` の Terminal の行。無い tab は何もしない（`PanelAct::Select` と同じ）
    /// Wave: SESSIONS-LEFT-1101
    OpenTerminal { project: String, tab: String },
    /// CHAT-SIDE-1101: Project の panel（Chat と Terminal の tab）の置き場を替える（`UiSettings::chat_place` に保存・全 Project 共通）。作り手 = header の右端の ◨（右 / 下）・
    /// 右に出している間の ⇧⌘P（下へ）・Settings › General › Chat（Bottom / Right）。追記 212: 下の帯の ⇨ と右の帯の ⇩ は無い・追記 219: 右の帯の › と畳んだ帯の ‹ も無い。
    /// panel ごと移し（選んでいる tab はそのまま）、移した先で開く。Chat の席なら chat bar の欄に focus（会話と下書きは同じ物）。
    /// Wave: CHAT-SIDE-1101
    ChatPlace(crate::model::ChatPlace),
    /// SESSIONS-LEFT-1101（owner 198 の 4）: その Project に空の session を作って開く（Project を開き、下の panel の Chat の tab）。作り手 = sidebar の Project の行の右端の `＋`（owner 209・210・AX `New session in {name}`・追記 244 で hover と Tab の focus の時だけ出る = `sidebar::row_actions`）。
    /// Wave: SESSIONS-LEFT-1101
    NewSession(String),
    /// G104 → HOME-ATTACH-1101: `+` の menu の `Add files or photos`（macOS の選ぶ窓を裏で開く・file・写真・zip・folder をいくつでも・選んだ物は `chat::attach` を通して `Shell::attach` へ）
    Attach,
    /// G104: `Effort ▸` の段（None = Default）。処理: `UiSettings::default_effort`・`Generated::effort`・`Shell::effort` に書く（次の呼び出しから・読んでいる job は変えない）
    SetEffort(Option<Effort>),
    /// G84: `Model ▸` の別名（`domain::chat::MODELS` の語・None = Default = `--model` を付けない）。処理: `UiSettings::default_model`・`Generated::model`・`Shell::model` に書く
    /// （次の呼び出しから・読んでいる job は変えない）。Provider ▸ の行は今ある `UseAiAccount`。
    SetModel(Option<String>),
    /// L1-NORAIL: ⇧⌘P と ⌘K の `Show chat`: Project の下の panel の Chat の tab を選んで開き、欄に focus（Project の外は何もしない = 会話は Home の欄）。
    /// 処理: app.rs が `Generated::panel_act(project, PanelAct::Chat)`。
    OpenChat,
    /// PANEL-SYNC-1101（owner 248「右sidebarと…は同期されていてしたから出すと右sidebarは畳まるように」）: 下から開く = 置き場を下に（`ChatPlace::Bottom`・右の sidebar は
    /// 同じ panel なので畳まれる）にして、今の場所の panel（Project = その Project・Home / Runs / Connections / Settings = Home の Terminal）に `act` を当てる。
    /// 作り手 = 窓の一番下の帯の畳んだ tab（`terminal_panel::Folded`・Chat = `PanelAct::Chat`・Terminal = `Select`・＋ = `Add` / `AddAgent` / `NewBrowserTab`）と畳んだ間の ⌃`。
    /// `ChatPlace` に入れない理由: あちらは置き場だけを替えて今の tab を出す（◨・Settings）・こちらは押した tab で開く。
    /// Wave: PANEL-SYNC-1101
    OpenBelow(crate::ui::terminal_panel::PanelAct),
    /// sidebar を畳む / 戻す（owner-addendum ②・sendback 10）。作り手 3 つ（どれも同じ variant・UI-4b の本体で足す）:
    /// title line の左端の icon button（`Shibubu` の前・作る順 = Tab の頭・tooltip は表の `Hide sidebar · ⌃⌘S` / `Show sidebar · ⌃⌘S`〈additions.md:302-303〉・
    /// AX の名前は ` · ` の前の語）・⌃⌘S（app.rs `keyboard()` の頭に独立の文で `consume_key(egui::Modifiers::CTRL | egui::Modifiers::MAC_CMD, egui::Key::S)` =
    /// ⌘ の pattern より先・隣の ⌃` の文と混ぜない = `a2_sidebar_key`）・⌘K の行 `Hide sidebar` / `Show sidebar`（hint `⌃⌘S`・rail の行の隣）。
    /// 処理（app.rs `apply`）: `let open = !self.shell.sidebar_open; self.store.ui.sidebar_hidden = Some(!open); self.shell.sidebar_open = open; self.mark();`
    /// 保証する: 押すたびに開閉が逆になり、選択は `UiSettings::sidebar_hidden` に書かれて再起動の後も同じ（`sessions.json` の debounce の保存・`rail_pinned` の先例）。
    /// 欄の rect・View・Project の revision・`ui_state` を変えない。失敗の種類は無い（保存の失敗は今の `Couldn't save your changes · {e}` の 1 行のまま）。
    /// 保証しない: 窓ごとの記憶（窓は 1 つ・窓が増えたら `UiSettings` から窓の設定へ移す）・Reduce Motion の fade（UI-6a・UI-4b は幅の animate 180ms）。
    /// Wave: UI-4b
    ToggleSidebar,
    RecheckGui,
    /// FX5: GitHub の行の `Check again`（`gh auth token` → `GET /user` を読み戻し直す）。処理: `ShibubuApp::probe_github`。
    RecheckGithub,
    /// G99: AI（Claude Code）を探し直す（Home の AI の 1 行の `Check again`）。処理: app.rs `start_llm_detect`（裏・待たない）。
    RecheckAi,
    /// G99: AI の 1 行の次の一手（`llm_cli::INSTALL_STEP` = 公式の install の頁を開く・`SIGN_IN_STEP` = `claude auth login` を裏で起こして終われば探し直す）。
    AiStep(&'static str),
    CloneApp,
    DeleteApp,
    SetTheme(egui::ThemePreference),
    Proposal { id: String, act: ProposalAct },
    /// Project の画面で押した Page の操作（今いる Project に当てる）。Page への移りは `Go(View::Project{..})` を使う。
    Page(PageIntent),
    /// USAGE-1: 使った記録を clipboard へ（作り手は ⌘K の `Copy usage report` の 1 行だけ）。処理（app.rs `apply`）:
    /// `ctx.copy_text(self.generated.usage_report()); self.message = "Usage report copied".into();`
    /// 保証する: View・Project・保存を変えない。文に Project の名前・path・打った文を入れない（`Generated::usage_report`）。保証しない: 送る事（利用者が自分で貼る）。
    /// Wave: USAGE-1
    CopyUsageReport,
    /// FX4（FX-016・UIUX N17「local history opt-in, dedup」）: ⌘K の最近の行を覚える（true）/ 覚えない（false）。既定は覚えない（`Palette::recent` = None）。
    /// 作り手: ⌘K の 1 行（`palette::commands` の ③ = `Remember recent commands` / `Forget recent commands`）。Settings の General の行は RQ1 の merge の後の双子。
    /// 処理（app.rs `apply`）: `self.shell.palette.recent = on.then(Vec::new); self.mark();`（false は覚えた行も消す・次の保存で `UiSettings::recent_commands` も None）。
    /// 例: `Intent::RememberCommands(true)`。保証する: View・欄の文・打ちかけを変えない。保証しない: 保存期間（RQ-014）。
    /// Wave: FX4
    RememberCommands(bool),
    /// FX1（FX-001）: 終わる前に保存し、保存できなければ黙って終わらない。作り手 3 つ: 窓の閉じる button（egui の `close_requested`）・
    /// ⌘Q / Dock の Quit（main.rs `ask_before_quit` が立てる印）= `Quit { anyway: false }`、作業の場所の頭の 1 行の `Quit anyway`（`theme::quit_inline`）= `Quit { anyway: true }`。
    /// 処理（app.rs `apply`）: `anyway` でなければ `save_now()`。Ok → 窓を閉じる（`ViewportCommand::Close`）・Err(理由) → `CancelClose` を送り、
    /// `quit_failed = Some(理由)` = 同じ場所に `Couldn't save your changes · {理由}` と `Quit anyway` / `Keep open`。`Keep open`（Esc も）は 1 行を消すだけ。
    /// 例: `if ctx.input(|i| i.viewport().close_requested()) { self.apply(ctx, Intent::Quit { anyway: false }) }`
    /// 保証する: 保存できない時に 1 回は必ず止まって聞く・`Keep open` で View・欄の文・打ちかけ・Page を変えない・保存できている時は何も出さない。
    /// 保証しない: SIGKILL・電源断・crash の時の保存（終わる道を通らない）・`Quit anyway` の後の保存（試すが失敗は捨てる = 利用者が選んだ）。
    /// Wave: FX1
    Quit { anyway: bool },
    /// FX2（FX-004〜007）: Home の欄の下の生成の 1 行（`build_progress::show`）で押した手。作り手 = `home::draw`（`Generated::model_progress` の 1 行・
    /// CLI を確かめている間の 1 行）だけ。処理（app.rs `apply`）: `Cancel` → CLI を確かめている間なら待ちを捨てて文を欄へ・生成中なら `Generated::stop_model_job`
    /// （子が消えて `ModelDone::Stopped` が届くまで欄の `Create` は押せない形のまま）・`Retry` → 欄の今の文で `Generate`・`UseSimpleRules` → 欄の今の文で
    /// `Generated::create_project_simple`（名前の表・AI を呼ばない・作った後の 1 行が AI を使わなかった事を言う）・欄が空なら 2 つとも欄に focus だけ・
    /// `Rephrase` → 欄に focus（文はそのまま）・`OpenRun` → `Generated::model_run_id` を `ui::runs::open_run` に渡して `Go(View::Runs)`・
    /// `AllowConnection` / `OpenInSource` → 生成の行は出さない（refine の行だけ・来たら何もしない）。
    /// 例: `if let Some(a) = build_progress::show(ui, "home", &b, |_| {}) { intent = Some(Intent::Build(a)) }`
    /// 保証する: 文を失わない（`Retry`・`UseSimpleRules` は欄の Create と同じ = 送った文は生成の間 job が持ち、止めた・失敗・作れない時は欄へ戻す）・
    /// Stop の後に新しい生成を始めない（`Retry` を押すまで）。保証しない: Page の欄の編集の Stop（FX2 の外）。
    /// Wave: FX2
    Build(super::build_progress::BuildAction),
    /// PV1: Home の欄の下の発見の手（`Shell::discovery`・app.rs が `project::discover::act` に渡す・何も入れない）。
    /// Wave: PV1
    Discover(super::discover::DiscoverAct),
    /// RQ1: Settings の form で値を変えた（作り手は `crate::settings::draw` だけ・form は `model::UiSettings` を `&mut` で直に書く = header の欄が `Shell::command` を書くのと同じ形）。
    /// 処理（app.rs `apply`）: `store.ui` から副作用を全部当て直して `mark()`（debounce の保存）: theme は作らない（Appearance は ⌘K と同じ `Intent::SetTheme`）・`reduce_motion` → `motion::set_reduce_motion` か
    /// `motion::clear_motion_mode`（None = OS に従う）・`text_zoom` → `ctx.set_zoom_factor`・`llm_max_calls` / `llm_max_secs` → `Generated::set_budget`。起動の時も同じ当て直しを 1 回。
    /// 保証する: View・Project・Page を変えない。画面に出る値は `store.ui`（保存した物）そのもの。保存の失敗は今の `Couldn't save your changes · {e}` の 1 行。
    /// 保証しない: 1 frame の中で同じ値を 2 度書いた事の区別（当て直しは冪等）。
    /// Wave: RQ1
    #[allow(dead_code)] // 作り手の `crate::settings` が無い試験の crate（ui だけを `#[path]` で引く）
    SettingsChanged,
    /// RQ1（RQ-004・UIUX N11）: Settings › About の `Copy diagnostics`。処理（app.rs `apply`）:
    /// `ctx.copy_text(rows の "{k}: {v}" を改行で); self.message = "Diagnostics copied".into();`（rows = `Generated::diagnostics(&connections_model().rows)`）。
    /// 保証する: View・Project・保存を変えない・送らない（貼るのは利用者）。文に path・token・打った文・Project の名前を入れない（`Generated::diagnostics`）。
    /// Wave: RQ1
    #[allow(dead_code)] // 同上
    CopyDiagnostics,
    /// RQ2（RQ-005）: Project の名前を `name` に替える。作り手は 1 つの欄だけ（`Shell::renaming` が Some の間の Enter・`⌄` と Home の行の `•••` の `Rename` は
    /// `renaming` を立てて欄に今の名前を入れ focus するだけ）。処理（app.rs `apply`）: `Generated::rename_project` → 通れば `shell.renaming = None`・拒否は欄の下の 1 行で欄に残す。
    /// 保証する: Project の id・Page・居場所を変えない。保証しない: folder の名前を替える事（Shibubu の中の名前だけ）。
    /// Wave: RQ2
    RenameProject { project: String, name: String },
    /// RQ2（RQ-005）: Project を複製して、できた Project の先頭 Page へ移る（`Generated::duplicate_project`）。作り手 = `⌄` と Home の行の `•••` の `Duplicate`。
    /// 保証する: 元の Project を変えない。何を写し何を写さないかは `Generated::duplicate_project` の doc。
    /// Wave: RQ2
    DuplicateProject(String),
    /// RQ2（RQ-005・Undo できないので確認の後だけ）: Project を消す（`Generated::delete_project` → 今その Project に居れば `Go(View::Home)`）。作り手 = 行内の確認
    /// （`theme::confirm_inline`・`Shell::project_confirm` が Some の時に Home の行の下か Project の頭に出る）の `Delete` だけ。`•••` / `⌄` の `Delete…` は `project_confirm` を立てるだけ。
    /// 保証する: 1 回目の押下で消さない・Esc と `Keep` は何も消さない（app.rs の Esc の連鎖が `project_confirm` を None に）。消す物と残す物は `Generated::delete_project` の doc。
    /// Wave: RQ2
    DeleteProject(String),
    /// RQ2（RQ-005・Archive / Unarchive）: sidebar の Your projects から外す / 戻す（`UiSettings::archived_projects`・何も消さない）。作り手 = sidebar の Project の行の右 click と `•••` の menu
    /// （SESSIONS-LEFT-1101 で Home の Continue の `•••` から移した・Archive した行は sidebar の Your projects の見出しの `•••` の `Show archived · {n}` で開く）。
    /// Wave: RQ2
    ArchiveProject { project: String, archived: bool },
    /// RQ2（RQ-007・Pin / Unpin）: sidebar の Your projects の上に留める / 外す（`UiSettings::pinned_projects`）。作り手 = sidebar の Project の行の右 click と `•••` の menu。
    /// Wave: RQ2
    PinProject { project: String, pinned: bool },
    /// SESSIONS-LEFT-1101（owner 198 の 3）: sidebar の Project の中の session と Page を開く / 閉じる（`UiSettings::open_projects` に覚える）。
    /// RQ2 の `SortContinue`（Continue の Recent / Name）の席を替えた（owner 198 の 5「name・recent いらない」）。
    /// 作り手 = `sidebar::draw` の Project の行（owner 210 の 6「^これいらない。押すと勝手にdropdownになるように」: 今開いている Project の行を押すと開閉・
    /// 他の Project の行は `Intent::Go` で、移った先の Project は app.rs の `go` が開く）。
    /// Wave: SESSIONS-LEFT-1101
    FoldProject { project: String, open: bool },
    /// SESSIONS-LEFT-1101 追記 244（owner「projectsの中身はしまえるように畳めるようにしたい」）: sidebar の見出し（`sidebar::HEADING_SESSIONS`・`HEADING_PROJECTS`）の下を
    /// 畳む / 開く（`UiSettings::folded_headings` に覚える = 再起動の後も畳んだまま）。作り手 = 見出しの行の hover で出る `⌄`（AX `Collapse {name}` / `Expand {name}`）。
    /// `FoldProject` に入れない理由: 見出しは既定が開いた形（覚えるのは畳んだ物）・Project は既定が閉じた形（覚えるのは開いた物）で置き場の向きが逆。
    /// Wave: SESSIONS-LEFT-1101
    FoldHeading { heading: String, open: bool },
    /// SESSIONS-LEFT-1101 追記 244: sidebar の Project の行の `•••` の `Rename`（名前の欄は `View::Project` の頭にしか無い = 今開いていない Project はまず移る）。
    /// app.rs の腕 = その Project でなければ `go(View::Project)` → `Shell::start_rename(project, name)`（`go` が名前の欄を閉じるので順はこの 1 つ）。`name` = 今の名前。
    /// Wave: SESSIONS-LEFT-1101
    StartRename { project: String, name: String },
    /// RQ2（RQ-009）: Home の empty state の `Start with a sample`（`home::EXAMPLES` の index・1 と 2 だけ）から AI を使わずに作り（名前の表）、`UiSettings::sample_projects` に足して開く。
    /// 保証する: model を呼ばない・外の app と file に書かない例だけ（`home::EXAMPLES` の doc・0 = 主デモの一言は sample にしない）・`Try` の chip（`Generate`）は変えない。行に `Sample` の印。
    /// Wave: RQ2
    #[allow(dead_code)] // 作り手（Home の empty state の `Start with a sample`）は後の片: 0 件の Home はデモの 1 手目の画面なので、デモの録画の後に足す
    TrySample(usize),
    /// RQ2（RQ-ACCT）: 次の AI の呼び出しに使う account を選ぶ（`None` = System default・`Some(account)` = `UiSettings::ai_accounts` の行そのもの = 種類と id の組・G84 operator 10-29 ②）。
    /// 処理（app.rs）: `UiSettings::set_active_account` で `ai_active` と `ai_active_kind` を書き、
    /// `llm_cli::CliConfig::account` を替えて裏で確かめ直し `Generated::set_llm`（読んでいる途中の 1 回は前の account のまま）。確かめが通らなければ前の account に戻して理由の 1 行。
    /// Wave: RQ2
    UseAiAccount(Option<crate::model::AiAccount>),
    /// RQ2（RQ-ACCT・Add account）: 2 つ目の plan の login を足す（`llm_cli::add_plan_account` = G99b の Sign in に CLAUDE_CONFIG_DIR を足して裏の子で・最大 5 分）。
    /// 通れば `ai_accounts` に足す（選ぶのは利用者・勝手に Active にしない）。終わらなければ dir を消して `Sign in didn't finish · try again`。
    /// 作り手 = Settings › AI & Models の `Add account`（sign in の途中は描かない）。処理 = app.rs `start_account_sign_in` → `poll_account_sign_in`。
    /// ACCOUNTS-1101: 中身 = どの AI に足すか（`AiAccount::provider` の語）。`"claude"` は上のまま。grok・kimi・opencode は `llm_cli::add_cli_account` で置き場を作って
    /// 列に足し、下の sign in の terminal をその置き場で開く（`start_cli_sign_in`・終われば確かめて切り替え・途中は terminal の × で止める）。作り手 = Connections と
    /// Settings の provider の見出しの `+ Add account`（`settings::accounts`）。分けられない CLI（cursor-agent）には button を出さず理由の 1 行。
    /// Wave: RQ2, ACCOUNTS-1101
    AddAiAccount(String),
    /// RQ2（RQ-ACCT・Re-authenticate）: その account の login をし直す（`llm_cli::reauthenticate`）。失敗・取り消しは列・使っている account・dir を変えない（理由の 1 行）。
    /// 作り手 = Settings の plan の account の行の `Re-authenticate`。通って使っている account なら確かめ直す。
    /// 中身は account そのもの（種類と id の組・G84 operator 10-29: 手で書いた設定で id が重なっても、押した行の account だけをし直す）。
    /// Wave: RQ2
    ReauthAiAccount(crate::model::AiAccount),
    /// RQ2（RQ-ACCT・Remove）: account を外す（`llm_cli::remove_account` = CLAUDE_CONFIG_DIR か Keychain の鍵を消す）。Active なら System default に戻す。確認は行内（Undo できない）。
    /// 中身は account そのもの（種類と id の組・G84 operator 10-29: id が重なった 2 つを両方消さない = 利用者の login を黙って消さない）。
    /// Wave: RQ2
    RemoveAiAccount(crate::model::AiAccount),
    /// ACCOUNTS-1101 2 段目（作る物 5）: 足した account に名前を付ける（Connections / Settings の行の名前を押す → 行内の欄 → Enter）。空 = 既定の名前に戻す。
    /// 中身は account そのもの（種類と id の組）と打った名前。処理（app.rs）= `UiSettings::set_account_name`。作り手 = `settings::accounts`。
    /// Wave: ACCOUNTS-1101
    RenameAiAccount(crate::model::AiAccount, String),
    /// RQ2（RQ-API・`Paste API key`）: clipboard の Anthropic の鍵を 1 回読んで Keychain へ（`llm_cli::add_key_account`）・`ai_accounts` に `Key` を足す。
    /// 鍵の値は Intent に載せない（Intent は Debug で出る）・画面の欄も作らない（app.rs が `/usr/bin/pbpaste` で読む・読んだ値は記録・画面・usage report に出さない）。
    /// 保証しない: 鍵が Anthropic で通る事（次の呼び出しで `Auth` / `Quota` の 1 行）。
    /// Wave: RQ2
    PasteApiKey,
    /// RQ2（RQ-API・`Use the key from your environment`）: 環境の `ANTHROPIC_API_KEY` を使う account（`AiAccount::EnvKey`）を足す。環境に鍵が無い・断った鍵なら出さない。
    /// Wave: RQ2
    UseEnvKey,
    /// RQ2（RQ-USAGE・`Try again`）: 使用量を今読み直す（`Generated::refresh_usage(now, true)`・間隔と backoff を待たない・読んでいる途中なら 2 本目を起こさない）。
    /// Wave: RQ2
    RefreshUsage,
    /// USAGE-OWN-1101: Settings › AI & Models の Used in Shibubu の `Recount`（追記 223 で帯の panel は外した）。Shibubu の中だけで使った量を今数え直す（`own_usage::Counter::refresh(.., true)`）。
    /// Keychain は読まない（`RefreshUsage` と分けた理由 = 押していない人に許可の窓を出さない・operator ①）。
    RecountOwnUsage,
    /// RQ2 反証の直し（holdout h1・h3）: 欄が空のまま右端の button（`Create` / `Change` / `Rename`）か Enter を押した時の、何を打てばいいかの 1 行（app.rs が status の行に出す）。
    /// 作り手 = `header::command_field` だけ（`enter_intent` が None・右端が button の時）。保証する: 何も作らない・変えない。
    /// Wave: RQ2
    Notice(String),
    /// RQ3（RQ-010・`Reveal in Finder`）: path を Finder で見せる（`/usr/bin/open -R`・`generated::run_open` と同じ口）。作り手 = Settings › Data & Recovery の
    /// `Saved in` の行（data dir）・`Export…` の後の 1 行（書き出した file）・Backups の行（その世代の dir）。保証する: 何も書かない・消さない・Shibubu の窓を前に出さない。
    /// Wave: RQ3
    #[allow(dead_code)] // 作り手（Settings › Data & Recovery）は RQ3 の本体で描く
    RevealInFinder(std::path::PathBuf),
    /// RQ3（RQ-011・`Back up now`）: 今の DB と設定の写しを `backups/` に 1 世代足す（`Generated::back_up_now`）。結果は節の 1 行（`Backed up · {時刻}` か理由）。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上
    BackUpNow,
    /// RQ3（RQ-011・`Restore`）: `Some(id)` = その世代（`DataSummary::backups` の id・壊れて隔離した物も）を次の起動で戻す予約・`None` = 予約を取り消す
    /// （`Generated::restore_backup`）。作り手 = Backups の行の `Restore` と予約の行の `Cancel`。保証する: 押しただけでは今の data を変えない（戻す前の data は
    /// `before-restore` の世代として残る）。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上
    RestoreBackup(Option<String>),
    /// RQ3（RQ-012・`Export…`）: Project を `.shibubu` の file に書き出す（`Generated::export_project`・置き場は app.rs が渡す Downloads）。作り手 = header の
    /// `{name} ⌄`（`header::draw` の `menu_extra`）と Home の行の `•••` の `Export…`。結果 = `Exported {file} · Reveal in Finder` の 1 行か理由。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上（menu の行も本体で足す・RQ2 の「Export… は RQ3 と一緒に出す」）
    ExportProject(String),
    /// PROJECT-DIR-1101: Project の folder を Finder で開く（`Generated::project_folder` → `/usr/bin/open`）。作り手 = `header::project_actions`（`{name} ⌄` と Home の行の `•••`）。
    /// Wave: PROJECT-DIR-1101
    ShowProjectFolder(String),
    /// PROJECT-DIR-1101: Project の folder を選び直す（G99c の選ぶ窓 → `PageIntent::SetWorkspace` = その folder を repo に・動いている terminal は cd しない）。作り手 = 同上。
    /// Wave: PROJECT-DIR-1101
    ChangeProjectFolder(String),
    /// RQ3（RQ-012・`Import`）: `.shibubu` の file から Project を作って開く（`Generated::import_project`）。作り手 = Settings › Data & Recovery の Import の list の行
    /// （`DataSummary::imports` = Downloads と Desktop の `.shibubu`・modal の file 選びは作らない）。保証する: 元の file を変えない・既存の Project を上書きしない。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上
    ImportProject(std::path::PathBuf),
    /// RQ3（RQ-013・`Delete my data…`）: その種類を消す（`Generated::delete_my_data` と、app.rs の腕が `UiSettings` の側 = `Projects` は pin・archive・sample・
    /// opened・drafts・last_project、`Connections` は ai_accounts・ai_active と `llm_cli::remove_account`）。作り手 = 行内の確認（`Shell::data_confirm` が Some の時）の
    /// `Delete` だけ（1 回目の `Delete…` は `data_confirm` を立てるだけ・`Keep` と Esc は何も消さない）。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上
    DeleteMyData(DataKind),
    /// RQ6（RQ-022）: Settings › Privacy の `Revoke`。その bundle id の AI の操作の同意を取り消す（`surface::revoke_act_consent`）。作り手 = `crate::settings::draw`。
    #[allow(dead_code)] // 同上
    RevokeConsent(String),
    /// RQ6（RQ-022）: Privacy の Mirror の同意の `Revoke`（`Generated::revoke_mirror_consent`）。
    #[allow(dead_code)] // 同上
    RevokeMirrorConsent(String),
    /// RQ3（RQ-028・`Restore diagram from before`）: Project ごとの store に替える前（G72）の共有の web の store に在る Diagram の図を、この Project の Diagram の
    /// store へ写して Diagram の Page を開く（`Generated::restore_diagrams` → `PageWebViews::restore_diagrams`）。作り手 = header の `{name} ⌄`（`menu_extra`・その Project に
    /// Diagram の Page が在る時だけ）。
    /// 保証する: 前の共有の store を消さない・この Project の図が空でなければ上書きしない。
    /// Wave: RQ3
    #[allow(dead_code)] // 同上
    RestoreDiagrams(String),
    /// RQ4（RQ-015〜RQ-017）: Connections の行 `row`（`connections::Connection::key`）で手 `act` を押した（`connections::draw` が返す）か、1 つの欄の Enter
    /// （`Shell::connecting` の間 = `ConnAct::Submit`）。処理（app.rs `apply`）: 行の key と手の組ごとに `connections::ConnAct` の doc の表（確かめは裏の thread・
    /// 結果は行の `checked_at` と detail・失敗は行の detail に理由と次の一手の 1 行）。例: `Intent::Connection { row: "mcp:penpot".into(), act: ConnAct::Disconnect }`。
    /// 保証する: 秘密の値と欄の文を載せない（`PasteSecret` は名前だけ・`Submit` の文は app.rs が `shell.command` から読む）・`Remove` は `Shell::connection_confirm` の
    /// 確認の `Remove` からだけ届く（行の `Remove…` は確認を開くだけ）・View を変えない。保証しない: 確かめが通る事。
    /// merge の時: FX5 の `Intent::RecheckGithub` と今の `RecheckGui` は `Connection { row: "github" | "gui", act: CheckAgain }` へ寄せる（`ConnAct` の doc）。
    /// Wave: RQ4
    #[allow(dead_code)] // 作り手（`connections::draw` の acts の button と `Shell::enter_intent` の Submit）は RQ4 の本体で描く
    Connection { row: String, act: super::connections::ConnAct },
}

/// Project の画面の Page の操作（YCM-012・013）。app.rs が `Generated::page_intent` に渡し、返った View へ移る。
///
/// 使い方: Page タブの「＋Page」→ `Intent::Page(PageIntent::Add)`、戻る → `PageIntent::Back`。
/// 保証する: どれも Page ID を変えない。Delete の最後の 1 枚・無い id は拒否され、理由の 1 行が返る（画面は今の Page のまま）。
/// UI-5a の行き先（Page の帯 = 36 の行の左に文字の tab・右端に `↶ ↷ ＋ •••`。menu の並びの正本は `generated::page_menus`）: `Add` = 帯の `＋`（MIRROR-FRAME-1101 から menu を開かず直に）／
/// `AddMirrorFrame`・`AddBrowser`・`TerminalPanel(PanelAct::Add)` = Page の右下の `＋`（pointer が Page の上の時だけ見える）と Page の右 click の menu ／
/// `Move`（`Move left` / `Move right`）・`Placement`（`Layout: {name}` の 1 項目・押すたびに次の型 = 今の動き）・`ShowTerminal`（`Show terminal`）・`Reread`（`Refresh`）・
/// `Redo`・`Delete page…`（tab の横の確認を開くだけ）= `•••` の menu ／ `Undo`・`Redo` = 帯の `↶ ↷`（在る時だけ）と ⌘Z・⇧⌘Z ／ `Back` = title line の `‹` の席。
/// 保証しない（UI-5b）: tab の右 click の menu・`Placement` の行き先の指定（`Layout` の 4 行）。
/// 契約（FX3 から・variant は足さない）: 帯の tab は全 Page（TITLEBAR-ICONS-1101 で FX3 の 5 枚と `More` を消した・溢れた分は帯の横 scroll）・⌘K の
/// `Go to {Page}`（`Generated::page_commands`）・⌘1〜9（n 枚目）からも行ける。`•••` の先頭に `Rename…`（`Rename`）。帯の操作が通ると影響を受けた tab に 600ms の tint と Page の上の
/// 1 行（`generated::change_line`・`RegionState::Ready`）。AI がこの Project の変更を読んでいる間（`JobWhat::Edit` の job）は、帯の `↶ ↷ ＋ •••` を押せない形にし、
/// Page の形を変える腕（`Add`・`Rename`・`Delete`・`Move`・`Undo`・`Redo`・`AddBrowser`・`AddMirrorFrame`・`AddTerminal`）は `page_intent` が
/// `One change at a time. The last one is still being read.`（Ready・何も書かない）で拒む（FX-013・AI の遅い編集が base_hash で黙って落ちるのを人の側で起こさない）。
/// Wave: W2, UI-5a, FX3
#[derive(Clone, Debug, PartialEq)]
pub enum PageIntent {
    /// 末尾に空の Page を 1 枚足して、そこへ移る。title は「Page {n}」。
    /// 契約（FX3・FX-014）: 移った次の frame で、足した Page の tab に keyboard focus を置く（帯の外なら横に scroll して見せる）。Page の上の 1 行 `Added page “{Page}” · ⌘Z to undo`。
    /// Wave: FX3
    Add,
    /// FX3（FX-009・UIUX B02・B09）: Page の名前を `title` に替える（route と id は変えない・`router::rename_page` → `apply_project(.., DirectEdit)`・Undo / Redo の対象）。
    /// 作る側: `•••` の先頭の `Rename…`（`generated::page_menus` の項目の intent は `Rename { page, title: String::new() }` = 欄を開く印で、
    /// `draw_page` が `Delete page…` と同じく横取りして tab の席に `theme::rename_inline(.., "Page name")` を出す・1 回目で名前を替えない）→ 欄の Enter が
    /// `Rename { page, title: 打った名前 }` を返す。Esc（app.rs の Esc の連鎖 = `Generated::dismiss_rename`）と別の tab へ移ると破棄（何も書かない）。
    /// 保証する: 空・別の Page と同じ名前（`ProjectPatchError::DuplicateTitle`）は Err の 1 行で何も書かず、欄は打った名前のまま開いている（打ち直せる）。
    /// 今と同じ名前（trim 後に完全に同じ）は何も書かず欄を閉じる（revision を足さない = `Move` の同じ位置と同じ）。通ったら DB の Project を読み戻した名前で tab を描き、
    /// Page の上の 1 行 `Renamed “{old}” to “{new}”`（Ready）。保証しない: 自然言語の `rename Docs to Tests` の道（`Edit` の腕・`composer::project` が拒む）。
    /// 例: `g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "Tests".into() })?` → `g.project(&project)?.1.page(&b).title == "Tests"`
    /// Wave: FX3
    Rename { page: String, title: String },
    /// 消して、次（無ければ前）の Page へ移る。
    /// UI-5a: 消す前に、消す Page が今の Page なら `Generated::save_view_state` の保存の半分（`save_view_inner`・断られた時に窓を隠さない）で打ちかけを ui_state に保存する（`page:{id}:view` の行は消さないので、
    /// ⌘Z で戻った Page に打った文字が戻る・Done 3）。保存できなければ Err の 1 行で何も消さない。作る側は tab の横の確認（`theme::confirm_inline`）の `Delete` だけ
    /// （`•••` の `Delete page…` は `pending_delete` を立てるだけ・1 回目で消さない・owner の答え ③）。
    /// 契約（FX3・FX-015・UIUX B06・N05）: 確認の文は実際と同じ `Delete “{Page}”? ⌘Z brings it back.`（表 copy.en.md の §13 の後の文。打ちかけは上の保存で ⌘Z で戻る =
    /// 前の「Text typed on this page is lost.」は嘘だった）。消せたら Page の上の 1 行 `Deleted page “{Page}” · ⌘Z to undo`（Ready）。
    /// Wave: UI-5a, FX3
    Delete { page: String },
    /// 履歴の 1 つ前の Page へ。履歴が空なら何もしない。
    /// UI-5a: 作る側は title line の `‹` の席（`header::draw` の `back`・AX の名前 `Back to {Page}`・履歴が空なら描かず席の幅だけ取る）。帯の `Back` の button は消す。
    /// Wave: UI-5a
    Back,
    /// W6「→移動」: `page` を並びの `index` へ移す（`ProjectPatchOp::MovePage` を `apply_project(.., DirectEdit)` で当てる）。居場所は今の Page のまま。
    /// 使い方: Page タブの「→」→ `PageIntent::Move { page: 今の id, index: 今の位置 + 1 }`。
    /// 保証する: Page の id・route・木・Page の revision と `ui_state` の `page:{id}:view` は変えない（変わるのは並びと Project の revision だけ）。
    /// 今と同じ位置なら何も書かない。無い id は `PageNotFound` の 1 行で何も書かない。保証しない: Page の中の部品（surface）の移動（`MoveSurface` の op・W7）。
    /// 契約（FX3・FX-010・UIUX B03）: 作る側を 3 つに（どれもこの 1 つの variant・新しい鍵の組は作らない = DESIGN §11 と copy.en.md:513 に「tab を動かす鍵」が無い）:
    /// ① `•••` の `Move left` / `Move right`（今のまま）② 帯の tab の drag（押した tab を横に動かし、離した x より中心が左の他の席の数を帯に見えている席の中で数える・
    /// 自分の席と同じ数なら手にしない〈少し動かした drag・0.8 秒以上の長押しも〉・少ない = その席の Page の前・多い = その席の Page の後・帯の外の Page は並びのまま・
    /// 名前の欄の席も 1 つの席・動かない drag〈閾値 egui の既定〉は click = 移る）③ keyboard = Tab で `•••` に focus → Return で menu（先頭の項目に focus = `menu_popup`）→ Tab で `Move left` / `Move right` へ → Return。
    /// 通ったら席が変わった tab が前の位置から新しい位置へ `motion::travel`（`PAGE`・押せる席は最初の frame から新しい位置・Reduce Motion は即時）・その tab に tint・
    /// Page の上の 1 行 `Moved “{Page}” left` / `Moved “{Page}” right`（Ready）。
    /// Wave: FX3
    Move { page: String, index: usize },
    /// W7 自然言語の編集: Project の Chat の会話の案（CHAT-ONE-1101: 欄の Enter は `Talk`・案の Accept がこの文を送る）。
    /// `composer::project::edit_project` で ops にし、`apply_project(.., RevisionOrigin::NlEdit)` で 1 回だけ当て、router を `reconcile` する。
    /// 使い方: 会話の案の Accept → `Intent::Page(PageIntent::Edit { text: 案の 1 文 })`（app.rs は Page の他の操作と同じ道で、変えない）。
    /// 保証する: AddPage（Mirror の AddPage + AddSurface も含む）なら足した Page へ移る（`Add` と同じ）・MoveSurface なら動いた先の Page（`to_page`）へ移る（今の Page がそこなら移らない・UI-5a・sendback 60.4 A3）。読めない・既に在る・右に Page が無い等は
    /// 理由の 1 行（既に在る =「<名前> の Page は既に在ります」・右に Page が無い等 = 次の一手の 1 行・読めない = 読める 3 つの形）で何も書かず今の Page に残る（revision を足さない）。保証しない: 欄の文を送った後も残す事（送れたら空にする）。
    /// 契約（UI-2 から・variant の形は変えない）: 英語の 4 形も読む（`composer::project::edit_project` の doc）。`EditVerb::read(text)` が
    /// `RunTests`（`run the tests` 等）なら model を呼ばず `RunTests { command: "cargo test" }` と同じ道（Tests の Page の button と同じ command・同じ拒否）。
    /// `MoveLeft`・`RenamePage` は model を呼ばず名前の表で読む（model の語に無い・送ると unsupported → engine に流れる）。
    /// 読めない文の 1 行は `composer::project::rescue_line()`（engine に渡さなかった時だけ・文は欄に残る）。理由の 1 行は全部英語（表の EN）。
    /// 保証しない: model が在る時に AddPage・MoveRight の英語の言い換えを model が読む事（W13 の I3 のまま）。
    /// Wave: UI-2, UI-5a
    Edit { text: String },
    /// W8 Undo（YCM-045・DEMO-09）: Project を 1 つ前の revision の snapshot に戻す（`UndoRedo::undo`）。帯の右の組の `↶`（UI-5a: painter の記号・AX の名前 `Undo · ⌘Z`・
    /// `undo_redo` が None の時は出さない）・⌘Z（UI-5a: `ax::undo_for_project` の門 = focus の在る欄に文字が在る時だけ欄の undo に譲る・Terminal に focus が在っても project の Undo）。
    /// 今の Page の入力途中の状態を保存してから戻し、router を `reconcile` する（今の Page が消えたら履歴の 1 つ前、無ければ先頭へ）。
    /// 保証する: Project の hash・Page の数と順・Page ごとの `page_hash` が編集の前と同じ。`project_revisions` は書かない・消さない。
    /// Shibubu の UI の定義だけを戻す: `surface_receipts`・`ui_state`・Terminal の session と process・元の app と disk の file・外への送信は触らない。
    /// 戻せない時は `UndoError` の 1 行で何も書かない。保証しない: 元の app の中の編集を戻す事（それは元の app の Undo・W16-2）。
    /// 契約（FX3・FX-012・UIUX B04・MH-M19）: 何が戻り何が戻らないかを言う。`↶` の hover は AX の名前 `Undo · ⌘Z` のまま（labels.py が引く）に
    /// `generated::undo_scope_line` の 1 行を足す（`on_hover_text` をもう 1 回 = AX の名前は変えない）。戻せたら Page の上の 1 行
    /// `Undid: {action} · {undo_scope_line} · ⇧⌘Z to redo`（Ready・`generated::change_line`）。
    /// Wave: UI-5a, FX3
    Undo,
    /// W8 Redo: `Undo` で戻した 1 つ後の revision の snapshot へ進む（`UndoRedo::redo`）。帯の `↷` と `•••` の `Redo`（UI-5a: AX の名前 `Redo · ⇧⌘Z`・在る時だけ）・⇧⌘Z（`Undo` と同じ門）。保証は `Undo` と同じ向きの逆
    /// Wave: UI-5a
    /// （Project の hash・Page の数と `page_hash` が編集の後と同じ・外の効果を作り直さない）。Undo の後に別の編集を当てると Redo の行き先は消える。
    /// 契約（FX3・FX-057）: Undo で戻した Add / Move / Delete / Rename を Redo が同じ形に戻す（Page の id・並び・名前）・Page の上の 1 行 `Redid: {action}`（Ready）。
    /// Runs の行は作らない（Page の帯の手は Runs の run ではない = FX7 の範囲）。
    Redo,
    /// G92（W8 の引き出しを置き換え・owner 10-27「add terminalもできないよ」）: `•••` の `Show terminal` と ⌃`。この Page に置いた Terminal
    /// （Project の `terminal_specs` に在る物の先頭）の下の panel の tab を選んで開き、grid に focus（TERM-ONE-1101: Page には Terminal を描かない）。無ければ G95 から Page に置かず、
    /// 下の panel を開く（`PanelAct::Open`・tab が無ければ 1 つ足す）。⌃` は G95 から panel の開閉（`PanelAct::Toggle`）。
    /// 保証する: Project の revision と hash を変えない・Page の Terminal は新しい shell を起こさない（同じ session）・居場所の Page は変えない。
    /// 保証しない: Page を移す事（別の Page の Terminal は見せない）・拡大した Mirror の下に隠れた Terminal を見せる事。
    ShowTerminal { page: String },
    /// MIRROR-AUTO-1101 B（owner 192）: `•••` の `Stop mirroring`（この Page に Mirror の面が在り、1 つでも止まっていない時だけ）。この Page の Mirror の面を全部
    /// `MirrorWindow::Closed` にする（helper の stream を止める・元の app の窓は閉じない）。止めた事は面ごとに `mirror-window` に保存され、開き直し・再起動の後も止まったまま。
    /// 映し直すのは面の帯を押す（同意は尋ねない）。保証する: Project の revision と hash・居場所の Page を変えない。
    /// Wave: MIRROR-AUTO-1101
    StopMirroring { page: String },
    /// W13c: 配置の型を次の型へ替える（同梱の順 → 型なし → 先頭・`project::PlacementChoice` を `ui_state` に書くだけ）。Page タブの「配置: <名前>」。
    /// 保証する: 居場所の Page・Project の revision と hash・Page の木を変えない（`Reread` と同じ）。
    /// Wave: W13c
    Placement,
    /// W11: Tests Page の「走らせる」。Agent の口として `Generated::record_run` を呼び、RunReceipt を `run_receipts` に書く。
    /// 使い方: Tests Page の「走らせる」→ `PageIntent::RunTests { command: "cargo test".into() }`（試験は失敗の通しに `false` を渡す）。
    /// 保証する: Human の lease の間は拒否の 1 行で byte 0。失敗の run は `status_line` が「失敗」・`exit_code != 0`。空の command・Terminal が無い時は 1 行で何も書かない。
    /// 保証しない: Terminal に打った `cargo test` を自動で receipt にすること（口はこの intent だけ）。
    RunTests { command: String },
    /// W5: この Page に Terminal を 1 つ置き（`ProjectPatchOp::AddTerminal`・AI の plan の口）、その場で shell を起こす。Page は移らない。
    /// TERM-ONE-1101: Page には描かず、同じ id の下の panel の tab を選んで開く（`Generated::adopt_plan_terminals`）。
    /// shell は `$SHELL`（無ければ /bin/zsh）・cwd は Project の `resources` の先頭の kind "repo" の uri（`file://` か絶対 path）、無ければ `$HOME`。
    AddTerminal { page: String },
    /// W12a「↻ 読み直す」: Project の resource（repo）を今読み直す（`Generated::read_resources` を 1 回）。Page タブの並びのボタン
    /// （AX の名前も「↻ 読み直す」・uri を持ち、どこかの Page の node が bind している resource〈Dev の Git の欄〉が Project に無い時は出さない＝押しても何も起きないボタンを見せない・L2）。
    /// 保証する: 居場所の Page・Project の revision と hash・Page の入力途中の状態を変えない・Undo / Redo の対象にしない。
    /// 読めない時は理由の 1 行を Page の上に出す（`read_resources` の doc）。保証しない: 押した後に resource が変わらない事。
    Reread,
    /// W12W-B: この Page に Browser の面を 1 つ置く（`ProjectPatchOp::AddSurface`・`browser: true`・最初の URL は Google）。Page は移らない。
    /// Page タブの「＋ Browser」。保証する: spec の id が重なれば `DuplicateSurface` の 1 行。保証しない: 頁が読める事。
    /// 契約（FX3・FX-014・UIUX B08）: 置いた次の frame で、その Browser の上の欄（`SetSurfaceUrl` の欄）に keyboard focus（G92 の Terminal の `reveal` と同じ形・
    /// Generated の private な印）・Page の上の 1 行 `Added a browser to “{Page}”`（Ready）。保証しない: WebView（WKWebView）の中に focus を渡す事（欄は egui の側）。
    AddBrowser { page: String },
    /// MIRROR-FRAME-1101（owner 247・253 = 案 3）: この Page に中身の無い Mirror の枠を 1 つ置く（`ProjectPatchOp::AddSurface`・backend Mirror・`SurfaceTarget::App` の
    /// bundle id が空 = `SurfaceSpec::blank_frame`・keep_alive・`frame` = 640×400 = 246 の決まりで後ろの部品がどく）。Page は移らない。⌘Z 1 回で消える。
    /// 作り手 = Page の右下の ＋ と Page の右 click の menu の `Add mirror frame`（`generated::page_menus` の 1 つ目・feature `mirror` の build だけ）。
    /// 保証する: Page の上の 1 行 `Added a mirror frame to “{Page}” · ⌘Z to undo`（Ready）・置いた次の frame でその枠を見える所へ送る（`reveal`）。
    /// 保証しない: 窓を入れる事（枠の `Choose a window`・引いて入れる・`Open an app…` が `ChooseWindow { bundle_id: Some(..) }` を送る）。
    /// Wave: MIRROR-FRAME-1101
    AddMirrorFrame { page: String },
    /// W12W-B: Browser の上の欄。`input` を `bar_input_to_url` して `SetSurfaceUrl`。空は何も書かない。
    SetSurfaceUrl { surface_id: String, input: String },
    /// W12W-B: 題の帯 / 端の drag の終わり。`SetSurfaceFrame`。幅・高さが 1 未満なら何も書かない。
    /// MIRROR-HYBRID-1101 追記 246: Mirror の四隅の引き終わり（`MirrorPages::take_frames`）も同じ手。
    SetSurfaceFrame { surface_id: String, x: f64, y: f64, w: f64, h: f64 },
    /// W13b: Page の中の 1 問（一言が名指した app がこの Mac に無い席・`verifier::SeatKind::MissingApp`）の答えの button。`seat` = その席の id
    /// （`Seat::id` = 1 問を置いた Page の route の名前）。`use_app` = `Some(bundle_id)`「代わりに <名前>」・`None`「<名前> 無しで続ける」。
    /// 「入れる頁を開く」は intent にしない（`renderer::usable_line` の link を egui が既定の browser で開くだけ・Project を変えない・何も入れない）。
    /// 使い方: `draw_usable` が `usable_line` の押された index を `PageIntent::AnswerMissingApp { seat: u.seat.id.clone(), use_app }` にする。
    /// 保証する: 1 問の node を `SeatV1::asked_in` で探し、`Some` は 1 問の `instead` に在る bundle id の時だけ `[Page{Remove 1 問の node},
    /// AddSurface{SurfaceSpec{id: seat, composer::project::APP_SEAT_BACKEND, App{bundle_id, file: 席の file}, keep_alive: true}, 同じ親の同じ位置, node_key: "<seat>-surface"}]`、
    /// `None` は同じ node の `Page{Update}` で props を `{text: "Continuing without <名前>", style: "helper"}`（PV1 で送り直しを求める文を外した） に替える
    /// （update は props を丸ごと替えるので 1 問も消える）。どちらも `apply_project(.., DirectEdit)` で 1 回（Undo / Redo の対象）・居場所は今の Page のまま。
    /// 1 問が無い（答えた後・Undo で消えた）・`instead` に無い bundle id・当たらない op は理由の 1 行で何も書かない（revision を足さない）。
    /// 保証しない: 置いた面がすぐ使える事（W14b の使える検査）・入れた後にこの Project のまま面が置かれる事（入れた後は一言を送り直す）。
    /// `generated` の `ADD_SURFACE_BUILT` は trunk（W16M-3 の AddSurface）に入ったので true（false の build では `Some` を当てず理由の 1 行）。
    /// Wave: W13b
    AnswerMissingApp { seat: String, use_app: Option<String> },
    /// PV0（図 §49）: 無い app の 1 問の `Install {名前} & continue` と、その後の同じ場所の 1 行の button（`Approve and install`・`Cancel`・`Check again`）。
    /// `seat` = 1 問の席（`AnswerMissingApp` と同じ）。出すのは席の app の名前が前もって確かめた表（`project::acquire::CATALOG`）に在り、
    /// この Mac に Homebrew が在る時だけ（handler の無い button を出さない）。処理は `Generated::install_act`（裏の thread・毎 frame の `poll_installs`）。
    /// 保証する: `Start` は何も入れない（実際の Mac を読み・Homebrew の listing を表と照らすだけ）・入れるのは review の後の `Approve` だけ・
    /// 読み戻した .app の bundle id が表と合った時だけ、1 問を本物の面に替える op を `apply_project(.., DirectEdit)` で 1 回（一言を打ち直さない）。
    /// `Cancel` は review の間だけ（何も入れていない）・`Check again` は入ったか分からない時だけ（読み戻しだけ・入れ直さない）。Retry は無い。
    /// 保証しない: 再起動の後に途中の導入を続ける事（PV0 は覚えない・押し直すと先に Mac を読む）・dmg と formula の道（PV2）。
    /// Wave: PV0
    Install { seat: String, act: InstallAct },
    /// RQ4（RQ-019・`Choose window…`）: Mirror の面 `surface_id` が映す窓を選び直す（`title` = 窓の題に含む語 = `SurfaceTarget::App::file`・None = app の前の窓に任せる）。
    /// 作り手 = 面の帯の app 名の menu と、窓が消えた時の帯の `Choose window…`（`MirrorPages::take_chosen_windows` が返した物を generated.rs が送る・持ち主は面の帯 =
    /// CHAT-015 の card は同じ名前の button を描かない）。処理（`Generated::page_intent`）: `ProjectPatchOp::SetSurfaceWindow` を DirectEdit で 1 回（Undo できる）→ その面を探し直す。
    /// 保証する: 他の面・Page・居場所を変えない。保証しない: その題の窓が今も在る事（無ければ面の 1 行が `Open {app}`・`Choose window…`）。
    /// MIRROR-FRAME-1101: `bundle_id` が Some = app も替える（空の枠の `Choose a window` の一覧・引いて入れた窓・`Open an app…`）。同じ `SetSurfaceWindow` の 1 回 = ⌘Z で空の枠へ。
    /// Wave: RQ4, MIRROR-FRAME-1101
    #[allow(dead_code)] // 作り手は feature `mirror` の build だけ（generated.rs の `draw_page` が `MirrorPages::take_chosen_windows` から作る）
    ChooseWindow { surface_id: String, title: Option<String>, bundle_id: Option<String> },
    /// G95: Project の下の Terminal の panel の手（`＋` の `Add terminal`・⌃` の開閉・⌃⇧` の追加・⌘D の split・⌘1〜9 の切替・帯で押した物）。
    /// 処理は `Generated::panel_act`（Project の定義・revision・居場所の Page を変えない・Undo の対象にしない）。
    TerminalPanel(crate::ui::terminal_panel::PanelAct),
    /// G86: 会話の列の card（承認・同意・結果）で押した物。処理は今までの Page の 1 行と同じ `Generated::engine_act`（Page はもう描かない）
    Engine(crate::ui::engine_view::EngineAct),
    /// G99c → TERM-ONE-1101: chat bar の下の行の folder の icon（CHAT-ONE-1101・Page の `Choose folder…` の button は消した）。app.rs が folder を選ぶ窓（`osascript` の choose folder）を裏の thread で開き、選んだ path を
    /// `SetWorkspace` で渡す（取り消しは何もしない）。`Generated::page_intent` には来ない（来たら理由の 1 行）。
    ChooseFolder,
    /// G99c: 選んだ / 窓に落とした folder を Project の repo にする（`ProjectPatchOp::SetWorkspace` と「No folder」の行ごとに同じ場所の Terminal・
    /// `apply_project(.., DirectEdit)` で 1 回・Undo できる）。居場所は今の Page のまま。
    /// 保証する: 在る folder だけ（無い・file は理由の 1 行で何も書かない）・同じ folder で行も無ければ何も書かない・変えた時は Page の上に `New terminals open in {name}`（Ready）。
    /// 保証しない: 今起きている shell の cwd を替える事（TERM-ONE-1101: 次に開く terminal から = panel の tab の cwd は足した時の folder）。
    SetWorkspace { dir: String },
    /// AI-COMPOSE-1101: AI が書いた部品（`canvas` の node `node`）の `shibubu.set(key, value)`。処理（`Generated::page_intent`）: `CanvasProps::set_patch` を
    /// DirectEdit で 1 回（同じ値は書かない・Undo できる）。作り手 = app.rs の `canvas_sync`（feature `webview` の build だけ）。
    #[allow(dead_code)]
    CanvasSet { node: String, key: String, value: serde_json::Value },
    /// AI-COMPOSE-1101: 部品の `shibubu.act(intent, value)`。処理: `{value}` を値に埋めた一言で Engine を 1 本（`start_engine`・承認の決まりは今のまま）。
    /// Engine が動いている間は走らせず理由の 1 行。作り手は `CanvasSet` と同じ。
    #[allow(dead_code)]
    CanvasAct { intent: String, value: serde_json::Value },
}

/// PV0: 無い app の 1 問の導入の手（`PageIntent::Install`）。EngineAct（Engine の承認の card）と分けた理由: 入れる前の読み（`Start`）と
/// 読み戻しだけの `Check` は Engine の run に無い手で、承認も ConsentReceipt でなく review の 1 件に結ぶため。
/// Wave: PV0
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAct {
    /// `Install {名前} & continue`: 実際の Mac を読み、Homebrew の listing を表と照らして review を出す（何も入れない）
    Start,
    /// `Approve and install`: review の 1 件を入れて読み戻す
    Approve,
    /// `Cancel`: review を閉じて 1 問に戻す（何も入れていない時だけ）
    Cancel,
    /// `Check again`: 入ったか分からない時に読み戻しだけ
    Check,
    /// PV1（PV-006）: `Use <名前> in the browser (<host>)`: 表の公式の web 版を同じ席に置く（入れない・brew を叩かない）
    /// Wave: PV1
    UseWeb,
}

/// 殻の状態。永続する物（rail の pin・theme・sidebar の畳み・下書き）は model::UiSettings、それ以外はここ。
/// 契約（UI-4a から）: 言葉の欄は全 View で 1 つ（`command`）。`prompt`（Home の textarea）・`apps_filter`（Apps の欄）は消し、Page の欄の文
/// （`Generated` の private な `edit_draft`）も消した。場所ごとの打ちかけは `drafts`。`rail_note` は残す（書き手は `Intent::Proposal` の arm と、AI を通らずに作って添えた物を使わなかった 1 行 = app.rs の `say_unused`・sendback 12 / 30）。
/// 契約（UI-4b から）: field `sidebar_open` を足した（`rail_open` の隣・`Shell::new` は true = 今の開いた sidebar のまま）。
/// 例: `palette::commands(&shell, text, apps, &pages)`（FX4 から `&Shell` を取る）。
/// 契約（UI-6a から）: field `canvases` を足した（`Shell::new` は None・header の `•••` から ⌘K へ移す旧 canvas の行の材料 = sendback 73・76・78）。
/// 契約（PV1 で欄を足した）: `discovery` = Home の欄の下の発見の今（`ui::discover`）。
/// Wave: UI-4a, UI-4b, UI-6a, PV1
/// 契約（RQ2 から）: field `renaming`・`project_confirm`・`show_archived` を足した（`Shell::new` は None・None・false）。名前の欄は `command` の 1 つを使う（`renaming` の doc）。
/// 契約（SESSIONS-LEFT-1101）: `continue_by_name` を `open_projects`（`UiSettings::open_projects` の写し）に替え、`sidebar_query`（sidebar の上の検索の文・保存しない）を足した。
/// 契約（SESSIONS-LEFT-1101 追記 244）: `folded_headings`（`UiSettings::folded_headings` の写し）を足した。
/// 契約（RQ3 から）: field `data_confirm` を足した（`Shell::new` は None・`go` で None）。
/// 契約（RQ4 から）: field `connecting`・`connection_confirm` を足した（`Shell::new` は None・`go` で None）。
/// Wave: UI-4a, UI-4b, UI-6a, RQ2, RQ3, RQ4
#[derive(Debug)]
pub struct Shell {
    pub view: View,
    /// この frame で sidebar を開いて描くか。2 つ目の真実にしない: app.rs が毎 frame の頭で `Tier::current(ctx).sidebar_open(store.ui.sidebar_hidden)` を入れ、
    /// `Intent::ToggleSidebar` の arm だけが `store.ui.sidebar_hidden` と一緒に書く。読み手 = `sidebar::draw`（false なら幅を 0 へ寄せ、0 では SidePanel を描かない）・
    /// `header::draw`（title line の button の tooltip）・`palette::commands`（`Hide sidebar` / `Show sidebar`）。
    pub sidebar_open: bool,
    /// UI-6a: 今が互換 view（`View::CompatCanvas`）なら旧 canvas の数・外なら None。2 つ目の真実にしない: 本体で app.rs が毎 frame の頭で
    /// `matches!(view, View::CompatCanvas(_)).then(|| store.sessions.len())` を入れる写し。読み手 = `palette::commands`（Duplicate / 削除の行・1 つなら理由の行）。
    pub canvases: Option<usize>,
    /// 1 つの欄の今の場所の打ちかけ（UI-4a から全 View で同じ欄。header が `TextEdit::singleline(&mut shell.command)` を字面で 1 回だけ書く = `d1_one_field`）。
    pub command: String,
    /// 下の panel の Chat の列の底に出す 1 行の返事（提案の Apply / Undo の結果など・捏造しない: 未実装なら未実装と言う）
    pub chat_note: Option<String>,
    pub palette: Palette,
    /// 欄ごとの打ちかけ（SPEC §2「Home に 1 つ・Project ごとに 1 つ」）。key = `View::place`（`"home"` = Home の作る欄・Project の id・
    /// `"search"` = Runs・Connections・Settings・CompatCanvas・App の探す欄で 1 つ）。今の場所の文は `command` に在り、ここには入れない（`Shell::go` が入れ替える）。
    /// 保証する: Project A で打ちかけた変更文が B の欄に出ない（B で Enter しても B に当たらない）。空の文は持たない。
    /// 再起動を跨ぐ保存（UI-4b）: `model::UiSettings::drafts` に同じ key で写す（書き手・読み手はその field の doc）。
    pub drafts: BTreeMap<String, String>,
    /// true の間、欄（`header::command_field`・Home は主領域）が次の frame で focus を置いて false に戻す（1 回だけ）。`Shell::new(View::Home)` と `Shell::go(View::Home)` が立てる
    /// （⌘N・⌘K の `New project` = `Go(View::Home)`・sidebar の `New chat` = `NewChat` → Home・sendback 17 / 35.10・keys.md §2）。
    /// 保証しない: OS の前面の app を変える事（egui の focus だけ・背景で起こした時も前面の app と keyboard focus は動かない = D-g）。
    pub focus_command: bool,
    /// PV1: Home の欄の下の発見の今（迷いの一言で app.rs が置く・`Close` と次の一言で消える・保存しない）。
    pub discovery: Option<super::discover::Discovery>,
    /// G104: Home の欄に添えた物（欄の下の小さな角丸の長方形・× で外す・HOME-ATTACH-1101）。Home で `Generate` を送った時に空にし、AI が前の依頼を読んでいて受けなかった時は app.rs が戻す
    pub attach: Vec<Attachment>,
    /// G104: 添えられなかった理由の 1 行（形式・大きさ・秘密・数・provider）。次に添えた時か送った時に消える
    pub attach_line: Option<String>,
    /// G104: `Effort ▸` の今の段（None = Default）。2 つ目の真実にしない: 起動で `UiSettings::default_effort` から・書き手は `Intent::SetEffort` の arm だけ
    pub effort: Option<Effort>,
    /// G104: 今の provider が受けられる物（app.rs が毎 frame `Generated::accepts` を入れる写し）
    pub accepts: Accepts,
    /// G84: `Model ▸` の今の別名（None = Default）。起動で `UiSettings::default_model` から・書き手は `Intent::SetModel` の arm だけ
    pub model: Option<String>,
    /// G84: `Provider ▸` の行（account〈種類と id の組・None = System default〉, 語, 使っている）。app.rs が毎 frame `UiSettings` から作る写し
    /// （`Claude Code`・Shibubu が足した plan の login・利用者が足した API key〈従量の印つき〉。環境の鍵〈EnvKey〉は出さない）
    pub providers: Vec<(Option<crate::model::AiAccount>, String, bool)>,
    /// 111 通目（L4）: `providers` の各行の本当の状態（同じ長さ・同じ順・`model::ProviderState::of`）。Composer の下の行は Ready 以外を `Ready` と言わない。書き手 = app.rs が毎 frame `providers` と一緒に
    pub provider_states: Vec<crate::model::ProviderState>,
    /// CHAT-ONE-1101: chat bar の下の行の folder の icon の横の名前（app.rs が毎 frame 写す: Home = この chat の folder〈`ShibubuApp::workspace`〉・
    /// Project = その Project の folder〈`Generated::folder_name`〉・無ければ None = icon だけ）。読み手 = `home::chat_bar`。
    pub folder: Option<String>,
    /// G86: 最後に下の panel を開いた Engine の card の鍵（`show_card`・Project の id → 鍵）。同じ card で畳んだ panel を開き直さない（その Project で進めている間 = None で忘れる =
    /// 次の run の同じ card は開く）。B34: Project ごとに覚える（Home や別の Project を挟んで戻っても、同じ card で開き直さない）
    pub card_seen: std::collections::BTreeMap<String, String>,
    /// REVIEW-R7 Y-c: 下の panel を開いた提案の鍵（`suggestion:{id}`・起動の間）。提案は Apply / Later / Hide まで居続けるので、Home を挟んで戻っても開き直さない
    pub suggestions_seen: std::collections::BTreeSet<String>,
    /// RQ2（RQ-005・Rename）: 名前を直している Project の id。Some の間は 1 つの欄が名前の欄になる（2 つ目の TextEdit を作らない = d1_one_field）:
    /// 書き手 = `⌄` と Home の行の `•••` の `Rename`（`renaming = Some(id); command = 今の名前; focus_command = true`）。その間 placeholder は `Name this project`・
    /// 右端は `↵ Rename`・Enter は `Intent::RenameProject { project: id, name: command.trim() }`・Esc と場所の移り（`go`）は取り消して前の打ちかけに戻す。
    /// 保証しない: 2 つの Project を同時に直す事（1 つだけ）。
    pub renaming: Option<String>,
    /// RQ2（RQ-005・Delete…）: 行内の確認を開いている Project の id（Home の行の下か、その Project の頭に `theme::confirm_inline`）。`Delete` で `Intent::DeleteProject`、
    /// `Keep`・Esc・場所の移りで None。書き手 = `•••` / `⌄` の `Delete…`（1 回目で消さない）と確認の 2 つの button と app.rs の Esc の連鎖。
    pub project_confirm: Option<String>,
    /// SESSIONS-LEFT-1101: sidebar で session と Page を開いた Project の id（`UiSettings::open_projects` の写し・app.rs が毎 frame の頭で入れる・
    /// 書き手は `Intent::FoldProject` の腕と Project へ移った時の `go`・`sidebar_open` と同じく 2 つ目の真実にしない）。
    pub open_projects: Vec<String>,
    /// SESSIONS-LEFT-1101 追記 244: sidebar で畳んだ見出しの鍵（`UiSettings::folded_headings` の写し・app.rs が毎 frame の頭で入れる・書き手は `Intent::FoldHeading` の腕だけ）。
    pub folded_headings: Vec<String>,
    /// SESSIONS-LEFT-1101（owner 198 の 5「searchも左sidebar」）: sidebar の上の検索の文（session・Project・Page の名前を絞る・保存しない）。
    pub sidebar_query: String,
    /// RQ2（RQ-005）: sidebar の Your projects に Archive した行も出すか（Your projects の見出しの `•••` の `Show archived · {n}` / `Hide archived · {n}` で開閉・保存しない）。
    pub show_archived: bool,
    /// RQ3（RQ-013・Delete my data…）: 行内の確認を開いている種類（Settings › Data & Recovery の行の下に `theme::confirm_inline` で `DataKind::effect`）。
    /// `Delete` で `Intent::DeleteMyData`、`Keep`・Esc（app.rs の Esc の連鎖）・場所の移り（`go`）で None。書き手 = 行の `Delete…` と確認の 2 つの button と Esc の連鎖。
    pub data_confirm: Option<DataKind>,
    /// RQ4（RQ-015〜RQ-017）→ CONNECTIONS-1101（owner 196 の 9・10「上の chatbar にカーソルが当たるだけなのやめて」）: Connections の行 `key` の下にその場で欄が開いている
    /// （文は `field`・上の欄 `command` には触らない）。書き手 = 行の `Connect`（`web` = URL）・`Add`（`mcp` = 名前と起こす command か README の JSON・`cli:<id>` =
    /// program を選んだ後の固定の引数）・`Paste token`（`connections::secret_key` = 鍵）。欄の Enter と主の button は `Intent::Connection { row: key, act: ConnAct::Submit }`
    /// （鍵は `PasteSecret`・文は Intent に載せない）・`×`・Esc・場所の移り（`go`）は閉じて文を捨てる。
    pub connecting: Option<String>,
    /// RQ4（RQ-015・RQ-017・`Remove…`）: 行内の確認を開いている行の key（`connections::draw` が行の下に `theme::confirm_inline`）。`Remove` で
    /// `Intent::Connection { row, act: ConnAct::Remove }`、`Keep`・Esc・場所の移りで None。Trusted command を足す時の確認（program・引数・fingerprint を見せる）もこの欄。
    pub connection_confirm: Option<String>,
    /// CONNECTIONS-1101（owner 196 の 9・10）: `connecting` の行の下にその場で開く欄の文（Web の URL・MCP の command か JSON・Trusted command の引数・
    /// `connections::secret_key` の鍵）。上の欄（`command`）とは別 = 鍵を上の欄に描かない・Debug に出さない（`FieldText`）。書き手 = `connections::draw` の行の下の欄だけ。
    pub field: FieldText,
    /// CONNECTIONS-1101: MCP の Add の form の名前（空 = command から決める・`McpConfig::add_named`）
    pub field_name: String,
    /// L2（109 通目 15）: Home で Project を作る前の会話（この起動の間だけ。sqlite には書かない）。人の一言と Shibubu の問い・選択肢・案。
    pub talk: Vec<crate::domain::chat::Turn>,
    /// SESSIONS-LEFT-1101: 今の Home の会話の保存の key（`chat::HOME_CHAT` の後ろに id・最初の一言で作る・`New chat` で None）。
    /// Home の一言と答えはこの key で `chat_turns` に保存する（待つ 1 行は保存しない）= 左の一覧に出て、再起動の後も開ける。
    pub talk_id: Option<String>,
    /// PASSWORD-1101 の 3: chat に打った password らしき物（`chat::looks_like_password`）と保存先（繋いだ 1Password が在ればそれ・無ければ Keychain）。
    /// AI・会話・DB・画面に出さず、card の `Save to {manager}` か `Delete` まで memory の上だけに持つ（`vault::Secret` = Debug は伏せ字・Drop で 0）。
    /// 書き手 = app.rs の `apply` の 1 か所。読み手 = `agent_rail::Card::password`（manager の名前だけ）と app.rs の保存の腕。
    pub held: Option<(crate::domain::vault::Secret, crate::domain::vault::Manager)>,
    /// PASSWORD-1101 の 1: chat で名指しした manager を繋ぐ card（manager・この Mac で CLI が見つかったか・待ちか誤りの 1 行〈空 = まだ押していない〉）。
    /// 繋げた・`Not now` で None。書き手 = app.rs（`Manager::asked` の一言と sign in の答え）。
    pub vault_ask: Option<(crate::domain::vault::Manager, bool, String)>,
    /// PASSWORD-1101 の 2: Connections の鍵の欄の下に出す、繋いだ manager の項目（名前と参照だけ・値は無い）。欄を開いた時に app.rs が裏の thread で読む。
    pub vault_items: Vec<crate::domain::vault::SecretRef>,
}

/// CONNECTIONS-1101: 行の下の欄の文。鍵（Paste token）も入るので Debug は字数だけ（`Shell` の `#[derive(Debug)]` から値を出さない）。
/// 例: `let url = shell.field.0.trim().to_owned();`・`std::mem::take(&mut shell.field.0)`（鍵は取ったら残さない）。
/// 新しい型の理由: 既存の String の欄（`command`）は Debug にも上の欄にもそのまま出る
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FieldText(pub String);

impl std::fmt::Debug for FieldText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FieldText({} chars)", self.0.chars().count())
    }
}

impl Shell {
    pub fn new(view: View) -> Self {
        Shell {
            focus_command: view == View::Home,
            view,
            sidebar_open: true,
            canvases: None,
            command: String::new(),
            chat_note: None,
            palette: Palette::default(),
            drafts: BTreeMap::new(),
            discovery: None,
            attach: Vec::new(),
            attach_line: None,
            effort: None,
            accepts: Accepts::default(),
            model: None,
            providers: Vec::new(),
            provider_states: Vec::new(),
            folder: None,
            card_seen: Default::default(),
            suggestions_seen: Default::default(),
            renaming: None,
            project_confirm: None,
            open_projects: Vec::new(),
            folded_headings: Vec::new(),
            sidebar_query: String::new(),
            show_archived: false,
            data_confirm: None,
            connecting: None,
            connection_confirm: None,
            field: FieldText::default(),
            field_name: String::new(),
            talk: Vec::new(),
            talk_id: None,
            held: None,
            vault_ask: None,
            vault_items: Vec::new(),
        }
    }
    /// 場所を替える（palette を閉じる）。契約（UI-4a から）: 場所の key（`drafts` の doc）が替わる時、今の `command` を `drafts[今の key]` に預け
    /// （空なら key ごと消す）、`drafts[行き先の key]` を `command` に戻す（無ければ空）。同じ key の中の移り（同じ Project の Page・Runs ↔ Connections）は欄をそのまま。
    /// 行き先が Home なら `focus_command = true`。例: `shell.command = "add a Tests page".into(); shell.go(View::Home); assert!(shell.command.is_empty());`
    /// 保証しない: 打ちかけを disk に書く事（`go` は書かない・app.rs が `UiSettings::drafts` に写して保存する = UI-4b）。
    /// 契約（FX4・署名は変えない・本体で足す）: ⌘K が開いていれば、drafts を入れ替える前に `palette.stash` を `command` へ戻して閉じる
    /// （⌘K で打った語を打ちかけとして預けない・⌘K の前の打ちかけを元の場所の key へ預ける）。
    /// Wave: UI-4a, FX4
    pub fn go(&mut self, view: View) {
        // RQ2: 場所の移りは名前の欄と行内の確認を取り消す（前の打ちかけに戻してから預ける）
        self.end_rename();
        self.project_confirm = None;
        self.data_confirm = None;
        // RQ4: Connections の文の欄と行内の確認も取り消す（欄の文は end_rename と同じく前の打ちかけに戻す）
        self.end_connecting();
        self.connection_confirm = None;
        // ⌘K の間に打った語は捨て、⌘K の前の打ちかけを欄へ戻してから預ける
        if let Some(before) = self.palette.stash.take() {
            self.command = before;
        }
        if self.view.place() != view.place() {
            let typed = std::mem::take(&mut self.command);
            let from = self.view.place().to_owned();
            if typed.trim().is_empty() {
                self.drafts.remove(&from);
            } else {
                self.drafts.insert(from, typed);
            }
            self.command = self.drafts.remove(view.place()).unwrap_or_default();
        }
        self.focus_command |= view == View::Home;
        self.view = view;
        self.palette.open = false;
    }
    /// 1 つの欄で Enter（`cmd_enter` = ⌘↵・右端の button も同じ）を押した時に送る Intent。純関数（書かない・欄を空にしない）。
    /// 例（header）: `if submit && !busy { if let Some(i) = shell.enter_intent(cmd, apps) { if matches!(i, Intent::Generate(..) | Intent::Talk(..)) { shell.command.clear(); } out = Some(i); } }`
    /// 保証する: `command.trim()` が空なら None。⌘↵ は Home と Project で `Talk(text)`（L2・名前の一致も開かない）・他で `Generate(text)`。Enter は場所で決まる: Home = 名前が text と同じ（大小文字を見ない）行が `apps` に在れば
    /// `Go(その行の view)`・無ければ `Talk(text)`（L2・作らない）／Project = `Talk(text)`（CHAT-ONE-1101: 会話で答え、変更は案の Accept で `PageIntent::Edit`）／Runs・Connections・CompatCanvas・App =
    /// `palette::commands(self, text, apps, &[])` の頭の行（`Generate` の fallback を除く）・無ければ None（表 copy.en.md:30・:52）。text は trim した物。
    /// 保証しない: busy の時に送らない事（呼ぶ側の門: 欄を灰にする・`create_app` は欄へ戻す・`apply` の Edit は model が読む間 return）・IME の変換中の Enter（UI-4a は今の `lost_focus && Enter` のまま）。
    /// CONNECTIONS-1101: Connections の行の下の欄（`connecting`）は自分の Enter で送る（上の欄の Enter は今まで通り = 行の下の欄を送らない）。
    /// Wave: UI-4a, RQ4
    pub fn enter_intent(&self, cmd_enter: bool, apps: &[AppRow]) -> Option<Intent> {
        let text = self.command.trim();
        // 空は名前の欄の間も None（`command_field` が何を打てばいいかの 1 行 = `Intent::Notice` を出す・`rename_project` の空の拒否は 2 つ目の門）
        if text.is_empty() {
            return None;
        }
        // RQ2: 名前の欄の間は Enter も ⌘↵ も名前を替えるだけ
        if let Some(project) = &self.renaming {
            return Some(Intent::RenameProject { project: project.clone(), name: text.to_owned() });
        }
        // L2（111 通目・handoff 00 #15）: Home の ⌘↵ は名前の一致も開かず送る（会話）だけ。Home から直に作る道は無い（作るのは会話の案の `Build now`）
        // G104: 添えた物は Home の欄の物で、作る時に乗せるのは案の `Build now`。Home の外の ⌘↵ には乗せない
        // CHAT-ONE-1101: Project の Chat も Home と同じ会話（⌘↵ も Enter も送る・変えるのは会話の案の Build now）
        if cmd_enter {
            return Some(if matches!(self.view, View::Home | View::Project { .. }) { Intent::Talk(text.to_owned()) } else { Intent::Generate(text.to_owned(), Vec::new()) });
        }
        match &self.view {
            View::Home => Some(self.home_match(apps).map_or_else(|| Intent::Talk(text.to_owned()), |row| Intent::Go(row.view.clone()))),
            View::Project { .. } => Some(Intent::Talk(text.to_owned())),
            View::Runs | View::Connections | View::Settings(_) | View::App(_) | View::CompatCanvas(_) => {
                // 押せない行（canvas が 1 つの時の削除の理由）は欄の Enter でも走らせない（反証の直し: 開いた確認窓の Delete が何もせずに閉じた）
                let rows = super::palette::commands(self, text, apps, &[]).into_iter().filter(|c| !super::palette::disabled(c));
                rows.map(|c| c.intent).find(|i| !matches!(i, Intent::Generate(..)))
            }
        }
    }
    /// Home の欄の文と名前が同じ（前後の空白と大小文字を見ない）Project の行。
    fn home_match<'a>(&self, apps: &'a [AppRow]) -> Option<&'a AppRow> {
        let text = self.command.trim().to_lowercase();
        apps.iter().find(|a| !text.is_empty() && a.name.trim().to_lowercase() == text)
    }
    /// 欄の右端の 1 語（押す前に Enter の結果を書く・SPEC §7・表 copy.en.md:32・:52・:64）。`enter_intent(false, apps)` と同じ判定から作る。
    /// 保証する: Home = `↵ Send`（L2・名前が同じ行が在れば `↵ Open “{name}”`）／Project = `↵ Send`（CHAT-ONE-1101）／Runs・Connections・CompatCanvas・App = 欄が空なら空文字・
    /// 合う行が Project か app を開く行（`Go(View::Project{..} | View::App(_) | View::CompatCanvas(_))`）なら `↵ Open`・他の command なら `↵ Run`・
    /// 無ければ `No command matches · ⌘↵ to create a project from this`。
    /// 呼ぶ側: 文が `↵` か `Create` で始まる時は button（`Create` で始まる物だけ primary・他は Ghost・押すと Enter と同じ・AX の名前は鍵の記号を除いた語
    /// `Send`・`Open “{name}”`・`Open`・`Run`）、他は text_tertiary の文字。保証しない: 表の EN の照合（本体の門 `ui-strings --by en`）。
    /// 契約（FX4〈FX-020・UIUX H05〉・署名は変えない・本体で足す）: Enter と ⌘↵ の結果が違う所は ` · ⌘↵ …` を後ろに足す =
    /// Runs / Connections / CompatCanvas / App の合う行 `↵ Open · ⌘↵ New project` / `↵ Run · ⌘↵ New project`（L2: Home と Project〈CHAT-ONE〉は ⌘↵ も送るので付けない）。
    /// 同じ所（合う行が無い時の `No command matches · ⌘↵ to create a project from this`・空の欄の空文字）は今のまま。
    /// AX の名前（` · ` の前・`↵ ` を除いた語 = `Send`・`Open “{name}”`・`Open`・`Run`）。L2（111 通目）で Home の `Create` は `Send` に替えた（scripts/qa/bg/labels.py の `create` と台本の手 1 は operator）。
    /// 保証しない: 欄の右端の幅（`header::TAIL_MAX_W` = i8 の margin）に入りきる事（置き方は本体・押せる範囲と AX の名前は変えない）・⌘K の list が開いている間の右端（list の選んだ行が Enter の結果）。
    /// Wave: UI-4a, FX4
    pub fn enter_label(&self, apps: &[AppRow]) -> String {
        if self.renaming.is_some() {
            return "↵ Rename".to_owned();
        }
        match &self.view {
            // L2: Home の右端は送る（作らない）。名前の一致は開く
            View::Home => self.home_match(apps).map_or_else(|| "↵ Send".to_owned(), |row| format!("↵ Open “{}”", row.name.trim())),
            View::Project { .. } => "↵ Send".to_owned(),
            View::Runs | View::Connections | View::Settings(_) | View::App(_) | View::CompatCanvas(_) => match self.enter_intent(false, apps) {
                _ if self.command.trim().is_empty() => String::new(),
                Some(Intent::Go(View::Project { .. } | View::App(_) | View::CompatCanvas(_))) => "↵ Open · ⌘↵ New project".to_owned(),
                Some(_) => "↵ Run · ⌘↵ New project".to_owned(),
                None => "No command matches · ⌘↵ to create a project from this".to_owned(),
            },
        }
    }
    /// RQ2（RQ-005・Rename）: 1 つの欄を名前の欄にする（今の打ちかけを `drafts` に預け、欄に今の名前・focus）。行内の確認は閉じる。
    /// 例（`⌄` と Home の行の `•••` の `Rename`）: `shell.start_rename(project, row.name.clone())`。
    pub fn start_rename(&mut self, project: String, name: String) {
        if self.renaming.is_none() {
            let typed = std::mem::take(&mut self.command);
            if !typed.trim().is_empty() {
                self.drafts.insert(self.view.place().to_owned(), typed);
            }
        }
        (self.renaming, self.command, self.focus_command, self.project_confirm) = (Some(project), name, true, None);
    }
    /// RQ2: 名前の欄を閉じ、預けた打ちかけを欄に戻す（Esc・名前を替えた後・場所の移り）。閉じたら true（1 回の Esc で閉じるのは 1 つ）。
    pub fn end_rename(&mut self) -> bool {
        if self.renaming.take().is_none() {
            return false;
        }
        self.command = self.drafts.remove(self.view.place()).unwrap_or_default();
        true
    }
    /// RQ2（RQ-005）: `Delete…` の行内の確認（`project_confirm` が `project` の時だけ描く・Home の行の下と Project の頭）。`Delete` → `Intent::DeleteProject`・
    /// `Keep` → 閉じる。`name` は確認を描く時だけ読む（app.rs は DB の題を引く）。
    pub fn confirm_delete(&mut self, ui: &mut egui::Ui, project: &str, name: impl FnOnce() -> String) -> Option<Intent> {
        if self.project_confirm.as_deref() != Some(project) {
            return None;
        }
        match theme::confirm_inline(ui, &format!("Delete “{}”? Its pages and history are deleted. The folder and your files stay.", name())) {
            Some(true) => Some(Intent::DeleteProject(project.to_owned())),
            Some(false) => {
                self.project_confirm = None;
                None
            }
            None => None,
        }
    }
    /// RQ2: 名前の欄の placeholder（名前の欄の間は `Name this project`・他は場所の物）。AX の名前（`View::field_name`）は替えない（BG の driver が引く名前）。
    pub fn field_hint(&self) -> &'static str {
        match &self.renaming {
            Some(_) => "Name this project",
            None => self.view.field_hint(),
        }
    }
    /// RQ4 → CONNECTIONS-1101: Connections の行 `key` の下に欄を開く（空の欄・focus は欄が最初の frame に取る・行内の確認は閉じる）。上の欄（`command`・`drafts`・
    /// `focus_command`）には触らない（前は上の欄を文の欄にして focus を移していた = owner 196 の 10）。
    pub fn start_connecting(&mut self, key: String) {
        (self.field, self.field_name) = (FieldText::default(), String::new());
        self.vault_items.clear();
        (self.connecting, self.connection_confirm) = (Some(key), None);
    }
    /// RQ4 → CONNECTIONS-1101: 行の下の欄を閉じて文を捨てる（`×`・Esc・送れた後・場所の移り）。閉じたら true。
    pub fn end_connecting(&mut self) -> bool {
        (self.field, self.field_name) = (FieldText::default(), String::new());
        self.vault_items.clear();
        self.connecting.take().is_some()
    }
    /// G86 × L1-NORAIL（review B）: 今の Project の card の鍵（app.rs が毎 frame `engine_view::card_key` から作る・進めている間と結果の 1 行は None）。新しい card
    /// （承認・決める・同意・Ask・結果）が出た frame だけ true = app.rs が下の panel を開く（`PanelAct::Reveal`・選んでいる tab のまま・欄の focus も Terminal の focus も奪わない = AGT-006・R1）。
    /// 窓の幅に依らない（前の狭い段の枝は右の列が Page を隠すための物で、下の panel では要らない）。同じ鍵の間は開き直さない（利用者が畳めば畳んだまま）。
    /// Engine の card はその都度の出来事（鍵は文だけ）なので、進めている間（None）に忘れる = 次の run の同じ結果は開く。提案（`suggestion:`）は居続けるので、
    /// 一度開いた鍵は Home に出て戻っても開き直さない（REVIEW-R7 Y-c）。
    /// B34（NORAIL review N1）: `project` = 今描いている Project（Project の外では呼ばない）。Engine の card の鍵は Project ごとに覚え、None で忘れるのはその Project の鍵だけ
    /// （前は 1 つだけ覚え、Home や別の Project の None の frame で忘れて、戻るたびに同じ承認の card で畳んだ panel を開き直した）。
    /// 例: `if shell.show_card(&project, key) { generated.panel_act(&project, PanelAct::Reveal) }`
    pub fn show_card(&mut self, project: &str, key: Option<String>) -> bool {
        let Some(k) = key else {
            self.card_seen.remove(project);
            return false;
        };
        if k.starts_with("suggestion:") {
            self.card_seen.remove(project);
            return self.suggestions_seen.insert(k);
        }
        self.card_seen.insert(project.to_owned(), k.clone()).as_ref() != Some(&k)
    }
}

fn view_change_id() -> egui::Id {
    egui::Id::new("kangae.view.changed_at")
}

/// 今の View を覚える（header が毎 frame 最初に呼ぶ）。替わった frame の時刻が主領域の fade + slide と行の stagger の起点になる。
pub fn note_view(ctx: &egui::Context, view: &View) {
    motion::changed_at(ctx, view_change_id(), view);
}

/// 今の View になった時刻（ctx の time）。起動直後の View は遠い過去（= 動かさない）。
pub fn view_since(ctx: &egui::Context) -> f64 {
    motion::last_changed(ctx, view_change_id())
}

/// 主領域の差し込み口。gutter（§1.1）・max text column 1080・縦 scroll・min-width 0 をここで決め、
/// 中身は `draw` に任せる。renderer（unit R）は `View::App(id)` の時にこれへ AppDefinition を描く。
/// View が替わった直後は中身を fade + 8px の slide で出す（§15.2-1・起点は `note_view`）。
pub fn draw_app_region(ui: &mut egui::Ui, draw: &mut dyn FnMut(&mut egui::Ui)) {
    draw_region(ui, theme::MAX_TEXT_COLUMN, draw)
}

/// `draw_app_region` の列の最大幅を呼ぶ側が決める形。LAYOUT（operator 10-31 の門 2）: 本物の app・Web の面の在る Page は `f32::INFINITY`
/// （REAPER の列が窓の幅まで使う・文の列の 1080 で止めない）・他は `theme::MAX_TEXT_COLUMN`。
/// 例: `shell::draw_region(ui, if g.wide_page(&project, &page) { f32::INFINITY } else { theme::MAX_TEXT_COLUMN }, &mut |ui| { … })`
pub fn draw_region(ui: &mut egui::Ui, max_w: f32, draw: &mut dyn FnMut(&mut egui::Ui)) {
    let tier = Tier::current(ui.ctx());
    let mut area = egui::ScrollArea::vertical().id_salt("kangae-main-region").auto_shrink([false, false]);
    // G92: View が替わった frame（別の Project・Page を開いた）は上から出す。送り量は全部の View で 1 つなので、持ち越すと
    // 前の Page で Terminal まで送った量のまま（新しい Page の下端で止まる）開き、tab の帯・＋・••• が窓の外
    let ctx = ui.ctx();
    if view_since(ctx) == ctx.input(|i| i.time) {
        area = area.vertical_scroll_offset(0.0);
    }
    let out = area.show(ui, |ui| {
        let avail = ui.available_width();
        let width = (avail - 2.0 * tier.gutter).max(0.0).min(max_w);
        let left = ((avail - width) / 2.0).max(tier.gutter);
        ui.add_space(theme::SP_7);
        ui.horizontal_top(|ui| {
            ui.add_space(left);
            ui.vertical(|ui| {
                ui.set_width(width);
                // 殻の骨格（skeleton::paint_shell）が本物と同じ列に帯を置けるよう、scrollbar の分を引いた後の列を残す
                super::skeleton::note_main_column(ui);
                let t0 = view_since(ui.ctx());
                motion::fade_slide_in(ui, "kangae.view", t0, |ui| draw(ui));
            });
        });
        ui.add_space(theme::SP_9);
    });
    ui.ctx().data_mut(|d| d.insert_temp(main_scrolled_id(), out.state.offset.y));
}

fn main_scrolled_id() -> egui::Id {
    egui::Id::new("kangae-main-scrolled")
}

/// G92: 主領域（`draw_app_region`）を下へ送った量（前の frame の終わり・送っていない / 主領域の外は 0）。窓の残りの高さで大きさを決める部品
/// （Mirror の枠・Terminal の欄の行数）は、送る前の位置で残りを測る（送ると残りが増えて部品が伸び、見せたい所がまた窓の外へ逃げる）。
pub fn main_scrolled(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(main_scrolled_id())).unwrap_or(0.0)
}

/// 見出し（page の題 = `Type::Title` 24 太字〈UI-SWEEP-1101〉+ 1 行説明 13 + 右に primary CTA 最大 1 つ・§1.1「情報は 24px 後から」）。
/// 呼ぶ所 = Connections・Runs・Settings・中身の無い Project の Page・生成 app の頭（どの page も同じ段）。
pub fn content_opening(ui: &mut egui::Ui, title: &str, lead: &str, cta: Option<&str>) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut clicked = false;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            theme::heading(ui, Type::Title, title, t.text_primary);
            if !lead.is_empty() {
                ui.add_space(theme::SP_1);
                theme::label(ui, Type::Nav, lead, t.text_secondary);
            }
        });
        if let Some(label) = cta {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                clicked = theme::button(ui, label, theme::Variant::Primary).clicked();
            });
        }
    });
    ui.add_space(theme::SP_6);
    clicked
}

/// RQ-USAGE（operator ①）: まだ使用量を読んでいない時の帯の `Show usage` の hover（押すと Keychain の Claude Code の sign in を読む = 許可の窓が出得る事を先に言う）。
pub const USAGE_ASK: &str = "Plan usage · reads Claude's sign-in from Keychain";

/// STATUSBAR-1101（owner 193 通目「show usageやcouldn't switchなどはorcaのように一番下にbarを作り見れるように…大きさは小さく…refresh usage dataを回転マークで」）:
/// 窓の一番下の細い帯 1 本に描く物（app.rs が毎 frame 作る写し）。新しくした理由: 前の `execution_strip(ctx, running, current)` の 2 つに、AI・使用量・状態の 1 行が
/// 加わり、同じ帯 1 本にまとめた（Runs の数も同じ帯）。
pub struct StatusBar<'a> {
    /// 今の AI の短い名前（`Generated::ai_name`）。押すと `providers` の menu（上に開く）
    pub ai: &'a str,
    /// `Shell::providers`（account, 語, 使っている）。選ぶと `Intent::UseAiAccount`
    pub providers: &'a [(Option<crate::model::AiAccount>, String, bool)],
    /// 今の AI の使用量の 1 行と次の一手（app.rs の `usage_view(true)`）。None = 読めない AI か読まない（試験の `open`）・1 行が `USAGE_ASK` = まだ読んでいない
    /// （今の AI は `Show usage` だけ・説明は hover）。1 行は今の AI の hover（`updated N ago` まで）・次の一手は今の AI の隣
    pub usage: Option<(String, Option<(&'static str, Intent)>)>,
    /// STATUSBAR-1101 追記 210: つながっている AI 全部（`providers` の順・今の AI が先頭）。空 = 使用量の席を出さない（試験の `open`）
    pub ais: Vec<BarAi>,
    /// 使用量を読んでいる（↻ の代わりに回る印）
    pub reading: bool,
    /// 走っている run の数（0 = 出さない・押すと Runs）
    pub running: usize,
    /// 最後の状態の 1 行（app.rs の `message`・空 = 出さない）。右寄せ・帯の幅で切り、hover で全文・押すと帯の上に全文の card（× で閉じる）
    pub status: &'a str,
    /// PANEL-SYNC-1101: 畳んだ panel の tab（帯の左寄り・`Generated::folded`）。None = 開いている（下でも右でも）か panel の無い View
    pub folded: Option<crate::ui::terminal_panel::Folded>,
}

/// STATUSBAR-1101 追記（owner 210「providerのusageとかそこら辺が…orcaを参考に。下に置かれているよね」）: 帯に並べる AI 1 つ（app.rs の `bar_ais` が作る・ui は読み方を知らない）。
/// 読めた = 窓ごとに `("Session", "42% used 2h 10m", 42)`・読めない = `windows` が空で `why` が hover の 1 行（作り話の数字を出さない）・`failed` = 読みに失敗した（`⚠ Refresh failed`）。
/// 例: `BarAi { name: "Claude Code".into(), key: "claude".into(), active: true, windows: vec![("Session".into(), "42% used 2h 10m".into(), 42)], why: String::new(), failed: false }`
/// 新しくした理由: `StatusBar` の欄は今の AI 1 つの物（`usage`）だけで、AI ごとの窓と読めない理由を持つ物が無い。
#[derive(Debug, Clone, PartialEq)]
pub struct BarAi {
    pub name: String,
    /// PROVIDER-ICONS-1101（owner 248「usageのところにもそれぞれのproviderのicon入れて」）: 印の key（`AiAccount::provider` = `claude`・`grok`・`ollama` 等・
    /// `theme::provider_icon` が引く）。前の頭文字の四角（`ai_badge`）を替える
    pub key: String,
    /// 今使っている AI（次の一手と `Show usage` はこの AI の隣だけ）
    pub active: bool,
    pub windows: Vec<(String, String, u8)>,
    pub why: String,
    pub failed: bool,
}

/// USAGE-OWN-1101（owner 203 通目）: Settings › AI & Models の「Used in Shibubu」の文字（`own_usage::Counter::panel` が作る・ui は数え方を知らない）。
/// 表（今日・5 時間の枠・今週）→ 今週の内訳（AI と account → Project › 何のため）→ いつ数えたか・`Recount`・読めなかった物の 1 行。
/// 追記 223（owner「shibubu 0 request todayって何？…表示いらない」）: 下の帯の `Shibubu · N requests today` と、押すと開いた panel は外した（Settings に残す）。
/// 新しくした理由: `StatusBar` の他の欄は 1 行か窓の %で、期間の表と内訳を持つ物が無い。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnPanel {
    /// 表。1 行目 = 見出し（空・Today・5-hour window・This week）・以下 = 行の名前と 3 つの数
    pub table: Vec<[String; 4]>,
    /// 表の下の注（回数の数え方・API 換算は請求ではない）
    pub notes: Vec<String>,
    /// 今週の内訳: (AI と account の見出し, 見出しの印の provider の key〈`theme::provider_icon`・PROVIDER-ICONS-1101〉, [(Project › 何のため, 数の 1 行)])
    pub groups: Vec<(String, String, Vec<(String, String)>)>,
    /// `Counted 20s ago`・数えている最中は `Counting…`
    pub counted: String,
    /// 読めなかった物の 1 行ずつ（理由と次の一手）
    pub problems: Vec<String>,
}

/// STATUSBAR-1101: 窓の一番下の細い帯（`theme::STATUS_H`・字は `Type::Small`・地は sidebar と同じ・窓の幅いっぱい = sidebar より先に取る・畳んでも在る）。
/// 左から 今の AI ▾ → つながっている AI ごとに印と使用量（窓ごとに細い棒と `42% used 2h 10m`・読めない = `--`・失敗 = `⚠ Refresh failed`・
/// 幅が足りなければ右から `+N` に畳む）→ ↻（AX `Refresh usage`・全部を読み直す・読んでいる間は回る印）→ `▶ N running` → 右寄せで状態の 1 行 → 右端に歯車
/// （追記 229・AX `Settings · ⌘,`・押すと Settings）。
/// 返り値 = 押した手（AI の切り替え・`RefreshUsage`・使用量の次の一手・`Go(Runs)`・歯車の `Go(Settings(General))`）。例（app.rs・header の次・sidebar の前）: `if let Some(i) = shell::status_bar(ctx, &bar) { intents.push(i) }`
/// 保証しない: 帯の幅に全部が入る事（状態の 1 行が先に切れる・全文は hover と押した card）。
pub fn status_bar(ctx: &egui::Context, bar: &StatusBar) -> Option<Intent> {
    let t = theme::tokens(ctx);
    let mut intent = None;
    let full_id = egui::Id::new(("kangae.status.full", bar.status));
    let mut full = !bar.status.is_empty() && ctx.data(|d| d.get_temp::<bool>(full_id)).unwrap_or(false);
    egui::TopBottomPanel::bottom("kangae-status-bar")
        .exact_height(theme::STATUS_H)
        .show_separator_line(true)
        .frame(egui::Frame::new().fill(t.surface_1).inner_margin(egui::Margin::symmetric(theme::SP_2 as i8, 0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = theme::SP_1;
                // PANEL-SYNC-1101: 畳んだ panel の tab（左寄り・押すと下に開く = `terminal_panel::folded_tabs`）
                if let Some(a) = bar.folded.as_ref().and_then(|f| super::terminal_panel::folded_tabs(ui, f)) {
                    intent = Some(Intent::OpenBelow(a));
                }
                // PROVIDER-ICONS-1101: 今の AI の語の左とmenu の行の左にその provider の印（account の無い行 = System default = Claude）
                let key_of = |acct: &Option<crate::model::AiAccount>| acct.as_ref().map_or("claude", |a| a.provider()).to_owned();
                let now = bar.providers.iter().find(|p| p.2).map_or_else(|| "claude".to_owned(), |p| key_of(&p.0));
                let ai = bar_glyph(ui, &format!("{} {}", bar.ai, theme::icon_glyph(Icon::ChevronDown)), None, Some(now.as_str()), bar.ai).on_hover_text("Choose the AI");
                if let Some(i) = super::terminal_panel::strip_menu(ui, &ai, |ui| {
                    bar.providers.iter().find_map(|(acct, label, on)| {
                        theme::provider_menu_item(ui, &key_of(acct), label, if *on { theme::icon_glyph(Icon::Check) } else { "" }).clicked().then(|| Intent::UseAiAccount(acct.clone()))
                    })
                }) {
                    intent = Some(i);
                }
                // STATUSBAR-1101 追記 210: つながっている AI を全部（icon ＋ 窓ごとの `42% used 2h 10m`・読めない AI は `--` と hover の理由）。
                // 幅が足りなければ右から畳んで `+N`（押すと上に全部）。走っている数と状態の 1 行の席は先に取っておく
                let ask = bar.usage.as_ref().is_some_and(|(line, _)| line == USAGE_ASK);
                let mut room = ui.available_width() - 3.0 * theme::STATUS_H - (if bar.running > 0 { 120.0 } else { 0.0 }) - (if bar.status.is_empty() { 0.0 } else { 160.0 });
                let mut shown = 0;
                for a in &bar.ais {
                    let w = ai_width(ui, a, bar, ask);
                    if shown > 0 && w > room {
                        break;
                    }
                    room -= w;
                    shown += 1;
                }
                for a in &bar.ais[..shown] {
                    ui.add_space(theme::SP_2);
                    if let Some(i) = ai_usage(ui, a, bar, ask) {
                        intent = Some(i);
                    }
                }
                if shown < bar.ais.len() {
                    ui.add_space(theme::SP_2);
                    let more = bar_word(ui, &format!("+{}", bar.ais.len() - shown), "More AI usage").on_hover_text("Usage of every AI");
                    let popup = more.id.with("all-ai");
                    if more.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(popup));
                    }
                    let picked = egui::popup::popup_above_or_below_widget(ui, popup, &more, egui::AboveOrBelow::Above, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(240.0);
                        let mut out = None;
                        for a in &bar.ais {
                            if let Some(i) = ui.horizontal(|ui| ai_usage(ui, a, bar, ask)).inner {
                                out = Some(i);
                            }
                        }
                        out
                    });
                    if let Some(Some(i)) = picked {
                        intent = Some(i);
                    }
                }
                // ↻ = 全部を読み直す（押した時だけ・hover では読まない）。今の AI の次の一手（`Try again`・`Sign in`）が在る時は出さない（同じ手を 2 つ並べない）
                match &bar.usage {
                    Some((line, None)) if line != USAGE_ASK && bar.reading => {
                        ui.add(egui::Spinner::new().size(Type::Small.size())).on_hover_text("Reading usage");
                    }
                    Some((line, None)) if line != USAGE_ASK => {
                        if bar_word(ui, theme::icon_glyph(Icon::Refresh), "Refresh usage").on_hover_text("Refresh usage").clicked() {
                            intent = Some(Intent::RefreshUsage);
                        }
                    }
                    _ => {}
                }
                if bar.running > 0 {
                    ui.add_space(theme::SP_2);
                    // ICONS-1101 追記 238: 走っている数の前に回る印（▶ の字をやめた・押せるのは語）
                    let size = Type::Small.size();
                    let (_, at) = ui.allocate_space(egui::vec2(size, size));
                    super::states::paint_mark(ui.painter(), super::states::RegionState::Running, at.center(), size, t.text_secondary);
                    if bar_word(ui, &format!("{} running", bar.running), "Runs").on_hover_text("Open Runs").clicked() {
                        intent = Some(Intent::Go(View::Runs));
                    }
                }
                // STATUSBAR-1101 追記 229（owner「settingがどこにあるかもわかりずらい…歯車マークを一番下のusageとかあるところの右下に置いて」）:
                // 右端に歯車（⌘, と app menu の Settings… と ⌘K の Settings と同じ所へ）→ その左に状態の 1 行
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ICONS-1101: 歯車 = `Icon::Settings`（Settings の nav の icon と同じ）
                    let gear = bar_glyph(ui, "", Some(View::Settings(SettingsSection::General).nav_icon()), None, SETTINGS_GEAR);
                    if gear.on_hover_text(SETTINGS_GEAR).clicked() {
                        intent = Some(Intent::Go(View::Settings(SettingsSection::General)));
                    }
                    if !bar.status.is_empty() {
                        let ink = if super::toast::sounds_failed(bar.status) { t.danger } else { t.text_secondary };
                        let line = ui.add(egui::Label::new(Type::Small.text(bar.status).color(ink)).truncate().sense(egui::Sense::click()));
                        if line.on_hover_text(bar.status).clicked() {
                            full = !full;
                        }
                    }
                });
            });
        });
    // 長い状態の 1 行の全文（帯の右上に浮く card・× で閉じる・文が替われば閉じた形から）
    if full {
        let bottom = ctx.screen_rect().bottom() - theme::STATUS_H - theme::SP_2;
        egui::Area::new(egui::Id::new("kangae.status.full.area")).order(egui::Order::Foreground).pivot(egui::Align2::RIGHT_BOTTOM).fixed_pos(egui::pos2(ctx.screen_rect().right() - theme::SP_4, bottom)).show(ctx, |ui| {
            // 追記 213: × は card の右上の角（右上の知らせと同じ部品）
            if super::toast::card(ui, 420.0, |ui| {
                ui.add(egui::Label::new(Type::Body.text(bar.status).color(t.text_primary)).wrap());
            })
            .1
            {
                full = false;
            }
        });
    }
    ctx.data_mut(|d| d.insert_temp(full_id, full));
    intent
}

/// USAGE-OWN-1101 × ACCOUNTS-1101 2 段目（作る物 6「帯の hover と Settings の内訳で同じ数字」）: 「Used in Shibubu」の表と注と今週の内訳（AI の account →
/// Project › 何のため）。Settings › AI & Models が描く（追記 223 で帯の panel は外した）。例: `shell::own_usage_breakdown(ui, own)`
/// Wave: ACCOUNTS-1101
pub fn own_usage_breakdown(ui: &mut egui::Ui, own: &OwnPanel) {
    let t = theme::tokens(ui.ctx());
    theme::heading(ui, Type::BodyStrong, "Used in Shibubu", t.text_primary);
    egui::Grid::new("own-usage-table").num_columns(4).spacing([theme::SP_4, theme::SP_1]).show(ui, |ui| {
        for (i, row) in own.table.iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                let ink = if i == 0 || j == 0 { t.text_secondary } else { t.text_primary };
                ui.add(egui::Label::new(Type::Small.text(cell.as_str()).color(ink)));
            }
            ui.end_row();
        }
    });
    for note in &own.notes {
        theme::label(ui, Type::Small, note.as_str(), t.text_secondary);
    }
    ui.add_space(theme::SP_2);
    theme::heading(ui, Type::BodyStrong, "This week by account and project", t.text_primary);
    if own.groups.is_empty() {
        theme::label(ui, Type::Small, "Nothing used in Shibubu this week yet", t.text_secondary);
    }
    for (head, key, rows) in &own.groups {
        // PROVIDER-ICONS-1101（owner 248）: 見出しの左にその AI の印
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(theme::ICON_UI), egui::Sense::hover());
            theme::provider_icon(ui, mark.center(), key, head, t.text_secondary);
            theme::label(ui, Type::Small, head.as_str(), t.text_primary);
        });
        for (left, right) in rows {
            ui.horizontal(|ui| {
                ui.add_space(theme::SP_3);
                ui.add(egui::Label::new(Type::Small.text(left.as_str()).color(t.text_secondary)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(Type::Small.text(right.as_str()).color(t.text_primary)));
                });
            });
        }
    }
}

/// 同: 読めなかった物の 1 行ずつ・いつ数えたか・`Recount`（押した = `Intent::RecountOwnUsage`）。例: `if let Some(i) = shell::own_usage_footer(ui, own) { out = Some(i) }`
/// Wave: ACCOUNTS-1101
pub fn own_usage_footer(ui: &mut egui::Ui, own: &OwnPanel) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut out = None;
    for p in &own.problems {
        theme::label(ui, Type::Small, p.as_str(), t.danger);
    }
    ui.horizontal(|ui| {
        theme::label(ui, Type::Small, own.counted.as_str(), t.text_secondary);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::button(ui, "Recount", theme::Variant::Ghost).clicked() {
                out = Some(Intent::RecountOwnUsage);
            }
        });
    });
    out
}

/// 帯の押せる語（地なし・hover で `surface_3`・高さ = 帯・字 `Type::Small`）。AX = Button と `name`。
fn bar_word(ui: &mut egui::Ui, text: &str, name: &str) -> egui::Response {
    bar_glyph(ui, text, None, None, name)
}

/// STATUSBAR-1101 追記 229: 下の帯の歯車の AX の名前と hover（押すと Settings・鍵は ⌘,）。
pub const SETTINGS_GEAR: &str = "Settings · ⌘,";

/// 帯の押せる字（`bar_word` の本体・字 `Type::Small`）か icon（`icon` が在れば `text` は使わない・STATUSBAR-1101 追記 229 の歯車 = `ICON_UI` 16）。
/// `lead` = 字の左に置く provider の印の key（PROVIDER-ICONS-1101・今の AI の選び）。
fn bar_glyph(ui: &mut egui::Ui, text: &str, icon: Option<Icon>, lead: Option<&str>, name: &str) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), Type::Small.font(), t.text_secondary);
    let lead_w = if lead.is_some() { theme::ICON_UI + theme::SP_1 } else { 0.0 };
    let w = if icon.is_some() { theme::ICON_UI } else { lead_w + galley.size().x };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w + 2.0 * theme::SP_2, theme::STATUS_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if ui.is_rect_visible(rect) {
        let fill = if response.is_pointer_button_down_on() { t.surface_press } else if response.hovered() { t.surface_3 } else { egui::Color32::TRANSPARENT };
        let face = rect.shrink2(egui::vec2(0.0, 2.0));
        ui.painter().rect_filled(face, egui::CornerRadius::same(theme::RADIUS_SM), fill);
        theme::focus_ring(ui, &response, face, egui::CornerRadius::same(theme::RADIUS_SM));
        match icon {
            Some(icon) => {
                theme::paint_icon(ui.painter(), icon, rect.center(), theme::ICON_UI, t.text_secondary, 0.0);
            }
            None => {
                if let Some(key) = lead {
                    theme::provider_icon(ui, egui::pos2(rect.left() + theme::SP_2 + theme::ICON_UI / 2.0, rect.center().y), key, name, t.text_secondary);
                }
                ui.painter().galley(egui::pos2(rect.left() + theme::SP_2 + lead_w, rect.center().y - galley.size().y / 2.0), galley, t.text_secondary);
            }
        }
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// STATUSBAR-1101 追記 210: 読みに失敗した AI の語（hover = 理由の 1 行）。
const REFRESH_FAILED: &str = "⚠ Refresh failed";

/// STATUSBAR-1101 追記 210: 帯の AI 1 つ = 印 → 窓ごとに細い棒と `42% used 2h 10m`（AX `Session · 42% used 2h 10m`）／読めない = `--`（AX `{名前} usage: {理由}`・hover = 理由）／
/// 読みの失敗 = `⚠ Refresh failed`（hover = 理由）／今の AI の読む前 = `Show usage` だけ → 今の AI だけ次の一手（`Try again`・`Sign in`）。返り値 = 押した手。
fn ai_usage(ui: &mut egui::Ui, a: &BarAi, bar: &StatusBar, ask: bool) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let step = bar.usage.as_ref().filter(|_| a.active).and_then(|(_, step)| step.as_ref());
    // 今の AI の hover は `usage_view` の 1 行（`Checking usage`・`updated N ago` まで）
    let why = match &bar.usage {
        Some((line, _)) if a.active && !ask => line.as_str(),
        _ => a.why.as_str(),
    };
    ai_badge(ui, a);
    if a.active && ask {
        // 読む前（operator ①）: 数字は出さず `Show usage` だけ・Keychain を読む事は hover
        return step.and_then(|(name, then)| bar_word(ui, name, name).on_hover_text(USAGE_ASK).clicked().then(|| then.clone()));
    }
    if a.failed {
        ui.add(egui::Label::new(Type::Small.text(REFRESH_FAILED).color(t.danger))).on_hover_text(why);
    } else if a.windows.is_empty() {
        let dash = ui.add(egui::Label::new(Type::Small.text("--").color(t.text_tertiary)));
        theme::ax_name(dash, &format!("{} usage: {why}", a.name)).on_hover_text(why);
    } else {
        for (win, short, pct) in &a.windows {
            meter(ui, *pct);
            let word = ui.add(egui::Label::new(Type::Small.text(short.as_str()).color(t.text_secondary)));
            theme::ax_name(word, &format!("{win} · {short}")).on_hover_text(format!("{} · {win}\n{why}", a.name));
        }
    }
    step.and_then(|(name, then)| bar_word(ui, name, name).clicked().then(|| then.clone()))
}

/// `ai_usage` が取る幅（前の空き `SP_2` を含む・描く前に測って帯の畳みを決める）。
fn ai_width(ui: &egui::Ui, a: &BarAi, bar: &StatusBar, ask: bool) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    let text = |s: &str| ui.painter().layout_no_wrap(s.to_owned(), Type::Small.font(), egui::Color32::WHITE).size().x + gap;
    let step = bar.usage.as_ref().filter(|_| a.active).and_then(|(_, step)| step.as_ref()).map_or(0.0, |(name, _)| text(*name) + 2.0 * theme::SP_2);
    let body = if a.active && ask {
        0.0
    } else if a.failed {
        text(REFRESH_FAILED)
    } else if a.windows.is_empty() {
        text("--")
    } else {
        a.windows.iter().map(|(_, short, _)| 28.0 + gap + text(short.as_str())).sum::<f32>()
    };
    theme::SP_2 + AI_MARK + gap + body + step
}

const AI_MARK: f32 = 16.0;

/// 帯の AI の印（PROVIDER-ICONS-1101: `theme::provider_icon` = 同梱の brand の logo → Mac の app の icon → 名前の頭文字の丸・16 四方・hover = 名前）。
fn ai_badge(ui: &mut egui::Ui, a: &BarAi) {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(egui::vec2(AI_MARK, AI_MARK), egui::Sense::hover());
    theme::provider_icon(ui, rect.center(), &a.key, &a.name, t.text_secondary);
    response.on_hover_text(a.name.as_str());
}

/// 使用量の窓 1 つの細い棒（幅 28・高さ 4・100% は danger の色）。
fn meter(ui: &mut egui::Ui, pct: u8) {
    let t = theme::tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 4.0), egui::Sense::hover());
    let used = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * f32::from(pct.min(100)) / 100.0, rect.height()));
    ui.painter().rect_filled(rect, egui::CornerRadius::same(2), t.surface_3);
    ui.painter().rect_filled(used, egui::CornerRadius::same(2), if pct >= 100 { t.danger } else { t.text_secondary });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_tiers_match_design_section_8() {
        let wide = Tier::for_width(1440.0);
        assert_eq!((wide.sidebar, wide.gutter), (232.0, 28.0));
        let mid = Tier::for_width(1200.0);
        assert_eq!(mid.sidebar, 216.0);
        assert_eq!(Tier::for_width(1439.0), mid);
        let narrow = Tier::for_width(960.0);
        assert_eq!((narrow.sidebar, narrow.gutter), (216.0, 20.0));
        assert_eq!(Tier::for_width(1199.0), narrow);
        // UI-4b: 畳みの既定は 1200 未満だけ（狭い窓は畳んで始まる）
        assert_eq!((wide.sidebar_hidden, mid.sidebar_hidden, narrow.sidebar_hidden), (false, false, true));
    }
    /// UI-4b: 覚えた値（`Some`）は窓の幅の既定より先・`None` の間だけ幅の既定（1200 未満で畳む）。負の対照: `saved` を読まず幅の既定だけにすると 2 行目と 3 行目が赤。
    #[test]
    fn sidebar_open_prefers_the_saved_choice_over_the_width_default() {
        let (wide, narrow) = (Tier::for_width(1440.0), Tier::for_width(900.0));
        assert_eq!((wide.sidebar_open(None), narrow.sidebar_open(None)), (true, false), "人が触る前は幅の既定");
        assert_eq!((wide.sidebar_open(Some(true)), narrow.sidebar_open(Some(true))), (false, false), "畳んだ選択は広い窓でも畳んだまま");
        assert_eq!((wide.sidebar_open(Some(false)), narrow.sidebar_open(Some(false))), (true, true), "狭い窓で戻した選択を幅の既定が上書きしない");
    }
    #[test]
    fn navigating_closes_the_palette() {
        let mut s = Shell::new(View::Home);
        s.palette.open = true;
        s.go(View::Runs);
        assert_eq!(s.view, View::Runs);
        assert!(!s.palette.open, "navigating closes the palette");
    }

    fn project(id: &str, page: &str) -> View {
        View::Project { project: id.into(), page: page.into() }
    }
    fn rows() -> Vec<AppRow> {
        vec![AppRow { view: project("p.a", "x"), name: "Dev environment".into(), purpose: String::new(), status: String::new(), running: 0, needs: 0, pinned: false, archived: false, opened_at: None, sample: false }]
    }
    fn at(view: View, text: &str) -> Shell {
        let mut s = Shell::new(view);
        s.command = text.into();
        s
    }

    /// UI-4a（決定 1・図 §38）: 1 つの欄の placeholder と AX の名前は場所だけで決まる（表 copy.en.md:26〜30 の EN）。
    #[test]
    fn ui4a_field_hint_and_name_follow_the_place() {
        assert_eq!((View::Home.field_hint(), View::Home.field_name()), ("What do you want to create?", "What to create"));
        assert_eq!((project("p", "q").field_hint(), project("p", "q").field_name()), ("Ask, or say what to change, e.g. add a Tests page", "What to create"), "CHAT-ONE: 欄の名前は Home と同じ");
        for v in [View::Runs, View::Connections, View::CompatCanvas(1), View::App("a".into())] {
            assert_eq!((v.field_hint(), v.field_name()), ("Search commands and projects", "Search commands and projects"), "{v:?}");
        }
    }

    /// RQ1 interface（caller の形）: Settings は sidebar の nav に足さず（3 行のまま）、欄は Runs と同じ（探す欄・Enter は command の一覧の頭）・打ちかけは探す欄の key。
    /// 負の対照（commit message）: `NAV` に `View::Settings(General)` を足す → 赤。
    #[test]
    fn rq1_settings_is_not_a_nav_row_and_keeps_the_one_field() {
        let s = View::Settings(SettingsSection::General);
        assert!(!View::NAV.contains(&s), "sidebar の nav に Settings を足した");
        assert_eq!((s.field_hint(), s.field_name(), s.place(), s.is_app()), ("Search commands and projects", "Search commands and projects", "search", false));
        assert_eq!(SettingsSection::ALL.map(SettingsSection::label), ["General", "AI & Models", "Terminal", "Data & Recovery", "Privacy", "About"]);
        assert_eq!(at(s, "connections").enter_intent(false, &rows()), Some(Intent::Go(View::Connections)));
    }

    /// UI-4a Done 2 / L2: Home の Enter = 会話（同じ名前の Project は開く）・Home の ⌘↵ = 会話（一致も開かない）・Project の Enter = その Project を変える・
    /// Home の外の ⌘↵ = 作る・Runs / Connections の Enter = command の一覧の頭（作る行は除く・無ければ送らない）。空は送らない。
    /// 負の対照（commit message）: Project の腕を `Generate` にする → 赤。
    #[test]
    fn ui4a_enter_goes_by_the_place_and_cmd_enter_creates_outside_home() {
        let apps = rows();
        assert_eq!(at(View::Home, "  ").enter_intent(false, &apps), None, "空は送らない");
        assert_eq!(at(View::Home, " Track my inventory ").enter_intent(false, &apps), Some(Intent::Talk("Track my inventory".into())), "Home の Enter は作らず会話にする");
        assert_eq!(at(View::Home, "dev ENVIRONMENT").enter_intent(false, &apps), Some(Intent::Go(project("p.a", "x"))), "同じ名前は開く");
        assert_eq!(at(project("p.a", "x"), "add a Tests page").enter_intent(false, &apps), Some(Intent::Talk("add a Tests page".into())), "CHAT-ONE: Project も会話");
        assert_eq!(at(View::Runs, "connections").enter_intent(false, &apps), Some(Intent::Go(View::Connections)));
        assert_eq!(at(View::Connections, "dev env").enter_intent(false, &apps), Some(Intent::Go(project("p.a", "x"))));
        assert_eq!(at(View::Runs, "zzz").enter_intent(false, &apps), None, "合う行が無ければ作らない（作るのは ⌘↵）");
        for v in [View::Runs, View::Connections, View::CompatCanvas(1)] {
            assert_eq!(at(v.clone(), "Keep research notes").enter_intent(true, &apps), Some(Intent::Generate("Keep research notes".into(), vec![])), "{v:?}");
        }
        // CHAT-ONE: Project の ⌘↵ も Enter と同じ会話
        assert_eq!(at(project("p.a", "x"), "add a Tests page").enter_intent(true, &apps), Some(Intent::Talk("add a Tests page".into())));
        assert_eq!(at(View::Home, "Keep research notes").enter_intent(true, &apps), Some(Intent::Talk("Keep research notes".into())), "L2: Home の ⌘↵ も送るだけ（作らない）");
        assert_eq!(at(View::Home, "dev ENVIRONMENT").enter_intent(true, &apps), Some(Intent::Talk("dev ENVIRONMENT".into())), "L2: Home の ⌘↵ は名前の一致も開かず送る");
    }

    /// RQ2 interface: 新しい殻は名前の欄も確認も開いておらず Recent で始まる・Archive した Project も ⌘K の `Open {name}` には残る（消さない・戻る道）。
    /// 負の対照（commit message）: `palette::commands` の apps の行で `archived` を飛ばす → 赤。
    #[test]
    fn rq2_app_row_marks_default_to_nothing() {
        let s = Shell::new(View::Home);
        assert_eq!((s.renaming.as_deref(), s.project_confirm.as_deref(), s.open_projects.is_empty(), s.show_archived), (None, None, true, false));
        let row = AppRow { view: project("p.a", "x"), name: "Old notes".into(), purpose: String::new(), status: String::new(), running: 0, needs: 2, pinned: false, archived: true, opened_at: Some(1), sample: false };
        let rows = super::super::palette::commands(&s, "old", std::slice::from_ref(&row), &[]);
        assert!(rows.iter().any(|c| c.label == "Open Old notes" && c.intent == Intent::Go(project("p.a", "x"))), "Archive した Project が ⌘K から消えた: {:?}", rows.iter().map(|c| &c.label).collect::<Vec<_>>());
    }

    /// RQ4 → CONNECTIONS-1101: 行の下の欄の文（鍵も）は Debug に出ず、上の欄（`command`）にも入らず、上の欄の Enter は行の下の欄を送らない。
    /// 場所を移ると欄も行内の確認も閉じて文を捨てる。負の対照: `FieldText` の Debug を derive に戻す → SECRET が出て赤・`go` の `end_connecting` を外す → 残って赤。
    #[test]
    fn rq4_connection_field_keeps_its_text_out_of_debug_and_closes_when_leaving() {
        let mut s = Shell::new(View::Connections);
        s.command = "my draft".into();
        s.start_connecting(super::super::connections::secret_key("github", "token"));
        s.field.0 = "ghp_SECRET".into();
        assert!(!format!("{s:?}").contains("SECRET"), "欄の文が Debug に出た");
        assert_eq!(s.command, "my draft", "行の下の欄を開いて上の欄の打ちかけが替わった");
        assert!(!matches!(s.enter_intent(false, &[]), Some(Intent::Connection { .. })), "上の欄の Enter が行の下の欄を送った");
        s.connection_confirm = Some("mcp:penpot".into());
        s.go(View::Home);
        assert_eq!((s.connecting.as_deref(), s.connection_confirm.as_deref(), s.field.0.as_str()), (None, None, ""), "場所を移っても欄・確認・文が残る");
    }

    /// RQ3 interface（caller の形）: Delete my data の確認は場所を移ると閉じる（1 回目で消さない・開いたまま別の画面へ行かない）・種類の語は重ならず影響の 1 行を持つ。
    /// 負の対照（commit message）: `Shell::go` の `self.data_confirm = None` を外す → 赤。
    #[test]
    fn rq3_data_confirm_closes_when_leaving_and_each_kind_says_its_effect() {
        let mut s = Shell::new(View::Settings(SettingsSection::Data));
        assert_eq!(s.data_confirm, None);
        s.data_confirm = Some(DataKind::Consents);
        s.go(View::Home);
        assert_eq!(s.data_confirm, None, "場所を移っても Delete my data の確認が開いたまま");
        let labels = DataKind::ALL.map(DataKind::label);
        assert!(labels.iter().enumerate().all(|(i, l)| !l.is_empty() && !labels[..i].contains(l)), "{labels:?}");
        assert!(DataKind::ALL.iter().all(|k| !k.effect().is_empty() && !k.effect().contains('/')), "影響の 1 行が空か path を含む");
    }

    /// RQ2 反証の直し（holdout h1）: 名前の欄の間も、空の欄の Enter / ⌘↵ は何も送らない（`enter_intent` の「空なら None」を名前の欄にも）。名前が在れば trim して `RenameProject`。
    /// 負の対照（commit message）: `enter_intent` の空の判定を名前の欄の後ろへ戻す → `RenameProject { name: "" }` が出て赤。
    #[test]
    fn rq2_blank_name_in_the_rename_field_sends_nothing() {
        let mut s = Shell::new(View::Home);
        s.start_rename("project.app.c2".into(), "  ".into());
        for cmd in [false, true] {
            assert_eq!(s.enter_intent(cmd, &[]), None, "空の名前で送った（⌘↵ = {cmd}）");
        }
        s.command = "  Shop ".into();
        assert_eq!(s.enter_intent(false, &[]), Some(Intent::RenameProject { project: "project.app.c2".into(), name: "Shop".into() }));
    }

    /// UI-4a: 欄の右端の 1 語は Enter の結果を押す前に書く（表 copy.en.md:32・:52・:64）。
    #[test]
    fn ui4a_right_edge_says_what_enter_does() {
        let apps = rows();
        assert_eq!(at(View::Home, "").enter_label(&apps), "↵ Send", "L2: Home の右端は送る");
        assert_eq!(at(View::Home, "dev environment").enter_label(&apps), "↵ Open “Dev environment”", "L2: Home の ⌘↵ は作らないので ` · ⌘↵ Create` を書かない");
        assert_eq!(at(project("p.a", "x"), "").enter_label(&apps), "↵ Send", "CHAT-ONE: Project も送る");
        assert_eq!(at(View::Runs, "").enter_label(&apps), "");
        assert_eq!(at(View::Runs, "dev").enter_label(&apps), "↵ Open · ⌘↵ New project");
        assert_eq!(at(View::Connections, "runs").enter_label(&apps), "↵ Run · ⌘↵ New project");
        assert_eq!(at(View::Runs, "zzz").enter_label(&apps), "No command matches · ⌘↵ to create a project from this");
    }

    /// UI-6a（反証の直し）: canvas が 1 つの時の削除の行（押せない・label が理由）は欄の Enter でも走らない。2 つ以上なら削除の確認へ。
    /// 負の対照（commit message）: `enter_intent` の `palette::disabled` の filter を外す → 赤。
    #[test]
    fn ui6a_enter_skips_the_row_that_cannot_be_chosen() {
        let apps = rows();
        let mut s = at(View::CompatCanvas(1), "least");
        s.canvases = Some(1);
        assert_eq!(s.enter_intent(false, &apps), None, "押せない行を Enter で走らせた");
        assert_eq!(s.enter_label(&apps), "No command matches · ⌘↵ to create a project from this");
        let mut s = at(View::CompatCanvas(1), "delete canvas");
        s.canvases = Some(2);
        assert_eq!(s.enter_intent(false, &apps), Some(Intent::DeleteApp));
    }

    /// UI-4a（SPEC §2「Home に 1 つ・Project ごとに 1 つ」）: 場所が替わる時に欄の文を預けて戻す。Project A の打ちかけが B に出ない。
    /// 同じ欄の中（同じ Project の別の Page・Runs ↔ Connections）は欄をそのまま。Home へ行くと欄に focus（1 回）。
    /// DOGFOOD-X 周 2 回 8: Home の作る欄と探す欄（Runs など）は別の打ちかけ = Home の未送信の頼みが探す欄に出ず、探す欄で打った語が頼みに付かない。
    /// 負の対照（commit message）: `go` の預け / 戻しを外す → 赤・`place` の探す欄を `"home"` に戻す → 「Home の頼みが探す欄に出た」で赤。
    #[test]
    fn ui4a_drafts_follow_the_place() {
        let mut s = at(View::Home, "a timer");
        s.focus_command = false;
        s.go(project("p.a", "1"));
        assert_eq!((s.command.as_str(), s.drafts.get("home").map(String::as_str)), ("", Some("a timer")));
        s.command = "add a Tests page".into();
        s.go(project("p.a", "2"));
        assert_eq!(s.command, "add a Tests page", "同じ Project の Page の移りは欄をそのまま");
        s.go(project("p.b", "1"));
        assert_eq!(s.command, "", "A の変更文が B の欄に出た");
        s.go(project("p.a", "2"));
        assert_eq!(s.command, "add a Tests page");
        assert!(!s.focus_command);
        s.go(View::Runs);
        assert_eq!(s.command, "", "Home の頼みが探す欄に出た");
        s.command = "hello".into();
        s.go(View::Connections);
        assert_eq!(s.command, "hello", "Runs → Connections は同じ探す欄");
        s.go(View::Home);
        assert_eq!(s.command, "a timer", "探す欄で打った語が Home の頼みに付いた・頼みが戻らない");
        assert_eq!(s.drafts.get("search").map(String::as_str), Some("hello"), "探す欄の打ちかけが残らない");
        assert!(s.focus_command, "Home へ行くと欄に focus");
        s.command = "   ".into();
        s.go(project("p.b", "1"));
        assert!(!s.drafts.contains_key("home"), "空の文は持たない: {:?}", s.drafts);
        assert!(Shell::new(View::Home).focus_command && !Shell::new(View::Runs).focus_command);
    }
    /// FX4（FX-020・UIUX H05）: 右端は Enter と ⌘↵ の結果が違う所で両方を言う。AX の名前（` · ` の前・`↵ ` を除く）= `Send`（CHAT-ONE: Project も）。
    #[test]
    fn fx4_right_end_says_what_enter_and_cmd_enter_do() {
        let ax = |l: &str| l.trim_start_matches("↵ ").split(" · ").next().unwrap_or_default().to_owned();
        let mut s = Shell::new(project("p.a", "1"));
        s.command = "add a Tests page".into();
        assert_eq!(s.enter_label(&[]), "↵ Send");
        assert_eq!(ax(&s.enter_label(&[])), "Send");
        let mut home = Shell::new(View::Home);
        home.command = "Track my inventory".into();
        assert_eq!((home.enter_label(&[]), ax(&home.enter_label(&[]))), ("↵ Send".to_owned(), "Send".to_owned()), "L2: Home の右端は送る（作らない）");
        let mut runs = Shell::new(View::Runs);
        runs.command = "go to home".into();
        assert_eq!(runs.enter_label(&[]), "↵ Run · ⌘↵ New project");
        runs.command = "zzz".into();
        assert_eq!(runs.enter_label(&[]), "No command matches · ⌘↵ to create a project from this", "合う行が無い時は今のまま");
    }
    /// FX4（FX-016）: ⌘K を開いている間に場所を替えても、⌘K で打った語は打ちかけにならず、⌘K の前の打ちかけは元の場所に残る。
    #[test]
    fn fx4_leaving_with_the_list_open_keeps_the_draft_not_the_query() {
        let ctx = egui::Context::default();
        let mut s = Shell::new(project("p.a", "1"));
        s.command = "add a chart".into();
        s.palette.open(&ctx, &mut s.command);
        assert!(s.command.is_empty(), "⌘K を開いたのに欄に打ちかけが残る（語と混ざる）");
        s.command = "go to runs".into();
        s.go(View::Runs);
        assert!(!s.palette.open);
        assert_eq!(s.drafts.get("p.a").map(String::as_str), Some("add a chart"), "⌘K の前の打ちかけが元の Project に残らない");
        assert!(!s.drafts.values().any(|d| d == "go to runs"), "⌘K で打った語が打ちかけになった: {:?}", s.drafts);
    }

    /// G104 / L2: Home は送る（会話）だけで、添付は欄に残り案の `Build now` で乗る。Home の外の ⌘↵ には乗らない。
    #[test]
    fn g104_attachments_ride_only_on_the_home_line() {
        let a = vec![Attachment::Pasted { text: "line\n".repeat(300), lines: 300 }];
        let mut home = Shell::new(View::Home);
        (home.command, home.attach) = ("Plan a trip".into(), a.clone());
        assert_eq!(home.enter_intent(true, &[]), Some(Intent::Talk("Plan a trip".into())), "L2: Home の ⌘↵ は送るだけ（添付は欄に残り、案の Build now で乗る）");
        assert_eq!(home.enter_intent(false, &[]), Some(Intent::Talk("Plan a trip".into())), "Home の Enter は添付ごと作らない");
        let mut runs = Shell::new(View::Runs);
        (runs.command, runs.attach) = ("Plan a trip".into(), a);
        assert_eq!(runs.enter_intent(true, &[]), Some(Intent::Generate("Plan a trip".into(), vec![])));
    }

    /// G86a × L1-NORAIL（review B）: 新しい card（承認・決める・同意・Ask・結果）が出た frame だけ Chat を開く合図・同じ card の間は開き直さない・
    /// 進めている間（None）は開かない・窓の幅に依らない（Shell は段を持たない）・Project に入っただけでは開かない（go は合図を出さない）。
    /// REVIEW-R7 Y-c: 提案は Home に出て（鍵が None の frame）同じ Project に戻っても開き直さない・Engine の card は次の run の同じ結果でまた開く（鍵は文だけ）。
    /// 負の対照: show_card の `suggestion:` の腕を外す（前の形 = None で忘れる）→ Home を挟んだ同じ提案で開き直して赤（束の arm）・どの鍵も起動の間覚える形にする →
    /// 次の run の同じ結果で開かず赤・`key != self.card_seen` を外す → 同じ card で開き直して赤・前の狭い段の枝（承認と同意だけ）を戻す → Ask と結果で開かず赤。
    #[test]
    fn g86_a_new_card_opens_the_chat_once() {
        let mut s = Shell::new(View::Home);
        s.go(View::Project { project: "p1".into(), page: "page.dev".into() });
        assert!(!s.show_card("p1", None), "進めている間に開いた");
        for key in ["p1:approve:x:1", "p1:ask:Which file?", "p1:line:Done", "p1:decide:x:Which?:", "p1:consent:x"] {
            assert!(s.show_card("p1", Some(key.into())), "{key} の card で Chat を開かない");
            assert!(!s.show_card("p1", Some(key.into())), "{key} の同じ card で開き直した");
        }
        assert!(s.show_card("p1", Some("suggestion:add-search".into())), "新しい提案で開かない");
        assert!(!s.show_card("p1", None) && !s.show_card("p1", Some("suggestion:add-search".into())), "Home を挟んで同じ提案で開き直した");
        assert!(!s.show_card("p1", None) && s.show_card("p1", Some("p1:line:Done".into())), "次の run の同じ結果の card で開かない");
    }

    /// B34（NORAIL review N1）: 承認の card が待っている Project を畳んで、別の Project（card 無し = None）か Home（呼ばない）を挟んで戻っても、
    /// 同じ card で panel を開き直さない。その Project で進めて（None）から同じ card が出たら開く（次の run）。別の Project の card は別に開く。
    /// 負の対照（未測）: `card_seen` を前の 1 つだけ覚える形（`Option<String>`・None で全部忘れる）に戻す → p2 を挟んだ p1 で開き直して赤。
    #[test]
    fn b34_coming_back_to_a_project_does_not_reopen_the_same_card() {
        let mut s = Shell::new(View::Home);
        let (a, b) = ("p1:approve:Delete the draft:1:", "p2:approve:Send it:2:");
        assert!(s.show_card("p1", Some(a.into())), "承認の card で開かない");
        assert!(!s.show_card("p2", None), "card の無い Project で開いた");
        assert!(!s.show_card("p1", Some(a.into())), "B34: 別の Project を挟んで戻ったら同じ card で開き直した");
        assert!(s.show_card("p2", Some(b.into())), "別の Project の card で開かない");
        assert!(!s.show_card("p1", Some(a.into())), "B34: 別の Project の card を挟んで戻ったら同じ card で開き直した");
        assert!(!s.show_card("p1", None) && s.show_card("p1", Some(a.into())), "その Project で進めた後の同じ card（次の run）で開かない");
    }
}
