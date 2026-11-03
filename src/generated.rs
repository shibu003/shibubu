//! 一言目 → 動く app（統合 2 の通し・図 docs/diagrams.md §9 の 2 枚目）。
//! Generate = `composer::compose` だけ（engine::generate の互換 canvas は作らない）→ `AppRevision::new` → `runtime.open`（apps と revision の
//! 1 行目・session）→ `View::App(id)` で `renderer::render(定義, AppState, Host)`。押した物は `Host::invoke` → `AppRuntime::invoke` の 1 本。
//! 表示は ui_state から毎 frame 引き直す（成功を演出しない）。起動時は apps の行を全部開き直す（開けない行は数えて Home に 1 行）。
use crate::composer::verifier::{self, Check, LocalStep, Need, Outside, Probe, Readback, Seat, SeatKind, Usable};
use crate::composer::{self, ComposeContext, ComposeError};
use crate::domain::action::{ActionInvocation, ActionReadiness, InvocationOrigin};
use crate::domain::app::AppDefinitionV3;
use crate::domain::size_line;
use crate::domain::binding::DataBinding;
use crate::domain::patch::AppRevision;
use crate::domain::project::{self as project, PageDefinition, ProjectDefinitionV1, ProjectPatchError, ProjectPatchOp, ResourceReader, RevisionOrigin, SurfaceReceipt, UndoError, UndoRedo};
use crate::surface::Bridge;
use crate::domain::run::{ErrorCode, Outcome};
use crate::project::router::{self, PageRouter, PageSwitch};
use crate::project::{view_state_key, PageViewState};
use crate::runtime::registry::ActionRegistry;
use crate::runtime::{AppRuntime, CancelResult, Invoked, Runtime};
use crate::storage::{self, Store, Table};
use crate::domain::terminal::{AgentState, AgentWatch, InputLease, InputOwner, InputView, RunReceipt, RunState};
use crate::terminal::{AgentSetup, CommandOutput, ProjectCli, SessionInfo, SessionKey, SessionStatus, TermSize, TerminalConfig, TerminalManager};
use crate::ui::renderer::{self, ActionHost, AppState, TerminalPane};
use crate::ui::runs::{RunAction, RunEntry, RunResult, RunsAction, Undo};
use crate::ui::shell::{self, AppRow, InstallAct, Intent, PageIntent, View};
use crate::project::acquire;
use crate::ui::theme::{self, Variant};
use crate::ui::states::{self, RegionState};
use crate::ui::terminal_panel::{self, PanelAct, PanelEffect, TabStatus, TerminalPanel};
use eframe::egui;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 生成した app 1 つ（開いた runtime と画面の状態）。
pub struct Opened {
    rt: AppRuntime,
    state: AppState,
    /// 押した結果のうち Verified でない物: node key → 状態と 1 行（次に同じ node が Verified になったら消す）
    regions: BTreeMap<String, (RegionState, String)>,
    /// app の上に出す直近の 1 行（押した結果が Verified でない時だけ）と、その状態（UI-6a・sendback 60.3: `page_line` と同じ形。前は文だけで、描く側が全部 `× Failed` にしていた）
    line: Option<(RegionState, String)>,
}

/// STATUSBAR-1101 追記 2 の直し: Claude 以外の AI（Grok・Kimi Code）の使用量を読む口（`Startup::cli_usage`）。
/// 渡されていない間（試験の crate）は読まない = 帯は Claude の plan だけ（`refresh_usage` の保証は同じ・読める事は保証しない）。
#[derive(Clone, Copy)]
pub struct CliUsageReader {
    /// 使用量を読める AI の provider の語（`llm_cli::USAGE_CLIS`）
    pub clis: &'static [&'static str],
    /// その account の置き場（`llm_cli::cli_account_dir`）
    pub dir: fn(&std::path::Path, &crate::model::AiAccount) -> Option<std::path::PathBuf>,
    /// 1 本読む（`llm_cli::cli_usage`・裏の thread で呼ぶ）
    pub read: fn(&str, Option<&std::path::Path>, Duration) -> Result<crate::llm::PlanUsage, crate::llm::LlmError>,
}

/// 起動の時に main が渡す物（generated と ui は `#[path]` で試験の crate〈tests/yc_demo_e2e.rs・mirror_e0.rs・tests/ui_*.rs〉にも入り、そこに audio・llm_cli・
/// main の開く panel は無い = 直に呼ぶと試験の compile が赤だった）。渡されていない間は各欄の doc の「無い時」の振る舞い。
pub struct Startup {
    /// ui が読む物（icon の 2 つ・開く panel）。`set_startup` が `ui::STARTUP` に入れる
    pub ui: crate::ui::Startup,
    /// SIGNIN-1101: sign in の tab の command を解く（`llm_cli::sign_in_command`・表に無い command は None）。無い時は sign in の tab に何も打たない（shell だけ）
    pub sign_in_command: fn(&str) -> Option<String>,
    /// STATUSBAR-1101 追記 2: Grok・Kimi Code の使用量を読む口
    pub cli_usage: CliUsageReader,
}
static STARTUP: std::sync::OnceLock<Startup> = std::sync::OnceLock::new();

/// 例（main の run_native の前に 1 回・2 回目は無視）: `generated::set_startup(generated::Startup { ui: ui::Startup { app_icon: audio::surface::app_icon_png, … },
/// sign_in_command: llm_cli::sign_in_command, cli_usage: generated::CliUsageReader { clis: &llm_cli::USAGE_CLIS, dir: llm_cli::cli_account_dir, read: llm_cli::cli_usage } })`
pub fn set_startup(startup: Startup) {
    let _ = crate::ui::STARTUP.set(startup.ui);
    let _ = STARTUP.set(startup);
}

pub struct Generated {
    rt: Arc<Runtime>,
    /// runtime と同じ store（Project の行と revision を読む・書く。lock は runtime の外で短く取る）
    store: Arc<Mutex<Store>>,
    /// 新しい順
    apps: Vec<Opened>,
    /// Home の入力欄の下に出す 1 行（作れなかった理由・開けなかった行の数・保存できない理由）
    pub note: Option<String>,
    /// FX1: kangae.sqlite3 を開けず memory の store で動いている理由（`open` の Err の枝だけが入れる・`store_lost` が読む）
    lost: Option<String>,
    /// 開いた Project の居場所と戻りの履歴（key は Project の id）。保存しない: 再起動したら `open_project` で開き直す。
    routers: BTreeMap<String, PageRouter>,
    /// Project の画面の上に出す直前の拒否の 1 行（最後の 1 枚・古い base_hash・読めない状態）と、その状態の印。次に通った Page の操作で消す。
    /// 「走らせる」の拒否（人の lease・走っている間の 2 本目）は確認待ち: test の失敗ではないので Tests Page の「失敗」と同じ語（失敗しました）で出さない。
    page_line: Option<(RegionState, String)>,
    /// RQ3: その Project を次に `open_project` で開いた時に page_line に置く 1 行（Project の id, 状態, 文）。Import の足りない接続の 1 行が、取り込んだ後の
    /// `route` → `open_project` の作り直しで消えないように（page_line は Project を問わない 1 つなので、別の Project に持ち越さない形で渡す）
    opened_line: Option<(String, (RegionState, String))>,
    /// 「この Page を消す」を 1 回押した Page の id。Page の上に「消す / やめる」の 1 行を出し、2 回目の「消す」で初めて消す（誤押しで消さない）。
    pending_delete: Option<String>,
    /// FX3（FX-009）: `•••` の `Rename…` で開いた名前の欄（Page の id・打っている名前）。拒否の時は開いたまま（打った名前を失わない）・`pending_delete` と同時に開かない。
    pending_rename: Option<(String, String)>,
    /// FX3（FX-011）: 帯の手で影響を受けた tab（Page の id）と tint を始めた時刻（None = 次に描く frame の時刻から）。
    tinted: Option<(String, Option<f64>)>,
    /// FX3（FX-014）: 次の frame で keyboard focus を置く tab（`Add` で足した Page の id）。
    focus_tab: Option<String>,
    /// 1 つの欄（header の `Shell::command`）の変更の文が通った Project の id（`take_edit_applied` が 1 回だけ渡す・拒否では立てない）。UI-4a
    edit_applied: Option<String>,
    /// L2（review A・C）: Home の会話で Build now した案の文と、それまでの会話（`accept_plan` が置き、`save_plan` が同じ文の Project を作った時に取る）。
    /// 在る間、その文は変更の文でも `apply_home_edit` に回さない（案の Build now は AI が読む道）。Project の会話の頭はこの会話の続き。
    plan_talk: Option<(String, Vec<crate::domain::chat::Turn>)>,
    /// L2（DOGFOOD-X 周 2 回 1 #2）: `Preview plan` で model が読んだ案（文・案〈None = 1 Page の app〉）。同じ文の `create_project` が model を呼ばずに取る。
    previewed: Option<(String, Option<composer::project::ProjectPlan>)>,
    /// 直前の `finish_edit` が文の拒否（読めない・既に在る・当てる先が無い・Engine が読んでいる）で終わった印。1 行を Ready にする（書けなかった Error と分ける・③ C2）
    edit_refused: bool,
    /// Project の Terminal の PTY（W4・画面なし。Page への表示は W5）。session の記録は `terminal_sessions` の表。
    terminals: Arc<TerminalManager>,
    /// W11: Project ごとの「走らせる」の今（`start_run` の thread が書き、`run_state` が読む）。保存しない（再起動で消える）。
    runs: Arc<Mutex<BTreeMap<String, RunState>>>,
    /// W5: Page の Terminal の直前の誤りの 1 行（画面は W10 から manager の VT が持つ）。保存しない。
    term_views: TermViews,
    /// W12a: Project の resource（repo）を読む口（app.rs が `set_resource_reader` で入れる）。None（試験・E2E）なら読まずに理由の 1 行。
    reader: Option<Box<dyn ResourceReader>>,
    /// W12a: Project ごとの、最後に読んだ時に読めなかった resource の 1 行（draw_page が Page の上に出す・次に読めたら消える）。
    resource_lines: BTreeMap<String, Vec<String>>,
    /// W16-1: Native Direct で Page に重ねた面（メモリには保存しない。W16-3: TextEdit の pid・window_id は `overlay-identity.jsonl` に残し、Shibubu の再起動後に `restore_native` で同じ窓へ繋ぐ。spec は Project の `surface_specs`）。
    #[cfg(feature = "native_direct")]
    pub surfaces: crate::project::native::NativeSurfaces,
    /// Page の Mirror の面（W16M-1）。Mirror の面を持つ Page を初めて描いた時に、保存した同意で作る。
    #[cfg(feature = "mirror")]
    mirror: Option<crate::ui::mirror_view::MirrorPages>,
    /// W12W-1: Page に置いた WebView の面（app.rs の update が frame の終わりに `sync` する）
    pub web: crate::project::web::PageWebViews,
    /// W12W-1: 直前の「repo に保存」の結果 1 行（面の key, 置いた path の 1 行）。その面の Page にだけ出し、同じ面に次の download が届いたら消す
    web_saved: Option<(String, String)>,
    /// W13: 一言と自然言語の編集を読む model（利用者の LLM の CLI）。None = model 無し（名前の表）。裏の thread（`model_job`）と分け合うので Arc。
    /// RQ1: 型は上限の包み（`Budgeted`）のまま持つ（Settings の `Execution limit` が `set_budget` で替え、`budget_used` が使った分を読む）。渡す所で `dyn Llm` に
    llm: Option<Arc<crate::llm::Budgeted>>,
    /// RQ1: Settings に保存した上限（`UiSettings::llm_max_calls` / `llm_max_secs`）。後から届く model もこの値で包む（`set_llm`）
    budget_pref: (Option<u32>, Option<u64>),
    /// RQ2（RQ-USAGE）: 直前に読んだ plan の使用量（`refresh_usage` が裏で読む・None = まだ読んでいない）
    usage: Option<Result<crate::llm::PlanUsage, crate::llm::LlmError>>,
    /// RQ2（RQ-USAGE）: 裏で読んでいる使用量（同時に 1 本）と、最後に読み始めた時刻（unix 秒・0 = まだ）
    usage_job: Option<std::sync::mpsc::Receiver<Result<crate::llm::PlanUsage, crate::llm::LlmError>>>,
    usage_at: i64,
    /// STATUSBAR-1101 追記 2: Claude 以外の AI（`llm_cli::USAGE_CLIS`）の使用量（key = `AiAccount::id`）・裏で読んでいる 1 本・最後に読み始めた時刻（unix 秒・0 = まだ）
    cli_usage: BTreeMap<String, Result<crate::llm::PlanUsage, crate::llm::LlmError>>,
    cli_usage_job: Option<std::sync::mpsc::Receiver<Vec<(String, Result<crate::llm::PlanUsage, crate::llm::LlmError>)>>>,
    cli_usage_at: i64,
    /// W13: model が使えない理由の 1 行（`with_llm(Err)`・model が無い間ずっと出す）
    llm_missing: Option<String>,
    /// RQ-023: Engine の Automation の 3 択（書き手 = app.rs の update が毎 frame `UiSettings::automation` から写す = Settings の行は UiSettings を替える）
    pub automation: crate::domain::permission::AutomationMode,
    /// W13: 直前の生成・編集で model の答えを使えず名前の表で続けた理由の 1 行（次の生成・編集まで）
    llm_fallback: Option<String>,
    /// CU 所見 1: 続けて「AI の答えが使えなかった」回数（作れた・別の失敗で 0 に戻る）。2 回目から失敗の次の一手が `Use simple rules` になる
    unusable_answers: u8,
    /// W13: 裏の thread で model が読んでいる一言か編集（同時に 1 つ・`start_create` / `start_edit` → `poll_model_job`）
    model_job: Option<ModelJob>,
    /// FX2: Home の生成が終わった 1 行（止めた・AI の失敗）と次の一手（`model_progress` の `Build::Ended`）。次の `start_create`・`create_project` で消える
    model_ended: Option<(RegionState, String, Option<crate::ui::build_progress::BuildAction>)>,
    /// FX2（FX-007）: この起動の Home の生成の Runs の行（古い順・最後が今か直前の生成）。保存しない（記録は data dir の `llm-calls.jsonl`）
    model_runs: Vec<RunEntry>,
    /// G104: `Effort ▸` の段（None = Default）。次の `start_create` / `start_edit` から依頼に乗る（読んでいる job は始めた時の段のまま）。書き手 = app.rs（起動と `Intent::SetEffort`）
    pub effort: Option<crate::domain::chat::Effort>,
    /// G84: `Model ▸` の別名（None = Default）。次の `start_create` / `start_edit` から依頼に乗る（provider が受ける別名だけ）。書き手 = app.rs（起動と `Intent::SetModel`）
    pub model: Option<String>,
    /// W14b: 測り終えた使える検査（key = 生成 app の id か Project の id〈その Terminal の席〉）。Terminal の席は裏の thread が書く。保存しない（run ごと）
    usable: Arc<Mutex<BTreeMap<String, Vec<Usable>>>>,
    /// W14b: この process で測り始めた物（生成 app の id・Project の id・Terminal の席は `usable_seat_key`）。1 つにつき 1 回（直しも run ごとに 1 回）。
    /// Terminal の席は席ごと（後から「＋ Terminal」で足した席も次に描いた時に測る・W13c の BG で、足した席が「確かめています」のまま残った）
    usable_started: std::collections::BTreeSet<String>,
    /// W19-2 の配線: Page の依頼を渡す Engine の部品（None = 渡さない・今まで通り「読めません」の 1 行）
    engine: Option<EngineSetup>,
    /// W19-2 の配線: 裏の thread で走っている Engine の run（同時に 1 つ）
    engine_job: Option<EngineJob>,
    /// OPEN-INPLACE-1101 の 5: Project の chat の案の Preview で読んだ答え。同じ文・同じ Project の版の Build now（`start_edit`・1 回だけ）と
    /// Preview の押し直し（`preview_engine_plan`）が model を呼ばずに使う。AI の失敗は覚えない
    planned_edit: Option<PlannedEdit>,
    /// OPEN-INPLACE-1101 の 5: Preview の読みの最中に Build now を押した案（Project の id・案の文）。読み終えた `planned_read` がそのまま Build する
    build_queued: Option<(String, String)>,
    /// AI-COMPOSE-1101: 裏で走っている部品の read。Engine の run とは別（act の最中も読める）・同じ部品は 1 本まで
    reads: Vec<PartRead>,
    /// OPEN-INPLACE-1101 の 4: 前に読めた値ですぐ答える部品の read（node の key・頁の問いの id・値）。次の `take_reads` が先に返す
    kept_answers: Vec<(String, u64, Value)>,
    /// OPEN-INPLACE-1101 の 4: 部品の次の read の読み方（key = `{Project の id}/{node の key}`）。true = 前の値で答えず本物を読む（「Let Shibubu open it」の ✓ の後）・
    /// false = 前の値で答えて読み直さない（裏の読み直しが新しい値を残した後の開き直し）
    next_read: BTreeMap<String, bool>,
    /// OPEN-INPLACE-1101 の 1: 部品から始めた run が止まった理由と次の一手の 1 行（key = `{Project の id}/{node の key}`・その部品の次の read か次の run で消す）
    part_stops: BTreeMap<String, String>,
    /// TALK-HONEST-1101 追記 232: 部品の read が読めなかった答え（key = `{Project の id}/{node の key}`・頁の部品が赤で出している 1 行と開ける時の一言）。
    /// 読めたら・読み直しを始めたら消す。`situation` が部品の名前つきで chat と Engine に渡し、`open_part` が一言を Engine へ。保存しない
    part_errors: BTreeMap<String, ReadMiss>,
    /// TALK-HONEST-1101 追記 232: 部品の「Let Shibubu open it」で始めた run の部品（Project の id・node の key）。その run が ✓ で終われば `rereads` へ・
    /// 他の終わり方と Cancel で消す（承認・判断・同意を待つ間は残す）
    reread_after: Option<(String, String)>,
    /// TALK-HONEST-1101 追記 232: 1 回だけ読み直す部品（`take_rereads` で app.rs が頁を開き直す）
    rereads: Vec<(String, String)>,
    /// ENGINE-GENERIC-1101 追記 234: Project ごとの直近の Engine の run の 1 行（頼み・した事・通った / 止まった・確かめた手）。`poll_engine` が書き、
    /// `situation` が chat と Engine に渡す。保存しない（この起動の間だけ・run の記録は `ENGINE_RUNS_FILE`）
    last_runs: BTreeMap<String, String>,
    /// W19-2 の配線: Project ごとの Engine の 1 行（承認待ち・済み・止めた・失敗）。保存しない
    engine_views: BTreeMap<String, crate::ui::engine_view::EngineView>,
    /// NOTIFY-1101 追記（owner 192）: 右上の toast に出す知らせ（文, 失敗 = × まで残る）。ctx を持たない所（`poll_engine`・`finish_edit`）が積み、
    /// app.rs が毎 frame `take_notices` で `toast::post` へ。chat の中にも Page の上にも描かない
    notices: Vec<(String, bool)>,
    /// WF2: Project ごとの、人を待って止まった Engine の run の id（承認・判断の card が出ている間・再起動の後は `set_engine` が runs 表から）
    engine_waiting: BTreeMap<String, String>,
    /// L7-AGENT: Terminal の agent の一言（runs 表の承認待ちの行）の見張り（`poll_asks`）。
    asks: Asks,
    /// W19-2 の配線: rail の「実行履歴」の Engine の行（新しい順・この起動の間だけ。記録は data dir の `ENGINE_RUNS_FILE`）
    engine_history: Vec<String>,
    /// G86: Project ごとの会話の写し（初めて読む時に `chat_turns` から・書くのは `say` だけ）
    chats: BTreeMap<String, Vec<crate::domain::chat::Turn>>,
    /// SESSIONS-LEFT-1101: 左の会話の一覧の写し（`chat_list`・会話に足す / 消すと None = 次に読み直す）
    chat_rows: Option<Vec<(String, String, String)>>,
    /// SESSIONS-LEFT-1101（owner 210 の 4）: `new_session` で作ってまだ一言の無い session の作った時刻（key → RFC3339・保存しない・`chat_list` の `New session` の行の時刻）
    fresh: BTreeMap<String, String>,
    /// SESSIONS-LEFT-1101（owner 198 の 4）: Project の id → 今の chat session の key（無い Project = Project の id = 1 つ目の session）。
    /// 書き手 = `pick_session`・`new_session`。ui_state（app の session の `CHAT_SESSION_KEY`）に残し、`start` で戻す（owner 189 = 開き直したら前の session・TERM-RESTORE と同じ型）。
    session: BTreeMap<String, String>,
    /// USAGE-1: Project を開いた日の記録の置き場（data dir の `USAGE_FILE`・`open` が DB を開けた時だけ置く。None = 書かない〈`start` の試験・memory の store〉）。
    /// RQ1: 起動と最初の action の行も同じ書き手（`UsageLog`）
    usage_log: Option<UsageLog>,
    /// RQ1 反証の直し: `open` が DB を開けず memory の store で動いている（保存した Project は読めていない = 消えたのではない）。`start` は false
    memory_fallback: bool,
    /// G88b: 録音の帯の今（`Project の id/recorder_ref` ごと・process の間だけ）。録った wav は `dir`（data dir の `takes`）
    takes: Takes,
    /// G95: Project ごとの Terminal の panel（初めて描く・手を当てる時に `ui_state` の `panel.{project}` から読む・手のたびに書く）
    panels: BTreeMap<String, TerminalPanel>,
    /// PV0（図 §49）: 無い app を入れる口（製品 = Homebrew の既定の置き場と /Applications・試験 = 偽の brew と一時の dir・cfg(test) の既定は brew 無し）
    homebrew: acquire::Homebrew,
    /// PV0: 無い app の席ごとの導入の今（key = (Project の id, 席)）。保存しない: 再起動で消え、1 問に戻る（押し直すと先に実際の Mac を読む）
    installs: BTreeMap<(String, String), Install>,
    /// RQ3: 利用者の data dir（`open` が受けた dir・DB を開けなかった起動も Some = 世代を戻せる）。None = `start`（試験・memory の store）
    data_dir: Option<std::path::PathBuf>,
    /// PROJECT-DIR-1101（owner 204）: Project ごとの folder を作る親（製品 = `~/Shibubu`・app.rs の `new` が HOME から入れる）。
    /// None = 作らない（試験・`open` の既定 = 利用者の HOME に書かない）。
    pub projects_root: Option<std::path::PathBuf>,
    /// RQ3: Settings › Data & Recovery の最後の読み（`refresh_data`）と、読んでいる裏の thread
    data: Option<DataSummary>,
    data_job: Option<std::sync::mpsc::Receiver<DataSummary>>,
}

/// PV0: 1 つの席の導入の今（`Generated::installs`）。裏の thread は Resolving・Installing・Checking の間だけ（同時に 1 つ）。
/// 新しい型の理由: Engine の run（`EngineJob`）は model の計画と承認の card を持ち、入れる前の読み・review・読み戻しの段を持たない。
enum Install {
    /// `Start` の後: 実際の Mac を読み、Homebrew の listing を表と照らしている
    Resolving(std::thread::JoinHandle<acquire::Resolution>),
    /// review の 1 件（`Approve and install` を待つ・何も入れていない）
    Review(acquire::Candidate),
    /// 承認の後に listing が替わって戻った review（替わった中身・もう一度の `Approve and install` を待つ・何も入れていない）
    Changed(acquire::Candidate),
    /// 承認した 1 件を入れて読み戻している
    Installing(acquire::Candidate, std::thread::JoinHandle<acquire::Installation>),
    /// `Check again` の読み戻し
    Checking(std::thread::JoinHandle<acquire::Installation>),
    /// 入ったか分からない（`Check again` だけ・入れ直さない）
    Unsure(String),
    /// 入らなかった・入れない（理由の 1 行・公式の頁と前の答えだけ・Install は出さない）
    Failed(String),
}

/// USAGE-1: Project を開いた日の記録（data dir の中・1 行 = `{"at": RFC3339 UTC, "project": id}`・追記だけ）。送らない。読むのは `Generated::usage_report` だけ
/// 契約（RQ1 から・RQ-004・UIUX N16「起動後のFirst Action到達を測る・画面には表示せず製品内のローカル統計」）: 行の種類を 2 つ足す（どれも追記だけ・Project の id も文も持たない）:
/// `{"at": …, "launch": true}` = Shibubu の起動（`Generated::open` が DB を開けた時に 1 行）・`{"at": …, "action": true}` = その起動で初めて、Page の button が
/// 作った app の action を走らせ runtime が Ok を返した時（process で 1 行）。前の版の file の `"project"` の行はそのまま読む（読み手は `"project"` の在る行だけを開いた日に数える）。
/// Wave: USAGE-1, RQ1
pub const USAGE_FILE: &str = "usage.jsonl";

/// `USAGE_FILE` の書き手（追記だけ・書けなくても止めない = 記録は数えるためだけ）。`seen` = この process で既に書いた（Project の id, 地域の日）・
/// `acted` = この起動の最初の action の行を書いた（RQ1・Page の button の 1 回目だけ）。
struct UsageLog {
    path: std::path::PathBuf,
    seen: std::collections::BTreeSet<(String, i64)>,
    acted: bool,
}

impl UsageLog {
    fn new(path: std::path::PathBuf) -> UsageLog {
        UsageLog { path, seen: Default::default(), acted: false }
    }
    /// 1 行を `{"at": 今, ..field}` で足す。
    fn append(&self, now: i64, field: (&str, Value)) {
        let mut line = serde_json::json!({ "at": crate::agent::rfc3339_from_secs(now) });
        line[field.0] = field.1;
        use std::io::Write;
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).and_then(|mut f| writeln!(f, "{line}"));
    }
    /// RQ1: この起動で初めて Page の button が作った app の action を走らせ、確かめて通った時（`Host::invoke`）に 1 行。2 回目からは書かない
    fn note_action(&mut self) {
        if !std::mem::replace(&mut self.acted, true) {
            self.append(unix_now(), ("action", Value::Bool(true)));
        }
    }
}

/// 今の unix 秒（時計が読めなければ 0）。
fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 版・OS・Mac の種類（`sw_vers` と `sysctl` を process で 1 回だけ打つ・usage report と Settings › About が同じ値を使う = 毎 frame process を起こさない）。
/// 戻り値 = (macOS の版, `x86_64` か `x86_64 (Rosetta)` など)。
fn env_facts() -> &'static (String, String) {
    static FACTS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    FACTS.get_or_init(|| {
        let read = |cmd: &str, args: &[&str]| {
            std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|v| !v.is_empty())
        };
        let os = read("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_else(|| "unknown".into());
        // Apple Silicon の上の x86_64 の build は ARCH が x86_64 のまま → Rosetta で動いているかを足す（友達の Mac の種類を知るため）
        let rosetta = if read("/usr/sbin/sysctl", &["-n", "sysctl.proc_translated"]).as_deref() == Some("1") { " (Rosetta)" } else { "" };
        (os, format!("{}{rosetta}", std::env::consts::ARCH))
    })
}

/// RQ1: 秒を `42 s`・`3 min`・`2 h` に（usage report の First action の 1 行）。
fn span(secs: i64) -> String {
    match secs.max(0) {
        s @ 0..=59 => format!("{s} s"),
        s @ 60..=3599 => format!("{} min", s / 60),
        s => format!("{} h", s / 3600),
    }
}

/// USAGE-1: `Generated::usage_report` の中身（時刻を引数で受ける純粋な関数・試験はこれを直に呼ぶ）。
/// `revisions` = (Project の id, unix 秒, origin〈generate | nl_edit | direct_edit | undo〉)・`opens` = (Project の id, unix 秒)・`now` = unix 秒・
/// `offset` = 利用者の地域の UTC からの秒（JST は 32400）・`env` = 版と OS の 1 行。日は利用者の地域の日（(unix 秒 + offset) / 86400。UTC で切ると JST の朝 8 時と夕方が別の日になる）。「7 日以内にまた使った」= 最初の時刻の日より後の日に revision か開いた記録が在り、最初の時刻から 7 日以内。
/// 保証する: Project の id を文に出さない（作った順の `P1`・`P2`）。保証しない: 開いた記録の無い古い data（USAGE-1 より前）の開いた日。
/// 例: `usage_summary(&[("p".into(), t0, "generate".into())], &[], &[], now, 32_400, "v0.1.0 · macOS 14.5 · x86_64")`
/// 契約（RQ1 から・引数 `launches` を `opens` の次に足した）: `launches` = (起動の unix 秒, その起動の最初の action の unix 秒〈無ければ None〉) を起動の順に
/// （`USAGE_FILE` の `launch` の行と、次の `launch` の行より前の最初の `action` の行の組）。「first action」= 起動から、その起動で初めて Page の button が作った app の action を
/// 走らせ Ok が返るまで（N16 の install→first valid Action・relaunch→first Action。operator の既定: Project を作る一言は数えない = 作った物を使い始めた所を測る）。
/// 本体で 2 行目の後に 1 行を足す: `First action · {t} after the first launch · median {t} after a relaunch ({n} relaunches)`（t = `42 s`・`3 min`・`2 h`・
/// 起動の後に action が無ければ `none yet`・再起動が無ければ後半を出さない）・`launches` が空（RQ1 より前の data）なら `First action · not recorded`。
/// 保証しない: RQ1 より前の起動・action を一度も押さずに閉じた起動の長さ（その起動は median に入れない）。
/// Wave: USAGE-1, RQ1
pub fn usage_summary(revisions: &[(String, i64, String)], opens: &[(String, i64)], launches: &[(i64, Option<i64>)], now: i64, offset: i64, env: &str) -> String {
    const DAY: i64 = 86_400;
    let day = |t: i64| (t + offset).div_euclid(DAY);
    let n_days = |n: usize| crate::domain::plural(n, "day");
    #[derive(Default)]
    struct Used {
        times: Vec<i64>,
        open_days: std::collections::BTreeSet<i64>,
        nl: usize,
        direct: usize,
        undo: usize,
    }
    let mut by: BTreeMap<&str, Used> = BTreeMap::new();
    for (id, t, origin) in revisions {
        let u = by.entry(id.as_str()).or_default();
        u.times.push(*t);
        match origin.as_str() {
            "nl_edit" => u.nl += 1,
            "direct_edit" => u.direct += 1,
            "undo" => u.undo += 1,
            _ => {}
        }
    }
    for (id, t) in opens {
        let u = by.entry(id.as_str()).or_default();
        u.times.push(*t);
        u.open_days.insert(day(*t));
    }
    // 作った順（最初の時刻・同じなら id）に P1〜。id は並べるのにだけ使い、文に出さない
    let mut rows: Vec<(i64, &str, Used)> = by.into_iter().map(|(id, u)| (u.times.iter().copied().min().unwrap_or(now), id, u)).collect();
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let mut again_total = 0;
    let lines: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, (first, _, u))| {
            let d0 = day(*first);
            let used: std::collections::BTreeSet<i64> = u.times.iter().map(|&t| day(t)).collect();
            let again = u.times.iter().any(|&t| day(t) > d0 && t - first <= 7 * DAY);
            again_total += again as usize;
            let ago = (day(now) - d0).max(0) as usize;
            let started = if ago == 0 { "started today".to_owned() } else { format!("started {} ago", n_days(ago)) };
            format!(
                "P{} · {started} · used on {} · opened on {} · sentence edits {} · direct edits {} · undos {} · used again within 7 days: {}",
                i + 1,
                n_days(used.len()),
                n_days(u.open_days.len()),
                u.nl,
                u.direct,
                u.undo,
                if again { "yes" } else { "no" }
            )
        })
        .collect();
    let mut out = vec![format!("Shibubu usage report · {env}"), format!("Projects {} · used again within 7 days {again_total}", rows.len())];
    // RQ1（N16）: 最初の起動から最初の action まで・再起動から最初の action までの中央値（action の無い再起動は中央値に入れない）
    out.push(match launches.split_first() {
        None => "First action · not recorded".to_owned(),
        Some(((t0, a0), rest)) => {
            let first = a0.map_or_else(|| "none yet".to_owned(), |a| span(a - t0));
            let mut later: Vec<i64> = rest.iter().filter_map(|(t, a)| a.map(|a| a - t)).collect();
            later.sort_unstable();
            let n = rest.len();
            let relaunches = if n == 1 { "1 relaunch".to_owned() } else { format!("{n} relaunches") };
            match (n, later.get(later.len().saturating_sub(1) / 2)) {
                (0, _) => format!("First action · {first} after the first launch"),
                (_, Some(m)) => format!("First action · {first} after the first launch · median {} after a relaunch ({relaunches})", span(*m)),
                (_, None) => format!("First action · {first} after the first launch · none yet after a relaunch ({relaunches})"),
            }
        }
    });
    out.extend(lines);
    out.push("No project names, file paths, or anything you typed are in this report".into());
    out.join("\n")
}

/// W19-2 の配線（図 §33）: Page の依頼を走らせる口。製品は `engine_page::CoreEngine`（`capability_engine::run`）。generated.rs は Engine の型を持たない
/// （tests/yc_demo_e2e.rs が generated.rs を #[path] で引き、capability_engine・adapters を持たない）。`run` は裏の thread で呼ばれる（待ってよい）。
/// 契約（RQ3 から）: method `export_workflows`・`import_workflows` を足した（既定の本体 = Workflow も接続も無い・試験の偽の Engine はそのまま）。
/// generated.rs は `capability_engine::Workflow` を名指せない（上の理由）ので、Workflow は JSON の値（`shibubu.workflow/1`）で受け渡す。
/// Wave: W19-2, RQ3
pub trait PageEngine: Send + Sync {
    fn run(&self, req: EngineRequest) -> EngineAnswer;
    /// WF2: 再起動の後に、人を待っている run（runs 表から・`set_engine` が 1 回）。既定 = 無い（runs 表を持たない Engine・試験の偽物）
    /// Wave: WF2
    fn waiting(&self, store: &Store) -> Vec<EngineWaiting> {
        let _ = store;
        Vec::new()
    }
    /// RQ3（RQ-012・Export）: この Project の Workflow（data dir の `workflows/` の中で `ui` が `project_id` の物・JSON のまま）と、それが使う接続の参照
    /// （step の capability の `<provider>` と Adapter の種類 = `ProjectResource { id: provider, kind: AdapterKind の語, uri: None }`・鍵・token・URL の query を持たない）。
    /// 例（`Generated::export_project`）: `let (workflows, connections) = engine.export_workflows(&project_id);`。保証する: 何も書かない。
    fn export_workflows(&self, _project_id: &str) -> (Vec<Value>, Vec<project::ProjectResource>) {
        (Vec::new(), Vec::new())
    }
    /// RQ3（RQ-012・Import）: 書き出した Workflow を `ui` = `project_id`（取り込んだ先の Project の id）に替えて data dir の `workflows/` に保存し、この Mac で
    /// まだ使えない接続の参照（registry の `check` が Ready でない provider）を返す（`Generated::import_project` が開いた Page に
    /// `{name} isn't connected on this Mac · Connect` の 1 行・`Connect` = Connections の画面）。
    /// 保証する: 検査（`capability_engine::validate`）を通らない Workflow は保存しない（Err の 1 行・Project の取り込みは止めない）・既存の Workflow の id とは重ねない。
    fn import_workflows(&self, _project_id: &str, _workflows: &[Value]) -> Result<Vec<project::ProjectResource>, String> {
        Ok(Vec::new())
    }
    /// TALK-HONEST-1101（owner 206 追記）: 会話の AI に渡す「この Mac で Shibubu が今出来る事」の行（`domain::chat::talk_abilities`・Engine が見る手段と同じ物）。
    /// 保証する: 何も起こさない（MCP の server・bridge・git を起こさない）。既定 = 空（Engine を持たない・試験の偽物 = 会話は前と同じ）。
    fn abilities(&self) -> String {
        String::new()
    }
    /// AI-COMPOSE-1101（owner 206）: Page の AI が書いた部品の `shibubu.read(app, what)` = 名指しの app を読むだけ（書く手・送る手は 1 手も無い）で、
    /// `what` の形の JSON にする。Err = 頁の Promise の reject に渡す人の 1 行。裏の thread から呼ぶ（待つ・`Generated::start_part_read`）。
    /// 既定 = 読めない（Engine を持たない・試験の偽物）。
    /// TALK-HONEST-1101 追記 232（契約を替えた）: Err を `ReadMiss`（1 行 ＋ Shibubu が開けば読める時の一言）に。
    fn read(&self, _app: &str, _what: &str, _llm: Option<Arc<dyn crate::llm::Llm>>) -> Result<Value, ReadMiss> {
        Err(READ_NONE.into())
    }
    /// ENGINE-GENERIC-1101 追記 234: 今 窓を開いている app の状況の 1 行（`capability_engine::open_windows`・AX を読む = 裏の thread から・`with_windows`）。
    /// 契約を足した理由: 前は Engine の run の中だけで足していた = chat の AI は窓を知らなかった。既定 = None（Engine を持たない・試験の偽物）。
    fn windows(&self) -> Option<String> {
        None
    }
    /// OPEN-INPLACE-1101 の 5（234 の共有の状況・owner 240）: 一言の Engine の plan（手の一覧）を、何も送らずに人の言葉の行にする（`interpret` → `discover` →
    /// 事実の段 → `generate` まで・手は 1 つも走らせない・読むだけの手も走らせない）。行は承認の card と同じ字（`engine_view::approval_line` の手の部分）。
    /// 作った手順は run と同じく保存する = その後の Build now は同じ手順を使う（生成 0 回）= chat の案に書いた手と Engine が打つ手が同じ。
    /// 裏の thread から呼ぶ（`Generated::preview_engine_plan`）。Err = 手順を作れない理由の 1 行（Engine の `failure_line`）。
    /// 既定 = 作れない（Engine を持たない・試験の偽物）。
    /// Wave: OPEN-INPLACE-1101
    fn plan(&self, req: EngineRequest) -> Result<Vec<String>, String> {
        let _ = req;
        Err(PLAN_NONE.into())
    }
    /// ENGINE-AUDIT W9（節 14「一言の種類・名指し・曖昧さを表せない」・直した後の流れ B「interpret が種類と宛先を決める」）: 一言を interpret（AI 1 回）だけで
    /// 操作に分け、操作ごとの種類（`domain::chat::OperationKind`）と want を返す。何も送らず・窓も読まない。分けた操作は覚え、同じ一言の次の `run` は interpret を
    /// 呼び直さない（`CoreEngine::planned` = Preview → Build と同じ仕組み・AI は増えない）。呼ぶ側（本体 W9 = app.rs の `talk_in` → `domain::chat::route` → `follow_route`）は
    /// act だけを Engine に、change だけを composer に、question を会話の AI に回す（app の操作を composer に回さない・混ざった頼みの片方を黙って落とさない）。裏の thread から呼ぶ。
    /// 本体で返す物を `Classified`（種類・want・宛先・名指し・候補）にした = 候補が 2 つ以上・名指したのに宛先が無い時に chat が 1 回聞くため。
    /// 既定 = 分けられない（Engine を持たない・試験の偽物）。Err = 理由の 1 行。
    fn classify(&self, req: EngineRequest) -> Result<Vec<crate::domain::chat::Classified>, String> {
        let _ = req;
        Err(CLASSIFY_NONE.into())
    }
    /// OPEN-INPLACE-1101 の 2（Always allow）: 承認待ちの手 `steps` が全部、前に「Always allow」で決めた範囲（`surface::always_allowed` の行 = 手の input の
    /// `app_name`〈input に `bundle_id` が在ればそれも〉が同じ・action の id が同じ）に在るか。true なら `Generated::poll_engine` が承認の card を出さず
    /// Approve と同じ道で 1 回送る。空の `steps` は false。速く答える（file を 1 回読むだけ・UI の thread から呼ぶ）。
    /// 既定 = false（いつも聞く）。`CoreEngine` は `act_consents` の file を読む。
    /// Wave: OPEN-INPLACE-1101
    fn skips_approval(&self, steps: &[crate::ui::engine_view::ApprovalStep]) -> bool {
        let _ = steps;
        false
    }
}

/// OPEN-INPLACE-1101 の 5: Engine の plan を作れない build・Engine の時の 1 行（`PageEngine::plan` の既定）。
pub const PLAN_NONE: &str = "Shibubu can't list its steps for this yet";
/// ENGINE-AUDIT W9: 一言を分けられない時の 1 行（`PageEngine::classify` の既定・Engine か AI が無い）。
pub const CLASSIFY_NONE: &str = "Shibubu can't read what kind of request this is yet";

/// ENGINE-AUDIT W9 本体: chat の裏の thread が app.rs の `poll_talk` に返す物。`Routed` = 一言を分けた結果（先に届く・`Generated::follow_route`）・`Reply` = 会話の AI の答え
/// （`Route::talk` の時だけ後から届く）。1 つの enum にした理由: 同じ channel で順に届き、Home の会話（分けない）も同じ受け口を使う
pub enum TalkMsg {
    Reply(Result<Value, crate::llm::LlmError>),
    Routed { project: String, say: String, route: crate::domain::chat::Route },
}

/// OPEN-INPLACE-1101 の 5: Project の chat の案の Preview の 1 行目（Engine の手の行の上）。
pub const PLAN_STEPS: &str = "When you press Build now, Shibubu does these steps:";

/// CHAT-ONE-1101: Project の chat の案の Preview の 1 行（Project の変更・Engine へ行かない文）。
pub const CHANGE_PREVIEW: &str = "When you press Build now, Shibubu makes this change to this project.";

/// ENGINE-GENERIC-1101 追記 234: chat の thread（app.rs の `talk_in`）と Engine の thread（`start_engine_with`）が、`Generated::situation` に今 窓を開いている
/// app の 1 行を足すただ 1 つの所（両方がこれを通す = 同じ状況なら `Situation::text` が同じ字）。AX を読むので UI の thread で呼ばない。
/// 例: `let now = with_windows(engine.as_deref(), situation); let block = now.text();`
pub fn with_windows(engine: Option<&dyn PageEngine>, mut s: crate::domain::chat::Situation) -> crate::domain::chat::Situation {
    s.facts.extend(engine.and_then(|e| e.windows()));
    s
}

/// AI-COMPOSE-1101: app を読めない build・Engine の時に部品へ返す 1 行（`PageEngine::read` の既定）。
pub const READ_NONE: &str = "Reading from apps isn't ready yet";
/// TALK-HONEST-1101 追記 232: 部品の「Let Shibubu open it」を押したが、その部品に開ける一言が無い（読めた・読み直している・読む道が無い）時の 1 行（`Generated::open_part`）。
pub const OPEN_NONE: &str = "Nothing to open for this part now · read it again first";

/// TALK-HONEST-1101 追記 232（operator の決め「Let Shibubu open it の button は作る」）: 部品の read が読めなかった答え。`line` = 頁に出す 1 行・
/// `open` = Shibubu が app を開けば読める時の Engine への一言（`Open {探した物} in {app}`・探した物と app は read の data・部品の button が
/// `Generated::open_part` で `ask_engine` に渡す）。読む道が無い・AI が無い・止まった等は None（button を出さない）。
/// 新しくした理由: 前の Err（String）は頁の 1 行だけで、押すと開ける時の一言を持てない（既存の誤りの型に 1 行と一言を持つ物が無い）。
/// 例: `Err(ReadMiss { line: "Looked for the guitar list in {app}, but …".into(), open: Some("Open the guitar list in {app}".into()) })`・`Err(READ_NONE.into())`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadMiss {
    pub line: String,
    pub open: Option<String>,
}

impl From<String> for ReadMiss {
    fn from(line: String) -> Self {
        ReadMiss { line, open: None }
    }
}

impl From<&str> for ReadMiss {
    fn from(line: &str) -> Self {
        line.to_owned().into()
    }
}

/// WF2: `PageEngine::waiting` の 1 件（どの Project の・どの run の・頼んだ一言・画面に出す答え〈承認か Decide の card〉）。
/// Wave: WF2
pub struct EngineWaiting {
    pub project_id: String,
    pub run_id: String,
    pub goal: String,
    pub reply: EngineReply,
}

/// WF1・WF2: Engine の run を runs 表に残す口（`start_engine` が作る・Engine の thread が書く）・新しい行の親（Project の app の session と版・
/// space = Project の id）と、新しい run か止まった run の続きか。
/// Wave: WF1
pub struct EngineRuns {
    pub store: Arc<Mutex<Store>>,
    pub session_id: String,
    pub space_id: String,
    pub app_version: u32,
    pub start: RunStart,
}

/// WF2: Engine の run の始め方。
/// Wave: WF2
#[derive(Debug, Clone, PartialEq)]
pub enum RunStart {
    /// 新しい run（run の id = `EngineRequest::run_id`）
    New,
    /// 止まった run（`EngineRequest::run_id`）を続ける。承認は `EngineRequest::consents`・`choice` = Decide で選んだ choice の id
    Resume { choice: Option<String> },
}

/// `PageEngine::run` に渡す物（Project の app・この run の id・利用者の一言・この run だけの承認・AI に任せる app・model）。
/// Wave: W19-2
pub struct EngineRequest {
    pub app_id: String,
    pub run_id: String,
    pub goal: String,
    pub consents: Vec<crate::domain::permission::ConsentReceipt>,
    /// 承認した手のうち画面の操作（input に `pid` が在る手）の input（宛先の pid・`app_name`・任意の `bundle_id` と `document`・重ねない）。
    /// run の前に宛先の app を app ごとの AI の同意（`surface::grant_act_consent`・`gui-act-consents.jsonl` の 1 つの鍵）に書く
    /// （W19-1 の残り・operator 2026-10-24）。承認の無い run は空
    /// OPEN-INPLACE-1101 の 2: 「Always allow」で送った run は、宛先ごとに `"always": [その app で承認した手の action の id]` を足す（`CoreEngine::grant` が書く）
    pub grants: Vec<Value>,
    /// M2 G-a: 今の Page に置いた app の面の bundle id（`SurfaceTarget::App` がちょうど 1 つ・Url の面は数えない・面の無い Page と違う app が 2 つ以上は None）。
    /// Engine は一言が app を名指さない時の宛先にする（`capability_engine::EngineContext::target`）
    pub target: Option<String>,
    /// G73: `target` と同じ Page の、その app の面の書類（`SurfaceTarget::App` の `file` の file 名・違う file が 2 つ以上は None）。
    /// Engine は宛先の窓を役割ではなく書類で決める（`capability_engine::EngineContext::document`）
    pub document: Option<String>,
    /// G81: `document` の書類の disk の場所（`file://` + Project の repo の下の絶対 path・repo か面の file が無ければ None）。
    /// Engine は書類に字を置く手の済みを、窓に加えてこの file の中身でも確かめる（`capability_engine::EngineContext::document_file`）
    pub document_file: Option<String>,
    /// MIRROR-PIN-1102: 今の Page の Mirror に映っている `target` の app の窓 = (pid, CG の窓番号)（`engine_request` が `MirrorPages::shown` と `pin_window` で決める）。
    /// None = Page に `target` を映す面が無い・どれも映していない（今まで通り Adapter が役割と書類で窓を決める）。Some(Ok) = この窓だけを読み・押す
    /// （同じ app の他の窓で代用しない）。Some(Err(1 行)) = 同じ app の窓が Page に 2 つ以上映っていて、どれも触っていない（Engine はこの 1 行で何も送らずに閉じる）。
    /// `gui::GuiTarget` でなく組なのは、この file を取り込む試験の crate（tests/yc_demo_e2e.rs・mirror_e0.rs）が `gui` を持たないから（bundle id は `target`）
    pub window: Option<Result<(u32, u32), String>>,
    /// B74: Project の repo の場所（`file://` + 絶対 path・repo が無ければ None）。Engine は model が書いた repo からの相対の書類の読みをこの下の絶対に直す
    /// （`capability_engine::EngineContext::repo`・model には渡さない）
    pub repo: Option<String>,
    pub llm: Option<Arc<dyn crate::llm::Llm>>,
    /// 裏の thread で長く待つ間の進行の 1 行（Some = Page の「進めています」の下に出す・None = 消す）。`start_engine` が作り（呼ぶと egui を起こす）、
    /// `poll_engine` が毎 frame `EngineView::Running::waiting` に写す。製品は `CoreEngine` が registry（`AdapterRegistry::set_progress`・W19-3b G2）と
    /// 解析の Backend（`AnalysisBackend::set_progress`・W19-3-fix）の 2 つに配る
    pub progress: Arc<dyn Fn(Option<String>) + Send + Sync>,
    /// WF1: run を残す runs 表と始め方（None = 残さない・止まった run は同じ一言を最初から run し直す）
    pub runs: Option<EngineRuns>,
    /// RQ-023: Automation の 3 択（`Generated::automation` の写し）
    pub automation: crate::domain::permission::AutomationMode,
    /// ENGINE-GENERIC-1101（owner 211）: 一言と一緒に渡す今の状況（Page の状況 `Generated::situation` の後ろに、部品が `ask_engine` で渡した物）。
    /// Engine は `capability_engine::EngineContext::situation` で読む
    pub situation: crate::domain::chat::Situation,
    /// ENGINE-ADJUST-1102 ①（owner 256・人の Stop）: この run の人の Stop の印。`start_engine_with` が runtime の `running` の表に置いた flag（`Runtime::track`）= Runs の Stop
    /// （`Runtime::cancel`・runs 表に行が出来てから）と Running の card の Stop（`Generated::stop_engine`）が同じ物を立てる。Engine は `EngineContext::stop` と
    /// `AdapterRegistry::set_stop` で見る。一言を始めない呼び手（`plan`・試験）は立たない印
    pub stop: Arc<std::sync::atomic::AtomicBool>,
}

/// ENGINE-GENERIC-1101: Engine の 1 つの口（`Generated::ask_engine`）が始められない時の 1 行（Engine が走っている・Engine か Project が無い）。
pub const ENGINE_BUSY: &str = "Shibubu is busy · try again shortly";
/// ENGINE-ADJUST-1102 ①: Stop を押してから Engine が閉じるまでの進行の 1 行（`Generated::stop_engine` が `EngineJob::waiting` に置く・Running の card の下）。
pub const STOPPING_LINE: &str = "Stopping… nothing more will be sent";

/// `PageEngine::run` の答え（画面に出す形と、`ENGINE_RUNS_FILE` の 1 行に入れる記録）。
/// Wave: W19-2
pub struct EngineAnswer {
    pub reply: EngineReply,
    /// `ENGINE_RUNS_FILE` の 1 行の `record`（Engine の記録）か `error`（止めた・失敗の種類と原文）。画面には出さない
    pub log: (&'static str, Value),
}

/// 画面に出す答え（文は人の言葉だけ・`engine_page` が作る）。
/// Wave: W19-2
#[derive(Debug, Clone, PartialEq)]
pub enum EngineReply {
    /// 走った。`verified` = 元の app の実状態で確かめた数（0 なら済みと出さない）・`saved` = そのうち保存した file（disk）を読んで確かめた数
    /// （G81: 0 なら済みの 1 行は保存を言わない = 窓だけ確かめた事は窓の語で）・`history` = rail の実行履歴の行
    /// WF-E: `partial` = 頼んだ差分を読み戻していない理由（Some なら済みと出さない・`EngineRecord::partial`）・`without_ai` = AI を使わなかった理由
    /// （`EngineRecord::without_ai`・済みの 1 行に足す）
    /// ENGINE-AUDIT W7: `already` = 手を 1 つも打たなかった理由（頼んだ形がもう在った・`EngineRecord::already`・Some の時 `verified` はそれを確かめた読みの数）。
    /// 画面は「Already like that」の 1 行（ATTACH・それまでは verified の済みの 1 行で出る）
    Done { verified: usize, saved: usize, history: Vec<String>, partial: Option<String>, without_ai: Option<String>, already: Option<String> },
    /// 何も変えずに止めた。`steps` = 承認待ちの手（承認で通るかもしれない）・`line` = 承認では通らない時の 1 行（人の lease・許可・同意）
    Denied { steps: Vec<crate::ui::engine_view::ApprovalStep>, line: String },
    /// 宛先の app に AI に任せる同意が無くて送らなかった（`MacGui` の門・act の記録に `needs_consent`）。`line` = 何も送っていない事と押す物の 1 行
    NeedsConsent { line: String },
    /// 理由と次の一手の 1 行（済みとは出さない）
    Failed { line: String },
    /// WF2: 手順の途中で人の判断を待つ（何も送っていない手が残る・run は runs 表に `waiting_human`）。line = 出し直した理由（None = 初めて）
    /// Wave: WF2
    Decide { question: String, choices: Vec<crate::ui::engine_view::DecideOption>, line: Option<String> },
    /// WF2: 止まっている間に元データが変わったので、承認した手を送らず承認の card を出し直す（`line` = 理由の 1 行・run は `waiting_approval` のまま）
    /// Wave: WF2
    Recheck { steps: Vec<crate::ui::engine_view::ApprovalStep>, line: String },
}

/// W19-2 の配線: Page の依頼を走らせる口と、記録の置き場。app.rs が起動の時に 1 回 `set_engine` で渡す（試験は偽の registry の Engine と一時の dir）。
/// Wave: W19-2
pub struct EngineSetup {
    pub engine: Arc<dyn PageEngine>,
    /// `ENGINE_RUNS_FILE` の親（利用者の data dir）
    pub data_dir: std::path::PathBuf,
}

/// W19-2 の配線: Engine の run 1 回の記録の置き場（data dir の下・追記だけの JSONL・1 行 = `{"at", "goal", "run_id", "record"}` か `{"at", "goal", "run_id", "error"}`）。
/// 試験の側の oracle（Handoff と各手の Outcome）と evidence はここを読む。画面には出さない（rail は `engine_page::history_lines` の人の言葉だけ）。
/// Wave: W19-2
pub const ENGINE_RUNS_FILE: &str = "engine-runs.jsonl";

/// RQ3（RQ-011）: data dir の下の世代の置き場。1 世代 = 1 つの dir `<UTC の YYYYMMDDTHHMMSSZ>-<why>/`（中に `kangae.sqlite3` = `Store::back_up_to` の写し・
/// `sessions.json` = 設定の写し。`before-update` は DB だけ）。`<why>` は `manual`（`Back up now`）・`daily`（起動で前の daily から 24 時間たっていれば 1 つ）・
/// `before-update`（`Store::open_backing_up`）・`before-restore`（`restore_backup` が戻す前の今の data）・`damaged`（開けなかった DB・設定を隔離した物）。
/// 残す数（`Generated::prune_records`）: daily 7・manual 10・before-update 3・before-restore 3・damaged は消さない（`Delete my data › Backups` だけが消す）。
/// Wave: RQ3
pub const BACKUPS_DIR: &str = "backups";
/// RQ3（RQ-011）: 次の起動で戻す世代の予約の file（data dir の直下・中身は世代の id 1 行）。`Generated::open` が DB を開く前に読んで戻し、消す。
/// Wave: RQ3
pub const RESTORE_MARKER: &str = "restore-on-open";
/// RQ3（RQ-012）: Project を書き出した file の拡張子（中身は `ProjectExport` の JSON・UTF-8）。
/// Wave: RQ3
pub const PROJECT_FILE_EXT: &str = "shibubu";
/// RQ3（RQ-012）: `ProjectExport::schema` の値（`WORKFLOW_SCHEMA`・`CAPABILITY_SCHEMA` と同じ書き方）。
/// Wave: RQ3
pub const PROJECT_EXPORT_SCHEMA: &str = "shibubu.project/1";

/// RQ3（RQ-012・Export / Import）: `.shibubu` の file の中身（1 Project）。別の Mac・別の data dir の Shibubu が `Generated::import_project` で同じ Project として開く。
/// 保存の形: `{"schema": "shibubu.project/1", "project": ProjectDefinitionV1, "app": AppDefinitionV3, "state": {"<ui_state の key>": …}, "workflows": [Workflow の JSON],
/// "connections": [{"id": "github", "kind": "api"}]}`（未知の欄は拒否 = 新しい版の file は schema の 1 行で断る）。
/// 入れる: Project の定義と Pages・app の定義・面の中身と配置と Page の打ちかけ（app の session の `ui_state`）・Workflow・接続の参照（名前と種類だけ）。
/// 入れない（MH-S6「Secret、Cookie、OAuthトークン、元アプリ本体を含めない」）: 鍵・token・cookie・web の store・元の app と利用者の file・Terminal の panel
/// （`PANEL_KEY`）と session・Mirror の同意・resource の読んだ中身（`resource.*` = 取り込んだ先で読み直す）・run の履歴と承認待ち・版の履歴（Undo は取り込んだ所から）。
/// 例: `let file = g.export_project(&project, &downloads)?; let view = other.import_project(&file)?;`
/// 保証する: `state` の値は `storage::redact` を通さない（利用者の data そのもの）が、key は上の除外を引いた物だけ。保証しない: 取り込んだ先に同じ folder が在る事
/// （`resources` の repo の uri はそのまま・無ければ Page の 1 行）・取り込んだ先で接続が使える事（`connections` が足りない物を言う）。
/// 新しくした理由: `ProjectDefinitionV1`・`AppDefinitionV3` は DB に保存する 1 つの定義（`deny_unknown_fields`）で、app と面の中身と Workflow と接続の参照を
/// 1 つの file に束ねる型が無い（W20 の `.shibubu` の書き出しもこの型を使う）。
/// Wave: RQ3
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectExport {
    pub schema: String,
    pub project: ProjectDefinitionV1,
    pub app: AppDefinitionV3,
    #[serde(default)]
    pub state: BTreeMap<String, Value>,
    #[serde(default)]
    pub workflows: Vec<Value>,
    #[serde(default)]
    pub connections: Vec<project::ProjectResource>,
}

/// RQ3（RQ-010・RQ-011・RQ-012・RQ-014）: Settings › Data & Recovery の 1 回の読み（`Generated::refresh_data` が裏で作り、`data_summary` が返す）。
/// `location` = data dir（画面は home を `~` に）・`projects` = (Project の名前, byte)・`kept` = 種類ごとの byte・`backups` = (世代の id, `2 h ago · Daily · 2.1 MB`
/// の 1 行・新しい順)・`restoring` = `RESTORE_MARKER` の世代の id・`imports` = 取り込める `.shibubu`（`refresh_data` の `look_in` の直下・新しい順）・`checked_at` = 読んだ unix 秒。
/// 例: `crate::settings::draw(.., self.generated.data_summary(), &mut self.shell.data_confirm)`。保証しない: 読んだ後の変化（各 action の後に `refresh_data(.., true)`）。
/// 新しくした理由: 既存の (項目, 値) の行（`diagnostics`・`project_settings_rows`）は行ごとの button の宛先（世代の id・`DataKind`・file）を持てず、`settings::draw` の
/// 引数を 5 つ増やさないため 1 つに束ねた（中身は既存の型と tuple だけ）。
/// Wave: RQ3
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataSummary {
    pub location: std::path::PathBuf,
    pub projects: Vec<(String, u64)>,
    pub kept: Vec<(crate::ui::shell::DataKind, u64)>,
    pub backups: Vec<(String, String)>,
    pub restoring: Option<String>,
    pub imports: Vec<std::path::PathBuf>,
    pub checked_at: i64,
}

/// RQ3: DB の 3 つの file（本体と WAL の 2 つ）。世代の写し・隔離・戻しで一緒に動かす（戻した DB に古い WAL を当て直させない）。
const DB_FILES: [&str; 3] = ["kangae.sqlite3", "kangae.sqlite3-wal", "kangae.sqlite3-shm"];
/// RQ3: 設定の file（app.rs が `model::Store::load` で読む・`Generated::open` の後）。
const SETTINGS_FILE: &str = "sessions.json";
/// RQ3（`ui::shell::DataKind` の doc の持ち物）: 種類ごとの data dir の直下の記録。名前は持ち主の定数の写し（持ち主の module は tests/yc_demo_e2e.rs の crate に無い）:
/// engine-trace = `capability_engine::TRACE_FILE`・llm-calls = `llm_cli::CALL_LOG`・agent-sessions = `own_usage::AGENT_LOG`・mirror-relay = `surface::mirror::relay_log_path`・gui-acts = `gui::ACT_LOG_FILE`・
/// overlay / overlay-identity = `project::native::{OVERLAY_LOG, IDENTITY_LOG}`。
const HISTORY_FILES: [&str; 5] = [ENGINE_RUNS_FILE, "engine-trace.jsonl", "llm-calls.jsonl", USAGE_FILE, "agent-sessions.jsonl"];
const SCREEN_FILES: [&str; 4] = ["mirror-relay.jsonl", "gui-acts.jsonl", "overlay.jsonl", "overlay-identity.jsonl"];
/// RQ3: 足した AI の login の置き場（`llm_cli::ACCOUNTS_DIR` の写し・同じ理由）。
const ACCOUNTS_DIR: &str = "ai-accounts";
/// RQ3: 前の版の `model::Store::load` が data dir に置いた壊れた設定（`sessions.json.corrupt-<unix 秒>`）。Backups の list に出し、戻せる。
const LEGACY_DAMAGED: &str = "sessions.json.corrupt-";
/// RQ3: 記録の JSONL 1 file の上限（`prune_records`・超えたら古い行から）。
const RECORD_CAP: usize = 10 * 1024 * 1024;
/// RQ3（`Generated::open` の ③）: 壊れた DB を隔離して空から開いた起動の Home の 1 行（`home::draw` がこの note に `Restore…` を付ける）。
pub const DAMAGED_NOTE: &str = "Your saved projects were damaged and set aside · Restore them in Settings › Data & Recovery";

/// RQ3: unix 秒 → 世代の id の頭 `YYYYMMDDTHHMMSSZ`（`model::utc_stamp` と同じ形。generated は model を名指せない）。
fn stamp(secs: i64) -> String {
    crate::agent::rfc3339_from_secs(secs).replace(['-', ':'], "")
}

/// RQ3: 世代の id `<stamp>-<why>` の (unix 秒, why)。形が違えば None。
fn backup_id(id: &str) -> Option<(i64, &str)> {
    let (s, why) = id.split_once('-')?;
    let b = s.as_bytes();
    if !s.is_ascii() || b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    let rfc = format!("{}-{}-{}T{}:{}:{}Z", &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13], &s[13..15]);
    Some((crate::agent::secs_from_rfc3339(&rfc)?, why))
}

/// RQ3: 世代の id から戻す file（(data dir の中の名前, 世代の中の path)）。無い・形の違う・dir の外を指す id と、DB も設定も無い世代は None。
fn backup_files(dir: &Path, id: &str) -> Option<Vec<(&'static str, std::path::PathBuf)>> {
    if id.contains(['/', '\\']) || id.starts_with('.') {
        return None;
    }
    if id.starts_with(LEGACY_DAMAGED) {
        let f = dir.join(id);
        return f.is_file().then(|| vec![(SETTINGS_FILE, f)]);
    }
    backup_id(id)?;
    let set = dir.join(BACKUPS_DIR).join(id);
    let files: Vec<_> = DB_FILES.iter().chain([&SETTINGS_FILE]).map(|n| (*n, set.join(n))).filter(|(_, p)| p.is_file()).collect();
    files.iter().any(|(n, _)| *n == DB_FILES[0] || *n == SETTINGS_FILE).then_some(files)
}

/// RQ3: その why の一番新しい世代の時刻（`daily` の 24 時間の判定）。
fn last_backup(dir: &Path, why: &str) -> Option<i64> {
    let rd = std::fs::read_dir(dir.join(BACKUPS_DIR)).ok()?;
    rd.flatten().filter_map(|e| backup_id(&e.file_name().to_string_lossy()).filter(|(_, w)| *w == why).map(|(at, _)| at)).max()
}

/// RQ3: 前の版が隔離した設定（`LEGACY_DAMAGED`）の file。
fn legacy_damaged(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir).map_or(Vec::new(), |rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(LEGACY_DAMAGED)).map(|e| e.path()).collect())
}

/// RQ3: 在れば写す（無ければ何もしない = 設定の file の無い data dir）。
fn copy_if_there(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::copy(from, to) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !from.exists() => Ok(()),
        r => r.map(|_| ()),
    }
}

/// RQ3: file か dir を消す（無いのは Ok・`None` = data dir の無い起動）。
/// ENGINE-AUDIT W2: data dir の `workflows/<id>.json` のうち `ui` がこの Project の物を消す（`delete_project`・generated.rs は capability_engine の型を名指せないので JSON で見る）。
/// 読めない file・消せない file は残す（Project の削除は止めない）。
fn forget_project_workflows(dir: &Path, project_id: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            let mine = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).is_some_and(|v| v["ui"] == project_id);
            if mine {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

fn remove_all(p: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let Some(p) = p else { return Ok(()) };
    let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// RQ3（`Generated::open` の ①）: `RESTORE_MARKER` の世代を戻す。戻す種類（DB・設定）の今の file を `before-restore` の世代へ移してから写す。印は必ず消す。
/// 写せなければ移した file を元に戻す（今の data のまま）。戻り値 = Home の 1 行（予約が無ければ None）。
fn restore_on_open(dir: &Path, now: i64) -> Option<String> {
    let marker = dir.join(RESTORE_MARKER);
    let id = std::fs::read_to_string(&marker).ok()?.trim().to_owned();
    let _ = std::fs::remove_file(&marker);
    let Some(files) = backup_files(dir, &id) else { return Some("Couldn't restore that backup · it isn't there anymore".into()) };
    let keep = dir.join(BACKUPS_DIR).join(format!("{}-before-restore", stamp(now)));
    let db = files.iter().any(|(n, _)| DB_FILES.contains(n));
    let settings = files.iter().any(|(n, _)| *n == SETTINGS_FILE);
    let out: Vec<&str> = DB_FILES.into_iter().filter(|_| db).chain(settings.then_some(SETTINGS_FILE)).filter(|n| dir.join(n).exists()).collect();
    let (mut moved, mut copied) = (Vec::new(), Vec::new());
    let mut go = || -> std::io::Result<()> {
        std::fs::create_dir_all(&keep)?;
        for n in &out {
            std::fs::rename(dir.join(n), keep.join(n))?;
            moved.push(*n);
        }
        for (n, from) in &files {
            std::fs::copy(from, dir.join(n))?;
            copied.push(*n);
        }
        Ok(())
    };
    match go() {
        // `Settings › Data & Recovery` で終えない（その 1 行には Home が `Restore…` を付ける = 壊れた時だけ）
        Ok(()) => Some("Restored your backup · what you had before is kept as “Before a restore” in Backups".into()),
        Err(e) => {
            for n in copied {
                let _ = std::fs::remove_file(dir.join(n));
            }
            for n in moved {
                let _ = std::fs::rename(keep.join(n), dir.join(n));
            }
            let _ = std::fs::remove_dir(&keep);
            Some(format!("Couldn't restore that backup ({}) · your data is as it was", e.kind()))
        }
    }
}

/// RQ3（`Generated::open` の ③）: 開けなかった DB の 3 つの file を `damaged` の世代へ移す（消さない）。
fn set_aside_db(dir: &Path, now: i64) -> std::io::Result<()> {
    let set = dir.join(BACKUPS_DIR).join(format!("{}-damaged", stamp(now)));
    std::fs::create_dir_all(&set)?;
    for n in DB_FILES.into_iter().filter(|n| dir.join(n).exists()) {
        std::fs::rename(dir.join(n), set.join(n))?;
    }
    Ok(())
}

/// RQ3: file と dir の中の byte の和（symlink は辿らない・読めない所は 0）。
fn dir_bytes(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p).map_or(0, |rd| rd.flatten().map(|e| dir_bytes(&e.path())).sum())
}

/// RQ3: WebKit がその store を置く dir（`~/Library/WebKit/<bundle id>/WebsiteDataStore/<store の id の UUID>`・bundle id を問わず = dev の build も）。
/// `space` = `project::web::store_of` の space（Project の id か `SHARED_WEB_STORE.0`）。
fn web_store_dirs(space: &str) -> Vec<std::path::PathBuf> {
    let h: String = crate::surface::store_identifier(space, crate::surface::SHARED_WEB_STORE.1).iter().map(|b| format!("{b:02x}")).collect();
    let uuid = format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    std::fs::read_dir(std::path::PathBuf::from(home).join("Library/WebKit"))
        .map_or(Vec::new(), |rd| rd.flatten().map(|e| e.path().join("WebsiteDataStore").join(&uuid)).filter(|p| p.is_dir()).collect())
}

/// RQ3: その Project の DB の行の大きさ（app・版・面の中身・Project・Project の版の JSON）。
fn project_bytes(st: &Store, project_id: &str, app: &str) -> u64 {
    let len = |v: &Value| serde_json::to_string(v).map_or(0, |s| s.len() as u64);
    st.get(Table::Apps, app).ok().flatten().map_or(0, |r| len(&r.json))
        + st.list_revisions(app).unwrap_or_default().iter().map(|r| len(&r.json)).sum::<u64>()
        + st.list_ui_state(&session_of(app)).unwrap_or_default().iter().map(|(_, v)| len(v)).sum::<u64>()
        + st.get(Table::Projects, project_id).ok().flatten().map_or(0, |r| len(&r.json))
        + st.list_project_revisions(project_id).unwrap_or_default().iter().map(|r| len(&r.json) + len(&r.ops)).sum::<u64>()
}

/// RQ3（`Generated::refresh_data` の裏の thread）: Data & Recovery の 1 回の読み。DB は読むだけの接続（UI の thread の mutex を取らない）。
fn read_data(dir: &Path, projects: &[(String, String, String)], look_in: &[std::path::PathBuf], now: i64) -> DataSummary {
    use crate::ui::shell::DataKind;
    let st = Store::open_read_only(&dir.join(DB_FILES[0])).ok();
    let len = |v: &Value| serde_json::to_string(v).map_or(0, |s| s.len() as u64);
    let file = |n: &str| std::fs::metadata(dir.join(n)).map_or(0, |m| m.len());
    let webs = |space: &str| web_store_dirs(space).iter().map(|d| dir_bytes(d)).sum::<u64>();
    let projects: Vec<(String, u64)> = projects.iter().map(|(p, app, name)| (name.clone(), st.as_ref().map_or(0, |s| project_bytes(s, p, app)) + webs(p))).collect();
    let runs = st.as_ref().map_or(0, |s| [Table::Runs, Table::Evidence].into_iter().flat_map(|t| s.list(t, None).unwrap_or_default()).map(|r| len(&r.json)).sum::<u64>());
    let kept = DataKind::ALL
        .map(|k| {
            let b = match k {
                DataKind::Projects => projects.iter().map(|(_, b)| b).sum::<u64>() + dir_bytes(&dir.join("takes")),
                DataKind::History => runs + HISTORY_FILES.iter().map(|n| file(n)).sum::<u64>(),
                DataKind::ScreenRecords => SCREEN_FILES.iter().map(|n| file(n)).sum(),
                DataKind::Consents => file(crate::surface::ACT_CONSENT_FILE),
                DataKind::Connections => dir_bytes(&dir.join(ACCOUNTS_DIR)) + webs(crate::surface::SHARED_WEB_STORE.0),
                DataKind::Backups => dir_bytes(&dir.join(BACKUPS_DIR)) + legacy_damaged(dir).iter().map(|f| dir_bytes(f)).sum::<u64>(),
            };
            (k, b)
        })
        .to_vec();
    let mut imports: Vec<(std::time::SystemTime, std::path::PathBuf)> = look_in
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == PROJECT_FILE_EXT))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    imports.sort_by(|a, b| b.0.cmp(&a.0));
    DataSummary {
        location: dir.to_path_buf(),
        projects,
        kept,
        backups: list_backups(dir, now),
        // 予約の世代が消えた印は出さない（戻せない予約を `Restores when Shibubu reopens` と言わない）
        restoring: std::fs::read_to_string(dir.join(RESTORE_MARKER)).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty() && backup_files(dir, s).is_some()),
        imports: imports.into_iter().map(|(_, p)| p).take(20).collect(),
        checked_at: now,
    }
}

/// RQ3: Backups の list（(世代の id, `2 h ago · Daily · 2.1 MB`)・新しい順）。前の版が隔離した設定も `Damaged settings · set aside` で。
fn list_backups(dir: &Path, now: i64) -> Vec<(String, String)> {
    let when = |at: i64| crate::ui::runs::ago(&crate::agent::rfc3339_from_secs(at), now);
    let mut sets: Vec<(i64, String, String)> = Vec::new();
    for e in std::fs::read_dir(dir.join(BACKUPS_DIR)).into_iter().flatten().flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let Some((at, why)) = backup_id(&id) else { continue };
        let what = match why {
            "manual" => "Backed up by you",
            "daily" => "Daily",
            "before-update" => "Before an update",
            "before-restore" => "Before a restore",
            "damaged" => "Damaged · set aside",
            _ => continue,
        };
        if backup_files(dir, &id).is_some() {
            sets.push((at, id.clone(), format!("{} · {what} · {}", when(at), size_line(dir_bytes(&e.path())))));
        }
    }
    for f in legacy_damaged(dir) {
        let id = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let at = id.trim_start_matches(LEGACY_DAMAGED).parse::<i64>().unwrap_or(0);
        sets.push((at, id, format!("{} · Damaged settings · set aside · {}", when(at), size_line(dir_bytes(&f)))));
    }
    sets.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    sets.into_iter().map(|(_, id, line)| (id, line)).collect()
}

/// RQ3: 記録の 1 行の時刻（unix 秒）。`at`（RFC3339）・`at_ms`・`receipt.sent_ms`（unix ms）の 3 つの形。読めなければ None（`prune_jsonl` が file の書いた時刻で見る）。
fn line_secs(v: &Value) -> Option<i64> {
    if let Some(s) = v["at"].as_str() {
        return crate::agent::secs_from_rfc3339(s);
    }
    let ms = v["at_ms"].as_f64().or_else(|| v["receipt"]["sent_ms"].as_f64())?;
    Some((ms / 1000.0) as i64)
}

/// RQ3: JSONL の `before`（unix 秒）より古い行と、`RECORD_CAP` を超えた古い行（頭から）を消して書き直す。消した行の数。
/// 書き直しは一時の file から rename（途中で落ちても前の file が残る）・消す行が無ければ書かない。
fn prune_jsonl(path: &Path, before: i64) -> Result<usize, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: {}", path.display(), e.kind())),
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // 時刻の欄の無い行（Native Direct の窓の記録・前の版の trace）は file を最後に書いた時刻より前に書いた行 = その時刻が保存期間を過ぎていれば消す
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    let mut keep: Vec<&str> = lines.iter().copied().filter(|l| serde_json::from_str::<Value>(l).ok().as_ref().and_then(line_secs).or(written).is_none_or(|t| t >= before)).collect();
    let mut size: usize = keep.iter().map(|l| l.len() + 1).sum();
    let mut head = 0;
    while size > RECORD_CAP && head < keep.len() {
        size -= keep[head].len() + 1;
        head += 1;
    }
    keep.drain(..head);
    let gone = lines.len() - keep.len();
    if gone == 0 {
        return Ok(0);
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, keep.iter().map(|l| format!("{l}\n")).collect::<String>())
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
    Ok(gone)
}

/// RQ3: 残す世代の数（`BACKUPS_DIR` の doc）を超えた古い世代を消す。`damaged` と、Restore を予約した世代（`RESTORE_MARKER`）は消さない。
fn prune_backups(dir: &Path) -> Result<usize, String> {
    let reserved = std::fs::read_to_string(dir.join(RESTORE_MARKER)).unwrap_or_default().trim().to_owned();
    let mut sets: Vec<(i64, String, std::path::PathBuf)> = std::fs::read_dir(dir.join(BACKUPS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy() != reserved.as_str())
        .filter_map(|e| backup_id(&e.file_name().to_string_lossy()).map(|(at, why)| (at, why.to_owned(), e.path())))
        .collect();
    sets.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut n = 0;
    for (_, why, path) in sets {
        let limit = match why.as_str() {
            "daily" => 7,
            "manual" => 10,
            "before-update" | "before-restore" => 3,
            _ => continue,
        };
        let k = seen.entry(why).or_default();
        *k += 1;
        if *k > limit {
            std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// RQ2・RQ3: `app` の定義と Project を `new_app` の id に写す。id を指す所（app の id・Project の id・Page の app_ref・`<app>.` で始まる local の source と action）
/// だけを替える（JSON の文字列ごと・部分一致はしない）。`new_app == app` はそのまま。Duplicate と Import が使う。
fn moved_ids(app: &str, new_app: &str, def: &AppDefinitionV3, p: &ProjectDefinitionV1) -> Result<(AppDefinitionV3, ProjectDefinitionV1), String> {
    fn walk(v: Value, f: &dyn Fn(&str) -> Option<String>) -> Value {
        match v {
            Value::String(s) => Value::String(f(&s).unwrap_or(s)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| walk(x, f)).collect()),
            Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, walk(x, f))).collect()),
            v => v,
        }
    }
    let (old_p, new_p) = (project_id_of(app), project_id_of(new_app));
    let f = |s: &str| {
        if s == app {
            Some(new_app.to_owned())
        } else if s == old_p {
            Some(new_p.clone())
        } else {
            s.strip_prefix(&format!("{app}.")).map(|rest| format!("{new_app}.{rest}"))
        }
    };
    let def = serde_json::from_value(walk(serde_json::to_value(def).map_err(|e| e.to_string())?, &f)).map_err(|e| e.to_string())?;
    let p = serde_json::from_value(walk(serde_json::to_value(p).map_err(|e| e.to_string())?, &f)).map_err(|e| e.to_string())?;
    Ok((def, p))
}

/// L7-AGENT: Terminal の agent の一言（`project::context::ask` が runs 表に置いた承認待ちの行）の見張り。
/// 新しい型の理由: 別 process が書いた事（`PRAGMA data_version`）と、card に出した行・承認して読ませている行は Engine の run（`engine_waiting`）とも model の job とも別の持ち物。
#[derive(Default)]
struct Asks {
    /// 最後に見た `PRAGMA data_version`（別の接続が commit すると変わる・自分の書き込みでは変わらない）
    seen: Option<String>,
    /// 読み直す（別 process が書いた・card を閉じた・席が空いていなくて読めなかった Project が在る）
    dirty: bool,
    /// Project → (card に出している run の id, 押しを読み始める時刻 = 出した時 + `ASK_GAP`)
    shown: BTreeMap<String, (String, std::time::Instant)>,
    /// 承認して読ませている（Project, run の id）。在る間、その Project の読みは Project の編集だけ（Engine・`run the tests` に渡さない・`asking`）。
    /// model に読ませた時は `poll_model_job` の Edited で結果を run に書く
    reading: Option<(String, String)>,
    /// Runs の `Stop` で、読んでいる agent の一言を止めた（答えが届いても当てない）
    stop: bool,
    /// この時刻まで読み直さない（承認した 1 click で次の頼みの card を同じ所に出さない・席の空かない Project を毎 frame 読まない）
    next: Option<std::time::Instant>,
}

/// L7-AGENT: 決めた後・待った後に runs 表を読み直すまでの間・card を出してから押しを読むまでの間（2 度押しで、読んでいない次の card を決めさせない）
const ASK_GAP: std::time::Duration = std::time::Duration::from_secs(1);
/// L7-AGENT: 承認の card で通すのは Project の編集だけ（`Effect::LocalWrite`）。Engine の操作・test を走らせる一言は通さない
const ASK_SCOPE_LINE: &str = "Only changes to this project can be approved here. Nothing was run";

/// 裏の thread で走っている Engine の run（`start_engine` → `poll_engine`）。
struct EngineJob {
    project_id: String,
    goal: String,
    run_id: String,
    /// この run で承認した手（Denied が承認で通らない拒否か＝同じ digest を渡したかを見る・同意が無くて止めた時に「X を AI に任せる」で同じ手を送り直す）
    approved: Vec<crate::ui::engine_view::ApprovalStep>,
    rx: std::sync::mpsc::Receiver<EngineAnswer>,
    /// `EngineRequest::progress` が書く最後の 1 行（`poll_engine` が Page に写す・W19-3b G2）
    waiting: Arc<std::sync::Mutex<Option<String>>>,
    /// ENGINE-ADJUST-1102 ①: この run の人の Stop の印（`EngineRequest::stop` と同じ Arc・`stop_engine` が立てる・runtime の `running` の表にも在り `poll_engine` が外す）
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// ENGINE-AUDIT W1: 始めた時刻（Running の card の 1 行に経過秒を足す = 固まったのか進んでいるのかが分かる）
    started: std::time::Instant,
}

/// W13: 裏の thread で model が読んでいる一言か編集。UI の thread は答えを待たない（反証の直し: 待つ間 最長 2 分 画面と AX が止まっていた）。
struct ModelJob {
    what: JobWhat,
    rx: std::sync::mpsc::Receiver<JobRead>,
    /// FX2: この job だけの止める印（`read_goal` → `LlmModel` → `Llm::complete_json_cancellable`）。編集の job は立てない
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// FX2: 始めた時刻（`model_progress` の経過）と、止め始めた時刻（Some = 止めている・`Build::Stopping` の経過）
    started: std::time::Instant,
    stopping: Option<std::time::Instant>,
    /// G104: 深く考える段（`Effort::wait_factor` > 1）で始めた = 読んでいる 1 行に `Thinking longer`
    longer: bool,
}

enum JobWhat {
    /// preview = `Preview plan` の読み（L2・作らない = 答えは `previewed` に置き、`Build now` が同じ案を保存する）
    Create { goal: String, ws: Option<std::path::PathBuf>, preview: bool },
    /// hash = 読ませた時の Project（読んでいる間に Project が変わったら `apply_project` が当てずに 1 行）
    /// OPEN-INPLACE-1101 の 5: preview = Project の chat の案の Preview（当てない・`preview_engine_plan`）
    Edit { project_id: String, text: String, hash: String, preview: bool },
}

/// 裏の thread の答え（model の呼び出しだけ。DB と画面には触れない）。
/// FX2: `Plan` の 2 つ目 = 最後の model の呼び出しの `LlmError`（止めた = `Cancelled`・失敗の種類 = `failure_step`）。答えが届いた・呼んでいない時は None
enum JobRead {
    Plan(GoalRead, Option<crate::llm::LlmError>),
    Edit(Result<Vec<ProjectPatchOp>, composer::patch::EditError>),
    /// OPEN-INPLACE-1101 の 5: 案の Preview の答え = 変更の読みと、読めない文（Engine へ行く文）なら Engine の plan の行（`PageEngine::plan`）
    Planned(Result<Vec<ProjectPatchOp>, composer::patch::EditError>, Option<Result<Vec<String>, String>>),
}

/// OPEN-INPLACE-1101 の 5: Project の chat の案の Preview で読んだ答え（`Generated::planned_edit`）。新しい型の理由: 読みの答えと置いた行と版を
/// 1 つで持ち、Build now と Preview の押し直しが同じ条件（`is`）で使う（tuple の 5 つ組を 2 か所で崩さない）。
struct PlannedEdit {
    project_id: String,
    text: String,
    /// 読ませた時の Project の hash（版が替わったら使わない）
    hash: String,
    read: Result<Vec<ProjectPatchOp>, composer::patch::EditError>,
    /// 案の card に置いた行（Preview の押し直しはこれを置き直す）
    lines: Vec<String>,
}

impl PlannedEdit {
    fn is(&self, project_id: &str, text: &str, hash: &str) -> bool {
        self.project_id == project_id && self.text == text.trim() && self.hash == hash
    }
}

/// 一言を model に読ませた答えと、app の一覧を引けなかった理由の 1 行（`read_goal`）。
type GoalRead = (Result<Option<composer::project::ProjectPlan>, ComposeError>, Option<String>);

/// W13: `poll_model_job` が返す、届いた答えを当てた結果（app.rs が画面を移す）。
pub enum ModelDone {
    /// `create_project` と同じ（Ok(None) は 1 Page の app で足りる一言 → 呼ぶ側が `create`）。goal は送った一言（Home の欄に戻す時に使う）
    Created { goal: String, made: Result<Option<View>, String> },
    /// `page_intent(PageIntent::Edit)` と同じ（Err の 1 行は Page の上にも出る）
    Edited { project: String, made: Result<View, String> },
    /// FX2（FX-005）: `stop_model_job` で止めた生成が、子の process を回収し終えた（`LlmError::Cancelled` が届いた）。何も作っていない・名前の表でも作らない。
    /// goal は送った一言（app.rs が `back_to_home_field` で Home の欄へ戻す）。届いた後の `model_progress` は `Ended`（`Stopped. Nothing was added to Projects.`）。
    /// 止めた後に答えが届いても（CLI が書き終えた直後の Stop）作らず Stopped（利用者が止めた = 足さない）。
    /// Wave: FX2
    Stopped { goal: String },
    /// FX2（FX-006）: Home の生成が AI の失敗で終わった（何も作っていない・名前の表で黙って作らない）。goal は送った一言（app.rs が Home の欄へ戻す）。
    /// 理由の 1 行と次の一手 1 つは `model_progress` の `Ended`（`Created` の Err と分ける = note に同じ 1 行を 2 度出さない）。
    /// Wave: FX2
    Failed { goal: String },
    /// L2（review A・DOGFOOD-X 周 2 回 1 #2）: `start_preview` の答え。何も書いていない。`lines` = 案の card の読むだけの行（作る Page と app・
    /// 読めなかった理由）。空 = 止めた（card は `Preview plan` に戻る）。読めた案は `previewed` に残り、同じ文の `create_project` が model を呼ばずに保存する。
    /// TALK-1101: `sections` = card の中に開く見出しつきの節（`plan_sections`・`Turn::sections` へ）。読めなかった・止めた・名前の表の案は空。
    Previewed { goal: String, lines: Vec<String>, sections: Vec<(String, Vec<String>)> },
    /// OPEN-INPLACE-1101 の 5: Project の chat の案の Preview の答えが届いた（`preview_engine_plan`）。行は Generated が案の card に置いた（`preview_change`）・
    /// 何も書いていない。app.rs は何もしない（どの Project かは要らない = 欄を持たない）
    Planned,
}

/// FX2: 止めた生成の 1 行（表 copy.en.md の Home「(new) cancelled」）。Runs の生成の行の Stop の note の終わった文にも使う（app.rs `poll_model`）。
pub(crate) const STOPPED_LINE: &str = "Stopped. Nothing was added to Projects.";
/// FX2: 止めない呼び出しに渡す印（編集の job・同期の `create_project`）。
static NEVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 裏の thread が答えずに終わった時（panic）の理由の 1 行。
const MODEL_LOST: &str = "The AI call stopped partway";

/// W13: `llm::Llm`（利用者の CLI）を composer の `ModelProvider` に包む。schema の `title` を `JsonRequest::name` に、composer の文を input（CLI の stdin）に。
/// FX2: `cancel` = job の止める印（止めない呼び出しは `NEVER`）・`failed` = 最後の呼び出しの `LlmError`（composer には文で渡すので種類はここに残す）。
struct LlmModel<'a> {
    llm: &'a dyn crate::llm::Llm,
    cancel: &'a std::sync::atomic::AtomicBool,
    failed: std::cell::RefCell<Option<crate::llm::LlmError>>,
}

impl<'a> LlmModel<'a> {
    fn new(llm: &'a dyn crate::llm::Llm, cancel: &'a std::sync::atomic::AtomicBool) -> Self {
        LlmModel { llm, cancel, failed: std::cell::RefCell::new(None) }
    }
}

/// W13: model の system prompt（中身の指示は composer の文の側。ここは形だけ）。
const LLM_INSTRUCTIONS: &str = "あなたは Shibubu（利用者の Mac の作業場を組む app）の中の planner。渡された JSON Schema の形の JSON だけを返す。tool は使わない。";

/// G104: 依頼に添付と深さを乗せる包み（`start_create` = 添付と effort・`start_edit` = effort だけ）。provider が受けない物は乗せない。
/// G84: model の別名も（両方の口・provider が受ける別名だけ）。
/// USAGE-OWN-1101: 頼んだ Project の id も（呼び出しの記録の Project ごとの内訳・model には送らない）。Engine の道もこれで包む（添付・深さ・別名は無し）。
struct Asked {
    llm: Arc<dyn crate::llm::Llm>,
    files: Vec<std::path::PathBuf>,
    effort: Option<crate::domain::chat::Effort>,
    model: Option<String>,
    project: Option<String>,
}

impl Asked {
    fn of(llm: Arc<dyn crate::llm::Llm>, attach: &[crate::domain::chat::Attachment], effort: Option<crate::domain::chat::Effort>, model: Option<String>, project: Option<&str>) -> Asked {
        use crate::domain::chat::Attachment;
        let files = attach
            .iter()
            .filter_map(|a| match a {
                Attachment::File { path, .. } => Some(path.clone()),
                Attachment::Pasted { text, .. } => pasted_file(text).ok(),
            })
            .collect();
        Asked { llm, files, effort, model, project: project.map(str::to_owned) }
    }
    /// 依頼に添付と effort を乗せた写し（provider が受けない物は乗せない）
    fn dressed(&self, req: &crate::llm::JsonRequest) -> crate::llm::JsonRequest {
        let mut req = req.clone();
        if self.llm.takes_files() {
            req.files.clone_from(&self.files);
        }
        // PROVIDER-1101: provider が受ける段だけ（受けない段は付けない = AI の既定）
        if self.effort.is_some_and(|e| self.llm.efforts().contains(&e.flag())) {
            req.effort = self.effort.map(|e| e.flag().to_owned());
        }
        // G84: provider が受ける model の語だけ（受けない・一覧に無い語は付けない = AI の既定）
        if self.model.as_deref().is_some_and(|m| self.llm.models().iter().any(|(id, _)| id == m)) {
            req.model.clone_from(&self.model);
        }
        if self.project.is_some() {
            req.project.clone_from(&self.project);
        }
        req
    }
    /// 読む待ちが伸びる段か（provider が effort を受ける時だけ）
    fn longer(&self) -> bool {
        self.effort.is_some_and(|e| e.wait_factor() > 1 && self.llm.efforts().contains(&e.flag()))
    }
}

/// HOME-ATTACH-1101: 貼った文（`Attachment::Pasted`）を file の添付と同じ道で渡す file（`$TMPDIR/shibubu-pasted/<中身の hash>/Pasted text.txt`）。
/// llm_cli がこれを呼び出しの dir の `files/` に写し、model は `files/Pasted text.txt` を Read で読む（2 つ目は `2-Pasted text.txt`）。
// ponytail: 書けない $TMPDIR では黙ってその 1 つを落とす（送る前の recheck は書かない）・古い file は OS の $TMPDIR の掃除に任せる
fn pasted_file(text: &str) -> std::io::Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    let dir = std::env::temp_dir().join("shibubu-pasted").join(format!("{:016x}", h.finish()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("Pasted text.txt");
    std::fs::write(&path, text)?;
    Ok(path)
}

impl crate::llm::Llm for Asked {
    fn label(&self) -> String {
        self.llm.label()
    }
    fn send(&self, req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
        self.llm.send(&self.dressed(req))
    }
    // FX2 の Stop が届くように、止める印も包んだ provider へ渡す（既定の send_cancellable は印を見ない）
    fn send_cancellable(&self, req: &crate::llm::JsonRequest, cancel: &std::sync::atomic::AtomicBool) -> Result<Value, crate::llm::LlmError> {
        self.llm.send_cancellable(&self.dressed(req), cancel)
    }
    fn takes_files(&self) -> bool {
        self.llm.takes_files()
    }
    fn provider_name(&self) -> String {
        self.llm.provider_name()
    }
    fn efforts(&self) -> Vec<&'static str> {
        self.llm.efforts()
    }
    fn models(&self) -> Vec<(String, String)> {
        self.llm.models()
    }
}

impl composer::ModelProvider for LlmModel<'_> {
    fn complete_json(&self, prompt: &str, schema: &Value) -> Result<Value, String> {
        let name = schema["title"].as_str().unwrap_or("shibubu_request");
        let req = crate::llm::JsonRequest::new(name, LLM_INSTRUCTIONS, prompt, schema.clone());
        self.llm.complete_json_cancellable(req, self.cancel).map_err(|e| {
            let line = e.to_string();
            *self.failed.borrow_mut() = Some(e);
            line
        })
    }
}

/// W5: Page に出す Terminal の表示の状態。画面そのものは manager の VT（W10）に在り、Page を移っても失わない。
#[derive(Default)]
struct TermViews {
    /// 直前の誤り（起こせない・送れない・大きさを変えられない）の 1 行。出ている間は毎 frame 起こし直さない（「新しく起こす」で消える）。
    lines: BTreeMap<SessionKey, String>,
    /// G92: 次に描く Page で見える所まで送る部品（Terminal の id か Browser の面の id）。`Host::take_reveal` が読んで消し、`draw_page` の終わりにも消す（1 frame だけ）。
    reveal: Option<String>,
    /// G95: panel の tab ごとの agent の見張り（`panel_status` が毎 frame 進める・保存しない）
    watches: BTreeMap<SessionKey, AgentWatch>,
    /// USAGE-OWN-1101: この frame で panel の tab の前面に居た agent（pid・名前・tab の cwd・Project の id）。app.rs が `take_agents` で受け、Shibubu の中の使用量の数え手へ
    fronts: Vec<(i32, &'static str, String, String)>,
    /// G95: 出した通知（tab の名前・変わった先）。この起動の間だけ・試験が数える
    notified: Vec<(String, AgentState)>,
    /// TERM-RESTORE-1101: 起こせなかった terminal の 1 行（`TermHost::pane` が 1 回だけ積む・`draw_terminal_panel` が右上の toast に出して空にする）
    failed: Vec<String>,
    /// L7-ORCA #12: panel の帯の `+` の menu に出す入っている agent（`set_agents`・app.rs が AI の確かめの後に 1 回入れる。入る前は空 = menu は New terminal と New browser tab だけ）
    // ponytail: 起動の間は見直さない（後から入れた agent は Shibubu を開き直すと出る）。要れば menu を開いた時に見直す
    agents: Vec<&'static str>,
    /// TERM-XTERM-1101: この frame に renderer が置いた Terminal の枠（`place`・(session, 全体, 見えている所, focus, 打鍵の席)）。`Generated::term_sync` が取って空にする
    placed: Vec<(SessionKey, crate::surface::Frame, crate::surface::Frame, bool, InputView)>,
    /// TERM-XTERM-1101: 頁で右 click された所と選び（`term_sync` が積み、次の frame の `place` が `TermPlaced::menu` で渡して消す）
    menus: BTreeMap<SessionKey, (egui::Vec2, String)>,
    /// TERM-XTERM-1101: 開いた xterm の頁（session ごと）
    #[cfg(all(feature = "webview", target_os = "macos"))]
    xterm: BTreeMap<SessionKey, XtermSlot>,
    /// TERM-XTERM-1101: 頁を開けなかった session（この起動の間は egui の grid のまま・理由は `failed` で toast に 1 回）
    no_xterm: std::collections::BTreeSet<SessionKey>,
    /// TERM-XTERM-1101: 打ってから画面に出るまでの 1 分ごとの中央値（stderr = app.log）
    latency: crate::surface::TermLatency,
    /// TERM-XTERM-1101: 頁に focus が在る間に押した Shibubu の short cut（`TermMsg::Shortcut`）。app.rs の `raw_input_hook` が次の frame の入力に足す（`take_term_keys`）
    keys: Vec<egui::Event>,
}

/// TERM-XTERM-1101: 開いた xterm の頁 1 つ（`TermViews::xterm`）。
#[cfg(all(feature = "webview", target_os = "macos"))]
struct XtermSlot {
    view: crate::surface::webview::TermView,
    /// 頁に書いた出力の続き（`read_since` の cursor・`Ready` で seed を書いた時の end から）
    cursor: u64,
    /// 最後に置いた席（頁の打鍵を書く `write_as(Human, ..)` の view）
    seat: InputView,
    /// 今の頁の見た目（替わった frame だけ `set_theme`）
    theme: crate::surface::TermTheme,
}

impl TermViews {
    /// TERM-XTERM-1101: renderer の `terminal_place` の本体（`TermHost`・`Host` が呼ぶ）。描き手の無い build・試験・頁を開けなかった session は None（egui の grid）。
    /// それ以外は枠をこの frame に積み（`Generated::term_sync` が取る）、前の frame の頁で右 click されていれば `TermPlaced::menu` で 1 回渡す。
    fn place(&mut self, key: SessionKey, rect: egui::Rect, shown: egui::Rect, focus: bool, seat: InputView) -> Option<renderer::TermPlaced> {
        if cfg!(test) || !cfg!(all(feature = "webview", target_os = "macos")) || self.no_xterm.contains(&key) || shown.width() < 1.0 || shown.height() < 1.0 {
            return None;
        }
        let frame = |r: egui::Rect| crate::surface::Frame { x: f64::from(r.min.x), y: f64::from(r.min.y), w: f64::from(r.width()), h: f64::from(r.height()) };
        let menu = self.menus.remove(&key);
        self.placed.retain(|p| p.0 != key);
        self.placed.push((key, frame(rect), frame(shown), focus, seat));
        Some(renderer::TermPlaced { menu })
    }
}

/// G95: panel の保存先（`ui_state` の session と key）。Project の定義に入れない（開閉・並べ替えで revision を増やさない）
/// G95: panel の今は Project の app の session（`session_of`・Layout の選択と同じ所）の `ui_state` の key。`ui_state` は sessions の行を要る（FK）
const PANEL_KEY: &str = "terminal_panel";
/// SESSIONS-LEFT-1101 × owner 189: Project の今の chat session の key を残す ui_state の key（値 = session の key の文字列・1 つ目の session は行を消す）
const CHAT_SESSION_KEY: &str = "chat_session";
/// PROJECT-DIR-1101: Project の自分の folder の置き場 = その Project の app の session の ui_state（`session_of(app)`・key `FOLDER_KEY`・値 `{"dir": 絶対 path}`）。
/// panel（`PANEL_KEY`）と同じ所 = sessions の行（app の FK）を先に作れば ui_state の FK を満たす（前の `ui_state("project-folder", id)` は sessions に行が無く必ず落ちた）。
/// 定義（`ProjectDefinitionV1`）に入れない = 利用者の Undo の 1 手にならない。書き出し・複製・取り込みには写さない（この Mac の path・複製は自分の folder を作る）。
/// Project を消すと cascade で行も消える（folder そのものは消さない）。
const FOLDER_KEY: &str = "project_folder";
/// ACCOUNTS-1101 2 段目（作る物 3「選んだ account は Project ごとに覚える」）: その Project で選んだ AI の account の置き場 = `FOLDER_KEY` と同じ所（Project の app の
/// session の ui_state・sessions の行を先に作る = FK）。値 `{"kind": 種類, "id": id}`・System default は `{"kind": null}`・行が無い = 覚えていない。
/// 定義に入れない（Undo の 1 手にならない）・書き出し・複製・取り込みには写さない（account はこの Mac の物）。
const ACCOUNT_KEY: &str = "ai_account";

/// HOME-TERM-1101（owner 208「homeの中でterminalを開けるようにしなくてはいけない」）: Home の Terminal の置き場（`SessionKey::project_id`・Project ではない）。
/// Project の外（Home・Runs・Connections・Settings）の Terminal と sign in は全部ここの panel（Project の panel と同じ部品・tab・`+ New`・⌃`・高さ・TERM-RESTORE）。
/// cwd = `projects_root`（`~/Shibubu`・無ければ作る・作れなければ HOME）。panel の今は `data_dir` の `HOME_PANEL_FILE`（`ui_state` は sessions の行 = app を要るので使えない）。
/// 例（app.rs）: `g.draw_terminal_panel(ctx, HOME_PLACE, None, None, None)`・`g.panel_act(HOME_PLACE, PanelAct::SignIn { name, command })`・`g.probe_tab(HOME_PLACE, &tab)`。
/// Wave: HOME-TERM-1101
pub const HOME_PLACE: &str = "shibubu.home";
/// HOME-TERM-1101: Home の Terminal の panel（開閉・高さ・tab）を残す file（`data_dir` の中・`TerminalPanel` の JSON）。
const HOME_PANEL_FILE: &str = "home-terminal.json";

/// G95: panel の tab の起こし方（id = session id・shell は `$SHELL`・cwd は tab の cwd）。Project の `terminal_specs` には書かない。
/// L7-ORCA #12: agent の tab は shell に最初にその agent を打つ（`AGENTS` の名前だけ = 保存の行を書き換えられても他の command は打たない）。
/// SIGNIN-1101: sign in の tab は `llm_cli::FOUND_CLIS` の sign in の command だけ（先頭の語は見つけた file の path = `llm_cli::sign_in_command`・`Startup::sign_in_command` の口から）。
// ponytail: 毎 frame の spec で sign in の tab だけ install dir を数個 stat する（tab は 1 つ・起こさない）。重くなれば tab を足す時に 1 回だけ解く
/// L7-ORCA #12・SIGNIN-1101: tab の shell に最初に打つ物。`AGENTS` の名前はそのまま・他は sign in の口（`Startup::sign_in_command`）が表の command と解いた物だけ・
/// 口が無い（試験の crate）か表に無い = None（保存の行を書き換えられても他の command は打たない）
fn tab_command(command: &str, sign_in: Option<fn(&str) -> Option<String>>) -> Option<String> {
    if crate::domain::terminal::AGENTS.contains(&command) {
        Some(command.to_owned())
    } else {
        sign_in.and_then(|f| f(command))
    }
}

fn panel_spec(project_id: &str, tab: &terminal_panel::PanelTab) -> project::TerminalSessionSpec {
    let command = tab.command.as_deref().and_then(|c| tab_command(c, STARTUP.get().map(|s| s.sign_in_command)));
    project::TerminalSessionSpec { id: tab.id.clone(), project_id: project_id.to_owned(), shell: user_shell(), working_directory: tab.cwd.clone(), keep_alive_on_page_switch: true, command }
}

/// 段 2 作り直し（owner 142・図 §31）× NOTIFY-1101（owner 185）: 「Install CLI & skill」。Terminal の panel（`settings = false`）と Settings › Terminal（`true`）が同じ物を描く。
/// 入口は右上の toast 1 枚（`draw_terminal_panel` が入っていない Project で最初に terminal を開いた時に `SETUP_INTRO` と button `Install`・押すと app.rs が `read` して `open`）。
/// - panel・閉じている: 何も描かない（常設の 1 行は消した・入った知らせは入れた時の toast 1 回）
/// - Settings: 入っていれば `line`（` · Remove in Settings › Terminal` を落とす）・入っていなければ `SETUP_NOT_YET`・Shibubu の物が 1 つでも在れば横に `Remove`
/// - panel・開いている（= 人の手が今要る）: 行ごとに checkbox（AX の名前 = agent の名前・link は `shibubu command`）・置き場（HOME は `~`）・`setup_state_text`、
///   `SETUP_PATH_NOTE` の 1 行、`Install`（`AgentSetup::install`・入ったら閉じて `line` を toast・失敗は行に残して開いたまま）と `Cancel`
/// 押した手はその場で当てる（HOME の小さな file 数個・待たない）。保証する: 押すまで何も書かない・`Installed` の行の checkbox は押せない（入れる物が無い）。
pub fn setup_line(ui: &mut egui::Ui, setup: &mut AgentSetup, settings: bool) {
    let t = theme::tokens(ui.ctx());
    let helper = |ui: &mut egui::Ui, text: &str| {
        ui.add(egui::Label::new(theme::Type::Helper.text(text).color(t.text_secondary)).wrap().selectable(true));
    };
    if settings {
        // Settings の中では「Remove in Settings › Terminal」を言わない（横に Remove が在る）
        let line = setup.line().map_or_else(|| SETUP_NOT_YET.to_owned(), |l| l.replace(" · Remove in Settings › Terminal", ""));
        ui.horizontal_wrapped(|ui| {
            helper(ui, &line);
            if setup.rows.iter().any(|r| r.state == crate::terminal::SetupState::Installed) && theme::button_compact(ui, "Remove", Variant::Ghost).clicked() {
                setup.remove();
            }
        });
        return;
    }
    if !setup.open {
        return;
    }
    // ponytail: 開いている間は panel の中で Terminal の欄が縮む（term_rows の下限まで・PTY も resize）。閉じれば戻る。気になれば一覧を panel の上の帯に
    let home = setup.home.clone();
    for row in setup.rows.iter_mut() {
        ui.horizontal(|ui| {
            let name = row.agent.unwrap_or("shibubu command");
            // PROVIDER-ICONS-1101: 名前の左に印（agent = その logo・link = Terminal の印）
            let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(theme::ICON_UI), egui::Sense::hover());
            match row.agent {
                Some(a) => theme::provider_icon(ui, mark.center(), a, a, t.text_secondary),
                None => theme::kind_icon(ui, mark.center(), None, None, Some(crate::domain::icon::Icon::SquareTerminal), name, t.text_secondary),
            };
            ui.add_enabled(row.state != crate::terminal::SetupState::Installed, egui::Checkbox::new(&mut row.on, theme::Type::Helper.text(name)));
            let shown = row.path.strip_prefix(&home).map_or_else(|_| row.path.display().to_string(), |p| format!("~/{}", p.display()));
            helper(ui, &format!("{shown} · {}", crate::terminal::setup_state_text(row)));
        });
    }
    helper(ui, SETUP_PATH_NOTE);
    ui.horizontal(|ui| {
        if theme::button_compact(ui, "Install", Variant::Ghost).clicked() {
            setup.install();
            // 入ったら閉じて済んだ知らせを toast 1 回（失敗は行に残して開いたまま）
            let done = setup.line();
            setup.open = done.is_none();
            if let Some(line) = done {
                crate::ui::toast::post(ui.ctx(), line, false, None);
            }
        }
        if theme::button_compact(ui, "Cancel", Variant::Ghost).clicked() {
            setup.open = false;
        }
    });
}

/// 段 2 作り直し × NOTIFY-1101: 入っていない Project で最初に terminal を開いた時の toast の文（button `Install`）。
pub const SETUP_INTRO: &str = "Shibubu: agents in this terminal can read and change this project.";

/// 段 2 作り直し × NOTIFY-1101: Settings › Terminal の、まだ入っていない時の 1 行（button は toast の 1 つだけ）。
pub const SETUP_NOT_YET: &str = "Not installed · open a terminal in a project (+ › New terminal or ⌃⇧`) and press Install on the notice at the top right";

/// 段 2 作り直し: 一覧の link の行の下の 1 行（`~/.local/bin` が利用者の shell の PATH に在るかは Shibubu から確かめられない）。
pub const SETUP_PATH_NOTE: &str = "If \"shibubu\" isn't found outside Shibubu, add this line to your shell startup file: export PATH=\"$HOME/.local/bin:$PATH\"";

/// G95: macOS の通知 1 回（`osascript` の `display notification`・依存を足さない）。試験の build では出さない（数は `TermViews::notified`）。
/// 保証しない: 押すとその tab を開く事（押すと開くのは Script Editor・UNUserNotificationCenter の objc が要る）。
fn notify(tab: &str, state: &AgentState) {
    if cfg!(test) {
        return;
    }
    let quoted = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!("display notification {} with title \"Shibubu\"", quoted(&format!("{tab} · {}", state.label())));
    // 待つ thread で回収する（zombie を残さない）
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).status();
    });
}

/// W10: Page の Terminal に写す scrollback の行の数（VT は `TerminalConfig::scrollback_lines` = 10 000 行を持つ）。
// ponytail: 毎 frame 写すので 1 000 行で止める。全部を出すなら、VT が変わった時だけ写す cache を manager に足す
const HISTORY_SHOWN: usize = 1_000;

/// FX6（FX-033）: `Run tests` を打つ専用の session（id = `tests.{project}`）。利用者が Page に置いた Terminal の shell には打たない
/// （前は今の Page の Terminal を先に使い、既定の 1 Page では Dev の shell に打っていた）。作業 folder は Project の repo。
/// 保証する: Terminal の席に依らず走る・Project の `terminal_specs` に書かない（Page の席にならない）。repo の folder が無ければ None（`$HOME` で test を走らせない）。
fn tests_spec(p: &ProjectDefinitionV1) -> Option<project::TerminalSessionSpec> {
    Some(project::TerminalSessionSpec { id: format!("tests.{}", p.id), project_id: p.id.clone(), shell: user_shell(), working_directory: existing_repo_dir(p)?, keep_alive_on_page_switch: true, command: None })
}
const NO_REPO_LINE: &str = "No repo folder in this project, so there is nowhere to run tests";
/// W11: Tests Page の「走らせる」が印を待つ長さ（compile を含む本物の repo の `cargo test`。20 秒では時間切れで receipt が残らなかった）。
const RUN_TESTS_TIMEOUT: Duration = Duration::from_secs(600);
/// Terminal の出力で起こす frame の遅れ（`Generated::wake_on_terminal`）。出力が流れている間の frame の上限を決める（約 30 frame/秒）。
const TERMINAL_FRAME: std::time::Duration = std::time::Duration::from_millis(33);
/// TERM-XTERM-1101: xterm の頁が 1 つでも開いている（`term_sync` が毎 frame 書く・`wake_on_terminal` の口が読む = 出力で待たずに起こす）
static XTERM_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `Generated::ensure_terminal` の中身（Host が毎 frame 呼ぶので `&mut self` を取らない）。
/// この起動の manager に在ればそのまま・どちらも無ければ spec のまま起こして記録する。
/// TERM-RESTORE-1101（owner 189）: 前の起動の記録だけ（Shibubu を閉じた・Mac を再起動した・落ちた）なら、同じ session id の新しい shell を起こし、
/// 保存した画面（`terminal::read_snapshot`）をその上に描く。folder は保存した folder（無い・消えた folder なら spec の folder・それも無ければ HOME）。
/// agent は起こし直さない（spec の `command` を打たない = 利用者の枠とお金を勝手に使わない）。Restart を押させない。
fn ensure_spec(mgr: &TerminalManager, store: &Mutex<Store>, spec: &project::TerminalSessionSpec) -> Result<SessionInfo, String> {
    let key = SessionKey::of(spec);
    if let Some(info) = mgr.list(Some(&key.project_id)).into_iter().find(|i| i.key == key) {
        return Ok(info);
    }
    let st = store.lock().unwrap_or_else(|e| e.into_inner());
    let row = st.get(Table::TerminalSessions, &spec.id).map_err(|e| format!("Couldn't read the terminal history ({e}). Start a new terminal"))?;
    if let Some(row) = row {
        // 前の起動の PTY は残っていない（Exited で閉じた物も同じ扱い: この起動では読めない）
        let info: SessionInfo = serde_json::from_value(row.json).map_err(|e| format!("Couldn't read the terminal history ({e}). Start a new terminal"))?;
        let saved = mgr.restore_dir().and_then(|d| crate::terminal::read_snapshot(&d, &key));
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let cwd = saved.as_ref().and_then(|s| s.cwd.clone()).into_iter().chain([spec.working_directory.clone(), info.working_directory, home]).find(|d| Path::new(d).is_dir()).unwrap_or_else(|| "/".into());
        let again = project::TerminalSessionSpec { working_directory: cwd, command: None, ..spec.clone() };
        return spawn_spec(mgr, &st, &again, saved.as_ref()).map_err(|e| format!("Couldn't reopen the terminal ({e})"));
    }
    spawn_spec(mgr, &st, spec, None)
}

/// spec の id と cwd で起こし（同じ session id）、記録を書く。動いていれば `AlreadyRunning` の 1 行。
/// G90 (3): shell は今の `$SHELL`（`user_shell`・New terminal と同じ）。Project の spec の shell は作った時の写し（composer の `/bin/zsh`）で、
/// login shell を替えた人の最初の起動と Restart が古い shell で起きていた。
/// TERM-RESTORE-1101: 保存する session として起こす（`spawn_restorable`）。`saved` が在れば保存した大きさで起こしてその画面を先に描く（最初の frame で今の大きさに直る）。
fn spawn_spec(mgr: &TerminalManager, st: &Store, spec: &project::TerminalSessionSpec, saved: Option<&crate::terminal::Snapshot>) -> Result<SessionInfo, String> {
    let spec = &project::TerminalSessionSpec { shell: user_shell(), ..spec.clone() };
    // 24×80 で起こし、Page が描けた大きさを最初の frame の `terminal_resize` で渡す（W10）
    let info = mgr.spawn_restorable(spec, saved.map_or_else(TermSize::default, |s| s.size), saved).map_err(|e| e.to_string())?;
    put_session(st, &info).map_err(|e| format!("Couldn't save the terminal's history ({e}) · It will be lost when you close it"))?;
    Ok(info)
}

/// 「新しく起こす」: 同じ spec で起こし直し、表示を空から読み直す（新しい session の cursor は 0 から）。
fn restart_spec(mgr: &TerminalManager, store: &Mutex<Store>, tv: &mut TermViews, spec: &project::TerminalSessionSpec) -> Result<SessionInfo, String> {
    let key = SessionKey::of(spec);
    let st = store.lock().unwrap_or_else(|e| e.into_inner());
    let r = spawn_spec(mgr, &st, spec, None);
    match &r {
        Ok(_) => {
            tv.lines.remove(&key);
        }
        Err(e) => {
            tv.lines.insert(key, e.clone());
        }
    }
    r
}

/// W11: command を 1 回走らせ（`run_marked` は Agent の lease を通す）、印が返れば RunReceipt を保存して返す（`record_run` と `start_run` の thread の中身）。
/// Err は Tests Page に出す 1 行（何が起きたか + 次の一手・D-e）で、receipt を作らない。test の失敗（`exit_code != 0`）は Ok の receipt で、Err と混ぜない。
/// だから Err の 1 行には「成功」「失敗」を入れない（`TerminalError::Io` の原文は「失敗しました」を含むので出さない）。
fn run_and_save(mgr: &TerminalManager, store: &Mutex<Store>, key: &SessionKey, repo: &str, command: &str, timeout: Duration) -> Result<RunReceipt, String> {
    use crate::terminal::TerminalError;
    let started_at = crate::runtime::now_rfc3339();
    let out: CommandOutput = mgr.run_marked(key, command, timeout).map_err(|e| match e {
        TerminalError::TimedOut { waited, .. } => format!("“{command}” gave no result after {} s. Make sure it finished in the terminal, then run it again", waited.as_secs()),
        TerminalError::Exited { .. } => format!("The run ended before {command} finished · Run again"),
        TerminalError::LeaseDenied(d) => d.to_string(),
        _ => format!("Couldn't get the result of {command} · Run again"),
    })?;
    let finished_at = crate::runtime::now_rfc3339();
    let unsaved = |why: String| format!("Couldn't save the result of “{command}” ({why}). Run it again");
    let r = mgr.receipt_of(key, storage::new_id(), repo.into(), command.into(), &out, started_at, finished_at).map_err(|e| unsaved(e.to_string()))?;
    r.validate().map_err(|e| e.to_string())?;
    let json = serde_json::to_value(&r).map_err(|e| unsaved(e.to_string()))?;
    store.lock().unwrap_or_else(|e| e.into_inner()).put_run_receipt(&r.run_id, &r.project_id, &r.session_id, &json).map_err(|e| unsaved(e.to_string()))?;
    Ok(r)
}

/// `$SHELL`（絶対 path で在る物）、無ければ /bin/zsh。
fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| s.starts_with('/') && Path::new(s).exists()).unwrap_or_else(|| "/bin/zsh".into())
}

/// Project の `resources` の先頭の kind "repo"（`file://` か絶対 path）で、在る folder（面の file の基点 `repo_dir` と違い、在る事を見る）。
fn existing_repo_dir(p: &ProjectDefinitionV1) -> Option<String> {
    p.resources
        .iter()
        .find(|r| r.kind == "repo")
        .and_then(|r| r.uri.as_deref())
        .map(|u| u.strip_prefix("file://").unwrap_or(u).to_owned())
        .filter(|d| d.starts_with('/') && Path::new(d).is_dir())
}

/// PROJECT-DIR-1101: Project の名前 → folder の名前（`/`・`:`・制御文字は `-` か落とす・前後の空白と先頭の `.` を落とす・80 字まで・空なら `Project`）。
fn safe_dir_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).map(|c| if matches!(c, '/' | ':') { '-' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    let clean: String = clean.chars().take(80).collect();
    match clean.trim_end() {
        "" | "-" => "Project".into(),
        c => c.to_owned(),
    }
}

fn put_session(st: &Store, info: &SessionInfo) -> Result<(), storage::StoreError> {
    let json = serde_json::to_value(info).map_err(|e| storage::StoreError::Usage(e.to_string()))?;
    st.put(Table::TerminalSessions, &info.key.session_id, Some(&info.key.project_id), &json)
}

/// USAGE-1: この Mac の地域の UTC からの秒（`/bin/date +%z` の `+0900` → 32400）。読めなければ 0（UTC の日で数える）
fn utc_offset() -> i64 {
    let z = std::process::Command::new("/bin/date").arg("+%z").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    let (sign, hm) = match z.split_at_checked(1) {
        Some(("+", hm)) => (1, hm),
        Some(("-", hm)) => (-1, hm),
        _ => return 0,
    };
    match (hm.get(0..2).and_then(|h| h.parse::<i64>().ok()), hm.get(2..4).and_then(|m| m.parse::<i64>().ok())) {
        (Some(h), Some(m)) => sign * (h * 3600 + m * 60),
        _ => 0,
    }
}

impl Drop for Generated {
    fn drop(&mut self) {
        self.close_terminals();
    }
}

/// session は app ごとに 1 つ（決まった名前なので再起動しても同じ ui_state を引く）。
/// AI-COMPOSE-1101 / OPEN-INPLACE-1101 の 4: 裏で走っている部品の read 1 本。`kept` = 前に読めた値（読めなかった時はそれで答える = 誤りを出さない）・
/// `answered` = 頁にはもう `kept` で答えた（裏の読み直し: 違う値だけ残して開き直す・読めなければ何も出さない）
struct PartRead {
    project: String,
    node: String,
    id: u64,
    app: String,
    what: String,
    kept: Option<Value>,
    answered: bool,
    rx: std::sync::mpsc::Receiver<Result<Value, ReadMiss>>,
}

/// `start_engine_with` の chat の始めの行。You = 一言をそのまま人の行・Earlier = `Doing what you asked earlier: {一言}`（`part_event`）・
/// Quiet = 始めは何も言わず、終わった時に Shibubu の記録の行を 1 つ（「Let Shibubu open it」= OPEN-INPLACE-1101 の 1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Said {
    You,
    Earlier,
    Quiet,
}

/// OPEN-INPLACE-1101 の 4: 部品の読めた値を残す ui_state の key（Project の app の session ごと・中身は `{read_slot: 値}`）。⌘Z の revision に積まない
const READS_KEY: &str = "canvas_reads";

/// OPEN-INPLACE-1101 の 4: 残した値の中の 1 つの場所（部品・app・何を = 同じ頼みは同じ場所・app は大文字小文字と前後の空白を揃える）
fn read_slot(node: &str, app: &str, what: &str) -> String {
    format!("{node}\u{1f}{}\u{1f}{}", app.trim().to_lowercase(), what.trim())
}

fn session_of(app_id: &str) -> String {
    format!("main.{app_id}")
}

impl Generated {
    /// FX1（FX-001）: kangae.sqlite3 を開けず memory の store で動いている時の理由（Some = 閉じると、この起動で作った Project が消える）。
    /// `open` の Err の枝だけが入れる（`start` で memory の store を渡した試験の殻は None = 保存の失敗に数えない）。読み手は `ShibubuApp::save_now`。
    /// 例: `if let Some(why) = self.generated.store_lost() { return Err(why.to_owned()) }`
    /// 保証する: `open` の後に変わらない（開き直す口は作らない）・`note` を他の 1 行が上書きしても消えない（`note` と別に持つ）。
    /// 保証しない: 開けた DB への書き込みの失敗（それは `route` / `page_intent` の Err）。
    /// Wave: FX1
    pub fn store_lost(&self) -> Option<&str> {
        self.lost.as_deref()
    }

    /// `<dir>/kangae.sqlite3` を開いて local action を登録した runtime を起こし、apps の行を全部開き直す。
    /// DB を開けない時は memory の store で動かし、保存されない事を Home に出す（黙って消さない）。
    /// W9: DB を開けた時は Terminal の config に `terminal::ProjectCli { data_dir: dir の絶対 path, bin_dir: 実行 file の dir }`
    /// を入れる（Page の Terminal の中で `shibubu project pages --json` が打てる）。memory の store の時・path を決められない時は入れない（読む保存が無い）。
    /// 契約（RQ3 から・署名は変えない・本体は RQ3 の実装の片）: 開く順を ① `RESTORE_MARKER` が在れば、今の DB と `sessions.json` を `before-restore` の世代へ移して
    /// 予約の世代を写し、印を消す（写せなければ今の data のまま・印を消して `Couldn't restore that backup · …` の note）② `Store::open_backing_up`（migration の前の写し）
    /// ③ `StoreError::is_damaged` なら DB の 3 つの file を `damaged` の世代へ移して空から開き、note = `Your saved projects were damaged and set aside · Restore them in
    /// Settings › Data & Recovery`（他の Err は今まで通り memory の store）④ 開けた後、前の `daily` から 24 時間たっていれば裏の thread で `daily` を 1 つ、にする
    /// （④ は開く前から DB が在った起動だけ = その日に開く前までの data の写し。初めての起動〈空の dir〉は写す物が無いので作らない・隔離した起動も作らない）。
    /// `data_dir` は ①〜④ のどれでも Some（DB を開けなかった起動からも世代を戻せる）。保証しない: 戻した data が前の版の Shibubu の物でも開ける事（`NewerSchema` は今と同じ拒否）。
    /// Wave: RQ3
    pub fn open(dir: &Path) -> Self {
        let now = unix_now();
        let db = dir.join(DB_FILES[0]);
        // ① 予約した世代を戻す（DB を開く前）
        let restored = restore_on_open(dir, now);
        let existed = db.is_file();
        // ② 前の版の DB は migration の前に写す ③ 壊れた DB は隔離して空から開く（他の誤り = lock・権限・新しい版は今まで通り memory の store）
        let before_update = dir.join(BACKUPS_DIR).join(format!("{}-before-update", stamp(now))).join(DB_FILES[0]);
        let mut opened = Store::open_backing_up(&db, &before_update);
        let mut damaged = false;
        if matches!(&opened, Err(e) if e.is_damaged()) && set_aside_db(dir, now).is_ok() {
            (opened, damaged) = (Store::open(&db), true);
        }
        let (store, note, cli, saved, lost_why) = match opened {
            Ok(s) => {
                let cli = (|| Some(ProjectCli { data_dir: dir.canonicalize().ok()?, bin_dir: std::env::current_exe().ok()?.parent()?.to_path_buf() }))();
                (s, damaged.then(|| DAMAGED_NOTE.to_owned()).or(restored), cli, true, None)
            }
            Err(e) => (Store::open_in_memory().expect("memory の SQLite"), Some(format!("Couldn't save the project ({e}). It will be lost when you close it")), None, false, Some(e.to_string())),
        };
        // ④ 毎日の世代（前の daily から 24 時間・前から在った DB だけ = 初めての起動と試験の空の dir は写さない）。裏の thread で読むだけの接続から
        if saved && existed && !damaged && last_backup(dir, "daily").is_none_or(|at| now - at >= 86_400) {
            let (dir, db) = (dir.to_path_buf(), db.clone());
            let _ = std::thread::Builder::new().name("daily-backup".into()).spawn(move || {
                let set = dir.join(BACKUPS_DIR).join(format!("{}-daily", stamp(now)));
                let wrote = Store::open_read_only(&db).and_then(|s| s.back_up_to(&set.join(DB_FILES[0]))).map_err(|e| e.to_string());
                if wrote.and_then(|()| copy_if_there(&dir.join(SETTINGS_FILE), &set.join(SETTINGS_FILE)).map_err(|e| e.to_string())).is_err() {
                    let _ = std::fs::remove_dir_all(&set);
                }
            });
        }
        let mut g = Self::start(store, ActionRegistry::with_local_actions(), note);
        g.memory_fallback = !saved;
        g.data_dir = Some(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()));
        g.lost = lost_why.map(|e| format!("projects made since Shibubu opened aren't saved ({e})"));
        // USAGE-1: DB を開けた時だけ開いた日を残す（memory の store の時は消える Project なので数えない）
        if saved {
            let log = UsageLog::new(dir.join(USAGE_FILE));
            log.append(unix_now(), ("launch", Value::Bool(true))); // RQ1: 起動の 1 行（First action の起点）
            g.usage_log = Some(log);
            g.takes.dir = dir.join("takes");
        }
        // W16-1: 重ねた記録は data dir の overlay.jsonl（試験の側が oracle と照合する）
        #[cfg(feature = "native_direct")]
        {
            g.surfaces = crate::project::native::NativeSurfaces::new(Some(crate::project::native::record_path(dir)));
        }
        g.web = crate::project::web::PageWebViews::new(dir.join(crate::project::web::STAGING_DIR));
        // RQ2: 前の起動で消した Project の website data（WebView を作る前）。DB に在る Project（消すのに失敗した・同じ id で作り直した）の store は消さない
        // DB を開けなかった起動（memory の store）は在るか分からないので読まない（queue は次の起動まで残す）
        if saved {
            g.web.remove_forgotten_stores(&|project| g.project_saved(project));
        }
        match cli {
            Some(cli) => g.with_terminal_config(TerminalConfig { project_cli: Some(cli), ..TerminalConfig::default() }),
            None => g,
        }
    }

    pub fn start(store: Store, registry: ActionRegistry, mut note: Option<String>) -> Self {
        let rows = store.list(Table::Apps, None);
        let store = Arc::new(Mutex::new(store));
        let rt = Runtime::start(store.clone(), registry).expect("runtime を起こせない（前回の run の後始末に失敗）");
        let mut apps = Vec::new();
        let mut broken = 0;
        for row in rows.unwrap_or_default() {
            let opened = serde_json::from_value::<AppDefinitionV3>(row.json)
                .ok()
                .and_then(|def| AppRevision::new(def).ok())
                .and_then(|rev| rt.open(rev, &session_of(&row.id)).ok());
            match opened {
                Some(app) => apps.insert(0, Opened::new(app)),
                None => broken += 1,
            }
        }
        if broken > 0 {
            note = Some(format!("{broken} saved projects couldn't be opened because their definitions no longer pass the check"));
        }
        // 前の起動の PTY は残っていない: 動いていた記録は Unavailable に書き戻す（偽の「動いている」を出さない・D-e）
        {
            let st = store.lock().unwrap_or_else(|e| e.into_inner());
            for row in st.list(Table::TerminalSessions, None).unwrap_or_default() {
                if let Ok(info) = serde_json::from_value::<SessionInfo>(row.json) {
                    if info.status == SessionStatus::Running {
                        let _ = put_session(&st, &info.after_restart());
                    }
                }
            }
        }
        // download の一時の dir は `open` が data dir の下に置き直す（test の `start` は OS の一時の dir）
        // SESSIONS-LEFT-1101 × owner 189: 前の起動で Project ごとに開いていた chat の session を戻す（その Project の key だけ・読めなければ 1 つ目）
        let session: BTreeMap<String, String> = {
            let st = store.lock().unwrap_or_else(|e| e.into_inner());
            apps.iter()
                .filter_map(|o: &Opened| {
                    let app = o.rt.revision().definition().id.clone();
                    let key = st.get_ui_state(&session_of(&app), CHAT_SESSION_KEY).ok().flatten()?.as_str()?.to_owned();
                    let project = project_id_of(&app);
                    (crate::domain::chat::session_project(&key) == Some(project.as_str()) && key != project).then_some((project, key))
                })
                .collect()
        };
        let web = crate::project::web::PageWebViews::new(std::env::temp_dir().join(crate::project::web::STAGING_DIR));
        Generated {
            rt,
            store,
            apps,
            note,
            lost: None,
            routers: BTreeMap::new(),
            automation: Default::default(),
            page_line: None,
            opened_line: None,
            pending_delete: None,
            pending_rename: None,
            tinted: None,
            focus_tab: None,
            edit_applied: None,
            plan_talk: None,
            previewed: None,
            edit_refused: false,
            terminals: Arc::new(TerminalManager::new(TerminalConfig::default())),
            runs: Arc::default(),
            term_views: TermViews::default(),
            reader: None,
            resource_lines: BTreeMap::new(),
            web,
            web_saved: None,
            #[cfg(feature = "native_direct")]
            surfaces: crate::project::native::NativeSurfaces::new(None),
            #[cfg(feature = "mirror")]
            mirror: None,
            llm: None,
            budget_pref: (None, None),
            usage: None,
            usage_job: None,
            usage_at: 0,
            cli_usage: BTreeMap::new(),
            cli_usage_job: None,
            cli_usage_at: 0,
            llm_missing: None,
            llm_fallback: None,
            unusable_answers: 0,
            model_job: None,
            model_ended: None,
            model_runs: Vec::new(),
            effort: None,
            model: None,
            usable: Arc::default(),
            usable_started: Default::default(),
            engine: None,
            engine_job: None,
            planned_edit: None,
            build_queued: None,
            reads: Vec::new(),
            kept_answers: Vec::new(),
            next_read: BTreeMap::new(),
            part_stops: BTreeMap::new(),
            part_errors: BTreeMap::new(),
            reread_after: None,
            rereads: Vec::new(),
            last_runs: BTreeMap::new(),
            engine_views: BTreeMap::new(),
            notices: Vec::new(),
            engine_waiting: BTreeMap::new(),
            asks: Asks::default(),
            engine_history: Vec::new(),
            chats: BTreeMap::new(),
            chat_rows: None,
            fresh: BTreeMap::new(),
            session,
            usage_log: None,
            memory_fallback: false,
            takes: Takes { dir: std::env::temp_dir().join("shibubu-takes"), by_ref: BTreeMap::new() },
            panels: BTreeMap::new(),
            homebrew: acquire::Homebrew::find(),
            installs: BTreeMap::new(),
            data_dir: None,
            projects_root: None,
            data: None,
            data_job: None,
        }
    }

    /// Terminal の設定を替える（試験が rc を読まない `-f` で起こす時）。session を起こす前に呼ぶ。
    /// TERM-RESTORE-1101: 保存の置き場が在る config（`project_cli` = DB を開けた起動）なら、出力があった terminal を `RESTORE_EVERY` ごとに書く裏の thread を 1 本
    /// （落ちた・Mac の再起動で `shutdown` が来ない時の備え）。manager を持たない（Weak）= 替えた・閉じた manager の thread は次に起きた時に終わる。
    pub fn with_terminal_config(mut self, config: TerminalConfig) -> Self {
        self.terminals = Arc::new(TerminalManager::new(config));
        if self.terminals.restore_dir().is_some() {
            let weak = Arc::downgrade(&self.terminals);
            let _ = std::thread::Builder::new().name("terminal-snapshots".into()).spawn(move || loop {
                std::thread::sleep(crate::terminal::RESTORE_EVERY);
                let Some(mgr) = weak.upgrade() else { return };
                mgr.save_snapshots(true);
            });
        }
        self
    }

    pub fn terminals(&self) -> &TerminalManager {
        &self.terminals
    }

    /// W11: Agent が command を 1 回走らせ、RunReceipt を保存する。lease が拒めば走らせない。
    ///
    /// 使い方: `let r = g.record_run(&key, &cwd, "false", Duration::from_secs(20))?; assert!(!r.succeeded());`
    /// 保証する: `write_as(Agent)` が Ok の後にだけ `run_marked`（`run_marked` も Agent の lease を通す）。失敗の run は `status_line` が「失敗」。
    /// 製品の口は `PageIntent::RunTests`（Tests Page の「走らせる」）。試験専用の直呼びだけでは受け入れにしない。
    pub fn record_run(&self, key: &SessionKey, repo: &str, command: &str, timeout: Duration) -> Result<RunReceipt, String> {
        self.terminals.write_as(key, InputOwner::Agent, &key.page_view(), b"").map_err(|e| e.to_string())?;
        run_and_save(&self.terminals, &self.store, key, repo, command, timeout)
    }

    /// W11: `record_run` を UI の thread を待たせずに始める。本体で `PageIntent::RunTests` をこちらに替える（`record_run` は試験の同期の口として残す）。
    /// 理由: 8405a44 の実機で「走らせる」が UI の thread で 13 秒止まり（timeout 20 秒）、その間 AX が 2 要素に崩れた（20 通目: 背景の AX の不応答は製品の不具合）。
    ///
    /// 使い方: `g.start_run(&key, &cwd, "cargo test", Duration::from_secs(600))?;` → 次の frame から `g.run_state(project)` を描く。
    /// 保証する: lease の判定（`may_write_after(Agent)`）は呼んだ thread で済ませ、拒否と、同じ Project で走っている間の 2 本目は Err の 1 行で byte 0。
    /// Ok なら印を待つのは別の thread で、返るまでに command の終わりを待たない。印が返れば `record_run` と同じ RunReceipt を保存して `run_state` は None、
    /// 返らなければ receipt を作らず `RunState::Unfinished`。RunTests の timeout は 10 分（20 秒では compile を含む本物の repo の cargo test が時間切れで receipt が残らない）。
    /// 保証しない: 走っている run を止める事（Ctrl-C は人の口）。再起動をまたいだ run。
    /// Wave: W11
    pub fn start_run(&self, key: &SessionKey, repo: &str, command: &str, timeout: Duration) -> Result<(), String> {
        let project = key.project_id.clone();
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(RunState::Running { command, .. }) = runs.get(&project) {
            return Err(format!("“{command}” is running. Press again when it finishes"));
        }
        self.terminals.write_as(key, InputOwner::Agent, &key.page_view(), b"").map_err(|e| e.to_string())?;
        runs.insert(project.clone(), RunState::Running { command: command.into(), started_at: crate::runtime::now_rfc3339() });
        drop(runs);
        let (mgr, store, states, key, repo, command) = (self.terminals.clone(), self.store.clone(), self.runs.clone(), key.clone(), repo.to_owned(), command.to_owned());
        let spawned = std::thread::Builder::new().name(format!("run-{project}")).spawn(move || {
            let done = run_and_save(&mgr, &store, &key, &repo, &command, timeout);
            let mut states = states.lock().unwrap_or_else(|e| e.into_inner());
            match done {
                Ok(_) => states.remove(&key.project_id),
                Err(line) => states.insert(key.project_id.clone(), RunState::Unfinished { command, line }),
            };
        });
        if let Err(e) = spawned {
            self.runs.lock().unwrap_or_else(|e| e.into_inner()).remove(&project);
            return Err(format!("Couldn't get ready to run ({e}). Press again"));
        }
        Ok(())
    }

    /// W11: Project の「走らせる」の今。走っていない・最後の run が receipt で終わった時は None。
    ///
    /// 使い方: draw_page の Tests Page が毎 frame 呼び、Running なら「走らせています」、Unfinished なら line を出す（receipt の一覧はその下）。
    /// 保証する: 何も書かない・待たない（UI の thread が呼ぶ）。
    /// Wave: W11
    pub fn run_state(&self, project_id: &str) -> Option<RunState> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).get(project_id).cloned()
    }

    /// W13: 生成と自然言語の編集に使う model（利用者の LLM の CLI）を渡す。起動の時に 1 回（app.rs の 1 か所・`llm_cli::CliLlm::detect` の結果をそのまま）。
    /// 呼ばなければ model 無し（今まで通り・試験の既定＝quota を使わない）。
    ///
    /// 例: `Generated::open(&dir).with_llm(CliLlm::detect(cfg).map(|c| Box::new(c) as Box<dyn Llm>))`
    /// 保証する（W13 の実装で）: Ok の時、`create_project` は `project::installed_apps` → `composer::project::plan_project` で一言を model に読ませ
    /// （名前の表を引かない）、`Ok(None)` は今の `create` へ、`Err(ComposeError::Model)` は今の名前の表（`compose_project` → `create`）で続けて理由を
    /// `llm_line` に置く。`PageIntent::Edit` は `edit_project_by_model`、`Err(EditError::Model)` は `edit_project` で読み直す。どの失敗も既存の Project を書き換えない。
    /// Err の時は model 無しで動き、`llm_line` は `LlmError::NoCli` の 1 行（llm_cli の理由の 1 行そのまま・例 `AI is off for this launch`）。
    /// この 2 つ（`create_project`・`page_intent`）は model の答えを待つ。app.rs は待たない `start_create` / `start_edit` → `poll_model_job`（同じ答え方）を使う。
    /// 保証しない: model の答えが一言に合う事（I3 は試験の側が DB を読んで決める）。
    pub fn with_llm(mut self, llm: Result<Box<dyn crate::llm::Llm>, crate::llm::LlmError>) -> Self {
        self.set_llm(llm);
        self
    }

    /// L7-ORCA #12: panel の帯の `+` の menu に出す入っている agent（`llm_cli::agent_clis` の結果・file を見るだけで起こさない）。
    /// app.rs が AI の確かめと同じ所・同じ login の PATH で 1 回渡す（generated は llm_cli を名指せない = 結合試験の crate に llm_cli が無い）。
    pub fn set_agents(&mut self, agents: Vec<&'static str>) {
        self.term_views.agents = agents;
    }

    /// W13: `with_llm` と同じ事を、開いた後に（起動を止めないよう app.rs が裏の thread で detect し、届いた時に 1 回）。
    /// W14（YCM-067）: 製品が使う model はどれもここで上限を被せる（`llm::BUDGET_ENV`・既定 10 回 600 秒・process の間の合計）。
    /// 上限で止めた生成と編集は他の model の失敗と同じく名前の表で続け、`llm_line` が `LlmError::OverBudget` の 1 行を出す。
    pub fn set_llm(&mut self, llm: Result<Box<dyn crate::llm::Llm>, crate::llm::LlmError>) {
        (self.llm, self.llm_missing) = match llm {
            Ok(l) => (Some(Arc::new(crate::llm::Budgeted::new(l, crate::llm::Budget::resolve(self.budget_pref.0, self.budget_pref.1)))), None),
            Err(e) => (None, Some(e.to_string())),
        };
        self.llm_fallback = None;
        // RQ2: account を替えたら前の account の使用量を出さない（次の `refresh_usage` で読み直す）
        (self.usage, self.usage_job, self.usage_at) = (None, None, 0);
    }

    /// G99: model（利用者の CLI）を使えるか（`set_llm(Ok)` の後）。app.rs が「窓が前に来た時に探し直す」と button を出すかを決める。
    pub fn llm_ready(&self) -> bool {
        self.llm.is_some()
    }

    /// DOGFOOD-X 周 7 #1 #2: AI が上限で止まっていれば、その 1 行（`LlmError::OverBudget`・次の一手 = Settings › AI & Models › Execution limit）。
    /// model が無い・上限の前は None。`llm_ready` は変えない（止まっていても CLI は在る = G99 の探し直しと Install / Sign in の button を出さない）。
    /// 例（app.rs の Home の会話・`Preview plan`・Build now）: `if let Some(why) = g.llm_stopped() { stopped_talk(&why, &text) }`。保証する: 呼ばない・1 回を使わない。
    /// Wave: L2
    pub fn llm_stopped(&self) -> Option<String> {
        self.llm.as_ref().and_then(|b| b.stopped()).map(|e| e.to_string())
    }

    /// TALK-1101 追記 2: 人の 1 つの頼みが来た = AI の上限を数え直す（`Budgeted::new_request`）。model が無い時は何もしない。
    /// 呼ぶ所は人の手だけ（Home の一言 `talk`・`Preview plan`・`create_app`〈Build now・⌘K の Create〉・Project の一言 `PageIntent::Edit`・ENGINE-AUDIT W9 本体で
    /// 部品の押し `CanvasAct` / `part_event`・「Let Shibubu open it」`open_part`・承認 `Approve` / `AlwaysAllow` = どれも人が押した）・どれも `llm_stopped` を見る前。
    /// 人の手の無い繰り返し（`start_read`・`start_edit`・`start_engine` の中・Terminal の agent の `shibubu project ask`）は数え直さない（上限で止める = W14 の目的）。
    /// 例: `if g.model_busy_line().is_none() { g.new_request(); }`
    pub fn new_request(&self) {
        if let Some(b) = &self.llm {
            b.new_request();
        }
    }

    /// L2: Home の会話が、今の model を 1 回呼ぶための写し。無い時は手元の問いのまま。
    pub fn fork_llm(&self) -> Option<Arc<dyn crate::llm::Llm>> {
        self.llm.as_ref().map(|b| Arc::clone(b) as Arc<dyn crate::llm::Llm>)
    }

    /// ENGINE-GENERIC-1101 追記 234: chat の thread に渡す Engine（`with_windows` の窓の 1 行を Engine と同じ道で読む）。Engine が無ければ None。
    pub fn fork_engine(&self) -> Option<Arc<dyn PageEngine>> {
        self.engine.as_ref().map(|e| e.engine.clone())
    }

    /// W13: model の状態の 1 行（Home の欄の下に出す・D-e）。model が使えている時と、まだ `with_llm` を呼んでいない時は None。
    /// 例: `if let Some(line) = g.llm_line() { states::inline_wrap(ui, RegionState::Partial, line) }`
    /// 保証する: 直前の生成で model の答えを使えず名前の表で続けた時は、その理由（`ComposeError::Model` の 1 行）を次の生成か編集まで出す。
    /// 契約（UI-6a = owner の問題 3「`◐ Partly shown · …` が Home と全 Page」・sendback 27 / 50）: 呼ぶのは Home（app.rs）だけ。
    /// `draw_page` は呼ばない（門 `a6_no_page_partial`）。編集で model を使えなかった理由（`finish_edit` の `EditError::Model`）は `llm_fallback` でなく
    /// 右上の toast（`notices`・× まで残る・NOTIFY-1101 追記）に出す。CLI が無い（`llm_missing`）と
    /// 生成の理由は Home の欄の下に 1 回。保証しない: Connections › AI の行（C3 / C4・後）。
    /// Wave: UI-6a
    pub fn llm_line(&self) -> Option<&str> {
        self.llm_fallback.as_deref().or(self.llm_missing.as_deref())
    }

    /// RQ1（RQ-003・Settings › AI & Models の `Execution limit`）: AI の呼び出しの上限を替える。`calls` / `secs` = `UiSettings::llm_max_calls` / `llm_max_secs`（None の欄 = env か既定）。
    /// 今の model（`set_llm` で包んだ `Budgeted`）にも次の呼び出しから当て（`Budgeted::set_budget`）、後から届く model は `set_llm` が `Budget::resolve(calls, secs)` で包む
    /// （起動の detect は裏の thread なので、Settings の値を当てるのが model より先でも後でも同じ上限になる）。
    /// 呼ぶ側の例（app.rs の Settings の当て直し・起動の 1 回と `Intent::SettingsChanged`）: `self.generated.set_budget(self.store.ui.llm_max_calls, self.store.ui.llm_max_secs);`
    /// 保証する: 使った回数と時間は 0 に戻さない（1 つの頼みの中は数え続ける・`new_request` が 0 に戻す）・model が無い時は値を覚えるだけ。保証しない: 送っている途中の 1 回を止める事。
    /// Wave: RQ1
    pub fn set_budget(&mut self, calls: Option<u32>, secs: Option<u64>) {
        self.budget_pref = (calls, secs);
        if let Some(b) = &self.llm {
            b.set_budget(crate::llm::Budget::resolve(calls, secs));
        }
    }

    /// RQ1（RQ-003）: 今の頼み（TALK-1101 追記 2: 最後の一言 / Build now から）で AI を呼んだ回数と時間の合計（`Budgeted::used`）。model が無い（まだ確かめていない・CLI が無い）時は None。
    /// 呼ぶ側の例（Settings › AI & Models）: `settings::draw(.., self.generated.budget_used(), ..)` → `3 of 10 calls · 42 of 600 seconds used by your last request`。
    /// 止まった理由は上限と並べて画面が決める（回数が上限 = `Stopped at 10 calls`・時間が上限 = `Stopped at 600 seconds`・どちらも次の一手は同じ画面の上限を上げる・DOGFOOD-X 周 7 #3: 保存した 0 は開き直しても 0 なので「開き直す」は言わない）。
    /// 保証する: 何も書かない・model を呼ばない。
    /// Wave: RQ1
    pub fn budget_used(&self) -> Option<(u32, std::time::Duration)> {
        self.llm.as_ref().map(|b| b.used())
    }

    /// RQ2（RQ-USAGE）: plan の使用量を裏の thread で読み直す（`Llm::plan_usage`・model を呼ばない）。`force` = false は前に読んでから 60 秒未満なら何もしない
    /// （下の帯と Settings が毎 frame 呼んでよい）・true は `Refresh` の button。読んだ結果は次の frame から `plan_usage()`。
    /// STATUSBAR-1101 追記 2: `others` = 帯に並べる AI の account（`Shell::providers`）。そのうち `llm_cli::USAGE_CLIS` の AI も同じ呼び出しで読む
    /// （`llm_cli::cli_usage`・5 分ごと・`force` は待たない・Claude の plan とは別の 1 本・model が無くても読む）。結果は `cli_usages()`。
    /// 呼ぶ側の例（app.rs の frame の頭）: `if self.generated.refresh_usage(now, false, &accounts) { ctx.request_repaint(); }`。
    /// 保証する: UI の thread で network を待たない・同時に 1 本・model が無い時は読まず false。戻り値 = この呼び出しで読みを始めたか。保証しない: 読める事（`usage_line` の 3 つの場面）。
    /// Wave: RQ2
    pub fn refresh_usage(&mut self, now: i64, force: bool, others: &[crate::model::AiAccount]) -> bool {
        use std::sync::mpsc::TryRecvError;
        // STATUSBAR-1101 追記 2: Grok・Kimi Code の使用量（Claude の plan とは別の 1 本・model が無くても・5 分ごと = Orca の 15 分より短く Claude の 60 秒より長く・↻ は待たない）
        if let Some(rx) = &self.cli_usage_job {
            match rx.try_recv() {
                Ok(got) => (self.cli_usage, self.cli_usage_job) = (got.into_iter().collect(), None),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.cli_usage_job = None,
            }
        }
        // 読む口が無い（main が `set_startup` を呼んでいない = 試験の crate）間は読まない
        let reader = STARTUP.get().map(|s| s.cli_usage);
        let read: Vec<(String, String, Option<std::path::PathBuf>)> = reader
            .map(|r| {
                others
                    .iter()
                    .filter(|a| r.clis.contains(&a.provider()))
                    .map(|a| (a.id().to_owned(), a.provider().to_owned(), self.data_dir.as_deref().and_then(|d| (r.dir)(d, a))))
                    .collect()
            })
            .unwrap_or_default();
        let mut cli = false;
        if let (Some(r), true) = (reader, self.cli_usage_job.is_none() && !read.is_empty() && (force || self.cli_usage_at == 0 || now - self.cli_usage_at >= 300)) {
            let cli_usage = r.read;
            let (tx, rx) = std::sync::mpsc::channel();
            let wake = self.terminals.wake();
            let spawned = std::thread::Builder::new().name("cli-usage".into()).spawn(move || {
                let got: Vec<_> = read.into_iter().map(|(id, provider, dir)| (id, cli_usage(&provider, dir.as_deref(), std::time::Duration::from_secs(10)))).collect();
                let _ = tx.send(got);
                if let Some(wake) = wake {
                    wake();
                }
            });
            if spawned.is_ok() {
                (self.cli_usage_job, self.cli_usage_at, cli) = (Some(rx), now, true);
            }
        }
        if let Some(rx) = &self.usage_job {
            match rx.try_recv() {
                Ok(u) => (self.usage, self.usage_job) = (Some(u), None),
                Err(TryRecvError::Empty) => return cli,
                Err(TryRecvError::Disconnected) => self.usage_job = None,
            }
        }
        let Some(llm) = self.llm.clone() else { return cli };
        // 間隔: 60 秒・429 の後は 5 分（Anthropic の答えの retry-after が長ければそれ）。`Try again` は待たない
        let wait = match &self.usage {
            Some(Err(crate::llm::LlmError::RateLimited { retry_after })) => retry_after.map_or(300, |s| (s as i64).max(300)),
            // Keychain が読めない（時間切れ・鍵が掛かっている）間は 60 秒ごとに `security` を起こし直さない（`Refresh` は待たない）
            Some(Err(crate::llm::LlmError::NoCli { detail })) if detail == crate::llm::KEYCHAIN_UNREADABLE => 300,
            _ => 60,
        };
        if !force && self.usage_at != 0 && now - self.usage_at < wait {
            return cli;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.terminals.wake();
        let spawned = std::thread::Builder::new().name("plan-usage".into()).spawn(move || {
            let _ = tx.send(crate::llm::Llm::plan_usage(&*llm));
            if let Some(wake) = wake {
                wake();
            }
        });
        if spawned.is_err() {
            return cli;
        }
        (self.usage_job, self.usage_at) = (Some(rx), now);
        true
    }

    /// RQ2（RQ-USAGE）: 直前に読んだ plan の使用量（None = まだ読んでいない・読み中）。`llm::usage_line` に渡して 1 行にする。
    /// 例: `crate::settings::draw(.., self.generated.plan_usage(), ..)`。保証する: 何も書かない・model を呼ばない。
    /// Wave: RQ2
    pub fn plan_usage(&self) -> Option<&Result<crate::llm::PlanUsage, crate::llm::LlmError>> {
        self.usage.as_ref()
    }

    /// STATUSBAR-1101 追記 2: 直前に読んだ Claude 以外の AI の使用量（key = `AiAccount::id`・無い key = まだ読んでいない / 読まない AI）。
    /// 例: `bar_ais(&self.shell.providers, &ai, self.generated.plan_usage(), self.generated.cli_usages(), now)`。保証する: 何も書かない。
    /// Wave: STATUSBAR-1101
    pub fn cli_usages(&self) -> &BTreeMap<String, Result<crate::llm::PlanUsage, crate::llm::LlmError>> {
        &self.cli_usage
    }

    /// USAGE-OWN-1101: この frame までに Terminal の panel の前面で見た agent（pid・`agent_name` の語・tab の cwd・Project の id）を渡して空にする。
    /// 呼ぶ側の例（app.rs の frame の終わり）: `self.own.see(&self.data_dir, &self.generated.take_agents(), now_ms)`
    pub fn take_agents(&mut self) -> Vec<(i32, &'static str, String, String)> {
        std::mem::take(&mut self.term_views.fronts)
    }

    /// STATUSBAR-1101: 使用量を裏で読んでいる最中か（下の帯の ↻ が回る間・追記 2: Grok・Kimi Code の読みも）。
    pub fn usage_reading(&self) -> bool {
        self.usage_job.is_some() || self.cli_usage_job.is_some()
    }

    /// STATUSBAR-1101: 下の帯の左端の今の AI の短い名前（`ai_rows` と同じ `Llm::label` から）: `Claude Code`・`Grok`・それ以外は `ollama · llama3` の形。
    /// AI が無い = `No AI`・確かめ中 = `Checking for AI`。保証する: 版・鍵の末尾・path を入れない。
    pub fn ai_name(&self) -> String {
        match (&self.llm, &self.llm_missing) {
            (Some(b), _) => {
                let label = crate::llm::Llm::label(&**b);
                let head = label.split(" · ").next().unwrap_or_default();
                match head.split_once('/').map_or(head, |(kind, _)| kind) {
                    "claude-cli" => "Claude Code".into(),
                    "grok" => "Grok".into(),
                    _ => head.replacen('/', " · ", 1),
                }
            }
            (None, Some(_)) => "No AI".into(),
            (None, None) => "Checking for AI".into(),
        }
    }

    /// RQ1（RQ-003・Settings › AI & Models の「見つかった AI と状態」）: AI の行（既存の Connections の行の型をそのまま使う・新しい型を作らない）。
    /// 今は 1 行（`set_llm` で受けた CLI）: 確かめ中 = `Available`・`Checking for AI on this Mac`／使える = `Connected`・`Claude Code {版} · signed in with your plan`
    /// （`Llm::label` の版）／使えない = `Unavailable`・`llm_line` と同じ理由の 1 行（次の一手を含む）。G99 の在りか探し（resolver）が入ったら、見つかった CLI ごとに 1 行
    /// （G99 の結果を読むだけ・ここで探さない）。呼ぶ側の例: `let ai = self.generated.ai_rows();`
    /// 保証する: 何も書かない・model を呼ばない・detail に path・email・token を入れない・`recheck` は false（AI の再検出の口は G99）。保証しない: 行の数（G99 の後に増える）。
    /// Wave: RQ1
    pub fn ai_rows(&self) -> Vec<crate::ui::connections::Connection> {
        use crate::domain::capability::DisplayTier;
        let (state, detail, checking) = match (&self.llm, &self.llm_missing) {
            (Some(b), _) => {
                let label = crate::llm::Llm::label(&**b);
                // RQ-API: 鍵の account（label の後ろに ` · API key …a1b2`）は使っている間ずっと請求の 1 行を出す。
                // L4: 見つかった Grok（`grok/1.0.70`）と local LLM（`ollama/llama3`）は Claude Code と言わず、plan も請求も言わない
                let detail = match label.strip_prefix("claude-cli/").map(|v| v.split_once(" · ").ok_or(v)) {
                    Some(Ok((v, key))) => format!("Claude Code {v} · {key} · Billed per request by Anthropic"),
                    Some(Err(version)) => format!("Claude Code {version} · signed in with your plan"),
                    // PROVIDER-1101: その AI の名前と版（`kimi/0.42.0` → `Kimi Code 0.42.0`・`ollama/llama3` → `Ollama llama3`）
                    None => format!("{} {} · on this Mac", crate::llm::Llm::provider_name(&**b), label.split_once('/').map_or("", |(_, v)| v)),
                };
                (DisplayTier::Connected, detail, false)
            }
            (None, Some(why)) => (DisplayTier::Unavailable, crate::ui::runs::mask_personal(why), false),
            (None, None) => (DisplayTier::Available, "Checking for AI on this Mac".to_owned(), true),
        };
        vec![crate::ui::connections::Connection { name: "AI".into(), state, detail, recheck: false, checking, settings: None, key: "ai".into(), acts: vec![], checked_at: None }]
    }

    /// W13: 起動の CLI の確かめ（app.rs の裏の thread）が終わる前に model 無しで作る時の 1 行（D-e・反証の直し: 黙って名前の表で作っていた）。
    /// 確かめの答えが届いたら `set_llm` が消す。確かめ終わっている（`set_llm` 済み）なら何もしない。
    pub fn mark_llm_unchecked(&mut self) {
        if self.llm.is_none() && self.llm_missing.is_none() {
            self.llm_fallback = Some("Built with simple rules · the AI check didn't finish".into());
        }
    }

    /// W13: 一言を model に読ませるのを裏の thread で始める（app.rs の Home の 1 か所）。true = 始めた（答えは `poll_model_job` が当てる）。
    /// false = model が無い・thread を起こせない → 呼ぶ側は今の `create_project` / `create`（model 無しは手元で速い）。
    /// 呼ぶ前に `model_busy_line` が None である事（同時に 1 つ）を呼ぶ側が見る。
    /// G104: `attach` = 一言に添えた物（Home の長方形）。model に files として（貼った文も file に書いて）、`effort` と一緒に全部の呼び出しに乗る（`chat::recheck` は呼ぶ側・app.rs）。
    /// L2（review A）: 変更の文（`said_is_edit`）の振り分けは呼ぶ側（app.rs の `create_app`: Home の素の一言だけ・案の Build now は振り分けない）。
    /// TALK-1101（Build now）: 案の文なら、その会話（`accept_plan` が先に置く `plan_talk`）で答えずに残った問い（`chat::open_question`）を model に渡し、
    /// model が決めて作る（選んだ答えは `ProjectPlan::preview` の picked → 作った Project の会話の頭の `I picked` の 1 行）。
    pub fn start_create(&mut self, goal: &str, workspace: Option<&Path>, attach: &[crate::domain::chat::Attachment]) -> bool {
        let unanswered = self.plan_talk.as_ref().filter(|(g, _)| g == goal).and_then(|(_, talk)| crate::domain::chat::open_question(talk)).map(str::to_owned);
        self.start_read(goal, workspace, attach, false, unanswered)
    }

    /// L2（review A・DOGFOOD-X 周 2 回 1 #2）: 案の `Preview plan` で、作る時と同じ読み（`read_goal`・1 回）を裏で始める。何も書かない。
    /// 答えは `poll_model_job` の `ModelDone::Previewed`（案の card の読むだけの行 = `plan_lines`）・読めた案は `previewed` に残り、`Build now` の
    /// `create_project` が model を呼ばずにその案を保存する（Preview で見た Page と app がそのまま出来る・model の呼び出しは Preview の 1 回だけ）。
    /// 例（app.rs の `plan_step`）: `if g.llm_ready() && g.start_preview(&sentence, ws, &shell.attach) { turn.preview = vec![PLAN_READING.into()] }`
    /// false = model が無い・thread を起こせない（呼ぶ側は名前の表の `plan_lines`）。呼ぶ前に `model_busy_line` が None である事を呼ぶ側が見る。
    /// TALK-1101: `unanswered` = 会話で答えずに残った問い（`chat::open_question`）。model が答えを選び、節の `Not decided yet` と Build now の `I picked` に出す
    /// （読めた案を Build now がそのまま保存するので、選びも Preview の時に決める）。
    pub fn start_preview(&mut self, goal: &str, workspace: Option<&Path>, attach: &[crate::domain::chat::Attachment], unanswered: Option<&str>) -> bool {
        self.start_read(goal, workspace, attach, true, unanswered.map(str::to_owned))
    }

    /// L2: 案を読ませた答え（`start_preview`）が `goal` の物として在るか。Some(true) = Project の案・Some(false) = 1 Page の app で足りる一言・None = 読んでいない。
    pub fn previewed(&self, goal: &str) -> Option<bool> {
        self.previewed.as_ref().filter(|(g, _)| g == goal).map(|(_, plan)| plan.is_some())
    }

    fn start_read(&mut self, goal: &str, workspace: Option<&Path>, attach: &[crate::domain::chat::Attachment], preview: bool, unanswered: Option<String>) -> bool {
        let Some(llm) = self.llm.clone() else { return false };
        let asked_label = crate::llm::Llm::label(llm.as_ref());
        let asked = Asked::of(llm, attach, self.effort, self.model.clone(), None);
        let longer = asked.longer();
        let llm: Arc<dyn crate::llm::Llm> = Arc::new(asked);
        // 一言に書いた repo の場所を env より先に使う（`create_project` と同じ）
        let ws = crate::project::workspace_in(goal).or_else(|| workspace.map(Path::to_path_buf));
        let (tx, rx) = std::sync::mpsc::channel();
        let (g, w) = (goal.to_owned(), ws.clone());
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = cancel.clone();
        // 答えを送ったら画面を起こす（idle の窓は frame を頼まないので、起こさないと poll_model_job が入力まで呼ばれない・W14 の実機の試行 1）
        let wake = self.terminals.wake();
        let spawned = std::thread::Builder::new().name("llm-plan".into()).spawn(move || {
            let (read, failed) = read_goal(&*llm, &g, w.as_deref(), &c, unanswered);
            let _ = tx.send(JobRead::Plan(read, failed));
            if let Some(wake) = wake {
                wake();
            }
        });
        if spawned.is_err() {
            return false;
        }
        // FX2: 前の終わった 1 行を消し、Runs にこの生成の行を足す（この起動の間だけ）
        self.model_ended = None;
        let now = crate::runtime::now_rfc3339();
        self.model_runs.push(RunEntry {
            id: format!("run_gen_{}", storage::new_id()),
            what: if preview { "Preview your plan" } else { "Create from your request" }.into(),
            app: "Shibubu".into(),
            at: now.clone(),
            status: crate::domain::run::RunStatus::Running,
            // FX-040（REVIEW-FX2 d1）: 一言と添えた物は AI の CLI の先へ送る = Mac の外へ出る手。相手 = 使っている AI の名前・取り消せない（承認の card は足さない・押したのが同意）
            effect: crate::domain::action::Effect::ExternalSend,
            target: ai_name(&asked_label),
            undo: Undo::conservative(crate::domain::action::Effect::ExternalSend),
            attempts: vec![crate::domain::run::Attempt { seq: 1, started_at: now, finished_at: None, outcome: None }],
            // Connections に AI の行が無い（G99 の後で true に）= AI の失敗の行に Connect を出さない
            connect: false,
            project: None,
            stopping: false,
            started_by: "You asked on Home".into(),
        });
        self.model_job = Some(ModelJob { what: JobWhat::Create { goal: goal.to_owned(), ws, preview }, rx, cancel, started: std::time::Instant::now(), stopping: None, longer });
        true
    }

    /// FX2: 今の生成の Runs の行を終わった状態にする（status・最後の試行の終わりと結果・時刻）。
    /// 反証の直し: 止めた（Cancelled）・AI の失敗（Failed と `Outcome::Failed`）は何も書く前に終わった = `Undo::NotNeeded`（詳細に取り消しの行を出さない）。
    /// 保存を試みて失敗した行（Failed・outcome なし）は `Undo::Unknown` のまま。
    fn finish_model_run(&mut self, status: crate::domain::run::RunStatus, outcome: Option<Outcome>) {
        let Some(e) = self.model_runs.last_mut() else { return };
        let now = crate::runtime::now_rfc3339();
        if status == crate::domain::run::RunStatus::Cancelled || matches!(outcome, Some(Outcome::Failed { .. })) {
            e.undo = Undo::NotNeeded;
        }
        e.status = status;
        e.at = now.clone();
        if let Some(a) = e.attempts.last_mut() {
            a.finished_at = Some(now);
            a.outcome = outcome;
        }
    }

    /// W13: 自然言語の編集を model に読ませるのを裏の thread で始める（app.rs の Page の欄）。true = 始めた。false = model が無い・Project が無い・
    /// thread を起こせない → 呼ぶ側は今の `page_intent(PageIntent::Edit)`（その 1 行も同じ道で出る）。
    /// 契約（UI-2 から・署名は変えない）: `composer::project::EditVerb::read(text)` が model の語に無い形（`MoveLeft`・`RenamePage`・`RunTests`）なら
    /// model を呼ばず thread も起こさず false（呼ぶ側の `page_intent(PageIntent::Edit)` が名前の表で読む・`run the tests` は RunTests へ）。
    /// 例（app.rs の Page の欄）: `if g.model_busy_line().is_some() || g.start_edit(&project, text) { return; } g.page_intent(&project, PageIntent::Edit { text })`
    /// 保証する: false の時は何も書かず `model_job` を作らない。保証しない: AddPage・MoveRight の英語の形を model に送らない事（model が在れば今まで通り送る）。
    /// Wave: UI-2
    pub fn start_edit(&mut self, project_id: &str, text: &str) -> bool {
        self.start_change_read(project_id, text, false)
    }

    /// `start_edit` と `preview_engine_plan` の 1 つの道: 変更の文を model に読ませる job（`JobWhat::Edit`）。`preview` = 案の Preview（当てない・
    /// 読めない文は Engine の plan も同じ thread で作る = `JobRead::Planned`）。Preview でない時、同じ Project の版・同じ文の Preview の答え（`planned_edit`）が
    /// 在れば model を呼ばずにそれを流す（OPEN-INPLACE-1101 の 5: Build の AI の呼び出しは Preview の分だけ）。
    fn start_change_read(&mut self, project_id: &str, text: &str, preview: bool) -> bool {
        if !model_reads(text) {
            return false;
        }
        let Some(llm) = self.llm.clone() else { return false };
        let asked = Asked::of(llm, &[], self.effort, self.model.clone(), Some(project_id));
        let longer = asked.longer();
        let llm: Arc<dyn crate::llm::Llm> = Arc::new(asked);
        let Ok((app, p, hash)) = self.project(project_id) else { return false };
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.terminals.wake();
        let kept = if preview { None } else { self.planned_edit.take().filter(|k| k.is(project_id, text, &hash)) };
        if let Some(PlannedEdit { read, .. }) = kept {
            let _ = tx.send(JobRead::Edit(read));
            if let Some(wake) = wake {
                wake();
            }
        } else {
            // Preview: 読めない文（Engine へ行く文）の plan を作る Engine と、Build の run と同じ宛先・状況
            let plan = if preview && self.engine.is_some() {
                let situation = self.situation(project_id);
                let req = self.engine_request(project_id, &app, &p, text, situation);
                self.engine.as_ref().map(|e| (e.engine.clone(), req))
            } else {
                None
            };
            let said = text.to_owned();
            let spawned = std::thread::Builder::new().name("llm-edit".into()).spawn(move || {
                let read = composer::project::edit_project_by_model(&p, &said, &LlmModel::new(&*llm, &NEVER));
                let _ = tx.send(if preview {
                    let steps = match (&read, plan) {
                        (Err(composer::patch::EditError::Unsupported(_)), Some((engine, mut req))) => {
                            req.situation = with_windows(Some(&*engine), std::mem::take(&mut req.situation));
                            Some(engine.plan(req))
                        }
                        _ => None,
                    };
                    JobRead::Planned(read, steps)
                } else {
                    JobRead::Edit(read)
                });
                if let Some(wake) = wake {
                    wake();
                }
            });
            if spawned.is_err() {
                return false;
            }
        }
        self.page_line = None;
        self.pending_delete = None;
        self.pending_rename = None;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.model_job = Some(ModelJob { what: JobWhat::Edit { project_id: project_id.to_owned(), text: text.to_owned(), hash, preview }, rx, cancel, started: std::time::Instant::now(), stopping: None, longer });
        true
    }

    /// G104: 今の provider が受けられる物（送る時の `chat::recheck`・`Shell::accepts` へ写す）。AI が無ければ全部 false。
    pub fn accepts(&self) -> crate::domain::chat::Accepts {
        match &self.llm {
            Some(l) => {
                use crate::llm::Llm;
                crate::domain::chat::Accepts { ai: true, provider: l.provider_name(), photos: l.takes_files(), files: l.takes_files(), efforts: l.efforts().into_iter().filter_map(crate::domain::chat::Effort::from_flag).collect(), models: l.models() }
            }
            None => crate::domain::chat::Accepts::default(),
        }
    }

    /// W13: 裏の model が読んでいる間の 1 行（Home と Page の欄の下・D-e）。None = 待っていない。
    /// 契約（FX2）: `stop_model_job` の後、子が回収されて `poll_model_job` が `ModelDone::Stopped` を返すまでも Some（= 欄の `Create` は押せない形・2 つ目の生成を受けない）。
    pub fn model_busy_line(&self) -> Option<&'static str> {
        // G104: 深く考える段で始めた job は `Thinking longer`（待ちの上限も伸びている・llm_cli の `wait_factor`）
        self.model_job.as_ref().map(|j| match (&j.what, j.longer) {
            (JobWhat::Create { preview: true, .. }, false) => "Reading your plan",
            (JobWhat::Create { preview: true, .. }, true) => "Reading your plan · Thinking longer",
            (JobWhat::Create { .. }, false) => "Reading your request",
            (JobWhat::Create { .. }, true) => "Reading your request · Thinking longer",
            (JobWhat::Edit { preview: true, .. }, false) => "Reading your plan",
            (JobWhat::Edit { preview: true, .. }, true) => "Reading your plan · Thinking longer",
            (JobWhat::Edit { .. }, false) => "Reading your change",
            (JobWhat::Edit { .. }, true) => "Reading your change · Thinking longer",
        })
    }

    /// FX3（FX-013）: AI がこの Project の変更（`start_edit` の job）を読んでいる間か（帯の形を変える手の門・帯の押せない形）。
    fn reading_change_of(&self, project_id: &str) -> bool {
        matches!(&self.model_job, Some(ModelJob { what: JobWhat::Edit { project_id: id, .. }, .. }) if id == project_id)
    }

    /// FX2（FX-005）: Home の生成（`start_create` の job）を止める。job に渡した印（`Arc<AtomicBool>`・`LlmModel` → `Llm::complete_json_cancellable`）を立て、
    /// 止め始めた時刻を覚える。true = 止め始めた（答えは `poll_model_job` の `ModelDone::Stopped`）・false = 止める生成が無い（job が無い・編集の job・既に止めている）。
    /// 例（app.rs `apply` の `Intent::Build(BuildAction::Cancel)`）: `if !self.generated.stop_model_job() { /* CLI を確かめている間の待ちを捨てる */ }`
    /// 保証する: 待たない（UI の thread）・印は この job だけ（Engine が同じ model で読んでいる呼び出しを止めない）・止めた job の答えは名前の表で作り直さず捨てる
    /// （`receiver` を捨てるだけにしない = 子が回収されるまで job を持ち続け、その間 `model_busy_line` は Some）。
    /// 止めた後に答えが届いても（CLI が書き終えた直後の Stop）作らない（`ModelDone::Stopped`）。
    /// 保証しない: 編集（Page の欄）の job を止める事（FX2 の外）。
    /// Wave: FX2
    pub fn stop_model_job(&mut self) -> bool {
        let Some(job) = self.model_job.as_mut() else { return false };
        if !matches!(job.what, JobWhat::Create { .. }) || job.stopping.is_some() {
            return false;
        }
        job.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        job.stopping = Some(std::time::Instant::now());
        // Runs の生成の行も Stopping に（Stop をもう 1 回出さない・REVIEW-FX2 c1）
        if let Some(e) = self.model_runs.last_mut() {
            e.stopping = true;
        }
        true
    }

    /// FX2（FX-004〜006）: Home の欄の下に出す生成の今（`home::draw` → `build_progress::show`）。None = 出す物が無い（job が無く、終わった 1 行も無い）。
    /// - 生成中: `Build::Running { phase: Phase::Read, planned: &[], waiting: None, elapsed }`（`elapsed` は `start_create` からの時間）。段は `Read` だけ:
    ///   job の thread は `installed_apps` と CLI の呼び出しの間ずっと「一言を読む」段で、`save_plan` は UI の thread の `poll_model_job` の中で同じ frame に
    ///   終わる（`Investigate`・`Compose` は見えないので出さない）。待ちの理由は CLI を確かめている間の 1 行だけで、それは app.rs が作る Running
    ///   （今の CLI の呼び出しは login・上限を待たずに失敗を返す = `Ended`）
    /// - 止めている: `Build::Stopping { elapsed }`（`stop_model_job` からの時間）
    /// - 終わった: `Build::Ended { state, line, action }` = 止めた（Ready・`Stopped. Nothing was added to Projects.`・None）か、AI の失敗（Error・
    ///   `LlmError` の 1 行〈Display〉・`failure_step`）。次の `start_create`・`create_project` で消える。作れた時は出さない（Project が開く）
    /// 例: `let b = self.generated.model_progress(); home::draw(ui, note, &rows, &mut self.shell, b.as_ref(), self.generated.llm_line())`
    /// 保証する: 何も書かない・待たない・AI が一部だけ答えた（app の一覧を引けなかった）時に「完成」の語を出さない（その理由は今の `llm_line`）。
    /// 保証しない: 編集の job（Page の欄・今の `model_busy_line` の 1 行のまま）・再起動の後（job も終わった 1 行も保存しない）。
    /// Wave: FX2
    pub fn model_progress(&self) -> Option<crate::ui::build_progress::Build<'_>> {
        use crate::ui::build_progress::{Build, Phase};
        match &self.model_job {
            Some(ModelJob { what: JobWhat::Create { .. }, stopping: Some(t), .. }) => Some(Build::Stopping { elapsed: t.elapsed() }),
            Some(ModelJob { what: JobWhat::Create { .. }, started, .. }) => Some(Build::Running { phase: Phase::Read, planned: &[], waiting: None, elapsed: started.elapsed() }),
            Some(_) => None,
            None => self.model_ended.as_ref().map(|(state, line, action)| Build::Ended { state: *state, line, action: action.as_ref() }),
        }
    }

    /// FX2（FX-007）: 今の（か直前に終わった）Home の生成の run の id（`run_entries` の生成の行の `RunEntry::id` と同じ）。`Open Runs` が `ui::runs::open_run` に渡す。
    /// 例: `if let Some(id) = self.generated.model_run_id() { ui::runs::open_run(ctx, id); self.go(View::Runs) }`
    /// 保証する: `model_progress` が Some の間は Some。保証しない: 再起動の後（この起動の間だけ）。
    /// Wave: FX2
    pub fn model_run_id(&self) -> Option<&str> {
        let live = matches!(&self.model_job, Some(ModelJob { what: JobWhat::Create { .. }, .. })) || (self.model_job.is_none() && self.model_ended.is_some());
        live.then(|| self.model_runs.last().map(|e| e.id.as_str())).flatten()
    }

    /// 欄の変更の文が通った Project の id を 1 回だけ返す。app.rs が毎 frame 見て、その Project の打ちかけを空にする
    /// （今の場所がその Project なら `shell.command`・他の場所に移っていれば `shell.drafts` のその key）。
    /// 例（app.rs の update）: `if let Some(p) = g.take_edit_applied() { if matches!(&shell.view, View::Project { project, .. } if *project == p) { shell.command.clear() } else { shell.drafts.remove(&p); } }`
    /// 立てる所 = 3 か所: 同期の `page_intent(Edit)` が通った（欄の `run the tests` を含む）・`finish_edit` が ops を当てた・
    /// Engine に渡した（裏の `poll_model_job` の `ModelDone::Edited` も同じ `finish_edit` を通る）。
    /// 保証する: 拒否（読めない・既に在る・当てる先が無い・前の依頼が読まれている・書けない）の後は None（直せるよう文は欄に残る）。1 回取ったら次は None。
    /// Home の「作る」に来た変更（`create_project` → `apply_home_edit`）では立てない（文は Home の欄の物・当てた Project の打ちかけは別の文）。
    /// 保証しない: どの文が通ったか（文は欄の側に在る・ここは合図だけ）・Generate の欄（header が送った時に空にし、作れなければ `create_app` が戻す）。
    /// Wave: UI-4a
    pub fn take_edit_applied(&mut self) -> Option<String> {
        self.edit_applied.take()
    }

    /// W13: 裏の model の答えが届いていれば当てる（毎 frame・待たない）。届いていなければ None。
    /// 当て方は同期の `create_project` / `page_intent(Edit)` と同じ（答えを使えなければ名前の表で続けて理由を 1 行・既存の Project を書き換えない）。
    /// `bridge` は Project を保存する時だけ呼ぶ（app が在るかを聞く・起こせなければ None = 面は unavailable と理由）。
    /// 契約（UI-4a から）: `ModelDone::Edited` の拒否の 1 行の状態は同期の `page_intent(Edit)` と同じ（文を直せば通る拒否は Ready・書けなかった時だけ Error・③ C2）。
    pub fn poll_model_job(&mut self, bridge: impl FnOnce() -> Option<Bridge>) -> Option<ModelDone> {
        let read = match self.model_job.as_ref()?.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        let job = self.model_job.take()?;
        Some(match job.what {
            JobWhat::Create { goal, ws, preview } => {
                use crate::domain::run::RunStatus;
                let (read, failed) = match read {
                    Some(JobRead::Plan(r, e)) => (r, e),
                    _ => ((Err(ComposeError::Model(MODEL_LOST.into())), None), None),
                };
                // FX2（FX-005）: 止めた生成は、答えが何であっても作らない（子は回収済み = 答えが届いた）・名前の表でも作らない
                if job.stopping.is_some() {
                    self.finish_model_run(RunStatus::Cancelled, None);
                    self.model_ended = Some((RegionState::Ready, STOPPED_LINE.into(), None));
                    return Some(if preview { ModelDone::Previewed { goal, lines: Vec::new(), sections: Vec::new() } } else { ModelDone::Stopped { goal } });
                }
                // L2: Preview の読みの失敗は card の 1 行（`Build now` はもう一度読んで作る = 今の `start_create`）。Home の欄の下の終わった 1 行には出さない
                if let (true, (Err(ComposeError::Model(why)), _)) = (preview, &read) {
                    let error = failed.as_ref().map_or(ErrorCode::VerificationFailed, run_error);
                    self.finish_model_run(RunStatus::Failed, Some(Outcome::Failed { error }));
                    return Some(ModelDone::Previewed { goal, lines: vec![format!("AI wasn't available · {why}"), "Build now asks the AI again.".into()], sections: Vec::new() });
                }
                // FX2（FX-006）: AI の失敗は黙って名前の表で作らない = 理由の 1 行と次の一手 1 つ（`Use simple rules` で利用者が選ぶ）
                if let (Err(ComposeError::Model(why)), _) = &read {
                    let line = format!("AI wasn't available · {why}");
                    // CU 所見 1: 答えが読めない（Malformed か、答えは届いたが使えない = failed が None）が続く AI は、2 回目から `Use simple rules` を出す（Retry だけでは Project を作れない）
                    let unusable = failed.as_ref().is_none_or(|e| matches!(e, crate::llm::LlmError::Malformed { .. }));
                    self.unusable_answers = if unusable { self.unusable_answers.saturating_add(1) } else { 0 };
                    let action = after_unusable(failed.as_ref().map_or(Some(crate::ui::build_progress::BuildAction::Retry), failure_step), unusable, self.unusable_answers);
                    let error = failed.as_ref().map_or(ErrorCode::VerificationFailed, run_error);
                    self.finish_model_run(RunStatus::Failed, Some(Outcome::Failed { error }));
                    self.llm_fallback = None;
                    self.model_ended = Some((RegionState::Error, line, action));
                    return Some(ModelDone::Failed { goal });
                }
                self.unusable_answers = 0;
                if preview {
                    let (lines, sections) = match self.resolve_plan(&goal, ws.as_deref(), Some(read)) {
                        Ok(plan) => {
                            let read = (plan_lines(&goal, plan.as_ref()), plan_sections(plan.as_ref()));
                            self.previewed = Some((goal.clone(), plan));
                            read
                        }
                        Err(line) => (vec![line], Vec::new()),
                    };
                    self.finish_model_run(RunStatus::Verified, None);
                    return Some(ModelDone::Previewed { goal, lines, sections });
                }
                let made = match self.resolve_plan(&goal, ws.as_deref(), Some(read)) {
                    Ok(Some(plan)) => {
                        let mut b = bridge();
                        self.save_plan(plan, b.as_mut()).map(Some)
                    }
                    other => other.map(|_| None),
                };
                // Runs の行: Project は DB の行を読み戻せた時だけ Verified・1 Page の app（Ok(None)）は呼ぶ側が作る = 確かめていない・作れなかった = Failed
                let read_back = match &made {
                    Ok(Some(View::Project { project, .. })) => {
                        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                        matches!(store.get(Table::Projects, project), Ok(Some(_)))
                    }
                    _ => false,
                };
                match &made {
                    Ok(_) if read_back => self.finish_model_run(RunStatus::Verified, Some(Outcome::Verified { evidence: Vec::new() })),
                    Ok(_) => self.finish_model_run(RunStatus::Verified, None),
                    Err(_) => self.finish_model_run(RunStatus::Failed, None),
                }
                ModelDone::Created { goal, made }
            }
            JobWhat::Edit { project_id, text, hash, preview: true } => self.planned_read(project_id, text, hash, read),
            JobWhat::Edit { project_id, text, hash, preview: false } => {
                use crate::domain::run::RunStatus;
                let read = match read {
                    Some(JobRead::Edit(r)) => r,
                    _ => Err(composer::patch::EditError::Model(MODEL_LOST.into())),
                };
                // L7-AGENT（反証の直し）: Runs の Stop で止めた agent の一言は、答えが届いても当てない（run は Cancelled）
                if std::mem::take(&mut self.asks.stop) && self.asking(&project_id) {
                    if let Some((_, run)) = self.asks.reading.take() {
                        self.finish_ask(&run, &[RunStatus::Cancelled]);
                    }
                    use crate::domain::chat::{Mark, Who};
                    self.say(&project_id, Who::Shibubu, "Stopped before the change was made. Nothing was changed", Mark::Stopped, Vec::new());
                    return Some(ModelDone::Edited { project: project_id, made: Err("Stopped".into()) });
                }
                let made = self.project(&project_id).and_then(|(app, p, _)| self.finish_edit(&project_id, &text, Some(read), &app, &p, &hash));
                // 文の拒否は同期の page_intent(Edit) と同じ Ready・書けなかった時だけ Error（③ C2）
                let state = if std::mem::take(&mut self.edit_refused) { RegionState::Ready } else { RegionState::Error };
                let made = self.remember(made, state);
                // L7-AGENT: 承認した agent の一言なら、その run に結果を書く（agent は `shibubu project context` の runs で読む）
                if self.asking(&project_id) {
                    if let Some((_, run)) = self.asks.reading.take() {
                        self.finish_ask(&run, if made.is_ok() { &[RunStatus::Verifying, RunStatus::Verified] } else { &[RunStatus::Failed] });
                    }
                }
                ModelDone::Edited { project: project_id, made }
            }
        })
    }

    /// W19-2 の配線（図 §33）: Page の依頼を Engine に渡す部品を入れる（app.rs の起動の 1 か所）。呼ばなければ今まで通り（編集として読めない文は「読めません」の 1 行）。
    /// Wave: W19-2
    /// 例: `g.set_engine(EngineSetup { engine: Arc::new(engine_page::CoreEngine { .. }), data_dir: dir.clone() })`（app.rs）
    /// WF2: 入れる時に 1 回、再起動の前に人を待って止まった run（`PageEngine::waiting`）の card を同じ Project に戻す（Decide = 判断の card・承認 = 承認の card）。
    pub fn set_engine(&mut self, setup: EngineSetup) {
        use crate::ui::engine_view::EngineView;
        let waiting = setup.engine.waiting(&self.store.lock().unwrap_or_else(|e| e.into_inner()));
        for w in waiting {
            let view = match w.reply {
                EngineReply::Decide { question, choices, line } => EngineView::Decide { goal: w.goal, question, choices, line },
                EngineReply::Denied { steps, .. } | EngineReply::Recheck { steps, .. } if !steps.is_empty() => EngineView::Approve { goal: w.goal, steps, line: None },
                _ => continue,
            };
            self.engine_views.insert(w.project_id.clone(), view);
            self.engine_waiting.insert(w.project_id, w.run_id);
        }
        self.engine = Some(setup);
    }

    /// TALK-HONEST-1101: 会話の AI に渡す「この Mac で今出来る事」（`PageEngine::abilities`・`set_engine` が無ければ空 = 会話は前と同じ）。
    /// 例: `let able = generated.abilities();`（app.rs の `talk`・会話の前に置く）
    pub fn abilities(&self) -> String {
        self.engine.as_ref().map(|e| e.engine.abilities()).unwrap_or_default()
    }

    /// WF2: Project の止まった run を cancelled にする（Cancel・同じ Project の新しい依頼で card を替えた）。止まっていなければ何もしない。
    fn drop_waiting(&mut self, project_id: &str) {
        if let Some(run_id) = self.engine_waiting.remove(project_id) {
            self.cancel_waiting(&run_id);
        }
    }

    /// run の行が人を待っている（WaitingHuman・WaitingApproval）なら cancelled にする（`drop_waiting`・人が Stop した run が承認や判断で止まって閉じた時の `poll_engine`）。
    fn cancel_waiting(&self, run_id: &str) {
        use crate::domain::run::RunStatus;
        use crate::runtime::receipt;
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(Some(mut rec)) = receipt::load(&st, run_id) {
            if matches!(rec.run.status, RunStatus::WaitingHuman | RunStatus::WaitingApproval) && receipt::set_status(&mut rec, RunStatus::Cancelled).is_ok() {
                let _ = receipt::save(&st, &rec);
            }
        }
    }

    /// W19-2 の配線: `goal` を裏の thread で `PageEngine::run`（製品は `capability_engine::run`）に渡す（同時に 1 つ）。true = 始めた（Page に「進めています」の 1 行・答えは `poll_engine`）。
    /// false = `set_engine` が無い・Engine か model が読んでいる・Project が無い・thread を起こせない（呼ぶ側は今まで通りの 1 行）。
    /// Wave: W19-2
    /// `approved` = 「承認して送る」（か「X を AI に任せる」）で承認した手（新しい依頼は空）。手ごとに `ConsentReceipt`（action_ref・exact payload の digest・
    /// app_id = Project の app・この run の新しい run_id）を作り、画面の操作の手（input に `pid`）の宛先を `EngineRequest::grants` に入れる。
    /// 保証する: `EngineRequest` は Project の app と `run_id`（承認の記録の app_id / run_id と照合）・承認はこの run だけに使う（新しい依頼は新しい run_id）。
    /// 保証しない: run の結果（`poll_engine`）。
    /// WF2: `choice` = Decide の card で選んだ choice の id（承認ではない・None = 新しい依頼か承認）。Project に止まった run が在り、`approved` か `choice` が
    /// 在れば、その run の id で cursor の続きから（`RunStart::Resume`）・それ以外は新しい run（止まった run は cancelled にして card を替える）。
    pub fn start_engine(&mut self, project_id: &str, goal: &str, approved: Vec<crate::ui::engine_view::ApprovalStep>, choice: Option<String>) -> bool {
        self.start_engine_with(project_id, goal, approved, choice, crate::domain::chat::Situation::default(), Said::You, false)
    }

    /// ENGINE-GENERIC-1101（owner 211）: Page の状況（`situation`）の後ろに部品が渡した状況（`with`）を足して `EngineRequest::situation` に載せる `start_engine`。
    /// 呼ぶのは 1 つの口（`ask_engine`）・`part_event`・`start_engine`（`with` は空）だけ。`earlier` = 一言が利用者の今打った言葉ではなく、前に頼んだ事から
    /// 会話の AI が Project に残したきっかけの言葉（`part_event`）= chat に「You」で出さず Shibubu の行 `Doing what you asked earlier: {一言}`（operator 決め・嘘を言わない）。
    /// OPEN-INPLACE-1101: `said` = chat の始めの行（`Said`）・`always` = 承認の card の Always allow（宛先ごとに `"always": [その app で承認した手の action の id]`）
    #[allow(clippy::too_many_arguments)]
    fn start_engine_with(&mut self, project_id: &str, goal: &str, approved: Vec<crate::ui::engine_view::ApprovalStep>, choice: Option<String>, with: crate::domain::chat::Situation, said: Said, always: bool) -> bool {
        use crate::domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject};
        let Some(setup) = &self.engine else { return false };
        if self.engine_job.is_some() || goal.trim().is_empty() {
            return false;
        }
        let Ok((app, p, _)) = self.project(project_id) else { return false };
        let (engine, wake) = (setup.engine.clone(), self.terminals.wake());
        // WF2: 止まった run が在り、承認か選んだ答えを渡す時は同じ run の続き（同じ run の id = 承認の receipt が縛る id）・それ以外は新しい run
        let resume = self.engine_waiting.get(project_id).cloned().filter(|_| !approved.is_empty() || choice.is_some());
        if resume.is_none() {
            self.drop_waiting(project_id);
        }
        // TALK-HONEST-1101 追記 232: 新しい一言の run は前の「Let Shibubu open it」の読み直しを持ち越さない（`open_part` は始めた後に付け直す・承認と選んだ答えの続きは持ち越す）
        if approved.is_empty() && choice.is_none() {
            self.reread_after = None;
        }
        let (run_id, now) = (resume.clone().unwrap_or_else(storage::new_id), crate::runtime::now_rfc3339());
        let consents = approved
            .iter()
            .map(|s| ConsentReceipt {
                id: storage::new_id(),
                app_id: app.clone(),
                subject: ConsentSubject::Action { action_ref: s.action.clone(), run_id: Some(run_id.clone()) },
                scopes: s.scopes.clone(),
                payload_digest: Some(s.digest.clone()),
                decision: ConsentDecision::Approved,
                decided_at: now.clone(),
            })
            .collect();
        let mut grants: Vec<Value> = Vec::new();
        for s in approved.iter().filter(|s| s.input["pid"].is_u64()) {
            let to = serde_json::json!({ "pid": s.input["pid"], "app_name": s.input["app_name"], "bundle_id": s.input["bundle_id"], "document": s.input["document"] });
            if !grants.contains(&to) {
                grants.push(to);
            }
        }
        if always {
            for to in grants.iter_mut() {
                let mut kinds: Vec<&str> = Vec::new();
                for s in approved.iter().filter(|s| s.input["pid"] == to["pid"] && s.input["app_name"] == to["app_name"]) {
                    if !kinds.contains(&s.action.as_str()) {
                        kinds.push(&s.action);
                    }
                }
                to["always"] = serde_json::json!(kinds);
            }
        }
        let waiting = Arc::new(std::sync::Mutex::new(None));
        // 裏の thread が書き、Page は poll_engine で読む。idle の frame は 0 なので起こす（無いと次の入力まで 1 行が出ない・W19a2 と同じ）
        let progress: Arc<dyn Fn(Option<String>) + Send + Sync> = {
            let (slot, wake) = (waiting.clone(), self.terminals.wake());
            Arc::new(move |line| {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = line;
                if let Some(w) = &wake {
                    w();
                }
            })
        };
        let runs = self.apps.iter().find(|o| o.rt.revision().definition().id == app).map(|o| EngineRuns {
            store: self.store.clone(),
            session_id: o.rt.session_id().to_owned(),
            space_id: project_id.to_owned(),
            app_version: o.rt.revision().definition().version,
            start: if resume.is_some() { RunStart::Resume { choice: choice.clone() } } else { RunStart::New },
        });
        let situation = self.situation(project_id).with(with);
        // ENGINE-ADJUST-1102 ①: 人の Stop の印は runtime の `running` の表に置く（Runs の Stop が同じ物を立てる）・card の Stop は `stop_engine`
        let stop = self.rt.track(&run_id);
        let req = EngineRequest { run_id: run_id.clone(), consents, grants, progress, runs, stop: stop.clone(), ..self.engine_request(project_id, &app, &p, goal, situation) };
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("engine-run".into()).spawn(move || {
            // 追記 234: 窓の 1 行は chat の thread と同じ所（`with_windows`）で足す = chat と同じ塊
            let mut req = req;
            req.situation = with_windows(Some(&*engine), std::mem::take(&mut req.situation));
            let _ = tx.send(engine.run(req));
            if let Some(wake) = wake {
                wake();
            }
        });
        if spawned.is_err() {
            return false;
        }
        // WF2: 判断の card で選んだ続き（approved は空・choice が Some）は新しい依頼ではない = 一言を 2 度目に足さない
        if approved.is_empty() && choice.is_none() && !self.accepted(project_id, goal) {
            use crate::domain::chat::Who;
            let line = match said {
                Said::You => Some((Who::You, goal.trim().to_owned())),
                Said::Earlier => Some((Who::Shibubu, format!("Doing what you asked earlier: {}", goal.trim()))),
                Said::Quiet => None,
            };
            if let Some((who, line)) = line {
                self.say(project_id, who, &line, crate::domain::chat::Mark::None, Vec::new());
            }
        }
        self.engine_job = Some(EngineJob { project_id: project_id.to_owned(), goal: goal.trim().to_owned(), run_id, approved, rx, waiting, stop, started: std::time::Instant::now() });
        self.engine_views.insert(project_id.to_owned(), crate::ui::engine_view::EngineView::Running { goal: goal.trim().to_owned(), waiting: None, stopping: false });
        true
    }

    /// 一言の Engine の依頼の共通の中身（`start_engine_with` の run と `preview_engine_plan` の plan が同じ宛先・書類・repo・状況を渡す = 案に見せた手と同じ手）。
    /// 承認・同意・runs 表は空・run の id は新しい id・進行の 1 行は捨てる（run は `..` で上書きする）。
    fn engine_request(&self, project_id: &str, app: &str, p: &ProjectDefinitionV1, goal: &str, situation: crate::domain::chat::Situation) -> EngineRequest {
        // 今の Page に app が無い（Dev・Diagram）時は、一言が名指す書類が Project に 1 つならその Page の app（engine_target）
        let current = self.routers.get(project_id).and_then(|r| p.page(r.current()));
        let (target, file) = engine_target(p, current, goal.trim()).map_or((None, None), |(t, f)| (Some(t), f));
        let document = file.as_deref().and_then(|f| Path::new(f).file_name()?.to_str()).map(str::to_owned);
        // G81: 面の file は repo からの相対（`SurfaceTarget::App`）
        let document_file = file.as_deref().zip(repo_dir(p)).map(|(f, dir)| format!("file://{}", dir.join(f).display()));
        // B74: repo の場所（model が書いた repo からの相対の読みを Engine が直す）
        let repo = repo_dir(p).map(|dir| format!("file://{}", dir.display()));
        // MIRROR-PIN-1102（図 §71）: 今の Page の Mirror の面が映している target の窓（同じ app の窓が 2 つ以上映っていて決まらなければ聞く 1 行）
        #[cfg(feature = "mirror")]
        let window = target.as_deref().zip(current).zip(self.mirror.as_ref()).and_then(|((want, page), m)| {
            let refs = surface_refs(page);
            let shown: Vec<_> = p
                .surface_specs
                .iter()
                .filter(|s| s.backend == project::SurfaceBackendKind::Mirror && refs.contains(&s.id.as_str()))
                .filter_map(|s| m.shown(&s.id))
                .filter(|(t, _)| t.bundle_id == want)
                .collect();
            pin_window(&shown)
        });
        #[cfg(not(feature = "mirror"))]
        let window = None;
        EngineRequest {
            app_id: app.to_owned(),
            run_id: storage::new_id(),
            goal: goal.trim().to_owned(),
            consents: Vec::new(),
            grants: Vec::new(),
            target,
            document,
            document_file,
            window,
            repo,
            llm: self.llm.clone().map(|l| Arc::new(Asked::of(l, &[], None, None, Some(project_id))) as Arc<dyn crate::llm::Llm>),
            progress: Arc::new(|_| {}),
            runs: None,
            automation: self.automation,
            situation,
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// ENGINE-ADJUST-1102 ①（owner 256「止めるのは人の Stop だけ」・Running の card の Stop の button が呼ぶ入口）: この Project で走っている Engine の run の人の Stop。
    /// 印（`EngineJob::stop` = `EngineRequest::stop` = runtime の `running` の flag）を立てて進行の 1 行を `STOPPING_LINE` にするだけで、止めるのは Engine
    /// （段の間・AI の前後・手の前・読みの続きの前で見て、副作用の手を打たず「Stopped by you」で閉じる = 答えは今まで通り `poll_engine` に届く）。
    /// Runs の Stop（`run_action` の Cancel → `Runtime::cancel`）は同じ flag を runs 表に行が出来てから（Execution の前）立てる。走っていなければ false（何もしない）。
    /// 例（card の button）: `if !g.stop_engine(&project) { /* もう終わっている */ }`
    pub fn stop_engine(&mut self, project_id: &str) -> bool {
        let Some(job) = self.engine_job.as_ref().filter(|j| j.project_id == project_id) else { return false };
        job.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        *job.waiting.lock().unwrap_or_else(|e| e.into_inner()) = Some(STOPPING_LINE.into());
        true
    }

    /// ENGINE-GENERIC-1101（owner 211）: 部品から Engine への 1 つの口。部品（録音の帯・Page の部品・chat）は app の名前も決まった機能も持たず、一言
    /// （利用者がその時に打った言葉か、利用者の言葉から会話の AI が Project に残したきっかけの言葉）と、部品が知っている状況（`with` = 録った file・選んだ値・
    /// 直前の事）だけを渡す。Engine はその場で app の口（AppleScript の辞書・AX の木・menu・開ける file の型・URL scheme・CLI）を読んで手を組み、初めての手は
    /// 承認の card・済みは読み戻しで確かめる（`start_engine` と同じ run・同時に 1 つ・状況は `situation` の後ろに `with`）。Err = 始められない 1 行（`ENGINE_BUSY`）。
    /// 例（部品の `shibubu.act`）: `g.ask_engine(&project, &fill_value(&intent, &value), Situation::default())?`
    /// 部品の手のきっかけ（`part_event`）も同じ run（`start_engine_with`）だけを通り、chat の行だけが違う（`earlier`）
    pub fn ask_engine(&mut self, project_id: &str, say: &str, with: crate::domain::chat::Situation) -> Result<(), String> {
        if self.start_engine_with(project_id, say, Vec::new(), None, with, Said::You, false) {
            Ok(())
        } else {
            Err(ENGINE_BUSY.into())
        }
    }

    /// ENGINE-AUDIT W9 本体: chat の一言を分ける依頼（`PageEngine::classify` に渡す `EngineRequest` = run と同じ宛先・書類・状況・AI・新しい run の id）と Engine。
    /// Engine か AI が無ければ None（今まで通り会話の AI だけ）。`situation` は chat が同じ時点で集めた物（`situation`）。裏の thread で `engine.classify(req)` を呼ぶ
    pub fn classify_request(&mut self, project_id: &str, text: &str, situation: crate::domain::chat::Situation) -> Option<(Arc<dyn PageEngine>, EngineRequest)> {
        let engine = self.fork_engine()?;
        let Ok((app, p, _)) = self.project(project_id) else { return None };
        let req = self.engine_request(project_id, &app, &p, text, situation);
        req.llm.is_some().then_some((engine, req))
    }

    /// W9 本体: 分けた結果に従う（app.rs の `poll_talk` が `TalkMsg::Routed` で呼ぶ）。ask = Shibubu の問いの行（選択肢 = 候補・押すとその文が次の一言 = また分ける）・
    /// engine = 一言のまま Engine へ（chat にはもう人の一言が在るので `Said::Quiet`・Engine が act だけを行う・走れなければ 1 行）。talk は呼ぶ側（会話の AI の答えを待つ）
    pub fn follow_route(&mut self, project_id: &str, say: &str, route: &crate::domain::chat::Route) {
        use crate::domain::chat::{Mark, Turn, Who};
        if let Some((question, choices)) = &route.ask {
            let turn = Turn { who: Who::Shibubu, text: question.clone(), mark: Mark::None, at: crate::runtime::now_rfc3339(), attachments: Vec::new(), choices: choices.clone(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None };
            self.say_turn(project_id, turn);
            return;
        }
        if !route.engine {
            return;
        }
        // B86: 候補が 2 つ以上でも Page の app に決まった = 聞かずに進む事を 1 行（「Using TextEdit for “…”」）
        if let Some(using) = &route.using {
            self.say(project_id, Who::Shibubu, using, Mark::None, Vec::new());
        }
        if !self.start_engine_with(project_id, say, Vec::new(), None, crate::domain::chat::Situation::default(), Said::Quiet, false) {
            self.say(project_id, Who::Shibubu, ENGINE_BUSY, Mark::None, Vec::new());
        }
    }

    /// ENGINE-GENERIC-1101（owner 211・operator の決め 2）: 今の Page の状況 = Engine に一言と一緒に渡す事実（`domain::chat::Situation`）。app の名前で
    /// 分けず、Page の木と Project の data から集める: Page の題・その Page の面（label と、app〈bundle id・書類〉か web〈URL〉か）・部品で選んだ値
    /// （canvas の `values` を部品の見出しと）・録った take（録音の帯の今・`files` にも）。面の書類は名前だけ（場所は今まで通り `EngineRequest::document_file`）。
    /// 窓を開いている app は裏の thread が `with_windows` で足す・直前の手は部品が `with` で渡す（`part_event`）。
    /// `files` の順は木の順で、承認の後の続きでも同じ物を同じ順に集める（手の `situation.files.<n>`）。
    /// 追記 234（owner「engineが把握することとchatが把握することは全て共有して」）: chat の AI（app.rs の `talk_in`）と Engine（`start_engine_with`）の
    /// ただ 1 つの入口。上に足して: 部品が今出している誤り（TALK-HONEST 追記 232・`“{部品}” shows an error: {1 行}`）・Engine の今と直近の run
    /// （頼み・した事・通った / 止まった・確かめた手）・会話の直近 2 往復・出来る事（`abilities`）。両方が `Situation::text` で同じ字にする。
    /// `&mut self` にした理由: 会話（`chat`）を読む = 初めての Project は store から写しを作る。
    /// 例: `g.situation(&project)` → facts `["Page: Hum to Guitar", "On this page: Mixer (app com.example.mixer)", "Chosen in “Guitar”: Acoustic Guitar",
    ///       "“Guitar” shows an error: Looked for …", "Shibubu's last run here: You asked: … · Result: …", "They said in this chat: …", "What Shibubu can do on this Mac: …"]`
    pub fn situation(&mut self, project_id: &str) -> crate::domain::chat::Situation {
        use crate::domain::chat::{Situation, Who, TALK_WAIT};
        use crate::ui::engine_view::EngineView;
        let mut s = Situation::default();
        let Ok((_, p, _)) = self.project(project_id) else { return s };
        if let Some(page) = self.routers.get(project_id).and_then(|r| p.page(r.current())).or(p.pages.first()) {
            s.facts.push(format!("Page: {}", page.title));
            for n in page.root.nodes() {
                let name = n.label.clone().unwrap_or_else(|| n.key.clone());
                if let Some(spec) = n.props.get("surface_ref").and_then(Value::as_str).and_then(|r| p.surface_specs.iter().find(|x| x.id == r)) {
                    match &spec.target {
                        project::SurfaceTarget::App { bundle_id, file } => {
                            let doc = file.as_deref().filter(|f| !f.trim().is_empty()).map_or_else(String::new, |f| format!(" · file {f}"));
                            s.facts.push(format!("On this page: {name} (app {bundle_id}{doc})"));
                        }
                        project::SurfaceTarget::Url { url } => s.facts.push(format!("On this page: {name} (web {url})")),
                    }
                }
                if let Some(c) = crate::domain::ui::CanvasProps::of(n) {
                    for (k, v) in &c.values {
                        let v = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                        s.facts.push(if c.values.len() == 1 { format!("Chosen in “{name}”: {v}") } else { format!("Chosen in “{name}” ({k}): {v}") });
                    }
                }
                let take = n.props.get("recorder_ref").and_then(Value::as_str).and_then(|r| self.takes.by_ref.get(&format!("{project_id}/{r}")));
                if let Some((path, renderer::TakeStage::Recorded { seconds, .. })) = take.and_then(|t| Some((t.path.clone()?, &t.stage))) {
                    let file = path.file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                    s.facts.push(format!("Recorded in “{name}”: {file} ({seconds:.0} s)"));
                    s.files.push(path);
                }
                // TALK-HONEST-1101 追記 232: 部品が今 頁に赤で出している誤り（read が読めなかった 1 行）
                if let Some(miss) = self.part_errors.get(&format!("{project_id}/{}", n.key)) {
                    s.facts.push(format!("“{name}” shows an error: {}", miss.line));
                }
            }
        }
        // 追記 234: Engine の今（走っている・人を待っている）と直近の run（`poll_engine` の `last_runs`）
        match self.engine_views.get(project_id) {
            // ENGINE-AUDIT W1（話 12「今どうなってる？」）: 段の 1 行と経過秒も chat に（Engine と chat が同じ物を知る・234）
            Some(EngineView::Running { goal, waiting, .. }) => s.facts.push(match waiting {
                Some(w) => format!("Shibubu is doing now: {goal} · {w}"),
                None => format!("Shibubu is doing now: {goal}"),
            }),
            Some(EngineView::Approve { goal, steps, .. }) => s.facts.push(format!("Waiting for their approval before sending {}: {goal}", crate::domain::plural(steps.len(), "step"))),
            Some(EngineView::Consent { goal, line, .. }) => s.facts.push(format!("Stopped before sending “{goal}”: {line}")),
            Some(EngineView::Decide { goal, question, .. }) => s.facts.push(format!("Waiting for their choice for “{goal}”: {question}")),
            Some(EngineView::Ask { who, text }) => s.facts.push(format!("{who} asked Shibubu (waiting for their approval): {text}")),
            Some(EngineView::Line { .. }) | None => {}
        }
        s.facts.extend(self.last_runs.get(project_id).cloned());
        // 追記 234: 会話で決まった事（今の session の直近 2 往復・chat の AI には会話の全文もある）
        let turns = self.chat(project_id);
        let said: Vec<_> = turns.iter().filter(|t| t.text != TALK_WAIT).collect();
        s.facts.extend(said[said.len().saturating_sub(4)..].iter().map(|t| format!("{} said in this chat: {}", if t.who == Who::You { "They" } else { "Shibubu" }, brief(&t.text))));
        // 追記 234: この Mac で出来る事（前は chat の頭だけ・`talk_abilities` の塊のまま 1 つ）
        let able = self.abilities();
        if !able.is_empty() {
            s.facts.push(able);
        }
        s
    }

    /// ENGINE-GENERIC-1101（owner 211・operator の決め 3）: 部品の手が起きた（`node` = 手を持つ部品の key・`event` = `domain::ui::TRIGGER_ACTS` の手・
    /// 例 録り終えた `recorded`）。Project に会話の AI が利用者の一言から残したきっかけ（canvas の `on` の act = `<node>.<event>`）が在れば、その言葉
    /// （key が在れば値の穴 `{value}` を部品の値で埋めた物・2 つ以上は `. ` で繋ぐ）を `with` と一緒に `ask_engine` と同じ run へ（chat には「You」でなく
    /// Shibubu の `Doing what you asked earlier: {一言}` = 利用者が今打った言葉ではない・`start_engine_with` の `earlier`）。初めての手は承認の card・
    /// 済みは読み戻し・出来なければ Engine が何が足りないかの 1 行（`ask_engine` と同じ run）。きっかけが無ければ何もしない（Ok）= 製品が前もって
    /// 決めた手（take をどこかへ入れる button 等）は無い。値をまだ選んでいない → 走らせずに「先に選ぶ」の 1 行。
    /// 例（`draw_page_body` の録り終えた帯）: `g.part_event(&project, &node.key, "recorded", Situation { facts: vec![…], files: vec![take] })`
    pub fn part_event(&mut self, project_id: &str, node: &str, event: &str, with: crate::domain::chat::Situation) -> Result<(), String> {
        // ENGINE-AUDIT W9 本体（節「AI の上限」= 部品の道で上限になる）: 部品の手は人の手 = 1 つの頼み（AI の枠を数え直す）
        self.new_request();
        let (_, p, _) = self.project(project_id)?;
        let act = format!("{node}.{event}");
        let mut says = Vec::new();
        for pg in &p.pages {
            for (part, t, v) in crate::domain::ui::triggers_on(&pg.root, &act) {
                if t.key.trim().is_empty() {
                    says.push(t.intent.trim().to_owned());
                    continue;
                }
                let Some(v) = v else {
                    let name = pg.root.find(&part).and_then(|n| n.label.clone()).unwrap_or(part);
                    return Err(format!("Choose in “{name}” first"));
                };
                says.push(t.goal(&v).trim().to_owned());
            }
        }
        if says.is_empty() {
            return Ok(());
        }
        if self.start_engine_with(project_id, &says.join(". "), Vec::new(), None, with, Said::Earlier, false) {
            Ok(())
        } else {
            Err(ENGINE_BUSY.into())
        }
    }

    /// AI-COMPOSE-1101（owner 206）: Page の AI が書いた部品の `shibubu.read(app, what)` を裏の thread で 1 本（`PageEngine::read`・AI 1 回・Project の枠で数える）。
    /// 同じ部品の read が走っている間・Engine が無い間は始めずに理由の 1 行（頁の script の loop で AI の枠を減らさない）。答えは `take_reads` で受ける。
    /// 例（app.rs の `part_sync`）: `if let Err(line) = g.start_part_read(&project, &node, id, &app, &what) { canvas.answer(id, &Err(line), false) }`
    /// OPEN-INPLACE-1101 の 4（owner が chat に打った「一度取得したら list するように」）: その部品が前に読めた値（Project の app の ui_state の `READS_KEY`）が
    /// 在れば、次の `take_reads` がすぐその値で答え（頁は前の一覧を出し続ける）、裏で読み直す。読み直しが読めて値が違えば残した値を書き替えてその部品を
    /// 1 回読み直す（`take_rereads`）・同じなら何もしない・読めなければ何も出さない（app の画面が別の物に替わっても一覧は消さない）。誤りの 1 行と
    /// 「Let Shibubu open it」は一度も読めていない部品だけ。読めた値は Project の app の ui_state に残す（props でない = ⌘Z の revision を足さない・`keep_read`）。
    /// 「Let Shibubu open it」の ✓ の後の読み直しは前の値で答えず本物を読む。
    pub fn start_part_read(&mut self, project_id: &str, node: &str, id: u64, app: &str, what: &str) -> Result<(), String> {
        let key = format!("{project_id}/{node}");
        let kept = self.kept_read(project_id, node, app, what);
        if self.reads.iter().any(|r| r.project == project_id && r.node == node) {
            // 読み直しの最中に頁が開き直した: 前に読めた値が在ればそれで答える（2 本目は読まない）
            let Some(v) = kept else { return Err("Still reading for this part · try again shortly".into()) };
            self.kept_answers.push((node.to_owned(), id, v));
            return Ok(());
        }
        // OPEN-INPLACE-1101 の 4: 前に読めた値ですぐ答え、裏で読み直す。「Let Shibubu open it」の ✓ の後（true）は前の値で答えず本物を読む・
        // 裏の読み直しが新しい値を残した後の開き直し（false）は読み直さない
        let mode = self.next_read.remove(&key);
        // 232: 読み直しを始めた = 頁の前の誤りの 1 行はもう出ていない（読めなければ `take_reads` が新しい 1 行を覚える）・OPEN-INPLACE: 止まった 1 行も消す
        self.part_errors.remove(&key);
        self.part_stops.remove(&key);
        let answered = kept.is_some() && mode != Some(true);
        if let Some(v) = kept.as_ref().filter(|_| answered) {
            self.kept_answers.push((node.to_owned(), id, v.clone()));
            if mode == Some(false) {
                return Ok(());
            }
        }
        let Some(engine) = self.engine.as_ref().map(|e| e.engine.clone()) else {
            // 読む道が無い: 一度読めた部品は前の値・一度も読めていない部品だけ理由の 1 行
            let Some(v) = kept else {
                self.part_errors.insert(key, READ_NONE.into());
                return Err(READ_NONE.into());
            };
            if !answered {
                self.kept_answers.push((node.to_owned(), id, v));
            }
            return Ok(());
        };
        let llm = self.llm.clone().map(|l| Arc::new(Asked::of(l, &[], None, None, Some(project_id))) as Arc<dyn crate::llm::Llm>);
        let (tx, rx) = std::sync::mpsc::channel();
        let (a, w, wake) = (app.to_owned(), what.to_owned(), self.terminals.wake());
        std::thread::Builder::new()
            .name("canvas-read".into())
            .spawn(move || {
                let _ = tx.send(engine.read(&a, &w, llm));
                if let Some(wake) = wake {
                    wake();
                }
            })
            .map_err(|_| "Couldn't start reading · try again".to_owned())?;
        self.reads.push(PartRead { project: project_id.to_owned(), node: node.to_owned(), id, app: app.to_owned(), what: what.to_owned(), kept, answered, rx });
        Ok(())
    }

    /// OPEN-INPLACE-1101 の 4: その部品が前に読めた値（Project の app の session の ui_state・`READS_KEY`）
    fn kept_read(&self, project_id: &str, node: &str, app: &str, what: &str) -> Option<Value> {
        let (app_id, _, _) = self.project(project_id).ok()?;
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.get_ui_state(&session_of(&app_id), READS_KEY).ok()??.get(read_slot(node, app, what)).cloned()
    }

    /// OPEN-INPLACE-1101 の 4: 読めた値を残す（同じ値は書かない・書けなくても読んだ値は頁に出る = 次の起動で前の値が無いだけ）
    fn keep_read(&self, project_id: &str, node: &str, app: &str, what: &str, value: Value) {
        let Ok((app_id, _, _)) = self.project(project_id) else { return };
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = store.get_ui_state(&session_of(&app_id), READS_KEY).ok().flatten().filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
        let slot = read_slot(node, app, what);
        if all.get(&slot) != Some(&value) {
            all[slot] = value;
            let _ = store.put_ui_state(&session_of(&app_id), READS_KEY, &all);
        }
    }

    /// AI-COMPOSE-1101: 届いた部品の read の答え（node の key・問いの id・答え）。毎 frame・待たない。答えずに thread が終わった物も理由の 1 行で返す（頁を待たせたままにしない）。
    /// TALK-HONEST-1101 追記 232: 読めなかった答えはその部品の誤りとして覚える（`situation` が chat と Engine に渡す・`open_part` が一言を使う）・読めたら消す。
    pub fn take_reads(&mut self) -> Vec<(String, u64, Result<Value, ReadMiss>)> {
        use std::sync::mpsc::TryRecvError;
        let mut done: Vec<(String, u64, Result<Value, ReadMiss>)> = self.kept_answers.drain(..).map(|(n, id, v)| (n, id, Ok(v))).collect();
        let (mut keep, mut again) = (Vec::new(), Vec::new());
        let errors = &mut self.part_errors;
        self.reads.retain(|r| match r.rx.try_recv() {
            Err(TryRecvError::Empty) => true,
            got => {
                let got = got.unwrap_or_else(|_| Err("The read stopped · try again".into()));
                let key = format!("{}/{}", r.project, r.node);
                match (got, &r.kept) {
                    // OPEN-INPLACE-1101 の 4: 裏の読み直し（頁は前の値を出している）= 違う値だけ残して 1 回開き直す・同じ値と読めない時は何も出さない
                    (Ok(v), Some(before)) if r.answered => {
                        if v != *before {
                            keep.push((r.project.clone(), r.node.clone(), r.app.clone(), r.what.clone(), v));
                            again.push((r.project.clone(), r.node.clone()));
                        }
                    }
                    (Err(_), _) if r.answered => {}
                    (Ok(v), _) => {
                        errors.remove(&key);
                        keep.push((r.project.clone(), r.node.clone(), r.app.clone(), r.what.clone(), v.clone()));
                        done.push((r.node.clone(), r.id, Ok(v)));
                    }
                    // 一度読めた部品は誤りの 1 行を出さず前の値（誤りは一度も読めていない時だけ）
                    (Err(_), Some(before)) => done.push((r.node.clone(), r.id, Ok(before.clone()))),
                    (Err(miss), None) => {
                        errors.insert(key, miss.clone());
                        done.push((r.node.clone(), r.id, Err(miss)));
                    }
                }
                false
            }
        });
        for (project, node, app, what, v) in keep {
            self.keep_read(&project, &node, &app, &what, v);
        }
        for (project, node) in again {
            self.next_read.insert(format!("{project}/{node}"), false);
            self.rereads.push((project, node));
        }
        done
    }

    /// TALK-HONEST-1101 追記 232（operator の決め・owner 206「出来ないで止めない」）: 部品の「Let Shibubu open it」。その部品の読めなかった答えが持つ一言
    /// （`ReadMiss::open` = `Open {探した物} in {app}`・app の名前は read の data で code に枝は無い）を 1 つの口（`ask_engine`・状況は `situation`）へ。
    /// 初めての手は承認の card・済みは読み戻し（今の道のまま）。その run が ✓ で終わったらこの部品を 1 回だけ読み直す（`take_rereads`）。
    /// OPEN-INPLACE-1101 の 1: この run の今と承認の card は部品の所（`part_run`）に出し、chat には Shibubu の記録の行だけ（`part_run` の doc）。
    /// Err = 押した所に出す 1 行（開ける一言が無い・Engine が動いている）。
    /// 例（app.rs の `part_sync`）: `CanvasMsg::Open => if let Err(line) = g.open_part(&project, node) { self.message = line }`
    pub fn open_part(&mut self, project_id: &str, node: &str) -> Result<(), String> {
        let key = format!("{project_id}/{node}");
        let Some(say) = self.part_errors.get(&key).and_then(|m| m.open.clone()) else { return Err(OPEN_NONE.into()) };
        // ENGINE-AUDIT W9 本体: 「Let Shibubu open it」は人の手 = 1 つの頼み
        self.new_request();
        if !self.start_engine_with(project_id, &say, Vec::new(), None, crate::domain::chat::Situation::default(), Said::Quiet, false) {
            return Err(ENGINE_BUSY.into());
        }
        self.part_stops.remove(&key);
        self.reread_after = Some((project_id.to_owned(), node.to_owned()));
        Ok(())
    }

    /// TALK-HONEST-1101 追記 232: 1 回だけ読み直す部品（Project の id・node の key）を渡して空にする（毎 frame・app.rs の `part_sync` が頁を開き直す = 頁の script がもう一度 read）。
    pub fn take_rereads(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.rereads)
    }

    /// OPEN-INPLACE-1101 の 1（owner 240「let shibubu open it を押したら勝手に動くように・chat に戻る意味がわからない」）: 「Let Shibubu open it」で始めた run
    /// （`open_part` が覚えた Project と node）の今を、その部品の所に出す 1 つ（app.rs の `part_sync` が替わった時だけ `Canvas::run` で頁へ）。
    /// Running → `Working`（Engine の progress の 1 行・無ければ `Working on “{一言}”`）・Approve → `Approve`（`approval_line`・頁は Always allow / Allow once /
    /// Cancel = `CanvasMsg::Allow` / `Cancel` → `engine_act`）・済み（✓）→ None（その部品を 1 回読み直す = `take_rereads`）・止まった・拒まれた・同意が要る →
    /// `Stopped`（理由と次の一手の 1 行・次の read で消える）。他の部品・他の Project・部品から始めていない run は None。
    /// 保証する: この run の間 `engine_view` は Running / Approve を返さない（chat の列と Page の欄の下に card を出さない = 押す所は部品の所 1 つ）・
    /// chat には Shibubu の行で始めの 1 行（`Opening {一言} for this part`）と終わりの 1 行（済み・止まった理由）だけ。
    /// 例（app.rs）: `let now = g.part_run(&project, node); if now != last { canvas.run(now.as_ref()) }`
    /// Wave: OPEN-INPLACE-1101
    pub fn part_run(&self, project_id: &str, node: &str) -> Option<crate::surface::PartRun> {
        use crate::surface::PartRun;
        use crate::ui::engine_view::{approval_line, asks_every_time, consent_label, EngineView};
        if let Some(line) = self.part_stops.get(&format!("{project_id}/{node}")) {
            return Some(PartRun::Stopped(line.clone()));
        }
        if self.reread_after.as_ref().is_none_or(|(p, n)| p != project_id || n != node) {
            return None;
        }
        match self.engine_views.get(project_id)? {
            EngineView::Running { goal, waiting, .. } => Some(PartRun::Working(waiting.clone().unwrap_or_else(|| format!("Working on “{goal}”")))),
            // ENGINE-AUDIT W7: 押す手だけの承認は毎回聞く = 頁に Always allow を出さない（その 1 行）
            EngineView::Approve { goal, steps, .. } => Some(PartRun::Approve(approval_line(goal, steps), asks_every_time(steps))),
            EngineView::Consent { steps, .. } => Some(PartRun::Approve(consent_label(steps), None)),
            EngineView::Decide { question, .. } => Some(PartRun::Stopped(format!("{question} · Choose in the chat"))),
            EngineView::Line { .. } | EngineView::Ask { .. } => None,
        }
    }

    /// OPEN-INPLACE-1101 の 5（234・owner 240「chat が約束した事と Engine の手が違う」）: Project の chat の案の `Preview plan` を、Build now と同じ道で読む
    /// （裏の thread で 1 本 = 変更の文の model の読み → 読めない文〈Engine へ行く文〉なら `PageEngine::plan`）。読んでいる間は `PLAN_READING`、届いたら
    /// `poll_model_job` が案の card に行を置く（Engine の手の行・Project の変更の 1 行・作れない理由の 1 行の後に `PLAN_UNMADE`・`ModelDone::Planned`）。
    /// Err = 始めない（名前の表で読む文・AI が無い・別の読みの最中）= 呼ぶ側は今の 2 行のまま。
    /// 例（app.rs の `change_step`）: `Intent::PreviewPlan => if self.generated.preview_engine_plan(&project, &sentence).is_err() { /* 今の 2 行 */ }`
    /// 保証する: 何も送らない・何も書かない（読みの答えと plan の手順は覚える）・その後の同じ文の Build now は変更の読み・interpret・generate を呼び直さない
    /// （`start_edit` が覚えた読みを流し、Engine は覚えた操作と保存した手順で走る = Build の AI の呼び出しは Preview の分だけ）。
    /// 保証しない: Adapter を新しく作る手順の Build（試験の後に作り直す = generate 1 回）・再起動の後の Build（interpret 1 回）。
    /// Wave: OPEN-INPLACE-1101
    /// 同じ文・同じ Project の版（hash）の前の Preview の答えが在れば、その行を置き直すだけ（AI 0 回・版が替わっていれば読み直す）。
    pub fn preview_engine_plan(&mut self, project_id: &str, sentence: &str) -> Result<(), String> {
        let text = sentence.trim();
        let hash = self.project(project_id).map(|(_, _, h)| h).ok();
        if let Some(lines) = self.planned_edit.as_ref().filter(|k| hash.as_deref().is_some_and(|h| k.is(project_id, text, h))).map(|k| k.lines.clone()) {
            self.preview_change(project_id, lines);
            return Ok(());
        }
        if self.model_job.is_some() || !self.start_change_read(project_id, text, true) {
            return Err(PLAN_NONE.into());
        }
        self.preview_change(project_id, vec![crate::domain::chat::PLAN_READING.to_owned()]);
        Ok(())
    }

    /// OPEN-INPLACE-1101 の 5（operator の直し ①・owner 240「押したのに何も反応しない」）: この Project の案の Preview の読みの最中に Build now を押した →
    /// 押した事を案の card に出し（`PLAN_READING` の下に `BUILD_QUEUED`）、読み終えたらそのまま Build する（`planned_read` が `start_edit` = 覚えた読みと plan で
    /// AI の呼び出しは増えない）。true = 覚えた・false = その文の Preview を読んでいない（呼ぶ側は今まで通り）。
    /// 例（app.rs の `change_step`）: `Intent::AcceptPlan => if g.model_busy_line().is_some() { g.build_when_read(&project, &sentence); return }`
    pub fn build_when_read(&mut self, project_id: &str, text: &str) -> bool {
        let text = text.trim();
        let reading = matches!(&self.model_job, Some(ModelJob { what: JobWhat::Edit { project_id: p, text: t, preview: true, .. }, .. }) if p == project_id && t == text);
        if reading {
            self.build_queued = Some((project_id.to_owned(), text.to_owned()));
            self.preview_change(project_id, vec![crate::domain::chat::PLAN_READING.to_owned(), crate::domain::chat::BUILD_QUEUED.to_owned()]);
        }
        reading
    }

    /// OPEN-INPLACE-1101 の 5: この Project の案の card の Build now を灰にする待ちの理由（app.rs の `chat_column` の `wait`）。この Project の Preview の読みの
    /// 最中は None（押せる = `build_when_read`）・押した後は `BUILD_QUEUED`・他は `model_busy_line`（今まで通り）。
    pub fn card_wait(&self, project_id: &str) -> Option<&'static str> {
        if self.build_queued.as_ref().is_some_and(|(p, _)| p == project_id) {
            return Some(crate::domain::chat::BUILD_QUEUED);
        }
        let previewing = matches!(&self.model_job, Some(ModelJob { what: JobWhat::Edit { project_id: p, preview: true, .. }, .. }) if p == project_id);
        if previewing {
            None
        } else {
            self.model_busy_line()
        }
    }

    /// OPEN-INPLACE-1101 の 5: 案の Preview の答え（`JobRead::Planned`）を案の card の行にし（案がまだ同じ文の時だけ）、変更の読みを Build now のために覚える。
    fn planned_read(&mut self, project_id: String, text: String, hash: String, read: Option<JobRead>) -> ModelDone {
        use crate::domain::chat::PLAN_UNMADE;
        let (edit, steps) = match read {
            Some(JobRead::Planned(edit, steps)) => (Some(edit), steps),
            _ => (None, None),
        };
        let mut lines = match (steps, &edit) {
            (Some(Ok(steps)), _) if !steps.is_empty() => std::iter::once(PLAN_STEPS.to_owned()).chain(steps).collect(),
            (Some(Ok(_)), _) => vec![PLAN_NONE.to_owned()],
            (Some(Err(line)), _) => vec![line],
            (None, Some(Err(composer::patch::EditError::Model(why)))) => vec![format!("AI wasn't available · {why}"), "Build now asks the AI again.".into()],
            (None, None) => vec![MODEL_LOST.to_owned()],
            (None, Some(_)) => vec![CHANGE_PREVIEW.to_owned()],
        };
        lines.push(PLAN_UNMADE.to_owned());
        // AI の失敗は覚えない（Build now と Preview の押し直しは今まで通り AI に読ませる）
        if let Some(read) = edit.filter(|e| !matches!(e, Err(composer::patch::EditError::Model(_)))) {
            self.planned_edit = Some(PlannedEdit { project_id: project_id.clone(), text: text.clone(), hash, read, lines: lines.clone() });
        }
        if crate::domain::chat::pending_proposal(self.chat_of(&project_id)).map(str::trim) == Some(text.as_str()) {
            self.preview_change(&project_id, lines);
        }
        // 読んでいる間に Build now を押していた → そのまま Build（app.rs の Accept と同じ道・覚えた読みと plan を使う）
        if self.build_queued.take().is_some_and(|(p, t)| p == project_id && t == text) {
            self.new_request();
            if !self.start_edit(&project_id, &text) {
                let _ = self.page_intent(&project_id, PageIntent::Edit { text: text.clone() });
            }
        }
        ModelDone::Planned
    }

    /// W19-2 の配線: Engine の答えが届いていれば Page の 1 行と rail の実行履歴にし、`ENGINE_RUNS_FILE` に 1 行足す（毎 frame・待たない）。true = 届いた。
    /// Wave: W19-2
    /// 保証する: Ok は verified が 1 件以上の時だけ「済み」の 1 行・`Denied` で承認待ちの手が在り、その digest をこの run に渡していない → 承認の 1 行
    /// （`pending` と `later`）・`Denied` で承認では通らない（承認待ちの手が無い・同じ digest を渡したのに止められた = 人の lease・許可・同意）→ 拒否の 1 行・
    /// それ以外の Err → 理由と次の一手の 1 行（`engine_page::failure_line`）・thread が答えずに終わった → 失敗の 1 行。
    /// OPEN-INPLACE-1101 の 2（Always allow）: 承認待ちの手（`pending` と `later`）が全部 Always allow の範囲に在る（`PageEngine::skips_approval`）なら、承認の card を出さず `engine_act(Approve)` と同じ道で 1 回送る（receipt は同じ作り方・読み戻しは
    /// 省かない = Engine の verify のまま）。同じ一言の続きの run がまた承認で止まったら（覚えた範囲の外の手が出た）card を出す = 自動で 2 回送らない。
    pub fn poll_engine(&mut self) -> bool {
        use crate::ui::engine_view::{EngineView, STOPPED_LINE};
        // 待っている事の 1 行（裏の thread の `EngineRequest::progress`）を Page の「進めています」に写す（Some も None も・W19-3b G2）
        if let Some(job) = &self.engine_job {
            // ENGINE-AUDIT W1: 段の 1 行に経過秒を足す（1 行が無い間は秒だけ = 固まったのか進んでいるのかが分かる）
            let secs = job.started.elapsed().as_secs();
            let now = Some(match job.waiting.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                Some(line) => format!("{line} · {secs} s"),
                None => format!("Working… {secs} s"),
            });
            if let Some(EngineView::Running { waiting, .. }) = self.engine_views.get_mut(&job.project_id) {
                *waiting = now;
            }
        }
        let got = match self.engine_job.as_ref().map(|j| j.rx.try_recv()) {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return false,
            Some(Ok(r)) => Some(r),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => None,
        };
        let Some(job) = self.engine_job.take() else { return false };
        // ENGINE-ADJUST-1102 ①: 答えが届いた = 人の Stop の印を runtime の表から外す（`start_engine_with` の `track` の逆）
        self.rt.untrack(&job.run_id);
        self.log_engine_run(&job, got.as_ref().map_or(("error", serde_json::json!({ "kind": "lost" })), |a| a.log.clone()));
        let goal = job.goal.clone();
        let line = |state, line: String| EngineView::Line { state, line };
        let lost = "Your request stopped partway. Ask again".to_string();
        let reply = got.map_or(EngineReply::Failed { line: lost }, |a| a.reply);
        // WF-036: AI を使わずに走った事を、済みの 1 行に足す
        let no_ai = match &reply {
            EngineReply::Done { without_ai: Some(w), .. } => Some(w.clone()),
            _ => None,
        };
        // 済みの 1 行の数（録画に映る・1 だけ単数）
        let items = |n: usize| crate::domain::plural(n, "item");
        // 人が Stop を押した run（`EngineAct::Stop`）が Done 以外で閉じた = 承認・判断・同意の card も失敗の toast も出さず「Stopped by you」の 1 行
        // （Done で閉じた = 止まる前に済んだ = 済みの 1 行のまま・止めたと言わない）。Engine の答えの行は rail の履歴に残す
        let by_you = matches!(self.engine_views.get(&job.project_id), Some(EngineView::Running { stopping: true, .. })) && !matches!(reply, EngineReply::Done { .. });
        // ENGINE-AUDIT W7: 頼んだ形がもう在った（手を 1 つも打っていない）= 部品の所は ✓ と同じく読み直す（今なら読める）
        let already = matches!(reply, EngineReply::Done { already: Some(_), .. });
        let (mut view, mut lines) = match reply {
            // ENGINE-AUDIT W7: 何もしていない = ✓ でも Partial でもない（Ready = 状態の語を描かない）。verified の数より先に見る
            EngineReply::Done { already: Some(why), history, .. } => (line(RegionState::Ready, format!("Already like that · {why}")), history),
            // WF-007: 読み戻しは合ったが頼んだ差分を読んでいない = 済みと出さない（理由の 1 行）
            EngineReply::Done { partial: Some(why), history, .. } => (line(RegionState::Partial, format!("Sent, but {why}, so it isn't marked done. Look inside the app")), history),
            EngineReply::Done { verified: 0, history, .. } => {
                (line(RegionState::Partial, "Sent, but there is no way to check it in the original app, so it isn't marked done. Look inside the app".into()), history)
            }
            EngineReply::Done { verified, saved: 0, history, .. } => (line(RegionState::Verified, format!("Done. Checked {} in the original app", items(verified))), history),
            // G81: 保存した file を読んで確かめた時だけ保存を言う（窓の数と分ける）
            EngineReply::Done { verified, saved, history, .. } if verified == saved => (line(RegionState::Verified, "Done. Checked the saved file".into()), history),
            EngineReply::Done { verified, saved, history, .. } => {
                (line(RegionState::Verified, format!("Done. Checked {} in the original app and the saved file", items(verified - saved))), history)
            }
            EngineReply::Denied { steps, line: l } => {
                // 承認待ちの手の digest をこの run に渡したのに止められた = 承認では通らない（人の lease・許可・同意）→ 承認の 1 行に戻さない
                let given = steps.iter().any(|s| job.approved.iter().any(|a| a.digest == s.digest));
                if !steps.is_empty() && !given {
                    let n = steps.len();
                    (EngineView::Approve { goal: goal.clone(), steps, line: None }, vec![format!("You asked: {goal}"), format!("Waiting for approval: {n} steps (nothing has been sent yet)")])
                } else {
                    // 押せる答えの無い拒否は理由の文だけ（Ready = 状態の語を描かない・UI-6a: Needs approval は同じ行に承認の button がある時だけ・G73）
                    (line(RegionState::Ready, l.clone()), vec![format!("You asked: {goal}"), format!("Stopped: {l}")])
                }
            }
            // 同意が無くて送らなかった: 承認した手が在れば「X を AI に任せる」で同じ手を送り直せる 1 行（modal にしない）
            EngineReply::NeedsConsent { line: l } => {
                // 承認した手が無い = 押せる答えが無い拒否は理由の文だけ（Ready・Denied の腕と同じ UI-6a・G73）
                let view = if job.approved.is_empty() { line(RegionState::Ready, l.clone()) } else { EngineView::Consent { goal: goal.clone(), steps: job.approved.clone(), line: l.clone() } };
                (view, vec![format!("You asked: {goal}"), format!("Stopped: {l}")])
            }
            EngineReply::Failed { line: l } => (line(RegionState::Error, l.clone()), vec![format!("You asked: {goal}"), format!("Stopped: {l}")]),
            EngineReply::Decide { question, choices, line: l } => {
                let h = vec![format!("You asked: {goal}"), format!("Waiting for your choice: {question}")];
                (EngineView::Decide { goal: goal.clone(), question, choices, line: l }, h)
            }
            // 止まっている間に元データが変わった承認: 理由の 1 行と今の承認の card（古い承認は使っていない）
            EngineReply::Recheck { steps, line: l } => (EngineView::Approve { goal: goal.clone(), steps, line: Some(l.clone()) }, vec![format!("You asked: {goal}"), format!("Stopped: {l}")]),
        };
        if by_you {
            view = line(RegionState::Ready, STOPPED_LINE.into());
            lines.push("Stopped: by you".into());
            // 承認・判断で止まって閉じた run の行も人を待たせない（Runs に承認待ちで残さない）
            self.cancel_waiting(&job.run_id);
        }
        if let (Some(w), EngineView::Line { line, .. }) = (&no_ai, &mut view) {
            line.push_str(&format!(" · Without AI ({w})"));
        }
        // OPEN-INPLACE-1101 の 1: 部品の「Let Shibubu open it」から始めた run（終わりの 1 行は部品の所・chat は記録の 1 行）
        let at_part = self.reread_after.clone().filter(|(p, _)| *p == job.project_id);
        // OPEN-INPLACE-1101 の 2: 承認待ちの手が全部 Always allow の範囲なら card を出さず 1 回送る（この run が承認を持たない時だけ = 自動で 2 回送らない）
        let skip = match &view {
            EngineView::Approve { steps, line: None, .. } if job.approved.is_empty() => self.engine.as_ref().is_some_and(|e| e.engine.skips_approval(steps)),
            _ => false,
        };
        let lines = if skip { vec![format!("You asked: {goal}"), "Always allowed, so sent without asking".to_string()] } else { lines };
        // G86: 済み・止めた 1 行は依頼への答えとして会話に残す（承認・同意の card は残さない = 押した後の結果の 1 行が残る）。
        // NOTIFY-1101 追記（owner 192「AI の失敗も chat の中でなく toast」）: 失敗は会話に足さず toast（× まで残る）。結果の 1 行は card に描かない（`card_key` の Line = None）
        // OPEN-INPLACE-1101: 部品から始めた run は失敗も含めて Shibubu の記録の 1 行（一言 · 結果）だけを会話に（理由は部品の所に出す = toast にしない）
        // ENGINE-AUDIT W7: 「Already like that」は止めた印（Stopped）も ✓ も付けない
        let mark = |state: RegionState| if already { crate::domain::chat::Mark::None } else { mark_of(state) };
        match (&view, &at_part) {
            (EngineView::Line { state, line }, Some(_)) => self.say(&job.project_id, crate::domain::chat::Who::Shibubu, &format!("{goal} · {line}"), mark(*state), Vec::new()),
            (EngineView::Line { state: RegionState::Error, line }, None) => self.notices.push((line.clone(), true)),
            (EngineView::Line { state, line }, None) => self.say(&job.project_id, crate::domain::chat::Who::Shibubu, line, mark(*state), Vec::new()),
            _ => {}
        }
        // WF2: card が出ている間は、その run を止まった run として覚える（承認・選んだ答えで cursor から続ける）
        if matches!(view, EngineView::Approve { .. } | EngineView::Decide { .. }) {
            self.engine_waiting.insert(job.project_id.clone(), job.run_id.clone());
        } else {
            self.engine_waiting.remove(&job.project_id);
        }
        // 追記 234: この Project の直近の run を 1 行に（頼み・した事・止まった理由 = rail の行 ＋ 済みの 1 行）= 次の chat と Engine の状況
        let mut run = lines.clone();
        if let EngineView::Line { line, .. } = &view {
            if !run.iter().any(|l| l.starts_with("Stopped: ")) {
                run.push(format!("Result: {line}"));
            }
        }
        self.last_runs.insert(job.project_id.clone(), format!("{LAST_RUN}{}", run.join(" · ")));
        // TALK-HONEST-1101 追記 232: 「Let Shibubu open it」の run が ✓ で終わった = その部品を 1 回だけ読み直す（承認・判断・同意の card の間は待つ）
        // OPEN-INPLACE-1101: ✓ の後の読み直しは前の値で答えず本物を読む・他の終わりは理由の 1 行を部品の所に残す（次の read で消える）
        if let Some((p, n)) = at_part {
            match &view {
                EngineView::Line { state, .. } if *state == RegionState::Verified || already => {
                    self.next_read.insert(format!("{p}/{n}"), true);
                    self.rereads.push((p, n));
                    self.reread_after = None;
                }
                EngineView::Line { line, .. } => {
                    self.part_stops.insert(format!("{p}/{n}"), line.clone());
                    self.reread_after = None;
                }
                _ => {}
            }
        }
        let project = job.project_id.clone();
        self.engine_views.insert(job.project_id, view);
        self.engine_history.splice(0..0, lines);
        if skip {
            self.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        }
        true
    }

    /// `ENGINE_RUNS_FILE` に run 1 回を 1 行足す（書けなくても画面は止めない・rail の履歴はこの起動の間は残る）。
    fn log_engine_run(&self, job: &EngineJob, (key, value): (&'static str, Value)) {
        let Some(setup) = &self.engine else { return };
        let mut row = serde_json::json!({ "at": crate::runtime::now_rfc3339(), "goal": job.goal, "run_id": job.run_id, "consents": job.approved.len() });
        row[key] = value;
        let path = setup.data_dir.join(ENGINE_RUNS_FILE);
        let _ = std::fs::create_dir_all(&setup.data_dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "{row}");
        }
    }

    /// W19-2 の配線: Page の Engine の行で押した物。Approve = 承認待ちの手（か、同意が無くて止めた手）を `start_engine` に渡す（手ごとの `ConsentReceipt` と、
    /// 画面の操作の宛先の app の AI に任せる同意）: WF2 で止まった run が在れば同じ run_id で cursor の続きから・無ければ同じ一言を新しい run_id でもう一度。
    /// Choose(n) = 判断の card の n 番目を止まった run に渡す（承認ではない・receipt を作らない）。Cancel = 1 行を消し、止まった run を cancelled に（何も送らない）。
    /// Stop = `stop_engine` で止める印を立て、走っている run の card を「Stopping…」に（閉じたら `poll_engine` が「Stopped by you」）。
    /// Wave: W19-2
    /// 保証する: 承認待ちでも同意待ちでもない時の Approve は何もしない・承認は 1 回押すと 1 回の run だけに使う（続きの run は同じ run_id・新しい依頼は新しい run_id）。
    pub fn engine_act(&mut self, project_id: &str, act: crate::ui::engine_view::EngineAct) {
        use crate::ui::engine_view::{EngineAct, EngineView};
        // L7-AGENT: Terminal の agent の一言の card（Approve = 人が欄に打ったのと同じ道で 1 回・Cancel = 何も変えない）
        if let (Some(EngineView::Ask { .. }), EngineAct::Approve | EngineAct::Cancel) = (self.engine_views.get(project_id), act) {
            self.decide_ask(project_id, act == EngineAct::Approve);
            return;
        }
        match act {
            EngineAct::Cancel => {
                // G86: 承認・同意・判断（WF2 の Decide）の card を閉じた = 送らなかった事を会話に残す（結果の 1 行の Close は残さない）
                if matches!(self.engine_views.remove(project_id), Some(EngineView::Approve { .. } | EngineView::Consent { .. } | EngineView::Decide { .. })) {
                    self.say(project_id, crate::domain::chat::Who::Shibubu, "Not sent. Nothing was changed", crate::domain::chat::Mark::Stopped, Vec::new());
                }
                self.drop_waiting(project_id);
            }
            // OPEN-INPLACE-1101 の 2: Always allow = Approve と同じく今の手を全部送り、`grants` の宛先に `"always": [action の id]` を足す
            // （`CoreEngine::grant` が bundle を決めて `surface::always_allow` に書く）。Consent の card では Always allow を出さない（`engine_view::draw`）
            EngineAct::Approve | EngineAct::AlwaysAllow => {
                let (goal, steps) = match self.engine_views.get(project_id).cloned() {
                    Some(EngineView::Approve { goal, steps, .. } | EngineView::Consent { goal, steps, .. }) => (goal, steps),
                    _ => return,
                };
                let always = act == EngineAct::AlwaysAllow;
                // ENGINE-AUDIT W9 本体: Approve / Always allow は人の手 = 1 つの頼み（承認の後の run が AI の上限で止まらない）
                self.new_request();
                if !self.start_engine_with(project_id, &goal, steps, None, crate::domain::chat::Situation::default(), Said::You, always) {
                    let line = "Can't send now: the last request is still running. Approve again when its result appears".to_string();
                    // NOTIFY-1101 追記: 結果の 1 行は card に描かないので toast でも言う（× まで残る）
                    self.notices.push((line.clone(), true));
                    self.engine_views.insert(project_id.to_owned(), EngineView::Line { state: RegionState::Blocked, line });
                }
            }
            // WF2: 判断の card の n 番目（承認ではない・receipt を作らない）を止まった run に渡し、cursor の続きから
            EngineAct::Choose(n) => {
                let Some(EngineView::Decide { goal, question, choices, .. }) = self.engine_views.get(project_id).cloned() else { return };
                let Some(id) = choices.get(n).map(|c| c.id.clone()) else { return };
                if !self.start_engine(project_id, &goal, Vec::new(), Some(id)) {
                    let line = Some("Can't continue now: the last request is still running. Choose again when its result appears".to_string());
                    self.engine_views.insert(project_id.to_owned(), EngineView::Decide { goal, question, choices, line });
                }
            }
            // Running の card の Stop: `stop_engine` で止める印を立て（進行の 1 行は `STOPPING_LINE`）、card を run が閉じるまで「Stopping…」にする
            // （閉じた時の 1 行は `poll_engine`）。走っていない・もう終わった（false）時は card を替えない
            EngineAct::Stop => {
                if !self.stop_engine(project_id) {
                    return;
                }
                if let Some(EngineView::Running { stopping, .. }) = self.engine_views.get_mut(project_id) {
                    *stopping = true;
                }
            }
        }
    }

    /// L7-AGENT（①・図 §31b）: Terminal の `shibubu project ask` が runs 表に置いた承認待ちの一言を、開いている Project の承認の card
    /// （`EngineView::Ask`）にする（毎 frame・待たない・app.rs の `poll_model`）。true = card を出した。
    /// 読むのは別 process が DB に書いた後（`PRAGMA data_version` が変わった）か、card を閉じた・上書きされた後か、席が空かずに待った後
    /// （待った後と決めた後は `ASK_GAP` を空ける = 毎 frame runs 表を舐めない・決めた 1 click の下に次の card を出さない）。
    /// card の「誰が」は、Terminal の id がこの Project の Terminal でなければ `… outside this project`。
    /// card を出すのは Project の席が空いている時だけ（Engine・model が読んでいる・他の card が出ている間は待つ）・古い順に 1 つずつ。
    /// 保証する: card を出すだけで Project には何も当てない（当てるのは `engine_act(Approve)` = `decide_ask`）。
    /// 保証しない: 別 process が書いた時に frame が来る事（来るのは Terminal の出力 = `shibubu project ask` の 1 行で `wake_on_terminal` が起こす時）。
    /// Wave: L7
    pub fn poll_asks(&mut self) -> bool {
        use crate::ui::engine_view::EngineView;
        let version = self.store.lock().unwrap_or_else(|e| e.into_inner()).pragma("data_version").ok();
        if version != self.asks.seen {
            self.asks.seen = version;
            self.asks.dirty = true;
        }
        // 反証の直し: card が他の card（Engine の Running 等）に上書きされた Project は、出した印を外して席が空いたら出し直す（頼みを宙に浮かせない）
        let views = &self.engine_views;
        let before = self.asks.shown.len();
        self.asks.shown.retain(|p, _| matches!(views.get(p), Some(EngineView::Ask { .. })));
        if self.asks.shown.len() != before {
            self.asks.dirty = true;
        }
        if !self.asks.dirty || self.asks.next.is_some_and(|t| std::time::Instant::now() < t) {
            return false;
        }
        let (mut waited, mut shown) = (false, false);
        let projects: Vec<(String, usize)> = self.apps.iter().enumerate().map(|(i, o)| (project_id_of(&o.rt.revision().definition().id), i)).collect();
        for (project, i) in projects {
            // card が出ている Project は読み直さない（決めるか上書きされた時に `dirty` が立つ）
            if self.asks.shown.contains_key(&project) {
                continue;
            }
            let busy = self.model_job.is_some()
                || self.engine_job.is_some()
                || matches!(self.engine_views.get(&project), Some(EngineView::Running { .. } | EngineView::Approve { .. } | EngineView::Consent { .. } | EngineView::Decide { .. } | EngineView::Ask { .. }));
            if busy {
                waited = true;
                continue;
            }
            let ask = self.apps[i].rt.runs().unwrap_or_default().into_iter().find(|r| {
                r.space_id == project && r.workflow.is_none() && r.run.status == crate::domain::run::RunStatus::WaitingApproval && r.approval.as_ref().is_some_and(|a| a.action_id == crate::project::context::ASK_ACTION)
            });
            let Some(rec) = ask else { continue };
            let text = rec.input["text"].as_str().unwrap_or_default().to_owned();
            // 反証の直し: Terminal の id がこの Project の Terminal でなければそう言う（`SHIBUBU_PROJECT_ID` を替えれば別の Project に頼める）
            let mine = |id: &str| self.terminals.list(Some(&project)).iter().any(|s| s.key.session_id == id);
            let who = match &rec.run.origin {
                crate::domain::action::InvocationOrigin::Terminal { terminal_id, .. } if !mine(terminal_id) => format!("{} outside this project", rec.run.origin.who()),
                o => o.who(),
            };
            self.engine_views.insert(project.clone(), EngineView::Ask { who, text });
            self.asks.shown.insert(project, (rec.run.id, std::time::Instant::now() + ASK_GAP));
            shown = true;
        }
        self.asks.dirty = waited;
        // ponytail: 待った Project は ASK_GAP の後の frame で読み直す（frame を起こすのは他の出来事）。要れば ctx の request_repaint_after を渡す
        self.asks.next = waited.then(|| std::time::Instant::now() + ASK_GAP);
        shown
    }

    /// L7-AGENT: この Project の読みは承認した agent の一言（Project の編集だけを通す）
    fn asking(&self, project_id: &str) -> bool {
        self.asks.reading.as_ref().is_some_and(|(p, _)| p == project_id)
    }

    /// L7-AGENT: `EngineView::Ask` の card の答え。承認 = 承認の記録（`ConsentReceipt`・action_ref = `ASK_ACTION`・exact payload の hash・この run だけ）を run に書いて
    /// Running にし、人が Page の欄に打ったのと同じ道（`start_edit` が読めれば model・読めなければ `page_intent(Edit)`）で 1 回読む。結果は `finish_ask` が run に書く。
    /// 断る = 却下の記録を書いて Cancelled（Project に何も当てない）。どちらも会話に 1 行。
    /// 保証する: 承認の前に Project を変えない・1 つの承認で 1 回だけ読む（run が承認待ちでなければ読まない = 別の窓で決めた・2 度押し）・
    /// card を出してから `ASK_GAP` の間の押しは読まない（card が替わった frame の押し = 前の card への 2 度押し・card は出たまま）。
    /// 承認した時に model か Engine が読んでいる → 読まずに承認待ちのまま 1 行（席が空いたら card が戻る）。
    fn decide_ask(&mut self, project_id: &str, approved: bool) {
        use crate::domain::chat::{Mark, Who};
        use crate::domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject};
        use crate::domain::run::RunStatus;
        use crate::project::context::ASK_ACTION;
        use crate::runtime::receipt;
        use crate::ui::engine_view::EngineView;
        if self.asks.shown.get(project_id).is_some_and(|(_, from)| std::time::Instant::now() < *from) {
            return;
        }
        let Some(EngineView::Ask { who, text }) = self.engine_views.remove(project_id) else { return };
        let Some((run_id, _)) = self.asks.shown.remove(project_id) else { return };
        self.asks.dirty = true;
        self.asks.next = Some(std::time::Instant::now() + ASK_GAP);
        if approved && (self.model_job.is_some() || self.engine_job.is_some()) {
            let line = "Can't change now: the last request is still being read. The request comes back when it's done".to_string();
            self.notices.push((line.clone(), true));
            self.engine_views.insert(project_id.to_owned(), EngineView::Line { state: RegionState::Blocked, line });
            return;
        }
        // Some(card の文のままか) = run に決めた事を書いた・None = もう決まっていた / 無い
        let decided = {
            let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
            match receipt::load(&st, &run_id) {
                Ok(Some(mut rec)) if rec.run.status == RunStatus::WaitingApproval => {
                    // 反証の直し: 承認するのは card に出した文そのもの。行の文か hash が card の後に替わっていれば承認しない（Cancelled・何も読まない）
                    let same = rec.input["text"].as_str() == Some(text.as_str())
                        && rec.approval.as_ref().is_some_and(|a| a.payload_hash == crate::runtime::permission::payload_hash(ASK_ACTION, &rec.run.app_id, &rec.space_id, &rec.input, &a.scopes));
                    let yes = approved && same;
                    rec.consent = Some(ConsentReceipt {
                        id: storage::new_id(),
                        app_id: rec.run.app_id.clone(),
                        subject: ConsentSubject::Action { action_ref: ASK_ACTION.into(), run_id: Some(run_id.clone()) },
                        scopes: vec![ASK_ACTION.into()],
                        payload_digest: rec.approval.as_ref().map(|a| a.payload_hash.clone()),
                        decision: if yes { ConsentDecision::Approved } else { ConsentDecision::Denied },
                        decided_at: crate::runtime::now_rfc3339(),
                    });
                    (receipt::set_status(&mut rec, if yes { RunStatus::Running } else { RunStatus::Cancelled }).is_ok() && receipt::save(&st, &rec).is_ok()).then_some(same)
                }
                _ => None,
            }
        };
        let Some(same) = decided else {
            self.say(project_id, Who::Shibubu, &format!("{who}'s request was already answered or is gone. Nothing was changed"), Mark::Stopped, Vec::new());
            return;
        };
        if !approved {
            self.say(project_id, Who::Shibubu, &format!("Not changed. You declined {who}'s request: “{text}”"), Mark::Stopped, Vec::new());
            return;
        }
        if !same {
            self.say(project_id, Who::Shibubu, &format!("{who}'s request changed after it was shown. Nothing was changed"), Mark::Stopped, Vec::new());
            return;
        }
        self.say(project_id, Who::Shibubu, &format!("{who} asked: “{text}” · You approved"), Mark::None, Vec::new());
        // 反証の直し: 承認の範囲（ApprovalRequest の Effect::LocalWrite）は Project の編集だけ。test を走らせる一言はここで止め、
        // Engine に渡る一言は finish_edit が `asking` で止める（どちらも Failed・何も走らせない）
        if composer::project::EditVerb::read(&text) == Some(composer::project::EditVerb::RunTests) {
            self.say(project_id, Who::Shibubu, ASK_SCOPE_LINE, Mark::Stopped, Vec::new());
            self.finish_ask(&run_id, &[RunStatus::Failed]);
            return;
        }
        self.asks.reading = Some((project_id.to_owned(), run_id.clone()));
        if self.start_edit(project_id, &text) {
            return;
        }
        let r = self.page_intent(project_id, PageIntent::Edit { text });
        self.asks.reading = None;
        self.finish_ask(&run_id, if r.is_ok() { &[RunStatus::Verifying, RunStatus::Verified] } else { &[RunStatus::Failed] });
    }

    /// L7-AGENT: 承認して読ませた一言の結果を run に書く（Project を変えた = Verified・読めない / 範囲の外 = Failed・Runs の Stop で止めた = Cancelled）。
    /// 読めない・遷移できない行は触らない。Ok は `finish_edit` が Project に当てた時だけ（`asking` の間は Engine に渡さない）。
    fn finish_ask(&self, run_id: &str, to: &[crate::domain::run::RunStatus]) {
        use crate::runtime::receipt;
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(Some(mut rec)) = receipt::load(&st, run_id) else { return };
        if to.iter().all(|s| receipt::set_status(&mut rec, *s).is_ok()) {
            let _ = receipt::save(&st, &rec);
        }
    }

    /// G86: Project の会話の列（古い順・`chat_turns` から・読めなければ空）。app.rs が rail の `AgentRailSource::chat` に渡す。
    /// 書き手 = Home の作る・欄の変更・Engine の依頼と結果（`say`）。保証しない: 他の Project の会話（横断しない）。
    /// SESSIONS-LEFT-1101: 読むのはその Project の今の session（`session_key`）。Engine の 1 行・案の card・Preview も同じ session に入る。
    pub fn chat(&mut self, project_id: &str) -> Vec<crate::domain::chat::Turn> {
        self.chat_of(project_id).clone()
    }

    /// SESSIONS-LEFT-1101（owner 181「session履歴は全て左に置くように」）: 保存した会話の全部（Home の会話も Project の会話も・新しい順）=
    /// (key, 題 = 最初の人の一言の最初の 1 文, 最後に足した時刻)。key は Project の session の key（`session_project` で Project を引く）か `chat::HOME_CHAT` の key。
    /// 毎 frame 呼んでよい（写しを持ち、足す・消す・session を替えるで読み直す）。
    /// owner 210 の 4（「newsessionえらぶと…前の会話の履歴がなくなった。増えていく形で」）: Project の今の session でまだ一言の無い物（`new_session` で作った）は
    /// 一番上に `New session` と作った時刻で出す（前の session はそのまま下に残る）。一言も無いまま他の session に移ると外れる（空の session を増やさない）。
    /// 読めなければ空。保証しない: 消した Project の会話を外す事（描く側が Project の行で引く）。
    /// Wave: SESSIONS-LEFT-1101
    pub fn chat_list(&mut self) -> &[(String, String, String)] {
        let (store, fresh) = (&self.store, &self.fresh);
        self.chat_rows.get_or_insert_with(|| {
            let rows = store.lock().unwrap_or_else(|e| e.into_inner()).list_chats().unwrap_or_default();
            let saved: Vec<(String, String, String)> = rows.into_iter().map(|(key, first, at)| (key, chat_title(first.get("text").and_then(Value::as_str).unwrap_or_default()), at)).collect();
            // owner 210 の 4: 押して作ったまだ一言の無い session は一番上（作った時刻）・1 つ目の session は一言まで出さない
            let empty: Vec<(String, String, String)> = fresh.iter().filter(|(k, _)| !saved.iter().any(|(s, ..)| s == *k)).map(|(k, at)| (k.clone(), NEW_SESSION_TITLE.to_owned(), at.clone())).collect();
            empty.into_iter().chain(saved).collect()
        })
    }

    /// SESSIONS-LEFT-1101: 1 つの会話を保存からも写しからも消す（Home の会話が Project になった = 続きは Project の会話・左に同じ会話を 2 行出さない・
    /// 消した Project の session）。
    /// Wave: SESSIONS-LEFT-1101
    pub fn forget_chat(&mut self, key: &str) {
        let _ = self.store.lock().unwrap_or_else(|e| e.into_inner()).delete_chat_turns(key);
        self.chats.remove(key);
        self.chat_rows = None;
    }

    /// SESSIONS-LEFT-1101（owner 198 の 4「projectsごとにchat sessionをいくつも持てる」）: その Project の今の session の key（`chat`・`say_turn` と
    /// Engine の 1 行が読み書きする列）。選んでいなければ Project の id（1 つ目の session = 前からの会話）。2 つ目からは `{project}{chat::SESSION}{id}`。
    /// Wave: SESSIONS-LEFT-1101
    pub fn session_key(&self, project_id: &str) -> String {
        self.session.get(project_id).cloned().unwrap_or_else(|| project_id.to_owned())
    }

    /// SESSIONS-LEFT-1101: その Project の今の session を替える（左の Project の中の session の行）。`key` がその Project の物でなければ何もしない。
    /// Wave: SESSIONS-LEFT-1101
    pub fn pick_session(&mut self, project_id: &str, key: &str) {
        if key == project_id {
            self.session.remove(project_id);
        } else if crate::domain::chat::session_project(key) == Some(project_id) {
            self.session.insert(project_id.to_owned(), key.to_owned());
        }
        self.keep_session(project_id);
        self.forget_empty_sessions();
    }

    /// owner 210 の 4: 今の session でなくなった `fresh`（一言の無い session）を忘れる（一覧から外れる = 空の session を増やさない）・一覧を読み直す
    fn forget_empty_sessions(&mut self) {
        let now: Vec<&String> = self.session.values().collect();
        self.fresh.retain(|k, _| now.contains(&k));
        self.chat_rows = None;
    }

    /// SESSIONS-LEFT-1101 × owner 189（開き直したら前の session）: 今の session を ui_state に残す（1 つ目 = 行を消す）。
    /// 残せなくても続ける（再起動の後に 1 つ目へ戻るだけ・会話そのものは chat_turns に在る）。
    fn keep_session(&self, project_id: &str) {
        let Some(app) = self.app_of_project(project_id) else { return };
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let session = session_of(&app);
        let _ = match self.session.get(project_id) {
            Some(key) => {
                if st.get(Table::Sessions, &session).ok().flatten().is_none() {
                    let _ = st.put(Table::Sessions, &session, Some(&app), &serde_json::json!({ "app_id": app }));
                }
                st.put_ui_state(&session, CHAT_SESSION_KEY, &Value::String(key.clone()))
            }
            None => st.delete_ui_state(&session, CHAT_SESSION_KEY).map(|_| ()),
        };
    }

    /// SESSIONS-LEFT-1101: その Project に空の session を作って今の session にする（行は最初の一言で保存する・左の一覧には押した時から `New session` で出る =
    /// `chat_list`・owner 210 の 4）。前の今の session が空ならそれは一覧から外れる。返す = key。
    /// Wave: SESSIONS-LEFT-1101
    pub fn new_session(&mut self, project_id: &str) -> String {
        let key = format!("{project_id}{}{}", crate::domain::chat::SESSION, storage::new_id());
        self.session.insert(project_id.to_owned(), key.clone());
        self.fresh.insert(key.clone(), crate::runtime::now_rfc3339());
        self.keep_session(project_id);
        self.forget_empty_sessions();
        key
    }

    /// SESSIONS-LEFT-1101: key の会話そのもの（session を解かない = Home の会話・答えを頼んだ時の session）。
    /// Wave: SESSIONS-LEFT-1101
    pub fn chat_in(&mut self, key: &str) -> Vec<crate::domain::chat::Turn> {
        self.turns_of(key).clone()
    }

    /// SESSIONS-LEFT-1101: AI に渡す、同じ Project の他の session の要点（新しい順に 3 本まで・題と最後の 2 往復・1 行 200 字・全部で 1500 字まで）。
    /// 他の session が無ければ None。要約は AI に作らせない（呼び出しを 1 回増やさない = 利用者の枠）= 切り詰めた末尾の往復。
    /// 例（app.rs の `talk_in`）: `head.into_iter().chain(g.other_sessions(&project)).chain([said])`。
    /// Wave: SESSIONS-LEFT-1101
    pub fn other_sessions(&mut self, project_id: &str) -> Option<String> {
        use crate::domain::chat::{session_project, Who, TALK_OTHERS};
        let now = self.session_key(project_id);
        let keys: Vec<(String, String)> = self.chat_list().iter().filter(|(k, _, _)| *k != now && session_project(k) == Some(project_id)).take(3).map(|(k, t, _)| (k.clone(), t.clone())).collect();
        if keys.is_empty() {
            return None;
        }
        let mut out = format!("{TALK_OTHERS}:");
        for (key, title) in keys {
            let turns = self.chat_in(&key);
            let tail: Vec<String> = turns.iter().rev().take(4).rev().map(|t| format!("{}: {}", if t.who == Who::You { "You" } else { "Shibubu" }, brief(&t.text))).collect();
            let block = format!("\n- {title}\n  {}", tail.join("\n  "));
            if out.len() + block.len() > 1500 {
                break;
            }
            out.push_str(&block);
        }
        Some(out)
    }

    /// SESSIONS-LEFT-1101（owner 198 の 3）: 左の Project の中に並べる Page（id と題・Project の順）。開けない Project は空。
    // ponytail: 呼ぶたびに Project の行を 1 回読む（app.rs は開いた Project と検索の間だけ呼ぶ）。重くなれば Project の hash で写しを持つ
    /// 追記 224: 3 つ目 = その Page の最初の部品の出所（`source_surface` の `SurfaceTarget::App` の bundle id か、PROVIDER-ICONS-1101 追記 252 で web の部品の
    /// http(s) の URL = `theme::kind_icon` が site の icon を引く・左の sidebar の行の icon）。無ければ None（Shibubu で作った Page = Page の印）。
    /// Wave: SESSIONS-LEFT-1101
    pub fn page_list(&self, project_id: &str) -> Vec<(String, String, Option<String>)> {
        self.project(project_id)
            .map(|(_, p, _)| {
                p.pages
                    .iter()
                    .map(|pg| {
                        let app = pg.root.nodes().into_iter().filter(|n| n.component == "source_surface").filter_map(|n| n.props.get("surface_ref")?.as_str()).find_map(|id| {
                            match &p.surface_specs.iter().find(|s| s.id == id)?.target {
                                project::SurfaceTarget::App { bundle_id, .. } => Some(bundle_id.clone()),
                                project::SurfaceTarget::Url { url } => Some(url.clone()).filter(|u| u.starts_with("https://") || u.starts_with("http://")),
                            }
                        });
                        (pg.id.clone(), pg.title.clone(), app)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// SESSIONS-LEFT-1101 追記 224: 左の sidebar の Project の中の Terminal の行（panel の tab の並び = (tab の id, 名前〈217 の自動の名前〉, 今の前面の agent)）。
    /// 例（app.rs）: `let terms = pages.keys().map(|p| (p.clone(), g.term_rows(p))).collect();`
    pub fn term_rows(&mut self, project_id: &str) -> Vec<(String, String, Option<&'static crate::domain::terminal::AgentKind>)> {
        self.load_panel(project_id);
        let Some(panel) = self.panels.get(project_id) else { return Vec::new() };
        panel
            .tabs
            .iter()
            .map(|tab| {
                let key = SessionKey { project_id: project_id.to_owned(), session_id: tab.id.clone() };
                let kind = self.terminals.probe(&key).ok().and_then(|p| p.foreground).and_then(|f| crate::domain::terminal::agent_kind(&f));
                (tab.id.clone(), tab.name.clone(), kind)
            })
            .collect()
    }

    /// Project の今の session の列（`session_key` で解く）。
    fn chat_of(&mut self, project_id: &str) -> &mut Vec<crate::domain::chat::Turn> {
        let key = self.session_key(project_id);
        self.turns_of(&key)
    }

    /// key の列（初めて読む時に `chat_turns` から・読めない行は飛ばす）。
    fn turns_of(&mut self, key: &str) -> &mut Vec<crate::domain::chat::Turn> {
        let store = &self.store;
        self.chats.entry(key.to_owned()).or_insert_with(|| {
            let rows = store.lock().unwrap_or_else(|e| e.into_inner()).list_chat_turns(key).unwrap_or_default();
            rows.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
        })
    }

    /// G86: 会話に 1 つ足す（保存できなくても画面の写しには足す = この起動の間は見える）。
    fn say(&mut self, project_id: &str, who: crate::domain::chat::Who, text: &str, mark: crate::domain::chat::Mark, attachments: Vec<String>) {
        let turn = crate::domain::chat::Turn { who, text: text.to_owned(), mark, at: crate::runtime::now_rfc3339(), attachments, choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None };
        self.say_turn(project_id, turn);
    }

    /// `say` の本体（時刻と中身の決まった行をそのまま足す・Home の会話を写す時は Home で言った時刻のまま）。
    /// CHAT-ONE-1101: app.rs の `talk` も呼ぶ（Project の Chat に打った一言と AI の答え = Home の会話と同じ Turn を、この Project の列に保存する）。
    /// SESSIONS-LEFT-1101: 書くのはその Project の今の session（`session_key`）。
    pub fn say_turn(&mut self, project_id: &str, turn: crate::domain::chat::Turn) {
        let key = self.session_key(project_id);
        self.say_in(&key, turn);
    }

    /// SESSIONS-LEFT-1101: key の会話に 1 つ足す（session を解かない = Home の会話の保存・答えを頼んだ時の session に答えを足す）。
    /// Wave: SESSIONS-LEFT-1101
    pub fn say_in(&mut self, key: &str, turn: crate::domain::chat::Turn) {
        // 写しを先に読む（後で読むと今書いた行が 2 度入る）
        self.turns_of(key);
        if let Ok(json) = serde_json::to_value(&turn) {
            let _ = self.store.lock().unwrap_or_else(|e| e.into_inner()).append_chat_turn(key, &json);
        }
        self.turns_of(key).push(turn);
        self.fresh.remove(key);
        self.chat_rows = None;
    }

    /// CHAT-ONE-1101: 会話の最後の Shibubu の答えに `Preview plan` の読むだけの行を置く（画面の写しだけ・保存しない = 開き直すと Preview から）。
    pub fn preview_change(&mut self, project_id: &str, lines: Vec<String>) {
        if let Some(t) = self.chat_of(project_id).iter_mut().rev().find(|t| t.who == crate::domain::chat::Who::Shibubu) {
            t.preview = lines;
        }
    }

    /// CHAT-ONE-1101: `text` が会話の案（`pending_proposal`）そのもの = 案の card の Accept で来た変更。人の一言と案はもう会話に在るので、
    /// 変更の道（`finish_edit`・`start_engine`）は「You: …」を言い直さない（答えの 1 行だけ足す）。
    fn accepted(&mut self, project_id: &str, text: &str) -> bool {
        crate::domain::chat::pending_proposal(self.chat_of(project_id)) == Some(text.trim())
    }

    /// CHAT-ONE-1101: Project の Chat の会話を AI に渡す時の 1 行目の中身（`chat::TALK_PROJECT` の後ろ = 名前と Page の並び）。開けない Project は None。
    pub fn talk_project(&self, project_id: &str) -> Option<String> {
        let (_, p, _) = self.project(project_id).ok()?;
        Some(format!("{} · pages: {}", p.name, page_titles(&p)))
    }

    /// CHAT-ONE-1101（owner 183「choose folderをchatごと、appごとに」）: chat bar の下の folder の icon の横の名前 = この Project の folder（repo の resource・
    /// 在る folder だけ）の最後の名前。無ければ None（icon だけ）。選ぶのは `PageIntent::ChooseFolder`（TERM-ONE-1101 が同じ値を terminal の cwd に使う）。
    /// PROJECT-DIR-1101: repo が無ければその Project の自分の folder（`project_folder`・毎 frame 呼ばれるので作らずに読むだけ = `saved_folder`）。
    pub fn folder_name(&self, project_id: &str) -> Option<String> {
        let (_, p, _) = self.project(project_id).ok()?;
        let dir = self.saved_folder(&p)?;
        dir.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    /// B61-REPO: 会話の頭の `Their repo` に渡す repo の名前（repo の resource だけ・自分の folder は repo ではないので渡さない = AI に「this repo」と読ませない）。
    pub fn repo_name(&self, project_id: &str) -> Option<String> {
        let (_, p, _) = self.project(project_id).ok()?;
        let dir = existing_repo_dir(&p)?;
        Path::new(&dir).file_name().map(|n| n.to_string_lossy().into_owned())
    }

    /// NOTIFY-1101 追記: 積んだ知らせを取り出す（app.rs が毎 frame 1 回 `toast::post(ctx, 文, 失敗, None)` へ・取ったら空）。
    pub fn take_notices(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.notices)
    }

    /// 試験だけ: Project の今の card を置く（app.rs の試験が card の出方を窓の幅ごとに測る・本物は Engine と `poll_asks` が置く）
    #[cfg(test)]
    pub(crate) fn put_engine_view(&mut self, project_id: &str, view: crate::ui::engine_view::EngineView) {
        self.engine_views.insert(project_id.to_owned(), view);
    }

    /// W19-2 の配線: Page に出す Engine の 1 行（無ければ None）。
    /// Wave: W19-2
    /// OPEN-INPLACE-1101 の 1: 部品の「Let Shibubu open it」から始めた run の Running と Approve は None（その run は部品の所 = `part_run` に出す・
    /// chat の列と Page の欄の下に同じ card を 2 つ出さない）。済み・止まった後の 1 行（Line）は今まで通り。
    pub fn engine_view(&self, project_id: &str) -> Option<&crate::ui::engine_view::EngineView> {
        use crate::ui::engine_view::EngineView;
        let v = self.engine_views.get(project_id)?;
        let at_part = self.reread_after.as_ref().is_some_and(|(p, _)| p == project_id);
        (!(at_part && matches!(v, EngineView::Running { .. } | EngineView::Approve { .. } | EngineView::Consent { .. }))).then_some(v)
    }

    /// W19-2 の配線: rail の「実行履歴」に出す Engine の行（新しい run が先）。
    /// Wave: W19-2
    pub fn engine_history(&self) -> Vec<String> {
        self.engine_history.clone()
    }

    /// W2b-AX 直し: Page の Terminal の出力と shell の終わりで egui を起こす（idle の窓は frame を頼まない・src/app.rs `next_repaint`）。
    /// W13 の裏の model の答え（`start_create` / `start_edit` の thread）も同じ口で起こす（W14 の実機で見つけた: 起こさないと答えが入力まで当たらない）。
    /// 起こすのは `TERMINAL_FRAME` 後の 1 frame = 出力が流れ続けても約 30 frame/秒で、止まれば 0（chunk ごとの `request_repaint` は常時の pump に戻る）。
    /// W19a2: Engine の裏の thread（`start_engine`）も答えを送った後に同じ口で起こす（W19a2 の実機で「AI が一言を読んでいます」のまま止まった）。
    /// 使い方: 起動で 1 回（app.rs `ShibubuApp::open`）。`with_terminal_config` の後に呼ぶ（manager を替えると口も消える）。
    pub fn wake_on_terminal(&self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        // TERM-XTERM-1101: xterm の頁が在る間は待たずに起こす（打った字の echo を次の frame で頁へ・egui は grid を組み直さない）。重なった頼みは egui が 1 frame にまとめる
        self.terminals.set_wake(Arc::new(move || {
            if XTERM_ON.load(std::sync::atomic::Ordering::Relaxed) {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(TERMINAL_FRAME);
            }
        }));
    }

    /// TERM-XTERM-1101: 頁に focus が在る間に押された Shibubu の short cut（⌘K・⌘T・⌃` …）。app.rs の `raw_input_hook` が次の frame の egui の入力に足す。
    pub fn take_term_keys(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.term_views.keys)
    }

    /// TERM-XTERM-1101: frame の終わりに 1 回（app.rs・`web.sync` の後・同じ `cover` と `holes`）。この frame に置かれた Terminal（`TermViews::place`）の xterm の頁を
    /// 開く（初めて置かれた frame の 1 回）/ 矩形と穴を合わせる・`Ready` の後は `vt::seed` を 1 回書いてから出力を流す（`read_since` の続き・1 回 `TERM_WRITE_MAX` まで）・
    /// 頁の言う事を返す: 打鍵 = `write_as(Human, 置いた席)`・大きさ = `resize`・link = `open_in_browser`・右 click = 次の frame の `TermPlaced::menu`・遅れ = `TermLatency` → stderr。
    /// 置かれなかった頁は隠す（tab を替えても頁と scrollback を持つ）・manager に session が無くなった頁は閉じる・theme が替われば `set_theme`。
    /// 開けなかった session は `no_xterm` に入れて egui の grid に戻し、理由を toast に 1 回（`failed`）。
    /// 戻り値 = first responder が頁の中（呼ぶ側が egui の focus を外す・`web.sync` と同じ）。
    #[cfg(all(feature = "webview", target_os = "macos"))]
    pub fn term_sync(&mut self, ctx: &egui::Context, frame: &eframe::Frame, cover: bool, holes: &[crate::surface::Frame], egui_took_focus: bool) -> bool {
        use crate::surface::{webview::TermView, TermMsg, TERM_WRITE_MAX};
        let Generated { terminals: mgr, term_views: views, .. } = self;
        let placed = std::mem::take(&mut views.placed);
        // manager に無くなった session（閉じた tab・消した Project）の頁は閉じる
        let live: std::collections::BTreeSet<SessionKey> = mgr.list(None).into_iter().map(|i| i.key).collect();
        views.xterm.retain(|k, _| live.contains(k));
        views.no_xterm.retain(|k| live.contains(k));
        let theme = theme::term_theme(&theme::tokens(ctx));
        let mut within = false;
        // この frame に置かれなかった頁は隠す（頁と scrollback は持ったまま・tab を戻せばそのまま）
        for (_, slot) in views.xterm.iter_mut().filter(|(k, _)| !placed.iter().any(|p| &p.0 == *k)) {
            within |= slot.view.frame(None, cover, &[], false).1;
        }
        for (key, rect, shown, focus, seat) in placed {
            if !views.xterm.contains_key(&key) {
                let c = ctx.clone();
                match TermView::open(frame, &theme, rect, Box::new(move || c.request_repaint())) {
                    Ok(view) => {
                        views.xterm.insert(key.clone(), XtermSlot { view, cursor: 0, seat: seat.clone(), theme: theme.clone() });
                    }
                    Err(why) => {
                        // 次の frame から egui の grid（今まで通り）。理由は右上に 1 回
                        views.no_xterm.insert(key);
                        views.failed.push(format!("Couldn't open the terminal view ({why}), so it's drawn the old way"));
                        ctx.request_repaint();
                        continue;
                    }
                }
            }
            let Some(slot) = views.xterm.get_mut(&key) else { continue };
            slot.seat = seat;
            if slot.theme != theme {
                slot.view.set_theme(&theme);
                slot.theme = theme.clone();
            }
            let (msgs, inside) = slot.view.frame(Some((rect, shown)), cover, holes, egui_took_focus);
            within |= inside;
            if focus {
                slot.view.focus();
            }
            for m in msgs {
                // 2 つ目 = 通れば Terminal の状態の 1 行を消す手（打鍵と大きさ = `TermHost::term_do` と同じ）
                let (r, clears) = match m {
                    // 頁が出来た（初め・読み直した）: 今の画面を 1 回書き、続きは ring buffer の今の end から
                    // 頁が出来た（初め・読み直した）: 今の画面を 1 回書き、続きは ring buffer の今の end から
                    // ponytail: screen と end は別の lock = その間に来た出力は 1 回分欠けうる。困ったら manager に両方を 1 つの lock で返す口
                    TermMsg::Ready => (
                        mgr.screen(&key, HISTORY_SHOWN).map_err(|e| e.to_string()).and_then(|screen| {
                            slot.cursor = mgr.read_since(&key, u64::MAX).map_err(|e| e.to_string())?.next;
                            slot.view.write(crate::terminal::vt::seed(&screen).as_bytes())
                        }),
                        false,
                    ),
                    TermMsg::Input(bytes) => (mgr.write_as(&key, InputOwner::Human, &slot.seat, &bytes).map_err(|e| e.to_string()), true),
                    TermMsg::Size { cols, rows } => (mgr.resize(&key, TermSize { rows, cols }).map_err(|e| e.to_string()), true),
                    TermMsg::Open(url) => (crate::surface::open_in_browser(&url), false),
                    TermMsg::Menu { x, y, selection } => {
                        views.menus.insert(key.clone(), (egui::vec2(x, y), selection));
                        ctx.request_repaint();
                        (Ok(()), false)
                    }
                    TermMsg::Shown(ms) => {
                        if let Some(line) = views.latency.add(ms, std::time::Instant::now()) {
                            eprintln!("{line}");
                        }
                        (Ok(()), false)
                    }
                    TermMsg::Shortcut { key: k, modifiers } => {
                        views.keys.push(egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
                        views.keys.push(egui::Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
                        ctx.request_repaint();
                        (Ok(()), false)
                    }
                };
                // 送れない・大きさを変えられない・link を開けない理由は Terminal の状態の 1 行（grid の時と同じ所）
                match r {
                    Ok(()) if clears => {
                        views.lines.remove(&key);
                    }
                    Ok(()) => {}
                    Err(e) => {
                        views.lines.insert(key.clone(), e);
                    }
                }
            }
            if slot.view.ready {
                // 出力の続き: 1 回 TERM_WRITE_MAX まで・残りは次の frame（隠れている頁には流さない = 見せた時に追いつく）
                if let Ok(chunk) = mgr.read_since(&key, slot.cursor) {
                    if chunk.dropped > 0 {
                        // 長く隠れている間に ring buffer が先へ流れた: 抜けた byte で崩さず、頁を消して今の画面から書き直す（RIS → seed）
                        if let Ok(screen) = mgr.screen(&key, HISTORY_SHOWN) {
                            if slot.view.write(format!("\x1bc{}", crate::terminal::vt::seed(&screen)).as_bytes()).is_ok() {
                                slot.cursor = chunk.next;
                            }
                        }
                        continue;
                    }
                    let n = chunk.bytes.len().min(TERM_WRITE_MAX);
                    if n > 0 && slot.view.write(&chunk.bytes[..n]).is_ok() {
                        slot.cursor = chunk.start + n as u64;
                    }
                    if n < chunk.bytes.len() {
                        ctx.request_repaint();
                    }
                }
            }
        }
        XTERM_ON.store(!views.xterm.is_empty(), std::sync::atomic::Ordering::Relaxed);
        within
    }

    /// Page に WebView か Native Direct の面が在る間は、その poll のための frame が要る（app.rs `next_repaint`・idle の frame 0 の例外）。
    pub fn polls_surfaces(&self) -> bool {
        #[cfg(feature = "native_direct")]
        if self.surfaces.wants_frames() {
            return true;
        }
        self.web.wants_frames()
    }

    /// Project に Terminal を 1 本起こし、その記録を `terminal_sessions` に書く（YCM-020）。shell は `$SHELL`（無ければ /bin/zsh）。
    /// W5 の注: これは Project の `terminal_specs` に書かない（Page に置けない・W4 の E2E の口）。Page の Terminal は `PageIntent::AddTerminal` → `ensure_terminal`。
    /// Err は Terminal の場所に出す 1 行（何が起きたか + 次の一手・D-e）。
    pub fn open_terminal(&self, project_id: &str, working_directory: &str) -> Result<SessionInfo, String> {
        let spec = project::TerminalSessionSpec { id: storage::new_id(), project_id: project_id.into(), shell: user_shell(), working_directory: working_directory.into(), keep_alive_on_page_switch: true, command: None };
        let info = self.terminals.spawn(&spec, TermSize::default()).map_err(|e| e.to_string())?;
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        put_session(&st, &info).map_err(|e| format!("Couldn't save the terminal's history ({e}) · It will be lost when you close it"))?;
        Ok(info)
    }

    /// W5: Project の `terminal_specs` の `terminal_id` の session を返す。この起動でまだ起こしていなければ spec のまま起こし（同じ session id）、
    /// `terminal_sessions` に書く。動いている・終わった session はそのまま返す（終わった物を起こし直すのは「新しく起こす」= `restart_terminal` だけ）。
    /// TERM-RESTORE-1101: 前の起動の記録しか無い時は同じ session id の新しい shell を起こし、保存した画面をその上に描く（`ensure_spec`・agent は打たない）。
    ///
    /// 使い方: Host の `terminal(terminal_ref)` が毎 frame `g.ensure_terminal(project, terminal_ref)?` → `TerminalManager::screen`。試験は `info.pid` を `ps -o ppid=,tty=` で見る。
    /// 保証する: Page を移る・戻るは manager を呼ばない ＝ 同じ pid が `close` / `close_project` / `shutdown`（Shibubu の終了）まで続く。
    /// 再起動をまたぐと session id（`SessionKey`）は同じで pid は変わる。負の対照: 毎回起こし直す実装は pid が変わり W5 の受け入れが赤。
    /// Err は Terminal の場所に出す 1 行（Project・spec が無い・shell を起こせない・記録を書けない）。
    /// Wave: W5
    pub fn ensure_terminal(&mut self, project_id: &str, terminal_id: &str) -> Result<SessionInfo, String> {
        let spec = self.terminal_spec(project_id, terminal_id)?;
        ensure_spec(&self.terminals, &self.store, &spec)
    }

    /// W5: 終わった・前の起動の session を同じ spec で起こし直す（「新しく起こす」）。動いている session は起こし直さず Err の 1 行。
    pub fn restart_terminal(&mut self, project_id: &str, terminal_id: &str) -> Result<SessionInfo, String> {
        let spec = self.terminal_spec(project_id, terminal_id)?;
        restart_spec(&self.terminals, &self.store, &mut self.term_views, &spec)
    }

    fn terminal_spec(&self, project_id: &str, terminal_id: &str) -> Result<project::TerminalSessionSpec, String> {
        let (_, p, _) = self.project(project_id)?;
        p.terminal_specs.into_iter().find(|t| t.id == terminal_id).ok_or_else(|| "This terminal is no longer in the project · Add terminal".to_owned())
    }

    /// Shibubu の終了: 全 Terminal を閉じ（shell 0 本・zombie 0）、終わり方を記録に書く。2 回目は書き直すだけ。
    /// TERM-RESTORE-1101: 閉じる前に画面と folder を保存する（`TerminalManager::shutdown` の始め・次の起動の `ensure_spec` が描く）。
    pub fn close_terminals(&self) {
        self.terminals.shutdown();
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        for info in self.terminals.list(None) {
            let _ = put_session(&st, &info);
        }
    }

    /// G95: Project の下の panel を描く（app.rs の update が `View::Project` の時に `CentralPanel` より前に呼ぶ = Page の面は panel の分だけ縮む）。
    /// 帯（左端の開閉・常設の `Chat` の tab・Terminal の tab・状態の点・`+` の menu・split）と `chat`（CHAT-ONE-1101: Chat の tab を開いている時だけ・
    /// 会話と chat bar は app.rs が描く = Shell と提案は app.rs が持つ）、Terminal を選んで開いていれば選んだ tab の grid（panel の下端まで・TERM-ONE-1101: terminal ごとの入力欄は無い・grid に直に打つ）。
    /// 押した手はその場で `panel_act` に渡し、当てた手を返す（app.rs が Chat を開いた時に欄へ focus を置く・誤りは Page の上の 1 行）。
    /// agent が Working → Needs you / Done に変わった時、panel が畳まれているか窓が前面でなければ通知を 1 回。
    /// 段 2 作り直し × NOTIFY-1101: `setup`（app.rs の `agent_setup`・Project の口が在る起動だけ Some）が在れば、入っていない間は右上の toast 1 回
    /// （`SETUP_INTRO` と `Install`）・押した後は選んだ tab の本体の最初に一覧（`setup_line`）・入った後は何も描かない。PTY には打たない（shell の出力・agent の入力に混ざらない）。
    /// TERM-ONE-1101: 描く前に plan の Terminal を tab にする（`adopt_plan_terminals`・Page には Terminal を描かない = terminal は panel の 1 種類だけ）。
    /// HOME-TERM-1101: `project_id` = `HOME_PLACE` は Home の Terminal（Project の定義を読まない・plan の Terminal も Page も無い = `+ New` に `New browser tab` を出さない・
    /// 被せない・誤りは右上の toast）。
    /// CHAT-SIDE-1101 追記 219〜221: `side` = Some(保存した幅) の時は同じ panel を右の sidebar（`agent_rail::chat_side`）に描き、下には描かない（高さの境は無い）。
    /// 境を引き終えた幅を書き戻す。None = 下（今まで）。PANEL-SYNC-1101: 畳んでいれば下にも右にも描かない（tab は窓の一番下の帯 = `folded`）。
    pub fn draw_terminal_panel(&mut self, ctx: &egui::Context, project_id: &str, mut setup: Option<&mut AgentSetup>, chat: Option<&mut dyn FnMut(&mut egui::Ui)>, side: Option<&mut f32>) -> Option<PanelAct> {
        let home = project_id == HOME_PLACE;
        if !home {
            let Ok((_, p, _)) = self.project(project_id) else { return None };
            self.adopt_plan_terminals(project_id, &p);
        }
        let status = self.panel_status(project_id, ctx.input(|i| i.focused));
        // TERM-RESTORE-1101: 前の画面のまま戻せなかった・起こせなかった terminal（右上の toast・× で消える）
        for line in std::mem::take(&mut self.term_views.failed) {
            crate::ui::toast::post(ctx, line, true, None);
        }
        // Working の間だけ frame を頼む（出力が止まって Needs you に変わる時を拾う・idle の frame は 0 のまま）
        if status.values().any(|s| s.agent == Some(AgentState::Working)) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let main_h = ctx.available_rect().height();
        let t = theme::tokens(ctx);
        let mut act = None;
        let agents = self.term_views.agents.clone();
        // MIRROR-FIT-1101（owner 198 の 8）: 本物の app・Web の面の Page（`wide_page`）は、開いた panel で縮めずに上へ被せる（Mirror の枠も本物の窓も WebView も動かさない）
        let page = self.routers.get(project_id).map(|r| r.current().to_owned()).unwrap_or_default();
        let covers = self.wide_page(project_id, &page);
        // SESSIONS-LEFT-1101（owner 210 の 4）: 帯の Chat の tab に今の session の題（一言の無い 1 つ目の session は無し）
        let key = self.session_key(project_id);
        let title = self.chat_list().iter().find(|r| r.0 == key).map(|r| r.1.clone()).filter(|t| !t.is_empty());
        let Generated { panels, terminals, store, term_views, .. } = self;
        let Some(panel) = panels.get_mut(project_id) else { return None };
        panel.chat_title = title;
        let specs = panel.tabs.iter().map(|tab| panel_spec(project_id, tab)).collect();
        let mut host = TermHost { mgr: terminals, store, views: term_views, specs, view: InputView::Drawer, picked: None };
        // owner 210: 左右の余白（`INSET_X`）は会話だけ。帯と Terminal の本体は `terminal_panel::draw` がこの余白の外まで描く（panel の端から端まで）
        let frame = egui::Frame::new().fill(t.surface_1).inner_margin(egui::Margin::symmetric(terminal_panel::INSET_X as i8, 0));
        let h = panel.height(main_h);
        let over = covers && h > terminal_panel::STRIP_H + 0.5;
        let room = ctx.available_rect();
        let (open, tall) = (panel.open, side.is_none().then_some(main_h));
        let body = |ui: &mut egui::Ui| {
            act = terminal_panel::draw(ui, panel, tall, &status, &agents, project_id != HOME_PLACE, &mut |ui: &mut egui::Ui, tab: &terminal_panel::PanelTab| {
                // NOTIFY-1101: 入っていない Project で最初に terminal を開いた時に toast 1 回（`Install` を押すと app.rs が一覧を開く）・開いている間だけ一覧（押すまで HOME に何も書かない）
                if let Some(setup) = setup.as_deref_mut() {
                    if !setup.open && setup.line().is_none() {
                        crate::ui::toast::post_once(ui.ctx(), &format!("agent-setup:{project_id}"), SETUP_INTRO, false, Some("Install"));
                    }
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(terminal_panel::INSET_X as i8, 0)).show(ui, |ui| setup_line(ui, setup, false));
                }
                let top = ui.cursor().top();
                // 割った時は上下の半分ずつ: grid は渡された枠の下端まで（L7-ORCA #9）
                let bottom = ui.max_rect().bottom() - theme::SP_1;
                let _ = renderer::components::terminal_body(ui, &format!("panel.{}", tab.id), &tab.id, top, &mut host, t, Some(bottom));
            }, chat);
        };
        if !open {
            // PANEL-SYNC-1101（owner 248「下にしまった時は下のおびにtabを置いて」）: 畳んだ panel は下にも右にも描かない（tab は窓の一番下の帯 = `folded` →
            // `terminal_panel::folded_tabs`・押すと `Intent::OpenBelow`）。前は右に置いたまま畳むと下に何も残らず、開く口が ◨ と ⌃` だけだった（f44c22e3）
            terminal_panel::set_covering(ctx, None);
        } else if let Some(saved) = side {
            // 右の sidebar（追記 219〜221）
            if let Some(w) = crate::ui::agent_rail::chat_side(ctx, *saved, body) {
                *saved = w;
            }
            terminal_panel::set_covering(ctx, None);
        } else if over {
            // 被せる: 下には何も取らない（Page は畳んだ時のまま = PANEL-SYNC-1101 から畳んだ panel は帯の高さも取らない）・panel は不透明の地で Page の上の層に描く（押しも下の Page へ通さない）
            let r = egui::Rect::from_min_max(egui::pos2(room.left(), room.bottom() - h), room.right_bottom());
            let id = egui::Id::new("terminal-panel-over");
            egui::Area::new(id).order(egui::Order::Middle).fixed_pos(r.min).constrain(false).show(ctx, |ui| {
                ui.set_clip_rect(r);
                let _ = ui.allocate_rect(r, egui::Sense::click_and_drag());
                ui.painter().rect_filled(r, 0.0, t.surface_1);
                ui.painter().hline(r.x_range(), r.top(), ui.visuals().widgets.noninteractive.bg_stroke);
                let inner = r.shrink2(egui::vec2(theme::SP_2, 0.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)), |ui| body(ui));
            });
            // Page の中の Area（枠を決めた Browser の帯）より上に
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
            terminal_panel::set_covering(ctx, Some(r));
        } else {
            egui::TopBottomPanel::bottom(egui::Id::new("terminal-panel")).exact_height(h).resizable(false).show_separator_line(true).frame(frame).show(ctx, |ui| body(ui));
            terminal_panel::set_covering(ctx, None);
        }
        // 帯の手が無ければ grid の右 click の手（L7-ORCA #9）
        let act = act.or(host.picked.take());
        // L7-ORCA #12: `New browser tab` は今の Page に Browser を置く（Project の定義を変える = Undo できる・panel_act は定義を変えない）
        let r = match act.clone() {
            Some(PanelAct::NewBrowserTab) => {
                let page = self.routers.get(project_id).map(|r| r.current().to_owned()).unwrap_or_default();
                self.page_intent(project_id, PageIntent::AddBrowser { page }).map(|_| ())
            }
            Some(act) => self.panel_act(project_id, act),
            None => Ok(()),
        };
        // HOME-TERM-1101: Home に Page の頭の 1 行は無い = 右上の toast（次の frame）
        match r {
            Err(e) if home => self.term_views.failed.push(e),
            Err(e) => self.page_line = Some((RegionState::Error, e)),
            Ok(()) => {}
        }
        act
    }

    /// TERM-ONE-1101（owner 183「appにterminalを呼ぶのと下にterminalがある違いを作る意味がわからない」）: plan の Terminal（Project の `terminal_specs`）を
    /// 下の panel の tab にする（`PanelAct::Adopt`・tab にした id は `planned` に残る = 1 回だけ・閉じた tab は戻さない・shell は tab の cwd = 今の folder で起こす）。
    /// 名前 = spec の id `{app}.{slug}` の slug（`dev` → `Dev 1`）・slug の無い id（`AddTerminal` の新しい id）は `Terminal 1`。
    /// 呼ぶ所: `draw_terminal_panel`（毎 frame）・`AddTerminal` と `ShowTerminal`（同じ手の中で tab を選べるように）。Err は Page の上の 1 行。
    fn adopt_plan_terminals(&mut self, project_id: &str, p: &ProjectDefinitionV1) {
        let planned = self.panel(project_id).planned.clone();
        for spec in p.terminal_specs.iter().filter(|t| !planned.contains(&t.id)) {
            let slug = spec.id.rsplit_once('.').map_or("", |(_, s)| s).replace('-', " ");
            let mut c = slug.chars();
            let name = c.next().map_or_else(|| "Terminal".to_owned(), |f| f.to_uppercase().chain(c).collect());
            if let Err(e) = self.panel_act(project_id, PanelAct::Adopt { id: spec.id.clone(), name }) {
                self.page_line = Some((RegionState::Error, e));
            }
        }
    }

    /// G95: panel の手を 1 つ当てる（足す = shell を起こす・閉じる = shell を止める・開閉と高さと並び）→ `ui_state` に書く。
    /// 保証する: Project の定義・revision・居場所の Page を変えない（Undo の対象にしない）。Err は Page の上に出す 1 行。
    /// HOME-TERM-1101: `HOME_PLACE` は Project を読まず、新しい tab の cwd = `~/Shibubu`・書く先 = `data_dir` の `HOME_PANEL_FILE`（`data_dir` の無い試験は書かない）。
    pub fn panel_act(&mut self, project_id: &str, act: PanelAct) -> Result<(), String> {
        let (app, cwd) = if project_id == HOME_PLACE {
            // ~/Shibubu（無ければ作る）・`projects_root` の無い試験と作れない時は HOME
            let root = self.projects_root.as_ref().filter(|d| std::fs::create_dir_all(d).is_ok()).map(|d| d.display().to_string());
            (None, root.or_else(|| std::env::var("HOME").ok()).unwrap_or_else(|| "/".into()))
        } else {
            let (app, p, _) = self.project(project_id)?;
            let cwd = self.terminal_cwd(&p);
            (Some(app), cwd)
        };
        self.load_panel(project_id);
        let terminals = self.terminals.clone();
        // 閉じる確認: 前面に shell 以外の job（agent・build 等）が居る時だけ
        let busy = |id: &str| terminals.probe(&SessionKey { project_id: project_id.to_owned(), session_id: id.to_owned() }).is_ok_and(|pr| pr.foreground.is_some());
        let Some(panel) = self.panels.get_mut(project_id) else { return Ok(()) };
        let was_open = panel.open;
        // R1: card が開いた（`Reveal`）時は focus を動かさない（欄で打っている人の focus を Terminal へ持って行かない）・
        // TERM-ONE-1101: plan の Terminal を tab にした（`Adopt`）時も同じ（毎 frame の採用で focus を奪わない）
        let quiet = matches!(act, PanelAct::Reveal | PanelAct::Adopt { .. });
        let effect = panel.apply(act, || (storage::new_id(), cwd), busy);
        // 開いた時は選んだ tab の入力欄に focus（⌃`・Show terminal）。Chat を開いた時の欄の focus は app.rs（Shell の欄）
        let reveal = (!quiet && !was_open && panel.open && !panel.chat).then(|| panel.active_tab().map(|t| t.id.clone())).flatten();
        let json = serde_json::to_value(&*panel).map_err(|e| e.to_string())?;
        if reveal.is_some() {
            self.term_views.reveal = reveal;
        }
        match effect {
            Some(PanelEffect::Spawn(tab)) => {
                let spec = panel_spec(project_id, &tab);
                // 起こせなくても tab は残る（本体に理由と Restart を出す・毎 frame 起こし直さない）
                if let Err(e) = ensure_spec(&self.terminals, &self.store, &spec) {
                    self.term_views.lines.insert(SessionKey::of(&spec), e);
                }
                if !quiet {
                    self.term_views.reveal = Some(tab.id);
                }
            }
            Some(PanelEffect::Stop(id)) => {
                let key = SessionKey { project_id: project_id.to_owned(), session_id: id };
                if self.terminals.close(&key).is_ok() {
                    if let Some(info) = self.terminals.list(Some(project_id)).into_iter().find(|i| i.key == key) {
                        let _ = put_session(&self.store.lock().unwrap_or_else(|e| e.into_inner()), &info);
                    }
                }
                // TERM-RESTORE-1101: 閉じた tab の画面は次の起動に戻さない
                if let Some(dir) = self.terminals.restore_dir() {
                    crate::terminal::remove_snapshots(&dir, project_id, Some(&key));
                }
                self.term_views.watches.remove(&key);
                self.term_views.lines.remove(&key);
            }
            None => {}
        }
        let kept = |e: String| format!("Couldn't keep the terminal tabs ({e}) · They reset when you reopen");
        let Some(app) = app else {
            // HOME-TERM-1101: Home は sessions の行（app の FK）を持たない = data dir の file
            let Some(dir) = &self.data_dir else { return Ok(()) };
            return std::fs::write(dir.join(HOME_PANEL_FILE), json.to_string()).map_err(|e| kept(e.to_string()));
        };
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let session = session_of(&app);
        if st.get(Table::Sessions, &session).map_err(|e| kept(e.to_string()))?.is_none() {
            st.put(Table::Sessions, &session, Some(&app), &serde_json::json!({ "app_id": app })).map_err(|e| kept(e.to_string()))?;
        }
        st.put_ui_state(&session, PANEL_KEY, &json).map_err(|e| kept(e.to_string()))
    }

    /// SIGNIN-1101: panel の tab の shell の今（前面の command・終わったか）。tab の shell がこの起動に無い（閉じた・起こせなかった）は None。
    /// 例（app.rs の sign in の見張り）: `match g.probe_tab(&project, &tab) { Some(p) if p.foreground.is_some() => …, Some(p) if !p.exited => …, _ => … }`
    pub fn probe_tab(&self, project_id: &str, tab: &str) -> Option<crate::domain::terminal::AgentProbe> {
        self.terminals.probe(&SessionKey { project_id: project_id.to_owned(), session_id: tab.to_owned() }).ok()
    }

    /// G95: その Project の panel の今（読めなければ既定）。
    /// PANEL-SYNC-1101: 畳んだ panel の tab（窓の一番下の帯に並べる材料）。開いている（下でも右でも）なら None = 同じ tab を 2 か所に出さない。
    /// `place` = Project の id か `HOME_PLACE`（Chat の無い panel・＋ に `New browser tab` を出さない）。Chat の語は前の frame の題（`draw_terminal_panel` が入れる `chat_title`）。
    /// 例（app.rs の下の帯）: `folded: match &shell.view { View::Project { project, .. } => g.folded(project), View::Home => g.folded(HOME_PLACE), _ => None }`
    pub fn folded(&mut self, place: &str) -> Option<terminal_panel::Folded> {
        let panel = self.panel(place);
        if panel.open {
            return None;
        }
        let home = place == HOME_PLACE;
        let chat = (!home).then(|| terminal_panel::chat_label(panel.chat_title.as_deref()));
        Some(terminal_panel::Folded { chat, tabs: self.term_rows(place), agents: self.term_views.agents.clone(), browser: !home })
    }

    pub fn panel(&mut self, project_id: &str) -> &TerminalPanel {
        self.load_panel(project_id);
        &self.panels[project_id]
    }

    /// G95: 保存した panel を 1 回だけ読む（読めない・無い時は既定 = 畳んだ形 = 下には描かず tab は窓の一番下の帯）。tab の shell は起こさない（再起動の後は Restart の形）。
    fn load_panel(&mut self, project_id: &str) {
        if self.panels.contains_key(project_id) {
            return;
        }
        if project_id == HOME_PLACE {
            let saved = self.data_dir.as_ref().and_then(|d| std::fs::read(d.join(HOME_PANEL_FILE)).ok()).and_then(|b| serde_json::from_slice::<TerminalPanel>(&b).ok());
            // Home の chat は真ん中 = panel に Chat の席は無い（⌃` が描く前でも Terminal の席を開く）
            let panel = TerminalPanel { chat: false, ..saved.unwrap_or_default() };
            self.panels.insert(project_id.to_owned(), panel);
            return;
        }
        let saved = self.project(project_id).ok().and_then(|(app, ..)| self.store.lock().unwrap_or_else(|e| e.into_inner()).get_ui_state(&session_of(&app), PANEL_KEY).ok().flatten());
        let mut panel = saved.and_then(|v| serde_json::from_value::<TerminalPanel>(v).ok()).unwrap_or_default();
        // PROJECT-DIR-1101: 前の版の既定（HOME）で始まっていた tab は自分の folder へ（利用者が選んだ folder の tab はそのまま・次に書く時に保存される）
        let home = std::env::var("HOME").ok();
        if let Some(dir) = panel.tabs.iter().any(|t| t.cwd.is_empty() || Some(&t.cwd) == home.as_ref()).then(|| self.project_folder(project_id)).flatten() {
            for t in panel.tabs.iter_mut().filter(|t| t.cwd.is_empty() || Some(&t.cwd) == home.as_ref()) {
                t.cwd = dir.display().to_string();
            }
        }
        self.panels.insert(project_id.to_owned(), panel);
    }

    /// G95: panel の tab ごとの今（前面の process から agent の状態を進め、変わり目で通知）。`focused` = 窓が前面か。
    // ponytail: 今の Project の tab だけを見る（別の Project の agent は開くまで通知しない）。要れば全 Project の panel を回す
    fn panel_status(&mut self, project_id: &str, focused: bool) -> BTreeMap<String, TabStatus> {
        self.load_panel(project_id);
        let panel = &self.panels[project_id];
        let mut out = BTreeMap::new();
        // TITLEBAR-ICONS-1101 の 4: 自動の名前の材料（HOME は題の中で `~` に・shell 自身が前面の時の process の名前 = 起こした shell）
        let home = std::env::var("HOME").unwrap_or_default();
        let shell_name = user_shell().rsplit('/').next().unwrap_or("zsh").to_owned();
        let mut renames = Vec::new();
        for tab in &panel.tabs {
            let key = SessionKey { project_id: project_id.to_owned(), session_id: tab.id.clone() };
            let probe = self.terminals.probe(&key).ok();
            if let Some((pid, agent)) = probe.as_ref().and_then(|p| Some((p.pid?, crate::domain::terminal::agent_name(p.foreground.as_deref()?)?))) {
                self.term_views.fronts.push((pid, agent, tab.cwd.clone(), project_id.to_owned()));
            }
            let watch = self.term_views.watches.entry(key).or_default();
            if let Some(state) = probe.as_ref().and_then(|p| watch.step(p)) {
                if !panel.open || !focused {
                    notify(&tab.name, &state);
                    self.term_views.notified.push((tab.name.clone(), state));
                }
            }
            let kind = probe.as_ref().and_then(|p| p.foreground.as_deref()).and_then(crate::domain::terminal::agent_kind);
            // TITLEBAR-ICONS-1101 の 4: 動いている shell の tab だけ自動の名前に（中の題か「process · folder」・利用者が付けた名前は替えない・
            // 前の起動の tab = shell が無い = 保存した名前のまま = 再起動で戻る）
            if let Some(p) = probe.as_ref().filter(|p| !p.exited && !tab.named) {
                let process = p.foreground.as_deref().and_then(crate::domain::terminal::process_name).unwrap_or_else(|| shell_name.clone());
                let folder = Path::new(&tab.cwd).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let name = crate::domain::terminal::tab_title(p.title.as_deref(), &process, &folder, &home);
                if name != tab.name {
                    renames.push((tab.id.clone(), name));
                }
            }
            out.insert(tab.id.clone(), TabStatus { agent: watch.state.clone(), running: probe.as_ref().is_some_and(|p| !p.exited), kind });
        }
        // 替わった時だけ書く（保存は他の手と同じ `panel_act`）
        // ponytail: 前面の process が一瞬の command（`ls`）でも名前が替わる・ちらつきが気になれば前面が 1 秒続いた時だけにする
        for (tab, name) in renames {
            let _ = self.panel_act(project_id, PanelAct::Retitle { tab, name });
        }
        out
    }

    /// W12a: Project の resource を読む口を入れる（Core は `ResourceReader` だけを見る・git を知らない）。
    /// 例: `generated.set_resource_reader(Box::new(crate::adapters::AdapterRegistry::with_local_adapters()));`（app.rs の起動の時に 1 回）
    /// 保証する: 入れ替えても保存した物（ui_state・Project）は変えない。入れない間の `read_resources` は読まずに理由の 1 行を返す。
    pub fn set_resource_reader(&mut self, reader: Box<dyn ResourceReader>) {
        self.reader = Some(reader);
    }

    /// W12a: Project の `resources` を 1 つずつ `ResourceReader::read` で読み、中身を Page の binding が引く所に書く。
    /// 保存形式: 読めた時は `Observation.payload` をそのまま `ui_state(session_of(app), project::resource_source_id(resource.id))` に、
    /// 読めない時は同じ key に JSON の null を書く（前に読めた古い値を残さない＝古い SHA を今の値として描かない）。新しい表・migration は無し。
    /// 読めない間は、その resource に bind した node（`binding.source_ref == resource_source_id(id)`）の region を `(RegionState::Error, "読めていません")`
    /// にして中身も空の文も描かない（「変更した file はありません」等の空の文を、読めていない時に言わない）。読めたら外す。
    /// 読む時: `open_project`（作った直後・再起動の後を含む）・`navigate_page`（同じ Project の中で Page を移った時）・`PageIntent::Reread`（「↻ 読み直す」）。
    /// 毎 frame は読まない。uri の無い resource と、どの Page の node も bind していない resource（Dev を消した・Git の欄の無い版で保存した Project）は
    /// 読まない（Ok の行にも出さない・描く所の無い物のために git を起こさない）。
    ///
    /// 例: `let lines = g.read_resources(&project)?; // 空 = 全部読めた`
    ///
    /// 保証する: Ok は読めなかった resource ごとの 1 行（理由と次の一手・D-e・action id・serde の原文・内部の id を入れない）の並びで、
    /// draw_page は同じ行を Page の上に出す（次に読めたら消す）。reader が無い時は読まずに「この版では repo の状態を読めません」の 1 行。
    /// Err は Project を開けない・ui_state に書けない時の 1 行（呼ぶ側は移るのを止めない・page_line に出す）。
    /// 保証しない: 読んだ値が今も同じ事（読み直すまで古い）・速さ（UI の thread で Adapter の時間切れまで待つ）。
    /// Wave: W12a
    pub fn read_resources(&mut self, project_id: &str) -> Result<Vec<String>, String> {
        let (app, p, _) = self.project(project_id)?;
        let mut lines = Vec::new();
        for (r, key, bound) in readable_resources(&p) {
            let payload = match self.reader.as_mut() {
                None => Err(format!("This version can't read {}", r.kind)),
                Some(reader) => match reader.read(r) {
                    (Outcome::Verified { .. }, Some(obs)) => Ok(obs.payload),
                    (o, _) => Err(resource_line(&r.kind, &o)),
                },
            };
            {
                let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                store
                    .put_ui_state(&session_of(&app), &key, payload.as_ref().unwrap_or(&Value::Null))
                    .map_err(|e| format!("Couldn't keep {}'s status ({e}) · Refresh", r.kind))?;
            }
            // bind した node の region: 読めない間は Error（空の文を言わない）・読めたら外す（外さないと Error が残り続ける）
            if let Some(opened) = self.apps.iter_mut().find(|o| o.rt.revision().definition().id == app) {
                for k in bound {
                    match &payload {
                        Ok(_) => opened.regions.remove(&k),
                        Err(_) => opened.regions.insert(k, (RegionState::Error, "Not read yet".to_owned())),
                    };
                }
            }
            lines.extend(payload.err());
        }
        self.resource_lines.insert(project_id.to_owned(), lines.clone());
        Ok(lines)
    }

    /// 一言 → compose → 検査 → runtime.open。Ok は開く app の id。Err は Home に出す 1 行（理由と次の一手）。
    pub fn create(&mut self, goal: &str) -> Result<String, String> {
        let plan = composer::compose(goal, &new_ctx()).map_err(compose_line)?;
        let id = self.open_new(plan.into_app())?;
        self.note = None;
        Ok(id)
    }

    /// DOGFOOD-X 周 7 #6: Home から作る 1 Page の app（`create`）と、それを包んだ Project の会話の頭（Home の会話か一言・「Made …」と次の一手 = `chat_head`）。
    /// 前は Home の会話も作った事も Project の Chat に無かった（「Nothing here yet」）。
    /// 呼ぶ側の例（app.rs の `open_made` / `on_created` の 1 Page・`create_project_simple`）: `let id = g.create_in_chat(&text)?; go(View::App(id))`。
    /// 保証する: 作れなければ会話を書かない。試験の `create` は会話を書かない（今の期待のまま）。
    /// Wave: L2
    pub fn create_in_chat(&mut self, goal: &str) -> Result<String, String> {
        let id = self.create(goal)?;
        let name = self.find(&id).map(|o| self.name_of(o)).unwrap_or_else(|| goal.trim().to_owned());
        self.chat_head(&project_id_of(&id), goal.trim(), &[], &format!("Made {name}"));
        Ok(id)
    }

    /// 作った app を検査 → `runtime.open` → Apps の先頭へ（`create` と `create_project` の共通）。Ok は app の id。
    fn open_new(&mut self, app: AppDefinitionV3) -> Result<String, String> {
        let rev = AppRevision::new(app).map_err(|errs| format!("The built screen failed a check · {} · Say it another way", errs.first().map(ToString::to_string).unwrap_or_default()))?;
        let id = rev.definition().id.clone();
        let opened = self.rt.open(rev, &session_of(&id)).map_err(|e| format!("Couldn't save the project · {}", wording(e)))?;
        self.apps.insert(0, Opened::new(opened));
        Ok(id)
    }

    /// L2（review A・C）: Home の会話で案を Build now した（app.rs の `create_app` が作る前に 1 回）。`goal` = 案の文・`talk` = それまでの会話。
    /// 例: `if accepted { g.accept_plan(&text, shell.talk.clone()) }`。保証する: この文は変更の文でも `apply_home_edit` に回らない（`create_project`・
    /// `create_project_simple`）・同じ文の Project を作った時に会話を頭に写す（`save_plan`）。作れなかった時は残り、次の Build now が置き換える。
    pub fn accept_plan(&mut self, goal: &str, talk: Vec<crate::domain::chat::Turn>) {
        self.plan_talk = Some((goal.to_owned(), talk));
    }

    fn planned(&self, goal: &str) -> bool {
        self.plan_talk.as_ref().is_some_and(|(g, _)| g == goal)
    }

    /// W3: 一言 → 複数 Page の Project（`composer::project::compose_project`）。Ok(None) は「1 Page の app で足りる一言」で、呼ぶ側は今の `create` を使う。
    ///
    /// 例（app.rs の `create_app`）: `match g.create_project(&text, ws.as_deref(), bridge.as_mut())? { Some(v) => go(v), None => go(View::App(g.create(&text)?)) }`
    ///
    /// 保証する（Ok(Some) の時）:
    /// - 殻の app を `runtime.open` し、`projects` の行と `project_revisions` の revision 1（`origin = generate`・base_hash 無し・ops `[]`）を
    ///   1 回の BEGIN IMMEDIATE で書く（作った時刻は DB の既定の now = 呼んだ後。I1）。返す View は先頭の Page（Dev）
    /// - repo の場所は一言の中の `/` から始まる `.git` を持つ dir（`project::workspace_in`）が在ればそれ、無ければ引数の `workspace`（env）
    /// - `surface_specs` ごとに `project::readback` を 1 回呼び、`surface_receipts` に 1 行ずつ書く。`bridge` が None・bridge が答えない・app が無い は
    ///   `unavailable` と理由（Err にしない・Project は作る）。verified は書かない
    /// - 再起動の後も `rows()` に出て `open_project` で開ける（W2 の `project.{app}` の決め方のまま）
    ///
    /// 保証しない: 面の app が操作できる事（W16・W12W）・Terminal を起こす事（W4・W5）。Err は Home に出す 1 行（`create` と同じ言い方）。
    pub fn create_project(&mut self, goal: &str, workspace: Option<&Path>, bridge: Option<&mut Bridge>) -> Result<Option<View>, String> {
        // 一言に書いた repo の場所を env より先に使う（Finder から開いた .app は env を渡せない・反証の直し）
        let said = crate::project::workspace_in(goal);
        let ws = said.as_deref().or(workspace);
        // FX2: 前の生成の終わった 1 行（止めた・AI の失敗）は次の生成で消す
        self.model_ended = None;
        // Home の「作る」に「Page を足して」が来たら、新しい app を作らず今開いている app に当てる（Build now した案の文は当てない = review A）。
        if said_is_edit(goal) && !self.planned(goal) {
            return self.apply_home_edit(goal, ws).map(Some);
        }
        // L2: `Preview plan` で model が読んだ同じ文の案は、読み直さずにそのまま保存する（見せた Page と app が出来る）
        if let Some((_, plan)) = self.previewed.take_if(|(g, _)| g == goal) {
            return plan.map(|plan| self.save_plan(plan, bridge)).transpose();
        }
        // W13: model が在れば一言を model に読ませる（名前の表を引かない・この呼び方は答えを待つ。app.rs は裏の `start_create`）
        // TALK-1101: 待つ道（試験・CLI）には会話が無い = 答えていない問いも無い
        let read = self.llm.as_deref().map(|llm| read_goal(llm, goal, ws, &NEVER, None).0);
        let Some(plan) = self.resolve_plan(goal, ws, read)? else { return Ok(None) };
        self.save_plan(plan, bridge).map(Some)
    }

    /// FX2（FX-006・`Use simple rules`）: AI を呼ばず名前の表で作る（`create_project` の model 無しの道と同じ・Home の変更の文は `apply_home_edit`）。
    /// 直前の Home の生成が AI の失敗で終わっていれば、作った後の 1 行（`llm_line`）が AI を使わなかった事とその理由を言う
    /// （表 copy.en.md の `Built with simple rules because AI wasn't available · {reason}`）。
    /// 例（app.rs の `Intent::Build(BuildAction::UseSimpleRules)`）: `let made = g.create_project_simple(&text, ws, bridge.as_mut()); self.open_made(text, made)`
    /// 保証する: model を 1 回も呼ばない・Ok は必ず Some（1 Page の app は `create_in_chat` で作った `View::App`・周 7 #6）・作った Project の Chat に
    /// 「Built with simple rules …」を 1 行（周 7 #5）。AI が上限で止まっている時の理由は `llm_stopped`（周 7 #2）。保証しない: 一言に合う Project（名前の表の読み）。
    /// B13: 頭に写す Home の会話がもう同じ理由を言っていれば、Chat の 1 行は「Built with simple rules」だけ（`llm_line` は理由つきのまま）。
    /// Wave: FX2, L2
    pub fn create_project_simple(&mut self, goal: &str, workspace: Option<&Path>, bridge: Option<&mut Bridge>) -> Result<Option<View>, String> {
        // DOGFOOD-X 周 7 #2: Home の生成が失敗していなくても、AI が上限で止まっている（Build now が呼ばずに名前の表へ来た）ならその理由
        let why = self.model_ended.take().filter(|(state, ..)| *state == RegionState::Error).map(|(_, line, _)| line).or_else(|| self.llm_stopped());
        let said = crate::project::workspace_in(goal);
        let ws = said.as_deref().or(workspace);
        if said_is_edit(goal) && !self.planned(goal) {
            return self.apply_home_edit(goal, ws).map(Some);
        }
        // B13（REVIEW-R7 Y-b）: 写す Home の会話がもう理由を言っている（`stopped_talk` の返事）なら、Project の Chat で理由を 2 度言わない。
        // 作る前に見る（`chat_head` が `plan_talk` を取って消す）
        let told = why.as_ref().is_some_and(|w| self.plan_talk.as_ref().is_some_and(|(g, talk)| g == goal && talk.iter().any(|t| t.text.contains(w.as_str()))));
        let made = match self.resolve_plan(goal, ws, None)? {
            Some(plan) => self.save_plan(plan, bridge)?,
            // 周 7 #6: 1 Page の app もここで作り、その Project の会話の頭を書く（呼ぶ側の `create` に任せると Project の Chat が空のまま）
            None => View::App(self.create_in_chat(goal)?),
        };
        let line = match why {
            Some(line) => format!("Built with simple rules because {line}"),
            None => "Built with simple rules".into(),
        };
        // 周 7 #5: AI を使わずに作った事は、作った Project の Chat にも 1 行（`llm_line` は Home だけの契約 = UI-6a・Page には出さない）
        let project = match &made {
            View::Project { project, .. } => Some(project.clone()),
            View::App(id) => Some(project_id_of(id)),
            _ => None,
        };
        if let Some(project) = project {
            self.say(&project, crate::domain::chat::Who::Shibubu, if told { "Built with simple rules" } else { line.as_str() }, crate::domain::chat::Mark::None, Vec::new());
        }
        self.llm_fallback = Some(line);
        Ok(Some(made))
    }

    /// model の答え（None = model 無し）を Project の案にする。答えを使えなければ名前の表で続け、理由を 1 行（llm_line）。Ok(None) は 1 Page の app。
    fn resolve_plan(&mut self, goal: &str, ws: Option<&Path>, read: Option<GoalRead>) -> Result<Option<composer::project::ProjectPlan>, String> {
        match read {
            Some((Ok(plan), apps_line)) => {
                self.llm_fallback = apps_line;
                Ok(plan)
            }
            Some((Err(ComposeError::Model(why)), _)) => {
                self.llm_fallback = Some(format!("Built with simple rules because AI wasn't available · {why}"));
                composer::project::compose_project(goal, &new_ctx(), ws).map_err(compose_line)
            }
            Some((Err(e), _)) => Err(compose_line(e)),
            None => composer::project::compose_project(goal, &new_ctx(), ws).map_err(compose_line),
        }
    }

    /// LAYOUT（operator 10-31 の門 2）: この Page に本物の app・Web の面（`source_surface`）が在るか = 主領域を文の列の幅（`MAX_TEXT_COLUMN`）で止めないか。
    /// page_id の Page が無ければ先頭の Page（`draw_page` と同じ）。Project が無い → false。
    pub fn wide_page(&self, project_id: &str, page_id: &str) -> bool {
        self.project(project_id).is_ok_and(|(_, p, _)| {
            let page = p.pages.iter().find(|g| g.id == page_id).or(p.pages.first());
            page.is_some_and(|g| g.root.nodes().iter().any(|n| n.component == "source_surface"))
        })
    }

    /// Project の案を保存して開く（`create_project` の保証の後半: 殻の app・projects と revision 1・面ごとの読み戻し）。
    /// LAYOUT: 試験（app.rs の本物の殻）が model の答えの形の案を渡すので crate の中に開く。
    pub(crate) fn save_plan(&mut self, plan: composer::project::ProjectPlan, mut bridge: Option<&mut Bridge>) -> Result<View, String> {
        let picked = plan.preview.map(|v| v.picked).unwrap_or_default();
        let p = plan.project;
        self.open_new(plan.app)?;
        let saved_err = |e: storage::StoreError| format!("Couldn't save the project ({e}). Send it again");
        let json = serde_json::to_value(&p).map_err(|e| e.to_string())?;
        {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.immediate(saved_err, |store| {
                store.put(Table::Projects, &p.id, None, &json).map_err(saved_err)?; // revision の FK の親が先
                store.put_project_revision(&p.id, 1, &json, None, &serde_json::json!([]), RevisionOrigin::Generate.as_str()).map_err(saved_err)
            })?;
        }
        // 面ごとに 1 回読み戻す（W3 は「在るか」だけ。操作の効果は読まない = verified を書かない）
        for spec in &p.surface_specs {
            let page_id = page_of_surface(&p, &spec.id);
            let r = match bridge.as_deref_mut() {
                Some(b) => crate::project::readback(b, spec, &page_id, ""),
                None => crate::project::unavailable(spec, &page_id, "Screen control isn't running, so apps weren't checked"),
            };
            self.put_receipt(&p.id, &r)?;
        }
        self.note = None;
        // DOGFOOD-X ⑥: 作った物と次の一手（「Made X: X」だけで止めない）。周 2 回 1 #1: 名前は 1 回（Page が 1 枚で名前と同じなら Page の名前を重ねない）
        let made = match p.pages.as_slice() {
            [one] if one.title.trim().eq_ignore_ascii_case(p.name.trim()) => format!("Made {}", p.name),
            pages => format!("Made {} with {} {}: {}", p.name, pages.len(), if pages.len() == 1 { "page" } else { "pages" }, page_titles(&p)),
        };
        self.chat_head(&p.id, &p.goal, &picked, &made);
        Ok(View::Project { project: p.id.clone(), page: p.pages[0].id.clone() })
    }

    /// G86: 作った Project の会話の頭 = 一言と、作った物と次の一手（`made` = 「Made …」）。L2（review C）: Home の会話で Build now した案なら、その会話をそのまま頭に写す
    /// （続きとして読める）。案の card の手（選択肢・Build now）は写さない（Project の列で押せて何もしない物にしない）。案の文は Shibubu の答えの後ろに `Plan: …` の段落で残す。
    /// 書き手 = `save_plan`（Project）と `create_in_chat`（1 Page の app・周 7 #6）。
    /// TALK-1101（Build now）: `picked` = 答えずに残った問いに model が選んだ答え（その人の言葉）。在れば会話の後に `I picked: …` の 1 行（作り始めの 1 行・`Made …` の前）。
    fn chat_head(&mut self, project_id: &str, goal: &str, picked: &[String], made: &str) {
        use crate::domain::chat::{Mark, Who};
        match self.plan_talk.take().filter(|(said, _)| said == goal) {
            Some((_, talk)) => {
                // B13（REVIEW-R7 Y-b）: Home の案の Later の 1 行（「… nothing was built …」）は写さない（すぐ後の「Made …」と食い違う古い状態の行）
                for mut t in talk.into_iter().filter(|t| t.text != crate::domain::chat::TALK_WAIT && !t.text.starts_with(crate::domain::chat::LATER_SAID)) {
                    if let Some(plan) = t.proposal.take() {
                        // CHAT-FREE-1101: 答えは Markdown = 同じ行に足すと code block の閉じの ``` を壊す（後ろが全部 code になる）ので段落を分ける
                        t.text = format!("{}\n\nPlan: {plan}", t.text);
                    }
                    (t.choices, t.preview, t.sections) = (Vec::new(), Vec::new(), Vec::new());
                    self.say_turn(project_id, t);
                }
            }
            None => self.say(project_id, Who::You, goal, Mark::None, Vec::new()),
        }
        if !picked.is_empty() {
            self.say(project_id, Who::Shibubu, &format!("{}: {}", crate::domain::chat::PICKED, picked.join(" · ")), Mark::None, Vec::new());
        }
        self.say(project_id, Who::Shibubu, &format!("{made}. Next, say what to change, e.g. “add a Tests page”"), Mark::None, Vec::new());
    }

    /// W13c: この Project の今の配置の型（`project::PlacementChoice` が在ればそれ・無ければ一言に合う既定・`null` か知らない id は None = 今の並び）。
    /// 例: `let t = g.placement(&project)?; let root = composer::layout::place(&page, &p, t.as_ref());`
    /// 保証する: 何も書かない。Err は ui_state を読めない時の 1 行（呼ぶ側は型なしで描く）。
    /// Wave: W13c
    pub fn placement(&self, project_id: &str) -> Result<Option<composer::layout::PlacementTemplate>, String> {
        let (app, p, _) = self.project(project_id)?;
        let saved = self.store.lock().unwrap_or_else(|e| e.into_inner()).get_ui_state(&session_of(&app), crate::project::PLACEMENT_KEY).map_err(|e| format!("Couldn't read the layout ({e}), so pages are shown in their current order"))?;
        let all = composer::layout::bundled_templates();
        let default = composer::layout::default_template(&p.goal, &all);
        // G88a: model の置き場（席が 2 つ以上の Page）が在れば、選択の無い Project はそれで描く（置き場の無い Page は既定の型）
        let seats = composer::layout::seat_template(&p, default);
        Ok(match saved {
            None => seats.or_else(|| default.cloned()),
            // 後の版の形・知らない id は型なし（今の並び）
            Some(v) => serde_json::from_value::<crate::project::PlacementChoice>(v).ok().and_then(|c| c.template).and_then(|id| seats.into_iter().chain(all.iter().cloned()).find(|t| t.id == id)),
        })
    }

    /// W14b: 作った物（生成 app の id か Project の id）の使える検査の今（面ごと・`composer::verifier::usable` の答え）。
    /// 測る時: 生成 app は `create` の後と、この process で初めて描く時（UI の thread で数十 ms・別の session `usable.<app>` で 1 手）。
    /// Project は初めて描く時に Terminal の席を裏の thread で（別の manager の PTY・印は temp の file・後から足した席は次に描いた時に）、app の面と Web の面は描く時に receipt と WebView の今から。
    /// 保証する: 利用者の data（session `main.<app>`・repo・Terminal の記録・RunReceipt）に書かない・外に効く手を呼ばない・測る前の席は Wait。
    /// 保存しない（process ごと = run ごと。開き直すと測り直す・直しは run ごとに 1 回）。
    /// 例: `for u in g.usable_of(&project) { assert!(u.ready() || !u.line().is_empty()) }`
    /// Wave: W14b
    pub fn usable_of(&self, id: &str) -> Vec<Usable> {
        let done = self.usable.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Ok((_, p, _)) = self.project(id) else { return done.get(id).cloned().unwrap_or_default() };
        let waiting = |seat: Seat, line: &str| Usable { seat, reach: Check::Wait(line.into()), first_action: Check::Skipped, readback: Check::Skipped, fixed: false };
        let mut out = Vec::new();
        // 包んだ Project の生成 app（殻の app は action を持たないので席にしない）
        for app in &p.app_refs {
            let Some(def) = self.find(app).map(|o| o.rt.revision().definition().clone()).filter(|d| !d.actions.is_empty()) else { continue };
            match done.get(app) {
                Some(list) => out.extend(list.iter().cloned()),
                None => out.push(waiting(verifier::app_seat(&def), "Checked when you open this page")),
            }
        }
        let receipts = self.surface_receipts(id).unwrap_or_default();
        let mut probe = SurfaceProbe {
            receipts: &receipts,
            web: &self.web,
            project: id,
            #[cfg(feature = "mirror")]
            mirror: self.mirror.as_ref(),
        };
        for seat in verifier::project_seats(&p) {
            if seat.kind != SeatKind::Terminal {
                out.push(verifier::usable(&seat, &mut probe, "", &mut 0));
                continue;
            }
            match done.get(id).and_then(|l| l.iter().find(|u| u.seat.id == seat.id)) {
                Some(u) => out.push(u.clone()),
                None if self.usable_started.contains(&usable_seat_key(id, &seat.id)) => out.push(waiting(seat, "Starting the shell")),
                None => out.push(waiting(seat, "Checked when you open this page")),
            }
        }
        out
    }

    /// W14b: この process で初めて描く時に 1 回だけ測り始める（`usable_of` の doc）。生成 app の席はここで測り終え、Terminal の席は裏の thread。
    fn start_usable(&mut self, project_id: &str, p: &ProjectDefinitionV1) {
        if self.usable_started.insert(project_id.to_owned()) {
            for app in &p.app_refs {
                self.start_app_usable(app);
            }
        }
        // まだ測っていない Terminal の席だけ（毎 frame 呼ばれるので、測り始めた席は数えない）
        let started = &mut self.usable_started;
        let seats: Vec<Seat> = verifier::project_seats(p).into_iter().filter(|s| s.kind == SeatKind::Terminal && started.insert(usable_seat_key(project_id, &s.id))).collect();
        if seats.is_empty() {
            return;
        }
        let (done, config, specs, wake, id, mine) = (self.usable.clone(), self.terminals.config().clone(), p.terminal_specs.clone(), self.terminals.wake(), project_id.to_owned(), seats.clone());
        let spawned = std::thread::Builder::new().name(format!("usable-{id}")).spawn(move || {
            let seats = mine;
            let mut probe = TerminalProbe::new(config, specs);
            let mut fixes = 1;
            let mark = usable_mark();
            let list: Vec<Usable> = seats.iter().map(|s| verifier::usable(s, &mut probe, &mark, &mut fixes)).collect();
            drop(probe); // PTY を閉じ、印の file を消してから答える
            merge_usable(&mut done.lock().unwrap_or_else(|e| e.into_inner()), id, list);
            if let Some(wake) = wake {
                wake();
            }
        });
        if let Err(e) = spawned {
            let line = format!("Couldn't check the terminal ({e}) · Reopen the page to try again");
            let list = seats.into_iter().map(|seat| Usable { seat, reach: Check::Fail { need: Need::Retry, line: line.clone() }, first_action: Check::Skipped, readback: Check::Skipped, fixed: false }).collect();
            merge_usable(&mut self.usable.lock().unwrap_or_else(|e| e.into_inner()), project_id.to_owned(), list);
        }
    }

    /// W14b: 生成 app の席を測る（まだ測っていない時だけ・UI の thread・別の session `usable.<app>`）。殻の app（action 無し）は測らない。
    fn start_app_usable(&mut self, app: &str) {
        if self.usable.lock().unwrap_or_else(|e| e.into_inner()).contains_key(app) {
            return;
        }
        let Some(rev) = self.find(app).map(|o| o.rt.revision()).filter(|r| !r.definition().actions.is_empty()) else { return };
        let seat = verifier::app_seat(rev.definition());
        let mut probe = AppProbe::new(self.rt.clone(), self.store.clone(), rev);
        let u = verifier::usable(&seat, &mut probe, &usable_mark(), &mut 1);
        drop(probe); // 評価の session の中身を消す
        self.usable.lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_owned(), vec![u]);
    }

    /// PV0: 無い app の 1 問の導入の手（`PageIntent::Install` の doc）。Start = 表の 1 件と Homebrew が在り、他の導入が走っていない時だけ裏で
    /// `Homebrew::resolve`・Approve = review の時だけ裏で `Homebrew::install`・Cancel = review を閉じる・Check = Unsure の時だけ裏で `Homebrew::check`。
    /// 何も書かない（revision を足さない）。拒否は理由の 1 行の Err（`page_intent` が Ready の 1 行にする）。居場所の Page は変えない。
    /// Wave: PV0
    fn install_act(&mut self, project_id: &str, p: &ProjectDefinitionV1, seat: &str, act: InstallAct) -> Result<View, String> {
        let key = (project_id.to_owned(), seat.to_owned());
        let (.., asked) = asked_place(p, seat)?;
        let name = asked.app_name.clone().unwrap_or_default();
        let entry = acquire::entry_for(&name).ok_or_else(|| format!("Shibubu can't install {name} itself · Use the link or another answer below"))?;
        // Homebrew は 1 本ずつ（他の席・他の Project の導入が裏で走っている間は始めない）
        let busy = self.installs.iter().any(|(k, i)| *k != key && matches!(i, Install::Resolving(_) | Install::Installing(..) | Install::Checking(_)));
        let (homebrew, wake) = (self.homebrew.clone(), self.terminals.wake());
        let done = move || {
            if let Some(w) = &wake {
                w();
            }
        };
        let next = match (act, self.installs.get(&key)) {
            (InstallAct::Start, None) if homebrew.brew.is_none() => return Err(acquire::no_brew(entry)),
            (InstallAct::Start | InstallAct::Approve | InstallAct::Check, _) if busy => return Err("Wait for the other install to finish, then try again".into()),
            (InstallAct::Start, None) => Install::Resolving(std::thread::spawn(move || {
                let r = homebrew.resolve(entry);
                done();
                r
            })),
            (InstallAct::Approve, Some(Install::Review(c) | Install::Changed(c))) => {
                let c = c.clone();
                let approved = c.clone();
                Install::Installing(
                    c,
                    std::thread::spawn(move || {
                        let r = homebrew.install(&approved);
                        done();
                        r
                    }),
                )
            }
            (InstallAct::Cancel, Some(Install::Review(_) | Install::Changed(_))) => {
                self.installs.remove(&key);
                self.router(project_id, p)?;
                return Ok(self.switch(project_id, None));
            }
            (InstallAct::Check, Some(Install::Unsure(_))) => Install::Checking(std::thread::spawn(move || {
                let r = homebrew.check(entry);
                done();
                r
            })),
            // PV1（PV-006）: 表の公式の web 版を同じ席に（入れない・brew を叩かない・走っている間は出さない）
            (InstallAct::UseWeb, None | Some(Install::Unsure(_) | Install::Failed(_))) => {
                let url = entry.web.ok_or_else(|| format!("{name} has no web version here · Use another answer below"))?;
                let ops = placed_ops(p, seat, &format!("{} in the browser", entry.app_name), project::SurfaceTarget::Url { url: url.into() })?;
                let (app, _, hash) = self.project(project_id)?;
                let (next, _) = self.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit)?;
                self.installs.remove(&key);
                self.router(project_id, p)?.reconcile(&next, None);
                return Ok(self.switch(project_id, None));
            }
            // 押せる button の無い手（Review でない Approve・Unsure でない Check・走っている間の Start）は何もしない
            _ => return Err(format!("{name} is already being handled here · Use the buttons below")),
        };
        self.installs.insert(key, next);
        self.router(project_id, p)?;
        Ok(self.switch(project_id, None))
    }

    /// PV0: 裏の導入の答えが届いていれば次の今にする（毎 frame・待たない・app.rs が `poll_engine` の隣で呼ぶ）。true = 届いた。
    /// Resolution::Present・Installation::Verified → 1 問を入れた app の面に替える op（`placed_ops`）を `apply_project(.., DirectEdit)` で 1 回 →
    /// Page の上に `Compose (6 of 6) · …` の 1 行（Verified）・席の導入の今を消す。Review → review の 1 件・Refused / Failed → Failed・Unsure → Unsure。
    /// thread が答えずに終わった → Unsure（入ったか分からない）。保証しない: 面がすぐ使える事（W14b の使える検査が測る）。
    /// Wave: PV0
    pub fn poll_installs(&mut self) -> bool {
        let finished = |i: &Install| match i {
            Install::Resolving(j) => j.is_finished(),
            Install::Installing(_, j) | Install::Checking(j) => j.is_finished(),
            Install::Review(_) | Install::Changed(_) | Install::Unsure(_) | Install::Failed(_) => false,
        };
        let done: Vec<(String, String)> = self.installs.iter().filter(|(_, i)| finished(i)).map(|(k, _)| k.clone()).collect();
        for key in &done {
            let unsure = || Install::Unsure("Can't tell if it was installed · Look in Applications, then press Check again".into());
            let next = match self.installs.remove(key) {
                Some(Install::Resolving(j)) => match j.join() {
                    Ok(acquire::Resolution::Review(c)) => Some(Install::Review(c)),
                    Ok(acquire::Resolution::Present(i)) => self.place_installed(key, &i, true),
                    Ok(acquire::Resolution::Refused(line)) => Some(Install::Failed(line)),
                    // 読むだけの段の thread が落ちた: 何も入れていない
                    Err(_) => Some(Install::Failed("Couldn't check Homebrew's listing · Nothing was installed".into())),
                },
                Some(Install::Installing(_, j) | Install::Checking(j)) => match j.join() {
                    Ok(acquire::Installation::Verified(i)) => self.place_installed(key, &i, false),
                    Ok(acquire::Installation::Failed(line)) => Some(Install::Failed(line)),
                    Ok(acquire::Installation::Unsure(line)) => Some(Install::Unsure(line)),
                    Ok(acquire::Installation::Changed(c)) => Some(Install::Changed(c)),
                    Err(_) => Some(unsure()),
                },
                other => other,
            };
            if let Some(n) = next {
                self.installs.insert(key.clone(), n);
            }
        }
        !done.is_empty()
    }

    /// PV0: 読み戻した app で 1 問を面に替える（`placed_ops` を DirectEdit で 1 回）・Page の上に `Compose (6 of 6)` の 1 行。席の今は None（消える）。
    /// 当てられない（1 問がもう無い・古い hash）→ 理由の 1 行（入れた app は残る・Project は変えない）。
    fn place_installed(&mut self, (project_id, seat): &(String, String), i: &acquire::Installed, was_there: bool) -> Option<Install> {
        let placed = self.place_installed_inner(project_id, seat, i);
        let compose = acquire::Stage::Compose.row();
        self.page_line = Some(match placed {
            Ok(name) if was_there => (RegionState::Verified, format!("{compose} · {name} was already on this Mac ({}) · It's on this page now", i.version)),
            Ok(name) => (RegionState::Verified, format!("{compose} · Installed {name} {} · Checked {} ({}) · It's on this page now", i.version, acquire::tilde(&i.path.display().to_string(), &std::env::var("HOME").unwrap_or_default()), i.bundle_id)),
            Err(e) => (RegionState::Error, format!("{compose} · It's on this Mac, but couldn't be put on this page · {e}")),
        });
        None
    }

    fn place_installed_inner(&mut self, project_id: &str, seat: &str, i: &acquire::Installed) -> Result<String, String> {
        let (app, p, hash) = self.project(project_id)?;
        let (.., asked) = asked_place(&p, seat)?;
        let said = asked.app_name.clone().unwrap_or_default();
        let name = acquire::entry_for(&said).map_or(said.clone(), |e| e.app_name.to_owned());
        let ops = placed_ops(&p, seat, &name, project::SurfaceTarget::App { bundle_id: i.bundle_id.clone(), file: asked.file.clone() })?;
        let (next, _) = self.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit)?;
        self.router(project_id, &p)?.reconcile(&next, None);
        Ok(name)
    }

    /// PV0: 無い app の 1 問の 1 行と答え（`draw_usable` が描き、試験が直に読む）。(状態と 1 行の差し替え〈None = 使える検査の 1 行のまま〉, 答え〈文字・link の URL・押した時の intent〉)。
    /// 導入の今が無い席: 表に在る app なら先頭に `Install {名前} & continue`（Homebrew が在る時だけ）・link は表の公式の頁（model の `install_url` を使わない）・
    /// 表に無い app は今まで通り（link = model の https の頁・代わりに・無しで続ける）。導入の今が在る席: その段の 1 行と、その段で押せる物だけ。
    /// Wave: PV0
    fn missing_answers(&self, project_id: &str, seat: &str, name: &str, asked: &composer::project::SeatV1) -> (Option<(RegionState, String)>, Vec<(String, Option<String>, Option<PageIntent>)>) {
        use acquire::Stage;
        let entry = asked.app_name.as_deref().and_then(acquire::entry_for);
        let install = |act| Some(PageIntent::Install { seat: seat.to_owned(), act });
        let answer = |use_app| Some(PageIntent::AnswerMissingApp { seat: seat.to_owned(), use_app });
        // 前の答え（反証の直し: AddSurface の当たらない build では「代わりに」を出さない）
        let mut rest: Vec<(String, Option<String>, Option<PageIntent>)> =
            asked.instead.iter().filter(|_| ADD_SURFACE_BUILT).map(|a| (format!("Use {} instead", a.name), None, answer(Some(a.bundle_id.clone())))).collect();
        // PV1（PV-006・ACQ-03）: 表の公式の web 版（入れずに同じ席で使う・行き先の host を文字に）
        if let Some(url) = entry.and_then(|e| e.web).filter(|_| ADD_SURFACE_BUILT) {
            rest.push((format!("Use {name} in the browser ({})", crate::project::origin(url).trim_start_matches("https://")), None, install(InstallAct::UseWeb)));
        }
        rest.push((format!("Continue without {name}"), None, answer(None)));
        // 反証の直し: 行き先を押す前に分かるよう host を文字に出す。PV0: 表に在る app は表の公式の頁（model が選んだ URL を使わない）
        let url = entry.map(|e| e.official_page.to_owned()).or_else(|| asked.install_url.clone());
        let link = url.map(|url| (format!("Open the page to install {name} ({})", crate::project::origin(&url).trim_start_matches("https://")), Some(url), None));
        let working = |stage: Stage, what: String| (Some((RegionState::Loading, format!("{} · {what}", stage.row()))), Vec::new());
        match (self.installs.get(&(project_id.to_owned(), seat.to_owned())), entry) {
            // handler が在る時だけ（表の 1 件と Homebrew）
            (None, Some(_)) if ADD_SURFACE_BUILT && self.homebrew.brew.is_some() => (None, [(format!("Install {name} & continue"), None, install(InstallAct::Start))].into_iter().chain(link).chain(rest).collect()),
            (None, _) => (None, link.into_iter().chain(rest).collect()),
            (Some(Install::Resolving(_)), _) => working(Stage::Resolve, format!("Checking Homebrew's listing for {name}")),
            (Some(Install::Review(c)), _) => (Some((RegionState::Blocked, acquire::review_line(c))), vec![("Approve and install".into(), None, install(InstallAct::Approve)), ("Cancel".into(), None, install(InstallAct::Cancel))]),
            (Some(Install::Changed(c)), _) => (Some((RegionState::Blocked, acquire::changed_review_line(c))), vec![("Approve and install".into(), None, install(InstallAct::Approve)), ("Cancel".into(), None, install(InstallAct::Cancel))]),
            (Some(Install::Installing(c, _)), _) => working(Stage::ApproveInstall, format!("Installing {name} {} with Homebrew · This can take a few minutes", c.version)),
            (Some(Install::Checking(_)), _) => working(Stage::Verify, format!("Looking for {name} in Applications")),
            (Some(Install::Unsure(line)), _) => (Some((RegionState::Partial, format!("{} · {line}", Stage::Verify.row()))), [("Check again".into(), None, install(InstallAct::Check))].into_iter().chain(rest).collect()),
            (Some(Install::Failed(line)), _) => (Some((RegionState::Error, line.clone())), link.into_iter().chain(rest).collect()),
        }
    }

    /// G99c: この Project に「No folder」の行が在るか（窓に落とした folder を受けるか・app.rs が見る）。開けない Project は false。
    /// TERM-ONE-1101: 行は描かない（`shown_root`）が、受け口はこのまま（Project の中の drop は添える file と意味が重なるので広げない）。
    pub fn wants_folder(&self, project_id: &str) -> bool {
        self.project(project_id).is_ok_and(|(_, p, _)| p.pages.iter().any(|pg| !no_folder_rows(pg).is_empty()))
    }

    /// W14b: 使える検査の 1 行を、その席の Page に描く（Terminal は置いた Page・無ければ先頭・生成 app は先頭・面は置いた Page）。
    /// W13b: 無い app の席（MissingApp）は 1 行の下に答え（入れる頁の link〈https の時だけ・文字に行き先の host〉・代わりに <一覧の app>〈`ADD_SURFACE_BUILT` の
    /// build だけ〉・<名前> 無しで続ける）を置き、
    /// 押された button を `PageIntent::AnswerMissingApp` で返す（link は intent にしない）。
    fn draw_usable(&self, ui: &mut egui::Ui, p: &ProjectDefinitionV1, page: &PageDefinition) -> Option<PageIntent> {
        let mut answer = None;
        for u in self.usable_of(&p.id) {
            if usable_page(p, &u.seat) != page.id {
                continue;
            }
            let asked = (u.seat.kind == SeatKind::MissingApp).then(|| asked_node(page, &u.seat.id).map(|(_, s)| s)).flatten();
            // 答え（文字・link の URL・押した時の intent）と、導入の今が在る時の 1 行の差し替え（PV0・`missing_answers`）
            let (over, answers) = match &asked {
                Some(s) => self.missing_answers(&p.id, &u.seat.id, &u.seat.name, s),
                None => (None, Vec::new()),
            };
            let choices: Vec<(String, Option<String>)> = answers.iter().map(|(t, url, _)| (t.clone(), url.clone())).collect();
            let (state, line) = match over {
                Some((s, l)) => (Some(s), l),
                // G87: Mirror の面は面の中で状態と次の一手の button を言う。Page の上に同じ状態をもう 1 回描かない（AX の usable の group は残す = BG の oracle）
                None => (if u.seat.kind == SeatKind::Mirror { None } else { usable_state(&u, !choices.is_empty()) }, u.line()),
            };
            if let Some(i) = renderer::usable_line(ui, &u.seat.id, u.value(), state, &line, &choices) {
                if let Some(intent) = answers.get(i).and_then(|a| a.2.clone()) {
                    answer = Some(intent);
                }
            }
        }
        answer
    }

    /// W3: Project の面ごとの最新の読み戻し（`surface_receipts` の surface ごとに seq の最大・surface_specs の順）。
    /// 例: draw_page が `Host` に渡し、renderer の `ActionHost::surface_receipt` が `surface-status:<id>` を描く。
    /// 保証する: 読めない行（後の版の JSON 等）は黙って飛ばさず Err の 1 行。まだ 1 行も無い面は返さない（呼ぶ側は「未確認」と出す）。
    pub fn surface_receipts(&self, project_id: &str) -> Result<Vec<SurfaceReceipt>, String> {
        let (_, p, _) = self.project(project_id)?;
        let rows = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.list_surface_receipts(project_id, None).map_err(|e| format!("Couldn't read this page's check · {e}"))?
        };
        let mut latest: BTreeMap<String, storage::SurfaceReceiptRow> = BTreeMap::new();
        for row in rows {
            if latest.get(&row.surface_id).is_none_or(|old| old.seq < row.seq) {
                latest.insert(row.surface_id.clone(), row);
            }
        }
        p.surface_specs
            .iter()
            .filter_map(|s| latest.remove(&s.id))
            .map(|row| serde_json::from_value(row.json).map_err(|e| format!("Couldn't read the check ({e}). This version of Shibubu can't read that format")))
            .collect()
    }

    /// 面の読み戻し 1 件を `validate` に通して `surface_receipts` に足す（`create_project` と `readback_native` の共通）。Err は画面に出す 1 行。
    fn put_receipt(&self, project_id: &str, r: &SurfaceReceipt) -> Result<(), String> {
        r.validate().map_err(|e| e.to_string())?;
        let json = serde_json::to_value(r).map_err(|e| e.to_string())?;
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.put_surface_receipt(project_id, &r.surface_id, &r.page_id, r.kind.as_str(), r.state.as_str(), &json).map_err(|e| format!("Couldn't keep this page's check · {e}"))?;
        Ok(())
    }

    /// W16-2: Project の Native Direct の面 `surface_id` を読み戻し（`self.surfaces.readback`）、前の読みから変わった時だけ `surface_receipts` に 1 行足す。
    /// 中身: spec と repo の場所（`resources` の `repo`・draw_page と同じ読み方）→ `readback` → 最新の receipt（`surface_receipts`・無ければ `project::unavailable`）→
    /// `Readback::receipt` → `validate` → `put_surface_receipt`。Ok(Some) は足した行・Ok(None) は変わらず書かなかった。
    /// 呼ぶ所（本体の commit で配線）: draw_page が `native::READBACK_EVERY` ごと・`save_view_state` の hide_page の前・`activate` の show_page の後。
    /// 例: `if let Ok(Some(r)) = g.readback_native(&project, "docs") { /* 次の frame の surface-status:docs に出る */ }`
    /// 保証する: 書くのは `surface_receipts` の追記だけ（Project・revision・ui_state は書かない＝Undo の対象にならない・YCM-045）・元の app の文書と file を書かない・
    /// verified は `Readback::receipt` の 1 の時だけ。Err は 1 行（開けない Project・その id の面が無い・Native Direct でない・保存の失敗）。
    /// 保証しない: 受け入れの A1・A2（正は試験の側の `shibubu.qa.a1/1`・TextEdit の AX value）。
    #[cfg(feature = "native_direct")]
    pub fn readback_native(&mut self, project_id: &str, surface_id: &str) -> Result<Option<SurfaceReceipt>, String> {
        let (_, p, _) = self.project(project_id)?;
        let spec = p.surface_specs.iter().find(|s| s.id == surface_id).cloned().ok_or("This project doesn't have that screen. Rebuild the project")?;
        // 反証の直し: 比べる相手は窓の文書を最後に読めた読み（今の読みの前に取る）。AX の応答待ち・重なっていない読みで進めない（進めると待つ間の保存が verified にならない）
        let seen = self.surfaces.seen(project_id, surface_id);
        let (before, now) = self.surfaces.readback(project_id, &spec, repo_dir(&p).as_deref()).map_err(|e| e.to_string())?;
        let before = seen.or(before);
        let mut last = match self.surface_receipts(project_id)?.into_iter().find(|r| r.surface_id == surface_id) {
            Some(r) => r,
            None => crate::project::unavailable(&spec, &page_of_surface(&p, surface_id), "Not read back yet"),
        };
        let project::SurfaceTarget::App { bundle_id, file } = &spec.target else { return Err(crate::project::native::NativeError::NotNative.to_string()) };
        // 作った時に bridge が起きず版の無い receipt（project::unavailable）しか無いと、保存を読み戻しても verified にできない。起きている bridge に版を聞いて埋める（読むだけ）
        if last.app_version.trim().is_empty() {
            if let Some(v) = self.surfaces.app_version(bundle_id) {
                last.app_version = v;
            }
        }
        let name = file.as_deref().map(crate::project::native::file_name_of).unwrap_or("Document");
        let Some(r) = now.receipt(before.as_ref(), &last, name) else { return Ok(None) };
        self.put_receipt(project_id, &r)?;
        Ok(Some(r))
    }

    /// W16-3: Shibubu を起こし直した後、同じ pid・window_id の窓に繋ぎ直す（`NativeSurfaces::restore`）。
    /// 例: `g.open_project(&id, None)?; g.restore_native(&id)?;`
    /// 保証する: 終了した app は `Gone` の 1 行（「接続済み」「直接操作中」と出さない）・新しい窓を題名だけで取りに行かない。
    /// 保証しない: Space / 全画面で重ねられる事（`off_space`）。
    /// Wave: W16-3
    #[cfg(feature = "native_direct")]
    pub fn restore_native(&mut self, project_id: &str) -> Result<usize, String> {
        self.surfaces.restore(project_id).map_err(|e| e.to_string())
    }

    /// W16-3: 「TextEdit を右へ」の後、重ねている Dock の page_id を行き先に付け替える（窓は動かさない）。
    /// 例: `g.apply_project(.., &[MoveSurface { .. }])?; g.retarget_native(&p.id, "docs", &to_page);`
    /// 保証する: 同じ pid・window_id のまま。保証しない: Project の木が既に MoveSurface 済みである事。
    /// Wave: W16-3
    #[cfg(feature = "native_direct")]
    pub fn retarget_native(&mut self, project_id: &str, surface_id: &str, page_id: &str) {
        self.surfaces.retarget_page(project_id, surface_id, page_id);
    }

    /// W16-3: 木（「<面の名前> を右へ」の NL 編集・Undo / Redo の後）の Page に、重ねている Native Direct の Dock の page_id を合わせる（窓は動かさない）。
    #[cfg(feature = "native_direct")]
    fn retarget_natives(&mut self, project_id: &str, next: &ProjectDefinitionV1) {
        for spec in &next.surface_specs {
            if spec.backend != project::SurfaceBackendKind::NativeDirect {
                continue;
            }
            let pg = page_of_surface(next, &spec.id);
            if !pg.is_empty() {
                self.retarget_native(project_id, &spec.id, &pg);
            }
        }
    }

    /// W16-2: `page_id` の Page に置いた Native Direct の面の id（`save_view_state` の隠す前・`activate` の戻した後に読み戻す面）。開けない Project・無い Page は空。
    #[cfg(feature = "native_direct")]
    fn native_surfaces_on(&self, project_id: &str, page_id: &str) -> Vec<String> {
        let Ok((_, p, _)) = self.project(project_id) else { return Vec::new() };
        p.page(page_id).map(|page| native_specs(&p, page).into_iter().map(|s| s.id.clone()).collect()).unwrap_or_default()
    }

    /// W16-2: 面を読み戻す（失敗しても Page の移りは止めない。同じ失敗は面の Page を描く時の読み戻しが page_line に出す）。
    #[cfg(feature = "native_direct")]
    fn readback_page(&mut self, project_id: &str, page_id: &str) {
        for sid in self.native_surfaces_on(project_id, page_id) {
            let _ = self.readback_native(project_id, &sid);
        }
    }

    pub fn title(&self, id: &str) -> Option<String> {
        self.find(id).map(|o| self.name_of(o))
    }

    /// RQ2（RQ-005）: 画面の Project の名前 = 保存した Project の `name`（Rename・Undo がすぐ効く）・行が無ければ app の題。
    // ponytail: rows と header が毎 frame 呼ぶ（Project ごとに DB 1 行）。重くなったら apply_project と undo の戻り値で cache する
    fn name_of(&self, o: &Opened) -> String {
        let id = project_id_of(&o.rt.revision().definition().id);
        let row = self.store.lock().unwrap_or_else(|e| e.into_inner()).get(Table::Projects, &id).ok().flatten();
        row.and_then(|r| r.json["name"].as_str().map(str::to_owned)).unwrap_or_else(|| o.title())
    }

    /// RQ2（RQ-006・`1 needs you`）: その Project で人の手を待つ数 = Engine の承認待ち（承認・AI に任せる同意）と、直前の操作が止まった印（承認が要る・接続が要る）。
    /// ponytail: この process の中だけ（再起動の後は 0）。run の記録（run_receipts）から数えるのは Runs の画面の仕事
    fn needs_of(&self, o: &Opened) -> usize {
        use crate::ui::engine_view::EngineView;
        let project = project_id_of(&o.rt.revision().definition().id);
        let engine = matches!(self.engine_views.get(&project), Some(EngineView::Approve { .. } | EngineView::Consent { .. } | EngineView::Ask { .. }));
        let line = matches!(o.line, Some((RegionState::Blocked | RegionState::Disconnected, _)));
        usize::from(engine) + usize::from(line)
    }

    /// Project の行（SESSIONS-LEFT-1101 から左の sidebar の Your projects・新しい順）。RQ2: `needs` = その Project で利用者を待つ run の数（承認待ち・質問）をここで数える（本体の前は 0）。
    /// `pinned` / `archived` / `sample` / `opened_at` は `UiSettings`（`pinned_projects` 等）から app.rs が重ねる（Generated は設定を持たない）。並びは `sidebar::project_order`。
    /// Wave: UI-4a, RQ2
    pub fn rows(&self) -> Vec<AppRow> {
        self.apps
            .iter()
            .map(|o| {
                let def = o.rt.revision().definition().clone();
                let reg = o.rt.runtime().registry();
                // composer の文（「準備完了」等）は runtime が持たない操作があれば名乗らせない
                let missing = def.actions.iter().filter(|a| reg.get(&a.id).is_none()).count();
                let status = o.line.as_ref().map(|(_, line)| line.clone()).unwrap_or_else(|| match missing {
                    0 => def.state.status_message.clone().unwrap_or_default(),
                    1 => "1 action not set up".to_owned(),
                    n => format!("{n} actions not set up"),
                });
                AppRow { view: View::App(def.id.clone()), name: self.name_of(o), purpose: def.goal.literal_goal.clone(), status, running: 0, needs: self.needs_of(o), pinned: false, archived: false, opened_at: None, sample: false }
            })
            .collect()
    }

    /// `View::App(id)` の中身。無い id（消えた・開けなかった）は Empty の 1 行と Apps への道。戻り値は Apps へ戻るを押したか。
    /// 契約（UI-4a から・署名は変えない）: 道は Home（button の文は表の `Go to Home`・copy.en.md:87・G14 B8 の 63）。戻り値 true = それを押した → 呼ぶ側は `Go(View::Home)`。
    pub fn draw(&mut self, ui: &mut egui::Ui, id: &str) -> bool {
        let Some((def, state, mut host)) = self.open_app(id) else {
            return states::region(ui, RegionState::Empty, "This project couldn't be found.", Some("Go to Home"));
        };
        // 押した結果の 1 行は持っている状態で描く（承認待ち・送ったか分からない・確かめていないを × Failed と言わない = 60.3）
        if let Some((state, line)) = host.line.as_ref() {
            states::inline(ui, *state, line);
        }
        renderer::render(ui, &def, state, &mut host);
        // W14b: 使える検査（この process で初めて描いた時に測る）
        self.start_app_usable(id);
        for u in self.usable_of(id) {
            renderer::usable_line(ui, &u.seat.id, u.value(), usable_state(&u, false), &u.line(), &[]);
        }
        false
    }

    /// 開いている app 1 つの定義・画面の状態・runtime への口（`draw` と、画面を介さない通しの試験 tests/yc_demo_e2e.rs が使う）。
    pub fn open_app(&mut self, id: &str) -> Option<(AppDefinitionV3, &mut AppState, Host<'_>)> {
        let Self { apps, web, usage_log, .. } = self;
        let app = apps.iter_mut().find(|o| o.rt.revision().definition().id == id)?;
        let def = app.rt.revision().definition().clone();
        Some((def, &mut app.state, Host { rt: &app.rt, regions: &mut app.regions, line: &mut app.line, receipts: Vec::new(), term: None, portals: BTreeMap::new(), placed: Vec::new(), surfaces: Vec::new(), project: String::new(), web: Some(web), takes: None, usage: usage_log.as_mut() }))
    }

    /// `open_app` に Project の Terminal の口（W5・W11 の席 `view`）と Page の面（`PageSurfaces`・Mirror は W16M-1）を足した物（`draw_page` だけ）。
    /// field を分けて借りる（Host が apps の 1 つを借りたまま manager を引き、同時に self.mirror を renderer へ渡す）。
    fn open_page_app<'a>(
        &'a mut self,
        id: &str,
        specs: Vec<project::TerminalSessionSpec>,
        surface_specs: &'a [project::SurfaceSpec],
        view: InputView,
    ) -> Option<(AppDefinitionV3, &'a mut AppState, Host<'a>, renderer::PageSurfaces<'a>)> {
        let Generated { apps, terminals, store, term_views, web, takes, usage_log, #[cfg(feature = "mirror")] mirror, .. } = self;
        let app = apps.iter_mut().find(|o| o.rt.revision().definition().id == id)?;
        let def = app.rt.revision().definition().clone();
        let term = Some(TermHost { mgr: terminals, store, views: term_views, specs, view, picked: None });
        let page_surfaces = renderer::PageSurfaces {
            specs: surface_specs,
            #[cfg(feature = "mirror")]
            mirror: mirror.as_mut(),
        };
        let takes = Some(TakeHost { slots: takes, feeds: BTreeMap::new(), reached: Vec::new() });
        Some((def, &mut app.state, Host { rt: &app.rt, regions: &mut app.regions, line: &mut app.line, receipts: Vec::new(), term, portals: BTreeMap::new(), placed: Vec::new(), surfaces: Vec::new(), project: String::new(), web: Some(web), takes, usage: usage_log.as_mut() }, page_surfaces))
    }

    /// Runs の行: 開いている app の run（runtime の RunRecord）を新しい順に（対応は docs/diagrams.md §22 の下）。effect は承認の要求 → app の宣言 → 無ければ local の書込。
    /// 契約（FX2・FX-007）: この起動の Home の生成も 1 件ずつ行にする（id = `model_run_id`・what = `Create from your request`・app = `Shibubu`・
    /// effect = LocalWrite・status = 生成中 Running／止めた Cancelled／AI の失敗 Failed〈attempt の outcome に `run_error` の語〉／作れた Verified〈Project が在る〉・
    /// 反証の直し: 止めた・AI の失敗の行は undo = NotNeeded・生成の行は connect = false〈Connections に AI の行が無い〉）。
    /// 生成の行は保存しない（再起動で消える・記録は data dir の `llm-calls.jsonl` の `CallRecord`）。
    /// Wave: FX2
    pub fn run_entries(&self) -> Vec<RunEntry> {
        let mut out: Vec<RunEntry> = Vec::new();
        for o in &self.apps {
            let rev = o.rt.revision();
            let def = rev.definition();
            // WF1: Engine の手順の run（`workflow` が在る行）は Engine の行と rail が持つ（action 1 回の run の表には出さない・FX7 で Runs に足す）
            for rec in o.rt.runs().unwrap_or_default().into_iter().filter(|r| r.workflow.is_none()) {
                let step = rec.run.steps.last();
                let action = step.map(|s| s.action_ref.clone()).unwrap_or_default();
                let effect = rec.approval.as_ref().map(|r| r.effect).or_else(|| def.action(&action).map(|a| a.effect)).unwrap_or(crate::domain::action::Effect::LocalWrite);
                out.push(RunEntry {
                    id: rec.run.id.clone(),
                    what: action,
                    app: o.title(),
                    at: rec.run.updated_at.clone(),
                    status: rec.run.status,
                    effect,
                    target: rec.target.clone(),
                    undo: Undo::conservative(effect),
                    attempts: step.map(|s| s.attempts.clone()).unwrap_or_default(),
                    connect: true,
                    project: Some(project_id_of(&def.id)),
                    // FX-039: 止めるよう頼んだ走っている run（記録に `CancelRequested` が在る）は `Stopping`
                    started_by: rec.run.origin.who(),
                    stopping: rec.run.status == crate::domain::run::RunStatus::Running && o.rt.runtime().run_log(&rec.run.id).is_ok_and(|log| log.iter().any(|l| l.event == "CancelRequested")),
                });
            }
        }
        out.extend(self.model_runs.iter().cloned());
        // FX7（FX-038）: 送ったか分からない run を先頭に（見つけられずに送り直したり放置したりしない）・あとは新しい順
        let unknown = |e: &RunEntry| e.result() == crate::ui::runs::RunResult::Unknown;
        out.sort_by(|a, b| unknown(b).cmp(&unknown(a)).then_with(|| b.at.cmp(&a.at)));
        out
    }

    /// FX2（FX-007）: Runs の `Stop` が Home の生成の行なら、`stop_model_job` で止めて詳細の下の 1 行を返す（runtime の run ではない = `run_action` は止められない）。
    /// None = 今の生成の行ではない（呼ぶ側は今の `run_action`）。
    /// 例（app.rs の View::Runs）: `let line = g.stop_from_runs(&a).unwrap_or_else(|| g.run_action(&a).1)`
    /// Wave: FX2
    pub fn stop_from_runs(&mut self, a: &RunsAction) -> Option<String> {
        // L7-AGENT（反証の直し）: 承認して model が読んでいる agent の一言（runtime の executor の run ではない = cancel の印が届かない）
        if a.action == RunAction::Cancel && self.asks.reading.as_ref().is_some_and(|(_, r)| *r == a.run_id) && self.model_job.is_some() {
            self.asks.stop = true;
            return Some("Stopping… The change won't be made".to_owned());
        }
        let live = a.action == RunAction::Cancel && self.model_job.is_some() && self.model_run_id() == Some(a.run_id.as_str());
        live.then(|| {
            self.stop_model_job();
            "Stopping the AI…".to_owned()
        })
    }

    /// Runs で押した次の一手。止めるは runtime の cancel（承認待ちも）、接続は Connections へ（true）。
    pub fn run_action(&self, a: &RunsAction) -> (bool, String) {
        match a.action {
            RunAction::Connect => (true, String::new()),
            // 行き先は呼ぶ側（app.rs が `open_project`）が決める
            RunAction::OpenProject => (false, String::new()),
            // FX7（FX-037）: 記録を読み直す。分かった（別の process が書いた）なら今の結果を、まだなら相手の app で確かめる道を言う。送り直さない
            RunAction::CheckAgain => (
                false,
                match self.rt.run(&a.run_id) {
                    Ok(Some(rec)) => {
                        let outcome = rec.run.steps.last().and_then(|s| s.attempts.last()).and_then(|t| t.outcome.clone());
                        let now = crate::ui::runs::RunResult::of(rec.run.status, outcome.as_ref());
                        if now == crate::ui::runs::RunResult::Unknown {
                            let place = if rec.target.trim().is_empty() { "the app".to_owned() } else { rec.target.clone() };
                            format!("Still not confirmed. Check {place} to see whether it arrived. Shibubu won't send it again.")
                        } else {
                            format!("Checked again: {}.", now.wording())
                        }
                    }
                    Ok(None) => "That run isn't in the history any more.".into(),
                    Err(e) => format!("Couldn't check it · {}", wording(e)),
                },
            ),
            RunAction::Cancel => (
                false,
                match self.rt.cancel(&a.run_id) {
                    Ok(CancelResult::Cancelled) => "Stopped before anything changed.".into(),
                    Ok(CancelResult::Requested) => "Stopping… It stops if nothing was sent yet. If it already was, the result stays.".into(),
                    Ok(CancelResult::TooLate(_)) => "Too late to stop. It's already checking the result.".into(),
                    Err(e) => format!("Couldn't stop it · {}", wording(e)),
                },
            ),
        }
    }

    /// app の Project と、その hash（次の差分の base_hash）。保存した Project が在ればそれ、無ければ Page 1 枚に包んだ物（YCM-011・
    /// 包んだだけでは何も書かない = apps / app_revisions も projects も変えない）。開いていない app は Ok(None)。
    /// 保存した行が読めない（store の失敗・壊れた / 後の版の JSON）は「開いていない」と混ぜず、理由の 1 行を Err で返す。
    pub fn project_of(&self, app_id: &str) -> Result<Option<(ProjectDefinitionV1, String)>, String> {
        let Some(opened) = self.find(app_id) else { return Ok(None) };
        let wrapped = ProjectDefinitionV1::wrap(opened.rt.revision().definition());
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let p = match store.get(Table::Projects, &wrapped.id).map_err(read_err)? {
            Some(row) => saved_project(row.json)?,
            None => wrapped,
        };
        let hash = project_hash(&p);
        Ok(Some((p, hash)))
    }

    /// Project に差分を当てて revision を 1 つ足す（YCM-016・043）。`base_hash` が今の Project の hash と違えば何も書かずに拒否する
    /// （古い版に向けた差分で後の変更を消さない）。包んだだけの Project に初めて当てる時は、包んだ形を revision 1 として先に残す。
    /// 読む・照合する・書くは 1 回の BEGIN IMMEDIATE の中（別 process の writer を挟ませない）。Ok は新しい Project と hash。
    /// W8: 書く前に `store.truncate_project_revisions(&id, 今の revision)` を呼ぶ（Undo の後なら Redo の行き先を消す・最後の行に居れば 0 行）。
    /// これが無いと Undo の後の編集が `(project_id, revision)` の PK で落ちる（本体の負の対照）。
    pub fn apply_project(&self, app_id: &str, base_hash: &str, ops: &[ProjectPatchOp], origin: RevisionOrigin) -> Result<(ProjectDefinitionV1, String), String> {
        let def = self.find(app_id).ok_or("This project isn't open. Reopen it from Home")?.rt.revision().definition().clone();
        let wrapped = ProjectDefinitionV1::wrap(&def);
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let saved_err = |e: storage::StoreError| format!("Couldn't save the project ({e}). Try again");
        store.immediate(saved_err, |store| {
            let saved = store.get(Table::Projects, &wrapped.id).map_err(saved_err)?;
            let current: ProjectDefinitionV1 = match &saved {
                Some(row) => saved_project(row.json.clone())?,
                None => wrapped.clone(),
            };
            let current_hash = project_hash(&current);
            if current_hash != base_hash {
                return Err("This project was changed somewhere else. Reopen it, then apply your change again".to_string());
            }
            let defs: Vec<_> = current.pages.iter().filter_map(|pg| self.find(pg.app_ref.as_deref()?)).map(|o| o.rt.revision().definition().clone()).collect();
            let next = project::apply(&current, ops, &defs.iter().collect::<Vec<_>>()).map_err(|e| e.to_string())?;
            let ops_json = serde_json::to_value(ops).map_err(|e| e.to_string())?;
            let json = serde_json::to_value(&next).map_err(|e| e.to_string())?;
            store.put(Table::Projects, &next.id, None, &json).map_err(saved_err)?; // revision の FK の親が先
            if saved.is_none() {
                let first = serde_json::to_value(&current).map_err(|e| e.to_string())?;
                store.put_project_revision(&current.id, current.revision as i64, &first, None, &serde_json::json!([]), RevisionOrigin::Generate.as_str()).map_err(saved_err)?;
            }
            store.truncate_project_revisions(&current.id, current.revision as i64).map_err(saved_err)?;
            store.put_project_revision(&next.id, next.revision as i64, &json, Some(&current_hash), &ops_json, origin.as_str()).map_err(saved_err)?;
            let hash = project_hash(&next);
            Ok((next, hash))
        })
    }

    /// 古い入口 `View::App(app_id)` を、包んだ Project の先頭 Page に変える（YCM-011）。開いていない app は None。
    ///
    /// 使い方: app.rs の `Intent::Go(View::App(id))` で `if let Some(v) = g.project_view_of(&id) { g.open_project(..) }`。
    /// 保証する: 何も書かない（`project_of` と同じく、包んだだけでは apps / projects を変えない）。保存した Project が在ればその先頭 Page。
    pub fn project_view_of(&self, app_id: &str) -> Option<View> {
        self.find(app_id)?;
        Some(match self.project_of(app_id) {
            Ok(Some((p, _))) => View::Project { page: p.pages[0].id.clone(), project: p.id },
            // 読めない保存は open_project が理由の 1 行で返す（古い View::App の描き方へ黙って落とさない）
            _ => View::Project { project: project_id_of(app_id), page: String::new() },
        })
    }

    /// Project を `page`（None・無い id なら先頭）で開き、PageRouter を作り直して、その Page の入力途中の状態を戻す（`activate`）。
    /// Ok は移る先の `View::Project`。Err は Apps に出す 1 行（開いていない・保存した Project が読めない）。
    ///
    /// 保証する: 開き直しても Page の数・順・id は保存した Project のまま（再起動で 3 枚が並ぶ）。
    pub fn open_project(&mut self, project_id: &str, page: Option<&str>) -> Result<View, String> {
        let (_, p, _) = self.project(project_id)?;
        let router = PageRouter::open(&p, page).map_err(|e| e.to_string())?;
        let current = router.current().to_owned();
        self.routers.insert(project_id.to_owned(), router);
        self.pending_delete = None;
        self.pending_rename = None;
        self.page_line = self.activate(project_id, &current).err().map(|e| (RegionState::Error, e));
        // RQ3: 取り込んだ時の 1 行（足りない接続）はこの Project を開いた所で出す（開けなかった理由の方が先）
        if self.opened_line.as_ref().is_some_and(|(p, _)| p == project_id) {
            let line = self.opened_line.take().map(|(_, l)| l);
            self.page_line = self.page_line.take().or(line);
        }
        // W16-3: 同じ pid・window_id に繋ぎ直す（activate の show_page が errors を消した後。Gone は次の draw_page の ensure が題名で取りに行かない）
        #[cfg(feature = "native_direct")]
        if let Err(e) = self.restore_native(project_id) {
            self.page_line = self.page_line.take().or(Some((RegionState::Error, e)));
        }
        // W12a: 開いた時（作った直後・再起動の後）に resource を 1 回読む。書けなくても開くのは止めない
        if let Err(e) = self.read_resources(project_id) {
            self.page_line = self.page_line.take().or(Some((RegionState::Error, e)));
        }
        // PROJECT-DIR-1101: folder の無い Project（前の版で作った物）は開いた所で作る（Show folder in Finder がすぐ使える・作れなくても開くのは止めない）
        let _ = self.project_folder(project_id);
        self.note_open(project_id);
        Ok(View::Project { project: project_id.to_owned(), page: current })
    }

    /// PROJECT-DIR-1101（owner 204「app ごとに自動で振り分けている状態にしてよ」）: その Project の Terminal（下の panel の tab・Page の Terminal・agent）が始まる folder。
    /// 順: repo（`existing_repo_dir` = `Change folder…`・chat bar の folder で当てた物）→ 保存した自分の folder（`FOLDER_KEY`・消されていれば同じ path に作り直す）→
    /// 無ければ `projects_root/<名前>` を作って保存（名前は `safe_dir_name`・同じ名前の物が在れば ` 2`・` 3`…）。Project の名前を替えても動かさない（保存した path のまま）。
    /// 例: `if let Some(dir) = self.generated.project_folder(&project) { run_open("/usr/bin/open", &[&dir.to_string_lossy()])?; }`
    /// 保証する: 中身は何も置かない・消さない・repo を当てた Project には作らない。None = repo が無く `projects_root` も無い（試験）・作れなかった（理由は log）・Project が無い。
    /// Wave: PROJECT-DIR-1101
    pub fn project_folder(&self, project_id: &str) -> Option<std::path::PathBuf> {
        let (_, p, _) = self.project(project_id).ok()?;
        self.folder_for(&p)
    }

    /// `project_folder` の本体（取り込む途中の Project = まだ保存していない定義でも引ける）。保存してから作る（作れなかった path は次に作り直す・
    /// 覚えていない folder を残さない）。
    /// ponytail: 自分の folder を利用者が消した間に別の Project が同じ名前を取ると 2 つで 1 つの folder（保存した path を信じる）。分けるなら作る時に他の行を引く
    fn folder_for(&self, p: &ProjectDefinitionV1) -> Option<std::path::PathBuf> {
        if let Some(repo) = existing_repo_dir(p) {
            return Some(std::path::PathBuf::from(repo));
        }
        let dir = match self.saved_folder(p) {
            Some(d) => d,
            None => {
                let (root, base) = (self.projects_root.as_ref()?, safe_dir_name(&p.name));
                let dir = (1..).map(|n| if n == 1 { root.join(&base) } else { root.join(format!("{base} {n}")) }).find(|d| !d.exists())?;
                // 取り込む途中（app の行がまだ無い）は覚えられない = 作らない（次に開いた時に作る）
                let app = p.id.strip_prefix("project.")?;
                let session = session_of(app);
                let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
                let kept = match st.get(Table::Sessions, &session) {
                    Ok(Some(_)) => Ok(()),
                    _ => st.put(Table::Sessions, &session, Some(app), &serde_json::json!({ "app_id": app })),
                }
                .and_then(|()| st.put_ui_state(&session, FOLDER_KEY, &serde_json::json!({ "dir": dir.display().to_string() })));
                if let Err(e) = kept {
                    eprintln!("PROJECT-DIR: {}: folder を覚えられません: {e}", p.id);
                    return None;
                }
                dir
            }
        };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                eprintln!("PROJECT-DIR: {}: folder を作れません: {e}", dir.display());
                None
            }
        }
    }

    /// `project_folder` の読むだけの半分（repo か保存した path・作らない・在るかは見ない）。chat bar の名前と Project の設定の行が毎 frame 読む。
    fn saved_folder(&self, p: &ProjectDefinitionV1) -> Option<std::path::PathBuf> {
        if let Some(repo) = existing_repo_dir(p) {
            return Some(std::path::PathBuf::from(repo));
        }
        let session = session_of(p.id.strip_prefix("project.")?);
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        st.get_ui_state(&session, FOLDER_KEY).ok().flatten().and_then(|v| v["dir"].as_str().map(std::path::PathBuf::from)).filter(|d| d.is_absolute())
    }

    /// ACCOUNTS-1101 2 段目: その Project で最後に選んだ AI の account（chat bar の Provider で選んで確かめが通った物・`ACCOUNT_KEY`）。
    /// None = 覚えていない・Some(None) = System default・Some(Some((種類, id))) = `AiAccount::kind` と `AiAccount::id`（列で引くのは呼ぶ側）。
    /// 例: `if let Some(want) = g.project_account(&project) { … }`
    /// Wave: ACCOUNTS-1101
    pub fn project_account(&self, project_id: &str) -> Option<Option<(String, String)>> {
        let (_, p, _) = self.project(project_id).ok()?;
        let session = session_of(p.id.strip_prefix("project.")?);
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let v = st.get_ui_state(&session, ACCOUNT_KEY).ok().flatten()?;
        Some(match (v["kind"].as_str(), v["id"].as_str()) {
            (Some(kind), Some(id)) => Some((kind.to_owned(), id.to_owned())),
            _ => None,
        })
    }

    /// ACCOUNTS-1101 2 段目: その Project の account を覚える（None = System default）。呼ぶ側 = app.rs の `poll_llm`（Project の中で切り替えた確かめが通った時）。
    /// Err = 覚えられなかった理由（画面には出さない・log）。例: `let _ = g.set_project_account(&project, ui.active_account());`
    /// Wave: ACCOUNTS-1101
    pub fn set_project_account(&self, project_id: &str, account: Option<&crate::model::AiAccount>) -> Result<(), String> {
        let (_, p, _) = self.project(project_id)?;
        let app = p.id.strip_prefix("project.").ok_or_else(|| format!("{project_id} isn't a project"))?;
        let session = session_of(app);
        let value = match account {
            Some(a) => serde_json::json!({ "kind": a.kind(), "id": a.id() }),
            None => serde_json::json!({ "kind": null }),
        };
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        // FOLDER_KEY と同じ: ui_state は sessions の行を要る（FK）= 無ければ先に作る
        let kept = match st.get(Table::Sessions, &session) {
            Ok(Some(_)) => Ok(()),
            _ => st.put(Table::Sessions, &session, Some(app), &serde_json::json!({ "app_id": app })),
        };
        kept.and_then(|()| st.put_ui_state(&session, ACCOUNT_KEY, &value)).map_err(|e| e.to_string())
    }

    /// Page の Terminal・下の panel の tab・agent の作業 folder = `project_folder`（repo → 自分の folder）。どちらも無い（`projects_root` の無い試験・作れなかった）時だけ `$HOME`。
    fn terminal_cwd(&self, p: &ProjectDefinitionV1) -> String {
        self.folder_for(p).map(|d| d.display().to_string()).or_else(|| std::env::var("HOME").ok()).unwrap_or_else(|| "/".into())
    }

    /// USAGE-1: 開いた日を `USAGE_FILE` に 1 行（同じ Project・同じ地域の日は process で 1 回）。書けなくても開くのは止めない（記録は数えるためだけ）
    fn note_open(&mut self, project_id: &str) {
        let Some(log) = &mut self.usage_log else { return };
        let now = unix_now();
        if log.seen.insert((project_id.to_owned(), (now + utc_offset()).div_euclid(86_400))) {
            log.append(now, ("project", Value::String(project_id.to_owned())));
        }
    }

    /// RQ1（RQ-004・Settings › About と `Copy diagnostics`・監査 S7「App/Bridge/OSのバージョン、macOS権限の確認、モデル/Adapter疎通試験」）: About に並べる行と、
    /// 同じ行を `"{k}: {v}"` で改行に繋いだ物が clipboard の文（画面と貼った文が食い違わない）。新しい型を作らず rail の `about_rows` と同じ (項目, 値) の形。
    /// 行の並び: `Shibubu`（`v{版}`）・`macOS`（版）・`Mac`（arch・Rosetta）・`AI`（`ai_rows` の状態の語と detail）・`AI calls this launch`（`budget_used`）・
    /// `checks` の各行（`{name}` = `{状態の語} · {detail}`・画面の操作の許可と疎通・GitHub・Web data・MCP）・`Projects`（数だけ）・`Runs needing attention`（数だけ）。
    /// `checks` = app.rs の `connections_model().rows`（bridge の許可と疎通は app.rs が持つ）。呼ぶ側の例:
    /// `let rows = self.generated.diagnostics(&self.connections_model().rows); ctx.copy_text(&rows.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n"));`
    /// 保証する: 値は全部 `storage::redact::redact` を通す・file の path（`/Users/…`・`~/…`）・token・email・打った文・Project と Page の名前を入れない・何も書かない・送らない・
    /// model を呼ばない・疎通を新しく打たない（今の状態を読むだけ。測り直しは各行の `Check again`）。保証しない: 送信（貼るのは利用者）・`Report a problem`（作らない）。
    /// Wave: RQ1
    pub fn diagnostics(&self, checks: &[crate::ui::connections::Connection]) -> Vec<(String, String)> {
        let (os, mac) = env_facts();
        let state = |c: &crate::ui::connections::Connection| if c.checking { format!("Checking · {}", c.detail) } else { format!("{} · {}", c.state.label(), c.detail) };
        let mut rows: Vec<(String, String)> = vec![("Shibubu".into(), format!("v{}", env!("CARGO_PKG_VERSION"))), ("macOS".into(), os.clone()), ("Mac".into(), mac.clone())];
        rows.extend(self.ai_rows().iter().map(|c| (c.name.to_string(), state(c))));
        rows.push((
            "AI calls this launch".into(),
            match self.budget_used() {
                Some((n, t)) => format!("{n} · {} seconds", t.as_secs()),
                None => "No AI yet".into(),
            },
        ));
        // L4: Connections の AI providers の `ai` の行は上の `ai_rows` と同じ物（2 度書かない）
        rows.extend(checks.iter().filter(|c| c.key != "ai").map(|c| (c.name.to_string(), state(c))));
        rows.push(("Projects".into(), self.rows().len().to_string())); // sidebar に見えている数（Page 1 枚の app は projects の表に書かない）
        let attention = self.run_entries().iter().filter(|e| matches!(e.result(), RunResult::Unknown | RunResult::NeedsApproval | RunResult::NeedsConnection)).count();
        rows.push(("Runs needing attention".into(), attention.to_string()));
        rows.into_iter().map(|(k, v)| (k, crate::ui::runs::mask_personal(&storage::redact::redact(&v)))).collect()
    }

    /// USAGE-1: 「使った記録をコピー」の文（app.rs が ⌘K の `Copy usage report` で clipboard に入れる）。
    /// 読む物: Project の revision（作った・一言で直した・手で直した・Undo の時刻）と `USAGE_FILE`（開いた日）。何も書かない・送らない。
    /// 保証する: Project の名前・id・file の path・打った文を含まない（`usage_summary`）。読めない行は数えずに飛ばす。
    /// 保証しない: Undo の後に別の編集を当てて消えた revision（`Store::truncate_project_revisions`）は数えない = 回数は下限。
    /// 例: `ctx.copy_text(self.generated.usage_report());`
    /// 契約（RQ1 から・署名は変えない）: `USAGE_FILE` の `launch` と `action` の行から `usage_summary` の `launches` を作る（起動の行ごとに、次の起動の行より前の最初の action）。
    /// 作り手は ⌘K の行と Settings › About の `Copy usage report`（どちらも `Intent::CopyUsageReport`）。
    /// Wave: USAGE-1, RQ1
    pub fn usage_report(&self) -> String {
        let mut revisions = Vec::new();
        {
            let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
            for p in st.list(Table::Projects, None).unwrap_or_default() {
                for r in st.list_project_revisions(&p.id).unwrap_or_default() {
                    if let Some(t) = crate::agent::secs_from_rfc3339(&r.created_at) {
                        revisions.push((r.project_id, t, r.origin));
                    }
                }
            }
        }
        let log = self.usage_log.as_ref().and_then(|l| std::fs::read_to_string(&l.path).ok()).unwrap_or_default();
        let rows: Vec<Value> = log.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let at = |v: &Value| v["at"].as_str().and_then(crate::agent::secs_from_rfc3339);
        let opens: Vec<(String, i64)> = rows.iter().filter_map(|v| Some((v["project"].as_str()?.to_owned(), at(v)?))).collect();
        // RQ1: 起動の行ごとに、次の起動の行より前の最初の action の行（file の順で組む = 同じ秒に起動し直しても取り違えない・追記だけの file）
        let mut launches: Vec<(i64, Option<i64>)> = Vec::new();
        for v in &rows {
            match (at(v), v["launch"] == Value::Bool(true), v["action"] == Value::Bool(true)) {
                (Some(t), true, _) => launches.push((t, None)),
                (Some(t), _, true) => {
                    if let Some(last) = launches.last_mut() {
                        last.1.get_or_insert(t);
                    }
                }
                _ => {}
            }
        }
        let (os, mac) = env_facts();
        let env = format!("v{} · macOS {os} · {mac}", env!("CARGO_PKG_VERSION"));
        usage_summary(&revisions, &opens, &launches, unix_now(), utc_offset(), &env)
    }

    /// 主領域を `from` から `to` へ移す時の Page の段（app.rs の `go` と `on_exit` が呼ぶ 1 か所）。古い `View::App(id)` は包んだ Project の先頭 Page へ変える。
    /// 同じ Project の中は `navigate_page`、別の Project は今の Page を保存して `open_project`、Project 以外は今の Page を保存して移る。
    pub fn route(&mut self, from: &View, to: View) -> Result<View, String> {
        let to = match to {
            View::App(id) => self.project_view_of(&id).unwrap_or(View::App(id)),
            v => v,
        };
        match (from, &to) {
            (View::Project { project: a, .. }, View::Project { project: b, page }) if a == b => return self.navigate_page(b, page),
            // 保存できなければ移らない（打った文を黙って失わない・navigate_page と同じ規則）
            // UI-5a: tab の横の消す確認は Project の帯の行 → 離れたら閉じる（Home・Apps の最初の Esc を見えない確認が飲まない）
            (View::Project { project, page }, _) => {
                self.save_view_state(project, page)?;
                self.pending_delete = None;
                self.pending_rename = None;
            }
            _ => {}
        }
        match to {
            View::Project { project, page } => self.open_project(&project, Some(page.as_str()).filter(|p| !p.is_empty())),
            v => Ok(v),
        }
    }

    /// 同じ Project の別の Page へ移る（SPEC §4: 今の Page の `save_view_state` → router の navigate → 移った先の `activate`）。
    ///
    /// 使い方: app.rs の `Intent::Go(View::Project{project, page})` で、今の View が同じ Project ならこれ、違えば `open_project`。
    /// 保証する: 移る前の Page の入力途中の状態を保存してから移る。無い Page は Err の 1 行で、居場所も状態も変えない。
    pub fn navigate_page(&mut self, project_id: &str, to: &str) -> Result<View, String> {
        let r = self.navigate_inner(project_id, to);
        // W12a: 同じ Project の中で Page を移った時に resource を 1 回読む（＋Page・編集の後の移りは navigate_inner だけで読まない）
        if r.is_ok() {
            if let Err(e) = self.read_resources(project_id) {
                self.page_line = self.page_line.take().or(Some((RegionState::Error, e)));
            }
        }
        self.remember(r, RegionState::Error)
    }

    /// Page の操作（＋Page・名前替え・削除・戻る）。形を変える物は `router::{new_page, rename_page}` / `RemovePage` を
    /// `apply_project(.., RevisionOrigin::DirectEdit)` で当て、router を `reconcile` する。Ok は移る先（移らない操作は今の Page）。
    ///
    /// 保証する: 拒否（最後の 1 枚・古い base_hash・無い id）は Err の 1 行で、何も書かず今の Page に残る。Page ID は変えない。
    /// 契約（UI-4a から・署名は変えない）: `Edit` の拒否のうち文を直せば通る物（読めない = 救いの 1 行・既に在る・当てる先が無い・前の依頼が読まれている）は
    /// Page の上の 1 行を `RegionState::Ready`（× Failed を付けない・`RunTests` の拒否と同じ・③ C2）。書けなかった時（store・古い base_hash）だけ Error。
    /// 欄を空にする合図は `take_edit_applied`（ここでは欄に触らない）。
    /// 契約（UI-6a = sendback 60.5）: `AnswerMissingApp` の拒否（この版に無い手・候補に無い・もう答えた）も `Ready`（理由の文だけ）。
    /// 今の `Blocked` は Page の上の 1 行に押せる button が無いので ▣ の規則（sendback 31: 承認・許可の button が同じ行に在る時だけ）に合わない
    /// （1 問の button は使える検査の行 = `renderer::usable_line` の `choices` に在り、そちらの Blocked は残る）。
    /// 契約（FX3・署名は変えない）: ① FX-013: AI がこの Project の変更を読んでいる間（`JobWhat::Edit { project_id }` がこの Project）、
    /// Page の形を変える腕（`Add`・`Rename`・`Delete`・`Move`・`Undo`・`Redo`・`AddBrowser`・`AddTerminal`）は当てる前に
    /// `Err("One change at a time. The last one is still being read.")`（Ready・何も書かない・job は止めない）。`Edit` は今の app.rs の門のまま（読んでいる間は送らない）。
    /// ② 帯の手が通ったら `page_line` に `(RegionState::Ready, change_line(&前, &後, &intent))`（None なら今まで通り）・影響を受けた tab の tint の時刻を覚える（private）。
    /// ③ `Rename` は今と同じ名前（trim 後）なら書かずに Ok（今の Page）・`DuplicateTitle` と空は Err の 1 行で `pending_rename` を開いたまま（打った名前を失わない）。
    /// 例: `g.page_intent(&project, PageIntent::Add)?; assert_eq!(g.page_line, Some((RegionState::Ready, "Added page “Page 2” · ⌘Z to undo".into())))`
    /// Wave: W2, UI-4a, UI-6a, FX3
    pub fn page_intent(&mut self, project_id: &str, intent: PageIntent) -> Result<View, String> {
        // UI-2（図 §36）: 欄の `run the tests` は Tests の Page の「走らせる」と同じ道・同じ拒否（model にも Engine にも渡さない）
        let field_run = matches!(&intent, PageIntent::Edit { text } if composer::project::EditVerb::read(text) == Some(composer::project::EditVerb::RunTests));
        let intent = if field_run { PageIntent::RunTests { command: "cargo test".into() } } else { intent };
        // 反証の直し: 走っている 1 行と結果は Tests の Page（route /tests）にしか描かないので、欄からはその Page へ移ってから走らせる
        // （拒否の 1 行もそこに出る）。Tests の Page が無い Project は今の Page のまま（表に EN が無い・sendback 60.4）
        if field_run {
            let tests = self.project(project_id).ok().and_then(|(_, p, _)| p.pages.iter().find(|pg| pg.route == "/tests").map(|pg| pg.id.clone()));
            if let Some(to) = tests {
                self.navigate_page(project_id, &to)?;
            }
        }
        // 反証の直し: RunTests の拒否（人が打っている・走っている・folder が無い）は ▣ Needs approval にしない（承認の button が同じ行に無い・UI-1 の約束）・
        // × Failed にもしない（test の失敗と混ぜない・e2e の守り）。Ready は UI-6 で語を描かず理由の文だけになる（SPEC §8「Ready は何も出さない」）
        // ponytail: Terminal を起こせない等の本当の失敗も Ready の 1 行。拒否と失敗を分けるなら page_intent_inner の Err に種類を持たせる
        // W13b: 1 問への答えの拒否（この版に無い手・候補に無い・もう答えた）は問いが待っている状態で、失敗ではない。
        // UI-6a（60.5）: Page の上のこの行に押せる button は無い（1 問の button は使える検査の行）ので ▣ にせず Ready（理由の文だけ）
        // PV0: 導入の手の拒否（もう答えた・別の導入が走っている・Homebrew が無い）も問いが待っている状態（理由の文だけ）
        let state = if matches!(intent, PageIntent::RunTests { .. } | PageIntent::AnswerMissingApp { .. } | PageIntent::Install { .. }) { RegionState::Ready } else { RegionState::Error };
        // FX3（FX-013）: AI がこの Project の変更を読んでいる間は Page の形を変える手を当てない（人の手で AI の編集が base_hash で黙って落ちない・何も書かない・job は止めない）
        let shapes = matches!(
            intent,
            PageIntent::Add
                | PageIntent::Rename { .. }
                | PageIntent::Delete { .. }
                | PageIntent::Move { .. }
                | PageIntent::Undo
                | PageIntent::Redo
                | PageIntent::AddBrowser { .. }
                | PageIntent::AddMirrorFrame { .. }
                | PageIntent::AddTerminal { .. }
        );
        // AI-COMPOSE-1101: AI が書いた部品の値を残す手も revision を 1 つ積むので同じ（値は頁の中に見えたまま・残すのは AI の変更の後）
        if (shapes || matches!(intent, PageIntent::CanvasSet { .. })) && self.reading_change_of(project_id) {
            self.page_line = Some((RegionState::Ready, ONE_CHANGE.into()));
            return Err(ONE_CHANGE.into());
        }
        let band = shapes.then(|| self.project(project_id).ok().map(|(_, p, _)| (p, intent.clone()))).flatten();
        let r = self.page_intent_inner(project_id, intent);
        if field_run && r.is_ok() {
            self.edit_applied = Some(project_id.to_owned());
        }
        // FX3（FX-011・FX-014）: 帯の手が通ったら Page の上に何が変わったかの 1 行（手そのものが同じ呼び出しの中で出した Error の 1 行〈移った先の activate など〉があればそちらを優先・前の手の 1 行は page_intent_inner が先に消す）・影響を受けた tab の tint・足した tab に focus
        if let (Ok(view), Some((before, done))) = (&r, band) {
            if let Ok((_, after, _)) = self.project(project_id) {
                if let Some(line) = change_line(&before, &after, &done) {
                    self.page_line = self.page_line.take().or(Some((RegionState::Ready, line)));
                }
                self.tinted = band_diff(&before, &after).and_then(|(_, id)| id).map(|id| (id, None));
            }
            if let (PageIntent::Add, View::Project { page, .. }) = (&done, view) {
                self.focus_tab = Some(page.clone());
            }
        }
        // 文の拒否（finish_edit が印を立てた）は直せば通る = Ready（× Failed を付けない・③ C2）。書けなかった時は Error のまま
        let state = if std::mem::take(&mut self.edit_refused) { RegionState::Ready } else { state };
        self.remember(r, state)
    }

    /// 今の画面の状態（`Opened::state`）から Page の入力途中の状態を写して ui_state に保存する
    /// （session `main.{app_ref}`・key `project::view_state_key(page_id)`・JSON は `project::PageViewState`）。
    ///
    /// 保証する: 保存先の無い物（開けない Project・router の外で消えた Page・app_ref を持たない Page）は何も書かず Ok
    /// （消えた Page から在る Page へ・Apps へ逃げる道を塞がない）。書けなかったら Err の 1 行（呼ぶ側は移らない）。
    pub fn save_view_state(&mut self, project_id: &str, page_id: &str) -> Result<(), String> {
        let r = self.save_view_inner(project_id, page_id);
        // W16-1: 保存できて移る時だけ、その Page に重ねた窓を Shibubu の後ろへ（移らない時に隠すと枠が空になる）
        // W16-2: 隠す前に 1 回読み戻す（移る前の文書・A2）
        #[cfg(feature = "native_direct")]
        if r.is_ok() {
            self.readback_page(project_id, page_id);
            self.surfaces.hide_page(page_id);
        }
        r
    }

    fn save_view_inner(&mut self, project_id: &str, page_id: &str) -> Result<(), String> {
        let Ok((_, p, _)) = self.project(project_id) else { return Ok(()) };
        let Some(page) = p.page(page_id) else { return Ok(()) };
        let Some(app_ref) = page.app_ref.as_deref() else { return Ok(()) };
        let opened = self.find(app_ref).ok_or("The project for this page isn't open. Reopen it from Home")?;
        let json = serde_json::to_value(PageViewState::capture(&opened.state)).map_err(|e| e.to_string())?;
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.put_ui_state(&session_of(app_ref), &view_state_key(page_id), &json).map_err(|e| format!("Couldn't keep what you were typing on this page · {e}"))
    }

    /// 保存した Page の入力途中の状態を今の画面の状態に戻す。保存が無ければ空の状態にする（前の Page の draft を出さない）。
    ///
    /// 保証する: 読めない JSON（後の版・壊れた）は空の状態にして Err の 1 行を返す（画面は開く）。
    pub fn activate(&mut self, project_id: &str, page_id: &str) -> Result<(), String> {
        // W16-1: その Page に重ねていた窓を、隠した時と同じ窓のまま前へ戻す（重ねていない Page は何もしない）
        // W16-2: 戻した後に 1 回読み戻す（戻った後の文書・A2）
        #[cfg(feature = "native_direct")]
        {
            self.surfaces.show_page(page_id);
            self.readback_page(project_id, page_id);
        }
        let (_, p, _) = self.project(project_id)?;
        let page = p.page(page_id).ok_or_else(|| ProjectPatchError::PageNotFound(page_id.into()).to_string())?;
        let Some(app_ref) = page.app_ref.clone() else { return Ok(()) };
        let saved = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.get_ui_state(&session_of(&app_ref), &view_state_key(page_id))
        };
        let opened = self.apps.iter_mut().find(|o| o.rt.revision().definition().id == app_ref).ok_or("The project for this page isn't open. Reopen it from Home")?;
        let read = saved.map_err(|e| e.to_string()).and_then(|v| v.map(serde_json::from_value::<PageViewState>).transpose().map_err(|e| e.to_string()));
        let (view, err) = match read {
            Ok(v) => (v.unwrap_or_default(), None),
            Err(_) => (PageViewState::default(), Some("Couldn't restore what you were typing on this page. It opened empty.".to_owned())),
        };
        view.restore(&mut opened.state);
        err.map_or(Ok(()), Err)
    }

    /// `View::Project{project, page}` の中身: 上に Page の帯（文字の tab と `↶ ↷ ＋ •••`・UI-5a）、下に `renderer::render_page`。
    /// 押した物は Intent で返す（タブ = `Go(View::Project{..})`・menu の項目等 = `Page(..)`・無い Project = `Go(View::Home)` への道）。
    ///
    /// 保証する: 無い Project / Page は Empty の 1 行と次の一手を出し、偽の画面を描かない。直前の拒否の 1 行は Page の上に出す。
    /// 契約（UI-4a から・署名は変えない）: Page の中の言葉の欄（「Change this project」と `Send changes`）は描かない（欄は header の 1 つ・d1_one_field）。
    /// 無い Project の道は `Go(View::Home)`（button の文は表の `Go to Home`・G14 B8 の 63）。Engine の 1 行・消す確認・帯は今のまま。
    /// 契約（UI-6a = 状態の行を 1 か所に）: `llm_line()` を呼ばない（Partial の AI の 1 行は Home だけ・編集の理由は toast = NOTIFY-1101 追記）。
    /// 状態の 1 行は `states::inline*` を通す（Ready は事実の文だけ・同じ文は 1 pass に 1 回）。`Host::line` は持っている状態で描く（60.3）。
    /// Tests の Page の結果の頭の語は表の EN（✓ Passed / × Failed）のまま。sendback 71: Git の欄の理由の 1 行の `↻ Refresh` は `•••` の menu を名指す文に
    /// （帯の `Refresh` を menu へ移すのは UI-5a = yc/ui-5a・その merge の後）。保証しない: Page の上の行を 0 にする事（題の行への集約は UI-6b・消す確認を tab の横へは UI-5a）。
    /// 契約（FX3・署名は変えない）: 帯の tab は全 Page（TITLEBAR-ICONS-1101 で FX3 の `page_tabs` の 5 枚と `More` の menu を消した = 溢れた分は帯の横 scroll）。
    /// tab の drag は `PageIntent::Move`（doc）。`•••` の `Rename…` は `pending_rename` を立て、tab の席に
    /// `theme::rename_inline` を出す（確定 = `PageIntent::Rename`・Err の 1 行は欄の右）。帯の手が通った後の Page の上の 1 行は `change_line`（`page_line` の Ready）・
    /// 影響を受けた tab に `motion::TINT` の tint。`Add` の後は足した tab に・`AddBrowser` の後は Browser の上の欄に keyboard focus（FX-014）。
    /// AI がこの Project の変更を読んでいる間は `↶ ↷ ＋ •••` を `add_enabled_ui(false)` で押せない形にし、hover と Page の上の Loading の 1 行の隣に
    /// `One change at a time. The last one is still being read.`（FX-013・tab で Page を移るのは止めない）。消す確認の文は `Delete “{Page}”? ⌘Z brings it back.`（FX-015）。
    /// 例（試験）: `g.draw_page(ui, &project, &page)`（帯 ＋ 本体を 1 つの Ui に）。製品（app.rs）は `draw_page_band`（header）と `draw_page_body`（主領域）を別に呼ぶ（L1-NORAIL）。
    /// Wave: UI-6a, FX3, L1-NORAIL
    #[cfg(test)]
    pub fn draw_page(&mut self, ui: &mut egui::Ui, project_id: &str, page_id: &str) -> Option<Intent> {
        // L1-NORAIL: 製品は帯を header の中央（`draw_page_band`）・本体を主領域（`draw_page_body`）に別に描く。帯 ＋ 本体を 1 つの Ui に描くのは試験の呼び手
        let shown = self.project(project_id).is_ok_and(|(_, p, _)| p.page(page_id).is_some());
        let band = self.draw_page_band(ui, project_id, page_id);
        if shown {
            theme::rule(ui);
        }
        self.draw_page_body(ui, project_id, page_id).or(band)
    }

    /// L1-NORAIL: Page の帯（Page の tab〈全部・横 scroll〉・＋・↶ ↷ •••・消す確認・名前の欄）だけを `ui` に描く（app.rs は header の中央 = `header::draw` の `band`）。
    /// 契約は `draw_page` の帯の部分のまま（FX3・UI-5a）。無い Project / Page は何も描かず None（本体 = `draw_page_body` が Empty の 1 行を出す）。
    /// 例（app.rs）: `header::draw(.., &mut |ui| generated.draw_page_band(ui, &project, &page))`
    /// 契約（TITLEBAR-ICONS-1101 の 2'・案 A = operator の既定・owner が B / C を選んだら替える・署名は変えない）: `ui` は題の帯の中（header が Project の ⌄ の右から
    /// ◨ の左までを渡す）。左から Page の tab を全部（`theme::tab(.., Some(Icon::File), ..)`・今の Page は下線・⌄ を付けない・`page_tabs` と `MAX_TABS` と `More` は消した）を
    /// 横に scroll する 1 列に（scroll bar なし・trackpad の横と shift＋wheel・`drag_to_scroll(false)` = tab の drag の並べ替えと窓の drag を取り合わない・今の Page と
    /// 足した Page が替わった frame だけ見える所へ寄せる = terminal_panel の 215 と同じ）。最後の tab のすぐ右に ＋（列の中・溢れたら列の右端に残す）。
    /// 右端に ↶ ↷ •••（`↶ ↷` は在る時だけ）。⌘1〜9 は app.rs の keyboard が n 枚目の Page へ（Terminal に focus が在る時の ⌘1〜9 は今まで通り Terminal の tab）。
    /// 消す確認・名前の欄・tint・drag の Move・読んでいる間の押せない形は今のまま。帯の空いた所は header の drag（窓を動かす）と double-click（拡大）。
    pub fn draw_page_band(&mut self, ui: &mut egui::Ui, project_id: &str, page_id: &str) -> Option<Intent> {
        let Ok((_, p, _)) = self.project(project_id) else { return None };
        let page = p.page(page_id).cloned()?;
        let mut intent = None;
        // 読めない履歴は ↶ ↷ を出さない（理由の 1 行は本体 = `draw_page_body`）
        let (ur, _) = or_line(self.undo_redo(project_id));
        // W13c: 配置の型（読めなければ型なし = 今の並び・理由の 1 行は本体）
        let (placement, _) = or_line(self.placement(project_id));
        // UI-5a: 36 の行の左に文字の tab・右端に ↶ ↷ ＋ •••（↶ ↷ は在る時だけ・＋ ••• は右端に固定）。帯の他の操作は ＋ と ••• の menu と title line の ‹ へ（図 §38）
        let layout = placement.as_ref().map_or("None", |t| t.name.as_str());
        let (_, mut options) = page_menus(&p, &page.id, &ur, layout);
        // G94: 今の Page に効く型が 1 つも無ければ `Layout: …` の項目を出さない（押しても何も起きない物を見せない・L2）
        if placement_ids(&p, Some(&page)).is_empty() {
            options.retain(|c| c.intent != Intent::Page(PageIntent::Placement));
        }
        // MIRROR-AUTO-1101 B: この Page の Mirror の面が全部止まっている（赤で畳んだ・Stop mirroring の後）なら `Stop mirroring` を出さない（L2）
        #[cfg(feature = "mirror")]
        let stopped = |id: &String| self.mirror.as_ref().is_some_and(|m| m.window(id) == crate::ui::mirror_view::MirrorWindow::Closed);
        #[cfg(not(feature = "mirror"))]
        let stopped = |_: &String| true;
        if mirror_surfaces_on(&p, &page).iter().all(stopped) {
            options.retain(|c| !matches!(c.intent, Intent::Page(PageIntent::StopMirroring { .. })));
        }
        let pick = |ui: &mut egui::Ui, items: &[crate::ui::palette::Command]| {
            let mut picked = None;
            for c in items {
                let delete = matches!(c.intent, Intent::Page(PageIntent::Delete { .. }));
                // Page が 1 枚の時の `A project needs at least one page` は押せない形（理由の文がそのまま字）
                let enabled = !delete || p.pages.len() > 1;
                let variant = if delete { Variant::Danger } else { Variant::Ghost };
                if ui.add_enabled_ui(enabled, |ui| theme::button_menu_item(ui, &c.label, &c.hint, variant)).inner.clicked() {
                    picked = Some(c.intent.clone());
                }
            }
            picked
        };
        let confirming = self.pending_delete.as_deref() == Some(page.id.as_str());
        // FX3（FX-013）: AI がこの Project の変更を読んでいる間は ↶ ↷ ＋ ••• を押せない形（tab で移るのは止めない）
        let reading = self.reading_change_of(project_id);
        // FX3（FX-011）: 影響を受けた tab の tint（次に描く frame の時刻から motion::TINT で消える・色だけ = dur を通さない・試験の Cut は 0）
        let now = ui.input(|i| i.time);
        if let Some((_, t0 @ None)) = &mut self.tinted {
            *t0 = Some(now);
        }
        let cut = crate::ui::motion::motion_mode(ui.ctx()) == crate::ui::motion::MotionMode::Cut;
        let tint = match &self.tinted {
            Some((id, Some(t0))) if !cut => Some((id.clone(), 1.0 - crate::ui::motion::progress(ui.ctx(), *t0, crate::ui::motion::TINT))),
            _ => None,
        };
        if tint.as_ref().is_none_or(|(_, v)| *v <= 0.0) {
            self.tinted = self.tinted.take().filter(|(_, t0)| t0.is_none());
        }
        let tint_of = |id: &str| tint.as_ref().filter(|(t, _)| t == id).map_or(0.0, |(_, v)| *v);
        let focus_tab = self.focus_tab.take();
        let tab_icon = Some(crate::domain::icon::Icon::File);
        ui.horizontal(|ui| {
            ui.set_min_height(theme::TAB_H);
            if confirming {
                // 押した tab の横に 1 行の確認（答え ③・確認の間はその右の tab と ↶ ↷ ＋ ••• を描かない）。Enter は focus の在る Keep = 消さない。
                // TITLEBAR-ICONS-1101: 左の tab が多くても確認が見えるよう、列は右端に寄せた横 scroll（左の tab が外へ出る）
                egui::ScrollArea::horizontal()
                    .id_salt(("page-tabs-confirm", &p.id))
                    .stick_to_right(true)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for pg in &p.pages {
                                if theme::tab(ui, tab_icon, &pg.title, pg.id == page.id, 0.0).clicked() && pg.id != page.id {
                                    intent = Some(Intent::Go(View::Project { project: p.id.clone(), page: pg.id.clone() }));
                                }
                                if pg.id == page.id {
                                    break;
                                }
                            }
                            // FX3（FX-015）: 実際と同じ文（打ちかけは消す前に保存され ⌘Z で Page ごと戻る = Delete の腕）
                            match theme::confirm_inline(ui, &format!("Delete “{}”? ⌘Z brings it back.", page.title)) {
                                Some(true) => intent = Some(Intent::Page(PageIntent::Delete { page: page.id.clone() })),
                                Some(false) => self.pending_delete = None,
                                None => {}
                            }
                        });
                    });
                return;
            }
            // TITLEBAR-ICONS-1101 の 2'（案 A）: 右の組 = ↶ ↷ •••（在る物だけ）・左の room = Page の tab の列（横 scroll）とそのすぐ右の ＋
            let n = 1 + usize::from(ur.undo.is_some()) + usize::from(ur.redo.is_some());
            let gap = ui.spacing().item_spacing.x;
            let group_w = n as f32 * theme::BUTTON_H + (n - 1) as f32 * gap;
            // 子の ui は親の cursor を動かさないので、右の組の x は tab の数で動かない
            let (_, room) = ui.allocate_space(egui::vec2((ui.available_width() - group_w - gap).max(0.0), theme::TAB_H));
            let mut row = ui.new_child(egui::UiBuilder::new().id_salt("page-tabs-row").max_rect(room).layout(egui::Layout::left_to_right(egui::Align::Center)));
            row.set_clip_rect(room.intersect(ui.clip_rect()));
            // 今の Page か Page の数が替わった frame だけ今の tab を見える所へ寄せる（毎 frame 寄せると手で scroll できない・terminal_panel の 215 と同じ）
            let seen_id = egui::Id::new(("page-tabs-seen", &p.id));
            let seen = (page.id.clone(), p.pages.len());
            let reveal = ui.ctx().data(|d| d.get_temp::<(String, usize)>(seen_id)).as_ref() != Some(&seen);
            ui.ctx().data_mut(|d| d.insert_temp(seen_id, seen));
            // 帯の席（Page の index・席の中心の x）を描いた順に。名前の欄の席も数える（drag が越えた席を数え落とさない）
            let mut slots: Vec<(usize, f32)> = Vec::new();
            let mut dropped: Option<(usize, f32)> = None;
            // 列は ＋ の分を引いた幅まで: 溢れなければ列は中身の幅に縮み ＋ は最後の tab のすぐ右・溢れたら列が最大幅で ＋ は room の右端に残る。
            // scroll bar は出さない（trackpad の横・shift＋wheel）・drag で scroll しない（tab の drag は並べ替え・空いた所の drag は窓）
            egui::ScrollArea::horizontal()
                .id_salt(("page-tabs-scroll", &p.id))
                .max_width((room.width() - theme::BUTTON_H - gap).max(0.0))
                .auto_shrink([true, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .drag_to_scroll(false)
                .show(&mut row, |row| {
                    row.horizontal(|row| {
                        // travel の原点 = 列の中身の左端（scroll で一緒に動く = scroll では tab を滑らせない）
                        let origin = row.max_rect().left();
                        for (i, pg) in p.pages.iter().enumerate() {
                            // FX3 反証の直し（FX-011・UIUX B03）: 席が変わった tab は前の位置から新しい位置へ滑る（列の左端からの x を travel で寄せ、差だけ描く物をずらす・
                            // 押せる席は最初の frame から新しい位置・Reduce Motion と試験の Cut は即時 = travel の契約・初めて見た tab は動かない）
                            let x0 = row.cursor().min.x;
                            let dx = crate::ui::motion::travel(row.ctx(), egui::Id::new(("page-tab-x", &p.id, &pg.id)), x0 - origin, crate::ui::motion::PAGE) - (x0 - origin);
                            // FX3（FX-009）: `Rename…` で開いた名前の欄は tab の席に（Enter = 名前替え・Esc = 閉じる）
                            if let Some((_, draft)) = self.pending_rename.as_mut().filter(|(id, _)| *id == pg.id) {
                                match theme::rename_inline(row, draft, "Page name") {
                                    Some(true) => intent = Some(Intent::Page(PageIntent::Rename { page: pg.id.clone(), title: draft.clone() })),
                                    Some(false) => self.pending_rename = None,
                                    None => {}
                                }
                                slots.push((i, (x0 + row.min_rect().right()) / 2.0));
                                continue;
                            }
                            let r = row
                                .with_visual_transform(egui::emath::TSTransform::from_translation(egui::vec2(dx, 0.0)), |ui| theme::tab(ui, tab_icon, &pg.title, pg.id == page.id, tint_of(&pg.id)))
                                .inner
                                .interact(egui::Sense::drag());
                            if focus_tab.as_deref() == Some(pg.id.as_str()) {
                                r.request_focus();
                                r.scroll_to_me(None);
                            } else if reveal && pg.id == page.id {
                                r.scroll_to_me(None);
                            }
                            slots.push((i, r.rect.center().x));
                            if r.clicked() && pg.id != page.id {
                                intent = Some(Intent::Go(View::Project { project: p.id.clone(), page: pg.id.clone() }));
                            }
                            // FX3（FX-010）: tab を掴んで他の tab の中心を越えて離すと Move（•••の Move left / right と同じ intent）
                            if r.drag_stopped() && !reading {
                                if let Some(at) = row.ctx().pointer_interact_pos() {
                                    dropped = Some((i, at.x));
                                }
                            }
                        }
                    });
                });
            if let Some((from, x)) = dropped {
                // 席で数える: 中心が x より左の他の席の数 k と、掴んだ tab の席 pos を比べる。同じ = 手にしない・左 = その席の Page の前・右 = その席の Page の後
                let others: Vec<usize> = slots.iter().filter(|(j, _)| *j != from).map(|(j, _)| *j).collect();
                let k = slots.iter().filter(|(j, c)| *j != from && *c < x).count();
                let pos = slots.iter().position(|(j, _)| *j == from).unwrap_or(k);
                let to = match k.cmp(&pos) {
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Less => Some(others[k]),
                    std::cmp::Ordering::Greater => Some(others[k - 1]),
                };
                if let Some(to) = to {
                    intent = Some(Intent::Page(PageIntent::Move { page: p.pages[from].id.clone(), index: to }));
                }
            }
            // ＋ は列のすぐ右。MIRROR-FRAME-1101（owner 253 = 案 3）: Page を 1 枚足すだけ（menu を開かない・部品は Page の右下の ＋ と右 click = `draw_page_body`）
            row.add_enabled_ui(!reading, |ui| {
                if theme::icon_button(ui, crate::domain::icon::Icon::Plus, "Add page · ⌘T").on_disabled_hover_text(ONE_CHANGE).clicked() {
                    intent = Some(Intent::Page(PageIntent::Add));
                }
            });
            ui.add_enabled_ui(!reading, |ui| {
                // 行き先が無い向きは出さない（押しても何も起きないボタンを見せない・L2）
                // FX3（FX-012）: ↶ の hover の 2 行目に ⌘Z が戻さない物（AX の名前 `Undo · ⌘Z` は変えない）
                if ur.undo.is_some() && theme::icon_button(ui, crate::domain::icon::Icon::Undo, "Undo · ⌘Z").on_hover_text(undo_scope_line(&p)).on_disabled_hover_text(ONE_CHANGE).clicked() {
                    intent = Some(Intent::Page(PageIntent::Undo));
                }
                if ur.redo.is_some() && theme::icon_button(ui, crate::domain::icon::Icon::Redo, "Redo · ⇧⌘Z").on_disabled_hover_text(ONE_CHANGE).clicked() {
                    intent = Some(Intent::Page(PageIntent::Redo));
                }
                let more = theme::icon_button(ui, crate::domain::icon::Icon::Ellipsis, "Page options").on_disabled_hover_text(ONE_CHANGE);
                match theme::menu_popup(ui, &more, |ui| pick(ui, &options)) {
                    // 1 回目では消さない: tab の横の確認を出すだけ（PageIntent::Delete を返すのは確認の Delete だけ・答え ③）
                    Some(Intent::Page(PageIntent::Delete { page: target })) => {
                        self.pending_rename = None;
                        self.pending_delete = Some(target);
                    }
                    // FX3（FX-009）: `Rename…` は tab の席に名前の欄を開くだけ（今の名前で・1 回目で名前を替えない）
                    Some(Intent::Page(PageIntent::Rename { page: target, .. })) => {
                        let title = p.page(&target).map(|pg| pg.title.clone()).unwrap_or_default();
                        self.pending_rename = Some((target, title));
                    }
                    Some(i) => intent = Some(i),
                    None => {}
                }
            });
        });
        intent
    }

    /// L1-NORAIL: Page の本体（Page の上の 1 行・面・`renderer::render_page`）を `ui` に描く（app.rs は主領域）。帯は `draw_page_band`（header）。
    /// 無い Project / Page は Empty の 1 行と次の一手（`draw_page` の契約のまま）。
    pub fn draw_page_body(&mut self, ui: &mut egui::Ui, project_id: &str, page_id: &str) -> Option<Intent> {
        let p = match self.project(project_id) {
            Ok((_, p, _)) => p,
            Err(line) => return states::region(ui, RegionState::Empty, &line, Some("Go to Home")).then_some(Intent::Go(View::Home)),
        };
        let Some(page) = p.page(page_id).cloned() else {
            let first = View::Project { project: p.id.clone(), page: p.pages[0].id.clone() };
            return states::region(ui, RegionState::Empty, "This page was deleted.", Some("Go to first page")).then_some(Intent::Go(first));
        };
        let mut intent = None;
        let (ur, ur_err) = or_line(self.undo_redo(project_id));
        let (placement, placement_err) = or_line(self.placement(project_id));
        // W12a: read_resources が読む resource（uri を持ち、どこかの Page の node が bind している）が在る Project だけ「↻ 読み直す」と
        // 読めなかった理由の 1 行を出す（押しても何も起きないボタンを見せない・L2）
        let readable = readable_resources(&p).next().is_some();
        // FX3（FX-013）: AI がこの Project の変更を読んでいる間（帯の ↶ ↷ ＋ ••• を押せない理由を本体の Loading の 1 行の隣に）
        let reading = self.reading_change_of(project_id);
        // 変更の文は下の panel の Chat の欄（`Shell::enter_intent` → `Page(Edit)`・L1-NORAIL）。ここに言葉の欄を置かない（d1_one_field）
        // G86: Engine に渡した依頼の card（進めている・承認・済み・止めた・失敗）は下の panel の Chat の列に描く（`agent_rail::chat_column`・同じ button を 2 か所に描かない）
        if let Some((state, line)) = &self.page_line {
            states::inline_wrap(ui, *state, line);
            // G94: 文が言う `Add terminal` を button に（Terminal の無い Project の Run tests・消えた Terminal の Restart）
            if line.ends_with("· Add terminal") && theme::button_compact(ui, "Add terminal", Variant::Ghost).clicked() {
                intent = Some(Intent::Page(PageIntent::AddTerminal { page: page.id.clone() }));
            }
            // RQ3: 取り込んだ Project の足りない接続の 1 行（`import_project`）の `Connect` は Connections の画面へ
            if line.ends_with("· Connect") && theme::button_compact(ui, "Connect", Variant::Ghost).clicked() {
                intent = Some(Intent::Go(View::Connections));
            }
        }
        if let Some(line) = &placement_err {
            states::inline(ui, RegionState::Error, line);
        }
        // W13: AI が読んでいる間（裏の thread）。CLI が使えない理由（llm_line）は Home だけ（UI-6a・owner の問題 3「Home と全 Page」）・
        // 直前の編集で model の答えを使えなかった理由は toast（finish_edit の notices）
        if let Some(line) = self.model_busy_line() {
            // FX3（FX-013）: 帯の ↶ ↷ ＋ ••• を押せない理由をその隣に
            ui.horizontal(|ui| {
                states::inline(ui, RegionState::Loading, line);
                if reading {
                    states::inline(ui, RegionState::Ready, ONE_CHANGE);
                }
            });
        }
        // ⌘Z の拒否で page_line に同じ 1 行が入った時は 2 度出さない
        if let Some(line) = ur_err.filter(|l| self.page_line.as_ref().map(|(_, p)| p) != Some(l)) {
            states::inline(ui, RegionState::Error, &line);
        }
        // W16-2: この Page の Native Direct の面を READBACK_EVERY ごとに読み戻す（保存・Undo を同じ frame の surface-status に出す）。保存の失敗は page_line の 1 行に
        #[cfg(feature = "native_direct")]
        for spec in native_specs(&p, &page) {
            if self.surfaces.readback_due(project_id, &spec.id, std::time::Instant::now()) {
                if let Err(e) = self.readback_native(project_id, &spec.id) {
                    self.page_line = Some((RegionState::Error, e));
                }
            }
        }
        // W12a: 最後に読んだ時に読めなかった resource ごとの 1 行（理由と次の一手）。次に読めたら消える。「↻ 読み直す」が無い時は出さない
        // （Dev を消した直後は Delete が読まないので古い 1 行が残っている・押せない次の一手を言わない・D-e）
        for line in self.resource_lines.get(project_id).filter(|_| readable).into_iter().flatten() {
            states::inline(ui, RegionState::Error, line);
        }
        // W14b: 使える検査（この process で初めて描いた時に測り始め、この Page の席の 1 行を出す）
        self.start_usable(project_id, &p);
        if let Some(answer) = self.draw_usable(ui, &p, &page) {
            intent = Some(Intent::Page(answer));
        }
        // TERM-ONE-1101（owner 185「choosefolderはfolderアイコンで」「choose folderの後に...がある意味もわからない」）: folder を選ぶのは chat bar の下の行の
        // folder の icon だけ（CHAT-ONE-1101 の部品・`PageIntent::ChooseFolder`）。Page に `Choose folder…` の button と `or drop a folder` の行を置かない
        // ponytail: 毎 frame DB を 1 回読む（project と同じ）。重くなったら create_project の戻り値で cache する
        let (receipts, receipt_err) = or_line(self.surface_receipts(project_id));
        if let Some(line) = &receipt_err {
            states::inline(ui, RegionState::Error, line);
        }
        // W16-1: この Page の面のうち Native Direct で重ねる物を確かめ（窓を探すのは初回と一手の後だけ）、枠に描く今を Host に渡す
        #[allow(unused_mut)]
        let mut portals = BTreeMap::new();
        #[cfg(feature = "native_direct")]
        {
            let workspace = repo_dir(&p);
            for spec in native_specs(&p, &page) {
                let _ = self.surfaces.ensure(project_id, &page.id, spec, workspace.as_deref());
                if let Some(v) = self.surfaces.view(project_id, &spec.id) {
                    portals.insert(spec.id.clone(), v);
                }
            }
        }
        // Mirror の面（W16M-1）: Mirror の面を持つ Project を初めて描く時に、保存した同意と窓の形（UI-5m）で作る
        #[cfg(feature = "mirror")]
        if self.mirror.is_none() && p.surface_specs.iter().any(|s| s.backend == project::SurfaceBackendKind::Mirror) {
            // W16M-3（窓の選び方）: 窓は Project の repo の dir + file の書類の path で選ぶ（同じ題名の別の書類の窓に打たない）
            self.mirror = Some(self.mirror_pages(crate::ui::mirror_view::MirrorPages::helper_path(), repo_dir(&p)));
        }
        // 保存の欄は WebView の矩形より上（矩形は残りの高さを取り、子の NSView は egui の上に乗る・下に描くと見えない）
        self.draw_web_rows(ui, &p, &page);
        let specs = p.terminal_specs.clone();
        let run = self.run_state(project_id);
        // G88b: この Page の録音の帯ごとの入れる先の面（`feeds` = 面の id）と、届く面（使える検査の reach が通った席）
        let feeds: BTreeMap<String, Option<project::SurfaceSpec>> = page
            .root
            .nodes()
            .into_iter()
            .filter(|n| n.component == "recorder")
            .filter_map(|n| {
                let r = n.props.get("recorder_ref")?.as_str()?.to_owned();
                let f = n.props.get("feeds").and_then(Value::as_str).and_then(|f| p.surface_specs.iter().find(|s| s.id == f)).cloned();
                Some((r, f))
            })
            .collect();
        // ponytail: 帯の在る Page だけ毎 frame 使える検査を読む（draw_usable と同じ）。重くなったら 1 frame 1 回に寄せる
        let reached: Vec<String> = if feeds.is_empty() { Vec::new() } else { self.usable_of(project_id).into_iter().filter(|u| matches!(u.reach, Check::Pass(_))).map(|u| u.seat.id).collect() };
        // MIRROR-FRAME-1101（owner 253 = 案 3）: Page の何も無い所の右 click を受ける 1 枚の面。部品より先に置く（部品が上に来る）・focusable にしない
        // （AX と Tab の順に出さない）= renderer の node の選択の面と同じ形。範囲は部品を描き始める所から下の見えている所
        let clip = ui.clip_rect();
        let bg = ui.interact(egui::Rect::from_min_max(egui::pos2(clip.left(), ui.cursor().top().max(clip.top())), clip.max), egui::Id::new(("page-parts-bg", project_id, &page.id)), egui::Sense::CLICK);
        // 面（Mirror・Web・Terminal の face）の上の右 click は面の物（元の app の右 click）なので menu を開かない
        let mut faces: Vec<egui::Rect> = Vec::new();
        let placed = match page.app_ref.as_deref().and_then(|a| self.open_page_app(a, specs, &p.surface_specs, InputView::Page { page_id: page.id.clone() })) {
            None => {
                states::inline(ui, RegionState::Empty, "Nothing to show on this page");
                Vec::new()
            }
            Some((def, state, mut host, surfaces)) => {
                host.receipts = receipts;
                host.portals = portals;
                host.surfaces = p.surface_specs.clone();
                host.project = p.id.clone();
                if let Some(t) = host.takes.as_mut() {
                    t.feeds = feeds;
                    t.reached = reached;
                }
                // 押した結果の 1 行は持っている状態で描く（60.3）
                if let Some((state, line)) = host.line.as_ref() {
                    states::inline(ui, *state, line);
                }
                if page.root.children.is_empty() {
                    shell::content_opening(ui, &page.title, &def.goal.literal_goal, None);
                    // 状態の語が「Nothing here yet」なので detail は空（同じ語を 2 回出さない・G73 r14 shots/160）
                    states::inline(ui, RegionState::Empty, "");
                } else {
                    // W13c: 置き方だけを型に従わせた木で描く（保存した Page の木は変えない）。W13b の反証の直し: 1 問の node を外してから置く（空の区画を作らない）
                    let shown = PageDefinition { root: shown_root(page.root.clone()), ..page.clone() };
                    let placed = PageDefinition { root: composer::layout::place(&shown, &p, placement.as_ref()), ..page.clone() };
                    renderer::render_page(ui, &def, &placed, state, &mut host, surfaces);
                    faces = state.rects.iter().filter(|(k, _)| k.starts_with("face:")).map(|(_, r)| *r).collect();
                }
                // route が鍵（rename は title だけを替える: `rename Docs to Tests` で Docs に結果の列を出さない・答え 25 の趣旨）
                if page.route == "/tests" {
                    if theme::ax_name(theme::button_compact(ui, "Run tests", Variant::Primary), "Run tests").clicked() {
                        intent = Some(Intent::Page(PageIntent::RunTests { command: "cargo test".into() }));
                    }
                    match &run {
                        Some(RunState::Running { command, .. }) => {
                            states::inline(ui, RegionState::Running, &format!("Running {command}"));
                            // 印を待つ thread は egui を起こさないので、走っている間は描き直して終わりを拾う
                            ui.ctx().request_repaint_after(Duration::from_millis(250));
                        }
                        // 結果を取れなかった（時間切れ・Terminal が終わった）。test の「失敗」と混ぜない = Error（失敗しました）でなく Stale（sendback 31 の Uncertain と同じ）
                        Some(RunState::Unfinished { line, .. }) => states::inline(ui, RegionState::Stale, line),
                        None => {}
                    }
                    let recs = host.run_receipts(project_id);
                    if recs.is_empty() {
                        states::inline(ui, RegionState::Empty, "No runs yet");
                    } else {
                        for r in recs {
                            // 結果の語は表（copy.en.md:333・334）の EN の頭（成功 = exit_code 0 だけ・`RunReceipt::succeeded`）。time と件数の片は後（UI-6）
                            // ICONS-1101 追記 238: 結果の印（通った = 緑の check・落ちた = 赤の丸）を語の頭に・語は表の EN（Passed / Failed）
                            let (state, word) = if r.succeeded() { (RegionState::Verified, "Passed") } else { (RegionState::Error, "Failed") };
                            let tok = theme::tokens(ui.ctx());
                            let job = egui::text::LayoutJob::single_section(word.to_owned(), egui::TextFormat { font_id: theme::Type::Body.font(), color: tok.text_primary, ..Default::default() });
                            states::marked(ui, state, job, state.color(&tok), |l| l.wrap().selectable(true));
                            ui.add(egui::Label::new(format!("exit {}", r.exit_code)).wrap().selectable(true));
                        }
                    }
                }
                std::mem::take(&mut host.placed)
            }
        };
        // MIRROR-FRAME-1101（owner 253 = 案 3）: 部品を足す menu は 1 つ（`page_menus` の 1 つ目）。Page の右下の ＋（AX `Add to page`・pointer が Page の上・menu が
        // 開いている・＋ に focus が在る間だけ見え、見えない間も AX と Tab の順に居る）と、Page の何も無い所（`faces` の外）の右 click（`bg`）が同じ popup を開く。
        // 右 click は押した所に開く（押した frame の次の frame に開く = その click で popup が閉じない・egui の `CloseOnClickOutside` は副 button の click も数える）。
        // AI が変更を読んでいる間（`reading`）は押せない形と ONE_CHANGE（帯の ＋ と同じ・FX-013）・右 click も開かない
        let parts = page_menus(&p, &page.id, &ur, placement.as_ref().map_or("None", |t| t.name.as_str())).0;
        let ctx = ui.ctx().clone();
        let (at_id, keep_id) = (egui::Id::new(("page-parts-at", project_id, &page.id)), egui::Id::new(("page-parts-keep", project_id, &page.id)));
        // 右 click の所（false = 次の frame に開く・true = 開いた）
        let mut at = ctx.data(|d| d.get_temp::<(egui::Pos2, bool)>(at_id));
        if let Some(pos) = bg.interact_pointer_pos().filter(|pos| bg.secondary_clicked() && !reading && !faces.iter().any(|r| r.contains(*pos))) {
            at = Some((pos, false));
            ctx.request_repaint();
        }
        let shown = ui.rect_contains_pointer(clip) || at.is_some() || ctx.data(|d| d.get_temp::<bool>(keep_id)).unwrap_or(false);
        let r = egui::Rect::from_min_size(clip.right_bottom() - egui::Vec2::splat(theme::BUTTON_H + theme::SP_3), egui::Vec2::splat(theme::BUTTON_H));
        let mut corner = ui.new_child(egui::UiBuilder::new().id_salt(("page-parts-plus", project_id, &page.id)).max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        corner.set_opacity(crate::ui::motion::animate(&ctx, egui::Id::new(("page-parts-plus-a", project_id, &page.id)), if shown { 1.0 } else { 0.0 }, crate::ui::motion::HOVER));
        let picked = corner
            .add_enabled_ui(!reading, |ui| {
                let plus = theme::icon_button(ui, crate::domain::icon::Icon::Plus, "Add to page").on_disabled_hover_text(ONE_CHANGE);
                let menu = plus.id.with("kangae-menu");
                match (plus.clicked(), at) {
                    // ＋ を押した = ＋ の所に開く（開閉は `menu_popup`）
                    (true, _) => at = None,
                    (false, Some((pos, false))) if !ui.input(|i| i.pointer.any_click()) => {
                        ui.memory_mut(|m| m.open_popup(menu));
                        at = Some((pos, true));
                    }
                    _ => {}
                }
                let mut opener = plus.clone();
                if let Some((pos, true)) = at {
                    opener.rect = egui::Rect::from_min_size(pos, egui::Vec2::ZERO);
                }
                let picked = theme::menu_popup(ui, &opener, |ui| {
                    let mut out = None;
                    for c in &parts {
                        if theme::button_menu_item(ui, &c.label, &c.hint, Variant::Ghost).clicked() {
                            out = Some(c.intent.clone());
                        }
                    }
                    out
                });
                let open = ui.memory(|m| m.is_popup_open(menu));
                if !open && at.is_some_and(|(_, opened)| opened) {
                    at = None;
                }
                // `has_focus` は Context の lock を取る = `data_mut` の閉包の中で呼ぶと同じ thread が lock を二重に取り、main thread が止まったまま戻らない
                // （30 回目 c1fdec89: Project を開くと AX の窓 0・sample の main thread は parking_lot の park に 2118/2118）。lock の外で先に読む
                let keep = open || plus.has_focus();
                ctx.data_mut(|d| d.insert_temp(keep_id, keep));
                picked
            })
            .inner;
        ctx.data_mut(|d| match at {
            Some(a) => d.insert_temp(at_id, a),
            None => d.remove::<(egui::Pos2, bool)>(at_id),
        });
        if let Some(i) = picked {
            intent = intent.or(Some(i));
        }
        // ENGINE-GENERIC-1101: この Page の帯が録り終えた = 部品の手 `recorded`（Host は Generated を借りられないので描いた後に）。Project のきっかけが
        // 在れば一言と状況（録った file・直前の事）を Engine の 1 つの口へ・無ければ何もしない。始められない理由は帯の 1 行
        for n in page.root.nodes().into_iter().filter(|n| n.component == "recorder") {
            let Some(key) = n.props.get("recorder_ref").and_then(Value::as_str).map(|r| format!("{project_id}/{r}")) else { continue };
            let Some(slot) = self.takes.by_ref.get_mut(&key).filter(|s| s.fresh) else { continue };
            slot.fresh = false;
            let (Some(path), renderer::TakeStage::Recorded { seconds, .. }) = (slot.path.clone(), &slot.stage) else { continue };
            let name = n.label.clone().unwrap_or_else(|| n.key.clone());
            let with = crate::domain::chat::Situation { facts: vec![format!("Just happened: “{name}” recorded a {seconds:.0} s take")], files: vec![path] };
            if let Err(e) = self.part_event(project_id, &n.key, "recorded", with) {
                if let Some(slot) = self.takes.by_ref.get_mut(&key) {
                    slot.line = e;
                }
            }
        }
        if intent.is_none() {
            if let Some((sid, input)) = self.web.take_go() {
                intent = Some(Intent::Page(PageIntent::SetSurfaceUrl { surface_id: sid, input }));
            } else if let Some((sid, x, y, w, h)) = self.web.take_frame() {
                intent = Some(Intent::Page(PageIntent::SetSurfaceFrame { surface_id: sid, x, y, w, h }));
            }
        }
        // 描いた後: 枠の画面座標と押された一手を面へ（Host は runtime を借りているので、ここで流す）
        #[cfg(feature = "native_direct")]
        for (sid, at, action) in placed {
            self.surfaces.on_frame(project_id, &sid, at, action, std::time::Instant::now());
        }
        // W16-3: 別の Page へ移った面の窓を、この Page の上に浮かせない
        #[cfg(feature = "native_direct")]
        self.surfaces.hide_unseen(project_id, page_id);
        #[cfg(not(feature = "native_direct"))]
        let _ = placed;
        // G92: 見せる部品は描いた 1 frame だけ（拡大した Mirror の下に隠れて描かなかった部品を、後で急に送らない）
        self.term_views.reveal = None;
        #[cfg(feature = "mirror")]
        self.save_mirror_choices(page.app_ref.as_deref());
        // RQ4（RQ-019）: 面の帯の menu で選んだ窓を 1 frame に 1 つ Project へ（他の手を押した frame は次の frame に回す）
        #[cfg(feature = "mirror")]
        if intent.is_none() {
            if let Some((surface_id, title, bundle_id)) = self.mirror.as_mut().and_then(|m| m.take_chosen_windows().pop()) {
                intent = Some(Intent::Page(PageIntent::ChooseWindow { surface_id, title, bundle_id }));
            }
        }
        // MIRROR-HYBRID-1101 追記 246 の 3: Mirror の四隅で引き終わった枠を Project へ（⌘Z で戻る・他の手を押した frame は次の frame に回す）
        #[cfg(feature = "mirror")]
        if intent.is_none() {
            if let Some((surface_id, f)) = self.mirror.as_mut().and_then(|m| m.take_frames().pop()) {
                intent = Some(Intent::Page(PageIntent::SetSurfaceFrame { surface_id, x: f.x, y: f.y, w: f.w, h: f.h }));
            }
        }
        self.draw_web_slots(ui, &p, &page);
        intent
    }

    /// G94: 見せる手（Add terminal / Add browser / Show terminal）の前に、拡大した Mirror を戻す（拡大の間は他の部品が描かれず reveal が捨てられる）。
    fn unzoom_mirror(&mut self) {
        #[cfg(feature = "mirror")]
        if let Some(m) = &mut self.mirror {
            m.unzoom();
        }
    }

    /// 殻の毎 frame の終わり（Mirror の Page を描かなかった frame でも）。Mirror の Page を離れたら App Nap を止めるのを放す（W16M-1b）。
    #[cfg(feature = "mirror")]
    pub fn end_frame(&mut self) {
        if let Some(m) = &mut self.mirror {
            m.end_frame();
        }
    }

    /// G91: Shibubu を閉じる（殻の `on_exit`）: Mirror の面がしまった本物の窓を元の位置・大きさへ戻す（eframe は終了で app を drop しない事がある）。
    #[cfg(feature = "mirror")]
    pub fn release_mirrors(&mut self) {
        if let Some(m) = &mut self.mirror {
            m.release_sources();
        }
    }

    /// Mirror の面を、保存した同意（`mirror-consent`）と窓の形（`mirror-window`・UI-5m）で作る。閉じた・しまった面は帯のまま始まり、窓を探さない。
    /// MIRROR-AUTO-1101 B: `mirror-consent` の取り消しの印（`revoke_mirror_consent`）の app は自動で同意しない。
    #[cfg(feature = "mirror")]
    fn mirror_pages(&self, helper: std::path::PathBuf, workspace: Option<std::path::PathBuf>) -> crate::ui::mirror_view::MirrorPages {
        use crate::surface::mirror::{ConsentRecord, CONSENT_UI_STATE_SESSION};
        use crate::ui::mirror_view::{MirrorPages, WINDOW_UI_STATE_SESSION};
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let rows = |session: &str| store.list_ui_state(session).unwrap_or_default().into_iter();
        let mut pages = MirrorPages::new(helper, rows(CONSENT_UI_STATE_SESSION).filter_map(|(_, v)| serde_json::from_value::<ConsentRecord>(v).ok()), workspace);
        pages.restore_windows(rows(WINDOW_UI_STATE_SESSION).filter_map(|(id, v)| Some((id, serde_json::from_value(v).ok()?))));
        pages.restore_revoked(rows(CONSENT_UI_STATE_SESSION).filter(|(_, v)| v["revoked"] == true).map(|(b, _)| b));
        pages
    }

    /// 人が押した同意と替えた窓の形（UI-5m）を ui_state へ（保存できなければ 1 行。今の process の中では効いている）。
    #[cfg(feature = "mirror")]
    fn save_mirror_choices(&mut self, app: Option<&str>) {
        use crate::surface::mirror::CONSENT_UI_STATE_SESSION;
        use crate::ui::mirror_view::WINDOW_UI_STATE_SESSION;
        let Some(m) = self.mirror.as_mut() else { return };
        let (granted, shapes) = (m.take_granted(), m.take_window_changes());
        for r in granted {
            if let Err(e) = self.save_mirror_state(app, CONSENT_UI_STATE_SESSION, &r.bundle_id, &r) {
                self.page_line = Some((RegionState::Error, format!("Couldn't remember your Mirror choice ({e}). Shibubu asks again after a restart.")));
            }
        }
        // 1 frame に同じ面が 2 回替わっても順に書くので後の形が残る
        for (id, w) in shapes {
            if let Err(e) = self.save_mirror_state(app, WINDOW_UI_STATE_SESSION, &id, &w) {
                self.page_line = Some((RegionState::Error, format!("Couldn't remember how the mirror was left ({e}). It opens again after a restart.")));
            }
        }
    }

    /// Mirror の選択を `ui_state(session, key)` に保存する（同意 = `mirror-consent` と bundle id・W16M-0 の決定 7-5／窓の形 = `mirror-window` と surface id・UI-5m）。
    /// `ui_state` の session_id は sessions の行を要る（FK・sessions は app に属す）ので、無ければ `app`（押した Page の app）の行として作る。
    // ponytail: その app を消すと同意と窓の形も消える（CASCADE）。app から外すなら ui_state と別の表に
    #[cfg(feature = "mirror")]
    fn save_mirror_state(&self, app: Option<&str>, session: &str, key: &str, v: &impl serde::Serialize) -> Result<(), String> {
        let v = serde_json::to_value(v).map_err(|e| e.to_string())?;
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if store.get(Table::Sessions, session).map_err(|e| e.to_string())?.is_none() {
            let app = app.ok_or("This page has no app")?;
            store.put(Table::Sessions, session, Some(app), &serde_json::json!({ "app_id": app })).map_err(|e| e.to_string())?;
        }
        store.put_ui_state(session, key, &v).map_err(|e| e.to_string())
    }

    /// W12W-1: この Page の Web の面（`source_surface` の `surface_ref` が指す webview・Url の面）の key と URL。
    fn web_surfaces_on(p: &ProjectDefinitionV1, page: &PageDefinition) -> Vec<(String, String)> {
        let refs: Vec<&str> = page.root.nodes().into_iter().filter(|n| n.component == "source_surface").filter_map(|n| n.props.get("surface_ref")?.as_str()).collect();
        p.surface_specs
            .iter()
            .filter(|s| refs.contains(&s.id.as_str()) && crate::project::web::web_spec(s, None).is_some())
            .filter_map(|s| match &s.target {
                project::SurfaceTarget::Url { url } => Some((crate::project::web::key(&p.id, &s.id), url.clone())),
                _ => None,
            })
            .collect()
    }

    /// W12W-1: Web の面の帯（detail）・書き出した file の保存の欄・直前の保存の結果。`render_page` の前に描く（WebView の矩形の上）。
    fn draw_web_rows(&mut self, ui: &mut egui::Ui, p: &ProjectDefinitionV1, page: &PageDefinition) {
        let repo = repo_dir(p);
        let t = theme::tokens(ui.ctx());
        for (key, _) in Self::web_surfaces_on(p, page) {
            if let Some(detail) = self.web.detail(&key) {
                theme::label(ui, theme::Type::Helper, detail, t.text_secondary);
            }
            if let Some(pending) = self.web.pending_save(&key) {
                // 次の download が届いた: 前の「置きました」は古い
                if self.web_saved.as_ref().is_some_and(|(k, _)| *k == key) {
                    self.web_saved = None;
                }
                if crate::project::web::save_row(ui, pending) {
                    if let Ok(path) = self.web.save(&key, repo.as_deref()) {
                        let shown = repo.as_deref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(&path).display().to_string();
                        self.web_saved = Some((key.clone(), format!("Written to {shown} · read back from disk")));
                    }
                }
            }
            if let Some((_, line)) = self.web_saved.as_ref().filter(|(k, _)| *k == key) {
                theme::label(ui, theme::Type::Helper, line, t.text_secondary);
            }
        }
    }

    /// W12W-1: 矩形の中の状態（読み込み中・失敗と次の一手）。`render_page` の後に描く（`source_surface` が矩形を塗るので、その上に）。
    /// 頁が出ている（Live）間は WebView が矩形を覆うので、ここは何も描かない。
    fn draw_web_slots(&mut self, ui: &mut egui::Ui, p: &ProjectDefinitionV1, page: &PageDefinition) {
        use crate::surface::WebState;
        let t = theme::tokens(ui.ctx());
        for (key, url) in Self::web_surfaces_on(p, page) {
            let host = crate::surface::host_of(&url).unwrap_or_default();
            let Some(slot) = self.web.slots().iter().find(|(id, _)| *id == key).map(|(_, f)| egui::Rect::from_min_size(egui::pos2(f.x as f32, f.y as f32), egui::vec2(f.w as f32, f.h as f32))) else { continue };
            let line = match self.web.state(&key) {
                Some(WebState::Live) => None,
                None | Some(WebState::Starting | WebState::Loading) => Some((format!("Loading {host}"), false)),
                // W12W-1b: 終端でない。次の一手は下の button（WebView の在る build は「もう一度」= 同じ WebView で読み直す・開けなかった面は作り直す、と「browser で開く」）
                Some(WebState::Recoverable(why)) => Some((format!("Couldn't open {host} here · {why}"), true)),
            };
            if let Some((line, handoff)) = line {
                let inner = slot.shrink(theme::SP_4);
                // TEXT-SELECT-1101: painter の字は選べないので、枠の中の子の Ui に選べる label（親の cursor は動かさない）
                ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)))
                    .add(egui::Label::new(egui::RichText::new(&line).font(egui::FontId::proportional(14.0)).color(t.text_secondary)).wrap());
                if handoff {
                    // 帯の 2 つの素の button は helper と同じ形（Page の中 = 線なし・owner 209・scope の外の欄の枠は残す）
                    ui.scope(|ui| {
                        theme::native_buttons(ui);
                        let mut x = 0.0;
                        if crate::surface::WEBVIEW_BUILT {
                            let b = egui::Rect::from_min_size(inner.left_top() + egui::vec2(0.0, 28.0), egui::vec2(120.0, 28.0));
                            let r = ui.put(b, egui::Button::new("Try again"));
                            ui.ctx().accesskit_node_builder(r.id, |n| n.set_author_id(format!("{}:{}", crate::project::web::RETRY_AX, crate::project::web::sid_of(&key))));
                            if r.clicked() {
                                self.web.retry(&key);
                            }
                            x = 128.0;
                        }
                        let b = egui::Rect::from_min_size(inner.left_top() + egui::vec2(x, 28.0), egui::vec2(160.0, 28.0));
                        if ui.put(b, egui::Button::new("Open in browser")).clicked() {
                            let to = self.web.handoff_url(&key).unwrap_or(url.clone());
                            if let Err(e) = crate::surface::open_in_browser(&to) {
                                self.web_saved = Some((key.clone(), e));
                            }
                        }
                    });
                }
            }
        }
    }

    /// Project の id から app の id（W2 の Project は全部 app 1 つを包んだ物: id = `project.{app}`・`ProjectDefinitionV1::wrap`）。
    pub fn app_of_project(&self, project_id: &str) -> Option<String> {
        self.apps.iter().map(|o| o.rt.revision().definition().id.clone()).find(|id| project_id_of(id) == project_id)
    }

    /// Project の保存した行が在るか（開けたかは問わない）。開いている Project と、`apps` の行は在るが今回は開けなかった Project
    /// （定義が今の検査を通らない = `start` の broken）が true・行が無い id と `project.` で始まらない key は false・store が読めない時は true（捨てない側）。
    /// DB を開けず memory の store で動いている起動（`saves` が false）も true（読めていないだけで消えたのではない・RQ1 反証の直し）。
    /// 例（app.rs の起動）: `drafts.retain(|k, v| !v.trim().is_empty() && (k == "home" || generated.project_saved(k)))`。保証しない: 開ける事（`app_of_project`）。
    /// Wave: UI-4b
    pub fn project_saved(&self, project_id: &str) -> bool {
        if self.memory_fallback || self.app_of_project(project_id).is_some() {
            return true;
        }
        let Some(app_id) = project_id.strip_prefix("project.") else { return false };
        self.store.lock().unwrap_or_else(|e| e.into_inner()).get(Table::Apps, app_id).map_or(true, |row| row.is_some())
    }

    /// Project を DB に保存しているか（false = `open` が DB を開けず memory の store・閉じたら消える・保存した Project は今回読めていない）。
    /// 呼ぶ側の例（app.rs の起動の `reopen_last_project`）: false なら最後の Project を「消えた」と言わず、覚えた id も消さない。
    /// Wave: RQ1
    pub fn saves(&self) -> bool {
        !self.memory_fallback
    }

    /// RQ2（RQ-005）: Project の名前を替える（Home の行の `•••` → `Rename` と header の `{name} ⌄` → `Rename`・どちらも 1 つの欄 `Shell::renaming` で打つ）。
    /// `ProjectPatchOp::RenameProject` を 1 revision で当てる（Undo できる・Pages と folder と履歴は変えない）。空の名前 = Err `A project needs a name`。
    /// 例（app.rs の `Intent::RenameProject { project, name }`）: `self.generated.rename_project(&project, &name)?`。保証しない: 名前が他と重ならない事（同じ名前を許す）。
    /// Wave: RQ2
    pub fn rename_project(&mut self, project_id: &str, name: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("A project needs a name".into());
        }
        let (app, _, hash) = self.project(project_id)?;
        self.apply_project(&app, &hash, &[ProjectPatchOp::RenameProject { name: name.trim().to_owned() }], RevisionOrigin::DirectEdit).map(|_| ())
    }

    /// RQ2（RQ-005）: Project を複製する（`Duplicate`）。定義と Pages と面の中身を新しい id（`project.{新しい app}`・名前 `{name} copy`）へ写し、開いた View を返す。
    /// 写さない: run の履歴・承認待ち・Terminal の session・面の login（web の store は空から）。例: `let v = self.generated.duplicate_project(&project)?; self.go(v);`
    /// 保証する: 元の Project を変えない・利用者の folder と file を写さない（folder は元と同じ所を指す）。Err = 画面の 1 行。
    /// Wave: RQ2
    pub fn duplicate_project(&mut self, project_id: &str) -> Result<View, String> {
        let (app, p, _) = self.project(project_id)?;
        let def = self.find(&app).ok_or("This project isn't open. Reopen it from Home")?.rt.revision().definition().clone();
        let new_app = self.free_app_id(&app, false).ok_or("Too many copies of this project · Delete one first")?;
        let (mut new_def, mut np) = moved_ids(&app, &new_app, &def, &p)?;
        let name = format!("{} copy", p.name);
        new_def.title = Some(name.clone());
        (np.name, np.revision) = (name, 1);
        self.open_new(new_def)?;
        let saved_err = |e: storage::StoreError| format!("Couldn't save the copy ({e}). Try again");
        let json = serde_json::to_value(&np).map_err(|e| e.to_string())?;
        {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.immediate(saved_err, |store| {
                // 面の中身（local の data・Page の打ちかけ・配置）は写す・Terminal の panel は写さない（Terminal の session は新しい Project で起こし直す）
                for (k, v) in store.list_ui_state(&session_of(&app)).map_err(saved_err)? {
                    // PROJECT-DIR-1101: 自分の folder も写さない（複製は自分の folder を作る）
                    // ACCOUNTS-1101 2 段目: 選んだ AI の account も写さない（この Mac の account）
                    if k != PANEL_KEY && k != FOLDER_KEY && k != ACCOUNT_KEY {
                        store.put_ui_state(&session_of(&new_app), &k, &v).map_err(saved_err)?;
                    }
                }
                store.put(Table::Projects, &np.id, None, &json).map_err(saved_err)?;
                store.put_project_revision(&np.id, 1, &json, None, &serde_json::json!([]), RevisionOrigin::Generate.as_str()).map_err(saved_err)
            })?;
        }
        Ok(View::Project { project: np.id.clone(), page: np.pages[0].id.clone() })
    }

    /// RQ2・RQ3: 新しい app の id（`as_is` = 元の id が空いていればそれ〈Import〉・次に `{app}-copy`・`{app}-copy-{n}`〈Duplicate はここから〉）。
    /// 消した Project の id は次の起動で web の store を消すまで使わない（同じ id の複製に、消した Project の store の中身が出る・RQ2 反証の直し）。
    fn free_app_id(&self, app: &str, as_is: bool) -> Option<String> {
        let taken = |id: &str| {
            self.find(id).is_some()
                || self.web.queued_for_removal(&project_id_of(id))
                || self.store.lock().unwrap_or_else(|e| e.into_inner()).get(Table::Apps, id).map_or(true, |r| r.is_some())
        };
        let copies = (1..100).map(|n| if n == 1 { format!("{app}-copy") } else { format!("{app}-copy-{n}") });
        as_is.then(|| app.to_owned()).into_iter().chain(copies).find(|id| !taken(id))
    }

    /// RQ3: 何かが動いている（Terminal の run・Engine の run と承認待ち・model の編集。`project_id` が None なら全 Project と作成中の一言も）。
    /// 動いている間は戻さない・消さない（`delete_project`・`restore_backup`・`delete_my_data` の `Stop the run first`）。
    fn working(&self, project_id: Option<&str>) -> bool {
        use crate::ui::engine_view::EngineView;
        let mine = |p: &str| project_id.is_none_or(|x| x == p);
        let running = self.runs.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(p, s)| mine(p) && matches!(s, RunState::Running { .. }));
        let engine = self.engine_job.as_ref().is_some_and(|j| mine(&j.project_id))
            || self.engine_views.iter().any(|(p, v)| mine(p) && matches!(v, EngineView::Running { .. } | EngineView::Approve { .. } | EngineView::Consent { .. }));
        let model = self.model_job.as_ref().is_some_and(|j| match &j.what {
            JobWhat::Edit { project_id: p, .. } => mine(p),
            JobWhat::Create { .. } => project_id.is_none(),
        });
        running || engine || model
    }

    /// RQ2（RQ-005・RQ-007）: Project を消す（Home の `Delete…` → 行の中の確認 `Delete “{name}”? Its pages and history are deleted. The folder and your files stay.` → `Delete`）。
    /// 消す: Shibubu の DB のその Project の行（apps と projects の行と revision・ui_state・surface_receipts・terminal_sessions）と、Project ごとの web の store（`PageWebViews::forget_project`）。
    /// 消さない: 利用者の folder と file・Project の前から在った app・共有の web の store（Browser）・Keychain・他の Project。
    /// run が動いている・承認を待っている間は Err `Stop the run first`（消さない）。例: `self.generated.delete_project(&id)?; self.store.ui.pinned_projects.retain(|p| p != &id);`
    /// 保証しない: 取り消し（Undo の対象にしない・だから確認を 1 回挟む）。
    /// Wave: RQ2
    pub fn delete_project(&mut self, project_id: &str) -> Result<(), String> {
        let app = self.app_of_project(project_id).ok_or("This project couldn't be found")?;
        if self.working(Some(project_id)) {
            return Err("Stop the run first".into());
        }
        // 戻せない事（Terminal を閉じる・web の store を消す）は DB の削除が通った後だけ（通らなければ「Try again」で Project も store も残る・反証の直し）
        {
            let gone = |e: storage::StoreError| format!("Couldn't delete the project ({e}). Try again");
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.immediate(gone, |store| {
                // projects の子（revision・surface_receipts）と apps の子（revision・session・ui_state・run）は CASCADE
                store.delete(Table::Projects, project_id).map_err(gone)?;
                store.delete_project_rows(project_id).map_err(gone)?;
                store.delete(Table::Apps, &app).map_err(gone).map(|_| ())
            })?;
        }
        self.rt.forget(&app);
        // ENGINE-AUDIT W2（節 2「Project をまたぐ」）: この Project の保存した手順（data dir の workflows/ の `ui` = この Project）も消す
        if let Some(dir) = self.engine.as_ref().map(|e| e.data_dir.join("workflows")) {
            forget_project_workflows(&dir, project_id);
        }
        self.apps.retain(|o| o.rt.revision().definition().id != app);
        self.routers.remove(project_id);
        self.engine_views.remove(project_id);
        self.panels.remove(project_id);
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).remove(project_id);
        let _ = self.terminals.close_project(project_id);
        if let Some(dir) = self.terminals.restore_dir() {
            crate::terminal::remove_snapshots(&dir, project_id, None); // TERM-RESTORE-1101: 消した Project の terminal の画面
        }
        // DB からは消えた: ここで Err を返すと「消せなかった」と嘘になるので、store の消し直しの失敗は forget_project の queue（次の起動）に任せる
        let _ = self.web.forget_project(project_id);
        Ok(())
    }

    /// RQ2（RQ-008）: ✦ rail の `About` の下に足す Project の設定の行（呼ぶ側が `project_about_rows` の後ろに繋ぐ・G94 の行は変えない）。
    /// 行: `Folder access`（Read only / Read and write）・`Web sign-ins`（Kept for this project / Shared with Browser）・`Approvals`（Ask before sending / Ask before each step）・
    /// `AI`（その Project が使う AI の account の名前・既定は `System default`）。例: `rows.extend(self.generated.project_settings_rows(&project)?)`。
    /// 保証する: 何も書かない・押せない行は値と理由だけ（変える口は本体で Intent に足す）。Err = 開いていない・見つからない Project の 1 行。
    /// Wave: RQ2
    pub fn project_settings_rows(&self, project_id: &str) -> Result<Vec<(String, String)>, String> {
        let (_, p, _) = self.project(project_id)?;
        // PROJECT-DIR-1101: repo の無い Project も自分の folder を持つ（「folder が無い」と言わない）
        let folder = match (repo_dir(&p).is_some(), self.saved_folder(&p).is_some()) {
            (true, _) => "Read and write · the Terminal works in this folder",
            (false, true) => "Read and write · the Terminal works in this project's own folder",
            (false, false) => "No folder · this project doesn't use one",
        };
        Ok(vec![
            ("Folder access".into(), folder.into()),
            ("Web sign-ins".into(), "Kept for this project · Browser pages share one sign-in".into()),
            ("Approvals".into(), "Ask before sending".into()),
        ])
    }

    /// RQ3（RQ-010・RQ-011・RQ-012・RQ-014）: Settings › Data & Recovery の中身を裏の thread で読み直す（data dir と Project ごとの web の store の大きさ・`BACKUPS_DIR` の
    /// 世代と前の版の `sessions.json.corrupt-*`・`RESTORE_MARKER`・`look_in` の直下の `.shibubu`）。`force` = false は前に読んでから 30 秒未満なら何もしない
    /// （節を開いている間 app.rs が毎 frame 呼んでよい）・true は節を開いた時と各 action の後。読んだ結果は次の frame から `data_summary()`。
    /// 呼ぶ側の例（app.rs の `View::Settings(SettingsSection::Data)` の腕）: `self.generated.refresh_data(&[home.join("Downloads"), home.join("Desktop")], now, false)`
    /// 保証する: UI の thread で disk を歩かない・答えを受け取る読みは 1 本（force は読み中の答えを捨てる）・何も書かない・`data_dir` が None（試験の `start`）なら読まず false。
    /// 戻り値 = この呼び出しで読みを始めたか。
    /// 保証しない: Shibubu の外（利用者の folder・元の app・`~/.claude`）の大きさ。
    /// Wave: RQ3
    pub fn refresh_data(&mut self, look_in: &[std::path::PathBuf], now: i64, force: bool) -> bool {
        use std::sync::mpsc::TryRecvError;
        if let Some(rx) = &self.data_job {
            match rx.try_recv() {
                Ok(d) => (self.data, self.data_job) = (Some(d), None),
                Err(TryRecvError::Empty) if !force => return false,
                // force（action の後）: 読み中の答えは action の前の中身なので捨てて読み直す（捨てた thread は送れずに終わる・何も書かない）
                Err(_) => self.data_job = None,
            }
        }
        let Some(dir) = self.data_dir.clone() else { return false };
        if !force && self.data.as_ref().is_some_and(|d| now - d.checked_at < 30) {
            return false;
        }
        // 名前は UI の thread で（開いた app と DB の行）・大きさと世代と file は裏で
        let projects: Vec<(String, String, String)> = self
            .apps
            .iter()
            .map(|o| {
                let app = o.rt.revision().definition().id.clone();
                (project_id_of(&app), app, self.name_of(o))
            })
            .collect();
        let (look_in, (tx, rx), wake) = (look_in.to_vec(), std::sync::mpsc::channel(), self.terminals.wake());
        let spawned = std::thread::Builder::new().name("data-summary".into()).spawn(move || {
            let _ = tx.send(read_data(&dir, &projects, &look_in, now));
            if let Some(wake) = wake {
                wake();
            }
        });
        if spawned.is_err() {
            return false;
        }
        self.data_job = Some(rx);
        true
    }

    /// RQ3: 直前に読んだ Data & Recovery の中身（None = まだ読んでいない・読み中）。例: `crate::settings::draw(.., self.generated.data_summary(), ..)`。
    /// Wave: RQ3
    pub fn data_summary(&self) -> Option<&DataSummary> {
        self.data.as_ref()
    }

    /// RQ3（RQ-011・`Back up now`）: 今の DB（`Store::back_up_to`）と `sessions.json` の写しを `BACKUPS_DIR` に `manual` の 1 世代として書き、世代の id を返す。
    /// app.rs は呼ぶ前に設定を保存する（`sessions.json` の写しに今の設定が入る）。例: `match g.back_up_now() { Ok(_) => "Backed up · just now", Err(why) => why }`。
    /// 保証する: 今の data を変えない・途中で失敗したら書きかけの世代の dir を消す。Err = 画面の 1 行（`Couldn't back up ({理由}) · Try again`・
    /// memory の store の起動は `Your projects aren't being saved right now, so there's nothing to back up`）。保証しない: 速さ（DB の大きさに比例）。
    /// Wave: RQ3
    pub fn back_up_now(&mut self) -> Result<String, String> {
        let dir = self.data_dir.clone().filter(|_| !self.memory_fallback).ok_or("Your projects aren't being saved right now, so there's nothing to back up")?;
        let id = format!("{}-manual", stamp(unix_now()));
        let set = dir.join(BACKUPS_DIR).join(&id);
        if set.exists() {
            return Err("A backup was made a moment ago · Try again in a second".into());
        }
        let wrote = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .back_up_to(&set.join(DB_FILES[0]))
            .map_err(|e| e.to_string())
            .and_then(|()| copy_if_there(&dir.join(SETTINGS_FILE), &set.join(SETTINGS_FILE)).map_err(|e| e.to_string()));
        if let Err(why) = wrote {
            let _ = std::fs::remove_dir_all(&set);
            return Err(format!("Couldn't back up ({why}) · Try again"));
        }
        Ok(id)
    }

    /// RQ3（RQ-011・`Restore`）: `Some(id)` = その世代を次の起動で戻す予約（`RESTORE_MARKER` に id を書く・戻すのは `open` の ①）・`None` = 予約を消す。
    /// 戻り値 = 画面の 1 行（`Restores when Shibubu reopens · Quit and reopen Shibubu` / `Restore cancelled`）。run・Engine・model の編集が動いている間は
    /// Err `Stop the run first`（`delete_project` と同じ）・無い id・読めない世代は Err の 1 行。
    /// 保証する: 押しただけでは今の data を変えない・戻す時に今の data を `before-restore` の世代に残す（戻した事も戻せる）。
    /// 保証しない: 戻した後に、予約の後で作った Project が残る事（`before-restore` から戻せる・画面の 1 行がそう言う）。
    /// Wave: RQ3
    pub fn restore_backup(&mut self, id: Option<&str>) -> Result<String, String> {
        let dir = self.data_dir.clone().ok_or("Backups aren't kept for this window")?;
        let marker = dir.join(RESTORE_MARKER);
        let Some(id) = id else {
            return match std::fs::remove_file(&marker) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Couldn't cancel the restore ({}) · Try again", e.kind())),
                _ => Ok("Restore cancelled".into()),
            };
        };
        if self.working(None) {
            return Err("Stop the run first".into());
        }
        backup_files(&dir, id).ok_or("That backup isn't there anymore")?;
        std::fs::write(&marker, format!("{id}\n")).map_err(|e| format!("Couldn't plan the restore ({}) · Try again", e.kind()))?;
        Ok("Restores when Shibubu reopens · Quit and reopen Shibubu".into())
    }

    /// RQ3（RQ-012・`Export…`）: Project を `dest_dir/{Project の名前}.shibubu`（在れば `{名前} 2.shibubu`…）に `ProjectExport` の JSON で書き、file の path を返す。
    /// Workflow と接続の参照は `PageEngine::export_workflows`（Engine が無い起動は空・接続の `uri` は Engine が何を渡しても書かない）。Terminal の席の shell と作業 folder は
    /// 空で書く（この Mac の path・取り込む側が自分の物を入れる）。例（app.rs・置き場は `~/Downloads`・HOME が無ければ app.rs が断る）:
    /// `let file = self.generated.export_project(&project, &downloads)?; self.message = format!("Exported {} · Reveal in Finder", file_name(&file));`
    /// 保証する: 何も変えない（DB・Project・web の store）・file を上書きしない・`ProjectExport` の doc の「入れない」物を入れない。Err = 開いていない Project・書けない置き場の 1 行。
    /// Wave: RQ3
    pub fn export_project(&self, project_id: &str, dest_dir: &Path) -> Result<std::path::PathBuf, String> {
        let (app, p, _) = self.project(project_id)?;
        let def = self.find(&app).ok_or("This project isn't open. Reopen it from Home")?.rt.revision().definition().clone();
        // 面の中身と配置と打ちかけ。Terminal の panel と、読んだ resource の中身（取り込んだ先で読み直す）は入れない
        let state: BTreeMap<String, Value> = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .list_ui_state(&session_of(&app))
            .map_err(|e| format!("Couldn't read the project ({e}) · Try again"))?
            .into_iter()
            .filter(|(k, _)| k != PANEL_KEY && k != FOLDER_KEY && k != ACCOUNT_KEY && !k.starts_with(&project::resource_source_id("")))
            .collect();
        let (workflows, mut connections) = self.engine.as_ref().map(|e| e.engine.export_workflows(project_id)).unwrap_or_default();
        // 接続の参照は名前と種類だけ（Engine が URL を渡しても書かない = URL の query の鍵を持ち出さない）
        for c in &mut connections {
            c.uri = None;
        }
        // Terminal の shell と作業 folder はこの Mac の物（利用者の名前の入った path）なので書かない。取り込む側が自分の Mac の物を入れる（`import_project`）
        let mut p = p;
        for t in &mut p.terminal_specs {
            (t.shell, t.working_directory) = (String::new(), String::new());
        }
        let bundle = ProjectExport { schema: PROJECT_EXPORT_SCHEMA.into(), project: p, app: def, state, workflows, connections };
        let text = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
        let cant = |e: std::io::Error| format!("Couldn't write the file ({}) · Try again", e.kind());
        std::fs::create_dir_all(dest_dir).map_err(cant)?;
        // file の名前に使えない字は替える（`/` と `:`・先頭の `.`）。在る file は上書きしない（`{name} 2.shibubu`…）
        let name: String = bundle.project.name.trim().chars().map(|c| if matches!(c, '/' | ':' | '\\') || c.is_control() { '-' } else { c }).collect();
        let name = match name.trim_start_matches('.') { "" => "Project", n => n }.to_owned();
        for n in 1..1000 {
            let file = dest_dir.join(if n == 1 { format!("{name}.{PROJECT_FILE_EXT}") } else { format!("{name} {n}.{PROJECT_FILE_EXT}") });
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&file) {
                Ok(mut f) => {
                    use std::io::Write;
                    return f.write_all(text.as_bytes()).map(|()| file.clone()).map_err(|e| {
                        let _ = std::fs::remove_file(&file);
                        cant(e)
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(cant(e)),
            }
        }
        Err("Too many files with this name · Move some out of the folder".into())
    }

    /// RQ3（RQ-012・`Import`）: `.shibubu` の file を読み、新しい id の Project として保存して開いた View を返す（id は `duplicate_project` と同じ決め方 = 重なれば
    /// `-copy[-n]`・名前はそのまま）。Workflow は `PageEngine::import_workflows` で `ui` を新しい id に替えて保存し、この Mac で使えない接続があれば Page に
    /// `{name} isn't connected on this Mac · Connect`（Connect = Connections の画面）の 1 行。例: `let v = self.generated.import_project(&file)?; self.go(v);`
    /// 保証する: 元の file を変えない・既存の Project を上書きしない・検査（`validate_project` と app の検査）を通らない定義は保存しない。
    /// Err = 画面の 1 行（読めない・`.shibubu` でない・新しい版の file = `This file was made by a newer Shibubu · Update Shibubu first`・保存できない）。
    /// Wave: RQ3
    pub fn import_project(&mut self, file: &Path) -> Result<View, String> {
        let not_ours = "This isn't a Shibubu project file";
        let text = std::fs::read_to_string(file).map_err(|e| format!("Couldn't read the file ({}) · Try again", e.kind()))?;
        let raw: Value = serde_json::from_str(&text).map_err(|_| not_ours)?;
        // 版を先に見る（新しい版の file は未知の欄で読めないので、読む前に言う）
        match raw.get("schema").and_then(Value::as_str) {
            Some(PROJECT_EXPORT_SCHEMA) => {}
            Some(s) if s.starts_with("shibubu.project/") => return Err("This file was made by a newer Shibubu · Update Shibubu first".into()),
            _ => return Err(not_ours.into()),
        }
        let bundle: ProjectExport = serde_json::from_value(raw).map_err(|_| not_ours)?;
        let app = bundle.app.id.clone();
        if project_id_of(&app) != bundle.project.id {
            return Err(not_ours.into());
        }
        let new_app = self.free_app_id(&app, true).ok_or("Too many copies of this project · Delete one first")?;
        let (def, mut np) = moved_ids(&app, &new_app, &bundle.app, &bundle.project)?;
        // Terminal の席は、file が言う実行 file と folder でなく、この Mac の利用者の shell と Project の folder（PROJECT-DIR-1101: 取り込んだ名前で作る）で起こす
        // （他人が作った file の shell を確認なしに起こさない・書き出した Mac の path は無い）。起動の 1 行（`command`）も持ち込まない
        // （L7 の反証の直し: 残すと Page を開いただけで file の書いた command が shell に打たれる）
        let cwd = self.terminal_cwd(&np);
        let dropped = np.terminal_specs.iter().any(|t| t.command.is_some());
        for t in &mut np.terminal_specs {
            (t.shell, t.working_directory, t.command) = (user_shell(), cwd.clone(), None);
        }
        // 検査の理由（内部の語）は log へ・画面は英語の 1 行
        project::validate_project(&np, &[&def]).map_err(|errs| {
            eprintln!("shibubu: import {}: {}", file.display(), errs.join(" / "));
            "This project file is damaged or incomplete · Export it again".to_owned()
        })?;
        self.open_new(def)?;
        let saved_err = |e: storage::StoreError| format!("Couldn't save the project ({e}) · Try again");
        let json = serde_json::to_value(&np).map_err(|e| e.to_string())?;
        let saved = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.immediate(saved_err, |store| {
                for (k, v) in bundle.state.iter().filter(|(k, _)| *k != PANEL_KEY && *k != FOLDER_KEY && *k != ACCOUNT_KEY) {
                    store.put_ui_state(&session_of(&new_app), k, v).map_err(saved_err)?;
                }
                store.put(Table::Projects, &np.id, None, &json).map_err(saved_err)?;
                store.put_project_revision(&np.id, 1, &json, None, &serde_json::json!([]), RevisionOrigin::Generate.as_str()).map_err(saved_err)
            })
        };
        if let Err(why) = saved {
            // 開いた app を戻す（保存できなかった Project を一覧に残さない）
            self.rt.forget(&new_app);
            self.apps.retain(|o| o.rt.revision().definition().id != new_app);
            let _ = self.store.lock().unwrap_or_else(|e| e.into_inner()).delete(Table::Apps, &new_app);
            return Err(why);
        }
        let view = View::Project { project: np.id.clone(), page: np.pages[0].id.clone() };
        // Workflow と接続: 取り込めない Workflow・この Mac で使えない接続は Page の 1 行（Project の取り込みは止めない）。
        // 呼ぶ側はこの後 `route` で開く = `open_project` が page_line を作り直すので、開いた時に置く 1 行として渡す（`opened_line`）
        let line = match self.engine.as_ref().map(|e| e.engine.import_workflows(&np.id, &bundle.workflows)) {
            None => None,
            Some(Ok(missing)) if missing.is_empty() => None,
            // 末尾の `· Connect` は draw_page が Connections へ移る button にする
            Some(Ok(missing)) => Some((RegionState::Disconnected, format!("{} isn't connected on this Mac · Connect", missing.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")))),
            Some(Err(why)) => Some((RegionState::Partial, format!("The project's steps couldn't be imported · {why}"))),
        };
        // 捨てた起動の 1 行は黙らずに言う（Connect を末尾に残すので前に足す）
        let note = "Terminal start commands in the file weren't imported or run";
        let line = match (line, dropped) {
            (Some((s, l)), true) => Some((s, format!("{note} · {l}"))),
            (None, true) => Some((RegionState::Partial, note.to_owned())),
            (line, false) => line,
        };
        self.opened_line = line.map(|l| (np.id.clone(), l));
        Ok(view)
    }

    /// RQ3（RQ-013・`Delete my data…` の `Delete`）: その種類（`ui::shell::DataKind` の doc の持ち物）を消し、画面の 1 行（`Deleted {label}`）を返す。
    /// この関数が消す: DB の行・data dir の file と dir・共有の web の store（`Connections`）。GitHub の token は Shibubu が持たない（env の `GITHUB_TOKEN`）ので消す物が無い。
    /// app.rs の腕がこの関数の Ok の後に消す: `UiSettings` の側（`Intent::DeleteMyData` の doc）と AI の account の鍵（`llm_cli::remove_account`・generated は llm_cli を
    /// 名指せない。先に呼ぶと下の「動いている間は消さない」より前に戻せない物を消す）。
    /// run・Engine・model の編集が動いている間は Err `Stop the run first`。例: `self.message = self.generated.delete_my_data(kind).unwrap_or_else(|e| e);`
    /// 保証する: 消えた事を読み戻してから Ok（DB は数え直し・file は在るかを見る）・他の種類に触らない・利用者の folder と file・元の app・Claude Code の `~/.claude` は消さない。
    /// 保証しない: 取り消し（Undo の対象にしない・だから行内の確認を 1 回挟む）。
    /// Wave: RQ3
    pub fn delete_my_data(&mut self, kind: crate::ui::shell::DataKind) -> Result<String, String> {
        use crate::ui::shell::DataKind;
        if self.working(None) {
            return Err("Stop the run first".into());
        }
        let dir = self.data_dir.clone();
        let at = |n: &str| dir.as_ref().map(|d| d.join(n));
        let failed = |e: std::io::Error| format!("Couldn't delete everything ({}) · Try again", e.kind());
        let gone = |e: storage::StoreError| format!("Couldn't delete everything ({e}) · Try again");
        match kind {
            DataKind::Projects => {
                let open: Vec<String> = self.apps.iter().map(|o| project_id_of(&o.rt.revision().definition().id)).collect();
                for p in &open {
                    self.delete_project(p)?;
                }
                // 今回開けなかった Project（定義が今の検査を通らない）の行も
                {
                    let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                    store.immediate(gone, |store| {
                        for r in store.list(Table::Apps, None).map_err(gone)? {
                            store.delete_project_rows(&project_id_of(&r.id)).map_err(gone)?;
                            store.delete(Table::Apps, &r.id).map_err(gone)?;
                        }
                        for r in store.list(Table::Projects, None).map_err(gone)? {
                            store.delete(Table::Projects, &r.id).map_err(gone)?;
                        }
                        Ok(())
                    })?;
                    let left = store.list(Table::Apps, None).map_err(gone)?.len() + store.list(Table::Projects, None).map_err(gone)?.len();
                    if left > 0 {
                        return Err(format!("{left} projects are still there · Try again"));
                    }
                }
                remove_all(at("takes")).map_err(failed)?;
                Ok("Deleted projects and pages · your folders and files stay".into())
            }
            DataKind::History => {
                {
                    let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                    store.immediate(gone, |store| store.list(Table::Runs, None).map_err(gone)?.iter().try_for_each(|r| store.delete(Table::Runs, &r.id).map(|_| ()).map_err(gone)))?;
                    if !store.list(Table::Runs, None).map_err(gone)?.is_empty() {
                        return Err("Some runs are still there · Try again".into());
                    }
                }
                HISTORY_FILES.iter().try_for_each(|n| remove_all(at(n))).map_err(failed)?;
                // ENGINE-AUDIT W2: 保存した手順（workflows/）も記録と一緒に消す（Project をまたいで同じ失敗を起こさない）
                remove_all(at("workflows")).map_err(failed)?;
                self.engine_history.clear();
                self.last_runs.clear();
                Ok("Deleted run history and AI calls".into())
            }
            DataKind::ScreenRecords => {
                SCREEN_FILES.iter().try_for_each(|n| remove_all(at(n))).map_err(failed)?;
                Ok("Deleted screen and input records".into())
            }
            DataKind::Consents => {
                remove_all(at(crate::surface::ACT_CONSENT_FILE)).map_err(failed)?;
                // Mirror の同意は ui_state の `mirror-consent` の session の行（CASCADE で同意の行ごと・`surface::mirror::CONSENT_UI_STATE_SESSION` の写し）
                self.store.lock().unwrap_or_else(|e| e.into_inner()).delete(Table::Sessions, "mirror-consent").map_err(gone)?;
                // Mirror の同意の門は起動中の memory の写し（`ConsentGate::from_records`）。捨てて、次に描く時に空の同意から作り直させる（再起動を待たずに聞き直す）
                #[cfg(feature = "mirror")]
                {
                    self.mirror = None;
                }
                Ok("Shibubu will ask again before AI or Mirror works in each app".into())
            }
            DataKind::Connections => {
                // Browser の面の共有の web の store（sign-in）。使っている WebView が在って今消せなければ次の起動で消す（`forget_project` の queue・
                // 起動の `remove_forgotten_stores` は `project.` で始まらない id を DB に無い物として消す）。AI の account と鍵は app.rs の腕（この関数が Ok の後）。
                // この store には G72 より前に Diagram で描いた図（RQ-028 が戻す元）も在る: 確かめの 1 行（`DataKind::effect`）が消える事を言い、押した人だけが消す
                let _ = self.web.forget_project(crate::surface::SHARED_WEB_STORE.0);
                remove_all(at(ACCOUNTS_DIR)).map_err(failed)?;
                Ok("Removed added AI accounts and keys · Browser sign-ins and page data are cleared now or when Shibubu reopens · Claude Code's own sign-in stays".into())
            }
            DataKind::Backups => {
                // 予約した Restore の印も（世代が無いのに `Restores when Shibubu reopens` と Cancel を出し続け、次の起動で戻せない 1 行になる）
                remove_all(at(RESTORE_MARKER)).map_err(failed)?;
                remove_all(at(BACKUPS_DIR)).map_err(failed)?;
                if let Some(d) = &dir {
                    for f in legacy_damaged(d) {
                        remove_all(Some(f)).map_err(failed)?;
                    }
                }
                Ok("Deleted backups · your current data stays".into())
            }
        }
    }

    /// RQ3（RQ-028）: 前の共有の web の store に在る Diagram の図を、この Project の Diagram の store へ写し始め（`PageWebViews::restore_diagrams`）、その Project の
    /// Diagram の Page へ移った View を返す。終わりは裏から届き、Page の 1 行（`Restored the diagram from before` / `No diagram from before was found` / 理由）と面の読み直し。
    /// 例（app.rs の `Intent::RestoreDiagrams(project)`）: `match self.generated.restore_diagrams(&project) { Ok(v) => self.go(v), Err(why) => self.message = why }`
    /// 保証する: 前の store を消さない・図の在る Project には書かない（`PageWebViews::restore_diagrams` の doc）。Err = Diagram の Page が無い・始められない 1 行。
    /// Wave: RQ3
    pub fn restore_diagrams(&mut self, project_id: &str) -> Result<View, String> {
        // ponytail: 作り手（header の ⌄ の行）を描いていない（PageWebViews::restore_diagrams が未実装・未測）。押す所が 0 なので届いたら理由の 1 行
        let _ = project_id;
        Err("Restoring the diagram from before isn't available yet".into())
    }

    /// RQ3（RQ-014・Data retention）: 保存期間を過ぎた記録と上限を超えた記録を消し、消した行と file の数を返す。app.rs が起動の 1 回と `history_days` を変えた時に呼ぶ。
    /// 消す物: `History`（`history_days`・None = 30 日）・`ScreenRecords`（7 日）の行（JSONL は古い行から書き直し・DB の runs と evidence は日付で）・どの JSONL も
    /// 1 file 10 MB を超えたら古い行から・`BACKUPS_DIR` は `BACKUPS_DIR` の doc の数を超えた古い世代。
    /// 消さない: `Projects`・`Consents`（同意は記録でなく決定）・`Connections`・`damaged` の世代。例: `let _ = self.generated.prune_records(now, self.store.ui.history_days);`
    /// 保証する: 時刻の読めない行は file を最後に書いた時刻で見る（それが保存期間の内なら残す・上限の時だけ古い順に）・書き直しは一時の file から rename
    /// （途中で落ちても前の file が残る）。Err = 画面に出さない 1 行（log）。
    /// Wave: RQ3
    pub fn prune_records(&self, now: i64, history_days: Option<u16>) -> Result<usize, String> {
        let days = i64::from(match history_days {
            Some(d @ (7 | 30 | 90)) => d,
            _ => 30,
        });
        let (mut n, mut errs) = (0, Vec::new());
        let mut add = |r: Result<usize, String>| match r {
            Ok(k) => n += k,
            Err(e) => errs.push(e),
        };
        if let Some(dir) = self.data_dir.as_deref() {
            for f in HISTORY_FILES {
                add(prune_jsonl(&dir.join(f), now - days * 86_400));
            }
            for f in SCREEN_FILES {
                add(prune_jsonl(&dir.join(f), now - 7 * 86_400));
            }
            add(prune_backups(dir));
        }
        // DB の runs（History）: 終わった run だけ（承認待ち・動いている run は日付が古くても残す）
        let cutoff = crate::agent::rfc3339_from_secs(now - days * 86_400);
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        add((|| {
            let mut k = 0;
            for r in store.list(Table::Runs, None).map_err(|e| e.to_string())? {
                let done = serde_json::from_value::<crate::domain::run::RunStatus>(r.json["run"]["status"].clone()).is_ok_and(|s| s.is_terminal());
                if done && r.updated_at < cutoff && store.delete(Table::Runs, &r.id).map_err(|e| e.to_string())? {
                    k += 1;
                }
            }
            Ok(k)
        })());
        drop(add);
        if errs.is_empty() { Ok(n) } else { Err(errs.join(" · ")) }
    }

    /// 開いている Project の (app id, 定義, hash)。開いていない・読めないは Err の 1 行。
    // ponytail: draw_page が毎 frame 呼ぶ（DB 1 行 + hash）。重くなったら apply_project の戻り値で cache する
    fn project(&self, project_id: &str) -> Result<(String, ProjectDefinitionV1, String), String> {
        let gone = || "This project couldn't be found".to_owned();
        let app = self.app_of_project(project_id).ok_or_else(gone)?;
        let (p, hash) = self.project_of(&app)?.ok_or_else(gone)?;
        Ok((app, p, hash))
    }

    /// router が無ければ（View を直に渡された）先頭から作る。
    fn router(&mut self, project_id: &str, p: &ProjectDefinitionV1) -> Result<&mut PageRouter, String> {
        if !self.routers.contains_key(project_id) {
            self.routers.insert(project_id.to_owned(), PageRouter::open(p, None).map_err(|e| e.to_string())?);
        }
        Ok(self.routers.get_mut(project_id).expect("今入れた"))
    }

    /// 拒否は 1 行を `state` の印で残す（draw_page が Page の上に出す）。通った操作の 1 行（読めなかった状態）は `switch` が残した物のまま。
    fn remember(&mut self, r: Result<View, String>, state: RegionState) -> Result<View, String> {
        if let Err(e) = &r {
            self.page_line = Some((state, e.clone()));
        }
        r
    }

    /// SPEC §4 の順: 今の Page の save_view_state → router の navigate → 移った先の activate。保存できなければ移らない（打った文を失わない）。
    fn navigate_inner(&mut self, project_id: &str, to: &str) -> Result<View, String> {
        self.page_line = None;
        self.pending_delete = None;
        self.pending_rename = None;
        let (_, p, _) = self.project(project_id)?;
        let current = self.router(project_id, &p)?.current().to_owned();
        if to != current && p.page(to).is_some() {
            self.save_view_state(project_id, &current)?;
        }
        let sw = self.router(project_id, &p)?.navigate(&p, to).map_err(|e| e.to_string())?;
        Ok(self.switch(project_id, sw))
    }

    /// 移った先を activate する（保存は呼ぶ側が先に済ませた）。読めない状態は空で開き、理由の 1 行を Page の上に残す。
    fn switch(&mut self, project_id: &str, sw: Option<PageSwitch>) -> View {
        if let Some(sw) = &sw {
            if let Err(e) = self.activate(project_id, &sw.to) {
                self.page_line = Some((RegionState::Error, e));
            }
        }
        let page = self.routers.get(project_id).map(|r| r.current().to_owned()).unwrap_or_default();
        View::Project { project: project_id.to_owned(), page }
    }

    /// `PageIntent::Edit` の後半（同期の `page_intent` と裏の `poll_model_job` の共通）: model の答え（None = model 無し）→ 使えなければ名前の表で
    /// 読み直して理由を toast（`notices`・NOTIFY-1101 追記・Home の llm_line には出さない）→ `apply_project(.., NlEdit)` を 1 回
    /// （`hash` が今の Project と違えば当てずに 1 行）→ reconcile。編集は生成の理由（llm_fallback）を消す（その Project で次の手が済んだ）。
    fn finish_edit(&mut self, project_id: &str, text: &str, read: Option<Result<Vec<ProjectPatchOp>, composer::patch::EditError>>, app: &str, p: &ProjectDefinitionV1, hash: &str) -> Result<View, String> {
        use composer::patch::EditError;
        let (read, fallback) = match read {
            Some(Err(EditError::Model(why))) => (composer::project::edit_project(p, text), Some(format!("AI wasn't available, so the change was read with simple rules · {why}"))),
            Some(r) => (r, None),
            None => (composer::project::edit_project(p, text), None),
        };
        self.llm_fallback = None;
        // 名前の表で読み直した理由は、通った編集の後に toast（× まで残る・NOTIFY-1101 追記 = owner 192「Partly shown · AI wasn't available …」を Page の上に出さない）。
        // 拒否された時は拒否の文の後ろに繋ぐ（反証の直し: 前は理由が画面のどこにも残らなかった）
        let read_with_rules = |g: &mut Self, r: Result<View, String>| match (r, &fallback) {
            (Ok(v), Some(why)) => {
                g.notices.push((why.clone(), true));
                Ok(v)
            }
            (Err(e), Some(why)) => Err(format!("{e} · {why}")),
            (r, None) => r,
        };
        // W19-2 の配線（図 §33）: Project の編集として読めない文は「操作の依頼」として Engine へ（set_engine の無い時・Engine が走っている時は今まで通りの 1 行）
        // L7-AGENT（反証の直し）: 承認した agent の一言（`asking`）は Engine に渡さない（承認の範囲は Project の編集だけ）
        let asking = self.asking(project_id);
        // CHAT-ONE-1101: 会話の案の Accept で来た変更は人の一言を言い直さない（`accepted`）
        let quiet = asking || self.accepted(project_id, text);
        if matches!(read, Err(EditError::Unsupported(_))) && !asking && self.start_engine(project_id, text, Vec::new(), None) {
            self.edit_applied = Some(project_id.to_owned());
            let r = Ok(self.switch(project_id, None));
            return read_with_rules(self, r);
        }
        let ops = match read {
            Ok(ops) => ops,
            Err(e) => {
                // 文の拒否（直せば通る）: 呼ぶ側が 1 行を Ready にする印。欄の文は残る（edit_applied を立てない）
                self.edit_refused = true;
                let line = match e {
                    EditError::AlreadyApplied(what) => format!("There's already {what}."),
                    EditError::NoTarget(line) => line,
                    EditError::Unsupported(_) if asking => ASK_SCOPE_LINE.into(),
                    // 上の「Engine が走っている時」の 1 行: 「読めない」ではなく待つ 1 行（copy.en.md:198・打った文は欄に残る）。走っていない時だけ救いの 1 行
                    EditError::Unsupported(_) if self.engine_job.is_some() => ONE_CHANGE.into(),
                    EditError::Unsupported(_) => composer::project::rescue_line(),
                    other => other.to_string(),
                };
                use crate::domain::chat::{Mark, Who};
                // agent の一言は人の発言にしない（`decide_ask` が「{who} asked: …」を先に言っている）
                if !quiet {
                    self.say(project_id, Who::You, text, Mark::None, Vec::new());
                }
                self.say(project_id, Who::Shibubu, &line, Mark::Stopped, Vec::new());
                return read_with_rules(self, Err(line));
            }
        };
        // AddPage は足した Page へ・MoveSurface は動いた先の Page へ（今の Page がそこなら navigate_inner は移らない・UI-5a・sendback 60.4 A3）・
        // AI-COMPOSE-1101: AI が書いた部品（add_part）は置いた Page へ
        let added = ops.iter().find_map(|op| match op {
            ProjectPatchOp::AddPage { page, .. } => Some(page.id.clone()),
            ProjectPatchOp::MoveSurface { to_page, .. } => Some(to_page.clone()),
            ProjectPatchOp::Page { page_id, .. } => Some(page_id.clone()),
            _ => None,
        });
        let (next, _) = self.apply_project(app, hash, &ops, RevisionOrigin::NlEdit)?;
        #[cfg(feature = "native_direct")]
        if ops
            .iter()
            .any(|op| matches!(op, ProjectPatchOp::MoveSurface { .. }))
        {
            self.retarget_natives(project_id, &next);
        }
        self.router(project_id, p)?.reconcile(&next, None);
        // 欄の打った文を消す印（agent の一言なら人の打ちかけを消さない・人の発言にしない）
        if !asking {
            self.edit_applied = Some(project_id.to_owned());
        }
        {
            use crate::domain::chat::{Mark, Who};
            if !quiet {
                self.say(project_id, Who::You, text, Mark::None, Vec::new());
            }
            self.say(project_id, Who::Shibubu, &changed_line(&ops, &next), Mark::None, Vec::new());
        }
        let r = match added {
            Some(id) => self.navigate_inner(project_id, &id),
            None => Ok(self.switch(project_id, None)),
        };
        read_with_rules(self, r)
    }

    // 消した Page の ui_state の行（page:{id}:view）は消さない（W8 で決めた: Undo / Redo で戻った Page に入力途中の状態が戻る・id は乱数で再利用されない）
    fn page_intent_inner(&mut self, project_id: &str, intent: PageIntent) -> Result<View, String> {
        self.page_line = None;
        self.pending_delete = None;
        // FX3: 名前の欄は Rename の拒否の時だけ開いたまま（打った名前を失わない）・他の手で閉じる
        if !matches!(intent, PageIntent::Rename { .. }) {
            self.pending_rename = None;
        }
        let (app, p, hash) = self.project(project_id)?;
        let err = |e: ProjectPatchError| e.to_string();
        match intent {
            PageIntent::Add => {
                let id = format!("page.{}", storage::new_id());
                let op = router::new_page(&p, id.clone(), &format!("Page {}", p.pages.len() + 1)).map_err(err)?;
                let (next, _) = self.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                self.navigate_inner(project_id, &id)
            }
            PageIntent::Rename { page, title } => {
                // 今と同じ名前（trim 後）は書かずに閉じる（revision を足さない = Move の同じ位置と同じ）。拒否より先に見る
                // （FX3 反証の直し: 同じ名前の Page が 2 枚在ると〈Add の `Page {n}`・保存済みの data〉、名前のまま Enter が別の Page と同じ名前で拒まれ、欄が閉じなかった）
                if p.page(&page).is_some_and(|pg| pg.title == title.trim()) {
                    self.pending_rename = None;
                    return Ok(self.switch(project_id, None));
                }
                // FX3（FX-009）: 別の Page と同じ名前・空は打ち直せば通る拒否 = Ready の 1 行（欄は打った名前のまま開いている）
                let op = match router::rename_page(&p, &page, &title) {
                    Ok(op) => op,
                    Err(e @ ProjectPatchError::DuplicateTitle(_)) => {
                        self.edit_refused = true;
                        return Err(e.to_string());
                    }
                    // 空の名前（`Invalid` の Display は日本語の検査の文 = G97 の持ち物）は表の `Enter a file name` と同じ形の 1 行
                    Err(ProjectPatchError::Invalid(_)) => {
                        self.edit_refused = true;
                        return Err("Enter a page name".into());
                    }
                    Err(e) => return Err(e.to_string()),
                };
                let (next, _) = self.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit)?;
                self.pending_rename = None;
                self.router(project_id, &p)?.reconcile(&next, None);
                Ok(self.switch(project_id, None))
            }
            PageIntent::Delete { page } => {
                let land = self.router(project_id, &p)?.landing_after_delete(&p, &page).map_err(err)?;
                // UI-5a: 消す前に今の Page の打ちかけを ui_state に保存する（page:{id}:view の行は消さない = ⌘Z で戻った Page に打った文字も戻る・Done 3）。
                // 保存できなければ Err の 1 行で何も消さない。save_view_state でなく inner を呼ぶのは、RemovePage が断られた時に窓を隠さないため（隠すのは消せた後の下の 1 か所）
                if self.router(project_id, &p)?.current() == page {
                    self.save_view_inner(project_id, &page)?;
                }
                let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::RemovePage { page_id: page.clone() }], RevisionOrigin::DirectEdit)?;
                // W16-1: 消せた後にだけ、消した Page に重ねていた窓を Shibubu の後ろへ（消した後は描かれないので前に残さない。
                // 断られた〈最後の 1 枚・古い base_hash〉時に隠すと、残った Page の枠が空のまま「直接操作中」になる）
                #[cfg(feature = "native_direct")]
                self.surfaces.hide_page(&page);
                // 移った先だけ activate する（消した Page の打ちかけは上で save 済み）
                let sw = self.router(project_id, &p)?.reconcile(&next, Some(&land));
                Ok(self.switch(project_id, sw))
            }
            PageIntent::AddTerminal { page } => {
                let pg = p.page(&page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                let spec = project::TerminalSessionSpec { id: storage::new_id(), project_id: p.id.clone(), shell: user_shell(), working_directory: self.terminal_cwd(&p), keep_alive_on_page_switch: true, command: None };
                let op = ProjectPatchOp::AddTerminal { node_key: format!("terminal.{}", spec.id), page_id: page.clone(), parent: pg.root.key.clone(), index: None, spec: spec.clone() };
                let (next, _) = self.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                // 起こせなくても席は残る（Terminal の場所に理由と「新しく起こす」を出す・毎 frame 起こし直さない）
                if let Err(e) = ensure_spec(&self.terminals, &self.store, &spec) {
                    self.term_views.lines.insert(SessionKey::of(&spec), e);
                }
                // TERM-ONE-1101: Page には描かない = 足した Terminal は下の panel の tab（同じ id・同じ shell）を選んで開き、grid に focus（拡大した Mirror は解く・G94）
                self.unzoom_mirror();
                let (_, now, _) = self.project(project_id)?;
                self.adopt_plan_terminals(project_id, &now);
                self.panel_act(project_id, PanelAct::Select(spec.id.clone()))?;
                self.term_views.reveal = Some(spec.id);
                Ok(self.switch(project_id, None))
            }
            PageIntent::Back => {
                let current = self.router(project_id, &p)?.current().to_owned();
                if !self.router(project_id, &p)?.history().is_empty() {
                    self.save_view_state(project_id, &current)?;
                }
                let sw = self.router(project_id, &p)?.back(&p);
                Ok(self.switch(project_id, sw))
            }
            PageIntent::Move { page, index } => {
                // apply_project は呼ぶたびに revision を書くので、同じ位置（末尾で「→」= index が丸められて元の位置）は当てる前に止める
                let from = p.pages.iter().position(|pg| pg.id == page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                if index.min(p.pages.len() - 1) == from {
                    return Ok(self.switch(project_id, None));
                }
                let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::MovePage { page_id: page, index }], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                Ok(self.switch(project_id, None))
            }
            // W7: composer::project::edit_project → apply_project(.., NlEdit) を 1 回 → reconcile。AddPage は navigate_inner で足した Page へ（PageIntent::Edit の doc）
            PageIntent::Edit { text } => {
                // W13: model が在れば model に読ませる（この呼び方は答えを待つ。app.rs は裏の `start_edit`）
                // UI-2: model の語に無い形（左へ・名前替え）は model に送らず名前の表で読む（図 §36）
                let read = self.llm.as_deref().filter(|_| model_reads(&text)).map(|llm| composer::project::edit_project_by_model(&p, &text, &LlmModel::new(llm, &NEVER)));
                self.finish_edit(project_id, &text, read, &app, &p, &hash)
            }
            // W8: 今の Page を save_view_state → 1 回の BEGIN IMMEDIATE で hash を照合し projects の行を undo / redo の行の snapshot に置き換える
            // （project_revisions は書かない）→ reconcile(履歴の 1 つ前) → activate。行き先が無ければ UndoError の 1 行（PageIntent::Undo / Redo の doc）
            PageIntent::Undo | PageIntent::Redo => {
                let undo = matches!(intent, PageIntent::Undo);
                let to = self.undo_redo(project_id)?;
                let target = if undo { to.undo.ok_or(UndoError::NothingToUndo) } else { to.redo.ok_or(UndoError::NothingToRedo) }.map_err(|e| e.to_string())?;
                let current = self.router(project_id, &p)?.current().to_owned();
                self.save_view_state(project_id, &current)?;
                let next = {
                    let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                    let saved_err = |e: storage::StoreError| format!("Couldn't save the project ({e}). Try again");
                    store.immediate(saved_err, |store| {
                        let now = match store.get(Table::Projects, &p.id).map_err(saved_err)? {
                            Some(row) => saved_project(row.json)?,
                            None => p.clone(),
                        };
                        if project_hash(&now) != hash {
                            return Err("This project changed somewhere else. Reopen it, then try again.".to_string());
                        }
                        // reason は画面に出る固定の語（serde の原文・内部の id を入れない・W2 の反証の直しと同じ規則）
                        let broken = || UndoError::Unreadable { revision: target, reason: "Saved by a newer version, or damaged".into() }.to_string();
                        let row = store.list_project_revisions(&p.id).map_err(|_| broken())?.into_iter().find(|r| r.revision == target as i64);
                        let row = row.ok_or_else(broken)?;
                        // 読み戻せない JSON を projects に書かない（書いた後に開けなくなる）
                        let next = serde_json::from_value::<ProjectDefinitionV1>(row.json.clone()).map_err(|_| broken())?;
                        if next.revision != target || next.id != p.id {
                            return Err(broken());
                        }
                        store.put(Table::Projects, &p.id, None, &row.json).map_err(saved_err)?;
                        Ok(next)
                    })?
                };
                // 今の Page が消えたら履歴の 1 つ前（無ければ先頭）へ
                let router = self.router(project_id, &p)?;
                let landing = router.history().iter().rev().find(|id| next.page(id).is_some()).cloned();
                let sw = router.reconcile(&next, landing.as_deref());
                // W16-3: Undo/Redo も MoveSurface の NL 編集と同じく、復元した木の page_id に Dock を付け替える（窓は動かさない）
                #[cfg(feature = "native_direct")]
                self.retarget_natives(project_id, &next);
                // G86（operator 10-29 CU ③）: 会話の在る Project は、戻した / やり直した後の Page を 1 行（会話の最後の「Changed …」と今の Project を食い違わせない）。
                // 27 手の needle（Undo・Redo・Done 等）を含まない語・Project の名前は入れない（rehearse.py:947 が承認の 1 行の「Shibubu Demo」を AXStaticText で探す）
                if !self.chat_of(project_id).is_empty() {
                    let line = format!("{} · Pages now: {}", if undo { "Undid the last change" } else { "Redid the change" }, page_titles(&next));
                    self.say(project_id, crate::domain::chat::Who::Shibubu, &line, crate::domain::chat::Mark::None, Vec::new());
                }
                Ok(self.switch(project_id, sw))
            }
            // W12a: read_resources を 1 回・居場所は今の Page のまま（PageIntent::Reread の doc）
            PageIntent::Reread => {
                self.router(project_id, &p)?;
                self.read_resources(project_id)?;
                Ok(self.switch(project_id, None))
            }
            // W13b: 1 問への答え → apply_project(.., DirectEdit) を 1 回・居場所は今の Page のまま（PageIntent::AnswerMissingApp の doc）
            PageIntent::AnswerMissingApp { seat, use_app } => {
                let ops = missing_app_ops(&p, &seat, use_app.as_deref())?;
                if !ADD_SURFACE_BUILT && ops.iter().any(|op| matches!(op, ProjectPatchOp::AddSurface { .. })) {
                    return Err("This version can't place a replacement app yet. Press “Continue without it”, or install it and send your request again".into());
                }
                let (next, _) = self.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                Ok(self.switch(project_id, None))
            }
            // PV0: 導入の手 → 裏の thread（何も書かない）。1 問を面に替えるのは `poll_installs` が読み戻しの後に 1 回
            PageIntent::Install { seat, act } => self.install_act(project_id, &p, &seat, act),
            // G99c: folder を当てる op を 1 回（PageIntent::SetWorkspace の doc）・窓の答えの末尾の / は落とす
            PageIntent::SetWorkspace { dir } => {
                let dir = match dir.trim_end_matches('/') {
                    "" => "/".to_owned(),
                    d => d.to_owned(),
                };
                if !Path::new(&dir).is_dir() {
                    return Err(format!("{dir} isn't a folder · choose a folder"));
                }
                let ops = workspace_ops(&p, &dir);
                let uri = format!("file://{dir}");
                let same = ops.len() == 1
                    && p.resources.iter().any(|r| r.id == "repo" && r.uri.as_deref() == Some(uri.as_str()))
                    && p.terminal_specs.iter().all(|t| t.working_directory == dir);
                if !same {
                    let (next, _) = self.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit)?;
                    self.router(project_id, &p)?.reconcile(&next, None);
                    // TERM-ONE-1101（owner 183）: 動いている terminal は cd しない・次に開く terminal から（panel_act の cwd = terminal_cwd）
                    let name = Path::new(&dir).file_name().map_or_else(|| dir.clone(), |n| n.to_string_lossy().into_owned());
                    self.page_line = Some((RegionState::Ready, format!("New terminals open in {name}")));
                }
                Ok(self.switch(project_id, None))
            }
            PageIntent::ChooseFolder => Err("Choose the folder in the window that opened".into()),
            // AI-COMPOSE-1101: AI が書いた部品の値を残す（その node を持つ Page に Update を 1 回・同じ値は書かない）。居場所は今の Page のまま
            PageIntent::CanvasSet { node, key, value } => {
                let found = p.pages.iter().find_map(|pg| pg.root.find(&node).map(|n| (pg.id.clone(), n)));
                let Some((page_id, n)) = found else { return Err("That part is no longer on this project".into()) };
                if let Some(patch) = crate::domain::ui::CanvasProps::set_patch(n, &key, value) {
                    let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::Page { page_id, patch }], RevisionOrigin::DirectEdit)?;
                    self.router(project_id, &p)?.reconcile(&next, None);
                }
                Ok(self.switch(project_id, None))
            }
            // AI-COMPOSE-1101: 部品の頼みを Engine に（`{value}` を値に・承認の決まりは今のまま）。Engine が動いている間は走らせない
            // ENGINE-GENERIC-1101: 部品から Engine への 1 つの口（`ask_engine`）を通す
            PageIntent::CanvasAct { intent, value } => {
                // ENGINE-AUDIT W9 本体: 部品の押しは人の手 = 1 つの頼み
                self.new_request();
                self.ask_engine(project_id, &crate::domain::ui::fill_value(&intent, &value), Default::default())?;
                Ok(self.switch(project_id, None))
            }
            PageIntent::ShowTerminal { page } => {
                let pg = p.page(&page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                // この Page に置いた Terminal（Project の terminal_specs に在る物）の先頭。無ければ Add terminal と同じ道で 1 つ置く（それも見せる）
                let placed = pg.root.nodes().into_iter().filter(|n| n.component == "terminal").filter_map(|n| n.props.get("terminal_ref")?.as_str().map(str::to_owned)).find(|r| p.terminal_specs.iter().any(|t| t.id == *r));
                // G95: Page に Terminal が無ければ下の panel を開く（tab が無ければ 1 つ足す・Page に置かない）
                let Some(r) = placed else {
                    self.router(project_id, &p)?;
                    self.panel_act(project_id, PanelAct::Open)?;
                    return Ok(self.switch(project_id, None));
                };
                // TERM-ONE-1101: plan の Terminal は下の panel の tab（`adopt_plan_terminals`）= その tab を選んで開き、grid に focus
                self.router(project_id, &p)?;
                self.unzoom_mirror();
                self.adopt_plan_terminals(project_id, &p);
                self.panel_act(project_id, PanelAct::Select(r.clone()))?;
                self.term_views.reveal = Some(r);
                Ok(self.switch(project_id, None))
            }
            // MIRROR-AUTO-1101 B: `•••` の `Stop mirroring` = この Page の Mirror の面を全部 `Closed`（stream を止める・帯を押すと映り直す・形は描いた frame の終わりの
            // `save_mirror_choices` が保存 = 開き直し・再起動を跨ぐ）
            PageIntent::StopMirroring { page } => {
                let pg = p.page(&page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                let ids = mirror_surfaces_on(&p, pg);
                #[cfg(feature = "mirror")]
                if let Some(m) = self.mirror.as_mut() {
                    for id in &ids {
                        m.set_window(id, crate::ui::mirror_view::MirrorWindow::Closed, f64::NEG_INFINITY);
                    }
                }
                #[cfg(not(feature = "mirror"))]
                let _ = ids;
                Ok(self.switch(project_id, None))
            }
            PageIntent::Placement => {
                let current = self.router(project_id, &p)?.current().to_owned();
                let now = self.placement(project_id)?.map(|t| t.id);
                // G94: 今の Page に効く型だけを回す（効かない型へ回すと label だけ替わり画面が動かない）
                let ids = placement_ids(&p, p.page(&current));
                if ids.is_empty() {
                    return Err("No layout changes this page".into());
                }
                // 置き場 → 同梱の順 → 型なし → 先頭
                let next = match now.and_then(|id| ids.iter().position(|x| *x == id)) {
                    Some(i) => ids.get(i + 1).cloned(),
                    None => ids.first().cloned(),
                };
                let json = serde_json::to_value(crate::project::PlacementChoice { template: next }).map_err(|e| e.to_string())?;
                let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                store.put_ui_state(&session_of(&app), crate::project::PLACEMENT_KEY, &json).map_err(|e| format!("Couldn't save the layout · {e}"))?;
                drop(store);
                Ok(self.switch(project_id, None))
            }
            PageIntent::RunTests { command } => {
                if command.trim().is_empty() {
                    return Err("No test command set".into());
                }
                // FX6（FX-033）: 専用の session で走らせる（利用者の Terminal には打たない）。結果は Tests の Page の receipt
                let spec = tests_spec(&p).ok_or(NO_REPO_LINE)?;
                ensure_spec(&self.terminals, &self.store, &spec)?;
                let key = SessionKey::of(&spec);
                self.start_run(&key, spec.working_directory.as_str(), command.trim(), RUN_TESTS_TIMEOUT)?;
                Ok(self.switch(project_id, None))
            }
            PageIntent::AddBrowser { page } => {
                let pg = p.page(&page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                let id = storage::new_id();
                let spec = project::SurfaceSpec {
                    id: id.clone(),
                    backend: project::SurfaceBackendKind::WebView,
                    target: project::SurfaceTarget::Url { url: crate::project::web::DEFAULT_BROWSER_URL.into() },
                    keep_alive: true,
                    frame: None,
                    browser: true,
                };
                let op = ProjectPatchOp::AddSurface {
                    node_key: format!("surface.{id}"),
                    page_id: page,
                    parent: pg.root.key.clone(),
                    index: None,
                    spec,
                };
                let (next, _) = self.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                // G92: 足した Browser を次の frame で見える所まで送る
                self.unzoom_mirror();
                self.term_views.reveal = Some(id);
                Ok(self.switch(project_id, None))
            }
            // MIRROR-FRAME-1101: 空の Mirror の枠を Page の root の末尾に 1 つ（AddBrowser と同じ 1 つの op = ⌘Z 1 回で消える・窓は探さない）
            PageIntent::AddMirrorFrame { page } => {
                let pg = p.page(&page).ok_or_else(|| err(ProjectPatchError::PageNotFound(page.clone())))?;
                let id = storage::new_id();
                let spec = project::SurfaceSpec {
                    id: id.clone(),
                    backend: project::SurfaceBackendKind::Mirror,
                    target: project::SurfaceTarget::App { bundle_id: String::new(), file: None },
                    keep_alive: true,
                    frame: Some(BLANK_FRAME),
                    browser: false,
                };
                let op = ProjectPatchOp::AddSurface { node_key: format!("surface.{id}"), page_id: page, parent: pg.root.key.clone(), index: None, spec };
                let (next, _) = self.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                self.unzoom_mirror();
                self.term_views.reveal = Some(id);
                Ok(self.switch(project_id, None))
            }
            PageIntent::SetSurfaceUrl { surface_id, input } => {
                let Some(url) = crate::surface::bar_input_to_url(&input) else {
                    return Ok(self.switch(project_id, None));
                };
                if let crate::surface::WebNav::Deny { why, .. } = crate::surface::decide_navigation(&url, &[]) {
                    self.page_line = Some((RegionState::Error, why));
                    return Ok(self.switch(project_id, None));
                }
                let same = p.surface_specs.iter().any(|s| s.id == surface_id && matches!(&s.target, project::SurfaceTarget::Url { url: cur } if *cur == url));
                if same {
                    return Ok(self.switch(project_id, None));
                }
                let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::SetSurfaceUrl { surface_id: surface_id.clone(), url: url.clone() }], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                self.web.load(&crate::project::web::key(project_id, &surface_id), &url);
                Ok(self.switch(project_id, None))
            }
            PageIntent::SetSurfaceFrame { surface_id, x, y, w, h } => {
                let frame = project::SurfaceFrame { x, y, w, h };
                if !frame.valid() {
                    return Ok(self.switch(project_id, None));
                }
                let same = p.surface_specs.iter().any(|s| s.id == surface_id && s.frame == Some(frame));
                if same {
                    return Ok(self.switch(project_id, None));
                }
                let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::SetSurfaceFrame { surface_id, frame }], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                Ok(self.switch(project_id, None))
            }
            // G95: panel の手（Project の定義を変えない・居場所は今の Page のまま）
            PageIntent::TerminalPanel(act) => {
                self.router(project_id, &p)?;
                self.panel_act(project_id, act)?;
                Ok(self.switch(project_id, None))
            }
            PageIntent::Engine(act) => {
                self.router(project_id, &p)?;
                self.engine_act(project_id, act);
                Ok(self.switch(project_id, None))
            }
            // RQ4（RQ-019）: ProjectPatchOp::SetSurfaceWindow を DirectEdit で 1 回（Undo できる）。面の探し直しは Mirror が `SurfaceTarget::App::file` の替わりを見て行う
            // MIRROR-FRAME-1101: `bundle_id` が在れば app も（空の枠に窓を入れる = 同じ 1 回の op）
            PageIntent::ChooseWindow { surface_id, title, bundle_id } => {
                let title = title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
                let bundle_id = bundle_id.map(|b| b.trim().to_owned()).filter(|b| !b.is_empty());
                let same = p.surface_specs.iter().any(|s| {
                    s.id == surface_id && matches!(&s.target, project::SurfaceTarget::App { bundle_id: b, file } if *file == title && bundle_id.as_ref().is_none_or(|n| n == b))
                });
                if same {
                    return Ok(self.switch(project_id, None));
                }
                let (next, _) = self.apply_project(&app, &hash, &[ProjectPatchOp::SetSurfaceWindow { surface_id, title, bundle_id }], RevisionOrigin::DirectEdit)?;
                self.router(project_id, &p)?.reconcile(&next, None);
                Ok(self.switch(project_id, None))
            }
        }
    }

    /// W8: 今の Project の Undo / Redo の行き先（`domain::project::UndoRedo` の doc の規則・DB だけから決める）。
    /// 使い方: draw_page が「↶ 元に戻す」「↷ やり直す」を出すかを決める（None なら出さない・L2）。試験は Undo の前後で行き先を見る。
    /// 保証する: 何も書かない・再起動をまたいでも同じ値。Err は開いていない・読めない Project の 1 行（`project` と同じ）か、
    /// revision の行が読めない時の固定の 1 行（serde の原文を画面に出さない・draw_page はボタンを出さずにその 1 行を出す）。
    // ponytail: draw_page が毎 frame 全 revision の行を読む（surface_receipts と同じ）。行が増えて重くなったら番号だけ引く query にする
    /// Wave: W8
    pub fn undo_redo(&self, project_id: &str) -> Result<UndoRedo, String> {
        let (_, p, _) = self.project(project_id)?;
        let head = p.revision;
        let revs: Vec<u64> = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let unreadable = "Undo is off · history unreadable";
            store.list_project_revisions(project_id).map_err(|_| unreadable.to_owned())?.iter().map(|r| r.revision as u64).collect()
        };
        let at = |n: u64| revs.contains(&n).then_some(n);
        Ok(UndoRedo { undo: head.checked_sub(1).filter(|&n| n >= 1).and_then(at), redo: at(head + 1) })
    }

    /// UI-5a: title line の `‹` の席に渡す、戻る先の Page の名前（router の履歴を後ろから見て、今の Page でなく今も在る最初の Page の title = `PageIntent::Back` が移る先）。
    /// 呼ぶ例（app.rs）: `let back = match &self.shell.view { View::Project { project, .. } => self.generated.back_target(project), _ => None };` → `header::draw(.., back.as_deref(), ..)`。
    /// 保証する: 何も書かない（router の履歴も変えない）。戻れる Page が無い・開いていない Project は None（押しても何も起きない `‹` を描かない・L2）。
    /// Wave: UI-5a
    pub fn back_target(&self, project_id: &str) -> Option<String> {
        let r = self.routers.get(project_id)?;
        let (_, p, _) = self.project(project_id).ok()?;
        // router::back と同じ選び方（今の Page と消えた Page を飛ばす）。履歴は変えない
        r.history().iter().rev().filter(|id| id.as_str() != r.current()).find_map(|id| p.page(id)).map(|pg| pg.title.clone())
    }

    /// RQ6（RQ-022）: Mirror に映してよいと押した app の bundle id（保存した同意の行・名前順）。Settings › Privacy の一覧。何も書かない。
    /// Wave: RQ6
    pub fn mirror_consents(&self) -> Vec<String> {
        // `surface::mirror::CONSENT_UI_STATE_SESSION` の写し（tests/yc_demo_e2e.rs が surface::mirror を持たない crate でも通る）
        let rows = self.store.lock().unwrap_or_else(|e| e.into_inner()).list_ui_state("mirror-consent").unwrap_or_default();
        // MIRROR-AUTO-1101 B: 取り消しの印の行は出さない
        rows.into_iter().filter(|(_, v)| v["revoked"] != true).map(|(bundle, _)| bundle).collect()
    }

    /// RQ6（RQ-022・Privacy の `Revoke`）: Mirror の同意を 1 つ取り消す（保存した行を取り消しの印 `{"bundle_id", "revoked": true}` に替え、起動中の同意の門を作り直させる =
    /// 次に描く時に `Mirror here` で聞き直す）。在って取り消したら true。
    /// MIRROR-AUTO-1101 B: 行を消すだけだと面が在る Project を描いた時に押さずに同意し直すので印を残す（`Mirror here` を押すと同意で上書き）。
    /// Wave: RQ6, MIRROR-AUTO-1101
    pub fn revoke_mirror_consent(&mut self, bundle_id: &str) -> Result<bool, String> {
        let gone = {
            let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let had = store.get_ui_state("mirror-consent", bundle_id).map_err(|e| format!("Couldn't revoke ({e}). Try again"))?.is_some_and(|v| v["revoked"] != true);
            if had {
                store.put_ui_state("mirror-consent", bundle_id, &serde_json::json!({ "bundle_id": bundle_id, "revoked": true })).map_err(|e| format!("Couldn't revoke ({e}). Try again"))?;
            }
            had
        };
        #[cfg(feature = "mirror")]
        if gone {
            self.mirror = None;
        }
        Ok(gone)
    }

    /// RQ5（RQ-020）: agent が読む「今の Page の app」と Page の id。Err = 提案を出せない理由の 1 行（利用者の言葉・右の列の Suggestions が空の下に言う）。app の定義の木を今の Page の木に替え、版を Project の revision にした物
    /// （Page の root は app の木の写しで、Project への編集は Page の木にだけ当たる）。版が Project の revision なので、何かの編集・Undo で提案が古くなる。
    /// 開いていない・形の検査に落ちる Page（面や Terminal の node を含む）は None = 提案しない。保証する: 何も書かない。
    /// Wave: RQ5
    pub fn agent_app(&self, project_id: &str) -> Result<(AppRevision, String), String> {
        let cannot = || "Suggestions can't read this page yet".to_owned();
        let (app, p, _) = self.project(project_id).map_err(|_| cannot())?;
        let page = self.routers.get(project_id).and_then(|r| p.page(r.current())).or_else(|| p.pages.first()).ok_or_else(cannot)?;
        // 面（Browser / 元の app）と Terminal を含む Page も app の検査に通る（実測: 試験 `rq5_pages_with_a_terminal_or_a_browser_are_still_read`）。通らない Page は
        // 黙って 0 件にせず、理由を右の列が言う（Err の 1 行）
        let mut def = self.find(page.app_ref.as_deref().unwrap_or(&app)).ok_or_else(cannot)?.rt.revision().definition().clone();
        def.composition.root = page.root.clone();
        def.version = u32::try_from(p.revision).unwrap_or(u32::MAX);
        AppRevision::new(def).map(|rev| (rev, page.id.clone())).map_err(|_| cannot())
    }

    /// CHAT-011 反証 B28（121「suggest は chat 内で自動で出る」）: 今の Page の表と一覧が持つ件数（source id → 件数）。app.rs が agent の `Observed::rows` に渡す
    /// （agent は自分で source を読まない）。数え方は描く側（`Host::data`）と同じ所 = binding の path の配列の長さ・まだ何も無い（null・無い）は 0。
    /// Page の決め方は `agent_app` と同じ（今の Page・無ければ先頭）。
    /// 例（app.rs `sync_agent`）: `let rows = self.generated.agent_rows(&project); if rows != self.agent_seen_rows { self.agent.observe(Observed { rows: rows.clone(), ..Default::default() }); self.agent.refresh(&now); }`
    /// 保証する: 何も書かない・開いていない Project と読めない source は数えない（空の map・その source だけ無い）。保証しない: 表と一覧の外の binding（form・text・chart）・Page の外の source。
    /// Wave: CHAT-011
    pub fn agent_rows(&self, project_id: &str) -> BTreeMap<String, usize> {
        let Ok((app, p, _)) = self.project(project_id) else { return BTreeMap::new() };
        let Some(page) = self.routers.get(project_id).and_then(|r| p.page(r.current())).or_else(|| p.pages.first()) else { return BTreeMap::new() };
        let Some(o) = self.find(page.app_ref.as_deref().unwrap_or(&app)) else { return BTreeMap::new() };
        page.root
            .nodes()
            .into_iter()
            .filter(|n| n.component == "table" || n.component == "list")
            .filter_map(|n| n.binding.as_ref())
            .filter_map(|b| {
                let doc = o.rt.ui_state(&b.source_ref).ok()?;
                Some((b.source_ref.clone(), at_path(&doc, &b.path).and_then(Value::as_array).map_or(0, Vec::len)))
            })
            .collect()
    }

    /// RQ5（RQ-021）: 提案の変更（`Suggestion::change.operations`）を `page_id` の Page に 1 revision で当てる（`apply_project`・DirectEdit = ⌘Z で戻り、DB から読み戻せる）。
    /// 古い hash・当たらない op は何も書かずに Err。Ok は移る先の View（今の Page のまま）。
    /// Wave: RQ5
    pub fn apply_suggestion(&mut self, project_id: &str, page_id: &str, patches: &[crate::domain::patch::AppPatch]) -> Result<View, String> {
        let (app, p, hash) = self.project(project_id)?;
        let ops: Vec<ProjectPatchOp> = patches.iter().map(|patch| ProjectPatchOp::Page { page_id: page_id.to_owned(), patch: patch.clone() }).collect();
        let (next, _) = self.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit)?;
        self.router(project_id, &p)?.reconcile(&next, None);
        Ok(self.switch(project_id, None))
    }

    /// G94: ✦ rail の `About` の行。Project の画面の時だけ Some（Name・Pages・Folder）で、他の画面は None（呼ぶ側が旧 canvas の Session の行にする）。
    /// 例: `let about = g.about_rows(&view)`。保証する: 何も書かない。
    pub fn about_rows(&self, view: &View) -> Option<Vec<(String, String)>> {
        let View::Project { project, .. } = view else { return None };
        // RQ2（RQ-008）: G94 の行の後ろに Project の設定の行（AI の行は app.rs が account を知っているので足す）
        self.project(project).ok().map(|(_, p, _)| project_about_rows(&p).into_iter().chain(self.project_settings_rows(project).unwrap_or_default()).collect())
    }

    /// UI-5a: tab の横の消す確認を閉じる（`Keep` と同じ・何も消さない）。app.rs の Esc の連鎖（palette → 承認と旧 canvas の確認 → rail）の「確認」の段が呼ぶ。
    /// 呼ぶ例: `else if self.generated.dismiss_delete() {}`。保証する: 確認が開いていた時だけ true（1 回の Esc で閉じるのは 1 つだけ）・Project と ui_state を書かない。
    /// Wave: UI-5a
    pub fn dismiss_delete(&mut self) -> bool {
        self.pending_delete.take().is_some()
    }

    /// FX3（FX-009）: tab の席の名前の欄（`•••` の `Rename…` で開いた `theme::rename_inline`）を閉じる = 打った名前を捨てる（Esc と同じ・何も書かない）。
    /// app.rs の Esc の連鎖の「確認」の段で `dismiss_delete` の次に呼ぶ: `}else if self.generated.dismiss_delete()||self.generated.dismiss_rename(){`。
    /// 保証する: 欄が開いていた時だけ true（1 回の Esc で閉じるのは 1 つ・消す確認と名前の欄は同時に開かない = 片方を開くともう片方を閉じる）・Project と ui_state を書かない。
    /// 保証しない: 欄の Esc を egui の TextEdit が先に食べた時の 2 回目（`rename_inline` の `Some(false)` が同じ frame に閉じる）。
    /// Wave: FX3
    pub fn dismiss_rename(&mut self) -> bool {
        self.pending_rename.take().is_some()
    }

    /// FX3（FX-008）: ⌘K の palette に渡す、この Project の全 Page の行（並びの順・label `Go to {Page}`・hint 空・intent `Go(View::Project { project, page })`）。
    /// 例（app.rs）: `let pages = match &self.shell.view { View::Project { project, .. } => self.generated.page_commands(project), _ => Vec::new() };` →
    /// `palette::draw(ctx, &mut self.shell.palette, &rows, .., self.shell.canvases, &pages)`。
    /// 保証する: 何も書かない・帯の外へ scroll した Page にも行ける（帯の tab と同じ行き先）・今の Page の行も出す（押しても移らないだけ）。
    /// 開いていない・読めない Project は空（理由は Page の上の 1 行が言う）。保証しない: 他の Project の Page・Page の中の部品の行。
    /// Wave: FX3
    pub fn page_commands(&self, project_id: &str) -> Vec<crate::ui::palette::Command> {
        let Ok((_, p, _)) = self.project(project_id) else { return Vec::new() };
        p.pages
            .iter()
            .map(|pg| crate::ui::palette::Command { label: format!("Go to {}", pg.title), hint: String::new(), intent: Intent::Go(View::Project { project: p.id.clone(), page: pg.id.clone() }) })
            .collect()
    }

    fn find(&self, id: &str) -> Option<&Opened> {
        self.apps.iter().find(|o| o.rt.revision().definition().id == id)
    }
}

impl Opened {
    fn new(rt: AppRuntime) -> Self {
        Opened { rt, state: AppState::default(), regions: BTreeMap::new(), line: None }
    }
    fn title(&self) -> String {
        let def = self.rt.revision();
        let def = def.definition();
        def.title.clone().unwrap_or_else(|| def.goal.literal_goal.clone())
    }
}

/// app を包んだ Project の id（`ProjectDefinitionV1::wrap` と同じ形）。
fn project_id_of(app_id: &str) -> String {
    format!("project.{app_id}")
}

/// Project の repo の場所（`resources` の `repo` の `file://`）。面の file はここからの相対。
/// id か kind が `"repo"`（W16 の面の file は id、W12W の保存先は kind。1cbff72 が同名の 2 関数を残していたので 1 つに寄せた）。
/// 段 2: `project::context` も面の書類の絶対 path に使う（同じ値を 2 か所で計算しない）。
pub(crate) fn repo_dir(p: &ProjectDefinitionV1) -> Option<std::path::PathBuf> {
    p.resources.iter().find(|r| r.id == "repo" || r.kind == "repo").and_then(|r| r.uri.as_deref()?.strip_prefix("file://")).map(std::path::PathBuf::from)
}

/// `page` に置いた面の id（`source_surface` の `props.surface_ref`・木の順）。
fn surface_refs(page: &project::PageDefinition) -> Vec<&str> {
    page.root.nodes().into_iter().filter_map(|n| n.props.get("surface_ref")?.as_str()).collect()
}

/// M2 G-a: `page` に置いた app の面（`SurfaceTarget::App`・backend は問わない）の bundle id がちょうど 1 つならそれ（`EngineRequest::target`）と、
/// その面の書類の file（repo からの相対のまま・G73 `EngineRequest::document` は file 名・G81 `document_file` は repo の下の場所・面の file が 1 つの時だけ・
/// file の無い面と違う file が 2 つ以上は None）。Url の面は数えない・同じ app の面が 2 つは 1 つ・面の無い Page と違う app が 2 つ以上は None。
fn placed_apps<'a>(p: &'a ProjectDefinitionV1, page: &project::PageDefinition) -> std::collections::BTreeMap<&'a str, std::collections::BTreeSet<Option<&'a str>>> {
    let refs = surface_refs(page);
    p.surface_specs.iter().filter(|s| refs.contains(&s.id.as_str())).fold(Default::default(), |mut m, s| {
        if let project::SurfaceTarget::App { bundle_id, file } = &s.target {
            m.entry(bundle_id.as_str()).or_default().insert(file.as_deref());
        }
        m
    })
}

fn page_app(p: &ProjectDefinitionV1, page: &project::PageDefinition) -> Option<(String, Option<String>)> {
    let apps = placed_apps(p, page);
    let [(app, files)] = <[_; 1]>::try_from(apps.into_iter().collect::<Vec<_>>()).ok()?;
    let document = if files.len() == 1 { files.into_iter().next().flatten().map(str::to_owned) } else { None };
    Some((app.to_owned(), document))
}

/// 今の Page に app がちょうど 1 つならそれ（`page_app` のまま）。app が 2 つ以上なら None（別の Page で代用しない）。
/// app が 0 の時だけ（Dev・Diagram・空の Page）: 一言が file 名（拡張子を除いた語でも、3 字以上）で名指す書類が Project の中に 1 つなら、その Page の app と file。
/// 0 個でも 2 個以上でも None（推測で別の書類に書かない）。short-r3: Architecture diagram の Page から「Change the README title」。
fn engine_target(p: &ProjectDefinitionV1, current: Option<&project::PageDefinition>, goal: &str) -> Option<(String, Option<String>)> {
    if let Some(page) = current {
        if let Some(hit) = page_app(p, page) {
            return Some(hit);
        }
        if !placed_apps(p, page).is_empty() {
            return None;
        }
    }
    let mut hits: Vec<(String, String)> = Vec::new();
    for page in &p.pages {
        if let Some((app, Some(file))) = page_app(p, page) {
            if file_named_in(goal, &file) {
                hits.push((app, file));
            }
        }
    }
    let Ok([one]) = <[_; 1]>::try_from(hits) else { return None };
    Some((one.0, Some(one.1)))
}

/// MIRROR-PIN-1102: Page の Mirror が映している `target` の窓（`MirrorPages::shown` の答えを Page の順に・触った時刻つき）→ `EngineRequest::window`。
/// 0 = None・1 = その窓・2 つ以上 = 一番新しく触った 1 つ。どれも触っていない・一番新しいのが同時 = Err（"{n} {App} windows are on this page · Click the one
/// Shibubu should use, then ask again"・上の面を選ぶ代用はしない = P0）。pid・窓番号が u32 に収まらない面は映していない扱い（数えない）。
/// 同じ窓を 2 つの面が映していても窓は 1 つと数える（聞かない）。
/// 例: `pin_window(&[(gb_42, f64::NEG_INFINITY)]) == Some(Ok((501, 42)))`
/// Wave: MIRROR-PIN-1102
#[cfg(feature = "mirror")]
fn pin_window(shown: &[(crate::surface::mirror::MirrorTarget, f64)]) -> Option<Result<(u32, u32), String>> {
    let mut fit: Vec<((u32, u32), f64)> = shown.iter().filter_map(|(t, at)| Some(((u32::try_from(t.pid).ok()?, u32::try_from(t.window_id).ok()?), *at))).collect();
    // 同じ窓は 1 つ（触った時刻は新しい方を残す）
    fit.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    fit.dedup_by_key(|f| f.0);
    let newest = fit.iter().map(|f| f.1).fold(f64::NEG_INFINITY, f64::max);
    let top: Vec<_> = fit.iter().filter(|f| fit.len() == 1 || (newest.is_finite() && f.1 == newest)).collect();
    match (fit.len(), top.as_slice()) {
        (0, _) => None,
        (_, [one]) => Some(Ok(one.0)),
        (n, _) => {
            let app = shown.first().map_or("", |(t, _)| if t.app_name.is_empty() { t.bundle_id.as_str() } else { t.app_name.as_str() });
            Some(Err(format!("{n} {app} windows are on this page · Click the one Shibubu should use, then ask again")))
        }
    }
}

/// `file` の file 名か、拡張子を除いた語（3 字以上）が `goal` に語として在るか。前後が英数なら語ではない（`READ` は `README` に当たらない）。
fn file_named_in(goal: &str, file: &str) -> bool {
    let name = std::path::Path::new(file).file_name().and_then(|s| s.to_str()).unwrap_or(file);
    let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    word_in(goal, name) || word_in(goal, stem)
}

fn word_in(hay: &str, needle: &str) -> bool {
    if needle.chars().count() < 3 {
        return false;
    }
    let hay: Vec<char> = hay.chars().flat_map(|c| c.to_lowercase()).collect();
    let needle: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    hay.windows(needle.len()).enumerate().any(|(i, w)| {
        w == needle.as_slice()
            && hay.get(i.wrapping_sub(1)).is_none_or(|c| !c.is_ascii_alphanumeric())
            && hay.get(i + needle.len()).is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// 面 `surface_id` を置いた Page の id。どの Page にも無ければ空。
/// G86: 会話の 1 行の Page の名前（「Dev, Tests」）。
fn page_titles(p: &ProjectDefinitionV1) -> String {
    p.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>().join(", ")
}

/// TALK-HONEST-1101 追記 232 の 5: 編集を当てた後の chat の 1 行（`Changed {Project}: {何を変えたか}`）。`next` = 当てた後の Project（Page の題を引く）。
/// 例: 部品の Insert → `Changed Hum to Guitar: added “Guitar” to “Hum to Guitar”`・AddPage → `Changed Hum to Guitar: added the “Tests” page`
/// 名前の分からない操作（Page を消す・面の枠や URL を替える 等）しか無ければ前の形（Page の題の並び）。
fn changed_line(ops: &[ProjectPatchOp], next: &ProjectDefinitionV1) -> String {
    use crate::domain::patch::PatchOp;
    let title = |id: &str| next.page(id).map_or_else(|| id.to_owned(), |pg| pg.title.clone());
    let what: Vec<String> = ops
        .iter()
        .filter_map(|op| match op {
            ProjectPatchOp::AddPage { page, .. } => Some(format!("added the “{}” page", page.title)),
            ProjectPatchOp::RenamePage { title: t, .. } => Some(format!("renamed a page to “{t}”")),
            ProjectPatchOp::AddTerminal { page_id, .. } => Some(format!("added a terminal to “{}”", title(page_id))),
            ProjectPatchOp::MoveSurface { surface_key, to_page, .. } => {
                let name = next.pages.iter().find_map(|pg| pg.root.find(surface_key)).and_then(|n| n.label.clone()).map_or_else(|| "a window".to_owned(), |l| format!("“{l}”"));
                Some(format!("moved {name} to “{}”", title(to_page)))
            }
            ProjectPatchOp::Page { page_id, patch } if patch.op == PatchOp::Insert => {
                let name = patch.value.as_ref().and_then(|v| v["label"].as_str()).map_or_else(|| "a part".to_owned(), |l| format!("“{l}”"));
                Some(format!("added {name} to “{}”", title(page_id)))
            }
            ProjectPatchOp::Page { page_id, .. } => Some(format!("changed “{}”", title(page_id))),
            _ => None,
        })
        .collect();
    let said = if what.is_empty() { page_titles(next) } else { what.join(", ") };
    format!("Changed {}: {said}", next.name)
}

/// ENGINE-GENERIC-1101 追記 234: 状況の「Engine の直近の run」の行の頭（`Generated::situation`・後ろに頼みと結果を ` · ` で）。
const LAST_RUN: &str = "Shibubu's last run here: ";

/// SESSIONS-LEFT-1101 × 追記 234: AI に渡す会話の 1 行の要点（空白を 1 つに・200 字で切る）。`other_sessions` と `situation` の会話の行。
fn brief(text: &str) -> String {
    let s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 200 { format!("{}…", s.chars().take(200).collect::<String>()) } else { s }
}

/// L2（review A・DOGFOOD-X 周 2 回 1 #2）: 案の card の `Preview plan` の読むだけの行 = 作る Page・入る app（面の名前・Web は host・この Mac に無い app は
/// そう言う）・何も作っていない事。`plan` = 案（None = 1 Page の app で足りる一言 = 名前の表の `compose` の題）。model の案（`start_preview`）にも
/// 名前の表の案（app.rs の AI 無しの Preview）にも同じ形で使う。
/// 例: `plan_lines("A weekly meeting notes app …", Some(&plan))` → `["3 pages: Record · Notes · Share", "Apps: REAPER · TextEdit · Discord", PLAN_UNMADE]`
/// TALK-1101: model が「何が出来るか」を書いた案（`ProjectPlan::preview` の summary）は、その 1〜2 文と PLAN_UNMADE だけ（Page と app は `plan_sections` の節）。
pub fn plan_lines(goal: &str, plan: Option<&composer::project::ProjectPlan>) -> Vec<String> {
    use crate::domain::chat::PLAN_UNMADE;
    if let Some(summary) = plan.and_then(|p| p.preview.as_ref()).map(|v| v.summary.trim()).filter(|s| !s.is_empty()) {
        return vec![summary.to_owned(), PLAN_UNMADE.into()];
    }
    let Some(p) = plan.map(|plan| &plan.project) else {
        let title = composer::compose(goal, &ComposeContext::default()).ok().and_then(|b| b.app().title.clone()).unwrap_or_else(|| goal.to_owned());
        return vec![format!("One page: {title}"), PLAN_UNMADE.into()];
    };
    let titles: Vec<&str> = p.pages.iter().map(|g| g.title.as_str()).collect();
    let mut lines = vec![match titles.as_slice() {
        [one] => format!("One page: {one}"),
        many => format!("{} pages: {}", many.len(), many.join(" · ")),
    }];
    let mut apps: Vec<String> = Vec::new();
    for n in p.pages.iter().flat_map(|g| g.root.nodes()) {
        let name = match (n.component.as_str(), n.props.get(composer::project::ASKED_PROP)) {
            ("source_surface", _) => n.label.as_deref().map(|l| l.split_once("://").map_or(l, |(_, rest)| rest.split('/').next().unwrap_or(rest)).to_owned()),
            (_, Some(seat)) => seat.get("app_name").and_then(Value::as_str).map(|a| format!("{a} (not on this Mac)")),
            _ => None,
        };
        if let Some(name) = name.filter(|a| !a.is_empty() && !apps.contains(a)) {
            apps.push(name);
        }
    }
    if !apps.is_empty() {
        lines.push(format!("Apps: {}", apps.join(" · ")));
    }
    lines.push(PLAN_UNMADE.into());
    lines
}

/// TALK-1101（owner 190 通目）: `Preview plan` で card の中に開く見出しつきの節（`Turn::sections`）。model の説明（`ProjectPlan::preview`）が在る案だけ・
/// 名前の表の案と None は空（`plan_lines` の行だけ）。見出しは固定の English・行は model がその人の言葉で書いた物、ただし `On your Mac` は model に書かせず
/// Page の木と面から導く（触る app・録音・terminal・この Mac に無い app と、macOS が 1 回聞く許可 = 案に無い許可を言わない）。空の節は出さない。
/// 例: `plan_sections(Some(&plan))` → `[("On screen", ["GarageBand と鼻歌: 録音の button · GarageBand の窓"]), ("How you use it", ["1. 録音を押す", …]),
/// ("On your Mac", ["GarageBand opens and shows on this page", "Records from your microphone", "macOS asks once to allow Microphone · Screen Recording · Accessibility"]), …]`
pub fn plan_sections(plan: Option<&composer::project::ProjectPlan>) -> Vec<(String, Vec<String>)> {
    use crate::domain::chat::{PLAN_MAC, PLAN_OPEN, PLAN_SCREENS, PLAN_STEPS};
    let Some((p, said)) = plan.and_then(|plan| Some((&plan.project, plan.preview.as_ref()?))) else { return Vec::new() };
    let screens = p.pages.iter().enumerate().map(|(i, page)| match said.screens.get(i).filter(|s| !s.is_empty()) {
        Some(items) => format!("{}: {}", page.title, items.join(" · ")),
        None => page.title.clone(),
    });
    let steps = said.steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1));
    let (mut mac, mut mic, mut screen) = (Vec::new(), false, false);
    for n in p.pages.iter().flat_map(|g| g.root.nodes()) {
        let spec = n.props.get("surface_ref").and_then(Value::as_str).and_then(|id| p.surface_specs.iter().find(|s| s.id == id));
        let line = match (n.component.as_str(), spec, n.props.get(composer::project::ASKED_PROP)) {
            ("source_surface", Some(spec), _) if matches!(spec.target, project::SurfaceTarget::App { .. }) => {
                let name = n.label.as_deref().unwrap_or("The app");
                if spec.backend == project::SurfaceBackendKind::Mirror {
                    screen = true;
                    format!("{name} opens and shows on this page")
                } else {
                    format!("{name} opens in its own window")
                }
            }
            ("recorder", _, _) => {
                mic = true;
                "Records from your microphone".to_owned()
            }
            ("terminal", _, _) => "A terminal opens in your project folder".to_owned(),
            (_, _, Some(seat)) => match seat.get("app_name").and_then(Value::as_str) {
                Some(a) => format!("{a} isn't on this Mac · the page asks you to install it"),
                None => continue,
            },
            _ => continue,
        };
        if !mac.contains(&line) {
            mac.push(line);
        }
    }
    let asks: Vec<&str> = [(mic, "Microphone"), (screen, "Screen Recording"), (screen, "Accessibility")].into_iter().filter(|(on, _)| *on).map(|(_, w)| w).collect();
    if !asks.is_empty() {
        mac.push(format!("macOS asks once to allow {}", asks.join(" · ")));
    }
    [(PLAN_SCREENS, screens.collect::<Vec<_>>()), (PLAN_STEPS, steps.collect()), (PLAN_MAC, mac), (PLAN_OPEN, said.open.clone())]
        .into_iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(head, lines)| (head.to_owned(), lines))
        .collect()
}

/// G86: 結果の 1 行の状態 → 会話の印。
fn mark_of(state: RegionState) -> crate::domain::chat::Mark {
    use crate::domain::chat::Mark;
    match state {
        RegionState::Verified => Mark::Verified,
        RegionState::Partial => Mark::Partial,
        RegionState::Error => Mark::Failed,
        _ => Mark::Stopped,
    }
}

fn page_of_surface(p: &ProjectDefinitionV1, surface_id: &str) -> String {
    p.pages.iter().find(|pg| surface_refs(pg).contains(&surface_id)).map(|pg| pg.id.clone()).unwrap_or_default()
}

/// W16: `page` に置いた Native Direct の面（surface_specs の順）。draw_page の ensure と読み戻し・Page を移る時の読み戻しが回す。
#[cfg(feature = "native_direct")]
fn native_specs<'a>(p: &'a ProjectDefinitionV1, page: &project::PageDefinition) -> Vec<&'a project::SurfaceSpec> {
    let refs = surface_refs(page);
    p.surface_specs.iter().filter(|s| s.backend == project::SurfaceBackendKind::NativeDirect && refs.contains(&s.id.as_str())).collect()
}

/// FX2（FX-006）: Home の生成が AI の失敗で終わった時の次の一手 1 つ（`Build::Ended` の action）。1 行は `LlmError` の Display（表 copy.en.md の `{reason}`）。
/// - `Retry`（Try again）: 時間が経てば戻る・もう一度送れば通りうる = Timeout・RateLimited・Server・Network・Malformed
/// - `Rephrase`（Say it another way）: 言い方で変わる = Refused・Invalid
/// - `UseSimpleRules`: 今は AI で作れない（もう一度送っても同じ）= NoKey・NoCli・Auth・Quota・PlanLimit・OverBudget・Rejected・Schema
/// - None: 利用者が止めた = Cancelled（1 行は `Stopped. Nothing was added to Projects.`）
/// 例: `let action = failure_step(&e); self.ended = Some((RegionState::Error, e.to_string(), action))`
/// 保証する: 1 つの失敗に手は 0 か 1（押せるのに動かない手を返さない = どれも app.rs `Intent::Build` に処理が在る）。
/// 保証しない: login し直す手（Sign in は G99 の口・G99 が merge されたら Auth・NoCli の腕をその手に替える。FX2 は作らない）。
/// Wave: FX2
pub fn failure_step(e: &crate::llm::LlmError) -> Option<crate::ui::build_progress::BuildAction> {
    use crate::llm::LlmError as E;
    use crate::ui::build_progress::BuildAction as A;
    match e {
        E::Timeout { .. } | E::RateLimited { .. } | E::Server { .. } | E::Network { .. } | E::Malformed { .. } => Some(A::Retry),
        E::Refused { .. } | E::Invalid { .. } => Some(A::Rephrase),
        E::NoKey | E::NoCli { .. } | E::Auth { .. } | E::Quota { .. } | E::PlanLimit { .. } | E::OverBudget { .. } | E::Rejected { .. } | E::Schema { .. } => Some(A::UseSimpleRules),
        E::Cancelled { .. } => None,
    }
}

/// CU 所見 1: 使えない答えが `count` 回続いた失敗の次の一手。1 回目は `Try again`・2 回目から `Use simple rules`（その他の失敗の手は変えない）。
fn after_unusable(base: Option<crate::ui::build_progress::BuildAction>, unusable: bool, count: u8) -> Option<crate::ui::build_progress::BuildAction> {
    use crate::ui::build_progress::BuildAction as A;
    match base {
        Some(A::Retry) if unusable && count >= 2 => Some(A::UseSimpleRules),
        other => other,
    }
}

/// FX2（FX-007）: AI の失敗を Runs の行の失敗の語（`runs::error_word`）に（語は本当の理由: CLI が無い・login・鍵 = `Not set up yet`・網と service =
/// `Connection lost`）。Connections にはまだ AI の行が無いので、生成の行は `RunEntry::connect` = false で `Connect` の一手を出さない（FX2 反証の直し・G99 の後で替える）。
fn run_error(e: &crate::llm::LlmError) -> ErrorCode {
    use crate::llm::LlmError as E;
    match e {
        E::Timeout { .. } => ErrorCode::Timeout,
        E::RateLimited { .. } | E::Quota { .. } | E::PlanLimit { .. } | E::OverBudget { .. } => ErrorCode::RateLimited,
        E::NoKey | E::NoCli { .. } | E::Auth { .. } => ErrorCode::NotReady,
        E::Network { .. } | E::Server { .. } => ErrorCode::ConnectionLost,
        E::Refused { .. } | E::Rejected { .. } => ErrorCode::InvalidInput,
        E::Malformed { .. } | E::Invalid { .. } | E::Schema { .. } => ErrorCode::VerificationFailed,
        E::Cancelled { .. } => ErrorCode::Cancelled,
    }
}

/// 新しく作る app の compose の文脈（id は `app.<新しい id>`）。
/// W13: 一言を model に読ませる（`create_project` の UI の thread でも `start_create` の裏の thread でも・`installed_apps` の mdfind と `claude -p` を待つ）。
/// FX2: `cancel` が立つと model を呼ばない・呼んでいれば子を止める（`complete_json_cancellable`）。2 つ目 = 最後の呼び出しの `LlmError`（呼んで答えが届いたら None）。
/// 保証しない: `installed_apps`（mdfind・最長 10 秒）の途中で止まる事（止めた印はその後の model の呼び出しの前に見る）。
fn read_goal(llm: &dyn crate::llm::Llm, goal: &str, ws: Option<&Path>, cancel: &std::sync::atomic::AtomicBool, unanswered: Option<String>) -> (GoalRead, Option<crate::llm::LlmError>) {
    let ((apps, on_mac), apps_line) = match crate::project::installed_apps(std::time::Duration::from_secs(10)) {
        Ok(a) => (a, None),
        Err(why) => ((Vec::new(), Vec::new()), Some(format!("{why}. The project was built without checking the app's name"))),
    };
    let model = LlmModel::new(llm, cancel);
    let plan = composer::project::plan_project(goal, &model, &apps, &on_mac, &ComposeContext { unanswered, ..new_ctx() }, ws);
    ((plan, apps_line), model.failed.into_inner())
}

/// UI-2: model の語（`edit_schema` の 4 つ）で読める文か。名前の表だけの形（左へ・名前替え・走らせる）は model に送らない（図 §36）。
fn model_reads(text: &str) -> bool {
    use composer::project::EditVerb;
    !matches!(EditVerb::read(text), Some(EditVerb::MoveLeft | EditVerb::RenamePage | EditVerb::RunTests))
}

fn new_ctx() -> ComposeContext<'static> {
    ComposeContext { app_id: Some(format!("app.{}", storage::new_id())), ..ComposeContext::default() }
}

/// 一言が既存の Project への変更の文か（名前の表の `EditRequest`）。app.rs の `create_app` は Home の素の一言にだけ当てる（案の Build now には当てない・review A）。
pub(crate) fn said_is_edit(goal: &str) -> bool {
    matches!(composer::compose(goal, &ComposeContext::default()), Err(ComposeError::EditRequest(_)))
}

impl Generated {
    /// Home の「作る」が既存 app への変更だった時、いちばん新しい app に当ててその Page を開く。
    /// app が 0 件なら拒否せず、メモの Project を作ってから同じ文を当てる。
    fn apply_home_edit(&mut self, text: &str, workspace: Option<&Path>) -> Result<View, String> {
        if self.apps.is_empty() {
            // 「メモを作って」は Project を作らない（Ok(None)）。開発環境は Project を作る。
            // repo の dir も渡す（Mirror の窓を選ぶ書類の path の基点・無いと同じ題名の別の書類の窓に打つ・M4 ③）
            if self.create_project("開発環境を作って", workspace, None)?.is_none() {
                return Err(compose_line(ComposeError::EditRequest(text.to_owned())));
            }
        }
        let Some(app) = self.apps.first().map(|o| o.rt.revision().definition().id.clone()) else {
            return Err(compose_line(ComposeError::EditRequest(text.to_owned())));
        };
        let project = project_id_of(&app);
        // 欄を空にする合図（take_edit_applied）は立てない: 文は Home の欄の物（header が送った時に空にした）で、立てると app.rs が
        // 当てた Project の打ちかけ（別の文）を消す（UI-4a 反証）。同じ frame で先に立った合図（裏の model の Edited）は残す
        let before = self.edit_applied.take();
        let r = self.page_intent(&project, PageIntent::Edit { text: text.to_owned() });
        self.edit_applied = before;
        r
    }
}

fn compose_line(e: ComposeError) -> String {
    match e {
        ComposeError::Empty => "Describe what to create in one line".to_owned(),
        ComposeError::Clarify(p) => match p.questions.first() {
            Some(q) => format!("Couldn't read the purpose. {}", q.text),
            None => "Couldn't tell what to build. Say in one line what you want to track, manage or measure.".to_owned(),
        },
        ComposeError::EditRequest(_) => "That reads like a change to an existing project. To create one, start with “Make…”.".to_owned(),
        ComposeError::Invalid(errs) => format!("The built screen failed a check · {} · Say it another way", errs.first().map(String::as_str).unwrap_or("")),
        // W13: 呼ぶ側は名前の表で続けるので、ここに来るのは続けた先も作れなかった時だけ
        ComposeError::Model(why) => format!("AI wasn't available · {why} · Say it another way"),
    }
}

fn read_err(e: storage::StoreError) -> String {
    format!("Couldn't open the saved project · {e} · Try again")
}
fn saved_project(json: serde_json::Value) -> Result<ProjectDefinitionV1, String> {
    serde_json::from_value(json).map_err(|e| format!("This version can't open it ({e})"))
}

/// Project の hash = 正準形 JSON の sha256（承認の hash と同じ作り方。domain は hash を持たない）。
fn project_hash(p: &ProjectDefinitionV1) -> String {
    crate::runtime::permission::digest(&serde_json::to_value(p).unwrap_or_default())
}

/// W6: Page 1 枚の hash（`project_hash` と同じ作り方を `PageDefinition` 全体〈id・route・title・app_ref・木・revision〉に）。
/// 使い方: 差分の前後で `before.pages.iter().map(|pg| (pg.id.clone(), page_hash(pg)))` を比べ、触っていない Page の値が同じ事を見る（W6・W7 の受け入れ）。
/// 保証する: 同じ Page なら同じ値。`project::apply` が触らない Page（MovePage の移した Page を含む）は前後で同じ値。
/// 保証しない: Terminal の PID・surface の窓 id と pid（Page の外。統合の後もこの比べは Terminal を見ない・未測・窓は M1 の後に `tests/common/readback.rs` の `l4_same_windows`）。
/// Wave: W6
#[cfg_attr(not(test), allow(dead_code))] // 今は試験だけが使う（W7・W8 の受け入れも同じ比べ）
pub fn page_hash(page: &project::PageDefinition) -> String {
    crate::runtime::permission::digest(&serde_json::to_value(page).unwrap_or_default())
}

/// Runs の「With」に出す AI の名前（`Llm::label` の `claude-cli/2.1` → `Claude`）。
fn ai_name(label: &str) -> String {
    let head = label.split('/').next().unwrap_or(label).trim_end_matches("-cli");
    let mut c = head.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// FX-043: 同じ button の同じ中身を `PRESS_WINDOW_SECS` の間にもう 1 回押した時に同じになる idempotency key（外へ出る手だけ）。
/// 窓は時計の区切りでなく最初の押下から数える（区切りをまたぐ 2 度押しも同じ key・REVIEW-FX2 a1）。窓を過ぎた押下は別の key。
const PRESS_WINDOW_SECS: u64 = 3;
fn press_key(node_key: &str, input: &Value, now_secs: u64) -> String {
    use std::hash::{Hash, Hasher};
    static FIRST: std::sync::Mutex<BTreeMap<String, u64>> = std::sync::Mutex::new(BTreeMap::new());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    input.to_string().hash(&mut h);
    let id = format!("press:{node_key}:{:016x}", h.finish());
    let mut first = FIRST.lock().unwrap_or_else(|e| e.into_inner());
    if first.len() > 256 {
        first.clear();
    }
    let anchor = match first.get(&id) {
        Some(&t) if now_secs >= t && now_secs - t < PRESS_WINDOW_SECS => t,
        _ => now_secs,
    };
    first.insert(id.clone(), anchor);
    format!("{id}:{anchor}")
}

/// renderer の ActionHost を runtime に繋ぐ。
/// 契約（UI-6a・sendback 60.3 で `line` の型を変えた）: `line` = 押した結果のうち Verified でない直近の 1 行と状態（`Outcome` の写し: Unverified → Partial・
/// Rejected / Failed / 呼べない → Error・EffectUnknown → Stale ＋ `ActionHost::locked`・NeedsApproval → 承認の道〈60.2〉まで今の Blocked ＋ `locked`）。
/// 描く側（`Generated::draw`・`draw_page`）は `states::inline(ui, state, line)`。button から押した Blocked と Stale は gate の 1 行が出すので `line` に入れない。
/// 型を変えた理由: EffectUnknown・NeedsApproval・Unverified が Page の上に `× Failed · …` で出る（嘘の Failed）のを、文を描く所で状態を取り戻せないため。
/// 呼ぶ側の例: `if let Some((state, line)) = host.line.as_ref() { states::inline(ui, *state, line) }`。保証しない: 1 行の置き場（題の行への集約は UI-6b）。
/// Wave: UI-6a
pub struct Host<'a> {
    pub rt: &'a AppRuntime,
    pub regions: &'a mut BTreeMap<String, (RegionState, String)>,
    pub line: &'a mut Option<(RegionState, String)>,
    /// W3: Project の面ごとの最新の読み戻し（`draw_page` が `surface_receipts` で入れる・app だけの画面では空）。
    pub receipts: Vec<SurfaceReceipt>,
    /// W5: Project の Page の時だけ Some（app 単体の画面・試験の `open_app` は None = 「Project の Page にだけ置けます」）。
    term: Option<TermHost<'a>>,
    /// W16-1: Native Direct で重ねる面の枠の今（`draw_page` が `NativeSurfaces::view` で入れる・feature 無しは空）。
    pub portals: BTreeMap<String, crate::ui::portal_frame::PortalView>,
    /// W16-1: renderer が描いた枠の画面座標と押された一手（`draw_page` が描いた後に `NativeSurfaces::on_frame` へ流す）。
    pub placed: Vec<(String, crate::surface::Host, Option<crate::ui::portal_frame::PortalAction>)>,
    /// W12W-1: Project の面の起こし方（`draw_page` が入れる・app だけの画面では空 = 何も置かない）
    pub surfaces: Vec<project::SurfaceSpec>,
    /// W12W-1: 面の key の Project（`draw_page` が入れる・`web::key(project, surface_id)`。どの Project も Diagram の面の id は同じ）
    pub project: String,
    /// W12W-1: Page に置く WebView（`embeds` の面の矩形をここへ渡す）
    pub web: Option<&'a mut crate::project::web::PageWebViews>,
    /// G88b: Project の Page の録音の帯（`draw_page` だけ Some）
    takes: Option<TakeHost<'a>>,
    /// RQ1: 最初の action の行の書き手（`open_app` / `open_page_app` が data dir の記録を渡す・試験と画面の外の評価は None = 書かない）
    usage: Option<&'a mut UsageLog>,
}

/// G88b: Host が録音の帯を引く口（帯の今・recorder_ref ごとの入れる先の面・届く面の id）。
/// ENGINE-GENERIC-1101: `feeds`・`reached` は録画（11/02）の `Add take to REAPER`（`TAKE_APPS`）のためだけに残す。録画の後に消す。
struct TakeHost<'a> {
    slots: &'a mut Takes,
    feeds: BTreeMap<String, Option<project::SurfaceSpec>>,
    reached: Vec<String>,
}

impl TakeHost<'_> {
    fn add(&self, r: &str) -> Option<renderer::AddTake> {
        let feeds = self.feeds.get(r)?.as_ref();
        add_take(feeds, feeds.is_some_and(|s| self.reached.contains(&s.id)))
    }
}

/// G88b: 録音の帯の今の全部（process の間だけ・保存しない）。
struct Takes {
    dir: std::path::PathBuf,
    by_ref: BTreeMap<String, TakeSlot>,
}

/// G88b: 帯 1 つの今。録る・入れるは裏の thread（UI を止めない）・毎 frame `poll` で終わりを拾う。
#[derive(Default)]
struct TakeSlot {
    stage: renderer::TakeStage,
    path: Option<std::path::PathBuf>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    rolling: Arc<Mutex<Option<std::time::Instant>>>,
    job: Option<std::thread::JoinHandle<Result<crate::audio::record::Take, String>>>,
    adding: Option<(String, std::thread::JoinHandle<Result<crate::audio::reaper::Receipt, String>>)>,
    preview: Option<crate::audio::preview::Preview>,
    line: String,
    /// ENGINE-GENERIC-1101: 録り終えてまだ部品の手（`recorded`）として渡していない（`poll` が立て、`draw_page` が `Generated::part_event` に渡して下ろす）
    fresh: bool,
}

/// G88b: 録る裏の thread が mic の拒否を返す印（帯は `MicOff` の形にする）
const MIC_OFF: &str = "mic-off";
/// G88b: 帯 1 回の録音の上限（plan の recorder の node の `max_seconds` と同じ）
const TAKE_MAX_SECONDS: f32 = 60.0;
/// G88b: take を入れる手の上限（REAPER の script の往復・混んだ機械）
const TAKE_INSERT_TIMEOUT: Duration = Duration::from_secs(90);

impl TakeSlot {
    /// 裏の thread の終わりを拾って段を進める（待たない）。
    fn poll(&mut self) {
        if let Some(job) = self.job.take_if(|j| j.is_finished()) {
            match job.join().unwrap_or_else(|_| Err("Recording stopped unexpectedly. Record again".into())) {
                Ok(take) => {
                    let peaks = crate::audio::preview::peaks(&take.path, 64).unwrap_or_default().into_iter().map(|(lo, hi)| lo.abs().max(hi.abs())).collect();
                    self.stage = renderer::TakeStage::Recorded { seconds: take.info.seconds(), peaks };
                    self.line = take.notes.join(" · ");
                    self.path = Some(take.path);
                    self.fresh = true;
                }
                Err(e) if e == MIC_OFF => self.stage = renderer::TakeStage::MicOff,
                Err(e) => {
                    self.stage = renderer::TakeStage::Idle;
                    self.line = e;
                }
            }
        } else if self.job.is_some() {
            let since = *self.rolling.lock().unwrap_or_else(|e| e.into_inner());
            self.stage = renderer::TakeStage::Recording { seconds: since.map_or(0.0, |t| t.elapsed().as_secs_f32()) };
        }
        if let Some((app, job)) = self.adding.take_if(|(_, j)| j.is_finished()) {
            let r = job.join().unwrap_or_else(|_| Err(format!("Couldn't add the take to {app}")));
            self.line = receipt_line(&app, &r);
        }
        if self.preview.as_mut().is_some_and(|p| !p.is_playing()) {
            self.preview = None;
        }
    }

    /// 入れられるか（take が在る・入れる先が在り押せる・入れている最中でない）。
    fn check_add(&self, add: Option<&renderer::AddTake>) -> Result<(), String> {
        self.path.as_ref().ok_or("Record a take first")?;
        let add = add.ok_or("This page has no app to add the take to")?;
        if let Some(why) = &add.blocked {
            return Err(why.clone());
        }
        if self.adding.is_some() {
            return Err(format!("Still adding to {}", add.app));
        }
        Ok(())
    }

    /// 帯で押された手（`ActionHost::take_act`）。`add` = 入れる先（`add_take` の答え）・`bundle` = その面の bundle id。
    fn act(&mut self, act: renderer::TakeAct, r: &str, dir: &Path, add: Option<renderer::AddTake>, bundle: Option<&str>) -> Result<(), String> {
        use renderer::TakeAct;
        use std::sync::atomic::{AtomicBool, Ordering};
        let open = |args: &[&str]| run_open("/usr/bin/open", args);
        match act {
            TakeAct::Record => {
                if self.job.is_some() {
                    return Err("Already recording".into());
                }
                std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make the takes folder ({e})"))?;
                let out = dir.join(format!("{r}.wav"));
                *self = TakeSlot { stop: Arc::new(AtomicBool::new(false)), ..TakeSlot::default() };
                let (stop, rolling) = (self.stop.clone(), self.rolling.clone());
                self.job = Some(std::thread::spawn(move || {
                    use crate::audio::record;
                    if !record::fake_enabled() && record::mic_permission(Duration::from_secs(10)).refusal().is_some() {
                        return Err(MIC_OFF.into());
                    }
                    record::record(TAKE_MAX_SECONDS, &out, &stop, || *rolling.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now()))
                }));
                self.stage = renderer::TakeStage::Recording { seconds: 0.0 };
                Ok(())
            }
            TakeAct::Stop => {
                self.stop.store(true, Ordering::SeqCst);
                Ok(())
            }
            TakeAct::Play => {
                let path = self.path.as_ref().ok_or("Record a take first")?;
                self.preview = Some(crate::audio::preview::Preview::start(path)?);
                Ok(())
            }
            TakeAct::Add => {
                self.check_add(add.as_ref())?;
                let (Some(path), Some(add)) = (self.path.clone(), add) else { return Err("Record a take first".into()) };
                let insert = TAKE_APPS.iter().find(|(b, _, _)| Some(*b) == bundle).map(|(_, _, f)| *f).ok_or("This page has no app to add the take to")?;
                self.line = format!("Adding to {}…", add.app);
                self.adding = Some((add.app, std::thread::spawn(move || insert(&path, TAKE_INSERT_TIMEOUT))));
                Ok(())
            }
            TakeAct::Discard => {
                if let Some(p) = self.path.take() {
                    let _ = std::fs::remove_file(p);
                }
                *self = TakeSlot::default();
                Ok(())
            }
            TakeAct::Reveal => open(&["-R", &self.path.as_ref().ok_or("Record a take first")?.display().to_string()]),
            TakeAct::MicSettings => open(&["x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"]),
        }
    }
}

/// G94: `Layout: {name}` が回す型の id（G88a の model の置き場 → 同梱の順）のうち `page` に効く物。`page` が None なら空。例: `placement_ids(&p, p.page(id))`
fn placement_ids(p: &ProjectDefinitionV1, page: Option<&PageDefinition>) -> Vec<String> {
    let Some(page) = page else { return Vec::new() };
    composer::layout::seat_template(p, None).into_iter().chain(composer::layout::bundled_templates()).filter(|t| composer::layout::applies_to(t, page)).map(|t| t.id).collect()
}

/// G94: ✦ rail の `About` が Project の時に見せる (項目, 値)。旧 canvas の Session でなく今開いている Project の事。例: `project_about_rows(&p)`
fn project_about_rows(p: &ProjectDefinitionV1) -> Vec<(String, String)> {
    vec![
        ("Name".into(), p.name.clone()),
        ("Pages".into(), p.pages.len().to_string()),
        ("Folder".into(), repo_dir(p).map_or("Not set".into(), |d| d.display().to_string())),
        ("Autonomous actions".into(), "None. Shibubu asks before every run".into()),
    ]
}

/// G94: `/usr/bin/open` を 1 回（終わりを待つ）。起こせない・exit code が 0 でない（System Settings / Finder が開かなかった）なら帯の 1 行になる Err。
/// 例: `run_open("/usr/bin/open", &["-R", path])`。`bin` は試験が差し替える。
/// FX1（FX-003）: app.rs の隔離した file の `Reveal in Finder` も同じ口（private → pub(crate)・本体は同じ）。
/// Wave: FX1
pub(crate) fn run_open(bin: &str, args: &[&str]) -> Result<(), String> {
    match std::process::Command::new(bin).args(args).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("Couldn't open it (exit code {})", s.code().map_or("none".to_owned(), |c| c.to_string()))),
        Err(e) => Err(format!("Couldn't open it ({e})")),
    }
}

/// W5: Host が Page の Terminal を引く口（manager・記録・表示の状態・この Project の spec）。
struct TermHost<'a> {
    mgr: &'a TerminalManager,
    store: &'a Mutex<Store>,
    views: &'a mut TermViews,
    specs: Vec<project::TerminalSessionSpec>,
    /// この Page の席。Drawer はまだ本物の PTY を出さない（D-e の 1 行）。
    view: InputView,
    /// L7-ORCA #9: grid の右 click で選んだ panel の手（frame の後に `panel_act` へ・Page の Terminal の host は受けない）
    picked: Option<PanelAct>,
}

impl TermHost<'_> {
    fn spec(&self, terminal_ref: &str) -> Option<&project::TerminalSessionSpec> {
        self.specs.iter().find(|t| t.id == terminal_ref)
    }

    fn pane(&mut self, terminal_ref: &str) -> Option<TerminalPane> {
        let spec = self.spec(terminal_ref)?.clone();
        let key = SessionKey::of(&spec);
        let in_mgr = self.mgr.list(Some(&key.project_id)).into_iter().find(|i| i.key == key);
        // 起こせなかった（誤りの 1 行がある）間は起こし直さない
        let info = match (in_mgr, self.views.lines.get(&key)) {
            (Some(i), _) => Ok(i),
            (None, Some(line)) => Err(line.clone()),
            (None, None) => ensure_spec(self.mgr, self.store, &spec),
        };
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                // TERM-RESTORE-1101: 起こせなかった時だけ知らせる（toast・1 回）。tab の本体にも理由と Restart（もう一度起こす）
                if self.views.lines.insert(key, e.clone()).is_none() {
                    self.views.failed.push(e.clone());
                }
                return Some(TerminalPane { status_line: e, view: Some(self.view.clone()), ..TerminalPane::default() });
            }
        };
        // 前の起動の session は manager に無い（写す画面が無い）。終わった session は最後の画面を写す
        let screen = match info.status {
            SessionStatus::Unavailable => None,
            _ => self.mgr.screen(&key, HISTORY_SHOWN).map_err(|e| self.views.lines.insert(key.clone(), e.to_string())).ok(),
        };
        let name = Path::new(&info.shell).file_name().map_or(info.shell.clone(), |n| n.to_string_lossy().into_owned());
        let mut status_line = match &info.status {
            SessionStatus::Running => format!("{name} · pid {} · {}", info.pid, composer::project::display_path(&info.working_directory)),
            SessionStatus::Exited { exit } => match (&exit.signal, exit.code) {
                (Some(sig), _) => format!("Shell stopped ({sig}) · Restart"),
                (None, Some(code)) => format!("Shell exited (code {code}) · Restart"),
                (None, None) => "Shell exited · Restart".to_owned(),
            },
            // TERM-RESTORE-1101: 前の起動の session は `ensure_spec` が起こし直す（ここへは来ない）
            SessionStatus::Unavailable => "Shell exited · Restart".to_owned(),
        };
        if let Some(line) = self.views.lines.get(&key) {
            status_line = line.clone();
        }
        let running = info.status == SessionStatus::Running;
        let text = screen.as_ref().map(|s| s.text()).unwrap_or_default();
        Some(TerminalPane { text, screen, pid: running.then_some(info.pid), accepts_input: running, status_line, view: Some(self.view.clone()) })
    }
}

impl TermHost<'_> {
    /// Terminal 1 つへの操作（送る・打鍵・大きさ）。誤りは Terminal の状態の 1 行に出し（D-e）、通れば前の誤りを消す。Page の席（`Host`）と panel（G95）が同じ口を通る。
    fn term_do(&mut self, terminal_ref: &str, f: impl FnOnce(&TerminalManager, &SessionKey) -> Result<(), crate::terminal::TerminalError>) -> Result<(), String> {
        let key = SessionKey::of(self.spec(terminal_ref).ok_or("This terminal is no longer in the project · Add terminal")?);
        let r = f(self.mgr, &key).map_err(|e| e.to_string());
        match &r {
            Ok(()) => self.views.lines.remove(&key),
            Err(e) => self.views.lines.insert(key, e.clone()),
        };
        r
    }
}

/// G95: panel の Terminal を描く host（`renderer::components::terminal_body` に渡す）。席は `InputView::Drawer`（旧い引き出しの席）。
/// Terminal の口だけを持ち、`invoke` は何もしない（panel に action の node は無い）。
impl ActionHost for TermHost<'_> {
    fn invoke(&mut self, _inv: ActionInvocation) {}

    fn terminal(&mut self, terminal_ref: &str) -> Option<TerminalPane> {
        self.pane(terminal_ref)
    }

    fn take_reveal(&mut self, part_ref: &str) -> bool {
        self.views.reveal.take_if(|r| r.as_str() == part_ref).is_some()
    }

    fn terminal_send(&mut self, terminal_ref: &str, line: &str) -> Result<(), String> {
        self.terminal_keys(terminal_ref, format!("{line}\n").as_bytes())
    }

    fn terminal_restart(&mut self, terminal_ref: &str) -> Result<(), String> {
        let spec = self.spec(terminal_ref).ok_or("This terminal is no longer in the project · Add terminal")?.clone();
        restart_spec(self.mgr, self.store, self.views, &spec).map(|_| ())
    }

    fn terminal_keys(&mut self, terminal_ref: &str, bytes: &[u8]) -> Result<(), String> {
        let view = self.view.clone();
        self.term_do(terminal_ref, |m, k| m.write_as(k, InputOwner::Human, &view, bytes))
    }

    fn terminal_resize(&mut self, terminal_ref: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.term_do(terminal_ref, |m, k| m.resize(k, TermSize { rows, cols }))
    }

    fn terminal_place(&mut self, terminal_ref: &str, rect: egui::Rect, shown: egui::Rect, focus: bool) -> Option<renderer::TermPlaced> {
        let key = SessionKey::of(self.spec(terminal_ref)?);
        let seat = self.view.clone();
        self.views.place(key, rect, shown, focus, seat)
    }

    fn terminal_touch_human(&mut self, terminal_ref: &str, view: InputView) -> Result<(), String> {
        self.term_do(terminal_ref, |m, k| m.touch_human(k, view))
    }

    fn terminal_release_human(&mut self, terminal_ref: &str, view: &InputView) -> Result<(), String> {
        let view = view.clone();
        self.term_do(terminal_ref, move |m, k| m.release_human(k, &view))
    }

    fn terminal_lease(&self, terminal_ref: &str) -> Option<InputLease> {
        self.mgr.lease(&SessionKey::of(self.spec(terminal_ref)?)).ok()
    }

    fn terminal_panel_act(&mut self, _terminal_ref: &str, act: PanelAct) -> Result<(), String> {
        self.picked = Some(act);
        Ok(())
    }
}

impl Host<'_> {
    fn human_view(&self) -> InputView {
        self.term.as_ref().map(|t| t.view.clone()).unwrap_or(InputView::Drawer)
    }

    /// Page の Terminal 1 つへの操作（`TermHost::term_do` へ）。
    fn term_do(&mut self, terminal_ref: &str, f: impl FnOnce(&TerminalManager, &SessionKey) -> Result<(), crate::terminal::TerminalError>) -> Result<(), String> {
        self.term.as_mut().ok_or("No terminal on this page")?.term_do(terminal_ref, f)
    }

    /// 押した結果を `regions`（button の node）と `line`（Page の上の 1 行）に写す（`invoke` の後半・runtime を通さずに試験できる口）。
    fn record(&mut self, key: Option<String>, r: Result<Invoked, ErrorCode>) {
        let shown = match r {
            Ok(Invoked::Done(receipt)) => match receipt.outcome {
                Outcome::Verified { .. } => None,
                Outcome::Unverified { reason } => Some((RegionState::Partial, format!("Saved, not checked · {reason}"))),
                Outcome::Rejected { reason } => Some((RegionState::Error, format!("Declined · {reason}"))),
                Outcome::Failed { error } => Some((RegionState::Error, format!("Failed · {}", wording(error)))),
                // 送ったか分からない: 送り直さない（押せない = `locked`）。承認の button は無いので ▣ にせず Stale（60.1）
                Outcome::EffectUnknown { reason } => Some((RegionState::Stale, format!("Can't tell if it arrived · {reason} · Open Runs"))),
            },
            // 状態の語が Needs approval なので detail は行き先だけ（`Needs approval · Needs approval` にしない）。承認待ちの run は Runs に在る
            // （承認の button はまだ無い = sendback 60.2）。Blocked は bindings の gate も兼ねる（button の連打は key が無く、押すたびに承認待ちが増える）
            Ok(Invoked::NeedsApproval(_)) => Some((RegionState::Blocked, "Open Runs".to_owned())),
            Err(e) => Some((RegionState::Error, format!("Couldn't run it · {}", wording(e)))),
        };
        // button の行（gate の 1 行）が押せない状態（承認待ち・送ったか分からない）と文を出すので、上の 1 行に重ねない
        let on_button = key.is_some();
        if let Some(k) = key {
            match &shown {
                Some(s) => {
                    self.regions.insert(k, s.clone());
                }
                None => {
                    self.regions.remove(&k);
                }
            }
        }
        *self.line = shown.filter(|(state, _)| !(on_button && matches!(state, RegionState::Blocked | RegionState::Stale)));
    }
}

impl ActionHost for Host<'_> {
    /// registry に無い action（composer が作ったが runtime が持たない id）は押せない（理由つき）。
    fn readiness(&self, action_ref: &str) -> Option<ActionReadiness> {
        let reg = self.rt.runtime().registry();
        match reg.get(action_ref) {
            None => Some(ActionReadiness::Unsupported),
            Some(r) if !reg.adapter_ready(r.contract.adapter) => Some(ActionReadiness::RequiresConnection),
            Some(_) => None,
        }
    }

    fn data(&self, binding: &DataBinding) -> Option<Value> {
        let doc = self.rt.ui_state(&binding.source_ref).ok()?;
        at_path(&doc, &binding.path).filter(|v| !v.is_null()).cloned()
    }

    fn region(&self, node_key: &str) -> Option<(RegionState, String)> {
        self.regions.get(node_key).cloned()
    }

    /// 60.1: `regions` は `invoke` だけが入れる。その Stale（送ったか分からない）と Blocked（承認待ち）は押し直すと 2 回届く。
    fn locked(&self, node_key: &str) -> bool {
        matches!(self.regions.get(node_key), Some((RegionState::Stale | RegionState::Blocked, _)))
    }

    fn surface_receipt(&self, surface_id: &str) -> Option<SurfaceReceipt> {
        self.receipts.iter().find(|r| r.surface_id == surface_id).cloned()
    }

    fn surface_portal(&self, surface_id: &str) -> Option<crate::ui::portal_frame::PortalView> {
        self.portals.get(surface_id).cloned()
    }

    fn surface_placed(&mut self, surface_id: &str, at: crate::surface::Host, action: Option<crate::ui::portal_frame::PortalAction>) {
        self.placed.push((surface_id.to_string(), at, action));
    }

    /// Web の面（webview・Url）だけ Page に置く。WebView の無い build でも置く（矩形の中で理由と「browser で開く」を言う・D-e）。
    fn embeds(&self, surface_id: &str) -> bool {
        self.web.is_some() && self.surfaces.iter().any(|s| s.id == surface_id && crate::project::web::web_spec(s, None).is_some())
    }

    fn place_surface(&mut self, surface_id: &str, slot: egui::Rect, shown: egui::Rect) {
        let Some(spec) = self.surfaces.iter().find(|s| s.id == surface_id) else { return };
        let Some(web) = self.web.as_deref_mut() else { return };
        if let Some(w) = crate::project::web::web_spec(spec, Some(web.download_dir().to_path_buf())) {
            let frame = |r: egui::Rect| crate::surface::Frame { x: r.min.x as f64, y: r.min.y as f64, w: r.width() as f64, h: r.height() as f64 };
            web.place(&crate::project::web::key(&self.project, surface_id), w, spec.keep_alive, frame(slot), frame(shown));
        }
    }

    fn browser_go(&mut self, surface_id: &str, input: &str) {
        let Some(web) = self.web.as_deref_mut() else { return };
        web.queue_go(surface_id.to_owned(), input.to_owned());
    }

    fn browser_back(&mut self, surface_id: &str) {
        let Some(web) = self.web.as_deref_mut() else { return };
        web.go_back(&crate::project::web::key(&self.project, surface_id));
    }

    fn browser_forward(&mut self, surface_id: &str) {
        let Some(web) = self.web.as_deref_mut() else { return };
        web.go_forward(&crate::project::web::key(&self.project, surface_id));
    }

    fn browser_reload(&mut self, surface_id: &str) {
        let Some(web) = self.web.as_deref_mut() else { return };
        web.reload(&crate::project::web::key(&self.project, surface_id));
    }

    fn browser_frame(&mut self, surface_id: &str, x: f64, y: f64, w: f64, h: f64) {
        let Some(web) = self.web.as_deref_mut() else { return };
        web.queue_frame(surface_id.to_owned(), x, y, w, h);
    }

    fn browser_url(&self, surface_id: &str) -> Option<String> {
        let key = crate::project::web::key(&self.project, surface_id);
        if let Some(u) = self.web.as_deref().and_then(|w| w.url(&key)) {
            return Some(u);
        }
        self.surfaces.iter().find(|s| s.id == surface_id).and_then(|s| match &s.target {
            project::SurfaceTarget::Url { url } => Some(url.clone()),
            _ => None,
        })
    }

    fn next_change(&self) -> Option<std::time::Duration> {
        self.rt.next_change()
    }

    fn invoke(&mut self, inv: ActionInvocation) {
        let key = match &inv.origin {
            InvocationOrigin::Button { node_key } => Some(node_key.clone()),
            _ => None,
        };
        // FX-043: 外へ出る手（送る・外へ書く）は button から idempotency key を渡す。押した直後に同じ中身をもう 1 回押しても（反応が遅くて 2 回押した）、
        // runtime は 2 回目に 1 回目の Receipt を返すだけで送り直さない。時間の区切り（`PRESS_WINDOW_SECS`）を越えた同じ中身は別の押下
        let external = self.rt.revision().definition().action(&inv.action_ref).is_some_and(|a| matches!(a.effect, crate::domain::action::Effect::ExternalSend | crate::domain::action::Effect::ExternalWrite));
        let press = match (&inv.origin, external) {
            (InvocationOrigin::Button { node_key }, true) => Some(press_key(node_key, &inv.input, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))),
            _ => None,
        };
        let r = self.rt.invoke_keyed(&inv.action_ref, inv.input, inv.origin, press.as_deref());
        let button = key.is_some();
        if button && matches!(&r, Ok(Invoked::Done(d)) if matches!(d.outcome, Outcome::Verified { .. })) {
            if let Some(u) = self.usage.as_deref_mut() {
                u.note_action();
            }
        }
        self.record(key, r);
    }

    fn terminal(&mut self, terminal_ref: &str) -> Option<TerminalPane> {
        self.term.as_mut()?.pane(terminal_ref)
    }

    fn take_reveal(&mut self, part_ref: &str) -> bool {
        let Some(t) = self.term.as_mut() else { return false };
        t.views.reveal.take_if(|r| r.as_str() == part_ref).is_some()
    }

    /// G88b: 録音の帯の今（Project の Page の時だけ）。裏の thread の終わりはここで拾う。
    fn take(&mut self, recorder_ref: &str) -> Option<renderer::TakePane> {
        let t = self.takes.as_mut()?;
        let add = t.add(recorder_ref);
        let slot = t.slots.by_ref.entry(format!("{}/{recorder_ref}", self.project)).or_default();
        slot.poll();
        Some(renderer::TakePane { stage: slot.stage.clone(), add, line: slot.line.clone() })
    }

    /// G88b: 帯で押された手。拒否の理由は帯の 1 行（次の frame の `take` の `line`）。
    fn take_act(&mut self, recorder_ref: &str, act: renderer::TakeAct) -> Result<(), String> {
        let t = self.takes.as_mut().ok_or("No recorder on this page")?;
        let add = t.add(recorder_ref);
        let bundle = t.feeds.get(recorder_ref).cloned().flatten().and_then(|s| match s.target {
            project::SurfaceTarget::App { bundle_id, .. } => Some(bundle_id),
            project::SurfaceTarget::Url { .. } => None,
        });
        let dir = t.slots.dir.clone();
        let slot = t.slots.by_ref.entry(format!("{}/{recorder_ref}", self.project)).or_default();
        let r = slot.act(act, recorder_ref, &dir, add, bundle.as_deref());
        if let Err(e) = &r {
            slot.line = e.clone();
        }
        r
    }

    fn terminal_send(&mut self, terminal_ref: &str, line: &str) -> Result<(), String> {
        self.terminal_keys(terminal_ref, format!("{line}\n").as_bytes())
    }

    fn terminal_restart(&mut self, terminal_ref: &str) -> Result<(), String> {
        let t = self.term.as_mut().ok_or("No terminal on this page")?;
        let spec = t.spec(terminal_ref).ok_or("This terminal is no longer in the project · Add terminal")?.clone();
        restart_spec(t.mgr, t.store, t.views, &spec).map(|_| ())
    }

    /// W10: grid に focus の間の打鍵・貼り付け・IME の確定（人の入力・`terminal_send` と同じ口）。
    fn terminal_keys(&mut self, terminal_ref: &str, bytes: &[u8]) -> Result<(), String> {
        let view = self.human_view();
        self.term_do(terminal_ref, |m, k| m.write_as(k, InputOwner::Human, &view, bytes))
    }

    /// W10: Page が描ける大きさを PTY と VT に（`TerminalManager::resize` の 1 つの口）。0 は「窓を広げてください」の 1 行。
    fn terminal_resize(&mut self, terminal_ref: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.term_do(terminal_ref, |m, k| m.resize(k, TermSize { rows, cols }))
    }

    /// TERM-XTERM-1101: Page の Terminal も panel と同じ描き手（`TermViews::place`）。
    fn terminal_place(&mut self, terminal_ref: &str, rect: egui::Rect, shown: egui::Rect, focus: bool) -> Option<renderer::TermPlaced> {
        let seat = self.human_view();
        let t = self.term.as_mut()?;
        let key = SessionKey::of(t.spec(terminal_ref)?);
        t.views.place(key, rect, shown, focus, seat)
    }

    fn terminal_write(&mut self, terminal_ref: &str, owner: InputOwner, view: &InputView, bytes: &[u8]) -> Result<(), String> {
        let view = view.clone();
        self.term_do(terminal_ref, move |m, k| m.write_as(k, owner, &view, bytes))
    }

    fn terminal_touch_human(&mut self, terminal_ref: &str, view: InputView) -> Result<(), String> {
        self.term_do(terminal_ref, |m, k| m.touch_human(k, view))
    }

    fn terminal_release_human(&mut self, terminal_ref: &str, view: &InputView) -> Result<(), String> {
        let view = view.clone();
        self.term_do(terminal_ref, move |m, k| m.release_human(k, &view))
    }

    fn terminal_lease(&self, terminal_ref: &str) -> Option<InputLease> {
        let t = self.term.as_ref()?;
        let spec = t.spec(terminal_ref)?;
        t.mgr.lease(&SessionKey::of(spec)).ok()
    }

    fn run_receipts(&self, project_id: &str) -> Vec<RunReceipt> {
        let Some(t) = self.term.as_ref() else {
            return Vec::new();
        };
        let st = t.store.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(rows) = st.list_run_receipts(project_id, None) else {
            return Vec::new();
        };
        drop(st);
        rows.into_iter().filter_map(|row| serde_json::from_value(row.json).ok()).collect()
    }
}

// ============================== W14b 使える検査の Probe（`composer::verifier::usable` が呼ぶ） ==============================

/// W14b: Terminal の席を測り始めた印の key（`Generated::usable_started`）。
fn usable_seat_key(project_id: &str, seat_id: &str) -> String {
    format!("{project_id}\u{1f}{seat_id}")
}

/// W14b: 測り終えた席を Project の答えに足す（同じ席は新しい答えに替える・他の席の答えは残す）。
fn merge_usable(done: &mut BTreeMap<String, Vec<Usable>>, id: String, list: Vec<Usable>) {
    let all = done.entry(id).or_default();
    all.retain(|u| !list.iter().any(|n| n.seat.id == u.seat.id));
    all.extend(list);
}

/// W14b: 1 手で入れる印（run ごとに新しい）。
fn usable_mark() -> String {
    format!("usable-{}", storage::new_id())
}

/// W13b: この branch の `domain::project::apply` が `AddSurface`（W16M-3 の本体）を当てられるか。false の間は「代わりに <app>」の答えを
/// 当てず理由の 1 行にする（`apply_unchecked` の todo!() で .app を落とさない）。trunk（W16M-3 の AddSurface）に入ったので true（`w13b_add_surface_guard_matches_this_build` が守る）。
const ADD_SURFACE_BUILT: bool = true;

/// Page の描く木（保存した木は変えない・前の版は今まで通り描く）。外す物:
/// - W13b の反証の直し: 1 問の node（`SeatV1::asked_in` が Some）。その node の 1 行（前の版が描く「入れてから…送ってください」）と、
///   この Page の使える検査の 1 行の問い（「下から 1 つ選んでください」）が食い違う指示を 2 つ出さない。問いと答えは使える検査の 1 行が持つ（MissingApp の席は毎 frame 測らずに出る・`usable_of`）。
/// - TERM-ONE-1101（owner 183）: `terminal` の node。plan の Terminal は下の panel の tab（`Generated::adopt_plan_terminals`）= Page に小さな Terminal を描かない。
/// - TERM-ONE-1101（owner 185）× NOTIFY-1101: 「No folder」の行（`is_no_folder`・保存した木は変えない = `workspace_ops` が今まで通り探して替える）。folder は chat bar の folder の icon が示す（無い時は空の icon）。
fn shown_root(mut n: crate::domain::ui::UiNode) -> crate::domain::ui::UiNode {
    n.children = n.children.into_iter().filter(|c| composer::project::SeatV1::asked_in(c).is_none() && c.component != "terminal" && !is_no_folder(c)).map(shown_root).collect();
    n
}

/// G99c: 「No folder」の行か（text が `composer::project::NO_FOLDER` の node）。
fn is_no_folder(n: &crate::domain::ui::UiNode) -> bool {
    n.component == "text" && n.props.get("text").and_then(Value::as_str) == Some(composer::project::NO_FOLDER)
}

/// G88a: 席（`<slug>-next` の node）の 1 問。1 Page に 1 問が複数でも席ごと（前の版の 1 席 1 Page は slug = route の名前）。
fn asked_node<'a>(page: &'a PageDefinition, seat: &str) -> Option<(&'a crate::domain::ui::UiNode, composer::project::SeatV1)> {
    page.root.nodes().into_iter().find(|n| n.key == format!("{seat}-next")).and_then(|n| composer::project::SeatV1::asked_in(n).map(|s| (n, s)))
}

/// W13b: 1 問への答えの op（`PageIntent::AnswerMissingApp` の doc）。決定的・何も書かない。席 = 1 問の node の席の slug（`asked_node`）。
/// `Some(bundle id)`（1 問の `instead` に在る時だけ）→ 1 問の node の Remove と、同じ親の同じ位置への AddSurface（id = 席・`APP_SEAT_BACKEND`・席の file）と、
/// その面の node の Update（`label` = 一覧の app の名前・composer の面と同じ）。
/// `None` → 同じ node の Update（props を「Continuing without <名前>」の 1 行に替える = 1 問も消える・PV1 で送り直しを求める文を外した）。1 問が無い・候補に無い → 理由の 1 行。
fn missing_app_ops(p: &ProjectDefinitionV1, seat: &str, use_app: Option<&str>) -> Result<Vec<ProjectPatchOp>, String> {
    use crate::domain::patch::{AppPatch, PatchOp};
    let (page_id, key, _, _, asked) = asked_place(p, seat)?;
    let name = asked.app_name.clone().unwrap_or_default();
    let Some(bundle) = use_app else {
        // PV1（PV-006）: 一言の送り直しを求めない（席だけ答えた・他の席はそのまま）
        let props = serde_json::json!({ "text": format!("Continuing without {name}"), "style": "helper" });
        return Ok(vec![ProjectPatchOp::Page { page_id, patch: AppPatch { op: PatchOp::Update, target_key: key, value: Some(serde_json::json!({ "props": props })) } }]);
    };
    let app = asked.instead.iter().find(|a| a.bundle_id == bundle).ok_or_else(|| format!("That app isn't one of the options for {name} · choose one below"))?;
    placed_ops(p, seat, &app.name, project::SurfaceTarget::App { bundle_id: app.bundle_id.clone(), file: asked.file.clone() })
}

/// W13b / PV0: 席の 1 問の node の (Page の id, node の key, 親の key, 親の中の位置, 席の写し)。1 問が無い（答えた後・Undo で消えた）→ 理由の 1 行。
fn asked_place(p: &ProjectDefinitionV1, seat: &str) -> Result<(String, String, String, usize, composer::project::SeatV1), String> {
    let found = p.pages.iter().find_map(|pg| {
        let (n, asked) = asked_node(pg, seat)?;
        let key = n.key.clone();
        let (parent, index) = pg.root.nodes().iter().find_map(|n| n.children.iter().position(|c| c.key == key).map(|i| (n.key.clone(), i)))?;
        Some((pg.id.clone(), key, parent, index, asked))
    });
    found.ok_or_else(|| "This question is already answered · carry on with this page".to_owned())
}

/// PV0: 1 問を `target`（一覧の代わりの app・入れて読み戻した app・PV1 の表の web 版）の面に替える op（1 問の Remove・同じ親の同じ位置への
/// AddSurface〈id = 席・app は `APP_SEAT_BACKEND`・URL は WebView〈Browser でない = host は URL の 1 つ〉〉・面の node の label = `name`）。1 問が無い → 理由の 1 行。
/// 契約（PV1 で `app: &KnownApp` を `name`・`target` に替えた = web 版の面も同じ op で置くため）: app の席の file は呼ぶ側が `target` に入れる。
/// Wave: PV0, PV1
fn placed_ops(p: &ProjectDefinitionV1, seat: &str, name: &str, target: project::SurfaceTarget) -> Result<Vec<ProjectPatchOp>, String> {
    use crate::domain::patch::{AppPatch, PatchOp};
    let (page_id, key, parent, index, _) = asked_place(p, seat)?;
    let backend = match target {
        project::SurfaceTarget::App { .. } => composer::project::APP_SEAT_BACKEND,
        project::SurfaceTarget::Url { .. } => project::SurfaceBackendKind::WebView,
    };
    let spec = project::SurfaceSpec { id: seat.to_owned(), backend, target, keep_alive: true, frame: None, browser: false };
    Ok(vec![
        ProjectPatchOp::Page { page_id: page_id.clone(), patch: AppPatch { op: PatchOp::Remove, target_key: key, value: None } },
        ProjectPatchOp::AddSurface { spec, page_id: page_id.clone(), parent, node_key: format!("{seat}-surface"), index: Some(index) },
        // FX3 反証の直し: 面の node に app の名前（composer の面と同じ label）= ⌘Z の 1 句（`undo_scope_line`）に bundle id の切れ端（`xos`・`slackmacgap`）を出さない
        ProjectPatchOp::Page { page_id, patch: AppPatch { op: PatchOp::Update, target_key: format!("{seat}-surface"), value: Some(serde_json::json!({ "label": name })) } },
    ])
}

/// G99c: 「No folder」の行（text が `composer::project::NO_FOLDER` の node・key `<slug>-next`）の (親の key, 親の中の位置, 席の slug)。
fn no_folder_rows(page: &PageDefinition) -> Vec<(String, usize, String)> {
    page.root
        .nodes()
        .into_iter()
        .flat_map(|parent| {
            parent
                .children
                .iter()
                .enumerate()
                .filter(|(_, n)| is_no_folder(n))
                .filter_map(|(i, n)| Some((parent.key.clone(), i, n.key.strip_suffix("-next")?.to_owned())))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// G99c: folder を当てる op（`PageIntent::SetWorkspace` の doc）。決定的・何も書かない。先頭は `SetWorkspace`、続けて「No folder」の行ごとに
/// その行の Remove と、同じ親の同じ位置への AddTerminal（id `<app>.<slug>`・cwd = dir・node `<slug>-terminal`・`compose_project` の Terminal と同じ形）。
fn workspace_ops(p: &ProjectDefinitionV1, dir: &str) -> Vec<ProjectPatchOp> {
    use crate::domain::patch::{AppPatch, PatchOp};
    let mut ops = vec![ProjectPatchOp::SetWorkspace { dir: dir.to_owned() }];
    for pg in &p.pages {
        let app = pg.app_ref.clone().or_else(|| p.app_refs.first().cloned()).unwrap_or_else(|| p.id.clone());
        for (parent, index, slug) in no_folder_rows(pg) {
            ops.push(ProjectPatchOp::Page { page_id: pg.id.clone(), patch: AppPatch { op: PatchOp::Remove, target_key: format!("{slug}-next"), value: None } });
            let spec = project::TerminalSessionSpec {
                id: format!("{app}.{slug}"),
                project_id: p.id.clone(),
                shell: "/bin/zsh".into(),
                working_directory: dir.to_owned(),
                keep_alive_on_page_switch: true, command: None,
            };
            ops.push(ProjectPatchOp::AddTerminal { spec, page_id: pg.id.clone(), parent, node_key: format!("{slug}-terminal"), index: Some(index) });
        }
    }
    ops
}

/// G88b: take を入れる口（`audio::reaper::insert` と同じ形・試験は偽を差す）。
pub type TakeInsert = fn(&std::path::Path, std::time::Duration) -> Result<crate::audio::reaper::Receipt, String>;

/// G88b: take を入れられる app の表（bundle id・帯に出す名前・入れる口）。app の名前を code に書くのはここ 1 か所。
/// ENGINE-GENERIC-1101（owner 211「appに合わせて機能を追加するのではなく」）: app ごとの入れる口を製品に持つ道は取り下げ。録画（11/02）が使うので
/// 印を付けて残し、録画の後にこの表・`add_take`・`TakeHost::feeds`・`renderer::AddTake`・`TakeAct::Add`・`audio::reaper::insert` を消す（録った事実は
/// もう `Generated::part_event` の状況に渡る・REAPER に入れるのは利用者の一言から Engine が組む手か recipe の data に）。
pub const TAKE_APPS: [(&str, &str, TakeInsert); 1] = [("com.cockos.reaper", "REAPER", crate::audio::reaper::insert)];

/// G88b: 入れた結果の帯の 1 行。入った → `✓ Added to {app} · Undo in {app} (⌘Z)`／同じ take が在った → `Already in {app}`／
/// 入ったか分からない（timeout）・読み戻しが合わない → 理由の 1 行（自動で送り直さない）／Err → その 1 行。
/// Wave: M2
pub fn receipt_line(app: &str, r: &Result<crate::audio::reaper::Receipt, String>) -> String {
    use crate::audio::reaper::Receipt;
    match r {
        Ok(Receipt::Verified(_)) => format!("✓ Added to {app} · Undo in {app} (⌘Z)"),
        Ok(Receipt::Duplicate(_)) => format!("Already in {app}"),
        Ok(Receipt::Unverified { reason, .. }) => format!("Added to {app}, but it doesn't match the take · {reason}"),
        Ok(Receipt::EffectUnknown { reason }) => format!("Can't tell if it reached {app} · {reason} · Look in {app} before adding again"),
        Err(e) => e.clone(),
    }
}

/// G88b: 帯の `Add take to {app}` の形。`feeds` の面の app が `TAKE_APPS` に無い → None（`Reveal in Finder` だけ）。
/// 在って面が Ready でない → `blocked` に次の一手の 1 行（`Open {app} first`）。Ready → 押せる。
/// Wave: M2
/// ENGINE-GENERIC-1101: AI-COMPOSE-1101 の「入れる口の無い app にも出す」（`by_ai`）は取り下げ（録った後に何をするかは `Generated::part_event` の
/// きっかけ = 利用者の一言）。録画（11/02）の REAPER の道のためだけに残し、録画の後に `TAKE_APPS` と一緒に消す。
pub fn add_take(feeds: Option<&project::SurfaceSpec>, ready: bool) -> Option<renderer::AddTake> {
    let project::SurfaceTarget::App { bundle_id, .. } = &feeds?.target else { return None };
    let (_, app, _) = TAKE_APPS.iter().find(|(b, _, _)| b == bundle_id)?;
    Some(renderer::AddTake { app: (*app).into(), blocked: (!ready).then(|| format!("Open {app} first")) })
}

/// W14b: その席の 1 行を出す Page の id（Terminal = terminal node を置いた Page・面 = source_surface を置いた Page・生成 app = その app の先頭の Page。
/// どこにも置いていなければ先頭の Page）。
fn usable_page(p: &ProjectDefinitionV1, seat: &Seat) -> String {
    let placed = |pg: &&PageDefinition| match seat.kind {
        SeatKind::Generated => pg.app_ref.as_deref() == Some(seat.id.as_str()),
        SeatKind::Terminal => pg.root.nodes().iter().any(|n| n.props.get("terminal_ref").and_then(Value::as_str) == Some(seat.id.as_str())),
        // W13b: 1 問を置いた Page（G88a: 席の node `<id>-next` の在る Page）
        SeatKind::MissingApp => asked_node(pg, &seat.id).is_some(),
        _ => pg.root.nodes().iter().any(|n| n.props.get("surface_ref").and_then(Value::as_str) == Some(seat.id.as_str())),
    };
    p.pages.iter().find(placed).or(p.pages.first()).map(|pg| pg.id.clone()).unwrap_or_default()
}

/// W14b: 1 行の状態（None = 使える）。最初に止まった問いの語で決める。
/// UI-6a（sendback 31 / 56）: `▣ Needs approval`（Blocked）は同じ行に押せる答え（W13b の 1 問 = `asked`）が在る時だけ。button の無い入れる・許可は
/// Disconnected（届かない理由と次の一手の文）・承認（評価は外に効く手を走らせない）は None = 画面に何も描かない（NOTIFY-1101: 使う時に承認の card が
/// chat に出る・`Needs approval: “Edit …” is a real change…` はいつも同じ事しか言わない説明の文。AX の usable の group は残る）。
fn usable_state(u: &Usable, asked: bool) -> Option<RegionState> {
    if u.ready() {
        return None;
    }
    [&u.reach, &u.first_action, &u.readback].into_iter().find_map(|c| match c {
        Check::Wait(_) => Some(RegionState::Loading),
        Check::Fail { need: Need::Install | Need::Permission, .. } if asked => Some(RegionState::Blocked),
        // 接続が必要と言うのは繋ぐ・開き直すで届く時と、入れる・許可を外で済ませる時（押す物はこの行に無い）
        Check::Fail { need: Need::Connect | Need::Install | Need::Permission, .. } => Some(RegionState::Disconnected),
        Check::Fail { need: Need::Retry, .. } => Some(RegionState::Error),
        _ => None,
    })
}

/// W14b: 生成 app の 3 問を本物の runtime で測る（別の session `usable.<app>` ─ 利用者の session `main.<app>`・Runs の一覧・timeline に書かない）。
/// ① 画面の外の egui で描き、主役の node（role primary）が描かれたかと、入力欄を押した後に AX の focus がその欄に来たかを読む
/// ② `LocalStep` の action を `presses` 回押す（入力は schema の最小値・文字の欄の 1 つに印）③ store の ui_state（session `usable.<app>`・
/// source = action の id の最後の段を除いた物）を 1 手とは別に読む ── 1 手の返り（Receipt）は読み戻しに使わない。Drop で評価の session の中身を消す。
/// 例: `let mut probe = AppProbe::new(rt, store, rev); let u = verifier::usable(&verifier::app_seat(rev.definition()), &mut probe, &mark, &mut 1);`
/// Wave: W14b
pub struct AppProbe {
    rt: Arc<Runtime>,
    store: Arc<Mutex<Store>>,
    rev: AppRevision,
    eval: Option<AppRuntime>,
    /// 1 手が書いた source（Drop で消す）
    touched: Vec<String>,
}

impl AppProbe {
    pub fn new(rt: Arc<Runtime>, store: Arc<Mutex<Store>>, rev: AppRevision) -> Self {
        AppProbe { rt, store, rev, eval: None, touched: Vec::new() }
    }

    /// 評価の session の名前（利用者の `main.<app>` と別）。
    pub fn session(&self) -> String {
        format!("usable.{}", self.rev.definition().id)
    }
}

impl Drop for AppProbe {
    fn drop(&mut self) {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        // 空の object（runtime の local action は object でない中身を読めない＝次の run の 1 手が落ちる）
        for source in &self.touched {
            let _ = store.put_ui_state(&self.session(), source, &serde_json::json!({}));
        }
    }
}

/// 文字の欄（enum・format の無い string）の 1 つに印を入れる（required を先に）。
fn mark_input(input: &mut Value, schema: &serde_json::Map<String, Value>, mark: &str) {
    let props = schema.get("properties").and_then(Value::as_object);
    let text = |name: &str| props.and_then(|p| p.get(name)).is_some_and(|p| p.get("type").and_then(Value::as_str).unwrap_or("string") == "string" && p.get("enum").is_none() && p.get("format").is_none());
    let required: Vec<&str> = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let name = required.iter().copied().find(|n| text(n)).or_else(|| props.and_then(|p| p.keys().map(String::as_str).find(|n| text(n))));
    if let (Some(n), Some(obj)) = (name, input.as_object_mut()) {
        obj.insert(n.into(), Value::String(mark.into()));
    }
}

/// ① の中身: 画面の外の egui で app を描き（最大 6 frame）、主役の node の矩形と、最初の入力欄を押した後の AX の focus を読む。
fn draw_reach(def: &AppDefinitionV3, rt: &AppRuntime, name: &str) -> Check {
    use egui::accesskit::Role;
    let primary = def.composition.root.nodes().into_iter().find(|n| n.role.as_deref() == Some("primary")).map(|n| n.key.clone()).unwrap_or_else(|| def.composition.root.key.clone());
    let ctx = egui::Context::default();
    theme::install(&ctx, egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let (mut state, mut regions, mut line) = (AppState::default(), BTreeMap::new(), None);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
    let (mut events, mut drawn, mut target, mut released) = (Vec::new(), false, None, false);
    for _ in 0..6 {
        let input = egui::RawInput { screen_rect: Some(screen), events: std::mem::take(&mut events), ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut host = Host { rt, regions: &mut regions, line: &mut line, receipts: Vec::new(), term: None, portals: BTreeMap::new(), placed: Vec::new(), surfaces: Vec::new(), project: String::new(), web: None, takes: None, usage: None };
                renderer::render(ui, def, &mut state, &mut host);
            });
        });
        drawn |= state.rects.contains_key(&primary);
        let Some(update) = out.platform_output.accesskit_update else { continue };
        match target {
            Some((t, _)) if released && update.focus == t => return Check::Pass("The main part was drawn and the input field has focus".into()),
            Some((_, at)) if !released => {
                released = true;
                events = vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }];
            }
            Some(_) => {}
            None => match update.nodes.iter().find(|(_, n)| matches!(n.role(), Role::TextInput | Role::MultilineTextInput)) {
                Some((id, n)) => {
                    let Some(b) = n.bounds() else { continue };
                    let at = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
                    target = Some((*id, at));
                    events = vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }];
                }
                None if drawn => return Check::Pass("The main part was drawn (a screen with no input field)".into()),
                None => {}
            },
        }
    }
    match (drawn, target) {
        (false, _) => Check::Fail { need: Need::Retry, line: format!("The main part of {name} isn't drawn. Build it again") },
        (true, Some(_)) => Check::Fail { need: Need::Retry, line: format!("The input in {name} doesn't take focus when pressed. Build it again") },
        (true, None) => Check::Pass("The main part was drawn (a screen with no input field)".into()),
    }
}

impl Probe for AppProbe {
    fn reach(&mut self, seat: &Seat) -> Check {
        let eval = match self.rt.open(self.rev.clone(), &self.session()) {
            Ok(r) => r,
            Err(e) => return Check::Fail { need: Need::Retry, line: format!("Couldn't open the screen that checks {} ({}). Reopen it", seat.name, wording(e)) },
        };
        let c = draw_reach(self.rev.definition(), &eval, &seat.name);
        self.eval = Some(eval);
        c
    }

    fn act(&mut self, seat: &Seat, step: &LocalStep) -> Check {
        let what = seat.first.as_ref().map_or("1 step", |f| f.what.as_str());
        let def = self.rev.definition();
        let (Some(eval), Some(a)) = (self.eval.as_ref(), step.action().and_then(|id| def.action(id))) else {
            return Check::Fail { need: Need::Retry, line: format!("{} has no action for “{what}”. Build it again", seat.name) };
        };
        let mut input = verifier::sample_input(&a.input_schema);
        mark_input(&mut input, &a.input_schema, step.mark());
        let origin = def.composition.root.nodes().into_iter().find(|n| n.action_ref.as_deref() == Some(a.id.as_str())).map_or(InvocationOrigin::CommandPalette, |n| InvocationOrigin::Button { node_key: n.key.clone() });
        if let Some((source, _)) = a.id.rsplit_once('.') {
            self.touched.push(source.to_owned());
        }
        for i in 0..seat.first.as_ref().map_or(1, |f| f.presses.max(1)) {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(30)); // start と stop の間に経過を残す
            }
            // RQ4（RQ-019 M26）: Connections で切った接続を使う手は「切った・Connections で戻す」の 1 行（Project は開けたまま）
            let off = || [a.capability_ref.as_deref(), Some(a.id.as_str())].into_iter().flatten().find_map(disconnected_line);
            match eval.invoke(&a.id, input.clone(), origin.clone()) {
                Ok(Invoked::Done(r)) => match r.outcome {
                    Outcome::Failed { error } => {
                        return match off().filter(|_| matches!(error, ErrorCode::NotReady)) {
                            Some(line) => Check::Fail { need: Need::Connect, line },
                            None => Check::Fail { need: Need::Retry, line: format!("“{what}” didn't go through ({}). Build it again", wording(error)) },
                        }
                    }
                    Outcome::Rejected { reason } => return Check::Fail { need: Need::Retry, line: format!("“{what}” was refused ({reason}). Build it again") },
                    _ => {}
                },
                Ok(Invoked::NeedsApproval(_)) => return Check::Fail { need: Need::Approve, line: format!("“{what}” needs approval, and the check never approves") },
                Err(ErrorCode::NotReady) => return Check::Fail { need: Need::Connect, line: off().unwrap_or_else(|| format!("“{what}” needs a connection · Connect")) },
                Err(e) => return Check::Fail { need: Need::Retry, line: format!("Can't press “{what}” ({}). Build it again", wording(e)) },
            }
        }
        Check::Pass(format!("Pressed “{what}”"))
    }

    fn read_back(&mut self, _seat: &Seat, step: &LocalStep) -> Readback {
        let Some((source, _)) = step.action().and_then(|a| a.rsplit_once('.')) else { return Readback::Missing("This step has no actions".into()) };
        match self.store.lock().unwrap_or_else(|e| e.into_inner()).get_ui_state(&self.session(), source) {
            Ok(Some(v)) => Readback::Read { via: Outside::LocalDb, value: v.to_string() },
            Ok(None) => Readback::Missing("Nothing here yet".into()),
            Err(e) => Readback::Missing(format!("Couldn't read the database ({e})")),
        }
    }
}

/// W14b: Terminal の ① と ② を待つ長さ（利用者の rc を読む shell の起動を含む）。
const USABLE_TERMINAL_WAIT: Duration = Duration::from_secs(20);

/// W14b: Project の Terminal の 3 問（別の `TerminalManager` の PTY ─ 利用者の Terminal・`terminal_sessions` の記録・RunReceipt に触れない）。
/// ① spec と同じ shell と作業 folder で起こし、`run_marked("true")` の印が返る（prompt が返った）② 「1 行打つ」=
/// `printf '%s\n' <印> > <temp の dir の file>`（repo に書かない）③ その file を disk から読む。Drop で PTY を全部閉じ、temp の dir を消す。
/// 例: `let mut probe = TerminalProbe::new(mgr.config().clone(), p.terminal_specs.clone());`
/// Wave: W14b
pub struct TerminalProbe {
    mgr: TerminalManager,
    specs: Vec<project::TerminalSessionSpec>,
    dir: std::path::PathBuf,
    /// 席の id → この probe が起こした session
    keys: BTreeMap<String, SessionKey>,
}

impl TerminalProbe {
    pub fn new(config: TerminalConfig, specs: Vec<project::TerminalSessionSpec>) -> Self {
        let dir = std::env::temp_dir().join(format!("shibubu-usable-{}", storage::new_id()));
        TerminalProbe { mgr: TerminalManager::new(config), specs, dir, keys: BTreeMap::new() }
    }

    /// ② が書き ③ が読む file（席ごと）。
    pub fn file(&self, seat: &Seat) -> std::path::PathBuf {
        self.dir.join(format!("{}.txt", seat.id.replace(['/', '\\'], "_")))
    }

    /// この probe が起こした session（試験が pid と記録を照らす）。
    pub fn sessions(&self) -> Vec<SessionInfo> {
        self.mgr.list(None)
    }
}

impl Drop for TerminalProbe {
    fn drop(&mut self) {
        self.mgr.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Probe for TerminalProbe {
    fn reach(&mut self, seat: &Seat) -> Check {
        let Some(spec) = self.specs.iter().find(|t| t.id == seat.id) else {
            return Check::Fail { need: Need::Retry, line: "This terminal isn't in the project. Build it again".into() };
        };
        // 測り直しは前の shell を閉じて新しく起こす
        if let Some(key) = self.keys.remove(&seat.id) {
            let _ = self.mgr.close(&key);
        }
        let eval = project::TerminalSessionSpec { id: format!("usable.{}.{}", spec.id, storage::new_id()), ..spec.clone() };
        let info = match self.mgr.spawn(&eval, TermSize::default()) {
            Ok(i) => i,
            Err(e) => return Check::Fail { need: Need::Retry, line: format!("Couldn't start the terminal ({e}). Check the working folder, then reopen") },
        };
        self.keys.insert(seat.id.clone(), info.key.clone());
        match self.mgr.run_marked(&info.key, "true", USABLE_TERMINAL_WAIT) {
            Ok(_) => Check::Pass("The terminal started and showed a prompt".into()),
            Err(_) => Check::Fail { need: Need::Retry, line: format!("The terminal didn't show a prompt in {} s. Reopen to check again", USABLE_TERMINAL_WAIT.as_secs()) },
        }
    }

    fn act(&mut self, seat: &Seat, step: &LocalStep) -> Check {
        let Some(key) = self.keys.get(&seat.id) else { return Check::Fail { need: Need::Retry, line: "The terminal isn't running. Reopen it".into() } };
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return Check::Fail { need: Need::Retry, line: format!("Couldn't create the folder for the command ({})", e.kind()) };
        }
        let cmd = format!("printf '%s\\n' '{}' > '{}'", step.mark(), self.file(seat).display());
        match self.mgr.run_marked(key, &cmd, USABLE_TERMINAL_WAIT) {
            Ok(o) if o.exit_code == 0 => Check::Pass("Typed one line".into()),
            Ok(o) => Check::Fail { need: Need::Retry, line: format!("The command ended with exit code {}. Reopen to check again", o.exit_code) },
            Err(_) => Check::Fail { need: Need::Retry, line: format!("The command gave no result in {} s. Reopen to check again", USABLE_TERMINAL_WAIT.as_secs()) },
        }
    }

    fn read_back(&mut self, seat: &Seat, _step: &LocalStep) -> Readback {
        match std::fs::read_to_string(self.file(seat)) {
            Ok(s) => Readback::Read { via: Outside::Disk, value: s },
            Err(e) => Readback::Missing(format!("Couldn't read the file ({})", e.kind())),
        }
    }
}

/// RQ4（RQ-019 M26）: Connections で切った接続の (capability の頭・席に言う名前)。app.rs が切る・戻す・server を止める度に入れる
/// （`github.` → GitHub・`web.` → Web data・`mcp.<server>.` → server の名前）。generated.rs は mcp / github を知らない（tests/yc_demo_e2e.rs が単独で取り込む）。
static OFF_CAPABILITIES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// RQ4: `OFF_CAPABILITIES` を置き換える（app.rs の `sync_connections`）。
pub fn set_disconnected_capabilities(off: Vec<(String, String)>) {
    *OFF_CAPABILITIES.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = off;
}

/// 手の capability が切った接続の物なら席の 1 行 `{name} is disconnected · Reconnect in Connections`。他は None（今の 1 行のまま）。
fn disconnected_line(capability: &str) -> Option<String> {
    let off = OFF_CAPABILITIES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    off.iter().find(|(prefix, _)| capability.starts_with(prefix.as_str())).map(|(_, name)| format!("{name} is disconnected · Reconnect in Connections"))
}

/// W14b: 描く時に分かる面（本物の app の面・Mirror・Web）の ①。② はいつも外への書き込み（`project_seats`）なので `LocalStep` が作れず呼ばれない。
struct SurfaceProbe<'a> {
    receipts: &'a [SurfaceReceipt],
    web: &'a crate::project::web::PageWebViews,
    project: &'a str,
    #[cfg(feature = "mirror")]
    mirror: Option<&'a crate::ui::mirror_view::MirrorPages>,
}

impl SurfaceProbe<'_> {
    /// app の面の ① の前半: receipt（Project を作った時の find_app と permission）から。Ok = 入っていて許可も在る。
    fn installed(&self, seat: &Seat) -> Result<(), Check> {
        use project::{ReceiptState as S, NOT_INSTALLED, NOT_TRUSTED};
        let name = &seat.name;
        let Some(r) = self.receipts.iter().find(|r| r.surface_id == seat.id) else {
            return Err(Check::Fail { need: Need::Connect, line: format!("{name} hasn't been checked · Check again") });
        };
        match r.state {
            S::Verified => Ok(()),
            S::Unverified if r.oracle == NOT_TRUSTED => Err(Check::Fail { need: Need::Permission, line: format!("To use {name} from a page, allow Shibubu in System Settings › Privacy & Security › Accessibility") }),
            S::Unverified => Ok(()),
            S::Unavailable if r.oracle == NOT_INSTALLED => Err(Check::Fail { need: Need::Install, line: format!("{name} isn't installed on this Mac. Install it, then send your request again") }),
            S::Unavailable => Err(Check::Fail { need: Need::Connect, line: format!("Couldn't find {name} · Check again") }),
            S::Ended => Err(Check::Fail { need: Need::Connect, line: format!("{name} isn't running · Open {name}") }),
        }
    }
}

impl Probe for SurfaceProbe<'_> {
    fn reach(&mut self, seat: &Seat) -> Check {
        let name = &seat.name;
        match seat.kind {
            SeatKind::Web if !crate::surface::WEBVIEW_BUILT => Check::Fail { need: Need::Install, line: format!("This version can't show websites here · Open in browser") },
            SeatKind::Web => match self.web.state(&crate::project::web::key(self.project, &seat.id)) {
                None => Check::Wait("Opening this page loads it".into()),
                Some(crate::surface::WebState::Starting | crate::surface::WebState::Loading) => Check::Wait(format!("Loading {name}")),
                Some(crate::surface::WebState::Live) => Check::Pass(format!("{name} finished loading")),
                Some(crate::surface::WebState::Recoverable(why)) => Check::Fail { need: Need::Connect, line: why },
            },
            SeatKind::NativeApp => match self.installed(seat) {
                Err(c) => c,
                Ok(()) if cfg!(feature = "native_direct") => Check::Pass(format!("{name} is installed and Shibubu has permission to control it")),
                Ok(()) => Check::Fail { need: Need::Install, line: format!("Open in {name}") },
            },
            SeatKind::Mirror => match self.installed(seat) {
                Err(c) => c,
                Ok(()) => self.mirror_reach(seat),
            },
            SeatKind::Terminal | SeatKind::Generated => Check::Fail { need: Need::Retry, line: format!("This check can't measure {name}") },
            // W13b: 測らない・いつも Fail{Install}（SeatKind::MissingApp の doc）。答えの button は draw_usable がこの 1 行の下に置く
            SeatKind::MissingApp => Check::Fail { need: Need::Install, line: format!("{name} isn't on this Mac · choose one below") },
        }
    }

    fn act(&mut self, seat: &Seat, _step: &LocalStep) -> Check {
        Check::Fail { need: Need::Approve, line: format!("The check never calls actions that change {}", seat.name) }
    }

    fn read_back(&mut self, seat: &Seat, _step: &LocalStep) -> Readback {
        Readback::Missing(format!("{} isn't run during evaluation", seat.name))
    }
}

impl SurfaceProbe<'_> {
    /// Mirror の ① の後半: 元の窓の映像が 1 枚来たか（feature `mirror` の無い build は映す部品が無い）。
    fn mirror_reach(&self, seat: &Seat) -> Check {
        let name = &seat.name;
        #[cfg(feature = "mirror")]
        {
            use crate::surface::PortalState as P;
            return match self.mirror.and_then(|m| m.reach(&seat.id)) {
                None => Check::Wait("Opening this page mirrors it".into()),
                Some(Ok(())) => Check::Pass(format!("The image of {name}'s window arrived")),
                Some(Err(P::Attaching)) => Check::Wait(format!("Waiting for the {name} window")),
                Some(Err(P::PermissionRequired)) => Check::Fail { need: Need::Permission, line: format!("To mirror the {name} window, allow it on this page or in System Settings › Privacy & Security › Screen Recording") },
                // G87: 次の一手は面の中の button（Open {App}・Try again）。この 1 行は AX の usable の group だけ（draw_usable が画面に描かない）
                Some(Err(P::SourceUnavailable)) => Check::Fail { need: Need::Connect, line: format!("Couldn't find {name}'s window") },
                // G87: Shibubu が起こそうとした app が入っていない（MirrorError::NotInstalled）
                Some(Err(P::Undiscovered)) => Check::Fail { need: Need::Install, line: format!("{name} isn't installed on this Mac") },
                Some(Err(_)) => Check::Fail { need: Need::Retry, line: format!("The part that mirrors the {name} window isn't responding. Reopen it") },
            };
        }
        #[allow(unreachable_code)]
        Check::Fail { need: Need::Install, line: format!("This version can't mirror apps · Open in {name}") }
    }
}

fn wording(e: ErrorCode) -> &'static str {
    match e {
        ErrorCode::NotReady => "Not set up yet",
        ErrorCode::PermissionDenied => "Not permitted",
        ErrorCode::ApprovalDenied => "Not approved",
        ErrorCode::InvalidInput => "The input has the wrong format",
        ErrorCode::ConnectionLost => "The connection dropped",
        ErrorCode::Timeout => "Timed out",
        ErrorCode::RateLimited => "The same action is already running",
        ErrorCode::TargetNotFound => "Not found",
        ErrorCode::TargetAmbiguous => "Couldn't tell which one you mean",
        ErrorCode::DriverRefused => "The other side refused",
        ErrorCode::VerificationFailed => "Not verified",
        ErrorCode::Cancelled => "Undone",
        ErrorCode::Internal => "Internal error",
    }
}

/// draw_page の「読めなければ既定の値で描き、理由の 1 行を別に出す」（Undo / Redo の行き先・引き出しの開閉・面の receipt）。
fn or_line<T: Default>(r: Result<T, String>) -> (T, Option<String>) {
    match r {
        Ok(v) => (v, None),
        Err(e) => (T::default(), Some(e)),
    }
}

/// UI-5a: Page の帯の 2 つの menu の項目（`＋` と `•••`）。純関数（DB を読まない・描かない）で、`draw_page` は並びを回して `theme::button_menu_item` を描くだけ。
/// 項目の型は ⌘K と同じ `ui::palette::Command`（label = 見える語と AX の名前・hint = 右端の鍵・空なら描かない・intent = 押した時の Intent）。新しい型は作らない。
/// 呼ぶ例: `let (add, options) = page_menus(&p, &page.id, &ur, placement.as_ref().map_or("None", |t| t.name.as_str()));`
/// 保証する: 返り値の 1 つ目 = 部品を足す menu（下の MIRROR-FRAME-1101 の契約）。
/// 契約（MIRROR-FRAME-1101・owner 253 = 案 3・署名は変えない）: 1 つ目は部品を足す menu（menu の fn は 1 つ = Page の右下の ＋〈AX `Add to page`・pointer が Page の上の時だけ見え、
/// 見えない間も AX の木と Tab の順に居る〉と Page の何も無い所の右 click〈押した所に開く〉が同じ並びを開く）= `Add mirror frame`（`AddMirrorFrame { page }`・feature `mirror` の
/// build だけ）・`Add browser`（`AddBrowser { page }`）・`Add terminal`（`TerminalPanel(PanelAct::Add)`）。帯の tab の右の ＋ は menu を開かず `PageIntent::Add`（AX `Add page · ⌘T`）。
/// 2 つ目 = `•••` の上から: `Move left`（先頭の Page では入れない）・`Move right`（末尾では入れない）・`Layout: {layout}`（`Placement`）・
/// `Show terminal`（hint 無し・⌃` は G95 から下の panel の開閉・`ShowTerminal { page: page_id }`・G92）・`Stop mirroring`（`StopMirroring { page: page_id }`・
/// この Page に Mirror の面が在る時だけ・全部止まっていれば描く側が外す・MIRROR-AUTO-1101）・`Refresh`（`Reread`・`readable_resources` が在る時だけ）・`Redo`（hint `⇧⌘Z`・`ur.redo` が在る時だけ）・
/// `Delete page…`（`Delete { page: page_id }`・最後）。当てはまらない項目は並びに入れない（押しても何も起きない物を見せない・L2）。
/// Page が 1 枚の時は `Delete page…` の label を `A project needs at least one page` に替えて残す（押せない形と danger の字は描く側が intent と Page の数で決める・`Command` に field を足さない）。
/// 保証しない: `Delete page…` を押した時に消す事（描く側が `pending_delete` を立て、tab の横の確認の `Delete` だけが `PageIntent::Delete` を返す・答え ③）・
/// `Layout` の 4 行・tab の右 click の並び。
/// 契約（FX3・署名は変えない）: 2 つ目の先頭に `Rename…`（hint 空・`Rename { page: page_id, title: String::new() }` = 欄を開く印・描く側が横取りして
/// `pending_rename` を立てる・1 回目で名前を替えない）。⌘K の行は `Generated::page_commands`・溢れた Page は帯の横 scroll（TITLEBAR-ICONS-1101）。
/// Wave: UI-5a, FX3
pub fn page_menus(p: &ProjectDefinitionV1, page_id: &str, ur: &UndoRedo, layout: &str) -> (Vec<crate::ui::palette::Command>, Vec<crate::ui::palette::Command>) {
    use crate::ui::palette::Command;
    let item = |label: &str, hint: &str, intent: PageIntent| Command { label: label.to_owned(), hint: hint.to_owned(), intent: Intent::Page(intent) };
    let page = page_id.to_owned();
    // MIRROR-FRAME-1101（owner 253 = 案 3）: 部品を足す menu（Page の右下の ＋ と右 click）。Page を足すのは帯の tab の ＋ と ⌘T
    let mut add = Vec::new();
    if cfg!(feature = "mirror") {
        add.push(item("Add mirror frame", "", PageIntent::AddMirrorFrame { page: page.clone() }));
    }
    add.push(item("Add browser", "", PageIntent::AddBrowser { page: page.clone() }));
    // G95: 利用者が足す Terminal は下の panel の tab（Page の面に置くのは AI の plan の `PageIntent::AddTerminal` だけ）
    add.push(item("Add terminal", "", PageIntent::TerminalPanel(PanelAct::Add)));
    let at = p.pages.iter().position(|pg| pg.id == page_id);
    // FX3（FX-009）: 先頭に `Rename…`（title が空 = 欄を開く印・draw_page が横取りする）
    let mut options = vec![item("Rename…", "", PageIntent::Rename { page: page.clone(), title: String::new() })];
    if let Some(at) = at.filter(|&at| at > 0) {
        options.push(item("Move left", "", PageIntent::Move { page: page.clone(), index: at - 1 }));
    }
    if let Some(at) = at.filter(|&at| at + 1 < p.pages.len()) {
        options.push(item("Move right", "", PageIntent::Move { page: page.clone(), index: at + 1 }));
    }
    options.push(item(&format!("Layout: {layout}"), "", PageIntent::Placement));
    options.push(item("Show terminal", "", PageIntent::ShowTerminal { page: page.clone() }));
    if p.page(page_id).is_some_and(|pg| !mirror_surfaces_on(p, pg).is_empty()) {
        options.push(item("Stop mirroring", "", PageIntent::StopMirroring { page: page.clone() }));
    }
    if readable_resources(p).next().is_some() {
        options.push(item("Refresh", "", PageIntent::Reread));
    }
    if ur.redo.is_some() {
        options.push(item("Redo", "⇧⌘Z", PageIntent::Redo));
    }
    let delete = if p.pages.len() > 1 { "Delete page…" } else { "A project needs at least one page" };
    options.push(item(delete, "", PageIntent::Delete { page }));
    (add, options)
}

/// MIRROR-AUTO-1101 B: この Page に置いた Mirror の面の surface id（`source_surface` の `surface_ref` が指す backend Mirror の面・並びは `surface_specs` の順）。純関数。
fn mirror_surfaces_on(p: &ProjectDefinitionV1, page: &PageDefinition) -> Vec<String> {
    let refs: Vec<&str> = page.root.nodes().into_iter().filter(|n| n.component == "source_surface").filter_map(|n| n.props.get("surface_ref")?.as_str()).collect();
    // MIRROR-FRAME-1101: 中身の無い枠は映していない（`Stop mirroring` を出さない）
    p.surface_specs.iter().filter(|s| s.backend == project::SurfaceBackendKind::Mirror && !s.blank_frame() && refs.contains(&s.id.as_str())).map(|s| s.id.clone()).collect()
}

/// MIRROR-FRAME-1101（owner 247）: Page の ＋ の `Add mirror frame` が置く空の枠の大きさ（題の帯を含む point・x・y = 並びの所からのずれ 0 = 246 の `SurfaceSpec::frame`）。
const BLANK_FRAME: project::SurfaceFrame = project::SurfaceFrame { x: 0.0, y: 0.0, w: 640.0, h: 400.0 };

/// SESSIONS-LEFT-1101（owner 210 の 4）: 押して作ったまだ一言の無い session の左の行と Chat の帯の題
const NEW_SESSION_TITLE: &str = "New session";

/// SESSIONS-LEFT-1101: 左の会話の行の題 = 一言の最初の行の最初の 1 文（`.` `?` `!` `。` `？` `！` まで・無ければ行のまま）。幅で切るのは描く側。
fn chat_title(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default();
    let end = line.char_indices().find(|(_, c)| matches!(c, '.' | '?' | '!' | '。' | '？' | '！')).map_or(line.len(), |(i, c)| i + c.len_utf8());
    line[..end].trim().to_owned()
}

/// FX3（FX-013）: 変更を読んでいる間に来た 2 つ目の変更の 1 行（表 copy.en.md の「a page edit while a change is read」・Engine が読んでいる間の欄の拒否と同じ文）。
const ONE_CHANGE: &str = "One change at a time. The last one is still being read.";

/// FX3（FX-011・FX-012・FX-015・FX-057）: 帯の手が通った後の Page の上の 1 行（`page_line` の `RegionState::Ready`・表 copy.en.md の「Receipts and change results」）。
/// `before` = 当てる前の Project・`after` = 当てて DB から読み戻した Project・`intent` = 通った手。純関数。
/// 語: `Add` → `Added page “{Page}” · ⌘Z to undo`／`Move` → `Moved “{Page}” left` か `Moved “{Page}” right`／`Rename` → `Renamed “{old}” to “{new}”`／
/// `Delete` → `Deleted page “{Page}” · ⌘Z to undo`／`AddBrowser` → `Added a browser to “{Page}”`／`AddMirrorFrame` → `Added a mirror frame to “{Page}” · ⌘Z to undo`（MIRROR-FRAME-1101）／
/// `Undo` → `Undid: {action} · {undo_scope_line(after)} · ⇧⌘Z to redo`／`Redo` → `Redid: {action}`。`{action}` は Page の並びの差から読む前向きの 1 句
/// （Undo は `after → before`・Redo は `before → after` の向き = `Added page “X”`・`Deleted page “X”`・`Renamed “a” to “b”`・`Moved “X” left / right`）・差が Page の中だけなら `the last change`。
/// それ以外の手（`Back`・`Edit`・`Placement`・`Reread`・`ShowTerminal`・`RunTests`・面の手・`TerminalPanel`）と、何も変わらなかった時は None（今の 1 行のまま）。
/// 例: `assert_eq!(change_line(&p0, &p1, &PageIntent::Add), Some("Added page “Page 2” · ⌘Z to undo".into()))`
/// 保証する: 内部の語（revision・surface・id）を入れない・Page の名前は利用者が見ている名前（`title`）。保証しない: 元の app の中の変化を言う事（Shibubu は見ていない）。
/// Wave: FX3
pub fn change_line(before: &ProjectDefinitionV1, after: &ProjectDefinitionV1, intent: &PageIntent) -> Option<String> {
    if before.revision == after.revision {
        return None;
    }
    let action = |from, to| band_diff(from, to).map_or_else(|| "the last change".to_owned(), |(s, _)| s);
    match intent {
        PageIntent::Add | PageIntent::Delete { .. } => band_diff(before, after).map(|(s, _)| format!("{s} · ⌘Z to undo")),
        PageIntent::Rename { .. } => band_diff(before, after).map(|(s, _)| s),
        // 押した Page の向き（隣どうしの入れ替えでも押した側の名前で言う）
        PageIntent::Move { page, .. } => {
            let at = |q: &ProjectDefinitionV1| q.pages.iter().position(|pg| pg.id == *page);
            let (from, to, title) = (at(before)?, at(after)?, &after.page(page)?.title);
            (from != to).then(|| format!("Moved “{title}” {}", if to < from { "left" } else { "right" }))
        }
        PageIntent::AddBrowser { page } => after.page(page).map(|pg| format!("Added a browser to “{}”", pg.title)),
        PageIntent::AddMirrorFrame { page } => after.page(page).map(|pg| format!("Added a mirror frame to “{}” · ⌘Z to undo", pg.title)),
        PageIntent::Undo => Some(format!("Undid: {} · {} · ⇧⌘Z to redo", action(after, before), undo_scope_line(after))),
        PageIntent::Redo => Some(format!("Redid: {}", action(before, after))),
        _ => None,
    }
}

/// FX3: Page の並びの差（`from` → `to`）を前向きの 1 句と、その手で変わった Page の id（`to` に在る時だけ・tint の先）にする。純関数。
/// 足した → `Added page “X”`・消した → `Deleted page “X”`（id なし）・名前 → `Renamed “a” to “b”`・並び → `Moved “X” left / right`・Page の中だけの差は None。
// ponytail: 1 回の手 = 1 つの差の前提。隣どうしの入れ替えはどちらが動いたか差から決まらないので、前の位置が後ろの側（左へ動いた側）の名前で言う
fn band_diff(from: &ProjectDefinitionV1, to: &ProjectDefinitionV1) -> Option<(String, Option<String>)> {
    if let Some(pg) = to.pages.iter().find(|pg| from.page(&pg.id).is_none()) {
        return Some((format!("Added page “{}”", pg.title), Some(pg.id.clone())));
    }
    if let Some(pg) = from.pages.iter().find(|pg| to.page(&pg.id).is_none()) {
        return Some((format!("Deleted page “{}”", pg.title), None));
    }
    if let Some((a, b)) = from.pages.iter().find_map(|a| to.page(&a.id).filter(|b| b.title != a.title).map(|b| (a, b))) {
        return Some((format!("Renamed “{}” to “{}”", a.title, b.title), Some(b.id.clone())));
    }
    let moved = from.pages.iter().enumerate().filter_map(|(i, pg)| to.pages.iter().position(|q| q.id == pg.id).filter(|&j| j != i).map(|j| (i, j, pg))).max_by_key(|&(i, j, _)| (i.abs_diff(j), i))?;
    let (i, j, pg) = moved;
    Some((format!("Moved “{}” {}", pg.title, if j < i { "left" } else { "right" }), Some(pg.id.clone())))
}

/// FX3（FX-012・UIUX B04・MH-M19）: ⌘Z / `↶` が戻さない物を言う 1 句（`↶` の hover の 2 行目と `Undid: …` の 1 行の真ん中）。純関数。
/// Project の面に Shibubu の外の app（Mirror・App の面〈`SurfaceTarget::App`〉）が在れば `Changes inside {App} stay`（名前は app の名前 = 面の node の props `title` を先に・無ければ
/// bundle id の最後の語・2 つは `{A} and {B}`・3 つ以上は `{A}, {B} and {n} more`）・無ければ `Files unchanged`（Terminal と disk の file は戻さない = `PageIntent::Undo` の doc の保証）。
/// 例: `undo_scope_line(&p)` → `Changes inside REAPER stay`。保証しない: 元の app に Undo を送る事（元の app の ⌘Z）・WebView の頁の中身（頁が持つ）。
/// Wave: FX3
pub fn undo_scope_line(p: &ProjectDefinitionV1) -> String {
    let mut names: Vec<String> = Vec::new();
    for s in &p.surface_specs {
        let project::SurfaceTarget::App { bundle_id, .. } = &s.target else { continue };
        let node = p.pages.iter().flat_map(|pg| pg.root.nodes()).find(|n| n.props.get("surface_ref").and_then(|v| v.as_str()) == Some(s.id.as_str()));
        // 面の名前（composer は `label` に `README.md in TextEdit` の形で書く = ` in ` の後が app）→ props の title → bundle id の最後の語
        let said = node.and_then(|n| n.label.clone().or_else(|| n.props.get("title")?.as_str().map(str::to_owned)));
        let name = said.map(|l| l.rsplit(" in ").next().unwrap_or(&l).to_owned()).unwrap_or_else(|| bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_owned());
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    match names.as_slice() {
        [] => "Files unchanged".into(),
        [a] => format!("Changes inside {a} stay"),
        [a, b] => format!("Changes inside {a} and {b} stay"),
        [a, b, rest @ ..] => format!("Changes inside {a}, {b} and {} more stay", rest.len()),
    }
}

/// W12a: `read_resources` が読む resource（uri を持ち、どこかの Page の node が `binding.source_ref == resource_source_id(id)` で bind している）と、
/// その key・bind した node の key。読む（read_resources）と「↻ 読み直す」・理由の 1 行を出す（draw_page）はこの 1 つで決める（2 か所の条件をずらさない）。
fn readable_resources(p: &ProjectDefinitionV1) -> impl Iterator<Item = (&project::ProjectResource, String, Vec<String>)> {
    p.resources.iter().filter(|r| r.uri.is_some()).filter_map(|r| {
        let key = project::resource_source_id(&r.id);
        let bound: Vec<String> = p.pages.iter().flat_map(|pg| pg.root.nodes()).filter(|n| n.binding.as_ref().is_some_and(|b| b.source_ref == key)).map(|n| n.key.clone()).collect();
        (!bound.is_empty()).then_some((r, key, bound))
    })
}

/// W12a: resource を読めなかった時の Page の上の 1 行（理由と次の一手・D-e）。Core は読み方（git 等）を知らないので、
/// resource の kind と ErrorCode だけで言う（action id・serde の原文・内部の id を入れない。Rejected の reason は registry が画面に出せる形で書く）。
fn resource_line(kind: &str, o: &Outcome) -> String {
    match o {
        Outcome::Rejected { reason } => reason.clone(),
        Outcome::Failed { error: ErrorCode::NotReady } => format!("The tool that reads {kind} isn't available on this Mac (not installed, or still installing). Once it's installed, press ↻ Refresh"),
        Outcome::Failed { error: ErrorCode::TargetNotFound } => format!("The {kind} location is missing or isn't a {kind}. Check the location, then press ↻ Refresh"),
        Outcome::Failed { error: ErrorCode::InvalidInput } => format!("Can't read the format of the {kind} location. Write the {kind} location (a path starting with /) in your request and build again"),
        Outcome::Failed { error: ErrorCode::Timeout } => format!("Couldn't read {kind} in time. Wait a moment, then press ↻ Refresh"),
        Outcome::Failed { error } => format!("Couldn't read {kind} ({}). Use “Refresh” to try again", wording(*error)),
        _ => format!("Couldn't read {kind}. Use “Refresh” to try again"),
    }
}

/// binding の path（`a.b.0`）を doc の上で引く。空の path は doc 全体。
fn at_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').filter(|s| !s.is_empty()).try_fold(doc, |v, seg| match v {
        Value::Array(a) => a.get(seg.parse::<usize>().ok()?),
        _ => v.get(seg),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    impl Generated {
        /// LAYOUT の試験（app.rs の本物の殻から読む）: Project の定義（保存した Page の木）と、Page の app が前の frame に描いた node の矩形
        /// （`AppState::rects`・key → 窓の点）。ここ（tests の中）に置く = 本体の `#[cfg(test)]` の行で scripts/interfaces.py と check_app_agnostic.py の読みが止まらない。
        pub(crate) fn layout_probe(&self, project_id: &str) -> (ProjectDefinitionV1, std::collections::HashMap<String, egui::Rect>) {
            let (app, p, _) = self.project(project_id).expect("Project が無い");
            let rects = self.find(&app).map(|o| o.state.rects.clone()).unwrap_or_default();
            (p, rects)
        }
    }

    /// 試験の env（`SHELL`・PATH 等）の lock = process 全体の物。`user_shell` を読む試験と env を替える試験が持つ（bin と generated.rs を取り込む
    /// 結合試験の crate で同じ 1 つ・yc_demo_e2e の `ENV` もこれ = 結合試験の `SHELL=/bin/zsh` の間に g90c が今の shell を読まない）。
    pub(crate) static TEST_ENV: Mutex<()> = Mutex::new(());
    use serde_json::json;

    fn fresh() -> Generated {
        Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::with_local_actions(), None)
    }

    /// 混雑でも緑にする期限: 1 分 load を 4 で割った倍率（1〜8 倍）を掛ける。待つ対象が来ない（壊れている）なら伸ばしても赤のまま
    /// （期限の上限を伸ばすだけで性質は変えない）。load は `sysctl -n vm.loadavg`（`{ 1.23 4.56 7.89 }`）。読めなければ 1 倍。
    fn patient(base: std::time::Duration) -> std::time::Duration {
        let load = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().find_map(|w| w.parse::<f64>().ok()))
            .unwrap_or(0.0);
        base.mul_f64((load / 4.0).clamp(1.0, 8.0))
    }

    /// W14b (a) 自己申告だけで ready にしない: 生成 app の ③ は 1 手の返り（Receipt は Verified）ではなく DB を読む。
    /// 1 手の後に DB から中身が消える（外に結果が残っていない）と、本物の probe は ready を出さない。正の対照: 消さなければ 3 問とも通る。
    /// 負の対照: `AppProbe::read_back` を 1 手の印を返す形（自己申告）に替えると、消した方が ready になってこの試験が赤。
    /// 利用者の session（`main.<app>`）には何も入らず、評価の session は測った後に空。
    #[test]
    fn w14b_app_probe_reads_the_db_not_the_receipt() {
        struct Wiped(AppProbe);
        impl Probe for Wiped {
            fn reach(&mut self, seat: &Seat) -> Check {
                self.0.reach(seat)
            }
            fn act(&mut self, seat: &Seat, step: &LocalStep) -> Check {
                let c = self.0.act(seat, step);
                let source = step.action().and_then(|a| a.rsplit_once('.')).unwrap().0;
                self.0.store.lock().unwrap().put_ui_state(&self.0.session(), source, &json!({})).unwrap();
                c
            }
            fn read_back(&mut self, seat: &Seat, step: &LocalStep) -> Readback {
                self.0.read_back(seat, step)
            }
        }
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        let rev = g.find(&id).unwrap().rt.revision();
        let seat = verifier::app_seat(rev.definition());
        let source = seat.first.as_ref().and_then(|f| f.action.as_deref()).and_then(|a| a.rsplit_once('.')).expect("記録の 1 手").0.to_owned();
        let mut probe = AppProbe::new(g.rt.clone(), g.store.clone(), rev.clone());
        let session = probe.session();
        let u = verifier::usable(&seat, &mut probe, "usable-w14b-a", &mut 1);
        assert!(u.ready(), "正の対照: 本物の probe で 3 問とも通る: {u:?}");
        assert_eq!(u.reach, Check::Pass("The main part was drawn and the input field has focus".into()), "① は描いて focus を見た");
        let in_db = g.store.lock().unwrap().get_ui_state(&session, &source).unwrap().unwrap();
        assert!(in_db.to_string().contains("usable-w14b-a"), "③ の前提: 1 手の印は評価の session の DB に在る: {in_db}");
        drop(probe);
        assert_eq!(g.store.lock().unwrap().get_ui_state(&session, &source).unwrap(), Some(json!({})), "測った後は評価の session を空にする");
        assert_eq!(g.store.lock().unwrap().get_ui_state(&session_of(&id), &source).unwrap(), None, "利用者の session に書いた");
        let u = verifier::usable(&seat, &mut Wiped(AppProbe::new(g.rt.clone(), g.store.clone(), rev)), "usable-w14b-b", &mut 1);
        assert!(matches!(u.first_action, Check::Pass(_)), "1 手そのものは通る（Receipt は Verified）: {u:?}");
        assert!(!u.ready() && matches!(u.readback, Check::Fail { need: Need::Retry, .. }), "外に結果が無いのに ready（自己申告で決めた）: {u:?}");
        assert!(g.run_entries().is_empty(), "評価の 1 手が Runs の一覧に出た: {:?}", g.run_entries().iter().map(|r| &r.what).collect::<Vec<_>>());
    }
    /// W16M-1 の実機で見つけた: 同意の保存が sessions の FK で落ちていた。保存した同意は開き直しても読める
    #[cfg(feature = "mirror")]
    #[test]
    fn w16m1_consent_is_saved_and_read_back() {
        use crate::surface::mirror::{ConsentRecord, CONSENT_UI_STATE_SESSION};
        let mut g = fresh();
        let app = g.create("メモを書きたい").unwrap();
        let r = ConsentRecord { v: 1, bundle_id: "com.apple.TextEdit".into(), granted_at_ms: 1.0 };
        g.save_mirror_state(Some(&app), CONSENT_UI_STATE_SESSION, &r.bundle_id, &r).expect("同意を保存できる");
        g.save_mirror_state(Some(&app), CONSENT_UI_STATE_SESSION, &r.bundle_id, &r).expect("2 回目（上書き）も通る");
        let rows = g.store.lock().unwrap().list_ui_state(CONSENT_UI_STATE_SESSION).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(serde_json::from_value::<ConsentRecord>(rows[0].1.clone()).unwrap(), r);
    }
    /// UI-5m Done 2: 閉じた Mirror の窓は再起動を跨ぐ。製品と同じ道（`save_mirror_choices` → 同じ store から `mirror_pages`）で作り直すと、
    /// 閉じた面は閉じたまま・拡大は開いた形で戻る・閉じた面は描いても窓を探さない（helper の list が 0 回）。正の対照: 開いた面は同じ helper で窓を探す。
    /// 負の対照: `save_mirror_choices` の窓の形の保存を外すと、作り直した面が Open で始まって赤。
    #[cfg(feature = "mirror")]
    #[test]
    fn ui5m_closed_mirror_survives_a_restart() {
        use crate::ui::mirror_view::MirrorWindow;
        let mut g = fresh();
        let app = g.create("メモを書きたい").unwrap();
        let (sh, dir) = crate::surface::mirror::tests::fake_stream_helper("cat >/dev/null");
        let d = dir.display();
        std::fs::write(&sh, format!("#!/bin/sh\necho \"$1\" >> {d}/calls\necho '[]'\n")).unwrap();
        let mut pages = g.mirror_pages(sh.clone(), None);
        pages.set_window("notes", MirrorWindow::Closed, 1.0);
        pages.set_window("docs", MirrorWindow::Zoomed, 1.0);
        g.mirror = Some(pages);
        g.save_mirror_choices(Some(&app));
        assert_eq!(g.page_line.as_ref().map(|(_, l)| l.clone()), None, "窓の形を保存できない");
        // 再起動 = 同じ store から作り直す
        let mut again = g.mirror_pages(sh, None);
        assert_eq!((again.window("notes"), again.window("docs")), (MirrorWindow::Closed, MirrorWindow::Open), "閉じた形が再起動を跨がない / 拡大のまま始まった");
        assert!(again.take_window_changes().is_empty(), "起動で読んだ形を保存し直す変化に数えた");
        let spec = |id: &str| project::SurfaceSpec { id: id.into(), backend: project::SurfaceBackendKind::Mirror, target: project::SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let calls = || std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().count();
        let mut frames = |pages: &mut crate::ui::mirror_view::MirrorPages, spec: &project::SurfaceSpec, until: std::time::Duration, stop: &dyn Fn() -> bool| {
            let t0 = std::time::Instant::now();
            while t0.elapsed() < until && !stop() {
                time += 0.05;
                let _ = ctx.run(egui::RawInput { time: Some(time), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| pages.show(ui, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)), spec));
                });
                pages.end_frame();
                std::thread::sleep(Duration::from_millis(30));
            }
        };
        frames(&mut again, &spec("notes"), patient(Duration::from_millis(600)), &|| false);
        assert_eq!(calls(), 0, "再起動の後、閉じた面が窓を探した");
        frames(&mut again, &spec("docs"), patient(Duration::from_secs(5)), &|| calls() > 0);
        assert!(calls() > 0, "正の対照: 開いた面が窓を探さない（試験が helper の呼び出しを測っていない）");
    }
    /// app.rs の試験も使う（Page の button と同じ runtime の書き込みを Host から・CHAT-011 反証 B28）。
    pub(crate) fn host_of<'a>(g: &'a mut Generated, id: &str) -> (AppDefinitionV3, Host<'a>) {
        let o = g.apps.iter_mut().find(|o| o.rt.revision().definition().id == id).unwrap();
        (o.rt.revision().definition().clone(), Host { rt: &o.rt, regions: &mut o.regions, line: &mut o.line, receipts: Vec::new(), term: None, portals: BTreeMap::new(), placed: Vec::new(), surfaces: Vec::new(), project: String::new(), web: None, takes: None, usage: None })
    }
    fn display(h: &Host<'_>, b: &DataBinding) -> String {
        h.data(b).and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
    }
    /// composer が作った定義から、残り時間を映す node の binding と、押すと動く button（key と action）を引く（試験で id を書き写さない）。
    fn timer_parts(def: &AppDefinitionV3) -> (DataBinding, String, String) {
        let nodes = def.composition.root.nodes();
        let shown = nodes.iter().find_map(|n| n.binding.as_ref().filter(|b| b.path == "remaining_display")).expect("残り時間を映す node が無い").clone();
        let button = nodes.iter().find(|n| n.action_ref.as_deref().is_some_and(|a| a.ends_with(".toggle"))).expect("開始 / 停止の button が無い");
        let action = button.action_ref.clone().unwrap();
        assert!(def.action(&action).is_some(), "button の action が定義に在る: {action}");
        assert_eq!(action.rsplit_once('.').map(|(s, _)| s), Some(shown.source_ref.as_str()), "button と表示が同じ source");
        (shown, button.key.clone(), action)
    }

    /// L7-AGENT（Acceptance・図 §31b）: Terminal の agent が別の接続（= 別 process の `shibubu project ask`）で置いた一言は、画面の Shibubu の
    /// 承認の card になるだけで Project は変わらない。断れば何も変わらず run は cancelled（却下の記録）・承認した時だけ人が欄に打ったのと同じ道で変わり
    /// run は verified（承認の記録 = action_ref・exact payload の hash・この run）。agent は `shibubu project context` の runs で結果を読める。
    #[test]
    fn l7_agent_ask_changes_the_project_only_after_approval() {
        use crate::project::context::{self, ASK_ACTION};
        use crate::ui::engine_view::{EngineAct, EngineView};
        use crate::domain::run::RunStatus;
        let dir = std::env::temp_dir().join(format!("shibubu-gen-l7-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let View::Project { project, .. } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        let pages = |g: &Generated| g.project(&project).unwrap().1.pages.iter().map(|p| p.title.clone()).collect::<Vec<_>>();
        let (before, rev) = (pages(&g), g.project(&project).unwrap().1.revision);
        assert!(!before.iter().any(|t| t == "Tests"));
        assert!(!g.poll_asks(), "頼まれていない間は card を出さない");
        let terminal = Store::open(&dir.join(DB_FILES[0])).unwrap(); // 別の接続 = Terminal の CLI
        let by = InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("grok".into()) };
        let asked = context::ask(&terminal, &project, "add a Tests page", by.clone()).unwrap();
        assert!(g.poll_asks(), "別の接続の commit（data_version）で拾う");
        // t1 はこの Project の Terminal ではない（別の Project・Shibubu の外から頼めば出どころをそう言う・自分の Terminal の時は l7_ask_from_this_projects_terminal_is_not_called_outside）
        assert_eq!(g.engine_view(&project), Some(&EngineView::Ask { who: "grok in a terminal outside this project".into(), text: "add a Tests page".into() }));
        assert_eq!((pages(&g), g.project(&project).unwrap().1.revision), (before.clone(), rev), "card を出しただけで変えない");
        assert!(!g.poll_asks(), "同じ行で card を 2 度出さない");
        // 断る（負の対照）: Project は 1 byte も変えず、run は cancelled と却下の記録
        l7_arm(&mut g);
        g.engine_act(&project, EngineAct::Cancel);
        assert_eq!(g.engine_view(&project), None);
        assert_eq!((pages(&g), g.project(&project).unwrap().1.revision), (before.clone(), rev), "承認しなければ何も変わらない");
        let load = |id: &str| crate::runtime::receipt::load(&terminal, id).unwrap().unwrap();
        let declined = load(&asked.id);
        assert_eq!(declined.run.status, RunStatus::Cancelled);
        let c = declined.consent.expect("却下の記録");
        assert_eq!(c.decision, crate::domain::permission::ConsentDecision::Denied);
        g.asks.next = None;
        assert!(!g.poll_asks(), "決めた行は card に戻らない");
        // 承認: 同じ一言をもう 1 回 → card → 承認した時だけ Tests の Page が増える
        let asked = context::ask(&terminal, &project, "add a Tests page", by).unwrap();
        g.asks.next = None;
        assert!(g.poll_asks());
        l7_arm(&mut g);
        g.engine_act(&project, EngineAct::Approve);
        assert!(pages(&g).iter().any(|t| t == "Tests"), "承認で変わる: {:?}", pages(&g));
        assert!(g.project(&project).unwrap().1.revision > rev);
        let done = load(&asked.id);
        assert_eq!(done.run.status, RunStatus::Verified);
        let c = done.consent.expect("承認の記録");
        assert_eq!(c.decision, crate::domain::permission::ConsentDecision::Approved);
        assert_eq!(c.subject, crate::domain::permission::ConsentSubject::Action { action_ref: ASK_ACTION.into(), run_id: Some(asked.id.clone()) });
        // exact payload: 記録の hash を、card の文から別に計算した hash と照らす（記録どうしを比べない）
        let app = project.strip_prefix("project.").unwrap();
        let want = crate::runtime::permission::payload_hash(ASK_ACTION, app, &project, &json!({ "text": "add a Tests page" }), &[ASK_ACTION.to_owned()]);
        assert_eq!(c.payload_digest.as_deref(), Some(want.as_str()), "exact payload の承認");
        // agent の読み口: runs に 2 本・待っている物は無い
        let ctx = context::read(&terminal, &project).unwrap();
        let mine: Vec<RunStatus> = ctx.runs.iter().filter(|r| r.what == "add a Tests page").map(|r| r.status).collect();
        assert_eq!(mine, [RunStatus::Cancelled, RunStatus::Verified]);
        assert!(ctx.waiting.is_empty());
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し（be0fcb98 への反証）の試験の台: Engine（`Idle`）を入れた Project・別の接続（= Terminal の CLI）・Project の外の Terminal の agent。
    fn l7_ask_bench(tag: &str) -> (Generated, String, std::path::PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("shibubu-gen-{tag}-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let View::Project { project, .. } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Idle), data_dir: dir.clone() });
        let terminal = Store::open(&dir.join(DB_FILES[0])).unwrap();
        (g, project, dir, terminal)
    }

    /// 台の上で agent が頼み（`context::ask`）、card を出して押せる所まで待つ（`l7_arm`）。戻り = run の id。
    fn l7_ask(g: &mut Generated, terminal: &Store, project: &str, text: &str) -> String {
        let by = InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("grok".into()) };
        let asked = crate::project::context::ask(terminal, project, text, by).unwrap();
        g.asks.next = None;
        assert!(g.poll_asks(), "card が出ない: {text}");
        l7_arm(g);
        asked.id
    }

    /// 出ている card の `ASK_GAP` を過ぎた事にする（人が card を読んでから押す）
    fn l7_arm(g: &mut Generated) {
        g.asks.shown.values_mut().for_each(|(_, from)| *from = std::time::Instant::now());
    }

    fn l7_status(terminal: &Store, id: &str) -> crate::domain::run::RunStatus {
        crate::runtime::receipt::load(terminal, id).unwrap().unwrap().run.status
    }

    /// L7 の反証の直し: Engine に渡る一言（`tell me the weather`）は承認しても Engine に渡さない（承認の範囲 = Project の編集だけ）。Failed・`ASK_SCOPE_LINE`・Project は不変。
    /// 変異: finish_edit の `!asking` を外す → Engine の job が起きて赤。
    #[test]
    fn l7_approved_ask_is_not_handed_to_the_engine() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-engine");
        let rev = g.project(&project).unwrap().1.revision;
        let id = l7_ask(&mut g, &terminal, &project, "tell me the weather");
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        assert!(g.engine_job.is_none(), "承認の範囲（Project の編集）の外を Engine に渡した");
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::Failed);
        assert_eq!(g.chat(&project).last().map(|t| t.text.clone()).as_deref(), Some(ASK_SCOPE_LINE));
        // 台の Project を作った人の一言は You で在る。agent の一言だけを見る
        assert!(!g.chat(&project).iter().any(|t| t.who == crate::domain::chat::Who::You && t.text.contains("tell me the weather")), "agent の一言を人の発言にした");
        assert_eq!(g.project(&project).unwrap().1.revision, rev);
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し: `run the tests` は承認しても走らせない（Failed・`ASK_SCOPE_LINE`）。変異: decide_ask の RunTests の止めを外す → 1 行が違って赤。
    #[test]
    fn l7_approved_ask_does_not_run_the_tests() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-tests");
        let id = l7_ask(&mut g, &terminal, &project, "run the tests");
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::Failed);
        assert_eq!(g.chat(&project).last().map(|t| t.text.clone()).as_deref(), Some(ASK_SCOPE_LINE));
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し: card を出した後に行の文を書き換えた頼みは承認しない（Cancelled・Project は不変・「changed after it was shown」）。
    /// DB に書ける子は hash も計算し直せるので、hash も合わせて書き換える（文の照合だけが止める）。
    /// 変異: decide_ask の `same` の文の照合を true にする → 書き換えた文で Project が変わって赤。
    #[test]
    fn l7_ask_changed_after_the_card_is_not_approved() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-same");
        let rev = g.project(&project).unwrap().1.revision;
        let id = l7_ask(&mut g, &terminal, &project, "add a Tests page");
        let mut rec = crate::runtime::receipt::load(&terminal, &id).unwrap().unwrap();
        rec.input = json!({ "text": "add a Docs page" });
        let a = rec.approval.as_mut().unwrap();
        a.payload_hash = crate::runtime::permission::payload_hash(&a.action_id, &rec.run.app_id, &rec.space_id, &rec.input, &a.scopes);
        crate::runtime::receipt::save(&terminal, &rec).unwrap();
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        assert_eq!((l7_status(&terminal, &id), g.project(&project).unwrap().1.revision), (crate::domain::run::RunStatus::Cancelled, rev), "card と違う文を承認として読んだ");
        assert!(g.chat(&project).last().is_some_and(|t| t.text.contains("changed after it was shown")));
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し: 決めた直後（`ASK_GAP` の間）は次の頼みの card を出さない（2 度押しで次の Approve を押させない）。変異: decide_ask の `next` を外す → すぐ出て赤。
    #[test]
    fn l7_no_new_ask_card_right_after_deciding() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-gap");
        l7_ask(&mut g, &terminal, &project, "add a Tests page");
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Cancel);
        let by = InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("grok".into()) };
        crate::project::context::ask(&terminal, &project, "add a Docs page", by).unwrap();
        assert!(!g.poll_asks(), "決めた直後に次の card を出した");
        g.asks.next = None;
        assert!(g.poll_asks(), "正の対照: 間を過ぎれば出る");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し: card が他の card に上書きされても、席が空けば同じ頼みの card が戻る（頼みを宙に浮かせない）。変異: poll_asks の `retain` を外す → 戻らず赤。
    #[test]
    fn l7_covered_ask_card_comes_back() {
        use crate::ui::engine_view::EngineView;
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-cover");
        let id = l7_ask(&mut g, &terminal, &project, "add a Tests page");
        g.engine_views.insert(project.clone(), EngineView::Line { state: RegionState::Ready, line: "other".into() });
        g.asks.next = None;
        assert!(g.poll_asks(), "上書きされた頼みが戻らない");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Ask { text, .. }) if text == "add a Tests page"));
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::WaitingApproval);
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し（operator 04:0x「card が替わった frame は押せない」）: card が出てから `ASK_GAP` の間の押しは読まない
    /// （Engine の card への 2 度押しの 2 つ目が、次の frame に出た頼みの card に当たる）。card は出たまま・run は承認待ちのまま・間の後なら決められる。
    /// 変異: decide_ask の時刻の門を外す → 出た frame の押しで Verified になって赤。
    #[test]
    fn l7_ask_card_is_not_decided_in_the_frame_it_appears() {
        use crate::ui::engine_view::{EngineAct, EngineView};
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-arm");
        let rev = g.project(&project).unwrap().1.revision;
        let by = InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("grok".into()) };
        let id = crate::project::context::ask(&terminal, &project, "add a Tests page", by).unwrap().id;
        assert!(g.poll_asks());
        g.engine_act(&project, EngineAct::Approve);
        assert_eq!((l7_status(&terminal, &id), g.project(&project).unwrap().1.revision), (crate::domain::run::RunStatus::WaitingApproval, rev), "出た frame の押しで決めた");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Ask { .. })), "押しを読まない間も card は出たまま");
        l7_arm(&mut g);
        g.engine_act(&project, EngineAct::Approve);
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::Verified, "正の対照: 間の後の押しは読む");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し（hash の負の対照）: 文は同じで payload（input）だけ替えた頼みは承認しない（Cancelled・Project は不変）。
    /// 変異: decide_ask の `same` から hash の照合を外す → 文が同じなので承認して Verified になり赤。
    #[test]
    fn l7_ask_with_a_changed_payload_is_not_approved() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-hash");
        let rev = g.project(&project).unwrap().1.revision;
        let id = l7_ask(&mut g, &terminal, &project, "add a Tests page");
        let mut rec = crate::runtime::receipt::load(&terminal, &id).unwrap().unwrap();
        rec.input = json!({ "text": "add a Tests page", "also": "rm -rf ~" });
        crate::runtime::receipt::save(&terminal, &rec).unwrap();
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        assert_eq!((l7_status(&terminal, &id), g.project(&project).unwrap().1.revision), (crate::domain::run::RunStatus::Cancelled, rev), "承認した時と違う payload を通した");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直し（試験の穴: AI が在る時の承認の道）: 承認した一言は `start_edit` の裏の thread で model（偽）が読み、`poll_model_job` が当てて run は Verified。
    /// 人の発言にしない（`asking`）。本物の model の同じ道は `llm_cli::l7_live_…`（手で回す）。変異: finish_edit の `!asking` を外す → You の行が出て赤。
    #[test]
    fn l7_approved_ask_is_read_by_the_model() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-model");
        g.set_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] }))]))));
        let id = l7_ask(&mut g, &terminal, &project, "add a Tests page");
        g.engine_act(&project, crate::ui::engine_view::EngineAct::Approve);
        assert!(g.model_job.is_some(), "承認した一言を model に渡さない");
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::Running, "読んでいる間は Running");
        let ModelDone::Edited { made, .. } = wait_done(&mut g) else { panic!("編集の答えでない") };
        assert!(made.is_ok(), "{made:?}");
        assert!(g.project(&project).unwrap().1.pages.iter().any(|p| p.title == "Tests"));
        assert_eq!(l7_status(&terminal, &id), crate::domain::run::RunStatus::Verified);
        assert!(!g.chat(&project).iter().any(|t| t.who == crate::domain::chat::Who::You && t.text == "add a Tests page"), "agent の一言を人の発言にした");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7 の反証の直しの正の対照: この Project の Terminal（panel の tab）の頼みは出どころに `outside this project` を足さない。
    /// 変異: poll_asks の `!mine` を外す（いつも outside）→ 赤。
    #[test]
    fn l7_ask_from_this_projects_terminal_is_not_called_outside() {
        let (mut g, project, dir, terminal) = l7_ask_bench("l7-mine");
        g.panel_act(&project, PanelAct::Add).unwrap();
        let tab = g.panel(&project).tabs.last().unwrap().id.clone();
        let by = InvocationOrigin::Terminal { terminal_id: tab, agent: Some("grok".into()) };
        crate::project::context::ask(&terminal, &project, "add a Tests page", by).unwrap();
        g.asks.next = None;
        assert!(g.poll_asks());
        assert!(matches!(g.engine_view(&project), Some(crate::ui::engine_view::EngineView::Ask { who, .. }) if who == "grok in a terminal"), "{:?}", g.engine_view(&project));
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W9: DB を開けた時だけ Terminal に Project の口（保存先の絶対 path・実行 file の dir）を渡す。memory の store には渡さない。
    #[test]
    fn w9_open_hands_the_project_cli_to_terminals_only_with_a_saved_db() {
        let dir = std::env::temp_dir().join(format!("shibubu-gen-w9-{}", storage::new_id()));
        let g = Generated::open(&dir);
        let cli = g.terminals().config().project_cli.clone().expect("DB を開けたのに Project の口が無い");
        assert_eq!(cli.data_dir, dir.canonicalize().unwrap());
        assert!(cli.data_dir.is_absolute());
        assert_eq!(cli.bin_dir, std::env::current_exe().unwrap().parent().unwrap());
        drop(g);
        // 保存先に file が在って DB を作れない → memory の store → 口は渡さない（読む保存が無い）
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let g = Generated::open(&blocked);
        assert!(g.note.is_some(), "memory の store で開いた事を言う");
        assert_eq!(g.terminals().config().project_cli, None);
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SESSIONS-LEFT-1101（owner 210 の 4「newsessionえらぶとappの中にあった前の会話の履歴がなくなった。増えていく形で」）: 押した新しい session は一言の前から
    /// 一覧の一番上に `New session` で出て、前の session は残る・一言も無いまま前の session に戻ると空の session は外れる。
    /// 変異: `chat_list` の空の今の session を足す所を外す → 1 つ目の assert で赤・`pick_session` の後も残す（`fresh` で足す）→ 最後の assert で赤。
    #[test]
    fn sessions_left_new_session_is_listed_before_its_first_message() {
        use crate::domain::chat::{session_project, Mark, Who};
        let dir = w1_dir("sessions-210");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        g.say(&project, Who::You, "最初の会話", Mark::None, Vec::new());
        let first = g.session_key(&project);
        let fresh = g.new_session(&project);
        let mine: Vec<(String, String)> = g.chat_list().iter().filter(|r| session_project(&r.0) == Some(project.as_str())).map(|r| (r.0.clone(), r.1.clone())).collect();
        assert_eq!(mine.first(), Some(&(fresh.clone(), "New session".to_owned())), "押した session が一覧の一番上に無い: {mine:?}");
        assert!(mine.iter().any(|(k, _)| *k == first), "前の session が一覧から消えた: {mine:?}");
        g.pick_session(&project, &first);
        assert!(!g.chat_list().iter().any(|r| r.0 == fresh), "一言の無い session が他へ移った後も残る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SESSIONS-LEFT-1101 × owner 189（開き直したら前の session）: Project で開いた chat の session は同じ DB を開き直しても同じ（ui_state の `chat_session`）・
    /// 1 つ目に戻せば行を消す（開き直しても 1 つ目）。負の対照: `start` の読み戻しを `BTreeMap::new()` に戻す → 1 つ目の assert が赤 /
    /// `keep_session` の `delete_ui_state` を外す → 2 つ目が赤。
    #[test]
    fn sessions_left_the_open_session_comes_back_after_restart() {
        let dir = std::env::temp_dir().join(format!("shibubu-gen-sl-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let project = project_id_of(&g.create("タイマーを作って").unwrap());
        let key = g.new_session(&project);
        drop(g);
        let mut g = Generated::open(&dir);
        assert_eq!(g.session_key(&project), key, "開き直したら 1 つ目の session に戻った");
        g.pick_session(&project, &project);
        drop(g);
        let g = Generated::open(&dir);
        assert_eq!(g.session_key(&project), project, "1 つ目に戻したのに前の session を戻した");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 通し（試験）: タイマーを作る → 押す前は 25:00 → 開始で running・表示が進む → 同じ DB を開き直しても残る → 読書記録は別の形
    #[test]
    fn timer_goal_runs_and_survives_restart_and_books_differ() {
        let dir = std::env::temp_dir().join(format!("shibubu-gen-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        assert!(g.note.is_none(), "{:?}", g.note);
        let id = g.create("タイマーを作って").unwrap();
        let (bind, key, action) = {
            let (def, mut h) = host_of(&mut g, &id);
            let (bind, key, action) = timer_parts(&def);
            assert_eq!(h.readiness(&action), None, "runtime に在る action は定義の readiness のまま");
            assert_eq!(display(&h, &bind), "25:00");
            h.invoke(ActionInvocation { action_ref: action.clone(), origin: InvocationOrigin::Button { node_key: key.clone() }, input: json!({}) });
            assert!(h.line.is_none() && h.region(&key).is_none(), "Verified なら何も足さない: {:?}", h.line);
            assert_eq!(h.rt.ui_state(&bind.source_ref).unwrap()["running"], json!(true));
            (bind, key, action)
        };
        assert!(!g.rows()[0].status.contains("not set up"), "全部動く app に「使えない」を出さない: {}", g.rows()[0].status);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (_, h) = host_of(&mut g, &id);
        let shown = display(&h, &bind);
        assert!(shown != "25:00" && shown.starts_with("24:"), "動いている間は表示が進む: {shown}");
        drop(g);
        let mut g = Generated::open(&dir);
        assert_eq!(g.rows().len(), 1, "再起動で app が残る");
        let (def, h) = host_of(&mut g, &id);
        assert_eq!(timer_parts(&def), (bind.clone(), key, action), "開き直した定義も同じ button と表示");
        assert_eq!(h.rt.ui_state(&bind.source_ref).unwrap()["running"], json!(true), "再起動で ui_state が残る");
        let books = g.create("読書記録をつけたい").unwrap();
        let shape = |g: &Generated, id: &str| composer::component_set(g.find(id).unwrap().rt.revision().definition());
        assert_ne!(shape(&g, &id), shape(&g, &books), "読書記録で構成が変わる");
        assert_eq!(g.rows()[0].view, View::App(books.clone()), "新しい順");
        let (def, mut h) = host_of(&mut g, &books);
        let missing: Vec<_> = def.actions.iter().filter(|a| h.rt.runtime().registry().get(&a.id).is_none()).map(|a| a.id.clone()).collect();
        assert!(missing.is_empty(), "読書記録の操作は全部 runtime に在る: {missing:?}");
        let add = def.actions.iter().find(|a| a.id.ends_with(".add")).expect("読書記録に足す操作が無い").id.clone();
        h.invoke(ActionInvocation { action_ref: add.clone(), origin: InvocationOrigin::Button { node_key: "books-add".into() }, input: json!({ "title": "坊っちゃん" }) });
        assert!(h.line.is_none(), "1 冊足すと Verified: {:?}", h.line);
        let source = add.rsplit_once('.').unwrap().0;
        assert_eq!(h.rt.ui_state(source).unwrap()["items"][0]["title"], json!("坊っちゃん"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// composer の一覧（読書記録の外: 映画・筋トレ・家計簿）の操作は entity ごとに書かず、開いた時に定義から導く。
    /// 足す・状態を変える・絞るが Verified、状態行は「使えません」を言わない。導けない verb（import_csv）だけが Unsupported のまま数えられる
    #[test]
    fn list_entities_get_generic_local_actions_from_their_definition() {
        let mut g = fresh();
        for (goal, title_field) in [("見た映画を記録したい", "title"), ("筋トレの記録をつけたい", "exercise")] {
            let id = g.create(goal).unwrap();
            let (def, mut h) = host_of(&mut g, &id);
            let reg = h.rt.runtime().registry();
            let missing: Vec<_> = def.actions.iter().filter(|a| reg.get(&a.id).is_none()).map(|a| a.id.clone()).collect();
            assert!(missing.is_empty(), "{goal}: 一覧の操作は全部 runtime に在る: {missing:?}");
            let add = def.actions.iter().find(|a| a.id.ends_with(".add")).expect("足す操作").id.clone();
            let source = add.rsplit_once('.').unwrap().0.to_owned();
            assert!(!["local.books", "local.inventory", "local.tasks", "local.notes"].contains(&source.as_str()), "corpus の外の entity で試す: {source}");
            assert_eq!(h.readiness(&add), None, "{goal}: 押せる");
            h.invoke(ActionInvocation { action_ref: add.clone(), origin: InvocationOrigin::Button { node_key: "add".into() }, input: json!({ title_field: "一件目" }) });
            assert!(h.line.is_none(), "{goal}: 1 件足すと Verified: {:?}", h.line);
            let doc = h.rt.ui_state(&source).unwrap();
            assert_eq!(doc["items"][0][title_field], json!("一件目"), "{goal}");
            let item = doc["items"][0]["id"].as_str().unwrap().to_owned();
            if let Some(upd) = def.actions.iter().find(|a| a.id.starts_with(&format!("{source}.update_"))) {
                let field = upd.id.rsplit_once(".update_").unwrap().1.to_owned();
                let value = def.actions.iter().find(|a| a.id == upd.id).and_then(|a| a.input_schema["properties"][&field]["enum"][1].as_str()).unwrap_or("二").to_owned();
                h.invoke(ActionInvocation { action_ref: upd.id.clone(), origin: InvocationOrigin::Button { node_key: "upd".into() }, input: json!({ "item_id": item, field.clone(): value }) });
                assert!(h.line.is_none(), "{goal}: {} が Verified: {:?}", upd.id, h.line);
                assert_eq!(h.rt.ui_state(&source).unwrap()["items"][0][&field], json!(value));
            }
            let filter = format!("{source}.filter");
            if def.action(&filter).is_some() {
                h.invoke(ActionInvocation { action_ref: filter.clone(), origin: InvocationOrigin::Button { node_key: "filter".into() }, input: json!({ "query": "一件" }) });
                assert!(h.line.is_none(), "{goal}: 絞り込みが Verified: {:?}", h.line);
                assert_eq!(h.rt.ui_state(&source).unwrap()["filter"]["ids"], json!([item]), "{goal}: 欄の名前を渡さなくても文字の欄で絞る");
            }
            let row = g.rows().into_iter().find(|r| r.view == View::App(id.clone())).unwrap();
            assert!(!row.status.contains("not set up"), "{goal}: {}", row.status);
            // Runs の行は runtime の RunRecord から（押した add が Verified で載る）
            let runs = g.run_entries();
            let ran = runs.iter().find(|e| e.what == add).unwrap_or_else(|| panic!("{goal}: Runs に add が無い: {:?}", runs.iter().map(|e| &e.what).collect::<Vec<_>>()));
            assert_eq!(ran.outcome().map(|o| matches!(o, Outcome::Verified { .. })), Some(true), "{goal}");
        }
        // Runs の一手: 接続は Connections へ・確かめた run は一手無し・止めても止めない
        let any = g.run_entries()[0].id.clone();
        assert_eq!(g.run_action(&RunsAction { run_id: any.clone(), action: RunAction::Connect }).0, true);
        assert_eq!(g.run_entries()[0].next_step(), Some(RunAction::OpenProject), "確かめた run の一手は Project へ戻る Open project だけ（FX-037: 押しても何もしない Connect / Cancel / Check again は出さない）");
        let (_, line) = g.run_action(&RunsAction { run_id: any, action: RunAction::Cancel });
        assert!(line.contains("Too late to stop"), "Verified の run は止めない: {line}");
        // FX7: `Check again` は記録を読み直して今の結果を言う（確かめた run は Verified のまま・無い id は無いと言う）。どちらも送り直さない
        let (_, again) = g.run_action(&RunsAction { run_id: g.run_entries()[0].id.clone(), action: RunAction::CheckAgain });
        assert_eq!(again, "Checked again: Verified.", "Check again が記録を読み直さない");
        assert_eq!(g.run_action(&RunsAction { run_id: "no-such-run".into(), action: RunAction::CheckAgain }).1, "That run isn't in the history any more.");
        // 家計簿は CSV の読み込み（import_csv）を持つ。読み込みは導かない = その 1 個だけ使えないと言う（成功を名乗らない）
        let id = g.create("家計簿をつけたい").unwrap();
        let def = g.find(&id).unwrap().rt.revision().definition().clone();
        let reg = g.find(&id).unwrap().rt.runtime().registry();
        let missing: Vec<_> = def.actions.iter().filter(|a| reg.get(&a.id).is_none()).map(|a| a.id.clone()).collect();
        assert_eq!(missing.iter().map(|m| m.rsplit_once('.').unwrap().1).collect::<Vec<_>>(), vec!["import_csv"], "{missing:?}");
        assert_eq!(g.rows()[0].status, "1 action not set up");
    }

    /// corpus と同じ名前の一覧（日記 = local.notes・タスク・在庫）: composer の入力の形（1 件は item_id）で、足す・変える・済み・削除（承認して）が
    /// 全部 Verified。絞り込みは自動の欄（created_at / updated_at）に当たらない（":" は時刻にだけ在る）
    #[test]
    fn corpus_named_lists_take_composer_inputs() {
        let mut g = fresh();
        for goal in ["日記をつけたい", "タスクを管理したい", "在庫を管理したい"] {
            let id = g.create(goal).unwrap();
            let def = g.find(&id).unwrap().rt.revision().definition().clone();
            let rt = &g.find(&id).unwrap().rt;
            let add = def.actions.iter().find(|a| a.id.ends_with(".add")).unwrap_or_else(|| panic!("{goal}: 足す操作")).id.clone();
            let source = add.rsplit_once('.').unwrap().0.to_owned();
            let head = def.actions.iter().find(|a| a.id == add).and_then(|a| a.input_schema["required"][0].as_str().map(String::from)).unwrap_or_else(|| "title".into());
            let run = |action: &str, input: Value| -> Outcome {
                let done = match rt.invoke(action, input, InvocationOrigin::Button { node_key: "k".into() }).unwrap() {
                    Invoked::Done(r) => r,
                    Invoked::NeedsApproval(req) => rt.approve(&req.run_id, &req.payload_hash).unwrap(),
                };
                done.outcome
            };
            assert!(matches!(run(&add, json!({ head.clone(): "一件目" })), Outcome::Verified { .. }), "{goal}: {add}");
            let item = rt.ui_state(&source).unwrap()["items"][0]["id"].as_str().unwrap().to_owned();
            let filter = format!("{source}.filter");
            if def.action(&filter).is_some() {
                assert!(matches!(run(&filter, json!({ "query": ":" })), Outcome::Verified { .. }), "{goal}");
                assert_eq!(rt.ui_state(&source).unwrap()["filter"]["ids"], json!([]), "{goal}: 時刻の欄で当てない");
            }
            for a in def.actions.iter().filter(|a| a.id.starts_with(&format!("{source}.update_")) || a.id == format!("{source}.toggle_done")) {
                let mut input = json!({ "item_id": item });
                if let Some(field) = a.id.rsplit_once(".update_").map(|(_, f)| f) {
                    let p = &a.input_schema["properties"][field];
                    input[field] = match (&p["enum"][0], p["type"].as_str()) {
                        (Value::String(s), _) => json!(s),
                        (_, Some("integer" | "number")) => json!(2),
                        _ => json!("二"),
                    };
                }
                assert!(matches!(run(&a.id, input.clone()), Outcome::Verified { .. }), "{goal}: {} {input}", a.id);
            }
            let delete = format!("{source}.delete");
            assert!(def.action(&delete).is_some(), "{goal}: 削除が定義に在る");
            let out = run(&delete, json!({ "item_id": item }));
            assert!(matches!(out, Outcome::Verified { .. }), "{goal}: {delete} → {out:?}");
            assert_eq!(rt.ui_state(&source).unwrap()["items"], json!([]), "{goal}: 消えた");
        }
        // 承認待ちの削除は Runs の一手（止める）で終われる（待ったまま出口が無い、にしない）
        let id = g.create("日記をつけたい").unwrap();
        let rt = &g.find(&id).unwrap().rt;
        let origin = || InvocationOrigin::Button { node_key: "k".into() };
        let Invoked::Done(added) = rt.invoke("local.notes.add", json!({ "title": "消さない" }), origin()).unwrap() else { panic!("add は承認不要") };
        assert!(matches!(added.outcome, Outcome::Verified { .. }));
        let item = rt.ui_state("local.notes").unwrap()["items"][0]["id"].clone();
        let Invoked::NeedsApproval(req) = rt.invoke("local.notes.delete", json!({ "item_id": item }), origin()).unwrap() else { panic!("削除は承認待ち") };
        let waiting = g.run_entries().into_iter().find(|e| e.id == req.run_id).unwrap();
        assert_eq!(waiting.next_step(), Some(RunAction::Cancel));
        let (go, line) = g.run_action(&RunsAction { run_id: req.run_id.clone(), action: RunAction::Cancel });
        assert!(!go && line.starts_with("Stopped"), "{line}");
        let after = g.run_entries().into_iter().find(|e| e.id == req.run_id).unwrap();
        assert_eq!(after.next_step(), Some(RunAction::OpenProject), "取り消した run は終わり: 残る一手は Project へ戻る Open project だけ");
        assert_eq!(g.find(&id).unwrap().rt.ui_state("local.notes").unwrap()["items"].as_array().map(Vec::len), Some(1), "止めたので消えていない");
    }

    /// W2b-AX 直し: idle の窓は frame を頼まない（src/app.rs `next_repaint`）ので、動いている timer は残りの秒が替わる時の frame を
    /// 描く側が自分で頼む（窓が背景でも）。止まっている・止めた timer は頼まない。
    #[test]
    fn w2b_ax_running_timer_asks_for_the_next_second_only_while_running() {
        use std::time::Duration;
        let mut g = fresh();
        let id = g.create("タイマーを作って").unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light); // 見出しの font family（本物の app と同じ）
        let mut t = 0.0;
        let mut frame = |g: &mut Generated| {
            t += 0.5;
            let out = ctx.run(egui::RawInput { time: Some(t), focused: false, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    g.draw(ui, &id);
                });
            });
            out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay)
        };
        let toggle = |g: &mut Generated| {
            let (def, mut h) = host_of(g, &id);
            let (_, key, action) = timer_parts(&def);
            h.invoke(ActionInvocation { action_ref: action, origin: InvocationOrigin::Button { node_key: key }, input: json!({}) });
        };
        for _ in 0..3 {
            frame(&mut g);
        }
        assert_eq!(frame(&mut g), Duration::MAX, "止まっている timer で frame を頼む");
        toggle(&mut g);
        frame(&mut g);
        let d = frame(&mut g);
        assert!(d <= Duration::from_secs(1), "動いている timer が次の秒の frame を頼まない（{d:?}）= 背景・idle の窓で残りが止まって見える");
        toggle(&mut g);
        for _ in 0..3 {
            frame(&mut g);
        }
        assert_eq!(frame(&mut g), Duration::MAX, "止めた timer で frame を頼み続ける");
    }

    /// local action が登録されていなければ、同じ button は押せない形で出て、押しても NotReady の 1 行（成功を演出しない）
    #[test]
    fn unregistered_actions_are_unsupported_and_invoke_says_so() {
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::default(), None);
        let id = g.create("タイマーを作って").unwrap();
        let (_, mut h) = host_of(&mut g, &id);
        assert_eq!(h.readiness("local.timer.toggle"), Some(ActionReadiness::Unsupported));
        h.invoke(ActionInvocation { action_ref: "local.timer.toggle".into(), origin: InvocationOrigin::Button { node_key: "timer-toggle".into() }, input: json!({}) });
        assert_eq!(h.region("timer-toggle").map(|r| r.0), Some(RegionState::Error));
        assert!(h.line.as_ref().is_some_and(|(_, l)| l.contains("Not set up")), "{:?}", h.line);
    }

    /// registry に無い操作を持つ app は、状態行で「準備完了」を名乗らず、使えない数を言う
    #[test]
    fn rows_say_how_many_actions_cannot_run() {
        let mut g = Generated::start(Store::open_in_memory().unwrap(), ActionRegistry::default(), None);
        let id = g.create("タイマーを作って").unwrap();
        let n = g.find(&id).unwrap().rt.revision().definition().actions.len();
        let status = &g.rows()[0].status;
        assert_eq!(status, &format!("{n} actions not set up"), "{status}");
    }

    #[test]
    fn unreadable_goals_stay_on_home_with_a_reason() {
        let mut g = fresh();
        for goal in ["", "   "] {
            assert!(g.create(goal).is_err(), "{goal:?}");
        }
        assert!(g.rows().is_empty(), "作れなかった物を app にしない");
    }

    // ---- YC W1: Project / Page / Revision（YCM-010・011・016・043）----

    fn w1_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("shibubu-w1-{tag}-{}", storage::new_id()))
    }
    fn tests_page(id: &str) -> project::PageDefinition {
        project::PageDefinition {
            id: id.into(),
            route: format!("/{id}"),
            title: "Tests".into(),
            app_ref: None,
            root: crate::domain::ui::UiNode { key: format!("{id}-root"), component: "column".into(), ..Default::default() },
            revision: 1,
        }
    }
    fn add_page(id: &str) -> Vec<ProjectPatchOp> {
        vec![ProjectPatchOp::AddPage { page: tests_page(id), index: None }]
    }

    /// Done 2: Wave 0 の版（schema v1・projects の表が無い）の DB に、読書記録の app と 1 冊・1 run がある。
    /// 今の版で開くと Page 1 枚の Project として読め、ui_state と run も残り、apps の行は書き換わらない（包んでも何も書かない）。
    #[test]
    fn w1_old_db_app_reads_as_one_page_project_and_keeps_state() {
        let dir = w1_dir("old");
        let mut g = Generated::open(&dir);
        let id = g.create("読書記録をつけたい").unwrap();
        let source = {
            let (def, mut h) = host_of(&mut g, &id);
            let add = def.actions.iter().find(|a| a.id.ends_with(".add")).unwrap().id.clone();
            h.invoke(ActionInvocation { action_ref: add.clone(), origin: InvocationOrigin::Button { node_key: "books-add".into() }, input: json!({ "title": "坊っちゃん" }) });
            assert!(h.line.is_none(), "{:?}", h.line);
            add.rsplit_once('.').unwrap().0.to_owned()
        };
        drop(g);
        // Wave 0 の版に戻す: V2 以降の表（W2c の V3 の surface_receipts・W4 の V4 の terminal_sessions も）と、その適用の記録を消す（V1 の表は今と同じ SQL なので、残りは Wave 0 が作る DB そのもの）
        let db = dir.join("kangae.sqlite3");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("DROP TABLE run_receipts; DROP TABLE surface_receipts; DROP TABLE terminal_sessions; DROP TABLE project_revisions; DROP TABLE projects; DROP TABLE chat_turns; DELETE FROM schema_migrations WHERE version >= 2;").unwrap();
        let app_row: String = conn.query_row("SELECT json FROM apps WHERE id=?1", [&id], |r| r.get(0)).unwrap();
        let app_revs: i64 = conn.query_row("SELECT COUNT(*) FROM app_revisions", [], |r| r.get(0)).unwrap();
        drop(conn);

        let g = Generated::open(&dir);
        assert!(g.note.is_none(), "{:?}", g.note);
        let (p, hash) = g.project_of(&id).unwrap().expect("古い app が Project として読める");
        let def = g.find(&id).unwrap().rt.revision().definition().clone();
        assert_eq!(p.pages.len(), 1, "Page 1 枚");
        assert_eq!(p.pages[0].app_ref.as_deref(), Some(id.as_str()));
        assert_eq!(p.pages[0].root, def.composition.root, "Page の木は app の木");
        assert_eq!((p.revision, p.app_refs.clone()), (1, vec![id.clone()]));
        assert_eq!(g.project_of(&id).unwrap().unwrap().1, hash, "包むたびに同じ hash");
        assert_eq!(g.find(&id).unwrap().rt.ui_state(&source).unwrap()["items"][0]["title"], json!("坊っちゃん"), "ui_state が残る");
        assert_eq!(g.run_entries().len(), 1, "run が残る");
        assert_eq!(g.rows().len(), 1, "Project を包んでも Apps の行は増えない");
        drop(g);
        let conn = rusqlite::Connection::open(&db).unwrap();
        let after: String = conn.query_row("SELECT json FROM apps WHERE id=?1", [&id], |r| r.get(0)).unwrap();
        assert_eq!(after, app_row, "apps の行を書き換えない");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM app_revisions", [], |r| r.get::<_, i64>(0)).unwrap(), app_revs);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "読むだけでは書かない");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// USAGE-1: 「7 日以内にまた使った」は最初の日より後の日・7 日以内だけ。負の対照: 同じ日だけ（b）・14 日後（c）は数えない。
    /// 開いた記録（opens）も使った日に入る（d は作った翌日に開いただけで yes）。Project の id は文に出ない（作った順の P1〜）。
    #[test]
    fn usage1_summary_counts_reuse_within_7_days() {
        let d = 86_400;
        let t0 = 20_000 * d + 3_600;
        let r = |p: &str, t: i64, o: &str| (p.to_owned(), t, o.to_owned());
        let revs = vec![
            r("project.a", t0, "generate"),
            r("project.a", t0 + 2 * d, "direct_edit"),
            r("project.a", t0 + 2 * d + 60, "nl_edit"),
            r("project.b", t0 + 60, "generate"),
            r("project.b", t0 + 120, "nl_edit"),
            r("project.b", t0 + 180, "undo"),
            r("project.c", t0 + 120, "generate"),
            r("project.c", t0 + 14 * d, "nl_edit"),
            r("project.d", t0 + 180, "generate"),
        ];
        let opens = vec![("project.d".to_owned(), t0 + d), ("project.b".to_owned(), t0 + 300)];
        let s = usage_summary(&revs, &opens, &[], t0 + 15 * d, 0, "v0 · macOS 14.5 · x86_64");
        for line in [
            "Shibubu usage report · v0 · macOS 14.5 · x86_64",
            "Projects 4 · used again within 7 days 2",
            "P1 · started 15 days ago · used on 2 days · opened on 0 days · sentence edits 1 · direct edits 1 · undos 0 · used again within 7 days: yes",
            "P2 · started 15 days ago · used on 1 day · opened on 1 day · sentence edits 1 · direct edits 0 · undos 1 · used again within 7 days: no",
            "P3 · started 15 days ago · used on 2 days · opened on 0 days · sentence edits 1 · direct edits 0 · undos 0 · used again within 7 days: no",
            "P4 · started 15 days ago · used on 2 days · opened on 1 day · sentence edits 0 · direct edits 0 · undos 0 · used again within 7 days: yes",
            "No project names, file paths, or anything you typed are in this report",
        ] {
            assert!(s.lines().any(|l| l == line), "行が無い: {line}\n{s}");
        }
        assert!(!s.contains("project."), "Project の id が出た: {s}");
        let empty = usage_summary(&[], &[], &[], t0, 0, "v0");
        assert!(empty.lines().any(|l| l == "Projects 0 · used again within 7 days 0"), "{empty}");
        // 日は利用者の地域で切る: 23:00 UTC と翌 08:00 UTC は JST（+9h）では同じ日（08:00 と 17:00）・UTC では 2 日。負の対照: offset を足さないと JST でも yes
        let late = 20_000 * d + 23 * 3_600;
        let tz = vec![r("project.z", late, "generate"), r("project.z", late + 9 * 3_600, "nl_edit")];
        let jst = usage_summary(&tz, &[], &[], late + d, 32_400, "v0");
        assert!(jst.contains("used on 1 day ·") && jst.contains("used again within 7 days: no"), "JST で同じ日なのに 2 日: {jst}");
        let utc = usage_summary(&tz, &[], &[], late + d, 0, "v0");
        assert!(utc.contains("used on 2 days ·") && utc.contains("used again within 7 days: yes"), "{utc}");
    }

    /// USAGE-1: 開くたびに data dir の `USAGE_FILE` へ 1 行（同じ Project・同じ日は 1 行）。report に Project の id・一言・data dir の path が出ない。
    /// memory の store（`start`）は書かない。負の対照: `note_open` の dedup を外すと 2 行で赤・`open_project` から `note_open` を外すと 0 行で赤。
    #[test]
    fn usage1_open_project_writes_one_line_per_day_and_report_hides_names() {
        let dir = w1_dir("usage");
        let mut g = Generated::open(&dir);
        let id = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, .. } = g.project_view_of(&id).unwrap() else { panic!() };
        g.open_project(&project, None).unwrap();
        g.open_project(&project, None).unwrap();
        let log = std::fs::read_to_string(dir.join(USAGE_FILE)).unwrap();
        // RQ1 から起動の行（`launch`）も同じ file に入る → 開いた行だけ数える
        assert_eq!(log.lines().filter(|l| l.contains("\"project\"")).count(), 1, "同じ日に 2 回開いて 1 行でない: {log}");
        assert!(log.contains(&project), "{log}");
        let s = g.usage_report();
        assert!(s.lines().any(|l| l.starts_with("Projects 1 ·")), "{s}");
        assert!(s.contains("opened on 1 day ·"), "{s}");
        for leak in [project.as_str(), id.as_str(), "読書", dir.to_str().unwrap()] {
            assert!(!s.contains(leak), "{leak} が出た: {s}");
        }
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = fresh();
        let id = m.create("読書記録をつけたい").unwrap();
        let View::Project { project, .. } = m.project_view_of(&id).unwrap() else { panic!() };
        m.open_project(&project, None).unwrap();
        assert!(m.usage_log.is_none(), "memory の store で開いた日を書く先を持った");
    }

    /// RQ1（N16）: 起動ごとに `launch` の 1 行・その起動で初めて Page の button の action が確かめて通った時だけ `action` の 1 行（2 回目と Agent の手は書かない）→
    /// usage report の 3 行目が `First action · …`。memory の store は書かない。負の対照: `Host::invoke` の `note_action` を外す → action の行が 0 で赤。
    #[test]
    fn rq1_launch_and_first_action_lines_feed_the_report() {
        let dir = w1_dir("rq1-usage");
        let mut g = Generated::open(&dir);
        let id = g.create("タイマーを作って").unwrap();
        let count = |kind: &str| std::fs::read_to_string(dir.join(USAGE_FILE)).unwrap_or_default().lines().filter(|l| l.contains(&format!("\"{kind}\":true"))).count();
        assert_eq!((count("launch"), count("action")), (1, 0));
        {
            let (def, _, mut h) = g.open_app(&id).unwrap();
            let (_, _, action) = timer_parts(&def);
            h.invoke(ActionInvocation { action_ref: action, origin: InvocationOrigin::Agent { suggestion_id: "s".into() }, input: json!({}) });
        }
        assert_eq!(count("action"), 0, "Agent の手を数えた");
        for _ in 0..2 {
            let (def, _, mut h) = g.open_app(&id).unwrap();
            let (_, key, action) = timer_parts(&def);
            h.invoke(ActionInvocation { action_ref: action, origin: InvocationOrigin::Button { node_key: key }, input: json!({}) });
            assert!(h.line.is_none(), "前提: button の action が Verified でない: {:?}", h.line);
        }
        assert_eq!(count("action"), 1, "最初の 1 回だけでない");
        let s = g.usage_report();
        let third = s.lines().nth(2).unwrap_or_default();
        assert!(third.starts_with("First action · ") && third.ends_with(" after the first launch"), "3 行目: {s}");
        drop(g);
        let g = Generated::open(&dir);
        assert_eq!(count("launch"), 2, "開き直して起動の行が足されない");
        assert!(g.usage_report().lines().nth(2).unwrap_or_default().ends_with("none yet after a relaunch (1 relaunch)"), "{}", g.usage_report());
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = fresh();
        let id = m.create("タイマーを作って").unwrap();
        {
            let (def, _, mut h) = m.open_app(&id).unwrap();
            let (_, key, action) = timer_parts(&def);
            h.invoke(ActionInvocation { action_ref: action, origin: InvocationOrigin::Button { node_key: key }, input: json!({}) });
        }
        assert!(m.usage_log.is_none());
    }

    /// RQ1: First action の 1 行（純関数）。記録が無い・action の無い初回・再起動の中央値（action の無い再起動は入れない）。負の対照: 中央値の sort を外す → 3 つ目が赤。
    #[test]
    fn rq1_usage_summary_first_action_line() {
        let line = |launches: &[(i64, Option<i64>)]| usage_summary(&[], &[], launches, 10_000, 0, "v0").lines().nth(2).unwrap().to_owned();
        assert_eq!(line(&[]), "First action · not recorded");
        assert_eq!(line(&[(100, None)]), "First action · none yet after the first launch");
        assert_eq!(
            line(&[(0, Some(42)), (1_000, Some(1_000 + 7_200)), (9_000, None), (20_000, Some(20_030)), (30_000, Some(30_000 + 180))]),
            "First action · 42 s after the first launch · median 3 min after a relaunch (4 relaunches)"
        );
        assert_eq!(line(&[(0, Some(5)), (50, None)]), "First action · 5 s after the first launch · none yet after a relaunch (1 relaunch)");
    }

    /// RQ1: 保存した上限は model が届く前に当てても後でも同じ（`set_llm` が包む時に使う）・使った数は 0 に戻さない・AI の行は 3 つの状態。
    /// 負の対照: `set_llm` の `Budget::resolve(self.budget_pref…)` を `Budget::from_env()` に戻す → 1 つ目が赤。
    #[test]
    fn rq1_budget_and_ai_rows_follow_the_model() {
        use crate::domain::capability::DisplayTier;
        struct Echo;
        impl crate::llm::Llm for Echo {
            fn label(&self) -> String {
                "claude-cli/2.1.0".into()
            }
            fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
                Ok(json!({}))
            }
        }
        let mut g = fresh();
        let row = |g: &Generated| g.ai_rows().into_iter().map(|c| (c.state, c.detail, c.checking)).collect::<Vec<_>>();
        assert_eq!(row(&g), [(DisplayTier::Available, "Checking for AI on this Mac".to_owned(), true)]);
        assert_eq!(g.budget_used(), None);
        g.set_budget(Some(0), None); // 先に保存の値 → 後から届いた model にも当たる
        g.set_llm(Ok(Box::new(Echo)));
        assert_eq!(row(&g), [(DisplayTier::Connected, "Claude Code 2.1.0 · signed in with your plan".to_owned(), false)]);
        let llm = g.llm.clone().unwrap();
        let req = || crate::llm::JsonRequest::new("t", "", "", json!({"type": "object"}));
        assert!(matches!(crate::llm::Llm::send(&*llm, &req()), Err(crate::llm::LlmError::OverBudget { .. })), "保存した 0 回が後から届いた model に当たらない");
        g.set_budget(Some(5), None);
        assert!(crate::llm::Llm::send(&*llm, &req()).is_ok());
        assert_eq!(g.budget_used().map(|u| u.0), Some(1));
        g.set_llm(Err(crate::llm::LlmError::NoCli { detail: "Claude Code isn't installed · see /Users/you/.local/bin or ask me@example.com".into() }));
        let gone = row(&g);
        assert_eq!(gone[0].0, DisplayTier::Unavailable);
        assert!(!gone[0].1.contains("/Users") && !gone[0].1.contains('@') && gone[0].1.contains("[path]"), "理由の行の path / email: {}", gone[0].1);
    }

    /// L4: 見つかった Grok / local LLM を使っている間、AI の行は Claude Code とも plan とも言わない（`grok · 1.0.70 · on this Mac`）。
    /// 負の対照: `ai_rows` の `strip_prefix("claude-cli/")` の None の腕を前の `unwrap_or(&label)` に戻す → `Claude Code grok/1.0.70 · signed in with your plan` で赤。
    #[test]
    fn l4_found_ai_row_does_not_claim_claude_or_a_plan() {
        struct Named(&'static str);
        impl crate::llm::Llm for Named {
            fn label(&self) -> String {
                self.0.into()
            }
            fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
                Ok(json!({}))
            }
        }
        for (label, want) in [("grok/1.0.70", "grok · 1.0.70 · on this Mac"), ("ollama/llama3", "ollama · llama3 · on this Mac"), ("claude-cli/2.1.0", "Claude Code 2.1.0 · signed in with your plan")] {
            let mut g = fresh();
            g.set_llm(Ok(Box::new(Named(label))));
            assert_eq!(g.ai_rows()[0].detail, want, "{label}");
        }
    }

    /// RQ1: About の行（= Copy diagnostics の文）は版・OS・Mac・AI・checks・数で、path・email・Project の名前・打った文を入れない。
    /// 負の対照: `diagnostics` の `mask_personal` を外す → check の detail の path で赤。
    #[test]
    fn rq1_diagnostics_hide_paths_names_and_typed_text() {
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        let check = crate::ui::connections::Connection {
            name: "Screen control".into(),
            state: crate::domain::capability::DisplayTier::Unavailable,
            detail: "Not found at /Users/you/Applications/Bridge.app · mail you@example.com".into(),
            recheck: true,
            checking: false,
            settings: None,
            key: "gui".into(),
            acts: vec![],
            checked_at: None,
        };
        let rows = g.diagnostics(&[check]);
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        for want in ["Shibubu", "macOS", "Mac", "AI", "AI calls this launch", "Screen control", "Projects", "Runs needing attention"] {
            assert!(keys.contains(&want), "{want} の行が無い: {keys:?}");
        }
        let all = rows.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
        for leak in ["/Users", "~/", "@", "読書", id.as_str()] {
            assert!(!all.contains(leak), "{leak} が出た: {all}");
        }
        assert!(all.contains("Projects: 1"), "{all}");
    }

    /// W11: V4 の DB（run_receipts が無い）を開くと V5 が足され、sessions / apps / terminal_sessions は残る。
    /// V5 は CREATE TABLE IF NOT EXISTS にしない（二重適用を隠さない）。
    #[test]
    fn w11_v4_db_gains_run_receipts_and_keeps_sessions_and_apps() {
        let dir = w1_dir("v4");
        let mut g = Generated::open(&dir);
        let id = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&id).unwrap() else { panic!() };
        g.open_project(&project, None).unwrap();
        g.page_intent(&project, PageIntent::AddTerminal { page }).unwrap();
        drop(g);
        let db = dir.join("kangae.sqlite3");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let apps: i64 = conn.query_row("SELECT COUNT(*) FROM apps", [], |r| r.get(0)).unwrap();
        let sessions: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        let terms: i64 = conn.query_row("SELECT COUNT(*) FROM terminal_sessions", [], |r| r.get(0)).unwrap();
        assert!(apps >= 1 && sessions >= 1 && terms >= 1, "apps={apps} sessions={sessions} terminal_sessions={terms}");
        conn.execute_batch("DROP TABLE run_receipts; DROP TABLE chat_turns; DELETE FROM schema_migrations WHERE version >= 5;").unwrap();
        drop(conn);

        let g = Generated::open(&dir);
        assert!(g.note.is_none(), "{:?}", g.note);
        drop(g);
        let conn = rusqlite::Connection::open(&db).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='run_receipts'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "V5 が run_receipts を足す");
        let version: i64 = conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 6);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM apps", [], |r| r.get::<_, i64>(0)).unwrap(), apps);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0)).unwrap(), sessions);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM terminal_sessions", [], |r| r.get::<_, i64>(0)).unwrap(), terms);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Done 3: AddPage を当てて閉じ、開き直すと同じ構成・同じ revision・同じ hash。revision は snapshot と ops と origin を持つ。
    #[test]
    fn w1_add_page_survives_reopen_with_same_revision_and_hash() {
        let dir = w1_dir("reopen");
        let mut g = Generated::open(&dir);
        let id = g.create("読書記録をつけたい").unwrap();
        let (p1, h1) = g.project_of(&id).unwrap().unwrap();
        let ops = add_page("tests");
        let (p2, h2) = g.apply_project(&id, &h1, &ops, RevisionOrigin::DirectEdit).expect("AddPage が当たる");
        assert_eq!(p2.revision, 2);
        assert_eq!(p2.pages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec![p1.pages[0].id.as_str(), "tests"]);
        assert_ne!(h1, h2);
        drop(g);

        let g = Generated::open(&dir);
        let (p, h) = g.project_of(&id).unwrap().expect("開き直した Project");
        assert_eq!(p, p2, "同じ構成");
        assert_eq!((p.revision, h.clone()), (2, h2.clone()), "同じ revision と hash");
        let revs = g.store.lock().unwrap().list_project_revisions(&p.id).unwrap();
        assert_eq!(revs.len(), 2);
        assert_eq!((revs[0].revision, revs[0].origin.as_str(), revs[0].base_hash.clone()), (1, "generate", None));
        assert_eq!(serde_json::from_value::<ProjectDefinitionV1>(revs[0].json.clone()).unwrap(), p1, "1 枚目は包んだ形の snapshot");
        assert_eq!((revs[1].revision, revs[1].origin.as_str(), revs[1].base_hash.as_deref()), (2, "direct_edit", Some(h1.as_str())));
        assert_eq!(revs[1].ops, serde_json::to_value(&ops).unwrap(), "差分の表示用に当てた ops");
        assert_eq!(serde_json::from_value::<ProjectDefinitionV1>(revs[1].json.clone()).unwrap(), p2, "全量の snapshot");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Done 4: 古い base_hash に向けた差分は拒否され、何も書かれない（後の変更を消さない）。理由と次の一手を 1 行で返す。
    #[test]
    fn w1_patch_on_stale_base_hash_is_rejected() {
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        let (_, h1) = g.project_of(&id).unwrap().unwrap();
        // 1 回目から古い hash: 包んだだけの Project にも照合が効き、何も書かない
        let err = g.apply_project(&id, "0000", &add_page("a"), RevisionOrigin::NlEdit).unwrap_err();
        assert!(err.contains("Reopen it, then apply your change again"), "{err}");
        assert_eq!(g.store.lock().unwrap().list(Table::Projects, None).unwrap().len(), 0, "拒否は何も書かない");
        let (p2, h2) = g.apply_project(&id, &h1, &add_page("a"), RevisionOrigin::DirectEdit).unwrap();
        // 別の所で a を足した後に、h1 を元にした b の差分が来る
        let err = g.apply_project(&id, &h1, &add_page("b"), RevisionOrigin::NlEdit).unwrap_err();
        assert!(err.contains("changed somewhere else") && err.contains("Reopen it"), "{err}");
        let (p, h) = g.project_of(&id).unwrap().unwrap();
        assert_eq!((p, h), (p2.clone(), h2.clone()), "後の変更（a）が残り、b は入らない");
        assert_eq!(g.store.lock().unwrap().list_project_revisions(&p2.id).unwrap().len(), 2);
        // 今の hash なら当たる
        assert_eq!(g.apply_project(&id, &h2, &add_page("b"), RevisionOrigin::NlEdit).unwrap().0.revision, 3);
    }

    /// W1 反証: 保存した Project の行が読めない時は「開いていない app」（Ok(None)）と混ぜず、理由の 1 行を返す。
    #[test]
    fn w1_unreadable_saved_project_is_an_error_not_a_closed_app() {
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        assert_eq!(g.project_of("no.such.app"), Ok(None), "開いていない app");
        let (p, _) = g.project_of(&id).unwrap().unwrap();
        let mut later = serde_json::to_value(&p).unwrap();
        later["layout_v2"] = json!({}); // 後の版が書いた行（deny_unknown_fields）
        g.store.lock().unwrap().put(Table::Projects, &p.id, None, &later).unwrap();
        let err = g.project_of(&id).unwrap_err();
        assert!(err.contains("This version can't open it"), "{err}");
    }

    /// W1 反証: Page の差分で app に無い action を付けると、保存の前に拒否され何も書かない（patch::apply と同じ照合）。
    #[test]
    fn w1_bind_to_missing_action_is_rejected_and_writes_nothing() {
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        let (p, h) = g.project_of(&id).unwrap().unwrap();
        let key = p.pages[0].root.nodes().iter().find(|n| n.action_ref.is_some()).expect("action を持つ node").key.clone();
        let bind = |a: &str| vec![ProjectPatchOp::Page {
            page_id: p.pages[0].id.clone(),
            patch: crate::domain::patch::AppPatch { op: crate::domain::patch::PatchOp::Bind, target_key: key.clone(), value: Some(json!({ "action_ref": a })) },
        }];
        let err = g.apply_project(&id, &h, &bind("no.such.action"), RevisionOrigin::DirectEdit).unwrap_err();
        assert!(err.contains("no.such.action"), "{err}");
        assert_eq!(g.store.lock().unwrap().list(Table::Projects, None).unwrap().len(), 0, "拒否は何も書かない");
        let real = g.find(&id).unwrap().rt.revision().definition().actions[0].id.clone();
        assert!(g.apply_project(&id, &h, &bind(&real), RevisionOrigin::DirectEdit).is_ok(), "在る action なら当たる");
    }

    #[test]
    fn path_walks_objects_and_arrays() {
        let doc = json!({ "selected": { "body": "x" }, "items": [{ "id": 1 }] });
        assert_eq!(at_path(&doc, "selected.body"), Some(&json!("x")));
        assert_eq!(at_path(&doc, "items.0.id"), Some(&json!(1)));
        assert_eq!(at_path(&doc, ""), Some(&doc));
        assert_eq!(at_path(&doc, "nope.x"), None);
    }

    // ---- W2: Project の画面（View::Project・PageRouter・Page ごとの入力途中の状態）----

    fn typed(g: &mut Generated, app: &str, key: &str) -> Option<String> {
        let (_, state, _) = g.open_app(app).unwrap();
        state.draft(key, None).and_then(|d| d.get("text")).and_then(Value::as_str).map(String::from)
    }
    fn type_in(g: &mut Generated, app: &str, key: &str, text: &str) {
        let (_, state, _) = g.open_app(app).unwrap();
        state.draft_mut(key, None).insert("text".into(), json!(text));
    }
    fn page_of(v: &View) -> String {
        match v {
            View::Project { page, .. } => page.clone(),
            other => panic!("Project の View ではない: {other:?}"),
        }
    }

    /// RQ3 interface（caller の形・本体の前は todo!() で赤 = 設計通り）: RQ-012「export → 別の data dir に import で同じ Project が開く」。書き出した file は
    /// `ProjectExport` として読め、鍵の形の文字と Terminal の panel を持たない・別の dir の Shibubu が開き、名前と Pages と面の中身が同じ・元の file は変わらない。
    /// 負の対照（本体の commit で取る）: `import_project` で `state` を写さない → 面の中身の assert が赤。
    #[test]
    fn rq3_export_then_import_in_another_data_dir_opens_the_same_project() {
        let (a, b) = (w1_dir("rq3-export-a"), w1_dir("rq3-export-b"));
        let mut g = Generated::open(&a);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        g.store.lock().unwrap().put_ui_state(&session_of(&app), &view_state_key(&page), &json!({ "scroll": 3 })).unwrap();
        let out = a.join("out");
        let file = g.export_project(&project, &out).unwrap();
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some(PROJECT_FILE_EXT));
        let text = std::fs::read_to_string(&file).unwrap();
        let bundle: ProjectExport = serde_json::from_str(&text).unwrap();
        assert_eq!(bundle.schema, PROJECT_EXPORT_SCHEMA);
        assert!(!bundle.state.contains_key(PANEL_KEY) && !text.contains("sk-ant-"), "書き出しに panel か鍵の形が入った");
        let mut other = Generated::open(&b);
        let View::Project { project: got, page: got_page } = other.import_project(&file).unwrap() else { panic!() };
        let got_app = other.app_of_project(&got).expect("取り込んだ Project が開いていない");
        assert_eq!(other.title(&got_app), g.title(&app), "名前が違う");
        assert_eq!(other.store.lock().unwrap().get_ui_state(&session_of(&got_app), &view_state_key(&got_page)).unwrap(), Some(json!({ "scroll": 3 })), "面の中身を写していない");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "元の file を変えた");
    }

    /// RQ3: Data & Recovery の読み（裏の thread）が届くまで待つ。
    fn read_summary(g: &mut Generated, look_in: &[std::path::PathBuf]) -> DataSummary {
        let now = unix_now();
        g.refresh_data(look_in, now, true);
        for _ in 0..500 {
            g.refresh_data(look_in, now, false);
            if g.data_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        g.data_summary().cloned().expect("Data & Recovery を読めていない")
    }
    fn backup_sets(dir: &Path, why: &str) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir.join(BACKUPS_DIR)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(why)).collect();
        v.sort();
        v
    }

    /// RQ3（RQ-011・FX-003）: Back up now → DB を壊して開き直す → 空の（保存する）store で開いて Home に `DAMAGED_NOTE`・壊れた file は `damaged` の世代に残る →
    /// Data & Recovery の list に両方 → Restore は予約だけ（押しただけでは変えない）→ 開き直すと Project と設定が戻り、戻す前の data は `before-restore`・印は消える。
    /// 負の対照（commit message）: `open` の ①（`restore_on_open`）を外す → 開き直しても Project が戻らず赤。
    #[test]
    fn rq3_back_up_then_damage_then_restore_on_reopen_brings_the_project_back() {
        let dir = w1_dir("rq3-restore");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, .. } = g.project_view_of(&app).unwrap() else { panic!() };
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"marker":"before"}"#).unwrap();
        let id = g.back_up_now().unwrap();
        assert!(id.ends_with("-manual") && dir.join(BACKUPS_DIR).join(&id).join(DB_FILES[0]).is_file(), "世代が無い: {id}");
        drop(g);
        for n in DB_FILES {
            let _ = std::fs::remove_file(dir.join(n));
        }
        let junk = b"not a database, only words that fill the first page of the file. ".repeat(64);
        std::fs::write(dir.join(DB_FILES[0]), &junk).unwrap();
        let mut g = Generated::open(&dir);
        assert_eq!(g.note.as_deref(), Some(DAMAGED_NOTE));
        assert!(g.app_of_project(&project).is_none() && !g.memory_fallback, "空の保存する store で開いていない");
        let damaged = backup_sets(&dir, "-damaged");
        assert_eq!(damaged.len(), 1, "壊れた DB を隔離していない");
        assert_eq!(std::fs::read(dir.join(BACKUPS_DIR).join(&damaged[0]).join(DB_FILES[0])).unwrap(), junk, "壊れた file を残していない");
        let d = read_summary(&mut g, &[]);
        assert!(d.backups.iter().any(|(b, l)| *b == id && l.contains("Backed up by you")) && d.backups.iter().any(|(b, l)| *b == damaged[0] && l.contains("Damaged")), "{:?}", d.backups);
        assert_eq!(g.restore_backup(Some("../elsewhere")), Err("That backup isn't there anymore".into()));
        assert_eq!(g.restore_backup(Some(&id)), Ok("Restores when Shibubu reopens · Quit and reopen Shibubu".into()));
        assert_eq!(read_summary(&mut g, &[]).restoring.as_deref(), Some(id.as_str()), "予約が list に出ない");
        assert!(g.app_of_project(&project).is_none(), "押しただけで戻した");
        drop(g);
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"marker":"after"}"#).unwrap();
        let g = Generated::open(&dir);
        assert!(g.app_of_project(&project).is_some(), "Project が戻らない: {:?}", g.note);
        assert!(g.note.as_deref().is_some_and(|n| n.starts_with("Restored your backup")), "{:?}", g.note);
        assert_eq!(std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap(), r#"{"marker":"before"}"#, "設定を戻していない");
        assert!(!dir.join(RESTORE_MARKER).exists(), "予約の印が残った");
        let kept = backup_sets(&dir, "-before-restore");
        assert!(kept.len() == 1 && dir.join(BACKUPS_DIR).join(&kept[0]).join(SETTINGS_FILE).is_file(), "戻す前の data を残していない: {kept:?}");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ3（RQ-013）: Delete my data は種類ごとに消して読み戻す。History は記録の file だけ（Project と画面の記録は残る）・ScreenRecords は画面の記録だけ・
    /// Backups は世代だけ・Consents は同意だけ・Projects は DB の行（開き直しても戻らない）。
    /// 負の対照（commit message）: History の腕の `HISTORY_FILES` の削除を外す → engine-runs が残って赤。
    #[test]
    fn rq3_delete_my_data_removes_only_that_kind_and_reads_back() {
        use crate::ui::shell::DataKind;
        let dir = w1_dir("rq3-delete");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, .. } = g.project_view_of(&app).unwrap() else { panic!() };
        for f in HISTORY_FILES.iter().chain(SCREEN_FILES.iter()) {
            std::fs::write(dir.join(f), "{\"at\":\"2026-10-28T00:00:00Z\"}\n").unwrap();
        }
        let consent = crate::surface::ACT_CONSENT_FILE;
        std::fs::write(dir.join(consent), "{}\n").unwrap();
        g.back_up_now().unwrap();
        let there = |n: &str| dir.join(n).exists();
        assert_eq!(g.delete_my_data(DataKind::History), Ok("Deleted run history and AI calls".into()));
        assert!(HISTORY_FILES.iter().all(|f| !there(f)), "History の記録が残った");
        assert!(SCREEN_FILES.iter().all(|f| there(f)) && g.app_of_project(&project).is_some(), "History の外に触れた");
        g.delete_my_data(DataKind::ScreenRecords).unwrap();
        assert!(SCREEN_FILES.iter().all(|f| !there(f)) && there(consent) && there(BACKUPS_DIR));
        g.delete_my_data(DataKind::Backups).unwrap();
        assert!(!there(BACKUPS_DIR) && there(consent));
        g.delete_my_data(DataKind::Consents).unwrap();
        assert!(!there(consent));
        assert_eq!(g.delete_my_data(DataKind::Projects), Ok("Deleted projects and pages · your folders and files stay".into()));
        {
            let st = g.store.lock().unwrap();
            assert_eq!((st.list(Table::Apps, None).unwrap().len(), st.list(Table::Projects, None).unwrap().len()), (0, 0), "DB の行が残った");
        }
        assert!(g.rows().is_empty() && g.app_of_project(&project).is_none());
        drop(g);
        assert!(Generated::open(&dir).rows().is_empty(), "開き直すと Project が戻った");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ3（RQ-014）: 保存期間を過ぎた行だけ消す（History = `history_days`・既定 30 日・画面の記録 = 7 日・日付の読めない行は残す）・世代は種類ごとの数まで
    /// 新しい順に残す（`damaged` は残す）。負の対照（commit message）: `prune_jsonl` の日付の filter を外す → 10 日前の行が残って赤。
    #[test]
    fn rq3_prune_records_keeps_recent_lines_and_the_newest_backups() {
        let dir = w1_dir("rq3-prune");
        let g = Generated::open(&dir);
        let now = unix_now();
        let at = |days: i64| crate::agent::rfc3339_from_secs(now - days * 86_400);
        let runs = dir.join(HISTORY_FILES[0]);
        std::fs::write(&runs, format!("{{\"at\":\"{}\"}}\n{{\"at\":\"{}\"}}\nnot json\n", at(10), at(1))).unwrap();
        let screen = dir.join(SCREEN_FILES[0]);
        std::fs::write(&screen, format!("{{\"at_ms\":{}}}\n{{\"at_ms\":{}}}\n", (now - 8 * 86_400) * 1000, now * 1000)).unwrap();
        for d in 0..9 {
            std::fs::create_dir_all(dir.join(BACKUPS_DIR).join(format!("{}-daily", stamp(now - d * 86_400)))).unwrap();
        }
        std::fs::create_dir_all(dir.join(BACKUPS_DIR).join(format!("{}-damaged", stamp(now - 400 * 86_400)))).unwrap();
        // RQ3 反証の直し（generated.rs:678）: 時刻の欄の無い行（窓の題名を持つ Native Direct の記録・前の版の trace）は file を書いた時刻で見る
        // = 8 日前に書いたきりの画面の記録は 7 日で消え、今書いた trace は残る。負の対照（commit message）: prune_jsonl の `.or(written)` を外す → 題名の行が残って赤
        let ident = dir.join(SCREEN_FILES[3]);
        std::fs::write(&ident, "{\"title\":\"Budget 2026.xlsx\"}\n").unwrap();
        std::fs::File::options().write(true).open(&ident).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs((now - 8 * 86_400) as u64)).unwrap();
        let trace = dir.join(HISTORY_FILES[1]);
        std::fs::write(&trace, "{\"stage\":\"ask\"}\n").unwrap();
        g.prune_records(now, None).unwrap();
        assert_eq!(std::fs::read_to_string(&runs).unwrap().lines().count(), 3, "30 日の内の行を消した");
        assert_eq!(std::fs::read_to_string(&screen).unwrap().lines().count(), 1, "画面の記録の 7 日を越えた行が残った");
        assert!(!std::fs::read_to_string(&ident).unwrap().contains("Budget"), "時刻の無い 8 日前の画面の記録が残った");
        g.prune_records(now, Some(7)).unwrap();
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), format!("{{\"at\":\"{}\"}}\nnot json\n", at(1)), "7 日を越えた行が残った・読めない行を消した");
        assert_eq!(std::fs::read_to_string(&trace).unwrap(), "{\"stage\":\"ask\"}\n", "今書いた時刻の無い行を消した");
        let daily = backup_sets(&dir, "-daily");
        assert_eq!(daily.len(), 7, "daily の数");
        assert!(!daily.contains(&format!("{}-daily", stamp(now - 8 * 86_400))) && backup_sets(&dir, "-damaged").len() == 1, "古い方を残した・damaged を消した");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ3（RQ-012・D-e）: 読めない file は理由の 1 行で断り、何も足さない（新しい版の file・`.shibubu` でない file）。
    #[test]
    fn rq3_import_says_why_it_cannot_read_a_file() {
        let dir = w1_dir("rq3-import-bad");
        let mut g = Generated::open(&dir);
        let f = dir.join("x.shibubu");
        std::fs::write(&f, r#"{"schema":"shibubu.project/2"}"#).unwrap();
        assert_eq!(g.import_project(&f), Err("This file was made by a newer Shibubu · Update Shibubu first".into()));
        std::fs::write(&f, "{}").unwrap();
        assert_eq!(g.import_project(&f), Err("This isn't a Shibubu project file".into()));
        assert!(g.rows().is_empty(), "断った file から Project を足した");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ3 反証の直し（holdout h3・反証 generated.rs:3369 の 2 件）: 書き出しは接続の参照を名前と種類だけ（Engine が URL を渡しても書かない）・Terminal の席の
    /// shell と作業 folder（この Mac の path）を書かない。取り込みは file の shell と folder で起こさず、この Mac の利用者の shell と Project の folder（無ければ HOME）・
    /// 検査に落ちた file は英語の 1 行（内部の語を出さない）。負の対照（commit message）: export の `c.uri = None` を外す → token が file に残って赤。
    #[test]
    fn rq3_export_leaves_out_urls_and_paths_and_import_starts_its_own_shell() {
        struct Leaky;
        impl PageEngine for Leaky {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                EngineAnswer { reply: EngineReply::Failed { line: "stopped".into() }, log: ("error", json!({})) }
            }
            fn export_workflows(&self, _: &str) -> (Vec<Value>, Vec<project::ProjectResource>) {
                (Vec::new(), vec![project::ProjectResource { id: "github".into(), kind: "api".into(), uri: Some("https://api.github.com/?token=SECRET-RQ3".into()) }])
            }
        }
        let (a, b) = (w1_dir("rq3-clean-a"), w1_dir("rq3-clean-b"));
        let ws = a.join("repo");
        let mut g = Generated::open(&a);
        let View::Project { project, .. } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        assert!(!g.project(&project).unwrap().1.terminal_specs.is_empty(), "前提: 主デモの Project は Terminal の席を持つ");
        g.set_engine(EngineSetup { engine: Arc::new(Leaky), data_dir: a.clone() });
        let file = g.export_project(&project, &a.join("out")).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("SECRET-RQ3") && !text.contains("api.github.com"), "接続の URL（鍵の入った query）を書いた");
        let bundle: ProjectExport = serde_json::from_str(&text).unwrap();
        assert_eq!(bundle.connections.iter().map(|c| (c.id.as_str(), c.kind.as_str(), c.uri.is_none())).collect::<Vec<_>>(), vec![("github", "api", true)]);
        assert!(bundle.project.terminal_specs.iter().all(|t| t.shell.is_empty() && t.working_directory.is_empty()), "この Mac の shell か folder を書いた: {:?}", bundle.project.terminal_specs);
        // 他人が書き換えた file: 知らない実行 file と folder
        let mut evil: Value = serde_json::from_str(&text).unwrap();
        for t in evil["project"]["terminal_specs"].as_array_mut().unwrap() {
            t["shell"] = json!("/tmp/rq3-not-a-shell");
            t["working_directory"] = json!("/nonexistent/rq3");
            // L7 の反証: 起動の 1 行（`command`）を持ち込むと Page を開いただけで shell に打たれる
            t["command"] = json!("echo rq3-evil");
        }
        let f2 = a.join("out").join("changed.shibubu");
        std::fs::write(&f2, evil.to_string()).unwrap();
        let mut other = Generated::open(&b);
        let View::Project { project: got, .. } = other.import_project(&f2).unwrap() else { panic!() };
        let (_, np, _) = other.project(&got).unwrap();
        assert!(!np.terminal_specs.is_empty() && np.terminal_specs.iter().all(|t| t.shell == user_shell() && t.working_directory == g.terminal_cwd(&np)), "file の shell か folder で起こす: {:?}", np.terminal_specs);
        assert!(np.terminal_specs.iter().all(|t| t.command.is_none()), "file の起動の 1 行を持ち込んだ: {:?}", np.terminal_specs);
        let said = other.opened_line.clone().map(|(p, (s, l))| (p, s, l));
        assert_eq!(said, Some((got.clone(), RegionState::Partial, "Terminal start commands in the file weren't imported or run".to_owned())), "捨てた起動の 1 行を黙った");
        evil["project"]["pages"] = json!([]);
        std::fs::write(&f2, evil.to_string()).unwrap();
        assert_eq!(other.import_project(&f2), Err("This project file is damaged or incomplete · Export it again".into()), "検査の内部の語を画面に出した");
        drop((g, other));
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// RQ3 反証の直し（generated.rs:3480 の 2 件）: Restore を予約した世代は保存期間で消さない・Delete my data › Backups は予約の印も消す・世代の無い印は
    /// `restoring` に出さない（`Restores when Shibubu reopens · <id>` と Cancel を出し続けない）。
    /// 負の対照（commit message）: Backups の腕の `RESTORE_MARKER` の削除を外す → 印が残って赤。
    #[test]
    fn rq3_a_reserved_restore_goes_with_its_backup_and_pruning_keeps_it() {
        use crate::ui::shell::DataKind;
        let dir = w1_dir("rq3-marker");
        let mut g = Generated::open(&dir);
        g.create("読書記録を作って").unwrap();
        let now = unix_now();
        for d in 0..8 {
            std::fs::create_dir_all(dir.join(BACKUPS_DIR).join(format!("{}-daily", stamp(now - d * 86_400)))).unwrap();
        }
        let old = format!("{}-daily", stamp(now - 30 * 86_400));
        std::fs::create_dir_all(dir.join(BACKUPS_DIR).join(&old)).unwrap();
        std::fs::write(dir.join(BACKUPS_DIR).join(&old).join(SETTINGS_FILE), "{}").unwrap();
        assert!(g.restore_backup(Some(&old)).is_ok());
        g.prune_records(now, None).unwrap();
        assert!(dir.join(BACKUPS_DIR).join(&old).is_dir(), "予約した世代を保存期間で消した");
        assert_eq!(read_summary(&mut g, &[]).restoring.as_deref(), Some(old.as_str()));
        g.delete_my_data(DataKind::Backups).unwrap();
        assert!(!dir.join(RESTORE_MARKER).exists(), "世代を消したのに予約の印が残った");
        assert_eq!(read_summary(&mut g, &[]).restoring, None);
        // 印だけ残った data dir（この直しの前の版）でも、世代の無い予約は出さない
        std::fs::write(dir.join(RESTORE_MARKER), format!("{old}\n")).unwrap();
        assert_eq!(read_summary(&mut g, &[]).restoring, None, "世代の無い予約を出した");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ3 反証の直し（`delete_my_data` の Consents の腕）: `Apps you allowed` の削除は Mirror の同意の門（起動中の memory の写し）も捨てる = 次に描く時に DB の空の同意から作り直す
    /// （前は再起動まで聞き直さずに窓へ付いた）。負の対照（commit message）: Consents の腕の `self.mirror = None` を外す → 門が残って赤。
    #[cfg(feature = "mirror")]
    #[test]
    fn rq3_deleting_consents_drops_the_mirror_gate() {
        use crate::ui::shell::DataKind;
        let dir = w1_dir("rq3-consents-mirror");
        let mut g = Generated::open(&dir);
        g.mirror = Some(g.mirror_pages(dir.join("no-helper"), None));
        g.delete_my_data(DataKind::Consents).unwrap();
        assert!(g.mirror.is_none(), "同意を消したのに起動中の Mirror の門が残った");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ2（RQ-005）: 名前替え・複製・削除を DB から読み戻す。名前替えは revision 1 つ（direct_edit）で Undo で戻る・複製は元を変えず面の中身（ui_state）を写す・
    /// run が動いている間は消さない・消すと projects / apps / revision / ui_state の行が無く、他の Project は残り、再起動しても戻らない。
    /// 負の対照（commit message）: `delete_project` の `store.delete(Table::Apps, ..)` を外す → apps の行が残って赤。
    /// RQ5（RQ-020・021）: agent が読む app は今の Page の木で版が Project の revision・提案の変更は 1 revision として当たり（⌘Z で戻り・やり直しで進み・再起動で読み戻る）、
    /// 当たらない提案（同じ key の Insert）は何も書かない。負の対照: `apply_suggestion` を `RevisionOrigin` 付きの 2 回の `apply_project` に分ける → revision が +2 で赤。
    #[test]
    fn rq5_a_suggestion_applies_as_one_undoable_revision_and_reads_back() {
        use crate::domain::patch::{AppPatch, PatchOp};
        let dir = w1_dir("rq5-suggest");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let (rev, page_id) = g.agent_app(&project).expect("今の Page の app が読めない");
        let (_, p0, h0) = g.project(&project).unwrap();
        assert_eq!((page_id.as_str(), u64::from(rev.version())), (page.as_str(), p0.revision), "Page と版が Project のものでない");
        let root = p0.page(&page).unwrap().root.key.clone();
        let note = AppPatch { op: PatchOp::Insert, target_key: root, value: Some(json!({ "key": "rq5.note", "component": "text", "props": { "text": "Added by a suggestion", "style": "helper" } })) };
        g.apply_suggestion(&project, &page, &[note.clone()]).unwrap();
        let (_, p1, h1) = g.project(&project).unwrap();
        assert_eq!(p1.revision, p0.revision + 1, "1 回の提案が 1 revision でない");
        assert!(p1.page(&page).unwrap().root.find("rq5.note").is_some(), "提案の node が Page に無い");
        assert!(g.apply_suggestion(&project, &page, &[note]).is_err(), "同じ key の Insert が通った");
        assert_eq!(g.project(&project).unwrap().2, h1, "当たらない提案が Project を書いた");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(g.project(&project).unwrap().2, h0, "Undo で提案の前に戻らない");
        g.page_intent(&project, PageIntent::Redo).unwrap();
        drop(g);
        let g = Generated::open(&dir);
        assert!(g.project(&project).unwrap().1.page(&page).unwrap().root.find("rq5.note").is_some(), "再起動の後に提案の node が読み戻らない");
    }

    /// RQ6（RQ-022）: 保存した Mirror の同意が Privacy の一覧に出て、取り消すと消える（他の app の同意は残る）。無い app は false。
    /// 負の対照: `revoke_mirror_consent` の取り消しの印を書く put を外す → 一覧に残って赤。
    #[test]
    fn rq6_mirror_consents_are_listed_and_revoked_one_at_a_time() {
        let dir = w1_dir("rq6-mirror-consent");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        {
            let store = g.store.lock().unwrap();
            store.put(Table::Sessions, "mirror-consent", Some(app.as_str()), &json!({ "app_id": app })).unwrap();
            for b in ["com.apple.TextEdit", "com.hnc.Discord"] {
                store.put_ui_state("mirror-consent", b, &json!({ "v": 1, "bundle_id": b, "granted_at_ms": 1.0 })).unwrap();
            }
        }
        assert_eq!(g.mirror_consents(), ["com.apple.TextEdit", "com.hnc.Discord"]);
        assert_eq!(g.revoke_mirror_consent("com.apple.TextEdit"), Ok(true));
        assert_eq!(g.mirror_consents(), ["com.hnc.Discord"], "取り消した app が残った / 他の app まで消えた");
        assert_eq!(g.revoke_mirror_consent("com.apple.TextEdit"), Ok(false));
    }

    /// RQ5（反証 rq3 の直し・operator 10-29）: Terminal の Page も Browser（面）の Page も app の検査に通り、提案を読める（a7d6623 では読めていた物を止めない）。
    /// 通らない Page だけが Err の 1 行（`Suggestions can't read this page yet`）を右の列に出す（面 / Terminal だからという前置きの門は実測で外れたので置かない）。
    /// 負の対照: `agent_app` に面 / Terminal の node で Err を返す前置きの門を戻す → 赤。
    #[test]
    fn rq5_pages_with_a_terminal_or_a_browser_are_still_read() {
        let dir = w1_dir("rq5-live");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        assert!(g.agent_app(&project).is_ok(), "普通の Page で提案を読めない");
        g.page_intent(&project, PageIntent::AddTerminal { page: page.clone() }).unwrap();
        assert!(g.agent_app(&project).is_ok(), "Terminal の Page の提案が読めなくなった（a7d6623 では読めていた）");
        g.page_intent(&project, PageIntent::AddBrowser { page }).unwrap();
        assert!(g.agent_app(&project).is_ok(), "面（Browser）の Page の提案が読めない");
    }

    #[test]
    fn rq2_rename_duplicate_and_delete_read_back_from_the_db() {
        let dir = w1_dir("rq2-manage");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        assert_eq!(g.rename_project(&project, "  "), Err("A project needs a name".into()));
        let before = g.title(&app).unwrap();
        g.rename_project(&project, "  Reading log ").unwrap();
        assert_eq!(g.title(&app).as_deref(), Some("Reading log"));
        assert!(g.rows().iter().any(|r| r.name == "Reading log"), "Home の行が新しい名前でない");
        assert_eq!(g.store.lock().unwrap().list_project_revisions(&project).unwrap().last().map(|r| r.origin.clone()).as_deref(), Some("direct_edit"));
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(g.title(&app), Some(before), "Undo で前の名前に戻らない");
        g.page_intent(&project, PageIntent::Redo).unwrap();
        g.store.lock().unwrap().put_ui_state(&session_of(&app), &view_state_key(&page), &json!({ "scroll": 3 })).unwrap();
        let View::Project { project: copy, page: copy_page } = g.duplicate_project(&project).unwrap() else { panic!() };
        let copy_app = g.app_of_project(&copy).expect("複製が開いていない");
        assert_eq!((copy_app.as_str(), g.title(&copy_app).as_deref()), (format!("{app}-copy").as_str(), Some("Reading log copy")));
        assert_eq!(g.store.lock().unwrap().get_ui_state(&session_of(&copy_app), &view_state_key(&copy_page)).unwrap(), Some(json!({ "scroll": 3 })), "面の中身を写していない");
        assert_eq!(g.title(&app).as_deref(), Some("Reading log"), "元を変えた");
        let View::Project { project: copy2, .. } = g.duplicate_project(&project).unwrap() else { panic!() };
        assert_eq!(copy2, format!("project.{app}-copy-2"));
        // 反証の直し: 消した複製の id は、その web の store を次の起動で消すまで次の複製に使わない（使うと次の起動で生きた複製の store が消える）
        g.delete_project(&copy2).unwrap();
        let View::Project { project: copy3, .. } = g.duplicate_project(&project).unwrap() else { panic!() };
        assert_eq!(copy3, format!("project.{app}-copy-3"), "store を消す予定の id を使い直した");
        g.runs.lock().unwrap().insert(project.clone(), RunState::Running { command: "make".into(), started_at: "2026-10-28T00:00:00Z".into() });
        assert_eq!(g.delete_project(&project), Err("Stop the run first".into()));
        g.runs.lock().unwrap().remove(&project);
        // 反証の直し: Terminal と run の記録（FK の無い 2 つの表）は消す Project の行だけ消え、複製の行は残る
        {
            let st = g.store.lock().unwrap();
            for p in [&project, &copy] {
                st.put(Table::TerminalSessions, &format!("ts-{p}"), Some(p.as_str()), &json!({})).unwrap();
                st.put_run_receipt(&format!("run-{p}"), p, "dev", &json!({})).unwrap();
            }
        }
        // 反証の直し: 別の writer が書く lock を持つ間は消せず（Try again）、Project も Terminal も web の store の予定も残る
        let queue = dir.join(crate::project::web::FORGET_STORES);
        {
            let other = rusqlite::Connection::open(dir.join("kangae.sqlite3")).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert!(g.delete_project(&project).is_err(), "書く lock が取れないのに消えた");
            other.execute_batch("ROLLBACK").unwrap();
        }
        assert!(g.app_of_project(&project).is_some() && !g.web.queued_for_removal(&project), "消せなかった Project の web の store を消す予定に入れた");
        g.delete_project(&project).unwrap();
        assert!(std::fs::read_to_string(&queue).unwrap_or_default().lines().any(|l| l == project), "消した Project の web の store を消す予定に入れていない");
        {
            let st = g.store.lock().unwrap();
            assert!(st.get(Table::Projects, &project).unwrap().is_none() && st.get(Table::Apps, &app).unwrap().is_none());
            assert!(st.list_project_revisions(&project).unwrap().is_empty(), "revision が残った");
            assert_eq!(st.get_ui_state(&session_of(&app), &view_state_key(&page)).unwrap(), None, "ui_state が残った");
            assert!(st.list(Table::TerminalSessions, Some(project.as_str())).unwrap().is_empty() && st.list_run_receipts(&project, None).unwrap().is_empty(), "Terminal か run の記録が残った");
            assert_eq!((st.list(Table::TerminalSessions, Some(copy.as_str())).unwrap().len(), st.list_run_receipts(&copy, None).unwrap().len()), (1, 1), "複製の記録まで消した");
        }
        assert!(g.app_of_project(&project).is_none() && g.app_of_project(&copy).is_some(), "他の Project まで消した");
        drop(g);
        let g = Generated::open(&dir);
        assert!(g.app_of_project(&project).is_none() && g.app_of_project(&copy).is_some(), "再起動で戻った");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RQ2（RQ-006）反証の直し: `n needs you` の数 = Engine の承認待ちの Project だけ 1・他の Project は 0（Home の行・`⌄` の list・header の合計が読む `rows()[i].needs`）。
    /// 負の対照（commit message）: `needs_of` の engine の項を外す → 1 にならず赤。
    #[test]
    fn rq2_needs_counts_the_project_waiting_for_approval_only() {
        use crate::ui::engine_view::EngineView;
        let dir = w1_dir("rq2-needs");
        let mut g = Generated::open(&dir);
        let (a, b) = (g.create("読書記録を作って").unwrap(), g.create("タイマーを作って").unwrap());
        assert!(g.rows().iter().all(|r| r.needs == 0), "何も待っていないのに needs");
        g.engine_views.insert(project_id_of(&a), EngineView::Approve { goal: "send".into(), steps: vec![], line: None });
        let needs = |id: &str| g.rows().iter().find(|r| r.view == View::App(id.into())).map(|r| r.needs);
        assert_eq!((needs(&a), needs(&b)), (Some(1), Some(0)), "承認待ちの Project だけ 1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W2 の通し（試験・画面なし）: 一言 → Project の先頭 Page → ＋Page で 3 枚 → A → B → C → 戻る を 3 回繰り返しても、
    /// 入力途中の文字が Page ごとに元どおり（別の Page の文字が出ない）。再起動しても 3 枚が同じ順・同じ id で並び、A の文字も戻る。
    #[test]
    fn w2_three_pages_keep_their_own_drafts_and_survive_restart() {
        let dir = w1_dir("w2-pages");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        assert_eq!(page_of(&g.open_project(&project, Some(&a)).unwrap()), a);
        assert_eq!(g.project(&project).unwrap().1.pages.len(), 1, "一言では Page 1 枚");
        type_in(&mut g, &app, "a-note", "坊っちゃ");
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        assert_eq!(typed(&mut g, &app, "a-note"), None, "B に A の文字を出さない");
        type_in(&mut g, &app, "b-note", "B のメモ");
        let c = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        assert_eq!((typed(&mut g, &app, "a-note"), typed(&mut g, &app, "b-note")), (None, None));
        type_in(&mut g, &app, "c-note", "C");
        let ids: Vec<String> = g.project(&project).unwrap().1.pages.iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, [a.clone(), b.clone(), c.clone()]);
        let mine = |g: &mut Generated| [typed(g, &app, "a-note"), typed(g, &app, "b-note"), typed(g, &app, "c-note")];
        let only = |k: usize, s: &str| {
            let mut v = [None, None, None];
            v[k] = Some(s.to_owned());
            v
        };
        for round in 0..3 {
            assert_eq!(page_of(&g.navigate_page(&project, &a).unwrap()), a);
            assert_eq!(mine(&mut g), only(0, "坊っちゃ"), "round {round}: A");
            assert_eq!(page_of(&g.navigate_page(&project, &b).unwrap()), b);
            assert_eq!(mine(&mut g), only(1, "B のメモ"), "round {round}: B");
            assert_eq!(page_of(&g.navigate_page(&project, &c).unwrap()), c);
            assert_eq!(mine(&mut g), only(2, "C"), "round {round}: C");
            assert_eq!(page_of(&g.page_intent(&project, PageIntent::Back).unwrap()), b, "round {round}: 戻る");
            assert_eq!(mine(&mut g), only(1, "B のメモ"), "round {round}: 戻った B");
        }
        assert!(g.page_line.is_none(), "{:?}", g.page_line);
        assert_eq!(page_of(&g.navigate_page(&project, &a).unwrap()), a);
        drop(g);
        let mut g = Generated::open(&dir);
        let v = g.project_view_of(&app).unwrap();
        assert_eq!(v, View::Project { project: project.clone(), page: a.clone() }, "古い View::App は先頭 Page へ");
        assert_eq!(page_of(&g.open_project(&project, Some(&a)).unwrap()), a);
        let p = g.project(&project).unwrap().1;
        assert_eq!(p.pages.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), ids, "再起動しても 3 枚が同じ順");
        assert_eq!(p.pages.iter().map(|p| p.title.as_str()).skip(1).collect::<Vec<_>>(), ["Page 2", "Page 3"]);
        assert_eq!(mine(&mut g), only(0, "坊っちゃ"), "再起動しても A の文字");
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!(revs.iter().map(|r| r.origin.as_str()).collect::<Vec<_>>(), ["generate", "direct_edit", "direct_edit"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W2 Done 3: 最後の 1 枚は消せず、理由の 1 行を Page の上に残し、何も書かない。2 枚なら消せて次（無ければ前）の Page へ移る。名前替えは id と route を変えない。
    #[test]
    fn w2_last_page_is_kept_with_a_reason_and_delete_lands_next() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, None).unwrap();
        let err = g.page_intent(&project, PageIntent::Delete { page: a.clone() }).unwrap_err();
        assert!(err.contains("最後の Page") && err.contains("消せません"), "{err}");
        assert!(!err.contains(&a), "Page の id を画面に出さない: {err}");
        assert_eq!(g.page_line.clone(), Some((RegionState::Error, err.clone())), "draw_page が Page の上に出す 1 行");
        assert_eq!(g.store.lock().unwrap().list(Table::Projects, None).unwrap().len(), 0, "拒否は何も書かない");
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        assert_eq!(g.page_line.clone(), Some((RegionState::Ready, "Added page “Page 2” · ⌘Z to undo".into())), "通った操作で前の拒否の 1 行を消す（FX3: 何が変わったかの 1 行に）");
        let route = g.project(&project).unwrap().1.page(&b).unwrap().route.clone();
        g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "Tests".into() }).unwrap();
        let renamed = g.project(&project).unwrap().1.page(&b).unwrap().clone();
        assert_eq!((renamed.title.as_str(), renamed.route.as_str()), ("Tests", route.as_str()));
        assert_eq!(page_of(&g.page_intent(&project, PageIntent::Delete { page: b.clone() }).unwrap()), a, "次が無ければ前へ");
        assert_eq!(g.project(&project).unwrap().1.pages.len(), 1);
        assert_eq!(g.page_intent(&project, PageIntent::Back).map(|v| page_of(&v)), Ok(a.clone()), "消えた Page へは戻らない");
        let err = g.navigate_page(&project, &b).unwrap_err();
        assert!(err.contains("ありません"), "{err}");
    }

    /// UI-5a Done 1 の芯: `＋` と `•••` の並びが帯の操作の行き先を持ち、当てはまらない項目は並びに無い。
    #[test]
    fn ui5a_page_menus_hold_the_band_actions() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, None).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let p = g.project(&project).unwrap().1;
        let ur = g.undo_redo(&project).unwrap();
        let page = |i: PageIntent| Intent::Page(i);
        let (add, options) = page_menus(&p, &b, &ur, "None");
        // MIRROR-FRAME-1101: 1 つ目は部品を足す menu（Page を足すのは帯の tab の ＋ と ⌘T）
        let mut parts = if cfg!(feature = "mirror") { vec![page(PageIntent::AddMirrorFrame { page: b.clone() })] } else { Vec::new() };
        parts.extend([page(PageIntent::AddBrowser { page: b.clone() }), page(PageIntent::TerminalPanel(PanelAct::Add))]);
        assert_eq!(add.iter().map(|c| c.intent.clone()).collect::<Vec<_>>(), parts);
        let has = |i: PageIntent| options.iter().any(|c| c.intent == page(i.clone()));
        assert!(has(PageIntent::Move { page: b.clone(), index: 0 }) && !has(PageIntent::Move { page: b.clone(), index: 2 }), "末尾の Page は Move left だけ");
        assert!(has(PageIntent::Placement) && has(PageIntent::ShowTerminal { page: b.clone() }), "Layout と Show terminal");
        assert!(!has(PageIntent::Redo), "redo が無い時は Redo を入れない");
        assert_eq!(options.last().map(|c| c.intent.clone()), Some(page(PageIntent::Delete { page: b.clone() })), "Delete page… は最後");
        let (_, first) = page_menus(&p, &a, &ur, "None");
        assert!(first.iter().any(|c| c.intent == page(PageIntent::Move { page: a.clone(), index: 1 })) && first.iter().any(|c| c.intent == page(PageIntent::ShowTerminal { page: a.clone() })));
    }

    /// UI-5a Done 3（holdout）: Page に打つ → 消す → ⌘Z（`PageIntent::Undo`）→ Page・並びの位置・打った文字が戻る。
    /// 負の対照（commit message）: Delete の腕の消す前の save を外す → 打った文字が戻らず赤。
    #[test]
    fn ui5a_delete_then_undo_brings_back_the_page_its_place_and_the_typed_text() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&a)).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let c = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        g.navigate_page(&project, &b).unwrap();
        type_in(&mut g, &app, "b-note", "B に打ちかけ");
        let ids = |g: &Generated| g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).collect::<Vec<_>>();
        let before = ids(&g);
        assert_eq!(page_of(&g.page_intent(&project, PageIntent::Delete { page: b.clone() }).unwrap()), c, "消すと次の Page へ");
        assert_eq!(ids(&g), [a.clone(), c.clone()]);
        assert_eq!(typed(&mut g, &app, "b-note"), None, "C に B の文字を出さない");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(ids(&g), before, "Page と並びの位置が戻る");
        assert_eq!(page_of(&g.navigate_page(&project, &b).unwrap()), b);
        assert_eq!(typed(&mut g, &app, "b-note").as_deref(), Some("B に打ちかけ"), "打った文字が戻る");
        assert!(g.page_line.is_none(), "{:?}", g.page_line);
    }

    /// UI-5a Done 6（答え ③）: ••• の `Delete page…` は 1 回目では消さず、押した tab の横に `Delete` / `Keep` の 1 行を出す。開いた frame の focus は `Keep`
    /// （Enter は Keep を押す = 消さない）・`Keep` と Esc（app.rs の連鎖が呼ぶ `dismiss_delete`）で閉じて何も変わらない・確認の `Delete` を押して初めて `PageIntent::Delete`。
    /// 道は AX の Click と Enter の key（BG の driver と人の手と同じ入力）。負の対照（commit message）: menu の Delete を確認なしで返す・focus を Delete に置く → 赤。
    #[test]
    fn ui5a_delete_asks_beside_the_tab_keep_has_focus_and_only_the_second_delete_deletes() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&a)).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let n = g.project(&project).unwrap().1.pages.len();
        let pages = |g: &Generated| g.project(&project).unwrap().1.pages.len();
        let mut ax = AxPage::new();
        let ask = |ax: &mut AxPage, g: &mut Generated| {
            ax.frame(g, &project, &b, vec![]);
            assert_eq!(ax.press(g, &project, &b, "Page options"), None);
            assert_eq!(ax.press(g, &project, &b, "Delete page…"), None, "1 回目は Intent を返さない（消さない）");
            assert_eq!(ax.frame(g, &project, &b, vec![]), None);
            assert!(ax.has("Delete") && ax.has("Keep"), "tab の横に確認の 1 行が無い: {:?}", ax.labels());
            assert!(!ax.has("Page options"), "確認の間は右の組を描かない: {:?}", ax.labels());
        };
        ask(&mut ax, &mut g);
        assert_eq!(pages(&g), n, "1 回目では消えない");
        assert_eq!(ax.focused(), Some("Keep"), "開いた frame の focus は Keep");
        let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        assert_eq!(ax.frame(&mut g, &project, &b, vec![enter]), None, "Enter で消さない");
        ax.frame(&mut g, &project, &b, vec![]);
        assert!(!ax.has("Keep") && ax.has("Page options"), "Enter（focus の Keep）で閉じる: {:?}", ax.labels());
        assert!(!g.dismiss_delete(), "閉じた後の Esc は何も閉じない");
        ask(&mut ax, &mut g);
        assert!(g.dismiss_delete(), "Esc の段: 開いた確認を閉じる");
        ax.frame(&mut g, &project, &b, vec![]);
        assert!(!ax.has("Keep"), "Esc で閉じる: {:?}", ax.labels());
        ask(&mut ax, &mut g);
        assert_eq!(ax.press(&mut g, &project, &b, "Keep"), None);
        ax.frame(&mut g, &project, &b, vec![]);
        assert!(!ax.has("Keep") && pages(&g) == n, "Keep で閉じて何も変わらない");
        ask(&mut ax, &mut g);
        let del = ax.press(&mut g, &project, &b, "Delete");
        assert_eq!(del, Some(Intent::Page(PageIntent::Delete { page: b.clone() })), "確認の Delete で初めて消す");
        assert_eq!(page_of(&g.page_intent(&project, PageIntent::Delete { page: b.clone() }).unwrap()), a);
        assert_eq!(pages(&g), n - 1);
    }

    /// UI-5a Done 1: 帯の操作の行き先が画面に在る（AX の名前で見る = BG の driver と同じ道）。`＋` の 3 つ・`•••` の 7 つ（Refresh は repo を読む Project・Redo は redo の在る時）・
    /// `↶ ↷`・Project の欄（Edit = UI-4a から Page の帯でなく header の 1 つの欄・AX の名前 `View::field_name`・d1_one_field）。RunTests・SetSurfaceUrl・SetSurfaceFrame は region の帯（renderer・UI-5a で触らない・check_wired が作る側を数える）・Back は title line の
    /// `‹`（header の試験）・Rename は UI-5b（sendback 5a-1）。Page が 1 枚の時の `Delete page…` は押せない理由の項目に替わる。負の対照（commit message）: page_menus から `Delete page…` を外す → 赤。
    #[test]
    fn ui5a_band_actions_are_reachable_by_name() {
        let mut g = fresh();
        g.set_resource_reader(Box::new(FakeReader::new(read_ok(), Some(repo(json!([]))))));
        let ws = std::env::temp_dir().join(format!("shibubu-ui5a-band-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap());
        let band = page_ax(&mut g, &project, &tests);
        for want in ["Undo · ⌘Z", "Add page · ⌘T", "Add to page", "Page options", "Tests"] {
            assert!(band.iter().any(|x| x == want), "帯に {want} が無い: {band:?}");
        }
        assert_eq!(View::Project { project: project.clone(), page: tests.clone() }.field_name(), "Change this project", "Project の欄（Edit）の AX の名前");
        let add = menu_ax(&mut g, &project, &tests, "Add to page");
        for want in ["Add terminal", "Add browser"] {
            assert!(add.iter().any(|x| x == want), "＋ の menu に {want} が無い: {add:?}");
        }
        let back = page_of(&g.page_intent(&project, PageIntent::Undo).unwrap());
        assert!(page_ax(&mut g, &project, &back).iter().any(|x| x == "Redo · ⇧⌘Z"), "Undo の後の ↷");
        let options = menu_ax(&mut g, &project, &back, "Page options");
        let layout = options.iter().any(|x| x.starts_with("Layout: "));
        let moves = options.iter().any(|x| x == "Move left" || x == "Move right");
        for want in ["Show terminal", "Refresh", "Redo", "Delete page…"] {
            assert!(options.iter().any(|x| x == want), "••• の menu に {want} が無い: {options:?}");
        }
        assert!(layout && moves, "••• の menu に Layout / Move が無い: {options:?}");
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let one = menu_ax(&mut g, &project, &page, "Page options");
        assert!(one.iter().any(|x| x == "A project needs at least one page") && !one.iter().any(|x| x == "Delete page…"), "1 枚の時の理由の項目: {one:?}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 読めない保存（後の版の欄・壊れた JSON）は空の状態で開き、理由の 1 行を出す（画面は開く・前の Page の文字を出さない）。
    #[test]
    fn w2_unreadable_view_state_opens_empty_with_a_reason() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.store.lock().unwrap().put_ui_state(&session_of(&app), &view_state_key(&a), &json!({ "scroll": 3 })).unwrap();
        type_in(&mut g, &app, "x", "前の文字");
        assert_eq!(page_of(&g.open_project(&project, None).unwrap()), a);
        assert!(g.page_line.as_ref().is_some_and(|(_, l)| l.contains("Couldn't restore")), "{:?}", g.page_line);
        assert_eq!(typed(&mut g, &app, "x"), None);
        let (_, line) = g.page_line.clone().unwrap();
        assert!(!line.contains("scroll") && !line.contains("unknown field"), "serde の原文を画面に出さない: {line}");
    }

    /// W2 反証: 今の Page が router の外（別の窓・別 process の apply）で消えても、在る Page へのタブ・「先頭の Page へ」・戻るで移れる。
    #[test]
    fn w2_page_removed_elsewhere_does_not_block_moving() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, None).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let gone = |g: &mut Generated, id: &str| {
            let (_, _, h) = g.project(&project).unwrap();
            g.apply_project(&app, &h, &[ProjectPatchOp::RemovePage { page_id: id.into() }], RevisionOrigin::DirectEdit).unwrap();
        };
        gone(&mut g, &b);
        assert_eq!(g.navigate_page(&project, &a).map(|v| page_of(&v)), Ok(a.clone()), "消えた B から在る A へ");
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let c = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        gone(&mut g, &c);
        assert_eq!(g.page_intent(&project, PageIntent::Back).map(|v| page_of(&v)), Ok(b), "消えた C から戻る");
        assert!(g.page_line.is_none(), "{:?}", g.page_line);
    }

    /// W2 反証: Project の画面を離れる時（別の Project・Home・終了の on_exit）は今の Page の最新の文字を保存する。再起動で古い文字に戻らない。
    #[test]
    fn w2_leaving_the_project_saves_the_latest_draft() {
        let dir = w1_dir("w2-leave");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let View::Project { project, page: a } = g.route(&View::Home, View::App(app.clone())).unwrap() else { panic!() };
        type_in(&mut g, &app, "note", "foo");
        let on_b = g.page_intent(&project, PageIntent::Add).unwrap();
        let on_a = g.route(&on_b, View::Project { project: project.clone(), page: a.clone() }).unwrap();
        assert_eq!(typed(&mut g, &app, "note").as_deref(), Some("foo"));
        type_in(&mut g, &app, "note", "bar");
        assert_eq!(g.route(&on_a, View::Home), Ok(View::Home), "終了（on_exit）と同じ道");
        drop(g);
        let mut g = Generated::open(&dir);
        g.route(&View::Home, View::App(app.clone())).unwrap();
        assert_eq!(typed(&mut g, &app, "note").as_deref(), Some("bar"), "離れる前の最新の文字");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W2 反証: Project の画面を離れる時に保存できなければ移らず理由の 1 行（打った文を黙って失わない）。navigate_page と同じ規則。
    #[test]
    fn w2_leaving_is_refused_when_the_draft_cannot_be_saved() {
        let dir = w1_dir("w2-leave-fail");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録を作って").unwrap();
        let here = g.route(&View::Home, View::App(app.clone())).unwrap();
        type_in(&mut g, &app, "note", "失いたくない");
        rusqlite::Connection::open(dir.join("kangae.sqlite3")).unwrap().execute_batch("DROP TABLE ui_state").unwrap();
        let err = g.route(&here, View::Home).unwrap_err();
        assert!(err.contains("Couldn't keep"), "{err}");
        assert!(g.route(&here, View::Connections).is_err(), "Connections へも同じ");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 描いた文（Shape::Text の galley）を全部集める。2 frame 目を見る（1 frame 目の測りが配置に効く）。
    const DEMO: &str = "この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい";

    /// W3: bridge が答える時は、入っている app は unverified（版つき）・Web は unverified（origin。W12W-0: WebView の無い build では unavailable）・入っていない app は unavailable と理由。verified は 0。
    #[test]
    fn w3_create_project_reads_back_each_surface_once_and_never_verified() {
        let mut g = fresh();
        let mut b = crate::project::tests::fake_bridge("w3-installed.sh", "1");
        let View::Project { project, page } = g.create_project(DEMO, None, Some(&mut b)).unwrap().unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        assert_eq!(page, p.pages[0].id);
        let got: Vec<_> = g.surface_receipts(&project).unwrap().into_iter().map(|r| (r.surface_id, r.page_id, r.state, r.app_id, r.app_version)).collect();
        assert_eq!(
            got,
            [
                ("docs".into(), p.pages[1].id.clone(), project::ReceiptState::Unverified, "com.apple.TextEdit".into(), "1.19".into()),
                ("diagram".into(), p.pages[2].id.clone(), if crate::surface::WEBVIEW_BUILT { project::ReceiptState::Unverified } else { project::ReceiptState::Unavailable }, "https://excalidraw.com".into(), String::new()),
            ]
        );
        let mut b = crate::project::tests::fake_bridge("w3-missing.sh", "0");
        let View::Project { project, .. } = g.create_project(DEMO, None, Some(&mut b)).unwrap().unwrap() else { panic!() };
        let docs = g.surface_receipts(&project).unwrap().remove(0);
        assert_eq!(docs.state, project::ReceiptState::Unavailable);
        assert!(docs.oracle.contains("入っていません"), "{}", docs.oracle);
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!((revs.len(), revs[0].origin.as_str(), revs[0].base_hash.as_deref(), revs[0].ops.clone()), (1, "generate", None, json!([])));
    }

    /// W16 の試験の画面: Light の theme・幅 1440・viewport の inner_rect = 画面（`slot_on_screen` が global の point を出せる）。
    #[cfg(feature = "native_direct")]
    struct Screen {
        ctx: egui::Context,
        rect: egui::Rect,
    }

    #[cfg(feature = "native_direct")]
    impl Screen {
        fn new(h: f32) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            Screen { ctx, rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, h)) }
        }

        /// 1 frame: CentralPanel に `draw_page` を描き、偽の bridge が答える間 20ms 待つ。
        fn draw(&self, g: &mut Generated, project: &str, page: &str) {
            let mut input = egui::RawInput { screen_rect: Some(self.rect), ..Default::default() };
            input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { inner_rect: Some(self.rect), ..Default::default() });
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    g.draw_page(ui, project, page);
                });
            });
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        /// `limit` まで描き続けて、docs の面が重なる（Active）のを待つ。重ならなくても戻る（呼ぶ側が確かめる）。
        fn draw_until_active(&self, g: &mut Generated, project: &str, page: &str, limit: std::time::Duration) {
            let t = std::time::Instant::now();
            while crate::project::native::tests::life(&g.surfaces, project, "docs") != Some(crate::surface::Lifecycle::Active) && t.elapsed() < limit {
                self.draw(g, project, page);
            }
        }
    }

    /// W16-1 の配線（反証 3）: Generated の Page の移り（navigate_page → save_view_state → hide_page・activate → show_page）と Delete を、
    /// 偽の bridge の上で通す。別の Page で隠れ・戻ると同じ窓・消すのを断られたら窓は前のまま（隠さない）。
    #[cfg(feature = "native_direct")]
    #[test]
    fn w16_1_page_moves_hide_and_return_the_same_window_and_a_refused_delete_keeps_it() {
        use crate::project::native::tests as nt;
        use crate::surface::Lifecycle;
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        let (first, docs, diagram) = (p.pages[0].id.clone(), p.pages[1].id.clone(), p.pages[2].id.clone());
        let spec = p.surface_specs.iter().find(|s| s.id == "docs").unwrap().clone();
        let (s, _log, rec) = nt::rig("generated", &nt::TEXTEDIT);
        g.surfaces = s;
        g.open_project(&project, Some(&docs)).unwrap();
        g.surfaces.ensure(&project, &docs, &spec, None).unwrap();
        nt::settle_in(&mut g.surfaces, &project);
        let win = g.surfaces.window(&project, "docs");
        assert!(win.is_some());

        g.navigate_page(&project, &first).unwrap();
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Hidden), "別の Page で隠れる");
        g.navigate_page(&project, &docs).unwrap();
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Active));
        assert_eq!(g.surfaces.window(&project, "docs"), win, "戻ると同じ窓");

        // 残り 1 枚（docs）まで消す → docs を消すのは断られる。窓は隠さない
        g.page_intent(&project, PageIntent::Delete { page: first }).unwrap();
        g.page_intent(&project, PageIntent::Delete { page: diagram }).unwrap();
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Active), "別の Page を消しても docs の窓は触らない");
        let err = g.page_intent(&project, PageIntent::Delete { page: docs }).unwrap_err();
        assert!(err.contains("最後の Page"), "{err}");
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Active), "断られた Delete で窓を Shibubu の後ろに隠さない");
        let events: Vec<_> = nt::records(&rec).into_iter().map(|r| r.event).collect();
        use crate::project::native::OverlayEvent as E;
        assert_eq!(events, [E::Attached, E::HostHidden, E::HostShown]);
    }

    /// W16-1 実機 3 回目: Docs の枠は Page の縦の流れで窓の下端を越えていた（枠の下端 966・Shibubu の窓の下端 960）。
    /// 元の窓へ送る枠（set_frame）は見えている所の下端で止める。
    #[cfg(feature = "native_direct")]
    #[test]
    fn w16_1_the_frame_sent_to_the_window_stops_at_the_visible_bottom() {
        use crate::project::native::tests as nt;
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let docs = g.project(&project).unwrap().1.pages[1].id.clone();
        let (s, log, _rec) = nt::rig("visible", &nt::TEXTEDIT);
        g.surfaces = s;
        g.open_project(&project, Some(&docs)).unwrap();
        let sc = Screen::new(600.0);
        sc.draw_until_active(&mut g, &project, &docs, std::time::Duration::from_secs(20));
        let sets: Vec<Value> = std::fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|r| r["cmd"] == "set_frame").collect();
        assert!(!sets.is_empty(), "枠へ送った（今: {:?}）", nt::life(&g.surfaces, &project, "docs"));
        for s in &sets {
            let bottom = s["y"].as_f64().unwrap() + s["h"].as_f64().unwrap();
            assert!(bottom <= sc.rect.max.y as f64 + 0.5, "見えている下端 {} を越えて送った: {s}", sc.rect.max.y);
        }
    }

    /// W16-2 の通し（偽の bridge・窓の文書は FAKE_AX_TEXT_FILE）: 人が窓に 1 行足す → draw_page の定期の読み戻しが NOT_ON_DISK → 保存 → verified（版は
    /// 起きている bridge で埋める）→ Undo → Page を離れる save_view_state の読み戻しが NOT_ON_DISK。書くのは surface_receipts の追記だけ（Project・revision は同じ）。
    #[cfg(feature = "native_direct")]
    #[test]
    fn w16_2_edit_save_undo_are_read_back_into_receipts_only() {
        use crate::project::native::{tests as nt, READBACK_EVERY};
        use crate::surface::Lifecycle;
        let (ws, ax) = nt::repo("gen", "# demo\n");
        let mut g = fresh();
        // 作る時に bridge が無い（版の無い unavailable の receipt）。読み戻しが起きている bridge で版を埋める道を通す
        let View::Project { project, .. } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        let (_, p, hash0) = g.project(&project).unwrap();
        let (first, docs) = (p.pages[0].id.clone(), p.pages[1].id.clone());
        let (s, log, _rec) = nt::rig("w16-2-gen", &nt::textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap())]));
        g.surfaces = s;
        let rows = |g: &Generated| g.store.lock().unwrap().list_surface_receipts(&project, None).unwrap().len();
        let latest = |g: &Generated| g.surface_receipts(&project).unwrap().into_iter().find(|r| r.surface_id == "docs").unwrap();
        let sc = Screen::new(900.0);
        g.open_project(&project, Some(&docs)).unwrap();
        sc.draw_until_active(&mut g, &project, &docs, std::time::Duration::from_secs(20));
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Active));
        let rows0 = rows(&g);
        assert_eq!(g.readback_native(&project, "docs"), Ok(None), "開いただけ（窓と disk が同じ・disk は変わらない）は書かない");

        // 人が窓に 1 行足す（保存していない）→ draw_page の定期の読み戻し（READBACK_EVERY）が NOT_ON_DISK を書く
        std::fs::write(&ax, "# demo\nn-w16-2\n").unwrap();
        let t = std::time::Instant::now();
        while latest(&g).oracle != project::NOT_ON_DISK && t.elapsed() < READBACK_EVERY * 4 {
            sc.draw(&mut g, &project, &docs);
        }
        let typed = latest(&g);
        assert_eq!((typed.state, typed.oracle.as_str()), (project::ReceiptState::Unverified, project::NOT_ON_DISK), "draw_page が読み戻した");
        assert!(renderer::surface_status_line(Some(&typed)).1.contains("has unsaved changes"));
        assert_eq!(g.readback_native(&project, "docs"), Ok(None), "同じ行を続けて書かない");

        // 保存（disk が窓の文書と同じになる）→ verified・版は bridge の find_app で埋めた・証拠は disk の sha
        std::fs::write(ws.join("README.md"), "# demo\nn-w16-2\n").unwrap();
        let saved = g.readback_native(&project, "docs").unwrap().expect("保存を書く");
        assert_eq!((saved.state, saved.app_version.as_str()), (project::ReceiptState::Verified, "1.19"));
        assert_eq!(saved.evidence_ref, format!("file_sha256:{}", crate::runtime::permission::sha256_hex(b"# demo\nn-w16-2\n")));
        assert_eq!(latest(&g), saved);

        // Undo（窓だけ戻る・disk は保存した中身）→ Page を離れる（save_view_state の読み戻し）だけで NOT_ON_DISK
        std::fs::write(&ax, "# demo\n").unwrap();
        g.navigate_page(&project, &first).unwrap();
        assert_eq!(latest(&g).oracle, project::NOT_ON_DISK, "隠す前に読み戻した");
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Hidden));
        // 戻る（activate の読み戻し）: 変わっていないので足さない
        g.navigate_page(&project, &docs).unwrap();
        assert_eq!(rows(&g), rows0 + 3, "NOT_ON_DISK・verified・NOT_ON_DISK の 3 行だけ");
        let (_, _, hash1) = g.project(&project).unwrap();
        assert_eq!(hash1, hash0, "Project を書かない（Undo の対象にしない）");
        assert_eq!(g.store.lock().unwrap().list_project_revisions(&project).unwrap().len(), 1, "revision を足さない");
        let cmds: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).map(|r| r["cmd"].as_str().unwrap_or("").to_string()).collect();
        assert!(cmds.iter().filter(|c| *c == "ax_text").count() >= 3, "{cmds:?}");
        assert!(!cmds.iter().any(|c| c == "open_app" || c == "set_value"), "元の app に書かない: {cmds:?}");
        assert_eq!(std::fs::read_to_string(ws.join("README.md")).unwrap(), "# demo\nn-w16-2\n", "製品は file を書かない");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 反証の直し（50dda73 の待たない道・背景の TextEdit の ax_text は実測 13〜31 秒）: 打った後（NOT_ON_DISK）に保存し、ax_text の応答を待つ読みが
    /// 2 回以上続いても、応答を使う読みは「窓の文書を最後に読めた時」の disk と比べるので verified になる（待つ読みで比べる相手を進めて IN_SYNC・None にしない）。
    #[cfg(feature = "native_direct")]
    #[test]
    fn w16_2_a_save_while_the_window_read_is_pending_is_verified() {
        use crate::project::native::tests as nt;
        use crate::surface::Lifecycle;
        let (ws, ax) = nt::repo("pending", "# demo\n");
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        let docs = g.project(&project).unwrap().1.pages[1].id.clone();
        let (s, _log, _rec) = nt::rig("w16-2-pending", &nt::textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap()), ("FAKE_AX_SLOW", "1.5")]));
        g.surfaces = s;
        let latest = |g: &Generated| g.surface_receipts(&project).unwrap().into_iter().find(|r| r.surface_id == "docs").unwrap();
        let sc = Screen::new(900.0);
        g.open_project(&project, Some(&docs)).unwrap();
        sc.draw_until_active(&mut g, &project, &docs, std::time::Duration::from_secs(30));
        assert_eq!(nt::life(&g.surfaces, &project, "docs"), Some(Lifecycle::Active));
        // 打つ（rig の待ちは 30 秒なのでこの読みは応答を待つ）→ NOT_ON_DISK
        std::fs::write(&ax, "# demo\nn-pending\n").unwrap();
        let typed = g.readback_native(&project, "docs").unwrap().expect("保存していない変更");
        assert_eq!(typed.oracle, project::NOT_ON_DISK);
        // ここから製品と同じ待たない道（応答は 1.5 秒・待つのは 0.1 秒）。保存 → 応答を待つ読みが 2 回（どちらも書かない）
        nt::set_ax_budget(&mut g.surfaces, std::time::Duration::from_millis(100));
        std::fs::write(ws.join("README.md"), "# demo\nn-pending\n").unwrap();
        assert_eq!(g.readback_native(&project, "docs"), Ok(None), "応答を待っている");
        assert_eq!(g.readback_native(&project, "docs"), Ok(None), "まだ待っている");
        // 応答が来て使われるまで描く（draw_page の定期の読み戻しが拾う）
        let t = std::time::Instant::now();
        while latest(&g).oracle == project::NOT_ON_DISK && t.elapsed() < std::time::Duration::from_secs(30) {
            sc.draw(&mut g, &project, &docs);
        }
        let saved = latest(&g);
        assert_eq!((saved.state, saved.oracle.as_str()), (project::ReceiptState::Verified, "file_sha+ax_text"), "{saved:?}");
        assert_eq!(saved.evidence_ref, format!("file_sha256:{}", crate::runtime::permission::sha256_hex(b"# demo\nn-pending\n")));
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// W16-3: persist → Dock を捨てる → open_project（restore_native を直接呼ばない）で同じ pid・window_id。「TextEdit を右へ」のあと Undo は Dock の page_id を木に戻す。終了した pid は接続済みと出さない。正の対照: もう 1 行足して保存すると disk sha が変わる。
    #[cfg(feature = "native_direct")]
    #[test]
    fn w16_3_restore_native_retarget_and_gone_keep_the_same_window() {
        use crate::project::native::tests as nt;
        use crate::surface::Lifecycle;
        let (ws, ax) = nt::repo("w16-3-gen", "# demo\n");
        let mut g = fresh();
        let View::Project { project, .. } =
            g.create_project(DEMO, Some(&ws), None).unwrap().unwrap()
        else {
            panic!()
        };
        let (_, p, _) = g.project(&project).unwrap();
        let (first, docs, diagram) = (
            p.pages[0].id.clone(),
            p.pages[1].id.clone(),
            p.pages[2].id.clone(),
        );
        let spec = p
            .surface_specs
            .iter()
            .find(|s| s.id == "docs")
            .unwrap()
            .clone();
        let (s, _log, _rec) = nt::rig(
            "w16-3-gen",
            &nt::textedit(&[("FAKE_AX_TEXT_FILE", ax.to_str().unwrap())]),
        );
        g.surfaces = s;
        g.open_project(&project, Some(&docs)).unwrap();
        g.surfaces
            .ensure(&project, &docs, &spec, Some(&ws))
            .unwrap();
        nt::settle_in(&mut g.surfaces, &project);
        let win = g.surfaces.window(&project, "docs");
        assert_eq!(win, Some((222, Some(500))));

        g.navigate_page(&project, &first).unwrap();
        g.navigate_page(&project, &docs).unwrap();
        assert_eq!(g.surfaces.window(&project, "docs"), win);

        g.surfaces.persist_identity(&project, "docs").unwrap();
        g.surfaces.forget_docks();
        g.open_project(&project, Some(&docs)).unwrap();
        nt::settle_in(&mut g.surfaces, &project);
        assert_eq!(g.surfaces.window(&project, "docs"), win, "同じ窓に繋ぎ直す");
        assert_eq!(g.surfaces.identity(&project, "docs").unwrap().pid, 222);

        g.page_intent(
            &project,
            PageIntent::Edit {
                text: "TextEdit を右へ".into(),
            },
        )
        .unwrap();
        let id = g.surfaces.identity(&project, "docs").expect("重ねている");
        assert_eq!(
            id.page_id, diagram,
            "MoveSurface の後に page_id だけ付け替える"
        );
        assert_eq!(
            (id.pid, id.window_id),
            (222, 500),
            "「TextEdit を右へ」は窓を動かさない"
        );
        g.page_intent(&project, PageIntent::Undo).unwrap();
        let id = g.surfaces.identity(&project, "docs").expect("重ねている");
        assert_eq!(id.page_id, docs, "Undo で Dock の page_id を木に戻す");
        assert_eq!((id.pid, id.window_id), (222, 500));
        g.surfaces.hide_page(&docs);
        assert_eq!(
            nt::life(&g.surfaces, &project, "docs"),
            Some(Lifecycle::Hidden),
            "Undo 後の hide_page(今の Page) は窓を隠す"
        );
        g.surfaces.show_page(&docs);
        assert_eq!(g.surfaces.window(&project, "docs"), win);

        let sha0 = crate::runtime::permission::sha256_hex(b"# demo\n");
        std::fs::write(&ax, "# demo\nn-w16-3\n").unwrap();
        std::fs::write(ws.join("README.md"), "# demo\nn-w16-3\n").unwrap();
        let saved = g
            .readback_native(&project, "docs")
            .unwrap()
            .expect("正の対照の保存");
        assert_eq!(saved.state, project::ReceiptState::Verified);
        assert_ne!(
            saved.evidence_ref,
            format!("file_sha256:{sha0}"),
            "disk sha が変わる"
        );

        let gone = crate::project::native::OverlayIdentity {
            schema: crate::project::native::IDENTITY_SCHEMA.into(),
            project_id: project.clone(),
            surface_id: "docs".into(),
            page_id: diagram.clone(),
            bundle_id: "com.apple.TextEdit".into(),
            pid: 999,
            window_id: 500,
            title: "README.md".into(),
        };
        let path = crate::project::native::identity_path(_rec.parent().unwrap());
        gone.append(&path).unwrap();
        g.surfaces.forget_docks();
        g.open_project(&project, Some(&docs)).unwrap();
        let v = g.surfaces.view(&project, "docs").unwrap();
        assert_eq!(v.state, crate::surface::PortalState::SourceUnavailable);
        assert!(
            !v.detail.contains("接続済み")
                && nt::life(&g.surfaces, &project, "docs") != Some(Lifecycle::Active),
            "{}",
            v.detail
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 反証の直し（W3）: env が無くても一言に書いた repo の場所を Project に保存する（Finder から開いた .app の道）。
    #[test]
    fn w3_create_project_takes_the_repo_from_the_sentence_without_the_env() {
        let mut g = fresh();
        let dir = std::env::temp_dir().join(format!("shibubu-w3-sentence-ws-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let View::Project { project, .. } = g.create_project(&format!("{DEMO} {}", dir.display()), None, None).unwrap().unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        assert_eq!(p.resources.iter().map(|r| r.uri.clone()).collect::<Vec<_>>(), [Some(format!("file://{}", dir.display()))]);
        assert_eq!(p.terminal_specs.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W3: surface ごとに最新（seq の最大）だけを surface_specs の順で返す・読めない行は Err の 1 行・1 行も無い面は返さない。
    #[test]
    fn w3_surface_receipts_take_the_latest_per_surface_and_refuse_unreadable_rows() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        let mut later = crate::project::unavailable(&p.surface_specs[1], &p.pages[2].id, "二回目");
        later.state = project::ReceiptState::Ended;
        {
            let store = g.store.lock().unwrap();
            store.put_surface_receipt(&project, "diagram", &p.pages[2].id, "web", "ended", &serde_json::to_value(&later).unwrap()).unwrap();
            store.put_surface_receipt(&project, "gone", "page.x", "web", "unverified", &json!({"surface_id": "gone"})).unwrap();
        }
        let got: Vec<_> = g.surface_receipts(&project).unwrap().into_iter().map(|r| (r.surface_id, r.state)).collect();
        assert_eq!(got, [("docs".to_string(), project::ReceiptState::Unavailable), ("diagram".to_string(), project::ReceiptState::Ended)], "spec に無い面は返さない");
        g.store.lock().unwrap().put_surface_receipt(&project, "docs", &p.pages[1].id, "native_app", "verified", &json!({"surface_id": "docs", "later": 1})).unwrap();
        let e = g.surface_receipts(&project).unwrap_err();
        assert!(e.starts_with("Couldn't read the check"), "{e}");
    }

    /// R2: 試験で 1 画面を描いて、出た文字を集める口（前は `painted_texts` と W6 の `paint` の 2 つの写しだった）。
    /// 使い方: Page 1 枚は `page_texts(&mut g, &project, &page)`・`painted_texts_of(|ui| shell::draw_app_region(ui, &mut |ui| { g.draw(ui, &id); }))`。
    /// 保証する: Light の theme・1440×900 の CentralPanel で 2 frame 描き、2 frame 目の Text の shape の文字を描いた順に返す（今の 2 つの写しと同じ）。
    /// 保証しない: 位置・色（それは別の crate の `tests/common/mod.rs` の口・ここからは共有できない）・入力の手（click は渡さない）。
    fn painted_texts_of(paint: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        painted_of(paint).into_iter().map(|(t, _)| t).collect()
    }

    /// `painted_texts_of` と同じ 2 frame 目の文字に、画面の上の矩形を添える（W12W-1: 保存の欄が 900 の高さの中に出るか）。
    fn painted_of(mut paint: impl FnMut(&mut egui::Ui)) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        let mut out = None;
        for _ in 0..2 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| paint(ui));
            }));
        }
        texts_of(&out.unwrap().shapes)
    }

    /// 描いた Text の shape の文字と画面の上の矩形（描いた順・visual transform の後 = 目に見える位置）。
    fn texts_of(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn walk(s: &egui::epaint::Shape, acc: &mut Vec<(String, egui::Rect)>) {
            match s {
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
                egui::epaint::Shape::Text(t) => acc.push((t.galley.text().to_owned(), egui::Rect::from_min_size(t.pos, t.galley.size()))),
                _ => {}
            }
        }
        let mut acc = Vec::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
        acc
    }

    /// `draw_page` 1 枚の `painted_texts_of`（Page の画面の試験が共有する形）。
    fn page_texts(g: &mut Generated, project: &str, page: &str) -> Vec<String> {
        painted_texts_of(|ui| {
            g.draw_page(ui, project, page);
        })
    }

    /// UI-5a: draw_page を AccessKit 付きで frame ごとに回す。painter の `↶ ↷` は字を持たず、帯の操作は `＋` と `•••` の menu の中なので、
    /// 帯は AX の名前で見て menu は AX の Click で開く（BG の driver と同じ道）。`ctx` を frame をまたいで持つ = popup の開閉と focus が続く。
    struct AxPage {
        ctx: egui::Context,
        nodes: Vec<(egui::accesskit::NodeId, String)>,
        focus: Option<egui::accesskit::NodeId>,
        /// FX3: 名前ごとの画面の矩形（AX の bounds・tab の drag の試験が押す位置を引く）
        rects: Vec<(String, egui::Rect)>,
        /// FX3 反証の直し: 描いた字と目に見える矩形（`texts_of`・tab の travel の試験が字の位置を見る）
        texts: Vec<(String, egui::Rect)>,
    }

    impl AxPage {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            AxPage { ctx, nodes: Vec::new(), focus: None, rects: Vec::new(), texts: Vec::new() }
        }

        /// 1 frame 描き、押された Intent を返す（呼ぶ側が app.rs と同じく page_intent / navigate_page に渡す）。
        fn frame(&mut self, g: &mut Generated, project: &str, page: &str, events: Vec<egui::Event>) -> Option<Intent> {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events, ..Default::default() };
            let mut got = None;
            let out = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got = g.draw_page(ui, project, page));
            });
            let tree = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
            self.nodes = tree.nodes.iter().filter_map(|(id, n)| n.label().map(|l| (*id, l.to_owned()))).collect();
            let rect = |b: egui::accesskit::Rect| egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32));
            self.rects = tree.nodes.iter().filter_map(|(_, n)| Some((n.label()?.to_owned(), rect(n.bounds()?)))).collect();
            self.focus = Some(tree.focus);
            self.texts = texts_of(&out.shapes);
            got
        }

        /// FX3: `label` の node の画面の矩形（無ければ panic = 試験が成り立たない）。
        fn rect(&self, label: &str) -> egui::Rect {
            self.rects.iter().find(|(l, _)| l == label).map(|(_, r)| *r).unwrap_or_else(|| panic!("{label} の矩形が AX の木に無い: {:?}", self.labels()))
        }

        fn labels(&self) -> Vec<String> {
            self.nodes.iter().map(|(_, l)| l.clone()).collect()
        }

        fn has(&self, label: &str) -> bool {
            self.nodes.iter().any(|(_, l)| l == label)
        }

        fn focused(&self) -> Option<&str> {
            self.focus.and_then(|f| self.nodes.iter().find(|(id, _)| *id == f)).map(|(_, l)| l.as_str())
        }

        /// `label` の node を AX の Click で押す（1 frame）。
        fn press(&mut self, g: &mut Generated, project: &str, page: &str, label: &str) -> Option<Intent> {
            let target = self.nodes.iter().find(|(_, l)| l == label).unwrap_or_else(|| panic!("{label} が AX の木に無い: {:?}", self.labels())).0;
            let click = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target, data: None });
            self.frame(g, project, page, vec![click])
        }
    }

    /// 帯の AX の名前（2 frame 目）。
    fn page_ax(g: &mut Generated, project: &str, page: &str) -> Vec<String> {
        let mut ax = AxPage::new();
        ax.frame(g, project, page, vec![]);
        ax.frame(g, project, page, vec![]);
        ax.labels()
    }

    /// `opener`（Page の右下の `Add to page` か帯の `Page options`）を AX の Click で開いた後の AX の名前（menu の項目を含む）。
    fn menu_ax(g: &mut Generated, project: &str, page: &str, opener: &str) -> Vec<String> {
        let mut ax = AxPage::new();
        ax.frame(g, project, page, vec![]);
        ax.press(g, project, page, opener);
        ax.frame(g, project, page, vec![]);
        ax.labels()
    }

    /// MIRROR-FRAME-1101 interface（caller の形・本体の前は ignore）: `AddMirrorFrame` = 空の Mirror の枠（bundle id が空・640×400・keep_alive）を Page の root の末尾に 1 つ・
    /// 1 行 `Added a mirror frame to “{Page}” · ⌘Z to undo`。窓を入れる（`ChooseWindow` の bundle_id）と app と題の語が入り、⌘Z 1 回で空の枠に・もう 1 回で枠が消える。
    /// 負の対照: 本体の `AddMirrorFrame` の腕を interface の何もしない形に戻す → 1 つ目の assert が赤・`ChooseWindow` の op の `bundle_id` を None に戻す → 3 つ目が赤。
    #[test]
    fn mirror_frame_1101_add_a_blank_frame_fill_it_and_undo_twice() {
        let dir = w1_dir("mf1101-frame");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let before = g.project(&project).unwrap().1.surface_specs.len();
        g.page_intent(&project, PageIntent::AddMirrorFrame { page: page.clone() }).unwrap();
        let (_, p, _) = g.project(&project).unwrap();
        assert_eq!(p.surface_specs.len(), before + 1, "枠を置いていない");
        let spec = p.surface_specs.iter().find(|s| s.blank_frame()).expect("空の Mirror の枠");
        assert_eq!((spec.backend, spec.keep_alive, spec.frame), (project::SurfaceBackendKind::Mirror, true, Some(project::SurfaceFrame { x: 0.0, y: 0.0, w: 640.0, h: 400.0 })));
        let sid = spec.id.clone();
        let pg = p.page(&page).unwrap();
        assert_eq!(pg.root.children.last().and_then(|n| n.props.get("surface_ref")).and_then(|v| v.as_str()), Some(sid.as_str()), "Page の末尾に置いていない");
        assert_eq!(g.page_line, Some((RegionState::Ready, format!("Added a mirror frame to “{}” · ⌘Z to undo", pg.title))));
        g.page_intent(&project, PageIntent::ChooseWindow { surface_id: sid.clone(), title: Some("Docs".into()), bundle_id: Some("com.google.Chrome".into()) }).unwrap();
        let target = |g: &Generated| g.project(&project).unwrap().1.surface_specs.iter().find(|s| s.id == sid).map(|s| s.target.clone());
        assert_eq!(target(&g), Some(project::SurfaceTarget::App { bundle_id: "com.google.Chrome".into(), file: Some("Docs".into()) }), "窓を入れても app が入らない");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(target(&g), Some(project::SurfaceTarget::App { bundle_id: String::new(), file: None }), "⌘Z 1 回で空の枠に戻らない");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(target(&g), None, "⌘Z 2 回で枠が消えない");
    }

    /// MIRROR-FRAME-1101 interface（owner 253 = 案 3）: 部品を足す menu は 1 つ（`page_menus` の 1 つ目）= `Add mirror frame`（feature mirror の時）・`Add browser`・`Add terminal`。
    /// Page の右下の ＋（AX `Add to page`）が開く。帯の tab の ＋ は menu を開かず Page を 1 枚足す手を返す（AX `Add page · ⌘T`）。
    /// 負の対照: 帯の ＋ を menu に戻す → 最後の assert が赤・page_menus の 1 つ目に `Add page` を残す → 1 つ目が赤。
    #[test]
    fn mirror_frame_1101_one_parts_menu_and_the_tab_plus_adds_a_page() {
        let dir = w1_dir("mf1101-menu");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let (_, p, _) = g.project(&project).unwrap();
        let (parts, _) = page_menus(&p, &page, &g.undo_redo(&project).unwrap(), "None");
        let want: Vec<&str> = if cfg!(feature = "mirror") { vec!["Add mirror frame", "Add browser", "Add terminal"] } else { vec!["Add browser", "Add terminal"] };
        assert_eq!(parts.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), want);
        let menu = menu_ax(&mut g, &project, &page, "Add to page");
        assert!(want.iter().all(|w| menu.iter().any(|m| m == w)), "右下の ＋ の menu: {menu:?}");
        let mut ax = AxPage::new();
        ax.frame(&mut g, &project, &page, vec![]);
        assert_eq!(ax.press(&mut g, &project, &page, "Add page · ⌘T"), Some(Intent::Page(PageIntent::Add)), "帯の ＋ が Page を足す手を返さない");
    }

    /// W12W-1 反証の直し: 書き出した file の保存の欄は、それを書き出した Project の Diagram の Page にだけ・WebView の矩形より上・900 の高さの中に出る。
    /// 「置きました」の 1 行も同じ面の Page にだけ。どの Project も Diagram の面の id は `diagram` なので、key に Project が無いと B の Page に A の欄が出て
    /// B の repo に書く。負の対照（commit message）: key を surface_id だけに戻す・保存の欄を render_page の後に戻す。
    #[test]
    fn w12w1_save_row_is_on_its_own_projects_diagram_page_above_the_web_view() {
        use crate::project::web;
        let mut g = fresh();
        let View::Project { project: a, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let View::Project { project: b, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_ne!(a, b);
        let (pa, pb) = (g.project(&a).unwrap().1, g.project(&b).unwrap().1);
        let dir = std::env::temp_dir().join(format!("w12w1-rows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join("Untitled.excalidraw");
        std::fs::write(&staged, "{}").unwrap();
        let mut d = Some(crate::surface::WebDownload { url: "blob:https://excalidraw.com/1".into(), staged: staged.clone(), done: Some(true) });
        g.web.absorb(&web::key(&a, "diagram"), &mut d);
        assert!(d.is_none(), "終わった download は欄へ移る");
        let row = |t: &[(String, egui::Rect)]| t.iter().find(|(s, _)| s == "Save to repo").map(|(_, r)| *r);
        let on_a = painted_of(|ui| {
            g.draw_page(ui, &a, &pa.pages[2].id);
        });
        let r = row(&on_a).unwrap_or_else(|| panic!("A の Diagram の Page の画面の中に「repo に保存」が無い: {on_a:?}"));
        let slot_top = on_a.iter().find(|(s, _)| s == "Diagram in Excalidraw").map(|(_, r)| r.min.y).unwrap_or_else(|| panic!("正の対照: Diagram の面が Page に置かれていない: {on_a:?}"));
        assert!(r.min.y >= 0.0 && r.max.y <= 900.0 && r.max.y <= slot_top, "欄 {r:?} は WebView の矩形（{slot_top} から下）より上・画面の中");
        let on_a_docs = painted_texts_of(|ui| {
            g.draw_page(ui, &a, &pa.pages[1].id);
        });
        assert!(on_a_docs.iter().any(|t| t == crate::ui::theme::icon_glyph(crate::domain::icon::Icon::Ellipsis)) && !on_a_docs.iter().any(|t| t == "Save to repo"), "Diagram の無い Page に欄を出さない: {on_a_docs:?}");
        let on_b = painted_texts_of(|ui| {
            g.draw_page(ui, &b, &pb.pages[2].id);
        });
        assert!(on_b.iter().any(|t| t == crate::ui::theme::icon_glyph(crate::domain::icon::Icon::Ellipsis)) && !on_b.iter().any(|t| t == "Save to repo"), "A で書き出した file の欄を B に出さない: {on_b:?}");
        let repo = dir.join("repo-a");
        assert_eq!(g.web.save(&web::key(&b, "diagram"), Some(&repo)), Err(web::SaveError::NotStaged), "B の key で A の file を保存しない");
        g.web.save(&web::key(&a, "diagram"), Some(&repo)).unwrap();
        g.web_saved = Some((web::key(&a, "diagram"), "Written to docs/Untitled.excalidraw · read back from disk".into()));
        let placed = |g: &mut Generated, p: &str, page: &str| painted_texts_of(|ui| {
            g.draw_page(ui, p, page);
        }).iter().any(|t| t.contains("read back from disk"));
        assert!(placed(&mut g, &a, &pa.pages[2].id), "正の対照: A の Diagram の Page に「置きました」");
        assert!(!placed(&mut g, &b, &pb.pages[2].id) && !placed(&mut g, &a, &pa.pages[1].id), "「置きました」は保存した面の Page にだけ");
        let staged2 = dir.join("second.excalidraw");
        std::fs::write(&staged2, "{}").unwrap();
        g.web.absorb(&web::key(&a, "diagram"), &mut Some(crate::surface::WebDownload { url: "blob:2".into(), staged: staged2, done: Some(true) }));
        assert!(!placed(&mut g, &a, &pa.pages[2].id), "次の download が届いたら前の「置きました」を消す");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W12W-B: 「＋ Browser」で WebView の面が付き、URL と frame は revision に残って再起動の後も同じ。空の欄と http は書かない。
    #[test]
    fn w12wb_add_browser_places_a_webview_and_url_and_frame_survive_restart() {
        let dir = w1_dir("w12wb-browser");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let menu = menu_ax(&mut g, &project, &page, "Add to page");
        assert!(menu.iter().any(|t| t == "Add browser"), "＋ の menu に Add browser が無い: {menu:?}");
        g.page_intent(&project, PageIntent::AddBrowser { page: page.clone() }).unwrap();
        let (_, p, _) = g.project(&project).unwrap();
        let spec = p.surface_specs.iter().find(|s| s.browser).expect("Browser の面");
        assert_eq!(spec.backend, project::SurfaceBackendKind::WebView);
        match &spec.target {
            project::SurfaceTarget::Url { url } => assert_eq!(url, crate::project::web::DEFAULT_BROWSER_URL),
            other => panic!("{other:?}"),
        }
        assert!(spec.keep_alive && spec.frame.is_none());
        let sid = spec.id.clone();
        assert!(p.pages[0].root.children.iter().any(|n| n.component == "source_surface" && n.props.get("surface_ref").and_then(|v| v.as_str()) == Some(sid.as_str())));
        g.page_intent(&project, PageIntent::SetSurfaceUrl { surface_id: sid.clone(), input: "  ".into() }).unwrap();
        match &g.project(&project).unwrap().1.surface_specs.iter().find(|s| s.id == sid).unwrap().target {
            project::SurfaceTarget::Url { url } => assert_eq!(url, crate::project::web::DEFAULT_BROWSER_URL, "空は送らない"),
            other => panic!("{other:?}"),
        }
        g.page_intent(&project, PageIntent::SetSurfaceUrl { surface_id: sid.clone(), input: "http://example.com/".into() }).unwrap();
        assert!(g.page_line.as_ref().is_some_and(|(_, l)| l.contains("https")), "http は止める: {:?}", g.page_line);
        match &g.project(&project).unwrap().1.surface_specs.iter().find(|s| s.id == sid).unwrap().target {
            project::SurfaceTarget::Url { url } => assert_eq!(url, crate::project::web::DEFAULT_BROWSER_URL),
            other => panic!("{other:?}"),
        }
        g.page_intent(&project, PageIntent::SetSurfaceUrl { surface_id: sid.clone(), input: "https://www.youtube.com/watch?v=x".into() }).unwrap();
        g.page_intent(&project, PageIntent::SetSurfaceFrame { surface_id: sid.clone(), x: 10.0, y: 20.0, w: 400.0, h: 300.0 }).unwrap();
        g.page_intent(&project, PageIntent::SetSurfaceFrame { surface_id: sid.clone(), x: 0.0, y: 0.0, w: 0.0, h: 10.0 }).unwrap();
        let before = g.project(&project).unwrap().1.surface_specs.iter().find(|s| s.id == sid).unwrap().clone();
        drop(g);
        let g = Generated::open(&dir);
        let (_, p, _) = g.project(&project).unwrap();
        let spec = p.surface_specs.iter().find(|s| s.id == sid).expect("再起動の後も面が残る");
        assert!(spec.browser);
        match &spec.target {
            project::SurfaceTarget::Url { url } => assert_eq!(url, "https://www.youtube.com/watch?v=x"),
            other => panic!("{other:?}"),
        }
        assert_eq!(spec.frame, Some(project::SurfaceFrame { x: 10.0, y: 20.0, w: 400.0, h: 300.0 }));
        assert_eq!(spec.frame, before.frame);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W2b の実機の不合格（一覧に著者が出ない）を composer の本物の出力で: 一言 → 足す {題名, 著者} → 描く → 題名の行に著者の文字。
    /// columns を手で渡す tests/renderer.rs の w2b_list_shows_the_columns_after_the_title では、composer が欄を別の名前（timeline の `fields`）で
    /// 渡していた事を拾えなかった。
    #[test]
    fn w2b_composed_books_log_shows_the_author_in_the_row() {
        let mut g = fresh();
        let id = g.create("読書記録をつけたい").unwrap();
        {
            let (def, mut h) = host_of(&mut g, &id);
            let add = def.actions.iter().find(|a| a.id.ends_with(".add")).expect("読書記録に足す操作が無い").id.clone();
            h.invoke(ActionInvocation { action_ref: add, origin: InvocationOrigin::Button { node_key: "books-entry".into() }, input: json!({ "title": "吾輩は猫である", "author": "夏目漱石" }) });
            assert!(h.line.is_none(), "1 冊足すと Verified: {:?}", h.line);
        }
        let texts = painted_texts_of(|ui| shell::draw_app_region(ui, &mut |ui| {
            g.draw(ui, &id);
        }));
        let row = texts.iter().find(|t| t.contains("吾輩は猫である") && !t.contains("記録")).unwrap_or_else(|| panic!("正の対照: 題名の行が描かれていない: {texts:?}"));
        assert!(row.contains("夏目漱石"), "題名の行に著者が出ていない: {row:?}");
    }

    /// W6 holdout（interface の commit 26c1b54 では `PageIntent::Move` が todo!() で赤だった）: 3 枚の Project で A を 1 つ右へ移すと
    /// 並びだけが B・A・C に変わり、どの Page の `page_hash` も前後で同じ（木も Page の revision も触らない）。revision は `direct_edit` で ops は
    /// `MovePage` 1 つ。居場所は今の Page（C）のまま。同じ位置への移動と無い id は何も書かない。
    /// 負の対照（実装の commit で取った）: `project::apply` の MovePage が Page を touched に入れる（Page の revision が進む）と hash の比べが赤・
    /// 同じ位置の判定を外すと revision の数が赤。
    /// 未測: Terminal の PID（統合の後もこの試験は Terminal を置かない）・surface の窓 id と pid（M1 の後に `l4_same_windows`）。
    #[test]
    fn w6_move_page_keeps_untouched_page_hashes() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&a)).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let c = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let hashes = |p: &ProjectDefinitionV1| p.pages.iter().map(|pg| (pg.id.clone(), page_hash(pg))).collect::<BTreeMap<_, _>>();
        let ids = |p: &ProjectDefinitionV1| p.pages.iter().map(|pg| pg.id.clone()).collect::<Vec<_>>();
        let before = g.project(&project).unwrap().1;
        let v = g.page_intent(&project, PageIntent::Move { page: a.clone(), index: 1 }).unwrap();
        assert_eq!(page_of(&v), c, "移しても居場所は今の Page のまま");
        let after = g.project(&project).unwrap().1;
        assert_eq!(ids(&after), [b.clone(), a.clone(), c.clone()]);
        assert_eq!(hashes(&after), hashes(&before), "どの Page の木も revision も変わらない（並びだけ）");
        assert_eq!(after.revision, before.revision + 1);
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        let last = revs.last().unwrap();
        assert_eq!((last.origin.as_str(), last.ops.clone()), ("direct_edit", json!([{ "op": "move_page", "page_id": a, "index": 1 }])));
        let n = revs.len();
        g.page_intent(&project, PageIntent::Move { page: a.clone(), index: 1 }).unwrap();
        assert!(g.page_intent(&project, PageIntent::Move { page: "page.none".into(), index: 0 }).is_err(), "無い id は拒否の 1 行");
        assert_eq!(g.store.lock().unwrap().list_project_revisions(&project).unwrap().len(), n, "同じ位置・無い id は何も書かない");
    }

    /// W6 の画面: `Move right` は末尾でない Page にだけ・`Move left` は先頭でない Page にだけ ••• の menu に出る（押しても何も起きない項目を見せない・L2）。押した結果は上の holdout と同じ道。
    #[test]
    fn w6_move_button_shows_only_when_there_is_a_right_side() {
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&a)).unwrap();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        // UI-5a: 帯の「→」は ••• の menu の Move left / Move right に（並びの端の向きは並びに入れない）
        let on_a = menu_ax(&mut g, &project, &a, "Page options");
        assert!(on_a.iter().any(|t| t == "Delete page…"), "正の対照: ••• の menu が開いていない: {on_a:?}");
        assert!(on_a.iter().any(|t| t == "Move right") && !on_a.iter().any(|t| t == "Move left"), "先頭の Page の向き: {on_a:?}");
        let on_b = menu_ax(&mut g, &project, &b, "Page options");
        assert!(on_b.iter().any(|t| t == "Move left") && !on_b.iter().any(|t| t == "Move right"), "末尾の Page に Move right が出ている: {on_b:?}");
    }

    /// W7 holdout（I2 の DB の側）: 「Tests Page を足して」→ revision 1 つ・origin nl_edit・ops は add_page 1 つ・足した Page へ移る・他の Page の hash が同じ。
    /// 過剰側: 2 回目は「既に」の 1 行で revision を足さない。「Excalidraw を右へ」→ move_surface 1 つ・動いた先の Page へ移る（UI-5a A3）・surface_specs と receipt が前後で同じ。
    /// 未測: Terminal の PID（統合の後もこの試験は Terminal を置かない）・surface の窓 id と pid と handle（M1 の後・surface 0 件は赤）・画面の AX の木の Page タブ（実機）。
    #[test]
    fn w7_nl_edit_writes_one_nl_edit_revision_and_keeps_other_pages() {
        let mut g = fresh();
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let hashes = |p: &ProjectDefinitionV1| p.pages.iter().map(|pg| (pg.id.clone(), page_hash(pg))).collect::<BTreeMap<_, _>>();
        let revs = |g: &Generated| g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        let receipts_before = g.surface_receipts(&project).unwrap();
        let before = g.project(&project).unwrap().1;
        assert!(before.pages.iter().all(|pg| pg.title != "Tests"), "前提: 生成は Tests を作らない");
        let edit = |g: &mut Generated, text: &str| g.page_intent(&project, PageIntent::Edit { text: text.into() });

        let tests = page_of(&edit(&mut g, "Tests Page を足して").unwrap());
        let after = g.project(&project).unwrap().1;
        assert_eq!(after.pages.len(), before.pages.len() + 1);
        assert_eq!(after.pages.last().map(|pg| (pg.id.as_str(), pg.title.as_str())), Some((tests.as_str(), "Tests")), "足した Page へ移る");
        let mut kept = hashes(&after);
        kept.remove(&tests);
        assert_eq!(kept, hashes(&before), "他の Page の hash が同じ");
        let last = revs(&g).last().cloned().unwrap();
        assert_eq!(last.origin.as_str(), "nl_edit");
        assert_eq!(last.ops.as_array().map(|a| a.iter().map(|o| o["op"].clone()).collect::<Vec<_>>()), Some(vec![json!("add_page")]), "ops は AddPage 1 つ");

        let n = revs(&g).len();
        let again = edit(&mut g, "Tests Page を足して").unwrap_err();
        assert!(again.contains("Tests") && again.contains("already"), "{again}");
        assert_eq!(revs(&g).len(), n, "過剰側: 2 回目は何も書かない");

        let mid = g.project(&project).unwrap().1;
        // UI-5a（sendback 60.4 A3）: 動いた先の Page へ移る（Dev に居ても Tests へ・負の対照 = finish_edit の MoveSurface の腕を外すと Dev のままで赤）
        g.navigate_page(&project, &dev).unwrap();
        assert_eq!(page_of(&edit(&mut g, "Excalidraw を右へ").unwrap()), tests, "動いた先の Page へ移る");
        let last = revs(&g).last().cloned().unwrap();
        assert_eq!((last.origin.as_str(), last.ops.clone()), ("nl_edit", json!([{ "op": "move_surface", "surface_key": "diagram-surface", "to_page": tests, "parent": "tests-root" }])));
        let moved = g.project(&project).unwrap().1;
        assert_eq!(moved.surface_specs, mid.surface_specs, "面の定義は変えない");
        assert_eq!(g.surface_receipts(&project).unwrap(), receipts_before, "receipt は前後で同じ");
        assert!(edit(&mut g, "天気を教えて").is_err(), "読めない文は 1 行");
        assert_eq!(revs(&g).len(), n + 1);
    }

    /// W16M-3: 「notes.md を Mirror で見る Page を足して」→ origin nl_edit・ops は add_page + add_surface・足した Page へ移る。
    #[test]
    fn w16m3_nl_edit_adds_mirror_page_and_navigates() {
        let mut g = fresh();
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let View::Project { page, .. } = g.page_intent(&project, PageIntent::Edit { text: "notes.md を Mirror で見る Page を足して".into() }).unwrap() else { panic!() };
        let after = g.project(&project).unwrap().1;
        assert_eq!(after.pages.last().map(|pg| (pg.id.as_str(), pg.title.as_str())), Some((page.as_str(), "notes")), "足した Mirror の Page へ移る");
        // 足した面は一番後ろ（M3: feature `mirror` では Docs の TextEdit の席も Mirror なので backend で探さない）
        let spec = after.surface_specs.last().filter(|s| s.backend == project::SurfaceBackendKind::Mirror).expect("足した Mirror の面");
        assert_eq!(spec.target, project::SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) });
        assert!(spec.keep_alive);
        let last = g.store.lock().unwrap().list_project_revisions(&project).unwrap().last().cloned().unwrap();
        assert_eq!(last.origin.as_str(), "nl_edit");
        let ops: Vec<_> = last.ops.as_array().unwrap().iter().map(|o| o["op"].clone()).collect();
        assert_eq!(ops, vec![json!("add_page"), json!("add_surface")]);
        let again = g.page_intent(&project, PageIntent::Edit { text: "notes.md を Mirror で見る Page を足して".into() }).unwrap_err();
        assert!(again.contains("already"), "{again}");
    }

    /// Home の「作る」に「Mirror で見る Page を足して」が来たら、app が 0 件でも拒否せずメモを作ってから足す。既にある app には新しい app を足さない。
    #[test]
    fn home_edit_sentence_applies_to_the_open_app() {
        let mut g = fresh();
        let View::Project { project, page, .. } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        let made = g.project(&project).unwrap().1;
        assert!(made.surface_specs.iter().any(|s| s.backend == project::SurfaceBackendKind::Mirror));
        assert_eq!(made.pages.last().map(|pg| pg.id.as_str()), Some(page.as_str()));
        assert_eq!(g.apps.len(), 1);
        let View::Project { project: same, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let _ = same;
        let before = g.apps.len();
        let View::Project { page, .. } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(g.apps.len(), before);
        let after = g.project(&same).unwrap().1;
        assert!(after.surface_specs.iter().any(|s| s.backend == project::SurfaceBackendKind::Mirror));
        assert_eq!(after.pages.last().map(|pg| pg.id.as_str()), Some(page.as_str()));
    }

    /// M4 ③（W16M-3 の不具合）: Home の「作る」の「Mirror で見る Page を足して」で app が 0 件の時に作る Project も、env の repo の dir（`workspace`）を持つ。
    /// 無いと Mirror は窓を書類の path で選べず（`MirrorPages` の workspace が None）、同じ題名の別の書類の窓（list の先頭）に打つ。
    #[test]
    fn w16m3_home_edit_keeps_the_workspace_for_the_mirror_document() {
        let mut g = fresh();
        let ws = std::env::temp_dir().join(format!("shibubu-m4-ws-{}", storage::new_id()));
        let View::Project { project, .. } = g.create_project("notes.md を Mirror で見る Page を足して", Some(&ws), None).unwrap().unwrap() else { panic!() };
        let made = g.project(&project).unwrap().1;
        assert!(made.surface_specs.iter().any(|s| s.backend == project::SurfaceBackendKind::Mirror));
        assert_eq!(repo_dir(&made), Some(ws), "Mirror の書類の path の基点（repo の dir）");
    }

    /// W8 holdout（interface の commit 9c97093 では `PageIntent::Undo / Redo` と `undo_redo` が todo!() で赤だった・A3 の DB の側・YCM-045）: 「Tests Page を足して」→ Undo で Project の hash・revision・Page の数と page_hash が編集の前と同じ →
    /// Redo で編集の後と同じ。Undo / Redo は project_revisions を書かず receipt を変えない。行き先が無い時は 1 行で何も書かない。
    /// 再起動の後も行き先が同じ。Undo の後に別の編集（＋ Page）を当てると Redo の行き先が消える（truncate が無いと PK で赤）。
    /// 未測: 外の効果（Terminal の `echo <nonce> > undo-probe.txt` の sha が Undo / Redo の前後で同じ・統合の後もこの試験は Terminal を置かない）・画面の AX の木（実機）。
    #[test]
    fn w8_undo_redo_moves_the_head_and_never_writes_a_revision() {
        let dir = std::env::temp_dir().join(format!("shibubu-w8-undo-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let hashes = |p: &ProjectDefinitionV1| p.pages.iter().map(|pg| (pg.id.clone(), page_hash(pg))).collect::<Vec<_>>();
        let revs = |g: &Generated| g.store.lock().unwrap().list_project_revisions(&project).unwrap().len();
        let now = |g: &Generated| {
            let (_, p, h) = g.project(&project).unwrap();
            (p, h)
        };
        assert_eq!(g.undo_redo(&project).unwrap(), UndoRedo { undo: None, redo: None }, "生成のまま（revision 1）は戻せない");
        let nothing = g.page_intent(&project, PageIntent::Undo).unwrap_err();
        assert!(nothing.contains("元に戻せる変更はありません"), "{nothing}");
        let receipts = g.surface_receipts(&project).unwrap();
        let (before, before_hash) = now(&g);
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap());
        let (after, after_hash) = now(&g);
        assert_ne!(after_hash, before_hash, "正の対照: 編集で hash が変わる");
        assert_eq!(g.undo_redo(&project).unwrap(), UndoRedo { undo: Some(before.revision), redo: None });
        let n = revs(&g);

        let v = g.page_intent(&project, PageIntent::Undo).unwrap();
        let (undone, undone_hash) = now(&g);
        assert_eq!(
            (undone.revision, &undone_hash, undone.pages.len(), hashes(&undone)),
            (before.revision, &before_hash, before.pages.len(), hashes(&before)),
            "Undo で編集の前に戻る"
        );
        assert!(page_of(&v) != tests && undone.page(&page_of(&v)).is_some(), "消えた Tests の Page から在る Page へ移る: {v:?}");
        assert_eq!(revs(&g), n, "Undo は revision を書かない・消さない");
        assert_eq!(g.undo_redo(&project).unwrap(), UndoRedo { undo: None, redo: Some(after.revision) });

        g.page_intent(&project, PageIntent::Redo).unwrap();
        let (redone, redone_hash) = now(&g);
        assert_eq!((redone.revision, &redone_hash, hashes(&redone)), (after.revision, &after_hash, hashes(&after)), "Redo で編集の後に戻る");
        let none = g.page_intent(&project, PageIntent::Redo).unwrap_err();
        assert!(none.contains("やり直せる変更はありません"), "{none}");
        assert_eq!((revs(&g), now(&g).1), (n, after_hash.clone()), "行き先が無い Redo は何も書かない");
        assert_eq!(g.surface_receipts(&project).unwrap(), receipts, "receipt は Undo / Redo の前後で同じ");

        g.page_intent(&project, PageIntent::Undo).unwrap();
        drop(g);
        let mut g = Generated::open(&dir);
        assert_eq!(g.undo_redo(&project).unwrap(), UndoRedo { undo: None, redo: Some(after.revision) }, "再起動をまたいでも同じ行き先");
        assert_eq!(now(&g).1, before_hash, "再起動で戻した Project が開く");
        g.open_project(&project, None).unwrap();
        g.page_intent(&project, PageIntent::Add).unwrap();
        assert_eq!(g.undo_redo(&project).unwrap().redo, None, "Undo の後の別の編集で Redo の行き先が消える");
        let rows = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!(rows.iter().map(|r| (r.revision, r.origin.as_str())).collect::<Vec<_>>(), vec![(1, "generate"), (2, "direct_edit")], "行は欠けなく 1..=最後");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8 の画面: `↶` `↷`（AX `Undo · ⌘Z` / `Redo · ⇧⌘Z`）は行き先が在る向きだけ出る（押しても何も起きないボタンを見せない・L2）。`Show terminal`（UI-5a から ••• の menu）は常に在る。
    /// （G92 で引き出しの 1 行を消した。`Show terminal` の中身は `g92_show_terminal_*`）
    #[test]
    fn w8_undo_buttons_show_only_with_a_target() {
        let mut g = fresh();
        let ws = std::env::temp_dir().join(format!("shibubu-w8-ws-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        // UI-5a: ↶ ↷ は painter の形（字が無い）なので AX の名前で見る・Show terminal は ••• の menu の中
        let ax = |g: &mut Generated, page: &str| page_ax(g, &project, page);
        let has = |t: &[String], s: &str| t.iter().any(|x| x == s);
        let (undo, redo) = ("Undo · ⌘Z", "Redo · ⇧⌘Z");
        let menu = menu_ax(&mut g, &project, &dev, "Page options");
        assert!(has(&menu, "Show terminal") && has(&menu, "Page options"), "正の対照: 帯と ••• の menu が描かれていない: {menu:?}");
        let a = ax(&mut g, &dev);
        assert!(!has(&a, undo) && !has(&a, redo), "生成のままに Undo / Redo を出した: {a:?}");
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap());
        let a = ax(&mut g, &tests);
        assert!(has(&a, undo) && !has(&a, redo), "編集の後は Undo だけ: {a:?}");
        let back = page_of(&g.page_intent(&project, PageIntent::Undo).unwrap());
        let a = ax(&mut g, &back);
        assert!(!has(&a, undo) && has(&a, redo), "Undo の後は Redo だけ: {a:?}");
    }

    /// G92: 本番の主領域（`shell::draw_app_region` の scroll）の中に、高さ `h` の窓で Page を描く画面。ctx を frame をまたいで持つ（scroll と focus が続く）。
    #[cfg(feature = "mirror")]
    struct G92Screen {
        ctx: egui::Context,
        rect: egui::Rect,
        time: std::cell::Cell<f64>,
    }

    #[cfg(feature = "mirror")]
    impl G92Screen {
        fn new(h: f32) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            G92Screen { ctx, rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, h)), time: std::cell::Cell::new(0.0) }
        }

        /// `n` frame 描き、最後の frame の AX の木を返す（shell の出力と PTY の大きさの替わりを 20ms ずつ待つ・ctx の時刻は 1 frame 0.1 秒進めて出の動きを終わらせる）。
        fn draw(&self, g: &mut Generated, project: &str, page: &str, n: usize) -> egui::accesskit::TreeUpdate {
            self.draw_with(g, project, page, n, vec![])
        }

        /// `draw` の最初の frame に `events`（pointer・wheel）を足した物。
        fn draw_with(&self, g: &mut Generated, project: &str, page: &str, n: usize, mut events: Vec<egui::Event>) -> egui::accesskit::TreeUpdate {
            let mut last = None;
            for _ in 0..n {
                self.time.set(self.time.get() + 0.1);
                let mut input = egui::RawInput { screen_rect: Some(self.rect), time: Some(self.time.get()), events: std::mem::take(&mut events), ..Default::default() };
                input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { inner_rect: Some(self.rect), ..Default::default() });
                let out = self.ctx.run(input, |ctx| {
                    // 本番は header が毎 frame 主領域より先に View を覚える（替わった frame の送り量と出の動き）
                    shell::note_view(ctx, &View::Project { project: project.to_owned(), page: page.to_owned() });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        shell::draw_app_region(ui, &mut |ui| {
                            g.draw_page(ui, project, page);
                        })
                    });
                });
                last = out.platform_output.accesskit_update;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            last.expect("AccessKit の木が無い")
        }
    }

    /// G92: AX の木で `hit` に当たる最初の node の id と矩形。
    #[cfg(feature = "mirror")]
    fn g92_rect(t: &egui::accesskit::TreeUpdate, hit: impl Fn(&egui::accesskit::Node) -> bool) -> Option<(egui::accesskit::NodeId, egui::Rect)> {
        t.nodes.iter().find(|(_, n)| hit(n)).and_then(|(id, n)| n.bounds().map(|b| (*id, egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))))
    }

    /// G92（operator の実測 10-27: Layout が Tasks の Project を開き直すと、一番下の Terminal まで送られて開き tab の帯・＋・••• が窓の外）:
    /// 下まで送った後に別の Project を開くと、主領域は上から（送り量は全部の View で 1 つ・Home は中身が短いので 0 に戻るが、
    /// rail・⌘K で Project から Project へ移ると持ち越す）。負の対照: `draw_app_region` の「替わった View は送り量 0」を外す → 赤（前の送り量のまま開く）。
    #[cfg(feature = "mirror")]
    #[test]
    #[ignore = "TERM-ONE-1101: Page に Terminal を描かない（plan の Terminal は下の panel の tab）· panel の上で測り直すまで外す"]
    fn g92_opening_a_project_again_starts_at_the_top() {
        // a・b とも Mirror と Terminal の Page（480 の窓で送れる形・g92_scrolling と同じ）。b が送れない Page だと、b を開いた時に
        // 送り量が中身の高さで 0 に詰められ、「替わった View は 0」を外しても緑になる（変異で実測）。
        // 「… Page を足して」は今開いている Project の編集なので、b は主デモの一言で作ってから、その b に Mirror の Page を足す
        let mut g = fresh();
        let View::Project { project: pa, page: ga } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        g.page_intent(&pa, PageIntent::AddTerminal { page: ga.clone() }).unwrap();
        let View::Project { project: pb, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_ne!(pa, pb, "前提: b が a と別の Project になっていない");
        let View::Project { page: gb, .. } = g.page_intent(&pb, PageIntent::Edit { text: "notes.md を Mirror で見る Page を足して".into() }).unwrap() else { panic!() };
        g.page_intent(&pb, PageIntent::AddTerminal { page: gb.clone() }).unwrap();
        let screen = G92Screen::new(480.0);
        let down = || vec![egui::Event::PointerMoved(egui::pos2(720.0, 300.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -2000.0), modifiers: egui::Modifiers::NONE }];
        // 足した Terminal まで送る 1 回（reveal）を先に b で済ませる（残っていると b を開いた frame の後に Terminal まで少し送り、0 にならない・実測 1.36）
        g.open_project(&pb, Some(&gb)).unwrap();
        let _ = screen.draw(&mut g, &pb, &gb, 8);
        g.open_project(&pa, Some(&ga)).unwrap();
        let _ = screen.draw(&mut g, &pa, &ga, 8);
        let _ = screen.draw_with(&mut g, &pa, &ga, 4, down());
        // wheel の送りは egui が数 frame に分けて流す（滑らかな送り）→ 流し終えてから開く（残りが b の frame に流れて 0 にならない・実測 1.36）
        let _ = screen.draw(&mut g, &pa, &ga, 40);
        let sent = shell::main_scrolled(&screen.ctx);
        assert!(sent > 20.0, "前提: a の Page が wheel で送れていない（{sent}）");
        g.open_project(&pb, Some(&gb)).unwrap();
        let _ = screen.draw(&mut g, &pb, &gb, 4);
        assert_eq!(shell::main_scrolled(&screen.ctx), 0.0, "別の Project が前の送り量（{sent}）のまま開いた");
        // 開いた後は wheel で送れる（替わった frame だけ 0・毎 frame 0 に戻さない）・b を送ったまま a を開き直しても上から（operator の実測の形）
        let _ = screen.draw_with(&mut g, &pb, &gb, 4, down());
        let _ = screen.draw(&mut g, &pb, &gb, 40);
        let sent_b = shell::main_scrolled(&screen.ctx);
        assert!(sent_b > 20.0, "開いた後に wheel で送れない（{sent_b}）");
        g.open_project(&pa, Some(&ga)).unwrap();
        let _ = screen.draw(&mut g, &pa, &ga, 4);
        assert_eq!(shell::main_scrolled(&screen.ctx), 0.0, "開き直した a が b の送り量（{sent_b}）のまま開いた");
    }

    /// G92: 主領域を wheel で送っても Mirror の枠と Terminal の欄は伸びない（窓の残りは送る前の位置で測る・`shell::main_scrolled`）。
    /// 伸びると送った分だけ部品が窓の下へ逃げ、送るたびにまた逃げる。負の対照: 残りの高さから送り量を引くのを外す → 赤（送った分だけ伸びる）。
    #[cfg(feature = "mirror")]
    #[test]
    #[ignore = "TERM-ONE-1101: Page に Terminal を描かない（plan の Terminal は下の panel の tab）· panel の上で測り直すまで外す"]
    fn g92_scrolling_the_page_does_not_grow_the_mirror_or_the_terminal() {
        let mut g = fresh();
        let View::Project { project, page } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let screen = G92Screen::new(480.0);
        let _ = screen.draw(&mut g, &project, &page, 2);
        g.page_intent(&project, PageIntent::AddTerminal { page: page.clone() }).unwrap();
        let spec = g.project(&project).unwrap().1.terminal_specs.last().cloned().expect("足した Terminal");
        let measure = |t: &egui::accesskit::TreeUpdate| {
            let (_, band) = g92_rect(t, |n| n.label() == Some("Close mirror")).expect("Mirror の題の帯");
            let (_, grid) = g92_rect(t, |n| n.author_id() == Some(crate::ui::ax::AxScope::Terminal.id(&spec.id).as_str())).expect("Terminal の欄");
            (band.min.y, grid.min.y - band.min.y, grid.height())
        };
        // 足した Terminal まで送った後（reveal）・さらに wheel で一番下まで・一番上まで
        let (top0, gap0, grid0) = measure(&screen.draw(&mut g, &project, &page, 8));
        let wheel = |dy: f32| vec![egui::Event::PointerMoved(egui::pos2(720.0, 300.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, dy), modifiers: egui::Modifiers::NONE }];
        let (top1, gap1, grid1) = measure(&screen.draw_with(&mut g, &project, &page, 10, wheel(-2000.0)));
        let (top2, gap2, grid2) = measure(&screen.draw_with(&mut g, &project, &page, 10, wheel(2000.0)));
        assert!(top2 > top1 + 20.0, "前提: wheel で主領域が送れていない（帯の上端 {top0} → {top1} → {top2}）");
        assert!((gap1 - gap0).abs() < 1.0 && (gap2 - gap0).abs() < 1.0, "送ると Mirror の枠が伸びた（帯から欄まで {gap0} → {gap1} → {gap2}）");
        assert!((grid1 - grid0).abs() < 1.0 && (grid2 - grid0).abs() < 1.0, "送ると Terminal の欄が伸びた（{grid0} → {grid1} → {grid2}）");
    }

    /// G92 → G95: `•••` の `Show terminal` は Page の Terminal を見せる。無ければ Page に置かず下の panel を開いて tab を 1 つ足す（G95）。
    /// どちらでも Project（hash）と Page の Terminal の数を変えず、2 回目は shell も増やさない。「この版では開けない」という嘘の文はどの frame の AX の木にも無い。
    /// 負の対照: draw_page の終わりに旧い引き出しの 1 行（`states::inline(.., Disconnected, ..)`）を戻す → 赤・無い時の腕を `AddTerminal` に戻す → 数が増えて赤。
    #[cfg(feature = "mirror")]
    #[test]
    fn g92_show_terminal_opens_the_page_terminal_and_tells_no_lie() {
        let mut g = fresh();
        let View::Project { project, page } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let screen = G92Screen::new(928.0);
        let _ = screen.draw(&mut g, &project, &page, 2);
        let count = |g: &Generated| g.project(&project).unwrap().1.terminal_specs.len();
        let before = count(&g);
        let (_, _, hash) = g.project(&project).unwrap();
        g.page_intent(&project, PageIntent::ShowTerminal { page: page.clone() }).unwrap();
        let first = screen.draw(&mut g, &project, &page, 6);
        assert_eq!((count(&g), g.project(&project).unwrap().2.clone()), (before, hash.clone()), "Terminal の無い Page の Show terminal が Page に置いた（G95: panel を開く）");
        assert!(g.panel(&project).open && g.panel(&project).tabs.len() == 1, "panel が開いていない");
        let shells = g.terminals.list(Some(&project)).len();
        g.page_intent(&project, PageIntent::ShowTerminal { page: page.clone() }).unwrap();
        let again = screen.draw(&mut g, &project, &page, 3);
        assert_eq!((g.project(&project).unwrap().2, count(&g), g.terminals.list(Some(&project)).len()), (hash, before, shells), "2 回目の Show terminal が Project か shell を増やした");
        for tree in [&first, &again] {
            let lie: Vec<&str> = tree.nodes.iter().flat_map(|(_, n)| [n.label(), n.value()]).flatten().filter(|s| s.contains("can't open a terminal")).collect();
            assert!(lie.is_empty(), "嘘の文が描かれた: {lie:?}");
        }
    }

    /// FX6（FX-033）: `Run tests` は専用の session に打つ。Page に置いた Terminal の shell とは別の id で、Project の `terminal_specs` に入らず、
    /// Terminal の席に依らず走る（前は Terminal が 0 件だと `No terminal in this project · Add terminal` で拒否した）。repo の無い Project は拒否。
    /// 負の対照（commit message）: `tests_spec` の id を `p.terminal_specs[0].id` に戻す → 1 つ目の assert が赤。
    #[test]
    fn fx6_run_tests_has_its_own_session() {
        let mut g = fresh();
        let ws = std::env::temp_dir().join(format!("shibubu-fx6-tests-{}", storage::new_id()));
        std::fs::create_dir_all(&ws).unwrap();
        let View::Project { project, page } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        g.page_intent(&project, PageIntent::AddTerminal { page: page.clone() }).unwrap();
        let p = g.project(&project).unwrap().1;
        let spec = tests_spec(&p).expect("repo の folder が在る");
        assert!(p.terminal_specs.iter().all(|t| t.id != spec.id), "Tests が利用者の Terminal の session に打つ: {} / {:?}", spec.id, p.terminal_specs);
        assert_eq!((spec.project_id.as_str(), spec.working_directory.as_str()), (p.id.as_str(), g.terminal_cwd(&p).as_str()));
        let result = g.page_intent(&project, PageIntent::RunTests { command: "true".into() });
        assert!(result.is_ok(), "Run tests が拒否された: {result:?}");
        let mut bare = fresh();
        let View::Project { project: other, .. } = bare.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(bare.page_intent(&other, PageIntent::RunTests { command: "true".into() }).unwrap_err(), NO_REPO_LINE, "repo の無い Project は $HOME で走らせない");
    }

    /// FX6（FX-030・UIUX G06）: AI 無しで作った全部の button（操作を持つ node）は「押せる」か「押せない形 + 理由の 1 行」のどちらか。
    /// 押せない（gate が Err）のに理由が空の button は、押しても何も起きない飾りになる。
    /// 負の対照（commit message）: `bindings::input_reason` の `Enter “{l}” to enable this` を空文字にする → 赤。
    #[test]
    fn fx6_every_generated_button_is_pressable_or_says_why_not() {
        let mut g = fresh();
        for goal in ["読書記録をつけたい", "在庫を整理したい", "家計を見える化したい", "タイマーを作って", "a reading log", "旅行の計画を立てたい", "バグの報告をまとめたい", "毎朝の英単語を覚えたい"] {
            let id = g.create(goal).unwrap();
            let (def, _, h) = g.open_app(&id).expect("開いた app");
            let nodes = def.composition.root.nodes();
            let buttons: Vec<_> = nodes.iter().filter(|n| n.action_ref.is_some()).collect();
            assert!(!buttons.is_empty(), "{goal}: 操作が 1 つも無い");
            for n in buttons {
                if let Err(gate) = crate::ui::renderer::bindings::gate(&def, n, &h, &json!({})) {
                    assert!(!gate.text().trim().is_empty(), "{goal}: {} が押せないのに理由が無い（飾りの button）", n.key);
                }
            }
        }
    }

    /// DOGFOOD-X 周 7 #4: 名前の表で作る English の長い一言。名前は利用者の書いた形の最初の句（一言を小文字にした全文でない）・足す手は欄つきの form で、
    /// 足す手（`.add` の node）に「No field on this screen sets the input」（画面に無い欄を要る = 押す前から壊れた Add）が無い（organize・monitor・plan も）。
    /// 負の対照（束）: organize の `entry` を外して `Add` の button に戻す → 2 つ目が赤・`subject_of` の `short_name` を外す → 名前で赤。
    #[test]
    fn r7_simple_rules_names_it_short_and_every_add_has_its_fields() {
        const SAID: &str = "Put my Zoom recording zoom-test-recording.mp4 into iMovie, cut the first 10 seconds and export it";
        let mut g = fresh();
        let id = g.create(SAID).expect("名前の表で作る");
        let (def, ..) = g.open_app(&id).expect("開いた app");
        assert_eq!(def.title.as_deref(), Some("Put my Zoom recording zoom-test-recording.mp4 into iMovie"), "名前が一言の全文か小文字");
        for goal in [SAID, "在庫を整理したい", "仕事の状況を把握したい", "旅行の計画を立てたい", "Track my inventory"] {
            let id = g.create(goal).unwrap();
            let (def, _, h) = g.open_app(&id).expect("開いた app");
            // 足す手だけ（選んだ 1 件の手〈変える・済み・削除〉は item_id を選択から渡すので、{} で gate すると欄の無い item_id を言う = 別の話）
            let adds: Vec<_> = def.composition.root.nodes().into_iter().filter(|n| n.action_ref.as_deref().is_some_and(|a| a.ends_with(".add"))).collect();
            assert!(!adds.is_empty(), "{goal}: 足す手が無い");
            for n in adds {
                if let Err(gate) = crate::ui::renderer::bindings::gate(&def, n, &h, &json!({})) {
                    assert!(!gate.text().contains("No field on this screen"), "{goal}: {} は画面に無い欄を要る: {}", n.key, gate.text());
                }
            }
        }
    }

    /// FX7（FX-043・UIUX N01）: 外へ出る手の button が渡す idempotency key は、同じ button の同じ中身を短い間に 2 回押すと同じ（runtime が 2 回目を送り直さない）・
    /// 中身が違う・button が違う・時間の区切りを越えたら別。runtime が同じ key の 2 回目に 1 回目の Receipt を返す事は tests/runtime.rs が縛る。
    /// 負の対照（commit message）: key に毎回違う値を混ぜる → 1 つ目の assert が赤。
    #[test]
    fn fx7_press_key_is_the_same_for_a_double_press_and_differs_otherwise() {
        let a = json!({ "to": "x", "body": "hi" });
        let t = 1_000_000_u64;
        assert_eq!(press_key("send", &a, t), press_key("send", &a, t + 1), "2 回押しても key が違う");
        assert_ne!(press_key("send", &a, t), press_key("send", &json!({ "to": "x", "body": "bye" }), t), "中身が違うのに同じ key");
        assert_ne!(press_key("send", &a, t), press_key("other", &a, t), "button が違うのに同じ key");
        assert_ne!(press_key("send", &a, t), press_key("send", &a, t + 3 * PRESS_WINDOW_SECS), "時間の区切りを越えても同じ key（正当な再送が止まる）");
        // REVIEW-FX2 a1: 時計の区切り（5 | 6 は 5 / 3 = 1・6 / 3 = 2）をまたぐ 2 度押しも同じ key
        assert_eq!(press_key("edge", &a, 5), press_key("edge", &a, 6), "区切りをまたぐ 2 度押しで key が替わる");
    }

    /// CU 所見 1: 使えない答えが続く AI の失敗は、1 回目 `Try again`・2 回目から `Use simple rules`（Project を作れない行き止まりにしない）。他の失敗の手と、使えない答えでない失敗は変えない。
    /// 負の対照（commit message）: `after_unusable` の `count >= 2` の腕を外す → 赤。
    #[test]
    fn cu1_unusable_answers_offer_simple_rules_from_the_second_time() {
        use crate::ui::build_progress::BuildAction as A;
        assert_eq!(after_unusable(Some(A::Retry), true, 1), Some(A::Retry));
        assert_eq!(after_unusable(Some(A::Retry), true, 2), Some(A::UseSimpleRules), "2 回目も Try again だけ");
        assert_eq!(after_unusable(Some(A::Retry), false, 5), Some(A::Retry), "Timeout などは何回でも Try again");
        assert_eq!(after_unusable(Some(A::Rephrase), true, 9), Some(A::Rephrase));
        assert_eq!(after_unusable(None, true, 9), None);
    }

    /// G94 ①: 録音の帯の `Open System Settings`・`Reveal in Finder` は `open` の exit code を見る（失敗なら帯の 1 行になる Err・成功は Ok）。
    /// 負の対照: `run_open` の `Ok(s) if s.success()` を `Ok(_)` に戻す → 赤。
    #[test]
    fn g94_open_failure_is_an_error_line() {
        assert_eq!(run_open("/usr/bin/true", &[]), Ok(()));
        let e = run_open("/usr/bin/false", &[]).unwrap_err();
        assert!(e.starts_with("Couldn't open it") && e.contains("exit code 1"), "{e}");
        assert!(run_open("/nonexistent/open", &[]).unwrap_err().starts_with("Couldn't open it"));
    }

    /// G94 ③: `Layout: {name}` は今の Page に効く型だけを回す。dev の型は Dev / Docs / Diagram / Mirror の Page 用なので、`Page 4` では回さない（label だけ替わり画面が動かなかった）。
    /// 負の対照: `placement_ids` の `applies_to` の絞りを外す → 赤（`Page 4` の候補と回した先に dev が入る）。
    #[test]
    fn g94_layout_cycles_only_templates_that_fit_the_page() {
        let mut g = fresh();
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let View::Project { page: added, .. } = g.page_intent(&project, PageIntent::Add).unwrap() else { panic!() };
        let p = g.project(&project).unwrap().1;
        let dev_template = composer::layout::bundled_templates().into_iter().find(|t| t.id == "dev").expect("dev の型");
        assert!(composer::layout::applies_to(&dev_template, p.page(&dev).unwrap()), "dev の型が Dev の Page に効かない");
        assert!(!composer::layout::applies_to(&dev_template, p.page(&added).unwrap()), "dev の型が Page 4 に効く");
        assert!(placement_ids(&p, p.page(&dev)).contains(&"dev".to_owned()) && !placement_ids(&p, p.page(&added)).contains(&"dev".to_owned()));
        assert!(placement_ids(&p, None).is_empty());
        for _ in 0..6 {
            g.page_intent(&project, PageIntent::Placement).unwrap();
            assert_ne!(g.placement(&project).unwrap().map(|t| t.id), Some("dev".to_owned()), "効かない型へ回した");
        }
    }

    /// G96（operator の実測: qa 8f3fcb80 で `•••` → `Layout: …` を AX の press で押すと 3 回とも exit 101・accesskit_consumer 0.26.0 tree.rs:306 の unwrap）:
    /// Layout を替えると Page の部品が別の region へ移る（AX の木で親が替わる）。0.26.0 は新しい親が前の親より先に来る順だと移った node を孤児として消し、
    /// 同じ更新の updated で unwrap が None。Mac の AX の道具（VoiceOver・BG の driver の press）が繋がっている時だけ adapter が木を読むので、mouse だけでは落ちない。
    /// 直し = vendor/accesskit_consumer（上流 #576 の写し・Cargo.toml の [patch.crates-io]）。`•••` → `Layout: …` を AX の press で 6 回押して毎 frame の木を consumer に渡しても落ちない・
    /// 押した手が Layout を替える。**この試験は落ちる形を通らない**（試験の画面では Layout を替えても親の替わる node が 0 = 上流の 0.26.0 でも落ちない）。
    /// 落ちる形（新しい親が前の親より先に来る更新）は vendor の `shibubu_reparent_before_old_parent_keeps_the_node` が持つ（負の対照: 写しの直しを外す → 赤）。
    /// 実機の再現（operator）: Project「Repo dev environment」→ `•••` → `Layout: …` を computer use の press で 5 回・patch の前は 3 回のうち 3 回 exit 101。
    #[test]
    fn g96_changing_the_layout_does_not_crash_the_accessibility_tree() {
        struct Nop;
        impl accesskit_consumer::TreeChangeHandler for Nop {
            fn node_added(&mut self, _node: &accesskit_consumer::Node) {}
            fn node_updated(&mut self, _old: &accesskit_consumer::Node, _new: &accesskit_consumer::Node) {}
            fn focus_moved(&mut self, _old: Option<&accesskit_consumer::Node>, _new: Option<&accesskit_consumer::Node>) {}
            fn node_removed(&mut self, _node: &accesskit_consumer::Node) {}
        }
        let mut g = fresh();
        let View::Project { project, page } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0));
        let mut time = 0.0;
        // 1 frame（AX の action を足せる）→ 押された Intent と AX の木
        let mut frame = |g: &mut Generated, events: Vec<egui::Event>| {
            time += 0.1;
            let input = egui::RawInput { screen_rect: Some(rect), time: Some(time), events, ..Default::default() };
            let mut got = None;
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    shell::draw_app_region(ui, &mut |ui| {
                        got = got.take().or(g.draw_page(ui, &project, &page));
                    })
                });
            });
            (got, out.platform_output.accesskit_update.expect("AccessKit の木が無い"))
        };
        let parents = |u: &egui::accesskit::TreeUpdate| u.nodes.iter().flat_map(|(id, n)| n.children().iter().map(move |c| (*c, *id))).collect::<std::collections::HashMap<_, _>>();
        let labels = |u: &egui::accesskit::TreeUpdate| u.nodes.iter().filter_map(|(id, n)| n.label().map(|l| (*id, l.to_owned()))).collect::<Vec<_>>();
        let (_, first) = frame(&mut g, vec![]);
        let (mut before, mut seen) = (parents(&first), labels(&first));
        let mut now = seen.clone();
        let mut tree = accesskit_consumer::Tree::new(first, false);
        let (mut moved, mut layouts) = (0, 0);
        // 毎 frame の木を consumer に渡す（adapter と同じ）・親の替わった node を数える
        let mut step = |g: &mut Generated, events: Vec<egui::Event>| {
            let (got, u) = frame(g, events);
            let now = parents(&u);
            moved += now.iter().filter(|(c, p)| before.get(*c).is_some_and(|b| b != *p)).count();
            (before, seen) = (now, labels(&u));
            tree.update_and_process_changes(u, &mut Nop);
            (got, seen.clone())
        };
        let press = |seen: &[(egui::accesskit::NodeId, String)], hit: &dyn Fn(&str) -> bool| {
            let target = seen.iter().find(|(_, l)| hit(l)).unwrap_or_else(|| panic!("押す項目が AX の木に無い: {:?}", seen.iter().map(|(_, l)| l).collect::<Vec<_>>())).0;
            vec![egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target, data: None })]
        };
        for _ in 0..6 {
            // `•••` を AX で開く → 次の frame の menu の `Layout: …` を AX で押す（押した frame で menu が閉じ、Page の部品が別の region へ移る）
            step(&mut g, press(&now, &|l| l == "Page options"));
            let (_, menu) = step(&mut g, vec![]);
            let (got, _) = step(&mut g, press(&menu, &|l| l.starts_with("Layout: ")));
            if let Some(Intent::Page(pi)) = got {
                g.page_intent(&project, pi).unwrap();
                layouts += 1;
            }
            for _ in 0..3 {
                now = step(&mut g, vec![]).1;
            }
        }
        drop(step);
        assert_eq!(layouts, 6, "AX の press で Layout が替わらない");
        // 落ちる形を通ったか（今は 0・上の doc）。通るようになったら patch の負の対照がこの試験でも赤になる
        eprintln!("g96: 親の替わった node {moved}");
    }

    /// G94 ③: ✦ rail の `About` は Project の画面では今開いている Project の事（Name・Pages の数・Folder）を言う。Project でない画面は None（旧 canvas の Session の行）。
    /// 負の対照: `project_about_rows` の Pages を外す・`about_rows` が常に None → 赤。
    #[test]
    fn g94_about_is_about_the_open_project() {
        let mut g = fresh();
        let View::Project { project, page } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let p = g.project(&project).unwrap().1;
        let rows = g.about_rows(&View::Project { project: project.clone(), page }).expect("Project の画面の About");
        let get = |k: &str| rows.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        assert_eq!(get("Name"), Some(p.name.clone()));
        assert_eq!(get("Pages"), Some(p.pages.len().to_string()));
        assert!(get("Folder").is_some());
        assert!(g.about_rows(&View::Home).is_none(), "Project でない画面でも Project の行");
    }

    /// G92: BG の driver が引く名前（scripts/qa/bg/labels.py）と製品の名前が同じ: `Add terminal`・`Show terminal`（menu の項目）。
    /// TERM-ONE-1101: terminal ごとの入力欄（`Type in the terminal`）は消した。BG の driver（rehearse.py の `h_term_type`）は grid に打てない（窓が key の時だけ PTY へ送る）ので手 #3 / #4 は未測になる（diagrams §54）。
    #[test]
    fn g92_terminal_names_match_the_driver_labels() {
        let labels = include_str!("../scripts/qa/bg/labels.py");
        for name in ["Add terminal", "Show terminal"] {
            assert!(labels.contains(&format!("\"{name}\"")), "labels.py に {name} が無い");
        }
        let mut g = fresh();
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        let p = g.project(&project).unwrap().1;
        let (add, options) = page_menus(&p, &page, &g.undo_redo(&project).unwrap(), "None");
        let names: Vec<&str> = add.iter().chain(&options).map(|c| c.label.as_str()).collect();
        assert!(names.contains(&"Add terminal") && names.contains(&"Show terminal"), "{names:?}");
        assert!(!include_str!("ui/renderer/components.rs").contains("\"Type in the terminal\""), "terminal ごとの入力欄が戻った（TERM-ONE-1101）");
    }

    /// G95 用: rc を読まない shell（`-f`）の Generated と、Page に Terminal の無い Project（読書記録）。
    fn g95_project() -> (Generated, String, String) {
        let mut g = fresh().with_terminal_config(TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..TerminalConfig::default() });
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        let p = g.project(&project).unwrap().1;
        assert!(!p.page(&page).unwrap().root.nodes().iter().any(|n| n.component == "terminal"), "前提: Page に Terminal が無い");
        (g, project, page)
    }

    /// G95: `＋` の `Add terminal` は Page でなく下の panel に tab を足す（Project の hash と `terminal_specs` は変えない・shell は動く・cwd = Project の folder）。
    /// 変異: `page_menus` の `Add terminal` を `AddTerminal { page }` に戻す → 赤・`panel_act` の Spawn を外す → shell が無く赤。
    #[test]
    fn g95_add_terminal_goes_to_the_panel_not_the_page() {
        let (mut g, project, page) = g95_project();
        let (_, before, hash) = g.project(&project).unwrap();
        let (add, _) = page_menus(&before, &page, &g.undo_redo(&project).unwrap(), "None");
        let item = add.iter().find(|c| c.label == "Add terminal").expect("＋ の Add terminal");
        assert_eq!(item.intent, Intent::Page(PageIntent::TerminalPanel(PanelAct::Add)));
        g.page_intent(&project, PageIntent::TerminalPanel(PanelAct::Add)).unwrap();
        let (_, after, hash2) = g.project(&project).unwrap();
        assert_eq!((hash2, after.terminal_specs.len()), (hash, before.terminal_specs.len()), "Page に Terminal を置いた");
        let panel = g.panel(&project).clone();
        assert!(panel.open && panel.tabs.len() == 1, "{panel:?}");
        assert_eq!(panel.tabs[0].cwd, g.terminal_cwd(&after));
        let key = SessionKey { project_id: project.clone(), session_id: panel.tabs[0].id.clone() };
        assert_eq!(g.terminals.status(&key).unwrap(), SessionStatus::Running, "足した tab の shell が動いていない");
    }

    /// G95: `•••` の `Show terminal` は Page に Terminal が無ければ panel を開く（1 つ足す・2 回目は足さない）。Project は変えない。
    /// 変異: ShowTerminal の無い時の腕を `AddTerminal` に戻す → terminal_specs が増えて赤。
    #[test]
    fn g95_show_terminal_without_a_page_terminal_opens_the_panel() {
        let (mut g, project, page) = g95_project();
        let (_, before, hash) = g.project(&project).unwrap();
        g.page_intent(&project, PageIntent::ShowTerminal { page: page.clone() }).unwrap();
        let (_, after, hash2) = g.project(&project).unwrap();
        assert_eq!((hash2, after.terminal_specs.len()), (hash, before.terminal_specs.len()), "Page に Terminal を置いた");
        let panel = g.panel(&project).clone();
        assert!(panel.open && panel.tabs.len() == 1, "{panel:?}");
        g.page_intent(&project, PageIntent::ShowTerminal { page }).unwrap();
        assert_eq!(g.panel(&project).tabs.len(), 1, "2 回目は足さない");
    }

    /// G95 → TERM-RESTORE-1101（owner 189）: 再起動の後、panel の tab は名前・cwd・並びのまま戻り、tab を描くと同じ session id の新しい shell が起き、
    /// 前の画面（終わる時に保存した字）と線がその上に在る。`Restart` を押させない。
    /// 変異: `panel_act` の保存（put_ui_state）を外す → 開き直しで tab が空で赤・`close_terminals` の `save_snapshots` を外す → 前の字が無く赤・
    /// `ensure_spec` を前の Unavailable に戻す → accepts_input が false で赤。
    #[test]
    fn g95_panel_tabs_come_back_with_the_last_screen_after_relaunch() {
        let dir = w1_dir("g95-relaunch");
        let mut g = Generated::open(&dir);
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        g.panel_act(&project, PanelAct::Add).unwrap();
        g.panel_act(&project, PanelAct::Add).unwrap();
        let first = g.panel(&project).tabs[0].id.clone();
        g.panel_act(&project, PanelAct::Rename { tab: first.clone(), name: "agent".into() }).unwrap();
        g.panel_act(&project, PanelAct::Move { tab: first.clone(), to: 1 }).unwrap();
        let saved = g.panel(&project).clone();
        assert_eq!((saved.tabs[1].id.as_str(), saved.tabs[1].name.as_str()), (first.as_str(), "agent"));
        let key = SessionKey { project_id: project.clone(), session_id: saved.tabs[0].id.clone() };
        g.terminals.write(&key, b"echo g95-restore-$((6*7))\n").unwrap();
        let end = std::time::Instant::now() + patient(std::time::Duration::from_secs(30));
        while !g.terminals.screen(&key, 200).unwrap().text().contains("g95-restore-42") {
            assert!(std::time::Instant::now() < end, "echo の出力が出ない");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(g);
        let mut g = Generated::open(&dir);
        g.open_project(&project, Some(&page)).unwrap();
        let back = g.panel(&project).clone();
        assert_eq!((back.tabs.clone(), back.open), (saved.tabs.clone(), true), "tab の名前・cwd・並びが戻らない");
        let specs = back.tabs.iter().map(|t| panel_spec(&project, t)).collect();
        let Generated { terminals, store, term_views, .. } = &mut g;
        let mut host = TermHost { mgr: terminals, store, views: term_views, specs, view: InputView::Drawer, picked: None };
        let pane = host.pane(&back.tabs[0].id).expect("tab の本体");
        assert!(pane.accepts_input && !pane.status_line.contains("Restart"), "開き直した tab に shell が無い: {}", pane.status_line);
        let screen = pane.screen.expect("tab の画面");
        assert!(screen.text().contains("g95-restore-42") && screen.rule.is_some(), "前の画面と線が無い: {:?}", screen.text());
        assert_eq!(g.terminals.list(Some(&project)).len(), 1, "描いた tab だけに shell（もう 1 つの tab は選んだ時）");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ACCOUNTS-1101 2 段目（interface・呼ぶ側の形）: Project ごとに AI の account を覚え（System default と「覚えていない」を分ける）、開き直しても残り、複製には写らない。
    /// 負の対照（commit message）: 複製の filter から `ACCOUNT_KEY` を外す → 複製が元の account を持って赤・sessions の行を先に作らない → foreign_keys=ON の store で書けず赤。
    #[test]
    fn accounts2_each_project_remembers_its_ai_account() {
        use crate::model::AiAccount;
        let dir = w1_dir("accounts2-project");
        let open = |g: &mut Generated, goal: &str| {
            let app = g.create(goal).unwrap();
            let View::Project { project, .. } = g.project_view_of(&app).unwrap() else { panic!() };
            project
        };
        let mut g = Generated::open(&dir);
        let (a, b, c) = (open(&mut g, "読書記録をつけたい"), open(&mut g, "家計簿をつけたい"), open(&mut g, "日記を書きたい"));
        assert_eq!(g.project_account(&a), None, "選んでいない Project が account を覚えている");
        g.set_project_account(&a, Some(&AiAccount::Plan { id: "plan-1".into() })).unwrap();
        g.set_project_account(&b, None).unwrap();
        let plan = Some(Some(("plan".to_owned(), "plan-1".to_owned())));
        assert_eq!(g.project_account(&a), plan, "選んだ account を覚えない");
        assert_eq!(g.project_account(&b), Some(None), "System default と覚えていないを分けない");
        assert_eq!(g.project_account(&c), None, "別の Project の account が混ざった");
        let View::Project { project: copy, .. } = g.duplicate_project(&a).unwrap() else { panic!() };
        assert_eq!(g.project_account(&copy), None, "複製に account を写した");
        drop(g);
        let g = Generated::open(&dir);
        assert_eq!(g.project_account(&a), plan, "開き直したら忘れた");
    }

    /// PROJECT-DIR-1101（owner 204）: Project ごとに自分の folder（`projects_root/<名前>`・空）が出来て、下の panel の Terminal はそこで始まる。
    /// 同じ名前の 2 つ目は ` 2`・名前を替えても動かない・消された folder は同じ path に作り直す・repo を当てた Project は repo・
    /// 前の起動の既定（HOME）で始まっていた tab は自分の folder へ・`projects_root` の無い起動（試験）は作らない。
    /// 変異: 保存（`FOLDER_KEY`）を外す → 名前を替えた後に別の folder で赤・` 2` を外す → 2 つ目が 1 つ目と同じで赤・`load_panel` の HOME の置き換えを外す → 赤。
    /// 変異（owner 210 の 3・FK の直し）: 置き場を前の `ui_state("project-folder", Project の id)` に戻す → foreign_keys=ON の store（`Generated::open`）で書けず最初の assert で赤。
    #[test]
    fn project_dir_each_project_gets_its_own_folder_and_its_terminals_start_there() {
        let dir = w1_dir("project-dir");
        let root = dir.join("Shibubu");
        let home = std::env::var("HOME").unwrap_or_default();
        let open = |g: &mut Generated, goal: &str| {
            let app = g.create(goal).unwrap();
            let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
            g.open_project(&project, Some(&page)).unwrap();
            project
        };
        let mut g = Generated::open(&dir);
        g.projects_root = Some(root.clone());
        let a = open(&mut g, "読書記録をつけたい");
        let name = g.project(&a).unwrap().1.name;
        let mine = root.join(safe_dir_name(&name));
        assert_eq!(g.project_folder(&a), Some(mine.clone()), "開いた Project に自分の folder が無い");
        assert!(mine.is_dir() && std::fs::read_dir(&mine).unwrap().next().is_none(), "folder を作らない・中に何か置いた");
        // chat bar の folder の名前は自分の folder・AI の会話の頭（`Their repo`）には渡さない（repo ではない）
        assert_eq!((g.folder_name(&a), g.repo_name(&a)), (mine.file_name().map(|n| n.to_string_lossy().into_owned()), None), "chat bar に自分の folder が無い・AI に repo と渡した");
        g.panel_act(&a, PanelAct::Add).unwrap();
        let tab = g.panel(&a).tabs[0].clone();
        assert_eq!(tab.cwd, mine.display().to_string(), "Terminal が自分の folder で始まらない");
        let b = open(&mut g, "読書記録をつけたい");
        assert_eq!(g.project_folder(&b), Some(root.join(format!("{} 2", safe_dir_name(&name)))), "同じ名前の 2 つ目が ` 2` の folder でない");
        g.rename_project(&a, "Hum to Guitar").unwrap();
        std::fs::remove_dir(&mine).unwrap();
        assert_eq!(g.project_folder(&a), Some(mine.clone()), "名前を替えたら folder が動いた");
        assert!(mine.is_dir(), "消された folder を作り直さない");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        g.page_intent(&b, PageIntent::SetWorkspace { dir: repo.display().to_string() }).unwrap();
        assert_eq!(g.project_folder(&b), Some(repo.clone()), "repo を当てた Project が repo でない");
        // 前の起動の既定（HOME）で始まっていた tab → 次に開いた時は自分の folder
        g.panels.get_mut(&a).unwrap().tabs[0].cwd = home.clone();
        g.panel_act(&a, PanelAct::Rename { tab: tab.id.clone(), name: "old".into() }).unwrap();
        drop(g);
        let mut g = Generated::open(&dir);
        g.projects_root = Some(root.clone());
        g.open_project(&a, None).unwrap();
        assert_eq!(g.project_folder(&a), Some(mine.clone()), "開き直した後に保存した folder を読み戻せない（store に書けていない）");
        assert_eq!(g.panel(&a).tabs[0].cwd, mine.display().to_string(), "HOME で始まっていた tab が自分の folder に替わらない");
        drop(g);
        // projects_root の無い起動（試験）は作らない
        let mut g = Generated::open(&dir.join("no-root"));
        let c = open(&mut g, "読書記録をつけたい");
        assert_eq!(g.project_folder(&c), None);
        assert_eq!(safe_dir_name(" a/b:c "), "a-b-c");
        assert_eq!(safe_dir_name("..hidden"), "hidden");
        assert_eq!(safe_dir_name(" / "), "Project");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L7-ORCA #12: agent の tab は shell に最初にその agent を打つ（spec の `command`）。`AGENTS` に無い名前（保存の行を書き換えた等）は打たず shell だけ。
    /// 変異: panel_spec の `command` を None に戻す → 赤・`AGENTS` の照合を外す → 知らない command を打って赤。
    #[test]
    fn l7_agent_tab_types_only_a_known_agent() {
        let tab = terminal_panel::PanelTab { id: "a".into(), name: "codex 1".into(), cwd: "/tmp".into(), command: Some("codex".into()), named: false };
        assert_eq!(panel_spec("p", &tab).command.as_deref(), Some("codex"), "agent を打たない");
        let bad = terminal_panel::PanelTab { command: Some("rm -rf ~".into()), ..tab.clone() };
        assert_eq!(panel_spec("p", &bad).command, None, "AGENTS に無い command を打つ");
        // SIGNIN-1101: sign in の tab は main が渡す口（`Startup::sign_in_command` = `llm_cli::sign_in_command`・表の照合と置き場の解きはその試験）で解く。
        // ここは表の 1 行だけを解く偽の口（`STARTUP` は process に 1 つ = 試験で入れると他の試験の口も替わるので、`tab_command` に直に渡す）。
        // AGENTS に無い command をそのまま打たず口に渡すか・口が無い（試験の crate）なら打たないかを見る
        let fake: fn(&str) -> Option<String> = |c| (c == "kimi login").then(|| "~/.kimi-code/bin/kimi login".to_owned());
        assert_eq!(tab_command("kimi login", Some(fake)).as_deref(), Some("~/.kimi-code/bin/kimi login"), "表の sign in の command を打たない");
        assert_eq!(tab_command("kimi login; rm -rf ~", Some(fake)), None, "表の外の command を打つ");
        assert_eq!(tab_command("kimi login", None), None, "sign in の口が無いのに打った");
        assert_eq!(tab_command("codex", None).as_deref(), Some("codex"), "agent の名前を口に渡した");
        assert_eq!(panel_spec("p", &terminal_panel::PanelTab { command: None, ..tab }).command, None);
    }

    /// L7（handoff 07・QA T-01「hide drawer → PTY continues, text persists」）: drawer を隠しても shell と中の job は動き続け、出力は残る（隠す ≠ 止める）。
    /// 開き直すと同じ tab・同じ pid（起こし直さない）。止めるのは tab の × の確認の `Stop` だけ（terminal_panel の `g95_tabs_add_rename_move_and_ask_before_stopping`）。
    /// 変異: `TerminalPanel::apply` の Toggle で `PanelEffect::Stop` を返す → 隠した後に pid が消えて赤。
    #[test]
    fn l7_hiding_the_drawer_keeps_the_shell_and_its_output() {
        let (mut g, project, _) = g95_project();
        g.panel_act(&project, PanelAct::Add).unwrap();
        let tab = g.panel(&project).tabs.last().unwrap().id.clone();
        let key = SessionKey { project_id: project.clone(), session_id: tab.clone() };
        let out = |g: &Generated| String::from_utf8_lossy(&g.terminals.read_since(&key, 0).unwrap().bytes).into_owned();
        // 打った行の echo と分けるため、shell が計算した字（l7-alive-42）を待つ
        g.terminals.write(&key, b"printf 'l7-alive-%s\\n' $((6*7)); sleep 60\n").unwrap();
        let wait = std::time::Instant::now();
        while !out(&g).contains("l7-alive-42") {
            assert!(wait.elapsed() < patient(Duration::from_secs(30)), "job が走らない: {:?}", out(&g));
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = g.terminals.list(Some(&project)).into_iter().find(|i| i.key == key).unwrap().pid;
        assert!(g.panel(&project).open, "前提: tab を足すと開く");
        g.panel_act(&project, PanelAct::Toggle).unwrap();
        assert!(!g.panel(&project).open, "隠した");
        assert!(crate::terminal::pid_alive(pid), "隠しただけで shell を止めた");
        assert!(g.panel_status(&project, true).remove(&tab).is_some_and(|s| s.running), "隠した tab も動いている印");
        assert!(out(&g).contains("l7-alive-42"), "隠しても出力は残る");
        g.panel_act(&project, PanelAct::Toggle).unwrap();
        assert_eq!(g.panel(&project).active.as_deref(), Some(tab.as_str()), "開き直すと同じ tab");
        assert_eq!(g.terminals.list(Some(&project)).into_iter().find(|i| i.key == key).map(|i| i.pid), Some(pid), "起こし直さない");
    }

    /// G95: 前面が agent（`claude` という名の script）の tab は ▶ Working → 出力が止まって ● Needs you。窓が前面でなければ通知を 1 回だけ・
    /// panel が開いていて窓が前面なら通知しない（見えている）。
    /// 変異: `panel_status` の通知の条件を外す（前面でも出す）→ 1 本目で数が 0 でなく赤・`probe` の前面を常に None → Working にならず赤。
    #[test]
    fn g95_agent_tab_turns_needs_you_and_notifies_once() {
        let (mut g, project, _) = g95_project();
        let bin = std::env::temp_dir().join(format!("shibubu-g95-agent-{}", storage::new_id()));
        std::fs::create_dir_all(&bin).unwrap();
        let claude = bin.join("claude");
        std::fs::write(&claude, "#!/bin/sh\nfor i in 1 2 3; do printf 'thinking\\n'; sleep 1; done\nsleep 60\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // tab を 1 つ足して agent を起こし、Needs you まで panel_status を回す（focused = 窓が前面か）
        let run = |g: &mut Generated, focused: bool| {
            g.panel_act(&project, PanelAct::Add).unwrap();
            let tab = g.panel(&project).tabs.last().unwrap().id.clone();
            assert!(g.panel(&project).open, "前提: tab を足すと panel が開く");
            let key = SessionKey { project_id: project.clone(), session_id: tab.clone() };
            g.terminals.write(&key, format!("{}\n", claude.display()).as_bytes()).unwrap();
            let wait = std::time::Instant::now();
            let mut seen: Vec<Option<AgentState>> = Vec::new();
            while wait.elapsed() < patient(Duration::from_secs(30)) {
                let s = g.panel_status(&project, focused).remove(&tab).unwrap_or_default();
                if seen.last() != Some(&s.agent) {
                    seen.push(s.agent.clone());
                }
                if s.agent == Some(AgentState::NeedsYou) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(seen.contains(&Some(AgentState::Working)) && seen.last() == Some(&Some(AgentState::NeedsYou)), "{seen:?}");
            for _ in 0..5 {
                g.panel_status(&project, focused);
            }
            key
        };
        // 1 本目: panel が開いていて窓が前面 → 見えているので通知しない
        let front = run(&mut g, true);
        assert!(g.term_views.notified.is_empty(), "前面で見えているのに通知した: {:?}", g.term_views.notified);
        // 2 本目: 窓が前面でない → 変わり目に 1 回
        let back = run(&mut g, false);
        assert_eq!(g.term_views.notified.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(), [AgentState::NeedsYou], "通知は変わり目に 1 回");
        for key in [front, back] {
            let _ = g.terminals.close(&key);
        }
        let _ = std::fs::remove_dir_all(&bin);
    }

    /// W8 反証: revision の行が読めない（壊れた・後の版の ops）時、Undo / Redo のボタンを黙って消さず、理由の 1 行を出す。
    /// その 1 行と ⌘Z の拒否の 1 行に serde の原文を出さない（W2 の反証の直しと同じ規則）。拒否は何も書かない。
    #[test]
    fn w8_unreadable_history_shows_a_line_instead_of_silently_hiding_undo() {
        let dir = std::env::temp_dir().join(format!("shibubu-w8-history-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap());
        let hash = g.project(&project).unwrap().2;
        rusqlite::Connection::open(dir.join("kangae.sqlite3")).unwrap().execute("UPDATE project_revisions SET ops_json='{' WHERE project_id=?1 AND revision=1", [&project]).unwrap();
        let t = page_texts(&mut g, &project, &tests);
        let a = page_ax(&mut g, &project, &tests);
        assert!(a.iter().any(|x| x == "Page options") && !a.iter().any(|x| x.starts_with("Undo") || x.starts_with("Redo")), "読めない履歴で Undo / Redo を出した: {a:?}");
        let line = t.iter().find(|x| x.contains("history unreadable")).unwrap_or_else(|| panic!("ボタンが黙って消えた（理由の 1 行が無い）: {t:?}"));
        assert!(!line.contains("json") && !line.contains("EOF") && !line.contains("column"), "serde の原文を画面に出さない: {line}");
        let refused = g.page_intent(&project, PageIntent::Undo).unwrap_err();
        assert!(refused.contains("history unreadable") && !refused.contains("json") && !refused.contains("EOF"), "{refused}");
        assert_eq!(g.project(&project).unwrap().2, hash, "読めない履歴の Undo は何も書かない");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8 反証: 読めない Undo の行き先（後の版の JSON・別の revision の中身）の 1 行に、serde の原文と Project の id を出さない（W2 の反証の直し 8922b3b と同じ規則）。
    /// 読めない行き先の Undo は何も書かない。
    #[test]
    fn w8_unreadable_undo_target_line_carries_no_serde_text_or_internal_id() {
        let dir = std::env::temp_dir().join(format!("shibubu-w8-lines-{}", storage::new_id()));
        let mut g = Generated::open(&dir);
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap();
        let hash = g.project(&project).unwrap().2;
        let db = rusqlite::Connection::open(dir.join("kangae.sqlite3")).unwrap();
        db.execute("UPDATE project_revisions SET json='{\"later\":1}' WHERE project_id=?1 AND revision=1", [&project]).unwrap();
        let later = g.page_intent(&project, PageIntent::Undo).unwrap_err();
        assert!(later.contains("保存した版 1 を読めません") && !later.contains("field") && !later.contains("later"), "serde の原文を出さない: {later}");
        db.execute(
            "UPDATE project_revisions SET json=(SELECT json FROM project_revisions WHERE project_id=?1 AND revision=2) WHERE project_id=?1 AND revision=1",
            [&project],
        )
        .unwrap();
        let other = g.page_intent(&project, PageIntent::Undo).unwrap_err();
        assert!(other.contains("保存した版 1 を読めません") && !other.contains(project.as_str()) && !other.contains("revision 2"), "内部の id を出さない: {other}");
        assert_eq!(g.project(&project).unwrap().2, hash, "読めない行き先の Undo は何も書かない");
        drop(db);
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- W12a: Project の resource（repo）を読む（試験の読み口・git も Adapter も使わない） ----

    /// 決めた結果を返し、呼ばれた回数を数える ResourceReader。`set` で次からの結果を替える。
    #[derive(Clone)]
    struct FakeReader {
        calls: Arc<Mutex<usize>>,
        next: Arc<Mutex<(Outcome, Option<Value>)>>,
    }
    impl FakeReader {
        fn new(o: Outcome, payload: Option<Value>) -> Self {
            FakeReader { calls: Arc::new(Mutex::new(0)), next: Arc::new(Mutex::new((o, payload))) }
        }
        fn set(&self, o: Outcome, payload: Option<Value>) {
            *self.next.lock().unwrap() = (o, payload);
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }
    impl ResourceReader for FakeReader {
        fn read(&mut self, _r: &project::ProjectResource) -> (Outcome, Option<crate::domain::run::Observation>) {
            *self.calls.lock().unwrap() += 1;
            let (o, payload) = self.next.lock().unwrap().clone();
            let obs = payload.map(|payload| crate::domain::run::Observation { id: "1".into(), source_ref: "fake".into(), observed_at: "2026-10-21T00:00:00Z".into(), payload });
            (o, obs)
        }
    }
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    fn read_ok() -> Outcome {
        Outcome::Verified { evidence: vec!["fake".into()] }
    }
    fn repo(changes: Value) -> Value {
        json!({ "head": { "sha": SHA, "branch": "main" }, "changes": changes })
    }
    fn repo_state(g: &Generated, project: &str) -> Option<Value> {
        let app = g.app_of_project(project).unwrap();
        g.store.lock().unwrap().get_ui_state(&session_of(&app), "resource.repo").unwrap()
    }

    /// G73（r14 の shots/160）: 中身の無い Page（app の Page・木が空）の 1 行は「○ Nothing here yet」を 1 回だけ（状態の語と detail に同じ語を 2 回出さない・
    /// 「Needs approval · Needs approval」にしない 2662 行と同じ規則）。負の対照（手で取る）: detail を "Nothing here yet" に戻す → 赤。
    #[test]
    fn g73_an_empty_page_says_nothing_here_yet_once() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let app = project.strip_prefix("project.").expect("project.<app>");
        let (p, h) = g.project_of(app).unwrap().expect("Project");
        let page = project::PageDefinition { app_ref: p.app_refs.first().cloned(), ..tests_page("empty") };
        g.apply_project(app, &h, &[ProjectPatchOp::AddPage { page, index: None }], RevisionOrigin::DirectEdit).unwrap();
        let t = page_texts(&mut g, &project, "empty");
        assert!(t.iter().any(|x| x.contains("Nothing here yet")), "空の Page の 1 行が無い: {t:?}");
        assert!(!t.iter().any(|x| x.matches("Nothing here yet").count() > 1), "同じ語を 2 回: {t:?}");
    }

    /// W12a: 読む時は open_project・navigate_page・↻ 読み直す だけ（作る・＋Page・編集・Undo では読まない）。読めた中身は ui_state の
    /// resource.repo にそのまま書き、Dev の Page の Git の欄が binding で引いて描く（branch・40 桁の SHA・変更 file の path）。
    #[test]
    fn w12a_reads_on_open_navigate_and_reread_only_and_the_dev_page_draws_the_repo() {
        let mut g = fresh();
        let reader = FakeReader::new(read_ok(), Some(repo(json!([{ "status": " M", "path": "README.md" }]))));
        g.set_resource_reader(Box::new(reader.clone()));
        let ws = std::env::temp_dir().join(format!("shibubu-w12a-ws-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        assert_eq!(reader.calls(), 0, "作るだけでは読まない（開く時に読む）");
        g.open_project(&project, Some(&dev)).unwrap();
        assert_eq!(reader.calls(), 1, "開いた時に 1 回");
        assert_eq!(repo_state(&g, &project), Some(repo(json!([{ "status": " M", "path": "README.md" }]))), "payload をそのまま ui_state へ");
        let t = page_texts(&mut g, &project, &dev);
        for want in [SHA, "main", "README.md", "Git"] {
            assert!(t.iter().any(|x| x == want), "Dev の Page に {want} が無い: {t:?}");
        }
        // UI-5a: 読み直すは ••• の menu の Refresh
        assert!(menu_ax(&mut g, &project, &dev, "Page options").iter().any(|x| x == "Refresh"), "••• に Refresh が無い");
        assert!(!t.iter().any(|x| x.contains("Not read yet") || x.contains("No changes")), "{t:?}");
        for _ in 0..3 {
            let _ = page_texts(&mut g, &project, &dev);
        }
        assert_eq!(reader.calls(), 1, "毎 frame は読まない");
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap());
        g.page_intent(&project, PageIntent::Undo).unwrap();
        g.page_intent(&project, PageIntent::Redo).unwrap();
        g.page_intent(&project, PageIntent::ShowTerminal { page: tests.clone() }).unwrap();
        assert_eq!(reader.calls(), 1, "編集・Undo・Redo・Show terminal では読まない");
        g.navigate_page(&project, &dev).unwrap();
        assert_eq!(reader.calls(), 2, "Page を移った時に 1 回");
        let (_, _, hash) = g.project(&project).unwrap();
        g.navigate_page(&project, &tests).unwrap();
        let reads = reader.calls();
        let at = page_of(&g.page_intent(&project, PageIntent::Reread).unwrap());
        assert_eq!((reader.calls(), at.as_str()), (reads + 1, tests.as_str()), "↻ 読み直すは 1 回読み、居場所を変えない");
        assert_eq!(g.project(&project).unwrap().2, hash, "読み直しは Project の revision と hash を変えない");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// W12a: 読めない時は ui_state に null（古い SHA を残さない）・bind した node は「読めていません」（空の文を言わない）・Page の上に
    /// 理由と次の一手の 1 行。次に読めたら 1 行も Error も消え、読めた上で空なら空の文を言う。
    #[test]
    fn w12a_unreadable_repo_writes_null_and_says_why_instead_of_the_empty_line() {
        let mut g = fresh();
        let reader = FakeReader::new(read_ok(), Some(repo(json!([{ "status": "??", "path": "new file.txt" }]))));
        g.set_resource_reader(Box::new(reader.clone()));
        let ws = std::env::temp_dir().join(format!("shibubu-w12a-gone-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        reader.set(Outcome::Failed { error: ErrorCode::TargetNotFound }, None);
        assert_eq!(g.read_resources(&project).unwrap().len(), 1, "読めなかった resource ごとに 1 行");
        assert_eq!(repo_state(&g, &project), Some(Value::Null), "読めない時は null（古い値を残さない）");
        let draw = |g: &mut Generated| page_texts(g, &project, &dev);
        let t = draw(&mut g);
        assert!(!t.iter().any(|x| x.contains(SHA) || x.contains("new file.txt")), "読めない時に古い SHA・変更を描いた: {t:?}");
        assert_eq!(t.iter().filter(|x| x.contains("Not read yet")).count(), 3, "branch・SHA・変更 file の 3 つ: {t:?}");
        assert!(!t.iter().any(|x| x.contains("No changes") || x.contains("No commits yet")), "読めていないのに空の文: {t:?}");
        assert!(t.iter().any(|x| x.contains("The repo location is missing") && x.contains("↻ Refresh")), "理由と次の一手の 1 行が無い: {t:?}");
        assert!(!t.iter().any(|x| x.contains("TargetNotFound") || x.contains("git.status")), "内部の語を出さない: {t:?}");
        reader.set(read_ok(), Some(json!({ "head": { "sha": null, "branch": null }, "changes": [] })));
        assert!(g.page_intent(&project, PageIntent::Reread).is_ok());
        let t = draw(&mut g);
        assert!(!t.iter().any(|x| x.contains("Not read yet") || x.contains("The repo location is missing")), "読めたら消える: {t:?}");
        for empty in ["No changes", "No commits yet", "Not on a branch"] {
            assert!(t.iter().any(|x| x.contains(empty)), "読めた上で空なら「{empty}」: {t:?}");
        }
    }

    /// W12a: 読み口が無い（E2E・試験）時は読まずに理由の 1 行・repo の無い Project には「↻ 読み直す」を出さない（押しても何も起きない・L2）。
    #[test]
    fn w12a_no_reader_says_so_and_no_repo_shows_no_reread_button() {
        let mut g = fresh();
        let ws = std::env::temp_dir().join(format!("shibubu-w12a-noreader-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        assert_eq!(repo_state(&g, &project), Some(Value::Null));
        let t = page_texts(&mut g, &project, &dev);
        assert!(t.iter().any(|x| x.contains("This version can't read repo")), "{t:?}");
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        assert_eq!(g.read_resources(&project).unwrap(), Vec::<String>::new(), "読む場所の無い Project は何も読まない");
        let t = menu_ax(&mut g, &project, &dev, "Page options");
        assert!(!t.iter().any(|x| x == "Refresh"), "repo の無い Project に読み直すを出した: {t:?}");
        assert!(t.iter().any(|x| x == "Delete page…"), "正の対照: ••• の menu は開いている: {t:?}");
    }

    /// W12a 反証: uri を持つ resource が在っても、それを描く node（binding が resource.repo）がどの Page にも無ければ読まず「↻ 読み直す」も出さない
    /// （Dev を消した Project・W3〜W12a interface の版で保存した Git の欄の無い Project は同じ形）。押しても何も起きないボタン（L2）と無駄な git を起こさない。
    #[test]
    fn w12a_no_bound_git_field_means_no_reads_and_no_reread_button() {
        let mut g = fresh();
        let reader = FakeReader::new(Outcome::Failed { error: ErrorCode::TargetNotFound }, None);
        g.set_resource_reader(Box::new(reader.clone()));
        let ws = std::env::temp_dir().join(format!("shibubu-w12a-nodev-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        assert_eq!(reader.calls(), 1, "正の対照: Git の欄が在る間は開いた時に読む");
        let draw = |g: &mut Generated, page: &str| page_texts(g, &project, page);
        // UI-5a: 読み直すは ••• の menu の Refresh（理由の 1 行は帯の下の字のまま）
        let menu = |g: &mut Generated, page: &str| menu_ax(g, &project, page, "Page options");
        assert!(menu(&mut g, &dev).iter().any(|x| x == "Refresh"), "正の対照: Git の欄が在る間は出す");
        let land = page_of(&g.page_intent(&project, PageIntent::Delete { page: dev.clone() }).unwrap());
        assert!(g.project(&project).unwrap().1.resources.iter().any(|r| r.uri.is_some()), "前提: Dev を消しても resource の uri は残る");
        // 消した直後（Delete は読まない）: 前に読めなかった時の理由の 1 行を、押すボタンの無い Page に残さない（D-e）
        let t = draw(&mut g, &land);
        assert!(!t.iter().any(|x| x.contains("The repo location is missing")), "Dev を消した直後に古い理由の 1 行を出した: {t:?}");
        assert!(!menu(&mut g, &land).iter().any(|x| x == "Refresh"), "Dev を消した直後に読み直すを出した");
        let other = g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).find(|id| *id != land).unwrap();
        let reads = reader.calls();
        g.navigate_page(&project, &other).unwrap();
        g.open_project(&project, Some(&land)).unwrap();
        assert_eq!(g.read_resources(&project).unwrap(), Vec::<String>::new(), "描く node の無い resource は読まない");
        assert_eq!(reader.calls(), reads, "Git の欄の無い Project で git を読んだ");
        let t = draw(&mut g, &land);
        assert!(!t.iter().any(|x| x.contains("The repo location is missing")), "Git の欄が無いのに理由の 1 行を出した: {t:?}");
        assert!(t.iter().any(|x| x == crate::ui::theme::icon_glyph(crate::domain::icon::Icon::Ellipsis)), "正の対照: 帯は描かれている: {t:?}");
        let m = menu(&mut g, &land);
        assert!(!m.iter().any(|x| x == "Refresh") && m.iter().any(|x| x == "Delete page…"), "Git の欄が無いのに読み直すを出した: {m:?}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// W12a 実機 4 回目（`209eca0`）: commit の後の「↻ 読み直す」で accesskit_consumer 0.26 の tree.rs:306 の unwrap に落ちて app が消えた。
    /// 前の frame と次の frame の両方に在る AX の node の親が替わると、TreeUpdate の中の順番しだいで、更新した node を孤児として消してから引く。
    /// 起きた形: 変更 file が 1 行 → 0 行で、空の文（`states::region` の子 Ui）が前の frame の `data_scope` の子 Ui と同じ名 "child"・同じ番目になり、
    /// 「変更した file はありません」が README.md の行の id のまま親だけ `data:dev-git-changes` → 窓に替わった（直しは `data_scope` の子 Ui を group の id で名付ける）。
    /// 契約: 読み直しの前後（変更 file 1 行 → 0 行 → 1 行）で、両方の frame に在る node の親は替わらない。
    #[test]
    fn w12a_reread_keeps_every_ax_node_under_the_same_parent() {
        use egui::accesskit::{NodeId, TreeUpdate};
        use std::collections::HashMap;
        let mut g = fresh();
        let one = json!([{ "status": " M", "path": "README.md" }]);
        let reader = FakeReader::new(read_ok(), Some(repo(one.clone())));
        g.set_resource_reader(Box::new(reader.clone()));
        let ws = std::env::temp_dir().join(format!("shibubu-w12a-ax-{}", storage::new_id()));
        let View::Project { project, page: dev } = g.create_project(DEMO, Some(&ws), None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let frame = |g: &mut Generated| -> TreeUpdate {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    g.draw_page(ui, &project, &dev);
                });
            });
            out.platform_output.accesskit_update.expect("AccessKit の木が無い")
        };
        let parents = |u: &TreeUpdate| -> HashMap<NodeId, NodeId> { u.nodes.iter().flat_map(|(p, n)| n.children().iter().map(move |c| (*c, *p))).collect() };
        let name = |u: &TreeUpdate, id: NodeId| u.nodes.iter().find(|(i, _)| *i == id).map(|(_, n)| format!("{:?} {:?} {:?} {:?}", n.role(), n.label(), n.value(), n.author_id()));
        let _ = frame(&mut g);
        let mut before = frame(&mut g);
        for (sha, changes) in [("fedcba9876543210fedcba9876543210fedcba98", json!([])), (SHA, one)] {
            reader.set(read_ok(), Some(json!({ "head": { "sha": sha, "branch": "main" }, "changes": changes })));
            g.page_intent(&project, PageIntent::Reread).unwrap();
            let after = frame(&mut g);
            let (pb, pa) = (parents(&before), parents(&after));
            let moved: Vec<String> = pa
                .iter()
                .filter(|(id, p)| pb.get(id).is_some_and(|q| q != *p))
                .map(|(id, p)| format!("{:?} 親 {:?} → {:?}", name(&after, *id), name(&before, pb[id]), name(&after, *p)))
                .collect();
            assert!(moved.is_empty(), "読み直し（変更 file {changes}）の前後で親が替わった AX の node:\n{}", moved.join("\n"));
            let t: Vec<String> = after.nodes.iter().filter_map(|(_, n)| n.value().map(String::from)).collect();
            assert!(t.iter().any(|x| x == sha), "前提: 読み直した SHA が木に出る: {t:?}");
            before = frame(&mut g);
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 殻の app の `metadata.composer.interpreter`（DB の apps の行に残る値・I3 の tally が見る）。
    fn interpreter_of(g: &Generated, project_id: &str) -> String {
        let (app, _, _) = g.project(project_id).unwrap();
        let def = g.find(&app).unwrap().rt.revision().definition().clone();
        def.metadata.as_ref().and_then(|m| m["composer"]["interpreter"].as_str()).unwrap_or("").to_owned()
    }

    /// W13: model が在る時は一言を model に読ませる（名前の表に無い一言でも Project・target は model の席から）。殻の app に interpreter = model。
    /// model が答えない時は名前の表で続け、理由を llm_line に 1 行（前に作った Project は書き換えない）。編集も model が読み、通れば 1 行は消える。
    /// UI-6a × NOTIFY-1101 追記: 編集で model を使えなかった理由は Home の llm_line でも Page の上でもなく toast（`take_notices`・owner 192）。
    #[test]
    fn w13_model_reads_the_goal_and_failures_fall_back_to_the_name_table() {
        use crate::llm::{fake::Fake, LlmError};
        let plan = json!({ "shape": "project", "name": "議事録", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": "https://example.org/notes", "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null },
            { "title": "Dev", "kind": "terminal", "app_name": null, "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 1, "slot": "main", "why": "", "feeds": null }
        ]});
        let edit = json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] });
        let fake = Fake::new(vec![Ok(plan), Err(LlmError::Timeout { ms: 1 }), Ok(edit), Err(LlmError::Malformed { detail: "壊れた答え".into() }), Err(LlmError::Malformed { detail: "二度目".into() })]);
        let mut g = fresh().with_llm(Ok(Box::new(fake)));
        assert_eq!(g.llm_line(), None, "model が使えている間は 1 行を出さない");
        let View::Project { project, .. } = g.create_project("議事録を example で取りたい n-13", None, None).unwrap().expect("model が project と読んだ") else { panic!() };
        let (_, first, first_hash) = g.project(&project).unwrap();
        assert_eq!(first.goal, "議事録を example で取りたい n-13");
        assert_eq!(first.surface_specs.iter().map(|s| s.target.clone()).collect::<Vec<_>>(), [project::SurfaceTarget::Url { url: "https://example.org/notes".into() }]);
        assert_eq!(first.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>(), ["Notes", "Dev"]);
        assert_eq!(interpreter_of(&g, &project), "model");

        let View::Project { project: second, .. } = g.create_project(DEMO, None, None).unwrap().expect("名前の表で続ける") else { panic!() };
        assert_eq!(g.project(&second).unwrap().1.pages.len(), 3, "名前の表の主デモの 3 枚");
        assert_eq!(interpreter_of(&g, &second), "rule_based");
        let line = g.llm_line().expect("model を使えなかった理由").to_owned();
        assert!(line.contains("simple rules") && line.contains("didn't answer within"), "{line}");
        assert_eq!(g.project(&project).unwrap().2, first_hash, "前の Project は書き換えない");

        let View::Project { page, .. } = g.page_intent(&project, PageIntent::Edit { text: "テスト用のページを追加して".into() }).unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        assert_eq!(p.page(&page).map(|pg| pg.title.as_str()), Some("Tests"), "model の add_page が Page になり、そこへ移る");
        assert_eq!(g.llm_line(), None, "通った編集で 1 行は消える");

        let View::Project { page, .. } = g.page_intent(&project, PageIntent::Edit { text: "Docs Page を足して".into() }).expect("名前の表で読み直す") else { panic!() };
        assert_eq!(g.project(&project).unwrap().1.page(&page).map(|pg| pg.title.as_str()), Some("Docs"));
        // NOTIFY-1101 追記: 理由は Page の上の Partial でなく toast（× まで残る）
        let notices = g.take_notices();
        assert!(matches!(notices.as_slice(), [(l, true)] if l.contains("read with simple rules") && l.contains("AI's answer couldn't be read") && !l.contains("壊れた答え")), "{notices:?}");
        assert!(!g.page_line.as_ref().is_some_and(|(s, _)| *s == RegionState::Partial), "Page の上に Partly shown を残した: {:?}", g.page_line);
        assert_eq!(g.llm_line(), None, "編集の理由を Home の 1 行に出した");
        // 反証の直し: 名前の表で読み直した編集が拒否されても、AI を使えなかった理由を拒否の文の後ろに残す
        let err = g.page_intent(&project, PageIntent::Edit { text: "Docs Page を足して".into() }).unwrap_err();
        assert!(err.starts_with("There's already") && err.contains("read with simple rules") && err.contains("AI's answer couldn't be read") && !err.contains("二度目"), "{err}");
        assert!(g.page_line.as_ref().is_some_and(|(_, l)| l == &err), "{:?}", g.page_line);
    }

    /// W14（YCM-067）: `with_llm` で渡した model には上限（env 無し = 既定の 10 回）が被さり、11 回目の生成と、その後の編集は model を呼ばずに
    /// 名前の表で続け、理由の 1 行（上限と開き直しの次の一手）を出す。前に作った Project は書き換えない。
    /// 負の対照: `set_llm` の `Budgeted::new` を外す（`Arc::from(l)` に戻す）と、11 回目が Fake の 12 番目〈尽きた〉に届き 1 行が「上限」でなくなって赤。
    /// 反証の直し（呼んでいない AI の「答え」を言わない）の負の対照: 生成か編集の 1 行を「AI の答えを使えなかったので」に戻す → 赤。
    /// NOTIFY-1101 追記の負の対照: finish_edit の `notices.push` を `page_line = Some((Partial, ..))` に戻す → toast が空・Page に描いて赤。
    #[test]
    fn w14_the_product_model_stops_at_the_call_limit_and_says_why() {
        use crate::llm::fake::Fake;
        assert!(std::env::var_os(crate::llm::BUDGET_ENV).is_none(), "この試験は既定の上限を見る（{} を外して回す）", crate::llm::BUDGET_ENV);
        let plan = |i: usize| json!({ "shape": "project", "name": format!("議事録 {i}"), "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": "https://example.org/notes", "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null }
        ]});
        let fake = Fake::new((0..12).map(|i| Ok(plan(i))).collect());
        let mut g = fresh().with_llm(Ok(Box::new(fake)));
        let mut first = None;
        for i in 0..10 {
            let View::Project { project, .. } = g.create_project(&format!("議事録を example で取りたい {i}"), None, None).unwrap().unwrap() else { panic!() };
            assert_eq!(interpreter_of(&g, &project), "model", "{i} 回目は上限の前");
            first.get_or_insert_with(|| (project.clone(), g.project(&project).unwrap().2));
        }
        assert_eq!(g.llm_line(), None);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().expect("名前の表で続ける") else { panic!() };
        assert_eq!(interpreter_of(&g, &project), "rule_based", "11 回目は model を呼ばない");
        let line = g.llm_line().expect("止めた理由の 1 行").to_owned();
        assert!(line.contains("stopped at its limit (10 calls)") && line.contains("raise it in Settings › AI & Models › Execution limit"), "{line}");
        // 反証の直し: 上限で止めた時は AI を呼んでいないので「答え」は無い（前は「AI の答えを使えなかったので」と頭に付けていた）
        assert!(!line.contains("answer"), "呼んでいない AI の答えを言っている: {line}");
        g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).expect("編集も名前の表で読み直す");
        // UI-6a × NOTIFY-1101 追記: 編集の理由は toast（× まで残る・Home の llm_line でも Page の上でもない）・次の一手まで全文
        let notices = g.take_notices();
        assert!(matches!(notices.as_slice(), [(l, true)] if l.contains("at its limit (10 calls)") && l.contains("raise it in Settings › AI & Models › Execution limit") && !l.contains("answer")), "{notices:?}");
        let page = g.project(&project).unwrap().1.pages[0].id.clone();
        let shown = painted_of(|ui| {
            ui.allocate_ui(egui::vec2(640.0, 900.0), |ui| g.draw_page(ui, &project, &page));
        });
        assert!(!shown.iter().any(|(t, _)| t.contains("at its limit (10 calls)")), "Page の上に Partly shown を描いた: {shown:?}");
        let (first, first_hash) = first.unwrap();
        assert_eq!(g.project(&first).unwrap().2, first_hash, "前の Project は書き換えない");
    }

    /// W13: CLI が使えない時は model 無し（名前の表）で作り、その理由の 1 行を出し続ける（D-e）。
    #[test]
    fn w13_without_the_cli_the_name_table_makes_it_and_says_why() {
        use crate::llm::LlmError;
        let mut g = fresh().with_llm(Err(LlmError::NoCli { detail: "AI is off for this launch".into() }));
        let line = g.llm_line().expect("理由の 1 行").to_owned();
        assert_eq!(line, "AI is off for this launch", "表の NoCli の EN は理由の 1 行だけ");
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(interpreter_of(&g, &project), "rule_based");
        assert_eq!(g.llm_line(), Some(line.as_str()), "作った後も出し続ける");
        assert_eq!(g.create_project("読書記録をつけたい", None, None), Ok(None), "1 Page の app は今の create");
    }

    /// 答えを試験が渡すまで待つ model（5 秒で諦めて Timeout・答えを待つ実装でも試験が止まり切らない）。
    struct Gated(Mutex<std::sync::mpsc::Receiver<Result<Value, crate::llm::LlmError>>>);
    impl crate::llm::Llm for Gated {
        fn label(&self) -> String {
            "gated".into()
        }
        fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
            self.0.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(Err(crate::llm::LlmError::Timeout { ms: 5000 }))
        }
    }

    /// FX2: job の印（`cancel`）が立つまで答えない model（立ったら `Cancelled`・20 秒で諦めて Timeout）。止められない `send` で呼ばれたら Timeout（= 試験が赤）。
    /// 呼ばれた回数を数える（止めた後にもう一度呼ばない事を読む）。
    struct Stoppable(Arc<std::sync::atomic::AtomicU32>);
    impl crate::llm::Llm for Stoppable {
        fn label(&self) -> String {
            "stoppable".into()
        }
        fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
            Err(crate::llm::LlmError::Timeout { ms: 0 })
        }
        fn send_cancellable(&self, _req: &crate::llm::JsonRequest, cancel: &std::sync::atomic::AtomicBool) -> Result<Value, crate::llm::LlmError> {
            use std::sync::atomic::Ordering;
            self.0.fetch_add(1, Ordering::SeqCst);
            let t = std::time::Instant::now();
            while !cancel.load(Ordering::SeqCst) {
                if t.elapsed() > std::time::Duration::from_secs(20) {
                    return Err(crate::llm::LlmError::Timeout { ms: 20_000 });
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(crate::llm::LlmError::Cancelled { after_ms: 1 })
        }
    }

    /// FX2（FX-004・FX-005・FX-007・caller の形・本体の前は todo!() で赤）: Home の生成の Stop。生成中は Running（段は Read）と run の id・
    /// Stop で job の印が立ち、子が回収される（model が Cancelled を返す）まで Stopping で `model_busy_line` は Some（Create を受けない）・
    /// 届いたら `ModelDone::Stopped` で一言が戻り、何も作らず（名前の表でも作らない）・AI をもう一度呼ばず・終わった 1 行は `Stopped. Nothing was added to Projects.`（手 0）。
    /// 負の対照（本体で取る）: `stop_model_job` を `self.model_job = None`（receiver を捨てるだけ）にする → model は 20 秒呼ばれ続け、`model_busy_line` が
    /// すぐ None になり、`ModelDone::Stopped` が来ない = 赤。
    #[test]
    fn fx2_stop_ends_the_ai_call_and_makes_nothing() {
        use crate::ui::build_progress::{Build, Phase};
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = Arc::new(AtomicU32::new(0));
        let mut g = fresh().with_llm(Ok(Box::new(Stoppable(calls.clone()))));
        let before = g.rows().len();
        assert!(g.start_create(DEMO, None, &[]), "model が在れば裏で始める");
        assert!(matches!(g.model_progress(), Some(Build::Running { phase: Phase::Read, .. })), "{:?}", g.model_progress());
        let run = g.model_run_id().expect("生成中は Runs の行の id が在る").to_owned();
        assert!(g.run_entries().iter().any(|e| e.id == run), "生成中の run が Runs の行に在る");
        let t = std::time::Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(t.elapsed() < patient(std::time::Duration::from_secs(10)), "model が呼ばれない");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(g.stop_model_job(), "止め始める");
        assert!(!g.stop_model_job(), "2 回目は止める物が無い");
        assert!(matches!(g.model_progress(), Some(Build::Stopping { .. })), "{:?}", g.model_progress());
        assert!(g.model_busy_line().is_some(), "子が回収されるまで Create を受けない");
        let ModelDone::Stopped { goal } = wait_done(&mut g) else { panic!("止めた生成は Stopped で終わる") };
        assert_eq!(goal, DEMO, "一言を欄へ戻せる");
        assert_eq!(g.model_busy_line(), None);
        assert_eq!(g.model_progress(), Some(Build::Ended { state: RegionState::Ready, line: "Stopped. Nothing was added to Projects.", action: None }));
        assert_eq!(g.model_run_id(), Some(run.as_str()), "終わった 1 行からも同じ run へ");
        // FX2 反証の直し: 止めた生成は何も書いていない = Runs の詳細に「Not sure it can be undone」を出さない（取り消しの行ごと出さない）
        let row = g.run_entries().into_iter().find(|e| e.id == run).expect("Runs に生成の行");
        assert_eq!(row.undo, crate::ui::runs::Undo::NotNeeded, "止めた生成の取り消しの語");
        assert_eq!(g.rows().len(), before, "何も作らない（名前の表でも作らない）");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "止めた後に AI をもう一度呼ばない");
        assert_eq!(g.llm_line(), None, "止めたのは AI の失敗ではない（simple rules の 1 行を出さない）");
    }

    /// FX2（FX-006・caller の形・本体の前は todo!() で赤）: AI の失敗の種類ごとの次の一手は 0 か 1（Try again・Say it another way・Use simple rules）。
    #[test]
    fn fx2_each_ai_failure_has_one_next_step() {
        use crate::llm::LlmError as E;
        use crate::ui::build_progress::BuildAction as A;
        let d = || "x".to_owned();
        let cases = [
            (E::Timeout { ms: 120_000 }, Some(A::Retry)),
            (E::RateLimited { retry_after: None }, Some(A::Retry)),
            (E::Server { status: 503 }, Some(A::Retry)),
            (E::Network { detail: d() }, Some(A::Retry)),
            (E::Malformed { detail: d() }, Some(A::Retry)),
            (E::Refused { detail: d() }, Some(A::Rephrase)),
            (E::Invalid { path: "$.a".into(), reason: d() }, Some(A::Rephrase)),
            (E::NoKey, Some(A::UseSimpleRules)),
            (E::NoCli { detail: d() }, Some(A::UseSimpleRules)),
            (E::Auth { status: 401, detail: d() }, Some(A::UseSimpleRules)),
            (E::Quota { detail: d() }, Some(A::UseSimpleRules)),
            (E::PlanLimit { detail: d() }, Some(A::UseSimpleRules)),
            (E::OverBudget { detail: d() }, Some(A::UseSimpleRules)),
            (E::Rejected { status: 400, detail: d() }, Some(A::UseSimpleRules)),
            (E::Schema { path: "$.a".into(), reason: d() }, Some(A::UseSimpleRules)),
            (E::Cancelled { after_ms: 5 }, None),
        ];
        for (e, want) in cases {
            assert_eq!(failure_step(&e), want, "{e:?}");
        }
    }

    /// FX2（FX-006・FX-007）: AI の失敗（plan の上限）は黙って名前の表で作らない = `ModelDone::Failed`・Project 0・理由の 1 行と `Use simple rules`・Runs の行は Failed。
    /// 利用者が `Use simple rules` を選ぶと AI を呼ばずに名前の表で作り、1 行が AI を使わなかった事と理由を言う（表 `Built with simple rules because AI wasn't available · {reason}`）。
    /// 負の対照（commit message）: `poll_model_job` の AI の失敗の腕を外す（前の `resolve_plan` の名前の表へ）→ `Created` で Project が出来て赤。
    #[test]
    fn fx2_ai_failure_builds_nothing_until_simple_rules_is_chosen() {
        use crate::ui::build_progress::{Build, BuildAction};
        use crate::ui::runs::RunResult;
        use std::sync::atomic::{AtomicU32, Ordering};
        struct AtLimit(Arc<AtomicU32>);
        impl crate::llm::Llm for AtLimit {
            fn label(&self) -> String {
                "at-limit".into()
            }
            fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(crate::llm::LlmError::PlanLimit { detail: "resets 3pm".into() })
            }
        }
        let calls = Arc::new(AtomicU32::new(0));
        let mut g = fresh().with_llm(Ok(Box::new(AtLimit(calls.clone()))));
        let before = g.rows().len();
        assert!(g.start_create(DEMO, None, &[]));
        let ModelDone::Failed { goal } = wait_done(&mut g) else { panic!("AI の失敗は Failed で終わる（名前の表で作らない）") };
        assert_eq!(goal, DEMO, "一言を欄へ戻せる");
        assert_eq!(g.rows().len(), before, "AI の失敗で黙って Project を作った");
        let Some(Build::Ended { state: RegionState::Error, line, action: Some(BuildAction::UseSimpleRules) }) = g.model_progress() else {
            panic!("理由の 1 行と Use simple rules: {:?}", g.model_progress())
        };
        let line = line.to_owned();
        assert_eq!(line, "AI wasn't available · your Claude plan's limit is reached (resets 3pm) · try again after it resets");
        assert_eq!(g.llm_line(), None, "失敗の理由は Ended の 1 行だけ（llm_line に 2 度出さない）");
        let run = g.model_run_id().expect("終わった 1 行から Runs の行へ").to_owned();
        let row = g.run_entries().into_iter().find(|e| e.id == run).expect("Runs に生成の行");
        assert_eq!(row.result(), RunResult::Failed);
        assert_eq!(row.target, "At-limit", "Runs の行の相手 = 使っている AI の名前（一言ではない・FX-040 d1）");
        assert_eq!(row.effect, crate::domain::action::Effect::ExternalSend, "一言は AI の CLI の先へ送る = Mac の外へ出る手");
        assert_eq!(row.undo, crate::ui::runs::Undo::NotNeeded, "AI の失敗は何も書いていない（Not sure it can be undone と言わない）");
        let made = g.create_project_simple(DEMO, None, None).expect("名前の表で作る");
        assert!(matches!(made, Some(View::Project { .. })), "主デモの一言は名前の表でも Project: {made:?}");
        assert_eq!(g.rows().len(), before + 1);
        assert_eq!(g.llm_line(), Some(format!("Built with simple rules because {line}").as_str()), "AI を使わなかった事を 1 行で言う");
        // DOGFOOD-X 周 7 #5: 同じ 1 行を作った Project の Chat にも（`llm_line` は Home だけ）
        let Some(View::Project { project, .. }) = made.clone() else { unreachable!() };
        assert_eq!(g.chat(&project).last().map(|t| t.text.clone()), Some(format!("Built with simple rules because {line}")), "AI を使わずに作った事が Project の Chat に無い");
        assert_eq!(g.model_progress(), None, "作った後は終わった 1 行を消す");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "Use simple rules は AI を呼ばない");
    }

    /// DOGFOOD-X 周 7 #2 #5 #6: Execution limit の Calls を 0 にした AI で、Home の会話の案を名前の表で作る（1 Page の app の道）。
    /// 止まった事は `llm_stopped`（呼ばない・1 回を使わない・`llm_ready` は true のまま）、作った物は必ず Some（呼ぶ側の `create` に任せない）、
    /// 包んだ Project の Chat の頭は Home の会話・作った物・「Built with simple rules」。
    /// B13（REVIEW-R7 Y-b）: 止まった理由は写した Home の返事が 1 回だけ言う（最後の行で 2 度目を言わない）・Home の案の Later の 1 行（nothing was built）は写さない。
    /// 負の対照（束）: `create_project_simple` の 1 Page の腕を `create` だけにする → 「Home の会話が Project の Chat に無い」で赤・Chat の say を外す → 最後の行で赤・
    /// `told` を false に固定する → 理由が 2 回で赤・`chat_head` の LATER_SAID の filter を外す → 写った Later の行で赤（どれも未測）。
    #[test]
    fn r7_simple_rules_under_a_stopped_ai_writes_the_project_chat() {
        use crate::domain::chat::{Mark, Turn, Who};
        const GOAL: &str = "Track my inventory";
        let mut g = fresh().with_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(json!({}))]))));
        g.set_budget(Some(0), None);
        let why = g.llm_stopped().expect("Calls 0 は止まっている");
        assert!(why.contains("(0 calls)") && why.contains("Settings › AI & Models › Execution limit"), "{why}");
        assert!(g.llm_ready(), "止まっていても CLI は在る（探し直しと Sign in を出さない）");
        assert!(composer::project::compose_project(GOAL, &new_ctx(), None).unwrap().is_none(), "前提: 1 Page の app の一言");
        let turn = |who, text: &str, proposal: Option<&str>| Turn { who, text: text.into(), mark: Mark::None, at: "2026-10-31T00:00:00Z".into(), attachments: Vec::new(), choices: Vec::new(), proposal: proposal.map(str::to_owned), preview: Vec::new(), sections: Vec::new(), needs: None };
        // 案を Later にしてから「build it」で同じ案に戻った会話（Later の 1 行が会話に残っている = app.rs の `plan_step` の LaterPlan）
        let later = format!("{} · {GOAL}", crate::domain::chat::LATER_SAID);
        g.accept_plan(GOAL, vec![turn(Who::You, GOAL, None), turn(Who::Shibubu, &why, Some(GOAL)), turn(Who::Shibubu, &later, None), turn(Who::You, "build it", None), turn(Who::Shibubu, &why, Some(GOAL))]);
        let made = g.create_project_simple(GOAL, None, None).expect("名前の表で作る");
        let Some(View::App(id)) = made.clone() else { panic!("1 Page の app を呼ぶ側に任せた: {made:?}") };
        let said: Vec<(Who, String)> = g.chat(&project_id_of(&id)).into_iter().map(|t| (t.who, t.text)).collect();
        assert_eq!(said.first(), Some(&(Who::You, GOAL.to_owned())), "Home の会話が Project の Chat に無い: {said:?}");
        assert!(said.iter().any(|(w, t)| *w == Who::Shibubu && t.starts_with("Made ") && t.contains("Next, say what to change")), "作った物と次の一手が無い: {said:?}");
        assert!(!said.iter().any(|(_, t)| t.contains("nothing was built")), "B13: Later の古い 1 行を作った Project の Chat に写した: {said:?}");
        assert_eq!(said.last().map(|(_, t)| t.as_str()), Some("Built with simple rules"), "B13: AI を使わずに作った事が無いか、写した返事と同じ理由を 2 度言った: {said:?}");
        assert!(!said.iter().filter(|(w, _)| *w == Who::Shibubu).skip_while(|(_, t)| !t.starts_with("Made ")).any(|(_, t)| t.contains(&why)), "B13: 作った後の行で止まった理由を言い直した: {said:?}");
        assert_eq!(g.budget_used(), Some((0, std::time::Duration::ZERO)), "止まっている AI を呼んだ");
    }

    /// FX2 反証の直し: AI の失敗で終わった生成の Runs の行は、語は本当の理由のまま（`Not set up yet`・`Connection lost`）で、次の一手に `Connect` を出さない
    /// （Connections に AI の行が無い = 押しても直す先が無い・G99 で AI の行が出来たら `RunEntry::connect` を true に）。Home の次の一手は `failure_step` のまま。
    /// 負の対照（commit message）: 生成の行の `connect` を true にする → NoKey・NoCli・Auth・Network・Server の 5 つで `Connect` が出て赤。
    #[test]
    fn fx2_ai_failure_rows_in_runs_offer_no_connect() {
        use crate::llm::LlmError as E;
        use crate::ui::runs::{RunAction, RunResult};
        struct Fails(E);
        impl crate::llm::Llm for Fails {
            fn label(&self) -> String {
                "fails".into()
            }
            fn send(&self, _req: &crate::llm::JsonRequest) -> Result<Value, E> {
                Err(self.0.clone())
            }
        }
        let d = || "x".to_owned();
        let cases = [
            (E::NoKey, "Not set up yet"),
            (E::NoCli { detail: d() }, "Not set up yet"),
            (E::Auth { status: 401, detail: d() }, "Not set up yet"),
            (E::Network { detail: d() }, "Connection lost"),
            (E::Server { status: 503 }, "Connection lost"),
        ];
        for (e, word) in cases {
            let mut g = fresh().with_llm(Ok(Box::new(Fails(e.clone()))));
            assert!(g.start_create(DEMO, None, &[]));
            assert!(matches!(wait_done(&mut g), ModelDone::Failed { .. }), "{e:?}");
            let run = g.model_run_id().expect("終わった 1 行から Runs の行へ").to_owned();
            let row = g.run_entries().into_iter().find(|r| r.id == run).expect("Runs に生成の行");
            assert_eq!(row.result(), RunResult::Failed, "{e:?}");
            assert_eq!(row.evidence(), [word], "{e:?}: 理由の語");
            assert_ne!(row.next_step(), Some(RunAction::Connect), "{e:?}: 直す先の無い Connect");
        }
    }

    fn wait_done(g: &mut Generated) -> ModelDone {
        let t = std::time::Instant::now();
        loop {
            if let Some(d) = g.poll_model_job(|| None) {
                return d;
            }
            assert!(t.elapsed() < patient(std::time::Duration::from_secs(30)), "答えが当たらない");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// UI-2（図 §36）: AI が在っても model の語に無い形（`rename Docs to Notes`・`move <面> left`・`run the tests`）は model に送らず thread も起こさない
    /// （start_edit が false）。rename は名前の表で当たり、`run the tests` は Tests の Page の「走らせる」と同じ道・同じ拒否（Terminal の無い Project）。
    /// 読めない文の 1 行は rescue_line（例は表から）。Fake は答えを持たない（呼べば「尽きました」の Model の Err → llm_line に理由が出る）。
    /// 負の対照: start_edit の `model_reads` の門を外す → true で赤・page_intent の field_run を外す → 救いの 1 行になり赤・
    /// page_intent_inner の `filter(model_reads)` を外す → llm_line に理由が出て赤。
    #[test]
    fn ui2_named_forms_skip_the_model_and_run_the_tests_runs() {
        use crate::llm::fake::Fake;
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let mut g = g.with_llm(Ok(Box::new(Fake::new(vec![]))));
        for said in ["rename Docs to Notes", "move Excalidraw left", "run the tests"] {
            assert!(!g.start_edit(&project, said), "{said}: model に送らない");
            assert!(g.model_busy_line().is_none(), "{said}: thread を起こさない");
        }
        g.page_intent(&project, PageIntent::Edit { text: "rename Docs to Notes".into() }).expect("名前の表で替える");
        let titles = |g: &Generated| g.project(&project).unwrap().1.pages.iter().map(|pg| pg.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&g), ["Dev", "Notes", "Diagram"]);
        assert_eq!(g.llm_line(), None, "model を呼んでいない");
        let button = g.page_intent(&project, PageIntent::RunTests { command: "cargo test".into() }).unwrap_err();
        let field = g.page_intent(&project, PageIntent::Edit { text: "Run the tests.".into() }).unwrap_err();
        assert_eq!(field, button, "欄の run the tests は Tests の button と同じ拒否");
        // 反証の直し: 拒否の 1 行は ▣ Needs approval にしない（承認の button が同じ行に無い・UI-1 の約束）・× Failed（test の失敗）とも混ぜない
        assert_eq!(g.page_line.clone(), Some((RegionState::Ready, field.clone())), "拒否は理由の文だけの Ready");
        assert_ne!(field, composer::project::rescue_line());
        assert_eq!(g.llm_line(), None, "model を呼んでいない");
        let fresh_g = &mut fresh();
        let View::Project { project: other, .. } = fresh_g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(fresh_g.page_intent(&other, PageIntent::Edit { text: "天気を教えて".into() }).unwrap_err(), composer::project::rescue_line());
    }

    /// 反証の直し（UI-2・欄の `run the tests`）: 走っている 1 行と結果は Tests の Page（route `/tests`）にしか描かないので、欄から打った時は
    /// その Page へ移ってから走らせる（拒否の 1 行もその Page に出る）。Tests の Page が無い Project では今の Page のまま（sendback 60.4）。
    /// 負の対照: page_intent の「Tests の Page へ移る」を外す → Dev のままで赤。
    #[test]
    fn ui2_field_run_the_tests_goes_to_the_tests_page_first() {
        let mut g = fresh();
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.open_project(&project, Some(&dev)).unwrap();
        let current = |g: &Generated| g.routers.get(&project).map(|r| r.current().to_owned());
        g.page_intent(&project, PageIntent::Edit { text: "run the tests".into() }).unwrap_err();
        assert_eq!(current(&g).as_deref(), Some(dev.as_str()), "Tests の Page が無ければ今の Page");
        let tests = page_of(&g.page_intent(&project, PageIntent::Edit { text: "add a Tests page".into() }).unwrap());
        g.navigate_page(&project, &dev).unwrap();
        let refused = g.page_intent(&project, PageIntent::Edit { text: "run the tests".into() }).unwrap_err();
        assert_eq!(current(&g).as_deref(), Some(tests.as_str()), "Tests の Page へ移ってから走らせる");
        assert_eq!(g.page_line.clone(), Some((RegionState::Ready, refused)), "repo の無い Project の拒否は移った先の 1 行");
    }

    /// 反証の直し（UI-2）: 生成した app の button が承認待ち（`Invoked::NeedsApproval`）になった時、button の行は `▣ Needs approval · Open Runs`
    /// （状態の語と本文を重ねない・承認待ちの run は Runs に在る）で、上の 1 行に `× Failed · …` として重ねない（承認待ちは失敗ではない）。
    /// region は Blocked のまま（bindings の gate が押し直しを止める・button の連打は key が無いので押すたびに承認待ちの run が増える）。
    /// 負の対照: detail を `Needs approval` に戻す / `*self.line` の Blocked を外す filter を外す → 赤。
    #[test]
    fn ui2_needs_approval_is_not_a_failure_line() {
        let mut g = fresh();
        let id = g.create("日記をつけたい").unwrap();
        let (_, mut h) = host_of(&mut g, &id);
        let press = |h: &mut Host<'_>, action: &str, input: Value| h.invoke(ActionInvocation { action_ref: action.into(), origin: InvocationOrigin::Button { node_key: "k".into() }, input });
        press(&mut h, "local.notes.add", json!({ "title": "消さない" }));
        assert!(h.line.is_none(), "正の対照: 足すのは承認不要で Verified: {:?}", h.line);
        let item = h.rt.ui_state("local.notes").unwrap()["items"][0]["id"].clone();
        press(&mut h, "local.notes.delete", json!({ "item_id": item }));
        assert_eq!(h.region("k"), Some((RegionState::Blocked, "Open Runs".to_owned())), "button の行");
        assert_eq!(h.line.clone(), None, "承認待ちを × Failed の 1 行にしない");
        // UI-6a（60.1）: 承認待ちの node は押し直すと 2 回届く = locked（gate が読む）
        assert!(h.locked("k"), "承認待ちの button を押し直せる");
        assert!(!h.locked("other"), "押していない node まで止めた");
    }

    /// UI-6a（60.1・反証の直し）: 送ったか分からない（EffectUnknown）手は button の行に Stale と今の文・Page の上の 1 行には重ねない・押し直せない（locked）。
    /// Verified になれば印は消える。負の対照（commit message）: `locked` の `RegionState::Stale |` を外す → 赤。
    #[test]
    fn ui6a_effect_unknown_is_stale_and_cannot_be_pressed_again() {
        let mut g = fresh();
        let id = g.create("日記をつけたい").unwrap();
        let (_, mut h) = host_of(&mut g, &id);
        let done = |outcome: Outcome| {
            Ok(Invoked::Done(crate::domain::run::Receipt { id: "r".into(), run_id: "run".into(), step_id: "s".into(), attempt_seq: 1, action_ref: "a".into(), outcome, observation_refs: Vec::new(), recorded_at: String::new() }))
        };
        h.record(Some("k".into()), done(Outcome::EffectUnknown { reason: "the line dropped".into() }));
        assert_eq!(h.region("k"), Some((RegionState::Stale, "Can't tell if it arrived · the line dropped · Open Runs".to_owned())));
        assert_eq!(h.line.clone(), None, "button の行と同じ 1 行を Page の上に重ねた");
        assert!(h.locked("k"), "送ったか分からない button を押し直せる（2 回届く）");
        h.record(None, done(Outcome::EffectUnknown { reason: "the line dropped".into() }));
        assert_eq!(h.line.clone().map(|(s, _)| s), Some(RegionState::Stale), "button の無い手は Page の上の 1 行に（× Failed にしない）");
        h.record(Some("k".into()), done(Outcome::Verified { evidence: vec!["seen".into()] }));
        assert!(!h.locked("k"), "届いた後も押せない");
    }

    /// UI-6a（owner の問題 3「`◐ Partly shown · …` が Home と全 Page」・sendback 27 / 50）: CLI が使えない理由は Home の 1 行の元（`llm_line`）に在るが、
    /// Project の Page には描かない。負の対照（commit message）: `draw_page` に `llm_line` の 1 行を戻す → 赤。
    #[test]
    fn ui6a_missing_cli_line_is_home_only() {
        use crate::llm::LlmError;
        let mut g = fresh().with_llm(Err(LlmError::NoCli { detail: "AI is off for this launch".into() }));
        let View::Project { project, page } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(g.llm_line(), Some("AI is off for this launch"), "Home の 1 行の元が無い");
        let shown = painted_of(|ui| {
            g.draw_page(ui, &project, &page);
        });
        // 見るのは llm_line の文だけ（DEMO の Dev の Page は folder の無い欄を正当に `◐ Partly shown` で描く = 反証の直し）
        assert!(!shown.iter().any(|(t, _)| t.contains("AI is off")), "Page に AI の 1 行: {shown:?}");
    }

    /// W15-c2（r20 の写真: Page の頭に飾りが 5〜6 行で本物の app が窓の半分まで押し下げられる）: Page に tab と同じ名前の見出しと Project の一言の全文を出さない・
    /// Web の面の頭は名前と状態の 1 行（生の URL の見出しと「Use it right here on the page」の行を描かない）。
    /// 負の対照: `render_page` の `opening` を true に戻す → 1 本目が赤・Web の面の見出しを戻す → 2 本目が赤。
    #[test]
    fn w15c2_a_page_starts_with_its_app_not_with_decoration() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let p = g.project(&project).unwrap().1;
        for page in &p.pages {
            let shown = painted_texts_of(|ui| {
                g.draw_page(ui, &project, &page.id);
            });
            assert_eq!(shown.iter().filter(|t| **t == page.title).count(), 1, "{} は tab の 1 つだけ（見出しを重ねない）: {shown:?}", page.title);
            assert!(!shown.iter().any(|t| t.contains(DEMO)), "Project の一言の全文を Page の中に出した: {shown:?}");
            assert!(!shown.iter().any(|t| t.contains("Use it right here")), "面の頭に説明の行: {shown:?}");
        }
    }

    /// UI-6a（owner の問題 3「`● Terminal …` が同じ Page に 2 回」・sendback 56）: 使える検査の通った席は画面に何も描かない（AX の Group は残る = e2e の
    /// `w14b_*` が読む）。通らない席の同じ文は 1 行。button の無い入れる・許可は ▣ にしない（Disconnected）・承認は描かない（NOTIFY-1101）・
    /// 1 問の答え（button）が在る時だけ Blocked。負の対照（commit message）: `usable_line_in` の None の腕に `●` の 1 行を戻す → 1 本目が赤。
    #[test]
    fn ui6a_usable_lines_say_nothing_when_usable_and_once_when_not() {
        let shown = painted_of(|ui| {
            renderer::usable_line(ui, "dev", "ready", None, "Terminal is usable", &[]);
            renderer::usable_line(ui, "dev2", "ready", None, "Terminal is usable", &[]);
            renderer::usable_line(ui, "a", "not_ready", Some(RegionState::Disconnected), "Install TextEdit", &[]);
            renderer::usable_line(ui, "b", "not_ready", Some(RegionState::Disconnected), "Install TextEdit", &[]);
        });
        assert!(!shown.iter().any(|(t, _)| t.contains("Terminal is usable")), "通った席を描いた: {shown:?}");
        assert_eq!(shown.iter().filter(|(t, _)| t.contains("Install TextEdit")).count(), 1, "同じ文を 2 回: {shown:?}");
        let seat = Seat { id: "s".into(), name: "TextEdit".into(), kind: SeatKind::NativeApp, first: None };
        let fail = |need: Need| Usable { seat: seat.clone(), reach: Check::Fail { need, line: "x".into() }, first_action: Check::Skipped, readback: Check::Skipped, fixed: false };
        assert_eq!(usable_state(&fail(Need::Install), false), Some(RegionState::Disconnected));
        assert_eq!(usable_state(&fail(Need::Permission), false), Some(RegionState::Disconnected));
        assert_eq!(usable_state(&fail(Need::Approve), false), None, "承認の説明の文は描かない（NOTIFY-1101・使う時に chat の card）");
        assert_eq!(usable_state(&fail(Need::Install), true), Some(RegionState::Blocked), "1 問の button が在る行は ▣");
    }

    /// 答えを返すだけの Engine（`engine_job` は `poll_engine` を呼ぶまで残る = 走っている間）。
    struct Idle;
    impl PageEngine for Idle {
        fn run(&self, _req: EngineRequest) -> EngineAnswer {
            EngineAnswer { reply: EngineReply::Failed { line: "stopped".into() }, log: ("error", json!({})) }
        }
    }

    /// G73（UI-6a）: 押せる答えの無い Engine の拒否（承認した手が無いのに同意が無くて止めた）の 1 行に ▣ Needs approval を付けない（Ready = 理由の文だけ）。
    /// 負の対照（手で取る）: poll_engine の NeedsConsent の腕の Ready を Blocked に戻す → 赤。
    #[test]
    fn g73_a_refusal_without_an_answer_is_not_needs_approval() {
        struct NoConsent;
        impl PageEngine for NoConsent {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                EngineAnswer { reply: EngineReply::NeedsConsent { line: "TextEdit isn't handed to the AI yet, so nothing was sent".into() }, log: ("error", json!({})) }
            }
        }
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(NoConsent), data_dir: std::env::temp_dir() });
        g.page_intent(&project, PageIntent::Edit { text: "tell me the weather".into() }).expect("Engine へ");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !g.poll_engine() {
            assert!(std::time::Instant::now() < end, "Engine の答えが届かない");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(g.engine_view(&project), Some(crate::ui::engine_view::EngineView::Line { state: RegionState::Ready, .. })), "{:?}", g.engine_view(&project));
    }

    /// operator 10-29（録画に映る済みの 1 行）: 確かめた数が 1 なら「1 item」・2 なら「2 items」（窓だけ・窓と保存した file の両方の 2 つの文）。頭の「Done. Checked」は
    /// rehearse の needle（labels.py の done_checked）なので変えない。負の対照: `domain::plural` の単数の腕を外す → 1 の 2 行が「1 items」で赤（4 行を見てから落とす）。
    #[test]
    fn done_line_says_1_item_and_2_items() {
        struct Checked(usize, usize);
        impl PageEngine for Checked {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                EngineAnswer { reply: EngineReply::Done { verified: self.0, saved: self.1, history: Vec::new(), partial: None, without_ai: None, already: None }, log: ("record", json!({})) }
            }
        }
        let mut bad = Vec::new();
        for (verified, saved, want) in [
            (1, 0, "Done. Checked 1 item in the original app"),
            (2, 0, "Done. Checked 2 items in the original app"),
            (2, 1, "Done. Checked 1 item in the original app and the saved file"),
            (3, 1, "Done. Checked 2 items in the original app and the saved file"),
        ] {
            let mut g = fresh();
            let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
            g.set_engine(EngineSetup { engine: Arc::new(Checked(verified, saved)), data_dir: std::env::temp_dir() });
            g.page_intent(&project, PageIntent::Edit { text: "tell me the weather".into() }).expect("Engine へ");
            let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !g.poll_engine() {
                assert!(std::time::Instant::now() < end, "Engine の答えが届かない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let view = g.engine_view(&project);
            if !matches!(&view, Some(crate::ui::engine_view::EngineView::Line { state: RegionState::Verified, line }) if line == want) {
                bad.push(format!("{verified}・{saved}: {view:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// ENGINE-AUDIT W7（spec 261「既に選ばれていれば『もうそうなっています』」・operator 11-03）: 手を 1 つも打たなかった Done（`already`）は
    /// 「Already like that · 理由」の 1 行 = 済み（✓ Verified）でも Partial でもない・会話の行に止めた印も ✓ も付けない。verified の数（読みの数）が 1 でも済みと言わない。
    /// 負の対照（未測）: poll_engine の already の腕を外す → 「Done. Checked 1 item」の Verified で赤・`mark` を mark_of に戻す → Mark::Stopped で赤。
    #[test]
    fn w7_already_like_that_is_neither_done_nor_partial() {
        use crate::domain::chat::Mark;
        struct Already;
        impl PageEngine for Already {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                let why = Some("“Acoustic Guitar” is already selected in GarageBand".to_owned());
                EngineAnswer { reply: EngineReply::Done { verified: 1, saved: 0, history: Vec::new(), partial: None, without_ai: None, already: why }, log: ("record", json!({})) }
            }
        }
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Already), data_dir: std::env::temp_dir() });
        // 一言は done_line_says_1_item_and_2_items と同じ（Engine へ行く事が分かっている文）
        g.page_intent(&project, PageIntent::Edit { text: "tell me the weather".into() }).expect("Engine へ");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !g.poll_engine() {
            assert!(std::time::Instant::now() < end, "Engine の答えが届かない");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let want = "Already like that · “Acoustic Guitar” is already selected in GarageBand";
        assert!(
            matches!(g.engine_view(&project), Some(crate::ui::engine_view::EngineView::Line { state: RegionState::Ready, line }) if line == want),
            "{:?}",
            g.engine_view(&project)
        );
        let last = g.chat(&project).pop().map(|t| (t.text, t.mark));
        assert_eq!(last, Some((want.to_owned(), Mark::None)), "会話の行");
    }

    /// review-UI-2 A1: 製品は常に `set_engine`（app.rs）。Engine が前の依頼を走らせている間に読めない文を送ると、救いの 1 行（嘘: 待てば通る文かもしれない）
    /// ではなく copy.en.md:198 の 1 行・打った文は欄に残る（UI-4a: 欄は header・残すのは「通った」の合図を立てない事・1 行は Ready = ③ C2）。
    /// 負の対照: finish_edit の `engine_job.is_some()` の腕を外す → 救いの 1 行で赤。
    #[test]
    fn ui2_unreadable_while_the_engine_runs_says_one_change_at_a_time() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Idle), data_dir: std::env::temp_dir() });
        g.page_intent(&project, PageIntent::Edit { text: "tell me the weather".into() }).expect("1 つ目は Engine へ");
        assert_eq!(g.take_edit_applied(), Some(project.clone()), "Engine に渡した = 通った（欄を空にする合図）");
        let err = g.page_intent(&project, PageIntent::Edit { text: "and tomorrow".into() }).unwrap_err();
        assert_eq!(err, "One change at a time. The last one is still being read.");
        assert_eq!(g.take_edit_applied(), None, "打った文は残す（空にする合図を立てない）");
        assert_eq!(g.page_line.clone(), Some((RegionState::Ready, err)), "待てば通る文に × Failed を付けない");
    }

    /// review-UI-2 B7 → UI-4a: 欄の placeholder の例は `EditVerb::AddPage.example()` と同じ文（literal は `--by en` の鍵なので式にしない）。
    /// 欄は header の 1 つ（`View::field_hint`）で、Page の中には言葉の欄も `Send changes` も描かない（d1_one_field）。
    /// 負の対照: `example()` の AddPage を `add a Notes page` にする → 1 つ目の assert が赤。draw_page に欄を戻す → 2 つ目が赤。
    #[test]
    fn ui2_placeholder_shows_the_add_page_example() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let page = g.project(&project).unwrap().1.pages[2].id.clone();
        let want = format!("Change this project, e.g. {}", composer::project::EditVerb::AddPage.example().unwrap());
        assert_eq!(View::Project { project: project.clone(), page: page.clone() }.field_hint(), want);
        let texts = page_texts(&mut g, &project, &page);
        assert!(!texts.iter().any(|t| t.starts_with("Change this project") || t == "Send changes"), "Page の中に 2 つ目の欄: {texts:?}");
    }

    /// UI-4a ③ C2: 文を直せば通る拒否（読めない・既に在る）は Page の上の 1 行を Ready（× Failed を付けない）・欄を空にする合図を立てない。
    /// 通った変更は合図を 1 回だけ立てる。負の対照: finish_edit の `edit_refused = true` を消す → Error で赤・拒否の後に合図を立てる → None の assert で赤。
    #[test]
    fn ui4a_fixable_edit_rejections_are_ready_and_keep_the_text() {
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.page_intent(&project, PageIntent::Edit { text: "add a Notes page".into() }).expect("通る");
        assert_eq!(g.take_edit_applied(), Some(project.clone()));
        assert_eq!(g.take_edit_applied(), None, "1 回取ったら次は None");
        let pages = g.project(&project).unwrap().1.pages.len();
        for text in ["add a Notes page", "tell me the weather"] {
            let err = g.page_intent(&project, PageIntent::Edit { text: text.into() }).unwrap_err();
            assert_eq!(g.page_line.clone(), Some((RegionState::Ready, err.clone())), "{text}");
            assert_eq!(g.take_edit_applied(), None, "{text}: 拒否で欄を空にしない");
            assert_eq!(g.project(&project).unwrap().1.pages.len(), pages, "{text}: 何も当てない");
        }
    }

    /// 反証の直し（W13）: model の呼び出しは裏の thread で待ち、呼ぶ側（UI の thread）は答えを待たずに返る。読む間は「読んでいます」の 1 行・
    /// 答えが届いたら同期の道と同じく Project を作り・編集を当てる。読んでいる間に Project が変わった編集は当てずに 1 行。
    /// 負の対照: `start_create` の中で `read_goal` を呼んで待つ（前の作り）と、start が 5 秒かかり 1 つ目の assert が赤。
    #[test]
    fn w13_model_reads_on_a_background_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut g = fresh().with_llm(Ok(Box::new(Gated(Mutex::new(rx)))));
        let t = std::time::Instant::now();
        assert!(g.start_create("議事録を example で取りたい n-13", None, &[]), "model が在れば裏で始める");
        assert!(t.elapsed() < patient(std::time::Duration::from_secs(2)), "答えを待たずに返る: {:?}", t.elapsed());
        assert!(g.model_busy_line().is_some_and(|l| l.contains("Reading your")));
        assert!(g.poll_model_job(|| None).is_none(), "答えが来るまで何もしない");
        tx.send(Ok(json!({ "shape": "project", "name": "議事録", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": "https://example.org/notes", "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null }
        ]}))).unwrap();
        let ModelDone::Created { goal, made: Ok(Some(View::Project { project, .. })) } = wait_done(&mut g) else { panic!("Project を作る") };
        assert_eq!(goal, "議事録を example で取りたい n-13");
        assert_eq!(interpreter_of(&g, &project), "model");
        assert_eq!((g.model_busy_line(), g.llm_line()), (None, None));

        assert!(g.start_edit(&project, "テスト用のページを追加して"));
        assert!(g.model_busy_line().is_some_and(|l| l.contains("Reading your change")));
        tx.send(Ok(json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] }))).unwrap();
        let ModelDone::Edited { project: edited, made: Ok(View::Project { page, .. }) } = wait_done(&mut g) else { panic!("編集を当てる") };
        assert_eq!(edited, project);
        assert_eq!(g.project(&project).unwrap().1.page(&page).map(|pg| pg.title.as_str()), Some("Tests"));

        // FX3（FX-013）の契約: 読んでいる間の人の帯の手は 1 行で待たせ（何も書かない）、AI の編集が base_hash で黙って落ちない（前は人の手が通り AI の編集が落ちた）
        assert!(g.start_edit(&project, "Docs Page を足して"));
        assert_eq!(g.page_intent(&project, PageIntent::Add), Err(ONE_CHANGE.to_owned()), "読んでいる間に人が Page を足す → 待つ 1 行");
        tx.send(Ok(json!({ "steps": [{ "verb": "add_page", "name": "Docs" }] }))).unwrap();
        let ModelDone::Edited { made: Ok(_), .. } = wait_done(&mut g) else { panic!("人が帯を変えていないので AI の編集が当たる") };
        assert!(g.project(&project).unwrap().1.pages.iter().any(|pg| pg.title == "Docs"));
    }

    /// W14 の実機（BG・`docs/evidence/W14/log.md` の試行 1）で見つけた W13 の不具合: idle の窓は frame を頼まないので、裏の model の答えが
    /// channel に届いても egui が起きず、「AI が一言を読んでいます」のまま Project が開かない（入力が来るまで）。答えを送った thread が
    /// 画面を起こす口（`wake_on_terminal` が置く `TerminalManager` の wake）を呼ぶ。一言も編集も。
    /// 負の対照: thread の `wake` の呼び出しを外すと、答えを送って 5 秒たっても 0 回で赤。
    #[test]
    fn w13_model_answer_wakes_the_screen() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut g = fresh().with_llm(Ok(Box::new(Gated(Mutex::new(rx)))));
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        g.terminals().set_wake(Arc::new(move || {
            w.fetch_add(1, SeqCst);
        }));
        let until = |what: &str, n: usize| {
            let t = std::time::Instant::now();
            while woke.load(SeqCst) < n {
                assert!(t.elapsed() < patient(std::time::Duration::from_secs(5)), "{what}: 答えが届いても画面を起こさない");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        assert!(g.start_create("議事録を example で取りたい n-13", None, &[]));
        assert_eq!(woke.load(SeqCst), 0, "答えの前は起こさない");
        tx.send(Ok(json!({ "shape": "project", "name": "議事録", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": "https://example.org/notes", "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null }
        ]}))).unwrap();
        until("一言", 1);
        let ModelDone::Created { made: Ok(Some(View::Project { project, .. })), .. } = wait_done(&mut g) else { panic!("Project を作る") };
        let before = woke.load(SeqCst);
        assert!(g.start_edit(&project, "テスト用のページを追加して"));
        tx.send(Ok(json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] }))).unwrap();
        until("編集", before + 1);
        assert!(matches!(wait_done(&mut g), ModelDone::Edited { made: Ok(_), .. }));
    }

    /// 反証の直し（W13）: CLI の確かめが終わる前に model 無しで作った時は、黙って名前の表で作らず理由を 1 行（D-e）。確かめの答えが届いたら消える。
    #[test]
    fn w13_creating_before_the_cli_check_says_why() {
        use crate::llm::LlmError;
        let mut g = fresh();
        assert_eq!(g.llm_line(), None, "確かめを頼んでいない試験の既定は 1 行を出さない");
        g.mark_llm_unchecked();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        assert_eq!(interpreter_of(&g, &project), "rule_based");
        assert!(g.llm_line().is_some_and(|l| l.contains("the AI check didn't finish")), "{:?}", g.llm_line());
        g.set_llm(Err(LlmError::NoCli { detail: "使わない設定です".into() }));
        assert!(g.llm_line().is_some_and(|l| !l.contains("the AI check didn't finish") && l.contains("使わない設定")));
        g.mark_llm_unchecked();
        assert!(!g.llm_line().unwrap().contains("the AI check didn't finish"), "確かめ終わった後は言わない");
    }

    // ---- W13b: 一言が名指した app がこの Mac に無い時は、面を黙って落とさず Page の中で 1 問（25 通目） ----

    /// 決めた Plan を返す model（この Mac の Spotlight を読まない: app の一覧は試験が渡す）。
    struct PlanOf(Value);
    impl composer::ModelProvider for PlanOf {
        fn complete_json(&self, _prompt: &str, _schema: &Value) -> Result<Value, String> {
            Ok(self.0.clone())
        }
    }

    /// Notes（在る app）・Draw（無い Zeta Edit・入れる頁と代わりの Brushwork）・Paint（無い Paint X・入れる頁も代わりも無い）の Project を保存して開く。
    fn w13b_project(g: &mut Generated) -> String {
        use composer::project::KnownApp;
        let apps = [KnownApp { name: "Notes Maker".into(), bundle_id: "org.example.notes".into() }, KnownApp { name: "Brushwork".into(), bundle_id: "org.example.brushwork".into() }];
        let plan = json!({ "shape": "project", "name": "作業場", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null },
            { "title": "Draw", "kind": "app", "app_name": "Zeta Edit", "bundle_id": "dev.example.zeta", "file": "docs/a.svg", "url": null, "install_url": "https://example.org/zeta", "instead": [{ "name": "Brushwork", "bundle_id": "org.example.brushwork" }], "page": 1, "slot": "main", "why": "", "feeds": null },
            { "title": "Paint", "kind": "app", "app_name": "Paint X", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 2, "slot": "main", "why": "", "feeds": null }
        ]});
        let plan = composer::project::plan_project("Notes Maker と Zeta Edit と Paint X で作業場 n-13b", &PlanOf(plan), &apps, &[], &new_ctx(), None).unwrap().expect("project の形");
        let View::Project { project, .. } = g.save_plan(plan, None).unwrap() else { panic!() };
        project
    }

    fn page_by_route(p: &ProjectDefinitionV1, seat: &str) -> PageDefinition {
        p.pages.iter().find(|pg| pg.route == format!("/{seat}")).cloned().expect("席の Page")
    }

    fn asked_on(page: &PageDefinition) -> usize {
        page.root.nodes().into_iter().filter(|n| composer::project::SeatV1::asked_in(n).is_some()).count()
    }

    /// G88a: 明示の無い 3 席は 1 Page（無い app の 1 問が 2 つ並ぶ）。1 問の席は席の slug で引く（route でない）: 答えはその席の node だけを替え、
    /// 1 行はその Page に出る。配置は保存した選択が無ければ model の置き場の型（`layout::SEAT_TEMPLATE`）・「Layout:」の替え順は 置き場 → 同梱の順 → 型なし → 置き場。
    /// 負の対照（commit message）: verifier の MissingApp の id を route に戻す → 2 つの 1 問が同じ席で赤／`placement` が置き場の型を見ない → 既定の型で赤
    #[test]
    fn g88a_one_page_answers_each_missing_app_by_its_seat_and_starts_from_the_models_layout() {
        use composer::project::KnownApp;
        let mut g = fresh();
        let apps = [KnownApp { name: "Notes Maker".into(), bundle_id: "org.example.notes".into() }, KnownApp { name: "Brushwork".into(), bundle_id: "org.example.brushwork".into() }];
        let plan = json!({ "shape": "project", "name": "作業場", "pages_requested": false, "seats": [
            { "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "書く物", "feeds": null },
            { "title": "Draw", "kind": "app", "app_name": "Zeta Edit", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [{ "name": "Brushwork", "bundle_id": "org.example.brushwork" }], "page": 0, "slot": "side", "why": "見る物", "feeds": null },
            { "title": "Paint", "kind": "app", "app_name": "Paint X", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "side", "why": "見る物", "feeds": null }
        ]});
        let plan = composer::project::plan_project("Notes Maker と Zeta Edit と Paint X で作業場 n-88a", &PlanOf(plan), &apps, &[], &new_ctx(), None).unwrap().expect("project の形");
        let View::Project { project, .. } = g.save_plan(plan, None).unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        assert_eq!(p.pages.len(), 1, "明示の無い 3 席が 1 Page でない");
        let missing: Vec<Usable> = g.usable_of(&project).into_iter().filter(|u| u.seat.kind == SeatKind::MissingApp).collect();
        assert_eq!(missing.iter().map(|u| u.seat.id.as_str()).collect::<Vec<_>>(), ["draw", "paint"], "1 問ごとの席でない");
        assert!(missing.iter().all(|u| usable_page(&p, &u.seat) == p.pages[0].id), "1 行が問いの Page に出ない");
        let ops = missing_app_ops(&p, "paint", None).expect("paint の 1 問への答え");
        assert!(matches!(&ops[..], [ProjectPatchOp::Page { patch, .. }] if patch.target_key == "paint-next"), "paint の答えが別の席の node を替える: {ops:?}");
        let ops = missing_app_ops(&p, "draw", Some("org.example.brushwork")).expect("draw の代わり");
        assert!(matches!(&ops[..], [ProjectPatchOp::Page { patch, .. }, ProjectPatchOp::AddSurface { spec, node_key, index: Some(1), .. }, ProjectPatchOp::Page { patch: named, .. }] if patch.target_key == "draw-next" && spec.id == "draw" && node_key == "draw-surface" && named.target_key == "draw-surface"), "{ops:?}");

        // 配置: 選択が無ければ model の置き場 → 「Layout:」を押すたびに 同梱の順 → 型なし → 置き場に戻る
        let id = |g: &Generated| g.placement(&project).unwrap().map(|t| t.id);
        assert_eq!(id(&g).as_deref(), Some(composer::layout::SEAT_TEMPLATE), "選択の無い Project が model の置き場で描かれない");
        // G94: 回すのはこの Page に効く型だけ（置き場 → 同梱のうち効く物）
        let ids = placement_ids(&p, p.pages.first());
        assert_eq!(ids.first().map(String::as_str), Some(composer::layout::SEAT_TEMPLATE), "回す型の先頭が置き場でない: {ids:?}");
        let mut seen = Vec::new();
        for _ in 0..ids.len() + 1 {
            g.page_intent(&project, PageIntent::Placement).unwrap();
            seen.push(id(&g));
        }
        let mut want: Vec<Option<String>> = ids[1..].iter().cloned().map(Some).collect();
        want.extend([None, Some(composer::layout::SEAT_TEMPLATE.to_string())]);
        assert_eq!(seen, want, "替え順が 置き場 → 効く同梱 → 型なし → 置き場 でない");
    }

    /// G88b: 入れた結果の 4 種と Err の帯の文・`Add take to REAPER` は表に在る app の面が Ready の時だけ押せる（表に無い app は出さない・ENGINE-GENERIC-1101 で
    /// AI-COMPOSE-1101 の「表に無い app も出す」を取り下げた = 録った後の手は Project のきっかけ）。
    /// 負の対照（commit message）: Ready を見ない → 窓の無い REAPER で押せて赤
    #[test]
    fn g88b_receipts_become_one_line_and_add_take_waits_for_a_ready_face() {
        use crate::audio::reaper::{Item, Receipt};
        let item = Item { guid: "{g88b}".into(), track: "Hum".into(), position: 0.0, length: 3.0, source: "/x/kangae-hum.wav".into(), new_track: false, project_bpm: 120.0 };
        assert_eq!(receipt_line("REAPER", &Ok(Receipt::Verified(item.clone()))), "✓ Added to REAPER · Undo in REAPER (⌘Z)");
        assert_eq!(receipt_line("REAPER", &Ok(Receipt::Duplicate(item.clone()))), "Already in REAPER");
        let unknown = receipt_line("REAPER", &Ok(Receipt::EffectUnknown { reason: "REAPER didn't answer in 90 s".into() }));
        assert!(unknown.contains("REAPER didn't answer in 90 s") && !unknown.starts_with('✓'), "{unknown}");
        let odd = receipt_line("REAPER", &Ok(Receipt::Unverified { item, reason: "the length differs".into() }));
        assert!(odd.contains("the length differs") && !odd.starts_with('✓'), "{odd}");
        assert_eq!(receipt_line("REAPER", &Err("REAPER isn't running".into())), "REAPER isn't running");
        let face = |bundle: &str| project::SurfaceSpec { id: "reaper".into(), backend: composer::project::APP_SEAT_BACKEND, target: project::SurfaceTarget::App { bundle_id: bundle.into(), file: None }, keep_alive: true, frame: None, browser: false };
        assert_eq!(add_take(Some(&face("com.cockos.reaper")), true), Some(renderer::AddTake { app: "REAPER".into(), blocked: None }));
        assert_eq!(add_take(Some(&face("com.cockos.reaper")), false), Some(renderer::AddTake { app: "REAPER".into(), blocked: Some("Open REAPER first".into()) }));
        assert_eq!(add_take(Some(&face("com.apple.garageband10")), true), None, "take を入れられない app に入れる button を出した（取り下げた by_ai の道）");
        assert_eq!(add_take(None, true), None);
    }

    /// ENGINE-ADJUST-1102 ①（owner 256・interface）: Running の card の Stop（`stop_engine`）は、Engine の thread が読む `EngineRequest::stop` と同じ印を立てる。
    /// 走っていない・別の Project・終わった後は false（何もしない）。立てた印は Engine が見て閉じ、答えは今まで通り `poll_engine` に届く（この偽の Engine は印を待って
    /// 「Stopped by you」の Denied で閉じる = 本物の閉じ方の形・承認の card は出ない）。負の対照: `stop_engine` の `store(true)` を外す → 偽の Engine が 5 秒待って Failed で閉じ、赤。
    #[test]
    fn engine_adjust_stop_engine_sets_the_flag_the_engine_thread_sees() {
        struct WaitsForStop;
        impl PageEngine for WaitsForStop {
            fn run(&self, req: EngineRequest) -> EngineAnswer {
                let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !req.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    if std::time::Instant::now() > end {
                        return EngineAnswer { reply: EngineReply::Failed { line: "not stopped".into() }, log: ("error", json!({})) };
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                EngineAnswer { reply: EngineReply::Denied { steps: Vec::new(), line: "Stopped by you · nothing more was sent".into() }, log: ("error", json!({ "kind": "stopped" })) }
            }
        }
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(WaitsForStop), data_dir: std::env::temp_dir() });
        assert!(!g.stop_engine(&project), "走っていないのに止めた");
        assert_eq!(g.ask_engine(&project, "Open the guitar list in the app", crate::domain::chat::Situation::default()), Ok(()));
        assert!(!g.stop_engine("project.other"), "別の Project の Stop で止めた");
        assert!(g.stop_engine(&project));
        answered(&mut g);
        let Some(crate::ui::engine_view::EngineView::Line { line, .. }) = g.engine_view(&project) else { panic!("{:?}", g.engine_view(&project)) };
        assert!(line.contains("Stopped by you"), "{line}");
        assert!(!g.stop_engine(&project), "終わった後に止めた");
    }

    /// ENGINE-GENERIC-1101: 届いた `EngineRequest` の一言と状況を残し、止めた答えを返す Engine。
    struct Heard(Arc<Mutex<Vec<(String, crate::domain::chat::Situation)>>>);
    impl PageEngine for Heard {
        fn run(&self, req: EngineRequest) -> EngineAnswer {
            self.0.lock().unwrap().push((req.goal.clone(), req.situation.clone()));
            EngineAnswer { reply: EngineReply::Failed { line: "stopped".into() }, log: ("error", json!({})) }
        }
    }

    /// ENGINE-GENERIC-1101: Engine の答えが届くまで `poll_engine` を回す（20 秒で赤）。
    fn answered(g: &mut Generated) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !g.poll_engine() {
            assert!(std::time::Instant::now() < end, "Engine の答えが届かない");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// ENGINE-GENERIC-1101（owner 211・interface）: 部品は app も決まった機能も知らず、一言と状況（file・直前の事）だけを Engine の 1 つの口（`ask_engine`）に渡す。
    /// 渡した一言と状況がそのまま `EngineRequest` に乗る・走っている間の 2 本目は始めずに 1 行・部品の `shibubu.act`（`PageIntent::CanvasAct`）も同じ口を通り、
    /// 前の部品の file は次の run に残らない。
    /// 負の対照: `start_engine_with` の `.with(with)` を外す → files の assert が赤・`ask_engine` の Err の腕を Ok にする → busy の assert が赤。
    #[test]
    fn engine_generic_a_part_hands_its_say_and_situation_to_the_one_engine_mouth() {
        use crate::domain::chat::Situation;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Heard(seen.clone())), data_dir: std::env::temp_dir() });
        let take = std::env::temp_dir().join("take-3.wav");
        let with = Situation { facts: vec!["Just happened: recorded a 12 s take".into()], files: vec![take.clone()] };
        assert_eq!(g.ask_engine(&project, "Put the take just recorded on the selected track", with), Ok(()));
        assert_eq!(g.ask_engine(&project, "Do it again", Situation::default()), Err(ENGINE_BUSY.into()), "走っている間に 2 本目を始めた");
        answered(&mut g);
        assert_eq!(g.page_intent(&project, PageIntent::CanvasAct { intent: "Set the sound to {value}".into(), value: json!("Clean") }).map(|_| ()), Ok(()));
        answered(&mut g);
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.iter().map(|(goal, _)| goal.as_str()).collect::<Vec<_>>(), ["Put the take just recorded on the selected track", "Set the sound to Clean"]);
        assert!(got[0].1.files == [take] && got[0].1.facts.iter().any(|f| f == "Just happened: recorded a 12 s take"), "部品が渡した状況が Engine に乗らない: {:?}", got[0].1);
        assert!(got[1].1.files.is_empty(), "前の部品の file が次の run に残った: {:?}", got[1].1);
    }

    /// ENGINE-AUDIT W9 本体（spec の流れ B・dogfood r6 = 一言が Engine でなく composer に化けた）: 分けた結果に従う = act は一言のまま Engine へ（chat には人の一言だけ）・
    /// 候補が 2 つ以上は 1 回だけ聞く（選択肢 = 候補・Engine は走らない）。負の対照（手で）: `follow_route` の engine の腕を外す → 1 本目が赤・ask の腕を外す → 2 本目が赤。
    #[test]
    fn w9_a_classified_act_goes_to_the_engine_and_an_unclear_target_is_asked_once() {
        use crate::domain::chat::{route, Classified, OperationKind, Who};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Heard(seen.clone())), data_dir: std::env::temp_dir() });
        let act = |target: Option<&str>, named: bool, candidates: &[&str]| Classified { kind: OperationKind::Act, want: "Open the guitar list".into(), target: target.map(str::to_owned), named, candidates: candidates.iter().map(|c| c.to_string()).collect() };
        let r = route(&[act(Some("com.example.mixer"), true, &[]), Classified { kind: OperationKind::Question, want: "Is it open?".into(), ..Default::default() }]);
        assert!(r.engine && r.talk && r.ask.is_none(), "{r:?}");
        g.follow_route(&project, "Open the guitar list and tell me if it is open", &r);
        answered(&mut g);
        assert_eq!(seen.lock().unwrap().iter().map(|(goal, _)| goal.clone()).collect::<Vec<_>>(), ["Open the guitar list and tell me if it is open"], "一言のまま Engine へ");
        let r = route(&[act(None, false, &["com.example.mixer", "com.example.daw"])]);
        g.follow_route(&project, "Make it a bit louder", &r);
        let last = g.chat(&project).last().cloned().expect("問いの行");
        assert!(last.who == Who::Shibubu && last.choices == ["com.example.mixer", "com.example.daw"] && last.text.contains("Which app"), "{last:?}");
        assert_eq!(seen.lock().unwrap().len(), 1, "聞く時に Engine を走らせた");
    }

    /// ENGINE-GENERIC-1101（owner 211・operator の決め 3）: 部品の手（録り終えた `recorded`）は、会話の AI が Project に残したきっかけの言葉
    /// （値の穴を部品の値で埋めた物）と状況を、同じ 1 つの口で Engine に渡すだけ。製品の code に app の名前も「take を入れる」手も無い（入れる先の app は
    /// 一言と Page の面から Engine がその場で決める）。きっかけの無い手は何もしない。Page の状況（題・選んだ値・録った take）は `situation` が足す。
    /// 値をまだ選んでいないきっかけは走らせずに「先に選ぶ」の 1 行。
    /// 負の対照: `part_event` の値の穴を埋めない（`t.goal(&v)` → `t.intent`）→ goal の assert が赤・`situation` の部品の値の行を外す → facts の assert が赤・
    /// 値の無い腕を飛ばす（`continue`）→ 「Choose in」の assert が赤・`part_event` の `earlier` を false → 「You」の assert が赤。
    #[test]
    fn engine_generic_a_parts_event_hands_the_projects_trigger_and_the_situation_to_the_engine() {
        use crate::domain::chat::Situation;
        use crate::domain::patch::{AppPatch, PatchOp};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Heard(seen.clone())), data_dir: std::env::temp_dir() });
        let insert = |g: &mut Generated, parts: Vec<Value>| {
            let (app, p, hash) = g.project(&project).unwrap();
            let page = &p.pages[0];
            let ops: Vec<_> = parts.into_iter().map(|v| ProjectPatchOp::Page { page_id: page.id.clone(), patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(v) } }).collect();
            g.apply_project(&app, &hash, &ops, RevisionOrigin::DirectEdit).expect("部品を置けない");
        };
        // 手を持つ部品（録音の帯）と、その手の後に Engine に頼むきっかけを持つ部品（会話の AI が利用者の一言から書いた data）
        let rec = "hum";
        let band = json!({ "key": rec, "component": "recorder", "label": "Hum", "props": { "label": "Record humming", "max_seconds": 60, "recorder_ref": rec, "feeds": null } });
        let say = "Set the selected track's sound to {value}, then put the take just recorded on that track";
        let part = json!({ "key": "part-guitar", "component": "canvas", "label": "Guitar", "props": {
            "html": "<select></select>", "values": { "guitar": "Acoustic Guitar" }, "on": [{ "act": format!("{rec}.recorded"), "key": "guitar", "intent": say }] } });
        insert(&mut g, vec![band, part]);
        let take = std::env::temp_dir().join("take-3.wav");
        g.takes.by_ref.insert(format!("{project}/{rec}"), TakeSlot { stage: renderer::TakeStage::Recorded { seconds: 12.0, peaks: Vec::new() }, path: Some(take.clone()), ..TakeSlot::default() });
        let with = Situation { facts: vec!["Just happened: “Hum” recorded a 12 s take".into()], files: vec![take.clone()] };
        assert_eq!(g.part_event(&project, rec, "recorded", with), Ok(()));
        answered(&mut g);
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "きっかけ 1 つで run 1 本: {got:?}");
        assert_eq!(got[0].0, "Set the selected track's sound to Acoustic Guitar, then put the take just recorded on that track");
        let s = &got[0].1;
        let has = |f: &str| s.facts.iter().any(|x| x == f);
        assert!(s.files == [take.clone()] && has("Chosen in “Guitar”: Acoustic Guitar") && has("Recorded in “Hum”: take-3.wav (12 s)") && has("Just happened: “Hum” recorded a 12 s take"), "{s:?}");
        assert!(s.facts.iter().any(|f| f.starts_with("Page: ")), "{s:?}");
        // 利用者が今打った言葉ではない = chat に「You」で出さず Shibubu の行（operator 決め）
        let lines: Vec<_> = g.chat(&project).into_iter().map(|t| (t.who, t.text)).collect();
        assert!(!lines.iter().any(|(w, t)| *w == crate::domain::chat::Who::You && t.contains("Acoustic Guitar")), "きっかけの一言を人の発言にした: {lines:?}");
        assert!(lines.iter().any(|(w, t)| *w == crate::domain::chat::Who::Shibubu && t == &format!("Doing what you asked earlier: {}", got[0].0)), "{lines:?}");
        assert_eq!(g.part_event(&project, rec, "played", Situation::default()), Ok(()), "きっかけの無い手で止めた");
        assert_eq!(seen.lock().unwrap().len(), 1, "きっかけの無い手で Engine を走らせた");
        // 値をまだ選んでいない部品のきっかけは走らせない（何を選ぶかの 1 行）
        let amp = json!({ "key": "part-amp", "component": "canvas", "label": "Amp", "props": {
            "html": "<select></select>", "on": [{ "act": format!("{rec}.recorded"), "key": "amp", "intent": "Use the {value} amp" }] } });
        insert(&mut g, vec![amp]);
        assert_eq!(g.part_event(&project, rec, "recorded", Situation::default()), Err("Choose in “Amp” first".into()));
        assert_eq!(seen.lock().unwrap().len(), 1, "値の無いきっかけで Engine を走らせた");
    }

    /// MIRROR-PIN-1102（interface・本体の前は ignore）: Page の Mirror が映している窓（Page の順・触った時刻つき）→ Engine に渡す窓。
    /// 1 つ = それ・2 つ以上 = 一番新しく触った 1 つ・どれも触っていない／同時 = Err（代用しない）・0 = None・u32 に収まらない窓番号の面は数えない。
    /// 負の対照（本体の後・手で）: 触った時刻の比べを外して先頭を返す → 「一番新しく触った」と「触っていない 2 つ」の行が赤。
    #[cfg(feature = "mirror")]
    #[test]
    fn mirror_pin_1102_the_page_mirror_window_or_none_or_ask() {
        use crate::surface::mirror::MirrorTarget;
        let gb = |window_id: u64| MirrorTarget { window_id, pid: 501, bundle_id: "com.apple.garageband10".into(), app_name: "GarageBand".into() };
        let never = f64::NEG_INFINITY;
        assert_eq!(pin_window(&[]), None, "映していない Page で窓を決めた");
        assert_eq!(pin_window(&[(gb(42), never)]), Some(Ok((501, 42))));
        let many = pin_window(&[(gb(42), never), (gb(43), never)]);
        assert!(matches!(&many, Some(Err(line)) if line.starts_with("2 GarageBand windows are on this page")), "触っていない 2 つから選んだ: {many:?}");
        assert_eq!(pin_window(&[(gb(42), 10.0), (gb(43), 20.0)]), Some(Ok((501, 43))), "一番新しく触った面でない");
        assert_eq!(pin_window(&[(gb(42), 20.0), (gb(43), never)]), Some(Ok((501, 42))), "触った面でない");
        assert!(matches!(pin_window(&[(gb(42), 20.0), (gb(43), 20.0)]), Some(Err(_))), "同時に触った 2 つから 1 つを選んだ");
        assert_eq!(pin_window(&[(gb(u64::from(u32::MAX) + 1), never)]), None, "u32 に収まらない窓番号を渡した");
        assert_eq!(pin_window(&[(gb(42), never), (gb(42), 5.0)]), Some(Ok((501, 42))), "同じ窓を映す 2 つの面で聞いた");
    }

    /// TALK-HONEST-1101 追記 232（owner「pageにエラーが出てるのにこの返答は何？」）× 追記 234: 部品の read が読めなかった 1 行は部品の名前つきで状況
    /// （chat の AI と Engine の 1 つの入口 = `situation`）に乗り、読み直しを始めると消える。Engine の直近の run（頼みと止まった理由）も次の状況に乗る。
    /// 負の対照（手で取る）: `take_reads` の誤りを覚える行を外す → shows an error の assert が赤・`start_part_read` の消す行を外す → 消えるの assert が赤・
    /// `poll_engine` の `last_runs` を外す → last run の assert が赤。
    #[test]
    fn honest232_a_parts_error_and_the_last_run_reach_the_situation() {
        use crate::domain::chat::Situation;
        use crate::domain::patch::{AppPatch, PatchOp};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Heard(seen.clone())), data_dir: std::env::temp_dir() });
        let (app, p, hash) = g.project(&project).unwrap();
        let page = &p.pages[0];
        let part = json!({ "key": "part-guitar", "component": "canvas", "label": "Guitar", "props": { "html": "<select></select>" } });
        let op = ProjectPatchOp::Page { page_id: page.id.clone(), patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(part) } };
        g.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit).expect("部品を置けない");
        // Heard の read は既定の本体（`READ_NONE` の Err）= 部品の頁に赤い 1 行が出る形
        let answer = |g: &mut Generated| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let got = g.take_reads();
                if !got.is_empty() {
                    break got;
                }
                assert!(std::time::Instant::now() < end, "read の答えが届かない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        assert_eq!(g.start_part_read(&project, "part-guitar", 1, "GarageBand", "guitar sound names"), Ok(()));
        assert_eq!(answer(&mut g)[0].2, Err(READ_NONE.into()));
        let shown = format!("“Guitar” shows an error: {READ_NONE}");
        assert!(g.situation(&project).facts.contains(&shown), "{:?}", g.situation(&project));
        assert_eq!(g.start_part_read(&project, "part-guitar", 2, "GarageBand", "guitar sound names"), Ok(()));
        assert!(!g.situation(&project).facts.contains(&shown), "読み直している間も前の誤りを言った");
        let _ = answer(&mut g);
        // Engine の run が止まった → 次の状況（chat にも Engine にも）に頼みと止まった理由
        assert_eq!(g.ask_engine(&project, "Open the guitar list", Situation::default()), Ok(()));
        answered(&mut g);
        let s = g.situation(&project);
        assert!(s.facts.iter().any(|f| f.starts_with(LAST_RUN) && f.contains("Open the guitar list") && f.contains("stopped")), "{s:?}");
    }

    /// TALK-HONEST-1101 追記 232 の 5: 編集の後の chat の 1 行は、Project の名前と Page の題の並び（`Changed Hum to Guitar: Hum to Guitar`）でなく、何を変えたか。
    /// 負の対照: `changed_line` を前の `page_titles` に戻す → 部品の assert が赤。
    #[test]
    fn honest232_b_the_changed_line_says_what_changed() {
        use crate::domain::patch::{AppPatch, PatchOp};
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let (_, p, _) = g.project(&project).unwrap();
        let page = &p.pages[0];
        let part = json!({ "key": "part-guitar", "component": "canvas", "label": "Guitar", "props": { "html": "<select></select>" } });
        let op = ProjectPatchOp::Page { page_id: page.id.clone(), patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(part) } };
        assert_eq!(changed_line(&[op], &p), format!("Changed {}: added “Guitar” to “{}”", p.name, page.title));
        let mut tests = page.clone();
        (tests.id, tests.title) = ("tests".into(), "Tests".into());
        assert_eq!(changed_line(&[ProjectPatchOp::AddPage { page: tests, index: None }], &p), format!("Changed {}: added the “Tests” page", p.name));
    }

    /// TALK-HONEST-1101 追記 232（operator の決め「Let Shibubu open it の button は作る」）: 部品の読めなかった答えが開ける一言（`ReadMiss::open`）を
    /// 持つ時だけ、`open_part` がその一言を Engine の 1 つの口（`ask_engine`）へ渡し、run が ✓ で終わったらその部品を 1 回だけ読み直す（`take_rereads`）。
    /// 止まった run では読み直さない・読めなかった答えの無い部品は始めずに `OPEN_NONE`。
    /// 負の対照（実装の後に手で取る）: `poll_engine` の rereads に足す行を外す → 読み直しの assert が赤・Failed でも足す → 止まった run の assert が赤。
    #[test]
    fn honest232_c_let_shibubu_open_it_asks_the_engine_and_reads_the_part_again_once() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Opener(Arc<Mutex<Vec<String>>>, Arc<AtomicBool>);
        impl PageEngine for Opener {
            fn run(&self, req: EngineRequest) -> EngineAnswer {
                self.0.lock().unwrap().push(req.goal.clone());
                if self.1.load(Ordering::SeqCst) {
                    EngineAnswer { reply: EngineReply::Done { verified: 1, saved: 0, history: Vec::new(), partial: None, without_ai: None, already: None }, log: ("record", json!({})) }
                } else {
                    EngineAnswer { reply: EngineReply::Failed { line: "stopped".into() }, log: ("error", json!({})) }
                }
            }
            fn read(&self, app: &str, _what: &str, _llm: Option<Arc<dyn crate::llm::Llm>>) -> Result<Value, ReadMiss> {
                Err(ReadMiss { line: format!("Looked for the guitar list in {app}, but {app} isn't open · Open {app}, or let Shibubu open it"), open: Some(format!("Open the guitar list in {app}")) })
            }
        }
        let (goals, ok) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(true)));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Opener(goals.clone(), ok.clone())), data_dir: std::env::temp_dir() });
        let part = "part-guitar";
        let missed = |g: &mut Generated, id: u64| {
            assert_eq!(g.start_part_read(&project, part, id, "Mixer", "the guitar list"), Ok(()));
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let got = g.take_reads();
                if let Some((_, _, got)) = got.into_iter().next() {
                    assert!(got.is_err_and(|m| m.open.is_some()), "開ける一言の無い答えになった");
                    break;
                }
                assert!(std::time::Instant::now() < end, "read の答えが届かない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        // 読めなかった答えの無い部品は始めない
        assert_eq!(g.open_part(&project, part), Err(OPEN_NONE.into()));
        assert!(goals.lock().unwrap().is_empty(), "開ける一言の無い部品で Engine を走らせた");
        // 読めなかった → 押す → 一言が Engine へ・✓ → その部品を 1 回だけ読み直す
        missed(&mut g, 1);
        assert_eq!(g.open_part(&project, part), Ok(()));
        answered(&mut g);
        assert_eq!(*goals.lock().unwrap(), ["Open the guitar list in Mixer"]);
        assert_eq!(g.take_rereads(), [(project.clone(), part.to_owned())], "✓ の後に部品を読み直さない");
        assert!(g.take_rereads().is_empty(), "読み直しが 2 回出た");
        // 止まった run では読み直さない
        ok.store(false, Ordering::SeqCst);
        missed(&mut g, 2);
        assert_eq!(g.open_part(&project, part), Ok(()));
        answered(&mut g);
        assert_eq!(goals.lock().unwrap().len(), 2);
        assert!(g.take_rereads().is_empty(), "止まった run の後に読み直した");
    }

    /// OPEN-INPLACE-1101 の 1・2（owner 240「let shibubu open it を押したら勝手に動くように・chat に戻る意味がわからない」「Need approve には always allow を」）:
    /// 部品から始めた run は部品の所に出る（承認 → `PartRun::Approve`・進めている → `Working`・止まった → `Stopped`）・その間 chat の列の card は無い（`engine_view` は None）。
    /// 部品の所の Always allow（`engine_act(AlwaysAllow)`）は今の手を送り、宛先に `"always": [action の id]` を足す・次の同じ手は承認を聞かずに 1 回送る
    /// （`skips_approval`）。chat は Shibubu の行だけ（人の発言にしない）。
    /// 負の対照: `part_run` を None のまま → Approve の assert が赤・`engine_view` の隠す枝を外す → card の assert が赤・grants の always を外す → always の assert が赤・
    /// `poll_engine` の skips_approval の枝を外す → 2 回目の Approve の assert が赤。
    /// ENGINE-AUDIT W7: 手は menu（押す手だけの承認は毎回聞く = Always allow を出さない）。負の対照（未測）: part_run の `asks_every_time` を None に → 押す手だけの assert が赤。
    #[test]
    fn open_inplace_a_the_parts_run_and_its_approval_are_shown_at_the_part() {
        use crate::surface::PartRun;
        use crate::ui::engine_view::{ApprovalStep, EngineAct};
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Asker(Arc<Mutex<Vec<(usize, Vec<Value>)>>>, Arc<AtomicBool>);
        impl PageEngine for Asker {
            fn run(&self, req: EngineRequest) -> EngineAnswer {
                self.0.lock().unwrap().push((req.consents.len(), req.grants.clone()));
                if self.1.load(Ordering::SeqCst) {
                    return EngineAnswer { reply: EngineReply::Failed { line: "Mixer stopped answering · Open Mixer, then try again".into() }, log: ("error", json!({})) };
                }
                if req.consents.is_empty() {
                    // ENGINE-AUDIT W7: Always allow の効く手 = menu（押す手だけの card は毎回聞く = Always allow を出さない）
                    let input = json!({ "pid": 4242, "window_id": 77, "app_name": "Mixer", "path": ["View", "Library"] });
                    // digest は偽の Engine が見ない（承認を渡したかは consents の数で見る）。generated.rs は adapters を持たない試験の crate（yc_demo_e2e・mirror_e0）にも #[path] で入る
                    let step = ApprovalStep { action: "desktop.ax.menu".into(), input, digest: "d-library".into(), scopes: vec!["desktop.act".into()] };
                    return EngineAnswer { reply: EngineReply::Denied { steps: vec![step], line: String::new() }, log: ("error", json!({})) };
                }
                EngineAnswer { reply: EngineReply::Done { verified: 1, saved: 0, history: Vec::new(), partial: None, without_ai: None, already: None }, log: ("record", json!({})) }
            }
            fn read(&self, app: &str, _what: &str, _llm: Option<Arc<dyn crate::llm::Llm>>) -> Result<Value, ReadMiss> {
                Err(ReadMiss { line: format!("Looked for the guitar list in {app}, but {app} isn't open · Open {app}, or let Shibubu open it"), open: Some(format!("Open the guitar list in {app}")) })
            }
            fn skips_approval(&self, steps: &[ApprovalStep]) -> bool {
                let always = self.0.lock().unwrap().iter().any(|(_, grants)| grants.iter().any(|t| t["always"] == json!(["desktop.ax.menu"])));
                always && !steps.is_empty() && steps.iter().all(|s| s.action == "desktop.ax.menu" && s.input["app_name"] == "Mixer")
            }
        }
        let (runs, fail) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Asker(runs.clone(), fail.clone())), data_dir: std::env::temp_dir() });
        let part = "part-guitar";
        let missed = |g: &mut Generated, id: u64| {
            assert_eq!(g.start_part_read(&project, part, id, "Mixer", "the guitar list"), Ok(()));
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while g.take_reads().is_empty() {
                assert!(std::time::Instant::now() < end, "read の答えが届かない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        // 1 回目: 承認は部品の所に（chat の列にも Page の欄の下にも card を出さない）
        missed(&mut g, 1);
        assert_eq!(g.open_part(&project, part), Ok(()));
        assert!(matches!(g.part_run(&project, part), Some(PartRun::Working(_))), "進めている間に部品の所に何も無い: {:?}", g.part_run(&project, part));
        assert!(g.engine_view(&project).is_none(), "部品の run の Running を chat の列に出した");
        answered(&mut g);
        assert!(matches!(g.part_run(&project, part), Some(PartRun::Approve(l, None)) if l.contains("Mixer")), "承認が部品の所に無い: {:?}", g.part_run(&project, part));
        assert!(g.engine_view(&project).is_none(), "部品の run の承認の card を chat の列にも出した");
        assert!(g.part_run(&project, "part-other").is_none(), "押していない部品に run を出した");
        // ENGINE-AUDIT W7: 押す手だけの承認なら部品の所も毎回聞く 1 行（頁は Always allow を描かない）。見たら今の card に戻す
        let shown = g.engine_views.get(&project).cloned().expect("承認の card");
        let press = ApprovalStep { action: "desktop.ax.click".into(), input: json!({ "pid": 4242, "app_name": "Mixer", "label": "Delete Track" }), digest: "d".into(), scopes: vec![] };
        g.engine_views.insert(project.clone(), crate::ui::engine_view::EngineView::Approve { goal: "clean up".into(), steps: vec![press], line: None });
        assert!(
            matches!(g.part_run(&project, part), Some(PartRun::Approve(_, Some(why))) if why == "Asks every time before pressing “Delete Track”"),
            "押す手だけの承認に Always allow を出す形: {:?}",
            g.part_run(&project, part)
        );
        g.engine_views.insert(project.clone(), shown);
        // Always allow → 今の手を送る（宛先に always）・✓ → 部品の所は消えて 1 回読み直す
        g.engine_act(&project, EngineAct::AlwaysAllow);
        assert!(matches!(g.part_run(&project, part), Some(PartRun::Working(_))), "{:?}", g.part_run(&project, part));
        answered(&mut g);
        assert_eq!(runs.lock().unwrap().len(), 2);
        let (consents, grants) = runs.lock().unwrap()[1].clone();
        assert!(consents == 1 && grants.len() == 1 && grants[0]["always"] == json!(["desktop.ax.menu"]), "Always allow が宛先に覚える手を渡さない: {grants:?}");
        assert!(g.part_run(&project, part).is_none(), "✓ の後も部品の所に run が残った");
        assert_eq!(g.take_rereads(), [(project.clone(), part.to_owned())], "✓ の後に部品を読み直さない");
        // 2 回目: 覚えた手は承認を聞かずに 1 回送る
        missed(&mut g, 2);
        assert_eq!(g.open_part(&project, part), Ok(()));
        answered(&mut g);
        assert!(!matches!(g.part_run(&project, part), Some(PartRun::Approve(..))), "Always allow の後にまた承認を聞いた");
        answered(&mut g);
        assert_eq!(runs.lock().unwrap().iter().map(|(c, _)| *c).collect::<Vec<_>>(), [0, 1, 0, 1], "覚えた手を送らない・2 回送った");
        assert_eq!(g.take_rereads(), [(project.clone(), part.to_owned())]);
        // 止まった → 理由と次の一手の 1 行が部品の所に（次の read で消える）
        fail.store(true, Ordering::SeqCst);
        missed(&mut g, 3);
        assert_eq!(g.open_part(&project, part), Ok(()));
        answered(&mut g);
        assert!(matches!(g.part_run(&project, part), Some(PartRun::Stopped(l)) if l.contains("Open Mixer")), "{:?}", g.part_run(&project, part));
        missed(&mut g, 4);
        assert!(g.part_run(&project, part).is_none(), "読み直した後も止まった 1 行が残った");
        // chat は Shibubu の記録の行だけ（押した一言を人の発言にしない）
        let lines: Vec<_> = g.chat(&project).into_iter().map(|t| (t.who, t.text)).collect();
        assert!(!lines.iter().any(|(w, t)| *w == crate::domain::chat::Who::You && t.contains("guitar list")), "{lines:?}");
        assert!(lines.iter().any(|(w, t)| *w == crate::domain::chat::Who::Shibubu && t.contains("Open the guitar list in Mixer")), "{lines:?}");
    }

    /// OPEN-INPLACE-1101 の 4（owner が chat に打った「一度取得したら list するように」）: 一度読めた値は Project の app の ui_state に残り（⌘Z の revision は増えない）、次の read は
    /// すぐその値で答え、裏で読み直す。読み直しが読めなければ何も出さない（誤りの 1 行は一度も読めていない時だけ）・違う値なら残す値を替えて 1 回読み直し、
    /// その読み直しは残した新しい値ですぐ答える（もう一度は読まない）。
    /// 負の対照: `take_reads` の `keep_read` を外す → 残った値の assert が赤・前の値で答える枝を外す → 2 回目の Ok の assert が赤・読み直しの失敗を答えにする → 空の assert が赤・
    /// `next_read` の false を外す → 4 回目の数の assert が赤。
    #[test]
    fn open_inplace_b_a_list_once_read_is_kept_and_shown_until_a_new_one_is_read() {
        use crate::domain::patch::{AppPatch, PatchOp};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Lister(Arc<Mutex<Vec<Result<Value, ReadMiss>>>>, Arc<AtomicUsize>);
        impl PageEngine for Lister {
            fn run(&self, _req: EngineRequest) -> EngineAnswer {
                EngineAnswer { reply: EngineReply::Failed { line: "stopped".into() }, log: ("error", json!({})) }
            }
            fn read(&self, _app: &str, _what: &str, _llm: Option<Arc<dyn crate::llm::Llm>>) -> Result<Value, ReadMiss> {
                self.1.fetch_add(1, Ordering::SeqCst);
                self.0.lock().unwrap().remove(0)
            }
        }
        let kept = |g: &Generated, project: &str, node: &str| g.kept_read(project, node, "Mixer", "the guitar list").unwrap_or(Value::Null);
        let drummer = || Err(ReadMiss { line: "Looked for the guitar list in Mixer, but Mixer shows Drummer".into(), open: Some("Open the guitar list in Mixer".into()) });
        let answers = Arc::new(Mutex::new(vec![Ok(json!(["Acoustic", "Clean"])), drummer(), Ok(json!(["Acoustic", "Clean", "Jazz"]))]));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut g = fresh();
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.set_engine(EngineSetup { engine: Arc::new(Lister(answers, calls.clone())), data_dir: std::env::temp_dir() });
        let (app, p, hash) = g.project(&project).unwrap();
        let page = &p.pages[0];
        let node = json!({ "key": "part-guitar", "component": "canvas", "label": "Guitar", "props": { "html": "<select></select>" } });
        let op = ProjectPatchOp::Page { page_id: page.id.clone(), patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(node) } };
        g.apply_project(&app, &hash, &[op], RevisionOrigin::DirectEdit).expect("部品を置けない");
        let part = "part-guitar";
        let first = |g: &mut Generated| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some((_, _, got)) = g.take_reads().into_iter().next() {
                    break got;
                }
                assert!(std::time::Instant::now() < end, "read の答えが届かない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let settle = |g: &mut Generated, n: usize| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while calls.load(Ordering::SeqCst) < n {
                assert!(std::time::Instant::now() < end, "裏の読み直しが走らない");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            g.take_reads()
        };
        // 1 回目: 読めた → Project に残る（app の大文字小文字と前後の空白は同じ頼み）・props には書かない = ⌘Z の revision は増えない
        let before = g.project(&project).unwrap().2;
        assert_eq!(g.start_part_read(&project, part, 1, "Mixer", "the guitar list"), Ok(()));
        assert_eq!(first(&mut g).ok(), Some(json!(["Acoustic", "Clean"])));
        assert_eq!(kept(&g, &project, part), json!(["Acoustic", "Clean"]), "読めた一覧を Project に残さない");
        assert_eq!(g.kept_read(&project, part, " mixer ", "the guitar list"), Some(json!(["Acoustic", "Clean"])), "同じ頼みを別の場所に残した");
        assert_eq!(g.project(&project).unwrap().2, before, "読めた一覧で Project の revision を足した");
        // 2 回目: app が別の画面（Drummer）→ すぐ前の一覧で答え、読み直しの誤りは出さない
        assert_eq!(g.start_part_read(&project, part, 2, "Mixer", "the guitar list"), Ok(()));
        assert_eq!(first(&mut g).ok(), Some(json!(["Acoustic", "Clean"])), "一度読めた一覧を出さない");
        assert!(settle(&mut g, 2).is_empty(), "読み直しの誤りを部品に出した");
        assert!(!g.situation(&project).facts.iter().any(|f| f.contains("shows an error")), "一度読めた部品の誤りを状況に乗せた");
        assert!(g.take_rereads().is_empty() && kept(&g, &project, part) == json!(["Acoustic", "Clean"]), "読めなかった読み直しで一覧を替えた");
        // 3 回目: 違う一覧が読めた → 残す値を替えて 1 回読み直す
        assert_eq!(g.start_part_read(&project, part, 3, "Mixer", "the guitar list"), Ok(()));
        assert_eq!(first(&mut g).ok(), Some(json!(["Acoustic", "Clean"])));
        let _ = settle(&mut g, 3);
        assert_eq!(kept(&g, &project, part), json!(["Acoustic", "Clean", "Jazz"]), "新しく読めた一覧を残さない");
        assert_eq!(g.take_rereads(), [(project.clone(), part.to_owned())], "違う一覧が読めたのに部品を読み直さない");
        // 4 回目（その読み直し）: 残した新しい一覧ですぐ答え、もう一度は読まない
        assert_eq!(g.start_part_read(&project, part, 4, "Mixer", "the guitar list"), Ok(()));
        assert_eq!(first(&mut g).ok(), Some(json!(["Acoustic", "Clean", "Jazz"])), "読み直しが新しい一覧で答えない");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "読み直しでもう一度読んだ（AI の枠を減らす）");
        assert!(g.take_reads().is_empty() && g.take_rereads().is_empty(), "読み直しの後にまだ答えた・読み直した");
    }

    /// G88b: 配る .app の Info.plist が mic の許可の文を持つ（無いと録る時に TCC で落ちる）。
    #[test]
    fn g88b_the_app_asks_for_the_microphone_in_its_info_plist() {
        let script = include_str!("../scripts/release/build-mac.sh");
        assert!(script.contains("<key>NSMicrophoneUsageDescription</key><string>Shibubu records your humming when you press Record humming. Nothing is sent anywhere.</string>"), "build-mac.sh の Info.plist に mic の文が無い");
    }

    /// W13b 本体: 無い app の席は使える検査の 1 行（① 入れる）がその app の Page に出て、答えは Project の op（DirectEdit・Undo できる）か理由の 1 行。
    /// 「無しで続ける」→ 同じ node の Page{Update}・問いが消え席も消える・2 回目は 1 行で何も書かない・Undo で問いが戻る。
    /// 「代わりに <app>」→ 一覧に無い bundle id は当てない・当てる op は 1 問の Remove + 同じ場所の AddSurface（W16M-3）。AddSurface の当たらない build では理由の 1 行。
    /// 負の対照: `usable_page` の MissingApp の腕を外す → 1 行が先頭の Page（Notes）に出て赤。
    #[test]
    fn w13b_missing_app_asks_in_its_page_and_the_answers_are_one_direct_edit() {
        use crate::domain::patch::{AppPatch, PatchOp};
        let mut g = fresh();
        let project = w13b_project(&mut g);
        let (_, p, hash) = g.project(&project).unwrap();
        let missing: Vec<Usable> = g.usable_of(&project).into_iter().filter(|u| u.seat.kind == SeatKind::MissingApp).collect();
        assert_eq!(missing.iter().map(|u| (u.seat.id.as_str(), u.seat.name.as_str())).collect::<Vec<_>>(), [("draw", "Zeta Edit"), ("paint", "Paint X")], "在る app に問いを出した・無い app の問いを落とした");
        for u in &missing {
            assert!(matches!(u.reach, Check::Fail { need: Need::Install, .. }) && u.value() == "not_ready", "{u:?}");
            assert!(u.line().starts_with("Install: ") && u.line().contains(&u.seat.name), "{}", u.line());
            assert_eq!(usable_page(&p, &u.seat), page_by_route(&p, &u.seat.id).id, "問いがその app の Page に出ない");
        }

        // 当てる op（trunk の W16M-3 の AddSurface で当たる形）・一覧に無い代わりは当てない
        let draw = page_by_route(&p, "draw");
        let spec = project::SurfaceSpec { id: "draw".into(), backend: composer::project::APP_SEAT_BACKEND, target: project::SurfaceTarget::App { bundle_id: "org.example.brushwork".into(), file: Some("docs/a.svg".into()) }, keep_alive: true, frame: None, browser: false };
        assert_eq!(
            missing_app_ops(&p, "draw", Some("org.example.brushwork")),
            Ok(vec![
                ProjectPatchOp::Page { page_id: draw.id.clone(), patch: AppPatch { op: PatchOp::Remove, target_key: "draw-next".into(), value: None } },
                ProjectPatchOp::AddSurface { spec, page_id: draw.id.clone(), parent: "draw-root".into(), node_key: "draw-surface".into(), index: Some(0) },
                ProjectPatchOp::Page { page_id: draw.id.clone(), patch: AppPatch { op: PatchOp::Update, target_key: "draw-surface".into(), value: Some(json!({ "label": "Brushwork" })) } },
            ])
        );
        let err = g.page_intent(&project, PageIntent::AnswerMissingApp { seat: "draw".into(), use_app: Some("org.example.ghost".into()) }).unwrap_err();
        assert!(err.contains("isn't one of the options"), "{err}");
        assert_eq!(g.project(&project).unwrap().2, hash, "当てられない答えで書いた");

        // 無しで続ける
        g.page_intent(&project, PageIntent::AnswerMissingApp { seat: "paint".into(), use_app: None }).unwrap();
        let (_, after, after_hash) = g.project(&project).unwrap();
        assert_eq!(after.revision, p.revision + 1);
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!(revs.last().map(|r| r.origin.as_str()), Some("direct_edit"));
        let now = page_by_route(&after, "paint");
        assert_eq!(asked_on(&now), 0, "答えた後に問いが残った");
        assert!(now.root.nodes().iter().any(|n| n.props.get("text").and_then(Value::as_str).is_some_and(|t| t.starts_with("Continuing without Paint X"))), "{:?}", now.root);
        assert!(!g.usable_of(&project).iter().any(|u| u.seat.id == "paint"), "答えた席が使える検査に残った");
        assert_eq!(asked_on(&page_by_route(&after, "draw")), 1, "別の席の問いまで消した");
        let err = g.page_intent(&project, PageIntent::AnswerMissingApp { seat: "paint".into(), use_app: None }).unwrap_err();
        assert!(err.contains("already answered"), "{err}");
        assert_eq!(g.project(&project).unwrap().2, after_hash, "答えた後の 2 回目で書いた");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(asked_on(&page_by_route(&g.project(&project).unwrap().1, "paint")), 1, "Undo で問いが戻らない");

        // 代わりの app: AddSurface（W16M-3）が当たる build だけ面を置く。当たらない build は理由の 1 行で何も書かない（todo!() で落とさない）
        let (_, before, before_hash) = g.project(&project).unwrap();
        match g.page_intent(&project, PageIntent::AnswerMissingApp { seat: "draw".into(), use_app: Some("org.example.brushwork".into()) }) {
            Ok(_) if ADD_SURFACE_BUILT => {
                let (_, now, _) = g.project(&project).unwrap();
                assert_eq!(now.revision, before.revision + 1);
                assert!(now.surface_specs.iter().any(|s| s.id == "draw" && s.target == project::SurfaceTarget::App { bundle_id: "org.example.brushwork".into(), file: Some("docs/a.svg".into()) }), "{:?}", now.surface_specs);
                assert_eq!(asked_on(&page_by_route(&now, "draw")), 0, "面を置いた後に問いが残った");
                // FX3 反証の直し: 面の node に app の名前（composer の面〈Notes Maker〉と同じ label）= ⌘Z の 1 句に bundle id の切れ端（brushwork）を出さない
                assert_eq!(undo_scope_line(&now), "Changes inside Notes Maker and Brushwork stay");
            }
            Err(e) if !ADD_SURFACE_BUILT => {
                assert!(e.contains("この版にまだありません"), "{e}");
                // BG（docs/evidence/W13b）で「✕ 失敗しました」の語で出ていた: 何も失敗していない・問いが待っている
                // UI-6a（60.5）: この行に押せる button は無いので ▣ でもなく Ready（理由の文だけ）
                assert_eq!(g.page_line.as_ref().map(|(s, _)| *s), Some(RegionState::Ready), "答えの拒否を失敗の語か ▣ で出した");
                assert_eq!(g.project(&project).unwrap().2, before_hash, "当てられない op で書いた");
            }
            other => panic!("ADD_SURFACE_BUILT = {ADD_SURFACE_BUILT}: {other:?}"),
        }
    }

    /// W13b: `ADD_SURFACE_BUILT` がこの build の `project::apply` と合っている（trunk は W16M-3 の本体で AddSurface が当たる＝ true）。
    /// 負の対照: `ADD_SURFACE_BUILT` を false にする → 赤。
    #[test]
    fn w13b_add_surface_guard_matches_this_build() {
        let mut g = fresh();
        let project = w13b_project(&mut g);
        let (_, p, _) = g.project(&project).unwrap();
        let ops = missing_app_ops(&p, "draw", Some("org.example.brushwork")).unwrap();
        let built = std::panic::catch_unwind(|| project::apply_unchecked(&p, &ops)).is_ok();
        assert_eq!(built, ADD_SURFACE_BUILT, "AddSurface が当たる = {built}。ADD_SURFACE_BUILT をこれに合わせる");
    }

    /// W13b: 問いの答えは使える検査の 1 行（AX の `usable:<seat>`）の子の link と button。AX で押すと draw_page が `AnswerMissingApp` を返す。
    /// 反証の直し: link の文字に行き先の host を出す（押す前に model が選んだ URL の行き先を確かめられる）・AddSurface の当たらない build では
    /// 「代わりに <app>」を出さない（押すと必ず理由の 1 行になる button を置かない）・Page の本文に前の版の 1 行（「入れてから…送ってください」）を描かない
    /// （上の問いと食い違う指示を 2 つ出さない）。
    /// 負の対照: `draw_usable` が choices を渡さない（`&[]` に戻す）→ group の子に button が無く赤・host を文字に足さない / guard を外す / 本文から 1 問の node を
    /// 外さない → それぞれ赤。
    #[test]
    fn w13b_the_answers_are_children_of_the_usable_group_and_press_by_ax() {
        use egui::accesskit::{Action, ActionRequest, Node, NodeId, Role};
        let mut g = fresh();
        let project = w13b_project(&mut g);
        let page = page_by_route(&g.project(&project).unwrap().1, "draw").id;
        g.open_project(&project, Some(&page)).unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let frame = |g: &mut Generated, events: Vec<egui::Event>| {
            let mut got = None;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events, ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got = g.draw_page(ui, &project, &page));
            });
            (out, got)
        };
        frame(&mut g, vec![]);
        let update = frame(&mut g, vec![]).0.platform_output.accesskit_update.expect("AccessKit の木");
        let nodes: std::collections::HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let group = nodes.iter().find(|(_, n)| n.author_id() == Some("usable:draw")).map(|(id, _)| *id).expect("usable:draw の group");
        fn under(id: NodeId, nodes: &std::collections::HashMap<NodeId, &Node>, out: &mut Vec<(NodeId, Role, String)>) {
            for c in nodes[&id].children() {
                if let Some(n) = nodes.get(c) {
                    out.push((*c, n.role(), n.label().unwrap_or("").to_owned()));
                    under(*c, nodes, out);
                }
            }
        }
        let mut kids = Vec::new();
        under(group, &nodes, &mut kids);
        let answers: Vec<(Role, &str)> = kids.iter().filter(|(_, r, _)| matches!(r, Role::Button | Role::Link)).map(|(_, r, l)| (*r, l.as_str())).collect();
        let instead = [(Role::Button, "Use Brushwork instead")];
        let want: Vec<(Role, &str)> = [(Role::Link, "Open the page to install Zeta Edit (example.org)")].into_iter().chain(instead.into_iter().filter(|_| ADD_SURFACE_BUILT)).chain([(Role::Button, "Continue without Zeta Edit")]).collect();
        // 本文に描いた、問いと食い違う 1 行（前の版の「入れてから…」）。答えと 1 つの assert で比べる（3 つの直しの崩れが 1 回の赤に全部出る）
        let old_line = "入れてから、もう一度一言を送ってください";
        let said: Vec<&str> = nodes.values().flat_map(|n| [n.label(), n.value()]).flatten().filter(|s| s.contains(old_line)).collect();
        assert_eq!((answers, said), (want, Vec::<&str>::new()), "ADD_SURFACE_BUILT = {ADD_SURFACE_BUILT}");
        let presses = [("Use Brushwork instead", Some("org.example.brushwork")), ("Continue without Zeta Edit", None)];
        for (label, want) in presses.into_iter().filter(|(l, _)| ADD_SURFACE_BUILT || !l.starts_with("代わりに")) {
            let target = kids.iter().find(|(_, _, l)| l == label).map(|k| k.0).unwrap();
            let press = egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target, data: None });
            let got = frame(&mut g, vec![press]).1.or_else(|| frame(&mut g, vec![]).1);
            assert_eq!(got, Some(Intent::Page(PageIntent::AnswerMissingApp { seat: "draw".into(), use_app: want.map(Into::into) })), "{label}");
        }
    }

    // ---- PV0 細い通し 1 本: 無い app を承認の後に入れて、同じ席を本物の面に替える（偽の brew・この Mac に何も入れない）----

    /// Notes（在る app）と Game（無い Godot・model の偽の頁と偽の bundle id・代わりの Brushwork）の Project を、偽の brew の `g` に保存して開く。
    /// 返すのは (project, 偽の brew の dir)。`mode` は `acquire::pv0_tests::fake_brew` の mode。
    fn pv0_project(g: &mut Generated, name: &str, mode: &str) -> (String, std::path::PathBuf) {
        use crate::project::acquire::pv0_tests::{fake_brew, listing, temp, URL};
        use composer::project::KnownApp;
        let d = temp(name);
        g.homebrew = acquire::Homebrew { install_limit: Duration::from_millis(800), ..acquire::Homebrew::at(fake_brew(&d, mode, &listing("4.7.2", URL)), d.join("apps")) };
        let apps = [KnownApp { name: "Notes Maker".into(), bundle_id: "org.example.notes".into() }, KnownApp { name: "Brushwork".into(), bundle_id: "org.example.brushwork".into() }];
        let plan = json!({ "shape": "project", "name": "Game", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null },
            { "title": "Game", "kind": "app", "app_name": "Godot", "bundle_id": "com.evil.godot", "file": null, "url": null, "install_url": "https://evil.example/godot.dmg", "instead": [{ "name": "Brushwork", "bundle_id": "org.example.brushwork" }], "page": 1, "slot": "main", "why": "", "feeds": null }
        ]});
        let plan = composer::project::plan_project(&format!("Make a 2D game in Godot pv0-{name}"), &PlanOf(plan), &apps, &[], &new_ctx(), None).unwrap().expect("project の形");
        let View::Project { project, .. } = g.save_plan(plan, None).unwrap() else { panic!() };
        (project, d)
    }

    /// 席の 1 問の 1 行と答え（`missing_answers` を画面と同じ材料で呼ぶ）。答え = (文字, link の URL, intent)。
    fn pv0_answers(g: &Generated, project: &str, seat: &str) -> (Option<(RegionState, String)>, Vec<(String, Option<String>, Option<PageIntent>)>) {
        let (_, p, _) = g.project(project).unwrap();
        let asked = p.pages.iter().find_map(|pg| asked_node(pg, seat).map(|(_, s)| s)).expect("1 問");
        g.missing_answers(project, seat, asked.app_name.as_deref().unwrap_or_default(), &asked)
    }

    fn pv0_labels(a: &[(String, Option<String>, Option<PageIntent>)]) -> Vec<&str> {
        a.iter().map(|(t, ..)| t.as_str()).collect()
    }

    /// 裏の導入が終わる（その席の今が `done` を満たす）まで `poll_installs` を回す。
    fn pv0_wait(g: &mut Generated, project: &str, seat: &str, done: impl Fn(Option<&Install>) -> bool) {
        let until = std::time::Instant::now() + patient(Duration::from_secs(10));
        loop {
            g.poll_installs();
            if done(g.installs.get(&(project.to_owned(), seat.to_owned()))) {
                return;
            }
            assert!(std::time::Instant::now() < until, "導入が終わらない");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pv0_start(seat: &str, act: InstallAct) -> PageIntent {
        PageIntent::Install { seat: seat.into(), act }
    }

    /// PV0 の通し（PV-026）: 目的の一言 → 無い Godot の 1 問に `Install Godot & continue`（表の 1 件・link は表の公式の頁で model の偽の頁でない）→
    /// 押す = 何も入れずに review の 1 件（版・発行元・出所・変更）→ Cancel で 1 問に戻る → もう一度 → `Approve and install` → 偽の brew が 1 回だけ入れる →
    /// 読み戻した bundle id で 1 問が本物の面に替わる（1 回の DirectEdit・他の席は変えない・一言を打ち直さない）・`Compose (6 of 6)` の 1 行。
    /// 負の対照: `poll_installs` の Verified の腕で placed_ops を当てない → 面が無く赤／`Start` で resolve でなく install を走らせる → calls で赤。
    #[test]
    fn pv0_install_and_continue_places_the_installed_app_without_asking_again() {
        use crate::project::acquire::pv0_tests::calls;
        let mut g = fresh();
        let (project, d) = pv0_project(&mut g, "flow", "ok");
        let (_, p, _) = g.project(&project).unwrap();
        let notes_hash = page_hash(&page_by_route(&p, "notes"));
        let (over, a) = pv0_answers(&g, &project, "game");
        assert_eq!(over, None, "導入の前に 1 行を差し替えた");
        assert_eq!(pv0_labels(&a), ["Install Godot & continue", "Open the page to install Godot (godotengine.org)", "Use Brushwork instead", "Use Godot in the browser (editor.godotengine.org)", "Continue without Godot"]);
        assert_eq!(a[0].2, Some(pv0_start("game", InstallAct::Start)));
        assert_eq!(a[1].1.as_deref(), Some("https://godotengine.org/"), "model の頁（evil.example）を link にした");

        // 押す = review（何も入れない）
        g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
        assert_eq!(calls(&d), ["info --cask --json=v2 godot"], "review の前に入れた");
        let (over, a) = pv0_answers(&g, &project, "game");
        let (state, line) = over.expect("review の 1 行");
        assert_eq!(state, RegionState::Blocked);
        assert!(line.starts_with("Approve & Install (4 of 6) · Will install Godot 4.7.2") && line.contains("Source: github.com") && !line.contains("evil"), "{line}");
        assert_eq!(pv0_labels(&a), ["Approve and install", "Cancel"]);
        // Cancel → 1 問に戻る（何も入れていない）
        g.page_intent(&project, pv0_start("game", InstallAct::Cancel)).unwrap();
        assert!(g.installs.is_empty() && pv0_answers(&g, &project, "game").0.is_none());

        g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
        g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| i.is_none());
        let installs: Vec<String> = calls(&d).into_iter().filter(|c| c.starts_with("install")).collect();
        assert_eq!(installs, [format!("install --cask --appdir={} godot", d.join("apps").display())], "1 回だけ入れていない");

        let (_, after, _) = g.project(&project).unwrap();
        assert_eq!(after.revision, p.revision + 1, "1 回の op でない");
        assert!(after.surface_specs.iter().any(|s| s.id == "game" && s.target == project::SurfaceTarget::App { bundle_id: "org.godotengine.godot".into(), file: None }), "{:?}", after.surface_specs);
        assert!(!after.surface_specs.iter().any(|s| matches!(&s.target, project::SurfaceTarget::App { bundle_id, .. } if bundle_id == "com.evil.godot")), "model の bundle id を使った");
        assert_eq!(asked_on(&page_by_route(&after, "game")), 0, "面を置いた後に問いが残った");
        assert_eq!(page_hash(&page_by_route(&after, "notes")), notes_hash, "別の席の Page を変えた");
        let line = g.page_line.clone().expect("済みの 1 行");
        assert_eq!(line.0, RegionState::Verified);
        assert!(line.1.starts_with("Compose (6 of 6) · Installed Godot 4.7.2"), "{}", line.1);
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!(revs.last().map(|r| r.origin.as_str()), Some("direct_edit"));
    }

    /// 既に在る Godot（Spotlight に出ていなかった）は入れない: 押すと brew を 1 回も叩かず、読み戻した app の面に替わる（ACQ-02・二重に入れない）。
    /// 負の対照: resolve の先頭の readback を外す → info が呼ばれ review になって赤。
    #[test]
    fn pv0_an_app_already_on_this_mac_is_not_installed_again() {
        use crate::project::acquire::pv0_tests::calls;
        let mut g = fresh();
        let (project, d) = pv0_project(&mut g, "present", "ok");
        let plist = d.join("apps/Godot.app/Contents");
        std::fs::create_dir_all(&plist).unwrap();
        std::fs::write(plist.join("Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.godotengine.godot</string><key>CFBundleShortVersionString</key><string>4.6</string></dict></plist>"#).unwrap();
        g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| i.is_none());
        assert_eq!(calls(&d), Vec::<String>::new(), "在る app で brew を叩いた");
        let (_, after, _) = g.project(&project).unwrap();
        assert!(after.surface_specs.iter().any(|s| s.id == "game" && s.target == project::SurfaceTarget::App { bundle_id: "org.godotengine.godot".into(), file: None }));
        assert!(g.page_line.as_ref().is_some_and(|(s, l)| *s == RegionState::Verified && l.contains("already on this Mac")), "{:?}", g.page_line);
    }

    /// Homebrew の無い Mac: Install を出さない（押しても動かない button を作らない）・link は表の公式の頁・intent で来ても理由の 1 行で何も走らせない。
    /// 表に無い app（Zeta Edit）の 1 問は今まで通り（model の頁・Install 無し）。
    /// 負の対照: `missing_answers` が brew の有無を見ない → Install が出て赤。
    #[test]
    fn pv0_without_homebrew_there_is_no_install_button_and_homebrew_is_not_installed() {
        let mut g = fresh();
        let (project, _) = pv0_project(&mut g, "nobrew", "ok");
        g.homebrew.brew = None;
        let (_, a) = pv0_answers(&g, &project, "game");
        assert_eq!(pv0_labels(&a), ["Open the page to install Godot (godotengine.org)", "Use Brushwork instead", "Use Godot in the browser (editor.godotengine.org)", "Continue without Godot"]);
        let err = g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap_err();
        assert!(err.contains("Homebrew isn't on this Mac") && err.contains("godotengine.org"), "{err}");
        assert!(g.installs.is_empty(), "brew の無い Mac で導入を始めた");
        // 表に無い app は今まで通り
        let other = w13b_project(&mut g);
        assert_eq!(pv0_labels(&pv0_answers(&g, &other, "draw").1), ["Open the page to install Zeta Edit (example.org)", "Use Brushwork instead", "Continue without Zeta Edit"]);
    }

    /// 入らなかった（brew が 1 で終わった）: 理由の 1 行と、公式の頁・前の答えだけ（Install も Retry も出さない）・Project は変えない。
    /// Homebrew が別の bundle id の app を置いた: Unsure（Homebrew が走った事・Check again と前の答え・「入った」とも「Nothing was placed」とも言わない）・Project は変えない。
    /// 負の対照: Failed の答えに Install を足す → 赤／install の別の bundle id の腕を Failed に戻す → wrong で赤／check の別の物の腕を Failed に戻す → Check again で赤。
    #[test]
    fn pv0_a_failed_install_says_why_and_offers_no_retry() {
        for (mode, state, why, first) in [("fail", RegionState::Error, "Download failed", "Open the page to install Godot (godotengine.org)"), ("wrong", RegionState::Partial, "com.example.other", "Check again")] {
            let mut g = fresh();
            let (project, _) = pv0_project(&mut g, &format!("failed-{mode}"), mode);
            let hash = g.project(&project).unwrap().2;
            g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
            pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
            g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
            pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Failed(_) | Install::Unsure(_))));
            let (over, a) = pv0_answers(&g, &project, "game");
            let (got, line) = over.expect("理由の 1 行");
            assert!(got == state && line.contains(why) && !line.contains("Nothing was placed") && !line.contains("Found") && !line.contains("Installed"), "{mode}: {got:?} {line}");
            assert_eq!(pv0_labels(&a), [first, "Use Brushwork instead", "Use Godot in the browser (editor.godotengine.org)", "Continue without Godot"], "{mode}");
            assert_eq!(g.project(&project).unwrap().2, hash, "{mode}: 入らなかったのに書いた");
            if mode == "wrong" {
                // Check again も同じ別の app なら Unsure のまま（Failed の「何も置いていない」にしない）
                g.page_intent(&project, pv0_start("game", InstallAct::Check)).unwrap();
                pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Unsure(_) | Install::Failed(_))));
                assert!(matches!(g.installs.get(&(project.clone(), "game".to_owned())), Some(Install::Unsure(l)) if l.contains("com.example.other")), "Check again が別の app を Failed にした");
            }
        }
    }

    /// 入ったか分からない（時間切れ）: `Check again`（読み戻しだけ）と前の答え・Install も Retry も出さない。Godot.app が来ていれば Check again で面に替わる。
    /// 負の対照: Check の腕で install を走らせる → calls の install が 2 行で赤。
    #[test]
    fn pv0_an_uncertain_install_offers_check_again_not_retry() {
        use crate::project::acquire::pv0_tests::calls;
        let mut g = fresh();
        let (project, d) = pv0_project(&mut g, "unsure", "slow");
        g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
        g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Unsure(_))));
        let (over, a) = pv0_answers(&g, &project, "game");
        assert!(over.as_ref().is_some_and(|(s, l)| *s == RegionState::Partial && l.contains("Check again")), "{over:?}");
        assert_eq!(pv0_labels(&a), ["Check again", "Use Brushwork instead", "Use Godot in the browser (editor.godotengine.org)", "Continue without Godot"]);
        // 別の席・別の手は Unsure の間に入れ直さない
        assert!(g.page_intent(&project, pv0_start("game", InstallAct::Approve)).is_err());
        // Godot.app が来た（Homebrew が後で終えた）→ Check again で面に替わる・brew は叩き直さない
        let plist = d.join("apps/Godot.app/Contents");
        std::fs::create_dir_all(&plist).unwrap();
        std::fs::write(plist.join("Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.godotengine.godot</string><key>CFBundleShortVersionString</key><string>4.7.2</string></dict></plist>"#).unwrap();
        g.page_intent(&project, pv0_start("game", InstallAct::Check)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| i.is_none());
        assert!(g.project(&project).unwrap().1.surface_specs.iter().any(|s| s.id == "game"));
        assert_eq!(calls(&d).iter().filter(|c| c.starts_with("install")).count(), 1, "Check again で入れ直した");
    }

    /// 承認の後に listing の版が替わった: 入れずに替わった中身の review に戻る（1 行に替わった事と新しい版・Approve と Cancel だけ）→ もう一度の承認で 1 回だけ入れる。
    /// 負の対照: `poll_installs` の Changed の腕を Failed にする → review に戻らず赤。
    #[test]
    fn pv0_a_listing_changed_after_approval_goes_back_to_review() {
        use crate::project::acquire::pv0_tests::{calls, listing, URL};
        let mut g = fresh();
        let (project, d) = pv0_project(&mut g, "changed", "ok");
        g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
        std::fs::write(d.join("info.json"), listing("4.7.3", URL).to_string()).unwrap();
        g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Changed(_))));
        let installs = || calls(&d).iter().filter(|c| c.starts_with("install")).count();
        assert_eq!(installs(), 0, "替わった listing で入れた");
        let (over, a) = pv0_answers(&g, &project, "game");
        let (state, line) = over.expect("review の 1 行");
        assert!(state == RegionState::Blocked && line.contains("changed after you approved") && line.contains("Will install Godot 4.7.3"), "{line}");
        assert_eq!(pv0_labels(&a), ["Approve and install", "Cancel"]);
        g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
        pv0_wait(&mut g, &project, "game", |i| i.is_none());
        assert_eq!(installs(), 1);
        assert!(g.page_line.as_ref().is_some_and(|(_, l)| l.contains("Installed Godot 4.7.2")), "{:?}", g.page_line);
    }

    /// 2 つ目の導入は 1 つ目が走っている間は始めない（Homebrew は 1 本ずつ）・答えた後の 1 問には Install が効かない。
    /// 負の対照: install_act の「走っている」の門を外す → 2 つ目が Resolving になって赤。
    #[test]
    fn pv0_one_install_at_a_time_and_none_after_the_question_is_answered() {
        let mut g = fresh();
        let (first, _) = pv0_project(&mut g, "one", "slow");
        let homebrew = g.homebrew.clone();
        let (second, _) = pv0_project(&mut g, "two", "ok");
        g.homebrew = homebrew;
        g.page_intent(&first, pv0_start("game", InstallAct::Start)).unwrap();
        pv0_wait(&mut g, &first, "game", |i| matches!(i, Some(Install::Review(_))));
        g.page_intent(&first, pv0_start("game", InstallAct::Approve)).unwrap();
        let err = g.page_intent(&second, pv0_start("game", InstallAct::Start)).unwrap_err();
        assert!(err.contains("Wait for"), "{err}");
        assert!(!g.installs.contains_key(&(second.clone(), "game".to_owned())));
        pv0_wait(&mut g, &first, "game", |i| matches!(i, Some(Install::Unsure(_))));
        // 答えた 1 問（無しで続ける）には効かない
        g.page_intent(&second, PageIntent::AnswerMissingApp { seat: "game".into(), use_app: None }).unwrap();
        assert!(g.page_intent(&second, pv0_start("game", InstallAct::Start)).is_err());
    }

    /// AX: `Install Godot & continue` は使える検査の group（`usable:game`）の子の button で、AX で押すと `PageIntent::Install{Start}` が返る。
    /// 負の対照: draw_usable が missing_answers の intent でなく AnswerMissingApp を返す → 赤。
    #[test]
    fn pv0_the_install_button_is_a_child_of_the_usable_group_and_presses_by_ax() {
        use egui::accesskit::{Action, ActionRequest, Node, NodeId, Role};
        let mut g = fresh();
        let (project, _) = pv0_project(&mut g, "ax", "ok");
        let page = page_by_route(&g.project(&project).unwrap().1, "game").id;
        g.open_project(&project, Some(&page)).unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let frame = |g: &mut Generated, events: Vec<egui::Event>| {
            let mut got = None;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), events, ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got = g.draw_page(ui, &project, &page));
            });
            (out, got)
        };
        frame(&mut g, vec![]);
        let update = frame(&mut g, vec![]).0.platform_output.accesskit_update.expect("AccessKit の木");
        let nodes: std::collections::HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let group = nodes.iter().find(|(_, n)| n.author_id() == Some("usable:game")).map(|(id, _)| *id).expect("usable:game の group");
        let mut stack = vec![group];
        let mut target = None;
        while let Some(id) = stack.pop() {
            for c in nodes[&id].children() {
                if let Some(n) = nodes.get(c) {
                    if n.role() == Role::Button && n.label() == Some("Install Godot & continue") {
                        target = Some(*c);
                    }
                    stack.push(*c);
                }
            }
        }
        let target = target.expect("group の子に Install の button が無い");
        let press = egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target, data: None });
        let got = frame(&mut g, vec![press]).1.or_else(|| frame(&mut g, vec![]).1);
        assert_eq!(got, Some(Intent::Page(pv0_start("game", InstallAct::Start))));
    }

    // ---- PV1（PV-006・ACQ-03・ACQ-12）: 1 問の `Use <名前> in the browser` と「無しで続ける」の文 ----

    /// 表に web 版が在る app の 1 問は `Use Godot in the browser (editor.godotengine.org)`（host を文字に・`Install & continue` の後・無しで続けるの前）。
    /// 押す = 同じ席に WebView の面（URL = 表の web 版・Browser でない）を 1 回の DirectEdit で置く・brew を叩かない・他の Page と別の Project は変えない（ACQ-12）。
    /// 負の対照: UseWeb の腕で model の install_url を置く → URL で赤／placed_ops が backend を APP_SEAT_BACKEND のまま → backend で赤。
    #[test]
    fn pv1_the_question_offers_the_web_version_and_only_that_seat_changes() {
        use crate::project::acquire::pv0_tests::calls;
        let mut g = fresh();
        let (other, _) = pv0_project(&mut g, "web-other", "ok");
        let (project, d) = pv0_project(&mut g, "web", "ok");
        let other_hash = g.project(&other).unwrap().2;
        let (_, p, _) = g.project(&project).unwrap();
        let notes = page_hash(&page_by_route(&p, "notes"));
        let (_, a) = pv0_answers(&g, &project, "game");
        assert_eq!(pv0_labels(&a), ["Install Godot & continue", "Open the page to install Godot (godotengine.org)", "Use Brushwork instead", "Use Godot in the browser (editor.godotengine.org)", "Continue without Godot"]);
        let web = a.iter().find(|x| x.0.starts_with("Use Godot in the browser")).and_then(|x| x.2.clone()).expect("web 版の答え");
        assert_eq!(web, pv0_start("game", InstallAct::UseWeb));
        g.page_intent(&project, web).unwrap();
        let (_, after, _) = g.project(&project).unwrap();
        let spec = after.surface_specs.iter().find(|s| s.id == "game").expect("web の面");
        assert_eq!(spec.backend, project::SurfaceBackendKind::WebView);
        assert_eq!(spec.target, project::SurfaceTarget::Url { url: "https://editor.godotengine.org/releases/latest/".into() }, "model の頁（evil.example）を置いた");
        assert!(!spec.browser, "URL の欄を持つ Browser にした");
        assert_eq!(after.revision, p.revision + 1, "1 回の op でない");
        assert_eq!(asked_on(&page_by_route(&after, "game")), 0, "面を置いた後に問いが残った");
        assert_eq!(page_hash(&page_by_route(&after, "notes")), notes, "別の席の Page を変えた");
        assert_eq!(g.project(&other).unwrap().2, other_hash, "別の Project を変えた");
        assert_eq!(calls(&d), Vec::<String>::new(), "web 版で brew を叩いた");
        // 置いた後の 1 問には効かない
        assert!(g.page_intent(&project, pv0_start("game", InstallAct::UseWeb)).is_err());
    }

    /// 表に web 版の無い app（Zeta Edit）の 1 問には web の答えを出さない・「無しで続ける」の後の文は一言の送り直しを求めない（PV-006）。
    /// 負の対照: web の答えを表を見ずに出す → Zeta Edit の並びで赤／文を元に戻す → request again で赤。
    #[test]
    fn pv1_no_web_answer_without_a_web_version_and_skipping_asks_for_nothing() {
        let mut g = fresh();
        let other = w13b_project(&mut g);
        assert!(!pv0_labels(&pv0_answers(&g, &other, "draw").1).iter().any(|l| l.contains("browser")));
        let (project, _) = pv0_project(&mut g, "skip", "ok");
        g.page_intent(&project, PageIntent::AnswerMissingApp { seat: "game".into(), use_app: None }).unwrap();
        let page = page_by_route(&g.project(&project).unwrap().1, "game");
        let texts: Vec<String> = page.root.nodes().iter().filter_map(|n| n.props.get("text").and_then(Value::as_str).map(str::to_owned)).collect();
        assert!(texts.iter().any(|t| t == "Continuing without Godot"), "{texts:?}");
        assert!(!texts.iter().any(|t| t.contains("request again")), "一言の送り直しを求めた: {texts:?}");
    }

    // ---- FX3 Page の帯を最後まで（Interface の段・caller の形。本体の前は todo!() か assert で赤）----

    /// 読書記録の Project を開き、`n` 枚に足して（`Add` は末尾に `Page {n}`）、(project, 全 Page の id) を返す。
    fn fx3_project(g: &mut Generated, n: usize) -> (String, Vec<String>) {
        let app = g.create("読書記録をつけたい").unwrap();
        let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
        g.open_project(&project, Some(&page)).unwrap();
        for _ in 1..n {
            g.page_intent(&project, PageIntent::Add).unwrap();
        }
        let ids = g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).collect();
        (project, ids)
    }

    fn fx3_ready(s: &str) -> Option<(RegionState, String)> {
        Some((RegionState::Ready, s.to_owned()))
    }

    /// PANEL-SYNC-1101 interface（caller の形）: 畳んだ panel だけ窓の一番下の帯の材料が在る（Project = Chat の語・＋ に `New browser tab`・Home の Terminal = Chat 無し）・
    /// 開くと None（同じ tab を 2 か所に出さない）。負の対照（未実行・owner 186 / 187）: `folded` の `if panel.open { return None }` を外す → 開いた後も Some で赤。
    #[test]
    fn panel_sync_folded_tabs_only_while_the_panel_is_folded() {
        let mut g = fresh();
        let (project, _) = fx3_project(&mut g, 1);
        let f = g.folded(&project).expect("畳んだ Project の panel の材料が無い");
        assert_eq!((f.chat.as_deref(), f.browser), (Some("Chat"), true));
        let home = g.folded(HOME_PLACE).expect("畳んだ Home の Terminal の材料が無い");
        assert_eq!((home.chat, home.browser), (None, false));
        g.panel_act(&project, PanelAct::Chat).unwrap();
        assert!(g.folded(&project).is_none(), "開いた panel の tab を帯にも出した");
    }

    /// TITLEBAR-ICONS-1101 の 2'（案 A・FX3 の「tab 5 つ ＋ More」を置き換えた）: Page は何枚でも全部が題の帯の tab（`More` は無い・溢れた分は横の scroll）・
    /// ＋ は最後の tab のすぐ右・••• は帯の右端。道は AX の名前と矩形（BG の driver と同じ）。5 枚以下の帯の名前は前と同じ。
    /// 負の対照: `page_tabs` の 5 枚に戻す → 7 枚目が AX に無く赤・＋ を右の組（••• の左）に戻す → ＋ と最後の tab の間が開いて赤。
    #[test]
    fn titlebar1101_every_page_is_a_tab_and_plus_follows_the_last_one() {
        let mut g = fresh();
        let (project, _) = fx3_project(&mut g, 7);
        let p = g.project(&project).unwrap().1;
        let last = p.pages[6].clone();
        let mut ax = AxPage::new();
        ax.frame(&mut g, &project, &last.id, vec![]);
        ax.frame(&mut g, &project, &last.id, vec![]);
        let band = ax.labels();
        for pg in &p.pages {
            assert!(band.contains(&pg.title), "{} の tab が帯に無い: {band:?}", pg.title);
        }
        assert!(!band.iter().any(|l| l == "More"), "More が残る: {band:?}");
        let (tab, plus, options) = (ax.rect(&last.title), ax.rect("Add page · ⌘T"), ax.rect("Page options"));
        assert!(plus.left() >= tab.right() - 0.5 && plus.left() - tab.right() <= theme::SP_2 + 1.0, "＋ が最後の tab のすぐ右に無い: tab {tab:?} ＋ {plus:?}");
        assert!(options.left() > plus.right() + theme::BUTTON_H, "••• が帯の右端に無い: ＋ {plus:?} ••• {options:?}");
    }

    /// FX3（FX-008）: ⌘K に今の Project の全 Page の `Go to {Page}`（帯に見えない 6 枚目からも行ける）・打った文で絞れる・無い Project は空。
    #[test]
    fn fx3_palette_goes_to_every_page() {
        let mut g = fresh();
        let (project, _) = fx3_project(&mut g, 7);
        let p = g.project(&project).unwrap().1;
        let rows = g.page_commands(&project);
        let want: Vec<(String, Intent)> = p.pages.iter().map(|pg| (format!("Go to {}", pg.title), Intent::Go(View::Project { project: project.clone(), page: pg.id.clone() }))).collect();
        assert_eq!(rows.iter().map(|c| (c.label.clone(), c.intent.clone())).collect::<Vec<_>>(), want);
        let hits = crate::ui::palette::commands(&crate::ui::shell::Shell::new(View::Runs), "page 7", &[], &rows);
        assert_eq!(hits[0].intent, want[6].1, "打って 7 枚目へ: {hits:?}");
        assert!(g.page_commands("project.none").is_empty(), "無い Project は空");
    }

    /// FX3（FX-009・UIUX B02・B09）: `•••` の `Rename…` → tab の席に名前の欄（AX の名前 `Page name`・focus）→ Esc（`dismiss_rename`）は何も変えない →
    /// もう 1 回開いて打って Enter = `PageIntent::Rename` → 当てると DB から読み戻した名前・帯の tab もその名前・Page の上の 1 行。道は AX の Click・SetValue・Enter。
    #[test]
    fn fx3_rename_from_page_options_opens_the_name_field_and_enter_renames() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 2);
        let b = ids[1].clone();
        let mut ax = AxPage::new();
        let open = |ax: &mut AxPage, g: &mut Generated| {
            ax.frame(g, &project, &b, vec![]);
            assert_eq!(ax.press(g, &project, &b, "Page options"), None);
            assert_eq!(ax.press(g, &project, &b, "Rename…"), None, "1 回目は名前を替えない（欄を開くだけ）");
            ax.frame(g, &project, &b, vec![]);
            assert!(ax.has("Page name"), "tab の席に名前の欄が無い: {:?}", ax.labels());
            assert_eq!(ax.focused(), Some("Page name"), "開いた欄に focus");
        };
        open(&mut ax, &mut g);
        assert!(g.dismiss_rename(), "Esc の段: 開いた欄を閉じる");
        assert!(!g.dismiss_rename(), "閉じた後の Esc は何も閉じない");
        ax.frame(&mut g, &project, &b, vec![]);
        assert!(!ax.has("Page name"), "Esc で閉じる: {:?}", ax.labels());
        assert_eq!(g.project(&project).unwrap().1.page(&b).unwrap().title, "Page 2", "Esc は何も変えない");
        open(&mut ax, &mut g);
        let field = ax.nodes.iter().find(|(_, l)| l == "Page name").unwrap().0;
        let set = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action: egui::accesskit::Action::SetValue, target: field, data: Some(egui::accesskit::ActionData::Value("Tests".into())) });
        ax.frame(&mut g, &project, &b, vec![set]);
        let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let got = ax.frame(&mut g, &project, &b, vec![enter]).or_else(|| ax.frame(&mut g, &project, &b, vec![]));
        assert_eq!(got, Some(Intent::Page(PageIntent::Rename { page: b.clone(), title: "Tests".into() })), "Enter で名前替え");
        g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "Tests".into() }).unwrap();
        assert_eq!(g.project(&project).unwrap().1.page(&b).unwrap().title, "Tests", "DB の Project を読み戻した名前");
        assert_eq!(g.page_line, fx3_ready("Renamed “Page 2” to “Tests”"));
        let band = page_ax(&mut g, &project, &b);
        assert!(band.iter().any(|l| l == "Tests") && !band.iter().any(|l| l == "Page name"), "帯の tab が新しい名前・欄は閉じた: {band:?}");
    }

    /// FX3（FX-009）: 別の Page と同じ名前（前後の空白・大文字小文字を見ない）は `There's already a page named “…”.` で何も書かない・今と同じ名前は revision を足さない・
    /// 大文字小文字だけの違いは名前替え。拒否は `router::rename_page` の `DuplicateTitle`（帯の欄の 1 か所）。
    #[test]
    fn fx3_rename_refuses_another_pages_name_and_same_name_writes_nothing() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 3);
        let (a, b) = (ids[0].clone(), ids[1].clone());
        let rev = |g: &Generated| g.project(&project).unwrap().1.revision;
        let r0 = rev(&g);
        g.pending_rename = Some((b.clone(), "  PAGE 3 ".into()));
        let err = g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "  PAGE 3 ".into() }).unwrap_err();
        assert_eq!(err, "There's already a page named “PAGE 3”.");
        assert_eq!(rev(&g), r0, "拒否は何も書かない");
        assert_eq!((g.page_line.clone(), g.pending_rename.clone()), (fx3_ready(&err), Some((b.clone(), "  PAGE 3 ".into()))), "拒否は Ready の 1 行・欄は打った名前のまま開いている");
        assert_eq!(g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "  ".into() }), Err("Enter a page name".to_owned()), "空の名前");
        assert_eq!((g.page_line.clone(), rev(&g)), (fx3_ready("Enter a page name"), r0), "空も Ready の 1 行で何も書かない");
        g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: " Page 2 ".into() }).unwrap();
        assert_eq!(rev(&g), r0, "今と同じ名前は revision を足さない");
        g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "page 2".into() }).unwrap();
        assert_eq!((g.project(&project).unwrap().1.page(&b).unwrap().title.clone(), rev(&g)), ("page 2".to_owned(), r0 + 1), "大文字小文字だけの違いは名前替え");
        let p = g.project(&project).unwrap().1;
        let title_a = p.page(&a).unwrap().title.clone();
        assert_eq!(router::rename_page(&p, &b, &title_a), Err(ProjectPatchError::DuplicateTitle(title_a.clone())));
        // 反証の直し: 同じ名前の Page は Add でも作れる（[A, page 2, Page 3] → page 2 を消す → Add = `Page {len+1}` = 2 枚目の Page 3）・保存済みの data にも在る。
        // その Page の欄を名前のまま Enter = 閉じるだけ（前は別の Page と同じ名前の拒否が先で、欄が開いたまま Esc しか出口が無かった）
        g.page_intent(&project, PageIntent::Delete { page: b.clone() }).unwrap();
        let c = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let titles = g.project(&project).unwrap().1.pages.iter().map(|pg| pg.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles.iter().filter(|t| *t == "Page 3").count(), 2, "同じ名前の 2 枚: {titles:?}");
        let r1 = rev(&g);
        g.pending_rename = Some((c.clone(), "Page 3".into()));
        assert!(g.page_intent(&project, PageIntent::Rename { page: c.clone(), title: "Page 3".into() }).is_ok(), "今の名前のまま Enter は拒まない");
        assert_eq!((rev(&g), g.pending_rename.clone()), (r1, None), "何も書かず欄を閉じる");
    }

    /// FX3（FX-011・FX-012・UIUX B04・B10）: 帯の手が通ると Page の上に何が変わったかの 1 行（Ready）・Undo は何が戻らないか（`undo_scope_line`）と ⇧⌘Z を言う。
    #[test]
    fn fx3_band_lines_say_what_changed_and_what_undo_leaves() {
        let mut g = fresh();
        let (project, _) = fx3_project(&mut g, 1);
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        assert_eq!(g.page_line, fx3_ready("Added page “Page 2” · ⌘Z to undo"));
        g.page_intent(&project, PageIntent::Move { page: b.clone(), index: 0 }).unwrap();
        assert_eq!(g.page_line, fx3_ready("Moved “Page 2” left"));
        g.page_intent(&project, PageIntent::Rename { page: b.clone(), title: "Tests".into() }).unwrap();
        assert_eq!(g.page_line, fx3_ready("Renamed “Page 2” to “Tests”"));
        let scope = undo_scope_line(&g.project(&project).unwrap().1);
        assert_eq!(scope, "Files unchanged", "外の app の面が無い Project");
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(g.page_line, fx3_ready(&format!("Undid: Renamed “Page 2” to “Tests” · {scope} · ⇧⌘Z to redo")));
        g.page_intent(&project, PageIntent::Redo).unwrap();
        assert_eq!(g.page_line, fx3_ready("Redid: Renamed “Page 2” to “Tests”"));
        g.page_intent(&project, PageIntent::Delete { page: b.clone() }).unwrap();
        assert_eq!(g.page_line, fx3_ready("Deleted page “Tests” · ⌘Z to undo"));
        let p = g.project(&project).unwrap().1;
        assert_eq!(change_line(&p, &p, &PageIntent::Placement), None, "帯の形を変えない手は 1 行を作らない");
    }

    /// FX3（FX-057・MH-F）: Undo で戻した Add / Move / Delete を Redo（⇧⌘Z と `↷` と同じ `PageIntent::Redo`）が同じ並びに戻し、Page の上に `Redid: …`。
    #[test]
    fn fx3_redo_brings_back_add_move_and_delete() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 1);
        let order = |g: &Generated| g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).collect::<Vec<_>>();
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let mut cases: Vec<(PageIntent, &str)> = vec![(PageIntent::Move { page: b.clone(), index: 0 }, "Redid: Moved “Page 2” left"), (PageIntent::Delete { page: b.clone() }, "Redid: Deleted page “Page 2”")];
        let after_add = order(&g);
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert_eq!(order(&g), ids, "Undo で足す前");
        g.page_intent(&project, PageIntent::Redo).unwrap();
        assert_eq!((order(&g), g.page_line.clone()), (after_add, fx3_ready("Redid: Added page “Page 2”")));
        for (intent, line) in cases.drain(..) {
            g.page_intent(&project, intent.clone()).unwrap();
            let after = order(&g);
            g.page_intent(&project, PageIntent::Undo).unwrap();
            assert_ne!(order(&g), after, "{intent:?}: Undo で戻る");
            g.page_intent(&project, PageIntent::Redo).unwrap();
            assert_eq!((order(&g), g.page_line.clone()), (after, fx3_ready(line)), "{intent:?}");
        }
    }

    /// FX3（FX-013・UIUX B05）: AI がこの Project の変更を読んでいる間、帯の形を変える手は `One change at a time. …` の 1 行（Ready）で拒み何も書かない（AI の編集が
    /// base_hash で黙って落ちない）。読み終われば通る。本体で `w13_model_reads_on_a_background_thread` の「読んでいる間に人が Page を足す」をこの契約に直す。
    #[test]
    fn fx3_band_waits_while_the_ai_reads_a_change() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 2);
        let (tx, rx) = std::sync::mpsc::channel();
        g.set_llm(Ok(Box::new(Gated(Mutex::new(rx)))));
        assert!(g.start_edit(&project, "Docs Page を足して"), "model が読む");
        let wait = "One change at a time. The last one is still being read.";
        for intent in [PageIntent::Add, PageIntent::Rename { page: ids[1].clone(), title: "Notes".into() }, PageIntent::Move { page: ids[1].clone(), index: 0 }, PageIntent::Delete { page: ids[1].clone() }] {
            assert_eq!(g.page_intent(&project, intent.clone()), Err(wait.to_owned()), "{intent:?}");
            assert_eq!(g.page_line, fx3_ready(wait), "{intent:?}: × Failed にしない");
        }
        assert_eq!(g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).collect::<Vec<_>>(), ids, "何も書かない");
        tx.send(Ok(json!({ "steps": [{ "verb": "add_page", "name": "Docs" }] }))).unwrap();
        let ModelDone::Edited { made: Ok(_), .. } = wait_done(&mut g) else { panic!("AI の編集が当たる（人が帯を変えていない）") };
        assert!(g.page_intent(&project, PageIntent::Add).is_ok(), "読み終わった後は通る");
    }

    /// FX3（FX-014・UIUX B08）: `Add page` の次の frame で、足した Page の tab に keyboard focus（どこに出たかが分かり、そのまま鍵で使える）。
    #[test]
    fn fx3_added_page_tab_takes_focus() {
        let mut g = fresh();
        let (project, _) = fx3_project(&mut g, 1);
        let b = page_of(&g.page_intent(&project, PageIntent::Add).unwrap());
        let mut ax = AxPage::new();
        ax.frame(&mut g, &project, &b, vec![]);
        ax.frame(&mut g, &project, &b, vec![]);
        assert_eq!(ax.focused(), Some("Page 2"), "足した tab に focus: {:?}", ax.labels());
    }

    /// FX3（FX-015・UIUX B06・N05）: 消す確認の文は実際と同じ（打ちかけは保存され ⌘Z で戻る = `ui5a_delete_then_undo_…` が守る）。前の「…is lost.」を出さない。
    #[test]
    fn fx3_delete_question_says_undo_brings_it_back() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 2);
        g.pending_delete = Some(ids[1].clone());
        let t = painted_texts_of(|ui| {
            g.draw_page(ui, &project, &ids[1]);
        });
        assert!(t.iter().any(|x| x == "Delete “Page 2”? ⌘Z brings it back."), "確認の文: {t:?}");
        assert!(!t.iter().any(|x| x.contains("is lost")), "実際と違う文: {t:?}");
    }

    /// FX3（FX-010・UIUX B03）: 帯の tab を掴んで左の tab の中心を越えて離すと `PageIntent::Move`（`•••` の Move left と同じ intent）。動かない click は今まで通り移るだけ。
    /// 道は本物の pointer（押す → 動かす → 離す・frame を分ける）。
    #[test]
    fn fx3_dragging_a_tab_past_its_neighbour_moves_the_page() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 3);
        let title_a = g.project(&project).unwrap().1.page(&ids[0]).unwrap().title.clone();
        let mut ax = AxPage::new();
        ax.frame(&mut g, &project, &ids[2], vec![]);
        let (from, onto) = (ax.rect("Page 3").center(), ax.rect(&title_a).center() - egui::vec2(4.0, 0.0));
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        ax.frame(&mut g, &project, &ids[2], vec![egui::Event::PointerMoved(from), button(from, true)]);
        ax.frame(&mut g, &project, &ids[2], vec![egui::Event::PointerMoved(from.lerp(onto, 0.5))]);
        ax.frame(&mut g, &project, &ids[2], vec![egui::Event::PointerMoved(onto)]);
        let got = ax.frame(&mut g, &project, &ids[2], vec![button(onto, false)]).or_else(|| ax.frame(&mut g, &project, &ids[2], vec![]));
        assert_eq!(got, Some(Intent::Page(PageIntent::Move { page: ids[2].clone(), index: 0 })), "drag で先頭へ");
    }

    /// FX3 反証の直し（FX-010）: 7 枚で今の Page（並びの 7 番目）は帯の 5 つ目の席。その tab を少し動かして自分の席で離す（6px を越える・0.8 秒の長押しと同じ drag）
    /// = 何もしない（前は index 4 へ Move・5 と 6 枚目が右へずれた）。4 つ目の tab（Page 4）の中心を越えて離すと Page 4 の前 = index 3。
    #[test]
    fn fx3_dropping_the_fifth_tab_on_its_own_slot_moves_nothing() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 7);
        let order = |g: &Generated| g.project(&project).unwrap().1.pages.iter().map(|pg| pg.id.clone()).collect::<Vec<_>>();
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let mut ax = AxPage::new();
        ax.frame(&mut g, &project, &ids[6], vec![]);
        let release = |ax: &mut AxPage, g: &mut Generated, from: egui::Pos2, onto: egui::Pos2| {
            ax.frame(g, &project, &ids[6], vec![egui::Event::PointerMoved(from), button(from, true)]);
            ax.frame(g, &project, &ids[6], vec![egui::Event::PointerMoved(from.lerp(onto, 0.5))]);
            ax.frame(g, &project, &ids[6], vec![egui::Event::PointerMoved(onto)]);
            ax.frame(g, &project, &ids[6], vec![button(onto, false)]).or_else(|| ax.frame(g, &project, &ids[6], vec![]))
        };
        let from = ax.rect("Page 7").center();
        assert_eq!(release(&mut ax, &mut g, from, from + egui::vec2(8.0, 0.0)), None, "自分の席で離した drag は手にしない");
        assert_eq!(order(&g), ids, "並びは変わらない");
        let onto = ax.rect("Page 4").center() - egui::vec2(4.0, 0.0);
        assert_eq!(release(&mut ax, &mut g, from, onto), Some(Intent::Page(PageIntent::Move { page: ids[6].clone(), index: 3 })), "Page 4 の中心を越えた = Page 4 の前");
    }

    /// FX3 反証の直し（FX-011・UIUX B03「移動アニメーション」）: Move の後、動いた tab は前の位置から新しい位置へ `motion::travel`（`PAGE`）で滑る。
    /// 押せる席（AX の矩形）は最初の frame から新しい位置・描いた字は前の位置から始まり、寄せ終わると席と同じ。Reduce Motion（Fade）と試験の Cut は即時（`travel` の契約）。
    #[test]
    fn fx3_moved_tab_travels_from_its_old_place() {
        let mut g = fresh();
        let (project, ids) = fx3_project(&mut g, 3);
        let mut ax = AxPage::new();
        crate::ui::motion::set_motion_mode(&ax.ctx, crate::ui::motion::MotionMode::Full);
        ax.frame(&mut g, &project, &ids[2], vec![]);
        ax.frame(&mut g, &project, &ids[2], vec![]);
        // 帯の tab の字（同じ字の title line を除く = tab の AX の矩形の高さの中）の中心の x
        let painted = |ax: &AxPage| {
            let seat = ax.rect("Page 3");
            ax.texts.iter().find(|(t, at)| t == "Page 3" && seat.y_range().contains(at.center().y)).map(|(_, at)| at.center().x).expect("帯の Page 3 の字")
        };
        let before = painted(&ax);
        g.page_intent(&project, PageIntent::Move { page: ids[2].clone(), index: 0 }).unwrap();
        ax.frame(&mut g, &project, &ids[2], vec![]);
        let seat = ax.rect("Page 3").center().x;
        assert!(seat < before - 20.0, "押せる席は新しい位置（先頭）: {seat} / 前 {before}");
        assert!((painted(&ax) - before).abs() < 1.0, "最初の frame の字は前の位置: {} / 前 {before}", painted(&ax));
        for _ in 0..30 {
            ax.frame(&mut g, &project, &ids[2], vec![]);
        }
        assert!((painted(&ax) - seat).abs() < 1.0, "寄せ終わると席と同じ: {} / 席 {seat}", painted(&ax));
    }

    /// G99c: 「No folder」の Project に folder を当てる（`Choose folder…` の窓と、窓に落とした folder の両方が `PageIntent::SetWorkspace` に来る）。
    /// repo の resource と Terminal の作業 folder が入り、「No folder」の行は同じ場所の Terminal になる・同じ folder は書かない・別の folder は Terminal の cwd も替える・
    /// folder でない path と相対 path は何も書かない・Undo で「No folder」に戻る。
    /// 負の対照: `workspace_ops` の行の置き換えを外す（SetWorkspace だけ）→ No folder が残って赤 / apply の terminal_specs の付け替えを外す → 古い cwd で赤。
    #[test]
    fn g99c_choosing_a_folder_fills_the_repo_and_turns_no_folder_into_a_terminal() {
        let mut g = fresh();
        let View::Project { project, page: dev } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let (_, p, hash) = g.project(&project).unwrap();
        assert!(p.resources.is_empty() && p.terminal_specs.is_empty(), "{:?}", p.resources);
        assert!(g.wants_folder(&project), "No folder の行が在るのに folder を待たない");
        let ws = std::env::temp_dir().join(format!("shibubu-g99c-{}", storage::new_id()));
        std::fs::create_dir_all(ws.join("b")).unwrap();
        let dir = ws.display().to_string();

        // folder でない path・相対 path は何も書かない
        let file = ws.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(g.page_intent(&project, PageIntent::SetWorkspace { dir: file.display().to_string() }).is_err());
        assert!(matches!(project::apply(&p, &[ProjectPatchOp::SetWorkspace { dir: "code/x".into() }], &[]), Err(ProjectPatchError::Invalid(_))));
        assert_eq!(g.project(&project).unwrap().2, hash, "folder でない path で書いた");

        // 窓の答えは末尾に / が付く（osascript の POSIX path）
        g.page_intent(&project, PageIntent::SetWorkspace { dir: format!("{dir}/") }).unwrap();
        let (_, after, after_hash) = g.project(&project).unwrap();
        assert_eq!(after.revision, p.revision + 1);
        let revs = g.store.lock().unwrap().list_project_revisions(&project).unwrap();
        assert_eq!(revs.last().map(|r| r.origin.as_str()), Some("direct_edit"));
        let uri = format!("file://{dir}");
        assert_eq!(after.resources.iter().map(|r| (r.id.as_str(), r.uri.as_deref())).collect::<Vec<_>>(), [("repo", Some(uri.as_str()))]);
        assert_eq!(after.terminal_specs.iter().map(|t| t.working_directory.as_str()).collect::<Vec<_>>(), [dir.as_str()]);
        let t = after.page(&dev).unwrap().root.nodes().into_iter().find(|n| n.component == "terminal").cloned().expect("Dev に Terminal が無い");
        assert_eq!(t.props.get("terminal_ref").and_then(Value::as_str), Some(after.terminal_specs[0].id.as_str()));
        assert!(!after.pages.iter().flat_map(|pg| pg.root.nodes()).any(|n| n.props.get("text").and_then(Value::as_str) == Some(composer::project::NO_FOLDER)), "No folder が残った");
        assert!(!g.wants_folder(&project));

        // 同じ folder は書かない
        g.page_intent(&project, PageIntent::SetWorkspace { dir: dir.clone() }).unwrap();
        assert_eq!(g.project(&project).unwrap().2, after_hash, "同じ folder で書いた");

        // 別の folder は Terminal の作業 folder も替える
        let other = ws.join("b").display().to_string();
        g.page_intent(&project, PageIntent::SetWorkspace { dir: other.clone() }).unwrap();
        assert_eq!(g.project(&project).unwrap().1.terminal_specs.iter().map(|t| t.working_directory.clone()).collect::<Vec<_>>(), [other]);

        // Undo 2 回で No folder に戻る
        g.page_intent(&project, PageIntent::Undo).unwrap();
        g.page_intent(&project, PageIntent::Undo).unwrap();
        assert!(g.wants_folder(&project), "Undo で No folder に戻らない");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// G90 (3)（operator 10-28 の候補）: Page の Terminal の最初の起動と Restart は、Project に写した shell（作った時の `/bin/zsh`）でなく今の `$SHELL` で起きる。
    /// 負の対照: `spawn_spec` の `shell: user_shell()` を外す → Project の shell（無い path）で起こせず赤
    #[test]
    fn g90c_terminals_start_and_restart_with_the_shell_of_now() {
        // 結合試験の crate では他の試験が SHELL を /bin/zsh に替えて戻す（`TEST_ENV` の doc）= 起こす時と比べる時の間に替わらないよう lock の中で
        let _env = TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = fresh();
        let View::Project { project, page } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        let (app, p, hash) = g.project(&project).unwrap();
        let parent = p.page(&page).unwrap().root.key.clone();
        let spec = project::TerminalSessionSpec { id: format!("{project}.g90c"), project_id: project.clone(), shell: "/bin/shibubu-g90c-no-such-shell".into(), working_directory: std::env::temp_dir().display().to_string(), keep_alive_on_page_switch: true, command: None };
        g.apply_project(&app, &hash, &[ProjectPatchOp::AddTerminal { spec: spec.clone(), page_id: page.clone(), parent, node_key: "g90c-terminal".into(), index: None }], RevisionOrigin::DirectEdit).unwrap();
        let first = g.ensure_terminal(&project, &spec.id).expect("Project に写した shell で起こそうとした");
        assert_eq!(first.shell, user_shell());
        g.terminals().close(&first.key).unwrap();
        let again = g.restart_terminal(&project, &spec.id).expect("Restart が Project に写した shell で起こそうとした");
        assert_eq!((again.shell.as_str(), again.status.clone()), (user_shell().as_str(), crate::terminal::SessionStatus::Running));
        assert_ne!(again.pid, first.pid);
        g.close_terminals();
    }

    /// L2（review A・DOGFOOD-X 周 2 回 1 #2 = 473f4037 の .app で「Preview plan が page を見せない」）: `start_preview` は作る時と同じ読みを 1 回して
    /// 何も書かず、作る Page と入る app を読むだけの行で返す。同じ文の `create_project`（案の Build now）は読み直さずに読めた案を保存する
    /// （Fake の答えは 1 つ = 読み直せば尽きて名前の表に落ちる）。
    /// 負の対照（l123-acd-arm.sh の P）: `create_project` の `previewed` の腕を外す → Build now が読み直して名前の表で作り赤。
    #[test]
    fn l2_accept_saves_the_previewed_plan_without_asking_again() {
        use crate::llm::fake::Fake;
        let goal = "議事録を example で取りたい l2-preview";
        let plan = json!({ "shape": "project", "name": "議事録", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": "https://example.org/notes", "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null },
            { "title": "Dev", "kind": "terminal", "app_name": null, "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 1, "slot": "main", "why": "", "feeds": null }
        ]});
        let mut g = fresh().with_llm(Ok(Box::new(Fake::new(vec![Ok(plan)]))));
        assert!(g.start_preview(goal, None, &[], None), "model が在れば裏で読む");
        assert_eq!(g.model_busy_line(), Some("Reading your plan"));
        let ModelDone::Previewed { goal: read, lines, .. } = wait_done(&mut g) else { panic!("Preview の読みの答えでない") };
        assert_eq!(read, goal);
        assert_eq!(lines, ["2 pages: Notes · Dev", "Apps: example.org", crate::domain::chat::PLAN_UNMADE], "作る Page と入る app の行");
        assert_eq!((g.rows().len(), g.previewed(goal)), (0, Some(true)), "Preview で書いた・案を置いていない");
        let made = g.create_project(goal, None, None);
        let Ok(Some(View::Project { project, .. })) = &made else { panic!("Build now が読み直して作れない: {made:?}") };
        let (_, p, _) = g.project(project).unwrap();
        assert_eq!(p.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>(), ["Notes", "Dev"], "Build now が読み直して Preview で見せた案と違う物を作った");
        assert_eq!(interpreter_of(&g, project), "model", "Build now が読み直して名前の表で作った");
        assert_eq!(g.previewed(goal), None, "保存した案が残った");
    }

    /// L2（DOGFOOD-X 周 2 回 1 #1 = 473f4037 の .app の「Made Weekly meeting notes: Weekly meeting notes」）: 作った後の会話の最後の行は名前を 1 回だけ言う
    /// （Page 1 枚で名前と同じ = Page の名前を重ねない）。
    /// 負の対照（l123-acd-arm.sh の N）: `save_plan` の 1 枚の腕を外す → 名前 2 回で赤。
    #[test]
    fn l2_the_made_line_says_the_name_once() {
        use crate::llm::fake::Fake;
        let web = |title: &str, n: i64| json!({ "title": title, "kind": "web", "app_name": null, "bundle_id": null, "file": null, "url": format!("https://example.org/{n}"), "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null });
        let one = json!({ "shape": "project", "name": "Weekly meeting notes", "pages_requested": false, "seats": [web("Notes", 1), web("Share", 2)] });
        let mut g = fresh().with_llm(Ok(Box::new(Fake::new(vec![Ok(one)]))));
        let View::Project { project, .. } = g.create_project("weekly notes l2-name", None, None).unwrap().expect("Project") else { panic!() };
        let last = g.chat(&project).last().map(|t| t.text.clone());
        assert_eq!(last.as_deref(), Some("Made Weekly meeting notes. Next, say what to change, e.g. “add a Tests page”"), "名前を 2 回言った");
    }

    /// G104 の試験の model: 受け取った依頼を写し、答えない（Err で終わる）。`takes` = 添付と effort を受けるか。
    struct Seen(Arc<Mutex<Vec<crate::llm::JsonRequest>>>, bool);
    impl crate::llm::Llm for Seen {
        fn label(&self) -> String {
            "seen".into()
        }
        fn send(&self, req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
            self.0.lock().unwrap().push(req.clone());
            Err(crate::llm::LlmError::Timeout { ms: 1 })
        }
        fn takes_files(&self) -> bool {
            self.1
        }
        fn efforts(&self) -> Vec<&'static str> {
            if self.1 { crate::domain::chat::Effort::ALL.map(|e| e.flag()).to_vec() } else { Vec::new() }
        }
    }

    /// G104（CHAT-001・CHAT-005）: Home の一言に添えた file と選んだ effort が model の依頼に乗る（受けない provider には file と effort を乗せない）。
    /// 深く考える段（High 以上）で始めた job は読んでいる 1 行に `Thinking longer`・走っている job の段は後から替えても変わらない。`accepts` は provider の写し。
    /// 負の対照: Asked::send の `req.files` の行を外す → 赤・`req.effort` の行を外す → 赤・takes_files の門を外す → 受けない provider に file が乗って赤。
    #[test]
    fn g104_attachments_and_effort_ride_on_the_request() {
        use crate::domain::chat::{Accepts, AttachKind, Attachment, Effort};
        let attach = vec![Attachment::File { path: "/tmp/g104/notes.md".into(), kind: AttachKind::File, bytes: 3 }];
        for takes in [true, false] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut g = fresh().with_llm(Ok(Box::new(Seen(seen.clone(), takes))));
            let efforts = if takes { Effort::ALL.to_vec() } else { Vec::new() };
            assert_eq!(g.accepts(), Accepts { ai: true, provider: "This AI".into(), photos: takes, files: takes, efforts, ..Default::default() });
            g.effort = Some(Effort::High);
            assert!(g.start_create(DEMO, None, &attach), "{takes}");
            let line = g.model_busy_line().unwrap().to_owned();
            g.effort = Some(Effort::Low);
            assert_eq!(g.model_busy_line().unwrap(), line, "走っている job の段は固定");
            assert_eq!(line, if takes { "Reading your request · Thinking longer" } else { "Reading your request" });
            wait_done(&mut g);
            let reqs = seen.lock().unwrap().clone();
            assert!(!reqs.is_empty(), "model を呼んでいない");
            for r in &reqs {
                if takes {
                    assert_eq!((r.files.clone(), r.effort.as_deref()), (vec![std::path::PathBuf::from("/tmp/g104/notes.md")], Some("high")));
                } else {
                    assert_eq!((r.files.len(), r.effort.as_deref()), (0, None), "受けない provider に乗せた");
                }
            }
        }
        assert_eq!(fresh().accepts(), Accepts::default(), "AI 無し");
    }

    /// HOME-ATTACH-1101: 貼った文の添付は file と同じ道（`files`）で model の依頼に乗る = `Pasted text.txt` の中身が貼った文そのもの・欄の文（input）には混ざらない。
    /// 負の対照: Asked::of の `Pasted` の腕を None にする → files が空で赤。
    #[test]
    fn home_attach_pasted_text_rides_as_a_file() {
        use crate::domain::chat::Attachment;
        let text = (1..=412).map(|i| format!("log line {i}\n")).collect::<String>();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut g = fresh().with_llm(Ok(Box::new(Seen(seen.clone(), true))));
        assert!(g.start_create(DEMO, None, &[Attachment::Pasted { text: text.clone(), lines: 412 }]));
        wait_done(&mut g);
        let reqs = seen.lock().unwrap().clone();
        assert!(!reqs.is_empty(), "model を呼んでいない");
        for r in &reqs {
            assert_eq!(r.files.len(), 1, "{:?}", r.files);
            assert_eq!(r.files[0].file_name().and_then(|n| n.to_str()), Some("Pasted text.txt"));
            assert_eq!(std::fs::read_to_string(&r.files[0]).unwrap(), text, "貼った文と違う");
            assert!(!r.input.contains("log line 1\n"), "file の道でなく一言に混ざった");
        }
    }

    /// G84 の試験の model: 受け取った依頼を写し、答えない。`1` = 受ける model の一覧（渡す語, menu の語）。
    struct Picks(Arc<Mutex<Vec<crate::llm::JsonRequest>>>, Vec<(String, String)>);
    impl crate::llm::Llm for Picks {
        fn label(&self) -> String {
            "picks".into()
        }
        fn send(&self, req: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
            self.0.lock().unwrap().push(req.clone());
            Err(crate::llm::LlmError::Timeout { ms: 1 })
        }
        fn models(&self) -> Vec<(String, String)> {
            self.1.clone()
        }
    }

    /// PROVIDER-1101（interface・owner 180 通目「Ollama を選んでも Update Claude Code to attach pictures と出る」）: 選んだ AI の `accepts` は
    /// その AI の名前・effort の段・model の一覧で、押せない理由の語もその AI の名前（Claude Code と言わない）。
    /// 負の対照: `Accepts::refusal` の Photo の腕を Claude の語に戻すと「Update Claude Code to attach pictures」で赤。
    #[test]
    fn provider1101_accepts_speak_for_the_chosen_ai() {
        use crate::domain::chat::{Accepts, AttachKind, Effort};
        struct Named;
        impl crate::llm::Llm for Named {
            fn label(&self) -> String {
                "grok/1.0".into()
            }
            fn send(&self, _: &crate::llm::JsonRequest) -> Result<Value, crate::llm::LlmError> {
                Err(crate::llm::LlmError::Timeout { ms: 1 })
            }
            fn provider_name(&self) -> String {
                "Grok".into()
            }
            fn efforts(&self) -> Vec<&'static str> {
                vec![Effort::Low.flag(), Effort::High.flag()]
            }
            fn models(&self) -> Vec<(String, String)> {
                vec![("grok-4".into(), "grok-4".into())]
            }
        }
        let a = fresh().with_llm(Ok(Box::new(Named))).accepts();
        assert_eq!((a.provider.as_str(), a.efforts.clone(), a.models.clone()), ("Grok", vec![Effort::Low, Effort::High], vec![("grok-4".to_owned(), "grok-4".to_owned())]), "Budgeted で包んでも Grok の物");
        assert!(a.model_ok("grok-4") && !a.model_ok("opus"));
        let lines = [
            a.refusal(Some(AttachKind::Photo)),
            a.refusal(Some(AttachKind::File)),
            Accepts { efforts: Vec::new(), ..a.clone() }.refusal(None),
            Accepts { models: Vec::new(), ..a.clone() }.model_refusal(),
        ];
        for line in &lines {
            let line = line.as_deref().expect("押せない理由の 1 行");
            assert!(line.contains("Grok") && !line.contains("Claude"), "今の provider の名前で言わない: {line}");
        }
    }

    /// G84（Model ▸）: 選んだ別名は provider が受ける時だけ依頼に乗る（受けない provider・表に無い語・Default は None = `--model` を付けない）。
    /// `accepts` の models は provider の写し（Budgeted で包んでも同じ）。
    /// 負の対照（g-b の変異 gb_asked）: Asked::send の `model_ok` の門を外す → 受けない provider に乗って赤。
    #[test]
    fn g84_the_chosen_model_rides_only_when_the_provider_takes_it() {
        let list = |names: &[&str]| names.iter().map(|n| (n.to_string(), n.to_uppercase())).collect::<Vec<_>>();
        let all: Vec<&str> = crate::domain::chat::MODELS.iter().map(|(a, _)| *a).collect();
        for (mask, model, want) in [(list(&["opus"]), Some("opus"), Some("opus")), (list(&[]), Some("opus"), None), (list(&all), Some("haiku"), None), (list(&["opus"]), None, None)] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut g = fresh().with_llm(Ok(Box::new(Picks(seen.clone(), mask.clone()))));
            assert_eq!(g.accepts().models, mask, "Budgeted が models を言わない");
            g.model = model.map(str::to_owned);
            assert!(g.start_create(DEMO, None, &[]));
            wait_done(&mut g);
            let reqs = seen.lock().unwrap().clone();
            assert!(!reqs.is_empty(), "model を呼んでいない");
            assert!(reqs.iter().all(|r| r.model.as_deref() == want), "{mask:?} {model:?}: {:?}", reqs.iter().map(|r| r.model.clone()).collect::<Vec<_>>());
        }
    }

    /// G86（operator 10-29 CU ③・qa .app 1a655d7c）: Undo で Tests の Page を戻すと、会話の最後に「Undid the last change · Pages now: {Pages}」（Tests が無い・Project の名前は入れない）・
    /// Redo で「Redid the change · …」（Tests が在る）。会話の無い Project には足さない。負の対照（g-b の変異 gb_undo）: Undo / Redo の say を外す → 最後が Changed のままで赤。
    #[test]
    fn g86_undo_and_redo_say_the_pages_of_now_in_the_chat() {
        let dir = w1_dir("g86-undo");
        let mut g = Generated::open(&dir);
        let View::Project { project, .. } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap();
        let last = |g: &mut Generated| g.chat(&project).last().map(|t| t.text.clone()).unwrap_or_default();
        assert!(last(&mut g).starts_with("Changed ") && last(&mut g).contains("Tests"), "{}", last(&mut g));
        g.page_intent(&project, PageIntent::Undo).unwrap();
        let undid = last(&mut g);
        assert!(undid.starts_with("Undid the last change · ") && !undid.contains("Tests"), "{undid}");
        g.page_intent(&project, PageIntent::Redo).unwrap();
        let redid = last(&mut g);
        assert!(redid.starts_with("Redid the change · ") && redid.contains("Tests"), "{redid}");
        for needle in ["Undo", "Redo", "Done", "Saved", "Failed", "Shibubu Demo"] {
            assert!(!undid.contains(needle) && !redid.contains(needle), "27 手の needle {needle}: {undid} / {redid}");
        }
    }

    /// G86a（owner 10-27 11:2x「自分のプロンプトの履歴が見えるようにchat式で」）: 会話の列 = Home の一言と作った物・変更の一言と答え・拒否の 1 行（Stopped）が
    /// この順で残り、同じ data dir で開き直すと同じ会話が戻る（#25）。Engine の card は Page に描かない（右の列だけ・同じ button を 2 か所に描かない）。
    /// 負の対照（G86M・g86m.py）: save = say の保存を外す → 開き直すと会話が空 / page = Page にも engine_view::draw を戻す → Page の AX に Approve and send
    #[test]
    fn g86_the_chat_comes_back_after_a_restart_and_the_page_draws_no_card() {
        use crate::domain::chat::{Mark, Who};
        use crate::ui::engine_view::{ApprovalStep, EngineView, APPROVE_LABEL};
        let dir = w1_dir("g86");
        let mut g = Generated::open(&dir);
        let View::Project { project, page } = g.create_project(DEMO, None, None).unwrap().unwrap() else { panic!() };
        g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap();
        g.page_intent(&project, PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap_err();
        let said: Vec<(Who, String, Mark)> = g.chat(&project).into_iter().map(|t| (t.who, t.text, t.mark)).collect();
        assert_eq!(said.len(), 6, "{said:?}");
        assert_eq!((said[0].0, said[0].1.as_str(), said[0].2), (Who::You, DEMO, Mark::None), "{said:?}");
        assert!(said[1].0 == Who::Shibubu && said[1].1.starts_with("Made ") && said[1].1.contains(':'), "{said:?}");
        assert_eq!((said[2].0, said[2].1.as_str()), (Who::You, "Tests Page を足して"));
        assert!(said[3].0 == Who::Shibubu && said[3].1.starts_with("Changed ") && said[3].1.contains("Tests"), "{said:?}");
        assert_eq!((said[4].0, said[5].0, said[5].2), (Who::You, Who::Shibubu, Mark::Stopped), "拒否は Stopped の 1 行: {said:?}");
        assert!(g.chat("project.other").is_empty(), "他の Project の会話は混ざらない");
        drop(g);
        let mut g = Generated::open(&dir);
        assert_eq!(g.chat(&project).into_iter().map(|t| (t.who, t.text, t.mark)).collect::<Vec<_>>(), said, "開き直すと同じ会話");
        let step = ApprovalStep { action: "pad.save".into(), input: json!({ "app_name": "Pad" }), digest: "d".into(), scopes: vec![] };
        g.engine_views.insert(project.clone(), EngineView::Approve { goal: "Save it".into(), steps: vec![step], line: None });
        let ax = page_ax(&mut g, &project, &page);
        assert!(!ax.iter().any(|l| l == APPROVE_LABEL || l == "Cancel"), "Page に承認の card を描いた: {ax:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // PV0 の反証（review・yc/review-pv）の攻撃の試験: PV の試験と file を分ける（include! はこの file の dir から引く = どの crate でも同じ file）
    mod pv0_attack {
        use super::*;
        include!("../tests/pv0_attack/generated.rs");
    }

    fn surf(id: &str, target: project::SurfaceTarget) -> project::SurfaceSpec {
        project::SurfaceSpec { id: id.into(), backend: project::SurfaceBackendKind::Mirror, target, keep_alive: true, frame: None, browser: false }
    }

    fn page_with(id: &str, surface_id: &str) -> project::PageDefinition {
        project::PageDefinition {
            id: id.into(),
            route: format!("/{id}"),
            title: id.into(),
            app_ref: None,
            root: crate::domain::ui::UiNode {
                key: "root".into(),
                component: "column".into(),
                children: vec![crate::domain::ui::UiNode {
                    key: surface_id.into(),
                    component: "source_surface".into(),
                    props: serde_json::from_value(json!({ "surface_ref": surface_id })).unwrap(),
                    ..crate::domain::ui::UiNode::default()
                }],
                ..crate::domain::ui::UiNode::default()
            },
            revision: 1,
        }
    }

    fn proj(pages: Vec<project::PageDefinition>, specs: Vec<project::SurfaceSpec>) -> ProjectDefinitionV1 {
        ProjectDefinitionV1 {
            id: "p".into(),
            name: "p".into(),
            goal: String::new(),
            app_refs: vec![],
            pages,
            resources: vec![],
            terminal_specs: vec![],
            surface_specs: specs,
            revision: 1,
        }
    }

    /// short-r3: 今の Page が Diagram（app の面が 0）で、一言が README を名指す → TextEdit と README.md。
    /// 今の Page に app が在ればそれを使う（一言が別の書類を名指しても代用しない）。名指しが 0 個・2 個、今の Page の app が 2 つ、は None。
    /// 負の対照: `engine_target` の fallback（`let mut hits` の直前）を `return None` にすると、Diagram の腕だけ赤。今の Page の腕は緑のまま。
    #[test]
    fn short_r3_names_the_readme_when_this_page_has_no_app() {
        let readme = || surf("docs", project::SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("README.md".into()) });
        let notes = || surf("notes", project::SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("notes.md".into()) });
        let diagram = || surf("draw", project::SurfaceTarget::Url { url: "https://excalidraw.com".into() });
        let goal = "Change the README title to “Shibubu Demo” and save it";
        let p = proj(vec![page_with("readme", "docs"), page_with("draw", "draw")], vec![readme(), diagram()]);
        assert_eq!(engine_target(&p, p.page("draw"), goal), Some(("com.apple.TextEdit".into(), Some("README.md".into()))));
        assert_eq!(engine_target(&p, p.page("readme"), goal), Some(("com.apple.TextEdit".into(), Some("README.md".into()))));
        assert_eq!(engine_target(&p, p.page("draw"), "draw a box"), None, "名指しが無い");
        let two = proj(vec![page_with("readme", "docs"), page_with("notes", "notes"), page_with("draw", "draw")], vec![readme(), notes(), diagram()]);
        assert_eq!(engine_target(&two, two.page("draw"), "Change the README and the notes"), None, "書類が 2 つ");
        assert_eq!(engine_target(&two, two.page("readme"), "edit the notes"), Some(("com.apple.TextEdit".into(), Some("README.md".into()))), "今の Page の app を一言で取り換えない");
        assert_eq!(engine_target(&two, two.page("draw"), "edit the notes"), Some(("com.apple.TextEdit".into(), Some("notes.md".into()))));
        let two_apps = proj(vec![page_with_two("mix", "docs", "term")], vec![
            readme(),
            surf("term", project::SurfaceTarget::App { bundle_id: "com.apple.Terminal".into(), file: None }),
        ]);
        assert_eq!(engine_target(&two_apps, two_apps.page("mix"), goal), None, "今の Page の app が 2 つなら別 Page で代用しない");
    }

    fn page_with_two(id: &str, a: &str, b: &str) -> project::PageDefinition {
        let mut page = page_with(id, a);
        page.root.children.push(crate::domain::ui::UiNode {
            key: b.into(),
            component: "source_surface".into(),
            props: serde_json::from_value(json!({ "surface_ref": b })).unwrap(),
            ..crate::domain::ui::UiNode::default()
        });
        page
    }
}
