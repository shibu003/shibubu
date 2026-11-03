//! LLM provider（composer と Agent の chat bar の裏）。TODO.md GEN-012: model は optional・timeout / retry / 利用上限・
//! 構造化 JSON・実行前の検証。呼び手は `complete_json` に schema つきの依頼を渡し、その schema で検証済みの JSON だけを
//! 受け取る（enum に無い action id ＝幻覚は `LlmError::Invalid` で落ちる）。鍵が無い・失敗した時に呼び手の local heuristics へ
//! 落とすのは `complete_or_local`（heuristics 自体は呼び手 = composer の持ち場）。
//! lib target が無いので tests/llm.rs は `#[path]` でこの file を取り込む。provider（anthropic / openai / 試験の fake）はこの file の末尾の module。
//! API の事実と出典（2026-10-17 取得）は各 provider の module の先頭と docs/internal/connectors/anthropic-chatbar.md。
//! 製品（Shibubu.app）は既定で鍵を持たない（YC W13）: 製品の provider は利用者が login 済みの CLI を PTY で起こす `crate::llm_cli::CliLlm`。
//! RQ2（YC_DECISIONS 89 通目で「鍵を持たない」を撤回）: 利用者が Settings › AI & Models で API key を選んだ時だけ、同じ CLI の子に鍵を渡す（`llm_cli::child_env`・既定は plan の login のまま）
//! （PTY を使うので、この directory の外に置く。この file は `#[path]` で単独に取り込まれる）。鍵の provider（`from_env`）は bin の kangae-codegen だけ。
#![allow(dead_code)]

use serde_json::Value;
use std::{fmt, io::{self, Read}, process::Command, sync::atomic::AtomicBool, thread, time::Duration};

/// 依頼 1 件。schema は Anthropic（output_config.format）と OpenAI（strict な json_schema）の両方が受ける形に限る
/// （どちらに送るかは実行時の鍵で決まるので、片方でしか通らない schema は作った人の手元でだけ動く）:
/// root は type: object、object は全部 additionalProperties: false で全 property を required（任意の値は type に "null" を足す）、
/// keyword は type / properties / required / additionalProperties / items / enum / const / anyOf と注釈の description / title だけ
/// （minLength / minimum / minItems 等は Anthropic が 400 を返す。数や長さは返った値を呼び手が確かめる）、anyOf と type の配列は
/// 合わせて 16 箇所まで（Anthropic の上限）。外れると送る前に `LlmError::Schema`
#[derive(Clone, Debug, PartialEq)]
pub struct JsonRequest {
    /// schema の名前（OpenAI の text.format.name。英数字と _ - の 64 文字以内）
    pub name: String,
    /// system prompt
    pub instructions: String,
    /// 利用者の言葉と文脈
    pub input: String,
    pub schema: Value,
    /// thinking（Anthropic）・reasoning（OpenAI）も含む出力の上限。超えると途中で切れて `Malformed`
    pub max_tokens: u32,
    /// G104: 一言に添えた file（絶対 path・`domain::chat::attach` を通った物）。読めるのは `Llm::takes_files` が true の provider だけ
    pub files: Vec<std::path::PathBuf>,
    /// G104: 考える深さ（`domain::chat::Effort::flag` の語）。None = provider の既定。渡すのは `Llm::efforts` がその段を持つ provider だけ（flag の形は provider が決める）
    pub effort: Option<String>,
    /// G84: model の語（`Llm::models` の渡す語）。None = provider の既定（flag を付けない）。渡すのは `Llm::models` にその語が在る provider だけ
    pub model: Option<String>,
    /// USAGE-OWN-1101: 頼んだ Project の id（呼び出しの記録 `llm_cli::CallRecord::project` へ・model には送らない）。None = Home・作る前の案
    pub project: Option<String>,
}
impl JsonRequest {
    pub fn new(name: &str, instructions: &str, input: &str, schema: Value) -> Self {
        Self { name: name.into(), instructions: instructions.into(), input: input.into(), schema, max_tokens: 8192, files: Vec::new(), effort: None, model: None, project: None }
    }
}

/// provider の口。provider は `send` だけを書き、呼び手は `complete_json` を呼ぶ（schema の検査と返答の検証を飛ばせない）
pub trait Llm: Send + Sync {
    /// "anthropic/claude-sonnet-5-5" の形。生成の記録（GEN-010 の model / version）に残す
    fn label(&self) -> String;
    /// 送って、model が書いた JSON を返す（未検証）。schema は検査済み
    fn send(&self, req: &JsonRequest) -> Result<Value, LlmError>;
    /// G104: `JsonRequest::files`（写真・文字・PDF）を model に読ませられるか。既定 false（鍵の provider と試験の fake は読まない）
    fn takes_files(&self) -> bool {
        false
    }
    /// PROVIDER-1101: 画面の名前（`Claude Code`・`Grok`・`Ollama` …）。`Accepts::provider` と押せない理由の語。既定は名前を持たない provider（鍵の provider・試験の fake）
    fn provider_name(&self) -> String {
        "This AI".into()
    }
    /// G104 → PROVIDER-1101: `JsonRequest::effort` で渡せる段（その AI の flag が受ける物・Claude Code `--effort`・Grok `--reasoning-effort` …）。既定は空（選ばせない）。
    /// 前の `takes_effort`（`--effort` の有無だけ）を替えた = Grok・Ollama で effort を選べなかった（owner 180 通目）。
    /// 型が `domain::chat::Effort` でなく語（`Effort::flag`）なのは、この module を `#[path]` で取り込む crate（bin kangae-codegen・tests/llm.rs・codegen.rs）が domain を持たないから
    /// （`JsonRequest::effort` も同じ語）。画面の段に戻すのは呼ぶ側（`Effort::from_flag`）
    fn efforts(&self) -> Vec<&'static str> {
        Vec::new()
    }
    /// G84 → PROVIDER-1101: `JsonRequest::model` で渡せる model（渡す語, menu の語）= その AI から読んだ一覧（Claude Code = `--help` の別名・Grok = `grok models`・
    /// local LLM = `/v1/models`）。既定は空（鍵の provider と試験の fake は選ばせない）。前の `u8`（Claude Code の固定の表の bit）を替えた
    fn models(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn complete_json(&self, req: JsonRequest) -> Result<Value, LlmError> {
        check_request(&req)?;
        let v = self.send(&req)?;
        validate(&req.schema, &v)?;
        Ok(v)
    }
    /// FX2（FX-005）: `send` の止められる形。`cancel` は呼ぶ側の 1 つの job（Home の生成 1 回）だけの印で、立てたらその呼び出しの子を止める
    /// （runtime の `Arc<AtomicBool>` の cancel と同じ形・provider ごとに 1 つの「今の呼び出し」を持たない = Engine が同じ `Arc<dyn Llm>` で読んでいる呼び出しを巻き込まない）。
    /// 使い方（provider の側）: 子を起こす provider（`llm_cli::CliLlm`）と包む物（`Budgeted`）だけが上書きする。
    /// 保証する（既定の本体）: 送る前に `cancel` が立っていれば送らず `LlmError::Cancelled { after_ms: 0 }`・立っていなければ `send` と同じ。
    /// 保証しない（既定の本体）: 送った後に止める事（鍵の provider と試験の Fake は 1 回の呼び出しを途中で止める口を持たない = 返るまで待つ）。
    /// Wave: FX2
    fn send_cancellable(&self, req: &JsonRequest, cancel: &AtomicBool) -> Result<Value, LlmError> {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(LlmError::Cancelled { after_ms: 0 });
        }
        self.send(req)
    }
    /// FX2: `complete_json` の止められる形（送る前の schema の検査と返った JSON の検証は同じ・送るのは `send_cancellable`）。
    /// 例（generated.rs の `LlmModel` が job の印を持つ時）: `llm.complete_json_cancellable(JsonRequest::new(name, LLM_INSTRUCTIONS, prompt, schema), &cancel)`
    /// 保証する: 止めた時は `Err(LlmError::Cancelled { .. })` だけを返す（Timeout・Malformed にしない = 呼ぶ側が名前の表で作り直さない）。
    /// 印が立っていなければ `complete_json` と同じ（`send_cancellable` の既定と provider の上書きは、立たない印で `send` と同じ）。
    /// Wave: FX2
    fn complete_json_cancellable(&self, req: JsonRequest, cancel: &AtomicBool) -> Result<Value, LlmError> {
        check_request(&req)?;
        let v = self.send_cancellable(&req, cancel)?;
        validate(&req.schema, &v)?;
        Ok(v)
    }
    /// RQ2（RQ-USAGE）: 利用者の plan の今の使用量（Session = 5 時間・Weekly・model 別の週）を読む。model を呼ばない（quota も Shibubu の上限 `Budget` も使わない）。
    /// 既定 = 読めない provider（鍵の provider・試験の偽物）: `NoCli` の 1 行。本体で `CliLlm` が上書きし plan の login の token で読む（`llm_cli::read_plan_usage`）・鍵の account は
    /// plan の % が無いので `Quota` でなく `NoCli { "API key · billed per use" }`（画面は % の代わりにこの 1 行）。
    /// 例（`Generated::refresh_usage` の裏の thread）: `let u = llm.plan_usage();`。保証しない: 速さ（network・10 秒で打ち切る）・数の正しさ（Anthropic の答えのまま）。
    /// Wave: RQ2
    fn plan_usage(&self) -> Result<PlanUsage, LlmError> {
        Err(LlmError::NoCli { detail: "Usage isn't available for this AI".into() })
    }
}

/// RQ2（RQ-USAGE）: plan の使用量の 1 回の読み（Orca の rate-limits の写しを利用者の目線で選び直した物・YC_DECISIONS 88 通目）。
/// `windows` = (画面の名前, 使った %〈0〜100〉, 戻る時刻〈unix 秒・None = 答えに無い〉)。名前は `Session`（5 時間）・`Weekly`・model 別の週（`Weekly · Fable` 等）の順で、
/// 答えに無い窓は入れない（0 % を作らない）。`checked_at` = 読んだ時刻（unix 秒・`Updated N ago` の元）。
/// 例: `PlanUsage { windows: vec![("Session".into(), 42, Some(1_761_610_000)), ("Weekly".into(), 18, None)], checked_at: 1_761_600_000 }`
/// 保証する: token・email・account の名前を持たない（数と時刻だけ）。保証しない: Anthropic の窓の名前が変わらない事（読めない窓は捨てて読めた窓だけ・全部読めなければ `Malformed`）。
/// 新しくした理由: 既存の型に「前もって見る残り」を持つ物が無い（`Budget` / `Budgeted::used` は Shibubu が決めた回数と秒の上限・`LlmError::PlanLimit` は当たった後の失敗の 1 行）。
/// Wave: RQ2
#[derive(Clone, Debug, PartialEq)]
pub struct PlanUsage {
    pub windows: Vec<(String, u8, Option<i64>)>,
    pub checked_at: i64,
}

/// RQ2（RQ-USAGE）: 下の帯と Settings › AI & Models の 1 行（`*_line` の作法 = 英語の 1 行・次の一手つき）。純関数。
/// 例: `usage_line(&Ok(u), now)` → `Session 42% · resets in 2h 10m · Weekly 18% · updated 3m ago`。
/// 3 つの場面（取れない・未 login・上限）: `Err(NoCli)` = その detail（`Claude Code isn't signed in` 等）・`Err(Network | Timeout | Server | Malformed)` =
/// `Usage couldn't be read`・どれかの窓が 100 % = `Session limit reached · resets in 2h 10m`（戻る時刻が無ければ `· try again later`）。
/// RQ-USAGE: 次の一手（`Try again`・`Sign in`）は 1 行に書かず隣の button（`llm_cli::usage_step`・帯と Settings で同じ）。読み過ぎ（429）は button を出さず待つ事を言う。
/// 保証する: CJK を出さない・token を出さない・`now` より前の戻る時刻は `resets soon`。保証しない: 帯の幅に収まる事（呼ぶ側が切る）。
/// Wave: RQ2
/// RQ2 反証の直し: plan の login の token を Keychain から読めなかった（`security` の時間切れ・鍵の掛かった Keychain・断った許可の窓）時の 1 行（`llm_cli::oauth_token`）。
/// 未 login（`llm_cli::NOT_SIGNED_IN`）と分ける・`Generated::refresh_usage` はこの後も 429 と同じく 5 分あける。
pub const KEYCHAIN_UNREADABLE: &str = "Couldn't read Claude Code's sign-in from Keychain";

pub fn usage_line(usage: &Result<PlanUsage, LlmError>, now: i64) -> String {
    let u = match usage {
        Ok(u) if !u.windows.is_empty() => u,
        Ok(_) => return "Usage couldn't be read".into(),
        Err(LlmError::NoCli { detail }) => return detail.clone(),
        Err(LlmError::RateLimited { .. }) => return "Usage was checked too often · it updates again in a few minutes".into(),
        Err(_) => return "Usage couldn't be read".into(),
    };
    if let Some((name, _, at)) = u.windows.iter().find(|(_, pct, _)| *pct >= 100) {
        let when = match resets(*at, now) {
            r if r.is_empty() => "try again later".to_owned(),
            r => r,
        };
        return format!("{name} limit reached · {when}");
    }
    let mut parts: Vec<String> = window_lines(u, now).into_iter().map(|(line, _)| line).collect();
    let ago = (now - u.checked_at).max(0);
    parts.push(match ago {
        0..=59 => "updated just now".to_owned(),
        60..=3599 => format!("updated {}m ago", ago / 60),
        _ => format!("updated {}h ago", ago / 3600),
    });
    parts.join(" · ")
}

/// `usage_line` の窓の部分（`Session 42% · resets in 2h 10m`）。
fn window_lines(u: &PlanUsage, now: i64) -> Vec<(String, u8)> {
    u.windows
        .iter()
        .map(|(name, pct, at)| match resets(*at, now) {
            r if r.is_empty() => (format!("{name} {pct}%"), *pct),
            r => (format!("{name} {pct}% · {r}"), *pct),
        })
        .collect()
}

/// STATUSBAR-1101 追記（owner 210「providerのusage…orcaを参考に」）: 下の帯の窓ごとの短い語（Orca と同じ `42% used 2h 10m`・戻る時刻が答えに無い / 過ぎた = `42% used`）。純関数。
/// 例: `window_short(&u, now)` → `[("Session".into(), "42% used 2h 10m".into(), 42), ("Weekly".into(), "18% used".into(), 18)]`
pub fn window_short(u: &PlanUsage, now: i64) -> Vec<(String, String, u8)> {
    u.windows
        .iter()
        .map(|(name, pct, at)| {
            let short = match at {
                Some(at) if *at > now => format!("{pct}% used {}", span(*at - now)),
                _ => format!("{pct}% used"),
            };
            (name.clone(), short, *pct)
        })
        .collect()
}

/// 戻る時刻の語（`resets in 2h 10m`・過ぎていれば `resets soon`・答えに無ければ空）。
fn resets(at: Option<i64>, now: i64) -> String {
    match at {
        Some(at) if at > now => format!("resets in {}", span(at - now)),
        Some(_) => "resets soon".to_owned(),
        None => String::new(),
    }
}

/// `usage_line` の残り時間（`2h 10m`・`10m`・`3d 4h`・1 分未満は `1m`）。
fn span(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{}m", m.max(1)),
        (0, h) => format!("{h}h {m}m"),
        (d, h) => format!("{d}d {h}h"),
    }
}

/// 失敗の種類。文言（Display）は画面にそのまま出せる形で、次の一手を付ける
/// 契約（UI-2 から・variant は変えない）: 台本に出る `NoCli` の Display（Home の LLM の 1 行・sendback 27）は表の EN（copy.en.md の
/// `LlmError::NoCli` の行 = `{detail}`・detail も表の EN）。他の variant の Display は「8 本の後」の strict の段（src/llm は TARGETS の外）。
/// 例: `Err(e @ LlmError::NoCli { .. }) => llm_line = Some(e.to_string())`
/// Wave: UI-2
#[derive(Clone, Debug, PartialEq)]
pub enum LlmError {
    /// 鍵が無い（model は optional。呼び手は local heuristics で続ける）
    NoKey,
    /// W13: 利用者の LLM の CLI が使えない（無い・`SHIBUBU_LLM_CLI=""` で切った・login していない・plan でない login・`--version` が答えない）。
    /// 製品は鍵を持たないので、呼び手は AI 無しの手元の推定で続け、この 1 行を Home に出す（D-e）。detail は理由（path・email・token を入れない）
    NoCli { detail: String },
    /// 401 / 403
    Auth { status: u16, detail: String },
    /// 利用上限・残高・支払い。retry しても戻らない
    Quota { detail: String },
    /// W13: 利用者の plan（Claude の Pro・Max など）の使用量の上限。時間が来れば戻る（detail は CLI の文のまま・「resets 3pm」など戻る時刻を含む）。
    /// Console の請求は持たないので、その語を出さない（`llm_cli::parse_result` が出す）
    PlanLimit { detail: String },
    /// 429 が retry の後も続いた。retry_after は API が言った待ち秒数
    RateLimited { retry_after: Option<u64> },
    /// 5xx（500 / 503 / 504 / 529）が retry の後も続いた
    Server { status: u16 },
    /// その他の 4xx（依頼の誤り。retry しない）
    Rejected { status: u16, detail: String },
    /// timeout（retry しない）
    Timeout { ms: u64 },
    Network { detail: String },
    /// model が答えを断った（refusal）
    Refused { detail: String },
    /// 応答が壊れている（JSON でない・途中で切れた・本文が無い）
    Malformed { detail: String },
    /// JSON は来たが呼び手の schema に合わない（幻覚の action id など）。path は "$.actions[0].id" の形
    Invalid { path: String, reason: String },
    /// 呼び手の schema が送れない形（作る側の誤り。送る前に止める）
    Schema { path: String, reason: String },
    /// W14（YCM-067）: Shibubu が決めた生成と修正の上限（回数か時間・`Budget`）に達した。model を呼んでいない。detail は「2 calls」「600 seconds」の形
    OverBudget { detail: String },
    /// FX2（FX-005）: 利用者が `Stop` を押し、呼び出しの子を止めた（`send_cancellable` の `cancel`）。`after_ms` = 印を見てから子を回収し終えるまで
    /// （UIUX §6.4 の child process stop latency・`llm_cli::CallRecord::stopped_after_ms` に同じ数）。失敗ではない: 呼ぶ側は名前の表で作り直さず、何も足さない。
    /// Wave: FX2
    Cancelled { after_ms: u64 },
}
/// 画面の `{reason}` の 1 行（表 = docs/ui-copy/copy.en.md「Errors that fill {reason}」の `LlmError::*` の行・表に無い 4 つは additions.md「AI errors」）。
/// 中の技術的な文（io の誤り・HTTP の body・parse の文・schema の path）は出さない。PlanLimit の detail（CLI の「resets 3pm」）と OverBudget の上限は出す。
impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoKey => write!(f, "no AI key is set"),
            // 表（copy.en.md の `LlmError::NoCli` の行）の EN は `{detail}` だけ（llm_cli の detail が英語の 1 行・sendback 27）
            Self::NoCli { detail } => write!(f, "{detail}"),
            // RQ-API: 鍵の account の 2 つの失敗に次の一手（鍵を貼り直す・Console で足すか account を替える）
            Self::Auth { .. } => write!(f, "the AI key was refused · Paste API key again in Settings › AI & Models"),
            Self::Quota { .. } => write!(f, "the AI usage limit or balance is reached · add credit in the Anthropic Console or choose another account in Settings › AI & Models"),
            Self::PlanLimit { detail } => write!(f, "your Claude plan's limit is reached ({detail}) · try again after it resets"),
            Self::RateLimited { retry_after: Some(s) } => write!(f, "AI is busy · try again in {s} s"),
            Self::RateLimited { retry_after: None } => write!(f, "AI is busy · try again shortly"),
            Self::Server { .. } => write!(f, "the AI service is having trouble · try again shortly"),
            Self::Rejected { detail, .. } if detail == TOO_LONG => write!(f, "this request is too long for Claude · shorten it or start a new chat"),
            Self::Rejected { .. } => write!(f, "AI didn't accept the request"),
            Self::Timeout { ms } => write!(f, "AI didn't answer within {} s", *ms as f64 / 1000.0),
            Self::Network { .. } => write!(f, "the AI can't be reached · check the network"),
            Self::Refused { .. } => write!(f, "AI declined to answer · say it another way"),
            Self::Malformed { .. } => write!(f, "AI's answer couldn't be read · try again"),
            Self::Invalid { .. } => write!(f, "AI's answer didn't fit, so it wasn't used"),
            Self::Schema { .. } => write!(f, "the request to the AI couldn't be made"),
            Self::OverBudget { detail } => write!(f, "AI was stopped at its limit ({detail}) · raise it in Settings › AI & Models › Execution limit"),
            // FX2: 表（copy.en.md の Home「(new) cancelled」）の頭の語。Home の 1 行は Generated が `Stopped. Nothing was added to Projects.` を出す
            Self::Cancelled { .. } => write!(f, "Stopped"),
        }
    }
}
impl std::error::Error for LlmError {}

/// W14（YCM-067「制御不能な AI の反復を禁止」）: 生成と修正で model を呼ぶ回数と時間の上限。超えたら呼ばずに止め、理由の 1 行（`LlmError::OverBudget`）を返す。
/// Wave: W14
/// 使い方: `let llm = Budgeted::new(Box::new(cli), Budget::from_env());`（generated.rs の `with_llm` の 1 か所で包む）→ 生成と編集は今の `complete_json` のまま。
/// 試験（067）: `SHIBUBU_LLM_BUDGET=calls=1` で起こし、2 回目の生成と編集が `OverBudget` の 1 行で止まり、model の CLI の `CallRecord` が 1 行だけ。
/// 保証する: 上限は 1 つの頼みの間の合計（TALK-1101 追記 2・人の一言 / Build now / Preview plan ごとに `Budgeted::new_request` で 0 から。前は process の間の合計で、
/// Home で 5 往復話して 1 回作っただけで尽きた）・API 費用の上限は持たない（製品は鍵を持たず、利用者の plan の login だけを使う・W13）。
/// 保証しない: 利用者の plan の残り（それは CLI の `Quota` / `RateLimited`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// model を呼んでよい回数（0 = 呼ばない）
    pub max_calls: u32,
    /// 呼び出しにかかった時間の合計の上限
    pub max_total: Duration,
}

/// W14: `Budget::from_env` が読む環境変数。値は `calls=<n>` と `secs=<n>` を `,` で並べる（片方だけでもよい）。
/// Wave: W14
pub const BUDGET_ENV: &str = "SHIBUBU_LLM_BUDGET";

impl Budget {
    /// `BUDGET_ENV` を読む。無い欄は既定（1 run の上限と同じ 10 回・600 秒・YC_WAVES の W13 の行）。
    /// Wave: W14
    /// 保証する: 読めない値は既定にする（黙って上限を外さない）・0 は「呼ばない」。
    pub fn from_env() -> Budget {
        Budget::parse(&std::env::var(BUDGET_ENV).unwrap_or_default())
    }

    /// RQ1（RQ-003）: 使う上限を 1 つに決める。`calls` / `secs`（Settings に保存した値 = `UiSettings::llm_max_calls` / `llm_max_secs`）が在ればその欄、
    /// 無い欄は `from_env`（env の `calls=` / `secs=` か既定の 10 回・600 秒）。Settings › AI & Models の `Execution limit` が値を書く（env は試験の口として残す・画面の値が勝つ）。
    /// 呼ぶ側の例: `generated.set_budget(store.ui.llm_max_calls, store.ui.llm_max_secs)` → 中で `Budget::resolve(calls, secs)`。
    /// 保証する: 保存した欄が env より勝つ・保存した 0 は「呼ばない」のまま（既定に戻さない）・片方だけ保存した時はもう片方が env か既定。
    /// 保証しない: 保存した値の範囲の検査（画面の欄が 0〜100 回・0〜3600 秒に詰める）。
    /// Wave: RQ1
    pub fn resolve(calls: Option<u32>, secs: Option<u64>) -> Budget {
        let mut b = Budget::from_env();
        if let Some(n) = calls {
            b.max_calls = n;
        }
        if let Some(s) = secs {
            b.max_total = Duration::from_secs(s);
        }
        b
    }

    /// `from_env` の中身（試験は env を書き換えずにこちらを呼ぶ・env は test の thread の間で共有なので）。
    fn parse(s: &str) -> Budget {
        let mut b = Budget { max_calls: 10, max_total: Duration::from_secs(600) };
        for (k, v) in s.split(',').filter_map(|kv| kv.split_once('=')) {
            match (k.trim(), v.trim().parse::<u64>()) {
                ("calls", Ok(n)) => b.max_calls = u32::try_from(n).unwrap_or(u32::MAX),
                ("secs", Ok(n)) => b.max_total = Duration::from_secs(n),
                _ => {} // 読めない欄は既定のまま（黙って上限を外さない）
            }
        }
        b
    }
}

/// W14: `Llm` を包み、送る前に `Budget` を見る。超えていれば中の `send` を呼ばず `LlmError::OverBudget`。
/// Wave: W14
/// 例: `let b = Budgeted::new(Box::new(fake), Budget { max_calls: 1, max_total: Duration::from_secs(600) }); b.complete_json(req())?; assert!(matches!(b.complete_json(req()), Err(LlmError::OverBudget { .. })))`
/// 保証する: 回数は `send` を呼んだ数（失敗した呼び出しも 1 回）・時間は `send` の前後の壁時計の合計・`label` は中の model と同じ。
/// 保証しない: 1 回の呼び出しを途中で止める事（1 回の上限は中の provider の timeout）。
pub struct Budgeted {
    inner: Box<dyn Llm>,
    /// 上限（RQ1: Settings の `Execution limit` が `set_budget` で替える・`send` の確かめが毎回読む）
    budget: std::sync::Mutex<Budget>,
    used: std::sync::Mutex<(u32, Duration)>,
}

impl Budgeted {
    /// Wave: W14
    pub fn new(inner: Box<dyn Llm>, budget: Budget) -> Budgeted {
        Budgeted { inner, budget: std::sync::Mutex::new(budget), used: std::sync::Mutex::new((0, Duration::ZERO)) }
    }
    /// 今までに使った回数と時間（画面の 1 行と試験が読む）。
    /// Wave: W14
    pub fn used(&self) -> (u32, Duration) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// TALK-1101 追記 2（owner 192 通目の画像: 起動ごとの 10 回が Home の会話 5 往復と 1 回の作成で尽きた）: 人の 1 つの頼みが来た = 使った回数と時間を 0 に戻す。
    /// 呼ぶ所 = 人が送った・押した時だけ（`Generated::new_request` の doc の 4 か所）。止まらない繰り返し（1 つの頼みの中の model の呼び出し・Terminal の agent の
    /// 頼み）は今までどおり上限で止まる。送っている途中の 1 回の分は、終わった時に新しい頼みへ足される。
    /// Wave: TALK-1101
    pub fn new_request(&self) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner()) = (0, Duration::ZERO);
    }
    /// RQ1（RQ-003・監査 S2「実行中の変更は安全な停止点で反映」）: 上限を替える。次の `send` の確かめから新しい上限で数え、使った分（`used`）は 0 に戻さない
    /// （1 つの頼みの中は数え続ける・上げれば止まった後も続けられ、下げれば次の呼び出しで止まる）。送っている途中の 1 回は止めない。
    /// 呼ぶ側の例: `Generated::set_budget` が持っている `Arc<Budgeted>` に `b.set_budget(Budget::resolve(calls, secs))`。
    /// Wave: RQ1
    pub fn set_budget(&self, budget: Budget) {
        *self.budget.lock().unwrap_or_else(|e| e.into_inner()) = budget;
    }
    /// DOGFOOD-X 周 7 #1 #2: 次の呼び出しが上限で止まるか（予約しない・中の model を呼ばない）。Home の会話と `Preview plan` が呼ぶ前に見て、
    /// 止まっている事と次の一手（`OverBudget` の 1 行）を言う。確かめは `reserve` と同じ 1 つ（`over`）。
    /// 例: `if let Some(why) = b.stopped() { say(why.to_string()) } else { b.complete_json(req) }`。保証しない: 見た後に裏の別の呼び出しが上限に着く事（`reserve` が拾う）。
    /// Wave: L2
    pub fn stopped(&self) -> Option<LlmError> {
        let used = *self.used.lock().unwrap_or_else(|e| e.into_inner());
        over(used, *self.budget.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// 上限に着いているか（`reserve` と `stopped` の 1 つの確かめ）。回数を先に見る。
fn over(used: (u32, Duration), budget: Budget) -> Option<LlmError> {
    if used.0 >= budget.max_calls {
        return Some(LlmError::OverBudget { detail: plural(u64::from(budget.max_calls), "call") });
    }
    if used.1 >= budget.max_total {
        return Some(LlmError::OverBudget { detail: plural(budget.max_total.as_secs(), "second") });
    }
    None
}

/// 上限の数と単位（`OverBudget` の detail・画面に出る）: 1 だけ単数（`1 call`・`600 seconds`）。
fn plural(n: u64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

impl Llm for Budgeted {
    fn label(&self) -> String {
        self.inner.label()
    }
    /// RQ2: 使用量の読みは中の provider へそのまま（model を呼ばないので回数と秒に数えない・上限で止まっていても読める）。
    fn plan_usage(&self) -> Result<PlanUsage, LlmError> {
        self.inner.plan_usage()
    }
    /// FX2: `send` と同じ上限の確かめと 1 回の予約をしてから、中の `send_cancellable` へ同じ `cancel` を渡す。
    /// 保証する: 止めた呼び出しも 1 回に数える（子を起こした）・送る前に立っていた時は中を呼ばず数えない。
    /// Wave: FX2
    fn send_cancellable(&self, req: &JsonRequest, cancel: &AtomicBool) -> Result<Value, LlmError> {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(LlmError::Cancelled { after_ms: 0 });
        }
        self.reserve()?;
        let started = std::time::Instant::now();
        let out = self.inner.send_cancellable(req, cancel);
        self.used.lock().unwrap_or_else(|e| e.into_inner()).1 += started.elapsed();
        out
    }
    fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
        // 立たない印 = 中の `send` と同じ（`send_cancellable` の既定・CliLlm の send は同じ本体）
        self.send_cancellable(req, &AtomicBool::new(false))
    }
    // G104: 包んだ provider の物を言う（言わないと製品の CLI〈set_llm が必ず Budgeted で包む〉の添付と effort が永久に押せない形になる）
    fn takes_files(&self) -> bool {
        self.inner.takes_files()
    }
    fn provider_name(&self) -> String {
        self.inner.provider_name()
    }
    fn efforts(&self) -> Vec<&'static str> {
        self.inner.efforts()
    }
    fn models(&self) -> Vec<(String, String)> {
        self.inner.models()
    }
}

impl Budgeted {
    /// 確かめと 1 回の予約を同じ lock の中で（裏の thread が 2 本同時に呼んでも上限を越えない）。送る間は lock を持たない
    fn reserve(&self) -> Result<(), LlmError> {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let budget = *self.budget.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = over(*used, budget) {
            return Err(e);
        }
        used.0 += 1;
        Ok(())
    }
}

/// 鍵を env → Keychain（service 名で引く。account は問わない）の順に探す。空は無いと同じ。値は表示も保存もしない
pub fn api_key(env_name: &str, keychain_service: &str) -> Option<String> {
    let keychain = || Command::new("security").args(["find-generic-password", "-s", keychain_service, "-w"]).output().ok()
        .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    std::env::var(env_name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
        .or_else(|| keychain().filter(|v| !v.is_empty()))
}

/// 使える provider を 1 つ選ぶ: Anthropic（ANTHROPIC_API_KEY → Keychain kangae-anthropic）→ OpenAI（OPENAI_API_KEY → Keychain kangae-openai）。
/// 鍵がどちらも無ければ None（model は optional）。Keychain を引くので起動時に 1 回だけ呼ぶ
pub fn from_env() -> Option<Box<dyn Llm>> {
    if let Ok(a) = anthropic::Anthropic::from_env() { return Some(Box::new(a)) }
    openai::OpenAi::from_env().ok().map(|o| Box::new(o) as Box<dyn Llm>)
}

/// model の答えか、使えなかった時の local heuristics の答え。Local の reason は画面と生成の記録に出す
#[derive(Debug, PartialEq)]
pub enum Answer { Model { value: Value, label: String }, Local { value: Value, reason: LlmError } }

/// GEN-012 の「model は optional」: llm が None（鍵が無い）・失敗・答えの拒否（幻覚の id を含む）の時は local() の答えにする
pub fn complete_or_local(llm: Option<&dyn Llm>, req: JsonRequest, local: impl FnOnce() -> Value) -> Answer {
    let Some(llm) = llm else { return Answer::Local { value: local(), reason: LlmError::NoKey } };
    match llm.complete_json(req) {
        Ok(value) => Answer::Model { value, label: llm.label() },
        Err(reason) => Answer::Local { value: local(), reason },
    }
}

/// timeout と retry。既定は 20 秒・429 / 5xx を 2 回まで（待ちは retry-after の秒数、無ければ 0.5 秒 → 1 秒）。
/// 待ちが max_wait を超える時は待たずに返す（chat bar を止めない。秒数は RateLimited に載る）。試験で短くする
#[derive(Clone, Debug)]
pub struct Http { pub timeout: Duration, pub retries: u32, pub backoff: Duration, pub max_wait: Duration }
impl Default for Http {
    fn default() -> Self { Self { timeout: Duration::from_secs(20), retries: 2, backoff: Duration::from_millis(500), max_wait: Duration::from_secs(10) } }
}

/// POST して応答の JSON を返す。timeout と接続の失敗は retry しない（20 秒 × 3 を待たせない）
fn post_json(http: &Http, url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value, LlmError> {
    let agent = ureq::AgentBuilder::new().timeout(http.timeout).redirects(0).build();
    let mut attempt = 0;
    loop {
        let mut r = agent.post(url);
        for (k, v) in headers { r = r.set(k, v) }
        let err = match r.send_json(body) {
            Ok(resp) => return read_json(resp, http),
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok());
                status_error(code, &read_json(resp, http).unwrap_or(Value::Null), retry_after)
            }
            Err(ureq::Error::Transport(t)) => {
                let io_kind = std::error::Error::source(&t).and_then(|e| e.downcast_ref::<io::Error>()).map(io::Error::kind);
                return Err(if io_kind.is_some_and(timed_out) { LlmError::Timeout { ms: http.timeout.as_millis() as u64 } } else { LlmError::Network { detail: t.to_string() } })
            }
        };
        let wait = match &err {
            LlmError::RateLimited { retry_after: Some(s) } => Duration::from_secs(*s),
            LlmError::RateLimited { .. } | LlmError::Server { .. } => http.backoff * 2u32.pow(attempt),
            _ => return Err(err),
        };
        if attempt >= http.retries || wait > http.max_wait { return Err(err) }
        thread::sleep(wait);
        attempt += 1;
    }
}
fn timed_out(kind: io::ErrorKind) -> bool { matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) }
fn read_json(resp: ureq::Response, http: &Http) -> Result<Value, LlmError> {
    const CAP: u64 = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    resp.into_reader().take(CAP + 1).read_to_end(&mut bytes).map_err(|e| {
        if timed_out(e.kind()) { LlmError::Timeout { ms: http.timeout.as_millis() as u64 } } else { LlmError::Network { detail: e.to_string() } }
    })?;
    if bytes.len() as u64 > CAP { return Err(LlmError::Malformed { detail: "応答が大きすぎます".into() }) }
    serde_json::from_slice(&bytes).map_err(|_| LlmError::Malformed { detail: "応答が JSON ではありません（プロキシや一時的な障害の可能性）".into() })
}

/// HTTP の失敗を種類に分ける。Anthropic {"type":"error","error":{"type","message","details":{"error_code"}}} と
/// OpenAI {"error":{"type","code","message"}} の両方を読む。利用上限・残高・支払いは retry しても戻らないので Quota:
/// Anthropic は 402 billing_error・429 の enforced_spend_limit_reached（retry-after 無し）・400 の「specified API usage limits」
/// （https://platform.claude.com/docs/en/api/rate-limits）と 400 の「credit balance is too low」（公式 doc に無く利用者の報告から）、
/// OpenAI は 429 の insufficient_quota と *_spend_limit_exceeded 等（https://developers.openai.com/api/docs/guides/error-codes）
/// REVIEW-RQN: 鍵の account の CLI の失敗（`llm_cli::parse_answer`）もこの表で分ける（CLI の文の API の誤りの JSON か、文を message にした body）。
pub(crate) fn status_error(status: u16, body: &Value, retry_after: Option<u64>) -> LlmError {
    let e = &body["error"];
    let kind = e["type"].as_str().unwrap_or("");
    let code = e["code"].as_str().or(e["details"]["error_code"].as_str()).unwrap_or("");
    let detail = e["message"].as_str().unwrap_or("").to_string();
    if too_long(status, kind, &detail) {
        return LlmError::Rejected { status, detail: TOO_LONG.into() };
    }
    let quota = status == 402 || matches!(kind, "billing_error" | "insufficient_quota")
        || matches!(code, "enforced_spend_limit_reached" | "insufficient_quota" | "credit_balance_too_low" | "credit_balance_exhausted"
            | "organization_spend_limit_exceeded" | "project_spend_limit_exceeded" | "organization_usage_limit_exceeded")
        || (status == 400 && (detail.contains("usage limits") || detail.contains("credit balance")));
    match status {
        _ if quota => LlmError::Quota { detail },
        401 | 403 => LlmError::Auth { status, detail },
        429 => LlmError::RateLimited { retry_after },
        500..=599 => LlmError::Server { status },
        _ => LlmError::Rejected { status, detail },
    }
}

/// REVIEW-RQN（operator 2）: 依頼が model の文脈の長さを超えた時の `Rejected` の detail（Display = 短くするか新しい chat の 1 行・Console と上限の語を言わない）。
pub const TOO_LONG: &str = "request_too_large";

/// 文脈の長さを超えたか: 413・型 `request_too_large`・400（`invalid_request_error`・公式に専用の型が無い）のうち文脈の長さの文
/// （https://platform.claude.com/docs/en/api/errors ・2026-10-29 に読んだ）。「limit」1 語では見ない（Console の上限・混雑と分ける）。
pub(crate) fn too_long(status: u16, kind: &str, message: &str) -> bool {
    let low = message.to_lowercase();
    status == 413
        || kind == "request_too_large"
        || (status == 400 && ["prompt is too long", "context limit", "context window", "context length"].iter().any(|w| low.contains(w)))
}

/// model の答えの本文（text）を JSON にする
fn parse_json_text(text: &str) -> Result<Value, LlmError> {
    if text.trim().is_empty() { return Err(LlmError::Malformed { detail: "答えの本文がありません".into() }) }
    serde_json::from_str(text.trim()).map_err(|e| LlmError::Malformed { detail: format!("答えが JSON ではありません（{e}）") })
}

const KEYWORDS: [&str; 10] = ["type", "properties", "required", "additionalProperties", "items", "enum", "const", "anyOf", "description", "title"];

/// 送る前の検査（Fake も同じ）。schema の誤りは作る側の誤りなので、呼び手の test の時点で落ちる
fn check_request(req: &JsonRequest) -> Result<(), LlmError> {
    let name_ok = !req.name.is_empty() && req.name.len() <= 64 && req.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !name_ok { return Err(LlmError::Schema { path: "name".into(), reason: "英数字と _ - の 64 文字以内にしてください（OpenAI の制約）".into() }) }
    if req.schema["type"] != "object" { return Err(LlmError::Schema { path: "$".into(), reason: "root は type: object にしてください（OpenAI の制約）".into() }) }
    let mut unions = 0;
    check_schema(&req.schema, "$", &mut unions)?;
    if unions > 16 { return Err(LlmError::Schema { path: "$".into(), reason: format!("anyOf と type の配列が {unions} 箇所あります（Anthropic の上限は 16）") }) }
    Ok(())
}
fn check_schema(s: &Value, path: &str, unions: &mut usize) -> Result<(), LlmError> {
    let bad = |reason: String| Err(LlmError::Schema { path: path.into(), reason });
    let Some(o) = s.as_object() else { return bad("schema が object ではありません".into()) };
    if let Some(k) = o.keys().find(|k| !KEYWORDS.contains(&k.as_str())) {
        return bad(format!("keyword {k} は使えません（両 provider が受ける物だけ。数や長さは返った値を呼び手が確かめる）"))
    }
    if s.get("anyOf").is_some() || types_of(s).len() > 1 { *unions += 1 }
    if types_of(s).contains(&"object") {
        if s["additionalProperties"] != false { return bad("object には additionalProperties: false が要ります".into()) }
        let required: Vec<&str> = s["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        for (k, sub) in s["properties"].as_object().into_iter().flatten() {
            if !required.contains(&k.as_str()) { return bad(format!("{k} を required に入れてください（任意の値は type に \"null\" を足す）")) }
            check_schema(sub, &format!("{path}.{k}"), unions)?;
        }
    }
    if let Some(items) = s.get("items") { check_schema(items, &format!("{path}[]"), unions)? }
    for alt in s["anyOf"].as_array().into_iter().flatten() { check_schema(alt, path, unions)? }
    Ok(())
}

/// 返った JSON を呼び手の schema で検証する。model の出力は信用しない（enum に無い action id はここで落ちる）。
/// ponytail: JSON Schema の部分集合（KEYWORDS）だけ。$ref・pattern・format 等は check_schema が送る前に拒否するので、ここに来ない
pub fn validate(schema: &Value, v: &Value) -> Result<(), LlmError> { walk(schema, v, "$") }
fn walk(s: &Value, v: &Value, path: &str) -> Result<(), LlmError> {
    let bad = |reason: String| Err(LlmError::Invalid { path: path.into(), reason });
    let types = types_of(s);
    if !types.is_empty() && !types.iter().any(|t| is_type(t, v)) { return bad(format!("{} ではありません（{}）", types.join(" か "), short(v))) }
    if let Some(allowed) = s["enum"].as_array() {
        if !allowed.contains(v) { return bad(format!("{} は許された値（{}）にありません", short(v), allowed.iter().map(short).collect::<Vec<_>>().join(", "))) }
    }
    if let Some(c) = s.get("const") {
        if c != v { return bad(format!("{} であるべき所が {} です", short(c), short(v))) }
    }
    if let Some(alts) = s["anyOf"].as_array() {
        if !alts.iter().any(|a| walk(a, v, path).is_ok()) { return bad(format!("どの形（anyOf）にも合いません（{}）", short(v))) }
    }
    if let Value::Object(o) = v {
        for k in s["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if !o.contains_key(k) { return bad(format!("必須の {k} がありません")) }
        }
        for (k, x) in o {
            match s["properties"].get(k) {
                Some(sub) => walk(sub, x, &format!("{path}.{k}"))?,
                None if s["additionalProperties"] == false => return bad(format!("schema に無い {k} があります")),
                None => {}
            }
        }
    }
    if let (Value::Array(a), Some(items)) = (v, s.get("items")) {
        for (i, x) in a.iter().enumerate() { walk(items, x, &format!("{path}[{i}]"))? }
    }
    Ok(())
}
fn types_of(s: &Value) -> Vec<&str> {
    match &s["type"] { Value::String(t) => vec![t.as_str()], Value::Array(a) => a.iter().filter_map(Value::as_str).collect(), _ => vec![] }
}
fn is_type(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => false,
    }
}
/// error 文言に入れる値（長い値は 60 文字で切る）。capability_engine の読み戻しの行も同じ物を使う
pub(crate) fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }
}

// ---- anthropic（旧 llm/anthropic.rs）----
pub mod anthropic {
    //! Anthropic Messages API。構造化出力は output_config.format（json_schema）で強制する。
    //! tool use の input_schema + tool_choice での強制は使わない: claude-sonnet-5-5 は tool_choice の any / tool を 400 で拒否する
    //! （https://platform.claude.com/docs/en/api/errors の Forced tool use not supported）。
    //! 出典（2026-10-17 取得）: https://platform.claude.com/docs/en/build-with-claude/structured-outputs（形・答えは text block の JSON）
    //!   https://platform.claude.com/docs/en/about-claude/models/overview（model id）・https://platform.claude.com/docs/en/about-claude/pricing（価格）
    //!   https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback（fallbacks: "default" と fallback block）
    use super::{api_key, parse_json_text, post_json, Http, JsonRequest, Llm, LlmError};
    use serde_json::{json, Value};

    pub const BASE: &str = "https://api.anthropic.com";
    /// 既定 model。入力 $2 / 出力 $10 per MTok（cache hit $0.10）。env ANTHROPIC_MODEL で替えられる
    pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
    /// env ANTHROPIC_API_KEY が無い時の鍵の置き場。入れ方: security add-generic-password -a "$USER" -s kangae-anthropic -w（値は prompt で打つ）
    pub const KEYCHAIN_SERVICE: &str = "kangae-anthropic";
    /// 断られた依頼（stop_reason: refusal）を Anthropic が選ぶ別 model で答え直させる beta。claude-sonnet-5-5 では cyber / frontier_llm の拒否が対象
    const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

    pub struct Anthropic { pub key: String, pub model: String, pub base: String, pub http: Http }
    impl Anthropic {
        pub fn new(key: String) -> Self {
            let model = std::env::var("ANTHROPIC_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
            Self { key, model, base: BASE.into(), http: Http::default() }
        }
        /// 鍵は env ANTHROPIC_API_KEY → Keychain kangae-anthropic。無ければ NoKey
        pub fn from_env() -> Result<Self, LlmError> { api_key("ANTHROPIC_API_KEY", KEYCHAIN_SERVICE).map(Self::new).ok_or(LlmError::NoKey) }
        fn post(&self, req: &JsonRequest, fallback: bool) -> Result<Value, LlmError> {
            // thinking・temperature・tool_choice は送らない（claude-sonnet-5-5 では disabled / 非既定値 / any・tool がどれも 400）
            // ponytail: effort は low 固定（既定の high は長く考え、20 秒の timeout に掛かる）。深く考えさせたい呼び手が出たら JsonRequest に足す
            let mut body = json!({
                "model": self.model, "max_tokens": req.max_tokens, "system": req.instructions,
                "messages": [{"role": "user", "content": req.input}],
                "output_config": {"format": {"type": "json_schema", "schema": req.schema}, "effort": "low"},
            });
            let mut headers = vec![("x-api-key", self.key.as_str()), ("anthropic-version", "2023-06-01")];
            if fallback {
                body["fallbacks"] = json!("default");
                headers.push(("anthropic-beta", FALLBACK_BETA));
            }
            post_json(&self.http, &format!("{}/v1/messages", self.base), &headers, &body)
        }
    }
    impl Llm for Anthropic {
        fn label(&self) -> String { format!("anthropic/{}", self.model) }
        fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
            // beta を受けない org・経路の 400（"Unexpected value(s) ... for the `anthropic-beta` header"）は fallback なしで 1 回だけ送り直す
            let v = match self.post(req, true) {
                Err(LlmError::Rejected { status: 400, detail }) if detail.contains("anthropic-beta") || detail.contains("fallbacks") => self.post(req, false)?,
                r => r?,
            };
            match v["stop_reason"].as_str() {
                Some("refusal") => {
                    let d = &v["stop_details"];
                    return Err(LlmError::Refused { detail: d["explanation"].as_str().or(d["category"].as_str()).unwrap_or("分類なし").into() })
                }
                Some("max_tokens") => return Err(LlmError::Malformed { detail: format!("max_tokens（{}）で途中で切れました", req.max_tokens) }),
                _ => {}
            }
            // thinking block は飛ばす。fallback block より前の text は断った model の途中の出力なので捨てる
            let mut text = String::new();
            for b in v["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("fallback") => text.clear(),
                    Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            parse_json_text(&text)
        }
    }
} // mod anthropic

// ---- fake（旧 llm/fake.rs）----
pub mod fake {
    //! test 用の Llm。決めた返答を順に返し、受けた依頼を記録する。返答は本物と同じ complete_json の検証を通るので、
    //! 呼び手（composer / chat bar）の test で「幻覚の action id が拒否され local に落ちる」をそのまま確かめられる
    use super::{JsonRequest, Llm, LlmError};
    use serde_json::Value;
    use std::{collections::VecDeque, sync::Mutex};

    pub struct Fake { replies: Mutex<VecDeque<Result<Value, LlmError>>>, pub requests: Mutex<Vec<JsonRequest>> }
    impl Fake {
        pub fn new(replies: Vec<Result<Value, LlmError>>) -> Self { Self { replies: Mutex::new(replies.into()), requests: Mutex::new(Vec::new()) } }
    }
    impl Llm for Fake {
        fn label(&self) -> String { "fake".into() }
        fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
            self.requests.lock().unwrap().push(req.clone());
            self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err(LlmError::Malformed { detail: "Fake の返答が尽きました".into() }))
        }
    }
} // mod fake

// ---- openai（旧 llm/openai.rs）----
pub mod openai {
    //! OpenAI Responses API（src/ai.rs の呼び方を移した物）。構造化出力は text.format の json_schema（strict）。
    //! 出典（2026-10-17 取得）: https://developers.openai.com/api/docs/guides/structured-outputs（形・全 property required・refusal・incomplete）
    //!   https://developers.openai.com/api/docs/deprecations（ai.rs の gpt-4.1-mini は 2027-01-14 廃止・置き換えは gpt-6-luna）
    //!   https://developers.openai.com/api/docs/models/gpt-6-luna（$0.10 / $0.50 per 1M tokens・structured outputs と /v1/responses に対応）
    use super::{api_key, parse_json_text, post_json, Http, JsonRequest, Llm, LlmError};
    use serde_json::{json, Value};

    pub const BASE: &str = "https://api.openai.com";
    /// 既定 model。env OPENAI_MODEL で替えられる（ai.rs と同じ変数）
    pub const DEFAULT_MODEL: &str = "gpt-6-luna";
    /// env OPENAI_API_KEY が無い時の鍵の置き場（入れ方は kangae-anthropic と同じ）
    pub const KEYCHAIN_SERVICE: &str = "kangae-openai";

    pub struct OpenAi { pub key: String, pub model: String, pub base: String, pub http: Http }
    impl OpenAi {
        pub fn new(key: String) -> Self {
            let model = std::env::var("OPENAI_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
            Self { key, model, base: BASE.into(), http: Http::default() }
        }
        /// 鍵は env OPENAI_API_KEY → Keychain kangae-openai。無ければ NoKey
        pub fn from_env() -> Result<Self, LlmError> { api_key("OPENAI_API_KEY", KEYCHAIN_SERVICE).map(Self::new).ok_or(LlmError::NoKey) }
    }
    impl Llm for OpenAi {
        fn label(&self) -> String { format!("openai/{}", self.model) }
        fn send(&self, req: &JsonRequest) -> Result<Value, LlmError> {
            let body = json!({
                "model": self.model, "instructions": req.instructions, "input": req.input, "max_output_tokens": req.max_tokens,
                // 依頼の中身（mail・予定）を OpenAI 側に残さない
                "store": false,
                "text": {"format": {"type": "json_schema", "name": req.name, "schema": req.schema, "strict": true}},
            });
            let auth = format!("Bearer {}", self.key);
            let v = post_json(&self.http, &format!("{}/v1/responses", self.base), &[("Authorization", auth.as_str())], &body)?;
            if v["status"] == "incomplete" {
                return Err(LlmError::Malformed { detail: format!("途中で切れました（{}）", v["incomplete_details"]["reason"].as_str().unwrap_or("理由不明")) })
            }
            let mut text = String::new();
            let parts = v["output"].as_array().into_iter().flatten().filter(|o| o["type"] == "message").flat_map(|o| o["content"].as_array().into_iter().flatten());
            for c in parts {
                match c["type"].as_str() {
                    Some("refusal") => return Err(LlmError::Refused { detail: c["refusal"].as_str().unwrap_or("").into() }),
                    Some("output_text") => text.push_str(c["text"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            parse_json_text(&text)
        }
    }
} // mod openai

/// W14 の署名を compile で縛り、本体の要らない性質（OverBudget の 1 行）だけを走らせる。
#[cfg(test)]
mod w14_interface {
    use super::*;
    #[test]
    fn w14_budget_signatures() {
        let _: fn() -> Budget = Budget::from_env;
        let _: fn(Box<dyn Llm>, Budget) -> Budgeted = Budgeted::new;
        let _: fn(&Budgeted) -> (u32, Duration) = Budgeted::used;
        fn is_llm<T: Llm>() {}
        is_llm::<Budgeted>();
    }

    /// RQ1 interface: 保存した上限が env より勝ち、無ければ `from_env` と同じ。署名を compile で縛る（`set_budget` の本体は RQ1 の本体）。
    /// 負の対照（commit message）: `resolve` を `Budget::from_env()` だけにする → 赤。
    #[test]
    fn rq1_saved_budget_wins_over_env() {
        // env を読む欄と比べない（env は test の thread の間で共有）: 保存した欄だけで決まる値を見る
        assert_eq!(Budget::resolve(Some(0), Some(30)), Budget { max_calls: 0, max_total: Duration::from_secs(30) }, "保存した 0 回を既定に戻した");
        assert_eq!(Budget::resolve(Some(2), None).max_calls, 2, "回数だけ保存した時に回数が効かない");
        assert_eq!(Budget::resolve(None, Some(5)).max_total, Duration::from_secs(5), "秒だけ保存した時に秒が効かない");
        let _: fn(&Budgeted, Budget) = Budgeted::set_budget;
    }
    #[test]
    fn w14_over_budget_line_says_it_stopped_and_what_happens_next() {
        let line = LlmError::OverBudget { detail: "1 call".into() }.to_string();
        // DOGFOOD-X 周 7 #3: 次の一手は上限を上げる所（保存した 0 は開き直しても 0 のまま = 「開き直す」は効かない）
        assert!(line.contains("(1 call)") && line.contains("stopped at its limit") && line.contains("Settings › AI & Models › Execution limit") && !line.contains("reopen"), "{line}");
    }

    /// RQ2 interface（caller の形・本体の前は `usage_line` の todo!() で赤）: % と戻るまでの時間・上限に着いた窓・読めない時の次の一手を 1 行で言い、
    /// 使用量の読みは上限の包みを通しても数えない（model を呼ばない）。負の対照（commit message）: `Budgeted::plan_usage` を消す（既定の NoCli に落ちる）→ 赤。
    #[test]
    fn rq2_usage_line_says_percent_and_resets() {
        struct Plan;
        impl Llm for Plan {
            fn label(&self) -> String { "plan".into() }
            fn send(&self, _: &JsonRequest) -> Result<Value, LlmError> { unreachable!("使用量の読みで model を呼んだ") }
            fn plan_usage(&self) -> Result<PlanUsage, LlmError> { Ok(PlanUsage { windows: vec![("Session".into(), 42, Some(7_800)), ("Weekly".into(), 18, None)], checked_at: 0 }) }
        }
        let b = Budgeted::new(Box::new(Plan), Budget { max_calls: 0, max_total: Duration::ZERO });
        let u = b.plan_usage();
        assert_eq!(b.used().0, 0, "使用量の読みを呼び出しに数えた");
        let line = usage_line(&u, 0);
        assert!(line.contains("Session 42%") && line.contains("resets in 2h 10m") && line.contains("Weekly 18%"), "{line}");
        let full = Ok(PlanUsage { windows: vec![("Session".into(), 100, Some(600))], checked_at: 0 });
        assert!(usage_line(&full, 0).starts_with("Session limit reached · resets in 10m"), "{}", usage_line(&full, 0));
        // RQ-USAGE: 次の一手は隣の button（1 行に `Try again` を重ねて書かない）
        assert_eq!(usage_line(&Err(LlmError::Network { detail: "offline".into() }), 0), "Usage couldn't be read");
        // STATUSBAR-1101 追記 210: 帯の短い語（戻る時刻が無い / 過ぎた窓は残り時間を作らない）
        let short = window_short(u.as_ref().unwrap(), 9_000);
        assert_eq!(short, vec![(String::from("Session"), String::from("42% used"), 42u8), (String::from("Weekly"), String::from("18% used"), 18u8)]);
        assert_eq!(window_short(u.as_ref().unwrap(), 0)[0].1, "42% used 2h 10m");
    }
}

/// r5 の #21（画面に「AI の応答を読めませんでした（…）」）: どの `LlmError` の 1 行も画面に CJK を出さない・中の技術的な文（壊れた答えの parse の文）は出さない（D-e・copy.en.md「Errors that fill {reason}」）。
/// 負の対照: Display の腕を 1 つ日本語に戻す → `llm_error_lines_are_english` が赤・Malformed に `{detail}` を戻す → `!line.contains(SECRET)` で赤。
#[cfg(test)]
mod display_en {
    use super::*;
    const SECRET: &str = "expected value at line 1 column 1";

    #[test]
    fn llm_error_lines_are_english() {
        let d = || "detail".to_owned();
        let all = [
            LlmError::NoKey,
            LlmError::NoCli { detail: d() },
            LlmError::Auth { status: 401, detail: d() },
            LlmError::Quota { detail: d() },
            LlmError::PlanLimit { detail: d() },
            LlmError::RateLimited { retry_after: Some(3) },
            LlmError::RateLimited { retry_after: None },
            LlmError::Server { status: 503 },
            LlmError::Rejected { status: 400, detail: d() },
            LlmError::Timeout { ms: 1500 },
            LlmError::Network { detail: d() },
            LlmError::Refused { detail: d() },
            LlmError::Malformed { detail: SECRET.into() },
            LlmError::Invalid { path: "$.a".into(), reason: d() },
            LlmError::Schema { path: "$.a".into(), reason: d() },
            LlmError::OverBudget { detail: plural(1, "call") },
            LlmError::Cancelled { after_ms: 120 },
        ];
        for e in &all {
            let line = e.to_string();
            let cjk: String = line.chars().filter(|c| matches!(*c as u32, 0x3000..=0x30ff | 0x3400..=0x9fff | 0xff00..=0xffef)).collect();
            assert!(cjk.is_empty(), "{e:?} の 1 行に CJK（{cjk}）: {line}");
        }
        let line = LlmError::Malformed { detail: SECRET.into() }.to_string();
        assert!(line == "AI's answer couldn't be read · try again" && !line.contains(SECRET), "{line}");
        // REVIEW-RQN 2: HTTP の道も文脈の長さは短くする 1 行（「limit」の語で Console の上限にしない）・残高は Console
        let body = |t: &str, m: &str| serde_json::json!({"type":"error","error":{"type":t,"message":m}});
        for (status, b) in [(413, Value::Null), (400, body("invalid_request_error", "input length and `max_tokens` exceed context limit: 190000 + 32000 > 200000"))] {
            assert_eq!(status_error(status, &b, None).to_string(), "this request is too long for Claude · shorten it or start a new chat", "{status} {b}");
        }
        assert!(matches!(status_error(400, &body("invalid_request_error", "Your credit balance is too low"), None), LlmError::Quota { .. }));
        assert_eq!((plural(1, "call"), plural(10, "call"), plural(0, "second")), ("1 call".into(), "10 calls".into(), "0 seconds".into()));
    }
}

/// W14 本体（YCM-067）: 上限で止まり、中の model を呼ばず、理由の 1 行を返す。env は書き換えない（`Budget::parse` を直に）。
/// 負の対照: `send` の `used.0 >= max_calls` の確かめを外すと `w14_calls_limit_*` が赤（2 回目が中の Fake に届き「返答が尽きました」）。
#[cfg(test)]
mod w14 {
    use super::*;
    use serde_json::json;

    fn req() -> JsonRequest {
        JsonRequest::new("w14", "", "", json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false }))
    }
    fn budget(calls: u32, secs: u64) -> Budget {
        Budget { max_calls: calls, max_total: Duration::from_secs(secs) }
    }

    #[test]
    fn w14_parse_reads_calls_and_secs_and_never_lifts_the_limit() {
        let d = budget(10, 600);
        assert_eq!(Budget::parse(""), d, "無ければ既定（1 run の上限と同じ）");
        assert_eq!(Budget::parse("calls=1"), budget(1, 600));
        assert_eq!(Budget::parse(" secs = 30 "), budget(10, 30));
        assert_eq!(Budget::parse("calls=2,secs=5"), budget(2, 5));
        assert_eq!(Budget::parse("calls=0"), budget(0, 600), "0 は呼ばない");
        for bad in ["calls=-1", "calls=many", "calls=", "call=1", "calls", "secs=1.5"] {
            assert_eq!(Budget::parse(bad), d, "{bad}: 読めない値は既定（上限を外さない）");
        }
    }

    #[test]
    fn w14_calls_limit_stops_before_the_model_and_says_why() {
        let b = Budgeted::new(Box::new(fake::Fake::new(vec![Ok(json!({}))])), budget(1, 600));
        assert_eq!(b.label(), "fake", "label は中の model と同じ");
        assert_eq!(b.complete_json(req()), Ok(json!({})));
        match b.complete_json(req()) {
            Err(e @ LlmError::OverBudget { .. }) => assert!(e.to_string().contains("(1 call)"), "{e}"),
            other => panic!("2 回目は上限で止まる: {other:?}"),
        }
        assert_eq!(b.used().0, 1, "止めた呼び出しは数えない（中の model を呼んでいない）");
    }

    #[test]
    fn w14_zero_calls_never_reaches_the_model_and_failures_still_count() {
        let none = Budgeted::new(Box::new(fake::Fake::new(vec![Ok(json!({}))])), budget(0, 600));
        assert!(matches!(none.complete_json(req()), Err(LlmError::OverBudget { .. })));
        assert_eq!(none.used(), (0, Duration::ZERO));
        let failing = Budgeted::new(Box::new(fake::Fake::new(vec![Err(LlmError::Timeout { ms: 1 })])), budget(1, 600));
        assert_eq!(failing.complete_json(req()), Err(LlmError::Timeout { ms: 1 }));
        assert!(matches!(failing.complete_json(req()), Err(LlmError::OverBudget { .. })), "失敗した呼び出しも 1 回");
    }

    /// RQ1（Settings › AI & Models の `Execution limit`）: 上限を上げれば止まった後も続き、下げれば次の呼び出しで止まる。使った分は 0 に戻さない（起動ごとに数える）。
    /// 負の対照: `set_budget` の本体を空にする → 上げた後の 2 回目が OverBudget のままで赤。
    #[test]
    fn rq1_set_budget_raises_and_lowers_without_resetting_what_was_used() {
        let b = Budgeted::new(Box::new(fake::Fake::new(vec![Ok(json!({})), Ok(json!({})), Ok(json!({}))])), budget(1, 600));
        assert_eq!(b.complete_json(req()), Ok(json!({})));
        assert!(matches!(b.complete_json(req()), Err(LlmError::OverBudget { .. })), "1 回の上限で止まる");
        b.set_budget(budget(2, 600));
        assert_eq!(b.complete_json(req()), Ok(json!({})), "上げた上限で続けられる");
        assert_eq!(b.used().0, 2, "使った分を 0 に戻した");
        b.set_budget(budget(2, 600));
        match b.complete_json(req()) {
            Err(e @ LlmError::OverBudget { .. }) => assert!(e.to_string().contains("(2 calls)"), "止めた上限の数が新しい上限でない: {e}"),
            other => panic!("同じ上限の 3 回目は止まる: {other:?}"),
        }
        b.set_budget(budget(0, 600));
        assert!(matches!(b.complete_json(req()), Err(LlmError::OverBudget { .. })), "0 に下げたら呼ばない");
        assert_eq!(b.used().0, 2);
    }

    /// DOGFOOD-X 周 7 #1 #2: `stopped` は呼ぶ前に止まる事を言い、予約しない（見るだけで 1 回を使わない）。上げれば None。
    /// 負の対照（束）: `stopped` の本体を `None` にする → 1 つ目が赤。
    #[test]
    fn r7_stopped_says_the_limit_before_calling_and_reserves_nothing() {
        let b = Budgeted::new(Box::new(fake::Fake::new(vec![Ok(json!({}))])), budget(0, 600));
        let why = b.stopped().expect("上限 0 は呼ぶ前に止まっていると言う");
        assert!(matches!(why, LlmError::OverBudget { .. }) && why.to_string().contains("(0 calls)"), "{why}");
        assert_eq!(b.used(), (0, Duration::ZERO), "見ただけで 1 回を使った");
        b.set_budget(budget(1, 600));
        assert_eq!(b.stopped(), None, "上げたのに止まっていると言う");
        assert_eq!(b.complete_json(req()), Ok(json!({})), "stopped を見た後の 1 回目が呼べない");
        assert!(b.stopped().is_some(), "1 回の上限に着いた後");
    }

    /// 時間の上限: 合計が上限に達したら次は呼ばない（1 回の途中では止めない）。
    #[test]
    fn w14_time_limit_counts_wall_clock_of_sends() {
        struct Slow;
        impl Llm for Slow {
            fn label(&self) -> String { "slow".into() }
            fn send(&self, _: &JsonRequest) -> Result<Value, LlmError> {
                thread::sleep(Duration::from_millis(30));
                Ok(json!({}))
            }
        }
        let b = Budgeted::new(Box::new(Slow), Budget { max_calls: 10, max_total: Duration::from_millis(20) });
        assert!(b.complete_json(req()).is_ok(), "1 回目は上限の前なので呼ぶ");
        assert!(b.used().1 >= Duration::from_millis(30), "{:?}", b.used());
        match b.complete_json(req()) {
            Err(e @ LlmError::OverBudget { .. }) => assert!(e.to_string().contains("(0 seconds)"), "{e}"),
            other => panic!("時間の上限で止まる: {other:?}"),
        }
        assert_eq!(b.used().0, 1);
    }
}
