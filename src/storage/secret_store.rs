//! secret は DB に入れない（CORE-010）。DB には参照名（key）だけを置き、値は macOS Keychain に置く。
//! Keychain は `/usr/bin/security` を固定引数で包む（crate は足さない）。値は argv に出さず
//! `security -i` の stdin に hex で渡す（ps に見えない・quote の問題が無い）。
//! test は `SecretStore::memory()`（env `KANGAE_SECRET_STORE=memory` でも同じ）。本物の Keychain には書かない。
use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Keychain の service 名。機械全体で 1 つ（上流 app と衝突しない名前）。
pub const KEYCHAIN_SERVICE: &str = "Kangae";
const SECURITY: &str = "/usr/bin/security";
const NOT_FOUND: i32 = 44; // security の errSecItemNotFound
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum SecretError {
    Io(std::io::Error),
    /// `security` が 0 / 44 以外で終わった。stderr に理由。
    Tool { code: Option<i32>, stderr: String },
    /// `security` が 10 秒応答しない（Keychain の GUI 確認で止まっている等）。
    Timeout,
    /// key に使えるのは英数字と `. _ - : / @`、200 文字まで。
    BadKey(String),
    Lock,
}
impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "security を起動できない: {e}"),
            Self::Tool { code, stderr } => write!(f, "security が失敗 (exit {code:?}): {stderr}"),
            Self::Timeout => write!(f, "security が {} 秒応答しない", TIMEOUT.as_secs()),
            Self::BadKey(k) => write!(f, "secret の key が不正: {k:?}"),
            Self::Lock => write!(f, "memory store の lock が壊れている"),
        }
    }
}
impl std::error::Error for SecretError {}
impl From<std::io::Error> for SecretError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub enum SecretStore {
    Keychain { service: String },
    Memory(Mutex<HashMap<String, String>>),
}

impl SecretStore {
    /// `KANGAE_SECRET_STORE=memory` なら in-memory、それ以外は Keychain（service = `KEYCHAIN_SERVICE`）。
    pub fn from_env() -> Self {
        match std::env::var("KANGAE_SECRET_STORE").as_deref() {
            Ok("memory") => Self::memory(),
            _ => Self::keychain(KEYCHAIN_SERVICE),
        }
    }
    pub fn memory() -> Self {
        Self::Memory(Mutex::new(HashMap::new()))
    }
    pub fn keychain(service: &str) -> Self {
        Self::Keychain { service: service.into() }
    }
    pub fn is_memory(&self) -> bool {
        matches!(self, Self::Memory(_))
    }

    /// 無ければ作り、あれば置き換える。
    pub fn put(&self, key: &str, value: &str) -> Result<(), SecretError> {
        check_key(key)?;
        match self {
            Self::Memory(m) => {
                m.lock().map_err(|_| SecretError::Lock)?.insert(key.into(), value.into());
                Ok(())
            }
            Self::Keychain { service } => {
                check_key(service)?;
                let line = format!(
                    "add-generic-password -U -a {key} -s {service} -X {}\n",
                    hex(value.as_bytes())
                );
                let out = security(&["-i"], Some(&line))?;
                if out.code == Some(0) {
                    Ok(())
                } else {
                    Err(out.into_error())
                }
            }
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        check_key(key)?;
        match self {
            Self::Memory(m) => Ok(m.lock().map_err(|_| SecretError::Lock)?.get(key).cloned()),
            Self::Keychain { service } => {
                check_key(service)?;
                let out = security(&["find-generic-password", "-a", key, "-s", service, "-w"], None)?;
                match out.code {
                    Some(0) => Ok(Some(out.stdout.strip_suffix('\n').unwrap_or(&out.stdout).to_string())),
                    Some(NOT_FOUND) => Ok(None),
                    _ => Err(out.into_error()),
                }
            }
        }
    }

    /// 消したら true、無ければ false。
    pub fn delete(&self, key: &str) -> Result<bool, SecretError> {
        check_key(key)?;
        match self {
            Self::Memory(m) => Ok(m.lock().map_err(|_| SecretError::Lock)?.remove(key).is_some()),
            Self::Keychain { service } => {
                check_key(service)?;
                let out = security(&["delete-generic-password", "-a", key, "-s", service], None)?;
                match out.code {
                    Some(0) => Ok(true),
                    Some(NOT_FOUND) => Ok(false),
                    _ => Err(out.into_error()),
                }
            }
        }
    }
}

/// RQ2（RQ-USAGE）: 他の app（Claude Code）が置いた項目を service 名だけで読む（読むだけ・書かない・消さない）。None = 無い。
/// service は argv の 1 要素（shell を通さない）なので `check_key` を当てない（`Claude Code-credentials` は空白を含む）。10 秒で打ち切る。
/// 例: `secret_store::read_generic_password("Claude Code-credentials")?`
pub fn read_generic_password(service: &str) -> Result<Option<String>, SecretError> {
    if service.is_empty() || service.chars().any(char::is_control) {
        return Err(SecretError::BadKey(service.into()));
    }
    let out = security(&["find-generic-password", "-s", service, "-w"], None)?;
    match out.code {
        Some(0) => Ok(Some(out.stdout.strip_suffix('\n').unwrap_or(&out.stdout).to_string())),
        Some(NOT_FOUND) => Ok(None),
        _ => Err(out.into_error()),
    }
}

/// RQ4（RQ-015・RQ-016・`ConnAct::PasteSecret`）: Connections の行の秘密の Keychain の key。`row` は `ui::connections::Connection::key`（`github`・`web`・`mcp:<server>`）、
/// `name` は秘密の名前（`token`・MCP の env の名前）。`check_key` で使えない文字は `-` に替え、200 文字で切る。
/// 例: `keys.put(&secret_store::connection_key("mcp:penpot", "PENPOT_TOKEN"), &value)?` → key `conn.mcp:penpot.PENPOT_TOKEN`。
/// 保証する: 同じ (row, name) は同じ key・返した key は `check_key` を通る。保証しない: 違う (row, name) の key が重ならない事（使えない文字を `-` にした結果が同じなら重なる・
/// MCP の server の名前は `McpConfig::add_from_text` が英数字と `-` にしてから足す）。
/// Wave: RQ4
pub fn connection_key(row: &str, name: &str) -> String {
    let raw = format!("conn.{row}.{name}");
    raw.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@') { c } else { '-' }).take(200).collect()
}

/// key と service は `security` の引数になる。空白・引用符・制御文字を通さない。
fn check_key(key: &str) -> Result<(), SecretError> {
    let ok = !key.is_empty()
        && key.len() <= 200
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@'));
    if ok {
        Ok(())
    } else {
        Err(SecretError::BadKey(key.into()))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}
impl Output {
    fn into_error(self) -> SecretError {
        SecretError::Tool { code: self.code, stderr: self.stderr.trim().to_string() }
    }
}

/// `security` を起動し、stdin を流し、10 秒で kill。stdout / stderr は別 thread で吸う（pipe 詰まり防止）。
fn security(args: &[&str], stdin: Option<&str>) -> Result<Output, SecretError> {
    let mut child = Command::new(SECURITY)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())?; // drop で close → `-i` は EOF で終わる
    }
    let drain = |mut r: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            b
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("stdout は piped")));
    let err = drain(Box::new(child.stderr.take().expect("stderr は piped")));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SecretError::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    Ok(Output { code: status.code(), stdout, stderr })
}
