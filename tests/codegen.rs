//! Codegen Engine（src/codegen）の試験。model は llm::Fake（外部 provider を叩かない）、隔離は本物の sandbox-exec と osascript。
//! 隔離の試験は「隔離の中の code が ~/ に書く・~/ を読む・network に出る・子 process を起こす・Apple Event を送る」を 1 本ずつ流し、
//! 拒否されて型つきの誤りとして戻る事を見る（docs/diagrams.md §17 の表）。混雑した機械で osascript の起動が数秒かかるので、
//! 1 版の上限は広めに取り、壁時計を assert するのは timeout の試験だけにする。
#[path = "../src/llm/mod.rs"]
mod llm;
#[allow(dead_code, unused_imports)]
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/codegen/mod.rs"]
mod codegen;

use codegen::{Fault, Options, Spec, SpecError, Stop};
use llm::fake::Fake;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn spec() -> Spec {
    Spec::parse(&json!({
        "name": "sum_and_sign",
        "description": "a と b の和を sum に、和の符号を sign（positive / negative / zero）に入れて返す",
        "input_schema": {"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}}, "required": ["a", "b"], "additionalProperties": false},
        "output_schema": {"type": "object", "properties": {"sum": {"type": "integer"}, "sign": {"enum": ["positive", "negative", "zero"]}}, "required": ["sum", "sign"], "additionalProperties": false},
        "examples": [
            {"input": {"a": 1, "b": 2}, "output": {"sum": 3, "sign": "positive"}},
            {"input": {"a": -5, "b": 2}, "output": {"sum": -3, "sign": "negative"}},
            {"input": {"a": 2, "b": -2}, "output": {"sum": 0, "sign": "zero"}}
        ]
    }).to_string()).unwrap()
}

const BROKEN: &str = "function solve(input) {\n  return input.a + ;\n}";
/// 0 を negative と言う（例 3 だけ外す = 2/3）
const NO_ZERO: &str = "function solve(input) { var s = input.a + input.b; return { sum: s, sign: s > 0 ? 'positive' : 'negative' }; }";
/// 書き方の違う 2/3（合格数が同じ）
const NO_ZERO_2: &str = "function solve(input) { var s = input.b + input.a; return { sum: s, sign: s >= 1 ? 'positive' : 'negative' }; }";
/// 1/3
const ONLY_POSITIVE: &str = "function solve(input) { return { sum: input.a + input.b, sign: 'positive' }; }";
const RIGHT: &str = "function solve(input) { var s = input.a + input.b; return { sum: s, sign: s > 0 ? 'positive' : s < 0 ? 'negative' : 'zero' }; }";

fn reply(code: &str) -> Result<Value, llm::LlmError> { Ok(json!({"code": code})) }

/// osascript を流す試験は 1 本ずつ（14 本が同時に osascript を起こすと、混雑で 1 版の上限を超えて Timeout になる = 測りたい物と違う赤）
fn serial_run(spec: &Spec, fake: &Fake, opts: &Options) -> Result<codegen::Outcome, SpecError> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    codegen::run(spec, fake, opts)
}

fn opts(name: &str) -> Options {
    let dir = std::env::temp_dir().join(format!("kangae-codegen-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // 期限は試験 (c) の外では効かせない（混雑で版が遅いと Deadline で止まり、測りたい止まり方にならない）
    Options { deadline: Duration::from_secs(600), run_timeout: Duration::from_secs(60), dir: Some(dir), ..Options::default() }
}

fn kinds(faults: &[Fault]) -> Vec<&'static str> {
    faults.iter().map(|f| match f {
        Fault::Model { .. } => "model", Fault::Syntax { .. } => "syntax", Fault::Denied { .. } => "denied", Fault::Crash { .. } => "crash",
        Fault::Thrown { .. } => "thrown", Fault::Shape { .. } => "shape", Fault::Mismatch { .. } => "mismatch",
        Fault::Timeout { .. } => "timeout", Fault::Resource { .. } => "resource",
    }).collect()
}

/// (a) 1 回目に壊れた code・2 回目に例を 1 つ外す code・3 回目に正しい code → 3 版で合格。誤りは次の生成に戻っている
#[test]
fn three_versions_reach_all_passed_and_errors_flow_back() {
    let fake = Fake::new(vec![reply(BROKEN), reply(NO_ZERO), reply(RIGHT)]);
    let o = serial_run(&spec(), &fake, &opts("a")).unwrap();
    assert_eq!(o.stop, Stop::AllPassed, "{:#?}", o.history);
    assert_eq!(o.history.len(), 3);
    assert_eq!(kinds(&o.history[0].faults), ["syntax"], "{:?}", o.history[0].faults);
    assert!(matches!(o.history[0].faults[0], Fault::Syntax { line: Some(2), .. }), "{:?}", o.history[0].faults);
    assert_eq!((o.history[1].passed, kinds(&o.history[1].faults)), (2, vec!["mismatch"]));
    assert!(matches!(&o.history[1].faults[0], Fault::Mismatch { example: 2, .. }));
    assert_eq!(o.history[2].passed, 3);
    let best = o.best.as_ref().unwrap();
    assert_eq!((best.version, best.passed), (3, 3));
    assert_eq!(best.code.unverified_source(), RIGHT);
    assert!(best.code.needs_approval());
    assert_eq!(serde_json::to_value(&best.code).unwrap()["approval"], "required");
    // 版の file と履歴が残る
    assert_eq!(std::fs::read_to_string(o.dir.join("v2/solve.js")).unwrap(), NO_ZERO);
    let h: Value = serde_json::from_str(&std::fs::read_to_string(o.dir.join("history.json")).unwrap()).unwrap();
    assert_eq!(h.as_array().unwrap().len(), 3);
    // 2 回目の生成には 1 回目の構文の誤り、3 回目には 2 回目の不一致が渡っている
    let reqs = fake.requests.lock().unwrap();
    let input = |i: usize| serde_json::from_str::<Value>(&reqs[i].input).unwrap();
    assert_eq!(input(0)["best"], Value::Null);
    assert_eq!(input(1)["rejected"]["faults"][0]["kind"], "syntax");
    assert_eq!(input(2)["best"]["faults"][0]["kind"], "mismatch");
    assert_eq!(input(2)["best"]["code"], NO_ZERO);
}

/// (b) 合格数が増えない版（同じ 2/3・減った 1/3）は採らず、続けて 3 回で止まる。最良は最初の 2/3 のまま
#[test]
fn versions_without_more_passes_are_not_taken() {
    let fake = Fake::new(vec![reply(NO_ZERO), reply(NO_ZERO_2), reply(ONLY_POSITIVE), reply(NO_ZERO_2), reply(RIGHT)]);
    let o = serial_run(&spec(), &fake, &opts("b")).unwrap();
    assert_eq!(o.stop, Stop::NoProgress, "{:#?}", o.history);
    let passed: Vec<usize> = o.history.iter().map(|v| v.passed).collect();
    let accepted: Vec<bool> = o.history.iter().map(|v| v.accepted).collect();
    assert_eq!(passed, [2, 2, 1, 2]);
    assert_eq!(accepted, [true, false, false, false]);
    let best = o.best.unwrap();
    assert_eq!((best.version, best.code.unverified_source()), (1, NO_ZERO));
    assert_eq!(kinds(&o.remaining), ["mismatch"]);
}

/// (c) 期限 0 なら 1 版で止まる（最良が無ければ最後の版の誤りを返す）
#[test]
fn deadline_zero_stops_after_one_version() {
    let fake = Fake::new(vec![reply(BROKEN), reply(RIGHT)]);
    let o = serial_run(&spec(), &fake, &Options { deadline: Duration::ZERO, ..opts("c") }).unwrap();
    assert_eq!(o.stop, Stop::Deadline);
    assert_eq!(o.history.len(), 1);
    assert!(o.best.is_none());
    assert_eq!(kinds(&o.remaining), ["syntax"]);
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
}

/// 隔離の試験の共通: code を 1 版だけ流して、その版の誤りを返す。拒否が次の生成に戻る事も見る
fn one_version(name: &str, code: &str) -> Vec<Fault> {
    let fake = Fake::new(vec![reply(code), reply(RIGHT)]);
    let o = serial_run(&spec(), &fake, &opts(name)).unwrap();
    assert_eq!(o.history.len(), 2, "{:#?}", o.history);
    let reqs = fake.requests.lock().unwrap();
    let back: Value = serde_json::from_str(&reqs[1].input).unwrap();
    let sent: Vec<&str> = back["rejected"]["faults"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
    assert_eq!(sent, kinds(&o.history[0].faults), "誤りが次の生成に戻っていない");
    o.history[0].faults.clone()
}

fn assert_denied(faults: &[Fault], needle: &str) {
    assert!(
        faults.iter().any(|f| matches!(f, Fault::Denied { message, .. } if message.contains(needle))),
        "隔離の拒否（{needle}）として戻っていない: {faults:#?}"
    );
}

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap()) }

/// ~/ に置く probe file。test が途中で落ちても消す
struct Probe(PathBuf);
impl Drop for Probe {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

/// (d) 隔離の中の code が ~/ に書こうとすると拒否され、誤り Denied として戻る（file も生えない）
#[test]
fn writing_home_is_denied() {
    let target = Probe(home().join(format!("kangae-codegen-probe-{}.txt", std::process::id())));
    let code = format!(
        "function solve(input) {{ ObjC.import('Foundation'); var e = $(); if (!$('x').writeToFileAtomicallyEncodingError({:?}, false, 4, e)) throw new Error(e.description.js); return {{ sum: -1, sign: 'zero' }}; }}",
        target.0.to_string_lossy()
    );
    let faults = one_version("d", &code);
    assert!(!target.0.exists(), "隔離の中から ~/ に file が書けた");
    assert_denied(&faults, "Operation not permitted");
}

/// ~/ を読めない（読めると中身が返り値 → 不一致の誤り → model へ、と外に出る）
#[test]
fn reading_home_is_denied() {
    let probe = Probe(home().join(format!(".kangae-codegen-read-probe-{}", std::process::id())));
    std::fs::write(&probe.0, "not-for-the-model").unwrap();
    let code = format!(
        "function solve(input) {{ ObjC.import('Foundation'); var e = $(); var s = $.NSString.stringWithContentsOfFileEncodingError({:?}, 4, e); if (s.isNil()) throw new Error(e.description.js); return {{ sum: -1, sign: s.js }}; }}",
        probe.0.to_string_lossy()
    );
    let faults = one_version("read", &code);
    assert!(!format!("{faults:?}").contains("not-for-the-model"), "~/ の中身が誤りに出た: {faults:?}");
    assert_denied(&faults, "Operation not permitted");
}

/// (e) network に出ようとすると拒否（127.0.0.1 の listener に接続が 1 本も来ない）
#[test]
fn network_is_denied() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let reached = Arc::new(AtomicBool::new(false));
    let r2 = Arc::clone(&reached);
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            r2.store(true, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nreached");
        }
    });
    let code = format!(
        "function solve(input) {{ ObjC.import('Foundation'); var e = $(); var s = $.NSString.stringWithContentsOfURLEncodingError($.NSURL.URLWithString('http://127.0.0.1:{port}/'), 4, e); if (s.isNil()) throw new Error(e.description.js); return {{ sum: -1, sign: s.js }}; }}"
    );
    let faults = one_version("e", &code);
    assert!(!reached.load(Ordering::SeqCst), "隔離の中から 127.0.0.1:{port} に接続できた: {faults:?}");
    assert_denied(&faults, "Operation not permitted");
}

/// 子 process を起こせない（NSTask）
#[test]
fn child_process_is_denied() {
    let code = "function solve(input) { ObjC.import('Foundation'); var t = $.NSTask.launchedTaskWithLaunchPathArguments('/usr/bin/true', $([])); t.waitUntilExit; return { sum: -1, sign: 'zero' }; }";
    assert_denied(&one_version("child", code), "launch path not accessible");
}

/// Apple Event を送れない（mach-lookup が無いので宛先の app を引けない）
#[test]
fn apple_event_is_denied() {
    let code = "function solve(input) { return { sum: -1, sign: Application('Finder').name() }; }";
    assert_denied(&one_version("ae", code), "Application can");
}

/// (f) 無限 loop の code は timeout で止まり、誤り Timeout として戻る
#[test]
fn infinite_loop_is_stopped_by_timeout() {
    let fake = Fake::new(vec![reply("function solve(input) { while (true) {} }")]);
    let o = serial_run(&spec(), &fake, &Options { run_timeout: Duration::from_secs(8), patience: 1, ..opts("f") }).unwrap();
    assert!(o.ms < 40_000, "{} ms", o.ms);
    assert_eq!(kinds(&o.history[0].faults), ["timeout"], "{:?}", o.history[0].faults);
    assert!(matches!(o.history[0].faults[0], Fault::Timeout { ms: 8000, reached: 0 }));
}

/// 物理 memory の上限を超えた code は kill され、誤り Resource として戻る（8 GB の機械を守る上限）。
/// 確保は約 400 MiB で止まる形にしておく（上限が効かなかった時も run_timeout で終わり、機械を埋めない）。
/// 2 版目の正しい code が同じ上限で通る = 上限は osascript 自体の大きさより上（何でも kill する上限ではない）
#[test]
fn memory_hog_is_stopped() {
    let code = "function solve(input) { var a = []; for (var i = 0; i < 50; i++) a.push(new Array(1e6).fill(i)); while (true) {} }";
    let fake = Fake::new(vec![reply(code), reply(RIGHT)]);
    let o = serial_run(&spec(), &fake, &Options { memory_bytes: 96 << 20, run_timeout: Duration::from_secs(90), ..opts("mem") }).unwrap();
    assert_eq!(kinds(&o.history[0].faults), ["resource"], "{:?}", o.history[0].faults);
    assert!(matches!(&o.history[0].faults[0], Fault::Resource { what } if what.contains("memory")), "{:?}", o.history[0].faults);
    assert_eq!((o.stop, o.history[1].passed), (Stop::AllPassed, 3), "{:?}", o.history[1].faults);
}

/// 結果 file は 1 MiB で止まる（rlimit FSIZE。書ける file の数は例の数なので、隔離の中から書ける量が有限）
#[test]
fn huge_result_is_capped_on_disk() {
    let fake = Fake::new(vec![reply("function solve(input) { return { sum: 0, sign: 'x'.repeat(2000000) }; }")]);
    let o = serial_run(&spec(), &fake, &Options { patience: 1, ..opts("fsize") }).unwrap();
    let size = std::fs::metadata(o.dir.join("v1/out/0.json")).map(|m| m.len()).unwrap_or(0);
    assert!(size <= 1 << 20, "結果 file が {size} byte 書けた");
    assert!(kinds(&o.history[0].faults).contains(&"resource"), "{:?}", o.history[0].faults);
}

/// 仕様の検査: 例が 3 組未満・鍵の形・例が schema に合わない は生成の前に止まる（model を呼ばない）
#[test]
fn bad_specs_are_rejected_before_generation() {
    let fake = Fake::new(vec![reply(RIGHT)]);
    let mut s = spec();
    s.examples.truncate(2);
    assert_eq!(codegen::run(&s, &fake, &opts("spec1")).unwrap_err(), SpecError::TooFewExamples(2));

    let mut s = spec();
    s.description = format!("{} sk-ant-api03-{}", s.description, "x".repeat(24));
    assert_eq!(codegen::run(&s, &fake, &opts("spec2")).unwrap_err(), SpecError::Secret { path: "$.description".into() });

    let mut s = spec();
    s.examples[1].input = json!({"a": "1", "b": 2});
    assert_eq!(codegen::run(&s, &fake, &opts("spec3")).unwrap_err(),
        SpecError::Example { index: 1, side: "input", reason: "$.a: integer ではありません（\"1\"）".into() });

    // 鍵の名前（token・password）だけなら拒否しない（パスワードの強さを測る関数は作れる）
    let mut s = spec();
    s.description = "password の token を数える".into();
    assert!(codegen::check(&s).is_ok());
    assert!(fake.requests.lock().unwrap().is_empty());
}

/// ホストで直接実行する道が 0: spawn はこの file の Command::new(SANDBOX) の 1 か所だけ、bin には spawn が無い
#[test]
fn no_path_runs_generated_code_outside_the_sandbox() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/codegen/mod.rs")).unwrap();
    let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let spawns: Vec<&str> = code.match_indices("Command::new(").map(|(i, m)| {
        let rest = &code[i + m.len()..];
        &rest[..rest.find(')').unwrap_or(0)]
    }).collect();
    assert_eq!(spawns, ["SANDBOX"], "spawn が sandbox-exec 以外にある");
    assert_eq!(codegen::SANDBOX, "/usr/bin/sandbox-exec");
    for forbidden in ["std::process::Command::new", "process::Command::new", ".exec()", "libc::", "posix_spawn", "fork("] {
        assert!(!code.contains(forbidden), "{forbidden} がある");
    }
    let bin = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bin/kangae-codegen.rs")).unwrap();
    assert!(!bin.contains("Command"), "CLI が process を起こしている");
}

/// profile の形: 既定で拒否・network を明示で拒否・mach-lookup / fork / network の許可が無い
#[test]
fn profile_denies_by_default() {
    let p = codegen::profile(std::path::Path::new("/private/tmp/v1"), &[PathBuf::from("/private/tmp/v1/out/0.json")]);
    assert!(p.contains("(deny default)") && p.contains("(deny network*)"), "{p}");
    for allowed in ["(allow network", "mach-lookup", "process-fork", "appleevent", "(allow file-write* (subpath"] {
        assert!(!p.contains(allowed), "{allowed} が profile にある:\n{p}");
    }
    assert!(p.contains("(allow file-write* (literal \"/private/tmp/v1/out/0.json\"))"), "{p}");
}
