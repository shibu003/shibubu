//! TerminalManager — Project が持つ本物の PTY（YCM-020・021・028・SPEC v15 §3〜§6）。画面なし（W4）。表示は W5、VT は W10、lease は W11。
//! W10: session ごとに `Vt`（`vt.rs`・alacritty_terminal）を 1 つ持ち、reader の thread が食わせる。Page の表示は `TerminalManager::screen` の写し
//! （`domain::terminal::TermScreen`・色・cursor・alt screen）で、UI の frame は parse しない。W5 の plain text の写し（`OutputView`）は W10 で消した。
//! この module が PTY の唯一の持ち主。Page は session を参照するだけで、Page を外しても shell は止まらない（SPEC §4）。
//! 起こし方（`TerminalSessionSpec`）は Project の定義に、実行の記録（`SessionInfo`）は `terminal_sessions` の表に置く。PTY の handle は保存しない。
//! 製品の code から cua を引かない（D-a: E2E が `mod cua` 無しでこの file を取り込む）。`crate::` で引くのは E2E が `#[path]` で持つ module だけ。
#![allow(dead_code)] // resize・interrupt・read_since・close_project は W5（Page の表示）と Project を閉じる口から呼ぶ。今は test だけ

pub mod vt; // W10: VT（`vt::Vt`）。session ごとに 1 つ（`Session::vt`）

use crate::domain::project::TerminalSessionSpec;
use crate::domain::terminal::{AgentProbe, InputLease, InputOwner, InputView, LeaseDenied, RunReceipt, TermScreen};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 子の shell に渡す PATH の system の部分（Finder から起こした app の PATH は短いので明示する）。git は /usr/bin か /opt/homebrew/bin に在る。
/// 製品の既定は `seed_path`（これに利用者の dir を足した物）で、`-l -i` の shell が login の profile と rc でさらに足す（利用者の環境。自分の道具の dir を足す rc もある）。
/// E2E の不在 / 罠の通し（YC_WAVES §1）は自分の PATH（cua-driver の dir を外した物 / 罠が先頭の物）を `TerminalConfig::child_path` に渡し、rc は読まない（`-f`）。
pub const DEFAULT_CHILD_PATH: &str =
    "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin";

/// G99: login shell を引く前の種の PATH（Orca `src/main/startup/configure-process.ts:123-175` の写し・stablyai/orca ff6a3755a・MIT）。
/// `DEFAULT_CHILD_PATH` の後ろに、利用者が道具を入れる dir（homebrew の sbin・nix・`~/bin`・`~/.local/bin`・opencode・vite-plus・grok・MacPorts は在る時だけ）。
/// login shell が読めない時も、Terminal と CLI の在りか探し（`llm_cli::login_path`）はこれで探す。
pub fn seed_path(home: &str) -> String {
    let mut dirs: Vec<String> = DEFAULT_CHILD_PATH.split(':').map(str::to_string).collect();
    dirs.extend(["/opt/homebrew/sbin", "/usr/local/sbin", "/nix/var/nix/profiles/default/bin"].map(str::to_string));
    if !home.is_empty() {
        dirs.extend([".nix-profile/bin", "bin", ".local/bin", ".opencode/bin", ".vite-plus/bin", ".grok/bin"].map(|d| format!("{home}/{d}")));
    }
    if Path::new("/opt/local/bin/port").exists() {
        dirs.push("/opt/local/bin".into());
    }
    dirs.join(":")
}

/// W9: 子の shell に渡す、その Terminal が属する Project の id（= `TerminalSessionSpec::project_id`）。`shibubu project …` が読む Project はこれだけ。
pub const PROJECT_ENV: &str = "SHIBUBU_PROJECT_ID";

/// W9: 子の shell に渡す保存先（`kangae.sqlite3` の在る dir の絶対 path）。GUI の `app.rs` が読む名前と同じ。
pub const DATA_DIR_ENV: &str = "SHIBUBU_DATA_DIR";

/// L7-AGENT: 子の shell に渡す、その Terminal の id（= `TerminalSessionSpec::id`）。`shibubu project ask` が「誰が頼んだか」に書く。
pub const TERMINAL_ENV: &str = "SHIBUBU_TERMINAL_ID";

/// SHIBUBU-SKILL 段 2: 子の shell に渡す、Terminal の agent への案内の 1 行（値 = `SKILL_HINT`）。`config.project_cli` が Some の時だけ
/// （PATH に `shibubu` が無い時に「打て」と言わない）。SHIBUBU_DATA_DIR にも workspace にも file は置かない（RULE-043・利用者の repo を汚さない）。
pub const SKILL_HINT_ENV: &str = "SHIBUBU_SKILL_HINT";

/// 段 2: 案内の 1 行（env の値）。初めての人が読んで次の一手が分かる英語。agent には届かない（env を読む agent だけ）。
/// agent に届けるのは利用者が押す「Install CLI & skill」で入れる stub（`SKILL_STUB`・`AgentSetup`）。
pub const SKILL_HINT: &str = "Shibubu: this terminal belongs to the project. Run \"shibubu skills get shibubu-cli\" to see what you and your agent can do here.";

// ---- 段 2 作り直し（owner 142・Orca の部品 2 と 5・図 §31）: Install CLI & skill ----

/// 利用者が選んだ agent の skills の置き場に入れる短い stub。本文は持たず `shibubu skills get shibubu-cli` を読ませる（app と案内の版がずれない）。
pub const SKILL_STUB: &str = include_str!("../../docs/skills/shibubu-cli/SKILL.md");

/// stub を入れる agent（名前 = PATH の上の実行 file・`domain::terminal::agent_name` の表の字）と HOME の下の skills の置き場。stub は `<置き場>/shibubu-cli/SKILL.md`。
/// 出典（2026-10-31 に確かめた）: claude = Claude Code の personal skills `~/.claude/skills`・codex = https://developers.openai.com/codex/skills の USER 行
/// `$HOME/.agents/skills`（`~/.codex/skills` は docs に無い旧い置き場）・gemini = gemini-cli の docs/cli/skills.md の User skills `~/.gemini/skills`・
/// grok = grok 1.0.70 の binary の字 `~/.grok/skills/`。
/// 保証しない: `CLAUDE_CONFIG_DIR` / `GROK_HOME` を替えた agent（Shibubu は GUI の env しか知らない）・`~/.agents/skills` を読む他の agent（cursor 等）にも届く事。
pub const SKILL_AGENTS: [(&str, &str); 4] = [("claude", ".claude/skills"), ("codex", ".agents/skills"), ("gemini", ".gemini/skills"), ("grok", ".grok/skills")];

/// Shibubu の Terminal の外でも `shibubu` が見つかる link（HOME の下 = admin が要らない・`/usr/local/bin` は作らない）。先は `AgentSetup::exe`。
pub const CLI_LINK: &str = ".local/bin/shibubu";

/// 置き場 1 つの状態。中身が同じなら何もしない・違えば利用者が checkbox で選ぶまで書かない（`.bak` は残さない）。
/// 新しくした理由: Shibubu が保存先の外（利用者の HOME）に置いた file が「Shibubu の物と同じか」を持つ既存の型が無い（`SessionStatus` は shell・`RunState` は run）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupState {
    NotInstalled,
    /// 中身が `SKILL_STUB` と同じ・link の先が `exe`
    Installed,
    /// 同じ名前の別の物が在る（利用者の file・前の版の stub・別の Shibubu への link）
    Different,
    /// 書こうとして失敗した（行に出す 1 行・次に読み直すまで残す）
    Failed(String),
}

/// 一覧の 1 行（agent の stub か、`shibubu` の link）。
/// 新しくした理由: 置き場・状態・見つかったか・利用者の選び（checkbox）を一緒に持つ既存の型が無い（`ProjectCli` は子の env の設定で `TerminalConfig` の中で比べる値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRow {
    /// None = `shibubu` の link（`CLI_LINK`）・Some = その agent の stub
    pub agent: Option<&'static str>,
    /// 絶対 path（HOME の下）
    pub path: std::path::PathBuf,
    pub state: SetupState,
    /// agent の CLI がこの Mac で見つかった（link の行は常に true）
    pub found: bool,
    /// 入れる（checkbox）。既定 = 見つかって未導入の agent と未導入の link は on・Different と見つからない agent は off（利用者が選ぶ）
    pub on: bool,
}

/// 「Install CLI & skill」の部品（Orca の部品 2 = 利用者が選んだ agent に stub・部品 5 = CLI を PATH に）。Terminal の panel の 1 行目と Settings › Terminal が同じ物を描く。
/// 使い方: `let mut s = AgentSetup::new(home, cli.bin_dir.join("shibubu")); s.set_found(&["claude"]); s.rows[1].on = false; s.install(); s.line()`
/// 保証する: `new` / `read` / `set_found` / `line` は何も書かない（押されるまで HOME に 1 byte も増やさない）・`install` は `on` の行だけ・中身が同じなら書かない（mtime も同じ）・
/// `remove` は `Installed` の行だけ（stub の file・空になった `shibubu-cli/`・link）を消し、agent の他の skill に触らない。
/// 保証しない: 読んだ後に外で変わった事（次の `read` まで前の状態）・`~/.local/bin` が利用者の shell の PATH に在る事（一覧が 1 行で言う）。
/// 新しくした理由: 利用者の HOME と link の先と UI の開閉を一緒に持つ既存の型が無い（`ProjectCli` は子の env・`TerminalPanel` は tab の並びで保存する値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSetup {
    /// 利用者の HOME（製品 = env の HOME・試験 = fixture。owner の HOME には試験で書かない）
    pub home: std::path::PathBuf,
    /// link の先（.app = `Contents/MacOS/shibubu`・`ProjectCli::bin_dir` の `shibubu`）
    pub exe: std::path::PathBuf,
    /// この Mac で見つかった agent の CLI（`set_found`・`read` が `on` の既定に使う）
    pub found: Vec<&'static str>,
    /// 最後に読んだ一覧（`SKILL_AGENTS` の順 + 最後に link）
    pub rows: Vec<SetupRow>,
    /// Terminal の panel の 1 行目が一覧に変わっている（button を押した〜Install / Cancel）
    pub open: bool,
}

impl AgentSetup {
    /// 読むだけ（`read`）。見つかった agent はまだ無い（`set_found` で）。
    pub fn new(home: std::path::PathBuf, exe: std::path::PathBuf) -> Self {
        let mut s = AgentSetup { home, exe, found: Vec::new(), rows: Vec::new(), open: false };
        s.read();
        s
    }

    /// 一覧を disk から読み直す（読むだけ）。`on` は既定に戻す・`Failed` も読み直した状態に。
    pub fn read(&mut self) {
        let mut rows: Vec<SetupRow> = SKILL_AGENTS
            .iter()
            .map(|&(agent, dir)| {
                let path = self.home.join(dir).join("shibubu-cli/SKILL.md");
                let state = setup_state_of(Some(agent), &path, &self.exe);
                let found = self.found.contains(&agent);
                SetupRow { agent: Some(agent), on: found && state == SetupState::NotInstalled, path, state, found }
            })
            .collect();
        let path = self.home.join(CLI_LINK);
        let state = setup_state_of(None, &path, &self.exe);
        rows.push(SetupRow { agent: None, on: state == SetupState::NotInstalled, path, state, found: true });
        self.rows = rows;
    }

    /// この Mac で見つかった agent の CLI（app.rs が `llm_cli::clis_on_mac` で・起動の確かめの後）を入れて読み直す。
    /// 一覧を開いている間は読み直さない（利用者の checkbox を消さない・次に開いた時に効く）。
    pub fn set_found(&mut self, found: &[&'static str]) {
        self.found = found.to_vec();
        if !self.open {
            self.read();
        }
    }

    /// `on` の行を入れる（`Installed` は触らない・`Different` は置き換える）→ 読み直す。失敗した行は `Failed(1 行)` のまま残す（on も残す = もう 1 回押せる）。
    pub fn install(&mut self) {
        let failed: Vec<(std::path::PathBuf, String)> = self
            .rows
            .iter()
            .filter(|r| r.on && r.state != SetupState::Installed)
            .filter_map(|r| {
                let done = match r.agent {
                    Some(_) => write_stub(&r.path),
                    None => link_cli(&r.path, &self.exe),
                };
                done.err().map(|why| (r.path.clone(), why))
            })
            .collect();
        self.read();
        self.mark_failed(failed);
    }

    /// `Installed` の行だけ消す（消す直前にもう 1 回確かめる = 読んだ後に利用者が書き換えた file は消さない）→ 読み直す。
    /// stub は file と、空になった `shibubu-cli/` だけ（`remove_dir` は中身が在ると失敗する = agent の他の物に触らない）。
    pub fn remove(&mut self) {
        let failed: Vec<(std::path::PathBuf, String)> = self
            .rows
            .iter()
            .filter(|r| r.state == SetupState::Installed && setup_state_of(r.agent, &r.path, &self.exe) == SetupState::Installed)
            .filter_map(|r| {
                let gone = std::fs::remove_file(&r.path).map_err(|e| format!("Couldn't remove it ({e})"));
                if let (Ok(()), Some(_), Some(dir)) = (&gone, r.agent, r.path.parent()) {
                    let _ = std::fs::remove_dir(dir);
                }
                gone.err().map(|why| (r.path.clone(), why))
            })
            .collect();
        self.read();
        self.mark_failed(failed);
    }

    /// 入っている時の 1 行（link が `Installed` で agent の stub が 1 つ以上 `Installed`）:
    /// `shibubu CLI and skill installed for claude, codex · Remove in Settings › Terminal`。それ以外は None（panel は button の行）。
    pub fn line(&self) -> Option<String> {
        let (link, stubs) = self.rows.split_last()?;
        let agents: Vec<&str> = stubs.iter().filter(|r| r.state == SetupState::Installed).filter_map(|r| r.agent).collect();
        (link.agent.is_none() && link.state == SetupState::Installed && !agents.is_empty())
            .then(|| format!("shibubu CLI and skill installed for {} · Remove in Settings › Terminal", agents.join(", ")))
    }

    fn mark_failed(&mut self, failed: Vec<(std::path::PathBuf, String)>) {
        for (path, why) in failed {
            if let Some(r) = self.rows.iter_mut().find(|r| r.path == path) {
                (r.state, r.on) = (SetupState::Failed(why), true);
            }
        }
    }
}

/// 置き場 1 つの今（読むだけ）。stub = 中身が `SKILL_STUB` と 1 byte も違わなければ Installed・無ければ NotInstalled・他（別の中身・dir・読めない）は Different。
/// link = `exe` を指す symlink なら Installed・無ければ NotInstalled・他（別の先・file・dir）は Different。
fn setup_state_of(agent: Option<&str>, path: &Path, exe: &Path) -> SetupState {
    let missing = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    match agent {
        Some(_) => match std::fs::read_to_string(path) {
            Ok(text) if text == SKILL_STUB => SetupState::Installed,
            Err(e) if missing(&e) => SetupState::NotInstalled,
            _ => SetupState::Different,
        },
        None => match std::fs::symlink_metadata(path) {
            Err(e) if missing(&e) => SetupState::NotInstalled,
            Ok(m) if m.file_type().is_symlink() && std::fs::read_link(path).is_ok_and(|to| to == exe) => SetupState::Installed,
            _ => SetupState::Different,
        },
    }
}

/// stub を書く（`shibubu-cli/` も作る）。`.bak` は作らない（Different を置き換えるのは利用者が on にした時だけ）。
fn write_stub(path: &Path) -> Result<(), String> {
    path.parent().map_or(Ok(()), |dir| std::fs::create_dir_all(dir)).and_then(|()| std::fs::write(path, SKILL_STUB)).map_err(|e| format!("Couldn't write it ({e}) · check that folder, then press Install again"))
}

/// `~/.local/bin/shibubu` → `exe`。先の無い exe には作らない（入ったと言わない）・同じ名前の folder は消さない。
fn link_cli(link: &Path, exe: &Path) -> Result<(), String> {
    if !exe.is_file() {
        return Err(format!("Shibubu's command isn't at {} · reinstall Shibubu, then press Install again", exe.display()));
    }
    let fail = |e: std::io::Error| format!("Couldn't link it ({e}) · check ~/.local/bin, then press Install again");
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.is_dir() => return Err("A folder named shibubu is in ~/.local/bin · move it, then press Install again".into()),
        Ok(_) => std::fs::remove_file(link).map_err(fail)?,
        Err(_) => {}
    }
    std::os::unix::fs::symlink(exe, link).map_err(fail)
}

/// 一覧の行の右の語（`Not installed`・`Installed`・`A different file is here · tick it to replace it`・失敗の 1 行・見つからない agent は ` · not found on this Mac`）。
pub fn setup_state_text(row: &SetupRow) -> String {
    let state = match &row.state {
        SetupState::NotInstalled => "Not installed",
        SetupState::Installed => "Installed",
        SetupState::Different => "A different file is here · tick it to replace it",
        SetupState::Failed(why) => why.as_str(),
    };
    if row.found {
        state.to_owned()
    } else {
        format!("{state} · not found on this Mac")
    }
}

/// W9: Terminal の中で `shibubu project …` を打てるようにする設定（YCM-051）。`Generated::open` が入れる（`Generated::start` の memory の
/// store には保存先が無いので None のまま）。`Generated::with_terminal_config` は config を丸ごと替えるので、試験は要る時に自分で入れる。
/// 例: `TerminalConfig { project_cli: Some(ProjectCli { data_dir: data.canonicalize()?, bin_dir: exe.parent().unwrap().into() }), ..Default::default() }`
/// 保証しない: 利用者の rc（`-i` で読む）が PATH を置き換えた後も `shibubu` が見つかる事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCli {
    /// 子の `DATA_DIR_ENV`（絶対 path。相対 path は子の cwd で別の場所を指すので入れない）。
    pub data_dir: std::path::PathBuf,
    /// `shibubu` の在る dir（.app は `Contents/MacOS`・試験は `CARGO_BIN_EXE_shibubu` の dir）。子の PATH の先頭に足す。
    pub bin_dir: std::path::PathBuf,
}

/// TERM-RESTORE-1101: 画面を保存する置き場（`ProjectCli::data_dir` の下・Project ごとの dir に tab の session id ごとの file 1 つ・dir は 0700・file は 0600）。
/// 画面には秘密が写りうるので data dir の外（repo の folder・一時の dir）に置かない。
pub const RESTORE_DIR: &str = "terminal-restore";
/// TERM-RESTORE-1101: 1 本の terminal で保存する行（scrollback の最後と今の画面・色つき）
pub const RESTORE_LINES: usize = 2000;
/// TERM-RESTORE-1101: 出力があった terminal を上書きする間隔（落ちた時・Mac の再起動に備える。終わる時は `Generated::close_terminals` が全部書く）
pub const RESTORE_EVERY: Duration = Duration::from_secs(30);

/// TERM-RESTORE-1101（owner 189「terminalはorcaのように再起動すると自動で前の画面に戻るように」）: 1 本の terminal の画面の写し。
/// `ansi` = 最後の `RESTORE_LINES` 行を字・色・列だけの byte にした物（`vt::ansi`・VT に食わせると同じ行が並ぶ・cursor の移動と alt screen の切り替えは入れない）・
/// `cwd` = shell の今の folder（`lsof` で読む・読めなければ None）・`size` = 保存した時の画面の大きさ（同じ大きさで食わせる = 列がずれない・最初の frame で今の大きさに直る）。
/// 新しい型の理由: `TermScreen` は毎 frame の描く形で、次の起動へ渡す形（byte・folder・大きさ）とは別。
/// Wave: TERM-RESTORE-1101
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub ansi: String,
    pub cwd: Option<String>,
    pub size: TermSize,
}

/// TERM-RESTORE-1101: `dir`（`TerminalManager::restore_dir`）に保存した `key` の画面（無い・読めない・形が違う = None）。
/// 例（`Generated` の `ensure_spec`・前の起動の session）: `let saved = mgr.restore_dir().and_then(|d| read_snapshot(&d, &key));`
pub fn read_snapshot(dir: &Path, key: &SessionKey) -> Option<Snapshot> {
    let text = String::from_utf8(std::fs::read(snapshot_path(dir, key)?).ok()?).ok()?;
    let (head, ansi) = text.split_once('\n')?;
    let head: serde_json::Value = serde_json::from_str(head).ok()?;
    if head["v"] != 1 {
        return None;
    }
    let dim = |k: &str| head[k].as_u64().filter(|n| (1..=1000).contains(n)).map(|n| n as u16);
    Some(Snapshot { ansi: ansi.to_owned(), cwd: head["cwd"].as_str().map(str::to_owned), size: TermSize { rows: dim("rows")?, cols: dim("cols")? } })
}

/// TERM-RESTORE-1101: `key` の画面を書く（一時の file に書いて置き換える = 途中で落ちても前の file が残る・dir 0700・file 0600）。
pub fn write_snapshot(dir: &Path, key: &SessionKey, snapshot: &Snapshot) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = snapshot_path(dir, key).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("The terminal ID can't be a file name: {key}")))?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path.parent().unwrap_or(dir))?;
    let head = serde_json::json!({ "v": 1, "cwd": snapshot.cwd, "rows": snapshot.size.rows, "cols": snapshot.size.cols });
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let wrote = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| {
        writeln!(f, "{head}")?;
        f.write_all(snapshot.ansi.as_bytes())
    });
    let r = wrote.and_then(|()| std::fs::rename(&tmp, &path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// TERM-RESTORE-1101: 消す（tab を閉じた = `Some(key)`・Project を消した = その Project の dir ごと）。無ければ何もしない。
/// 例: `remove_snapshots(&dir, &project_id, Some(&key))`（panel の tab の ×）・`remove_snapshots(&dir, &project_id, None)`（`Generated::delete_project`）
pub fn remove_snapshots(dir: &Path, project_id: &str, key: Option<&SessionKey>) {
    match key {
        Some(k) => {
            if let Some(p) = snapshot_path(dir, k) {
                let _ = std::fs::remove_file(p);
            }
        }
        None if file_name_ok(project_id) => {
            let _ = std::fs::remove_dir_all(dir.join(project_id));
        }
        None => {}
    }
}

/// TERM-RESTORE-1101: 画面の file の場所（`dir/<project_id>/<session_id>.ansi`）。id が file の名前にならない（`/`・NUL・`.` 始まり・空）は None = 書かない。
fn snapshot_path(dir: &Path, key: &SessionKey) -> Option<PathBuf> {
    (file_name_ok(&key.project_id) && file_name_ok(&key.session_id)).then(|| dir.join(&key.project_id).join(format!("{}.ansi", key.session_id)))
}

fn file_name_ok(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('.') && !id.contains(['/', '\0'])
}

/// TERM-RESTORE-1101: shell の今の folder（`lsof -a -p <pid> -d cwd -Fn` の `n` の行・読めなければ None）。
// ponytail: 1 回ごとに lsof を起こす（数十 ms・終わる時は動いている tab の数だけ待つ）。重ければ libc の proc_pidinfo（PROC_PIDVNODEPATHINFO）に
fn shell_cwd(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-w", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix('n')).filter(|p| p.starts_with('/')).map(str::to_owned)
}

/// session を指す鍵。session は Project に属する（YCM-015）。session_id は `TerminalSessionSpec::id`（`storage::new_id` で作り、全 Project で一意）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionKey {
    pub project_id: String,
    pub session_id: String,
}

impl SessionKey {
    /// 使い方: `mgr.write(&SessionKey::of(&spec), b"pwd\n")`
    pub fn of(spec: &TerminalSessionSpec) -> Self {
        SessionKey {
            project_id: spec.project_id.clone(),
            session_id: spec.id.clone(),
        }
    }

    /// この session 自身の Page の席（`InputView::Page { page_id: session_id }`）。Page を知らない口の view: `TerminalManager::write`（人）・
    /// `run_marked` と `Generated::{record_run, start_run}`（Agent）。Page から打つ人の view は `InputView::Page { page_id: <Page の id> }` で、これとは違う
    /// （人と Agent の判定は view を見ない・`InputLease::may_write` の表）。R4 で 5 つの写しをここへ寄せた。
    /// Wave: R4
    pub fn page_view(&self) -> InputView {
        InputView::Page {
            page_id: self.session_id.clone(),
        }
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.project_id, self.session_id)
    }
}

/// PTY の大きさ（文字の行と列）。`resize` で SIGWINCH が子に届き、`stty size` が `"{rows} {cols}"` を返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TermSize {
    fn default() -> Self {
        TermSize { rows: 24, cols: 80 }
    }
}

/// 子の終わり方。`kill -9` された shell は code 1 と signal `"Killed: 9"` の両方を持つ（spike 2026-10-19）ので、code だけで判定しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitInfo {
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// session の状態。`Unavailable` = 前の起動の session（再起動で PTY は消えた・SPEC §6「exited/unavailable」）。daemon の再接続は MVP に無い。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionStatus {
    Running,
    Exited { exit: ExitInfo },
    Unavailable,
}

/// session の 1 件。`list` が返し、そのまま `terminal_sessions` の 1 行の json になる（保存形式は下の「保存」）。
///
/// 保存: `store.put(Table::TerminalSessions, &key.session_id, Some(&key.project_id), &serde_json::to_value(&info)?)`。
/// 起動の時、`status == Running` の行は `after_restart` を当てて書き戻す（前の起動の PTY は残っていない）。
/// Wave: W4
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    pub key: SessionKey,
    /// 子の shell の pid（Shibubu の直接の子）。`ps -o ppid=,tty= -p {pid}` で親が Shibubu・tty を持つ事を試験の側が見る。
    pub pid: u32,
    pub shell: String,
    pub working_directory: String,
    pub status: SessionStatus,
}

impl SessionInfo {
    /// 保存した行を次の起動で読んだ時の状態。Running → Unavailable、それ以外はそのまま（実装済み）。
    pub fn after_restart(mut self) -> Self {
        if self.status == SessionStatus::Running {
            self.status = SessionStatus::Unavailable;
        }
        self
    }
}

/// 出力の読み取り。cursor は session の始めからの byte の通し番号（ring buffer で古い byte を捨てても番号は変わらない）。
///
/// 使い方: `let c = mgr.read_since(&key, cursor)?; show(&c.bytes); cursor = c.next;`（Page を移っても cursor を持ち越すだけ。reader は増やさない）。
/// 保証する: `bytes` は `start..next` の連続した byte、`dropped` は cursor から `start` までに捨てた byte 数（上限超え・YCM-028）。
/// 保証しない: UTF-8 の文字の境目（途中で切れうる。VT は W10）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    pub start: u64,
    pub bytes: Vec<u8>,
    pub next: u64,
    pub dropped: u64,
}

/// 1 回の command の結果。出力の後ろに、その command のためだけに作った nonce と `$?` の印が shell から返った時だけ作る（L3）。
///
/// 保証する: `exit_code` は shell が印に書いた `$?`、`output` は送った後から印の前までの byte（echo された command の行と prompt を含みうる）。
/// 保証しない: 印を壊す command（`exec`・`exit`・shell を替える物）。その時は `TimedOut` か `Exited`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub nonce: String,
    pub output: String,
    pub exit_code: i32,
    /// W11: command を送った時（`take(Agent)` が通った直後）の lease。receipt の `lease_holder`・`surface_id` の正
    /// （走っている間に人が触って今の lease が変わっても、送った側を残す）。
    pub lease: InputLease,
}

/// 出力が足された・子が終わった時に呼ぶ口（W2b-AX 直し: 画面は idle の間 frame を回さないので、PTY の側から起こす・src/main.rs の契約）。
/// この module は egui を知らない: 画面の側（`Generated::wake_on_terminal`）が `ctx.request_repaint_after(..)` を包んで渡す。
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// manager 全体の設定。`Default` は製品の値（実装済み）。
/// Wave: W4
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalConfig {
    /// 子の PATH（他の環境変数は親から引き継ぎ、`TERM=xterm-256color` を足す）。
    pub child_path: String,
    /// shell の引数。既定は `["-l", "-i"]`（G99: login shell = Terminal.app・Orca と同じ・`.zprofile` / `.bash_profile` の PATH も読む）。試験は `["-f", "-i"]` で rc を読まない。
    pub shell_args: Vec<String>,
    /// session ごとの出力の ring buffer の上限（byte）。
    pub scrollback_bytes: usize,
    /// W10: session ごとの VT が通常の画面の上に残す行の上限（`Vt::new` に渡す・alt screen は持たない）。
    pub scrollback_lines: usize,
    /// `close` で SIGHUP から SIGKILL までに待つ時間。
    pub close_grace: Duration,
    /// W9: Some なら子に `DATA_DIR_ENV` を渡し、PATH の先頭に `bin_dir` を足す（`spawn` の doc）。既定は None（今までと同じ子の環境）。
    pub project_cli: Option<ProjectCli>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            child_path: seed_path(&std::env::var("HOME").unwrap_or_default()),
            shell_args: vec!["-l".into(), "-i".into()],
            scrollback_bytes: 4 << 20,
            scrollback_lines: 10_000,
            close_grace: Duration::from_millis(500),
            project_cli: None,
        }
    }
}

/// PTY の誤り。Display は利用者に出す 1 行（何が起きたか + 次の一手・D-e）。
/// 契約（UI-2 から・variant と Display の在処は変えない）: Display は英語（`docs/ui-copy/` の表の EN・例 `Couldn't start the terminal ({e}). Check the working folder, then reopen`）。
/// 台本（`docs/demo/full.md`）に出る Terminal の誤りの 1 行はこの Display。呼ぶ側の文での分岐は variant で引く（Display の文を鍵にしない）。
/// 保証しない: Display 以外の文（log・試験の印）を英語にする事。
/// Wave: W4, UI-2
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalError {
    /// spec が使えない（shell が空・cwd が無い・鍵が重なる以外の形の誤り）。
    InvalidSpec(String),
    /// 同じ鍵の session が動いている。
    AlreadyRunning(SessionKey),
    NotFound(SessionKey),
    /// 終わった session に書いた・resize した。
    Exited {
        key: SessionKey,
        exit: ExitInfo,
    },
    /// PTY を開けない・shell を起こせない。
    Spawn(String),
    /// PTY への書き込み・読み取りの OS の誤り。
    Io(String),
    TimedOut {
        key: SessionKey,
        waited: Duration,
    },
    /// W10: 行か列が 0 の大きさ（畳んだ枠から計った大きさ）。PTY も VT も変えない。
    InvalidSize(TermSize),
    /// W11: 入力の lease が拒んだ。PTY の列には足していない（byte 0）。
    LeaseDenied(LeaseDenied),
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(why) => write!(
                f,
                "Can't start the terminal ({why}). Check the shell and the working folder"
            ),
            Self::AlreadyRunning(_) => write!(
                f,
                "This terminal is already running. Use the one that is open"
            ),
            Self::NotFound(_) => {
                write!(f, "This terminal wasn't found. Start a new one")
            }
            Self::Exited { exit, .. } => match (&exit.signal, exit.code) {
                (Some(sig), _) => write!(
                    f,
                    "The terminal has ended ({sig}). Start a new one"
                ),
                (None, Some(code)) => write!(
                    f,
                    "The terminal has ended (exit code {code}). Start a new one"
                ),
                (None, None) => write!(f, "The terminal has ended. Start a new one"),
            },
            Self::Spawn(why) => write!(
                f,
                "Can't start the terminal ({why}). Try again"
            ),
            Self::Io(why) => write!(
                f,
                "Couldn't talk to the terminal ({why}). Close it and start a new one"
            ),
            Self::TimedOut { waited, .. } => write!(
                f,
                "Not finished after {} s. Stop it with Ctrl-C, or wait a little longer",
                waited.as_secs()
            ),
            Self::InvalidSize(s) => write!(
                f,
                "The terminal size ({} rows × {} columns) can't be used. Make the window larger",
                s.rows, s.cols
            ),
            Self::LeaseDenied(d) => write!(f, "{d}"),
        }
    }
}

impl std::error::Error for TerminalError {}

impl From<LeaseDenied> for TerminalError {
    fn from(d: LeaseDenied) -> Self {
        Self::LeaseDenied(d)
    }
}

/// 1 本の session。`ctl`（PTY と子）・`vt`・`out`（出力の ring buffer）を別の lock にする: reader の thread は `ctl` を取らない
/// （`close` が `ctl` を持ったまま EOF を待つ間も、reader は VT に食わせて返事を出し、EOF まで読める）。
/// lock の順: `vt` → `ctl`（`resize` だけ）。reader と `screen` は `vt` を離してから `out` を取る。PTY への書き込みは `input` の列（lock を待たない）。
/// W11: `lease` → `last_human`（`take`）。`written` は `take` の後に取る（`write_as` だけ。reader の返事はどれも取らない）。
struct Session {
    key: SessionKey,
    pid: u32,
    shell: String,
    working_directory: String,
    ctl: Mutex<Ctl>,
    /// PTY の master への書き込みの列（人と AI の入力 = `write`・端末の返事 = reader と `screen`）。session の writer の thread が
    /// 足した順に書く: 子が stdin を読まずに tty の入力が詰まっても、待つのはその thread だけ（UI の frame と reader は待たない・W10 反証）。
    input: mpsc::Sender<Vec<u8>>,
    /// W10: この session の画面（reader が `out` より先に食わせる）。
    vt: Mutex<vt::Vt>,
    out: Mutex<Out>,
    /// reader が byte を足した・EOF を見た時に鳴らす（`run_marked` が待つ）。
    cv: Condvar,
    /// W11: 今の入力の持ち主。保存しない（再起動で PTY が消える）。
    lease: Mutex<InputLease>,
    /// TERM-RESTORE-1101: `spawn_restorable` で起こした（`save_snapshots` が書く）・最後に書いた時（`changed_only` の比べ先）
    restorable: bool,
    saved: Mutex<Option<Instant>>,
    /// W11: 最後に人の入力（`write_as(Human)`・`touch_human`）が通った時。人の lease はここから `InputLease::HUMAN_HOLD` で切れる。
    last_human: Mutex<Instant>,
    /// W11: `write_as` が Ok で列に足した人 / Agent の byte（端末の返事・`run_marked` の印は入れない）。
    written: Mutex<Vec<u8>>,
    /// G95: 前に読んだ前面の group と、その leader の command 行（同じ group の間は `ps` を起こさない）。
    front: Mutex<Option<(i32, String)>>,
}

struct Ctl {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// 回収した終わり方（一度入れたら変えない）。
    exit: Option<ExitInfo>,
    reader: Option<thread::JoinHandle<()>>,
}

struct Out {
    buf: VecDeque<u8>,
    /// `buf[0]` の通し番号（捨てた byte の数）。
    base: u64,
    eof: bool,
    /// G95: 最後に byte が来た時（agent の Working / Needs you）
    at: Option<Instant>,
}

impl Out {
    fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }
}

fn exit_of(s: portable_pty::ExitStatus) -> ExitInfo {
    ExitInfo {
        code: Some(s.exit_code() as i32),
        signal: s.signal().map(str::to_owned),
    }
}

impl Session {
    /// 子が終わっていれば回収して `ExitInfo`（`try_wait`）。
    fn poll_exit(&self) -> Result<Option<ExitInfo>, TerminalError> {
        let mut c = self.ctl.lock().unwrap_or_else(|e| e.into_inner());
        if c.exit.is_none() {
            if let Some(s) = c
                .child
                .try_wait()
                .map_err(|e| TerminalError::Io(e.to_string()))?
            {
                c.exit = Some(exit_of(s));
            }
        }
        Ok(c.exit.clone())
    }

    /// W11: lease の判定と持ち手の更新の 1 か所（`write_as`・`touch_human`・`run_marked`）。判定は `InputLease::may_write_after`
    /// （人の lease は最後の人の入力から `HUMAN_HOLD` で切れる・focus では切らない）。Human が通れば最後の人の入力を今にする。
    /// Err なら何も変えない。lock の順: `lease` → `last_human`。Ok は通った直後の lease（`run_marked` が receipt 用に持つ）。
    fn take(&self, owner: InputOwner, view: InputView) -> Result<InputLease, LeaseDenied> {
        let mut lease = self.lease.lock().unwrap_or_else(|e| e.into_inner());
        let mut last = self.last_human.lock().unwrap_or_else(|e| e.into_inner());
        lease.may_write_after(owner, &view, last.elapsed())?;
        lease.holder = Some(owner);
        lease.view = Some(view);
        lease.since = now_rfc3339();
        if owner == InputOwner::Human {
            *last = Instant::now();
        }
        Ok(lease.clone())
    }

    /// byte を書き込みの列に足す（待たない）。writer の thread が書けずに終わった後（PTY が閉じた）は `Io`。
    fn send(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        self.input
            .send(bytes.to_vec())
            .map_err(|_| TerminalError::Io("Can't send input".into()))
    }

    /// 端末の返事（DA・DSR）を PTY に書く。終わった PTY への失敗は捨てる（返事を待つ子はもう居ない）。
    fn reply(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.send(bytes);
        }
    }

    fn info(&self) -> SessionInfo {
        let status = match self.poll_exit() {
            Ok(Some(exit)) => SessionStatus::Exited { exit },
            _ => SessionStatus::Running,
        };
        SessionInfo {
            key: self.key.clone(),
            pid: self.pid,
            shell: self.shell.clone(),
            working_directory: self.working_directory.clone(),
            status,
        }
    }

    fn running(&self) -> Result<(), TerminalError> {
        match self.poll_exit()? {
            Some(exit) => Err(TerminalError::Exited {
                key: self.key.clone(),
                exit,
            }),
            None => Ok(()),
        }
    }

    fn wait_exit(&self, timeout: Duration) -> Result<ExitInfo, TerminalError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(e) = self.poll_exit()? {
                return Ok(e);
            }
            if Instant::now() >= deadline {
                return Err(TerminalError::TimedOut {
                    key: self.key.clone(),
                    waited: timeout,
                });
            }
            // ponytail: 10ms の polling（子の終わりを知らせる口が try_wait しか無い）。session が数百になったら SIGCHLD を待つ
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn close(&self, grace: Duration) -> Result<ExitInfo, TerminalError> {
        let mut c = self.ctl.lock().unwrap_or_else(|e| e.into_inner());
        if c.exit.is_none() {
            let shell = self.pid as i32;
            // tty の上の process group を全部（前面の job・`&` の job・nohup の job）。job control の下では背景 job は shell と別の group で、
            // shell を SIGKILL すると孤児になって辿れない（holdout t5）ので、signal を送る前に取る。
            let mut groups = c
                .master
                .tty_name()
                .and_then(|t| t.file_name().map(|n| tty_groups(&n.to_string_lossy())))
                .unwrap_or_default();
            groups.extend(c.master.process_group_leader());
            groups.retain(|&g| g > 0 && g != shell);
            groups.sort_unstable();
            groups.dedup();
            // SAFETY: libSystem の kill(2)。shell は自分が起こした子（回収前なので別の process に再利用されていない）・負の pid は process group。
            // SIGCONT は止まった job（SIGTTIN 等）に SIGHUP を効かせるため（zsh も HUP の後に CONT を送る）。
            unsafe {
                for &g in &groups {
                    kill(-g, SIGHUP);
                    kill(-g, SIGCONT);
                }
                kill(shell, SIGHUP);
            }
            let deadline = Instant::now() + grace;
            while c.exit.is_none() && Instant::now() < deadline {
                if let Some(s) = c
                    .child
                    .try_wait()
                    .map_err(|e| TerminalError::Io(e.to_string()))?
                {
                    c.exit = Some(exit_of(s));
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            if c.exit.is_none() {
                // SAFETY: 同上。SIGHUP を無視した shell を止める（portable-pty の kill は SIGHUP しか送らない）。
                unsafe {
                    kill(shell, SIGKILL);
                }
                let s = c
                    .child
                    .wait()
                    .map_err(|e| TerminalError::Io(e.to_string()))?; // 回収 = zombie 0
                c.exit = Some(exit_of(s));
            }
            // SIGHUP を無視した・まだ残っている job の group を止める（もう居ない group には ESRCH が返るだけ）。
            // ponytail: group を取ってから送るまでの間に group が消えて同じ番号が別の group に再利用される窓は見ない（pid 一周の間の数 ms）
            unsafe {
                for &g in &groups {
                    kill(-g, SIGKILL);
                }
            }
        }
        // shell が終われば slave を持つ者が居なくなり reader は EOF を見る。SIGHUP から逃げた孫が slave を持っていれば待ち切らずに離す。
        let out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let (out, _) = self
            .cv
            .wait_timeout_while(out, grace.max(Duration::from_millis(200)), |o| !o.eof)
            .unwrap_or_else(|e| e.into_inner());
        if out.eof {
            if let Some(h) = c.reader.take() {
                let _ = h.join();
            }
        }
        Ok(c.exit.clone().expect("上で回収した"))
    }
}

const SIGHUP: i32 = 1;
const SIGKILL: i32 = 9;
const SIGCONT: i32 = 19; // macOS の番号
extern "C" {
    // libSystem。新しい crate を足さずに process group へ送るため（libc は portable-pty の推移的依存で、直接は名指さない）。
    fn kill(pid: i32, sig: i32) -> i32;
}

/// W9: `pid` の process が今在るか（`kill(pid, 0) == 0`・signal は送らない）。0 と i32 に入らない pid は false
/// （`kill(0, 0)` は呼んだ側の process group に当たり 0 を返す）。保証しない: zombie（回収前は在ると答える）・pid の再利用。
pub(crate) fn pid_alive(pid: u32) -> bool {
    // SAFETY: libSystem の kill(2) に signal 0（存在と権限の確かめだけ）。
    pid > 0 && pid <= i32::MAX as u32 && unsafe { kill(pid as i32, 0) } == 0
}

/// tty（`ttys012`）を制御端末に持つ process の group の番号（`ps -o pgid= -t`）。ps が起こせなければ空（前面の group と shell は別に送る）。
fn tty_groups(tty: &str) -> Vec<i32> {
    std::process::Command::new("/bin/ps")
        .args(["-o", "pgid=", "-t", tty])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// G95: pid の command 行（`ps -o args= -p`）。読めなければ空（agent でない扱い）。
fn command_line(pid: i32) -> String {
    std::process::Command::new("/bin/ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// RFC3339 UTC（秒）。lease の `since` と RunReceipt の時刻。runtime / google は引かない（E2E が terminal だけを取り込む道を残す）。
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 16 byte の乱数の hex（印の nonce・L3）。`crate::crypto` は E2E が取り込まないので、ここで /dev/urandom を読む。
fn nonce() -> Result<String, TerminalError> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| TerminalError::Io(format!("Can't get a random number: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn spawn_err(e: impl fmt::Display) -> TerminalError {
    TerminalError::Spawn(e.to_string())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 全 Project の PTY の持ち主（`Generated` が 1 つ持つ）。method はすべて `&self`（中で lock）で、UI の thread と分けて持てる。
/// session ごとに reader の thread を 1 本だけ持ち、出力を ring buffer に入れる。読む側は `read_since` の cursor で読む（reader を clone しない）。
///
/// 終わらせる操作は 3 つだけ: `close`（1 本）・`close_project`（Project を閉じた・消した）・`shutdown`（Shibubu の終了・Drop）。
/// **Page を外す・移る・Terminal を Page から消す（表示だけ）は、どれも manager を呼ばない** = shell は動き続ける（SPEC §3・§4）。
/// Wave: W4
pub struct TerminalManager {
    config: TerminalConfig,
    /// 起こした順。終わった session も `close` の後も残す（出力を読める）。同じ鍵を起こし直すと古い物を置き換える。
    sessions: Mutex<Vec<Arc<Session>>>,
    /// `set_wake` の口。`spawn` の時の写しを session の thread が持つ。None = 誰も起こさない（画面の無い使い方・試験）
    wake: Mutex<Option<Wake>>,
}

impl TerminalManager {
    /// 使い方: `let mgr = TerminalManager::new(TerminalConfig::default());`（session は 0 本。PTY はまだ開かない）
    pub fn new(config: TerminalConfig) -> Self {
        TerminalManager {
            config,
            sessions: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
        }
    }

    /// 出力と子の終わりで呼ぶ口を置く（W2b-AX 直し）。この後に `spawn` した session に効く（前に起こした session には効かない）。
    ///
    /// 使い方: `mgr.set_wake(Arc::new(move || ctx.request_repaint_after(d)))`（session を起こす前に 1 回）
    /// 保証する: reader が byte を足した時・EOF を見た時と、子が終わった（`try_wait` で回収した）と分かった時に呼ぶ。
    /// EOF が子の終わりより先に来ても（tty を閉じて動き続ける子・終わる途中の revoke）、終わった後にもう 1 回呼ぶ。lock を持たずに呼ぶ。
    /// 保証しない: 呼ぶ回数の上限（出力の chunk ごとに呼ぶ = 束ねるのは呼ぶ側）。
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    /// `set_wake` で置いた口の写し。Terminal の外の裏の thread（W13 の model の答え・`Generated::start_create` / `start_edit`・W19a2 の Engine の `start_engine`）も同じ口で画面を起こす。
    pub(crate) fn wake(&self) -> Option<Wake> {
        self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    fn get(&self, key: &SessionKey) -> Result<Arc<Session>, TerminalError> {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        s.iter()
            .find(|x| &x.key == key)
            .cloned()
            .ok_or_else(|| TerminalError::NotFound(key.clone()))
    }

    fn all(&self, project_id: Option<&str>) -> Vec<Arc<Session>> {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        s.iter()
            .filter(|x| project_id.is_none_or(|p| x.key.project_id == p))
            .cloned()
            .collect()
    }

    /// `spec.shell` を `config.shell_args` で、cwd = `spec.working_directory` の PTY の中に起こし、reader の thread を 1 本立てる。
    ///
    /// 使い方: `let info = mgr.spawn(&spec, TermSize::default())?; store.put(Table::TerminalSessions, …, &info)`
    /// 保証する: 返った時に子は動いていて `info.pid` は Shibubu の直接の子・PTY の slave は親の側で閉じてある（子が終われば reader は EOF を見る）。
    /// 保証しない: rc（`.zshrc`）の出力が無い事、prompt の形。同じ鍵が動いていれば `AlreadyRunning`（終わった鍵は起こし直せる）。
    /// W9: 子の環境に `PROJECT_ENV = spec.project_id` を必ず入れる。`config.project_cli` が Some なら
    /// `DATA_DIR_ENV = data_dir` も入れ、PATH を `bin_dir:child_path` にする（子の中で `shibubu project pages --json` が打てる）。None なら PATH は今までどおり。
    /// 段 2: Some の時は `SKILL_HINT_ENV = SKILL_HINT` も入れる（file は書かない）。
    /// W10: `Vt::new(size, config.scrollback_lines)` を session に持たせ、reader は読んだ byte を **先に VT に食わせてから** ring buffer に足す
    /// （`read_since` / `run_marked` に見えた byte は、その時には `screen` にも入っている）。VT の返事（DA・DSR）は reader が PTY に書く
    /// （端末の返事で、人の入力でも AI の入力でもない = W11 の lease の外）。rows か cols が 0 の `size` は `InvalidSize`（起こさない）。
    pub fn spawn(
        &self,
        spec: &TerminalSessionSpec,
        size: TermSize,
    ) -> Result<SessionInfo, TerminalError> {
        self.start(spec, size, None, false)
    }

    /// `spawn` と `spawn_restorable` の中身（`saved` は reader の thread を立てる前に VT へ・`restorable` は `save_snapshots` が見る）
    fn start(
        &self,
        spec: &TerminalSessionSpec,
        size: TermSize,
        saved: Option<&Snapshot>,
        restorable: bool,
    ) -> Result<SessionInfo, TerminalError> {
        let key = SessionKey::of(spec);
        if size.rows == 0 || size.cols == 0 {
            return Err(TerminalError::InvalidSize(size));
        }
        if spec.shell.trim().is_empty() {
            return Err(TerminalError::InvalidSpec("The shell is empty".into()));
        }
        if key.project_id.is_empty() || key.session_id.is_empty() {
            return Err(TerminalError::InvalidSpec(
                "The project or session ID is empty".into(),
            ));
        }
        // portable-pty は使えない cwd を黙って HOME に替えるので、先に拒む（違う folder で動かさない）
        if !Path::new(&spec.working_directory).is_dir() {
            return Err(TerminalError::InvalidSpec(format!(
                "The working folder doesn't exist: {}",
                spec.working_directory
            )));
        }
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = sessions.iter().find(|x| x.key == key) {
            if old.poll_exit()?.is_none() {
                return Err(TerminalError::AlreadyRunning(key));
            }
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(spawn_err)?;
        let mut cmd = CommandBuilder::new(&spec.shell);
        cmd.args(&self.config.shell_args);
        cmd.cwd(&spec.working_directory);
        cmd.env(PROJECT_ENV, &spec.project_id);
        cmd.env(TERMINAL_ENV, &spec.id);
        match &self.config.project_cli {
            Some(cli) => {
                cmd.env(DATA_DIR_ENV, &cli.data_dir);
                cmd.env("PATH", format!("{}:{}", cli.bin_dir.display(), self.config.child_path));
                cmd.env(SKILL_HINT_ENV, SKILL_HINT);
            }
            None => cmd.env("PATH", &self.config.child_path),
        }
        cmd.env("TERM", "xterm-256color");
        if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8"); // Finder から起こした app は LANG を持たない → 日本語が化ける
        }
        let child = pair.slave.spawn_command(cmd).map_err(spawn_err)?;
        drop(pair.slave); // 持ったままだと子が終わっても reader に EOF が来ない（spike 2026-10-19）
        let pid = child.process_id().unwrap_or(0);
        let mut reader = pair.master.try_clone_reader().map_err(spawn_err)?;
        let mut writer = pair.master.take_writer().map_err(spawn_err)?;
        let (input, queue) = mpsc::channel::<Vec<u8>>();
        // writer の thread: 列の byte を順に書く（1 回の `send` の byte は続けて書く）。session が消えて列が閉じるか、書けなければ終わる
        // （後の `send` は Io）。ponytail: 列に上限は無い（stdin を読まない job への貼り付けを重ねると溜まる）・tty の入力が詰まっている間は
        // 後ろの ⌃C も列で待つ。要れば列の byte 数に上限を置いて超えた貼り付けを拒む
        thread::Builder::new()
            .name(format!("pty-w-{key}"))
            .spawn(move || {
                for b in queue {
                    if writer.write_all(&b).and_then(|_| writer.flush()).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| TerminalError::Spawn(e.to_string()))?;
        let mut screen = vt::Vt::new(size, self.config.scrollback_lines);
        if let Some(saved) = saved {
            screen.seed(saved.ansi.as_bytes());
        }
        let s = Arc::new(Session {
            restorable,
            saved: Mutex::new(None),
            key: key.clone(),
            pid,
            shell: spec.shell.clone(),
            working_directory: spec.working_directory.clone(),
            ctl: Mutex::new(Ctl {
                master: pair.master,
                child,
                exit: None,
                reader: None,
            }),
            input,
            vt: Mutex::new(screen),
            out: Mutex::new(Out {
                buf: VecDeque::new(),
                base: 0,
                eof: false,
                at: None,
            }),
            cv: Condvar::new(),
            lease: Mutex::new(InputLease::idle(Some(key.session_id.clone()), now_rfc3339())),
            last_human: Mutex::new(Instant::now()),
            written: Mutex::new(Vec::new()),
            front: Mutex::new(None),
        });
        let (r, cap) = (s.clone(), self.config.scrollback_bytes.max(1));
        let wake = self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let ring = wake.clone();
        let h = thread::Builder::new()
            .name(format!("pty-{key}"))
            .spawn(move || {
                let mut b = [0u8; 8192];
                loop {
                    let n = match reader.read(&mut b) {
                        Ok(0) | Err(_) => 0, // macOS は EOF を Ok(0) か EIO で返す
                        Ok(n) => n,
                    };
                    if n == 0 {
                        r.out.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
                        r.cv.notify_all();
                    } else {
                        // 先に画面へ（ring buffer に見えた byte は画面にも入っている）。返事は VT を離してから列に足す（待たない）
                        let reply = r.vt.lock().unwrap_or_else(|e| e.into_inner()).feed(&b[..n]);
                        r.reply(&reply);
                        let mut o = r.out.lock().unwrap_or_else(|e| e.into_inner());
                        o.buf.extend(&b[..n]);
                        o.at = Some(Instant::now());
                        let over = o.buf.len().saturating_sub(cap);
                        if over > 0 {
                            o.buf.drain(..over);
                            o.base += over as u64;
                        }
                        r.cv.notify_all();
                    }
                    // 画面を起こすのは VT に食わせた後（起きた frame が今の byte を描く・W2b-AX の wake）
                    if let Some(w) = &ring {
                        w(); // lock の外（呼ぶ側の egui の lock と組まない）
                    }
                    if n == 0 {
                        return;
                    }
                }
            })
            .map_err(|e| TerminalError::Spawn(e.to_string()))?;
        s.ctl.lock().unwrap_or_else(|e| e.into_inner()).reader = Some(h);
        if let Some(w) = wake {
            // 終わりの見張り: EOF は shell の終わりより先に来うる（tty を閉じて動き続ける子・終わる途中の revoke で回収できる前）。
            // `close` に join されない（close は ctl を持ったまま reader を join するので、ctl を取るのはこの thread だけにする）。
            // ponytail: 250ms の try_wait の polling（子の終わりを知らせる口が try_wait しか無い・wait_exit と同じ）。画面の thread ではないので frame は増えない
            let x = s.clone();
            thread::Builder::new()
                .name(format!("pty-exit-{key}"))
                .spawn(move || {
                    while matches!(x.poll_exit(), Ok(None)) {
                        thread::sleep(Duration::from_millis(250));
                    }
                    w();
                })
                .map_err(|e| TerminalError::Spawn(e.to_string()))?;
        }
        // L7-AGENT: 起動の command（`claude`・`grok` 等）は shell に打った 1 行として列に足す（`shell -c` にしない = agent が終わっても shell は残る・
        // tty は shell が rc を読み終わるまで行を持っている）。ponytail: rc が入力を捨てる（`read -t 0` 等）と打たれない。要れば prompt を待ってから打つ
        if let Some(c) = spec.command.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            let _ = s.input.send(format!("{c}\r").into_bytes());
        }
        sessions.retain(|x| x.key != key);
        sessions.push(s.clone());
        Ok(s.info())
    }

    /// TERM-RESTORE-1101: `spawn` と同じに起こし、保存する session にする（`save_snapshots` が書く・`Generated` の Page と panel の Terminal）。`saved` が在れば
    /// 子の出力より先に VT に食わせる（`Vt::seed` = 前の画面とその下の線の行。PTY と ring buffer には入れない = `read_since` と `run_marked` に出ない）。
    /// 例（前の起動の tab）: `mgr.spawn_restorable(&TerminalSessionSpec { working_directory: saved.cwd, command: None, ..spec }, saved.size, Some(&saved))?`
    /// 保証しない: 子の rc が画面を消す（`clear`）と前の画面は scrollback に流れる（消えはしない）。
    /// Wave: TERM-RESTORE-1101
    pub fn spawn_restorable(
        &self,
        spec: &TerminalSessionSpec,
        size: TermSize,
        saved: Option<&Snapshot>,
    ) -> Result<SessionInfo, TerminalError> {
        self.start(spec, size, saved, true)
    }

    /// PTY に byte をそのまま書く。W11: **人の口**で view を持たない物（`interrupt`・試験・E2E）。Page の入力欄と grid の打鍵は
    /// Host が `write_as(Human, Page の view)` で書く（R4 で doc を実際に合わせた）。Agent は `write_as`。
    /// view は持たないので `key.page_view()`（Page{session_id}）として書く（Drawer は `write_as`。holder の view を流用すると 2 view が OtherView を踏まない）。
    ///
    /// 使い方: `mgr.write(&key, "ls\n".as_bytes())?`
    /// 保証する: 待たずに返る（UI の thread が呼ぶ・W10 反証: 前は stdin を読まない job の前の貼り付けで窓が固まった）。返った時に byte は
    /// session の書き込みの列に入っていて、呼んだ順に続けて PTY へ書かれる（1 回の byte の間に他の byte は入らない）。
    /// 保証しない: master に書けた事・shell がそれを読んだ事（子が stdin を読まない間は列で待つ）。PTY に書けなくなった後は `Io`。
    pub fn write(&self, key: &SessionKey, bytes: &[u8]) -> Result<(), TerminalError> {
        self.write_as(key, InputOwner::Human, &key.page_view(), bytes)
    }

    /// W11: owner / view を指定して PTY に書く。lease が拒めば `LeaseDenied` で列に足さない（byte 0）。
    ///
    /// 使い方: `mgr.write_as(&key, InputOwner::Agent, &InputView::Drawer, nonce)?;`
    /// 保証する（実装の約束）: `InputLease::may_write_after`（最後の人の入力から `HUMAN_HOLD`）が Err なら列に足さない。端末の返事（DA1・DSR）はこの口を通らない（lease の外）。
    /// 保証しない: 子が読んだ事。読み戻しの正は試験の側の `lease-probe.bin`（製品の `input_log` は照合の相手・食い違えば L1）。
    pub fn write_as(&self, key: &SessionKey, owner: InputOwner, view: &InputView, bytes: &[u8]) -> Result<(), TerminalError> {
        let s = self.get(key)?;
        s.running()?;
        s.take(owner, view.clone())?;
        if !bytes.is_empty() {
            s.send(bytes)?;
            s.written
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(bytes);
        }
        Ok(())
    }

    /// W11: この session の今の入力の lease。
    ///
    /// 使い方: `let l = mgr.lease(&key)?; assert_eq!(l.holder, Some(InputOwner::Human));`
    /// 保証する（実装の約束）: 返る値は最後に通った `write_as` / `touch_human` / `release_human` の後の物。無い session は `NotFound`。
    /// 保証しない: 人が打っている事そのもの（正は試験の側の `lease-probe.bin`）。
    pub fn lease(&self, key: &SessionKey) -> Result<InputLease, TerminalError> {
        let s = self.get(key)?;
        let lease = s.lease.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(lease)
    }

    /// W11: 人が打ち始めた（grid の focus・入力欄）。lease を Human + view にする。
    ///
    /// 使い方: `mgr.touch_human(&key, InputView::Page { page_id: page.into() })?`
    /// 保証する（実装の約束）: 別 view が Human を持っている間は `LeaseDenied::OtherView`。Agent からはいつでも取り戻す。
    pub fn touch_human(&self, key: &SessionKey, view: InputView) -> Result<(), TerminalError> {
        let s = self.get(key)?;
        s.running()?;
        s.take(InputOwner::Human, view)?;
        Ok(())
    }

    /// W11: この view の人が離した（Page・Terminal を閉じる時の明示の口。focus が外れただけでは呼ばない）。同じ view の Human の時だけ idle に戻す。
    ///
    /// 使い方: `mgr.release_human(&key, &InputView::Drawer)?`
    /// 保証しない: idle の時間切れ（それは `take` の `HUMAN_HOLD`・呼ばなくても 5 秒で切れる）。
    pub fn release_human(&self, key: &SessionKey, view: &InputView) -> Result<(), TerminalError> {
        let s = self.get(key)?;
        s.lease
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .release_human(view, now_rfc3339());
        Ok(())
    }

    /// W11: 今回この session の列に入れた人 / Agent の byte（端末の返事は入れない）。
    ///
    /// 使い方: `assert!(!mgr.input_log(&key)?.windows(nonce.len()).any(|w| w == nonce));`
    /// 保証する（実装の約束）: `write_as` が Ok で足した byte だけ。LeaseDenied の byte は入らない。
    /// 保証しない: 子が読んだ事（正は試験の側の `lease-probe.bin`。食い違えば L1）。
    pub fn input_log(&self, key: &SessionKey) -> Result<Vec<u8>, TerminalError> {
        let s = self.get(key)?;
        let written = s.written.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(written)
    }

    /// W11: `run_marked` の結果から RunReceipt を作る（保存は `Store::put_run_receipt`）。
    ///
    /// 使い方: `let r = mgr.receipt_of(&key, id, repo, cmd, &out, t0, t1)?; r.validate()?;`
    /// 保証する（実装の約束）: `exit_code` は `out.exit_code`（shell の `$?`）。`lease_holder` と `surface_id` は `out.lease`
    /// （command を送った時の lease。走っている間に人が触っても、今の lease を読み直さない）。無い session は `NotFound`。
    /// 保証しない: Tests Page に出た事（呼ぶ側が保存して描く）。
    pub fn receipt_of(
        &self,
        key: &SessionKey,
        run_id: String,
        repo: String,
        input: String,
        out: &CommandOutput,
        started_at: String,
        finished_at: String,
    ) -> Result<RunReceipt, TerminalError> {
        self.get(key)?;
        Ok(RunReceipt::from_output(
            run_id,
            key.project_id.clone(),
            key.session_id.clone(),
            repo,
            input,
            out.output.clone(),
            out.exit_code,
            &out.lease,
            started_at,
            finished_at,
        ))
    }

    /// Ctrl-C。PTY に `\x03` を書く（`kill` しない: shell の job control が前面の job に SIGINT を送る）。
    ///
    /// 使い方: `mgr.interrupt(&key)?`（`sleep 30` が止まり、次の command が動く）。保証しない: 子が SIGINT を無視しない事。
    pub fn interrupt(&self, key: &SessionKey) -> Result<(), TerminalError> {
        self.write(key, b"\x03")
    }

    /// 大きさを変える。子に SIGWINCH が届く。
    ///
    /// 使い方: `mgr.resize(&key, TermSize { rows: 40, cols: 132 })?` の後の `stty size` は `40 132`。
    /// 保証しない: 複数の表示の大きさの調停（PTY は 1 つの大きさしか持たない・SPEC §4。どの表示の大きさを渡すかは呼ぶ側 = W5）。
    /// W10: 大きさを変える口はここ 1 つ。PTY と session の `Vt` を同じ呼び出しで変える（片方だけ変わる道を作らない）。
    /// rows か cols が 0 なら `InvalidSize` で何も変えない。同じ session を 2 か所（Page と W8 の Drawer）に出す時は、最後に呼んだ大きさが勝つ。
    /// VT の lock を持ったまま PTY を変える: 子が SIGWINCH で新しい大きさで描き直した byte を、reader が古い大きさの VT に食わせない。
    pub fn resize(&self, key: &SessionKey, size: TermSize) -> Result<(), TerminalError> {
        if size.rows == 0 || size.cols == 0 {
            return Err(TerminalError::InvalidSize(size));
        }
        let s = self.get(key)?;
        s.running()?;
        let mut vt = s.vt.lock().unwrap_or_else(|e| e.into_inner());
        s.ctl
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Io(e.to_string()))?;
        vt.resize(size)
    }

    /// `cursor` から後の出力を返す（待たない）。cursor が buffer より古ければ、残っている一番古い byte から返し、捨てた分を `dropped` に書く。
    /// 終わった session も、manager が持っている間（`close` の後も含む）は読める。
    pub fn read_since(&self, key: &SessionKey, cursor: u64) -> Result<OutputChunk, TerminalError> {
        let s = self.get(key)?;
        let o = s.out.lock().unwrap_or_else(|e| e.into_inner());
        let start = cursor.clamp(o.base, o.end());
        let bytes = o.buf.range((start - o.base) as usize..).copied().collect();
        Ok(OutputChunk {
            start,
            bytes,
            next: o.end(),
            dropped: start.saturating_sub(cursor),
        })
    }

    /// W10: その session の画面の写し（色・cursor・alt screen）。Page の表示（`TerminalPane::screen`）と試験の側の読み戻しの正。
    ///
    /// 使い方: `let s = mgr.screen(&key, 1000)?; pane.text = s.text(); pane.screen = Some(s);`（毎 frame・待たない）
    /// 保証する: parse しない（reader の thread が食わせ済みの VT を lock の間に写すだけ。待つのは reader の 1 回の read〈8 KiB〉の分まで）
    /// ＝ `seq 1 200000` の間も UI の frame は止まらない。Page を外していた間の出力も VT には入っている（ring buffer が捨てた byte も画面は失わない）。
    /// `run_marked` / `read_since` が返した byte は、返った時には写しに入っている。終わった session も manager が持っている間（`close` の後も）は読める。
    /// 保証しない: 前の起動の session（manager に無い = `NotFound`。Page は Unavailable の 1 行を出す）。
    /// Wave: W10
    pub fn screen(&self, key: &SessionKey, history: usize) -> Result<TermScreen, TerminalError> {
        let s = self.get(key)?;
        let mut vt = s.vt.lock().unwrap_or_else(|e| e.into_inner());
        let reply = vt.feed(&[]); // 期限の切れた同期更新（ESU の来ない app）だけを描く。byte は parse しない
        let screen = vt.screen(history);
        drop(vt);
        s.reply(&reply);
        Ok(screen)
    }

    /// `command` を送り、次の行で nonce と `$?` の印を出す `printf` を送って、印が返るまで待つ（L3: 印の無い出力は結果にしない）。
    /// 印は 2 つに割って送る（echo された command の行が印に一致しないため・spike 2026-10-19）。nonce は /dev/urandom の 16 byte の hex。
    ///
    /// 使い方: `let out = mgr.run_marked(&key, "false", Duration::from_secs(5))?; assert_eq!(out.exit_code, 1);`
    /// 保証する: 印は今回の nonce の物だけを見る（前の command の印・出力の中の偽の印に一致しない）。人が持っている間は `LeaseDenied::HumanOwns` で byte 0。
    /// 保証しない: 対話の command（vim 等）・標準入力を読む command（印の行を食べる）。待ち切れなければ `TimedOut`（子は止めない）、途中で shell が終われば `Exited`。
    pub fn run_marked(
        &self,
        key: &SessionKey,
        command: &str,
        timeout: Duration,
    ) -> Result<CommandOutput, TerminalError> {
        let s = self.get(key)?;
        let lease = s.take(InputOwner::Agent, key.page_view())?;
        let nonce = nonce()?;
        let needle = format!("__SHIBUBU_{nonce}__:").into_bytes();
        let from = s.out.lock().unwrap_or_else(|e| e.into_inner()).end();
        s.send(
            format!("{command}\nprintf '%s%s:%s\\n' __SHIBUBU_ '{nonce}__' \"$?\"\n").as_bytes(),
        )?;
        let deadline = Instant::now() + timeout;
        let mut o = s.out.lock().unwrap_or_else(|e| e.into_inner());
        let mut scan = from; // 新しく来た byte だけを見る（全体を毎回見直すと seq 200000 で 2 乗になる・spike）
        loop {
            let start = from.max(o.base); // 送った後の byte（上限超えで捨てた分は無い）
            let lo = scan.saturating_sub(needle.len() as u64).max(start);
            let base = o.base;
            let hay = &o.buf.make_contiguous()[(start - base) as usize..];
            let skip = (lo - start) as usize;
            if let Some(i) = find(&hay[skip..], &needle).map(|i| i + skip) {
                let tail = &hay[i + needle.len()..];
                if let Some(nl) = tail.iter().position(|&b| b == b'\r' || b == b'\n') {
                    if let Ok(code) = std::str::from_utf8(&tail[..nl])
                        .unwrap_or("")
                        .parse::<i32>()
                    {
                        let output = String::from_utf8_lossy(&hay[..i]).into_owned();
                        return Ok(CommandOutput {
                            nonce,
                            output,
                            exit_code: code,
                            lease,
                        });
                    }
                }
                // 印の数字の途中で chunk が切れた: 次の byte を待つ（scan は進めない）
            } else {
                scan = o.end();
            }
            if o.eof {
                drop(o);
                let exit = s.wait_exit(Duration::from_secs(1)).unwrap_or(ExitInfo {
                    code: None,
                    signal: None,
                });
                return Err(TerminalError::Exited {
                    key: key.clone(),
                    exit,
                });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(TerminalError::TimedOut {
                    key: key.clone(),
                    waited: timeout,
                });
            }
            o =
                s.cv.wait_timeout(o, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
        }
    }

    /// G95: agent の見立ての材料（`AgentWatch::step` に渡す）。前面 = PTY の `tcgetpgrp`（`MasterPty::process_group_leader`）の group の leader の command 行。
    ///
    /// 使い方: `if let Some(s) = watch.step(&mgr.probe(&key)?) { … }`（毎 frame・panel の tab ごと）
    /// 保証する: shell 自身が前面の時・終わった後は `foreground` が None。`ps` は前面の group が替わった時だけ起こす（同じ group の間は前の答え）。
    /// 保証しない: tty を離れた子・`exec` で名前を替えた子の名前。無い session は `NotFound`。
    pub fn probe(&self, key: &SessionKey) -> Result<AgentProbe, TerminalError> {
        let s = self.get(key)?;
        let exit = s.poll_exit()?;
        let since_output = s.out.lock().unwrap_or_else(|e| e.into_inner()).at.map(|t| t.elapsed());
        let group = match exit {
            Some(_) => None,
            None => s.ctl.lock().unwrap_or_else(|e| e.into_inner()).master.process_group_leader(),
        };
        let foreground = match group {
            Some(g) if g > 0 && g != s.pid as i32 => {
                let mut front = s.front.lock().unwrap_or_else(|e| e.into_inner());
                match front.as_ref() {
                    Some((was, args)) if *was == g => Some(args.clone()),
                    _ => {
                        let args = command_line(g);
                        *front = Some((g, args.clone()));
                        Some(args)
                    }
                }
            }
            _ => None,
        };
        let pid = group.filter(|g| *g > 0 && *g != s.pid as i32 && foreground.is_some());
        let title = s.vt.lock().unwrap_or_else(|e| e.into_inner()).title();
        Ok(AgentProbe { foreground, since_output, exited: exit.is_some(), exit_code: exit.and_then(|e| e.code), pid, title })
    }

    /// 今の状態（`try_wait` で回収したか）。終わっていれば `Exited`。
    pub fn status(&self, key: &SessionKey) -> Result<SessionStatus, TerminalError> {
        Ok(self.get(key)?.info().status)
    }

    /// 子が終わるまで待つ（`exit 3` を書いた後など）。止めはしない。待ち切れなければ `TimedOut`。
    ///
    /// 使い方: `mgr.write(&key, b"exit 3\n")?; let e = mgr.wait_exit(&key, Duration::from_secs(5))?; assert_eq!(e.code, Some(3));`
    pub fn wait_exit(
        &self,
        key: &SessionKey,
        timeout: Duration,
    ) -> Result<ExitInfo, TerminalError> {
        self.get(key)?.wait_exit(timeout)
    }

    /// 1 本を終わらせる: PTY の tty の上の process group（前面・背景の job）と shell に SIGHUP → `config.close_grace` 待つ → 残れば shell に SIGKILL → `wait` で回収 → job の group に SIGKILL。
    /// 既に終わっていれば、その `ExitInfo` を返す（2 回呼んでもよい）。session は一覧に `Exited` で残る（出力も読める）。
    ///
    /// 使い方: Terminal を「Shell ごと終了」した時に `mgr.close(&key)?`。Page から外すだけの時は呼ばない。
    /// 保証する: 返った時に shell は回収済み（zombie 0・`pgrep -P <Shibubu の pid>` に出ない）。shell が起こした job（`&`・`nohup`・SIGHUP を無視する物）にも SIGKILL を送り済み。reader の thread は EOF を見ていれば終わっている。
    /// 保証しない: tty を離れた孫（`setsid`・daemon 化）が止まる事（その孫が PTY を持つ間は reader も残る）。
    pub fn close(&self, key: &SessionKey) -> Result<ExitInfo, TerminalError> {
        self.get(key)?.close(self.config.close_grace)
    }

    /// Project を閉じた・消した時: その Project の session を全部 `close` する。返すのは閉じた順の (鍵, 終わり方)。
    /// 保証する: 返った時にその Project の shell は 0 本。1 本の誤りで残りを止めない（誤りは `Err` の組で返す）。
    pub fn close_project(
        &self,
        project_id: &str,
    ) -> Vec<(SessionKey, Result<ExitInfo, TerminalError>)> {
        self.all(Some(project_id))
            .into_iter()
            .map(|s| (s.key.clone(), s.close(self.config.close_grace)))
            .collect()
    }

    /// Shibubu の終了（`on_exit`・Drop）: 全 Project の session を `close` する。2 回目は何もしない（終わった物の ExitInfo を返すだけ）。
    /// TERM-RESTORE-1101: 止める前に保存する session の画面と folder を全部書く（`save_snapshots(false)`・止めた後は shell の folder を読めない）。
    /// 保証する: 返った時に Shibubu の子の shell は 0 本・zombie 0。
    pub fn shutdown(&self) -> Vec<(SessionKey, Result<ExitInfo, TerminalError>)> {
        self.save_snapshots(false);
        self.all(None)
            .into_iter()
            .map(|s| (s.key.clone(), s.close(self.config.close_grace)))
            .collect()
    }

    /// TERM-RESTORE-1101: 保存の置き場（`config.project_cli` の `data_dir` の下の `RESTORE_DIR`）。DB を開けない起動（memory の store）と
    /// project_cli の無い config（試験）は None = 保存しない・前の画面は戻さない（新しい shell は起こす）。
    pub fn restore_dir(&self) -> Option<PathBuf> {
        self.config.project_cli.as_ref().map(|c| c.data_dir.join(RESTORE_DIR))
    }

    /// TERM-RESTORE-1101: 1 本の今の写し（最後の `RESTORE_LINES` 行・動いていれば shell の今の folder）。
    pub fn snapshot(&self, key: &SessionKey) -> Result<Snapshot, TerminalError> {
        let s = self.get(key)?;
        let screen = self.screen(key, RESTORE_LINES)?;
        let running = s.poll_exit()?.is_none();
        Ok(Snapshot { ansi: vt::ansi(&screen), cwd: running.then(|| shell_cwd(s.pid)).flatten(), size: TermSize { rows: screen.rows, cols: screen.cols } })
    }

    /// TERM-RESTORE-1101: `spawn_restorable` で起こして動いている session を `restore_dir` に書く。`changed_only` = 前に書いた後に出力があった物だけ
    /// （`RESTORE_EVERY` ごとの裏の thread）・false = 全部（終わる時・`shutdown` の前）。返すのは書いた数。書けなかった物は飛ばす（次の回にまた書く）。
    /// folder が読めない（shell が終わった）時は前の file の folder を残す。
    pub fn save_snapshots(&self, changed_only: bool) -> usize {
        let Some(dir) = self.restore_dir() else { return 0 };
        let mut wrote = 0;
        for s in self.all(None).into_iter().filter(|s| s.restorable) {
            if !matches!(s.poll_exit(), Ok(None)) {
                continue; // 閉じた・終わった tab は書かない（閉じた tab の file を作り直さない）
            }
            let out_at = s.out.lock().unwrap_or_else(|e| e.into_inner()).at;
            let mut saved = s.saved.lock().unwrap_or_else(|e| e.into_inner());
            if changed_only && out_at.is_none_or(|t| saved.is_some_and(|w| w >= t)) {
                continue;
            }
            let now = Instant::now();
            let Ok(mut snap) = self.snapshot(&s.key) else { continue };
            if snap.cwd.is_none() {
                snap.cwd = read_snapshot(&dir, &s.key).and_then(|old| old.cwd);
            }
            if write_snapshot(&dir, &s.key, &snap).is_ok() {
                *saved = Some(now);
                wrote += 1;
            }
        }
        wrote
    }

    /// その Project の session（終わった物も含む・起こした順）。`project_id` が None なら全部。
    pub fn list(&self, project_id: Option<&str>) -> Vec<SessionInfo> {
        self.all(project_id).iter().map(|s| s.info()).collect()
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// portable-pty の 13 項目（spike 2026-10-19・docs/evidence/W4/portable-pty-13.md）を本物の `/bin/zsh` で回す回帰の試験と、終了処理の試験。
/// 子の PATH は `child_path` で明示するので、同じ binary の E2E が process の PATH を書き換えている間も揺れない。
/// 試験であって実機確認ではない。時間の上限は混雑（load 10 超）でも落ちないよう長めに取る。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::terminal::TermColor;

    const T: Duration = Duration::from_secs(20);

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

    fn mgr() -> TerminalManager {
        TerminalManager::new(TerminalConfig {
            shell_args: vec!["-f".into(), "-i".into()],
            ..TerminalConfig::default()
        })
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("shibubu-pty-{tag}-{}", crate::storage::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn release_page(m: &TerminalManager, k: &SessionKey) {
        m.release_human(k, &k.page_view()).unwrap();
    }

    fn wait_path(p: &std::path::Path, t: Duration) {
        let wait = Instant::now();
        while !p.exists() {
            assert!(wait.elapsed() < t, "file が現れない: {p:?}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_needle(p: &std::path::Path, needle: &[u8], t: Duration) {
        let wait = Instant::now();
        loop {
            let b = std::fs::read(p).unwrap_or_default();
            if !needle.is_empty() && b.windows(needle.len()).any(|w| w == needle) {
                return;
            }
            assert!(wait.elapsed() < t, "byte が {p:?} に届かない: {b:?}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// 人が Page を持ち、前面を `cat -u > lease-probe.bin` にする（読み戻しの正は probe）。
    fn start_probe(m: &TerminalManager, k: &SessionKey, d: &std::path::Path) -> std::path::PathBuf {
        let probe = d.join("lease-probe.bin");
        let view = k.page_view();
        m.touch_human(k, view.clone()).unwrap();
        let cat = format!("cat -u > {}\n", probe.display());
        m.write_as(k, InputOwner::Human, &view, cat.as_bytes())
            .unwrap();
        wait_path(&probe, T);
        // 人はまだ打っている（cat の起動を待つ間に混雑で HUMAN_HOLD を過ぎても、ここからの 5 秒は人の物）
        m.touch_human(k, view).unwrap();
        probe
    }

    fn spec(project: &str, cwd: &Path) -> TerminalSessionSpec {
        TerminalSessionSpec {
            id: crate::storage::new_id(),
            project_id: project.into(),
            shell: "/bin/zsh".into(),
            working_directory: cwd.display().to_string(),
            keep_alive_on_page_switch: true, command: None,
        }
    }

    /// `ps` に出る状態（None = process が無い = 回収済みで zombie でもない）。
    fn ps_stat(pid: u32) -> Option<String> {
        let o = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    fn ppid(pid: u32) -> String {
        let o = std::process::Command::new("/bin/ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_owned()
    }

    fn all_output(m: &TerminalManager, k: &SessionKey) -> String {
        String::from_utf8_lossy(&m.read_since(k, 0).unwrap().bytes).into_owned()
    }

    /// TERM-RESTORE-1101: 保存する session（`spawn_restorable`）は画面と shell の今の folder を `restore_dir` に書き（file は 0600）、同じ鍵の次の shell は
    /// その画面を上に描いて保存した folder で起きる。前の画面は VT にだけ入る（`read_since` に出ない）。`spawn` の session は書かない。
    /// 変異: `save_snapshots` の restorable の照合を外す → spawn の session の file が出来て赤・`spawn_restorable` で `seed` を呼ばない → 前の字が無く赤・
    /// cwd を `lsof` で読まない → None で赤・file を 0600 で作らない → 赤。
    #[test]
    fn term_restore_saves_screen_and_folder_and_the_next_shell_draws_them() {
        let d = dir("restore");
        let m = TerminalManager::new(TerminalConfig {
            shell_args: vec!["-f".into(), "-i".into()],
            project_cli: Some(ProjectCli { data_dir: d.clone(), bin_dir: d.clone() }),
            ..TerminalConfig::default()
        });
        let (sp, plain) = (spec("p.restore", &d), spec("p.restore", &d));
        let key = SessionKey::of(&sp);
        m.spawn_restorable(&sp, TermSize::default(), None).unwrap();
        m.spawn(&plain, TermSize::default()).unwrap();
        m.write(&key, b"cd / && echo restore-mark-$((6*7))\n").unwrap();
        until(&m, &key, patient(T), |s| s.text().contains("restore-mark-42").then_some(())).expect("echo の出力が画面に出ない");
        assert_eq!(m.save_snapshots(false), 1, "保存する session だけを書く");
        let rd = m.restore_dir().expect("project_cli の在る config に置き場が無い");
        let saved = read_snapshot(&rd, &key).expect("書いた画面を読めない");
        assert_eq!(saved.cwd.as_deref(), Some("/"), "shell の今の folder（cd の後）");
        assert!(saved.ansi.contains("restore-mark-42"), "{:?}", saved.ansi);
        assert!(read_snapshot(&rd, &SessionKey::of(&plain)).is_none(), "spawn の session を書いた");
        use std::os::unix::fs::PermissionsExt;
        let file = std::fs::read_dir(rd.join("p.restore")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600, "画面の file を他の人が読める");
        m.shutdown();
        let next = TerminalSessionSpec { working_directory: saved.cwd.clone().unwrap(), ..sp.clone() };
        m.spawn_restorable(&next, saved.size, Some(&saved)).unwrap();
        m.write(&key, b"pwd\n").unwrap();
        let end = Instant::now() + patient(T);
        while !all_output(&m, &key).contains("\r\n/\r\n") {
            assert!(Instant::now() < end, "新しい shell が保存した folder で起きない: {:?}", all_output(&m, &key));
            thread::sleep(Duration::from_millis(50));
        }
        let s = m.screen(&key, RESTORE_LINES).unwrap();
        assert!(s.text().contains("restore-mark-42") && s.rule.is_some(), "前の画面と線が新しい shell の上に無い: {:?}", s.text());
        assert!(!all_output(&m, &key).contains("restore-mark-42"), "前の画面が PTY の出力に混ざった");
        m.shutdown();
        remove_snapshots(&rd, "p.restore", None);
        assert!(!rd.join("p.restore").exists(), "Project の dir が残った");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// L7-AGENT（④）: spec の起動の command は shell の最初の 1 行として走り（`$((6*7))` を shell が計算した = 打っただけではない）、
    /// 終わった後も shell は残る（次の 1 行が打てる）・子は自分の Terminal の id を `TERMINAL_ENV` で持つ。command の無い spec は何も打たない。
    #[test]
    fn l7_launch_command_is_the_first_line_and_the_child_knows_its_terminal() {
        let m = mgr();
        let d = dir("l7");
        let mut sp = spec("p.l7", &d);
        sp.command = Some("echo l7-launched-$((6*7))".into());
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let o = m.run_marked(&k, &format!("echo id=${TERMINAL_ENV}"), T).unwrap();
        assert_eq!(o.exit_code, 0, "起動の command の後も shell は残る");
        assert!(o.output.contains(&format!("id={}", sp.id)), "{:?}", o.output);
        assert!(all_output(&m, &k).contains("l7-launched-42"), "起動の command が走っていない: {:?}", all_output(&m, &k));
        let plain = spec("p.l7b", &d);
        let kp = SessionKey::of(&plain);
        m.spawn(&plain, TermSize::default()).unwrap();
        m.run_marked(&kp, "true", T).unwrap();
        assert!(!all_output(&m, &kp).contains("l7-launched"), "command の無い spec は何も打たない");
    }

    /// SHIBUBU-SKILL 段 2: `project_cli` が在る（PATH に `shibubu` が在る）Terminal の子だけが案内の 1 行を env に持つ。保存先（data dir）にも
    /// cwd（利用者の repo）にも file を 1 つも置かない（RULE-043: 製品の AI の cwd = data dir に案内を置かない）。
    /// 負の対照: spawn の `cmd.env(SKILL_HINT_ENV, …)` を外す → hint= が空で赤 / `None` の枝にも入れる → 口の無い子が持って赤。
    #[test]
    fn skills2_child_env_has_the_hint_and_no_file_is_written() {
        let (data, cwd) = (dir("skills2-data"), dir("skills2-cwd"));
        let names = |d: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        };
        let before = (names(&data), names(&cwd));
        let with_cli = TerminalManager::new(TerminalConfig {
            shell_args: vec!["-f".into(), "-i".into()],
            project_cli: Some(ProjectCli { data_dir: data.clone(), bin_dir: dir("skills2-bin") }),
            ..TerminalConfig::default()
        });
        let sp = spec("p.skills2", &cwd);
        let k = SessionKey::of(&sp);
        with_cli.spawn(&sp, TermSize::default()).unwrap();
        let o = with_cli.run_marked(&k, &format!("printf 'hint=%s;\\n' \"${SKILL_HINT_ENV}\""), T).unwrap();
        assert!(o.output.contains(&format!("hint={SKILL_HINT};")), "{:?}", o.output);
        let plain = mgr();
        let sp2 = spec("p.skills2b", &cwd);
        let k2 = SessionKey::of(&sp2);
        plain.spawn(&sp2, TermSize::default()).unwrap();
        let o = plain.run_marked(&k2, &format!("printf 'hint=%s;\\n' \"${SKILL_HINT_ENV}\""), T).unwrap();
        assert!(o.output.contains("hint=;"), "shibubu の無い子に打てと言った: {:?}", o.output);
        assert_eq!((names(&data), names(&cwd)), before, "data dir か cwd に file が増えた");
    }

    /// 段 2 作り直し (a)(b): stub の中身は `include_str!` の 1 つ・置き場は `SKILL_AGENTS` の表どおり（fixture の HOME の下）・link は fixture の `.local/bin` に
    /// 先 = exe の symlink。見つかった agent だけ既定で on・見つからない agent には撒かない。先の無い exe には link を作らず行が理由を言う。
    /// 負の対照: `install` で `on` を見ない → codex / gemini にも stub が出来て赤 / 表の codex を `.codex/skills` に → path で赤 /
    /// link を file の写しにする → `read_link` で赤 / `line` で link の条件を外す → exe の無い HOME で Some になって赤。
    #[test]
    fn skills3_install_writes_the_stub_for_picked_agents_and_links_the_cli() {
        let (home, bin) = (dir("skills3-home"), dir("skills3-bin"));
        let exe = bin.join("shibubu");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let stub = |d: &str| home.join(d).join("shibubu-cli/SKILL.md");
        let mut s = AgentSetup::new(home.clone(), exe.clone());
        s.set_found(&["claude", "grok"]);
        let rows: Vec<_> = s.rows.iter().map(|r| (r.agent, r.path.clone(), r.found, r.on)).collect();
        assert_eq!(
            rows,
            vec![
                (Some("claude"), stub(".claude/skills"), true, true),
                (Some("codex"), stub(".agents/skills"), false, false),
                (Some("gemini"), stub(".gemini/skills"), false, false),
                (Some("grok"), stub(".grok/skills"), true, true),
                (None, home.join(CLI_LINK), true, true),
            ]
        );
        assert!(s.rows.iter().all(|r| r.state == SetupState::NotInstalled), "{:?}", s.rows);
        assert!(setup_state_text(&s.rows[1]).ends_with(" · not found on this Mac"), "{}", setup_state_text(&s.rows[1]));
        assert_eq!(s.line(), None, "入れる前に入ったと言う");
        s.install();
        assert_eq!(std::fs::read_to_string(stub(".claude/skills")).unwrap(), SKILL_STUB);
        assert_eq!(std::fs::read_to_string(stub(".grok/skills")).unwrap(), SKILL_STUB);
        assert!(!home.join(".agents").exists() && !home.join(".gemini").exists(), "選んでいない agent に撒いた");
        assert_eq!(std::fs::read_link(home.join(CLI_LINK)).unwrap(), exe, "link の先が exe でない");
        assert!(s.rows.iter().all(|r| r.state == if r.found { SetupState::Installed } else { SetupState::NotInstalled }), "{:?}", s.rows);
        assert_eq!(s.line().as_deref(), Some("shibubu CLI and skill installed for claude, grok · Remove in Settings › Terminal"));
        // stub の形: 20 行まで・frontmatter の name・版の合った案内を CLI から読ませる・見つからない時の次の一手
        assert!(SKILL_STUB.lines().count() <= 20, "{}", SKILL_STUB.lines().count());
        assert!(SKILL_STUB.starts_with("---\nname: shibubu-cli\ndescription:"), "{SKILL_STUB}");
        for must in ["Shibubu project", "this terminal belongs to a Shibubu project", "Change this project", "shibubu CLI", "Run `shibubu skills get shibubu-cli`", "SHIBUBU_PROJECT_ID", "Install CLI & skill"] {
            assert!(SKILL_STUB.contains(must), "stub に「{must}」が無い");
        }
        // 先の無い exe: link を作らず、行が理由を言う（入ったと言わない）
        let home2 = dir("skills3-home2");
        let mut s = AgentSetup::new(home2.clone(), bin.join("missing"));
        s.set_found(&["claude"]);
        s.install();
        let link = s.rows.last().unwrap();
        assert!(matches!(&link.state, SetupState::Failed(why) if why.contains("missing")), "{link:?}");
        assert!(std::fs::symlink_metadata(home2.join(CLI_LINK)).is_err(), "先の無い link を作った");
        assert_eq!(s.line(), None, "link が無いのに CLI が入ったと言う");
    }

    /// 段 2 作り直し (d): 中身が同じなら 2 回押しても書かない（stub と link の mtime が同じ）・同じ名前の別の物（利用者の file・別の Shibubu への link）は
    /// 利用者が on にするまで置き換えない・Remove は Shibubu が入れた物だけ（agent の他の skill に触らない・空になった `shibubu-cli/` も消す）。
    /// 負の対照: `install` で `Installed` も書く → mtime が変わって赤 / `Different` の既定を on → 利用者の file が消えて赤 /
    /// `remove` で `Different` も消す → 利用者の file が消えて赤 / `remove` で skills の dir ごと消す → other が消えて赤。
    #[test]
    fn skills3_same_content_is_not_rewritten_and_a_different_file_waits_for_the_person() {
        let (home, bin) = (dir("skills3d-home"), dir("skills3d-bin"));
        let exe = bin.join("shibubu");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let (mine, grok, link) = (home.join(".claude/skills/shibubu-cli/SKILL.md"), home.join(".grok/skills/shibubu-cli/SKILL.md"), home.join(CLI_LINK));
        std::fs::create_dir_all(grok.parent().unwrap()).unwrap();
        std::fs::write(&grok, "my own notes\n").unwrap();
        let mut s = AgentSetup::new(home.clone(), exe.clone());
        s.set_found(&["claude", "grok"]);
        let g = s.rows.iter().find(|r| r.agent == Some("grok")).unwrap();
        assert_eq!((&g.state, g.on), (&SetupState::Different, false), "別の file を置き換える前に聞いていない");
        assert!(setup_state_text(g).starts_with("A different file is here"), "{}", setup_state_text(g));
        s.install();
        assert_eq!(std::fs::read_to_string(&grok).unwrap(), "my own notes\n", "聞かずに置き換えた");
        let when = |p: &Path| std::fs::symlink_metadata(p).unwrap().modified().unwrap();
        let (m1, l1) = (when(&mine), when(&link));
        std::thread::sleep(Duration::from_millis(50));
        s.read();
        s.install();
        assert_eq!((when(&mine), when(&link)), (m1, l1), "中身が同じなのに書き直した");
        // 別の Shibubu への link も同じ（聞くまで置き換えない）
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(bin.join("other-shibubu"), &link).unwrap();
        s.read();
        assert_eq!(s.rows.last().unwrap().state, SetupState::Different);
        s.install();
        assert_eq!(std::fs::read_link(&link).unwrap(), bin.join("other-shibubu"), "聞かずに link を替えた");
        // 利用者が on にした時だけ置き換える（.bak を残さない）
        for r in s.rows.iter_mut().filter(|r| r.state == SetupState::Different) {
            r.on = true;
        }
        s.install();
        assert_eq!(std::fs::read_to_string(&grok).unwrap(), SKILL_STUB);
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        assert_eq!(std::fs::read_dir(grok.parent().unwrap()).unwrap().count(), 1, ".bak 等を残した");
        // Remove は自分の物だけ
        let other = home.join(".claude/skills/other/SKILL.md");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "other skill\n").unwrap();
        std::fs::write(&grok, "changed by the person\n").unwrap();
        s.read();
        s.remove();
        assert!(!mine.exists() && !mine.parent().unwrap().exists(), "stub か shibubu-cli/ が残った");
        assert!(std::fs::symlink_metadata(&link).is_err(), "link が残った");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "other skill\n", "agent の他の skill に触った");
        assert_eq!(std::fs::read_to_string(&grok).unwrap(), "changed by the person\n", "Shibubu の物でない file を消した");
        assert_eq!(s.line(), None);
    }

    /// 項目 1〜9: spawn・pwd・echo・日本語・git status・`$?`・resize・SIGINT・seq 200000（1 本の session で順に）。
    #[test]
    fn w4_pty_items_1_to_9_on_one_shell() {
        let m = mgr();
        let d = dir("one");
        let sp = spec("p.one", &d);
        let k = SessionKey::of(&sp);
        // 1 spawn: Shibubu（この test の process）の直接の子・Running
        let info = m.spawn(&sp, TermSize::default()).unwrap();
        assert!(info.pid > 0);
        assert_eq!(
            ppid(info.pid),
            std::process::id().to_string(),
            "shell の親が Shibubu"
        );
        assert_eq!(info.status, SessionStatus::Running);
        // 2 pwd = spec の作業 folder
        let o = m.run_marked(&k, "pwd", T).unwrap();
        assert_eq!(o.exit_code, 0);
        assert!(
            o.output.contains(&format!("\r\n{}\r\n", d.display())),
            "{:?}",
            o.output
        );
        // 3 echo
        let o = m.run_marked(&k, "echo hello-pty", T).unwrap();
        assert!(o.output.contains("\r\nhello-pty\r\n"), "{:?}", o.output);
        // 4 日本語 UTF-8
        let o = m.run_marked(&k, "echo 日本語テスト・絵文字🍣", T).unwrap();
        assert!(
            o.output.contains("\r\n日本語テスト・絵文字🍣\r\n"),
            "{:?}",
            o.output
        );
        // 5 git status（読むだけ・子の PATH で git が見つかる）
        let o = m
            .run_marked(
                &k,
                &format!(
                    "git -C '{}' status --short | head -5",
                    env!("CARGO_MANIFEST_DIR")
                ),
                T,
            )
            .unwrap();
        assert_eq!(o.exit_code, 0, "{:?}", o.output);
        // 6 `$?` の終了 code（L3: 印の数字が command の $?）
        assert_eq!(m.run_marked(&k, "(exit 7)", T).unwrap().exit_code, 7);
        assert_eq!(m.run_marked(&k, "false", T).unwrap().exit_code, 1);
        // 7 resize → stty size
        m.resize(
            &k,
            TermSize {
                rows: 40,
                cols: 132,
            },
        )
        .unwrap();
        let o = m.run_marked(&k, "stty size; tput cols", T).unwrap();
        assert!(o.output.contains("40 132\r\n132\r\n"), "{:?}", o.output);
        // 8 SIGINT: `\x03` で sleep 30 が止まり、続きの echo は走らず、次の command が動く
        let from = m.read_since(&k, 0).unwrap().next;
        let t = Instant::now();
        m.write(&k, b"sleep 30; echo SHOULD_NOT\n").unwrap();
        thread::sleep(Duration::from_millis(500));
        m.interrupt(&k).unwrap();
        release_page(&m, &k);
        let o = m.run_marked(&k, "echo after-int", T).unwrap();
        assert!(o.output.contains("\r\nafter-int\r\n"));
        assert!(
            t.elapsed() < Duration::from_secs(25),
            "sleep 30 が止まっていない: {:?}",
            t.elapsed()
        );
        let since = String::from_utf8_lossy(&m.read_since(&k, from).unwrap().bytes).into_owned();
        assert!(!since.contains("\r\nSHOULD_NOT"), "{since:?}");
        // 9 大量の出力: 最後の行まで落とさず、印は出力の後
        let o = m
            .run_marked(&k, "seq 1 200000", Duration::from_secs(120))
            .unwrap();
        assert!(
            o.output.contains("\r\n100000\r\n") && o.output.contains("\r\n200000\r\n"),
            "seq の途中か最後が無い"
        );
        assert_eq!(o.exit_code, 0);
        assert_eq!(
            m.status(&k).unwrap(),
            SessionStatus::Running,
            "session は動き続ける"
        );
        m.close(&k).unwrap();
        assert_eq!(
            ps_stat(info.pid),
            None,
            "close の後に shell が残る（zombie 含む）"
        );
    }

    /// 項目 10: Page を移る = 読む側が替わっても、cursor を持ち越すだけで 1 byte も欠けず重ならない（reader は 1 本のまま）。
    #[test]
    fn w4_pty_item_10_cursor_handoff_between_pages() {
        let m = mgr();
        let sp = spec("p.hand", &dir("hand"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        m.run_marked(&k, "true", T).unwrap();
        let mut cursor = m.read_since(&k, 0).unwrap().next;
        m.write(
            &k,
            b"for i in $(seq 1 15); do echo tick$i; sleep 0.1; done; echo do''ne\n",
        )
        .unwrap();
        // Page A: tick4 が見えるまで読む
        let (mut a, deadline) = (Vec::new(), Instant::now() + T);
        while !String::from_utf8_lossy(&a).contains("\r\ntick4\r\n") && Instant::now() < deadline {
            let c = m.read_since(&k, cursor).unwrap();
            a.extend(&c.bytes);
            cursor = c.next;
            thread::sleep(Duration::from_millis(20));
        }
        // Page を外していた間（読まない）→ Page B が同じ cursor から読む
        thread::sleep(Duration::from_millis(400));
        let mut b = Vec::new();
        while !String::from_utf8_lossy(&b).contains("\r\ndone\r\n") && Instant::now() < deadline {
            let c = m.read_since(&k, cursor).unwrap();
            assert_eq!((c.start, c.dropped), (cursor, 0));
            b.extend(&c.bytes);
            cursor = c.next;
            thread::sleep(Duration::from_millis(20));
        }
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&a),
            String::from_utf8_lossy(&b)
        );
        for i in 1..=15 {
            assert_eq!(
                all.matches(&format!("\r\ntick{i}\r\n")).count(),
                1,
                "tick{i}: {all:?}"
            );
        }
        assert!(String::from_utf8_lossy(&b).contains("\r\ndone\r\n"));
    }

    /// 項目 11・12: `exit 3` → code 3・reader は EOF・書くと「終わっています」の 1 行（D-e）・同じ鍵は起こし直せる。
    #[test]
    fn w4_pty_items_11_12_exit_code_and_reader_eof() {
        let m = mgr();
        let sp = spec("p.exit", &dir("exit"));
        let k = SessionKey::of(&sp);
        let info = m.spawn(&sp, TermSize::default()).unwrap();
        m.write(&k, b"exit 3\n").unwrap();
        let e = m.wait_exit(&k, T).unwrap();
        assert_eq!(
            e,
            ExitInfo {
                code: Some(3),
                signal: None
            }
        );
        assert_eq!(
            m.status(&k).unwrap(),
            SessionStatus::Exited { exit: e.clone() }
        );
        let s = m.get(&k).unwrap();
        let o = s.out.lock().unwrap();
        let (o, _) = s.cv.wait_timeout_while(o, T, |o| !o.eof).unwrap();
        assert!(
            o.eof,
            "子が終わっても reader に EOF が来ない（slave を親が持っている）"
        );
        drop(o);
        let err = m.write(&k, b"echo x\n").unwrap_err();
        assert_eq!(
            err.to_string(),
            "The terminal has ended (exit code 3). Start a new one"
        );
        assert_eq!(ps_stat(info.pid), None, "回収されて zombie も残らない");
        m.spawn(&sp, TermSize::default())
            .expect("終わった鍵は起こし直せる");
        assert_eq!(m.list(Some("p.exit")).len(), 1, "起こし直しは置き換え");
        assert!(matches!(
            m.spawn(&sp, TermSize::default()),
            Err(TerminalError::AlreadyRunning(_))
        ));
    }

    /// 項目 13: 外から `kill -9` された shell を `try_wait` と reader の EOF で見つける（code だけで判定しない）。
    #[test]
    fn w4_pty_item_13_killed_child_is_detected() {
        let m = mgr();
        let sp = spec("p.kill", &dir("kill"));
        let k = SessionKey::of(&sp);
        let info = m.spawn(&sp, TermSize::default()).unwrap();
        m.run_marked(&k, "true", T).unwrap();
        std::process::Command::new("/bin/kill")
            .args(["-9", &info.pid.to_string()])
            .status()
            .unwrap();
        let e = m.wait_exit(&k, T).unwrap();
        assert!(e.signal.is_some(), "signal が無い: {e:?}");
        assert!(matches!(
            m.run_marked(&k, "true", T),
            Err(TerminalError::Exited { .. })
        ));
    }

    /// L3: 出力の中の偽の印（前の nonce・形だけ同じ物）に一致しない。
    #[test]
    fn w4_marker_ignores_forged_and_old_nonces() {
        let m = mgr();
        let sp = spec("p.nonce", &dir("nonce"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let first = m.run_marked(&k, "true", T).unwrap();
        let o = m
            .run_marked(
                &k,
                &format!(
                    "echo __SHIBUBU_{}__:9; echo __SHIBUBU_0000__:8; (exit 4)",
                    first.nonce
                ),
                T,
            )
            .unwrap();
        assert_ne!(o.nonce, first.nonce);
        assert_eq!(o.exit_code, 4, "偽の印の数字を拾った: {:?}", o.output);
    }

    /// 誤りの形: 無い作業 folder・空の shell・知らない鍵。
    #[test]
    fn w4_rejects_bad_specs_and_unknown_keys() {
        let m = mgr();
        let mut sp = spec("p.bad", Path::new("/no/such/dir"));
        assert!(matches!(
            m.spawn(&sp, TermSize::default()),
            Err(TerminalError::InvalidSpec(_))
        ));
        sp.working_directory = "/".into();
        sp.shell = " ".into();
        assert!(matches!(
            m.spawn(&sp, TermSize::default()),
            Err(TerminalError::InvalidSpec(_))
        ));
        let k = SessionKey {
            project_id: "p".into(),
            session_id: "none".into(),
        };
        assert!(matches!(m.write(&k, b"x"), Err(TerminalError::NotFound(_))));
        assert!(m.list(None).is_empty());
    }

    /// ring buffer の上限を超えた分は捨て、cursor との差を `dropped` で返す（YCM-028）。
    #[test]
    fn w4_scrollback_limit_reports_dropped_bytes() {
        let m = TerminalManager::new(TerminalConfig {
            shell_args: vec!["-f".into(), "-i".into()],
            scrollback_bytes: 4096,
            ..TerminalConfig::default()
        });
        let sp = spec("p.ring", &dir("ring"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        m.run_marked(&k, "seq 1 5000", T).unwrap();
        let c = m.read_since(&k, 0).unwrap();
        assert!(
            c.dropped > 0 && c.start == c.dropped && c.bytes.len() <= 4096,
            "{} {} {}",
            c.start,
            c.dropped,
            c.bytes.len()
        );
        assert_eq!(c.next, c.start + c.bytes.len() as u64);
    }

    /// 終了処理: SIGHUP を無視する shell も grace の後に SIGKILL で止めて回収・close_project は他の Project を止めない・shutdown で 0 本・Drop でも 0 本。
    #[test]
    fn w4_close_project_and_shutdown_leave_no_child() {
        let m = TerminalManager::new(TerminalConfig {
            shell_args: vec!["-f".into(), "-i".into()],
            close_grace: Duration::from_millis(300),
            ..TerminalConfig::default()
        });
        let d = dir("close");
        let (a1, a2, b1) = (spec("p.a", &d), spec("p.a", &d), spec("p.b", &d));
        let pids: Vec<u32> = [&a1, &a2, &b1]
            .iter()
            .map(|s| m.spawn(s, TermSize::default()).unwrap().pid)
            .collect();
        let ka1 = SessionKey::of(&a1);
        // a1 は SIGHUP を無視し、前面で sleep を動かす → SIGKILL の道
        m.run_marked(&ka1, "trap '' HUP", T).unwrap();
        m.write(&ka1, b"sleep 60\n").unwrap();
        thread::sleep(Duration::from_millis(300));
        let closed = m.close_project("p.a");
        assert_eq!(closed.len(), 2);
        assert!(closed.iter().all(|(_, r)| r.is_ok()), "{closed:?}");
        assert_eq!(
            (ps_stat(pids[0]), ps_stat(pids[1])),
            (None, None),
            "Project を閉じた後に shell が残る"
        );
        assert!(
            ps_stat(pids[2]).is_some(),
            "他の Project の shell まで止めた"
        );
        assert_eq!(
            m.status(&SessionKey::of(&b1)).unwrap(),
            SessionStatus::Running
        );
        assert!(
            m.list(Some("p.a"))
                .iter()
                .all(|i| matches!(i.status, SessionStatus::Exited { .. })),
            "閉じた session も一覧に残る"
        );
        assert_eq!(m.shutdown().len(), 3);
        assert_eq!(ps_stat(pids[2]), None, "shutdown の後に shell が残る");
        assert_eq!(m.shutdown().len(), 3, "2 回目も同じ結果（何もしない）");
        // Drop = shutdown
        let m2 = mgr();
        let pid = m2
            .spawn(&spec("p.drop", &d), TermSize::default())
            .unwrap()
            .pid;
        drop(m2);
        assert_eq!(ps_stat(pid), None, "manager を落とした後に shell が残る");
    }

    /// 終了処理（holdout t5）: shell が `&` で起こした背景 job・`nohup` の job・SIGHUP を無視する job も、Project を閉じたら残らない（W4「PTY の子の pid が 0」）。
    #[test]
    fn w4_close_project_reaps_background_jobs() {
        let m = mgr();
        let sp = spec("p.bg", &dir("bg"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let mut jobs = Vec::new();
        // 3 つ目は SIGHUP を無視する shell の背景 job（無視は子に継ぐ → shell の SIGKILL の後も孤児で残る道）
        for cmd in [
            "sleep 300 &",
            "nohup sleep 301 >/dev/null 2>&1 &",
            "trap '' HUP; sleep 302 &",
        ] {
            m.run_marked(&k, cmd, T).unwrap();
            let o = m.run_marked(&k, "echo pid=$!", T).unwrap();
            let at = o
                .output
                .rfind("\npid=")
                .unwrap_or_else(|| panic!("{:?}", o.output))
                + 5;
            let pid: u32 = o.output[at..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(ps_stat(pid).is_some(), "背景 job が起きていない");
            jobs.push(pid);
        }
        m.close_project("p.bg");
        let deadline = Instant::now() + Duration::from_secs(5);
        while jobs.iter().any(|&p| ps_stat(p).is_some()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        let left: Vec<_> = jobs
            .iter()
            .map(|&p| (p, ps_stat(p)))
            .filter(|(_, s)| s.is_some())
            .collect();
        for (p, _) in &left {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-9", &p.to_string()])
                .status();
        }
        assert!(
            left.is_empty(),
            "Project を閉じた後に背景 job が残る: {left:?}"
        );
    }

    /// 保存の形: SessionInfo の json の往復と、再起動の後の Running → Unavailable。
    #[test]
    fn w4_session_info_round_trips_and_running_reads_unavailable_after_restart() {
        let m = mgr();
        let sp = spec("p.save", &dir("save"));
        let info = m.spawn(&sp, TermSize::default()).unwrap();
        let back: SessionInfo =
            serde_json::from_value(serde_json::to_value(&info).unwrap()).unwrap();
        assert_eq!(back, info);
        assert_eq!(back.after_restart().status, SessionStatus::Unavailable);
        let exited = SessionInfo {
            status: SessionStatus::Exited {
                exit: ExitInfo {
                    code: Some(0),
                    signal: None,
                },
            },
            ..info
        };
        assert_eq!(exited.clone().after_restart(), exited);
    }

    /// W10 の試験の道具: `f` が Some を返すまで画面を読み直す（`limit` まで）。待ち切れなければ最後に読んだ画面を Err で返す。
    fn until<R>(
        m: &TerminalManager,
        k: &SessionKey,
        limit: Duration,
        mut f: impl FnMut(&TermScreen) -> Option<R>,
    ) -> Result<R, TermScreen> {
        let end = Instant::now() + limit;
        loop {
            let s = m.screen(k, 0).unwrap();
            if let Some(r) = f(&s) {
                return Ok(r);
            }
            if Instant::now() >= end {
                return Err(s);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// 試験が起こす道具（tmux・vim）の絶対 path。無ければ赤（試験の前提が無いのに緑にしない）。
    fn tool(name: &str) -> String {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .iter()
            .map(|d| format!("{d}/{name}"))
            .find(|p| Path::new(p).exists())
            .unwrap_or_else(|| panic!("{name} が見つかりません（W10 の試験の前提）"))
    }

    /// W10 反証: stdin を読まない job（`sleep`）の前に 400 行を貼っても、`write` は待たずに返る（grid の貼り付けは UI の thread が呼ぶ。
    /// tty の入力が詰まった所で待つと窓が固まる）。byte は session の列で待ち、job が終われば順に届き、後ろに送った command も動く。
    /// job が読まない間に tty の入力から溢れた分は macOS の tty が捨てる（どの端末でも同じ・2026-10-22 に python の pty で 400 行中
    /// 382〜397 行が動いた）ので、全行は数えない。数えるのは列の最後の行と、その後ろの command。
    #[test]
    fn w10_write_into_a_job_that_does_not_read_returns_at_once_and_keeps_order() {
        let m = mgr();
        let d = dir("paste");
        let sp = spec("p.paste", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        assert_eq!(m.run_marked(&k, "true", patient(T)).unwrap().exit_code, 0);
        m.write(&k, b"sleep 5\n").unwrap();
        // 出力の行は `P1000Q`〜`P1399Q`（打った行の echo は `% echo P…` で、行の頭に `P` は来ない）。`$((…))` のように tty が途中を
        // 捨てると開いたままになる形は使わない（zsh が続きの行を待ち、後ろの command を食べる）
        let paste: String = (0..400).map(|i| format!("echo P{}Q\n", i + 1000)).collect();
        let t0 = Instant::now();
        m.write(&k, paste.as_bytes()).unwrap();
        let took = t0.elapsed();
        assert!(
            // 詰まれば `sleep 5` が終わるまで（5 秒超）待つので、上限は 1 秒から 3 秒まで伸ばしても性質は同じ
            took < patient(Duration::from_secs(1)).min(Duration::from_secs(3)),
            "tty の入力が詰まっている間に write が {took:?} 待った（UI の frame が止まる）"
        );
        release_page(&m, &k);
        // 列の最後の行が動いてから後ろの command を送る: `sleep 5` が終わる前に送ると、tty の入力が溢れている間に
        // command の byte が捨てられ（macOS の tty）、欠けた行が 127 になる（CI の遅い runner で `left: 127`）
        let end = Instant::now() + patient(Duration::from_secs(90));
        while !all_output(&m, &k).contains("\nP1399Q") {
            assert!(Instant::now() < end, "列の最後の行が動かない");
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            m.run_marked(&k, "true", patient(Duration::from_secs(90)))
                .unwrap()
                .exit_code,
            0,
            "貼り付けの後ろに送った command も届く"
        );
        let out = all_output(&m, &k);
        let last = out.find("\nP1399Q").expect("列の最後の行が動かない");
        assert!(
            out.find("\nP1000Q").is_none_or(|first| first < last),
            "貼った順に動く"
        );
        m.close(&k).unwrap();
    }

    /// W10: 本物の `git diff` の色（消した行 = SGR 31・足した行 = 32・見出し = 太字）が VT の cell に在る。
    #[test]
    fn w10_git_diff_is_colored_in_the_screen() {
        let m = mgr();
        let d = dir("diff");
        let sp = spec("p.diff", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let setup = "git init -q . && printf 'keep\\nold-line\\n' > f.txt && git add f.txt && git -c user.email=t@example.invalid -c user.name=t commit -qm init && printf 'keep\\nnew-line\\n' > f.txt";
        assert_eq!(m.run_marked(&k, setup, T).unwrap().exit_code, 0);
        let diff = m
            .run_marked(
                &k,
                "git -c color.ui=auto -c color.diff=auto --no-pager diff",
                T,
            )
            .unwrap();
        assert_eq!(diff.exit_code, 0);
        let s = m.screen(&k, 0).unwrap();
        let run = |p: &str| {
            s.lines
                .iter()
                .flatten()
                .find(|r| r.text.starts_with(p))
                .map(|r| r.style)
        };
        assert_eq!(
            run("-old-line").map(|st| st.fg),
            Some(TermColor::Indexed(1)),
            "{:#?}",
            s.visible_rows()
        );
        assert_eq!(
            run("+new-line").map(|st| st.fg),
            Some(TermColor::Indexed(2))
        );
        assert!(run("diff --git").is_some_and(|st| st.bold), "見出しは太字");
        assert!(!s.text().contains('\x1b'), "escape は画面の文字に出ない");
        m.close(&k).unwrap();
    }

    /// W10: 大量の出力（`seq 1 200000`）の間も `screen` は待たされない（UI の frame が 1 秒以上止まらない）・終わった画面の最後の行は 200000。
    #[test]
    fn w10_screen_keeps_up_with_seq_200000() {
        let m = mgr();
        let d = dir("seq");
        let sp = spec("p.seq", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let done = std::sync::atomic::AtomicBool::new(false);
        let (worst, polls) = thread::scope(|sc| {
            let poll = sc.spawn(|| {
                let (mut worst, mut n) = (Duration::ZERO, 0);
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let t0 = Instant::now();
                    let _ = m.screen(&k, 1_000).unwrap(); // 本番の frame と同じ量（generated.rs の HISTORY_SHOWN）
                    worst = worst.max(t0.elapsed());
                    n += 1;
                    thread::sleep(Duration::from_millis(16));
                }
                (worst, n)
            });
            let out = m.run_marked(&k, "seq 1 200000", Duration::from_secs(300));
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(out.unwrap().exit_code, 0);
            poll.join().unwrap()
        });
        eprintln!("W10 seq: screen {polls} 回・一番長い {worst:?}");
        assert!(
            worst < Duration::from_secs(1),
            "screen が {worst:?} 止まった"
        );
        // 読んだ回数の下限は時間の仮定（速い machine では seq が 70ms で終わり、遅い runner では 1 回の読みが長く 4 回になる）。
        // 守る性質は「flood の間に読んでも 1 秒止まらない」= 上の worst。読みが 1 回も無かったら（worst が空）だけ赤にする
        assert!(polls >= 1, "seq の間に screen を 1 回も読めなかった");
        let rows = m.screen(&k, 0).unwrap().visible_rows();
        let at = rows
            .iter()
            .position(|r| r == "200000")
            .unwrap_or_else(|| panic!("最後の行が無い: {rows:#?}"));
        assert_eq!(rows[at - 1], "199999");
        assert_eq!(
            m.screen(&k, 1_000).unwrap().history,
            1_000,
            "scrollback から 1 000 行"
        );
        m.close(&k).unwrap();
    }

    /// W10: Shibubu の Terminal の中で tmux を起こし、tmux 自身の `capture-pane -p` の行と VT の grid の行が全行一致する
    /// （tmux の窓は grid の上 23 行・最下行は tmux の状態の行）。tmux は自分の socket（-S）で起こし、終わりに kill-server と socket の file を消す。
    #[test]
    fn w10_tmux_inside_matches_capture_pane() {
        let tmux = tool("tmux");
        let m = mgr();
        let d = dir("tmux");
        let sp = spec("p.tmux", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        // socket は -S で自分の path に（-L だと tmux が signal で終わった時に /tmp/tmux-<uid>/ に file が残る）。104 byte の上限に収める
        let sock = format!("/tmp/shibubu-w10-{}", &nonce().unwrap()[..12]);
        struct Kill(String, String);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = std::process::Command::new(&self.0)
                    .args(["-S", &self.1, "kill-server"])
                    .output();
                let _ = std::fs::remove_file(&self.1);
            }
        }
        let _kill = Kill(tmux.clone(), sock.clone());
        let inner = "printf '\\033[1;31mred\\033[0m 全角 tab\\tend\\n'; seq 1 10; printf '\\033[44m blue \\033[0m'; exec sleep 600";
        m.write(
            &k,
            format!("{tmux} -u -S {sock} -f /dev/null new-session -s w10 \"{inner}\"\n").as_bytes(),
        )
        .unwrap();
        let cap = || {
            let o = std::process::Command::new(&tmux)
                .args(["-S", &sock, "capture-pane", "-p", "-t", "w10"])
                .output()
                .unwrap();
            // tmux 3.7b の capture-pane は tab を覚えていて `\t` のまま返す（-p に開く option は無い）。画面と同じ 8 列ごとの
            // tab stop で空白に開いてから比べる（全角は 2 列）
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| {
                    let mut out = String::new();
                    let mut col = 0;
                    for ch in l.chars() {
                        if ch == '\t' {
                            let n = 8 - col % 8;
                            out.extend(std::iter::repeat_n(' ', n));
                            col += n;
                        } else {
                            out.push(ch);
                            col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        }
                    }
                    out
                })
                .collect::<Vec<_>>()
        };
        let got = until(&m, &k, Duration::from_secs(30), |s| {
            let pane = usize::from(s.rows) - 1;
            let mut c = cap();
            if !s.alt_screen || !c.iter().any(|l| l.contains("blue")) || c.len() > pane {
                return None;
            }
            c.resize(pane, String::new()); // capture-pane が末尾の空の行を落とした時は空で埋めて全行を比べる
            let rows = s.visible_rows();
            (rows[..pane] == c[..]).then(|| (rows, c))
        });
        let (rows, c) = got.unwrap_or_else(|s| {
            panic!(
                "一致しない\ncapture-pane: {:#?}\ngrid: {:#?}",
                cap(),
                s.visible_rows()
            )
        });
        assert_eq!(rows.len(), 24);
        assert!(
            c.iter().any(|l| l == "10") && c.iter().any(|l| l.contains("全角 tab")),
            "{c:#?}"
        );
        assert!(
            rows[23].contains("[w10]"),
            "最下行は tmux の状態の行: {:?}",
            rows[23]
        );
        let s = m.screen(&k, 0).unwrap();
        let blue = s.lines.iter().flatten().find(|r| r.text.contains("blue"));
        assert_eq!(
            blue.map(|r| r.style.bg),
            Some(TermColor::Indexed(4)),
            "tmux を通った色も cell に"
        );
        drop(_kill);
        until(&m, &k, Duration::from_secs(20), |s| {
            (!s.alt_screen).then_some(())
        })
        .unwrap_or_else(|s| panic!("tmux の後に alt screen のまま: {:#?}", s.visible_rows()));
        release_page(&m, &k);
        assert_eq!(
            m.run_marked(&k, "true", T).unwrap().exit_code,
            0,
            "tmux の後も shell が動く"
        );
        m.close(&k).unwrap();
    }

    /// W10: vim で既知の file を開くと、grid の 1 行目が file の 1 行目・最下行に file 名・alt screen の間の text はちょうど rows 行。
    /// `:q!` で alt screen を出ると shell に戻る。vim の問い合わせ（DA2・DSR）には VT が返す。
    #[test]
    fn w10_vim_shows_the_file_and_its_name() {
        let vim = tool("vim");
        let m = mgr();
        let d = dir("vim");
        std::fs::write(d.join("w10.txt"), "first line of w10\nsecond 全角\nthird\n").unwrap();
        let sp = spec("p.vim", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        m.write(&k, format!("{vim} -u NONE -N -i NONE w10.txt\n").as_bytes())
            .unwrap();
        let s = until(&m, &k, Duration::from_secs(30), |s| {
            let rows = s.visible_rows();
            (s.alt_screen && rows[0] == "first line of w10" && rows[23].contains("w10.txt"))
                .then(|| s.clone())
        })
        .unwrap_or_else(|s| panic!("vim の画面にならない: {:#?}", s.visible_rows()));
        let rows = s.visible_rows();
        assert_eq!(
            (rows[1].as_str(), rows[2].as_str(), rows[3].as_str()),
            ("second 全角", "third", "~")
        );
        assert_eq!(
            s.text().split('\n').count(),
            usize::from(s.rows),
            "alt screen の text は rows 行ちょうど"
        );
        assert_eq!(s.history, 0);
        m.write(&k, b":q!\r").unwrap();
        until(&m, &k, Duration::from_secs(20), |s| {
            (!s.alt_screen).then_some(())
        })
        .unwrap_or_else(|s| panic!("vim を出られない: {:#?}", s.visible_rows()));
        release_page(&m, &k);
        assert_eq!(
            m.run_marked(&k, "true", T).unwrap().exit_code,
            0,
            "vim の後も shell が動く"
        );
        m.close(&k).unwrap();
    }

    /// W10: 子が `ESC [ 6 n` を出すと reader が VT の返事（cursor の位置）を PTY に書き、子が読める（端末の返事 = lease の外）。
    #[test]
    fn w10_terminal_answers_cursor_position_query_through_the_pty() {
        let m = mgr();
        let sp = spec("p.dsr", &dir("dsr"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        // run_marked は使わない（read が後ろの印の行を食べる）。返事は打った行の後ろに PTY の入力として届く
        m.write(
            &k,
            b"printf '\\033[5;7H\\033[6n'; read -rs -t 5 -d R pos; echo \"pos=${pos#*\\[}\"\n",
        )
        .unwrap();
        let got = until(&m, &k, T, |s| {
            s.visible_rows()
                .iter()
                .any(|r| r.contains("pos=5;7"))
                .then_some(())
        });
        assert!(
            got.is_ok(),
            "返事が子に届かない: {:#?}",
            got.unwrap_err().visible_rows()
        );
        m.close(&k).unwrap();
    }

    /// W10: 大きさの口は 1 つ（PTY と VT を同じ呼び出しで）・0 は `InvalidSize` で何も変えない（spawn も）・終わった session も画面は読める。
    #[test]
    fn w10_resize_changes_pty_and_vt_together_and_rejects_zero() {
        let m = mgr();
        let d = dir("size");
        let sp = spec("p.size", &d);
        let k = SessionKey::of(&sp);
        let zero = TermSize { rows: 24, cols: 0 };
        assert_eq!(
            m.spawn(&sp, zero).unwrap_err(),
            TerminalError::InvalidSize(zero)
        );
        assert!(m.list(None).is_empty(), "0 の大きさでは起こさない");
        m.spawn(&sp, TermSize::default()).unwrap();
        assert_eq!(
            m.resize(&k, zero).unwrap_err(),
            TerminalError::InvalidSize(zero)
        );
        assert!(
            m.run_marked(&k, "stty size", T)
                .unwrap()
                .output
                .contains("24 80"),
            "0 の resize は PTY を変えない"
        );
        assert_eq!(
            (m.screen(&k, 0).unwrap().rows, m.screen(&k, 0).unwrap().cols),
            (24, 80)
        );
        m.resize(
            &k,
            TermSize {
                rows: 30,
                cols: 100,
            },
        )
        .unwrap();
        assert!(m
            .run_marked(&k, "stty size", T)
            .unwrap()
            .output
            .contains("30 100"));
        let s = m.screen(&k, 0).unwrap();
        assert_eq!((s.rows, s.cols, s.lines.len()), (30, 100, 30));
        m.write(&k, b"echo bye-w10; exit 0\n").unwrap();
        m.wait_exit(&k, T).unwrap();
        let after = until(&m, &k, T, |s| {
            s.visible_rows()
                .iter()
                .any(|r| r == "bye-w10")
                .then_some(())
        })
        .map(|_| ());
        assert!(after.is_ok(), "終わった session の最後の画面を読める");
        assert!(matches!(
            m.resize(&k, TermSize::default()),
            Err(TerminalError::Exited { .. })
        ));
        let other = SessionKey {
            project_id: "p.size".into(),
            session_id: "none".into(),
        };
        assert!(matches!(
            m.screen(&other, 0),
            Err(TerminalError::NotFound(_))
        ));
    }

    /// W11: 人が打っている間の Agent の nonce は lease-probe.bin に 0 件（input_log は照合の相手）。
    #[test]
    fn w11_human_lease_blocks_agent_bytes() {
        let m = mgr();
        let d = dir("w11-lease");
        let sp = spec("p.w11", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let probe = start_probe(&m, &k, &d);
        let view = k.page_view();
        let nonce = format!("W11N{}", crate::storage::new_id());
        assert_eq!(
            m.write_as(&k, InputOwner::Agent, &view, nonce.as_bytes())
                .unwrap_err(),
            TerminalError::LeaseDenied(LeaseDenied::HumanOwns)
        );
        let human = b"human-marker\n";
        m.write_as(&k, InputOwner::Human, &view, human).unwrap();
        wait_needle(&probe, human, T);
        let probe_bytes = std::fs::read(&probe).unwrap_or_default();
        assert!(
            !probe_bytes
                .windows(nonce.len())
                .any(|w| w == nonce.as_bytes()),
            "AI の nonce が PTY に届いた: {probe_bytes:?}"
        );
        let log = m.input_log(&k).unwrap();
        assert!(
            !log.windows(nonce.len()).any(|w| w == nonce.as_bytes()),
            "denied の nonce が input_log に入った: {log:?}"
        );
        assert!(log.windows(human.len()).any(|w| w == human));
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W11: 人の lease は focus ではなく時間で切れる。人が打った直後の Agent は HumanOwns で byte 0、打つのを止めて
    /// `InputLease::HUMAN_HOLD` たつと（release_human を呼ばなくても）Agent の nonce が PTY に届く。
    #[test]
    fn w11_human_hold_expires_on_the_real_pty() {
        let m = mgr();
        let d = dir("w11-hold");
        let sp = spec("p.w11h", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let probe = start_probe(&m, &k, &d);
        let view = k.page_view();
        let early = format!("W11E{}", crate::storage::new_id());
        assert_eq!(
            m.write_as(&k, InputOwner::Agent, &view, early.as_bytes())
                .unwrap_err(),
            TerminalError::LeaseDenied(LeaseDenied::HumanOwns)
        );
        thread::sleep(InputLease::HUMAN_HOLD + Duration::from_millis(200));
        let late = format!("W11L{}\n", crate::storage::new_id());
        m.write_as(&k, InputOwner::Agent, &view, late.as_bytes())
            .unwrap();
        assert_eq!(m.lease(&k).unwrap().holder, Some(InputOwner::Agent));
        wait_needle(&probe, late.as_bytes(), T);
        let probe_bytes = std::fs::read(&probe).unwrap_or_default();
        assert!(
            !probe_bytes.windows(early.len()).any(|w| w == early.as_bytes()),
            "拒否した nonce が PTY に届いた: {probe_bytes:?}"
        );
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W11: Human 保持中の run_marked は HumanOwns で、command は lease-probe.bin に残らない。
    #[test]
    fn w11_human_lease_blocks_run_marked() {
        let m = mgr();
        let d = dir("w11-run");
        let sp = spec("p.w11r", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let probe = start_probe(&m, &k, &d);
        let secret = "SECRET-W11-RUN";
        assert_eq!(
            m.run_marked(&k, &format!("echo {secret}"), T)
                .unwrap_err(),
            TerminalError::LeaseDenied(LeaseDenied::HumanOwns)
        );
        let human = b"human-alive\n";
        m.write_as(&k, InputOwner::Human, &k.page_view(), human)
            .unwrap();
        wait_needle(&probe, human, T);
        let probe_bytes = std::fs::read(&probe).unwrap_or_default();
        assert!(
            !probe_bytes
                .windows(secret.len())
                .any(|w| w == secret.as_bytes()),
            "Human 中の run_marked が probe に command を残した: {probe_bytes:?}"
        );
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W11: 本物の PTY で Page の持ち主だけが書け、Drawer は OtherView で probe に残らない。
    #[test]
    fn w11_two_views_only_holder_writes_on_pty() {
        let m = mgr();
        let d = dir("w11-views");
        let sp = spec("p.w11v", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let probe = start_probe(&m, &k, &d);
        let page = k.page_view();
        let holder = b"page-holder\n";
        m.write_as(&k, InputOwner::Human, &page, holder).unwrap();
        wait_needle(&probe, holder, T);
        let blocked = b"drawer-blocked\n";
        assert_eq!(
            m.write_as(&k, InputOwner::Human, &InputView::Drawer, blocked)
                .unwrap_err(),
            TerminalError::LeaseDenied(LeaseDenied::OtherView)
        );
        let probe_bytes = std::fs::read(&probe).unwrap_or_default();
        assert!(probe_bytes.windows(holder.len()).any(|w| w == holder));
        assert!(
            !probe_bytes.windows(blocked.len()).any(|w| w == blocked),
            "Drawer の byte が PTY に届いた: {probe_bytes:?}"
        );
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W11 holdout: わざと失敗する test の RunReceipt.exit_code が 0 でなく、status_line が「失敗」。
    #[test]
    fn w11_failed_command_receipt_is_not_success() {
        let m = mgr();
        let d = dir("w11-fail");
        let sp = spec("p.w11f", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let t0 = now_rfc3339();
        let out = m.run_marked(&k, "false", T).unwrap();
        let t1 = now_rfc3339();
        assert_ne!(out.exit_code, 0, "false の $? が 0: {out:?}");
        let r = m
            .receipt_of(
                &k,
                "run.fail".into(),
                d.display().to_string(),
                "false".into(),
                &out,
                t0,
                t1,
            )
            .unwrap();
        r.validate().unwrap();
        assert!(!r.succeeded(), "exit {} を成功と読んだ", r.exit_code);
        assert_eq!(r.status_line(), "失敗");
        assert!(!r.status_line().contains("exit"));
        assert_eq!(r.exit_code, out.exit_code);
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W11 反証: receipt の lease_holder と surface_id は command を送った時（`run_marked` の `take`）の物。`start_run` は UI を
    /// 待たせないので、走っている間に人が Terminal に触れる。それでも Agent が送った run の receipt は Agent のまま（今の lease は人）。
    #[test]
    fn w11_receipt_keeps_the_sender_when_human_touches_during_run() {
        let m = std::sync::Arc::new(mgr());
        let d = dir("w11-holder");
        let sp = spec("p.w11o", &d);
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        let run = {
            let (m, k) = (m.clone(), k.clone());
            thread::spawn(move || m.run_marked(&k, "sleep 2; false", T))
        };
        let wait = Instant::now();
        while m.lease(&k).unwrap().holder != Some(InputOwner::Agent) {
            assert!(wait.elapsed() < T, "run_marked が Agent の lease を取らない");
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(300));
        assert!(!run.is_finished(), "混雑で run が人の手より先に終わった（環境の赤）");
        m.touch_human(&k, k.page_view()).unwrap();
        let out = run.join().unwrap().unwrap();
        assert_eq!(m.lease(&k).unwrap().holder, Some(InputOwner::Human));
        let r = m
            .receipt_of(
                &k,
                "run.holder".into(),
                d.display().to_string(),
                "sleep 2; false".into(),
                &out,
                now_rfc3339(),
                now_rfc3339(),
            )
            .unwrap();
        assert_eq!(r.exit_code, 1, "{out:?}");
        assert_eq!(
            r.lease_holder,
            InputOwner::Agent,
            "Agent が送った run の receipt が人になった"
        );
        assert_eq!(r.surface_id.as_deref(), Some(k.session_id.as_str()));
        m.close(&k).ok();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// W10 の vt.md の材料（合否に入れない・`--ignored` で 1 回だけ回す）: tmux・vim・Claude Code の起動の画面を本物の PTY で起こし、
    /// 出力の byte・VT の画面の行（・tmux の capture-pane）を `$SHIBUBU_W10_EVIDENCE` の dir に書く。vt100 は同じ byte を食わせて比べる（vt.md）。
    /// Claude Code は起動の画面まで（何も送らない = 利用者の quota を使わない）。env は空から（ANTHROPIC_API_KEY を渡さない・入れ子の印を外す）・
    /// 設定は試験の dir の空の `CLAUDE_CONFIG_DIR`（利用者の hook・MCP・設定を起こさない）。
    #[test]
    #[ignore = "vt.md の材料を取る時だけ（SHIBUBU_W10_EVIDENCE=<dir>）"]
    fn w10_record_tui_screens_for_vt_md() {
        let out = std::path::PathBuf::from(
            std::env::var("SHIBUBU_W10_EVIDENCE").expect("SHIBUBU_W10_EVIDENCE=<dir>"),
        );
        std::fs::create_dir_all(&out).unwrap();
        let save = |name: &str, m: &TerminalManager, k: &SessionKey, extra: Option<Vec<String>>| {
            std::fs::write(
                out.join(format!("{name}.bytes")),
                m.read_since(k, 0).unwrap().bytes,
            )
            .unwrap();
            std::fs::write(
                out.join(format!("{name}.alacritty.txt")),
                m.screen(k, 0).unwrap().visible_rows().join("\n"),
            )
            .unwrap();
            if let Some(c) = extra {
                std::fs::write(out.join(format!("{name}.capture.txt")), c.join("\n")).unwrap();
            }
        };
        let (tmux, vim) = (tool("tmux"), tool("vim"));
        let cases: [(&str, String); 3] = [
            ("tmux", format!("{tmux} -u -S /tmp/shibubu-w10-rec -f /dev/null new-session -s rec \"printf '\\033[1;31mred\\033[0m 全角\\n'; seq 1 40; exec sleep 600\"\n")),
            ("vim", format!("printf 'first\\nsecond 全角\\n' > v.txt; {vim} -u NONE -N -i NONE v.txt\n")),
            ("claude", "mkdir -p cfg; env -i HOME=\"$HOME\" USER=\"$USER\" PATH=\"$PATH:$HOME/.local/bin\" TERM=\"$TERM\" LANG=en_US.UTF-8 CLAUDE_CONFIG_DIR=\"$PWD/cfg\" claude\n".into()),
        ];
        for (name, cmd) in cases {
            let m = mgr();
            let sp = spec("p.rec", &dir(name));
            let k = SessionKey::of(&sp);
            m.spawn(&sp, TermSize::default()).unwrap();
            m.write(&k, cmd.as_bytes()).unwrap();
            // 画面が 3 秒変わらなくなるまで（Claude Code は起動に数秒かかるので最初の 15 秒は待つ・最長 60 秒）
            let start = Instant::now();
            let min = Duration::from_secs(if name == "claude" { 15 } else { 1 });
            let (mut last, mut since, end) =
                (String::new(), start, start + Duration::from_secs(60));
            while Instant::now() < end
                && (start.elapsed() < min || since.elapsed() < Duration::from_secs(3))
            {
                thread::sleep(Duration::from_millis(200));
                let now = m.screen(&k, 0).unwrap().text();
                if now != last {
                    (last, since) = (now, Instant::now());
                }
            }
            let cap = (name == "tmux").then(|| {
                let o = std::process::Command::new(&tmux)
                    .args([
                        "-S",
                        "/tmp/shibubu-w10-rec",
                        "capture-pane",
                        "-p",
                        "-t",
                        "rec",
                    ])
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            });
            save(name, &m, &k, cap);
            if name == "tmux" {
                let _ = std::process::Command::new(&tmux)
                    .args(["-S", "/tmp/shibubu-w10-rec", "kill-server"])
                    .output();
                let _ = std::fs::remove_file("/tmp/shibubu-w10-rec");
            }
            m.close(&k).unwrap();
        }
    }

    /// W2b-AX 直し（反証の指摘 1）: `set_wake` の口は出力で鳴り、shell が終わった時にも鳴る。reader の EOF が子の終わりより先に来ても
    /// （tty を閉じて動き続ける子・終わる途中の revoke）、終わった後にもう 1 回鳴る（画面は idle の間 frame を回さないので、鳴らなければ
    /// Page は「動いている」のまま止まって見える）。先に EOF を作るのは `exec sleep 3`（tty を全部閉じた子が 3 秒後に何も書かずに終わる）。
    #[test]
    fn w2b_ax_wake_rings_on_output_and_after_exit_even_when_eof_came_first() {
        let m = mgr();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        m.set_wake(Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }));
        let sp = spec("p.wake", &dir("wake"));
        let k = SessionKey::of(&sp);
        m.spawn(&sp, TermSize::default()).unwrap();
        rx.recv_timeout(T).expect("shell の出力（prompt）で鳴らない");
        m.run_marked(&k, "true", T).unwrap();
        m.write(&k, b"exec /bin/sleep 3 </dev/null >/dev/null 2>&1\n").unwrap();
        let s = m.get(&k).unwrap();
        let deadline = Instant::now() + T;
        while !s.out.lock().unwrap().eof {
            assert!(Instant::now() < deadline, "試験が成り立たない: tty を閉じても EOF が来ない");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(m.status(&k).unwrap(), SessionStatus::Running, "試験が成り立たない: EOF が子の終わりより先ではない");
        // EOF と echo の wake を流し切る（子が終わるのは 3 秒後）
        while rx.recv_timeout(Duration::from_millis(500)).is_ok() {}
        assert_eq!(m.status(&k).unwrap(), SessionStatus::Running, "試験が成り立たない: 流し切る前に子が終わった");
        let deadline = Instant::now() + T;
        let exited = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if rx.recv_timeout(left).is_err() {
                break false;
            }
            if matches!(m.status(&k), Ok(SessionStatus::Exited { .. })) {
                break true;
            }
        };
        assert!(exited, "EOF の後に shell が終わっても鳴らない（{:?}）", m.status(&k));
    }
}
