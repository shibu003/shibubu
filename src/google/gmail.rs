//! Gmail: thread の一覧（件名・差出人・返信数）・下書きの作成 → id で読み戻し・送信（send_checked）→ id で読み戻し・送った thread の返信。
//! 送信の endpoint を打つのは send_checked だけで、呼ぶのは承認の束を確かめた後の outreach::send_plan だけ（gmail.compose が messages.send を許す）。
//! 出典（2026-10-16 取得）:
//!   https://developers.google.com/gmail/api/reference/rest/v1/users.threads/list（q は Gmail 検索窓と同じ・maxResults 既定 100 上限 500）
//!   https://developers.google.com/gmail/api/reference/rest/v1/users.threads/get（format=metadata・metadataHeaders）
//!   https://developers.google.com/gmail/api/reference/rest/v1/users.drafts/create（scope: gmail.compose）
//!   https://developers.google.com/gmail/api/reference/rest/v1/users.messages（raw = RFC 2822 を base64url）
//!   https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/list（q は検索窓と同じ・2026-10-17 取得）
//!   https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/get（gmail.compose で読める・2026-10-17 取得）
//!   https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send（scope に gmail.compose を含む・body と応答は Message・2026-10-17 取得）
//!   https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list（maxResults 既定 100・上限 500・各 message は id と threadId だけ・2026-10-17 取得）
//!   https://developers.google.com/workspace/gmail/api/guides/filtering（「All dates used in the search query are interpreted as midnight on that date
//!     in the PST timezone」→ 時刻は秒で渡す: in:sent after:1388552400・2026-10-17 取得）
//! 未読の一覧は list_threads(c, UNREAD, n)。
use super::{urlencode, Client, Fail, Written};
use crate::crypto::{base64, base64_decode};
use crate::domain::run::{ErrorCode, Outcome};
use serde_json::{json, Value};

pub const UNREAD: &str = "is:unread in:inbox";
/// Kangae が作る下書きに付ける header。再照合と「自分が作った物だけ消す」の印。
/// Message-ID は使えない: Gmail は下書きの Message-ID を自分の物（…@mail.gmail.com）に書き換える（2026-10-17 本物で実測）。X- の header は残る（同日実測）
pub const KEY_HEADER: &str = "X-Kangae-Key";

#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Thread { pub id: String, pub subject: String, pub from: String, pub replies: usize, pub snippet: String }

/// threads.list(q) → 各 thread を format=metadata で引く。件名と差出人は最初の message、返信数 = message 数 - 1
/// （自分の返信も数える。「相手からの返信」だけ数えるなら From を自分の address と比べる —— それは runtime の仕事）
pub fn list_threads(c: &Client, query: &str, max: usize) -> Result<Vec<Thread>, String> {
    let list = c.get(&format!("{}/gmail/v1/users/me/threads?q={}&maxResults={}", c.gmail_base, urlencode(query), max.clamp(1, 100)))?;
    let heads: Vec<(String, String)> = list["threads"].as_array().map(|a| a.iter()
        .filter_map(|t| Some((t["id"].as_str()?.to_string(), t["snippet"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default();
    let mut out = Vec::with_capacity(heads.len());
    for (id, snippet) in heads {
        let t = c.get(&format!("{}/gmail/v1/users/me/threads/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=From", c.gmail_base, urlencode(&id)))?;
        let msgs = t["messages"].as_array().cloned().unwrap_or_default();
        let first = msgs.first().cloned().unwrap_or(Value::Null);
        out.push(Thread { id, subject: header(&first, "Subject"), from: header(&first, "From"), replies: msgs.len().saturating_sub(1), snippet });
    }
    Ok(out)
}
fn header(msg: &Value, name: &str) -> String { part_header(&msg["payload"], name) }
/// MIME part の header（message なら payload、入れ子の part ならその物）
fn part_header(part: &Value, name: &str) -> String {
    part["headers"].as_array()
        .and_then(|h| h.iter().find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))))
        .and_then(|x| x["value"].as_str()).unwrap_or("").to_string()
}

/// RFC 2822 の text/plain。改行は CRLF、非 ASCII の件名は RFC 2047 の B encoding
pub fn mime_message(to: &str, subject: &str, body: &str) -> String {
    format!("To: {to}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}",
        encode_header(subject), body.replace("\r\n", "\n").replace('\n', "\r\n"))
}
// ponytail: encoded-word を 75 文字で折らない（Gmail は長い encoded-word を受ける）。厳密な RFC 2047 が要るなら 45 byte ごとに分割して空白で繋ぐ
fn encode_header(s: &str) -> String {
    if s.is_ascii() { s.to_string() } else { format!("=?UTF-8?B?{}?=", base64(s.as_bytes(), false)) }
}
/// drafts.create の request body
pub fn draft_request(to: &str, subject: &str, body: &str) -> Value {
    json!({"message": {"raw": base64(mime_message(to, subject, body).as_bytes(), true)}})
}
/// 下書きを作る（送らない・読み戻さない）。戻りは draft id。読み戻して Receipt にするのは create_draft_checked
pub fn create_draft(c: &Client, to: &str, subject: &str, body: &str) -> Result<String, String> {
    check_fields(to, subject, body, &[])?;
    let v = c.post(&format!("{}/gmail/v1/users/me/drafts", c.gmail_base), &draft_request(to, subject, body))?;
    v["id"].as_str().map(str::to_string).ok_or_else(|| "Gmail didn't return a draft ID".to_string())
}
/// Draft の宛先・件名・本文・header の検査（送る前・作る前。HTTP を出さずに止める）
pub fn check_draft_fields(d: &Draft) -> Result<(), String> { check_fields(&d.to, &d.subject, &d.body, &[d.in_reply_to.as_str(), d.references.as_str(), d.key.as_str()]) }
/// 宛先・件名・header に入れる値の検査（改行注入を request の前に止める）
fn check_fields(to: &str, subject: &str, body: &str, headers: &[&str]) -> Result<(), String> {
    let has_newline = |s: &str| s.contains(['\r', '\n']);
    if to.trim().is_empty() || !to.contains('@') { return Err("--to must be a recipient's email address".to_string()) }
    if has_newline(to) || has_newline(subject) { return Err("The recipient and subject can't contain line breaks".to_string()) }
    if headers.iter().any(|h| has_newline(h)) { return Err("The reply-to header contains a line break".to_string()) }
    if subject.trim().is_empty() { return Err("--subject (the subject) is empty".to_string()) }
    if body.len() > 200_000 { return Err("Keep the text within 200,000 bytes".to_string()) }
    Ok(())
}

/// 下書きの中身（承認 card に見せる物・digest の対象）。key は X-Kangae-Key header になり、timeout の後の再照合と削除の印に使う。
/// thread_id / in_reply_to / references は返信の時だけ（Gmail が thread に入れる条件: threadId と References / In-Reply-To と件名）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Draft {
    pub to: String,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub in_reply_to: String,
    #[serde(default)]
    pub references: String,
    pub key: String,
}
pub fn new_key() -> Result<String, String> { Ok(crate::crypto::random(16)?.iter().map(|b| format!("{b:02x}")).collect()) }

/// thread の最後の message（下書きは除く）への返信。宛先は Reply-To か From（to で上書きできる）、件名は Re: を 1 つだけ
// ponytail: 最後の message が自分の送信なら、本当は To へ返す（自分の address を profile で引く）。今は From のまま・to で上書きする
pub fn reply_draft(c: &Client, thread_id: &str, to: Option<&str>, body: &str, key: &str) -> Result<Draft, String> {
    let t = c.get(&format!("{}/gmail/v1/users/me/threads/{}?format=metadata&metadataHeaders=Subject&metadataHeaders=From&metadataHeaders=Reply-To&metadataHeaders=Message-ID&metadataHeaders=References",
        c.gmail_base, urlencode(thread_id)))?;
    let last = t["messages"].as_array().and_then(|m| m.iter().rev().find(|x| !x["labelIds"].as_array().is_some_and(|l| l.iter().any(|v| v == "DRAFT"))))
        .ok_or("This thread has no message to reply to")?;
    let h = |n: &str| header(last, n);
    let subject = h("Subject");
    let subject = if subject.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) { subject } else { format!("Re: {subject}") };
    let to = to.map(str::to_string).unwrap_or_else(|| if h("Reply-To").is_empty() { h("From") } else { h("Reply-To") });
    let mid = h("Message-ID");
    Ok(Draft { to, subject, body: body.to_string(), thread_id: thread_id.to_string(), references: format!("{} {mid}", h("References")).trim().to_string(), in_reply_to: mid, key: key.to_string() })
}
/// drafts.create の request body（X-Kangae-Key と返信の header を足した RFC 2822）
pub fn draft_body(d: &Draft) -> Value {
    let mut head = format!("{KEY_HEADER}: {}\r\n", d.key);
    if !d.in_reply_to.is_empty() { head += &format!("In-Reply-To: {}\r\nReferences: {}\r\n", d.in_reply_to, d.references) }
    let mut message = json!({"raw": base64(format!("{head}{}", mime_message(&d.to, &d.subject, &d.body)).as_bytes(), true)});
    if !d.thread_id.is_empty() { message["threadId"] = json!(d.thread_id) }
    json!({"message": message})
}
fn check_cmd(key: &str) -> String { format!("kangae-google gmail-draft-check --key {key} (same arguments as when it was created)") }

/// 下書きを作る（送らない）→ drafts.get(id) で読み戻し、宛先・件名・本文・thread・key が一致した時だけ Verified。
/// 同じ key の下書きが既に在れば作らない（Calendar の 409 と同じ働き。Gmail は id を client が決められないので先に探す。
/// 同じ承認で 2 回打つと下書きが 2 つになった: 2026-10-17 本物で実測）
pub fn create_draft_checked(c: &Client, d: &Draft) -> Written {
    if let Err(m) = check_draft_fields(d) { return Written::new(None, Outcome::Rejected { reason: m }) }
    let before = check_draft_by_key(c, d);
    match before.outcome {
        Outcome::Failed { error: ErrorCode::TargetNotFound } => {}
        Outcome::Verified { .. } => return Written { detail: "同じ key の下書きが既に在りました（作り直していません）。".into(), ..before },
        Outcome::Unverified { .. } => return Written::failed(before.id.as_deref(), ErrorCode::InvalidInput, "This key was used for a draft with different contents. Make a new key with new_key and show it again"),
        _ => return before, // 在るか確かめられない時は作らない（2 つ作らない方を取る）
    }
    match c.call("POST", &format!("{}/gmail/v1/users/me/drafts", c.gmail_base), Some(&draft_body(d))) {
        Ok(v) => match v["id"].as_str() {
            Some(id) => check_draft(c, id, d).after_accept(),
            None => check_draft_by_key(c, d).after_accept(),
        },
        Err(f) if f.accepted() => check_draft_by_key(c, d).after_accept(),
        Err(f) => Written::from_fail(None, &f, &check_cmd(&d.key)),
    }
}
/// 同じ物が在るか: 新しい下書きから順に X-Kangae-Key を見て探し、在れば比べる。EffectUnknown の後の再照合。無い = Failed{TargetNotFound}（作り直してよい）
// ponytail: Gmail の検索は X- の header を引けないので、2 日以内の新しい 20 件を 1 件ずつ見る（drafts.list は新しい順: 2026-10-17 実測）。下書きが 1 日に 20 件を超える人は取りこぼす
pub fn check_draft_by_key(c: &Client, d: &Draft) -> Written {
    let unknown = |f: super::Fail| Written::new(None, Outcome::EffectUnknown { reason: format!("Can't check whether the draft exists: {}", f.message) });
    let list = match c.call("GET", &format!("{}/gmail/v1/users/me/drafts?q={}&maxResults=20", c.gmail_base, urlencode("in:drafts newer_than:2d")), None) { Ok(v) => v, Err(f) => return unknown(f) };
    for id in list["drafts"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str()) {
        match c.call("GET", &format!("{}/gmail/v1/users/me/drafts/{}?format=metadata", c.gmail_base, urlencode(id)), None) {
            Ok(v) if header(&v["message"], KEY_HEADER) == d.key => return check_draft(c, id, d),
            Ok(_) => {}
            Err(f) if f.status == Some(404) => {}
            Err(f) => return unknown(f),
        }
    }
    Written::failed(None, ErrorCode::TargetNotFound, "There is no draft with this key (it wasn't created)")
}
/// drafts.get(id, format=full) と見せた中身を比べる
pub fn check_draft(c: &Client, id: &str, d: &Draft) -> Written {
    match c.call("GET", &format!("{}/gmail/v1/users/me/drafts/{}?format=full", c.gmail_base, urlencode(id)), None) {
        Ok(v) => match draft_differs(d, &v["message"]) {
            None => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("drafts.get {id}: recipient, subject, text, thread and {KEY_HEADER} match (nothing was sent)")] }),
            Some(field) => Written::new(Some(id), Outcome::Unverified { reason: format!("The {field} of the draft that was read back differs from what was shown") }),
        },
        Err(f) if f.status == Some(404) => Written::failed(Some(id), ErrorCode::TargetNotFound, "There is no draft with this ID"),
        Err(f) => Written::new(Some(id), Outcome::EffectUnknown { reason: format!("Can't check whether the draft exists: {}", f.message) }),
    }
}
fn draft_differs(d: &Draft, msg: &Value) -> Option<&'static str> {
    let h = |n: &str| header(msg, n);
    let norm = |s: &str| s.replace("\r\n", "\n").trim_end().to_string();
    let body = text_body(msg).unwrap_or_default();
    if !h("To").trim().eq_ignore_ascii_case(d.to.trim()) { Some("recipient") }
    else if h("Subject") != d.subject && h("Subject") != encode_header(&d.subject) { Some("subject") }
    else if norm(&body) != norm(&d.body) { Some("text") }
    else if !d.thread_id.is_empty() && msg["threadId"].as_str() != Some(d.thread_id.as_str()) { Some("thread") }
    else if h(KEY_HEADER) != d.key { Some(KEY_HEADER) }
    else { None }
}
/// Kangae が作った下書き（X-Kangae-Key が付いている）だけ消す。消えた事を読み戻し（404）で確かめる
pub fn delete_draft(c: &Client, id: &str) -> Written {
    let url = format!("{}/gmail/v1/users/me/drafts/{}", c.gmail_base, urlencode(id));
    match c.call("GET", &format!("{url}?format=metadata"), None) {
        Ok(v) if !header(&v["message"], KEY_HEADER).is_empty() => {}
        Ok(_) => return Written::new(Some(id), Outcome::Rejected { reason: format!("Deletes only drafts Shibubu created (the ones with {KEY_HEADER})") }),
        Err(f) if f.status == Some(404) => return Written::failed(Some(id), ErrorCode::TargetNotFound, "There is no draft with this ID (already deleted)"),
        Err(f) => return Written::failed(Some(id), f.code(), f.message),
    }
    match c.call("DELETE", &url, None) {
        Ok(_) => match c.call("GET", &url, None) {
            Err(f) if f.status == Some(404) => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("drafts.get {id}: 404")] }),
            Ok(_) => Written::new(Some(id), Outcome::Unverified { reason: "The draft can still be read after deleting it".into() }),
            Err(f) => Written::new(Some(id), Outcome::Unverified { reason: format!("Gmail accepted it, but the deletion can't be confirmed ({})", f.message) }),
        },
        Err(f) => Written::from_fail(Some(id), &f, &format!("kangae-google gmail-draft-delete --id {id}")),
    }
}

/// message の text/plain 本文（単一 part は payload.body.data、multipart は入れ子の中の最初の text/plain）。
/// charset が UTF-8 / US-ASCII でない（ISO-2022-JP 等。crate を足さないので変換しない）・text/plain が無い時は None
pub fn text_body(msg: &Value) -> Option<String> {
    fn find(p: &Value) -> Option<&Value> {
        let plain = p["mimeType"].as_str().is_none_or(|m| m.eq_ignore_ascii_case("text/plain"));
        if plain && p["body"]["data"].as_str().is_some_and(|s| !s.is_empty()) { return Some(p) }
        p["parts"].as_array()?.iter().find_map(find)
    }
    let part = find(&msg["payload"])?;
    let ctype = part_header(part, "Content-Type").to_lowercase();
    let charset = ctype.split(';').filter_map(|x| x.trim().strip_prefix("charset=")).next().unwrap_or("utf-8").trim_matches('"').to_string();
    if !matches!(charset.as_str(), "utf-8" | "utf8" | "us-ascii" | "ascii") { return None }
    base64_decode(part["body"]["data"].as_str()?).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// 送る（承認の束を確かめた後の outreach::send_plan からだけ呼ぶ）。messages.send → messages.get(id, full) で SENT label と中身が一致した時だけ Verified。
/// Verified = Gmail の送信済みに在る事まで。相手に届いたかは返信か不達の通知で分かる（200 は送れた証明ではない: gmail.md §1.4）
pub fn send_checked(c: &Client, d: &Draft) -> Written {
    if let Err(m) = check_draft_fields(d) { return Written::new(None, Outcome::Rejected { reason: m }) }
    let accepted_only = || Written::new(None, Outcome::Unverified { reason: format!("Gmail accepted it but didn't return a message ID. Check in Gmail whether Sent has one to {}", d.to) });
    match c.call("POST", &format!("{}/gmail/v1/users/me/messages/send", c.gmail_base), Some(&draft_body(d)["message"])) {
        Ok(v) => match v["id"].as_str() { Some(id) => check_sent(c, id, d).after_accept(), None => accepted_only() },
        Err(f) if f.accepted() => accepted_only(),
        Err(f) if f.unknown() => Written::new(None, Outcome::EffectUnknown { reason: format!("Can't tell whether it was sent: {}. It won't be sent again on its own. Run outreach-send again with the same approval; recipients already in Sent are skipped", f.message) }),
        Err(f) => Written::failed(None, f.code(), format!("{} (nothing was sent to {})", f.message, d.to)),
    }
}
/// messages.get(id, full) と見せた中身を比べる。SENT label が無ければ Unverified
pub fn check_sent(c: &Client, id: &str, d: &Draft) -> Written {
    match c.call("GET", &format!("{}/gmail/v1/users/me/messages/{}?format=full", c.gmail_base, urlencode(id)), None) {
        Ok(m) if !has_label(&m, "SENT") => Written::new(Some(id), Outcome::Unverified { reason: "The message that was sent isn't in Sent (SENT)".into() }),
        Ok(m) => match draft_differs(d, &m) {
            None => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("messages.get {id}: SENT, recipient, subject, text and {KEY_HEADER} match (whether it arrived shows in a reply or a bounce notice)")] }),
            Some(field) => Written::new(Some(id), Outcome::Unverified { reason: format!("The {field} of the message that was sent differs from what was shown") }),
        },
        Err(f) if f.status == Some(404) => Written::failed(Some(id), ErrorCode::TargetNotFound, "There is no message with this ID"),
        Err(f) => Written::new(Some(id), Outcome::EffectUnknown { reason: format!("Can't read the message that was sent: {}", f.message) }),
    }
}
fn has_label(m: &Value, label: &str) -> bool { m["labelIds"].as_array().is_some_and(|l| l.iter().any(|v| v == label)) }

/// Kangae が送った message（X-Kangae-Key の付いた物）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SentMsg { pub id: String, pub thread_id: String, pub key: String, pub to: String }
/// 送信済みを query（Gmail の検索窓と同じ・in:sent を含める）で引き、X-Kangae-Key の付いた物だけ新しい順に返す。cap 件見つけたら止める。
/// Gmail の検索は X- の header を引けないので 1 件ずつ metadata を読む。1 日の上限の数えと「同じ宛先に 2 回送らない」の判定が両方これに乗る
pub fn kangae_sent(c: &Client, query: &str, cap: usize) -> Result<Vec<SentMsg>, Fail> {
    const MAX_SCAN: usize = 10_000; // Workspace の 1 日 2,000 通の 5 倍。超えたら数え切れないので Err（少なく数えて上限を越えない）
    let (mut out, mut scanned, mut token) = (Vec::new(), 0usize, String::new());
    loop {
        let page = if token.is_empty() { String::new() } else { format!("&pageToken={}", urlencode(&token)) };
        let list = c.call("GET", &format!("{}/gmail/v1/users/me/messages?q={}&maxResults=500{page}", c.gmail_base, urlencode(query)), None)?;
        for m in list["messages"].as_array().into_iter().flatten() {
            let (Some(id), thread_id) = (m["id"].as_str(), m["threadId"].as_str().unwrap_or("")) else { continue };
            scanned += 1;
            let v = c.call("GET", &format!("{}/gmail/v1/users/me/messages/{}?format=metadata&metadataHeaders={KEY_HEADER}&metadataHeaders=To", c.gmail_base, urlencode(id)), None)?;
            let key = header(&v, KEY_HEADER);
            if !key.is_empty() { out.push(SentMsg { id: id.into(), thread_id: thread_id.into(), key, to: header(&v, "To") }) }
            if out.len() >= cap { return Ok(out) }
        }
        token = list["nextPageToken"].as_str().unwrap_or("").to_string();
        if token.is_empty() { return Ok(out) }
        if scanned >= MAX_SCAN { return Err(Fail { status: None, delivered: false, message: format!("Sent has more than {MAX_SCAN} messages, too many to count ({query})") }) }
    }
}

/// 返信 1 通（本文は引用を切った text/plain。UTF-8 以外・text/plain が無い時は Gmail の snippet）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Reply { pub id: String, pub from: String, pub date: String, pub body: String, pub snippet_only: bool }
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Replies { pub thread_id: String, pub count: usize, pub latest: Option<Reply> }
/// thread の中で after（自分が送った message の id）より後の、SENT と DRAFT の付かない message = 相手からの返信。
/// after が無ければ thread の最初の SENT より後（自分が 1 通も送っていない thread は全部）
pub fn thread_replies(c: &Client, thread_id: &str, after: Option<&str>) -> Result<Replies, String> {
    let t = c.get(&format!("{}/gmail/v1/users/me/threads/{}?format=full", c.gmail_base, urlencode(thread_id)))?;
    let mut msgs = t["messages"].as_array().cloned().unwrap_or_default();
    msgs.sort_by_key(|m| m["internalDate"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    let start = match after {
        Some(a) => msgs.iter().position(|m| m["id"] == a).map(|i| i + 1).ok_or_else(|| format!("Thread {thread_id} has no message {a}"))?,
        None => msgs.iter().position(|m| has_label(m, "SENT")).map(|i| i + 1).unwrap_or(0),
    };
    let theirs: Vec<&Value> = msgs[start..].iter().filter(|m| !has_label(m, "SENT") && !has_label(m, "DRAFT")).collect();
    let latest = theirs.last().map(|m| {
        let text = text_body(m).map(|b| strip_quoted(&b));
        Reply { id: m["id"].as_str().unwrap_or("").into(), from: header(m, "From"), date: header(m, "Date"),
            snippet_only: text.as_deref().is_none_or(str::is_empty), body: text.filter(|b| !b.is_empty()).unwrap_or_else(|| m["snippet"].as_str().unwrap_or("").to_string()) }
    });
    Ok(Replies { thread_id: thread_id.into(), count: theirs.len(), latest })
}
/// 返信の本文から引用（> の行・「… wrote:」「…書きました:」「… <mail>:」の行・Outlook の区切り）より下を切る
pub fn strip_quoted(body: &str) -> String {
    let quoted = |t: &str| t.starts_with('>') || t.ends_with("wrote:") || t.ends_with("書きました:") || t.ends_with("書きました：")
        || (t.ends_with(">:") && t.contains('@')) || t.starts_with("-----Original Message") || t.starts_with("________________________________");
    let mut kept: Vec<&str> = body.lines().take_while(|l| !quoted(l.trim())).collect();
    // 「On … Name <mail>」で折り返されて wrote: だけ次の行に来た形（2026-10-17 本物の返信で実測）→ 前半の行も引用の頭として切る
    let cut = kept.len() < body.lines().count();
    while kept.last().is_some_and(|l| l.trim().is_empty()) { kept.pop(); }
    if cut && kept.last().is_some_and(|l| { let t = l.trim(); t.ends_with('>') && t.contains('@') && t.contains('<') }) { kept.pop(); }
    kept.join("\n").trim().to_string()
}
