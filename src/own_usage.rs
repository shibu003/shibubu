//! USAGE-OWN-1101（owner 203 通目「shibubuがどれだけusageを使ったか…orcaでclaudeを使っているusageとは別にshibubuないだけで」）:
//! Shibubu の中だけで使った AI の量。Anthropic は account 全体の %（`llm::PlanUsage`・Orca や他の terminal の claude も混ざる）しか返さないので、Shibubu が自分で数える。
//! 数える物は 2 つ:
//! 1. Shibubu が自分で呼んだ AI = `<data_dir>/llm-calls.jsonl` の `llm_cli::CallRecord`（回数・token・時間・API 換算）
//! 2. Shibubu の Terminal の中の agent = Terminal の前面に居た claude / grok / codex の session（`AGENT_LOG` の `AgentSession`）の token。
//!    claude と grok は pid から session を引く（Shibubu の Terminal で起きた session だけ・Orca の session は入らない）。codex は pid の記録が無いので cwd と時刻で引く。
//!
//! 中身（prompt・答え・会話）は読まない・持たない: transcript の行から取り出すのは時刻・`usage`・request の id だけ。
//! 新しくした理由: 既存の `llm::PlanUsage` は account 全体の %、`Budgeted::used` は 1 つの頼みの回数と秒で、期間と内訳を持つ型が無い。

use crate::llm_cli::CallRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// `AgentSession` を 1 行ずつ足す file（data dir の下）。
pub const AGENT_LOG: &str = "agent-sessions.jsonl";

/// 画面の 3 つの期間（`OwnUsage::spans` の順）。
pub const SPANS: [&str; 3] = ["Today", "5-hour window", "This week"];

/// Shibubu の Terminal の前面で見た agent の session 1 つ（`AGENT_LOG` の 1 行）。
/// 例: `AgentSession { agent: "claude".into(), session: Some("e8ed…".into()), cwd: "/repo".into(), project: "p1".into(), from_ms: 1_793_575_880_958 }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    /// `domain::terminal::agent_name` の語（数えるのは claude・grok・codex）
    pub agent: String,
    /// claude = `sessions/<pid>.json` の sessionId・grok = `active_sessions.json` の session_id・codex = None（cwd と時刻で引く）
    pub session: Option<String>,
    pub cwd: String,
    pub project: String,
    /// agent を起こした時刻（unix ms）。これより前の turn は数えない（Orca で始めた会話を Shibubu で resume した時の前の分を混ぜない）
    pub from_ms: u64,
}

/// 期間 1 つの合計。`calls` は Shibubu が自分で呼んだ回数だけ（Terminal の agent は token だけを足す = 3 往復話すと 3 増える）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub calls: u32,
    /// 入力の token（cache から読んだ分も含む）
    pub input: u64,
    /// `input` のうち cache から読んだ分
    pub cached: u64,
    pub output: u64,
    /// Shibubu の呼び出しにかかった時間の和
    pub ms: u64,
    /// API 換算の和（百万分の 1 ドル・返した呼び出しの分だけ）
    pub cost_micros: u64,
    /// API 換算を返した呼び出しが 1 つでも在る（無ければ画面は API 換算の行を出さない）
    pub costed: bool,
}

/// 今週の内訳の 1 行（AI → account → Project → 何のため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// `Claude Code`・`Grok`・`Codex` 等
    pub ai: String,
    /// `AiAccount::id`（None = System default・Terminal の agent）
    pub account: Option<String>,
    /// Project の id（None = Home・作る前の案）。名前は `Generated::own_usage` が引く
    pub project: Option<String>,
    /// `purpose` の語
    pub purpose: &'static str,
    pub tally: Tally,
}

/// 1 回の数え（`count`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnUsage {
    /// `SPANS` の順（今日・5 時間の枠・今週）
    pub spans: [Tally; 3],
    /// 今週の内訳（AI・account・Project の順に並べ、同じ所は token の多い順）
    pub rows: Vec<Row>,
    /// 読めなかった物ごとの 1 行（理由と次の一手）。空 = 全部読めた
    pub problems: Vec<String>,
    /// 数えた時刻（unix 秒・`Counted 20s ago` の元）
    pub counted_at: i64,
}

/// 呼び出しの `JsonRequest::name` → 画面の「何のため」の語。
pub fn purpose(name: &str) -> &'static str {
    match name {
        "talk" => "Chat",
        crate::composer::project::PLAN_NAME => "Plans and builds",
        crate::composer::project::EDIT_NAME => "Page edits",
        "engine_interpret" | "engine_generate" | crate::engine_page::READ_NAME => "Work in apps",
        _ => "Other",
    }
}

/// Terminal の agent の「何のため」の語。
pub const TERMINAL_AGENTS: &str = "Terminal agents";

/// 3 つの期間の始まり（unix ms）。plan の窓の戻る時刻が在ればそこから数える（`Session` = 5 時間・`Weekly` = 7 日・account 全体の % と同じ枠）、
/// 無ければ 5 時間前・地域の月曜 0 時。今日 = 地域の 0 時（`offset_s` = UTC からの秒）。純関数。
/// 例: `starts(now, 32_400, None)` → `[今日の 0 時（JST）, now − 5h, 月曜の 0 時（JST）]`（ms）
pub fn starts(now_s: i64, offset_s: i64, plan: Option<&crate::llm::PlanUsage>) -> [u64; 3] {
    let resets = |name: &str| plan.and_then(|u| u.windows.iter().find(|(n, _, _)| n == name)).and_then(|(_, _, at)| *at).filter(|at| *at > now_s);
    let day = (now_s + offset_s).div_euclid(86_400);
    let today = day * 86_400 - offset_s;
    let window = resets("Session").map_or(now_s - 5 * 3600, |at| at - 5 * 3600);
    // 1970-01-01 は木曜（月曜 = 0 にすると 3）
    let week = resets("Weekly").map_or((day - (day + 3).rem_euclid(7)) * 86_400 - offset_s, |at| at - 7 * 86_400);
    [today, window, week].map(|s| (s.max(0) as u64) * 1000)
}

/// agent の記録の置き場。`Dirs::of_user()` = 利用者の env（`CLAUDE_CONFIG_DIR`・`GROK_HOME`・`CODEX_HOME`）か HOME の下・試験は手で作る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub claude: PathBuf,
    pub grok: PathBuf,
    pub codex: PathBuf,
}

impl Dirs {
    /// HOME が無ければ None（Terminal の agent は数えず 1 行）。
    pub fn of_user() -> Option<Dirs> {
        let home = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?);
        let at = |var: &str, rel: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map_or_else(|| home.join(rel), PathBuf::from);
        Some(Dirs { claude: at("CLAUDE_CONFIG_DIR", ".claude"), grok: at("GROK_HOME", ".grok"), codex: at("CODEX_HOME", ".codex") })
    }
}

/// 呼び出しの記録と agent の session を読んで数える（裏の thread で 60 秒ごと・`Counter::refresh`）。何も書かない。
/// `dirs` = agent の記録の置き場（None = Terminal の agent の分は読まず 1 行）。`starts` = `starts` の答え。
/// 保証する: 中身を持たない（`OwnUsage` は数と語だけ）・読めない物は飛ばして `problems` に 1 行（他は数える）。
/// 保証しない: 速さ（transcript が大きいと秒単位・UI の thread から呼ばない）。
pub fn count(data_dir: &Path, dirs: Option<&Dirs>, starts: [u64; 3], now_s: i64) -> OwnUsage {
    let since = starts.iter().copied().min().unwrap_or(0);
    let mut out = OwnUsage { counted_at: now_s, ..OwnUsage::default() };
    let mut uses: Vec<(u64, Row)> = Vec::new();
    match std::fs::read_to_string(data_dir.join(crate::llm_cli::CALL_LOG)) {
        Ok(text) => uses.extend(text.lines().filter_map(|l| serde_json::from_str::<CallRecord>(l).ok()).filter(|r| r.at_ms >= since).map(|r| (r.at_ms, call_row(&r)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => out.problems.push("Shibubu's AI calls couldn't be read · Recount".into()),
    }
    let sessions = agent_sessions(data_dir);
    match dirs {
        _ if sessions.is_empty() => {}
        None => out.problems.push("Terminal agents couldn't be counted · HOME isn't set".into()),
        Some(dirs) => {
            for s in &sessions {
                match agent_uses(dirs, s, since) {
                    Ok(v) => uses.extend(v),
                    Err(line) if !out.problems.contains(&line) => out.problems.push(line),
                    Err(_) => {}
                }
            }
        }
    }
    let mut rows: BTreeMap<(String, Option<String>, Option<String>, &'static str), Tally> = BTreeMap::new();
    for (at, u) in &uses {
        for (i, start) in starts.iter().enumerate() {
            if at >= start {
                add(&mut out.spans[i], &u.tally);
            }
        }
        if *at >= starts[2] {
            add(rows.entry((u.ai.clone(), u.account.clone(), u.project.clone(), u.purpose)).or_default(), &u.tally);
        }
    }
    out.rows = rows.into_iter().map(|((ai, account, project, purpose), tally)| Row { ai, account, project, purpose, tally }).collect();
    out.rows.sort_by(|a, b| (&a.ai, &a.account, &a.project).cmp(&(&b.ai, &b.account, &b.project)).then((b.tally.input + b.tally.output).cmp(&(a.tally.input + a.tally.output))));
    out
}

fn add(to: &mut Tally, t: &Tally) {
    to.calls += t.calls;
    to.input += t.input;
    to.cached += t.cached;
    to.output += t.output;
    to.ms += t.ms;
    to.cost_micros += t.cost_micros;
    to.costed |= t.costed;
}

/// 呼び出しの記録 1 行 → 内訳の 1 行（AI の語は `Llm::label` の頭から）。
fn call_row(r: &CallRecord) -> Row {
    let kind = r.label.split(" · ").next().unwrap_or_default();
    let ai = match kind.split_once('/').map_or(kind, |(k, _)| k) {
        "claude-cli" => "Claude Code".to_owned(),
        "grok" => "Grok".to_owned(),
        other => other.to_owned(),
    };
    Row {
        ai,
        account: r.account.clone(),
        project: r.project.clone(),
        purpose: purpose(&r.name),
        tally: Tally {
            calls: 1,
            input: r.input_tokens.unwrap_or(0),
            cached: r.cache_read_tokens.unwrap_or(0),
            output: r.output_tokens.unwrap_or(0),
            ms: r.ms,
            cost_micros: r.cost_micros.unwrap_or(0),
            costed: r.cost_micros.is_some(),
        },
    }
}

/// `AGENT_LOG` の行（同じ session は 1 つ・codex は cwd と起こした時刻で 1 つ）。読めない行は飛ばす。
fn agent_sessions(data_dir: &Path) -> Vec<AgentSession> {
    let text = std::fs::read_to_string(data_dir.join(AGENT_LOG)).unwrap_or_default();
    let mut out: Vec<AgentSession> = Vec::new();
    for s in text.lines().filter_map(|l| serde_json::from_str::<AgentSession>(l).ok()) {
        let same = |o: &AgentSession| o.agent == s.agent && o.session == s.session && (s.session.is_some() || o.cwd == s.cwd && o.from_ms.abs_diff(s.from_ms) < 60_000);
        if !out.iter().any(same) {
            out.push(s);
        }
    }
    out
}

/// Terminal の前面の agent（pid）から session を引く（UI の thread・小さな JSON を 1 つ読むだけ）。数えない agent・まだ書かれていない時は None（呼ぶ側は後でもう 1 回）。
/// 例（`Generated::panel_status`）: `own_usage::resolve("claude", 6307, &tab.cwd, project_id, now_ms, &dirs)`
pub fn resolve(agent: &str, pid: i32, cwd: &str, project: &str, now_ms: u64, dirs: &Dirs) -> Option<AgentSession> {
    let json = |p: PathBuf| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let (session, cwd, from_ms) = match agent {
        "claude" => {
            let v = json(dirs.claude.join("sessions").join(format!("{pid}.json")))?;
            (Some(v["sessionId"].as_str()?.to_owned()), v["cwd"].as_str().unwrap_or(cwd).to_owned(), v["startedAt"].as_u64().unwrap_or(now_ms))
        }
        "grok" => {
            let v = json(dirs.grok.join("active_sessions.json"))?;
            let s = v.as_array()?.iter().find(|s| s["pid"].as_i64() == Some(i64::from(pid)))?;
            (Some(s["session_id"].as_str()?.to_owned()), s["cwd"].as_str().unwrap_or(cwd).to_owned(), s["opened_at"].as_str().and_then(ms_of).unwrap_or(now_ms))
        }
        "codex" => (None, cwd.to_owned(), now_ms),
        _ => return None,
    };
    Some(AgentSession { agent: agent.to_owned(), session, cwd, project: project.to_owned(), from_ms })
}

/// RFC3339 → unix ms（秒の精度）。
fn ms_of(s: &str) -> Option<u64> {
    crate::google::parse_rfc3339(s).and_then(|t| u64::try_from(t).ok()).map(|t| t * 1000)
}

/// agent の session 1 つの token（`since` と `from_ms` より後の turn だけ）。Err = 画面の 1 行。
fn agent_uses(dirs: &Dirs, s: &AgentSession, since: u64) -> Result<Vec<(u64, Row)>, String> {
    let since = since.max(s.from_ms);
    let row = |ai: &str, at: u64, input: u64, cached: u64, output: u64| {
        (at, Row { ai: ai.to_owned(), account: None, project: Some(s.project.clone()), purpose: TERMINAL_AGENTS, tally: Tally { input, cached, output, ..Tally::default() } })
    };
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    match (s.agent.as_str(), s.session.as_deref()) {
        // ~/.claude/projects/<cwd を変えた名前>/<session>.jsonl の assistant の行の `message.usage`（同じ request は streaming で何行も出る = 最後の 1 行だけ）
        ("claude", Some(id)) => {
            let Some(path) = find_in_dirs(&dirs.claude.join("projects"), &format!("{id}.jsonl")) else { return Ok(Vec::new()) };
            let text = std::fs::read_to_string(path).map_err(|_| "Claude Code's history in a Shibubu terminal couldn't be read · Recount".to_owned())?;
            let mut last: BTreeMap<String, (u64, Row)> = BTreeMap::new();
            for (i, l) in text.lines().enumerate().filter(|(_, l)| l.contains("\"usage\"")) {
                let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
                let Some(at) = v["timestamp"].as_str().and_then(ms_of) else { continue };
                let u = &v["message"]["usage"];
                if v["type"] != "assistant" || at < since || !u.is_object() {
                    continue;
                }
                let key = v["requestId"].as_str().or(v["message"]["id"].as_str()).map_or_else(|| i.to_string(), str::to_owned);
                let (read, made) = (n(u, "cache_read_input_tokens"), n(u, "cache_creation_input_tokens"));
                last.insert(key, row("Claude Code", at, n(u, "input_tokens") + read + made, read, n(u, "output_tokens")));
            }
            Ok(last.into_values().collect())
        }
        // ~/.grok/sessions/<cwd>/<session>/usage.json の turn ごとの数（中身の無い file）
        ("grok", Some(id)) => {
            let Some(dir) = find_in_dirs(&dirs.grok.join("sessions"), id) else { return Ok(Vec::new()) };
            let text = match std::fs::read_to_string(dir.join("usage.json")) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(_) => return Err("Grok's history in a Shibubu terminal couldn't be read · Recount".into()),
            };
            let v: Value = serde_json::from_str(&text).unwrap_or_default();
            Ok(v["turns"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| Some((t["endedAt"].as_str().and_then(ms_of)?, t)))
                .filter(|(at, _)| *at >= since)
                .map(|(at, t)| row("Grok", at, n(t, "inputTokens"), n(t, "cachedReadTokens"), n(t, "outputTokens")))
                .collect())
        }
        ("codex", None) => codex_uses(&dirs.codex, s, since, &row),
        _ => Ok(Vec::new()),
    }
}

/// `root/<dir>/<name>` を dir の一覧から探す（claude の dir 名は cwd の記号を `-` に替えた物・grok は %-encode = どちらも名前を組み立てず探す）。
// ponytail: 60 秒ごとに dir の一覧を読み直す（数百 dir なら ms）・増えたら session ごとに見つけた path を覚える
fn find_in_dirs(root: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(root).ok()?.flatten().map(|e| e.path().join(name)).find(|p| p.exists())
}

/// codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` のうち、`session_meta` の cwd が同じで、Shibubu が見た時刻に動いていた物。
/// `token_count` の `info.total_token_usage`（累積）の差を、その行の時刻に数える。
// ponytail: codex は pid から session を引く記録が無い = 同じ cwd で同じ時刻に Shibubu の外で動いた codex は混ざり得る（codex が pid を書くようになったら resolve で引く）
fn codex_uses(codex: &Path, s: &AgentSession, since: u64, row: &dyn Fn(&str, u64, u64, u64, u64) -> (u64, Row)) -> Result<Vec<(u64, Row)>, String> {
    let root = codex.join("sessions");
    let seen_day = (s.from_ms / 1000 / 86_400) as i64;
    let mut out = Vec::new();
    for day in seen_day - 1..=seen_day + 1 {
        let ymd = crate::google::rfc3339_utc((day.max(0) * 86_400) as u64);
        let dir = root.join(&ymd[0..4]).join(&ymd[5..7]).join(&ymd[8..10]);
        for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let mut lines = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok());
            let Some(meta) = lines.next().filter(|v| v["type"] == "session_meta") else { continue };
            let started = meta["payload"]["timestamp"].as_str().or(meta["timestamp"].as_str()).and_then(ms_of).unwrap_or(u64::MAX);
            if meta["payload"]["cwd"].as_str() != Some(s.cwd.as_str()) || started > s.from_ms {
                continue;
            }
            let (mut prev, mut mine, mut last_at) = ([0u64; 3], Vec::new(), started);
            for v in lines {
                let Some(at) = v["timestamp"].as_str().and_then(ms_of) else { continue };
                last_at = at;
                let total = &v["payload"]["info"]["total_token_usage"];
                if v["payload"]["type"] != "token_count" || !total.is_object() {
                    continue;
                }
                let now = [n(total, "input_tokens"), n(total, "cached_input_tokens"), n(total, "output_tokens")];
                let d = [0, 1, 2].map(|i| now[i].saturating_sub(prev[i]));
                prev = now;
                if at >= since && d.iter().any(|x| *x > 0) {
                    mine.push(row("Codex", at, d[0], d[1], d[2]));
                }
            }
            // Shibubu が見た時に動いていた session だけ（見る前に終わった session は他所の物）
            if last_at + 60_000 >= s.from_ms {
                out.extend(mine);
            }
        }
    }
    Ok(out)
}

fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// 画面の token の数（`950`・`12.4k`・`3.1M`）。
pub fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// 画面の時間（`0s`・`42s`・`3m 20s`・`1h 5m`）。
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// 画面の API 換算（`$0.42`・1 セント未満は `<$0.01`）。
pub fn dollars(micros: u64) -> String {
    match micros {
        0 => "$0.00".into(),
        1..=9_999 => "<$0.01".into(),
        _ => format!("${:.2}", micros as f64 / 1e6),
    }
}

/// 下の帯の数え手（app.rs の 1 欄）: 裏の thread で 60 秒ごとに `count`・Terminal の前面の agent の session を `AGENT_LOG` に足す。
/// 新しくした理由: 数えた答え・裏の job・pid を引き直す時刻を 1 か所に持つ（generated.rs と ui は試験の crate に入るので、この module を名指せない）。
#[derive(Default)]
pub struct Counter {
    latest: Option<OwnUsage>,
    job: Option<Receiver<OwnUsage>>,
    /// 最後に数え始めた時刻（unix 秒・0 = まだ）
    at: i64,
    /// pid → (最後に引いた時刻, 書いた session の鍵)
    seen: BTreeMap<i32, (Instant, Option<String>)>,
}

impl Counter {
    /// 数え直す。`force` = false は前に数え始めてから 60 秒未満なら何もしない（毎 frame 呼んでよい）・true は panel の `Recount`。同時に 1 本。
    /// `plan` = account 全体の % を読めていればその窓（5 時間・今週の始まりを揃える）。`wake` = 数え終えたら画面を起こす。戻り値 = 数え始めた。
    /// 例（app.rs の frame）: `self.own.refresh(&self.data_dir, plan, now, false, move || c.request_repaint())`
    pub fn refresh(&mut self, data_dir: &Path, plan: Option<&crate::llm::PlanUsage>, now: i64, force: bool, wake: impl Fn() + Send + 'static) -> bool {
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(u) => (self.latest, self.job) = (Some(u), None),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => self.job = None,
            }
        }
        if !force && self.at != 0 && now - self.at < 60 {
            return false;
        }
        let (dir, plan) = (data_dir.to_path_buf(), plan.cloned());
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("own-usage".into()).spawn(move || {
            let st = starts(now, crate::google::local_offset().unwrap_or(0), plan.as_ref());
            let _ = tx.send(count(&dir, Dirs::of_user().as_ref(), st, now));
            wake();
        });
        if spawned.is_err() {
            return false;
        }
        (self.job, self.at) = (Some(rx), now);
        true
    }

    /// Terminal の前面の agent（`Generated::take_agents` = (pid, 名前, tab の cwd, Project の id)）の session を引き、新しい session なら `AGENT_LOG` に 1 行。
    /// 同じ pid は 30 秒ごとに引き直す（claude の起動の直後でまだ書かれていない・`/clear` で session が替わった）。書けなくても止めない（数えるためだけ）。
    pub fn see(&mut self, data_dir: &Path, fronts: &[(i32, &'static str, String, String)], now_ms: u64) {
        for (pid, agent, cwd, project) in fronts {
            if self.seen.get(pid).is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(30)) {
                continue;
            }
            let last = self.seen.get(pid).and_then(|(_, key)| key.clone());
            let found = Dirs::of_user().and_then(|d| resolve(agent, *pid, cwd, project, now_ms, &d));
            let key = found.as_ref().map(|s| s.session.clone().unwrap_or_else(|| format!("pid:{pid}")));
            self.seen.insert(*pid, (Instant::now(), key.clone().or_else(|| last.clone())));
            let Some(s) = found.filter(|_| key != last) else { continue };
            let line = serde_json::to_string(&s).unwrap_or_default() + "\n";
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(data_dir.join(AGENT_LOG)).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
    }

    /// 帯の語と panel の文字。`project` = Project の id → 名前（消した Project は None）・`account` = account の id → 画面の語（`provider_rows` の語）。
    /// 例（app.rs）: `self.own.panel(&|id| name_in_sidebar(id), &|id| provider_label(id), now)`
    pub fn panel(&self, project: &dyn Fn(&str) -> Option<String>, account: &dyn Fn(&str) -> Option<String>, now: i64) -> crate::ui::shell::OwnPanel {
        let Some(u) = &self.latest else {
            return crate::ui::shell::OwnPanel { counted: "Counting…".into(), ..Default::default() };
        };
        let requests = |n: u32| if n == 1 { "1 request".to_owned() } else { format!("{n} requests") };
        let row = |name: &str, f: &dyn Fn(&Tally) -> String| [name.to_owned(), f(&u.spans[0]), f(&u.spans[1]), f(&u.spans[2])];
        let mut table = vec![
            [String::new(), SPANS[0].into(), SPANS[1].into(), SPANS[2].into()],
            row("Requests", &|t: &Tally| t.calls.to_string()),
            row("Tokens in", &|t: &Tally| tokens(t.input)),
            row("Tokens out", &|t: &Tally| tokens(t.output)),
            row("Time", &|t: &Tally| duration(t.ms)),
        ];
        let costed = u.spans.iter().any(|t| t.costed);
        if costed {
            table.push(row("API estimate", &|t: &Tally| dollars(t.cost_micros)));
        }
        let mut notes = vec!["Requests are what Shibubu asked the AI · agents in Shibubu's terminals add tokens only".to_owned()];
        if costed {
            notes.push("API estimate = what these tokens would cost on the API · not a bill".into());
        }
        // 数の 1 行（回数・token・API 換算）。行と account の合計が同じ形
        let nums = |t: &Tally| {
            let mut v = Vec::new();
            if t.calls > 0 {
                v.push(requests(t.calls));
            }
            v.push(format!("{} in · {} out", tokens(t.input), tokens(t.output)));
            if t.costed {
                v.push(dollars(t.cost_micros));
            }
            v.join(" · ")
        };
        let mut groups: Vec<(String, String, Tally, Vec<(String, String)>)> = Vec::new();
        for r in &u.rows {
            // ACCOUNTS-1101 2 段目（作る物 6）: 見出し = account の語だけ（`provider_rows` の語 = `Claude · work`・`Grok`）。前は AI の名前を前に足して
            // `Grok · Grok`・`Claude Code · Claude plan · plan-1` と 2 回出ていた。account の無い行（System default・Terminal の agent）は AI の名前
            let head = match r.account.as_deref() {
                None => r.ai.clone(),
                Some(id) => account(id).map_or_else(|| format!("{} · {id}", r.ai), |l| l.split(" · billed").next().unwrap_or_default().to_owned()),
            };
            let place = match r.project.as_deref() {
                None => "Home".to_owned(),
                Some(id) => project(id).unwrap_or_else(|| "An archived or removed project".into()),
            };
            let line = (format!("{place} › {}", r.purpose), nums(&r.tally));
            // PROVIDER-ICONS-1101: 見出しの印の key（AI の名前 = `AgentKind::label` → その cmd・表に無い名前は小文字のまま = 頭文字の丸）
            let key = crate::domain::terminal::AGENT_KINDS.iter().chain(&crate::domain::terminal::LOCAL_KINDS).find(|k| k.label == r.ai).map_or_else(|| r.ai.to_lowercase(), |k| k.cmd.to_owned());
            match groups.last_mut() {
                Some((h, _, sum, rows)) if *h == head => {
                    add(sum, &r.tally);
                    rows.push(line);
                }
                _ => groups.push((head, key, r.tally, vec![line])),
            }
        }
        // account ごとの合計（行が 2 つ以上の account だけ・先頭に `Total`）
        let groups = groups
            .into_iter()
            .map(|(head, key, sum, mut rows)| {
                if rows.len() > 1 {
                    rows.insert(0, ("Total".to_owned(), nums(&sum)));
                }
                (head, key, rows)
            })
            .collect();
        let ago = (now - u.counted_at).max(0);
        let counted = match (self.job.is_some(), ago) {
            (true, _) => "Counting…".to_owned(),
            (_, 0..=59) => "Counted just now".to_owned(),
            (_, 60..=3599) => format!("Counted {}m ago", ago / 60),
            _ => format!("Counted {}h ago", ago / 3600),
        };
        crate::ui::shell::OwnPanel { table, notes, groups, counted, problems: u.problems.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(at_ms: u64, name: &str, project: Option<&str>, input: u64, output: u64, cost: Option<u64>) -> CallRecord {
        CallRecord {
            at_ms,
            label: "claude-cli/2.1.316".into(),
            name: name.into(),
            ms: 2_000,
            exit_code: Some(0),
            outcome: "ok".into(),
            stopped_after_ms: None,
            input_tokens: Some(input),
            prompt_chars: Some(10),
            output_tokens: Some(output),
            cache_read_tokens: Some(input / 2),
            cache_creation_tokens: None,
            cost_micros: cost,
            account: None,
            project: project.map(str::to_owned),
            session: Some("s".into()),
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-own-usage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Done の形: 3 往復の会話（talk 3 回）が今日・5 時間・今週の全部に 3 回として入り、前の週の 1 回は入らない。Terminal の agent は回数に入らず token だけ。
    #[test]
    fn three_talks_count_as_three_calls_and_terminal_agents_add_only_tokens() {
        let (data, home) = (dir("data"), dir("home"));
        let now_s = 1_793_600_000;
        let st = starts(now_s, 32_400, None);
        let at = now_s as u64 * 1000 - 60_000;
        let mut lines: Vec<String> = (0..3).map(|i| serde_json::to_string(&call(at + i, "talk", None, 1_000, 100, Some(5_000))).unwrap()).collect();
        lines.push(serde_json::to_string(&call(st[2] - 1, "talk", None, 9, 9, None)).unwrap());
        lines.push(serde_json::to_string(&call(at, crate::composer::project::EDIT_NAME, Some("p1"), 500, 50, None)).unwrap());
        std::fs::write(data.join(crate::llm_cli::CALL_LOG), lines.join("\n")).unwrap();
        // Shibubu の Terminal の claude の session（streaming で同じ request が 2 行 = 1 回に数える）と、Orca の session（記録に無い = 数えない）
        let proj = home.join(".claude/projects/-repo");
        std::fs::create_dir_all(&proj).unwrap();
        let ts = crate::google::rfc3339_utc(now_s as u64 - 30);
        let usage = r#"{"input_tokens":2,"cache_read_input_tokens":300,"cache_creation_input_tokens":0,"output_tokens":40}"#;
        let line = |req: &str| format!(r#"{{"type":"assistant","timestamp":"{ts}","requestId":"{req}","message":{{"id":"m","content":[{{"type":"text","text":"secret"}}],"usage":{usage}}}}}"#);
        std::fs::write(proj.join("mine.jsonl"), [line("r1"), line("r1"), line("r2")].join("\n")).unwrap();
        std::fs::write(proj.join("orca.jsonl"), line("r9")).unwrap();
        let s = AgentSession { agent: "claude".into(), session: Some("mine".into()), cwd: "/repo".into(), project: "p1".into(), from_ms: at - 600_000 };
        std::fs::write(data.join(AGENT_LOG), [serde_json::to_string(&s).unwrap(), serde_json::to_string(&s).unwrap()].join("\n")).unwrap();
        let dirs = Dirs { claude: home.join(".claude"), grok: home.join(".grok"), codex: home.join(".codex") };
        let u = count(&data, Some(&dirs), st, now_s);
        assert!(u.problems.is_empty(), "{:?}", u.problems);
        for span in u.spans {
            assert_eq!(span.calls, 4, "talk 3 回 + Edit 1 回（前の週の 1 回と Terminal の agent は回数に入らない）");
            assert_eq!(span.input, 3 * 1_000 + 500 + 2 * 302, "Terminal の agent は request 2 つ分（同じ request の 2 行を 2 重に数えない・Orca の session は入らない）");
            assert_eq!(span.output, 3 * 100 + 50 + 2 * 40);
            assert_eq!((span.cost_micros, span.costed), (15_000, true));
        }
        let chat = u.rows.iter().find(|r| r.purpose == "Chat").expect("会話の行");
        assert_eq!((chat.ai.as_str(), chat.project.as_deref(), chat.tally.calls), ("Claude Code", None, 3));
        assert!(u.rows.iter().any(|r| r.purpose == TERMINAL_AGENTS && r.project.as_deref() == Some("p1") && r.tally.output == 80));
        assert!(!format!("{u:?}").contains("secret"), "中身を持たない");
        let _ = (std::fs::remove_dir_all(&data), std::fs::remove_dir_all(&home));
    }

    #[test]
    fn spans_follow_the_plan_windows_or_fall_back_to_the_local_week() {
        // 2026-11-02 12:00 JST = 2026-11-02T03:00Z（月曜）
        let now = crate::google::parse_rfc3339("2026-11-02T03:00:00Z").unwrap();
        let [today, window, week] = starts(now, 32_400, None);
        assert_eq!(today / 1000, crate::google::parse_rfc3339("2026-11-01T15:00:00Z").unwrap() as u64, "JST の 0 時");
        assert_eq!(window / 1000, (now - 5 * 3600) as u64);
        assert_eq!(week, today, "月曜の 0 時");
        let plan = crate::llm::PlanUsage { windows: vec![("Session".into(), 40, Some(now + 3600)), ("Weekly".into(), 10, Some(now + 86_400))], checked_at: now };
        let [_, window, week] = starts(now, 32_400, Some(&plan));
        assert_eq!((window / 1000, week / 1000), ((now + 3600 - 5 * 3600) as u64, (now + 86_400 - 7 * 86_400) as u64), "account の % と同じ枠");
        assert_eq!((tokens(950), tokens(12_400), tokens(3_100_000)), ("950".into(), "12.4k".into(), "3.1M".into()));
        assert_eq!((duration(42_000), duration(200_000), dollars(4_000), dollars(420_000)), ("42s".into(), "3m 20s".into(), "<$0.01".into(), "$0.42".into()));
    }

    /// ACCOUNTS-1101 2 段目（作る物 6）: 内訳の見出しは account の語だけ（AI の名前を 2 回並べない）・account の無い行は AI の名前・行が 2 つ以上の account の先頭に `Total`。
    /// 書いただけ・未実行（owner 186 / 187）。負の対照: 見出しを前の `{ai} · {label}` に戻す → `Claude · work` の assert で赤・`Total` を足さない → 赤。
    #[test]
    fn accounts2_breakdown_is_by_account_with_a_total() {
        let t = |calls: u32, input: u64| Tally { calls, input, output: 10, ..Tally::default() };
        let row = |ai: &str, account: Option<&str>, project: Option<&str>, tally: Tally| Row { ai: ai.into(), account: account.map(str::to_owned), project: project.map(str::to_owned), purpose: "Chat", tally };
        let u = OwnUsage {
            rows: vec![row("Claude Code", None, None, t(1, 100)), row("Claude Code", Some("plan-1"), None, t(2, 200)), row("Claude Code", Some("plan-1"), Some("p1"), t(3, 300)), row("Grok", Some("grok"), None, t(1, 50))],
            counted_at: 100,
            ..OwnUsage::default()
        };
        let c = Counter { latest: Some(u), ..Counter::default() };
        let label = |id: &str| match id {
            "plan-1" => Some("Claude · work".to_owned()),
            "grok" => Some("Grok".to_owned()),
            _ => None,
        };
        let panel = c.panel(&|_| Some("Reading log".into()), &label, 100);
        let heads: Vec<&str> = panel.groups.iter().map(|(h, _, _)| h.as_str()).collect();
        assert_eq!(heads, ["Claude Code", "Claude · work", "Grok"], "見出しが account ごとでない・AI の名前を 2 回並べた");
        // PROVIDER-ICONS-1101: 見出しの印の key = AI の名前の cmd（負の対照〈未実行〉: key を r.ai のままにする → "Claude Code" で赤）
        let keys: Vec<&str> = panel.groups.iter().map(|(_, k, _)| k.as_str()).collect();
        assert_eq!(keys, ["claude", "claude", "grok"], "見出しの印の key が AI の cmd でない");
        let work = &panel.groups[1].2;
        assert_eq!(work[0], ("Total".to_owned(), "5 requests · 500 in · 20 out".to_owned()), "account の合計が無い・数が違う");
        assert_eq!(work.len(), 3, "合計の後に Project の行が 2 つ無い");
        assert!(panel.groups[0].2.iter().chain(&panel.groups[2].2).all(|(l, _)| l != "Total"), "行が 1 つの account に合計を足した");
    }
}
