use crate::adapters::{AdapterRegistry,Caller,sdk};
use crate::domain::{permission::{ConsentDecision,ConsentReceipt,ConsentSubject},run::{Observation,Outcome}};
use crate::gui::{Control,WindowRef,WindowState};
use crate::{domain::capability::DisplayTier,engine,generated::Generated,github,layout,model::*,repos,ui_tree};
use crate::ui::{self,build_progress::{Build,BuildAction,Phase},connections::{Connection,ConnectionsModel},shell::{AppRow,Intent,PageIntent,SettingsSection,Shell,Tier,View},states::RegionState,terminal_panel::PanelAct,theme::{self,Type,Variant}};
use eframe::egui::{self, Color32, RichText, Vec2,Pos2,Rect,Stroke,Align2,FontId,CursorIcon,Sense};
use std::{collections::HashMap, env, path::{PathBuf,Path},sync::{mpsc::{channel,Receiver,Sender},Arc,Mutex},thread,time::{Duration,Instant}};

/// Pointer movement since the press that started the current drag (egui 0.31 has no `Response::total_drag_delta`).
/// Panels snap to the 8px grid on every move, so per-frame `drag_delta` would get stuck below one grid step.
fn total_drag_delta(ui:&egui::Ui)->Option<Vec2>{
    ui.input(|i|i.pointer.press_origin().zip(i.pointer.latest_pos()).map(|(origin,now)|now-origin))
}

enum Event {
    Result {session:u64,action:u64,title:String,result:Result<String,String>},
    LaneStarted {session:u64},
    LaneFinished {session:u64},
    Windows {session:u64,result:Result<Vec<WindowRef>,String>},
    Controls {session:u64,pid:u32,window_id:i64,result:Result<Vec<Control>,String>},
    Inspect(Result<repos::RepoInspection,String>),
    Imported {session:u64,result:Result<RepoFeature,String>},
    GuiStatus(DisplayTier,String),
    /// FX5: GitHub の行の読み戻し（状態・account 名などの 1 文・確認した時刻 `HH:MM`）
    GithubStatus(DisplayTier,String,String),
    /// RQ4: Connections の行の確かめの答え（行の key・段・detail の 1 行）
    Checked{key:String,tier:DisplayTier,detail:String},
    /// RQ4（RQ-015）: 1 つの欄に打った Web data の URL を保存の前に 1 回 GET した答え（Err = HTTP の status〈無ければ 0〉と理由の 1 行）
    WebTested{url:String,result:Result<(),(u16,String)>},
    /// RQ4（RQ-017）: Choose program… の macOS の選ぶ窓の答え（None = 取り消した）
    ProgramChosen(Result<Option<PathBuf>,String>),
    /// PASSWORD-1101 の 1: password manager の sign in と項目の数（Err = card と行の 1 行）
    Vault{m:crate::domain::vault::Manager,result:Result<usize,String>},
    /// PASSWORD-1101 の 2: Paste の欄の下に出す項目（繋いだ manager の名前と参照だけ・manager ごとに 1 回）
    VaultItems(Vec<crate::domain::vault::SecretRef>),
    /// PASSWORD-1101 の 3: chat で止めた値を保存した答え（Err = 値と保存先を held に戻す・理由の 1 行）
    VaultSaved(Result<crate::domain::vault::SecretRef,(crate::domain::vault::Secret,crate::domain::vault::Manager,String)>),
}
/// G99c: folder を選ぶ macOS の窓（`osascript` の choose folder・Standard Additions）。選んだ folder の POSIX path（末尾に /）・取り消し・起こせない時は None。
/// HOME-ATTACH-1101: ⌘V の時の clipboard の file（Finder で ⌘C した file・folder の URL）。無い・読めない時は空（文の貼り付けのまま）。
fn clipboard_files()->Vec<PathBuf>{arboard::Clipboard::new().and_then(|mut c|c.get().file_list()).unwrap_or_default()}
fn choose_folder()->Option<String>{
    let out=std::process::Command::new("/usr/bin/osascript").args(["-e","POSIX path of (choose folder with prompt \"Choose the folder for this project\")"]).output().ok()?;
    let dir=String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success()&&!dir.is_empty()).then_some(dir)
}
/// 背景の thread から UI への口: 送ったら egui を起こす（W2b-AX 直し・src/main.rs の契約。idle の間は frame を回さないので、
/// 結果が届いた事を `poll` に知らせるのはこれだけ）。送る側の形は `Sender` と同じ（`let _=tx.send(..)`）。
#[derive(Clone)]
struct Tx{tx:Sender<Event>,ctx:egui::Context}
impl Tx{
    fn send(&self,e:Event)->Result<(),std::sync::mpsc::SendError<Event>>{self.tx.send(e)?;self.ctx.request_repaint();Ok(())}
}
/// 保存の debounce（打っている間は書かず、止まってから 1 回）。
const SAVE_DEBOUNCE:Duration=Duration::from_millis(700);
/// Page に WebView / Native Direct の面が在る間の poll の間隔（W2b-AX 直しの前の常時の pump と同じ）。
const SURFACE_POLL:Duration=Duration::from_millis(230);
#[derive(Clone)]
struct Pending {session:u64,title:String,actions:Vec<Action>,other_lane:Option<Vec<Action>>}

pub struct ShibubuApp {
    store:Store,data_path:PathBuf,tx:Tx,rx:Receiver<Event>,
    prompt:String,repo:String,
    results:HashMap<u64,Result<String,String>>,pending:Option<Pending>,
    windows:Vec<WindowRef>,controls:Vec<Control>,selected_window:usize,selected_control:usize,
    show_import:bool,import_repo:String,import_path:String,review:Option<repos::RepoInspection>,
    license_accepted:bool,cli_editor:HashMap<u64,(String,String)>,
    message:String,editing:bool,dirty:bool,last_save:Instant,
    selected_panel:Option<u64>,delete_session_prompt:bool,
    delete_panel_prompt:Option<u64>,running:HashMap<u64,usize>,
    drag_start:Option<(u64,f32,f32)>,resize_start:Option<(u64,f32,f32)>,
    active_resources:Arc<Mutex<std::collections::HashSet<String>>>,
    shell:Shell,gui_status:Option<(DisplayTier,String)>,gui_checking:bool,gui_checked:Option<Instant>,github_status:Option<(DisplayTier,String,String,Instant)>,github_checking:bool,
    /// 一言目から作った app（composer → runtime → renderer・src/generated.rs）
    generated:Generated,
    /// W3: 主デモの「この repo」（`SHIBUBU_WORKSPACE_DIR`・起動の時に読む）。CHAT-ONE-1101: Home の chat bar の folder の icon で選び直せる（Home の会話の folder）
    workspace:Option<PathBuf>,
    /// W13: 利用者の LLM の CLI を確かめる裏の thread の答え（起動を止めない・届いたら `Generated::set_llm` に 1 回渡して None）
    llm_rx:Option<Receiver<Result<Box<dyn crate::llm::Llm>,crate::llm::LlmError>>>,
    /// L2: 会話を埋める model の答え（同時に 1 本）。(どの会話か = None は Home・Some は Project の Chat〈CHAT-ONE-1101〉, Home の差し替える Shibubu の Turn の位置, 答え)。
    talk_rx:Option<(Option<String>,usize,Receiver<crate::generated::TalkMsg>)>,
    /// L4 → PROVIDER-1101: Mac の中で見つかった AI（在る CLI 全部〈login していない物は `needs_sign_in`〉・動いている local LLM）の答え
    found_rx:Option<Receiver<Vec<AiAccount>>>,
    /// PROVIDER-1101: Claude Code の CLI がこの Mac に在るか（`refresh_agent_setup` が file を見て書く・見るまでは true）。無ければ Provider に Claude Code の行を出さない
    claude_here:bool,
    /// RQ2（RQ-ACCT）: account を替えて確かめ直している間の、前の account の (id, 種類)（確かめが通らなければこれに戻す・None = 替えていない）
    llm_switch:Option<(Option<String>,Option<String>)>,
    /// ACCOUNTS-1101 2 段目: 切り替えた時に開いていた Project（確かめが通ったらその Project に覚える・確かめの数秒で画面が移っても切り替えた所に書く）
    llm_switch_for:Option<String>,
    /// ACCOUNTS-1101 2 段目: Project ごとの account を戻した Project（替わった frame で戻す・Home などは None）
    account_for:Option<String>,
    /// ACCOUNTS-1101 2 段目: 足した Claude の plan の (id, email)（名前の既定 = @ の前・画面に email は出さない）と読んだ時刻（file を読むので 5 秒に 1 回）
    account_emails:Vec<(String,String)>,emails_at:Option<Instant>,
    /// RQ2: data dir（AI の account の dir と Keychain の鍵の口）
    data_dir:PathBuf,
    /// W13: CLI の確かめの途中に送られた一言と送った時刻（画面を止めずに待つ・最長 30 秒・`update` が続ける）
    waiting:Option<(String,Vec<crate::domain::chat::Attachment>,Instant)>,
    /// G99: AI を最後に探し始めた時刻（窓が前に来た時は 60 秒より古い時だけ探し直す）・Home に「探している」の 1 行を出すか（押した時だけ・起動の確かめは黙って）
    llm_checked:Instant,llm_rechecking:Option<&'static str>,
    /// G99: 前の frame で窓が前面だったか（前に来た frame を見つける）
    was_focused:bool,
    /// G99c: folder を選ぶ窓を開いた Project（None = Home の chat bar・CHAT-ONE-1101）と、その答え（裏の thread・取り消しは None）。開いている間は 2 つ目を開かない
    folder_rx:Option<(Option<String>,Receiver<Option<String>>)>,
    /// G104: `+` の menu の選ぶ窓の答え（種類・選んだ file の path）。裏の thread が 1 回送る
    /// HOME-ATTACH-1101: ⌘V の時に clipboard の file を読む口（製品 = `clipboard_files`・試験は fixture の path を返す fn に替える = owner の clipboard を読まない）
    paste_files:fn()->Vec<PathBuf>,
    /// FX-042: 裏の生成を頼んだ時の View（答えが届いた時に別の場所に居れば、画面を替えずに知らせるだけ）
    create_from:Option<View>,
    /// REVIEW-FX2 b2: 確かめ待ち（`waiting`）に入れた時に居た View（出来た Project が今居る画面を奪わない・待ちの後の `create_from` になる）
    asked_from:Option<View>,
    /// W12W-3 反証: 前の frame の egui の focus（focus が別の widget へ移った frame に、WebView の中の first responder を egui の view へ戻す・`surface::focus_fix`）
    last_focus:Option<egui::Id>,
    /// FX1（FX-001）: 終わる前の保存に失敗した理由。Some の間は作業の場所の頭に `Couldn't save your changes · {理由}` と `Quit anyway` / `Keep open`
    /// （`theme::quit_inline`・手が要るので toast にしない）。書き手は `Intent::Quit` の arm と `Keep open`（None に戻す）だけ。FX1
    quit_failed:Option<String>,
    /// FX1: 終わると決めた（保存できた・`Quit anyway`）。この後の close 要求（自分で送った `ViewportCommand::Close`）は聞き直さずに通す
    quitting:bool,
    /// RQ-ACCT: Add account（None）/ Re-authenticate（その account の id）の browser の sign in の答え（裏の thread・最大 5 分）。在る間は 2 つ目を起こさない
    ai_sign_in:Option<(Option<String>,Receiver<Result<AiAccount,String>>)>,
    /// SIGNIN-1101: 下の panel の tab で走らせている AI の CLI の sign in（同時に 1 つ・`start_cli_sign_in` が作り `poll_cli_sign_in` が見る）
    cli_sign_in:Option<CliSignIn>,
    /// RQ-USAGE: 使用量の帯と読みを持つか（`new` だけ true・試験の `open` は本物の Keychain と usage の口に触らず、AI の有無で AX の木を変えない）
    read_usage:bool,
    /// TITLEBAR-ICONS-1101 の 2'（案 A の ⌘1〜9）: この frame の入力に Terminal の頁（xterm）から来た ⌘1〜9 が在るか（`raw_input_hook` が毎 frame 書く）。
    /// これか egui の grid の Terminal の focus（`ax::terminal_focused`）なら、打った所は Terminal = 今まで通り n 枚目の Terminal の tab（`PanelAct::SelectNth`）・
    /// どちらでもなければ n 枚目の Page（`keyboard` の Project の腕）。
    term_keys:bool,
    /// USAGE-OWN-1101（owner 203 通目）: Shibubu の中だけで使った AI の量の数え手（Settings › AI & Models の Used in Shibubu・`read_usage` の時だけ数える。追記 223 で帯の語は外した）
    own:crate::own_usage::Counter,
    /// RQ-USAGE（operator ①・10-29）: 利用者が使用量のために動いたか（Settings › AI & Models を開いた・帯の `Show usage`）。false の間は Keychain を読まない
    /// （初めて使う人に、自分で起こしていない許可の窓を出さない）。一度 true になればその起動の間は 60 秒ごとに読み直す
    usage_asked:bool,
    /// RQ-ACCT（REVIEW-RQN・operator 1）: 今の Add / Re-authenticate の sign in を止める印（`start_account_sign_in` が sign in ごとに作り直す）。
    /// その account の Remove が立てる → 裏の thread が子を止め、答えは捨てる（消した account に「Signed in again」を言わない）
    sign_in_stop:Arc<std::sync::atomic::AtomicBool>,
    /// REVIEW-RQN（operator 4）: plan の account の Remove の前の logout（`llm_cli::remove_account` の引数）。`new` = `llm_cli::sign_out`（本物の `claude auth logout`）・
    /// 試験の `open` = 何もしない口（試験から Remove に届く道で本物の CLI を起こさない = この Mac の利用者の login を消さない）
    sign_out:fn(&[(&str,Option<String>)])->Result<(),String>,
    /// RQ3: 利用者の Downloads と Desktop（`Export…` の置き場 = 先頭・Import の候補 = 両方）。`new` が HOME から入れる・試験の `open` は空（利用者の folder に書かない）
    user_dirs:Vec<PathBuf>,
    /// 段 2 作り直し（owner 142）: Install CLI & skill（Terminal の panel の 1 行目と Settings › Terminal）。`new` が HOME と `ProjectCli::bin_dir` の `shibubu` から作る・
    /// 試験の `open` は None（利用者の ~/.claude・~/.local/bin に書かない・試験は `Win::agent_home` で fixture の HOME を入れる）。Project の口が無い起動も None
    agent_setup:Option<crate::terminal::AgentSetup>,
    /// RQ4: Connections の行の確かめの答え（行の key → 段と detail）・確かめた時刻（unix 秒）・今確かめている行
    conn_result:HashMap<String,(DisplayTier,String)>,conn_at:std::collections::BTreeMap<String,i64>,conn_checking:std::collections::BTreeSet<String>,
    /// PASSWORD-1101: この Mac に CLI が在る password manager（Connections に入った時に探す = 行を出す・毎 frame は探さない）
    vault_found:Vec<crate::domain::vault::Manager>,
    /// RQ4（RQ-016）: Shibubu の mcp.json の MCP（Connections に入った時と手の後に読み直す・毎 frame 読まない）。他の app の設定は読まない（それは `mcp_found`）
    mcp_engine:crate::mcp::McpConfig,
    /// L4: 他の app（Claude Desktop・Claude Code）の MCP の設定を読んだ答え（`mcp::found_config`）。None = まだ読んでいない（読むのは `mcp-found` の行の
    /// `Look in Claude's settings` を押した時だけ・Connections に入っただけでは読まない）。
    mcp_found:Option<Vec<(&'static str,crate::mcp::McpConfig,std::collections::BTreeSet<String>)>>,
    /// RQ4（RQ-017）: Choose program… で選んだ program（引数を打つ間）と、足す前の確認に出している Trusted command
    cli_draft:Option<(u64,PathBuf)>,cli_pending:Option<RepoFeature>,
    /// RQ4（RQ-015）: Keychain に GitHub の token が在るか（Connections に入った時と Paste の後に書く）。
    /// 行の文はこれを読まない（FX-023: `Token set` / `No token` を出さない。次の一手は Paste token と Check again）。
    #[allow(dead_code)]
    github_keychain:bool,
    /// RQ5（RQ-020）: 今の Page の提案を出す agent と、それが読んだ (Project・Page・版)。変われば `sync_agent` が読み直す
    agent:crate::agent::Agent,agent_key:Option<(String,String,u32)>,
    /// CHAT-011 反証 B28: 最後に agent に渡した今の Page の件数（`Generated::agent_rows`・替わった時だけ observe と refresh）
    agent_seen_rows:std::collections::BTreeMap<String,usize>,
    /// CHAT-FREE-1101 段 2: chat の ```html の閉じた WebView（`Run` を押した 1 つ・`canvas_sync`）
    #[cfg(all(feature="webview",target_os="macos"))]
    canvas:Option<crate::surface::webview::Canvas>,
    /// AI-COMPOSE-1101: Page の AI が書いた部品の閉じた WebView（Project の id・node の key・key・開けなかった時は None = 開き直さない・`part_sync`）
    #[cfg(all(feature="webview",target_os="macos"))]
    parts:Vec<(String,String,u64,Option<crate::surface::webview::Canvas>)>,
}
/// SIGNIN-1101（owner 193）: 下の panel の tab の sign in の見張り（どの AI・どの Project の どの tab・前面に sign in の command を見たか・始めた時刻・最後に読んだ時刻）。
/// 新しい型の理由: `ai_sign_in`（Claude の browser の sign in の答えの受け口）と違い、ここは答えの口の無い PTY の tab を見張る
struct CliSignIn{acct:AiAccount,name:String,project:String,tab:String,saw_running:bool,started:Instant,probed:Instant}
/// RQ-USAGE（operator ①）: 読む前の帯の button（hover の `ui::shell::USAGE_ASK` が、押すと Keychain の Claude Code の sign in を読む = 許可の窓が出得る事を先に言う）
const SHOW_USAGE:&str="Show usage";
/// L2・B45: AI が無い Mac で名前の表でも作れない文（変更の文「一覧に検索を追加して」等）の 1 行（`Preview plan` の行と、案を出さない Home の返事）。
const NEEDS_AI:&str="Shibubu can't build this without an AI yet. Change the plan, or connect an AI in Settings.";
/// G84 × RQ2: account の 1 語（rail の About の `AI` の行と `Provider ▸` の行が同じ語・None = この Mac の Claude Code の login）。鍵の account は従量の印つき。
/// PROVIDER-1101（owner 180 通目「system default いらない」）: None は `Claude Code`・login していない CLI は sign in の印つき。
/// ACCOUNTS-1101 2 段目（作る物 3・5）: 足した account（Claude の plan・CLI）= 名前が在れば `{AI} · {名前}`（`Claude · work`）・無ければ `{AI} {n}`（`Claude 2`）。
/// 名前 = 利用者が付けた名前 → email の @ の前（`emails` = (id, email)・email そのものは出さない）。chat bar・下の帯・usage の内訳・Connections が同じ語。
fn account_label(a:Option<&AiAccount>,ui:&UiSettings,emails:&[(String,String)])->String{
    match a{
        None=>"Claude Code".into(),
        Some(AiAccount::Found{name,needs_sign_in:true,..})=>format!("{name} · sign in first"),
        Some(a@AiAccount::Plan{id})=>{
            let email=emails.iter().find(|(i,_)|i==id).map(|(_,e)|e.as_str());
            match crate::llm_cli::account_name(a,ui.account_name(a),email){
                Some(name)=>format!("{} · {name}",crate::llm_cli::found_name(a.provider())),
                None=>crate::llm_cli::numbered_name(a).unwrap_or_else(||id.clone()),
            }
        }
        Some(AiAccount::Key{last4,..})=>format!("API key ····{last4} · billed per request by Anthropic"),
        Some(AiAccount::EnvKey)=>"Key from environment · billed per request by Anthropic".into(),
        Some(AiAccount::Found{name,..})=>name.clone(),
    }
}
/// G84（③ の決定）→ PROVIDER-1101: `Provider ▸` の行 = Claude Code（None）→ Shibubu が足した plan の login → 利用者が足した API key（足した順）→ 機械で見つけた AI
/// （`llm_cli::found_rows` の順・login していない CLI も）。環境の鍵（EnvKey）は出さない。Claude Code の CLI が無い Mac（`claude_here` = false）は Claude の 3 種を出さない。
/// 行は account そのもの（種類と id の組・operator 10-29 ②）を持ち、使っている行は `UiSettings::active_account` の行（None = Claude Code）。例: `shell.providers=provider_rows(&store.ui,self.claude_here)`
/// ACCOUNTS-1101: 足した CLI の account はその AI の行の直後（CLI が Mac に無い account は出さない = 選んでも呼べない）。
/// ACCOUNTS-1101 2 段目: 行の語は `account_label`（`emails` = 足した Claude の plan の email・名前の既定の元）。
fn provider_rows(ui:&UiSettings,claude_here:bool,emails:&[(String,String)])->Vec<(Option<AiAccount>,String,bool)>{
    let active=ui.active_account();
    let claude=ui.ai_accounts.iter().filter(|a|claude_here&&matches!(a,AiAccount::Plan{..})&&a.provider()=="claude")
        .chain(ui.ai_accounts.iter().filter(|a|claude_here&&matches!(a,AiAccount::Key{..})));
    let found=ui.ai_accounts.iter().filter(|a|matches!(a,AiAccount::Found{..}))
        .flat_map(|f|std::iter::once(f).chain(ui.ai_accounts.iter().filter(move|a|matches!(a,AiAccount::Plan{..})&&a.provider()==f.id())));
    claude_here.then_some(None).into_iter().chain(claude.chain(found).map(Some))
        .map(|a|(a.cloned(),account_label(a,ui,emails),a==active)).collect()
}
/// STATUSBAR-1101 追記（owner 210「providerのusageとかそこら辺が…orcaを参考に」）: 下の帯に並べる AI = `Shell::providers` の全部（今の AI が先頭・残りは providers の順）。
/// 読めるのは今の AI が Claude の plan（System default か Shibubu が足した login）の時だけ（`Generated::plan_usage`）。他は数字を作らず `--` の hover の理由の 1 行。純関数。
/// STATUSBAR-1101 追記 2: `cli` = Claude 以外の AI の読み（`Generated::cli_usages`・key = `AiAccount::id`）= Grok・Kimi Code は今でなくても数字が並ぶ。
/// 例: `ais: bar_ais(&self.shell.providers, &ai, self.generated.plan_usage(), self.generated.cli_usages(), now)`。保証しない: 今でない plan の login の % （その login の Keychain を読むと許可の窓が出得るので読まない）。
fn bar_ais(providers:&[(Option<AiAccount>,String,bool)],ai:&str,usage:Option<&Result<crate::llm::PlanUsage,crate::llm::LlmError>>,cli:&std::collections::BTreeMap<String,Result<crate::llm::PlanUsage,crate::llm::LlmError>>,now:i64)->Vec<ui::shell::BarAi>{
    let mut out:Vec<ui::shell::BarAi>=providers.iter().map(|(acct,label,on)|{
        let name=match acct{
            None if *on=>ai.to_owned(),
            None=>"System default".to_owned(),
            Some(AiAccount::Key{last4,..})=>format!("API key ····{last4}"),
            Some(AiAccount::EnvKey)=>"Key from environment".to_owned(),
            _=>label.clone(),
        };
        // 足した CLI の account（`Plan { id: "grok-1" }`）は Claude の plan でない（provider で見る）
        let claude=acct.as_ref().is_none_or(|a|a.provider()=="claude");
        let read=if claude{usage.filter(|_|*on)}else{acct.as_ref().and_then(|a|cli.get(a.id()))};
        let (windows,why,failed)=match (acct,read){
            (_,Some(r))=>(r.as_ref().map(|u|crate::llm::window_short(u,now)).unwrap_or_default(),crate::llm::usage_line(r,now),crate::llm_cli::usage_step(r)==Some(crate::llm_cli::TRY_AGAIN)),
            (Some(a),None) if !claude&&crate::llm_cli::USAGE_CLIS.contains(&a.provider())=>(Vec::new(),"Usage isn't read yet".to_owned(),false),
            (Some(AiAccount::Found{id,..}),None) if crate::llm_cli::LOCAL_LLMS.iter().any(|(l,_)|*l==id.as_str())=>(Vec::new(),"Runs on this Mac · no usage limit".to_owned(),false),
            (Some(AiAccount::Found{name,..}),None)=>(Vec::new(),format!("Shibubu can't read {name}'s usage yet"),false),
            (Some(a),None) if !claude=>(Vec::new(),format!("Shibubu can't read {}'s usage yet",crate::llm_cli::found_name(a.provider())),false),
            (Some(AiAccount::Key{..}|AiAccount::EnvKey),None)=>(Vec::new(),"Billed per request · no plan limit to show".to_owned(),false),
            (_,None) if *on=>(Vec::new(),"Usage isn't read yet".to_owned(),false),
            (_,None)=>(Vec::new(),"Usage shows for the AI in use · choose it in the AI menu".to_owned(),false),
        };
        ui::shell::BarAi{name,key:acct.as_ref().map_or("claude",|a|a.provider()).to_owned(),active:*on,windows,why,failed}
    }).collect();
    out.sort_by_key(|a|!a.active);
    out
}
/// lane の Ok の 1 行 → 履歴と状態行の文。「業務上の成功は未検証」を含む結果は「完了」にしない（github.rs が同じ語を載せる・mac_gui.rs は英語の同じ文〈UI-2b〉）
fn result_line(title:&str,v:&str)->String{
    if v.contains("業務上の成功は未検証")||v.contains("Whether the work itself succeeded isn't verified") {format!("Accepted (result not verified) {title}: {v}")} else {format!("Done {title}: {v}")}
}

impl ShibubuApp {
    pub fn new(cc:&eframe::CreationContext<'_>)->Self {
        let dir=crate::surface::data_dir(); // FX1: env → ~/Library/Application Support/Shibubu（同意・relay・act の記録と同じ dir）
        // FX1（FX-002 の残り）: env で決めていない起動だけ、FX1 の前の既定（cwd の .shibubu-data・.kangae-data）を写してから読む（写せなければ旧 dir のまま）
        let by_env=["SHIBUBU_DATA_DIR","KANGAE_DATA_DIR"].iter().any(|k|env::var_os(k).is_some_and(|v|!v.is_empty()));
        let dir=match env::current_dir(){Ok(cwd) if !by_env=>crate::surface::adopt_old_data(&dir,&[cwd.join(".shibubu-data"),cwd.join(".kangae-data")]),_=>dir};
        // RQ1: app menu の Settings…（⌘,）。押すと印を立てて egui を起こす（menu の action は egui の event を作らない）。試験の `open` は AppKit に触らない
        #[cfg(target_os="macos")]{
            let c=cc.egui_ctx.clone();
            if let Err(e)=crate::surface::appkit(crate::surface::AppKitOp::InstallSettingsItem(Box::new(move||c.request_repaint()))){eprintln!("shibubu: Settings menu: {e}");}
        }
        let mut app=Self::open(cc,dir);
        app.read_usage=true;
        app.sign_out=crate::llm_cli::sign_out;
        // RQ4: GitHub と Web data の読みに使う token と URL（Keychain を読むので裏で・試験の `open` は env のまま）
        let ui=app.store.ui.clone();
        thread::spawn(move||{sync_connections(&ui);});
        if let Some(home)=env::var_os("HOME").map(PathBuf::from).filter(|h|h.is_absolute()){
            app.user_dirs=vec![home.join("Downloads"),home.join("Desktop")];
            // PROJECT-DIR-1101（owner 204）: Project ごとの folder の親（Terminal が始まる所・試験の `open` は None = 利用者の HOME に作らない）
            app.generated.projects_root=Some(home.join("Shibubu"));
            // 段 2 作り直し: 読むだけ（押されるまで HOME に書かない）。見つかった agent は起動の確かめの後に refresh_agent_setup が入れる
            app.agent_setup=app.generated.terminals().config().project_cli.as_ref().map(|c|crate::terminal::AgentSetup::new(home.clone(),c.bin_dir.join("shibubu")));
        }
        app
    }
    /// `new` の本体（data の置き場を env から読まずに渡す・試験は空の temp dir で本物の app を開く）
    fn open(cc:&eframe::CreationContext<'_>,dir:PathBuf)->Self {
        let mut generated=Generated::open(&dir);
        // MIRROR-SPEED-1101: Mirror の IOSurface を写さずに描く wgpu の口（wgpu でない起動・試験は無い = CPU で写す）
        #[cfg(feature="mirror")]
        if let Some(rs)=&cc.wgpu_render_state{crate::ui::mirror_view::set_gpu(rs.clone());}
        // W12a: Project の resource（repo）を読む口は Adapter の registry（Core は ResourceReader だけを見る・git は adapters の中）
        generated.set_resource_reader(Box::new(crate::adapters::AdapterRegistry::with_local_adapters()));
        generated.wake_on_terminal(&cc.egui_ctx); // W2b-AX 直し: Page の Terminal の出力・shell の終わりで egui を起こす（idle の frame は 0・src/main.rs の契約）
        // W19-2 の配線（図 §33）: Page の欄で編集として読めない文は Engine へ。registry は run ごとに作る（git と画面の操作）・答えが届いたら wake_on_terminal の ctx で起こす
        generated.set_engine(engine_setup(&dir));
        let workspace=crate::project::workspace_from_env().unwrap_or_else(|e|{generated.note=Some(e.to_string());None});
        let path=dir.join("sessions.json");let (mut store,load_note)=Store::load(&path);
        // RQ3 反証の直し: 隔離した設定の 1 行（`Settings › Data & Recovery` で終わる）は Home の note へ（home::draw がその下に `Restore…`・状態の行と二重にしない）。
        // Home の note が既に在る起動（DB も壊れた・戻した）はその 1 行を残し、設定の 1 行は今まで通り状態の行
        let load_note=match load_note{
            Some(n) if n.ends_with("Settings › Data & Recovery")&&generated.note.is_none()=>{generated.note=Some(n);None}
            n=>n,
        };
        // RQ3（RQ-014）: 保存期間を過ぎた記録を起動の 1 回で消す（10 MB の上限つきの JSONL と DB の行・画面に出さない）
        if let Err(e)=generated.prune_records(crate::google::now() as i64,store.ui.history_days){eprintln!("shibubu: keep records: {e}");}
        // RQ2: 選んだ AI の account が列に無ければ System default に戻す（消した account の id を残さない）
        if store.ui.ai_active.is_some()&&store.ui.active_account().is_none(){store.ui.set_active_account(None);}
        if store.sessions.is_empty(){store=Store::default()}
        if store.selected().is_none(){store.selected=store.sessions[0].id}
        // SHIBUBU_START（開発用・写真を撮る時の初期画面）: "home runs connections compat dark light" を空白区切りで。
        let mut view=View::Home;
        let mut pref=match store.ui.theme{ThemeChoice::System=>egui::ThemePreference::System,ThemeChoice::Light=>egui::ThemePreference::Light,ThemeChoice::Dark=>egui::ThemePreference::Dark};
        for token in env::var("SHIBUBU_START").or_else(|_|env::var("KANGAE_START")).unwrap_or_default().split_whitespace(){
            match token{
                "home"=>view=View::Home,"runs"=>view=View::Runs,"connections"=>view=View::Connections,
                "compat"=>view=View::CompatCanvas(store.selected),
                "dark"=>pref=egui::ThemePreference::Dark,"light"=>pref=egui::ThemePreference::Light,_=>{}
            }
        }
        theme::install(&cc.egui_ctx,pref);
        let (tx,rx)=channel();
        let tx=Tx{tx,ctx:cc.egui_ctx.clone()};
        let mut app=Self{repo:env::var("GITHUB_REPO").unwrap_or_default(),prompt:String::new(),store,data_path:path,tx,rx,
            results:HashMap::new(),pending:None,
            windows:vec![],controls:vec![],selected_window:0,selected_control:0,show_import:false,
            import_repo:String::new(),import_path:String::new(),review:None,license_accepted:false,
            cli_editor:HashMap::new(),message:load_note.unwrap_or_default(),
            editing:true,dirty:false,last_save:Instant::now(),
            selected_panel:None,delete_session_prompt:false,
            delete_panel_prompt:None,running:HashMap::new(),
            drag_start:None,resize_start:None,active_resources:Arc::new(Mutex::new(std::collections::HashSet::new())),
            shell:Shell::new(view.clone()),gui_status:None,gui_checking:false,gui_checked:None,github_status:None,github_checking:false,generated,workspace,llm_rx:None,talk_rx:None,found_rx:None,claude_here:true,llm_switch:None,llm_switch_for:None,account_for:None,account_emails:Vec::new(),emails_at:None,data_dir:dir.clone(),waiting:None,last_focus:None,quit_failed:None,quitting:false,user_dirs:Vec::new(),
            conn_result:HashMap::new(),conn_at:Default::default(),conn_checking:Default::default(),vault_found:Vec::new(),mcp_engine:Default::default(),mcp_found:Default::default(),
            cli_draft:None,cli_pending:None,github_keychain:false,agent:crate::agent::Agent::new(),agent_key:None,agent_seen_rows:Default::default(),
            #[cfg(all(feature="webview",target_os="macos"))]canvas:None,
            #[cfg(all(feature="webview",target_os="macos"))]parts:Vec::new(),
            llm_checked:Instant::now(),llm_rechecking:None,was_focused:true,folder_rx:None,create_from:None,asked_from:None,ai_sign_in:None,cli_sign_in:None,read_usage:false,term_keys:false,own:Default::default(),usage_asked:false,sign_in_stop:Default::default(),sign_out:|_|Ok(()),agent_setup:None,paste_files:clipboard_files};
        app.start_llm_detect(&cc.egui_ctx,false);
        // UI-4b（Done 6）: 再起動を跨ぐ打ちかけを場所ごとに戻す（空の文と、保存した行がもう無い Project の key は捨てる・今の場所の分は欄へ）。
        // 今回開けなかった Project（broken）の分は Shell に持ち続ける（行けないので欄には出ないが、次の保存で消さない・定義が直れば戻る）
        let mut drafts=app.store.ui.drafts.clone();
        drafts.retain(|k,v|!v.trim().is_empty()&&(k=="home"||app.generated.project_saved(k)));
        app.shell.command=drafts.remove(view.place()).unwrap_or_default();
        app.shell.drafts=drafts;
        app.shell.palette.recent=app.store.ui.recent_commands.clone();
        // G104: 選んだ effort の段を戻す（読めない語は Default）
        let effort=app.store.ui.default_effort.as_deref().and_then(crate::domain::chat::Effort::from_flag);
        (app.generated.effort,app.shell.effort)=(effort,effort);
        // G84: 選んだ model の別名を戻す（受けない別名は呼ぶ時に付けない = CLI の既定）
        (app.generated.model,app.shell.model)=(app.store.ui.default_model.clone(),app.store.ui.default_model.clone());
        app.agent=crate::agent::Agent::with_feedback(crate::agent::Feedback::from_json(&app.store.ui.agent_feedback).unwrap_or_default());
        if view==View::Connections{app.probe_gui();app.enter_connections();}
        app.apply_settings(&cc.egui_ctx); // RQ1: 保存した Reduce motion・Text size・Execution limit を起動の 1 回で当てる（theme は上の install）
        app.reopen_last_project();
        app
    }
    /// RQ1: Settings の値のうち ctx と model に効く物を当て直す（起動の 1 回と `Intent::SettingsChanged`・theme は `Intent::SetTheme` が当てる）。
    fn apply_settings(&mut self,ctx:&egui::Context){
        match self.store.ui.reduce_motion{Some(on)=>crate::ui::motion::set_reduce_motion(ctx,on),None=>crate::ui::motion::clear_motion_mode(ctx)}
        ctx.set_zoom_factor(f32::from(crate::settings::text_zoom(&self.store.ui))/100.0);
        self.generated.set_budget(self.store.ui.llm_max_calls,self.store.ui.llm_max_secs);
        #[cfg(feature="mirror")]
        crate::surface::mirror::set_smoothness(self.store.ui.mirror_smoothness.arg()); // MIRROR-SPEED-1101: Settings › General › Mirror smoothness
    }
    /// RQ1: ⌘+ / ⌘− / ⌘0（egui の既定の鍵）で替えた文字の大きさを Settings の値へ写す（範囲の外は詰めて当て直す・同じ値なら何もしない）。
    fn sync_text_zoom(&mut self,ctx:&egui::Context){
        let now=(ctx.zoom_factor()*100.0).round() as u16;
        let fit=now.clamp(crate::settings::ZOOM_MIN,crate::settings::ZOOM_MAX);
        if fit!=now{ctx.set_zoom_factor(f32::from(fit)/100.0);}
        if fit!=crate::settings::text_zoom(&self.store.ui){self.store.ui.text_zoom=(fit!=100).then_some(fit);self.mark();}
    }
    /// RQ1（General · When Shibubu opens = Last project）: 起動の画面が Home なら最後に開いた Project を開く。消えた Project の id は読み捨て、
    /// 開けなければ Home のまま理由の 1 行（外に効く操作は起こさない = Project を開くだけ）。DB を開けなかった起動（`Generated::saves` が false）は
    /// 消えたのではない: id を残し「deleted」と言わない（RQ1 反証の直し・`project_saved` もその起動では true）。
    fn reopen_last_project(&mut self){
        let last=self.store.ui.last_project.clone();
        if let Some(p)=&last{
            if !self.generated.project_saved(p){
                self.store.ui.last_project=None;self.mark();
                if self.store.ui.reopen_last_project{self.message="Couldn't reopen your last project · it was deleted".into();}
                return;
            }
        }
        let (Some(project),true,View::Home)=(last,self.store.ui.reopen_last_project,&self.shell.view) else{return};
        if !self.generated.saves(){self.message="Couldn't reopen your last project · your saved projects couldn't be loaded this time · quit and reopen Shibubu".into();return;}
        self.go(View::Project{project,page:String::new()});
        if self.shell.view==View::Home{self.message=format!("Couldn't reopen your last project · {}",self.message);}
    }
    /// W13: 裏の detect の答えを受け取る（毎 frame・待たない）。確かめの途中に送られた一言は、答えが届くか 30 秒たったら作る
    /// （30 秒で届かなければ model 無しで作り、その理由を 1 行・D-e。反証の直し: 前は UI の thread で 30 秒待ち、待ち切れたら黙って名前の表で作った）。
    /// G99: AI を裏で探す（起動・`Check again`・窓が前に来た時・`sign_in` の後）。`sign_in` なら先に `claude auth login` を起こし、終わってから探す。
    /// 探している間は llm_rx が Some（2 本目は起こさない）。W13: 在りか → --version → auth status は数秒かかる（混雑の下で auth status が 4 秒）ので裏で。model は呼ばない
    /// RQ2: 探すのは使っている account（`active_account`）・`sign_in` もその account に（`llm_cli::reauthenticate` = plan の account はその dir・System default は利用者の login）
    fn start_llm_detect(&mut self,ctx:&egui::Context,sign_in:bool){
        if self.llm_rx.is_some(){return;}
        let (llm_tx,llm_rx)=channel();
        let (detect_dir,account,repaint)=(self.data_dir.clone(),self.store.ui.active_account().cloned(),ctx.clone());
        let spawned=thread::Builder::new().name("llm-cli-detect".into()).spawn(move||{
            let signed=if sign_in{crate::llm_cli::reauthenticate(&detect_dir,account.as_ref(),crate::llm_cli::sign_in)}else{Ok(())};
            let cfg=crate::llm_cli::CliConfig{account,..crate::llm_cli::CliConfig::new(&detect_dir)};
            let found=crate::llm_cli::detect_account(cfg);
            // sign in が終わらず、探しても使えない時は sign in の 1 行（押した結果・隣に Sign in をもう一度）
            let found=match (found,signed){(Err(_),Err(why))=>Err(crate::llm::LlmError::NoCli{detail:why}),(found,_)=>found};
            let _=llm_tx.send(found);repaint.request_repaint();
        });
        // L4: 同じ時に Mac の中の AI も探す（login 済みの CLI と動いている local LLM・どちらも model を呼ばない）。
        // 試験の build は探さない（この Mac の grok と :11434 を毎回の `Win::open` で叩かない・試験は `found_rx` に直に渡す・mirror_view と同じ cfg!(test) の口）。
        // 探さなかった run（`probe_found` = None）は何も送らない = poll_found は列の Found にも使っている AI にも触らない（空の答えで消さない）
        let (ftx,frx)=channel();let (fdir,frepaint)=(self.data_dir.clone(),ctx.clone());
        if !cfg!(test)&&thread::Builder::new().name("ai-found-probe".into()).spawn(move||{if let Some(got)=crate::llm_cli::probe_found(&fdir){let _=ftx.send(got);frepaint.request_repaint();}}).is_ok(){self.found_rx=Some(frx);}
        self.llm_checked=Instant::now();
        if spawned.is_err(){self.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"Couldn't start the verification thread".into()}));}else{self.llm_rx=Some(llm_rx);}
    }
    /// RQ-ACCT: browser の sign in の途中の 1 行（Accounts は Add account / Re-authenticate を出さずにこの 1 行）か、AI の確かめ直しの途中の 1 行。
    fn signing_line(&self)->Option<&'static str>{
        self.ai_sign_in.as_ref().map(|_|"Finish signing in in your browser").or(self.llm_rechecking.filter(|_|self.llm_rx.is_some()))
    }
    /// RQ-ACCT × ACCOUNTS-1101: Accounts の行の印 = (足した Claude の plan の (id, email)〈画面だけ・設定に書かない〉, 足した CLI の account のうち login がまだの id)。
    /// file を読むだけ（Accounts を描いている間・毎 frame・account は数個）。
    fn account_marks(&self)->(Vec<(String,String)>,Vec<String>){
        let accts=&self.store.ui.ai_accounts;
        // ACCOUNTS-1101 2 段目: email は frame の頭で読んだ物（5 秒に 1 回・`account_emails`）
        (self.account_emails.clone(),
         accts.iter().filter(|a|crate::llm_cli::cli_account_signed_in(&self.data_dir,a)==Some(false)).map(|a|a.id().to_owned()).collect())
    }
    /// ACCOUNTS-1101 2 段目（作る物 3「選んだ account は Project ごとに覚える」）: その Project で選んだ account に戻す（`Generated::project_account`）。
    /// 覚えていない・列から消した account・sign in がまだの account・今と同じ・Claude Code の無い Mac の System default は何もしない
    /// （開くたびに確かめ直さない・開いた所で sign in の terminal を開かない）。
    fn restore_project_account(&mut self,ctx:&egui::Context,project:&str){
        let Some(want)=self.generated.project_account(project) else{return};
        let acct=match want{
            None=>None,
            Some((kind,id))=>match self.store.ui.ai_accounts.iter().find(|a|a.kind()==kind&&a.id()==id){Some(a)=>Some(a.clone()),None=>return},
        };
        if self.store.ui.active_account()==acct.as_ref()||(acct.is_none()&&!self.claude_here){return}
        if matches!(acct,Some(AiAccount::Found{needs_sign_in:true,..}))||acct.as_ref().is_some_and(|a|crate::llm_cli::cli_account_signed_in(&self.data_dir,a)==Some(false)){return}
        self.switch_ai(ctx,acct);
    }
    /// RQ-ACCT: Add account（`account` = None）/ Re-authenticate（その account）を裏の thread で（browser の login を最大 5 分待つ・答えは `poll_account_sign_in`）。
    /// 同時に 1 本・AI の確かめ（Home の Sign in を含む）の途中は起こさない（同じ login の dir に 2 本の sign in を走らせない）・勝手に Active にしない。
    fn start_account_sign_in(&mut self,ctx:&egui::Context,account:Option<AiAccount>){
        if self.ai_sign_in.is_some(){self.message="Finish the sign-in open in your browser first".into();return;}
        if self.llm_rx.is_some(){self.message="Still checking the AI · try again in a moment".into();return;}
        let (tx,rx)=channel();
        let (dir,existing,repaint,id)=(self.data_dir.clone(),self.store.ui.ai_accounts.clone(),ctx.clone(),account.as_ref().map(|a|a.id().to_owned()));
        let stop=Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.sign_in_stop=stop.clone();
        let spawned=thread::Builder::new().name("ai-account-sign-in".into()).spawn(move||{
            let sign_in=|env:&[(&str,Option<String>)]|crate::llm_cli::sign_in_until(env,&stop);
            let got=match account{
                None=>crate::llm_cli::add_plan_account(&dir,&existing,sign_in),
                Some(a)=>crate::llm_cli::reauthenticate(&dir,Some(&a),sign_in).map(|_|a),
            };
            let _=tx.send(got);repaint.request_repaint();
        });
        match spawned{
            Ok(_)=>{self.ai_sign_in=Some((id,rx));self.message="Finish signing in in your browser".into();}
            Err(_)=>self.message="Couldn't start the sign-in · Try again".into(),
        }
    }
    /// RQ-ACCT: Add / Re-authenticate の答え（毎 frame・待たない）。足した account は列の最後へ（選ぶのは利用者の `Use`）・し直した account を使っていれば確かめ直す。
    /// 失敗は理由の 1 行だけ（列・使っている account・前の login に触らない）。
    fn poll_account_sign_in(&mut self,ctx:&egui::Context){
        let got=match self.ai_sign_in.as_ref().map(|(_,rx)|rx.try_recv()){
            Some(Ok(got))=>got,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected))=>Err(crate::llm_cli::SIGN_IN_FAILED.into()),
            _=>return,
        };
        let Some((reauth,_))=self.ai_sign_in.take() else{return};
        match (got,reauth){
            (Err(why),_)=>self.message=why,
            (Ok(acct),None)=>{self.store.ui.ai_accounts.push(acct);self.mark();self.message="Account added · press Use to switch to it".into();}
            (Ok(acct),Some(_))=>{
                self.message="Signed in again".into();
                if self.store.ui.active_account()==Some(&acct){self.llm_rechecking=Some("Checking for AI on this Mac");self.start_llm_detect(ctx,false);}
            }
        }
    }
    /// RQ2（RQ-ACCT）: 次の呼び出しから使う account。裏で確かめ直し、通らなければ前の account に戻す（poll_llm）。
    /// SIGNIN-1101: sign in の tab の後もここ（sign in の印の行は今使っていても確かめ直す = 通れば poll_llm が印を外す）。
    fn switch_ai(&mut self,ctx:&egui::Context,acct:Option<AiAccount>){
        // ACCOUNTS-1101: 足した CLI の account も sign in の後は使っていても確かめ直す（印は無く login の file で決まる）
        let after_sign_in=matches!(acct,Some(AiAccount::Found{needs_sign_in:true,..}))||acct.as_ref().is_some_and(|a|crate::llm_cli::cli_account_dir(&self.data_dir,a).is_some());
        if self.store.ui.active_account()==acct.as_ref()&&!after_sign_in{return;}
        if self.llm_rx.is_some(){self.message="Still checking the AI · try again in a moment".into();return;}
        let before=self.store.ui.set_active_account(acct.as_ref());self.mark();
        self.start_llm_detect(ctx,false);
        if self.llm_rx.is_some(){
            self.llm_switch=Some(before);self.message="Checking the AI account".into();
            // ACCOUNTS-1101 2 段目: Project の中の切り替え（chat bar の Provider）は通ったらその Project に覚える
            self.llm_switch_for=match &self.shell.view{View::Project{project,..}=>Some(project.clone()),_=>None};
        }
        else{(self.store.ui.ai_active,self.store.ui.ai_active_kind)=before;self.message="Couldn't start the verification thread".into();}
    }
    /// SIGNIN-1101（owner 193「認証できる画面まで自動で飛ばすように」）: 「sign in first」の AI を選んだ = 確かめる前に、下の panel の新しい tab でその CLI の sign in の
    /// command（`llm_cli::FOUND_CLIS`）を打って前に出す（`PanelAct::SignIn`・browser で sign in する CLI はその CLI が browser を開く）。
    /// HOME-TERM-1101（owner 208）: panel = Project の中ならその Project の物・外（Home・Runs・Connections・Settings）なら Home の Terminal（`generated::HOME_PLACE`）。
    /// 画面は移らない・他の Project を開かない・他の Project の panel に打たない。同じ AI の sign in の tab が同じ panel に在れば足さずにそれを前へ。
    /// 終わり（前面の command が shell に戻った）は `poll_cli_sign_in`。install はしない（入っている CLI だけ）・API key は聞かない・model は呼ばない。
    /// ACCOUNTS-1101: 足した CLI の account（`Plan { id: "grok-2" }`）も同じ道。打つ 1 行は `llm_cli::account_sign_in_line`（置き場の変数を `env` で前置き）。
    fn start_cli_sign_in(&mut self,ctx:&egui::Context,acct:AiAccount){
        let name=match &acct{AiAccount::Found{name,..}=>name.clone(),a=>account_label(Some(a),&self.store.ui,&self.account_emails)};
        let Some(command)=crate::llm_cli::account_sign_in_line(&self.data_dir,&acct) else{self.message=format!("Sign in to {name} from its own app · Shibubu doesn't know its sign-in command");return};
        let place=match &self.shell.view{View::Project{project,..}=>project.clone(),_=>crate::generated::HOME_PLACE.to_owned()};
        // 別の置き場で同じ AI の sign in が走っていれば止める（同じ login の置き場に 2 本が同時に書かない・見張りの無い tab を残さない）
        if let Some(s)=self.cli_sign_in.take_if(|s|s.acct==acct&&s.project!=place){let _=self.generated.panel_act(&s.project,PanelAct::ConfirmClose{tab:s.tab});}
        let again=self.cli_sign_in.as_ref().filter(|s|s.acct==acct&&s.project==place).map(|s|s.tab.clone());
        let act=match &again{Some(t)=>PanelAct::Select(t.clone()),None=>PanelAct::SignIn{name:name.clone(),command}};
        if let Err(e)=self.generated.panel_act(&place,act){self.message=e;return;}
        if again.is_none(){
            let Some(tab)=self.generated.panel(&place).active_tab().map(|t|t.id.clone()) else{return};
            let now=Instant::now();
            self.cli_sign_in=Some(CliSignIn{acct,name:name.clone(),project:place,tab,saw_running:false,started:now,probed:now});
        }
        self.message=format!("Signing in to {name}…");
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    /// SIGNIN-1101: 下の tab の sign in の見張り（毎 frame・前面を読むのは 1 秒に 1 回）。前面に sign in の command を見た後で shell に戻った（= command が終わった）か、
    /// 20 秒たっても前面に何も来ない（すぐ終わった）→ 今の切り替えの確かめ（`switch_ai`・通れば印を外して `AI account switched`・通らなければ前の AI のまま次の一手の 1 行・tab は残す）。
    /// tab を閉じた・shell が終わった → 見張りをやめて理由の 1 行。AI の確かめが走っている間は待つ（2 本目を起こさない）。
    // ponytail: 終わりは前面の遷移で見る（sign in の command の exit code は見ない・通ったかは確かめが決める）
    fn poll_cli_sign_in(&mut self,ctx:&egui::Context){
        let Some(s)=self.cli_sign_in.as_mut() else{return};
        ctx.request_repaint_after(Duration::from_secs(1));
        if s.probed.elapsed()<Duration::from_secs(1){return;}
        s.probed=Instant::now();
        let (project,tab,name)=(s.project.clone(),s.tab.clone(),s.name.clone());
        let probe=self.generated.probe_tab(&project,&tab);
        let gone=match &probe{None=>Some("its terminal was closed"),Some(p) if p.exited=>Some("its terminal ended"),Some(_)=>None};
        if let Some(why)=gone{
            self.cli_sign_in=None;
            self.message=format!("Sign-in to {name} stopped · {why}");
            return;
        }
        let running=probe.is_some_and(|p|p.foreground.is_some());
        let Some(s)=self.cli_sign_in.as_mut() else{return};
        s.saw_running|=running;
        let done=!running&&(s.saw_running||s.started.elapsed()>=Duration::from_secs(20));
        if !done||self.llm_rx.is_some(){return;}
        if let Some(s)=self.cli_sign_in.take(){self.switch_ai(ctx,Some(s.acct));}
    }
    /// G99c・PROJECT-DIR-1101: folder を選ぶ窓を裏の thread で開く（UI を止めない・開いている間は 2 つ目を開かない）。答えは `poll_folder` が `project`（None = Home の会話）へ。
    fn ask_folder(&mut self,ctx:&egui::Context,project:Option<String>){
        if self.folder_rx.is_some(){return;}
        let (tx,rx)=channel();let repaint=ctx.clone();
        if thread::Builder::new().name("choose-folder".into()).spawn(move||{let _=tx.send(choose_folder());repaint.request_repaint();}).is_ok(){self.folder_rx=Some((project,rx));}
    }
    /// G99c: 選ぶ窓の答え（押した Project に・取り消しは何もしない）と、窓に落とした folder（今の Project に「No folder」の行が在る時だけ）を
    /// `PageIntent::SetWorkspace` で当てる。Home の chat bar で選んだ folder は Home の会話の folder（`workspace`・CHAT-ONE-1101）。毎 frame・待たない。
    fn poll_folder(&mut self,ctx:&egui::Context){
        let picked=match self.folder_rx.as_ref().map(|(_,rx)|rx.try_recv()){
            Some(Ok(dir))=>self.folder_rx.take().and_then(|(project,_)|dir.map(|d|(project,d))),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected))=>{self.folder_rx=None;None}
            _=>None,
        };
        let dropped=ctx.input(|i|i.raw.dropped_files.iter().filter_map(|f|f.path.clone()).find(|p|p.is_dir()));
        let dropped=match (&self.shell.view,dropped){
            (View::Project{project,..},Some(dir)) if self.generated.wants_folder(project)=>Some((Some(project.clone()),dir.display().to_string())),
            _=>None,
        };
        let Some((project,dir))=picked.or(dropped) else{return};
        let Some(project)=project else{self.workspace=Some(PathBuf::from(dir));return};
        let set=PageIntent::SetWorkspace{dir};
        match self.generated.page_intent(&project,set){
            Ok(v)=>if matches!(&self.shell.view,View::Project{project:now,..} if *now==project){self.shell.go(v);},
            // PROJECT-DIR-1101: Home の `•••` の Change folder… は Page の 1 行が見えない = status の行に理由
            Err(why)=>self.message=why,
        }
    }
    /// 段 2 作り直し → PROVIDER-1101: file を見るだけ（起こさない）: Install CLI & skill の一覧の「見つかった」と、Claude Code の行を出すか（`claude_here`・
    /// `CLI_ENV` の path を置いた run はその CLI が Claude Code・空 = AI を切った run は無い）。「Found … — not supported yet」の 1 行と `Shell::other_clis` は無くした。
    fn refresh_agent_setup(&mut self){
        let home=std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let path=crate::llm_cli::known_path();
        self.claude_here=match env::var_os(crate::llm_cli::CLI_ENV){Some(v)=>!v.is_empty(),None=>!crate::llm_cli::clis_on_mac(&["claude"],&path,&home).is_empty()};
        if let Some(setup)=self.agent_setup.as_mut(){
            let names=crate::terminal::SKILL_AGENTS.map(|(n,_)|n);
            setup.set_found(&crate::llm_cli::clis_on_mac(&names,&path,&home));
        }
    }
    /// L4: 見つかった AI の答え（毎 frame・待たない）。列の Found の行を今の答えに揃え、使っていた物が消えていれば Claude Code（None）に戻す。
    fn poll_found(&mut self,ctx:&egui::Context){
        let Some(got)=self.found_rx.as_ref().map(|rx|rx.try_recv()) else{return};
        let rows=match got{Ok(g)=>g,Err(std::sync::mpsc::TryRecvError::Empty)=>return,Err(_)=>{self.found_rx=None;return}};
        self.found_rx=None;
        if self.store.ui.set_found(rows){
            self.mark();
            if self.store.ui.ai_active.is_some()&&self.store.ui.active_account().is_none(){
                self.store.ui.set_active_account(None);self.llm_switch=None;
                self.message="That AI isn't on this Mac any more · using Claude Code".into();
                self.start_llm_detect(ctx,false);
            }
        }
        self.refresh_agent_setup();
    }
    fn poll_llm(&mut self){
        if let Some(rx)=&self.llm_rx{
            match rx.try_recv(){
                // RQ2: 替えた account の確かめが通らなければ、前の account と前の model のまま（理由の 1 行）
                Ok(Err(e)) if self.llm_switch.is_some()=>{
                    let tried=self.store.ui.active_account().cloned();
                    (self.store.ui.ai_active,self.store.ui.ai_active_kind)=self.llm_switch.take().unwrap_or_default();self.mark();self.llm_rx=None;self.llm_rechecking=None;
                    self.llm_switch_for=None;
                    // SIGNIN-1101: sign in の tab の後でまだ通らない = その tab は残っている（理由は tab の中・ここは次の一手だけ）
                    self.message=match tried{
                        Some(AiAccount::Found{name,needs_sign_in:true,..})=>format!("{name} isn't signed in yet · finish in its terminal below, then pick {name} again"),
                        _=>format!("Couldn't switch the AI account · {e}"),
                    };
                }
                Ok(found)=>{
                    if self.llm_switch.take().is_some(){
                        self.message="AI account switched · used from the next request".into();
                        // ACCOUNTS-1101 2 段目: Project の中で切り替えた = その Project に覚える（書けなくても切り替えは止めない・log だけ）
                        if let Some(p)=self.llm_switch_for.take(){
                            if let Err(e)=self.generated.set_project_account(&p,self.store.ui.active_account()){eprintln!("ACCOUNTS: {p}: account を覚えられません: {e}");}
                        }
                        // PROVIDER-1101: sign in の印の行で通った（Terminal で sign in した後）= その行の印を外す（次に探すまで待たない）
                        if let Some(AiAccount::Found{id,name,needs_sign_in:true})=self.store.ui.active_account().cloned(){
                            if let Some(a)=self.store.ui.ai_accounts.iter_mut().find(|a|matches!(a,AiAccount::Found{needs_sign_in:true,..})&&a.id()==id){
                                *a=AiAccount::Found{id,name,needs_sign_in:false};self.mark();
                            }
                        }
                    }
                    // G84: file を見るだけ（起こさない・PATH は確かめの thread が取った login の PATH）
                    self.refresh_agent_setup();
                    let home=std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                    self.generated.set_agents(crate::llm_cli::agent_clis(&crate::llm_cli::known_path(),&home));
                    self.generated.set_llm(found);self.llm_rx=None;self.llm_rechecking=None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected)=>{self.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"Verification stopped partway".into()}));self.llm_rx=None;self.llm_rechecking=None;}
                Err(std::sync::mpsc::TryRecvError::Empty)=>{}
            }
        }
        let Some((text,attach,since))=self.waiting.take() else{return};
        if self.llm_rx.is_none(){self.create_app(text,attach);}
        else if since.elapsed()>=Duration::from_secs(30){self.generated.mark_llm_unchecked();self.create_now(text);self.say_unused(&attach);}
        else{self.waiting=Some((text,attach,since));}
    }
    /// W13: 裏の model の答えが届いていれば開く（毎 frame・待たない）。Project を保存する時だけ bridge を起こす。
    fn poll_model(&mut self,ctx:&egui::Context){
        self.generated.poll_engine(); // W19-2 の配線: Engine の答え（Page の 1 行と rail の実行履歴は Generated が持つ）
        self.generated.poll_asks(); // L7-AGENT: Terminal の agent が `shibubu project ask` で頼んだ一言を承認の card に
        self.generated.poll_installs(); // PV0: 無い app の導入の答え（読み戻した後に 1 問を面に替える）
        let Some(done)=self.generated.poll_model_job(spawn_bridge) else{return};
        // FX2 反証の直し: Runs の生成の行で Stop を押した時の note（`Stopping the AI…`）を、行が Cancelled になったら終わった文に替える
        // （Home の Stop の後も置くが、note は開いたままの行にだけ出て、行を押して開くと消える）
        let stopped=match &done{crate::generated::ModelDone::Stopped{..}=>true,crate::generated::ModelDone::Previewed{lines,..}=>lines.is_empty(),_=>false};
        if let (true,Some(run))=(stopped,self.generated.model_run_id()){ui::runs::note(ctx,run,crate::generated::STOPPED_LINE);}
        match done{
            crate::generated::ModelDone::Created{goal,made}=>self.on_created(goal,made),
            // 読んでいる間に別の画面へ移っていたら、そのまま（Page の上の 1 行は Project に戻った時に出る）
            crate::generated::ModelDone::Edited{project,made}=>if let Ok(v)=made{
                if matches!(&self.shell.view,View::Project{project:now,..} if *now==project){self.shell.go(v);}
            },
            // FX2（FX-005）: 止めた生成は何も作らず、送った一言を Home の欄へ戻す（1 行は Generated::model_progress の Ended）
            crate::generated::ModelDone::Stopped{goal}=>self.back_to_home_field(goal),
            // FX2（FX-006）: AI の失敗も何も作らず一言を欄へ・Home で理由の 1 行と次の一手（`open_made` の Err と同じく Home へ・note には出さない）
            crate::generated::ModelDone::Failed{goal}=>{self.back_to_home_field(goal);self.go(View::Home);}
            // L2: 案の読みの答えは、その案の card の読むだけの行へ（読む間に案が替わった・閉じた = 捨てる）
            crate::generated::ModelDone::Previewed{goal,lines,sections}=>if crate::domain::chat::pending_proposal(&self.shell.talk)==Some(goal.as_str()){
                if let Some(t)=self.shell.talk.iter_mut().rev().find(|t|t.who==crate::domain::chat::Who::Shibubu){(t.preview,t.sections)=(lines,sections);}
            },
            // OPEN-INPLACE-1101 の 5: Project の chat の案の Preview（行は Generated が案の card に置いた）
            crate::generated::ModelDone::Planned=>{},
        }
    }
    fn mark(&mut self){self.dirty=true;}
    /// RQ3: Settings › Data & Recovery の中身を読み直す（`force` = action の後・30 秒の間引きを越える）
    fn refresh_data(&mut self,force:bool){self.generated.refresh_data(&self.user_dirs,crate::google::now() as i64,force);}
    /// UI-4b: 場所ごとの打ちかけ（`Shell::drafts` と今の場所の欄）を `UiSettings::drafts` へ写す。保存の直前と `on_exit` だけ（打った frame ごとには mark しない・
    /// 場所を替えた時と変更が通った時に mark する）。空の文は持たない。
    /// 契約（FX4）: ⌘K の最近の行も同じ時に写す（`self.store.ui.recent_commands = self.shell.palette.recent.clone()`）。読み戻しは `open` の
    /// drafts の隣（`app.shell.palette.recent = app.store.ui.recent_commands.clone()`）。
    fn snapshot_drafts(&mut self){
        let mut d=self.shell.drafts.clone();
        // ⌘K を開いている間、欄の文は ⌘K の語なので打ちかけは stash。名前の欄の間は預けた打ちかけを残す（CONNECTIONS-1101: Connections の行の下の欄は上の欄と別 = 保存しない）
        if self.shell.renaming.is_none(){
            d.insert(self.shell.view.place().to_owned(),self.shell.palette.stash.clone().unwrap_or_else(||self.shell.command.clone()));
        }
        // 探す欄（"search"）の語は再起動を跨がない（保存する key は Home と Project の id だけ = `UiSettings::drafts` の doc）
        // PASSWORD-1101 の 3: password らしい打ちかけは書かない（送れば `hold_password` が止める物 = 送らずに終えた物も同じ）
        d.retain(|k,v|k!="search"&&!v.trim().is_empty()&&!crate::domain::chat::looks_like_password(v));
        self.store.ui.drafts=d;
        self.store.ui.recent_commands=self.shell.palette.recent.clone();
    }
    /// FX1（FX-001）: 終わる前に全部を保存する 1 か所（`Intent::Quit`・`on_exit` が呼ぶ）。中身は今の `on_exit` の段のまま:
    /// 今の Page の入力途中（`Generated::save_view_state` = Page を離れる保存・native_direct ではその Page に重ねた窓を隠す）→ `save_store`。
    /// 例: `match self.save_now() { Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close), Err(why) => { ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose); self.quit_failed = Some(why) } }`
    /// 保証する: どれか 1 つでも書けなければ Err（最初の理由・利用者の語で 1 行・path を出すなら data dir の下の名前だけ）・View・欄の文・Page の選択を変えない。
    /// 保証しない: 書いた file を読み戻して確かめる事（rename が通れば Ok）・Terminal の scrollback（`close_terminals` は終わる時だけ）。
    /// Wave: FX1
    fn save_now(&mut self)->Result<(),String>{
        // 1 つ書けなくても残りは書く（失う物を減らす）・返すのは最初の理由
        let view=match &self.shell.view{View::Project{project,page}=>self.generated.save_view_state(project,page).err(),_=>None};
        let rest=self.save_store();
        view.map_or(rest,Err)
    }
    /// FX1 反証の直し: `save_now` から Page を離れる保存を除いた残り = `snapshot_drafts` → `Store::save(sessions.json)`、そして
    /// `Generated::store_lost`（kangae.sqlite3 を開けず memory で動いている）なら Err。Ok なら `dirty = false`。
    /// 時間の保存（`save_if_due`・debounce ごと）はこちらだけ（Page に居るまま窓を隠さない = generated.rs の「移る時だけ隠す」）。
    /// Wave: FX1
    fn save_store(&mut self)->Result<(),String>{
        let mut first:Option<String>=None;
        self.snapshot_drafts();
        match self.store.save(&self.data_path){Ok(())=>self.dirty=false,Err(e)=>{first.get_or_insert(e);}}
        self.last_save=Instant::now();
        if let Some(why)=self.generated.store_lost(){first.get_or_insert_with(||why.to_owned());}
        first.map_or(Ok(()),Err)
    }
    fn save_if_due(&mut self){if self.dirty&&self.last_save.elapsed()>SAVE_DEBOUNCE{
        if let Err(e)=self.save_store(){self.message=format!("Couldn't save your changes · {e}");}
    }}
    /// 次の frame を頼むまでの時間（W2b-AX 直し・契約は src/main.rs）。app が自分で持つ「時間で替わる物」は保存待ちの debounce の残りだけ。
    /// それ以外は None = 次の入力・背景の thread の結果（`Tx::send`）・animation（motion / skeleton / egui の cursor の点滅・
    /// 動いている timer〈renderer〉・「N 分前」〈runs〉は描く側が自分で頼む）が来るまで描かない（背景でも前面でも）。
    /// 例外: Page に WebView / Native Direct の面が在る間は `SURFACE_POLL`（どちらも毎 frame の poll で進む: WKWebView の isLoading・WebLife の時計・bridge の event と枠の追従）。
    fn next_repaint(&self)->Option<Duration>{
        let save=self.dirty.then(||SAVE_DEBOUNCE.saturating_sub(self.last_save.elapsed()));
        // ponytail: 面の poll は 230ms の pump に戻す。WKWebView の navigation と bridge の event で egui を起こせば面の在る Page も 0 にできる
        let poll=self.generated.polls_surfaces().then_some(SURFACE_POLL);
        save.into_iter().chain(poll).min()
    }
    fn poll(&mut self){
        while let Ok(e)=self.rx.try_recv(){
            match e {
                Event::LaneStarted{session}=>{*self.running.entry(session).or_insert(0)+=1;}
                Event::LaneFinished{session}=>{
                    if let Some(count)=self.running.get_mut(&session){*count=count.saturating_sub(1);}
                }
                Event::Result{session,action,title,result}=>{
                    let (out, display)=match result{
                        Ok(v)=>(result_line(&title,&v),Ok(v)),
                        Err(e)=>{let message=format!("{title} failed: {e}");(message.clone(),Err(message))}
                    };
                    self.results.insert(action,display);
                    if let Some(s)=self.store.sessions.iter_mut().find(|s|s.id==session){
                        s.history.insert(0,out.clone());s.history.truncate(50);
                    }
                    self.message=out;self.mark();
                }
                Event::Windows{session,result}=>{
                    if session==self.store.selected {match result {Ok(w)=>{
                        self.windows=w;self.selected_window=0;self.controls.clear();
                        self.message=format!("{} windows",self.windows.len());
                    },Err(e)=>self.message=e}}
                }
                Event::Controls{session,pid,window_id,result}=>{
                    let correct_target=self.windows.get(self.selected_window)
                        .map(|w|w.pid==pid&&w.window_id==window_id).unwrap_or(false);
                    if session==self.store.selected && correct_target {match result {Ok(c)=>{
                        self.controls=c;self.selected_control=0;
                        self.message=format!("{} controls found",self.controls.len());
                    },Err(e)=>self.message=e}}
                }
                Event::Inspect(r)=>match r{Ok(i)=>{self.review=Some(i);self.license_accepted=false;self.message="Check the licence, subfolder and commit".into()},Err(e)=>self.message=format!("Couldn't check: {e}")},
                Event::Imported{session,result}=>match result{Ok(f)=>{
                    self.message=format!("Imported {} · {}. No code was run",f.repo,f.subdir);
                    self.store.features.push(f);
                    if let Some(s)=self.store.sessions.iter_mut().find(|s|s.id==session){
                        if !s.panels.iter().any(|p|p.kind==PanelKind::Feature){s.panels.push(Panel::new(PanelKind::Feature,"Repo feature",1,1));}
                    }
                    self.mark()
                },Err(e)=>self.message=format!("Couldn't import: {e}")},
                Event::GuiStatus(tier,detail)=>{self.gui_checking=false;self.gui_status=Some((tier,detail));self.gui_checked=Some(Instant::now());}
                Event::GithubStatus(tier,detail,at)=>{self.github_checking=false;self.github_status=Some((tier,detail,at,Instant::now()));}
                Event::Checked{key,tier,detail}=>{
                    self.conn_checking.remove(&key);self.conn_at.insert(key.clone(),crate::google::now() as i64);
                    self.conn_result.insert(key,(tier,detail));
                }
                Event::WebTested{url,result}=>self.web_tested(url,result),
                Event::Vault{m,result}=>self.vault_connected(m,result),
                Event::VaultItems(items)=>if self.shell.connecting.as_deref().and_then(crate::ui::connections::secret_of).is_some_and(|(row,_)|crate::ui::connections::takes_items(row)){
                    self.shell.vault_items.extend(items);
                    self.shell.vault_items.sort_by_key(|i|i.label().to_lowercase());
                },
                Event::VaultSaved(r)=>match r{
                    Ok(item)=>self.say(crate::domain::chat::Who::Shibubu,format!("Saved in {} as “{}”",item.manager.label(),item.name)),
                    Err((value,m,why))=>{
                        if self.shell.held.is_none(){self.shell.held=Some((value,m));}
                        self.message=why;
                    }
                },
                Event::ProgramChosen(r)=>match r{
                    Ok(Some(path))=>{
                        let id=new_id();
                        self.message=format!("Type the fixed arguments for {} · or press Enter for none",crate::mcp::command_name(&path.display().to_string()));
                        self.cli_draft=Some((id,path));self.shell.start_connecting(format!("cli:{id}"));
                    }
                    Ok(None)=>{}
                    Err(why)=>self.message=why,
                },
            }
        }
    }
    /// L2: Home の一言を会話に足す。model が在れば待つ 1 行（`TALK_WAIT`・案も選択肢も無い）を置いて裏で 1 回呼び、着いたらその行だけを差し替える。
    /// model が無い・もう 1 本読んでいる・呼べなかった時は `local_talk` の問いと選択肢（同時に 1 本 = 利用者の quota を 2 重に使わない）。
    /// DOGFOOD-X 周 7 #1: AI が上限で止まっていれば呼ばず（thread も起こさない）、止まった事と次の一手を言う（`stopped_talk`）。
    /// HOME-ATTACH-1101（owner 185 通目）: 添えた物を今の AI が読めない（AI 無しも）・選んだ後に消えた時は送らずに、文を欄へ戻して chat bar の中に理由 1 行（`chat::recheck`）。
    fn talk(&mut self,text:String){
        use crate::domain::chat::{talk_history,talk_schema,Mark,Turn,Who,TALK_INSTRUCTIONS,TALK_WAIT};
        if let View::Project{project,..}=self.shell.view.clone(){return self.talk_in(project,text);}
        if let Err(line)=crate::domain::chat::recheck(&self.shell.attach,&self.generated.accepts(),&self.ai_name()){
            self.back_to_home_field(text);(self.shell.attach_line,self.shell.focus_command)=(Some(line),true);return;
        }
        let now=crate::runtime::now_rfc3339();
        let names:Vec<String>=self.shell.attach.iter().map(|a|a.name()).collect();
        let said=Turn{who:Who::You,text:text.clone(),mark:Mark::None,at:now.clone(),attachments:names,choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None};
        self.keep_home_turn(&said);
        self.shell.talk.push(said);
        self.shell.discovery=None;
        self.generated.new_request(); // TALK-1101 追記 2: 人の一言 = 1 つの頼み（上限を数え直してから止まっているかを見る）
        let stopped=self.generated.llm_stopped();
        let llm=if self.talk_rx.is_none()&&stopped.is_none(){self.generated.fork_llm()}else{None};
        let (tx,rx)=channel();
        // B61-REPO（録画の手 1）: 決まっている repo（SHIBUBU_WORKSPACE_DIR）は folder の名前だけ渡す = 「Which repo?」で案を止めない。無ければ今のまま聞く
        let repo=self.workspace.as_deref().and_then(|p|p.file_name()).map(|n|n.to_string_lossy().into_owned());
        // TALK-HONEST-1101: 会話の頭に「この Mac で今出来る事」（Engine が見る手段と同じ・確かめたかつき）。Engine が無ければ前と同じ
        let able=if llm.is_some(){self.generated.abilities()}else{String::new()};
        let history=[able,talk_history(&self.shell.talk,repo.as_deref())].into_iter().filter(|s|!s.is_empty()).collect::<Vec<_>>().join("\n");
        let asked=llm.is_some_and(|llm|thread::Builder::new().name("talk".into()).spawn(move||{
            let _=tx.send(crate::generated::TalkMsg::Reply(llm.complete_json(crate::llm::JsonRequest::new("talk",TALK_INSTRUCTIONS,&history,talk_schema()))));
        }).is_ok());
        let (ask,choices,proposal)=match (asked,stopped){(true,_)=>(TALK_WAIT.to_owned(),Vec::new(),None),(false,why)=>self.no_ai_reply(why.as_deref(),&text)};
        let reply=Turn{who:Who::Shibubu,text:ask,mark:Mark::None,at:now,attachments:Vec::new(),choices,proposal,preview:Vec::new(),sections:Vec::new(),needs:None};
        self.keep_home_turn(&reply);
        self.shell.talk.push(reply);
        if asked{self.talk_rx=Some((None,self.shell.talk.len()-1,rx));}
    }
    /// PASSWORD-1101: 今の会話に 1 行足す（AI は呼ばない）。Project = その Project の会話・他 = Home の会話。
    fn say(&mut self,who:crate::domain::chat::Who,text:String){
        use crate::domain::chat::{Mark,Turn};
        let turn=Turn{who,text,mark:Mark::None,at:crate::runtime::now_rfc3339(),attachments:Vec::new(),choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None};
        if let View::Project{project,..}=self.shell.view.clone(){return self.generated.say_turn(&project,turn);}
        self.keep_home_turn(&turn);
        self.shell.talk.push(turn);
    }
    /// PASSWORD-1101: password manager の card を見える所へ（Project = 下の panel の Chat の tab・Home = 会話の下・他の場所 = Home へ）
    fn reveal_card(&mut self){
        match self.shell.view.clone(){
            View::Project{project,..}=>{let _=self.show_chat(&project);}
            View::Home=>{}
            _=>self.go(View::Home),
        }
    }
    /// PASSWORD-1101 の 3（owner 252）: password らしい一言（`chat::looks_like_password`）は AI に送らず・会話に足さず・保存しない。値は `Shell::held` の
    /// `Secret` だけに置き、card（`Card::password`）が保存先（繋いでいる 1Password・無ければ Keychain）か消すかを聞く。欄の文は `enter_intent` が消している。
    fn hold_password(&mut self,text:String){
        use crate::domain::vault::{Manager,Secret};
        let m=if self.store.ui.vaults.contains(&Manager::OnePassword)&&Manager::OnePassword.find().is_some(){Manager::OnePassword}else{Manager::Keychain};
        (self.shell.held,self.shell.vault_ask)=(Some((Secret::new(text),m)),None);
        self.reveal_card();
    }
    /// SESSIONS-LEFT-1101: Home の会話の 1 行を保存する（左の Sessions と再起動の後のため）。最初の 1 行で session の key（`HOME_CHAT` + id）を作る・
    /// 待つ 1 行（`TALK_WAIT`）は保存しない（答えが着いた時に `poll_talk` が答えの行を保存する）。
    fn keep_home_turn(&mut self,turn:&crate::domain::chat::Turn){
        if turn.text==crate::domain::chat::TALK_WAIT{return}
        let key=self.shell.talk_id.get_or_insert_with(||format!("{}{}",crate::domain::chat::HOME_CHAT,new_id())).clone();
        self.generated.say_in(&key,turn.clone());
    }
    /// SESSIONS-LEFT-1101: 左の session の行を開く。Home の session = 今の Home の会話を閉じて（読んでいる答えは捨てる = `new_chat`）その会話を `Shell::talk` に読み、Home へ・
    /// Project の session = その session を今の session にして（`pick_session`）その Project を開き、下の panel の Chat の tab を開いて欄に focus。開けない Project は `go` が理由の 1 行。
    fn open_chat(&mut self,key:String){
        let Some(project)=crate::domain::chat::session_project(&key).map(str::to_owned) else{
            if self.shell.talk_id.as_deref()!=Some(key.as_str()){
                self.new_chat();
                self.shell.talk=self.generated.chat_in(&key);
                self.shell.talk_id=Some(key);
            }
            return self.go(View::Home);
        };
        self.generated.pick_session(&project,&key);
        if !matches!(&self.shell.view,View::Project{project:now,..} if *now==project){
            self.go(View::Project{project:project.clone(),page:String::new()});
            if !matches!(&self.shell.view,View::Project{project:now,..} if *now==project){return}
        }
        match self.show_chat(&project){Ok(_)=>self.shell.focus_command=true,Err(e)=>self.message=e}
    }
    /// CHAT-SIDE-1101: Project の chat を見せる（置き場は替えない）= panel の Chat の tab を選んで開く（追記 219〜221: 下でも右でも同じ panel）。
    fn show_chat(&mut self,project:&str)->Result<(),String>{self.generated.panel_act(project,PanelAct::Chat)}
    /// CHAT-ONE-1101: Project の Chat の一言（Home の `talk` と同じ会話・同じ AI の 1 回）。違いは 3 つ: 一言と答えはその Project の会話に保存（`say_turn`）・
    /// AI に渡す 1 行目は `TALK_PROJECT`（名前と Page の並び）= 案はこの Project への変更の 1 文・待つ 1 行は保存しない（描く所が足す）。
    /// その次は Engine と同じ状況の塊（`Generated::situation` ＋ `with_windows` → `Situation::text`・ENGINE-GENERIC 追記 234）= Page の誤りも Engine の直近の run も知っている。
    /// AI が無い・止まった・もう 1 本読んでいる時は `local_change`（変更の形の文なら案・無ければ何を変えるかの問い・止まった理由は頭に）。
    fn talk_in(&mut self,project:String,text:String){
        use crate::domain::chat::{local_change,talk_history,talk_schema,Mark,Turn,Who,TALK_INSTRUCTIONS,TALK_PROJECT};
        if let Err(line)=crate::domain::chat::recheck(&self.shell.attach,&self.generated.accepts(),&self.ai_name()){
            (self.shell.command,self.shell.attach_line,self.shell.focus_command)=(text,Some(line),true);return;
        }
        let now=crate::runtime::now_rfc3339();
        self.generated.new_request(); // TALK-1101 追記 2: 人の一言 = 1 つの頼み（Home の talk と同じ）
        let stopped=self.generated.llm_stopped();
        let llm=if self.talk_rx.is_none()&&stopped.is_none(){self.generated.fork_llm()}else{None};
        // ENGINE-GENERIC-1101 追記 234 × TALK-HONEST 追記 232: Engine と同じ 1 つの状況（Page・部品の誤り・Engine の今と直近の run・会話の直近・出来る事）を
        // 一言を足す前に（Engine が頼まれた時と同じ時点）。窓の 1 行は thread で Engine と同じ `with_windows`・文は同じ `Situation::text`
        let situation=llm.as_ref().map(|_|self.generated.situation(&project));
        let names:Vec<String>=std::mem::take(&mut self.shell.attach).iter().map(|a|a.name()).collect();
        self.generated.say_turn(&project,Turn{who:Who::You,text:text.clone(),mark:Mark::None,at:now.clone(),attachments:names,choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None});
        let head=self.generated.talk_project(&project).map(|p|format!("{TALK_PROJECT}: {p}"));
        // SESSIONS-LEFT-1101（owner 198 の 4）: 同じ Project の他の session の要点（題と最後の 2 往復）も渡す = 前の session で決めた事を AI が知っている
        let others=self.generated.other_sessions(&project);
        let said=talk_history(&self.generated.chat(&project),self.generated.repo_name(&project).as_deref());
        let engine=self.generated.fork_engine();
        // ENGINE-AUDIT W9 本体（spec の流れ B・dogfood r1 / r6 = app の操作が composer に化けた）: 一言を先に分ける（Engine の interpret = AI 1 回・窓は読まない）。
        // act は一言のまま Engine へ（`follow_route`・同じ一言の run は interpret を呼び直さない）・change / question は会話の AI・候補が 2 つ以上は 1 回だけ聞く。
        // 分けられない（Engine か AI が無い・AI が答えない）時は今まで通り会話の AI だけ
        // B86: 「Which app should Shibubu use for …?」の選択肢を押した一言は、元の頼みにその app を足して分け直す（会話の行は押した名前のまま）
        let goal=crate::domain::chat::answered_ask(&self.generated.chat(&project),&text).unwrap_or_else(||text.clone());
        let classify=situation.clone().zip(engine.clone()).and_then(|(s,_)|self.generated.classify_request(&project,&goal,s));
        let (routed_project,routed_say)=(project.clone(),goal);
        let (tx,rx)=channel();
        let asked=llm.is_some_and(|llm|thread::Builder::new().name("talk".into()).spawn(move||{
            use crate::generated::TalkMsg;
            // 窓の 1 行（開いている app）は Engine と同じ道で 1 回だけ読み、分ける依頼にも会話の AI にも同じ物を渡す（候補は Situation の app から出す決まり）
            let situation=situation.map(|s|crate::generated::with_windows(engine.as_deref(),s));
            let route=classify.and_then(|(e,mut r)|{r.situation=situation.clone().unwrap_or_default();e.classify(r).ok()}).map(|ops|crate::domain::chat::route(&ops));
            let hint=route.as_ref().and_then(|r|r.hint());
            if let Some(r)=route{
                let talk=r.talk;
                let _=tx.send(TalkMsg::Routed{project:routed_project,say:routed_say,route:r});
                if !talk{return}
            }
            let block=situation.and_then(|s|s.text());
            let history=head.into_iter().chain(block).chain(others).chain(hint).chain([said]).collect::<Vec<_>>().join("\n");
            let _=tx.send(TalkMsg::Reply(llm.complete_json(crate::llm::JsonRequest::new("talk",TALK_INSTRUCTIONS,&history,talk_schema()))));
        }).is_ok());
        // SESSIONS-LEFT-1101: 答えは頼んだ session に足す（待つ間に別の session を開いても混ざらない）
        if asked{self.talk_rx=Some((Some(self.generated.session_key(&project)),0,rx));return}
        let (ask,choices,proposal)=local_change(&text,stopped.as_deref());
        self.generated.say_turn(&project,Turn{who:Who::Shibubu,text:ask,mark:Mark::None,at:now,attachments:Vec::new(),choices,proposal,preview:Vec::new(),sections:Vec::new(),needs:None});
    }
    /// L2（111 通目・handoff 05 の Proposal → Preview → Accept）: 案の card の手。Preview plan = 読むだけの行と節を card の中に開く（何も書かない）・
    /// Change plan = 案の文を欄へ（直して Enter で送り直す）・Later = その案を閉じる（会話は残す）・Build now（TALK-1101・前の Accept）= 案の文で今の作る道を 1 回
    /// （Preview を押していなくてよい・答えていない問いは AI が選ぶ = `Generated::start_create`）。
    /// 会話は作れた時に閉じる（`settle_talk`）= 作れなければ card が残り、理由は欄の下の 1 行。New chat は `Intent::NewChat` の腕（`new_chat` → Home）。
    fn plan_step(&mut self,i:Intent){
        use crate::domain::chat::{pending_proposal,Who};
        if let View::Project{project,..}=self.shell.view.clone(){return self.change_step(project,i);}
        let Some(sentence)=pending_proposal(&self.shell.talk).map(str::to_owned) else{return};
        let last=self.shell.talk.iter().rposition(|t|t.who==Who::Shibubu);
        // DOGFOOD-X 周 5 #5: 作る前に答えが要る問いが在る間は読まない・作らない（card の primary も灰・答えると次の答えで決め直す）
        if matches!(i,Intent::PreviewPlan|Intent::AcceptPlan)&&crate::domain::chat::pending_needs(&self.shell.talk).is_some(){return}
        match i{
            Intent::PreviewPlan=>{
                // TALK-1101 追記 2: 押した = 1 つの頼み（読んでいる最中は前の頼みのまま）
                if self.generated.model_busy_line().is_none(){self.generated.new_request();}
                // L2（DOGFOOD-X 周 2 回 1 #2）: AI が在れば作る時と同じ読みを今 1 回（読めた案を Build now が保存する）・無い・読んでいる最中は名前の表か 1 行
                if let Err(line)=crate::domain::chat::recheck(&self.shell.attach,&self.generated.accepts(),&self.ai_name()){self.shell.attach_line=Some(line);return}
                // 周 7 #2: AI が上限で止まっていれば読みを始めない（名前の表の案と止まった事の 1 行）
                let reading=self.generated.llm_ready()&&self.generated.llm_stopped().is_none()&&self.generated.model_busy_line().is_none()&&self.generated.start_preview(&sentence,self.workspace.as_deref(),&self.shell.attach,crate::domain::chat::open_question(&self.shell.talk));
                let lines=if reading{vec![crate::domain::chat::PLAN_READING.to_owned()]}else{self.plan_preview(&sentence)};
                if let Some(t)=last.and_then(|n|self.shell.talk.get_mut(n)){(t.preview,t.sections)=(lines,Vec::new());}
            }
            Intent::ChangePlan=>(self.shell.command,self.shell.focus_command)=(sentence,true),
            Intent::LaterPlan=>{
                if let Some(t)=last.and_then(|n|self.shell.talk.get_mut(n)){(t.proposal,t.preview,t.sections)=(None,Vec::new(),Vec::new());}
                // CHAT-011 反証 Y2: 黙って消さない（提案の Later と同じく何が起きたかの 1 行）・案の文は会話に残す（次の一言で model も読む = 「build it」で戻る）
                use crate::domain::chat::{Mark,Turn,LATER_SAID};
                let later=Turn{who:Who::Shibubu,text:format!("{LATER_SAID} · {sentence}"),mark:Mark::None,at:crate::runtime::now_rfc3339(),attachments:Vec::new(),choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None};
                self.keep_home_turn(&later);
                self.shell.talk.push(later);
            }
            Intent::AcceptPlan=>{let attach=std::mem::take(&mut self.shell.attach);self.create_app(sentence,attach);}
            _=>{}
        }
    }
    /// Home の会話を閉じる（Home の読んでいる答えも捨てる・Project の Chat の読みは残す）。SESSIONS-LEFT-1101: 保存した会話は消さない（左の Sessions に残る）。
    fn new_chat(&mut self){
        self.shell.talk.clear();
        self.shell.talk_id=None;
        if self.talk_rx.as_ref().is_some_and(|r|r.0.is_none()){self.talk_rx=None;}
    }
    /// CHAT-ONE-1101: Project の Chat の案の card の手（Home の `plan_step` と同じ 4 つ・案はこの Project への変更の 1 文）。
    /// Preview plan = 読むだけの行（何も書かない・保存しない）・Change plan = 案の文を欄へ・Later = 何も変えずに閉じた 1 行（案の文は会話に残す）・
    /// Accept = 前の Page の欄と同じ変更の道（`start_edit` → 名前の表の `PageIntent::Edit`）。答え（変えた・止めた・Engine へ）は `finish_edit` が会話に足す
    /// （人の一言はもう会話に在るので言い直さない = `Generated::accepted`）。答えが要る問いが在る間は読まない・変えない。
    fn change_step(&mut self,project:String,i:Intent){
        use crate::domain::chat::{pending_needs,pending_proposal,Mark,Turn,Who,PLAN_UNMADE};
        let chat=self.generated.chat(&project);
        let Some(sentence)=pending_proposal(&chat).map(str::to_owned) else{return};
        if matches!(i,Intent::PreviewPlan|Intent::AcceptPlan)&&pending_needs(&chat).is_some(){return}
        match i{
            // OPEN-INPLACE-1101 の 5: Build now と同じ道で読む（Engine へ行く文は Engine の手の行）・読めない時（名前の表・AI 無し・読みの最中）は今の 2 行
            Intent::PreviewPlan=>if self.generated.preview_engine_plan(&project,&sentence).is_err(){self.generated.preview_change(&project,vec![crate::generated::CHANGE_PREVIEW.into(),PLAN_UNMADE.into()])},
            Intent::ChangePlan=>(self.shell.command,self.shell.focus_command)=(sentence,true),
            Intent::LaterPlan=>{
                let text=format!("Saved for later · nothing changed · {sentence}");
                self.generated.say_turn(&project,Turn{who:Who::Shibubu,text,mark:Mark::None,at:crate::runtime::now_rfc3339(),attachments:Vec::new(),choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None});
            }
            Intent::AcceptPlan=>{
                // OPEN-INPLACE-1101 の 5: Preview の読みの最中なら読み終えてから作る（card に BUILD_QUEUED）・他の読みの最中は今まで通り何もしない
                if self.generated.model_busy_line().is_some(){self.generated.build_when_read(&project,&sentence);return}
                self.generated.new_request(); // TALK-1101 追記 2: Build now = 1 つの頼み（Home の create_app と同じ）
                if self.generated.start_edit(&project,&sentence){return}
                if let Ok(v)=self.generated.page_intent(&project,PageIntent::Edit{text:sentence}){self.shell.go(v);}
            }
            _=>{}
        }
    }
    /// L2: AI の読みを始めない時の `Preview plan` の行（作る前に見せる・何も保存しない）。AI が無ければ名前の表が組む案を AI の時と同じ形
    /// （`generated::plan_lines` = 作る Page と app）で手元で引く。AI が在るのに読みを始められない（CLI を確かめている・別の読みの最中）時は
    /// Build now で AI が読む事だけを言う（Page の名前を作り話しない）。
    /// DOGFOOD-X 周 7 #2: AI が上限で止まっている（`llm_stopped`）時は「AI が読む」と言わず、1 行目に止まった事と次の一手・Build now が名前の表で作る事、
    /// その下に名前の表の案（Build now は `create_app` が AI を呼ばずに `create_simple` へ = 見せた物が出来る）。
    fn plan_preview(&self,sentence:&str)->Vec<String>{
        use crate::domain::chat::{PLAN_UNMADE,SIMPLE_ACCEPT};
        let stopped=self.generated.llm_stopped();
        if stopped.is_none()&&(self.generated.llm_ready()||self.llm_rx.is_some()){
            return vec!["When you press Build now, the AI reads this and picks the pages.".into(),PLAN_UNMADE.into()];
        }
        match (self.simple_plan(sentence),stopped){
            (Some(mut lines),Some(why))=>{lines.insert(0,format!("{why} · {SIMPLE_ACCEPT}"));lines}
            (Some(lines),None)=>lines,
            (None,Some(why))=>vec![why,"Simple rules can't build this · change the plan".into(),PLAN_UNMADE.into()],
            // review A: 変更の形の案も Build now は新しい Project の道（いちばん新しい Project を書き換えない）= AI が無ければ作れない
            (None,None)=>vec![NEEDS_AI.into(),PLAN_UNMADE.into()],
        }
    }
    /// 名前の表（AI を呼ばない）が `sentence` で作る物の読むだけの行（`generated::plan_lines`）。作れない文（変更の文「一覧に検索を追加して」等）は None。
    fn simple_plan(&self,sentence:&str)->Option<Vec<String>>{
        use crate::composer::{compose,project::compose_project};
        match compose_project(sentence,&Default::default(),self.workspace.as_deref()){
            Ok(Some(p))=>Some(crate::generated::plan_lines(sentence,Some(&p))),
            _ if compose(sentence,&Default::default()).is_ok()=>Some(crate::generated::plan_lines(sentence,None)),
            _=>None,
        }
    }
    /// REVIEW-R7 Y-a・B45: AI が読まない時の Home の返事。`why` = AI が上限で止まった 1 行（`stopped_talk`）・None = `local_talk`。
    /// AI が無い（CLI を確かめている途中でもない）か止まっている時、名前の表でも作れない文には案を出さない = 押すと失敗する Build now を
    /// 約束しない・次の一手の 1 行だけ（止まった = Execution limit・AI が無い = `NEEDS_AI`・Kangae ③ ⑤）。AI が在る（読み損ねた・もう 1 本読んでいる）
    /// 時は `local_talk` のまま（Build now は AI が読んで作る）。
    fn no_ai_reply(&self,why:Option<&str>,text:&str)->(String,Vec<String>,Option<String>){
        use crate::domain::chat::{local_talk,stopped_talk};
        let ai=why.is_none()&&(self.generated.llm_ready()||self.llm_rx.is_some());
        let reply=why.map_or_else(||local_talk(text),|w|stopped_talk(w,text));
        if !ai&&reply.2.is_some()&&self.simple_plan(text).is_none(){return (why.unwrap_or(NEEDS_AI).to_owned(),Vec::new(),None)}
        reply
    }
    /// L2: 案で作れた時だけ会話を閉じる（作った物へ移る・Home に戻っても古い card を残さない）。案と違う文（欄から直に作った）では閉じない。
    /// true = 閉じた（会話は作った Project の Chat の頭へ移った）。
    fn settle_talk(&mut self,goal:&str)->bool{
        let moved=crate::domain::chat::pending_proposal(&self.shell.talk)==Some(goal);
        if moved{
            // SESSIONS-LEFT-1101（owner 198 の 2「Build now で Project が出来たら、その session はその Project の中へ」）: 会話は作った Project の 1 つ目の session の頭へ
            // 写った（`chat_head`）= Home の保存は消す（左に同じ会話を 2 行出さない・続きは Project の中の session）
            if let Some(key)=self.shell.talk_id.clone(){self.generated.forget_chat(&key);}
            self.new_chat();
        }
        moved
    }
    /// L2: model の答えが着いたら、頼んだ位置の待つ 1 行だけを差し替える（答え・選択肢・案）。失敗は `local_talk` の問いと選択肢。
    /// CHAT-FREE-1101（owner 198 通目の 6）: 答えは AI が形を決めた Markdown のまま（`ui::markdown` が描く）・選択肢の数も AI 次第（前の 3 つまでの切りをやめた）。
    /// CHAT-ONE-1101: Project の Chat の答えはその Project の会話に足す（待つ 1 行は保存していない）・失敗は `local_change`。
    fn poll_talk(&mut self,ctx:&egui::Context){
        use crate::domain::chat::{local_change,Mark,Turn,Who,TALK_WAIT};
        let Some((_,_,rx))=self.talk_rx.as_ref() else{return};
        let got=match rx.try_recv(){
            Err(std::sync::mpsc::TryRecvError::Empty)=>{ctx.request_repaint_after(Duration::from_millis(200));return}
            // ENGINE-AUDIT W9 本体: 分けた結果が先に届く = それに従い（act は Engine へ・候補は問いの行）、会話の AI の答えが続くなら待ち続け、続かなければここで閉じる
            Ok(crate::generated::TalkMsg::Routed{project,say,route})=>{
                self.generated.follow_route(&project,&say,&route);
                ctx.request_repaint();
                if !route.talk{self.talk_rx=None;}
                return
            }
            Ok(crate::generated::TalkMsg::Reply(r))=>r.ok(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)=>None,
        };
        let Some((project,idx,_))=self.talk_rx.take() else{return};
        ctx.request_repaint();
        let said=match &project{
            Some(key)=>self.generated.chat_in(key).iter().rev().find(|t|t.who==Who::You).map(|t|t.text.clone()),
            None=>self.shell.talk.iter().take(idx).rev().find(|t|t.who==Who::You).map(|t|t.text.clone()),
        }.unwrap_or_default();
        // 周 7 #1: 送った後に別の呼び出しが上限に着いて断られた時も、関係の無い問いでなく止まった事と次の一手
        let stopped=self.generated.llm_stopped().filter(|_|got.is_none());
        let (ask,choices,proposal)=match (&project,stopped){
            (Some(_),why)=>local_change(&said,why.as_deref()),
            (None,why)=>self.no_ai_reply(why.as_deref(),&said),
        };
        let mut turn=Turn{who:Who::Shibubu,text:ask,mark:Mark::None,at:crate::runtime::now_rfc3339(),attachments:Vec::new(),choices,proposal,preview:Vec::new(),sections:Vec::new(),needs:None};
        if let Some(v)=got{
            let line=|x:&serde_json::Value|x.as_str().map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned);
            turn.text=v.get("ask").and_then(line).unwrap_or(turn.text);
            turn.choices=v.get("choices").and_then(|x|x.as_array()).map(|a|a.iter().filter_map(line).collect()).unwrap_or_default();
            turn.proposal=v.get("proposal").and_then(line);
            // DOGFOOD-X 周 5 #5: 作る前に答えが要る問い（model が needs に入れた物だけ・案が在る時だけ）
            turn.needs=if turn.proposal.is_some(){v.get("needs").and_then(line)}else{None};
        }
        match project{
            Some(key)=>self.generated.say_in(&key,turn),
            None=>{
                let kept=self.shell.talk.get_mut(idx).filter(|t|t.who==Who::Shibubu&&t.text==TALK_WAIT).map(|t|{
                    (t.text,t.choices,t.proposal,t.needs)=(turn.text,turn.choices,turn.proposal,turn.needs);
                    t.clone()
                });
                // SESSIONS-LEFT-1101: 待つ 1 行は保存していない = 答えの行をここで保存する
                if let Some(t)=kept{self.keep_home_turn(&t);}
            }
        }
    }
    /// 一言 → Project（`create_project`）か 1 Page の app（`create`）。作れなければ Home に残り、入力欄に一言を戻して理由を 1 行。
    /// W13: 利用者が login 済みの LLM の CLI が在れば、一言とこの Mac の app の一覧（名前と bundle id）を model（`claude -p`）に送って読ませる。
    /// 呼び出しは裏の thread（`Generated::start_create`）で、待つ間も画面は動き「AI が一言を読んでいます」の 1 行を出す（`poll_model` が開く）。
    /// CLI が無ければ名前の表（主デモの一言は Project・それ以外は今の `create`）。CLI の確かめの途中なら一言を持って待つ（`poll_llm`）。
    /// bridge は Project を作る時だけ起こして app が在るかを聞き、終わったら止める（Drop）。起こせなければ None = 面は unavailable と理由。
    /// G104: `attach` = 一言に添えた物（Home の token）。送る直前に確かめ直し（消えた・大きくなった・秘密を書き足した）、通らなければ送らずに欄と token へ戻して 1 行。
    fn create_app(&mut self,text:String,attach:Vec<crate::domain::chat::Attachment>){
        // 前の一言・編集を AI が読んでいる間は受けない（一言は Home の欄に戻す・「読んでいます」の 1 行は Home に出ている）
        if self.generated.model_busy_line().is_some()||self.waiting.is_some(){self.back_to_home_field(text);self.shell.attach=attach;return;}
        self.generated.new_request(); // TALK-1101 追記 2: Build now・⌘K の Create = 1 つの頼み（止まっているかを見る前に数え直す）
        if self.llm_rx.is_some(){self.asked_from=Some(self.shell.view.clone());self.waiting=Some((text,attach,Instant::now()));return;}
        if let Err(line)=crate::domain::chat::recheck(&attach,&self.generated.accepts(),&self.ai_name()){
            self.back_to_home_field(text);self.shell.attach=attach;self.shell.attach_line=Some(line);return;
        }
        // L2（review A・C）: 案の Build now（会話の案と同じ文）は変更の文でも AI が読む道（いちばん新しい Project を書き換えない）・会話は作った Project の頭へ。
        // 変更の文の振り分け（`said_is_edit` → `apply_home_edit`）は Home の素の一言（⌘K の Create 等）だけ
        let accepted=crate::domain::chat::pending_proposal(&self.shell.talk)==Some(text.as_str());
        if accepted{self.generated.accept_plan(&text,self.shell.talk.clone());}
        // DOGFOOD-X 周 7 #2: AI が上限で止まっていれば、会話と Preview が言ったとおり名前の表で作る（AI に送って断られる 1 手と Use simple rules を挟まない）。
        // REVIEW-R7 R1（B44）: Preview で AI が読んだ案が在れば下の腕（AI を呼ばない = 止まっていても見せた Page と app が出来る）。Preview の読みが上限の最後の 1 回でも同じ
        if accepted&&self.generated.llm_stopped().is_some()&&self.generated.previewed(&text).is_none(){self.create_simple(text);self.say_unused(&attach);return;}
        // DOGFOOD-X 周 2 回 1 #2: Preview で AI が読んだ案は読み直さずに保存する（見せた Page と app が出来る・添えた物は Preview の読みに乗った）
        if let Some(project)=self.generated.previewed(&text).filter(|_|accepted){
            let mut bridge=if project{spawn_bridge()}else{None};
            let made=self.generated.create_project(&text,self.workspace.as_deref(),bridge.as_mut());
            self.create_from=self.asked_from.take();
            self.on_created(text,made);
            return;
        }
        if (accepted||!crate::generated::said_is_edit(&text))&&self.generated.start_create(&text,self.workspace.as_deref(),&attach){self.create_from=Some(self.asked_from.take().unwrap_or_else(||self.shell.view.clone()));return;}
        // model を通らない一言（変更の文・上限で止まった AI）に添えた物は使わない（AI 無し・読めない AI は上の recheck が送らずに止める）。使わなかった物は 1 行で言う（反証 G-A: 黙らない）
        self.create_now(text);
        self.say_unused(&attach);
    }
    /// 反証 G-A: AI を通らずに作った時、添えた物を使わなかった事を下の panel の Chat の 1 行で言う（何も添えていなければ言わない）。
    fn say_unused(&mut self,attach:&[crate::domain::chat::Attachment]){
        let names:Vec<String>=attach.iter().map(|a|a.name()).collect();
        // 作れずに Home へ戻った時は言わない（欄と理由の 1 行が Home に在る）。作った Project では下の panel の Chat を開いて言う
        if names.is_empty()||!matches!(self.shell.view,View::Project{..}){return}
        let verb=if names.len()==1{"wasn't"}else{"weren't"};
        self.shell.chat_note=Some(format!("Made without the AI, so {} {verb} used. Attach again when the AI is ready",names.join(", ")));
        if let View::Project{project,..}=self.shell.view.clone(){let _=self.show_chat(&project);}
    }
    /// HOME-ATTACH-1101（owner 185 通目「語は今の AI の名前」）: 送る時に添付を断る 1 行の主語（`chat::recheck` の `who`）。
    /// 足した AI（`AiAccount::Found` = Ollama・LM Studio・Grok）はその名前・System default と Claude の plan / 鍵は Claude Code。
    fn ai_name(&self)->String{
        match self.store.ui.active_account(){Some(AiAccount::Found{name,..})=>name.clone(),_=>"Claude Code".into()}
    }
    /// G104: 選んだ・落とした・貼った file（folder・zip も）を添える（1 つずつ `chat::attach` と `chat::add`・通らなかった最後の 1 行を欄の下に）。足した後は欄に focus（H04）。
    /// HOME-ATTACH-1101（owner 185 通目）: 今の AI が何でも受け取る（写真を読めない AI でも長方形に置く・読めるかは送る時の `recheck`）。
    fn add_attachments(&mut self,paths:Vec<PathBuf>){
        use crate::domain::chat;
        let mut line=None;
        for p in paths{
            if let Err(e)=chat::attach(&p).and_then(|a|chat::add(&mut self.shell.attach,a)){line=Some(e);}
        }
        (self.shell.attach_line,self.shell.focus_command)=(line,true);
    }
    /// G104: Home の窓に落とした file を添える（毎 frame・待たない・`+` の選ぶ panel は ADD-FILE から `Intent::Attach` の中で同期に添える）。
    /// HOME-ATTACH-1101（owner 175 通目）: Home の欄への ⌘V も、欄が文を受ける前（描く前のこの frame の入力）にここで見る。clipboard に file が在れば
    /// （Finder の ⌘C・文は file の名前だけ）その file を添付に・`chat::PASTE_LINES` 行以上の文は欄に入れず `Pasted` の添付 1 つに。それ以外の貼り付けは触らない
    /// （今のまま欄に入る）。見るのは欄（header の `kangae-command`）に focus が在り ⌘K の list と Rename が閉じている間だけ（他の欄の貼り付けを取らない）。
    fn poll_attach(&mut self,ctx:&egui::Context){
        if self.shell.view!=View::Home{return}
        let dropped:Vec<PathBuf>=ctx.input(|i|i.raw.dropped_files.iter().filter_map(|f|f.path.clone()).collect());
        if !dropped.is_empty(){self.add_attachments(dropped);}
        let field=ctx.memory(|m|m.has_focus(egui::Id::new("kangae-command")));
        if !field||self.shell.palette.open||self.shell.renaming.is_some()||!ctx.input(|i|i.events.iter().any(|e|matches!(e,egui::Event::Paste(_)))){return}
        let read_files=self.paste_files;
        let (mut files,mut pasted,mut line)=(Vec::new(),false,None);
        ctx.input_mut(|i|i.events.retain(|e|{
            let egui::Event::Paste(text)=e else{return true};
            let found=read_files();
            if !found.is_empty(){files.extend(found);return false}
            match crate::domain::chat::paste(text){
                None=>true,
                Some(r)=>{pasted=true;line=r.and_then(|a|crate::domain::chat::add(&mut self.shell.attach,a)).err();false}
            }
        }));
        if pasted{(self.shell.attach_line,self.shell.focus_command)=(line,true);}
        if !files.is_empty(){self.add_attachments(files);}
    }
    /// FX2（`Intent::Build`）: Home の生成の 1 行で押した手。Stop = CLI を確かめている間なら待ちを捨てて文を欄へ・生成中なら job の印を立てる
    /// （子が消えて `ModelDone::Stopped` が届くまで欄は灰）・Try again / Use simple rules = 欄の今の文で（空なら欄に focus）・Say it another way = 欄に focus・
    /// Open Runs = その生成の run を開いた Runs へ。refine の手（AllowConnection・OpenInSource）は Home の生成の行に出ない = 何もしない。
    fn build_step(&mut self,ctx:&egui::Context,a:BuildAction){
        match a{
            BuildAction::Cancel=>match self.waiting.take(){
                Some((text,attach,_))=>{self.asked_from=None;self.back_to_home_field(text);self.shell.attach=attach;} // G104: 添付も欄に戻す
                None=>{self.generated.stop_model_job();}
            },
            BuildAction::Retry|BuildAction::UseSimpleRules=>{
                let said=std::mem::take(&mut self.shell.command);
                if said.trim().is_empty(){self.shell.command=said;self.shell.focus_command=true;return}
                if a==BuildAction::Retry{let attach=std::mem::take(&mut self.shell.attach);self.create_app(said,attach)}else{self.create_simple(said)}
            }
            BuildAction::Rephrase=>self.shell.focus_command=true,
            BuildAction::OpenRun=>{
                if let Some(id)=self.generated.model_run_id(){ui::runs::open_run(ctx,id);}
                self.go(View::Runs);
            }
            BuildAction::AllowConnection(_)|BuildAction::OpenInSource(_)=>{}
        }
    }
    /// FX2（Use simple rules）: AI を呼ばず名前の表で作る（作った後の 1 行が AI を使わなかった事を言う）。
    fn create_simple(&mut self,text:String){
        let project=matches!(crate::composer::project::compose_project(&text,&Default::default(),None),Ok(Some(_)));
        let mut bridge=if project{spawn_bridge()}else{None};
        let made=self.generated.create_project_simple(&text,self.workspace.as_deref(),bridge.as_mut());
        self.open_made(text,made);
    }
    /// model 無しで今作る（名前の表・手元で速い）。
    fn create_now(&mut self,text:String){
        let project=matches!(crate::composer::project::compose_project(&text,&Default::default(),None),Ok(Some(_)));
        let mut bridge=if project{spawn_bridge()}else{None};
        let made=self.generated.create_project(&text,self.workspace.as_deref(),bridge.as_mut());
        self.create_from=self.asked_from.take();
        self.on_created(text,made);
    }
    /// FX-042: 裏の生成の答え。頼んだ場所を離れている間に出来た Project では画面を替えない（今の作業と打っていた文を中断しない）。知らせは窓の下の帯の状態の 1 行と Runs の行・Projects の一覧
    fn on_created(&mut self,goal:String,made:Result<Option<View>,String>){
        let left=self.create_from.take().is_some_and(|from|from!=self.shell.view);
        match (&made,left){
            (Ok(Some(_)),true)=>{self.settle_talk(&goal);self.message="A new project is ready · Open it from Home".into()}
            // 1 Page の app（Ok(None)）も離れている間は画面を替えない（REVIEW-FX2 b1）。作る事はそのまま・知らせは Project の時と同じ
            (Ok(None),true)=>match self.generated.create_in_chat(&goal){
                Ok(_)=>{self.settle_talk(&goal);self.message="A new project is ready · Open it from Home".into()}
                Err(e)=>self.open_made(goal,Err(e)),
            },
            _=>self.open_made(goal,made),
        }
    }
    /// `create_project` の答えを開く（Ok(None) は 1 Page の `create_in_chat` = Project の Chat に会話の頭を書く・周 7 #6）。作れなければ Home に一言と理由を戻す。
    /// 案の Build now で会話が Project へ移った時は、その Project の Chat を開く（dogfood r6: 畳んだ panel の中に `I picked: …` と `Made …` が在り、
    /// Mirror だけの Page では Build now の答えが何も見えなかった）。欄から直に作った時は今まで通り畳んだまま。
    fn open_made(&mut self,text:String,made:Result<Option<View>,String>){
        let made=match made{
            Ok(Some(v))=>Ok(v),
            Ok(None)=>self.generated.create_in_chat(&text).map(View::App),
            Err(e)=>Err(e),
        };
        match made{
            Ok(v)=>{
                let moved=self.settle_talk(&text);
                self.go(v);
                // route が断れば Project でない（今の画面に理由の 1 行）
                if let (true,View::Project{project,..})=(moved,self.shell.view.clone()){let _=self.show_chat(&project);}
            }
            // 先に Home の欄へ戻してから移る（移れなくても今の場所の欄〈Project の変更の打ちかけ〉には入らない）
            Err(reason)=>{self.generated.note=Some(reason);self.back_to_home_field(text);self.go(View::Home);}
        }
    }
    /// 作る依頼の文を Home の欄へ戻す 1 か所（今が Home なら欄、他〈Project・Runs などの探す欄〉なら Home の打ちかけ = Home へ移った時に欄に出る）。
    /// 今の場所の欄（Project の変更・探す欄の打ちかけ）には書かない（UI-4a 反証・DOGFOOD-X 周 2 回 8）。Home に別の打ちかけが在れば戻す文で置き換える（理由の 1 行と、打ち直す物はこの文）。
    fn back_to_home_field(&mut self,text:String){
        if self.shell.view.place()=="home"{self.shell.command=text;}else{self.shell.drafts.insert("home".into(),text);}
    }
    /// 主領域を移す 1 か所。Page の段（古い View::App の変換・同じ Project は navigate_page・離れる時は今の Page を保存）は `Generated::route`。
    /// 開けない・移れない・離れる Page を保存できない時は今の画面に残り、理由の 1 行（Page の上と窓の下の帯）。
    fn go(&mut self,view:View){
        let from=self.shell.view.place().to_owned();
        match self.generated.route(&self.shell.view,view){
            // UI-5a: 旧 canvas の 2 つの確認は CompatCanvas の中の inline の行 → 別の View へ移ったら閉じる（見えない確認が cover と Esc を握らない）
            Ok(v)=>{if v!=self.shell.view{self.delete_session_prompt=false;self.delete_panel_prompt=None;self.message.clear();}self.shell.go(v)} // FX-027: 別の View へ移ったら前の場所の状態の 1 行は消す（route の Err の理由はこの後に書く）
            Err(line)=>self.message=line,
        }
        if self.shell.view.place()!=from{self.mark();} // UI-4b: 打ちかけが場所を移った = 次の保存で UiSettings::drafts へ
        // RQ1: 最後に開いた Project（When Shibubu opens = Last project の行き先）。memory の store の Project は閉じたら消えるので覚えない（保存した方を残す）
        if let View::Project{project,..}=self.shell.view.clone(){
            if self.generated.saves()&&self.store.ui.last_project.as_ref()!=Some(&project){self.store.ui.last_project=Some(project.clone());self.mark();}
            // SESSIONS-LEFT-1101（owner 210 の 6「押すと勝手にdropdownになるように」）: Project へ移ったら左の sidebar のその Project を開く（session と Page が見える）
            if self.shell.view.place()!=from&&!self.store.ui.open_projects.contains(&project){self.store.ui.open_projects.push(project.clone());self.mark();}
            // RQ2（RQ-007 Recent）: Project へ移った時刻（同じ Project の中の Page の移りでは書かない）
            if self.shell.view.place()!=from{self.store.ui.opened_projects.insert(project,crate::google::now() as i64);self.mark();}
        }
    }
    fn add_session(&mut self){
        let s=engine::generate(&self.prompt,&self.repo);
        self.store.selected=s.id;self.go(View::CompatCanvas(s.id));self.store.sessions.insert(0,s);self.selected_panel=None;
        self.windows.clear();self.controls.clear();self.mark();
        self.message="Built controls and a workflow · Check the connection".into();
    }
    fn ensure_action(&mut self,name:&str,kind:ActionKind)->Action {
        let action=Action::new(name,kind);
        if let Some(s)=self.store.selected_mut(){s.actions.push(action.clone());}
        self.mark();action
    }
    fn queue(&mut self,actions:Vec<Action>,title:&str){
        if actions.is_empty(){self.message="No actions to run".into();return}
        if self.editing && actions.iter().any(|a|a.kind.is_write()){
            self.message="Editing layout · actions are off".into();
            return;
        }
        self.pending=Some(Pending{session:self.store.selected,title:title.into(),actions,other_lane:None});
    }
    fn queue_parallel(&mut self,session:&Session){
        if session.workflows.len()<2 {self.message="Two workflows are needed".into();return}
        let first=engine::valid_workflow(session,&session.workflows[0]);
        let second=engine::valid_workflow(session,&session.workflows[1]);
        match (first,second){
            (Ok(a),Ok(b))=>{
                if self.editing && a.iter().chain(b.iter()).any(|step|step.kind.is_write()){
                    self.message="Run workflows that write in control mode".into();return
                }
                if !engine::disjoint(&a,&b){self.message="Two runs can't use the same window".into();return}
                self.pending=Some(Pending{session:session.id,title:"Two-lane parallel workflow".into(),actions:a,other_lane:Some(b)});
            }
            (Err(e),_)|(_,Err(e))=>self.message=format!("Can't run in parallel: {e}"),
        }
    }
    /// 「確認して実行」が押された。(A) この 1 回の承認 = 画面の操作 1 つにつき 1 つの ConsentReceipt（見せた payload の digest・この 1 回だけの run_id）
    fn start_pending(&mut self){
        let Some(p)=self.pending.take() else{return};
        let tx=self.tx.clone();let features=self.store.features.clone();let active=self.active_resources.clone();
        let app_id=p.session.to_string();
        let consents:HashMap<u64,ConsentReceipt>=p.actions.iter().chain(p.other_lane.iter().flatten())
            .filter_map(|a|desktop_consent(&app_id,&a.kind).map(|c|(a.id,c))).collect();
        let first=p.actions;
        if let Some(second)=p.other_lane {
            let tx_b=tx.clone();let f_b=features.clone();let active_b=active.clone();let c_b=consents.clone();
            thread::spawn(move||run_lane(p.session,second,tx_b,f_b,active_b,c_b));
        }
        thread::spawn(move||run_lane(p.session,first,tx,features,active,consents));
    }
    fn new_gui_binding(&mut self,tool:DesktopTool){
        let Some(w)=self.windows.get(self.selected_window).cloned() else{self.message="Find the windows first".into();return};
        let Some(c)=self.controls.get(self.selected_control).cloned() else{self.message="Read the controls first".into();return};
        let name=format!("{} / {}",w.title,c.label);
        let action=self.ensure_action(&name,ActionKind::Desktop{pid:w.pid as i64,window_id:w.window_id,
            label:c.label,role:c.role,tool,text:String::new(),app_name:w.app_name.clone()});
        if let Some(s)=self.store.selected_mut(){
            let wf=match s.workflows.iter().position(Workflow::is_desktop){Some(i)=>s.workflows.get_mut(i),None=>s.workflows.first_mut()};
            if let Some(wf)=wf{wf.steps.push(action.id);self.message=format!("Added a step to “{}”",wf.name);}
        }
        self.mark();
    }
    fn new_blank_session(&mut self) {
        let mut session=Session::starter();
        session.name="Untitled canvas".into();
        self.store.selected=session.id;
        self.go(View::CompatCanvas(session.id));
        self.store.sessions.insert(0,session);
        self.selected_panel=None;
        self.mark();
    }
    fn clone_session(&mut self) {
        if let Some(s)=self.store.selected().cloned(){
            let copied=s.duplicate();
            self.store.selected=copied.id;
            self.go(View::CompatCanvas(copied.id));
            self.store.sessions.insert(0,copied);
            self.selected_panel=None;
            self.windows.clear();self.controls.clear();
            self.mark();
        }
    }
    fn append_panel(&mut self, kind:PanelKind, title:&str) {
        if let Some(session)=self.store.selected_mut() {
            let index=session.panels.len();
            let panel=Panel::new(kind.clone(),title,index%2,index/2);
            self.selected_panel=Some(panel.id);
            session.panels.push(panel);
            if matches!(kind,PanelKind::GitHubIssues) && !session.actions.iter().any(|a|matches!(&a.kind,ActionKind::GitHubIssues{..})) {
                session.actions.push(Action::new("Get issues",ActionKind::GitHubIssues{repo:session.repo.clone()}));
            }
            if matches!(kind,PanelKind::GitHubPulls) && !session.actions.iter().any(|a|matches!(&a.kind,ActionKind::GitHubPulls{..})) {
                session.actions.push(Action::new("Get pull requests",ActionKind::GitHubPulls{repo:session.repo.clone()}));
            }
            if matches!(kind,PanelKind::WebService) && !session.actions.iter().any(|a|matches!(&a.kind,ActionKind::WebRead)) {
                session.actions.push(Action::new("Get web data",ActionKind::WebRead));
            }
            self.mark();
        }
    }
    fn active_count(&self,session:u64)->usize {self.running.get(&session).copied().unwrap_or(0)}
    /// §9: ⌘K palette / ⌘N 新規（= Home の欄に focus・UI-4a）/ ⇧⌘P rail / Esc overlay。旧 ⌘E（配置と操作の切替）は互換 view の中でだけ。
    fn keyboard(&mut self,ctx:&egui::Context) {
        // UI-4b: ⌃⌘S（sidebar を畳む / 戻す）は ⌘ の pattern より先に独立の文で取る（COMMAND の pattern は ⌃⌘ にも一致する・keys.md §3）
        if ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL|egui::Modifiers::MAC_CMD,egui::Key::S)){self.apply(ctx,Intent::ToggleSidebar);}
        // RQ1: ⌘,（app menu が在れば menu が先に取り `take_settings_request` へ・menu の無い窓と試験はここ）
        if crate::surface::take_settings_request()||ctx.input_mut(|i|i.consume_key(egui::Modifiers::COMMAND,egui::Key::Comma)){
            self.apply(ctx,Intent::Go(View::Settings(SettingsSection::General)));
        }
        let compat=matches!(self.shell.view,View::CompatCanvas(_));
        let (palette,create,chat,toggle)=ctx.input_mut(|i| (
            i.consume_key(egui::Modifiers::COMMAND,egui::Key::K),
            i.consume_key(egui::Modifiers::COMMAND,egui::Key::N),
            i.consume_key(egui::Modifiers::COMMAND|egui::Modifiers::SHIFT,egui::Key::P),
            compat && i.consume_key(egui::Modifiers::COMMAND,egui::Key::E),
        ));
        let escape=crate::ui::renderer::escape_for_app(ctx); // Terminal の grid に focus の間の Esc は PTY の物（W10 反証）
        let popup=ctx.memory(|m|m.any_popup_open()); // UI-5a: 開いた menu（帯の ＋ •••）の Esc は menu だけを閉じる（egui の popup が閉じる・同じ Esc で確認や rail まで閉じない）
        if palette{if self.shell.palette.open{self.shell.palette.close(ctx,&mut self.shell.command)}else{self.shell.palette.open(ctx,&mut self.shell.command)}}
        if create{self.apply(ctx,Intent::Go(View::Home));}
        if chat{
            // CHAT-SIDE-1101 追記 219: 右に出している間の ⇧⌘P は ◨ と同じ（panel ごと下へ）・他は Chat を選んで開く（右で閉じていれば右で開く・⌘K の Show chat = Intent::OpenChat は置き場を替えない）
            let right=match &self.shell.view{View::Project{project,..}=>self.store.ui.chat_place==ChatPlace::Right&&self.generated.panel(project).open,_=>false};
            self.apply(ctx,if right{Intent::ChatPlace(ChatPlace::Bottom)}else{Intent::OpenChat});
        }
        if toggle{self.editing=!self.editing;}
        if self.delete_panel_prompt!=self.selected_panel{self.delete_panel_prompt=None;} // UI-5a: 部品の確認は選んでいる部品の inspector の行（閉じた・選び直したら閉じる）
        if escape && !popup{
            if self.shell.palette.open{self.shell.palette.close(ctx,&mut self.shell.command);}
            else if self.quit_failed.is_some(){self.quit_failed=None;} // FX1: Esc = Keep open
            else if self.pending.is_some()||self.delete_session_prompt||self.delete_panel_prompt.is_some(){
                self.pending=None;self.delete_session_prompt=false;self.delete_panel_prompt=None;
            }else if self.shell.end_rename()||self.shell.project_confirm.take().is_some()||self.shell.data_confirm.take().is_some()
                ||self.shell.end_connecting()||self.cancel_connection_confirm(){ // RQ2: 名前の欄と Project の消す確認・RQ3: Delete my data の確認・RQ4: Connections の文の欄と行内の確認（何も変えない）
            }else if self.generated.dismiss_delete()||self.generated.dismiss_rename(){ // UI-5a: tab の横の消す確認・FX3: tab の席の名前の欄（打った名前を捨てる）
            }
        }
        // W8: Project の中だけ ⇧⌘Z / ⌘Z・⌃` で下の panel の今の tab（既定 = Chat・L1-NORAIL）を開閉（G95・⌃⇧` で Terminal の tab を足す・⌘D で左右に割る・⌘1〜9 で tab）。UI-5a: ⌘Z は文字の在る欄に focus が在る時だけ欄の undo に譲る
        // （Terminal・tab・button・空の欄に focus でも Project へ = ax::undo_for_project・図 §38）・⌘T で Page を足す（＋ の tooltip が言う鍵）。
        // ⌘Z の型は shift も拾う（matches_logically）ので ⇧⌘Z を先に取る
        // ⌃` の型も shift を拾うので ⌃⇧` を先に取る
        if matches!(self.shell.view,View::Project{..}){
            let field=!crate::ui::ax::undo_for_project(ctx);
            const NUMS:[egui::Key;9]=[egui::Key::Num1,egui::Key::Num2,egui::Key::Num3,egui::Key::Num4,egui::Key::Num5,egui::Key::Num6,egui::Key::Num7,egui::Key::Num8,egui::Key::Num9];
            let (redo,undo,new_tab,toggle,add,split,nth)=ctx.input_mut(|i| (
                !field && i.consume_key(egui::Modifiers::COMMAND|egui::Modifiers::SHIFT,egui::Key::Z),
                !field && i.consume_key(egui::Modifiers::COMMAND,egui::Key::Z),
                i.consume_key(egui::Modifiers::CTRL|egui::Modifiers::SHIFT,egui::Key::Backtick),
                i.consume_key(egui::Modifiers::CTRL,egui::Key::Backtick),
                i.consume_key(egui::Modifiers::COMMAND,egui::Key::T),
                i.consume_key(egui::Modifiers::COMMAND,egui::Key::D),
                NUMS.iter().position(|k|i.consume_key(egui::Modifiers::COMMAND,*k)),
            ));
            if redo{self.apply(ctx,Intent::Page(PageIntent::Redo));}
            else if undo{self.apply(ctx,Intent::Page(PageIntent::Undo));}
            if add{self.apply(ctx,Intent::Page(PageIntent::Add));}
            // TITLEBAR-ICONS-1101 の 2'（案 A）: ⌘1〜9 は Terminal に focus が在る時（xterm の頁・egui の grid）だけ Terminal の tab（G95 のまま）・他は n 枚目の Page
            let in_terminal=self.term_keys||crate::ui::ax::terminal_focused(ctx);
            let (term_nth,page_nth)=if in_terminal{(nth,None)}else{(None,nth)};
            if let (Some(n),View::Project{project,..})=(page_nth,self.shell.view.clone()){
                if let Some((page,_,_))=self.generated.page_list(&project).into_iter().nth(n){self.apply(ctx,Intent::Go(View::Project{project,page}));}
            }
            // PANEL-SYNC-1101: 畳んだ間の ⌃` は下に開く（右に置いていても = 右の sidebar は畳まれる）・開いている間は今の置き場で畳む
            let folded=match &self.shell.view{View::Project{project,..}=>!self.generated.panel(project).open,_=>false};
            if toggle&&folded{self.apply(ctx,Intent::OpenBelow(PanelAct::Toggle));}
            let panel=[new_tab.then_some(PanelAct::Add),(toggle&&!folded).then_some(PanelAct::Toggle),split.then_some(PanelAct::Split),term_nth.map(PanelAct::SelectNth)];
            for act in panel.into_iter().flatten(){self.apply(ctx,Intent::Page(PageIntent::TerminalPanel(act)));}
        }else if matches!(self.shell.view,View::Home|View::Runs|View::Connections|View::Settings(_)){
            // HOME-TERM-1101: Project の外の ⌃` / ⌃⇧` = Home の Terminal（Page の手は無いので panel に直に）
            let (new_tab,toggle)=ctx.input_mut(|i|(i.consume_key(egui::Modifiers::CTRL|egui::Modifiers::SHIFT,egui::Key::Backtick),i.consume_key(egui::Modifiers::CTRL,egui::Key::Backtick)));
            for act in [new_tab.then_some(PanelAct::Add),toggle.then_some(PanelAct::Toggle)].into_iter().flatten(){
                if let Err(e)=self.generated.panel_act(crate::generated::HOME_PLACE,act){self.message=e;}
            }
        }
    }
    /// sidebar / Home / palette / 欄の右端に出す 1 行ずつ（Session → AppRow）。
    /// 生成した app（新しい順）→ 互換 view の Session（手を付けていない最初の 1 つは出さない）。
    fn rows(&self)->Vec<AppRow>{
        let compat=self.store.sessions.iter().filter(|s|!s.is_untouched_starter()).map(|s|{
            let running=self.active_count(s.id);
            let parts=s.panels.iter().filter(|p|!p.hidden).count();
            let status=if running>0{format!("{running} running")}else{format!("{parts} parts · {} workflows",s.workflows.len())};
            AppRow{view:View::CompatCanvas(s.id),name:s.name.clone(),purpose:s.prompt.clone(),status,running,needs:0,pinned:false,archived:false,opened_at:None,sample:false}
        });
        // RQ2: Project を管理する印は UiSettings が持つ（Generated は設定を持たない）→ ここで行に重ねる
        let ui=&self.store.ui;
        self.generated.rows().into_iter().map(|mut row|{
            if let View::App(app)=&row.view{
                let id=format!("project.{app}");
                row.pinned=ui.pinned_projects.contains(&id);row.archived=ui.archived_projects.contains(&id);
                row.sample=ui.sample_projects.contains(&id);row.opened_at=ui.opened_projects.get(&id).copied();
            }
            row
        }).chain(compat).collect()
    }
    fn location(&self)->String{
        match &self.shell.view{
            View::CompatCanvas(id)=>self.store.sessions.iter().find(|s|s.id==*id).map(|s|s.name.clone()).unwrap_or_else(||"App".into()),
            View::App(id)=>self.generated.title(id).unwrap_or_else(||"App".into()),
            View::Project{project,..}=>self.generated.app_of_project(project).and_then(|a|self.generated.title(&a)).unwrap_or_else(||"App".into()),
            v=>v.nav_label().to_owned(),
        }
    }
    /// RQ2 の Remove と Delete my data › Connections の 1 つの口（REVIEW-RQN 4）: 鍵の account は Keychain の項目を今消す・plan の account は裏の thread で
    /// その dir の login を `self.sign_out` で消させてから dir を消す（`claude auth logout` を UI の thread で待たない・最大 20 秒）。列からは呼ぶ側が今外す。
    /// 保証しない: 裏の dir の消し忘れを画面で言う事（log だけ・id は使い回さないので次の Add には響かない）。
    fn forget_account(&self,acct:&AiAccount)->Result<(),String>{
        if !matches!(acct,AiAccount::Plan{..}){return crate::llm_cli::remove_account(&self.data_dir,acct,&crate::storage::secret_store::SecretStore::from_env(),self.sign_out)}
        let (dir,acct,out)=(self.data_dir.clone(),acct.clone(),self.sign_out);
        thread::Builder::new().name("ai-account-remove".into()).spawn(move||{
            if let Err(why)=crate::llm_cli::remove_account(&dir,&acct,&crate::storage::secret_store::SecretStore::memory(),out){eprintln!("shibubu: remove AI account: {why}");}
        }).map(|_|()).map_err(|_|"This account couldn't be removed · Try again".to_owned())
    }
    /// 直近の message は窓の一番下の帯の右（STATUSBAR-1101・`ui::shell::status_bar`）。保存の状態（読み戻していない `Saved`）は出さない（tally の L1・sendback 56）。
    /// RQ-USAGE: 使用量の 1 行と隣の button（窓の一番下の帯・Settings › AI & Models で同じ物）。None = 出さない（AI が無い・鍵の account の帯・試験の `open`）。
    /// 帯は次の一手の無い `NoCli`（鍵の account の `API key · billed per use`・使用量の無い AI）を None にする（210: その AI は `bar_ais` の `--` と hover の理由）。Settings は読めた時も `Refresh`（帯は 60 秒ごとに読み直すだけ）。
    /// 読む前の帯（operator ①）: `USAGE_ASK`（帯は hover にだけ出す）と `Show usage`（= `RefreshUsage`）だけ・数字は出さない。押した後の読みの途中は `Checking usage` の 1 行だけ。
    fn usage_view(&self,band:bool)->Option<(String,Option<(&'static str,Intent)>)>{
        if !self.read_usage{return None}
        let Some(u)=self.generated.plan_usage() else{
            // ACCOUNTS-1101: Claude Code の login か Claude の plan の account だけ（足した CLI の account も plan の % を持たない）
            let plan=self.generated.llm_ready()&&self.store.ui.active_account().is_none_or(|a|matches!(a,AiAccount::Plan{..})&&a.provider()=="claude");
            return (band&&plan).then(||if self.usage_asked{("Checking usage".to_owned(),None)}else{(ui::shell::USAGE_ASK.to_owned(),Some((SHOW_USAGE,Intent::RefreshUsage)))});
        };
        let step=crate::llm_cli::usage_step(u);
        if band&&step.is_none()&&matches!(u,Err(crate::llm::LlmError::NoCli{..})){return None}
        let step=step.or((!band&&u.is_ok()).then_some("Refresh"));
        let intent=|s:&'static str|if s==crate::llm_cli::SIGN_IN_STEP||s==crate::llm_cli::INSTALL_STEP{Intent::AiStep(s)}else{Intent::RefreshUsage};
        Some((crate::llm::usage_line(u,crate::google::now() as i64),step.map(|s|(s,intent(s)))))
    }
    /// CHAT-FREE-1101 段 2（owner 201「AI の画用紙」）: chat の ```html の閉じた WebView（`Run` を押した 1 つ = `ui::markdown::live_canvas`）を合わせる。
    /// frame の終わりの Page の WebView（`web.sync`）と同じ所・同じ `cover`。枠が置かれない frame（scroll で一部が見えない・別の画面）は隠すだけ・
    /// 押した card が替わる / `Stop` で閉じる。中の button の頼みは見えている間だけ・1 回だけ受けて閉じる（押した回数より多く会話に送らない）: `Reply` = 欄の Enter と同じ写し
    /// （Home は会話・Project は編集の依頼 = `Shell::enter_intent`）・`Build` = 案の Build now が押せる時（案が在り、答えの要る問いが無い）だけ。開けなければ `Run` を外して状態の 1 行。
    #[cfg(all(feature="webview",target_os="macos"))]
    fn canvas_sync(&mut self,ctx:&egui::Context,frame:&eframe::Frame,cover:bool,holes:&[crate::surface::Frame],intents:&mut Vec<Intent>){
        use crate::domain::chat::{pending_needs,pending_proposal};
        use crate::surface::{webview::Canvas,CanvasMsg};
        let live=ui::markdown::live_canvas(ctx);
        let slots=ui::markdown::take_canvases(ctx);
        self.part_sync(frame,cover,holes,&slots,intents);
        let slot=slots.into_iter().find(|s|s.part.is_none()&&Some(s.key)==live);
        if self.canvas.as_ref().is_some_and(|c|Some(c.key)!=live){self.canvas=None;}
        let rect=slot.as_ref().map(|s|canvas_frame(s.rect));
        if let (true,Some(s),Some(r))=(self.canvas.is_none(),&slot,rect){
            match Canvas::open(frame,s.key,&s.html,crate::surface::CanvasBridge::Chat,&serde_json::Map::new(),r){
                Ok(c)=>self.canvas=Some(c),
                Err(why)=>{ui::markdown::set_live_canvas(ctx,None);self.message=why;}
            }
        }
        let Some(c)=self.canvas.as_mut() else{return};
        // 頼みを受けるのは枠が置かれた frame だけ（隠れている間〈別の画面・popup の下〉に頁の script が出した物で、移った先の会話へ送らない）
        let Some(msg)=c.frame(rect,cover,holes).into_iter().next().filter(|_|rect.is_some()&&!cover) else{return};
        self.canvas=None;
        ui::markdown::set_live_canvas(ctx,None);
        match msg{
            CanvasMsg::Reply(text)=>intents.push(if self.shell.view==View::Home{Intent::Talk(text)}else{Intent::Generate(text,Vec::new())}),
            CanvasMsg::Build if pending_proposal(&self.shell.talk).is_some()&&pending_needs(&self.shell.talk).is_none()=>intents.push(Intent::AcceptPlan),
            CanvasMsg::Build=>self.message="No plan yet · say what to make".into(),
            // AI-COMPOSE-1101: Page の部品の 3 つの口は chat の頁では開いていない（`canvas_message` の Chat が捨てる）
            CanvasMsg::Read{..}|CanvasMsg::Set{..}|CanvasMsg::Act{..}|CanvasMsg::Open|CanvasMsg::Allow{..}|CanvasMsg::Cancel=>{}
        }
    }
    /// AI-COMPOSE-1101（owner 206）: Page の AI が書いた部品（renderer の `canvas`）の閉じた WebView を合わせる。開いている Project の部品だけ持ち（Project が替われば閉じる）、
    /// この frame に置かれた枠は開いて合わせ・置かれない枠は隠すだけ（見えるたびに開き直して read を何度も走らせない）。html が替われば key が替わり新しく開く。
    /// 頼みは枠が置かれた frame だけ受ける: Set / Act は Page の手（`PageIntent::CanvasSet` / `CanvasAct`）・Read は裏の thread（`Generated::start_part_read`）で、
    /// 届いた答えはその node の一番新しい部品へ（`take_reads` → `Canvas::answer`・始められなければ理由の 1 行で頁の Promise を reject）。
    /// ponytail: 隠した部品は Project が替わるまで残す（html を替えた前の部品も）。多い Project で重くなれば今の木に無い node の部品を閉じる
    #[cfg(all(feature="webview",target_os="macos"))]
    fn part_sync(&mut self,frame:&eframe::Frame,cover:bool,holes:&[crate::surface::Frame],slots:&[ui::markdown::CanvasSlot],intents:&mut Vec<Intent>){
        use crate::surface::{webview::Canvas,CanvasBridge,CanvasMsg};
        let project=match &self.shell.view{View::Project{project,..}=>project.clone(),_=>String::new()};
        let key=|s:&ui::markdown::CanvasSlot|egui::Id::new((project.as_str(),s.key)).value();
        self.parts.retain(|(p,..)|*p==project);
        // TALK-HONEST-1101 追記 232: 「Let Shibubu open it」の run が ✓ で終わった部品は頁を開き直す（下の for が同じ枠で開く = 頁の script がもう一度 read・1 回だけ）
        let again=self.generated.take_rereads();
        self.parts.retain(|(p,n,..)|!again.iter().any(|(ap,an)|ap==p&&an==n));
        for (node,id,got) in self.generated.take_reads(){
            // 開けば読める誤りには頁の bridge が「Let Shibubu open it」を出す（押すと CanvasMsg::Open）
            let open=matches!(&got,Err(m) if m.open.is_some());
            let got=got.map_err(|m|m.line);
            if let Some(c)=self.parts.iter().rev().find(|p|p.1==node).and_then(|p|p.3.as_ref()){c.answer(id,&got,open);}
        }
        for s in slots{
            let Some((node,values))=&s.part else{continue};
            let k=key(s);
            if self.parts.iter().any(|(_,_,have,_)|*have==k){continue}
            let c=match Canvas::open(frame,k,&s.html,CanvasBridge::Page,values,canvas_frame(s.rect)){
                Ok(c)=>Some(c),
                Err(why)=>{self.message=why;None}
            };
            self.parts.push((project.clone(),node.clone(),k,c));
        }
        for (_,node,k,c) in self.parts.iter_mut(){
            let Some(c)=c else{continue};
            let rect=slots.iter().find(|s|s.part.is_some()&&key(s)==*k).map(|s|canvas_frame(s.rect));
            // OPEN-INPLACE-1101 の 1: その部品から始めた run の今（進めている・承認・止まった）を部品の所に（変わった時だけ頁へ・None = 消す）
            c.run(self.generated.part_run(&project,node).as_ref());
            for msg in c.frame(rect,cover,holes).into_iter().filter(|_|rect.is_some()&&!cover){
                match msg{
                    CanvasMsg::Set{key,value}=>intents.push(Intent::Page(PageIntent::CanvasSet{node:node.clone(),key,value})),
                    CanvasMsg::Act{intent,value}=>intents.push(Intent::Page(PageIntent::CanvasAct{intent,value})),
                    CanvasMsg::Read{id,app,what}=>if let Err(line)=self.generated.start_part_read(&project,node,id,&app,&what){c.answer(id,&Err(line),false);},
                    // TALK-HONEST-1101 追記 232: 部品の「Let Shibubu open it」= その部品の覚えた一言で Engine へ（始められなければ理由の 1 行）
                    CanvasMsg::Open=>if let Err(line)=self.generated.open_part(&project,node){self.message=line},
                    // OPEN-INPLACE-1101 の 2: 部品の所の承認の card = Page の Engine の card と同じ手（Always allow = 同じ app の同じ種類の手を以後聞かない）
                    CanvasMsg::Allow{always}=>intents.push(Intent::Page(PageIntent::Engine(if always{ui::engine_view::EngineAct::AlwaysAllow}else{ui::engine_view::EngineAct::Approve}))),
                    CanvasMsg::Cancel=>intents.push(Intent::Page(PageIntent::Engine(ui::engine_view::EngineAct::Cancel))),
                    CanvasMsg::Reply(_)|CanvasMsg::Build=>{}
                }
            }
        }
    }
    /// 「画面の操作（Shibubu Bridge）」の行を thread で 1 回確かめる（YC W17-2 #4・#10。確かめている間の再確認は重ねない）。cua-driver は起こさない。
    fn probe_gui(&mut self){
        if self.gui_checking{return}
        self.gui_checking=true;
        let tx=self.tx.clone();
        thread::spawn(move||{let (tier,detail)=gui_status(&mut AdapterRegistry::desktop());let _=tx.send(Event::GuiStatus(tier,detail));});
    }
    /// FX5: GitHub の行を thread で 1 回読み戻す（`gh auth token` → `GET /user`・確かめている間の再確認は重ねない・値は残さない）。
    fn probe_github(&mut self){
        if self.github_checking{return}
        self.github_checking=true;
        let tx=self.tx.clone();
        thread::spawn(move||{
            let (tier,detail)=match github::standing(){
                github::Standing::SignedIn{login}=>(DisplayTier::Ready,format!("Signed in as {login}")),
                github::Standing::NoCli=>(DisplayTier::Available,"Not signed in · Install the GitHub CLI (gh), then run gh auth login".into()),
                github::Standing::NotSignedIn=>(DisplayTier::Available,"Not signed in · Run gh auth login in a terminal".into()),
                github::Standing::Failed(why)=>(DisplayTier::Available,why),
            };
            let _=tx.send(Event::GithubStatus(tier,detail,github::clock_hhmm()));
        });
    }
    /// その種類の action が「失敗でも未検証でもない結果」を 1 つでも返したか（§10 Verified = 操作の証拠あり）。
    fn verified(&self,kind:fn(&ActionKind)->bool)->bool{
        self.store.sessions.iter().flat_map(|s|s.actions.iter())
            .any(|a|kind(&a.kind)&&matches!(self.results.get(&a.id),Some(Ok(r)) if !r.contains("未検証")))
    }
    /// Connections の行（GitHub・画面の操作・Web data・Trusted commands・MCP）。
    /// 契約（RQ4 から・本体で替える）: 各行に `key` と、その行で効く手だけの `acts` と `checked_at` を入れる。GitHub = `PasteSecret("token")`・`CheckAgain`・`Disconnect`
    /// （切っていれば `Reconnect`）／画面の操作 = `CheckAgain`（と今の `Open settings`）／Web data = 未設定 `Connect`・設定済み `CheckAgain`・`Disconnect`・`Remove`・token が要る時
    /// `PasteSecret("token")`（URL は `UiSettings::web_data_url`・env は試験の口）／Trusted commands = 見出しの行 `cli`（`Add` = `Choose program…`）と command ごとの行
    /// `cli:<id>`（program の名前・固定の引数・指紋の頭 8 文字・`Disconnect` か `Connect`・`Remove`）／MCP = `mcp::connection_rows` の行。
    /// 切った接続（`UiSettings::disconnected`）を使う Project は開け、その席は `verifier::Check::Fail(Need::Connect, "{name} is disconnected · Reconnect in Connections")` の 1 行（RQ-019 M26）。
    fn connections_model(&self)->ConnectionsModel{
        use crate::ui::connections::ConnAct as A;
        let row=|key:&str,name:String,state:DisplayTier,detail:String,acts:Vec<A>|Connection{name:name.into(),state,detail,recheck:false,
            checking:self.conn_checking.contains(key),settings:None,key:key.to_owned().into(),acts,checked_at:self.conn_at.get(key).copied()};
        // 確かめの答え（無ければ今の設定から言える事）
        let checked=|key:&str,state:DisplayTier,detail:String|self.conn_result.get(key).cloned().unwrap_or((state,detail));
        let github=if self.disconnected("github"){
            row("github","GitHub".into(),DisplayTier::Available,"Disconnected · Shibubu reads public repos only · Reconnect".into(),vec![A::Reconnect])
        }else if let Some((tier,text,at,when))=&self.github_status{
            // FX5: 読み戻した時だけ Ready。確認から GITHUB_TTL たった Ready は Connected に下げる
            let stale=*tier==DisplayTier::Ready&&when.elapsed()>GITHUB_TTL;
            let line=if at.is_empty(){text.clone()}else{format!("{text} · checked {at}")};
            let (state,detail)=(if stale{DisplayTier::Connected}else{*tier},if stale{format!("{line} · may be out of date")}else{line});
            let state=if self.verified(|k|matches!(k,ActionKind::GitHubIssues{..}|ActionKind::GitHubPulls{..}|ActionKind::GitHubCreateIssue{..})){DisplayTier::Verified}else{state};
            row("github","GitHub".into(),state,detail,vec![A::PasteSecret("token".into()),A::CheckAgain,A::Disconnect])
        }else{
            // FX-023: 未確認の行は Token set / No token を言わない。在るかは出さず、次の一手は Paste token と Check again。
            let (state,detail)=checked("github",DisplayTier::Available,"Checked when you open Connections".into());
            let state=if self.verified(|k|matches!(k,ActionKind::GitHubIssues{..}|ActionKind::GitHubPulls{..}|ActionKind::GitHubCreateIssue{..})){DisplayTier::Verified}else{state};
            row("github","GitHub".into(),state,detail,vec![A::PasteSecret("token".into()),A::CheckAgain,A::Disconnect])
        };
        let (mut gui_state,mut gui_detail)=self.gui_status.clone().unwrap_or((DisplayTier::Available,"Checked when you open Connections".into()));
        // FX-025: 読み戻して確かめた Ready は GITHUB_TTL をすぎたら下げる
        if gui_state==DisplayTier::Ready&&self.gui_checked.is_some_and(|t|t.elapsed()>GITHUB_TTL){gui_state=DisplayTier::Connected;gui_detail=format!("{gui_detail} · may be out of date");}
        // OPEN-INPLACE-1101 の 2: Always allow で許した手（「GarageBand · Click」）を行の下の checkbox に（外す = また聞く）
        let mut gui_acts=vec![A::CheckAgain];
        gui_acts.extend(crate::surface::always_allowed(&crate::surface::act_consent_path()).iter().map(|a|A::AllowTool{tool:a.label(),allowed:true}));
        let gui=Connection{name:GUI_ROW.into(),state:gui_state,detail:gui_detail,recheck:false,checking:self.gui_checking,settings:Some(AX_SETTINGS),key:"gui".into(),acts:gui_acts,checked_at:None};
        let web=match self.web_url(){
            // CONNECTIONS-1101: URL の欄は行の下（Test は欄の button）
            _ if self.shell.connecting.as_deref()==Some("web")=>row("web","Web data".into(),DisplayTier::Available,"Type the address below · Enter tests it".into(),vec![]),
            None=>row("web","Web data".into(),DisplayTier::Available,"Not set up · Connect to add a web address".into(),vec![A::Connect]),
            Some(url) if self.disconnected("web")=>row("web","Web data".into(),DisplayTier::Available,format!("Disconnected · {} · Reconnect",github::url_host(&url)),vec![A::Reconnect,A::Remove]),
            Some(url)=>{
                let (state,detail)=checked("web",DisplayTier::Available,web_detail(Some(&url)));
                let state=if self.verified(|k|matches!(k,ActionKind::WebRead)){DisplayTier::Verified}else{state};
                row("web","Web data".into(),state,detail,vec![A::PasteSecret("token".into()),A::CheckAgain,A::Disconnect,A::Remove])
            }
        };
        let commands:Vec<&RepoFeature>=self.store.features.iter().filter(|f|!f.cli_path.is_empty()).collect();
        let enabled=commands.iter().filter(|f|f.enabled).count();
        let cli=row("cli","Trusted commands".into(),
            if self.verified(|k|matches!(k,ActionKind::TrustedCli{..})){DisplayTier::Verified}else if enabled>0{DisplayTier::Connected}else{DisplayTier::Available},
            if commands.is_empty(){"No commands yet · Choose program… to add one".into()}else{format!("{} commands · Shibubu asks before every run",commands.len())},vec![A::Add]);
        // L4（111 通目・AI providers の群）: 使っている AI（Settings › AI & Models と同じ `ai_rows`・`Check again` = 探し直す）。
        // CONNECTIONS-1101（owner 196 の 5・6・13）: 使っている AI の行の名前はその AI（`account_label`・前は「AI」）。
        // ACCOUNTS-1101（owner 203）: 見つかった AI と足した account の行は、この行の下の `settings::accounts`（AI ごとの塊・Use / Sign in / + Add account / Remove）が持つ
        // （前の `ai:found:*` の行は無くした = 同じ AI の行を 2 つの部品で描かない）
        let active=self.store.ui.active_account();
        let mut rows:Vec<Connection>=self.generated.ai_rows().into_iter().map(|c|Connection{acts:vec![A::CheckAgain],name:account_label(active,&self.store.ui,&self.account_emails).into(),..c}).collect();
        rows.extend([github,gui,web,cli]);
        // PASSWORD-1101 の 1: password manager の行（この Mac に CLI が在る物と、繋いだ物）
        for m in crate::domain::vault::Manager::ALL{
            let on=self.store.ui.vaults.contains(&m);
            if !on&&!self.vault_found.contains(&m){continue}
            let key=m.row();
            rows.push(if on{
                let (state,detail)=checked(&key,DisplayTier::Connected,"Connected · Shibubu reads only items you choose".into());
                row(&key,m.label().into(),state,detail,vec![A::CheckAgain,A::Disconnect])
            }else{
                let how=if m==crate::domain::vault::Manager::OnePassword{"Not connected · Connect signs in with Touch ID"}else{"Not connected · Connect lets Shibubu read items you choose"};
                let (state,detail)=checked(&key,DisplayTier::Available,how.into());
                row(&key,m.label().into(),state,detail,vec![A::Connect])
            });
        }
        let program=|f:&RepoFeature|crate::mcp::command_name(&f.cli_path);
        let what=|f:&RepoFeature|format!("{} · fingerprint {}",if f.cli_args.is_empty(){"No arguments".to_owned()}else{f.cli_args.join(" ")},f.cli_pin.get(..8).unwrap_or(""));
        for f in &commands{
            let key=format!("cli:{}",f.id);
            rows.push(if f.enabled{row(&key,program(f),DisplayTier::Connected,what(f),vec![A::Disconnect,A::Remove])}
                else{row(&key,program(f),DisplayTier::Available,format!("Off · {}",what(f)),vec![A::Connect,A::Remove])});
        }
        // 足す前の確認に出している Trusted command（確認を閉じたら出さない）
        if let Some(f)=self.cli_pending.as_ref().filter(|f|self.shell.connection_confirm.as_deref()==Some(format!("cli:{}",f.id).as_str())){
            rows.push(row(&format!("cli:{}",f.id),program(f),DisplayTier::Available,format!("{} · not added yet",what(f)),vec![]));
        }
        rows.extend(os_grant_rows());
        rows.extend(crate::mcp::connection_rows(&self.mcp_engine,self.mcp_found.as_deref().unwrap_or_default(),&self.conn_checking));
        ConnectionsModel{rows}
    }
    fn disconnected(&self,key:&str)->bool{self.store.ui.disconnected.iter().any(|k|k==key)}
    /// RQ4: Web data の URL（Connections で入れた物・無ければ env = 試験の口）
    fn web_url(&self)->Option<String>{
        self.store.ui.web_data_url.clone().or_else(||env::var("KANGAE_WEB_SERVICE_URL").ok()).filter(|u|github::web_url_ok(u))
    }
    /// RQ4: Connections に入った時: Shibubu の MCP の設定を読み直し、Keychain の GitHub の token の在りかを見る。
    /// L4: 他の app の設定は読まない（`mcp-found` の行の `Look in Claude's settings` を押した時だけ = `("mcp-found",CheckAgain)`）。
    /// RQ-016a: MCP の server は起こさない（入っただけで利用者の program を動かさない・起こすのは行の `Check again`・`Add`・`Connect` を押した時だけ）
    fn enter_connections(&mut self){
        self.mcp_engine=crate::mcp::load_engine_config().unwrap_or_default();
        self.github_keychain=github::keychain_token(&crate::storage::secret_store::SecretStore::from_env()).is_some();
        self.vault_found=crate::domain::vault::Manager::ALL.into_iter().filter(|m|m.find().is_some()).collect();
    }
    /// RQ4: MCP の設定を書いた後: 読み直し、Engine を新しい設定で作り直す（次の run から使う・止めた server は起こさない）
    fn mcp_changed(&mut self){
        self.mcp_engine=crate::mcp::load_engine_config().unwrap_or_default();
        self.generated.set_engine(engine_setup(&self.data_dir));
        sync_connections(&self.store.ui);
    }
    /// RQ4（RQ-016）: server を 1 つ裏で起こして tools/list を読む（前の答えを捨てる・待ちは MCP_CHECK まで）。答えは mcp の覚え（行が時刻ごと読む）と `Event::Checked`（Checking の終わり）
    fn check_mcp(&mut self,key:&str){
        let Some(name)=key.strip_prefix("mcp:") else{return};
        let Some(server)=self.mcp_engine.mcp_servers.get(name).cloned() else{return};
        if !self.conn_checking.insert(key.to_owned()){return}
        crate::mcp::forget_tools(Some(name));
        let (tx,key,name)=(self.tx.clone(),key.to_owned(),name.to_owned());
        thread::spawn(move||{
            let one=crate::mcp::McpConfig{mcp_servers:[(name.clone(),server)].into_iter().collect()}.resolved(&crate::storage::secret_store::SecretStore::from_env());
            let (tier,detail)=match crate::mcp::list_tools_all(&one,MCP_CHECK).remove(&name){
                Some(r)=>crate::mcp::checked_state(&r,Duration::ZERO,None),
                None=>(DisplayTier::Available,"Off".into()),
            };
            let _=tx.send(Event::Checked{key,tier,detail});
        });
    }
    /// RQ4（RQ-015）: GitHub に `GET /rate_limit`（利用枠を使わない）を 1 回。token が在ればそれで
    fn check_github(&mut self){
        if !self.conn_checking.insert("github".into()){return}
        let tx=self.tx.clone();
        thread::spawn(move||{
            let api=github::Api::from_env();
            let (tier,detail)=match api.rate_limit(){
                Ok(_) if api.token.is_some()=>(DisplayTier::Ready,"Token accepted by GitHub".into()),
                Ok(_)=>(DisplayTier::Ready,"GitHub answered · public repos only".into()),
                Err(github::Fail::Status{code:401,..})=>(DisplayTier::Unavailable,"GitHub didn't accept the token · Paste token to add a new one".into()),
                Err(f)=>(DisplayTier::Unavailable,f.message().to_owned()),
            };
            let _=tx.send(Event::Checked{key:"github".into(),tier,detail});
        });
    }
    /// PASSWORD-1101 の 1・3: password manager の行と chat の card の手（`ConnAct` の doc の pm の表）。
    fn vault_act(&mut self,row:&str,act:crate::ui::connections::ConnAct){
        use crate::ui::connections::ConnAct as A;
        let Some(m)=crate::domain::vault::Manager::of_row(row) else{return};
        match act{
            A::Connect|A::CheckAgain|A::Reconnect=>self.connect_vault(m),
            // card の `Not now` = card を閉じるだけ。行の `Disconnect` = 繋いだ物を外す（manager の項目も Keychain の参照も消さない・参照は使わなくなる）
            A::Disconnect if self.shell.vault_ask.as_ref().is_some_and(|(a,..)|*a==m)=>self.shell.vault_ask=None,
            A::Disconnect=>if self.store.ui.vaults.contains(&m){
                self.store.ui.vaults.retain(|v|*v!=m);
                self.conn_result.remove(row);self.conn_at.remove(row);self.mark();
                sync_connections(&self.store.ui);
                self.message=format!("{} disconnected · Shibubu won't read its items · Connect to use them again",m.label());
            },
            A::Add=>self.save_held(m),
            A::Remove=>{self.shell.held=None;self.message="Deleted · Shibubu didn't keep it".into();}
            _=>{}
        }
    }
    /// PASSWORD-1101 の 1: CLI を探し、裏の thread で sign in（1Password = Touch ID）と項目の数。card が開いていれば待ちの 1 行（1Password）と答えを card に。
    fn connect_vault(&mut self,m:crate::domain::vault::Manager){
        use crate::domain::vault::{self,Manager};
        let row=m.row();
        let card=self.shell.vault_ask.as_ref().is_some_and(|(a,..)|*a==m);
        let Some(cli)=m.find() else{
            if card{self.shell.vault_ask=Some((m,false,String::new()));}
            let how=match m{Manager::OnePassword=>vault::OP_INSTALL,Manager::Bitwarden=>vault::BW_INSTALL,Manager::Keychain=>"Shibubu can't reach Keychain"};
            self.conn_result.insert(row,(DisplayTier::Unavailable,format!("{} isn't on this Mac · {how}",m.label())));
            return;
        };
        if !self.conn_checking.insert(row){return}
        if card{self.shell.vault_ask=Some((m,true,if m==Manager::OnePassword{vault::WAIT_TOUCH_ID.into()}else{String::new()}));}
        let tx=self.tx.clone();
        thread::spawn(move||{
            let result=vault::sign_in(m,&cli).and_then(|()|vault::list_items(m,&cli)).map(|items|items.len());
            let _=tx.send(Event::Vault{m,result});
        });
    }
    /// PASSWORD-1101 の 1: sign in の答え。繋げた = `UiSettings::vaults` に足し、card なら閉じて会話に 1 行・繋げない = card と行に理由と次の一手。
    fn vault_connected(&mut self,m:crate::domain::vault::Manager,result:Result<usize,String>){
        let row=m.row();
        self.conn_checking.remove(&row);self.conn_at.insert(row.clone(),crate::google::now() as i64);
        let card=self.shell.vault_ask.as_ref().is_some_and(|(a,..)|*a==m);
        match result{
            Ok(n)=>{
                if !self.store.ui.vaults.contains(&m){self.store.ui.vaults.push(m);self.mark();}
                sync_connections(&self.store.ui);
                self.conn_result.insert(row,(DisplayTier::Ready,format!("Connected · {n} items")));
                if card{
                    self.shell.vault_ask=None;
                    self.say(crate::domain::chat::Who::Shibubu,format!("{} is connected. Shibubu keeps only item names and reads a value only at the moment it uses it.",m.label()));
                }
            }
            Err(why)=>{
                if card{self.shell.vault_ask=Some((m,true,why.clone()));}
                self.conn_result.insert(row,(DisplayTier::Unavailable,why));
            }
        }
    }
    /// PASSWORD-1101 の 3: chat で止めた値を manager へ（裏の thread・値は stdin だけ）。保存できたら会話に名前の 1 行・できなければ値を held に戻す。
    fn save_held(&mut self,m:crate::domain::vault::Manager){
        let Some((value,_))=self.shell.held.take() else{return};
        let Some(cli)=m.find() else{
            self.shell.held=Some((value,m));
            self.message=format!("{} isn't on this Mac · Delete it, or install it and try again",m.label());
            return;
        };
        let name=format!("Password from chat · {}",crate::runtime::now_rfc3339().get(..16).unwrap_or_default().replace('T'," "));
        let tx=self.tx.clone();
        thread::spawn(move||{
            let r=crate::domain::vault::save(m,&cli,&name,&value).map_err(|why|(value,m,why));
            let _=tx.send(Event::VaultSaved(r));
        });
    }
    /// RQ4（RQ-015）: Web data の URL を 1 回 GET（`Submit` = 欄の URL を保存の前に・`CheckAgain` = 保存した URL）
    fn test_web(&mut self,url:String,save:bool){
        if !self.conn_checking.insert("web".into()){return}
        let token=crate::storage::secret_store::SecretStore::from_env().get(&crate::storage::secret_store::connection_key("web","token")).ok().flatten()
            .or_else(||env::var("KANGAE_WEB_SERVICE_TOKEN").ok())
            .filter(|t|crate::domain::vault::SecretRef::parse(t).map_or(true,|r|self.store.ui.vaults.contains(&r.manager)));
        let tx=self.tx.clone();
        thread::spawn(move||{
            let token=token.and_then(crate::domain::vault::resolve); // PASSWORD-1101 の 2: 参照なら使う瞬間に値へ（Touch ID を待つ）
            let r=github::web_read_at(&url,token.as_deref()).map(|_|()).map_err(|f|(match f{github::Fail::Status{code,..}=>code,_=>0},f.message().to_owned()));
            let _=tx.send(if save{Event::WebTested{url,result:r}}else{
                let (tier,detail)=match r{Ok(())=>(DisplayTier::Ready,format!("{} answered",github::url_host(&url))),
                    Err((401|403,_))=>(DisplayTier::Unavailable,"Needs a token · Paste token to add it".into()),Err((_,why))=>(DisplayTier::Unavailable,why)};
                Event::Checked{key:"web".into(),tier,detail}
            });
        });
    }
    /// RQ4: 欄に打った Web data の URL の答え。届けば（鍵が要るだけの 401 / 403 も）保存して欄を閉じる・届かなければ欄に残して理由の 1 行
    fn web_tested(&mut self,url:String,result:Result<(),(u16,String)>){
        self.conn_checking.remove("web");
        let host=github::url_host(&url).to_owned();
        let (tier,detail)=match result{
            Ok(())=>(DisplayTier::Ready,format!("{host} answered")),
            Err((401|403,_))=>(DisplayTier::Unavailable,"Needs a token · Paste token to add it".into()),
            Err((_,why))=>{self.message=format!("{why} · fix the address and press Enter");return}
        };
        self.store.ui.web_data_url=Some(url);self.store.ui.disconnected.retain(|k|k!="web");self.mark();
        if self.shell.connecting.as_deref()==Some("web"){self.shell.end_connecting();}
        self.message=if tier==DisplayTier::Ready{format!("Web data connected · {host}")}else{detail.clone()};
        self.conn_at.insert("web".into(),crate::google::now() as i64);self.conn_result.insert("web".into(),(tier,detail));
        sync_connections(&self.store.ui);
    }
    /// RQ4: 切る（設定と Keychain の秘密は残す）・繋ぎ直す（`UiSettings::disconnected`）
    fn set_disconnected(&mut self,key:&str,off:bool){
        self.store.ui.disconnected.retain(|k|k!=key);
        if off{self.store.ui.disconnected.push(key.to_owned());}
        self.conn_result.remove(key);self.conn_at.remove(key);self.mark();
        sync_connections(&self.store.ui);
    }
    /// RQ4: clipboard を 1 回読み Keychain の `connection_key(row, name)` へ（値は画面・Intent・記録に出さない・末尾 4 文字だけ言う）
    /// CONNECTIONS-1101（owner 196 の 9「paste tokenを押しても何も起きない」）: 値は行の下の鍵の欄（`Shell::field`）から取る（取ったら残さない・画面と log には last4 だけ）。
    /// 前は clipboard を読み、空なら同じ 1 行を出すだけで何も起きない様に見えた。保存できたら欄を閉じる。
    fn paste_secret(&mut self,row:&str,name:&str)->bool{
        let clip=std::mem::take(&mut self.shell.field.0).trim().to_owned();
        if clip.is_empty()||clip.contains(char::is_whitespace){self.message=format!("Paste the whole {name} into the field · it has no spaces");return false}
        let keys=crate::storage::secret_store::SecretStore::from_env();
        match keys.put(&crate::storage::secret_store::connection_key(row,name),&clip){
            Ok(())=>{
                let last4:String=clip.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                self.shell.end_connecting();
                self.message=format!("{name} ····{last4} saved in Keychain");true
            }
            Err(e)=>{self.message=format!("Couldn't save the {name} in Keychain · {e}");false}
        }
    }
    /// PASSWORD-1101 の 2: Paste の欄の下で選んだ項目 = Keychain の `connection_key(row, name)` に値でなく参照（`SecretRef::stored`）を置く。
    /// 値は使う瞬間（`github::Api::from_env`・Web data の読み = 裏の thread）に `vault::resolve`。置けたら欄を閉じる。
    fn use_item(&mut self,row:&str,name:&str,item:&crate::domain::vault::SecretRef)->bool{
        match crate::storage::secret_store::SecretStore::from_env().put(&crate::storage::secret_store::connection_key(row,name),&item.stored()){
            Ok(())=>{self.shell.end_connecting();self.message=format!("Using {} · Shibubu reads it only when it uses it",item.label());true}
            Err(e)=>{self.message=format!("Couldn't keep the choice in Keychain · {e}");false}
        }
    }
    /// RQ4（RQ-015〜RQ-017）: Connections の行の手（`ConnAct` の doc の表）。確かめは裏の thread・結果は行の detail と時刻・失敗は理由と次の一手の 1 行
    fn connection(&mut self,row:String,act:crate::ui::connections::ConnAct){
        use crate::ui::connections::ConnAct as A;
        let keys=crate::storage::secret_store::SecretStore::from_env;
        // CONNECTIONS-1101: Paste token の 1 回目は行の下に鍵の欄を開くだけ。欄の Save / Enter が同じ手をもう一度送る = ここを抜けて下の保存の腕へ（github・web・mcp:<s>）
        if let A::PasteSecret(name)=&act{
            let key=crate::ui::connections::secret_key(&row,name);
            if self.shell.connecting.as_deref()!=Some(key.as_str()){
                self.shell.start_connecting(key);
                // PASSWORD-1101 の 2: GitHub と Web data の欄の下に、繋いだ password manager の項目の名前（裏の thread・値は読まない）
                if crate::ui::connections::takes_items(&row){
                    for m in self.store.ui.vaults.clone(){
                        let Some(cli)=m.find() else{continue};
                        let tx=self.tx.clone();
                        thread::spawn(move||{if let Ok(items)=crate::domain::vault::list_items(m,&cli){let _=tx.send(Event::VaultItems(items));}});
                    }
                }
                return
            }
        }
        match (row.as_str(),act){
            ("gui",A::CheckAgain)=>self.probe_gui(),
            // OPEN-INPLACE-1101 の 2: Always allow を外す（同じ app の同じ種類の手は次からまた承認を聞く・app ごとの同意は残す）
            ("gui",A::AllowTool{tool,allowed:false})=>{
                let path=crate::surface::act_consent_path();
                let Some(a)=crate::surface::always_allowed(&path).into_iter().find(|a|a.label()==tool) else{return};
                self.message=match crate::surface::revoke_always(&path,&a.bundle_id,&a.action){Ok(_)=>format!("{tool} removed · Shibubu will ask before it does this again"),Err(why)=>why};
            }
            ("github",A::CheckAgain)=>self.check_github(),
            ("github",A::Reconnect)=>{self.set_disconnected("github",false);self.check_github();}
            ("github",A::Disconnect)=>{self.set_disconnected("github",true);self.message="GitHub disconnected · your token stays in Keychain · Reconnect to use it again".into();}
            // PASSWORD-1101 の 2: 欄に貼った値か、欄の下で選んだ password manager の項目（値でなく参照・`use_item`）。どちらも置けたら読み直す
            (k@("github"|"web"),act@(A::PasteSecret(_)|A::UseItem{..}))=>{
                let kept=match act{A::UseItem{name,item}=>self.use_item(k,&name,&item),A::PasteSecret(name)=>self.paste_secret(k,&name),_=>false};
                if kept{
                    self.github_keychain|=k=="github";sync_connections(&self.store.ui);
                    if k=="github"{self.check_github();}else if let Some(u)=self.web_url(){self.test_web(u,false);}
                }
            }
            ("web",A::Connect)=>self.shell.start_connecting("web".into()),
            ("web",A::Submit|A::Test)=>{
                let url=self.shell.field.0.trim().to_owned();
                if !github::web_url_ok(&url){self.message="Use an https:// address (or one on this Mac) without a user name or #".into();return}
                self.test_web(url,true);
            }
            ("web",A::CheckAgain)=>if let Some(u)=self.web_url(){self.test_web(u,false);},
            ("web",A::Reconnect)=>{self.set_disconnected("web",false);if let Some(u)=self.web_url(){self.test_web(u,false);}}
            ("web",A::Disconnect)=>{self.set_disconnected("web",true);self.message="Web data disconnected · the address stays · Reconnect to use it again".into();}
            ("web",A::Remove)=>{
                self.shell.connection_confirm=None;
                let _=keys().delete(&crate::storage::secret_store::connection_key("web","token"));
                self.store.ui.web_data_url=None;self.set_disconnected("web",false);
                self.message="Web data removed".into();
            }
            ("mcp",A::Add)=>self.shell.start_connecting("mcp".into()),
            // L4: 利用者が頼んだ時だけ他の app の設定を読む（Shibubu に無い server が `mcp-found:` の行になる・元の app の名前つき）
            ("mcp-found",A::CheckAgain)=>{
                let found=std::env::var_os("HOME").map(|h|crate::mcp::found_config(std::path::Path::new(&h))).unwrap_or_default();
                self.message=crate::mcp::found_line(&self.mcp_engine,&found); // 足せない物（URL の server・読めない file）を「全部 Shibubu に在る」に混ぜない
                self.mcp_found=Some(found);
            }
            // CONNECTIONS-1101（owner 196 の 10）: 行の下の form（名前は任意・command か JSON）
            ("mcp",A::Submit)=>{
                let (named,text)=(self.shell.field_name.clone(),self.shell.field.0.clone());
                let added=crate::mcp::engine_config_for_edit().and_then(|mut cfg|{let name=cfg.add_named(&named,&text)?;crate::mcp::save_engine_config(&cfg)?;Ok(name)});
                match added{
                    Ok(name)=>{self.shell.end_connecting();self.message=format!("Added {name} · checking it");self.mcp_changed();self.check_mcp(&format!("mcp:{name}"));}
                    Err(why)=>self.message=why,
                }
            }
            (k,A::Connect) if k.starts_with("mcp-found:")=>{
                let name=&k["mcp-found:".len()..];
                let Some(server)=self.mcp_found.iter().flatten().find_map(|(_,c,_)|c.mcp_servers.get(name)).cloned() else{return};
                let saved=crate::mcp::engine_config_for_edit().and_then(|mut cfg|{cfg.mcp_servers.entry(name.to_owned()).or_insert(server);crate::mcp::save_engine_config(&cfg)});
                match saved{
                    Ok(_)=>{self.message=format!("Added {name} to Shibubu · checking it");self.mcp_changed();self.check_mcp(&format!("mcp:{name}"));}
                    Err(why)=>self.message=why,
                }
            }
            (k,act) if k.starts_with("mcp:")=>self.mcp_act(k,act),
            (k,act) if k.starts_with("pm:")=>self.vault_act(k,act),
            ("cli",A::Add)=>choose_program(self.tx.clone()),
            (k,A::Submit) if k.starts_with("cli:")=>{
                let Some((id,path))=self.cli_draft.clone().filter(|(id,_)|k==format!("cli:{id}")) else{return};
                match repos::trusted_command(id,&path,crate::mcp::words(&self.shell.field.0)){
                    Ok(f)=>{self.shell.end_connecting();self.cli_pending=Some(f);self.shell.connection_confirm=Some(row.clone());self.cli_draft=None;}
                    Err(why)=>self.message=why,
                }
            }
            (k,act) if k.starts_with("cli:")=>{
                let Ok(id)=k["cli:".len()..].parse::<u64>() else{return};
                self.shell.connection_confirm=None;
                match act{
                    A::Connect if self.cli_pending.as_ref().is_some_and(|f|f.id==id)=>{
                        let f=self.cli_pending.take().expect("checked");
                        self.message=format!("Added {} · Shibubu asks before every run",crate::mcp::command_name(&f.cli_path));
                        self.store.features.push(f);
                    }
                    A::Connect|A::Disconnect=>if let Some(f)=self.store.features.iter_mut().find(|f|f.id==id){
                        f.enabled=act==A::Connect;
                        self.message=format!("{} {}",crate::mcp::command_name(&f.cli_path),if f.enabled{"turned on"}else{"turned off · Connect to use it again"});
                    },
                    A::Remove=>{self.store.features.retain(|f|f.id!=id);self.message="Command removed".into();}
                    _=>return,
                }
                self.mark();
            }
            _=>{}
        }
    }
    /// RQ4（RQ-016）: Shibubu の mcp.json の server 1 つへの手（止める・戻す・消す・確かめ直す・秘密の env・tool の許可）。書いたら Engine を作り直す
    fn mcp_act(&mut self,key:&str,act:crate::ui::connections::ConnAct){
        use crate::ui::connections::ConnAct as A;
        let name=key["mcp:".len()..].to_owned();
        if act==A::CheckAgain{self.check_mcp(key);return}
        let Ok(mut cfg)=crate::mcp::engine_config_for_edit().map_err(|why|self.message=why) else{return};
        let mut recheck=false;
        let done:Result<String,String>=match act{
            A::Disconnect=>cfg.set_disabled(&name,true).map(|_|format!("{name} turned off · Connect to use it again")),
            A::Connect=>{recheck=true;cfg.set_disabled(&name,false).map(|_|format!("{name} turned on · checking it"))}
            A::Remove=>{
                self.shell.connection_confirm=None;
                cfg.remove(&name).map(|s|{
                    let keys=crate::storage::secret_store::SecretStore::from_env();
                    for var in &s.secret_env{let _=keys.delete(&crate::storage::secret_store::connection_key(key,var));}
                    crate::mcp::forget_tools(Some(&name));
                    format!("{name} removed")
                })
            }
            A::PasteSecret(var)=>{
                if !self.paste_secret(key,&var){return}
                recheck=true;
                cfg.mcp_servers.get_mut(&name).map(|s|{s.env.remove(&var);if !s.secret_env.contains(&var){s.secret_env.push(var.clone());}}).ok_or_else(||"That MCP server isn't in Shibubu's list".to_owned()).map(|_|self.message.clone())
            }
            A::AllowTool{tool,allowed}=>{
                let known:Vec<String>=self.mcp_engine.mcp_servers.get(&name).and_then(|s|crate::mcp::known_tools(&name,s)).and_then(Result::ok).unwrap_or_default().into_iter().map(|(t,_)|t.name).collect();
                cfg.allow_tool(&name,&tool,allowed,&known).map(|_|format!("{tool} {} for the AI",if allowed{"allowed"}else{"turned off"}))
            }
            _=>return,
        };
        match done.and_then(|line|crate::mcp::save_engine_config(&cfg).map(|_|line)){
            Ok(line)=>{self.message=line;self.mcp_changed();if recheck{self.check_mcp(key);}}
            Err(why)=>self.message=why,
        }
    }
    /// RQ5（RQ-020）: 今の Page の app を agent に読ませる。(Project・Page・版) が変わった時だけ読み直して候補を作り直す（毎 frame は時間の進みだけ）。
    /// 版は Project の revision なので、編集・Undo・提案の Apply の後は古い提案が退く。Project の外では何も持たない。
    /// CHAT-011 反証 B28: 今の Page の表と一覧の件数（`Generated::agent_rows`）も agent に渡す = 読み直した時（Project を開いた・Page を移った）と、
    /// Page の data で件数が替わった時に observe → refresh（件数の提案「Add search to …」・空の一覧の Add が製品の道で立つ）。
    fn sync_agent(&mut self){
        let now=crate::agent::now_rfc3339();
        let read=match &self.shell.view{View::Project{project,..}=>Some(self.generated.agent_app(project).map(|(rev,page)|((project.clone(),page,rev.version()),rev))),_=>None};
        let (key,rev)=match read{
            Some(Ok(r))=>r,
            // 読めない Page（面・Terminal）と Project の外は提案を持たない（L1-NORAIL: 0 件の Chat の列は何も出さない = 理由の行も出さない）
            _=>{
                if self.agent_key.take().is_some(){self.agent.clear_app();}
                return;
            }
        };
        let rows=self.generated.agent_rows(&key.0);
        let new=self.agent_key.as_ref()!=Some(&key);
        if new{self.agent.set_app(rev);self.agent_key=Some(key);}
        if new||rows!=self.agent_seen_rows{
            self.agent.observe(crate::agent::Observed{rows:rows.clone(),..Default::default()});
            self.agent.refresh(&now);self.agent_seen_rows=rows;
        }
        self.agent.tick(&now);
        // 見える提案だけ shown にする（Chat の tab を開いている間 = 列の頭に MAX_SHOWN 件・Terminal の tab を開いていて Engine の card が無い間 = 欄の上に 1 件〈CHAT-011 反証 Y1〉）
        if let View::Project{project,..}=self.shell.view.clone(){
            let (open,chat)={let p=self.generated.panel(&project);(p.open,p.chat)};
            let n=if !open{0}else if chat{crate::agent::MAX_SHOWN}else if self.generated.engine_view(&project).and_then(|v|ui::engine_view::card_key(&project,v)).is_none(){1}else{0};
            if n>0{self.agent.mark_shown(n);}
        }
    }
    /// RQ5（RQ-021）: 提案の手。Preview / Close / Later / Hide は agent の中だけ（Page を書かない）。Apply は agent の検査（根拠・権限・承認・再検査）を通った後に
    /// `Generated::apply_suggestion` で Project の 1 revision として書く（⌘Z で戻る）。Undo は agent の記録を戻してから Project の Undo（`PageIntent::Undo`）。
    fn proposal(&mut self,ctx:&egui::Context,id:String,act:ui::agent_rail::ProposalAct){
        use crate::agent::Acted;
        let View::Project{project,..}=self.shell.view.clone() else{return};
        let Some((_,page,_))=self.agent_key.clone() else{return};
        let ops=self.agent.app().and_then(|a|self.agent.queue().get(&a.definition().id,&id)).map(|t|t.suggestion.change.operations.clone()).unwrap_or_default();
        let now=crate::agent::now_rfc3339();
        self.shell.chat_note=None;
        match self.agent.act(&id,act.to_act(),&now){
            Ok(Acted::Applied(a))=>match self.generated.apply_suggestion(&project,&page,&ops){
                Ok(v)=>{
                    // agent は適用後の revision を持っている（版 = Project の新しい revision）ので読み直さない（Undo の照合に使う）
                    self.agent_key=Some((project,page,a.revision.version()));
                    self.shell.go(v);self.shell.chat_note=Some(format!("Applied: {} · Undo is under Recently applied",a.title));
                }
                Err(why)=>{self.agent_key=None;self.shell.chat_note=Some(why);}
            },
            Ok(Acted::Reverted{..})=>{self.apply(ctx,Intent::Page(PageIntent::Undo));self.agent_key=None;self.shell.chat_note=Some("Undone".into());}
            Ok(Acted::Snoozed{..}|Acted::Hidden)=>{
                self.store.ui.agent_feedback=self.agent.feedback().to_json();self.mark();
                self.shell.chat_note=Some(if act==ui::agent_rail::ProposalAct::Hide{"Hidden · it won't come back".into()}else{"Later · it comes back tomorrow".into()});
            }
            Ok(_)=>{}
            Err(e)=>self.shell.chat_note=Some(e.to_string()),
        }
    }
    fn cancel_connection_confirm(&mut self)->bool{self.cli_pending=None;self.shell.connection_confirm.take().is_some()}
    fn apply(&mut self,ctx:&egui::Context,intent:Intent){
        match intent{
            Intent::Go(view)=>{
                if let View::CompatCanvas(id)=&view{
                    if self.store.selected!=*id && self.store.sessions.iter().any(|s|s.id==*id){
                        self.store.selected=*id;self.selected_panel=None;self.windows.clear();self.controls.clear();self.mark();
                    }
                }
                if matches!(view,View::Connections|View::Settings(SettingsSection::About)) && self.gui_status.is_none(){self.probe_gui();}
                if view==View::Connections && self.github_status.is_none(){self.probe_github();}
                if view==View::Connections{self.enter_connections();}
                self.go(view);
            }
            Intent::NewApp=>self.new_blank_session(),
            // PASSWORD-1101 の 3: password らしい一言は AI へ送らず・会話に足さず・保存せず held へ（card が Save / Delete を聞く）
            Intent::Talk(text) if crate::domain::chat::looks_like_password(&text)=>self.hold_password(text),
            Intent::Generate(text,attach) if crate::domain::chat::looks_like_password(&text)=>{self.shell.attach=attach;self.hold_password(text)}
            // PASSWORD-1101 の 1: 繋いでいない password manager の名指し = 一言を会話に置き、AI でなく繋ぐ card が答える
            Intent::Talk(text) if crate::domain::vault::Manager::asked(&text).is_some_and(|m|!self.store.ui.vaults.contains(&m))=>{
                let m=crate::domain::vault::Manager::asked(&text).expect("guard");
                self.shell.vault_ask=Some((m,m.find().is_some(),String::new()));
                self.say(crate::domain::chat::Who::You,text);
                self.reveal_card();
            }
            Intent::Talk(text)=>self.talk(text),
            Intent::CheckItem(turn,item)=>if let Some(t)=self.shell.talk.get_mut(turn){if let Some(s)=crate::ui::markdown::toggle(&t.text,item){t.text=s;}},
            i@(Intent::PreviewPlan|Intent::ChangePlan|Intent::LaterPlan|Intent::AcceptPlan)=>self.plan_step(i),
            // SESSIONS-LEFT-1101: 左の `New chat` はどの View からも = 今の Home の会話を閉じて Home の欄へ（前の会話は左の Sessions に残る）
            Intent::NewChat=>{self.new_chat();self.go(View::Home);}
            Intent::Generate(text,attach)=>match crate::project::discover::discover(&text){
                // PV1: 迷いの一言（作る + game・app を名指ししない）は作らずに欄の下に成果物の候補（何も入れない・欄の文と添えた物は戻す）
                Some(d) if self.shell.view==View::Home=>{self.back_to_home_field(text);self.shell.attach=attach;self.shell.discovery=Some(d);}
                _=>{self.shell.discovery=None;self.create_app(text,attach)}
            },
            // PV1: Check は表の 1 つの .app だけ読む（/Applications・plutil 2 回 = 最長 10 秒）
            // ponytail: UI の thread で同期に読む。遅いと分かったら poll_installs と同じ裏の thread へ
            Intent::Discover(a)=>if let Some(d)=self.shell.discovery.as_mut(){
                if !crate::project::discover::act(d,a,&crate::project::acquire::Homebrew::find().apps_dir){self.shell.discovery=None;}
            },
            // FX2: Home の生成の 1 行で押した手（契約は `Intent::Build` の doc）
            Intent::Build(a)=>self.build_step(ctx,a),
            // ADD-FILE（HOME-ATTACH-1101 追記 3・owner 198 の 7）: Shibubu の process の中の開く panel（前の osascript の JXA は窓の後ろに開き、押しても何も起きなかった）。
            // panel が閉じるまで frame は止まる（modal）。選んだ物は今の添付の道・開けなければ窓の下の帯に理由の 1 行
            Intent::Attach=>match crate::open_panel("Add files or photos","Add"){
                Ok(paths)=>if !paths.is_empty(){self.add_attachments(paths);},
                Err(why)=>self.message=format!("The file picker couldn't open · {why}"),
            },
            Intent::SetEffort(e)=>{
                self.store.ui.default_effort=e.map(|e|e.flag().to_owned());
                (self.generated.effort,self.shell.effort)=(e,e);
                self.mark();
            }
            Intent::SetModel(m)=>{
                self.store.ui.default_model=m.clone();
                (self.generated.model,self.shell.model)=(m.clone(),m);
                self.mark();
            }
            // L1-NORAIL: ⇧⌘P・⌘K の Show chat = 下の panel の Chat を選んで開き、欄に focus（Project の外は Home の欄が会話）
            Intent::OpenChat=>if let View::Project{project,..}=self.shell.view.clone(){
                if let Err(e)=self.show_chat(&project){self.message=e;}
                self.shell.focus_command=true;
            },
            // CHAT-SIDE-1101: panel の置き場（保存・全 Project 共通）。追記 219〜221: panel ごと移す（選んでいる tab はそのまま）・移した先で開き、Chat の席なら chat bar の欄に focus
            // PANEL-SYNC-1101: 下から開く（窓の一番下の帯の畳んだ tab・畳んだ間の ⌃`）= 置き場を下に（右の sidebar は同じ panel なので畳まれる）・今の場所の panel に手
            Intent::OpenBelow(act)=>{
                if self.store.ui.chat_place!=ChatPlace::Bottom{self.store.ui.chat_place=ChatPlace::Bottom;self.mark();}
                match self.shell.view.clone(){
                    View::Project{project,..}=>{
                        self.apply(ctx,Intent::Page(PageIntent::TerminalPanel(act)));
                        let p=self.generated.panel(&project);
                        if p.chat&&p.open{self.shell.focus_command=true;}
                    }
                    _=>if let Err(e)=self.generated.panel_act(crate::generated::HOME_PLACE,act){self.message=e;},
                }
            }
            Intent::ChatPlace(p)=>{
                self.store.ui.chat_place=p;self.mark();
                if let View::Project{project,..}=self.shell.view.clone(){
                    let _=self.generated.panel_act(&project,PanelAct::Reveal);
                    self.shell.focus_command=self.generated.panel(&project).chat;
                }
            }
            // UI-4b: 作り手 3 つ = title line の左端の button・⌃⌘S（keyboard の頭）・⌘K の Hide sidebar / Show sidebar。覚える値は store の 1 欄だけ
            Intent::ToggleSidebar=>{
                let open=!self.shell.sidebar_open;
                self.store.ui.sidebar_hidden=Some(!open);self.shell.sidebar_open=open;self.mark();
                ctx.request_repaint(); // 幅の 180ms を次の frame から寄せる（保存の debounce を待たない）
            }
            Intent::RecheckGui=>self.probe_gui(),
            Intent::RecheckGithub=>self.probe_github(),
            Intent::RecheckAi=>{self.llm_rechecking=Some("Checking for AI on this Mac");self.start_llm_detect(ctx,false);}
            Intent::AiStep(step)=>if step==crate::llm_cli::INSTALL_STEP{
                if let Err(e)=std::process::Command::new("/usr/bin/open").arg(crate::llm_cli::INSTALL_URL).spawn(){self.message=format!("Couldn't open the install page ({})",e.kind());}
            }else if self.ai_sign_in.is_some(){self.message="Finish the sign-in open in your browser first".into();
            }else{self.llm_rechecking=Some("Finish signing in in your browser");self.start_llm_detect(ctx,true);},
            Intent::CloneApp=>self.clone_session(),
            Intent::DeleteApp=>self.delete_session_prompt=true,
            Intent::SetTheme(pref)=>{
                ctx.set_theme(pref);
                self.store.ui.theme=match pref{egui::ThemePreference::Dark=>ThemeChoice::Dark,egui::ThemePreference::Light=>ThemeChoice::Light,_=>ThemeChoice::System};
                self.store.ui.theme_chosen=true; // LOOK-1101: 選んだ値は次の起動でも既定（Dark）に戻さない
                self.mark();
            }
            // FX4（FX-016）: ⌘K の最近の行を覚える / 覚えない（false は覚えた行も消す・保存は `snapshot_drafts`）
            Intent::RememberCommands(on)=>{self.shell.palette.recent=on.then(Vec::new);self.mark();}
            Intent::CopyUsageReport=>{ctx.copy_text(self.generated.usage_report());self.message="Usage report copied".into();}
            // FX1: 保存できなければ閉じずに同じ場所の 1 行（Quit anyway / Keep open）。Ok か Quit anyway（保存は試す・失敗は捨てる = 利用者が選んだ）は自分で閉じる
            // （⌘Q は close 要求を経ないので Close を送る・その Close が返す次の close 要求は `quitting` で聞き直さない）
            Intent::Quit{anyway}=>match self.save_now(){
                Err(why) if !anyway=>{ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);self.quit_failed=Some(why);}
                _=>{self.quit_failed=None;self.quitting=true;ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
            },
            // RQ1: Settings の form が store.ui を直に書いた後の 1 回（reduce motion・text size・Execution limit を当て直して保存）
            Intent::SettingsChanged=>{
                self.apply_settings(ctx);self.mark();
                // RQ3（RQ-014）: 保存期間を短くしたらその場で消す（Settings › Data & Recovery の大きさも読み直す）
                if let Err(e)=self.generated.prune_records(crate::google::now() as i64,self.store.ui.history_days){eprintln!("shibubu: keep records: {e}");}
                self.refresh_data(true);
            }
            // RQ2: Project の管理と AI の account・使用量（作り手 = Home の行・header の menu・Settings › AI & Models・⌘K。どれも本体の片で描くので、interface の段は届かない腕）
            // RQ2（RQ-005）: Project の管理（作り手 = Home の行の •••・header の {name} ⌄・1 つの欄の名前の欄）
            Intent::RenameProject{project,name}=>match self.generated.rename_project(&project,&name){
                Ok(())=>{self.shell.end_rename();self.message="Project renamed".into();}
                Err(why)=>self.message=why, // 欄に残す（直して Enter・Esc で取り消し）
            },
            Intent::DuplicateProject(project)=>match self.generated.duplicate_project(&project){
                Ok(v)=>{self.message="Project duplicated".into();self.go(v);}
                Err(why)=>self.message=why,
            },
            Intent::DeleteProject(project)=>{
                self.shell.project_confirm=None;
                match self.generated.delete_project(&project){
                    Ok(())=>{
                        let ui=&mut self.store.ui;
                        ui.pinned_projects.retain(|p|*p!=project);ui.archived_projects.retain(|p|*p!=project);ui.sample_projects.retain(|p|*p!=project);
                        ui.opened_projects.remove(&project);ui.drafts.remove(&project);ui.open_projects.retain(|p|*p!=project);
                        // SESSIONS-LEFT-1101: 消した Project の session も消す（`Delete…` の確認が「history is deleted」と言う）
                        let keys:Vec<String>=self.generated.chat_list().iter().filter(|(k,_,_)|crate::domain::chat::session_project(k)==Some(project.as_str())).map(|(k,_,_)|k.clone()).collect();
                        for k in keys{self.generated.forget_chat(&k);}
                        if ui.last_project.as_deref()==Some(project.as_str()){ui.last_project=None;}
                        self.shell.drafts.remove(&project);
                        // 消した Project の名前の欄を開いたままにしない（Home の ••• → Rename の後に Delete… → Delete・反証の直し）
                        if self.shell.renaming.as_deref()==Some(project.as_str()){self.shell.end_rename();}
                        // 消した Project の画面に居たら Home へ（消した Project の打ちかけは預けない = generated.route を通さず Shell だけ移す）
                        if matches!(&self.shell.view,View::Project{project:p,..} if *p==project){self.shell.command.clear();self.shell.go(View::Home);}
                        self.message="Project deleted · its folder and your files stay".into();self.mark();
                    }
                    Err(why)=>self.message=why,
                }
            }
            Intent::ArchiveProject{project,archived}=>{
                let list=&mut self.store.ui.archived_projects;
                list.retain(|p|*p!=project);
                if archived{list.push(project);self.message="Project archived · find it with Show archived in the ••• next to Your projects, or with ⌘K".into();}else{self.message="Project restored to Your projects".into();}
                self.mark();
            }
            Intent::PinProject{project,pinned}=>{
                let list=&mut self.store.ui.pinned_projects;
                list.retain(|p|*p!=project);
                if pinned{list.push(project);}
                self.mark();
            }
            Intent::FoldProject{project,open}=>{
                let list=&mut self.store.ui.open_projects;
                list.retain(|p|*p!=project);
                if open{list.push(project);}
                self.mark();
            }
            // SESSIONS-LEFT-1101 追記 244: 見出しの ⌄（覚えるのは畳んだ見出し）
            Intent::FoldHeading{heading,open}=>{
                let list=&mut self.store.ui.folded_headings;
                list.retain(|h|*h!=heading);
                if !open{list.push(heading);}
                self.mark();
            }
            // SESSIONS-LEFT-1101 追記 244: Project の行の ••• の Rename = その Project へ移ってから名前の欄（go が欄を閉じるので移った後に立てる）
            Intent::StartRename{project,name}=>{
                if !matches!(&self.shell.view,View::Project{project:now,..} if *now==project){self.go(View::Project{project:project.clone(),page:String::new()});}
                if matches!(&self.shell.view,View::Project{project:now,..} if *now==project){self.shell.start_rename(project,name);}
            }
            Intent::NewSession(project)=>{let key=self.generated.new_session(&project);self.open_chat(key);}
            Intent::OpenSession(key)=>self.open_chat(key),
            // SESSIONS-LEFT-1101 追記 224: 左の sidebar の Terminal の行 = その Project を開き、その tab を選んで開く（置き場は替えない）
            Intent::OpenTerminal{project,tab}=>{
                if !matches!(&self.shell.view,View::Project{project:now,..} if *now==project){self.go(View::Project{project:project.clone(),page:String::new()});}
                if matches!(&self.shell.view,View::Project{project:now,..} if *now==project){
                    if let Err(e)=self.generated.panel_act(&project,PanelAct::Select(tab)){self.message=e;}
                }
            }
            // RQ2（RQ-009）: model を呼ばず名前の表で作る（外の app と file に書かない例だけ）
            Intent::TrySample(i)=>{
                let Some((_,said))=crate::ui::home::EXAMPLES.get(i).filter(|_|i>0) else{return};
                match self.generated.create_project(said,None,None){
                    Ok(Some(v))=>{
                        if let View::Project{project,..}=&v{self.store.ui.sample_projects.push(project.clone());self.mark();}
                        self.go(v);
                    }
                    Ok(None)=>match self.generated.create_in_chat(said){
                        Ok(id)=>{self.store.ui.sample_projects.push(format!("project.{id}"));self.mark();self.go(View::App(id));}
                        Err(why)=>self.message=why,
                    },
                    Err(why)=>self.message=why,
                }
            }
            // SIGNIN-1101（owner 193）: sign in の印の行は確かめない（確かめても落ちて前の AI に戻るだけだった）= 下の terminal で sign in を始め、終われば poll_cli_sign_in が切り替える
            Intent::UseAiAccount(Some(acct@AiAccount::Found{needs_sign_in:true,..}))=>self.start_cli_sign_in(ctx,acct),
            // ACCOUNTS-1101: login の file がまだの足した CLI の account も同じ（確かめても落ちるだけ）
            Intent::UseAiAccount(Some(acct)) if crate::llm_cli::cli_account_signed_in(&self.data_dir,&acct)==Some(false)=>self.start_cli_sign_in(ctx,acct),
            Intent::UseAiAccount(acct)=>self.switch_ai(ctx,acct),
            // RQ-ACCT: browser の sign in を裏で（G99b の sign_in にその account の CLAUDE_CONFIG_DIR）
            Intent::AddAiAccount(p) if p=="claude"=>self.start_account_sign_in(ctx,None),
            // ACCOUNTS-1101: 置き場を作って列に足し（選ぶのは sign in の後の確かめ）、下の terminal をその置き場で開く。途中で閉じても行は残る（Needs sign-in と Sign in / Remove）
            Intent::AddAiAccount(p)=>match crate::llm_cli::add_cli_account(&self.data_dir,&p,&self.store.ui.ai_accounts){
                Ok(acct)=>{self.store.ui.ai_accounts.push(acct.clone());self.mark();self.start_cli_sign_in(ctx,acct);}
                Err(why)=>self.message=why,
            },
            // G84（operator 10-29）: 押した行の account そのもの（種類と id の組）だけ。id だけで引くと手で書いた設定の重なった別の account をし直す / 両方消す
            Intent::ReauthAiAccount(acct)=>if let Some(a)=self.store.ui.ai_accounts.iter().find(|a|**a==acct).cloned(){self.start_account_sign_in(ctx,Some(a));},
            Intent::RemoveAiAccount(acct)=>{
                if !self.store.ui.ai_accounts.iter().any(|a|*a==acct){return}
                let was_active=self.store.ui.active_account()==Some(&acct);
                // REVIEW-RQN（operator 1）: その account の sign in（Re-authenticate）が走っていれば止めて答えを捨てる（消した account に「Signed in again」を言わない）
                if matches!(acct,AiAccount::Plan{..})&&self.ai_sign_in.as_ref().is_some_and(|(r,_)|r.as_deref()==Some(acct.id())){
                    self.sign_in_stop.store(true,std::sync::atomic::Ordering::SeqCst);self.ai_sign_in=None;
                }
                // ACCOUNTS-1101: その account の下の terminal の sign in を見張っていればやめて、その tab を止める（置き場を消す account の sign in を続けない）= 消した account に切り替えない
                if let Some(s)=self.cli_sign_in.take_if(|s|s.acct==acct){let _=self.generated.panel_act(&s.project,PanelAct::ConfirmClose{tab:s.tab});}
                let name=match &acct{
                    AiAccount::Plan{..}=>account_label(Some(&acct),&self.store.ui,&self.account_emails),
                    AiAccount::Key{last4,..}=>format!("API key ····{last4}"),
                    AiAccount::EnvKey=>"Key from environment".into(),
                    AiAccount::Found{name,..}=>name.clone(),
                };
                match self.forget_account(&acct){
                    Err(why)=>self.message=why,
                    Ok(())=>{
                        self.store.ui.ai_accounts.retain(|a|*a!=acct);self.mark();
                        // ACCOUNTS-1101 2 段目: 付けた名前も外す（同じ key の次の account に古い名前が付かない）
                        self.store.ui.account_names.remove(&acct.key());
                        self.message=format!("Removed {name}");
                        // 使っていた account なら System default に戻して確かめ直す。その account への切り替えを確かめている途中なら、その答えを捨てる
                        // （受け取ると消した account の CLI が入り、全部の呼び出しが「鍵が無い」で落ちる・反証の直し）
                        if was_active{
                            self.store.ui.set_active_account(None);self.llm_switch=None;
                            self.llm_rx=None;self.start_llm_detect(ctx,false);
                        }
                    }
                }
            }
            // ACCOUNTS-1101 2 段目（作る物 5）: 足した account の名前（空 = 既定の名前に戻す）
            Intent::RenameAiAccount(acct,name)=>if self.store.ui.set_account_name(&acct,&name){self.mark();},
            // RQ-API: clipboard を 1 回読む（値は Intent・画面・記録に出さない・末尾 4 文字だけ残す）
            Intent::PasteApiKey=>{
                let clip=std::process::Command::new("/usr/bin/pbpaste").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().map(|o|String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
                match crate::llm_cli::add_key_account(&clip,&crate::storage::secret_store::SecretStore::from_env(),&self.store.ui.ai_accounts){
                    Ok(acct)=>{
                        let last4=match &acct{crate::model::AiAccount::Key{last4,..}=>last4.as_str(),_=>""};
                        self.message=format!("API key ····{last4} added · press Use to switch to it · billed per request by Anthropic");
                        self.store.ui.ai_accounts.push(acct);self.mark();
                    }
                    Err(why)=>self.message=why,
                }
            }
            Intent::UseEnvKey=>{
                if crate::llm_cli::env_key_last4().is_none(){self.message="There's no API key in the environment Shibubu was opened from".into();return;}
                if !self.store.ui.ai_accounts.contains(&crate::model::AiAccount::EnvKey){self.store.ui.ai_accounts.push(crate::model::AiAccount::EnvKey);self.mark();}
                self.message="Key from environment added · press Use to switch to it · billed per request by Anthropic".into();
            }
            Intent::Notice(line)=>self.message=line,
            // RQ-USAGE: 帯の `Show usage`・`Try again` と Settings の `Refresh`（押した = 使用量のために動いた・60 秒を待たない）
            Intent::RefreshUsage=>{
                self.usage_asked=true;
                let others:Vec<AiAccount>=self.shell.providers.iter().filter_map(|(a,..)|a.clone()).collect();
                if self.read_usage&&self.generated.refresh_usage(crate::google::now() as i64,true,&others){ctx.request_repaint();}
            }
            // USAGE-OWN-1101: panel の `Recount`（Keychain は読まない・数えている最中なら 2 本目を起こさない）
            Intent::RecountOwnUsage=>{
                let plan=match self.generated.plan_usage(){Some(Ok(u))=>Some(u),_=>None};
                let c=ctx.clone();
                if self.own.refresh(&self.data_dir,plan,crate::google::now() as i64,true,move||c.request_repaint()){ctx.request_repaint();}
            }
            Intent::CopyDiagnostics=>{
                let rows=self.generated.diagnostics(&self.connections_model().rows);
                ctx.copy_text(rows.iter().map(|(k,v)|format!("{k}: {v}")).collect::<Vec<_>>().join("\n"));
                self.message="Diagnostics copied".into();
            }
            // RQ3: Data & Recovery（作り手 = Settings › Data & Recovery・header の {name} ⌄・Home の行の •••・Home の壊れた時の 1 行の Restore…）
            Intent::RevealInFinder(path)=>if let Err(why)=crate::generated::run_open("/usr/bin/open",&["-R",&path.to_string_lossy()]){self.message=why;},
            Intent::BackUpNow=>{
                // 世代に今の設定を入れる（保存待ちの debounce を待たない）
                self.snapshot_drafts();
                if let Err(e)=self.store.save(&self.data_path){self.message=format!("Couldn't back up (settings: {e}) · Try again");return;}
                self.dirty=false;
                self.message=match self.generated.back_up_now(){Ok(_)=>"Backed up · it's at the top of Backups".into(),Err(why)=>why};
                self.refresh_data(true);
            }
            Intent::RestoreBackup(id)=>{
                self.message=self.generated.restore_backup(id.as_deref()).unwrap_or_else(|why|why);
                self.refresh_data(true);
            }
            Intent::ExportProject(project)=>match self.user_dirs.first(){
                None=>self.message="Couldn't find your Downloads folder".into(),
                Some(dest)=>self.message=match self.generated.export_project(&project,dest){
                    Ok(file)=>format!("Exported {} to Downloads · it leaves out keys, sign-ins and the apps it uses",file.file_name().map(|n|n.to_string_lossy()).unwrap_or_default()),
                    Err(why)=>why,
                },
            },
            Intent::ImportProject(file)=>match self.generated.import_project(&file){
                Ok(v)=>{self.message="Project imported".into();self.go(v);}
                Err(why)=>self.message=why,
            },
            Intent::RevokeConsent(bundle)=>match crate::surface::revoke_act_consent(&crate::surface::act_consent_path(),&bundle){
                Ok(true)=>{
                    self.message=format!("Revoked · AI can't work in {bundle} until you allow it again");
                    sync_connections(&self.store.ui);
                }
                Ok(false)=>self.message=format!("{bundle} wasn't allowed"),
                Err(why)=>self.message=why,
            },
            Intent::RevokeMirrorConsent(bundle)=>match self.generated.revoke_mirror_consent(&bundle){
                Ok(true)=>self.message=format!("Revoked · Mirror asks again before it shows {bundle}"),
                Ok(false)=>self.message=format!("{bundle} wasn't allowed in Mirror"),
                Err(why)=>self.message=why,
            },
            Intent::DeleteMyData(kind)=>{
                use crate::ui::shell::DataKind;
                self.shell.data_confirm=None;
                match self.generated.delete_my_data(kind){
                    Ok(line)=>{
                        if kind==DataKind::Projects{
                            let ui=&mut self.store.ui;
                            ui.pinned_projects.clear();ui.archived_projects.clear();ui.sample_projects.clear();ui.opened_projects.clear();
                            ui.drafts.retain(|k,_|k=="home");ui.last_project=None;
                            self.shell.drafts.retain(|k,_|k=="home");self.shell.end_rename();
                            if matches!(self.shell.view,View::Project{..}|View::App(_)|View::CompatCanvas(_)){self.shell.command.clear();self.shell.go(View::Home);}
                        }
                        self.message=line;self.mark();
                        // AI の account の鍵（Keychain）と login の dir は llm_cli の口で（generated は llm_cli を名指せない）。delete_my_data が Ok の後だけ
                        // （RQ3 反証の直し: 先に消すと、run の間の門「Stop the run first」より前に、戻せない鍵と使っている login を消していた）。消せなかった account は列に残す
                        if kind==DataKind::Connections{
                            let accounts=std::mem::take(&mut self.store.ui.ai_accounts);
                            let (kept,_gone):(Vec<_>,Vec<_>)=accounts.into_iter().partition(|a|self.forget_account(a).is_err());
                            self.store.ui.ai_accounts=kept;
                            if self.store.ui.ai_active.is_some()&&self.store.ui.active_account().is_none(){
                                self.store.ui.set_active_account(None);self.llm_switch=None;
                                self.llm_rx=None;self.start_llm_detect(ctx,false);
                            }
                            if !self.store.ui.ai_accounts.is_empty(){self.message=format!("{} AI accounts couldn't be removed · Try again",self.store.ui.ai_accounts.len());}
                        }
                    }
                    Err(why)=>self.message=why,
                }
                self.refresh_data(true);
            }
            Intent::RestoreDiagrams(project)=>match self.generated.restore_diagrams(&project){Ok(v)=>self.go(v),Err(why)=>self.message=why},
            // RQ4: 行の key と手の組ごとの処理（`ConnAct` の doc の表）は本体。interface の段は作り手（acts の button・Submit）が 0 なので届かない
            // L4（111 通目）: Connections の AI providers の `ai` の行の Check again = 探し直す（account の手は `settings::accounts` が `Intent::UseAiAccount` などで返す）
            Intent::Connection{row,act} if row=="ai"=>if act==ui::connections::ConnAct::CheckAgain{self.apply(ctx,Intent::RecheckAi)},
            Intent::Connection{row,act}=>self.connection(row,act),
            Intent::Proposal{id,act}=>self.proposal(ctx,id,act),
            // CHAT-ONE-1101: Home の chat bar の folder の icon = Home の会話の folder を選ぶ（答えは poll_folder が `workspace` へ）
            Intent::Page(PageIntent::ChooseFolder) if !matches!(self.shell.view,View::Project{..})=>self.ask_folder(ctx,None),
            // PROJECT-DIR-1101: `•••` / `{name} ⌄` の folder の 2 項目（開く = 自分の folder か repo・選び直す = G99c の窓 → SetWorkspace）
            Intent::ShowProjectFolder(project)=>match self.generated.project_folder(&project){
                Some(dir)=>if let Err(why)=crate::generated::run_open("/usr/bin/open",&[dir.to_string_lossy().as_ref()]){self.message=why;},
                None=>self.message="Couldn't make this project's folder · Choose one with Change folder…".into(),
            },
            Intent::ChangeProjectFolder(project)=>self.ask_folder(ctx,Some(project)),
            Intent::Page(op)=>if let View::Project{project,..}=self.shell.view.clone(){
                // W13: 自然言語の編集は、model が在れば裏の thread で読ませる（答えは poll_model が当てる・読む間は Page に 1 行）
                if let PageIntent::Edit{text}=&op{
                    if self.generated.model_busy_line().is_none(){self.generated.new_request();} // TALK-1101 追記 2: Project の一言 = 1 つの頼み
                    if self.generated.model_busy_line().is_some()||self.generated.start_edit(&project,text){return;}
                }
                // G99c: 選ぶ窓は裏で開く（UI を止めない）。答えは update が押した Project に当てる
                if matches!(op,PageIntent::ChooseFolder){
                    self.ask_folder(ctx,Some(project));
                    return;
                }
                // 拒否（最後の 1 枚・古い base_hash）は Page の上に 1 行（draw_page）。画面は今の Page のまま
                if let Ok(v)=self.generated.page_intent(&project,op){self.shell.go(v);}
            },
        }
    }
    /// 互換 view の toolbar（旧 top bar の操作をここへ: GitHub 接続先・配置/操作の切替・＋UIパーツ・Repo 機能の導入・message）。
    fn compat_toolbar(&mut self,ui:&mut egui::Ui,session:&Session){
        let t=theme::tokens(ui.ctx());
        ui.horizontal(|ui|{
            ui.vertical(|ui|{
                theme::label(ui,Type::Title,&session.name,t.text_primary);
                theme::label(ui,Type::Helper,"Classic canvas · controls and workflows in one place",t.text_secondary);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                let running=self.active_count(session.id);
                if running>0{ui::states::inline(ui,RegionState::Running,&format!("{running} running"));}
                else{ui::states::inline(ui,RegionState::Ready,"Idle");}
            });
        });
        // UI-5a: canvas を消す確認は題の下の 1 行（Window をやめる・G14 B8・答え ③）。Esc は keyboard の連鎖が閉じる
        if self.delete_session_prompt{
            match theme::confirm_inline(ui,&format!("Delete “{}”? Its parts and history are deleted too.",session.name)){
                Some(true)=>{
                    if self.store.sessions.len()>1{
                        let id=self.store.selected;
                        self.store.sessions.retain(|s|s.id!=id);
                        self.store.selected=self.store.sessions[0].id;
                        self.selected_panel=None;self.mark();
                    }
                    self.delete_session_prompt=false;
                }
                Some(false)=>self.delete_session_prompt=false,
                None=>{}
            }
        }
        ui.add_space(theme::SP_2);
        ui.horizontal_wrapped(|ui|{
            theme::label(ui,Type::Helper,"GitHub",t.text_tertiary);
            ui.add(egui::TextEdit::singleline(&mut self.repo).hint_text("owner/repo").desired_width(165.0).font(Type::Nav.font()));
            if theme::button_compact(ui,"Set as source",Variant::Ghost).clicked(){
                if github::valid_repo(&self.repo){
                    if let Some(s)=self.store.selected_mut(){s.repo=self.repo.clone();
                        for action in &mut s.actions{match &mut action.kind{
                            ActionKind::GitHubIssues{repo}|ActionKind::GitHubPulls{repo}|
                            ActionKind::GitHubCreateIssue{repo,..}=>*repo=self.repo.clone(),_=>{}}}}
                    self.mark();self.message=format!("GitHub source: {}",self.repo);
                }else{self.message="Use the owner/repo form".into()}
            }
            ui.separator();
            if self.editing{theme::label(ui,Type::Helper,"Editing layout · actions are off",t.text_primary);}
            if theme::button_compact(ui,if self.editing{"Done"}else{"Edit layout"},Variant::Ghost).on_hover_text("⌘E").clicked(){self.editing=!self.editing;}
            // 素の menu の頭は線なし（UI-3）。同じ行の GitHub の欄の枠は残すので scope はこの button だけ
            ui.scope(|ui|{theme::native_buttons(ui);ui.menu_button("Add part",|ui|{
                for (kind,label) in [
                    (PanelKind::GitHubIssues,"GitHub issues"),
                    (PanelKind::GitHubPulls,"GitHub pull requests"),
                    (PanelKind::WebService,"Web data"),
                    (PanelKind::Desktop,"Screen control"),
                    (PanelKind::Feature,"Repo feature"),
                    (PanelKind::Workflow,"Workflow"),
                    (PanelKind::Notes,"Notes"),
                ] {
                    if ui.button(label).clicked(){self.append_panel(kind,label);ui.close_menu();}
                }
            });});
            if theme::button_compact(ui,"Import from GitHub…",Variant::Ghost).clicked(){self.show_import=true;}
        });
        ui.add_space(theme::SP_1);
        // FX-027: 直近の message は窓の一番下の帯 1 か所だけ（ここにも出すと同じ文が 2 か所に並ぶ）
        ui.add_space(theme::SP_2);
    }
    fn show_panel_content(&mut self,ui:&mut egui::Ui,panel:&Panel,session:&Session){
        match panel.kind {
            PanelKind::GitHubIssues|PanelKind::GitHubPulls=>{
                let is_issue=panel.kind==PanelKind::GitHubIssues;
                let action=session.actions.iter().find(|a|if is_issue{matches!(&a.kind,ActionKind::GitHubIssues{..})}else{matches!(&a.kind,ActionKind::GitHubPulls{..})}).cloned();
                ui.label(RichText::new(format!("Source: {}",session.repo)).small().color(text_muted()));
                if let Some(a)=action{
                    if !github::valid_repo(&session.repo){ui.colored_label(warning(),"No GitHub source");}
                    if ui.add_enabled_ui(github::valid_repo(&session.repo),|ui|theme::button(ui,if is_issue{"Get issues"}else{"Get pull requests"},Variant::Ghost)).inner.clicked(){self.queue(vec![a.clone()],if is_issue{"Get issues"}else{"Get pull requests"});}
                    if let Some(Ok(r)|Err(r))=self.results.get(&a.id){render_github_items(ui,r);}
                }
                if is_issue{
                    ui.separator();ui.small("New issue");
                    let title_key="issue_title";let body_key="issue_body";
                    let mut title=session.drafts.get(title_key).cloned().unwrap_or_default();
                    let mut body=session.drafts.get(body_key).cloned().unwrap_or_default();
                    let title_changed=ui.add(egui::TextEdit::singleline(&mut title).hint_text("Title")).changed();
                    let body_changed=ui.add(egui::TextEdit::multiline(&mut body).desired_rows(2).hint_text("Description")).changed();
                    if title_changed||body_changed{
                        if let Some(s)=self.store.selected_mut(){s.drafts.insert(title_key.into(),title.clone());s.drafts.insert(body_key.into(),body.clone());}
                        self.mark();
                    }
                    let can_create=github::valid_repo(&session.repo)&&!title.trim().is_empty();
                    if ui.add_enabled_ui(can_create && !self.editing,|ui|theme::button(ui,"Create issue…",Variant::Ghost)).inner.clicked(){
                        let a=self.ensure_action("Create issue",ActionKind::GitHubCreateIssue{repo:session.repo.clone(),title,body});
                        self.queue(vec![a],"Create issue");
                    }
                }
            }
            PanelKind::WebService=>{
                let action=session.actions.iter().find(|a|matches!(&a.kind,ActionKind::WebRead)).cloned();
                if let Some(a)=action{
                    let configured=self.web_url().is_some()&&!self.disconnected("web"); // RQ4: Connections の Web data（env は試験の口）
                    if !configured{ui.colored_label(warning(),"Web data isn't set up");}
                    if ui.add_enabled_ui(configured,|ui|theme::button(ui,"Get data",Variant::Ghost)).inner.clicked(){self.queue(vec![a.clone()],"Read a web service");}
                    if let Some(Ok(r)|Err(r))=self.results.get(&a.id){render_web_data(ui,r);}
                }
            }
            PanelKind::Desktop=>{
                // ①② の読み・③ の実行とも bridge（YC W17-1 / W17-2・YC_GATE #5・#6）。cua-driver は起こさない
                let bridge=crate::surface::find_bridge();
                if let Err(e)=&bridge{ui.colored_label(warning(),format!("Can't read the screen · {e}"));}
                if ui.add_enabled_ui(bridge.is_ok(),|ui|theme::button(ui,"Find windows",Variant::Ghost)).inner.clicked(){
                    let tx=self.tx.clone();let session_id=self.store.selected;
                    thread::spawn(move||{let _=tx.send(Event::Windows{session:session_id,result:desktop_windows(&mut AdapterRegistry::desktop())});});
                }
                if !self.windows.is_empty(){
                    self.selected_window=self.selected_window.min(self.windows.len()-1);
                    let previous=self.selected_window;
                    egui::ComboBox::from_id_salt(("gui-windows",session.id)).selected_text(&self.windows[self.selected_window].title)
                        .show_ui(ui,|ui|{for (i,w) in self.windows.iter().enumerate(){ui.selectable_value(&mut self.selected_window,i,format!("{} ({}/{})",w.title,w.pid,w.window_id));}});
                    if self.selected_window!=previous{self.controls.clear();self.selected_control=0;}
                    // W19-1: app ごとの AI の同意（既定は無し・Mirror の「映す」と別の口）。同意の無い app には ③ の実行が送らず同じ 1 行を返す
                    if let Some(Err(e))=act_consent_row(ui,&self.windows[self.selected_window],&crate::surface::act_consent_path()){self.message=e;}
                    if theme::button(ui,"Read controls",Variant::Ghost).clicked(){
                        let w=self.windows[self.selected_window].clone();let tx=self.tx.clone();
                        let session_id=self.store.selected;
                        thread::spawn(move||{
                            let result=desktop_controls(&mut AdapterRegistry::desktop(),&w);
                            let _=tx.send(Event::Controls{session:session_id,pid:w.pid,window_id:w.window_id,result});
                        });
                    }
                }
                if !self.controls.is_empty(){
                    self.selected_control=self.selected_control.min(self.controls.len()-1);
                    egui::ComboBox::from_id_salt(("gui-controls",session.id)).selected_text(&self.controls[self.selected_control].label)
                        .show_ui(ui,|ui|{for(i,c) in self.controls.iter().enumerate(){
                            let shown=match c.value.as_deref().filter(|v|!v.is_empty()){None=>format!("{} · {}",c.label,c.role),Some(v)=>format!("{} · {} · {}",c.label,c.role,v.chars().take(40).collect::<String>())};
                            ui.selectable_value(&mut self.selected_control,i,shown);}});
                    ui.horizontal(|ui|{
                        if theme::button(ui,"Add click step",Variant::Ghost).clicked(){self.new_gui_binding(DesktopTool::Click)}
                        if theme::button(ui,"Add typing step",Variant::Ghost).clicked(){self.new_gui_binding(DesktopTool::TypeText)}
                    });
                }
                ui.separator();
                for a in &session.actions{
                    if matches!(&a.kind,ActionKind::Desktop{..}){
                        ui.horizontal_wrapped(|ui|{
                            ui.small(&a.name);
                            if let ActionKind::Desktop{tool:DesktopTool::TypeText,..}=&a.kind {
                                let mut contents=match &a.kind{ActionKind::Desktop{text,..}=>text.clone(),_=>String::new()};
                                if ui.add(egui::TextEdit::singleline(&mut contents).hint_text("Text to type").desired_width(120.0)).changed(){
                                    if let Some(s)=self.store.selected_mut(){if let Some(item)=s.actions.iter_mut().find(|it|it.id==a.id){
                                        if let ActionKind::Desktop{text,..}=&mut item.kind{*text=contents;self.dirty=true;}
                                    }}
                                }
                            }
                            if theme::button_compact(ui,"Run",Variant::Ghost).clicked(){
                                let live=self.store.selected().and_then(|s|s.find_action(a.id)).cloned().unwrap_or_else(||a.clone());
                                self.queue(vec![live],"Screen control");
                            }
                        });
                    }
                }
            }
            PanelKind::Feature=>{
                if theme::button(ui,"Import from GitHub…",Variant::Ghost).clicked(){self.show_import=true;}
                let features=self.store.features.clone();
                for f in features{
                    ui.separator();ui.label(RichText::new(format!("{}/{} / {}",f.owner,f.repo,f.subdir)).small().strong());
                    ui.small(format!("{} · {}",f.license,&f.commit[..12.min(f.commit.len())]));
                    ui.small(if f.enabled{"Command registered"}else{"Code only · can't run"});
                    if f.enabled && theme::button(ui,"Run this feature",Variant::Ghost).clicked(){
                        let a=self.ensure_action("Imported command",ActionKind::TrustedCli{feature_id:f.id});
                        self.queue(vec![a],"Run registered command");
                    }
                }
            }
            PanelKind::Workflow=>{
                for wf in &session.workflows{
                    ui.group(|ui|{
                        ui.label(RichText::new(&wf.name).strong());
                        if wf.steps.is_empty(){ui.small("Not connected. Add a step that can run");}
                        for (index,id) in wf.steps.iter().enumerate(){
                            if let Some(a)=session.find_action(*id){
                                ui.horizontal(|ui|{
                                    ui.label(RichText::new(format!("{:02}",index+1)).size(12.0).color(text_muted()));
                                    ui.label(RichText::new(&a.name).size(12.0));
                                    if let Some(result)=self.results.get(id){
                                        let (word,color)=match result{Err(_)=>(RegionState::Error.wording(),error_color()),Ok(r) if r.contains("未検証")=>("Not verified",text_main()),Ok(_)=>(RegionState::Verified.wording(),text_main())};
                                        ui.label(RichText::new(word).size(12.0).color(color));
                                    }
                                });
                            }
                        }
                        if theme::button(ui,&format!("Run “{}”",wf.name),Variant::Ghost).clicked(){
                            match engine::valid_workflow(session,wf){Ok(a)=>self.queue(a,&wf.name),Err(e)=>self.message=e}
                        }
                    });
                }
                if session.workflows.len()>=2 && theme::button(ui,"Run the first two at once",Variant::Ghost).clicked(){self.queue_parallel(session);}
            }
            PanelKind::Notes=>{
                ui.label(RichText::new(&session.prompt).small());
                if theme::button(ui,"Create canvas for GitHub, editor and web",Variant::Ghost).clicked(){
                    self.prompt="Bring together GitHub issues and pull requests, IDE edits and web services".into();
                    self.add_session();
                }
            }
        }
    }
    /// 旧 inspector は card を選んだ時だけ右の Drawer（296）。
    fn inspector(&mut self, ctx:&egui::Context) {
        let Some(panel_id)=self.selected_panel else{return};
        let t=theme::tokens(ctx);
        egui::SidePanel::right("inspector").exact_width(theme::DRAWER_W).resizable(false).show_separator_line(true)
            .frame(egui::Frame::new().fill(t.surface_1).inner_margin(egui::Margin::symmetric(14,12)))
            .show(ctx,|ui|{
            ui.horizontal(|ui|{
                ui.vertical(|ui|{
                    let what=self.store.selected().and_then(|s|s.panels.iter().find(|p|p.id==panel_id)).map(|p|p.title.clone()).unwrap_or_default();
                    theme::label(ui,Type::Group,&format!("Details of {what}"),t.text_primary);
                    theme::label(ui,Type::Helper,"Structure and target of the selected control",t.text_secondary);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP),|ui|{
                    if theme::icon_button(ui,crate::domain::icon::Icon::X,"Close").clicked(){self.selected_panel=None;}
                });
            });
            ui.add_space(12.0);
            let Some(session)=self.store.selected().cloned() else{return};
            let panel=session.panels.iter().find(|p|p.id==panel_id && !p.hidden).cloned();
            egui::ScrollArea::vertical().id_salt("inspector-scroll").show(ui,|ui|{
                if let Some(panel)=panel{
                    let mut title=panel.title.clone();
                    ui.label(RichText::new("Part name").size(12.0).color(text_muted()));
                    if ui.add(egui::TextEdit::singleline(&mut title)).changed(){
                        if let Some(real)=self.store.selected_mut().and_then(|s|s.panels.iter_mut().find(|p|p.id==panel.id)){
                            real.title=title;
                        }
                        self.mark();
                    }
                    ui.add_space(8.0);
                    let mut locked=panel.locked;
                    if ui.checkbox(&mut locked,"Lock position").changed(){
                        if let Some(real)=self.store.selected_mut().and_then(|s|s.panels.iter_mut().find(|p|p.id==panel.id)){
                            real.locked=locked;
                        }
                        self.mark();
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Position and size (px)").size(12.0).color(text_muted()));
                    let (mut x,mut y,mut w,mut h)=(panel.x,panel.y,panel.width,panel.height);
                    let mut changed=false;
                    ui.horizontal(|ui|{
                        ui.label("X");changed|=ui.add(egui::DragValue::new(&mut x).speed(8.0).range(0.0..=4096.0)).changed();
                        ui.label("Y");changed|=ui.add(egui::DragValue::new(&mut y).speed(8.0).range(0.0..=4096.0)).changed();
                    });
                    ui.horizontal(|ui|{
                        ui.label("W");changed|=ui.add(egui::DragValue::new(&mut w).speed(8.0).range(layout::MIN_W..=4096.0)).changed();
                        ui.label("H");changed|=ui.add(egui::DragValue::new(&mut h).speed(8.0).range(layout::MIN_H..=4096.0)).changed();
                    });
                    if changed{
                        if let Some(real)=self.store.selected_mut().and_then(|s|s.panels.iter_mut().find(|p|p.id==panel.id)){
                            real.x=layout::snap(x);real.y=layout::snap(y);
                            real.width=layout::snap(w);real.height=layout::snap(h);
                        }
                        self.mark();
                    }
                    ui.add_space(10.0);
                    ui.separator();ui.add_space(7.0);
                    ui.label(RichText::new("Control tree").strong().size(12.0));
                    ui.label(RichText::new("Each control keeps its role, source and action as a typed node").size(12.0).color(text_muted()));
                    let tree=ui_tree::panel_tree(&session,&panel);
                    let mut nodes=Vec::new();ui_tree::flatten(&tree,&mut nodes);
                    for n in nodes.iter().skip(1){
                        ui.add_space(5.0);
                        ui.label(RichText::new(&n.name).size(12.0).strong());
                        ui.label(RichText::new(format!("{} · {}",n.role.label(),n.availability.label()))
                            .size(12.0).color(if n.availability==ui_tree::Availability::NeedsConnection{warning()}else{text_muted()}));
                        ui.label(RichText::new(format!("Source: {}",n.source)).size(12.0).color(text_muted()));
                        if let Some(action)=n.action_id{ui.label(RichText::new(format!("Action: {action}")).size(12.0).color(text_muted()));}
                    }
                    ui.add_space(16.0);
                    // UI-5a: 消す確認は同じ場所の 1 行（Window をやめる・G14 B8・答え ③）。Esc は keyboard の連鎖が閉じる
                    if self.delete_panel_prompt==Some(panel.id){
                        match theme::confirm_inline(ui,&format!("Delete “{}”?",panel.title)){
                            Some(true)=>{
                                if let Some(s)=self.store.selected_mut(){s.panels.retain(|p|p.id!=panel.id);}
                                self.selected_panel=None;self.delete_panel_prompt=None;self.mark();
                            }
                            Some(false)=>self.delete_panel_prompt=None,
                            None=>{}
                        }
                    }else if theme::button(ui,"Delete part…",Variant::Danger).clicked(){self.delete_panel_prompt=Some(panel.id);}
                }else{
                    theme::label(ui,Type::Helper,"Not found",t.text_secondary);
                }
            });
        });
    }
    /// 互換 view: 旧 canvas をそのまま主領域に描く（DESIGN §13「PanelKind と自由配置 canvas は互換 view のみ」）。
    fn compat_canvas(&mut self,ui:&mut egui::Ui){
            let Some(session)=self.store.selected().cloned() else{return};
            egui::Frame::new().inner_margin(egui::Margin{left:16,right:16,top:12,bottom:0}).show(ui,|ui|self.compat_toolbar(ui,&session));
            let available=ui.available_rect_before_wrap();
            let canvas=Rect::from_min_max(available.min+Vec2::new(16.0,0.0),available.max-Vec2::new(16.0,16.0));
            ui.painter().rect_filled(canvas,egui::CornerRadius::same(12),bg_canvas_inner());
            // Subtle canvas grid; logical panel coordinates are relative to canvas.min.
            let mut x=canvas.left()+12.0;
            while x<canvas.right(){
                ui.painter().line_segment([Pos2::new(x,canvas.top()),Pos2::new(x,canvas.bottom())],Stroke::new(0.5,grid_color()));
                x+=24.0;
            }
            let mut y=canvas.top()+12.0;
            while y<canvas.bottom(){
                ui.painter().line_segment([Pos2::new(canvas.left(),y),Pos2::new(canvas.right(),y)],Stroke::new(0.5,grid_color()));
                y+=24.0;
            }
            let visible:Vec<Panel>=session.panels.iter().filter(|p|!p.hidden).cloned().collect();
            if visible.is_empty(){
                ui.painter().text(canvas.center(),Align2::CENTER_CENTER,
                    "Describe what to build in the box above, or use Add part",
                    FontId::proportional(13.0),text_muted());
            }
            for mut panel in visible {
                layout::fit_panel(&mut panel,canvas.width(),canvas.height());
                let original=(panel.x,panel.y,panel.width,panel.height);
                let rect=Rect::from_min_size(canvas.min+Vec2::new(panel.x,panel.y),
                    Vec2::new(panel.width,panel.height));
                let selected=self.selected_panel==Some(panel.id);
                ui.painter().rect_filled(rect,egui::CornerRadius::same(10),bg_panel());
                let outline=if selected{text_main()}else{border_color()};
                for (a,b) in [(rect.left_top(),rect.right_top()),(rect.right_top(),rect.right_bottom()),
                    (rect.right_bottom(),rect.left_bottom()),(rect.left_bottom(),rect.left_top())]{
                    ui.painter().line_segment([a,b],Stroke::new(if selected{1.5}else{1.0},outline));
                }
                let header=Rect::from_min_size(rect.min,Vec2::new(rect.width(),layout::HEADER_H));
                ui.painter().rect_filled(header,egui::CornerRadius::same(9),bg_panel_header());
                let move_id=egui::Id::new(("panel-header",session.id,panel.id));
                let response=ui.interact(header,move_id,if self.editing&&!panel.locked{
                    Sense::click_and_drag()
                }else{Sense::click()});
                if response.clicked(){self.selected_panel=Some(panel.id);}
                if response.drag_started(){self.drag_start=Some((panel.id,panel.x,panel.y));}
                if self.editing&&!panel.locked&&response.dragged(){
                    self.selected_panel=Some(panel.id);
                    if let (Some((id,start_x,start_y)),Some(delta))=(self.drag_start,total_drag_delta(ui)){
                        if id==panel.id{
                            panel.x=start_x;panel.y=start_y;
                            layout::move_panel(&mut panel,delta.x,delta.y,canvas.width(),canvas.height());
                        }
                    }
                }
                if !response.dragged() && self.drag_start.map(|(id,_,_)|id)==Some(panel.id){self.drag_start=None;}
                response.on_hover_cursor(if self.editing&&!panel.locked{CursorIcon::Grab}else{CursorIcon::PointingHand});
                ui.painter().text(header.left_center()+Vec2::new(13.0,0.0),Align2::LEFT_CENTER,
                    &panel.title,FontId::proportional(13.0),text_main());
                let source=ui_tree::source_label(&panel.kind);
                let status=if matches!(panel.kind,PanelKind::Desktop)&&
                    !session.actions.iter().any(|a|matches!(&a.kind,ActionKind::Desktop{..})){
                    "Not connected"
                }else if panel.locked{"Fixed"}else{source};
                ui.painter().text(header.right_center()-Vec2::new(12.0,0.0),Align2::RIGHT_CENTER,
                    status,FontId::proportional(12.0),text_muted());
                let content=Rect::from_min_max(
                    rect.min+Vec2::new(11.0,layout::HEADER_H+7.0),
                    rect.max-Vec2::new(11.0,12.0));
                if content.width()>20.0&&content.height()>12.0{
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content),|ui|{
                        ui.set_clip_rect(content);
                        egui::ScrollArea::vertical().id_salt(("content",session.id,panel.id))
                            .auto_shrink([false,false]).show(ui,|ui|{
                                self.show_panel_content(ui,&panel,&session);
                            });
                    });
                }
                if self.editing&&!panel.locked{
                    let handle=Rect::from_min_size(rect.right_bottom()-Vec2::new(21.0,21.0),Vec2::splat(19.0));
                    ui.painter().line_segment([handle.left_bottom()+Vec2::new(7.0,-3.0),handle.right_top()+Vec2::new(-3.0,7.0)],
                        Stroke::new(2.0,text_muted()));
                    let resize=ui.interact(handle,egui::Id::new(("resize",session.id,panel.id)),Sense::click_and_drag());
                    if resize.drag_started(){self.resize_start=Some((panel.id,panel.width,panel.height));}
                    if resize.dragged(){
                        self.selected_panel=Some(panel.id);
                        if let (Some((id,start_w,start_h)),Some(delta))=(self.resize_start,total_drag_delta(ui)){
                            if id==panel.id{
                                panel.width=start_w;panel.height=start_h;
                                layout::resize_panel(&mut panel,delta.x,delta.y,canvas.width(),canvas.height());
                            }
                        }
                    }
                    if !resize.dragged() && self.resize_start.map(|(id,_,_)|id)==Some(panel.id){self.resize_start=None;}
                    resize.on_hover_cursor(CursorIcon::ResizeNwSe);
                }
                let new=(panel.x,panel.y,panel.width,panel.height);
                if new!=original{
                    if let Some(real)=self.store.selected_mut().and_then(|s|s.panels.iter_mut().find(|p|p.id==panel.id)){
                        real.x=new.0;real.y=new.1;real.width=new.2;real.height=new.3;
                    }
                    self.mark();
                }
            }
    }
    fn approval(&mut self,ctx:&egui::Context){
        let Some(p)=self.pending.clone() else{return};
        egui::Window::new("Needs approval").id(egui::Id::new("approval"))
            .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,Vec2::ZERO)
            .show(ctx,|ui|{
                ui.heading(&p.title);
                ui.small("Check the target and action. If a step fails, the rest stops");
                for a in p.actions.iter().chain(p.other_lane.iter().flat_map(|lane|lane.iter())) {
                    ui.separator();ui.label(RichText::new(&a.name).strong());
                    ui.small(format!("{} · {}",a.kind.label(),if a.kind.is_write(){"Changes things outside Shibubu"}else{"Reads only"}));
                    match &a.kind {
                        ActionKind::Desktop{label,tool,text,app_name,..}=>{
                            ui.small(format!("{app_name} window · {label}"));
                            if *tool==DesktopTool::TypeText{ui.small(format!("Types “{text}”"));}
                        }
                        ActionKind::GitHubCreateIssue{repo,title,body}=>{ui.small(format!("{repo}: {title}"));ui.small(body);}
                        ActionKind::TrustedCli{feature_id}=>{if let Some(f)=self.store.features.iter().find(|f|f.id==*feature_id){ui.small(format!("Command: {} {}",f.cli_path,f.cli_args.join(" ")));}}
                        _=>{}
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui|{
                    if theme::button(ui,"Cancel",Variant::Ghost).clicked(){self.pending=None;}
                    if theme::button(ui,"Approve and run",Variant::Primary).clicked(){self.start_pending();}
                });
            });
    }
    fn import_dialog(&mut self,ctx:&egui::Context){
        if !self.show_import{return}
        let mut close=false;
        egui::Window::new("Import from GitHub").id(egui::Id::new("import"))
            .default_width(540.0).resizable(true).show(ctx,|ui|{
                ui.label("Imports only the subfolder you name, pinned to one commit, not the whole public repo");
                ui.horizontal(|ui|{ui.label("Repo");ui.add(egui::TextEdit::singleline(&mut self.import_repo).hint_text("owner/repository"));});
                ui.horizontal(|ui|{ui.label("Folder in the repo");ui.add(egui::TextEdit::singleline(&mut self.import_path).hint_text("packages/feature"));});
                if theme::button(ui,"Check licence, path and commit",Variant::Ghost).clicked(){
                    let repo=self.import_repo.trim().to_string();let path=self.import_path.trim().to_string();
                    let tx=self.tx.clone();self.review=None;
                    thread::spawn(move||{let _=tx.send(Event::Inspect(repos::inspect(&repo,&path)));});
                }
                if let Some(review)=&self.review {
                    ui.separator();
                    ui.label(RichText::new(format!("{}/{} / {}",review.owner,review.repo,review.subdir)).strong());
                    ui.horizontal(|ui|{ui.small("Commit");ui.small(&review.sha);});
                    ui.horizontal(|ui|{ui.small("Licence");ui.small(format!("{} ({})",review.license,review.license_path));});
                    ui.horizontal(|ui|{ui.small("Description");ui.small(&review.description);});
                    if !review.license_url.is_empty(){ui.horizontal(|ui|{ui.small("Licence source");ui.small(&review.license_url);});}
                    ui.colored_label(warning(),"A folder on its own may not run. Check dependencies and licence terms.");
                    ui.checkbox(&mut self.license_accepted,"I reviewed the licence, dependencies and scope");
                    let allowed=self.license_accepted&&review.license!="UNKNOWN"&&review.license!="NOASSERTION";
                    if ui.add_enabled_ui(allowed,|ui|theme::button(ui,"Fetch this folder only (code is not run)",Variant::Ghost)).inner.clicked(){
                        let review=review.clone();let tx=self.tx.clone();let dir=self.data_path.parent().unwrap_or(Path::new(".")).join("features");
                        let session_id=self.store.selected;
                        thread::spawn(move||{let _=tx.send(Event::Imported{session:session_id,result:repos::import_feature(&review,&dir,true)});});
                        close=true;
                    }
                }
                ui.separator();
                ui.label(RichText::new("Connect a trusted command").strong());
                ui.small("You choose a command you already trust. Code imported from GitHub is never run or built automatically");
                let features=self.store.features.clone();
                for f in features {
                    ui.group(|ui|{
                        ui.label(format!("{} · {} · {}",f.repo,f.subdir,&f.commit[..12.min(f.commit.len())]));
                        let entry=self.cli_editor.entry(f.id).or_insert_with(||(f.cli_path.clone(),f.cli_args.join(" ")));
                        ui.add(egui::TextEdit::singleline(&mut entry.0).hint_text("/absolute/path/to/tool"));
                        ui.add(egui::TextEdit::singleline(&mut entry.1).hint_text("Fixed arguments"));
                        if theme::button(ui,"Register command",Variant::Ghost).clicked(){
                            let bin=entry.0.trim().to_owned();let args:Vec<String>=entry.1.split_whitespace().map(String::from).collect();
                            if !Path::new(&bin).is_absolute()||!Path::new(&bin).is_file(){self.message="Enter the full path of an existing program".into();}
                            else if let Some(real)=self.store.features.iter_mut().find(|r|r.id==f.id){
                                real.cli_path=bin;real.cli_args=args;real.enabled=true;real.cli_pin=repos::cli_pin(real).unwrap_or_default(); // 登録した時の中身を固定（ADR-002 (b)）
                                let feature_id=f.id;
                                if let Some(s)=self.store.selected_mut(){
                                    let action=Action::new("Run trusted command",ActionKind::TrustedCli{feature_id});
                                    if let Some(wf)=s.workflows.iter_mut().find(|w|w.is_desktop()){
                                        wf.steps.push(action.id);
                                    }
                                    s.actions.push(action);
                                }
                                self.mark();self.message="Command saved. Shibubu asks before every run".into();
                            }
                        }
                    });
                }
                if theme::button(ui,"Close",Variant::Ghost).clicked(){close=true;}
            });
        if close{self.show_import=false;}
    }
}

fn run_lane(session:u64,actions:Vec<Action>,tx:Tx,features:Vec<RepoFeature>,active:Arc<Mutex<std::collections::HashSet<String>>>,consents:HashMap<u64,ConsentReceipt>) {
    let _=tx.send(Event::LaneStarted{session});
    for a in actions {
        let resource=a.kind.exclusive_resource();
        let exclusive=if let Some(ref key)=resource {
            match active.lock(){Ok(mut guard)=>guard.insert(key.clone()),Err(_)=>false}
        }else{true};
        let result=if exclusive{execute(&a.kind,&features,consents.get(&a.id))}else{Err("Another run is using this target".into())};
        if let Some(ref key)=resource{if exclusive{if let Ok(mut g)=active.lock(){g.remove(key);}}}
        let stop=result.is_err();
        let _=tx.send(Event::Result{session,action:a.id,title:a.name,result});
        if stop{break}
    }
    let _=tx.send(Event::LaneFinished{session});
}
/// 選んだ窓の app を AI に任せる同意の 1 行（inline・modal にしない）。同意が無い時だけ出し、button（AX identifier `gui::act_consent_button_id`）を
/// 押すと `surface::grant_act_consent` で 1 行を書く。押した frame だけ Some（Err は書けなかった理由の 1 行）。
/// ponytail: 描く度に同意の file を読む（小さい追記だけの file・Desktop パネルを開いている間だけ）。重くなったら同意を ShibubuApp に持つ
pub(crate) fn act_consent_row(ui:&mut egui::Ui,w:&WindowRef,path:&Path)->Option<Result<crate::surface::ConsentRecord,String>>{
    if w.bundle_id.is_empty()||crate::surface::act_consents(path).contains(&w.bundle_id){return None}
    ui.horizontal_wrapped(|ui|{
        theme::native_buttons(ui); // 素の button も helper と同じ形（範囲の外は線なし = owner 209・AX の id と名前は egui の Button のまま = W19-3b の driver が引く）
        ui.colored_label(warning(),format!("Let the AI control {}? Nothing is sent to it until you press the button",w.app_name));
        let b=ui.button(format!("Hand {} to the AI",w.app_name));
        ui.ctx().accesskit_node_builder(b.id,|n|n.set_author_id(crate::gui::act_consent_button_id(&w.bundle_id)));
        b.clicked().then(||crate::surface::grant_act_consent(path,&w.bundle_id,crate::surface::now_ms()))
    }).inner
}

/// consent = 「確認して実行」で作った承認（画面の操作だけが使う・他の操作は今までどおり）
fn execute(kind:&ActionKind,features:&[RepoFeature],consent:Option<&ConsentReceipt>)->Result<String,String>{
    match kind {
        ActionKind::GitHubIssues{repo}=>github::issues(repo).map(|v|v.to_string()),
        ActionKind::GitHubPulls{repo}=>github::pulls(repo).map(|v|v.to_string()),
        ActionKind::GitHubCreateIssue{repo,title,body}=>github::create_issue(repo,title,body).map(|v|v.to_string()),
        ActionKind::WebRead=>github::web_read().map(|v|v.to_string()),
        ActionKind::Desktop{..}=>desktop_execute(&mut AdapterRegistry::desktop(),kind,consent),
        ActionKind::TrustedCli{feature_id}=>features.iter().find(|r|r.id==*feature_id)
            .ok_or("No feature registered".into()).and_then(repos::run_trusted_feature),
    }
}
/// FX5: GitHub の Ready を読み戻しから何分まで信じるか（過ぎたら Connected に下げ、`Check again` を促す）。
const GITHUB_TTL:Duration=Duration::from_secs(600);
/// RQ4: Connections の MCP の確かめの待ち（1 つの server を起こして tools/list を読むまで）
const MCP_CHECK:Duration=Duration::from_secs(10);
/// CHAT-FREE / AI-COMPOSE-1101: egui の矩形 → 閉じた WebView の枠（論理座標）
#[cfg(all(feature="webview",target_os="macos"))]
fn canvas_frame(r:egui::Rect)->crate::surface::Frame{
    crate::surface::Frame{x:r.min.x as f64,y:r.min.y as f64,w:r.width() as f64,h:r.height() as f64}
}
/// RQ4: Engine の部品（起動の 1 回と MCP の設定を書いた後・Engine は次の run から新しい設定を使う）
fn engine_setup(dir:&Path)->crate::generated::EngineSetup{
    let engine=crate::engine_page::CoreEngine{registry:Arc::new(AdapterRegistry::with_engine_adapters),data_dir:dir.to_path_buf(),
        mcp:engine_mcp(),env:crate::capability_engine::EnvFacts{os_version:crate::project::os_version(),..Default::default()},
        readback:crate::capability_engine::READBACK_WAIT,act_consents:crate::surface::act_consent_path()};
    crate::generated::EngineSetup{engine:Arc::new(engine),data_dir:dir.to_path_buf()}
}
/// RQ4: Engine が起こしてよい MCP（Shibubu の mcp.json・止めた server を落とし Keychain の秘密の env を入れた形）
fn engine_mcp()->crate::mcp::McpConfig{
    crate::mcp::load_engine_config().unwrap_or_default().resolved(&crate::storage::secret_store::SecretStore::from_env())
}
/// RQ4（RQ-015）: GitHub と Web data の読みに使う token と URL を決める（切った接続は渡さない・Keychain → env の順）
fn sync_connections(ui:&UiSettings){
    let keys=crate::storage::secret_store::SecretStore::from_env();
    let off=|k:&str|ui.disconnected.iter().any(|d|d==k);
    // PASSWORD-1101 の 2: 切った password manager の項目の参照は渡さない（参照は使う瞬間に `vault::resolve` が値へ）
    let live=|t:&String|crate::domain::vault::SecretRef::parse(t).map_or(true,|r|ui.vaults.contains(&r.manager));
    let gh=(!off("github")).then(||github::keychain_token(&keys).or_else(||env::var("GITHUB_TOKEN").ok())).flatten().filter(live);
    let web=ui.web_data_url.clone().or_else(||env::var("KANGAE_WEB_SERVICE_URL").ok()).filter(|u|github::web_url_ok(u)&&!off("web"))
        .map(|u|(u,keys.get(&crate::storage::secret_store::connection_key("web","token")).ok().flatten().or_else(||env::var("KANGAE_WEB_SERVICE_TOKEN").ok()).filter(live)));
    github::use_connections(gh,web);
    // RQ4（RQ-019 M26）: 切った接続を使う席の 1 行のための (capability の頭・名前)
    let mut off:Vec<(String,String)>=Vec::new();
    if ui.disconnected.iter().any(|k|k=="github"){off.push(("github.".into(),"GitHub".into()));}
    if ui.disconnected.iter().any(|k|k=="web"){off.push(("web.".into(),"Web data".into()));}
    off.extend(crate::mcp::load_engine_config().unwrap_or_default().mcp_servers.iter().filter(|(_,s)|s.disabled).map(|(n,_)|(format!("mcp.{}.",crate::capability_engine::slug(n)),n.clone())));
    crate::generated::set_disconnected_capabilities(off);
}
/// RQ4（RQ-017）: macOS の選ぶ窓で program を 1 つ選ばせる（絶対 path を打たせない・裏の thread・答えは `Event::ProgramChosen`）
fn choose_program(tx:Tx){
    thread::spawn(move||{
        let out=std::process::Command::new("/usr/bin/osascript").args(["-e","POSIX path of (choose file with prompt \"Choose the program Shibubu may run\")"])
            .stdin(std::process::Stdio::null()).output();
        let r=match out{
            Ok(o) if o.status.success()=>Ok(Some(PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))),
            Ok(o) if String::from_utf8_lossy(&o.stderr).contains("-128")=>Ok(None),
            Ok(_)=>Err("Couldn't open the file chooser · try again".into()),
            Err(e)=>Err(format!("Couldn't open the file chooser ({})",e.kind())),
        };
        let _=tx.send(Event::ProgramChosen(r));
    });
}
/// Connections の Web JSON の行の説明。URL は host だけ（query の鍵・userinfo を画面に出さない: AD-SEC-R2-2）
fn web_detail(url:Option<&str>)->String{
    match url{Some(u)=>format!("Address set · {} · not checked",github::url_host(u)),None=>github::WEB_DATA_OFF.into()}
}
fn render_github_items(ui:&mut egui::Ui,raw:&str){
    let Ok(value)=serde_json::from_str::<serde_json::Value>(raw) else {
        ui.label(RichText::new(raw).size(12.0).color(error_color()));return;
    };
    let Some(items)=value["items"].as_array() else {
        ui.label(RichText::new("The reply isn't in the expected format").size(12.0).color(warning()));return;
    };
    if items.is_empty(){ui.label("Nothing to show");}
    for item in items.iter().take(12){
        let title=item["title"].as_str().unwrap_or("Untitled");
        let number=item["number"].as_u64().unwrap_or(0);
        ui.horizontal_wrapped(|ui|{
            ui.label(RichText::new(format!("#{number}")).size(12.0).color(text_main()));
            let label=RichText::new(title).size(12.0);
            if let Some(url)=item["url"].as_str().filter(|url|url.starts_with("https://github.com/")){
                ui.hyperlink_to(label,url);
            }else{ui.label(label);}
        });
    }
}
fn render_web_data(ui:&mut egui::Ui,raw:&str){
    let formatted=serde_json::from_str::<serde_json::Value>(raw)
        .ok().and_then(|value|serde_json::to_string_pretty(&value["items"]).ok())
        .unwrap_or_else(||raw.to_owned());
    ui.label(RichText::new(formatted.chars().take(2200).collect::<String>()).monospace().size(12.0));
}
// ponytail: 互換 view（旧 canvas）の色関数は呼び出し側が多いので名前を残し、中身だけ DESIGN の token に差し替えた。
// 毎 frame の先頭で COMPAT_TOKENS に今の theme を写す。互換 view を消す時に一緒に消す。
thread_local!{static COMPAT_TOKENS:std::cell::Cell<theme::Tokens>=std::cell::Cell::new(theme::LIGHT);}
fn tk()->theme::Tokens{COMPAT_TOKENS.with(|c|c.get())}
fn bg_canvas_inner()->Color32{tk().surface_2}
fn bg_panel()->Color32{tk().surface_1}
fn bg_panel_header()->Color32{tk().surface_3}
fn grid_color()->Color32{tk().border.gamma_multiply(0.5)}
fn border_color()->Color32{tk().border_strong}
fn text_main()->Color32{tk().text_primary}
fn text_muted()->Color32{tk().text_tertiary}
fn warning()->Color32{tk().warning}
fn error_color()->Color32{tk().danger}

impl eframe::App for ShibubuApp {
    /// TERM-XTERM-1101: Terminal の xterm の頁に focus が在る間に押した Shibubu の short cut（⌘K・⌘T・⌃` …）を、この frame の egui の入力に足す
    /// （first responder が WebView の間は egui に key が来ない・`Generated::take_term_keys`）。
    /// TITLEBAR-ICONS-1101: 赤黄緑を題の帯（header 36）の縦の中央へ（AppKit は窓の大きさの変化と全画面の出入りで既定の位置に戻す = 毎 frame・寄っていれば何も書かない）。
    /// 全画面の間は赤黄緑が無いので打たない。この hook は eframe の本物の窓だけが呼ぶ（試験の `Win::step` は呼ばない = AppKit に触らない）。
    fn raw_input_hook(&mut self,_ctx:&egui::Context,raw:&mut egui::RawInput){
        #[cfg(target_os="macos")]
        if !raw.viewports.get(&raw.viewport_id).and_then(|v|v.fullscreen).unwrap_or(false){
            // Err（赤黄緑の無い窓）は既定の位置のまま続ける・記録しない（毎 frame の stderr を埋めない）
            let _=crate::surface::appkit(crate::surface::AppKitOp::TrafficLights{x:f64::from(ui::theme::TRAFFIC_X),bar_h:f64::from(ui::theme::HEADER_H)});
        }
        let keys=self.generated.take_term_keys();
        // TITLEBAR-ICONS-1101: Terminal の頁で打った ⌘1〜9 は Terminal の tab（egui の grid の focus は keyboard が見る・`term_keys` の doc）
        self.term_keys=keys.iter().any(|e|matches!(e,egui::Event::Key{key,pressed:true,modifiers,..} if modifiers.command && matches!(key.name().as_bytes(),[b'1'..=b'9'])));
        raw.events.extend(keys);
    }
    /// 描く順（場所を取る panel が先）: header → sidebar → strip → 互換 drawer → 下の panel（Chat と Terminal・L1-NORAIL）→ central → palette → 承認 / import / 削除。
    fn update(&mut self,ctx:&egui::Context,frame:&mut eframe::Frame){
        // CHAT-FREE-1101 段 2: WebView の入った build だけ chat の ```html を `Interactive view` の card にする（無い build は code block のまま）
        #[cfg(all(feature="webview",target_os="macos"))]ui::markdown::set_canvas_host(ctx,true);
        // UI-4b: sidebar を開いて描くかは毎 frame 覚えた値（無ければ窓の幅の既定）から解く（Shell::sidebar_open は写し・keyboard の ⌃⌘S より先）
        self.shell.sidebar_open=Tier::current(ctx).sidebar_open(self.store.ui.sidebar_hidden);
        // SESSIONS-LEFT-1101: sidebar で開いた Project の写し（書き手は FoldProject の腕と go）
        if self.shell.open_projects!=self.store.ui.open_projects{self.shell.open_projects=self.store.ui.open_projects.clone();}
        if self.shell.folded_headings!=self.store.ui.folded_headings{self.shell.folded_headings=self.store.ui.folded_headings.clone();}
        // UI-6a: ⌘K の旧 canvas の行（Duplicate / 削除・1 つなら理由の行）の材料 = 互換 view の間だけ canvas の数（写し・2 つ目の真実にしない）
        self.shell.canvases=matches!(self.shell.view,View::CompatCanvas(_)).then(||self.store.sessions.len());
        // RQ-023: Engine の Automation の門 = 依頼の時の UiSettings の今の値（写し・2 つ目の真実にしない・Settings で替えた直後の run から新しい門・再起動を待たない）
        self.generated.automation=self.store.ui.automation;
        self.poll();self.poll_llm();self.poll_talk(ctx);self.poll_found(ctx);self.poll_model(ctx);self.sync_agent();
        self.poll_account_sign_in(ctx);self.poll_cli_sign_in(ctx);
        // RQ6（RQ-024）: 帯の今の活動の 1 行（人を待っている / Shibubu が操作している）。言う間は 1 秒ごとに描き直して消える時を拾う
        let activity=crate::adapters::mac_gui::activity_line();
        if activity.is_some(){ctx.request_repaint_after(std::time::Duration::from_secs(1));}
        ui::portal_frame::set_activity(ctx,activity);
        self.keyboard(ctx);
        self.sync_text_zoom(ctx);
        self.poll_folder(ctx);
        // G104: `+` の menu の押せる形の材料（写し・2 つ目の真実にしない）
        self.shell.accepts=self.generated.accepts();
        // ACCOUNTS-1101 2 段目: 足した Claude の plan の email（名前の既定の元・file を読むので 5 秒に 1 回）
        if self.emails_at.is_none_or(|t|t.elapsed()>=Duration::from_secs(5)){
            self.account_emails=self.store.ui.ai_accounts.iter().filter_map(|a|crate::llm_cli::account_email(&self.data_dir,a.id()).map(|e|(a.id().to_owned(),e))).collect();
            self.emails_at=Some(Instant::now());
        }
        // ACCOUNTS-1101 2 段目: Project へ移った frame（起動で開いた Project も）= その Project で選んだ account に戻す（AI の確かめの途中は次の frame へ持ち越す）
        let here=match &self.shell.view{View::Project{project,..}=>Some(project.clone()),_=>None};
        if here!=self.account_for&&self.llm_rx.is_none(){
            self.account_for=here.clone();
            if let Some(p)=here{self.restore_project_account(ctx,&p);}
        }
        self.shell.providers=provider_rows(&self.store.ui,self.claude_here,&self.account_emails);
        // CHAT-ONE-1101: chat bar の folder の名前（Home = Home の会話の folder・Project = その Project の folder）
        self.shell.folder=match &self.shell.view{
            View::Project{project,..}=>self.generated.folder_name(project),
            _=>self.workspace.as_deref().and_then(|p|p.file_name()).map(|n|n.to_string_lossy().into_owned()),
        };
        // 111 通目（L4）: 各行の本当の状態。使っている行だけ確かめの答え（Ready は確かめが通った時だけ）・他は Detected。
        // PROVIDER-1101: login していない CLI の行は使っていなくても Needs sign-in（確かめが通った使っている行だけ Ready）
        let (ready,missing)=(self.generated.llm_ready(),self.generated.llm_line().map(str::to_owned));
        self.shell.provider_states=self.shell.providers.iter().map(|(a,_,on)|match a{
            Some(AiAccount::Found{needs_sign_in:true,..}) if !(*on&&ready)=>crate::model::ProviderState::NeedsSignIn,
            _=>crate::model::ProviderState::of(*on,ready,missing.as_deref()),
        }).collect();
        self.poll_attach(ctx);
        // G99: 窓が前に来た frame に、AI が使えず前の確かめから 60 秒より古ければ裏で探し直す（入れた・login した後に再起動しない）
        let focused=ctx.input(|i|i.viewport().focused).unwrap_or(true);
        if focused&&!self.was_focused&&!self.generated.llm_ready()&&self.llm_checked.elapsed()>Duration::from_secs(60){self.start_llm_detect(ctx,false);}
        self.was_focused=focused;
        // FX1: 窓の閉じる button（egui の close 要求）と ⌘Q・Dock の Quit・logout（main.rs `ask_before_quit` の印）を同じ `Intent::Quit` へ
        let quit_asked=crate::QUIT_ASKED.swap(false,std::sync::atomic::Ordering::SeqCst);
        if (quit_asked||ctx.input(|i|i.viewport().close_requested()))&&!self.quitting{self.apply(ctx,Intent::Quit{anyway:false});}
        COMPAT_TOKENS.with(|c|c.set(theme::tokens(ctx)));
        let tier=Tier::current(ctx);
        let rows=self.rows();
        let location=self.location();
        let running_total:usize=self.running.values().sum();
        let in_compat=matches!(self.shell.view,View::CompatCanvas(_));
        let mut intents:Vec<Intent>=Vec::new();
        // UI-5a: title line の ‹ の席（戻る先の Page の名前・無ければ席の幅だけ）
        let back=match &self.shell.view{View::Project{project,..}=>self.generated.back_target(project),_=>None};
        // G86 × L1-NORAIL（review B）: 新しい card（承認・決める・同意・Ask・結果）が出たら窓の幅に依らず下の panel を開く（畳んだまま見えずに止まらない・欄の focus は奪わない）。
        // 独立 review R1: 選んでいる tab は替えない（Terminal の focus を落とさない・CHAT-ONE-1101 から Terminal の tab の間の card は Chat の tab を開くまで見えない = terminal_panel の Reveal の doc）
        // CHAT-011 反証 Y1（121「suggest は chat 内で自動で出る」）: 新しい提案も同じ扱い = Engine の card が無ければ先頭の提案の id で、畳んだ帯でも Terminal の tab でも panel を開く
        // B34（NORAIL review N1）: Project の外（Home・Runs 等）では呼ばない = その間に覚えた鍵を忘れない（戻っても同じ card で開き直さない）
        let card=match &self.shell.view{View::Project{project,..}=>Some((project.clone(),self.generated.engine_view(project).and_then(|v|ui::engine_view::card_key(project,v)).or_else(||self.agent.visible().first().map(|t|format!("suggestion:{}",t.suggestion.id))))),_=>None};
        if let Some((project,card))=card{if self.shell.show_card(&project,card){
            // CHAT-SIDE-1101 追記 219〜221: 下でも右でも同じ panel を開く（focus は奪わない）
            let _=self.generated.panel_act(&project,PanelAct::Reveal);
        }}
        let busy=self.generated.model_busy_line().is_some()||self.waiting.is_some();
        // L1-NORAIL: Page の帯（tab ・ ↶ ↷ ＋ •••）は上の帯の中央（header の `band`）・本体は主領域（`draw_page_body`）
        let page_of=match &self.shell.view{View::Project{project,page}=>Some((project.clone(),page.clone())),_=>None};
        // CHAT-SIDE-1101 追記 210: header の右端の ◨（Project の間だけ・右に出しているか = 右に置いて開いている・追記 219: 右で閉じた panel は「出していない」）
        let chat_right=page_of.as_ref().map(|(p,_)|self.store.ui.chat_place==ChatPlace::Right&&self.generated.panel(p).open);
        if let Some(i)=ui::header::draw(ctx,&mut self.shell,&location,back.as_deref(),&rows,busy,chat_right,&mut |ui|page_of.as_ref().and_then(|(p,g)|self.generated.draw_page_band(ui,p,g))){intents.push(i);}
        // NOTIFY-1101: 知らせ（Generated の notices・Install の案内・画面収録の許可）は右上の toast。直近の message は窓の一番下の帯（STATUSBAR-1101）。
        // toast の `Install`（入っていない Project で最初に terminal を開いた時）= Install CLI & skill の一覧を開く
        match ui::toast::draw(ctx,ctx.available_rect().top()){
            Some("Install")=>if let Some(setup)=self.agent_setup.as_mut(){setup.read();setup.open=true;},
            // MIRROR-AUTO-1101 B: 画面収録の許可の知らせの手（Mirror の面の `Open settings` と同じ行き先）
            Some(ui::connections::SCREEN_CAPTURE_ACTION)=>ui::connections::open_settings(ui::connections::SCREEN_CAPTURE_SETTINGS),
            _=>{}
        }
        // RQ-USAGE（operator ①）: Keychain は利用者が使用量のために動いた後だけ読む（Settings › AI & Models を開いた・帯の `Show usage`）。その起動の間は以後 60 秒ごと
        // （下の帯と Settings・model を呼ばない）。読んだ後の帯が出ている間は 60 秒ごとに描き直す（`updated N ago` と次の読み）
        if self.shell.view==View::Settings(SettingsSection::AiModels){self.usage_asked=true;}
        if self.read_usage&&self.usage_asked{
            let others:Vec<AiAccount>=self.shell.providers.iter().filter_map(|(a,..)|a.clone()).collect();
            self.generated.refresh_usage(crate::google::now() as i64,false,&others);
        }
        let band=self.usage_view(true);
        if self.usage_asked&&band.is_some(){ctx.request_repaint_after(Duration::from_secs(60));}
        // STATUSBAR-1101（owner 193 通目）: 窓の一番下の細い帯 = 今の AI・使用量・↻・走っている数・直近の message（状態の 1 行・NOTIFY の toast から移した）。
        // sidebar より先に取る = 窓の幅いっぱい・畳んでも在る。同じ文を toast には出さない
        let ai=self.generated.ai_name();
        // STATUSBAR-1101 追記 210: つながっている AI を全部（読めない AI は理由の 1 行）・読まない起動（試験の `open`）は出さない
        let ais=if self.read_usage{bar_ais(&self.shell.providers,&ai,self.generated.plan_usage(),self.generated.cli_usages(),crate::google::now() as i64)}else{Vec::new()};
        // USAGE-OWN-1101（owner 203 通目）: Shibubu の中だけで使った量（自分の AI の呼び出し＋Shibubu の Terminal の agent）。60 秒ごとに裏で数え直す・Keychain は読まない
        let fronts=self.generated.take_agents();
        let own=self.read_usage.then(||{
            let now=crate::google::now() as i64;
            let plan=match self.generated.plan_usage(){Some(Ok(u))=>Some(u),_=>None};
            let c=ctx.clone();
            self.own.refresh(&self.data_dir,plan,now,false,move||c.request_repaint());
            self.own.see(&self.data_dir,&fronts,now as u64*1000);
            ctx.request_repaint_after(Duration::from_secs(60));
            // HOME-TERM-1101: Home の Terminal の agent は「Home」（Project の行は無い = 消した Project と言わない）
            let name=|id:&str|if id==crate::generated::HOME_PLACE{Some("Home".to_owned())}else{rows.iter().find_map(|r|match &r.view{View::Project{project,..} if project==id=>Some(r.name.clone()),_=>None})};
            let acct=|id:&str|self.shell.providers.iter().find(|(a,_,_)|a.as_ref().is_some_and(|a|a.id()==id)).map(|(_,l,_)|l.clone());
            self.own.panel(&name,&acct,now)
        });
        // PANEL-SYNC-1101: 畳んだ panel の tab を帯の左寄りに（Project = その Project の panel・Project の外 = Home の Terminal・開いていれば None）
        let folded=match &self.shell.view{View::Project{project,..}=>self.generated.folded(project),View::Home|View::Runs|View::Connections|View::Settings(_)=>self.generated.folded(crate::generated::HOME_PLACE),_=>None};
        let bar=ui::shell::StatusBar{ai:&ai,providers:&self.shell.providers,usage:band,ais,reading:self.generated.usage_reading(),running:running_total,status:&self.message,folded};
        if let Some(i)=ui::shell::status_bar(ctx,&bar){intents.push(i);}
        // SESSIONS-LEFT-1101: sidebar の Project の中の Page（開いた Project と、検索の間は全部）と、今の session（Project = その Project の今の session・Home = 今の会話）
        let searching=!self.shell.sidebar_query.trim().is_empty();
        let pages:std::collections::BTreeMap<String,Vec<(String,String,Option<String>)>>=rows.iter().filter(|r|!r.archived||self.shell.show_archived).filter_map(|r|r.project_id())
            .filter(|p|searching||self.store.ui.open_projects.contains(p)).map(|p|{let l=self.generated.page_list(&p);(p,l)}).collect();
        let session=match &self.shell.view{View::Project{project,..}=>Some(self.generated.session_key(project)),View::Home=>self.shell.talk_id.clone(),_=>None};
        // SESSIONS-LEFT-1101 追記 224: 同じ Project の Terminal の行（tab の名前と前面の agent）
        let terms:std::collections::BTreeMap<String,Vec<(String,String,Option<&'static crate::domain::terminal::AgentKind>)>>=pages.keys().map(|p|(p.clone(),self.generated.term_rows(p))).collect();
        if let Some(i)=ui::sidebar::draw(ctx,&mut self.shell,tier,&rows,self.generated.chat_list(),&pages,&terms,session.as_deref()){intents.push(i);}
        if in_compat{self.inspector(ctx);}
        // G95 × L1-NORAIL: 下の panel（Chat の tab と Terminal）は CentralPanel より先に取る（Page の面はその分だけ縮み、panel の下に入らない）。
        // Chat の tab（提案の行・会話・card・返事の 1 行・底の chat bar = Home と同じ部品・CHAT-ONE-1101）は Shell と提案を持つ ここで描く。Chat の tab・Show chat で開いた時は欄に focus
        // 段 2 作り直し: 選んだ Terminal の tab の 1 行目 = Install CLI & skill（agent_setup・Project の口が在る起動だけ）
        if let View::Project{project,..}=self.shell.view.clone(){
            let (mut turns,card)=(self.generated.chat(&project),self.generated.engine_view(&project).cloned());
            // CHAT-ONE-1101: この Project の一言を AI が読んでいる間は会話の最後に待つ 1 行（保存しない）
            if self.talk_rx.as_ref().is_some_and(|r|r.0.as_deref()==Some(self.generated.session_key(&project).as_str())){
                use crate::domain::chat::{Mark,Turn,Who,TALK_WAIT};
                turns.push(Turn{who:Who::Shibubu,text:TALK_WAIT.into(),mark:Mark::None,at:String::new(),attachments:Vec::new(),choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None});
            }
            let wait=self.generated.card_wait(&project); // OPEN-INPLACE-1101 の 5: Preview の読みの最中も Build now は押せる（押すと読み終えてから作る）
            let (shell,agent,setup)=(&mut self.shell,&self.agent,self.agent_setup.as_mut());
            let mut said=None;
            // CHAT-SIDE-1101 追記 219〜221: panel（Chat と Terminal の tab・会話の列 chat_column）は 1 つで置き場が 2 つ = 右なら右の sidebar に全部・下には描かない（同じ tab を 2 か所に出さない）
            let (side,saved_w)=(self.store.ui.chat_place==ChatPlace::Right,self.store.ui.chat_width.map_or(ui::agent_rail::SIDE_W,f32::from));
            let mut width=saved_w;
            let mut column=|ui:&mut egui::Ui|{
                if let Some(i)=ui::agent_rail::chat_column(ui,shell,&turns,card.as_ref(),wait,agent,&rows,busy,side){said=Some(i);}
            };
            let act=self.generated.draw_terminal_panel(ctx,&project,setup,Some(&mut column),side.then_some(&mut width));
            // CHAT-SIDE-1101（operator・owner 189）: 境を引き終えて幅が替わったら保存（開き直しても同じ幅）
            let w=width.round().clamp(ui::agent_rail::SIDE_W_MIN,ui::agent_rail::SIDE_W_MAX) as u16;
            if (f32::from(w)-saved_w).abs()>=1.0{self.store.ui.chat_width=Some(w);self.mark();}
            if let Some(i)=said{intents.push(i);}
            if matches!(act,Some(PanelAct::Chat|PanelAct::Toggle))&&self.generated.panel(&project).chat&&self.generated.panel(&project).open{self.shell.focus_command=true;}
        }
        // HOME-TERM-1101（owner 208）: Project の外の View の下 = Home の Terminal（Project の panel と同じ部品・~/Shibubu で始まる）。CentralPanel より先に取る。
        // Home では毎 frame（畳んでいれば描かず状態と通知だけ = PANEL-SYNC-1101・tab は窓の一番下の帯）・Runs / Connections / Settings では開いている間だけ（そこで押した sign in が開く = 画面は移らない）
        let outside=matches!(self.shell.view,View::Home|View::Runs|View::Connections|View::Settings(_));
        if outside&&(self.shell.view==View::Home||self.generated.panel(crate::generated::HOME_PLACE).open){
            self.generated.draw_terminal_panel(ctx,crate::generated::HOME_PLACE,None,None,None);
        }
        // RQ5（RQ-021）: Preview は差分だけ（何も書かない）。主領域の下に 1 つの面で出し、Apply / Later / Hide / Close は Intent::Proposal へ
        if let (View::Project{..},Some(p))=(&self.shell.view,self.agent.preview().cloned()){
            egui::TopBottomPanel::bottom("kangae-suggestion-preview").resizable(false).show(ctx,|ui|{
                egui::ScrollArea::vertical().max_height(280.0).show(ui,|ui|{if let Some(i)=ui::agent_rail::draw_preview(ui,&p){intents.push(i);}});
            });
        }
        let t=theme::tokens(ctx);
        let fill=if in_compat{t.surface_1}else{t.bg_app};
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill)).show(ctx,|ui|{
            // FX1: 作業の場所の頭の 1 行（どの View でも）— 終わる前の保存の失敗（Quit anyway / Keep open）・開き直した後の隔離した store（Reveal in Finder）
            let head=egui::Frame::new().inner_margin(egui::Margin::symmetric(tier.gutter as i8,theme::SP_2 as i8));
            if let Some(why)=self.quit_failed.clone(){
                head.show(ui,|ui|match theme::quit_inline(ui,&why){
                    Some(true)=>intents.push(Intent::Quit{anyway:true}),
                    Some(false)=>self.quit_failed=None,
                    None=>{}
                });
            }else if let Some(aside)=self.store.set_aside.clone(){
                head.show(ui,|ui|ui.horizontal(|ui|{
                    let name=aside.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    theme::label(ui,Type::Helper,format!("Damaged layout set aside as {name}"),t.text_secondary);
                    if theme::button(ui,"Reveal in Finder",Variant::Ghost).clicked(){
                        if let Err(e)=crate::generated::run_open("/usr/bin/open",&["-R",&aside.to_string_lossy()]){self.message=e;}
                    }
                }));
            }
            match self.shell.view.clone(){
                // owner 191（追記 5）: 会話の間の Home は主領域の幅いっぱい（左右は gutter だけ・文の列の 1080 で止めない）
                View::Home=>ui::shell::draw_region(ui,if self.shell.talk.is_empty(){theme::MAX_TEXT_COLUMN}else{f32::INFINITY},&mut |ui|{
                    // FX2: CLI を確かめている間（待ちの理由 1 行・Stop = 待ちを捨てる）→ Home の生成（段・経過・Stop／止めている／終わった 1 行と次の一手）→
                    // Page の変更を読んでいる間（Home では止められない = 押せる物 0 の 1 行・欄は送れるが create_app が受けずに文を戻す）。
                    // G99: AI を探し直している間（`llm_rechecking`）も同じ 1 行の段で出す
                    let checking=self.waiting.as_ref().map(|(_,_,since)|Build::Running{phase:Phase::Read,planned:&[],waiting:Some("Checking for AI on this Mac"),elapsed:since.elapsed()})
                        .or_else(||self.llm_rechecking.map(|why|Build::Running{phase:Phase::Read,planned:&[],waiting:Some(why),elapsed:self.llm_checked.elapsed()}));
                    let editing=self.generated.model_busy_line().filter(|_|self.generated.model_progress().is_none()).map(|line|Build::Ended{state:RegionState::Loading,line,action:None});
                    let build=checking.or_else(||self.generated.model_progress()).or(editing);
                    // G99: 探している間は前の 1 行と button を出さない（押した後に古い Install / Sign in が残らない）
                    let llm=self.generated.llm_line().filter(|_|self.llm_rechecking.is_none()).map(|l|(l,if self.generated.llm_ready(){None}else{crate::llm_cli::next_step(l)}));
                    if let Some(i)=ui::home::draw(ui,self.generated.note.as_deref(),&rows,&mut self.shell,build.as_ref(),llm){intents.push(i);}
                }),
                View::Runs=>{
                    let entries=self.generated.run_entries();
                    let view=ui::runs::RunsView{entries:&entries,loading:false,now_unix:ui::runs::RunsView::now()};
                    let mut pressed=None;
                    ui::shell::draw_app_region(ui,&mut |ui|{pressed=ui::runs::show(ui,&view);});
                    if let Some(a)=pressed{
                        // FX7: `Open project` はその run の Project を開く（Runs の外へ移る）
                        if a.action==ui::runs::RunAction::OpenProject{
                            match entries.iter().find(|e|e.id==a.run_id).and_then(|e|e.project.clone()).map(|p|self.generated.open_project(&p,None)){
                                Some(Ok(v))=>intents.push(Intent::Go(v)),
                                Some(Err(e))=>ui::runs::note(ctx,&a.run_id,&e),
                                None=>ui::runs::note(ctx,&a.run_id,"This run isn't part of a project."),
                            }
                        }
                        // FX2: 生成の行の Stop は job の印（runtime の run ではない）
                        else if let Some(line)=self.generated.stop_from_runs(&a){ui::runs::note(ctx,&a.run_id,&line);}
                        else{
                            let (connect,line)=self.generated.run_action(&a);
                            if connect{intents.push(Intent::Go(View::Connections));}else{ui::runs::note(ctx,&a.run_id,&line);} // G94: 結果は押した行の詳細の下（sidebar の最下行でなく）
                        }
                    }
                }
                View::Connections=>{
                    let model=self.connections_model();
                    // ACCOUNTS-1101: AI providers の群の中身 = Settings › AI & Models の Accounts と同じ部品（`settings::accounts`）
                    let (signing,(emails,unsigned))=(self.signing_line(),self.account_marks());
                    let (shell,settings)=(&mut self.shell,&self.store.ui);
                    ui::shell::draw_app_region(ui,&mut |ui|{if let Some(i)=ui::connections::draw(ui,&model,shell,&mut |ui|crate::settings::accounts(ui,settings,signing,&emails,&unsigned)){intents.push(i);}});
                }
                // RQ1: Settings（入口の ⌘,・app menu・⌘K は本体で配線・interface の段は届かない腕）
                View::Settings(section)=>{
                    // 節に要る物だけ作る（About の行は store と runs を読むので About の間だけ）
                    let ai=if section==SettingsSection::AiModels{self.generated.ai_rows()}else{Vec::new()};
                    let used=self.generated.budget_used();
                    let mut about=if section==SettingsSection::About{self.generated.diagnostics(&self.connections_model().rows)}else{Vec::new()};
                    // RQ6（UIUX §6.4）: Shibubu が前面の app を奪った回数（この起動の間）
                    if section==SettingsSection::About{about.push(("Focus taken from you".into(),crate::adapters::mac_gui::focus_taken_count().to_string()));}
                    // L1-NORAIL: `{name} ⌄` の Project settings の行き先 = 頭に今の Project（UiSettings::last_project）の 3 行（Folder access・Web sign-ins・Approvals）
                    if section==SettingsSection::About{
                        if let Some(rows)=self.store.ui.last_project.as_deref().and_then(|p|self.generated.project_settings_rows(p).ok()){about.splice(0..0,rows);}
                    }
                    // RQ3: Data & Recovery の中身（大きさ・世代・Import の候補）は refresh_data が裏で読む（開いている間 30 秒ごと・届いたら thread が起こす）
                    if section==SettingsSection::Data{self.refresh_data(false);ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));}
                    // RQ-USAGE: 読みは frame の頭（帯と同じ・60 秒ごと）
                    let usage=if section==SettingsSection::AiModels{self.usage_view(false)}else{None};
                    let data=if section==SettingsSection::Data{self.generated.data_summary()}else{None};
                    // RQ6: Privacy を開いている間だけ同意の file を読む（小さな file）
                    let allowed:Vec<String>=if section==SettingsSection::Privacy{let mut v:Vec<String>=crate::surface::act_consents(&crate::surface::act_consent_path()).into_iter().collect();v.sort();v}else{Vec::new()};
                    let mirror_allowed:Vec<String>=if section==SettingsSection::Privacy{self.generated.mirror_consents()}else{Vec::new()};
                    // RQ-ACCT: sign in の途中の 1 行（Add / Re-authenticate を出さない）と、足した plan の account の email（画面だけ・設定に書かない）
                    let signing=self.signing_line();
                    let (emails,unsigned)=if section==SettingsSection::AiModels{self.account_marks()}else{(Vec::new(),Vec::new())};
                    let settings=&mut self.store.ui;let confirm=&mut self.shell.data_confirm;
                    // 段 2 作り直し: Settings › Terminal は panel の 1 行目と同じ部品（app の持ち物・Generated の外）
                    let mut setup=self.agent_setup.as_mut();
                    ui::shell::draw_app_region(ui,&mut |ui|{if let Some(i)=crate::settings::draw(ui,section,settings,&ai,used,usage.as_ref().map(|(l,s)|(l.as_str(),s.clone())),signing,&emails,&unsigned,&about,data,confirm,&allowed,&mirror_allowed,setup.as_deref_mut(),own.as_ref()){intents.push(i);}});
                }
                View::App(id)=>ui::shell::draw_app_region(ui,&mut |ui|{
                    if self.generated.draw(ui,&id){intents.push(Intent::Go(View::Home));}
                }),
                // LAYOUT（門 2）: 面の在る Page は文の列の 1080 で止めない（REAPER の列が窓の幅まで）
                View::Project{project,page}=>ui::shell::draw_region(ui,if self.generated.wide_page(&project,&page){f32::INFINITY}else{ui::theme::MAX_TEXT_COLUMN},&mut |ui|{
                    // RQ2（RQ-005）: {name} ⌄ の Delete… の確認は Project の頭（1 回目で消さない・Keep と Esc は何もしない）
                    if let Some(i)=self.shell.confirm_delete(ui,&project,||self.generated.app_of_project(&project).and_then(|a|self.generated.title(&a)).unwrap_or_default()){intents.push(i);}
                    if let Some(i)=self.generated.draw_page_body(ui,&project,&page){intents.push(i);}
                }),
                View::CompatCanvas(id)=>{
                    if self.store.sessions.iter().any(|s|s.id==id){self.compat_canvas(ui);}
                    else{ui::shell::draw_app_region(ui,&mut |ui|{
                        if ui::states::region(ui,RegionState::Empty,"This project was deleted",Some("Go to Home")){intents.push(Intent::Go(View::Home));}
                    });}
                }
            }
        });
        // FX3（FX-008）: Project の画面の時は ⌘K に今の Project の全 Page（帯の 5 つに見えない Page へも行ける）
        let pages=match &self.shell.view{View::Project{project,..} if self.shell.palette.open=>self.generated.page_commands(project),_=>Vec::new()};
        if let Some(i)=ui::palette::draw(ctx,&mut self.shell,&rows,&pages){intents.push(i);}
        self.approval(ctx);self.import_dialog(ctx); // 旧 canvas の削除の確認は UI-5a で CompatCanvas の中の inline の行（delete_prompts の Window は無い）
        // W12W-1: Page に置いた WebView を合わせる（子の NSView は egui より上に乗る）。MIRROR-FIT-1101 追記 210: egui が上に描く物（toast・popup・menu・palette・
        // 承認と取り込みの窓）は WebView に穴として抜く = 隠さない。cover（WebView ごと隠す）は Area を持たない削除の確認だけ
        let holes=crate::project::web::overlay_holes(ctx,&[egui::Id::new("approval"),egui::Id::new("import")]);
        let cover=self.delete_session_prompt||self.delete_panel_prompt.is_some();
        // egui の欄が focus を取った frame（AX の AXPress・Tab）は first responder を egui の view へ戻す: AX の focus は AppKit の first responder を動かさず、
        // 戻さないと打った key が WebView の中の第三者の頁の欄に届く（W12W-3 run-1）
        let focus=ctx.memory(|m|m.focused());
        let took=focus.is_some()&&focus!=self.last_focus;
        if self.generated.web.sync(frame,cover,&holes,took){
            // first responder が WebView の中（人の click・autofocus）: egui の focus を外す（focus の枠を二重に出さない）
            ctx.memory_mut(|m|if let Some(id)=m.focused(){m.surrender_focus(id);});
        }
        // TERM-XTERM-1101: Terminal の中身の xterm の頁（下の panel・Home の Terminal）も同じ所・同じ cover と穴で。頁の中に first responder が在れば egui の focus を外す
        #[cfg(all(feature="webview",target_os="macos"))]
        if self.generated.term_sync(ctx,frame,cover,&holes,took){
            ctx.memory_mut(|m|if let Some(id)=m.focused(){m.surrender_focus(id);});
        }
        self.last_focus=ctx.memory(|m|m.focused());
        #[cfg(all(feature="webview",target_os="macos"))]self.canvas_sync(ctx,frame,cover,&holes,&mut intents);
        // W12W-B: 頁の中で URL が替わったら revision に残す（同じ URL は take_url_writes が毎 frame 出さない）
        if let View::Project{project,..}=self.shell.view.clone(){
            for (key,url) in self.generated.web.take_url_writes(){
                let sid=crate::project::web::sid_of(&key).to_owned();
                let _=self.generated.page_intent(&project,PageIntent::SetSurfaceUrl{surface_id:sid,input:url});
            }
        }
        for i in intents{self.apply(ctx,i);}
        // 1 つの欄の変更の文が通った（同期・裏の model・Engine に渡した）: その Project の打ちかけを空にする（拒否なら文は残る・UI-4a）
        if let Some(p)=self.generated.take_edit_applied(){
            if matches!(&self.shell.view,View::Project{project,..} if *project==p){self.shell.command.clear();}else{self.shell.drafts.remove(&p);}
            self.mark(); // UI-4b: 空にした打ちかけを次の保存で UiSettings::drafts からも消す
            ctx.request_repaint();
        }
        // NOTIFY-1101 追記（owner 192）: Engine の失敗・面の状態が替わった事・編集で AI を使えなかった理由（Generated が積む）も同じ toast へ。
        // frame の終わりで取る（poll・Page の描画・手の適用のどれで積んでも同じ frame で取り、post が次の frame を頼む）
        for (text,error) in self.generated.take_notices(){ui::toast::post(ctx,text,error,None);}
        #[cfg(feature="mirror")]self.generated.end_frame();
        self.save_if_due();
        if let Some(d)=self.next_repaint(){ctx.request_repaint_after(d);}
    }
    /// FX1: 保存の判断は `Intent::Quit` が終わる前に済ませる。ここは `save_now` をもう 1 回試すだけ（Quit を経ない終わり方 =
    /// logout の terminate など・失敗は stderr に 1 行・画面はもう無い）と、Terminal と Mirror の後始末。
    /// Wave: FX1
    fn on_exit(&mut self){
        // 今の Page の入力途中・場所ごとの打ちかけ・sessions.json（save_now）。画面はもう無いので失敗は stderr に 1 行
        if let Err(e)=self.save_now(){eprintln!("shibubu: couldn't save on exit · {e}");}
        // Project の Terminal を閉じる（eframe は終了で app を drop しない事がある・Drop にだけ頼らない）
        self.generated.close_terminals();
        #[cfg(feature="mirror")]self.generated.release_mirrors(); // G91: しまった本物の窓を元の位置へ
    }
}

/// FX5（FX-029）: OS の許可の行。許可は画面を見るたびに読む（押しても何も起きない `Check again` を持たない）ので、許していない時だけ `Open settings` と `Check again`
/// （`Check again` は同じ読み直し = `Intent::RecheckGui`）。Microphone は最初に録る時に OS が聞くので、ここでは事実の 1 行だけ（読み取りの口を足さない）。
/// Files and Folders は要る手が出た時だけ（今の build に要る手が無い）なので行を作らない。macOS 以外は行を作らない。
#[cfg(target_os="macos")]
fn os_grant_rows()->Vec<Connection>{
    #[link(name="CoreGraphics",kind="framework")]
    unsafe extern "C"{fn CGPreflightScreenCaptureAccess()->bool;}
    #[link(name="ApplicationServices",kind="framework")]
    unsafe extern "C"{fn AXIsProcessTrusted()->bool;}
    // 理由: crate を足さずに許可を読む。前提: どちらも許可の有無を返すだけで、dialog を出さず、他の process に何も送らない
    let (screen,ax)=unsafe{(CGPreflightScreenCaptureAccess(),AXIsProcessTrusted())};
    let row=|name:&'static str,on:bool,url:&'static str|Connection{name:name.into(),
        state:if on{DisplayTier::Ready}else{DisplayTier::Available},
        detail:if on{"Allowed".into()}else{"Off · Open settings".into()},
        recheck:!on,checking:false,settings:(!on).then_some(url),
        key:name.to_ascii_lowercase().replace(' ',"-").into(),
        acts:if on{Vec::new()}else{vec![crate::ui::connections::ConnAct::CheckAgain]},
        checked_at:None};
    vec![
        row("Screen Recording",screen,"x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"),
        row("Accessibility",ax,AX_SETTINGS),
        Connection{name:"Microphone".into(),state:DisplayTier::Available,detail:"Asked the first time you record".into(),recheck:false,checking:false,settings:None,key:"microphone".into(),acts:Vec::new(),checked_at:None},
    ]
}
#[cfg(not(target_os="macos"))]
fn os_grant_rows()->Vec<Connection>{Vec::new()}

/// Connections の「画面の操作」の行の名前（YC W17-2 #4）。
const GUI_ROW:&str="Screen control";
/// その行の `Open settings` の行き先 = System Settings › Privacy & Security › Accessibility（sendback 68）。
const AX_SETTINGS:&str="x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
/// Connections の行の状態（#4・#10）: registry（`AdapterRegistry::desktop`）の `check` → `tier`。MacGui を名指さない（R-G2・§1「汎用 Core と Adapter」）。
/// 新しい registry の tier は `tier_with(readiness, None)` と同じ（R-G2 の前の値と変わらない）。理由の 1 行と次の一手（D-e）。cua-driver を起こさない。
fn gui_status(r:&mut AdapterRegistry)->(DisplayTier,String){
    r.check();
    let tier=r.tier("desktop.ax.click");
    let detail=match tier{
        DisplayTier::Ready|DisplayTier::Verified=>"Running, with Accessibility · clicks not verified yet".into(),
        _=>"Screen control didn't start, or Accessibility is off · Open settings".into(),
    };
    (tier,detail)
}
/// 旧 Desktop パネルの読みの caller（承認の要らない読みだけ・承認の照合に使われない）
const PANEL:Caller<'static>=Caller{app_id:"compat-desktop",run_id:None};
/// registry の読みの結果 → payload の型。読めない時は Adapter の 1 行（許可・bridge が無い: operator 2026-10-23 (B)）か `sdk::legacy` の 1 行
fn payload<T:serde::de::DeserializeOwned>((o,obs):(Outcome,Option<Observation>))->Result<T,String>{
    match obs{
        Some(obs)=>serde_json::from_value(obs.payload).map_err(|e|format!("Shibubu can't read this screen's format. Reopen Shibubu ({e})")),
        None=>Err(sdk::legacy(o).err().unwrap_or_else(||"Couldn't read the screen. Press again".into())),
    }
}
/// 旧 Desktop パネルの ①（YC_GATE #5）: registry の `desktop.windows.list`（Core は MacGui を名指さない・W19-1）= bridge の apps + window_list で窓を読む（cua-driver を起こさない）。
fn desktop_windows(r:&mut AdapterRegistry)->Result<Vec<WindowRef>,String>{
    r.check();
    let ws:Vec<WindowRef>=payload(r.invoke(PANEL,"desktop.windows.list",&serde_json::json!({}),None))?;
    if ws.is_empty(){return Err("No window Shibubu can identify · Open a window, then press Find windows again".into())}
    Ok(ws.into_iter().map(|w|WindowRef{title:if w.title.trim().is_empty(){format!("{} · Untitled",w.app_name)}else{w.title.clone()},..w}).collect())
}
/// 旧 Desktop パネルの ②: registry の `desktop.window.state`（payload は `gui::WindowState`・各要素の ref は `Control::element`）。押せる / 値を置ける・
/// label のある要素だけ（窓そのものは payload に入らない）。label が空の要素は ③ の登録・実行（label+role の照合）で探し直せないので出さない。
fn desktop_controls(r:&mut AdapterRegistry,w:&WindowRef)->Result<Vec<Control>,String>{
    r.check();
    let s:WindowState=payload(r.invoke(PANEL,"desktop.window.state",&serde_json::json!({"pid":w.pid,"window_id":w.window_id,"app_name":w.app_name}),None))?;
    let out:Vec<Control>=s.controls.into_iter().filter(|c|!c.label.trim().is_empty()&&(!c.actions.is_empty()||c.value_settable)).collect();
    if out.is_empty(){return Err(format!("No controls to press in the window of {}",s.app_name))}
    Ok(out)
}
/// 旧 lane の Desktop の 1 操作 → registry の action id と入力（承認の digest と実行が同じ物を見る）
fn desktop_call(kind:&ActionKind)->Option<(&'static str,serde_json::Value)>{
    let ActionKind::Desktop{pid,window_id,label,role,tool,text,app_name}=kind else{return None};
    let mut input=serde_json::json!({"pid":pid,"window_id":window_id,"app_name":app_name,"label":label,"role":role});
    if *tool==DesktopTool::TypeText{input["text"]=serde_json::json!(text);return Some(("desktop.ax.type_text",input))}
    Some(("desktop.ax.click",input))
}
/// (A) 旧 lane の「確認して実行」1 回 = 1 つの ConsentReceipt（operator 2026-10-23）。scope は registry の契約・digest は registry と同じ `sdk::payload_digest`・
/// run_id はこの 1 回だけの新しい id（registry は同じ承認で 2 回実行しない）。画面の操作でない kind は None。
fn desktop_consent(app_id:&str,kind:&ActionKind)->Option<ConsentReceipt>{
    let (action,input)=desktop_call(kind)?;
    let scopes=AdapterRegistry::desktop().contract(action)?.scopes.iter().map(|s|s.to_string()).collect();
    let run_id=crate::storage::new_id().to_string();
    Some(ConsentReceipt{id:format!("lane-{run_id}"),app_id:app_id.into(),subject:ConsentSubject::Action{action_ref:action.into(),run_id:Some(run_id)},
        scopes,payload_digest:Some(sdk::payload_digest(action,&input)),decision:ConsentDecision::Approved,decided_at:crate::google::rfc3339_utc(sdk::now())})
}
/// ③（YC_GATE #3・#6）: registry の `desktop.ax.click` / `desktop.ax.type_text`（gate: 確認 → readiness → 承認 → MacGui）。承認の無い・別の内容への
/// 承認・app 名の無い旧 binding は送らず 1 行（D-e）。
fn desktop_execute(r:&mut AdapterRegistry,kind:&ActionKind,consent:Option<&ConsentReceipt>)->Result<String,String>{
    let Some((action,input))=desktop_call(kind) else{return Err("Not a screen action".into())};
    let run_id=consent.and_then(|c|match &c.subject{ConsentSubject::Action{run_id,..}=>run_id.as_deref(),_=>None});
    let app_id=consent.map_or("",|c|c.app_id.as_str());
    r.check();
    sdk::legacy(r.invoke(Caller{app_id,run_id},action,&input,consent).0)
}

#[cfg(test)]
impl ShibubuApp {
    pub(crate) fn test_set_automation(&mut self, mode: crate::domain::permission::AutomationMode) {
        self.store.ui.automation = mode;
    }
    pub(crate) fn test_clear_llm_rx(&mut self) { self.llm_rx = None; }
    pub(crate) fn test_generated(&mut self) -> &mut Generated { &mut self.generated }
}

#[cfg(test)]
pub(crate) mod tests {
    use eframe::egui::{self,accesskit::{Action,ActionRequest,NodeId,Role},Event,Pos2,RawInput,Rect,Vec2,ViewportId};
    use std::time::{Duration,Instant};

    /// L2（109 通目 15・111 通目 = handoff 00 #15 / 05）: 新しい data dir の Home で hello を Enter → Project は増えず、問いと 3 つの選択肢（button）が会話に出る・案の card は出ない。
    /// ⌘↵ と右端の `Send`（AX の名前 `Send`）の hello も作らずに会話へ。選択肢を押すと案の card（`Proposal`・最初から `Build now`・`Preview plan`）→ `Preview plan` で
    /// 読むだけの行 → `Build now` を押した時だけ Project が 1 つできる（会話は閉じる・TALK-1101）。`Later` は案を閉じて会話を残し、`Change plan` は案の文を欄へ。
    /// 負の対照: `enter_intent` の Home の腕を `Generate` に戻す → Enter の hello で赤。`enter_intent` の ⌘↵ の門から `self.view != View::Home` を外す → ⌘↵ の hello で赤。
    #[test]
    fn l2_hello_talks_and_only_accept_creates() {
        use crate::domain::chat::{BUILD_NOW, CHANGE_PLAN, LATER, NEW_CHAT, PLAN_UNMADE, PREVIEW_PLAN, PROPOSAL, TALK_ASK};
        use crate::ui::shell::View;
        let mut w = Win::open("l2-talk");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli { detail: "test".into() }));
        let before = w.app.generated.rows().len();
        w.submit("What to create", "hello", false);
        assert_eq!(w.app.shell.view, View::Home, "hello で画面が替わった");
        assert_eq!(w.app.generated.rows().len(), before, "Enter の hello で Project が増えた");
        let out = w.step(0.1, true, vec![]);
        let words = ax_words(&out);
        assert!(words.iter().any(|l| l == TALK_ASK), "問いが画面に無い: {words:?}");
        for c in ["Track my inventory", "Keep research notes", "I'll describe it", NEW_CHAT] {
            assert!(Win::button(&out, c).is_some(), "「{c}」の button が無い: {words:?}");
        }
        for c in [PREVIEW_PLAN, BUILD_NOW] {
            assert!(Win::button(&out, c).is_none(), "案の無い hello に {c} が出た");
        }
        assert!(!words.iter().any(|l| l == PROPOSAL), "案の無い hello に Proposal の card が出た");
        // ⌘↵ の hello・右端の Send の hello: 作らずに会話へ（それぞれ人の行と問いの 2 行が増える）
        w.submit("What to create", "hello", true);
        assert_eq!(w.app.generated.rows().len(), before, "⌘↵ の hello で Project が増えた");
        assert_eq!(w.app.shell.talk.len(), 4, "⌘↵ の hello が会話にならない: {:?}", w.app.shell.talk);
        let field = Win::home_field(&w.step(0.1, true, vec![]));
        w.step(0.1, true, vec![
            Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: field, data: None }),
            Event::AccessKitActionRequest(ActionRequest { action: Action::SetValue, target: field, data: Some(egui::accesskit::ActionData::Value("hello".into())) }),
        ]);
        let send = Win::button(&w.step(0.1, true, vec![]), "Send").expect("右端の Send");
        ax_click(&mut w, send);
        assert_eq!(w.app.generated.rows().len(), before, "Send の hello で Project が増えた");
        assert_eq!(w.app.shell.talk.len(), 6, "Send の hello が会話にならない: {:?}", w.app.shell.talk);
        w.app.apply(&w.ctx.clone(), Intent::AcceptPlan);
        assert_eq!(w.app.generated.rows().len(), before, "案の無い Build now が作った");
        // 選択肢 → 案の card（TALK-1101: Build now が最初から先頭・Preview plan は中身を開くだけ）
        let chip = Win::button(&w.step(0.1, true, vec![]), "Keep research notes").expect("選択肢");
        ax_click(&mut w, chip);
        assert_eq!(w.app.generated.rows().len(), before, "選択肢を押しただけで作った");
        let out = w.step(0.1, true, vec![]);
        assert!(ax_words(&out).iter().any(|l| l == PROPOSAL), "案の card が無い: {:?}", ax_words(&out));
        assert!(Win::button(&out, BUILD_NOW).is_some() && Win::button(&out, PREVIEW_PLAN).is_some(), "案の card に Build now と Preview plan が無い: {:?}", ax_words(&out));
        // Later: 案を閉じて会話は残す・Change plan: 案の文を欄へ（作らない）
        let talk = w.app.shell.talk.len();
        ax_click(&mut w, Win::button(&out, LATER).expect("Later"));
        assert_eq!((w.app.shell.talk.len(), crate::domain::chat::pending_proposal(&w.app.shell.talk)), (talk + 1, None), "Later が会話を消した・案が残った・1 行を足さない");
        // CHAT-011 反証 Y2: Later の後に何が起きたかの 1 行と案の文が会話に残る
        assert_eq!(w.app.shell.talk.last().map(|t| t.text.as_str()), Some(format!("{} · Keep research notes", crate::domain::chat::LATER_SAID).as_str()), "Later が黙って案を消した");
        // Later の後は選択肢を出さない（案の返事に選択肢は無い）= 同じ文を打って送り直す
        w.submit("What to create", "Keep research notes", false);
        let change = Win::button(&w.step(0.1, true, vec![]), CHANGE_PLAN).expect("Change plan");
        ax_click(&mut w, change);
        assert_eq!((w.app.shell.command.as_str(), w.app.generated.rows().len()), ("Keep research notes", before), "Change plan が案の文を欄に入れない・作った");
        w.app.shell.command.clear();
        // Preview plan: 読むだけの行（何も作らない）→ Build now を押した時だけ作る
        let preview = Win::button(&w.step(0.1, true, vec![]), PREVIEW_PLAN).expect("Preview plan");
        ax_click(&mut w, preview);
        assert_eq!(w.app.generated.rows().len(), before, "Preview plan で作った");
        let out = w.step(0.1, true, vec![]);
        assert!(ax_words(&out).iter().any(|l| l == PLAN_UNMADE), "Preview の行が無い: {:?}", ax_words(&out));
        ax_click(&mut w, Win::button(&out, BUILD_NOW).expect("Preview の後の Build now"));
        for _ in 0..3 {
            w.step(0.1, true, vec![]);
        }
        assert_eq!(w.app.generated.rows().len(), before + 1, "Build now で 1 つ作っていない: {:?} {:?}", w.app.shell.view, w.app.generated.note);
        assert!(w.app.shell.talk.is_empty(), "作った後も会話が残る");
    }

    /// review A・C（sub operator の独立 review 10-31）: 案の Build now（TALK-1101・前の Accept）を画面の道で押す（Preview を押さずに）。
    /// A = 変更の形の案（「〜 Page を足して」= EDIT_WORDS）の Build now は、いちばん新しい Project の書き換えにも、Project 0 件の時の「開発環境」の Project にもならない
    /// （AI が無ければ作らずに理由の 1 行・AI が在れば AI が読む道 = Preview の読みが始まる・Preview で読めなかった案の Build now も model の job が始まる）。
    /// C = 案で作った Project の会話の頭は Home の会話（案の手〈選択肢・Build now〉は写さない・案の文は Shibubu の行に `Plan:`）・最後の行は作った物と次の一手。
    /// 負の対照: A1 = `create_project` の `&& !self.planned(goal)` を外す → AI 無しの Build now で開発環境の Project ができて赤・A2 = `create_app` の `accepted||` を外す →
    /// Preview で読めなかった案の Build now が model の job を始めず赤・C = `save_plan` の `plan_talk` の腕を外す → 会話の頭が案の文で赤。
    #[test]
    fn l2_accept_reads_the_plan_and_carries_the_talk_into_the_project() {
        use crate::domain::chat::{Mark, Turn, Who, BUILD_NOW, PREVIEW_PLAN};
        use crate::ui::shell::View;
        let turn = |who, text: &str, proposal: Option<&str>| Turn { who, text: text.into(), mark: Mark::None, at: "2026-10-31T00:00:00Z".into(), attachments: Vec::new(), choices: vec!["x".into()], proposal: proposal.map(str::to_owned), preview: Vec::new(), sections: Vec::new(), needs: None };
        let press = |w: &mut Win, name: &str| {
            let id = Win::button(&w.step(0.1, true, vec![]), name).unwrap_or_else(|| panic!("{name} が無い"));
            ax_click(w, id);
        };
        let accept = |w: &mut Win| {
            press(w, BUILD_NOW);
            for _ in 0..3 {
                w.step(0.1, true, vec![]);
            }
        };
        let edit = "notes.md を Mirror で見る Page を足して";
        assert!(crate::generated::said_is_edit(edit), "前提: 「{edit}」は変更の文");
        let mut w = Win::open("l2-accept");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli { detail: "test".into() }));
        assert_eq!(w.app.generated.rows().len(), 0, "前提: Project 0 件の新しい data dir");
        // A（AI 無し）: 作らない（開発環境の Project を作って当てない）・Home に残り理由の 1 行
        w.app.shell.talk = vec![turn(Who::You, "hello", None), turn(Who::Shibubu, "Shall I build this?", Some(edit))];
        accept(&mut w);
        assert_eq!((w.app.generated.rows().len(), &w.app.shell.view), (0, &View::Home), "変更の形の案の Build now が Project を作った: {:?}", w.app.generated.note);
        // C: 主デモの案を Build now → 作った Project の会話の頭は Home の会話
        let demo = crate::ui::home::EXAMPLES[0].1;
        w.app.shell.talk = vec![turn(Who::You, "hello", None), turn(Who::Shibubu, "What would you like to make?", None), turn(Who::You, "A place to work on this repo", None), turn(Who::Shibubu, "Shall I build this?", Some(demo))];
        accept(&mut w);
        let View::Project { project, .. } = w.app.shell.view.clone() else { panic!("案の Build now で Project に移らない: {:?} {:?}", w.app.shell.view, w.app.generated.note) };
        let chat = w.app.generated.chat(&project);
        let said: Vec<&str> = chat.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(said.get(..3), Some(&["hello", "What would you like to make?", "A place to work on this repo"][..]), "Project の会話の頭が Home の会話でない: {said:?}");
        assert_eq!(said.get(3).copied(), Some(format!("Shall I build this?\n\nPlan: {demo}").as_str()), "案の文が残らない: {said:?}");
        assert!(chat.iter().all(|t| t.proposal.is_none() && t.choices.is_empty()), "案の手を Project の列に写した: {chat:?}");
        let last = said.last().unwrap();
        assert!(last.starts_with("Made ") && last.contains("Next, say what to change"), "最後の行が作った物と次の一手でない: {last}");
        assert!(w.app.shell.talk.is_empty(), "作った後も Home の会話が残る");
        // A（AI 在り）: 変更の形の案の Preview は AI が読む道（読む間は Build now が灰・何も書かない・いちばん新しい Project を書き換えない）
        let pages = |w: &Win| { let a = w.app.generated.app_of_project(&project).unwrap(); w.app.generated.project_of(&a).unwrap().unwrap().0.pages.len() };
        let n = pages(&w);
        w.app.go(View::Home);
        w.app.generated.set_llm(Ok(Box::new(Waits(std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0))))));
        w.app.shell.talk = vec![turn(Who::You, "add a mirror page", None), turn(Who::Shibubu, "Shall I build this?", Some(edit))];
        press(&mut w, PREVIEW_PLAN);
        assert_eq!(w.app.generated.model_busy_line(), Some("Reading your plan"), "AI 在りの Preview が AI の読みを始めない: {:?}", w.app.shell.talk.last());
        let out = w.step(0.1, true, vec![]);
        let disabled = out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(BUILD_NOW)).map(|(_, n)| n.is_disabled());
        assert_eq!(disabled, Some(true), "読む間に Build now が押せる（か無い）");
        w.app.generated.stop_model_job();
        let t0 = Instant::now();
        while w.app.generated.model_busy_line().is_some() {
            assert!(t0.elapsed() < Duration::from_secs(30), "止めた読みが終わらない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(w.app.shell.talk.last().is_some_and(|t| t.preview.is_empty()), "止めた Preview の card が Preview plan に戻らない");
        // A（AI 在り）: Preview で読めなかった変更の形の案の Build now も AI が読む道（model の job が始まる・いちばん新しい Project を書き換えない）
        w.app.shell.talk.last_mut().unwrap().preview = vec!["AI wasn't available · test".into()];
        press(&mut w, BUILD_NOW);
        assert_eq!(w.app.generated.model_busy_line(), Some("Reading your request"), "AI 在りの案の Build now が AI の道を通らない: {:?}", w.app.generated.note);
        assert_eq!(pages(&w), n, "案の Build now がいちばん新しい Project を書き換えた");
        w.app.generated.stop_model_job();
    }

    /// L2（review A・DOGFOOD-X 周 2 回 1 #1・#2 = 473f4037 の .app で本物の AI）: AI が在る時の `Preview plan` は作る時と同じ読みを 1 回して、
    /// 作る Page と入る app を読むだけの行で見せる・答えていない問いは案の card に残る・`Build now` は AI を呼び直さずその案を保存する（見せた Page が出来る）・
    /// Project の会話の頭は Home の会話（問いも残る）で、最後の行は名前を 1 回だけ言う（Page 1 枚で名前と同じ = `Made X: X` にしない）。
    /// 負の対照: P = `create_project` の `previewed` の腕を外す → Build now が 2 回目を呼び（Fake の答えが尽きる）見せた案と違う物で赤・
    /// Q = `talk_thread` の `open_question` の行を外す → card に問いが無く赤・N = `save_plan` の 1 枚の腕を外す → 名前 2 回で赤・
    /// C（dogfood r6・未測）= `open_made` の `show_chat` の行を外す → Chat が畳まれたままで赤。
    #[test]
    fn l2_preview_shows_the_planned_pages_and_apps_and_accept_saves_them() {
        use crate::domain::chat::{Mark, Turn, Who, BUILD_NOW, OPEN_QUESTION, PLAN_READING, PLAN_UNMADE, PREVIEW_PLAN};
        use crate::ui::shell::View;
        let seat = |title: &str, kind: &str, app: Option<&str>, url: Option<&str>| serde_json::json!({ "title": title, "kind": kind, "app_name": app, "bundle_id": app.map(|_| "com.apple.TextEdit"), "file": null, "url": url, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null });
        let plan = serde_json::json!({ "shape": "project", "name": "Weekly meeting notes", "pages_requested": false, "seats": [seat("Notes", "app", Some("TextEdit"), None), seat("Share", "web", None, Some("https://example.org/share"))] });
        let mut w = Win::open("l2-preview");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(plan)]))));
        let goal = "A weekly meeting notes app: notes in TextEdit, share the summary on example.org";
        let q = "Should the summary be made for you, or do you write it?";
        let turn = |who, text: &str| Turn { who, text: text.into(), mark: Mark::None, at: "2026-10-31T00:00:00Z".into(), attachments: Vec::new(), choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None };
        let asked = Turn { choices: vec!["Make it for me".into(), "I'll write it".into()], proposal: Some(goal.into()), ..turn(Who::Shibubu, &format!("Nice. {q}")) };
        w.app.shell.talk = vec![turn(Who::You, "hello"), turn(Who::Shibubu, "Hi! What would you like to make?"), turn(Who::You, goal), asked];
        // 答えていない問いは案の card に残る
        let out = w.step(0.1, true, vec![]);
        assert!(ax_words(&out).iter().any(|l| *l == format!("{OPEN_QUESTION}: {q}")), "答えていない問いが card に無い: {:?}", ax_words(&out));
        // Preview plan = AI が作る時と同じ読みを 1 回（読む間は 1 行・何も作らない）→ 作る Page と入る app
        let preview = Win::button(&out, PREVIEW_PLAN).expect("Preview plan");
        ax_click(&mut w, preview);
        let t0 = Instant::now();
        while w.app.shell.talk.last().is_some_and(|t| t.preview == [PLAN_READING]) {
            assert!(t0.elapsed() < Duration::from_secs(60), "案の読みが終わらない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let lines = w.app.shell.talk.last().unwrap().preview.clone();
        assert_eq!(lines.first().map(String::as_str), Some("One page: Weekly meeting notes"), "作る Page が見えない: {lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("Apps: ") && l.contains("TextEdit") && l.contains("example.org")), "入る app が見えない: {lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some(PLAN_UNMADE));
        assert_eq!(w.app.generated.rows().len(), 0, "Preview で作った");
        // Build now = 見せた案をそのまま保存（AI を呼び直さない = Fake の答えは 1 つだけ）
        let accept = Win::button(&w.step(0.1, true, vec![]), BUILD_NOW).expect("Preview の後の Build now");
        ax_click(&mut w, accept);
        for _ in 0..3 {
            w.step(0.1, true, vec![]);
        }
        let View::Project { project, .. } = w.app.shell.view.clone() else { panic!("Build now で見せた案が出来ない: {:?} {:?}", w.app.shell.view, w.app.generated.note) };
        // dogfood r6: 会話の続き（I picked・Made）が畳んだ panel に隠れない = 作った Project の Chat が開いている
        let panel = w.app.generated.panel(&project).clone();
        assert!(panel.chat && panel.open, "Build now で作った Project の Chat が畳まれたまま（Build now の答えが見えない）: {panel:?}");
        let a = w.app.generated.app_of_project(&project).unwrap();
        let made = w.app.generated.project_of(&a).unwrap().unwrap().0;
        assert_eq!((made.name.as_str(), made.pages.iter().map(|g| g.title.as_str()).collect::<Vec<_>>()), ("Weekly meeting notes", vec!["Weekly meeting notes"]), "見せた案と違う物を作った");
        assert!(w.app.generated.model_busy_line().is_none(), "Build now が AI を呼び直した");
        assert!(!w.app.generated.llm_line().is_some_and(|l| l.contains("simple rules")), "Build now が読み直して名前の表で作った: {:?}", w.app.generated.llm_line());
        let chat = w.app.generated.chat(&project);
        let said: Vec<&str> = chat.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(&said[..3], ["hello", "Hi! What would you like to make?", goal], "Project の会話の頭が Home の会話でない: {said:?}");
        assert!(said[3].contains(q), "答えていない問いが Project の会話に残らない: {said:?}");
        assert_eq!(*said.last().unwrap(), "Made Weekly meeting notes. Next, say what to change, e.g. “add a Tests page”", "名前を 2 回言った・作った物が無い: {said:?}");
    }

    /// DOGFOOD-X 周 7 #1 #2 #5 #6: Settings › AI & Models › Execution limit の Calls を 0 にした AI で Home に一言。返事は止まった事と次の一手
    /// （関係の無い問いと選択肢を出さない・会話の thread を起こさない）・`Preview plan` は「AI が読む」と言わず 1 行目に止まった事と Build now が名前の表で作る事・
    /// Build now は AI を呼ばずに作り、作った Project の Chat は Home の会話で始まり「Built with simple rules」で終わる（止まった理由は写した返事の 1 回 = B13）。
    /// 負の対照（束）: `stopped_talk` の本体を `local_talk(text)` にする → 「止まった事と次の一手を言わない」で赤・plan_preview の `stopped` を None にする →
    /// 「AI が読むと言った」で赤・create_app の止まった腕を外す → 「Build now で作らない」で赤。
    #[test]
    fn r7_a_stopped_ai_says_so_and_accept_builds_with_simple_rules() {
        use crate::domain::chat::{Who, PLAN_UNMADE, SIMPLE_ACCEPT, TALK_WAIT};
        use crate::ui::shell::View;
        const GOAL: &str = "Track my inventory";
        let mut w = Win::open("r7-stopped");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(serde_json::json!({}))]))));
        w.app.generated.set_budget(Some(0), None);
        let before = w.app.generated.rows().len();
        w.submit("What to create", GOAL, false);
        let reply = w.app.shell.talk.last().cloned().expect("Shibubu の返事が無い");
        assert!(w.app.talk_rx.is_none() && reply.who == Who::Shibubu && reply.text != TALK_WAIT, "止まっている AI に会話を送った: {reply:?}");
        assert!(reply.text.contains("stopped at its limit (0 calls)") && reply.text.contains("Settings › AI & Models › Execution limit"), "止まった事と次の一手を言わない: {}", reply.text);
        assert!(reply.choices.is_empty(), "一言と関係の無い選択肢: {:?}", reply.choices);
        assert_eq!(reply.proposal.as_deref(), Some(GOAL), "作る物を言った文が案にならない");
        let ctx = w.ctx.clone();
        w.app.apply(&ctx, Intent::PreviewPlan);
        let preview = w.app.shell.talk.last().map(|t| t.preview.clone()).unwrap_or_default();
        assert!(!preview.iter().any(|l| l.contains("the AI reads this")), "止まっているのに AI が読むと言った: {preview:?}");
        assert!(preview.first().is_some_and(|l| l.contains("stopped at its limit") && l.ends_with(SIMPLE_ACCEPT)), "1 行目が止まった事と名前の表でない: {preview:?}");
        assert!(preview.len() >= 3 && preview.last().map(String::as_str) == Some(PLAN_UNMADE), "名前の表の案が無い: {preview:?}");
        w.app.apply(&ctx, Intent::AcceptPlan);
        w.step(0.1, true, vec![]);
        assert_eq!(w.app.generated.rows().len(), before + 1, "Build now で作らない（AI に送って断られる 1 手を挟んだ）: {:?}", w.app.generated.model_progress());
        let View::Project { project, .. } = w.app.shell.view.clone() else { panic!("作った Project へ移らない: {:?}", w.app.shell.view) };
        let chat = w.app.generated.chat(&project);
        assert!(chat.first().is_some_and(|t| t.who == Who::You && t.text == GOAL), "Home の会話が Project の Chat に無い: {chat:?}");
        // B13（REVIEW-R7 Y-b）: 止まった理由は写した Home の返事の 1 回だけ（最後の行で言い直さない）
        assert!(chat.last().is_some_and(|t| t.text == "Built with simple rules"), "AI を使わずに作った事が Chat に無いか、理由を 2 度言った: {chat:?}");
        assert_eq!(chat.iter().filter(|t| t.text.contains("stopped at its limit (0 calls)")).count(), 1, "止まった理由が Project の Chat に 2 回: {chat:?}");
        assert_eq!(w.app.generated.budget_used(), Some((0, Duration::ZERO)), "止まっている AI を呼んだ");
        assert!(w.app.shell.talk.is_empty(), "作った後も Home の会話が残る");
    }

    /// REVIEW-R7 R1（ISSUES B44）: Preview の読みで AI が上限に着いても、Build now は Preview で AI が読んだ案をそのまま保存する（AI を呼ばない道 =
    /// 止まっていても使える）。前は `create_app` の「止まっていれば名前の表」の腕が previewed の腕より先で、見せた Page と違う Project を作った。
    /// 場面: Calls 1（TALK-1101 追記 2: 一言・押すごとに数え直す）= Preview の読み 1 回で上限・Fake の答えは 2 つだけ（Build now が呼べば答えが尽きる）。
    /// Build now も 1 つの頼み = 数え直すが、読んだ案を保存するだけで AI を呼ばない（使った回数は 0 のまま）。
    /// 負の対照（束の arm）: 止まった腕の `&&self.generated.previewed(&text).is_none()` を外す → 「見せた案と違う物」で赤。
    #[test]
    fn r7_r1_accept_after_the_preview_used_the_last_call_saves_what_the_preview_showed() {
        use crate::domain::chat::{PLAN_READING, TALK_WAIT};
        use crate::ui::shell::View;
        const GOAL: &str = "Track my inventory";
        let seat = |title: &str, kind: &str, app: Option<&str>, url: Option<&str>| serde_json::json!({ "title": title, "kind": kind, "app_name": app, "bundle_id": app.map(|_| "com.apple.TextEdit"), "file": null, "url": url, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null });
        let plan = serde_json::json!({ "shape": "project", "name": "Stock ledger", "pages_requested": false, "seats": [seat("Counts", "app", Some("TextEdit"), None), seat("Supplier", "web", None, Some("https://example.org/supplier"))] });
        let talk = serde_json::json!({ "ask": "Shall I build this?", "choices": [], "proposal": GOAL, "needs": null });
        let mut w = Win::open("r7-r1");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(talk), Ok(plan)]))));
        w.app.generated.set_budget(Some(1), None);
        w.submit("What to create", GOAL, false);
        let t0 = Instant::now();
        while w.app.shell.talk.last().is_some_and(|t| t.text == TALK_WAIT) {
            assert!(t0.elapsed() < Duration::from_secs(60), "会話の答えが着かない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(w.app.shell.talk.last().and_then(|t| t.proposal.clone()).as_deref(), Some(GOAL), "会話の答えが案にならない: {:?}", w.app.shell.talk.last());
        let ctx = w.ctx.clone();
        w.app.apply(&ctx, Intent::PreviewPlan);
        let t0 = Instant::now();
        while w.app.shell.talk.last().is_some_and(|t| t.preview == [PLAN_READING]) {
            assert!(t0.elapsed() < Duration::from_secs(60), "案の読みが終わらない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let lines = w.app.shell.talk.last().map(|t| t.preview.clone()).unwrap_or_default();
        assert_eq!(lines.first().map(String::as_str), Some("One page: Stock ledger"), "Preview に AI の案が出ない: {lines:?}");
        assert!(w.app.generated.llm_stopped().is_some(), "場面が組めていない: Preview の読みで上限に着かない（{:?}）", w.app.generated.budget_used());
        w.app.apply(&ctx, Intent::AcceptPlan);
        for _ in 0..3 {
            w.step(0.1, true, vec![]);
        }
        let made = match w.app.shell.view.clone() {
            View::Project { project, .. } => w.app.generated.app_of_project(&project).and_then(|a| w.app.generated.project_of(&a).ok().flatten()).map(|p| (p.0.name.clone(), p.0.pages.iter().map(|g| g.title.clone()).collect::<Vec<_>>())),
            _ => None,
        };
        assert_eq!(made, Some(("Stock ledger".to_owned(), vec!["Stock ledger".to_owned()])), "見せた案と違う物を作った: {:?} {:?}", w.app.shell.view, w.app.generated.llm_line());
        assert_eq!(w.app.generated.budget_used().map(|u| u.0), Some(0), "Build now が AI を呼んだ");
    }

    /// REVIEW-R7 Y-a（Kangae ③ ⑤）: AI が上限で止まっている時、名前の表でも作れない文（変更の文）には案を出さない = 「Build now uses simple rules」と
    /// 約束して、押すと失敗する Build now の card を描かない・次の一手は Execution limit の 1 つ。作れる文（Track my inventory）は今までどおり案と約束（r7_a）。
    /// 負の対照（束の arm）: `no_ai_reply` の早い return を外す → 「作れない文に Build now の約束と案」で赤。
    #[test]
    fn r7_ya_a_stopped_ai_does_not_promise_accept_for_a_sentence_simple_rules_cannot_build() {
        use crate::domain::chat::{local_talk, PREVIEW_PLAN, SIMPLE_ACCEPT};
        const SAID: &str = "一覧に検索を追加して";
        // 場面の前提: 手元の会話はこの文を案にし（直す前は約束が出る）、名前の表では作れない
        assert!(local_talk(SAID).2.is_some(), "場面が組めていない: local_talk が案にしない");
        assert!(crate::composer::compose(SAID, &Default::default()).is_err() && !matches!(crate::composer::project::compose_project(SAID, &Default::default(), None), Ok(Some(_))), "場面が組めていない: 名前の表で作れる");
        let mut w = Win::open("r7-ya");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Ok(Box::new(crate::llm::fake::Fake::new(vec![Ok(serde_json::json!({}))]))));
        w.app.generated.set_budget(Some(0), None);
        w.submit("What to create", SAID, false);
        let reply = w.app.shell.talk.last().cloned().expect("Shibubu の返事が無い");
        assert!(reply.text.contains("Settings › AI & Models › Execution limit"), "次の一手を言わない: {}", reply.text);
        assert!(!reply.text.contains(SIMPLE_ACCEPT) && reply.proposal.is_none(), "作れない文に Build now の約束と案: {reply:?}");
        let out = w.step(0.1, true, vec![]);
        assert!(Win::button(&out, PREVIEW_PLAN).is_none(), "作れない文の案の card を描いた");
        assert_eq!(w.app.generated.budget_used(), Some((0, Duration::ZERO)), "止まっている AI を呼んだ");
    }

    /// B45（REVIEW-R7 Y-a の AI が無い側）: AI が無い Mac（CLI を確かめ終えて無い）で名前の表でも作れない文 → 案を出さない = 押すと
    /// 「作れません」で Home に戻る Build now の card を描かない・返事は作れない事と次の一手の 1 行（`NEEDS_AI`）。作れる文（Keep research notes）は
    /// 今までどおり案（`l2_hello_talks_and_only_accept_creates`）。
    /// 負の対照（束の arm）: `no_ai_reply` の門の `!ai&&` を `ai&&` に替える（AI が無い時に門が閉じる）→ 「作れない文に案」で赤。
    #[test]
    fn b45_no_ai_does_not_offer_build_now_for_a_sentence_simple_rules_cannot_build() {
        use crate::domain::chat::{local_talk, BUILD_NOW, PREVIEW_PLAN};
        const SAID: &str = "一覧に検索を追加して";
        // 場面の前提: 手元の会話はこの文を案にし（直す前は Build now の card が出る）、名前の表では作れない
        assert!(local_talk(SAID).2.is_some(), "場面が組めていない: local_talk が案にしない");
        assert!(crate::composer::compose(SAID, &Default::default()).is_err() && !matches!(crate::composer::project::compose_project(SAID, &Default::default(), None), Ok(Some(_))), "場面が組めていない: 名前の表で作れる");
        let mut w = Win::open("b45-no-ai");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli { detail: "test".into() }));
        let before = w.app.generated.rows().len();
        w.submit("What to create", SAID, false);
        let reply = w.app.shell.talk.last().cloned().expect("Shibubu の返事が無い");
        assert_eq!((reply.text.as_str(), reply.proposal.as_deref()), (super::NEEDS_AI, None), "作れない文に案・次の一手が無い: {reply:?}");
        let out = w.step(0.1, true, vec![]);
        for c in [BUILD_NOW, PREVIEW_PLAN] {
            assert!(Win::button(&out, c).is_none(), "作れない文の案の card に {c}");
        }
        assert_eq!(w.app.generated.rows().len(), before, "作れない文で Project が増えた");
    }

    /// B61-REPO（録画の手 1・Orca task_c7129addc656）の場面: Home で主デモの一言 → 偽の model（入力の 1 行目が `TALK_REPO` の時だけ needs を null・本物の model の代わり）
    /// の答え → (needs, Preview plan の灰, model が受けた入力)。`repo` = `ShibubuApp::workspace`（SHIBUBU_WORKSPACE_DIR）。
    fn b61_talk(repo: Option<std::path::PathBuf>) -> (Option<String>, Option<bool>, Vec<String>) {
        use crate::domain::chat::{PREVIEW_PLAN, TALK_REPO, TALK_WAIT};
        use std::sync::{Arc, Mutex};
        const SAID: &str = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. Put each on its own page.";
        struct RepoFake(Arc<Mutex<Vec<String>>>);
        impl crate::llm::Llm for RepoFake {
            fn label(&self) -> String {
                "repo-fake".into()
            }
            fn send(&self, r: &crate::llm::JsonRequest) -> Result<serde_json::Value, crate::llm::LlmError> {
                self.0.lock().unwrap().push(r.input.clone());
                let knows = r.input.lines().next().is_some_and(|l| l.starts_with(TALK_REPO));
                Ok(serde_json::json!({ "ask": "Got it. Anything else?", "choices": [], "proposal": SAID, "needs": if knows { serde_json::Value::Null } else { serde_json::json!("Which repo should I set up?") } }))
            }
        }
        let mut w = Win::open("b61");
        w.app.llm_rx = None;
        w.app.workspace = repo;
        let seen = Arc::new(Mutex::new(Vec::new()));
        w.app.generated.set_llm(Ok(Box::new(RepoFake(seen.clone()))));
        w.submit("What to create", SAID, false);
        let t0 = Instant::now();
        while w.app.shell.talk.last().is_some_and(|t| t.text == TALK_WAIT) {
            assert!(t0.elapsed() < Duration::from_secs(10), "model の答えが会話に来ない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = w.step(0.1, true, vec![]);
        let grey = out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(PREVIEW_PLAN)).map(|(_, n)| n.is_disabled());
        let needs = w.app.shell.talk.last().and_then(|t| t.needs.clone());
        let seen = seen.lock().unwrap().clone();
        (needs, grey, seen)
    }

    /// B61-REPO: repo が決まっている（SHIBUBU_WORKSPACE_DIR）時は、会話の model に folder の名前だけを 1 行目で渡す → repo を聞き返さず、案の card の
    /// Preview plan が押せる（録画の手 1 が「Which repo should I set up?」で止まった形を出さない）。path（利用者の名前を含む）は送らない。
    /// 負の対照（束の arm）: talk の `repo` を None にする → needs が立ち Preview plan が灰で赤。
    #[test]
    fn b61_a_known_repo_reaches_the_talk_and_the_plan_needs_no_repo_question() {
        use crate::domain::chat::TALK_REPO;
        let repo = std::env::temp_dir().join(format!("b61-{}", crate::storage::new_id())).join("shibubu-yc-demo");
        let (needs, grey, seen) = b61_talk(Some(repo.clone()));
        assert_eq!(seen.len(), 1, "会話の model を 1 回だけ呼ぶ: {seen:?}");
        assert!(seen[0].starts_with(&format!("{TALK_REPO}: shibubu-yc-demo\n")), "決まった repo が会話の model に渡らない: {seen:?}");
        assert!(!seen[0].contains(&repo.parent().unwrap().display().to_string()), "repo の path（利用者の名前を含む）を送った: {seen:?}");
        assert_eq!((needs, grey), (None, Some(false)), "決まった repo を聞き返して案が止まる");
    }

    /// B61-REPO: repo が決まっていない時は今のまま聞く（repo の行を渡さない → model の needs で Preview plan は灰）。
    /// 負の対照（束の arm）: talk の repo を data dir の名前にする（いつも何かを渡す）→ 聞かずに進んで赤。
    #[test]
    fn b61_without_a_repo_the_talk_still_asks_which_repo() {
        let (needs, grey, seen) = b61_talk(None);
        assert!(seen.len() == 1 && seen[0].starts_with("You: "), "repo が無いのに repo の行を渡した: {seen:?}");
        assert_eq!((needs.as_deref(), grey), (Some("Which repo should I set up?"), Some(true)), "repo が無いのに聞かずに進む");
    }

    /// L2: model が在る時、一言の直後は待つ 1 行だけ（案の card も出さない）・model には会話の行だけを渡す（`You: hello`）・着いた答えの問い・選択肢・案が会話に出る。
    /// 負の対照: `talk` の待つ 1 行をやめて `local_talk` を先に出す → 一言の直後に待つ 1 行が無く（model の答えも会話に入らず）赤。
    /// 偽の答えは `talk_schema` の全部の欄（`needs` は 1e4a5529 から required）を持つ。欠けると `validate` が Malformed にして手元の問い（TALK_ASK）に替わる
    /// = 夜の門 8060b357 の赤（実測の負の対照）。
    #[test]
    fn l2_the_model_answers_in_the_chat_and_nothing_is_made_while_it_reads() {
        use crate::domain::chat::{PREVIEW_PLAN, TALK_WAIT};
        use std::sync::{Arc, Mutex};
        // 会話は 300ms で答える・案の読み（`talk` の外）は 300ms 待って AI が無い = 読む間の 1 行と読めなかった 1 行が見える
        struct TalkFake(Arc<Mutex<Vec<String>>>);
        impl crate::llm::Llm for TalkFake {
            fn label(&self) -> String {
                "talk-fake".into()
            }
            fn send(&self, r: &crate::llm::JsonRequest) -> Result<serde_json::Value, crate::llm::LlmError> {
                std::thread::sleep(Duration::from_millis(300));
                if r.name != "talk" {
                    return Err(crate::llm::LlmError::NoCli { detail: "test".into() });
                }
                self.0.lock().unwrap().push(r.input.clone());
                Ok(if r.input.ends_with("You: hello") {
                    serde_json::json!({"ask": "Hi! What would you like to make?", "choices": ["Track my inventory", "", "Keep research notes", "a", "b"], "proposal": null, "needs": null})
                } else {
                    serde_json::json!({"ask": "Got it. Shall I build this?", "choices": [], "proposal": "Track my inventory", "needs": null})
                })
            }
        }
        let mut w = Win::open("l2-model");
        w.app.llm_rx = None;
        let seen = Arc::new(Mutex::new(Vec::new()));
        w.app.generated.set_llm(Ok(Box::new(TalkFake(seen.clone()))));
        let before = w.app.generated.rows().len();
        let answered = |w: &mut Win| {
            let t0 = Instant::now();
            while w.app.shell.talk.last().is_some_and(|t| t.text == TALK_WAIT) {
                assert!(t0.elapsed() < Duration::from_secs(10), "model の答えが会話に来ない");
                w.step(0.05, true, vec![]);
                std::thread::sleep(Duration::from_millis(20));
            }
            w.step(0.1, true, vec![])
        };
        w.submit("What to create", "hello", false);
        assert_eq!(w.app.shell.talk.last().map(|t| t.text.as_str()), Some(TALK_WAIT), "model を待たずに答えた");
        let out = answered(&mut w);
        assert_eq!(seen.lock().unwrap().as_slice(), ["You: hello"], "model に会話の行の外の物を渡した");
        let last = w.app.shell.talk.last().unwrap().clone();
        assert_eq!((last.text.as_str(), last.proposal.as_deref()), ("Hi! What would you like to make?", None));
        assert_eq!(last.choices, ["Track my inventory", "Keep research notes", "a", "b"], "空の選択肢を捨てて、数は AI のまま（CHAT-FREE-1101 で 3 つまでの切りをやめた）");
        assert!(Win::button(&out, PREVIEW_PLAN).is_none(), "hello に案の card が出た");
        w.submit("What to create", "Track my inventory", false);
        let out = w.step(0.1, true, vec![]);
        assert!(Win::button(&out, PREVIEW_PLAN).is_none(), "model が読む間に案の card が出た");
        let out = answered(&mut w);
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(seen.lock().unwrap()[1].starts_with("You: hello\nShibubu: Hi! What would you like to make?\nYou: Track my inventory"), "前の会話を渡していない: {:?}", seen.lock().unwrap());
        assert!(Win::button(&out, PREVIEW_PLAN).is_some(), "案に Preview plan が無い: {:?}", ax_words(&out));
        // AI が在る時の Preview は作る時と同じ読みを今 1 回（読む間は 1 行・Page の名前を作り話しない）・読めなければ理由の 1 行・まだ作らない
        ax_click(&mut w, Win::button(&out, PREVIEW_PLAN).unwrap());
        assert_eq!(w.app.shell.talk.last().map(|t| t.preview.clone()), Some(vec![crate::domain::chat::PLAN_READING.to_owned()]), "AI の道の Preview が読みを始めない");
        let t0 = Instant::now();
        while w.app.generated.model_busy_line().is_some() {
            assert!(t0.elapsed() < Duration::from_secs(60), "案の読みが終わらない");
            w.step(0.05, true, vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let out = w.step(0.1, true, vec![]);
        assert!(ax_words(&out).iter().any(|l| l.starts_with("AI wasn't available")), "読めなかった理由が card に無い: {:?}", ax_words(&out));
        assert_eq!(w.app.generated.rows().len(), before, "Build now を押す前に作った");
    }

    /// DOGFOOD-X 周 5 #5: model が「答えが無いと作れない」と言った問い（`needs`）が在る間、案の card の primary は灰で理由の 1 行が出て、
    /// `Preview plan`・`Build now` の intent も読まない・作らない。答えると次の答えで決め直し、needs が無ければ押せる・理由の 1 行も消える。
    /// 周 4 #3（一言目で案が出るか）は model の振る舞いなので本物の `llm_cli::l2_live_a_first_request_…` が持つ。
    /// 負の対照（commit message）: poll_talk の needs を読まない → 「答える前に Preview plan を押せる」で赤・plan_step の門を外す → 「答える前に案を読んだ」で赤。
    #[test]
    fn l2_a_needed_answer_holds_the_plan_until_it_is_answered() {
        use crate::domain::chat::{BUILD_NOW, NEEDS_ANSWER, TALK_WAIT};
        const Q: &str = "Which Numbers file and table should they go in?";
        struct NeedsFake;
        impl crate::llm::Llm for NeedsFake {
            fn label(&self) -> String {
                "needs-fake".into()
            }
            fn send(&self, r: &crate::llm::JsonRequest) -> Result<serde_json::Value, crate::llm::LlmError> {
                if r.name != "talk" {
                    return Err(crate::llm::LlmError::NoCli { detail: "test".into() });
                }
                Ok(if r.input.contains("Home.numbers") {
                    serde_json::json!({"ask": "Got it, the Budget table in Home.numbers. Anything else?", "choices": [], "proposal": "Copy the 2022 numbers into the Budget table in Home.numbers", "needs": null})
                } else {
                    serde_json::json!({"ask": format!("Sure. {Q}"), "choices": ["I'll tell you the file name"], "proposal": "Copy the 2022 numbers into my budget table in Numbers", "needs": Q})
                })
            }
        }
        let mut w = Win::open("l2-needs");
        w.app.llm_rx = None;
        w.app.generated.set_llm(Ok(Box::new(NeedsFake)));
        let before = w.app.generated.rows().len();
        let answered = |w: &mut Win| {
            let t0 = Instant::now();
            while w.app.shell.talk.last().is_some_and(|t| t.text == TALK_WAIT) {
                assert!(t0.elapsed() < Duration::from_secs(10), "model の答えが会話に来ない");
                w.step(0.05, true, vec![]);
                std::thread::sleep(Duration::from_millis(20));
            }
            w.step(0.1, true, vec![])
        };
        let primary = |out: &egui::FullOutput| out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(BUILD_NOW)).map(|(_, n)| n.is_disabled());
        let why = format!("{NEEDS_ANSWER}: {Q}");
        w.submit("What to create", "Copy the 2022 numbers into my budget table in Numbers", false);
        let out = answered(&mut w);
        assert_eq!(primary(&out), Some(true), "答える前に Build now を押せる（か案の card が無い）: {:?}", ax_words(&out));
        assert!(ax_words(&out).contains(&why), "理由の 1 行が無い: {:?}", ax_words(&out));
        let ctx = w.ctx.clone();
        w.app.apply(&ctx, Intent::PreviewPlan);
        w.app.apply(&ctx, Intent::AcceptPlan);
        assert!(w.app.shell.talk.last().is_some_and(|t| t.preview.is_empty()), "答える前に案を読んだ: {:?}", w.app.shell.talk.last());
        assert_eq!(w.app.generated.rows().len(), before, "答える前に作った");
        w.submit("What to create", "Home.numbers, the Budget table", false);
        let out = answered(&mut w);
        assert_eq!(primary(&out), Some(false), "答えた後も Build now が灰: {:?}", ax_words(&out));
        assert!(!ax_words(&out).iter().any(|s| s.starts_with(NEEDS_ANSWER)), "答えた後も理由の 1 行が残る: {:?}", ax_words(&out));
    }

    /// UI-1: Home の英語の例 3 つは model 無しで作れる（押すと Home に「作れない」が戻る例を出さない）。repo の例は Project（repo + TextEdit + Excalidraw）。
    /// `ui::home` は試験の crate（tests/renderer.rs 等）にも `#[path]` で入り composer を持たないので、試験はここ（bin）に置く。
    #[test] fn ui1_english_examples_compose_without_a_model(){
        use crate::composer::{self,project,ComposeContext};
        let ctx=ComposeContext::default();
        for (label,text) in crate::ui::home::EXAMPLES{
            let as_project=matches!(project::compose_project(text,&ctx,None),Ok(Some(_)));
            assert!(as_project||composer::compose(text,&ctx).is_ok(),"{label}: 「{text}」が model 無しで作れない");
        }
        assert!(matches!(project::compose_project(crate::ui::home::EXAMPLES[0].1,&ctx,None),Ok(Some(_))),"repo の例が Project にならない");
    }

    /// AD-SEC-R2-2: Connections の Web JSON の行に query の鍵・userinfo を出さない（窓は AccessKit で AX に出るので、desktop.window.state から LLM の文脈へ流れる）
    #[test] fn web_detail_shows_only_the_host(){
        for url in ["https://api.example.com?api_key=SECRET","https://api.example.com/v1/data?api_key=SECRET","https://user:SECRET@api.example.com/x","https://api.example.com#SECRET"]{
            assert_eq!(super::web_detail(Some(url)),"Address set · api.example.com · not checked","{url}");
        }
    }

    /// FX5（FX-023・FX-025）: GitHub の行は読み戻した account 名と確認の時刻を言い、`Check again` を持つ。env の token の有無の語（`Token set` / `No token`）は出さない。
    /// `Check again` は行の `ConnAct`（`recheck` が true だと押した手が画面の操作の口へ行く）。
    /// 確認から GITHUB_TTL たった Ready は Connected に下がり「古いかもしれない」と言う。負の対照（commit message）: `stale` の下げを外す → Connected の assert が赤。
    #[test] fn fx5_github_row_says_who_is_signed_in_and_goes_stale(){
        use super::{DisplayTier,GITHUB_TTL};
        let mut w=Win::open("fx5-github");
        let row=|w:&Win|w.app.connections_model().rows.into_iter().find(|r|r.name=="GitHub").expect("GitHub の行");
        let before=row(&w);
        assert!(!before.detail.contains("oken"),"env の token の語が出る: {}",before.detail);
        w.app.github_status=Some((DisplayTier::Ready,"Signed in as octo".into(),"14:02".into(),Instant::now()));
        let now=row(&w);
        assert_eq!((now.state,now.detail.as_str()),(DisplayTier::Ready,"Signed in as octo · checked 14:02"));
        assert!(now.acts.iter().any(|a| matches!(a, crate::ui::connections::ConnAct::CheckAgain))&&!now.recheck,"Check again が GitHub の手に無い（recheck が立つと画面の操作の口へ行く）");
        w.app.github_status=Some((DisplayTier::Ready,"Signed in as octo".into(),"14:02".into(),Instant::now().checked_sub(GITHUB_TTL+Duration::from_secs(1)).unwrap()));
        let stale=row(&w);
        assert_eq!(stale.state,DisplayTier::Connected,"期限が切れた Ready を下げない");
        assert!(stale.detail.ends_with("may be out of date"),"{}",stale.detail);
    }

    /// FX6（FX-027・UIUX H12）: 直近の message（状態の 1 行）は画面に 1 か所だけ。旧 canvas の toolbar にも同じ文を出していた（sidebar の最下行と 2 か所）。
    /// 負の対照（commit message）: toolbar の `theme::label(.. &self.message ..)` を戻す → 赤。
    #[test] fn fx6_the_status_message_is_shown_in_one_place(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx6-one-status");
        let session=w.app.store.selected;
        w.app.go(View::CompatCanvas(session));
        w.app.message="FX6 status sentinel".into();
        w.settle();
        let out=w.step(0.1,true,vec![]);
        let n=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木").nodes.iter().filter(|(_,n)|n.label()==Some("FX6 status sentinel")||n.value()==Some("FX6 status sentinel")).count();
        assert_eq!(n,1,"同じ状態の文が {n} か所に出ている");
    }

    /// FX6（FX-027）: 場所を移ったら前の場所の状態の 1 行（窓の下の帯の message）は残らない。移れなかった時の理由は残る。
    /// 負の対照（commit message）: `go` の `self.message.clear()` を外す → 赤。
    #[test] fn fx6_the_status_line_does_not_follow_you_to_another_place(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx6-status-clears");
        w.app.go(View::Connections);
        w.app.message="GitHub source: o/r".into();
        w.app.go(View::Home);
        assert_eq!(w.app.message,"","別の場所へ移っても前の場所の 1 行が残った");
        w.app.message="stays while on the same view".into();
        w.app.go(View::Home);
        assert_eq!(w.app.message,"stays while on the same view","同じ View のままなら消さない");
    }

    /// FX7（FX-042・UIUX T02）: 生成を頼んだ場所を離れている間に出来た Project では画面を替えず、窓の下の帯の状態の 1 行で知らせる。頼んだ場所に居れば今まで通り開く。
    /// 負の対照（commit message）: `on_created` の `left` を false に固定する → 1 つ目の assert が赤。
    #[test] fn fx7_a_project_made_while_you_are_elsewhere_does_not_take_the_screen(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx7-quiet-made");
        w.app.llm_rx=None;
        let made=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない").expect("主デモの一言は Project");
        w.app.go(View::Runs);
        w.app.create_from=Some(View::Home); // Home で頼んだ後に Runs へ移った
        w.app.on_created("goal".into(),Ok(Some(made.clone())));
        assert_eq!(w.app.shell.view,View::Runs,"離れている間に出来た Project が画面を替えた");
        assert!(w.app.message.contains("ready"),"知らせが無い: {}",w.app.message);
        w.app.create_from=Some(View::Runs); // 頼んだ場所に居る
        w.app.on_created("goal".into(),Ok(Some(made.clone())));
        assert_eq!(w.app.shell.view,made,"頼んだ場所に居るのに開かない");
    }

    /// 偽の bridge（tests/fake-surface-bridge.py）を env つきの使い捨て script で包む。受け取った要求は log に残る
    fn fake_bridge(name:&str,env:&[(&str,&str)])->(std::path::PathBuf,std::path::PathBuf){
        use std::os::unix::fs::PermissionsExt;
        let dir=std::env::temp_dir().join(format!("shibubu-app-test-{}-{name}",std::process::id()));
        let _=std::fs::remove_dir_all(&dir);std::fs::create_dir_all(&dir).unwrap();
        let log=dir.join("bridge.log");
        let py=concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fake-surface-bridge.py");
        let exports:String=env.iter().map(|(k,v)|format!("export {k}='{v}'\n")).collect();
        let sh=dir.join("bridge.sh");
        std::fs::write(&sh,format!("#!/bin/sh\nexport FAKE_LOG='{}'\n{exports}exec python3 '{py}'\n",log.display())).unwrap();
        std::fs::set_permissions(&sh,std::fs::Permissions::from_mode(0o755)).unwrap();
        (sh,log)
    }

    /// YC W17-1・YC_GATE #5 ①: 旧 Desktop パネルの「ウィンドウを検出」は bridge の apps + window_list で読む（cua-driver の list_windows ではない）
    #[test] fn old_desktop_panel_lists_windows_via_bridge(){
        let (sh,log)=fake_bridge("windows",&[("FAKE_WL",r#"[[{"title":"メモ.txt"},{"title":"別の窓","on_screen":false}]]"#)]);
        let ws=super::desktop_windows(&mut desktop_at(sh.clone())).unwrap();
        assert_eq!(ws.iter().map(|w|(w.pid,w.window_id,w.title.as_str(),w.app_name.as_str())).collect::<Vec<_>>(),
            vec![(222,300,"メモ.txt","REAPER"),(222,301,"別の窓","REAPER")]);
        let sent=std::fs::read_to_string(&log).unwrap();
        assert!(sent.contains(r#""cmd":"apps""#),"{sent}");
        assert!(sent.contains("window_list"),"{sent}");
        // CG の窓番号が決まらない窓は推測で出さない → 0 件は「無い」ではなく次の一手つきの Err（D-e）
        let (none,_)=fake_bridge("windows-none",&[("FAKE_WL","[[]]")]);
        let e=super::desktop_windows(&mut desktop_at(none)).unwrap_err();
        assert!(e.contains("press Find windows again"),"{e}");
        // AX の許可が無い → 空の一覧ではなく理由（window_list は許可が無くても窓を空で返す）
        let (untrusted,_)=fake_bridge("windows-untrusted",&[("FAKE_MODE","untrusted")]);
        let e=super::desktop_windows(&mut desktop_at(untrusted)).unwrap_err();
        assert!(e.contains("Accessibility permission is missing"),"{e}");
    }

    /// YC W17-1・YC_GATE #5 ②: 旧 Desktop パネルの「AX操作要素を読む」は window_list → handle_for → ax_tree（押せる / 値を置ける要素だけ）
    #[test] fn old_desktop_panel_lists_elements_via_ax_tree(){
        // label が空の要素は出さない: ③ の登録・実行（registry の desktop.ax.click = MacGui::run_desktop_matching）は label+role で探し直すので、空 label は押せない
        let (sh,log)=fake_bridge("elements",&[("FAKE_AX_VALUE","下書き"),("FAKE_AX_UNLABELED","1")]);
        let mut g=desktop_at(sh);
        let w=super::desktop_windows(&mut g).unwrap().remove(0);
        let c=super::desktop_controls(&mut g,&w).unwrap();
        assert_eq!(c.iter().map(|c|(c.label.as_str(),c.role.as_str(),c.value.as_deref().unwrap_or(""))).collect::<Vec<_>>(),
            vec![("保存","AXButton",""),("本文","AXTextArea","下書き")]);
        let r=&c[0].element.as_ref().expect("② の ref は payload の Control::element で返る（operator 2026-10-23）").0;
        assert!(r.starts_with('e')&&r.ends_with(".2"),"ref は bridge の e<snapshot>.<n>: {r}");
        let sent=std::fs::read_to_string(&log).unwrap();
        assert!(sent.contains("ax_tree")&&sent.contains(r#""handle":"w1""#),"{sent}");
        // 窓だけ読んだ所で app が答えなくなった（owner 256: 途中までの読みを読めた物として使わない）→ 答えない事の 1 行の Err
        let (partial,_)=fake_bridge("elements-partial",&[("FAKE_AX_PARTIAL","1")]);
        let mut g=desktop_at(partial);
        let w=super::desktop_windows(&mut g).unwrap().remove(0);
        let e=super::desktop_controls(&mut g,&w).unwrap_err();
        assert!(e.contains("stopped answering")&&e.contains("REAPER"),"{e}");
    }

    /// REVIEW-RQN（operator 5）: 自分の窓（偽の bridge の app の pid = この process）を読んだ答えに email が残らない（MacGui::read の 1 か所・RQ-025 と同じ）。
    /// 他の app の窓（pid 222）の email は残る。負の対照（commit message）: read の `w.pid == std::process::id()` を false に → 自分の窓の email が残って赤。
    #[test] fn rqn_reading_shibubus_own_window_hides_emails(){
        fn read(env:&[(&str,&str)],tag:&str)->String{
            let (sh,_)=fake_bridge(tag,env);
            let mut g=desktop_at(sh);
            let w=super::desktop_windows(&mut g).unwrap().remove(0);
            super::desktop_controls(&mut g,&w).unwrap().iter().map(|c|format!("{} {:?}\n",c.label,c.value)).collect()
        }
        let pid=std::process::id().to_string();
        let own=read(&[("FAKE_APP_PID",pid.as_str()),("FAKE_AX_VALUE","Claude plan · someone@example.com")],"own-email");
        assert!(!own.contains("someone@example.com")&&own.contains("(email hidden)"),"自分の窓の読みに email が残った: {own}");
        let other=read(&[("FAKE_AX_VALUE","Claude plan · someone@example.com")],"other-email");
        assert!(other.contains("someone@example.com"),"他の app の窓の email まで伏せた: {other}");
    }

    /// `AdapterRegistry::desktop()` と同じ形（MacGui だけ）で、bridge の場所だけ偽物に向ける（desktop() は MacGui::new() = env の bridge）。
    /// W19-1: 偽の bridge の app（REAPER）には AI に任せる同意がある状態（同意の無い時の試験は `act_consent_is_asked_in_the_desktop_panel`）
    fn desktop_at(sh:std::path::PathBuf)->super::AdapterRegistry{
        let mut r=super::AdapterRegistry::default();r.register(Box::new(gui_at(sh))).unwrap();r
    }
    fn gui_at(sh:std::path::PathBuf)->crate::adapters::mac_gui::MacGui{
        crate::adapters::mac_gui::MacGui::at(sh.clone()).with_act_consents(consents_beside(&sh))
    }
    /// 偽の bridge の隣に、REAPER を AI に任せた同意の file を書く（bridge の dir が無い試験〈/nonexistent〉では書かない＝同意なし）
    fn consents_beside(sh:&std::path::Path)->std::path::PathBuf{
        let f=sh.with_file_name("act-consents.jsonl");
        if sh.parent().is_some_and(|d|d.is_dir()){crate::surface::grant_act_consent(&f,"com.cockos.reaper",1.0).unwrap();}
        f
    }
    fn log_has(log:&std::path::Path,cmd:&str)->usize{
        std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l|l.contains(&format!(r#""cmd":"{cmd}""#))).count()
    }
    fn save(app:&str,window_id:i64)->crate::model::ActionKind{
        crate::model::ActionKind::Desktop{pid:222,window_id,label:"保存".into(),role:"AXButton".into(),tool:crate::model::DesktopTool::Click,text:String::new(),app_name:app.into()}
    }

    /// YC W17-2・YC_GATE #3・#6: 旧 lane の Desktop の実行（execute）は registry（W19-1）→ MacGui（ax_tree → ax_press）を通り、cua-driver を通らない
    #[test] fn desktop_binding_runs_through_mac_gui(){
        let (sh,log)=fake_bridge("run",&[]);
        // execute は AdapterRegistry::desktop()（MacGui::new → find_bridge → KANGAE_SURFACE_BRIDGE）。この bin の他の test はこの env を読まない（MacGui::at を使う）
        // AI に任せる同意（W19-1）も env の file から（同じく、この bin の他の test は `with_act_consents` で決めて env を読まない）
        std::env::set_var("KANGAE_SURFACE_BRIDGE",&sh);std::env::set_var(crate::surface::ACT_CONSENT_ENV,consents_beside(&sh));
        let approve=|k:&crate::model::ActionKind|super::desktop_consent("s1",k);
        let ok=super::execute(&save("REAPER",300),&[],approve(&save("REAPER",300)).as_ref());
        let other=super::execute(&save("Notes",300),&[],approve(&save("Notes",300)).as_ref());
        std::env::remove_var("KANGAE_SURFACE_BRIDGE");std::env::remove_var(crate::surface::ACT_CONSENT_ENV);
        let ok=ok.unwrap();
        assert!(ok.contains("AXPress")&&ok.contains("未検証"),"{ok}");
        assert!(other.unwrap_err().contains("Notes"),"別 app の窓");
        assert_eq!(log_has(&log,"ax_press"),1,"押したのは REAPER の 1 回だけ");
        assert_eq!(log_has(&log,"ax_tree"),2,"押す前に毎回読み直す");
    }

    /// 反証 W17-2: 旧 lane の Desktop の押す（Delivered）は履歴で「完了」にならず「受理（結果未検証）」になる（cua の頃と同じ）
    #[test] fn desktop_press_history_line_is_accepted_not_done(){
        let (sh,_)=fake_bridge("history",&[]);
        let ok=crate::adapters::sdk::legacy(gui_at(sh).run_desktop(&save("REAPER",300))).unwrap();
        let line=super::result_line("保存",&ok);
        assert!(line.starts_with("Accepted (result not verified)"),"{line}");
    }

    /// W19-1（operator 2026-10-23 (A)・(B)・旧 binding）: ③ は registry の gate を通る。承認が無い・見せた内容と違う・app 名の無い旧 binding・
    /// 許可が無い → どれも押さず 1 行。承認は「確認して実行」1 回 = 1 つ（同じ承認を同じ registry で 2 回使えない）
    #[test] fn desktop_binding_goes_through_the_registry_gate(){
        let (sh,log)=fake_bridge("gate",&[]);
        let kind=save("REAPER",300);
        let c=super::desktop_consent("s1",&kind).expect("画面の操作には承認を作る");
        assert!(matches!(&c.subject,crate::domain::permission::ConsentSubject::Action{action_ref,run_id:Some(_)} if action_ref=="desktop.ax.click"),"{c:?}");
        assert_eq!(c.scopes,["desktop.input"],"scope は registry の契約から");
        assert!(super::desktop_consent("s1",&crate::model::ActionKind::WebRead).is_none());
        let e=super::desktop_execute(&mut desktop_at(sh.clone()),&kind,None).unwrap_err();
        assert!(e.contains("承認"),"{e}");
        let typed=|text:&str|crate::model::ActionKind::Desktop{pid:222,window_id:300,label:"本文".into(),role:"AXTextArea".into(),tool:crate::model::DesktopTool::TypeText,text:text.into(),app_name:"REAPER".into()};
        let shown=super::desktop_consent("s1",&typed("見せた文")).unwrap();
        let e=super::desktop_execute(&mut desktop_at(sh.clone()),&typed("別の文"),Some(&shown)).unwrap_err();
        assert!(e.contains("digest"),"見せた内容と違う入力を承認で通した: {e}");
        let old=super::desktop_execute(&mut desktop_at(sh.clone()),&save("",300),super::desktop_consent("s1",&save("",300)).as_ref()).unwrap_err();
        assert!(old.contains("old format")&&old.contains("Find windows"),"{old}");
        assert_eq!(log_has(&log,"ax_press")+log_has(&log,"ax_set_value"),0,"gate の前に送った");
        let mut r=desktop_at(sh.clone());
        assert!(super::desktop_execute(&mut r,&kind,Some(&c)).is_ok());
        let again=super::desktop_execute(&mut r,&kind,Some(&c)).unwrap_err();
        assert!(again.contains("使用済み"),"{again}");
        assert_eq!(log_has(&log,"ax_press"),1,"1 回の承認で 1 回だけ押す");
        // 許可が無い: Adapter の 1 行（registry の定型文「RequiresConnection のため実行しない」に替えない）
        let (untrusted,ulog)=fake_bridge("gate-untrusted",&[("FAKE_MODE","untrusted")]);
        let e=super::desktop_execute(&mut desktop_at(untrusted),&kind,Some(&c)).unwrap_err();
        assert!(e.contains("Accessibility permission is missing")&&!e.contains("RequiresConnection"),"{e}");
        assert_eq!(log_has(&ulog,"ax_press"),0);
    }

    /// W19-1 app ごとの AI の同意（YC_DECISIONS 11 通目・既定は無し・operator 2026-10-23「modal にしない」）: 同意の無い app の窓を選ぶと
    /// inline の 1 行と「AI に任せる」が出て、AX の Click（試験の道具の AXPress と同じ口）で同意の file に 1 行。同意の後は出さない・bundle の読めない窓には出さない
    #[test] fn act_consent_is_asked_in_the_desktop_panel(){
        let dir=std::env::temp_dir().join(format!("shibubu-act-consent-{}",crate::storage::new_id()));
        let path=dir.join(crate::surface::ACT_CONSENT_FILE);
        let w=crate::gui::WindowRef{pid:222,window_id:300,app_name:"REAPER".into(),title:String::new(),bundle_id:"com.cockos.reaper".into(),document:String::new()};
        let ctx=egui::Context::default();ctx.enable_accesskit();
        let frame=|w:&crate::gui::WindowRef,events:Vec<Event>|{
            let mut got=None;
            let out=ctx.run(RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,Vec2::new(800.0,600.0))),events,..Default::default()},|ctx|{
                egui::CentralPanel::default().show(ctx,|ui|{got=super::act_consent_row(ui,w,&path);});
            });
            let t=out.platform_output.accesskit_update.expect("AccessKit の木が無い");
            let button=t.nodes.iter().find(|(_,n)|n.author_id()==Some("gui-act-consent:com.cockos.reaper")).map(|(id,_)|*id);
            let line=t.nodes.iter().any(|(_,n)|n.label().or(n.value()).is_some_and(|l|l.contains("Let the AI control REAPER?")));
            (button,line,got)
        };
        let (button,line,got)=frame(&w,vec![]);
        let button=button.expect("同意の無い app に「AI に任せる」が出ない");
        assert!(line&&got.is_none());
        assert!(crate::surface::act_consents(&path).is_empty(),"押す前に同意を書いた");
        let unknown=crate::gui::WindowRef{bundle_id:String::new(),..w.clone()};
        assert_eq!(frame(&unknown,vec![]).0,None,"bundle の読めない窓に同意の button を出した（何に同意したか分からない）");
        frame(&w,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:button,data:None})]);
        let (_,_,got)=frame(&w,vec![]);
        assert!(got.is_none()||matches!(got,Some(Ok(_))));
        assert!(crate::surface::act_consents(&path).contains("com.cockos.reaper"),"押しても同意が file に無い");
        assert_eq!(frame(&w,vec![]).0,None,"同意の後も 1 行を出した");
        let _=std::fs::remove_dir_all(&dir);
    }

    /// CG の窓番号と AX の窓が 1 対 1 に決まらない窓は、推測で選ばず押さない（読みもしない）
    #[test] fn desktop_binding_ambiguous_window_is_unsupported(){
        let (sh,log)=fake_bridge("ambiguous",&[("FAKE_WL",r#"[[{"title":"a","no_cg":true}]]"#)]);
        let r=crate::adapters::sdk::legacy(crate::adapters::mac_gui::MacGui::at(sh).run_desktop(&save("REAPER",300)));
        assert!(r.as_ref().unwrap_err().contains("Can't settle on one window"),"{r:?}");
        assert_eq!((log_has(&log,"ax_tree"),log_has(&log,"ax_press")),(0,0));
    }

    /// YC W17-2・YC_GATE #4: Connections の行は「画面の操作（Shibubu Bridge）」で、bridge の hello（許可）から check → readiness → tier_with。
    /// 許可が無い・bridge が無い時は「使える」止まりで次の一手を 1 行（D-e）。cua-driver の status は見ない
    #[test] fn connections_row_reports_bridge_trust_without_cua(){
        use crate::domain::capability::DisplayTier;
        assert!(super::GUI_ROW=="Screen control"&&!super::GUI_ROW.to_lowercase().contains("cua"));
        let (sh,log)=fake_bridge("conn",&[]);
        let (tier,detail)=super::gui_status(&mut desktop_at(sh));
        assert_eq!(tier,DisplayTier::Ready,"{detail}");
        assert!(detail.contains("with Accessibility")&&detail.contains("not verified"),"{detail}");
        assert!(log_has(&log,"hello")>=1);
        for (tag,env) in [("conn-untrusted",vec![("FAKE_MODE","untrusted")]),("conn-gone",vec![("FAKE_MODE","die")])]{
            let (sh,_)=fake_bridge(tag,&env);
            let (tier,detail)=super::gui_status(&mut desktop_at(sh));
            assert_eq!(tier,DisplayTier::Available,"{tag}");
            assert!(detail.contains("Accessibility")&&detail.contains("Open settings"),"{tag}: {detail}"); // 次の一手 = 表の EN（INTEG-UI-a 321abd0・copy-fix H5）
        }
        let (tier,_)=super::gui_status(&mut desktop_at("/nonexistent/kangae-surface-bridge".into()));
        assert_eq!(tier,DisplayTier::Available);
    }

    /// UI-4a（sendback 68）: 画面の操作の行の次の一手が `Open settings` の時、その語（detail の最後の ` · ` の後）が Connections の行の button として在る
    /// （literal の写しを期待に書かない・`connections_model` が行き先を渡している事も見る）。負の対照: gui の行の `settings` を None に戻す → button が無く赤。
    #[test] fn ui4a_gui_row_next_step_is_a_button(){
        use crate::ui::shell::View;
        let (sh,_)=fake_bridge("ui4a-conn",&[("FAKE_MODE","untrusted")]);
        let (tier,detail)=super::gui_status(&mut desktop_at(sh));
        let mut w=Win::open("ui4a-conn");
        w.app.gui_status=Some((tier,detail.clone()));
        let row=w.app.connections_model().rows.into_iter().find(|r|r.name==super::GUI_ROW).expect("画面の操作の行");
        assert!(row.settings.is_some(),"画面の操作の行に行き先が無い");
        // CONNECTIONS-1101: 次の一手 `Open settings` の button は `Allow`（文の ` · Open settings` は落とす）
        let next=if detail.ends_with("· Open settings"){"Allow"}else{detail.rsplit(" · ").next().unwrap()};
        w.app.go(View::Connections);
        for _ in 0..4{w.step(0.1,false,vec![]);}
        let out=w.step(0.1,false,vec![]);
        let t=out.platform_output.accesskit_update.expect("AccessKit の木が無い");
        assert!(t.nodes.iter().any(|(_,n)|n.role()==Role::Button&&n.label()==Some(next)),"次の一手「{next}」の button が無い");
    }

    /// 再確認 1 回 = bridge に hello を 1 回。押す・打つ・menu は 1 つも送らない（確かめるだけ）
    #[test] fn recheck_gui_probes_bridge_once(){
        let (sh,log)=fake_bridge("recheck",&[]);
        let _=super::gui_status(&mut desktop_at(sh));
        assert_eq!(log_has(&log,"hello"),1);
        for cmd in ["ax_press","ax_set_value","ax_menu","key_to_pid"]{assert_eq!(log_has(&log,cmd),0,"{cmd}");}
    }

    /// RQ-016a: MCP の `Check again` は裏の thread で起こす（押した手は server を待たずに戻る）。黙った server は MCP_CHECK（10 秒）で諦めて Available と理由の 1 行・
    /// 答える server は Ready。Connections に入るだけでは起こさない（enter_connections に起こす道が無い = 製品の code の文で見る・試験の module の文は数えない）。
    /// 起こすのは偽の server だけ（tests/fake-mcp-server.py）・利用者の ~/.shibubu/mcp.json は読まない（mcp_engine を直に入れる）。
    /// 負の対照（この commit）: check_mcp の thread を外す → 押した手が 10 秒待って赤・enter_connections に確かめを戻す → 文の検査で赤。
    /// 門の赤（e6e9e9f0）は待ちではなく文の切り: `#[cfg(test)]` の直後が `mod tests` でない（前の impl・`pub(crate) mod tests`）。古い針に戻すと下の形が「tests の module」で赤。
    #[test] fn rq016a_check_again_runs_in_the_background_and_gives_up_after_10_s(){
        use crate::domain::capability::DisplayTier as T;
        let mut w=Win::open("rq016a");
        let log=w.dir.join("calls.jsonl");
        for (name,mode) in [("rq016a-app-ok",""),("rq016a-app-quiet","silent")]{
            w.app.mcp_engine.mcp_servers.insert(name.into(),crate::mcp::w19_2_body::quill(mode,&log));
        }
        let row=|app:&super::ShibubuApp,key:&str|app.connections_model().rows.into_iter().find(|r|r.key==key).expect(key);
        assert_eq!(row(&w.app,"mcp:rq016a-app-ok").state,T::Available,"押す前に Ready と言った");
        let t=Instant::now();
        w.app.check_mcp("mcp:rq016a-app-quiet");w.app.check_mcp("mcp:rq016a-app-ok");
        assert!(t.elapsed()<Duration::from_secs(2),"押した手が server を待った（画面の thread で起こした）: {:?}",t.elapsed());
        assert!(row(&w.app,"mcp:rq016a-app-quiet").checking,"確かめている間は Checking");
        while !w.app.conn_checking.is_empty()&&t.elapsed()<Duration::from_secs(20){w.app.poll();std::thread::sleep(Duration::from_millis(50));}
        let waited=t.elapsed();
        assert!(waited>=Duration::from_secs(9)&&waited<Duration::from_secs(18),"MCP_CHECK の 10 秒で止めていない: {waited:?}");
        let quiet=row(&w.app,"mcp:rq016a-app-quiet");
        assert_eq!((quiet.state,quiet.detail.as_str()),(T::Available,"The MCP server didn't answer within 10 s · Check again"));
        let ok=row(&w.app,"mcp:rq016a-app-ok");
        assert_eq!((ok.state,ok.detail.as_str()),(T::Ready,"2 tools"));
        // Connections に入るだけでは起こさない・起こす道は check_mcp の裏の thread の 1 つだけ（製品の code の文だけを見る）
        let src=include_str!("app.rs");
        // 製品の文 = 最初の `#[cfg(test)]` より前。試験 module が `pub(crate)` でも、その前に `#[cfg(test)] impl` が在っても切る。
        fn cut<'a>(src:&'a str)->&'a str{let i=src.find("\n#[cfg(test)]\n").expect("tests の module");&src[..i]}
        let product=cut(src);
        let body=|f:&str|{let i=product.find(f).expect(f);&product[i..product[i+1..].find("\n    fn ").map_or(product.len(),|j|i+1+j)]};
        let enter=body("fn enter_connections(");
        assert!(!enter.contains("check_mcp(")&&!enter.contains("list_tools"),"Connections に入るだけで server を起こす: {enter}");
        let check=body("fn check_mcp(");
        assert!(check.find("thread::spawn").zip(check.find("list_tools_all(")).is_some_and(|(s,l)|s<l),"check_mcp が画面の thread で起こす: {check}");
        assert_eq!(product.matches("mcp::list_tools_all(").count()+product.matches("McpSession::open(").count(),1,"app.rs に server を起こす道が増えた");
        // 負: 門の形。古い針は当たらない。切りをその針に戻すと cut が「tests の module」で赤。試験 module を製品に数えると下の assert が赤。
        let trunk_shape="fn check_mcp(){thread::spawn(||{mcp::list_tools_all();});}\n#[cfg(test)]\nimpl ShibubuApp {}\n#[cfg(test)]\npub(crate) mod tests {\nfn check_mcp(){mcp::list_tools_all();McpSession::open();}\n}\n";
        assert!(trunk_shape.find("\n#[cfg(test)]\nmod tests").is_none(),"古い針が門の形に当たると、戻した時に赤にならない");
        let sliced=cut(trunk_shape);
        assert!(!sliced.contains("mod tests"),"試験 module を製品の文に数えた");
        assert_eq!(sliced.matches("mcp::list_tools_all(").count(),1,"門の形で起こす道を数えていない");
        assert!(!sliced.contains("McpSession::open("));
    }

    /// RQ-016a の本物の 1 回（operator 104 通目「AI の道を変えた commit は本物で 1 回」）: この Mac に既に入っている MCP の server 1 つを、試験の側の mcp.json
    /// （SHIBUBU_RQ016A_REAL_MCP = file の path・server は 1 つだけ・command は絶対 path）から入れ、行が Available → Check again → Ready（tools/list）になるのを見る。
    /// 利用者の ~/.shibubu/mcp.json は読まない・tool は呼ばない（tools/list だけ）・新しく install / download しない。
    /// 手で回す: `SHIBUBU_RQ016A_REAL_MCP=<mcp.json> cargo test --bin shibubu rq016a_real -- --ignored --nocapture`
    #[test] #[ignore = "本物の MCP の server を 1 つ起こす（SHIBUBU_RQ016A_REAL_MCP の mcp.json）"]
    fn rq016a_real_mcp_server_reaches_ready(){
        use crate::domain::capability::DisplayTier as T;
        let path=std::env::var("SHIBUBU_RQ016A_REAL_MCP").expect("SHIBUBU_RQ016A_REAL_MCP に mcp.json の path を渡す");
        let cfg:crate::mcp::McpConfig=serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.mcp_servers.len(),1,"server は 1 つだけ");
        let key=format!("mcp:{}",cfg.mcp_servers.keys().next().unwrap());
        let mut w=Win::open("rq016a-real");
        w.app.mcp_engine=cfg;
        let row=|app:&super::ShibubuApp|app.connections_model().rows.into_iter().find(|r|r.key==key).expect("MCP の行");
        let before=row(&w.app);
        println!("before: {:?} · {}",before.state,before.detail);
        assert_eq!(before.state,T::Available,"押す前に Ready と言った");
        let t=Instant::now();
        w.app.check_mcp(&key);
        while !w.app.conn_checking.is_empty()&&t.elapsed()<Duration::from_secs(20){w.app.poll();std::thread::sleep(Duration::from_millis(50));}
        let after=row(&w.app);
        let tools=after.acts.iter().filter(|a|matches!(a,crate::ui::connections::ConnAct::AllowTool{..})).count();
        println!("after {:.1} s: {:?} · {} · checkbox {tools}",t.elapsed().as_secs_f64(),after.state,after.detail);
        assert_eq!(after.state,T::Ready,"{after:?}");
    }

    /// PASSWORD-1101: その frame の AX の木の字（名前と値）= 画面の字
    fn ax_texts(out:&egui::FullOutput)->Vec<String>{
        out.platform_output.accesskit_update.as_ref().expect("AccessKit の木").nodes.iter().flat_map(|(_,n)|[n.label(),n.value()]).flatten().map(str::to_owned).collect()
    }
    /// PASSWORD-1101: dir の下の全 file（子の dir も）のうち `needle` の byte を含む物
    fn files_with(dir:&std::path::Path,needle:&str)->Vec<std::path::PathBuf>{
        let (mut found,mut stack)=(Vec::new(),vec![dir.to_path_buf()]);
        while let Some(d)=stack.pop(){
            for p in std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e|e.path()){
                if p.is_dir(){stack.push(p)}else if std::fs::read(&p).is_ok_and(|b|b.windows(needle.len()).any(|w|w==needle.as_bytes())){found.push(p)}
            }
        }
        found
    }
    /// PASSWORD-1101 の 3・5（owner 252「passwordmanagerをchatで入れられるようにしたいのとそれの安全性を担保すること」）: chat に打った password らしき物は
    /// AI に送らず・会話に足さず・保存せず、card（`Save to …`・`Delete`）だけを出す。偽の値は画面の字・会話・Shell の Debug（log に出る形）・data dir の全 file
    /// （DB = 会話と Page の data・設定）に 0 件。`Delete` で持っていた値を捨てる。
    /// 但し書き: この殻に AI は無い（`local_talk` の道）ので `llm-calls.jsonl` は門の有無に依らず書かれない = AI に送らない事は `talk_rx` と会話で見る。
    /// 負の対照: app.rs の `apply` の `Intent::Talk(t)|Intent::Generate(t,_) if looks_like_password(&t)` の腕を外す → 一言が会話と DB と画面に出て赤。
    #[test]
    fn password_1101_a_typed_password_is_held_not_sent(){
        const FAKE:&str="SHIBUBU-TEST-SECRET-7f3a9c2e";
        assert_eq!(crate::domain::vault::OWN_SERVICE,crate::storage::secret_store::KEYCHAIN_SERVICE,"Keychain の一覧から外す Shibubu の service がずれた");
        let mut w=Win::open("password-1101-held");
        w.submit("What to create",&format!("my github password is {FAKE}"),false);
        let out=w.step(0.1,true,vec![]);
        let texts=ax_texts(&out);
        assert!(texts.iter().all(|t|!t.contains(FAKE)),"画面の字に値");
        assert!(texts.iter().any(|t|t==crate::domain::vault::HELD),"止めた card が無い: {texts:?}");
        assert!(w.app.shell.talk.iter().all(|t|!t.text.contains(FAKE)),"会話に値");
        assert!(w.app.talk_rx.is_none(),"AI に送った");
        assert!(w.app.shell.held.is_some(),"値を持っていない（Save に渡せない）");
        assert!(!format!("{:?}",w.app.shell).contains(FAKE),"Shell の Debug（log に出る形）に値");
        let del=Win::button(&out,"Delete").expect("Delete の button");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:del,data:None})]);
        w.settle();
        assert!(w.app.shell.held.is_none(),"Delete の後も持っている");
        w.reopen();
        assert_eq!(files_with(&w.dir,FAKE),Vec::<std::path::PathBuf>::new(),"data dir の file に値");
    }
    /// PASSWORD-1101 の 3: 送らずに欄に残した password（打ちかけ）を、終わる時の保存（`snapshot_drafts` → sessions.json）に書かず、開き直した欄にも戻さない。
    /// 負の対照: `snapshot_drafts` の `looks_like_password` の retain を外す → sessions.json に値が入って赤。
    #[test]
    fn password_1101_an_unsent_password_is_not_kept_as_a_draft(){
        const FAKE:&str="SHIBUBU-TEST-SECRET-5d1e07b4";
        let mut w=Win::open("password-1101-draft");
        w.type_in("What to create",&format!("pw: {FAKE}"));
        w.reopen();
        assert!(!w.app.shell.command.contains(FAKE),"打ちかけが戻った");
        assert_eq!(files_with(&w.dir,FAKE),Vec::<std::path::PathBuf>::new(),"data dir の file に値");
    }

    /// 窓 1 枚の headless の殻: 本物の `ShibubuApp`（空の data dir）を egui の `Context::run` で描く（窓・GL は無い）。
    /// `delay` は直前の frame で app と egui が次の frame を頼んだ遅れ（eframe はこの後に描く・Duration::MAX = 次の入力まで描かない）。
    pub(crate) struct Win{ctx:egui::Context,pub(crate) app:super::ShibubuApp,frame:eframe::Frame,t:f64,delay:Duration,pub(crate) dir:std::path::PathBuf,size:Vec2}
    impl Win{
        pub(crate) fn open(tag:&str)->Self{Self::open_in(std::env::temp_dir().join(format!("shibubu-{tag}-{}",crate::storage::new_id())))}
        /// RQ3: 同じ data dir で開き直す（終わって開き直した利用者の 2 回目の起動）
        fn open_in(dir:std::path::PathBuf)->Self{
            let ctx=egui::Context::default();
            ctx.enable_accesskit();
            let app=super::ShibubuApp::open(&eframe::CreationContext::_new_kittest(ctx.clone()),dir.clone());
            Win{ctx,app,frame:eframe::Frame::_new_kittest(),t:0.0,delay:Duration::MAX,dir,size:Vec2::new(1440.0,900.0)}
        }
        /// `dt` 秒後に 1 frame（窓の focus と入力を渡す）。
        pub(crate) fn step(&mut self,dt:f64,focused:bool,events:Vec<Event>)->egui::FullOutput{
            self.t+=dt;
            let input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,self.size)),time:Some(self.t),focused,events,..Default::default()};
            let (app,frame)=(&mut self.app,&mut self.frame);
            let out=self.ctx.run(input,|ctx|eframe::App::update(app,ctx,frame));
            self.delay=out.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX,|v|v.repaint_delay);
            out
        }
        /// HOME-ATTACH-1101: 窓に file を落とした 1 frame（winit の DroppedFile が egui の `RawInput::dropped_files` に入る形・窓は前面）。
        fn drop_files(&mut self,paths:&[std::path::PathBuf])->egui::FullOutput{
            self.t+=0.1;
            let dropped_files=paths.iter().map(|p|egui::DroppedFile{path:Some(p.clone()),..Default::default()}).collect();
            let input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,self.size)),time:Some(self.t),focused:true,dropped_files,..Default::default()};
            let (app,frame)=(&mut self.app,&mut self.frame);
            self.ctx.run(input,|ctx|eframe::App::update(app,ctx,frame))
        }
        /// eframe の event loop の写し: 入力を渡さず、頼まれた遅れの後に次の frame を描く。`secs` 秒の間に頼まれて描いた frame の数。
        /// 最初の 1 frame（窓の focus が替わった・何かの入力の後の frame に当たる）は数えない。
        fn frames_while_idle(&mut self,secs:f64,focused:bool)->usize{
            self.step(0.0,focused,vec![]);
            let (mut n,mut left)=(0,secs);
            while self.delay<Duration::MAX && n<10_000{
                let dt=self.delay.as_secs_f64().max(1.0/60.0);
                if dt>left{break}
                left-=dt;
                self.step(dt,focused,vec![]);
                n+=1;
            }
            n
        }
        /// Home の欄（header の 1 つの欄・AX の名前「What to create」・cua-driver が探すのと同じ AX の木から）
        fn home_field(out:&egui::FullOutput)->NodeId{Self::field(out,"What to create")}
        /// AX の名前が `name` の 1 行の入力欄（UI-4a: 言葉の欄は header の 1 つで、名前が場所で替わる）
        fn field(out:&egui::FullOutput,name:&str)->NodeId{
            let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
            t.nodes.iter().find(|(_,n)|n.role()==Role::TextInput&&n.label()==Some(name)).map(|(id,_)|*id).unwrap_or_else(||panic!("「{name}」の欄が無い"))
        }
        /// 言葉の欄（入力欄・TERM-ONE-1101 から terminal ごとの欄は無い）の (名前, rect)。
        fn word_fields(out:&egui::FullOutput)->Vec<(String,[f64;4])>{
            let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
            // SESSIONS-LEFT-1101: sidebar の検索（一覧の filter・owner 198 の 5）は言葉の欄に数えない
            t.nodes.iter().filter(|(_,n)|matches!(n.role(),Role::TextInput|Role::MultilineTextInput|Role::SearchInput)&&n.label()!=Some(crate::ui::sidebar::SEARCH))
                .map(|(_,n)|(n.label().unwrap_or_default().to_owned(),n.bounds().map(|b|[b.x0,b.y0,b.x1,b.y1]).unwrap_or_default()))
                .collect()
        }
        /// 欄の文を AX の SetValue で置き、Enter（`cmd` なら ⌘↵）を 1 回押す。欄には先に focus を入れる（AX の Click）。
        fn submit(&mut self,name:&str,text:&str,cmd:bool){
            let field=Self::field(&self.step(0.1,true,vec![]),name);
            self.step(0.1,true,vec![
                Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:field,data:None}),
                Event::AccessKitActionRequest(ActionRequest{action:Action::SetValue,target:field,data:Some(egui::accesskit::ActionData::Value(text.into()))}),
            ]);
            let modifiers=if cmd{egui::Modifiers::COMMAND}else{egui::Modifiers::NONE};
            self.step(0.1,true,vec![Event::Key{key:egui::Key::Enter,physical_key:None,pressed:true,repeat:false,modifiers}]);
            for _ in 0..2{self.step(0.1,true,vec![]);}
        }
        /// Home の欄に AX の Click（cua-driver の AXPress と同じ口）で focus を入れ、入った事を AX の木で確かめる。窓は背景のまま
        fn focus_home_field(&mut self){
            let field=Self::home_field(&self.step(0.1,false,vec![]));
            self.step(0.1,false,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:field,data:None})]);
            let out=self.step(0.1,false,vec![]);
            assert_eq!(out.platform_output.accesskit_update.map(|t|t.focus),Some(field),"AX の Click で欄に focus が入っていない（試験が成り立たない）");
        }
        /// 1 frame 描き、その pass の作業の場所の左端（panel を置いた後の `available_rect` の左 = sidebar の右端か窓の左）と、
        /// 同じ pass の最後に起動の骨格（`skeleton::paint_shell`・install_startup の on_end_pass と同じ順）を描いた rect を返す。
        fn probe(&mut self)->(f32,crate::ui::skeleton::ShellRects,egui::FullOutput){
            self.t+=0.1;
            let input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,self.size)),time:Some(self.t),focused:true,..Default::default()};
            let (app,frame)=(&mut self.app,&mut self.frame);
            let (mut left,mut sk)=(f32::NAN,None);
            let out=self.ctx.run(input,|ctx|{
                eframe::App::update(app,ctx,frame);
                left=ctx.available_rect().left();
                let painter=ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground,egui::Id::new("probe-skeleton")));
                sk=Some(crate::ui::skeleton::paint_shell(&painter,ctx.screen_rect(),0.0));
            });
            self.delay=out.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX,|v|v.repaint_delay);
            (left,sk.unwrap(),out)
        }
        /// ⌘Q と同じ終わり方（`on_exit` の保存）の後、同じ data dir で app を開き直す（egui の記憶も新しい = 再起動）。
        fn reopen(&mut self){
            eframe::App::on_exit(&mut self.app);
            let ctx=egui::Context::default();
            ctx.enable_accesskit();
            self.app=super::ShibubuApp::open(&eframe::CreationContext::_new_kittest(ctx.clone()),self.dir.clone());
            self.ctx=ctx;self.t=0.0;self.delay=Duration::MAX;
        }
        /// AX の名前が `name` の欄に AX の Click と SetValue で文を置く（Enter は押さない = 打ちかけ）。
        fn type_in(&mut self,name:&str,text:&str){
            let field=Self::field(&self.step(0.1,true,vec![]),name);
            self.step(0.1,true,vec![
                Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:field,data:None}),
                Event::AccessKitActionRequest(ActionRequest{action:Action::SetValue,target:field,data:Some(egui::accesskit::ActionData::Value(text.into()))}),
            ]);
            self.step(0.1,true,vec![]);
        }
        /// 段 2 作り直し: Install CLI & skill の HOME を fixture に（試験は owner の ~/.claude・~/.local/bin に書かない）。link の先は HOME の外の偽の `shibubu`。
        /// 起動の確かめ（llm と found）の答えを先に読ませる（後から `refresh_agent_setup` が `found` を替えない）。返り = fixture の HOME（空）。
        fn agent_home(&mut self,found:&[&'static str])->std::path::PathBuf{
            let t0=Instant::now();
            while (self.app.llm_rx.is_some()||self.app.found_rx.is_some())&&t0.elapsed()<Duration::from_secs(30){self.step(0.05,false,vec![]);std::thread::sleep(Duration::from_millis(10));}
            let (home,bin)=(self.dir.join("agent-home"),self.dir.join("agent-bin"));
            std::fs::create_dir_all(&home).unwrap();std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("shibubu"),"#!/bin/sh\n").unwrap();
            let mut s=crate::terminal::AgentSetup::new(home.clone(),bin.join("shibubu"));
            s.set_found(found);
            self.app.agent_setup=Some(s);
            home
        }
        /// AX の木の名前が `name` の button。
        fn button(out:&egui::FullOutput,name:&str)->Option<NodeId>{
            let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
            t.nodes.iter().find(|(_,n)|n.role()==Role::Button&&n.label()==Some(name)).map(|(id,_)|*id)
        }
        /// 鍵を 1 回押す（窓は前面）。⌘ は `mac_cmd` と `command` の両方（macOS の winit が送る形・UI-4a の holdout h6 の誤りを繰り返さない）。
        fn key(&mut self,key:egui::Key,modifiers:egui::Modifiers){
            self.step(0.1,true,vec![Event::Key{key,physical_key:None,pressed:true,repeat:false,modifiers}]);
        }
        /// 幅の 180ms（motion::RAIL）が終わるまで回す。
        fn settle(&mut self){for _ in 0..4{self.step(0.1,true,vec![]);}}
        /// LOOK-1101: 今の画面を 1 frame 描き、offscreen の GPU（egui-wgpu・Metal）で `path`（.png）に撮る（dogfood の必須 ② の写真）。
        /// 依存を足さない: future は noop の waker で回し（native の wgpu は待たない）、BMP を書いて macOS の sips で PNG に。
        /// 画像の texture は font の atlas だけ（Mirror の写しの texture は描かない）。GPU を使うので `#[ignore]` の写真の試験だけが呼ぶ。
        fn shoot(&mut self,path:&std::path::Path){
            use eframe::{egui_wgpu::{Renderer,ScreenDescriptor},wgpu};
            fn block<F:std::future::Future>(f:F)->F::Output{
                let mut f=std::pin::pin!(f);
                let mut cx=std::task::Context::from_waker(std::task::Waker::noop());
                loop{if let std::task::Poll::Ready(v)=f.as_mut().poll(&mut cx){return v}std::thread::yield_now();}
            }
            let out=self.step(0.0,true,vec![]);
            let ppp=out.pixels_per_point;
            let jobs=self.ctx.tessellate(out.shapes,ppp);
            let (w,h)=((self.size.x*ppp) as u32,(self.size.y*ppp) as u32);
            let gpu=wgpu::Instance::new(&wgpu::InstanceDescriptor{backends:wgpu::Backends::METAL,..Default::default()});
            let adapter=block(gpu.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("GPU の adapter が無い");
            let (device,queue)=block(adapter.request_device(&wgpu::DeviceDescriptor::default(),None)).expect("GPU の device を作れない");
            let fmt=wgpu::TextureFormat::Rgba8UnormSrgb;
            let tex=device.create_texture(&wgpu::TextureDescriptor{label:None,size:wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1},mip_level_count:1,sample_count:1,
                dimension:wgpu::TextureDimension::D2,format:fmt,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});
            let view=tex.create_view(&Default::default());
            let mut r=Renderer::new(&device,fmt,None,1,false);
            let font=self.ctx.fonts(|f|f.image());
            r.update_texture(&device,&queue,egui::TextureId::default(),&egui::epaint::ImageDelta::full(font,egui::TextureOptions::LINEAR));
            let sd=ScreenDescriptor{size_in_pixels:[w,h],pixels_per_point:ppp};
            let mut enc=device.create_command_encoder(&Default::default());
            let cmds=r.update_buffers(&device,&queue,&mut enc,&jobs,&sd);
            {
                let mut pass=enc.begin_render_pass(&wgpu::RenderPassDescriptor{label:None,color_attachments:&[Some(wgpu::RenderPassColorAttachment{view:&view,resolve_target:None,
                    ops:wgpu::Operations{load:wgpu::LoadOp::Clear(wgpu::Color::BLACK),store:wgpu::StoreOp::Store}})],depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None}).forget_lifetime();
                r.render(&mut pass,&jobs,&sd);
            }
            let row=(w*4).div_ceil(256)*256; // copy の 1 行は 256 byte の倍数
            let buf=device.create_buffer(&wgpu::BufferDescriptor{label:None,size:(row*h) as u64,usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
            enc.copy_texture_to_buffer(tex.as_image_copy(),wgpu::TexelCopyBufferInfo{buffer:&buf,layout:wgpu::TexelCopyBufferLayout{offset:0,bytes_per_row:Some(row),rows_per_image:None}},
                wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1});
            queue.submit(cmds.into_iter().chain([enc.finish()]));
            buf.slice(..).map_async(wgpu::MapMode::Read,|r|r.expect("写真の buffer を読めない"));
            device.poll(wgpu::Maintain::Wait);
            let px=buf.slice(..).get_mapped_range();
            // 24 bit の BMP（下の行から・1 行は 4 byte の倍数）
            let pad=(4-(w*3)%4)%4;
            let size=54+(w*3+pad)*h;
            let mut bmp=Vec::with_capacity(size as usize);
            bmp.extend(b"BM");bmp.extend(size.to_le_bytes());bmp.extend([0u8;4]);bmp.extend(54u32.to_le_bytes());
            bmp.extend(40u32.to_le_bytes());bmp.extend((w as i32).to_le_bytes());bmp.extend((h as i32).to_le_bytes());bmp.extend(1u16.to_le_bytes());bmp.extend(24u16.to_le_bytes());bmp.extend([0u8;24]);
            for y in (0..h).rev(){
                let line=&px[(y*row) as usize..];
                for x in 0..w as usize{bmp.extend([line[x*4+2],line[x*4+1],line[x*4]]);}
                bmp.extend(std::iter::repeat_n(0u8,pad as usize));
            }
            let tmp=path.with_extension("bmp");
            std::fs::create_dir_all(path.parent().expect("写真の folder")).unwrap();
            std::fs::write(&tmp,&bmp).unwrap();
            let ok=std::process::Command::new("sips").args(["-s","format","png"]).arg(&tmp).arg("--out").arg(path).output().expect("sips が無い");
            std::fs::remove_file(&tmp).unwrap();
            assert!(ok.status.success(),"sips が PNG にできない: {}",String::from_utf8_lossy(&ok.stderr));
        }
        /// 1 frame 回し、主領域の下端（= 下の panel の上端・描いた物で測る）と出力を返す（G95・LAYOUT）。
        fn main_bottom(&mut self)->(f32,egui::FullOutput){
            self.t+=0.1;
            let input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,self.size)),time:Some(self.t),focused:true,..Default::default()};
            let (app,frame)=(&mut self.app,&mut self.frame);
            let mut b=f32::NAN;
            let out=self.ctx.run(input,|ctx|{eframe::App::update(app,ctx,frame);b=ctx.available_rect().bottom();});
            (b,out)
        }
        /// 起動の裏の確かめ（`open` の llm-cli-detect: 在りか → --version → auth status）の答えを `poll_llm` が読むまで、窓は背景のまま frame を回す。
        /// 答えは届いた時に egui を起こすので、idle を測る試験はこの後に測る（CI の runner は claude が無く答えが数百 ms で届き、idle の 10 秒に 1 frame として入った・ci/gate 0e83e60）。
        /// ponytail: 実時間で最大 30 秒（auth status は混雑で 4 秒・QUICK は 1 段 20 秒）。過ぎたら待たずに測る（答えが測りの数 ms に重なる時だけ赤）
        fn startup_answered(&mut self){
            let t0=Instant::now();
            while self.app.llm_rx.is_some()&&t0.elapsed()<Duration::from_secs(30){self.step(0.05,false,vec![]);std::thread::sleep(Duration::from_millis(10));}
        }
        /// G87: 本物の mouse の 1 click（HID と同じ順: 動かす → 押す → 離す・frame を分ける）。窓は前面。AX の Click（BG の driver の道）は通らない道。
        fn click_at(&mut self,at:Pos2){
            let button=|pressed|Event::PointerButton{pos:at,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE};
            self.step(0.1,true,vec![Event::PointerMoved(at)]);
            self.step(0.1,true,vec![button(true)]);
            self.step(0.1,true,vec![button(false)]);
            self.step(0.1,true,vec![]);
        }
        /// AX の木で名前が `name` の node の中心（主領域の中の物: x が `left` より右）。
        fn center_of(out:&egui::FullOutput,name:&str,left:f32)->Pos2{
            let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
            let b=t.nodes.iter().filter(|(_,n)|n.label()==Some(name)).filter_map(|(_,n)|n.bounds()).find(|b|b.x0>=f64::from(left)).unwrap_or_else(||panic!("「{name}」が AX の木に無い"));
            Pos2::new(((b.x0+b.x1)/2.0) as f32,((b.y0+b.y1)/2.0) as f32)
        }
    }
    impl Drop for Win{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
    const CTRL_CMD:egui::Modifiers=egui::Modifiers{alt:false,ctrl:true,shift:false,mac_cmd:true,command:true};
    const CMD:egui::Modifiers=egui::Modifiers{alt:false,ctrl:false,shift:false,mac_cmd:true,command:true};

    use crate::ui::shell::{Intent,SettingsSection};
    use crate::model::ThemeChoice;
    /// RQ1: AX の木の名前が `name` の選ぶ欄（Settings の form の RadioButton）。
    fn radio(out:&egui::FullOutput,name:&str)->NodeId{
        let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        t.nodes.iter().find(|(_,n)|n.role()==Role::RadioButton&&n.label()==Some(name)).map(|(id,_)|*id).unwrap_or_else(||panic!("「{name}」の選ぶ欄が無い"))
    }
    fn ax_click(w:&mut Win,id:NodeId){
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:id,data:None})]);
        w.step(0.1,true,vec![]);
    }
    fn copied(out:&egui::FullOutput)->Vec<String>{
        out.platform_output.commands.iter().filter_map(|c|match c{egui::OutputCommand::CopyText(t)=>Some(t.clone()),_=>None}).collect()
    }

    /// RQ1 の通し（headless・本物の ShibubuApp・空の data dir）: ⌘, で主領域が Settings › General（左の list の 4 行）→ AX の Click で Dark・Reduce motion On・
    /// Larger text → `store.ui` に入り、その場で効く（theme・motion の段・zoom）→ ⌘Q と同じ終わり方で開き直しても値と効き目が戻る。
    /// 負の対照（commit message）: `apply_settings` を起動で呼ばない → 開き直した後の motion と zoom で赤。
    #[test] fn rq1_settings_values_apply_now_and_after_reopen(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq1-settings");
        w.step(0.1,true,vec![]);
        w.key(egui::Key::Comma,CMD);
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::General),"⌘, で Settings へ移らない");
        let out=w.step(0.1,true,vec![]);
        for row in ["General","AI & Models","Connections","About"]{assert!(Win::button(&out,row).is_some(),"左の list に「{row}」が無い");}
        ax_click(&mut w,radio(&out,"Dark"));
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,radio(&out,"On"));
        let out=w.step(0.1,true,vec![]);
        let larger=Win::button(&out,"Larger text").expect("Larger text が無い");
        ax_click(&mut w,larger);
        assert_eq!((w.app.store.ui.theme,w.app.store.ui.reduce_motion,w.app.store.ui.text_zoom),(ThemeChoice::Dark,Some(true),Some(110)));
        w.step(0.1,true,vec![]);
        assert_eq!(w.ctx.theme(),egui::Theme::Dark);
        assert_eq!(crate::ui::motion::motion_mode(&w.ctx),crate::ui::motion::MotionMode::Fade,"Reduce motion On が効いていない");
        assert!((w.ctx.zoom_factor()-1.1).abs()<1e-4,"Larger text が効いていない: {}",w.ctx.zoom_factor());
        w.reopen();
        w.step(0.1,true,vec![]);w.step(0.1,true,vec![]);
        assert_eq!((w.app.store.ui.theme,w.app.store.ui.reduce_motion,w.app.store.ui.text_zoom),(ThemeChoice::Dark,Some(true),Some(110)),"開き直して値が戻らない");
        assert_eq!(w.ctx.theme(),egui::Theme::Dark);
        assert_eq!(crate::ui::motion::motion_mode(&w.ctx),crate::ui::motion::MotionMode::Fade,"開き直して Reduce motion が効いていない");
        assert!((w.ctx.zoom_factor()-1.1).abs()<1e-4,"開き直して文字の大きさが戻らない: {}",w.ctx.zoom_factor());
        // ⌘0（egui の既定の鍵）で戻した値も Settings に写る
        w.key(egui::Key::Num0,CMD);
        w.step(0.1,true,vec![]);w.step(0.1,true,vec![]);
        assert_eq!(w.app.store.ui.text_zoom,None,"⌘0 が Settings に写らない");
    }

    /// LOOK-1101（owner 11-01「defaultでdark modeにして、設定のところで変えられるように」）: ① 新しい data dir は Dark ② 前の既定のまま
    /// （`theme` = System・選んだ印 `theme_chosen` が無い = owner の Mac の保存）も Dark（既定だけ替えると、外観 Light の Mac では Light のまま）
    /// ③ Settings › General › Appearance で Light を選び、閉じて開き直すと Light のまま（選んだ値を既定で上書きしない）。
    /// 見るのは ctx の theme の選び（`theme_preference`）と保存の値。headless は OS の外観が無く System が egui の fallback（Dark）に落ちるので、`ctx.theme()` では ① ② の差が出ない。
    /// 負の対照: `UiSettings` の読みの「印が無ければ既定」の 3 行を外す → ② が赤・`Intent::SetTheme` の `theme_chosen = true` を外す → ③ が赤。
    #[test] fn look1101_dark_by_default_and_a_chosen_appearance_stays(){
        use egui::ThemePreference as P;
        let pref=|w:&Win|w.ctx.options(|o|o.theme_preference);
        let mut w=Win::open("look-fresh");
        w.step(0.1,true,vec![]);
        assert_eq!((pref(&w),w.app.store.ui.theme),(P::Dark,ThemeChoice::Dark),"① 新しい data dir が Dark で開かない");
        let dir=std::env::temp_dir().join(format!("shibubu-look-old-{}",crate::storage::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut old=serde_json::to_value(crate::model::Store::default()).unwrap();
        let ui=old["ui"].as_object_mut().expect("保存の ui");
        ui.insert("theme".into(),"System".into());
        ui.remove("theme_chosen");
        std::fs::write(dir.join("sessions.json"),serde_json::to_vec(&old).unwrap()).unwrap();
        let mut w=Win::open_in(dir);
        w.step(0.1,true,vec![]);
        assert_eq!((pref(&w),w.app.store.ui.theme),(P::Dark,ThemeChoice::Dark),"② 選んだ事の無い保存（System）が Dark で開かない");
        w.key(egui::Key::Comma,CMD);
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,radio(&out,"Light"));
        assert_eq!((pref(&w),w.app.store.ui.theme,w.app.store.ui.theme_chosen),(P::Light,ThemeChoice::Light,true),"③ の前提: Light を選べない");
        w.reopen();
        w.step(0.1,true,vec![]);
        assert_eq!((pref(&w),w.app.store.ui.theme),(P::Light,ThemeChoice::Light),"③ Light を選んで開き直すと既定の Dark に戻った");
    }

    /// LOOK-1101 の写真（dogfood の必須 ②）: 新しい data dir（= 既定の Dark）の Home・Settings › General・Project の下の Chat の panel・
    /// Terminal の panel を offscreen で撮り docs/evidence/LOOK-1101/ へ。GPU を使うので既定の suite では走らせない:
    /// `cargo test --bin shibubu look1101_dark_photos -- --ignored`。Terminal は利用者の shell の prompt（名前・path）を写さないよう
    /// 素の sh（prompt `$ `）に替えて画面を消し、画面の字に名前と home の path が無い事を見てから撮る。
    #[test] #[ignore] fn look1101_dark_photos(){
        use crate::ui::shell::View;
        let at=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/evidence/LOOK-1101");
        let mut w=Win::open("look-photos");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        w.settle();
        assert_eq!(w.ctx.theme(),egui::Theme::Dark,"前提: Dark で開いていない");
        w.shoot(&at.join("dark-home.png"));
        w.app.apply(&w.ctx.clone(),Intent::Go(View::Settings(SettingsSection::General)));
        w.settle();
        w.shoot(&at.join("dark-settings.png"));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        w.settle();
        w.key(egui::Key::Backtick,egui::Modifiers::CTRL);
        w.settle();
        assert!(w.app.generated.panel(&pid).chat,"前提: Chat の tab が開いていない");
        w.shoot(&at.join("dark-chat.png"));
        w.key(egui::Key::Backtick,egui::Modifiers::CTRL|egui::Modifiers::SHIFT);
        w.settle();
        let term=w.app.generated.terminals().list(Some(pid.as_str())).pop().expect("前提: Terminal の session が無い").key;
        // 利用者の rc（prompt の theme が毎回 prompt を作り直す物も）を通さない素の sh に替えてから打つ
        w.app.generated.terminals().write(&term,b"exec env -i PATH=/usr/bin:/bin PS1='$ ' TERM=xterm-256color LANG=en_US.UTF-8 BASH_SILENCE_DEPRECATION_WARNING=1 /bin/sh\r").unwrap();
        std::thread::sleep(Duration::from_secs(1));
        w.app.generated.terminals().write(&term,b"clear; echo 'Hello from Shibubu'\r").unwrap();
        let user=std::env::var("USER").unwrap_or_default();
        let home=std::env::var("HOME").unwrap_or_default();
        let shown=(0..300).find_map(|_|{
            std::thread::sleep(Duration::from_millis(100));
            let t=w.app.generated.terminals().screen(&term,0).ok()?.text();
            (t.contains("Hello from Shibubu")&&!t.contains("exec env")&&t.trim_end().ends_with('$')).then_some(t)
        }).expect("Terminal に prompt が戻らない（30 秒）");
        assert!((user.is_empty()||!shown.contains(&user))&&(home.is_empty()||!shown.contains(&home)),"Terminal の画面に利用者の名前か home の path が写る: {shown}");
        w.settle();
        w.shoot(&at.join("dark-terminal.png"));
    }

    /// RQ1（When Shibubu opens）: 既定（Home）は開き直しても Home・Last project を選ぶと最後に開いた Project で始まる・その Project が消えていれば Home と理由の 1 行。
    /// 負の対照（commit message）: `reopen_last_project` の呼び出しを起動から外す → 2 つ目で赤。
    #[test] fn rq1_last_project_reopens_only_when_chosen(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq1-reopen");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        w.settle();
        assert_eq!(w.app.store.ui.last_project.as_deref(),Some(pid.as_str()),"開いた Project を覚えていない");
        w.reopen();w.settle();
        assert_eq!(w.app.shell.view,View::Home,"既定（Home）なのに Project で始まった");
        w.app.apply(&w.ctx.clone(),Intent::Go(View::Settings(SettingsSection::General)));
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,radio(&out,"Last project"));
        assert!(w.app.store.ui.reopen_last_project);
        w.reopen();w.settle();
        assert!(matches!(&w.app.shell.view,View::Project{project,..} if *project==pid),"Last project で始まらない: {:?} {}",w.app.shell.view,w.app.message);
        // 消えた Project: id を読み捨て、Home と理由の 1 行
        w.app.store.ui.last_project=Some("project.gone".into());
        w.app.go(View::Home);
        w.app.store.ui.last_project=Some("project.gone".into());
        w.reopen();w.settle();
        assert_eq!(w.app.shell.view,View::Home);
        assert_eq!(w.app.store.ui.last_project,None,"消えた Project の id が残る");
        assert!(w.app.message.starts_with("Couldn't reopen your last project"),"理由の 1 行が無い: {}",w.app.message);
    }

    /// RQ1 反証の直し: DB を開けなかった起動（memory の store）は Project が消えたのではない → 覚えた Project を消さず「deleted」と言わない・
    /// その Project の打ちかけも捨てない。負の対照（commit message）: `project_saved` の `memory_fallback` を外す → last_project が消えて赤。
    #[test] fn rq1_db_open_failure_is_not_a_deleted_project(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq1-nodb");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);w.settle();
        w.app.store.ui.reopen_last_project=true;
        w.app.shell.command="half typed".into(); // この Project の欄の打ちかけ（on_exit が drafts[pid] に残す）
        eframe::App::on_exit(&mut w.app);
        // DB を開けなくする: 同じ名前の dir を置く（壊れた・他が握っている file の代わり）
        for f in ["kangae.sqlite3","kangae.sqlite3-wal","kangae.sqlite3-shm"]{let _=std::fs::remove_file(w.dir.join(f));}
        std::fs::create_dir(w.dir.join("kangae.sqlite3")).unwrap();
        w.reopen();w.settle();
        assert!(!w.app.generated.saves(),"memory の store で開いていない");
        assert_eq!(w.app.shell.view,View::Home);
        assert_eq!(w.app.store.ui.last_project.as_deref(),Some(pid.as_str()),"DB を開けなかっただけで最後の Project を忘れた");
        assert!(w.app.message.starts_with("Couldn't reopen your last project")&&!w.app.message.contains("deleted"),"理由の 1 行: {}",w.app.message);
        assert_eq!(w.app.shell.drafts.get(&pid).map(String::as_str),Some("half typed"),"Project の打ちかけを捨てた");
    }

    /// RQ1: app menu の `Settings…`（action の本体 `request_settings`）は次の frame で Settings › General へ 1 回だけ移す・
    /// ⌘K の `Settings` の行も同じ所へ・About の `Copy diagnostics` は path と email を伏せた行を clipboard へ（送らない）。
    /// 負の対照（commit message）: `keyboard` の `take_settings_request()` を外す → 1 つ目で赤。
    #[test] fn rq1_menu_palette_and_copy_diagnostics(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq1-menu");
        w.step(0.1,true,vec![]);
        crate::surface::request_settings();
        w.step(0.1,true,vec![]);
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::General),"menu の Settings… で移らない");
        w.app.go(View::Home);
        w.step(0.1,true,vec![]);
        assert_eq!(w.app.shell.view,View::Home,"同じ押しで 2 度移った");
        // ⌘K → Settings
        w.key(egui::Key::K,CMD);
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,Win::button(&out,"Settings").expect("⌘K に Settings の行が無い"));
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::General),"⌘K の Settings で移らない");
        // About → Copy diagnostics
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,Win::button(&out,"About").expect("About の行"));
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::About));
        let out=w.step(0.1,true,vec![]);
        let copy=Win::button(&out,"Copy diagnostics").expect("Copy diagnostics が無い");
        // copy は押した frame の出力に乗る
        let out=w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:copy,data:None})]);
        let text=copied(&out).pop().unwrap_or_default();
        assert!(text.starts_with("Shibubu: v")&&text.contains("\nmacOS: ")&&text.contains("\nProjects: "),"診断の行: {text}");
        let home=std::env::var("HOME").unwrap_or_default();
        // email の形（x@y.z）だけを見る: この Mac の MCP の設定（`@scope/pkg` の引数）は email ではないので残ってよい
        let email=text.split_whitespace().any(|w|w.split_once('@').is_some_and(|(u,h)|!u.is_empty()&&h.contains('.')));
        assert!(!text.contains("/Users")&&!text.contains("~/")&&!email&&(home.len()<2||!text.contains(home.as_str())),"path か email が出た: {text}");
        assert_eq!(w.app.message,"Diagnostics copied");
    }

    /// STATUSBAR-1101 追記 229（owner「settingがどこにあるかもわかりずらい。歯車マークを一番下のusageとかあるところの右下に置いて」）: 窓の一番下の帯の
    /// 右端に歯車が在り、押すと Settings › General（⌘, と同じ所）。左の sidebar に Settings の行は無い（`View::NAV` の試験 = shell.rs の nav の 3 行）。
    /// 負の対照（手で取る）: status_bar の歯車の `intent` を外す → 移らないで赤・歯車を `if !bar.status.is_empty()` の中に戻す → 状態の無い窓で見つからず赤。
    #[test] fn statusbar229_the_gear_at_the_bottom_right_opens_settings(){
        use crate::ui::shell::{View,SETTINGS_GEAR};
        let mut w=Win::open("statusbar229");
        w.step(0.1,true,vec![]);
        let out=w.step(0.1,true,vec![]);
        ax_click(&mut w,Win::button(&out,SETTINGS_GEAR).expect("下の帯に歯車が無い"));
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::General),"歯車で Settings へ移らない");
    }

    /// TITLEBAR-ICONS-1101 の 2'（案 A「⌘1〜9 で Page を移る」）: Project の中の ⌘2 は 2 枚目の Page・⌘1 は 1 枚目。Terminal に focus が在る時の ⌘1〜9
    /// （xterm の頁から来た = `term_keys`・egui の grid = `ax::terminal_focused`）は今まで通り Terminal の tab（Page は動かない）。
    /// 負の対照: keyboard の腕を前の `PanelAct::SelectNth` だけに戻す → 1 つ目が赤・`term_keys` を見ない → 3 つ目が赤・`terminal_focused` を見ない → 4 つ目が赤。
    #[test] fn titlebar1101_cmd_digit_goes_to_that_page_unless_typed_in_a_terminal(){
        use crate::ui::shell::View;
        let mut w=Win::open("titlebar1101-cmd-digit");
        let Some(View::Project{project,page:first})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("主デモの一言から Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page:first.clone()});
        let key=|k:egui::Key|Event::Key{key:k,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::COMMAND};
        let page=|w:&Win|match &w.app.shell.view{View::Project{page,..}=>page.clone(),v=>panic!("Project の View ではない: {v:?}")};
        w.step(0.1,true,vec![key(egui::Key::T)]);
        w.app.go(View::Project{project:project.clone(),page:first.clone()});
        w.step(0.1,true,vec![]);
        let pages=w.app.generated.page_list(&project);
        assert!(pages.len()>=2,"Page が 2 枚に足りない: {pages:?}");
        w.step(0.1,true,vec![key(egui::Key::Num2)]);
        assert_eq!(page(&w),pages[1].0,"⌘2 で 2 枚目の Page へ移らない");
        w.step(0.1,true,vec![key(egui::Key::Num1)]);
        assert_eq!(page(&w),pages[0].0,"⌘1 で 1 枚目の Page へ移らない");
        w.app.term_keys=true;
        w.step(0.1,true,vec![key(egui::Key::Num2)]);
        assert_eq!(page(&w),pages[0].0,"Terminal（xterm）で打った ⌘2 で Page が動いた");
        w.app.term_keys=false;
        let grid=egui::Id::new("titlebar1101-grid");
        w.ctx.memory_mut(|m|m.request_focus(grid));
        crate::ui::ax::note_terminal_focus(&w.ctx,grid);
        w.step(0.1,true,vec![key(egui::Key::Num2)]);
        assert_eq!(page(&w),pages[0].0,"Terminal（egui の grid）に focus が在るのに ⌘2 で Page が動いた");
    }

    /// W2b-AX 直し（原因と契約は src/main.rs）: AX で Home の欄に focus を入れた後、窓が背景（focused=false）で入力も変化も無い間、
    /// app は frame を 1 つも頼まない。直す前は 230ms ごと（10 秒で 43 frame）で、隠れた窓の GL の flush に main thread が居続け、AX の問いが後ろに並んだ。
    /// 正の対照: 同じ欄で窓が前面（focused=true）なら、cursor の点滅（egui の 0.5 秒）が frame を頼み続ける = 点滅は今まで通り。
    #[test] fn w2b_ax_idle_focused_field_requests_no_frames_in_background(){
        let mut w=Win::open("w2b-ax-idle");
        w.startup_answered();
        w.focus_home_field();
        // 入力の後の数 frame（egui の後始末）は数えない
        for _ in 0..5{w.step(0.5,false,vec![]);}
        let bg=w.frames_while_idle(10.0,false);
        assert_eq!(bg,0,"背景・入力なし・変化なしの 10 秒で {bg} frame を頼んだ");
        let fg=w.frames_while_idle(10.0,true);
        assert!(fg>=10,"前面で focus の在る欄の cursor が点滅していない（10 秒で {fg} frame）");
    }

    /// 背景の thread の結果（画面の操作の status・GitHub・取り込み）は、idle の窓でも次の入力を待たずに画面に届く:
    /// 送った時に egui を起こし、起きた frame の poll で読む（idle の間は frame を回さないので、届いた事を知らせる口はこれだけ）。
    #[test] fn w2b_ax_background_result_wakes_the_idle_window(){
        let mut w=Win::open("w2b-ax-wake");
        w.startup_answered();
        for _ in 0..5{w.step(0.5,false,vec![]);}
        assert!(!w.ctx.has_requested_repaint(),"送る前から frame を頼んでいる（idle になっていない・{:?}）",w.delay);
        let tx=w.app.tx.clone();
        std::thread::spawn(move||{let _=tx.send(super::Event::GuiStatus(super::DisplayTier::Ready,"ready".into()));}).join().unwrap();
        assert!(w.ctx.has_requested_repaint(),"背景の thread が送っても egui が起きない（idle の窓は次の入力まで結果を出さない）");
        w.step(0.0,false,vec![]);
        assert!(matches!(&w.app.gui_status,Some((_,s)) if s=="ready"),"起きた frame で結果を読んでいない: {:?}",w.app.gui_status);
    }

    /// 1 frame の shape のうち、塗り・線・文字の色が `blue` の物（文字は区間ごと）。ICONS-1101 追記 238 から Running / Loading も青でない（回る印）= 例外なし。
    fn blue_outside_working_state(out:&egui::FullOutput,blue:egui::Color32)->Vec<String>{
        use egui::epaint::{ColorMode,Shape};
        fn walk(s:&Shape,f:&mut dyn FnMut(&Shape)){if let Shape::Vec(v)=s{v.iter().for_each(|s|walk(s,f))}else{f(s)}}
        let mut bad=Vec::new();
        for c in &out.shapes{walk(&c.shape,&mut |s|match s{
            Shape::Rect(r) if r.fill==blue||r.stroke.color==blue=>bad.push(format!("rect {:?}",r.rect)),
            Shape::Circle(c) if c.fill==blue||c.stroke.color==blue=>bad.push(format!("circle {:?}",c.center)),
            Shape::Path(p) if p.fill==blue||matches!(p.stroke.color,ColorMode::Solid(c) if c==blue)=>bad.push(format!("path {:?}",p.points.first())),
            Shape::LineSegment{points,stroke} if stroke.color==blue=>bad.push(format!("line {points:?}")),
            Shape::Mesh(m) if m.vertices.iter().any(|v|v.color==blue)=>bad.push("mesh".into()),
            Shape::Text(t)=>{
                let job=&t.galley.job;
                if t.fallback_color==blue||t.override_text_color==Some(blue){bad.push(format!("text {:?}",job.text));}
                for sec in &job.sections{
                    let part=&job.text[sec.byte_range.clone()];
                    if sec.format.color==blue{bad.push(format!("text {part:?}"));}
                }
            }
            _=>{}
        })}
        bad
    }

    /// UI-3 Done 4（owner の決定 4「サブカラーに青やめよう」）: Home と主デモの一言の Dev の Page を Light と Dark で描き、塗り・線・文字の色に
    /// `working`（前の `interactive_primary` と同じ青）が出ない（ICONS-1101 追記 238 から「動いている」も青でなく回る印）。
    /// 負の対照（commit message）: `Variant::Selected`（選択中の Page の tab）の休みの塗りを `t.working` に替える → Dev の Page で赤。
    /// G95: Project の下の Terminal の panel。畳むと下には何も描かない（PANEL-SYNC-1101・tab は窓の一番下の帯の `Chat` と `+`）・⌃` で開くと主領域の 35% 前後で、Page の主領域は panel の上で終わる
    /// （panel の分だけ縮む・成果物の面が panel の下に入らない）。panel の grid（`terminal:<tab>`・TERM-ONE-1101 から terminal ごとの入力欄は無い）は panel の中・窓の中。もう 1 回の ⌃` で畳む（tab は残る）。
    /// L1-NORAIL: 既定の tab は Chat（CHAT-ONE-1101 から欄の行は無い = 欄は Chat の tab の中だけ）。Terminal は ⌃⇧` で足して開く（⌃` は今の tab の開閉・畳んだ間は下に開く = PANEL-SYNC-1101）。
    /// 変異: update の `draw_terminal_panel` を `CentralPanel` の後へ動かす → 主領域が縮まず赤・⌃` を `ShowTerminal` に戻す → 畳まず赤・
    /// draw_terminal_panel の `if !open` の腕を外す → 畳んだ panel が下に帯を取って赤（PANEL-SYNC-1101）。
    /// MIRROR-FIT-1101（owner 198 の 8）: 本物の app・Web の面の Page（`wide_page`）では縮めずに panel を上へ被せる = 主領域の下端は畳んだ時のまま・panel の高さは帯の行で測る。
    /// 変異（MIRROR-FIT）: draw_terminal_panel の `over` を常に false → 被せずに縮めて赤。
    #[test] fn g95_terminal_panel_shrinks_the_page_and_stays_in_the_window(){
        use crate::ui::shell::View;
        let mut w=Win::open("g95-panel");
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page:page.clone()});
        let ctrl_backtick=|shift:bool|Event::Key{key:egui::Key::Backtick,physical_key:None,pressed:true,repeat:false,modifiers:if shift{egui::Modifiers::CTRL|egui::Modifiers::SHIFT}else{egui::Modifiers::CTRL}};
        w.main_bottom();
        let (folded,out)=w.main_bottom();
        // PANEL-SYNC-1101: 畳んだ panel は下に帯を描かない（tab は窓の一番下の帯の Chat と ＋ = New tab）
        let chat_folded=0.0;
        assert!((w.size.y-folded-chat_folded).abs()<1.0,"畳んだ panel が下に帯を描いた: 主領域の下端 {folded}");
        assert!(Win::button(&out,"Chat").is_some()&&Win::button(&out,"New tab").is_some(),"窓の一番下の帯の Chat / +（New tab）が無い");
        w.step(0.1,true,vec![ctrl_backtick(true)]);
        for _ in 0..3{w.main_bottom();}
        let (open,out)=w.main_bottom();
        let tree=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        let hide=Win::button(&out,"Hide terminal").expect("開いた帯の Hide terminal が無い");
        let strip=tree.nodes.iter().find(|(id,_)|*id==hide).and_then(|(_,n)|n.bounds()).expect("Hide terminal の矩形").y0 as f32;
        let panel_h=w.size.y-strip;
        if w.app.generated.wide_page(&project,&page){
            assert!((w.size.y-open-chat_folded).abs()<1.0,"app / Web の面の Page を panel が縮めた（被せない）: 主領域の下端 {open}");
        }else{
            assert!((open-strip).abs()<12.0,"panel の分だけ主領域が縮まない: 主領域の下端 {open}・帯 {strip}");
        }
        assert!(panel_h>w.size.y*0.25&&panel_h<w.size.y*0.45,"開いた panel が主領域の 35% 前後でない: {panel_h}");
        // TERM-ONE-1101: terminal ごとの入力欄は無い（帯の ••• の Input field (for IME) も消した）= grid に直に打つ。grid は panel の中・窓の中
        assert!(Win::button(&out,"Terminal options").is_none(),"帯に ••• が残った");
        let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        assert!(!t.nodes.iter().any(|(_,n)|n.label()==Some("Type in the terminal")),"terminal ごとの入力欄が出た");
        let tab=w.app.generated.panel(&project).active.clone().expect("開いた tab");
        let id=format!("terminal:{tab}");
        let grid=t.nodes.iter().find(|(_,n)|n.author_id()==Some(id.as_str())).and_then(|(_,n)|n.bounds()).expect("panel の grid");
        assert!(grid.y0 as f32>=strip-0.5&&grid.y1 as f32<=w.size.y,"panel の grid が panel の外: {grid:?}・帯 {strip}");
        assert!(Win::button(&out,"Hide terminal").is_some(),"開いた帯の Hide terminal が無い");
        w.step(0.1,true,vec![ctrl_backtick(false)]);
        w.main_bottom();
        let (again,_)=w.main_bottom();
        assert!((w.size.y-again-chat_folded).abs()<1.0,"⌃` で畳まないか、畳んだ panel が下に帯を描いた: {again}");
        assert_eq!(w.app.generated.panel(&project).tabs.len(),1,"畳んでも tab は残る");
        w.app.generated.close_terminals();
    }
    /// L7-ORCA #9: 本物の殻と shell の上で、panel の grid を右 click → `Split Down` で同じ panel に新しい Terminal を下に割る（grid が 2 つ・上下に並ぶ）。
    /// 変異: draw_terminal_panel の `act.or(host.picked.take())` を外す → 割れず赤・TermHost の `terminal_panel_act` が picked に入れない → 赤。
    #[test] fn l7_grid_right_click_splits_the_panel_terminal_down(){
        use crate::ui::shell::View;
        let mut w=Win::open("l7-grid-menu");
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page});
        w.app.generated.panel_act(&project,crate::ui::terminal_panel::PanelAct::Add).unwrap();
        let first=w.app.generated.panel(&project).tabs[0].id.clone();
        let grid=|out:&egui::FullOutput,tab:&str|->Option<egui::accesskit::Rect>{
            let id=format!("terminal:{tab}");
            out.platform_output.accesskit_update.as_ref()?.nodes.iter().find(|(_,n)|n.author_id()==Some(id.as_str())).and_then(|(_,n)|n.bounds())
        };
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let a=grid(&w.step(0.1,true,vec![]),&first).expect("panel の grid が無い");
        let at=Pos2::new(((a.x0+a.x1)/2.0) as f32,((a.y0+a.y1)/2.0) as f32);
        let (button,modifiers)=(egui::PointerButton::Secondary,egui::Modifiers::NONE);
        w.step(0.1,true,vec![Event::PointerMoved(at),Event::PointerButton{pos:at,button,pressed:true,modifiers}]);
        w.step(0.1,true,vec![Event::PointerButton{pos:at,button,pressed:false,modifiers}]);
        let out=w.step(0.1,true,vec![]);
        let split=Win::button(&out,"Split Down").expect("grid の右 click の menu に Split Down が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:split,data:None})]);
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let out=w.step(0.1,true,vec![]);
        let p=w.app.generated.panel(&project).clone();
        assert_eq!((p.tabs.len(),p.active.as_deref(),p.down),(2,Some(first.as_str()),true),"Split Down で下に割らない");
        let second=p.split.clone().expect("割った tab が無い");
        let (a,b)=(grid(&out,&first).expect("上の grid"),grid(&out,&second).expect("下の grid"));
        assert!(b.y0>a.y0&&(a.x0-b.x0).abs()<1.0,"上下に並ばない: {a:?} {b:?}");
        w.app.generated.close_terminals();
    }
    /// L1-NORAIL（review B・operator 10-31）: 960 の窓（前は右の列を Page に重ねない狭い段 = Ask の card で開かなかった）でも、新しい Ask の card は
    /// 下の panel の Chat の tab を開いて出る。Project に入っただけでは開かない・CHAT-ONE-1101: 畳んだ panel に欄は無く、開いた Chat の tab に chat bar が 1 つ（Home と同じ `What to create`）。
    /// 負の対照: show_card に前の狭い段の枝（承認と同意だけ）を戻す → Ask で開かず赤・update の card の panel_act を外す → 畳んだまま赤・
    /// Page の帯を主領域に戻す（header の `band` を空に）→ ＋ ••• が header の下で赤。
    #[test] fn l1_norail_an_ask_card_opens_the_chat_tab_at_960(){
        use crate::ui::{engine_view::{EngineView,ASK_LABEL},shell::View};
        let mut w=Win::open("l1-norail-ask");
        w.size=Vec2::new(960.0,700.0);
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page});
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let p=w.app.generated.panel(&project).clone();
        assert!(p.chat&&!p.open,"Project に入っただけで panel を開いた: {p:?}");
        let names=|out:&egui::FullOutput|->Vec<String>{out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().filter_map(|(_,n)|n.label().map(str::to_owned)).collect()};
        let out=w.step(0.1,true,vec![]);
        let fields=Win::word_fields(&out);
        assert!(fields.is_empty(),"CHAT-ONE: 畳んだ panel に欄を出した: {fields:?}");
        assert!(Win::button(&out,"Chat").is_some()&&Win::button(&out,"New tab").is_some(),"窓の一番下の帯に Chat の tab と ＋ が無い（PANEL-SYNC-1101）: {:?}",names(&out));
        assert!(!names(&out).iter().any(|l|l.starts_with("Suggestions")||l=="Hide suggestions"),"右の rail の名残: {:?}",names(&out));
        // B: Page の帯（＋ ・ •••）は上の帯（header = 題の帯 `HEADER_H`）の中
        let bounds=|out:&egui::FullOutput,name:&str|out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().find(|(_,n)|n.role()==Role::Button&&n.label()==Some(name)).and_then(|(_,n)|n.bounds());
        for name in ["Add page · ⌘T","Page options"]{
            let b=bounds(&out,name).unwrap_or_else(||panic!("Page の帯の {name} が無い: {:?}",names(&out)));
            assert!(b.y1<=f64::from(crate::ui::theme::HEADER_H),"Page の帯の {name} が上の帯に無い: {b:?}");
        }
        w.app.generated.put_engine_view(&project,EngineView::Ask{who:"Terminal 1".into(),text:"Add a Notes page".into()});
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let out=w.step(0.1,true,vec![]);
        let p=w.app.generated.panel(&project).clone();
        assert!(p.chat&&p.open,"960 で Ask の card が Chat を開かない: {p:?}");
        assert!(Win::button(&out,ASK_LABEL).is_some(),"Chat の tab に Ask の card の {ASK_LABEL} が無い: {:?}",names(&out));
        assert_eq!(Win::word_fields(&out).iter().map(|(n,_)|n.as_str()).collect::<Vec<_>>(),vec!["What to create"],"開いた Chat の欄が Home と同じ 1 つでない");
    }
    /// L1-NORAIL R1 × CHAT-ONE-1101（owner 184「terminalとしたのchatの間の空間がいらない」）を本物の殻で通す: Project → New terminal（Add）→ Terminal の tab の間は
    /// 言葉の欄が無く grid は panel の下端まで（差 1 行未満）→ grid を click して focus → Ask の card が出ても tab は Terminal のまま・grid の focus は落ちない（R1）→
    /// Chat の tab で chat bar（`What to create`）に「add a Notes page」↵ → 会話の案 → Preview plan → Accept で Page が 1 枚増える（AI 無しの名前の表の道）。
    /// 負の対照: update の show_card の手を `PanelAct::Chat` に戻す（9e63f032）→ tab が Chat に替わり grid の focus が落ちて赤・terminal_panel::draw の Terminal の腕で
    /// 欄の行を引く → grid の下端が上がり赤。
    #[test] fn chat_one_the_terminal_tab_has_no_field_and_the_chat_changes_the_project(){
        use crate::ui::{engine_view::EngineView,shell::View,terminal_panel::PanelAct};
        let mut w=Win::open("chat-one-r1");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page});
        w.app.generated.panel_act(&project,PanelAct::Add).unwrap();
        let tab=w.app.generated.panel(&project).tabs[0].id.clone();
        let grid=|out:&egui::FullOutput|->Option<(NodeId,egui::accesskit::Rect)>{
            let id=format!("terminal:{tab}");
            out.platform_output.accesskit_update.as_ref()?.nodes.iter().find(|(_,n)|n.author_id()==Some(id.as_str())).and_then(|(nid,n)|n.bounds().map(|b|(*nid,b)))
        };
        let focus=|out:&egui::FullOutput|out.platform_output.accesskit_update.as_ref().map(|t|t.focus);
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let out=w.step(0.1,true,vec![]);
        let (_,g)=grid(&out).expect("Terminal の tab の grid が無い");
        assert!(Win::word_fields(&out).is_empty(),"Terminal の tab の間に言葉の欄を出した: {:?}",Win::word_fields(&out));
        assert!(f64::from(w.size.y)-g.y1<f64::from(crate::ui::theme::Type::Body.line_height()),"grid が panel の下端まで無い: {g:?}");
        // R1: grid を click して focus → Ask の card が出ても tab は Terminal のまま・focus は grid のまま
        let at=Pos2::new(((g.x0+g.x1)/2.0) as f32,((g.y0+g.y1)/2.0) as f32);
        let (button,modifiers)=(egui::PointerButton::Primary,egui::Modifiers::NONE);
        w.step(0.1,true,vec![Event::PointerMoved(at),Event::PointerButton{pos:at,button,pressed:true,modifiers}]);
        w.step(0.1,true,vec![Event::PointerButton{pos:at,button,pressed:false,modifiers}]);
        let out=w.step(0.1,true,vec![]);
        let (gid,_)=grid(&out).expect("click の後に grid が無い");
        assert_eq!(focus(&out),Some(gid),"grid を click しても focus が無い（試験が成り立たない）");
        w.app.generated.put_engine_view(&project,EngineView::Ask{who:"Terminal 1".into(),text:"Add a Tests page".into()});
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let out=w.step(0.1,true,vec![]);
        let p=w.app.generated.panel(&project).clone();
        assert!(!p.chat&&p.open,"card で Terminal の tab から Chat に替えた（R1）: {p:?}");
        assert_eq!(grid(&out).map(|g|g.0).zip(focus(&out)),Some((gid,gid)),"card が出て Terminal の focus が落ちた（R1）");
        // Chat の tab の chat bar で変更を頼む = 会話の案 → Accept
        w.app.generated.panel_act(&project,PanelAct::Chat).unwrap();
        let pages=|w:&Win|{let a=w.app.generated.app_of_project(&project).unwrap();w.app.generated.project_of(&a).unwrap().unwrap().0.pages.len()};
        let n=pages(&w);
        w.submit("What to create","add a Notes page",false);
        assert_eq!(pages(&w),n,"会話の一言だけで変えた（案の Accept の前）");
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::PreviewPlan);
        w.app.apply(&ctx,Intent::AcceptPlan);
        for _ in 0..2{w.step(0.1,true,vec![]);}
        assert_eq!(pages(&w),n+1,"案の Accept で変更が当たらない: {:?}",w.app.generated.note);
        w.app.generated.close_terminals();
    }
    /// L1-NORAIL × CHAT-ONE-1101 の本物の 1 回（104 通目・製品の claude の道）: 960 の窓で Project の言葉の欄は下の panel の Chat の chat bar（`What to create`）。
    /// そこに変更の文を置いて ↵ すると本物の model が会話で 1 回答えて案を出し、Preview で model がもう 1 回読み、Accept はその読みで Page を 1 枚増やす（OPEN-INPLACE-1101 の 5）。入力の token は混ざりの上限の下（RULE-045・3 欄の和）。
    /// 手で回す: `SHIBUBU_W13_LIVE_DIR=<空の dir> cargo test --bin shibubu l1_norail_live -- --ignored --nocapture`
    #[test] #[ignore = "本物の AI を 1 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l1_norail_live_the_bottom_field_changes_the_project_through_the_real_cli(){
        use crate::llm::Llm as _;
        use crate::llm_cli::{tests::{records,under_the_token_cap},CliConfig,CliLlm};
        use crate::ui::shell::View;
        let dir=std::path::PathBuf::from(std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w=Win::open_in(dir.join("app"));
        w.size=Vec2::new(960.0,700.0);
        w.startup_answered();
        // Project は AI 無しで作る（呼び出しは欄の 1 回だけ）
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        let cli=CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI");
        println!("cli: {} {}",cli.program().display(),cli.label());
        w.app.generated.set_llm(Ok(Box::new(cli)));
        w.app.go(View::Project{project:project.clone(),page});
        w.app.generated.panel_act(&project,crate::ui::terminal_panel::PanelAct::Chat).unwrap();
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let pages=|w:&Win|{let a=w.app.generated.app_of_project(&project).unwrap();w.app.generated.project_of(&a).unwrap().unwrap().0.pages.iter().map(|g|g.title.clone()).collect::<Vec<_>>()};
        let before=pages(&w);
        w.submit("What to create","Add a page called Notes",false);
        assert!(w.app.talk_rx.is_some(),"Chat の ↵ が会話の model に渡らない");
        let t0=Instant::now();
        while w.app.talk_rx.is_some()&&t0.elapsed()<Duration::from_secs(240){w.step(0.1,true,vec![]);std::thread::sleep(Duration::from_millis(100));}
        let ctx=w.ctx.clone();
        // OPEN-INPLACE-1101 の 5: Preview は Build now と同じ読みを 1 回（読み終わるまで Build now は待つ）・Build now はその読みを使う = 呼び出しは増えない
        w.app.apply(&ctx,Intent::PreviewPlan);
        while w.app.generated.model_busy_line().is_some()&&t0.elapsed()<Duration::from_secs(240){w.step(0.1,true,vec![]);std::thread::sleep(Duration::from_millis(100));}
        w.app.apply(&ctx,Intent::AcceptPlan);
        assert!(w.app.generated.model_busy_line().is_some(),"案の Accept が変更の道に渡らない: {:?} · {:?}",w.app.generated.chat(&project).last(),w.app.generated.note);
        while w.app.generated.model_busy_line().is_some()&&t0.elapsed()<Duration::from_secs(240){w.step(0.1,true,vec![]);std::thread::sleep(Duration::from_millis(100));}
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let after=pages(&w);
        println!("{:.1} s · pages {before:?} -> {after:?} · note {:?}",t0.elapsed().as_secs_f64(),w.app.generated.note);
        let recs=records(&dir);
        println!("calls: {}",serde_json::to_string(&recs).unwrap());
        assert_eq!(recs.len(),2,"呼び出しは会話と変更の 2 回だけ（Preview の読みを Build now が使う）: {recs:?}");
        under_the_token_cap(&recs);
        assert_eq!(after.len(),before.len()+1,"本物の model の変更で Page が 1 枚増えない: {after:?} · {:?}",w.app.generated.note);
    }
    /// L7-ORCA #12: 本物の殻の上で、帯の `+`（AX `New tab`）→ `New browser tab` で今の Page に Browser が 1 つ増える（panel の tab にはしない）。
    /// 変異: draw_terminal_panel の NewBrowserTab の腕を外す（panel_act に渡す）→ 面が増えず赤。
    #[test] fn l7_plus_new_puts_a_browser_on_this_page(){
        use crate::ui::shell::View;
        let mut w=Win::open("l7-plus-browser");
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page:page.clone()});
        let browsers=|w:&Win|{let g=&w.app.generated;g.app_of_project(&project).and_then(|a|g.project_of(&a).ok().flatten()).map_or(0,|(p,_)|p.surface_specs.iter().filter(|s|s.browser).count())};
        let before=browsers(&w);
        // agent の行は app.rs が AI の確かめの後に渡す物（`set_agents`）だけ
        w.app.generated.set_agents(vec!["codex"]);
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let new=Win::button(&w.step(0.1,true,vec![]),"New tab").expect("帯の +（New tab）が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:new,data:None})]);
        let out=w.step(0.1,true,vec![]);
        assert!(Win::button(&out,"New terminal").is_some(),"+ の menu に New terminal が無い");
        assert!(Win::button(&out,"Codex").is_some(),"set_agents で渡した agent が + の menu に無い（PROVIDER-ICONS-1101: 行の名前は AgentKind::label）");
        let item=Win::button(&out,"New browser tab").expect("+ の menu に New browser tab が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:item,data:None})]);
        for _ in 0..2{w.step(0.1,true,vec![]);}
        assert_eq!(browsers(&w),before+1,"New browser tab で Page に Browser が増えない");
        assert!(w.app.generated.panel(&project).tabs.is_empty(),"Browser を panel の tab にした");
    }
    /// 段 2 作り直し: panel の AX の (名前, 枠) の行（label か value を持つ node）。
    fn ax_rows(out:&egui::FullOutput)->Vec<(String,egui::Rect)>{
        let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        // 選べる Label は egui が行ごとに TextRun の子（同じ文・同じ矩形）を足す（egui 0.31 text_selection/accesskit_text.rs）→ 数えると 1 つの文が 2 回になる。部品の節だけ数える
        t.nodes.iter().filter(|(_,n)|n.role()!=egui::accesskit::Role::TextRun).filter_map(|(_,n)|{
            let b=n.bounds()?;
            Some((n.label().or(n.value())?.to_owned(),Rect::from_min_max(Pos2::new(b.x0 as f32,b.y0 as f32),Pos2::new(b.x1 as f32,b.y1 as f32))))
        }).collect()
    }
    /// 段 2 作り直し: 主デモの Project を開き、⌃⇧`（New terminal）で Terminal の tab を足して開いた frame。
    /// INTEG-SKILL: trunk の L1-NORAIL で panel の既定が Chat の tab になり ⌃` は Chat の開閉だけ → Terminal を起こすのは ⌃⇧`（g95 と同じ）
    fn open_panel(w:&mut Win)->egui::FullOutput{
        use crate::ui::shell::View;
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project,page});
        assert!(w.app.generated.terminals().config().project_cli.is_some(),"保存先を開けたのに Project の口が無い（Install の行を出す前提が崩れた）");
        w.step(0.1,true,vec![]);
        w.step(0.1,true,vec![Event::Key{key:egui::Key::Backtick,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::CTRL|egui::Modifiers::SHIFT}]);
        for _ in 0..3{w.step(0.1,true,vec![]);}
        w.step(0.1,true,vec![])
    }
    /// 段 2 作り直し (c): button を押さずに起動して Project の Terminal の panel を開いても、HOME（fixture）に 1 byte も増えない（黙って全 agent に撒かない・
    /// 黙って link も作らない）。出るのは右上の toast（文と `Install`）だけ（NOTIFY-1101）。
    /// 負の対照: `AgentSetup::new` の最後で `install()` を呼ぶ（起動時に入れる）→ link が出来て赤 / `setup_line` の閉じた枝で `install()` → 赤。
    #[test] fn skills3_nothing_is_installed_until_the_person_presses(){
        let mut w=Win::open("skills3-quiet");
        let home=w.agent_home(&["claude","codex","gemini","grok"]);
        let out=open_panel(&mut w);
        for _ in 0..5{w.step(0.1,true,vec![]);}
        assert!(ax_rows(&out).iter().any(|(l,_)|l==crate::generated::SETUP_INTRO),"右上の toast が無い: {:?}",ax_rows(&out).iter().map(|r|&r.0).collect::<Vec<_>>());
        let made:Vec<_>=std::fs::read_dir(&home).unwrap().map(|e|e.unwrap().path()).collect();
        assert!(made.is_empty(),"押していないのに HOME に書いた: {made:?}");
        w.app.generated.close_terminals();
    }
    /// 段 2 作り直し (e) × NOTIFY-1101: 押す前 = 右上の toast（`SETUP_INTRO` と button `Install`・1 枚）・押すと panel の中が一覧（agent ごとの checkbox・
    /// 置き場・状態・PATH の 1 行・Install / Cancel）・Install の後 = 右上の toast に状態の文（panel の常設の行は無い）。
    /// 見つかった claude だけが既定で on（codex は入れない）。
    /// 負の対照: toast を積まない → 文が無くて赤 / toast の `Install` で一覧を開かない → 一覧が無くて赤 / Install を押しても `install` を呼ばない → 状態の文が無くて赤 /
    /// 一覧で見つからない agent の既定を on → codex の stub が出来て赤 / Settings にも Install の button を出す → 赤 / Settings の Remove が remove を呼ばない → 赤。
    #[test] fn skills3_panel_first_line_is_the_install_button_then_the_installed_line(){
        let mut w=Win::open("skills3-panel");
        let home=w.agent_home(&["claude"]);
        let out=open_panel(&mut w);
        let rows=ax_rows(&out);
        let top_right=|r:&egui::Rect,w:&Win|r.top()<w.size.y*0.5&&r.left()>=w.size.x*0.5;
        let intro:Vec<&(String,egui::Rect)>=rows.iter().filter(|(l,_)|l==crate::generated::SETUP_INTRO).collect();
        assert_eq!(intro.len(),1,"toast の文が 1 枚でない: {intro:?}");
        assert!(top_right(&intro[0].1,&w),"toast が右上に無い: {:?}",intro[0].1);
        let at=rows.iter().find(|(l,r)|l=="Install"&&top_right(r,&w)).map(|(_,r)|r.center()).unwrap_or_else(||panic!("toast の Install が無い: {:?}",rows.iter().map(|r|&r.0).collect::<Vec<_>>()));
        w.click_at(at);
        let out=w.step(0.1,true,vec![]);
        let rows=ax_rows(&out);
        let names:Vec<&String>=rows.iter().map(|r|&r.0).collect();
        for must in ["claude","codex","gemini","grok","shibubu command","Install","Cancel",crate::generated::SETUP_PATH_NOTE]{
            assert!(rows.iter().any(|(l,_)|l==must),"一覧に「{must}」が無い: {names:?}");
        }
        assert!(rows.iter().any(|(l,_)|l.contains("~/.claude/skills/shibubu-cli/SKILL.md")),"置き場の path（~ で）が無い: {names:?}");
        assert!(!rows.iter().any(|(l,_)|l==crate::generated::SETUP_INTRO),"押した toast が残った");
        let setup=w.app.agent_setup.as_ref().unwrap();
        assert!(setup.open&&setup.rows.iter().filter(|r|r.on).map(|r|r.agent).eq([Some("claude"),None]),"既定の on が見つかった claude と link でない: {:?}",setup.rows);
        // panel（窓の下半分）の中の Install（Page に同じ名前の button が在っても押さない）
        let at=rows.iter().find(|(l,r)|l=="Install"&&r.top()>=w.size.y*0.5).map(|(_,r)|r.center()).unwrap_or_else(||panic!("panel の Install が無い: {names:?}"));
        w.click_at(at);
        let out=w.step(0.1,true,vec![]);
        let rows=ax_rows(&out);
        let line="shibubu CLI and skill installed for claude · Remove in Settings › Terminal";
        let done:Vec<&(String,egui::Rect)>=rows.iter().filter(|(l,_)|l==line).collect();
        assert_eq!(done.len(),1,"入った後の toast が無い: {:?}",rows.iter().map(|r|&r.0).collect::<Vec<_>>());
        assert!(top_right(&done[0].1,&w),"入った後の知らせが右上の toast に無い: {:?}",done[0].1);
        assert_eq!(std::fs::read_to_string(home.join(".claude/skills/shibubu-cli/SKILL.md")).unwrap(),crate::terminal::SKILL_STUB);
        assert!(!home.join(".agents").exists(),"選んでいない codex に入れた");
        assert!(std::fs::read_link(home.join(crate::terminal::CLI_LINK)).is_ok(),"link が無い");
        // Settings › Terminal = 状態の文と Remove だけ（button は toast の 1 つ・自分の場所を指す文を言わない）→ Remove で Shibubu の物だけ消える
        w.app.go(crate::ui::shell::View::Settings(crate::ui::shell::SettingsSection::Terminal));
        for _ in 0..3{w.step(0.1,true,vec![]);}
        let out=w.step(0.1,true,vec![]);
        let rows=ax_rows(&out);
        assert!(rows.iter().any(|(l,_)|l=="shibubu CLI and skill installed for claude"),"Settings の状態の文が無い: {:?}",rows.iter().map(|r|&r.0).collect::<Vec<_>>());
        assert!(Win::button(&out,"Install CLI & skill").is_none(),"Settings に 2 つ目の Install の button");
        assert!(Win::button(&out,"Remove").is_some(),"Settings に Remove が無い");
        let at=Win::center_of(&out,"Remove",0.0);
        w.click_at(at);
        let out=w.step(0.1,true,vec![]);
        assert!(!home.join(".claude/skills/shibubu-cli").exists()&&std::fs::symlink_metadata(home.join(crate::terminal::CLI_LINK)).is_err(),"Remove で stub か link が残った");
        assert!(ax_rows(&out).iter().any(|(l,_)|l==crate::generated::SETUP_NOT_YET),"Remove の後の文が無い");
        w.app.generated.close_terminals();
    }
    #[test] fn ui3_blue_is_only_the_working_state_on_home_and_dev(){
        use crate::ui::{shell::View,theme};
        let mut w=Win::open("ui3-blue");
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("主デモの一言から Project を作れない").expect("主デモの一言は Project");
        for (pref,t) in [(egui::ThemePreference::Light,theme::LIGHT),(egui::ThemePreference::Dark,theme::DARK)]{
            w.ctx.set_theme(pref);
            for (name,view) in [("Home",View::Home),("Dev",project.clone())]{
                w.app.go(view);
                for _ in 0..3{w.step(0.1,false,vec![]);}
                let out=w.step(0.1,false,vec![]);
                let texts:Vec<String>=out.shapes.iter().flat_map(|c|match &c.shape{egui::Shape::Vec(v)=>v.clone(),s=>vec![s.clone()]}).filter_map(|s|match s{egui::Shape::Text(t)=>Some(t.galley.text().to_owned()),_=>None}).collect();
                let here=if name=="Home"{"What do you want to create?"}else{"Dev"};
                assert!(texts.iter().any(|s|s==here),"{name} を描いていない（{pref:?}）: {texts:?}");
                let bad=blue_outside_working_state(&out,t.working);
                assert!(bad.is_empty(),"{name}（{pref:?}）: 動いている状態の外に青 {bad:?}");
            }
        }
    }

    /// RQ2（RQ-005）: 本物の殻の上で `{name} ⌄`（AX `Project options`）→ Rename → 1 つの欄（名前の欄・右端 `↵ Rename`）で名前を替え、Home の行の
    /// `•••`（AX `Options for {name}`）で Archive すると Continue から外れて `Archived · 1`。`⌄` の Delete… は Project の頭の確認を出すだけで、Esc で閉じて消えず、
    /// 確認の Delete で消えて Home へ・DB の行も無い。負の対照（commit message）: Esc の連鎖から `project_confirm.take()` を外す → Esc の後も Keep が残って赤。
    #[test]
    #[ignore = "SESSIONS-LEFT-1101: Home の行の `•••` は消した（Archive は sidebar の Project の行の右 click の menu・Archived · n も sidebar）· 右 click の menu の上で測り直すまで外す"]
    fn rq2_project_menu_renames_in_the_one_field_archives_and_deletes_after_an_ask(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq2-menu");
        let Some(View::Project{project,page})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page:page.clone()});
        fn names(out:&egui::FullOutput)->Vec<(NodeId,String)>{
            out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().map(|l|(*id,l.to_owned()))).collect()
        }
        let has=|out:&egui::FullOutput,label:&str|names(out).iter().any(|(_,l)|l==label);
        let press=|w:&mut Win,label:&str|{
            let out=w.step(0.1,false,vec![]);
            let id=names(&out).into_iter().find(|(_,l)|l==label).unwrap_or_else(||panic!("{label} が AX に無い: {:?}",names(&out))).0;
            w.step(0.1,false,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:id,data:None})]);
        };
        for _ in 0..3{w.step(0.1,false,vec![]);}
        press(&mut w,"Project options");
        press(&mut w,"Rename");
        assert_eq!((w.app.shell.renaming.as_deref(),w.app.shell.command.as_str()),(Some(project.as_str()),"Dev environment"),"名前の欄に今の名前が入っていない");
        w.app.shell.command="Shop repo".into();
        press(&mut w,"Rename"); // 欄の右端の ↵ Rename
        let app=w.app.generated.app_of_project(&project).expect("Project が開いていない");
        assert_eq!((w.app.shell.renaming.as_deref(),w.app.generated.title(&app).as_deref()),(None,Some("Shop repo")),"名前が替わっていない: {}",w.app.message);
        w.app.go(View::Home);
        press(&mut w,"Options for Shop repo");
        press(&mut w,"Archive");
        let out=w.step(0.1,false,vec![]);
        assert!(w.app.store.ui.archived_projects.contains(&project)&&!has(&out,"Shop repo")&&has(&out,"Archived · 1"),"Archive で Continue から外れていない: {:?}",names(&out));
        w.app.go(View::Project{project:project.clone(),page});
        press(&mut w,"Project options");
        press(&mut w,"Delete…");
        assert!(has(&w.step(0.1,false,vec![]),"Keep"),"Project の頭の確認が出ていない");
        w.step(0.1,false,vec![Event::Key{key:egui::Key::Escape,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::NONE}]);
        assert!(!has(&w.step(0.1,false,vec![]),"Keep")&&w.app.generated.app_of_project(&project).is_some(),"Esc で確認が閉じない・消えた");
        press(&mut w,"Project options");
        press(&mut w,"Delete…");
        press(&mut w,"Delete");
        assert_eq!(w.app.shell.view,View::Home,"消した後に Home へ移っていない: {}",w.app.message);
        assert!(w.app.generated.app_of_project(&project).is_none()&&!w.app.store.ui.archived_projects.contains(&project),"Project か印が残った");
        let _=std::fs::remove_dir_all(&w.dir);
    }

    /// RQ3（通し 1 行）: 本物の殻で Home の `•••` → `Export…` で Downloads に `.shibubu` → Settings の list の `Data & Recovery` に `Reveal in Finder`・
    /// `Import {file}`・`Back up now`（押すと世代の `Restore` の行）→ `Delete Projects and pages…` は行内の確認（Esc で閉じて消さない・Delete で消えて Home の行も無い）→
    /// `Import {file}` で戻って開く → 世代の `Restore` は予約だけ → 開き直すと世代の Project（名前も世代の時の物）に戻る。DB を壊して開き直すと Home に
    /// `Restore…` → Settings › Data & Recovery に `Try restoring` の行。負の対照（commit message）: Esc の連鎖から `data_confirm.take()` を外す → Esc の後も Keep が残って赤。
    #[test] fn rq3_data_and_recovery_exports_backs_up_deletes_imports_and_restores_on_the_real_shell(){
        use crate::ui::shell::{View,SettingsSection};
        // AX の名前（button）と文字の値（label の行）
        fn names(out:&egui::FullOutput)->Vec<(NodeId,String)>{
            out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().or(n.value()).map(|l|(*id,l.to_owned()))).collect()
        }
        let has=|out:&egui::FullOutput,label:&str|names(out).iter().any(|(_,l)|l==label);
        // 裏の thread の読み（Data & Recovery）が届くまで描き直す
        let wait=|w:&mut Win,what:&dyn Fn(&str)->bool|->String{
            for _ in 0..300{
                if let Some((_,l))=names(&w.step(0.05,false,vec![])).into_iter().find(|(_,l)|what(l)){return l}
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("出ない: {:?}",names(&w.step(0.1,false,vec![])))
        };
        let press=|w:&mut Win,label:&str|{
            let out=w.step(0.1,false,vec![]);
            let id=names(&out).into_iter().find(|(_,l)|l==label).unwrap_or_else(||panic!("{label} が AX に無い: {:?}",names(&out))).0;
            w.step(0.1,false,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:id,data:None})]);
        };
        let esc=Event::Key{key:egui::Key::Escape,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::NONE};
        let mut w=Win::open("rq3-data");
        let down=w.dir.join("Downloads");
        w.app.user_dirs=vec![down.clone(),w.dir.join("Desktop")];
        let Some(View::Project{project,..})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Home);
        for _ in 0..3{w.step(0.1,false,vec![]);}
        press(&mut w,"Options for Dev environment");
        press(&mut w,"Export…");
        let file=down.join("Dev environment.shibubu");
        assert!(file.is_file()&&w.app.message.starts_with("Exported Dev environment.shibubu to Downloads"),"書き出していない: {}",w.app.message);
        w.app.go(View::Settings(SettingsSection::General));
        press(&mut w,"Data & Recovery");
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::Data),"list の行で節へ移らない");
        wait(&mut w,&|l|l=="Import Dev environment.shibubu");
        let out=w.step(0.1,false,vec![]);
        assert!(has(&out,"Reveal in Finder")&&has(&out,"No backups yet · Back up now makes the first one"),"{:?}",names(&out));
        press(&mut w,"Back up now");
        assert!(w.app.message.starts_with("Backed up"),"{}",w.app.message);
        let restore=wait(&mut w,&|l|l.starts_with("Restore ")&&l.contains("Backed up by you"));
        // Delete my data: 行内の確認（Esc は何も変えずに閉じる）
        press(&mut w,"Delete Projects and pages…");
        assert!(has(&w.step(0.1,false,vec![]),"Keep"),"行内の確認が出ない");
        w.step(0.1,false,vec![esc]);
        assert!(!has(&w.step(0.1,false,vec![]),"Keep")&&w.app.generated.app_of_project(&project).is_some(),"Esc で確認が閉じない・消えた");
        w.app.store.ui.pinned_projects.push(project.clone());
        press(&mut w,"Delete Projects and pages…");
        press(&mut w,"Delete");
        assert!(w.app.generated.rows().is_empty()&&w.app.store.ui.pinned_projects.is_empty(),"Project か印が残った: {}",w.app.message);
        // 消した後の Data & Recovery の読み直し（裏の thread・`refresh_data(true)`）が届くまで描く。届いた frame で Projects の行が消え、下の
        // `Import …` の button の自動の id がずれる。press の 2 つの frame の間に届くと、押した id が次の frame に無く何も起きない（夜の門 8060b357 の赤）
        for _ in 0..300{
            if w.app.generated.data_summary().is_some_and(|d|d.projects.is_empty()){break}
            w.step(0.05,false,vec![]);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(w.app.generated.data_summary().is_some_and(|d|d.projects.is_empty()),"消した後の Data & Recovery の読み直しが届かない");
        // Import: 消した Project が同じ名前で戻って開く
        press(&mut w,"Import Dev environment.shibubu");
        let View::Project{project:got,..}=w.app.shell.view.clone() else{panic!("取り込んだ Project を開いていない: {}",w.app.message)};
        let app=w.app.generated.app_of_project(&got).expect("取り込んだ Project が無い");
        assert_eq!(w.app.generated.title(&app).as_deref(),Some("Dev environment"));
        w.app.generated.rename_project(&got,"Imported").unwrap();
        // Restore: 押しただけでは変えない（予約と Cancel の行）→ 開き直すと世代の時の名前
        w.app.go(View::Settings(SettingsSection::Data));
        wait(&mut w,&|l|l==restore);
        press(&mut w,&restore);
        assert_eq!(w.app.message,"Restores when Shibubu reopens · Quit and reopen Shibubu");
        wait(&mut w,&|l|l=="Cancel the restore");
        assert_eq!(w.app.generated.title(&app).as_deref(),Some("Imported"),"押しただけで戻した");
        // 開き直す（Win の Drop は data dir を消すので、dir を抜いてから閉じる）
        let dir=std::mem::take(&mut w.dir);
        drop(w);
        let mut w=Win::open_in(dir.clone());
        let app=w.app.generated.app_of_project(&project).unwrap_or_else(||panic!("開き直しても Project が戻らない: {:?}",w.app.generated.note));
        w.step(0.1,false,vec![]);
        assert_eq!(w.app.generated.title(&app).as_deref(),Some("Dev environment"),"世代の時の Project でない");
        assert!(w.app.generated.note.as_deref().is_some_and(|n|n.starts_with("Restored your backup")),"{:?}",w.app.generated.note);
        // 壊れた DB: 隔離して空から開き、Home の 1 行の Restore… から節へ
        w.dir=std::path::PathBuf::new();
        drop(w);
        for n in ["kangae.sqlite3","kangae.sqlite3-wal","kangae.sqlite3-shm"]{let _=std::fs::remove_file(dir.join(n));}
        std::fs::write(dir.join("kangae.sqlite3"),b"not a database, only words that fill the first page of the file. ".repeat(64)).unwrap();
        let mut w=Win::open_in(dir.clone());
        w.app.go(View::Home);
        assert!(has(&w.step(0.1,false,vec![]),crate::generated::DAMAGED_NOTE),"壊れた時の 1 行が Home に無い");
        press(&mut w,"Restore…");
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::Data));
        wait(&mut w,&|l|l.starts_with("Try restoring ")&&l.contains("Damaged"));
    }

    /// RQ3 反証の直し: 答えを返すだけの Engine（`engine_job` は次の frame の `poll_engine` まで残る = 走っている間）と、Import で足りない接続を返す Engine。
    struct Rq3Engine(Vec<crate::domain::project::ProjectResource>);
    impl crate::generated::PageEngine for Rq3Engine{
        fn run(&self,_:crate::generated::EngineRequest)->crate::generated::EngineAnswer{
            crate::generated::EngineAnswer{reply:crate::generated::EngineReply::Failed{line:"stopped".into()},log:("error",serde_json::json!({}))}
        }
        fn import_workflows(&self,_:&str,_:&[serde_json::Value])->Result<Vec<crate::domain::project::ProjectResource>,String>{Ok(self.0.clone())}
    }

    /// RQ3 反証の直し（holdout h3・反証 generated.rs:2411）: 本物の殻の Import → 開いた Page の上に足りない接続の 1 行
    /// （`github isn't connected on this Mac · Connect`）と、押すと Connections へ移る `Connect`。前は import_project が page_line に書いた直後に
    /// route → open_project が作り直して消えていた・文の `Connect it in Connections` は押せなかった。
    /// 負の対照（commit message）: open_project の `opened_line` を page_line に置く行を外す → 1 行が無くて赤。
    #[test] fn rq3_import_names_the_missing_connection_on_the_page_and_connect_opens_connections(){
        use crate::ui::shell::{View,SettingsSection};
        let mut w=Win::open("rq3-import-missing");
        let app=w.app.generated.create("読書記録を作って").expect("作れない");
        let Some(View::Project{project,..})=w.app.generated.project_view_of(&app) else{panic!("Project の View が無い")};
        let file=w.app.generated.export_project(&project,&w.dir.join("Downloads")).expect("書き出せない");
        let github=crate::domain::project::ProjectResource{id:"github".into(),kind:"api".into(),uri:None};
        w.app.generated.set_engine(crate::generated::EngineSetup{engine:std::sync::Arc::new(Rq3Engine(vec![github])),data_dir:w.dir.clone()});
        w.app.go(View::Settings(SettingsSection::Data));
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::ImportProject(file));
        assert!(matches!(&w.app.shell.view,View::Project{project:p,..} if *p!=project),"取り込んだ Project を開いていない: {}",w.app.message);
        let out=w.step(0.1,false,vec![]);
        let nodes=&out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes;
        let texts:Vec<String>=nodes.iter().filter_map(|(_,n)|n.label().or(n.value()).map(str::to_owned)).collect();
        assert!(texts.iter().any(|l|l.contains("github isn't connected on this Mac · Connect")),"足りない接続を Page に言わない: {texts:?}");
        let connect:Vec<NodeId>=nodes.iter().filter(|(_,n)|n.label()==Some("Connect")).map(|(id,_)|*id).collect();
        assert_eq!(connect.len(),1,"押せる Connect が 1 つでない: {texts:?}");
        ax_click(&mut w,connect[0]);
        assert_eq!(w.app.shell.view,View::Connections,"Connect が Connections へ移らない: {}",w.app.message);
    }

    /// RQ3 反証の直し（app.rs:751）: run の間の Delete my data › Connections は何も消さない（前は「Stop the run first」の門より先に、AI の login の dir と
    /// Keychain の鍵を消していた）。負の対照（commit message）: account を消す腕を delete_my_data の前に戻す → login の dir が消えて赤。
    #[test] fn rq3_deleting_sign_ins_during_a_run_removes_nothing(){
        use crate::ui::shell::{View,DataKind};
        let mut w=Win::open("rq3-del-conn-run");
        let app=w.app.generated.create("読書記録を作って").expect("作れない");
        let Some(View::Project{project,..})=w.app.generated.project_view_of(&app) else{panic!("Project の View が無い")};
        let login=w.dir.join("ai-accounts").join("plan-1");
        std::fs::create_dir_all(&login).unwrap();
        w.app.store.ui.ai_accounts=vec![crate::model::AiAccount::Plan{id:"plan-1".into()}];
        w.app.generated.set_engine(crate::generated::EngineSetup{engine:std::sync::Arc::new(Rq3Engine(vec![])),data_dir:w.dir.clone()});
        assert!(w.app.generated.start_engine(&project,"tell me the weather",vec![],None),"Engine が始まらない");
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::DeleteMyData(DataKind::Connections));
        assert_eq!(w.app.message,"Stop the run first");
        assert!(login.is_dir()&&w.app.store.ui.ai_accounts.len()==1,"run の間に AI の login を消した");
    }

    /// CHAT-011 反証の赤 1（operator 10-31）: 提案の Apply は `proposal()` に届き、Project に 1 revision を書く。前は intent の match で、本物の腕より前に
    /// `Intent::Proposal{..}` の腕が「Applying suggestions isn't available yet」を返し、提案の手（Apply / Preview / Later / Hide / Undo）が全部死んでいた。
    /// 負の対照: その腕を `Intent::SettingsChanged` の前に戻す → chat_note がその文で revision が増えず赤。
    #[test] fn rq5_a_suggestion_apply_from_the_app_reaches_proposal_and_writes_one_revision(){
        let mut w=Win::open("rq5-apply-intent");
        let app=w.app.generated.create("家計を見える化したい").expect("作れない");
        let view=w.app.generated.project_view_of(&app).expect("Project の View が無い");
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::Go(view));
        w.step(0.1,false,vec![]);
        // 件数の多い source（表に探す欄が無い）= 「探す欄を足す」提案が立つ
        let rows=w.app.agent.app().expect("今の Page の app を agent が読んでいない").definition().sources.iter().map(|s|(s.id.clone(),25)).collect();
        w.app.agent.observe(crate::agent::Observed{rows,..Default::default()});
        w.app.agent.refresh(&crate::agent::now_rfc3339());
        let (id,title)=w.app.agent.visible().iter().find(|t|!t.suggestion.change.operations.is_empty()).map(|t|(t.suggestion.id.clone(),t.suggestion.title.clone())).expect("当てられる提案が無い");
        let before=w.app.generated.project_of(&app).unwrap().unwrap().0.revision;
        w.app.apply(&ctx,Intent::Proposal{id,act:crate::ui::agent_rail::ProposalAct::Apply});
        assert_eq!(w.app.shell.chat_note.as_deref(),Some(format!("Applied: {title} · Undo is under Recently applied").as_str()),"提案の Apply が proposal() に届かない");
        assert_eq!(w.app.generated.project_of(&app).unwrap().unwrap().0.revision,before+1,"提案の Apply が 1 revision を書かない");
    }

    /// CHAT-011 反証 B28（121「suggest は chat 内で自動で出る」）: 件数の提案は製品の道で立つ = Page の data が替わった次の frame に、app が今の Page の件数を
    /// agent に渡して（`Generated::agent_rows` → `observe` → `refresh`）「Add search to …」が出る。試験は agent に触らず、Page の `Log it` と同じ action を Host から 11 回。
    /// 負の対照（束の arm）: sync_agent の observe の行を外す → 「件数が替わっても探す欄の提案が立たない」で赤。
    #[test] fn chat011_b28_many_rows_on_the_page_raise_a_search_suggestion_by_themselves(){
        use crate::domain::action::{ActionInvocation,InvocationOrigin};
        use crate::ui::renderer::ActionHost;
        let mut w=Win::open("chat011-b28");
        let app=w.app.generated.create("家計を見える化したい").expect("作れない");
        let view=w.app.generated.project_view_of(&app).expect("Project の View が無い");
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::Go(view));
        w.step(0.1,false,vec![]);
        let search=|w:&Win|w.app.agent.visible().iter().any(|t|t.suggestion.title.starts_with("Add search to"));
        assert!(w.app.agent.app().is_some(),"今の Page の app を agent が読んでいない");
        assert!(!search(&w),"件数の前から探す欄の提案が在る（試験の前提が崩れた）");
        {
            let (def,mut h)=crate::generated::tests::host_of(&mut w.app.generated,&app);
            let add=def.actions.iter().find(|a|a.id.ends_with(".add")).expect("足す操作が無い");
            // 必須の欄だけ（家計 = 金額）を型どおりに埋める
            let input:serde_json::Map<String,serde_json::Value>=add.input_schema.get("required").and_then(|r|r.as_array()).into_iter().flatten().filter_map(|k|k.as_str()).map(|k|{
                let ty=add.input_schema.get("properties").and_then(|p|p.get(k)).and_then(|p|p.get("type")).and_then(|t|t.as_str()).unwrap_or("string");
                (k.to_owned(),if matches!(ty,"number"|"integer"){serde_json::json!(1)}else{serde_json::json!("x")})
            }).collect();
            for _ in 0..=crate::agent::candidate::SEARCH_ROWS_THRESHOLD{
                h.invoke(ActionInvocation{action_ref:add.id.clone(),origin:InvocationOrigin::Button{node_key:"entry".into()},input:serde_json::Value::Object(input.clone())});
                assert!(h.line.is_none(),"1 件足すと Verified でない: {:?}",h.line);
            }
        }
        w.step(0.1,false,vec![]);
        assert!(search(&w),"件数が替わっても探す欄の提案が立たない: {:?}",w.app.agent.visible().iter().map(|t|t.suggestion.title.clone()).collect::<Vec<_>>());
    }

    /// CHAT-011 反証 Y1（121「suggest は chat 内で自動で出る」）: 畳んだ既定の panel でも、新しい提案が立った次の frame に panel が開き、提案の card が見える
    /// （前は Chat の tab を自分で開くまで提案は無いのと同じ）。負の対照（束の arm）: show_card の鍵から提案の腕を外す → 「新しい提案で panel が開かない」で赤。
    #[test] fn chat011_y1_a_new_suggestion_opens_the_folded_panel(){
        use crate::ui::shell::View;
        let mut w=Win::open("chat011-y1");
        let app=w.app.generated.create("家計を見える化したい").expect("作れない");
        let view=w.app.generated.project_view_of(&app).expect("Project の View が無い");
        let View::Project{project,..}=view.clone() else{panic!("Project でない")};
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::Go(view));
        w.step(0.1,false,vec![]);
        assert!(!w.app.generated.panel(&project).open,"提案の前から panel が開いている（試験の前提が崩れた）");
        let rows=w.app.agent.app().expect("今の Page の app を agent が読んでいない").definition().sources.iter().map(|s|(s.id.clone(),25)).collect();
        w.app.agent.observe(crate::agent::Observed{rows,..Default::default()});
        w.app.agent.refresh(&crate::agent::now_rfc3339());
        assert!(!w.app.agent.visible().is_empty(),"提案が立たない");
        w.step(0.1,false,vec![]);
        assert!(w.app.generated.panel(&project).open,"新しい提案で panel が開かない");
        let out=w.step(0.1,false,vec![]);
        assert!(ax_words(&out).iter().any(|l|l=="Suggestion"),"開いた panel に提案の card が無い: {:?}",ax_words(&out));
    }

    /// RQ3 反証の直し（home.rs:44）: 設定の file が壊れて隔離された起動は、Home の 1 行の下に `Restore…`（押すと Settings › Data & Recovery）。
    /// 前は状態の行だけに出て Home に届かず、`Restore…` の道が死んでいた。負の対照（commit message）: app.rs の load_note を note に移す match を外す → Restore… が無くて赤。
    #[test] fn rq3_damaged_settings_offer_restore_on_home(){
        use crate::ui::shell::{View,SettingsSection};
        let dir=std::env::temp_dir().join(format!("shibubu-rq3-bad-settings-{}",crate::storage::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sessions.json"),b"{ not json").unwrap();
        let mut w=Win::open_in(dir);
        let out=w.step(0.1,false,vec![]);
        let nodes=&out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes;
        let texts:Vec<String>=nodes.iter().filter_map(|(_,n)|n.label().or(n.value()).map(str::to_owned)).collect();
        assert!(texts.iter().any(|l|l=="Your settings were damaged and set aside · Settings › Data & Recovery"),"隔離の 1 行が Home に無い: {texts:?}");
        let restore=nodes.iter().find(|(_,n)|n.label()==Some("Restore…")).map(|(id,_)|*id).unwrap_or_else(||panic!("Restore… が無い: {texts:?}"));
        ax_click(&mut w,restore);
        assert_eq!(w.app.shell.view,View::Settings(SettingsSection::Data));
    }

    /// RQ2 反証の直し: Home の行の `•••` → Rename の後に Delete… → Delete で消すと、名前の欄が消した Project を指したまま残らない（欄は Home の `↵ Send` に戻る）。
    /// 負の対照（commit message）: DeleteProject の腕の `end_rename` を外す → renaming が残って赤。
    #[test] fn rq2_deleting_the_project_being_renamed_closes_the_name_field(){
        use crate::ui::shell::View;
        let mut w=Win::open("rq2-del-rename");
        let Some(View::Project{project,..})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Home);
        w.app.shell.start_rename(project.clone(),"Dev environment".into());
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::DeleteProject(project.clone()));
        assert!(w.app.generated.app_of_project(&project).is_none(),"消えていない: {}",w.app.message);
        assert_eq!((w.app.shell.renaming.as_deref(),w.app.shell.enter_label(&[]).as_str()),(None,"↵ Send"),"消した Project の名前の欄が残った");
    }

    /// RQ2 反証の直し: 使っている account を消した時、その account への切り替えの確かめが走っていれば、その答えを受け取らない（受けると消した account の CLI が入る）。
    /// 負の対照（commit message）: RemoveAiAccount の腕で `llm_switch=None` と rx の差し替えを外す（前の `if self.llm_rx.is_none()` に戻す）→ 古い答えの受け口が残って赤。
    #[test] fn rq2_removing_the_account_being_switched_to_drops_the_pending_check(){
        let mut w=Win::open("rq2-remove-race");
        let acct=crate::model::AiAccount::Plan{id:"plan-1".into()};
        w.app.store.ui.ai_accounts=vec![acct.clone()];
        w.app.store.ui.ai_active=Some("plan-1".into());
        let (tx,rx)=std::sync::mpsc::channel::<Result<Box<dyn crate::llm::Llm>,crate::llm::LlmError>>();
        (w.app.llm_rx,w.app.llm_switch)=(Some(rx),Some((None,None)));
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::RemoveAiAccount(acct));
        assert_eq!((w.app.store.ui.ai_active.as_deref(),w.app.llm_switch.is_some()),(None,false),"消した後も切り替えを待っている");
        assert!(tx.send(Err(crate::llm::LlmError::NoCli{detail:"old".into()})).is_err(),"消した account の確かめの答えを、まだ受け取る");
    }

    /// G84（operator 10-29）: 手で書いた設定で id が重なる 2 つ（EnvKey と Plan{id:"env"}）のうち、Remove は押した 1 つだけを消し、Re-authenticate は押した 1 つだけをし直す
    /// （id だけで引くと、Remove は 2 つとも消して利用者の login を黙って消す・Re-authenticate は列で先の EnvKey をし直す）。本物の `claude auth login` は起こさない
    /// （plan の dir が無いので reauthenticate は sign in の前に「sign-in is gone」で返る・EnvKey なら「Paste API key instead」）。
    /// 負の対照（g-b の変異）: gb_remove = Remove の retain を id で比べる → 2 つとも消えて赤・gb_reauth = Re-authenticate を id で引く → EnvKey の答えで赤。
    #[test] fn g84_a_shared_id_removes_and_reauthenticates_only_the_pressed_account(){
        use crate::model::AiAccount;
        let mut w=Win::open("g84-shared-id-remove");
        let ctx=w.ctx.clone();
        let plan=AiAccount::Plan{id:"env".into()};
        (w.app.store.ui.ai_accounts,w.app.store.ui.ai_active,w.app.llm_rx)=(vec![AiAccount::EnvKey,plan.clone()],None,None);
        w.app.apply(&ctx,Intent::ReauthAiAccount(plan.clone()));
        let (_,rx)=w.app.ai_sign_in.take().expect("Re-authenticate が sign in を起こさない");
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)),Ok(Err("This account's sign-in is gone · Remove it and add it again".into())),"押した plan でない account をし直した");
        w.app.store.ui.ai_active=Some("env".into());
        w.app.apply(&ctx,Intent::RemoveAiAccount(plan));
        assert_eq!(w.app.message,"Removed Claude · env");
        assert_eq!(w.app.store.ui.ai_accounts,[AiAccount::EnvKey],"押していない account も消した");
        assert_eq!(w.app.store.ui.active_account(),Some(&AiAccount::EnvKey),"使っている EnvKey を外した");
    }

    /// REVIEW-RQN（operator 4）: plan の account の Remove は、その account の dir の logout を app の `sign_out` の口で 1 回起こし（試験の窓は偽の口・本物の
    /// `claude auth logout` を起こさない）、その後に dir を消す。UI の thread では待たない（列からは今外す）。
    /// 負の対照（commit message）: forget_account の plan の腕で `self.sign_out` の代わりに `|_|Ok(())` を渡す → 記録が 0 で赤。
    #[test] fn rqn_removing_a_plan_signs_out_through_the_injected_door(){
        use crate::model::AiAccount;
        static SEEN:std::sync::Mutex<Vec<String>>=std::sync::Mutex::new(Vec::new());
        fn record(env:&[(&str,Option<String>)])->Result<(),String>{
            SEEN.lock().unwrap().extend(env.iter().filter(|(k,_)|*k=="CLAUDE_CONFIG_DIR").filter_map(|(_,v)|v.clone()));Ok(())
        }
        let mut w=Win::open("rqn-remove-signout");
        w.app.llm_rx=None;
        let login=w.dir.join("ai-accounts").join("plan-7");
        std::fs::create_dir_all(&login).unwrap();
        w.app.store.ui.ai_accounts=vec![AiAccount::Plan{id:"plan-7".into()}];
        w.app.sign_out=record;
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::RemoveAiAccount(AiAccount::Plan{id:"plan-7".into()}));
        assert!(w.app.store.ui.ai_accounts.is_empty(),"列から今外していない");
        let t0=std::time::Instant::now();
        while login.exists()&&t0.elapsed()<std::time::Duration::from_secs(5){std::thread::sleep(std::time::Duration::from_millis(20));}
        assert!(!login.exists(),"dir を消していない");
        let seen=SEEN.lock().unwrap().clone();
        assert_eq!(seen.len(),1,"logout の口を 1 回通っていない: {seen:?}");
        assert!(seen[0].ends_with("ai-accounts/plan-7"),"{seen:?}");
    }

    /// RQ-ACCT（RQ-N）: Settings › AI & Models に `Add account` と plan の行の `Re-authenticate` が在り、押すと Add の腕に届く（AI の確かめの途中なので
    /// 理由の 1 行で止まり、本物の `claude auth login` は起こさない）。sign in の途中は 2 つを描かず「Finish signing in in your browser」の 1 行。
    /// 負の対照（commit message）: settings.rs の `Add account` の `busy.is_none()` を外す → sign in の途中も描いて赤。
    #[test] fn rqn_settings_offers_add_and_reauthenticate_and_hides_them_while_signing_in(){
        use crate::ui::shell::{View,SettingsSection};
        let mut w=Win::open("rqn-acct-buttons");
        w.app.store.ui.ai_accounts=vec![crate::model::AiAccount::Plan{id:"plan-1".into()}];
        w.app.go(View::Settings(SettingsSection::AiModels));
        let labels=|out:&egui::FullOutput|->Vec<(NodeId,String)>{out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().or(n.value()).map(|l|(*id,l.to_owned()))).collect()};
        let out=w.step(0.1,true,vec![]);
        let names=labels(&out);
        assert!(names.iter().any(|(_,l)|l=="Re-authenticate Claude 2"),"plan の行に Re-authenticate が無い: {names:?}");
        let add=names.iter().find(|(_,l)|l=="Add Claude account").map(|(id,_)|*id).unwrap_or_else(||panic!("Add account が無い: {names:?}"));
        // 確かめの途中（受け口を持ったまま）に押す → 腕まで届き、裏の sign in は起こさない
        let (_hold,rx)=std::sync::mpsc::channel::<Result<Box<dyn crate::llm::Llm>,crate::llm::LlmError>>();
        w.app.llm_rx=Some(rx);
        ax_click(&mut w,add);
        assert_eq!((w.app.message.as_str(),w.app.ai_sign_in.is_some()),("Still checking the AI · try again in a moment",false),"Add account が腕に届かない");
        let (_tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((None,rx));
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::ReauthAiAccount(crate::model::AiAccount::Plan{id:"plan-1".into()}));
        assert_eq!(w.app.message,"Finish the sign-in open in your browser first","2 本目の sign in を起こそうとした");
        let names=labels(&w.step(0.1,true,vec![]));
        assert!(names.iter().any(|(_,l)|l.contains("Finish signing in in your browser")),"sign in の途中の 1 行が無い: {names:?}");
        assert!(!names.iter().any(|(_,l)|l=="Add Claude account"||l.starts_with("Re-authenticate")),"sign in の途中に Add / Re-authenticate を描いた: {names:?}");
    }

    /// RQ-ACCT（RQ-N）: 足した account は列に入るが使う account は変わらない（選ぶのは Use）・Re-authenticate の失敗は列・使っている account・AI の確かめに触らない・
    /// 使っている account の Re-authenticate が通れば確かめ直す。負の対照（commit message）: 足した腕で `ai_active` を書く → 赤・失敗の腕で確かめ直す → 赤。
    #[test] fn rqn_added_account_waits_for_use_and_a_failed_reauthenticate_changes_nothing(){
        use crate::model::AiAccount;
        let mut w=Win::open("rqn-acct-result");
        let ctx=w.ctx.clone();
        let (p1,p2)=(AiAccount::Plan{id:"plan-1".into()},AiAccount::Plan{id:"plan-2".into()});
        w.app.store.ui.ai_accounts=vec![p1.clone()];
        let (tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((None,rx));
        tx.send(Ok(p2.clone())).unwrap();
        w.app.poll_account_sign_in(&ctx);
        assert_eq!((w.app.store.ui.ai_accounts.clone(),w.app.store.ui.ai_active.clone(),w.app.ai_sign_in.is_some()),(vec![p1.clone(),p2.clone()],None,false));
        assert_eq!(w.app.message,"Account added · press Use to switch to it");
        w.app.store.ui.ai_active=Some("plan-1".into());
        w.app.llm_rx=None;
        let (tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((Some("plan-1".into()),rx));
        tx.send(Err(crate::llm_cli::SIGN_IN_FAILED.into())).unwrap();
        w.app.poll_account_sign_in(&ctx);
        assert_eq!(w.app.message,crate::llm_cli::SIGN_IN_FAILED);
        assert_eq!((w.app.store.ui.ai_accounts.len(),w.app.store.ui.ai_active.as_deref(),w.app.llm_rx.is_some()),(2,Some("plan-1"),false),"失敗した Re-authenticate が何かを変えた");
        let (tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((Some("plan-1".into()),rx));
        tx.send(Ok(p1)).unwrap();
        w.app.poll_account_sign_in(&ctx);
        assert_eq!((w.app.message.as_str(),w.app.llm_rx.is_some(),w.app.store.ui.ai_accounts.len()),("Signed in again",true,2),"使っている account の login をし直したのに確かめ直さない");
        let (tx,rx)=std::sync::mpsc::channel::<Result<AiAccount,String>>();
        w.app.ai_sign_in=Some((None,rx));
        drop(tx);
        w.app.poll_account_sign_in(&ctx);
        assert_eq!((w.app.message.as_str(),w.app.ai_sign_in.is_some()),(crate::llm_cli::SIGN_IN_FAILED,false),"sign in の thread が落ちたのを言わない");
    }

    /// REVIEW-RQN（operator 1）: Re-authenticate の途中にその account を Remove → 止める印が立ち、答えは捨てる（後で sign in が通っても「Signed in again」を言わない）・
    /// 1 行は `Removed {account}` だけ。別の account の sign in（Add の途中）は止めない。
    /// 負の対照（commit message）: Remove の腕の `self.ai_sign_in=None` を外す → 後の答えで「Signed in again」になって赤。
    #[test] fn rqn_removing_an_account_stops_its_sign_in(){
        use crate::model::AiAccount;
        use std::sync::atomic::Ordering;
        let mut w=Win::open("rqn-remove-signing");
        w.app.llm_rx=None;
        let ctx=w.ctx.clone();
        w.app.store.ui.ai_accounts=vec![AiAccount::Plan{id:"plan-1".into()},AiAccount::Plan{id:"plan-2".into()}];
        let (tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((Some("plan-1".into()),rx));
        let stop=w.app.sign_in_stop.clone();
        w.app.apply(&ctx,Intent::RemoveAiAccount(AiAccount::Plan{id:"plan-1".into()}));
        assert!(stop.load(Ordering::SeqCst),"走っている sign in を止めていない");
        assert_eq!(w.app.message,"Removed Claude 2");
        let _=tx.send(Ok(AiAccount::Plan{id:"plan-1".into()})); // sign in が後から通った
        w.app.poll_account_sign_in(&ctx);
        w.step(0.1,true,vec![]);
        assert_eq!(w.app.message,"Removed Claude 2","消した account の sign in の答えを言った");
        assert_eq!(w.app.store.ui.ai_accounts,vec![AiAccount::Plan{id:"plan-2".into()}]);
        // Add の途中に別の account を消しても、その sign in は止めない
        let (_tx,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((None,rx));
        w.app.sign_in_stop=Default::default();
        w.app.apply(&ctx,Intent::RemoveAiAccount(AiAccount::Plan{id:"plan-2".into()}));
        assert!(w.app.ai_sign_in.is_some()&&!w.app.sign_in_stop.load(Ordering::SeqCst),"別の account の sign in まで止めた");
    }

    /// STATUSBAR-1101 追記 210: 帯に並べる AI は providers の全部・今の AI が先頭・読めるのは今の Claude の plan だけ（窓ごとの `42% used 2h 10m`）・他は数字を作らず理由の 1 行・
    /// login が切れたのは読みの失敗ではない。負の対照（commit message）: `usage.filter` の `*on` を外す → 今でない plan に今の数字が付いて赤・
    /// `usage_step(r)==Some(TRY_AGAIN)` を `r.is_err()` に → SIGNED_OUT が失敗になって赤。
    #[test] fn statusbar_210_lists_every_ai_with_its_usage_or_why(){
        use crate::llm::{LlmError,PlanUsage};
        use crate::model::AiAccount;
        let providers=vec![
            (None,"System default".to_owned(),false),
            (Some(AiAccount::Plan{id:"work".into()}),"Claude plan · work".to_owned(),true),
            (Some(AiAccount::Key{id:"k1".into(),last4:"abcd".into()}),"API key ····abcd · billed per request by Anthropic".to_owned(),false),
            (Some(AiAccount::Found{id:"ollama".into(),name:"Ollama".into(),needs_sign_in:false}),"Ollama".to_owned(),false),
            (Some(AiAccount::Found{id:"gemini".into(),name:"Gemini".into(),needs_sign_in:false}),"Gemini".to_owned(),false),
        ];
        let none=std::collections::BTreeMap::new();
        let ok:Result<PlanUsage,LlmError>=Ok(PlanUsage{windows:vec![("Session".into(),42,Some(7_800)),("Weekly".into(),18,None)],checked_at:0});
        let ais=super::bar_ais(&providers,"Claude Code",Some(&ok),&none,0);
        let names:Vec<&str>=ais.iter().map(|a|a.name.as_str()).collect();
        assert_eq!(names,["Claude plan · work","System default","API key ····abcd","Ollama","Gemini"],"今の AI が先頭で残りは providers の順");
        assert_eq!(ais[0].windows,vec![(String::from("Session"),String::from("42% used 2h 10m"),42u8),(String::from("Weekly"),String::from("18% used"),18u8)]);
        assert!(ais[0].active&&!ais[0].failed);
        assert!(ais[1..].iter().all(|a|!a.active&&a.windows.is_empty()&&!a.failed&&!a.why.is_empty()),"今でない AI に数字を付けた / 理由が無い: {ais:?}");
        assert_eq!(ais[2].why,"Billed per request · no plan limit to show");
        assert_eq!(ais[3].why,"Runs on this Mac · no usage limit");
        assert_eq!(ais[4].why,"Shibubu can't read Gemini's usage yet");
        let failed=super::bar_ais(&providers,"Claude Code",Some(&Err(LlmError::Network{detail:"offline".into()})),&none,0);
        assert!(failed[0].failed&&failed[0].windows.is_empty()&&failed[0].why=="Usage couldn't be read","{:?}",failed[0]);
        let out=super::bar_ais(&providers,"Claude Code",Some(&Err(LlmError::NoCli{detail:crate::llm_cli::SIGNED_OUT.into()})),&none,0);
        assert!(!out[0].failed&&out[0].why==crate::llm_cli::SIGNED_OUT,"login が切れたのを読みの失敗にした: {:?}",out[0]);
        // System default が今の AI = その AI の名前・まだ読んでいない = 数字なし
        let mut sys=providers.clone();
        sys[0].2=true;
        sys[1].2=false;
        let unread=super::bar_ais(&sys,"Claude Code",None,&none,0);
        assert_eq!((unread[0].name.as_str(),unread[0].windows.len(),unread[0].why.as_str()),("Claude Code",0,"Usage isn't read yet"));
    }

    /// STATUSBAR-1101 追記 2: Grok・Kimi Code は今の AI でなくても読めた窓が並ぶ（`cli`）・読む前は「まだ」・Claude の plan の数字は付かない・
    /// 足した CLI の account（`Plan { id: "opencode-1" }`）は Claude の plan と数えない（31a19203 では Claude の行の理由が出ていた）。
    /// 負の対照（commit message）: `cli.get` を外す → Grok に窓が無く赤・`claude` を `matches!(acct, None | Some(Plan))` に戻す → opencode-1 の理由が Claude の物で赤。
    #[test] fn statusbar_210_2_grok_and_kimi_show_their_own_usage(){
        use crate::llm::{LlmError,PlanUsage};
        use crate::model::AiAccount;
        let providers=vec![
            (None,"System default".to_owned(),true),
            (Some(AiAccount::Found{id:"grok".into(),name:"Grok".into(),needs_sign_in:false}),"Grok".to_owned(),false),
            (Some(AiAccount::Found{id:"kimi".into(),name:"Kimi Code".into(),needs_sign_in:false}),"Kimi Code".to_owned(),false),
            (Some(AiAccount::Plan{id:"opencode-1".into()}),"opencode · 1".to_owned(),false),
        ];
        let mut cli=std::collections::BTreeMap::new();
        cli.insert("grok".to_owned(),Ok::<PlanUsage,LlmError>(PlanUsage{windows:vec![("Weekly".into(),30,None)],checked_at:0}));
        let claude:Result<PlanUsage,LlmError>=Ok(PlanUsage{windows:vec![("Session".into(),42,None)],checked_at:0});
        let ais=super::bar_ais(&providers,"Claude Code",Some(&claude),&cli,0);
        assert_eq!(ais[1].windows,vec![(String::from("Weekly"),String::from("30% used"),30u8)],"Grok の読みが並ばない: {:?}",ais[1]);
        assert!(!ais[1].failed&&!ais[1].active);
        assert_eq!((ais[2].windows.len(),ais[2].why.as_str()),(0,"Usage isn't read yet"),"読む前の Kimi Code");
        assert_eq!((ais[3].windows.len(),ais[3].why.as_str()),(0,"Shibubu can't read opencode's usage yet"),"足した opencode の account を Claude の plan と数えた");
        cli.insert("kimi".to_owned(),Err(LlmError::Network{detail:"offline".into()}));
        let failed=super::bar_ais(&providers,"Claude Code",Some(&claude),&cli,0);
        assert!(failed[2].failed&&failed[2].why=="Usage couldn't be read","{:?}",failed[2]);
    }

    /// RQ-USAGE（RQ-N）× STATUSBAR-1101（追記 210）: 窓の一番下の帯に使用量（窓ごとに細い棒と `42% used 2h 10m`・読めたら ↻）・読みの失敗は `⚠ Refresh failed`（理由は hover）と `Try again`
    /// （押すと 60 秒を待たずに読み直す）・login が切れた時は `--`（AX に理由）と `Sign in`（押すと sign in の腕へ）・鍵の account は 1 行を出さず `--` と理由だけ（↻・Try again 無し）・
    /// Settings › AI & Models は 1 行と、読めた時は `Refresh`。偽の AI（答えを決めて返す・model も Keychain も usage の口も呼ばない）。
    /// 負の対照（commit message）: update の帯の `refresh_usage` を外す → 1 つ目で赤・帯の「次の一手の無い NoCli を None に」を外す → 鍵の場面で赤・`RefreshUsage` の腕の force を false に → 読み直さず赤。
    /// 途中の sign in を最初から持たせる（どの変異が `AiStep(Sign in)` に流れても本物の `claude auth login` を起こさない）。
    #[test] fn rqn_usage_band_says_the_numbers_or_why_and_the_next_step(){
        use crate::llm::{LlmError,PlanUsage};
        use crate::ui::shell::{View,SettingsSection};
        type Answer=std::sync::Arc<std::sync::Mutex<Result<PlanUsage,LlmError>>>;
        struct Fake(Answer);
        impl crate::llm::Llm for Fake{
            fn label(&self)->String{"fake".into()}
            fn send(&self,_:&crate::llm::JsonRequest)->Result<serde_json::Value,LlmError>{unreachable!("使用量の読みで model を呼んだ")}
            fn plan_usage(&self)->Result<PlanUsage,LlmError>{self.0.lock().unwrap().clone()}
        }
        let labels=|out:&egui::FullOutput|->Vec<(NodeId,String)>{out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().or(n.value()).map(|l|(*id,l.to_owned()))).collect()};
        // 裏の読みが届いて描かれるまで frame を回す（最大 5 秒）
        let until=|w:&mut Win,want:&dyn Fn(&[(NodeId,String)])->bool|->Vec<(NodeId,String)>{
            let t0=Instant::now();
            loop{
                let names=labels(&w.step(0.1,true,vec![]));
                if want(&names)||t0.elapsed()>Duration::from_secs(5){return names}
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let mut w=Win::open("rqn-usage-band");
        w.app.llm_rx=None;w.app.read_usage=true;w.app.usage_asked=true; // 押す前の形は rqn_usage_is_read_only_after_the_person_asks
        let (_hold,rx)=std::sync::mpsc::channel();
        w.app.ai_sign_in=Some((None,rx));
        let now=crate::google::now() as i64;
        let answer:Answer=std::sync::Arc::new(std::sync::Mutex::new(Ok(PlanUsage{windows:vec![("Session".into(),42,Some(now+7_800)),("Weekly".into(),18,None)],checked_at:now})));
        // 読めた: 帯に窓ごとの % と resets in・次の一手の button は無く ↻（Refresh usage）だけ（60 秒ごとにも読み直す）
        w.app.generated.set_llm(Ok(Box::new(Fake(answer.clone()))));
        // STATUSBAR-1101 追記 210: 窓ごとの語は Orca と同じ `42% used 2h 10m`（AX は窓の名前つき）
        let names=until(&mut w,&|n:&[(NodeId,String)]|n.iter().any(|(_,l)|l.starts_with("Session · 42% used")));
        assert!(names.iter().any(|(_,l)|l.starts_with("Session · 42% used 2h"))&&names.iter().any(|(_,l)|l=="Weekly · 18% used"),"帯に窓ごとの使用量が出ない: {names:?}");
        assert!(!names.iter().any(|(_,l)|l=="Try again"||l=="Sign in"||l=="Refresh"),"読めたのに帯に次の一手の button: {names:?}");
        assert!(names.iter().any(|(_,l)|l=="Refresh usage"),"読めた帯に ↻ が無い: {names:?}");
        assert!(!names.iter().any(|(_,l)|l=="Usage in Shibubu"),"USAGE-OWN-1101 追記 223: 帯に requests today の語を戻した: {names:?}");
        // 取れない: 理由の 1 行と Try again・押すと待たずに読み直す
        *answer.lock().unwrap()=Err(LlmError::Network{detail:"offline".into()});
        w.app.generated.set_llm(Ok(Box::new(Fake(answer.clone()))));
        // 210: 読みの失敗は `⚠ Refresh failed`（理由 `Usage couldn't be read` は hover）
        let names=until(&mut w,&|n:&[(NodeId,String)]|n.iter().any(|(_,l)|l=="⚠ Refresh failed"));
        assert!(names.iter().any(|(_,l)|l=="⚠ Refresh failed"),"読みの失敗の語が無い: {names:?}");
        let again=names.iter().find(|(_,l)|l=="Try again").map(|(id,_)|*id).unwrap_or_else(||panic!("Try again が無い: {names:?}"));
        *answer.lock().unwrap()=Ok(PlanUsage{windows:vec![("Session".into(),7,None)],checked_at:now});
        ax_click(&mut w,again);
        let names=until(&mut w,&|n:&[(NodeId,String)]|n.iter().any(|(_,l)|l=="Session · 7% used"));
        assert!(names.iter().any(|(_,l)|l=="Session · 7% used"),"Try again で読み直さない（60 秒を待った）: {names:?}");
        // login が切れた: 理由の 1 行と Sign in・押すと sign in の腕へ（途中の sign in を持たせ、本物の login を起こさずに届いた事だけ見る）
        *answer.lock().unwrap()=Err(LlmError::NoCli{detail:crate::llm_cli::SIGNED_OUT.into()});
        w.app.generated.set_llm(Ok(Box::new(Fake(answer.clone()))));
        // 210: login が切れたのは読みの失敗ではない = `--`（AX `{AI} usage: {理由}`・hover = 理由）と Sign in
        let names=until(&mut w,&|n:&[(NodeId,String)]|n.iter().any(|(_,l)|l.ends_with(&format!("usage: {}",crate::llm_cli::SIGNED_OUT))));
        let sign=names.iter().find(|(_,l)|l=="Sign in").map(|(id,_)|*id).unwrap_or_else(||panic!("login が切れたのに Sign in が無い: {names:?}"));
        ax_click(&mut w,sign);
        assert_eq!(w.app.message,"Finish the sign-in open in your browser first","帯の Sign in が sign in の腕に届かない");
        // 鍵の account: 使用量が無いので 1 行は帯に出さない（210: `--` の hover の理由だけ）
        *answer.lock().unwrap()=Err(LlmError::NoCli{detail:"API key · billed per use".into()});
        w.app.generated.set_llm(Ok(Box::new(Fake(answer.clone()))));
        let t0=Instant::now();
        while w.app.generated.plan_usage().is_none()&&t0.elapsed()<Duration::from_secs(5){w.step(0.1,true,vec![]);std::thread::sleep(Duration::from_millis(10));}
        assert!(w.app.generated.plan_usage().is_some(),"鍵の場面の読みが届かない");
        let names=labels(&w.step(0.1,true,vec![]));
        assert!(!names.iter().any(|(_,l)|l=="API key · billed per use"),"鍵の account の 1 行を帯に出した: {names:?}");
        assert!(names.iter().any(|(_,l)|l.ends_with("usage: API key · billed per use")),"210: 読めない AI の `--` に理由が無い: {names:?}");
        assert!(!names.iter().any(|(_,l)|l=="Refresh usage"||l=="Try again"),"読めない AI に読み直す手を出した: {names:?}");
        // Settings › AI & Models: 帯と同じ 1 行と Refresh
        *answer.lock().unwrap()=Ok(PlanUsage{windows:vec![("Session".into(),42,None)],checked_at:now});
        w.app.generated.set_llm(Ok(Box::new(Fake(answer.clone()))));
        w.app.go(View::Settings(SettingsSection::AiModels));
        let names=until(&mut w,&|n:&[(NodeId,String)]|n.iter().any(|(_,l)|l.starts_with("Session 42%"))&&n.iter().any(|(_,l)|l=="Session · 42% used"));
        assert!(names.iter().any(|(_,l)|l.starts_with("Session 42%"))&&names.iter().any(|(_,l)|l=="Session · 42% used"),"Settings の 1 行と帯の窓が無い: {names:?}");
        assert!(names.iter().any(|(_,l)|l=="Refresh"),"Settings の読めた 1 行に Refresh が無い: {names:?}");
    }

    /// RQ-USAGE（operator ①・10-29）× STATUSBAR-1101: 起動しただけでは使用量を読まない（Keychain の口 = 偽の AI の `plan_usage` が 0 回）・帯は `Show usage` だけ（説明の 1 行は hover）で数字を出さない・
    /// 押して 1 回読む・Settings › AI & Models を開いても 1 回読む（帯の button を押さなくても）。
    /// 負の対照（commit message）: update の読みの `usage_asked` の条件を外す（起動で読む形）→ 0 回の所で赤・Settings の節で `usage_asked` を立てる行を外す → 2 つ目で赤。
    #[test] fn rqn_usage_is_read_only_after_the_person_asks(){
        use crate::llm::{LlmError,PlanUsage};
        use crate::ui::shell::{View,SettingsSection};
        use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
        struct Counted(Arc<AtomicUsize>);
        impl crate::llm::Llm for Counted{
            fn label(&self)->String{"fake".into()}
            fn send(&self,_:&crate::llm::JsonRequest)->Result<serde_json::Value,LlmError>{unreachable!("使用量の読みで model を呼んだ")}
            fn plan_usage(&self)->Result<PlanUsage,LlmError>{self.0.fetch_add(1,Ordering::SeqCst);Ok(PlanUsage{windows:vec![("Session".into(),42,None)],checked_at:crate::google::now() as i64})}
        }
        let labels=|out:&egui::FullOutput|->Vec<(NodeId,String)>{out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().or(n.value()).map(|l|(*id,l.to_owned()))).collect()};
        let start=|tag:&str|->(Win,Arc<AtomicUsize>){
            let mut w=Win::open(tag);
            w.app.llm_rx=None;w.app.read_usage=true;
            let reads=Arc::new(AtomicUsize::new(0));
            w.app.generated.set_llm(Ok(Box::new(Counted(reads.clone()))));
            (w,reads)
        };
        // 起動しただけ: 1 秒ほど frame を回しても読まない・帯は Show usage だけ（長い説明の文と数字は出さない）
        let (mut w,reads)=start("rqn-usage-ask");
        let mut names=Vec::new();
        for _ in 0..10{names=labels(&w.step(0.1,true,vec![]));std::thread::sleep(Duration::from_millis(20));}
        assert_eq!(reads.load(Ordering::SeqCst),0,"押す前に使用量（Keychain）を読んだ");
        assert!(!names.iter().any(|(_,l)|l==crate::ui::shell::USAGE_ASK),"読む前の長い説明を帯に出した（hover だけ）: {names:?}");
        assert!(!names.iter().any(|(_,l)|l.starts_with("Session")),"読む前に数字を出した: {names:?}");
        let show=names.iter().find(|(_,l)|l==super::SHOW_USAGE).map(|(id,_)|*id).unwrap_or_else(||panic!("Show usage が無い: {names:?}"));
        ax_click(&mut w,show);
        let t0=Instant::now();
        while !labels(&w.step(0.1,true,vec![])).iter().any(|(_,l)|l=="Session · 42% used")&&t0.elapsed()<Duration::from_secs(5){std::thread::sleep(Duration::from_millis(10));}
        assert_eq!(reads.load(Ordering::SeqCst),1,"Show usage で 1 回読んでいない");
        // Settings › AI & Models を開いた時も読む
        let (mut w,reads)=start("rqn-usage-settings");
        w.settle();
        assert_eq!(reads.load(Ordering::SeqCst),0,"押す前に使用量（Keychain）を読んだ");
        w.app.go(View::Settings(SettingsSection::AiModels));
        let t0=Instant::now();
        while reads.load(Ordering::SeqCst)==0&&t0.elapsed()<Duration::from_secs(5){w.step(0.1,true,vec![]);std::thread::sleep(Duration::from_millis(10));}
        assert_eq!(reads.load(Ordering::SeqCst),1,"Settings › AI & Models を開いても読まない");
    }

    /// UI-5a: 本物の殻（keyboard の連鎖・header の ‹ の席・帯）の上で、⌘T で Page を足して移り、title line の ‹（AX `Back to {Page}`）で戻る。
    /// ••• の `Delete page…` で出た tab の横の確認は Esc で閉じて Page は消えない（dismiss_delete）。旧 canvas の `Delete canvas…` は Window でなく inline の確認（`Keep` で閉じる）。
    /// 負の対照（commit message）: ⌘T の consume を外す → Page が増えず赤・Esc の連鎖から dismiss_delete を外す → 確認が残って赤。
    #[test] fn ui5a_shell_keys_back_seat_and_escape_close_the_delete_ask(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui5a-shell");
        let Some(View::Project{project,page:first})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("主デモの一言から Project を作れない") else{panic!("主デモの一言は Project")};
        w.app.go(View::Project{project:project.clone(),page:first.clone()});
        fn names(out:&egui::FullOutput)->Vec<(NodeId,String)>{
            out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter().filter_map(|(id,n)|n.label().map(|l|(*id,l.to_owned()))).collect()
        }
        let has=|out:&egui::FullOutput,label:&str|names(out).iter().any(|(_,l)|l==label);
        let press=|w:&mut Win,label:&str|{
            let out=w.step(0.1,false,vec![]);
            let id=names(&out).into_iter().find(|(_,l)|l==label).unwrap_or_else(||panic!("{label} が AX に無い: {:?}",names(&out))).0;
            w.step(0.1,false,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:id,data:None})]);
        };
        let key=|k:egui::Key,m:egui::Modifiers|Event::Key{key:k,physical_key:None,pressed:true,repeat:false,modifiers:m};
        let page=|w:&Win|match &w.app.shell.view{View::Project{page,..}=>page.clone(),v=>panic!("Project の View ではない: {v:?}")};
        for _ in 0..3{w.step(0.1,false,vec![]);}
        assert!(!names(&w.step(0.1,false,vec![])).iter().any(|(_,l)|l.starts_with("Back to ")),"履歴が空なのに ‹ を描いた");
        w.step(0.1,false,vec![key(egui::Key::T,egui::Modifiers::COMMAND)]);
        let added=page(&w);
        assert_ne!(added,first,"⌘T で Page を足して移っていない");
        let out=w.step(0.1,false,vec![]);
        assert!(has(&out,"Back to Dev"),"‹ の AX の名前が戻る先の Page でない: {:?}",names(&out));
        press(&mut w,"Back to Dev");
        assert_eq!(page(&w),first,"‹ で戻っていない");
        w.app.go(View::Project{project:project.clone(),page:added.clone()});
        press(&mut w,"Page options");
        press(&mut w,"Delete page…");
        assert!(has(&w.step(0.1,false,vec![]),"Keep"),"tab の横の確認が出ていない");
        w.step(0.1,false,vec![key(egui::Key::Escape,egui::Modifiers::NONE)]);
        let out=w.step(0.1,false,vec![]);
        assert!(!has(&out,"Keep") && has(&out,"Page 4") && page(&w)==added,"Esc で確認を閉じて Page は残る: {:?}",names(&out));
        // 反証: 確認を開いたまま Project を離れたら閉じる（Home・Apps の最初の Esc を見えない確認が飲まない・overlay の rail が閉じる）
        press(&mut w,"Page options");
        press(&mut w,"Delete page…");
        assert!(has(&w.step(0.1,false,vec![]),"Keep"),"前提: tab の横の確認が出ている");
        w.app.go(View::Home);
        assert!(!w.app.generated.dismiss_delete(),"Project を離れても tab の横の確認が残る（Home の Esc を飲む）");
        w.app.new_blank_session(); // 2 つ目の canvas（1 つだけだと削除の行は理由の文で押せない）
        let session=w.app.store.selected;
        let n=w.app.store.sessions.len();
        w.app.go(View::CompatCanvas(session));
        // FX4: header の `•••` は無い。旧 canvas の削除は ⌘K の行 `Delete canvas…`（UI-6a から）
        w.key(egui::Key::K,CMD);
        w.app.shell.command="delete canvas".into();
        w.key(egui::Key::Enter,egui::Modifiers::NONE);
        let out=w.step(0.1,false,vec![]);
        assert!(has(&out,"Keep") && has(&out,"Delete"),"旧 canvas の確認が inline の 1 行でない: {:?}",names(&out));
        press(&mut w,"Keep");
        assert!(!has(&w.step(0.1,false,vec![]),"Keep") && w.app.store.sessions.len()==n,"Keep で閉じて canvas は残る");
    }

    /// UI-5a 反証: 旧 canvas の 2 つの確認（canvas・部品）は CompatCanvas の中に描く inline の行なので、別の View へ移ったら閉じる
    /// （閉じないと `cover` が Project の WebView を隠し続け、Esc の連鎖の最初の 1 回を見えない確認が飲む）。
    /// 負の対照（commit message）: `go` の確認を閉じる行を外す → 赤。
    #[test] fn ui5a_leaving_the_classic_canvas_closes_its_delete_asks(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui5a-compat-leave");
        let session=w.app.store.selected;
        let panel=w.app.store.selected().expect("canvas が無い").panels[0].id;
        w.app.go(View::CompatCanvas(session));
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,super::Intent::DeleteApp); // header の ••• の Delete canvas… と同じ intent
        w.app.selected_panel=Some(panel);w.app.delete_panel_prompt=Some(panel); // inspector の Delete part…
        w.step(0.1,false,vec![]);
        assert!(w.app.delete_session_prompt && w.app.delete_panel_prompt==Some(panel),"前提: 2 つの確認が開いている");
        w.app.go(View::Home);
        assert!(!w.app.delete_session_prompt && w.app.delete_panel_prompt.is_none(),"CompatCanvas を離れても旧 canvas の確認が残る（WebView を隠し Esc を飲む）");
    }

    /// UI-5a 反証: 部品の確認は選んでいる部品の inspector に描くので、部品を閉じた・別の部品を選んだら閉じる（見えない確認が Esc の最初の 1 回を飲まない）。
    /// 負の対照（commit message）: keyboard の頭の「選んでいる部品の確認だけ残す」行を外す → 赤。
    #[test] fn ui5a_closing_the_part_closes_its_delete_ask(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui5a-part");
        let session=w.app.store.selected;
        let panel=w.app.store.selected().expect("canvas が無い").panels[0].id;
        w.app.go(View::CompatCanvas(session));
        w.app.selected_panel=Some(panel);w.app.delete_panel_prompt=Some(panel);
        w.step(0.1,false,vec![]);
        assert_eq!(w.app.delete_panel_prompt,Some(panel),"前提: 選んでいる部品の確認は開いたまま");
        w.app.selected_panel=None; // inspector の × と同じ
        w.step(0.1,false,vec![]);
        assert!(w.app.delete_panel_prompt.is_none(),"部品を閉じても部品の確認が残る（Esc の最初の 1 回を飲む）");
    }

    /// UI-4a 決定 1 / Done 1・Done 3: 言葉の欄は窓に 1 つ（Terminal の grid を除く）で、x・y・幅・高さは Runs・Connections で同じ
    /// （窓の幅 960・1200・1440 の段ごと）。nav は Home・Runs・Connections で、Apps の行は無い・Project は sidebar の行から開ける。
    /// G83（owner 10-27）: Home の欄は主領域の縦の中央（`g83_home_puts_the_one_field_in_the_middle_of_the_main_region` が測る）= Home の行は rect の比べから外した
    /// （Home も欄は 1 つ・名前・Apps の行が無い事は見る）。CHAT-ONE-1101: Project の欄は下の panel の Chat の tab の中だけ（畳んだ panel = 欄は 0）= rect の比べは
    /// Runs と Connections の header の欄だけ。
    /// 負の対照（commit message）: Home に textarea を戻す → Home で欄が 2 つ・header の欄を `ui.add_sized` の右寄せに戻す → sidebar の幅で x がずれて赤。
    /// UI-SWEEP-1101（owner 196 通目の 11・12）: Runs と Connections は常の欄を描かない（⌘K の間だけ = `fx4_cmd_k_drops_the_list_under_the_one_field` が rect を見る）。
    /// 負の対照: header.rs の `&& shell.palette.open` を外す → Runs で欄が 1 つ出て赤。
    #[test] fn ui4a_one_field_in_the_same_rect_on_every_view(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4a-one-field");
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない").expect("主デモの一言は Project");
        for width in [960.0,1200.0,1440.0]{
            w.size=Vec2::new(width,800.0);
            for (want,view) in [("What to create",View::Home),("",project.clone()),("",View::Runs),("",View::Connections)]{
                w.app.go(view);
                for _ in 0..3{w.step(0.1,false,vec![]);}
                let out=w.step(0.1,false,vec![]);
                let fields=Win::word_fields(&out);
                if want.is_empty(){assert!(fields.is_empty(),"幅 {width}: 畳んだ panel の Project・Runs・Connections に常の欄を出した {fields:?}");continue}
                assert_eq!(fields.len(),1,"幅 {width}・{want}: 言葉の欄が 1 つでない {fields:?}");
                assert_eq!(fields[0].0,want,"幅 {width}");
                let t=out.platform_output.accesskit_update.as_ref().unwrap();
                let buttons:Vec<&str>=t.nodes.iter().filter(|(_,n)|n.role()==Role::Button).filter_map(|(_,n)|n.label()).collect();
                assert!(!buttons.contains(&"Apps"),"幅 {width}: Apps の行が残っている");
            }
        }
    }

    /// G83（owner 10-27「createするbarを画面の真ん中もしかしたら下ぐらいにおいて」）: Home の 1 つの欄は主領域の縦の中央（中心が主領域の高さの 45〜60%）・
    /// 横は本文の列の中央・幅は `Tier::input_w` 以上で列の内側。Try の chip と Continue は欄の下・右端の `Create` は欄の中。起動の骨格の欄の帯も同じ rect。
    /// 窓 960×700（sidebar を畳む段）・1200×800・1440×900。Home の ⌘↵ で Project を開いても focus は欄に残る（Id が置き場で替わらない）。
    /// 負の対照（commit message）: header が Home でも欄を描く → 欄が 2 つで赤・Home の欄の上の余白を消す → 縦の位置で赤・欄の Id を id_salt に戻す → focus で赤。
    /// G83b: Try の行の頭 = 欄の左端（±1・Continue は SESSIONS-LEFT-1101 で sidebar へ）。負の対照: home.rs の揃え（欄の左端までの余白と幅）を外す → 赤。
    #[test] fn g83_home_puts_the_one_field_in_the_middle_of_the_main_region(){
        use crate::ui::shell::{Tier,View};
        let mut w=Win::open("g83-home-field");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない").expect("主デモの一言は Project");
        w.app.go(View::Home);
        for (width,height) in [(960.0,700.0),(1200.0,800.0),(1440.0,900.0)]{
            w.size=Vec2::new(width,height);
            w.settle();
            let (_,sk,out)=w.probe();
            let fields=Win::word_fields(&out);
            assert_eq!(fields.len(),1,"幅 {width}: Home の言葉の欄が 1 つでない（header にも描いた？）{fields:?}");
            let (name,f)=(&fields[0].0,fields[0].1);
            assert_eq!(name,"What to create","幅 {width}");
            // 欄の Id は置き場で替わらない 1 つなので、header にも描くと AX の木では 1 つに潰れる。描いた欄の数は空の欄の placeholder の数で数える
            let hints=out.shapes.iter().filter(|s|matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()==View::Home.field_hint())).count();
            assert_eq!(hints,1,"幅 {width}: Home に欄の placeholder が {hints} 個（header にも欄を描いた？）");
            let at=((f[1]+f[3])/2.0-f64::from(sk.body.top()))/f64::from(sk.body.height());
            assert!((0.45..=0.60).contains(&at),"幅 {width}: 欄の中心が主領域の高さの {at:.3}（45〜60% でない）{f:?} 主領域 {:?}",sk.body);
            let col=sk.column;
            assert!(((f[0]+f[2])/2.0-f64::from(col.center().x)).abs()<0.5,"幅 {width}: 欄が本文の列の中央に無い {f:?} 列 {col:?}");
            assert!(f[2]-f[0]>=f64::from(Tier::for_width(width).input_w)-0.5&&f[0]>=f64::from(col.left())-0.5&&f[2]<=f64::from(col.right())+0.5,"幅 {width}: 欄の幅（input_w 以上・列の内側）{f:?} 列 {col:?}");
            let t=out.platform_output.accesskit_update.as_ref().unwrap();
            let rect_of=|name:&str|t.nodes.iter().find(|(_,n)|n.label()==Some(name)||n.value()==Some(name)).and_then(|(_,n)|n.bounds()).unwrap_or_else(||panic!("幅 {width}: 「{name}」が無い"));
            for chip in crate::ui::home::EXAMPLES.map(|e|e.0){
                assert!(rect_of(chip).y0>=f[3],"幅 {width}: 「{chip}」が欄の下に無い {:?} 欄 {f:?}",rect_of(chip));
            }
            let c=rect_of("Send");
            assert!(c.x0>=f[0]&&c.x1<=f[2]&&c.y0>=f[1]&&c.y1<=f[3],"幅 {width}: 右端の Send が欄の中に無い {c:?} 欄 {f:?}");
            // G83b: 欄の下の中身は欄と同じ左端（Try の行の頭・SESSIONS-LEFT-1101 で Continue の見出しと一覧は左の sidebar へ移した）
            assert!((rect_of("Try").x0-f[0]).abs()<=1.0,"幅 {width}: 「Try」の左端 {:?} が欄の左端 {} と違う",rect_of("Try"),f[0]);
            let s=[sk.field.min.x,sk.field.min.y,sk.field.max.x,sk.field.max.y];
            assert!(s.iter().zip(f).all(|(a,b)|(f64::from(*a)-b).abs()<0.5),"幅 {width}: 起動の骨格の欄 {s:?} が本物 {f:?} と違う");
        }
        // Home の Enter で送って（L2: 会話は focus を欄に戻す）案を Build now し Project を開いた後も、打つ手は欄に残る（Home の主領域と header の欄は同じ Id）
        w.submit("What to create",crate::ui::home::EXAMPLES[1].1,false);
        let ctx=w.ctx.clone();
        w.app.apply(&ctx,Intent::AcceptPlan);
        for _ in 0..2{w.step(0.1,true,vec![]);}
        assert!(matches!(w.app.shell.view,View::Project{..}),"案の Build now で Project を開いていない: {:?} {:?}",w.app.shell.view,w.app.generated.note);
        // CHAT-ONE-1101: Project の欄は Chat の tab の中だけ（開いた Project の panel は畳んだまま = 欄は 0）
        assert!(Win::word_fields(&w.step(0.1,true,vec![])).is_empty(),"畳んだ panel の Project に欄を出した");
    }

    /// G87（P0・owner 10-27「youtube aple musicのところを押しても反応しない」「上の+やその隣の点３つを押しても反応しない」）: 本物の mouse の 1 click
    /// （`Win::click_at`）で、Page の帯の tab・`＋`・`•••`・面の中の button（Mirror の `Close mirror`・feature mirror の build）・sidebar の行が効く。
    /// 形は owner の Project に寄せる: 主デモの Project（Diagram = WebView の面）に Mirror の面の Page（元の app の窓が無い）を足し、Mirror の Page から押す。
    /// AX の Click では通っていた（BG の driver は AX だけ = この道を一度も測っていなかった）。
    /// 負の対照（commit message）: renderer の node を選ぶ click 面（`kangae-pick`）を元の場所（`ui.clip_rect()` 全部・利用者の build でも作る）に戻す → 帯の click を click 面が取って赤。
    #[test] fn g87_a_mouse_click_reaches_the_page_band_the_face_and_the_sidebar(){
        use crate::ui::shell::View;
        let mut w=Win::open("g87-mouse");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let Some(View::Project{project,page:first})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        let Some(View::Project{project:same,page:mirror})=w.app.generated.create_project("notes.md を Mirror で見る Page を足して",None,None).expect("Mirror の Page を足せない") else{panic!("Mirror の Page")};
        assert_eq!(same,project,"前提: Mirror の Page は同じ Project に足す");
        let on=|w:&Win|match &w.app.shell.view{View::Project{page,..}=>page.clone(),v=>panic!("Project の View ではない: {v:?}")};
        w.app.go(View::Project{project:project.clone(),page:mirror.clone()});
        w.settle();
        let (left,_,out)=w.probe();
        // 帯の tab（先頭の Page = Dev）
        w.click_at(Win::center_of(&out,"Dev",left));
        assert_eq!(on(&w),first,"mouse の click で帯の tab が効かない（今の Page のまま）");
        // `＋` と `•••`: 1 click で menu が開く
        for (opener,item) in [("Add to page","Add browser"),("Page options","Delete page…")]{
            w.app.go(View::Project{project:project.clone(),page:mirror.clone()});
            w.settle();
            let (left,_,out)=w.probe();
            w.click_at(Win::center_of(&out,opener,left));
            let (_,_,out)=w.probe();
            assert!(Win::button(&out,item).is_some(),"mouse の click で「{opener}」の menu が開かない（「{item}」が無い）");
            w.key(egui::Key::Escape,egui::Modifiers::NONE);
        }
        // 面の中の button（Mirror の題の帯の赤 = Close mirror → 畳んだ帯 `Open mirror · TextEdit`）
        #[cfg(feature="mirror")]
        {
            w.app.go(View::Project{project:project.clone(),page:mirror.clone()});
            w.settle();
            let (left,_,out)=w.probe();
            w.click_at(Win::center_of(&out,"Close mirror",left));
            w.settle();
            let (_,_,out)=w.probe();
            assert!(Win::button(&out,"Open mirror · TextEdit").is_some(),"mouse の click で面の中の Close mirror が効かない");
        }
        // 同じ状態を 2 回出さない: Mirror の面の席の 1 行（usable）は AX の group だけで、Page の上に文を描かない（面の中が状態と次の一手の button を言う）
        w.app.go(View::Project{project:project.clone(),page:mirror.clone()});
        w.settle();
        let (_,_,out)=w.probe();
        let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        let ids:Vec<String>=w.app.generated.usable_of(&project).into_iter().filter(|u|u.seat.kind==crate::composer::verifier::SeatKind::Mirror).map(|u|format!("usable:{}",u.seat.id)).collect();
        let seats:Vec<(String,usize)>=t.nodes.iter().filter(|(_,n)|n.author_id().is_some_and(|a|ids.iter().any(|i|i==a))).map(|(_,n)|(n.label().unwrap_or_default().to_string(),n.children().len())).collect();
        assert!(!seats.is_empty(),"前提: Mirror の面の席の usable の group が無い（試験が測っていない）");
        assert!(seats.iter().all(|(_,kids)|*kids==0),"Mirror の面の状態を Page の上にも描いた: {seats:?}");
        // sidebar の行（主領域の外・前から効いていた = 正の対照）
        let (_,_,out)=w.probe();
        w.click_at(Win::center_of(&out,"Runs",0.0));
        assert_eq!(w.app.shell.view,View::Runs,"mouse の click で sidebar の Runs が効かない");
    }

    /// UI-4a Done 2（通し・本物の ShibubuApp の上・model 無し）: Home の欄に一言 → Enter は会話（L2・作らない・欄は空）→ 案の `Build now` で作って開く・Project の欄に変更 → Enter で当たり欄は空・
    /// 同じ変更をもう一度 → 拒否で欄に文が残る（何も当てない）・Home で名前の一致 → Enter で開く・⌘↵ は一致があっても開かず会話へ（L2: Home の ⌘↵ も作らない）。
    /// 負の対照（commit message）: app.rs の `take_edit_applied` の受け口を消す → 通った変更の後も欄に文が残り赤・generated の拒否の側で合図を立てる → 2 回目で欄が空になり赤。
    #[test] fn ui4a_enter_runs_by_the_place_through_the_one_field(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4a-enter");
        // 本物の claude CLI を呼ばない（名前の表で作る）・bridge を起こさない一言（Project にならない = create_now が bridge を起こさない）
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let one=crate::ui::home::EXAMPLES[1].1;
        assert!(!matches!(crate::composer::project::compose_project(one,&Default::default(),None),Ok(Some(_))),"前提: 「{one}」は Project にならない");
        w.step(0.1,true,vec![]);
        w.submit("What to create",one,false);
        // L2: Home の Enter は会話（作らない）・案の Build now で作る
        assert_eq!(w.app.shell.view,View::Home,"L2: Home の Enter で作った");
        assert_eq!(w.app.shell.command,"","会話に送った後も欄に一言が残る");
        w.app.apply(&w.ctx.clone(),Intent::AcceptPlan);
        for _ in 0..2{w.step(0.1,true,vec![]);}
        // 1 Page の app も Project に包んで開く（W2）
        let made=w.app.shell.view.clone();
        assert!(matches!(made,View::Project{..}),"Build now で作って開いていない: {made:?} {:?}",w.app.generated.note);
        assert_eq!(w.app.shell.command,"","作った後も欄に一言が残る");
        // 名前の一致は開く（作らない）・⌘↵ は作る
        let name=w.app.location();
        w.app.go(View::Home);
        let before=w.app.rows().len();
        w.submit("What to create",&name,false);
        assert_eq!(w.app.shell.view,made,"Home で名前「{name}」の一致の Enter が開いていない");
        assert_eq!(w.app.rows().len(),before,"一致の Enter で作った");
        // Project の欄: 通った変更で空・拒否で残る
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        let pages=|w:&Win|{let a=w.app.generated.app_of_project(&pid).unwrap();w.app.generated.project_of(&a).unwrap().unwrap().0.pages.len()};
        let n=pages(&w);
        // CHAT-ONE-1101: Project の Chat も会話 → 案の Accept で変える（拒否の理由も会話の 1 行）
        w.app.generated.panel_act(&pid,crate::ui::terminal_panel::PanelAct::Chat).unwrap();
        let change=|w:&mut Win|{w.submit("What to create","add a Notes page",false);let ctx=w.ctx.clone();w.app.apply(&ctx,Intent::PreviewPlan);w.app.apply(&ctx,Intent::AcceptPlan);for _ in 0..2{w.step(0.1,true,vec![]);}};
        change(&mut w);
        assert_eq!(pages(&w),n+1,"Project の Chat の案の Accept で変更が当たっていない");
        assert_eq!(w.app.shell.command,"","送った後も欄に文が残る");
        change(&mut w);
        assert_eq!(pages(&w),n+1,"拒否の変更で何か当てた");
        assert!(matches!(w.app.shell.view,View::Project{..}),"拒否で Project を離れた");
        // L2: Home の ⌘↵ は一致を開かず、作りもせず会話へ（人の行が 1 つ増える）
        w.app.go(View::Home);
        let before=w.app.rows().len();
        w.submit("What to create",&name,true);
        assert_eq!(w.app.shell.view,View::Home,"⌘↵ が一致を開いた・Home を離れた");
        assert_eq!(w.app.rows().len(),before,"Home の ⌘↵ で作った");
        assert!(w.app.shell.talk.iter().any(|t|t.text==name),"⌘↵ の一言が会話に無い: {:?}",w.app.shell.talk);
    }

    /// UI-4a 反証（Home の変更）: Home の欄の文が変更として読まれ、いちばん新しい Project に当たって移っても、その Project に残した打ちかけ（drafts）は消さない
    /// （送った文は Home の欄の物・header が送った時に Home の欄は空にした）。負の対照: `apply_home_edit` で合図を立てたままにする → 移った先の欄が空で赤。
    #[test] fn ui4a_home_edit_keeps_the_project_draft(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4a-home-edit");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let edit="notes.md を Mirror で見る Page を足して";
        assert!(matches!(crate::composer::compose(edit,&Default::default()),Err(crate::composer::ComposeError::EditRequest(_))),"前提: 「{edit}」は変更の文");
        assert!(!matches!(crate::composer::project::compose_project(edit,&Default::default(),None),Ok(Some(_))),"前提: 「{edit}」で bridge を起こさない");
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        let pages=|w:&Win|{let a=w.app.generated.app_of_project(&pid).unwrap();w.app.generated.project_of(&a).unwrap().unwrap().0.pages.len()};
        w.app.go(project);
        w.app.shell.command="rename Docs to Notes".into();
        w.app.go(View::Home);
        let n=pages(&w);
        // L2: Home の Enter・⌘↵ は会話なので、作る道（案の Build now・Retry と同じ `create_app`）を直に通す
        w.app.apply(&w.ctx.clone(),Intent::Generate(edit.into(),vec![]));
        w.step(0.1,true,vec![]);
        assert_eq!(pages(&w),n+1,"Home の変更がいちばん新しい Project に当たっていない: {:?}",w.app.generated.note);
        assert!(matches!(&w.app.shell.view,View::Project{project,..} if *project==pid),"当てた Project に移っていない: {:?}",w.app.shell.view);
        assert_eq!(w.app.shell.command,"rename Docs to Notes","Home の変更が、当てた Project の打ちかけを消した");
    }

    /// UI-4a 反証（作る依頼の戻し先）: AI が読む間・CLI の確かめを待つ間に来た作る依頼（⌘K の `Create project: “…”`）と、作れなかった一言は、
    /// Home の欄へ戻す（Project に居れば Home の打ちかけ = Home へ移った時に欄に出る）。今の場所の欄（Project の変更の打ちかけ）は書き換えない。
    /// 負の対照: busy の戻し先を `shell.command` に戻す → Project の欄が作る依頼の文になり赤。
    /// FX2（6 段・Home の生成の Stop）: ① Home の欄で一言を送る → ② 欄の下に段の 1 行（`Reading your request`）と `Stop`（AX の button）・欄の Create は押せない
    /// → ③ AX の Click で `Stop`（AXPress と同じ口）→ ④ model の呼び出しが job の印を見て止まる（子の回収の代わりに 300ms 待って Cancelled）・その間は
    /// `Stopping the AI…` で button 0 → ⑤ 読み戻し: Project の数が同じ・AI は 1 回・Runs の生成の行は Cancelled → ⑥ 同じ場所に `Stopped. Nothing was added to Projects.`
    /// と欄に一言が戻る。負の対照（commit message）: `build_step` の Cancel の腕を空にする → `Stopping the AI…` が出ず赤・`stop_model_job` を receiver を捨てるだけにしても同じ所で赤。
    /// FX2: job の印が立つまで答えない model（立ったら 300ms 後に Cancelled = 子を回収する間・20 秒で Timeout）。呼ばれた回数を数える。
    struct Waits(std::sync::Arc<std::sync::atomic::AtomicU32>);
    impl crate::llm::Llm for Waits{
        fn label(&self)->String{"waits".into()}
        fn send(&self,_r:&crate::llm::JsonRequest)->Result<serde_json::Value,crate::llm::LlmError>{Err(crate::llm::LlmError::Timeout{ms:0})}
        fn send_cancellable(&self,_r:&crate::llm::JsonRequest,cancel:&std::sync::atomic::AtomicBool)->Result<serde_json::Value,crate::llm::LlmError>{
            use std::sync::atomic::Ordering;
            self.0.fetch_add(1,Ordering::SeqCst);
            let t=Instant::now();
            while !cancel.load(Ordering::SeqCst){
                if t.elapsed()>Duration::from_secs(20){return Err(crate::llm::LlmError::Timeout{ms:20_000})}
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(300)); // 子を回収する間（Stopping の 1 行が見える）
            Err(crate::llm::LlmError::Cancelled{after_ms:300})
        }
    }
    /// AX の木の名前と値の全部。
    fn ax_words(out:&egui::FullOutput)->Vec<String>{
        let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        t.nodes.iter().flat_map(|(_,n)|[n.label(),n.value()]).flatten().map(str::to_owned).collect()
    }
    #[test] fn fx2_home_stop_ends_the_ai_call_and_keeps_the_words(){
        use std::sync::atomic::{AtomicU32,Ordering};
        use std::sync::Arc;
        let labels=ax_words;
        let mut w=Win::open("fx2-stop");
        w.app.llm_rx=None;
        let calls=Arc::new(AtomicU32::new(0));
        w.app.generated.set_llm(Ok(Box::new(Waits(calls.clone()))));
        let before=w.app.generated.rows().len();
        let said="Keep research notes about bees";
        w.app.apply(&w.ctx.clone(),Intent::Generate(said.into(),vec![])); // L2: Home の Enter・⌘↵ は会話なので作る道を直に
        let t0=Instant::now();
        let stop=loop{
            let out=w.step(0.05,true,vec![]);
            if let Some(b)=Win::button(&out,"Stop").filter(|_|calls.load(Ordering::SeqCst)>0){
                assert!(labels(&out).iter().any(|l|l.contains("Reading your request")),"段の 1 行が無い: {:?}",labels(&out));
                break b;
            }
            assert!(t0.elapsed()<Duration::from_secs(20),"Stop が出ない・model が呼ばれない: {:?}",labels(&out));
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(w.app.generated.model_busy_line().is_some(),"生成中に 2 つ目を受ける形");
        let out=w.step(0.05,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:stop,data:None})]);
        let out=if labels(&out).iter().any(|l|l.contains("Stopping the AI…")){out}else{w.step(0.05,true,vec![])};
        assert!(labels(&out).iter().any(|l|l.contains("Stopping the AI…")),"止めている間の 1 行が無い: {:?}",labels(&out));
        assert_eq!(Win::button(&out,"Stop"),None,"止めている間に押せる Stop が残った");
        let t1=Instant::now();
        let out=loop{
            let out=w.step(0.05,true,vec![]);
            if labels(&out).iter().any(|l|l.contains("Stopped. Nothing was added to Projects.")){break out}
            assert!(t1.elapsed()<Duration::from_secs(10),"Stop の後に止まった 1 行が出ない: {:?}",labels(&out));
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(w.app.generated.rows().len(),before,"止めたのに Project が出来た");
        assert_eq!(calls.load(Ordering::SeqCst),1,"止めた後に AI をもう一度呼んだ");
        assert_eq!(w.app.shell.command,said,"欄に一言が戻っていない");
        assert!(labels(&out).iter().any(|l|l==said),"欄（AX の値）に一言が無い");
        let run=w.app.generated.model_run_id().expect("終わった 1 行から Runs へ").to_owned();
        let row=w.app.generated.run_entries().into_iter().find(|e|e.id==run).expect("Runs に生成の行");
        assert_eq!(row.result(),crate::ui::runs::RunResult::Cancelled);
        assert_eq!(w.app.generated.model_busy_line(),None,"止まった後も欄が灰のまま");
    }

    /// RQ-023（WF-E の但し書き）: Automation を替えた直後の run の依頼が新しい値の門を通る（起動の時に 1 回だけ写すと、再起動まで古い門のまま）。
    /// ① 起動（既定 = Ask before write）で 1 frame → ② UiSettings を Suggest に（Settings の行が書く所）→ ③ 1 frame → ④ Page の依頼 → Engine が受けた値。
    /// 負の対照（手で取る）: update の写しを外す → ④ が Ask before write のまま赤。
    #[test] fn rq023_a_run_right_after_changing_automation_gets_the_new_gate(){
        use crate::domain::permission::AutomationMode;
        use crate::generated::{EngineAnswer,EngineReply,EngineRequest,EngineSetup,PageEngine};
        use crate::ui::shell::View;
        use std::sync::{Arc,Mutex};
        struct Seen(Arc<Mutex<Vec<AutomationMode>>>);
        impl PageEngine for Seen{
            fn run(&self,req:EngineRequest)->EngineAnswer{
                self.0.lock().unwrap().push(req.automation);
                EngineAnswer{reply:EngineReply::Failed{line:"seen".into()},log:("error",serde_json::json!({}))}
            }
        }
        let mut w=Win::open("rq023-now");
        let seen=Arc::new(Mutex::new(Vec::new()));
        w.app.generated.set_engine(EngineSetup{engine:Arc::new(Seen(seen.clone())),data_dir:w.dir.clone()});
        let Some(View::Project{project,..})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        w.step(0.05,true,vec![]);
        w.app.store.ui.automation=AutomationMode::Suggest;
        w.step(0.05,true,vec![]);
        assert!(w.app.generated.start_engine(&project,"Rename the note",vec![],None),"依頼が始まらない");
        let t0=Instant::now();
        while seen.lock().unwrap().is_empty(){
            assert!(t0.elapsed()<Duration::from_secs(10),"Engine が依頼を受けない");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(seen.lock().unwrap().as_slice(),&[AutomationMode::Suggest],"替えた直後の run が古い Automation の門を通った");
    }

    /// ENGINE-GENERIC-1101 追記 234（owner「engineが把握することとchatが把握することは全て共有して互換性をもたせて」）: 同じ Project で、chat の AI に渡す
    /// 状況の塊と Engine に渡す状況の塊が、その時の `Generated::situation` に窓の 1 行（`with_windows`）を足した物の同じ字（`Situation::text`）。
    /// 負の対照（手で取る）: `talk_in` の thread で状況に事実を 1 つ足す → chat の assert が赤・`start_engine_with` の thread で `with_windows` を外す →
    /// Engine の assert が赤。
    #[test] fn now234_a_chat_and_the_engine_get_the_same_situation_block(){
        use crate::domain::chat::{Situation,NOW_HEAD};
        use crate::generated::{EngineAnswer,EngineReply,EngineRequest,EngineSetup,PageEngine};
        use crate::ui::shell::View;
        use std::sync::{Arc,Mutex};
        const WINDOWS:&str="Windows open now: Mixer (com.example.mixer) × 1";
        struct Heard(Arc<Mutex<Vec<Situation>>>);
        impl PageEngine for Heard{
            fn run(&self,req:EngineRequest)->EngineAnswer{
                self.0.lock().unwrap().push(req.situation);
                EngineAnswer{reply:EngineReply::Failed{line:"stopped".into()},log:("error",serde_json::json!({}))}
            }
            fn windows(&self)->Option<String>{Some(WINDOWS.into())}
        }
        struct Said(Arc<Mutex<Vec<String>>>);
        impl crate::llm::Llm for Said{
            fn label(&self)->String{"said".into()}
            fn send(&self,r:&crate::llm::JsonRequest)->Result<serde_json::Value,crate::llm::LlmError>{
                self.0.lock().unwrap().push(r.input.clone());
                Ok(serde_json::json!({"ask":"The page shows no error now.","choices":[],"proposal":null,"needs":null}))
            }
        }
        let mut w=Win::open("now234");
        w.app.llm_rx=None;
        let Some(View::Project{project,..})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
        // 作った後に偽の AI と Engine を（作る読みに AI を使わせない = said の 1 通目が chat の一言）
        let (heard,said)=(Arc::new(Mutex::new(Vec::new())),Arc::new(Mutex::new(Vec::new())));
        w.app.generated.set_llm(Ok(Box::new(Said(said.clone()))));
        w.app.generated.set_engine(EngineSetup{engine:Arc::new(Heard(heard.clone())),data_dir:w.dir.clone()});
        let now=|w:&mut Win|{let mut s=w.app.generated.situation(&project);s.facts.push(WINDOWS.into());s.text().expect("状況が空")};
        // chat: 一言を送る前のその Project の状況（窓の 1 行は chat の thread が足す）
        let want=now(&mut w);
        assert!(want.starts_with(&format!("{NOW_HEAD}:\n- Page: ")),"{want}");
        w.app.talk_in(project.clone(),"What is this error?".into());
        let t0=Instant::now();
        while said.lock().unwrap().is_empty(){
            assert!(t0.elapsed()<Duration::from_secs(10),"chat の AI が呼ばれない");
            std::thread::sleep(Duration::from_millis(10));
        }
        let input=said.lock().unwrap()[0].clone();
        let at=input.find(&want).unwrap_or_else(||panic!("chat の AI に状況の塊が同じ字で届かない:\n{input}\n--- want ---\n{want}"));
        assert!(!input[at+want.len()..].starts_with("\n- "),"chat にだけ事実を足した: {input}");
        // Engine: 頼む前のその Project の状況（同じ fn・同じ順・同じ窓の 1 行）
        let want=now(&mut w);
        assert_eq!(w.app.generated.ask_engine(&project,"Open the guitar list",Situation::default()),Ok(()));
        while heard.lock().unwrap().is_empty(){
            assert!(t0.elapsed()<Duration::from_secs(20),"Engine が依頼を受けない");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(heard.lock().unwrap()[0].text(),Some(want),"Engine に渡す塊が chat と同じ字でない");
    }

    /// FX2 反証の直し（Runs の生成の行の Stop）: ① Runs でその run の行を開く → ② 詳細の `Stop`（AX の Click）→ ③ `stop_from_runs` が job の印を立てる →
    /// ④ 子の回収の間は詳細の下に `Stopping the AI…` → ⑤ 行が Cancelled になる → ⑥ 進行形の note は残らず、終わった文 `Stopped. Nothing was added to Projects.` に替わる。
    /// 負の対照（commit message）: `poll_model` の `ModelDone::Stopped` の腕で note を替えない → 行が Cancelled の後も `Stopping the AI…` が残って赤。
    #[test] fn fx2_runs_stop_note_ends_with_the_run(){
        use crate::ui::shell::View;
        use std::sync::atomic::{AtomicU32,Ordering};
        use std::sync::Arc;
        let mut w=Win::open("fx2-runs-stop");
        w.app.llm_rx=None;
        let calls=Arc::new(AtomicU32::new(0));
        w.app.generated.set_llm(Ok(Box::new(Waits(calls.clone()))));
        w.app.apply(&w.ctx.clone(),Intent::Generate("Keep research notes about bees".into(),vec![])); // L2: Home の Enter・⌘↵ は会話なので作る道を直に
        let t0=Instant::now();
        while calls.load(Ordering::SeqCst)==0{
            assert!(t0.elapsed()<Duration::from_secs(20),"model が呼ばれない");
            w.step(0.05,true,vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let run=w.app.generated.model_run_id().expect("生成中の run").to_owned();
        w.app.go(View::Runs);
        crate::ui::runs::open_run(&w.ctx,&run);
        w.step(0.1,true,vec![]);
        let out=w.step(0.1,true,vec![]);
        let stop=Win::button(&out,"Stop").expect("Runs の生成の行の詳細に Stop");
        w.step(0.05,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:stop,data:None})]);
        let out=w.step(0.05,true,vec![]);
        assert!(ax_words(&out).iter().any(|l|l=="Stopping the AI…"),"止めている間の note が無い: {:?}",ax_words(&out));
        let t1=Instant::now();
        while w.app.generated.model_busy_line().is_some(){
            assert!(t1.elapsed()<Duration::from_secs(10),"Stop の後に止まらない");
            w.step(0.05,true,vec![]);
            std::thread::sleep(Duration::from_millis(20));
        }
        let row=w.app.generated.run_entries().into_iter().find(|e|e.id==run).expect("Runs に生成の行");
        assert_eq!(row.result(),crate::ui::runs::RunResult::Cancelled);
        w.app.go(View::Runs);
        w.step(0.1,true,vec![]);
        let out=w.step(0.1,true,vec![]);
        let words=ax_words(&out);
        assert!(!words.iter().any(|l|l=="Stopping the AI…"),"行が Cancelled の後も進行形の note が残った: {words:?}");
        assert!(words.iter().any(|l|l=="Stopped. Nothing was added to Projects."),"止め終わった文が無い: {words:?}");
    }

    #[test] fn ui4a_create_text_goes_back_to_home_not_the_project_field(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4a-busy");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        w.app.shell.command="add a chart".into();
        w.app.waiting=Some(("Keep research notes".into(),Vec::new(),Instant::now()));
        w.app.create_app("Track my inventory".into(),Vec::new());
        assert_eq!(w.app.shell.command,"add a chart","busy の作る依頼が Project の変更の打ちかけを書き換えた");
        assert_eq!(w.app.shell.drafts.get("home").map(String::as_str),Some("Track my inventory"),"作る依頼の文が Home の欄に戻っていない");
        w.app.waiting=None;
        // 作れなかった一言: Home へ移って欄に出る・Project の打ちかけは Project に残る
        w.app.open_made("Make a thing".into(),Err("Couldn't read the purpose".into()));
        assert_eq!(w.app.shell.view,View::Home);
        assert_eq!(w.app.shell.command,"Make a thing","作れなかった一言が Home の欄に戻っていない");
        assert_eq!(w.app.shell.drafts.get(&pid).map(String::as_str),Some("add a chart"),"Project の打ちかけが消えた");
        // 今が Home の場所なら欄へ（busy）
        w.app.shell.command.clear();
        w.app.waiting=Some(("Keep research notes".into(),Vec::new(),Instant::now()));
        w.app.create_app("Plan a trip".into(),Vec::new());
        assert_eq!(w.app.shell.command,"Plan a trip");
    }

    /// HOME-ATTACH-1101（owner 175・180 通目）: 本物の ShibubuApp の Home の欄（AX の Click で focus）に ⌘V（egui の `Event::Paste`）。
    /// 300 行 → 欄は空のまま `Pasted text · 300 lines` の長方形 1 つ・299 行 → 欄に入り長方形は増えない・clipboard に Finder の file が在る ⌘V
    /// （`paste_files`・文は file の名前だけ）→ その file の長方形で欄は空・窓に写真と file を落とす → 長方形 2 つ（AX `Photo …`・`File …`）・× で 1 つ外れる・
    /// 送る前の Cancel（CLI を確かめている間の待ち）で一言が欄に戻る時、長方形（貼った文も）も戻る（G104）。
    /// owner 185 通目: 偽の AI は file も写真も読めない（`takes_files` = false）= それでも長方形は置け、Send の時に今の AI の名前で 1 行・送らない。
    /// 負の対照: chat::paste の `>= PASTE_LINES` を `> PASTE_LINES` → 300 行が欄に入って赤・poll_attach の paste_files を見ない → file 名が欄に入って赤・
    /// build_step の Cancel の `self.shell.attach=attach` を外す → 長方形が戻らず赤・talk の recheck を外す → 送って赤。
    #[test] fn home_attach_long_paste_finder_files_drops_and_cancel(){
        struct NoFiles;
        impl crate::llm::Llm for NoFiles{
            fn label(&self)->String{"no-files".into()}
            fn send(&self,_:&crate::llm::JsonRequest)->Result<serde_json::Value,crate::llm::LlmError>{Err(crate::llm::LlmError::Timeout{ms:1})}
        }
        let mut w=Win::open("home-attach");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Ok(Box::new(NoFiles)));
        let lines=|n:usize|(1..=n).map(|i|format!("line {i}\n")).collect::<String>();
        let paste=|w:&mut Win,text:String|{w.step(0.1,true,vec![Event::Paste(text)]);w.step(0.1,true,vec![]);};
        w.focus_home_field();
        paste(&mut w,lines(300));
        assert_eq!(w.app.shell.command,"","300 行が欄に入った");
        assert_eq!(w.app.shell.attach.iter().map(|a|a.name()).collect::<Vec<_>>(),["Pasted text · 300 lines"]);
        paste(&mut w,lines(299));
        assert!(w.app.shell.command.contains("line 1")&&w.app.shell.command.contains("line 299"),"299 行が欄に入らない: {:?}",w.app.shell.command);
        assert_eq!(w.app.shell.attach.len(),1,"299 行で長方形が増えた");

        let fixture=std::env::temp_dir().join("shibubu-home-attach-finder");
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(fixture.join("notes.md"),"meeting notes\n").unwrap();
        w.app.paste_files=||vec![std::env::temp_dir().join("shibubu-home-attach-finder").join("notes.md")];
        w.app.shell.command.clear();
        paste(&mut w,"notes.md".into());
        assert_eq!(w.app.shell.command,"","Finder の file の名前が欄に入った");
        assert_eq!(w.app.shell.attach.iter().map(|a|a.name()).collect::<Vec<_>>(),["Pasted text · 300 lines","notes.md"]);
        w.app.paste_files=||Vec::new();

        let shot=fixture.join("shot.png");
        image::RgbaImage::from_pixel(8,8,image::Rgba([200,80,40,255])).save(&shot).unwrap();
        let report=fixture.join("report.md");
        std::fs::write(&report,"q3 report\n").unwrap();
        w.app.shell.attach.clear();
        w.drop_files(&[shot.clone(),report.clone()]);
        let out=w.step(0.1,true,vec![]);
        assert_eq!(w.app.shell.attach.iter().map(|a|a.name()).collect::<Vec<_>>(),["shot.png","report.md"]);
        let names=ax_words(&out);
        for want in ["Photo shot.png","File report.md","Remove shot.png","Remove report.md"]{assert!(names.iter().any(|n|n==want),"{want}: {names:?}");}
        let x=Win::button(&out,"Remove report.md").expect("× が AX に無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:x,data:None})]);
        assert_eq!(w.app.shell.attach.iter().map(|a|a.name()).collect::<Vec<_>>(),["shot.png"],"× で外れない");
        w.app.talk("Plan a trip".into());
        assert!(w.app.shell.talk.is_empty()&&w.app.shell.command=="Plan a trip","読めない AI に送った: {:?}",w.app.shell.talk);
        assert_eq!(w.app.shell.attach_line.as_deref(),Some("Claude Code can't read pictures · remove shot.png or switch the AI in Provider"));

        let held=vec![w.app.shell.attach[0].clone(),crate::domain::chat::Attachment::Pasted{text:lines(300),lines:300}];
        w.app.shell.attach.clear();
        w.app.shell.command.clear();
        w.app.waiting=Some(("Plan a trip".into(),held.clone(),Instant::now()));
        let ctx=w.ctx.clone();
        w.app.build_step(&ctx,crate::ui::build_progress::BuildAction::Cancel);
        assert_eq!((w.app.shell.command.as_str(),&w.app.shell.attach),("Plan a trip",&held),"Cancel で長方形が戻らない");
        let _=std::fs::remove_dir_all(&fixture);
    }

    /// UI-4a Done 5（反証の直し: header だけでなく本物の ShibubuApp の keyboard と palette を通す）: Project の Chat の欄（`What to create`・CHAT-ONE-1101）に
    /// 1 文字ずつ（修飾なしの Key の押下・Text・離す・`k` を含む）打つ間、毎 frame palette は閉じたまま・欄は文字を取る。
    /// 負の対照（commit message）: keyboard() の ⌘K を `i.key_pressed(egui::Key::K)`（修飾を見ない）にする → `K` を打った frame で赤。
    #[test] fn ui4a_palette_stays_closed_while_typing(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4a-typing");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        w.app.generated.panel_act(&pid,crate::ui::terminal_panel::PanelAct::Chat).unwrap();
        let field=Win::field(&w.step(0.1,true,vec![]),"What to create");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:field,data:None})]);
        let text="Keep research notes k";
        for (i,c) in text.chars().enumerate(){
            let s=c.to_string();
            let key=egui::Key::from_name(&s);
            let press=|pressed|key.map(|key|Event::Key{key,physical_key:None,pressed,repeat:false,modifiers:egui::Modifiers::NONE});
            w.step(0.05,true,press(true).into_iter().chain([Event::Text(s.clone())]).chain(press(false)).collect());
            assert!(!w.app.shell.palette.open,"{i} 文字目（{c:?}）を打った frame で palette が開いた");
        }
        assert_eq!(w.app.shell.command,text,"欄が文字を取っていない（試験が成り立たない）");
    }

    /// UI-4 Done 9（UI-4b・幅 1200）: ⌃⌘S・title line の button・⌘K の `Hide sidebar` のどれでも sidebar を畳むと作業の場所の左端が窓の左まで広がり
    /// （骨格の帯も 0）、`Show sidebar` で `tier.sidebar`（216）に戻る。欄の rect は畳む前と後で同じ。畳んだ状態は `UiSettings.sidebar_hidden` に書かれる（開き直しは下の試験）。
    /// G83: 測る場所は Runs（Home の欄は主領域の列の中央なので sidebar の開閉で x が動く・owner 10-27・`g83_…` が列の中央を見る）。
    /// UI-SWEEP-1101: Runs の header に常の欄は無い（⌘K の間だけ）= 畳む前・後・⌘K を閉じた後のどれでも欄が 0 である事を見る（前は欄の rect が動かない事）。
    /// 負の対照（commit message）: ⌃⌘S の consume_key を消す → ① で赤・app.rs で sidebar を header より先に描く → 欄の x が動いて赤。
    #[test] fn ui4b_sidebar_folds_three_ways_and_the_field_stays(){
        use crate::ui::shell::Tier;
        let mut w=Win::open("ui4b-fold");
        w.size=Vec2::new(1200.0,800.0);
        let full=Tier::for_width(1200.0).sidebar;
        w.app.go(crate::ui::shell::View::Runs);
        w.settle();
        let (left,sk,out)=w.probe();
        assert!(w.app.shell.sidebar_open&&w.app.store.ui.sidebar_hidden.is_none(),"幅 1200 の初めての起動は開いて始まる");
        assert_eq!((left,sk.sidebar.width()),(full,full),"開いた sidebar の幅");
        assert!(Win::button(&out,"Connections").is_some()&&Win::button(&out,"Hide sidebar").is_some(),"sidebar の行・title line の button が無い");
        let no_field=|out:&egui::FullOutput,what:&str|assert!(Win::word_fields(out).is_empty(),"{what}: Runs の header に常の欄が出た（UI-SWEEP-1101）");
        no_field(&out,"開いた sidebar");
        let folded=|w:&mut Win,what:&str|{
            w.settle();
            let (left,sk,out)=w.probe();
            assert_eq!(w.app.store.ui.sidebar_hidden,Some(true),"{what}: 畳んだ選択を覚えていない");
            assert_eq!((left,sk.sidebar.width()),(0.0,0.0),"{what}: 畳んでも作業の場所が窓の左まで広がらない（骨格の帯も 0 のはず）");
            assert!(Win::button(&out,"Connections").is_none(),"{what}: 畳んだのに sidebar の行が残る");
            assert!(Win::button(&out,"Show sidebar").is_some(),"{what}: 畳んだ間の title line の button の名前が Show sidebar でない");
            no_field(&out,what);
        };
        let opened=|w:&mut Win,what:&str|{
            w.settle();
            let (left,sk,out)=w.probe();
            assert_eq!(w.app.store.ui.sidebar_hidden,Some(false),"{what}: 戻した選択を覚えていない");
            assert_eq!((left,sk.sidebar.width()),(full,full),"{what}: tier.sidebar の幅に戻らない");
            no_field(&out,what);
        };
        // ① ⌃⌘S
        w.key(egui::Key::S,CTRL_CMD);
        folded(&mut w,"⌃⌘S");
        // ② title line の button（AX の Click = 試験の道具の AXPress と同じ口）
        let (_,_,out)=w.probe();
        let b=Win::button(&out,"Show sidebar").expect("Show sidebar の button が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:b,data:None})]);
        opened(&mut w,"title line の button");
        // ③ ⌘K の行（Hide sidebar）→ ④ Show sidebar
        for (q,what) in [("hide sidebar","⌘K の Hide sidebar"),("show sidebar","⌘K の Show sidebar")]{
            w.key(egui::Key::K,CMD);
            assert!(w.app.shell.palette.open,"{what}: ⌘K で palette が開かない");
            w.app.shell.command=q.into();
            w.key(egui::Key::Enter,egui::Modifiers::NONE);
            assert!(!w.app.shell.palette.open,"{what}: Enter で palette が閉じない");
            if q.starts_with("hide"){folded(&mut w,what);}else{opened(&mut w,what);}
        }
    }

    /// UI-4 Done 9（UI-4b・開き直し）: title line の button で畳んで ⌘Q（on_exit）→ 同じ data dir で開き直すと畳んだまま（`sessions.json` の `"ui"."sidebar_hidden"`）。
    /// 負の対照（commit message）: `UiSettings.sidebar_hidden` に `#[serde(skip)]` → 開き直しで開いて赤。
    #[test] fn ui4b_folded_state_survives_reopen(){
        let mut w=Win::open("ui4b-reopen");
        w.size=Vec2::new(1440.0,900.0);
        w.settle();
        let (_,_,out)=w.probe();
        let b=Win::button(&out,"Hide sidebar").expect("Hide sidebar の button が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:b,data:None})]);
        w.settle();
        assert!(!w.app.shell.sidebar_open,"前提: title line の button で畳めていない");
        w.reopen();
        w.settle();
        let (left,sk,_)=w.probe();
        assert!(!w.app.shell.sidebar_open&&left==0.0&&sk.sidebar.width()==0.0,"開き直したら畳んだ状態を忘れた（左端 {left}・覚えた値 {:?}）",w.app.store.ui.sidebar_hidden);
    }

    /// UI-4 Done 9（UI-4b・幅 900）: 初めての起動は畳んで始まり（覚えた値は書かない）、畳んだまま ⌘K から各 Project と Home へ行ける（h_reach_when_folded の試験の側）。
    /// 狭い窓で人が戻した選択は、開き直しても狭い窓の既定に上書きされない。負の対照（commit message）: Tier の 1200 未満の `sidebar_hidden` を false → 畳んで始まらず赤。
    #[test] fn ui4b_narrow_window_starts_folded_and_keeps_the_choice(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4b-narrow");
        w.size=Vec2::new(900.0,700.0);
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない").expect("主デモの一言は Project");
        w.settle();
        let (left,_,_)=w.probe();
        assert!(!w.app.shell.sidebar_open&&left==0.0,"幅 900 の初めての起動が畳んで始まらない（左端 {left}）");
        assert_eq!(w.app.store.ui.sidebar_hidden,None,"幅の既定で畳んだだけで覚えた値を書いた");
        // 畳んだまま ⌘K → Project の名前 → Enter で開く・Go to Home で戻る
        let View::Project{project:pid,..}=project else{panic!()};
        let app_id=w.app.generated.app_of_project(&pid).expect("Project の app が無い");
        let name=w.app.rows().into_iter().find(|r|r.view==View::App(app_id.clone())).map(|r|r.name).expect("Project の行が無い");
        for (q,home) in [(name.as_str(),false),("go to home",true)]{
            w.key(egui::Key::K,CMD);
            w.app.shell.command=q.into();
            w.key(egui::Key::Enter,egui::Modifiers::NONE);
            w.settle();
            let there=if home{w.app.shell.view==View::Home}else{matches!(&w.app.shell.view,View::Project{project,..} if *project==pid)};
            assert!(there,"畳んだまま ⌘K の「{q}」で行けない: {:?}",w.app.shell.view);
            assert!(!w.app.shell.sidebar_open,"⌘K で移ったら sidebar が開いた");
        }
        // 狭い窓で戻す → 開き直しても開いたまま
        w.key(egui::Key::S,CTRL_CMD);
        w.settle();
        assert_eq!(w.app.store.ui.sidebar_hidden,Some(false));
        w.reopen();
        w.settle();
        let (left,_,_)=w.probe();
        assert!(w.app.shell.sidebar_open&&left>0.0,"狭い窓の既定が、人が戻した選択を開き直しで上書きした（左端 {left}）");
    }

    /// UI-4 Done 6（UI-4b）: Home と Project A に別々に打ちかけ、終わって開き直すと両方とも戻る（Home は欄に・A は A へ移った時に欄に）。もう無い Project の打ちかけは捨てる。
    /// 負の対照（commit message）: `open` の読み手（drafts を Shell へ戻す 4 行）を消す → 開き直しで Home の欄が空で赤・`on_exit` の `snapshot_drafts` を消す → 同じく赤。
    #[test] fn ui4b_drafts_come_back_after_reopen(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4b-drafts");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.settle();
        w.type_in("What to create","Track my inventory");
        w.app.go(project.clone());
        w.app.generated.panel_act(&pid,crate::ui::terminal_panel::PanelAct::Chat).unwrap();
        w.settle();
        w.type_in("What to create","add a chart");
        assert_eq!(w.app.shell.command,"add a chart","前提: Project の欄に打てていない");
        w.app.shell.drafts.insert("p.gone".into(),"old text".into());
        w.reopen();
        w.settle();
        assert_eq!(w.app.shell.view,View::Home,"開き直した最初の場所");
        assert_eq!(w.app.shell.command,"Track my inventory","Home の打ちかけが開き直しで戻らない");
        assert!(!w.app.shell.drafts.contains_key("p.gone"),"もう無い Project の打ちかけを残した: {:?}",w.app.shell.drafts);
        w.app.go(project);
        w.settle();
        assert_eq!(w.app.shell.command,"add a chart","Project A の打ちかけが開き直しで戻らない（drafts {:?}）",w.app.shell.drafts);
        assert_eq!(w.app.shell.drafts.get("home").map(String::as_str),Some("Track my inventory"),"A へ移ったら Home の打ちかけが消えた");
        let _=pid;
    }

    /// FX1（FX-001）: 書けない data dir で窓の閉じる要求 → 閉じない（`CancelClose`）・同じ場所に `Quit anyway` と `Keep open`・`Keep open` で 1 行が消え
    /// View と欄の文は残る・もう 1 回の閉じる要求の `Quit anyway` で `Close`。本体の前は `Intent::Quit` の arm の todo!() で赤（interface の commit）。
    /// 負の対照（本体の段）: `save_now` の Err を捨てる（今の on_exit の `let _=`）→ CancelClose が無く赤・`Keep open` で欄を空にする → 欄の assert が赤。
    #[test] fn fx1_close_with_unsaveable_dir_asks_and_keep_open_keeps_input(){
        use std::os::unix::fs::PermissionsExt;
        struct Writable(std::path::PathBuf); // 赤で抜けても temp の dir を書ける形に戻す
        impl Drop for Writable{fn drop(&mut self){let _=std::fs::set_permissions(&self.0,std::fs::Permissions::from_mode(0o700));}}
        let mut w=Win::open("fx1-quit");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        w.settle();
        w.type_in("What to create","keep this text");
        let _back=Writable(w.dir.clone());
        std::fs::set_permissions(&w.dir,std::fs::Permissions::from_mode(0o500)).unwrap(); // sessions.json.tmp を作れない = Store::save が Err
        let close=|w:&mut Win|{
            w.t+=0.1;
            let mut input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,w.size)),time:Some(w.t),focused:true,..Default::default()};
            input.viewports.insert(ViewportId::ROOT,egui::ViewportInfo{events:vec![egui::ViewportEvent::Close],..Default::default()});
            let (app,frame)=(&mut w.app,&mut w.frame);
            w.ctx.run(input,|ctx|eframe::App::update(app,ctx,frame))
        };
        let cmds=|o:&egui::FullOutput|o.viewport_output.get(&ViewportId::ROOT).map(|v|v.commands.clone()).unwrap_or_default();
        let out=close(&mut w);
        assert!(cmds(&out).contains(&egui::ViewportCommand::CancelClose),"保存できないのに閉じた: {:?}",cmds(&out));
        let out=w.step(0.1,true,vec![]);
        assert!(Win::button(&out,"Quit anyway").is_some(),"同じ場所に Quit anyway が無い");
        let keep=Win::button(&out,"Keep open").expect("同じ場所に Keep open が無い");
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:keep,data:None})]);
        let out=w.step(0.1,true,vec![]);
        assert!(Win::button(&out,"Keep open").is_none(),"Keep open の後も 1 行が残った");
        assert_eq!(w.app.shell.view,crate::ui::shell::View::Home,"Keep open で場所が変わった");
        assert_eq!(w.app.shell.command,"keep this text","Keep open で欄の文を失った");
        close(&mut w);
        let out=w.step(0.1,true,vec![]);
        let quit=Win::button(&out,"Quit anyway").expect("2 回目の閉じる要求で 1 行が出ない");
        let a=w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:quit,data:None})]);
        let b=w.step(0.1,true,vec![]);
        assert!(cmds(&a).contains(&egui::ViewportCommand::Close)||cmds(&b).contains(&egui::ViewportCommand::Close),"Quit anyway で閉じない");
    }

    /// FX1（FX-003）: 壊れた sessions.json で開くと、隔離した先が `Store::set_aside` に入り、作業の場所の頭に `Reveal in Finder`。
    /// RQ3 から隔離先は data dir の `backups/<UTC>-damaged/sessions.json`。英語の 1 行は `message` に残さず Home の `generated.note`（その下に `Restore…`）。
    /// 本体の前は `set_aside` が None のままで赤。負の対照: `quarantine` が set_aside を入れない → 1 つ目の expect が赤。`load_note` を note へ移す match を外す → 英語の 1 行の expect が赤。
    #[test] fn fx1_damaged_store_is_set_aside_with_reveal(){
        let dir=std::env::temp_dir().join(format!("shibubu-fx1-aside-{}",crate::storage::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sessions.json"),"{not json").unwrap();
        let ctx=egui::Context::default();
        ctx.enable_accesskit();
        let app=super::ShibubuApp::open(&eframe::CreationContext::_new_kittest(ctx.clone()),dir.clone());
        let mut w=Win{ctx,app,frame:eframe::Frame::_new_kittest(),t:0.0,delay:Duration::MAX,dir:dir.clone(),size:Vec2::new(1440.0,900.0)};
        let aside=w.app.store.set_aside.clone().expect("隔離した先が Store::set_aside に無い");
        assert!(aside.exists()&&aside.starts_with(dir.join("backups"))&&aside.file_name().is_some_and(|n| n=="sessions.json"),"隔離した file が data dir の backups に無い: {aside:?}");
        w.settle();
        let out=w.step(0.1,true,vec![]);
        assert!(Win::button(&out,"Reveal in Finder").is_some(),"開き直した画面に Reveal in Finder が無い");
        let note="Your settings were damaged and set aside · Settings › Data & Recovery";
        assert_eq!(w.app.generated.note.as_deref(),Some(note),"通知が無いか英語でない: {:?}",w.app.generated.note);
        assert!(w.app.message.is_empty(),"隔離の 1 行を状態の行にも出した: {}",w.app.message);
        let nodes=&out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes;
        let has=|s:&str|nodes.iter().any(|(_,n)|n.label()==Some(s)||n.value()==Some(s));
        assert!(has(note),"Home に隔離の 1 行が無い");
        assert!(has("Damaged layout set aside as sessions.json"),"頭の隔離の 1 行が無い");
    }

    /// FX1（FX-001）: kangae.sqlite3 を開けず memory の store で動いている（`Generated::store_lost`）時も、sessions.json が書けても閉じる要求で止まって聞く
    /// （閉じると、この起動で作った Project が消える）。理由の 1 行は作業の場所の頭・英語。
    /// 負の対照（commit message）: `save_now` が `store_lost` を見ない → CancelClose が無く赤。
    #[test] fn fx1_memory_store_is_not_lost_quietly(){
        let dir=std::env::temp_dir().join(format!("shibubu-fx1-lost-{}",crate::storage::new_id()));
        std::fs::create_dir_all(dir.join("kangae.sqlite3")).unwrap(); // DB の file の名前の dir = SQLite が開けない
        let ctx=egui::Context::default();
        ctx.enable_accesskit();
        let app=super::ShibubuApp::open(&eframe::CreationContext::_new_kittest(ctx.clone()),dir.clone());
        let mut w=Win{ctx,app,frame:eframe::Frame::_new_kittest(),t:0.0,delay:Duration::MAX,dir:dir.clone(),size:Vec2::new(1440.0,900.0)};
        assert!(w.app.generated.store_lost().is_some(),"DB を開けないのに store_lost が None");
        w.settle();
        w.t+=0.1;
        let mut input=RawInput{screen_rect:Some(Rect::from_min_size(Pos2::ZERO,w.size)),time:Some(w.t),focused:true,..Default::default()};
        input.viewports.insert(ViewportId::ROOT,egui::ViewportInfo{events:vec![egui::ViewportEvent::Close],..Default::default()});
        let (app,frame)=(&mut w.app,&mut w.frame);
        let out=w.ctx.run(input,|ctx|eframe::App::update(app,ctx,frame));
        let cmds=out.viewport_output.get(&ViewportId::ROOT).map(|v|v.commands.clone()).unwrap_or_default();
        assert!(cmds.contains(&egui::ViewportCommand::CancelClose),"memory の store のまま黙って閉じた: {cmds:?}");
        assert!(w.dir.join("sessions.json").exists(),"sessions.json は書けるのに書いていない");
        let out=w.step(0.1,true,vec![]);
        let ja=|c:char|('\u{3040}'..='\u{30ff}').contains(&c)||('\u{4e00}'..='\u{9fff}').contains(&c);
        let line=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い").nodes.iter()
            .filter_map(|(_,n)|n.value().map(str::to_owned)).find(|l|l.starts_with("Couldn't save your changes · ")); // egui の Label の文は AX の value
        assert!(line.as_deref().is_some_and(|l|l.contains("aren't saved")&&!l.chars().any(ja)),"理由の 1 行が無いか英語でない: {line:?}");
        assert!(Win::button(&out,"Keep open").is_some()&&Win::button(&out,"Quit anyway").is_some(),"Keep open / Quit anyway が無い");
        let _=std::fs::remove_dir_all(&dir);
    }

    /// FX1 反証の直し（FX-002 の残り）: FX1 の前の既定（cwd の `.shibubu-data`）に在る Project・Mirror の同意（ui_state `mirror-consent`）・
    /// sessions.json が、写した後の新しい data dir で読める・旧 dir は残る・新 dir に保存が在れば混ぜない・写せなければ旧 dir を返し新 dir に印を残さない。
    /// 負の対照: `adopt_old_data` が写さずに新 dir を返す（2df6b10 の形）→ Project の assert が赤。
    #[test] fn fx1_old_data_dir_is_copied_then_read(){
        use std::os::unix::fs::PermissionsExt;
        let root=std::env::temp_dir().join(format!("shibubu-fx1-move-{}",crate::storage::new_id()));
        let (old,new)=(root.join("cwd/.shibubu-data"),root.join("home/Library/Application Support/Shibubu"));
        let mut g=crate::generated::Generated::open(&old);
        g.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let crate::ui::shell::View::Project{project:pid,..}=g.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap() else{panic!()};
        let app_id=g.app_of_project(&pid).expect("Project の app が無い");
        drop(g);
        {
            let db=crate::storage::Store::open(&old.join("kangae.sqlite3")).unwrap();
            db.put(crate::storage::Table::Sessions,"mirror-consent",Some(&app_id),&serde_json::json!({"app_id":app_id})).unwrap();
            db.put_ui_state("mirror-consent","com.apple.TextEdit",&serde_json::json!({"bundle_id":"com.apple.TextEdit","granted_at_ms":1.0})).unwrap();
        }
        crate::model::Store::default().save(&old.join("sessions.json")).unwrap();
        let olds=[old.clone(),root.join("cwd/.kangae-data")];
        assert_eq!(crate::surface::adopt_old_data(&new,&olds),new,"写せたのに新 dir を返さない");
        assert!(crate::generated::Generated::open(&new).app_of_project(&pid).is_some(),"旧 dir の Project が新 dir で読めない");
        let db=crate::storage::Store::open_read_only(&new.join("kangae.sqlite3")).unwrap();
        assert!(db.get_ui_state("mirror-consent","com.apple.TextEdit").unwrap().is_some(),"旧 dir の Mirror の同意が新 dir で読めない");
        assert!(new.join("sessions.json").is_file()&&old.join("kangae.sqlite3").is_file()&&old.join("sessions.json").is_file(),"sessions.json を写していないか旧 dir を消した");
        // 新 dir に保存が在れば混ぜない（2 回目は写さない = 新 dir の sessions.json はそのまま）
        std::fs::write(new.join("sessions.json"),"{\"kept\":1}").unwrap();
        assert_eq!(crate::surface::adopt_old_data(&new,&olds),new);
        assert_eq!(std::fs::read_to_string(new.join("sessions.json")).unwrap(),"{\"kept\":1}","新 dir に保存が在るのに旧 dir で上書きした");
        // 写せない（新 dir の親が書けない）→ 旧 dir を返し、新 dir に印を残さない
        let locked=root.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked,std::fs::Permissions::from_mode(0o500)).unwrap();
        let blocked=locked.join("Shibubu");
        let got=crate::surface::adopt_old_data(&blocked,&olds);
        std::fs::set_permissions(&locked,std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(got,old,"写せないのに旧 dir を返さない");
        assert!(!blocked.join("kangae.sqlite3").exists()&&!blocked.join("sessions.json").exists(),"写せなかった新 dir に印が残った");
        // 旧 dir に保存が無ければ新 dir のまま（何も写さない）
        let empty=root.join("empty-home/Shibubu");
        assert_eq!(crate::surface::adopt_old_data(&empty,&[root.join("nowhere")]),empty);
        assert!(!empty.exists(),"写す物が無いのに新 dir を作った");
        let _=std::fs::remove_dir_all(&root);
    }

    /// FX1 反証の直し: 新しい版の Shibubu が保存した DB（`StoreError::NewerSchema`）でも、閉じる時の理由（`store_lost`）と Home の 1 行（`note`）は英語。
    /// 負の対照: NewerSchema の Display を日本語に戻す（2df6b10 の形）→ 赤。
    #[test] fn fx1_newer_store_reason_is_english(){
        let dir=std::env::temp_dir().join(format!("shibubu-fx1-newer-{}",crate::storage::new_id()));
        let path=dir.join("kangae.sqlite3");
        let v=crate::storage::Store::open(&path).unwrap().schema_version().unwrap();
        rusqlite::Connection::open(&path).unwrap().execute("INSERT INTO schema_migrations(version) VALUES (?1)",[v+1]).unwrap();
        let g=crate::generated::Generated::open(&dir);
        let ja=|c:char|('\u{3040}'..='\u{30ff}').contains(&c)||('\u{4e00}'..='\u{9fff}').contains(&c);
        let lost=g.store_lost().expect("新しい版の DB で store_lost が None").to_owned();
        assert!(lost.contains("newer")&&!lost.chars().any(ja),"閉じる時の理由が英語でない: {lost}");
        let note=g.note.clone().unwrap_or_default();
        assert!(note.contains("newer")&&!note.chars().any(ja),"Home の 1 行が英語でない: {note}");
        let _=std::fs::remove_dir_all(&dir);
    }

    /// FX1 反証の直し: 時間の保存（`save_if_due`・debounce ごと）は Page の View の保存（`Generated::save_view_state` = native_direct では
    /// その Page に重ねた窓を隠す・「移る時だけ」の契約）を通らない。通るのは Quit と on_exit の `save_now` だけ。
    /// 負の対照: `save_if_due` を `save_now` に戻す（2df6b10 の形）→ 時間の保存の後に view state の行が在り赤。
    #[test] fn fx1_timed_save_does_not_leave_the_page(){
        let mut w=Win::open("fx1-timed");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        w.settle();
        w.app.go(project);
        w.settle();
        let crate::ui::shell::View::Project{page,..}=w.app.shell.view.clone() else{panic!("前提: Project に移れていない")};
        let rows=|w:&Win|rusqlite::Connection::open(w.dir.join("kangae.sqlite3")).unwrap()
            .query_row("SELECT COUNT(*) FROM ui_state WHERE key=?1",[crate::project::view_state_key(&page)],|r|r.get::<_,i64>(0)).unwrap();
        w.app.dirty=true;
        w.app.last_save=Instant::now().checked_sub(super::SAVE_DEBOUNCE*2).unwrap();
        w.app.save_if_due();
        assert!(!w.app.dirty,"前提: 時間の保存が走っていない");
        assert_eq!(rows(&w),0,"時間の保存が Page を離れる保存（save_view_state）を通った");
        w.app.save_now().unwrap();
        assert_eq!(rows(&w),1,"前提: Quit の保存（save_now）が view state を書かない = この試験は何も測っていない");
    }

    /// UI-4b 反証の直し × STATUSBAR-1101: sidebar を畳んだ間も開いた間も直近の message（保存の失敗・Runs の Stop の結果・移れない理由・起動の注意）が
    /// 窓の一番下の帯に 1 つだけ出る（sidebar の中ではない・toast にも出さない）。帯は窓の下端に在り、sidebar を畳んでも開いても同じ高さ。
    /// 負の対照: app.rs の update の `StatusBar` の `status` に `""` を渡す → 0 で赤・toast に戻す → 帯の外の 1 つで赤。
    #[test] fn ui4b_message_shows_while_the_sidebar_is_folded(){
        use crate::ui::shell::View;
        let msg="Couldn't save your changes · disk is full";
        let shown=|out:&egui::FullOutput|->Vec<egui::Rect>{out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い")
            .nodes.iter().filter(|(_,n)|n.value()==Some(msg)||n.label()==Some(msg))
            .filter_map(|(_,n)|n.bounds().map(|b|egui::Rect::from_min_max(egui::pos2(b.x0 as f32,b.y0 as f32),egui::pos2(b.x1 as f32,b.y1 as f32)))).collect()};
        let in_bar=|r:&[egui::Rect],w:&Win|r.len()==1&&r[0].bottom()>=w.size.y-crate::ui::theme::STATUS_H-1.0;
        let mut w=Win::open("ui4b-msg");
        w.size=Vec2::new(1100.0,800.0);
        w.app.message=msg.into();
        w.settle();
        let (_,_,out)=w.probe();
        assert!(!w.app.shell.sidebar_open,"前提: 幅 1100 の初めての起動は畳んで始まる");
        assert!(in_bar(&shown(&out),&w),"畳んだ間（狭い窓の既定）の Home の下の帯に message が 1 つ無い: {:?}",shown(&out));
        w.app.go(View::Runs);
        w.app.message=msg.into();
        w.settle();
        let (_,_,out)=w.probe();
        assert!(in_bar(&shown(&out),&w),"畳んだ間の Runs の下の帯に message が無い（Stop の結果が見えない）: {:?}",shown(&out));
        w.size=Vec2::new(1440.0,900.0);
        w.settle();
        let (_,_,out)=w.probe();
        assert!(w.app.shell.sidebar_open,"前提: 幅 1440 は開いて始まる");
        assert!(in_bar(&shown(&out),&w),"開いている間も下の帯の 1 つだけ（2 度出さない）: {:?}",shown(&out));
        w.key(egui::Key::S,CTRL_CMD);
        w.settle();
        let (_,_,out)=w.probe();
        assert!(!w.app.shell.sidebar_open,"前提: ⌃⌘S で畳めていない");
        assert!(in_bar(&shown(&out),&w),"⌃⌘S で畳んだ後に message が無い: {:?}",shown(&out));
    }

    /// UI-4b 反証の直し（Done 6）: 保存した行が在るのに今回は開けなかった Project（定義が今の検査を通らない = `start` の broken）の打ちかけを、
    /// 起動で捨てず次の保存でも消さない（捨てるのは保存した行が無い Project だけ）。定義が直って開けば、その Project へ移った時に欄へ戻る。
    /// 負の対照（commit message）: `open` の retain を「開けた Project だけ」（`app_of_project`）に戻す → 2 回目の開き直しで打ちかけが消えて赤。
    #[test] fn ui4b_draft_of_an_unopenable_project_is_kept(){
        use crate::ui::shell::View;
        let mut w=Win::open("ui4b-broken");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        let app_id=w.app.generated.app_of_project(&pid).expect("Project の app が無い");
        w.settle();
        w.app.go(project.clone());
        w.app.generated.panel_act(&pid,crate::ui::terminal_panel::PanelAct::Chat).unwrap();
        w.settle();
        w.type_in("What to create","add a chart");
        w.app.go(View::Home);
        w.settle();
        assert_eq!(w.app.shell.drafts.get(&pid).map(String::as_str),Some("add a chart"),"前提: Project の打ちかけが Shell::drafts に無い");
        // 保存した行は残したまま定義を壊す（開けない Project = broken）
        let db=rusqlite::Connection::open(w.dir.join("kangae.sqlite3")).unwrap();
        let good:String=db.query_row("SELECT json FROM apps WHERE id=?1",[&app_id],|r|r.get(0)).unwrap();
        db.execute("UPDATE apps SET json='{\"broken\":true}' WHERE id=?1",[&app_id]).unwrap();
        w.reopen();
        w.settle();
        assert!(w.app.generated.app_of_project(&pid).is_none(),"前提: 壊した Project が開いた");
        // もう 1 回終わって開き直す = on_exit の snapshot_drafts が UiSettings::drafts を書き直す（ここで消えれば打ちかけを失う）
        w.reopen();
        assert_eq!(w.app.store.ui.drafts.get(&pid).map(String::as_str),Some("add a chart"),"開けなかった Project の打ちかけを捨てた: {:?}",w.app.store.ui.drafts);
        db.execute("UPDATE apps SET json=?2 WHERE id=?1",[&app_id,&good]).unwrap();
        w.reopen();
        w.settle();
        w.app.go(project);
        w.settle();
        assert_eq!(w.app.shell.command,"add a chart","定義が直った Project へ移っても打ちかけが戻らない");
    }

    /// 保存待ち（700ms の debounce）は、入力が止まっても保存する frame を 1 つ頼み、保存した後は何も頼まない。
    #[test] fn w2b_ax_unsaved_change_asks_only_for_the_saving_frame(){
        let mut w=Win::open("w2b-ax-save");
        w.startup_answered();
        for _ in 0..3{w.step(0.5,false,vec![]);}
        w.app.mark();
        w.app.last_save=Instant::now()+Duration::from_secs(60); // debounce の途中（混雑で frame が遅くても過ぎない）
        w.step(0.0,false,vec![]);
        assert!(w.delay<=Duration::from_millis(700),"保存待ちなのに保存する frame を頼まない（{:?}）",w.delay);
        w.app.last_save=Instant::now().checked_sub(Duration::from_secs(1)).unwrap(); // debounce が過ぎた
        let dt=w.delay.as_secs_f64();
        w.step(dt,false,vec![]);
        assert!(!w.app.dirty,"保存する frame で保存していない: {}",w.app.message);
        assert_eq!(w.delay,Duration::MAX,"保存した後も frame を頼む");
    }

    /// G84（Provider ▸・operator ③ 10-28）: 行 = System default → plan の login → 利用者が足した API key（従量の印）。環境の鍵（EnvKey）は出さない。
    /// 使っている行は ai_active の行だけ（無い id・None = System default・EnvKey を使っている時は印の無い列）。語は rail の AI の行と同じ（account_label）。
    /// 負の対照（g-b の変異 gb_envkey）: provider_rows の Key の filter を外して全部を並べる → EnvKey の行が出て赤・gb_billed: Key の語の印を外す → 赤。
    #[test] fn g84_provider_rows_hide_the_env_key_and_mark_a_billed_key(){
        use crate::model::{AiAccount,UiSettings};
        let mut ui=UiSettings::default();
        ui.ai_accounts=vec![AiAccount::Key{id:"key-1".into(),last4:"a1b2".into()},AiAccount::EnvKey,AiAccount::Plan{id:"plan-1".into()}];
        let rows=|ui:&UiSettings|super::provider_rows(ui,true,&[]).into_iter().map(|(a,l,on)|(a.map(|a|a.id().to_owned()).unwrap_or_default(),l,on)).collect::<Vec<_>>();
        assert_eq!(rows(&ui),[
            ("".into(),"Claude Code".into(),true),
            ("plan-1".into(),"Claude 2".into(),false),
            ("key-1".into(),"API key ····a1b2 · billed per request by Anthropic".into(),false),
        ]);
        ui.ai_active=Some("key-1".into());
        assert_eq!(rows(&ui).iter().filter(|r|r.2).map(|r|r.0.as_str()).collect::<Vec<_>>(),["key-1"],"使っている行の印");
        ui.ai_active=Some("env".into());
        assert!(rows(&ui).iter().all(|r|!r.2&&!r.1.contains("environment")),"EnvKey の行を出した・印を付けた");
        ui.ai_active=Some("gone".into());
        assert!(rows(&ui)[0].2,"無い id は Claude Code（None）");
        // PROVIDER-1101: Claude Code の CLI が無い Mac は Claude の 3 種を出さず、見つけた AI だけ（login していない CLI は印つき）
        ui.ai_accounts.push(AiAccount::Found{id:"kimi".into(),name:"Kimi Code".into(),needs_sign_in:true});
        let labels:Vec<String>=super::provider_rows(&ui,false,&[]).into_iter().map(|r|r.1).collect();
        assert_eq!(labels,["Kimi Code · sign in first"],"claude の無い Mac に Claude の行を出した");
    }

    /// G84（operator 10-29 ②・REVIEW-GB の補足）: 手で書いた設定で id が重なる（Plan{id:"env"} と EnvKey）時も、Provider ▸ の plan の行を選ぶと plan が使われ、
    /// 使っている印はその行に付く（環境の鍵 = 従量に替わらない）。種類は保存して読み直しても残る。種類の無い前の設定は id だけで引く（今のまま）。
    /// 負の対照（g-b の変異 gb_kind）: UiSettings::active_account の種類の比べを外す → 列で先の EnvKey が使われて赤。
    #[test] fn g84_a_shared_id_picks_the_row_by_kind_and_id(){
        use crate::model::{AiAccount,UiSettings};
        let plan=AiAccount::Plan{id:"env".into()};
        let mut ui=UiSettings::default();
        ui.ai_accounts=vec![AiAccount::EnvKey,plan.clone()];
        let row=super::provider_rows(&ui,true,&[]).into_iter().find(|r|r.0.as_ref()==Some(&plan)).expect("plan の行");
        assert_eq!(row.1,"Claude · env");
        assert_eq!(ui.set_active_account(row.0.as_ref()),(None,None));
        assert_eq!(ui.active_account(),Some(&plan),"plan の行を選んだのに環境の鍵が使われる");
        assert_eq!(super::account_label(ui.active_account(),&ui,&[]),"Claude · env");
        let on:Vec<String>=super::provider_rows(&ui,true,&[]).into_iter().filter(|r|r.2).map(|r|r.1).collect();
        assert_eq!(on,["Claude · env"],"使っている印が plan の行に無い");
        let back:UiSettings=serde_json::from_str(&serde_json::to_string(&ui).unwrap()).unwrap();
        assert_eq!(back.active_account(),Some(&plan),"保存して読み直すと種類が落ちる");
        let mut old=UiSettings::default();
        (old.ai_accounts,old.ai_active)=(vec![AiAccount::Plan{id:"plan-1".into()}],Some("plan-1".into()));
        assert_eq!(old.active_account(),old.ai_accounts.first(),"種類の無い前の設定を引けない");
    }
    /// FX4: 1 つの欄に AX の SetValue だけで文を置く（click しない = ⌘K の list を閉じない）。
    fn fx4_set(w:&mut Win,name:&str,text:&str){
        let field=Win::field(&w.step(0.1,true,vec![]),name);
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::SetValue,target:field,data:Some(egui::accesskit::ActionData::Value(text.into()))})]);
    }

    /// FX4（FX-016・UIUX H06）: ⌘K は 2 つ目の欄も窓も出さず、1 つの欄に focus を置いて欄のすぐ下（同じ左端）に list を落とす。Runs（header の欄）と Home（主領域の 52%）。
    /// Esc で閉じると ⌘K の前の打ちかけが欄に戻る。負の対照（本体の commit で取る）: `palette::draw` を Modal に戻す → 欄が 2 つで赤。
    #[test] fn fx4_cmd_k_drops_the_list_under_the_one_field(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx4-one-field");
        for (view,name) in [(View::Runs,"Search commands and projects"),(View::Home,"What to create")]{
            w.app.go(view.clone());
            w.settle();
            w.app.shell.command="a draft".into();
            w.key(egui::Key::K,CMD);
            assert!(w.app.shell.palette.open,"{name}: ⌘K で list が開かない");
            assert_eq!(w.app.shell.command,"","{name}: ⌘K の間も欄に打ちかけが残る（⌘K の語と混ざる）");
            let out=w.step(0.1,true,vec![]);
            let fields=Win::word_fields(&out);
            assert_eq!(fields.iter().map(|f|f.0.as_str()).collect::<Vec<_>>(),[name],"{name}: ⌘K で 2 つ目の欄が出た");
            let f=fields[0].1;
            assert_eq!(out.platform_output.accesskit_update.as_ref().map(|t|t.focus),Some(Win::field(&out,name)),"{name}: ⌘K で 1 つの欄に focus が無い");
            let t=out.platform_output.accesskit_update.as_ref().unwrap();
            let row=t.nodes.iter().find(|(_,n)|n.role()==Role::Button&&n.label()==Some("Go to Connections")).and_then(|(_,n)|n.bounds()).unwrap_or_else(||panic!("{name}: list の行が無い"));
            assert!(row.y0>=f[3]-0.5&&(row.x0-f[0]).abs()<=1.0,"{name}: list が欄のすぐ下・同じ左端に無い（欄 {f:?}・行 {row:?}）");
            w.key(egui::Key::Escape,egui::Modifiers::NONE);
            assert!(!w.app.shell.palette.open,"{name}: Esc で閉じない");
            assert_eq!(w.app.shell.command,"a draft","{name}: 閉じても ⌘K の前の打ちかけが戻らない");
        }
    }

    /// FX4（FX-016・UIUX N17）: 最近の行は ⌘K の `Remember recent commands` で覚え始め、選んだ行が頭に来て、開き直しても残る。既定は覚えない。
    #[test] fn fx4_recent_commands_are_opt_in_and_survive_reopen(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx4-recent");
        w.app.go(View::Runs);
        w.settle();
        assert_eq!(w.app.shell.palette.recent,None,"既定で覚えている");
        for q in ["remember recent","go to home"]{
            w.key(egui::Key::K,CMD);
            fx4_set(&mut w,"Search commands and projects",q);
            w.key(egui::Key::Enter,egui::Modifiers::NONE);
            w.settle();
        }
        assert_eq!(w.app.shell.view,View::Home,"⌘K の Go to Home で移らない");
        assert_eq!(w.app.shell.palette.recent,Some(vec!["Go to Home".to_owned()]),"選んだ行を覚えていない（覚える行そのものは覚えない）");
        w.reopen();
        w.settle();
        assert_eq!(w.app.store.ui.recent_commands,Some(vec!["Go to Home".to_owned()]),"開き直しで最近の行が消えた");
        w.key(egui::Key::K,CMD);
        let out=w.step(0.1,true,vec![]);
        let t=out.platform_output.accesskit_update.as_ref().unwrap();
        let mut rows:Vec<(f64,&str)>=t.nodes.iter().filter(|(_,n)|n.role()==Role::Button).filter_map(|(_,n)|Some((n.bounds()?.y0,n.label()?))).filter(|(_,l)|l.starts_with("Go to")||l.starts_with("New ")).collect();
        rows.sort_by(|a,b|a.0.total_cmp(&b.0));
        assert_eq!(rows.first().map(|r|r.1),Some("Go to Home"),"開き直した ⌘K の頭が最近の行でない: {rows:?}");
    }

    /// FX4（CHAT-019・UIUX H02）: Home の 1 つの欄の中心は主領域の見えている高さの 52%（`home::FIELD_AT`）に縛る（±1%・3 つの窓の幅）。header が 44 になっても同じ。
    /// 今の位置を縛る試験（本体の前から緑の予定）。負の対照（本体の commit で取る）: `FIELD_AT` を 0.45 にする → 赤。
    #[test] fn fx4_home_field_center_stays_at_52_percent(){
        use crate::ui::shell::View;
        let mut w=Win::open("fx4-home-52");
        w.app.go(View::Home);
        for (width,height) in [(960.0,700.0),(1200.0,800.0),(1440.0,900.0)]{
            w.size=Vec2::new(width,height);
            w.settle();
            let (_,sk,out)=w.probe();
            let f=Win::field(&out,"What to create");
            let b=out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().find(|(id,_)|*id==f).and_then(|(_,n)|n.bounds()).unwrap();
            let at=((b.y0+b.y1)/2.0-f64::from(sk.body.top()))/f64::from(sk.body.height());
            assert!((at-f64::from(crate::ui::home::FIELD_AT)).abs()<=0.01,"幅 {width}: 欄の中心が主領域の {at:.3}（52% ±1% でない）");
        }
    }

    /// I30（owner 271「chatbarが動かないように下に固定。今上にありすぎる」）: 会話の間の Home の欄は主領域の底に固定 = 会話 1 通・10 通・30 通（見えている高さより長い）で
    /// 欄の下端と主領域の底の距離が同じ（±1）・欄は主領域の下の 1/4・最後の 1 通は欄の上に見える（会話の ScrollArea の stick_to_bottom）・
    /// card の上で wheel を回しても主領域は送られない（送れる高さが 1px でも在れば赤 = 欄が底から動かない事を縛る）。
    /// 空の Home は `FIELD_AT` のまま（`fx4_home_field_center_stays_at_52_percent`）。AI は無しに固定（CLI を確かめる 1 行が欄の下に出入りしない）。
    /// 負の対照（手で・未実行）: `home::draw` の talking の枝を前の形（会話と card を主領域の 1 つの流れに続けて描く）に戻す → 1 通の欄が主領域の上半分で赤・30 通で主領域が送られて赤。
    #[test] fn i30_home_chat_bar_stays_at_the_bottom_while_talking(){
        use crate::domain::chat::{Mark,Turn,Who};
        use crate::ui::shell::View;
        let mut w=Win::open("i30-bottom");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        w.size=Vec2::new(1200.0,800.0);
        w.app.go(View::Home);
        let said=|n:usize|(0..n).map(|i|Turn{who:if i%2==0{Who::You}else{Who::Shibubu},text:format!("line {i}"),mark:Mark::None,at:"2026-11-03T00:00:00Z".into(),attachments:Vec::new(),choices:Vec::new(),proposal:None,preview:Vec::new(),sections:Vec::new(),needs:None}).collect::<Vec<_>>();
        let mut gaps=Vec::new();
        for n in [1usize,10,30]{
            w.app.shell.talk=said(n);
            w.settle();
            let (_,sk,out)=w.probe();
            let f=Win::field(&out,"What to create");
            let nodes=&out.platform_output.accesskit_update.as_ref().expect("AccessKit の木").nodes;
            let b=nodes.iter().find(|(id,_)|*id==f).and_then(|(_,x)|x.bounds()).expect("欄の rect");
            let body=sk.body;
            assert!(b.y1<=f64::from(body.bottom())&&b.y0>=f64::from(body.top()+body.height()*0.75),"{n} 通: 欄 {b:?} が主領域 {body:?} の下の 1/4 に無い");
            assert_eq!(crate::ui::shell::main_scrolled(&w.ctx),0.0,"{n} 通: 主領域が送られた（欄が底から外れる）");
            let last=format!("line {}",n-1);
            let l=nodes.iter().find(|(_,x)|x.label().or(x.value()).is_some_and(|s|s.contains(&last))).and_then(|(_,x)|x.bounds()).unwrap_or_else(||panic!("{n} 通: 最後の 1 通「{last}」が AX に無い"));
            assert!(l.y0>=f64::from(body.top())&&l.y1<=b.y0,"{n} 通: 最後の 1 通 {l:?} が欄 {b:?} の上に見えない（主領域 {body:?}）");
            gaps.push(f64::from(body.bottom())-b.y1);
            let on_card=egui::pos2(b.x0 as f32+10.0,b.y1 as f32+20.0);
            w.step(0.1,true,vec![Event::PointerMoved(on_card),Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:Vec2::new(0.0,-400.0),modifiers:egui::Modifiers::NONE}]);
            w.settle();
            assert_eq!(crate::ui::shell::main_scrolled(&w.ctx),0.0,"{n} 通: card の上の wheel で主領域が送られた（欄が底から動く）");
        }
        assert!(gaps.iter().all(|g|(g-gaps[0]).abs()<=1.0),"欄の下端と主領域の底の距離が会話の長さで変わる（1・10・30 通）: {gaps:?}");
    }

    /// FX4（FX-022・MH-M20）: 960 幅の Project の画面で、Tab だけで見えている全 button に届き、一巡して頭へ戻る。どの button にも AX の名前（VoiceOver が読む）。
    /// 実機の VoiceOver は DEV-024（試験の側は AX の木の名前まで）。
    #[test] fn fx4_narrow_window_reaches_every_button_by_tab(){
        let mut w=Win::open("fx4-tab");
        w.size=Vec2::new(960.0,700.0);
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).expect("Project を作れない").expect("主デモの一言は Project");
        w.app.go(project);
        w.settle();
        let out=w.step(0.1,true,vec![]);
        let t=out.platform_output.accesskit_update.as_ref().unwrap();
        let buttons:Vec<(NodeId,String)>=t.nodes.iter().filter(|(_,n)|n.role()==Role::Button&&!n.is_hidden()&&!n.is_disabled()).map(|(id,n)|(*id,n.label().unwrap_or_default().to_owned())).collect();
        assert!(buttons.iter().all(|(_,l)|!l.trim().is_empty()),"名前の無い button が {} 個",buttons.iter().filter(|(_,l)|l.trim().is_empty()).count());
        let mut seen=std::collections::HashSet::new();
        let mut first=None;
        for _ in 0..400{
            let out=w.step(0.05,true,vec![Event::Key{key:egui::Key::Tab,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::NONE}]);
            let Some(f)=out.platform_output.accesskit_update.map(|t|t.focus) else{continue};
            if first==Some(f){break}
            first.get_or_insert(f);
            seen.insert(f);
        }
        let missed:Vec<&str>=buttons.iter().filter(|(id,_)|!seen.contains(id)).map(|(_,l)|l.as_str()).collect();
        assert!(missed.is_empty(),"Tab で届かない button: {missed:?}");
    }

    /// LAYOUT: Page を開いて描き、Page の部品（保存した木の root の直下の子）の (key, component, 矩形)・描いた矩形の全部
    /// （`face:<key>` = 面の枠の矩形・`components::source_surface` が入れる）・主領域の下端（= 下の panel の上端。描いた物で測る = L1-NORAIL で panel の形が
    /// 替わっても定数を写さない）・最後の frame の出力（AX の木）。前の frame の高さで見積もる部品があるので settle の後に読む
    #[cfg(feature="mirror")]
    fn layout_parts(w:&mut Win,project:&str,page:&str)->(Vec<(String,String,Rect)>,std::collections::HashMap<String,Rect>,f32,egui::FullOutput){
        w.app.go(crate::ui::shell::View::Project{project:project.to_owned(),page:page.to_owned()});
        w.settle();
        let (bottom,out)=w.main_bottom();
        let (p,rects)=w.app.generated.layout_probe(project);
        let pg=p.pages.iter().find(|g|g.id==page).expect("Page");
        let parts=pg.root.children.iter().map(|c|(c.key.clone(),c.component.clone(),*rects.get(&c.key).unwrap_or_else(||panic!("{}: {} を描いていない（{:?}）",pg.title,c.key,rects.keys().collect::<Vec<_>>())))).collect();
        (parts,rects,bottom,out)
    }

    /// LAYOUT（DOGFOOD-X 周 2 の赤・.app 473f4037 で下の Terminal の panel を開くと録音の帯が panel の下に隠れて押せない・見えやすさ 2/5）:
    /// 下の panel を開いた形（L1-NORAIL: ⌃` = 既定の Chat の tab・⌃⇧` = Terminal の tab を足して開く = DOGFOOD-X の形。どちらも主領域 × `DEFAULT_FRACTION`・
    /// `OPEN_MIN_H` 以上）でも、録音の帯と `Record humming` が panel の上端より上に在る（見えて押せる）。⌃` で畳むと、開く前の面の枠の高さに戻る（operator 10-31:
    /// panel のために自動で畳んだ Music を閉じた後も畳んだままにしない = 完成予想図の Music 右上・`MirrorPages::fold_for_room`）。Terminal の tab を畳んだ高さも
    /// Chat と同じ 28 ＋ 欄の行（NORAIL R2）なので、開く前（Chat を畳んだ形）と同じ高さに戻る。tab は残る（呼ぶ側が `close_terminals`）。
    #[cfg(feature="mirror")]
    fn recorder_over_open_panel(w:&mut Win,project:&str,page:&str,at:&str){
        let (_,before,_,_)=layout_parts(w,project,page);
        for (tab,open) in [("Chat",egui::Modifiers::CTRL),("Terminal",egui::Modifiers::CTRL|egui::Modifiers::SHIFT)]{
            let at=format!("{at} {tab} の tab");
            w.key(egui::Key::Backtick,open);
            let (ps,_,top,out)=layout_parts(w,project,page);
            assert!(top<=w.size.y-crate::ui::terminal_panel::OPEN_MIN_H+0.5,"{at}: 前提: 下の panel が開いていない（主領域の下端 {top}）");
            assert_eq!(w.app.generated.panel(project).chat,tab=="Chat","{at}: 前提: 開いた tab が違う");
            let rec=ps.iter().find(|(_,c,_)|c=="recorder").map(|p|p.2).expect("録音の帯");
            let t=out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
            let button=t.nodes.iter().find(|(_,n)|n.role()==Role::Button&&n.label()==Some("Record humming")).and_then(|(_,n)|n.bounds()).unwrap_or_else(||panic!("{at}: Record humming が無い"));
            assert!(rec.min.y>=0.0&&rec.max.y<=top+0.5,"{at}: 下の panel（上端 {top}）を開くと録音の帯が隠れる: {rec:?}（全部: {ps:?}）");
            assert!(button.y0 as f32>=rec.min.y-0.5&&button.y1 as f32<=top+0.5,"{at}: 下の panel（上端 {top}）を開くと Record humming が押せない: {button:?}");
            w.key(egui::Key::Backtick,egui::Modifiers::CTRL);
            let (_,after,_,_)=layout_parts(w,project,page);
            for (k,r) in before.iter().filter(|(k,_)|k.starts_with("face:")){
                let h=after.get(k).map_or(0.0,|a|a.height());
                assert!((h-r.height()).abs()<=1.0,"{at}: 下の panel を閉じても {k} が開く前の高さ {:.0} に戻らない: {h:.0}（panel のための畳みが残った）",r.height());
            }
        }
    }

    /// LAYOUT（owner 10-31 の 19 点の 3「録音のところが下に行きすぎて scroll しなきゃいけない・youtube の画面も縮こまっている」・4「reaper が小さすぎる」・
    /// 5「service ごとの大きさや表示を鑑みて」・operator 10-31 の門 1〜3）: 本物の殻（sidebar・header・下の panel）で音楽の Project（owner の通しの AI の答えと同じ 4 席）を開く。
    /// 門 = 1440×900（L1-NORAIL = 右の rail は無い）: REAPER の列 ≥ Page の 60%・YouTube の枠 ≥ 390 × 400・Music の枠は窓の中で高さ ≥ 160・
    /// side は Music が上 / YouTube が下（model の席の順に依らない = 2 通りの順で）・録音の帯は REAPER の真下で窓の下端まで（REAPER が残りを取る）・全部が窓の中。
    /// 1200×800: 全部が窓の中（入らなければ Music を畳む = 黄の既定）・重ならない・YouTube の枠の幅 ≥ `WEB`。
    /// 2 つの窓とも、下の panel を開いても録音の帯と `Record humming` が panel の上（`recorder_over_open_panel`・DOGFOOD-X 周 2 の赤・席の順の後に 1 回）。
    /// README の Project（workspace 付き = Dev の Page に部品が複数）は 2 つの窓で全 Page が窓の中・重ならない。
    /// 負の対照（docs/internal/layout-arm.sh）: 1080 の上限・録音の置き場・side の並べ替え・列の幅・Music を床のまま・reserved_below・伸びる面を床で止める（`grow_height`）を戻す → 赤。
    #[cfg(feature="mirror")]
    #[test] fn layout_pages_fit_the_window_by_service_size(){
        use crate::ui::shell::View;
        use crate::composer::project::{self as cp,KnownApp};
        use crate::ui::renderer::layout::{WEB,APP_SIDE};
        use serde_json::{json,Value};
        struct PlanOf(Value);
        impl crate::composer::ModelProvider for PlanOf{fn complete_json(&self,_:&str,_:&Value)->Result<Value,String>{Ok(self.0.clone())}}
        let seat=|title:&str,kind:&str,app:Option<&str>,url:Option<&str>,slot:&str,feeds:Option<&str>|json!({"title":title,"kind":kind,"app_name":app,"bundle_id":null,"file":null,"url":url,"install_url":null,"instead":[],"page":0,"slot":slot,"why":"","feeds":feeds});
        let plan=|music_first:bool|{
            let (yt,am)=(seat("YouTube","web",None,Some("https://www.youtube.com"),"side",None),seat("Apple Music","app",Some("Music"),None,"side",None));
            let side=if music_first{[am,yt]}else{[yt,am]};
            json!({"shape":"project","name":"鼻歌録音と音楽参照","pages_requested":false,"seats":[
                seat("REAPER","app",Some("REAPER"),None,"main",None),side[0].clone(),side[1].clone(),
                seat("鼻歌","recorder",None,None,"bottom",Some("REAPER"))]})
        };
        let apps=[KnownApp{name:"REAPER".into(),bundle_id:"com.cockos.reaper".into()},KnownApp{name:"Music".into(),bundle_id:"com.apple.Music".into()}];
        let goal="REAPERとYouTube、Apple Musicをつなぎ、鼻歌を録音してREAPERに入れる";
        // (窓の幅, 高さ, 門か)
        for (ww,hh,gate) in [(1440.0_f32,900.0_f32,true),(1200.0,800.0,false)]{
            let mut w=Win::open("layout");
            w.size=Vec2::new(ww,hh);
            w.app.llm_rx=None;
            w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
            let tier=crate::ui::shell::Tier::for_width(ww);
            let page_w=ww-tier.sidebar-2.0*tier.gutter;
            let at=format!("{ww}×{hh}");
            // 窓の下端 = 畳んだ下の panel の上端（描いた主領域の下端で測る）
            let inside=|r:&Rect,bottom:f32|r.min.y>=0.0&&r.max.y<=bottom+0.5&&r.max.x<=ww+0.5;
            let no_overlap=|ps:&[(String,String,Rect)],what:&str|{
                for (i,a) in ps.iter().enumerate(){for b in &ps[i+1..]{
                    let x=a.2.intersect(b.2);
                    assert!(x.width()<=0.5||x.height()<=0.5,"{at} {what}: {} {:?} と {} {:?} が重なる",a.0,a.2,b.0,b.2);
                }}
            };
            // 音楽の Project（side の列の 2 通りの席の順）
            let mut last=None;
            for music_first in [false,true]{
                let at=format!("{at} 席の順 Music が先 = {music_first}");
                let ctx=crate::composer::ComposeContext{app_id:Some(format!("app.{}",crate::storage::new_id())),..Default::default()};
                let made=cp::plan_project(goal,&PlanOf(plan(music_first)),&apps,&[],&ctx,None).expect("案").expect("Project の形");
                let Ok(View::Project{project,page})=w.app.generated.save_plan(made,None) else{panic!("保存できない")};
                last=Some((project.clone(),page.clone()));
                let (ps,rects,bottom,_)=layout_parts(&mut w,&project,&page);
                assert!(bottom<=hh+0.5&&bottom>hh-crate::ui::terminal_panel::OPEN_MIN_H,"{at}: 前提: 下の panel が畳まれていない（主領域の下端 {bottom}）");
                assert_eq!(ps.len(),4,"{at}: 音楽の Page の部品が 4 つでない: {ps:?}");
                let get=|key:&str|ps.iter().find(|(k,..)|k.as_str()==key).map(|p|p.2).unwrap_or_else(||panic!("{key} が無い: {ps:?}"));
                let face=|key:&str|*rects.get(&format!("face:{key}")).unwrap_or_else(||panic!("{at}: {key} の枠を描いていない"));
                let (reaper,rec)=(get("reaper-surface"),ps.iter().find(|(_,c,_)|c=="recorder").map(|p|p.2).expect("録音の帯"));
                assert!(reaper.min.x>=tier.sidebar-0.5,"{at}: 前提: 本物の殻（sidebar {} の右）で描いていない: {reaper:?}",tier.sidebar);
                no_overlap(&ps,"音楽");
                assert!(inside(&rec,bottom),"{at}: 録音の帯が窓（下端 {bottom}）の外 = scroll しないと見えない: {rec:?}");
                for (k,_,r) in &ps{assert!(inside(r,bottom),"{at}: {k} が窓（下端 {bottom}）の外: {r:?}（全部: {ps:?}）");}
                // 録音の帯は REAPER の真下（間 1 つ）で窓の下端まで = REAPER が窓の残りを取る（前の幅 × 0.625 の縛りなら帯の下に空き）
                assert!(rec.min.y>=reaper.max.y-0.5&&rec.min.y-reaper.max.y<=2.0*crate::ui::theme::SP_4+0.5,"{at}: 録音の帯が REAPER の真下でない: REAPER {reaper:?}・録音 {rec:?}");
                assert!(rec.max.y>=bottom-64.0,"{at}: REAPER が窓の残りを取っていない（録音の帯の下端 {} が窓の下端 {bottom} から遠い）: REAPER {reaper:?}",rec.max.y);
                let yt=face("youtube-surface");
                assert!(yt.width()>=WEB.0-0.5,"{at}: YouTube の枠が {} より細い（列の service の min の幅）: {yt:?}",WEB.0);
                if !gate{continue;}
                let am=face("apple-music-surface");
                assert!(reaper.width()>=0.6*page_w-0.5,"{at}: REAPER の列が Page（{page_w}）の 60% より細い: {reaper:?}");
                assert!(yt.width()>=WEB.0-0.5&&yt.height()>=WEB.1-0.5,"{at}: YouTube の枠が縮こまっている（{:.0}×{:.0} < {}×{}）",yt.width(),yt.height(),WEB.0,WEB.1);
                assert!(inside(&am,bottom)&&am.height()>=APP_SIDE.1-0.5,"{at}: Music の枠が窓の外か {} より低い（畳んだ）: {am:?}",APP_SIDE.1);
                assert!(am.max.y<=yt.min.y+0.5&&am.min.x<yt.max.x&&yt.min.x<am.max.x,"{at}: side が Music の上・YouTube の下でない: Music {am:?}・YouTube {yt:?}");
            }
            // 下の panel を開いて閉じる形は席の順の後に 1 回（開く・閉じるで面の枠が開く前に戻る事も見る）
            let (project,page)=last.expect("音楽の Project");
            recorder_over_open_panel(&mut w,&project,&page,&at);
            // README の Project（主デモの一言・workspace 付き = Dev の Page に部品が複数・Page ごと）
            let ws=w.dir.join("repo");
            std::fs::create_dir_all(ws.join(".git")).unwrap();
            let Some(View::Project{project,..})=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,Some(&ws),None).expect("Project を作れない") else{panic!("主デモの一言は Project")};
            let (p,_)=w.app.generated.layout_probe(&project);
            assert!(p.pages.len()>=2,"{at}: 前提: README の Project の Page が 1 枚");
            assert!(p.pages.iter().any(|g|g.root.children.len()>=2),"{at}: 前提: README の Project のどの Page も部品が 1 つ（重なりを測れない）");
            for pg in &p.pages{
                let (ps,_,bottom,_)=layout_parts(&mut w,&project,&pg.id);
                no_overlap(&ps,&pg.title);
                for (k,_,r) in &ps{assert!(inside(r,bottom),"{at} {}: {k} が窓（下端 {bottom}）の外: {r:?}",pg.title);}
            }
            w.app.generated.close_terminals();
        }
    }

    /// LAYOUT の本物（製品の claude の道・plan の login・Shibubu の知識は渡さない・呼ぶ上限 2 回）: owner の音楽の一言を本物の model に組ませ、rail の無い 1440×900 の
    /// 本物の殻で全 Page の部品が重ならず窓の中・Web の枠 ≥ `WEB`・録音の帯の Page が完成予想図（owner 118・docs/assets/music-workspace-concept.png）と同じ配置
    /// （面は REAPER・Music・YouTube の 3 つ・REAPER が左で Page の 60% 以上・右の列は Music が上 / YouTube が下・録音の帯は REAPER の下）・下の panel を開いても録音の帯が
    /// 見えて押せる（`recorder_over_open_panel`）。AI が失敗して名前の表に落ちた（`llm_line`）・
    /// 呼んだ記録に ok が無い・`input_tokens` が（instructions + schema + 一言の文字数 + 8000）を超えた（107 通目・規則や記憶が混ざった）→ 赤。
    /// 手で回す: `SHIBUBU_W13_LIVE_DIR=<空の dir> cargo test --features mirror --bin shibubu layout_live -- --ignored --nocapture`（記録は `<dir>/llm-calls.jsonl`）。
    #[cfg(feature="mirror")]
    #[test]
    #[ignore = "本物の claude -p を呼ぶ（owner の plan の quota・上限 2 回）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn layout_live_music_goal_fits_the_window(){
        use crate::ui::shell::View;
        use crate::llm_cli::{CallRecord,CliConfig,CliLlm,CALL_LOG};
        use crate::domain::project::{SurfaceBackendKind,SurfaceTarget};
        use crate::ui::renderer::layout::{WEB,APP_SIDE};
        let dir=std::path::PathBuf::from(std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"));
        std::fs::create_dir_all(&dir).unwrap();
        let cli=CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の claude");
        let mut w=Win::open_in(dir.join("app"));
        w.app.llm_rx=None;
        w.app.generated.set_budget(Some(2),None);
        w.app.generated.set_llm(Ok(Box::new(cli)));
        let goal="REAPERとYouTube、Apple Musicをつなぎ、鼻歌を録音してREAPERに入れる";
        let Some(View::Project{project,..})=w.app.generated.create_project(goal,None,None).expect("Project を作れない") else{panic!("音楽の一言が Project にならない")};
        // AI の答えで組んだ（名前の表に落ちていない）・呼んだ記録・混ざりの上限
        assert!(w.app.generated.llm_line().is_none(),"AI が失敗して名前の表で組んだ（偽の緑）: {:?}",w.app.generated.llm_line());
        let recs:Vec<CallRecord>=std::fs::read_to_string(dir.join(CALL_LOG)).expect("呼んだ記録").lines().map(|l|serde_json::from_str(l).expect("記録の行")).collect();
        println!("calls: {}",recs.len());
        assert!(recs.iter().any(|r|r.outcome=="ok"),"ok の呼び出しが無い: {recs:?}");
        for r in &recs{
            let (tokens,chars)=(r.input_tokens.expect("usage.input_tokens"),r.prompt_chars.expect("文字数"));
            assert!(tokens<=chars+8_000,"{} の input_tokens {tokens} が上限 {} を超えた（規則・記憶が混ざった）",r.name,chars+8_000);
        }
        let (p,_)=w.app.generated.layout_probe(&project);
        // 前提: 録音の帯と面 2 つ以上が同じ Page に在る（model が録音を落とす・Page を 1 部品ずつに分けると下の検査が自明に緑）
        assert!(p.pages.iter().any(|g|g.root.children.iter().any(|c|c.component=="recorder")&&g.root.children.iter().filter(|c|c.component=="source_surface").count()>=2),
            "前提: 録音の帯と面 2 つ以上の Page が無い: {:?}",p.pages.iter().map(|g|(g.title.clone(),g.root.children.iter().map(|c|c.component.clone()).collect::<Vec<_>>())).collect::<Vec<_>>());
        let spec=|key:&str|p.pages.iter().flat_map(|g|g.root.children.iter()).find(|c|c.key==key)
            .and_then(|c|c.props.get("surface_ref").and_then(|v|v.as_str()))
            .and_then(|sid|p.surface_specs.iter().find(|s|s.id==sid));
        // 完成予想図の面の名（題は model が決めるので、起こす物 = bundle id / URL で引く）
        let named=|key:&str|match spec(key).map(|s|&s.target){
            Some(SurfaceTarget::App{bundle_id,..}) if bundle_id.to_lowercase().contains("reaper")=>"REAPER",
            Some(SurfaceTarget::App{bundle_id,..}) if bundle_id.eq_ignore_ascii_case("com.apple.music")=>"Music",
            Some(SurfaceTarget::Url{url}) if url.contains("youtube.com")||url.contains("youtu.be")=>"YouTube",
            _=>"other",
        };
        let tier=crate::ui::shell::Tier::for_width(1440.0);
        let page_w=1440.0-tier.sidebar-2.0*tier.gutter;
        let mut rec_page=None;
        for pg in &p.pages{
            let (ps,rects,bottom,_)=layout_parts(&mut w,&project,&pg.id);
            println!("{}: {:?}",pg.title,ps.iter().map(|(k,c,r)|format!("{k}({c}) {:.0},{:.0} {:.0}×{:.0}",r.min.x,r.min.y,r.width(),r.height())).collect::<Vec<_>>());
            for (i,a) in ps.iter().enumerate(){for b in &ps[i+1..]{
                let x=a.2.intersect(b.2);
                assert!(x.width()<=0.5||x.height()<=0.5,"{}: {} {:?} と {} {:?} が重なる",pg.title,a.0,a.2,b.0,b.2);
            }}
            for (k,_,r) in &ps{assert!(r.min.y>=0.0&&r.max.y<=bottom+0.5&&r.max.x<=1440.5,"{}: {k} が窓（下端 {bottom}）の外: {r:?}",pg.title);}
            let faces:Vec<(String,&str,Rect)>=ps.iter().filter(|(_,c,_)|c=="source_surface")
                .map(|(k,..)|(k.clone(),named(k),*rects.get(&format!("face:{k}")).unwrap_or_else(||panic!("{}: {k} の枠を描いていない",pg.title)))).collect();
            for (k,_,r) in &faces{
                if spec(k).map(|s|s.backend)==Some(SurfaceBackendKind::WebView){assert!(r.width()>=WEB.0-0.5,"{}: Web の枠が縮こまっている: {r:?}",pg.title);}
            }
            // owner 118「本物の私の完成予想図 png と同じものができたら成功」= docs/assets/music-workspace-concept.png・docs/v15/assets/task-3-music-studio.png と同じ配置
            // （写真の一致ではない）: 録音の帯の Page に面が 3 つ（REAPER・Apple Music・YouTube）・REAPER が左で Page の 60% 以上（図は ≈ 73〜75%）・
            // 右の列は Music（app）が上 / YouTube（web）が下・録音の帯は REAPER の下・Music も YouTube も見えている（畳まれず縮こまらず）
            let Some(rec)=ps.iter().find(|(_,c,_)|c=="recorder").map(|p|p.2) else{continue};
            rec_page=Some(pg.id.clone());
            let mut names:Vec<&str>=faces.iter().map(|f|f.1).collect();
            names.sort();
            assert_eq!(names,["Music","REAPER","YouTube"],"{}: 面の数・中身が完成予想図（REAPER・Apple Music・YouTube の 3 つ）と違う: {faces:?}",pg.title);
            let face=|n:&str|faces.iter().find(|f|f.1==n).map(|f|f.2).unwrap();
            let (reaper,am,yt)=(face("REAPER"),face("Music"),face("YouTube"));
            assert!(reaper.width()>=0.6*page_w-0.5,"{}: REAPER が Page（{page_w}）の 60% より細い（図は ≈ 73〜75%）: {reaper:?}",pg.title);
            assert!(reaper.max.x<=am.min.x+0.5&&reaper.max.x<=yt.min.x+0.5,"{}: REAPER が左・Music と YouTube が右の列でない: REAPER {reaper:?}・Music {am:?}・YouTube {yt:?}",pg.title);
            assert!(am.max.y<=yt.min.y+0.5&&am.min.x<yt.max.x&&yt.min.x<am.max.x,"{}: 右の列が Music（app）の上・YouTube（web）の下でない: Music {am:?}・YouTube {yt:?}",pg.title);
            assert!(rec.min.y>=reaper.max.y-0.5&&rec.min.x<reaper.max.x&&reaper.min.x<rec.max.x,"{}: 録音の帯が REAPER の下でない: REAPER {reaper:?}・録音 {rec:?}",pg.title);
            assert!(am.height()>=APP_SIDE.1-0.5&&yt.width()>=WEB.0-0.5&&yt.height()>=WEB.1-0.5,"{}: Music が畳まれた / YouTube が縮こまった（図はどちらも見える）: Music {am:?}・YouTube {yt:?}",pg.title);
        }
        recorder_over_open_panel(&mut w,&project,&rec_page.expect("録音の帯の Page"),"録音の帯の Page");
        w.app.generated.close_terminals();
    }

    /// L4（DOGFOOD-X 周 2）: Connections に入っただけでは他の app（Claude Desktop・Claude Code）の MCP の設定を読まない。`mcp-found` の行の
    /// `Look in Claude's settings` を押した時だけ読み、答えの 1 行を出す（押す frame は 1 つ・読むのはこの Mac の HOME の file・書かない）。
    /// 負の対照: `enter_connections` で `found_config` を読む行を戻す → 製品の文の検査で赤（入る道は本物の Keychain を読むので frame では通さない）。
    #[test] fn l4_other_apps_mcp_settings_are_read_only_when_asked(){
        use crate::ui::shell::View;
        let mut w=Win::open("l4-mcp-ask");
        w.app.go(View::Connections);
        w.settle();
        assert!(w.app.mcp_found.is_none(),"Connections に入っただけで他の app の設定を読んだ");
        let out=w.step(0.1,true,vec![]);
        let look=Win::button(&out,"Look in Claude's settings").expect("Look in Claude's settings の button が無い");
        w.app.message.clear();
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:look,data:None})]);
        assert!(w.app.mcp_found.is_some(),"押しても読まない");
        assert!(w.app.message.contains("Claude"),"押した答えの 1 行が無い: {:?}",w.app.message);
        // 入る道（Intent::Go → enter_connections）は本物の Keychain を読むので試験では呼ばない → 製品の文で見る（rq016a と同じ切り方）。
        // 読む口は Look の腕の 1 つだけで、enter_connections は読まない。
        let src=include_str!("app.rs");
        let product=&src[..src.find("\n#[cfg(test)]\n").expect("tests の module")];
        let enter=product.find("fn enter_connections(").map(|i|&product[i..product[i+1..].find("\n    fn ").map_or(product.len(),|j|i+1+j)]).expect("enter_connections");
        assert!(!enter.contains("found_config("),"Connections に入っただけで他の app の設定を読む: {enter}");
        assert_eq!(product.matches("found_config(").count(),1,"他の app の設定を読む口が増えた");
        let (arm,read)=(product.find(r#"("mcp-found",A::CheckAgain)=>"#).expect("Look の腕"),product.find("found_config(").unwrap());
        assert!(read>arm&&read-arm<200,"他の app の設定を読む口が Look の腕の外に在る");
    }

    /// L4（111 通目）→ PROVIDER-1101: Connections の AI providers の群に、使っている AI（`ai`）・見つかって使っていない AI（`Use`）・sign in が要る CLI の行（Terminal の command）が上から並び、
    /// `Use` を押すと `+` › Provider と同じ切り替え（次の一言からその AI・確かめ中の 1 行）。見つかった答えは `found_rx` に直に渡す（試験の build は Mac を探さない）。
    /// 確かめの thread は 127.0.0.1:11434 に 1 回 GET する（答えは見ない・押した frame の後は回さない）。
    /// 負の対照: apply の `ai` の腕を外す → `Use` が `connection` の知らない行に落ちて何もせず、使う AI が替わらず赤。
    /// ACCOUNTS-1101: 見つかった AI の行は `settings::accounts` の部品（AX `Use {名前}`・`Sign in to {名前}`）・model の AI の行は `ai` の 1 つだけ。
    /// 負の対照（ACCOUNTS-1101）: Connections の `draw` に `ai` を渡さない（`&mut |_| None`）→ `Use Ollama` が無く赤。
    /// HOME-TERM-1101（owner 208「homeからkimiをsign inしようとしたらgragebandの中のterminalが開きsign inし出した」）: Project が在っても、Project の外で押した
    /// sign in は Home の Terminal（`generated::HOME_PLACE` の panel）の新しい tab で始まり、画面は移らず、他の Project の panel に tab を足さない。Home の ⌃` は Home の Terminal を開閉する。
    /// 変異: `start_cli_sign_in` に `last_project` の道を戻す → View が Project に移って赤・Home の ⌃` の腕を外す → 開閉せず赤。
    #[test] fn home_term1101_sign_in_outside_a_project_uses_the_home_terminal(){
        use crate::ui::shell::View;
        let mut w=Win::open("home-term");
        let project=w.app.generated.create_project(crate::ui::home::EXAMPLES[0].1,None,None).unwrap().unwrap();
        let View::Project{project:pid,..}=project.clone() else{panic!()};
        w.app.go(project);
        w.settle();
        w.app.go(View::Connections);
        w.settle();
        assert_eq!(w.app.store.ui.last_project.as_deref(),Some(pid.as_str()),"前提: 最後に開いた Project を覚えていない");
        let kimi=crate::model::AiAccount::Found{id:"kimi".into(),name:"Kimi Code".into(),needs_sign_in:true};
        w.app.store.ui.ai_accounts.push(kimi.clone());
        w.app.apply(&w.ctx.clone(),Intent::UseAiAccount(Some(kimi)));
        w.settle();
        assert_eq!(w.app.shell.view,View::Connections,"sign in で画面が移った: {}",w.app.message);
        let home=w.app.generated.panel(crate::generated::HOME_PLACE).clone();
        assert!(home.open&&home.tabs.iter().any(|t|t.name=="Kimi Code 1"),"Home の Terminal に sign in の tab が無い: {:?}",home.tabs.iter().map(|t|&t.name).collect::<Vec<_>>());
        assert!(!w.app.generated.panel(&pid).tabs.iter().any(|t|t.name.starts_with("Kimi")),"他の Project の panel で sign in した");
        w.app.go(View::Home);
        w.settle();
        let open=w.app.generated.panel(crate::generated::HOME_PLACE).open;
        w.key(egui::Key::Backtick,egui::Modifiers::CTRL);
        w.settle();
        assert_ne!(w.app.generated.panel(crate::generated::HOME_PLACE).open,open,"Home で ⌃` が Home の Terminal を開閉しない");
    }
    #[test] fn l4_connections_lists_found_ai_and_use_switches_to_it(){
        use crate::ui::shell::View;
        let mut w=Win::open("l4-conn-ai");
        w.app.llm_rx=None;
        w.app.generated.set_llm(Err(crate::llm::LlmError::NoCli{detail:"test".into()}));
        let ollama=crate::model::AiAccount::Found{id:"ollama".into(),name:"Ollama".into(),needs_sign_in:false};
        let (tx,rx)=std::sync::mpsc::channel();
        let grok=crate::model::AiAccount::Found{id:"grok".into(),name:"Grok".into(),needs_sign_in:true};
        tx.send(vec![ollama.clone(),grok]).unwrap();
        w.app.found_rx=Some(rx);
        w.app.go(View::Connections);
        w.settle();
        let rows=w.app.connections_model().rows;
        let ai:Vec<&str>=rows.iter().filter(|r|crate::ui::connections::group_of(&r.key)==0).map(|r|r.key.as_ref()).collect();
        assert_eq!(ai,["ai"],"AI の行は使っている AI の 1 行だけ（見つかった AI は account の部品）: {ai:?}");
        let out=w.step(0.1,true,vec![]);
        assert!(Win::button(&out,"Sign in to Grok").is_some(),"sign in の要る AI に Sign in が無い");
        let use_=Win::button(&out,"Use Ollama").expect("見つかった AI の行に Use が無い");
        // 押す frame は 1 つだけ（押した手は frame の終わりに当たり、確かめの答えは次の frame の頭で読む = 確かめの速さに左右されない）
        w.step(0.1,true,vec![Event::AccessKitActionRequest(ActionRequest{action:Action::Click,target:use_,data:None})]);
        assert_eq!(w.app.store.ui.active_account(),Some(&ollama),"Use で使う AI が替わらない: {}",w.app.message);
        assert_eq!(w.app.message,"Checking the AI account");
    }
}

/// 作る時だけ起こす bridge（app が在るかを聞く・起こせなければ None = 面は unavailable と理由）。
/// ponytail: 作るたびに起こす（ready まで最長 5 秒・UI の thread）。Page に面を置く W12W・W16 で常駐に替える
fn spawn_bridge()->Option<crate::surface::Bridge>{
    crate::surface::find_bridge().and_then(|p|crate::surface::Bridge::spawn(&p,Duration::from_secs(5))).ok()
}
