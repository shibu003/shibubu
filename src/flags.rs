//! 層⑦「すぐ気づいて、すぐ戻す」の feature flag（docs/REVIEW_PROCESS.md §2）。
//! deploy 無しで切る: binary を作り直さず、env か file を変えて再起動すれば効く。
//!
//! 判定の順（上が勝つ）:
//! 1. env `KANGAE_FLAGS`（例 `new_composer=on,agent_rail=off`。値は on/off・true/false・1/0、大文字小文字と前後の空白は問わない）
//! 2. file `~/.kangae/flags.json`（例 `{"new_composer": true}`。値は true / false だけ）
//! 3. 呼び手が `enabled(name, default)` に渡す既定値（どこにも書いていない flag）
//!
//! 壊れた file（JSON でない・object でない・読めない）は丸ごと無視して warn。bool でない値と読めない env の項目は、
//! その項目だけ無視して warn。file が無いのは普通の状態なので warn しない。warn は stderr に出し、`warnings` にも残す（画面に出す用）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct Flags {
    values: HashMap<String, bool>,
    pub warnings: Vec<String>,
}

impl Flags {
    /// env `KANGAE_FLAGS` と `~/.kangae/flags.json` を読む。起動時に 1 回呼ぶ（読み直しは再起動）。
    pub fn load() -> Self {
        let env = std::env::var("KANGAE_FLAGS").ok();
        let file = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".kangae/flags.json"));
        let flags = Self::from_sources(env.as_deref(), file.as_deref());
        for w in &flags.warnings {
            eprintln!("kangae flags: {w}");
        }
        flags
    }

    /// `load` の中身。env と file を引数で受けるので、test は process の env を触らずに済む。
    pub fn from_sources(env: Option<&str>, file: Option<&Path>) -> Self {
        let mut flags = Self::default();
        if let Some(path) = file {
            flags.read_file(path);
        }
        // env を後に読む = 同じ名前は env が file を上書きする
        if let Some(spec) = env {
            flags.read_env(spec);
        }
        flags
    }

    /// `name` が on か。env にも file にも無ければ呼び手の `default`。
    pub fn enabled(&self, name: &str, default: bool) -> bool {
        self.values.get(name).copied().unwrap_or(default)
    }

    fn read_file(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.warnings.push(format!("{} を読めないので無視した: {e}", path.display()));
                return;
            }
        };
        let object = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(object)) => object,
            Ok(_) => {
                self.warnings.push(format!("{} が JSON の object でないので無視した", path.display()));
                return;
            }
            Err(e) => {
                self.warnings.push(format!("{} が壊れた JSON なので無視した: {e}", path.display()));
                return;
            }
        };
        for (name, value) in object {
            match value.as_bool() {
                Some(on) => {
                    self.values.insert(name, on);
                }
                None => self.warnings.push(format!("{} の {name} が true / false でないので無視した: {value}", path.display())),
            }
        }
    }

    fn read_env(&mut self, spec: &str) {
        for item in spec.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            let parsed = item.split_once('=').and_then(|(name, value)| {
                let on = match value.trim().to_ascii_lowercase().as_str() {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => return None,
                };
                Some((name.trim(), on))
            });
            match parsed {
                Some((name, on)) if !name.is_empty() => {
                    self.values.insert(name.to_string(), on);
                }
                _ => self.warnings.push(format!("KANGAE_FLAGS の「{item}」が name=on|off の形でないので無視した")),
            }
        }
    }
}
