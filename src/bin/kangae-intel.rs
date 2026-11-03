//! Software Intelligence の試し口（v0.13 W4-5・docs/SOFTWARE_INTELLIGENCE_AND_LEARNING.md §2 を読むだけで）。
//!   kangae-intel <app の名前 or bundle id> [--json] [--timeout S]
//! 入っているか・試せる事（read / 外に効く）・未確認の事・要る権限を、読んだ所と時刻つきで返す。起動しない・command を打たない。
//! src/main.rs とは独立に compile する（intel mod を #[path] で取り込む）。分岐は docs/diagrams.md §15。

#[path = "../intel/mod.rs"]
mod intel;

use intel::{Capability, GateClass, Intel, Probe};
use std::time::Duration;

const USAGE: &str = "使い方:
  kangae-intel <app の名前 or bundle id> [--json] [--timeout S]
    例: kangae-intel REAPER / kangae-intel com.apple.Music / kangae-intel \"Google Chrome\"
    入っているか・試せる事（read / 外に効く）・未確認の事・要る権限を、読んだ所と時刻つきで返す
    起動しない・command を打たない（読むだけ）。--json は data のまま。--timeout は外の command 1 回ごとの上限（既定 20 秒）";

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(args: Vec<String>) -> i32 {
    let (mut json, mut timeout, mut query) = (false, 20u64, None::<String>);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--timeout" => match it.next().and_then(|s| s.parse().ok()).filter(|s| *s > 0) {
                Some(s) => timeout = s,
                None => {
                    eprintln!("--timeout には 1 以上の秒数を渡してください\n{USAGE}");
                    return 2;
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            _ if a.starts_with("--") => {
                eprintln!("知らない option です: {a}\n{USAGE}");
                return 2;
            }
            _ if query.is_none() => query = Some(a),
            _ => {
                eprintln!("app は 1 つだけ渡してください（名前に空白があれば \"\" で囲む）\n{USAGE}");
                return 2;
            }
        }
    }
    let Some(q) = query else {
        eprintln!("{USAGE}");
        return 2;
    };
    match intel::investigate(&q, Duration::from_secs(timeout)) {
        Ok(r) if json => {
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            0
        }
        Ok(r) => {
            print!("{}", render(&r));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn absent<T>(p: &Probe<T>) -> Option<(&str, &intel::Source)> {
    match p {
        Probe::Absent { why, source } => Some((why, source)),
        _ => None,
    }
}

fn tag(g: GateClass) -> &'static str {
    match g {
        GateClass::Read => "read",
        GateClass::Outward => "外に効く",
    }
}

/// 人向け: 入っているか → 試せる事 → 未確認の事 → できない事 → 要る権限。各行に読んだ所と時刻。
fn render(r: &Intel) -> String {
    let mut o = Vec::new();
    match &r.app {
        Probe::Found { value: a, source } => {
            let version = a.version.as_deref().map_or("版 不明".to_string(), |v| format!("版 {v}"));
            o.push(format!("{} — 入っている: {}（{}・{version}）", a.name, a.path, a.bundle_id.as_deref().unwrap_or("bundle id なし")));
            o.push(format!("  読んだ所: {}（{}）", source.read, source.checked_at));
            if let Some(pid) = a.pid {
                o.push(format!("  起動中（pid {pid}）"));
            }
            o.extend(a.other_copies.iter().map(|c| format!("  他の copy: {c}")));
        }
        Probe::Absent { why, source } => {
            o.push(format!("{} — {why}", r.query));
            o.push(format!("  読んだ所: {}（{}）", source.read, source.checked_at));
        }
        Probe::Unchecked { why } => o.push(format!("{} — 入っているか未確認: {why}", r.query)),
    }
    // 確かめて無かった事（未確認ではない）
    for (why, source) in [absent(&r.dictionary), absent(&r.windows)].into_iter().flatten() {
        o.push(format!("  {why}（読んだ所: {}）", source.read));
    }
    let line = |c: &Capability| {
        let requires = if c.requires.is_empty() { String::new() } else { format!("・要る: {}", c.requires.join(", ")) };
        let route = c.endpoint.as_deref().map_or(c.route.clone(), |e| format!("{} {e}", c.route));
        format!("  [{}] {} — {}（{route}・{}）\n      読んだ所: {}（{}）{requires}", tag(c.gate), c.id, c.summary, c.evidence, c.source.read, c.source.checked_at)
    };
    let (script, others): (Vec<&Capability>, Vec<&Capability>) = r.capabilities.iter().partition(|c| c.id.starts_with("applescript."));
    o.push("試せる事:".into());
    let available: Vec<&&Capability> = others.iter().filter(|c| c.status == "available").collect();
    o.extend(available.iter().map(|c| line(c)));
    if let (Probe::Found { source, .. }, Some(first)) = (&r.dictionary, script.first()) {
        for g in [GateClass::Read, GateClass::Outward] {
            let names: Vec<&str> = script.iter().filter(|c| c.gate == g).map(|c| c.summary.split(" — ").next().unwrap_or("")).collect();
            if !names.is_empty() {
                o.push(format!("  [{}] AppleScript の command {} 個: {}", tag(g), names.len(), names.join(", ")));
            }
        }
        o.push(format!(
            "      読んだ所: {}（{}）・要る: {}・辞書は作用を書かないので count / exists / get 以外は外に効く扱い",
            source.read,
            source.checked_at,
            first.requires.join(", ")
        ));
    }
    if available.is_empty() && script.is_empty() {
        o.push("  （無し）".into());
    }
    o.push("未確認の事:".into());
    let before = o.len();
    for c in others.iter().filter(|c| c.status == "needs_check") {
        o.push(format!("  - {}（{}）: {}", c.id, c.route, c.reason.as_deref().unwrap_or("未確認")));
    }
    o.extend(r.unknowns.iter().map(|u| format!("  - {}: {}", u.what, u.why)));
    if o.len() == before {
        o.push("  （無し）".into());
    }
    let excluded: Vec<String> =
        others.iter().filter(|c| c.status == "excluded").map(|c| format!("  - {}（{}）: {}", c.id, c.route, c.reason.as_deref().unwrap_or(""))).collect();
    if !excluded.is_empty() {
        o.push("できない事:".into());
        o.extend(excluded);
    }
    o.push("要る権限:".into());
    if r.permissions.is_empty() {
        o.push("  （無し）".into());
    }
    for p in &r.permissions {
        o.push(format!("  - {} — {} に許す・{}", p.label, p.who, p.settings));
        // 辞書の command は 1 つずつ並べず数だけ
        let script = p.because.iter().filter(|b| b.starts_with("applescript.")).count();
        let mut because: Vec<String> = p.because.iter().filter(|b| !b.starts_with("applescript.")).cloned().collect();
        if script > 0 {
            because.push(format!("辞書の command {script} 個"));
        }
        o.push(format!("      根拠: {}", because.join("・")));
    }
    o.push(format!("（{} に読んだ。こちらからは起動していない・command は打っていない）", r.checked_at));
    o.join("\n") + "\n"
}
