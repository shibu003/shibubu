//! Google（Gmail / Calendar）接続の土台。OAuth の token は macOS Keychain、HTTP は ureq。
//! この crate には lib target が無いので、bin（src/bin/kangae-google.rs）と tests/google.rs は
//! `#[path]` でこの file を取り込む。file 名が mod.rs なので子 module はこの directory から引かれる。
//! sha256 / base64 / 乱数は `crate::crypto`、書き込みの結果は `crate::domain::run::Outcome`（取り込む側の crate の根に両方を置く）。
//! 出典（API の事実・2026-10-16 取得）: https://developers.google.com/gmail/api/reference/rest
//!   https://developers.google.com/calendar/api/v3/reference
#![allow(dead_code)]
pub mod calendar;
pub mod contacts;
pub mod gmail;
pub mod oauth;
pub mod outreach;

#[allow(unused_imports)] pub use crate::crypto::base64; // google::base64 の path を呼ぶ側（wt/adapters の tests）がまだ在る
use crate::domain::{permission::{ConsentDecision, ConsentReceipt}, run::{ErrorCode, Outcome}};
use serde_json::Value;
use std::{io::Read, process::Command, time::{Duration, SystemTime, UNIX_EPOCH}};

pub const GMAIL_BASE: &str = "https://gmail.googleapis.com";
pub const CALENDAR_BASE: &str = "https://www.googleapis.com";

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).redirects(0).build()
}
pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// access token を持つ client。base は試験で 127.0.0.1 の偽 server に差し替える
pub struct Client { pub access_token: String, pub gmail_base: String, pub calendar_base: String }
impl Client {
    pub fn new(access_token: String) -> Self { Self { access_token, gmail_base: GMAIL_BASE.into(), calendar_base: CALENDAR_BASE.into() } }
    pub fn get(&self, url: &str) -> Result<Value, String> {
        finish(agent().get(url).set("Authorization", &self.bearer()).set("Accept", "application/json").call())
    }
    pub fn post(&self, url: &str, body: &Value) -> Result<Value, String> {
        finish(agent().post(url).set("Authorization", &self.bearer()).set("Accept", "application/json").send_json(body))
    }
    /// 書き込みと読み戻しの口。失敗を Fail（status と「届いたか」）で返す。get / post（文言だけ返す）は今の呼び出し元のために残す
    pub fn call(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Value, Fail> {
        let req = agent().request(method, url).set("Authorization", &self.bearer()).set("Accept", "application/json");
        let resp = match body { Some(b) => req.send_json(b), None => req.call() };
        match resp {
            // agent は redirects(0) で、ureq 2.12 は 3xx を Ok で返す（Error::Status は 400 以上だけ）。転送は受理ではない
            Ok(r) if !(200..300).contains(&r.status()) => { let code = r.status(); Err(Fail { status: Some(code), delivered: true, message: status_message(code, r) }) }
            Ok(r) => {
                let status = r.status();
                let mut bytes = Vec::new();
                let read = r.into_reader().take(4 * 1024 * 1024).read_to_end(&mut bytes);
                // DELETE の 204 は本文が空
                if read.is_ok() && bytes.iter().all(u8::is_ascii_whitespace) { return Ok(Value::Null) }
                read.ok().and_then(|_| serde_json::from_slice(&bytes).ok())
                    .ok_or(Fail { status: Some(status), delivered: true, message: "Google's answer was in an unexpected form (a proxy or a temporary fault, perhaps)".to_string() })
            }
            Err(ureq::Error::Status(code, r)) => Err(Fail { status: Some(code), delivered: true, message: status_message(code, r) }),
            Err(ureq::Error::Transport(t)) => {
                use ureq::ErrorKind::*;
                // 接続の前に落ちた物だけ「届いていない」。それ以外（送った後の Io・timeout）は届いたかもしれない
                let unsent = matches!(t.kind(), InvalidUrl | UnknownScheme | Dns | InsecureRequestHttpsOnly | ConnectionFailed | InvalidProxyUrl | ProxyConnect | ProxyUnauthorized);
                Err(Fail { status: None, delivered: !unsent, message: format!("Google に接続できません（{t}）。ネットワークを確認してもう一度試してください。") })
            }
        }
    }
    fn bearer(&self) -> String { format!("Bearer {}", self.access_token) }
}

/// call の失敗。status = HTTP の code（届かなかった時は None）、delivered = request が Google に届いたかもしれない
#[derive(Debug, Clone, PartialEq)]
pub struct Fail { pub status: Option<u16>, pub delivered: bool, pub message: String }
impl Fail {
    /// 2xx だが本文が読めない = 受理はされた
    pub fn accepted(&self) -> bool { self.status.is_some_and(|s| (200..300).contains(&s)) }
    /// 書き込みが作られたか分からない: 届いた後に切れた（timeout を含む）か 5xx
    pub fn unknown(&self) -> bool { (self.status.is_none() && self.delivered) || self.status.is_some_and(|s| s >= 500) }
    pub fn code(&self) -> ErrorCode {
        let m = |p: &str| self.message.starts_with(p);
        match self.status {
            None => ErrorCode::ConnectionLost,
            Some(403) if m("API が有効化されていません") => ErrorCode::NotReady,
            Some(403) if m("Google の利用上限") => ErrorCode::RateLimited,
            Some(401 | 403) => ErrorCode::PermissionDenied,
            Some(429) => ErrorCode::RateLimited,
            Some(404 | 410) => ErrorCode::TargetNotFound,
            Some(400 | 409) => ErrorCode::InvalidInput,
            Some(500..=599) => ErrorCode::ConnectionLost,
            _ => ErrorCode::Internal,
        }
    }
}

/// 書き込み 1 回の結果（Receipt の材料。run / step の id は runtime が付ける）。
/// id = 作った物（Calendar は自分で決めた event id・Gmail は draft id。作られたか分からない時は None）、detail = 人が読む理由と次の一手
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Written {
    pub id: Option<String>,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}
impl Written {
    pub fn new(id: Option<&str>, outcome: Outcome) -> Self { Self { id: id.map(str::to_string), outcome, detail: String::new() } }
    pub fn failed(id: Option<&str>, error: ErrorCode, detail: impl Into<String>) -> Self { Self { detail: detail.into(), ..Self::new(id, Outcome::Failed { error }) } }
    pub fn verified(&self) -> bool { matches!(self.outcome, Outcome::Verified { .. }) }
    /// 人が読む理由（detail か、outcome の reason）
    pub fn why(&self) -> String {
        match &self.outcome {
            Outcome::Unverified { reason } | Outcome::Rejected { reason } | Outcome::EffectUnknown { reason } if self.detail.is_empty() => reason.clone(),
            _ => self.detail.clone(),
        }
    }
    /// 書き込みの失敗 → 作られたか分からない物は EffectUnknown（自動で再送しない。recheck = 同じ物が在るかを確かめる command）
    pub fn from_fail(id: Option<&str>, f: &Fail, recheck: &str) -> Self {
        if f.unknown() { Self::new(id, Outcome::EffectUnknown { reason: format!("Can't tell whether it was created: {}. Before creating it again, check with `{recheck}` whether the same one exists", f.message) }) }
        else { Self::failed(id, f.code(), f.message.clone()) }
    }
    /// 受理の後の読み戻し: 見つからない・読めないは Unverified（受理だけでは Verified にしない: RUN-004）
    pub fn after_accept(self) -> Self {
        match self.outcome {
            Outcome::Failed { .. } | Outcome::EffectUnknown { .. } => {
                Self::new(self.id.as_deref(), Outcome::Unverified { reason: format!("Google accepted it, but it can't be confirmed by reading it back ({})", self.why()) })
            }
            _ => self,
        }
    }
}

/// 見せた本文の digest（承認が束ねる物）。serde_json の Map は key が sort 済みで to_string は空白を入れない = 正準形（runtime の payload_hash と同じ前提）
pub fn digest(body: &Value) -> String { crate::crypto::sha256_hex(body.to_string().as_bytes()) }
/// 承認が「この本文」への Approved か（DESIGN §5.3: 見せていない物の実行許可にはならない）
pub fn approved(consent: Option<&ConsentReceipt>, body: &Value) -> bool {
    consent.is_some_and(|c| c.decision == ConsentDecision::Approved && c.payload_digest.as_deref() == Some(digest(body).as_str()))
}
fn finish(r: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    match r {
        // 3xx（redirects(0) なので Ok で来る）を受理として読まない
        Ok(resp) if !(200..300).contains(&resp.status()) => { let code = resp.status(); Err(status_message(code, resp)) }
        Ok(resp) => read_json(resp),
        Err(ureq::Error::Status(code, resp)) => Err(status_message(code, resp)),
        Err(ureq::Error::Transport(t)) => Err(format!("Google に接続できません（{t}）。ネットワークを確認してもう一度試してください。")),
    }
}
/// Google の error body は {"error":{"code","message","status"}}。文言は人が次の一手を取れる形にする
fn status_message(code: u16, resp: ureq::Response) -> String {
    let body = read_json(resp).unwrap_or(Value::Null);
    let detail = body["error"]["message"].as_str().or(body["error"].as_str()).unwrap_or("").to_string();
    let d = detail.to_lowercase();
    match code {
        401 => "Google の認証が切れています。`kangae-google auth` で再認証してください。".to_string(),
        403 if d.contains("insufficient") || d.contains("scope") => format!("権限（scope）が足りません（{detail}）。`kangae-google auth` で再認証してください。"),
        403 if d.contains("disabled") || d.contains("not been used") || d.contains("enable") => format!("API が有効化されていません（{detail}）。Google Cloud Console で Gmail API / Google Calendar API を有効にしてください。"),
        403 | 429 if d.contains("limit") || d.contains("quota") => format!("Google の利用上限に達しました（{detail}）。少し待ってもう一度試してください。"),
        403 => format!("Google がアクセスを拒否しました（{detail}）。"),
        404 => format!("見つかりません（{detail}）。"),
        500..=599 => format!("Google 側で障害が起きています（HTTP {code}）。少し待ってもう一度試してください。"),
        _ => format!("Google が想定外の応答を返しました（HTTP {code} {detail}）。"),
    }
}
pub fn read_json(resp: ureq::Response) -> Result<Value, String> {
    const CAP: usize = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    resp.into_reader().take(CAP as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("Couldn't read Google's answer ({e})"))?;
    if bytes.len() > CAP { return Err("Google's answer is too large".to_string()) }
    serde_json::from_slice(&bytes).map_err(|_| "Google's answer was in an unexpected form (a proxy or a temporary fault, perhaps)".to_string())
}

/// query と form の percent encoding（unreserved 以外を全部 %XX、空白は %20）
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
    }
    out
}
pub fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = if b[i] == b'%' && i + 3 <= b.len() { std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) } else { None };
        match (b[i], hex) {
            (_, Some(v)) => { out.push(v); i += 3 }
            (b'+', None) => { out.push(b' '); i += 1 }
            (c, None) => { out.push(c); i += 1 }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// unix 秒 → "YYYY-MM-DDTHH:MM:SSZ"（Calendar の timeMin / timeMax）。Howard Hinnant の civil_from_days
pub fn rfc3339_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
/// RFC3339（offset 必須・小数秒可）→ unix 秒。Google は dateTime を calendar の tz に直して返すので、読み戻しは文字列でなく瞬間で比べる
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if !s.is_ascii() || b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') || b[13] != b':' || b[16] != b':' { return None }
    let n = |r: std::ops::Range<usize>| { let t = &s[r]; t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse::<i64>().ok()).flatten() };
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 { return None }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') { rest = r.trim_start_matches(|c: char| c.is_ascii_digit()) }
    let off = match rest.as_bytes() {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => (if *sign == b'+' { 1 } else { -1 }) * (rest[1..3].parse::<i64>().ok()? * 3600 + rest[4..6].parse::<i64>().ok()? * 60),
        _ => return None,
    };
    // days_from_civil（rfc3339_utc の逆）
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    Some((era * 146_097 + doe - 719_468) * 86400 + hh * 3600 + mm * 60 + ss - off)
}
/// local の UTC offset（秒）。macOS の `date +%z`（例 +0900）。読めなければ None
pub fn local_offset() -> Option<i64> {
    let out = Command::new("/bin/date").arg("+%z").output().ok()?;
    let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (sign, hhmm) = (if z.starts_with('-') { -1 } else { 1 }, z.get(1..5)?);
    Some(sign * (hhmm[..2].parse::<i64>().ok()? * 3600 + hhmm[2..].parse::<i64>().ok()? * 60))
}
