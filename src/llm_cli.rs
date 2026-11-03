//! W13: 利用者が既に login している LLM の CLI（今は Claude Code の `claude -p`）を Shibubu の PTY から起こす provider（YC_WAVES の W13 の行）。
//! 既定は鍵を持たない: API key・REGISTRY・keys.env を読まない。子の env から鍵の変数（`SCRUBBED_ENV`）を外して起こし、plan の枠の login だけを使う。
//! RQ2（YC_DECISIONS 89 通目）: 利用者が Settings › AI & Models で選んだ時だけ、選んだ account の分を子に渡す（`child_env`・plan の別 account は `CLAUDE_CONFIG_DIR`・
//! 鍵は Keychain から `ANTHROPIC_API_KEY` に）。選ばない限り今の振る舞い（System default = 利用者の Claude Code の login）のまま。
//! `llm::Llm` を実装するので、呼び手は今の `complete_json`（送る前の schema の検査・返った JSON の検証）をそのまま通る。
//!
//! PTY の使い方: 在りかは利用者の対話の shell（`TerminalConfig::default()` の `-i`・rc の PATH）で `command -v` を引く（Finder から開いた .app の PATH には
//! `~/.local/bin` などが無い）。呼び出しは Project の Terminal と別の、この provider だけの PTY の session で、rc を読まない shell（`-f`）に
//! `command_line` を打ち、prompt は file から stdin へ・答えは file へ（PTY の echo と prompt を JSON に混ぜない・W10 の VT・W11 の lease に触れない）。
//! 回数は 1 回ごとに `CallRecord` を 1 行 `CliConfig::log` へ足す（試験の側の run.sh が run ごと・run をまたいで数える・YC_WAVES の W13 の上限）。
//!
//! 事実（2026-10-21・claude 2.1.307 の `--help`）: `--bare` は OAuth と keychain を読まず `ANTHROPIC_API_KEY` だけで認証する＝鍵で使った分の請求になるので使わない（YCM-066 の語は使わない）。
//! `--json-schema` は `--print` の時だけ。`claude auth status --json` は model を呼ばずに `loggedIn`・`authMethod`（plan の login は `"claude.ai"`）を返す
//! （email・org も返すが読まない・保存しない）。
//! 実測（2026-10-21・claude 2.1.307・plan の login・この file の command_line と同じ旗で 1 回）: 構造化の JSON は `structured_output`（object）に入り、
//! `result` にも同じ物が文字列で入る（`parse_result` は前者を先に読む）。prompt は stdin から読む（stdin に置いた語を答えに返した）。入力は約 1,240 token
//! （cache の作成込み）で、利用者の `~/.claude/CLAUDE.md`（数万 token）は混ざっていない（model も「見えない」と答えた）。利用者の hook の記録 file は
//! 呼ぶ前後で変わらなかった（`--setting-sources local` で user の設定の hook は読まれない）。model は利用者の既定（その回は claude-opus-5-5）。
//! **未測:** `authMethod` の plan 以外の値（API key・他社の cloud の login を持つ機械が無い）・使用量の上限に当たった時の答えの文言。
//!
//! 107 通目: 在りかの file 名が `grok` の時だけ、試験の本物の AI は Grok（空の HOME・空の GROK_HOME・login は `GROK_AUTH_PATH` の auth.json だけ・tool は `todo_write` だけ）。
//! claude の argv は変えない。
//! PROVIDER-1101（owner 180 通目）: file 名が kimi・opencode・cursor-agent の時もその CLI の非対話の呼び方で呼ぶ（`FOUND_CLIS`・`prepare_other_env` の空の HOME・tool 無し）。
//! Provider の行は機械で見つけた AI を全部（login していない CLI も `needs_sign_in` の行）・model と effort はその AI から読んだ一覧（`HelpFlags`）。

use crate::domain::project::TerminalSessionSpec;
use crate::llm::{JsonRequest, Llm, LlmError};
use crate::terminal::{
    seed_path, SessionKey, TermSize, TerminalConfig, TerminalError, TerminalManager,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 在りかの口。未設定 = 利用者の shell で `DEFAULT_PROGRAM` を探す・空 = 使わない（試験の不在の通し・quota の承認が無い run）・
/// 絶対 path = それを使う（試験の偽の CLI・run.sh の数える包み）。`CUA_DRIVER_BIN` と同じ形。
pub const CLI_ENV: &str = "SHIBUBU_LLM_CLI";

/// 探す名前。
pub const DEFAULT_PROGRAM: &str = "claude";

/// 子の env から外す変数（在ると CLI が plan の login でなく鍵で認証し、使った分の請求になる）。
/// RQ2: 利用者が鍵の account（`AiAccount::Key`・`EnvKey`）を選んだ時だけ、`child_env` がその 1 つを外さずに渡す。
/// RQ2: `AWS_BEARER_TOKEN_BEDROCK`（Bedrock の鍵・在ると CLI が Bedrock で認証して請求する）も外す（YC_DECISIONS 89・Orca の CLAUDE_AUTH_ENV_VARS）。
/// 107 通目: Grok の鍵の変数も外す（claude の account の渡し方は変えない）。
pub const SCRUBBED_ENV: [&str; 6] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "XAI_API_KEY",
    "GROK_AUTH",
];

/// `CliConfig::new` の記録の file 名（data dir の下）。
pub const CALL_LOG: &str = "llm-calls.jsonl";

/// 起こし方の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    /// 1 回の呼び出しの上限。超えたら子を止め `LlmError::Timeout`
    pub timeout: Duration,
    /// 子の cwd。data dir を使う（利用者の repo の CLAUDE.md・.claude を拾わない）
    pub work_dir: PathBuf,
    /// `CallRecord` を 1 行ずつ足す file。None なら書かない（試験だけ）
    pub log: Option<PathBuf>,
    /// RQ2（RQ-ACCT）: 使う AI の account（`UiSettings::ai_active` を `ai_accounts` で引いた物）。None = System default（利用者の Claude Code の login・今の振る舞い）。
    /// 子の env は `child_env` が決める。保証しない: 選んだ account の login が続いている事（切れていれば `LlmError::NoCli` で Re-authenticate の 1 行）。
    pub account: Option<crate::model::AiAccount>,
    /// 負の対照だけ（107 通目・RULE-045）。Some の時、規則と記憶の入った利用者の設定を読ませる: Grok = HOME を空の dir でなくこの path（利用者の HOME）・
    /// claude = `--setting-sources user,project,local`（利用者の `~/.claude` の CLAUDE.md と設定・hook は `disableAllHooks` で起こさない）。製品は None。
    pub user_rules: Option<PathBuf>,
}

impl CliConfig {
    /// 製品の既定: timeout 120 秒・cwd = `data_dir`（相対なら今の cwd で絶対に）・記録 = `<data_dir>/llm-calls.jsonl`。
    /// 例: `let cfg = llm_cli::CliConfig::new(&data_dir);`
    pub fn new(data_dir: &Path) -> Self {
        // script の中の path は子の cwd（= work_dir）から読まれるので、相対の data dir は今の cwd で絶対にする（app は `surface::data_dir` で絶対を渡す・試験が渡す相対のため）
        let work_dir = match std::env::current_dir() {
            Ok(cwd) if data_dir.is_relative() => cwd.join(data_dir),
            _ => data_dir.to_path_buf(),
        };
        CliConfig {
            timeout: Duration::from_secs(120),
            log: Some(work_dir.join(CALL_LOG)),
            work_dir,
            account: None,
            user_rules: None,
        }
    }
}

/// 1 回の呼び出しの記録（`CliConfig::log` の 1 行の JSON）。試験の側は行の数を数え、`outcome` が "ok" でない行も 1 回に数える。
/// 中身（prompt・答え・利用者の一言）は書かない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRecord {
    /// 起こした時刻（unix の ms）
    pub at_ms: u64,
    /// `Llm::label`（"claude-cli/2.1.307" の形）
    pub label: String,
    /// `JsonRequest::name`（`composer::project::PLAN_NAME` など＝何のための呼び出しか）
    pub name: String,
    /// かかった時間
    pub ms: u64,
    /// 子の終了 code（timeout で止めた・起こせなかった時は None）
    pub exit_code: Option<i32>,
    /// "ok" か `LlmError` の種類の名前（"timeout"・"plan_limit"・"malformed"・"cancelled" など）
    pub outcome: String,
    /// FX2（FX-005・UIUX §6.4 の child process stop latency）: `Stop` の印を見てから子を回収し終えるまでの ms（`outcome == "cancelled"` の行だけ Some）。
    /// 前の行（FX2 より前）は無い = None で読める（`deny_unknown_fields` は新しい行を古い読み手が読む時だけ効く）。無い時は書かない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_after_ms: Option<u64>,
    /// 107 通目: 答えの `usage.input_tokens`（claude も Grok も同じ欄）。無い時は書かない。中身（prompt・答え）は書かない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// 混ざりの上限の土台（instructions・schema・一言の文字数）。中身は書かない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_chars: Option<u64>,
    /// USAGE-OWN-1101: 答えの `usage.output_tokens`（以下の 7 つも無い時は書かない = 前の行はそのまま読める）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// USAGE-OWN-1101: `input_tokens`（3 欄の和のまま・RULE-043）のうち cache から読んだ分（`cache_read_input_tokens`）と cache に書いた分（`cache_creation_input_tokens`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    /// USAGE-OWN-1101: claude -p の `total_cost_usd`（同じ token を API で払ったらの換算・請求ではない）を百万分の 1 ドルで（小数を持たず Eq を保つ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<u64>,
    /// USAGE-OWN-1101: 使った account（`AiAccount::id`・None = System default）。email・名前は書かない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// USAGE-OWN-1101: 頼んだ Project の id（`JsonRequest::project`・None = Home）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// USAGE-OWN-1101: CLI の session の id（claude -p の答えの `session_id`・Grok は無い）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// 起動の時に 1 回確かめた CLI。
/// Wave: W13
pub struct CliLlm {
    program: PathBuf,
    version: String,
    config: CliConfig,
    /// G104: `--help` に在った旗（`--effort`・file を読む `--restricted`）。無い版では menu の項目を押せない形と理由 1 行に
    flags: HelpFlags,
    /// REVIEW-RQN（operator 3）: plan の使用量を本物の Keychain と `USAGE_URL` から読むか（`detect` の時に `CLI_ENV` が未設定 = 利用者の Claude Code）。
    /// 差し替えた run（偽の CLI・数える包み）は読まない（その CLI の login が誰の物か Shibubu は知らない・試験と録画の前の run で本物の口を開けない）。
    usage: bool,
}

/// G104: 起動の時に `--help` で 1 回確かめた旗（推測で付けない・model を呼ばない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HelpFlags {
    /// PROVIDER-1101: その CLI の effort の旗が受ける段（claude `--effort`・Grok `--reasoning-effort`・opencode `--variant`・cursor-agent の `model[effort=…]`）。空 = 旗が無い
    efforts: Vec<crate::domain::chat::Effort>,
    restricted: bool,
    /// G84 → PROVIDER-1101: 選べる model（渡す語, menu の語）= その CLI から読んだ一覧（claude `help_models`・Grok / opencode / cursor-agent `listed_models`・kimi `kimi_models`）
    models: Vec<(String, String)>,
}

impl CliLlm {
    /// 在りか（`CLI_ENV` → `locate_cli`〈login shell の PATH・決まった install dir・`--version` で動く物か、混んで時間切れの物〉）→ `--version` → `auth status --json` の順に確かめる。
    /// どれも model を呼ばない（quota を使わない）。
    ///
    /// 例（app.rs の起動）: `let g = Generated::open(&dir).with_llm(CliLlm::detect(CliConfig::new(&dir)).map(|c| Box::new(c) as Box<dyn Llm>));`
    /// 保証する: Ok は絶対 path の実行 file で、`loggedIn == true` かつ `authMethod == "claude.ai"`（plan の login）。Err は全部 `LlmError::NoCli{detail}`
    /// （`CLI_ENV` が空・見つからない・`--version` が 0 で終わらない・login していない・plan でない login〈API key・他社の cloud〉）で、detail に path・email・token を入れない。
    /// G99: 持っている login の PATH で見つからなければ 1 回だけ取り直して探す（Shibubu を開いた後に入れた CLI も再起動なしで見つける）。
    /// 署名を変えた理由: 在りかを PTY の shell の `command -v` で引くのをやめた（alias に化かされる・`.zprofile` を読まない）ので shell の設定が要らない。
    /// 保証しない: quota が残っている事（呼んだ時の `LlmError::PlanLimit`）・login がこの後も続く事。
    pub fn detect(config: CliConfig) -> Result<CliLlm, LlmError> {
        detect_from(std::env::var_os(CLI_ENV), config)
    }

    #[allow(dead_code)] // 試験と診断の口（製品は label で版を使う）
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[allow(dead_code)]
    pub fn version(&self) -> &str {
        &self.version
    }
}

impl Llm for CliLlm {
    /// "claude-cli/<--version の版>"（file 名が grok の時は "grok/<版>"）。
    /// RQ-API: 鍵の account は claude の時だけ後ろに ` · API key …<末尾 4 文字>`（`Generated::ai_rows` が請求の 1 行に替える）。
    fn label(&self) -> String {
        use crate::model::AiAccount;
        let kind = cli_id(&self.program).unwrap_or("claude-cli");
        match &self.config.account {
            Some(AiAccount::Key { last4, .. }) if kind == "claude-cli" => {
                format!("{kind}/{} · API key …{last4}", self.version)
            }
            Some(AiAccount::EnvKey) if kind == "claude-cli" => {
                format!("{kind}/{} · API key …{}", self.version, env_key_last4().unwrap_or_default())
            }
            _ => format!("{kind}/{}", self.version),
        }
    }

    /// RQ2（RQ-USAGE）: 選んだ plan の login（System default = 利用者の Claude Code・`Plan` = その account の dir）の token で `USAGE_URL` を読む。model を呼ばない。
    /// 鍵の account は plan の % が無いので `NoCli { "API key · billed per use" }`。
    fn plan_usage(&self) -> Result<crate::llm::PlanUsage, LlmError> {
        use crate::model::AiAccount;
        let dir = match &self.config.account {
            None => None,
            Some(a @ AiAccount::Plan { id }) if a.provider() == "claude" => Some(self.config.work_dir.join(ACCOUNTS_DIR).join(id)),
            Some(AiAccount::Key { .. } | AiAccount::EnvKey) => return Err(no_cli("API key · billed per use")),
            // ACCOUNTS-1101: 足した CLI の account も Claude の plan の % を持たない
            Some(AiAccount::Found { .. } | AiAccount::Plan { .. }) => return Err(no_cli("This AI has no Claude plan usage")),
        };
        if !self.usage {
            return Err(no_cli(USAGE_NOT_READ));
        }
        read_plan_usage(USAGE_URL, &oauth_token(dir.as_deref())?, Duration::from_secs(10))
    }

    /// G104: 添えた file は `--tools Read --restricted`（Read だけ・読めるのは呼び出しの dir の写しだけ）で読ませる。`--restricted` の無い版は読ませない
    fn takes_files(&self) -> bool {
        self.flags.restricted
    }

    fn provider_name(&self) -> String {
        cli_id(&self.program).map_or("Claude Code", found_name).into()
    }

    fn efforts(&self) -> Vec<&'static str> {
        self.flags.efforts.iter().map(|e| e.flag()).collect()
    }

    fn models(&self) -> Vec<(String, String)> {
        self.flags.models.clone()
    }

    /// `command_line` を PTY の session（rc を読まない shell・cwd = `config.work_dir`）で 1 回打ち、`req.input` を stdin に・答えを file に受け、`parse_result` で読む。
    /// 保証する: 1 回の呼び出しで子を 1 つ（retry しない）・`config.timeout` で子を止める・終わったら session を閉じる・成否にかかわらず `CallRecord` を 1 行足す。
    /// 子の終了 code が 0 でない・答えが読めない → `parse_result` の種類（plan の使用量の上限は `PlanLimit`・混雑は `RateLimited`・答えの拒否は `Refused`・壊れた答えは `Malformed`）。
    fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
        // FX2: 記録の 1 行と子の起こし方は `send_cancellable` の 1 か所（立たない印 = 前の send と同じ）
        self.send_cancellable(req, &AtomicBool::new(false))
    }

    /// FX2（FX-005）: `send` と同じ 1 回の呼び出しを、`cancel` が立ったら子ごと止める。待ちは `TerminalManager::wait_exit` を短く刻み（50ms 程度）、
    /// 刻みごとに `cancel` を見る（印を見たら `TerminalManager::close` = SIGHUP → `close_grace` → SIGKILL → 回収）。
    /// 例（Generated の生成の thread・`LlmModel` が job の印を持つ）: `llm.complete_json_cancellable(req, &job.cancel)` → `Budgeted` → ここ。
    /// 保証する: 止めた時は子の shell と CLI が回収済み（`close` の保証・zombie 0）で `Err(LlmError::Cancelled { after_ms })` を返す・`CallRecord` を 1 行
    /// （`outcome` = "cancelled"・`stopped_after_ms` = after_ms・`ms` = 起こしてから回収まで）・scratch の dir を消す・retry しない・timeout は今まで通り `config.timeout`。
    /// 送る前に立っていれば子を起こさず `Cancelled { after_ms: 0 }`（記録しない = 呼んでいない）。
    /// 保証しない: tty を離れた孫（`setsid`・daemon 化）が止まる事（`TerminalManager::close` と同じ）・cancel を見るまでの遅れが刻みより短い事。
    /// Wave: FX2
    fn send_cancellable(&self, req: &JsonRequest, cancel: &AtomicBool) -> Result<Value, LlmError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(LlmError::Cancelled { after_ms: 0 });
        }
        // 記録を書けないなら呼ばない（run.sh の上限は行の数で数える。書けないまま呼ぶと数え漏れで quota を使い過ぎる）
        let mut log = match &self.config.log {
            Some(p) => Some(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map_err(|e| no_cli_log("the AI call couldn't be prepared on disk", e.kind()))?,
            ),
            None => None,
        };
        let (at_ms, started) = (now_ms(), Instant::now());
        let (exit_code, meter, result) = self.call(req, cancel);
        let meter = meter.unwrap_or_default();
        // G84: model を選んだ呼び出しの失敗は model の 1 行と次の一手に（plan の上限・login 等は今の 1 行のまま）
        let result = match &req.model {
            Some(m) => result.map_err(|e| model_error(m, e)),
            None => result,
        };
        if let Some(f) = log.as_mut() {
            let rec = CallRecord {
                at_ms,
                label: self.label(),
                name: req.name.clone(),
                ms: started.elapsed().as_millis() as u64,
                exit_code,
                outcome: match &result {
                    Ok(_) => "ok".into(),
                    Err(e) => kind(e).into(),
                },
                stopped_after_ms: match &result {
                    Err(LlmError::Cancelled { after_ms }) => Some(*after_ms),
                    _ => None,
                },
                input_tokens: meter.input,
                prompt_chars: Some(prompt_chars(&req.instructions, &req.schema.to_string(), &req.input)),
                output_tokens: meter.output,
                cache_read_tokens: meter.cache_read,
                cache_creation_tokens: meter.cache_creation,
                cost_micros: meter.cost_micros,
                account: self.config.account.as_ref().map(|a| a.id().to_owned()),
                project: req.project.clone(),
                session: meter.session,
            };
            let _ = writeln!(f, "{}", serde_json::to_string(&rec).unwrap_or_default());
        }
        result
    }
}

/// FX2: 子の終わりを待つ 1 刻み（この間隔で Stop の印を見る）。
const CANCEL_TICK: Duration = Duration::from_millis(50);

impl CliLlm {
    /// 1 回の呼び出しの本体: `<work_dir>/llm-cli.<id>/` に prompt と script を置き、`/bin/zsh -f <script>` を PTY の session で 1 本起こして
    /// 終わりを待つ（script は `exec` で CLI に替わる＝子の終了 code が CLI の物・timeout の `close` で CLI ごと止まる）。PTY の echo は読まない。
    /// FX2: 待ちは `CANCEL_TICK` ずつ刻み、`cancel` が立ったら子を閉じて `Cancelled`（timeout と同じ `close`）。
    fn call(&self, req: &JsonRequest, cancel: &AtomicBool) -> (Option<i32>, Option<Meter>, Result<Value, LlmError>) {
        let dir = match scratch(&self.config.work_dir) {
            Ok(d) => d,
            Err(e) => return (None, None, Err(e)),
        };
        let r = self.call_in(&dir, req, cancel);
        let _ = fs::remove_dir_all(&dir);
        r
    }

    fn call_in(&self, dir: &Path, req: &JsonRequest, cancel: &AtomicBool) -> (Option<i32>, Option<Meter>, Result<Value, LlmError>) {
        let (prompt, out, err, script) = (
            dir.join("prompt.txt"),
            dir.join("out.json"),
            dir.join("err.txt"),
            dir.join("call.zsh"),
        );
        // G104: 添えた file は呼び出しの dir の `files/` に写し（CLI の Read が届くのはそこだけ・元の folder を見せない）、名前と link を一言の後ろに書く
        let input = match attach_files(dir, req) {
            Ok(input) => input,
            Err(e) => return (None, None, Err(no_cli_log("the attached files couldn't be prepared", e.kind()))),
        };
        // PROVIDER-1101: system prompt と schema の旗が無い CLI（kimi・opencode・cursor-agent）は 1 つの文に（`plain_prompt`）
        let other = cli_id(&self.program).filter(|id| *id != "grok");
        let (who, quiet) = (self.provider_name(), is_grok(&self.program) || other.is_some());
        let input = if other.is_some() { plain_prompt(&req.instructions, &req.schema.to_string(), &input) } else { input };
        let owned = if is_grok(&self.program) {
            match self.grok_env_paths() {
                Ok(p) => Some(p),
                Err(e) => return (None, None, Err(e)),
            }
        } else {
            None
        };
        let grok_ref = owned.as_ref().map(|(h, g, a)| (h.as_path(), g.as_path(), a.as_path()));
        // RQ2: claude だけ選んだ account の env（鍵の値は argv に出さず script の export）。Grok には鍵を渡さない。
        // PROVIDER-1101: kimi・opencode・cursor-agent は呼び出しの dir の下の空の HOME と各 CLI の設定の dir（`prepare_other_env`・command_line が `K=V` の引数に）
        let env = if is_grok(&self.program) {
            Vec::new()
        } else if let Some(id) = other {
            match prepare_other_env(id, dir, login_dir(&self.config).as_deref()) {
                Ok(env) => env.into_iter().map(|(k, v)| (k, Some(v))).collect(),
                Err(e) => return (None, None, Err(no_cli_log("the AI call couldn't be prepared on disk", e.kind()))),
            }
        } else {
            match child_env(
                self.config.account.as_ref(),
                &self.config.work_dir,
                &crate::storage::secret_store::SecretStore::from_env(),
            ) {
                Ok(env) => env,
                Err(e) => return (None, None, Err(e)),
            }
        };
        let mut argv = command_line(&self.program, req, &env, &prompt, grok_ref);
        // RULE-045 の負の対照だけ: claude に利用者の規則と記憶を読ませる（hook は起こさない = 記憶の書き込み・通知を走らせない）。製品は通らない
        if self.config.user_rules.is_some() && cli_id(&self.program).is_none() {
            if let Some(at) = argv.iter().position(|a| a == "--setting-sources") {
                argv[at + 1] = "user,project,local".into();
            }
            argv.extend(["--settings".to_string(), r#"{"disableAllHooks":true}"#.to_string()]);
        }
        // PROVIDER-1101: `PROMPT_ARG` は prompt の file の中身（shell が読んで argv に・kimi `-p`・opencode `run`・cursor-agent の prompt）
        let argv: Vec<String> = argv
            .iter()
            .map(|a| if a == PROMPT_ARG { format!("\"$(cat {})\"", quote(&prompt.display().to_string())) } else { quote(a) })
            .collect();
        let exports = if quiet { String::new() } else { exports(&env) };
        // Grok は --prompt-file。stdin にも一言を流すと、file 以外の口で読まれる。kimi・opencode・cursor-agent は argv の prompt だけ。
        let stdin = if quiet {
            "/dev/null".to_string()
        } else {
            quote(&prompt.display().to_string())
        };
        let body = format!(
            "{}{exports}exec {} < {} > {} 2> {}\n",
            unset_line(),
            argv.join(" "),
            stdin,
            quote(&out.display().to_string()),
            quote(&err.display().to_string()),
        );
        let write_script = || {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&script)?.write_all(body.as_bytes())
        };
        if let Err(e) = fs::write(&prompt, &input).and_then(|_| write_script()) {
            return (
                None,
                None,
                Err(no_cli_log("the AI call couldn't be prepared on disk", e.kind())),
            );
        }
        let mgr = TerminalManager::new(TerminalConfig {
            child_path: child_path_for(&self.program, &login_path(false)),
            shell_args: vec!["-f".into(), script.display().to_string()],
            scrollback_bytes: 64 << 10,
            close_grace: Duration::from_millis(500),
            ..TerminalConfig::default()
        });
        // G104: file を添えた呼び出しの cwd は呼び出しの dir（`--restricted` の Read が届く所 = 写した file だけ）。
        // PROVIDER-1101: kimi・opencode・cursor-agent も呼び出しの dir（作業の場所 = 空・data dir を見せない）
        let cwd = if req.files.is_empty() && other.is_none() { self.config.work_dir.clone() } else { dir.to_path_buf() };
        let spec = session_spec("call", "/bin/zsh", &cwd);
        let key = SessionKey::of(&spec);
        if let Err(e) = mgr.spawn(&spec, TermSize::default()) {
            return (None, None, Err(no_cli_log(&format!("{who} couldn't start"), e)));
        }
        // G104: 深く考える段は待ちの上限を伸ばす（High 2・Extra high 3・Max 5 倍）
        let factor = req.effort.as_deref().and_then(crate::domain::chat::Effort::from_flag).map_or(1, |e| e.wait_factor());
        let limit = self.config.timeout * factor;
        let deadline = Instant::now() + limit;
        let exit = loop {
            let tick = deadline.saturating_duration_since(Instant::now()).min(CANCEL_TICK);
            match mgr.wait_exit(&key, tick) {
                Ok(exit) => break exit,
                Err(TerminalError::TimedOut { .. }) if cancel.load(Ordering::SeqCst) => {
                    // FX2: 印を見てから子を回収し終えるまで（`close` は回収して返る）
                    let seen = Instant::now();
                    let _ = mgr.close(&key);
                    return (
                        None,
                        None,
                        Err(LlmError::Cancelled {
                            after_ms: seen.elapsed().as_millis() as u64,
                        }),
                    );
                }
                Err(TerminalError::TimedOut { .. }) if Instant::now() < deadline => {}
                Err(TerminalError::TimedOut { .. }) => {
                    let _ = mgr.close(&key);
                    return (
                        None,
                        None,
                        Err(LlmError::Timeout {
                            ms: limit.as_millis() as u64,
                        }),
                    );
                }
                Err(e) => {
                    let _ = mgr.close(&key);
                    return (
                        None,
                        None,
                        Err(no_cli_log(&format!("{who}'s answer couldn't be read"), e)),
                    );
                }
            }
        };
        let stdout = fs::read_to_string(&out).unwrap_or_default();
        // 検証に落ちても token 数は残す（混ざりの上限は答えの成否と別）。
        let tokens = meter_of(&stdout);
        if stdout.trim().is_empty() {
            // 答えが空: CLI が自分で出した 1 行（stderr の先頭）を理由に（鍵・email を含む行は出さない）
            let first = fs::read_to_string(&err).unwrap_or_default();
            let first = first
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("");
            let code = exit.code.map_or("?".to_string(), |c| c.to_string());
            let detail = if first.contains('@') || first.to_lowercase().contains("key") {
                format!("{who} gave no answer (exit code {code})")
            } else {
                format!("{who} gave no answer (exit code {code}): {}", short(first))
            };
            // G97: 繋がらない時の CLI の 1 行は「読めない」でなく network の 1 行に
            if offline(first) {
                return (exit.code, tokens, Err(LlmError::Network { detail: short(first) }));
            }
            // PROVIDER-1101: login の切れた CLI の 1 行は sign in の一手に（stderr に出す CLI がある）
            if let Some(id) = other.filter(|_| signed_out(first)) {
                return (exit.code, tokens, Err(no_cli(&sign_in_line(id))));
            }
            return (exit.code, tokens, Err(LlmError::Malformed { detail }));
        }
        if let Some(id) = other {
            return (exit.code, tokens, parse_text_answer(&stdout, id));
        }
        // RQ-API: 鍵の account の失敗は鍵と請求の言葉に（API の status と型で分ける・plan の「resets」と「Sign in」を言わない）
        let key = matches!(self.config.account, Some(crate::model::AiAccount::Key { .. } | crate::model::AiAccount::EnvKey));
        (exit.code, tokens, parse_answer(&stdout, key))
    }

    /// 107 通目: Grok の HOME。`user_rules` が Some の時はその path（消さない）。None の時は work_dir の空 dir を作り直す。
    fn grok_env_paths(&self) -> Result<(PathBuf, PathBuf, PathBuf), LlmError> {
        if let Some(home) = &self.config.user_rules {
            return Ok((home.clone(), home.join(".grok"), grok_auth_path()));
        }
        // ACCOUNTS-1101: 足した Grok の account はその置き場の auth（HOME と GROK_HOME は呼び出しごとの空の dir のまま）
        let (home, gh, auth) = prepare_empty_grok_home(&self.config.work_dir)
            .map_err(|e| no_cli_log("the AI call couldn't be prepared on disk", e.kind()))?;
        Ok((home, gh, login_dir(&self.config).map_or(auth, |d| grok_account_auth(&d))))
    }
}

/// `--version` と `auth status --json` の待ちの上限（model を呼ばない・混雑の下で auth status は 4 秒ほど）。
const QUICK: Duration = Duration::from_secs(20);

/// G97: CLI の文が network に繋がらない事を言っているか（接続・名前引き・offline）。本物の CLI の offline の文は未測なので node と curl の語で広めに見る。
fn offline(text: &str) -> bool {
    let low = text.to_lowercase();
    ["network", "connection", "unable to connect", "enotfound", "econnrefused", "econnreset", "etimedout", "getaddrinfo", "fetch failed", "offline"].iter().any(|w| low.contains(w))
}

/// G97: CLI の道の 401 / 403（plan の login が切れた・断られた）の 1 行。鍵は使っていないので「key」と言わない・次の一手は login し直し（`next_step`）。
pub const SIGNED_OUT: &str = "Claude refused the sign-in";
/// G99: Home の AI の 1 行（`NoCli` の detail）。次の一手は 1 行の隣の button（`next_step`）で、Terminal.app へは行かせない。
pub const NOT_INSTALLED: &str = "Claude Code isn't on this Mac";
pub const NOT_SIGNED_IN: &str = "Claude Code isn't signed in";
pub const KEY_SIGN_IN: &str = "Claude Code is signed in with a key, not your plan";
/// G99: `sign_in` が終わらなかった（失敗・取り消し・5 分の時間切れ）時の 1 行。隣に Sign in をもう一度出す（`next_step`）。
pub const SIGN_IN_FAILED: &str = "Sign in didn't finish · try again";
/// G99: `Install Claude Code` が開く公式の install の頁（2026-10-28 に 200 を確かめた）。
pub const INSTALL_URL: &str = "https://code.claude.com/docs/en/setup";
pub const INSTALL_STEP: &str = "Install Claude Code";
pub const SIGN_IN_STEP: &str = "Sign in";
/// G99: `sign_in` が browser の login を待つ上限
const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// G99: AI の 1 行（`LlmError::NoCli` の detail）の隣に出す button（入っていない → `INSTALL_STEP`・login していない / 鍵の login / 断られた → `SIGN_IN_STEP`）。
/// 自分の定数とだけ比べる（他の文は None = button を出さない・`Check again` は app.rs が出す）。
pub fn next_step(line: &str) -> Option<&'static str> {
    match line {
        NOT_INSTALLED => Some(INSTALL_STEP),
        NOT_SIGNED_IN | KEY_SIGN_IN | SIGNED_OUT | SIGN_IN_FAILED => Some(SIGN_IN_STEP),
        _ => None,
    }
}

/// RQ-USAGE: 使用量の 1 行（`llm::usage_line`）の隣の button。読めた = None（60 秒ごとに読み直す）・読めない / 空の答え / Keychain を読めない = `TRY_AGAIN`・
/// 未 login / 断られた = `SIGN_IN_STEP`（`next_step`）・読み過ぎ（429）= None（押すと間隔を待たずにもう 1 回 429 をもらうだけ）・鍵の account と使用量の無い AI = None。
pub fn usage_step(usage: &Result<crate::llm::PlanUsage, LlmError>) -> Option<&'static str> {
    match usage {
        Ok(u) if !u.windows.is_empty() => None,
        Err(LlmError::NoCli { detail }) if detail != crate::llm::KEYCHAIN_UNREADABLE => next_step(detail),
        Err(LlmError::RateLimited { .. }) => None,
        _ => Some(TRY_AGAIN),
    }
}
pub const TRY_AGAIN: &str = "Try again";

/// REVIEW-RQN（operator 3）: CLI を差し替えた run（`CLI_ENV` = 偽の CLI・数える包み）の使用量の 1 行（`NoCli`・button 無し = 帯は出さない・Settings は 1 行だけ）。
pub const USAGE_NOT_READ: &str = "Usage isn't read for a replaced Claude Code";

/// G99: `claude auth login --claudeai`（plan の login・CLI が browser を開く）を Shibubu の子で 1 回起こし、終わるまで待つ（最大 5 分）。UI の thread から呼ばない。
/// 終わったら呼ぶ側が `CliLlm::detect` で確かめ直す。Err は画面の 1 行（鍵・email を入れない）。
/// RQ-ACCT: `env` = `child_env` の差分（足した plan の account = その `CLAUDE_CONFIG_DIR`・System default = 鍵の変数を外すだけ）。確かめ（`detect`）と同じ差分を
/// 渡す（Claude Code は login を `CLAUDE_CONFIG_DIR` の文字列から作る Keychain の名前に置くので、違う文字列だと login が見えない）。
/// 署名を変えた理由: account ごとの login（足した account の sign in が利用者の `~/.claude` に書かない）。在りかは `detect` と同じ口（`CLI_ENV` が空 = 起こさない）。
/// 例: `llm_cli::sign_in(&llm_cli::child_env(Some(&acct), &data_dir, &keys)?)`（呼ぶのは `add_plan_account` / `reauthenticate` の引数として）。
/// 保証しない: CLI が browser を開ける事（開けず code を貼る形の CLI は 5 分で止める）・取り消した login の前の Keychain の項目を CLI が残す事（未測）。
pub fn sign_in(env: &[(&str, Option<String>)]) -> Result<(), String> {
    sign_in_from(std::env::var_os(CLI_ENV), env, SIGN_IN_WAIT, &std::sync::atomic::AtomicBool::new(false))
}

/// RQ-ACCT（REVIEW-RQN・operator 1）: `sign_in` に止める口を足した物（Settings の Add / Re-authenticate）。`stop` が立てば待つのをやめ、子（`claude auth login`）を止めて
/// `SIGN_IN_FAILED`（呼ぶ側は答えを捨てる）。例: `llm_cli::reauthenticate(&dir, Some(&a), |env| llm_cli::sign_in_until(env, &stop))`（stop = app.rs の `sign_in_stop`）。
/// 新しくした理由: G99b の `sign_in` は Home の Sign in が止める口を持たずに使う（署名を変えると trunk の呼び元に響く）。
pub fn sign_in_until(env: &[(&str, Option<String>)], stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    sign_in_from(std::env::var_os(CLI_ENV), env, SIGN_IN_WAIT, stop)
}

/// `sign_in` の本体（`cli` = `CLI_ENV` の値・`wait` = 待つ上限。試験は偽の CLI と短い上限を渡す = process の env を書き換えない）。
fn sign_in_from(cli: Option<OsString>, env: &[(&str, Option<String>)], wait: Duration, stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    let program = cli_program(cli).map_err(|e| e.to_string())?;
    let mut child = child_command(&program, &["auth", "login", "--claudeai"], env)
        .spawn()
        .map_err(|e| no_cli_log(SIGN_IN_FAILED, e.kind()).to_string())?;
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(()),
            Ok(Some(_)) => return Err(SIGN_IN_FAILED.into()),
            Ok(None) if !stop.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < deadline => thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    eprintln!("shibubu: llm-cli: sign in: stopped (the account was removed)");
                } else {
                    eprintln!("shibubu: llm-cli: sign in: no answer in {} s", wait.as_secs());
                }
                return Err(SIGN_IN_FAILED.into());
            }
        }
    }
}

fn no_cli(detail: &str) -> LlmError {
    LlmError::NoCli {
        detail: detail.into(),
    }
}

/// 中の技術的な文（io の種類・terminal の誤り）は log（stderr）へ、画面の `detail` は表（copy.en.md の no_cli）の 1 行だけ。
fn no_cli_log(detail: &str, cause: impl std::fmt::Display) -> LlmError {
    eprintln!("shibubu: llm-cli: {detail}: {cause}");
    no_cli(detail)
}

/// CLI の在りか（`CLI_ENV` の値: 空 = 使わない・絶対 path = それ・未設定 = `locate_cli`）。`detect` と `sign_in` が同じ口を通る
/// （RQ-ACCT: 前は sign in だけ `CLI_ENV` を見ず、偽の CLI を置いた試験・数える包みの run でも本物の `claude auth login` を起こした）。
fn cli_program(env: Option<OsString>) -> Result<PathBuf, LlmError> {
    match env {
        // G99: 消す口は env だけ（Connections に AI の行は無い・前の「turned off in Connections」は嘘だった）
        Some(v) if v.is_empty() => Err(no_cli("AI is off for this launch")),
        Some(v) => {
            let p = PathBuf::from(v);
            if !p.is_absolute() || !p.is_file() {
                let detail = if is_grok(&p) {
                    "Grok isn't at the set path"
                } else {
                    "Claude Code isn't at the set path"
                };
                return Err(no_cli(detail));
            }
            Ok(p)
        }
        None => locate_cli(DEFAULT_PROGRAM, false)
            .or_else(|| locate_cli(DEFAULT_PROGRAM, true))
            .ok_or_else(|| no_cli(NOT_INSTALLED)),
    }
}

/// `CLI_ENV` の値を受け取る `detect`（試験が process の env を書き換えずに 3 つの道を通すため）。
fn detect_from(env: Option<OsString>, config: CliConfig) -> Result<CliLlm, LlmError> {
    let usage = env.is_none();
    // L4 → PROVIDER-1101: 見つかった CLI（`FOUND_CLIS`）を選んだ時は、利用者の PATH のその CLI（`CLI_ENV` の空 = AI を切った run は今まで通り切る・path を置いた run はその path）
    let program = match &config.account {
        // ACCOUNTS-1101: 足した CLI の account（`Plan { id: "grok-2" }`）も同じ CLI（置き場だけ `login_dir`）
        Some(a @ (crate::model::AiAccount::Found { .. } | crate::model::AiAccount::Plan { .. })) if FOUND_CLIS.iter().any(|c| c.0 == a.provider()) && env.as_ref().is_none() => {
            let id = a.provider();
            let name = found_name(id);
            locate_cli(id, false).or_else(|| locate_cli(id, true)).ok_or_else(|| no_cli(&format!("{name} isn't on this Mac any more")))?
        }
        _ => cli_program(env)?,
    };
    // 107 通目: file 名が grok の時は claude の auth status を打たない。空の HOME で `--version` と `models` だけ（model は呼ばない）。
    if is_grok(&program) {
        let (home, gh, auth) = prepare_empty_grok_home(&config.work_dir)
            .map_err(|e| no_cli_log("the AI call couldn't be prepared on disk", e.kind()))?;
        let auth = login_dir(&config).map_or(auth, |d| grok_account_auth(&d));
        let home_s = home.display().to_string();
        let gh_s = gh.display().to_string();
        let auth_s = auth.display().to_string();
        let extra = [
            ("HOME", home_s.as_str()),
            ("GROK_HOME", gh_s.as_str()),
            ("GROK_AUTH_PATH", auth_s.as_str()),
        ];
        let (code, out) = run_quick_env(&program, &["--version"], &extra)
            .map_err(|why| no_cli_log("Grok couldn't start", why))?;
        let version = version_token(&out, true)
            .filter(|_| code == Some(0))
            .ok_or_else(|| no_cli("Grok didn't answer"))?
            .to_string();
        let (_, models) = run_quick_env(&program, &["models"], &extra)
            .map_err(|why| no_cli_log("the sign-in state couldn't be read", why))?;
        parse_grok_login(&models)?;
        // PROVIDER-1101: effort は `--help` に `--reasoning-effort` が在る時だけ（none〜max・Effort の 5 段は全部受ける = grok 1.0.70 の README）・model は `grok models` の残りの行
        let help = run_quick_env(&program, &["--help"], &extra).map(|(_, out)| out).unwrap_or_default();
        let efforts = if help.contains("--reasoning-effort") { crate::domain::chat::Effort::ALL.to_vec() } else { Vec::new() };
        return Ok(CliLlm {
            program,
            version,
            config,
            flags: HelpFlags { efforts, restricted: false, models: listed_models(&models) },
            usage,
        });
    }
    // PROVIDER-1101: kimi・opencode・cursor-agent。claude の auth status を打たず、空の HOME で `--version` と login の確かめだけ（model は呼ばない）
    if let Some(id) = cli_id(&program) {
        let env = prepare_other_env(id, &config.work_dir.join("ai-probe"), login_dir(&config).as_deref())
            .map_err(|e| no_cli_log("the AI call couldn't be prepared on disk", e.kind()))?;
        let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let name = found_name(id);
        let (code, out) = run_quick_env(&program, &["--version"], &extra)
            .map_err(|why| no_cli_log(&format!("{name} couldn't start"), why))?;
        let version = version_token(&out, true)
            .filter(|_| code == Some(0))
            .ok_or_else(|| no_cli(&format!("{name} didn't answer")))?
            .to_string();
        let (signed, models) = other_login(id, &program, &extra);
        if !signed {
            return Err(no_cli(&sign_in_line(id)));
        }
        return Ok(CliLlm { program, version, config, flags: HelpFlags { efforts: other_efforts(id), restricted: false, models }, usage });
    }
    let (code, out) = run_quick(&program, &["--version"], &[])
        .map_err(|why| no_cli_log("Claude Code couldn't start", why))?;
    let version = version_token(&out, false)
        .filter(|_| code == Some(0))
        .ok_or_else(|| no_cli("Claude Code didn't answer"))?
        .to_string();
    // RQ2: 選んだ account の env（鍵の account は鍵が在る事・plan の login は確かめない＝鍵で呼ぶ）
    use crate::model::AiAccount;
    let env = child_env(config.account.as_ref(), &config.work_dir, &crate::storage::secret_store::SecretStore::from_env())?;
    match &config.account {
        Some(AiAccount::Key { .. }) => {}
        Some(AiAccount::EnvKey) => {
            if env_key_last4().is_none() {
                return Err(no_cli("The API key from your environment isn't there · Choose another account in Settings"));
            }
        }
        // 見つかった AI は claude auth status を打たない。切り替えの本体は detect の前段（ここは抜けた時の門）。足した CLI の account も同じ
        Some(AiAccount::Found { .. }) => return Err(no_cli("That AI isn't signed in on this Mac")),
        Some(a @ AiAccount::Plan { .. }) if a.provider() != "claude" => return Err(no_cli("That AI isn't signed in on this Mac")),
        None | Some(AiAccount::Plan { .. }) => {
            // login していない時の終了 code は未測なので、code でなく stdout の JSON で決める
            let (_, out) = run_quick(&program, &["auth", "status", "--json"], &env)
                .map_err(|why| no_cli_log("the sign-in state couldn't be read", why))?;
            parse_auth_status(&out)?;
        }
    }
    // G104: 旗の有無は `--help` の字で（読めなければ旗を使わない = 添付と effort を押せない形にする）
    let help = run_quick(&program, &["--help"], &env).map(|(_, out)| out).unwrap_or_default();
    let efforts = if help.contains("--effort") { crate::domain::chat::Effort::ALL.to_vec() } else { Vec::new() };
    let flags = HelpFlags { efforts, restricted: help.contains("--restricted"), models: help_models(&help) };
    Ok(CliLlm {
        program,
        version,
        config,
        flags,
        usage,
    })
}

/// G99: CLI の在りか探し（Orca の写し・stablyai/orca ff6a3755a・MIT・`src/main/startup/hydrate-shell-path.ts`・`shell-path-probe.ts`・
/// `src/shared/node-cli-command-resolution.ts`・`system-cli-install-dirs.ts`・`src/main/ipc/preflight-command-exec.ts`）。
/// 1. login shell（`$SHELL -ilc`）の PATH を区切りの語で挟んで 1 回だけ取る（10 秒・ANSI を消す・失敗は種の PATH）
/// 2. 名前ごとに PATH の dir を順に歩き、実行できる file を探す（`command -v` は使わない = alias・function に化かされない）
/// 3. PATH に無い時だけ決まった install dir（nvm → volta・asdf・fnm・mise → ~/.local/bin・pnpm・yarn・bun → homebrew・nix・opencode・vite-plus・grok・MacPorts → kimi・mimo）
/// 4. 候補を上から `--version` で打ち、動いた最初の 1 つ（0 以外で終わる壊れた shim を飛ばす・どれも `VERSION_PROBE` の内に終わらなければ時間切れの最初の 1 つ = B87）。呼び出しと login の確かめはその絶対 path
/// 5. `force` で 1 を取り直す（入れた後に再起動しない）
/// 同じ物を llm_cli（Claude Code）・mcp（MCP の server の command）・Terminal の種の PATH（`terminal::seed_path`）が使う。
const PATH_MARK: &str = "__SHIBUBU_SHELL_PATH__";
/// login shell の PATH を待つ上限（重い profile は 6〜7 秒の実測・Orca と同じ 10 秒）
const PATH_PROBE: Duration = Duration::from_secs(10);
/// 候補ごとの `--version` の上限
const VERSION_PROBE: Duration = Duration::from_secs(3);

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

static LOGIN_PATH: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 子と在りか探しに使う PATH（login shell の順を先頭・種の PATH の残りを後ろ・重ねない）。1 回取ったら持ち、`force` で取り直す。
/// login shell が読めない（無い・起きない・10 秒を過ぎた・空）時は種の PATH だけ（理由は log に 1 行・画面に shell の話をしない）。
/// 保証しない: 待たない事（最初の 1 回と `force` は最大 10 秒待つ・UI の thread は `known_path`）。
pub fn login_path(force: bool) -> String {
    let mut held = LOGIN_PATH.lock().unwrap_or_else(|e| e.into_inner());
    if let (false, Some(p)) = (force, held.as_ref()) {
        return p.clone();
    }
    let home = home_dir();
    let seed = seed_path(&home.display().to_string());
    let got = probe_login_path(&user_shell(), &home, PATH_PROBE).unwrap_or_else(|why| {
        eprintln!("shibubu: llm-cli: login shell PATH: {why}");
        String::new()
    });
    let path = merge_path(&got, &seed);
    *held = Some(path.clone());
    path
}

/// 待たない版（UI の thread から・MCP の行）: 取ってあればそれ、まだ・取っている途中なら種の PATH。
pub fn known_path() -> String {
    match LOGIN_PATH.try_lock() {
        Ok(held) if held.is_some() => held.clone().unwrap_or_default(),
        _ => seed_path(&home_dir().display().to_string()),
    }
}

/// `shell -ilc` で PATH を区切りの間に printf させて読む（stdin は閉じる・stderr は捨てる・`wait` を過ぎたら止める）。
/// Err は理由（Orca の 4 つ: `no_shell`・`spawn_error`・`timeout`・`empty_path`）。
fn probe_login_path(shell: &str, home: &Path, wait: Duration) -> Result<String, &'static str> {
    if !Path::new(shell).is_file() {
        return Err("no_shell");
    }
    let mut child = Command::new(shell)
        .args(["-ilc", &format!("printf '{PATH_MARK}%s{PATH_MARK}' \"$PATH\"")])
        .env("HOME", home)
        .env("SHIBUBU_SHELL_PATH_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "spawn_error")?;
    let mut out = child.stdout.take().ok_or("spawn_error")?;
    let reader = thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timeout");
            }
        }
    }
    parse_path_probe(&reader.join().unwrap_or_default()).ok_or("empty_path")
}

/// 区切りの間の PATH（rc の banner・prompt・ANSI の色が前後に在っても）。区切りが 2 つ無い・空なら None。
pub fn parse_path_probe(out: &str) -> Option<String> {
    let clean = strip_ansi(out);
    let start = clean.find(PATH_MARK)? + PATH_MARK.len();
    let len = clean[start..].find(PATH_MARK)?;
    let path = clean[start..start + len].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// ESC [ … 終わりの文字（CSI）と ESC ] … BEL / ESC \（OSC）を消す。
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `first` の順を先頭に、`rest` の残りを後ろに（空の段と重なりを落とす）。
pub fn merge_path(first: &str, rest: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    first.split(':').chain(rest.split(':')).filter(|d| !d.is_empty() && seen.insert(*d)).collect::<Vec<_>>().join(":")
}

/// PATH に無い時だけ引く install dir（順は Orca の `node-cli-command-resolution.ts:226-233` と `system-cli-install-dirs.ts:8-37`）。
fn install_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = nvm_bins(home);
    dirs.extend(
        [
            ".volta/bin",
            ".asdf/shims",
            ".local/share/fnm/aliases/default/bin",
            "Library/Application Support/fnm/aliases/default/bin",
            ".local/share/mise/shims",
            ".local/bin",
            "Library/pnpm",
            ".yarn/bin",
            ".bun/bin",
        ]
        .iter()
        .map(|d| home.join(d)),
    );
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/nix/var/nix/profiles/default/bin"].iter().map(PathBuf::from));
    dirs.extend([".nix-profile/bin", ".opencode/bin", ".vite-plus/bin", ".grok/bin"].iter().map(|d| home.join(d)));
    dirs.push(PathBuf::from("/opt/local/bin"));
    // Shibubu が足す（Orca に無い）: kimi と mimo の installer の既定の dir。login shell が読めない Mac で 2 つが見つからなかった（10-27 実測）
    dirs.extend([".kimi-code/bin", ".mimocode/bin"].iter().map(|d| home.join(d)));
    dirs
}

/// nvm の node の bin（`alias/default` の版を先・残りは新しい順）。
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    let Ok(entries) = fs::read_dir(&root) else { return Vec::new() };
    let num = |v: &str| v.trim_start_matches('v').split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let mut versions: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with('v')).collect();
    versions.sort_by_key(|v| std::cmp::Reverse(num(v)));
    let default = fs::read_to_string(home.join(".nvm/alias/default")).unwrap_or_default();
    let want = default.trim().trim_start_matches('v');
    if !want.is_empty() {
        if let Some(i) = versions.iter().position(|v| {
            let v = v.trim_start_matches('v');
            v == want || v.starts_with(&format!("{want}."))
        }) {
            let d = versions.remove(i);
            versions.insert(0, d);
        }
    }
    versions.into_iter().map(|v| root.join(v).join("bin")).collect()
}

fn is_runnable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `name` の候補（PATH の dir の順 → `install_dirs` の順・実行できる file だけ・重ねない）。`name` に `/` が在れば候補にしない。
pub fn cli_candidates(name: &str, path: &str, home: &Path) -> Vec<PathBuf> {
    candidates_in(name, path, &install_dirs(home))
}

/// `cli_candidates` の本体（PATH の dir → `dirs` の順）。
fn candidates_in(name: &str, path: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .chain(dirs.iter().cloned())
        .map(|d| d.join(name))
        .filter(|p| is_runnable_file(p) && seen.insert(p.clone()))
        .collect()
}

/// 候補を上から `--version` で打ち、`VERSION_PROBE` の内に 0 で終わった最初の 1 つ（0 以外で終わる壊れた shim を飛ばす）。
/// B87（owner 270）: 0 で終わった物が無ければ、時間切れの最初の 1 つ（混んだ Mac〈load 127〉では本物の CLI も 3 秒で答えない・時間切れは「無い」ではない）。
/// 版の本当の確かめは呼ぶ側（`detect_from` の `--version`・`QUICK` = 20 秒）がする。
pub fn runnable(candidates: &[PathBuf], path: &str) -> Option<PathBuf> {
    let mut late = None;
    for p in candidates {
        match version_ok(p, path) {
            Some(true) => return Some(p.clone()),
            None if late.is_none() => late = Some(p.clone()),
            _ => {}
        }
    }
    late
}

/// `--version` が 0 で終わったか（`Some(false)` = 起こせない・0 以外で終わった・待てない・`None` = `VERSION_PROBE` の時間切れ）。
fn version_ok(program: &Path, path: &str) -> Option<bool> {
    let Ok(mut child) = Command::new(program)
        .arg("--version")
        .env("PATH", child_path_for(program, path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Some(false);
    };
    let deadline = Instant::now() + VERSION_PROBE;
    loop {
        let done = match child.try_wait() {
            Ok(Some(s)) => return Some(s.success()),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            Ok(None) => None,
            Err(_) => Some(false),
        };
        let _ = child.kill();
        let _ = child.wait();
        return done;
    }
}

/// CLI を起こす子の PATH: CLI の在る dir を先頭に（node の script の CLI は隣の node で動く・Orca #10932）・後ろに `path`。
pub fn child_path_for(program: &Path, path: &str) -> String {
    let dir = program.parent().map(|d| d.display().to_string()).unwrap_or_default();
    merge_path(&dir, path)
}

/// PROVIDER-1101（owner 180 通目「機械から検知したものを全て表示させるようにして」「notsupported yetにしている意味がわからない」）:
/// 行にする AI の CLI（file 名 = id, 画面の名前, Terminal で打つ sign in の command）。4 つとも `--help` で非対話の呼び方を確かめた
/// （2026-10-29: grok `--prompt-file`・kimi `-p`・opencode `run`・cursor-agent `-p`）。claude は `None` の行（app.rs の `account_label(None)` = Claude Code）なので入れない。
/// codex はこの Mac に無く非対話の呼び方を測れていないので入れない（行にして回せないのは嘘）。
pub const FOUND_CLIS: [(&str, &str, &str); 4] = [
    ("grok", "Grok", "grok login"),
    ("kimi", "Kimi Code", "kimi login"),
    ("opencode", "opencode", "opencode auth login"),
    ("cursor-agent", "Cursor Agent", "cursor-agent login"),
];

/// `FOUND_CLIS` と `LOCAL_LLMS` の画面の名前（無い id は "This AI"）。ACCOUNTS-1101 2 段目: `"claude"` は `Claude`（足した account の名前の頭・`Claude · work`）。
pub fn found_name(id: &str) -> &'static str {
    FOUND_CLIS.iter().map(|c| (c.0, c.1)).chain(LOCAL_LLMS).chain([("claude", "Claude")]).find(|(i, _)| *i == id).map_or("This AI", |(_, n)| n)
}

/// 選んだ CLI が login していない時の 1 行（`auth login` は起こさない = 利用者が Terminal で打つ command を言う）。
pub fn sign_in_line(id: &str) -> String {
    let cmd = FOUND_CLIS.iter().find(|c| c.0 == id).map_or("its sign-in command", |c| c.2);
    format!("Sign in to {} in Terminal first · run {cmd}", found_name(id))
}

/// SIGNIN-1101: `FOUND_CLIS` の sign in の command（3 つ目）を、Terminal の shell の PATH に無い置き場（`~/.kimi-code/bin` 等・`install_dirs`）でも動く形に
/// （先頭の語 = 見つけた file の path・HOME の下は `~/…`・shell が字のまま読めない path は表の command のまま）。表に無い command は None（保存の行を書き換えられても
/// 表の外は打たない）。file を見るだけ（起こさない・`--version` も打たない）。例: `sign_in_command("kimi login")` → `Some("~/.kimi-code/bin/kimi login")`。
/// ACCOUNTS-1101: 足した account の sign in（`account_sign_in_line` の `env K='v' … <表の command>`）も受ける。前置きは `<data dir>/ai-accounts/<provider>-<数字>` の dir から
/// `account_env` で作り直した字と完全に一致する時だけ（値に `'` を含む・他の変数・他の dir・表の外の command は None）。`env` で前置きする = fish でも動く。
pub fn sign_in_command(command: &str) -> Option<String> {
    let (prefix, command) = account_prefix(command)?;
    let &(id, _, table) = FOUND_CLIS.iter().find(|c| c.2 == command)?;
    let home = home_dir();
    let at = cli_candidates(id, &known_path(), &home).into_iter().next().map(|p| match p.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    });
    let plain = |p: &String| p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~+".contains(c));
    Some(match at.filter(plain) {
        Some(p) => format!("{prefix}{p}{}", &table[id.len()..]),
        None => format!("{prefix}{table}"),
    })
}

/// ACCOUNTS-1101: `env K='v' … <表の command>` を (前置き, 表の command) に分ける（前置きが無ければ ("", そのまま)）。前置きは最初の値の path か その親を
/// 置き場の dir（`ai-accounts/<provider>-<数字>`・絶対・`..` 無し）と見て `env_prefix` で作り直した字と完全に一致する時だけ Some（他の変数・他の値・継ぎ足しの command は None）。
fn account_prefix(command: &str) -> Option<(String, &str)> {
    if !command.starts_with("env ") {
        return Some((String::new(), command));
    }
    let &(id, _, table) = FOUND_CLIS.iter().find(|c| command.ends_with(&format!(" {}", c.2)))?;
    let prefix = &command[..command.len() - table.len()];
    let first = Path::new(prefix.strip_prefix("env ")?.split_once("='")?.1.split_once('\'')?.0);
    let ok = |d: &Path| is_account_dir(d, id) && env_prefix(id, d).as_deref() == Some(prefix);
    (ok(first) || first.parent().is_some_and(ok)).then(|| (prefix.to_owned(), table))
}

/// ACCOUNTS-1101: `<…>/ai-accounts/<provider>-<数字>` の絶対 path（`..`・`.` を含まない）か。
fn is_account_dir(d: &Path, provider: &str) -> bool {
    let n = d.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(provider)?.strip_prefix('-'));
    d.is_absolute()
        && d.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
        && d.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(ACCOUNTS_DIR))
        && n.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// ACCOUNTS-1101: sign in の 1 行の前置き `env K='v' … `（`account_env` の値・どれかが `'` を含めば None = shell の字で閉じられない）。
fn env_prefix(provider: &str, dir: &Path) -> Option<String> {
    let env = account_env(provider, dir)?;
    if env.iter().any(|(_, v)| v.contains('\'')) {
        return None;
    }
    Some(format!("env {} ", env.iter().map(|(k, v)| format!("{k}='{v}'")).collect::<Vec<_>>().join(" ")))
}

/// 在りかの file 名が `FOUND_CLIS` の id なら Some（grok・kimi・opencode・cursor-agent）。claude（他の名前）は None。
pub fn cli_id(program: &Path) -> Option<&'static str> {
    let name = program.file_name()?.to_str()?;
    FOUND_CLIS.iter().map(|c| c.0).find(|id| *id == name)
}

/// `FOUND_CLIS` のうち `path` の dir か `install_dirs(home)` に実行できる file が在る物（表の順）。file を見るだけで起こさない（`--version` も打たない）。
pub fn found_clis(path: &str, home: &Path) -> Vec<&'static str> {
    clis_in(&FOUND_CLIS.map(|c| c.0), path, &install_dirs(home))
}

/// L7-ORCA #12: `domain::terminal::AGENTS` のうち入っている物（`found_clis` と同じ見方・起こさない）。panel の帯の `+` の menu の agent の行。
pub fn agent_clis(path: &str, home: &Path) -> Vec<&'static str> {
    clis_in(&crate::domain::terminal::AGENTS, path, &install_dirs(home))
}

/// `found_clis` / `agent_clis` の本体（install dir を渡す = 試験は一時 dir だけを見る・この Mac の /usr/local/bin 等を読まない）。
fn clis_in(names: &[&'static str], path: &str, dirs: &[PathBuf]) -> Vec<&'static str> {
    names.iter().copied().filter(|name| !candidates_in(name, path, dirs).is_empty()).collect()
}

/// 段 2 作り直し: `names` のうち `path` の dir か `install_dirs(home)` に実行できる file が在る物（`names` の順）。`found_clis` と同じ探し方・file を見るだけ。
/// 呼ぶ側: app.rs の `refresh_agent_setup` が `terminal::SKILL_AGENTS` の名前で（Install CLI & skill の一覧の「見つかった」）と `claude` の在る / 無い。
pub fn clis_on_mac(names: &[&'static str], path: &str, home: &Path) -> Vec<&'static str> {
    clis_in(names, path, &install_dirs(home))
}

/// 動いている時だけ行にする local LLM。止めてある物の名前は出さない。
pub const LOCAL_LLMS: [(&str, &str); 2] = [("ollama", "Ollama"), ("lm-studio", "LM Studio")];

/// PROVIDER-1101: 在る CLI は全部行にする（login していない物は `needs_sign_in` = 選ぶと Terminal で sign in の 1 行・`auth login` は起こさない）。
/// 動いている local LLM も行。無い物の名前は出さない。「Found … — not supported yet」の 1 行は無くした（呼べない CLI を表に置かない）。
/// 負の対照: `needs_sign_in` を常に false にすると、login していない Cursor Agent が押せる行になって `provider1101_found_rows_list_every_ai_on_this_mac` が赤。
pub fn found_rows(signed_in: &[&str], present: &[&str], local_up: &[&str]) -> Vec<crate::model::AiAccount> {
    use crate::model::AiAccount;
    let cli = FOUND_CLIS.iter().filter(|c| present.contains(&c.0)).map(|&(id, name, _)| AiAccount::Found {
        id: id.to_owned(),
        name: name.to_owned(),
        needs_sign_in: !signed_in.contains(&id),
    });
    let local = LOCAL_LLMS.iter().filter(|(id, _)| local_up.contains(id)).map(|&(id, name)| AiAccount::Found {
        id: id.to_owned(),
        name: name.to_owned(),
        needs_sign_in: false,
    });
    cli.chain(local).collect()
}

/// local LLM の口（OpenAI 互換の `/v1`・既定の port: Ollama 11434・LM Studio 1234）。
pub const LOCAL_ENDPOINTS: [(&str, &str); 2] = [("ollama", "http://127.0.0.1:11434"), ("lm-studio", "http://127.0.0.1:1234")];

/// 動いている時だけ Some（`/v1/models` が答える）。中身は埋め込み用でない model の id（答えの順）。
/// PROVIDER-1101: 前は最初の 1 つだけを勝手に選んでいた。今は全部を `Model ▸` の一覧に（`Default` = 最初の 1 つ）。
fn local_models(base: &str, timeout: Duration) -> Option<Vec<String>> {
    let v: Value = ureq::AgentBuilder::new().timeout(timeout).build().get(&format!("{base}/v1/models")).call().ok()?.into_json().ok()?;
    Some(v["data"].as_array()?.iter().filter_map(|m| m["id"].as_str()).filter(|id| !id.contains("embed")).map(str::to_owned).collect())
}

/// PROVIDER-1101: Ollama の model のうち考える物（`/api/show` の `capabilities` に `thinking`・2026-11-01 にこの Mac の qwen3:0.6b で確かめた）。
/// 読めない物は考えない物に数える（effort を送らない）。LM Studio は読まない（同じ口が無い）。
fn thinking_models(base: &str, models: &[String], timeout: Duration) -> Vec<String> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    models
        .iter()
        .filter(|m| {
            agent
                .post(&format!("{base}/api/show"))
                .send_json(serde_json::json!({ "model": m }))
                .ok()
                .and_then(|r| r.into_json::<Value>().ok())
                .and_then(|v| v["capabilities"].as_array().map(|c| c.iter().any(|x| x == "thinking")))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Mac の中の AI を探す（裏の thread から）: 在る CLI（`FOUND_CLIS`）と login の確かめ（`signed_in`・model は呼ばない）と動いている local LLM。
/// `auth login` は起こさない。`work_dir` の下の `found-probe` だけを使う（`detect` の空の HOME と重ならない）。
/// `CLI_ENV` が在る run（空 = AI を切った起動・path = CLI を差し替えた試験と QA）は探さない（利用者の CLI を起こさず :11434 / :1234 も叩かない・`detect_account` と同じ門）。
/// 探さなかった時は None（「何も無かった」の空と分ける = 呼ぶ側は列の Found に触らず、使っている AI も替えない）。
pub fn probe_found(work_dir: &Path) -> Option<Vec<crate::model::AiAccount>> {
    probe_found_from(std::env::var_os(CLI_ENV), work_dir)
}

/// `CLI_ENV` の値を受け取る `probe_found`（試験が process の env を書き換えずに門を通すため・`detect_from` と同じ形）。
fn probe_found_from(env: Option<OsString>, work_dir: &Path) -> Option<Vec<crate::model::AiAccount>> {
    if env.is_some() {
        return None;
    }
    let (path, home) = (known_path(), home_dir());
    let present = found_clis(&path, &home);
    let probe = work_dir.join("found-probe");
    let signed: Vec<&str> = present.iter().copied().filter(|id| locate_cli(id, false).is_some_and(|p| signed_in(id, &p, &probe))).collect();
    let up: Vec<&str> = LOCAL_ENDPOINTS
        .iter()
        .filter(|(_, base)| local_models(base, Duration::from_secs(2)).is_some_and(|m| !m.is_empty()))
        .map(|(id, _)| *id)
        .collect();
    Some(found_rows(&signed, &present, &up))
}

/// login しているか（model は呼ばない）: Grok = 空の HOME で `grok models` の 1 行目・他 = `other_login`。
fn signed_in(id: &str, program: &Path, probe: &Path) -> bool {
    if id == "grok" {
        let Ok((h, gh, auth)) = prepare_empty_grok_home(probe) else { return false };
        let (h, gh, auth) = (h.display().to_string(), gh.display().to_string(), auth.display().to_string());
        let extra = [("HOME", h.as_str()), ("GROK_HOME", gh.as_str()), ("GROK_AUTH_PATH", auth.as_str())];
        return run_quick_env(program, &["models"], &extra).is_ok_and(|(_, out)| parse_grok_login(&out).is_ok());
    }
    let Ok(env) = prepare_other_env(id, probe, None) else { return false };
    let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    other_login(id, program, &extra).0
}

/// PROVIDER-1101: kimi・opencode・cursor-agent の login と model の一覧（model は呼ばない・`extra` = `prepare_other_env` の env）。
/// kimi = 利用者の `KIMI_CODE_HOME`（既定 `~/.kimi-code`）の `credentials/` に file が在る事と config.toml の `[models."…"]`（file を読むだけ）。
/// opencode = 空の env で `opencode models` が 0 で終わり 1 行以上（2026-11-01 実測: login 無しで無料の `opencode/…` が 7 つ）。
/// cursor-agent = 空の HOME で `cursor-agent models` が 0 で終わる（同日の実測: login の切れた Mac は「Authentication required」で 1 = sign in の行）。
fn other_login(id: &str, program: &Path, extra: &[(&str, &str)]) -> (bool, Vec<(String, String)>) {
    if id == "kimi" {
        // 渡した env の KIMI_CODE_HOME（`prepare_other_env` の写し = 利用者か足した account の provider と login）・無ければ利用者の置き場
        let home = extra.iter().find(|(k, _)| *k == "KIMI_CODE_HOME").map_or_else(kimi_home, |(_, v)| PathBuf::from(v));
        let config = fs::read_to_string(home.join("config.toml")).unwrap_or_default();
        let creds = fs::read_dir(home.join("credentials")).is_ok_and(|mut d| d.next().is_some());
        return (creds && config.contains("[providers."), kimi_models(&config));
    }
    match run_quick_env(program, &["models"], extra) {
        Ok((Some(0), out)) => {
            let models = listed_models(&out);
            (id != "opencode" || !models.is_empty(), models)
        }
        _ => (false, Vec::new()),
    }
}

/// PROVIDER-1101: 旗で渡せる effort の段（Grok と claude は `--help` で見る = detect の中）。opencode `--variant`・cursor-agent `model[effort=…]` は
/// low / medium / high の 3 つ（`--help` の例の語）。kimi は旗が無い（config.toml の `support_efforts` は model ごとで、呼び出しで替える口が無い）。
fn other_efforts(id: &str) -> Vec<crate::domain::chat::Effort> {
    use crate::domain::chat::Effort;
    match id {
        "opencode" | "cursor-agent" => vec![Effort::Low, Effort::Medium, Effort::High],
        _ => Vec::new(),
    }
}

/// 利用者の kimi の置き場（`KIMI_CODE_HOME` → `~/.kimi-code`）。
fn kimi_home() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME").filter(|v| !v.is_empty()).map_or_else(|| home_dir().join(".kimi-code"), PathBuf::from)
}

/// PROVIDER-1101: kimi・opencode・cursor-agent の子の env（Grok の空の HOME と同じ守り・`dir` の下に作り直す）。鍵の変数は `command_line` が外す。
/// どれも HOME は空の dir（利用者の `~/.claude`・`~/.cursor/skills`・rc・memory を読ませない）。
/// kimi = `KIMI_CODE_HOME` に利用者の config.toml の `[providers.…]`・`[models.…]` の表だけ（`kimi_login_config`・hooks と services を写さない）と login の
/// `credentials`（symlink）・skill は空の `no-skills`（`--skills-dir`）。2026-11-01 実測: 空の `KIMI_CODE_HOME` は「No providers configured.」・この形は provider を 1 つ読んだ。
/// opencode = XDG の 4 つを空の dir に・設定は tool を全部切った 1 枚（`opencode debug agent build` で tools が全部 false を確かめた）・
/// `OPENCODE_DISABLE_CLAUDE_CODE=1`（`~/.claude/CLAUDE.md` を読む口を閉じる）。cursor-agent = 空の HOME に login の置き場の `Library/Keychains` だけ（symlink・
/// `~/.cursor` の skill・rules・mcp・hooks は無い）。
/// ACCOUNTS-1101: `login` = 足した account の置き場（`cli_account_dir`・None = 利用者の login）。kimi は利用者の `KIMI_CODE_HOME` の代わりにそこから写し、
/// opencode は `XDG_DATA_HOME` をそこの `data`（`opencode auth login` の置き場 = `account_env`）にする。cursor-agent は分けられないので見ない。
pub fn prepare_other_env(id: &str, dir: &Path, login: Option<&Path>) -> std::io::Result<Vec<(&'static str, String)>> {
    let base = dir.join(format!("{id}-home"));
    let _ = fs::remove_dir_all(&base);
    let home = base.join("home");
    fs::create_dir_all(&home)?;
    fn at(p: &Path) -> String {
        p.display().to_string()
    }
    let mut env = vec![("HOME", at(&home))];
    match id {
        "kimi" => {
            let kimi = base.join("kimi");
            fs::create_dir_all(&kimi)?;
            fs::create_dir_all(dir.join("no-skills"))?;
            let user = login.map_or_else(kimi_home, Path::to_path_buf);
            fs::write(kimi.join("config.toml"), kimi_login_config(&fs::read_to_string(user.join("config.toml")).unwrap_or_default()))?;
            if user.join("credentials").is_dir() {
                std::os::unix::fs::symlink(user.join("credentials"), kimi.join("credentials"))?;
            }
            env.extend([("KIMI_CODE_HOME", at(&kimi)), ("KIMI_CODE_NO_AUTO_UPDATE", "1".into())]);
        }
        "opencode" => {
            let config = base.join("config");
            fs::create_dir_all(config.join("opencode"))?;
            fs::write(config.join("opencode/opencode.json"), OPENCODE_NO_TOOLS)?;
            for d in ["data", "cache", "state"] {
                fs::create_dir_all(base.join(d))?;
            }
            env.extend([
                ("XDG_CONFIG_HOME", at(&config)),
                ("XDG_DATA_HOME", at(&login.map_or_else(|| base.join("data"), |l| l.join("data")))),
                ("XDG_CACHE_HOME", at(&base.join("cache"))),
                ("XDG_STATE_HOME", at(&base.join("state"))),
                ("OPENCODE_DISABLE_CLAUDE_CODE", "1".into()),
            ]);
        }
        "cursor-agent" => {
            // login は Keychain（2026-11-01 実測: 空の HOME の `status` は Not logged in・`Library/Keychains` だけ繋ぐと利用者の HOME と同じ答え）
            fs::create_dir_all(home.join("Library"))?;
            std::os::unix::fs::symlink(home_dir().join("Library/Keychains"), home.join("Library/Keychains"))?;
        }
        _ => {}
    }
    Ok(env)
}

/// opencode の空の設定: tool を全部切り・許可は全部断る・共有と自動更新を切る（`opencode debug config` が 0 で読んだ形・2026-11-01 の 1.18.10）。
const OPENCODE_NO_TOOLS: &str = r#"{"$schema":"https://opencode.ai/config.json","permission":{"*":"deny"},"tools":{"*":false},"share":"disabled","autoupdate":false}"#;

/// kimi の config.toml から `[providers.…]`・`[models.…]` の表だけ（`[[hooks]]`・`[services.…]`・`[thinking]` を落とす = 利用者の hook を起こさない）。
pub fn kimi_login_config(toml: &str) -> String {
    let mut keep = false;
    let mut out = String::new();
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            keep = t.trim_start_matches('[').starts_with("providers.") || t.trim_start_matches('[').starts_with("models.");
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// kimi の config.toml の `[models."<別名>"]` の別名（`-m` に渡す語）と `display_name`（無ければ別名）。表の順。
pub fn kimi_models(toml: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut inside = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            let alias = line.strip_prefix("[models.").and_then(|r| r.strip_suffix(']')).map(|a| a.trim_matches('"').to_owned());
            inside = alias.is_some();
            if let Some(a) = alias {
                out.push((a.clone(), a));
            }
        } else if inside {
            if let Some(v) = line.strip_prefix("display_name").map(str::trim_start).and_then(|r| r.strip_prefix('=')) {
                if let Some(last) = out.last_mut() {
                    last.1 = v.trim().trim_matches('"').to_owned();
                }
            }
        }
    }
    out
}

/// PROVIDER-1101: `grok models`・`opencode models`・`cursor-agent models` の答えの行から model の一覧（渡す語, menu の語）。
/// 1 行に 1 つ: 先頭の `-`・`*`・`•` を外し、` - ` か 2 つ以上の空白で分けた前を id（空白の無い 1 語）・後ろを語（無ければ id）。`(current)`・`(default)` は外す。
/// 見出しの行（`:` で終わる・空白を含む 1 列・`You are logged in …`）は飛ばす。実測は opencode の `provider/model` の 1 列だけ（grok と cursor-agent の行の形は未測）。
pub fn listed_models(out: &str) -> Vec<(String, String)> {
    let mut models: Vec<(String, String)> = Vec::new();
    for line in strip_ansi(out).lines() {
        let line = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if line.is_empty() || line.ends_with(':') || line.starts_with("You are") {
            continue;
        }
        let clean = |s: &str| s.replace("(current)", "").replace("(default)", "").trim().to_owned();
        let (id, label) = match line.split_once(" - ").or_else(|| line.split_once("  ")) {
            Some((a, b)) => (clean(a), clean(b)),
            None => (clean(line), String::new()),
        };
        let ok = !id.is_empty() && !id.contains(char::is_whitespace) && id.chars().any(|c| c.is_ascii_alphabetic());
        if ok && !models.iter().any(|(m, _)| *m == id) {
            let label = if label.is_empty() { id.clone() } else { label };
            models.push((id, label));
        }
    }
    models
}

/// 動いている local LLM（Ollama・LM Studio）の 1 回の呼び出し。鍵も login も要らない・Mac の外へ出ない。
/// 例: `LocalLlm::detect_at("ollama", "http://127.0.0.1:11434", Duration::from_secs(120))?.complete_json(req)`
pub struct LocalLlm {
    base: String,
    id: String,
    /// `Default` の model（`/v1/models` の最初の 1 つ）
    model: String,
    /// PROVIDER-1101: 選べる model（`/v1/models` の埋め込み用でない物・答えの順）
    models: Vec<String>,
    /// PROVIDER-1101: そのうち考える model（`reasoning_effort` を送る物・`thinking_models`）
    thinking: Vec<String>,
    timeout: Duration,
    /// CHAT-ONE-1101: `CliConfig::log`（`llm-calls.jsonl`）。CLI と同じく 1 回 1 行（前は local LLM の呼び出しが残らず、Engine の「120 秒で答えなかった」がどの AI か後から見えなかった）
    log: Option<PathBuf>,
}

impl LocalLlm {
    pub fn detect_at(id: &str, base: &str, timeout: Duration) -> Result<Self, LlmError> {
        let models = local_models(base, Duration::from_secs(3)).filter(|m| !m.is_empty()).ok_or_else(|| no_cli("That AI isn't running on this Mac"))?;
        let thinking = if id == "ollama" { thinking_models(base, &models, Duration::from_secs(3)) } else { Vec::new() };
        Ok(Self { base: base.to_owned(), id: id.to_owned(), model: models[0].clone(), models, thinking, timeout, log: None })
    }
}

impl Llm for LocalLlm {
    fn label(&self) -> String {
        format!("{}/{}", self.id, self.model)
    }

    fn provider_name(&self) -> String {
        found_name(&self.id).into()
    }

    /// PROVIDER-1101: 考える model が 1 つでも在れば low / medium / high（OpenAI 互換の `reasoning_effort` の 3 つ）。無ければ空（`Accepts::refusal(None)` の 1 行）。
    // ponytail: effort の段は provider ごと（考えない model を選んでも段は出る = その呼び出しでは送らない）。model ごとに出し分けるなら Accepts に model ごとの段を
    fn efforts(&self) -> Vec<&'static str> {
        use crate::domain::chat::Effort;
        if self.thinking.is_empty() { Vec::new() } else { [Effort::Low, Effort::Medium, Effort::High].map(Effort::flag).to_vec() }
    }

    fn models(&self) -> Vec<(String, String)> {
        self.models.iter().map(|m| (m.clone(), m.clone())).collect()
    }

    /// CHAT-ONE-1101: 呼び出しの記録（`CliLlm` と同じ `CallRecord`・中身は書かない）。書けなくても呼ぶ（local LLM は利用者の quota を使わない）。
    fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
        let (at_ms, started) = (now_ms(), Instant::now());
        let result = self.post(req);
        if let Some(path) = &self.log {
            let rec = CallRecord {
                at_ms,
                label: self.label(),
                name: req.name.clone(),
                ms: started.elapsed().as_millis() as u64,
                exit_code: None,
                outcome: result.as_ref().map_or_else(|e| kind(e).into(), |_| "ok".into()),
                stopped_after_ms: None,
                input_tokens: None,
                prompt_chars: Some(prompt_chars(&req.instructions, &req.schema.to_string(), &req.input)),
                // USAGE-OWN-1101 の欄: local LLM は token も費用も account も持たない（利用者の quota を使わない）・頼んだ Project だけ
                output_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_micros: None,
                account: None,
                project: req.project.clone(),
                session: None,
            };
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{}", serde_json::to_string(&rec).unwrap_or_default());
            }
        }
        result
    }
}

impl LocalLlm {
    /// 1 回の呼び出しの本体（`send` が時間を測り `CallRecord` を 1 行足す）。
    fn post(&self, req: &JsonRequest) -> Result<Value, LlmError> {
        // PROVIDER-1101: 選んだ model（一覧に在る物だけ）・effort は考える model の時だけ
        let model = req.model.as_deref().filter(|m| self.models.iter().any(|x| x == m)).unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "stream": false,
            "max_tokens": req.max_tokens,
            "messages": [
                {"role": "system", "content": req.instructions},
                {"role": "user", "content": req.input},
            ],
            "response_format": {"type": "json_schema", "json_schema": {"name": req.name, "strict": true, "schema": req.schema}},
        });
        if let Some(e) = req.effort.as_deref().filter(|_| self.thinking.iter().any(|t| t == model)) {
            body["reasoning_effort"] = Value::from(if e == "low" || e == "medium" { e } else { "high" });
        }
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        let resp = agent.post(&format!("{}/v1/chat/completions", self.base)).send_json(body).map_err(|e| match e {
            ureq::Error::Status(code, _) if code >= 500 => LlmError::Server { status: code },
            ureq::Error::Status(code, _) => LlmError::Rejected { status: code, detail: String::new() },
            ureq::Error::Transport(t) if t.to_string().contains("timed out") => LlmError::Timeout { ms: self.timeout.as_millis() as u64 },
            ureq::Error::Transport(t) => LlmError::Network { detail: short(&t.to_string()) },
        })?;
        let v: Value = resp.into_json().map_err(|_| LlmError::Malformed { detail: "not JSON".into() })?;
        let text = v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).unwrap_or("").trim();
        let text = text.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
        serde_json::from_str(text).map_err(|_| LlmError::Malformed { detail: "the answer isn't JSON".into() })
    }
}

/// `CliLlm::detect` の入口を account で分ける（app の確かめの thread が呼ぶ）。見つかった local LLM はその口へ・他は今まで通り CLI。
/// `CLI_ENV` が空（AI を切った run）は local LLM も使わない。
pub fn detect_account(config: CliConfig) -> Result<Box<dyn Llm>, LlmError> {
    use crate::model::AiAccount;
    if let Some(AiAccount::Found { id, .. }) = &config.account {
        if let Some((_, base)) = LOCAL_ENDPOINTS.iter().find(|(i, _)| i == id) {
            if std::env::var_os(CLI_ENV).is_some_and(|v| v.is_empty()) {
                return Err(no_cli("AI is off for this launch"));
            }
            return LocalLlm::detect_at(id, base, config.timeout).map(|l| Box::new(LocalLlm { log: config.log.clone(), ..l }) as Box<dyn Llm>);
        }
    }
    CliLlm::detect(config).map(|c| Box::new(c) as Box<dyn Llm>)
}

/// G84: `--help` の `--model` の行（と続きの説明の行）が `'別名'` で挙げる `domain::chat::MODELS` の行（渡す語, menu の語）。`--model` が無い・別名が 1 つも無ければ空。
pub fn help_models(help: &str) -> Vec<(String, String)> {
    let mut lines = help.lines().skip_while(|l| !l.trim_start().starts_with("--model "));
    let Some(first) = lines.next() else { return Vec::new() };
    // 説明の続きの行（次の `-` で始まる旗の行まで）
    let text: Vec<&str> = std::iter::once(first).chain(lines.take_while(|l| !l.trim_start().starts_with('-'))).collect();
    let text = text.join(" ");
    crate::domain::chat::MODELS.iter().filter(|(alias, _)| text.contains(&format!("'{alias}'"))).map(|(a, l)| (a.to_string(), l.to_string())).collect()
}

/// `name` の動く 1 つの絶対 path（`login_path(force)` で 2〜4）。
pub fn locate_cli(name: &str, force: bool) -> Option<PathBuf> {
    let path = login_path(force);
    runnable(&cli_candidates(name, &path, &home_dir()), &path)
}

/// この provider だけの PTY の session（Project の Terminal と別の鍵・Project に保存しない）。
fn session_spec(what: &str, shell: &str, work_dir: &Path) -> TerminalSessionSpec {
    TerminalSessionSpec {
        id: format!("llm-cli.{what}.{}", unique()),
        project_id: "shibubu.llm-cli".into(),
        shell: shell.into(),
        working_directory: work_dir.display().to_string(),
        keep_alive_on_page_switch: false, command: None,
    }
}

/// `$SHELL`（絶対 path で在る物）、無ければ /bin/zsh（`generated::user_shell` と同じ規則）。
fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| s.starts_with('/') && Path::new(s).exists())
        .unwrap_or_else(|| "/bin/zsh".into())
}

/// 呼び出しごとの作業 dir（`<work_dir>/llm-cli.<id>`）。prompt（利用者の一言）を置くので、呼んだ後に消す。
fn scratch(work_dir: &Path) -> Result<PathBuf, LlmError> {
    let dir = work_dir.join(format!("llm-cli.{}", unique()));
    fs::create_dir_all(&dir)
        .map(|_| dir)
        .map_err(|e| no_cli_log("the AI call couldn't be prepared on disk", e.kind()))
}

fn unique() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 親が Claude Code の中で起こした Shibubu（開発の `cargo run`・試験）の時、子の CLI が親の session（socket・子 session の印・effort）に
/// 繋がらないよう外す変数。Finder から開いた .app には無い。`CLAUDE_CONFIG_DIR` など利用者の設定は外さない。
fn is_parent_session_var(name: &str) -> bool {
    name == "CLAUDECODE"
        || name.starts_with("CLAUDE_CODE_")
        || name == "CLAUDE_PID"
        || name == "CLAUDE_EFFORT"
}

fn unset_line() -> String {
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| {
            is_parent_session_var(k) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("unset {}\n", names.join(" "))
    }
}

/// RQ2: `child_env` の置く値（鍵・設定の dir）を script の export 行に（argv に出さない・`ps` に見えない）。
fn exports(env: &[(&str, Option<String>)]) -> String {
    env.iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("export {k}={}\n", quote(v))))
        .collect()
}

/// shell の 1 語（single quote）。
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `run_quick` と `sign_in` の子（model を呼ばない）: CLI は絶対 path・PATH は CLI の dir を先頭に login の PATH・stdin / stdout / stderr は閉じる・
/// 鍵と親の session の変数を外し、最後に `env`（`child_env` の差分 = plan の account の `CLAUDE_CONFIG_DIR`）を当てる。
fn child_command(program: &Path, args: &[&str], env: &[(&str, Option<String>)]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("PATH", child_path_for(program, &login_path(false)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(is_parent_session_var) {
            cmd.env_remove(k);
        }
    }
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    cmd
}

/// `run_quick` に環境を足す（scrub の後に置くので、足した名前は残る）。Grok の login の確かめが HOME を空にする。
fn run_quick_env(
    program: &Path,
    args: &[&str],
    extra: &[(&str, &str)],
) -> Result<(Option<i32>, String), String> {
    let owned: Vec<(&str, Option<String>)> = extra.iter().map(|(k, v)| (*k, Some((*v).to_string()))).collect();
    run_quick(program, args, &owned)
}

/// model を呼ばない短い command（`--version`・`auth status`・Grok の `models`）。`child_command` の子の stdout を読む・timeout で止める。
fn run_quick(program: &Path, args: &[&str], env: &[(&str, Option<String>)]) -> Result<(Option<i32>, String), String> {
    let mut child = child_command(program, args, env).stdout(Stdio::piped()).spawn().map_err(|e| e.kind().to_string())?;
    let deadline = Instant::now() + QUICK;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.kind().to_string())? {
            let mut s = String::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_string(&mut s);
            }
            return Ok((status.code(), s));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("No answer within {} s", QUICK.as_secs()));
        }
        // ponytail: 20ms の polling（答えは小さく、pipe の buffer を超えない）
        thread::sleep(Duration::from_millis(20));
    }
}

/// `CallRecord::outcome` の語。
fn kind(e: &LlmError) -> &'static str {
    match e {
        LlmError::NoKey => "no_key",
        LlmError::NoCli { .. } => "no_cli",
        LlmError::Auth { .. } => "auth",
        LlmError::Quota { .. } => "quota",
        LlmError::PlanLimit { .. } => "plan_limit",
        LlmError::RateLimited { .. } => "rate_limited",
        LlmError::Server { .. } => "server",
        LlmError::Rejected { .. } => "rejected",
        LlmError::Timeout { .. } => "timeout",
        LlmError::Network { .. } => "network",
        LlmError::Refused { .. } => "refused",
        LlmError::Malformed { .. } => "malformed",
        LlmError::Invalid { .. } => "invalid",
        LlmError::Schema { .. } => "schema",
        LlmError::OverBudget { .. } => "over_budget",
        LlmError::Cancelled { .. } => "cancelled",
    }
}

/// error の文言に入れる CLI の文（120 文字で切る）。
fn short(s: &str) -> String {
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s.to_string()
    }
}

/// G104: 添えた file を `dir/files/` に写し、一言の後ろに file の名前（`files/<名前>`）を書いた stdin の文を返す（無ければ一言のまま）。
/// 同じ名前の file は頭に番号を付けて分ける。HOME-ATTACH-1101: folder（と展開した zip）は中身ごと写し、中の file を 1 つずつ書く
/// （model の道具は Read だけで folder の中を一覧できない）。隠し file・`__MACOSX`・symlink は写さない（`domain::chat::skipped_in_folder` と同じ 2 つ・llm は domain に依らないので写し）。
fn attach_files(dir: &Path, req: &JsonRequest) -> std::io::Result<String> {
    let mut input = req.input.clone();
    if !req.files.is_empty() {
        let files = dir.join("files");
        fs::create_dir_all(&files)?;
        input.push_str("\n\n---\nThe person attached these files. Read each one with the Read tool before you answer, and use what is in them:\n");
        for (i, path) in req.files.iter().enumerate() {
            let name = path.file_name().map_or_else(|| format!("file-{i}"), |n| n.to_string_lossy().into_owned());
            // 反証 G-A: 付けた番号の名前も重なりうる（notes.md・2-notes.md・別の folder の notes.md）= 空いている番号まで進める
            let name = if files.join(&name).exists() { (2..).map(|n| format!("{n}-{name}")).find(|n| !files.join(n).exists()).unwrap_or_default() } else { name };
            if path.is_dir() {
                for inner in copy_tree(path, &files.join(&name))? {
                    input.push_str(&format!("- files/{name}/{}\n", inner.display()));
                }
            } else {
                fs::copy(path, files.join(&name))?;
                input.push_str(&format!("- files/{name}\n"));
            }
        }
    }
    Ok(input)
}

/// HOME-ATTACH-1101: folder `from` を `to` に中身ごと写し、写した file の `to` からの相対 path を返す（名前の順）。隠し file・`__MACOSX`・symlink は写さない。
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fs::create_dir_all(to)?;
    let mut entries: Vec<_> = fs::read_dir(from)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let ft = e.file_type()?;
        if name.to_string_lossy().starts_with('.') || name == "__MACOSX" || ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            out.extend(copy_tree(&e.path(), &to.join(&name))?.into_iter().map(|p| Path::new(&name).join(p)));
        } else {
            fs::copy(e.path(), to.join(&name))?;
            out.push(PathBuf::from(&name));
        }
    }
    Ok(out)
}

/// file 名が `grok` の時だけ Grok の argv（claude の path に `grok` という部分が在っても claude のまま）。
pub fn is_grok(program: &Path) -> bool {
    program.file_name().and_then(|n| n.to_str()) == Some("grok")
}

/// 空にする dir。`(work_dir/grok-home, work_dir/grok-home/.grok)`。
pub fn grok_dirs(work_dir: &Path) -> (PathBuf, PathBuf) {
    let home = work_dir.join("grok-home");
    let grok_home = home.join(".grok");
    (home, grok_home)
}

/// login の file だけ。子の HOME は空なので、親の HOME の `~/.grok/auth.json` を指す。
pub fn grok_auth_path() -> PathBuf {
    home_dir().join(".grok").join("auth.json")
}

/// 呼び出しのたびに空の HOME を作り直す（前の呼び出しの中身を残さない）。auth.json はここには置かない。
pub fn prepare_empty_grok_home(work_dir: &Path) -> std::io::Result<(PathBuf, PathBuf, PathBuf)> {
    let (home, grok_home) = grok_dirs(work_dir);
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&grok_home)?;
    Ok((home, grok_home, grok_auth_path()))
}

/// instructions・schema・一言の文字数（中身は記録しない）。混ざりの上限の土台。
pub fn prompt_chars(instructions: &str, schema: &str, input: &str) -> u64 {
    (instructions.chars().count() + schema.chars().count() + input.chars().count()) as u64
}

/// これを超える `usage.input_tokens` は、規則・記憶・skill が混ざったと見る（107 通目）。
pub fn input_token_cap(instructions: &str, schema: &str, input: &str) -> u64 {
    prompt_chars(instructions, schema, input) + 8_000
}

/// 入力の token の和 = `usage` の `input_tokens` ＋ `cache_read_input_tokens` ＋ `cache_creation_input_tokens`（claude は prompt の cache に本文を入れ、
/// `input_tokens` だけだと 2 のような値になる = 混ざりの上限が効かない・RULE-043）。Grok に cache の欄が無ければ `input_tokens` だけ。
/// 3 つとも無い・JSON でない時は None（答えの成否とは別）。
pub fn input_tokens_of(stdout: &str) -> Option<u64> {
    meter_of(stdout)?.input
}

/// USAGE-OWN-1101: 1 回の答えの量（`CallRecord` の token・費用・session の欄の元）。中身（`result`・`structured_output`）は読まない。
/// 新しくした理由: 前の `input_tokens_of` は入力の和 1 つだけを返し、出力・cache の内訳・費用・session を持てない（`input_tokens_of` はこれの `input`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meter {
    /// `input_tokens_of` と同じ 3 欄の和（無ければ None）
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_creation: Option<u64>,
    /// `total_cost_usd` × 1,000,000（claude -p だけが返す）
    pub cost_micros: Option<u64>,
    /// `session_id`（claude -p だけが返す）
    pub session: Option<String>,
}

/// USAGE-OWN-1101: 答えの JSON から `Meter` を読む。JSON でなければ None（欄が無いだけなら各欄が None）。
/// 例: `meter_of(r#"{"usage":{"input_tokens":2,"cache_read_input_tokens":40,"output_tokens":9},"total_cost_usd":0.0123,"session_id":"s1"}"#)`
/// → `input` 42・`output` 9・`cache_read` 40・`cost_micros` 12300・`session` "s1"。
pub fn meter_of(stdout: &str) -> Option<Meter> {
    let v = serde_json::from_str::<Value>(stdout.trim()).ok()?;
    let n = |k: &str| v.get("usage").and_then(|u| u.get(k)).and_then(Value::as_u64);
    let fields: Vec<u64> = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"].iter().filter_map(|k| n(k)).collect();
    Some(Meter {
        input: (!fields.is_empty()).then(|| fields.iter().sum()),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_creation: n("cache_creation_input_tokens"),
        cost_micros: v.get("total_cost_usd").and_then(Value::as_f64).filter(|d| d.is_finite() && *d >= 0.0).map(|d| (d * 1_000_000.0).round() as u64),
        session: v.get("session_id").and_then(Value::as_str).map(str::to_owned),
    })
}

/// `grok models` の最初の空でない行。model は呼ばない。違う時は `NoCli`（Sign in の button は claude の文だけに付けるので、ここは付けない）。
pub fn parse_grok_login(stdout: &str) -> Result<(), LlmError> {
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if first == "You are logged in with grok.com." {
        Ok(())
    } else {
        Err(no_cli("Grok isn't signed in · run grok login in Terminal"))
    }
}

/// claude は先頭の語が数字の時だけ。Grok の `grok 1.0.70 …` は数字で始まる最初の語。
fn version_token(out: &str, grok: bool) -> Option<&str> {
    let digit = |v: &&str| v.starts_with(|c: char| c.is_ascii_digit());
    if grok {
        out.split_whitespace().find(digit)
    } else {
        out.split_whitespace().next().filter(digit)
    }
}

/// 打つ command（argv）。claude は `/usr/bin/env -u <SCRUBBED_ENV>… <program> -p --output-format json --json-schema <schema> --system-prompt <instructions>
/// --tools "" --no-session-persistence --strict-mcp-config --setting-sources local`。`prompt_file` は使わない（一言は stdin）。
/// file 名が `grok` の時は実測の argv（107 通目）: `HOME` と `GROK_HOME` と `GROK_AUTH_PATH` を env の引数にし、`--prompt-file`・`--system-prompt-override`・`--tools todo_write`・`--max-turns 1`。
/// `grok_env` は (HOME, GROK_HOME, GROK_AUTH_PATH)。file や effort が在ってもこの形のまま（claude の旗を足さない）。
/// G104（claude だけ）: `req.effort` が在れば `--effort <語>`・`req.files` が在れば `--tools ""` の代わりに `--tools Read --restricted`。
/// PROVIDER-1101: Grok は `req.effort` を `--reasoning-effort <語>`・`req.model` を `-m <id>`（`--effort` と `--model` は付けない）。
/// kimi = `-p <prompt> --output-format text --skills-dir <空> [-m 別名]`・opencode = `run --pure --format default [-m provider/model] [--variant 段] <prompt>`・
/// cursor-agent = `-p --output-format text --mode ask --sandbox enabled --trust --workspace <呼び出しの dir> [--model id[effort=段]] <prompt>`。
/// 3 つは `env` の Some を `K=V`（`prepare_other_env` の HOME と設定の dir・鍵は無い）にし、`SCRUBBED_ENV` を全部 `-u`。prompt は `PROMPT_ARG`（呼ぶ側が file の中身に替える）。
/// 保証する: `--bare` を付けない。claude は `req.model` が在る時だけ `--model`。claude と Grok は利用者の一言（`req.input`）を argv に入れない
/// （kimi・opencode・cursor-agent は prompt を argv でしか受けない = 同じ利用者の `ps` には見える）。
/// RQ2: claude の `-u` は `env` の None だけ（置く値は script の export・argv に出さない）。Grok は `SCRUBBED_ENV` を全部 `-u` する。shell の quote は呼ぶ側（`send`）がする。
pub fn command_line(
    program: &Path,
    req: &JsonRequest,
    env: &[(&str, Option<String>)],
    prompt_file: &Path,
    grok_env: Option<(&Path, &Path, &Path)>,
) -> Vec<String> {
    let mut argv = vec!["/usr/bin/env".to_string()];
    if is_grok(program) {
        for var in SCRUBBED_ENV {
            argv.extend(["-u".to_string(), var.to_string()]);
        }
        let empty = Path::new("");
        let (home, grok_home, auth) = grok_env.unwrap_or((empty, empty, empty));
        argv.push(format!("HOME={}", home.display()));
        argv.push(format!("GROK_HOME={}", grok_home.display()));
        argv.push(format!("GROK_AUTH_PATH={}", auth.display()));
        argv.push(program.display().to_string());
        argv.extend([
            "--prompt-file".to_string(),
            prompt_file.display().to_string(),
            "--output-format".into(),
            "json".into(),
            "--json-schema".into(),
            req.schema.to_string(),
            "--system-prompt-override".into(),
            req.instructions.clone(),
            "--tools".into(),
            "todo_write".into(),
            "--no-subagents".into(),
            "--disable-web-search".into(),
            "--max-turns".into(),
            "1".into(),
        ]);
        if let Some(e) = &req.effort {
            argv.extend(["--reasoning-effort".to_string(), e.clone()]);
        }
        if let Some(m) = &req.model {
            argv.extend(["-m".to_string(), m.clone()]);
        }
        return argv;
    }
    if let Some(id) = cli_id(program) {
        for var in SCRUBBED_ENV {
            argv.extend(["-u".to_string(), var.to_string()]);
        }
        argv.extend(env.iter().filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={v}"))));
        argv.push(program.display().to_string());
        let dir = prompt_file.parent().unwrap_or(Path::new("."));
        let words: Vec<String> = match id {
            "kimi" => {
                let mut w: Vec<String> = vec!["-p".into(), PROMPT_ARG.into(), "--output-format".into(), "text".into(), "--skills-dir".into(), dir.join("no-skills").display().to_string()];
                if let Some(m) = &req.model {
                    w.extend(["-m".to_string(), m.clone()]);
                }
                w
            }
            "opencode" => {
                let mut w = vec!["run".to_string(), "--pure".into(), "--format".into(), "default".into()];
                if let Some(m) = &req.model {
                    w.extend(["-m".to_string(), m.clone()]);
                }
                if let Some(e) = &req.effort {
                    w.extend(["--variant".to_string(), e.clone()]);
                }
                w.push(PROMPT_ARG.into());
                w
            }
            _ => {
                let mut w: Vec<String> = ["-p", "--output-format", "text", "--mode", "ask", "--sandbox", "enabled", "--trust", "--workspace"].map(str::to_string).to_vec();
                w.push(dir.display().to_string());
                // ponytail: effort は model の名前の `[effort=…]` でしか渡せない（Default の model には付けられず送らない）。Default に付ける口が測れたら足す
                if let Some(m) = &req.model {
                    let m = req.effort.as_ref().map_or_else(|| m.clone(), |e| format!("{m}[effort={e}]"));
                    w.extend(["--model".to_string(), m]);
                }
                w.push(PROMPT_ARG.into());
                w
            }
        };
        argv.extend(words);
        return argv;
    }
    for (var, _) in env.iter().filter(|(_, v)| v.is_none()) {
        argv.extend(["-u".to_string(), var.to_string()]);
    }
    argv.push(program.display().to_string());
    argv.extend(
        [
            "-p",
            "--output-format",
            "json",
            "--json-schema",
            &req.schema.to_string(),
            "--system-prompt",
            &req.instructions,
            "--tools",
            if req.files.is_empty() { "" } else { "Read" },
            "--no-session-persistence",
            "--strict-mcp-config",
            "--setting-sources",
            "local",
        ]
        .map(str::to_string),
    );
    if !req.files.is_empty() {
        argv.push("--restricted".into());
    }
    if let Some(e) = &req.effort {
        argv.extend(["--effort".to_string(), e.clone()]);
    }
    if let Some(m) = &req.model {
        argv.extend(["--model".to_string(), m.clone()]);
    }
    argv
}

/// `claude auth status --json` の stdout を読む。Ok は plan の login（`loggedIn == true` かつ `authMethod == "claude.ai"`）だけ。
/// それ以外は `LlmError::NoCli`（login していない →「claude auth login で login してください」・plan でない →「plan の login に替えてください（鍵で使った分の請求にしない）」）。
pub fn parse_auth_status(stdout: &str) -> Result<(), LlmError> {
    let v: Value =
        serde_json::from_str(stdout.trim()).map_err(|_| no_cli("the sign-in state couldn't be read"))?;
    if v["loggedIn"] != Value::Bool(true) {
        return Err(no_cli(NOT_SIGNED_IN));
    }
    if v["authMethod"] != "claude.ai" {
        return Err(no_cli(KEY_SIGN_IN));
    }
    Ok(())
}

/// PROVIDER-1101: `command_line` の argv の中で prompt の file の中身に替わる印（`call_in` が `"$(cat <file>)"` に替える）。
pub const PROMPT_ARG: &str = "__SHIBUBU_PROMPT__";

/// PROVIDER-1101: system prompt と schema の旗が無い CLI（kimi・opencode・cursor-agent）に渡す 1 つの文（instructions → schema → 一言）。
pub fn plain_prompt(instructions: &str, schema: &str, input: &str) -> String {
    format!("{instructions}\n\nAnswer with only one JSON object that matches this JSON schema. No other text and no code fence.\n{schema}\n\n---\n{input}")
}

/// CLI の文が login の切れた事を言っているか（cursor-agent の実測「Authentication required. Run 'agent login'…」と他の CLI の語を広めに）。
fn signed_out(text: &str) -> bool {
    let low = text.to_lowercase();
    ["authentication required", "not logged in", "not signed in", "please log in", "please login", "unauthorized"].iter().any(|w| low.contains(w))
}

/// PROVIDER-1101: kimi・opencode・cursor-agent の文の答えから JSON の object を 1 つ（色の印を消し、最初の `{` から最後の `}` まで・code fence の中でも読める）。
/// 無ければ: login の語 → sign in の 1 行（`NoCli`）・上限の語 → `RateLimited`・他 → `Malformed`（CLI の 1 行目を短く・email と鍵の語を含む行は出さない）。
/// 検証（schema に合うか）は今までどおり `complete_json`。答えの形は未測（本物の呼び出しは束の最後の 1 回）。
pub fn parse_text_answer(stdout: &str, id: &str) -> Result<Value, LlmError> {
    let text = strip_ansi(stdout);
    if let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) {
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(text.get(a..=b).unwrap_or("")) {
            return Ok(v);
        }
    }
    if signed_out(&text) {
        return Err(no_cli(&sign_in_line(id)));
    }
    let low = text.to_lowercase();
    if ["rate limit", "usage limit", "quota"].iter().any(|w| low.contains(w)) {
        return Err(LlmError::RateLimited { retry_after: None });
    }
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let name = found_name(id);
    let detail = if first.contains('@') || first.to_lowercase().contains("key") {
        format!("{name} answered without JSON")
    } else {
        format!("{name} answered without JSON: {}", short(first))
    };
    Err(LlmError::Malformed { detail })
}

/// `--output-format json` の stdout（1 個の object）から、model が書いた構造化の JSON を取り出す（未検証のまま返す・検証は `complete_json`）。
/// `is_error` の答え: 429 は `RateLimited`（plan の上限の「resets」を含む時を除く）・「limit」の語は plan の使用量の上限 `PlanLimit`・5xx は `Server`・
/// 401 / 403 は login し直しの `NoCli`（`SIGNED_OUT`・G97: 鍵は使っていない。文が `selected model` の拒否なら Sign in にせず `Malformed`）・接続の語（`offline`）は `Network`、
/// 断りは `Refused`、JSON でない・欄が無い → `Malformed`。上限に当たった時の本物の答えの形は未測（module の doc）。
/// 答えの形が読めない時の 1 行（CLI は走り切った・model の答えが schema の JSON でない）。G84 の `model_error` はこれを model の失敗に替えない。
const UNEXPECTED: &str = "Claude Code's answer was in an unexpected form. Try again";

/// G84: model を選んだ呼び出しの失敗の次の一手（Home の `+` の `Model ▸` で Default に戻す）。
pub const MODEL_STEP: &str = "choose Default in + › Model";

/// G84: `--model` を付けた呼び出しの失敗の 1 行。CLI の答えが model の拒否と読めた時だけ `{Model} isn't available on this account`・
/// 読めない失敗（走り切らなかった答え・その他の 4xx）は `Couldn't run with {Model}`・どちらも ` · ` の後に `MODEL_STEP`。
/// plan の上限・login・鍵・混雑・5xx・時間切れ・network・断り・答えの形（`UNEXPECTED`）は今の 1 行のまま（model のせいと言わない）。
/// 生の文は画面に出さず、log の 1 行だけ（`mask_for_log` で鍵・家の dir・path・email を伏せる）。
fn model_error(alias: &str, e: LlmError) -> LlmError {
    let detail = match &e {
        LlmError::Malformed { detail } if detail != UNEXPECTED => detail.clone(),
        LlmError::Rejected { detail, .. } => detail.clone(),
        _ => return e,
    };
    let name = crate::domain::chat::MODELS.iter().find(|(a, _)| *a == alias).map_or(alias, |(_, label)| *label);
    eprintln!("shibubu: llm-cli: --model {alias}: {}", mask_for_log(&detail));
    let line = if model_refused(&detail) { format!("{name} isn't available on this account") } else { format!("Couldn't run with {name}") };
    no_cli(&format!("{line} · {MODEL_STEP}"))
}

/// G84: log に書く CLI の文（鍵の形の語 = `storage::redact`・家の dir・path・email = `runs::mask_personal`・120 文字で切る）。
/// Settings › About の `Copy diagnostics` の行（`Generated::diagnostics`）と同じ 2 段の伏せ。
fn mask_for_log(s: &str) -> String {
    short(&crate::ui::runs::mask_personal(&crate::storage::redact::redact(s)))
}

/// G84: CLI の失敗の文が「その model は無い・使えない」と言っているか。読む形 = Claude Code 2.1.312 の実行 file の中の文（2026-10-29 に G の pane が読んだ・起こしていない）:
/// `There's an issue with the selected model (…). It may not exist or you may not have access to it.`・`…, or ask your admin to enable this model.`（error = `model_not_found`）と、
/// API の `not_found_error` の `model: …`。それ以外は読めない失敗（言い過ぎない）。
// ponytail: 語の表。本物の拒否の答え（`-p --output-format json` の result）は未測。測ったら表をその文に寄せる
fn model_refused(detail: &str) -> bool {
    let low = detail.to_lowercase();
    low.contains("selected model")
        || (low.contains("model")
            && ["not_found_error", "model_not_found", "enable this model", "may not exist", "not have access", "does not exist", "doesn't exist", "not available", "isn't available", "not supported"]
                .iter()
                .any(|w| low.contains(w)))
}

pub fn parse_result(stdout: &str) -> Result<Value, LlmError> {
    parse_answer(stdout, false)
}

/// `parse_result` の本体（`key` = 鍵の account の呼び出し・`CliLlm::call` が `config.account` から渡す）。
/// 文脈の長さ（413・400 の「prompt is too long」等・`llm::too_long`）は plan も鍵も `Rejected(TOO_LONG)` = 短くするか新しい chat（「limit」の語で上限・Console に落とさない）。
/// 鍵の失敗は API の status と型で分ける（`llm::status_error` = HTTP の道と同じ表: 401 / 403 → `Auth`・402 / billing_error / 400 の残高と使用量の上限 → `Quota`・
/// 429 → `RateLimited`・5xx → `Server`・他 → `Rejected`）。CLI の文に API の誤りの JSON（`{"type":"error","error":{..}}`）が在れば型と code をそこから読む。
/// status の無い鍵の失敗は接続の語 → `Network`・他 → `Malformed`（plan の上限と login の 1 行を出さない）。
fn parse_answer(stdout: &str, key: bool) -> Result<Value, LlmError> {
    let bad = |d: &str| LlmError::Malformed { detail: d.into() };
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        if offline(stdout) {
            LlmError::Network { detail: short(stdout.trim()) }
        } else {
            bad(UNEXPECTED)
        }
    })?;
    if !v.is_object() {
        return Err(bad(UNEXPECTED));
    }
    // 107 通目: Grok の答えは `text`（schema の JSON の文字列）で、claude の `type` / `result` / `structured_output` は無い。
    if v.get("text").and_then(Value::as_str).is_some()
        && v.get("type").is_none()
        && v.get("is_error").is_none()
        && v.get("structured_output").is_none()
        && v.get("result").is_none()
    {
        let text = v["text"].as_str().unwrap_or("");
        return match serde_json::from_str::<Value>(text.trim()) {
            Ok(o) if o.is_object() => Ok(o),
            _ => Err(bad("Grok's answer was in an unexpected form. Try again")),
        };
    }
    let text = v["result"].as_str().unwrap_or("");
    let failed = v["is_error"] == true || v["subtype"].as_str().is_some_and(|s| s != "success");
    if failed {
        let detail = short(if text.is_empty() {
            v["subtype"].as_str().unwrap_or("")
        } else {
            text
        });
        let status = v["api_error_status"].as_u64().map(|s| s as u16);
        if crate::llm::too_long(status.unwrap_or(400), "", text) {
            return Err(LlmError::Rejected { status: status.unwrap_or(400), detail: crate::llm::TOO_LONG.into() });
        }
        if key {
            let body = text
                .find('{')
                .and_then(|i| serde_json::from_str::<Value>(&text[i..]).ok())
                .filter(|b| b["error"].is_object())
                .unwrap_or_else(|| serde_json::json!({ "error": { "message": text } }));
            return Err(match status {
                Some(s) => crate::llm::status_error(s, &body, None),
                None if offline(&detail) => LlmError::Network { detail },
                None => LlmError::Malformed { detail },
            });
        }
        let low = detail.to_lowercase();
        // 混雑の 429 は文に「rate limit」を含んでも RateLimited。plan の使用量の上限（「resets <時刻>」・429 で来る事もある）は時間で戻る PlanLimit。
        // CliLlm は plan の login だけを使う（`parse_auth_status`）ので、Console の請求の上限（`Quota`）にはしない（反証の直し）
        return Err(if status == Some(429) && !low.contains("reset") {
            LlmError::RateLimited { retry_after: None }
        } else if low.contains("limit") {
            LlmError::PlanLimit { detail }
        } else if let Some(s @ 500..=599) = status {
            LlmError::Server { status: s }
        } else if let Some(401 | 403) = status {
            // login が切れた 401 / 403 は、OAuth や bearer が model を名指しても Sign in。鍵の account は上の `if key` が先。
            // 選んだ model の拒否（selected model）は Sign in にしない。model を選んだ呼び出しが isn't available と言う（g84）。
            if low.contains("selected model") {
                LlmError::Malformed { detail: detail.clone() }
            } else {
                no_cli(SIGNED_OUT)
            }
        } else if model_refused(&detail) {
            // status の無い model の拒否（`selected model` / `may not exist`）は sign in にしない
            LlmError::Malformed { detail }
        } else if offline(&detail) {
            LlmError::Network { detail }
        } else {
            LlmError::Malformed { detail }
        });
    }
    if v["stop_reason"] == "refusal" {
        return Err(LlmError::Refused {
            detail: short(text),
        });
    }
    if let Some(o) = v.get("structured_output").filter(|o| o.is_object()) {
        return Ok(o.clone());
    }
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(o) if o.is_object() => Ok(o),
        _ => Err(bad(UNEXPECTED)),
    }
}

// ── RQ2: AI の account（RQ-ACCT）・API key（RQ-API）・plan の使用量（RQ-USAGE）。Add account / Re-authenticate は G99b の `sign_in` に
// `child_env` の差分（`CLAUDE_CONFIG_DIR`）を渡して呼ぶ（RQ-N）。Orca の account の型（account ごとの設定の dir・鍵の変数を外す）を利用者の目線で選び直した。

/// RQ2（RQ-ACCT）: plan の account ごとの Claude Code の設定の置き場 = `<data_dir>/ai-accounts/<id>/`（子の `CLAUDE_CONFIG_DIR`）。
/// 保証する: 利用者の `~/.claude` と `~/.claude.json` に書かない・読まない（System default だけがそれを使う）。
pub const ACCOUNTS_DIR: &str = "ai-accounts";

/// RQ2（RQ-API）: 鍵の Keychain の名前 = `ai-key.<AiAccount::Key の id>`（service = `secret_store::KEYCHAIN_SERVICE`）。設定の file・DB・log には末尾 4 文字（`last4`）だけ。
pub const KEY_ENTRY_PREFIX: &str = "ai-key.";

/// RQ2（RQ-USAGE）: plan の使用量を読む先（Orca の claude-oauth-usage-request と同じ・model を呼ばない・quota を使わない）。
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// RQ2: 子（`claude -p`・Add account の sign in）の env の差分 = (変数, Some(値) = 置く / None = 外す)。`CliLlm::send` と `command_line` の `env -u` がこれを使う。
/// 例: `let env = llm_cli::child_env(cfg.account.as_ref(), &data_dir, &SecretStore::from_env())?;`
/// 保証する: None（System default）と `Plan` は `SCRUBBED_ENV` を全部外す（`Plan` は加えて `CLAUDE_CODE_OAUTH_TOKEN` も外し〈在ると選んだ dir でなく env の token で認証する〉、
/// `CLAUDE_CONFIG_DIR = <data_dir>/ai-accounts/<id>` を置く）・
/// `Key` は Keychain の鍵を `ANTHROPIC_API_KEY` に置き、他の鍵の変数を外す・`EnvKey` は `ANTHROPIC_API_KEY` を外さず他を外す。
/// Keychain に鍵が無い・読めない → `LlmError::NoCli { "This API key is missing · Paste API key again" }`（鍵の値・path を detail に入れない）。保証しない: 鍵が有効な事（呼んだ時の `Auth`）。
/// 利用者が Claude Code で断った鍵（`~/.claude.json` の `customApiKeyResponses.rejected`・YC_DECISIONS 89「断った鍵は使わない」）は渡さない:
/// `EnvKey` = System default と同じに外す（Claude Code と同じく login を使う・Settings の行が理由を言う）・`Key` = `DECLINED_KEY`（Shibubu に貼った鍵・黙って plan に替えない）。
/// Wave: RQ2
pub fn child_env(
    account: Option<&crate::model::AiAccount>,
    data_dir: &Path,
    keys: &crate::storage::secret_store::SecretStore,
) -> Result<Vec<(&'static str, Option<String>)>, LlmError> {
    child_env_with(account, data_dir, keys, &|v| std::env::var(v).ok())
}

/// `Key` の鍵を利用者が Claude Code で断っていた時の 1 行（Settings で別の鍵か System default に替える）。
pub const DECLINED_KEY: &str = "This key was declined in Claude Code · Paste a different key or use Claude Code's sign-in";

/// `child_env` の本体（`var` = 親の env の読み口・試験は HOME と鍵を偽の値で渡す = process の env を書き換えない）。
fn child_env_with(
    account: Option<&crate::model::AiAccount>,
    data_dir: &Path,
    keys: &crate::storage::secret_store::SecretStore,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(&'static str, Option<String>)>, LlmError> {
    use crate::model::AiAccount;
    let declined = |key: &str| rejected_in(var("HOME").as_deref().map(Path::new), key);
    // 鍵の変数を外す（`keep` = 選んだ鍵の account が渡す 1 つ）。`CLAUDE_CODE_OAUTH_TOKEN` は在ると選んだ物でなく env の token で認証する
    let strip = |keep: &str| -> Vec<(&'static str, Option<String>)> {
        SCRUBBED_ENV.iter().filter(|v| **v != keep).map(|v| (*v, None)).collect()
    };
    Ok(match account {
        None => strip(""),
        // ACCOUNTS-1101: 足した CLI の account は Claude の dir も鍵も渡さない（置き場は `prepare_other_env` / Grok の auth）
        Some(a @ AiAccount::Plan { .. }) if a.provider() != "claude" => strip(""),
        Some(AiAccount::Plan { id }) => {
            let mut env = strip("");
            env.push(("CLAUDE_CODE_OAUTH_TOKEN", None));
            // REVIEW-RQN 4: Claude Code（2.1.312 の実行 file の文）は Keychain の名前を CLAUDE_SECURESTORAGE_CONFIG_DIR が在ればそれから作る（CLAUDE_CONFIG_DIR より先・
            // 空なら利用者の主の項目）→ 親の env に在ると、その account の sign in・logout が別の項目を指す
            env.push(("CLAUDE_SECURESTORAGE_CONFIG_DIR", None));
            env.push(("CLAUDE_CONFIG_DIR", Some(data_dir.join(ACCOUNTS_DIR).join(id).display().to_string())));
            env
        }
        Some(AiAccount::Key { id, .. }) => {
            let missing = || no_cli("This API key is missing · Paste API key again");
            let key = keys.get(&format!("{KEY_ENTRY_PREFIX}{id}")).map_err(|_| missing())?.ok_or_else(missing)?;
            if declined(&key) {
                return Err(no_cli(DECLINED_KEY));
            }
            let mut env = strip("ANTHROPIC_API_KEY");
            env.push(("CLAUDE_CODE_OAUTH_TOKEN", None));
            env.push(("ANTHROPIC_API_KEY", Some(key)));
            env
        }
        Some(AiAccount::EnvKey) if var("ANTHROPIC_API_KEY").is_some_and(|k| declined(&k)) => strip(""),
        Some(AiAccount::EnvKey) => {
            let mut env = strip("ANTHROPIC_API_KEY");
            env.push(("CLAUDE_CODE_OAUTH_TOKEN", None));
            env
        }
        // 見つかった CLI と local LLM には鍵も Claude の dir も渡さない。
        Some(AiAccount::Found { .. }) => strip(""),
    })
}

/// RQ2（RQ-ACCT）: `Add account`。`<data_dir>/ai-accounts/plan-<n>/`（0700）を作り、そこを `CLAUDE_CONFIG_DIR` にした `child_env` の差分で `sign_in` を 1 回呼ぶ
/// （browser が開く・終わるまで待つので UI の thread から呼ばない）。`sign_in` = 製品は `llm_cli::sign_in`・試験は偽の CLI の `sign_in_from`（署名を変えた理由: 本体を通す試験が
/// 本物の `claude auth login` を起こさない）。
/// 例: `let acct = llm_cli::add_plan_account(&data_dir, &settings.ai_accounts, llm_cli::sign_in)?; settings.ai_accounts.push(acct);`（Active にするのは利用者の `Use`）
/// 保証する: id は `existing` とも disk に残った dir とも、消した account の id（`ai-accounts/last-plan`）とも重ならない `plan-<n>`・data dir は `CliConfig::new` と同じ絶対 path（確かめと呼び出しが同じ login を見る）・
/// sign in が済まなければ dir を消して Err（作りかけの account を残さない）・email / token を返さない。
/// Err = 画面の 1 行（`SIGN_IN_FAILED` など・`next_step` が Sign in を返す物）。保証しない: 利用者が browser で sign in を終える事（5 分で打ち切る・`SIGN_IN_WAIT`）。
/// Wave: RQ2
pub fn add_plan_account(
    data_dir: &Path,
    existing: &[crate::model::AiAccount],
    sign_in: impl FnOnce(&[(&str, Option<String>)]) -> Result<(), String>,
) -> Result<crate::model::AiAccount, String> {
    use crate::model::AiAccount;
    use std::os::unix::fs::DirBuilderExt;
    let data_dir = CliConfig::new(data_dir).work_dir;
    let root = data_dir.join(ACCOUNTS_DIR);
    // 落ちた Add の dir（列に無い）も消した account の id も使い回さない（Claude Code は login を dir の文字列から作る Keychain の名前に置くので、同じ dir の
    // 新しい account が前の login で動く）。REVIEW-RQN 4: 使った一番大きい n を `ai-accounts/last-plan` に残し、列・disk の dir と合わせた最大の次にする
    let n_of = |id: &str| id.strip_prefix("plan-").and_then(|n| n.parse::<u64>().ok());
    let last = last_plan(&root);
    let on_disk: Vec<u64> = fs::read_dir(&root).into_iter().flatten().flatten().filter_map(|e| n_of(e.file_name().to_str().unwrap_or(""))).collect();
    let n = existing.iter().filter_map(|a| n_of(a.id())).chain(on_disk).fold(last, u64::max) + 1;
    let id = format!("plan-{n}");
    let dir = root.join(&id);
    let room = |e: std::io::Error| format!("Couldn't make room for another account ({}) · Try again", e.kind());
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(room)?;
    if let Err(e) = fs::write(root.join(LAST_PLAN), n.to_string()) {
        let _ = fs::remove_dir_all(&dir);
        return Err(room(e));
    }
    let account = AiAccount::Plan { id };
    let signed = child_env(Some(&account), &data_dir, &crate::storage::secret_store::SecretStore::memory())
        .map_err(|e| e.to_string())
        .and_then(|env| sign_in(&env));
    match signed {
        Ok(()) => Ok(account),
        Err(why) => {
            let _ = fs::remove_dir_all(&dir);
            Err(why)
        }
    }
}

/// RQ2（RQ-ACCT）: `Re-authenticate`（Settings の plan の account の行）と Home の `Sign in`（使っている account・None = System default）。
/// その account の `child_env` の差分で `sign_in` を呼び直す（`Plan` = その dir・None = 利用者の Claude Code の login）。鍵の account は `Paste API key` で置き直す。
/// 例: `llm_cli::reauthenticate(&data_dir, Some(&acct), llm_cli::sign_in)?`。署名を変えた理由: Home の Sign in も使っている account の dir に login する
/// （前は System default に login し、plan の account の確かめは同じ「isn't signed in」に戻った）・試験が偽の CLI を渡す。
/// 保証する: 他の account と System default に触らない・失敗しても dir・列・使っている account を変えない（前の login は CLI の Keychain の項目のまま）・
/// data dir は `CliConfig::new` と同じ絶対 path。Err = 画面の 1 行（`Key` / `EnvKey` は `Paste API key instead`・dir の無い plan は理由と次の一手）。
/// 保証しない: CLI が取り消された login の時に前の Keychain の項目を残す事（未測）。
/// Wave: RQ2
pub fn reauthenticate(
    data_dir: &Path,
    account: Option<&crate::model::AiAccount>,
    sign_in: impl FnOnce(&[(&str, Option<String>)]) -> Result<(), String>,
) -> Result<(), String> {
    use crate::model::AiAccount;
    let data_dir = CliConfig::new(data_dir).work_dir;
    match account {
        Some(AiAccount::Found { .. }) => return Err("Already signed in on this Mac".into()),
        // ACCOUNTS-1101: 足した CLI の account は下の terminal で sign in（Connections の行の Sign in）
        Some(a @ AiAccount::Plan { .. }) if a.provider() != "claude" => return Err("Sign in from its row in Connections".into()),
        Some(AiAccount::Key { .. } | AiAccount::EnvKey) => return Err("Paste API key instead".into()),
        Some(AiAccount::Plan { id }) if plan_dir(&data_dir, id).is_none_or(|d| !d.is_dir()) => {
            return Err("This account's sign-in is gone · Remove it and add it again".into())
        }
        _ => {}
    }
    let env = child_env(account, &data_dir, &crate::storage::secret_store::SecretStore::memory()).map_err(|e| e.to_string())?;
    sign_in(&env)
}

/// REVIEW-RQN 4: 使った一番大きい plan の n（`ai-accounts/` の中・消した id を使い回さない）。Add と Remove（REVIEW-RQN 6）が書く。
const LAST_PLAN: &str = "last-plan";

/// `ai-accounts/last-plan` の n（無い・読めない = 0）
fn last_plan(root: &Path) -> u64 {
    fs::read_to_string(root.join(LAST_PLAN)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// REVIEW-RQN（operator 4）: plan の account の `Remove` の前に、その dir の login を CLI に消させる（`claude auth logout`・`env` = `child_env` の差分）。
/// `remove_account` の引数として渡す（製品 = これ・試験 = 偽の CLI の `sign_out_from`・app.rs の試験の窓は何もしない口）。
/// Claude Code 2.1.312 の実行 file の文で確かめた（起こしていない・login していない・2026-10-29）: logout が消す Keychain の項目は `Claude Code-credentials` に
/// `CLAUDE_CONFIG_DIR` の sha256 の頭 8 文字を足した名前（`CLAUDE_SECURESTORAGE_CONFIG_DIR` が在ればそちら・`child_env` が外す）= その account の項目だけ。
/// 例: `llm_cli::remove_account(&data_dir, &acct, &keys, llm_cli::sign_out)?`
/// 保証する: その account の `CLAUDE_CONFIG_DIR`（ai-accounts の下）が無い env・`CLAUDE_SECURESTORAGE_CONFIG_DIR` を外していない env では CLI を起こさない
/// （利用者の主の login を消さない）。保証しない: 後の版の CLI が同じ名前の作り方を続ける事。
pub fn sign_out(env: &[(&str, Option<String>)]) -> Result<(), String> {
    sign_out_from(std::env::var_os(CLI_ENV), env)
}

/// `sign_out` の本体（`cli` = `CLI_ENV` の値・試験は偽の CLI）。
fn sign_out_from(cli: Option<OsString>, env: &[(&str, Option<String>)]) -> Result<(), String> {
    let own = env.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && v.as_deref().is_some_and(|d| Path::new(d).parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(ACCOUNTS_DIR))));
    if !own || !env.contains(&("CLAUDE_SECURESTORAGE_CONFIG_DIR", None)) {
        return Err("Not an added account's sign-in".into());
    }
    let program = cli_program(cli).map_err(|e| e.to_string())?;
    match run_quick(&program, &["auth", "logout"], env)? {
        (Some(0), _) => Ok(()),
        (code, _) => Err(format!("claude auth logout ended with {code:?}")),
    }
}

/// `<data_dir>/ai-accounts/<id>`（id は `plan-<数字>` だけ・`..`・`/` で ai-accounts の外を指させない・他は None）。
fn plan_dir(data_dir: &Path, id: &str) -> Option<PathBuf> {
    let n = id.strip_prefix("plan-")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| data_dir.join(ACCOUNTS_DIR).join(id))
}

/// RQ-ACCT: 足した plan の account の行に出す email（Claude Code が sign in の後に `<dir>/.claude.json` の `oauthAccount.emailAddress` に置く物）。
/// 画面だけに使う（設定・記録・usage report に書かない）。None = まだ・読めない（行は `plan-<n>` で名乗る）。保証しない: Claude Code がこの名前で置き続ける事。
/// Wave: RQ2
pub fn account_email(data_dir: &Path, id: &str) -> Option<String> {
    let text = fs::read_to_string(plan_dir(data_dir, id)?.join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v["oauthAccount"]["emailAddress"].as_str().map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned)
}

/// RQ2（RQ-ACCT・RQ-API）: `Remove`。`Plan` = その dir の login を `sign_out` で CLI に消させてから `<data_dir>/ai-accounts/<id>/` を消す・
/// `Key` = Keychain の `ai-key.<id>` を消す・`EnvKey` = 何もしない（利用者の env は消さない）。
/// 例: `llm_cli::remove_account(&data_dir, &acct, &keys, llm_cli::sign_out)?; settings.ai_accounts.retain(|a| a.id() != acct.id());`
/// 署名を変えた理由（REVIEW-RQN 4）: dir を消しても Claude Code の Keychain の項目（その dir の login）が残った。`sign_out` は `sign_in` と同じく差し込む口
/// （製品 = `llm_cli::sign_out`・試験 = 偽の CLI・app.rs の試験の窓は何もしない口 = 試験から本物の `claude auth logout` を起こさない）。
/// 保証する: `ai-accounts` の外（利用者の `~/.claude`・他の Keychain の項目）を消さない・無い物を消すのは Ok・dir が在る時だけ `sign_out` を 1 回
/// （済まなくても dir は消す・id は使い回さない = 消す n を `ai-accounts/last-plan` に残してから消す〈REVIEW-RQN 6・残せなければ Err で消さない〉）。保証しない: Anthropic 側の login の取り消し（Shibubu の外）。
/// Wave: RQ2
pub fn remove_account(
    data_dir: &Path,
    account: &crate::model::AiAccount,
    keys: &crate::storage::secret_store::SecretStore,
    sign_out: impl FnOnce(&[(&str, Option<String>)]) -> Result<(), String>,
) -> Result<(), String> {
    use crate::model::AiAccount;
    match account {
        // ACCOUNTS-1101: 足した CLI の account = その置き場ごと消す（login はその中の file・番号は `last-<provider>` に残っている = 使い回さない）。
        // 置き場を言えない（形の違う id）account は消さない（利用者の物を指さない）
        AiAccount::Plan { .. } if account.provider() != "claude" => {
            let Some(dir) = cli_account_dir(data_dir, account) else {
                return Err("This account couldn't be removed · Try again".into());
            };
            match fs::remove_dir_all(dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("This account couldn't be removed ({}) · Try again", e.kind())),
                _ => Ok(()),
            }
        }
        AiAccount::Plan { id } => {
            // data dir は `add_plan_account` と同じ絶対 path（logout が sign in と同じ Keychain の名前を指す）
            let data_dir = CliConfig::new(data_dir).work_dir;
            let Some(dir) = plan_dir(&data_dir, id) else {
                return Err("This account couldn't be removed · Try again".into());
            };
            // REVIEW-RQN 6: 消す n を last-plan に残してから消す（last-plan の前から在った account を消しても、次の Add がその n に戻らない = 同じ dir の
            // 古い login で新しい account が動かない）。残せなければ消さない
            let root = data_dir.join(ACCOUNTS_DIR);
            let n = id["plan-".len()..].parse::<u64>().unwrap_or(0);
            if root.is_dir() && n > last_plan(&root) {
                fs::write(root.join(LAST_PLAN), n.to_string()).map_err(|e| format!("This account couldn't be removed ({}) · Try again", e.kind()))?;
            }
            if dir.is_dir() {
                let out = child_env(Some(account), &data_dir, &crate::storage::secret_store::SecretStore::memory())
                    .map_err(|e| e.to_string())
                    .and_then(|env| sign_out(&env));
                if let Err(why) = out {
                    eprintln!("shibubu: llm-cli: remove: sign out didn't finish: {why}");
                }
            }
            match fs::remove_dir_all(dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("This account couldn't be removed ({}) · Try again", e.kind()))
                }
                _ => Ok(()),
            }
        }
        AiAccount::Key { id, .. } => keys
            .delete(&format!("{KEY_ENTRY_PREFIX}{id}"))
            .map(|_| ())
            .map_err(|_| "The key couldn't be removed from Keychain · Try again".into()),
        AiAccount::EnvKey => Ok(()),
        // 利用者の CLI の login も local LLM も消さない。
        AiAccount::Found { .. } => Ok(()),
    }
}

// ── ACCOUNTS-1101（owner 203「providerごとにadd accountで複数アカウントを追加できるように。orcaを参考に。」）: Claude 以外の CLI にも Add account。
// Orca の Accounts（System default ＋足した account・Add / Use / Remove）を、Shibubu の今の形（account ごとの置き場を子に渡す = 共有の login を書き替えない）で。

/// ACCOUNTS-1101: account ごとに login の置き場を分けられる CLI の、置き場の変数（sign in の terminal と呼び出しの両方がここを読む 1 か所）。
/// grok = `GROK_HOME` と `GROK_AUTH_PATH`（どちらも `<dir>/.grok` の下 = grok がどちらを読んでも同じ file）・kimi = `KIMI_CODE_HOME` = `<dir>`・
/// opencode = `XDG_DATA_HOME` = `<dir>/data`（`opencode auth login` の置き場）。None = 分けられない（cursor-agent は login を Keychain に置く・
/// claude は CLAUDE_CONFIG_DIR の別の道 `child_env`・local LLM は login が無い）。HOME を差し替える裏技はしない。
/// 保証しない: 各 CLI の login がこの置き場に書く事（未測・この pane は CLI を打てない）。
/// Wave: ACCOUNTS-1101
pub fn account_env(provider: &str, dir: &Path) -> Option<Vec<(&'static str, String)>> {
    let at = |p: PathBuf| p.display().to_string();
    match provider {
        "grok" => Some(vec![("GROK_HOME", at(dir.join(".grok"))), ("GROK_AUTH_PATH", at(grok_account_auth(dir)))]),
        "kimi" => Some(vec![("KIMI_CODE_HOME", at(dir.to_path_buf()))]),
        "opencode" => Some(vec![("XDG_DATA_HOME", at(dir.join("data")))]),
        _ => None,
    }
}

/// ACCOUNTS-1101: 足した Grok の account の login の file（sign in の `GROK_AUTH_PATH` と呼び出しの `GROK_AUTH_PATH` が同じ path）。
fn grok_account_auth(dir: &Path) -> PathBuf {
    dir.join(".grok").join("auth.json")
}

/// ACCOUNTS-1101: 呼び出しの account が足した CLI の account なら、その置き場（Grok の auth・kimi の home・opencode の data を差し替える）。
fn login_dir(config: &CliConfig) -> Option<PathBuf> {
    config.account.as_ref().and_then(|a| cli_account_dir(&config.work_dir, a))
}

/// ACCOUNTS-1101: `+ Add account` を出さない AI の理由の 1 行（Connections と Settings の provider の見出しの下）。None = 足せる（claude と `account_env` が Some の CLI）か
/// login の無い local LLM（何も言わない）。例: `one_account_line("cursor-agent")` → `Some("Cursor Agent keeps one sign-in on this Mac · Shibubu uses that one")`。
/// Wave: ACCOUNTS-1101
pub fn one_account_line(provider: &str) -> Option<String> {
    (FOUND_CLIS.iter().any(|c| c.0 == provider) && account_env(provider, Path::new("")).is_none())
        .then(|| format!("{} keeps one sign-in on this Mac · Shibubu uses that one", found_name(provider)))
}

/// ACCOUNTS-1101: CLI の account を 1 つ足す = `<data dir>/ai-accounts/<provider>-<n>/`（0700）を作り `AiAccount::Plan { id: "<provider>-<n>" }` を返す
/// （列に足して sign in の terminal を開くのは app.rs・選ぶのは sign in の後の確かめ）。n は列・disk の dir・`ai-accounts/last-<provider>` のどれとも重ならない次
/// （消した番号を使い回さない = その dir の古い login で新しい account が動かない）。data dir は `CliConfig::new` と同じ絶対 path（sign in と呼び出しが同じ置き場を見る）。
/// Err = 画面の 1 行（分けられない CLI は `one_account_line`・dir を作れない）。例: `let acct = llm_cli::add_cli_account(&data_dir, "grok", &settings.ai_accounts)?;`
/// Wave: ACCOUNTS-1101
pub fn add_cli_account(data_dir: &Path, provider: &str, existing: &[crate::model::AiAccount]) -> Result<crate::model::AiAccount, String> {
    use std::os::unix::fs::DirBuilderExt;
    if account_env(provider, Path::new("")).is_none() {
        return Err(one_account_line(provider).unwrap_or_else(|| format!("{} can't add another account", found_name(provider))));
    }
    let root = CliConfig::new(data_dir).work_dir.join(ACCOUNTS_DIR);
    let prefix = format!("{provider}-");
    let n_of = |id: &str| id.strip_prefix(prefix.as_str()).and_then(|n| n.parse::<u64>().ok());
    let last_file = root.join(format!("last-{provider}"));
    let last = fs::read_to_string(&last_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let on_disk: Vec<u64> = fs::read_dir(&root).into_iter().flatten().flatten().filter_map(|e| n_of(e.file_name().to_str().unwrap_or(""))).collect();
    let n = existing.iter().filter_map(|a| n_of(a.id())).chain(on_disk).fold(last, u64::max) + 1;
    let id = format!("{provider}-{n}");
    let dir = root.join(&id);
    let room = |e: std::io::Error| format!("Couldn't make room for another account ({}) · Try again", e.kind());
    // 置き場の変数が指す dir も先に作る（Grok の `.grok`・opencode の `data`）= CLI が無い dir に書けずに落ちない
    let inner = match provider {
        "grok" => dir.join(".grok"),
        "opencode" => dir.join("data"),
        _ => dir.clone(),
    };
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&inner).map_err(room)?;
    if let Err(e) = fs::write(&last_file, n.to_string()) {
        let _ = fs::remove_dir_all(&dir);
        return Err(room(e));
    }
    Ok(crate::model::AiAccount::Plan { id })
}

/// ACCOUNTS-1101: 足した CLI の account の置き場 `<work dir>/ai-accounts/<provider>-<数字>`（`account_env` が Some の provider だけ）。Claude の plan・他の種類は None。
/// Wave: ACCOUNTS-1101
pub fn cli_account_dir(data_dir: &Path, account: &crate::model::AiAccount) -> Option<PathBuf> {
    let crate::model::AiAccount::Plan { id } = account else { return None };
    // provider() は `<名前>-<数字>` の形だけ名前を返し、account_env は表の名前だけ Some = id に `/`・`..` が入った物は通らない
    account_env(account.provider(), Path::new(""))?;
    Some(CliConfig::new(data_dir).work_dir.join(ACCOUNTS_DIR).join(id))
}

/// ACCOUNTS-1101: 足した CLI の account に login が在るか（file を見るだけ・CLI を起こさない）。grok = `.grok/auth.json`・kimi = `credentials/` に 1 つ以上・
/// opencode = `data/opencode/auth.json`。None = 足した CLI の account でない（Found・Claude）。Connections の行の `Needs sign-in` と `Use` の行き先を決める。
/// Wave: ACCOUNTS-1101
pub fn cli_account_signed_in(data_dir: &Path, account: &crate::model::AiAccount) -> Option<bool> {
    let dir = cli_account_dir(data_dir, account)?;
    Some(match account.provider() {
        "grok" => grok_account_auth(&dir).is_file(),
        "kimi" => fs::read_dir(dir.join("credentials")).is_ok_and(|mut d| d.next().is_some()),
        "opencode" => dir.join("data").join("opencode").join("auth.json").is_file(),
        _ => false,
    })
}

/// ACCOUNTS-1101: sign in の terminal で打つ 1 行（`PanelAct::SignIn` の command・Project の panel と Home の Terminal〈`generated::HOME_PLACE`〉）。Found = 表の command（`grok login`）・
/// 足した CLI の account = `env GROK_HOME='…' GROK_AUTH_PATH='…' grok login`（置き場 = `account_env`・`sign_in_command` が同じ字を作り直して確かめる）。
/// None = sign in の command を知らない（Claude の plan は browser の sign in = `add_plan_account`）。
/// Wave: ACCOUNTS-1101
pub fn account_sign_in_line(data_dir: &Path, account: &crate::model::AiAccount) -> Option<String> {
    let provider = account.provider();
    let &(_, _, table) = FOUND_CLIS.iter().find(|c| c.0 == provider)?;
    match account {
        crate::model::AiAccount::Found { .. } => Some(table.to_owned()),
        crate::model::AiAccount::Plan { .. } => Some(format!("{}{table}", env_prefix(provider, &cli_account_dir(data_dir, account)?)?)),
        _ => None,
    }
}

/// ACCOUNTS-1101 2 段目（作る物 5・owner 203）: 足した account（`Plan`）の名前 = 利用者が付けた名前（`UiSettings::account_name`）→ sign in で分かった email の @ の前。
/// email そのものは返さない（画面に出さない）。どちらも無い・`Plan` でない = None（呼ぶ側は `numbered_name`）。
/// 例: `account_name(&Plan { id: "plan-1".into() }, None, Some("ryo@example.com"))` → `Some("ryo")`
/// Wave: ACCOUNTS-1101
pub fn account_name(account: &crate::model::AiAccount, given: Option<&str>, email: Option<&str>) -> Option<String> {
    if !matches!(account, crate::model::AiAccount::Plan { .. }) {
        return None;
    }
    let local = email.and_then(|e| e.split('@').next());
    given.into_iter().chain(local).map(str::trim).find(|n| !n.is_empty()).map(str::to_owned)
}

/// ACCOUNTS-1101 2 段目: 名前の無い足した account の名前 = `<AI の名前> <n + 1>`（System default が 1 つ目）。`Plan` でない = None。
/// 例: `numbered_name(&Plan { id: "grok-1".into() })` → `Some("Grok 2")`・`Plan { id: "plan-1" }` → `Some("Claude 2")`
/// Wave: ACCOUNTS-1101
pub fn numbered_name(account: &crate::model::AiAccount) -> Option<String> {
    let crate::model::AiAccount::Plan { id } = account else { return None };
    let ai = found_name(account.provider());
    // 形の違う id（手で書いた設定の `env` など）は id のまま添える
    Some(match id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()) {
        Some(n) => format!("{ai} {}", n + 1),
        None => format!("{ai} · {id}"),
    })
}

/// RQ2（RQ-API）: `Paste API key`（clipboard の文字列を受ける・入力欄は作らない）。前後の空白を落とし、`sk-ant-` で始まらない・短すぎる物は Err。
/// Keychain の `ai-key.<id>` に置き、`AiAccount::Key { id: "key-<n>", last4 }` を返す。
/// 例: `let acct = llm_cli::add_key_account(&clip, &keys, &settings.ai_accounts)?;`
/// 保証する: 鍵の値を返さない・log / 設定の file / Err の文に出さない（Err は `That doesn't look like an Anthropic API key · Copy it again`）・model を呼ばない（有効かは使った時に分かる）。
/// Wave: RQ2
pub fn add_key_account(
    key: &str,
    keys: &crate::storage::secret_store::SecretStore,
    existing: &[crate::model::AiAccount],
) -> Result<crate::model::AiAccount, String> {
    use crate::model::AiAccount;
    let key = key.trim();
    if !key.starts_with("sk-ant-") || key.len() < 24 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("That doesn't look like an Anthropic API key · Copy it again".into());
    }
    let id = (1..)
        .map(|n| format!("key-{n}"))
        .find(|id| !existing.iter().any(|a| a.id() == id))
        .expect("key-<n> は尽きない");
    keys.put(&format!("{KEY_ENTRY_PREFIX}{id}"), key)
        .map_err(|_| "The key couldn't be saved to Keychain · Try again".to_string())?;
    Ok(AiAccount::Key { id, last4: last4(key) })
}

/// 鍵の末尾 4 文字（画面と設定に置いてよいのはこれだけ）。
fn last4(key: &str) -> String {
    key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect()
}

/// 利用者が Claude Code で「この鍵は使わない」と答えた鍵か（`~/.claude.json` の `customApiKeyResponses.rejected` = 鍵の末尾 20 文字の列・Orca と同じ読み）。読むだけ。
fn rejected_by_claude_code(key: &str) -> bool {
    rejected_in(std::env::var_os("HOME").as_deref().map(Path::new), key)
}

/// `rejected_by_claude_code` の本体（`home` = 利用者の home・試験は temp の dir）。
fn rejected_in(home: Option<&Path>, key: &str) -> bool {
    let key = key.trim();
    let Some(home) = home else { return false };
    let Ok(text) = fs::read_to_string(home.join(".claude.json")) else { return false };
    let tail: String = key.chars().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
    serde_json::from_str::<Value>(&text).ok().and_then(|v| {
        v["customApiKeyResponses"]["rejected"].as_array().map(|r| r.iter().any(|x| x.as_str() == Some(&tail)))
    }) == Some(true)
}

/// RQ2（RQ-API）: `Use key from environment` の行に出す末尾 4 文字（Shibubu を起こした env の `ANTHROPIC_API_KEY`）。None = env に無い（行を出さない）。
/// 保証する: 4 文字より多く返さない。
/// Wave: RQ2
pub fn env_key_last4() -> Option<String> {
    let key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
    (!rejected_by_claude_code(&key)).then(|| last4(key.trim()))
}

/// RQ2（RQ-USAGE）: plan の login の OAuth の token を読む（`config_dir` = その account の `CLAUDE_CONFIG_DIR`・None = System default）。Claude Code が置いた Keychain の項目から。
/// 例: `let t = llm_cli::oauth_token(None)?; let u = llm_cli::read_plan_usage(USAGE_URL, &t, Duration::from_secs(10));`
/// 保証する: token を返すだけで保存しない・log / Err に出さない。未 login = `NoCli { NOT_SIGNED_IN }`・Keychain を読めない = `NoCli { llm::KEYCHAIN_UNREADABLE }`。
/// 保証しない: macOS が Keychain の確認を出さない事（未測）。
/// Wave: RQ2
pub fn oauth_token(config_dir: Option<&Path>) -> Result<String, LlmError> {
    let dirs: Vec<PathBuf> = match config_dir {
        None => vec![],
        // Claude Code は realpath でも名前を作る（/var → /private/var・Orca の claudeConfigDirKeychainAliases）
        Some(d) => {
            let mut v = vec![d.to_path_buf()];
            v.extend(fs::canonicalize(d).ok().filter(|c| c != d));
            v
        }
    };
    let services: Vec<String> = if dirs.is_empty() {
        vec![claude_keychain_service(None)]
    } else {
        dirs.iter().map(|d| claude_keychain_service(Some(d))).collect()
    };
    let found: Vec<Result<Option<String>, ()>> = services.iter().map(|s| crate::storage::secret_store::read_generic_password(s).map_err(|_| ())).collect();
    // 古い Claude Code は設定の dir の `.credentials.json` に置く
    let file = match config_dir {
        Some(d) => Some(d.join(".credentials.json")),
        None => std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude/.credentials.json")),
    };
    token_from(found, file.and_then(|f| fs::read_to_string(f).ok()))
}

/// `oauth_token` の判定（純関数）: Keychain の項目を順に見て token の在る最初の物・無ければ `.credentials.json` の中身・どちらも無ければ
/// Keychain の読みが 1 つでも失敗していたら `llm::KEYCHAIN_UNREADABLE`、それ以外は `NOT_SIGNED_IN`。
fn token_from(keychain: Vec<Result<Option<String>, ()>>, file: Option<String>) -> Result<String, LlmError> {
    let mut unreadable = false;
    for item in keychain {
        match item {
            Ok(Some(json)) => {
                if let Some(t) = oauth_token_of(&json) {
                    return Ok(t);
                }
            }
            Ok(None) => {}
            Err(()) => unreadable = true,
        }
    }
    match file.as_deref().and_then(oauth_token_of) {
        Some(t) => Ok(t),
        None if unreadable => Err(no_cli(crate::llm::KEYCHAIN_UNREADABLE)),
        None => Err(no_cli(NOT_SIGNED_IN)),
    }
}

/// Claude Code が login を置く Keychain の service 名: 既定 = `Claude Code-credentials`・設定の dir つき = その後ろに `-<sha256(dir) の先頭 8 桁>`（Claude Code 2.1+・Orca の getActiveClaudeService）。
/// ponytail: NFC の正規化はしない（`ai-accounts/plan-<n>` と data dir は Shibubu が作る ASCII の path・利用者の dir に NFD の文字が在れば読めない〈未測〉）
fn claude_keychain_service(config_dir: Option<&Path>) -> String {
    match config_dir {
        None => "Claude Code-credentials".into(),
        Some(d) => format!("Claude Code-credentials-{}", &crate::crypto::sha256_hex(d.display().to_string().as_bytes())[..8]),
    }
}

/// Keychain の項目（JSON）から token: `{"claudeAiOauth":{"accessToken":"…"}}`。
fn oauth_token_of(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().filter(|t| !t.is_empty()).map(str::to_owned)
}

/// RQ2（RQ-USAGE）: `url`（製品は `USAGE_URL`・試験は local の偽の server）へ GET（`ureq`・`timeout` で打ち切る）し、答えを `PlanUsage` にする（読めた窓だけ・`checked_at` = 今）。
/// 失敗の種類: 401/403 = `NoCli { SIGNED_OUT }`（login が切れた・断られた・G97 と同じ 1 行で次の一手は `next_step`）・429 = `RateLimited`・5xx = `Server`・繋がらない = `Network`・時間切れ = `Timeout`・形が違う = `Malformed`。
/// 保証する: model を呼ばない・token を Err に出さない。保証しない: Anthropic の答えの形が変わらない事。
/// Wave: RQ2
pub fn read_plan_usage(url: &str, token: &str, timeout: Duration) -> Result<crate::llm::PlanUsage, LlmError> {
    let headers = [("Authorization", format!("Bearer {token}")), ("anthropic-beta", "oauth-2025-04-20".into()), ("User-Agent", "claude-code/2.1.0".into())];
    let v = get_usage_json(url, &headers.iter().map(|(k, v)| (*k, v.as_str())).collect::<Vec<_>>(), SIGNED_OUT, timeout)?;
    parse_usage(&v, (now_ms() / 1000) as i64)
}

/// 使用量の口へ GET（`ureq`・`timeout` で打ち切る・redirect しない）→ JSON（`read_plan_usage` と `cli_usage` の共通）。401/403 = `NoCli { refused }`・429 = `RateLimited`・
/// 5xx = `Server`・繋がらない = `Network`・時間切れ = `Timeout`・JSON でない = `Malformed`。token を Err に出さない。
fn get_usage_json(url: &str, headers: &[(&str, &str)], refused: &str, timeout: Duration) -> Result<Value, LlmError> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).redirects(0).build();
    match headers.iter().fold(agent.get(url), |req, (k, v)| req.set(k, v)).call() {
        Ok(r) => r.into_json().map_err(|e| {
            if timed_out(&e) { LlmError::Timeout { ms: timeout.as_millis() as u64 } } else { LlmError::Malformed { detail: "The usage answer wasn't JSON".into() } }
        }),
        Err(ureq::Error::Status(401 | 403, _)) => Err(no_cli(refused)),
        Err(ureq::Error::Status(429, r)) => Err(LlmError::RateLimited { retry_after: r.header("retry-after").and_then(|s| s.trim().parse().ok()) }),
        Err(ureq::Error::Status(status, _)) if status >= 500 => Err(LlmError::Server { status }),
        Err(ureq::Error::Status(status, _)) => Err(LlmError::Rejected { status, detail: "The usage request was refused".into() }),
        Err(ureq::Error::Transport(t)) => {
            if timed_out(&t) {
                Err(LlmError::Timeout { ms: timeout.as_millis() as u64 })
            } else {
                Err(LlmError::Network { detail: format!("{:?}", t.kind()) })
            }
        }
    }
}

/// STATUSBAR-1101 追記 2（operator 21:5x・owner 210 の 1 の残り）: 下の帯で Claude の plan のほかに使用量を読む AI（`AiAccount::provider()`）。
/// Orca（/Applications/Orca.app の app.asar・out/main/index.js）が % を出している AI のうち、Shibubu の AI の行に在り、login が file に在る物（Keychain の許可の窓を出さない）。
/// 入れない物: codex（Shibubu の AI の行に無い = `FOUND_CLIS`）・gemini / antigravity（Orca も `--`）・opencode（Orca は opencode.ai の cookie を貼らせる）・cursor-agent（Orca も読まない）。
/// Wave: STATUSBAR-1101
pub const USAGE_CLIS: [&str; 2] = ["grok", "kimi"];

/// STATUSBAR-1101 追記 2: `USAGE_CLIS` の AI の login（file）で、その AI の使用量の口を読む（model を呼ばない・`read_plan_usage` と同じ失敗の種類と `PlanUsage`）。
/// `account_dir` = Shibubu が足した account の置き場（`cli_account_dir`）・None = この Mac の既定の login（grok = `$GROK_HOME` か `~/.grok` の `auth.json`・
/// kimi = `$KIMI_CODE_HOME` か `~/.kimi-code` の `credentials/kimi-code.json`）。
/// 口（Orca と同じ形・code は写していない）: grok = `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`（`creditUsagePercent` = 週・無ければ `monthlyLimit` と `used` = 月）・
/// kimi = `GET https://api.kimi.com/coding/v1/usages`（`usage` = 週・`limits[]` の 5 時間に近い窓 = Session）。
/// 例（`Generated::refresh_usage` の裏の thread）: `llm_cli::cli_usage("grok", None, Duration::from_secs(10))` → `Ok(PlanUsage { windows: [("Weekly", 30, Some(..))], .. })`。
/// 保証する: token を Err・log に出さない・file を書かない・CLI を起こさない（期限の切れた login は直さず「Terminal で 1 回開く」の 1 行）。
/// 未 login = `NoCli { "<名前> isn't signed in · run <cmd> in Terminal" }`・`USAGE_CLIS` の外 = `NoCli { "Shibubu can't read <名前>'s usage yet" }`。
/// 保証しない: 両社の答えの形が変わらない事（2026-11-01 の Orca の読みと同じ形）。
/// Wave: STATUSBAR-1101
pub fn cli_usage(provider: &str, account_dir: Option<&Path>, timeout: Duration) -> Result<crate::llm::PlanUsage, LlmError> {
    let home = |var: &str, dot: &str| {
        std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(dot))
    };
    let (file, base) = match provider {
        "grok" => (account_dir.map_or_else(|| home("GROK_HOME", ".grok").join("auth.json"), grok_account_auth), "https://cli-chat-proxy.grok.com/v1"),
        "kimi" => (account_dir.map_or_else(|| home("KIMI_CODE_HOME", ".kimi-code"), Path::to_path_buf).join("credentials").join("kimi-code.json"), "https://api.kimi.com/coding/v1"),
        _ => return Err(no_cli(&format!("Shibubu can't read {}'s usage yet", found_name(provider)))),
    };
    let now = (now_ms() / 1000) as i64;
    let (token, user) = cli_login(provider, fs::read_to_string(file).ok().as_deref(), now)?;
    read_cli_usage(provider, base, &token, user.as_deref(), now, timeout)
}

/// `cli_usage` の login の読み（純関数・`text` = login の file の中身）→ (token, Grok の user id)。未 login・期限切れ（Grok は 5 分前・Kimi Code は 5 秒前 = Orca と同じ）は `NoCli` の 1 行。
/// Grok の file = `{ "<発行元>": { key, user_id, expires_at } }`・`https://auth.x.ai`（`::` の後ろ付きも）の項目を先に・Kimi Code = `{ access_token, expires_at（unix 秒） }`。
fn cli_login(provider: &str, text: Option<&str>, now: i64) -> Result<(String, Option<String>), LlmError> {
    let (name, cmd) = FOUND_CLIS.iter().find(|c| c.0 == provider).map_or((provider, provider), |c| (c.1, c.2));
    let signed_out = || no_cli(&format!("{name} isn't signed in · run {cmd} in Terminal"));
    let expired = || no_cli(&format!("{name}'s sign-in expired · open {provider} in Terminal once"));
    let v: Value = text.and_then(|t| serde_json::from_str(t).ok()).ok_or_else(signed_out)?;
    if provider == "kimi" {
        let token = v["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(signed_out)?;
        return match v["expires_at"].as_f64() {
            Some(at) if at as i64 - now > 5 => Ok((token.to_owned(), None)),
            _ => Err(expired()),
        };
    }
    // (x.ai の項目か, 期限の内か, key, user id) = x.ai の期限の内の物が在ればそれ・無ければ x.ai の期限切れ（= 切れたと言う）か他の発行元の最初の物
    let mut pick: Option<(bool, bool, String, Option<String>)> = None;
    for (issuer, e) in v.as_object().into_iter().flatten() {
        let Some(key) = e["key"].as_str().filter(|k| !k.is_empty()) else { continue };
        let xai = issuer == "https://auth.x.ai" || issuer.starts_with("https://auth.x.ai::");
        // 読めない期限は期限の内と数える（Orca と同じ）
        let fresh = e["expires_at"].as_str().and_then(|at| crate::ui::runs::unix_secs(at.trim())).is_none_or(|at| at - now > 300);
        let user = e["user_id"].as_str().map(str::to_owned);
        if xai && fresh {
            return Ok((key.to_owned(), user));
        }
        if pick.as_ref().is_none_or(|p| xai && !p.0) {
            pick = Some((xai, fresh, key.to_owned(), user));
        }
    }
    match pick {
        Some((_, true, key, user)) => Ok((key, user)),
        Some(_) => Err(expired()),
        None => Err(signed_out()),
    }
}

/// `cli_usage` の口を読む（`base` = 製品は Grok / Kimi Code の URL・試験は local の偽の server）。Grok は `?format=credits` に % が無ければ `/billing` をもう 1 回（Orca と同じ）。
fn read_cli_usage(provider: &str, base: &str, token: &str, user: Option<&str>, now: i64, timeout: Duration) -> Result<crate::llm::PlanUsage, LlmError> {
    let (name, cmd) = FOUND_CLIS.iter().find(|c| c.0 == provider).map_or((provider, provider), |c| (c.1, c.2));
    let refused = format!("{name} refused the sign-in · run {cmd} in Terminal");
    let bearer = format!("Bearer {token}");
    let get = |path: &str| {
        let mut headers = vec![("Authorization", bearer.as_str()), ("Accept", "application/json")];
        if provider == "grok" {
            headers.push(("X-XAI-Token-Auth", "xai-grok-cli"));
            headers.extend(user.map(|u| ("x-userid", u)));
        }
        get_usage_json(&format!("{base}{path}"), &headers, &refused, timeout)
    };
    let windows = match provider {
        "grok" => match cli_windows(provider, &get("/billing?format=credits")?) {
            w if w.is_empty() => cli_windows(provider, &get("/billing")?),
            w => w,
        },
        _ => cli_windows(provider, &get("/usages")?),
    };
    if windows.is_empty() {
        return Err(no_cli(&format!("{name} didn't report a usage percentage for this account")));
    }
    Ok(crate::llm::PlanUsage { windows, checked_at: now })
}

/// `cli_usage` の答え → 窓（純関数・読めた窓だけ）。Grok = `config`（無ければ答えそのもの）の `creditUsagePercent` = Weekly・無ければ `used.val / monthlyLimit.val` = Monthly
/// （戻る = `currentPeriod.end` か `billingPeriodEnd`）。Kimi Code = `limits[]` の `window`（duration ＋ timeUnit）が 5 時間に一番近い `detail` = Session・`usage` = Weekly
/// （`used` か `limit − remaining`・戻る = `resetTime` か `resetAt`）。数は number でも文字でもよい。
fn cli_windows(provider: &str, v: &Value) -> Vec<(String, u8, Option<i64>)> {
    let num = |x: &Value| x.as_f64().or_else(|| x.as_str()?.trim().parse().ok()).filter(|n: &f64| n.is_finite());
    let pct = |p: f64| p.round().clamp(0.0, 100.0) as u8;
    let either = |a: &Value, b: &'static str, c: &Value| -> Option<i64> { reset_secs(if a.is_null() { &c[b] } else { a }) };
    if provider == "grok" {
        let c = if v["config"].is_object() { &v["config"] } else { v };
        let at = either(&c["currentPeriod"]["end"], "billingPeriodEnd", c);
        if let Some(p) = num(&c["creditUsagePercent"]) {
            return vec![("Weekly".into(), pct(p), at)];
        }
        return match (num(&c["monthlyLimit"]["val"]), num(&c["used"]["val"])) {
            (Some(limit), Some(used)) if limit > 0.0 => vec![("Monthly".into(), pct(used / limit * 100.0), at)],
            _ => Vec::new(),
        };
    }
    let window = |d: &Value| -> Option<(u8, Option<i64>)> {
        let limit = num(&d["limit"]).filter(|l| *l > 0.0)?;
        let used = num(&d["used"]).or_else(|| Some(limit - num(&d["remaining"])?))?;
        Some((pct(used / limit * 100.0), either(&d["resetTime"], "resetAt", d)))
    };
    let minutes = |w: &Value| -> Option<f64> {
        let n = num(&w["duration"])?;
        let unit = w["timeUnit"].as_str().unwrap_or_default().to_ascii_uppercase();
        Some(if unit.contains("MINUTE") { n } else if unit.contains("HOUR") { n * 60.0 } else if unit.contains("DAY") { n * 1440.0 } else if unit.contains("SECOND") { (n / 60.0).round() } else { n })
    };
    let session = v["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| Some(((minutes(&l["window"]).unwrap_or(300.0) - 300.0).abs(), window(&l["detail"])?)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    session.map(|(_, (p, at))| ("Session".to_owned(), p, at)).into_iter().chain(window(&v["usage"]).map(|(p, at)| ("Weekly".to_owned(), p, at))).collect()
}

/// 時間切れか（ureq の transport の io の種類・本文の読みの io）。
fn timed_out(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(e);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                return true;
            }
        }
        cur = e.source();
    }
    false
}

/// 使用量の答え（`five_hour`・`seven_day`・model 別の週）を `PlanUsage` に。読めた窓だけ・1 つも無ければ `Malformed`。
/// 窓 = `{utilization | used_percentage: 0〜100, resets_at: ISO 8601 か unix 秒 / ms}`・model 別の週 = `limits[]` の `kind: "weekly_scoped"`
/// （`percent`・`scope.model.display_name`）か古い名前（`fable_weekly` 等）。Orca の claude-oauth-usage-request / claude-usage-window と同じ読み。
fn parse_usage(v: &Value, now: i64) -> Result<crate::llm::PlanUsage, LlmError> {
    let window = |pct: &Value, at: &Value| -> Option<(u8, Option<i64>)> {
        let pct = pct.as_f64().filter(|p| p.is_finite())?;
        Some((pct.round().clamp(0.0, 100.0) as u8, reset_secs(at)))
    };
    let plain = |w: &Value| window(w.get("utilization").filter(|p| p.is_number()).or_else(|| w.get("used_percentage"))?, &w["resets_at"]);
    let mut windows = Vec::new();
    for (name, key) in [("Session", "five_hour"), ("Weekly", "seven_day")] {
        if let Some((p, at)) = v.get(key).and_then(plain) {
            windows.push((name.to_owned(), p, at));
        }
    }
    let scoped: Vec<(String, u8, Option<i64>)> = v["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["kind"] == "weekly_scoped")
        .filter_map(|l| {
            let model = l["scope"]["model"]["display_name"].as_str()?.trim();
            let (p, at) = window(&l["percent"], &l["resets_at"])?;
            (!model.is_empty()).then(|| (format!("Weekly · {model}"), p, at))
        })
        .collect();
    if !scoped.iter().any(|(n, ..)| n.eq_ignore_ascii_case("Weekly · Fable")) {
        if let Some((p, at)) = ["fable_weekly", "fable_seven_day", "seven_day_fable"].iter().find_map(|k| v.get(*k).and_then(plain)) {
            windows.push(("Weekly · Fable".into(), p, at));
        }
    }
    windows.extend(scoped);
    if windows.is_empty() {
        return Err(LlmError::Malformed { detail: "The usage answer had no limits in it".into() });
    }
    Ok(crate::llm::PlanUsage { windows, checked_at: now })
}

/// 戻る時刻: ISO 8601（`+00:00` も）か数（1e10 より大きければ ms）→ unix 秒。
fn reset_secs(v: &Value) -> Option<i64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => match s.trim().parse::<f64>() {
            Ok(n) => n,
            Err(_) => return crate::ui::runs::unix_secs(s.trim()),
        },
        _ => return None,
    };
    n.is_finite().then(|| if n > 1e10 { (n / 1000.0) as i64 } else { n as i64 })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// STATUSBAR-1101 追記 2: Grok と Kimi Code の login と答えを Orca と同じ形で読む（2026-11-01 の app.asar の読み・偽の値）。
    /// 負の対照（commit message）: `cli_login` の x.ai を先にする条件を外す → 他の発行元の key を取って赤・`cli_windows` の Kimi の 5 時間に近い窓を `limits[0]` に → 1 日の窓を取って赤。
    #[test]
    fn statusbar_210_2_grok_and_kimi_logins_and_answers_read_like_orca() {
        let now = 1_800_000_000;
        // Grok: x.ai の期限の内の項目・他の発行元より先・期限切れは「切れた」・無ければ「sign in していない」
        let grok = r#"{"https://other.example":{"key":"o-key"},"https://auth.x.ai::cli":{"key":"x-key","user_id":"u1","expires_at":"2027-02-01T00:00:00Z"}}"#;
        assert_eq!(cli_login("grok", Some(grok), now).unwrap(), ("x-key".to_owned(), Some("u1".to_owned())));
        let stale = r#"{"https://auth.x.ai":{"key":"x-key","expires_at":"2020-01-01T00:00:00Z"}}"#;
        assert!(matches!(cli_login("grok", Some(stale), now), Err(LlmError::NoCli { detail }) if detail == "Grok's sign-in expired · open grok in Terminal once"));
        assert!(matches!(cli_login("grok", None, now), Err(LlmError::NoCli { detail }) if detail == "Grok isn't signed in · run grok login in Terminal"));
        // Kimi Code: access_token ＋ expires_at（unix 秒）
        assert_eq!(cli_login("kimi", Some(r#"{"access_token":"k-tok","expires_at":1800000600}"#), now).unwrap().0, "k-tok");
        assert!(matches!(cli_login("kimi", Some(r#"{"access_token":"k-tok","expires_at":1800000003}"#), now), Err(LlmError::NoCli { detail }) if detail.contains("expired")));
        // Grok の答え: creditUsagePercent = 週・無ければ月の used / monthlyLimit
        let at = crate::ui::runs::unix_secs("2026-11-08T00:00:00Z");
        assert_eq!(cli_windows("grok", &serde_json::json!({"config":{"creditUsagePercent":30.4,"currentPeriod":{"end":"2026-11-08T00:00:00Z"}}})), vec![("Weekly".to_owned(), 30u8, at)]);
        assert_eq!(cli_windows("grok", &serde_json::json!({"monthlyLimit":{"val":"200"},"used":{"val":50},"billingPeriodEnd":"2026-11-08T00:00:00Z"})), vec![("Monthly".to_owned(), 25u8, at)]);
        assert!(cli_windows("grok", &serde_json::json!({"subscriptionTier":"SuperGrok"})).is_empty());
        // Kimi Code の答え: 5 時間に近い窓 = Session（1 日の窓より先）・usage = Weekly（remaining から）
        let kimi = serde_json::json!({
            "usage": {"limit": "100", "remaining": "80", "resetTime": "2026-11-08T00:00:00Z"},
            "limits": [
                {"window": {"duration": 1, "timeUnit": "TIME_UNIT_DAY"}, "detail": {"limit": 10, "used": 9}},
                {"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"}, "detail": {"limit": 10, "used": 3}}
            ]
        });
        assert_eq!(cli_windows("kimi", &kimi), vec![("Session".to_owned(), 30u8, None), ("Weekly".to_owned(), 20u8, at)]);
        assert!(matches!(cli_usage("gemini", None, Duration::from_secs(1)), Err(LlmError::NoCli { detail }) if detail.contains("can't read")));
    }
    use crate::composer::project::{edit_schema, plan_schema, EDIT_NAME, PLAN_NAME};
    use crate::llm::fake::Fake;
    use crate::llm::{Budget, Budgeted};
    use serde_json::json;

    /// Plan と編集の schema は両 provider の部分集合に収まり（送る前の検査を通る）、schema の形の答えは検証を通る。
    /// 負の対照: schema に minItems を足すと `LlmError::Schema` で赤（送る前に止まる＝quota を使わない）。
    #[test]
    fn w13_plan_and_edit_schemas_pass_the_request_check() {
        let plan = json!({ "shape": "project", "name": "議事録", "pages_requested": true, "seats": [
            { "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null }
        ]});
        let edit = json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] });
        // AI-COMPOSE-1101: 部品を書く形（anyOf の 2 つ目）も送る前の検査と答えの検証を通る
        let part = json!({ "steps": [{ "verb": "add_part", "name": "Guitar", "page": "Dev", "html": "<select></select>", "on": [{ "act": "take.recorded", "key": "guitar", "intent": "Use {value}" }] }] });
        for (name, schema, answer) in [
            (PLAN_NAME, plan_schema(), plan),
            (EDIT_NAME, edit_schema(), edit),
            (EDIT_NAME, edit_schema(), part),
        ] {
            let fake = Fake::new(vec![Ok(answer.clone())]);
            assert_eq!(
                fake.complete_json(JsonRequest::new(name, "", "", schema.clone())),
                Ok(answer),
                "{name}"
            );
            let mut bad = schema.clone();
            bad["properties"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .find(|v| v["type"] == "array")
                .unwrap()["minItems"] = json!(1);
            let fake = Fake::new(vec![]);
            assert!(
                matches!(
                    fake.complete_json(JsonRequest::new(name, "", "", bad)),
                    Err(LlmError::Schema { .. })
                ),
                "{name}"
            );
        }
    }

    /// 打つ command は鍵の変数を外し、`--bare` を付けず・model を選んでいなければ `--model` も付けず（G84）、tool を許さず、一言を argv に入れない。
    #[test]
    fn w13_command_line_uses_the_plan_login_and_no_tools() {
        let req = JsonRequest::new(
            PLAN_NAME,
            "あなたは Shibubu の planner",
            "一言 n-13-secret",
            plan_schema(),
        );
        let none = child_env(None, Path::new("/nonexistent"), &crate::storage::secret_store::SecretStore::memory()).unwrap();
        let argv = command_line(Path::new("/opt/x/claude"), &req, &none, Path::new("/tmp/prompt.txt"), None);
        assert_eq!(argv[0], "/usr/bin/env");
        for var in SCRUBBED_ENV {
            assert!(
                argv.windows(2).any(|w| w == ["-u", var]),
                "{var} を外す: {argv:?}"
            );
        }
        let at = argv
            .iter()
            .position(|a| a == "/opt/x/claude")
            .expect("program");
        assert_eq!(argv[at + 1], "-p");
        assert!(
            !argv.iter().any(|a| a == "--bare" || a == "--model"),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|w| w == ["--tools", ""]),
            "tool 0: {argv:?}"
        );
        assert!(argv.windows(2).any(|w| w[0] == "--json-schema"
            && serde_json::from_str::<Value>(&w[1]).ok() == Some(plan_schema())));
        assert!(argv
            .windows(2)
            .any(|w| w == ["--system-prompt", "あなたは Shibubu の planner"]));
        assert!(
            !argv.iter().any(|a| a.contains("n-13-secret")),
            "一言は stdin で渡す"
        );
        assert!(
            !argv.iter().any(|a| a == "--prompt-file" || a == "todo_write"),
            "claude の argv は変えない: {argv:?}"
        );
    }

    /// 107 通目: file 名が grok の argv は実測の形。一言は argv に無い。file と effort が在ってもこの形のまま。
    #[test]
    fn w13_grok_command_line_is_the_measured_shape() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["seen"],
            "properties": { "seen": { "type": "string" } }
        });
        let mut req = JsonRequest::new("seen", "Reply with the schema only.", "一言 n-107-secret", schema);
        req.files = vec![PathBuf::from("/tmp/a.txt")];
        req.effort = Some("max".into());
        let work = Path::new("/var/shibubu-work");
        let (home, gh) = grok_dirs(work);
        let auth = Path::new("/var/grok-auth.json");
        let prompt = Path::new("/var/prompt.txt");
        let argv = command_line(
            Path::new("/opt/x/grok"),
            &req,
            &[],
            prompt,
            Some((home.as_path(), gh.as_path(), auth)),
        );
        assert!(argv.windows(2).any(|w| w == ["-u", "XAI_API_KEY"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-u", "GROK_AUTH"]), "{argv:?}");
        assert!(argv.iter().any(|a| a == "HOME=/var/shibubu-work/grok-home"), "{argv:?}");
        assert!(argv.iter().any(|a| a == "GROK_HOME=/var/shibubu-work/grok-home/.grok"), "{argv:?}");
        assert!(argv.iter().any(|a| a == "GROK_AUTH_PATH=/var/grok-auth.json"), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--prompt-file", "/var/prompt.txt"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--tools", "todo_write"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--system-prompt-override", "Reply with the schema only."]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--max-turns", "1"]), "{argv:?}");
        assert!(!argv.iter().any(|a| a == "--system-prompt" || a == "-p" || a == "--bare" || a == "--model" || a == "--effort" || a == "--restricted"), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--reasoning-effort", "max"]), "PROVIDER-1101: Grok の effort は --reasoning-effort: {argv:?}");
        assert!(!argv.iter().any(|a| a.contains("n-107-secret")), "一言は --prompt-file: {argv:?}");
        let cap = input_token_cap(&req.instructions, &req.schema.to_string(), &req.input);
        assert!(5_227 <= cap && 21_658 > cap, "実測の 5227 は収まり 21658 は超える: {cap}");
    }

    /// Grok の stdout は `text` の中の JSON。`usage` が無くても答えは読める。text が JSON でなくても token 数は残る。
    #[test]
    fn w13_grok_parse_reads_text_and_keeps_usage_when_the_text_is_not_json() {
        let raw = r#"{"text":"{\"a\":1}","stopReason":"end_turn","usage":{"input_tokens":5227}}"#;
        assert_eq!(parse_result(raw).unwrap(), json!({ "a": 1 }));
        assert_eq!(input_tokens_of(raw), Some(5227));
        let missing = r#"{"text":"{\"a\":1}","stopReason":"end_turn"}"#;
        assert!(parse_result(missing).is_ok());
        assert_eq!(input_tokens_of(missing), None);
        let bad = r#"{"text":"not-json","stopReason":"end_turn","usage":{"input_tokens":9}}"#;
        assert!(matches!(parse_result(bad), Err(LlmError::Malformed { .. })));
        assert_eq!(input_tokens_of(bad), Some(9));
        let claude = r#"{"type":"result","subtype":"success","is_error":false,"result":"x","structured_output":{"a":1},"usage":{"input_tokens":40}}"#;
        assert_eq!(parse_result(claude).unwrap(), json!({ "a": 1 }));
        assert_eq!(input_tokens_of(claude), Some(40));
        // RULE-043: claude の prompt の cache（本文が cache の欄に入り input_tokens は 2 = 実測 10-31）は 3 欄の和で数える
        let cached = r#"{"type":"result","structured_output":{"a":1},"usage":{"input_tokens":2,"cache_creation_input_tokens":1200,"cache_read_input_tokens":4000,"output_tokens":9}}"#;
        assert_eq!(input_tokens_of(cached), Some(5202), "cache の欄を数えない（混ざりの上限が効かない）");
        assert_eq!(input_tokens_of(r#"{"usage":{"cache_read_input_tokens":7}}"#), Some(7));
        assert_eq!(input_tokens_of(r#"{"usage":{"output_tokens":7}}"#), None, "入力の欄が 1 つも無い");
        // USAGE-OWN-1101: 出力・cache の内訳・費用・session も読む（Grok の形は費用と session が無い）
        let m = meter_of(r#"{"type":"result","result":"x","usage":{"input_tokens":2,"cache_creation_input_tokens":1200,"cache_read_input_tokens":4000,"output_tokens":9},"total_cost_usd":0.0123,"session_id":"s-1"}"#).expect("JSON");
        assert_eq!(m, Meter { input: Some(5202), output: Some(9), cache_read: Some(4000), cache_creation: Some(1200), cost_micros: Some(12_300), session: Some("s-1".into()) });
        let g = meter_of(raw).expect("JSON");
        assert_eq!((g.input, g.output, g.cost_micros, g.session), (Some(5227), None, None, None));
        assert_eq!(meter_of("not json"), None);
        assert!(matches!(parse_grok_login("You are logged in with grok.com.\n"), Ok(())));
        match parse_grok_login("Not logged in\n") {
            Err(LlmError::NoCli { detail }) => assert!(detail.contains("signed in"), "{detail}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn w13_only_a_plan_login_is_usable() {
        assert_eq!(
            parse_auth_status(
                r#"{"loggedIn":true,"authMethod":"claude.ai","email":"x@example.org"}"#
            ),
            Ok(())
        );
        for bad in [
            r#"{"loggedIn":false}"#,
            r#"{"loggedIn":true,"authMethod":"api_key"}"#,
            "not json",
        ] {
            match parse_auth_status(bad) {
                Err(LlmError::NoCli { detail }) => assert!(
                    !detail.contains("example.org"),
                    "email を出さない: {detail}"
                ),
                other => panic!("{bad}: {other:?}"),
            }
        }
    }

    /// 試験の作業 dir（試験ごとに別）。
    fn tmp(what: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("w13-cli-{what}-{}", unique()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 偽の CLI（sh の script）。`-p` の時は自分の pid（FX2: 止めた後に消えた事を読む）・argv・stdin・env を隣の file に写し、stdin に SLEEP があれば答えずに 30 秒寝て、決めた答えを返す。
    fn fake_cli(dir: &Path, auth: &str, reply: &str) -> PathBuf {
        let p = dir.join("claude");
        fs::write(
            &p,
            format!(
                "#!/bin/sh\nhere=$(dirname \"$0\")\ncase \"$1\" in\n--version) echo '9.9.1 (Claude Code)' ;;\nauth) echo '{auth}' ;;\n-p) echo $$ > \"$here/pid.txt\"; printf '%s\\n' \"$@\" > \"$here/argv.txt\"; cat > \"$here/stdin.txt\"; env > \"$here/env.txt\"\n  if grep -q SLEEP \"$here/stdin.txt\"; then sleep 30; fi\n  echo '{reply}' ;;\nesac\n"
            ),
        )
        .unwrap();
        Command::new("/bin/chmod")
            .arg("+x")
            .arg(&p)
            .status()
            .unwrap();
        p
    }

    const PLAN: &str = r#"{"loggedIn":true,"authMethod":"claude.ai","email":"x@example.org"}"#;

    fn cli(dir: &Path, auth: &str, reply: &str, timeout: Duration) -> Result<CliLlm, LlmError> {
        let program = fake_cli(dir, auth, reply);
        let cfg = CliConfig {
            timeout,
            ..CliConfig::new(dir)
        };
        detect_from(Some(program.into_os_string()), cfg)
    }

    pub(crate) fn records(dir: &Path) -> Vec<CallRecord> {
        fs::read_to_string(dir.join(CALL_LOG))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("1 行 1 個の CallRecord"))
            .collect()
    }

    /// W14（YCM-067）: 上限 1 回の `Budgeted` で包んだ CLI（偽の CLI・quota を使わない）は、2 回目の生成と編集を CLI を起こさずに止める。
    /// 数えるのは試験の側の `CallRecord` の行（CLI を起こした回数）= 1 行だけ。
    /// 負の対照: `Budgeted::send` の回数の確かめを外すと、2 回目が CLI に届いて行が 2 になり赤。
    #[test]
    fn w14_budget_of_one_call_starts_the_cli_once() {
        let d = tmp("budget");
        let plan = r#"{"shape":"project","name":"議事録","pages_requested":false,"seats":[]}"#;
        let reply = format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"result":"x","structured_output":{plan}}}"#
        );
        let llm = Budgeted::new(
            Box::new(cli(&d, PLAN, &reply, Duration::from_secs(60)).unwrap()),
            Budget {
                max_calls: 1,
                max_total: Duration::from_secs(600),
            },
        );
        llm.complete_json(JsonRequest::new(PLAN_NAME, "", "一言", plan_schema()))
            .expect("1 回目は CLI が答える");
        for (name, schema) in [(PLAN_NAME, plan_schema()), (EDIT_NAME, edit_schema())] {
            match llm.complete_json(JsonRequest::new(name, "", "2 回目", schema)) {
                Err(LlmError::OverBudget { detail }) => assert_eq!(detail, "1 call"),
                other => panic!("{name}: 上限で止まる: {other:?}"),
            }
        }
        let recs = records(&d);
        assert_eq!(recs.len(), 1, "CLI を起こしたのは 1 回だけ: {recs:?}");
        assert_eq!(recs[0].outcome, "ok");
        let _ = fs::remove_dir_all(&d);
    }

    /// FX2（FX-005・caller の形・本体の前は todo!() で赤）: `Stop` の印で偽の CLI（SLEEP = 30 秒寝る）を子ごと止め、`Cancelled` と記録の 1 行
    /// （`outcome` = cancelled・`stopped_after_ms` = 返った after_ms）。子の pid が消えた事は `kill -0` で読む。
    /// 負の対照（本体で取る）: 印を見ずに今の `wait_exit(&key, timeout)` のまま待つ → 30 秒寝た後に `{}` が返り、時間と Cancelled の assert が赤。
    #[test]
    fn fx2_cancel_ends_the_child_and_records_the_wait() {
        let d = tmp("cancel");
        let llm = std::sync::Arc::new(cli(&d, PLAN, "{}", Duration::from_secs(60)).unwrap());
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (l, c) = (llm.clone(), cancel.clone());
        let t = Instant::now();
        let call = thread::spawn(move || l.send_cancellable(&JsonRequest::new(PLAN_NAME, "", "SLEEP", plan_schema()), &c));
        let pid_file = d.join("pid.txt");
        while !pid_file.exists() {
            assert!(t.elapsed() < Duration::from_secs(20), "偽の CLI が起きない");
            thread::sleep(Duration::from_millis(20));
        }
        let pid = fs::read_to_string(&pid_file).unwrap().trim().to_owned();
        cancel.store(true, Ordering::SeqCst);
        let got = call.join().unwrap();
        assert!(t.elapsed() < Duration::from_secs(20), "30 秒寝る子を待たずに返る: {:?}", t.elapsed());
        let Err(LlmError::Cancelled { after_ms }) = got else { panic!("Cancelled で返る: {got:?}") };
        let alive = Command::new("/bin/kill").args(["-0", pid.as_str()]).stderr(Stdio::null()).status().unwrap().success();
        assert!(!alive, "止めた後に子（偽の CLI の pid {pid}）が残った");
        let recs = records(&d);
        assert_eq!(recs.len(), 1, "止めた呼び出しも 1 行: {recs:?}");
        assert_eq!((recs[0].outcome.as_str(), recs[0].stopped_after_ms), ("cancelled", Some(after_ms)));
        let _ = fs::remove_dir_all(&d);
    }

    /// 在りかの 3 つの道: 空 = 使わない・相対 path = 使わない・絶対 path = その CLI（`--version` と `auth status` は model を呼ばない）。
    /// plan でない login は使えない。detail に path・email を出さない。
    #[test]
    fn w13_detect_takes_the_env_path_and_only_a_plan_login() {
        let d = tmp("detect");
        let cfg = || CliConfig::new(&d);
        for (env, want) in [
            (OsString::new(), "off for this launch"),
            (OsString::from("claude"), "set path"),
        ] {
            match detect_from(Some(env), cfg()) {
                Err(LlmError::NoCli { detail }) => assert!(detail.contains(want), "{detail}"),
                other => panic!("{:?}", other.map(|c| c.label())),
            }
        }
        let ok = cli(&d, PLAN, "{}", Duration::from_secs(5)).expect("plan の login の偽の CLI");
        assert_eq!(
            (ok.version(), ok.label().as_str()),
            ("9.9.1", "claude-cli/9.9.1")
        );
        assert!(ok.program().is_absolute());
        let other = tmp("detect-key");
        match cli(
            &other,
            r#"{"loggedIn":true,"authMethod":"console","email":"x@example.org"}"#,
            "{}",
            Duration::from_secs(5),
        ) {
            Err(LlmError::NoCli { detail }) => {
                assert!(
                    detail.contains("plan")
                        && !detail.contains("example.org")
                        && !detail.contains(&other.display().to_string()),
                    "{detail}"
                )
            }
            r => panic!("{:?}", r.map(|c| c.label())),
        }
        let _ = (fs::remove_dir_all(&d), fs::remove_dir_all(&other));
    }

    /// 1 回の呼び出し: 一言は stdin だけで渡り（argv に無い）、答えは `structured_output` から読み、schema の検証を通る。
    /// 親の Claude Code の session の変数と鍵の変数は子に渡らない。1 回ごとに CallRecord を 1 行（中身は書かない）・作業 dir は消える。
    #[test]
    fn w13_send_passes_the_prompt_on_stdin_and_logs_each_call() {
        let d = tmp("send");
        let plan = r#"{"shape":"project","name":"議事録","pages_requested":false,"seats":[{"title":"Notes","kind":"app","app_name":"Notes Maker","bundle_id":null,"file":null,"url":null,"install_url":null,"instead":[],"page":0,"slot":"main","why":"Where you take the notes","feeds":null}]}"#;
        let reply = format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"result":"x","structured_output":{plan}}}"#
        );
        let llm = cli(&d, PLAN, &reply, Duration::from_secs(60)).unwrap();
        let got = llm
            .complete_json(JsonRequest::new(
                PLAN_NAME,
                "あなたは planner",
                "一言 n-13-stdin",
                plan_schema(),
            ))
            .expect("答え");
        assert_eq!(got, serde_json::from_str::<Value>(plan).unwrap());
        assert_eq!(
            fs::read_to_string(d.join("stdin.txt")).unwrap(),
            "一言 n-13-stdin"
        );
        let argv = fs::read_to_string(d.join("argv.txt")).unwrap();
        assert!(
            argv.starts_with("-p\n") && !argv.contains("n-13-stdin") && !argv.contains("--bare"),
            "{argv}"
        );
        let env = fs::read_to_string(d.join("env.txt")).unwrap();
        for var in SCRUBBED_ENV
            .iter()
            .chain(&["CLAUDECODE", "CLAUDE_CODE_SESSION_ID"])
        {
            assert!(
                !env.lines().any(|l| l.starts_with(&format!("{var}="))),
                "{var} が子に渡った"
            );
        }
        llm.complete_json(JsonRequest::new(EDIT_NAME, "", "2 回目", edit_schema()))
            .expect_err("plan の答えは編集の schema に合わない");
        let recs = records(&d);
        assert_eq!(recs.len(), 2, "1 回ごとに 1 行: {recs:?}");
        assert_eq!(
            (
                recs[0].name.as_str(),
                recs[0].outcome.as_str(),
                recs[0].exit_code
            ),
            (PLAN_NAME, "ok", Some(0))
        );
        assert_eq!(recs[1].label, "claude-cli/9.9.1");
        let log = fs::read_to_string(d.join(CALL_LOG)).unwrap();
        assert!(
            !log.contains("n-13-stdin") && !log.contains("議事録"),
            "記録に中身を書かない: {log}"
        );
        let left: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("llm-cli."))
            .collect();
        assert!(left.is_empty(), "作業 dir が残った: {left:?}");
        let _ = fs::remove_dir_all(&d);
    }

    /// 答えない CLI は timeout で止め（retry しない）、それも 1 回に数える。
    #[test]
    fn w13_send_stops_a_cli_that_does_not_answer() {
        let d = tmp("timeout");
        let llm = cli(&d, PLAN, "{}", Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let r = llm.complete_json(JsonRequest::new(PLAN_NAME, "", "SLEEP", plan_schema()));
        assert!(matches!(r, Err(LlmError::Timeout { .. })), "{r:?}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "30 秒の sleep を待たない: {:?}",
            started.elapsed()
        );
        let recs = records(&d);
        assert_eq!(
            (recs.len(), recs[0].outcome.as_str(), recs[0].exit_code),
            (1, "timeout", None)
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// 答えが空の CLI は、終了 code を数字で言う Malformed（Some(..) の形を画面に出さない）。
    #[test]
    fn w13_send_with_an_empty_answer_says_the_exit_code() {
        let d = tmp("empty");
        let llm = cli(&d, PLAN, "", Duration::from_secs(60)).unwrap();
        match llm.complete_json(JsonRequest::new(PLAN_NAME, "", "x", plan_schema())) {
            Err(LlmError::Malformed { detail }) => {
                assert!(
                    detail.contains("exit code 0") && !detail.contains("Some"),
                    "{detail}"
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(records(&d)[0].outcome, "malformed");
        let _ = fs::remove_dir_all(&d);
    }

    /// `--output-format json` の答えの読み方（実測の形: structured_output が object・result にも文字列）。
    #[test]
    fn w13_parse_result_reads_the_structured_output() {
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}","structured_output":{"a":2}}"#;
        assert_eq!(
            parse_result(ok),
            Ok(json!({"a": 2})),
            "structured_output が先"
        );
        let text_only =
            r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}"}"#;
        assert_eq!(parse_result(text_only), Ok(json!({"a": 1})));
        let limit = r#"{"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit · resets 3pm"}"#;
        assert!(
            matches!(parse_result(limit), Err(LlmError::PlanLimit { detail }) if detail.contains("resets"))
        );
        let refused = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"refusal","result":"no"}"#;
        assert!(matches!(
            parse_result(refused),
            Err(LlmError::Refused { .. })
        ));
        for bad in [
            "not json",
            "[1]",
            r#"{"type":"result","is_error":false,"result":"hello"}"#,
            r#"{"is_error":true,"subtype":"error_during_execution"}"#,
        ] {
            assert!(
                matches!(parse_result(bad), Err(LlmError::Malformed { .. })),
                "{bad}"
            );
        }
    }

    /// G97（AUDIT2 B）: login が途中で切れた 401 は「AI の鍵を断られた」でなく login し直しの 1 行（鍵は使っていない）・
    /// network が無い時の CLI の文は「答えを読めない」でなく network の 1 行。負の対照: 401 の腕を `Auth` に戻す → 赤・`offline` を常に false → 赤。
    #[test]
    fn g97_signed_out_and_offline_say_what_happened() {
        for status in [401, 403] {
            let raw = format!(r#"{{"type":"result","subtype":"success","is_error":true,"api_error_status":{status},"result":"Invalid bearer token"}}"#);
            let line = parse_result(&raw).unwrap_err().to_string();
            assert!(next_step(&line) == Some(SIGN_IN_STEP) && !line.contains("key"), "{status}: {line}");
        }
        let offline_result = r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection error. getaddrinfo ENOTFOUND api.anthropic.com"}"#;
        for raw in [offline_result, "Error: Unable to connect to API (ConnectionRefused)"] {
            let e = parse_result(raw).unwrap_err();
            assert!(matches!(e, LlmError::Network { .. }), "{raw}: {e:?}");
            assert!(e.to_string().contains("network"), "{raw}: {e}");
        }
        assert!(matches!(parse_result("not json"), Err(LlmError::Malformed { .. })), "繋がらない語の無い壊れた答えまで network にした");
    }

    /// 401 / 403 の文が model を名指しても、OAuth や bearer なら次の一手は Sign in のまま（`model_refused` が先に食うと try again / isn't available になる）。
    /// `selected model` の拒否は Sign in にしない（g84）。status の無い model の拒否も Sign in にしない。負の対照: 401 の腕を `model_refused` の後に戻す → 1 つ目が赤。
    #[test]
    fn g97_login_401_keeps_sign_in_when_the_line_names_a_model() {
        for status in [401, 403] {
            let raw = format!(r#"{{"type":"result","subtype":"success","is_error":true,"api_error_status":{status},"result":"OAuth token does not have access to this organization (model: claude-opus-5-5)"}}"#);
            let e = parse_result(&raw).unwrap_err();
            let line = e.to_string();
            assert_eq!(next_step(&line), Some(SIGN_IN_STEP), "{status}: {line}");
            let rewritten = model_error("opus", e).to_string();
            assert_eq!(rewritten, line, "{status}: model を選んだ呼び出しで Sign in が消えた: {rewritten}");
            assert_eq!(next_step(&rewritten), Some(SIGN_IN_STEP), "{rewritten}");
        }
        let raw = r#"{"type":"result","subtype":"success","is_error":true,"result":"There's an issue with the selected model (opus). It may not exist or you may not have access to it."}"#;
        let e = parse_result(raw).unwrap_err();
        let rewritten = model_error("opus", e).to_string();
        assert!(rewritten.contains("isn't available") && next_step(&rewritten) != Some(SIGN_IN_STEP), "{rewritten}");
    }

    /// 反証の直し（W13）: plan の使用量の上限（時刻で戻る・429 で来ても）は Console の請求の語を出さず、戻る時刻をそのまま言う。
    /// 「limit」の語を含む 429 の混雑は `RateLimited`（前は語を先に見て、どちらも「待っても戻りません」の Quota だった）。
    #[test]
    fn w13_parse_result_tells_a_plan_limit_from_a_rate_limit() {
        let plan = r#"{"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit · resets 3pm"}"#;
        let plan_429 = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"You've hit your limit · resets 3pm"}"#;
        for raw in [plan, plan_429] {
            let e = parse_result(raw).unwrap_err();
            let line = e.to_string();
            assert_eq!(kind(&e), "plan_limit", "{raw}");
            assert!(
                line.contains("resets 3pm")
                    && !line.contains("Console")
                    && !line.contains("戻りません"),
                "{raw}: {line}"
            );
        }
        let busy = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"API Error: 429 rate limit exceeded"}"#;
        assert_eq!(
            parse_result(busy),
            Err(LlmError::RateLimited { retry_after: None })
        );
    }

    /// G84 × G104 の本物の通し（owner 104 通目 10-29「常に実際に本物のaiを使いながらtestしてください」・owner の plan の login の `claude -p` を 2 回・既定では回さない）。
    /// 鍵の変数と `--bare` は使わない（CliConfig の既定 = System default の account）。1 回目: `--model sonnet`・`--effort low`・添えた file 1 つ（`files/word.txt`）を
    /// 読ませて答えさせる。2 回目: `--model fable`（plan が断るなら G84 の 1 行・生の detail を出さない／通るならその事を記録・model のせいでない失敗は記録だけ）。
    /// 手で回す: `SHIBUBU_G84_LIVE_DIR=<空の dir> cargo test --bin shibubu g84_live -- --ignored --nocapture`。負の対照は quota を使わない（偽の CLI の g84_* の試験）。
    #[test]
    #[ignore = "本物の claude -p を 2 回呼ぶ（owner の plan の quota）。SHIBUBU_G84_LIVE_DIR を渡して手で回す"]
    fn g84_live_model_effort_and_a_file_through_the_real_cli() {
        let dir = PathBuf::from(std::env::var("SHIBUBU_G84_LIVE_DIR").expect("SHIBUBU_G84_LIVE_DIR に data dir を渡す"));
        fs::create_dir_all(dir.join("src")).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の claude");
        println!("cli: {} {} · models {:?} · files {} · effort {:?}", cli.program().display(), cli.label(), cli.models(), cli.takes_files(), cli.efforts());
        assert!(cli.models().iter().any(|(id, _)| id == "sonnet"), "--help から sonnet を読めない: {:?}", cli.models());
        let word = dir.join("src/word.txt");
        fs::write(&word, "pineapple").unwrap();
        let schema = json!({"type": "object", "properties": {"word": {"type": "string"}}, "required": ["word"], "additionalProperties": false});
        let mut req = JsonRequest::new("g84_live", "Answer with the single word written in the attached file.", "What word is in the attached file?", schema.clone());
        (req.model, req.effort, req.files) = (Some("sonnet".into()), Some("low".into()), vec![word]);
        let got = cli.complete_json(req).expect("sonnet・low・添付の 1 回が通らない");
        println!("sonnet: {got}");
        assert!(got["word"].as_str().is_some_and(|w| w.to_ascii_lowercase().contains("pineapple")), "添えた file を読めていない: {got}");
        let mut req = JsonRequest::new("g84_live", "Reply with the word ok.", "Say ok.", schema);
        req.model = Some("fable".into());
        match cli.complete_json(req) {
            Ok(v) => println!("fable: 通った（この plan は Fable を使える）: {v}"),
            Err(e) => {
                let line = e.to_string();
                println!("fable: {line}");
                // model の失敗なら G84 の 1 行（plan の上限・混雑など model のせいでない失敗は今の 1 行のまま = ここでは記録だけ）
                assert!(!line.contains("Fable") || line.ends_with(MODEL_STEP), "model の失敗の 1 行に次の一手が無い: {line}");
                assert!(!line.contains("model_not_found") && !line.contains("not_found_error") && !line.contains('{'), "生の detail が画面の 1 行に出た: {line}");
            }
        }
    }

    /// 偽の grok（sh）。`--version` と `models` は model を呼ばない。`--prompt-file` の時は file・argv・env・HOME を隣に写す。
    fn fake_grok(dir: &Path, models_line: &str) -> PathBuf {
        let p = dir.join("grok");
        fs::write(
            &p,
            format!(
                r#"#!/bin/sh
here=$(dirname "$0")
case "$1" in
--version) echo 'grok 1.0.70 (f50904eaee6c) [stable]' ;;
--help) echo '      --reasoning-effort <EFFORT>' ;;
models) echo '{models_line}' ;;
*)
  prompt=
  prev=
  for a in "$@"; do
    if [ "$prev" = "--prompt-file" ]; then prompt=$a; fi
    prev=$a
  done
  printf '%s\n' "$@" > "$here/argv.txt"
  env > "$here/env.txt"
  printf '%s' "$HOME" > "$here/home.txt"
  if [ -n "$prompt" ]; then cat "$prompt" > "$here/prompt.txt"; fi
  echo '{{"text":"{{\"seen\":\"hi\"}}","stopReason":"end_turn","usage":{{"input_tokens":5227,"output_tokens":2}}}}'
  ;;
esac
"#
            ),
        )
        .unwrap();
        Command::new("/bin/chmod").arg("+x").arg(&p).status().unwrap();
        p
    }

    fn seen_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["seen"],
            "properties": { "seen": { "type": "string" } }
        })
    }

    /// 107 通目: 偽の grok を 1 回。HOME は work_dir の空 dir・鍵の変数は渡らない・一言は argv に無い・token 数を 1 行に書く。
    /// 利用者の HOME を渡した時はそこを消さない（印の file が残る）。
    /// 負の対照: `grok_env_paths` が override でも空の dir を使うように戻すと、HOME が work_dir の下になって赤。
    #[test]
    fn w13_grok_send_uses_an_empty_home_and_records_input_tokens() {
        let d = tmp("grok-send");
        let program = fake_grok(&d, "You are logged in with grok.com.");
        let llm = detect_from(
            Some(program.into_os_string()),
            CliConfig { timeout: Duration::from_secs(60), ..CliConfig::new(&d) },
        )
        .expect("grok の login");
        assert_eq!(llm.label(), "grok/1.0.70");
        assert!(!llm.takes_files() && llm.efforts() == crate::domain::chat::Effort::ALL.map(|e| e.flag()), "PROVIDER-1101: Grok の effort は --help の --reasoning-effort");
        let req = JsonRequest::new("seen", "Reply with the schema only.", "一言 n-107-secret", seen_schema());
        let chars = prompt_chars(&req.instructions, &req.schema.to_string(), &req.input);
        assert_eq!(llm.complete_json(req).unwrap(), json!({ "seen": "hi" }));
        let home = fs::read_to_string(d.join("home.txt")).unwrap();
        let home_path = PathBuf::from(home.trim());
        assert_eq!(home_path, d.join("grok-home"), "HOME は空の dir: {home}");
        let names: Vec<_> = fs::read_dir(d.join("grok-home")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![".grok".to_string()], "空の HOME に在る物: {names:?}");
        assert_eq!(fs::read_dir(d.join("grok-home/.grok")).unwrap().count(), 0, "GROK_HOME は空");
        let argv = fs::read_to_string(d.join("argv.txt")).unwrap();
        assert!(argv.contains("--tools\ntodo_write\n") && !argv.contains("n-107-secret") && !argv.contains("--model"), "{argv}");
        assert_eq!(fs::read_to_string(d.join("prompt.txt")).unwrap(), "一言 n-107-secret");
        let env = fs::read_to_string(d.join("env.txt")).unwrap();
        for var in ["XAI_API_KEY", "GROK_AUTH", "ANTHROPIC_API_KEY"] {
            assert!(!env.lines().any(|l| l.starts_with(&format!("{var}="))), "{var} が子に渡った");
        }
        assert!(env.lines().any(|l| l.starts_with("GROK_AUTH_PATH=")), "login の file の path は残す");
        let recs = records(&d);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].input_tokens, Some(5227));
        assert_eq!(recs[0].prompt_chars, Some(chars));
        assert!(!fs::read_to_string(d.join(CALL_LOG)).unwrap().contains("n-107-secret"), "記録に一言を書かない");

        let user = tmp("grok-user-home");
        fs::create_dir_all(user.join(".grok")).unwrap();
        fs::write(user.join("keep-me"), "x").unwrap();
        fs::write(user.join(".grok/skill.md"), "skill").unwrap();
        let mut held = detect_from(
            Some(d.join("grok").into_os_string()),
            CliConfig { timeout: Duration::from_secs(60), ..CliConfig::new(&d) },
        )
        .unwrap();
        held.config.user_rules = Some(user.clone());
        held.complete_json(JsonRequest::new("seen", "Reply with the schema only.", "hi", seen_schema())).unwrap();
        assert_eq!(PathBuf::from(fs::read_to_string(d.join("home.txt")).unwrap().trim()), user);
        assert_eq!(fs::read_to_string(user.join("keep-me")).unwrap(), "x", "利用者の HOME を消さない");
        assert_eq!(fs::read_to_string(user.join(".grok/skill.md")).unwrap(), "skill");
        let _ = (fs::remove_dir_all(&d), fs::remove_dir_all(&user));
    }

    /// `grok models` の 1 行目が login の文でなければ model を呼ばない。
    #[test]
    fn w13_grok_login_must_be_the_exact_line() {
        let d = tmp("grok-login");
        let program = fake_grok(&d, "Not logged in");
        match detect_from(Some(program.into_os_string()), CliConfig::new(&d)) {
            Err(LlmError::NoCli { detail }) => assert!(detail.contains("signed in") && next_step(&detail).is_none(), "{detail}"),
            other => panic!("{:?}", other.map(|c| c.label())),
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// L7-AGENT の通し（本物の `claude -p` を 1 回・製品の道 = `CliLlm::detect` の `--setting-sources local`・`--tools ""`・規則も memory も持たない）。
    /// Terminal の agent の代わりに別の接続で `project::context::ask` → 画面の `poll_asks` の card → 承認 → 本物の model が一言を読む（`start_edit` の裏の thread）→
    /// `poll_model_job` で当てる → Tests の Page が在り、run は verified。Project は名前の表で作る（model を呼ばない）ので呼ぶのは承認の後の 1 回だけ。
    /// 手で回す: `SHIBUBU_W13_LIVE_DIR=<空の dir> cargo test --bin shibubu l7_live -- --ignored --nocapture`
    #[test]
    #[ignore = "本物の claude -p を 1 回呼ぶ（owner の plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l7_live_agent_ask_is_read_by_the_real_model_only_after_approval() {
        use crate::domain::action::InvocationOrigin;
        use crate::domain::run::RunStatus;
        use crate::generated::{Generated, ModelDone};
        use crate::ui::engine_view::{EngineAct, EngineView};
        use crate::ui::shell::View;
        let dir = PathBuf::from(std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"));
        fs::create_dir_all(&dir).unwrap();
        let mut g = Generated::open(&dir);
        let View::Project { project, .. } = g.create_project("notes.md を Mirror で見る Page を足して", None, None).unwrap().unwrap() else { panic!() };
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の claude");
        println!("cli: {} {}", cli.program().display(), cli.label());
        g.set_llm(Ok(Box::new(cli)));
        let terminal = crate::storage::Store::open(&dir.join("kangae.sqlite3")).unwrap();
        let by = InvocationOrigin::Terminal { terminal_id: "live".into(), agent: Some("claude".into()) };
        let asked = crate::project::context::ask(&terminal, &project, "add a Tests page", by).unwrap();
        assert!(g.poll_asks(), "card が出ない");
        assert!(matches!(g.engine_view(&project), Some(EngineView::Ask { .. })));
        // 人が card を読む間（出てから 1 秒 = ASK_GAP の間の押しは読まない）
        std::thread::sleep(Duration::from_millis(1100));
        g.engine_act(&project, EngineAct::Approve);
        let start = std::time::Instant::now();
        let done = loop {
            if let Some(d) = g.poll_model_job(|| None) {
                break d;
            }
            assert!(start.elapsed() < Duration::from_secs(300), "本物の model が 300 秒答えない");
            std::thread::sleep(Duration::from_millis(200));
        };
        let ModelDone::Edited { made, .. } = done else { panic!("編集の答えでない") };
        println!("made: {made:?} · {:.1}s · line: {:?}", start.elapsed().as_secs_f64(), g.llm_line());
        let ctx = crate::project::context::read(&terminal, &project).unwrap();
        let titles: Vec<&str> = ctx.pages.iter().map(|p| p.title.as_str()).collect();
        let run = ctx.runs.iter().find(|r| r.id == asked.id).expect("run");
        println!("pages: {titles:?} · run: {:?}", run.status);
        assert!(made.is_ok() && titles.contains(&"Tests"), "承認の後に Tests の Page: {titles:?}");
        assert_eq!(run.status, RunStatus::Verified);
    }

    /// W13 の通し（本物の `claude -p`・owner の plan の quota を 3 回使う・既定では回さない）。利用者の対話の shell で在りかを引き・plan の login を確かめ、
    /// Dev（主デモの一言）と Task Tracker（名前の表に無い app と site を名指す一言）の 2 Goal と編集 1 つを `Generated` に通す。
    /// 手で回す: `SHIBUBU_W13_LIVE_DIR=<空の dir> cargo test --bin shibubu w13_live -- --ignored --nocapture`（記録は `<dir>/llm-calls.jsonl`）。
    /// 合格は 3 行の outcome が全部 ok・Dev と Task Tracker の殻の app が `interpreter = model`・Task Tracker の面がリマインダーと https の URL（DB の行で見る）。
    /// 本物で回すのは owner の 10 通目の通り、もう 1 つの account が戻った後（10-25 ごろ）。負の対照は quota を使わない: `SHIBUBU_LLM_CLI=<上限の答えだけ返す偽の CLI>` で赤。
    #[test]
    #[ignore = "本物の claude -p を 3 回呼ぶ（owner の plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn w13_live_two_goals_and_an_edit_through_the_real_cli() {
        use crate::generated::Generated;
        use crate::ui::shell::{PageIntent, View};
        let dir = PathBuf::from(
            std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"),
        );
        fs::create_dir_all(&dir).unwrap();
        let ws = dir.join("repo");
        fs::create_dir_all(ws.join(".git")).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir))
            .expect("plan の login の claude");
        println!("cli: {} {}", cli.program().display(), cli.label());
        let mut g = Generated::open(&dir).with_llm(Ok(Box::new(cli)));
        // DB の行（I3 の tally と同じく、製品の画面でなく保存した物を読む）: Project の行と、その殻の app の行
        let db = dir.join("kangae.sqlite3");
        let rows = move |view: &View| -> (Value, Value) {
            let View::Project { project, .. } = view else {
                panic!("Project でない: {view:?}");
            };
            let c = rusqlite::Connection::open(&db).unwrap();
            let get = |table: &str, id: &str| -> Value {
                let json: String = c
                    .query_row(
                        &format!("SELECT json FROM {table} WHERE id = ?1"),
                        [id],
                        |r| r.get(0),
                    )
                    .unwrap();
                serde_json::from_str(&json).unwrap()
            };
            let p = get("projects", project);
            let app = get("apps", p["app_refs"][0].as_str().unwrap());
            (p, app)
        };
        let show = |(p, app): &(Value, Value)| {
            let titles: Vec<&str> = p["pages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|pg| pg["title"].as_str())
                .collect();
            format!(
                "pages={titles:?} surfaces={} terminals={} interpreter={}",
                p["surface_specs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["target"].to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                p["terminal_specs"].as_array().unwrap().len(),
                app["metadata"]["composer"]["interpreter"]
            )
        };
        let targets = |p: &Value| -> Vec<Value> {
            p["surface_specs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["target"].clone())
                .collect()
        };
        let dev = format!("この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい {}", ws.display());
        let made = g
            .create_project(&dev, None, None)
            .expect("Dev")
            .expect("Dev は Project");
        let dev_rows = rows(&made);
        println!("dev: {}\nline: {:?}", show(&dev_rows), g.llm_line());
        let View::Project { project, .. } = &made else {
            panic!()
        };
        let pages_before = dev_rows.0["pages"].as_array().unwrap().len();
        let edited = g
            .page_intent(
                project,
                PageIntent::Edit {
                    text: "テストの結果を見る Page を足して".into(),
                },
            )
            .expect("編集");
        let edit_rows = rows(&edited);
        println!("edit: {}\nline: {:?}", show(&edit_rows), g.llm_line());
        let tracker = "タスク管理の作業場を作って。タスクはリマインダーで管理して、参考に GitHub の Issues の頁を開きたい";
        let tracker_view = g.create_project(tracker, None, None).expect("Task Tracker");
        let tracker_rows = tracker_view.as_ref().map(&rows);
        match &tracker_rows {
            Some(r) => println!("tracker: {}\nline: {:?}", show(r), g.llm_line()),
            None => println!("tracker: single_app（名前の表に落ちた）"),
        }
        let recs = records(&dir);
        println!("calls: {}", serde_json::to_string(&recs).unwrap());
        assert_eq!(recs.len(), 3, "呼び出しは 3 回だけ");
        // 反証の直し: 記録が 3 行なだけでは、3 回とも model が失敗して名前の表で作り直しても緑になる。model の答えを使った事を DB の行で見る
        assert!(
            recs.iter().all(|r| r.outcome == "ok"),
            "3 回とも model の答えを使えた: {recs:?}"
        );
        assert_eq!(g.llm_line(), None, "名前の表に落ちた理由の 1 行が無い");
        assert_eq!(
            dev_rows.1["metadata"]["composer"]["interpreter"], "model",
            "Dev は model が読んだ"
        );
        assert_eq!(
            edit_rows.0["pages"].as_array().unwrap().len(),
            pages_before + 1,
            "編集で Page が 1 枚増えた"
        );
        let (tp, tapp) =
            tracker_rows.expect("Task Tracker は Project（名前の表なら single_app に落ちる）");
        assert_eq!(
            tapp["metadata"]["composer"]["interpreter"], "model",
            "Task Tracker は model が読んだ"
        );
        let t = targets(&tp);
        assert!(
            t.iter().any(|x| x["kind"] == "app"
                && x["bundle_id"]
                    .as_str()
                    .is_some_and(|b| b.eq_ignore_ascii_case("com.apple.reminders"))),
            "リマインダーの面: {t:?}"
        );
        assert!(
            t.iter().any(|x| x["kind"] == "url"
                && x["url"].as_str().is_some_and(|u| u.starts_with("https://"))),
            "https の面: {t:?}"
        );
        // 107 通目・RULE-045: grok も claude の道も、各回の入力の token（3 欄の和）が（instructions + schema + 一言の文字数 + 8000）を超えたら規則が混ざったので赤。
        under_the_token_cap(&recs);
    }

    /// 107 通目・RULE-045: 本物の呼び出しの記録の各行が混ざりの上限の下か（grok も claude も・入力の token = `input_tokens_of` の 3 欄の和）。
    pub(crate) fn under_the_token_cap(recs: &[CallRecord]) {
        for r in recs {
            let tokens = r.input_tokens.unwrap_or_else(|| panic!("{} の usage の入力の token が無い: {r:?}", r.name));
            let cap = r.prompt_chars.expect("文字数") + 8_000;
            println!("{} {}: input_tokens={tokens} cap={cap}", r.label, r.name);
            assert!(tokens <= cap, "{} の input_tokens {tokens} が上限 {cap} を超えた（規則・記憶が混ざった）", r.name);
        }
    }

    /// RULE-045 の負の対照（本物の claude を 1 回・既定では回さない）: 利用者の設定（`~/.claude` の CLAUDE.md・規則）を読ませると入力の token が上限を超えて赤。
    /// 製品の道（`--setting-sources local`）の 1 回が上限の下である事は `l2_live` と `w13_live` の `under_the_token_cap`。hook は起こさない（`disableAllHooks`）。
    #[test]
    #[ignore = "本物の claude を利用者の設定で 1 回。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn rule045_neg_claude_with_user_rules_exceeds_the_token_cap() {
        let dir = PathBuf::from(std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"));
        fs::create_dir_all(&dir).unwrap();
        let mut cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の claude");
        assert!(!is_grok(cli.program()), "負の対照は claude の道: {}", cli.program().display());
        cli.config.user_rules = Some(home_dir());
        let req = JsonRequest::new("seen", "Reply with the schema only.", "hi", seen_schema());
        let cap = input_token_cap(&req.instructions, &req.schema.to_string(), &req.input);
        let _ = cli.complete_json(req);
        let recs = records(&dir);
        assert_eq!(recs.len(), 1, "利用者の設定で 1 回: {recs:?}");
        let tokens = recs[0].input_tokens.expect("usage の入力の token");
        println!("neg: input_tokens={tokens} cap={cap} outcome={}", recs[0].outcome);
        assert!(tokens > cap, "利用者の設定を読んだ claude が上限 {cap} を超えない: {tokens}");
    }

    /// 107 通目の負の対照（本物の grok を 1 回・既定では回さない）。HOME を利用者の HOME にすると token が上限を超えて赤。
    /// 名前に `w13_live` を含めない（3 回の通しと別の cargo で回す）。`SHIBUBU_W13_LIVE_DIR` は空の dir。
    #[test]
    #[ignore = "本物の grok を利用者の HOME で 1 回。SHIBUBU_LLM_CLI が grok の絶対 path・SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn w13_grok_neg_user_home_exceeds_the_token_cap() {
        let program = std::env::var(CLI_ENV).expect("SHIBUBU_LLM_CLI に grok の絶対 path");
        assert!(is_grok(Path::new(&program)), "負の対照は grok だけ: {program}");
        let dir = PathBuf::from(
            std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"),
        );
        fs::create_dir_all(&dir).unwrap();
        let mut cli = CliLlm::detect(CliConfig::new(&dir)).expect("grok の login");
        cli.config.user_rules = Some(home_dir());
        let req = JsonRequest::new("seen", "Reply with the schema only.", "hi", seen_schema());
        let cap = input_token_cap(&req.instructions, &req.schema.to_string(), &req.input);
        let _ = cli.complete_json(req);
        let recs = records(&dir);
        assert_eq!(recs.len(), 1, "利用者の HOME で 1 回: {recs:?}");
        let tokens = recs[0].input_tokens.expect("usage.input_tokens");
        println!("neg: input_tokens={tokens} cap={cap} outcome={}", recs[0].outcome);
        assert!(tokens > cap, "利用者の HOME は上限 {cap} を超える: {tokens}");
    }

    /// L2（109 通目 15）本物の AI の通し（既定では回さない・本物 3 回）: 製品と同じ `talk` の JsonRequest（`chat::TALK_INSTRUCTIONS`・`talk_schema`・`talk_history`）を
    /// 製品の CLI の道（`CliLlm::detect` = 規則・memory・skill を持たない形）で送る。hello は案が null で返事が返る・作りたい物を言うと案が返る・
    /// その案を `Preview plan` の道（`start_preview`）で 1 回読ませて作る Page と app の行を見て、Accept の道（`accept_plan` → `create_project`）は
    /// 読み直さずに見せた案を保存する（review A・C・DOGFOOD-X 周 2 回 1 #2）。各回の入力の token は混ざりの上限の下（RULE-045・3 欄の和）。
    /// 「Project を作らない・案の `Accept` の時だけ作る」は app の `l2_*`（本物の ShibubuApp）が持つ。ここは model が指示と schema に従うかだけを測る。
    #[test]
    #[ignore = "本物の AI を 3 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l2_live_hello_talks_and_a_request_gets_a_proposal() {
        use crate::domain::chat::{talk_history, talk_schema, Mark, Turn, Who, TALK_INSTRUCTIONS};
        let dir = PathBuf::from(
            std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"),
        );
        fs::create_dir_all(&dir).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI");
        println!("cli: {} {}", cli.program().display(), cli.label());
        let turn = |who, text: &str| Turn { who, text: text.into(), mark: Mark::None, at: "t".into(), attachments: vec![], choices: vec![], proposal: None, preview: vec![], sections: vec![], needs: None };
        let ask = |turns: &[Turn]| {
            cli.complete_json(JsonRequest::new("talk", TALK_INSTRUCTIONS, &talk_history(turns, None), talk_schema()))
                .expect("talk の答え")
        };
        let mut turns = vec![turn(Who::You, "hello")];
        let a = ask(&turns);
        println!("hello -> {a}");
        assert!(a["proposal"].is_null(), "hello に案が付いた: {a}");
        assert!(a["ask"].as_str().is_some_and(|s| !s.trim().is_empty()), "hello に返事が無い: {a}");
        turns.push(turn(Who::Shibubu, a["ask"].as_str().unwrap()));
        turns.push(turn(Who::You, "I want to track my inventory: items, how many I have, and a list of what is running low."));
        let b = ask(&turns);
        println!("request -> {b}");
        assert!(b["proposal"].as_str().is_some_and(|s| !s.trim().is_empty()), "作りたい物を言ったのに案が無い: {b}");
        // review の試験の穴: 本物の案を Accept まで通す（本物 1 回 = Preview の plan の読み）。Accept は読み直さない・会話は作った Project の頭へ
        let proposal = b["proposal"].as_str().unwrap().trim().to_owned();
        turns.push(Turn { proposal: Some(proposal.clone()), ..turn(Who::Shibubu, b["ask"].as_str().unwrap_or("")) });
        let mut g = crate::generated::Generated::open(&dir);
        g.set_llm(Ok(Box::new(CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI"))));
        assert!(g.start_preview(&proposal, None, &[], None), "案の読みを始められない");
        let t0 = std::time::Instant::now();
        let lines = loop {
            match g.poll_model_job(|| None) {
                Some(crate::generated::ModelDone::Previewed { lines, .. }) => break lines,
                Some(_) => panic!("Preview の読みの答えでない"),
                None => assert!(t0.elapsed() < std::time::Duration::from_secs(300), "案の読みが終わらない"),
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        println!("preview -> {lines:?}");
        assert!(lines.last().map(String::as_str) == Some(crate::domain::chat::PLAN_UNMADE) && g.previewed(&proposal).is_some(), "Preview が案を読めない: {lines:?}");
        g.accept_plan(&proposal, turns.clone());
        let made = g.create_project(&proposal, None, None);
        println!("accept -> {made:?} · line: {:?}", g.llm_line());
        match made.expect("案の Accept が作れない") {
            Some(crate::ui::shell::View::Project { project, .. }) => {
                let chat = g.chat(&project);
                println!("chat: {:?}", chat.iter().map(|t| t.text.as_str()).collect::<Vec<_>>());
                assert_eq!(chat.first().map(|t| t.text.as_str()), Some("hello"), "作った Project の会話の頭が Home の会話でない");
                let a = g.app_of_project(&project).unwrap();
                let titles: Vec<String> = g.project_of(&a).unwrap().unwrap().0.pages.iter().map(|p| p.title.clone()).collect();
                assert!(titles.iter().all(|t| lines[0].contains(t.as_str())), "Preview で見せた Page と作った Page が違う: {titles:?} / {lines:?}");
            }
            other => println!("1 Page の app の道（会話は Project の列が無いので写さない）: {other:?}"),
        }
        let recs = records(&dir);
        println!("calls: {}", serde_json::to_string(&recs).unwrap());
        assert_eq!(recs.len(), 3, "呼び出しは 3 回だけ（会話 2・Preview の案の読み 1・Accept は 0）: {recs:?}");
        under_the_token_cap(&recs);
    }

    /// DOGFOOD-X 周 4 #3・周 5 #5 本物の AI（既定では回さない・本物 2 回）: 製品と同じ `talk` の JsonRequest（`TALK_INSTRUCTIONS`・`talk_schema`）を製品の CLI の道で。
    /// 周 4 の一言そのまま（作る物を言っている一言目）→ その turn で案が返り、細部の問いは needs に入れない（Accept を止めない）。
    /// 周 5 の一言そのまま（どの Numbers の書類か言っていない）→ needs に問いが返る（card の primary が灰になる側）。各回の入力の token は上限の下（RULE-045）。
    #[test]
    #[ignore = "本物の AI を 2 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l2_live_a_first_request_gets_a_proposal_and_a_missing_file_needs_an_answer() {
        use crate::domain::chat::{talk_history, talk_schema, Mark, Turn, Who, TALK_INSTRUCTIONS};
        // 自分の dir（同じ束の l2_live_hello_… と呼び出しの記録を混ぜない = 回数の assert が互いに崩れない）
        let dir = PathBuf::from(
            std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"),
        )
        .join("first-request");
        fs::create_dir_all(&dir).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI");
        println!("cli: {} {}", cli.program().display(), cli.label());
        let first = |text: &str| {
            let turns = [Turn { who: Who::You, text: text.into(), mark: Mark::None, at: "t".into(), attachments: vec![], choices: vec![], proposal: None, preview: vec![], sections: vec![], needs: None }];
            cli.complete_json(JsonRequest::new("talk", TALK_INSTRUCTIONS, &talk_history(&turns, None), talk_schema())).expect("talk の答え")
        };
        let a = first("I want to work on a small test repo with Ghostty and Zed side by side");
        println!("周 4 -> {a}");
        assert!(a["proposal"].as_str().is_some_and(|s| !s.trim().is_empty()), "一言目が作る物を言ったのに案が無い: {a}");
        assert!(a["needs"].is_null(), "細部の問いで Accept を止めた: {a}");
        let b = first("Copy the 2022 numbers from the first table on https://en.wikipedia.org/wiki/United_States_federal_budget into my budget table in Numbers");
        println!("周 5 -> {b}");
        assert!(b["needs"].as_str().is_some_and(|s| !s.trim().is_empty()), "どの書類かを聞かずに作れる形で返した: {b}");
        let recs = records(&dir);
        println!("calls: {}", serde_json::to_string(&recs).unwrap());
        assert_eq!(recs.len(), 2, "呼び出しは 2 回だけ: {recs:?}");
        under_the_token_cap(&recs);
    }

    /// B61-REPO（録画の手 1・本物の AI 1 回・既定では回さない）: 主デモの一言（「this repo」）に、決まった repo の 1 行（`talk_history` の `TALK_REPO`）を付けると、
    /// model は repo を聞き返さず（needs が null）案を出す。前は claude の cwd（data dir・git でない）を見て「Which repo do you mean?」を needs に入れ、案の card で止まった。
    #[test]
    #[ignore = "本物の AI を 1 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l2_live_a_known_repo_is_not_asked_again() {
        let a = live_repo_talk("repo", "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. Put each on its own page.");
        assert!(a["needs"].is_null(), "決まった repo を聞き返した: {a}");
        assert!(a["proposal"].as_str().is_some_and(|s| !s.trim().is_empty()), "作る物を言ったのに案が無い: {a}");
    }

    /// B63（本物の AI 1 回・既定では回さない）: 決まった repo（shibubu-yc-demo）が在っても、一言が別の repo を名指したら、それを決まった repo で代用せず
    /// needs で聞く（Engine P0「名指しの対象が決まらなければ代用しない」）。負の対照（束の arm）: B61 の前の文（never ask which repo）に戻す → 聞かずに赤。
    #[test]
    #[ignore = "本物の AI を 1 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l2_live_a_named_other_repo_is_asked() {
        let a = live_repo_talk("other-repo", "Set up a dev environment for my other repo foo-api. Edit its README in TextEdit.");
        assert!(a["needs"].as_str().is_some_and(|s| !s.trim().is_empty()), "別の repo を名指したのに聞かず、決まった repo で代用した: {a}");
    }

    /// B61 / B63 の本物の会話 1 回: 決まった repo（shibubu-yc-demo）の 1 行（`talk_history` の `TALK_REPO`）＋ 一言 → talk の答え。呼び出しは 1 回・token の上限の内。
    fn live_repo_talk(sub: &str, said: &str) -> serde_json::Value {
        use crate::domain::chat::{talk_history, talk_schema, Mark, Turn, Who, TALK_INSTRUCTIONS};
        let dir = PathBuf::from(std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す")).join(sub);
        fs::create_dir_all(&dir).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI");
        let turns = [Turn { who: Who::You, text: said.into(), mark: Mark::None, at: "t".into(), attachments: vec![], choices: vec![], proposal: None, preview: vec![], sections: vec![], needs: None }];
        let a = cli.complete_json(JsonRequest::new("talk", TALK_INSTRUCTIONS, &talk_history(&turns, Some("shibubu-yc-demo")), talk_schema())).expect("talk の答え");
        println!("{sub} -> {a}");
        let recs = records(&dir);
        assert_eq!(recs.len(), 1, "呼び出しは 1 回だけ: {recs:?}");
        under_the_token_cap(&recs);
        a
    }

    /// TALK-1101（owner 190・194 通目「送った言語に合わせて返す」・前の CHAT-011 の English 固定をやめた）本物の AI（既定では回さない・本物 1 回）:
    /// 日本語の一言には、答え（ask・proposal）が日本語で返り、利用者が名指しした app の名前（TextEdit）はそのまま残る。製品と同じ `talk` の JsonRequest を
    /// 製品の CLI の道で。入力の token は上限の下（RULE-045）。
    #[test]
    #[ignore = "本物の AI を 1 回（plan の quota）。SHIBUBU_W13_LIVE_DIR を渡して手で回す"]
    fn l2_live_a_japanese_sentence_gets_japanese_fields() {
        use crate::domain::chat::{talk_history, talk_schema, Mark, Turn, Who, TALK_INSTRUCTIONS};
        let dir = PathBuf::from(
            std::env::var("SHIBUBU_W13_LIVE_DIR").expect("SHIBUBU_W13_LIVE_DIR に data dir を渡す"),
        )
        .join("japanese");
        fs::create_dir_all(&dir).unwrap();
        let cli = CliLlm::detect(CliConfig::new(&dir)).expect("plan の login の CLI");
        let turns = [Turn { who: Who::You, text: "毎週の会議のメモを TextEdit にまとめて、決まった事を一覧にしたい".into(), mark: Mark::None, at: "t".into(), attachments: vec![], choices: vec![], proposal: None, preview: vec![], sections: vec![], needs: None }];
        let a = cli.complete_json(JsonRequest::new("talk", TALK_INSTRUCTIONS, &talk_history(&turns, None), talk_schema())).expect("talk の答え");
        println!("ja -> {a}");
        let japanese = |s: &str| s.chars().any(|c| matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff00..=0xffef));
        assert!(["ask", "proposal"].iter().all(|k| a[*k].as_str().is_some_and(japanese)), "日本語の一言に日本語で返らない: {a}");
        assert!(a["proposal"].as_str().is_some_and(|s| s.contains("TextEdit")), "名指しの app の名前が案から消えた: {a}");
        let recs = records(&dir);
        assert_eq!(recs.len(), 1, "呼び出しは 1 回だけ: {recs:?}");
        under_the_token_cap(&recs);
    }

    /// RQ2 interface（caller の形・本体の前は `add_key_account` / `child_env` の todo!() で赤）: 既定と plan の account は鍵の変数を全部外し、
    /// plan の account は自分の設定の dir だけを渡し、鍵の account は Keychain の自分の鍵だけを渡す（設定に残るのは末尾 4 文字）。偽の鍵・memory の Keychain だけ（使った分だけ払う API を呼ばない）。
    /// 負の対照（commit message）: `child_env` の `Key` で `ANTHROPIC_AUTH_TOKEN` を外さない → 赤。
    #[test]
    fn rq2_plan_account_strips_keys_and_key_account_passes_only_its_key() {
        use crate::model::AiAccount;
        use crate::storage::secret_store::SecretStore;
        let dir = tmp("rq2-acct");
        let keys = SecretStore::memory();
        let fake_key = "sk-ant-test-not-a-real-key-a1b2";

        let env = child_env(None, &dir, &keys).unwrap();
        for var in SCRUBBED_ENV {
            assert!(env.contains(&(var, None)), "System default が {var} を外していない: {env:?}");
        }
        assert!(!env.iter().any(|(k, _)| *k == "CLAUDE_CONFIG_DIR"), "System default に設定の dir を置いた");

        let plan = AiAccount::Plan { id: "plan-1".into() };
        let env = child_env(Some(&plan), &dir, &keys).unwrap();
        let own = dir.join(ACCOUNTS_DIR).join("plan-1").display().to_string();
        assert!(env.contains(&("CLAUDE_CONFIG_DIR", Some(own))), "{env:?}");
        assert!(env.contains(&("ANTHROPIC_API_KEY", None)) && env.contains(&("CLAUDE_CODE_OAUTH_TOKEN", None)), "{env:?}");
        assert!(env.contains(&("CLAUDE_SECURESTORAGE_CONFIG_DIR", None)), "Keychain の名前を変える変数を外していない: {env:?}");

        assert!(add_key_account("not a key", &keys, &[]).is_err(), "鍵でない物を受けた");
        let key = add_key_account(&format!("  {fake_key}\n"), &keys, &[plan.clone()]).unwrap();
        assert_eq!(key, AiAccount::Key { id: "key-1".into(), last4: "a1b2".into() });
        assert!(!format!("{key:?}").contains("sk-ant"), "account に鍵の値が残った");
        let env = child_env(Some(&key), &dir, &keys).unwrap();
        assert!(env.contains(&("ANTHROPIC_API_KEY", Some(fake_key.to_string()))), "{env:?}");
        assert!(env.contains(&("ANTHROPIC_AUTH_TOKEN", None)), "{env:?}");

        remove_account(&dir, &key, &keys, |_| -> Result<(), String> { panic!("鍵の Remove で logout を起こした") }).unwrap();
        match child_env(Some(&key), &dir, &keys) {
            Err(LlmError::NoCli { detail }) => assert!(detail.contains("Paste API key") && !detail.contains("sk-ant"), "{detail}"),
            other => panic!("消した鍵で子を起こそうとした: {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// RQ-API: 鍵の値は argv（`ps` に見える）に出ず、script の export だけに入る。plan の account の設定の dir も argv に出さない。
    /// 負の対照（commit message）: `command_line` の `-u` の filter を外して全部の変数を argv に → `-u ANTHROPIC_API_KEY` が出て赤。
    #[test]
    fn rq2_key_reaches_the_cli_only_through_the_script_env() {
        use crate::model::AiAccount;
        use crate::storage::secret_store::SecretStore;
        let (dir, keys) = (tmp("rq2-argv"), SecretStore::memory());
        let key = add_key_account("sk-ant-test-not-a-real-key-z9y8", &keys, &[]).unwrap();
        let env = child_env(Some(&key), &dir, &keys).unwrap();
        let req = JsonRequest::new(PLAN_NAME, "", "", plan_schema());
        let argv = command_line(Path::new("/opt/x/claude"), &req, &env, Path::new("/tmp/prompt.txt"), None);
        assert!(!argv.iter().any(|a| a.contains("sk-ant")), "鍵が argv に出た: {argv:?}");
        assert!(!argv.windows(2).any(|w| w == ["-u", "ANTHROPIC_API_KEY"]), "選んだ鍵を外した: {argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-u", "ANTHROPIC_AUTH_TOKEN"]), "{argv:?}");
        assert_eq!(exports(&env), "export ANTHROPIC_API_KEY='sk-ant-test-not-a-real-key-z9y8'\n");
        let plan = AiAccount::Plan { id: "plan-1".into() };
        let env = child_env(Some(&plan), &dir, &keys).unwrap();
        assert!(!command_line(Path::new("/opt/x/claude"), &req, &env, Path::new("/tmp/prompt.txt"), None).iter().any(|a| a.contains("ai-accounts")));
        assert!(exports(&env).starts_with("export CLAUDE_CONFIG_DIR="), "{}", exports(&env));
        // id の 2 つ目は key-2・Remove は ai-accounts の外を指す id を消さない
        let second = add_key_account("sk-ant-test-not-a-real-key-x7w6", &keys, &[key.clone()]).unwrap();
        assert_eq!(second.id(), "key-2");
        assert!(remove_account(&dir, &AiAccount::Plan { id: "../..".into() }, &keys, |_| Ok(())).is_err());
        assert!(remove_account(&dir, &AiAccount::Plan { id: "plan-9".into() }, &keys, |_| -> Result<(), String> { panic!("無い dir で logout を起こした") }).is_ok(), "無い物を消すのは Ok");
        let _ = fs::remove_dir_all(&dir);
    }

    /// RQ-ACCT: plan の account を選ぶと、偽の CLI の `auth status` と `-p` の両方が自分の設定の dir（`CLAUDE_CONFIG_DIR`）で起き、鍵の変数は渡らない。
    /// 負の対照（commit message）: `call_in` の `exports` の行を body から外す → env.txt に CLAUDE_CONFIG_DIR が無く赤。
    #[test]
    fn rq2_plan_account_runs_the_cli_in_its_own_config_dir() {
        use crate::model::AiAccount;
        let d = tmp("rq2-plan");
        let program = fake_cli(&d, PLAN, r#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"steps":[]}}"#);
        let cfg = CliConfig { account: Some(AiAccount::Plan { id: "plan-1".into() }), ..CliConfig::new(&d) };
        let llm = detect_from(Some(program.into_os_string()), cfg).expect("plan の account");
        let _ = llm.complete_json(JsonRequest::new(EDIT_NAME, "", "x", edit_schema()));
        let env = fs::read_to_string(d.join("env.txt")).expect("-p が起きた");
        let own = format!("CLAUDE_CONFIG_DIR={}", d.join(ACCOUNTS_DIR).join("plan-1").display());
        assert!(env.lines().any(|l| l == own), "{own} が渡っていない");
        assert!(!env.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=") || l.starts_with("CLAUDE_CODE_OAUTH_TOKEN=")));
        let _ = fs::remove_dir_all(&d);
    }

    /// 1 回だけ答える偽の使用量の server（local の port）。受けた依頼の頭（header）を返す。
    fn usage_server(reply: &'static str) -> (String, thread::JoinHandle<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/oauth/usage", l.local_addr().unwrap());
        let h = thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut got = Vec::new();
            let mut b = [0u8; 1024];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match c.read(&mut b) { Ok(0) | Err(_) => break, Ok(n) => got.extend_from_slice(&b[..n]) }
            }
            if reply == "SILENT" {
                thread::sleep(Duration::from_millis(1500));
            } else {
                let _ = c.write_all(reply.as_bytes());
            }
            String::from_utf8_lossy(&got).into_owned()
        });
        (url, h)
    }

    /// RQ-USAGE: 偽の server（model も Anthropic も呼ばない）の答えから %・戻る時刻を読み、失敗は種類ごとに分ける。token は Err の文に出ない。
    /// 負の対照（commit message）: `401 | 403` の腕を消す → `Rejected` に落ちて赤。
    #[test]
    fn rq2_read_plan_usage_reads_the_windows_and_says_each_failure() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"five_hour\":{\"utilization\":42.4,\"resets_at\":\"2026-10-28T12:10:00.5+00:00\"},\"seven_day\":{\"utilization\":18,\"resets_at\":null},\"limits\":[{\"kind\":\"weekly_scoped\",\"percent\":7,\"resets_at\":1790000000000,\"scope\":{\"model\":{\"display_name\":\"Fable\"}}}]}";
        let (url, h) = usage_server(ok);
        let u = read_plan_usage(&url, "tok-test-123", Duration::from_secs(5)).expect("読めた");
        let head = h.join().unwrap().to_lowercase();
        assert!(head.starts_with("get /api/oauth/usage") && head.contains("authorization: bearer tok-test-123") && head.contains("anthropic-beta: oauth-2025-04-20"), "{head}");
        assert_eq!(u.windows, vec![
            ("Session".to_string(), 42, crate::ui::runs::unix_secs("2026-10-28T12:10:00Z")),
            ("Weekly".to_string(), 18, None),
            ("Weekly · Fable".to_string(), 7, Some(1_790_000_000)),
        ]);
        let cases: [(&'static str, fn(&LlmError) -> bool); 5] = [
            ("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", |e| matches!(e, LlmError::NoCli { detail } if next_step(detail) == Some(SIGN_IN_STEP))),
            ("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", |e| *e == LlmError::RateLimited { retry_after: Some(30) }),
            ("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", |e| *e == LlmError::Server { status: 503 }),
            ("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}", |e| matches!(e, LlmError::Malformed { .. })),
            ("SILENT", |e| matches!(e, LlmError::Timeout { .. })),
        ];
        for (reply, want) in cases {
            let (url, h) = usage_server(reply);
            let e = read_plan_usage(&url, "tok-test-123", Duration::from_millis(400)).expect_err(reply);
            assert!(want(&e), "{reply}: {e:?}");
            assert!(!format!("{e:?}").contains("tok-test"), "token が Err に出た: {e:?}");
            let _ = h.join();
        }
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let e = read_plan_usage(&format!("http://{closed}/x"), "tok-test-123", Duration::from_secs(2)).expect_err("閉じた port");
        assert!(matches!(e, LlmError::Network { .. }), "{e:?}");
    }

    /// RQ-USAGE: Claude Code の Keychain の名前（設定の dir つきは sha256 の先頭 8 桁）と、その項目の JSON から token を読む。
    #[test]
    fn rq2_oauth_token_names_the_claude_code_keychain_item() {
        assert_eq!(claude_keychain_service(None), "Claude Code-credentials");
        assert_eq!(claude_keychain_service(Some(Path::new("/tmp/shibubu-acct/plan-1"))), "Claude Code-credentials-cbe8f985");
        assert_eq!(oauth_token_of(r#"{"claudeAiOauth":{"accessToken":"t-1","refreshToken":"r"}}"#).as_deref(), Some("t-1"));
        assert_eq!(oauth_token_of(r#"{"claudeAiOauth":{}}"#), None);
        assert_eq!(oauth_token_of("not json"), None);
    }

    /// RQ2 反証の直し: Keychain の読みの失敗（時間切れ・鍵の掛かった Keychain）は「Sign in」でなく自分の 1 行・file に token が在ればそれ・項目が無いだけなら Sign in。
    /// 負の対照（commit message）: `token_from` の `Err(()) => unreadable = true` を `{}` に → `NOT_SIGNED_IN` が出て赤。
    #[test]
    fn rq2_keychain_read_failure_is_not_called_signed_out() {
        let creds = r#"{"claudeAiOauth":{"accessToken":"t-file"}}"#.to_string();
        let detail = |r: Result<String, LlmError>| match r {
            Err(LlmError::NoCli { detail }) => detail,
            other => panic!("{other:?}"),
        };
        assert_eq!(detail(token_from(vec![Err(())], None)), crate::llm::KEYCHAIN_UNREADABLE);
        assert_eq!(detail(token_from(vec![Ok(None)], None)), NOT_SIGNED_IN);
        assert_eq!(token_from(vec![Err(())], Some(creds)).ok().as_deref(), Some("t-file"));
        assert_eq!(token_from(vec![Err(()), Ok(Some(r#"{"claudeAiOauth":{"accessToken":"t-kc"}}"#.into()))], None).ok().as_deref(), Some("t-kc"));
    }

    /// RQ2 反証の直し（holdout h5・YC_DECISIONS 89「断った鍵は使わない」）: 利用者が Claude Code で断った鍵（`~/.claude.json` の rejected = 鍵の末尾 20 文字）は子に渡らない。
    /// 環境の鍵（`EnvKey`）は System default と同じに外し、Shibubu に貼った鍵（`Key`）は理由の 1 行で止める。断っていない鍵はそのまま渡る。Bedrock の鍵はどの account でも外す。
    /// 偽の鍵・偽の HOME・memory の Keychain だけ（process の env は書き換えない）。負の対照（commit message）: `child_env_with` の EnvKey の断った腕を消す → 鍵が残って赤。
    #[test]
    fn rq2_declined_key_never_reaches_the_child() {
        use crate::model::AiAccount;
        use crate::storage::secret_store::SecretStore;
        let (dir, keys) = (tmp("rq2-declined"), SecretStore::memory());
        let (declined, fine) = ("sk-ant-test-declined-0000000000000000001", "sk-ant-test-fine-00000000000000000000002");
        let tail: String = declined.chars().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
        fs::write(dir.join(".claude.json"), format!(r#"{{"customApiKeyResponses":{{"rejected":["{tail}"],"approved":[]}}}}"#)).unwrap();
        let home = dir.display().to_string();
        let env_of = |key: &'static str| {
            let home = home.clone();
            move |v: &str| match v {
                "HOME" => Some(home.clone()),
                "ANTHROPIC_API_KEY" => Some(key.to_string()),
                _ => None,
            }
        };
        let env = child_env_with(Some(&AiAccount::EnvKey), &dir, &keys, &env_of(declined)).unwrap();
        assert!(env.contains(&("ANTHROPIC_API_KEY", None)), "断った環境の鍵を子に渡す: {env:?}");
        let env = child_env_with(Some(&AiAccount::EnvKey), &dir, &keys, &env_of(fine)).unwrap();
        assert!(!env.iter().any(|(k, _)| *k == "ANTHROPIC_API_KEY") && env.contains(&("AWS_BEARER_TOKEN_BEDROCK", None)), "{env:?}");
        let pasted = add_key_account(declined, &keys, &[]).unwrap();
        match child_env_with(Some(&pasted), &dir, &keys, &env_of(fine)) {
            Err(LlmError::NoCli { detail }) => assert!(detail == DECLINED_KEY && !detail.contains("sk-ant"), "{detail}"),
            other => panic!("断った鍵を貼った account で子を起こそうとした: {other:?}"),
        }
        let plan = child_env_with(Some(&AiAccount::Plan { id: "plan-1".into() }), &dir, &keys, &env_of(fine)).unwrap();
        assert!(plan.contains(&("AWS_BEARER_TOKEN_BEDROCK", None)), "plan の account に Bedrock の鍵が渡る: {plan:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// G99: 実行できる偽の file（sh の script）を置く。
    fn g99_exe(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
        Command::new("/bin/chmod").arg("+x").arg(p).status().unwrap();
    }

    /// G99 1: login shell の答えの区切りの間だけが PATH（rc の banner・prompt・ANSI の色が前後に在っても）。区切りが 1 つなら無し。
    #[test]
    fn g99_the_login_path_is_read_between_the_marks() {
        let out = format!("\x1b[1;32mWelcome\x1b[0m to zsh\n\x1b]0;title\x07{PATH_MARK}/a/bin:/b/bin{PATH_MARK}\x1b[?2004h% ");
        assert_eq!(parse_path_probe(&out).as_deref(), Some("/a/bin:/b/bin"));
        assert_eq!(parse_path_probe(&format!("{PATH_MARK}/a/bin")), None, "区切りが 1 つ");
        assert_eq!(parse_path_probe(&format!("{PATH_MARK}{PATH_MARK}")), None, "空");
        assert_eq!(merge_path("/a:/b", "/b:/c::/a"), "/a:/b:/c");
    }

    /// G99 2: `.zprofile` だけで PATH を足した道具は、在りか探し（login shell の PATH）でも Terminal の既定の shell（`-l -i`）でも見つかる。
    /// bash で `.bashrc` だけに足した PATH は login shell では読まれない（Terminal.app と同じ結果・`.bash_profile` が source しない時）。
    /// 変異: Terminal の既定から `-l` を外す → 赤（`-i` は `.zprofile` を読まない）。
    #[test]
    fn g99_a_zprofile_path_reaches_the_finder_and_the_terminal() {
        let home = tmp("g99-home");
        let tools = home.join("tools");
        g99_exe(&tools.join("g99tool"), "echo 1.0");
        fs::write(home.join(".zprofile"), "export PATH=\"$HOME/tools:$PATH\"\n").unwrap();
        let got = probe_login_path("/bin/zsh", &home, PATH_PROBE).expect("zsh の login の PATH");
        assert!(got.split(':').any(|d| Path::new(d) == tools), "login の PATH に .zprofile の dir が無い: {got}");
        let path = merge_path(&got, &seed_path(&home.display().to_string()));
        assert_eq!(runnable(&cli_candidates("g99tool", &path, &home), &path), Some(tools.join("g99tool")));
        let term_path = |shell: &str| {
            let o = Command::new(shell)
                .args(&TerminalConfig::default().shell_args)
                .args(["-c", "printf %s \"$PATH\""])
                .env("HOME", &home)
                .env_remove("ZDOTDIR")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        assert!(term_path("/bin/zsh").split(':').any(|d| Path::new(d) == tools), "Terminal の shell が .zprofile の PATH を読まない（-l が無い）");
        fs::remove_file(home.join(".zprofile")).unwrap();
        fs::write(home.join(".bashrc"), "export PATH=\"$HOME/tools:$PATH\"\n").unwrap();
        assert!(!term_path("/bin/bash").split(':').any(|d| Path::new(d) == tools), "bash の login shell は .bashrc だけを読まない（Terminal.app と同じ）");
        let _ = fs::remove_dir_all(&home);
    }

    /// G99 3: PATH に無い CLI も決まった install dir（nvm の default の版・kimi の installer の dir）から見つかる。MCP の行も同じ探し方。
    /// 変異: `install_dirs` から nvm を外す → 赤・kimi の dir を外す → 赤。
    #[test]
    fn g99_install_dirs_find_a_cli_off_the_path() {
        let home = tmp("g99-dirs");
        for v in ["v18.20.1", "v20.11.0"] {
            g99_exe(&home.join(format!(".nvm/versions/node/{v}/bin/g99node")), "echo ok");
        }
        fs::create_dir_all(home.join(".nvm/alias")).unwrap();
        fs::write(home.join(".nvm/alias/default"), "18\n").unwrap();
        g99_exe(&home.join(".kimi-code/bin/g99kimi"), "echo ok");
        let found = cli_candidates("g99node", "", &home);
        assert_eq!(found.first(), Some(&home.join(".nvm/versions/node/v18.20.1/bin/g99node")), "nvm の default の版が先でない: {found:?}");
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(cli_candidates("g99kimi", "", &home), [home.join(".kimi-code/bin/g99kimi")]);
        assert!(cli_candidates("g99none", "", &home).is_empty() && cli_candidates("a/b", "", &home).is_empty());
        assert!(crate::mcp::executable_in("g99node", "", &home), "MCP の行が nvm の CLI を見つけない");
        let _ = fs::remove_dir_all(&home);
    }

    /// G99 5: 見つからない後に CLI を入れて探し直すと、再起動なしで見つかる（候補は毎回 disk を見る・持っているのは login の PATH だけ）。
    /// 1 行ごとの button: 入っていない → Install・login の 3 つ → Sign in・他は無し。
    #[test]
    fn g99_installing_after_a_miss_is_found_on_the_next_look_and_the_line_says_what_to_press() {
        let home = tmp("g99-late");
        assert_eq!(runnable(&cli_candidates("g99late", "", &home), ""), None);
        g99_exe(&home.join(".local/bin/g99late"), "echo 1.0");
        assert_eq!(runnable(&cli_candidates("g99late", "", &home), ""), Some(home.join(".local/bin/g99late")));
        assert_eq!(next_step(NOT_INSTALLED), Some(INSTALL_STEP));
        for line in [NOT_SIGNED_IN, KEY_SIGN_IN, SIGNED_OUT, SIGN_IN_FAILED] {
            assert_eq!(next_step(line), Some(SIGN_IN_STEP), "{line}");
        }
        assert_eq!(next_step("AI is off for this launch"), None);
        for line in [NOT_INSTALLED, NOT_SIGNED_IN, KEY_SIGN_IN, SIGNED_OUT] {
            assert!(!line.contains("Terminal"), "Terminal.app へ行かせる 1 行: {line}");
        }
        let _ = fs::remove_dir_all(&home);
    }

    /// G99 4: PATH の先に壊れた shim（`--version` で 1）・後ろに動く実体 → 動く方を選ぶ（呼び出しと login の確かめはその絶対 path）。
    /// alias だけで file が無い名前は候補にならない。変異: `runnable` を候補の先頭に戻す → 赤。
    #[test]
    fn g99_a_broken_shim_on_the_path_is_skipped() {
        let d = tmp("g99-shim");
        let (dead, live) = (d.join("a"), d.join("b"));
        g99_exe(&dead.join("g99cli"), "exit 1");
        g99_exe(&live.join("g99cli"), "echo 2.0.0");
        let path = format!("{}:{}", dead.display(), live.display());
        let found = cli_candidates("g99cli", &path, &d);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(runnable(&found, &path), Some(live.join("g99cli")));
        let _ = fs::remove_dir_all(&d);
    }

    /// B87（owner 270）: 混んだ Mac で `--version` が `VERSION_PROBE` の内に答えない本物の CLI を「Claude Code isn't on this Mac」にしない。
    /// 0 で終わった物が在ればそれが先・0 以外で終わる壊れた shim は今まで通り飛ばす・どれも 0 で終わらなければ時間切れの最初の 1 つ。
    /// 負の対照: `runnable` の終わりの `late` を `None` に戻す → 2 つ目の assert が赤。
    #[test]
    fn b87_a_cli_too_slow_for_the_version_probe_is_still_found() {
        let d = tmp("b87-slow");
        let (dead, slow, live) = (d.join("a"), d.join("b"), d.join("c"));
        g99_exe(&dead.join("b87cli"), "exit 1");
        g99_exe(&slow.join("b87cli"), "exec /bin/sleep 5");
        g99_exe(&live.join("b87cli"), "echo 2.1.317");
        let all = format!("{}:{}:{}", dead.display(), slow.display(), live.display());
        assert_eq!(runnable(&cli_candidates("b87cli", &all, &d), &all), Some(live.join("b87cli")), "0 で終わった物より時間切れの物を選んだ");
        let no_live = format!("{}:{}", dead.display(), slow.display());
        assert_eq!(runnable(&cli_candidates("b87cli", &no_live, &d), &no_live), Some(slow.join("b87cli")), "時間切れの CLI を「無い」にした");
        assert_eq!(runnable(&[dead.join("b87cli")], ""), None, "0 以外で終わる壊れた shim を選んだ");
        let _ = fs::remove_dir_all(&d);
    }

    /// G104 の偽の CLI: `--help` に旗を書く（`help` = 書く旗）・`-p` の時は argv・stdin・cwd の `files/` の名前と中身を隣に写す。
    fn g104_cli(dir: &Path, help: &str, reply: &str) -> CliLlm {
        let p = dir.join("claude");
        fs::write(
            &p,
            format!(
                "#!/bin/sh\nhere=$(dirname \"$0\")\ncase \"$1\" in\n--version) echo '9.9.1 (Claude Code)' ;;\n--help) printf '%s\\n' '{help}' ;;\nauth) echo '{PLAN}' ;;\n-p) printf '%s\\n' \"$@\" > \"$here/argv.txt\"; cat > \"$here/stdin.txt\"; ls files > \"$here/ls.txt\" 2>/dev/null; cat files/* > \"$here/files.txt\" 2>/dev/null\n  echo '{reply}' ;;\nesac\n"
            ),
        )
        .unwrap();
        Command::new("/bin/chmod").arg("+x").arg(&p).status().unwrap();
        detect_from(Some(p.into_os_string()), CliConfig { timeout: Duration::from_secs(60), ..CliConfig::new(dir) }).unwrap()
    }

    /// G104（CHAT-001・CHAT-005）: `--help` に `--effort` と `--restricted` が在る CLI は添付と effort を受ける。添えた file は呼び出しの dir の `files/` に写り
    /// （CLI の cwd = そこ・`--tools Read --restricted`）、一言の後ろに `files/<名前>`・`--effort low`。添付も effort も無い呼び出しの argv は前と同じ。
    /// `--help` に旗の無い CLI は受けない（menu の項目は押せない形）。
    /// 負の対照: command_line の `--effort` の行を外す → 赤・attach_files の `fs::copy` を外す → 赤・`--restricted` を外す → 赤。
    #[test]
    fn g104_files_and_effort_reach_the_cli() {
        let d = tmp("g104");
        let plan = r#"{"shape":"project","name":"議事録","pages_requested":false,"seats":[]}"#;
        let reply = format!(r#"{{"type":"result","subtype":"success","is_error":false,"result":"x","structured_output":{plan}}}"#);
        let llm = g104_cli(&d, "  --effort <level>   Effort level (low, medium, high, xhigh, max)\n  --restricted   Restrict file tools", &reply);
        assert!(llm.takes_files() && !llm.efforts().is_empty());
        let src = d.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("notes.md"), "g104-notes-body").unwrap();
        let mut req = JsonRequest::new(PLAN_NAME, "", "一言 g104", plan_schema());
        req.files = vec![src.join("notes.md")];
        req.effort = Some("low".into());
        llm.complete_json(req).expect("答え");
        let argv = fs::read_to_string(d.join("argv.txt")).unwrap();
        assert!(argv.contains("\n--tools\nRead\n") && argv.contains("\n--restricted\n") && argv.contains("\n--effort\nlow\n"), "{argv}");
        assert_eq!(fs::read_to_string(d.join("ls.txt")).unwrap(), "notes.md\n", "写した file だけが cwd の files/ に在る");
        assert_eq!(fs::read_to_string(d.join("files.txt")).unwrap(), "g104-notes-body");
        let stdin = fs::read_to_string(d.join("stdin.txt")).unwrap();
        assert!(stdin.starts_with("一言 g104\n") && stdin.ends_with("- files/notes.md\n"), "{stdin}");
        assert!(!stdin.contains(&src.display().to_string()), "元の folder の path を model に見せない: {stdin}");

        fs::remove_file(d.join("ls.txt")).unwrap();
        llm.complete_json(JsonRequest::new(PLAN_NAME, "", "一言だけ", plan_schema())).expect("答え");
        let argv = fs::read_to_string(d.join("argv.txt")).unwrap();
        assert!(argv.contains("\n--tools\n\n--no-session-persistence\n") && !argv.contains("--restricted") && !argv.contains("--effort"), "前と同じ argv: {argv}");
        assert_eq!(fs::read_to_string(d.join("stdin.txt")).unwrap(), "一言だけ");
        assert_eq!(fs::read_to_string(d.join("ls.txt")).unwrap(), "", "file の無い呼び出しの cwd に files/ は無い");

        let old = tmp("g104-old");
        let llm = g104_cli(&old, "  --print   Print response and exit", &reply);
        assert!(!llm.takes_files() && llm.efforts().is_empty(), "--help に旗の無い CLI");
        let _ = (fs::remove_dir_all(&d), fs::remove_dir_all(&old));
    }

    /// HOME-ATTACH-1101: 添えた folder（展開した zip も）は呼び出しの dir の `files/<名前>/` に中身ごと写り、一言の後ろに中の file が 1 つずつ並ぶ。
    /// 隠し file（`.env`）・`__MACOSX`・symlink は写さず書かない。負の対照: copy_tree の隠し file の門を外す → `.env` が写って赤。
    #[test]
    fn home_attach_a_folder_reaches_the_cli_file_by_file() {
        let d = tmp("attach-folder");
        let proj = d.join("proj");
        fs::create_dir_all(proj.join("sub")).unwrap();
        fs::create_dir_all(proj.join("__MACOSX")).unwrap();
        fs::write(proj.join("a.md"), "aa").unwrap();
        fs::write(proj.join("sub/b.md"), "bb").unwrap();
        fs::write(proj.join(".env"), "SECRET=1").unwrap();
        fs::write(proj.join("__MACOSX/._a.md"), "rsrc").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", proj.join("hosts")).unwrap();
        let call = d.join("call");
        fs::create_dir_all(&call).unwrap();
        let mut req = JsonRequest::new(PLAN_NAME, "", "一言", plan_schema());
        req.files = vec![proj.clone()];
        let stdin = attach_files(&call, &req).unwrap();
        assert!(stdin.ends_with("- files/proj/a.md\n- files/proj/sub/b.md\n"), "{stdin}");
        assert_eq!(fs::read_to_string(call.join("files/proj/sub/b.md")).unwrap(), "bb");
        for gone in [".env", "__MACOSX", "hosts"] {
            assert!(!call.join("files/proj").join(gone).exists() && !stdin.contains(gone), "{gone} を写した: {stdin}");
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// G84（Model ▸）: 選んだ別名は `--model <別名>` がちょうど 1 つ（Default = 無し は w13 の試験）。別名は `--help` の `--model` の行と続きの行の `'別名'` から
    /// （claude 2.1.312 の原文）・`--fallback-model` の行や `--model` の無い版は 0。
    /// 負の対照（g-b の変異 gb_model_always）: command_line の `if let Some(m)` を外して常に `--model sonnet` → w13 が赤・gb_help: help_models の続きの行を外す → 赤。
    #[test]
    fn g84_a_chosen_model_is_one_flag_and_the_aliases_come_from_help() {
        let mut req = JsonRequest::new(PLAN_NAME, "", "一言", plan_schema());
        req.model = Some("opus".into());
        let none = child_env(None, Path::new("/nonexistent"), &crate::storage::secret_store::SecretStore::memory()).unwrap();
        let argv = command_line(Path::new("/opt/x/claude"), &req, &none, Path::new("/tmp/prompt.txt"), None);
        assert_eq!(argv.iter().filter(|a| *a == "--model").count(), 1, "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--model", "opus"]), "{argv:?}");
        let help = "  --fallback-model <model>              Enable automatic fallback to specified\n\
                    \x20                                       model(s) when the default model is\n\
                    \x20 --model <model>                       Model for the current session. Provide\n\
                    \x20                                       an alias for the latest model (e.g.\n\
                    \x20                                       'fable', 'opus', or 'sonnet') or a\n\
                    \x20                                       model's full name.\n\
                    \x20 -n, --name <name>                     Set a display name ('haiku')\n";
        let names: Vec<String> = help_models(help).into_iter().map(|(a, _)| a).collect();
        assert_eq!(names, ["fable", "opus", "sonnet"]);
        assert!(help_models("  --fallback-model <model>   'opus'\n  --print\n").is_empty(), "--model の無い版");
        assert!(help_models("").is_empty());
    }

    /// G84（operator 10-29）: model を選んだ呼び出しが CLI に断られた時の 1 行。偽の CLI の答え 3 通り:
    /// model の拒否（Claude Code の文）= `Opus isn't available on this account · choose Default in + › Model`・読めない失敗 = `Couldn't run with Opus · …`
    /// （生の文〈path〉を画面の 1 行に出さない）・plan の上限 = 今の 1 行のまま。model を選んでいない呼び出しの同じ拒否は今のまま（Malformed）。
    /// 負の対照（g-b の変異 gb_refused）: model_refused を常に false → 1 つ目が `Couldn't run with` になって赤・gb_model_err: send の map_err を外す → 赤。
    #[test]
    fn g84_a_refused_model_says_so_with_the_next_step() {
        let fail = |text: &str, status: u32| format!(r#"{{"type":"result","subtype":"success","is_error":true,"api_error_status":{status},"result":"{text}"}}"#).replace('\'', "'\\''");
        let refused = fail("There's an issue with the selected model (opus). It may not exist or you may not have access to it. Run --model to pick a different model.", 404);
        let unread = fail("boom at /Users/you/Library/secret.txt", 400);
        let limit = fail("Claude AI usage limit reached · resets 3pm", 429);
        let call = |reply: &str, model: Option<&str>| {
            let d = tmp("g84-refused");
            let llm = g104_cli(&d, "  --model <model>   Model for the current session. Provide an alias for the latest model (e.g. 'opus')", reply);
            let mut req = JsonRequest::new(PLAN_NAME, "", "一言", plan_schema());
            req.model = model.map(str::to_owned);
            let e = llm.complete_json(req).expect_err("断られた");
            let _ = fs::remove_dir_all(&d);
            e
        };
        let line = call(&refused, Some("opus")).to_string();
        assert_eq!(line, format!("Opus isn't available on this account · {MODEL_STEP}"));
        let denied = fail("There's an issue with the selected model (opus). It may not exist or you may not have access to it.", 403);
        assert_eq!(call(&denied, Some("opus")).to_string(), format!("Opus isn't available on this account · {MODEL_STEP}"), "model の 403 を sign in にした");
        let line = call(&unread, Some("opus")).to_string();
        assert_eq!(line, format!("Couldn't run with Opus · {MODEL_STEP}"));
        assert!(!line.contains("/Users") && !line.contains("boom"), "生の文を出した: {line}");
        assert!(matches!(call(&limit, Some("opus")), LlmError::PlanLimit { .. }), "plan の上限を model のせいにした");
        assert!(matches!(call(&refused, None), LlmError::Malformed { .. }), "model を選んでいない呼び出しを替えた");
    }

    /// G84（REVIEW-GB の撃つ所 8 ③）: model の失敗の log の 1 行は鍵の形の語・家の dir の path・email を伏せる。負の対照（g-b の変異 gb_log）: redact を外す → 鍵が残って赤。
    #[test]
    fn g84_the_model_failure_log_hides_keys_paths_and_email() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/you".into());
        let line = mask_for_log(&format!("bad key sk-ant-api03-g84notarealkeyAAAAAAAAAAAA at {home}/Library/x.json for me@example.com"));
        assert!(!line.contains("sk-ant") && !line.contains(&home) && !line.contains("me@example.com"), "{line}");
        assert!(line.contains("[email]"), "{line}");
    }

    /// PROVIDER-1101（owner 180 通目）: 在る CLI は全部行（login していない物は `needs_sign_in`）・動いている local LLM も行・無い物の名前は出さない・
    /// 「not supported yet」の 1 行は無い（found_rows は行だけを返す）。sign in の 1 行は Terminal で打つ command を言う。
    /// 負の対照: `needs_sign_in` を常に false にすると、login していない Cursor Agent が押せる行になって赤。
    #[test]
    fn provider1101_found_rows_list_every_ai_on_this_mac() {
        use crate::model::AiAccount::Found;
        let f = |id: &str, name: &str, needs_sign_in: bool| Found { id: id.into(), name: name.into(), needs_sign_in };
        let rows = found_rows(&["grok", "kimi", "opencode"], &["grok", "kimi", "opencode", "cursor-agent"], &["ollama", "lm-studio"]);
        assert_eq!(
            rows,
            [f("grok", "Grok", false), f("kimi", "Kimi Code", false), f("opencode", "opencode", false), f("cursor-agent", "Cursor Agent", true), f("ollama", "Ollama", false), f("lm-studio", "LM Studio", false)]
        );
        assert_eq!(found_rows(&[], &["grok"], &[]), [f("grok", "Grok", true)], "login していない Grok も行（sign in の印つき）");
        assert!(found_rows(&[], &[], &[]).is_empty(), "無い物の名前を出した");
        assert_eq!(sign_in_line("cursor-agent"), "Sign in to Cursor Agent in Terminal first · run cursor-agent login");
    }

    /// PROVIDER-1101: model の一覧はその AI の答えから（opencode の実測の 1 列・` - ` の 2 列・見出しと login の行は飛ばす）・kimi は config.toml の別名と表示名。
    /// kimi の子の設定は providers / models の表だけ（hooks を写さない）。文の答えは JSON の object を拾い、login の切れた文は sign in の 1 行。
    #[test]
    fn provider1101_model_lists_come_from_each_ai() {
        let oc = "opencode/big-pickle\nopencode/mimo-v2.5-free\n";
        assert_eq!(listed_models(oc), [("opencode/big-pickle".to_owned(), "opencode/big-pickle".to_owned()), ("opencode/mimo-v2.5-free".to_owned(), "opencode/mimo-v2.5-free".to_owned())]);
        let cur = "Available models\n\nauto - Auto (current)\ngpt-5 - GPT-5\nTip: use --model <id>:\n";
        assert_eq!(listed_models(cur), [("auto".to_owned(), "Auto".to_owned()), ("gpt-5".to_owned(), "GPT-5".to_owned())]);
        assert!(listed_models("You are logged in with grok.com.\n").is_empty(), "login の行を model にした");
        let toml = "[[hooks]]\ncommand = \"x.sh\"\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n[models.\"kimi-code/k3\"]\ndisplay_name = \"K3\"\n[models.\"kimi-code/k3-256k\"]\nmodel = \"k3\"\n[services.moonshot_search]\nkey = \"s\"\n";
        assert_eq!(kimi_models(toml), [("kimi-code/k3".to_owned(), "K3".to_owned()), ("kimi-code/k3-256k".to_owned(), "kimi-code/k3-256k".to_owned())]);
        let kept = kimi_login_config(toml);
        assert!(kept.contains("[providers.") && kept.contains("[models.\"kimi-code/k3\"]"), "{kept}");
        assert!(!kept.contains("hooks") && !kept.contains("x.sh") && !kept.contains("services"), "利用者の hook と service を写した: {kept}");
        assert_eq!(parse_text_answer("\x1b[1mThinking…\x1b[0m\n```json\n{\"seen\": \"hi\"}\n```\n", "kimi").unwrap(), serde_json::json!({ "seen": "hi" }));
        match parse_text_answer("Error: Authentication required. Run 'agent login'", "cursor-agent") {
            Err(LlmError::NoCli { detail }) => assert_eq!(detail, sign_in_line("cursor-agent")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_text_answer("I can't do that", "opencode"), Err(LlmError::Malformed { .. })));
    }

    /// PROVIDER-1101: effort と model は各 CLI の旗の形で command line に載る（Grok = `--reasoning-effort`・`-m`／opencode = `--variant`／cursor-agent = `model[effort=…]`）。
    /// kimi・opencode・cursor-agent は鍵を外し、HOME と設定の dir を `K=V` に・prompt は印（`PROMPT_ARG`）で一言そのものは無い・tool を許す旗を付けない。
    /// 負の対照: command_line の Grok の分岐を外して claude の形にすると Grok の command line に `--effort` が載って `w13_grok_command_line_…` と共に赤。
    #[test]
    fn provider1101_effort_and_model_flags_follow_each_cli() {
        let mut req = JsonRequest::new("seen", "Reply with the schema only.", "一言 p-1101-secret", json!({"type": "object"}));
        req.effort = Some("high".into());
        req.model = Some("m1".into());
        let prompt = Path::new("/var/call/prompt.txt");
        let env = [("HOME", Some("/var/call/x-home/home".to_owned()))];
        let grok = command_line(Path::new("/opt/x/grok"), &req, &[], prompt, None);
        assert!(grok.windows(2).any(|w| w == ["--reasoning-effort", "high"]) && grok.windows(2).any(|w| w == ["-m", "m1"]), "{grok:?}");
        assert!(!grok.iter().any(|a| a == "--effort" || a == "--model"), "{grok:?}");
        let oc = command_line(Path::new("/usr/local/bin/opencode"), &req, &env, prompt, None);
        assert!(oc.windows(2).any(|w| w == ["--variant", "high"]) && oc.windows(2).any(|w| w == ["-m", "m1"]) && oc.iter().any(|a| a == "--pure"), "{oc:?}");
        let cur = command_line(Path::new("/x/cursor-agent"), &req, &env, prompt, None);
        assert!(cur.windows(2).any(|w| w == ["--model", "m1[effort=high]"]) && cur.windows(2).any(|w| w == ["--mode", "ask"]) && cur.windows(2).any(|w| w == ["--workspace", "/var/call"]), "{cur:?}");
        assert!(!cur.iter().any(|a| a == "--force" || a == "--yolo" || a == "--approve-mcps"), "{cur:?}");
        let kimi = command_line(Path::new("/x/.kimi-code/bin/kimi"), &req, &env, prompt, None);
        assert!(kimi.windows(2).any(|w| w == ["-p", PROMPT_ARG]) && kimi.windows(2).any(|w| w == ["-m", "m1"]) && kimi.windows(2).any(|w| w == ["--skills-dir", "/var/call/no-skills"]), "{kimi:?}");
        assert!(!kimi.iter().any(|a| a == "--yolo" || a == "--auto" || a == "--effort"), "{kimi:?}");
        for argv in [&oc, &cur, &kimi] {
            assert!(argv.windows(2).any(|w| w == ["-u", "ANTHROPIC_API_KEY"]) && argv.iter().any(|a| a == "HOME=/var/call/x-home/home"), "{argv:?}");
            assert!(!argv.iter().any(|a| a.contains("p-1101-secret")), "一言を argv の語にした: {argv:?}");
        }
    }

    /// L4: AI を切った起動（`CLI_ENV` が空）と CLI を差し替えた run（path）は Mac の中を探さない（行も 1 行も出さない・利用者の grok を起こさない）。
    /// 負の対照: `probe_found_from` の先頭の門を外す → この Mac の grok（在る）が行か「sign in」の 1 行になって赤（grok の無い Mac では武装しない）。
    #[test]
    fn l4_no_probe_when_ai_is_off_or_the_cli_is_replaced() {
        let d = tmp("l4-noprobe");
        assert_eq!(probe_found_from(Some(OsString::new()), &d), None, "AI を切った起動で探した");
        assert_eq!(probe_found_from(Some(d.join("bin/claude").into_os_string()), &d), None, "差し替えた run で探した");
        assert!(!d.join("found-probe").exists(), "探す dir を作った");
        let _ = fs::remove_dir_all(&d);
    }

    /// 1 回だけ答える偽の local LLM（OpenAI 互換）。`models` が空なら model 0 件。受けた POST の body を返す。
    fn fake_local(models: &[&str], content: &str) -> (String, thread::JoinHandle<String>) {
        use std::io::{BufRead, BufReader};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let data: Vec<Value> = models.iter().map(|m| serde_json::json!({"id": m})).collect();
        let models_body = serde_json::json!({"data": data}).to_string();
        let chat_body = serde_json::json!({"choices": [{"message": {"content": content}}]}).to_string();
        // PROVIDER-1101: Ollama の `/api/show`（考える model か）は考えない model と答える
        let show_body = serde_json::json!({"capabilities": ["completion"]}).to_string();
        let h = thread::spawn(move || {
            let mut posted = String::new();
            for _ in 0..4 {
                let Ok((mut c, _)) = l.accept() else { break };
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() { break }
                    if let Some(v) = h.to_lowercase().strip_prefix("content-length:") { len = v.trim().parse().unwrap(); }
                }
                let chat = first.contains("/v1/chat/completions");
                let body = if first.starts_with("POST") {
                    let mut b = vec![0u8; len];
                    r.read_exact(&mut b).unwrap();
                    if chat { posted = String::from_utf8(b).unwrap(); }
                    if chat { &chat_body } else { &show_body }
                } else { &models_body };
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                if chat { break }
            }
            posted
        });
        (base, h)
    }

    /// L4: 動いている local LLM が選んだ物で次の一言を回す（鍵も login も聞かない・schema を送る・system に instructions だけ）。
    /// 負の対照: model が 0 件の LLM は使えない（`local_models` の埋め込みの filter を外すと embed だけの LLM が使えて赤）。
    /// PROVIDER-1101: model の一覧は埋め込み用でない物全部・考える model が無ければ effort は空（送らない）。
    #[test]
    fn l4_a_running_local_llm_answers_the_next_request() {
        let (base, served) = fake_local(&["nomic-embed-text", "llama3"], "{\"ok\": true}");
        let llm = LocalLlm::detect_at("ollama", &base, Duration::from_secs(5)).expect("動いている local LLM");
        assert_eq!(llm.label(), "ollama/llama3", "埋め込み model を選んだ");
        assert_eq!(llm.models(), [("llama3".to_owned(), "llama3".to_owned())]);
        assert!(llm.efforts().is_empty(), "考えない model に effort の段を出した");
        let schema = serde_json::json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
        let got = llm.complete_json(JsonRequest::new("t", "INSTR", "hello", schema)).unwrap();
        assert_eq!(got, serde_json::json!({"ok": true}));
        let posted: Value = serde_json::from_str(&served.join().unwrap()).unwrap();
        assert_eq!(posted["model"], "llama3");
        assert_eq!(posted["messages"][0]["content"], "INSTR");
        assert_eq!(posted["messages"][1]["content"], "hello");
        let (only_embed, _h) = fake_local(&["nomic-embed-text"], "{}");
        assert!(matches!(LocalLlm::detect_at("ollama", &only_embed, Duration::from_secs(5)), Err(LlmError::NoCli { .. })), "埋め込みだけの LLM が使えた");
        // 動いていない口
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(matches!(LocalLlm::detect_at("lm-studio", &format!("http://{dead}"), Duration::from_secs(2)), Err(LlmError::NoCli { .. })));
    }

    /// L4: 見つかった Grok を選んで `CLI_ENV` が grok の path なら、claude の `auth status` を打たず Grok の login の確かめだけで通る（sign in を聞かない）。
    /// 負の対照: `Found` の分岐を claude の道へ戻す（detect_from の is_grok の前の判定を外す）と claude を探して赤。
    #[test]
    fn l4_a_found_grok_is_used_without_asking_to_sign_in() {
        let d = tmp("l4-grok");
        let grok = d.join("bin/grok");
        let body = r#"case "$1" in --version) echo "grok 1.0.70 (x)";; models) echo "You are logged in with grok.com.";; esac"#;
        g99_exe(&grok, body);
        let cfg = CliConfig { account: Some(crate::model::AiAccount::Found { id: "grok".into(), name: "Grok".into(), needs_sign_in: false }), ..CliConfig::new(&d) };
        let cli = detect_from(Some(grok.clone().into_os_string()), cfg).expect("login 済みの Grok");
        assert_eq!(cli.label(), "grok/1.0.70");
        // login していない Grok は通さず、Sign in の claude の文にもしない
        let out = d.join("bin2/grok");
        g99_exe(&out, r#"case "$1" in --version) echo "grok 1.0.70";; models) echo "Not logged in";; esac"#);
        let cfg = CliConfig { account: Some(crate::model::AiAccount::Found { id: "grok".into(), name: "Grok".into(), needs_sign_in: false }), ..CliConfig::new(&d) };
        assert!(matches!(detect_from(Some(out.into_os_string()), cfg), Err(LlmError::NoCli { detail }) if detail == "Grok isn't signed in · run grok login in Terminal"));
        let _ = fs::remove_dir_all(&d);
    }

    /// G84 → PROVIDER-1101: 行にする CLI は PATH の dir と install dir に実行できる file が在る物だけ（表の順）・起こさない（偽の CLI は起きたら印を書く）・
    /// 実行できない file と見つからない物の名前は出さない。
    /// 負の対照（g-b の変異 gb_other）: clis_in の `!…is_empty()` を外す → 見つからない物まで出て赤。
    #[test]
    fn g84_found_clis_are_found_by_file_and_never_run() {
        let d = tmp("g84-other");
        let (bin, home) = (d.join("bin"), d.join("home"));
        let mark = d.join("ran.txt");
        let body = format!("echo ran >> '{}'", mark.display());
        g99_exe(&bin.join("grok"), &body);
        g99_exe(&home.join(".local/bin/cursor-agent"), &body);
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("kimi"), "not executable").unwrap();
        // 一時 dir だけを見る（install_dirs の /usr/local/bin 等はこの Mac の物なので渡さない・反証 REVIEW-GB / operator 10-29）
        let found = clis_in(&FOUND_CLIS.map(|c| c.0), &bin.display().to_string(), &[home.join(".local/bin")]);
        assert_eq!(found, ["grok", "cursor-agent"], "実行できない kimi を数えた・見つからない物を出した");
        assert!(clis_in(&FOUND_CLIS.map(|c| c.0), &d.join("none").display().to_string(), &[d.join("nohome")]).is_empty());
        // L7-ORCA #12: panel の agent の行も同じ見方（表の順・入っている物だけ・ICONS 以降 kimi も AGENTS に在るが実行できない file なので出ない）
        let agents = clis_in(&crate::domain::terminal::AGENTS, &bin.display().to_string(), &[home.join(".local/bin")]);
        assert_eq!(agents, ["grok", "cursor-agent"], "入っていない agent を出した");
        assert!(install_dirs(&home).contains(&home.join(".local/bin")), "製品の install dir に ~/.local/bin が無い");
        assert!(!mark.exists(), "見つけるために CLI を起こした");
        let _ = fs::remove_dir_all(&d);
    }

    /// RQ-N: sign in の偽の CLI（`auth login` の引数と env を隣の file に書き、`exit.txt` の数で終わる・`sleep` なら 30 秒眠る）。
    fn rqn_fake_login(dir: &Path) -> PathBuf {
        let p = dir.join("bin").join("claude");
        g99_exe(&p, r#"here=$(dirname "$0"); printf '%s\n' "$@" > "$here/argv.txt"; env > "$here/env.txt"; x=$(cat "$here/exit.txt" 2>/dev/null); [ "$x" = sleep ] && sleep 30; exit ${x:-0}"#);
        p
    }

    /// 偽の CLI で `sign_in` する口（`add_plan_account` / `reauthenticate` の引数・本物の `claude auth login` を起こさない）。
    fn rqn_sign_in(fake: &Path, wait: Duration) -> impl Fn(&[(&str, Option<String>)]) -> Result<(), String> + '_ {
        move |env| sign_in_from(Some(fake.as_os_str().to_owned()), env, wait, &std::sync::atomic::AtomicBool::new(false))
    }

    /// RQ-ACCT（RQ-N）: Add account は偽の CLI の `auth login --claudeai` を、足した account の dir（`CLAUDE_CONFIG_DIR`）で 1 回起こし、鍵の変数と env の token は渡さない。
    /// 列に在る id も、前の Add が残した dir の id も使わない。sign in が済まなければ dir を消して何も返さない（作りかけの account を残さない）。
    /// 負の対照（commit message）: Err の腕の `remove_dir_all` を外す → plan-4 の dir が残って赤・`child_command` の env の当て直しを外す → CLAUDE_CONFIG_DIR が無くて赤。
    #[test]
    fn rqn_add_account_signs_in_with_its_own_config_dir() {
        use crate::model::AiAccount;
        let d = tmp("rqn-add");
        let fake = rqn_fake_login(&d);
        let bin = d.join("bin");
        let root = d.join(ACCOUNTS_DIR);
        fs::create_dir_all(root.join("plan-2")).unwrap();
        let plan1 = AiAccount::Plan { id: "plan-1".into() };
        let got = add_plan_account(&d, &[plan1.clone()], rqn_sign_in(&fake, Duration::from_secs(20))).expect("偽の CLI の sign in");
        assert_eq!(got, AiAccount::Plan { id: "plan-3".into() }, "列の id か残った dir の id を使った");
        let argv = fs::read_to_string(bin.join("argv.txt")).expect("偽の CLI が呼ばれていない");
        assert_eq!(argv.lines().collect::<Vec<_>>(), ["auth", "login", "--claudeai"]);
        let env = fs::read_to_string(bin.join("env.txt")).unwrap();
        let own = format!("CLAUDE_CONFIG_DIR={}", root.join("plan-3").display());
        assert!(env.lines().any(|l| l == own), "{own} で起きていない");
        assert!(!env.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=") || l.starts_with("CLAUDE_CODE_OAUTH_TOKEN=")), "鍵か token が sign in に渡った");
        assert!(root.join("plan-3").is_dir());
        fs::write(bin.join("exit.txt"), "1").unwrap();
        let e = add_plan_account(&d, &[plan1, got], rqn_sign_in(&fake, Duration::from_secs(20))).expect_err("済まない sign in");
        assert_eq!(e, SIGN_IN_FAILED);
        assert_eq!(next_step(&e), Some(SIGN_IN_STEP));
        assert!(!root.join("plan-4").exists(), "済まない Add の dir が残った");
        assert_eq!(sign_in_from(Some(OsString::new()), &[], Duration::from_secs(1), &std::sync::atomic::AtomicBool::new(false)), Err("AI is off for this launch".into()), "AI を消した起動で sign in を起こした");
        let _ = fs::remove_dir_all(&d);
    }

    /// REVIEW-RQN（operator 4）: plan の account の Remove は dir を消す前に、その dir（`CLAUDE_CONFIG_DIR`・`CLAUDE_SECURESTORAGE_CONFIG_DIR` は外す）で偽の CLI の
    /// `auth logout` を 1 回起こす。logout が済まなくても dir は消す。その account の dir の無い env（System default）では CLI を起こさない（利用者の主の login）。
    /// 消した後の Add は消した id を使い回さない。鍵の account の Remove は logout を起こさない。本物の `claude auth logout` は起こさない（偽の CLI だけ）。
    /// 負の対照（commit message）: remove_account の `sign_out` の呼び出しを外す → 回数 0 で赤・
    /// add_plan_account の `last` を 0 に戻す → 消した plan-1 を使い回して赤・child_env の CLAUDE_SECURESTORAGE_CONFIG_DIR を外す行を消す → logout が起きず赤。
    #[test]
    fn rqn_remove_signs_out_its_own_dir_first() {
        use crate::storage::secret_store::SecretStore;
        let d = tmp("rqn-remove");
        let fake = rqn_fake_login(&d);
        let bin = d.join("bin");
        let root = d.join(ACCOUNTS_DIR);
        let plan = add_plan_account(&d, &[], rqn_sign_in(&fake, Duration::from_secs(20))).expect("偽の CLI の sign in");
        let own = root.join(plan.id());
        fs::write(own.join(".claude.json"), "{}").unwrap();
        let calls = std::cell::Cell::new(0);
        let out = |env: &[(&str, Option<String>)]| {
            calls.set(calls.get() + 1);
            assert!(own.is_dir(), "dir を消してから logout を起こした");
            sign_out_from(Some(fake.as_os_str().to_owned()), env)
        };
        remove_account(&d, &plan, &SecretStore::memory(), out).expect("Remove");
        assert_eq!(calls.get(), 1, "logout を 1 回起こしていない");
        assert_eq!(fs::read_to_string(bin.join("argv.txt")).unwrap().lines().collect::<Vec<_>>(), ["auth", "logout"]);
        let env = fs::read_to_string(bin.join("env.txt")).unwrap();
        assert!(env.lines().any(|l| l == format!("CLAUDE_CONFIG_DIR={}", own.display())), "その account の dir で logout していない");
        assert!(!env.lines().any(|l| l.starts_with("CLAUDE_SECURESTORAGE_CONFIG_DIR=")), "Keychain の名前を変える変数が logout に渡った");
        assert!(!own.exists(), "dir が残った");
        let again = add_plan_account(&d, &[], rqn_sign_in(&fake, Duration::from_secs(20))).expect("2 つ目");
        assert_ne!(again, plan, "消した account の id を使い回した");
        // logout が済まなくても dir は消す
        fs::write(bin.join("exit.txt"), "1").unwrap();
        remove_account(&d, &again, &SecretStore::memory(), |env| sign_out_from(Some(fake.as_os_str().to_owned()), env)).expect("logout が済まなくても消す");
        assert!(!root.join(again.id()).exists());
        // 鍵の account の Remove は logout を起こさない
        let keys = SecretStore::memory();
        let key = add_key_account("sk-ant-test-not-a-real-key-r4q4", &keys, &[]).unwrap();
        remove_account(&d, &key, &keys, |_| -> Result<(), String> { panic!("鍵の Remove で logout を起こした") }).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    /// REVIEW-RQN 6（REVIEW-RQN の攻撃 rqn_atk_removing_a_plan_logs_it_out_first_and_its_id_is_not_reused）: last-plan の前から在った account
    /// （04047f8 までの build で Add した形 = ai-accounts/plan-1 だけ・last-plan 無し）を消しても、次の Add はその id に戻らない。
    /// 負の対照（commit message）: remove_account の last-plan に残す行を外す → plan-1 を使い回して赤。
    #[test]
    fn rqn_removing_a_plan_from_before_the_counter_does_not_reuse_its_id() {
        use crate::storage::secret_store::SecretStore;
        let d = tmp("rqn-old-plan");
        let fake = rqn_fake_login(&d);
        let root = d.join(ACCOUNTS_DIR);
        fs::create_dir_all(root.join("plan-1")).unwrap();
        let old = crate::model::AiAccount::Plan { id: "plan-1".into() };
        remove_account(&d, &old, &SecretStore::memory(), |_| Ok(())).expect("Remove");
        assert!(!root.join("plan-1").exists(), "dir が残った");
        let again = add_plan_account(&d, &[], rqn_sign_in(&fake, Duration::from_secs(20))).expect("Add");
        assert_ne!(again.id(), "plan-1", "消した account の id を Add が使い回した");
        let _ = fs::remove_dir_all(&d);
    }

    /// REVIEW-RQN（operator 4）: `sign_out_from` は足した account の dir（ai-accounts の下の CLAUDE_CONFIG_DIR）で、Keychain の名前を変える
    /// CLAUDE_SECURESTORAGE_CONFIG_DIR を外した env の時だけ偽の CLI を起こす。System default（dir 無し）・ai-accounts の外の dir・外していない env は起こさない
    /// （利用者の主の login を消さない）。負の対照（commit message）: 門を丸ごと外す → System default で赤・dir の確かめを「dir が在る」だけにする → 外の dir で赤・
    /// 外したかの確かめを消す → 外していない env で赤。
    #[test]
    fn rqn_sign_out_runs_only_for_an_added_account_dir() {
        use crate::storage::secret_store::SecretStore;
        let d = tmp("rqn-signout-gate");
        let fake = rqn_fake_login(&d);
        let argv = d.join("bin").join("argv.txt");
        let run = |env: &[(&str, Option<String>)]| {
            let _ = fs::remove_file(&argv);
            let r = sign_out_from(Some(fake.as_os_str().to_owned()), env);
            (r.is_ok(), argv.exists())
        };
        let added = d.join(ACCOUNTS_DIR).join("plan-1").display().to_string();
        assert_eq!(run(&[("CLAUDE_SECURESTORAGE_CONFIG_DIR", None), ("CLAUDE_CONFIG_DIR", Some(added.clone()))]), (true, true), "足した account の dir で起きない");
        let main = child_env(None, &d, &SecretStore::memory()).unwrap();
        assert_eq!(run(&main), (false, false), "System default（利用者の主の login）で logout を起こした");
        let outside = d.join(".claude").display().to_string();
        assert_eq!(run(&[("CLAUDE_SECURESTORAGE_CONFIG_DIR", None), ("CLAUDE_CONFIG_DIR", Some(outside))]), (false, false), "ai-accounts の外の dir で logout を起こした");
        assert_eq!(run(&[("CLAUDE_CONFIG_DIR", Some(added))]), (false, false), "CLAUDE_SECURESTORAGE_CONFIG_DIR を外していない env で logout を起こした");
        let _ = fs::remove_dir_all(&d);
    }

    /// RQ-ACCT（RQ-N）: Re-authenticate は その account の dir で偽の CLI の sign in を起こし直す。失敗・時間切れでも dir と中の file はそのまま（前の login を Shibubu は消さない）。
    /// 鍵の account は CLI を起こさず `Paste API key instead`・dir の消えた plan は理由の 1 行・Home の Sign in（None = System default）は ai-accounts を指さない。
    /// 負の対照（commit message）: `reauthenticate` の消えた dir の門を外す → plan-9 で CLI を起こして赤。
    #[test]
    fn rqn_reauthenticate_keeps_the_login_when_it_fails() {
        use crate::model::AiAccount;
        let d = tmp("rqn-reauth");
        let fake = rqn_fake_login(&d);
        let bin = d.join("bin");
        let dir = d.join(ACCOUNTS_DIR).join("plan-1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".credentials.json"), "before").unwrap();
        let plan = AiAccount::Plan { id: "plan-1".into() };
        let env_of = || fs::read_to_string(bin.join("env.txt")).unwrap_or_default();
        fs::write(bin.join("exit.txt"), "1").unwrap();
        assert_eq!(reauthenticate(&d, Some(&plan), rqn_sign_in(&fake, Duration::from_secs(20))), Err(SIGN_IN_FAILED.into()));
        assert!(env_of().lines().any(|l| l == format!("CLAUDE_CONFIG_DIR={}", dir.display())), "その account の dir で起きていない");
        assert_eq!(fs::read_to_string(dir.join(".credentials.json")).unwrap(), "before", "失敗した Re-authenticate が前の login に触った");
        fs::write(bin.join("exit.txt"), "sleep").unwrap();
        let t = Instant::now();
        assert_eq!(reauthenticate(&d, Some(&plan), rqn_sign_in(&fake, Duration::from_millis(800))), Err(SIGN_IN_FAILED.into()));
        assert!(t.elapsed() < Duration::from_secs(15) && dir.is_dir(), "時間切れで待ち続けたか dir を消した");
        fs::write(bin.join("exit.txt"), "0").unwrap();
        assert_eq!(reauthenticate(&d, Some(&plan), rqn_sign_in(&fake, Duration::from_secs(20))), Ok(()));
        fs::remove_file(bin.join("env.txt")).unwrap();
        let key = AiAccount::Key { id: "key-1".into(), last4: "a1b2".into() };
        assert_eq!(reauthenticate(&d, Some(&key), rqn_sign_in(&fake, Duration::from_secs(20))), Err("Paste API key instead".into()));
        let gone = reauthenticate(&d, Some(&AiAccount::Plan { id: "plan-9".into() }), rqn_sign_in(&fake, Duration::from_secs(20))).expect_err("消えた dir");
        assert!(gone.contains("Remove it"), "{gone}");
        assert!(reauthenticate(&d, Some(&AiAccount::Plan { id: "../x".into() }), rqn_sign_in(&fake, Duration::from_secs(20))).is_err());
        assert!(!bin.join("env.txt").exists(), "鍵・消えた dir・外を指す id で CLI を起こした");
        assert_eq!(reauthenticate(&d, None, rqn_sign_in(&fake, Duration::from_secs(20))), Ok(()));
        assert!(!env_of().contains(ACCOUNTS_DIR), "System default の Sign in が足した account の dir を指した");
        let _ = fs::remove_dir_all(&d);
    }

    /// RQ-ACCT（REVIEW-RQN・operator 1）: 止める口が立つと、待つ上限（20 秒）を待たずに子を止めて `SIGN_IN_FAILED`。
    /// 負の対照（commit message）: `sign_in_from` の loop の `stop` の条件を外す → 20 秒待って赤（5 秒の上限）。
    #[test]
    fn rqn_sign_in_stops_when_asked() {
        use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
        let d = tmp("rqn-stop");
        let fake = rqn_fake_login(&d);
        fs::write(d.join("bin").join("exit.txt"), "sleep").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let t = Instant::now();
        let waiter = thread::spawn(move || sign_in_from(Some(fake.as_os_str().to_owned()), &[], Duration::from_secs(20), &flag));
        thread::sleep(Duration::from_millis(400));
        stop.store(true, Ordering::SeqCst);
        assert_eq!(waiter.join().unwrap(), Err(SIGN_IN_FAILED.into()));
        assert!(t.elapsed() < Duration::from_secs(5), "止めても待ち続けた: {:?}", t.elapsed());
        let _ = fs::remove_dir_all(&d);
    }

    /// RQ-ACCT: 足した account の行の email は Claude Code が置いた `<dir>/.claude.json` の `oauthAccount.emailAddress` から（画面だけ）。無い・外を指す id は None。
    #[test]
    fn rqn_account_email_reads_the_accounts_own_config() {
        let d = tmp("rqn-email");
        let dir = d.join(ACCOUNTS_DIR).join("plan-1");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(account_email(&d, "plan-1"), None);
        fs::write(dir.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":" you@example.org ","accountUuid":"x"}}"#).unwrap();
        assert_eq!(account_email(&d, "plan-1").as_deref(), Some("you@example.org"));
        assert_eq!(account_email(&d, "../plan-1"), None);
        let _ = fs::remove_dir_all(&d);
    }

    /// RQ-API（RQ-N）: 鍵の account の呼び出しの失敗は鍵と請求の言葉（401 = 鍵を貼り直す `Auth`・Console の残高と使用量の上限 = `Quota` と Console の次の一手）。
    /// 同じ答えを plan の account が受けたら今のまま（login し直し・時間で戻る上限）。偽の CLI・偽の鍵の無い env（`EnvKey` は Keychain を読まない）。
    /// REVIEW-RQN（operator 2）: 「limit」の語でなく API の status と型で分ける — 文脈の長さ（400 の context limit・413）は短くするか新しい chat の 1 行（Console・plan・resets を言わない・
    /// plan の account も同じ）・429 の混雑は待つ 1 行（Console を言わない）・Console は残高と請求（402・billing_error・429 の spend limit の code）だけ。
    /// 負の対照（commit message）: `parse_answer` の `if key` の腕を外す → 401 が Sign in の 1 行で赤・`too_long` の腕を外す → context limit が plan の上限 / Console で赤。
    #[test]
    fn rqn_key_account_failures_say_key_and_billing() {
        use crate::model::AiAccount;
        let too_long = "too long for Claude";
        let answers = [
            (401, "Invalid API key · Please run /login".to_string(), "key was refused"),
            (403, "Permission denied".into(), "key was refused"),
            (400, "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.".into(), "Anthropic Console"),
            (400, "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC.".into(), "Anthropic Console"),
            (402, r#"API Error: 402 {"type":"error","error":{"type":"billing_error","message":"There is an issue with your billing"}}"#.into(), "Anthropic Console"),
            (429, r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"Your workspace has reached its spend limit","details":{"error_code":"enforced_spend_limit_reached"}}}"#.into(), "Anthropic Console"),
            (400, "input length and `max_tokens` exceed context limit: 190000 + 32000 > 200000, decrease input length or `max_tokens` and try again".into(), too_long),
            (400, "Prompt is too long".into(), too_long),
            (413, r#"API Error: 413 {"type":"error","error":{"type":"request_too_large","message":"Request exceeds the maximum allowed number of bytes."}}"#.into(), too_long),
            (429, r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed the rate limit for your organization of 50,000 input tokens per minute."}}"#.into(), "busy"),
        ];
        for (status, text, want) in &answers {
            let raw = json!({"type":"result","subtype":"success","is_error":true,"api_error_status":status,"result":text}).to_string();
            let d = tmp("rqn-key-err");
            let program = fake_cli(&d, PLAN, &raw);
            let key = CliLlm { program: program.clone(), version: "9.9.1".into(), config: CliConfig { account: Some(AiAccount::EnvKey), ..CliConfig::new(&d) }, flags: HelpFlags::default(), usage: false };
            let e = key.complete_json(JsonRequest::new(EDIT_NAME, "", "x", edit_schema())).expect_err(text);
            let line = e.to_string();
            assert!(line.contains(want), "{status} {text}: {e:?} / {line}");
            assert!(next_step(&line).is_none(), "鍵の account に Sign in の button を出す: {line}");
            if *want != "Anthropic Console" {
                assert!(!["Console", "add credit", "plan", "resets"].iter().any(|w| line.contains(w)), "{status} {text}: {line}");
            }
            let plan = CliLlm { program, version: "9.9.1".into(), config: CliConfig::new(&d), flags: HelpFlags::default(), usage: false };
            let pe = plan.complete_json(JsonRequest::new(EDIT_NAME, "", "x", edit_schema())).expect_err(text);
            assert!(!matches!(pe, LlmError::Auth { .. } | LlmError::Quota { .. }), "plan の account の答えまで鍵の言葉にした: {pe:?}");
            if *want == too_long {
                assert!(pe.to_string().contains(too_long), "plan の account の文脈の長さを上限にした: {pe:?} / {pe}");
            }
            let _ = fs::remove_dir_all(&d);
        }
    }

    /// RQ-USAGE（RQ-N）: 使用量の 1 行の隣の button（3 つの場面 = 取れない → `Try again`・未 login / 断られた → `Sign in`・上限 → button 無しで resets in）。
    /// 読み過ぎ（429）と鍵の account は button を出さない（鍵の account は Keychain も usage の口も読まない）。1 行に次の一手を重ねて書かない。
    /// 負の対照（commit message）: `usage_step` の NoCli の腕を外す（全部 Try again）→ Sign in の場面で赤・429 の腕を外す → 赤。
    #[test]
    fn rqn_usage_step_is_try_again_or_sign_in() {
        use crate::llm::{usage_line, PlanUsage, KEYCHAIN_UNREADABLE};
        let ok = |pct| Ok(PlanUsage { windows: vec![("Session".into(), pct, Some(600))], checked_at: 0 });
        let cases: Vec<(Result<PlanUsage, LlmError>, Option<&str>)> = vec![
            (ok(42), None),
            (ok(100), None),
            (Ok(PlanUsage { windows: vec![], checked_at: 0 }), Some(TRY_AGAIN)),
            (Err(LlmError::Network { detail: "offline".into() }), Some(TRY_AGAIN)),
            (Err(LlmError::Timeout { ms: 10_000 }), Some(TRY_AGAIN)),
            (Err(LlmError::Server { status: 503 }), Some(TRY_AGAIN)),
            (Err(no_cli(KEYCHAIN_UNREADABLE)), Some(TRY_AGAIN)),
            (Err(no_cli(NOT_SIGNED_IN)), Some(SIGN_IN_STEP)),
            (Err(no_cli(SIGNED_OUT)), Some(SIGN_IN_STEP)),
            (Err(LlmError::RateLimited { retry_after: Some(120) }), None),
            (Err(no_cli("API key · billed per use")), None),
        ];
        for (u, want) in &cases {
            assert_eq!(usage_step(u), *want, "{u:?}");
            let line = usage_line(u, 0);
            assert!(!line.contains("Try again") && !line.contains("Refresh"), "1 行に次の一手を重ねた: {line}");
        }
        assert_eq!(usage_line(&ok(100), 0), "Session limit reached · resets in 10m");
        let d = tmp("rqn-usage-key");
        let key = CliLlm { program: d.join("never-run"), version: "9.9.1".into(), config: CliConfig { account: Some(crate::model::AiAccount::EnvKey), ..CliConfig::new(&d) }, flags: HelpFlags::default(), usage: true };
        assert_eq!(usage_step(&crate::llm::Llm::plan_usage(&key)), None, "鍵の account に使用量の button を出した");
        let _ = fs::remove_dir_all(&d);
    }

    /// REVIEW-RQN（operator 3・録画の前に必須）: `CLI_ENV` に偽の CLI を置いた run の Show usage（`plan_usage`）は本物の Keychain と `USAGE_URL` を読まない
    /// （`detect` が差し替えを覚える・process の env は書き換えない）。1 行は `USAGE_NOT_READ`・button 無し（帯は出さない）。
    /// 負の対照（commit message）: `plan_usage` の `if !self.usage` を外す → Keychain を引いて NOT_SIGNED_IN / KEYCHAIN_UNREADABLE になり赤
    /// （tmp の dir の Keychain の名前は無いので確認は出ない）。
    #[test]
    fn rqn_replaced_cli_does_not_read_usage() {
        use crate::model::AiAccount;
        let d = tmp("rqn-usage-fake");
        let program = fake_cli(&d, PLAN, "{}");
        let cfg = CliConfig { account: Some(AiAccount::Plan { id: "plan-1".into() }), ..CliConfig::new(&d) };
        let llm = detect_from(Some(program.into_os_string()), cfg).expect("偽の CLI");
        let u = crate::llm::Llm::plan_usage(&llm);
        assert!(matches!(&u, Err(LlmError::NoCli { detail }) if detail == USAGE_NOT_READ), "差し替えた run で使用量の口を開けた: {u:?}");
        assert_eq!(usage_step(&u), None, "差し替えた run に button を出した");
        let _ = fs::remove_dir_all(&d);
    }

    /// ACCOUNTS-1101 2 段目（interface・呼ぶ側の形）: 名前 = 付けた名前 → email の @ の前（email そのものは出さない）→ 無ければ `<AI> <n+1>`。
    /// 負の対照（commit message）: email をそのまま返す → `@` の assert で赤・`n + 1` を `n` にする → `Claude 2` で赤。
    #[test]
    fn accounts2_names_prefer_the_given_name_then_the_email_local_part() {
        use crate::model::AiAccount;
        let plan = AiAccount::Plan { id: "plan-1".into() };
        let grok = AiAccount::Plan { id: "grok-3".into() };
        assert_eq!(account_name(&plan, Some("work"), Some("ryo@example.com")).as_deref(), Some("work"));
        assert_eq!(account_name(&plan, None, Some("ryo@example.com")).as_deref(), Some("ryo"));
        assert_eq!(account_name(&plan, Some("  "), None), None, "空白の名前を名前にした");
        assert_eq!(account_name(&plan, None, Some("@example.com")), None, "@ の前が空でも名前にした");
        assert!(account_name(&plan, None, Some("ryo@example.com")).is_some_and(|n| !n.contains('@')), "email を画面の名前にした");
        assert_eq!(account_name(&AiAccount::EnvKey, Some("x"), None), None, "足した account でない行に名前を付けた");
        assert_eq!((numbered_name(&plan).as_deref(), numbered_name(&grok).as_deref()), (Some("Claude 2"), Some("Grok 4")));
        assert_eq!(numbered_name(&AiAccount::Found { id: "grok".into(), name: "Grok".into(), needs_sign_in: false }), None);
    }

    /// ACCOUNTS-1101: 足した account の sign in の 1 行は、置き場から作り直した字と一致する時だけ打つ（継ぎ足し・別の dir・別の変数・前に足した変数は None）。
    /// 書いたが未実行（owner 186 / 187）。負の対照: `account_prefix` の `env_prefix(..) == Some(prefix)` を外す → `PATH` に替えた行が通って赤。
    #[test]
    fn accounts1101_sign_in_line_round_trips_and_rejects_tampering() {
        let d = std::env::temp_dir().join(format!("shibubu-acct-{}", std::process::id()));
        let line = account_sign_in_line(&d, &crate::model::AiAccount::Plan { id: "grok-2".into() }).expect("grok の account の 1 行");
        assert!(line.starts_with("env GROK_HOME='") && line.ends_with(" grok login"), "{line}");
        assert!(sign_in_command(&line).is_some_and(|c| c.starts_with("env GROK_HOME='") && c.ends_with(" login")), "作った 1 行を打たない: {line}");
        for bad in [format!("{line}; rm -rf ~"), line.replace("grok-2", "grok-x"), line.replace(ACCOUNTS_DIR, "elsewhere"), line.replace("GROK_HOME", "PATH"), format!("env X='1' {line}")] {
            assert_eq!(sign_in_command(&bad), None, "{bad}");
        }
        assert_eq!(account_sign_in_line(&d, &crate::model::AiAccount::Plan { id: "plan-1".into() }), None, "Claude の plan は browser の sign in");
        assert_eq!(crate::model::AiAccount::Plan { id: "grok-2".into() }.provider(), "grok");
        assert_eq!(one_account_line("cursor-agent").as_deref(), Some("Cursor Agent keeps one sign-in on this Mac · Shibubu uses that one"));
        assert_eq!(one_account_line("grok"), None);
    }
}
