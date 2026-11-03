//! Codegen Engine（DESIGN §16.3-5）: 在る物で足りない時だけ、足りない部分を作る。分岐は docs/diagrams.md §17。
//!
//! - 言語: `osascript -l JavaScript`（macOS に最初から在る JXA）の純粋な関数 `function solve(input)`。入出力は JSON。
//! - 隔離: 生成した code は `sandbox-exec -p PROFILE` の中でだけ動かす。spawn は `isolate` の 1 か所だけで、ホストの shell・
//!   osascript で直接動かす道は無い（tests/codegen.rs がこの file の `Command::new` を数える）。
//! - loop: 生成 → 隔離して例を流す → 型つきの誤り（`Fault`）を次の生成へ。例の合格数が真に増えた版だけ採る。
//!   止まるのは 全部通った / 続けて k 回良くならない / 期限 の 3 つだけ。各版は作業 dir の `vN/` に file で残し、`history.json` に履歴。
//! - 返す code は `Unverified`（要承認）。harness と code は同じ process で動くので、悪意のある code は結果を偽れる ——
//!   合格は「例を通した」であって、安全や正しさの証明ではない。
//!
//! bin crate の module だが src/bin と tests/ から `#[path]` で取り込むので、中で `crate::` を書かない（`super::llm`・`super::storage`）。
#![allow(dead_code)] // 統合役が Refine Engine に配線するまで main から呼ばれない

use super::llm::{validate, JsonRequest, Llm, LlmError};
use super::storage::log::redact;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs, thread};

pub const SANDBOX: &str = "/usr/bin/sandbox-exec";
pub const OSASCRIPT: &str = "/usr/bin/osascript";
/// 生成した code の上限（これより長い答えは誤り Model）
const MAX_CODE: usize = 64 * 1024;
/// 例 1 つの結果 file の上限（rlimit FSIZE と同じ。超えると kernel が SIGXFSZ で止める）
const MAX_RESULT: u64 = 1024 * 1024;
/// 例の入力を argv で渡すので、その上限（ARG_MAX より十分小さく）
const MAX_INPUTS: usize = 256 * 1024;
/// 次の生成に戻す文字列・値の長さ
const FEEDBACK_CHARS: usize = 1500;

// ============================== 仕様 ==============================

/// 「足りない部分」の仕様。例は 3 組以上（例が全部通るまで回すので、例がそのまま合格の定義になる）
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub examples: Vec<Example>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub input: Value,
    pub output: Value,
}

/// 仕様を受け付けない理由。どれも生成の前に止まる（model を呼ばない・dir も作らない）
#[derive(Clone, Debug, PartialEq)]
pub enum SpecError {
    Parse(String),
    Missing(&'static str),
    TooFewExamples(usize),
    Schema(&'static str),
    Example { index: usize, side: &'static str, reason: String },
    /// 鍵の形をした文字列（storage::log::redact が伏せる形・PEM の秘密鍵）。path は "$.examples[0].input.key" の形
    Secret { path: String },
    TooLarge { bytes: usize },
    Io(String),
}
impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "仕様を読めません（{e}）。name・description・input_schema・output_schema・examples の JSON にしてください。"),
            Self::Missing(k) => write!(f, "仕様の {k} が空です。"),
            Self::TooFewExamples(n) => write!(f, "例が {n} 組です。入力と出力の例を 3 組以上にしてください。"),
            Self::Schema(k) => write!(f, "仕様の {k} が JSON Schema の object ではありません。"),
            Self::Example { index, side, reason } => write!(f, "例 {} の {side} が schema に合いません（{reason}）。", index + 1),
            Self::Secret { path } => write!(f, "仕様の {path} に鍵の形をした値があります。鍵を仕様と例に入れず、架空の値に置き換えてください。"),
            Self::TooLarge { bytes } => write!(f, "例の入力が合わせて {bytes} byte あります。{MAX_INPUTS} byte 以下にしてください。"),
            Self::Io(e) => write!(f, "作業 dir を作れません（{e}）。"),
        }
    }
}
impl std::error::Error for SpecError {}

impl Spec {
    pub fn parse(text: &str) -> Result<Spec, SpecError> {
        serde_json::from_str(text).map_err(|e| SpecError::Parse(e.to_string()))
    }
}

/// 生成の前の検査。例が schema に合わない仕様は、どんな code でも全部は通らない
pub fn check(spec: &Spec) -> Result<(), SpecError> {
    if spec.name.trim().is_empty() { return Err(SpecError::Missing("name")) }
    if spec.description.trim().is_empty() { return Err(SpecError::Missing("description")) }
    if !spec.input_schema.is_object() { return Err(SpecError::Schema("input_schema")) }
    if !spec.output_schema.is_object() { return Err(SpecError::Schema("output_schema")) }
    if spec.examples.len() < 3 { return Err(SpecError::TooFewExamples(spec.examples.len())) }
    let raw = serde_json::to_value(spec).map_err(|e| SpecError::Parse(e.to_string()))?;
    if let Some(path) = find_secret(&raw, "$") { return Err(SpecError::Secret { path }) }
    for (index, ex) in spec.examples.iter().enumerate() {
        for (side, schema, v) in [("input", &spec.input_schema, &ex.input), ("output", &spec.output_schema, &ex.output)] {
            validate(schema, v).map_err(|e| SpecError::Example { index, side, reason: llm_reason(&e) })?;
        }
    }
    let bytes = inputs_json(spec).len();
    if bytes > MAX_INPUTS { return Err(SpecError::TooLarge { bytes }) }
    Ok(())
}

/// 鍵の形の表は storage::log の 1 か所（log に出す時に伏せる形 = 仕様に入れて model へ送らない形）。PEM の秘密鍵だけ足す
fn find_secret(v: &Value, path: &str) -> Option<String> {
    let secret = |s: &str| redact(s) != s || (s.contains("-----BEGIN") && s.contains("PRIVATE KEY"));
    match v {
        Value::String(s) => secret(s).then(|| path.to_string()),
        Value::Array(a) => a.iter().enumerate().find_map(|(i, x)| find_secret(x, &format!("{path}[{i}]"))),
        Value::Object(o) => o.iter().find_map(|(k, x)| {
            let p = format!("{path}.{k}");
            if secret(k) { Some(p) } else { find_secret(x, &p) }
        }),
        _ => None,
    }
}

fn inputs_json(spec: &Spec) -> String {
    Value::Array(spec.examples.iter().map(|e| e.input.clone()).collect()).to_string()
}

fn llm_reason(e: &LlmError) -> String {
    match e {
        LlmError::Invalid { path, reason } => format!("{path}: {reason}"),
        other => other.to_string(),
    }
}

// ============================== 誤り・版・結果 ==============================

/// 版の誤り。次の生成にそのまま（長い値は切って）戻す
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fault {
    /// model が code を返せなかった（LlmError・空・長すぎる）
    Model { message: String },
    /// 構文の誤り（osascript が読めない）。line は solve.js の行
    Syntax { line: Option<u32>, message: String },
    /// 隔離の拒否（file・network・子 process・Apple Event）。example は solve() の中で起きた時の例の番号（0 起点）
    Denied { example: Option<usize>, message: String },
    /// solve() の外で落ちた（top level の throw・harness が結果を書けない）
    Crash { message: String },
    /// solve() が投げた
    Thrown { example: usize, message: String },
    /// 返り値が output_schema に合わない（undefined を含む）
    Shape { example: usize, got: Value, reason: String },
    /// 返り値が期待と違う
    Mismatch { example: usize, expected: Value, got: Value },
    /// 壁時計の上限で止めた。reached は結果を書けた例の数
    Timeout { ms: u64, reached: usize },
    /// memory・出力の上限で止めた
    Resource { what: String },
}
impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Model { message } => write!(f, "生成できませんでした: {message}"),
            Self::Syntax { line: Some(l), message } => write!(f, "構文の誤り（{l} 行目）: {message}"),
            Self::Syntax { line: None, message } => write!(f, "構文の誤り: {message}"),
            Self::Denied { example: Some(i), message } => write!(f, "例 {} で隔離が拒否しました（file・network・子 process・他の app は使えません）: {message}", i + 1),
            Self::Denied { example: None, message } => write!(f, "隔離が拒否しました（file・network・子 process・他の app は使えません）: {message}"),
            Self::Crash { message } => write!(f, "solve() の外で止まりました: {message}"),
            Self::Thrown { example, message } => write!(f, "例 {} で solve() が投げました: {message}", example + 1),
            Self::Shape { example, got, reason } => write!(f, "例 {} の返り値 {} が output_schema に合いません: {reason}", example + 1, clip(&got.to_string())),
            Self::Mismatch { example, expected, got } => write!(f, "例 {} が違います: 期待 {} / 返り値 {}", example + 1, clip(&expected.to_string()), clip(&got.to_string())),
            Self::Timeout { ms, reached } => write!(f, "{} 秒で止めました（例 {reached} 個まで結果あり）。終わらない loop が無いか確かめてください。", *ms as f64 / 1000.0),
            Self::Resource { what } => write!(f, "上限を超えたので止めました: {what}"),
        }
    }
}

/// 生成した code。例は通したが、安全や正しさの証明ではない（DESIGN §16.3-5: 利用者の許可なしにホストで実行しない）。
/// 中身を読む口は `unverified_source` だけ（名前で要承認を思い出させる）
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unverified {
    source: String,
    approval: &'static str,
}
impl Unverified {
    fn new(source: String) -> Self { Self { source, approval: "required" } }
    pub fn unverified_source(&self) -> &str { &self.source }
    pub fn needs_approval(&self) -> bool { true }
}

#[derive(Clone, Debug, Serialize)]
pub struct Version {
    pub n: u32,
    /// 作業 dir からの相対 path（vN/solve.js）。生成に失敗した版は None
    pub file: Option<PathBuf>,
    pub passed: usize,
    pub total: usize,
    pub faults: Vec<Fault>,
    /// 合格数が最良より真に増えて採った
    pub accepted: bool,
    pub ms: u64,
    #[serde(skip)]
    code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    AllPassed,
    NoProgress,
    Deadline,
}
impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::AllPassed => "例を全部通した",
            Self::NoProgress => "続けて良くならなかった",
            Self::Deadline => "期限",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Best {
    pub version: u32,
    pub passed: usize,
    pub code: Unverified,
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub stop: Stop,
    /// 最良版（合格が 1 つも無ければ None）
    pub best: Option<Best>,
    pub total: usize,
    /// 最良版に残った誤り（最良版が無ければ最後の版の誤り）
    pub remaining: Vec<Fault>,
    pub history: Vec<Version>,
    pub dir: PathBuf,
    pub ms: u64,
}

// ============================== loop ==============================

#[derive(Clone, Debug)]
pub struct Options {
    /// 既定 120 秒。版と版の間で見る（版の途中では切らない。次の版が収まらない見込みなら始めない）
    pub deadline: Duration,
    /// 続けて良くならない版の数（既定 3）
    pub patience: u32,
    /// 1 版の隔離実行の壁時計（既定 20 秒）
    pub run_timeout: Duration,
    /// 隔離した process の物理 memory の上限（既定 512 MiB）
    pub memory_bytes: u64,
    /// 作業 dir（無ければ temp_dir の下に作る）
    pub dir: Option<PathBuf>,
}
impl Default for Options {
    fn default() -> Self {
        Self { deadline: Duration::from_secs(120), patience: 3, run_timeout: Duration::from_secs(20), memory_bytes: 512 << 20, dir: None }
    }
}

const INSTRUCTIONS: &str = "macOS の JavaScript for Automation（osascript -l JavaScript）で純粋な関数を 1 つ書く。
- `function solve(input)` を定義する。input は仕様の input_schema に合う値、返り値は output_schema に合う JSON の値。
- 使ってよいのは JavaScript の言語と標準の object（Math・JSON・String・Array・Object・Number・Date・RegExp・Intl 等）だけ。
  Application()・ObjC・$・Library()・doShellScript・delay・file・network・他の app は使わない（隔離の中で拒否されて誤りになる）。
- run() は定義しない（呼び出し側が付ける）。console.log ではなく return で返す。
- 例（examples）の入力を表で引いて答える作りにせず、description の規則を実装する。例は全部この関数で通ること。
- best（今の最良版）と rejected（直前に採らなかった版）が渡されたら、faults を全部消すように直した全文を返す。
返すのは {\"code\": \"<solve を定義した JavaScript の全文>\"} だけ。";

fn reply_schema() -> Value {
    json!({"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"], "additionalProperties": false})
}

/// 生成 → 隔離して例を流す → 誤りを戻して直す を回す。spec が受け付けられない時だけ Err（その時は model を呼ばない）
pub fn run(spec: &Spec, llm: &dyn Llm, opts: &Options) -> Result<Outcome, SpecError> {
    check(spec)?;
    let started = Instant::now();
    let dir = work_dir(opts.dir.as_deref()).map_err(|e| SpecError::Io(e.to_string()))?;
    let total = spec.examples.len();
    let mut history: Vec<Version> = Vec::new();
    let mut best: Option<usize> = None; // history の添字
    let mut stale = 0u32;
    let mut slowest = Duration::ZERO;
    let stop = loop {
        let t0 = Instant::now();
        let n = history.len() as u32 + 1;
        let rejected = history.last().filter(|v| !v.accepted);
        let mut v = match generate(llm, spec, best.map(|i| &history[i]), rejected) {
            Err(fault) => Version { n, file: None, passed: 0, total, faults: vec![fault], accepted: false, ms: 0, code: None },
            Ok(code) => {
                let (passed, faults) = evaluate(&code, spec, &dir.join(format!("v{n}")), opts);
                Version { n, file: Some(PathBuf::from(format!("v{n}/solve.js"))), passed, total, faults, accepted: false, ms: 0, code: Some(code) }
            }
        };
        // 例の合格数が真に増えた版だけ採る（同じ・減ったは捨てる）
        let best_passed = best.map_or(0, |i| history[i].passed);
        v.accepted = v.passed > best_passed;
        v.ms = t0.elapsed().as_millis() as u64;
        slowest = slowest.max(t0.elapsed());
        if v.accepted { best = Some(history.len()); stale = 0 } else { stale += 1 }
        let all = v.accepted && v.passed == total;
        history.push(v);
        let _ = fs::write(dir.join("history.json"), serde_json::to_string_pretty(&history).unwrap_or_default());
        if all { break Stop::AllPassed }
        let elapsed = started.elapsed();
        if elapsed >= opts.deadline || opts.deadline - elapsed < slowest { break Stop::Deadline }
        if stale >= opts.patience { break Stop::NoProgress }
    };
    let best_v = best.map(|i| &history[i]);
    let remaining = best_v.or(history.last()).map(|v| v.faults.clone()).unwrap_or_default();
    let best = best_v.map(|v| Best { version: v.n, passed: v.passed, code: Unverified::new(v.code.clone().unwrap_or_default()) });
    Ok(Outcome { stop, best, total, remaining, history, dir, ms: started.elapsed().as_millis() as u64 })
}

fn generate(llm: &dyn Llm, spec: &Spec, best: Option<&Version>, rejected: Option<&Version>) -> Result<String, Fault> {
    let attempt = |v: &Version| json!({
        "version": v.n, "passed": v.passed, "total": v.total, "code": v.code,
        "faults": v.faults.iter().map(feedback).collect::<Vec<_>>(),
    });
    let input = json!({
        "spec": spec,
        "best": best.map(attempt),
        "rejected": rejected.map(attempt),
    });
    let req = JsonRequest::new("kangae_codegen_jxa", INSTRUCTIONS, &input.to_string(), reply_schema());
    let v = llm.complete_json(req).map_err(|e| Fault::Model { message: e.to_string() })?;
    let code = v["code"].as_str().unwrap_or("").trim().to_string();
    if code.is_empty() { return Err(Fault::Model { message: "code が空です".into() }) }
    if code.len() > MAX_CODE { return Err(Fault::Model { message: format!("code が {} byte あります（上限 {MAX_CODE}）", code.len()) }) }
    Ok(code)
}

/// 次の生成へ戻す形（型 + 文言。長い値は切る）
fn feedback(f: &Fault) -> Value {
    let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        for x in o.values_mut() {
            let s = match x { Value::String(s) => s.clone(), Value::Array(_) | Value::Object(_) => x.to_string(), _ => continue };
            if s.chars().count() > FEEDBACK_CHARS { *x = Value::String(clip(&s)) }
        }
        o.insert("text".into(), Value::String(f.to_string()));
    }
    v
}

fn clip(s: &str) -> String {
    if s.chars().count() > FEEDBACK_CHARS { format!("{}…", s.chars().take(FEEDBACK_CHARS).collect::<String>()) } else { s.to_string() }
}

/// 作業 dir を作って canonicalize する。kernel は /var/folders を /private/var/folders として見るので、
/// profile には canonical な path を書く（書かないと書き込みが全部拒否される）
fn work_dir(dir: Option<&Path>) -> io::Result<PathBuf> {
    let d = match dir {
        Some(d) => d.to_path_buf(),
        None => {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            std::env::temp_dir().join(format!("kangae-codegen-{}-{nanos}", std::process::id()))
        }
    };
    fs::create_dir_all(&d)?;
    fs::canonicalize(&d)
}

// ============================== 評価（隔離して例を流す） ==============================

/// harness: code の後ろに付ける固定の run()。例を 1 つずつ solve() に通し、結果を out/i.json に 1 つずつ書く
/// （timeout で kill されても、そこまでの結果が残る）。code を先に置くので、構文の誤りの行番号は solve.js と同じ
const HARNESS: &str = r#"
function run(argv) {
  ObjC.import('Foundation');
  var out = argv[0], inputs = JSON.parse(argv[1]);
  for (var i = 0; i < inputs.length; i++) {
    var r;
    try {
      var v = solve(inputs[i]);
      r = v === undefined ? { k: "undefined" } : { k: "ok", v: v };
    } catch (e) {
      r = { k: "thrown", m: String(e && e.message !== undefined ? e.message : e) };
    }
    var s;
    try { s = JSON.stringify(r); } catch (e) { s = JSON.stringify({ k: "thrown", m: "返り値を JSON にできません: " + e }); }
    if (!$(s).writeToFileAtomicallyEncodingError(out + "/" + i + ".json", false, $.NSUTF8StringEncoding, null)) {
      throw new Error("harness: 例 " + (i + 1) + " の結果を書けません");
    }
  }
  return "";
}
"#;

/// 隔離の profile。最小の集合は 14.5 の実物で決めた（docs/diagrams.md §17 の表）:
/// 起動は osascript だけ（fork 無し）・読むのは system と版の dir・書くのは例の数だけの結果 file・network 無し・
/// mach-lookup 無し（Apple Event と標準追加の doShellScript はこれで LaunchServices と XPC を引けずに落ちる）
pub fn profile(vdir: &Path, outputs: &[PathBuf]) -> String {
    let writes: String = outputs.iter().map(|p| format!(" (literal {})", sbpl(p))).collect();
    format!(
        "(version 1)
(deny default)
(allow process-exec (literal \"{OSASCRIPT}\"))
(allow sysctl-read)
(allow file-read* (literal \"/\") (subpath \"/System\") (subpath \"/usr\") (subpath \"/private/etc\") (subpath \"/private/var/db\") (subpath {}))
(allow file-write*{writes})
(deny network*)
",
        sbpl(vdir)
    )
}

/// SBPL の文字列 literal
fn sbpl(p: &Path) -> String {
    format!("\"{}\"", p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""))
}

fn evaluate(code: &str, spec: &Spec, vdir: &Path, opts: &Options) -> (usize, Vec<Fault>) {
    let out = vdir.join("out");
    let total = spec.examples.len();
    let outputs: Vec<PathBuf> = (0..total).map(|i| out.join(format!("{i}.json"))).collect();
    let harness = vdir.join("harness.js");
    let written = fs::create_dir_all(&out)
        .and_then(|_| fs::write(vdir.join("solve.js"), code))
        .and_then(|_| fs::write(&harness, format!("{code}\n{HARNESS}")));
    if let Err(e) = written { return (0, vec![Fault::Crash { message: format!("版の file を書けません: {e}") }]) }
    // --dir に前の回の vN/out が残っていても、その結果を数えない
    for p in &outputs { let _ = fs::remove_file(p); }
    let (out_arg, inputs) = (out.to_string_lossy().into_owned(), inputs_json(spec));
    let ran = isolate(&profile(vdir, &outputs), &out, &harness, &[&out_arg, &inputs], opts);
    let mut faults = Vec::new();
    match ran.end {
        End::Timeout => {
            let reached = outputs.iter().filter(|p| p.exists()).count();
            faults.push(Fault::Timeout { ms: opts.run_timeout.as_millis() as u64, reached });
        }
        End::Memory => faults.push(Fault::Resource { what: format!("物理 memory が {} MiB を超えた", opts.memory_bytes >> 20) }),
        End::Spawn(e) => return (0, vec![Fault::Crash { message: format!("sandbox-exec を起動できません: {e}") }]),
        End::Exit(status) => {
            if signal(status) == Some(SIGXFSZ) {
                faults.push(Fault::Resource { what: format!("例の結果が {} MiB を超えた", MAX_RESULT >> 20) })
            } else if !status.success() || outputs.iter().any(|p| !p.exists()) {
                let message = error_text(&ran.stderr, vdir);
                let none = outputs.iter().all(|p| !p.exists());
                faults.push(if none && message.contains("SyntaxError") {
                    Fault::Syntax { line: line_of(&message), message }
                } else if denied(&message) {
                    Fault::Denied { example: None, message }
                } else if message.is_empty() {
                    Fault::Crash { message: format!("結果が {} 例分しかありません（{status}）。run() を書き換えていないか確かめてください", outputs.iter().filter(|p| p.exists()).count()) }
                } else {
                    Fault::Crash { message }
                });
            }
        }
    }
    let mut passed = 0;
    for (i, (ex, path)) in spec.examples.iter().zip(&outputs).enumerate() {
        let r = match read_result(path) {
            None => continue, // 未到達（timeout・crash の誤りは上で足した）
            Some(Err(what)) => { faults.push(Fault::Resource { what: format!("例 {} の結果: {what}", i + 1) }); continue }
            Some(Ok(r)) => r,
        };
        let fault = match r["k"].as_str() {
            Some("ok") if same(&r["v"], &ex.output) => { passed += 1; continue }
            Some("ok") => match validate(&spec.output_schema, &r["v"]) {
                Err(e) => Fault::Shape { example: i, got: r["v"].clone(), reason: llm_reason(&e) },
                Ok(()) => Fault::Mismatch { example: i, expected: ex.output.clone(), got: r["v"].clone() },
            },
            Some("undefined") => Fault::Shape { example: i, got: Value::Null, reason: "solve() が undefined を返しました（return を忘れていないか）".into() },
            _ => {
                let message = clip(r["m"].as_str().unwrap_or(""));
                if denied(&message) { Fault::Denied { example: Some(i), message } } else { Fault::Thrown { example: i, message } }
            }
        };
        faults.push(fault);
    }
    (passed, faults)
}

/// 結果 file。無ければ None（未到達）、大きすぎる・JSON でない（書く途中で止められた）は Err
fn read_result(path: &Path) -> Option<Result<Value, String>> {
    let mut f = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    if (&mut f).take(MAX_RESULT + 1).read_to_end(&mut bytes).is_err() { return Some(Err("読めません".into())) }
    if bytes.len() as u64 > MAX_RESULT { return Some(Err(format!("{} MiB を超えた", MAX_RESULT >> 20))) }
    Some(serde_json::from_slice(&bytes).map_err(|_| "書く途中で止まった（JSON として壊れている）".to_string()))
}

/// JSON の値として同じか（数は f64 で比べる: JSON.stringify は 1.0 を 1 と書くので、u64 の 1 と f64 の 1.0 を同じに扱う）
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64().zip(y.as_f64()).is_some_and(|(x, y)| x == y),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

/// 隔離の拒否の文言（14.5 の実測: file と network は POSIX 1 `Operation not permitted`、NSTask は `launch path not accessible`、
/// Apple Event は LaunchServices を引けず -2700 `Application can't be found`、標準追加は -1708、TCC の拒否は -1743）
fn denied(message: &str) -> bool {
    ["Operation not permitted", "launch path not accessible", "Application can't be found", "Application can’t be found", "(-1708)", "(-1743)"]
        .iter()
        .any(|s| message.contains(s))
}

/// stderr から osascript の誤りの行だけを取る（XPC や CFURL の雑音は捨てる）。作業 dir の path は vN/… に縮める
fn error_text(stderr: &str, vdir: &Path) -> String {
    let base = vdir.parent().map(|p| format!("{}/", p.to_string_lossy())).unwrap_or_default();
    let lines: Vec<&str> = stderr.lines().filter(|l| l.contains("error") && !l.contains("Connection") && !l.contains("CFURLCopy") && !l.contains("DEBUG:")).collect();
    clip(&lines.join("\n").replace(&base, ""))
}

/// "Error on line 2: SyntaxError: …" の 2
fn line_of(message: &str) -> Option<u32> {
    let rest = &message[message.find("on line ")? + 8..];
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

// ============================== 隔離した実行（spawn はここだけ） ==============================

enum End {
    Exit(ExitStatus),
    Timeout,
    Memory,
    Spawn(String),
}
struct Ran {
    end: End,
    stderr: String,
}

/// sandbox-exec の中で osascript を 1 回動かす。env は空にして HOME / TMPDIR を版の out に、rlimit で CPU 時間と file の大きさを、
/// 壁時計と物理 memory は外から見て kill する（sandbox-exec は exec するので child の pid が osascript）
fn isolate(profile: &str, cwd: &Path, script: &Path, args: &[&str], opts: &Options) -> Ran {
    let mut cmd = Command::new(SANDBOX);
    cmd.arg("-p").arg(profile).arg(OSASCRIPT).args(["-l", "JavaScript"]).arg(script).args(args)
        .env_clear().env("PATH", "/usr/bin:/bin").env("HOME", cwd).env("TMPDIR", cwd).env("LANG", "en_US.UTF-8")
        .current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    limits(&mut cmd, opts.run_timeout.as_secs() + 2);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ran { end: End::Spawn(e.to_string()), stderr: String::new() },
    };
    // stderr は thread で読み切る（pipe が詰まって child が止まらないように）。残すのは先頭 64 KiB
    let err = child.stderr.take().map(|mut r| thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut r).take(64 * 1024).read_to_end(&mut kept);
        let _ = io::copy(&mut r, &mut io::sink());
        String::from_utf8_lossy(&kept).into_owned()
    }));
    let until = Instant::now() + opts.run_timeout;
    let end = loop {
        match child.try_wait() {
            Ok(Some(s)) => break End::Exit(s),
            Ok(None) => {}
            Err(e) => { let _ = child.kill(); let _ = child.wait(); break End::Spawn(e.to_string()) }
        }
        if Instant::now() >= until { let _ = child.kill(); let _ = child.wait(); break End::Timeout }
        if footprint(child.id()) > opts.memory_bytes { let _ = child.kill(); let _ = child.wait(); break End::Memory }
        thread::sleep(Duration::from_millis(25));
    };
    Ran { end, stderr: err.and_then(|t| t.join().ok()).unwrap_or_default() }
}

const SIGXFSZ: i32 = 25;
fn signal(s: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    { std::os::unix::process::ExitStatusExt::signal(&s) }
    #[cfg(not(unix))]
    { let _ = s; None }
}

/// rlimit: CPU 時間（壁時計が効かなかった時の kernel 側の止め）と、1 つの file の大きさ（結果 file の上限）
#[cfg(unix)]
fn limits(cmd: &mut Command, cpu_secs: u64) {
    use std::os::unix::process::CommandExt;
    #[repr(C)]
    struct RLimit { cur: u64, max: u64 }
    extern "C" { fn setrlimit(resource: i32, rlp: *const RLimit) -> i32; }
    const RLIMIT_CPU: i32 = 0;
    const RLIMIT_FSIZE: i32 = 1;
    // pre_exec の中は fork の後なので、allocation をせず setrlimit だけを呼ぶ
    let set = move || {
        for (res, n) in [(RLIMIT_CPU, cpu_secs), (RLIMIT_FSIZE, MAX_RESULT)] {
            if unsafe { setrlimit(res, &RLimit { cur: n, max: n }) } != 0 { return Err(io::Error::last_os_error()) }
        }
        Ok(())
    };
    unsafe { cmd.pre_exec(set) };
}
#[cfg(not(unix))]
fn limits(_: &mut Command, _: u64) {}

/// 物理 memory（phys_footprint）。libproc の proc_pid_rusage（RUSAGE_INFO_V0）。読めなければ 0
#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> u64 {
    #[repr(C)]
    struct RusageInfoV0 { uuid: [u8; 16], user_time: u64, system_time: u64, pkg_idle_wkups: u64, interrupt_wkups: u64, pageins: u64, wired_size: u64, resident_size: u64, phys_footprint: u64, proc_start_abstime: u64, proc_exit_abstime: u64 }
    extern "C" { fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32; }
    let mut info = std::mem::MaybeUninit::<RusageInfoV0>::zeroed();
    if unsafe { proc_pid_rusage(pid as i32, 0, info.as_mut_ptr()) } != 0 { return 0 }
    unsafe { info.assume_init() }.phys_footprint
}
#[cfg(not(target_os = "macos"))]
fn footprint(_: u32) -> u64 { 0 }
