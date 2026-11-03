//! 営業メールの一括（test1 の connector 側）: 宛先表 + template → 相手ごとの下書き → 1 回の承認（下書きの digest の束）で N 件送る → 返信の件数と本文。
//! 門は 4 つ: template の未定義の欄・件名・footer の法令の表示（作る前）・送信用 digest への ConsentReceipt（HTTP の前）・1 日の上限（Shibubu が 24 時間に送った分だけ数える）・
//! 同じ宛先に 2 回送らない（送信済みの X-Kangae-Key）。途中で失敗したら残りを止め、何件送って何件残ったかを返す。
//! footer の法令の表示: 送り先の国は推測せず、日本と米国の要件の和を全件に掛ける。出典（2026-10-17 取得）:
//!   特定電子メール法 4 条（e-Gov 法令 API 414AC0100000026・2025-06-01 施行の版）「一 当該送信者…の氏名又は名称
//!     二 前条第三項本文の通知を受けるための電子メールアドレス又は…符号であって総務省令・内閣府令で定めるもの 三 その他総務省令・内閣府令で定める事項」
//!   同 施行規則 9 条（414M60000008066・2020-12-15 の版）「一 …送信をしないように求める旨の通知を…行うことができる旨 二 法第四条第一号に規定する者の住所
//!     三 …苦情、問合せ等を受け付けることのできる電話番号、電子メールアドレス又は…符号」。7 条 1 項 2 号: 送信に用いた address 宛ての返信で受信拒否できる時、その旨は本文の任意の場所でよい
//!   CAN-SPAM（https://www.ftc.gov/business-guidance/resources/can-spam-act-compliance-guide-business）「Your message must include your valid physical postal address」
//!     「a clear and conspicuous explanation of how the recipient can opt out」「The subject line must accurately reflect the content of the message」「The law makes no exception for business-to-business email」
//!   門に入れていない物: 広告である旨（CAN-SPAM。文言で機械判定できない）・送る前の同意（特定電子メール法 3 条は opt-in。公表された address・取引関係の例外に当たるかは人が決める）
use super::{approved, contacts::{Contact, NO_MAIL}, digest, gmail::{self, Draft}, Client, Fail, Written};
use crate::domain::{permission::ConsentReceipt, run::Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

pub const DEFAULT_DAILY_LIMIT: usize = 20;
/// 受ける上限の最大 = 個人の Gmail の 1 日の送信上限（500 通。Workspace は 2,000: gmail.md §3.3）。API・web・SMTP の全部で共有の枠
pub const MAX_DAILY_LIMIT: usize = 500;
/// template で使える欄（宛先表の列 + 送り手の表示）
pub const FIELDS: [&str; 11] = ["name", "first_name", "last_name", "company", "title", "mail", "url", "batch", "sender_name", "sender_address", "sender_contact"];
/// footer の受信拒否の行: この語のどれか + 返信 / reply
const OPT_OUT: [&str; 7] = ["今後不要", "不要でしたら", "不要なら", "配信停止", "unsubscribe", "opt out", "opt-out"];
const BY_REPLY: [&str; 2] = ["返信", "reply"];
/// footer を探す範囲（本文の最後の、空でない行の数）
const FOOTER_LINES: usize = 10;
/// 新しい mail の件名を返信・転送に見せかける頭（「件名の偽り」のうち機械で見られる物。全角の ： は : に揃えてから見る）
const FAKE_THREAD: [&str; 6] = ["re:", "fw:", "fwd:", "aw:", "返信:", "転送:"];
/// 送り手の表示を置く所（bin が読む。住所は repo にも template にも書かない）
pub const SENDER_FILE: &str = "~/.kangae/sender.json";

/// 送り手の表示（footer の材料）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sender {
    #[serde(default)] pub name: String,
    /// 郵便の届く住所（番地・私書箱）
    #[serde(default)] pub address: String,
    /// 受信拒否と問合せを受ける mail address か URL
    #[serde(default)] pub contact: String,
}
impl Sender {
    fn field(&self, k: &str) -> Option<String> {
        Some(match k { "sender_name" => &self.name, "sender_address" => &self.address, "sender_contact" => &self.contact, _ => return None }.trim().to_string())
    }
}
/// 送り手の表示が揃っているか（全部 1 行・contact は mail か http(s) の URL。電話だけでは 4 条 2 号の受信拒否の通知先にならない）
pub fn check_sender(s: &Sender) -> Result<(), String> {
    let one_line = |v: &str| !v.trim().is_empty() && !v.contains(['\r', '\n']);
    let c = s.contact.trim();
    let contact_ok = one_line(c) && (!super::contacts::clean_mail(c).is_empty() || ((c.starts_with("https://") || c.starts_with("http://")) && c.contains('.') && !c.contains(char::is_whitespace)));
    let mut missing = Vec::new();
    if !one_line(&s.name) { missing.push("name (the sender's full name or organization name · Japan's Specified Electronic Mail Act, Article 4, item 1)") }
    if !one_line(&s.address) { missing.push("address (a postal address mail can reach · its enforcement rules, Article 9, item 2 / CAN-SPAM's physical postal address)") }
    if !contact_ok { missing.push("contact (an email address or URL that takes opt-outs and inquiries · Article 4, item 2 and enforcement rules Article 9, item 3)") }
    if missing.is_empty() { return Ok(()) }
    Err(format!("The sender details are incomplete: {}. Write {{\"name\": \"…\", \"address\": \"…\", \"contact\": \"…\"}} in {SENDER_FILE} (the name can also be given with --sender)", missing.join("; ")))
}
/// 件名の門: 空・返信 / 転送に見せかける頭
pub fn subject_problem(subject: &str) -> Option<String> {
    let s = subject.trim().replace('：', ":").to_lowercase();
    if s.is_empty() { return Some("The subject is empty".to_string()) }
    FAKE_THREAD.iter().any(|p| s.starts_with(p)).then(|| format!("The subject “{}” starts like a reply or a forward (it is a new email). CAN-SPAM requires the subject to describe the contents accurately", subject.trim()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template { pub subject: String, pub body: String }
/// 1 行目が「件名: …」（Subject: も可）、その後が本文
pub fn parse_template(text: &str) -> Result<Template, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (first, rest) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    let subject = ["件名:", "件名：", "Subject:", "subject:"].iter().find_map(|p| first.trim().strip_prefix(p))
        .ok_or("Make the first line of the template “Subject: …” (the text goes below it)")?.trim();
    Ok(Template { subject: subject.to_string(), body: rest.trim_matches('\n').trim_end().to_string() })
}

enum Tok { Text(String), Field(String, Option<String>) }
/// {欄} / {欄|空の時の文字}。{{ と }} は文字の { と }
fn tokens(s: &str) -> Result<Vec<Tok>, String> {
    let (mut out, mut text, mut it) = (Vec::new(), String::new(), s.chars().peekable());
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => { it.next(); text.push('{') }
            '}' if it.peek() == Some(&'}') => { it.next(); text.push('}') }
            '{' => {
                let mut inner = String::new();
                loop {
                    match it.next() {
                        Some('}') => break,
                        Some('{') | None => return Err(format!("“{{{inner}” isn't closed. Write a literal {{ as {{{{")),
                        Some(ch) => inner.push(ch),
                    }
                }
                if !text.is_empty() { out.push(Tok::Text(std::mem::take(&mut text))) }
                let (name, fallback) = match inner.split_once('|') { Some((n, f)) => (n, Some(f.to_string())), None => (inner.as_str(), None) };
                out.push(Tok::Field(name.trim().to_string(), fallback));
            }
            '}' => return Err("There's a single “}”. Write a literal } as }}".to_string()),
            c => text.push(c),
        }
    }
    if !text.is_empty() { out.push(Tok::Text(text)) }
    Ok(out)
}
/// 差し込む。get(欄) = None は未定義、空は「欄が空」（{欄|…} が在ればその文字）
fn render(s: &str, get: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::new();
    for t in tokens(s)? {
        match t {
            Tok::Text(x) => out += &x,
            Tok::Field(name, fallback) => match (get(&name), fallback) {
                (None, _) => return Err(format!("No field named {{{name}}}")),
                (Some(v), _) if !v.trim().is_empty() => out += v.trim(),
                (Some(_), Some(f)) => out += &f,
                (Some(_), None) => return Err(format!("{name} is empty (set a fallback with {{{name}|fallback text}})")),
            },
        }
    }
    Ok(out)
}
/// footer: 本文の最後の FOOTER_LINES 行に、送り手の名前・住所・連絡先と「今後不要なら返信で」の 1 行（足りない物を全部挙げる）
pub fn footer_problem(body: &str, s: &Sender) -> Option<String> {
    let tail: Vec<String> = body.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).rev().take(FOOTER_LINES).collect();
    let has = |v: &str| !v.trim().is_empty() && tail.iter().any(|l| l.contains(&v.trim().to_lowercase()));
    let opt_out = tail.iter().any(|l| OPT_OUT.iter().any(|w| l.contains(w)) && BY_REPLY.iter().any(|w| l.contains(w)));
    let mut missing = Vec::new();
    if !has(&s.name) { missing.push(format!("The sender's name ({} or {{sender_name}})", s.name.trim())) }
    if !has(&s.address) { missing.push("The address ({sender_address})".to_string()) }
    if !has(&s.contact) { missing.push("The contact for opt-outs and inquiries ({sender_contact})".to_string()) }
    if !opt_out { missing.push("An opt-out line such as “To unsubscribe, reply to this email”".to_string()) }
    (!missing.is_empty()).then(|| format!("The footer lacks required legal details (last {FOOTER_LINES} lines of the text): {}. Japan's Specified Electronic Mail Act (Article 4, enforcement rules Article 9) and the US CAN-SPAM Act require them in the text", missing.join("; ")))
}
/// template を作る前に確かめる: 送り手の表示・件名・未定義の欄（全部挙げる）・footer（送り手の表示は本物の値で差し込んで見る）
pub fn check_template(t: &Template, s: &Sender) -> Result<(), String> {
    check_sender(s)?;
    // m は文の頭（「The subject …」）なので、枠の途中に入れる時は頭を小文字に
    if let Some(m) = subject_problem(&t.subject) {
        let m = m.chars().next().map(|c| c.to_lowercase().collect::<String>() + &m[c.len_utf8()..]).unwrap_or_default();
        return Err(format!("In the template, {m}"));
    }
    let mut unknown: Vec<String> = Vec::new();
    for part in [&t.subject, &t.body] {
        for tok in tokens(part)? {
            if let Tok::Field(n, _) = tok { if !FIELDS.contains(&n.as_str()) && !unknown.contains(&n) { unknown.push(n) } }
        }
    }
    if !unknown.is_empty() {
        let names: Vec<String> = unknown.iter().map(|n| format!("{{{n}}}")).collect();
        return Err(format!("The template has undefined fields: {} (fields you can use: {}). No draft was created", names.join(" "), FIELDS.join(" ")));
    }
    let sample = render(&t.body, &|k| Some(s.field(k).unwrap_or_else(|| "x".to_string())))?;
    footer_problem(&sample, s).map_or(Ok(()), |m| Err(format!("{m}. No draft was created")))
}
fn check_limit(limit: usize) -> Result<(), String> {
    if (1..=MAX_DAILY_LIMIT).contains(&limit) { Ok(()) } else { Err(format!("Give the daily limit as 1 to {MAX_DAILY_LIMIT} ({limit})")) }
}

/// 下書きにしなかった行と理由
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skip { pub who: String, pub reason: String }
/// 見せる・承認する・送るの単位。drafts の中身（と送り手・上限）が digest の対象。gmail_drafts は digest の外
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// 前の版の plan（sender_name だけ）は空で読まれ、check_plan で作り直しを求める
    #[serde(default)]
    pub sender: Sender,
    pub daily_limit: usize,
    pub drafts: Vec<Draft>,
    #[serde(default)]
    pub skipped: Vec<Skip>,
    /// outreach-drafts が Gmail に作った下書き（X-Kangae-Key → draft id）。送った後に消す
    #[serde(default)]
    pub gmail_drafts: BTreeMap<String, String>,
}
/// 宛先表 → 相手ごとの下書き（通信しない）。template の誤りは Err（下書き 0 件）、行ごとの不足（mail なし・欄が空・同じ宛先）は skipped
pub fn plan(rows: &[Contact], t: &Template, sender: &Sender, daily_limit: usize) -> Result<Plan, String> {
    check_template(t, sender)?;
    check_limit(daily_limit)?;
    let (mut drafts, mut skipped, mut seen) = (Vec::new(), Vec::new(), HashSet::new());
    for r in rows {
        let skip = |reason: String| Skip { who: r.who().to_string(), reason };
        if r.mail.is_empty() { skipped.push(skip(NO_MAIL.to_string())); continue }
        if !seen.insert(r.mail.to_lowercase()) { skipped.push(skip(format!("An earlier row has the same recipient ({})", r.mail))); continue }
        let get = |k: &str| sender.field(k).or_else(|| r.field(k).map(|v| v.replace(['\r', '\n'], " ")));
        match (render(&t.subject, &get), render(&t.body, &get)) {
            (Ok(subject), Ok(body)) => drafts.push(Draft { to: r.mail.clone(), subject, body, thread_id: String::new(), in_reply_to: String::new(), references: String::new(), key: gmail::new_key()? }),
            (Err(e), _) | (_, Err(e)) => skipped.push(skip(e)),
        }
    }
    let sender = Sender { name: sender.name.trim().into(), address: sender.address.trim().into(), contact: sender.contact.trim().into() };
    Ok(Plan { sender, daily_limit, drafts, skipped, gmail_drafts: BTreeMap::new() })
}
/// 承認が束ねる物: effect・送り手の表示・上限・下書きごとの digest（順序も含む）。effect が違うので、下書きの承認で送信は打てない
fn bundle(p: &Plan, effect: &str) -> Value {
    let each: Vec<String> = p.drafts.iter().map(|d| digest(&serde_json::to_value(d).unwrap_or(Value::Null))).collect();
    json!({"effect": effect, "sender": p.sender, "daily_limit": p.daily_limit, "drafts": each})
}
pub fn send_digest(p: &Plan) -> String { digest(&bundle(p, "external_send")) }
pub fn drafts_digest(p: &Plan) -> String { digest(&bundle(p, "external_write")) }
/// HTTP の前に plan を確かめる（file を手で直した時も）: 送り手の表示・上限・宛先の形と重複・件名・footer
pub fn check_plan(p: &Plan) -> Result<(), String> {
    check_sender(&p.sender).map_err(|e| format!("{e}. Rebuild the plan with outreach-plan"))?;
    check_limit(p.daily_limit)?;
    if p.drafts.is_empty() { return Err("There are no drafts to send".to_string()) }
    let mut seen = HashSet::new();
    for d in &p.drafts {
        gmail::check_draft_fields(d).map_err(|e| format!("{}: {e}", d.to))?;
        if let Some(m) = subject_problem(&d.subject).or_else(|| footer_problem(&d.body, &p.sender)) { return Err(format!("{}: {m}", d.to)) }
        if !seen.insert(d.to.to_lowercase()) { return Err(format!("The recipient ({}) appears twice", d.to)) }
    }
    Ok(())
}

/// 1 件ごとの結果（Written の id = Gmail の message id / draft id）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step { pub to: String, pub key: String, pub result: Written }
/// 一括の結果。done = Verified の分・skipped = 送らなかった宛先と理由・stopped = 止まった理由（to が空なら宛先に依らない理由）・remaining = 手を付けていない件数
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchReport {
    pub done: Vec<Step>,
    pub skipped: Vec<Skip>,
    pub stopped: Option<Step>,
    pub remaining: usize,
    /// 送る前に数えた、Kangae が 24 時間に送った数（下書きの一括では None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_24h_before: Option<usize>,
}
impl BatchReport {
    fn halted(n: usize, w: Written) -> Self { Self { done: vec![], skipped: vec![], stopped: Some(Step { to: String::new(), key: String::new(), result: w }), remaining: n, sent_24h_before: None } }
    pub fn finished(&self) -> bool { self.stopped.is_none() }
    /// 人が読む 1 行: 何件済んで・何件飛ばし・何件残ったか・止まった理由
    pub fn summary(&self, verb: &str) -> String {
        let mut s = format!("{verb} {} · skipped {} · {} left", self.done.len(), self.skipped.len(), self.remaining);
        if let Some(st) = &self.stopped {
            s += &if st.to.is_empty() { format!(". Reason it stopped: {}", st.result.why()) } else { format!(". Stopped at {}: {}", st.to, st.result.why()) };
        }
        s
    }
}
fn read_fail(f: Fail, what: &str) -> Written {
    Written::new(None, Outcome::EffectUnknown { reason: format!("Can't check {what}: {}", f.message) })
}

/// 1 日の上限のために数える: Kangae が now の 24 時間前から送った数（cap で数えるのを止める）
pub fn sent_24h(c: &Client, now: u64, cap: usize) -> Result<usize, Fail> {
    gmail::kangae_sent(c, &format!("in:sent after:{}", now.saturating_sub(86_400)), cap).map(|v| v.len())
}

/// 送る。1 回の ConsentReceipt（send_digest への Approved）で plan の N 件。承認・plan の検査に通らなければ HTTP 0 本。
/// 順に: 上限（Kangae の 24 時間の数 + この run の数）→ 同じ宛先に Kangae が送っていないか → messages.send → 読み戻し。Verified 以外が出たらそこで止める（自動で再送しない）
pub fn send_plan(c: &Client, p: &Plan, consent: Option<&ConsentReceipt>, now: u64) -> BatchReport {
    let n = p.drafts.len();
    if !approved(consent, &bundle(p, "external_send")) {
        return BatchReport::halted(n, Written::new(None, Outcome::Rejected { reason: format!("Before sending, the batch of {n} you were shown needs your approval. Nothing was sent") }));
    }
    if let Err(m) = check_plan(p) { return BatchReport::halted(n, Written::new(None, Outcome::Rejected { reason: format!("{m}. Nothing was sent") })) }
    let before = match sent_24h(c, now, p.daily_limit) { Ok(k) => k, Err(f) => return BatchReport::halted(n, read_fail(f, "the number Shibubu sent in 24 hours (the daily limit)")) };
    let mut r = BatchReport { done: vec![], skipped: vec![], stopped: None, remaining: 0, sent_24h_before: Some(before) };
    for (i, d) in p.drafts.iter().enumerate() {
        if before + r.done.len() >= p.daily_limit {
            r.stopped = Some(Step { to: String::new(), key: String::new(), result: Written::new(None, Outcome::Rejected { reason: format!(
                "The daily limit of {} was reached ({before} sent by Shibubu in 24 hours + {} in this run). Run again with the same approval after the oldest sends are more than 24 hours old (recipients already sent are skipped)",
                p.daily_limit, r.done.len()) }) });
            r.remaining = n - i;
            return r;
        }
        match gmail::kangae_sent(c, &format!("in:sent to:{}", d.to), 1) {
            Ok(found) if !found.is_empty() => {
                let reason = if found[0].key == d.key { "Already sent with this approval (not sent again)" } else { "Shibubu has sent to this recipient before (it never sends twice to the same recipient)" };
                r.skipped.push(Skip { who: d.to.clone(), reason: reason.to_string() });
                continue;
            }
            Ok(_) => {}
            Err(f) => { r.stopped = Some(Step { to: String::new(), key: String::new(), result: read_fail(f, &format!("whether it was sent before to {} (it wasn't sent)", d.to)) }); r.remaining = n - i; return r }
        }
        let w = gmail::send_checked(c, d);
        let step = Step { to: d.to.clone(), key: d.key.clone(), result: w };
        if step.result.verified() { r.done.push(step) } else { r.stopped = Some(step); r.remaining = n - i - 1; return r }
    }
    r
}

/// Gmail に下書きを作る（送らない）。1 回の ConsentReceipt（drafts_digest への Approved）で N 件。最初の非 Verified で止める
pub fn create_drafts(c: &Client, p: &Plan, consent: Option<&ConsentReceipt>) -> BatchReport {
    let n = p.drafts.len();
    if !approved(consent, &bundle(p, "external_write")) {
        return BatchReport::halted(n, Written::new(None, Outcome::Rejected { reason: format!("Before creating drafts, the batch of {n} you were shown needs your approval. No draft was created") }));
    }
    if let Err(m) = check_plan(p) { return BatchReport::halted(n, Written::new(None, Outcome::Rejected { reason: format!("{m}. No draft was created") })) }
    let mut r = BatchReport { done: vec![], skipped: vec![], stopped: None, remaining: 0, sent_24h_before: None };
    for (i, d) in p.drafts.iter().enumerate() {
        let step = Step { to: d.to.clone(), key: d.key.clone(), result: gmail::create_draft_checked(c, d) };
        if step.result.verified() { r.done.push(step) } else { r.stopped = Some(step); r.remaining = n - i - 1; return r }
    }
    r
}

/// 送った 1 件の返信（sent = false はまだ送っていない）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplyRow {
    pub to: String,
    pub key: String,
    pub sent: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<gmail::Reply>,
    /// この行を読めなかった理由（他の行は続けて読む）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}
/// plan の各宛先: 送信済みから X-Kangae-Key で送った message を引き、その thread の相手からの返信の件数と最新 1 通（read だけ）
pub fn replies(c: &Client, p: &Plan) -> Vec<ReplyRow> {
    p.drafts.iter().map(|d| {
        let row = ReplyRow { to: d.to.clone(), key: d.key.clone(), sent: false, thread_id: String::new(), count: 0, latest: None, error: String::new() };
        let found = match gmail::kangae_sent(c, &format!("in:sent to:{}", d.to), 50) {
            Ok(v) => v.into_iter().find(|s| s.key == d.key),
            Err(f) => return ReplyRow { error: format!("Can't read Sent for {}: {}", d.to, f.message), ..row },
        };
        let Some(s) = found else { return row };
        match gmail::thread_replies(c, &s.thread_id, Some(&s.id)) {
            Ok(rep) => ReplyRow { sent: true, thread_id: s.thread_id, count: rep.count, latest: rep.latest, ..row },
            Err(e) => ReplyRow { sent: true, thread_id: s.thread_id, error: e, ..row },
        }
    }).collect()
}
