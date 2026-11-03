//! LLM provider の試験。本物の API は叩かず（鍵が在っても使わない）、127.0.0.1 の偽 HTTP server で
//! 5 経路（200 / 429 → retry / 5xx / 不正 JSON / timeout）と、送る形・schema の検査・Fake・local への fallback を確かめる。
//! Keychain と env には触らない（provider は struct を直接組む）。
#[path = "../src/llm/mod.rs"]
mod llm;
use llm::{anthropic::Anthropic, fake::Fake, openai::OpenAi, Answer, Http, JsonRequest, Llm, LlmError};
use serde_json::{json, Value};
use std::{collections::VecDeque, io::{Read, Write}, net::{TcpListener, TcpStream}, sync::{Arc, Mutex}, thread, time::{Duration, Instant}};

struct Req { path: String, headers: Vec<(String, String)>, body: Value }
impl Req { fn header(&self, k: &str) -> Option<&str> { self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str()) } }

/// 偽 server の返し方。hang は request を読んで返さない（timeout の経路）
struct Reply { status: u16, headers: Vec<(&'static str, &'static str)>, body: String, hang: bool }
impl Reply { fn with(mut self, k: &'static str, v: &'static str) -> Self { self.headers.push((k, v)); self } }
fn raw(status: u16, body: &str) -> Reply { Reply { status, headers: vec![], body: body.to_string(), hang: false } }
fn status(code: u16, body: Value) -> Reply { raw(code, &body.to_string()) }
fn ok(body: Value) -> Reply { status(200, body) }
fn hang() -> Reply { Reply { hang: true, ..raw(0, "") } }

/// 接続ごとに次の Reply を返す（Connection: close なので 1 接続 1 request）。尽きたら 418。request は記録する。
/// 接続ごとに thread を立てるので、hang の間に retry が来たら即座に答える（retry した事が結果に出る）
fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Req>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = Arc::clone(&log);
    let mut replies: VecDeque<Reply> = replies.into();
    thread::spawn(move || for conn in l.incoming() {
        let Ok(mut s) = conn else { break };
        let reply = replies.pop_front().unwrap_or_else(|| raw(418, "{}"));
        let log = Arc::clone(&log2);
        thread::spawn(move || {
            let req = read_request(&mut s);
            log.lock().unwrap().push(req);
            if reply.hang { thread::sleep(Duration::from_secs(3)); return }
            let extra: String = reply.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
            let _ = s.write_all(format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{}",
                reply.status, reply.body.len(), reply.body).as_bytes());
        });
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
    let path = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let body_start = (head_end + 4).min(buf.len());
    while buf.len() < body_start + len { match s.read(&mut tmp) { Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]), _ => break } }
    let body = serde_json::from_slice(&buf[body_start..(body_start + len).min(buf.len())]).unwrap_or(Value::Null);
    Req { path, headers, body }
}

fn http() -> Http { Http { timeout: Duration::from_secs(2), retries: 2, backoff: Duration::from_millis(10), max_wait: Duration::from_secs(2) } }
fn claude(base: &str) -> Anthropic { Anthropic { key: "test-key".to_string(), model: "claude-sonnet-5-5".to_string(), base: base.to_string(), http: http() } }
fn gpt(base: &str) -> OpenAi { OpenAi { key: "test-key".to_string(), model: "gpt-6-luna".to_string(), base: base.to_string(), http: http() } }

/// chat bar の依頼の形: 返事の文と、実行候補の action（id は呼び手が持つ一覧の enum）
fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["reply", "actions"], "properties": {
        "reply": {"type": "string"},
        "actions": {"type": "array", "items": {"type": "object", "additionalProperties": false, "required": ["id", "note"], "properties": {
            "id": {"type": "string", "enum": ["gmail.draft", "gcal.create"]},
            "note": {"type": ["string", "null"]}}}}}})
}
fn req() -> JsonRequest { JsonRequest::new("kangae_actions", "Choose actions from the list.", "来週の打合せを入れて", schema()) }
fn good() -> Value { json!({"reply": "予定を作ります", "actions": [{"id": "gcal.create", "note": null}]}) }
fn claude_says(v: &Value) -> Value {
    json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5", "stop_reason": "end_turn",
        "content": [{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "text", "text": v.to_string()}], "usage": {"input_tokens": 9, "output_tokens": 9}})
}
fn gpt_says(v: &Value) -> Value {
    json!({"id": "resp_1", "status": "completed", "output": [{"type": "reasoning", "summary": []},
        {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": v.to_string()}]}]})
}
fn api_error(kind: &str, message: &str) -> Value { json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"}) }

#[test]
fn path1_200_returns_validated_json_via_output_config_not_forced_tool_use() {
    let (base, log) = serve(vec![ok(claude_says(&good()))]);
    assert_eq!(claude(&base).complete_json(req()).unwrap(), good());
    {
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1);
        let r = &log[0];
        assert_eq!(r.path, "/v1/messages");
        assert_eq!(r.header("x-api-key"), Some("test-key"));
        assert_eq!(r.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(r.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
        assert_eq!(r.body["model"], "claude-sonnet-5-5");
        assert_eq!(r.body["system"], "Choose actions from the list.");
        assert_eq!(r.body["messages"], json!([{"role": "user", "content": "来週の打合せを入れて"}]));
        assert_eq!(r.body["output_config"], json!({"format": {"type": "json_schema", "schema": schema()}, "effort": "low"}));
        assert_eq!(r.body["fallbacks"], "default");
        // claude-sonnet-5-5 は tool_choice の any / tool・thinking の disabled・temperature の非既定値を 400 で拒否する
        for k in ["tool_choice", "tools", "thinking", "temperature"] { assert!(r.body.get(k).is_none(), "{k} は送らない: {}", r.body) }
    }
    // beta を受けない org の 400 → fallback なしで 1 回だけ送り直す
    let beta_400 = api_error("invalid_request_error", "Unexpected value(s) `server-side-fallback-2026-07-01` for the `anthropic-beta` header.");
    let (base, log) = serve(vec![status(400, beta_400), ok(claude_says(&good()))]);
    assert_eq!(claude(&base).complete_json(req()).unwrap(), good());
    {
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].header("anthropic-beta"), None);
        assert!(log[1].body.get("fallbacks").is_none(), "{}", log[1].body);
    }
    // server-side fallback が起きた応答: fallback block より前の text（断った model の途中の出力）は捨てる
    let swapped = json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "{\"reply\": \"途中"},
        {"type": "fallback", "from": {"model": "claude-sonnet-5-5"}, "to": {"model": "claude-sonnet-5"}}, {"type": "text", "text": good().to_string()}]});
    let (base, _) = serve(vec![ok(swapped)]);
    assert_eq!(claude(&base).complete_json(req()).unwrap(), good());
    // OpenAI（ai.rs の Responses API を移した物）
    let (base, log) = serve(vec![ok(gpt_says(&good()))]);
    assert_eq!(gpt(&base).complete_json(req()).unwrap(), good());
    let log = log.lock().unwrap();
    let r = &log[0];
    assert_eq!(r.path, "/v1/responses");
    assert_eq!(r.header("authorization"), Some("Bearer test-key"));
    assert_eq!(r.body["text"]["format"], json!({"type": "json_schema", "name": "kangae_actions", "schema": schema(), "strict": true}));
    assert_eq!((&r.body["model"], &r.body["instructions"], &r.body["input"], &r.body["store"]),
        (&json!("gpt-6-luna"), &json!("Choose actions from the list."), &json!("来週の打合せを入れて"), &json!(false)));
}

#[test]
fn path2_429_is_retried_honoring_retry_after_but_quota_is_not() {
    let rate = || status(429, api_error("rate_limit_error", "Number of request tokens has exceeded your per-minute rate limit"));
    // retry-after: 1 → 1 秒待って 2 本目で通る
    let (base, log) = serve(vec![rate().with("retry-after", "1"), ok(claude_says(&good()))]);
    let t = Instant::now();
    assert_eq!(claude(&base).complete_json(req()).unwrap(), good());
    assert!(t.elapsed() >= Duration::from_millis(900), "retry-after を守っていない: {:?}", t.elapsed());
    assert_eq!(log.lock().unwrap().len(), 2);
    // retry-after 無しは backoff で 2 回まで。3 本とも 429 なら RateLimited（4 本目の 200 には行かない）
    let (base, log) = serve(vec![rate(), rate(), rate(), ok(claude_says(&good()))]);
    assert_eq!(claude(&base).complete_json(req()).unwrap_err(), LlmError::RateLimited { retry_after: None });
    assert_eq!(log.lock().unwrap().len(), 3);
    // retry-after が max_wait（試験では 2 秒）より長い → 待たずに秒数つきで返す
    let (base, log) = serve(vec![rate().with("retry-after", "60"), ok(claude_says(&good()))]);
    let t = Instant::now();
    let e = claude(&base).complete_json(req()).unwrap_err();
    assert_eq!(e, LlmError::RateLimited { retry_after: Some(60) });
    assert!(t.elapsed() < Duration::from_secs(1) && e.to_string().contains("try again in 60 s"), "{e} {:?}", t.elapsed());
    assert_eq!(log.lock().unwrap().len(), 1);
    // 利用上限・残高・支払いは retry しない（1 本で Quota）
    let spend_cap = json!({"type": "error", "error": {"type": "rate_limit_error", "details": {"error_code": "enforced_spend_limit_reached"},
        "message": "You have reached your API usage limits: your organization has crossed its monthly API usage threshold. You will regain access on 2026-11-01 at 00:00 UTC."}});
    let cases = vec![
        ("anthropic spend cap", status(429, spend_cap)),
        ("anthropic billing", status(402, api_error("billing_error", "There is an issue with your billing."))),
        ("anthropic own limit", status(400, api_error("invalid_request_error", "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC."))),
        ("anthropic credit", status(400, api_error("invalid_request_error", "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."))),
        ("openai quota", status(429, json!({"error": {"type": "insufficient_quota", "code": "insufficient_quota", "message": "You exceeded your current quota."}}))),
        ("openai spend", status(429, json!({"error": {"type": "insufficient_quota", "code": "project_spend_limit_exceeded", "message": "Project spend limit reached."}}))),
    ];
    for (name, reply) in cases {
        let (base, log) = serve(vec![reply, ok(claude_says(&good()))]);
        let r = if name.starts_with("openai") { gpt(&base).complete_json(req()) } else { claude(&base).complete_json(req()) };
        let e = r.unwrap_err();
        assert!(matches!(e, LlmError::Quota { .. }) && e.to_string().contains("usage limit or balance is reached"), "{name}: {e:?}");
        assert_eq!(log.lock().unwrap().len(), 1, "{name}: 上限は retry しない");
    }
}

#[test]
fn path3_5xx_is_retried_twice_then_reported() {
    let e5 = |code: u16, kind: &str| status(code, api_error(kind, "temporary"));
    let (base, log) = serve(vec![e5(529, "overloaded_error"), ok(claude_says(&good()))]);
    assert_eq!(claude(&base).complete_json(req()).unwrap(), good());
    assert_eq!(log.lock().unwrap().len(), 2);
    let (base, log) = serve(vec![e5(529, "overloaded_error"), e5(500, "api_error"), e5(504, "timeout_error"), ok(claude_says(&good()))]);
    let t = Instant::now();
    let e = claude(&base).complete_json(req()).unwrap_err();
    assert_eq!(e, LlmError::Server { status: 504 });
    assert!(t.elapsed() >= Duration::from_millis(30), "backoff（10 ms → 20 ms）を待っていない: {:?}", t.elapsed());
    assert!(e.to_string().contains("having trouble"), "{e}");
    assert_eq!(log.lock().unwrap().len(), 3);
    // OpenAI の 503（server_is_overloaded）も retry する
    let overloaded = json!({"error": {"type": "service_unavailable_error", "code": "server_is_overloaded", "message": "overloaded"}});
    let (base, log) = serve(vec![status(503, overloaded), ok(gpt_says(&good()))]);
    assert_eq!(gpt(&base).complete_json(req()).unwrap(), good());
    assert_eq!(log.lock().unwrap().len(), 2);
    // 4xx は retry しない
    let (base, log) = serve(vec![status(401, api_error("authentication_error", "invalid x-api-key")), ok(claude_says(&good()))]);
    let e = claude(&base).complete_json(req()).unwrap_err();
    assert!(matches!(e, LlmError::Auth { status: 401, .. }) && e.to_string().contains("AI key was refused"), "{e:?}");
    assert_eq!(log.lock().unwrap().len(), 1);
    let (base, log) = serve(vec![status(400, api_error("invalid_request_error", "max_tokens: too large")), ok(claude_says(&good()))]);
    assert!(matches!(claude(&base).complete_json(req()).unwrap_err(), LlmError::Rejected { status: 400, .. }));
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[test]
fn path4_bad_json_is_rejected_including_hallucinated_action_ids() {
    let says = |text: &str| json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": text}]});
    let cases = vec![
        ("本文が JSON でない（proxy）", raw(200, "<html>bad gateway</html>")),
        ("答えが JSON でない", ok(says("はい、予定を作ります"))),
        ("答えが空", ok(json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": "", "signature": "s"}]}))),
        ("max_tokens で途中", ok(json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "{\"reply\": \"途"}]}))),
    ];
    for (name, reply) in cases {
        let (base, log) = serve(vec![reply, ok(claude_says(&good()))]);
        let e = claude(&base).complete_json(req()).unwrap_err();
        assert!(matches!(e, LlmError::Malformed { .. }), "{name}: {e:?}");
        assert_eq!(log.lock().unwrap().len(), 1, "{name}: 壊れた応答は retry しない");
    }
    let refusal = json!({"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber", "explanation": null}, "content": []});
    let (base, _) = serve(vec![ok(refusal)]);
    assert_eq!(claude(&base).complete_json(req()).unwrap_err(), LlmError::Refused { detail: "cyber".into() });
    // 幻覚の action id（呼び手の enum に無い）は path つきで拒否する
    let hallucinated = json!({"reply": "全部消します", "actions": [{"id": "gmail.delete_all", "note": null}]});
    let (base, _) = serve(vec![ok(claude_says(&hallucinated))]);
    let e = claude(&base).complete_json(req()).unwrap_err();
    assert_eq!(e, LlmError::Invalid { path: "$.actions[0].id".into(), reason: "\"gmail.delete_all\" は許された値（\"gmail.draft\", \"gcal.create\"）にありません".into() });
    assert!(e.to_string().contains("wasn't used"), "{e}");
    // 形の崩れ: 必須の欠け・schema に無い key・型違い
    for (bad, path) in [
        (json!({"reply": "x"}), "$"),
        (json!({"reply": "x", "actions": [], "open_url": "https://example.invalid"}), "$"),
        (json!({"reply": 1, "actions": []}), "$.reply"),
        (json!({"reply": "x", "actions": [{"id": "gcal.create", "note": 3}]}), "$.actions[0].note"),
    ] {
        let (base, _) = serve(vec![ok(claude_says(&bad))]);
        match claude(&base).complete_json(req()).unwrap_err() { LlmError::Invalid { path: p, .. } => assert_eq!(p, path, "{bad}"), e => panic!("{bad}: {e:?}") }
    }
    // OpenAI: 幻覚・refusal・途中切れ
    let (base, _) = serve(vec![ok(gpt_says(&hallucinated))]);
    assert!(matches!(gpt(&base).complete_json(req()).unwrap_err(), LlmError::Invalid { .. }));
    let gpt_refusal = json!({"status": "completed", "output": [{"type": "message", "content": [{"type": "refusal", "refusal": "I can't help with that."}]}]});
    let (base, _) = serve(vec![ok(gpt_refusal)]);
    assert_eq!(gpt(&base).complete_json(req()).unwrap_err(), LlmError::Refused { detail: "I can't help with that.".into() });
    let cut = json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": []});
    let (base, _) = serve(vec![ok(cut)]);
    assert!(matches!(gpt(&base).complete_json(req()).unwrap_err(), LlmError::Malformed { detail } if detail.contains("max_output_tokens")));
}

#[test]
fn path5_timeout_is_explicit_and_not_retried() {
    // 2 本目は即答する。retry したら Ok になってこの試験が落ちる
    let (base, log) = serve(vec![hang(), ok(claude_says(&good()))]);
    let mut c = claude(&base);
    c.http.timeout = Duration::from_millis(300);
    let t = Instant::now();
    let e = c.complete_json(req()).unwrap_err();
    assert_eq!(e, LlmError::Timeout { ms: 300 });
    assert!(t.elapsed() < Duration::from_millis(2500), "{:?}", t.elapsed());
    assert!(e.to_string().contains("within 0.3 s"), "{e}");
    assert_eq!(log.lock().unwrap().len(), 1);
    // 繋がらない（閉じた port）は Network で、これも retry しない
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let e = claude(&format!("http://127.0.0.1:{closed}")).complete_json(req()).unwrap_err();
    assert!(matches!(e, LlmError::Network { .. }) && e.to_string().contains("check the network"), "{e:?}");
}

#[test]
fn schema_is_checked_before_sending_and_validator_covers_its_keywords() {
    let (base, log) = serve(vec![]);
    let c = claude(&base);
    let with = |schema: Value| JsonRequest { schema, ..req() };
    for (schema, path) in [
        (json!({"type": "array", "items": {"type": "string"}}), "$"),
        (json!({"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}}), "$"),
        (json!({"type": "object", "additionalProperties": false, "required": [], "properties": {"a": {"type": "string"}}}), "$"),
        (json!({"type": "object", "additionalProperties": false, "required": ["a"], "properties": {"a": {"type": "array", "minItems": 2, "items": {"type": "string"}}}}), "$.a"),
        (json!({"type": "object", "additionalProperties": false, "required": ["a"], "properties": {"a": {"type": "array", "items": {"type": "object", "properties": {}}}}}), "$.a[]"),
    ] {
        match c.complete_json(with(schema.clone())).unwrap_err() { LlmError::Schema { path: p, .. } => assert_eq!(p, path, "{schema}"), e => panic!("{schema}: {e:?}") }
    }
    assert!(matches!(c.complete_json(JsonRequest { name: "名前 空白".into(), ..req() }).unwrap_err(), LlmError::Schema { .. }));
    // 任意の値（type に null）は Anthropic の上限 16 箇所まで
    let nullable = |n: usize| {
        let names: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
        let props: serde_json::Map<String, Value> = names.iter().map(|k| (k.clone(), json!({"type": ["string", "null"]}))).collect();
        json!({"type": "object", "additionalProperties": false, "required": names, "properties": props})
    };
    let (base16, log16) = serve(vec![ok(claude_says(&json!({})))]);
    assert!(!matches!(claude(&base16).complete_json(with(nullable(16))).unwrap_err(), LlmError::Schema { .. }), "16 箇所は送る");
    assert_eq!(log16.lock().unwrap().len(), 1);
    match c.complete_json(with(nullable(17))).unwrap_err() { LlmError::Schema { reason, .. } => assert!(reason.contains("17"), "{reason}"), e => panic!("{e:?}") }
    assert!(log.lock().unwrap().is_empty(), "schema の誤りは送る前に止まる");
    let s = json!({"type": "object", "additionalProperties": false, "required": ["n", "k", "v"], "properties": {
        "n": {"type": "integer"}, "k": {"anyOf": [{"type": "string", "enum": ["a", "b"]}, {"type": "null"}]}, "v": {"const": "v1"}}});
    assert!(llm::validate(&s, &json!({"n": 2, "k": null, "v": "v1"})).is_ok());
    assert!(llm::validate(&s, &json!({"n": 2.0, "k": "a", "v": "v1"})).is_ok(), "2.0 は integer");
    for bad in [json!({"n": 2.5, "k": null, "v": "v1"}), json!({"n": 2, "k": "z", "v": "v1"}), json!({"n": 2, "k": null, "v": "v2"}), json!([1])] {
        assert!(matches!(llm::validate(&s, &bad), Err(LlmError::Invalid { .. })), "{bad}");
    }
}

#[test]
fn fake_goes_through_the_same_validation_and_local_fallback_keeps_the_reason() {
    let hallucinated = json!({"reply": "x", "actions": [{"id": "shell.exec", "note": null}]});
    let fake = Fake::new(vec![Ok(good()), Ok(hallucinated.clone()), Err(LlmError::Quota { detail: "spend cap".into() })]);
    assert_eq!(fake.complete_json(req()).unwrap(), good());
    assert!(matches!(fake.complete_json(req()).unwrap_err(), LlmError::Invalid { .. }), "Fake でも幻覚の id は拒否される");
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
    let local = || json!({"reply": "手元の推定で作りました", "actions": []});
    assert!(matches!(llm::complete_or_local(Some(&fake), req(), local), Answer::Local { reason: LlmError::Quota { .. }, .. }));
    assert_eq!(llm::complete_or_local(None, req(), local), Answer::Local { value: local(), reason: LlmError::NoKey });
    let fake = Fake::new(vec![Ok(hallucinated)]);
    match llm::complete_or_local(Some(&fake), req(), local) {
        Answer::Local { value, reason: LlmError::Invalid { path, .. } } => { assert_eq!(value, local()); assert_eq!(path, "$.actions[0].id") }
        a => panic!("{a:?}"),
    }
    let fake = Fake::new(vec![Ok(good())]);
    assert_eq!(llm::complete_or_local(Some(&fake), req(), local), Answer::Model { value: good(), label: "fake".into() });
}
