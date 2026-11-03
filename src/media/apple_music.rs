//! Music.app を osascript（JXA = `-l JavaScript`）で操る。JXA は `JSON.stringify` で返せるので AppleScript の list を parse しない。
//! 語彙は Music.app の scripting dictionary（`/System/Applications/Music.app/Contents/Resources/com.apple.Music.sdef`・Music 1.4.5）:
//! `library playlist` の `tracks` / `file tracks`、`persistent ID`（"This id does not change over time."）、`location`、`play`、
//! `player state`（stopped / playing / paused / fast forwarding / rewinding）、`player position`、`current track`。
//! 初回の Apple Event で macOS が Automation の許可を聞き、答えが出るまで osascript は返らないので deadline で切る（dialog は押さない）。
//! 曲の音声には触らない: 再生は Music.app に任せ、`location` は download 済みかの判定にだけ使って path は返さない。
//! 調査と実測は開発メモ docs/internal/connectors/apple-music.md。

use super::{first_line, jxa, Osa};
use serde_json::Value;
use std::process::{Command, Stdio};
use std::time::Duration;

/// 許可の確認（dialog）を本人が読んで押すまで osascript は止まり、押されればそのまま続く。押す間に切らないよう長めに
/// （REAPER の insert と同じ 90 秒。曲の多い library の一括取得もこの中）。`--timeout` で変更可
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// library の曲を property ごとに一括で取る（曲ごとに Apple Event を往復しない）。argv[0] = 先頭何曲か（0 = 全部）。
/// download 済み = `file track` で `location` が在る。未 download の Apple Music の曲も `file track` で location が null、
/// iCloud の曲は `shared track`（2026-10-17 実測: 1139 曲 = file track 667（location は全て null）+ shared track 472）。
/// location の一括取得が落ちたら downloaded は null（不明）にして一覧は返す。
const LIBRARY: &str = r#"function run(argv) {
  const lib = Application('Music').libraryPlaylists[0];
  const t = lib.tracks;
  const ids = t.persistentID();
  const n = Number(argv[0]) > 0 ? Math.min(Number(argv[0]), ids.length) : ids.length;
  const cut = a => a.slice(0, n);
  const names = cut(t.name()), artists = cut(t.artist()), albums = cut(t.album()), secs = cut(t.duration());
  let cloud = null;
  try { cloud = cut(t.cloudStatus()); } catch (e) {}
  let local = null;
  try {
    const f = lib.fileTracks, fids = f.persistentID(), locs = f.location();
    local = new Set(fids.filter((_, i) => locs[i] != null));
  } catch (e) {}
  return JSON.stringify({total: ids.length, tracks: cut(ids).map((id, i) => ({
    id, title: names[i], artist: artists[i], album: albums[i], seconds: secs[i],
    downloaded: local ? local.has(id) : null, cloud: cloud ? cloud[i] : null}))});
}"#;

/// argv = [by, value]。by = "id" なら persistent ID で、"title" なら題名の完全一致 → 無ければ部分一致で探し、先頭を play して、
/// 0.25 秒ごとに最大 10 秒、鳴り始めたか（player state と current track の persistent ID）を読み戻す。
/// cloud status（subscription / purchased / matched / uploaded …）と download 済みか（file track の location）も返す（理由の材料）。
const PLAY: &str = r#"function run(argv) {
  const m = Application('Music'), lib = m.libraryPlaylists[0];
  let hits;
  if (argv[0] === 'id') {
    hits = lib.tracks.whose({persistentID: argv[1]})();
  } else {
    hits = lib.tracks.whose({name: argv[1]})();
    if (hits.length === 0) hits = lib.tracks.whose({name: {_contains: argv[1]}})();
  }
  if (hits.length === 0) return JSON.stringify({matches: 0});
  const t = hits[0];
  let cloud = null, downloaded = null;
  try { cloud = t.cloudStatus(); } catch (e) {}
  try { downloaded = t.location() != null; } catch (e) {}
  const chosen = {id: t.persistentID(), title: t.name(), artist: t.artist(), album: t.album(), seconds: t.duration(), downloaded, cloud};
  m.play(t);
  let state = '', now = null, nowTitle = null;
  for (let i = 0; i < 40; i++) {
    state = m.playerState();
    try { now = m.currentTrack.persistentID(); nowTitle = m.currentTrack.name(); } catch (e) { now = null; nowTitle = null; }
    if (state === 'playing' && now === chosen.id) break;
    delay(0.25);
  }
  return JSON.stringify({matches: hits.length, chosen, state, now, now_title: nowTitle});
}"#;

/// Apple Music の契約の状態。Music.app の scripting には無いので、macOS の Apple Music の cache
/// （defaults の com.apple.itunescloud の ICDefaultsCachedSubscriptionStatus。公式の口ではない）のうち、今の account の分だけを読む。
/// account の欄（DSID など）は返さない。形が違えば known: false。
const ACCOUNT: &str = r#"function run() {
  const d = $.NSUserDefaults.alloc.initWithSuiteName('com.apple.itunescloud');
  const c = ObjC.deepUnwrap(d.objectForKey('ICDefaultsCachedSubscriptionStatus'));
  if (!c || typeof c.responses !== 'object') return JSON.stringify({known: false});
  const dsid = String(c.lastKnownActiveDSID || '0');
  const k = dsid === '0' ? null : Object.keys(c.responses).find(k => k.includes('"' + dsid + '"'));
  if (!k) return JSON.stringify({known: true, signed_in: false});
  const music = ((c.responses[k] || {}).status || {}).music || {};
  return JSON.stringify({known: true, signed_in: true, subscription: music.status || null});
}"#;

/// 今の曲と位置。`running()` は Apple Event を送らないので、Music.app が居なければ起こさず許可も聞かない。
const NOW: &str = r#"function run() {
  const m = Application('Music');
  if (!m.running()) return JSON.stringify({running: false});
  const state = m.playerState();
  let track = null, position = null;
  try {
    const c = m.currentTrack;
    track = {id: c.persistentID(), title: c.name(), artist: c.artist(), album: c.album(), seconds: c.duration()};
  } catch (e) {}
  try { position = m.playerPosition(); } catch (e) {}
  return JSON.stringify({running: true, state, position, track});
}"#;

/// 1 曲。曲の file の場所は持たない。
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// Music.app の persistent ID（16 進の文字列。再起動しても変わらない）
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 長さ（秒）。stream など長さの無い物は None
    pub seconds: Option<f64>,
    /// この Mac に file が在るか（download 済み・取り込み済み）。判定できなかった時と、今の曲の読み戻しでは None
    pub downloaded: Option<bool>,
    /// Music.app の cloud status（subscription = Apple Music の曲 / purchased / matched / uploaded / …）。読めなければ None
    pub cloud: Option<String>,
}

/// library の一覧。`tracks` は先頭 limit 曲、`total` は library 全体の曲数。
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub total: usize,
    pub tracks: Vec<Track>,
}

/// 今の再生状態。
#[derive(Debug, Clone, PartialEq)]
pub struct Now {
    /// Music.app が起動しているか（false なら他の欄は空）
    pub running: bool,
    /// player state（"playing" / "paused" / "stopped" / "fast forwarding" / "rewinding"）
    pub state: String,
    /// 今の曲の再生位置（秒）
    pub position: Option<f64>,
    pub track: Option<Track>,
}

/// 再生を頼んで読み戻した値。
#[derive(Debug, Clone, PartialEq)]
pub struct Played {
    /// 鳴らすよう頼んだ曲（一致した曲の先頭）
    pub track: Track,
    /// 一致した曲の数（題名なら完全一致、無ければ部分一致の数）
    pub matches: usize,
    /// 読み戻した player state
    pub state: String,
    /// 読み戻した current track の persistent ID と題名
    pub now_id: Option<String>,
    pub now_title: Option<String>,
}

impl Played {
    /// 頼んだ曲が鳴っている事を読み戻しで確かめたか（CD-51: playing で persistent ID が一致した時だけ）
    pub fn verified(&self) -> bool {
        self.state == "playing" && self.now_id.as_deref() == Some(self.track.id.as_str())
    }
}

/// 再生の結果（DATA_CONTRACTS §2.1 の Result の形のうち、再生が取る物）。
#[derive(Debug, Clone, PartialEq)]
pub enum Playback {
    /// player state が playing で、current track の persistent ID が頼んだ曲と一致した
    Verified(Played),
    /// 頼んだが、頼んだ曲が鳴っているのを確かめられない（別の曲が鳴っている・止まったまま）。理由と次の一手
    Unverified { played: Played, reason: String },
    /// Music.app が答えなかった（deadline・Apple Event の timeout）。鳴り始めたかどうか分からない（自動で再送しない）
    EffectUnknown { reason: String },
}

/// Apple Music の契約の状態（macOS の cache から。公式の口ではない）。
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub signed_in: bool,
    /// 契約が有効か（cache の music.status が Enabled / Disabled）。分からなければ None
    pub subscription: Option<bool>,
}

/// Music.app が起動中か。process 名 "Music" の完全一致（Apple Event を送らないので許可も聞かない）。
pub fn is_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "Music"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// library の先頭 limit 曲（0 = 全部）。Music.app が居なければ起動する。
pub fn library(limit: usize, timeout: Duration) -> Result<Library, String> {
    parse_library(&run(LIBRARY, &[&limit.to_string()], timeout)?)
}

/// 題名の曲を Music.app で再生し、鳴り始めたかを読み戻す。題名は script に埋め込まず argv で渡す。
/// Err は送る前に止まった時・見つからない時・許可が無い時（再生は起きていない）。
pub fn play(title: &str, timeout: Duration) -> Result<Playback, String> {
    if title.trim().is_empty() {
        return Err("The title is empty".to_string());
    }
    play_by("title", title, timeout)?.ok_or_else(|| {
        format!("“{title}” wasn't found in the Music library (check the title with music-library)")
    })
}

/// persistent ID（music-library の id。16 桁の 16 進）の曲を再生し、鳴り始めたかを読み戻す。
pub fn play_id(id: &str, timeout: Duration) -> Result<Playback, String> {
    if !valid_persistent_id(id) {
        return Err(format!("Not a persistent ID: {} (give the 16 hex digits from the id column of music-library)", id.chars().take(40).collect::<String>()));
    }
    play_by("id", id, timeout)?.ok_or_else(|| format!("The song with ID {id} wasn't found in the Music library (check the ID with music-library)"))
}

fn play_by(by: &str, value: &str, timeout: Duration) -> Result<Option<Playback>, String> {
    match jxa(PLAY, &[by, value], timeout) {
        Ok(out) => Ok(parse_play(&out)?.map(|p| judge(p, timeout))),
        Err(Osa::Deadline) => Ok(Some(Playback::EffectUnknown { reason: unknown(&format!("No answer within {} s", timeout.as_secs())) })),
        Err(Osa::Failed(e)) if e.contains("(-1712)") => Ok(Some(Playback::EffectUnknown { reason: unknown("Apple event timeout (-1712)") })),
        Err(Osa::Failed(e)) => Err(osascript_error(&e)),
    }
}

fn unknown(what: &str) -> String {
    format!(
        "Music: {what}. Can't tell whether playback started (it won't be sent again on its own). If Music has a window left open (open file, save, confirm and so on), close it, check the current song with music-now, then run it again"
    )
}

/// 読み戻しから Verified / Unverified を決める。鳴っていなければ契約の状態（cache）を添える（理由を推測で足さない）。
fn judge(p: Played, timeout: Duration) -> Playback {
    if p.verified() {
        return Playback::Verified(p);
    }
    let reason = match (&p.now_id, p.state.as_str()) {
        (Some(now), "playing") if now != &p.track.id => format!(
            "A different song is playing (“{}” · ID {now}); it isn't the song you asked for (ID {}). {}",
            p.now_title.as_deref().unwrap_or(""),
            p.track.id,
            if p.matches > 1 {
                format!("There are {} songs with the same title. Give the ID from music-library with music-play --id", p.matches)
            } else {
                "Give the ID from music-library with music-play --id".to_string()
            }
        ),
        _ => {
            let acct = match account(timeout.min(Duration::from_secs(20))) {
                Ok(Some(a)) => format!(" · {}", account_text(&a)),
                _ => String::new(),
            };
            format!(
                "Music didn't start playing after 10 s (state: {} · this song: {}{acct}). Check the message on Music's screen",
                state_text(&p.state),
                track_text(&p.track)
            )
        }
    };
    Playback::Unverified { played: p, reason }
}

/// player state の日本語
pub fn state_text(s: &str) -> &str {
    match s {
        "playing" => "Playing",
        "paused" => "Paused",
        "stopped" => "Stopped",
        "fast forwarding" => "Fast-forwarding",
        "rewinding" => "Rewinding",
        other => other,
    }
}

/// 「Apple Music の曲・この Mac に未 download」のような曲の素性
pub fn track_text(t: &Track) -> String {
    let cloud = match t.cloud.as_deref() {
        Some("subscription") => "A song from Apple Music".to_string(),
        Some("purchased") => "A purchased song".to_string(),
        Some("matched") | Some("uploaded") => "A song from the iCloud Music Library".to_string(),
        Some(other) => format!("cloud status {other}"),
        None => "Cloud status unknown".to_string(),
    };
    let dl = match t.downloaded {
        Some(true) => "On this Mac",
        Some(false) => "Not downloaded to this Mac",
        None => "Download status unknown",
    };
    format!("{cloud} · {dl}")
}

/// 「Apple Music: sign in 済み・契約は有効（macOS の cache から）」
pub fn account_text(a: &Account) -> String {
    let sub = match (a.signed_in, a.subscription) {
        (false, _) => "Not signed in",
        (true, Some(true)) => "Signed in · subscription active",
        (true, Some(false)) => "Signed in · subscription inactive",
        (true, None) => "Signed in · subscription status unknown",
    };
    format!("Apple Music: {sub} (from the macOS cache)")
}

/// 今の曲・位置・状態。Music.app が居なければ起こさない。
pub fn now(timeout: Duration) -> Result<Now, String> {
    parse_now(&run(NOW, &[], timeout)?)
}

/// Music.app が操作に答えるか（今の状態を 1 回読む。居なければ Apple Event を送らない）。答えなければ人の言葉の理由。
/// 窓が在っても、開いたままの panel（ファイルを開く など）の間は Apple Event に答えず、既定の 2 分で -1712 になる（2026-10-17 実測）。
pub fn responding(timeout: Duration) -> Result<Now, String> {
    let out = jxa(NOW, &[], timeout).map_err(|e| match e {
        Osa::Deadline => format!(
            "No answer within {} s. Check whether Music has a window left open (open file, save, confirm and so on) or a permission prompt showing (on a busy Mac, Music can take over 10 minutes to answer after it starts)",
            timeout.as_secs()
        ),
        Osa::Failed(e) => osascript_error(&e),
    })?;
    parse_now(&out)
}

/// Apple Music の契約の状態（cache が無い・形が違えば None）。Music.app に Apple Event は送らない。
pub fn account(timeout: Duration) -> Result<Option<Account>, String> {
    parse_account(&jxa(ACCOUNT, &[], timeout).map_err(|e| match e {
        Osa::Deadline => format!("Couldn't read the Apple Music state within {} s", timeout.as_secs()),
        Osa::Failed(e) => format!("Can't read the Apple Music state: {}", first_line(&e)),
    })?)
}

/// Music.app の persistent ID（16 桁の 16 進）か
pub fn valid_persistent_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// jxa の失敗を Music.app 向けの文に（読むだけの操作用。deadline は許可の確認待ちを先に疑う）。
fn run(script: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    jxa(script, args, timeout).map_err(|e| match e {
        Osa::Deadline => format!(
            "Music didn't respond within {} s. If a prompt asking permission to control Music is on screen, press OK and run it again (for a large library, raise the seconds with --timeout)",
            timeout.as_secs()
        ),
        Osa::Failed(e) => osascript_error(&e),
    })
}

/// osascript の失敗（stderr）を次の一手つきの文に。番号は AppleScript の error（`execution error: … (-1743)` の形）:
/// -1743 errAEEventNotPermitted（Automation で拒否）/ -1744 errAEEventWouldRequireUserConsent / -1712 timeout / -600 app が居ない。
pub fn osascript_error(stderr: &str) -> String {
    let has = |code: &str| stderr.contains(&format!("({code})"));
    if has("-1743") || has("-1744") {
        "Controlling Music isn't allowed. In System Settings › Privacy & Security › Automation, turn on “Music” for this terminal (or Shibubu), then run it again".to_string()
    } else if has("-1712") {
        "Music didn't respond (Apple event timeout). If Music has a window left open (open file, save, confirm and so on), close it and run it again (on a busy Mac, Music can take over 10 minutes to answer after it starts)".to_string()
    } else if has("-600") {
        "Music quit partway. Run it again".to_string()
    } else {
        format!("Controlling Music failed: {}", first_line(stderr))
    }
}

fn json(stdout: &str) -> Result<Value, String> {
    serde_json::from_str(stdout.trim()).map_err(|_| format!("Can't read Music's answer: {}", first_line(stdout)))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn track(v: &Value) -> Option<Track> {
    Some(Track {
        id: v["id"].as_str()?.to_string(),
        title: text(&v["title"]),
        artist: text(&v["artist"]),
        album: text(&v["album"]),
        seconds: v["seconds"].as_f64(),
        downloaded: v["downloaded"].as_bool(),
        cloud: v["cloud"].as_str().map(str::to_string),
    })
}

/// LIBRARY の stdout（`{"total":N,"tracks":[…]}`）を読む。
pub fn parse_library(stdout: &str) -> Result<Library, String> {
    let v = json(stdout)?;
    let tracks = v["tracks"].as_array().ok_or_else(|| format!("Music's answer has no tracks: {}", first_line(stdout)))?;
    let tracks: Vec<Track> = tracks.iter().filter_map(track).collect();
    Ok(Library { total: v["total"].as_u64().map(|n| n as usize).unwrap_or(tracks.len()), tracks })
}

/// PLAY の stdout を読む。一致する曲が無ければ None。
pub fn parse_play(stdout: &str) -> Result<Option<Played>, String> {
    let v = json(stdout)?;
    let bad = || format!("Can't read Music's answer: {}", first_line(stdout));
    let matches = v["matches"].as_u64().ok_or_else(bad)? as usize;
    if matches == 0 {
        return Ok(None);
    }
    Ok(Some(Played {
        track: track(&v["chosen"]).ok_or_else(bad)?,
        matches,
        state: text(&v["state"]),
        now_id: v["now"].as_str().map(str::to_string),
        now_title: v["now_title"].as_str().map(str::to_string),
    }))
}

/// NOW の stdout を読む。
pub fn parse_now(stdout: &str) -> Result<Now, String> {
    let v = json(stdout)?;
    let running = v["running"].as_bool().ok_or_else(|| format!("Can't read Music's answer: {}", first_line(stdout)))?;
    Ok(Now { running, state: text(&v["state"]), position: v["position"].as_f64(), track: track(&v["track"]) })
}

/// ACCOUNT の stdout を読む。cache が無い・形が違えば None。
pub fn parse_account(stdout: &str) -> Result<Option<Account>, String> {
    let v = json(stdout)?;
    if v["known"].as_bool() != Some(true) {
        return Ok(None);
    }
    let subscription = match v["subscription"].as_str() {
        Some("Enabled") => Some(true),
        Some("Disabled") => Some(false),
        _ => None,
    };
    Ok(Some(Account { signed_in: v["signed_in"].as_bool().unwrap_or(false), subscription }))
}
