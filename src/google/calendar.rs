//! Calendar: primary の予定一覧（read）と、予定の作成 → id で読み戻し（write）。招待（attendees）付きは external_send で、承認なしには作らない。
//! 出典（2026-10-16 / 10-17 取得）: https://developers.google.com/calendar/api/v3/reference/events/list
//!   timeMin / timeMax は RFC3339（offset 必須）、orderBy=startTime は singleEvents=true の時だけ、maxResults 既定 250・上限 2500
//!   https://developers.google.com/workspace/calendar/api/v3/reference/events/insert
//!   id は自分で決めてよい（base32hex = a-v と 0-9・5〜1024 文字・calendar 内 unique）、sendUpdates は all / externalOnly / none（既定 false）
use super::{approved, now, parse_rfc3339, rfc3339_utc, urlencode, Client, Written, CALENDAR_BASE};
use crate::domain::{action::Effect, permission::ConsentReceipt, run::{ErrorCode, Outcome}};
use serde_json::{json, Value};

/// Kangae が作る予定の id の頭。消すのはこの頭の id だけ
pub const ID_PREFIX: &str = "kangae";

#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Event { pub id: String, pub summary: String, pub start: String, pub end: String, pub location: String, pub link: String }

/// 今から days 日先まで（繰り返しは個別に展開・開始順・100 件まで）
pub fn list_events(c: &Client, days: u64) -> Result<Vec<Event>, String> {
    let t0 = now();
    list_events_between(c, t0, t0 + days.clamp(1, 366) * 86400)
}
pub fn list_events_between(c: &Client, t0: u64, t1: u64) -> Result<Vec<Event>, String> {
    let v = c.get(&format!("{}/calendar/v3/calendars/primary/events?timeMin={}&timeMax={}&singleEvents=true&orderBy=startTime&maxResults=100",
        c.calendar_base, urlencode(&rfc3339_utc(t0)), urlencode(&rfc3339_utc(t1))))?;
    Ok(v["items"].as_array().map(|a| a.iter().map(parse_event).collect()).unwrap_or_default())
}
/// local の「今日」= offset を足した日の 0 時〜翌 0 時（UTC 秒）
// ponytail: offset は今の物を使う。夏時間の切替日は 1 時間ずれる（日本に夏時間は無い）。要るなら tz の規則表を読む
pub fn today(now: u64, offset: i64) -> (u64, u64) {
    let local = now as i64 + offset;
    let start = local - local.rem_euclid(86400) - offset;
    (start.max(0) as u64, (start + 86400).max(0) as u64)
}
/// start / end は dateTime（時刻あり）か date（終日）のどちらか
pub fn parse_event(v: &Value) -> Event {
    let when = |k: &str| v[k]["dateTime"].as_str().or(v[k]["date"].as_str()).unwrap_or("").to_string();
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Event { id: s("id"), summary: v["summary"].as_str().unwrap_or("(untitled)").to_string(), start: when("start"), end: when("end"), location: s("location"), link: s("htmlLink") }
}
/// events.insert の request（method / url / scope / effect / body）。attendees が居れば sendUpdates=all で Google が招待 mail を送る = external_send
pub fn event_request(summary: &str, start: &str, end: &str, attendees: &[String], description: &str) -> Result<Value, String> {
    if summary.trim().is_empty() { return Err("--summary (the title) is empty".to_string()) }
    let when = |label: &str, s: &str| -> Result<Value, String> {
        let b = s.as_bytes();
        let date = s.is_ascii() && s.len() == 10 && b[4] == b'-' && b[7] == b'-';
        let datetime = s.is_ascii() && s.len() >= 20 && b[10] == b'T' && (s.ends_with('Z') || s[19..].contains(['+', '-']));
        if date { Ok(json!({"date": s})) } else if datetime { Ok(json!({"dateTime": s})) }
        else { Err(format!("Give --{label} as 2026-10-17T10:00:00+09:00 (the offset is required) or 2026-10-17 (all day) ({s})")) }
    };
    if let Some(a) = attendees.iter().find(|a| !a.contains('@') || a.contains(char::is_whitespace)) { return Err(format!("--attendee must be an email address ({a})")) }
    let mut body = json!({"summary": summary, "start": when("start", start)?, "end": when("end", end)?});
    if !description.is_empty() { body["description"] = json!(description) }
    if !attendees.is_empty() { body["attendees"] = json!(attendees.iter().map(|a| json!({"email": a})).collect::<Vec<_>>()) }
    let effect = if attendees.is_empty() { Effect::ExternalWrite } else { Effect::ExternalSend };
    Ok(json!({"method": "POST",
        "url": format!("{CALENDAR_BASE}/calendar/v3/calendars/primary/events{}", if attendees.is_empty() { "" } else { "?sendUpdates=all" }),
        "scope": "https://www.googleapis.com/auth/calendar.events", "effect": effect, "body": body}))
}
/// 予定の id を自分で決める（20 byte の乱数 → base32hex）。承認の前に決めて本文と digest に入れる →
/// timeout の後に同じ本文で作り直しても Google が 409 を返し、予定が 2 つにならない
pub fn new_event_id() -> Result<String, String> {
    const BASE32HEX: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    Ok(format!("{ID_PREFIX}{}", crate::crypto::random(20)?.iter().map(|b| BASE32HEX[(b & 31) as usize] as char).collect::<String>()))
}
fn invites(body: &Value) -> bool { body["attendees"].as_array().is_some_and(|a| !a.is_empty()) }
fn check_cmd(id: &str) -> String { format!("kangae-google gcal-check --id {id} (same arguments as when it was created)") }

/// 予定を作る → events.get(id) で読み戻して本文と一致した時だけ Verified（CD-53）。body は event_request の body に "id"（new_event_id）を足した物。
/// attendees が居る本文（external_send）は、この本文の digest への承認（ConsentReceipt）が無ければ HTTP を 1 本も出さずに Rejected
pub fn create_event(c: &Client, body: &Value, consent: Option<&ConsentReceipt>) -> Written {
    let Some(id) = body["id"].as_str().filter(|i| i.starts_with(ID_PREFIX)) else {
        return Written::new(None, Outcome::Rejected { reason: format!("The event ID (starts with {ID_PREFIX} · new_event_id) is missing") })
    };
    if invites(body) && !approved(consent, body) {
        return Written::new(Some(id), Outcome::Rejected { reason: "An event with attendees makes Google send invitation emails (sending outside). This event hasn't been approved yet. Approve it first".into() })
    }
    let url = format!("{}/calendar/v3/calendars/primary/events?sendUpdates={}", c.calendar_base, if invites(body) { "all" } else { "none" });
    match c.call("POST", &url, Some(body)) {
        Ok(_) => check_event(c, body).after_accept(),
        Err(f) if f.accepted() => check_event(c, body).after_accept(),
        // 同じ id が既に在る = 前の試行が通っていた（作り直さず読み戻す）。消した予定の id は使い直せない
        Err(f) if f.status == Some(409) => {
            let w = check_event(c, body);
            if w.verified() { Written { detail: "An event with the same ID already existed (a previous attempt had created it)".into(), ..w } }
            else { Written::failed(Some(id), ErrorCode::InvalidInput, format!("This ID is already in use ({}). Make a new ID with new_event_id and show it again", w.why())) }
        }
        Err(f) => Written::from_fail(Some(id), &f, &check_cmd(id)),
    }
}
/// 同じ物が在るか: events.get(id) と本文を比べる。作成直後の読み戻しと、EffectUnknown の後の再照合の両方で使う。
/// 無い（404・消された）= Failed{TargetNotFound}（作られていない → 同じ id で作り直してよい）、読めない = EffectUnknown のまま
pub fn check_event(c: &Client, body: &Value) -> Written {
    let id = body["id"].as_str().unwrap_or("");
    match c.call("GET", &format!("{}/calendar/v3/calendars/primary/events/{}", c.calendar_base, urlencode(id)), None) {
        Ok(v) if v["status"] == "cancelled" => Written::failed(Some(id), ErrorCode::TargetNotFound, "The event with this ID has been deleted"),
        Ok(v) => match differs(body, &v) {
            None => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("events.get {id}: title, start, end, description and attendees match"), v["htmlLink"].as_str().unwrap_or("").to_string()] }),
            Some(field) => Written::new(Some(id), Outcome::Unverified { reason: format!("The {field} of the event that was read back differs from what was shown") }),
        },
        Err(f) if matches!(f.status, Some(404 | 410)) => Written::failed(Some(id), ErrorCode::TargetNotFound, "There is no event with this ID (it wasn't created)"),
        Err(f) => Written::new(Some(id), Outcome::EffectUnknown { reason: format!("Can't check whether the event exists: {}", f.message) }),
    }
}
/// 見せた本文と読み戻しの違う所（無ければ None）。時刻は瞬間で比べる（Google は calendar の tz に直して返す）
fn differs(want: &Value, got: &Value) -> Option<&'static str> {
    let when = |w: &Value, g: &Value| match (w["date"].as_str(), w["dateTime"].as_str()) {
        (Some(d), _) => g["date"].as_str() == Some(d),
        (_, Some(t)) => parse_rfc3339(t).is_some() && parse_rfc3339(t) == g["dateTime"].as_str().and_then(parse_rfc3339),
        _ => false,
    };
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let emails = |v: &Value| {
        let mut e: Vec<String> = v["attendees"].as_array().map(|a| a.iter().filter_map(|x| x["email"].as_str()).map(str::to_lowercase).collect()).unwrap_or_default();
        e.sort();
        e
    };
    if text(want, "summary") != text(got, "summary") { Some("title") }
    else if !when(&want["start"], &got["start"]) { Some("start") }
    else if !when(&want["end"], &got["end"]) { Some("end") }
    else if text(want, "description") != text(got, "description") { Some("description") }
    else if emails(want) != emails(got) { Some("attendees") }
    else { None }
}
/// Kangae が作った予定（id が kangae で始まる）だけ消す。取消の mail は出さない（sendUpdates=none）。消えた事を読み戻しで確かめる
pub fn delete_event(c: &Client, id: &str) -> Written {
    if !id.starts_with(ID_PREFIX) { return Written::new(Some(id), Outcome::Rejected { reason: format!("Deletes only events Shibubu created (IDs that start with {ID_PREFIX})") }) }
    let url = format!("{}/calendar/v3/calendars/primary/events/{}", c.calendar_base, urlencode(id));
    match c.call("DELETE", &format!("{url}?sendUpdates=none"), None) {
        Ok(_) => match c.call("GET", &url, None) {
            Ok(v) if v["status"] == "cancelled" => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("events.get {id}: status=cancelled")] }),
            Err(f) if matches!(f.status, Some(404 | 410)) => Written::new(Some(id), Outcome::Verified { evidence: vec![format!("events.get {id}: {}", f.status.unwrap_or(404))] }),
            Ok(_) => Written::new(Some(id), Outcome::Unverified { reason: "The event can still be read after deleting it".into() }),
            Err(f) => Written::new(Some(id), Outcome::Unverified { reason: format!("Google accepted it, but the deletion can't be confirmed ({})", f.message) }),
        },
        Err(f) if matches!(f.status, Some(404 | 410)) => Written::failed(Some(id), ErrorCode::TargetNotFound, "There is no event with this ID (already deleted)"),
        Err(f) => Written::from_fail(Some(id), &f, &format!("kangae-google gcal-delete --id {id}")),
    }
}
