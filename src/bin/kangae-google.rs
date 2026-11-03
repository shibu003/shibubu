//! kangae-google — Google（Gmail / Calendar）接続の CLI。test1 / test2（docs/ACCEPTANCE.md §3）の土台。
//! token は macOS Keychain（service kangae-google）。予定・下書き・送信は「見せる → 見せた本文の digest で承認 → 作る / 送る → 読み戻す」。
//! 送るのは outreach-send（承認された束の N 件）だけ。
#[path = "../crypto.rs"]
mod crypto;
#[path = "../domain/mod.rs"]
mod domain;
#[path = "../google/mod.rs"]
mod google;
use domain::permission::{ConsentDecision, ConsentReceipt, ConsentSubject};
use google::{calendar, contacts, digest, gmail, local_offset, now, oauth, outreach, rfc3339_utc, Client, Written};
use serde_json::{json, Value};
use std::{collections::HashMap, io::Read, process::exit};

const USAGE: &str = "使い方: kangae-google <command> [--key value ...]
  auth                                         ブラウザで Google に同意 → token を Keychain へ
  status                                       token の有無と期限（通信しない）
  gmail-threads [--query \"is:unread in:inbox\"] [--max 20]
                                               thread の一覧（id・返信数・差出人・件名）。既定の query は newer_than:7d
  gmail-draft (--to A --subject S | --thread T [--to A]) --body B [--key K --approve D]
                                               下書き（--thread は返信）。--approve 無し = 見せるだけ。送信はしない
  gmail-draft-check (gmail-draft と同じ引数) --key K
                                               その下書きが在るか（作られたか分からなかった時）
  gmail-draft-delete --id ID                   Kangae が作った下書きだけ消す
  gcal-events [--days 7 | --today]             予定の一覧（開始・終了・件名・場所）
  gcal-create --summary S --start T --end T [--attendee E ...] [--description D] [--id ID --approve D]
                                               予定。--approve 無し = 見せるだけ。参加者が居ると Google が招待 mail を送る
  gcal-check (gcal-create と同じ引数) --id ID   その予定が在るか（作られたか分からなかった時）
  gcal-delete --id ID                          Kangae が作った予定（id が kangae…）だけ消す
  yc-open                                      YC の会社一覧（ycombinator.com/companies）を本人の既定の browser で開く（Kangae は YC を読まない）
  outreach-import [--linkedin Connections.csv] [--paste F|-] [--yc F|-] [--out 表.tsv]
                                               宛先表（名前・会社・役職・mail・出典。mail が無い人も残す・同じ mail は 1 行）。通信しない。
                                               --yc = yc-open で選んだ会社の貼り付け（1 行 1 社: 名前・https の site・batch）か name と website の列の CSV / JSON
  outreach-import --yc-fetch [--yc-pages 300]  YC の一覧を api.ycombinator.com から自動で取る（既定 off。YC の利用規約に当たる。owner の判断待ち）
  outreach-plan --table 表.tsv --template T [--sender 名前] [--limit 20] [--out plan.json]
                                               相手ごとの下書きを見せる（template の 1 行目は「件名: …」・{first_name} 等を差し込む）。通信しない。
                                               送り手の名前・住所・連絡先は ~/.kangae/sender.json（{\"name\", \"address\", \"contact\"}）。template の最後に
                                               {sender_name} {sender_address} {sender_contact} と受信拒否の 1 行が無いと止める（特定電子メール法・CAN-SPAM）
  outreach-drafts --plan P [--approve D]       plan の下書きを Gmail に作る（送らない）
  outreach-send --plan P [--approve D]         plan の N 件を送る。--approve 無し = 件数と今日の残りを見せるだけ
  outreach-replies --plan P                    送った各宛先の返信の件数と最新 1 通
  gmail-replies --thread T [--after MESSAGE_ID]  thread の相手からの返信の件数と最新 1 通
  env: KANGAE_GOOGLE_CLIENT_ID / KANGAE_GOOGLE_CLIENT_SECRET（手順: docs/internal/connectors/google.md）
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { eprint!("{USAGE}"); exit(2) };
    if let Err(e) = run(cmd, &opts(&args[1..])) { eprintln!("エラー: {e}"); exit(1) }
}
/// --key value の列。同じ key は複数回可（--attendee）。次が -- で始まる・無い key は値なしの印（--today）
fn opts(args: &[String]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            let v = if it.peek().is_some_and(|n| !n.starts_with("--")) { it.next().cloned().unwrap_or_default() } else { String::new() };
            out.entry(k.to_string()).or_default().push(v)
        }
    }
    out
}
/// Keychain の token（切れていれば refresh。Keychain に書くのは refresh token が変わった時だけ）
fn client() -> Result<Client, String> {
    let cfg = oauth::Config::from_env()?;
    let tokens = oauth::keychain_load()?.ok_or("未認証です。先に `kangae-google auth` を実行してください。")?;
    let (tokens, changed) = oauth::refresh_if_needed(&cfg, oauth::TOKEN_URL, tokens)?;
    // 保存に失敗しても、更新した token でこの 1 回は続ける
    if changed { if let Err(e) = oauth::keychain_save(&tokens) { eprintln!("注意: Google が新しくした refresh token を保存できませんでした（{e}）。この command は続けます。") } }
    Ok(Client::new(tokens.access_token))
}
/// CLI の承認 = 見せた本文の digest を人が打ち返した事
fn consent(action_ref: &str, digest: &str) -> ConsentReceipt {
    ConsentReceipt { id: format!("cli-{}", &digest[..12]), app_id: "kangae-google".into(), subject: ConsentSubject::Action { action_ref: action_ref.into(), run_id: None },
        scopes: vec![], payload_digest: Some(digest.into()), decision: ConsentDecision::Approved, decided_at: rfc3339_utc(now()) }
}
/// 見せるだけ（作らない）。次の一手は stderr
fn preview(shown: Value, digest: &str, add: &str) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(&shown).map_err(|e| e.to_string())?);
    eprintln!("まだ作っていません。作るには同じ引数に `{add} --approve {digest}` を足してください。");
    Ok(())
}
fn read_file(path: &str) -> Result<String, String> {
    if path == "-" { let mut s = String::new(); std::io::stdin().read_to_string(&mut s).map_err(|e| format!("標準入力を読めません（{e}）。"))?; return Ok(s) }
    std::fs::read_to_string(path).map_err(|e| format!("{path} を読めません（{e}）。"))
}
/// 書きかけを残さない（一時 file → rename）
fn write_file(path: &str, text: &str) -> Result<(), String> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{path} に書けません（{e}）。"))
}
/// 送り手の表示: ~/.kangae/sender.json（無ければ空 → check_template が何を書くかを返す）。--sender は名前だけ上書き
fn sender(name: Option<String>) -> Result<outreach::Sender, String> {
    let path = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".kangae/sender.json"));
    let mut s: outreach::Sender = match path.map(std::fs::read_to_string) {
        Some(Ok(t)) => serde_json::from_str(&t).map_err(|e| format!("{} が読めません（{e}）。{{\"name\": \"…\", \"address\": \"…\", \"contact\": \"…\"}} の形にしてください。", outreach::SENDER_FILE))?,
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{} を読めません（{e}）。", outreach::SENDER_FILE)),
        _ => outreach::Sender::default(),
    };
    if let Some(n) = name { s.name = n }
    Ok(s)
}
fn read_plan(path: &str) -> Result<outreach::Plan, String> {
    serde_json::from_str(&read_file(path)?).map_err(|e| format!("{path} は outreach-plan の出力ではありません（{e}）。"))
}
fn write_plan(path: &str, p: &outreach::Plan) -> Result<(), String> { write_file(path, &(serde_json::to_string_pretty(p).map_err(|e| e.to_string())? + "\n")) }
/// 一括の結果を print。止まっていれば exit 1（何件済んで何件残ったか）
fn batch_report(r: &outreach::BatchReport, verb: &str) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(r).map_err(|e| e.to_string())?);
    if r.finished() { eprintln!("{}", r.summary(verb)); Ok(()) } else { Err(r.summary(verb)) }
}
/// Receipt の材料を print。Verified でなければ exit 1（理由と次の一手）
fn report(w: Written) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(&w).map_err(|e| e.to_string())?);
    if w.verified() { Ok(()) } else { Err(w.why()) }
}
fn run(cmd: &str, o: &HashMap<String, Vec<String>>) -> Result<(), String> {
    let one = |k: &str| o.get(k).and_then(|v| v.last()).cloned().filter(|v| !v.trim().is_empty());
    let need = |k: &str| one(k).ok_or_else(|| format!("--{k} が必要です。\n{USAGE}"));
    let num = |k: &str, default: u64| -> Result<u64, String> {
        match one(k) { None => Ok(default), Some(v) => v.parse().map_err(|_| format!("--{k} は整数で指定してください（{v}）。")) }
    };
    let approve = one("approve");
    match cmd {
        "auth" => {
            let cfg = oauth::Config::from_env()?;
            let t = oauth::authorize(&cfg)?;
            oauth::keychain_save(&t)?;
            println!("認証できました。token は macOS Keychain（service: kangae-google）に保存しました。\nscope: {}\n次: kangae-google gmail-threads --query \"is:unread in:inbox\"", t.scope);
        }
        "status" => match oauth::keychain_load()? {
            None => println!("未認証です。`kangae-google auth` を実行してください。"),
            Some(t) => println!("認証済み（Keychain service: kangae-google）。access token の期限: {}{}\nscope: {}",
                rfc3339_utc(t.expires_at), if t.expired(now()) { "（切れている → 呼び出しのたびに refresh token で取り直す）" } else { "" }, t.scope),
        },
        "gmail-threads" => {
            let query = one("query").unwrap_or_else(|| "newer_than:7d".to_string());
            let max = num("max", 20)? as usize;
            let threads = gmail::list_threads(&client()?, &query, max)?;
            if threads.is_empty() { println!("該当する thread はありません（query: {query}）。") }
            for t in &threads { println!("{}\t返信 {}\t{}\t{}", t.id, t.replies, t.from, t.subject) }
            eprintln!("{} 件（id\t返信数\t差出人\t件名）", threads.len());
        }
        "gmail-draft" | "gmail-draft-check" => {
            let key = match one("key") {
                Some(k) => k,
                None if cmd == "gmail-draft" && approve.is_none() => gmail::new_key()?,
                None => return Err("--key が必要です（見せた時に出た key）。".to_string()),
            };
            let d = match one("thread") {
                Some(t) => gmail::reply_draft(&client()?, &t, one("to").as_deref(), &need("body")?, &key)?,
                None => gmail::Draft { to: need("to")?, subject: need("subject")?, body: need("body")?, thread_id: String::new(), in_reply_to: String::new(), references: String::new(), key: key.clone() },
            };
            if cmd == "gmail-draft-check" { return report(gmail::check_draft_by_key(&client()?, &d)) }
            let shown = serde_json::to_value(&d).map_err(|e| e.to_string())?;
            let dg = digest(&shown);
            match approve {
                None => preview(json!({"effect": "external_write", "draft": shown, "digest": dg}), &dg, &format!("--key {key}"))?,
                Some(a) if a != dg => return Err("--approve の digest が今の中身と違います（thread に新しい message が来た等）。見せた物と違う物は作りません。--approve 無しでもう一度見てください。".to_string()),
                Some(_) => return report(gmail::create_draft_checked(&client()?, &d)),
            }
        }
        "gmail-draft-delete" => return report(gmail::delete_draft(&client()?, &need("id")?)),
        "gcal-events" => {
            let (t0, t1) = if o.contains_key("today") { calendar::today(now(), local_offset().ok_or("local の時差（date +%z）を読めません。")?) }
                           else { let t = now(); (t, t + num("days", 7)?.clamp(1, 366) * 86400) };
            let events = calendar::list_events_between(&client()?, t0, t1)?;
            if events.is_empty() { println!("{} 〜 {} の予定はありません。", rfc3339_utc(t0), rfc3339_utc(t1)) }
            for e in &events { println!("{}\t{}\t{}{}", e.start, e.end, e.summary, if e.location.is_empty() { String::new() } else { format!("\t@ {}", e.location) }) }
        }
        "gcal-create" | "gcal-check" => {
            let attendees: Vec<String> = o.get("attendee").cloned().unwrap_or_default().into_iter().filter(|a| !a.is_empty()).collect();
            let mut req = calendar::event_request(&need("summary")?, &need("start")?, &need("end")?, &attendees, &one("description").unwrap_or_default())?;
            let id = match one("id") {
                Some(id) => id,
                None if cmd == "gcal-create" && approve.is_none() => calendar::new_event_id()?,
                None => return Err("--id が必要です（見せた時に出た id）。".to_string()),
            };
            req["body"]["id"] = json!(id);
            if cmd == "gcal-check" { return report(calendar::check_event(&client()?, &req["body"])) }
            let dg = digest(&req["body"]);
            match approve {
                None => {
                    if !attendees.is_empty() { eprintln!("参加者が居ます（external_send）: 承認すると Google が {} 人に招待 mail を送ります。", attendees.len()) }
                    preview(json!({"effect": req["effect"], "body": req["body"], "digest": dg}), &dg, &format!("--id {id}"))?
                }
                Some(a) if a != dg => return Err("--approve の digest が今の本文と違います。見せた物と違う物は作りません。--approve 無しでもう一度見てください。".to_string()),
                Some(_) => return report(calendar::create_event(&client()?, &req["body"], Some(&consent("calendar.events.create", &dg)))),
            }
        }
        "gcal-delete" => return report(calendar::delete_event(&client()?, &need("id")?)),
        "yc-open" => {
            // handoff（yc-directory.md 判断 1 の既定）: 開くのは本人の browser。Kangae は YC に HTTP を打たない
            let url = "https://www.ycombinator.com/companies";
            let ok = std::process::Command::new("/usr/bin/open").arg(url).status().is_ok_and(|s| s.success());
            if !ok { return Err(format!("既定の browser で開けませんでした。browser で {url} を開いてください。")) }
            eprintln!("本人の既定の browser で {url} を開きました。\n送りたい会社を選び、1 行 1 社で貼ってください（例: Airbnb https://airbnb.com W09。会社 page の URL だけでも可）:\n  kangae-google outreach-import --yc - --out 表.tsv   （貼り付けて Ctrl-D）\nYC は mail を公開していません。mail は相手の site 等で本人が確かめて表に足してください（Kangae は推測しません）。");
        }
        "outreach-import" => {
            // 取り込んだ日は local の日付（UTC だと夕方以降が翌日になる）
            let date = &rfc3339_utc(now().saturating_add_signed(local_offset().unwrap_or(0)))[..10];
            let files = |k: &str| o.get(k).cloned().unwrap_or_default().into_iter().filter(|f| !f.is_empty()).collect::<Vec<_>>();
            let mut lists = Vec::new();
            for f in files("linkedin") { lists.push(contacts::from_linkedin(&read_file(&f)?, &format!("LinkedIn Connections.csv（{date} 取り込み）")).map_err(|e| format!("{f}: {e}"))?) }
            for f in files("paste") { lists.push(contacts::from_paste(&read_file(&f)?, &format!("貼り付け（{date}）")).map_err(|e| format!("{f}: {e}"))?) }
            for f in files("yc") { lists.push(contacts::from_yc(&read_file(&f)?, &format!("YC directory（{date} 本人の貼り付け）")).map_err(|e| format!("{f}: {e}"))?) }
            if o.contains_key("yc-fetch") {
                eprintln!("注意: --yc-fetch は YC の一覧を自動で取ります。YC の利用規約（https://www.ycombinator.com/legal）は robots・scraping・data の自動取得を禁じています（owner の判断待ちの opt-in）。");
                // 試験の口は 127.0.0.1 だけ（src/github.rs と同じ）
                let base = std::env::var("KANGAE_YC_API_BASE").ok().filter(|b| b.starts_with("http://127.0.0.1")).unwrap_or_else(|| contacts::YC_API_BASE.to_string());
                lists.push(contacts::yc_fetch(base.trim_end_matches('/'), num("yc-pages", 300)?.clamp(1, 1000) as usize, &format!("YC directory（{date} 自動取得）"))?);
            }
            if lists.is_empty() { return Err(format!("--linkedin / --paste / --yc のどれかに file を渡してください（YC の会社は先に `kangae-google yc-open` で開いて選ぶ）。\n{USAGE}")) }
            let (rows, merged) = contacts::merge(lists);
            let tsv = contacts::to_tsv(&rows);
            match one("out") { Some(out) => write_file(&out, &tsv)?, None => print!("{tsv}") }
            let with_mail = rows.iter().filter(|r| !r.mail.is_empty()).count();
            eprintln!("{} 行（mail あり {with_mail}・{} {}・同じ mail をまとめた {merged}）。次: kangae-google outreach-plan --table <表> --template <template> --sender \"<送り手の名前>\"",
                rows.len(), contacts::NO_MAIL, rows.len() - with_mail);
        }
        "outreach-plan" => {
            let rows = contacts::from_paste(&read_file(&need("table")?)?, "宛先表")?;
            let t = outreach::parse_template(&read_file(&need("template")?)?)?;
            let p = outreach::plan(&rows, &t, &sender(one("sender"))?, num("limit", outreach::DEFAULT_DAILY_LIMIT as u64)? as usize)?;
            let out = one("out").unwrap_or_else(|| "outreach-plan.json".to_string());
            write_plan(&out, &p)?;
            for (i, d) in p.drafts.iter().enumerate() { println!("── {}/{}  {}\n件名: {}\n\n{}\n", i + 1, p.drafts.len(), d.to, d.subject, d.body) }
            // 外した行は 20 行まで並べ、残りは理由ごとの件数（YC の会社一覧は全部「mail なし」）
            for s in p.skipped.iter().take(20) { println!("外した: {}（{}）", s.who, s.reason) }
            if p.skipped.len() > 20 {
                let mut by: Vec<(&str, usize)> = Vec::new();
                for s in &p.skipped[20..] { match by.iter_mut().find(|(r, _)| *r == s.reason) { Some(x) => x.1 += 1, None => by.push((&s.reason, 1)) } }
                println!("外した: ほか {} 件（{}）", p.skipped.len() - 20, by.iter().map(|(r, n)| format!("{r} {n}")).collect::<Vec<_>>().join("・"));
            }
            eprintln!("下書き {} 件・外した {} 件。まだ作っても送ってもいません（plan: {out}）。\n  Gmail に下書きを作る（送らない）: kangae-google outreach-drafts --plan {out} --approve {}\n  送る（{} 件・1 日の上限 {}）: kangae-google outreach-send --plan {out} --approve {}",
                p.drafts.len(), p.skipped.len(), outreach::drafts_digest(&p), p.drafts.len(), p.daily_limit, outreach::send_digest(&p));
        }
        "outreach-drafts" => {
            let path = need("plan")?;
            let mut p = read_plan(&path)?;
            let dg = outreach::drafts_digest(&p);
            match approve {
                None => {
                    outreach::check_plan(&p)?;
                    println!("{}", serde_json::to_string_pretty(&json!({"effect": "external_write", "drafts": p.drafts.len(), "to": p.drafts.iter().map(|d| &d.to).collect::<Vec<_>>(), "digest": dg})).map_err(|e| e.to_string())?);
                    eprintln!("まだ作っていません。{} 件の下書きを Gmail に作るには `kangae-google outreach-drafts --plan {path} --approve {dg}`（送りません）。", p.drafts.len());
                }
                Some(a) if a != dg => return Err("--approve の digest が今の plan と違います（plan を書き換えた等）。見せた物と違う物は作りません。--approve 無しでもう一度見てください。".to_string()),
                Some(_) => {
                    let r = outreach::create_drafts(&client()?, &p, Some(&consent("gmail.drafts.create", &dg)));
                    for s in &r.done { if let Some(id) = &s.result.id { p.gmail_drafts.insert(s.key.clone(), id.clone()); } }
                    write_plan(&path, &p)?;
                    let existed = r.done.iter().filter(|s| s.result.detail.contains("既に在りました")).count();
                    let res = batch_report(&r, "作った");
                    if existed > 0 { eprintln!("うち {existed} 件は同じ key の下書きが既に在りました（作り直していません）。") }
                    return res;
                }
            }
        }
        "outreach-send" => {
            let path = need("plan")?;
            let mut p = read_plan(&path)?;
            let dg = outreach::send_digest(&p);
            match approve {
                None => {
                    outreach::check_plan(&p)?;
                    let before = outreach::sent_24h(&client()?, now(), p.daily_limit).map_err(|f| format!("{}24 時間に Kangae が送った数を数えられません。", f.message))?;
                    let left = p.daily_limit.saturating_sub(before);
                    let shown = json!({"effect": "external_send", "count": p.drafts.len(), "daily_limit": p.daily_limit, "kangae_sent_24h": before, "left_today": left,
                        "drafts": p.drafts.iter().map(|d| json!({"to": d.to, "subject": d.subject, "key": d.key})).collect::<Vec<_>>(), "digest": dg});
                    if left < p.drafts.len() { eprintln!("今日の残りは {left} 件です。承認すると {left} 件送って止まり、残りは上限が空いてから同じ承認で送ります。") }
                    println!("{}", serde_json::to_string_pretty(&shown).map_err(|e| e.to_string())?);
                    eprintln!("まだ送っていません。{} 件を送るには `kangae-google outreach-send --plan {path} --approve {dg}`（Kangae が前に送った宛先は飛ばします）。", p.drafts.len());
                }
                Some(a) if a != dg => return Err("--approve の digest が今の plan と違います（plan を書き換えた等）。見せた物と違う物は送りません。--approve 無しでもう一度見てください。".to_string()),
                Some(_) => {
                    let c = client()?;
                    let r = outreach::send_plan(&c, &p, Some(&consent("gmail.messages.send", &dg)), now());
                    // outreach-drafts で作った同じ key の下書きは、送った後に消す（残すと手で 2 回目を送れてしまう）
                    for s in &r.done {
                        let Some(id) = p.gmail_drafts.get(&s.key).cloned() else { continue };
                        let w = gmail::delete_draft(&c, &id);
                        if w.verified() || matches!(w.outcome, domain::run::Outcome::Failed { error: domain::run::ErrorCode::TargetNotFound }) { p.gmail_drafts.remove(&s.key); }
                        else { eprintln!("送った {} の Gmail の下書き {id} を消せません（{}）。`kangae-google gmail-draft-delete --id {id}` で消してください。", s.to, w.why()) }
                    }
                    write_plan(&path, &p)?;
                    return batch_report(&r, "送った");
                }
            }
        }
        "outreach-replies" => {
            let rows = outreach::replies(&client()?, &read_plan(&need("plan")?)?);
            println!("{}", serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            let (sent, failed) = (rows.iter().filter(|r| r.sent).count(), rows.iter().filter(|r| !r.error.is_empty()).count());
            eprintln!("送った {sent} 件のうち返信あり {} 件（返信 計 {} 通）・まだ送っていない {} 件。", rows.iter().filter(|r| r.count > 0).count(), rows.iter().map(|r| r.count).sum::<usize>(), rows.len() - sent);
            if failed > 0 { return Err(format!("{failed} 件は読めませんでした（各行の error）。")) }
        }
        "gmail-replies" => {
            let r = gmail::thread_replies(&client()?, &need("thread")?, one("after").as_deref())?;
            println!("{}", serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?);
            eprintln!("返信 {} 通。", r.count);
        }
        _ => return Err(format!("不明な command: {cmd}\n{USAGE}")),
    }
    Ok(())
}
