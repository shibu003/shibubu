//! Google 接続の試験。本物の Google は叩かず、127.0.0.1 の偽 HTTP server で
//! token refresh の request / threads の parse / draft の request body / loopback の redirect 受け口を確かめる。
//! 書き込み（予定・下書き）は「作る → id で読み戻し → 一致で Verified」の Receipt の材料（Written）を、偽 server の応答ごとに見る。
//! Keychain には触らない（触るのは bin だけ）。
#[path = "../src/crypto.rs"]
mod crypto;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/google/mod.rs"]
mod google;
use domain::{permission::{ConsentDecision, ConsentReceipt, ConsentSubject}, run::{ErrorCode, Outcome}};
use google::{calendar, contacts::{self, Contact}, gmail, oauth::{self, Config, Tokens}, outreach, Client, Written};
use serde_json::{json, Value};
use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex}, thread, time::Duration};

struct Req { method: String, path: String, headers: Vec<(String, String)>, body: String }
impl Req { fn header(&self, k: &str) -> Option<&str> { self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str()) } }

/// 偽 server。request を記録し、path（か "METHOD path"）の前方一致で最初に合う route の応答（無ければ 404）を返す。
/// status 0 = request を読んだ後に応答せず切る（届いた後の切断・timeout と同じ枝）、204 = 本文なし
fn fake(routes: Vec<(&'static str, u16, Value)>) -> (String, Arc<Mutex<Vec<Req>>>) {
    fake_with(move |req| {
        let line = format!("{} {}", req.method, req.path);
        routes.iter().find(|(p, _, _)| req.path.starts_with(p) || line.starts_with(p)).map(|(_, c, b)| (*c, b.to_string())).unwrap_or((404, "{}".to_string()))
    })
}
/// 偽 server の本体。handler が request ごとに (status, body) を決める（状態を持つ偽 Gmail 用）
fn fake_with(handler: impl Fn(&Req) -> (u16, String) + Send + 'static) -> (String, Arc<Mutex<Vec<Req>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = Arc::clone(&log);
    thread::spawn(move || for conn in l.incoming() {
        let Ok(mut s) = conn else { break };
        let req = read_request(&mut s);
        let (status, body) = handler(&req);
        let body = if status == 204 { String::new() } else { body };
        // 応答の前に記録する（応答を受けた試験が log を読む時に、最後の request が必ず入っている）
        log2.lock().unwrap().push(req);
        if status != 0 { let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()); }
    });
    (base, log)
}
/// header を \r\n\r\n まで、body を Content-Length 分読む（ureq は header と body を分けて書く事がある）
fn read_request(s: &mut TcpStream) -> Req {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let (mut buf, mut tmp) = (Vec::new(), [0u8; 4096]);
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p }
        match s.read(&mut tmp) { Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]), _ => break buf.len() }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let body_start = (head_end + 4).min(buf.len());
    while buf.len() < body_start + len { match s.read(&mut tmp) { Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]), _ => break } }
    let body = String::from_utf8_lossy(&buf[body_start..(body_start + len).min(buf.len())]).to_string();
    Req { method, path, headers, body }
}
fn client(base: &str) -> Client { Client { access_token: "tok".to_string(), gmail_base: base.to_string(), calendar_base: base.to_string() } }
fn cfg() -> Config { Config { client_id: "cid".to_string(), client_secret: "sec".to_string() } }

#[test]
fn token_refresh_posts_form_and_keeps_refresh_token() {
    let (base, log) = fake(vec![("/token", 200, json!({"access_token": "new-acc", "expires_in": 3600, "scope": "s1 s2", "token_type": "Bearer"}))]);
    let url = format!("{base}/token");
    let old = Tokens { access_token: "old".to_string(), refresh_token: "rt-1".to_string(), expires_at: 0, scope: String::new() };
    let (t, changed) = oauth::refresh_if_needed(&cfg(), &url, old.clone()).unwrap();
    assert!(!changed, "refresh token が同じなら Keychain に書かない（access token は memory だけ）");
    assert_eq!(t.access_token, "new-acc");
    assert_eq!(t.refresh_token, "rt-1", "refresh の応答に refresh_token が無い時は旧を持ち続ける");
    assert!(t.expires_at > google::now() + 3000);
    assert_eq!(t.scope, "s1 s2");
    {
        let r = &log.lock().unwrap()[0];
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/token"));
        assert_eq!(r.header("content-type"), Some("application/x-www-form-urlencoded"));
        for needle in ["grant_type=refresh_token", "refresh_token=rt-1", "client_id=cid", "client_secret=sec"] { assert!(r.body.contains(needle), "{needle} が無い: {}", r.body) }
    }
    // まだ切れていない token は触らない（request も出ない）
    let fresh = Tokens { expires_at: google::now() + 3600, ..old.clone() };
    let (t2, changed2) = oauth::refresh_if_needed(&cfg(), &url, fresh.clone()).unwrap();
    assert!(!changed2);
    assert_eq!(t2, fresh);
    assert_eq!(log.lock().unwrap().len(), 1);
    // Google が refresh token を新しくした時だけ書き直す
    let (base_rt, _) = fake(vec![("/token", 200, json!({"access_token": "acc-2", "expires_in": 3600, "refresh_token": "rt-2"}))]);
    let (t3, changed3) = oauth::refresh_if_needed(&cfg(), &format!("{base_rt}/token"), old.clone()).unwrap();
    assert!(changed3 && t3.refresh_token == "rt-2", "{t3:?}");
    // invalid_grant（Testing 状態の 7 日失効・取り消し）は再認証を案内する
    let (base2, _) = fake(vec![("/token", 400, json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."}))]);
    let e = oauth::refresh(&cfg(), &format!("{base2}/token"), &fresh).unwrap_err();
    assert!(e.contains("kangae-google auth") && e.contains("7 days"), "{e}");
    // code 交換: form の中身と refresh_token 必須
    let (base3, log3) = fake(vec![("/token", 200, json!({"access_token": "acc", "expires_in": 3599, "refresh_token": "rt-new", "scope": "s"}))]);
    let t = oauth::exchange_code(&cfg(), &format!("{base3}/token"), "4/abc", "http://127.0.0.1:1234", "ver-1").unwrap();
    assert_eq!((t.access_token.as_str(), t.refresh_token.as_str()), ("acc", "rt-new"));
    let b = log3.lock().unwrap()[0].body.clone();
    for needle in ["code=4%2Fabc", "grant_type=authorization_code", "redirect_uri=http%3A%2F%2F127.0.0.1%3A1234", "code_verifier=ver-1", "client_id=cid", "client_secret=sec"] { assert!(b.contains(needle), "{needle} が無い: {b}") }
    let (base4, _) = fake(vec![("/token", 200, json!({"access_token": "acc", "expires_in": 3599}))]);
    assert!(oauth::exchange_code(&cfg(), &format!("{base4}/token"), "c", "r", "v").unwrap_err().contains("refresh token"));
}

#[test]
fn auth_url_requests_minimal_scopes_offline_consent_and_pkce() {
    let u = oauth::auth_url(&cfg(), "http://127.0.0.1:1234", "ver-1", "st-1");
    assert!(u.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=cid&"), "{u}");
    for needle in ["redirect_uri=http%3A%2F%2F127.0.0.1%3A1234", "response_type=code", "code_challenge_method=S256", "state=st-1",
                   "access_type=offline", "prompt=consent", "gmail.readonly", "gmail.compose", "calendar.events"] { assert!(u.contains(needle), "{needle} が無い: {u}") }
    assert!(!u.contains("gmail.send") && !u.contains("gmail.modify") && !u.contains("mail.google.com"), "送信・全権の scope は要求しない: {u}");
}

fn hit(port: u16, target: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}
#[test]
fn loopback_redirect_accepts_code_once_and_rejects_bad_state() {
    let run = |state: &'static str, targets: Vec<&'static str>| -> (Vec<String>, Result<String, String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = thread::spawn(move || oauth::wait_for_code(&l, state, Duration::from_secs(10)));
        let responses = targets.iter().map(|t| hit(port, t)).collect();
        (responses, h.join().unwrap())
    };
    let (resp, r) = run("st-1", vec!["/favicon.ico", "/?state=st-1&code=4%2Fabc&scope=x"]);
    assert!(resp[0].starts_with("HTTP/1.1 404"), "{}", resp[0]);
    assert!(resp[1].starts_with("HTTP/1.1 200") && resp[1].contains("is done"), "{}", resp[1]);
    assert_eq!(r.unwrap(), "4/abc");
    let (resp, r) = run("st-1", vec!["/?state=wrong&code=abc"]);
    assert!(resp[0].starts_with("HTTP/1.1 400"), "{}", resp[0]);
    assert!(r.unwrap_err().contains("state"));
    let (resp, r) = run("st-1", vec!["/?error=access_denied&state=st-1"]);
    assert!(resp[0].starts_with("HTTP/1.1 400"), "{}", resp[0]);
    assert!(r.unwrap_err().contains("access_denied"));
    // 誰も来なければ期限で止まる（hang しない）
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    assert!(oauth::wait_for_code(&l, "s", Duration::from_millis(300)).unwrap_err().contains("no answer came from Google"));
}

#[test]
fn threads_parse_subject_from_and_reply_count() {
    let t1 = json!({"id": "t1", "messages": [
        {"id": "m1", "payload": {"headers": [{"name": "Subject", "value": "Hello"}, {"name": "From", "value": "Alice <a@x.io>"}]}},
        {"id": "m2", "payload": {"headers": [{"name": "Subject", "value": "Re: Hello"}, {"name": "From", "value": "me@x.io"}]}}]});
    let t2 = json!({"id": "t2", "messages": [{"id": "m3", "payload": {"headers": [{"name": "subject", "value": "Solo"}, {"name": "from", "value": "b@x.io"}]}}]});
    let (base, log) = fake(vec![
        ("/gmail/v1/users/me/threads/t1", 200, t1), ("/gmail/v1/users/me/threads/t2", 200, t2),
        ("/gmail/v1/users/me/threads", 200, json!({"threads": [{"id": "t1", "snippet": "s1"}, {"id": "t2", "snippet": "s2"}], "resultSizeEstimate": 2}))]);
    let threads = gmail::list_threads(&client(&base), "newer_than:7d", 20).unwrap();
    assert_eq!(threads, vec![
        gmail::Thread { id: "t1".to_string(), subject: "Hello".to_string(), from: "Alice <a@x.io>".to_string(), replies: 1, snippet: "s1".to_string() },
        gmail::Thread { id: "t2".to_string(), subject: "Solo".to_string(), from: "b@x.io".to_string(), replies: 0, snippet: "s2".to_string() }]);
    {
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 3);
        assert!(log[0].path.starts_with("/gmail/v1/users/me/threads?") && log[0].path.contains("q=newer_than%3A7d") && log[0].path.contains("maxResults=20"), "{}", log[0].path);
        assert_eq!(log[0].header("authorization"), Some("Bearer tok"));
        assert!(log[1].path.contains("/threads/t1?format=metadata") && log[1].path.contains("metadataHeaders=Subject") && log[1].path.contains("metadataHeaders=From"), "{}", log[1].path);
        assert!(log.iter().all(|r| r.method == "GET"));
    }
    // 空の結果
    let (base2, _) = fake(vec![("/gmail/v1/users/me/threads", 200, json!({"resultSizeEstimate": 0}))]);
    assert!(gmail::list_threads(&client(&base2), "x", 5).unwrap().is_empty());
    // 401 は再認証の案内
    let (base3, _) = fake(vec![("/gmail", 401, json!({"error": {"code": 401, "message": "Invalid Credentials", "status": "UNAUTHENTICATED"}}))]);
    let e = gmail::list_threads(&client(&base3), "x", 5).unwrap_err();
    assert!(e.contains("kangae-google auth"), "{e}");
    // 403 で API 未有効
    let (base4, _) = fake(vec![("/gmail", 403, json!({"error": {"code": 403, "message": "Gmail API has not been used in project 1 before or it is disabled.", "status": "PERMISSION_DENIED"}}))]);
    let e = gmail::list_threads(&client(&base4), "x", 5).unwrap_err();
    assert!(e.contains("有効"), "{e}");
}

#[test]
fn draft_request_body_is_base64url_rfc2822_and_never_sends() {
    let (base, log) = fake(vec![("/gmail/v1/users/me/drafts", 200, json!({"id": "d1", "message": {"id": "m9", "threadId": "t9"}}))]);
    let id = gmail::create_draft(&client(&base), "a@x.io", "Hi", "line1\nline2").unwrap();
    assert_eq!(id, "d1");
    {
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].method.as_str(), log[0].path.as_str()), ("POST", "/gmail/v1/users/me/drafts"));
        assert!(!log[0].path.contains("send"));
        let body: Value = serde_json::from_str(&log[0].body).unwrap();
        // Python: base64.urlsafe_b64encode(b'To: a@x.io\r\nSubject: Hi\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset="UTF-8"\r\nContent-Transfer-Encoding: 8bit\r\n\r\nline1\r\nline2')
        assert_eq!(body["message"]["raw"], "VG86IGFAeC5pbw0KU3ViamVjdDogSGkNCk1JTUUtVmVyc2lvbjogMS4wDQpDb250ZW50LVR5cGU6IHRleHQvcGxhaW47IGNoYXJzZXQ9IlVURi04Ig0KQ29udGVudC1UcmFuc2Zlci1FbmNvZGluZzogOGJpdA0KDQpsaW5lMQ0KbGluZTI=");
    }
    // 日本語の件名は RFC 2047（Python: base64.b64encode('こんにちは'.encode())）
    assert!(gmail::mime_message("a@x.io", "こんにちは", "").starts_with("To: a@x.io\r\nSubject: =?UTF-8?B?44GT44KT44Gr44Gh44Gv?=\r\n"));
    // header injection と不正な宛先は request を出さずに止める
    let (base2, log2) = fake(vec![]);
    assert!(gmail::create_draft(&client(&base2), "a@x.io\r\nBcc: evil@x.io", "s", "b").is_err());
    assert!(gmail::create_draft(&client(&base2), "a@x.io", "s\nX: y", "b").is_err());
    assert!(gmail::create_draft(&client(&base2), "nobody", "s", "b").is_err());
    assert!(gmail::create_draft(&client(&base2), "a@x.io", " ", "b").is_err());
    assert!(log2.lock().unwrap().is_empty());
}

#[test]
fn calendar_events_parse_and_create_is_json_only() {
    let items = json!({"items": [
        {"id": "e1", "summary": "打合せ", "start": {"dateTime": "2026-10-17T10:00:00+09:00"}, "end": {"dateTime": "2026-10-17T11:00:00+09:00"}, "location": "Zoom", "htmlLink": "https://www.google.com/calendar/event?eid=1"},
        {"id": "e2", "start": {"date": "2026-10-18"}, "end": {"date": "2026-10-19"}}]});
    let (base, log) = fake(vec![("/calendar/v3/calendars/primary/events", 200, items)]);
    let ev = calendar::list_events(&client(&base), 7).unwrap();
    assert_eq!(ev[0], calendar::Event { id: "e1".to_string(), summary: "打合せ".to_string(), start: "2026-10-17T10:00:00+09:00".to_string(),
        end: "2026-10-17T11:00:00+09:00".to_string(), location: "Zoom".to_string(), link: "https://www.google.com/calendar/event?eid=1".to_string() });
    assert_eq!((ev[1].summary.as_str(), ev[1].start.as_str()), ("(untitled)", "2026-10-18"));
    {
        let p = &log.lock().unwrap()[0].path;
        for needle in ["singleEvents=true", "orderBy=startTime", "timeMin=", "timeMax=", "maxResults=100"] { assert!(p.contains(needle), "{needle} が無い: {p}") }
        assert!(p.contains("Z&"), "timeMin / timeMax は RFC3339 UTC: {p}");
    }
    // gcal-create は JSON を作るだけ（HTTP を出さない）
    let req = calendar::event_request("打合せ", "2026-10-17T10:00:00+09:00", "2026-10-17T11:00:00+09:00", &["a@x.io".to_string()], "memo").unwrap();
    assert_eq!(req["method"], "POST");
    assert_eq!(req["url"], "https://www.googleapis.com/calendar/v3/calendars/primary/events?sendUpdates=all");
    assert_eq!(req["body"], json!({"summary": "打合せ", "start": {"dateTime": "2026-10-17T10:00:00+09:00"}, "end": {"dateTime": "2026-10-17T11:00:00+09:00"}, "description": "memo", "attendees": [{"email": "a@x.io"}]}));
    let allday = calendar::event_request("x", "2026-10-17", "2026-10-18", &[], "").unwrap();
    assert_eq!(allday["body"]["start"], json!({"date": "2026-10-17"}));
    assert!(!allday["url"].as_str().unwrap().contains("sendUpdates"));
    assert!(calendar::event_request("x", "2026-10-17T10:00:00", "2026-10-17T11:00:00", &[], "").is_err(), "offset 無しは Calendar が 400 を返すので先に止める");
    assert!(calendar::event_request(" ", "2026-10-17", "2026-10-18", &[], "").is_err());
    assert!(calendar::event_request("x", "2026-10-17", "2026-10-18", &["not-mail".to_string()], "").is_err());
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[test]
fn encoders_match_known_vectors() {
    for (input, std, url) in [("", "", ""), ("f", "Zg==", "Zg=="), ("fo", "Zm8=", "Zm8="), ("foo", "Zm9v", "Zm9v"), ("foob", "Zm9vYg==", "Zm9vYg=="), ("fooba", "Zm9vYmE=", "Zm9vYmE="), ("foobar", "Zm9vYmFy", "Zm9vYmFy")] {
        assert_eq!(google::base64(input.as_bytes(), false), std);
        assert_eq!(google::base64(input.as_bytes(), true), url);
    }
    assert_eq!(google::base64(&[0xfb, 0xff, 0xbf], false), "+/+/");
    assert_eq!(google::base64(&[0xfb, 0xff, 0xbf], true), "-_-_");
    // Python: datetime.fromtimestamp(n, timezone.utc)
    for (secs, s) in [(0, "1970-01-01T00:00:00Z"), (951_782_400, "2000-02-29T00:00:00Z"), (1_760_000_000, "2025-10-09T08:53:20Z"), (4_102_444_799, "2099-12-31T23:59:59Z")] {
        assert_eq!(google::rfc3339_utc(secs), s);
    }
    assert_eq!(google::urlencode("newer_than:7d from:a@x.io"), "newer_than%3A7d%20from%3Aa%40x.io");
    assert_eq!(google::urldecode("4%2Fabc+d%20e%"), "4/abc d e%");
}

#[test]
fn pkce_s256_matches_rfc7636_appendix_b() {
    // 出典: https://www.rfc-editor.org/rfc/rfc7636.txt Appendix B（2026-10-17 に本文と照合）
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(crypto::sha256(verifier.as_bytes()), [19, 211, 30, 150, 26, 26, 216, 236, 47, 22, 177, 12, 76, 152, 46, 8, 118, 168, 120, 173, 109, 241, 68, 86, 110, 225, 137, 74, 203, 112, 249, 195]);
    assert_eq!(oauth::pkce_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let u = oauth::auth_url(&cfg(), "http://127.0.0.1:1234", verifier, "st-1");
    assert!(u.contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256"), "{u}");
    assert!(!u.contains(verifier), "verifier は URL に乗らない（token の交換でだけ送る）: {u}");
    // FIPS 180-4 の例（NIST: "" と "abc"）と 2 block に跨る長さ
    assert_eq!(crypto::sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(crypto::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(crypto::sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}

#[test]
fn base64_decode_reads_both_alphabets_and_rejects_junk() {
    for input in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "日本語の本文\r\n2 行目"] {
        for url in [false, true] {
            let enc = crypto::base64(input.as_bytes(), url);
            assert_eq!(crypto::base64_decode(&enc).as_deref(), Some(input.as_bytes()), "{enc}");
            assert_eq!(crypto::base64_decode(enc.trim_end_matches('=')).as_deref(), Some(input.as_bytes()), "padding なしも読む");
        }
    }
    assert_eq!(crypto::base64_decode("-_-_").unwrap(), [0xfb, 0xff, 0xbf]);
    assert!(crypto::base64_decode("ab$c").is_none());
    assert!(crypto::base64_decode("abcde").is_none(), "4n+1 文字は base64 ではない");
}

#[test]
fn rfc3339_instants_and_local_today() {
    for secs in [0u64, 951_782_400, 1_760_000_000, 4_102_444_799] { assert_eq!(google::parse_rfc3339(&google::rfc3339_utc(secs)), Some(secs as i64)) }
    assert_eq!(google::parse_rfc3339("2026-10-18T10:00:00+09:00"), google::parse_rfc3339("2026-10-18T01:00:00Z"));
    assert_eq!(google::parse_rfc3339("2026-10-17T20:30:00.123-04:00"), google::parse_rfc3339("2026-10-18T00:30:00Z"));
    for bad in ["2026-10-18T10:00:00", "2026-13-01T00:00:00Z", "2026-10-18 10:00:00Z", "2026-10-18T10:00:00+0900", "２０２６-10-18T10:00:00Z"] { assert_eq!(google::parse_rfc3339(bad), None, "{bad}") }
    // 2026-10-17T16:30:00+09:00 の「今日」= 2026-10-17T00:00+09:00 〜 翌 0 時
    let now = google::parse_rfc3339("2026-10-17T16:30:00+09:00").unwrap() as u64;
    let (t0, t1) = calendar::today(now, 9 * 3600);
    assert_eq!((google::rfc3339_utc(t0), google::rfc3339_utc(t1)), ("2026-10-16T15:00:00Z".to_string(), "2026-10-17T15:00:00Z".to_string()));
    // 0 時を跨いだ直後（+09:00 の 00:10 は UTC では前日）
    let (t0, _) = calendar::today(google::parse_rfc3339("2026-10-18T00:10:00+09:00").unwrap() as u64, 9 * 3600);
    assert_eq!(google::rfc3339_utc(t0), "2026-10-17T15:00:00Z");
    assert_eq!(calendar::today(now, 0).0, google::parse_rfc3339("2026-10-17T00:00:00Z").unwrap() as u64);
}

const EID: &str = "kangae0123456789abcdefghij";
fn event_body(attendees: &[String]) -> Value {
    let mut req = calendar::event_request("[Kangae test] 打合せ", "2026-10-18T10:00:00+09:00", "2026-10-18T11:00:00+09:00", attendees, "").unwrap();
    req["body"]["id"] = json!(EID);
    req["body"].clone()
}
/// Google が返す形: 送った +09:00 を UTC で返す（瞬間は同じ）
fn event_echo(summary: &str) -> Value {
    json!({"id": EID, "status": "confirmed", "summary": summary, "htmlLink": "https://www.google.com/calendar/event?eid=x",
           "start": {"dateTime": "2026-10-18T01:00:00Z", "timeZone": "Asia/Tokyo"}, "end": {"dateTime": "2026-10-18T02:00:00Z"}})
}
fn approve(body: &Value) -> ConsentReceipt {
    ConsentReceipt { id: "c1".into(), app_id: "a".into(), subject: ConsentSubject::Action { action_ref: "calendar.events.create".into(), run_id: None },
        scopes: vec![], payload_digest: Some(google::digest(body)), decision: ConsentDecision::Approved, decided_at: "2026-10-17T00:00:00Z".into() }
}
fn kind(w: &Written) -> &'static str {
    match w.outcome { Outcome::Verified { .. } => "verified", Outcome::Unverified { .. } => "unverified", Outcome::Rejected { .. } => "rejected", Outcome::Failed { .. } => "failed", Outcome::EffectUnknown { .. } => "effect_unknown" }
}
fn methods(log: &Arc<Mutex<Vec<Req>>>) -> Vec<String> { log.lock().unwrap().iter().map(|r| format!("{} {}", r.method, r.path.split('?').next().unwrap_or(""))).collect() }
const EV: &str = "/calendar/v3/calendars/primary/events";

#[test]
fn calendar_create_is_verified_only_after_events_get_matches() {
    let body = event_body(&[]);
    // 作る → 読み戻し（時刻は UTC で返るが瞬間が同じなので一致）
    let (base, log) = fake(vec![("POST /calendar", 200, json!({"id": EID})), ("GET /calendar", 200, event_echo("[Kangae test] 打合せ"))]);
    let w = calendar::create_event(&client(&base), &body, None);
    assert_eq!(kind(&w), "verified", "{w:?}");
    assert_eq!(w.id.as_deref(), Some(EID));
    assert_eq!(methods(&log), [format!("POST {EV}"), format!("GET {EV}/{EID}")]);
    {
        let log = log.lock().unwrap();
        assert!(log[0].path.ends_with("?sendUpdates=none"), "参加者 0 の予定は通知を出さない: {}", log[0].path);
        let sent: Value = serde_json::from_str(&log[0].body).unwrap();
        assert_eq!(sent["id"], EID, "id は自分で決めて送る（再試行で 2 つ作らない）");
        assert!(sent.get("attendees").is_none());
    }
    // CD-53: POST は id を返したが get が 404 → 受理だけ = Unverified
    let (base, _) = fake(vec![("POST /calendar", 200, json!({"id": EID})), ("GET /calendar", 404, json!({"error": {"code": 404, "message": "Not Found"}}))]);
    assert_eq!(kind(&calendar::create_event(&client(&base), &body, None)), "unverified");
    // 読み戻した件名が違う → Unverified（理由に「件名」）
    let (base, _) = fake(vec![("POST /calendar", 200, json!({"id": EID})), ("GET /calendar", 200, event_echo("別の予定"))]);
    let w = calendar::create_event(&client(&base), &body, None);
    assert!(kind(&w) == "unverified" && w.why().contains("The title of the event"), "{w:?}");
    // 2xx で本文が空（204 等）でも読み戻しへ進む
    let (base, _) = fake(vec![("POST /calendar", 204, json!({})), ("GET /calendar", 200, event_echo("[Kangae test] 打合せ"))]);
    assert_eq!(kind(&calendar::create_event(&client(&base), &body, None)), "verified");
}

#[test]
fn calendar_create_failures_never_resend_and_say_what_is_known() {
    let body = event_body(&[]);
    // 届いた後に切れた（timeout と同じ枝）→ EffectUnknown・POST は 1 本だけ（自動で再送しない）・次の一手は gcal-check
    let (base, log) = fake(vec![("POST /calendar", 0, json!({}))]);
    let w = calendar::create_event(&client(&base), &body, None);
    assert_eq!(kind(&w), "effect_unknown", "{w:?}");
    assert!(w.why().contains("gcal-check --id"), "{}", w.why());
    assert_eq!(methods(&log), [format!("POST {EV}")]);
    // 5xx も作られたか分からない
    let (base, log) = fake(vec![("POST /calendar", 503, json!({"error": {"code": 503, "message": "Backend Error"}}))]);
    assert_eq!(kind(&calendar::create_event(&client(&base), &body, None)), "effect_unknown");
    assert_eq!(log.lock().unwrap().len(), 1);
    // 接続できない（届いていない）→ Failed{ConnectionLost}
    let closed = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://127.0.0.1:{}", l.local_addr().unwrap().port()) };
    assert_eq!(calendar::create_event(&client(&closed), &body, None).outcome, Outcome::Failed { error: ErrorCode::ConnectionLost });
    // 4xx は作られていない
    let (base, _) = fake(vec![("POST /calendar", 403, json!({"error": {"code": 403, "message": "Request had insufficient authentication scopes."}}))]);
    assert_eq!(calendar::create_event(&client(&base), &body, None).outcome, Outcome::Failed { error: ErrorCode::PermissionDenied });
    // 再試行で 409（前の試行が通っていた）→ 作り直さず読み戻して Verified
    let (base, log) = fake(vec![("POST /calendar", 409, json!({"error": {"code": 409, "message": "The requested identifier already exists."}})), ("GET /calendar", 200, event_echo("[Kangae test] 打合せ"))]);
    let w = calendar::create_event(&client(&base), &body, None);
    assert!(kind(&w) == "verified" && w.detail.contains("already existed"), "{w:?}");
    assert_eq!(methods(&log), [format!("POST {EV}"), format!("GET {EV}/{EID}")]);
    // 409 で中身が違う / 消された id → 使えない id（Failed）
    let (base, _) = fake(vec![("POST /calendar", 409, json!({})), ("GET /calendar", 200, json!({"id": EID, "status": "cancelled"}))]);
    assert_eq!(calendar::create_event(&client(&base), &body, None).outcome, Outcome::Failed { error: ErrorCode::InvalidInput });
    // 再照合: 無い = TargetNotFound（作られていない → 同じ id で作り直してよい）
    let (base, _) = fake(vec![("GET /calendar", 404, json!({}))]);
    assert_eq!(calendar::check_event(&client(&base), &body).outcome, Outcome::Failed { error: ErrorCode::TargetNotFound });
    // id が無い・kangae で始まらない本文は送らない
    let (base, log) = fake(vec![]);
    let mut no_id = body.clone();
    no_id["id"] = json!("abcde12345");
    assert_eq!(kind(&calendar::create_event(&client(&base), &no_id, None)), "rejected");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn invites_are_external_send_and_need_approval_of_the_exact_body() {
    let invited = event_body(&["a@x.io".to_string()]);
    let req = calendar::event_request("x", "2026-10-18", "2026-10-19", &["a@x.io".to_string()], "").unwrap();
    assert_eq!(req["effect"], "external_send");
    assert_eq!(calendar::event_request("x", "2026-10-18", "2026-10-19", &[], "").unwrap()["effect"], "external_write");
    let (base, log) = fake(vec![("POST /calendar", 200, json!({"id": EID})), ("GET /calendar", 200, json!({}))]);
    // 承認なし → HTTP 0 本で Rejected
    let w = calendar::create_event(&client(&base), &invited, None);
    assert!(kind(&w) == "rejected" && w.why().contains("invitation"), "{w:?}");
    // 別の本文への承認（参加者を 1 人足した）→ Rejected
    let other = event_body(&["a@x.io".to_string(), "b@x.io".to_string()]);
    assert_eq!(kind(&calendar::create_event(&client(&base), &invited, Some(&approve(&other)))), "rejected");
    // 拒否の記録 → Rejected
    let denied = ConsentReceipt { decision: ConsentDecision::Denied, ..approve(&invited) };
    assert_eq!(kind(&calendar::create_event(&client(&base), &invited, Some(&denied))), "rejected");
    assert!(log.lock().unwrap().is_empty(), "承認の無い招待は 1 本も送らない");
    // この本文への承認 → sendUpdates=all で POST（読み戻しの参加者が違うので Unverified）
    let w = calendar::create_event(&client(&base), &invited, Some(&approve(&invited)));
    assert_eq!(kind(&w), "unverified");
    assert!(log.lock().unwrap()[0].path.ends_with("?sendUpdates=all"));
}

#[test]
fn calendar_delete_only_kangae_ids_and_reads_back() {
    let (base, log) = fake(vec![("DELETE /calendar", 204, json!({})), ("GET /calendar", 200, json!({"id": EID, "status": "cancelled"}))]);
    let w = calendar::delete_event(&client(&base), "abc123someoneelse");
    assert_eq!(kind(&w), "rejected");
    assert!(log.lock().unwrap().is_empty());
    let w = calendar::delete_event(&client(&base), EID);
    assert_eq!(kind(&w), "verified", "{w:?}");
    assert_eq!(methods(&log), [format!("DELETE {EV}/{EID}"), format!("GET {EV}/{EID}")]);
    assert!(log.lock().unwrap()[0].path.ends_with("?sendUpdates=none"));
    // 消した後も読める → Unverified
    let (base, _) = fake(vec![("DELETE /calendar", 204, json!({})), ("GET /calendar", 200, event_echo("x"))]);
    assert_eq!(kind(&calendar::delete_event(&client(&base), EID)), "unverified");
}

const DR: &str = "/gmail/v1/users/me/drafts";
fn thread_meta() -> Value {
    json!({"id": "t1", "messages": [
        {"id": "m1", "labelIds": ["INBOX"], "payload": {"headers": [{"name": "Subject", "value": "Hello"}, {"name": "From", "value": "Alice <a@x.io>"},
            {"name": "Message-ID", "value": "<orig@x.io>"}, {"name": "References", "value": "<r0@x.io>"}]}},
        {"id": "m2", "labelIds": ["DRAFT"], "payload": {"headers": [{"name": "Subject", "value": "Re: Hello"}, {"name": "From", "value": "me@x.io"}]}}]})
}
fn draft_echo(body: &str, key: &str) -> Value {
    json!({"id": "d1", "message": {"id": "m9", "threadId": "t1", "payload": {"mimeType": "text/plain", "headers": [
        {"name": "To", "value": "me@x.io"}, {"name": "Subject", "value": "Re: Hello"}, {"name": "Message-Id", "value": "<CAx@mail.gmail.com>"}, {"name": "X-Kangae-Key", "value": key}],
        "body": {"data": crypto::base64(body.as_bytes(), true)}}}})
}

#[test]
fn reply_draft_threads_on_the_last_message_and_is_verified_by_drafts_get() {
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"resultSizeEstimate": 0})), ("/gmail/v1/users/me/threads/t1", 200, thread_meta()), ("POST /gmail", 200, json!({"id": "d1", "message": {"id": "m9", "threadId": "t1"}})),
        ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("了解です\r\nよろしく", "k1"))]);
    let c = client(&base);
    let d = gmail::reply_draft(&c, "t1", Some("me@x.io"), "了解です\nよろしく", "k1").unwrap();
    assert_eq!((d.subject.as_str(), d.in_reply_to.as_str(), d.references.as_str()), ("Re: Hello", "<orig@x.io>", "<r0@x.io> <orig@x.io>"), "下書きの message ではなく最後の受信に返す");
    assert_eq!(gmail::reply_draft(&c, "t1", None, "b", "k1").unwrap().to, "Alice <a@x.io>");
    let w = gmail::create_draft_checked(&c, &d);
    assert_eq!(kind(&w), "verified", "{w:?}");
    assert_eq!(w.id.as_deref(), Some("d1"));
    {
        let log = log.lock().unwrap();
        let post = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.path, DR);
        assert!(log.iter().all(|r| !r.path.contains("send")), "送信の endpoint は呼ばない");
        let sent: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(sent["message"]["threadId"], "t1");
        let raw = String::from_utf8(crypto::base64_decode(sent["message"]["raw"].as_str().unwrap()).unwrap()).unwrap();
        assert!(raw.starts_with("X-Kangae-Key: k1\r\nIn-Reply-To: <orig@x.io>\r\nReferences: <r0@x.io> <orig@x.io>\r\nTo: me@x.io\r\nSubject: Re: Hello\r\n"), "{raw}");
        assert!(raw.ends_with("\r\n\r\n了解です\r\nよろしく"), "{raw}");
    }
    // 読み戻した本文が違う → Unverified
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"resultSizeEstimate": 0})), ("POST /gmail", 200, json!({"id": "d1"})), ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("別の本文", "k1"))]);
    let w = gmail::create_draft_checked(&client(&base), &d);
    assert!(kind(&w) == "unverified" && w.why().contains("The text of the draft"), "{w:?}");
    // 読み戻しに印（X-Kangae-Key）が無い・違う → Unverified（Gmail が Message-ID を書き換えた時と同じ形: 2026-10-17 実測）
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"resultSizeEstimate": 0})), ("POST /gmail", 200, json!({"id": "d1"})), ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("了解です\r\nよろしく", "other"))]);
    let w = gmail::create_draft_checked(&client(&base), &d);
    assert!(kind(&w) == "unverified" && w.why().contains("X-Kangae-Key"), "{w:?}");
    // 読み戻しが 404 → Unverified（受理だけ）
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"resultSizeEstimate": 0})), ("POST /gmail", 200, json!({"id": "d1"}))]);
    assert_eq!(kind(&gmail::create_draft_checked(&client(&base), &d)), "unverified");
    // 同じ key の下書きが既に在る（同じ承認で 2 回打った）→ POST せずに Verified
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"drafts": [{"id": "d1"}]})), ("POST /gmail", 200, json!({"id": "d2"})),
        ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("了解です\r\nよろしく", "k1"))]);
    let w = gmail::create_draft_checked(&client(&base), &d);
    assert!(kind(&w) == "verified" && w.id.as_deref() == Some("d1") && w.detail.contains("既に在りました"), "{w:?}");
    assert!(log.lock().unwrap().iter().all(|r| r.method == "GET"), "作り直さない");
    // 在るか確かめられない（一覧が 503）→ 作らない
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts?", 503, json!({})), ("POST /gmail", 200, json!({"id": "d2"}))]);
    assert_eq!(kind(&gmail::create_draft_checked(&client(&base), &d)), "effect_unknown");
    assert!(log.lock().unwrap().iter().all(|r| r.method == "GET"));
    // 改行注入は request を出さない
    let (base, log) = fake(vec![]);
    let evil = gmail::Draft { in_reply_to: "<x@y>\r\nBcc: evil@x.io".into(), ..d.clone() };
    assert_eq!(kind(&gmail::create_draft_checked(&client(&base), &evil)), "rejected");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn draft_timeout_is_effect_unknown_and_recheck_finds_it_by_key_header() {
    let d = gmail::Draft { to: "me@x.io".into(), subject: "Re: Hello".into(), body: "b".into(), thread_id: "t1".into(), in_reply_to: String::new(), references: String::new(), key: "k2".into() };
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"resultSizeEstimate": 0})), ("POST /gmail", 0, json!({}))]);
    let w = gmail::create_draft_checked(&client(&base), &d);
    assert_eq!(kind(&w), "effect_unknown", "{w:?}");
    assert!(w.why().contains("gmail-draft-check --key k2"), "{}", w.why());
    assert_eq!(log.lock().unwrap().iter().filter(|r| r.method == "POST").count(), 1, "自動で再送しない");
    // 再照合: 新しい下書きに同じ key が無い → TargetNotFound（人の下書き d0 は key が違うので飛ばす）
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"drafts": [{"id": "d0"}]})), ("GET /gmail/v1/users/me/drafts/d0", 200, draft_echo("b", "other"))]);
    assert_eq!(gmail::check_draft_by_key(&client(&base), &d).outcome, Outcome::Failed { error: ErrorCode::TargetNotFound });
    assert!(log.lock().unwrap()[0].path.contains("q=in%3Adrafts%20newer_than%3A2d"), "{}", log.lock().unwrap()[0].path);
    // 在る → 読み戻して Verified（d0 を飛ばして d1）
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"drafts": [{"id": "d0"}, {"id": "d1"}]})),
        ("GET /gmail/v1/users/me/drafts/d0", 200, draft_echo("b", "other")), ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("b", "k2"))]);
    let w = gmail::check_draft_by_key(&client(&base), &d);
    assert!(kind(&w) == "verified" && w.id.as_deref() == Some("d1"), "{w:?}");
    // 同じ key だが本文が違う → Unverified
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/drafts?", 200, json!({"drafts": [{"id": "d1"}]})), ("GET /gmail/v1/users/me/drafts/d1", 200, draft_echo("違う", "k2"))]);
    assert_eq!(kind(&gmail::check_draft_by_key(&client(&base), &d)), "unverified");
}

#[test]
fn draft_delete_only_kangae_drafts_and_reads_back() {
    let mine = json!({"id": "d1", "message": {"payload": {"headers": [{"name": "Message-Id", "value": "<CAx@mail.gmail.com>"}, {"name": "X-Kangae-Key", "value": "k1"}]}}});
    let theirs = json!({"id": "d2", "message": {"payload": {"headers": [{"name": "Message-ID", "value": "<abc@mail.gmail.com>"}]}}});
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/drafts/d2?format=metadata", 200, theirs), ("GET /gmail/v1/users/me/drafts/d1?format=metadata", 200, mine),
        ("DELETE /gmail", 204, json!({})), ("GET /gmail/v1/users/me/drafts/d1", 404, json!({"error": {"code": 404, "message": "Requested entity was not found."}}))]);
    assert_eq!(kind(&gmail::delete_draft(&client(&base), "d2")), "rejected");
    assert_eq!(methods(&log), [format!("GET {DR}/d2")], "人の下書きには DELETE を打たない");
    let w = gmail::delete_draft(&client(&base), "d1");
    assert_eq!(kind(&w), "verified", "{w:?}");
    assert_eq!(methods(&log)[1..], [format!("GET {DR}/d1"), format!("DELETE {DR}/d1"), format!("GET {DR}/d1")]);
}

// ── 営業メールの一括（contacts / outreach）。送るのは偽 Gmail だけ（本物の send は 0 回）

/// LinkedIn の Connections.csv の形（先頭の Notes: の注意書き・引用符の中の改行と , と ""）。列の順は一次資料に無いので並べ替えて渡す
const LINKEDIN: &str = "\u{feff}Notes:\r\n\"When exporting your connection data, you may notice that some of the email addresses are missing.\r\nYou will only see email addresses for connections who have allowed their connections to see or download their email address.\"\r\n\r\nFirst Name,Last Name,URL,Email Address,Company,Position,Connected On\r\nAda,Lovelace,https://www.linkedin.com/in/ada,Ada@Example.com,Analytical Engines,\"Founder, CEO\",16 Oct 2026\r\nGrace,Hopper,https://www.linkedin.com/in/grace,,Navy,\"Engineer \"\"COBOL\"\"\",01 Jan 2020\r\n太郎,山田,https://www.linkedin.com/in/yamada,not-an-address,山田技研,CTO,02 Feb 2021\r\n";

#[test]
fn linkedin_csv_skips_the_notes_and_reads_columns_by_name() {
    let rows = contacts::from_linkedin(LINKEDIN, "LinkedIn").unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!((rows[0].name.as_str(), rows[0].first_name.as_str(), rows[0].company.as_str(), rows[0].title.as_str(), rows[0].mail.as_str()),
        ("Ada Lovelace", "Ada", "Analytical Engines", "Founder, CEO", "Ada@Example.com"));
    assert_eq!(rows[0].url, "https://www.linkedin.com/in/ada");
    assert_eq!((rows[1].mail.as_str(), rows[1].title.as_str()), ("", "Engineer \"COBOL\""), "mail を出していない人は空（推測しない）");
    assert_eq!((rows[2].name.as_str(), rows[2].mail.as_str()), ("山田 太郎", ""), "漢字の名前は姓が先・mail の形でない物は空");
    // 列の順が違っても名前で引く
    let shuffled = "Email Address,Position,First Name,Company,Last Name\nb@x.io,CTO,Bob,Acme,Smith\n";
    let r = contacts::from_linkedin(shuffled, "L").unwrap();
    assert_eq!((r[0].name.as_str(), r[0].mail.as_str(), r[0].company.as_str(), r[0].title.as_str()), ("Bob Smith", "b@x.io", "Acme", "CTO"));
    assert!(contacts::from_linkedin("Name,Email\nA,a@x.io\n", "L").unwrap_err().contains("First Name"), "見出しが無ければ表を作らない");
}

#[test]
fn paste_reads_tsv_csv_and_name_mail_lists_and_round_trips_the_table() {
    let tsv = contacts::from_paste("名前\tメール\t会社\t役職\n田中 花子\thanako@x.io\t花子商店\t代表\n", "貼り付け").unwrap();
    assert_eq!((tsv[0].name.as_str(), tsv[0].last_name.as_str(), tsv[0].first_name.as_str(), tsv[0].mail.as_str(), tsv[0].company.as_str()), ("田中 花子", "田中", "花子", "hanako@x.io", "花子商店"));
    let csv = contacts::from_paste("Carol King,carol@x.io,Tapestry\n", "p").unwrap();
    assert_eq!((csv[0].name.as_str(), csv[0].mail.as_str(), csv[0].company.as_str()), ("Carol King", "carol@x.io", "Tapestry"), "見出しの無い CSV は @ の cell が mail・残りが名前と会社");
    let list = contacts::from_paste("\"Doe, Jane\" <jane@x.io>, Bob <bob@x.io>; solo@x.io\nbad <nope>\n", "p").unwrap();
    let got: Vec<(&str, &str)> = list.iter().map(|c| (c.name.as_str(), c.mail.as_str())).collect();
    assert_eq!(got, [("Doe, Jane", "jane@x.io"), ("Bob", "bob@x.io"), ("", "solo@x.io"), ("bad", "")]);
    assert_eq!((list[0].first_name.as_str(), list[0].last_name.as_str()), ("Jane", "Doe"), "「姓, 名」の並び");
    assert_eq!(contacts::split_name("山田 太郎"), ("太郎".to_string(), "山田".to_string()));
    assert_eq!(contacts::split_name("山田太郎"), (String::new(), String::new()), "空白の無い漢字の名前は分けない（推測しない）");
    // 宛先表（to_tsv）を読み戻すと同じ物（出典も表の物を使う・「mail なし」は空に戻る）
    let rows = contacts::from_linkedin(LINKEDIN, "LinkedIn").unwrap();
    let table = contacts::to_tsv(&rows);
    assert!(table.lines().nth(2).unwrap().contains("\tNo email\t"), "{table}");
    assert_eq!(contacts::from_paste(&table, "別の出典").unwrap(), rows);
    assert!(contacts::from_paste("  \n", "p").is_err());
}

#[test]
fn yc_list_keeps_only_company_site_batch_and_merge_dedupes_by_mail() {
    // site-api の yc-companies の CSV（列は recipe の columns の一部）
    let yc = "name,batch,status,one_liner,website,slug\nAirbnb,Winter 2009,Public,\"Book, stay\",https://airbnb.com,airbnb\nStripe,Summer 2009,Active,Payments,https://stripe.com,stripe\n";
    let r = contacts::from_yc(yc, "YC").unwrap();
    assert_eq!(r[0], Contact { company: "Airbnb".into(), url: "https://airbnb.com".into(), batch: "Winter 2009".into(), source: "YC".into(), ..Default::default() }, "会社名・site・batch だけ（人も mail も作らない）");
    let json_hits = r#"{"hits":[{"name":"Dropbox","website":"https://dropbox.com","batch":"Summer 2007","founders":[{"email":"x@dropbox.com"}]}]}"#;
    let j = contacts::from_yc(json_hits, "YC").unwrap();
    assert_eq!((j[0].company.as_str(), j[0].mail.as_str()), ("Dropbox", ""), "JSON に何が在っても mail は取らない");
    assert!(contacts::from_yc("company,url\nA,b\n", "YC").is_err());
    // handoff: yc-open で開いた directory から本人が選んだ会社の貼り付け（1 行 1 社・tab か空白・会社 page の URL だけでも可）
    let pasted = contacts::from_yc("Open Door Labs https://odl.test W21\nBeta\thttps://beta.test\tSummer 2024\nhttps://www.ycombinator.com/companies/gamma-ai\n\nW09\n", "YC").unwrap();
    let got: Vec<(&str, &str, &str)> = pasted.iter().map(|c| (c.company.as_str(), c.url.as_str(), c.batch.as_str())).collect();
    assert_eq!(got, [("Open Door Labs", "https://odl.test", "W21"), ("Beta", "https://beta.test", "Summer 2024"), ("gamma-ai", "https://www.ycombinator.com/companies/gamma-ai", "")], "batch だけの行は会社にしない");
    assert!(pasted.iter().all(|c| c.mail.is_empty() && c.name.is_empty()));
    // 同じ mail（大文字小文字違い）は 1 行に: 空の欄を埋め・出典を足す。mail なしは全部同じ行だけまとめる
    let a = vec![Contact { name: "Ada Lovelace".into(), mail: "ada@example.com".into(), source: "LinkedIn".into(), ..Default::default() }];
    let b = vec![Contact { name: "Ada".into(), mail: "ADA@example.com".into(), company: "AE".into(), source: "貼り付け".into(), ..Default::default() }];
    let (rows, merged) = contacts::merge(vec![a, b, r.clone(), r]);
    assert_eq!((rows.len(), merged), (3, 3), "{rows:?}");
    assert_eq!((rows[0].name.as_str(), rows[0].company.as_str(), rows[0].source.as_str()), ("Ada Lovelace", "AE", "LinkedIn + 貼り付け"));
}

/// CLI（配る形）: YC の既定は handoff = 本人の貼り付けだけ読み、YC へ HTTP 0 本。--yc-fetch の opt-in の時だけ取りに行く（全頁・totalPages で止まる）
#[test]
fn yc_import_sends_no_http_to_yc_without_the_opt_in() {
    let (base, log) = fake(vec![
        ("/v0.1/companies?page=1", 200, json!({"companies": [{"name": "Acme", "website": "https://acme.test", "batch": "Winter 2021"}], "page": 1, "totalPages": 2})),
        ("/v0.1/companies?page=2", 200, json!({"companies": [{"name": "Zed", "website": "https://zed.test", "batch": "Summer 2022"}], "page": 2, "totalPages": 2})),
    ]);
    let dir = std::env::temp_dir().join(format!("kangae-yc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let paste = dir.join("yc.txt");
    std::fs::write(&paste, "Beta https://beta.test S24\n").unwrap();
    let paste = paste.to_str().unwrap();
    let run = |args: &[&str]| std::process::Command::new(env!("CARGO_BIN_EXE_kangae-google")).args(args).env("KANGAE_YC_API_BASE", &base).output().unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    let out = run(&["outreach-import", "--yc", paste]);
    assert!(out.status.success() && text(&out.stdout).contains("\tBeta\t") && !text(&out.stdout).contains("Acme"), "{}{}", text(&out.stdout), text(&out.stderr));
    let none = run(&["outreach-import"]);
    assert!(!none.status.success() && text(&none.stderr).contains("yc-open"), "{}", text(&none.stderr));
    assert_eq!(log.lock().unwrap().len(), 0, "opt-in 無しでは YC へ 1 本も打たない");
    let fetched = run(&["outreach-import", "--yc-fetch", "--yc", paste]);
    let stdout = text(&fetched.stdout);
    assert!(fetched.status.success() && ["\tAcme\t", "\tZed\t", "\tBeta\t"].iter().all(|c| stdout.contains(c)), "{stdout}{}", text(&fetched.stderr));
    assert!(text(&fetched.stderr).contains("利用規約"), "opt-in の時は規約に当たる事を言う");
    let paths: Vec<String> = log.lock().unwrap().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, ["/v0.1/companies?page=1", "/v0.1/companies?page=2"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 試験用の架空の送り手（本物の住所は ~/.kangae/sender.json にだけ在る）
fn sender() -> outreach::Sender { outreach::Sender { name: "山田 太郎".into(), address: "架空県架空市 1-2-3（試験用）".into(), contact: "optout@example.com".into() } }
const TEMPLATE: &str = "件名: {company|貴社} の {first_name} さんへ\n\n{first_name} さん\n\n{company|貴社} の {title} として…\n\n山田 太郎\n{sender_address}\n問合せ: {sender_contact}\n今後このようなご連絡が不要でしたら、このメールにご返信ください。\n";
fn people() -> Vec<Contact> {
    let p = |name: &str, mail: &str, company: &str, title: &str| Contact { name: name.into(), mail: mail.into(), company: company.into(), title: title.into(), source: "t".into(), ..Default::default() };
    let mut v: Vec<Contact> = vec![p("Ada Lovelace", "ada@x.io", "AE", "Founder"), p("Bob Smith", "bob@x.io", "", "CTO"), p("Carol King", "", "Tap", "CEO"),
        p("Dan Brown", "dan@x.io", "Codex", ""), p("Ada Again", "ADA@x.io", "AE", "CEO"), p("Eve Adams", "eve@x.io", "Sec\nInc", "Lead")];
    for c in &mut v { (c.first_name, c.last_name) = contacts::split_name(&c.name) }
    v
}

#[test]
fn template_errors_stop_before_any_draft_and_rows_are_filled_per_person() {
    let t = outreach::parse_template(TEMPLATE).unwrap();
    let p = outreach::plan(&people(), &t, &sender(), 20).unwrap();
    let got: Vec<(&str, &str)> = p.drafts.iter().map(|d| (d.to.as_str(), d.subject.as_str())).collect();
    assert_eq!(got, [("ada@x.io", "AE の Ada さんへ"), ("bob@x.io", "貴社 の Bob さんへ"), ("eve@x.io", "Sec Inc の Eve さんへ")], "空の会社は {{company|貴社}}・値の改行は空白");
    assert!(p.drafts[0].body.starts_with("Ada さん\n\nAE の Founder として"), "{}", p.drafts[0].body);
    let skipped: Vec<(&str, &str)> = p.skipped.iter().map(|s| (s.who.as_str(), s.reason.as_str())).collect();
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    assert_eq!(skipped[0], ("Carol King", "No email"));
    assert!(skipped[1].0 == "Dan Brown" && skipped[1].1.contains("title is empty"), "{skipped:?}");
    assert!(skipped[2].0 == "Ada Again" && skipped[2].1.contains("same recipient"), "{skipped:?}");
    assert_ne!(p.drafts[0].key, p.drafts[1].key);
    // 未定義の欄は全部挙げて template ごと誤り（下書き 0 件）
    let bad = outreach::Template { subject: "{frist_name} さん".into(), body: t.body.replace("{title}", "{role}") };
    let e = outreach::plan(&people(), &bad, &sender(), 20).unwrap_err();
    assert!(e.contains("{frist_name}") && e.contains("{role}") && e.contains("No draft was created"), "{e}");
    // footer: 送り手の名前と「今後不要なら返信で」の 1 行の両方が要る
    let no_name = outreach::Template { subject: t.subject.clone(), body: t.body.replace("山田 太郎\n", "") };
    assert!(outreach::plan(&people(), &no_name, &sender(), 20).unwrap_err().contains("footer"));
    let no_opt_out = outreach::Template { subject: t.subject.clone(), body: t.body.replace("今後このようなご連絡が不要でしたら、このメールにご返信ください。", "よろしくお願いします。") };
    assert!(outreach::plan(&people(), &no_opt_out, &sender(), 20).unwrap_err().contains("footer"));
    let sender_field = outreach::Template { subject: t.subject.clone(), body: t.body.replace("山田 太郎\n", "{sender_name}\n") };
    assert!(outreach::plan(&people(), &sender_field, &sender(), 20).is_ok(), "{{sender_name}} でも footer の名前になる");
    assert!(outreach::plan(&people(), &t, &outreach::Sender { name: " ".into(), ..sender() }, 20).is_err(), "送り手の名前が無い");
    // 法令の表示（特定電子メール法 4 条・施行規則 9 条 / CAN-SPAM）: 住所の欄・連絡先の欄が無い template と、値の無い送り手は下書き 0 件
    let no_address = outreach::Template { subject: t.subject.clone(), body: t.body.replace("{sender_address}\n", "") };
    let e = outreach::plan(&people(), &no_address, &sender(), 20).unwrap_err();
    assert!(e.contains("The address") && !e.contains("The contact for opt-outs") && e.contains("in the text. No draft was created"), "{e}");
    let no_contact = outreach::Template { subject: t.subject.clone(), body: t.body.replace("問合せ: {sender_contact}\n", "") };
    assert!(outreach::plan(&people(), &no_contact, &sender(), 20).unwrap_err().contains("The contact for opt-outs"));
    let e = outreach::plan(&people(), &t, &outreach::Sender { address: String::new(), ..sender() }, 20).unwrap_err();
    assert!(e.contains("address") && e.contains("sender.json"), "{e}");
    assert!(outreach::plan(&people(), &t, &outreach::Sender { contact: "03-0000-0000".into(), ..sender() }, 20).unwrap_err().contains("contact"), "電話だけでは受信拒否の通知先にならない");
    assert!(outreach::plan(&people(), &t, &outreach::Sender { contact: "https://example.com/optout".into(), ..sender() }, 20).is_ok(), "URL は可");
    // 件名の偽り: 新しい mail を返信・転送に見せかける頭は止める（全角の ： も）
    for fake_reply in ["Re: ご相談", "RE：ご相談", "Fwd: 資料", "返信: ご相談"] {
        let e = outreach::plan(&people(), &outreach::Template { subject: fake_reply.into(), body: t.body.clone() }, &sender(), 20).unwrap_err();
        assert!(e.contains("a reply or a forward") && e.starts_with("In the template, the subject “"), "{fake_reply}: {e}");
    }
    assert!(outreach::plan(&people(), &outreach::Template { subject: "Regarding {company|貴社}".into(), body: t.body.clone() }, &sender(), 20).is_ok());
    assert!(outreach::plan(&people(), &t, &sender(), 0).is_err() && outreach::plan(&people(), &t, &sender(), 501).is_err(), "上限は 1〜500");
    assert!(outreach::parse_template("本文だけ").is_err() && outreach::parse_template("件名: x\n{oops\n").is_ok());
    assert!(outreach::plan(&people(), &outreach::parse_template("件名: x\n{oops\n山田 太郎\n不要なら返信で").unwrap(), &sender(), 20).unwrap_err().contains("isn't closed"));
}

/// 偽 Gmail の送信済み 1 通
struct Mail { id: String, to: String, subject: String, key: String, body: String }
fn mail_json(m: &Mail, strip_key: bool) -> Value {
    let mut headers = vec![json!({"name": "To", "value": m.to}), json!({"name": "Subject", "value": m.subject})];
    if !m.key.is_empty() && !strip_key { headers.push(json!({"name": "X-Kangae-Key", "value": m.key})) }
    json!({"id": m.id, "threadId": format!("t-{}", m.id), "labelIds": ["SENT"], "payload": {"mimeType": "text/plain", "headers": headers, "body": {"data": crypto::base64(m.body.as_bytes(), true)}}})
}
/// 状態を持つ偽 Gmail。seed = 既に送信済みの (宛先, X-Kangae-Key。空 = 人が手で送った物)。messages.send は raw を読んで送信済みに足し、
/// messages.list(q) は to: で絞って新しい順、messages.get は保存した header と本文を返す。fail = (n 回目の send, status。0 は応答せず切る)、strip_key = 読み戻しで印を落とす
fn fake_gmail(seed: &[(&str, &str)], fail: Option<(usize, u16)>, strip_key: bool) -> (String, Arc<Mutex<Vec<Req>>>) {
    let store: Mutex<Vec<Mail>> = Mutex::new(seed.iter().enumerate().map(|(i, (to, key))| Mail { id: format!("s{i}"), to: to.to_string(), subject: "old".into(), key: key.to_string(), body: "old".into() }).collect());
    let sends = AtomicUsize::new(0);
    fake_with(move |req| {
        let (path, query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
        let mut store = store.lock().unwrap();
        if req.method == "POST" && path == "/gmail/v1/users/me/messages/send" {
            let n = sends.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some((at, status)) = fail { if n == at { return (status, json!({"error": {"code": status, "message": "backend"}}).to_string()) } }
            let v: Value = serde_json::from_str(&req.body).unwrap();
            let raw = String::from_utf8(crypto::base64_decode(v["raw"].as_str().unwrap()).unwrap()).unwrap();
            let (head, body) = raw.split_once("\r\n\r\n").unwrap();
            let h = |k: &str| head.split("\r\n").find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap_or("").to_string();
            let id = format!("m{n}");
            store.push(Mail { id: id.clone(), to: h("To"), subject: h("Subject"), key: h("X-Kangae-Key"), body: body.replace("\r\n", "\n") });
            return (200, json!({"id": id, "threadId": format!("t-{id}"), "labelIds": ["SENT"]}).to_string());
        }
        if req.method == "GET" && path == "/gmail/v1/users/me/messages" {
            let q = google::urldecode(query.split('&').find_map(|kv| kv.strip_prefix("q=")).unwrap_or(""));
            let to = q.split_whitespace().find_map(|w| w.strip_prefix("to:")).map(str::to_string);
            let list: Vec<Value> = store.iter().rev().filter(|m| to.as_deref().is_none_or(|t| m.to.eq_ignore_ascii_case(t))).map(|m| json!({"id": m.id, "threadId": format!("t-{}", m.id)})).collect();
            return (200, json!({"messages": list, "resultSizeEstimate": list.len()}).to_string());
        }
        if let Some(id) = path.strip_prefix("/gmail/v1/users/me/threads/t-") {
            return store.iter().find(|m| m.id == id).map(|m| { let mut j = mail_json(m, strip_key); j["internalDate"] = json!("1"); (200, json!({"id": format!("t-{id}"), "messages": [j]}).to_string()) })
                .unwrap_or((404, json!({"error": {"code": 404, "message": "Requested entity was not found."}}).to_string()));
        }
        if req.method == "GET" { if let Some(id) = path.strip_prefix("/gmail/v1/users/me/messages/") {
            return store.iter().find(|m| m.id == id).map(|m| (200, mail_json(m, strip_key).to_string())).unwrap_or((404, json!({"error": {"code": 404, "message": "Requested entity was not found."}}).to_string()));
        } }
        (404, "{}".to_string())
    })
}
fn sends(log: &Arc<Mutex<Vec<Req>>>) -> usize { log.lock().unwrap().iter().filter(|r| r.method == "POST" && r.path.contains("/send")).count() }
fn consent_for(dg: &str) -> ConsentReceipt {
    ConsentReceipt { id: "c".into(), app_id: "a".into(), subject: ConsentSubject::Action { action_ref: "gmail.messages.send".into(), run_id: None },
        scopes: vec![], payload_digest: Some(dg.into()), decision: ConsentDecision::Approved, decided_at: "2026-10-17T00:00:00Z".into() }
}
/// 反証 UI-2c: footer の誤りが例に出す受信拒否の 1 行は、そのまま写せば footer の門を通る（写した利用者が同じ誤りで止まり続けない）
#[test]
fn the_opt_out_example_in_the_footer_error_passes_the_footer_check() {
    let s = sender();
    let body = format!("Hi\n\n{}\n{}\n{}\n", s.name, s.address, s.contact);
    let e = outreach::footer_problem(&body, &s).expect("受信拒否の 1 行が無い");
    let example = e.split_once("such as “").and_then(|(_, r)| r.split_once('”')).map(|(x, _)| x.to_owned()).unwrap_or_else(|| panic!("例が無い: {e}"));
    assert_eq!(outreach::footer_problem(&format!("{body}{example}\n"), &s), None, "{example}");
}
fn plan4() -> outreach::Plan {
    let mut rows: Vec<Contact> = ["a", "b", "c", "d"].iter().map(|n| Contact { name: format!("{n} Person"), mail: format!("{n}@x.io"), company: "Co".into(), title: "CEO".into(), ..Default::default() }).collect();
    for c in &mut rows { (c.first_name, c.last_name) = contacts::split_name(&c.name) }
    outreach::plan(&rows, &outreach::parse_template(TEMPLATE).unwrap(), &sender(), 20).unwrap()
}
const NOW: u64 = 1_800_000_000;

#[test]
fn bulk_send_needs_one_approval_of_the_exact_bundle_or_sends_nothing() {
    let p = plan4();
    let (base, log) = fake_gmail(&[], None, false);
    let c = client(&base);
    // 承認なし・下書き用の digest・他の plan の digest → Rejected、HTTP 0 本（数える読みも打たない）
    for consent in [None, Some(consent_for(&outreach::drafts_digest(&p))), Some(consent_for(&outreach::send_digest(&plan4())))] {
        let r = outreach::send_plan(&c, &p, consent.as_ref(), NOW);
        let st = r.stopped.as_ref().unwrap();
        assert!(kind(&st.result) == "rejected" && r.done.is_empty() && r.remaining == 4, "{r:?}");
    }
    assert!(log.lock().unwrap().is_empty(), "承認が無ければ 1 本も出さない");
    // 承認の後に plan を書き換えた（宛先・本文・順序）→ digest が合わない
    let dg = outreach::send_digest(&p);
    let mut swapped = p.clone();
    swapped.drafts.swap(0, 1);
    let mut edited = p.clone();
    edited.drafts[2].body.push_str("\n追記");
    for q in [swapped, edited] { assert_eq!(kind(&outreach::send_plan(&c, &q, Some(&consent_for(&dg)), NOW).stopped.unwrap().result), "rejected") }
    // digest ごと作り直しても footer の無い下書き・同じ宛先は送らない
    let mut no_footer = p.clone();
    no_footer.drafts[1].body = "footer の無い本文".into();
    let mut dup = p.clone();
    dup.drafts[3].to = "A@x.io".into();
    let mut no_address = p.clone();
    no_address.drafts[0].body = no_address.drafts[0].body.replace("架空県架空市 1-2-3（試験用）\n", "");
    let mut fake_re = p.clone();
    fake_re.drafts[2].subject = format!("Re: {}", fake_re.drafts[2].subject);
    let mut no_sender_address = p.clone();
    no_sender_address.sender.address.clear();
    for q in [no_footer, dup, no_address, fake_re, no_sender_address] {
        let st = outreach::send_plan(&c, &q, Some(&consent_for(&outreach::send_digest(&q))), NOW).stopped.unwrap();
        assert!(kind(&st.result) == "rejected" && st.result.why().ends_with(". Nothing was sent"), "{}", st.result.why());
        let st = outreach::create_drafts(&c, &q, Some(&consent_for(&outreach::drafts_digest(&q)))).stopped.unwrap();
        assert!(kind(&st.result) == "rejected" && st.result.why().ends_with(". No draft was created"), "{}", st.result.why());
    }
    assert!(log.lock().unwrap().is_empty());
    // 前の版の plan（sender_name だけ・住所なし）は作り直しを求める
    let old_plan: outreach::Plan = serde_json::from_value(json!({"sender_name": "山田 太郎", "daily_limit": 20, "drafts": p.drafts})).unwrap();
    let e = outreach::check_plan(&old_plan).unwrap_err();
    assert!(e.contains("(the name can also be given with --sender). Rebuild the plan"), "{e}");
    // 正しい承認 → 4 件送って全部読み戻しで Verified
    let r = outreach::send_plan(&c, &p, Some(&consent_for(&dg)), NOW);
    assert!(r.finished() && r.done.len() == 4 && r.remaining == 0 && r.sent_24h_before == Some(0), "{r:?}");
    assert!(r.done.iter().all(|s| kind(&s.result) == "verified"));
    assert_eq!(sends(&log), 4);
    assert!(r.summary("Sent").starts_with("Sent 4 · skipped 0 · 0 left"), "{}", r.summary("Sent"));
    {
        let log = log.lock().unwrap();
        let count = log.iter().find(|r| r.path.starts_with("/gmail/v1/users/me/messages?")).unwrap();
        assert!(count.path.contains(&format!("q=in%3Asent%20after%3A{}", NOW - 86_400)), "24 時間は秒で渡す: {}", count.path);
        let raw: Value = serde_json::from_str(&log.iter().find(|r| r.method == "POST").unwrap().body).unwrap();
        let raw = String::from_utf8(crypto::base64_decode(raw["raw"].as_str().unwrap()).unwrap()).unwrap();
        assert!(raw.starts_with(&format!("X-Kangae-Key: {}\r\nTo: a@x.io\r\n", p.drafts[0].key)), "{raw}");
    }
    // 同じ承認でもう一度 → 全部「既に送ってあります」で飛ばす（2 回送らない）
    let again = outreach::send_plan(&c, &p, Some(&consent_for(&dg)), NOW);
    assert!(again.finished() && again.done.is_empty() && again.skipped.len() == 4 && again.skipped[0].reason.contains("Already sent with this approval"), "{again:?}");
    assert_eq!(sends(&log), 4);
    // 下書きの一括: 送信用の承認では作らない
    let (base, log) = fake(vec![]);
    let r = outreach::create_drafts(&client(&base), &p, Some(&consent_for(&dg)));
    assert_eq!(kind(&r.stopped.unwrap().result), "rejected");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn bulk_send_stops_at_the_first_failure_and_says_how_many_went_and_remain() {
    let p = plan4();
    let ok = consent_for(&outreach::send_digest(&p));
    // 3 件目が 500 → 2 件送った・3 件目は分からない（再送しない）・残り 1
    let (base, log) = fake_gmail(&[], Some((3, 500)), false);
    let r = outreach::send_plan(&client(&base), &p, Some(&ok), NOW);
    let st = r.stopped.as_ref().unwrap();
    assert!(r.done.len() == 2 && st.to == "c@x.io" && kind(&st.result) == "effect_unknown" && r.remaining == 1, "{r:?}");
    assert!(st.result.why().contains("It won't be sent again on its own"), "{}", st.result.why());
    assert_eq!(sends(&log), 3, "止まった後は打たない");
    assert!(r.summary("Sent").contains("Sent 2 · skipped 0 · 1 left. Stopped at c@x.io: "), "{}", r.summary("Sent"));
    // 応答の前に切れた（timeout と同じ）→ EffectUnknown。4xx → Failed（送られていない）
    let (base, _) = fake_gmail(&[], Some((1, 0)), false);
    assert_eq!(kind(&outreach::send_plan(&client(&base), &p, Some(&ok), NOW).stopped.unwrap().result), "effect_unknown");
    let (base, log) = fake_gmail(&[], Some((2, 400)), false);
    let r = outreach::send_plan(&client(&base), &p, Some(&ok), NOW);
    assert!(r.done.len() == 1 && kind(&r.stopped.as_ref().unwrap().result) == "failed" && r.remaining == 2, "{r:?}");
    assert_eq!(sends(&log), 2);
    // 読み戻しに X-Kangae-Key が無い（Gmail が印を落とした）→ Unverified で 1 件目の後に止まる（印が無いと次の run で 2 回目を送ってしまう）
    let (base, log) = fake_gmail(&[], None, true);
    let r = outreach::send_plan(&client(&base), &p, Some(&ok), NOW);
    let st = r.stopped.as_ref().unwrap();
    assert!(r.done.is_empty() && kind(&st.result) == "unverified" && st.result.why().contains("X-Kangae-Key") && r.remaining == 3, "{r:?}");
    assert_eq!(sends(&log), 1);
    // 送る前の数えが読めない → 1 件も送らない
    let (base, log) = fake(vec![("GET /gmail/v1/users/me/messages?", 503, json!({}))]);
    let r = outreach::send_plan(&client(&base), &p, Some(&ok), NOW);
    let st = r.stopped.as_ref().unwrap();
    assert!(r.done.is_empty() && r.remaining == 4 && kind(&st.result) == "effect_unknown", "{r:?}");
    // 何を確かめられなかったかが先・Google の失敗の文が後（反証 UI-2c: 「Can't check <失敗の文><何を>」と繋がっていた）
    assert!(st.result.why().starts_with("Can't check the number Shibubu sent in 24 hours (the daily limit): "), "{}", st.result.why());
    assert!(r.summary("Sent").contains("left. Reason it stopped: Can't check the number"), "{}", r.summary("Sent"));
    assert_eq!(sends(&log), 0);
}

#[test]
fn daily_limit_counts_only_kangae_mail_and_the_same_address_is_never_sent_twice() {
    let mut p = plan4();
    p.daily_limit = 3;
    let ok = consent_for(&outreach::send_digest(&p));
    // 送信済み: Kangae の 2 通（1 通は a@x.io = この plan の宛先）と、人が手で送った 3 通（印なし = 数えない）
    let seed = [("a@x.io", "old-key"), ("z@x.io", "k-z"), ("h1@x.io", ""), ("h2@x.io", ""), ("h3@x.io", "")];
    let (base, log) = fake_gmail(&seed, None, false);
    let r = outreach::send_plan(&client(&base), &p, Some(&ok), NOW);
    assert_eq!(r.sent_24h_before, Some(2), "{r:?}");
    assert!(r.skipped.len() == 1 && r.skipped[0].who == "a@x.io" && r.skipped[0].reason.contains("never sends twice"), "{r:?}");
    assert_eq!(r.done.iter().map(|s| s.to.as_str()).collect::<Vec<_>>(), ["b@x.io"], "Kangae の 2 + この run の 1 = 上限 3");
    let st = r.stopped.as_ref().unwrap();
    assert!(st.to.is_empty() && kind(&st.result) == "rejected" && st.result.why().contains("The daily limit of 3 was reached") && r.remaining == 2, "{r:?}");
    assert_eq!(sends(&log), 1);
    // 人が手で送った宛先（印なし）は「Kangae が送った」に数えない → 送る
    let mut q = plan4();
    q.drafts.truncate(1);
    q.drafts[0].to = "h1@x.io".into();
    let (base, log) = fake_gmail(&seed, None, false);
    let r = outreach::send_plan(&client(&base), &q, Some(&consent_for(&outreach::send_digest(&q))), NOW);
    assert!(r.finished() && r.done.len() == 1, "{r:?}");
    assert_eq!(sends(&log), 1);
    // 送る前の見せる段の数え（read だけ）
    let (base, log) = fake_gmail(&seed, None, false);
    assert_eq!(outreach::sent_24h(&client(&base), NOW, 20).unwrap(), 2);
    assert_eq!(sends(&log), 0);
}

#[test]
fn replies_are_messages_from_others_after_the_one_kangae_sent() {
    let text = |id: &str, labels: &[&str], ms: u64, from: &str, body: &str| json!({"id": id, "internalDate": ms.to_string(), "labelIds": labels, "snippet": format!("snip {id}"),
        "payload": {"mimeType": "multipart/alternative", "headers": [{"name": "From", "value": from}, {"name": "Date", "value": "Sat, 17 Oct 2026 10:00:00 +0900"}],
            "parts": [{"mimeType": "text/plain", "headers": [{"name": "Content-Type", "value": "text/plain; charset=\"UTF-8\""}], "body": {"data": crypto::base64(body.as_bytes(), true)}},
                      {"mimeType": "text/html", "body": {"data": crypto::base64(b"<p>x</p>", true)}}]}});
    let jis = json!({"id": "m6", "internalDate": "600", "labelIds": ["INBOX"], "snippet": "ありがとうございます、ぜひ",
        "payload": {"mimeType": "text/plain", "headers": [{"name": "From", "value": "Ada <a@x.io>"}, {"name": "Content-Type", "value": "text/plain; charset=ISO-2022-JP"}], "body": {"data": "GyRCJCIbKEI"}}});
    // 順序は internalDate で並べ直す（応答の順に頼らない）
    let thread = json!({"id": "t1", "messages": [jis, text("m4", &["SENT"], 400, "me", "追伸"), text("m0", &["INBOX"], 100, "Ada <a@x.io>", "前の話"),
        text("m1", &["SENT"], 200, "me", "はじめまして"), text("m3", &["DRAFT"], 350, "me", "書きかけ"),
        text("m2", &["INBOX", "UNREAD"], 300, "Ada <a@x.io>", "興味があります。\n来週どうですか\n\nOn Sat, Oct 17, 2026 at 10:00 AM 山田 太郎 wrote:\n> はじめまして")]});
    let (base, _) = fake(vec![("/gmail/v1/users/me/threads/t1", 200, thread)]);
    let r = gmail::thread_replies(&client(&base), "t1", Some("m1")).unwrap();
    assert_eq!(r.count, 2, "m1 より後で SENT・DRAFT の付かない m2 と m6（m0 は前・m3 は下書き・m4 は自分）: {r:?}");
    let latest = r.latest.unwrap();
    assert!(latest.id == "m6" && latest.snippet_only && latest.body == "ありがとうございます、ぜひ", "UTF-8 でない本文は snippet: {latest:?}");
    assert_eq!(gmail::thread_replies(&client(&base), "t1", None).unwrap().count, 2, "after が無ければ最初の SENT より後");
    assert_eq!(gmail::thread_replies(&client(&base), "t1", Some("m4")).unwrap().count, 1, "after の message より後だけ（m6）");
    assert!(gmail::thread_replies(&client(&base), "t1", Some("nope")).is_err());
    assert_eq!(gmail::strip_quoted("興味があります。\n来週どうですか\n\nOn Sat, Oct 17, 2026 at 10:00 AM 山田 太郎 wrote:\n> はじめまして"), "興味があります。\n来週どうですか");
    assert_eq!(gmail::strip_quoted("はい\n2026年10月17日(土) 10:00 山田 太郎 <me@x.io>:\n> x"), "はい");
    assert_eq!(gmail::strip_quoted("Sounds good.\n\nOn Wed, Oct 15, 2026 at 10:00\u{202f}AM Bob <b@x.io>\nwrote:\n> x"), "Sounds good.", "折り返した引用の頭");
    assert_eq!(gmail::strip_quoted("ではまた\nBob <b@x.io>"), "ではまた\nBob <b@x.io>", "引用が無ければ署名の行は切らない");
    // plan から: 送った宛先は送信済みの key で thread を引く・まだ送っていない宛先は sent=false（thread を読まない）
    let p = plan4();
    let (base, log) = fake_gmail(&[], None, false);
    let c = client(&base);
    let mut one = p.clone();
    one.drafts.truncate(1);
    assert!(outreach::send_plan(&c, &one, Some(&consent_for(&outreach::send_digest(&one))), NOW).finished());
    let rows = outreach::replies(&c, &p);
    let got: Vec<(bool, usize, &str)> = rows.iter().map(|r| (r.sent, r.count, r.thread_id.as_str())).collect();
    assert_eq!(got, [(true, 0, "t-m1"), (false, 0, ""), (false, 0, ""), (false, 0, "")], "{rows:?}");
    assert_eq!(log.lock().unwrap().iter().filter(|r| r.path.contains("/threads/")).count(), 1, "送っていない宛先の thread は読まない");
    // 1 件の読みが失敗しても他の行は出す（その行に理由）
    let (base, _) = fake(vec![("GET /gmail/v1/users/me/messages?", 503, json!({}))]);
    let rows = outreach::replies(&client(&base), &one);
    assert!(rows.len() == 1 && !rows[0].sent && rows[0].error.contains("Can't read Sent"), "{rows:?}");
}
