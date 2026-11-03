//! Apple Music（Music.app）と YouTube の口（docs/ACCEPTANCE.md test3 の土台）。
//! `apple_music` は Music.app の library の列挙・再生・今の曲を osascript（JXA）で、`youtube` は Data API v3 の検索と
//! 本人の browser への handoff（`open -b`）。曲の音声には触らない（DRM の曲も再生は Music.app、YouTube の player は Kangae に埋め込まない）。
//! 分岐は docs/diagrams.md §12。CLI は src/bin/kangae-media.rs（`#[path]` でこの mod を取り込むので main.rs と独立に compile できる）。

pub mod apple_music;
pub mod youtube;

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// osascript が答えを返さなかった時の終わり方。
#[derive(Debug, Clone, PartialEq)]
pub enum Osa {
    /// deadline を過ぎたので kill した（相手が受け取ったかは分からない）
    Deadline,
    /// 失敗で終わった（stderr の原文。起こせなかった時はその理由）
    Failed(String),
}

/// osascript の実行 file。`KANGAE_OSASCRIPT_BIN` で差し替え可（test は偽物を置く）。
pub fn osascript_bin() -> PathBuf {
    std::env::var_os("KANGAE_OSASCRIPT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin/osascript"))
}

/// `osascript -l JavaScript -e <script> -- <args>` を走らせて stdout を返す。args は script に埋め込まず argv で渡す。
/// stdout / stderr は thread で drain する（曲の多い library の JSON は pipe の 64KB を超える）。deadline を過ぎたら kill する。
pub fn jxa(script: &str, args: &[&str], timeout: Duration) -> Result<String, Osa> {
    let bin = osascript_bin();
    let mut child = Command::new(&bin)
        .args(["-l", "JavaScript", "-e", script, "--"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Osa::Failed(format!("Can't start {} (osascript on macOS is needed): {e}", bin.display())))?;
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            b
        })
    };
    let out = drain(Box::new(child.stdout.take().ok_or(Osa::Failed("Can't read the output of osascript".into()))?));
    let err = drain(Box::new(child.stderr.take().ok_or(Osa::Failed("Can't read the output of osascript".into()))?));
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| Osa::Failed(format!("Can't wait for osascript: {e}")))? {
            break s;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Osa::Deadline);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(Osa::Failed(stderr));
    }
    Ok(stdout)
}

/// 空でない最初の行（300 文字まで）
pub fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect()
}
