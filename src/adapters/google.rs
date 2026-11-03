//! Google（main に入った src/google を包む）: Gmail threads = external_read、下書き作成 = external_write（送信はしない）、
//! Calendar events = external_read、予定作成 = external_write（参加者は付けない = 招待 mail を出さない）。経路は official API（REST）。
//! 作成は src/google の create_draft_checked / create_event（作る → client が決めた印で読み戻し → 一致した時だけ Verified・CD-53）。
//! id と scope は corpus 08 / 09（docs/examples/v3）と同じ。token の取得・refresh・Keychain は src/google/oauth.rs（bin kangae-google）の仕事で、
//! ここは access token（Client）と同意済みの scope の文字列を受け取るだけ。
use super::super::domain::{action::Effect, binding::Source, capability::CapabilityReadiness, run::{ErrorCode, Observation, Outcome}};
use super::super::google::{calendar, gmail, Client, Written};
use super::sdk::{now, payload_digest, read_result, source, Adapter, AdapterKind, Route, SourceSync, Spec};
use serde_json::{json, Value};

const SPECS: &[Spec] = &[
    Spec { id: "gmail.threads.read", effect: Effect::ExternalRead, approval: false, scopes: &["mail.read"],
        input: r#"{"type":"object","properties":{"query":{"type":"string"},"max":{"type":"integer"}}}"#, verifier: "read_back" },
    Spec { id: "gmail.drafts.create", effect: Effect::ExternalWrite, approval: true, scopes: &["mail.compose"],
        input: r#"{"type":"object","required":["to","subject","body"],"properties":{"to":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"},"key":{"type":"string"}}}"#, verifier: "read_back" },
    Spec { id: "calendar.events.read", effect: Effect::ExternalRead, approval: false, scopes: &["calendar.read"],
        input: r#"{"type":"object","properties":{"days":{"type":"integer"}}}"#, verifier: "read_back" },
    Spec { id: "calendar.events.create", effect: Effect::ExternalWrite, approval: true, scopes: &["calendar.write"],
        input: r#"{"type":"object","required":["summary","start","end"],"properties":{"summary":{"type":"string"},"start":{"type":"string"},"end":{"type":"string"},"description":{"type":"string"},"id":{"type":"string"}}}"#, verifier: "read_back" },
];
/// corpus の scope → src/google/oauth.rs が同意を求める OAuth scope の末尾
fn oauth_scope(scope: &str) -> &'static str {
    match scope { "mail.read" => "gmail.readonly", "mail.compose" => "gmail.compose", _ => "calendar.events" }
}
/// 読み取りの失敗（src/google の get は失敗を文言で返す）→ ErrorCode。分類は google/mod.rs の status_message / finish の文言の前方一致。
/// 書き込みは src/google の Written（status で分けた Receipt）をそのまま使うので、ここを通らない
// ponytail: 文言の前方一致。src/google の読み取りが Fail（status）を返す形になったら Fail::code に置き換える（文言が変わると tests/adapters.rs の google 試験が赤になる）
fn failure(m: &str) -> Outcome {
    let p = |s: &str| m.starts_with(s);
    let code = if p("Google の認証が切れています") || p("権限（scope）が足りません") || p("Google がアクセスを拒否しました") { ErrorCode::PermissionDenied }
        else if p("Google の利用上限") { ErrorCode::RateLimited }
        else if p("API が有効化されていません") { ErrorCode::NotReady }
        else if p("見つかりません") { ErrorCode::TargetNotFound }
        else if p("Google に接続できません") || p("Google 側で障害") { ErrorCode::ConnectionLost }
        else if p("Google が想定外の応答") { ErrorCode::InvalidInput }
        else { ErrorCode::Internal };
    Outcome::Failed { error: code }
}
/// 書き込みの Receipt（src/google の Written）→ Outcome。Verified に添えた事情（「同じ物が既に在りました」）は根拠の最後に足す
fn written(w: Written) -> Outcome {
    match w.outcome {
        Outcome::Verified { mut evidence } if !w.detail.is_empty() => { evidence.push(w.detail); Outcome::Verified { evidence } }
        o => o,
    }
}

pub struct Google { client: Option<Client>, granted: String, gmail: SourceSync, calendar: SourceSync }
impl Google {
    /// `granted` は token の scope（空白区切りの URL。oauth::Tokens.scope）
    pub fn new(client: Option<Client>, granted: &str) -> Self { Self { client, granted: granted.into(), gmail: SourceSync::default(), calendar: SourceSync::default() } }
    fn has(&self, action: &str) -> bool {
        self.client.is_some() && SPECS.iter().filter(|s| s.id == action).flat_map(|s| s.scopes.iter())
            .all(|s| self.granted.split_whitespace().any(|g| g.rsplit('/').next() == Some(oauth_scope(s))))
    }
    fn sync(&mut self, action: &str) -> &mut SourceSync { if action.starts_with("gmail.") { &mut self.gmail } else { &mut self.calendar } }
    fn run(&self, c: &Client, action: &str, input: &Value, t: u64) -> (Outcome, Option<Observation>) {
        let s = |k: &str| input[k].as_str().unwrap_or("");
        match action {
            "gmail.threads.read" => {
                let query = input["query"].as_str().unwrap_or("newer_than:7d");
                match gmail::list_threads(c, query, input["max"].as_u64().unwrap_or(20) as usize) {
                    Ok(threads) => read_result(Ok(json!(threads)), "google.gmail", "threads.list", t),
                    Err(m) => (failure(&m), None),
                }
            }
            "calendar.events.read" => match calendar::list_events(c, input["days"].as_u64().unwrap_or(7)) {
                Ok(events) => read_result(Ok(json!(events)), "google.calendar", "events.list", t),
                Err(m) => (failure(&m), None),
            },
            // 作る印（下書きの X-Kangae-Key・予定の id）は承認した入力の digest から決める: 送れたか分からない後に人が同じ内容を承認し直して
            // 打っても同じ印になり、下書きは key で先に見つけ、予定は Google の 409 で、2 つ目を作らない（AD-R2-1）。入力に key / id があればそれ
            // ponytail: 消した予定の id は Google が使い直させない。全く同じ予定を消してから作り直すと 409 → InvalidInput。その時は入力に新しい id（calendar::new_event_id）
            "gmail.drafts.create" => {
                let d = gmail::Draft { to: s("to").into(), subject: s("subject").into(), body: s("body").into(), thread_id: String::new(), in_reply_to: String::new(),
                    // 空の key は無い物として扱う: header の無い利用者自身の下書きと「同じ key」にしない（AD-R3-3）
                    references: String::new(), key: input["key"].as_str().filter(|k| !k.trim().is_empty()).map_or_else(|| payload_digest(action, input), str::to_string) };
                (written(gmail::create_draft_checked(c, &d)), None)
            }
            _ => {
                // 参加者を付けると sendUpdates=all で招待 mail が出る（external_send）。この external_write の action では付けない
                if !input["attendees"].is_null() { return (Outcome::Rejected { reason: "参加者を付けると招待 mail が送られます。この操作では参加者を付けません".into() }, None) }
                let mut request = match calendar::event_request(s("summary"), s("start"), s("end"), &[], s("description")) {
                    Ok(r) => r,
                    Err(m) => return (Outcome::Rejected { reason: m }, None),
                };
                request["body"]["id"] = json!(input["id"].as_str().filter(|k| !k.trim().is_empty()).map_or_else(|| format!("{}{}", calendar::ID_PREFIX, payload_digest(action, input)), str::to_string));
                (written(calendar::create_event(c, &request["body"], None)), None)
            }
        }
    }
}
impl Adapter for Google {
    fn id(&self) -> &'static str { "google" }
    fn kind(&self) -> AdapterKind { AdapterKind::Api }
    fn route(&self) -> Route { Route::OfficialApi }
    fn provider(&self) -> &'static str { "google" }
    fn specs(&self) -> &'static [Spec] { SPECS }
    fn readiness(&self, action: &str) -> CapabilityReadiness {
        let sync = if action.starts_with("gmail.") { &self.gmail } else { &self.calendar };
        sync.capability(self.has(action))
    }
    fn sources(&self) -> Vec<Source> {
        vec![source("google.gmail", AdapterKind::Api, "google", &self.gmail, self.has("gmail.threads.read") || self.has("gmail.drafts.create")),
             source("google.calendar", AdapterKind::Api, "google", &self.calendar, self.has("calendar.events.read"))]
    }
    /// 同意済みの面だけ 1 回ずつ読む（Gmail は profile・Calendar は 1 件）。token の refresh はしない（oauth.rs の仕事）
    fn check(&mut self) {
        let Some(c) = &self.client else { return };
        let t = now();
        let probe = |url: String| match c.get(&url) { Ok(_) => Outcome::Verified { evidence: vec![url] }, Err(m) => failure(&m) };
        let gmail = (self.has("gmail.threads.read") || self.has("gmail.drafts.create")).then(|| probe(format!("{}/gmail/v1/users/me/profile", c.gmail_base)));
        let calendar = self.has("calendar.events.read").then(|| probe(format!("{}/calendar/v3/calendars/primary/events?maxResults=1", c.calendar_base)));
        if let Some(o) = gmail { self.gmail.record_check(t, &o) }
        if let Some(o) = calendar { self.calendar.record_check(t, &o) }
    }
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        let t = now();
        let Some(c) = &self.client else { return (Outcome::Rejected { reason: "Google に接続していません（kangae-google auth）".into() }, None) };
        let (outcome, observation) = self.run(c, action, input, t);
        self.sync(action).record(t, &outcome, observation.as_ref());
        (outcome, observation)
    }
    fn cleanup(&mut self) { self.client = None; self.granted.clear(); self.gmail = SourceSync::default(); self.calendar = SourceSync::default(); }
}
