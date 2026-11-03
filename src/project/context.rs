//! W9: Terminal から Project を読む口（`shibubu project pages|context|surfaces [--json]`・YCM-050・051・052・SPEC v15 §Integration）。
//! .app に入る binary は GUI の `shibubu` 1 つだけなので、CLI も同じ binary: `main()` が eframe を起こす前に `cli_main` を呼び、
//! argv の 1 番が `project` でなければ None で GUI に返す（Project の Terminal の中と、起動の引数でない argv〈`--list` 等〉では返さず `Usage`・`decide`）。Project と保存先は、Shibubu が PTY の子に渡す環境変数
//! （`terminal::PROJECT_ENV`・`terminal::DATA_DIR_ENV`・`TerminalConfig::project_cli`）から決める。
//! 読むのは SQLite だけで、書かない（`Store::open_read_only`）。読む表は `projects`（今 = head の定義）・`terminal_sessions`・`surface_receipts`・`runs`（L7）。
//! L7-AGENT: 書く動詞は `ask` の 1 つだけ（`shibubu project ask "<一言>"`）。一言を runs 表の承認待ちの行に置くだけで、Project は変えない
//! （画面の Shibubu が拾って承認の card を出し、人が承認した時だけ、人が欄に打ったのと同じ道で読む・`Generated::poll_asks`）。
//! これは製品の自己申告: 合否は試験の側の oracle（PID は `ps`・窓は CGWindowList・Page は画面の AX の木）と照らす（YC_WAVES W9・YC_ACCEPTANCE §5）。
//! SHIBUBU-SKILL 段 2: `shibubu skills list|get <name>` = Terminal の agent 向けの案内（`docs/skills/shibubu-cli.md` の `GUIDE_END` より前を binary に埋めた物）。
//! DB を開かず、Project の Terminal の外でも出す（agent が最初に読む 1 枚・Orca の `orca skills get orca-cli` と同じ形）。
//! 図: docs/diagrams.md §31。

use crate::domain::project::{
    PageDefinition, ProjectDefinitionV1, ProjectResource, ReceiptKind, ReceiptState,
    SurfaceBackendKind, SurfaceReceipt, SurfaceTarget,
};
use crate::domain::action::InvocationOrigin;
use crate::domain::run::RunStatus;
use crate::storage::{Store, StoreError, Table};
use crate::terminal::{self, SessionInfo, SessionStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

/// 出力の JSON の版（`ProjectContext::schema`）。欄の名前・意味を変える時は数を上げる。読む側（試験の `tally.py`・agent）は知らない版を拒否する。
/// L7: `runs`・`waiting` を足したので /2（/1 を読む側は知らない欄を拒否する）。
/// SHIBUBU-SKILL 段 2: 面の行に `kind`・`app`・`document`・`url` を足したが /2 のまま（足しただけで既存の欄の名前と意味は変えない・
/// 読む側は同じ binary の `ProjectContext`・operator 13:1x「driver の W9 は /2 の形のまま・新しい field の追加だけ」）。
pub const SCHEMA: &str = "shibubu.project/2";

/// `CliError::Usage` の後ろに付ける 1 行。末尾の `shibubu skills get shibubu-cli` は、env も panel の 1 行も見えない agent
/// （`claude -p` の Bash だけ）が `shibubu --help` か打ち間違いから案内に辿り着く唯一の口なので、名前まで字のまま書く。
pub const USAGE: &str = "Usage: shibubu project pages|context|surfaces [--json] · shibubu project ask \"<change>\" [--json] · shibubu skills list [--json] · Agents start with: shibubu skills get shibubu-cli";

/// 段 2: `shibubu skills get` が知っている案内の名前（今は 1 つ）。
pub const GUIDE_NAME: &str = "shibubu-cli";

/// 段 2: 案内の file の、agent に出す所の終わりの印（この行から下は出さない。設計の notes は include_str! しない別の file `docs/skills/shibubu-cli-notes.md`・B79）。
pub const GUIDE_END: &str = "<!-- end of guide";

/// 段 2: 案内の file 全部（binary に埋める = .app の中でも file の path に依らず、binary と版が揃う）。出すのは `guide()` の前半だけ。
const GUIDE_FILE: &str = include_str!("../../docs/skills/shibubu-cli.md");

/// L7-AGENT: `ask` の一言の action の id（runs 表の行の step・`ApprovalRequest::action_id`・承認の記録の action_ref）。
pub const ASK_ACTION: &str = "project.edit";

/// L7-AGENT: `ask` の一言の上限の字数（承認の card に全部出す・それより長い物は拒否して打ち直してもらう）。
pub const ASK_MAX_CHARS: usize = 2000;

/// 何を読むか（argv の 2 番）。名前は YC_WAVES の W9 の行のまま変えない（W12a の列が同じ名前で合わせる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `pages`: Page の一覧（画面の Page タブの順・id・route・題）と、Page ごとに置いた Terminal と面。
    Pages,
    /// `context`: Project 全体（Page・全 Terminal・全面・resources）。
    Context,
    /// `surfaces`: 面の一覧（起こし方・最新の読み戻し・繋いだ窓）。
    Surfaces,
    /// L7-AGENT `ask "<一言>"`: 一言を runs 表の承認待ちの行に置く（Project は変えない・`ask`）。一言は残りの引数を空白 1 つでつないだ物。
    /// Wave: L7
    Ask { text: String },
    /// SHIBUBU-SKILL 段 2 `skills list` / `skills get <name>`: Terminal の agent 向けの案内（None = list・Some = get）。
    /// argv の 1 番が `skills`（`project` の下ではない = Project の外でも読める・DB を開かない）。
    Skills { name: Option<String> },
}

/// 出し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 既定: 人が読む 1 行ずつ。形は約束しない（機械は `--json` を使う）。
    Text,
    /// `--json`: 1 つの JSON object + 改行。形は `SCHEMA` で約束する。
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub format: Format,
}

/// argv（0 番は実行 file）を読む。1 番が `project` でなければ None（GUI を起こす。Finder / `open` が渡す `-psn_…` も None。
/// ただし Project の Terminal の中と、起動の引数でない argv では `cli_main` が None を `Usage` にする・`decide`）。
/// 例: `parse(&["shibubu".into(), "project".into(), "pages".into(), "--json".into()])` → `Some(Ok(Invocation { command: Command::Pages, format: Format::Json }))`
/// 保証する: 知らない command・引数は `Usage`（黙って既定に読み替えない）。`--json` は command の後ならどの位置でもよい。
/// 段 2: 1 番が `skills` も CLI（`parse_skills`）。`ask --help` / `-h` は一言にせず `Usage`（runs に行を置かない）。
pub fn parse(args: &[String]) -> Option<Result<Invocation, CliError>> {
    match args.get(1).map(String::as_str) {
        Some("project") => {}
        Some("skills") => return Some(parse_skills(&args[2..])),
        _ => return None,
    }
    if args.get(2).map(String::as_str) == Some("ask") {
        let json = args[3..].iter().any(|a| a == "--json");
        // 段 2: 1 つの引数として来た `--help` / `-h` / `--xxx` は一言にしない（承認の card に「--help」を出さない・runs に行を置かない）
        if let Some(flag) = args[3..].iter().find(|a| *a != "--json" && (a.starts_with("--") || *a == "-h")) {
            return Some(Err(CliError::Usage(match flag.as_str() {
                "--help" | "-h" => "shibubu project ask \"<change>\" shows the change to the person in the Shibubu window. Nothing changes until they approve it".into(),
                other => format!("Unknown argument: {other}"),
            })));
        }
        let text = args[3..].iter().filter(|a| *a != "--json").map(String::as_str).collect::<Vec<_>>().join(" ").trim().to_owned();
        if text.is_empty() {
            return Some(Err(CliError::Usage("Say what to change, for example: shibubu project ask \"add a Tests page\"".into())));
        }
        if text.chars().count() > ASK_MAX_CHARS {
            return Some(Err(CliError::Usage(format!("The change is longer than {ASK_MAX_CHARS} characters. Say it shorter"))));
        }
        let format = if json { Format::Json } else { Format::Text };
        return Some(Ok(Invocation { command: Command::Ask { text }, format }));
    }
    let command = match args.get(2).map(String::as_str) {
        Some("pages") => Command::Pages,
        Some("context") => Command::Context,
        Some("surfaces") => Command::Surfaces,
        Some(other) => return Some(Err(CliError::Usage(format!("Unknown command: {other}")))),
        None => return Some(Err(CliError::Usage("No command given".into()))),
    };
    let mut format = Format::Text;
    for a in &args[3..] {
        match a.as_str() {
            "--json" => format = Format::Json,
            other => return Some(Err(CliError::Usage(format!("Unknown argument: {other}")))),
        }
    }
    Some(Ok(Invocation { command, format }))
}

/// 段 2: `skills` の後ろ。`list [--json]`（案内の名前と 1 行の説明）・`get <name>`（案内の本文・`--json` は無い）。
/// 例: `parse_skills(&["get".into(), "shibubu-cli".into()])` → `Ok(Invocation { command: Command::Skills { name: Some("shibubu-cli".into()) }, format: Format::Text })`
/// 保証する: 知らない command・引数・名前の無い `get` は `Usage`（無い名前は `render_skills` が拒否する）。
fn parse_skills(rest: &[String]) -> Result<Invocation, CliError> {
    let usage = |why: String| Err(CliError::Usage(why));
    let skills = |name: Option<&String>, format| Ok(Invocation { command: Command::Skills { name: name.cloned() }, format });
    let Some((command, args)) = rest.split_first() else {
        return usage("No command given".into());
    };
    match (command.as_str(), args) {
        ("list", []) => skills(None, Format::Text),
        ("list", [json]) if json == "--json" => skills(None, Format::Json),
        ("get", []) => usage(format!("Say which guide, for example: shibubu skills get {GUIDE_NAME}")),
        ("get", [name]) => skills(Some(name), Format::Text),
        ("list", [extra, ..]) | ("get", [_, extra, ..]) => usage(format!("Unknown argument: {extra}")),
        (other, _) => usage(format!("Unknown command: {other}")),
    }
}

/// 段 2: 案内の本文（`GUIDE_FILE` の `GUIDE_END` より前）。印が無ければ空（印から下を出さない）。
pub fn guide() -> &'static str {
    GUIDE_FILE.split_once(GUIDE_END).map_or("", |(head, _)| head)
}

/// 段 2: 案内の 1 行の説明（本文の頭の `description:` の値・`>-` の折り返しを空白 1 つでつないだ物）。
pub fn guide_description() -> String {
    let mut lines = guide().lines().skip_while(|l| !l.starts_with("description:"));
    let first = lines.next().unwrap_or("").trim_start_matches("description:").trim();
    let first = if first.starts_with(['>', '|']) { "" } else { first };
    std::iter::once(first)
        .chain(lines.take_while(|l| l.starts_with(' ')).map(str::trim))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 段 2: `skills` の出力。None = list（Text は名前と説明の 1 行 + 読み方の 1 行・Json は `{schema, skills: [{name, description}]}` の 1 行）・
/// Some(`GUIDE_NAME`) = 本文（改行 1 つで終わる）・それ以外の名前は `Usage`（`There's no guide named …`）。
/// 例: `print!("{}", render_skills(Some("shibubu-cli"), Format::Text)?)`
pub fn render_skills(name: Option<&str>, format: Format) -> Result<String, CliError> {
    match name {
        None => Ok(match format {
            Format::Json => format!(
                "{}\n",
                serde_json::json!({ "schema": SCHEMA, "skills": [{ "name": GUIDE_NAME, "description": guide_description() }] })
            ),
            Format::Text => format!("{GUIDE_NAME}  {}\nRead one with: shibubu skills get <name>\n", guide_description()),
        }),
        Some(GUIDE_NAME) => Ok(format!("{}\n", guide().trim_end())),
        Some(other) => Err(CliError::Usage(format!(
            "There's no guide named {other}. See the guides with: shibubu skills list"
        ))),
    }
}

/// `cli_main` の分かれ目（`project` は `terminal::PROJECT_ENV` の値・試験は値で呼ぶ）。`parse` が None（GUI）でも、Project の Terminal の
/// 中（値が在って空でない）なら `Usage` にする: その Terminal の PATH の先頭はこの binary で、子は保存先も持つので、打ち間違い
/// （`shibubu pages --json`・`shibubu --help`）が 2 つ目の Shibubu を同じ DB で起こし、起動の後始末（`Generated::start`）が動いている
/// Terminal の記録を Unavailable に書き換えてしまう（W9 の反証）。Terminal の外（値が無い・空）でも、GUI に返すのは残りの argv が
/// 起動の引数（`first_non_launch_arg`）だけの時: 窓は前面を取るので、試験の binary と取り違えた `--list`（10-22 yc-s10 の
/// `deps/shibubu-<hash> --list` が S13 の実機を止めた）・`--help`・打ち間違いでは開かず `Usage`。
fn decide(args: &[String], project: Option<OsString>) -> Option<Result<Invocation, CliError>> {
    match parse(args) {
        None if project.is_some_and(|v| !v.is_empty()) => Some(Err(CliError::Usage(
            "Shibubu doesn't open a second window from a project's terminal. Use the Shibubu window that is open".into(),
        ))),
        None => first_non_launch_arg(args.get(1..).unwrap_or_default()).map(|a| {
            Err(CliError::Usage(format!(
                "Shibubu doesn't open with the argument {a}. Open it with shibubu alone or with Shibubu.app"
            )))
        }),
        p => p,
    }
}

/// argv（0 番の後）のうち、macOS が app に渡す起動の引数でない最初の 1 つ（None = 全部が起動の引数 = 窓を開いてよい）。
/// 起動の引数 = `-psn_…`（LaunchServices）と NSArgumentDomain の `-名前 値`（`open --args -NSAppSleepDisabled YES`・`scripts/qa/bg/run.sh`）。
/// 値が `--` で始まる組（libtest の `-q --list`）は起動の引数にしない。
fn first_non_launch_arg(rest: &[String]) -> Option<&str> {
    let mut i = 0;
    while let Some(a) = rest.get(i) {
        if a.starts_with("-psn_") {
            i += 1;
            continue;
        }
        let named = a
            .strip_prefix('-')
            .is_some_and(|k| k.starts_with(|c: char| c.is_ascii_alphabetic()));
        match rest.get(i + 1) {
            Some(v) if named && !v.starts_with("--") => i += 2,
            _ => return Some(a),
        }
    }
    None
}

/// `main()` の最初に呼ぶ。None = GUI を起こす（Project の Terminal の外で、argv が起動の引数だけの時・`decide`）。Some(code) = CLI として終わる。
/// 例（main.rs）: `let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();`
/// `if let Some(code) = project::context::cli_main(&args) { std::process::exit(code) }`
/// 保証する: 出せた時は stdout に `render` の文字列で code 0。誤りは stdout に何も出さず（偽の空の一覧を出さない）、stderr に `CliError` の 1 行で code は `CliError::exit_code`。
/// 保証しない: 画面の Shibubu に問い合わせる事（DB を読むだけ・読んでいる間の画面の変更は次の読みで出る）。
/// Wave: W9
pub fn cli_main(args: &[String]) -> Option<i32> {
    let out = decide(args, std::env::var_os(terminal::PROJECT_ENV))?.and_then(|invocation| {
        // 段 2: 案内は DB を開かない（Project の Terminal の外でも読める）
        if let Command::Skills { name } = &invocation.command {
            return render_skills(name.as_deref(), invocation.format);
        }
        let (project_id, db) = target_from_env()?;
        // L7-AGENT: 書く動詞だけ書ける接続で（file が無ければ作らない・同じ binary の Shibubu が開いた DB なので migration は当たらない）
        if let Command::Ask { text } = &invocation.command {
            if !db.is_file() {
                return Err(CliError::Store(format!("{} doesn't exist", db.display())));
            }
            let store = Store::open(&db).map_err(|e| CliError::Store(e.to_string()))?;
            let by = InvocationOrigin::Terminal {
                terminal_id: std::env::var(terminal::TERMINAL_ENV).unwrap_or_default(),
                agent: calling_agent(),
            };
            return Ok(render_ask(&ask(&store, &project_id, text, by)?, invocation.format));
        }
        let store = Store::open_read_only(&db).map_err(|e| CliError::Store(e.to_string()))?;
        Ok(render(&read(&store, &project_id)?, invocation))
    });
    let text = match out {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{e}");
            return Some(e.exit_code());
        }
    };
    // `print!` は読む側が先に閉じた時（`| head -c 1`）に panic する。書けなかった事を 1 行で言って 1 で終わる
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|_| stdout.flush())
    {
        Ok(()) => Some(0),
        Err(e) => {
            eprintln!("Couldn't write the output ({e}). Check where the output goes, then type it again");
            Some(1)
        }
    }
}

/// 子の環境から（Project の id, `kangae.sqlite3` の path）を決める。保存先は `terminal::DATA_DIR_ENV`、無ければ `KANGAE_DATA_DIR`（.app の起動口が入れる）。
/// 例: `let (project_id, db) = target_from_env()?; let ctx = read(&Store::open_read_only(&db).map_err(..)?, &project_id)?;`
/// 保証する: `terminal::PROJECT_ENV` が無い・空は `NotInProject`（別の Project は読まない・`--project` の口は作らない・YCM-053）・保存先が絶対 path でなければ `Store`。
pub fn target_from_env() -> Result<(String, PathBuf), CliError> {
    target_from(
        std::env::var_os(terminal::PROJECT_ENV),
        std::env::var_os(terminal::DATA_DIR_ENV),
        std::env::var_os("KANGAE_DATA_DIR"),
    )
}

/// `target_from_env` の中身（env は process 全体の物なので、試験はこちらを値で呼ぶ）。空の値は無いのと同じ。
fn target_from(
    project: Option<OsString>,
    shibubu_dir: Option<OsString>,
    kangae_dir: Option<OsString>,
) -> Result<(String, PathBuf), CliError> {
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty());
    let id = set(project)
        .map(|v| v.to_string_lossy().into_owned())
        .ok_or(CliError::NotInProject)?;
    let dir = set(shibubu_dir)
        .or_else(|| set(kangae_dir))
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::Store(format!(
                "The environment variable {} for the storage location is missing",
                terminal::DATA_DIR_ENV
            ))
        })?;
    if !dir.is_absolute() {
        return Err(CliError::Store(format!(
            "The save location isn't a full path: {}",
            dir.display()
        )));
    }
    Ok((id, dir.join("kangae.sqlite3")))
}

/// L7-AGENT ①: 一言を runs 表の承認待ちの行（`RunRecord`・status = WaitingApproval・`approval` = この一言の exact payload・`origin` = 誰が頼んだか）に置く。
/// 画面の Shibubu が拾って承認の card を出す（`Generated::poll_asks`）。承認した時だけ、人が欄に打ったのと同じ道で 1 回読む。
/// 例: `let line = ask(&Store::open(&db)?, "project.app.x", "add a Tests page", InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("claude".into()) })?;`
/// 保証する: Project の行は変えない・Project の無い id は `ProjectNotFound`・Shibubu で一度も開いていない Project（session の行が無い）は `Store` の 1 行で何も書かない。
/// 保証しない: 画面の Shibubu が開いている事（開いていなければ行は待ち続け、開いた時に card が出る）。
/// Wave: L7
pub fn ask(store: &Store, project_id: &str, text: &str, by: InvocationOrigin) -> Result<RunLine, CliError> {
    use crate::domain::action::Effect;
    use crate::domain::run::{Run, Step};
    use crate::runtime::receipt::{self, ApprovalRequest, RunRecord};
    let store_err = |e: StoreError| CliError::Store(e.to_string());
    let row = store
        .get(Table::Projects, project_id)
        .map_err(store_err)?
        .ok_or_else(|| CliError::ProjectNotFound(project_id.into()))?;
    let p: ProjectDefinitionV1 =
        serde_json::from_value(row.json).map_err(|e| CliError::Unreadable(format!("Project: {e}")))?;
    // Project の id は `project.<app の id>`・app の session は `main.<app の id>`（Generated の project_id_of / session_of と同じ形）
    let app = project_id
        .strip_prefix("project.")
        .filter(|a| !a.is_empty())
        .ok_or_else(|| CliError::ProjectNotFound(project_id.into()))?;
    let session = format!("main.{app}");
    if store.get(Table::Sessions, &session).map_err(store_err)?.is_none() {
        return Err(CliError::Store("this project hasn't been opened in Shibubu yet. Open it in Shibubu".into()));
    }
    let app_version = store
        .get(Table::Apps, app)
        .map_err(store_err)?
        .and_then(|r| r.json["version"].as_u64())
        .unwrap_or(0) as u32;
    let (id, now) = (crate::storage::new_id(), crate::runtime::now_rfc3339());
    let input = serde_json::json!({ "text": text });
    let scopes = vec![ASK_ACTION.to_owned()];
    let rec = RunRecord {
        run: Run {
            id: id.clone(),
            app_id: app.into(),
            app_version,
            workflow_ref: None,
            origin: by,
            status: RunStatus::WaitingApproval,
            steps: vec![Step { id: crate::storage::new_id(), action_ref: ASK_ACTION.into(), status: RunStatus::WaitingApproval, attempts: Vec::new() }],
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        session_id: session,
        space_id: project_id.into(),
        input: input.clone(),
        target: p.name,
        idempotency_key: None,
        approval: Some(ApprovalRequest {
            run_id: id,
            action_id: ASK_ACTION.into(),
            effect: Effect::LocalWrite,
            payload_hash: crate::runtime::permission::payload_hash(ASK_ACTION, app, project_id, &input, &scopes),
            input,
            scopes,
            space_id: project_id.into(),
            requested_at: now,
        }),
        consent: None,
        receipt: None,
        workflow: None,
    };
    receipt::save(store, &rec).map_err(|e| CliError::Store(format!("{e:?}")))?;
    Ok(run_line(&rec))
}

/// `ask` の出力。Text = 何を頼んだか + 承認されるまで何も変わらない事 + 次の一手（runs で結果を読む）の 1 行。Json = `{schema, run}` の 1 行。
pub fn render_ask(run: &RunLine, format: Format) -> String {
    match format {
        Format::Json => format!("{}\n", serde_json::json!({ "schema": SCHEMA, "run": run })),
        Format::Text => format!(
            "Asked Shibubu: “{}”. Nothing changes until someone approves it in the Shibubu window. See the result with: shibubu project context (run {})\n",
            run.what, run.id
        ),
    }
}

/// 頼んだ agent の名前（この process の祖先の command 行のうち最初の agent・`ps` 1 回）。人が shell に打った時・読めない時は None。
fn calling_agent() -> Option<String> {
    let out = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,args="]).output().ok()?;
    agent_among(&String::from_utf8_lossy(&out.stdout), std::os::unix::process::parent_id())
}

/// `ps -A -o pid=,ppid=,args=` の出力で、`from` から親へ辿って最初に agent（`domain::terminal::agent_name`）の行。辿るのは 32 段まで（輪を回らない）。
fn agent_among(ps: &str, from: u32) -> Option<String> {
    let rows: BTreeMap<u32, (u32, &str)> = ps
        .lines()
        .filter_map(|l| {
            let mut w = l.trim_start().splitn(2, char::is_whitespace);
            let pid = w.next()?.parse().ok()?;
            let rest = w.next()?.trim_start();
            let mut w = rest.splitn(2, char::is_whitespace);
            let ppid = w.next()?.parse().ok()?;
            Some((pid, (ppid, w.next().unwrap_or("").trim())))
        })
        .collect();
    let mut pid = from;
    for _ in 0..32 {
        let (ppid, args) = rows.get(&pid)?;
        if let Some(a) = crate::domain::terminal::agent_name(args) {
            return Some(a.to_owned());
        }
        if *ppid <= 1 {
            return None;
        }
        pid = *ppid;
    }
    None
}

/// runs 表の 1 行を出す形に（頼んだ事 = `ask` の一言・Engine の一言・どちらでもなければ action の id）。
fn run_line(rec: &crate::runtime::receipt::RunRecord) -> RunLine {
    let what = rec.input["text"]
        .as_str()
        .or_else(|| rec.input.pointer("/record/goal").and_then(|v| v.as_str()))
        .map(str::to_owned)
        .unwrap_or_else(|| rec.run.steps.last().map(|s| s.action_ref.clone()).unwrap_or_default());
    RunLine {
        id: rec.run.id.clone(),
        status: rec.run.status,
        what,
        by: rec.run.origin.who(),
        created_at: rec.run.created_at.clone(),
        updated_at: rec.run.updated_at.clone(),
    }
}

/// 1 回の読み（その時点の写し）。Page と面の置き場は `projects` の行の定義（今 = head・Undo の後も同じ・W8）、Terminal は
/// `terminal_specs` と `terminal_sessions` の行（spec の無い記録 = `open_terminal` の物も）、面の状態は `surface_receipts` の面ごとの最新の行。
/// 例: `let ctx = read(&store, &project_id)?; let live: Vec<usize> = ctx.pages.iter().map(|p| p.terminals.iter().filter(|t| t.alive).count()).collect();`
/// 保証する: Page の順と id・route・題は画面の Page タブと同じ定義から・`alive` はこの読みの時に確かめた値（記録の Running を写さない）。
/// 保証しない: 読む間の別 process の書き込み・pid の再利用（記録が Running のままの pid を別の process が使っていれば `alive` に見える。
/// 記録は Shibubu が起こした時・閉じた時・開き直した時にしか書かないので、shell が自分で終わった後は、この起動の間ずっと〈閉じるまで〉と、
/// 落ちてから開き直すまでの間。試験の oracle は `ps` で親が Shibubu・tty・zombie でない事を見る = こちらより厳しい）・回収前の zombie（`ps` にも `Z` で出る）。
pub fn read(store: &Store, project_id: &str) -> Result<ProjectContext, CliError> {
    let store_err = |e: StoreError| CliError::Store(e.to_string());
    let unreadable =
        |what: &str, e: serde_json::Error| CliError::Unreadable(format!("{what}: {e}"));
    let row = store
        .get(Table::Projects, project_id)
        .map_err(store_err)?
        .ok_or_else(|| CliError::ProjectNotFound(project_id.into()))?;
    let p: ProjectDefinitionV1 =
        serde_json::from_value(row.json).map_err(|e| unreadable("Project", e))?;
    // Page ごとに置いた Terminal / 面の id（木の順・同じ物は 1 回 = 数は shell / 面の数）
    let placed = |page: &PageDefinition, component: &str, prop: &str| {
        let mut ids: Vec<String> = Vec::new();
        for n in page.root.nodes() {
            match n.props.get(prop).and_then(|v| v.as_str()) {
                Some(id) if n.component == component && !ids.iter().any(|x| x == id) => {
                    ids.push(id.to_owned())
                }
                _ => {}
            }
        }
        ids
    };
    let on_pages: Vec<(Vec<String>, Vec<String>)> = p
        .pages
        .iter()
        .map(|pg| {
            (
                placed(pg, "terminal", "terminal_ref"),
                placed(pg, "source_surface", "surface_ref"),
            )
        })
        .collect();
    let page_ids = |id: &str, surface: bool| -> Vec<String> {
        p.pages
            .iter()
            .zip(&on_pages)
            .filter(|(_, (t, s))| if surface { s } else { t }.iter().any(|x| x == id))
            .map(|(pg, _)| pg.id.clone())
            .collect()
    };

    let mut records: Vec<SessionInfo> = Vec::new();
    for r in store
        .list(Table::TerminalSessions, Some(project_id))
        .map_err(store_err)?
    {
        records.push(serde_json::from_value(r.json).map_err(|e| unreadable("Terminal history", e))?);
    }
    let terminal = |id: &str, shell: &str, working_directory: &str| {
        let rec = records.iter().find(|r| r.key.session_id == id);
        TerminalEntry {
            id: id.to_owned(),
            page_ids: page_ids(id, false),
            shell: shell.to_owned(),
            working_directory: working_directory.to_owned(),
            pid: rec.map(|r| r.pid),
            recorded: rec.map(|r| r.status.clone()),
            // 記録の Running を写さない: 今その pid が在る時だけ（前の起動の記録は開き直しで Unavailable になっている）
            alive: rec
                .is_some_and(|r| r.status == SessionStatus::Running && terminal::pid_alive(r.pid)),
        }
    };
    let mut terminals: Vec<TerminalEntry> = p
        .terminal_specs
        .iter()
        .map(|s| terminal(&s.id, &s.shell, &s.working_directory))
        .collect();
    for r in &records {
        if !p.terminal_specs.iter().any(|s| s.id == r.key.session_id) {
            terminals.push(terminal(&r.key.session_id, &r.shell, &r.working_directory));
        }
    }

    let mut latest = BTreeMap::new(); // 面の id → 最新の行（行は seq の順に来る）
    for r in store
        .list_surface_receipts(project_id, None)
        .map_err(store_err)?
    {
        latest.insert(r.surface_id.clone(), r);
    }
    let repo = crate::generated::repo_dir(&p);
    let mut surfaces = Vec::new();
    for s in &p.surface_specs {
        let receipt = match latest.remove(&s.id) {
            None => None,
            Some(row) => {
                let r: SurfaceReceipt =
                    serde_json::from_value(row.json).map_err(|e| unreadable("Screen checks", e))?;
                Some(ReceiptEntry {
                    kind: r.kind,
                    state: r.state,
                    oracle: r.oracle,
                    verified_at: r.verified_at,
                    recorded_at: row.recorded_at,
                })
            }
        };
        let (kind, app, document, url) = surface_facts(&s.target, repo.as_deref());
        surfaces.push(SurfaceEntry {
            id: s.id.clone(),
            page_ids: page_ids(&s.id, true),
            backend: s.backend,
            target: s.target.clone(),
            keep_alive: s.keep_alive,
            receipt,
            window: None, // S の列には面に窓を繋ぐ道が無い（M1 の後に決める）
            kind,
            app,
            document,
            url,
        });
    }

    let pages = p
        .pages
        .iter()
        .zip(&on_pages)
        .enumerate()
        .map(|(index, (pg, (t, s)))| PageEntry {
            index,
            id: pg.id.clone(),
            route: pg.route.clone(),
            title: pg.title.clone(),
            app_ref: pg.app_ref.clone(),
            terminals: t
                .iter()
                .filter_map(|id| terminals.iter().find(|x| &x.id == id).cloned())
                .collect(),
            surfaces: s
                .iter()
                .filter_map(|id| surfaces.iter().find(|x| &x.id == id).cloned())
                .collect(),
        })
        .collect();
    // L7-AGENT ②: この Project の run（app の session の行のうち space_id がこの Project の物・作った順）と、人を待っている run の id
    let app = project_id.strip_prefix("project.").unwrap_or(project_id);
    let mut runs: Vec<RunLine> = crate::runtime::receipt::list(store, &format!("main.{app}"))
        .map_err(|e| CliError::Store(format!("{e:?}")))?
        .iter()
        .filter(|r| r.space_id == project_id)
        .map(run_line)
        .collect();
    let waiting = runs
        .iter()
        .filter(|r| matches!(r.status, RunStatus::WaitingApproval | RunStatus::WaitingHuman))
        .map(|r| r.id.clone())
        .collect();
    // ponytail: 出すのは新しい 20 本だけ（agent が読む token を抑える）。古い run を引く口が要れば `--all`
    runs.drain(..runs.len().saturating_sub(RUNS_SHOWN));
    Ok(ProjectContext {
        schema: SCHEMA.into(),
        observed_at: crate::runtime::now_rfc3339(),
        project: ProjectSummary {
            id: p.id,
            name: p.name,
            goal: p.goal,
            revision: p.revision,
        },
        pages,
        terminals,
        surfaces,
        resources: p.resources,
        runs,
        waiting,
    })
}

/// `ProjectContext::runs` に出す新しい run の数。
const RUNS_SHOWN: usize = 20;

/// 出す文字列。Json: `pages` = `{schema, observed_at, project, pages}`・`surfaces` = `{schema, observed_at, project, surfaces}`・
/// `context` = `ProjectContext` の全部（どれも 1 行の JSON + 改行）。Text: Page か面ごとに 1 行（Terminal も面も 0 の Page も出す）。
/// 例: `print!("{}", render(&ctx, Invocation { command: Command::Pages, format: Format::Json }))`
/// 保証する: Json の欄の名前と意味は `SCHEMA` の版の間は変えない。保証しない: Text の形。
pub fn render(ctx: &ProjectContext, invocation: Invocation) -> String {
    if invocation.format == Format::Json {
        let mut v = serde_json::to_value(ctx)
            .expect("ProjectContext は欄の名前が文字列なので JSON にできる");
        let list = match invocation.command {
            Command::Pages => Some("pages"),
            Command::Surfaces => Some("surfaces"),
            Command::Context | Command::Ask { .. } | Command::Skills { .. } => None,
        };
        if let (Some(list), Some(o)) = (list, v.as_object_mut()) {
            o.retain(|k, _| {
                matches!(k.as_str(), "schema" | "observed_at" | "project") || k == list
            });
        }
        return format!("{v}\n");
    }
    let page_line = |pg: &PageEntry| {
        let alive = pg.terminals.iter().filter(|t| t.alive).count();
        format!(
            "{}. {} ({})  Terminals {} ({alive} running)  Screens {}\n",
            pg.index + 1,
            pg.title,
            pg.route,
            pg.terminals.len(),
            pg.surfaces.len()
        )
    };
    let surface_line = |s: &SurfaceEntry| {
        let target = match &s.target {
            SurfaceTarget::App {
                bundle_id,
                file: Some(f),
            } => format!("{bundle_id} ({f})"),
            SurfaceTarget::App {
                bundle_id,
                file: None,
            } => bundle_id.clone(),
            SurfaceTarget::Url { url } => url.clone(),
        };
        let state =
            s.receipt
                .as_ref()
                .map_or("Not checked yet", |r| match r.state {
                    ReceiptState::Verified => "Checked",
                    ReceiptState::Unverified => "Not verified",
                    ReceiptState::Unavailable => "Unavailable",
                    ReceiptState::Ended => "Not running",
                });
        format!("{target}  {state}\n")
    };
    let pages: String = ctx.pages.iter().map(page_line).collect();
    let surfaces: String = match ctx.surfaces.as_slice() {
        [] => "No screens (place an app or website on a page and they appear)\n".to_owned(),
        all => all.iter().map(surface_line).collect(),
    };
    // L7: 人を待っている run（承認・判断）は context の最後に 1 本 1 行
    let waiting: String = ctx
        .runs
        .iter()
        .filter(|r| ctx.waiting.contains(&r.id))
        .map(|r| format!("Waiting for a person in Shibubu: “{}” · asked by {} (run {})\n", r.what, r.by, r.id))
        .collect();
    // 段 2: context だけ、app の面ごとに呼び名と書類の 1 行（`REAPER · document: ~/x.rpp`・書類の無い面は `(unknown)`）
    let documents: String = ctx
        .surfaces
        .iter()
        .filter_map(|s| {
            let app = s.app.as_ref()?;
            let doc = s.document.as_deref().map_or("(unknown)".to_owned(), crate::composer::project::display_path);
            Some(format!("{} · document: {doc}\n", app.name))
        })
        .collect();
    match invocation.command {
        Command::Pages => pages,
        Command::Surfaces => surfaces,
        Command::Context | Command::Ask { .. } | Command::Skills { .. } => format!(
            "{} (revision {})\n{pages}{surfaces}{documents}{waiting}",
            ctx.project.name, ctx.project.revision
        ),
    }
}

/// `shibubu project context --json` の全体（`pages` / `surfaces` の出力はこの一部）。
/// 例: `let ctx: ProjectContext = serde_json::from_str(&stdout)?; assert_eq!(ctx.schema, SCHEMA);`
/// 保証する: 知らない欄は拒否・`terminals` と `surfaces` は Project の全部（Page に置いていない物も = `page_ids` が空）。
/// Wave: W9
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContext {
    /// `SCHEMA`。
    pub schema: String,
    /// この読みの時刻（RFC3339・UTC）。
    pub observed_at: String,
    pub project: ProjectSummary,
    /// 画面の Page タブの順。
    pub pages: Vec<PageEntry>,
    pub terminals: Vec<TerminalEntry>,
    pub surfaces: Vec<SurfaceEntry>,
    pub resources: Vec<ProjectResource>,
    /// L7-AGENT ②: この Project の run（新しい `RUNS_SHOWN` 本・作った順）。`ask` の一言も Engine の run も。
    pub runs: Vec<RunLine>,
    /// L7-AGENT ②: 人の承認か判断を待っている run の id（全部・古い順。`runs` から落ちた古い物も）。
    pub waiting: Vec<String>,
}

/// run 1 本（runs 表の行の、agent が読むのに要る欄だけ）。
/// Wave: L7
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLine {
    pub id: String,
    /// `waiting_approval`（人の承認待ち）・`running`・`verified`（通った）・`failed`・`cancelled`（人が断った）等。
    pub status: RunStatus,
    /// 頼んだ事（`ask` の一言・Engine の一言）。どちらでもない action 1 回の run は action の id。
    pub what: String,
    /// 誰が頼んだか（`InvocationOrigin::who`）。
    pub by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub goal: String,
    /// 今（head）の revision = `projects` の行の定義の `revision`（Undo の後は戻った番号。`list_project_revisions(..).last()` ではない・W8）。
    pub revision: u64,
}

/// Page 1 枚。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEntry {
    /// 0 始まりの順（画面の Page タブの左から）。
    pub index: usize,
    pub id: String,
    pub route: String,
    /// Page タブの文字（Page タブに AX の identifier は無いので、試験の側は画面の AX の木の Page タブとこれで照らす）。
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_ref: Option<String>,
    /// この Page の `terminal` node が指す Terminal（木の順）。
    pub terminals: Vec<TerminalEntry>,
    /// この Page の `source_surface` node が指す面（木の順）。
    pub surfaces: Vec<SurfaceEntry>,
}

/// Terminal 1 本。`id` = `TerminalSessionSpec::id` = `SessionKey::session_id` = 画面の AX の `terminal:<id>`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalEntry {
    pub id: String,
    /// 置いた Page の id（空 = Page の外: W4 の `open_terminal`・Page から外した Terminal）。
    pub page_ids: Vec<String>,
    pub shell: String,
    pub working_directory: String,
    /// `terminal_sessions` の記録の pid（この起動か前の起動の物）。記録が無い（まだ一度も起こしていない）時は None。
    pub pid: Option<u32>,
    /// 記録の状態（Shibubu が起こした時・閉じた時に書く。shell が自分で終わった後も Running のままでありうる）。記録が無ければ None。
    pub recorded: Option<SessionStatus>,
    /// この読みの時に pid が生きていた（記録が Running で、`kill(pid, 0) == 0`）。それ以外（Unavailable・Exited・記録無し）は false。
    /// pid の再利用は見ない: 自分で終わった shell の pid を別の process が使うと true になりうる（`read` の保証しない）。
    pub alive: bool,
}

/// 面 1 つ。`id` = `SurfaceSpec::id` = 画面の AX の `surface-status:<id>`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEntry {
    pub id: String,
    /// 置いた Page の id（空 = どの Page にも置いていない）。
    pub page_ids: Vec<String>,
    pub backend: SurfaceBackendKind,
    pub target: SurfaceTarget,
    pub keep_alive: bool,
    /// `surface_receipts` のこの面の最新の行。無ければ None（画面の `surface-status` の値 `unchecked` と同じ）。
    pub receipt: Option<ReceiptEntry>,
    /// Shibubu が今この面に繋いでいる窓。S の列（M1 の前）には面に窓を繋ぐ道が無いので常に None。
    /// 窓 id と pid は `SurfaceSpec` に保存しないので、繋いだ窓を書く所は M1（W16-1 の Native Direct）の後に決める。
    pub window: Option<WindowRef>,
    /// 段 2: `desktop_app`（`target` が app）か `web`（`target` が url）。Terminal は面ではない（`ProjectContext::terminals`）。
    pub kind: String,
    /// 段 2: app の面の app（bundle id と呼び名）。web の面は None。
    pub app: Option<AppEntry>,
    /// 段 2: 面が名指している書類の path（`target.file` を Project の repo〈`resources` の repo の `file://`〉の下に置いた絶対 path・
    /// repo が無ければ `target.file` のまま）。書類の無い app の面と web の面は None（agent が「どの file か」を Shibubu から知る口・周 3 の見つけ物）。
    pub document: Option<String>,
    /// 段 2: web の面の URL（`target.url` と同じ値）。app の面は None。
    pub url: Option<String>,
}

/// 段 2: 面の app（`SurfaceEntry::app`）。
/// 新しい型の理由: bundle id と呼び名の組を持つ既存の型が無い（`SurfaceTarget::App` は名前を持たず、`gui::WindowRef` は窓の pid と番号が要る）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppEntry {
    pub bundle_id: String,
    /// 呼び名 = bundle id の最後の段（com.cockos.reaper → reaper・com.apple.TextEdit → TextEdit）。
    /// Finder の表示名ではない（CLI は Spotlight を引かない・ponytail: 表示名が要れば Project を作った時の app の表示名を `SurfaceSpec` に保存する）。
    pub name: String,
}

/// 段 2: 面の `target` から `SurfaceEntry` の (`kind`, `app`, `document`, `url`)。`repo` = Project の repo の dir（`generated::repo_dir`）。
/// 例: `surface_facts(&SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: Some("song.rpp".into()) }, Some(Path::new("/w")))`
///   = `("desktop_app", Some(AppEntry { bundle_id: "com.cockos.reaper", name: "reaper" }), Some("/w/song.rpp"), None)`
fn surface_facts(target: &SurfaceTarget, repo: Option<&std::path::Path>) -> (String, Option<AppEntry>, Option<String>, Option<String>) {
    match target {
        SurfaceTarget::App { bundle_id, file } => {
            let name = bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_owned();
            // repo.join は file が絶対 path ならそのまま
            let document = file.as_ref().map(|f| repo.map_or_else(|| f.clone(), |r| r.join(f).display().to_string()));
            ("desktop_app".into(), Some(AppEntry { bundle_id: bundle_id.clone(), name }), document, None)
        }
        SurfaceTarget::Url { url } => ("web".into(), None, None, Some(url.clone())),
    }
}

/// 面の最新の読み戻し（`domain::project::SurfaceReceipt` の、状態を言うのに要る欄だけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEntry {
    pub kind: ReceiptKind,
    pub state: ReceiptState,
    /// 読んだ道の名前（`SurfaceReceipt::oracle`）。
    pub oracle: String,
    /// verified の時だけ空でない（`SurfaceReceipt::validate`）。
    pub verified_at: String,
    /// 行を書いた時刻（SQLite の RFC3339・UTC）。
    pub recorded_at: String,
}

/// 面に繋いだ窓（L4 の照合の鍵）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRef {
    pub pid: u32,
    /// CGWindowID（試験の側は CGWindowList の `kCGWindowNumber` と照らす）。
    pub window_id: u32,
}

/// CLI の誤り。Display は stderr に出す 1 行（何が起きたか + 次の一手・D-e）。
/// Wave: W9
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// 打ち方の誤り（知らない command・引数・command が無い）。code 2。
    Usage(String),
    /// Project の Terminal の外で打った（`terminal::PROJECT_ENV` が無い・空）。code 2。
    NotInProject,
    /// 保存を開けない（保存先の環境変数が無い・相対 path・DB の file が無い・新しい版の DB）。code 1。
    Store(String),
    /// Project の行が保存に無い（消した・別の保存先）。code 1。
    ProjectNotFound(String),
    /// 行が読めない（後の版の JSON・壊れた保存）。code 1。
    Unreadable(String),
}

impl CliError {
    /// 0 = 出せた・1 = 読めない・2 = 打ち方か打つ場所の誤り。
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::NotInProject => 2,
            Self::Store(_) | Self::ProjectNotFound(_) | Self::Unreadable(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(why) => write!(f, "{why}. {USAGE}"),
            Self::NotInProject => write!(
                f,
                "This shell doesn't know the project. Type this in a Shibubu project's terminal"
            ),
            Self::Store(why) => write!(
                f,
                "Couldn't open the project's saved data ({why}). Type it again in a Shibubu project's terminal"
            ),
            Self::ProjectNotFound(id) => write!(
                f,
                "Project '{id}' isn't in the saved data. Reopen the project in Shibubu, then type it again"
            ),
            Self::Unreadable(why) => write!(
                f,
                "Couldn't read the project's saved data ({why}). Update Shibubu, then type it again"
            ),
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    /// 契約: 3 つの名前と `--json` だけを読み、`project` で始まらない argv は GUI に返す。知らない物は黙って既定にしない。
    #[test]
    fn w9_parse_reads_three_commands_and_leaves_the_gui_alone() {
        for (s, command) in [
            ("shibubu project pages --json", Command::Pages),
            ("shibubu project context --json", Command::Context),
            ("shibubu project surfaces --json", Command::Surfaces),
        ] {
            assert_eq!(
                parse(&argv(s)),
                Some(Ok(Invocation {
                    command,
                    format: Format::Json
                })),
                "{s}"
            );
        }
        assert_eq!(
            parse(&argv("shibubu project pages")),
            Some(Ok(Invocation {
                command: Command::Pages,
                format: Format::Text
            }))
        );
        for gui in ["shibubu", "shibubu -psn_0_12345", "shibubu pages --json"] {
            assert_eq!(parse(&argv(gui)), None, "GUI を起こす: {gui}");
        }
        for bad in [
            "shibubu project",
            "shibubu project page --json",
            "shibubu project pages --jsn",
            "shibubu project pages --json extra",
        ] {
            let err = parse(&argv(bad)).expect("project の口").unwrap_err();
            assert!(
                matches!(err, CliError::Usage(_)) && err.exit_code() == 2,
                "{bad}: {err:?}"
            );
            assert!(err.to_string().ends_with(USAGE), "次の一手の 1 行: {err}");
        }
    }

    /// 契約: 出力の形は閉じている（知らない欄は拒否・欄の名前は SCHEMA の版の間は変えない）。
    #[test]
    fn w9_context_json_is_closed() {
        let ctx = ProjectContext {
            schema: SCHEMA.into(),
            observed_at: "2026-10-21T00:00:00.000Z".into(),
            project: ProjectSummary {
                id: "project.a".into(),
                name: "a".into(),
                goal: "g".into(),
                revision: 2,
            },
            pages: vec![PageEntry {
                index: 0,
                id: "page.a.1".into(),
                route: "/".into(),
                title: "Dev".into(),
                app_ref: None,
                terminals: Vec::new(),
                surfaces: Vec::new(),
            }],
            terminals: vec![TerminalEntry {
                id: "t1".into(),
                page_ids: vec!["page.a.1".into()],
                shell: "/bin/zsh".into(),
                working_directory: "/".into(),
                pid: Some(42),
                recorded: Some(SessionStatus::Running),
                alive: true,
            }],
            surfaces: Vec::new(),
            resources: Vec::new(),
            runs: vec![RunLine {
                id: "r1".into(),
                status: RunStatus::WaitingApproval,
                what: "add a Tests page".into(),
                by: "claude in a terminal".into(),
                created_at: "2026-10-30T00:00:00.000Z".into(),
                updated_at: "2026-10-30T00:00:00.000Z".into(),
            }],
            waiting: vec!["r1".into()],
        };
        let v = serde_json::to_value(&ctx).unwrap();
        assert_eq!(v["runs"][0]["status"], "waiting_approval");
        assert_eq!(v["terminals"][0]["recorded"]["state"], "running");
        assert_eq!(
            serde_json::from_value::<ProjectContext>(v.clone()).unwrap(),
            ctx
        );
        let mut extra = v;
        extra["pages"][0]["count"] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<ProjectContext>(extra).is_err(),
            "知らない欄は拒否"
        );
    }

    fn os(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    /// 反証（W9）: Terminal の PATH の先頭に GUI の binary が在るので、打ち間違い（`shibubu pages --json`・`shibubu --help`・`shibubu`）が
    /// 2 つ目の Shibubu を同じ DB で起こし、動いている Terminal の記録を Unavailable に書き換えていた。Project の Terminal の中では GUI に返さない。
    #[test]
    fn w9_inside_a_project_terminal_typos_are_usage_not_a_second_gui() {
        for typed in [
            "shibubu",
            "shibubu -psn_0_12345",
            "shibubu pages --json",
            "shibubu projects pages",
            "shibubu --help",
        ] {
            let err = decide(&argv(typed), os("project.a"))
                .unwrap_or_else(|| panic!("Project の Terminal の中で GUI を起こす: {typed}"))
                .unwrap_err();
            assert!(
                matches!(err, CliError::Usage(_)) && err.exit_code() == 2,
                "{typed}: {err:?}"
            );
            let line = err.to_string();
            assert!(
                line.contains("Shibubu doesn't open") && line.ends_with(USAGE) && !line.contains('\n'),
                "何が起きたか + 次の一手の 1 行: {line}"
            );
            // Terminal の外で GUI に返すのは起動の引数だけの argv（それ以外は下の test）
            if matches!(typed, "shibubu" | "shibubu -psn_0_12345") {
                for outside in [None, os("")] {
                    assert_eq!(
                        decide(&argv(typed), outside),
                        None,
                        "Terminal の外は今までどおり GUI: {typed}"
                    );
                }
            }
        }
        assert_eq!(
            decide(&argv("shibubu project pages --json"), os("project.a")),
            parse(&argv("shibubu project pages --json")),
            "project の口は env に依らない"
        );
    }

    /// 反証（前面の不具合・10-22 04:24 yc-s10 / 10-21 yc-s13）: 試験の数を数える手が `target/debug/deps/shibubu-<hash>`（libtest の
    /// binary ではなく製品の bin）を `--list` で起こし、Terminal の外なので GUI に返って窓が前面を取った。窓を開くのは macOS が app に渡す
    /// 起動の引数（`-psn_…`・NSArgumentDomain の `-名前 値`）だけの時。それ以外は Terminal の外でも `Usage` の 1 行で code 2。
    #[test]
    fn argv_that_is_not_a_launch_is_usage_not_a_window() {
        for launch in [
            "shibubu",
            "shibubu -psn_0_12345",
            "shibubu -NSAppSleepDisabled YES",
            "shibubu -ApplePersistenceIgnoreState YES -NSAppSleepDisabled YES",
        ] {
            for outside in [None, os("")] {
                assert_eq!(decide(&argv(launch), outside), None, "窓を開く: {launch}");
            }
        }
        for (typed, named) in [
            ("shibubu --list", "--list"),
            ("shibubu --list --format terse", "--list"),
            ("shibubu w10_", "w10_"),
            ("shibubu --exact w10_x", "--exact"),
            ("shibubu -q --list", "-q"),
            ("shibubu --help", "--help"),
            ("shibubu -h", "-h"),
            ("shibubu pages --json", "pages"),
            ("shibubu -NSAppSleepDisabled YES --list", "--list"),
        ] {
            for outside in [None, os("")] {
                let err = decide(&argv(typed), outside)
                    .unwrap_or_else(|| panic!("窓を開いてしまう: {typed}"))
                    .unwrap_err();
                assert!(
                    matches!(err, CliError::Usage(_)) && err.exit_code() == 2,
                    "{typed}: {err:?}"
                );
                let line = err.to_string();
                assert!(
                    line.contains(&format!("the argument {named}"))
                        && line.contains("Shibubu doesn't open")
                        && line.ends_with(USAGE)
                        && !line.contains('\n'),
                    "どの引数で何が起きなかったか + 次の一手の 1 行: {line}"
                );
            }
        }
    }

    /// L7-AGENT: `ask` は残りの引数を 1 つの一言に（`--json` はどこでも）・空と長すぎる一言は `Usage`（黙って既定にしない）。
    #[test]
    fn l7_parse_reads_ask_with_the_rest_as_one_sentence() {
        let ask = |s: &str| parse(&argv(s)).expect("project の口");
        assert_eq!(
            ask("shibubu project ask add a Tests page --json"),
            Ok(Invocation { command: Command::Ask { text: "add a Tests page".into() }, format: Format::Json })
        );
        assert_eq!(
            parse(&["shibubu".into(), "project".into(), "ask".into(), "rename Docs to “Notes”".into()]),
            Some(Ok(Invocation { command: Command::Ask { text: "rename Docs to “Notes”".into() }, format: Format::Text })),
            "1 つの引数の一言（agent が引用符で渡す）"
        );
        for bad in ["shibubu project ask", "shibubu project ask --json"] {
            let e = ask(bad).unwrap_err();
            assert!(matches!(e, CliError::Usage(_)) && e.to_string().contains("Say what to change"), "{bad}: {e}");
        }
        let long = format!("shibubu project ask {}", "x".repeat(ASK_MAX_CHARS + 1));
        assert!(matches!(ask(&long), Err(CliError::Usage(_))), "長すぎる一言");
    }

    /// 段 2（段 1 の見つけ物 3）: `ask --help` / `-h` は一言にせず使い方（parse で止まる = runs に行を置かない）。`--` で始まる別の引数
    /// （`--jsn` の打ち間違い）も承認の card に出さず `Unknown argument`。引用符の中の `--` は一言のまま。
    /// 負の対照: parse の ask の枝から flag の検査を外す → `--help` が Ok(Ask) になって赤。
    #[test]
    fn skills2_ask_help_is_usage_not_a_change() {
        for typed in ["shibubu project ask --help", "shibubu project ask -h", "shibubu project ask --json --help"] {
            let e = parse(&argv(typed)).expect("project の口").unwrap_err();
            let line = e.to_string();
            assert!(
                matches!(e, CliError::Usage(_)) && e.exit_code() == 2 && line.contains("approve") && line.ends_with(USAGE),
                "{typed}: {line}"
            );
        }
        let e = parse(&argv("shibubu project ask --jsn add a page")).unwrap().unwrap_err();
        assert!(e.to_string().starts_with("Unknown argument: --jsn"), "{e}");
        assert_eq!(
            parse(&["shibubu".into(), "project".into(), "ask".into(), "rename --old to --new".into(), "--json".into()]),
            Some(Ok(Invocation { command: Command::Ask { text: "rename --old to --new".into() }, format: Format::Json })),
            "1 つの引数の中の -- は一言"
        );
    }

    /// 段 2: `skills` は CLI（GUI を起こさない）で、Project の Terminal の外でも読める。知らない物は `Usage`（黙って list にしない）。
    #[test]
    fn skills2_parse_reads_list_and_get_anywhere() {
        let skills = |name: Option<&str>, format| Some(Ok(Invocation { command: Command::Skills { name: name.map(str::to_owned) }, format }));
        assert_eq!(parse(&argv("shibubu skills list")), skills(None, Format::Text));
        assert_eq!(parse(&argv("shibubu skills list --json")), skills(None, Format::Json));
        assert_eq!(parse(&argv("shibubu skills get shibubu-cli")), skills(Some(GUIDE_NAME), Format::Text));
        for outside in [None, os(""), os("project.a")] {
            assert_eq!(decide(&argv("shibubu skills get shibubu-cli"), outside), skills(Some(GUIDE_NAME), Format::Text), "env に依らない");
        }
        for (bad, why) in [
            ("shibubu skills", "No command given"),
            ("shibubu skills lst", "Unknown command: lst"),
            ("shibubu skills list --jsn", "Unknown argument: --jsn"),
            ("shibubu skills get", "Say which guide"),
            ("shibubu skills get shibubu-cli --json", "Unknown argument: --json"),
        ] {
            let e = parse(&argv(bad)).expect("skills の口").unwrap_err();
            assert!(matches!(e, CliError::Usage(_)) && e.exit_code() == 2 && e.to_string().starts_with(why) && e.to_string().ends_with(USAGE), "{bad}: {e}");
        }
    }

    /// 段 2: `skills get shibubu-cli` = docs/skills/shibubu-cli.md の `GUIDE_END` より前（binary に埋めた写し = 今の docs と 1 byte も違わない）。
    /// 印から下は出さない。無い名前は `Usage`。list は名前と、本文の頭の `description` を 1 行にした物。
    /// 負の対照: ① docs の `<!-- end of guide` の行を消す → 本文が空で赤 ② `guide` を `GUIDE_FILE` 全部にする → 印の行が出て赤
    /// ③ `include_str!` を古い本文の字に替える → 今の docs と違って赤。
    #[test]
    fn skills2_get_prints_the_docs_guide_head() {
        let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/skills/shibubu-cli.md")).unwrap();
        let (head, _) = docs.split_once(GUIDE_END).expect("docs に案内の終わりの印が無い");
        let out = render_skills(Some(GUIDE_NAME), Format::Text).unwrap();
        assert_eq!(out, format!("{}\n", head.trim_end()), "埋めた本文が今の docs の前半と違う");
        assert!(out.starts_with("---\nname: shibubu-cli\n") && out.contains("# Shibubu CLI") && out.contains("## Never"), "{out}");
        assert!(!out.contains(GUIDE_END), "印から下まで出した");
        let e = render_skills(Some("nope"), Format::Text).unwrap_err();
        assert!(matches!(e, CliError::Usage(_)) && e.to_string().starts_with("There's no guide named nope. See the guides with: shibubu skills list"), "{e}");
        let d = guide_description();
        assert!(d.starts_with("Read and change the Shibubu project") && d.ends_with("\"did my change go through\".") && !d.contains('\n') && !d.contains(">-") && !d.contains("  "), "{d:?}");
        let v: serde_json::Value = serde_json::from_str(&render_skills(None, Format::Json).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({ "schema": SCHEMA, "skills": [{ "name": GUIDE_NAME, "description": d }] }));
        let text = render_skills(None, Format::Text).unwrap();
        assert!(text.starts_with(GUIDE_NAME) && text.contains("shibubu skills get") && text.ends_with('\n'), "{text}");
    }

    /// 段 2: env も panel の 1 行も見えない agent（`claude -p` の Bash だけ）が辿り着く口 = 使い方の 1 行。`shibubu --help`（Project の Terminal の中）の
    /// 1 行に案内の名前が字のまま在る。英文の後ろに全角の「。」を付けない（段 1 の見つけ物 1）。
    #[test]
    fn skills2_usage_names_the_guide() {
        let line = decide(&argv("shibubu --help"), os("project.a")).unwrap().unwrap_err().to_string();
        assert!(line.contains("shibubu skills get shibubu-cli") && !line.contains('。') && !line.contains('\n'), "{line}");
    }

    /// L7-AGENT: 頼んだ agent = 祖先の command 行のうち最初の agent（shell・Shibubu を飛ばす）。agent の居ない祖先は None（人が打った）。
    #[test]
    fn l7_agent_among_walks_the_parents() {
        let ps = "  1     0 /sbin/launchd\n 50     1 /Applications/Shibubu.app/Contents/MacOS/shibubu\n 60    50 -zsh\n 70    60 node /usr/local/bin/claude --resume\n 80    70 /bin/bash -c shibubu project ask x\n 90    60 /bin/zsh -i\n";
        assert_eq!(agent_among(ps, 80), Some("claude".into()), "bash の上の claude");
        assert_eq!(agent_among(ps, 70), Some("claude".into()));
        assert_eq!(agent_among(ps, 90), None, "人が shell に打った");
        assert_eq!(agent_among(ps, 999), None, "表に無い pid");
        assert_eq!(agent_among(" 5 5 /bin/sh\n", 5), None, "自分が親の輪を回らない");
    }

    /// 読む Project と保存先は子の環境だけから決める（別の Project を読む口は無い）。
    #[test]
    fn w9_target_comes_only_from_the_terminal_env() {
        let db = |s: &str| PathBuf::from(s).join("kangae.sqlite3");
        assert_eq!(
            target_from(os("project.a"), os("/d/s"), os("/d/k")),
            Ok(("project.a".into(), db("/d/s"))),
            "SHIBUBU_DATA_DIR が先"
        );
        assert_eq!(
            target_from(os("project.a"), None, os("/d/k")),
            Ok(("project.a".into(), db("/d/k")))
        );
        assert_eq!(
            target_from(os("project.a"), os(""), os("/d/k")),
            Ok(("project.a".into(), db("/d/k"))),
            "空は無いのと同じ"
        );
        for project in [None, os("")] {
            let e = target_from(project, os("/d/s"), None).unwrap_err();
            assert_eq!((e.clone(), e.exit_code()), (CliError::NotInProject, 2));
        }
        for (s, k) in [(None, None), (os("data"), None), (None, os("./x"))] {
            let e = target_from(os("project.a"), s, k).unwrap_err();
            assert!(
                matches!(e, CliError::Store(_)) && e.exit_code() == 1,
                "{e:?}"
            );
        }
    }

    fn node(v: serde_json::Value) -> crate::domain::ui::UiNode {
        serde_json::from_value(v).unwrap()
    }

    fn session(id: &str, pid: u32, status: SessionStatus) -> serde_json::Value {
        serde_json::to_value(SessionInfo {
            key: crate::terminal::SessionKey {
                project_id: "project.a".into(),
                session_id: id.into(),
            },
            pid,
            shell: "/bin/zsh".into(),
            working_directory: "/w4".into(),
            status,
        })
        .unwrap()
    }

    /// 3 Page（A: t1・t2・面 s1 / B: t1 / C: 空）・Page の外の t3・面 s2・spec の無い記録 w4。
    fn fixture() -> (Store, u32) {
        let page = |id: &str, title: &str, children: serde_json::Value| PageDefinition {
            id: id.into(),
            route: format!("/{title}"),
            title: title.into(),
            app_ref: Some("app.a".into()),
            root: node(
                serde_json::json!({ "key": format!("{id}.root"), "component": "column", "children": children }),
            ),
            revision: 1,
        };
        let term = |key: &str, id: &str| serde_json::json!({ "key": key, "component": "terminal", "props": { "terminal_ref": id } });
        let spec = |id: &str| crate::domain::project::TerminalSessionSpec {
            id: id.into(),
            project_id: "project.a".into(),
            shell: "/bin/zsh".into(),
            working_directory: "/".into(),
            keep_alive_on_page_switch: true, command: None,
        };
        let surface = |id: &str| crate::domain::project::SurfaceSpec {
            id: id.into(),
            backend: SurfaceBackendKind::NativeDirect,
            target: SurfaceTarget::App {
                bundle_id: "com.example.x".into(),
                file: None,
            },
            keep_alive: true,
            frame: None,
            browser: false,
        };
        let p = ProjectDefinitionV1 {
            id: "project.a".into(),
            name: "a".into(),
            goal: "g".into(),
            app_refs: vec!["app.a".into()],
            pages: vec![
                page(
                    "page.a",
                    "A",
                    serde_json::json!([
                        term("ta1", "t1"),
                        { "key": "row", "component": "row", "children": [term("ta2", "t2"), term("ta3", "t1")] },
                        { "key": "sa", "component": "source_surface", "props": { "surface_ref": "s1" } }
                    ]),
                ),
                page("page.b", "B", serde_json::json!([term("tb1", "t1")])),
                page("page.c", "C", serde_json::json!([])),
            ],
            resources: Vec::new(),
            terminal_specs: vec![spec("t1"), spec("t2"), spec("t3")],
            surface_specs: vec![surface("s1"), surface("s2")],
            revision: 2,
        };
        let store = Store::open_in_memory().unwrap();
        let v = serde_json::to_value(&p).unwrap();
        store.put(Table::Projects, "project.a", None, &v).unwrap();
        // Undo の後: project_revisions の最後（3）は Redo の行き先で、今（head）は projects の行の 2
        for rev in 1..=3 {
            let mut snap = v.clone();
            snap["revision"] = rev.into();
            store
                .put_project_revision(
                    "project.a",
                    rev,
                    &snap,
                    None,
                    &serde_json::json!([]),
                    "nl_edit",
                )
                .unwrap();
        }
        // 終わって回収された pid（kill(pid, 0) が ESRCH）
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let me = std::process::id();
        for (id, pid, status) in [
            ("t1", me, SessionStatus::Running),
            ("t2", dead, SessionStatus::Running),
            ("t3", me, SessionStatus::Unavailable),
            (
                "w4",
                me,
                SessionStatus::Exited {
                    exit: crate::terminal::ExitInfo {
                        code: Some(0),
                        signal: None,
                    },
                },
            ),
        ] {
            store
                .put(
                    Table::TerminalSessions,
                    id,
                    Some("project.a"),
                    &session(id, pid, status),
                )
                .unwrap();
        }
        let receipt = |state: ReceiptState, oracle: &str| SurfaceReceipt {
            surface_id: "s1".into(),
            page_id: "page.a".into(),
            kind: ReceiptKind::NativeApp,
            state,
            oracle: oracle.into(),
            app_id: "com.example.x".into(),
            app_version: String::new(),
            os_version: String::new(),
            operation: String::new(),
            verified_at: String::new(),
            evidence_ref: String::new(),
        };
        for r in [
            receipt(ReceiptState::Unverified, "first"),
            receipt(ReceiptState::Unavailable, "latest"),
        ] {
            store
                .put_surface_receipt(
                    "project.a",
                    "s1",
                    "page.a",
                    r.kind.as_str(),
                    r.state.as_str(),
                    &serde_json::to_value(&r).unwrap(),
                )
                .unwrap();
        }
        (store, dead)
    }

    /// L7-AGENT ①②: `ask` は runs 表の承認待ちの行を 1 つ書くだけ（Project の行は 1 byte も変えない）・誰が頼んだかを持つ・`read` の runs と waiting に出る。
    /// Shibubu で開いていない Project（session の行が無い）・無い Project は何も書かずに拒否。
    #[test]
    fn l7_ask_writes_one_waiting_run_and_leaves_the_project_alone() {
        let (store, _) = fixture();
        let before = store.get(Table::Projects, "project.a").unwrap().unwrap().json;
        let by = InvocationOrigin::Terminal { terminal_id: "t1".into(), agent: Some("claude".into()) };
        assert!(
            matches!(ask(&store, "project.a", "add a Tests page", by.clone()), Err(CliError::Store(ref w)) if w.contains("Open it in Shibubu")),
            "session の行が無い = 一度も開いていない"
        );
        assert_eq!(read(&store, "project.a").unwrap().runs, Vec::new(), "拒否は何も書かない");
        store.put(Table::Apps, "a", None, &serde_json::json!({ "version": 3 })).unwrap();
        store.put(Table::Sessions, "main.a", Some("a"), &serde_json::json!({ "app_id": "a" })).unwrap();
        assert_eq!(ask(&store, "project.gone", "x", by.clone()).unwrap_err(), CliError::ProjectNotFound("project.gone".into()));
        let line = ask(&store, "project.a", "add a Tests page", by.clone()).unwrap();
        assert_eq!((line.status, line.what.as_str(), line.by.as_str()), (RunStatus::WaitingApproval, "add a Tests page", "claude in a terminal"));
        assert_eq!(store.get(Table::Projects, "project.a").unwrap().unwrap().json, before, "Project は変えない");
        let rec = crate::runtime::receipt::load(&store, &line.id).unwrap().expect("runs 表の行");
        assert_eq!(rec.run.origin, by, "誰が頼んだか");
        assert_eq!((rec.space_id.as_str(), rec.session_id.as_str(), rec.run.app_id.as_str(), rec.run.app_version), ("project.a", "main.a", "a", 3));
        let a = rec.approval.expect("承認の問い");
        assert_eq!((a.action_id.as_str(), a.input.clone()), (ASK_ACTION, serde_json::json!({ "text": "add a Tests page" })));
        assert!(rec.consent.is_none() && rec.workflow.is_none());
        let ctx = read(&store, "project.a").unwrap();
        assert_eq!(ctx.runs, vec![line.clone()]);
        assert_eq!(ctx.waiting, vec![line.id.clone()], "承認待ち");
        let text = render(&ctx, Invocation { command: Command::Context, format: Format::Text });
        assert!(text.contains("Waiting for a person in Shibubu: “add a Tests page” · asked by claude in a terminal"), "{text}");
        let out = render_ask(&line, Format::Text);
        assert!(out.contains("Nothing changes until someone approves it") && out.contains(&line.id) && out.ends_with('\n'), "{out}");
        let v: serde_json::Value = serde_json::from_str(&render_ask(&line, Format::Json)).unwrap();
        assert_eq!((v["schema"].as_str(), v["run"]["status"].as_str()), (Some(SCHEMA), Some("waiting_approval")));
    }

    /// Page の順・置き場・head の revision・読んだ時に確かめた alive・面ごとの最新の receipt。
    #[test]
    fn w9_read_follows_the_head_and_checks_pids_at_read_time() {
        let (store, dead) = fixture();
        let ctx = read(&store, "project.a").unwrap();
        assert_eq!(ctx.schema, SCHEMA);
        assert_eq!(
            ctx.project.revision, 2,
            "head = projects の行（Redo の行き先 3 ではない）"
        );
        let pages: Vec<(usize, &str, &str, Vec<&str>, Vec<&str>)> = ctx
            .pages
            .iter()
            .map(|p| {
                (
                    p.index,
                    p.id.as_str(),
                    p.route.as_str(),
                    p.terminals.iter().map(|t| t.id.as_str()).collect(),
                    p.surfaces.iter().map(|s| s.id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            pages,
            [
                (0, "page.a", "/A", vec!["t1", "t2"], vec!["s1"]),
                (1, "page.b", "/B", vec!["t1"], vec![]),
                (2, "page.c", "/C", vec![], vec![]),
            ],
            "木の順・同じ Terminal は 1 回・空の Page も出す"
        );
        let t = |id: &str| ctx.terminals.iter().find(|t| t.id == id).unwrap();
        assert_eq!(
            ctx.terminals
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["t1", "t2", "t3", "w4"],
            "spec の順の後に spec の無い記録"
        );
        assert_eq!(t("t1").page_ids, ["page.a", "page.b"]);
        assert!(t("t3").page_ids.is_empty() && t("w4").page_ids.is_empty());
        assert_eq!(
            ctx.terminals.iter().map(|t| t.alive).collect::<Vec<_>>(),
            [true, false, false, false],
            "Running で今在る pid だけ"
        );
        assert_eq!(
            (t("t2").pid, t("t2").recorded.clone()),
            (Some(dead), Some(SessionStatus::Running)),
            "記録は写すが alive にしない"
        );
        assert_eq!(
            t("w4").working_directory,
            "/w4",
            "spec の無い記録は記録の値"
        );
        assert_eq!(ctx.surfaces.len(), 2);
        let s1 = &ctx.surfaces[0];
        assert_eq!(
            (s1.page_ids.clone(), s1.window),
            (vec!["page.a".to_owned()], None)
        );
        let r = s1.receipt.as_ref().expect("s1 の receipt");
        assert_eq!(
            (r.state, r.oracle.as_str()),
            (ReceiptState::Unavailable, "latest"),
            "面ごとの最新の行"
        );
        assert!(!r.recorded_at.is_empty());
        assert!(ctx.surfaces[1].receipt.is_none() && ctx.surfaces[1].page_ids.is_empty());
        assert_eq!(
            ctx.pages[0].terminals[0], ctx.terminals[0],
            "Page の中の Terminal は全体の物と同じ値"
        );
    }

    #[test]
    fn w9_read_refuses_missing_and_unreadable_projects() {
        let (store, _) = fixture();
        assert_eq!(
            read(&store, "project.gone").unwrap_err(),
            CliError::ProjectNotFound("project.gone".into())
        );
        let mut v = store
            .get(Table::Projects, "project.a")
            .unwrap()
            .unwrap()
            .json;
        v["later"] = serde_json::json!(1);
        store.put(Table::Projects, "project.a", None, &v).unwrap();
        let e = read(&store, "project.a").unwrap_err();
        assert!(
            matches!(e, CliError::Unreadable(_)) && e.exit_code() == 1,
            "{e:?}"
        );
        assert!(e.to_string().contains("Update Shibubu"), "{e}");
    }

    /// 段 2（operator 13:1x・周 3 の見つけ物 = agent が note の在処を Shibubu から知れない）: 面ごとに kind・app（bundle id と呼び名）・
    /// 書類の絶対 path（repo の下）・URL。既存の欄（`target` の相対の file）は変えない。repo が無ければ相対のまま。context の Text は app ごとに 1 行。
    /// 負の対照: `surface_facts` の document を `file.clone()`（repo を見ない）にする → `/w/songs/a.rpp` で赤。
    #[test]
    fn skills2_context_names_the_app_document_and_url() {
        let (store, _) = fixture();
        let row = store.get(Table::Projects, "project.a").unwrap().unwrap().json;
        let mut p: ProjectDefinitionV1 = serde_json::from_value(row).unwrap();
        p.surface_specs[1].target = SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: Some("songs/a.rpp".into()) };
        let mut web = p.surface_specs[1].clone();
        (web.id, web.backend, web.target) = ("s3".into(), SurfaceBackendKind::WebView, SurfaceTarget::Url { url: "https://excalidraw.com/#room".into() });
        p.surface_specs.push(web);
        let put = |p: &ProjectDefinitionV1| store.put(Table::Projects, "project.a", None, &serde_json::to_value(p).unwrap()).unwrap();
        put(&p);
        let ctx = read(&store, "project.a").unwrap();
        let s = |id: &str| ctx.surfaces.iter().find(|s| s.id == id).unwrap().clone();
        let app = |b: &str, n: &str| Some(AppEntry { bundle_id: b.into(), name: n.into() });
        assert_eq!((s("s1").kind.as_str(), s("s1").app, s("s1").document, s("s1").url), ("desktop_app", app("com.example.x", "x"), None, None));
        assert_eq!((s("s3").kind.as_str(), s("s3").app, s("s3").document, s("s3").url), ("web", None, None, Some("https://excalidraw.com/#room".into())));
        assert_eq!((s("s2").app, s("s2").document), (app("com.cockos.reaper", "reaper"), Some("songs/a.rpp".into())), "repo が無ければ相対のまま");
        p.resources.push(ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some("file:///w".into()) });
        put(&p);
        let ctx = read(&store, "project.a").unwrap();
        let s2 = ctx.surfaces.iter().find(|s| s.id == "s2").unwrap();
        assert_eq!(s2.document.as_deref(), Some("/w/songs/a.rpp"), "repo の下の絶対 path");
        assert_eq!(s2.target, SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: Some("songs/a.rpp".into()) }, "既存の欄は変えない");
        let v: serde_json::Value = serde_json::from_str(&render(&ctx, Invocation { command: Command::Surfaces, format: Format::Json })).unwrap();
        assert_eq!((&v["surfaces"][1]["document"], &v["surfaces"][1]["app"]["name"], &v["surfaces"][0]["url"]), (&serde_json::json!("/w/songs/a.rpp"), &serde_json::json!("reaper"), &serde_json::Value::Null));
        assert_eq!(serde_json::from_str::<ProjectContext>(&render(&ctx, Invocation { command: Command::Context, format: Format::Json })).unwrap(), ctx, "context は新しい欄ごと読み戻せる");
        let text = render(&ctx, Invocation { command: Command::Context, format: Format::Text });
        assert!(text.contains("x · document: (unknown)\n") && text.contains("reaper · document: /w/songs/a.rpp\n"), "{text}");
        assert!(!text.contains("excalidraw.com · document"), "web の面は書類の行を出さない: {text}");
        assert_eq!(render(&ctx, Invocation { command: Command::Surfaces, format: Format::Text }).lines().count(), 3, "surfaces の Text は面ごとに 1 行のまま");
    }

    /// `pages` / `surfaces` は 4 つの欄だけ・`context` は全部・どれも 1 行の JSON。Text は Page ごとに 1 行（空の Page も）。
    #[test]
    fn w9_render_cuts_the_json_per_command() {
        let (store, _) = fixture();
        let ctx = read(&store, "project.a").unwrap();
        let run = |command, format| render(&ctx, Invocation { command, format });
        for (command, list) in [(Command::Pages, "pages"), (Command::Surfaces, "surfaces")] {
            let out = run(command, Format::Json);
            assert!(
                out.ends_with('\n') && out.trim_end().lines().count() == 1,
                "{out}"
            );
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            let mut want = vec!["observed_at", "project", "schema", list];
            want.sort_unstable();
            assert_eq!(keys, want);
            let whole = serde_json::to_value(&ctx).unwrap();
            assert_eq!(v[list], whole[list]);
        }
        let back: ProjectContext =
            serde_json::from_str(&run(Command::Context, Format::Json)).unwrap();
        assert_eq!(back, ctx, "context は全部・知らない欄なしで読み戻せる");
        let text = run(Command::Pages, Format::Text);
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(
            text.lines().nth(2).unwrap().contains("Terminals 0"),
            "空の Page も 1 行: {text}"
        );
        assert!(
            text.lines()
                .next()
                .unwrap()
                .contains("Terminals 2 (1 running)"),
            "{text}"
        );
        assert_eq!(run(Command::Surfaces, Format::Text).lines().count(), 2);
    }
}
