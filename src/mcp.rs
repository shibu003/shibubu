//! MCP (Model Context Protocol) 連携モジュール。
//! Claude / Claude Desktop から引き継いだ設定 (`~/.shibubu/mcp.json`) をロードし、
//! 各 MCP サーバーの探索・接続状態の判定・Connection UI への反映を行う。

use crate::domain::capability::DisplayTier;
use crate::ui::connections::Connection;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// MCP の server 1 つの起こし方（`mcp.json` の `mcpServers` の 1 件・Claude Desktop と同じ形）。
/// 契約（RQ4 から）: field `disabled`・`allowed_tools`・`secret_env` を足した（どれも `#[serde(default)]` で、既定の値なら書かない = 前の版と他の app が読む形のまま）。
/// 保存の形に足した key: `"disabled": true`・`"allowedTools": ["get_issue"]`・`"secretEnv": ["PENPOT_TOKEN"]`。
/// 例: `McpServerConfig { command: "npx".into(), args: vec!["-y".into(), "@playwright/mcp".into()], ..Default::default() }`。
/// 保証する: `secret_env` の名前の値は file に書かない（Keychain の `secret_store::connection_key("mcp:<server>", name)`・`McpConfig::resolved` が起こす前に env へ入れる）。
/// 保証しない: `command` が今の PATH で見つかる事（Finder から開いた app の PATH は短い・G99 の resolver の絶対 path を `McpConfig::resolved` が入れる = G99 の merge の後）。
/// Wave: RQ4
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// RQ4（RQ-016・`Disconnect`）: Shibubu は起こさない（設定は残す・`Connect` で戻す）。Engine の発見・Connections の確かめのどちらも起こさない。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// RQ4（RQ-016・tool の許可）: Engine に使わせる tool の名前。`None` = server が出す tool 全部（前の版と同じ）・`Some(list)` = その名前だけ（無い名前は無視）。
    #[serde(default, rename = "allowedTools", skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// RQ4（RQ-016・env を見せて変える）: 値を Keychain に置いた env の名前（`ConnAct::PasteSecret(name)` が足す）。画面は名前と末尾 4 文字だけを見せる。
    #[serde(default, rename = "secretEnv", skip_serializing_if = "Vec::is_empty")]
    pub secret_env: Vec<String>,
}

/// MCP の設定 file（`mcpServers` の表）。
/// 契約（RQ4 から）: Connections で server を足す・止める・消す・tool を許す method と、起こす前の形を作る `resolved` を足した（どれも `self` だけを変え、書くのは
/// `save_engine_config`）。例: `let mut cfg = mcp::load_engine_config().unwrap_or_default(); let name = cfg.add_from_text("npx -y @playwright/mcp")?; mcp::save_engine_config(&cfg)?;`
/// Wave: RQ4
#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

impl McpConfig {
    /// RQ4（RQ-016・`Add`）: 1 つの欄に打つか貼った文から server を足し、足した名前を返す。文は 2 つの形: ① 起こす command の行（`npx -y @playwright/mcp`・
    /// 空白で語に分け、`"…"` と `'…'` の中は 1 語・名前は package か program の名前から `@scope/` と `-mcp` / `mcp-` を落とした物）② README の JSON
    /// （`{"mcpServers": {"name": {...}}}` か `{"name": {...}}` か `{"command": ...}`・複数在れば全部足して最初の名前を返す）。同じ名前が在れば `-2`・`-3` を付ける。
    /// 保証する: 起こさない（確かめは呼ぶ側が `list_tools_all` で）・JSON の env の値は `env` に入れる（画面は名前だけを見せる）。
    /// 失敗（Err の 1 行・英語）: 空・command の無い JSON・形の違う JSON。
    /// Wave: RQ4
    pub fn add_from_text(&mut self, text: &str) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Paste the command that starts the MCP server, or the JSON from its instructions".into());
        }
        let found: Vec<(String, McpServerConfig)> = if text.starts_with('{') {
            let bad = || "That JSON isn't in a form Shibubu can read · paste the server's mcpServers part".to_string();
            let v: serde_json::Value = serde_json::from_str(text).map_err(|_| bad())?;
            let entries: Vec<(Option<String>, serde_json::Value)> = match (v.get("mcpServers"), v.get("command")) {
                (Some(m), _) => m.as_object().ok_or_else(bad)?.iter().map(|(k, v)| (Some(k.clone()), v.clone())).collect(),
                (None, Some(_)) => vec![(None, v)],
                (None, None) => v.as_object().ok_or_else(bad)?.iter().map(|(k, v)| (Some(k.clone()), v.clone())).collect(),
            };
            let mut out = Vec::new();
            for (key, entry) in entries {
                let server: McpServerConfig = serde_json::from_value(entry).map_err(|_| "That JSON has no command to start the server".to_string())?;
                if server.command.trim().is_empty() {
                    return Err("That JSON has no command to start the server".into());
                }
                let name = key.map_or_else(|| server_name(&server.command, &server.args), |k| slug_name(&k));
                out.push((name, server));
            }
            if out.is_empty() {
                return Err(bad());
            }
            out
        } else {
            let mut w = words(text);
            let command = w.remove(0);
            let name = server_name(&command, &w);
            vec![(name, McpServerConfig { command, args: w, ..Default::default() })]
        };
        let mut first = None;
        for (name, server) in found {
            let mut unique = name.clone();
            let mut n = 2;
            while self.mcp_servers.contains_key(&unique) {
                unique = format!("{name}-{n}");
                n += 1;
            }
            first.get_or_insert_with(|| unique.clone());
            self.mcp_servers.insert(unique, server);
        }
        Ok(first.unwrap_or_default())
    }
    /// CONNECTIONS-1101（owner 196 の 10）: Connections の MCP の Add の form（名前・command か JSON）。名前が空なら `add_from_text` のまま、在れば足した server を
    /// その名前（`slug_name`）に移す（JSON に複数在る時は最初の 1 つだけ）。同じ名前が在れば Err で何も足さない。足した名前を返す。
    /// 保証しない: URL の server（Shibubu の MCP は起こす command の stdio だけ・`McpServerConfig` に URL の口が無い）。
    pub fn add_named(&mut self, name: &str, text: &str) -> Result<String, String> {
        let want = slug_name(name.trim());
        if !name.trim().is_empty() && (want.is_empty() || self.mcp_servers.contains_key(&want)) {
            return Err(format!("There's already an MCP server named {} · pick another name", name.trim()));
        }
        let got = self.add_from_text(text)?;
        if name.trim().is_empty() || got == want {
            return Ok(got);
        }
        let server = self.mcp_servers.remove(&got).ok_or_else(|| "That MCP server isn't in Shibubu's list".to_owned())?;
        self.mcp_servers.insert(want.clone(), server);
        Ok(want)
    }
    /// RQ4（RQ-016・`Disconnect` / `Connect`）: server を止める（`disabled = true`）/ 戻す。無い名前は Err `That MCP server isn't in Shibubu's list`。
    /// Wave: RQ4
    pub fn set_disabled(&mut self, name: &str, disabled: bool) -> Result<(), String> {
        self.mcp_servers.get_mut(name).ok_or_else(not_listed)?.disabled = disabled;
        Ok(())
    }
    /// RQ4（RQ-016・`Remove`）: server を表から消して、消した設定を返す（呼ぶ側が Keychain の `secret_env` の値も消す）。無い名前は Err（`set_disabled` と同じ文）。
    /// Wave: RQ4
    pub fn remove(&mut self, name: &str) -> Result<McpServerConfig, String> {
        self.mcp_servers.remove(name).ok_or_else(not_listed)
    }
    /// RQ4（RQ-016・tool の許可）: `tool` を許す / 外す。初めて外す時は `allowed_tools` を `known`（今 server が出している tool の名前）から作ってから外す。
    /// 全部許した状態に戻ったら `None` に戻す（file に書かない）。無い名前は Err。
    /// Wave: RQ4
    pub fn allow_tool(&mut self, name: &str, tool: &str, allowed: bool, known: &[String]) -> Result<(), String> {
        let server = self.mcp_servers.get_mut(name).ok_or_else(not_listed)?;
        let mut list = server.allowed_tools.take().unwrap_or_else(|| known.to_vec());
        list.retain(|t| t != tool);
        if allowed {
            list.push(tool.to_owned());
        }
        server.allowed_tools = (known.is_empty() || !known.iter().all(|k| list.contains(k))).then_some(list);
        Ok(())
    }
    /// RQ4（RQ-016）: Engine と Connections の確かめが起こす形。`disabled` の server を落とし、`secret_env` の値を `keys` から `env` に入れる（読めない名前は入れず、
    /// その server の確かめが失敗の 1 行で言う）。G99 の merge の後は `command` を G99 の resolver の絶対 path に替える（探し方を作らない）。
    /// 保証する: `self` を変えない・file に書かない。保証しない: 鍵の値が正しい事。
    /// Wave: RQ4
    pub fn resolved(&self, keys: &crate::storage::secret_store::SecretStore) -> McpConfig {
        let mcp_servers = self
            .mcp_servers
            .iter()
            .filter(|(_, s)| !s.disabled)
            .map(|(name, s)| {
                let mut s = s.clone();
                for var in &s.secret_env {
                    let key = crate::storage::secret_store::connection_key(&format!("mcp:{name}"), var);
                    if let Ok(Some(v)) = keys.get(&key) {
                        s.env.insert(var.clone(), v);
                    }
                }
                (name.clone(), s)
            })
            .collect();
        McpConfig { mcp_servers }
    }
}

fn not_listed() -> String {
    "That MCP server isn't in Shibubu's list".into()
}

/// 1 行を語に分ける（空白で区切り・`"…"` と `'…'` の中は 1 語・escape は持たない）。MCP の command の行と Trusted command の固定の引数が使う。
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut quote = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.get_or_insert_with(String::new).push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                cur.get_or_insert_with(String::new);
            }
            (None, c) if c.is_whitespace() => out.extend(cur.take()),
            (None, c) => cur.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(cur);
    out
}

/// 名前を英数字と `-` だけにする（Keychain の key と画面の行の名前・`secret_store::connection_key` の doc）。空なら `server`。
fn slug_name(raw: &str) -> String {
    let s: String = raw.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "server".into() } else { s }
}

/// command の行から server の名前: 起こす道具（npx・uvx・node…）なら最初の package の語・他は program の名前。`@scope/name` は name（name が `mcp` か
/// `server` なら scope）・版（`@1.2`）・拡張子を落とし、頭の `mcp-server-` / `mcp-` と末尾の `-mcp-server` / `-mcp` を落とす。
fn server_name(command: &str, args: &[String]) -> String {
    const RUNNERS: &[&str] = &["npx", "bunx", "pnpx", "uvx", "pipx", "node", "deno", "bun", "python", "python3", "uv", "pnpm", "yarn", "docker"];
    const SUB: &[&str] = &["run", "dlx", "exec", "x", "tool", "-m"];
    let prog = command_name(command);
    let word = if RUNNERS.contains(&prog.as_str()) {
        args.iter().find(|a| !a.starts_with('-') && !SUB.contains(&a.as_str())).cloned().unwrap_or(prog)
    } else {
        prog
    };
    let (scope, rest) = match word.strip_prefix('@').and_then(|w| w.split_once('/')) {
        Some((scope, rest)) => (Some(scope), rest),
        None => (None, word.as_str()),
    };
    let rest = rest.split('@').next().unwrap_or(rest);
    let rest = Path::new(rest).file_stem().map_or(rest.to_owned(), |s| s.to_string_lossy().into_owned());
    let base = slug_name(&rest);
    let mut name = base.as_str();
    for p in ["mcp-server-", "mcp-"] {
        name = name.strip_prefix(p).unwrap_or(name);
    }
    for p in ["-mcp-server", "-mcp"] {
        name = name.strip_suffix(p).unwrap_or(name);
    }
    match (name, scope) {
        ("mcp" | "server" | "mcp-server" | "", Some(scope)) => slug_name(scope),
        ("", None) => "server".into(),
        (n, _) => n.to_owned(),
    }
}

/// Engine が MCP の server を起こしてよい設定（利用者が Shibubu に入れた物だけ: 環境変数 `SHIBUBU_MCP_CONFIG` か `~/.shibubu/mcp.json`）。
/// 他の app の設定（Claude Desktop の `claude_desktop_config.json`・Claude Code の `~/.claude.json`）は読まない（それは `found_config` = 利用者が頼んだ時だけ・Engine は起こさない）。
/// Wave: W19-2
/// 例: `let mcp = mcp::load_engine_config().unwrap_or_default();`（`EngineContext::mcp`）
/// 保証する: 他の app の file が在っても None（Engine はその server を起こさない）・読めない・形の違う file は None。
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から呼ぶ
pub fn load_engine_config() -> Option<McpConfig> {
    let path = engine_config_path(std::env::var("SHIBUBU_MCP_CONFIG").ok(), std::env::var("HOME").ok())?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// RQ4: Connections が書き換える前に読む Shibubu の MCP の設定（`save_engine_config` と同じ置き場）。file が無ければ空・在るのに読めなければ Err
/// （読めない file を空とみなして上書きしない = 利用者の server を消さない）。
pub(crate) fn engine_config_for_edit() -> Result<McpConfig, String> {
    let path = std::env::var("SHIBUBU_MCP_CONFIG")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".shibubu/mcp.json")));
    let Some(path) = path.filter(|p| p.exists()) else { return Ok(McpConfig::default()) };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| "Shibubu's MCP settings file couldn't be read · fix it or move it away, then try again".into())
}

/// `load_engine_config` の置き場（環境変数を書き換えずに試験できるよう、値を引数で受ける）。環境変数で選んだ file が無ければ None = 空
/// （`~/.shibubu/mcp.json` に戻らない = QA と新しい data dir の試験が利用者の server を見ずに空の MCP で回せる・`engine_config_for_edit` が書く先と同じ file を読む）。
/// 空の値は選んでいない扱い（`engine_config_for_edit` と同じ）。
fn engine_config_path(env: Option<String>, home: Option<String>) -> Option<PathBuf> {
    match env.filter(|p| !p.trim().is_empty()) {
        Some(p) => Some(PathBuf::from(p)).filter(|p| p.is_file()),
        None => home.map(|h| PathBuf::from(h).join(".shibubu/mcp.json")).filter(|p| p.is_file()),
    }
}

/// command の file 名（画面と記録に出す名前・path の dir を出さない）。file 名が取れなければ command のまま。capability_engine の CLI の手も同じ物を使う
pub(crate) fn command_name(cmd: &str) -> String {
    Path::new(cmd).file_name().map_or_else(|| cmd.to_string(), |s| s.to_string_lossy().into_owned())
}

/// コマンドが実行可能か（G99: Claude Code と同じ在りか探し `llm_cli::cli_candidates`・login shell の PATH と決まった install dir〈nvm・volta・mise・bun など〉）。
/// UI の thread から呼ぶので待たない（`known_path`: login の PATH を取る前は種の PATH）。
fn is_executable(cmd: &str) -> bool {
    executable_in(cmd, &crate::llm_cli::known_path(), &PathBuf::from(std::env::var("HOME").unwrap_or_default()))
}

/// `is_executable` の中身（PATH と HOME を渡す版・試験が偽の HOME で通す）。
pub(crate) fn executable_in(cmd: &str, path: &str, home: &Path) -> bool {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return p.is_file();
    }
    !crate::llm_cli::cli_candidates(cmd, path, home).is_empty()
}

/// RQ4（RQ-016）: Shibubu の MCP の設定（`SHIBUBU_MCP_CONFIG` か `~/.shibubu/mcp.json`）を書く。`~/.shibubu/` が無ければ作る・tmp に書いて rename（途中で落ちても前の file が残る）・0600。
/// 他の app の設定（Claude Desktop・`~/.claude.json`）には書かない。書いた path を返す。例: `cfg.set_disabled("penpot", true)?; mcp::save_engine_config(&cfg)?;`
/// 保証する: 書いた後に `load_engine_config` が同じ値を読む。失敗は Err の 1 行（`Couldn't save the MCP settings · {理由}`）。
/// Wave: RQ4
pub fn save_engine_config(cfg: &McpConfig) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let fail = |why: String| format!("Couldn't save the MCP settings · {why}");
    let path = std::env::var("SHIBUBU_MCP_CONFIG")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".shibubu/mcp.json")))
        .ok_or_else(|| fail("no home folder".into()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e.kind().to_string()))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| fail(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| fail(e.kind().to_string()))?;
    Ok(path)
}

/// RQ4（RQ-016・2 回目の run では起こし直さない）: `cfg` の server（`disabled` を除く）ごとに `tools/list` を読み、名前 → 結果を返す。
/// L4（DOGFOOD-X 周 8: npx・uvx の server を一度に全部起こすと package の解決が重なり load1 が 100 を越えた）: 起こすのは同時に `MCP_AT_ONCE` 本まで
/// （別の thread 2 本が順に取る）。各 server の待ちは `timeout` まで・答えなかった子は孫ごと止めてから次を起こす（`McpSession` の drop）。
/// 全体の待ちは (`timeout` + `CLOSE_GRACE`) × 本数 / 2 まで。答えは process の中に覚え、同じ名前と
/// 同じ設定の 2 回目は起こさずに返す（Ok は `forget_tools` まで・Err は 60 秒）。`allowed_tools` が Some なら、その名前の tool だけを返す。
/// 例（Engine の発見・Connections の確かめ）: `for (name, r) in mcp::list_tools_all(&cfg.resolved(&keys), MCP_TIMEOUT) { … }`。
/// 保証しない: 覚えた後に server の tool が替わった事を知る事（`Check again` = `forget_tools(Some(name))` で読み直す）。
/// Wave: RQ4
pub fn list_tools_all(cfg: &McpConfig, timeout: std::time::Duration) -> BTreeMap<String, Result<Vec<(McpTool, McpToolHints)>, McpError>> {
    use std::time::Instant;
    let live: Vec<(&String, &McpServerConfig)> = cfg.mcp_servers.iter().filter(|(_, s)| !s.disabled).collect();
    let mut out = BTreeMap::new();
    let mut todo = Vec::new();
    {
        let cache = TOOLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (name, s) in &live {
            match cache.get(&((*name).clone(), launch(s))) {
                Some((at, r)) if r.is_ok() || at.elapsed() < TOOLS_ERR_KEEP => {
                    out.insert((*name).clone(), r.clone());
                }
                _ => todo.push(((*name).clone(), (*s).clone())),
            }
        }
    }
    // 起こすのは MCP_AT_ONCE 本の thread が順に（1 本が黙っても片方の枠だけを塞ぐ）。答えた・止めた後に次を起こす = 生きている子は MCP_AT_ONCE 本まで
    let queue = std::sync::Arc::new(std::sync::Mutex::new(todo.iter().rev().cloned().collect::<Vec<_>>()));
    let (tx, rx) = std::sync::mpsc::channel();
    for _ in 0..todo.len().min(MCP_AT_ONCE) {
        let (tx, queue) = (tx.clone(), queue.clone());
        std::thread::spawn(move || loop {
            let next = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pop();
            let Some((name, s)) = next else { break };
            let r = list_tools_hinted(&s, timeout);
            remember_tools(&name, &s, &r);
            if tx.send((name, r)).is_err() {
                break; // 待つ側が帰った = 残りは起こさない
            }
        });
    }
    drop(tx);
    let waves = todo.len().div_ceil(MCP_AT_ONCE).max(1) as u32;
    let deadline = Instant::now() + (timeout + CLOSE_GRACE + std::time::Duration::from_millis(500)) * waves;
    while out.len() < live.len() {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name, r)) => {
                out.insert(name, r);
            }
            Err(_) => break,
        }
    }
    for (name, s) in &todo {
        if !out.contains_key(name) {
            let r = Err(McpError::Timeout { ms: timeout.as_millis() as u64 });
            remember_tools(name, s, &r);
            out.insert(name.clone(), r);
        }
    }
    for (name, r) in out.iter_mut() {
        if let (Ok(tools), Some(allow)) = (r, cfg.mcp_servers.get(name).and_then(|s| s.allowed_tools.as_ref())) {
            tools.retain(|(t, _)| allow.contains(&t.name));
        }
    }
    out
}

/// `list_tools_all` が覚えた答え（（server の名前, 起こし方）→ 時刻・答え）。process の中だけ（保存しない）。
/// L4: 起こし方ごとに覚える（同じ名前で起こし方の違う設定が答えを上書きし合わない = Accept の発見が「前に答えた」を見誤らない）。
#[allow(clippy::type_complexity)]
static TOOLS: std::sync::Mutex<BTreeMap<(String, Launch), (std::time::Instant, Result<Vec<(McpTool, McpToolHints)>, McpError>)>> = std::sync::Mutex::new(BTreeMap::new());
/// 起こし方（command・args・秘密でない env）。
type Launch = (String, Vec<String>, BTreeMap<String, String>);
/// L4: `list_tools_all` が同時に起こす server の数（DOGFOOD-X 周 8 の load1 > 100）。
const MCP_AT_ONCE: usize = 2;
/// 起こせなかった・答えなかった server を覚える長さ（過ぎたら次の `list_tools_all` が起こし直す）
const TOOLS_ERR_KEEP: std::time::Duration = std::time::Duration::from_secs(60);
/// RQ-016a: 読んだ tool の一覧で Ready と言える長さ（過ぎたら Connections の行は Available と `Check again`・Engine の覚えはそのまま）。yc/trunk の FX-025 の
/// `GITHUB_TTL` と同じ 600 秒（trunk の merge の後に 1 つに寄せる）。
const READY_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// 覚えた答えが同じ server の物かを見る形（起こし方だけ・`disabled` と `allowed_tools` を替えても起こし直さない・`resolved` が入れた秘密の env の値は見ない）
fn launch(s: &McpServerConfig) -> Launch {
    (s.command.clone(), s.args.clone(), s.env.iter().filter(|(k, _)| !s.secret_env.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn remember_tools(name: &str, s: &McpServerConfig, r: &Result<Vec<(McpTool, McpToolHints)>, McpError>) {
    TOOLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert((name.to_owned(), launch(s)), (std::time::Instant::now(), r.clone()));
}

/// 覚えている答え（起こさない・Connections の行が毎 frame 読む）。同じ起こし方の答えだけ。
/// Wave: RQ4
pub fn known_tools(name: &str, s: &McpServerConfig) -> Option<Result<Vec<(McpTool, McpToolHints)>, McpError>> {
    known(name, s).map(|(_, r)| r)
}

/// `known_tools` と同じ答えと、それを読んだ時刻（Connections の行の `checked …` と `READY_TTL`）。
fn known(name: &str, s: &McpServerConfig) -> Option<(std::time::Instant, Result<Vec<(McpTool, McpToolHints)>, McpError>)> {
    let cache = TOOLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.get(&(name.to_owned(), launch(s))).map(|(at, r)| (*at, r.clone()))
}

/// 試験の口: 覚えた答えを `by` だけ古くする（`READY_TTL` を待たずに、過ぎた後の行を見る）。
#[cfg(test)]
fn age_tools(name: &str, by: std::time::Duration) {
    for ((n, _), (at, _)) in TOOLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter_mut() {
        if n == name {
            *at = at.checked_sub(by).expect("この機械の Instant は READY_TTL より前に戻れない");
        }
    }
}

/// RQ4（RQ-016・`Check again`）: `list_tools_all` の覚えた答えを捨てる（`Some(name)` = その server・`None` = 全部）。次の `list_tools_all` が起こして読み直す。
/// Wave: RQ4
pub fn forget_tools(name: Option<&str>) {
    let mut cache = TOOLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match name {
        Some(n) => cache.retain(|(k, _), _| k != n),
        None => cache.clear(),
    }
}

/// 各 MCP サーバーの設定から Connection 行を生成する
/// 契約（RQ4 から）: 行は 3 種類。頭に見出しの行 key `mcp`（`MCP servers`・acts = `Add`）。① Shibubu の設定（`engine` = `load_engine_config`）の server =
/// key `mcp:<name>`・状態は `known_tools` の答えを `checked_state` で（RQ-016a: まだ = Available `Found {cmd} · not checked yet`・答えて tool 0 = Connected・
/// 1 つ以上 = Ready `{n} tools`・`READY_TTL` 過ぎと誤り = Available と `Check again`・`checked_at` = 答えを読んだ時刻・`disabled` = Available `Off`）・acts = `CheckAgain`・`Disconnect`（止めていれば `Connect`）・`Remove`・鍵らしい env と `secret_env` ごとの `PasteSecret`・tool ごとの
/// `AllowTool`（`allowed` = 今許しているか）。`is_executable` だけで Ready と言わない（UV-5: 起こせない server を使える様に見せない）・program が無ければ起こさず Unavailable。
/// L4: Shibubu の server の後に、他の app の設定を読む手の行 key `mcp-found`（acts = `CheckAgain` =「Look in Claude's settings」= `found_config` を 1 回読む・
/// 開いただけでは読まない・見出しの `•••` に隠さず主な手にする）。
/// ② 他の app の設定（`found` = `found_config` の答え・まだ読んでいなければ空）にだけ在る server = key `mcp-found:<name>`・Available・detail に元の app の名前
/// （`In Claude Desktop's settings`・同じ名前が 2 つの app に在れば上の app の物）・acts = `Connect`（Shibubu の file へ写す）。
/// `checking` は今確かめている行の key。
/// 保証する: 起こさない（読むのは `list_tools_all` が覚えた答えだけ = 毎 frame 呼べる）・detail に引数と command の dir と env の値を出さない・Verified と言わない。
/// 契約（RQ-016a）: 引数 `checked`（行の key → 時刻）を外した。時刻は覚えた答えの方が正しい（Engine の run が読んだ答えにも付く）。
/// Wave: RQ4（RQ-016a は RQ4 の要件の id・Wave の §0 の行は RQ4）
pub fn connection_rows(engine: &McpConfig, found: &[(&str, McpConfig, std::collections::BTreeSet<String>)], checking: &std::collections::BTreeSet<String>) -> Vec<Connection> {
    use crate::ui::connections::ConnAct;
    let row = |key: String, name: String, state: DisplayTier, detail: String, acts: Vec<ConnAct>| Connection {
        checked_at: None,
        checking: checking.contains(&key),
        name: Cow::Owned(name),
        state,
        detail,
        recheck: false,
        settings: None,
        key: Cow::Owned(key),
        acts,
    };
    let mut rows = vec![row(
        "mcp".into(),
        "MCP servers".into(),
        DisplayTier::Available,
        match engine.mcp_servers.len() {
            0 => "No MCP servers yet · Add one with its command or JSON".into(),
            n => format!("{n} in Shibubu · Add another with its command or JSON"),
        },
        vec![ConnAct::Add],
    )];
    for (name, server) in &engine.mcp_servers {
        let cmd_name = command_name(&server.command);
        let key = format!("mcp:{name}");
        let mut acts = Vec::new();
        let mut checked_at = None;
        let (state, detail) = if server.disabled {
            acts.extend([ConnAct::Connect, ConnAct::Remove]);
            (DisplayTier::Available, "Off · Connect to use it again".to_string())
        } else {
            acts.extend([ConnAct::CheckAgain, ConnAct::Disconnect, ConnAct::Remove]);
            let secrets = server.secret_env.iter().chain(server.env.keys().filter(|k| looks_secret(k) && !server.secret_env.contains(k)));
            acts.extend(secrets.map(|k| ConnAct::PasteSecret(k.clone())));
            match known(name, server) {
                _ if !is_executable(&server.command) => (DisplayTier::Unavailable, format!("Not found: {cmd_name} (the program wasn't found) · Remove or install it")),
                // RQ-016a: program が在るだけ = まだ起こしていない（Ready と言わない・起こすのは `Check again`・`Add`・`Connect` を押した時だけ）
                None => (DisplayTier::Available, format!("Found {cmd_name} · not checked yet")),
                Some((at, r)) => {
                    checked_at = Some(crate::google::now() as i64 - at.elapsed().as_secs() as i64);
                    let allowed = server.allowed_tools.as_deref();
                    acts.extend(r.iter().flatten().map(|(t, _)| ConnAct::AllowTool { tool: t.name.clone(), allowed: tool_allowed(allowed, &t.name) }));
                    checked_state(&r, at.elapsed(), allowed)
                }
            }
        };
        rows.push(Connection { checked_at, ..row(key, format!("MCP: {name}"), state, detail, acts) });
    }
    rows.push(row(
        "mcp-found".into(),
        "MCP servers in Claude's settings".into(),
        DisplayTier::Available,
        "Claude Desktop and Claude Code · Shibubu reads them only when you press Look".into(),
        vec![ConnAct::CheckAgain],
    ));
    let mut shown = std::collections::BTreeSet::new();
    for (app, cfg, _) in found {
        for name in cfg.mcp_servers.keys().filter(|n| !engine.mcp_servers.contains_key(*n) && shown.insert(n.as_str())) {
            rows.push(row(format!("mcp-found:{name}"), format!("MCP: {name}"), DisplayTier::Available, format!("In {app}'s settings · Connect to add it to Shibubu"), vec![ConnAct::Connect]));
        }
    }
    rows
}

/// RQ-016a: `list_tools_all` の答え → MCP の行の状態と 1 行（Connections の行と `Check again` の結果が同じ言い方をする）。`age` = 答えを読んでからの長さ・
/// `allowed` = `McpServerConfig::allowed_tools`。tool が 1 つ以上 = Ready `{n} tools`（`READY_TTL` を過ぎたら Available と `Check again`）・0 = Connected（答えたが
/// tool が無い）・誤り = Available と理由と `Check again`（Unavailable にしない = 設定は在り、押せば確かめ直せる）。
/// 例: `let (tier, detail) = mcp::checked_state(&r, Duration::ZERO, None);`
/// 保証する: Verified と言わない（tool を呼んで結果を確かめていない）。
/// Wave: RQ4（RQ-016a）
pub fn checked_state(r: &Result<Vec<(McpTool, McpToolHints)>, McpError>, age: std::time::Duration, allowed: Option<&[String]>) -> (DisplayTier, String) {
    match r {
        Ok(tools) if tools.is_empty() => (DisplayTier::Connected, "Answered, but it offers no tools · Check again after you change it".into()),
        Ok(tools) => {
            let on = tools.iter().filter(|(t, _)| tool_allowed(allowed, &t.name)).count();
            let used = if on == tools.len() { String::new() } else { format!(" · {on} allowed") };
            if age < READY_TTL {
                (DisplayTier::Ready, format!("{} tools{used}", tools.len()))
            } else {
                (DisplayTier::Available, format!("{} tools{used} · Check again to confirm it still answers", tools.len()))
            }
        }
        Err(e) => (DisplayTier::Available, format!("{e} · Check again")),
    }
}

/// `allowed_tools` が None（全部）か、その名前を含むか
fn tool_allowed(allowed: Option<&[String]>, tool: &str) -> bool {
    allowed.is_none_or(|a| a.iter().any(|x| x == tool))
}

/// 鍵らしい env の名前（`Paste {name}` の button を出す・値は画面に出さない）
fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|w| n.contains(w))
}

/// 他の app の MCP の設定の置き場（画面に出す app の名前・`home` からの path）。上が先（同じ名前の server は上の app の物）。
const OTHER_APPS: [(&str, &str); 2] = [("Claude Desktop", "Library/Application Support/Claude/claude_desktop_config.json"), ("Claude Code", ".claude.json")];

/// 他の app（Claude Desktop・Claude Code）の設定に在る MCP の server（Connections の `mcp-found:` の行・`Connect` で Shibubu へ写す元）。
/// 呼ぶのは利用者が `mcp-found` の行の `Look in Claude's settings` を押した時だけ（Connections を開いただけでは他の app の file を開かない）。
/// 答え = file の在った app ごとの（名前・Shibubu が足せる server・足せない物の名前）。無い file の app は入れない。
/// 読む所 = 上の `mcpServers` と、Claude Code の Project ごとの `projects.<dir>.mcpServers`（`claude mcp add` の既定の scope はこちら・
/// 2026-10-31 この Mac の `~/.claude.json` で上は 0 本・Project の側に 3 本を実測）。同じ名前は先に読んだ足せる物（足せない物の後に足せる物が来れば足せる物）。
/// 足せない物 = command の無い server（URL の server）と形の違う 1 件（名前ごと）・開けない / JSON として読めない file は `<app> settings file` の 1 つ（1 件が壊れていても他は出す）。
/// interface（L4 周 2）: 足せない物の名前を足した = 「何も無い」と「足せない物だけ在る」を答えの 1 行（`found_line`）で分け、app を跨いで同じ名前を 1 つに数えるため。
/// Engine はこれを起こさない（`load_engine_config` だけ）。例: `let found = mcp::found_config(Path::new(&home));`
/// Wave: RQ4, L4
pub fn found_config(home: &Path) -> Vec<(&'static str, McpConfig, std::collections::BTreeSet<String>)> {
    OTHER_APPS
        .iter()
        .filter_map(|(app, f)| {
            let p = home.join(f);
            if !p.exists() {
                return None;
            }
            let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
                return Some((*app, McpConfig::default(), std::collections::BTreeSet::from([format!("{app} settings file")])));
            };
            let projects = v["projects"].as_object().into_iter().flat_map(|p| p.values().map(|d| &d["mcpServers"]));
            let (mut cfg, mut skipped) = (McpConfig::default(), std::collections::BTreeSet::new());
            for (name, s) in std::iter::once(&v["mcpServers"]).chain(projects).filter_map(|l| l.as_object()).flatten() {
                if cfg.mcp_servers.contains_key(name) {
                    continue;
                }
                match serde_json::from_value::<McpServerConfig>(s.clone()) {
                    Ok(s) => {
                        cfg.mcp_servers.insert(name.clone(), s);
                        skipped.remove(name);
                    }
                    Err(_) => {
                        skipped.insert(name.clone());
                    }
                }
            }
            Some((*app, cfg, skipped))
        })
        .collect()
}

/// L4: `Look in Claude's settings` の答えの 1 行（`found` = `found_config` の答え・`engine` = Shibubu の設定）。
/// 足せない物は app を跨いで名前ごとに 1 つ・どこかで足せる名前は数えない。足せない物が在る時は「全部 Shibubu に在る」と言わない・理由は分けない（URL の server と読めない file）。
/// Wave: L4
pub fn found_line(engine: &McpConfig, found: &[(&str, McpConfig, std::collections::BTreeSet<String>)]) -> String {
    let addable: std::collections::BTreeSet<&String> = found.iter().flat_map(|(_, c, _)| c.mcp_servers.keys()).collect();
    let new = addable.iter().filter(|n| !engine.mcp_servers.contains_key(**n)).count();
    let cant = found.iter().flat_map(|(_, _, k)| k).filter(|n| !addable.contains(n)).collect::<std::collections::BTreeSet<_>>().len();
    let cant = match cant {
        0 => String::new(),
        k => format!(" · {k} there Shibubu can't read or add"),
    };
    match (found.len(), new) {
        (0, _) => "No Claude Desktop or Claude Code settings on this Mac".into(),
        (_, 0) if cant.is_empty() => "Nothing new in Claude's settings · everything there is already in Shibubu".into(),
        (_, 0) => format!("Nothing to add from Claude's settings{cant}"),
        (_, n) => format!("Found {n} in Claude's settings · Connect to add one to Shibubu{cant}"),
    }
}

// ---- W19-2: 汎用の MCP の client（stdio・JSON-RPC 2.0）。Engine の Capability Discovery・recipe の Mcp の手・REA（W19-3）が同じ口を使う ----

/// MCP の server の tool 1 つ（`tools/list` の 1 件）。Engine の Capability Discovery が `CapabilityRecord`（kind = Mcp・`Origin::Official`）の候補にする。
/// Wave: W19-2
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から使う
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// `tools/list` の 1 件の `annotations`（MCP の ToolAnnotations・server の申告）のうち Engine が見る物。Engine は model の申告より厳しい側にだけ使う
/// （`read_only` の tool だけを読む action にできる・`destructive` の tool は消す扱い・解析の道具は `not_destructive` の tool だけ）。印が無ければ全部 false
/// （読むだけとは見なさない・消す扱いは書かれた時だけ・壊さないとも見なさない）。申告は server の物で、確かめた物ではない
/// （https://modelcontextprotocol.io/specification/2025-06-18/schema#toolannotations ・cutoff 2026-06 時点の知識）。
/// Wave: W19-2, W19-3
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から使う
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct McpToolHints {
    /// `annotations.readOnlyHint == true`
    pub read_only: bool,
    /// `annotations.destructiveHint == true`
    pub destructive: bool,
    /// `readOnlyHint == true` か、`destructiveHint == false` を明示した（MCP の既定は destructiveHint = true なので、書いていない tool は false）。
    /// W19-3: 本物の REA 6.7.0 の解析の tool は readOnlyHint = false・destructiveHint = false（docs/evidence/W19-3/rea-facts.md）
    pub not_destructive: bool,
}

/// MCP の client の誤り。Display は画面に出せる 1 行（server の env・鍵の値を入れない）。
/// Wave: W19-2
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から使う
#[derive(Clone, Debug, PartialEq)]
pub enum McpError {
    /// server を起こせない（command が無い・すぐ exit した）
    Spawn(String),
    /// JSON-RPC の形が違う・`initialize` が通らない
    Protocol(String),
    /// tool が誤りを返した（`isError` か JSON-RPC の error）
    Tool { code: i64, message: String },
    Timeout { ms: u64 },
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(why) => write!(f, "Can't start the MCP server ({why}). Check the command in the settings"),
            Self::Protocol(why) => write!(f, "Can't read the MCP server's answer ({why})"),
            Self::Tool { code, message } => write!(f, "The MCP tool returned an error ({code}: {message})"),
            Self::Timeout { ms } => write!(f, "The MCP server didn't answer within {} s", ms / 1000),
        }
    }
}
impl std::error::Error for McpError {}

/// `initialize` で送る MCP の版（https://modelcontextprotocol.io/specification/2025-06-18 ・cutoff 2026-06 時点の知識）。server が別の版を返しても受ける。
const PROTOCOL_VERSION: &str = "2025-06-18";

/// 起こした server 1 つとの session。`list_tools` / free の `call_tool` は 1 回の用件ごとに開いて閉じる。W19-3: 開いた target を process の中に持つ
/// tool（REA の `open_binary` → `inspect_artifact`）を続けて呼ぶため、1 本の解析の間だけ子を保つ口として公開する（新しい型ではなく、それまでの private の Session）。
/// drop で子を止めて待つ（子を残さない）。
/// Wave: W19-2, W19-3
/// 例: `let mut s = McpSession::open(&server, Duration::from_secs(90))?; s.call_tool("open_binary", &json!({ "path": app }), t)?; let r = s.call_tool("inspect_artifact", &json!({}), t)?; s.close();`
/// 保証する: 1 回の `call_tool` ごとに渡した待ちの上限を守る（前の呼びの時間を引かない）・tool の `isError` は `McpError::Tool`・`close` と drop は stdin を閉じて
/// 少し待ち、残っていれば kill して待つ（子を残さない）。保証しない: server が呼びの間で状態を保つ事（server の約束）・server が子の process を残さない事。
pub struct McpSession {
    child: std::process::Child,
    /// 閉じる時に先に落とす（stdin の EOF が MCP の stdio の終わりの合図）
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    deadline: std::time::Instant,
    ms: u64,
    next: u64,
    /// `initialize` の serverInfo.version（無ければ「不明」・W19-3 の解析の道具の版）
    version: String,
}

impl McpSession {
    /// 子を起こし `initialize` → `notifications/initialized`（`timeout` はここまでの上限）。env は親から鍵の変数（`llm_cli::SCRUBBED_ENV`）を外し、設定の `env` だけを足す。
    /// Wave: W19-2, W19-3
    pub fn open(server: &McpServerConfig, timeout: std::time::Duration) -> Result<McpSession, McpError> {
        use std::process::{Command, Stdio};
        let name = command_name(&server.command);
        let mut cmd = Command::new(&server.command);
        cmd.args(&server.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // L4: 子を自分の process group の頭にする（止める時に npx → node・uvx → python の孫まで group ごと止める = `kill_tree`）
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        for var in crate::llm_cli::SCRUBBED_ENV {
            cmd.env_remove(var);
        }
        cmd.envs(&server.env);
        let mut child = cmd.spawn().map_err(|e| McpError::Spawn(format!("{name}: {:?}", e.kind())))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            kill_tree(&mut child);
            return Err(McpError::Spawn(format!("{name}: can't start the connection")));
        };
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut s = McpSession { child, stdin: Some(stdin), lines, deadline: std::time::Instant::now() + timeout, ms: timeout.as_millis() as u64, next: 1, version: String::new() };
        let init = s.request(
            "initialize",
            serde_json::json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "shibubu", "version": env!("CARGO_PKG_VERSION") } }),
        )?;
        if !init["protocolVersion"].is_string() {
            return Err(McpError::Protocol("The server's first answer was incomplete".into()));
        }
        s.version = init["serverInfo"]["version"].as_str().filter(|v| !v.trim().is_empty()).unwrap_or("Unknown").chars().take(40).collect();
        s.send(&serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(s)
    }

    /// `initialize` の serverInfo.version（無ければ「不明」）。
    /// Wave: W19-3
    pub fn version(&self) -> &str {
        &self.version
    }

    /// 開いた session で `tools/call` を 1 回（待ちの上限は `timeout`・呼ぶ度に数え直す）。
    /// Wave: W19-3
    /// 保証する: tool の `isError` は `McpError::Tool`（成功と読まない）・timeout で子を止める（session は使えなくなる）。
    pub fn call_tool(&mut self, tool: &str, arguments: &serde_json::Value, timeout: std::time::Duration) -> Result<serde_json::Value, McpError> {
        self.call_until(tool, arguments, std::time::Instant::now() + timeout, timeout.as_millis() as u64)
    }

    /// 開いた session で `tools/list` を読む（`list_tools_hinted` と同じ読み方・印つき・W19-3-fix で `probe` を置き換えた）。解析の道具は版と tool を読んだ session をそのまま解析に使う
    /// （1 run で道具を 1 回だけ起こす・`capability_engine::analysis_backends`）。
    /// Wave: W19-3
    /// 保証する: `list_tools_hinted` と同じ（tool を呼ばない・20 頁まで・印が無ければ両方 false）・待ちの上限は `open` の `timeout` の残り。
    pub fn tools(&mut self) -> Result<Vec<(McpTool, McpToolHints)>, McpError> {
        tools_of(self)
    }

    /// session を終える: stdin を閉じて（MCP の stdio の終わり方）少し待ち、残っていれば kill して待つ。drop と同じ。
    /// Wave: W19-3
    pub fn close(self) {
        drop(self);
    }

    /// `tools/call` を `deadline` まで待つ（`ms` は Timeout の時に言う上限）。
    fn call_until(&mut self, tool: &str, arguments: &serde_json::Value, deadline: std::time::Instant, ms: u64) -> Result<serde_json::Value, McpError> {
        (self.deadline, self.ms) = (deadline, ms);
        let r = self.request("tools/call", serde_json::json!({ "name": tool, "arguments": arguments }))?;
        if r["isError"] == true {
            let text: Vec<&str> = r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
            return Err(McpError::Tool { code: 0, message: text.join(" ").chars().take(200).collect() });
        }
        Ok(r)
    }

    fn send(&mut self, msg: &serde_json::Value) -> Result<(), McpError> {
        use std::io::Write;
        let Some(stdin) = self.stdin.as_mut() else { return Err(McpError::Spawn("The session is closing".into())) };
        writeln!(stdin, "{msg}").and_then(|_| stdin.flush()).map_err(|e| match e.kind() {
            std::io::ErrorKind::BrokenPipe => McpError::Spawn("The server has ended".into()),
            k => McpError::Protocol(format!("Can't write to the server ({k})")),
        })
    }

    /// 1 つの依頼を送り、同じ id の答えを待つ（通知は読み捨て・server からの依頼は「対応しない」と返す・JSON でない行は読み捨て）。
    fn request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, McpError> {
        use std::sync::mpsc::RecvTimeoutError;
        let id = self.next;
        self.next += 1;
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let left = self.deadline.saturating_duration_since(std::time::Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => return Err(McpError::Timeout { ms: self.ms }),
                Err(RecvTimeoutError::Disconnected) => return Err(McpError::Spawn(format!("The server ended before it answered"))),
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if v.get("method").is_some() {
                if let Some(their) = v.get("id") {
                    self.send(&serde_json::json!({ "jsonrpc": "2.0", "id": their, "error": { "code": -32601, "message": "not supported" } }))?;
                }
                continue;
            }
            if v["id"] != serde_json::json!(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                return Err(McpError::Tool { code: e["code"].as_i64().unwrap_or(0), message: e["message"].as_str().unwrap_or("").chars().take(200).collect() });
            }
            return v.get("result").cloned().ok_or_else(|| McpError::Protocol(format!("The server's answer was empty")));
        }
    }
}

/// stdin を閉じてから server が自分で終わるのを待つ上限（REA 6.7.0 は 0.53 s で exit 0・docs/evidence/W19-3/rea-facts.md）。過ぎたら kill。
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

impl Drop for McpSession {
    /// MCP の stdio の終わり方（stdin を閉じる → 待つ → 終わらなければ止める）。kill は SIGKILL（SIGTERM の段は持たない・libc を足さない）。
    fn drop(&mut self) {
        drop(self.stdin.take());
        let end = std::time::Instant::now() + CLOSE_GRACE;
        while std::time::Instant::now() < end {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        kill_tree(&mut self.child);
    }
}

/// L4（DOGFOOD-X 周 8: 答えなかった npx・uvx の server の孫が残り load が下がらなかった）: 子と、子が起こした物をまとめて止める。子は `open` で自分の
/// process group の頭にしてあるので group ごと SIGKILL（libc を足さず /bin/kill）。頭がまだ止まっていない時だけ呼ぶ（group の番号が自分の子の物のうち）。
fn kill_tree(child: &mut std::process::Child) {
    let _ = std::process::Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
    let _ = child.kill();
    let _ = child.wait();
}

/// stdio の MCP の server を 1 回起こし、`initialize` → `tools/list` を読んで閉じる（1 行 1 message）。
/// Wave: W19-2
/// 例: `for (name, s) in &mcp::load_engine_config().unwrap_or_default().mcp_servers { let tools = mcp::list_tools(s, Duration::from_secs(10))?; }`
/// 保証する: `timeout` で子を止める・終わったら子を閉じる（残さない）・tool を呼ばない（一覧だけ）・`nextCursor` を 20 頁まで辿る。保証しない: 一覧の tool が副作用を持たない事（呼ぶ前に Engine の gate）。
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から呼ぶ
pub fn list_tools(server: &McpServerConfig, timeout: std::time::Duration) -> Result<Vec<McpTool>, McpError> {
    Ok(list_tools_hinted(server, timeout)?.into_iter().map(|(t, _)| t).collect())
}

/// `list_tools` と同じに読み、tool ごとに `annotations` の印（`McpToolHints`）を添える（Engine の Capability Discovery が使う）。
/// Wave: W19-2
/// 保証する: `list_tools` と同じ（timeout・子を閉じる・tool を呼ばない・20 頁まで）・`annotations` が無い・形が違う tool は印なし（両方 false）。
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から呼ぶ
pub fn list_tools_hinted(server: &McpServerConfig, timeout: std::time::Duration) -> Result<Vec<(McpTool, McpToolHints)>, McpError> {
    tools_of(&mut McpSession::open(server, timeout)?)
}

/// 開いた session で `tools/list` を `nextCursor` の 20 頁まで読む。
fn tools_of(s: &mut McpSession) -> Result<Vec<(McpTool, McpToolHints)>, McpError> {
    let shape = |e: serde_json::Error| McpError::Protocol(format!("The server's list of tools is in an unexpected form ({e})"));
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let params = cursor.as_ref().map_or_else(|| serde_json::json!({}), |c| serde_json::json!({ "cursor": c }));
        let page = s.request("tools/list", params)?;
        let got: Vec<serde_json::Value> = serde_json::from_value(page["tools"].clone()).map_err(shape)?;
        for v in got {
            let hint = |k: &str| v["annotations"][k] == true;
            let read_only = hint("readOnlyHint");
            let hints = McpToolHints { read_only, destructive: hint("destructiveHint"), not_destructive: read_only || v["annotations"]["destructiveHint"] == false };
            tools.push((serde_json::from_value::<McpTool>(v).map_err(shape)?, hints));
        }
        cursor = page["nextCursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

/// `tools/call` を 1 回（recipe の Mcp の手・REA の解析も同じ口・REA 専用の client を書かない）。
/// Wave: W19-2, W19-3
/// 保証する: `list_tools` と同じ（timeout・子を閉じる）・tool の `isError` は `McpError::Tool`（成功と読まない）。保証しない: 結果の中身の正しさ（呼ぶ側が読み戻す）。
#[allow(dead_code)] // capability_engine（UI への配線は W19-2 の後）から呼ぶ
pub fn call_tool(server: &McpServerConfig, tool: &str, arguments: &serde_json::Value, timeout: std::time::Duration) -> Result<serde_json::Value, McpError> {
    let deadline = std::time::Instant::now() + timeout;
    McpSession::open(server, timeout)?.call_until(tool, arguments, deadline, timeout.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_mcp_config() {
        let json = r#"{
            "mcpServers": {
                "reaper": {
                    "command": "/usr/local/bin/reaper-mcp-server",
                    "args": ["--port", "8080"]
                }
            }
        }"#;
        let cfg: McpConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.mcp_servers.contains_key("reaper"));
        let reaper = &cfg.mcp_servers["reaper"];
        assert_eq!(reaper.command, "/usr/local/bin/reaper-mcp-server");
        assert_eq!(reaper.args, vec!["--port", "8080"]);
    }

    /// RQ1 反証の直し: MCP の行（Connections と Settings › About の `Copy diagnostics`）は command の file 名と状態だけ。
    /// 引数は出さない（`--api-key x` や接続文字列の中の鍵が平文で clipboard に乗っていた）・見つからない command も dir を出さない。
    /// 負の対照: `connection_rows` の detail に `server.args.join(" ")` を戻す → 赤。
    #[test]
    fn rq1_rows_show_the_command_name_only() {
        let json = r#"{"mcpServers":{
            "context7":{"command":"sh","args":["-y","@upstash/context7-mcp","--api-key","ctx7sk-FAKE0000demo1111key2222","postgresql://u:pw0rdFAKE@db.local/x"]},
            "gone":{"command":"/Users/you/My Tools/srv","args":["--token","plainSecretValue123456"]}}}"#;
        let cfg: McpConfig = serde_json::from_str(json).unwrap();
        // RQ4: 頭の見出しの行を除く・確かめていない server は Ready と言わない（UV-5）
        let rows = connection_rows(&cfg, &[], &Default::default());
        let all = rows[1..].iter().filter(|r| r.key.starts_with("mcp:")).map(|r| format!("{}: {:?} · {}", r.name, r.state, r.detail)).collect::<Vec<_>>().join("\n");
        assert_eq!(all, "MCP: context7: Available · Found sh · not checked yet\nMCP: gone: Unavailable · Not found: srv (the program wasn't found) · Remove or install it");
    }

    /// RQ4 interface（caller の形・本体の前は todo!() で赤）: 1 つの欄の 2 つの形（command の行・README の JSON）から server を足し、止める・tool を許す・消すが
    /// file の形で往復する（既定の値は書かない = Claude Desktop と同じ形のまま）。
    #[test]
    fn rq4_add_from_text_reads_a_command_line_and_a_json_snippet() {
        let mut cfg = McpConfig::default();
        assert_eq!(cfg.add_from_text("npx -y @playwright/mcp").unwrap(), "playwright");
        assert_eq!(cfg.mcp_servers["playwright"].args, vec!["-y", "@playwright/mcp"]);
        let json = r#"{"mcpServers":{"penpot":{"command":"npx","args":["-y","@penpot/mcp"],"env":{"PENPOT_URL":"https://design.penpot.app"}}}}"#;
        assert_eq!(cfg.add_from_text(json).unwrap(), "penpot");
        assert_eq!(cfg.add_from_text("npx -y @playwright/mcp").unwrap(), "playwright-2", "同じ名前は -2");
        assert!(cfg.add_from_text("   ").is_err() && cfg.add_from_text(r#"{"x":{"args":[]}}"#).is_err());
        let plain = serde_json::to_string(&cfg).unwrap();
        assert!(!plain.contains("disabled") && !plain.contains("allowedTools") && !plain.contains("secretEnv"), "既定の値を書いた: {plain}");
        cfg.set_disabled("penpot", true).unwrap();
        cfg.allow_tool("playwright", "browser_close", false, &["browser_navigate".into(), "browser_close".into()]).unwrap();
        assert_eq!(cfg.mcp_servers["playwright"].allowed_tools.as_deref(), Some(&["browser_navigate".to_string()][..]));
        cfg.allow_tool("playwright", "browser_close", true, &["browser_navigate".into(), "browser_close".into()]).unwrap();
        assert_eq!(cfg.mcp_servers["playwright"].allowed_tools, None, "全部許したら None に戻す");
        let back: McpConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
        assert!(cfg.remove("playwright-2").is_ok() && cfg.remove("playwright-2").is_err() && cfg.set_disabled("ghost", true).is_err());
    }
}

/// W19-2 の MCP の client の署名を compile で縛り、保存形式（tools/list の 1 件の serde）だけを走らせる（本体を通すのは下の `w19_2_body`）。
#[cfg(test)]
mod w19_2_interface {
    use super::*;
    #[test]
    fn w19_2_mcp_client_signatures_and_tool_shape() {
        let _: fn(&McpServerConfig, std::time::Duration) -> Result<Vec<McpTool>, McpError> = list_tools;
        let _: fn(&McpServerConfig, &str, &serde_json::Value, std::time::Duration) -> Result<serde_json::Value, McpError> = call_tool;
        // W19-3: 1 本の解析の間だけ子を保つ session の口
        let _: fn(&McpServerConfig, std::time::Duration) -> Result<McpSession, McpError> = McpSession::open;
        let _: fn(&mut McpSession, &str, &serde_json::Value, std::time::Duration) -> Result<serde_json::Value, McpError> = McpSession::call_tool;
        let _: fn(&McpSession) -> &str = McpSession::version;
        let _: fn(McpSession) = McpSession::close;
        // W19-3-fix: 版と tool を読んだ session をそのまま解析に使う口
        let _: fn(&mut McpSession) -> Result<Vec<(McpTool, McpToolHints)>, McpError> = McpSession::tools;
        let t: McpTool = serde_json::from_value(serde_json::json!({ "name": "analyze", "inputSchema": { "type": "object" } })).unwrap();
        assert_eq!((t.name.as_str(), t.description.as_str(), t.input_schema["type"].as_str()), ("analyze", "", Some("object")), "MCP の inputSchema の名前で読む");
    }
}

/// W19-2 の MCP の client の本体を、試験の側で起こした local の MCP server（tests/fake-mcp-server.py・架空の app Quillpad）で通す。外の server は使わない。
#[cfg(test)]
pub(crate) mod w19_2_body {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    pub(crate) fn quill(mode: &str, log: &Path) -> McpServerConfig {
        McpServerConfig {
            command: "python3".into(),
            args: vec!["-I".into(), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mcp-server.py").into()],
            env: [("FAKE_MCP_MODE".to_string(), mode.to_string()), ("FAKE_MCP_LOG".to_string(), log.display().to_string())].into_iter().collect(),
            ..Default::default()
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-mcp-{name}-{}-{}", std::process::id(), crate::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn w19_2_mcp_lists_and_calls_a_local_server_and_reads_tool_errors_as_errors() {
        let d = dir("ok");
        let log = d.join("calls.jsonl");
        let doc = d.join("a.quill.json");
        std::fs::write(&doc, r#"{"title":"Old","body":"b"}"#).unwrap();
        let s = quill("", &log);
        let tools = list_tools(&s, Duration::from_secs(20)).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["quill_read", "quill_set_title"]);
        assert_eq!(tools[1].input_schema["required"], json!(["path", "title"]), "inputSchema を読む");
        assert!(!log.exists(), "tools/list は tool を呼ばない");
        let r = call_tool(&s, "quill_set_title", &json!({ "path": doc, "title": "New" }), Duration::from_secs(20)).unwrap();
        assert_eq!(r["content"][0]["text"], "saved");
        let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&doc).unwrap()).unwrap();
        assert_eq!(on_disk["title"], "New", "server の側の file が変わった（試験の側で disk を読む）");
        let read = call_tool(&s, "quill_read", &json!({ "path": doc }), Duration::from_secs(20)).unwrap();
        assert_eq!(read["structuredContent"]["title"], "New");
        // tool の isError は成功と読まない・JSON-RPC の error も Tool
        let missing = call_tool(&s, "quill_read", &json!({ "path": d.join("none.json") }), Duration::from_secs(20));
        assert!(matches!(&missing, Err(McpError::Tool { code: 0, message }) if message.contains("FileNotFoundError")), "{missing:?}");
        let unknown = call_tool(&s, "quill_delete", &json!({}), Duration::from_secs(20));
        assert!(matches!(unknown, Err(McpError::Tool { code: -32602, .. })), "{unknown:?}");
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 3, "呼んだ tool は 3 回だけ（list は数えない・知らない tool は server が断った）");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_2_mcp_hints_come_from_the_server_annotations() {
        let d = dir("hints");
        let log = d.join("calls.jsonl");
        let got = list_tools_hinted(&quill("", &log), Duration::from_secs(20)).unwrap();
        let hints: Vec<(&str, McpToolHints)> = got.iter().map(|(t, h)| (t.name.as_str(), *h)).collect();
        let ro = McpToolHints { read_only: true, destructive: false, not_destructive: true };
        // quill_set_title は readOnlyHint = false・destructiveHint = false を明示（壊さないと言う・読むだけとは言わない）
        assert_eq!(hints, [("quill_read", ro), ("quill_set_title", McpToolHints { not_destructive: true, ..McpToolHints::default() })], "readOnlyHint が true の tool だけ");
        assert_eq!(got.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(), list_tools(&quill("", &log), Duration::from_secs(20)).unwrap(), "tool の形は list_tools と同じ");
        assert!(!log.exists(), "tool を呼ばない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// L4（DOGFOOD-X 周 8）: `list_tools_all` が起こすのは同時に `MCP_AT_ONCE`（2）本まで。各 server を sh で包み、起きた時に `+`・終わった時に `-` を書いて重なりを数える
    /// （`-` は server が stdin の終わりで止まった後・枠を返すのはその後）。負の対照: `MCP_AT_ONCE` を 6 にする → 6 本が同時に起きて赤。
    #[test]
    fn l4_list_tools_all_starts_at_most_two_servers_at_once() {
        let d = dir("l4-at-once");
        let log = d.join("live.log");
        let mut cfg = McpConfig::default();
        for i in 0..6 {
            let q = quill("", &d.join("calls.jsonl"));
            let mut args = vec!["-c".to_string(), "echo + >> \"$0\"; \"$@\"; echo - >> \"$0\"".into(), log.display().to_string(), q.command.clone()];
            args.extend(q.args);
            cfg.mcp_servers.insert(format!("l4-at-once-{i}"), McpServerConfig { command: "/bin/sh".into(), args, env: q.env, ..Default::default() });
        }
        let got = list_tools_all(&cfg, Duration::from_secs(20));
        assert!(got.values().all(|r| r.is_ok()), "{got:?}");
        let marks = std::fs::read_to_string(&log).unwrap();
        let (mut live, mut most) = (0i32, 0i32);
        for m in marks.lines() {
            live += if m == "+" { 1 } else { -1 };
            most = most.max(live);
        }
        assert_eq!(marks.lines().filter(|m| *m == "+").count(), 6, "{marks:?}");
        assert!(most <= 2, "同時に {most} 本起こした: {marks:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// L4（DOGFOOD-X 周 8）: 答えなかった server は、起こした子だけでなく孫（npx → node・uvx → python）まで止める（子は自分の process group の頭）。
    /// server = sh が孫の sleep を起こして pid を書き、答えずに待つ。負の対照: group を止める行を外す → sh だけ止まり、孫が残って赤。
    #[test]
    fn l4_a_timed_out_server_leaves_no_children_running() {
        let d = dir("l4-tree");
        let pidfile = d.join("grandchild.pid");
        let s = McpServerConfig { command: "/bin/sh".into(), args: vec!["-c".into(), "sleep 300 & echo $! > \"$0\"; sleep 300".into(), pidfile.display().to_string()], ..Default::default() };
        let cfg = McpConfig { mcp_servers: [("l4-tree".to_string(), s)].into_iter().collect() };
        let got = list_tools_all(&cfg, Duration::from_millis(1500));
        assert!(matches!(got["l4-tree"], Err(McpError::Timeout { .. })), "{got:?}");
        let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_string();
        let alive = || std::process::Command::new("/bin/kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        let t = std::time::Instant::now();
        while alive() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let left = alive();
        if left {
            let _ = std::process::Command::new("/bin/kill").args(["-KILL", &pid]).status(); // 試験が起こした孫だけ
        }
        assert!(!left, "答えなかった server の孫（sleep・pid {pid}）が残った");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// L4（DOGFOOD-X 周 2）: 他の app の設定から見つけた行は元の app の名前を出し（Claude Desktop / Claude Code）、Shibubu に在る server は出さない。
    /// 「n in Shibubu」は Shibubu の設定だけを数える。`found_config` は在る file の app だけを上から返し、Project ごとの server も読み、足せない物を数える。
    /// 負の対照: 見つけた行の detail を前の「In Claude's settings」に戻す → 赤。`projects` を読まない → Claude Code の c が無く赤。
    #[test]
    fn l4_found_rows_name_the_app_they_came_from() {
        let cfg = |names: &[&str]| McpConfig { mcp_servers: names.iter().map(|n| (n.to_string(), McpServerConfig { command: "/usr/bin/true".into(), ..Default::default() })).collect() };
        let found = [("Claude Desktop", cfg(&["a", "b"]), Default::default()), ("Claude Code", cfg(&["b", "c"]), Default::default())];
        let rows = connection_rows(&cfg(&["a"]), &found, &std::collections::BTreeSet::new());
        let got: Vec<(&str, &str)> = rows.iter().map(|r| (r.key.as_ref(), r.detail.as_str())).filter(|(k, _)| k.starts_with("mcp-found:") || *k == "mcp").collect();
        assert_eq!(
            got,
            [
                ("mcp", "1 in Shibubu · Add another with its command or JSON"),
                ("mcp-found:b", "In Claude Desktop's settings · Connect to add it to Shibubu"),
                ("mcp-found:c", "In Claude Code's settings · Connect to add it to Shibubu"),
            ]
        );
        let look = rows.iter().find(|r| r.key == "mcp-found").map(|r| r.acts.clone());
        assert_eq!(look, Some(vec![crate::ui::connections::ConnAct::CheckAgain]), "他の app の設定を見る手の行が無い");
        let h = dir("l4-found");
        assert!(found_config(&h).is_empty(), "file の無い app を返した");
        let desk = h.join("Library/Application Support/Claude/claude_desktop_config.json");
        std::fs::create_dir_all(desk.parent().unwrap()).unwrap();
        std::fs::write(&desk, r#"{"mcpServers":{"x":{"command":"/usr/bin/true"}}}"#).unwrap();
        // Claude Code: 上の `mcpServers` と Project ごとの `projects.<dir>.mcpServers`・URL の server（command 無し）は足せない数に
        std::fs::write(
            h.join(".claude.json"),
            r#"{"mcpServers":{"u":{"command":"/usr/bin/true"},"g":{"type":"http","url":"https://example.com/g"}},"projects":{"/p":{"mcpServers":{"g":{"command":"/usr/bin/true"},"c":{"command":"/usr/bin/true"},"web":{"type":"http","url":"https://example.com/mcp"}}},"/q":{"mcpServers":{"web":{"type":"http","url":"https://example.com/mcp"}}},"/r":{}}}"#,
        )
        .unwrap();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let read = |h: &Path| -> Vec<(&str, Vec<String>, Vec<String>)> { found_config(h).into_iter().map(|(a, c, k)| (a, c.mcp_servers.into_keys().collect(), k.into_iter().collect())).collect() };
        // g: 上は URL・Project の側は command → 足せる物（足せない数に残さない）
        assert_eq!(read(&h), [("Claude Desktop", s(&["x"]), s(&[])), ("Claude Code", s(&["c", "g", "u"]), s(&["web"]))]);
        std::fs::write(&desk, "{").unwrap();
        assert_eq!(read(&h)[0], ("Claude Desktop", s(&[]), s(&["Claude Desktop settings file"])), "読めない file を「何も無い」にした");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&desk, std::fs::Permissions::from_mode(0o000)).unwrap();
        let locked = read(&h)[0].clone();
        std::fs::set_permissions(&desk, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(locked, ("Claude Desktop", s(&[]), s(&["Claude Desktop settings file"])), "開けない file を「無い」にした");
        let _ = std::fs::remove_dir_all(&h);
    }

    /// L4（DOGFOOD-X 周 2・review）: Look の答えの 1 行は、足せない物が在る時に「everything there is already in Shibubu」と言わず、
    /// 同じ名前の足せない物を app を跨いで 1 つに数え、どこかで足せる名前は足せない数に入れない。
    /// 負の対照: `if cant.is_empty()` の門を外す → 足せない物だけの時に「全部在る」と言い赤。名前の集合を数でなく件数にする → 2 と数え赤。
    #[test]
    fn l4_look_line_never_says_all_there_when_some_cant_be_added() {
        let cfg = |names: &[&str]| McpConfig { mcp_servers: names.iter().map(|n| (n.to_string(), McpServerConfig { command: "/usr/bin/true".into(), ..Default::default() })).collect() };
        let k = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<std::collections::BTreeSet<_>>();
        let a = cfg(&["a"]);
        assert_eq!(found_line(&a, &[]), "No Claude Desktop or Claude Code settings on this Mac");
        assert_eq!(found_line(&a, &[("Claude Desktop", cfg(&["a"]), k(&[]))]), "Nothing new in Claude's settings · everything there is already in Shibubu");
        assert_eq!(
            found_line(&a, &[("Claude Desktop", cfg(&["a"]), k(&["web"])), ("Claude Code", cfg(&[]), k(&["web"]))]),
            "Nothing to add from Claude's settings · 1 there Shibubu can't read or add"
        );
        assert_eq!(found_line(&a, &[("Claude Desktop", cfg(&[]), k(&["web"])), ("Claude Code", cfg(&["web"]), k(&[]))]), "Found 1 in Claude's settings · Connect to add one to Shibubu");
        assert_eq!(
            found_line(&a, &[("Claude Desktop", cfg(&["b"]), k(&["Claude Desktop settings file"]))]),
            "Found 1 in Claude's settings · Connect to add one to Shibubu · 1 there Shibubu can't read or add"
        );
    }

    /// Engine は他の app（Claude Desktop・Claude Code）の MCP の設定を読まない（そこに在る server を Engine が勝手に起こさない）。
    #[test]
    fn w19_2_engine_config_reads_only_shibubu_own_file() {
        let h = dir("home");
        let cfg = r#"{"mcpServers":{"x":{"command":"/usr/bin/true"}}}"#;
        for f in ["Library/Application Support/Claude/claude_desktop_config.json", ".claude.json"] {
            let p = h.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, cfg).unwrap();
        }
        let home = Some(h.display().to_string());
        assert_eq!(engine_config_path(None, home.clone()), None, "他の app の設定しか無ければ使わない");
        let own = h.join(".shibubu/mcp.json");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, cfg).unwrap();
        assert_eq!(engine_config_path(None, home.clone()), Some(own.clone()));
        let env = h.join("chosen.json");
        std::fs::write(&env, cfg).unwrap();
        assert_eq!(engine_config_path(Some(env.display().to_string()), home.clone()), Some(env), "利用者が環境変数で選んだ file が先");
        // L4（DOGFOOD-X 周 2）: 無い file を指す環境変数は空（~/.shibubu/mcp.json に戻らない = 新しい data dir の QA が利用者の 10 本を見ない）・空の値は選んでいない扱い
        // 負の対照: `engine_config_path` の Some の腕に `.or_else(|| home の ~/.shibubu/mcp.json)` を戻す → 1 つ目が Some(own) で赤。
        assert_eq!(engine_config_path(Some(h.join("none.json").display().to_string()), home.clone()), None, "無い file を指す環境変数で利用者の file に戻った");
        assert_eq!(engine_config_path(Some(" ".into()), home), Some(own), "空の環境変数は選んでいない扱い");
        let _ = std::fs::remove_dir_all(&h);
    }

    /// RQ4 interface（caller の形・本体の前は todo!() で赤）: 10 server のうち 1 つが黙っても、`list_tools_all` は全部の答えを返し（L4: 2 本ずつなので黙った 1 本は
    /// 片方の枠だけを塞ぐ・上限 30 秒）、同じ設定の 2 回目は 1 つも起こさない（`forget_tools` の後は起こし直す）・`disabled` は起こさない。起こした回数は sh が 1 行書いてから exec で数える。
    /// 負の対照（本体の commit）: 覚えを外す → 2 回目に起こして赤。同時に起こす本数は `l4_list_tools_all_starts_at_most_two_servers_at_once`。
    #[test]
    fn rq4_list_tools_all_is_parallel_and_does_not_restart_on_the_second_run() {
        let d = dir("rq4-par");
        let starts = d.join("starts.log");
        let counted = |mode: &str| {
            let q = quill(mode, &d.join("calls.jsonl"));
            let mut args = vec!["-c".to_string(), "echo start >> \"$0\"; exec \"$@\"".into(), starts.display().to_string(), q.command.clone()];
            args.extend(q.args);
            McpServerConfig { command: "/bin/sh".into(), args, env: q.env, ..Default::default() }
        };
        let mut cfg = McpConfig::default();
        for i in 0..9 {
            cfg.mcp_servers.insert(format!("ok{i}"), counted(""));
        }
        cfg.mcp_servers.insert("quiet".into(), counted("silent"));
        cfg.mcp_servers.insert("off".into(), McpServerConfig { disabled: true, ..counted("") });
        let n = || std::fs::read_to_string(&starts).map(|t| t.lines().count()).unwrap_or(0);
        let t = std::time::Instant::now();
        let first = list_tools_all(&cfg, Duration::from_secs(10));
        assert!(t.elapsed() < Duration::from_secs(30), "黙った 1 本の後ろで止まった: {:?}", t.elapsed());
        assert_eq!(first.len(), 10, "disabled を起こした / 答えが足りない: {:?}", first.keys().collect::<Vec<_>>());
        assert!(first["ok0"].is_ok() && matches!(first["quiet"], Err(McpError::Timeout { .. })));
        assert_eq!(n(), 10);
        let second = list_tools_all(&cfg, Duration::from_secs(10));
        assert_eq!((second.len(), n()), (10, 10), "2 回目に起こし直した");
        forget_tools(Some("ok0"));
        let _ = list_tools_all(&cfg, Duration::from_secs(10));
        assert_eq!(n(), 11, "forget_tools の後に起こし直さない");
    }

    /// RQ-016a: Connections の MCP の行は program が在るだけでは Ready と言わない。確かめ（`list_tools_all` = 行の `Check again` の中身）が答えを読んで初めて上がる:
    /// 未確認 = Available `Found … · not checked yet` → 答えて tool 0 = Connected → tool 1 つ以上 = Ready `{n} tools`（`checked_at` = 答えを読んだ時刻）→
    /// `READY_TTL` を過ぎたら Available と `Check again`。黙った server は上限で止めて Available と理由の 1 行（Unavailable にしない）。どの行も Verified と言わない。
    /// 負の対照（この commit）: 未確認の腕を Ready に戻す → 1 つ目の assert で赤・TTL の比較を外す → 古い答えが Ready のまま赤・誤りを Unavailable に戻す → 黙った行で赤。
    #[test]
    fn rq016a_mcp_rows_climb_from_found_to_ready_and_drop_back() {
        use std::collections::BTreeSet;
        let d = dir("rq016a");
        let log = d.join("calls.jsonl");
        let one = |name: &str, mode: &str| McpConfig { mcp_servers: [(name.to_string(), quill(mode, &log))].into_iter().collect() };
        let (talk, quiet) = (one("rq016a-ok", ""), one("rq016a-quiet", "silent"));
        let mut all = one("rq016a-none", "notools");
        all.mcp_servers.extend(talk.mcp_servers.clone().into_iter().chain(quiet.mcp_servers.clone()));
        let rows = || -> BTreeMap<String, (DisplayTier, String, Option<i64>)> {
            connection_rows(&all, &[], &BTreeSet::new()).into_iter().map(|r| (r.key.to_string(), (r.state, r.detail, r.checked_at))).collect()
        };
        let row = |rows: &BTreeMap<String, (DisplayTier, String, Option<i64>)>, k: &str| rows[&format!("mcp:rq016a-{k}")].clone();
        let before = rows();
        assert_eq!(row(&before, "ok"), (DisplayTier::Available, "Found python3 · not checked yet".into(), None), "program が在るだけで Ready と言った");
        let _ = list_tools_all(&talk, Duration::from_secs(20));
        let _ = list_tools_all(&one("rq016a-none", "notools"), Duration::from_secs(20));
        let silent = list_tools_all(&quiet, Duration::from_millis(1500));
        assert!(matches!(silent["rq016a-quiet"], Err(McpError::Timeout { ms: 1500 })), "{silent:?}");
        let after = rows();
        let now = crate::google::now() as i64;
        for k in ["ok", "none", "quiet"] {
            assert!(row(&after, k).2.is_some_and(|at| (now - at).abs() < 60), "確かめた時刻が無い: {k} {:?}", row(&after, k));
        }
        assert_eq!(row(&after, "ok").0, DisplayTier::Ready, "{:?}", row(&after, "ok"));
        assert_eq!(row(&after, "ok").1, "2 tools");
        assert_eq!((row(&after, "none").0, row(&after, "none").1), (DisplayTier::Connected, "Answered, but it offers no tools · Check again after you change it".into()));
        assert_eq!((row(&after, "quiet").0, row(&after, "quiet").1), (DisplayTier::Available, "The MCP server didn't answer within 1 s · Check again".into()));
        age_tools("rq016a-ok", READY_TTL);
        let stale = rows();
        assert_eq!((row(&stale, "ok").0, row(&stale, "ok").1), (DisplayTier::Available, "2 tools · Check again to confirm it still answers".into()), "古い答えで Ready と言った");
        assert!([before, after, stale].iter().flat_map(|r| r.values()).all(|r| r.0 != DisplayTier::Verified), "Connections の行が Verified と言った");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn w19_2_mcp_spawn_failure_exit_and_silence_are_errors_not_empty_lists() {
        let d = dir("bad");
        let log = d.join("calls.jsonl");
        let none = McpServerConfig { command: "/nonexistent/shibubu-mcp".into(), ..Default::default() };
        assert!(matches!(list_tools(&none, Duration::from_secs(5)), Err(McpError::Spawn(_))));
        assert!(matches!(list_tools(&quill("exit", &log), Duration::from_secs(20)), Err(McpError::Spawn(_))), "答えずに終わった server");
        let t = std::time::Instant::now();
        let silent = list_tools(&quill("silent", &log), Duration::from_millis(1500));
        assert_eq!(silent, Err(McpError::Timeout { ms: 1500 }));
        assert!(t.elapsed() < Duration::from_secs(10), "timeout で子を止めて戻る（server の 30 秒の sleep を待たない）");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// W19-3: 1 本の解析の間だけ子を保つ session を、偽物の REA の役（tests/fake-mcp-server.py・本物の REA 6.7.0 の一覧の写しと、`open_binary` で開いた target を
/// process の中にだけ持つ契約）で通す。本物の REA は呼ばない（本物は capability_engine の `w19_3_real`・`#[ignore]`）。
#[cfg(test)]
mod w19_3_body {
    use super::w19_2_body::quill;
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-mcp-w19-3-{name}-{}-{}", std::process::id(), crate::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rea(d: &Path) -> McpServerConfig {
        let mut s = quill("", &d.join("calls.jsonl"));
        s.env.insert("FAKE_MCP_ROLE".into(), "rea".into());
        s.env.insert("FAKE_MCP_EXIT_LOG".into(), d.join("exit.log").display().to_string());
        s
    }

    /// 見本の束（Info.plist の CFBundleExecutable が Mach-O〈/usr/bin/true の写し〉を指す・起動しない）。
    fn bundle(d: &Path) -> PathBuf {
        let app = d.join("Pad.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), "<plist><dict><key>CFBundleExecutable</key><string>Pad</string></dict></plist>").unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/Pad")).unwrap();
        app
    }

    fn tool_error(r: &Result<serde_json::Value, McpError>, has: &str) -> bool {
        matches!(r, Err(McpError::Tool { message, .. }) if message.contains(has))
    }

    #[test]
    fn w19_3_session_keeps_the_opened_target_until_closed_by_stdin() {
        let d = dir("session");
        let (app, s, t) = (bundle(&d), rea(&d), Duration::from_secs(20));
        // 負の対照: 呼ぶ度に process を起こす口（free の call_tool）では、開いた target が次の呼びに残らない
        call_tool(&s, "open_binary", &json!({ "path": app }), t).unwrap();
        let alone = call_tool(&s, "inspect_artifact", &json!({}), t);
        assert!(tool_error(&alone, "No app is open"), "{alone:?}");

        let mut ss = McpSession::open(&s, t).unwrap();
        assert_eq!(ss.version(), "6.7.0");
        let opened = ss.call_tool("open_binary", &json!({ "path": app }), t).unwrap();
        assert_eq!(opened["structuredContent"]["result"]["kind"], "executable", "{opened}");
        let r = ss.call_tool("inspect_artifact", &json!({}), t).unwrap();
        let files: Vec<&str> = r["structuredContent"]["normalized_result"]["substeps"][0]["evidence"]["normalized_result"]["occurrences"].as_array().unwrap().iter().filter_map(|o| o["logical_path"].as_str()).collect();
        assert!(files.contains(&"Contents/MacOS/Pad"), "同じ session で開いた束を読んだ: {files:?}");
        let bad = ss.call_tool("inspect_artifact", &json!({ "path": app }), t);
        assert!(tool_error(&bad, "Unrecognized key"), "本物と同じく知らない key は tool の誤り: {bad:?}");
        assert!(ss.call_tool("inspect_artifact", &json!({}), t).is_ok(), "tool の誤りの後も session は使える");
        let exits = || std::fs::read_to_string(d.join("exit.log")).unwrap_or_default().lines().count();
        let before = exits();
        ss.close();
        assert_eq!(exits(), before + 1, "close は stdin を閉じ、server が自分で終わった（kill ではない）");

        // 閉じた session の target は次の session に無い
        let mut again = McpSession::open(&s, t).unwrap();
        let after = again.call_tool("inspect_artifact", &json!({}), t);
        assert!(tool_error(&after, "No app is open"), "{after:?}");
        drop(again);
        assert_eq!(exits(), before + 2, "drop も同じ閉じ方");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 本物の REA 6.7.0 の tools/list の写し（tests/fixtures/rea/tools-list-6.7.0.json）: 使う 5 つの tool は壊さないと申告し（読むだけとは言わない）、
    /// close_binary は壊すと申告する・`ReaBackend` が渡す引数の key が本物の inputSchema に在る。
    #[test]
    fn w19_3_probe_reads_the_real_rea_tool_list_and_its_self_declared_hints() {
        let fx: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/rea/tools-list-6.7.0.json")).unwrap();
        let d = dir("list");
        let mut s = McpSession::open(&rea(&d), Duration::from_secs(20)).unwrap();
        let tools = s.tools().unwrap();
        assert_eq!(s.version(), fx["serverInfo"]["version"]);
        assert_eq!(tools.len(), fx["tools"].as_array().unwrap().len(), "全頁を読んだ");
        let get = |n: &str| tools.iter().find(|(t, _)| t.name == n).unwrap_or_else(|| panic!("{n} が一覧に無い"));
        for n in crate::intel::analysis::REA_ALLOWED {
            assert!(get(n).1.not_destructive, "{n}: {:?}", get(n).1);
        }
        assert_eq!(get("open_binary").1, McpToolHints { read_only: false, destructive: false, not_destructive: true }, "本物の open_binary は読むだけとは言わない");
        assert_eq!(get("close_binary").1, McpToolHints { read_only: false, destructive: true, not_destructive: false });
        let schema = |n: &str| get(n).0.input_schema.clone();
        assert_eq!(schema("open_binary")["required"], json!(["path"]));
        assert!(schema("inspect_artifact")["properties"].get("path").is_none() && schema("inspect_artifact")["additionalProperties"] == false, "inspect_artifact は path を取らない（開いた target を読む）");
        assert!(schema("inspect_macho")["properties"].get("detail").is_some());
        assert_eq!(schema("analyze_javascript_application")["required"], json!(["input_path"]));
        assert!(!d.join("calls.jsonl").exists(), "tool を呼ばない");
        let _ = std::fs::remove_dir_all(&d);
    }
}
