//! Codegen Engine の CLI（DESIGN §16.3-5）。「足りない部分」の仕様から、例を全部通す JXA の関数を作る。
//!   kangae-codegen <spec.json> [--deadline-secs N] [--patience K] [--run-timeout-secs S] [--dir PATH] [--json]
//! 生成した code は sandbox-exec の中でだけ動かし、返す code は要承認（この CLI はホストで実行しない）。
//! src/main.rs とは独立に compile する（codegen と、それが使う llm・storage を #[path] で取り込む）。分岐は docs/diagrams.md §17。

#[path = "../llm/mod.rs"]
mod llm;
#[allow(dead_code, unused_imports)]
#[path = "../storage/mod.rs"]
mod storage;
#[path = "../codegen/mod.rs"]
mod codegen;

use codegen::{Options, Outcome, Spec, Stop};
use llm::Llm;
use std::time::Duration;

const USAGE: &str = "使い方:
  kangae-codegen <spec.json> [--deadline-secs N] [--patience K] [--run-timeout-secs S] [--dir PATH] [--json]
spec.json: {\"name\", \"description\", \"input_schema\", \"output_schema\", \"examples\": [{\"input\", \"output\"} を 3 組以上]}
  --deadline-secs N     全体の期限（既定 120。版と版の間で見る）
  --patience K          続けて良くならない版がこの数で止める（既定 3）
  --run-timeout-secs S  1 版の隔離実行の上限（既定 20）
  --dir PATH            版の file と history.json を置く dir（既定は一時 dir の下）
  --json                結果を JSON で出す（全版の履歴つき）
model: ANTHROPIC_API_KEY → Keychain kangae-anthropic → OPENAI_API_KEY → Keychain kangae-openai の順に探す
試験用: KANGAE_CODEGEN_REPLIES=<file>  model の代わりに file の JSON 配列（{\"code\": …} の並び）を順に返す";

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(args: Vec<String>) -> i32 {
    let mut opts = Options::default();
    let mut spec_path = None;
    let mut as_json = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let secs = |v: Option<String>| v.and_then(|s| s.parse::<u64>().ok());
        let ok = match a.as_str() {
            "-h" | "--help" => { println!("{USAGE}"); return 0 }
            "--json" => { as_json = true; true }
            "--deadline-secs" => secs(it.next()).map(|n| opts.deadline = Duration::from_secs(n)).is_some(),
            "--run-timeout-secs" => secs(it.next()).filter(|n| *n > 0).map(|n| opts.run_timeout = Duration::from_secs(n)).is_some(),
            "--patience" => secs(it.next()).filter(|n| *n > 0).map(|n| opts.patience = n as u32).is_some(),
            "--dir" => it.next().map(|d| opts.dir = Some(d.into())).is_some(),
            s if !s.starts_with('-') && spec_path.is_none() => { spec_path = Some(a.clone()); true }
            _ => false,
        };
        if !ok { eprintln!("{a} を読めません（値が無いか数でない）\n{USAGE}"); return 2 }
    }
    let Some(spec_path) = spec_path else { eprintln!("{USAGE}"); return 2 };
    let spec = match std::fs::read_to_string(&spec_path).map_err(|e| e.to_string()).and_then(|t| Spec::parse(&t).map_err(|e| e.to_string())) {
        Ok(s) => s,
        Err(e) => { eprintln!("{spec_path}: {e}"); return 2 }
    };
    let model: Box<dyn Llm> = match std::env::var("KANGAE_CODEGEN_REPLIES") {
        Ok(path) => match replies(&path) {
            Ok(f) => Box::new(f),
            Err(e) => { eprintln!("KANGAE_CODEGEN_REPLIES（{path}）: {e}"); return 2 }
        },
        Err(_) => match llm::from_env() {
            Some(m) => m,
            None => { eprintln!("{}", llm::LlmError::NoKey); return 2 }
        },
    };
    let outcome = match codegen::run(&spec, model.as_ref(), &opts) {
        Ok(o) => o,
        Err(e) => { eprintln!("{e}"); return 2 }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&outcome).unwrap_or_default());
    } else {
        print_human(&outcome);
    }
    if outcome.stop == Stop::AllPassed { 0 } else { 1 }
}

fn replies(path: &str) -> Result<llm::fake::Fake, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| format!("JSON の配列ではありません（{e}）"))?;
    Ok(llm::fake::Fake::new(v.into_iter().map(Ok).collect()))
}

fn print_human(o: &Outcome) {
    let secs = o.ms as f64 / 1000.0;
    match &o.best {
        Some(b) if o.stop == Stop::AllPassed => println!("例 {}/{} を通した（v{}・{secs:.1} 秒）", b.passed, o.total, b.version),
        Some(b) => println!("例 {}/{} まで（最良 v{}・止まった理由: {}・{secs:.1} 秒）", b.passed, o.total, b.version, o.stop),
        None => println!("例を 1 つも通せませんでした（止まった理由: {}・{secs:.1} 秒）", o.stop),
    }
    if let Some(b) = &o.best {
        println!("code: {}（要承認: ホストではまだ実行していません）", o.dir.join(format!("v{}/solve.js", b.version)).display());
    }
    for f in &o.remaining {
        println!("残った誤り: {f}");
    }
    println!("履歴: {}（{} 版）", o.dir.join("history.json").display(), o.history.len());
}
