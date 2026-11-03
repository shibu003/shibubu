//! Music.app（osascript の出力）と YouTube（Data API の応答）の parse と CLI の分岐を fixture で。本物は叩かない:
//! osascript は KANGAE_OSASCRIPT_BIN の偽物、YouTube は 127.0.0.1 の偽 server（KANGAE_YOUTUBE_API_BASE）。
//! fixture の id（persistent ID・videoId）は架空。形は実物と同じ: Music.app は JXA が JSON.stringify する形で、2026-10-17 に実機の
//! library（1139 曲）の出力を同じ parse_library が読めた。YouTube の 200 は search.list の実応答（同日・10 件）を同じ parse_search が読めた。

#[allow(dead_code)]
#[path = "../src/media/mod.rs"]
mod media;

use media::apple_music::{osascript_error, parse_account, parse_library, parse_now, parse_play, valid_persistent_id, Account, Library, Track};
use media::youtube::{error_message, opened_window, parse_browser, parse_count, parse_search, unescape, valid_video_id, watch_url, Browser, Video, Win};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const LIBRARY_OUT: &str = r#"{"total":1234,"tracks":[
{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3,"downloaded":true},
{"id":"0F1E2D3C4B5A6978","title":"Blinding Lights","artist":"The Weeknd","album":"After Hours","seconds":200.04,"downloaded":false},
{"id":"1111222233334444","title":"Radio 1","artist":"","album":"","seconds":null,"downloaded":null}]}"#;
const PLAY_OK: &str = r#"{"matches":2,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3},"state":"playing","now":"6A1B2C3D4E5F6071"}"#;
const PLAY_PAUSED: &str = r#"{"matches":1,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3},"state":"paused","now":"6A1B2C3D4E5F6071"}"#;
const PLAY_OTHER: &str = r#"{"matches":1,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3},"state":"playing","now":"0F1E2D3C4B5A6978"}"#;
const NOW_PLAYING: &str = r#"{"running":true,"state":"playing","position":83.6,"track":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3}}"#;
const TCC_DENIED: &str = "execution error: Error: Error: Not authorized to send Apple events to Music. (-1743)\n";

const SEARCH_OK: &str = r#"{"kind":"youtube#searchListResponse","etag":"e","nextPageToken":"CAoQAA","regionCode":"JP",
"pageInfo":{"totalResults":1000000,"resultsPerPage":10},"items":[
{"kind":"youtube#searchResult","etag":"a","id":{"kind":"youtube#video","videoId":"fixture0001"},
 "snippet":{"publishedAt":"2020-01-01T00:00:00Z","channelId":"UC0","title":"夜に駆ける Official Music Video","description":"","thumbnails":{},"channelTitle":"Ayase / YOASOBI","liveBroadcastContent":"none","publishTime":"2020-01-01T00:00:00Z"}},
{"kind":"youtube#searchResult","etag":"b","id":{"kind":"youtube#video","videoId":"fixture-02_"},
 "snippet":{"title":"Tom &amp; Jerry &#39;Live&#39; &quot;Best&quot; &lt;3 &#x1F3B5;","channelTitle":"A&amp;B Records"}},
{"kind":"youtube#searchResult","etag":"c","id":{"kind":"youtube#channel","channelId":"UC1"},"snippet":{"title":"channel だけの結果","channelTitle":"x"}},
{"kind":"youtube#searchResult","etag":"d","id":{"kind":"youtube#video","videoId":"../etc"},"snippet":{"title":"形の違う id","channelTitle":"x"}}]}"#;
const QUOTA: &str = r#"{"error":{"code":403,"message":"The request cannot be completed because you have exceeded your <a href=\"/youtube/v3/getting-started#quota\">quota</a>.","errors":[{"message":"The request cannot be completed because you have exceeded your <a href=\"/youtube/v3/getting-started#quota\">quota</a>.","domain":"youtube.quota","reason":"quotaExceeded"}]}}"#;
// 400 / 403 の key 系は Google API の error model（errors[].reason + google.rpc.ErrorInfo の details[].reason）から組んだ物
const KEY_INVALID: &str = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","errors":[{"message":"API key not valid. Please pass a valid API key.","domain":"global","reason":"badRequest"}],"status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID","domain":"googleapis.com","metadata":{"service":"youtube.googleapis.com"}}]}}"#;
const DISABLED: &str = r#"{"error":{"code":403,"message":"YouTube Data API v3 has not been used in project 1 before or it is disabled.","errors":[{"message":"x","domain":"usageLimits","reason":"accessNotConfigured"}],"status":"PERMISSION_DENIED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"SERVICE_DISABLED","domain":"googleapis.com"}]}}"#;
const BLOCKED: &str = r#"{"error":{"code":403,"message":"Requests to this API youtube method youtube.api.v3.V3DataSearchService.List are blocked.","errors":[{"message":"x","domain":"global","reason":"forbidden"}],"status":"PERMISSION_DENIED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_SERVICE_BLOCKED","domain":"googleapis.com"}]}}"#;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kangae-media-test-{}-{name}", std::process::id()))
}

/// 偽の osascript（1 つを全 test で共有）。引数を NUL 区切りで $FAKE_ARGS に書き、$FAKE_OUT を stdout・$FAKE_ERR を stderr に出して
/// $FAKE_CODE で終わる。$FAKE_SLEEP があれば exec sleep で黙る（kill されれば sleep ごと消える）。
fn fake_osascript() -> PathBuf {
    static FAKE: OnceLock<PathBuf> = OnceLock::new();
    FAKE.get_or_init(|| {
        let p = tmp("osascript.sh");
        std::fs::write(
            &p,
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FAKE_ARGS\"\n[ -n \"$FAKE_SLEEP\" ] && exec sleep \"$FAKE_SLEEP\"\nprintf '%s' \"$FAKE_OUT\"\nprintf '%s' \"$FAKE_ERR\" >&2\nexit \"${FAKE_CODE:-0}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    })
    .clone()
}

fn media(args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kangae-media"));
    // 検索の回数の file を利用者の本物の data dir に書かせない（FX1: SHIBUBU_DATA_DIR が先に効くので外す）
    c.args(args)
        .env("KANGAE_OSASCRIPT_BIN", fake_osascript())
        .env_remove("SHIBUBU_DATA_DIR")
        .env("KANGAE_DATA_DIR", tmp("data"))
        // 窓の読みは本物の bridge / cua-driver を拾わない（使う test だけが偽物を渡す）
        .env("KANGAE_SURFACE_BRIDGE", "")
        .env("CUA_DRIVER_BIN", "")
        .env_remove("KANGAE_YOUTUBE_API_KEY")
        .env_remove("KANGAE_YOUTUBE_API_BASE");
    c
}

/// 偽 osascript で CLI を 1 回。戻り値は (出力, 偽 osascript が受け取った引数)
fn run_osa(args: &[&str], out: &str, err: &str, code: i32) -> (Output, Vec<String>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let argfile = tmp(&format!("args-{}", N.fetch_add(1, Ordering::SeqCst)));
    let o = media(args)
        .env("FAKE_ARGS", &argfile)
        .env("FAKE_OUT", out)
        .env("FAKE_ERR", err)
        .env("FAKE_CODE", code.to_string())
        .output()
        .unwrap();
    let got = std::fs::read(&argfile).map(|b| String::from_utf8_lossy(&b).split('\0').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
    std::fs::remove_file(&argfile).ok();
    (o, got)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// 127.0.0.1 の偽 YouTube。1 回だけ受けて status と body を返し、受け取った request（行と header）を返す。10 秒来なければ ""。
fn serve_once(status: &'static str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    l.set_nonblocking(true).unwrap();
    let h = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut s = loop {
            match l.accept() {
                Ok((s, _)) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => return String::new(),
            }
        };
        s.set_nonblocking(false).unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
        }
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(resp.as_bytes()).unwrap();
        String::from_utf8_lossy(&req).into_owned()
    });
    (base, h)
}

fn t(id: &str, title: &str, artist: &str, album: &str, seconds: Option<f64>, downloaded: Option<bool>) -> Track {
    Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), seconds, downloaded, cloud: None }
}

#[test]
fn library_output_parses_with_download_flag_and_missing_values() {
    let lib = parse_library(LIBRARY_OUT).unwrap();
    assert_eq!(
        lib,
        Library {
            total: 1234,
            tracks: vec![
                t("6A1B2C3D4E5F6071", "夜に駆ける", "YOASOBI", "THE BOOK", Some(261.3), Some(true)),
                t("0F1E2D3C4B5A6978", "Blinding Lights", "The Weeknd", "After Hours", Some(200.04), Some(false)),
                t("1111222233334444", "Radio 1", "", "", None, None),
            ],
        }
    );
    assert_eq!(parse_library(r#"{"total":0,"tracks":[]}"#).unwrap(), Library { total: 0, tracks: vec![] }, "空の library");
    assert!(parse_library("").unwrap_err().contains("Can't read Music's answer"));
    assert!(parse_library(r#"{"total":1}"#).unwrap_err().contains("has no tracks"));
}

#[test]
fn play_output_is_verified_only_when_the_asked_track_is_playing() {
    let p = parse_play(PLAY_OK).unwrap().unwrap();
    assert_eq!((p.matches, p.state.as_str(), p.track.title.as_str()), (2, "playing", "夜に駆ける"));
    assert!(p.verified());
    assert!(!parse_play(PLAY_PAUSED).unwrap().unwrap().verified(), "一時停止のまま");
    assert!(!parse_play(PLAY_OTHER).unwrap().unwrap().verified(), "別の曲が鳴っている");
    assert_eq!(parse_play(r#"{"matches":0}"#).unwrap(), None, "題名に当たる曲が無い");
}

#[test]
fn now_output_parses_running_states() {
    let n = parse_now(NOW_PLAYING).unwrap();
    assert!(n.running);
    assert_eq!((n.state.as_str(), n.position), ("playing", Some(83.6)));
    assert_eq!(n.track.unwrap().id, "6A1B2C3D4E5F6071");
    let off = parse_now(r#"{"running":false}"#).unwrap();
    assert!(!off.running && off.track.is_none());
    let stopped = parse_now(r#"{"running":true,"state":"stopped","position":null,"track":null}"#).unwrap();
    assert_eq!((stopped.state.as_str(), stopped.position, stopped.track), ("stopped", None, None));
}

#[test]
fn osascript_errors_map_to_next_steps() {
    assert!(osascript_error(TCC_DENIED).contains("Privacy & Security › Automation"));
    let e = osascript_error("execution error: Error: Error: AppleEvent timed out. (-1712)");
    assert!(e.contains("didn't respond") && e.contains("open file"), "Music の「Open」panel が開いていると Apple Event が 2 分で -1712 になる（2026-10-17 実測）: {e}");
    assert!(osascript_error("execution error: Error: Error: Application isn't running. (-600)").contains("quit partway"));
    assert!(!osascript_error("execution error: Error: Error: Can't get object. (-6000)").contains("quit partway"), "番号は () ごと照合する");
    assert_eq!(osascript_error("\n0:5: syntax error: x\nmore"), "Controlling Music failed: 0:5: syntax error: x");
}

#[test]
fn search_response_parses_videos_only_and_unescapes_titles() {
    let v: serde_json::Value = serde_json::from_str(SEARCH_OK).unwrap();
    assert_eq!(
        parse_search(&v).unwrap(),
        vec![
            Video { id: "fixture0001".into(), title: "夜に駆ける Official Music Video".into(), channel: "Ayase / YOASOBI".into() },
            Video { id: "fixture-02_".into(), title: "Tom & Jerry 'Live' \"Best\" <3 🎵".into(), channel: "A&B Records".into() },
        ],
        "channel の結果と形の違う id は捨てる"
    );
    assert!(parse_search(&serde_json::json!({"error": {}})).is_err());
    assert_eq!(unescape("&日本&;&amp;&#;&bogus;a&"), "&日本&;&&#;&bogus;a&", "知らない参照・& の直後の多 byte 文字はそのまま");
}

#[test]
fn search_errors_map_to_next_steps() {
    let quota = error_message(403, QUOTA, "夜に駆ける live");
    assert!(quota.contains("100 a day"), "{quota}");
    assert!(quota.ends_with("search_query=%E5%A4%9C%E3%81%AB%E9%A7%86%E3%81%91%E3%82%8B%20live"), "{quota}");
    assert!(error_message(400, KEY_INVALID, "q").contains("isn't valid"));
    assert!(error_message(403, DISABLED, "q").contains("isn't turned on"));
    assert!(error_message(403, BLOCKED, "q").contains("API restrictions"));
    let html = error_message(503, r#"{"error":{"code":503,"message":"Backend <b>Error</b>"}}"#, "q");
    assert_eq!(html, "YouTube is having trouble (HTTP 503: Backend Error). Wait a moment and try again");
    assert!(error_message(502, "<html>Bad Gateway</html>", "q").contains("HTTP 502)"), "JSON でない body");
}

#[test]
fn video_id_gate_keeps_open_to_watch_urls() {
    assert_eq!(watch_url("dQw4w9WgXcQ").unwrap(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    for bad in ["", "dQw4w9WgXc", "dQw4w9WgXcQQ", "/etc/hosts1", "file:///x.a", "dQw4w9WgX;Q", "ｄQw4w9WgXcQ"] {
        assert!(!valid_video_id(bad), "{bad}");
        assert!(watch_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn cli_music_library_prints_tracks_and_passes_limit() {
    let (o, args) = run_osa(&["music-library", "--limit", "3"], LIBRARY_OUT, "", 0);
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("1234 曲のうち先頭 3 曲"), "{out}");
    assert!(out.contains("  1. 夜に駆ける — YOASOBI — THE BOOK  4:21  download 済み"), "{out}");
    assert!(out.contains("  2. Blinding Lights — The Weeknd — After Hours  3:20  未 download"), "{out}");
    assert!(out.contains("  3. Radio 1  --:--  download 不明"), "{out}");
    assert_eq!(&args[..2], ["-l", "JavaScript"]);
    assert_eq!(&args[args.len() - 2..], ["--", "3"]);
    let (o, _) = run_osa(&["music-library"], r#"{"total":0,"tracks":[]}"#, "", 0);
    assert!(stdout(&o).contains("library に曲がありません"), "空の library は空と言う");
}

#[test]
fn cli_music_play_passes_title_as_argv_and_reports_verification() {
    let title = r#"夜に駆ける"); Application('Finder').delete(Path('/'"#;
    let (o, args) = run_osa(&["music-play", title], PLAY_OK, "", 0);
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    assert_eq!(args.last().map(String::as_str), Some(title), "題名は argv の最後に 1 つで渡る");
    let script = &args[args.iter().position(|a| a == "-e").unwrap() + 1];
    assert!(!script.contains("Finder"), "題名は script に埋め込まない");
    let out = stdout(&o);
    assert!(out.contains("再生中: 夜に駆ける — YOASOBI — THE BOOK"), "{out}");
    assert!(out.contains("他に 1 曲"), "{out}");
    let (o, _) = run_osa(&["music-play", "夜に駆ける"], PLAY_PAUSED, "", 0);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("確かめられませんでした: Music didn't start playing after 10 s (state: Paused"), "{}", stderr(&o));
    let (o, _) = run_osa(&["music-play", "無い曲"], r#"{"matches":0}"#, "", 0);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("“無い曲” wasn't found in the Music library"));
}

#[test]
fn cli_music_now_and_permission_denied() {
    let (o, _) = run_osa(&["music-now"], NOW_PLAYING, "", 0);
    assert_eq!(stdout(&o).trim(), "Playing: 夜に駆ける — YOASOBI — THE BOOK  1:24 / 4:21");
    let (o, _) = run_osa(&["music-now"], r#"{"running":false}"#, "", 0);
    assert!(stdout(&o).contains("Music.app は起動していません"));
    let (o, _) = run_osa(&["music-now"], "", TCC_DENIED, 1);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Automation"), "{}", stderr(&o));
}

#[test]
fn cli_osascript_deadline_kills_and_points_at_the_permission_dialog() {
    let started = Instant::now();
    let o = media(&["music-now", "--timeout", "1"]).env("FAKE_ARGS", tmp("args-sleep")).env("FAKE_SLEEP", "20").output().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "deadline で切れる（{:?}）", started.elapsed());
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("didn't respond within 1 s"), "{}", stderr(&o));
    assert!(stderr(&o).contains("a prompt asking permission"), "{}", stderr(&o));
    std::fs::remove_file(tmp("args-sleep")).ok();
}

#[test]
fn cli_yt_search_sends_key_in_header_not_in_url() {
    let (base, server) = serve_once("200 OK", SEARCH_OK);
    let o = media(&["yt-search", "夜に駆ける", "live"]).env("KANGAE_YOUTUBE_API_KEY", "fake-key-123").env("KANGAE_YOUTUBE_API_BASE", &base).output().unwrap();
    let req = server.join().unwrap();
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let line = req.lines().next().unwrap_or("");
    assert!(line.starts_with("GET /search?"), "{line}");
    for p in ["part=snippet", "type=video", "maxResults=10", "q=%E5%A4%9C%E3%81%AB%E9%A7%86%E3%81%91%E3%82%8B+live"] {
        assert!(line.contains(p), "{p} が無い: {line}");
    }
    assert!(!line.contains("fake-key-123") && !line.contains("key="), "key を URL に入れない: {line}");
    assert!(req.lines().any(|h| h.eq_ignore_ascii_case("x-goog-api-key: fake-key-123")), "{req}");
    let out = stdout(&o);
    assert!(out.contains("「夜に駆ける live」: 2 件"), "{out}");
    assert!(out.contains("  1. fixture0001  夜に駆ける Official Music Video — Ayase / YOASOBI"), "{out}");
    assert!(out.contains("  2. fixture-02_  Tom & Jerry 'Live' \"Best\" <3 🎵 — A&B Records"), "{out}");
}

#[test]
fn cli_yt_search_reports_quota_and_missing_key() {
    let (base, server) = serve_once("403 Forbidden", QUOTA);
    let o = media(&["yt-search", "q"]).env("KANGAE_YOUTUBE_API_KEY", "k").env("KANGAE_YOUTUBE_API_BASE", &base).output().unwrap();
    server.join().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("100 a day"), "{}", stderr(&o));
    let o = media(&["yt-search", "q"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("KANGAE_YOUTUBE_API_KEY") && stderr(&o).contains("turn on YouTube Data API v3"), "{}", stderr(&o));
}

#[test]
fn cli_rejects_bad_video_id_and_unknown_command() {
    let o = media(&["yt-open", "/System/Applications/Calculator.app"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Not a YouTube video ID"));
    let o = media(&["yt-play"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("使い方"));
    let o = media(&["music-play"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "題名が無い");
}

// ---- p-audio 4 本目: 再生の Verified / Unverified / EffectUnknown・Apple Music の契約・YouTube の回数・browser・前面の窓 ----

/// 2026-10-17 の実機の PLAY と同じ形（cloud・downloaded・now_title 付き）
const PLAY_SUB_OK: &str = r#"{"matches":1,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3,"downloaded":false,"cloud":"subscription"},"state":"playing","now":"6A1B2C3D4E5F6071","now_title":"夜に駆ける"}"#;
const PLAY_SAME_NAME: &str = r#"{"matches":2,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3,"downloaded":true,"cloud":"purchased"},"state":"playing","now":"7B2C3D4E5F607182","now_title":"夜に駆ける"}"#;
const PLAY_STOPPED: &str = r#"{"matches":1,"chosen":{"id":"6A1B2C3D4E5F6071","title":"夜に駆ける","artist":"YOASOBI","album":"THE BOOK","seconds":261.3,"downloaded":false,"cloud":"subscription"},"state":"stopped","now":null,"now_title":null}"#;

#[test]
fn cli_music_play_verified_only_on_matching_persistent_id() {
    let (o, args) = run_osa(&["music-play", "夜に駆ける"], PLAY_SUB_OK, "", 0);
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    assert_eq!(&args[args.len() - 2..], ["title", "夜に駆ける"], "探し方と題名は argv で");
    let out = stdout(&o);
    assert!(out.contains("persistent ID 6A1B2C3D4E5F6071 の一致を読み戻して確認・A song from Apple Music · Not downloaded to this Mac"), "{out}");
    // 同名の別曲（persistent ID が違う）が鳴っている → Unverified と --id の案内
    let (o, _) = run_osa(&["music-play", "夜に駆ける"], PLAY_SAME_NAME, "", 0);
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(e.contains("A different song is playing (“夜に駆ける” · ID 7B2C3D4E5F607182)") && e.contains("There are 2 songs with the same title") && e.contains("music-play --id"), "{e}");
    // 鳴らなかった: 状態と曲の素性だけを書き、理由を推測で足さない
    let (o, _) = run_osa(&["music-play", "夜に駆ける"], PLAY_STOPPED, "", 0);
    let e = stderr(&o);
    assert!(e.contains("(state: Stopped · this song: A song from Apple Music · Not downloaded to this Mac)") && e.contains("Check the message on Music's screen"), "{e}");
}

#[test]
fn cli_music_play_by_id_checks_the_id_and_passes_it_as_argv() {
    let (o, args) = run_osa(&["music-play", "--id", "6A1B2C3D4E5F6071"], PLAY_SUB_OK, "", 0);
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    assert_eq!(&args[args.len() - 2..], ["id", "6A1B2C3D4E5F6071"]);
    let (o, args) = run_osa(&["music-play", "--id", "6A1B'); Application('Finder')"], PLAY_SUB_OK, "", 0);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Not a persistent ID") && args.is_empty(), "osascript を呼ばない: {}", stderr(&o));
    assert!(valid_persistent_id("0f1e2d3c4b5a6978") && !valid_persistent_id("0F1E2D3C4B5A697") && !valid_persistent_id("0F1E2D3C4B5A697G"));
}

#[test]
fn cli_music_play_timeout_is_effect_unknown_and_not_resent() {
    let started = Instant::now();
    let args = tmp("args-play-sleep");
    let o = media(&["music-play", "夜に駆ける", "--timeout", "1"]).env("FAKE_ARGS", &args).env("FAKE_SLEEP", "20").output().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(e.contains("Can't tell whether playback started (it won't be sent again on its own)") && e.contains("music-now"), "{e}");
    let (o, _) = run_osa(&["music-play", "夜に駆ける"], "", "execution error: Error: Error: AppleEvent timed out. (-1712)\n", 1);
    assert!(stderr(&o).contains("Apple event timeout (-1712). Can't tell whether playback started"), "-1712 も届いたか分からない: {}", stderr(&o));
    let (o, _) = run_osa(&["music-play", "夜に駆ける"], "", TCC_DENIED, 1);
    assert!(stderr(&o).contains("Automation") && !stderr(&o).contains("Can't tell"), "拒否は届いていない: {}", stderr(&o));
    std::fs::remove_file(args).ok();
}

#[test]
fn account_cache_reads_only_signin_and_subscription() {
    assert_eq!(parse_account(r#"{"known":false}"#).unwrap(), None, "cache が無い・形が違う");
    assert_eq!(parse_account(r#"{"known":true,"signed_in":false}"#).unwrap(), Some(Account { signed_in: false, subscription: None }));
    assert_eq!(parse_account(r#"{"known":true,"signed_in":true,"subscription":"Enabled"}"#).unwrap(), Some(Account { signed_in: true, subscription: Some(true) }));
    assert_eq!(parse_account(r#"{"known":true,"signed_in":true,"subscription":"Disabled"}"#).unwrap(), Some(Account { signed_in: true, subscription: Some(false) }));
    assert_eq!(parse_account(r#"{"known":true,"signed_in":true,"subscription":null}"#).unwrap().unwrap().subscription, None);
    assert!(parse_account("oops").is_err());
}

#[test]
fn youtube_rate_limits_are_not_the_daily_quota() {
    for (code, body) in [
        (429, r#"{"error":{"code":429,"message":"Too Many Requests"}}"#),
        (403, r#"{"error":{"code":403,"errors":[{"reason":"rateLimitExceeded"}]}}"#),
        (403, r#"{"error":{"code":403,"errors":[{"reason":"userRateLimitExceeded"}]}}"#),
    ] {
        let e = error_message(code, body, "q");
        assert!(e.contains("in a short time") && e.contains("Wait about a minute") && !e.contains("limit"), "{code}: {e}");
    }
    assert!(error_message(403, QUOTA, "q").contains("reset at midnight Pacific time"));
}

#[test]
fn youtube_search_count_is_per_pacific_day() {
    assert_eq!(parse_count("2026-10-17 7\n", "2026-10-17"), 7);
    assert_eq!(parse_count("2026-10-16 99\n", "2026-10-17"), 0, "日が替われば 0");
    assert_eq!(parse_count("", "2026-10-17"), 0);
    assert_eq!(parse_count("garbage", "2026-10-17"), 0);
    // CLI: 応答が来た検索ごとに数え、出力に「この Kangae から N 回目」
    let dir = tmp("data-count");
    let _ = std::fs::remove_dir_all(&dir);
    for n in 1..=2 {
        let (base, server) = serve_once("200 OK", SEARCH_OK);
        let o = media(&["yt-search", "q"]).env("KANGAE_YOUTUBE_API_KEY", "k").env("KANGAE_YOUTUBE_API_BASE", &base).env("KANGAE_DATA_DIR", &dir).output().unwrap();
        server.join().unwrap();
        assert!(stdout(&o).contains(&format!("search number {n} from Shibubu today (Pacific time) · the default limit is 100 a day (searches made with the same key elsewhere aren't counted")), "{}", stdout(&o));
    }
    let (base, server) = serve_once("403 Forbidden", QUOTA);
    media(&["yt-search", "q"]).env("KANGAE_YOUTUBE_API_KEY", "k").env("KANGAE_YOUTUBE_API_BASE", &base).env("KANGAE_DATA_DIR", &dir).output().unwrap();
    server.join().unwrap();
    let saved = std::fs::read_to_string(dir.join("youtube-searches.txt")).unwrap();
    assert!(saved.ends_with(" 3\n"), "失敗した呼び出しも枠を使うので数える: {saved:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn browser_and_opened_window_are_read() {
    assert_eq!(
        parse_browser(r#"{"bundle_id":"com.google.Chrome","name":"Google Chrome","pid":4242}"#).unwrap(),
        Some(Browser { bundle_id: "com.google.Chrome".into(), name: "Google Chrome".into(), pid: Some(4242) })
    );
    assert_eq!(parse_browser(r#"{"bundle_id":"com.apple.Safari","name":"","pid":null}"#).unwrap().unwrap().name, "com.apple.Safari");
    assert_eq!(parse_browser(r#"{"bundle_id":null}"#).unwrap(), None);
    let w = |id: i64, title: &str, here: bool| Win { id, title: title.into(), here };
    let before = vec![w(1, "猫 - YouTube - Google Chrome", true), w(2, "Gmail", true)];
    let after = vec![w(1, "猫 - YouTube - Google Chrome", true), w(3, "夜に駆ける - YouTube", false), w(2, "夜に駆ける - YouTube", true)];
    assert_eq!(opened_window(&before, &after).map(|w| w.id), Some(2), "前から在った YouTube の窓は数えない・今の画面の物を先に");
    assert_eq!(opened_window(&before, &after[..2]).map(|w| (w.id, w.here)), Some((3, false)), "今の画面に無ければ画面外の物");
    assert_eq!(opened_window(&before, &before), None);
    let loading = vec![w(1, "猫 - YouTube - Google Chrome", true), w(2, "Gmail", true), w(310, "Untitled", true)];
    assert_eq!(opened_window(&before, &loading).map(|w| w.id), Some(310), "読み込み中で題名が Untitled の新しい窓");
    let retitled = vec![w(1, "猫 - YouTube - Google Chrome", true), w(2, "Gmail (3)", true)];
    assert_eq!(opened_window(&before, &retitled), None, "題名が変わっただけの YouTube でない窓は数えない");
}

/// 偽の open（引数を NUL 区切りで $FAKE_OPEN_ARGS に）。窓は偽の bridge（`yt_bridge`）で読む
fn yt_open_fakes(name: &str) -> (PathBuf, PathBuf) {
    let open = tmp(&format!("{name}-open.sh"));
    std::fs::write(&open, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FAKE_OPEN_ARGS\"\n").unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    (open, tmp(&format!("{name}-open-args")))
}

#[test]
fn cli_yt_open_hands_to_the_running_default_browser_and_reads_the_front_window() {
    let (open, open_args) = yt_open_fakes("ytopen");
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytopen"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Google Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &open)
        .env("FAKE_OPEN_ARGS", &open_args)
        .env("KANGAE_SURFACE_BRIDGE", yt_bridge("ytopen", &[]))
        .output()
        .unwrap();
    assert!(o.status.success(), "stderr: {}", stderr(&o));
    let args: Vec<String> = String::from_utf8_lossy(&std::fs::read(&open_args).unwrap()).split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
    assert_eq!(args, ["-b", "com.google.Chrome", "https://www.youtube.com/watch?v=dQw4w9WgXcQ"], "-a / -n / -g を付けない");
    let out = stdout(&o);
    assert!(out.contains("Google Chrome で開きました: https://www.youtube.com/watch?v=dQw4w9WgXcQ") && !out.contains("起動していなかった"), "{out}");
    assert!(out.contains("前面の窓: 「夜に駆ける - YouTube」（今の画面・macOS bridge で確認）"), "{out}");
}

/// 偽の bridge（tests/fake-surface-bridge.py）。window_list の 1 回目は YouTube の窓が無く、2 回目から在る
fn yt_bridge(name: &str, extra: &[(&str, &str)]) -> PathBuf {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py");
    let mut env = vec![(
        "FAKE_WL",
        r#"[[{"title":"Gmail","on_screen":true}],[{"title":"Gmail","on_screen":true},{"title":"夜に駆ける - YouTube","on_screen":true}]]"#,
    )];
    env.extend_from_slice(extra);
    let exports: String = env.iter().map(|(k, v)| format!("export {k}='{v}'\n")).collect();
    let p = tmp(&format!("{name}-bridge.sh"));
    std::fs::write(&p, format!("#!/bin/sh\n{exports}exec python3 '{}'\n", py.display())).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn cli_yt_open_reads_the_front_window_through_the_bridge_without_cua_driver() {
    // CuaDriver が無い Mac: bridge の window_list で前面の窓を読み戻す
    let (open, open_args) = yt_open_fakes("ytbridge");
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytbridge"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Google Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &open)
        .env("FAKE_OPEN_ARGS", &open_args)
        .env("KANGAE_SURFACE_BRIDGE", yt_bridge("ytbridge", &[]))
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(out.contains("前面の窓: 「夜に駆ける - YouTube」（今の画面・macOS bridge で確認）"), "{out}{}", stderr(&o));
}

#[test]
fn cli_yt_open_says_unchecked_with_reasons_when_there_is_no_reader() {
    // bridge が無い → 窓は「未確認（理由）」。黙って成功にも「窓が無い」にもしない
    let (open, open_args) = yt_open_fakes("ytnone");
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytnone"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Google Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &open)
        .env("FAKE_OPEN_ARGS", &open_args)
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(out.contains("窓は未確認です（bridge: KANGAE_SURFACE_BRIDGE が空なので bridge を使いません）"), "{out}{}", stderr(&o));
    assert!(!out.contains("前面の窓"), "{out}");
}

#[test]
fn cli_yt_open_reads_the_address_bar_through_the_bridge() {
    // 題名が「Untitled」のまま（読み込み中）でも、bridge の ax_find で address bar の URL を読む
    let (open, open_args) = yt_open_fakes("ytbaddr");
    let bridge = yt_bridge(
        "ytbaddr",
        &[("FAKE_WL", r#"[[{"title":"Untitled","on_screen":true}]]"#), ("FAKE_AX_VALUE", "youtube.com/watch?v=dQw4w9WgXcQ")],
    );
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytbaddr"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &open)
        .env("FAKE_OPEN_ARGS", &open_args)
        .env("KANGAE_SURFACE_BRIDGE", &bridge)
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(out.contains("前面の窓: 「Untitled」（今の画面。address bar が watch?v=dQw4w9WgXcQ・macOS bridge で確認）"), "{out}{}", stderr(&o));
}

// ---- kangae-surface music / youtube（Source Surface の状態の data）----

fn surface_cmd(args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kangae-surface"));
    c.args(args).env("KANGAE_OSASCRIPT_BIN", fake_osascript()).env("KANGAE_SURFACE_BRIDGE", "").env("CUA_DRIVER_BIN", "").env_remove("KANGAE_YOUTUBE_API_KEY").env_remove("SHIBUBU_DATA_DIR");
    c
}

#[test]
fn cli_surface_youtube_is_available_manual_with_open_in_browser() {
    let dir = tmp("data-surface");
    let o = surface_cmd(&["youtube"])
        .env("FAKE_ARGS", tmp("args-surface-yt"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Google Chrome","pid":4242}"#)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    let l: Vec<&str> = out.lines().collect();
    assert_eq!(l[0], "detecting: YouTube を探しています");
    assert_eq!(
        l[1],
        "available_manual: YouTube は本人の browser で開きます。埋め込みは未対応（Kangae の画面に web を持たない）・代わり: 本人の browser（Google Chrome（起動中））",
        "CD-50 の検査文"
    );
    assert!(out.contains("[YouTube で開く]") && out.contains("API key が無いので、browser の YouTube で検索する"), "{out}");
    let o = surface_cmd(&["youtube"])
        .env("FAKE_ARGS", tmp("args-surface-yt"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.apple.Safari","name":"Safari","pid":null}"#)
        .env("KANGAE_YOUTUBE_API_KEY", "k")
        .env("KANGAE_DATA_DIR", &dir)
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(out.contains("Safari（起動していない。開く時に起動します）") && out.contains("search number 0 from Shibubu today"), "{out}");
    assert!(!out.contains("k）") && !out.contains(": k"), "key の値は出さない: {out}");
    let o = surface_cmd(&["youtube", "--open"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn cli_surface_music_degrades_when_music_does_not_answer() {
    // この Mac で Music.app が起動しているかで状態は変わる。起動中なら、偽 osascript の -1712 で degraded に落ちる事を見る
    let o = surface_cmd(&["music"])
        .env("FAKE_ARGS", tmp("args-surface-music"))
        .env("FAKE_ERR", "execution error: Error: Error: AppleEvent timed out. (-1712)\n")
        .env("FAKE_CODE", "1")
        .output()
        .unwrap();
    let out = stdout(&o);
    let l: Vec<&str> = out.lines().collect();
    assert_eq!(l.first().copied(), Some("detecting: Music を探しています"), "{out}");
    let state = l.get(1).and_then(|s| s.split(':').next()).unwrap_or("");
    let running = Command::new("pgrep").args(["-x", "Music"]).output().map(|o| o.status.success()).unwrap_or(false);
    if running {
        assert_eq!(state, "degraded", "{out}");
        assert!(l[1].contains("操作に答えません") && l[1].contains("window left open"), "{out}");
        assert_eq!(o.status.code(), Some(1));
    } else {
        assert!(["available_manual", "unsupported"].contains(&state), "{out}");
    }
    assert!(out.contains("Apple Music: "), "契約の行は必ず出る: {out}");
}

#[test]
fn cli_yt_open_reads_the_address_bar_when_the_title_is_still_loading() {
    // 2026-10-17 の実物: load 700 の Chrome は読み込みが終わらず、前面の窓の題名は「Untitled」のまま。address bar の URL で確かめる
    let (open, open_args) = yt_open_fakes("ytaddr");
    // 前面の「Untitled」と、別の Space の窓（今の画面に無い）。address bar は bridge の ax_find で読む
    let bridge = yt_bridge(
        "ytaddr",
        &[
            ("FAKE_WL", r#"[[{"title":"Untitled","on_screen":true},{"title":"Google Chrome","on_screen":false}]]"#),
            ("FAKE_AX_VALUE", "youtube.com/watch?v=dQw4w9WgXcQ"),
        ],
    );
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytaddr"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &open)
        .env("FAKE_OPEN_ARGS", &open_args)
        .env("KANGAE_SURFACE_BRIDGE", &bridge)
        .output()
        .unwrap();
    let out = stdout(&o);
    assert!(out.contains("前面の窓: 「Untitled」（今の画面。address bar が watch?v=dQw4w9WgXcQ・macOS bridge で確認）"), "{out}{}", stderr(&o));
}

#[test]
fn cli_yt_open_does_not_hang_on_a_slow_open() {
    // 2026-10-17 の実物: load 900 で `open -b … URL` が 15 分以上返らなかった。待ち切らずに窓で確かめる
    let (_, open_args) = yt_open_fakes("ytslow");
    let slow = tmp("ytslow-open.sh");
    std::fs::write(&slow, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FAKE_OPEN_ARGS\"\nexec sleep 40\n").unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = Instant::now();
    let o = media(&["yt-open", "dQw4w9WgXcQ"])
        .env("FAKE_ARGS", tmp("args-ytslow"))
        .env("FAKE_OUT", r#"{"bundle_id":"com.google.Chrome","name":"Chrome","pid":4242}"#)
        .env("KANGAE_OPEN_BIN", &slow)
        .env("FAKE_OPEN_ARGS", &open_args)
        .env("KANGAE_SURFACE_BRIDGE", yt_bridge("ytslow", &[]))
        .output()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(38), "open の 40 秒を待たない（{:?}）", started.elapsed());
    let out = stdout(&o);
    assert!(out.contains("Chrome に開くよう頼みました") && out.contains("15 秒で終わらない") && out.contains("前面の窓: 「夜に駆ける - YouTube」"), "{out}{}", stderr(&o));
}
