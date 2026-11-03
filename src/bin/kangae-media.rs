//! Apple Music（Music.app）と YouTube を繋ぐ CLI（docs/ACCEPTANCE.md test3 の土台）。
//!   kangae-media music-library --limit 20   library の曲（題名 / artist / album / 長さ / download 済みか）
//!   kangae-media music-play "題名"          Music.app で再生して、鳴り始めたかを読み戻す（--id <persistent ID> でも）
//!   kangae-media music-now                  今の曲と位置（Music.app を起こさない）
//!   kangae-media yt-search "検索語"         YouTube Data API v3 の search.list で上位 10 件
//!   kangae-media yt-open <videoId>          本人の既定の browser の前面で watch page を開き、窓を読み戻す（handoff）
//! src/main.rs とは独立に compile する（media mod と、窓の読みに audio/surface.rs を #[path] で取り込む）。分岐は docs/diagrams.md §12・§18。
//! 窓の読み（yt-open の前面の窓・address bar）は macOS bridge だけ（無ければ「未確認（理由）」）。読むだけ。

#[allow(dead_code)]
#[path = "../media/mod.rs"]
mod media;
#[allow(dead_code)]
#[path = "../audio/surface.rs"]
mod surface;

use media::apple_music::{self, Playback, Track};
use media::youtube::{self, Win};
use std::time::{Duration, Instant};
use surface::{WindowReader, WindowsRead};

const USAGE: &str = "使い方:
  kangae-media music-library [--limit N]  Music.app の library の曲（題名 / artist / album / 長さ / download 済みか / 種類 / id）を先頭 N 曲（既定 50・0 で全部）
  kangae-media music-play \"題名\"          その曲を Music.app で再生し、鳴り始めたかを読み戻す（完全一致 → 無ければ部分一致）
  kangae-media music-play --id <id>       music-library の id（persistent ID）の曲を再生する（同じ題名の曲が複数ある時）
  kangae-media music-now                  今の曲・位置・再生状態（Music.app が居なければ起こさない）
  kangae-media yt-search \"検索語\"         YouTube Data API v3 の search.list で上位 10 件（videoId / 題名 / channel）
  kangae-media yt-open <videoId>          既定の browser の前面で YouTube の watch page を開く（handoff。Kangae には埋め込まない）
共通: --timeout S（osascript を待つ秒数・既定 90）
環境変数: KANGAE_YOUTUBE_API_KEY（YouTube Data API v3 の key）  SHIBUBU_DATA_DIR / KANGAE_DATA_DIR（検索の回数を数える file の dir・既定 ~/Library/Application Support/Shibubu）
          KANGAE_OSASCRIPT_BIN / KANGAE_OPEN_BIN（osascript / open の差し替え・test 用）
          KANGAE_SURFACE_BRIDGE（窓の確認の bridge・空なら使わない）";

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} に値が要ります"))
}

fn run(args: Vec<String>) -> i32 {
    let mut it = args.into_iter();
    let cmd = it.next().unwrap_or_default();
    let mut limit = 50usize;
    let mut timeout = apple_music::DEFAULT_TIMEOUT;
    let mut rest: Vec<String> = Vec::new();
    let mut id: Option<String> = None;
    while let Some(a) = it.next() {
        let r: Result<(), String> = match a.as_str() {
            "--limit" => need(&mut it, &a)
                .and_then(|v| v.parse::<usize>().map_err(|_| format!("--limit は 0 以上の整数で: {v}")))
                .map(|v| limit = v),
            "--timeout" => need(&mut it, &a)
                .and_then(|v| v.parse::<u64>().map_err(|_| format!("--timeout は秒の整数で: {v}")))
                .map(|v| timeout = Duration::from_secs(v)),
            "--id" => need(&mut it, &a).map(|v| id = Some(v)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            _ => {
                rest.push(a);
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("{e}\n{USAGE}");
            return 2;
        }
    }
    // 題名と検索語は quote し忘れても 1 つに繋ぐ
    let arg = rest.join(" ");
    let out = match (cmd.as_str(), rest.len()) {
        ("music-library", 0) => music_library(limit, timeout),
        ("music-play", 0) if id.is_some() => music_play(None, id.as_deref(), timeout),
        ("music-play", n) if n > 0 && id.is_none() => music_play(Some(&arg), None, timeout),
        ("music-now", 0) => music_now(timeout),
        ("yt-search", n) if n > 0 => yt_search(&arg),
        ("yt-open", 1) => yt_open(&arg, timeout),
        ("-h" | "--help", _) => {
            println!("{USAGE}");
            return 0;
        }
        ("", _) => {
            eprintln!("sub command が要ります\n{USAGE}");
            return 2;
        }
        _ => {
            eprintln!("引数が合いません: {cmd} {arg}\n{USAGE}");
            return 2;
        }
    };
    match out {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn music_library(limit: usize, timeout: Duration) -> Result<String, String> {
    if !apple_music::is_running() {
        eprintln!("Music.app を開きます（library を読むのに Music.app が要ります。初回は macOS が操作の許可を聞きます）");
    }
    let lib = apple_music::library(limit, timeout)?;
    if lib.tracks.is_empty() {
        return Ok("Music.app の library に曲がありません（Apple Music に sign in して曲を library に追加するか、曲の file を Music.app に読み込むと、ここに出ます）".to_string());
    }
    let head = if lib.tracks.len() < lib.total { format!("のうち先頭 {} 曲", lib.tracks.len()) } else { String::new() };
    let mut s = format!("Music.app の library: {} 曲{head}（download 済み = この Mac に file が在る）", lib.total);
    for (i, t) in lib.tracks.iter().enumerate() {
        let dl = match t.downloaded {
            Some(true) => "download 済み",
            Some(false) => "未 download",
            None => "download 不明",
        };
        let kind = match t.cloud.as_deref() {
            Some("subscription") => "  Apple Music の曲",
            Some("purchased") => "  購入した曲",
            Some("matched") | Some("uploaded") => "  iCloud の曲",
            _ => "",
        };
        s += &format!("\n{:>3}. {}  {}  {dl}{kind}  id {}", i + 1, names(t), length(t.seconds), t.id);
    }
    Ok(s)
}

fn music_play(title: Option<&str>, id: Option<&str>, timeout: Duration) -> Result<String, String> {
    if !apple_music::is_running() {
        eprintln!("Music.app を開きます（再生は Music.app が行います。初回は macOS が操作の許可を聞きます）");
    }
    let r = match (title, id) {
        (_, Some(id)) => apple_music::play_id(id, timeout)?,
        (Some(t), None) => apple_music::play(t, timeout)?,
        (None, None) => return Err("題名か --id が要ります".to_string()),
    };
    match r {
        Playback::Verified(p) => {
            let others = match (p.matches > 1, title) {
                (true, Some(t)) => format!("\n「{t}」に当たる曲が他に {} 曲あります（1 曲を選ぶには music-library の id を music-play --id で）", p.matches - 1),
                _ => String::new(),
            };
            Ok(format!(
                "再生中: {}（Music.app。player state と persistent ID {} の一致を読み戻して確認・{}）{others}",
                names(&p.track),
                p.track.id,
                apple_music::track_text(&p.track)
            ))
        }
        Playback::Unverified { played, reason } => Err(format!("Music.app に「{}」の再生を頼みましたが、確かめられませんでした: {reason}", names(&played.track))),
        Playback::EffectUnknown { reason } => Err(reason),
    }
}

fn music_now(timeout: Duration) -> Result<String, String> {
    let n = apple_music::now(timeout)?;
    if !n.running {
        return Ok("Music.app は起動していません（再生していません）".to_string());
    }
    Ok(match &n.track {
        Some(t) => format!("{}: {}  {} / {}", apple_music::state_text(&n.state), names(t), length(n.position), length(t.seconds)),
        None => format!("{}（今の曲はありません）", apple_music::state_text(&n.state)),
    })
}

fn yt_search(q: &str) -> Result<String, String> {
    let key = youtube::key_from_env().ok_or_else(|| youtube::NO_KEY.to_string())?;
    let videos = youtube::search(q, &key)?;
    let quota = youtube::quota_text(youtube::searches_today());
    if videos.is_empty() {
        return Ok(format!("YouTube の検索「{q}」: 結果はありません\n{quota}"));
    }
    let mut s = format!("YouTube の検索「{q}」: {} 件（YouTube Data API v3 の search.list）", videos.len());
    for (i, v) in videos.iter().enumerate() {
        s += &format!("\n{:>3}. {}  {} — {}", i + 1, v.id, v.title, v.channel);
    }
    s += &format!("\n{quota}\n開く: kangae-media yt-open <videoId>（本人の browser の YouTube で再生。Kangae には埋め込みません）");
    Ok(s)
}

fn to_wins(ws: &[surface::Window]) -> Vec<Win> {
    ws.iter().map(|w| Win { id: w.id, title: w.title.clone(), here: w.here() }).collect()
}

/// 既定の browser の前面で開き、最大 20 秒、開いた窓を読み戻す（Space は切り替えない）。前面の根拠は、前に無かった YouTube の題名の窓が
/// 今の画面に在る事か、今の画面の一番手前の窓の address bar に watch?v=<id> が在る事。窓は `WindowReader`（bridge が先）で読む。
fn yt_open(id: &str, timeout: Duration) -> Result<String, String> {
    youtube::watch_url(id)?;
    let browser = youtube::default_browser(timeout)?;
    let name = browser.as_ref().map(|b| b.name.clone()).unwrap_or_else(|| "既定の browser".to_string());
    let mut reader = WindowReader::new();
    let before = match browser.as_ref().and_then(|b| b.pid).map(|p| reader.windows(p)) {
        Some(WindowsRead::Read { list, .. }) => to_wins(&list),
        _ => Vec::new(),
    };
    let (url, finished) = youtube::open(id, browser.as_ref().map(|b| b.bundle_id.as_str()))?;
    let launched = if browser.as_ref().is_some_and(|b| b.pid.is_none()) { format!("（{name} は起動していなかったので起動しました）") } else { String::new() };
    let head = if finished {
        format!("{name} で開きました{launched}: {url}（handoff: 再生・広告・login は browser の YouTube のまま。Kangae には埋め込みません）")
    } else {
        format!("{name} に開くよう頼みました: {url}（open が {} 秒で終わらないので、窓で確かめます。機械が混んでいる時に起きます）", youtube::OPEN_WAIT.as_secs())
    };
    let started = Instant::now();
    let mut pid = browser.as_ref().and_then(|b| b.pid);
    let mut by = None;
    loop {
        if pid.is_none() {
            pid = youtube::default_browser(timeout).ok().flatten().and_then(|b| b.pid);
        }
        let raw = match pid.map(|p| reader.windows(p)) {
            Some(WindowsRead::Read { list, by: b, .. }) => {
                by = Some(b);
                list
            }
            Some(WindowsRead::Unchecked { why, no_reader: true }) => return Ok(format!("{head}\n窓は未確認です（{why}）")),
            Some(WindowsRead::Unchecked { why, .. }) if started.elapsed() > Duration::from_secs(20) => return Ok(format!("{head}\n窓を確かめられませんでした（{why}）")),
            _ => Vec::new(),
        };
        let via = by.map(|b| format!("・{} で確認", b.label())).unwrap_or_default();
        let after = to_wins(&raw);
        if let Some(w) = youtube::opened_window(&before, &after).filter(|w| w.here && w.title.contains("YouTube")) {
            return Ok(format!("{head}\n前面の窓: 「{}」（今の画面{via}）", w.title));
        }
        let front = raw.iter().filter(|w| w.here()).max_by_key(|w| w.z.unwrap_or(i64::MIN));
        if let (Some(f), Some(p)) = (front, pid) {
            if reader.value_contains(p, f, &format!("watch?v={id}")) == Ok(true) {
                return Ok(format!("{head}\n前面の窓: 「{}」（今の画面。address bar が watch?v={id}{via}）", f.title));
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            return Ok(match youtube::opened_window(&before, &after) {
                Some(w) => format!("{head}\n窓「{}」は今の画面に在りません（別の Space か最小化）。{name} に切り替えてください", w.title),
                None => format!("{head}\n20 秒待っても、今の画面の {name} の窓に watch?v={id} が見つかりませんでした。{name} の画面を確かめてください"),
            });
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// 題名 — artist — album（空の欄は省く）
fn names(t: &Track) -> String {
    [&t.title, &t.artist, &t.album].iter().filter(|s| !s.is_empty()).map(|s| s.as_str()).collect::<Vec<_>>().join(" — ")
}

/// 秒 → m:ss（1 時間以上は h:mm:ss）。無ければ --:--
fn length(secs: Option<f64>) -> String {
    match secs {
        Some(s) if s.is_finite() => {
            let s = s.max(0.0).round() as u64;
            if s >= 3600 {
                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            } else {
                format!("{}:{:02}", s / 60, s % 60)
            }
        }
        _ => "--:--".to_string(),
    }
}
