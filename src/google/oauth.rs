//! OAuth 2.0 installed-app（loopback redirect）と token の保管（macOS Keychain）。
//! 出典（2026-10-16 取得）: https://developers.google.com/identity/protocols/oauth2/native-app
//!   redirect は http://127.0.0.1:<空き port>、PKCE は S256（RFC 7636 §4.2「S256 を扱える client は S256 を使わなければならない」・CD-31）、
//!   token endpoint は https://oauth2.googleapis.com/token、Desktop app の client_secret は「秘密にできない」扱い（それでも送る）。
//!   refresh token の失効: https://developers.google.com/identity/protocols/oauth2#expiration（Testing 状態は 7 日）
use super::{agent, now, read_json, urldecode, urlencode};
use crate::crypto::{base64, random, sha256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{env, io::{Read, Write}, net::{TcpListener, TcpStream}, process::{Command, Stdio}, time::{Duration, Instant}};

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// 要求する scope。gmail.compose は下書きと送信（messages.send）の両方を許すので gmail.send は別に要求しない（送るのは承認の束を確かめた後の
/// outreach::send_plan だけ）。calendar.readonly は calendar.events が events.list を許すので要求しない
/// 出典: https://developers.google.com/gmail/api/auth/scopes / https://developers.google.com/calendar/api/auth
pub const SCOPES: [&str; 3] = [
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.compose",
    "https://www.googleapis.com/auth/calendar.events",
];
pub const KEYCHAIN_SERVICE: &str = "kangae-google";
pub const KEYCHAIN_ACCOUNT: &str = "oauth";

pub struct Config { pub client_id: String, pub client_secret: String }
impl Config {
    /// env から。値は file に書かない（手順: docs/internal/connectors/google.md）
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| env::var(k).ok().filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("The environment variable {k} isn't set. Export the values of an OAuth client (Desktop app) from the Google Cloud Console (steps: docs/internal/connectors/google.md)"));
        Ok(Self { client_id: get("KANGAE_GOOGLE_CLIENT_ID")?, client_secret: get("KANGAE_GOOGLE_CLIENT_SECRET")? })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tokens { pub access_token: String, pub refresh_token: String, pub expires_at: u64, #[serde(default)] pub scope: String }
impl Tokens {
    /// 60 秒の余裕を見て切れ扱い
    pub fn expired(&self, now: u64) -> bool { now + 60 >= self.expires_at }
}

pub fn auth_url(cfg: &Config, redirect_uri: &str, verifier: &str, state: &str) -> String {
    // access_type=offline と prompt=consent が無いと 2 回目以降の同意で refresh_token が返らない
    format!("{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        urlencode(&cfg.client_id), urlencode(redirect_uri), urlencode(&SCOPES.join(" ")), pkce_challenge(verifier), urlencode(state))
}
/// RFC 7636 §4.2: code_challenge = BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))（padding なし）。verifier 自体は token の交換でだけ送る
/// 出典（2026-10-17 照合）: https://www.rfc-editor.org/rfc/rfc7636.txt §4.2・Appendix B
pub fn pkce_challenge(verifier: &str) -> String { base64(&sha256(verifier.as_bytes()), true).trim_end_matches('=').to_string() }

/// ブラウザで同意 → loopback で code を受ける → token。Keychain には書かない（呼ぶ側が keychain_save）
pub fn authorize(cfg: &Config) -> Result<Tokens, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Can't open a local port ({e})"))?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr().map_err(|e| e.to_string())?.port());
    let (verifier, state) = (random_token()?, random_token()?);
    let url = auth_url(cfg, &redirect_uri, &verifier, &state);
    eprintln!("Opening Google's consent screen in your browser. If it doesn't open, open this URL:\n{url}\n(Get to “Allow” within 5 minutes)");
    let _ = Command::new("open").arg(&url).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let code = wait_for_code(&listener, &state, Duration::from_secs(300))?;
    exchange_code(cfg, TOKEN_URL, &code, &redirect_uri, &verifier)
}

/// 32 byte の乱数を base64url（= 43 文字。code_verifier の文字種と長さ 43〜128 の条件を満たす: RFC 7636 §4.1）
fn random_token() -> Result<String, String> {
    Ok(base64(&random(32)?, true).trim_end_matches('=').to_string())
}

/// loopback で redirect を 1 回受ける。favicon 等の無関係な request は 404 で流し、error= と state 不一致は Err。timeout で諦める（hang しない）
pub fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        let (mut stream, _) = match listener.accept() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline { return Err(format!("Waited {} s but no answer came from Google. Run `kangae-google auth` again", timeout.as_secs())) }
                std::thread::sleep(Duration::from_millis(100));
                continue
            }
            Err(e) => return Err(format!("Can't receive the redirect ({e})")),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        let target = head.split_whitespace().nth(1).unwrap_or("");
        let params: Vec<(String, String)> = target.split_once('?').map(|(_, q)| q).unwrap_or("").split('&').filter(|p| !p.is_empty())
            .map(|p| { let (k, v) = p.split_once('=').unwrap_or((p, "")); (urldecode(k), urldecode(v)) }).collect();
        let get = |k: &str| params.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        if let Some(err) = get("error") {
            respond(&mut stream, 400, &format!("Google refused the authorization: {err}"));
            return Err(format!("Google refused the authorization ({err}). Check that you pressed Allow on the consent screen, and that this Google account is a test user"))
        }
        let Some(code) = get("code") else { respond(&mut stream, 404, "not found"); continue };
        if get("state") != Some(state) {
            respond(&mut stream, 400, "The state doesn't match");
            return Err("The state doesn't match (a different authorization answer arrived). Run `kangae-google auth` again".to_string())
        }
        respond(&mut stream, 200, "Google sign-in for Shibubu is done. You can close this tab");
        return Ok(code.to_string())
    }
}
fn respond(stream: &mut TcpStream, status: u16, msg: &str) {
    let text = msg.replace('&', "&amp;").replace('<', "&lt;");
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>Shibubu</title><p>{text}</p>");
    let reason = match status { 200 => "OK", 404 => "Not Found", _ => "Bad Request" };
    let _ = stream.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    let _ = stream.flush();
}

/// 認可 code → token（token_url は試験で差し替える）
pub fn exchange_code(cfg: &Config, token_url: &str, code: &str, redirect_uri: &str, verifier: &str) -> Result<Tokens, String> {
    let v = token_request(token_url, &[("code", code), ("client_id", cfg.client_id.as_str()), ("client_secret", cfg.client_secret.as_str()),
        ("redirect_uri", redirect_uri), ("grant_type", "authorization_code"), ("code_verifier", verifier)])?;
    let refresh = v["refresh_token"].as_str()
        .ok_or("Google didn't return a refresh token. Remove Kangae's access at https://myaccount.google.com/permissions, then run `kangae-google auth` again")?.to_string();
    tokens_from(&v, refresh)
}
pub fn refresh(cfg: &Config, token_url: &str, old: &Tokens) -> Result<Tokens, String> {
    let v = token_request(token_url, &[("refresh_token", old.refresh_token.as_str()), ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()), ("grant_type", "refresh_token")])?;
    // refresh の応答には通常 refresh_token が無い → 旧を持ち続ける
    let refresh = v["refresh_token"].as_str().unwrap_or(&old.refresh_token).to_string();
    tokens_from(&v, refresh)
}
/// 切れていれば refresh。戻りの bool は「Keychain に書き直す」= refresh token が変わった時だけ。access token（1 時間で切れる）は memory に持つだけで、
/// 次の command はもう一度 refresh する（POST 1 本）。毎回書くと、Keychain の書きが返らない Mac では毎回 KEYCHAIN_TIMEOUT 待つ（2026-10-17 実測・keychain_save の注）
pub fn refresh_if_needed(cfg: &Config, token_url: &str, tokens: Tokens) -> Result<(Tokens, bool), String> {
    if !tokens.expired(now()) { return Ok((tokens, false)) }
    let t = refresh(cfg, token_url, &tokens)?;
    let changed = t.refresh_token != tokens.refresh_token;
    Ok((t, changed))
}
fn tokens_from(v: &Value, refresh_token: String) -> Result<Tokens, String> {
    let access_token = v["access_token"].as_str().ok_or("Google's token answer has no access token")?.to_string();
    Ok(Tokens { access_token, refresh_token, expires_at: now() + v["expires_in"].as_u64().unwrap_or(3600), scope: v["scope"].as_str().unwrap_or("").to_string() })
}
fn token_request(url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let body = form.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect::<Vec<_>>().join("&");
    match agent().post(url).set("Content-Type", "application/x-www-form-urlencoded").send_string(&body) {
        Ok(resp) => read_json(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let v = read_json(resp).unwrap_or(Value::Null);
            let (err, desc) = (v["error"].as_str().unwrap_or("").to_string(), v["error_description"].as_str().unwrap_or("").to_string());
            Err(if err == "invalid_grant" {
                format!("The token isn't valid ({desc}). The app is in Testing, where access expires 7 days after consent, or access was revoked. Sign in again with `kangae-google auth`")
            } else {
                format!("Can't get a token (HTTP {code} {err} {desc}). Check the client ID and secret, and the OAuth client (Desktop app) in the Google Cloud Console")
            })
        }
        Err(ureq::Error::Transport(t)) => Err(format!("Google に接続できません（{t}）。ネットワークを確認してもう一度試してください。")),
    }
}

/// Keychain へ保存。secret を argv に出さないため security の対話 mode に stdin で渡す（-X は hex なので quote も要らない）
pub fn keychain_save(t: &Tokens) -> Result<(), String> {
    let json = serde_json::to_string(t).map_err(|e| e.to_string())?;
    let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
    let line = format!("add-generic-password -a {KEYCHAIN_ACCOUNT} -s {KEYCHAIN_SERVICE} -X {hex} -U\n");
    let spawn = |e: std::io::Error| format!("Can't start the security command ({e})");
    // ponytail: security -i の 1 行は約 4 KB まで（実測: 15.7 KB の hex は 3 つの command に割れた）。超える token だけ argv に逃がす（ps に一瞬見える）
    let mut child = if line.len() <= 3500 {
        let mut child = Command::new("security").args(["-i", "-q"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(spawn)?;
        child.stdin.take().ok_or("Can't open the input of security")?.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        child
    } else {
        Command::new("security").args(["add-generic-password", "-a", KEYCHAIN_ACCOUNT, "-s", KEYCHAIN_SERVICE, "-X", &hex, "-U"])
            .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(spawn)?
    };
    // security が返らない事がある（2026-10-17 実測: 書きは新しい項目でも security の中の flock（Security::LocalFileLocker::lock）で待ったまま。
    // login keychain の lock file を握っていたのは networkserviceproxy で、login.keychain-db の safe-save の一時 file（0 byte）を書きかけのまま 4 時間。
    // 確認の画面は出ていない（cua の list_windows に SecurityAgent の窓なし）。読み（find-generic-password）は 0.7 秒で返る。
    // operator が自分の user の trustd --agent に TERM を送った後、一時 file が消えて add / delete とも 1 秒未満に戻った → 書きかけの側が trustd を待っていた候補）
    // → 待つのは KEYCHAIN_TIMEOUT まで（普段の書きは 1 秒未満）
    let deadline = Instant::now() + KEYCHAIN_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(_) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() { let _ = e.read_to_string(&mut err); }
                return Err(format!("Can't save to the Keychain: {}", err.trim()));
            }
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("The Keychain (security) didn't answer in {} s. Another process may have stopped partway through writing the Keychain (restarting the Mac clears it). If a confirmation dialog is showing, press Allow", KEYCHAIN_TIMEOUT.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Keychain から。未保存なら Ok(None)
pub fn keychain_load() -> Result<Option<Tokens>, String> {
    let out = Command::new("security").args(["find-generic-password", "-a", KEYCHAIN_ACCOUNT, "-s", KEYCHAIN_SERVICE, "-w"]).output()
        .map_err(|e| format!("Can't start the security command ({e})"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if out.status.code() == Some(44) || err.contains("could not be found") { return Ok(None) }
        return Err(format!("Can't read the Keychain: {}", err.trim()))
    }
    serde_json::from_str::<Tokens>(String::from_utf8_lossy(&out.stdout).trim()).map(Some)
        .map_err(|_| format!("Can't read the contents of {KEYCHAIN_SERVICE} in the Keychain. Delete it with `security delete-generic-password -a {KEYCHAIN_ACCOUNT} -s {KEYCHAIN_SERVICE}`, then run `kangae-google auth` again"))
}
