//! 宛先表: LinkedIn の Connections.csv・貼り付けの表（TSV / CSV / 「名前 <mail>」）・YC の会社一覧 → 1 つの表（Contact の列）。
//! 通信するのは yc_fetch だけ（bin が --yc-fetch の opt-in の時だけ呼ぶ）。
//! LinkedIn への login・scraping・API は使わない（本人が書き出した file か貼り付けだけ）。mail の推測もしない（無い人は「mail なし」で残す）。
//! 出典（2026-10-17 取得）:
//!   https://www.linkedin.com/help/linkedin/answer/a1339364 「Connections: Contains the first and last name, public profile URL, email address,
//!     company, position, and connection date」「An email address for the connection will be included only when authorized by the connection」
//!   https://www.linkedin.com/help/linkedin/answer/a566336 「You will only see email addresses for connections who have allowed …」
//!   列の literal・順序・file 先頭の「Notes:」の注意書きは LinkedIn の一次資料に無い（第三者の guide は順序も日付の形も割れる）→
//!   先頭の行を読み飛ばして First Name と Last Name を含む行を見出しにし、列は名前で引く（順序を決め打ちしない）
//!   YC: 既定は handoff（yc-open で本人の browser に directory を開き、本人が選んだ会社を貼る）。YC の Terms of Use（https://www.ycombinator.com/legal）は
//!     「data mining, robots, scraping or similar data gathering or extraction methods」を禁じる → 自動取得（yc_fetch）は owner の判断待ちの opt-in の後ろだけ
//!     （docs/internal/connectors/yc-directory.md 判断 1）。使うのは会社名・website・batch だけ（創業者の名前や連絡先は取らない）
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// YC が運営する無文書の endpoint（25 社 / 頁・totalPages・2026-10-16 実測。文書・quota の記載は無い）
pub const YC_API_BASE: &str = "https://api.ycombinator.com";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    #[serde(default)] pub name: String,
    #[serde(default)] pub first_name: String,
    #[serde(default)] pub last_name: String,
    #[serde(default)] pub company: String,
    #[serde(default)] pub title: String,
    /// 空 = mail なし（表では「mail なし」と書く）
    #[serde(default)] pub mail: String,
    /// LinkedIn の profile か会社の site
    #[serde(default)] pub url: String,
    #[serde(default)] pub batch: String,
    #[serde(default)] pub source: String,
}
/// 宛先表の列（TSV の見出し）
pub const COLUMNS: [&str; 9] = ["name", "first_name", "last_name", "company", "title", "mail", "url", "batch", "source"];
pub const NO_MAIL: &str = "No email";

impl Contact {
    pub fn field(&self, k: &str) -> Option<&str> {
        Some(match k {
            "name" => &self.name, "first_name" => &self.first_name, "last_name" => &self.last_name, "company" => &self.company,
            "title" => &self.title, "mail" => &self.mail, "url" => &self.url, "batch" => &self.batch, "source" => &self.source, _ => return None,
        })
    }
    fn field_mut(&mut self, k: &str) -> Option<&mut String> {
        Some(match k {
            "name" => &mut self.name, "first_name" => &mut self.first_name, "last_name" => &mut self.last_name, "company" => &mut self.company,
            "title" => &mut self.title, "mail" => &mut self.mail, "url" => &mut self.url, "batch" => &mut self.batch, "source" => &mut self.source, _ => return None,
        })
    }
    /// 人に見せる 1 語（名前 → 会社 → mail）
    pub fn who(&self) -> &str { [&self.name, &self.company, &self.mail].into_iter().find(|s| !s.is_empty()).map(|s| s.as_str()).unwrap_or("(no name)") }
    fn is_empty(&self) -> bool { COLUMNS.iter().all(|k| k == &"source" || self.field(k).is_none_or(str::is_empty)) }
    /// 名前の欄を揃える: name だけなら分ける・first / last だけなら繋ぐ
    fn complete_name(mut self) -> Self {
        if self.name.is_empty() {
            // 漢字・かなの名前は姓を先に（LinkedIn の First Name は名・Last Name は姓）
            let (a, b) = if self.last_name.chars().any(is_cjk) { (&self.last_name, &self.first_name) } else { (&self.first_name, &self.last_name) };
            self.name = format!("{a} {b}").trim().to_string()
        }
        if self.first_name.is_empty() && self.last_name.is_empty() { (self.first_name, self.last_name) = split_name(&self.name) }
        self
    }
}

/// RFC 4180: "…" の中の , と改行と ""（= "）。CRLF / LF。先頭の BOM は捨てる。sep は ',' か '\t'
pub fn parse_csv(text: &str, sep: char) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut chars) = (false, text.trim_start_matches('\u{feff}').chars().peekable());
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => { chars.next(); cell.push('"') }
            '"' if quoted => quoted = false,
            '"' if cell.trim().is_empty() => { cell.clear(); quoted = true }
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' if !quoted => { row.push(std::mem::take(&mut cell)); rows.push(std::mem::take(&mut row)) }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() { row.push(cell); rows.push(row) }
    rows
}

/// mail の形（@ が 1 つ・domain に . ・空白や区切り文字なし）。違えば空（推測で直さない）
pub fn clean_mail(s: &str) -> String {
    let s = s.trim().trim_start_matches("mailto:").trim();
    let ok = s.split('@').count() == 2 && !s.contains(|c: char| c.is_whitespace() || ",;<>\"()[]".contains(c))
        && s.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'));
    if ok { s.to_string() } else { String::new() }
}
fn is_cjk(c: char) -> bool { matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF) }
/// (first, last)。漢字・かな・ハングルの名前は姓が先（空白が無ければ分けない = 両方空）。「Doe, Jane」は姓, 名。他は最初の語が first
pub fn split_name(full: &str) -> (String, String) {
    if let Some((last, first)) = full.split_once(',').filter(|_| !full.chars().any(is_cjk)) {
        return (first.trim().to_string(), last.trim().to_string());
    }
    let words: Vec<&str> = full.split_whitespace().collect();
    match (full.chars().any(is_cjk), words.as_slice()) {
        (_, []) => (String::new(), String::new()),
        (true, [_]) => (String::new(), String::new()),
        (true, [last, rest @ ..]) => (rest.join(" "), last.to_string()),
        (false, [first, rest @ ..]) => (first.to_string(), rest.join(" ")),
    }
}

/// 見出しの名前 → Contact の欄（大文字小文字・前後の空白・_ と空白の違いを見ない）
fn column_of(head: &str) -> Option<&'static str> {
    let h = head.trim().to_lowercase().replace(['_', '-'], " ");
    Some(match h.as_str() {
        "name" | "full name" | "名前" | "氏名" => "name",
        "first name" | "firstname" | "given name" | "名" => "first_name",
        "last name" | "lastname" | "family name" | "surname" | "姓" => "last_name",
        "company" | "organization" | "会社" | "会社名" | "所属" => "company",
        "title" | "position" | "job title" | "役職" | "肩書" => "title",
        "mail" | "email" | "e mail" | "email address" | "e mail address" | "メール" | "メールアドレス" => "mail",
        "url" | "profile url" | "website" | "site" | "サイト" => "url",
        "batch" => "batch",
        "source" | "出典" => "source",
        _ => return None,
    })
}
/// 見出し行と列の対応 → 各行を Contact に。source の列が無い・空の行は default_source
fn rows_to_contacts(head: &[String], rows: &[Vec<String>], default_source: &str) -> Vec<Contact> {
    let cols: Vec<Option<&str>> = head.iter().map(|h| column_of(h)).collect();
    rows.iter().filter_map(|r| {
        let mut c = Contact::default();
        for (v, col) in r.iter().zip(&cols) {
            if let Some(f) = col.and_then(|k| c.field_mut(k)) { *f = v.trim().to_string() }
        }
        c.mail = if c.mail == NO_MAIL { String::new() } else { clean_mail(&c.mail) };
        if c.source.is_empty() { c.source = default_source.to_string() }
        (!c.is_empty()).then(|| c.complete_name())
    }).collect()
}

/// LinkedIn の「データのコピーを取得」の Connections.csv。先頭の注意書き（Notes: …）を読み飛ばし、First Name と Last Name を含む行を見出しにする
pub fn from_linkedin(text: &str, source: &str) -> Result<Vec<Contact>, String> {
    let rows = parse_csv(text, ',');
    let has = |r: &Vec<String>, k: &str| r.iter().any(|h| column_of(h) == Some(k));
    let at = rows.iter().position(|r| has(r, "first_name") && has(r, "last_name"))
        .ok_or("The headings of LinkedIn's Connections.csv (First Name, Last Name) weren't found. Give the Connections.csv exported with LinkedIn's Settings → Data privacy → Get a copy of your data")?;
    Ok(rows_to_contacts(&rows[at], &rows[at + 1..], source))
}

/// 貼り付け: TSV（表計算から copy）/ 見出し付き CSV / 「名前 <mail>」の並び（改行・; ・> の後の , で区切る）/ mail だけの行。
/// 見出しが無い TSV / CSV は、@ を含む cell を mail、残りを順に 名前・会社・役職 と読む。宛先表（to_tsv の出力）もこれで読み戻す
pub fn from_paste(text: &str, source: &str) -> Result<Vec<Contact>, String> {
    let first = text.lines().find(|l| !l.trim().is_empty()).ok_or("Nothing was pasted")?;
    let sep = if first.contains('\t') { Some('\t') } else if first.contains('<') && first.contains('@') { None } else if first.contains(',') { Some(',') } else { None };
    let Some(sep) = sep else { return Ok(addresses(text, source)) };
    let rows: Vec<Vec<String>> = parse_csv(text, sep).into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect();
    let Some(head) = rows.first() else { return Err("The paste has no line with content".into()) };
    let headed = head.iter().filter(|h| column_of(h).is_some()).count() >= 2 || head.iter().any(|h| column_of(h) == Some("mail"));
    if headed { return Ok(rows_to_contacts(head, &rows[1..], source)) }
    Ok(rows.iter().filter_map(|r| {
        let mail = r.iter().map(|c| clean_mail(c)).find(|m| !m.is_empty()).unwrap_or_default();
        let mut rest = r.iter().map(|c| c.trim()).filter(|c| !c.is_empty() && !c.contains('@'));
        let c = Contact { name: rest.next().unwrap_or("").into(), company: rest.next().unwrap_or("").into(), title: rest.next().unwrap_or("").into(), mail, source: source.into(), ..Default::default() };
        (!c.is_empty()).then(|| c.complete_name())
    }).collect())
}
/// 「"山田, 太郎" <t@x.io>, Hanako <h@x.io>」→ 2 件
fn addresses(text: &str, source: &str) -> Vec<Contact> {
    let (mut entries, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for ch in text.chars() {
        match ch {
            '"' => { quoted = !quoted; cur.push(ch) }
            '\n' | ';' if !quoted => entries.push(std::mem::take(&mut cur)),
            ',' if !quoted && (cur.trim_end().ends_with('>') || (cur.contains('@') && !cur.contains('<'))) => entries.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    entries.push(cur);
    entries.iter().filter_map(|e| {
        let e = e.trim();
        let (name, mail) = match (e.rfind('<'), e.rfind('>')) {
            (Some(a), Some(b)) if a < b => (e[..a].trim().trim_matches('"').trim(), &e[a + 1..b]),
            _ => ("", e),
        };
        let c = Contact { name: name.to_string(), mail: clean_mail(mail), source: source.to_string(), ..Default::default() };
        (!c.is_empty()).then(|| c.complete_name())
    }).collect()
}

/// YC の会社一覧 → 会社名・site・batch だけ（人の名前も mail も作らない）。読める形:
///   本人の貼り付け（handoff: yc-open で開いた directory から選んだ会社を 1 行 1 社。名前・http(s) の site・batch（W21 / Summer 2021）を tab か空白で。会社 page の URL だけでも可）
///   見出しに name と website の在る CSV / TSV・{companies | hits | […]} の JSON（yc_fetch・site-api の recipe・yc-oss の出力）
pub fn from_yc(text: &str, source: &str) -> Result<Vec<Contact>, String> {
    let company = |name: &str, site: &str, batch: &str| Contact { company: name.trim().into(), url: site.trim().into(), batch: batch.trim().into(), source: source.into(), ..Default::default() };
    let text = text.trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let items = v.as_array().or(v["companies"].as_array()).or(v["hits"].as_array()).ok_or("The YC data has no array of companies ([…], companies or hits)")?;
        let s = |x: &Value, k: &str| x[k].as_str().unwrap_or("").to_string();
        return Ok(items.iter().map(|x| company(&s(x, "name"), &s(x, "website"), &s(x, "batch"))).filter(|c| !c.company.is_empty()).collect());
    }
    let first = text.lines().find(|l| !l.trim().is_empty()).ok_or("The YC list is empty")?;
    // 区切りが在って URL の無い 1 行目 = 見出し（name と website が要る）。他は見出しの無い貼り付け
    if first.contains(['\t', ',']) && !first.contains("http") {
        let rows = parse_csv(text, if first.contains('\t') { '\t' } else { ',' });
        let at = rows.iter().position(|r| r.iter().any(|c| !c.trim().is_empty())).ok_or("The YC list is empty")?;
        let head = &rows[at];
        let col = |k: &str| head.iter().position(|h| h.trim().eq_ignore_ascii_case(k));
        let (Some(n), Some(w)) = (col("name"), col("website")) else {
            return Err("The YC list has no name and website columns. For companies chosen with yc-open, paste one company per line (for example: Airbnb https://airbnb.com W09)".into())
        };
        let b = col("batch");
        let cell = |r: &Vec<String>, i: Option<usize>| i.and_then(|i| r.get(i)).map(String::as_str).unwrap_or("").to_string();
        return Ok(rows[at + 1..].iter().map(|r| company(&cell(r, Some(n)), &cell(r, Some(w)), &cell(r, b))).filter(|c| !c.company.is_empty()).collect());
    }
    Ok(text.lines().filter_map(yc_line).map(|(n, u, b)| company(&n, &u, &b)).collect())
}
/// 貼り付けの 1 行 → (会社名, site, batch)。最初の http(s) の語が site・「W21」「Summer 2021」が batch・残りが会社名（無ければ YC の会社 page の slug）
fn yc_line(line: &str) -> Option<(String, String, String)> {
    let toks: Vec<&str> = line.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
    let (mut url, mut batch, mut name, mut i) = (String::new(), String::new(), Vec::new(), 0);
    let short_batch = |t: &str| { let b = t.as_bytes(); b.len() == 3 && b"WSFXwsfx".contains(&b[0]) && b[1..].iter().all(u8::is_ascii_digit) };
    while i < toks.len() {
        let t = toks[i];
        let season = ["winter", "spring", "summer", "fall", "autumn"].contains(&t.to_lowercase().as_str());
        if url.is_empty() && (t.starts_with("https://") || t.starts_with("http://")) { url = t.to_string() }
        else if batch.is_empty() && season && toks.get(i + 1).is_some_and(|y| y.len() == 4 && y.bytes().all(|c| c.is_ascii_digit())) { batch = format!("{t} {}", toks[i + 1]); i += 1 }
        else if batch.is_empty() && short_batch(t) { batch = t.to_uppercase() }
        else { name.push(t) }
        i += 1;
    }
    let mut company = name.join(" ");
    if company.is_empty() { company = url.split("ycombinator.com/companies/").nth(1).and_then(|s| s.split(['/', '?', '#']).next()).unwrap_or("").to_string() }
    (!company.is_empty()).then_some((company, url, batch))
}

/// YC の会社一覧を YC_API_BASE（試験は 127.0.0.1 の偽 server）から全頁取る。**bin は --yc-fetch の opt-in の時だけ呼ぶ**（YC の Terms of Use に当たる・owner の判断待ち）。
/// totalPages か空の頁で終わり、max_pages で打ち切る。頁の間は 300ms 空ける。途中で失敗したら取れた分も捨てて Err（半端な表を作らない）
pub fn yc_fetch(base: &str, max_pages: usize, source: &str) -> Result<Vec<Contact>, String> {
    let mut out = Vec::new();
    for page in 1..=max_pages {
        if page > 1 { std::thread::sleep(Duration::from_millis(300)) }
        let v: Value = super::agent().get(&format!("{base}/v0.1/companies?page={page}")).set("Accept", "application/json").call()
            .map_err(|e| e.to_string()).and_then(|r| r.into_json().map_err(|e| e.to_string()))
            .map_err(|e| format!("Can't get page {page} of the YC list ({e}). No table was made"))?;
        let got = from_yc(&v.to_string(), source).map_err(|e| format!("Page {page} of the YC list: {e}"))?;
        if got.is_empty() { break }
        out.extend(got);
        if v["totalPages"].as_u64().is_some_and(|t| page as u64 >= t) { break }
    }
    Ok(out)
}

/// 1 つの表にする: 同じ mail（大文字小文字を見ない）は 1 行に（先の行の空の欄を後の行で埋め、出典を「 + 」で足す）。
/// mail なしは名前・会社・url が全部同じ行だけまとめる。戻り = (表, まとめた行の数)
pub fn merge(lists: Vec<Vec<Contact>>) -> (Vec<Contact>, usize) {
    let (mut out, mut index, mut merged): (Vec<Contact>, std::collections::HashMap<String, usize>, usize) = (Vec::new(), Default::default(), 0);
    for c in lists.into_iter().flatten() {
        let key = if c.mail.is_empty() { format!("\0{}\0{}\0{}", c.name.to_lowercase(), c.company.to_lowercase(), c.url.to_lowercase()) } else { c.mail.to_lowercase() };
        match index.get(&key) {
            Some(&i) => {
                merged += 1;
                let first = &mut out[i];
                for k in COLUMNS.iter().filter(|k| **k != "source") {
                    let v = c.field(k).unwrap_or("").to_string();
                    if let Some(f) = first.field_mut(k).filter(|f| f.is_empty()) { *f = v }
                }
                if !first.source.split(" + ").any(|s| s == c.source) { first.source = format!("{} + {}", first.source, c.source) }
            }
            None => { index.insert(key, out.len()); out.push(c) }
        }
    }
    (out, merged)
}

/// 宛先表の TSV（見出し = COLUMNS・mail が空なら「mail なし」）。cell の tab と改行は空白にする（TSV に入らない）
pub fn to_tsv(rows: &[Contact]) -> String {
    let mut s = COLUMNS.join("\t") + "\n";
    for r in rows {
        let cells: Vec<String> = COLUMNS.iter().map(|k| match (*k, r.field(k).unwrap_or("")) {
            ("mail", "") => NO_MAIL.to_string(),
            (_, v) => v.replace(['\t', '\r', '\n'], " "),
        }).collect();
        s += &(cells.join("\t") + "\n");
    }
    s
}
