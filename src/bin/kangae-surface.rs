//! 元のアプリへの native handoff の試し口（v0.13 の render_mode = native_handoff・DESIGN.md §14.2）。
//!   kangae-surface reaper           REAPER を探し、起動しているか・窓が今の画面に在るかを状態で返す（起動しない・前に出さない）
//!   kangae-surface reaper --open    起動していなければ起動し、起動中なら前に出して、窓が出るまで待つ
//!   kangae-surface music [--open]   Music.app の状態に、操作に答えるか・Apple Music の sign in と契約・再生できる曲を足す
//!   kangae-surface youtube          YouTube は本人の browser への handoff だけ（available_manual ＋ [YouTube で開く]・CD-50）
//! 窓の確認は macOS bridge（bridge/macos-surface・CGWindowList + AX）だけ。bridge が無ければ「未確認（理由）」。
//! 手で操作するのに bridge は要らない。src/main.rs とは独立に compile する。分岐は docs/diagrams.md §10・§12・§18。

#[allow(dead_code)] // 値の読み（WindowReader::value_contains）は yt-open だけが使う
#[path = "../audio/surface.rs"]
mod surface;
#[allow(dead_code)]
#[path = "../media/mod.rs"]
mod media;

use media::{apple_music, youtube};
use std::process::Command;
use std::time::Duration;
use surface::{State, Surface};

const USAGE: &str = "使い方:
  kangae-surface reaper|music [--open] [--timeout S]
    REAPER / Music.app を探して、状態（detecting / available_manual / ready / degraded / unsupported）を返す
    --open     起動していなければ起動し、起動中なら前に出して、窓が今の画面に出るまで待つ（既定 90 秒・--timeout S）
               --open が無ければ起動も前に出す事もしない
    music は、操作に答えるか（答えなければ degraded）・Apple Music の sign in と契約（macOS の cache）・再生できる曲を足す
  kangae-surface youtube
    YouTube は本人の browser で開く（available_manual ＋ [YouTube で開く]）。既定の browser・API key の有無・今日の検索の回数
終了: ready / available_manual は 0、degraded / unsupported は 1
窓の確認: macOS bridge だけ（無ければ「未確認（理由）」）
環境変数: KANGAE_SURFACE_BRIDGE（bridge の場所・空なら使わない）・KANGAE_BRIDGE_AX_TIMEOUT（AX の応答を待つ秒）";

/// Music.app（macOS に同梱）
const MUSIC: surface::App = surface::App {
    name: "Music",
    bundle_id: "com.apple.Music",
    process: "Music",
    download: "macOS に同梱（/System/Applications/Music.app）",
};

/// 操作に答えるかを見る deadline（Apple Event の既定の timeout は 2 分なので、それより短く切る）
const PING: Duration = Duration::from_secs(15);

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(args: Vec<String>) -> i32 {
    let mut it = args.into_iter();
    let target = it.next().unwrap_or_default();
    let mut open = false;
    let mut timeout = Duration::from_secs(90);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--open" => open = true,
            "--timeout" => match it.next().and_then(|v| v.parse::<u64>().ok()) {
                Some(s) => timeout = Duration::from_secs(s),
                None => {
                    eprintln!("--timeout は秒の整数で\n{USAGE}");
                    return 2;
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("不明な引数: {other}\n{USAGE}");
                return 2;
            }
        }
    }
    let app = match target.as_str() {
        "reaper" => surface::REAPER,
        "music" => MUSIC,
        "youtube" if !open => return youtube_surface(),
        "youtube" => {
            eprintln!("youtube に --open はありません（動画は kangae-media yt-open <videoId> で開く）\n{USAGE}");
            return 2;
        }
        "-h" | "--help" => {
            println!("{USAGE}");
            return 0;
        }
        "" => {
            eprintln!("アプリの名前が要ります（reaper / music / youtube）\n{USAGE}");
            return 2;
        }
        other => {
            eprintln!("{other} にはまだ対応していません（reaper / music / youtube）\n{USAGE}");
            return 2;
        }
    };
    println!("{}: {} を探しています{}", State::Detecting.as_str(), app.name, if open { "（見つかれば開いて、窓が出るまで待ちます）" } else { "" });
    let mut s = surface::probe(&app, open, timeout);
    let mut extra = Vec::new();
    if app.bundle_id == MUSIC.bundle_id {
        music_extra(&mut s, &mut extra);
    }
    println!("{}: {}", s.state.as_str(), s.reason);
    let mut detail = Vec::new();
    if let Some(p) = &s.app_path {
        detail.push(format!("app {}{}", p.display(), s.version.as_ref().map(|v| format!("（{v}）")).unwrap_or_default()));
    }
    if let Some(pid) = s.pid {
        detail.push(format!("pid {pid}"));
    }
    if let Some(w) = &s.window {
        let title = if w.title.trim().is_empty() { String::new() } else { format!("「{}」", w.title) };
        detail.push(format!("window {}{title}{}", w.id, if w.here() { "（今の画面）" } else { "（今の画面の外）" }));
    }
    if s.state != State::Unsupported {
        detail.push("操作: 手で（manual）".to_string());
    }
    if let Some((by, _)) = &s.reader {
        detail.push(format!("窓の確認: {}", by.label()));
    }
    if let Some(t) = clock() {
        detail.push(format!("確認 {t}"));
    }
    println!("  {}", detail.join(" / "));
    if let Some((_, Some(note))) = &s.reader {
        println!("  {note}");
    }
    for l in extra {
        println!("  {l}");
    }
    match s.state {
        State::AvailableManual if !open => println!("次: kangae-surface {target} --open（起動 / 前に出す）"),
        State::Degraded => println!("次: 少し待って kangae-surface {target} を打ち直す（{} に開いたままの画面や dialog があれば閉じる）", app.name),
        _ => {}
    }
    match s.state {
        State::Ready | State::AvailableManual => 0,
        _ => 1,
    }
}

/// Music.app: 起動中なら操作に答えるか（窓が在っても、開いたままの panel で Apple Event に答えない事がある）と、
/// Apple Music の sign in・契約（macOS の cache）・再生できる曲を足す。
fn music_extra(s: &mut Surface, extra: &mut Vec<String>) {
    if s.pid.is_some() {
        if let Err(e) = apple_music::responding(PING) {
            s.state = State::Degraded;
            s.reason = format!("Music.app は起動中ですが、操作に答えません: {e}");
        }
    }
    match apple_music::account(PING) {
        Ok(Some(a)) => {
            extra.push(apple_music::account_text(&a));
            extra.push(
                match (a.signed_in, a.subscription) {
                    (true, Some(true)) => "再生できる曲: library の曲（Apple Music の曲も）",
                    (true, None) => "再生できる曲: この Mac に在る曲（Apple Music の曲は契約の状態が分からない）",
                    _ => "再生できる曲: この Mac に在る曲（Apple Music の曲は契約が要る）",
                }
                .to_string(),
            );
        }
        Ok(None) => extra.push("Apple Music: sign in の状態を確かめられませんでした（macOS の cache が無い）".to_string()),
        Err(e) => extra.push(format!("Apple Music: {e}")),
    }
}

/// YouTube は Kangae に埋め込まず、本人の browser に handoff する（CD-50）。状態は available_manual に固定。
fn youtube_surface() -> i32 {
    println!("{}: YouTube を探しています", State::Detecting.as_str());
    let browser = match youtube::default_browser(PING) {
        Ok(Some(b)) => format!("{}（{}）", b.name, if b.pid.is_some() { "起動中" } else { "起動していない。開く時に起動します" }),
        Ok(None) => "既定の browser が見つかりません".to_string(),
        Err(e) => format!("既定の browser を確かめられませんでした: {e}"),
    };
    println!(
        "{}: YouTube は本人の browser で開きます。埋め込みは未対応（Kangae の画面に web を持たない）・代わり: 本人の browser（{browser}）",
        State::AvailableManual.as_str()
    );
    println!("  操作: [YouTube で開く]（kangae-media yt-open <videoId>）");
    match youtube::key_from_env() {
        Some(_) => println!("  検索: API key あり（kangae-media yt-search）/ {}", youtube::quota_text(youtube::searches_today())),
        None => println!("  検索: API key が無いので、browser の YouTube で検索する（Kangae から検索するには KANGAE_YOUTUBE_API_KEY を入れる）"),
    }
    0
}

/// 今の時刻（local の HH:MM:SS）
fn clock() -> Option<String> {
    let o = Command::new("/bin/date").arg("+%H:%M:%S").output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!t.is_empty()).then_some(t)
}
