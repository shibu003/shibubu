//! 鼻歌を録って、試聴して、REAPER に入れる CLI（docs/ACCEPTANCE.md test3・v0.13 M6 の土台）。
//!   kangae-rec --seconds 3 --out hum.wav   mic から最大 3 秒録って 44.1k / 16bit / mono の wav を書く（Enter で止めて残す）
//!   kangae-rec --play hum.wav              afplay で試聴（Enter で止める）
//!   kangae-rec --insert hum.wav            起動中の REAPER の選択 track の cursor 位置へ入れて、読み戻して確かめる（同じ take は入れない）
//!   kangae-rec --undo hum.wav              その insert を REAPER の undo で 1 回戻す（最後の操作がその insert の時だけ）
//! src/main.rs とは独立に compile する（audio mod を #[path] で取り込む）。分岐は docs/diagrams.md §10。

#[allow(dead_code)]
#[path = "../audio/mod.rs"]
mod audio;

use audio::preview::{peaks, sparkline, Preview};
use audio::reaper::{Item, Receipt};
use audio::tempo;
use audio::record::{self, Ended, MicPermission};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "使い方:
  kangae-rec [--seconds N] [--out FILE.wav]   mic から最大 N 秒（既定 3）録って wav を書く（--out を省くと ~/Music/Kangae/kangae-hum-<日時>.wav）
                                              録音中: Enter で止めて録れた分を残す / Ctrl-C で取り消して何も残さない
  kangae-rec --play FILE.wav                  afplay で試聴する（Enter で止める）
  kangae-rec --insert FILE.wav [--timeout S]  起動中の REAPER の選択 track の cursor 位置へ入れる（選択が無ければ新しい track）
                                              入った item を読み戻して確かめる。同じ take が既に在れば入れない
  kangae-rec --undo FILE.wav [--timeout S]    その insert を REAPER の undo で 1 回戻す（REAPER の最後の操作がその insert の時だけ）
環境変数: KANGAE_AUDIO_FAKE=1（mic を使わず無音を録る）  KANGAE_REAPER_BIN（REAPER の実行 file）
          KANGAE_AFPLAY_BIN / KANGAE_OSASCRIPT_BIN（afplay / osascript の差し替え・test 用）";

/// 波形 preview の本数（terminal の 1 行）
const BINS: usize = 48;

fn main() {
    // 図 §10 E4: Ctrl-C は process ごと止め、書き出す前の音は残さない。
    // 親が SIGINT を無視・塞いでいると（cargo test が子へその mask を渡す）kill -INT が届かず、
    // 録り切って exit する。その wait の signal は None。既定に戻して塞ぎを外す。
    arm_ctrl_c();
    std::process::exit(run(std::env::args().skip(1).collect()));
}

/// SIGINT の disposition を既定（process を止める）に戻し、親から継いだ塞ぎを外す。
fn arm_ctrl_c() {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
        fn sigemptyset(set: *mut u32) -> i32;
        fn sigaddset(set: *mut u32, signo: i32) -> i32;
        fn sigprocmask(how: i32, set: *const u32, old: *mut u32) -> i32;
    }
    unsafe {
        signal(2, 0); // SIGINT, SIG_DFL
        let mut set = 0u32;
        sigemptyset(&mut set);
        sigaddset(&mut set, 2);
        sigprocmask(2, &set, std::ptr::null_mut()); // SIG_UNBLOCK
    }
}

fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} に値が要ります"))
}

fn run(args: Vec<String>) -> i32 {
    let mut seconds = 3.0f32;
    let mut out: Option<PathBuf> = None;
    let mut insert: Option<PathBuf> = None;
    let mut play: Option<PathBuf> = None;
    let mut undo: Option<PathBuf> = None;
    let mut timeout = audio::reaper::DEFAULT_TIMEOUT;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let r: Result<(), String> = match a.as_str() {
            "--seconds" => need(&mut it, &a)
                .and_then(|v| v.parse::<f32>().map_err(|_| format!("--seconds は数値で: {v}")))
                .map(|v| seconds = v),
            "--out" => need(&mut it, &a).map(|v| out = Some(PathBuf::from(v))),
            "--insert" => need(&mut it, &a).map(|v| insert = Some(PathBuf::from(v))),
            "--play" => need(&mut it, &a).map(|v| play = Some(PathBuf::from(v))),
            "--undo" => need(&mut it, &a).map(|v| undo = Some(PathBuf::from(v))),
            "--timeout" => need(&mut it, &a)
                .and_then(|v| v.parse::<u64>().map_err(|_| format!("--timeout は秒の整数で: {v}")))
                .map(|v| timeout = Duration::from_secs(v)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => Err(format!("不明な引数: {other}")),
        };
        if let Err(e) = r {
            eprintln!("{e}\n{USAGE}");
            return 2;
        }
    }
    let r = if let Some(wav) = play {
        play_cli(&wav)
    } else if let Some(wav) = insert {
        insert_cli(&wav, timeout)
    } else if let Some(wav) = undo {
        audio::reaper::undo(&wav, timeout).map(|desc| {
            println!("REAPER の undo を 1 回呼びました: 「{desc}」を戻しました（その item が消えたのを読み戻しで確かめました）");
            0
        })
    } else {
        record_cli(seconds, out)
    };
    match r {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Enter（1 行）で立つ flag。stdin が閉じている（EOF）時は立てない（UI や script から呼ばれても勝手に止まらない）。
fn stop_on_enter() -> &'static AtomicBool {
    static STOP: AtomicBool = AtomicBool::new(false);
    std::thread::spawn(|| {
        let mut line = String::new();
        if matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) {
            STOP.store(true, Ordering::SeqCst);
        }
    });
    &STOP
}

/// --out を省いた時の保存先: ~/Music/Kangae/kangae-hum-<日時>.wav（temp dir は macOS が消すので、REAPER が参照する take を置かない）。
/// 同じ名前が在れば -2, -3 …
fn default_out() -> PathBuf {
    let dir = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Music/Kangae")).unwrap_or_else(std::env::temp_dir);
    let stamp = Command::new("/bin/date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::process::id().to_string());
    let base = dir.join(format!("kangae-hum-{stamp}.wav"));
    (1..).map(|n| record::numbered(&base, n)).find(|p| !p.exists()).unwrap_or(base)
}

fn record_cli(seconds: f32, out: Option<PathBuf>) -> Result<i32, String> {
    let out = out.unwrap_or_else(default_out);
    // mic の確認（混んだ機械で数秒）より先に、安い検査で止める
    record::preflight(seconds, &out)?;
    let fake = record::fake_enabled();
    let perm = if fake {
        MicPermission::Authorized
    } else {
        println!("mic の許可を確かめています…");
        record::mic_permission(Duration::from_secs(15))
    };
    if let Some(why) = perm.refusal() {
        return Err(why);
    }
    match &perm {
        MicPermission::NotDetermined => eprintln!("初めて mic を使います。macOS が mic の使用を聞くので「OK」を押してください（押すまでの間は無音で録れます）"),
        MicPermission::Unknown(why) => eprintln!("mic の許可を確かめられませんでした（{why}）。そのまま録ります"),
        _ => {}
    }
    let take = record::record(seconds, &out, stop_on_enter(), || {
        println!("録音中…（最大 {seconds} 秒。Enter で止めて録れた分を残す / Ctrl-C で取り消して何も残さない）")
    })?;
    let i = &take.info;
    let summary = format!(
        "{:.2} 秒 / {} Hz / {} bit / {} ch / peak {:.3}",
        i.seconds(),
        i.sample_rate,
        i.bits,
        i.channels,
        i.peak
    );
    let p = take.path.display();
    let code = match &take.ended {
        Ended::Full => {
            println!("{p} に書きました: {summary}");
            0
        }
        Ended::Stopped => {
            println!("Enter で止めました。{p} に書きました: {summary}");
            0
        }
        Ended::DeviceLost(why) => {
            println!("{p} に書きました: {summary}");
            eprintln!("録音中に入力 device が外れたので、そこで止めました（{why}）。そこまでの {:.2} 秒分を {p} に残しました", i.seconds());
            1
        }
    };
    if let Ok(pk) = peaks(&take.path, BINS) {
        println!("波形: {}", sparkline(&pk));
    }
    for n in &take.notes {
        eprintln!("注意: {n}");
    }
    if i.peak == 0.0 && !fake {
        eprintln!(
            "注意: 全て無音でした。{}",
            match perm {
                MicPermission::Authorized => "mic は許可されています。入力の音量が 0 か mute になっていないか、System Settings › Sound › Input を確かめてください",
                MicPermission::NotDetermined => "mic の許可の確認が出ていたら「OK」を押してから、もう一度録ってください",
                _ => "mic の許可が無い可能性があります。System Settings › Privacy & Security › Microphone でこの terminal（または Kangae）を許可して、もう一度録ってください",
            }
        );
    }
    println!("次: kangae-rec --play {p}（試聴） / kangae-rec --insert {p}（REAPER へ）");
    Ok(code)
}

fn play_cli(wav: &Path) -> Result<i32, String> {
    let info = record::inspect(wav)?;
    let mut p = Preview::start(wav)?;
    println!("試聴中: {}（{:.2} 秒）— Enter で止める", wav.display(), info.seconds());
    if let Ok(pk) = peaks(wav, BINS) {
        println!("波形: {}", sparkline(&pk));
    }
    let stop = stop_on_enter();
    loop {
        if stop.load(Ordering::SeqCst) {
            p.stop();
            println!("止めました");
            return Ok(0);
        }
        if !p.is_playing() {
            p.wait()?;
            println!("再生し終えました");
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn insert_cli(wav: &Path, timeout: Duration) -> Result<i32, String> {
    let info = record::inspect(wav)?;
    let r = audio::reaper::insert(wav, timeout)?;
    let at = |it: &Item| {
        format!(
            "track \"{}\"{} / 位置 {:.3} 秒 / 長さ {:.3} 秒 / item {}",
            it.track,
            if it.new_track { "（新しい track）" } else { "" },
            it.position,
            it.length,
            it.guid
        )
    };
    let it = match &r {
        Receipt::Verified(it) => {
            println!("REAPER に入れました（読み戻して確かめました）: {}（{:.2} 秒 / {} Hz / {} ch）→ {}", wav.display(), info.seconds(), info.sample_rate, info.channels, at(it));
            println!("戻すには: kangae-rec --undo {}（REAPER の undo を 1 回。REAPER の Edit › Undo でも同じ）", wav.display());
            it
        }
        Receipt::Duplicate(it) => {
            println!("この take は既に REAPER に入っています（入れていません）: {}", at(it));
            it
        }
        Receipt::Unverified { item, reason } => {
            eprintln!("REAPER に item は増えましたが、読み戻しが合いません（{reason}）。REAPER の画面で確かめてください: {}", at(item));
            return Ok(1);
        }
        Receipt::EffectUnknown { reason } => {
            eprintln!("{reason}");
            return Ok(1);
        }
    };
    println!("{}", tempo_line(wav, info.seconds(), it.project_bpm));
    Ok(0)
}

/// 録音の BPM を推定して、project の tempo に合わせるかの提案（data を出すだけ。REAPER は編集しない）。
fn tempo_line(wav: &Path, seconds: f32, project_bpm: f64) -> String {
    if seconds < tempo::MIN_SECONDS {
        return format!("tempo: 録音が {} 秒未満なので見ていません（{} 秒以上録ると推定します）", tempo::MIN_SECONDS, tempo::MIN_SECONDS);
    }
    match tempo::estimate_bpm(wav) {
        Ok(Some(bpm)) => match tempo::propose(bpm, project_bpm) {
            Some(p) => format!("提案: {}\n  根拠: {}\n  変わる事: {}\n  （まだ何も変えていません）", p.text, p.why, p.what_changes),
            None => format!("tempo: 録音（約 {} BPM）は project の tempo（{project_bpm} BPM）と合っています", bpm.bpm.round()),
        },
        Ok(None) => "tempo: 録音から拍を読めませんでした（無音か、拍のはっきりしない音）".to_string(),
        Err(e) => format!("tempo: {e}"),
    }
}
