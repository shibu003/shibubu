//! YouTube Data API v3 の search.list で動画を探し、選んだ 1 本を本人の browser に handoff（`open`）する。
//! Data API に再生の method は無く、player は Kangae に埋め込まない（DESIGN §4.4 の handoff。音声だけの再生・download は
//! YouTube API Services Developer Policies III.I.7 / III.E.1 で禁止なので作らない）。
//! 出典: https://developers.google.com/youtube/v3/docs/search/list（"Quota impact: 100 calls per day"）、
//! https://docs.cloud.google.com/docs/authentication/api-keys-use（"use the x-goog-api-key HTTP header"。query の key は URL に残るので使わない）、
//! error の reason は https://developers.google.com/youtube/v3/docs/errors と google/api/error_reason.proto（ErrorInfo）。
//! 調査と実測は開発メモ docs/internal/connectors/youtube.md。

use super::{first_line, jxa, Osa};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const API_BASE: &str = "https://www.googleapis.com/youtube/v3";
/// 1 回の検索で出す件数（search.list の maxResults は 0〜50）
pub const MAX_RESULTS: usize = 10;
/// project ごとの search.list の既定の上限（回 / 日・太平洋時間の 0 時に戻る）。
/// 出典: https://developers.google.com/youtube/v3/determine_quota_cost（2026-10-17 取得: "a default quota allocation of 100 search.list calls"・
/// "Daily quotas reset at midnight Pacific Time"）。API は残りを返さない（Cloud Console の Quotas だけ）ので、Kangae は自分の回数を数える
pub const DAILY_SEARCHES: u32 = 100;

/// key が無い時の文。手順は Google Cloud Console の画面の名前で書く（初めて使う人が key を作れるように）
pub const NO_KEY: &str = "The YouTube API key isn't set. In the Google Cloud Console, pick a project, turn on YouTube Data API v3 under “APIs & Services › Library”, then put the key made under “Credentials › Create credentials › API key” in the environment variable KANGAE_YOUTUBE_API_KEY (limit the key's “API restrictions” to YouTube Data API v3 only)";

/// 検索結果の 1 件。
#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    /// videoId（11 文字）
    pub id: String,
    pub title: String,
    pub channel: String,
}

/// 試験用の口: KANGAE_YOUTUBE_API_BASE は http://127.0.0.1 で始まる時だけ採用（src/github.rs の KANGAE_GITHUB_API_BASE と同じ型）
fn api_base() -> String {
    match std::env::var("KANGAE_YOUTUBE_API_BASE") {
        Ok(b) if b.starts_with("http://127.0.0.1") => b.trim_end_matches('/').to_string(),
        _ => API_BASE.to_string(),
    }
}

/// env `KANGAE_YOUTUBE_API_KEY`（空なら無い扱い）。
pub fn key_from_env() -> Option<String> {
    std::env::var("KANGAE_YOUTUBE_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// search.list（type=video）の上位 MAX_RESULTS 件。1 回で key の検索枠（1 日 100 回）を 1 回使う。
pub fn search(q: &str, key: &str) -> Result<Vec<Video>, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("The search words are empty".to_string());
    }
    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .redirects(0)
        .build()
        .get(&format!("{}/search", api_base()))
        .query("part", "snippet")
        .query("type", "video")
        .query("maxResults", &MAX_RESULTS.to_string())
        .query("q", q)
        .set("x-goog-api-key", key)
        .set("User-Agent", "Kangae-Rust-MVP/0.4")
        .call();
    if matches!(&resp, Ok(_) | Err(ureq::Error::Status(..))) {
        count_search(); // 応答が来た呼び出しは枠を使う（失敗した呼び出しも数えられる）
    }
    match resp {
        Ok(r) => {
            let v: Value = serde_json::from_str(&read_body(r)?).map_err(|_| {
                "YouTube's answer was in an unexpected form (a proxy or a temporary fault, perhaps). Wait a moment and try again".to_string()
            })?;
            parse_search(&v)
        }
        Err(ureq::Error::Status(code, r)) => Err(error_message(code, &read_body(r).unwrap_or_default(), q)),
        Err(ureq::Error::Transport(t)) => Err(format!(
            "Can't connect to YouTube ({}). Check your network connection and try again",
            t.kind()
        )),
    }
}

fn read_body(r: ureq::Response) -> Result<String, String> {
    let mut b = Vec::new();
    r.into_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut b)
        .map_err(|e| format!("Couldn't read YouTube's answer ({e}). Try again"))?;
    if b.len() > 1024 * 1024 {
        return Err("YouTube's answer is too large".to_string());
    }
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// search.list の応答（200）から動画を取り出す。videoId が無い・形の違う物は捨てる。題名は HTML の実体参照を戻す。
pub fn parse_search(v: &Value) -> Result<Vec<Video>, String> {
    let items = v["items"].as_array().ok_or("YouTube's answer has no items")?;
    Ok(items
        .iter()
        .filter_map(|it| {
            let id = it["id"]["videoId"].as_str().filter(|id| valid_video_id(id))?;
            Some(Video {
                id: id.to_string(),
                title: unescape(it["snippet"]["title"].as_str().unwrap_or("")),
                channel: unescape(it["snippet"]["channelTitle"].as_str().unwrap_or("")),
            })
        })
        .take(MAX_RESULTS)
        .collect())
}

/// 失敗（4xx / 5xx）の body を次の一手つきの文に。reason は旧形式の `error.errors[].reason` と
/// ErrorInfo の `error.details[].reason` の両方を見る。q は上限切れの時に browser で検索する URL を作るのに使う。
pub fn error_message(code: u16, body: &str, q: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = &v["error"];
    let reasons: Vec<&str> = ["errors", "details"]
        .iter()
        .flat_map(|k| e[*k].as_array().into_iter().flatten())
        .filter_map(|x| x["reason"].as_str())
        .collect();
    let has = |r: &str| reasons.contains(&r);
    if has("quotaExceeded") {
        format!(
            "Today's YouTube search limit was reached (by default 100 a day, reset at midnight Pacific time). Try again later, or search YouTube in your browser: https://www.youtube.com/results?search_query={}",
            encode(q)
        )
    } else if code == 429 || has("rateLimitExceeded") || has("userRateLimitExceeded") || has("tooManyRequests") {
        format!("Too many YouTube searches were sent in a short time (HTTP {code}). Wait about a minute and try again")
    } else if has("API_KEY_INVALID") || has("keyInvalid") {
        "The YouTube API key isn't valid (KANGAE_YOUTUBE_API_KEY). Check the key under “Credentials” in the Google Cloud Console".to_string()
    } else if has("SERVICE_DISABLED") || has("accessNotConfigured") {
        "YouTube Data API v3 isn't turned on in this key's project. Turn it on under “APIs & Services › Library” in the Google Cloud Console".to_string()
    } else if has("ipRefererBlocked") || reasons.iter().any(|r| r.starts_with("API_KEY_") && r.ends_with("_BLOCKED")) {
        "This key's restrictions don't allow YouTube Data API v3. In the Google Cloud Console, under “Credentials”, add YouTube Data API v3 to the key's “API restrictions” and remove the “Application restrictions”".to_string()
    } else {
        let msg = plain(e["message"].as_str().unwrap_or(""));
        let msg = if msg.trim().is_empty() { String::new() } else { format!(": {}", msg.trim()) };
        match code {
            500..=599 => format!("YouTube is having trouble (HTTP {code}{msg}). Wait a moment and try again"),
            401 | 403 => format!("YouTube refused the search (HTTP {code}{msg})"),
            _ => format!("YouTube gave an unexpected answer (HTTP {code}{msg})"),
        }
    }
}

/// YouTube の videoId（A-Z a-z 0-9 _ - の 11 文字）か。`open` に file path や別の scheme を渡さないための門。
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// watch page の URL。videoId でなければ Err。
pub fn watch_url(id: &str) -> Result<String, String> {
    if valid_video_id(id) {
        Ok(format!("https://www.youtube.com/watch?v={id}"))
    } else {
        let shown: String = id.chars().take(60).collect();
        Err(format!("Not a YouTube video ID: {shown} (give the 11-character ID in the yt-search result)"))
    }
}

/// 本人の既定の browser（https を開く app）。
#[derive(Debug, Clone, PartialEq)]
pub struct Browser {
    pub bundle_id: String,
    pub name: String,
    /// 起動中なら pid
    pub pid: Option<u32>,
}

/// 既定の browser を NSWorkspace（URLForApplicationToOpenURL）で引き、runningApplications で起動中かを見る。
const BROWSER: &str = r#"ObjC.import('AppKit');
function run() {
  const ws = $.NSWorkspace.sharedWorkspace;
  const u = ws.URLForApplicationToOpenURL($.NSURL.URLWithString('https://www.youtube.com/'));
  if (!u || u.isNil()) return JSON.stringify({bundle_id: null});
  const b = $.NSBundle.bundleWithURL(u);
  const id = ObjC.unwrap(b.bundleIdentifier);
  const name = ObjC.unwrap(b.objectForInfoDictionaryKey('CFBundleName')) || ObjC.unwrap(u.lastPathComponent).replace(/\.app$/, '');
  const apps = ws.runningApplications;
  let pid = null;
  for (let i = 0; i < apps.count; i++) {
    const a = apps.objectAtIndex(i);
    if (ObjC.unwrap(a.bundleIdentifier) === id) { pid = a.processIdentifier; break; }
  }
  return JSON.stringify({bundle_id: id, name, pid});
}"#;

/// 既定の browser（無ければ Ok(None)）。
pub fn default_browser(timeout: Duration) -> Result<Option<Browser>, String> {
    parse_browser(&jxa(BROWSER, &[], timeout).map_err(|e| match e {
        Osa::Deadline => format!("Couldn't check the default browser within {} s", timeout.as_secs()),
        Osa::Failed(e) => format!("Can't check the default browser: {}", first_line(&e)),
    })?)
}

/// BROWSER の stdout を読む。
pub fn parse_browser(stdout: &str) -> Result<Option<Browser>, String> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| format!("Can't read the default browser's answer: {}", first_line(stdout)))?;
    Ok(v["bundle_id"].as_str().filter(|s| !s.is_empty()).map(|id| Browser {
        bundle_id: id.to_string(),
        name: v["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(id).to_string(),
        pid: v["pid"].as_u64().map(|p| p as u32),
    }))
}

/// `open` の実行 file。`KANGAE_OPEN_BIN` で差し替え可（test は偽物を置いて本物の browser を開かない）。
fn open_bin() -> PathBuf {
    std::env::var_os("KANGAE_OPEN_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/bin/open"))
}

/// `open` の終わりを待つ長さ。混んだ機械（load 900）では `open` 自体が 15 分以上返らなかった（2026-10-17 実測）
pub const OPEN_WAIT: Duration = Duration::from_secs(15);

/// watch page を browser で開く（handoff）。bundle があれば `open -b <bundle> <url>`（-a / -n / -g を付けない =
/// 新しい instance を起こさず、既に開いている browser の前面へ。起動していなければ LaunchServices がその browser を起動する）。
/// 見えない所で鳴らす background player は Developer Policies III.I.9 で禁止なので前に出す。
/// 戻り値は (開いた URL, open が OPEN_WAIT 以内に終わったか)。終わらなければ open は止めずに返す（止めると依頼が消えうる。窓で確かめる）。
pub fn open(id: &str, bundle: Option<&str>) -> Result<(String, bool), String> {
    let url = watch_url(id)?;
    let mut cmd = Command::new(open_bin());
    if let Some(b) = bundle {
        cmd.args(["-b", b]);
    }
    let mut child = cmd
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null()) // 待ち切らずに返すので、呼び手の pipe を open に握らせない
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Can't open the browser ({e}). Paste this URL into your browser: {url}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok((url, true)),
            Ok(Some(st)) => {
                return Err(format!(
                    "Couldn't open it in the browser (open ended with exit code {}). Paste this URL into your browser: {url}",
                    st.code().unwrap_or(-1)
                ))
            }
            Ok(None) if started.elapsed() > OPEN_WAIT => return Ok((url, false)),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("Can't wait for open ({e}). Paste this URL into your browser: {url}")),
        }
    }
}

/// 窓の 1 件（bridge の window_list〈audio::surface::WindowReader〉から、要る欄だけ）。
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    pub id: i64,
    pub title: String,
    /// is_on_screen かつ今の Space
    pub here: bool,
}

/// open の前後の窓から、開いた watch page の窓を探す: 題名に YouTube を含み前に無かった（id, 題名）の窓 → 無ければ
/// open の後に新しく出来た窓（混んだ機械では読み込みが終わるまで題名が「Untitled」のまま。2026-10-17 実測）。
/// それぞれ今の画面の物を先に。無ければ None。
pub fn opened_window<'a>(before: &[Win], after: &'a [Win]) -> Option<&'a Win> {
    let titled = |w: &&Win| w.title.contains("YouTube") && !before.iter().any(|b| b.id == w.id && b.title == w.title);
    let new = |w: &&Win| !before.iter().any(|b| b.id == w.id);
    let pick = |f: &dyn Fn(&&Win) -> bool| after.iter().filter(|w| f(w)).find(|w| w.here).or_else(|| after.iter().find(|w| f(w)));
    pick(&titled).or_else(|| pick(&new))
}

/// 検索の回数を数える file（app の store と同じ dir の `youtube-searches.txt`）。
/// 置き場の dir は `surface::data_dir_from` と同じ順と既定（`SHIBUBU_DATA_DIR` → `KANGAE_DATA_DIR` → `~/Library/Application Support/Shibubu`・FX1）。
/// ponytail: media は self-contained（tests/media.rs と kangae-media が単独で `#[path]` で取り込む・surface を持たない）ので順をここに写す。
/// media を surface と同じ crate の根で取り込む時に `data_dir()` へ替える。
fn count_file() -> PathBuf {
    let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    set("SHIBUBU_DATA_DIR")
        .or_else(|| set("KANGAE_DATA_DIR"))
        .or_else(|| set("HOME").map(|h| h.join("Library/Application Support/Shibubu")))
        .unwrap_or_else(|| PathBuf::from(".shibubu-data"))
        .join("youtube-searches.txt")
}

/// 太平洋時間の今日（search.list の枠が戻る区切り）。`date` が読めなければ None
fn pt_today() -> Option<String> {
    let o = Command::new("/bin/date").env("TZ", "America/Los_Angeles").arg("+%F").output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (s.len() == 10).then_some(s)
}

/// count file の中身（「YYYY-MM-DD N」）のうち、today の回数。日が違う・読めなければ 0
pub fn parse_count(text: &str, today: &str) -> u32 {
    let mut f = text.split_whitespace();
    match (f.next(), f.next().and_then(|n| n.parse().ok())) {
        (Some(day), Some(n)) if day == today => n,
        _ => 0,
    }
}

/// この Kangae から今日（太平洋時間）search.list を呼んだ回数。同じ key を他で使った分は入らない
pub fn searches_today() -> u32 {
    let Some(today) = pt_today() else { return 0 };
    std::fs::read_to_string(count_file()).map(|t| parse_count(&t, &today)).unwrap_or(0)
}

// ponytail: 同時に 2 つの検索が走ると 1 回分を数え落とす（read → write）。目安の数なので lock は付けない
fn count_search() {
    let Some(today) = pt_today() else { return };
    let f = count_file();
    let n = std::fs::read_to_string(&f).map(|t| parse_count(&t, &today)).unwrap_or(0) + 1;
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&f, format!("{today} {n}\n"));
}

/// 「今日（太平洋時間）この Kangae から N 回目・既定の上限 100 回」の 1 行
pub fn quota_text(n: u32) -> String {
    format!(
        "Search quota: search number {n} from Shibubu today (Pacific time) · the default limit is {DAILY_SEARCHES} a day (searches made with the same key elsewhere aren't counted; the exact remainder is under “Quotas” in the Google Cloud Console)"
    )
}

/// search.list の題名は HTML の実体参照のまま返る（`&#39;` `&amp;` など）ので文字に戻す。知らない参照はそのまま残す。
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest.find(';').filter(|&e| e <= 10).and_then(|e| entity(&rest[1..e]).map(|c| (c, e))) {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Google の error message は HTML の link を含む事がある（`<a href=…>quota</a>`）ので tag を落とす。
fn plain(s: &str) -> String {
    let mut tag = false;
    s.chars()
        .filter(|&c| {
            if c == '<' {
                tag = true;
            }
            let keep = !tag;
            if c == '>' {
                tag = false;
            }
            keep
        })
        .collect()
}

/// URL の query 用の percent-encoding（RFC 3986 の unreserved 以外を %XX に）。
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
