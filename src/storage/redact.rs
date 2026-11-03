//! secret を伏せる（CORE-012 の redact を `storage::log` から出した物・W19-3）。self-contained: serde_json と std だけ・`crate::` も `super::` も使わない
//! （`storage::log` が re-export し、intel が `#[path]` で同じ file を取り込んで解析の結果を伏せる）。
use serde_json::Value;

pub const REDACTED: &str = "[redacted]";

/// key 名が secret を示す（token / secret / password / authorization / api_key / cookie / credential …）。
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token", "secret", "password", "passwd", "authorization", "api_key", "apikey", "api-key",
        "cookie", "credential", "private_key",
    ]
    .iter()
    .any(|s| k.contains(s))
}

/// key 名が本文を示す（content / body / prompt / text / html / transcript で終わる）。
pub fn is_content_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["content", "body", "prompt", "text", "html", "transcript"].iter().any(|s| k.ends_with(s))
}

// ponytail: regex 無しの語単位の走査。`--token x` や `password x`（区切りが空白だけ）は伏せない。
// 足りなくなったら prefix / key の表に足す。
const SECRET_PREFIXES: &[&str] = &[
    "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", // GitHub
    "sk-", "sk_live_", "sk_test_", // OpenAI / Anthropic / Stripe
    "ya29.", "1//", "AIza", // Google OAuth / API key
    "xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-", // Slack
    "AKIA", // AWS
];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+')
}

fn looks_like_secret(word: &str) -> bool {
    (word.len() >= 20 && SECRET_PREFIXES.iter().any(|p| word.starts_with(p)))
        || (word.len() >= 30 && word.starts_with("eyJ") && word.matches('.').count() == 2) // JWT
}

/// 文字列の中の secret を `[redacted]` に置き換える。
/// 伏せる物: 既知 prefix の token（GitHub / OpenAI / Google / Slack / AWS / JWT）、
/// `key=value` / `key: value` / `"key":"value"` の value（key が `is_secret_key`）、`Bearer xxx` の xxx、秘密鍵の PEM の塊。
pub fn redact(s: &str) -> String {
    let pem = redact_pem(s);
    let mut out = String::with_capacity(pem.len());
    let mut rest = pem.as_str();
    let mut armed = false; // 次の語を伏せる
    let mut last_word = "";
    loop {
        let sep_len = rest.find(is_token_char).unwrap_or(rest.len());
        let (sep, tail) = rest.split_at(sep_len);
        out.push_str(sep);
        if (sep.contains('=') || sep.contains(':')) && is_secret_key(last_word) {
            armed = true;
        }
        let word_len = tail.find(|c| !is_token_char(c)).unwrap_or(tail.len());
        let (word, after) = tail.split_at(word_len);
        if word.is_empty() {
            break;
        }
        let scheme = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic");
        if armed && scheme {
            out.push_str(word); // `Authorization: Bearer xxx` の Bearer は残し、次を伏せる
        } else if armed || looks_like_secret(word) {
            out.push_str(REDACTED);
            armed = false;
        } else {
            out.push_str(word);
            armed = word.eq_ignore_ascii_case("bearer");
        }
        last_word = word;
        rest = after;
    }
    out
}

/// JSON を再帰して伏せる。secret な key の値は型を問わず丸ごと `[redacted]`、文字列は `redact`。
pub fn redact_value(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let v = if is_secret_key(k) { Value::from(REDACTED) } else { redact_value(v) };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        Value::String(s) => Value::String(redact(s)),
        other => other.clone(),
    }
}

/// 秘密鍵の PEM の塊（`-----BEGIN … PRIVATE KEY-----` から `-----END … PRIVATE KEY-----` まで・END が無ければ尻まで）を伏せる。
/// 本文の行は既知の prefix を持たない base64 なので、語の走査では伏せられない（W19-3: 解析の対象の文字に混じった鍵）。
fn redact_pem(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("-----BEGIN") {
        let head_end = rest[at..].find("PRIVATE KEY-----").map(|i| at + i + "PRIVATE KEY-----".len());
        let Some(head_end) = head_end.filter(|&e| !rest[at..e].contains('\n')) else {
            out.push_str(&rest[..at + "-----BEGIN".len()]);
            rest = &rest[at + "-----BEGIN".len()..];
            continue;
        };
        out.push_str(&rest[..at]);
        out.push_str(REDACTED);
        let tail = &rest[head_end..];
        rest = match tail.find("-----END").and_then(|e| tail[e..].find("PRIVATE KEY-----").map(|k| e + k + "PRIVATE KEY-----".len())) {
            Some(end) => &tail[end..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}
