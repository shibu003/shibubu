//! W10: Terminal の画面の写し（VT の grid を表示と試験に渡す形）。作るのは `crate::terminal`（`TerminalManager::screen`・`Vt::screen`）、
//! 描くのは `crate::ui::renderer`（`TerminalPane::screen`）。ui は terminal を取り込まない test crate にも入るので、両方が引ける domain に置く。
//! VT の crate（alacritty_terminal / vt100）の型は出さない（どちらを選んでも同じ形）。
//! TERM-RESTORE-1101: 画面は `terminal::Snapshot`（byte・folder・大きさ）で保存し、次の起動の新しい shell の上に描く（この型は毎 frame の描く形のまま）。
//!
//! W11: 入力の lease（YCM-026・061）と RunReceipt（YCM-027・065）。ui / terminal / storage が同じ型を使う。
//! `runtime::lease::LeaseKey`（run 同士の排他）と `surface::ControlLease`（Native Direct / Mirror・W19-1）とは別物。
//! ここの lease は Terminal の入力（人の byte が Shibubu の PTY を通る）。Native Direct の面の人の lease は W19-1。
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 色。`Indexed` は 0〜15 が名前つきの 16 色（31m = `Indexed(1)`・91m = `Indexed(9)`）、16〜255 は xterm の 256 色。
/// RGB にするのは renderer（16 色は theme から・`Default` は前景 / 背景の既定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// cell の見た目（SGR）。`inverse` は前景と背景の入れ替えを描く側に任せる（`Default` 同士の入れ替えは theme が要るため）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermStyle {
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strike: bool,
}

/// 1 行の中の、同じ見た目の続きの cell。`col` は始めの cell の列（0 始まり）。
/// 幅 2 の文字（全角）は 1 文字として `text` に入り、続きの cell は入れない（次の run の `col` が 2 進む）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TermRun {
    pub col: u16,
    pub text: String,
    pub style: TermStyle,
}

/// 今の画面の中の cursor の位置（`history` の行を数えない・0 始まり）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermCursor {
    pub row: u16,
    pub col: u16,
}

/// Terminal の画面 1 枚の写し（`TerminalManager::screen(key, history)` が返す・毎 frame 作り直す）。
///
/// 使い方（試験の側）: `let s = mgr.screen(&key, 0)?; assert_eq!(s.visible_rows(), tmux_capture_pane_p_lines);`
/// 使い方（色）: `s.lines.iter().flatten().any(|r| r.text.starts_with('-') && r.style.fg == TermColor::Indexed(1))`（`git diff` の消した行）。
/// 保証する（作る側の約束）: `lines` は上から順で、先頭の `history` 行が scrollback（画面の上に流れた行・古い順）、残りのちょうど `rows` 行が今の画面。
/// 1 行の run は列 0 から隙間なく並ぶ（空白の cell も run に入る）。行末の「既定の見た目の空白」だけは run に入れない（色のある空白は入れる）。
/// alt screen（vim・tmux）の間は `history` = 0（alt screen は scrollback を持たない）。
/// 保証しない: 画面の外に出た行の数の上限（`TerminalConfig::scrollback_lines` が決める）・mouse の報告の mode・題名（OSC 0 / 2）。
/// Wave: W10
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TermScreen {
    pub rows: u16,
    pub cols: u16,
    pub lines: Vec<Vec<TermRun>>,
    pub history: usize,
    /// None = 隠れている（DECTCEM の `ESC [?25l`）。
    pub cursor: Option<TermCursor>,
    /// `ESC [?1049h` / `?47h` の間 true。
    pub alt_screen: bool,
    /// DECCKM（`ESC [?1h`）。矢印の key を `ESC O A` で送る（`ui::renderer::terminal_input_bytes`）。
    pub app_cursor: bool,
    /// `ESC [?2004h`。貼り付けを `ESC [200~ … ESC [201~` で包む。
    pub bracketed_paste: bool,
    /// TERM-RESTORE-1101: 前の起動の画面と新しい shell の間の線の行（`lines` の位置・その行に字は無い）。描く側は行の真ん中に細い線 1 本（`components::terminal_grid`）。
    /// 線の行が scrollback から流れ出た・`screen` の `history` の外・alt screen の間は None。
    pub rule: Option<usize>,
}

impl TermScreen {
    /// 1 行の文字（run の `text` を繋ぎ、行末の空白を落とす）。`tmux capture-pane -p` の既定（`-N` 無し）と同じ切り方。
    pub fn line_text(line: &[TermRun]) -> String {
        let mut s: String = line.iter().map(|r| r.text.as_str()).collect();
        s.truncate(s.trim_end_matches(' ').len());
        s
    }

    /// 今の画面の `rows` 行の文字（空の行は ""）。tmux の受け入れは、これと `tmux capture-pane -p` の行を全行で比べる。
    pub fn visible_rows(&self) -> Vec<String> {
        self.lines
            .iter()
            .skip(self.history)
            .map(|l| Self::line_text(l))
            .collect()
    }

    /// AX の `terminal:<ref>` の値（`TerminalPane::text`）: scrollback と今の画面の全行を `\n` で繋いだ物。末尾の空の行も落とさない
    /// （試験の側は最後の `rows` 行を今の画面として読む）。
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| Self::line_text(l))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---- G95: panel の Terminal の前面の agent の状態（Orca の ✳ / ◐ / ✓ に当たる）----

/// G95: panel の tab 1 つの前面で動く agent の今。tab = icon の角の点（TITLEBAR-ICONS-1101 の 2・色）と hover の語（`label`・色だけで分けない）。
/// 作るのは `AgentWatch::step`（材料は `AgentProbe`）。保存しない（再起動で process は残らない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// 出力が `AgentWatch::IDLE` の内に来ている
    Working,
    /// agent が前面に居て、出力が `IDLE` より長く止まっている = 入力待ち
    NeedsYou,
    /// agent が前面から居なくなった（code = shell ごと終わった時の終了 code・agent だけ終わった時は None）
    Done { code: Option<i32> },
}

/// G95: 1 回の見立ての材料（`TerminalManager::probe` が作る・試験は手で作る = 偽の pgid と偽の時計）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentProbe {
    /// PTY の前面の process group の leader の command 行（`ps -o args=`）。shell 自身が前面・読めない時は None
    pub foreground: Option<String>,
    /// 最後の出力からの時間（まだ 1 byte も来ていなければ None）
    pub since_output: Option<Duration>,
    /// shell が終わった
    pub exited: bool,
    /// shell の終了 code（終わった時だけ）
    pub exit_code: Option<i32>,
    /// USAGE-OWN-1101: 前面の process group の leader の pid（`foreground` と同じ時だけ Some）。Shibubu の Terminal の agent の session を引く
    /// （claude = `~/.claude/sessions/<pid>.json`・grok = `~/.grok/active_sessions.json`・`own_usage::resolve`）
    pub pid: Option<i32>,
    /// TITLEBAR-ICONS-1101 の 4: 中の program が付けた最後の題（OSC 0 / 2・ResetTitle で None・`terminal::vt::Vt::title`）。tab の自動の名前（`tab_title`）の材料
    pub title: Option<String>,
}

/// G95: tab 1 つの agent の見張り（前の状態を持ち、変わり目を返す）。新しい型の理由: 変わり目（Working → Needs you / Done）で通知を 1 回だけ出すには前の状態が要り、
/// `InputLease`（入力の持ち主）とも `SessionStatus`（shell の生死）とも別の軸。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentWatch {
    /// 前面の agent の名前（`agent_name` の答え）。shell に戻っても Done の間は残す
    pub agent: Option<String>,
    pub state: Option<AgentState>,
}

impl AgentState {
    /// tab の hover と帯の語（英語・sentence case）
    pub fn label(&self) -> String {
        match self {
            AgentState::Working => "Working".into(),
            AgentState::NeedsYou => "Needs you".into(),
            AgentState::Done { code: Some(c) } if *c != 0 => format!("Done · exit {c}"),
            AgentState::Done { .. } => "Done".into(),
        }
    }
}

impl AgentWatch {
    /// 出力がこれより長く止まれば `NeedsYou`
    pub const IDLE: Duration = Duration::from_secs(2);

    /// 1 回見立てて状態を進める。返り値 = 通知に値する変わり目（`Working` から `NeedsYou` / `Done` に変わった時だけ・その新しい状態）。
    /// 例: `if let Some(s) = watch.step(&mgr.probe(&key)?) { if !panel.open || !focused { notify(&tab.name, &s) } }`
    pub fn step(&mut self, probe: &AgentProbe) -> Option<AgentState> {
        let tracking = matches!(self.state, Some(AgentState::Working | AgentState::NeedsYou));
        let front = (!probe.exited).then(|| probe.foreground.as_deref().and_then(agent_name)).flatten();
        let next = match front {
            Some(name) => {
                self.agent = Some(name.to_owned());
                let busy = probe.since_output.is_some_and(|d| d < Self::IDLE);
                Some(if busy { AgentState::Working } else { AgentState::NeedsYou })
            }
            // agent が居なくなった（shell ごと終わった時は終了 code）・Done は次の agent まで残す
            None if tracking => Some(AgentState::Done { code: if probe.exited { probe.exit_code } else { None } }),
            None => self.state.clone().filter(|s| matches!(s, AgentState::Done { .. })),
        };
        let was_working = self.state == Some(AgentState::Working);
        let changed = next != self.state;
        self.state = next.clone();
        next.filter(|n| changed && was_working && *n != AgentState::Working)
    }
}

/// TITLEBAR-ICONS-1101 の 2（owner 216「terminalの横の丸はclaudeならclaudeのアイコンcodexならcodexなど検知してもののアイコンが出るように。それはorcaを真似して」）:
/// Terminal の前面で見つける agent 1 種 = 1 か所の表 `AGENT_KINDS` の行。検知の名前（`cmd` と `aliases` = Orca の detectCmd / detectCmdAliases と同じ見方・形は写さない）・
/// 人に見せる名前（tab・左の sidebar の行の hover・Connections）・その provider の Mac の app の bundle id・同梱の brand の logo の key（PROVIDER-ICONS-1101 で logo が出所の 1 番目）。
/// 新しい型の理由: 名前の文字列 1 つ（前の `AGENTS`）では別名・見せる名前・app・icon を持てない。
/// Wave: TITLEBAR-ICONS-1101
#[derive(Debug, PartialEq, Eq)]
pub struct AgentKind {
    /// 打つ command の名前（`+` の menu の行・`AGENTS` の並び）
    pub cmd: &'static str,
    /// 同じ agent の別の command 名（`kimi-code` 等）
    pub aliases: &'static [&'static str],
    /// 人に見せる名前（英語・製品の名前のまま）
    pub label: &'static str,
    /// その provider の Mac の app（bundle id）。logo が無い時の icon（出所の 2 番目・入っていなければ頭文字の丸）
    pub app: Option<&'static str>,
    /// PROVIDER-ICONS-1101（owner 248・249「opencode, ollamaやkiroなども全てicon持ってくるように。抜かりなく。」）: 同梱の brand の logo の key（`domain::icon::logo_png`・
    /// @lobehub/icons-static-svg 1.95.1〈MIT〉を焼いた PNG・出所の 1 番目）。None = 使える logo が無い（aider・droid・crush = app → 頭文字の丸・理由は NOTICE）。
    /// 前の `icon`（Phosphor の OpenAiLogo 等）を替えた理由: Phosphor は UI の icon だけに使い、provider の絵は各社の logo の集まりから取る（spec の 1）。
    pub logo: Option<&'static str>,
}

/// TITLEBAR-ICONS-1101 の 2: 見つける agent の表（Orca が見つける物と同じ並びの程度・前の `AGENTS` の 9 つが先頭 = `+` の menu の順は今まで通り）。
/// Connections（LAYOUT）・左の sidebar の行（224）・panel の tab の icon がこの表を読む。
// ponytail: 名前の表。要る agent が出たら 1 行足す
pub const AGENT_KINDS: [AgentKind; 15] = [
    AgentKind { cmd: "claude", aliases: &[], label: "Claude Code", app: Some("com.anthropic.claudefordesktop"), logo: Some("claude") },
    AgentKind { cmd: "codex", aliases: &[], label: "Codex", app: Some("com.openai.codex"), logo: Some("codex") },
    AgentKind { cmd: "gemini", aliases: &[], label: "Gemini CLI", app: None, logo: Some("gemini") },
    AgentKind { cmd: "grok", aliases: &[], label: "Grok", app: None, logo: Some("grok") },
    AgentKind { cmd: "aider", aliases: &[], label: "Aider", app: None, logo: None },
    AgentKind { cmd: "opencode", aliases: &[], label: "OpenCode", app: None, logo: Some("opencode") },
    AgentKind { cmd: "cursor-agent", aliases: &[], label: "Cursor Agent", app: Some("com.todesktop.230313mzl4w4u92"), logo: Some("cursor-agent") },
    AgentKind { cmd: "amp", aliases: &[], label: "Amp", app: None, logo: Some("amp") },
    AgentKind { cmd: "goose", aliases: &[], label: "Goose", app: None, logo: Some("goose") },
    AgentKind { cmd: "kimi", aliases: &["kimi-code"], label: "Kimi Code", app: None, logo: Some("kimi") },
    AgentKind { cmd: "copilot", aliases: &[], label: "GitHub Copilot", app: None, logo: Some("copilot") },
    AgentKind { cmd: "droid", aliases: &[], label: "Droid", app: None, logo: None },
    AgentKind { cmd: "qwen", aliases: &[], label: "Qwen Code", app: None, logo: Some("qwen") },
    AgentKind { cmd: "crush", aliases: &[], label: "Crush", app: None, logo: None },
    // PROVIDER-ICONS-1101: Kiro CLI（`kiro-cli`・chat の子の process は `kiro-cli-chat`）。Mac の app（Kiro CLI.app）の bundle id は CodeWhisperer の物なので app に書かない
    AgentKind { cmd: "kiro-cli", aliases: &["kiro", "kiro-cli-chat"], label: "Kiro", app: None, logo: Some("kiro") },
];

/// PROVIDER-ICONS-1101（owner 249「ollamaや…全てicon持ってくるように」）: Terminal の前面で見つけて印だけ出す物（local の LLM の CLI）。tab と左の行の icon、
/// provider の印（`AiAccount::provider` の `ollama`・`lm-studio` = `llm_cli::LOCAL_LLMS` の id）に使う（`agent_kind` が `AGENT_KINDS` の次に引く）。
/// `AGENT_KINDS` に入れない理由（operator 11-02 05:1x）: あちらの `AGENTS` は panel の `+` の menu と agent の状態の印（Working / Needs you）の対象で、
/// `ollama` / `lms` は打っても help が出るだけ（会話の前に model を選ぶ）・状態の印も読まない。
pub const LOCAL_KINDS: [AgentKind; 2] = [
    AgentKind { cmd: "ollama", aliases: &[], label: "Ollama", app: None, logo: Some("ollama") },
    AgentKind { cmd: "lms", aliases: &["lm-studio"], label: "LM Studio", app: None, logo: Some("lmstudio") },
];

/// G95: Terminal の前面で agent と読む command の名前（`agent_name`）・L7-ORCA #12: panel の帯の `+` の menu で起こせる agent（入っている物だけ・`llm_cli::agent_clis`）。
/// TITLEBAR-ICONS-1101: `AGENT_KINDS` の `cmd` の並び（表は 1 か所）。
pub const AGENTS: [&str; AGENT_KINDS.len()] = {
    let mut out = [""; AGENT_KINDS.len()];
    let mut i = 0;
    while i < out.len() {
        out[i] = AGENT_KINDS[i].cmd;
        i += 1;
    }
    out
};

/// TITLEBAR-ICONS-1101 の 4: 前面の command 行の process の名前（path の最後・login shell の `-zsh` は `zsh`・`node /x/bin/gemini` の様な interpreter 越しは中の名前）。
/// 例: `process_name("/opt/homebrew/bin/codex exec")` → `Some("codex")`・`process_name("python3 -m http.server")` → `Some("http.server")`・空 → None。
pub fn process_name(args: &str) -> Option<String> {
    const SHIMS: [&str; 9] = ["node", "bun", "deno", "python", "python3", "sh", "bash", "zsh", "env"];
    let base = |w: &str| w.rsplit('/').next().unwrap_or(w).trim_start_matches('-').to_owned();
    let mut words = args.split_whitespace();
    let first = base(words.next()?);
    if SHIMS.contains(&first.as_str()) {
        if let Some(inner) = words.find(|w| !w.starts_with('-')) {
            return Some(base(inner));
        }
    }
    Some(first)
}

/// TITLEBAR-ICONS-1101 の 2: 前面の command 行の agent（`AGENT_KINDS` の行・`cmd` か `aliases` が process の名前と同じ）。agent でなければ None。
/// PROVIDER-ICONS-1101: 次に `LOCAL_KINDS`（ollama・LM Studio = 印だけ）も引く。provider の id（`AiAccount::provider` = `claude`・`grok`・`lm-studio` 等）でも引ける
/// （`theme::provider_icon` がこれで logo を引く）。
/// 例: `agent_kind("node /x/kimi-code --y").map(|k| k.label)` → `Some("Kimi Code")`・`agent_kind("ollama run llama3").map(|k| k.label)` → `Some("Ollama")`。
pub fn agent_kind(args: &str) -> Option<&'static AgentKind> {
    let name = process_name(args)?;
    AGENT_KINDS.iter().chain(&LOCAL_KINDS).find(|k| k.cmd == name || k.aliases.contains(&name.as_str()))
}

/// G95: 前面の command 行から agent の名前（`claude`・`codex`・`gemini` 等 = `AgentKind::cmd`）。`node /x/bin/gemini --y` の様な interpreter 越しも読む。agent でなければ None。
/// PROVIDER-ICONS-1101: `LOCAL_KINDS`（印だけ）は agent の名前にしない（状態の印を読まない）。
/// 例: `agent_name("/opt/homebrew/bin/codex exec")` → `Some("codex")`・`agent_name("vim notes.md")` → None・`agent_name("ollama run llama3")` → None。
pub fn agent_name(args: &str) -> Option<&'static str> {
    agent_kind(args).map(|k| k.cmd).filter(|c| AGENTS.contains(c))
}

/// TITLEBAR-ICONS-1101 の 4（owner 217「terminalの名前もsessionによって自動で名前をつけるようにorcaみたいに」）: tab の自動の名前。
/// `raw` = 中の program が付けた題（OSC 0 / 2・`AgentProbe::title`）・`process` = 前面の process の名前（shell 自身なら shell の名前）・`folder` = 始めた folder の名前・
/// `home` = 利用者の HOME（題の中の HOME は `~` に）。題が在れば題（先頭の `user@host:` と、動きの記号だけの語〈Claude Code の ✳ 等〉は落とす = 録画に名前が映らない・
/// 記号が回っても名前は替わらない）・無ければ「process · folder」。
/// 例: `tab_title(Some("u@box:~/x"), "zsh", "x", "/home/u")` → `"~/x"`・`tab_title(Some("✳ Fix the login"), "claude", "x", "")` → `"Fix the login"`・
/// `tab_title(None, "zsh", "app", "")` → `"zsh · app"`。
pub fn tab_title(raw: Option<&str>, process: &str, folder: &str, home: &str) -> String {
    let raw = raw.map(|t| if home.len() > 1 { t.replace(home, "~") } else { t.to_owned() });
    match raw.as_deref().map(strip_user_host).map(strip_spinner).filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None if folder.is_empty() => process.to_owned(),
        None => format!("{process} · {folder}"),
    }
}

/// 題の先頭の `user@host`（`:` か空白まで）を落とす。最初の語に `@` が無ければそのまま。
fn strip_user_host(t: &str) -> &str {
    let t = t.trim();
    let word_end = t.find(char::is_whitespace).unwrap_or(t.len());
    let Some(at) = t[..word_end].find('@') else { return t };
    let after = &t[at + 1..];
    let host_end = after.find(|c: char| c == ':' || c.is_whitespace()).unwrap_or(after.len());
    after[host_end..].trim_start_matches(':').trim()
}

/// 題の先頭の、字も数も無い記号だけの語（`✳`・`⠋` 等の動きの印）を落とす（`~` と `/` と `.` で始まる path は残す）。
fn strip_spinner(t: &str) -> &str {
    match t.split_once(' ') {
        Some((head, rest)) if !head.starts_with(['~', '/', '.']) && head.chars().all(|c| !c.is_alphanumeric()) => rest.trim(),
        _ => t,
    }
}

// ---- W11: 入力の lease（YCM-026・061）----

/// Terminal の入力を誰が書いているか。`surface::FocusOwner` は Portal の focus（None を含む）で、こちらは入力の持ち手だけ。
///
/// 使い方: `lease.may_write(InputOwner::Agent, &view)?; mgr.write_as(key, InputOwner::Agent, &view, bytes)?`
/// 保証する: 判定は `InputLease::may_write` 1 か所。人が持っている間の Agent は `LeaseDenied::HumanOwns`（PTY に byte を足さない）。
/// 保証しない: 人が打ち終わった事の検出。それは `InputLease::may_write_after`（最後の人の入力から `HUMAN_HOLD`）。
/// Wave: W11
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputOwner {
    Human,
    Agent,
}

/// 同じ session を出す場所（YCM-026 の view）。Page の席と W8 の Drawer が同時に書く時、持ち主の view だけが通る。
///
/// 使い方: Page から `InputView::Page { page_id }`、引き出しから `InputView::Drawer`。
/// 保証しない: 同じ Page を 2 枠に出した時の調停（MVP は 1 Page に席 1 つ）。
/// Wave: W11
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputView {
    Page { page_id: String },
    Drawer,
}

/// session 1 つの今の入力の持ち主。保存しない（再起動で PTY が消えるので lease も消える）。RunReceipt に写して残す。
///
/// 使い方: `let mut l = InputLease::idle(Some("term.dev".into()), now); l.apply_write(InputOwner::Human, view, now)?;`
/// 保証する: `may_write` の表（下）。surface_id は W19 が GuiAdapter の拒否に使う（ここの lease の surface の持ち主）。
/// 保証しない: PTY に書けた事（それは `TerminalManager::write_as`）。人の lease の時間切れは `may_write_after`（`HUMAN_HOLD`）が決める。
/// Wave: W11
///
/// 判定（`may_write`）:
/// - holder 無し → Human も Agent も可
/// - Human が持っている → 同じ view の Human だけ可。Agent は `HumanOwns`。別 view の Human は `OtherView`
/// - Agent が持っている → 同じ view の Agent は可。別 view の Agent は `OtherView`。Human はどの view からでも可（人は取り戻せる）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputLease {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<InputOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<InputView>,
    /// W19 が使う。この lease が紐づく surface（Terminal の面の id）。面が無ければ None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    /// RFC3339 UTC。最後に持ち手が変わった時刻。
    pub since: String,
}

/// `InputLease::may_write` の拒否。Display は Terminal の下に出す 1 行（D-e）。
/// Wave: W11
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseDenied {
    /// 人が打っている間の Agent（YCM-061）。
    HumanOwns,
    /// 別の view が持っている（YCM-026）。
    OtherView,
}

impl std::fmt::Display for LeaseDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HumanOwns => write!(
                f,
                "人が Terminal に打っている間は、AI の入力は送りません。打つのを止めて {} 秒たつと送れます",
                InputLease::HUMAN_HOLD.as_secs()
            ),
            Self::OtherView => write!(f, "この Terminal は他の画面が打っています。打ち終わってからにしてください"),
        }
    }
}
impl std::error::Error for LeaseDenied {}

impl InputLease {
    /// 誰も打っていない。Agent も Human も書いてよい。
    pub fn idle(surface_id: Option<String>, since: String) -> Self {
        Self {
            holder: None,
            view: None,
            surface_id,
            since,
        }
    }

    /// この owner / view が書いてよいか。Ok なら PTY に足してよい。Err なら byte は 0。
    pub fn may_write(&self, owner: InputOwner, view: &InputView) -> Result<(), LeaseDenied> {
        match self.holder {
            None => Ok(()),
            Some(InputOwner::Human) if owner == InputOwner::Agent => Err(LeaseDenied::HumanOwns),
            Some(InputOwner::Human) if self.view.as_ref() != Some(view) => Err(LeaseDenied::OtherView),
            Some(InputOwner::Agent) if owner == InputOwner::Agent && self.view.as_ref() != Some(view) => {
                Err(LeaseDenied::OtherView)
            }
            Some(_) => Ok(()),
        }
    }

    /// `may_write` が Ok なら持ち手を更新する。Err なら self は変えない。
    pub fn apply_write(&mut self, owner: InputOwner, view: InputView, since: String) -> Result<(), LeaseDenied> {
        self.may_write(owner, &view)?;
        self.holder = Some(owner);
        self.view = Some(view);
        self.since = since;
        Ok(())
    }

    /// 人の lease が続く時間。最後の人の入力（`write_as(Human)`・`touch_human`）からこれだけ人が何もしなければ、Agent も書ける。
    /// 「人が打っている間」（W11）は UI の focus ではなくこの時間で決める: 8405a44 の実機で「走らせる」を押すと同じ frame で
    /// 欄の focus が外れて Human を離し、focus で離す作りでは製品の口から「人の入力の間の AI」を作れなかった。
    /// 長さは W19-1 の Native Direct の人の lease（値の変化から 5 秒）と揃える。
    ///
    /// 本体（W11）の約束: renderer は `lost_focus` で `release_human` を呼ばない（時間で切れる）。`HumanOwns` の 1 行に
    /// 「打つのを止めて 5 秒たつと送れます」の次の一手を足す（D-e）。`release_human` は Page / Terminal を閉じる時の明示の口として残す。
    /// Wave: W11
    pub const HUMAN_HOLD: Duration = Duration::from_secs(5);

    /// `may_write` に人の lease の時間切れを足した判定。manager の `write_as` / `run_marked` はこれで決める（本体 W11）。
    ///
    /// 使い方: `lease.may_write_after(InputOwner::Agent, &view, last_human.elapsed())?`（`last_human` は manager が session ごとに `Instant` で持つ。`since` の文字列は表示と receipt 用）
    /// 保証する: Human が持っていて `human_idle >= HUMAN_HOLD` なら holder 無しと同じ（誰でも可）。それ以外は `may_write` と同じ表。純関数。
    /// 保証しない: Agent の持ち手の時間切れ（Agent は時間で切れない・`may_write` と同じ）。
    /// Wave: W11
    pub fn may_write_after(&self, owner: InputOwner, view: &InputView, human_idle: Duration) -> Result<(), LeaseDenied> {
        if self.holder == Some(InputOwner::Human) && human_idle >= Self::HUMAN_HOLD {
            return Ok(());
        }
        self.may_write(owner, view)
    }

    /// この view の人が離した。持っている view が同じ時だけ idle に戻す（他の view の lease は触らない）。
    /// 戻したら true。
    pub fn release_human(&mut self, view: &InputView, since: String) -> bool {
        if self.holder == Some(InputOwner::Human) && self.view.as_ref() == Some(view) {
            self.holder = None;
            self.view = None;
            self.since = since;
            true
        } else {
            false
        }
    }
}

// ---- W11: RunReceipt（YCM-027・065）----

/// Terminal で走らせた command 1 回の記録（表 `run_receipts` の json・Tests Page が読む）。
///
/// 使い方: `let r = RunReceipt::from_output(id, project, session, repo, cmd, out.output, out.exit_code, &lease, t0, t1); r.validate()?; store.put_run_receipt(...)?`
/// 保証する: `succeeded` は `exit_code == 0` だけ。`status_line` は 0 以外を「成功」と出さない（YCM-065）。5 欄（run_id・時刻・入力／出力・repo・exit_code）は `validate` が空を拒否。
/// 保証しない: stdout と stderr を PTY が分けた事（混ざる。stderr は空でもよい）。command が本当に cargo test である事（任意の command）。
/// `exit_code` は走らせた command の `$?`（shell が印に書いた値）で、Shibubu の process の終わり（panic の 101 を含む）は入らない。
/// 印が返らなければ receipt は作らない（`RunState::Unfinished`）。8405a44 の 101 は `cargo test` の test の失敗の code で、
/// その直後に .app が消えたのは receipt に出ない別の事象（W11 本体で再現して直す）。
/// Wave: W11
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReceipt {
    pub run_id: String,
    pub project_id: String,
    pub session_id: String,
    /// 参照 Repo（作業 dir か git remote）。
    pub repo: String,
    /// 走らせた command（入力）。
    pub input: String,
    /// PTY に出た byte（stdout と stderr は混ざる）。
    pub stdout: String,
    /// 分けて取れた stderr。PTY では空。
    pub stderr: String,
    pub exit_code: i32,
    /// RFC3339 UTC。
    pub started_at: String,
    pub finished_at: String,
    pub lease_holder: InputOwner,
    /// lease の surface の持ち主。面が無ければ None。W11 の受け入れ「lease の surface の持ち主が receipt に残る」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
}

/// `RunReceipt::validate` の拒否。画面と log に出す 1 行は Display。
/// Wave: W11
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunReceiptError {
    /// 必須の欄が空（欄の名前）。
    MissingField(&'static str),
}

impl std::fmt::Display for RunReceiptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(n) => write!(f, "Run の記録に '{n}' がありません。もう一度走らせてください"),
        }
    }
}
impl std::error::Error for RunReceiptError {}

impl RunReceipt {
    /// `CommandOutput` から作る（terminal の型は出さない）。lease の持ち手が None の時は Agent（command を送った側の既定）。
    pub fn from_output(
        run_id: String,
        project_id: String,
        session_id: String,
        repo: String,
        input: String,
        output: String,
        exit_code: i32,
        lease: &InputLease,
        started_at: String,
        finished_at: String,
    ) -> Self {
        Self {
            run_id,
            project_id,
            session_id,
            repo,
            input,
            stdout: output,
            stderr: String::new(),
            exit_code,
            started_at,
            finished_at,
            lease_holder: lease.holder.unwrap_or(InputOwner::Agent),
            surface_id: lease.surface_id.clone(),
        }
    }

    /// 成功 = exit_code == 0 だけ。別の `ok` 欄は持たない（失敗を成功と出せない）。
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0
    }

    /// Tests Page の 1 行。exit_code == 0 の時だけ「成功」。0 以外は「失敗」（exit code は画面の別欄）。
    pub fn status_line(&self) -> String {
        if self.succeeded() {
            "成功".into()
        } else {
            "失敗".into()
        }
    }

    /// 保存の前。YCM-065 の 5 欄（run_id・時刻・入力／出力・repo・exit_code）と session / project が空なら拒否。
    /// stdout / stderr の空は許す（出力の無い command）。exit_code は i32 なので空にならない。
    pub fn validate(&self) -> Result<(), RunReceiptError> {
        for (name, v) in [
            ("run_id", self.run_id.as_str()),
            ("project_id", self.project_id.as_str()),
            ("session_id", self.session_id.as_str()),
            ("repo", self.repo.as_str()),
            ("input", self.input.as_str()),
            ("started_at", self.started_at.as_str()),
            ("finished_at", self.finished_at.as_str()),
        ] {
            if v.is_empty() {
                return Err(RunReceiptError::MissingField(name));
            }
        }
        Ok(())
    }
}

/// Tests Page の「走らせる」の今（receipt の一覧の上に出す 1 行）。終わって印が返った run は `RunReceipt` で、ここには無い。
///
/// 使い方: `match g.run_state(project) { Some(RunState::Running { command, .. }) => line(format!("走らせています（{command}）")), Some(RunState::Unfinished { line, .. }) => error(line), None => {} }`
/// 保証する: test の失敗（receipt・`exit_code != 0`・「失敗」）と、結果を取れなかった終わり方（`Unfinished`・receipt 無し）を混ぜない。
/// 保証しない: 保存（再起動で消える。走っていた run は印が返らないので receipt も無い）。
/// Wave: W11
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// 印を待っている。`started_at` は RFC3339 UTC。
    Running { command: String, started_at: String },
    /// 印が返らずに終わった（時間切れ・shell が終わった・保存できない）。`line` は何が起きたか + 次の一手（D-e）で、「成功」「失敗」を含まない。
    /// 次の `start_run` まで残る。
    Unfinished { command: String, line: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(col: u16, text: &str) -> TermRun {
        TermRun {
            col,
            text: text.into(),
            style: TermStyle::default(),
        }
    }

    /// 行末の空白だけを落とし（行の中と行頭の空白は残す）、history の行は今の画面に数えない。
    #[test]
    fn w10_rows_trim_only_trailing_spaces_and_skip_history() {
        let red = TermStyle {
            fg: TermColor::Indexed(1),
            ..TermStyle::default()
        };
        let s = TermScreen {
            rows: 3,
            cols: 10,
            history: 1,
            lines: vec![
                vec![run(0, "old")],
                vec![
                    run(0, "  a b  "),
                    TermRun {
                        col: 7,
                        text: "x ".into(),
                        style: red,
                    },
                ],
                vec![],
                vec![run(0, "全角"), run(4, "!")],
            ],
            ..TermScreen::default()
        };
        assert_eq!(s.visible_rows(), vec!["  a b  x", "", "全角!"]);
        assert_eq!(s.text(), "old\n  a b  x\n\n全角!");
    }

    fn view(page: &str) -> InputView {
        InputView::Page { page_id: page.into() }
    }

    fn lease(holder: Option<InputOwner>, v: Option<InputView>) -> InputLease {
        InputLease {
            holder,
            view: v,
            surface_id: Some("term.dev".into()),
            since: "2026-10-22T00:00:00Z".into(),
        }
    }

    /// W11 の判定表。人が持っている間の Agent と、別 view の同時書き込みだけを拒む。
    #[test]
    fn w11_lease_decision_table() {
        let a = view("page.a");
        let b = view("page.b");
        let idle = lease(None, None);
        assert!(idle.may_write(InputOwner::Human, &a).is_ok());
        assert!(idle.may_write(InputOwner::Agent, &a).is_ok());
        let human_a = lease(Some(InputOwner::Human), Some(a.clone()));
        assert!(human_a.may_write(InputOwner::Human, &a).is_ok());
        assert_eq!(human_a.may_write(InputOwner::Agent, &a), Err(LeaseDenied::HumanOwns));
        assert_eq!(human_a.may_write(InputOwner::Agent, &b), Err(LeaseDenied::HumanOwns));
        assert_eq!(human_a.may_write(InputOwner::Human, &b), Err(LeaseDenied::OtherView));
        assert_eq!(human_a.may_write(InputOwner::Human, &InputView::Drawer), Err(LeaseDenied::OtherView));
        let agent_a = lease(Some(InputOwner::Agent), Some(a.clone()));
        assert!(agent_a.may_write(InputOwner::Agent, &a).is_ok());
        assert_eq!(agent_a.may_write(InputOwner::Agent, &b), Err(LeaseDenied::OtherView));
        assert!(agent_a.may_write(InputOwner::Human, &a).is_ok(), "人は Agent から取り戻せる");
        assert!(agent_a.may_write(InputOwner::Human, &b).is_ok(), "人は別 view からでも取り戻せる");
        let mut l = idle;
        l.apply_write(InputOwner::Human, a.clone(), "t1".into()).unwrap();
        assert_eq!(l.holder, Some(InputOwner::Human));
        assert_eq!(l.apply_write(InputOwner::Agent, a.clone(), "t2".into()), Err(LeaseDenied::HumanOwns));
        assert_eq!(l.holder, Some(InputOwner::Human), "拒否のあと持ち手は変わらない");
        assert!(l.release_human(&a, "t3".into()));
        assert_eq!(l.holder, None);
        assert!(!l.release_human(&a, "t4".into()));
    }

    /// W11: 人の lease は最後の人の入力から `HUMAN_HOLD` ちょうどで切れる（その 1ms 前は切れない）。Agent の持ち手は時間で切れない。
    #[test]
    fn w11_human_hold_expires_only_after_idle() {
        let (a, b) = (view("page.a"), view("page.b"));
        let human_a = lease(Some(InputOwner::Human), Some(a.clone()));
        let hold = InputLease::HUMAN_HOLD;
        let before = hold - Duration::from_millis(1);
        assert_eq!(human_a.may_write_after(InputOwner::Agent, &a, before), Err(LeaseDenied::HumanOwns));
        assert_eq!(human_a.may_write_after(InputOwner::Human, &b, before), Err(LeaseDenied::OtherView));
        assert!(human_a.may_write_after(InputOwner::Agent, &a, hold).is_ok());
        assert!(human_a.may_write_after(InputOwner::Human, &b, hold).is_ok());
        let agent_a = lease(Some(InputOwner::Agent), Some(a.clone()));
        assert_eq!(agent_a.may_write_after(InputOwner::Agent, &b, Duration::from_secs(3600)), Err(LeaseDenied::OtherView));
    }

    fn receipt(exit: i32) -> RunReceipt {
        RunReceipt::from_output(
            "run-1".into(),
            "project.x".into(),
            "dev".into(),
            "/repo".into(),
            "cargo test --lib w11_fail".into(),
            "test w11_fail ... FAILED".into(),
            exit,
            &lease(Some(InputOwner::Agent), Some(view("page.tests"))),
            "2026-10-22T00:00:00Z".into(),
            "2026-10-22T00:00:01Z".into(),
        )
    }

    /// 失敗の run を成功と出さない（YCM-065）。exit 0 だけが「成功」。
    #[test]
    fn w11_run_receipt_failed_is_not_success() {
        let fail = receipt(1);
        assert!(!fail.succeeded());
        assert_eq!(fail.status_line(), "失敗");
        assert!(!fail.status_line().contains("成功"));
        assert!(!fail.status_line().contains("exit"));
        assert_eq!(fail.lease_holder, InputOwner::Agent);
        assert_eq!(fail.surface_id.as_deref(), Some("term.dev"));
        fail.validate().unwrap();
        let ok = receipt(0);
        assert!(ok.succeeded());
        assert_eq!(ok.status_line(), "成功");
        assert!(!ok.status_line().contains("exit"));
        let json = serde_json::to_value(&fail).unwrap();
        let back: RunReceipt = serde_json::from_value(json).unwrap();
        assert_eq!(back, fail);
    }

    #[test]
    fn w11_run_receipt_validate_rejects_empty_fields() {
        for (name, r) in [
            ("run_id", {
                let mut r = receipt(1);
                r.run_id.clear();
                r
            }),
            ("repo", {
                let mut r = receipt(1);
                r.repo.clear();
                r
            }),
            ("input", {
                let mut r = receipt(1);
                r.input.clear();
                r
            }),
            ("started_at", {
                let mut r = receipt(1);
                r.started_at.clear();
                r
            }),
        ] {
            assert_eq!(r.validate(), Err(RunReceiptError::MissingField(name)), "{name}");
        }
        let empty_out = receipt(101);
        let mut empty_out = empty_out;
        empty_out.stdout.clear();
        empty_out.stderr.clear();
        empty_out.validate().unwrap();
        assert_eq!(empty_out.status_line(), "失敗");
    }

    /// G95: 偽の時計（since_output）と偽の前面（foreground）で状態を進める。通知に値するのは Working → Needs you / Done の 1 回だけ。
    /// 変異: `step` の `since < IDLE` を `<=` 以外に崩す（常に Working）→ Needs you に変わらず赤・変わり目の返しを毎回にする → 2 回目の Needs you で Some が返り赤。
    #[test]
    fn g95_agent_watch_turns_working_into_needs_you_once() {
        let probe = |fg: Option<&str>, secs: u64| AgentProbe { foreground: fg.map(str::to_owned), since_output: Some(Duration::from_secs(secs)), exited: false, exit_code: None, pid: None, title: None };
        let mut w = AgentWatch::default();
        assert_eq!(w.step(&probe(None, 0)), None, "shell だけは agent でない");
        assert_eq!(w.state, None);
        assert_eq!(w.step(&probe(Some("claude"), 0)), None, "動き始めは通知しない");
        assert_eq!(w.state, Some(AgentState::Working));
        assert_eq!(w.step(&probe(Some("claude"), 3)), Some(AgentState::NeedsYou), "止まった = 入力待ちの 1 回");
        assert_eq!(w.step(&probe(Some("claude"), 5)), None, "同じ状態では 2 度目を返さない");
        assert_eq!(w.step(&probe(Some("claude"), 0)), None);
        assert_eq!(w.step(&probe(None, 0)), Some(AgentState::Done { code: None }), "agent が前面から居なくなった");
        assert_eq!(w.state, Some(AgentState::Done { code: None }));
        assert_eq!(w.step(&probe(None, 9)), None, "Done は次の agent まで残る");
        let ended = AgentProbe { exited: true, exit_code: Some(3), ..probe(Some("codex"), 0) };
        let mut v = AgentWatch { agent: Some("codex".into()), state: Some(AgentState::Working) };
        assert_eq!(v.step(&ended), Some(AgentState::Done { code: Some(3) }), "shell ごと終わった時は終了 code");
    }

    /// G95: 前面の command 行から agent の名前（interpreter 越しも）。変異: interpreter の飛ばしを外す → node の行が None で赤。
    #[test]
    fn g95_agent_name_reads_the_front_command() {
        assert_eq!(agent_name("claude"), Some("claude"));
        assert_eq!(agent_name("/opt/homebrew/bin/codex exec --full-auto"), Some("codex"));
        assert_eq!(agent_name("node /usr/local/lib/node_modules/@google/gemini-cli/bin/gemini"), Some("gemini"));
        assert_eq!(agent_name("vim notes.md"), None);
        assert_eq!(agent_name(""), None);
        // L7-AGENT: grok（Homebrew の bin・node 越し）
        assert_eq!(agent_name("/usr/local/bin/grok"), Some("grok"));
        assert_eq!(agent_name("node /usr/local/lib/node_modules/@vibe-kit/grok-cli/dist/grok --model x"), Some("grok"));
        assert_eq!(AgentState::NeedsYou.label(), "Needs you");
        assert_eq!(AgentState::Done { code: Some(1) }.label(), "Done · exit 1");
    }

    /// TITLEBAR-ICONS-1101 の 2・4: 別名でも表の行（kimi-code → Kimi Code）・`AGENTS` は表の `cmd` の並び・tab の自動の名前は題（`user@host` と動きの記号を落とす・HOME は ~）
    /// か「process · folder」。負の対照: `aliases` を見ない → kimi-code が None で赤 / `strip_user_host` を外す → 名前が映って赤 / `strip_spinner` を外す → ✳ が残って赤。
    #[test]
    fn icons_agent_kinds_and_tab_titles() {
        assert_eq!(agent_kind("node /usr/local/bin/kimi-code --yolo").map(|k| k.label), Some("Kimi Code"));
        assert_eq!(agent_name("claude --resume"), Some("claude"));
        assert_eq!(AGENTS[..3], ["claude", "codex", "gemini"]);
        assert_eq!(process_name("-zsh").as_deref(), Some("zsh"));
        assert_eq!(tab_title(Some("u@box:~/x"), "zsh", "x", ""), "~/x");
        assert_eq!(tab_title(Some("u@box /home/u/app"), "zsh", "app", "/home/u"), "~/app");
        assert_eq!(tab_title(Some("✳ Fix the login bug"), "claude", "app", ""), "Fix the login bug");
        assert_eq!(tab_title(Some("  "), "zsh", "app", ""), "zsh · app");
        assert_eq!(tab_title(None, "zsh", "", ""), "zsh");
        assert_eq!(tab_title(Some("~/x"), "zsh", "x", ""), "~/x", "path の ~ を記号と読んで落とした");
    }

    /// PROVIDER-ICONS-1101: 表の logo は全部同梱の PNG が在る・kiro は `kiro-cli` と chat の子の process で見つかる・ollama は印だけ（agent の名前にしない）・
    /// provider の id（`AiAccount::provider`）でも logo を引ける。
    /// 負の対照（未実行・owner 186 / 187）: `logo_png` の表から kiro の行を消す → 1 つ目の assert が赤・`agent_name` の filter を外す → ollama の assert が赤。
    #[test]
    fn provider_icons_1101_every_logo_is_bundled_and_local_llms_are_icons_only() {
        for k in AGENT_KINDS.iter().chain(&LOCAL_KINDS) {
            if let Some(key) = k.logo {
                assert!(crate::domain::icon::logo_png(key).is_some(), "{} の logo {key} が同梱されていない", k.cmd);
            }
        }
        assert_eq!(agent_kind("/opt/homebrew/bin/kiro-cli chat").map(|k| k.label), Some("Kiro"));
        assert_eq!(agent_kind("kiro-cli-chat --resume").map(|k| k.label), Some("Kiro"));
        assert_eq!((agent_kind("ollama run llama3").map(|k| k.label), agent_name("ollama run llama3")), (Some("Ollama"), None));
        assert_eq!(agent_name("kiro-cli chat"), Some("kiro-cli"), "kiro は + の menu と状態の印の対象");
        for id in ["claude", "grok", "kimi", "opencode", "cursor-agent", "ollama", "lm-studio"] {
            assert!(agent_kind(id).and_then(|k| k.logo).is_some(), "provider {id} に logo が無い");
        }
    }
}
